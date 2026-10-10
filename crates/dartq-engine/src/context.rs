//! The multi-context runtime substrate (PRD §16): configuration-forked
//! analysis contexts coexisting over one base, with per-context overlays,
//! writer-priority scheduling, and epoch-based cancellation.
//!
//! # Shape (§16.1–§16.3)
//!
//! * A [`Context`] is a configuration fork dimension — SDK identity, package
//!   graph (which decides the language-version table, §16.2), merged
//!   analysis options, and the reserved flag word — hashed into a
//!   [`ContextKey`]. Two opens with equal keys are the same context (dedup
//!   on insert), so a live context set is always pairwise-distinct in
//!   configuration.
//! * Each context owns one [`Engine`] — its independent semantic-layer
//!   products (residency, library graph, per-library results, derived
//!   facts). Fork-irrelevant content is shared at the identity level:
//!   [`ContentId`]s are content addresses valid across contexts, and
//!   identical overlay bytes occupy one slot in the runtime's own
//!   content-addressed overlay store (§16.3's path → `ContentId` map).
//!   Physical byte sharing of *engine* stores is opt-in per store handle:
//!   contexts constructed over one
//!   [`ContentPool`](crate::ContentPool)
//!   ([`pooled_memory_store`](crate::pooled_memory_store)) hold every
//!   distinct content exactly once between them, while reads keep the
//!   zero-copy `get` contract and each context's own GC path intact (the
//!   pool reclaims what no context references anymore).
//! * An overlay edit flows through the existing apply-path discipline —
//!   `set_content` enqueue → `apply_pending` barrier — so overlay bytes make
//!   a file resident without touching disk, with the same batch +
//!   invalidation semantics as any other content change.
//! * Writes bump the context's epoch; tasks enqueued under older epochs are
//!   dropped at the single drain point ([`Runtime::drain`], §16.6).

use std::fmt;
use std::path::{Path, PathBuf};

use dartq_source::{ContentId, MemorySourceStore, SourceStore};
use rustc_hash::{FxHashMap, FxHashSet};

use crate::analysis_options;
use crate::event::EngineEvent;
use crate::scheduler::{DrainReport, Scheduler, Task, Tier};
use crate::{AffectedReport, Engine, EngineConfig, EngineError};

/// A small stable integer naming one open context. Opaque, process-lifetime
/// only; ids recycle through a freelist after [`Runtime::close_context`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ContextId(u32);

impl ContextId {
    /// The underlying integer, stable while the context is open.
    #[must_use]
    pub const fn index(self) -> u32 {
        self.0
    }

    /// Mints the id for arena slot `index` (crate-internal: tests and the
    /// scheduler address slots directly).
    pub(crate) const fn from_index(index: u32) -> Self {
        Self(index)
    }
}

impl fmt::Display for ContextId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#{}", self.0)
    }
}

/// blake3-128 over the canonical context configuration tuple: the SDK
/// identity and its target segment, the conditional-import declared
/// variables, the package graph rows, the merged-options file fingerprint,
/// and the reserved flag word. Two opens with equal keys are the same
/// context.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ContextKey([u8; 16]);

impl ContextKey {
    /// The 16 identity bytes (hex-encodable, log-friendly).
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 16] {
        &self.0
    }
}

/// One context's configuration: everything [`EngineConfig`] already carries
/// (the fork dimensions of PRD §5.4's context slice — SDK identity, target
/// segment, declared variables, package graph, merged options) plus the
/// reserved flag word for fork dimensions not yet materialized (the flutter
/// panel — §16.2). Consumed by [`Runtime::open_context`].
pub struct ContextConfig {
    /// The per-context analysis configuration, passed through to
    /// [`Engine::new`] verbatim (including the context's own content store).
    /// Byte sharing across contexts is opt-in through one
    /// [`ContentPool`](crate::ContentPool): construct every context's store
    /// with [`pooled_memory_store`](crate::pooled_memory_store) over the
    /// same pool and each distinct content is stored exactly once between
    /// them; standalone stores keep full isolation.
    pub engine: EngineConfig,
    /// Reserved fork word, hashed into the key verbatim. Zero by default;
    /// callers bit-encode panel toggles here until they grow into
    /// real config fields.
    pub flags: u64,
}

/// One entry of a per-context overlay batch (§16.3): full-content
/// replacement or removal. Writes never touch the base tree on disk.
pub enum OverlayChange {
    /// `path` reads as `bytes` in this context's view from the next apply on.
    /// Exactly one leading UTF-8 BOM is stripped (the engine read-layer
    /// rule); invalid UTF-8 decodes lossily at the engine boundary as in
    /// [`crate::Engine::set_content`].
    Set { path: PathBuf, bytes: Vec<u8> },
    /// `path` is absent from this context's view. Already-absent is success
    /// (batch semantics — the overlay declares the view, it does not edit
    /// history).
    Remove { path: PathBuf },
}

/// One open context: its engine (the independent semantic-layer products),
/// its identity key, its cancellation epoch, and its standing overlay
/// (§16.3's path → `ContentId` map — re-appliable if the view is rebuilt).
struct Context {
    engine: Engine,
    key: ContextKey,
    /// Monotonic write epoch (§16.6): bumped by every [`Runtime::apply_overlay`]
    /// and [`Runtime::cancel`]; tasks carrying older epochs are stale.
    epoch: u64,
    /// Standing overlay: paths this context reads as overlay bytes. Bounded
    /// by the workspace file set; entries leave on removal or close.
    overlay: FxHashMap<PathBuf, ContentId>,
}

/// The multi-context runtime (§16.1): one process, one base, N
/// configuration-forked contexts, a shared content-addressed overlay store,
/// and the per-context scheduler queues.
///
/// Single-owner like [`Engine`] (`&mut self` makes every state transition
/// total); the daemon decides threading above this face.
///
/// Bounded by construction: contexts are explicit open/close (ids recycle),
/// overlays are bounded by the workspace file set, scheduler queues drain or
/// clear, and [`Runtime::sweep`] reclaims dead overlay content.
#[derive(Default)]
pub struct Runtime {
    /// The shared overlay content layer (§16.3): bytes addressable by
    /// [`ContentId`], deduplicated across every context's overlay. Runtime-
    /// owned and runtime-exclusive — engines keep their own per-context
    /// stores (see the module docs for the sharing adjudication).
    shared: MemorySourceStore,
    /// Dense context arena; `None` = closed slot awaiting id reuse.
    contexts: Vec<Option<Context>>,
    /// Closed ids available for reuse (bounded by the historical open peak).
    free: Vec<ContextId>,
    /// Live-context identity index. Key removal on close means dedup is
    /// among live contexts only — reopening a closed configuration is a new
    /// context.
    keys: FxHashMap<ContextKey, ContextId>,
    /// Per-context task queues (PRD §16.5), drained writer-first.
    scheduler: Scheduler,
}

impl Runtime {
    /// An empty runtime: no contexts, no overlay content.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Opens (or dedups to) the context described by `config` (§16.2).
    ///
    /// Returns the existing id when a live context's key equals the computed
    /// one — same configuration is the same context, and the incoming
    /// config (SDK snapshot, store) is dropped. Otherwise constructs the
    /// context's engine.
    ///
    /// # Errors
    ///
    /// [`EngineError::PackageConfig`] when the config cannot back the
    /// URI-resolution table.
    ///
    /// # Panics
    ///
    /// Panics once more than [`u32::MAX`] distinct contexts have ever been
    /// opened (the id space is exhausted) — unreachable at any real scale,
    /// the same posture as [`crate::LibraryId`].
    pub fn open_context(&mut self, config: ContextConfig) -> Result<ContextId, EngineError> {
        let key = context_key(&config);
        if let Some(&id) = self.keys.get(&key) {
            return Ok(id);
        }
        let engine = Engine::new(config.engine)?;
        let id = match self.free.pop() {
            Some(id) => id,
            None => ContextId::from_index(
                u32::try_from(self.contexts.len()).expect("more than u32::MAX contexts opened"),
            ),
        };
        let index = id.index() as usize;
        if index >= self.contexts.len() {
            self.contexts.push(None);
        }
        self.contexts[index] = Some(Context {
            engine,
            key,
            epoch: 0,
            overlay: FxHashMap::default(),
        });
        self.keys.insert(key, id);
        Ok(id)
    }

    /// Closes a context: drops its engine, its standing overlay, and its
    /// scheduler queue; recycles the id. Its overlay content becomes dead
    /// and is reclaimed by the next [`Runtime::sweep`].
    ///
    /// # Errors
    ///
    /// [`EngineError::UnknownContext`] when `id` is not open.
    pub fn close_context(&mut self, id: ContextId) -> Result<(), EngineError> {
        let context = self
            .contexts
            .get_mut(id.index() as usize)
            .and_then(Option::take)
            .ok_or(EngineError::UnknownContext(id))?;
        self.keys.remove(&context.key);
        self.scheduler.drop_context(id);
        self.free.push(id);
        Ok(())
    }

    /// The closed context's engine view: analyze, queries, content reads —
    /// the per-context analysis face.
    #[must_use]
    pub fn context_state(&self, id: ContextId) -> Option<&Engine> {
        Some(&self.slot(id)?.engine)
    }

    /// Mutable variant of [`Runtime::context_state`] (discovery, content
    /// updates through the engine's own faces).
    pub fn context_state_mut(&mut self, id: ContextId) -> Option<&mut Engine> {
        Some(&mut self.slot_mut(id)?.engine)
    }

    /// Applies an overlay batch to one context's view (§16.3): each entry
    /// flows through the engine's enqueue → apply-path discipline, so the
    /// view updates as one barrier with normal invalidation — and the base
    /// tree on disk is never touched. The context's epoch bumps afterwards:
    /// tasks enqueued before this write are stale and drop at the next
    /// drain (§16.6).
    ///
    /// # Errors
    ///
    /// [`EngineError::UnknownContext`] when `id` is not open.
    pub fn apply_overlay(
        &mut self,
        id: ContextId,
        changes: &[OverlayChange],
        on_event: &mut dyn FnMut(EngineEvent),
    ) -> Result<AffectedReport, EngineError> {
        // Direct field access (not `slot_mut`): the store writes and the
        // context borrow must stay disjoint field borrows.
        let context = self
            .contexts
            .get_mut(id.index() as usize)
            .and_then(Option::as_mut)
            .ok_or(EngineError::UnknownContext(id))?;
        for change in changes {
            match change {
                OverlayChange::Set { path, bytes } => {
                    let text = crate::strip_one_bom(bytes);
                    let cid = self.shared.put(text);
                    context.overlay.insert(path.clone(), cid);
                    context.engine.set_content(path, text);
                }
                OverlayChange::Remove { path } => {
                    context.overlay.remove(path);
                    // "Ensure absent": a path the engine never interned is
                    // already absent — the only `remove_file` error is
                    // `UnknownPath`, so ignoring it is the semantics, not a
                    // swallowed failure.
                    let _ = context.engine.remove_file(path);
                }
            }
        }
        let outcome = context.engine.apply_pending(on_event);
        context.epoch += 1;
        Ok(outcome)
    }

    /// Cancels the context's in-flight work without a write: bumps the
    /// epoch so every task enqueued so far goes stale, returning the new
    /// epoch (§16.6 — partial results of stale tasks never surface).
    ///
    /// # Errors
    ///
    /// [`EngineError::UnknownContext`] when `id` is not open.
    pub fn cancel(&mut self, id: ContextId) -> Result<u64, EngineError> {
        let context = self.slot_mut(id).ok_or(EngineError::UnknownContext(id))?;
        context.epoch += 1;
        Ok(context.epoch)
    }
    /// The context's current epoch.
    #[must_use]
    pub fn epoch(&self, id: ContextId) -> Option<u64> {
        Some(self.slot(id)?.epoch)
    }

    /// Enqueues one task against `id`'s queue (PRD §16.5), stamped with the
    /// context's current epoch. `tier` is the task's precedence within its
    /// class (see [`Tier`]); the caller owns the mapping — the daemon wires
    /// it where each intake path enters the scheduler. Returns the stamped
    /// task.
    ///
    /// # Errors
    ///
    /// [`EngineError::UnknownContext`] when `id` is not open.
    pub fn enqueue(
        &mut self,
        id: ContextId,
        write: bool,
        tier: Tier,
        cookie: u64,
    ) -> Result<Task, EngineError> {
        // Direct field access keeps the context borrow disjoint from the
        // scheduler queue write below.
        let context = self
            .contexts
            .get_mut(id.index() as usize)
            .and_then(Option::as_mut)
            .ok_or(EngineError::UnknownContext(id))?;
        let task = Task {
            context: id,
            epoch: context.epoch,
            write,
            tier,
            cookie,
        };
        self.scheduler.enqueue(task);
        Ok(task)
    }

    /// The single drain point (§16.5, §16.6): pops every queued task,
    /// drops stale-epoch ones, and returns the live set in writer-priority
    /// order, tier-ordered within each class — every pending write
    /// (interactive first, then normal, background; context ascending,
    /// arrival order within a tier), then every read (same order).
    ///
    /// # Panics
    ///
    /// Panics on the structural invariant that every queued task references
    /// an open context — maintained by [`Runtime::enqueue`] checking and
    /// [`Runtime::close_context`] clearing; a miss is a program bug.
    #[must_use]
    pub fn drain(&mut self) -> DrainReport {
        // Field capture, not `self.slot`: a method-call closure would borrow
        // all of `self` and fight the scheduler's mutable borrow.
        let contexts = &self.contexts;
        let epochs = |id: ContextId| {
            contexts
                .get(id.index() as usize)
                .and_then(Option::as_ref)
                .expect("queued tasks reference live contexts: enqueue checks and close clears")
                .epoch
        };
        self.scheduler.drain(&epochs)
    }

    /// Reclaims the shared overlay store's dead content: every id no
    /// standing overlay references is swept (content addressing makes a
    /// later identical write return byte-identical under the same name).
    /// Returns how many entries were removed.
    pub fn sweep(&mut self) -> usize {
        let live: FxHashSet<ContentId> = self
            .contexts
            .iter()
            .flatten()
            .flat_map(|context| context.overlay.values().copied())
            .collect();
        self.shared.collect_garbage(&|id| live.contains(&id))
    }

    /// Distinct contents the shared overlay store holds.
    #[must_use]
    pub fn stored_contents(&self) -> usize {
        self.shared.len()
    }

    /// Number of open contexts.
    #[must_use]
    pub fn len_contexts(&self) -> usize {
        self.contexts.iter().flatten().count()
    }

    fn slot(&self, id: ContextId) -> Option<&Context> {
        self.contexts
            .get(id.index() as usize)
            .and_then(Option::as_ref)
    }

    fn slot_mut(&mut self, id: ContextId) -> Option<&mut Context> {
        self.contexts
            .get_mut(id.index() as usize)
            .and_then(Option::as_mut)
    }
}

/// Length-prefixed field fold: prefixing every field with its length keeps
/// the byte concatenation canonical (no `(ab, c)` / `(a, bc)` aliasing).
fn fold_field(hasher: &mut blake3::Hasher, bytes: &[u8]) {
    hasher.update(&(u64::try_from(bytes.len()).expect("field exceeds u64")).to_le_bytes());
    hasher.update(bytes);
}

/// blake3-128 over the canonical configuration tuple (§16.2): SDK identity,
/// the SDK's language metadata and target segment, the conditional-import
/// declared variables, the config directory, every package-graph row (name,
/// root, language version — the version table §16.2 derives from the graph),
/// the merged-options fingerprint per root, and the flag word. Rows, roots,
/// and declared-variable names sort before folding so table layout cannot
/// move the key. The target and variable columns mirror the context salt's
/// dimensions (state/persist.rs `context_salt`) with the same encoding
/// discipline — sorted keys, length-prefixed fields — so a context fork
/// that moves the salt moves the key too.
fn context_key(config: &ContextConfig) -> ContextKey {
    let engine = &config.engine;
    let mut hasher = blake3::Hasher::new();
    fold_field(&mut hasher, engine.sdk.id.to_string().as_bytes());
    fold_field(
        &mut hasher,
        engine.sdk.language_metadata.to_string().as_bytes(),
    );
    fold_field(&mut hasher, engine.target.as_bytes());
    let mut variables: Vec<(&str, &str)> = engine
        .declared_variables
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect();
    variables.sort_unstable();
    for (name, value) in &variables {
        fold_field(&mut hasher, name.as_bytes());
        fold_field(&mut hasher, value.as_bytes());
    }
    fold_field(&mut hasher, engine.config_dir.to_string_lossy().as_bytes());

    let mut packages: Vec<(String, String, String)> = engine
        .packages
        .iter()
        .map(|package| {
            (
                package.name.as_str().to_owned(),
                package.root.to_string_lossy().into_owned(),
                package
                    .language_version
                    .map(|version| version.to_string())
                    .unwrap_or_default(),
            )
        })
        .collect();
    packages.sort_unstable();
    packages.dedup();
    for (name, root, version) in &packages {
        fold_field(&mut hasher, name.as_bytes());
        fold_field(&mut hasher, root.as_bytes());
        fold_field(&mut hasher, version.as_bytes());
    }

    // Options fingerprint: per root, the located `analysis_options.yaml`'s
    // path and raw bytes — the file content the merged options derive from.
    // (The include chain is folded through the declaring file only; include-
    // only edits are a known fingerprint edge, tracked for M3 follow-up.)
    let mut roots: Vec<&Path> = engine
        .packages
        .iter()
        .map(|package| package.root.as_path())
        .collect();
    roots.sort_unstable();
    roots.dedup();
    for root in roots {
        fold_field(&mut hasher, root.to_string_lossy().as_bytes());
        match analysis_options::AnalysisOptions::find_for_dir(root) {
            Some(file) => {
                hasher.update(&[1]);
                fold_field(&mut hasher, file.to_string_lossy().as_bytes());
                // An unreadable file degrades to empty contributions in the
                // engine's own loader, so an empty fold matches the analyzed
                // semantics.
                let bytes = std::fs::read(file).unwrap_or_default();
                fold_field(&mut hasher, &bytes);
            }
            None => {
                hasher.update(&[0]);
            }
        }
    }

    fold_field(&mut hasher, &config.flags.to_le_bytes());
    let digest = hasher.finalize();
    let mut key = [0u8; 16];
    key.copy_from_slice(&digest.as_bytes()[..16]);
    ContextKey(key)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ContentPool, EngineEvent, pooled_memory_store};
    use dartq_package::PackageTable;
    use dartq_pub::{LockFile, PackageConfig, Pubspec, build_graph};
    use dartq_sdk::SdkSnapshot;
    use std::collections::BTreeMap;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// A minimal one-package workspace fixture (root package `t` + a
    /// synthetic SDK) — the context key and open/dedup test bed. The SDK
    /// must be loaded fresh per config ([`SdkSnapshot`] is not `Clone`).
    fn fixture(tag: &str) -> (PathBuf, Arc<PackageConfig>, PackageTable) {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let root = std::env::temp_dir().join(format!(
            "dartq-engine-context-{tag}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&root);
        for dir in ["lib", ".dart_tool", "sdk/lib/_internal"] {
            std::fs::create_dir_all(root.join(dir)).expect("fixture dirs");
        }
        std::fs::write(
            root.join("pubspec.yaml"),
            "name: t\nversion: 0.1.0\nenvironment:\n  sdk: ^3.5.0\n",
        )
        .expect("pubspec");
        std::fs::write(
            root.join("pubspec.lock"),
            "packages: {}\nsdks:\n  dart: \">=3.5.0 <4.0.0\"\n",
        )
        .expect("lock");
        let config_text = r#"{"configVersion": 2, "packages": [{"name": "t", "rootUri": "../", "languageVersion": "3.5"}], "generator": "pub"}"#;
        std::fs::write(root.join(".dart_tool/package_config.json"), config_text).expect("config");
        std::fs::write(root.join("sdk/version"), "3.13.1\n").expect("version");
        std::fs::write(
            root.join("sdk/lib/libraries.json"),
            r#"{"vm": {"libraries": {"core": {"uri": "core/core.dart"}}}}"#,
        )
        .expect("libraries");
        std::fs::write(
            root.join("sdk/lib/_internal/allowed_experiments.json"),
            r#"{"version": 1, "experimentSets": {"sdkExperiments": []}, "sdk": {"default": {"experimentSet": "sdkExperiments"}}, "packages": {}}"#,
        )
        .expect("experiments");
        // Canonicalize before the pub faces resolve it (the #287 spelling
        // universe — macOS /var vs /private/var).
        let root = root.canonicalize().expect("fixture root resolves");
        let config_dir = root.join(".dart_tool");
        let config = Arc::new(PackageConfig::parse(config_text).expect("valid config"));
        let lock = LockFile::parse("packages: {}\nsdks:\n  dart: \">=3.5.0 <4.0.0\"\n")
            .expect("valid lock");
        let pubspec = Pubspec::parse("name: t\nversion: 0.1.0\nenvironment:\n  sdk: ^3.5.0\n")
            .expect("pubspec");
        let graph = build_graph(
            &config,
            &lock,
            &pubspec,
            &BTreeMap::new(),
            None,
            &config_dir,
        )
        .expect("graph");
        let packages = PackageTable::build(&config, &graph, &config_dir).expect("table");
        (root, config, packages)
    }

    fn sdk(root: &Path) -> SdkSnapshot {
        SdkSnapshot::load(&root.join("sdk"), None).expect("synthetic SDK loads")
    }

    /// An engine config over `store`, everything else at the fixture
    /// defaults (the `vm` target, no declared variables).
    fn engine_config(
        root: &Path,
        config: &Arc<PackageConfig>,
        packages: &PackageTable,
        store: Box<dyn SourceStore + Send + Sync>,
    ) -> EngineConfig {
        EngineConfig {
            store,
            sdk: sdk(root),
            packages: packages.clone(),
            pub_config: Arc::clone(config),
            config_dir: root.join(".dart_tool"),
            semdiag: None,
            target: crate::DEFAULT_TARGET.to_owned(),
            declared_variables: FxHashMap::default(),
        }
    }

    /// The two new fork columns move the key (#483): equal configurations
    /// digest equally, and a different target segment or declared-variable
    /// set forks it — the salt's dimension set with its canonicalization.
    #[test]
    fn context_key_moves_with_target_and_declared_variables() {
        let (root, config, packages) = fixture("key");
        let base = context_key(&ContextConfig {
            engine: engine_config(&root, &config, &packages, crate::memory_store()),
            flags: 0,
        });
        let again = context_key(&ContextConfig {
            engine: engine_config(&root, &config, &packages, crate::memory_store()),
            flags: 0,
        });
        assert_eq!(base, again, "equal configurations digest equally");

        let mut forked = ContextConfig {
            engine: engine_config(&root, &config, &packages, crate::memory_store()),
            flags: 0,
        };
        forked.engine.target = "js".to_owned();
        assert_ne!(
            base,
            context_key(&forked),
            "the target segment forks the key"
        );

        let mut defined = ContextConfig {
            engine: engine_config(&root, &config, &packages, crate::memory_store()),
            flags: 0,
        };
        defined
            .engine
            .declared_variables
            .insert("x".to_owned(), "1".to_owned());
        defined
            .engine
            .declared_variables
            .insert("y".to_owned(), "2".to_owned());
        assert_ne!(
            base,
            context_key(&defined),
            "a declared variable forks the key"
        );
        // Canonicalization: insertion order into the variable map cannot
        // move the digest.
        let mut reordered = ContextConfig {
            engine: engine_config(&root, &config, &packages, crate::memory_store()),
            flags: 0,
        };
        reordered
            .engine
            .declared_variables
            .insert("y".to_owned(), "2".to_owned());
        reordered
            .engine
            .declared_variables
            .insert("x".to_owned(), "1".to_owned());
        assert_eq!(
            context_key(&defined),
            context_key(&reordered),
            "variable order is canonicalized away"
        );
    }

    /// Same configuration dedups to one context; a forked target segment
    /// opens a distinct context (the per-context SDK panel selection).
    #[test]
    fn open_context_dedups_equal_configs_and_forks_on_target() {
        let (root, config, packages) = fixture("runtime");
        let mut runtime = Runtime::new();
        let open = |runtime: &mut Runtime, target: &str| {
            let mut engine = engine_config(&root, &config, &packages, crate::memory_store());
            engine.target = target.to_owned();
            runtime.open_context(ContextConfig { engine, flags: 0 })
        };
        let base = open(&mut runtime, "vm").expect("open");
        assert_eq!(
            open(&mut runtime, "vm").expect("open"),
            base,
            "equal configurations are the same context"
        );
        let js = open(&mut runtime, "js").expect("open");
        assert_ne!(js, base, "the target segment forks the context");
        assert_eq!(runtime.len_contexts(), 2);
    }

    /// Contexts constructed over one pool hold shared content exactly once:
    /// both engines put the same bytes, the pool's footprint stays one
    /// entry, and each engine's own view reads it back (the multi-context
    /// store face).
    #[test]
    fn pooled_contexts_store_shared_content_once() {
        let (root, config, packages) = fixture("pool");
        let pool = ContentPool::new();
        let mut a = Engine::new(engine_config(
            &root,
            &config,
            &packages,
            pooled_memory_store(&pool),
        ))
        .expect("engine");
        let mut b = Engine::new(engine_config(
            &root,
            &config,
            &packages,
            pooled_memory_store(&pool),
        ))
        .expect("engine");
        let path = root.join("lib/main.dart");
        let source = b"void main() {}\n";
        let main_a = a.set_content(&path, source);
        let main_b = b.set_content(&path, source);
        let mut on_event = |_event: EngineEvent| {};
        a.apply_pending(&mut on_event);
        b.apply_pending(&mut on_event);

        assert_eq!(pool.len(), 1, "one distinct content across both stores");
        assert_eq!(
            pool.len_bytes(),
            u64::try_from(source.len()).expect("fixture fits u64"),
            "counted once, not once per context"
        );
        assert_eq!(a.stored_contents(), 1);
        assert_eq!(b.stored_contents(), 1);
        assert_eq!(a.content(main_a), Some(&source[..]));
        assert_eq!(b.content(main_b), Some(&source[..]));
    }
}
