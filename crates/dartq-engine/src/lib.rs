//! The dartq engine facade: one analysis context, two-phase content
//! updates, and topologically ordered library recompute (engine-api-spec
//! §1, incremental-l1-spec §3).
//!
//! # Shape (§1.1–§1.5)
//!
//! * One [`Engine`] binds the immutable context — content store, SDK
//!   snapshot, package identity table, and the pub config the URI layer
//!   resolves against. Context change = new Engine.
//! * [`Engine::set_content`] / [`Engine::remove_file`] only enqueue into the
//!   pending overlay (official `changeFile`: enqueue, never compute);
//!   [`Engine::apply_pending`] is the barrier that runs the ContentId gate,
//!   the per-file facts recompute, and the L1 cascade.
//! * [`Engine::analyze`] applies as its fixed first phase, recomputes dirty
//!   libraries in import-edge topological order on the batch fan-out, and
//!   returns the merged, canonically sorted diagnostic set.
//! * [`EngineEvent`]s arrive on a synchronous `FnMut` callback, in work
//!   order; the returned diagnostics are in canonical order — the two are
//!   deliberately distinct (§5 risk 1).
//!
//! # Module layout (§1.6)
//!
//! `state` (residency + overlay + reverse edges), `l1` (apply), `pipeline`
//! (analyze), `event`, `cancel`.
//!
//! # The semantic diagnostics seam (#120/#121)
//!
//! The engine assembles syntax (scan + parse recovery) and URI-layer
//! diagnostics itself. Semantic diagnosis — `undefined_*`, `wrong_number_*`,
//! the part/part-of family — plugs in through [`SemanticDiagnoser`]: the
//! trait lives here so the engine needs no dependency edge back upward, and
//! its implementor lives above the engine (the CLI wires a small adapter
//! onto the dartq-semantic diagnostic modules once #120/#121 land —
//! dartq-semantic itself cannot implement it without inverting the
//! dependency). With no diagnoser configured the engine under-reports
//! exactly those families and nothing else.

pub mod cancel;
pub mod event;
pub mod l1;
pub mod pipeline;
pub mod state;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use dartq_diagnostics::Diagnostic;
use dartq_package::PackageTable;
use dartq_pub::{PackageConfig, PubError};
use dartq_sdk::SdkSnapshot;
use dartq_source::{FileId, FileTable, LineIndex, MemorySourceStore, SourceStore};
use rustc_hash::FxHashMap;
use thiserror::Error;

pub use cancel::{CancellationToken, Cancelled};
pub use event::{EngineEvent, RevalidateGate};
pub use state::{DirectiveFacts, DirectiveKind, LibraryId, UnitFacts};

/// Everything immutable across the engine's lifetime (§1.1): the M1 slice
/// of PRD §5.4's six context fields — store, SDK, packages — plus the pub
/// config + config directory the URI-resolution table derives from.
pub struct EngineConfig {
    /// Content-addressed byte storage. The engine takes ownership: M1 is
    /// single-Engine-per-process (§5 risk 4), so the drafted `Arc` sharing
    /// returns with M3 multi-context. `MemorySourceStore` is the default.
    pub store: Box<dyn SourceStore + Send + Sync>,
    /// SDK snapshot (PRD §7); loading hard-errors on a crippled SDK before
    /// construction (dartq-sdk doc).
    pub sdk: SdkSnapshot,
    /// Package identity table over one resolution (PRD §8); carries the
    /// per-package language versions the engine derives per-file feature
    /// sets from.
    pub packages: PackageTable,
    /// The parsed `package_config.json` the identity table was built from —
    /// the URI-resolution face re-derives its lib-dir index from it.
    pub pub_config: Arc<PackageConfig>,
    /// The directory holding `package_config.json` (normally `.dart_tool/`).
    pub config_dir: PathBuf,
    /// The semantic diagnostics plug point (#120/#121); `None` keeps the
    /// engine's syntax + URI-layer set (under-reporting contract).
    pub semdiag: Option<Arc<dyn SemanticDiagnoser>>,
}

/// Engine construction and explicit-API failures.
#[derive(Debug, Error)]
pub enum EngineError {
    /// `remove_file` of a path the engine never interned and does not
    /// track (deliberate deviation from the official silent no-op,
    /// incremental-l1-spec §1.2: dartq reports explicitly).
    #[error("unknown path: {0}")]
    UnknownPath(PathBuf),
    /// The pub package config cannot back the URI-resolution table
    /// (dartq-pub's shared `resolve` face rejected it: a broken `rootUri`,
    /// duplicate names, a name outside the reader grammar, or a
    /// `packageUri` escaping its root).
    #[error("package config unusable: {0}")]
    PackageConfig(#[from] PubError),
}

/// The analysis driver (§1).
pub struct Engine {
    state: state::EngineState,
    graph: Option<pipeline::ResolvedGraph>,
    semdiag: Option<Arc<dyn SemanticDiagnoser>>,
}

impl Engine {
    /// Binds the context. `dart:` URIs resolve against the SDK's `vm`
    /// segment (target selection is an M2 context field).
    ///
    /// # Errors
    ///
    /// [`EngineError::PackageConfig`] when the config cannot back the
    /// URI-resolution table.
    pub fn new(config: EngineConfig) -> Result<Self, EngineError> {
        let semdiag = config.semdiag;
        let state = state::EngineState::new(
            config.packages,
            &config.pub_config,
            &config.config_dir,
            config.sdk,
            config.store,
        )?;
        Ok(Self {
            state,
            graph: None,
            semdiag,
        })
    }

    /// Enqueues a content override. Only touches the pending overlay — no
    /// derived state is invalidated here. Interns `path` if new and returns
    /// its `FileId`. Last write per path wins; `set_content` then
    /// `remove_file` before apply nets to a no-op for that path.
    ///
    /// The bytes decode as UTF-8 eagerly (the whole pipeline downstream is
    /// `&str`); invalid sequences become U+FFFD. The oracle never feeds
    /// invalid UTF-8, so this is documented behavior, not a parity surface.
    pub fn set_content(&mut self, path: &Path, bytes: &[u8]) -> FileId {
        let file = self.state.files.intern(path);
        if !self.state.pending.contains_key(&file) {
            self.state.pending_order.push(file);
        }
        let text = String::from_utf8_lossy(bytes).into_owned();
        self.state
            .pending
            .insert(file, state::PendingChange::Set(text));
        file
    }

    /// Enqueues a removal. Unknown path — never interned and not pending —
    /// is an explicit error (official `changeFile` silently no-ops; dartq
    /// reports, incremental-l1-spec §1.2).
    ///
    /// # Errors
    ///
    /// [`EngineError::UnknownPath`] when the path is neither pending nor
    /// resident.
    pub fn remove_file(&mut self, path: &Path) -> Result<(), EngineError> {
        if let Some(file) = self.state.files.try_file_id(path) {
            if !self.state.pending.contains_key(&file) {
                self.state.pending_order.push(file);
            }
            self.state
                .pending
                .insert(file, state::PendingChange::Remove);
            return Ok(());
        }
        Err(EngineError::UnknownPath(path.to_path_buf()))
    }

    /// Barrier: drains the pending queue and runs the L1 cascade
    /// (§3.2 steps 1–5). Events fire only for a non-empty queue.
    pub fn apply_pending(&mut self, on_event: &mut dyn FnMut(EngineEvent)) -> AffectedReport {
        let outcome = l1::apply(&mut self.state, on_event);
        AffectedReport {
            changed: outcome.changed,
            removed: outcome.removed,
            invalidated: outcome.invalidated,
        }
    }

    /// Phase 1: [`Self::apply_pending`]. Phase 2: recompute dirty libraries
    /// in import-edge topological order; clean libraries keep cached
    /// results. Returns the merged diagnostic set of every live library,
    /// canonically sorted (path bytes, offset, code name) — the superset the
    /// incremental-equals-full oracle contract needs (deviation note in
    /// engine-api-spec: the drafted per-`targets` return cannot satisfy the
    /// byte-equality arm, so `targets` drives only the AnalyzeFile
    /// amplification events).
    ///
    /// # Errors
    ///
    /// [`Cancelled`] when a checkpoint fired: completed libraries keep their
    /// cached results, the rest stay dirty.
    pub fn analyze(
        &mut self,
        targets: &[FileId],
        on_event: &mut dyn FnMut(EngineEvent),
        cancel: &CancellationToken,
    ) -> Result<Vec<Diagnostic>, Cancelled> {
        pipeline::analyze(
            &mut self.state,
            &mut self.graph,
            self.semdiag.as_deref(),
            targets,
            on_event,
            cancel,
        )
    }

    /// PRD §17.2 discovery face: package roots (from the identity table)
    /// plus `lib`/`bin`/`test`/`tool` per root and root-level `*.dart`
    /// files; never a blind tree walk (hidden dirs and
    /// `.git`/`build`/`node_modules`/`.dart_tool` always excluded). Interns
    /// paths, enqueues the bytes as a `set_content` batch, and reports the
    /// file/LOC counts the metrics hook needs (one pass; LOC counts lines).
    ///
    /// # Errors
    ///
    /// Propagates I/O errors from reading discovered files.
    pub fn discover_workspace(&mut self) -> std::io::Result<DiscoveryStats> {
        const SURFACES: [&str; 4] = ["lib", "bin", "test", "tool"];
        const EXCLUDED: [&str; 4] = [".git", "build", "node_modules", ".dart_tool"];

        let mut roots: Vec<PathBuf> = self
            .state
            .packages
            .iter()
            .map(|package| package.root.clone())
            .collect();
        roots.sort();
        roots.dedup();

        let mut stats = DiscoveryStats::default();
        for root in &roots {
            // The four declared surfaces plus directly-in-root dart files.
            let mut queue: Vec<PathBuf> =
                SURFACES.iter().map(|surface| root.join(surface)).collect();
            if let Ok(entries) = std::fs::read_dir(root) {
                queue.extend(entries.flatten().map(|entry| entry.path()).filter(|path| {
                    path.is_file() && path.extension().is_some_and(|ext| ext == "dart")
                }));
            }
            while let Some(dir) = queue.pop() {
                let Ok(entries) = std::fs::read_dir(&dir) else {
                    continue;
                };
                for entry in entries.flatten() {
                    let Ok(file_type) = entry.file_type() else {
                        continue;
                    };
                    let path = entry.path();
                    let name = entry.file_name();
                    let name = name.to_string_lossy();
                    if file_type.is_dir() {
                        if name.starts_with('.') || EXCLUDED.contains(&name.as_ref()) {
                            continue;
                        }
                        queue.push(path);
                    } else if path.extension().is_some_and(|ext| ext == "dart") {
                        let bytes = std::fs::read(&path)?;
                        stats.loc_total += u64::from(
                            u32::try_from(bytes.iter().filter(|&&b| b == b'\n').count())
                                .unwrap_or(u32::MAX),
                        );
                        stats.file_count += 1;
                        self.set_content(&path, &bytes);
                    }
                }
            }
        }
        Ok(stats)
    }

    /// The engine's file table (the CLI's `FileLayoutProvider` backing).
    #[must_use]
    pub fn file_table(&self) -> &FileTable {
        &self.state.files
    }

    /// The line index of a resident file.
    #[must_use]
    pub fn line_index(&self, file: FileId) -> Option<&LineIndex> {
        self.state
            .residency
            .get(&file)
            .map(|facts| &facts.line_index)
    }

    /// The resident bytes of a file.
    #[must_use]
    pub fn content(&self, file: FileId) -> Option<&[u8]> {
        let facts = self.state.residency.get(&file)?;
        self.state.store.get(facts.content)
    }

    /// The library owning `file` (defining or part), if resident.
    #[must_use]
    pub fn library_of(&self, file: FileId) -> Option<LibraryId> {
        self.state.library_of_file.get(&file).copied()
    }

    /// The defining file of a live library. Singleton convention
    /// (incremental-oracle-cases §4): a singleton library's sole member is its
    /// defining file — the face the incremental oracle's `invalidated` →
    /// defining-path projection reads through.
    #[must_use]
    pub fn library_defining_file(&self, library: LibraryId) -> Option<FileId> {
        self.state.libraries.info(library).map(|info| info.defining)
    }

    /// Live libraries with their member files (defining first, then parts
    /// in directive order).
    #[must_use]
    pub fn libraries(&self) -> Vec<(LibraryId, Vec<FileId>)> {
        self.state
            .libraries
            .iter_infos()
            .map(|(id, info)| (id, pipeline::member_files(info)))
            .collect()
    }

    /// Library-level import dependency graph, CSR form (GPU posture: flat
    /// `&[u32]` offsets/edges, zero-copy exportable).
    #[must_use]
    pub fn library_import_graph(&self) -> CsrGraph {
        pipeline::library_import_csr(&self.state)
    }

    /// File-level directive reference graph, CSR form (same posture).
    #[must_use]
    pub fn file_reference_graph(&self) -> CsrGraph {
        pipeline::file_reference_csr(&self.state)
    }
}

/// What one [`Engine::apply_pending`] changed (§1.2).
#[derive(Debug, Default)]
pub struct AffectedReport {
    /// Files whose ContentId differed from residency (gate passed, work done).
    pub changed: Vec<FileId>,
    /// Files removed by this apply.
    pub removed: Vec<FileId>,
    /// Post-closure dirty library set (what analyze must recompute).
    pub invalidated: Vec<LibraryId>,
}

/// Discovery counters (§3 metrics hook, PRD §17.2).
#[derive(Debug, Default)]
pub struct DiscoveryStats {
    /// Discovered `.dart` files.
    pub file_count: usize,
    /// Total lines across the discovered set.
    pub loc_total: u64,
}

/// Flat CSR arrays: `offsets[i]..offsets[i+1]` slice `edges` for node `i`.
/// Both views are plain `&[u32]` — the zero-copy export face for external
/// compute backends (GPU posture ADR, engine-api-spec §3 rider).
#[derive(Debug, Clone, Default)]
pub struct CsrGraph {
    offsets: Vec<u32>,
    edges: Vec<u32>,
}

impl CsrGraph {
    /// Per-node start offsets; `nodes() + 1` entries.
    #[must_use]
    pub fn offsets(&self) -> &[u32] {
        &self.offsets
    }

    /// Packed adjacency targets.
    #[must_use]
    pub fn edges(&self) -> &[u32] {
        &self.edges
    }

    /// Number of nodes.
    #[must_use]
    pub fn nodes(&self) -> usize {
        self.offsets.len().saturating_sub(1)
    }

    /// Edge count.
    #[must_use]
    pub fn len_edges(&self) -> usize {
        self.edges.len()
    }
}

/// Read-only view of one library for the semantic diagnostics seam — the
/// context #120/#121 diagnose against (semantic crate's assumption contract
/// documents what these fields guarantee).
pub struct LibraryView<'a> {
    /// The engine library id.
    pub library: LibraryId,
    /// Member files: defining first, then parts in directive order.
    pub files: &'a [FileId],
    /// The interned library graph over the current analysis set.
    pub graph: &'a dartq_resolution::LibraryGraph,
    /// This library's id in [`LibraryView::graph`].
    pub graph_library: dartq_resolution::LibraryId,
    /// The shared string intern table (resolve `StringId`s against it).
    pub strings: &'a dartq_source::StringTable,
    /// Every resident file's facts: directives, element model, syntax
    /// diagnostics, line index. Read-only snapshot.
    pub facts: &'a FxHashMap<FileId, Arc<UnitFacts>>,
    /// The SDK snapshot (`dart:` library support queries).
    pub sdk: &'a SdkSnapshot,
}

/// The semantic diagnostics plug point (#120/#121).
///
/// Implemented above the engine (the CLI wires it onto the dartq-semantic
/// diagnostic modules — dartq-semantic cannot depend on the engine without
/// inverting the layering). Per-library calls arrive on the batch fan-out
/// workers, so implementations must be `Send + Sync` and stateless across
/// calls; diagnostics append into `out` and carry no ordering obligations (the
/// engine sorts canonically).
pub trait SemanticDiagnoser: Send + Sync {
    /// Appends this library's semantic diagnostics to `out`.
    fn diagnose_library(&self, library: LibraryView<'_>, out: &mut Vec<Diagnostic>);
}

/// The M1 default store constructor (`MemorySourceStore`, §1.1).
#[must_use]
pub fn memory_store() -> Box<dyn SourceStore + Send + Sync> {
    Box::new(MemorySourceStore::new())
}
