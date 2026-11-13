//! Facts persistence: the plain-data projection
//! a [`dartq_snapshot`] entry carries, the package-graph fingerprint and
//! the SDK identity encoding the generation header keys on.
//!
//! # Why a mirror instead of `impl Serialize for UnitFacts`
//!
//! Arena ids are process-local by contract: [`StringId`]/[`FileId`] have no
//! numeric constructors (their integers mean nothing outside the issuing
//! tables), so a faithful payload must move the *texts* and re-intern them
//! at load — [`PersistedFacts::materialize`] re-interns the string slice in
//! order (reproducing the indexes exactly) and mints file ids through the
//! caller's intern face. `LineIndex` is deliberately absent: it is a pure
//! function of the source text, the entry key pins that text's content
//! hash, and [`PersistedFacts::materialize`] rebuilds it from the source
//! the loader holds anyway — zero payload bytes for a derivable value
//! (memory charter: no bytes beyond what the types cannot re-derive).
//! `UnitFacts::content` is likewise not carried: the entry key's
//! `content_hash` already pins it, and materializing from the same source
//! reproduces the id by construction.
//!
//! Save shape: [`PersistedFacts::of`] projects resident facts, the caller
//! wraps them in `dartq_snapshot::Entry` payloads keyed by
//! `FACTS_SEMANTIC_VERSION` (state.rs); the header carries
//! [`sdk_identity`], [`package_graph_fingerprint`], and the context salt
//! ([`context_salt`]). Load shape (landed, #482): `Engine::
//! ingest_facts_snapshot` stages a `Snapshot<PersistedFacts>` and the next
//! apply materializes identity-matched entries through
//! [`PersistedFacts::materialize`].

use std::path::{Path, PathBuf};
use std::sync::Arc;

use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};

use dartq_diagnostics::{
    CodeInfo, CodeRegistry, ContextMessage, Diagnostic, DiagnosticType, Severity,
};
use dartq_package::{PackageIdentity, PackageTable, SourceCategory};
use dartq_resolution::{
    CombinatorClause, Combinators, Configuration, ResolutionCode, ResolvedTarget,
};
use dartq_sdk::SdkSnapshot;
use dartq_index::{LibraryIndex, NameRow, RefKind, RefRow, SubtypeEdge, TargetRef};
use dartq_semantic::ApiSignature;
use dartq_semantic::persisted::{PersistedElements, PersistedRange};
use dartq_source::{ContentId, FileId, FileTable, LineIndex, SourceRange, StringId, StringTable};

use crate::state::{DirectiveFacts, DirectiveKind, DocImportFacts, LibraryResult, UnitFacts};

/// blake3 truncated to 128 bits — the house digest width.
fn blake3_128(bytes: &[u8]) -> [u8; 16] {
    let digest = blake3::hash(bytes);
    let mut id = [0u8; 16];
    id.copy_from_slice(&digest.as_bytes()[..16]);
    id
}

/// The package graph fingerprint: blake3-128 over every package
/// identity's resolution-relevant columns — name, root, lib dir, language
/// version, source category — in ascending-name order, each field
/// length-prefixed so concatenation is unambiguous. Canonical order makes
/// the digest a pure function of the graph, equal tables → equal digests
/// across processes. Any column change (a repoint, a version bump, a new
/// dependency) forks the generation header — the snapshot misses and the
/// workspace reanalyzes, which is exactly the semantics a foreign package
/// graph demands.
#[must_use]
pub fn package_graph_fingerprint(packages: &PackageTable) -> [u8; 16] {
    let mut identities: Vec<&PackageIdentity> = packages.iter().collect();
    identities.sort_by(|a, b| a.name.cmp(&b.name));
    let mut bytes = Vec::new();
    for package in identities {
        field(&mut bytes, package.name.to_string().as_bytes());
        field(&mut bytes, package.root.as_os_str().as_encoded_bytes());
        field(&mut bytes, package.lib_dir.as_os_str().as_encoded_bytes());
        let version = package
            .language_version
            .as_ref()
            .map_or_else(|| "-".to_string(), ToString::to_string);
        field(&mut bytes, version.as_bytes());
        let category = match package.category {
            SourceCategory::Path => "path",
            SourceCategory::PubCache => "pub-cache",
            SourceCategory::Sdk => "sdk",
        };
        field(&mut bytes, category.as_bytes());
    }
    blake3_128(&bytes)
}

/// Appends one length-prefixed field (u32 LE length, then the bytes).
#[expect(clippy::cast_possible_truncation)] // span arithmetic narrows under the scanner u32 offset cap
fn field(bytes: &mut Vec<u8>, value: &[u8]) {
    bytes.extend_from_slice(&(value.len() as u32).to_le_bytes());
    bytes.extend_from_slice(value);
}

/// The canonical SDK identity string the generation header carries: the
/// dart version, the revision (`-` when a source checkout has none), the
/// raw `libraries.json` content hash, and the resolved target segment —
/// fixed field order, space-separated, one-way. Every component changes
/// what `dart:` resolution and the feature set resolve to, so the header
/// equality gate (any mismatch is a whole-generation miss) keys on exactly
/// these. `target` is the context's own segment (`EngineState::target`):
/// two contexts resolving against different panels never share a header.
#[must_use]
pub fn sdk_identity(sdk: &SdkSnapshot, target: &str) -> String {
    format!(
        "dart-sdk {} revision {} libraries {} target {}",
        sdk.dart.version,
        sdk.dart.revision.as_deref().unwrap_or("-"),
        sdk.dart.libraries_json_hash,
        target,
    )
}

/// The running analyzer build's identity: blake3-128 over the current
/// executable's bytes, read and hashed once per process (the bytes cannot
/// change under a running process). The persisted facts are baked analyzer
/// OUTPUT — scan rows, parse-recovery rows, directive targets and codes,
/// element captures — so the build that produced them is as much a fork
/// dimension of a facts generation as the SDK or the package graph: a
/// rebuilt or upgraded binary whose `FACTS_SEMANTIC_VERSION` did not move
/// (the bump discipline is manual, and the history above records its
/// misses — v15, v20, v24) must never serve the previous build's rows for
/// unchanged content (#3113: a warm generation survived the engine
/// rebuild, and the same root replayed stale rows while a fresh root — a
/// different context salt — answered fresh). An unreadable executable
/// (an exotic embedder) folds a constant failure marker instead: the
/// identity degrades to one shared build, never to a session failure —
/// the accelerator posture.
#[must_use]
pub fn analyzer_build() -> [u8; 16] {
    static BUILD: std::sync::OnceLock<[u8; 16]> = std::sync::OnceLock::new();
    *BUILD.get_or_init(|| {
        let bytes = std::env::current_exe().and_then(|exe| std::fs::read(&exe));
        match bytes {
            Ok(bytes) => blake3_128(&bytes),
            Err(_) => blake3_128(b"dartq-analyzer-build-unreadable"),
        }
    })
}

/// The context-fork salt the generation header carries (#482): blake3-128
/// over the fork dimensions [`sdk_identity`] and
/// [`package_graph_fingerprint`] do not already pin — the analyzer build
/// ([`analyzer_build`], #3113), the context root (the `.dart_tool`
/// directory the engine was assembled over), the SDK target segment, the
/// conditional-import declared-variables map, and the analysis-options
/// content. The root is what keeps distinct workspaces apart when the
/// package graph cannot (#2274): a pubspec-less tree's package table is
/// empty, so without it the fingerprint degenerates to a constant and two
/// byte-identical bare trees would share one generation directory. The
/// build keeps analyzer generations apart when nothing else can: same
/// workspace, same SDK, same content — a different binary may analyze
/// differently, so its facts are a different generation. Sorted keys and
/// length-prefixed fields make the digest a pure function of the
/// dimension values (equal contexts digest equally across processes); any
/// difference forks the header, so two contexts that would analyze
/// differently can never collide on one generation directory. Options
/// fold the *located options file's* path and raw bytes per root — the
/// same content the merged options derive from, the `context_key`
/// precedent — so exclude-glob and lint edits both move it, and an
/// unreadable file contributes exactly what the engine's own loader
/// analyzed (empty bytes).
// The engine's declared-variables map is the face (its FxHasher is part
// of the engine's identity, not a generalization axis).
#[expect(clippy::implicit_hasher)] // FxHashMap is part of the digest's identity, not a generalization axis
#[must_use]
pub fn context_salt(
    build: [u8; 16],
    context_root: &Path,
    target: &str,
    declared_variables: &FxHashMap<String, String>,
    roots: &[PathBuf],
) -> [u8; 16] {
    let mut bytes = Vec::new();
    field(&mut bytes, &build);
    field(&mut bytes, context_root.as_os_str().as_encoded_bytes());
    field(&mut bytes, target.as_bytes());
    let mut variables: Vec<(&str, &str)> = declared_variables
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect();
    variables.sort_unstable();
    for (name, value) in variables {
        field(&mut bytes, name.as_bytes());
        field(&mut bytes, value.as_bytes());
    }
    let mut roots: Vec<&Path> = roots.iter().map(PathBuf::as_path).collect();
    roots.sort_unstable();
    roots.dedup();
    for root in roots {
        field(&mut bytes, root.as_os_str().as_encoded_bytes());
        match crate::analysis_options::AnalysisOptions::find_for_dir(root) {
            Some(file) => {
                field(&mut bytes, &[1]);
                field(&mut bytes, file.as_os_str().as_encoded_bytes());
                let options = std::fs::read(&file).unwrap_or_default();
                field(&mut bytes, &options);
            }
            None => field(&mut bytes, &[0]),
        }
    }
    blake3_128(&bytes)
}

/// The snapshot payload of one file's [`UnitFacts`] (#341): the compact
/// fact columns, names on per-entry string slices — no token tables, no
/// source bytes (the module docs enumerate the deliberate omissions).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersistedFacts {
    /// The baked API signature digest — byte-true through the round trip,
    /// the #344 seam's L2-gate comparison input.
    pub api_signature: [u8; 16],
    /// Directives in source order.
    pub directives: Vec<PersistedDirective>,
    /// The `@docImport`s of the file's validating doc comment
    /// ([`DocImportFacts`]), extraction order.
    pub doc_imports: Vec<PersistedDocImport>,
    /// The semantic element model.
    pub elements: PersistedElements,
    /// Syntax + URI-layer diagnostics.
    pub diagnostics: Vec<PersistedDiagnostic>,
    /// The interned file paths every file reference indexes (directive
    /// targets, diagnostic files).
    pub files: Vec<PathBuf>,
}

impl PersistedFacts {
    /// Projects resident facts onto the payload form. `strings` is the
    /// engine's intern table (names resolve against it); `files` the
    /// engine's file table (every `FileId` the facts carry was interned
    /// there during resolution — the engine invariant this projection
    /// leans on).
    #[must_use]
    pub fn of(facts: &UnitFacts, strings: &StringTable, files: &FileTable) -> Self {
        let mut files = Files::new(files);
        let directives = facts
            .directives
            .iter()
            .map(|directive| PersistedDirective::of(directive, strings, &mut files))
            .collect();
        let doc_imports = facts
            .doc_imports
            .iter()
            .map(|import| PersistedDocImport {
                uri: import.uri.as_ref().map(std::string::ToString::to_string),
                uri_span: PersistedRange::of(import.uri_span),
                code: import.code.map(PersistedResolutionCode::of),
            })
            .collect();
        let diagnostics = facts
            .diagnostics
            .iter()
            .filter_map(|diagnostic| PersistedDiagnostic::of(diagnostic, &mut files))
            .collect();
        Self {
            api_signature: *facts.api_signature.as_bytes(),
            directives,
            doc_imports,
            elements: PersistedElements::of(&facts.elements, strings),
            diagnostics,
            files: files.paths,
        }
    }

    /// Restores the facts: re-interns the string slices into `strings`,
    /// mints file ids through `intern_file` (the load wiring drives its own
    /// file table), and rebuilds the line index from `source` — the source
    /// bytes whose content hash keys this entry, which the loader holds by
    /// construction. `content` is that hash, computed once upstream (the
    /// apply batch's own content id — the entry key pins the same value, so
    /// the snapshot path never re-hashes the source). `codes` is the
    /// builtin registry, built **once per load** by the caller: `materialize`
    /// runs per entry, and a per-entry `CodeRegistry::builtin` would
    /// rebuild the ~270-entry map for every snapshot file (the l1.rs #269
    /// F2 anti-pattern).
    #[must_use]
    pub fn materialize(
        self,
        source: &str,
        content: ContentId,
        strings: &StringTable,
        codes: &CodeRegistry,
        intern_file: &mut dyn FnMut(&Path) -> FileId,
    ) -> UnitFacts {
        let files: Vec<FileId> = self.files.iter().map(|path| intern_file(path)).collect();
        let directives = self
            .directives
            .into_iter()
            .map(|directive| directive.restore(strings, &files))
            .collect();
        let doc_imports = self
            .doc_imports
            .into_iter()
            .map(|import| DocImportFacts {
                uri: import.uri.map(Box::from),
                uri_span: import.uri_span.source_range(),
                code: import.code.map(PersistedResolutionCode::resolve),
            })
            .collect();
        let diagnostics = self
            .diagnostics
            .into_iter()
            .filter_map(|diagnostic| diagnostic.restore(codes, &files))
            .map(Arc::new)
            .collect();
        UnitFacts {
            content,
            api_signature: ApiSignature::from_bytes(self.api_signature),
            directives,
            doc_imports,
            elements: Arc::new(self.elements.into_elements(strings)),
            diagnostics,
            line_index: LineIndex::new(source),
        }
    }
}

/// The persisted per-library semantic run (#4199): the cross-session half
/// of the L3 reuse gate (`g485 W2`). The in-process gate compares the
/// condensation fingerprint of a library's cycle against the fingerprint
/// recorded in its [`LibraryResult`]; this payload carries that recorded
/// pair — the diagnostic rows plus the fingerprint they were computed
/// under — into the L3 content store, keyed by
/// [`library_run_key`]. A fresh process rebuilds the condensation from
/// replayed (or recomputed) facts, derives the same key for every
/// invalidated library, and adopts the stored run when the embedded
/// fingerprint equals the current one — the exact equality the
/// in-process gate already trusts, extended across process death.
///
/// The rows mirror through [`PersistedDiagnostic`] (files as per-entry
/// path slices, re-interned at adopt). A run whose rows cannot project
/// faithfully (a `FileId` missing from the file table — an engine bug)
/// is never persisted: a payload that would under-report on reload must
/// not displace a full recompute.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersistedLibraryRun {
    /// The cycle fingerprint the run was computed under — the self-check
    /// a read performs before serving the payload (the store key folds
    /// the same value, but the embedded copy verifies the bytes actually
    /// landed under that key).
    pub fingerprint: [u8; 16],
    /// The run's rows in canonical order, as stored.
    pub diagnostics: Vec<PersistedDiagnostic>,
    /// The library's reference surface — the `LibraryIndex` projection the
    /// query faces (`query_symbols`, `resolve_at`, `override_family`,
    /// callers, supertypes) serve through `EngineState::surface`. The
    /// replay path installs it verbatim through `set_surface`, so a
    /// pure-replay turn answers exactly what the recompute it skipped
    /// would have built; a run whose surface cannot encode is never
    /// persisted (the faithful-or-nothing contract below), so a stored
    /// payload always carries its surface.
    pub surface: PersistedSurface,
    /// The interned file paths every row reference indexes — diagnostics
    /// and surface rows share the one table (`target_file` reaches past
    /// the member set to dependency declarations, so the run's file
    /// domain is the union either way).
    pub files: Vec<PathBuf>,
}

impl PersistedLibraryRun {
    /// Projects a resident run with its reference surface. `None` when
    /// any row cannot encode — the faithful-or-nothing contract above
    /// (under-reporting payloads never reach the store).
    pub(crate) fn of(
        result: &LibraryResult,
        surface: &LibraryIndex,
        files: &FileTable,
        strings: &StringTable,
    ) -> Option<Self> {
        let mut sink = Files::new(files);
        let diagnostics = result
            .diagnostics
            .iter()
            .map(|row| PersistedDiagnostic::of(row, &mut sink))
            .collect::<Option<Vec<_>>>()?;
        let surface = PersistedSurface::of(surface, &mut sink, strings)?;
        Some(Self {
            fingerprint: *result.cycle_fingerprint.as_bytes(),
            diagnostics,
            surface,
            files: sink.paths,
        })
    }

    /// Restores the run for a library whose current cycle fingerprint is
    /// `expected`: `None` when the payload does not verify (a fingerprint
    /// mismatch, undecodable rows, or rows attributing to files the
    /// caller's id face cannot resolve — a key-fold hit proves the member
    /// faces identical, so the file domain is this workspace's own; the
    /// replay caller therefore mints it on demand rather than requiring
    /// pre-membership, see `replay_library_runs`: a run's file domain
    /// reaches declaration TARGETS — SDK library files, dependency
    /// internals — the warm turn has not discovered). `intern` answers
    /// the loading process's id for a path. `codes` is the builtin
    /// registry (built once per replay pass by the caller). `strings`
    /// re-interns the surface's name texts through the loading engine's
    /// shared table — interior mutability, safe under the replay
    /// fan-out's parallelism.
    ///
    /// Returns the run and its reference surface as a pair — the replay
    /// loop installs both from the one materialized file table so their
    /// `FileId` domains agree by construction.
    pub(crate) fn materialize(
        self,
        expected: &ApiSignature,
        codes: &CodeRegistry,
        strings: &StringTable,
        intern: &mut dyn FnMut(&Path) -> Option<FileId>,
    ) -> Option<(LibraryResult, LibraryIndex)> {
        if self.fingerprint != *expected.as_bytes() {
            return None;
        }
        let files: Vec<FileId> = self
            .files
            .iter()
            .map(|path| intern(path))
            .collect::<Option<Vec<_>>>()?;
        let diagnostics = self
            .diagnostics
            .into_iter()
            .filter_map(|row| row.restore(codes, &files))
            .map(Arc::new)
            .collect();
        let surface = self.surface.restore(&files, strings)?;
        Some((
            LibraryResult {
                diagnostics,
                cycle_fingerprint: ApiSignature::from_bytes(self.fingerprint),
            },
            surface,
        ))
    }
}

/// The persisted per-library reference surface — the [`LibraryIndex`]
/// mirror that makes a cross-session replay's query faces answer
/// identically to the recompute it skipped. The index's derived columns
/// (the CSR run tables `runs`/`name_runs`/`name_refs`, the id-ordered
/// `super_runs` directory) are pure functions of the row tables, so the
/// payload carries the rows only and [`LibraryIndex::assemble`]
/// re-derives them at load — the same derivation [`LibraryBuilder::finish`]
/// runs, keeping the two constructors column-identical by construction.
///
/// Interned ids are process-local by contract, exactly as in
/// [`PersistedFacts`]: file references ride the run's shared `files`
/// path table (a `u32` index per row), and every interned `StringId`
/// text joins a per-payload `strings` slice once, referenced by `u32`
/// index — the same intern-the-texts-not-the-ids discipline the facts
/// mirror follows. `StringId::NONE` columns (a bare ref's qualifier, a
/// library-face target's container) encode as `None`, one tag byte under
/// the binary codec (#4207) — no field may be omitted on the wire: a
/// positional codec would decode skipped fields against the wrong bytes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersistedSurface {
    /// Resolved reference rows, in the index's stored order.
    pub refs: Vec<PersistedRefRow>,
    /// Unresolved-name rows of the built table.
    pub names: Vec<PersistedNameRow>,
    /// Seam rows appended after the build (`record_unresolved`) —
    /// empty in every surface the current engine produces, carried for
    /// the seam's faithful round-trip.
    pub overflow: Vec<PersistedNameRow>,
    /// Resolved supertype edges, in the index's stored order.
    pub supers: Vec<PersistedSubtypeEdge>,
    /// The name texts every `*_name`/`container`/`qualifier` index
    /// resolves through — first-use ordered, deduplicated.
    pub strings: Vec<String>,
}

/// One resolved reference row in payload form — the mirror of
/// [`RefRow`]: every `FileId`/`StringId` remapped to a `files`/`strings`
/// index, the kind as a mirror enum, `None` when the id is
/// `StringId::NONE`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersistedRefRow {
    /// Indexes the run's `files`.
    pub file: u32,
    /// Indexes the run's `files`.
    pub target_file: u32,
    /// Indexes [`PersistedSurface::strings`].
    pub target_name: u32,
    /// Indexes [`PersistedSurface::strings`]; `None` for
    /// `StringId::NONE` (library-face targets).
    pub target_container: Option<u32>,
    pub kind: PersistedRefKind,
    pub range: PersistedRange,
    /// Indexes [`PersistedSurface::strings`]; `None` for
    /// `StringId::NONE` (bare uses).
    pub qualifier: Option<u32>,
}

/// One unresolved-name row in payload form — the mirror of
/// [`NameRow`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersistedNameRow {
    /// Indexes [`PersistedSurface::strings`].
    pub name: u32,
    pub kind: PersistedRefKind,
    pub range: PersistedRange,
    pub qualified: bool,
}

/// One resolved supertype edge in payload form — the mirror of
/// [`SubtypeEdge`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersistedSubtypeEdge {
    /// Indexes the run's `files`.
    pub file: u32,
    /// Indexes [`PersistedSurface::strings`].
    pub name: u32,
    pub supertype: PersistedTargetRef,
    pub clause: PersistedSuperClause,
}

/// The mirror of [`TargetRef`]: a workspace declaration as a
/// `(file, name)` index pair, or the bare name when the supertype lies
/// outside the analyzed set.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PersistedTargetRef {
    /// A workspace declaration — indexes `(files, strings)`.
    Decl { file: u32, name: u32 },
    /// Outside the analyzed set — indexes `strings`.
    Name(u32),
}

/// The mirror of [`RefKind`] — the exhaustive match forcing a
/// persistence-format review on any variant addition.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PersistedRefKind {
    Referenced,
    Read,
    Invoked,
    Written,
    ReadWritten,
}

/// The mirror of `dartq_index::SuperClause` — the same
/// review-forcing exhaustive match.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PersistedSuperClause {
    Extends,
    With,
    On,
    Implements,
}

/// The per-payload intern table under construction: every name text gets
/// one `strings` slot, first-use ordered. The run's `Files` sink covers
/// the file half; this covers the string half — two remaps, one row
/// domain each.
struct Names<'a> {
    table: &'a StringTable,
    texts: Vec<String>,
    at: rustc_hash::FxHashMap<StringId, u32>,
}

impl<'a> Names<'a> {
    fn new(table: &'a StringTable) -> Self {
        Self {
            table,
            texts: Vec::new(),
            at: rustc_hash::FxHashMap::default(),
        }
    }

    /// Resolves `id` to its text and interning it in the payload's own
    /// table, deduplicated. `StringId::NONE` — the packed-row optional
    /// column's sentinel — answers `None` instead of a slot (the
    /// absent-key encoding).
    #[expect(clippy::cast_possible_truncation)] // span arithmetic narrows under the scanner u32 offset cap
    fn id(&mut self, id: StringId) -> Option<u32> {
        if id == StringId::NONE {
            return None;
        }
        if let Some(&at) = self.at.get(&id) {
            return Some(at);
        }
        self.texts.push(self.table.resolve(id).to_owned());
        let at = self.texts.len() as u32 - 1;
        self.at.insert(id, at);
        Some(at)
    }

    /// The required-column twin of [`Self::id`] — a name a row cannot
    /// drop. A `StringId::NONE` in a required slot degrades to `None`
    /// like an untracked file id: under-report (the run never reaches
    /// the store), never a crash on the accelerator path.
    fn name(&mut self, id: StringId) -> Option<u32> {
        self.id(id)
    }
}

impl PersistedSurface {
    /// Projects a resident surface onto the payload form — `files` is
    /// the run's shared file sink (ref targets reach past the member
    /// set), `strings` the engine's intern table the ids resolve
    /// against. `None` when any referenced file cannot encode — the
    /// same encode↔table pairing break as the diagnostics half
    /// (an engine bug; under-report, never a crash).
    fn of(surface: &LibraryIndex, files: &mut Files<'_>, strings: &StringTable) -> Option<Self> {
        let mut names = Names::new(strings);
        let refs = surface
            .refs()
            .iter()
            .map(|row| {
                Some(PersistedRefRow {
                    file: files.id(row.file)?,
                    target_file: files.id(row.target_file)?,
                    target_name: names.name(row.target_name)?,
                    target_container: names.id(row.target_container),
                    kind: PersistedRefKind::of(row.kind),
                    range: PersistedRange::of(row.range),
                    qualifier: names.id(row.qualifier),
                })
            })
            .collect::<Option<Vec<_>>>()?;
        let unresolved = |rows: &[NameRow], names: &mut Names<'_>| {
            rows.iter()
                .map(|row| {
                    Some(PersistedNameRow {
                        name: names.name(row.name)?,
                        kind: PersistedRefKind::of(row.kind),
                        range: PersistedRange::of(row.range),
                        qualified: row.qualified,
                    })
                })
                .collect::<Option<Vec<_>>>()
        };
        let overflow_rows = unresolved(surface.names_overflow_rows(), &mut names)?;
        let name_rows = unresolved(surface.names(), &mut names)?;
        let supers = surface
            .supers()
            .iter()
            .map(|edge| {
                Some(PersistedSubtypeEdge {
                    file: files.id(edge.file)?,
                    name: names.name(edge.name)?,
                    supertype: match edge.supertype {
                        TargetRef::Decl(file, name) => PersistedTargetRef::Decl {
                            file: files.id(file)?,
                            name: names.name(name)?,
                        },
                        TargetRef::Name(name) => PersistedTargetRef::Name(names.name(name)?),
                    },
                    clause: PersistedSuperClause::of(edge.clause),
                })
            })
            .collect::<Option<Vec<_>>>()?;
        Some(Self {
            refs,
            names: name_rows,
            overflow: overflow_rows,
            supers,
            strings: names.texts,
        })
    }

    /// Restores the surface: file indexes resolve against the run's
    /// materialized file slice (an out-of-range index is a corrupt
    /// payload — `None`, the same unattributable-row posture as the
    /// diagnostics half), name texts re-intern through `strings`, and
    /// [`LibraryIndex::assemble`] re-derives every derived column from
    /// the restored rows.
    fn restore(self, files: &[FileId], strings: &StringTable) -> Option<LibraryIndex> {
        let name = |at: u32| -> Option<StringId> {
            self.strings
                .get(at as usize)
                .map(|text| strings.intern(text))
        };
        let optional = |at: Option<u32>| -> Option<StringId> {
            match at {
                Some(at) => name(at),
                None => Some(StringId::NONE),
            }
        };
        let refs = self
            .refs
            .iter()
            .map(|row| {
                Some(RefRow {
                    file: *files.get(row.file as usize)?,
                    target_file: *files.get(row.target_file as usize)?,
                    target_name: name(row.target_name)?,
                    target_container: optional(row.target_container)?,
                    kind: row.kind.resolve(),
                    range: row.range.source_range(),
                    qualifier: optional(row.qualifier)?,
                })
            })
            .collect::<Option<Vec<_>>>()?;
        let unresolved = |rows: &[PersistedNameRow]| {
            rows.iter()
                .map(|row| {
                    Some(NameRow {
                        name: name(row.name)?,
                        kind: row.kind.resolve(),
                        range: row.range.source_range(),
                        qualified: row.qualified,
                    })
                })
                .collect::<Option<Vec<_>>>()
        };
        let overflow = unresolved(&self.overflow)?;
        let names = unresolved(&self.names)?;
        let supers = self
            .supers
            .iter()
            .map(|edge| {
                Some(SubtypeEdge {
                    file: *files.get(edge.file as usize)?,
                    name: name(edge.name)?,
                    supertype: match edge.supertype {
                        PersistedTargetRef::Decl {
                            file: target_file,
                            name: target_name,
                        } => TargetRef::Decl(*files.get(target_file as usize)?, name(target_name)?),
                        PersistedTargetRef::Name(target_name) => {
                            TargetRef::Name(name(target_name)?)
                        }
                    },
                    clause: edge.clause.resolve(),
                })
            })
            .collect::<Option<Vec<_>>>()?;
        Some(LibraryIndex::assemble(refs, names, overflow, supers))
    }
}

impl PersistedRefKind {
    fn of(kind: RefKind) -> Self {
        match kind {
            RefKind::Referenced => Self::Referenced,
            RefKind::Read => Self::Read,
            RefKind::Invoked => Self::Invoked,
            RefKind::Written => Self::Written,
            RefKind::ReadWritten => Self::ReadWritten,
        }
    }

    fn resolve(self) -> RefKind {
        match self {
            Self::Referenced => RefKind::Referenced,
            Self::Read => RefKind::Read,
            Self::Invoked => RefKind::Invoked,
            Self::Written => RefKind::Written,
            Self::ReadWritten => RefKind::ReadWritten,
        }
    }
}

impl PersistedSuperClause {
    fn of(clause: dartq_index::SuperClause) -> Self {
        match clause {
            dartq_index::SuperClause::Extends => Self::Extends,
            dartq_index::SuperClause::With => Self::With,
            dartq_index::SuperClause::On => Self::On,
            dartq_index::SuperClause::Implements => Self::Implements,
        }
    }

    fn resolve(self) -> dartq_index::SuperClause {
        match self {
            Self::Extends => dartq_index::SuperClause::Extends,
            Self::With => dartq_index::SuperClause::With,
            Self::On => dartq_index::SuperClause::On,
            Self::Implements => dartq_index::SuperClause::Implements,
        }
    }
}

/// Encodes a library run for the L3 store (#4207): positional binary via
/// `bincode-next`'s serde bridge — roughly a third of the JSON it
/// replaced on a reference-dense payload, and the codec the store's
/// entries are uniform under. `None` when the payload cannot encode (an
/// infallible shape today; the miss posture is the store's accelerator
/// contract — a run that cannot persist simply recomputes next session).
pub(crate) fn encode_run(payload: &PersistedLibraryRun) -> Option<Vec<u8>> {
    bincode_next::serde::encode_to_vec(payload, bincode_next::config::standard()).ok()
}

/// Decodes a stored library run; `None` on any decode failure. An entry
/// written under a previous codec (JSON-era frames are self-describing
/// text, never valid bincode) lands here as a miss — the run recomputes
/// and the sweep re-writes it under the current codec, so old entries
/// drain by use, never by a migration pass.
pub(crate) fn decode_run(bytes: &[u8]) -> Option<PersistedLibraryRun> {
    bincode_next::serde::decode_from_slice(bytes, bincode_next::config::standard())
        .ok()
        .map(|(payload, _)| payload)
}

/// The content-store key of one library run (#4199): blake3-128 over the
/// run's fork dimensions — the analyzer build ([`analyzer_build`], the
/// #3113 posture), the engine's `FACTS_SEMANTIC_VERSION`, the grants
/// configuration fingerprint, the package-graph fingerprint, the cycle
/// fingerprint (itself the transitive fold of member and dependency
/// faces, so equality of this field carries the whole dependency
/// closure), the library's ordered member content identity
/// ([`LibraryInfo::content`]), and the library's ordered member paths
/// (defining file first, parts after — [`library_member_paths`]). The
/// content column is load-bearing, not belt-and-braces: a member edit
/// that moves no element face (a body edit, a parse error) leaves the
/// cycle fingerprint byte-equal, and the in-process gate stays sound
/// there only because apply drops a result on every content move — the
/// cross-session key must reproduce that guard itself. The path column
/// is the same discipline for identity, not content: a run's rows and
/// surface anchor at the member paths verbatim (the payload carries
/// paths, replay mints them back), and every other column is
/// path-free — two byte-identical files at different paths (an
/// overlay-introduced file carrying another member's exact bytes) fold
/// equal everywhere else, so without this column one library's stored
/// run would falsely serve the other's diagnostics under the wrong
/// anchor (#4221). The lints twin folds the path for the same reason
/// ([`lint_rows_key`]). The leading columns pin everything neither fold
/// can see: a different binary, a semantic-shape bump, a
/// feature/experiment configuration move, or a package-graph repoint
/// each fork the key even when every signature stands — the
/// context-salt discipline, applied per run. Two processes over the
/// same workspace, SDK, options, and build derive identical keys for
/// identical inputs.
#[must_use]
pub(crate) fn library_run_key(
    build: [u8; 16],
    semantic_version: u32,
    grants: u64,
    packages: [u8; 16],
    fingerprint: &ApiSignature,
    content: &[ContentId],
    paths: &[&std::path::Path],
) -> ContentId {
    let mut bytes = Vec::with_capacity(58 + content.len() * 16);
    field(&mut bytes, &build);
    bytes.extend_from_slice(&semantic_version.to_le_bytes());
    bytes.extend_from_slice(&grants.to_le_bytes());
    field(&mut bytes, &packages);
    field(&mut bytes, fingerprint.as_bytes());
    bytes.extend_from_slice(&(content.len() as u32).to_le_bytes());
    for id in content {
        bytes.extend_from_slice(id.as_bytes());
    }
    bytes.extend_from_slice(&(paths.len() as u32).to_le_bytes());
    for path in paths {
        field(&mut bytes, path.as_os_str().as_encoded_bytes());
    }
    dartq_source::content_hash(&bytes)
}

/// The content-store key of one file's persisted lint rows (#4205): the
/// lint stage's twin of [`library_run_key`] — the same context-salt
/// discipline applied to the lint face. The fork dimensions the lints
/// layer's own row fingerprint does not pin: the analyzer build (a
/// different binary may lint differently — the #3113 posture), the grants
/// configuration fingerprint, and the package-graph fingerprint. The
/// lints layer's fingerprint (`fingerprint`, the memo key's fold) already
/// carries everything a row reads: the file's content id (`content`), the
/// settled features and language version, the enable set, the containing
/// package's facts, the path, the cannot-ignore set, the semantic
/// library-context generation, the breaking-version gate, and — where the
/// cross-library rule runs — the reachability-inputs epoch. Two processes
/// over the same workspace, SDK, options, and build derive identical keys
/// for identical inputs.
#[must_use]
pub(crate) fn lint_rows_key(
    build: [u8; 16],
    grants: u64,
    packages: [u8; 16],
    content: ContentId,
    fingerprint: u64,
) -> ContentId {
    let mut bytes = Vec::with_capacity(64);
    field(&mut bytes, &build);
    bytes.extend_from_slice(&grants.to_le_bytes());
    field(&mut bytes, &packages);
    bytes.extend_from_slice(content.as_bytes());
    bytes.extend_from_slice(&fingerprint.to_le_bytes());
    dartq_source::content_hash(&bytes)
}

/// The face a persisted answer record serves (#4243): the analyze face's
/// output spellings. The tag folds into the envelope key, so records of
/// different faces never alias — a format's bytes are its own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnswerFace {
    /// The LON machine document.
    AnalyzeLon,
    /// The JSON machine envelope, no memory key.
    AnalyzeJson,
    /// The JSON machine envelope whose `memory` member the serving
    /// process re-splices (the recorded blob holds the pre-splice bytes).
    AnalyzeJsonMemory,
    /// The human text face.
    AnalyzeText,
}

impl AnswerFace {
    /// The key's tag byte — one stable byte per face, folded first.
    fn tag(self) -> u8 {
        match self {
            Self::AnalyzeLon => 0,
            Self::AnalyzeJson => 1,
            Self::AnalyzeJsonMemory => 2,
            Self::AnalyzeText => 3,
        }
    }
}

/// One ask target of an answer shape, the canonicalized spelling the key
/// folds (the `aims_key` discipline): variant-tagged, path-sorted, deduped
/// asks name the same coverage and reuse one record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AnswerAim {
    /// A directory aim — analysis rooted at the directory.
    Dir(PathBuf),
    /// A file aim.
    File(PathBuf),
}

impl AnswerAim {
    /// The key's pair: the variant tag and the path bytes.
    fn leg(&self) -> (u8, &Path) {
        match self {
            Self::Dir(path) => (0, path.as_path()),
            Self::File(path) => (1, path.as_path()),
        }
    }
}

/// The answer's shape (#4243): the face plus the canonicalized target set
/// — everything about the ask, besides the workspace inputs the header,
/// grants, and stamps pin, that the rendered bytes depend on. An empty
/// aim set is the full-workspace ask.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AnswerShape {
    /// The served face.
    pub face: AnswerFace,
    /// The canonicalized targets (sorted, deduped — the constructor
    /// normalizes; the fields are pub for the builder tests).
    pub aims: Vec<AnswerAim>,
}

impl AnswerShape {
    /// The canonical form: aims sorted by (variant tag, path bytes) and
    /// deduped, so two asks naming the same coverage in different orders
    /// derive one key.
    #[must_use]
    pub fn new(face: AnswerFace, aims: Vec<AnswerAim>) -> Self {
        let mut aims = aims;
        aims.sort_by(|a, b| a.leg().cmp(&b.leg()));
        aims.dedup_by(|a, b| a.leg() == b.leg());
        Self { face, aims }
    }
}

/// The answered-turn envelope key (#4243): blake3-128 over the generation
/// header (SDK identity, package-graph fingerprint, context salt — the
/// analyzer build, context root, target, declared variables, and options
/// bytes all live in the salt), the feature-grants fingerprint (the
/// experiment configuration a run's output depends on, the same u64 the
/// run keys fold), and the canonical answer shape. Every input is
/// available at assembly time — zero store I/O — so two processes over
/// the same workspace, SDK, options, and build derive identical keys for
/// identical asks. The single derivation point: the daemon turn cache
/// (#4240) and the CLI probe read one spelling, never their own.
#[must_use]
pub fn envelope_key(header: &dartq_snapshot::GenerationHeader, grants: u64, shape: &AnswerShape) -> [u8; 16] {
    let mut bytes = Vec::with_capacity(96);
    bytes.extend_from_slice(&shape.face.tag().to_le_bytes());
    bytes.extend_from_slice(&header.format_version.to_le_bytes());
    bytes.extend_from_slice(&header.semantic_version.to_le_bytes());
    field(&mut bytes, header.sdk_identity.as_bytes());
    field(&mut bytes, &header.package_graph_fingerprint);
    field(&mut bytes, &header.context_salt);
    bytes.extend_from_slice(&grants.to_le_bytes());
    bytes.extend_from_slice(&(shape.aims.len() as u32).to_le_bytes());
    for aim in &shape.aims {
        let (tag, path) = aim.leg();
        bytes.extend_from_slice(&tag.to_le_bytes());
        field(&mut bytes, path.as_os_str().as_encoded_bytes());
    }
    blake3_128(&bytes)
}

/// One file's freshness stamp over the trust boundary the refresh guard
/// and the host freshness walk already accept (#2596), widened to the
/// full identity tuple: device and inode (same-bytes-same-file across
/// renames), mtime in nanoseconds, and size. `None` when the path cannot
/// stat — the caller omits the file from the stamp set, and its
/// appearance later changes the set.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnswerStamp {
    pub dev: u64,
    pub ino: u64,
    pub mtime_ns: u64,
    pub size: u64,
}

impl AnswerStamp {
    /// Stats `path` into the stamp tuple. Nanoseconds saturate at zero
    /// for a pre-epoch mtime; non-unix hosts fold zero device/inode
    /// columns (the mtime/size pair still pins the bytes).
    #[must_use]
    pub fn capture(path: &Path) -> Option<Self> {
        let meta = std::fs::metadata(path).ok()?;
        let mtime_ns = meta
            .modified()
            .ok()
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |span| u64::try_from(span.as_nanos()).unwrap_or(u64::MAX));
        #[cfg(unix)]
        let (dev, ino) = {
            use std::os::unix::fs::MetadataExt;
            (meta.dev(), meta.ino())
        };
        #[cfg(not(unix))]
        let (dev, ino) = (0, 0);
        Some(Self {
            dev,
            ino,
            mtime_ns,
            size: meta.len(),
        })
    }
}

/// The counts and severity presence a served answer replays (#4243): the
/// metrics the run reported and the flags the exit tier re-derives
/// against the serving process's own fatal posture.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnswerSummary {
    pub files: u64,
    pub loc: u64,
    pub diagnostics: u64,
    pub libraries_analyzed: u64,
    pub libraries_from_cache: u64,
    pub has_error: bool,
    pub has_warning: bool,
    pub has_info: bool,
}

/// The answered-turn record's sidecar payload (#4243): the rendered
/// bytes' content-store id (hex — the [`ContentId`] wire spelling, no
/// serde feature dependency), the read-time stamp set the serving probe
/// re-derives and compares, and the summary a hit replays. Bincode under
/// the `encode_run` discipline — a decode failure of any kind is a miss,
/// the accelerator contract.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnsweredRecord {
    /// The payload codec version; a mismatched record decodes to a miss.
    pub version: u32,
    /// The rendered bytes' content id — the blob the store serves — in
    /// the `Display` hex spelling ([`ContentId::parse_hex`] on read; a
    /// foreign spelling decodes to a miss).
    pub blob: String,
    /// The stamp set captured at read time by the run that rendered the
    /// bytes — read time only, never a post-render re-walk — sorted by
    /// path.
    pub stamps: Vec<(PathBuf, AnswerStamp)>,
    /// The metrics/tier faces a hit replays.
    pub summary: AnswerSummary,
}

/// The answer record's codec version.
pub const ANSWER_RECORD_VERSION: u32 = 1;

/// Encodes an answer record (the [`encode_run`] discipline): positional
/// bincode; `None` when the payload cannot encode (the miss posture).
pub(crate) fn encode_answer(record: &AnsweredRecord) -> Option<Vec<u8>> {
    bincode_next::serde::encode_to_vec(record, bincode_next::config::standard()).ok()
}

/// Decodes a stored answer record; `None` on any failure (foreign bytes,
/// a torn sidecar, a future codec) — a miss, never an error.
pub(crate) fn decode_answer(bytes: &[u8]) -> Option<AnsweredRecord> {
    let (record, _) =
        bincode_next::serde::decode_from_slice::<AnsweredRecord, _>(bytes, bincode_next::config::standard())
            .ok()?;
    (record.version == ANSWER_RECORD_VERSION).then_some(record)
}

/// Why an answer probe missed: the trace taxonomy's machine keys — every
/// miss is the full path (the pure-accelerator contract), the reason only
/// observes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnswerMiss {
    /// No armed content store — nothing to read.
    NoStore,
    /// No sidecar under the derived key (a first run, an evicted record,
    /// or a forked header/grants/shape dimension).
    NoRecord,
    /// The sidecar decoded to nothing (torn write, future codec).
    Decode,
    /// The workspace stamp set moved against the record's read-time
    /// capture — the tree changed, the answer must re-render.
    Stamps,
    /// The record's blob is gone from the store (evicted between the
    /// sidecar write and this probe).
    Blob,
}

impl AnswerMiss {
    /// The trace row's machine key.
    pub fn key(self) -> &'static str {
        match self {
            Self::NoStore => "no_store",
            Self::NoRecord => "no_record",
            Self::Decode => "decode",
            Self::Stamps => "stamp_mismatch",
            Self::Blob => "blob_missing",
        }
    }
}

/// One answer probe's outcome (#4243): the served bytes and replayed
/// summary on a hit, the miss reason on a miss.
#[derive(Debug, PartialEq)]
pub enum AnswerProbe {
    /// The rendered bytes a full run produced under inputs pinned equal,
    /// with the summary the metrics and exit tier replay.
    Hit {
        /// The stored blob's bytes, served verbatim.
        bytes: Vec<u8>,
        /// The recorded counts and severity presence.
        summary: AnswerSummary,
    },
    /// A miss — the caller takes today's full path unchanged.
    Miss(AnswerMiss),
}

/// One extracted `@docImport` in payload form — the mirror of
/// [`DocImportFacts`]: the decoded URI (or `None` for interpolation), the
/// raw literal's UTF-16 span, and the resolved diagnostic code.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersistedDocImport {
    pub uri: Option<String>,
    pub uri_span: PersistedRange,
    pub code: Option<PersistedResolutionCode>,
}

/// One extracted, resolved directive in payload form.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersistedDirective {
    pub kind: PersistedDirectiveKind,
    pub uri: Option<String>,
    pub uri_span: PersistedRange,
    pub target: PersistedTarget,
    pub code: Option<PersistedResolutionCode>,
    /// The `as` prefix text.
    pub prefix: Option<String>,
    pub deferred: bool,
    /// The `show`/`hide` clauses in source order, each carrying its written
    /// token sequence: names as written (bare identifiers, written order,
    /// duplicates preserved — the constructor re-derives the `name=` setter
    /// spellings on restore) with the tokens' captured UTF-16 spans (#539,
    /// the undefined-combinator report anchors).
    pub combinators: Vec<PersistedCombinator>,
    /// The dotted name of `library a.b;` / `part of a.b;`.
    pub name: Option<String>,
    /// The `if` clauses in source order (#512): condition name/value,
    /// candidate URI, and the URI literal's span (a selected branch's
    /// missing-target diagnostic attributes to the branch literal, so the
    /// span rides with the clause). Restore splits them back into the
    /// index-locked [`DirectiveFacts`] columns.
    pub configurations: Vec<PersistedConfiguration>,
}

/// One conditional-import clause in payload form.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersistedConfiguration {
    /// The condition's dotted name, as written (`dart.library.io`, a `-D`
    /// name).
    pub dotted_name: String,
    /// The `=` value; `None` for the bare form (and for a written `= ''` —
    /// the constructor folds it, `valueOrTrue`).
    pub value: Option<String>,
    /// The URI string selected when the condition matches.
    pub uri: String,
    /// Whether the clause URI is a string interpolation — upstream's
    /// `DirectiveUriWithoutString` face (`file_state.dart:118`): the clause
    /// still takes its selection slot, and selecting it reports
    /// `uri_with_interpolation` instead of resolving `uri` (a placeholder
    /// empty string on the wire). Always written on the wire: the binary
    /// codec is positional, so no field may omit itself (#4207) — the
    /// universal `false` costs its one tag byte.
    pub interpolated: bool,
    /// UTF-16 span of the clause's URI literal.
    pub uri_span: PersistedRange,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PersistedDirectiveKind {
    Import,
    Export,
    Part,
    PartOf,
    Library,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PersistedTarget {
    /// A file target; indexes [`PersistedFacts::files`].
    File(u32),
    External,
    Missing,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersistedCombinator {
    /// `hide` when set, `show` otherwise.
    pub hide: bool,
    /// The clause's names as written: written order, duplicates preserved
    /// (the stored clause sets are sorted and deduplicated, so this
    /// sequence can only come from [`Combinators::written`]). One payload
    /// element is exactly one source clause (#719): the clause boundary is
    /// observable (`Combinators::written_shapes_equal`'s duplicate-directive
    /// identity and the duplicate-combinator check's per-clause seen-set),
    /// so adjacent same-kind clauses stay separate elements.
    pub names: Vec<String>,
    /// UTF-16 span per written name, index-locked to [`Self::names`] —
    /// the cold capture's report anchors (#539), rebuilt through
    /// `Combinators::with_spans` on restore.
    pub name_spans: Vec<PersistedRange>,
}

/// Mirror of the frozen resolution code set; an exhaustive match on
/// both sides makes a new [`ResolutionCode`] variant a compile error here,
/// i.e. a forced persistence-format review instead of silent truncation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PersistedResolutionCode {
    UriDoesNotExist,
    UriDoesNotExistInDocImport,
    UriHasNotBeenGenerated,
    InvalidUri,
    UriWithInterpolation,
    UseOfNativeExtension,
    ImportOfNonLibrary,
    ExportOfNonLibrary,
    PartOfNonPart,
    PartOfDifferentLibrary,
    PartOfUnnamedLibrary,
    DuplicatePart,
    AmbiguousImport,
    AmbiguousExport,
    SharedDeferredPrefix,
}

impl PersistedResolutionCode {
    fn of(code: ResolutionCode) -> Self {
        match code {
            ResolutionCode::UriDoesNotExist => Self::UriDoesNotExist,
            ResolutionCode::UriDoesNotExistInDocImport => Self::UriDoesNotExistInDocImport,
            ResolutionCode::UriHasNotBeenGenerated => Self::UriHasNotBeenGenerated,
            ResolutionCode::InvalidUri => Self::InvalidUri,
            ResolutionCode::UriWithInterpolation => Self::UriWithInterpolation,
            ResolutionCode::UseOfNativeExtension => Self::UseOfNativeExtension,
            ResolutionCode::ImportOfNonLibrary => Self::ImportOfNonLibrary,
            ResolutionCode::ExportOfNonLibrary => Self::ExportOfNonLibrary,
            ResolutionCode::PartOfNonPart => Self::PartOfNonPart,
            ResolutionCode::PartOfDifferentLibrary => Self::PartOfDifferentLibrary,
            ResolutionCode::PartOfUnnamedLibrary => Self::PartOfUnnamedLibrary,
            ResolutionCode::DuplicatePart => Self::DuplicatePart,
            ResolutionCode::AmbiguousImport => Self::AmbiguousImport,
            ResolutionCode::AmbiguousExport => Self::AmbiguousExport,
            ResolutionCode::SharedDeferredPrefix => Self::SharedDeferredPrefix,
        }
    }

    fn resolve(self) -> ResolutionCode {
        match self {
            Self::UriDoesNotExist => ResolutionCode::UriDoesNotExist,
            Self::UriDoesNotExistInDocImport => ResolutionCode::UriDoesNotExistInDocImport,
            Self::UriHasNotBeenGenerated => ResolutionCode::UriHasNotBeenGenerated,
            Self::InvalidUri => ResolutionCode::InvalidUri,
            Self::UriWithInterpolation => ResolutionCode::UriWithInterpolation,
            Self::UseOfNativeExtension => ResolutionCode::UseOfNativeExtension,
            Self::ImportOfNonLibrary => ResolutionCode::ImportOfNonLibrary,
            Self::ExportOfNonLibrary => ResolutionCode::ExportOfNonLibrary,
            Self::PartOfNonPart => ResolutionCode::PartOfNonPart,
            Self::PartOfDifferentLibrary => ResolutionCode::PartOfDifferentLibrary,
            Self::PartOfUnnamedLibrary => ResolutionCode::PartOfUnnamedLibrary,
            Self::DuplicatePart => ResolutionCode::DuplicatePart,
            Self::AmbiguousImport => ResolutionCode::AmbiguousImport,
            Self::AmbiguousExport => ResolutionCode::AmbiguousExport,
            Self::SharedDeferredPrefix => ResolutionCode::SharedDeferredPrefix,
        }
    }
}

/// One diagnostic in payload form. The code rides as its four registry
/// columns: registry-known names restore as the canonical `&'static` row;
/// foreign names (a lint table outside the builtin registry) rebuild from
/// the stored columns so no diagnostic is ever dropped on load — the one
/// exception being a corrupt out-of-range file index, which
/// `PersistedDiagnostic::restore` drops as unattributable.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersistedDiagnostic {
    pub severity: PersistedSeverity,
    pub code_name: String,
    pub code_machine: String,
    pub code_dtype: PersistedDiagnosticType,
    pub code_published: bool,
    pub message: String,
    /// Indexes [`PersistedFacts::files`].
    pub file: u32,
    pub range: PersistedRange,
    pub correction: Option<String>,
    /// The `contextMessages` rows, each carrying its own file index. `None`
    /// is the no-context face — one tag byte under the binary codec (no
    /// field may omit itself on a positional wire, #4207); `Some` persists
    /// every element's message, file, and UTF-16 span. A corrupt file index
    /// inside a context row drops just that element on restore (under-report,
    /// never a crash) — the parent diagnostic survives because its own
    /// `file` already resolved.
    pub context: Option<Vec<PersistedContextMessage>>,
}

/// One `contextMessages` element in payload form — the mirror of
/// [`dartq_diagnostics::ContextMessage`] with its `FileId` remapped to a
/// [`PersistedFacts::files`] index.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersistedContextMessage {
    /// The context message text.
    pub message: String,
    /// Indexes [`PersistedFacts::files`].
    pub file: u32,
    /// UTF-16 span within the referenced file.
    pub range: PersistedRange,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PersistedSeverity {
    Info,
    Warning,
    Error,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PersistedDiagnosticType {
    SyntacticError,
    CompileTimeError,
    StaticWarning,
    Lint,
    Hint,
    Todo,
    Tool,
}

impl PersistedDirective {
    fn of(directive: &DirectiveFacts, strings: &StringTable, files: &mut Files<'_>) -> Self {
        Self {
            kind: match directive.kind {
                DirectiveKind::Import => PersistedDirectiveKind::Import,
                DirectiveKind::Export => PersistedDirectiveKind::Export,
                DirectiveKind::Part => PersistedDirectiveKind::Part,
                DirectiveKind::PartOf => PersistedDirectiveKind::PartOf,
                DirectiveKind::Library => PersistedDirectiveKind::Library,
            },
            uri: directive.uri.as_ref().map(std::string::ToString::to_string),
            uri_span: PersistedRange::of(directive.uri_span),
            target: match directive.target {
                // An unencodable file id degrades to the missing-target
                // face — the load side's out-of-range posture mirrored
                // (`under-report, never a crash`).
                ResolvedTarget::Resolved(file) => files
                    .id(file)
                    .map_or(PersistedTarget::Missing, PersistedTarget::File),
                ResolvedTarget::External => PersistedTarget::External,
                ResolvedTarget::Missing => PersistedTarget::Missing,
            },
            code: directive.code.map(PersistedResolutionCode::of),
            prefix: directive.prefix.map(|id| strings.resolve(id).to_owned()),
            deferred: directive.deferred,
            combinators: written_clauses(directive.combinators.written_with_clauses(), strings),
            name: directive
                .name
                .as_ref()
                .map(std::string::ToString::to_string),
            configurations: directive
                .configurations
                .iter()
                .zip(directive.configuration_spans.iter())
                .enumerate()
                .map(|(index, (config, span))| PersistedConfiguration {
                    dotted_name: config.dotted_name.to_string(),
                    value: config.value.as_ref().map(std::string::ToString::to_string),
                    uri: config.uri.to_string(),
                    interpolated: directive
                        .interpolated_configurations
                        .contains(&(index as u32)),
                    uri_span: PersistedRange::of(*span),
                })
                .collect(),
        }
    }

    fn restore(self, strings: &StringTable, files: &[FileId]) -> DirectiveFacts {
        let mut interpolated_configurations: Vec<u32> = Vec::new();
        let (configurations, configuration_spans): (Vec<Configuration>, Vec<SourceRange>) = self
            .configurations
            .into_iter()
            .enumerate()
            .map(|(index, config)| {
                let span = config.uri_span.source_range();
                if config.interpolated {
                    interpolated_configurations.push(u32::try_from(index).unwrap_or(u32::MAX));
                }
                (
                    Configuration::new(&config.dotted_name, config.value.as_deref(), &config.uri),
                    span,
                )
            })
            .unzip();
        let combinators = {
            // Disk payload: a clause whose name and span columns disagree is
            // a corrupt entry — serde-valid, so only the loader can see it.
            // The whole directive's combinator face degrades to the empty
            // one (the face a cold capture gives an import with no show/hide
            // clauses) instead of tripping the constructor's capture-bug
            // assert: under-report, never a crash, the same posture as the
            // target index above. Zip-truncating was rejected — a truncated
            // clause is a face no source spelling could have produced, while
            // the empty face is the real no-combinator input. The live
            // capture path keeps the assert: there a mismatch is an internal
            // bug, not disk rot.
            let clauses = self.combinators;
            let desynced = clauses
                .iter()
                .any(|clause| clause.names.len() != clause.name_spans.len());
            if desynced {
                Combinators::unfiltered()
            } else {
                Combinators::with_spans(
                    clauses.into_iter().map(|clause| {
                        // Written order and duplicates ride verbatim; the
                        // constructor re-expands the `name=` setter spellings
                        // and re-sorts the clause sets exactly as the cold
                        // capture did (l1.rs walk_import_export → the same
                        // `with_spans` face).
                        let names: Box<[StringId]> = clause
                            .names
                            .iter()
                            .map(|text| strings.intern(text))
                            .collect();
                        let spans: Box<[SourceRange]> = clause
                            .name_spans
                            .iter()
                            .map(|span| span.source_range())
                            .collect();
                        let combinator = if clause.hide {
                            CombinatorClause::Hide(names)
                        } else {
                            CombinatorClause::Show(names)
                        };
                        (combinator, spans)
                    }),
                    strings,
                )
            }
        };
        DirectiveFacts {
            kind: match self.kind {
                PersistedDirectiveKind::Import => DirectiveKind::Import,
                PersistedDirectiveKind::Export => DirectiveKind::Export,
                PersistedDirectiveKind::Part => DirectiveKind::Part,
                PersistedDirectiveKind::PartOf => DirectiveKind::PartOf,
                PersistedDirectiveKind::Library => DirectiveKind::Library,
            },
            uri: self.uri.map(Box::from),
            uri_span: self.uri_span.source_range(),
            target: match self.target {
                // The index is disk payload: an out-of-range value is a
                // corrupt entry, and the file's miss-never-error posture
                // degrades to the missing-target face instead of panicking
                // the loader (under-report, never a crash).
                PersistedTarget::File(at) => files
                    .get(at as usize)
                    .copied()
                    .map_or(ResolvedTarget::Missing, ResolvedTarget::Resolved),
                PersistedTarget::External => ResolvedTarget::External,
                PersistedTarget::Missing => ResolvedTarget::Missing,
            },
            code: self.code.map(PersistedResolutionCode::resolve),
            prefix: self.prefix.as_deref().map(|text| strings.intern(text)),
            deferred: self.deferred,
            combinators,
            name: self.name.map(Box::from),
            configurations: configurations.into(),
            configuration_spans: configuration_spans.into(),
            interpolated_configurations: interpolated_configurations.into(),
        }
    }
}

impl PersistedDiagnostic {
    /// `None` when the diagnostic's file cannot be encoded — the
    /// encode↔table pairing broken (an engine bug): the row skips, the
    /// load side's unattributable-row posture mirrored (persist is
    /// best-effort; under-report, never a crash).
    fn of(diagnostic: &Diagnostic, files: &mut Files<'_>) -> Option<Self> {
        Some(Self {
            severity: match diagnostic.severity {
                Severity::Info => PersistedSeverity::Info,
                Severity::Warning => PersistedSeverity::Warning,
                Severity::Error => PersistedSeverity::Error,
            },
            code_name: diagnostic.code.name.to_string(),
            code_machine: diagnostic.code.machine.to_string(),
            code_dtype: match diagnostic.code.dtype {
                DiagnosticType::SyntacticError => PersistedDiagnosticType::SyntacticError,
                DiagnosticType::CompileTimeError => PersistedDiagnosticType::CompileTimeError,
                DiagnosticType::StaticWarning => PersistedDiagnosticType::StaticWarning,
                DiagnosticType::Lint => PersistedDiagnosticType::Lint,
                DiagnosticType::Hint => PersistedDiagnosticType::Hint,
                DiagnosticType::Todo => PersistedDiagnosticType::Todo,
                DiagnosticType::Tool => PersistedDiagnosticType::Tool,
            },
            code_published: diagnostic.code.has_published_docs,
            message: diagnostic.message.into_owned(),
            file: files.id(diagnostic.file)?,
            range: PersistedRange::of(diagnostic.range),
            correction: diagnostic.correction.as_deref().map(str::to_owned),
            // A context row whose file fails the encode is an encode↔table
            // pairing break (an engine bug): the whole diagnostic skips,
            // the same under-report posture as its own `file` index above.
            context: diagnostic
                .context
                .as_deref()
                .map(|messages| {
                    messages
                        .iter()
                        .map(|m| {
                            Some(PersistedContextMessage {
                                message: m.message.as_str().to_owned(),
                                file: files.id(m.file)?,
                                range: PersistedRange::of(m.range),
                            })
                        })
                        .collect::<Option<Vec<_>>>()
                })
                .flatten(),
        })
    }

    /// Restores one diagnostic. Returns `None` only for a corrupt payload —
    /// a file index outside [`PersistedFacts::files`] cannot be faithfully
    /// attributed, and mis-attributing it to some other file would fabricate
    /// a diagnostic; the loader's never-fail posture drops the single
    /// unattributable row rather than panicking the whole load.
    fn restore(self, codes: &CodeRegistry, files: &[FileId]) -> Option<Diagnostic> {
        let file = *files.get(self.file as usize)?;
        let code = codes.get(&self.code_name).unwrap_or_else(|| {
            // A code outside the builtin registry (a lint table's own row):
            // rebuild from the stored columns. The spellings leak — the
            // intern-table discipline; the distinct-code count bounds it.
            CodeInfo::new(
                Box::leak(self.code_name.into_boxed_str()),
                Box::leak(self.code_machine.into_boxed_str()),
                match self.code_dtype {
                    PersistedDiagnosticType::SyntacticError => DiagnosticType::SyntacticError,
                    PersistedDiagnosticType::CompileTimeError => DiagnosticType::CompileTimeError,
                    PersistedDiagnosticType::StaticWarning => DiagnosticType::StaticWarning,
                    PersistedDiagnosticType::Lint => DiagnosticType::Lint,
                    PersistedDiagnosticType::Hint => DiagnosticType::Hint,
                    PersistedDiagnosticType::Todo => DiagnosticType::Todo,
                    PersistedDiagnosticType::Tool => DiagnosticType::Tool,
                },
                self.code_published,
            )
        });
        let severity = match self.severity {
            PersistedSeverity::Info => Severity::Info,
            PersistedSeverity::Warning => Severity::Warning,
            PersistedSeverity::Error => Severity::Error,
        };
        // A context row whose stored file index is out of range is a corrupt
        // payload: that element drops, the diagnostic it annotates survives
        // (under-report, never a crash — the same posture as a bad top-level
        // `file` index dropping the row itself, one layer down).
        let context: Vec<ContextMessage> = self
            .context
            .unwrap_or_default()
            .into_iter()
            .filter_map(|m| {
                Some(ContextMessage {
                    message: m.message.into(),
                    file: *files.get(m.file as usize)?,
                    range: m.range.source_range(),
                })
            })
            .collect();
        let diagnostic = Diagnostic::new(
            severity,
            code,
            self.message,
            file,
            self.range.source_range(),
            self.correction,
        )
        .with_context(context);
        Some(diagnostic)
    }
}

/// The file-id slice under construction: every referenced `FileId` gets one
/// path slot, first-use ordered. An id the engine's table never interned —
/// the encode↔table pairing broken, so an engine bug — answers `None` and
/// the referencing row skips or degrades: persist is best-effort, the
/// load side's under-report posture mirrored (`under-report, never a
/// crash`).
struct Files<'a> {
    table: &'a FileTable,
    paths: Vec<PathBuf>,
    at: rustc_hash::FxHashMap<FileId, u32>,
}

impl<'a> Files<'a> {
    fn new(table: &'a FileTable) -> Self {
        Self {
            table,
            paths: Vec::new(),
            at: rustc_hash::FxHashMap::default(),
        }
    }

    #[expect(clippy::cast_possible_truncation)] // span arithmetic narrows under the scanner u32 offset cap
    fn id(&mut self, file: FileId) -> Option<u32> {
        if let Some(&at) = self.at.get(&file) {
            return Some(at);
        }
        let path = self.table.path(file)?.to_path_buf();
        self.paths.push(path);
        let at = self.paths.len() as u32 - 1;
        self.at.insert(file, at);
        Some(at)
    }
}

/// The written combinator clauses in payload form (#539, #719): the
/// directive's written token sequence — what the cold capture (l1.rs
/// `walk_import_export` → `Combinators::with_spans`) recorded — split into
/// one payload element per source clause, written order and duplicates
/// preserved, each token with its captured span. The stored clause name
/// sets cannot be projected directly: they are expanded (sorted,
/// deduplicated, `name=` setter spellings appended), so the written surface
/// only exists on [`Combinators::written_with_clauses`] — and its clause
/// index must ride: W3 made clause boundaries observable
/// ([`Combinators::written_shapes_equal`]'s duplicate-directive identity
/// and the duplicate-combinator check's per-clause seen-set), so merging
/// adjacent same-kind clauses (`hide a hide a` → one `hide a, a`) would
/// make a warm start answer those queries differently from the cold
/// capture.
fn written_clauses(
    written: impl Iterator<Item = (StringId, bool, SourceRange, u32)>,
    strings: &StringTable,
) -> Vec<PersistedCombinator> {
    let mut clauses: Vec<PersistedCombinator> = Vec::new();
    let mut current = None;
    for (name, hidden, span, id) in written {
        match clauses.last_mut() {
            Some(clause) if current == Some(id) => {
                clause.names.push(strings.resolve(name).to_owned());
                clause.name_spans.push(PersistedRange::of(span));
            }
            _ => clauses.push(PersistedCombinator {
                hide: hidden,
                names: vec![strings.resolve(name).to_owned()],
                name_spans: vec![PersistedRange::of(span)],
            }),
        }
        current = Some(id);
    }
    clauses
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::DirectiveKind;
    use crate::{CancellationToken, Engine, EngineConfig, EngineEvent, memory_store};
    use dartq_pub::{LockFile, PackageConfig, Pubspec, build_graph};
    use dartq_semantic::persisted::{
        PersistedAnnotation, PersistedAugKind, PersistedAugTarget, PersistedAugment,
        PersistedAugmentFormal, PersistedAugmentModifiers, PersistedAugmentTypeParam,
        PersistedContainer, PersistedContainerKind, PersistedExtensionFace, PersistedFormal,
        PersistedFormals, PersistedMemberEntry, PersistedMemberKind, PersistedMicroKind,
        PersistedTopDecl, PersistedTy,
    };
    use dartq_snapshot::{SnapshotConfig, SnapshotStore};
    use dartq_source::SourceRange;
    use dartq_source::content_hash;
    use std::collections::BTreeMap;
    use std::path::Path;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// Byte-exact pinned encoding of [`shape_probe`], the representative
    /// persisted payload — the structural facts-shape guard. Any layout edit
    /// in the `PersistedFacts` tree (a field added, removed, renamed,
    /// retyped, or reordered; a serde representation change) moves these
    /// bytes while `FACTS_SHAPE_VERSION` (state.rs) stays put, and
    /// [`Self::persisted_payload_shape_is_pinned`] fails. The pinned literal
    /// (not a hash of it) is deliberate: it bites identically, is reviewable
    /// by eye, and the failure prints the actual encoding for a one-paste
    /// re-pin. Mirror-enum variant *additions* leave existing encodings
    /// unchanged (the pin cannot see them); the exhaustive `of`/`restore`
    /// matches already force that review at compile time.
    const FACTS_SHAPE_JSON_PIN: &str = concat!(
        r#"{"api_signature":[7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7],"directives":["#,
        r#"{"kind":"Import","uri":"package:a/a.dart","uri_span":{"offset":0,"length":18},"#,
        r#""target":{"File":0},"code":"UriDoesNotExist","prefix":"p","deferred":true,"#,
        r#""combinators":[{"hide":false,"names":["showOnly"],"name_spans":[{"offset":40,"#,
        r#""length":8}]},{"hide":true,"names":["hideMe"],"name_spans":[{"offset":53,"length":6}]}],"#,
        r#""name":"main","configurations":[{"dotted_name":"dart.library.io","value":null,"#,
        r#""uri":"io.dart","interpolated":false,"uri_span":{"offset":20,"length":9}},"#,
        r#"{"dotted_name":"flutter.mode","value":"release","uri":"release.dart","#,
        r#""interpolated":false,"uri_span":{"offset":36,"length":14}}]}],"#,
        r#""doc_imports":[{"uri":"package:b/b.dart","uri_span":{"offset":60,"length":17},"#,
        r#""code":"UriDoesNotExistInDocImport"},{"uri":null,"uri_span":{"offset":80,"length":9},"#,
        r#""code":"UriWithInterpolation"}],"#,
        r#""elements":{"strings":[],"decls":[],"containers":[],"container_order":[],"#,
        r#""extension_faces":[],"duplicates":[],"arity_violations":[],"const_creations":[],"augmentations":[],"decl_kinds":[],"anon_methods":[],"wildcard_duplicates":[],"formal_annotations":[],"ctor_annotations":[],"library_annotations":[]},"#,
        r#""diagnostics":["#,
        r#"{"severity":"Error","code_name":"uri_does_not_exist","code_machine":"URI_DOES_NOT_EXIST","#,
        r#""code_dtype":"CompileTimeError","code_published":false,"message":"Target of URI "#,
        r#"doesn't exist: 'package:a/a.dart'.","file":0,"range":{"offset":0,"length":18},"#,
        r#""correction":null,"context":null},{"severity":"Warning","code_name":"dead_code","#,
        r#""code_machine":"DEAD_CODE","code_dtype":"StaticWarning","code_published":true,"#,
        r#""message":"Dead code.","file":1,"range":{"offset":4,"length":9},"#,
        r#""correction":"Try removing the dead code.","context":[{"message":"'dead' "#,
        r#"is declared here.","file":1,"range":{"offset":0,"length":4}}]}],"#,
        r#""files":["/w/lib/main.dart","/w/lib/other.dart"]}"#,
    );

    /// The representative payload the shape pin guards: one instance of every
    /// engine-side payload struct in the `PersistedFacts` tree, exercising
    /// the `Some`/`None` faces (`uri`, `code`, `prefix`, `name`,
    /// `correction`, `context` — the second row carries a context message,
    /// pinning the `PersistedContextMessage` layout), both combinator
    /// directions, a file-target index, and two severities. The element
    /// model rides empty (`FileElements::default()` projects to empty
    /// columns): the pin guards the engine-side layout, including
    /// `PersistedElements`' own column list, while the populated element
    /// rows are pinned separately
    /// ([`Self::populated_elements_shape_is_pinned`]).
    fn shape_probe() -> PersistedFacts {
        let strings = StringTable::new();
        PersistedFacts {
            api_signature: [7; 16],
            directives: vec![PersistedDirective {
                kind: PersistedDirectiveKind::Import,
                uri: Some("package:a/a.dart".to_owned()),
                uri_span: PersistedRange::of(SourceRange {
                    offset: 0,
                    length: 18,
                }),
                target: PersistedTarget::File(0),
                code: Some(PersistedResolutionCode::UriDoesNotExist),
                prefix: Some("p".to_owned()),
                deferred: true,
                combinators: vec![
                    PersistedCombinator {
                        hide: false,
                        names: vec!["showOnly".to_owned()],
                        name_spans: vec![PersistedRange::of(SourceRange {
                            offset: 40,
                            length: 8,
                        })],
                    },
                    PersistedCombinator {
                        hide: true,
                        names: vec!["hideMe".to_owned()],
                        name_spans: vec![PersistedRange::of(SourceRange {
                            offset: 53,
                            length: 6,
                        })],
                    },
                ],
                name: Some("main".to_owned()),
                configurations: vec![
                    PersistedConfiguration {
                        dotted_name: "dart.library.io".to_owned(),
                        value: None,
                        uri: "io.dart".to_owned(),
                        interpolated: false,
                        uri_span: PersistedRange::of(SourceRange {
                            offset: 20,
                            length: 9,
                        }),
                    },
                    PersistedConfiguration {
                        dotted_name: "flutter.mode".to_owned(),
                        value: Some("release".to_owned()),
                        uri: "release.dart".to_owned(),
                        interpolated: false,
                        uri_span: PersistedRange::of(SourceRange {
                            offset: 36,
                            length: 14,
                        }),
                    },
                ],
            }],
            doc_imports: vec![
                PersistedDocImport {
                    uri: Some("package:b/b.dart".to_owned()),
                    uri_span: PersistedRange::of(SourceRange {
                        offset: 60,
                        length: 17,
                    }),
                    code: Some(PersistedResolutionCode::UriDoesNotExistInDocImport),
                },
                PersistedDocImport {
                    uri: None,
                    uri_span: PersistedRange::of(SourceRange {
                        offset: 80,
                        length: 9,
                    }),
                    code: Some(PersistedResolutionCode::UriWithInterpolation),
                },
            ],
            elements: PersistedElements::of(&dartq_semantic::FileElements::default(), &strings),
            diagnostics: vec![
                PersistedDiagnostic {
                    severity: PersistedSeverity::Error,
                    code_name: "uri_does_not_exist".to_owned(),
                    code_machine: "URI_DOES_NOT_EXIST".to_owned(),
                    code_dtype: PersistedDiagnosticType::CompileTimeError,
                    code_published: false,
                    message: "Target of URI doesn't exist: 'package:a/a.dart'.".to_owned(),
                    file: 0,
                    range: PersistedRange::of(SourceRange {
                        offset: 0,
                        length: 18,
                    }),
                    correction: None,
                    context: None,
                },
                PersistedDiagnostic {
                    severity: PersistedSeverity::Warning,
                    code_name: "dead_code".to_owned(),
                    code_machine: "DEAD_CODE".to_owned(),
                    code_dtype: PersistedDiagnosticType::StaticWarning,
                    code_published: true,
                    message: "Dead code.".to_owned(),
                    file: 1,
                    range: PersistedRange::of(SourceRange {
                        offset: 4,
                        length: 9,
                    }),
                    correction: Some("Try removing the dead code.".to_owned()),
                    context: Some(vec![PersistedContextMessage {
                        message: "'dead' is declared here.".to_owned(),
                        file: 1,
                        range: PersistedRange::of(SourceRange {
                            offset: 0,
                            length: 4,
                        }),
                    }]),
                },
            ],
            files: vec![
                PathBuf::from("/w/lib/main.dart"),
                PathBuf::from("/w/lib/other.dart"),
            ],
        }
    }

    /// The facts-shape guard (state.rs `FACTS_SHAPE_VERSION` contract): the
    /// representative payload's encoding must match the pinned bytes
    /// byte-for-byte, so a persisted-layout edit cannot land without the
    /// version bump that invalidates old snapshots. The message names the
    /// whole fix; the printed actual encoding is the re-pin value.
    #[test]
    fn persisted_payload_shape_is_pinned() {
        let bytes = serde_json::to_vec(&shape_probe()).expect("probe payload serializes");
        let encoded = String::from_utf8(bytes).expect("json is utf-8");
        assert_eq!(
            encoded, FACTS_SHAPE_JSON_PIN,
            "the persisted facts payload layout changed without a \
             FACTS_SHAPE_VERSION bump (state.rs): a snapshot written by the \
             old shape must never be served to code expecting the new one. \
             Bump FACTS_SHAPE_VERSION, extend its shape history, and re-pin \
             FACTS_SHAPE_JSON_PIN to the actual encoding printed below."
        );
        // Decode + re-encode must reproduce the pinned bytes: serde's own
        // representation of the shape is stable, not just the derive surface.
        let decoded: PersistedFacts =
            serde_json::from_str(FACTS_SHAPE_JSON_PIN).expect("pinned encoding decodes");
        assert_eq!(
            serde_json::to_string(&decoded).expect("re-encode"),
            FACTS_SHAPE_JSON_PIN,
            "decode+re-encode must reproduce the pinned encoding byte-for-byte"
        );
    }

    /// Byte-exact pinned encoding of [`Self::populated_elements_probe`] —
    /// the elements-subtree half of the facts-shape guard, the mirror rows
    /// the main probe rides empty. Any layout edit inside an element row
    /// (a mirror column added to a formal, member, or annotation; a serde
    /// representation change) moves these bytes while
    /// `FACTS_SHAPE_VERSION` (state.rs) stays put, and
    /// [`Self::populated_elements_shape_is_pinned`] fails — the same
    /// bump-the-version-and-re-pin contract as the main pin.
    const FACTS_SHAPE_ELEMENTS_JSON_PIN: &str = concat!(
        r#"{"strings":["C","m","x","y","int","0","f","z","Deprecated","('v')","String","E","T","List","probe"],"decls":[[6,{"kind":"TopLevelFn","type_params":0,"type_param_names":[],"var_ty":"Unknown","return_ty":"Void","formals":{"positional":[{"name":7,"ty":{"Function":{"nullable":true,"type_params":0,"type_params_named":[],"return_ty":{"Named":{"name":10,"nullable":false,"args":[]}},"positional":[{"Named":{"name":4,"nullable":false,"args":[]}}],"named":[],"required_positional":1}},"required":true,"covariant":false,"default_value":null}],"named":[]},"annotations":[]}]],"containers":[[0,{"kind":"Class","is_abstract":false,"is_base":false,"is_final":false,"is_interface":false,"is_mixin_class":false,"is_mixin_application":false,"superclass":null,"intro_superclass":false,"members":[[1,{"kind":"Method","is_augment":false,"is_static":false,"is_abstract":false,"is_external":false,"is_final":false,"is_const":false,"covariant":false,"type_params":0,"type_param_names":[],"type_param_bounds":[],"var_ty":"Unknown","return_ty":"Void","formals":{"positional":[{"name":2,"ty":{"Named":{"name":4,"nullable":false,"args":[]}},"required":true,"covariant":false,"default_value":null}],"named":[{"name":3,"ty":{"Named":{"name":4,"nullable":false,"args":[]}},"required":false,"covariant":true,"default_value":5}]},"annotations":[{"name":8,"offset":0,"length":16,"args_text":9}],"name_range":[34,1]}]],"type_params":0,"type_param_names":[],"type_param_bounds":[],"type_param_variances":[],"sealed":false,"with_clause":[],"implements_clause":[],"on_clause":[],"with_clause_args":[],"implements_clause_args":[],"on_clause_args":[],"superclass_args":[],"header_residue":[],"primary_formals":[],"enum_constants":[],"constructors":[],"has_unnamed_constructor":false,"has_const_constructor":false,"ctor_formals":[],"factory_formals":[],"annotations":[]}]],"container_order":[0],"extension_faces":[{"name":11,"on_type":13,"on_type_nullable":false,"on_ty":{"Named":{"name":13,"nullable":false,"args":[{"Named":{"name":12,"nullable":false,"args":[]}}]}},"type_param_names":[12],"type_param_bounds":[{"Named":{"name":4,"nullable":false,"args":[]}}],"members":[[14,{"kind":"Getter","is_augment":false,"is_static":false,"is_abstract":false,"is_external":false,"is_final":false,"is_const":false,"covariant":false,"type_params":0,"type_param_names":[],"type_param_bounds":[],"var_ty":"Unknown","return_ty":{"Named":{"name":4,"nullable":false,"args":[]}},"formals":null,"annotations":[],"name_range":null}]],"annotations":[]}],"duplicates":[],"arity_violations":[],"const_creations":[],"augmentations":[{"kind":"Class","name":0,"display_name":0,"name_range":{"offset":10,"length":1},"augment_kw":{"offset":1,"length":7},"container":null,"is_static":false,"is_final":false,"modifiers":{"abstract_kw":null,"base_kw":{"offset":8,"length":4},"final_kw":null,"interface_kw":null,"sealed_kw":null,"mixin_kw":null},"extends_kw":{"offset":15,"length":7},"type_params":[{"name":13,"name_range":{"offset":40,"length":1},"bound":[{"Named":{"name":4,"nullable":false,"args":[]}},{"offset":49,"length":3}]}],"type_params_close":{"offset":52,"length":1},"return_ty":null,"return_ty_range":null,"formals":[{"name":7,"name_range":{"offset":70,"length":1},"full_range":{"offset":66,"length":5},"ty":{"Named":{"name":4,"nullable":false,"args":[]}},"ty_range":{"offset":66,"length":3},"required_positional":true,"optional_positional":false,"named":false,"covariant_kw":null,"required_kw":null,"default_separator":null}],"parens":[{"offset":65,"length":1},{"offset":76,"length":1}],"group_open":null,"const_kw":null,"factory_kw":null,"is_complete":false}],"decl_kinds":[{"container":0,"name":0,"kind":"Constructor","is_final":false,"is_mixin_application":false,"is_const":true,"is_factory":false,"is_complete":false,"head_range":{"offset":58,"length":3},"name_range":null}],"anon_methods":[],"wildcard_duplicates":[],"formal_annotations":[],"ctor_annotations":[],"library_annotations":[]}"#,
    );

    /// The element rows the main probe rides past empty, modeled on the
    /// source `@Deprecated('v') class C { void m(int x, {covariant int y =
    /// 0}) {} } void f(String Function(int)? z) {} extension E<T extends
    /// int> on List<T> { int get probe => 1; }`: one top-level function
    /// row, one container row with one annotated method member, one
    /// extension face carrying a bounded type parameter, and the empty
    /// columns (container order, duplicates, arity). The mirror columns
    /// are populated where they earn their keep: the named formal carries
    /// the covariant bit and a default-value text (the #1637 columns), the
    /// member carries an annotation star with its argument-group text
    /// (#1083) and its name-token anchor (#1867), the top-level function's
    /// formal carries a function-typed declared type (the function-shape
    /// columns), and the extension face carries its `on` type, type
    /// parameters, and bounds (#2431/#2495 — the subtree whose columns
    /// first entered the tree unobserved) — so a layout edit in any of
    /// those subtrees moves the pinned bytes and forces the
    /// `FACTS_SHAPE_VERSION` bump the state.rs contract requires.
    fn populated_elements_probe() -> PersistedElements {
        PersistedElements {
            strings: vec![
                "C".to_owned(),
                "m".to_owned(),
                "x".to_owned(),
                "y".to_owned(),
                "int".to_owned(),
                "0".to_owned(),
                "f".to_owned(),
                "z".to_owned(),
                "Deprecated".to_owned(),
                "('v')".to_owned(),
                "String".to_owned(),
                "E".to_owned(),
                "T".to_owned(),
                "List".to_owned(),
                "probe".to_owned(),
            ],
            decls: vec![(
                6,
                PersistedTopDecl {
                    kind: PersistedMicroKind::TopLevelFn,
                    type_params: 0,
                    type_param_names: Vec::new(),
                    var_ty: PersistedTy::Unknown,
                    return_ty: PersistedTy::Void,
                    formals: Some(Box::new(PersistedFormals {
                        positional: vec![PersistedFormal {
                            name: 7,
                            // `String Function(int)?` — the function-shape
                            // columns (return type, parameter count,
                            // positional formals).
                            ty: PersistedTy::Function {
                                nullable: true,
                                type_params: 0,
                                type_params_named: Vec::new(),
                                return_ty: Box::new(PersistedTy::Named {
                                    name: 10,
                                    nullable: false,
                                    args: Vec::new(),
                                }),
                                positional: vec![PersistedTy::Named {
                                    name: 4,
                                    nullable: false,
                                    args: Vec::new(),
                                }],
                                named: Vec::new(),
                                // `String Function(int)` — the single
                                // positional formal is required.
                                required_positional: Some(1),
                            },
                            required: true,
                            covariant: false,
                            default_value: None,
                        }],
                        named: Vec::new(),
                    })),
                    annotations: Vec::new(),
                },
            )],
            containers: vec![(
                0,
                PersistedContainer {
                    kind: PersistedContainerKind::Class,
                    is_abstract: false,
                    is_base: false,
                    is_final: false,
                    is_interface: false,
                    is_mixin_class: false,
                    // A body class declaration, not a named mixin
                    // application (`class X = S with M`).
                    is_mixin_application: false,
                    intro_superclass: false,
                    superclass: None,
                    factory_formals: Vec::new(),
                    members: vec![(
                        1,
                        PersistedMemberEntry {
                            kind: PersistedMemberKind::Method,
                            is_augment: false,
                            is_static: false,
                            is_abstract: false,
                            is_external: false,
                            is_final: false,
                            is_const: false,
                            covariant: false,
                            type_params: 0,
                            type_param_names: Vec::new(),
                            type_param_bounds: Vec::new(),
                            var_ty: PersistedTy::Unknown,
                            return_ty: PersistedTy::Void,
                            formals: Some(Box::new(PersistedFormals {
                                positional: vec![PersistedFormal {
                                    name: 2,
                                    ty: PersistedTy::Named {
                                        name: 4,
                                        nullable: false,
                                        args: Vec::new(),
                                    },
                                    required: true,
                                    covariant: false,
                                    default_value: None,
                                }],
                                named: vec![PersistedFormal {
                                    name: 3,
                                    ty: PersistedTy::Named {
                                        name: 4,
                                        nullable: false,
                                        args: Vec::new(),
                                    },
                                    required: false,
                                    covariant: true,
                                    default_value: Some(5),
                                }],
                            })),
                            annotations: vec![PersistedAnnotation {
                                name: 8,
                                offset: 0,
                                length: 16,
                                args_text: Some(9),
                            }],
                            name_range: Some((34, 1)),
                        },
                    )],
                    type_params: 0,
                    type_param_names: Vec::new(),
                    type_param_bounds: Vec::new(),
                    // No type parameters, so no written variance modifiers
                    // (#3903's column, parallel to the spellings).
                    type_param_variances: Vec::new(),
                    sealed: false,
                    with_clause: Vec::new(),
                    implements_clause: Vec::new(),
                    on_clause: Vec::new(),
                    // The #3847 clause-face argument columns, parallel to
                    // the three name columns above (empty clauses ⇒ empty
                    // argument lists) and to the `extends` head.
                    with_clause_args: Vec::new(),
                    implements_clause_args: Vec::new(),
                    on_clause_args: Vec::new(),
                    superclass_args: Vec::new(),
                    header_residue: Vec::new(),
                    primary_formals: Vec::new(),
                    enum_constants: Vec::new(),
                    constructors: Vec::new(),
                    has_unnamed_constructor: false,
                    has_const_constructor: false,
                    ctor_formals: Vec::new(),
                    annotations: Vec::new(),
                },
            )],
            container_order: vec![0],
            // `extension E<T extends int> on List<T> { int get probe => 1;
            // }` — the face's name, `on` type, nullable bit, full `on` type
            // (`List<T>` keeps its argument), type-parameter names, bounds
            // (`int`, parallel to the names), and members.
            extension_faces: vec![PersistedExtensionFace {
                name: Some(11),
                on_type: Some(13),
                on_type_nullable: false,
                on_ty: PersistedTy::Named {
                    name: 13,
                    nullable: false,
                    args: vec![PersistedTy::Named {
                        name: 12,
                        nullable: false,
                        args: Vec::new(),
                    }],
                },
                type_param_names: vec![12],
                type_param_bounds: vec![PersistedTy::Named {
                    name: 4,
                    nullable: false,
                    args: Vec::new(),
                }],
                members: vec![(
                    14,
                    PersistedMemberEntry {
                        kind: PersistedMemberKind::Getter,
                        is_augment: false,
                        is_static: false,
                        is_abstract: false,
                        is_external: false,
                        is_final: false,
                        is_const: false,
                        covariant: false,
                        type_params: 0,
                        type_param_names: Vec::new(),
                        type_param_bounds: Vec::new(),
                        var_ty: PersistedTy::Unknown,
                        return_ty: PersistedTy::Named {
                            name: 4,
                            nullable: false,
                            args: Vec::new(),
                        },
                        formals: None,
                        annotations: Vec::new(),
                        name_range: None,
                    },
                )],
                annotations: Vec::new(),
            }],
            duplicates: Vec::new(),
            arity_violations: Vec::new(),
            const_creations: Vec::new(),
            // The #2784 augmentation capture face: one populated `augment`
            // row (header modifiers, a bounded type parameter, one typed
            // positional formal — every column the verifier's checks read)
            // and one constructor target row, so a layout edit in either
            // subtree moves the pinned bytes like every populated mirror.
            augmentations: vec![PersistedAugment {
                kind: PersistedAugKind::Class,
                name: 0,
                display_name: 0,
                name_range: PersistedRange {
                    offset: 10,
                    length: 1,
                },
                augment_kw: PersistedRange {
                    offset: 1,
                    length: 7,
                },
                container: None,
                is_static: false,
                is_final: false,
                modifiers: PersistedAugmentModifiers {
                    abstract_kw: None,
                    base_kw: Some(PersistedRange {
                        offset: 8,
                        length: 4,
                    }),
                    final_kw: None,
                    interface_kw: None,
                    sealed_kw: None,
                    mixin_kw: None,
                },
                extends_kw: Some(PersistedRange {
                    offset: 15,
                    length: 7,
                }),
                type_params: vec![PersistedAugmentTypeParam {
                    name: 13,
                    name_range: PersistedRange {
                        offset: 40,
                        length: 1,
                    },
                    bound: Some((
                        PersistedTy::Named {
                            name: 4,
                            nullable: false,
                            args: Vec::new(),
                        },
                        PersistedRange {
                            offset: 49,
                            length: 3,
                        },
                    )),
                }],
                type_params_close: Some(PersistedRange {
                    offset: 52,
                    length: 1,
                }),
                return_ty: None,
                return_ty_range: None,
                formals: vec![PersistedAugmentFormal {
                    name: Some(7),
                    name_range: PersistedRange {
                        offset: 70,
                        length: 1,
                    },
                    full_range: PersistedRange {
                        offset: 66,
                        length: 5,
                    },
                    ty: Some(PersistedTy::Named {
                        name: 4,
                        nullable: false,
                        args: Vec::new(),
                    }),
                    ty_range: Some(PersistedRange {
                        offset: 66,
                        length: 3,
                    }),
                    required_positional: true,
                    optional_positional: false,
                    named: false,
                    covariant_kw: None,
                    required_kw: None,
                    // The positional formal carries no `=`/`:` default.
                    default_separator: None,
                }],
                parens: Some((
                    PersistedRange {
                        offset: 65,
                        length: 1,
                    },
                    PersistedRange {
                        offset: 76,
                        length: 1,
                    },
                )),
                group_open: None,
                const_kw: None,
                factory_kw: None,
                is_complete: false,
            }],
            decl_kinds: vec![PersistedAugTarget {
                container: Some(0),
                name: 0,
                kind: PersistedAugKind::Constructor,
                is_final: false,
                is_mixin_application: false,
                is_const: true,
                is_factory: false,
                is_complete: false,
                // The constructor head's `errorRange` (#3201's column, the
                // v24 history entry): populated here per the probe's
                // pattern — a new column rides a real value, so a later
                // edit of its encoding moves the pinned bytes.
                head_range: Some(PersistedRange {
                    offset: 58,
                    length: 3,
                }),
                // The synthetic target row records no intro-fragment name
                // span; `None` is the span-less decode default.
                name_range: None,
            }],
            anon_methods: Vec::new(),
            // The #2914 wildcard-duplicate log and the #2924 annotation
            // stars (formal/constructor/library), ridden empty by this
            // probe: the populated mirror rows live in their own subtrees.
            wildcard_duplicates: Vec::new(),
            formal_annotations: Vec::new(),
            ctor_annotations: Vec::new(),
            library_annotations: Vec::new(),
        }
    }

    /// The elements-subtree half of the shape guard (state.rs
    /// `FACTS_SHAPE_VERSION` contract): the populated mirror rows must
    /// encode byte-for-byte to the pinned literal, so a mirror-layout edit
    /// cannot land without the version bump that invalidates old
    /// snapshots — these subtrees moved zero bytes while the main probe
    /// rode empty (facts-consistency audit 2026-10-17, G2). The message
    /// names the whole fix; the printed actual encoding is the re-pin
    /// value.
    #[test]
    fn populated_elements_shape_is_pinned() {
        let bytes =
            serde_json::to_vec(&populated_elements_probe()).expect("probe payload serializes");
        let encoded = String::from_utf8(bytes).expect("json is utf-8");
        assert_eq!(
            encoded, FACTS_SHAPE_ELEMENTS_JSON_PIN,
            "the persisted elements layout changed without a \
             FACTS_SHAPE_VERSION bump (state.rs): a snapshot written by the \
             old shape must never be served to code expecting the new one. \
             Bump FACTS_SHAPE_VERSION, extend its shape history, and re-pin \
             FACTS_SHAPE_ELEMENTS_JSON_PIN to the actual encoding printed \
             below."
        );
        // Decode + re-encode must reproduce the pinned bytes: serde's own
        // representation of the shape is stable, not just the derive surface.
        let decoded: PersistedElements =
            serde_json::from_str(FACTS_SHAPE_ELEMENTS_JSON_PIN).expect("pinned encoding decodes");
        assert_eq!(
            serde_json::to_string(&decoded).expect("re-encode"),
            FACTS_SHAPE_ELEMENTS_JSON_PIN,
            "decode+re-encode must reproduce the pinned encoding byte-for-byte"
        );
    }

    /// A throwaway directory tree for the graph fixtures: package dirs
    /// `a`/`b` and `.dart_tool/` under one canonical root, so every rebuild
    /// resolves the same root spellings and fingerprint deltas are pure
    /// column effects.
    fn graph_config_dir() -> PathBuf {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let root = std::env::temp_dir().join(format!(
            "dartq-engine-persist-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&root);
        for dir in ["a", "b", ".dart_tool"] {
            std::fs::create_dir_all(root.join(dir)).expect("fixture dir");
        }
        root.join(".dart_tool")
    }

    /// Builds a `PackageTable` over `config_dir` through the real
    /// dartq-pub/dartq-package faces — every identity column the
    /// fingerprint folds is live.
    fn graph_table(config_dir: &Path, config_text: &str, lock_text: &str) -> PackageTable {
        let config = PackageConfig::parse(config_text).expect("valid config");
        let lock = LockFile::parse(lock_text).expect("valid lock");
        let root_pubspec = Pubspec::parse("name: t\nversion: 0.1.0\nenvironment:\n  sdk: ^3.5.0\n")
            .expect("valid pubspec");
        let graph = build_graph(
            &config,
            &lock,
            &root_pubspec,
            &BTreeMap::new(),
            None,
            config_dir,
        )
        .expect("graph");
        PackageTable::build(&config, &graph, config_dir).expect("table")
    }

    /// Three packages (root `t` + `a` + `b`) in one config, plus the
    /// per-column mutation variants the sensitivity test applies.
    const GRAPH_CONFIG: &str = r#"{"configVersion": 2, "packages": [
        {"name": "t", "rootUri": "../", "languageVersion": "3.5"},
        {"name": "a", "rootUri": "../a", "languageVersion": "3.5"},
        {"name": "b", "rootUri": "../b", "languageVersion": "3.5"}
    ], "generator": "pub"}"#;
    const GRAPH_CONFIG_REORDERED: &str = r#"{"configVersion": 2, "packages": [
        {"name": "t", "rootUri": "../", "languageVersion": "3.5"},
        {"name": "b", "rootUri": "../b", "languageVersion": "3.5"},
        {"name": "a", "rootUri": "../a", "languageVersion": "3.5"}
    ], "generator": "pub"}"#;
    const GRAPH_CONFIG_NAME_MUTATED: &str = r#"{"configVersion": 2, "packages": [
        {"name": "t", "rootUri": "../", "languageVersion": "3.5"},
        {"name": "c", "rootUri": "../a", "languageVersion": "3.5"},
        {"name": "b", "rootUri": "../b", "languageVersion": "3.5"}
    ], "generator": "pub"}"#;
    const GRAPH_CONFIG_ROOT_MUTATED: &str = r#"{"configVersion": 2, "packages": [
        {"name": "t", "rootUri": "../", "languageVersion": "3.5"},
        {"name": "a", "rootUri": "../a2", "languageVersion": "3.5"},
        {"name": "b", "rootUri": "../b", "languageVersion": "3.5"}
    ], "generator": "pub"}"#;
    const GRAPH_CONFIG_LIB_MUTATED: &str = r#"{"configVersion": 2, "packages": [
        {"name": "t", "rootUri": "../", "languageVersion": "3.5"},
        {"name": "a", "rootUri": "../a", "packageUri": "lib2", "languageVersion": "3.5"},
        {"name": "b", "rootUri": "../b", "languageVersion": "3.5"}
    ], "generator": "pub"}"#;
    const GRAPH_CONFIG_VERSION_MUTATED: &str = r#"{"configVersion": 2, "packages": [
        {"name": "t", "rootUri": "../", "languageVersion": "3.5"},
        {"name": "a", "rootUri": "../a", "languageVersion": "3.6"},
        {"name": "b", "rootUri": "../b", "languageVersion": "3.5"}
    ], "generator": "pub"}"#;
    const GRAPH_LOCK_EMPTY: &str = "packages: {}\nsdks:\n  dart: \">=3.5.0 <4.0.0\"\n";
    /// Locking `a` as hosted flips its source category (Path → `PubCache`),
    /// the fifth folded column.
    const GRAPH_LOCK_HOSTED_A: &str = "packages:\n  a:\n    description:\n      name: a\n      sha256: \"0\"\n      url: \"https://pub.dev\"\n    source: hosted\n    version: \"1.0.0\"\nsdks:\n  dart: \">=3.5.0 <4.0.0\"\n";

    /// A synthetic SDK scaffold with parameterizable metadata — exactly the
    /// components `sdk_identity` folds (version, revision, libraries.json).
    fn sdk_snapshot(version: &str, revision: Option<&str>, libraries_json: &str) -> SdkSnapshot {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let root = std::env::temp_dir().join(format!(
            "dartq-engine-persist-sdk-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("lib/_internal")).expect("sdk dirs");
        std::fs::write(root.join("version"), version).expect("version");
        if let Some(revision) = revision {
            std::fs::write(root.join("revision"), revision).expect("revision");
        }
        std::fs::write(root.join("lib/libraries.json"), libraries_json).expect("libraries");
        std::fs::write(
            root.join("lib/_internal/allowed_experiments.json"),
            r#"{"version": 1, "experimentSets": {"sdkExperiments": []}, "sdk": {"default": {"experimentSet": "sdkExperiments"}}, "packages": {}}"#,
        )
        .expect("experiments");
        SdkSnapshot::load(&root, None).expect("synthetic SDK loads")
    }

    const SDK_LIBRARIES: &str = r#"{"vm": {"libraries": {"core": {"uri": "core/core.dart"}}}}"#;
    const SDK_LIBRARIES_MUTATED: &str =
        r#"{"vm": {"libraries": {"core": {"uri": "core/core2.dart"}}}}"#;

    /// Corrupt snapshot payloads never panic the loader: the file-id column
    /// is disk data, so both consumers bounds-check it — an out-of-range
    /// directive target degrades to the missing-target face (under-report),
    /// an out-of-range diagnostic file index drops the unattributable row.
    #[test]
    fn corrupt_file_indices_degrade_instead_of_panicking() {
        let strings = StringTable::new();
        let payload = PersistedFacts {
            api_signature: [7; 16],
            directives: vec![PersistedDirective {
                kind: PersistedDirectiveKind::Import,
                uri: Some("package:a/a.dart".to_owned()),
                uri_span: PersistedRange::of(SourceRange {
                    offset: 0,
                    length: 18,
                }),
                target: PersistedTarget::File(9),
                code: None,
                prefix: None,
                deferred: false,
                combinators: Vec::new(),
                name: None,
                configurations: Vec::new(),
            }],
            doc_imports: Vec::new(),
            elements: PersistedElements::of(&dartq_semantic::FileElements::default(), &strings),
            diagnostics: vec![PersistedDiagnostic {
                severity: PersistedSeverity::Error,
                code_name: "uri_does_not_exist".to_owned(),
                code_machine: "URI_DOES_NOT_EXIST".to_owned(),
                code_dtype: PersistedDiagnosticType::CompileTimeError,
                code_published: false,
                message: "Target of URI doesn't exist: 'package:a/a.dart'.".to_owned(),
                file: 9,
                range: PersistedRange::of(SourceRange {
                    offset: 0,
                    length: 18,
                }),
                correction: None,
                context: None,
            }],
            files: vec![PathBuf::from("/w/lib/main.dart")],
        };

        let codes = CodeRegistry::builtin();
        let restored_strings = StringTable::new();
        let mut restored_files = FileTable::new();
        let restored = payload.materialize(
            "import 'package:a/a.dart';\n",
            content_hash(b"import 'package:a/a.dart';\n"),
            &restored_strings,
            &codes,
            &mut |path| restored_files.intern(path),
        );
        assert_eq!(
            restored.directives[0].target,
            ResolvedTarget::Missing,
            "out-of-range target degrades to Missing"
        );
        assert!(
            restored.diagnostics.is_empty(),
            "the unattributable row is dropped"
        );
    }

    /// The fingerprint is the whole-generation miss gate:
    /// identical tables digest equally across fresh builds, config entry
    /// order is normalized away, and moving any one folded column (name,
    /// root, lib dir, language version, source category) forks the digest.
    #[test]
    fn package_graph_fingerprint_is_stable_and_column_sensitive() {
        let config_dir = graph_config_dir();
        let table = |config: &str, lock: &str| graph_table(&config_dir, config, lock);
        let base = table(GRAPH_CONFIG, GRAPH_LOCK_EMPTY);
        let base_digest = package_graph_fingerprint(&base);

        assert_eq!(
            base_digest,
            package_graph_fingerprint(&table(GRAPH_CONFIG, GRAPH_LOCK_EMPTY)),
            "two fresh builds of identical tables digest equally"
        );
        assert_eq!(
            base_digest,
            package_graph_fingerprint(&table(GRAPH_CONFIG_REORDERED, GRAPH_LOCK_EMPTY)),
            "config entry order is normalized away"
        );

        for (column, config, lock) in [
            ("name", GRAPH_CONFIG_NAME_MUTATED, GRAPH_LOCK_EMPTY),
            ("root", GRAPH_CONFIG_ROOT_MUTATED, GRAPH_LOCK_EMPTY),
            ("lib dir", GRAPH_CONFIG_LIB_MUTATED, GRAPH_LOCK_EMPTY),
            (
                "language version",
                GRAPH_CONFIG_VERSION_MUTATED,
                GRAPH_LOCK_EMPTY,
            ),
            ("source category", GRAPH_CONFIG, GRAPH_LOCK_HOSTED_A),
        ] {
            assert_ne!(
                base_digest,
                package_graph_fingerprint(&table(config, lock)),
                "{column} column must fork the fingerprint"
            );
        }
    }

    /// The SDK identity half of the miss gate: identical snapshots produce
    /// equal identities, and every folded component (version, revision,
    /// libraries.json content hash, target segment) moves it.
    #[test]
    fn sdk_identity_is_stable_and_component_sensitive() {
        let base = sdk_identity(&sdk_snapshot("3.13.1\n", None, SDK_LIBRARIES), "vm");
        assert!(
            base.contains("dart-sdk 3.13.1 revision - "),
            "an absent revision spells `-`: {base}"
        );
        assert_eq!(
            base,
            sdk_identity(&sdk_snapshot("3.13.1\n", None, SDK_LIBRARIES), "vm"),
            "identical snapshots produce equal identities"
        );

        let revised = sdk_identity(
            &sdk_snapshot("3.13.1\n", Some("abc123\n"), SDK_LIBRARIES),
            "vm",
        );
        assert!(
            revised.contains("revision abc123 ") && revised != base,
            "the revision component moves the identity: {revised}"
        );
        assert_ne!(
            base,
            sdk_identity(&sdk_snapshot("3.14.0\n", None, SDK_LIBRARIES), "vm"),
            "the version component moves the identity"
        );
        assert_ne!(
            base,
            sdk_identity(&sdk_snapshot("3.13.1\n", None, SDK_LIBRARIES_MUTATED), "vm"),
            "the libraries.json hash component moves the identity"
        );
        assert_ne!(
            base,
            sdk_identity(&sdk_snapshot("3.13.1\n", None, SDK_LIBRARIES), "js"),
            "the target segment moves the identity"
        );
    }

    /// A bare config dir for the salt fixtures, with an optional
    /// `analysis_options.yaml` (the file the options dimension folds).
    fn salt_config_dir(tag: &str, options: Option<&str>) -> PathBuf {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "dartq-engine-salt-{tag}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("fixture dir");
        if let Some(options) = options {
            std::fs::write(dir.join("analysis_options.yaml"), options).expect("options file");
        }
        dir
    }

    /// A throwaway cache root for the snapshot round-trip.
    fn snapshot_cache_dir() -> PathBuf {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let root = std::env::temp_dir().join(format!(
            "dartq-engine-snapshot-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&root);
        root
    }

    /// The context salt is a pure function of the fork dimensions and
    /// sensitive to every one of them: equal dimensions digest equally;
    /// the analyzer build, the context root, the target, a declared
    /// variable, and the located options content each move it.
    #[test]
    fn context_salt_is_stable_and_dimension_sensitive() {
        let plain = salt_config_dir("plain", None);
        let roots = |dir: &Path| vec![dir.to_path_buf()];
        let build = analyzer_build();
        let base = context_salt(build, &plain, "vm", &FxHashMap::default(), &roots(&plain));
        assert_eq!(
            base,
            context_salt(build, &plain, "vm", &FxHashMap::default(), &roots(&plain)),
            "equal dimensions digest equally"
        );
        assert_ne!(
            base,
            context_salt([7; 16], &plain, "vm", &FxHashMap::default(), &roots(&plain)),
            "the analyzer build moves the salt (#3113: a rebuilt binary \
             forks the generation, never serving the old build's facts)"
        );
        assert_ne!(
            base,
            context_salt(build, &plain, "js", &FxHashMap::default(), &roots(&plain)),
            "the target segment moves the salt"
        );
        let mut defined = FxHashMap::default();
        defined.insert("dart.vm.profile".to_owned(), "true".to_owned());
        assert_ne!(
            base,
            context_salt(build, &plain, "vm", &defined, &roots(&plain)),
            "a declared variable moves the salt"
        );
        let other_root = salt_config_dir("other-root", None);
        assert_ne!(
            base,
            context_salt(
                build,
                &other_root,
                "vm",
                &FxHashMap::default(),
                &roots(&plain)
            ),
            "the context root moves the salt"
        );

        let options = salt_config_dir("options", Some("linter:\n  rules:\n    - avoid_print\n"));
        let with_options = context_salt(
            build,
            &options,
            "vm",
            &FxHashMap::default(),
            &roots(&options),
        );
        assert_ne!(base, with_options, "located options content moves the salt");
        // The *content* (not the root path) is the folded dimension:
        // rewriting the same file in place moves the digest.
        std::fs::write(
            options.join("analysis_options.yaml"),
            "linter:\n  rules:\n    - avoid_catches_without_threes\n",
        )
        .expect("rewrite options");
        assert_ne!(
            with_options,
            context_salt(
                build,
                &options,
                "vm",
                &FxHashMap::default(),
                &roots(&options)
            ),
            "an options edit moves the salt"
        );
    }

    /// #3113's pin at the cache-key builder: the persisted-facts generation
    /// never serves across analyzer builds. A generation saved under this
    /// process's real header loads under its own header and misses under a
    /// header that differs only in the context salt — the dimension the
    /// analyzer build folds into, so a rebuilt or upgraded binary (same
    /// workspace, same SDK, same content, same `FACTS_SEMANTIC_VERSION`)
    /// takes the whole-generation miss and recomputes instead of replaying
    /// the previous build's baked rows.
    #[test]
    fn a_generation_never_loads_across_analyzer_builds() {
        let root = bare_workspace("build-fork");
        let mut engine = bare_engine(&root);
        engine.set_content(&root.join("lib/main.dart"), b"void main() {}\n");
        engine.apply_pending(&mut |_| {});
        let store = SnapshotStore::open(SnapshotConfig {
            root: Some(snapshot_cache_dir()),
            capacity_bytes: 64 * 1024 * 1024,
        })
        .expect("open store");
        let own = engine.snapshot_header();
        assert_eq!(
            engine.save_facts_snapshot(&store).expect("save"),
            1,
            "one entry for the one file"
        );
        assert!(
            store.load::<PersistedFacts>(&own).is_some(),
            "the saving build's header loads its generation"
        );
        let mut other_build = own.clone();
        other_build.context_salt = other_build.context_salt.map(|byte| byte ^ 0xff);
        assert_ne!(
            other_build.context_salt, own.context_salt,
            "the foreign build's salt differs by construction"
        );
        assert!(
            store.load::<PersistedFacts>(&other_build).is_none(),
            "another analyzer build never loads this generation — the #3113 \
             stale replay (a warm generation surviving an engine rebuild)"
        );
    }

    /// The analyzer build identity itself: stable within the process (one
    /// read, memoized) and content-derived — two calls agree byte-for-byte.
    #[test]
    fn analyzer_build_is_stable_within_a_process() {
        assert_eq!(
            analyzer_build(),
            analyzer_build(),
            "the executable's bytes cannot change under the running process"
        );
    }

    /// A pubspec-less workspace fixture: one `lib/main.dart` of identical
    /// bytes, no pubspec, no package config — the bare-tree shape whose
    /// empty package table once digested a context-invariant header
    /// (#2274).
    fn bare_workspace(tag: &str) -> PathBuf {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let root = std::env::temp_dir().join(format!(
            "dartq-engine-bare-{tag}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("lib")).expect("fixture dir");
        std::fs::write(root.join("lib/main.dart"), "void main() {}\n").expect("fixture file");
        root
    }

    /// The bare-tree engine assembly: the empty package table and empty
    /// package config the analyze face's floor builds (the
    /// `bare_assembly` posture — no pubspec anywhere above the target).
    fn bare_engine(root: &Path) -> Engine {
        let pub_config = Arc::new(
            PackageConfig::parse(r#"{"configVersion": 2, "packages": []}"#).expect("valid config"),
        );
        Engine::new(EngineConfig {
            store: memory_store(),
            sdk: sdk_snapshot("3.13.1\n", None, SDK_LIBRARIES),
            packages: PackageTable::basic(root, None).expect("table"),
            pub_config,
            config_dir: root.join(".dart_tool"),
            context_roots: Vec::new(),
            semdiag: None,
            target: "vm".to_owned(),
            declared_variables: FxHashMap::default(),
            experiments: Vec::new(),
        })
        .expect("engine")
    }

    /// The [`bare_engine`] assembly over one shared synthetic SDK root: the
    /// #4199 run key folds the grants fingerprint, which folds the SDK lib
    /// root (an SDK move is a legitimate miss), so a test asserting
    /// cross-engine replay must build every engine over the same root.
    fn bare_engine_over_sdk(root: &Path, sdk_root: &Path) -> Engine {
        let pub_config = Arc::new(
            PackageConfig::parse(r#"{"configVersion": 2, "packages": []}"#).expect("valid config"),
        );
        Engine::new(EngineConfig {
            store: memory_store(),
            sdk: SdkSnapshot::load(sdk_root, None).expect("synthetic SDK loads"),
            packages: PackageTable::basic(root, None).expect("table"),
            pub_config,
            config_dir: root.join(".dart_tool"),
            context_roots: Vec::new(),
            semdiag: None,
            target: "vm".to_owned(),
            declared_variables: FxHashMap::default(),
            experiments: Vec::new(),
        })
        .expect("engine")
    }

    /// The #2274 cross-workspace collision pin: two pubspec-less
    /// workspaces with byte-identical files digest distinct headers (the
    /// context root rides the salt, so the generation directories fork),
    /// the same workspace digests its header stably across engines (the
    /// equality a same-path warm start needs), and one tree's saved
    /// generation never loads under the other tree's header.
    #[test]
    fn bare_workspaces_fork_headers_and_never_cross_load() {
        let root_a = bare_workspace("a");
        let root_b = bare_workspace("b");
        let mut engine_a = bare_engine(&root_a);
        let engine_b = bare_engine(&root_b);

        // Distinct trees, identical bytes: the salts — and the whole
        // headers with them — stay apart.
        let header_a = engine_a.snapshot_header();
        let header_b = engine_b.snapshot_header();
        assert_ne!(
            header_a.context_salt, header_b.context_salt,
            "two pubspec-less trees must not share a context salt"
        );
        assert_ne!(header_a, header_b, "the headers fork");

        // Determinism: a fresh engine over the same root reproduces the
        // header byte-for-byte.
        assert_eq!(
            header_a,
            bare_engine(&root_a).snapshot_header(),
            "the same workspace digests the same header across engines"
        );

        // No cross-load: A's saved generation answers A's header and misses
        // B's — a `Some` for B would mean one shared generation directory
        // replaying a foreign workspace's facts, the #2274 defect.
        engine_a.set_content(&root_a.join("lib/main.dart"), b"void main() {}\n");
        engine_a.apply_pending(&mut |_| {});
        let store = SnapshotStore::open(SnapshotConfig {
            root: Some(snapshot_cache_dir()),
            capacity_bytes: 64 * 1024 * 1024,
        })
        .expect("open store");
        assert_eq!(
            engine_a.save_facts_snapshot(&store).expect("save"),
            1,
            "one entry for the one file"
        );
        assert!(
            store.load::<PersistedFacts>(&header_a).is_some(),
            "the workspace's own header loads its generation"
        );
        assert!(
            store.load::<PersistedFacts>(&header_b).is_none(),
            "a foreign pubspec-less workspace never loads the generation"
        );
    }

    /// The full warm-start seam (#482): save → fresh store load → ingest →
    /// the next apply serves the materialized facts — the warm face equals
    /// a from-scratch engine's on every observable column, and the
    /// directive re-resolves against the consuming file's directory —
    /// while an entry whose content moved since the save never serves (its
    /// file recomputes from scratch) and stands in staging until a later
    /// claim, the budget shed, or the next ingest replaces it (#1387 — the
    /// single-shot drain this seam replaced dropped precisely the later
    /// revert hits, an undo).
    #[test]
    fn warm_start_round_trips_and_stale_entries_never_serve() {
        let config_dir = graph_config_dir();
        let root = config_dir.parent().expect("fixture root").to_path_buf();
        let main_path = root.join("lib/main.dart");
        let other_path = root.join("lib/other.dart");
        let main_src = "import 'other.dart';\nvoid main() { other(); }\n";
        let other_src = "int other() => 1;\n";
        // The stale variant renames the declaration — an API-face change,
        // so the recomputed signature must differ from the saved one (a
        // body-only edit would legitimately keep it stable, the L2 gate's
        // own contract).
        let stale_other_src = "int other2() => 1;\n";

        let pub_config = Arc::new(PackageConfig::parse(GRAPH_CONFIG).expect("valid config"));
        let build_engine = || {
            Engine::new(EngineConfig {
                store: memory_store(),
                sdk: sdk_snapshot("3.13.1\n", None, SDK_LIBRARIES),
                packages: graph_table(&config_dir, GRAPH_CONFIG, GRAPH_LOCK_EMPTY),
                pub_config: Arc::clone(&pub_config),
                config_dir: config_dir.clone(),
                context_roots: Vec::new(),
                semdiag: None,
                target: "vm".to_owned(),
                declared_variables: FxHashMap::default(),
                experiments: Vec::new(),
            })
            .expect("engine")
        };
        let mut saved_engine = build_engine();
        let main_saved = saved_engine.set_content(&main_path, main_src.as_bytes());
        let other_saved = saved_engine.set_content(&other_path, other_src.as_bytes());
        let mut on_event = |_event: EngineEvent| {};
        let cancel = CancellationToken::new();
        saved_engine
            .analyze(&[], &mut on_event, &cancel)
            .expect("analyze");
        assert!(
            saved_engine
                .file_facts(main_saved)
                .expect("resident")
                .diagnostics
                .is_empty(),
            "the fixture analyzes clean"
        );

        let cache = snapshot_cache_dir();
        let store = SnapshotStore::open(SnapshotConfig {
            root: Some(cache.clone()),
            capacity_bytes: 64 * 1024 * 1024,
        })
        .expect("open store");
        assert_eq!(
            saved_engine.save_facts_snapshot(&store).expect("save"),
            2,
            "one entry per distinct content"
        );
        let header = saved_engine.snapshot_header();

        // A fresh process's world: fresh engine, fresh load, ingest.
        let mut warm_engine = build_engine();
        let snapshot = store.load::<PersistedFacts>(&header).expect("generation");
        assert_eq!(snapshot.entries.len(), 2);
        assert_eq!(warm_engine.ingest_facts_snapshot(snapshot), 2);
        assert_eq!(warm_engine.state.warm_start.len(), 2, "staged");

        // The same main content hits the snapshot; the mutated other is
        // stale — its entry's hash no longer names the live content.
        let main_warm = warm_engine.set_content(&main_path, main_src.as_bytes());
        let other_warm = warm_engine.set_content(&other_path, stale_other_src.as_bytes());
        warm_engine
            .analyze(&[], &mut on_event, &cancel)
            .expect("analyze");

        // The matched entry was consumed at its claim; the stale one is
        // unclaimable but stands (#1387) — it can only serve a later batch
        // whose content reverts to the staged identity, never this one.
        assert_eq!(
            warm_engine.state.warm_start.len(),
            1,
            "matched entry consumed at the claim, stale entry standing"
        );

        // The warm face equals the from-scratch engine on every observable.
        let reference = saved_engine.file_facts(main_saved).expect("resident");
        let warm = warm_engine.file_facts(main_warm).expect("resident");
        assert_eq!(
            reference.api_signature(),
            warm.api_signature(),
            "the warm signature equals a fresh compute"
        );
        assert_eq!(reference.diagnostics.len(), warm.diagnostics.len());
        assert!(warm.diagnostics.is_empty());
        assert_eq!(
            reference.line_index.line_count(),
            warm.line_index.line_count()
        );
        let ResolvedTarget::Resolved(warm_target) = warm.directives[0].target else {
            panic!("the warm import resolved");
        };
        let ResolvedTarget::Resolved(reference_target) = reference.directives[0].target else {
            panic!("the reference import resolved");
        };
        assert_eq!(
            warm_engine.file_table().path(warm_target),
            saved_engine.file_table().path(reference_target),
            "the warm directive re-resolves exactly as a fresh compute does"
        );

        // The stale file recomputed: its facts name the live content, not
        // the saved generation's.
        let stale = warm_engine.file_facts(other_warm).expect("resident");
        assert_eq!(stale.content(), content_hash(stale_other_src.as_bytes()));
        assert_ne!(
            stale.api_signature(),
            saved_engine
                .file_facts(other_saved)
                .expect("resident")
                .api_signature(),
        );

        let _ = std::fs::remove_dir_all(&cache);
    }

    /// The single generation's entries directory under `cache` — the
    /// fixtures write exactly one header, so the snapshots root holds one
    /// directory.
    fn only_entries_dir(cache: &Path) -> PathBuf {
        let mut generations: Vec<PathBuf> = std::fs::read_dir(cache.join("snapshots"))
            .expect("snapshots root")
            .flatten()
            .map(|entry| entry.path())
            .collect();
        assert_eq!(generations.len(), 1, "one generation per fixture");
        generations.pop().expect("generation").join("entries")
    }

    /// The #1386 flush discipline at the engine face: a flush with zero
    /// changes writes zero entries (every recorded key is skipped — no
    /// projection, no encode, no read, no rewrite), a one-file edit
    /// rewrites exactly that file's entry, and the delta flushes leave a
    /// generation a fresh store instance loads in full.
    #[test]
    fn flush_rewrites_only_changed_entries() {
        use filetime::{FileTime, set_file_mtime};
        use std::time::{Duration, UNIX_EPOCH};

        let config_dir = graph_config_dir();
        let root = config_dir.parent().expect("fixture root").to_path_buf();
        let main_path = root.join("lib/main.dart");
        let other_path = root.join("lib/other.dart");
        let main_src = "import 'other.dart';\nvoid main() { other(); }\n";
        let other_src = "int other() => 1;\n";
        let other_edited = "// edited\nint other() => 1;\n";

        let pub_config = Arc::new(PackageConfig::parse(GRAPH_CONFIG).expect("valid config"));
        let mut engine = Engine::new(EngineConfig {
            store: memory_store(),
            sdk: sdk_snapshot("3.13.1\n", None, SDK_LIBRARIES),
            packages: graph_table(&config_dir, GRAPH_CONFIG, GRAPH_LOCK_EMPTY),
            pub_config: Arc::clone(&pub_config),
            config_dir: config_dir.clone(),
            context_roots: Vec::new(),
            semdiag: None,
            target: "vm".to_owned(),
            declared_variables: FxHashMap::default(),
            experiments: Vec::new(),
        })
        .expect("engine");
        engine.set_content(&main_path, main_src.as_bytes());
        engine.set_content(&other_path, other_src.as_bytes());
        let mut on_event = |_event: EngineEvent| {};
        let cancel = CancellationToken::new();
        engine
            .analyze(&[], &mut on_event, &cancel)
            .expect("analyze");

        let cache = snapshot_cache_dir();
        let store = SnapshotStore::open(SnapshotConfig {
            root: Some(cache.clone()),
            capacity_bytes: 64 * 1024 * 1024,
        })
        .expect("open store");
        assert_eq!(engine.save_facts_snapshot(&store).expect("flush"), 2);

        // Plant a distinctly old mtime on every entry file: any rewrite
        // after this point is observable as a different mtime.
        let entries_dir = only_entries_dir(&cache);
        let entry_files = |dir: &Path| -> Vec<PathBuf> {
            let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
                .expect("entries dir")
                .flatten()
                .map(|entry| entry.path())
                .collect();
            files.sort();
            files
        };
        let first_flush = entry_files(&entries_dir);
        assert_eq!(first_flush.len(), 2, "one entry per distinct content");
        let planted = UNIX_EPOCH + Duration::from_secs(1_000_000);
        for path in &first_flush {
            set_file_mtime(path, FileTime::from_unix_time(1_000_000, 0)).expect("plant mtime");
        }
        let mtime = |path: &Path| {
            std::fs::metadata(path)
                .expect("entry meta")
                .modified()
                .expect("entry mtime")
        };

        // The no-change flush: the manifest still claims both keys, but
        // not one entry file is touched.
        assert_eq!(
            engine.save_facts_snapshot(&store).expect("no-change flush"),
            2,
            "the manifest still claims the full set"
        );
        for path in &first_flush {
            assert_eq!(
                mtime(path),
                planted,
                "an unchanged entry must not be rewritten: {}",
                path.display()
            );
        }

        // One content change: exactly that file's entry is rewritten (its
        // new key was never recorded), the untouched file's entry keeps
        // its planted mtime, and the swept old key leaves one coherent set.
        engine.set_content(&other_path, other_edited.as_bytes());
        engine
            .analyze(&[], &mut on_event, &cancel)
            .expect("analyze");
        assert_eq!(engine.save_facts_snapshot(&store).expect("flush"), 2);
        let after_edit = entry_files(&entries_dir);
        assert_eq!(
            after_edit.len(),
            2,
            "the edited file's old key is swept, the new key written"
        );
        let rewritten = after_edit
            .iter()
            .filter(|path| mtime(path) != planted)
            .count();
        assert_eq!(
            rewritten, 1,
            "exactly the edited file's entry pays the rewrite"
        );

        // Coherence: a fresh store instance (the daemon-restart shape)
        // loads the generation the delta flushes produced.
        let header = engine.snapshot_header();
        let fresh = SnapshotStore::open(SnapshotConfig {
            root: Some(cache.clone()),
            capacity_bytes: 64 * 1024 * 1024,
        })
        .expect("reopen store");
        let snapshot = fresh
            .load::<PersistedFacts>(&header)
            .expect("the delta-flushed generation loads in full");
        assert_eq!(snapshot.entries.len(), 2);

        let _ = std::fs::remove_dir_all(&cache);
    }

    /// A directive with show/hide combinators, a file target, a prefix, and
    /// a diagnostic survive the payload round trip; the api signature rides
    /// byte-exact; the restored line index and content hash match a fresh
    /// compute over the same source.
    #[test]
    #[expect(clippy::too_many_lines)] // long diagnostic-table dispatch
    fn facts_round_trip_through_the_payload() {
        let strings = StringTable::new();
        let mut files = FileTable::new();
        let target = files.intern(Path::new("/w/lib/other.dart"));
        let declared = files.intern(Path::new("/w/lib/main.dart"));
        let show_name = strings.intern("showOnly");
        let hide_name = strings.intern("hideMe");
        let prefix = strings.intern("p");
        let range = SourceRange {
            offset: 4,
            length: 10,
        };
        let directives = vec![DirectiveFacts {
            kind: DirectiveKind::Import,
            uri: Some("package:other/other.dart".into()),
            uri_span: range,
            target: ResolvedTarget::Resolved(target),
            code: None,
            prefix: Some(prefix),
            deferred: true,
            combinators: Combinators::new(
                [
                    CombinatorClause::Show(Box::new([show_name])),
                    CombinatorClause::Hide(Box::new([hide_name])),
                ],
                &strings,
            ),
            name: None,
            configurations: Box::default(),
            configuration_spans: Box::default(),
            interpolated_configurations: Box::default(),
        }];
        let diagnostics = vec![Arc::new(Diagnostic::new(
            Severity::Error,
            CodeRegistry::builtin()
                .get("uri_does_not_exist")
                .expect("registry code"),
            "Target of URI doesn't exist: 'package:other/other.dart'.",
            declared,
            range,
            None,
        ))];
        let source = "import 'package:other/other.dart';\nvoid main() {}\n";
        let facts = UnitFacts {
            content: content_hash(source.as_bytes()),
            api_signature: ApiSignature::from_bytes([9; 16]),
            directives,
            doc_imports: Vec::new(),
            elements: Arc::new(dartq_semantic::FileElements::default()),
            diagnostics,
            line_index: LineIndex::new(source),
        };

        let payload = PersistedFacts::of(&facts, &strings, &files);
        let bytes = serde_json::to_vec(&payload).expect("serialize");
        let decoded: PersistedFacts = serde_json::from_slice(&bytes).expect("deserialize");

        // Restore into fresh tables — a new process's world. One registry
        // for the whole load (the per-entry build is the retired
        // anti-pattern).
        let codes = CodeRegistry::builtin();
        let restored_strings = StringTable::new();
        let mut restored_files = FileTable::new();
        let restored = decoded.materialize(
            source,
            facts.content,
            &restored_strings,
            &codes,
            &mut |path| restored_files.intern(path),
        );

        assert_eq!(restored.content, facts.content, "content hash reproduced");
        assert_eq!(restored.api_signature, facts.api_signature);
        assert_eq!(
            restored.line_index.line_count(),
            facts.line_index.line_count()
        );

        // The file ids live in fresh tables; compare diagnostics through
        // the paths they name, the columns directly.
        let original = &facts.diagnostics[0];
        let diagnostic = &restored.diagnostics[0];
        let restored_file = diagnostic.file;
        assert_eq!(
            restored_files.path(restored_file),
            files.path(original.file),
            "diagnostic file path survives the remap"
        );
        assert_eq!(diagnostic.severity, original.severity);
        assert_eq!(diagnostic.code, original.code);
        assert_eq!(diagnostic.message.as_str(), original.message.as_str());
        assert_eq!(diagnostic.range, original.range);
        assert_eq!(
            diagnostic.correction.as_deref(),
            original.correction.as_deref()
        );

        let directive = &restored.directives[0];
        assert_eq!(directive.kind, DirectiveKind::Import);
        assert_eq!(directive.uri.as_deref(), facts.directives[0].uri.as_deref());
        assert_eq!(directive.uri_span, range);
        // The file id differs (fresh table); the path it names must not.
        let ResolvedTarget::Resolved(restored_target) = directive.target else {
            panic!("file target restored");
        };
        assert_eq!(
            restored_files.path(restored_target),
            files.path(target),
            "target path survives the remap"
        );
        assert_eq!(
            directive.prefix.map(|id| restored_strings.resolve(id)),
            Some("p")
        );
        assert!(directive.deferred);
        // Combinators behave identically: resolve the names against the
        // restored table (fresh intern indexes) and probe both faces.
        let restored_show = restored_strings.intern("showOnly");
        let restored_hide = restored_strings.intern("hideMe");
        assert!(
            directive.combinators.allows(restored_show),
            "shown name passes"
        );
        assert!(
            !directive.combinators.allows(restored_hide),
            "hidden name filtered"
        );
        assert!(directive.combinators.has_show());
        assert_eq!(
            directive.combinators.has_show(),
            facts.directives[0].combinators.has_show()
        );
    }

    /// The `contextMessages` payload (#1821): message text, the referenced
    /// file's index into the facts' path table, and the UTF-16 span all
    /// survive the round trip — `None` rows ride an explicit `null` (the
    /// positional codec writes every field, #4207), while `Some` rows ride
    /// their elements' columns verbatim. A corrupt inner file index drops
    /// just that element (under-report, never a crash) — the parent
    /// diagnostic survives.
    #[test]
    fn diagnostic_context_round_trips_across_the_payload() {
        let strings = StringTable::new();
        let mut files = FileTable::new();
        let reported = files.intern(Path::new("/w/lib/main.dart"));
        let context_file = files.intern(Path::new("/w/lib/dep.dart"));

        let with_context = Diagnostic::new(
            Severity::Warning,
            CodeRegistry::builtin()
                .get("dead_code")
                .expect("registry code"),
            "Dead code.",
            reported,
            SourceRange {
                offset: 4,
                length: 9,
            },
            None,
        )
        .with_context(vec![ContextMessage {
            message: "The declaration is here.".into(),
            file: context_file,
            range: SourceRange {
                offset: 12,
                length: 6,
            },
        }]);
        let without_context = Diagnostic::new(
            Severity::Error,
            CodeRegistry::builtin()
                .get("uri_does_not_exist")
                .expect("registry code"),
            "Target of URI doesn't exist: 'package:a/a.dart'.",
            reported,
            SourceRange {
                offset: 0,
                length: 18,
            },
            None,
        );

        let source = "import 'package:a/a.dart';\nvoid f() {}\n";
        let facts = UnitFacts {
            content: content_hash(source.as_bytes()),
            api_signature: ApiSignature::from_bytes([3; 16]),
            directives: Vec::new(),
            doc_imports: Vec::new(),
            elements: Arc::new(dartq_semantic::FileElements::default()),
            diagnostics: vec![Arc::new(with_context), Arc::new(without_context)],
            line_index: LineIndex::new(source),
        };

        let payload = PersistedFacts::of(&facts, &strings, &files);
        let bytes = serde_json::to_vec(&payload).expect("serialize");
        let decoded: PersistedFacts = serde_json::from_slice(&bytes).expect("deserialize");

        // The no-context row carries an explicit null — the positional
        // codec never omits a field (#4207).
        let contextless_bytes = serde_json::to_string(&decoded.diagnostics[1]).expect("re-encode");
        assert!(
            contextless_bytes.contains("\"context\":null"),
            "None rides an explicit null: {contextless_bytes}"
        );
        let context_bytes =
            serde_json::to_string(&decoded.diagnostics[0]).expect("re-encode context row");
        assert!(
            context_bytes.contains("\"context\""),
            "Some persists the context key: {context_bytes}"
        );

        let codes = CodeRegistry::builtin();
        let restored_strings = StringTable::new();
        let mut restored_files = FileTable::new();
        let restored = decoded.materialize(
            source,
            facts.content,
            &restored_strings,
            &codes,
            &mut |path| restored_files.intern(path),
        );
        assert_eq!(restored.diagnostics.len(), 2);
        let messages = restored.diagnostics[0]
            .context
            .as_deref()
            .expect("context restored");
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].message.as_str(), "The declaration is here.");
        assert_eq!(
            restored_files.path(messages[0].file),
            Some(Path::new("/w/lib/dep.dart")),
            "the context file's path survives the remap"
        );
        assert_eq!(
            messages[0].range,
            SourceRange {
                offset: 12,
                length: 6
            }
        );
        assert!(
            restored.diagnostics[1].context.is_none(),
            "an empty context stays absent on restore"
        );
    }

    /// A `context` row whose stored file index is out of range is disk rot:
    /// serde decodes it, only the loader can see it. The offending element
    /// drops while the parent diagnostic survives — under-report, never a
    /// crash, the same posture as a corrupt top-level `file` index dropping
    /// its own row one layer down.
    #[test]
    fn corrupt_context_file_index_drops_the_element_only() {
        let strings = StringTable::new();
        let payload = PersistedFacts {
            api_signature: [7; 16],
            directives: Vec::new(),
            doc_imports: Vec::new(),
            elements: PersistedElements::of(&dartq_semantic::FileElements::default(), &strings),
            diagnostics: vec![PersistedDiagnostic {
                severity: PersistedSeverity::Warning,
                code_name: "dead_code".to_owned(),
                code_machine: "DEAD_CODE".to_owned(),
                code_dtype: PersistedDiagnosticType::StaticWarning,
                code_published: true,
                message: "Dead code.".to_owned(),
                file: 0,
                range: PersistedRange::of(SourceRange {
                    offset: 4,
                    length: 9,
                }),
                correction: None,
                context: Some(vec![
                    PersistedContextMessage {
                        message: "kept".to_owned(),
                        file: 0,
                        range: PersistedRange::of(SourceRange {
                            offset: 1,
                            length: 2,
                        }),
                    },
                    PersistedContextMessage {
                        message: "dropped".to_owned(),
                        file: 7,
                        range: PersistedRange::of(SourceRange {
                            offset: 0,
                            length: 1,
                        }),
                    },
                ]),
            }],
            files: vec![PathBuf::from("/w/lib/main.dart")],
        };

        let codes = CodeRegistry::builtin();
        let restored_strings = StringTable::new();
        let mut restored_files = FileTable::new();
        let restored = payload.materialize(
            "void f() {}\n",
            content_hash(b"void f() {}\n"),
            &restored_strings,
            &codes,
            &mut |path| restored_files.intern(path),
        );

        assert_eq!(restored.diagnostics.len(), 1, "parent survives");
        let context = restored.diagnostics[0]
            .context
            .as_deref()
            .expect("one element survives");
        assert_eq!(context.len(), 1, "the corrupt element drops");
        assert_eq!(context[0].message.as_str(), "kept");
    }

    /// The written combinator sequence survives the payload round trip
    /// (#539): written order, duplicates (one name written twice → two
    /// tokens with distinct spans), and the captured UTF-16 spans — the
    /// warm face equals the cold capture's (`Combinators::with_spans`,
    /// the l1.rs `walk_import_export` posture), which is the sequence the
    /// undefined-combinator reports iterate.
    #[test]
    #[expect(clippy::too_many_lines)] // long diagnostic-table dispatch
    fn combinator_written_sequence_round_trips_with_spans() {
        let strings = StringTable::new();
        let mut files = FileTable::new();
        let target = files.intern(Path::new("/w/lib/other.dart"));
        let alpha = strings.intern("alpha");
        let beta = strings.intern("beta");
        // The cold capture of `show alpha, alpha, beta hide alpha;` (the
        // name spans are token-accurate UTF-16 offsets): the duplicated
        // `alpha` carries two distinct anchors.
        let clause_input: Vec<(CombinatorClause, Box<[SourceRange]>)> = vec![
            (
                CombinatorClause::Show(Box::new([alpha, alpha, beta])),
                Box::new([
                    SourceRange {
                        offset: 39,
                        length: 5,
                    },
                    SourceRange {
                        offset: 46,
                        length: 5,
                    },
                    SourceRange {
                        offset: 53,
                        length: 4,
                    },
                ]),
            ),
            (
                CombinatorClause::Hide(Box::new([alpha])),
                Box::new([SourceRange {
                    offset: 63,
                    length: 5,
                }]),
            ),
        ];
        let combinators = Combinators::with_spans(clause_input, &strings);
        let directives = vec![DirectiveFacts {
            kind: DirectiveKind::Import,
            uri: Some("package:other/other.dart".into()),
            uri_span: SourceRange {
                offset: 7,
                length: 26,
            },
            target: ResolvedTarget::Resolved(target),
            code: None,
            prefix: None,
            deferred: false,
            combinators,
            name: None,
            configurations: Box::default(),
            configuration_spans: Box::default(),
            interpolated_configurations: Box::default(),
        }];
        let source = "import 'package:other/other.dart' show alpha, alpha, beta hide alpha;\n";
        let facts = UnitFacts {
            content: content_hash(source.as_bytes()),
            api_signature: ApiSignature::from_bytes([5; 16]),
            directives,
            doc_imports: Vec::new(),
            elements: Arc::new(dartq_semantic::FileElements::default()),
            diagnostics: Vec::new(),
            line_index: LineIndex::new(source),
        };

        let payload = PersistedFacts::of(&facts, &strings, &files);
        let bytes = serde_json::to_vec(&payload).expect("serialize");
        let decoded: PersistedFacts = serde_json::from_slice(&bytes).expect("deserialize");

        let codes = CodeRegistry::builtin();
        let restored_strings = StringTable::new();
        let mut restored_files = FileTable::new();
        let restored = decoded.materialize(
            source,
            facts.content,
            &restored_strings,
            &codes,
            &mut |path| restored_files.intern(path),
        );

        let written = |facts: &UnitFacts, strings: &StringTable| {
            facts.directives[0]
                .combinators
                .written()
                .map(|(name, hidden, span)| (strings.resolve(name).to_owned(), hidden, span))
                .collect::<Vec<_>>()
        };
        let cold = written(&facts, &strings);
        let warm = written(&restored, &restored_strings);
        assert_eq!(
            warm, cold,
            "the warm written face equals the cold capture's: order, \
             duplicates, and spans intact"
        );
        assert_eq!(
            warm,
            vec![
                (
                    "alpha".to_owned(),
                    false,
                    SourceRange {
                        offset: 39,
                        length: 5
                    }
                ),
                (
                    "alpha".to_owned(),
                    false,
                    SourceRange {
                        offset: 46,
                        length: 5
                    }
                ),
                (
                    "beta".to_owned(),
                    false,
                    SourceRange {
                        offset: 53,
                        length: 4
                    }
                ),
                (
                    "alpha".to_owned(),
                    true,
                    SourceRange {
                        offset: 63,
                        length: 5
                    }
                ),
            ],
            "the duplicated name rides as two tokens at distinct anchors"
        );

        // The decision faces behave identically on the restored clause
        // sets (`hide alpha` rejects it despite the show listing).
        let restored_alpha = restored_strings.intern("alpha");
        let restored_beta = restored_strings.intern("beta");
        assert!(!restored.directives[0].combinators.allows(restored_alpha));
        assert!(restored.directives[0].combinators.allows(restored_beta));
        assert!(restored.directives[0].combinators.has_show());
    }

    /// A disk payload whose combinator columns desync (JSON-valid, so only
    /// the loader can see it) degrades to the empty combinator face instead
    /// of tripping `Combinators::with_spans`' capture-bug assert (#554):
    /// under-report, never a crash — the directive behaves as if it carried
    /// no show/hide clauses at all.
    #[test]
    fn desynced_combinator_payload_degrades_instead_of_panicking() {
        let strings = StringTable::new();
        let mut files = FileTable::new();
        let target = files.intern(Path::new("/w/lib/other.dart"));
        let alpha = strings.intern("alpha");
        let spans: Box<[SourceRange]> = Box::new([SourceRange {
            offset: 40,
            length: 5,
        }]);
        let combinators = Combinators::with_spans(
            vec![(CombinatorClause::Hide(Box::new([alpha])), spans)],
            &strings,
        );
        let directives = vec![DirectiveFacts {
            kind: DirectiveKind::Import,
            uri: Some("package:other/other.dart".into()),
            uri_span: SourceRange {
                offset: 7,
                length: 26,
            },
            target: ResolvedTarget::Resolved(target),
            code: None,
            prefix: None,
            deferred: false,
            combinators,
            name: None,
            configurations: Box::default(),
            configuration_spans: Box::default(),
            interpolated_configurations: Box::default(),
        }];
        let source = "import 'package:other/other.dart' hide alpha;\n";
        let facts = UnitFacts {
            content: content_hash(source.as_bytes()),
            api_signature: ApiSignature::from_bytes([5; 16]),
            directives,
            doc_imports: Vec::new(),
            elements: Arc::new(dartq_semantic::FileElements::default()),
            diagnostics: Vec::new(),
            line_index: LineIndex::new(source),
        };

        let payload = PersistedFacts::of(&facts, &strings, &files);
        let bytes = serde_json::to_vec(&payload).expect("serialize");
        let mut decoded: PersistedFacts = serde_json::from_slice(&bytes).expect("deserialize");

        // Corrupt the payload the way disk rot would: drop the span column
        // under a non-empty name column. Serde accepts it; only the
        // loader's length pairing can notice.
        decoded.directives[0].combinators[0].name_spans.clear();

        let codes = CodeRegistry::builtin();
        let restored_strings = StringTable::new();
        let mut restored_files = FileTable::new();
        let restored = decoded.materialize(
            source,
            facts.content,
            &restored_strings,
            &codes,
            &mut |path| restored_files.intern(path),
        );

        let combinators = &restored.directives[0].combinators;
        assert!(
            !combinators.has_show(),
            "degraded to the empty face: no clause data survives a desynced payload"
        );
        let hidden_before = restored_strings.intern("alpha");
        let never_listed = restored_strings.intern("absent");
        assert!(
            combinators.allows(hidden_before) && combinators.allows(never_listed),
            "the empty face filters nothing — the corrupt hide clause is dropped whole, \
             not truncated into a face no source spelling could produce"
        );
    }

    /// The version-bump contract (#539, the #512 precedent):
    /// `FACTS_SHAPE_VERSION` folds into `FACTS_SEMANTIC_VERSION`, which
    /// keys the generation header's directory hash — a payload written by
    /// an older shape cannot even be named by a newer engine's load. The
    /// miss is clean (the accelerator invariant): nothing is staged, the
    /// old generation stays intact for a same-version reader, and the warm
    /// start recomputes from source, yielding the spanned written face the
    /// new shape persists.
    #[test]
    #[expect(clippy::too_many_lines)] // long diagnostic-table dispatch
    fn warm_start_misses_cleanly_on_a_schema_version_change() {
        let config_dir = graph_config_dir();
        let root = config_dir.parent().expect("fixture root").to_path_buf();
        let main_path = root.join("lib/main.dart");
        let other_path = root.join("lib/other.dart");
        let main_src = "import 'other.dart' show alpha, alpha, beta hide alpha;\nvoid main() { alpha(); beta(); }\n";
        let other_src = "int alpha() => 1;\nint beta() => 2;\n";

        let pub_config = Arc::new(PackageConfig::parse(GRAPH_CONFIG).expect("valid config"));
        let build_engine = || {
            Engine::new(EngineConfig {
                store: memory_store(),
                sdk: sdk_snapshot("3.13.1\n", None, SDK_LIBRARIES),
                packages: graph_table(&config_dir, GRAPH_CONFIG, GRAPH_LOCK_EMPTY),
                pub_config: Arc::clone(&pub_config),
                config_dir: config_dir.clone(),
                context_roots: Vec::new(),
                semdiag: None,
                target: "vm".to_owned(),
                declared_variables: FxHashMap::default(),
                experiments: Vec::new(),
            })
            .expect("engine")
        };
        let mut saved_engine = build_engine();
        saved_engine.set_content(&main_path, main_src.as_bytes());
        saved_engine.set_content(&other_path, other_src.as_bytes());
        let mut on_event = |_event: EngineEvent| {};
        let cancel = CancellationToken::new();
        saved_engine
            .analyze(&[], &mut on_event, &cancel)
            .expect("analyze");

        let cache = snapshot_cache_dir();
        let store = SnapshotStore::open(SnapshotConfig {
            root: Some(cache.clone()),
            capacity_bytes: 64 * 1024 * 1024,
        })
        .expect("open store");
        assert_eq!(saved_engine.save_facts_snapshot(&store).expect("save"), 2);

        let header = saved_engine.snapshot_header();
        assert!(
            store.load::<PersistedFacts>(&header).is_some(),
            "the saved generation loads under its own header"
        );
        let mut moved = header.clone();
        moved.semantic_version += 1;
        assert!(
            store.load::<PersistedFacts>(&moved).is_none(),
            "a schema-version change forks the generation hash — the old \
             payload is unreachable, the documented invalidation path"
        );

        // The warm start therefore recomputes from source, and the fresh
        // capture produces the spanned written face the new shape carries.
        let mut warm_engine = build_engine();
        let main_warm = warm_engine.set_content(&main_path, main_src.as_bytes());
        warm_engine
            .analyze(&[], &mut on_event, &cancel)
            .expect("analyze");
        let facts = warm_engine.file_facts(main_warm).expect("resident");
        let written: Vec<(String, bool, SourceRange)> = facts.directives[0]
            .combinators
            .written()
            .map(|(name, hidden, span)| {
                (
                    warm_engine.state.strings.resolve(name).to_owned(),
                    hidden,
                    span,
                )
            })
            .collect();
        assert_eq!(
            written,
            vec![
                (
                    "alpha".to_owned(),
                    false,
                    SourceRange {
                        offset: 25,
                        length: 5
                    }
                ),
                (
                    "alpha".to_owned(),
                    false,
                    SourceRange {
                        offset: 32,
                        length: 5
                    }
                ),
                (
                    "beta".to_owned(),
                    false,
                    SourceRange {
                        offset: 39,
                        length: 4
                    }
                ),
                (
                    "alpha".to_owned(),
                    true,
                    SourceRange {
                        offset: 49,
                        length: 5
                    }
                ),
            ],
            "the recompute (the version miss's outcome) yields the spanned \
             written sequence"
        );

        let _ = std::fs::remove_dir_all(&cache);
    }

    /// The source the `clause_boundaries_survive_the_payload` round-trip
    /// helper analyzes (one import; the combinators ride the fixture
    /// directives, not this text).
    const CLAUSE_ROUND_TRIP_SOURCE: &str = "import 'package:other/other.dart';\n";

    /// Round-trips one import directive whose combinators are `clauses`
    /// (each: hidden flag, name texts, per-name spans) through the payload,
    /// returning the cold and the restored `Combinators` plus the payload's
    /// JSON encoding. Each call builds fresh tables, and the cold capture
    /// and the restore intern the identical name set in the identical
    /// order, so [`Combinators::written_shapes_equal`]'s interned-id
    /// comparison is exact across the pair.
    fn clause_round_trip(
        clauses: &[(bool, &[&str], &[SourceRange])],
    ) -> (Combinators, Combinators, String) {
        let strings = StringTable::new();
        let mut files = FileTable::new();
        let target = files.intern(Path::new("/w/lib/other.dart"));
        let to_clauses = || {
            clauses
                .iter()
                .map(|(hidden, names, spans)| {
                    let names: Box<[StringId]> =
                        names.iter().map(|name| strings.intern(name)).collect();
                    let clause = if *hidden {
                        CombinatorClause::Hide(names)
                    } else {
                        CombinatorClause::Show(names)
                    };
                    (clause, spans.to_vec().into_boxed_slice())
                })
                .collect::<Vec<_>>()
        };
        let cold = Combinators::with_spans(to_clauses(), &strings);
        let facts = UnitFacts {
            content: content_hash(CLAUSE_ROUND_TRIP_SOURCE.as_bytes()),
            api_signature: ApiSignature::from_bytes([5; 16]),
            directives: vec![DirectiveFacts {
                kind: DirectiveKind::Import,
                uri: Some("package:other/other.dart".into()),
                uri_span: SourceRange {
                    offset: 7,
                    length: 26,
                },
                target: ResolvedTarget::Resolved(target),
                code: None,
                prefix: None,
                deferred: false,
                combinators: Combinators::with_spans(to_clauses(), &strings),
                name: None,
                configurations: Box::default(),
                configuration_spans: Box::default(),
                interpolated_configurations: Box::default(),
            }],
            doc_imports: Vec::new(),
            elements: Arc::new(dartq_semantic::FileElements::default()),
            diagnostics: Vec::new(),
            line_index: LineIndex::new(CLAUSE_ROUND_TRIP_SOURCE),
        };

        let payload = PersistedFacts::of(&facts, &strings, &files);
        let json = serde_json::to_string(&payload).expect("serialize");
        let decoded: PersistedFacts = serde_json::from_str(&json).expect("deserialize");
        let codes = CodeRegistry::builtin();
        let restored_strings = StringTable::new();
        let mut restored_files = FileTable::new();
        let restored = decoded.materialize(
            CLAUSE_ROUND_TRIP_SOURCE,
            facts.content,
            &restored_strings,
            &codes,
            &mut |path| restored_files.intern(path),
        );
        let warm = restored
            .directives
            .into_iter()
            .next()
            .expect("one directive")
            .combinators;
        (cold, warm, json)
    }

    /// Clause boundaries survive the payload (#719): `hide alpha hide alpha`
    /// (two clauses) and `hide alpha, alpha` (one clause) — likewise
    /// `show alpha show beta` and `show alpha, beta` — serialize to
    /// different payloads and restore to `Combinators` whose written shapes
    /// match only their own cold face (`written_shapes_equal` compares
    /// clause ids, the per-clause seen-set boundary of the
    /// duplicate-combinator check). The pre-#719 merged-run payload
    /// collapsed both spellings of each pair, so a warm start over-reported
    /// duplicate combinators and equated the paired imports.
    #[test]
    fn clause_boundaries_survive_the_payload() {
        let span = |offset, length| SourceRange { offset, length };

        let (hide_split, hide_split_warm, hide_split_json) = clause_round_trip(&[
            (true, &["alpha"], &[span(39, 5)]),
            (true, &["alpha"], &[span(57, 5)]),
        ]);
        let (hide_grouped, hide_grouped_warm, hide_grouped_json) =
            clause_round_trip(&[(true, &["alpha", "alpha"], &[span(39, 5), span(57, 5)])]);

        assert_ne!(
            hide_split_json, hide_grouped_json,
            "one payload element per source clause: the two spellings must \
             not collapse into the same bytes"
        );
        assert!(
            !hide_split.written_shapes_equal(&hide_grouped),
            "fixture sanity: the cold faces are different shapes"
        );
        assert!(
            hide_split_warm.written_shapes_equal(&hide_split),
            "the warm face reproduces the cold two-clause shape"
        );
        assert!(
            hide_grouped_warm.written_shapes_equal(&hide_grouped),
            "the warm face reproduces the cold one-clause shape"
        );
        assert!(
            !hide_split_warm.written_shapes_equal(&hide_grouped)
                && !hide_grouped_warm.written_shapes_equal(&hide_split)
                && !hide_split_warm.written_shapes_equal(&hide_grouped_warm),
            "`hide alpha hide alpha` and `hide alpha, alpha` stay different \
             shapes across the round trip, in both directions"
        );

        let (show_split, show_split_warm, _) = clause_round_trip(&[
            (false, &["alpha"], &[span(39, 5)]),
            (false, &["beta"], &[span(57, 4)]),
        ]);
        let (show_grouped, show_grouped_warm, _) =
            clause_round_trip(&[(false, &["alpha", "beta"], &[span(39, 5), span(57, 4)])]);

        assert!(
            !show_split.written_shapes_equal(&show_grouped),
            "fixture sanity: `show alpha show beta` and `show alpha, beta` \
             are different cold shapes"
        );
        assert!(
            show_split_warm.written_shapes_equal(&show_split)
                && show_grouped_warm.written_shapes_equal(&show_grouped),
            "each warm show face reproduces its own cold clause structure"
        );
        assert!(
            !show_split_warm.written_shapes_equal(&show_grouped)
                && !show_grouped_warm.written_shapes_equal(&show_split)
                && !show_split_warm.written_shapes_equal(&show_grouped_warm),
            "`show alpha show beta` and `show alpha, beta` stay different \
             shapes across the round trip — the duplicate-directive identity \
             must not equate them after a warm start"
        );
    }

    /// The full warm-start seam for the clause-boundary invariant (#719):
    /// for a file whose imports stress adjacent same-kind clauses
    /// (`hide alpha hide alpha`; `show alpha show beta` next to
    /// `show alpha, beta`), the warm facts reproduce the cold combinator
    /// shapes on every directive and the cold diagnostics row-for-row.
    /// The two duplicate-clause imports legitimately carry one
    /// `multiple_combinators` row each (the oracle reports both, verified
    /// against `dart analyze`); the warm start must reproduce exactly that
    /// pair — never a duplicate-combinator or duplicate-import row beyond
    /// it. The merged-run payload collapsed the boundaries, so a warm start
    /// over-reported the duplicates and equated the paired imports.
    #[test]
    #[expect(clippy::too_many_lines)] // long diagnostic-table dispatch
    fn warm_start_reproduces_cold_clause_boundary_faces() {
        let config_dir = graph_config_dir();
        let root = config_dir.parent().expect("fixture root").to_path_buf();
        let main_path = root.join("lib/main.dart");
        let other_path = root.join("lib/other.dart");
        let main_src = concat!(
            "import 'other.dart' hide alpha hide alpha;\n",
            "import 'other.dart' show alpha show beta;\n",
            "import 'other.dart' show alpha, beta;\n",
            "void main() { alpha(); beta(); }\n",
        );
        let other_src = "int alpha() => 1;\nint beta() => 2;\n";

        let pub_config = Arc::new(PackageConfig::parse(GRAPH_CONFIG).expect("valid config"));
        let build_engine = || {
            Engine::new(EngineConfig {
                store: memory_store(),
                sdk: sdk_snapshot("3.13.1\n", None, SDK_LIBRARIES),
                packages: graph_table(&config_dir, GRAPH_CONFIG, GRAPH_LOCK_EMPTY),
                pub_config: Arc::clone(&pub_config),
                config_dir: config_dir.clone(),
                context_roots: Vec::new(),
                semdiag: None,
                target: "vm".to_owned(),
                declared_variables: FxHashMap::default(),
                experiments: Vec::new(),
            })
            .expect("engine")
        };
        let mut cold = build_engine();
        let main_cold = cold.set_content(&main_path, main_src.as_bytes());
        let _ = cold.set_content(&other_path, other_src.as_bytes());
        let mut on_event = |_event: EngineEvent| {};
        let cancel = CancellationToken::new();
        cold.analyze(&[], &mut on_event, &cancel).expect("analyze");
        // The cold face is the oracle's own: one MULTIPLE_COMBINATORS row
        // per duplicate-clause import, spanning the whole combinator list
        // (`import 'other.dart' ` is 20 columns, so the spans start at
        // offsets 20/63 with lengths 21/20 — verified against
        // `dart analyze --format machine` on this exact file shape).
        let row = |d: &Diagnostic| (d.code.name, d.range.offset, d.range.length, d.severity);
        assert_eq!(
            cold.file_facts(main_cold)
                .expect("resident")
                .diagnostics
                .iter()
                .map(|d| row(d))
                .collect::<Vec<_>>(),
            vec![
                ("multiple_combinators", 20, 21, Severity::Warning),
                ("multiple_combinators", 63, 20, Severity::Warning),
            ],
            "fixture sanity: exactly the oracle's two MULTIPLE_COMBINATORS rows"
        );

        let cache = snapshot_cache_dir();
        let store = SnapshotStore::open(SnapshotConfig {
            root: Some(cache.clone()),
            capacity_bytes: 64 * 1024 * 1024,
        })
        .expect("open store");
        assert_eq!(
            cold.save_facts_snapshot(&store).expect("save"),
            2,
            "one entry per distinct content"
        );
        let header = cold.snapshot_header();

        // A fresh process's world: fresh engine, load, ingest, re-apply —
        // every main.dart content hits its snapshot entry.
        let mut warm = build_engine();
        let snapshot = store.load::<PersistedFacts>(&header).expect("generation");
        assert_eq!(warm.ingest_facts_snapshot(snapshot), 2);
        let main_warm = warm.set_content(&main_path, main_src.as_bytes());
        let _ = warm.set_content(&other_path, other_src.as_bytes());
        warm.analyze(&[], &mut on_event, &cancel).expect("analyze");
        assert!(
            warm.state.warm_start.is_empty(),
            "single-shot staging: both matched entries consumed"
        );

        let cold_facts = cold.file_facts(main_cold).expect("resident");
        let warm_facts = warm.file_facts(main_warm).expect("resident");
        assert_eq!(
            warm_facts
                .diagnostics
                .iter()
                .map(|d| row(d))
                .collect::<Vec<_>>(),
            cold_facts
                .diagnostics
                .iter()
                .map(|d| row(d))
                .collect::<Vec<_>>(),
            "the warm start must reproduce the cold rows exactly — no \
             duplicate-combinator or duplicate-import row beyond them, got \
             {:?} vs cold {:?}",
            warm_facts
                .diagnostics
                .iter()
                .map(|diagnostic| diagnostic.code.name)
                .collect::<Vec<_>>(),
            cold_facts
                .diagnostics
                .iter()
                .map(|diagnostic| diagnostic.code.name)
                .collect::<Vec<_>>(),
        );
        assert_eq!(
            cold_facts.directives.len(),
            warm_facts.directives.len(),
            "the same directive list"
        );
        for (index, (cold_directive, warm_directive)) in cold_facts
            .directives
            .iter()
            .zip(warm_facts.directives.iter())
            .enumerate()
        {
            // The shapes compare through each engine's own string table:
            // `written_shapes_equal` compares interned name ids, which are
            // engine-local by design — the cold batch's parallel scan
            // (l1.rs `par_iter`) interns across files in a nondeterministic
            // order, and its only product caller (pipeline.rs's
            // duplicate-directive identity) compares within one engine. The
            // projection compares what the invariant pins: name text,
            // hidden flag, captured span, and clause id per written token.
            let shape = |directive: &DirectiveFacts, strings: &StringTable| {
                directive
                    .combinators
                    .written_with_clauses()
                    .map(|(name, hidden, span, clause)| {
                        (strings.resolve(name).to_owned(), hidden, span, clause)
                    })
                    .collect::<Vec<_>>()
            };
            assert_eq!(
                shape(cold_directive, &cold.state.strings),
                shape(warm_directive, &warm.state.strings),
                "directive #{index}: the warm written combinator shape \
                 diverges from the cold capture"
            );
        }

        let _ = std::fs::remove_dir_all(&cache);
    }

    /// The #4199 cross-session run replay, end to end: a producer engine's
    /// completed run lands in the content store (the post-analyze sweep),
    /// a fresh engine over the same workspace — same snapshot staging, so
    /// the warm start claims the facts — adopts the stored run through the
    /// pipeline's standing reuse gate (`from_cache: true` on the target
    /// library) and returns the producer's exact rows, and a content edit
    /// that moves the file's face reverts that library to a full recompute
    /// (`from_cache: false`) — the legitimate-miss set, nothing broader.
    #[test]
    fn library_runs_replay_across_engine_instances_and_gate_on_face_moves() {
        let root = bare_workspace("run-replay");
        // One SDK root for every engine: the run key folds the grants
        // fingerprint, which folds the SDK lib root — per-engine synthetic
        // SDKs would fork the key and the test would pin a miss.
        let sdk_root = std::env::temp_dir().join(format!(
            "dartq-engine-run-replay-sdk-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&sdk_root);
        std::fs::create_dir_all(sdk_root.join("lib/_internal")).expect("sdk dirs");
        std::fs::write(sdk_root.join("version"), "3.13.1\n").expect("version");
        std::fs::write(sdk_root.join("lib/libraries.json"), SDK_LIBRARIES).expect("libraries");
        std::fs::write(
            sdk_root.join("lib/_internal/allowed_experiments.json"),
            r#"{"version": 1, "experimentSets": {"sdkExperiments": []}, "sdk": {"default": {"experimentSet": "sdkExperiments"}}, "packages": {}}"#,
        )
        .expect("experiments");
        // A face-carrying fixture: the parse-error row rides the payload, so
        // the replay assertion below proves rows survived, not just the key.
        let source = "void main() { var x = ; }\n";
        let main_path = root.join("lib/main.dart");
        let snap_root = snapshot_cache_dir();
        let store_root = snapshot_cache_dir();

        let mut producer = bare_engine_over_sdk(&root, &sdk_root);
        producer
            .open_content_store(&crate::store::StoreConfig {
                root: store_root.clone(),
                size_limit: 16 << 20,
            })
            .expect("open store");
        let main_cold = producer.set_content(&main_path, source.as_bytes());
        let mut cold_recomputes = 0;
        let mut on_event = |event: EngineEvent| {
            if let EngineEvent::LibraryAnalyzed {
                from_cache: false, ..
            } = event
            {
                cold_recomputes += 1;
            }
        };
        let cancel = CancellationToken::new();
        let cold_rows = producer
            .analyze(&[main_cold], &mut on_event, &cancel)
            .expect("analyze");
        assert!(
            !cold_rows.is_empty(),
            "fixture sanity: the parse-error file carries rows"
        );
        let snapshot_store = SnapshotStore::open(SnapshotConfig {
            root: Some(snap_root.clone()),
            capacity_bytes: 64 * 1024 * 1024,
        })
        .expect("open snapshot store");
        let saved = producer
            .save_facts_snapshot(&snapshot_store)
            .expect("save snapshot");
        assert_eq!(saved, 1, "one entry: one file, one content");
        let header = producer.snapshot_header();
        drop(producer);

        // The cross-session hit: fresh process shape — same store root,
        // staged snapshot, identical bytes.
        let mut warm = bare_engine_over_sdk(&root, &sdk_root);
        warm.open_content_store(&crate::store::StoreConfig {
            root: store_root.clone(),
            size_limit: 16 << 20,
        })
        .expect("open store");
        let snapshot = snapshot_store
            .load::<PersistedFacts>(&header)
            .expect("generation");
        assert_eq!(warm.ingest_facts_snapshot(snapshot), 1);
        let main_warm = warm.set_content(&main_path, source.as_bytes());
        let mut warm_hits = 0;
        let mut warm_recomputes = 0;
        let mut on_event = |event: EngineEvent| {
            if let EngineEvent::LibraryAnalyzed { from_cache, .. } = event {
                if from_cache {
                    warm_hits += 1;
                } else {
                    warm_recomputes += 1;
                }
            }
        };
        let warm_rows = warm
            .analyze(&[main_warm], &mut on_event, &cancel)
            .expect("analyze");
        assert_eq!(
            warm_hits, 1,
            "the replayed run stands: the reuse gate serves the target \
             library from the adopted result"
        );
        assert_eq!(warm_recomputes, 0, "nothing recomputed on the hit turn");
        let row = |d: &Diagnostic| (d.code.name, d.range.offset, d.range.length, d.severity);
        assert_eq!(
            warm_rows.iter().map(&row).collect::<Vec<_>>(),
            cold_rows.iter().map(&row).collect::<Vec<_>>(),
            "the replayed rows equal the cold pass's, row for row"
        );

        // The cross-session miss: same store, staged snapshot, but the file
        // content moved — the claim fails, the face moves, the run key forks,
        // and the library recomputes cold.
        let mut edited = bare_engine_over_sdk(&root, &sdk_root);
        edited
            .open_content_store(&crate::store::StoreConfig {
                root: store_root.clone(),
                size_limit: 16 << 20,
            })
            .expect("open store");
        let snapshot = snapshot_store
            .load::<PersistedFacts>(&header)
            .expect("generation");
        assert_eq!(edited.ingest_facts_snapshot(snapshot), 1);
        let main_edited =
            edited.set_content(&main_path, b"void main() { var x = 1; }\n");
        let mut edited_hits = 0;
        let mut edited_recomputes = 0;
        let mut on_event = |event: EngineEvent| {
            if let EngineEvent::LibraryAnalyzed { from_cache, .. } = event {
                if from_cache {
                    edited_hits += 1;
                } else {
                    edited_recomputes += 1;
                }
            }
        };
        let edited_rows = edited
            .analyze(&[main_edited], &mut on_event, &cancel)
            .expect("analyze");
        assert_eq!(
            edited_hits, 0,
            "a moved face never adopts: no cache hit on the miss turn"
        );
        assert_eq!(
            edited_recomputes, 1,
            "the library recomputes — the legitimate-miss set"
        );
        assert!(
            edited_rows.is_empty(),
            "the edited content is clean; the miss degrades to a full \
             recompute, not stale rows"
        );

        let _ = std::fs::remove_dir_all(&snap_root);
        let _ = std::fs::remove_dir_all(&store_root);
        let _ = std::fs::remove_dir_all(&sdk_root);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A pubspec-less workspace with a caller-chosen `lib/` file set — the
    /// [`bare_workspace`] shape for the multi-library targeted-turn pins.
    fn bare_workspace_with(tag: &str, files: &[(&str, &str)]) -> PathBuf {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let root = std::env::temp_dir().join(format!(
            "dartq-engine-targeted-{tag}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("lib")).expect("fixture dir");
        for (name, text) in files {
            std::fs::write(root.join("lib").join(name), text).expect("fixture file");
        }
        root
    }

    /// One synthetic SDK root for engines that must share the grants
    /// fingerprint (the run key folds the SDK lib root, so per-engine roots
    /// fork every key and every replay would be a miss).
    fn shared_sdk_root(tag: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "dartq-engine-targeted-sdk-{tag}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("lib/_internal")).expect("sdk dirs");
        std::fs::write(root.join("version"), "3.13.1\n").expect("version");
        std::fs::write(root.join("lib/libraries.json"), SDK_LIBRARIES).expect("libraries");
        std::fs::write(
            root.join("lib/_internal/allowed_experiments.json"),
            r#"{"version": 1, "experimentSets": {"sdkExperiments": []}, "sdk": {"default": {"experimentSet": "sdkExperiments"}}, "packages": {}}"#,
        )
        .expect("experiments");
        root
    }

    /// A diagnostic set projected onto every output-bearing column, with
    /// file ids resolved to basenames so two engines' sets compare (file
    /// ids are process-local).
    fn row_projection(engine: &Engine, rows: &[Diagnostic]) -> Vec<RowProjection> {
        let basename = |file| {
            engine
                .file_table()
                .path(file)
                .and_then(|path| path.file_name())
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default()
        };
        rows.iter()
            .map(|row| {
                (
                    basename(row.file),
                    row.range.offset,
                    row.range.length,
                    row.code.name,
                    row.code.machine,
                    row.severity,
                    row.message.as_str().to_owned(),
                    row.correction.as_deref().map(str::to_owned),
                    row.context.as_ref().map(|context| {
                        context
                            .iter()
                            .map(|message| {
                                (
                                    basename(message.file),
                                    message.range.offset,
                                    message.range.length,
                                    message.message.as_str().to_owned(),
                                )
                            })
                            .collect::<Vec<_>>()
                    }),
                )
            })
            .collect()
    }

    /// The row projection's element shape — spelled out once so the
    /// expected and actual sets share one tuple type.
    type RowProjection = (
        String,
        u32,
        u32,
        &'static str,
        &'static str,
        Severity,
        String,
        Option<String>,
        Option<Vec<(String, u32, u32, String)>>,
    );

    /// #4234: a file-aim ("targeted") warm turn replays only the aim's
    /// library. Before this, the warm single-file face read, decoded,
    /// materialized, surface-installed, and merged all 1,318 cockpit
    /// libraries per turn; a targeted turn adopts the aim library's stored
    /// run and defers the rest, returning exactly the full turn's rows for
    /// the aim file — the rows the CLI's `covered` filter keeps, byte for
    /// byte — while the next untargeted turn drains the deferred
    /// libraries and reproduces the whole set.
    #[test]
    fn targeted_turn_replays_only_the_aim_library_and_restores_the_full_set() {
        let root = bare_workspace_with(
            "aim",
            &[
                ("a.dart", "import 'b.dart';\nvoid a() { b(); var x = ; }\n"),
                ("b.dart", "void b() {}\n"),
                ("c.dart", "void c() { var z = ; }\n"),
            ],
        );
        let sdk_root = shared_sdk_root("aim");
        let snap_root = snapshot_cache_dir();
        let store_root = snapshot_cache_dir();
        let cancel = CancellationToken::new();
        let config = || crate::store::StoreConfig {
            root: store_root.clone(),
            size_limit: 16 << 20,
        };

        // The producer: one cold full turn, every run persisted, the facts
        // saved for the warm shape.
        let mut producer = bare_engine_over_sdk(&root, &sdk_root);
        producer.open_content_store(&config()).expect("open store");
        assert_eq!(producer.discover_workspace().expect("discover").file_count, 3);
        let cold_rows = producer
            .analyze(&[], &mut |_| {}, &cancel)
            .expect("cold analyze");
        let full = row_projection(&producer, &cold_rows);
        assert_eq!(
            full.iter()
                .map(|row| row.0.as_str())
                .collect::<std::collections::BTreeSet<_>>(),
            ["a.dart", "c.dart"].into_iter().collect(),
            "fixture sanity: the parse errors land on the two broken files"
        );
        let snapshot_store = SnapshotStore::open(SnapshotConfig {
            root: Some(snap_root.clone()),
            capacity_bytes: 64 << 20,
        })
        .expect("open snapshot store");
        assert_eq!(
            producer.save_facts_snapshot(&snapshot_store).expect("save"),
            3
        );
        let header = producer.snapshot_header();
        drop(producer);

        // The warm full turn: the whole-store replay the targeted turn must
        // match for the aim, and the full set a later untargeted turn must
        // restore.
        let mut full_engine = bare_engine_over_sdk(&root, &sdk_root);
        full_engine.open_content_store(&config()).expect("open store");
        let snapshot = snapshot_store
            .load::<PersistedFacts>(&header)
            .expect("generation");
        assert_eq!(full_engine.ingest_facts_snapshot(snapshot), 3);
        full_engine.discover_workspace().expect("discover");
        let warm_full_rows = full_engine
            .analyze(&[], &mut |_| {}, &cancel)
            .expect("warm full");
        assert_eq!(
            row_projection(&full_engine, &warm_full_rows),
            full,
            "the warm full turn reproduces the cold set"
        );
        assert_eq!(
            full_engine.last_analyze_stats(),
            (0, 3),
            "every library served from the standing runs"
        );

        // The targeted turn: only a.dart's library replays; b.dart and
        // c.dart defer instead of decoding and merging.
        let mut aim_engine = bare_engine_over_sdk(&root, &sdk_root);
        aim_engine.open_content_store(&config()).expect("open store");
        let snapshot = snapshot_store
            .load::<PersistedFacts>(&header)
            .expect("generation");
        assert_eq!(aim_engine.ingest_facts_snapshot(snapshot), 3);
        aim_engine.discover_workspace().expect("discover");
        let aim = aim_engine
            .file_table()
            .try_file_id(&root.join("lib/a.dart"))
            .expect("aim id");
        let aim_rows = aim_engine
            .analyze(&[aim], &mut |_| {}, &cancel)
            .expect("targeted analyze");
        let expected: Vec<RowProjection> = full
            .iter()
            .filter(|row| row.0 == "a.dart")
            .cloned()
            .collect();
        assert_eq!(
            row_projection(&aim_engine, &aim_rows),
            expected,
            "the targeted turn returns the full turn's aim rows, row for row"
        );
        assert_eq!(
            aim_engine.last_analyze_stats(),
            (0, 1),
            "one library served, nothing recomputed"
        );
        assert_eq!(
            aim_engine.state.results.len(),
            1,
            "only the aim's library was replayed from the store"
        );
        assert_eq!(
            aim_engine.state.slice_deferred.len(),
            2,
            "the other invalidated libraries deferred instead of recomputing"
        );

        // The deferred set rejoins the next untargeted turn: the two
        // libraries replay nothing (staging was consumed) and recompute,
        // and the output is whole again.
        let restored = aim_engine
            .analyze(&[], &mut |_| {}, &cancel)
            .expect("restore");
        assert_eq!(
            row_projection(&aim_engine, &restored),
            full,
            "the full turn after a targeted one is whole"
        );
        assert!(
            aim_engine.state.slice_deferred.is_empty(),
            "the deferral drained"
        );
        assert_eq!(
            aim_engine.state.results.len(),
            3,
            "every live library holds a run again"
        );
        let _ = std::fs::remove_dir_all(&snap_root);
        let _ = std::fs::remove_dir_all(&store_root);
        let _ = std::fs::remove_dir_all(&sdk_root);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The targeted turn's part-file aim: a `part` file resolves through
    /// its defining library (`library_of_file`), so the aim adopts the one
    /// run that carries the part's rows and emits them — the full turn's
    /// part rows, row for row.
    #[test]
    fn targeted_turn_resolves_a_part_file_aim_to_its_library() {
        let root = bare_workspace_with(
            "part",
            &[
                ("host.dart", "part 'piece.dart';\nvoid host() { var x = ; }\n"),
                ("piece.dart", "part of 'host.dart';\nvoid piece() { var y = ; }\n"),
                ("other.dart", "void other() { var z = ; }\n"),
            ],
        );
        let sdk_root = shared_sdk_root("part");
        let snap_root = snapshot_cache_dir();
        let store_root = snapshot_cache_dir();
        let cancel = CancellationToken::new();
        let config = || crate::store::StoreConfig {
            root: store_root.clone(),
            size_limit: 16 << 20,
        };

        let mut producer = bare_engine_over_sdk(&root, &sdk_root);
        producer.open_content_store(&config()).expect("open store");
        producer.discover_workspace().expect("discover");
        let cold_rows = producer
            .analyze(&[], &mut |_| {}, &cancel)
            .expect("cold analyze");
        let full = row_projection(&producer, &cold_rows);
        assert!(
            full.iter().any(|row| row.0 == "piece.dart"),
            "fixture sanity: the part file carries its own rows"
        );
        let snapshot_store = SnapshotStore::open(SnapshotConfig {
            root: Some(snap_root.clone()),
            capacity_bytes: 64 << 20,
        })
        .expect("open snapshot store");
        assert_eq!(
            producer.save_facts_snapshot(&snapshot_store).expect("save"),
            3
        );
        let header = producer.snapshot_header();
        drop(producer);

        let mut aim_engine = bare_engine_over_sdk(&root, &sdk_root);
        aim_engine.open_content_store(&config()).expect("open store");
        let snapshot = snapshot_store
            .load::<PersistedFacts>(&header)
            .expect("generation");
        assert_eq!(aim_engine.ingest_facts_snapshot(snapshot), 3);
        aim_engine.discover_workspace().expect("discover");
        let aim = aim_engine
            .file_table()
            .try_file_id(&root.join("lib/piece.dart"))
            .expect("part id");
        let aim_rows = aim_engine
            .analyze(&[aim], &mut |_| {}, &cancel)
            .expect("targeted analyze");
        // The aim resolves to the DEFINING library, so the turn returns
        // that library's rows — the part's own rows plus its defining
        // file's (the part's diagnostics live in the one run). The
        // caller's `covered` filter keeps the part's rows, and those must
        // equal the full turn's part rows.
        let aimed: Vec<RowProjection> = full
            .iter()
            .filter(|row| row.0 == "piece.dart" || row.0 == "host.dart")
            .cloned()
            .collect();
        let projected = row_projection(&aim_engine, &aim_rows);
        assert_eq!(
            projected, aimed,
            "the part aim serves its defining library's rows"
        );
        assert_eq!(
            projected
                .into_iter()
                .filter(|row| row.0 == "piece.dart")
                .collect::<Vec<_>>(),
            full.iter()
                .filter(|row| row.0 == "piece.dart")
                .cloned()
                .collect::<Vec<_>>(),
            "the covered view (the part's own rows) is the full turn's, row for row"
        );
        assert_eq!(
            aim_engine.state.results.len(),
            1,
            "one library — the part's defining library — replayed"
        );
        assert_eq!(aim_engine.state.slice_deferred.len(), 1, "other.dart deferred");
        let _ = std::fs::remove_dir_all(&snap_root);
        let _ = std::fs::remove_dir_all(&store_root);
        let _ = std::fs::remove_dir_all(&sdk_root);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The targeted turn's fallback: a target with no live library (a path
    /// the analysis never claimed — an options file, a stray id) keeps the
    /// full replay, because such a target's rows are not a library run's
    /// and a slice could drop them.
    #[test]
    fn targeted_turn_falls_back_when_a_target_has_no_library() {
        let root = bare_workspace_with(
            "fallback",
            &[("a.dart", "void a() { var x = ; }\n"), ("b.dart", "void b() { var y = ; }\n")],
        );
        let sdk_root = shared_sdk_root("fallback");
        let snap_root = snapshot_cache_dir();
        let store_root = snapshot_cache_dir();
        let cancel = CancellationToken::new();
        let config = || crate::store::StoreConfig {
            root: store_root.clone(),
            size_limit: 16 << 20,
        };

        let mut producer = bare_engine_over_sdk(&root, &sdk_root);
        producer.open_content_store(&config()).expect("open store");
        producer.discover_workspace().expect("discover");
        let cold_rows = producer
            .analyze(&[], &mut |_| {}, &cancel)
            .expect("cold analyze");
        let full = row_projection(&producer, &cold_rows);
        let snapshot_store = SnapshotStore::open(SnapshotConfig {
            root: Some(snap_root.clone()),
            capacity_bytes: 64 << 20,
        })
        .expect("open snapshot store");
        assert_eq!(
            producer.save_facts_snapshot(&snapshot_store).expect("save"),
            2
        );
        let header = producer.snapshot_header();
        drop(producer);

        let mut aim_engine = bare_engine_over_sdk(&root, &sdk_root);
        aim_engine.open_content_store(&config()).expect("open store");
        let snapshot = snapshot_store
            .load::<PersistedFacts>(&header)
            .expect("generation");
        assert_eq!(aim_engine.ingest_facts_snapshot(snapshot), 2);
        aim_engine.discover_workspace().expect("discover");
        // An interned path no analysis claims: the live-path ceiling seam
        // interns without content or residency.
        aim_engine.note_live_path_ceiling(&root.join("lib/stray.dart"));
        let stray = aim_engine
            .file_table()
            .try_file_id(&root.join("lib/stray.dart"))
            .expect("stray id");
        let rows = aim_engine
            .analyze(&[stray], &mut |_| {}, &cancel)
            .expect("fallback analyze");
        assert_eq!(
            row_projection(&aim_engine, &rows),
            full,
            "the unmapped target keeps the full turn's output"
        );
        assert_eq!(
            aim_engine.state.results.len(),
            2,
            "the full replay adopted both libraries"
        );
        assert!(
            aim_engine.state.slice_deferred.is_empty(),
            "the fallback defers nothing"
        );
        let _ = std::fs::remove_dir_all(&snap_root);
        let _ = std::fs::remove_dir_all(&store_root);
        let _ = std::fs::remove_dir_all(&sdk_root);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Successive targeted turns on ONE warm engine (#4234): the staging a
    /// first targeted turn claims is gone, so the second turn takes the
    /// no-staging arm — the deferred set must requeue and the slice must
    /// retarget, or the second aim's library (no standing result, never
    /// recomputed) silently drops out of the turn's rows. The pin: each
    /// aim answers its own library's rows — the full turn's rows for
    /// that file — and the final untargeted turn restores the whole set.
    #[test]
    fn successive_targeted_turns_each_serve_their_own_aim() {
        let root = bare_workspace_with(
            "chain",
            &[
                ("a.dart", "import 'b.dart';\nvoid a() { b(); var x = ; }\n"),
                ("b.dart", "void b() {}\n"),
                ("c.dart", "void c() { var z = ; }\n"),
            ],
        );
        let sdk_root = shared_sdk_root("chain");
        let snap_root = snapshot_cache_dir();
        let store_root = snapshot_cache_dir();
        let cancel = CancellationToken::new();
        let config = || crate::store::StoreConfig {
            root: store_root.clone(),
            size_limit: 16 << 20,
        };

        let mut producer = bare_engine_over_sdk(&root, &sdk_root);
        producer.open_content_store(&config()).expect("open store");
        producer.discover_workspace().expect("discover");
        let cold_rows = producer
            .analyze(&[], &mut |_| {}, &cancel)
            .expect("cold analyze");
        let full = row_projection(&producer, &cold_rows);
        let snapshot_store = SnapshotStore::open(SnapshotConfig {
            root: Some(snap_root.clone()),
            capacity_bytes: 64 << 20,
        })
        .expect("open snapshot store");
        assert_eq!(
            producer.save_facts_snapshot(&snapshot_store).expect("save"),
            3
        );
        let header = producer.snapshot_header();
        drop(producer);

        let mut engine = bare_engine_over_sdk(&root, &sdk_root);
        engine.open_content_store(&config()).expect("open store");
        let snapshot = snapshot_store
            .load::<PersistedFacts>(&header)
            .expect("generation");
        assert_eq!(engine.ingest_facts_snapshot(snapshot), 3);
        engine.discover_workspace().expect("discover");
        let aim_of = |name: &str| root.join(format!("lib/{name}"));
        let a_id = engine
            .file_table()
            .try_file_id(&aim_of("a.dart"))
            .expect("a id");
        let c_id = engine
            .file_table()
            .try_file_id(&aim_of("c.dart"))
            .expect("c id");

        // Turn one — aim a.dart: the staged replay adopts the aim library
        // alone, deferring the other two.
        let first = engine
            .analyze(&[a_id], &mut |_| {}, &cancel)
            .expect("first targeted");
        assert_eq!(
            row_projection(&engine, &first)
                .into_iter()
                .map(|row| row.0)
                .collect::<Vec<_>>(),
            vec!["a.dart".to_owned()],
            "the first aim answers its own rows"
        );
        assert_eq!(engine.state.slice_deferred.len(), 2);

        // Turn two — aim c.dart on the SAME engine, staging consumed: the
        // deferred set requeues, the slice retargets, and c.dart's library
        // recomputes (no standing result, nothing stored to hit under a
        // consumed staging) instead of vanishing from the output. The
        // returned set is the merge over standing runs — a.dart's rows ride
        // along; the covered view (the caller's filter) is the contract.
        let second = engine
            .analyze(&[c_id], &mut |_| {}, &cancel)
            .expect("second targeted");
        let projected = row_projection(&engine, &second);
        let covered: Vec<RowProjection> = projected
            .iter()
            .filter(|row| row.0 == "c.dart")
            .cloned()
            .collect();
        let expected: Vec<RowProjection> = full
            .iter()
            .filter(|row| row.0 == "c.dart")
            .cloned()
            .collect();
        assert_eq!(
            covered, expected,
            "the second aim's covered view is the full turn's c.dart rows — \
             the rows a stale partial merge would have dropped"
        );
        assert!(
            projected.iter().all(|row| row.0 != "b.dart"),
            "the never-aimed, never-adopted library contributes nothing"
        );
        assert_eq!(
            engine.state.results.len(),
            2,
            "two libraries hold runs: a.dart's standing, c.dart's fresh"
        );
        assert_eq!(
            engine.state.slice_deferred.len(),
            1,
            "the non-aim remainder deferred again"
        );

        // The untargeted close: the deferral drains and the whole set
        // returns.
        let restored = engine.analyze(&[], &mut |_| {}, &cancel).expect("restore");
        assert_eq!(
            row_projection(&engine, &restored),
            full,
            "the full turn after the targeted chain is whole"
        );
        assert!(engine.state.slice_deferred.is_empty());
        assert_eq!(engine.state.results.len(), 3);
        let _ = std::fs::remove_dir_all(&snap_root);
        let _ = std::fs::remove_dir_all(&store_root);
        let _ = std::fs::remove_dir_all(&sdk_root);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The run payload's own contract, pinned at the JSON face: a well-formed
    /// entry round-trips into a faithful `LibraryResult` (rows, context,
    /// fingerprint) plus its reference surface (the resolved ref, the
    /// unresolved-name row, the supertype edge — each interned column
    /// re-minted through the loading tables), a fingerprint mismatch never
    /// serves, and a file the loading engine never interned rejects the
    /// whole payload — the all-or-nothing gate the engine-level test
    /// exercises through the store.
    #[test]
    fn library_run_payload_round_trips_and_rejects_mismatches() {
        let json = concat!(
            r#"{"fingerprint":[1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16],"#,
            r#""diagnostics":[{"severity":"Warning","code_name":"dead_code","#,
            r#""code_machine":"DEAD_CODE","code_dtype":"StaticWarning","#,
            r#""code_published":true,"message":"Dead code.","file":0,"#,
            r#""range":{"offset":4,"length":9},"#,
            r#""correction":"Try removing the dead code.","#,
            r#""context":[{"message":"'dead' is declared here.","file":1,"#,
            r#""range":{"offset":0,"length":4}}]}],"#,
            r#""surface":{"refs":[{"file":0,"target_file":1,"target_name":0,"#,
            r#""target_container":null,"kind":"Invoked","#,
            r#""range":{"offset":10,"length":5},"qualifier":3}],"#,
            r#""names":[{"name":1,"kind":"Referenced","range":{"offset":20,"length":7},"#,
            r#""qualified":false}],"overflow":[],"#,
            r#""supers":[{"file":0,"name":2,"supertype":{"Name":1},"clause":"Extends"}],"#,
            r#""strings":["greet","Missing","C","p"]},"#,
            r#""files":["/w/lib/main.dart","/w/lib/other.dart"]}"#,
        );
        let payload: PersistedLibraryRun =
            serde_json::from_str(json).expect("decode");
        let fingerprint =
            ApiSignature::from_bytes([1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16]);
        let mut files = FileTable::new();
        let main = files.intern(Path::new("/w/lib/main.dart"));
        let other = files.intern(Path::new("/w/lib/other.dart"));
        let codes = CodeRegistry::builtin();
        let strings = StringTable::new();

        // The round trip: faithful rows under a matching fingerprint.
        let (run, surface) = payload
            .clone()
            .materialize(&fingerprint, &codes, &strings, &mut |path| {
                files.try_file_id(path)
            })
            .expect("matching fingerprint serves");
        assert_eq!(run.cycle_fingerprint, fingerprint);
        assert_eq!(run.diagnostics.len(), 1);
        let diagnostic = &run.diagnostics[0];
        assert_eq!(diagnostic.code.name, "dead_code");
        assert_eq!(diagnostic.severity, Severity::Warning);
        assert_eq!(diagnostic.message.as_str(), "Dead code.");
        assert_eq!(diagnostic.file, main);
        assert_eq!(diagnostic.range.offset, 4);
        assert_eq!(diagnostic.range.length, 9);
        assert_eq!(
            diagnostic.correction.as_deref(),
            Some("Try removing the dead code.")
        );
        let context = diagnostic.context.as_deref().expect("context rides");
        assert_eq!(context.len(), 1);
        assert_eq!(context[0].message.as_str(), "'dead' is declared here.");
        assert_eq!(context[0].file, other);
        assert_eq!(context[0].range.offset, 0);
        assert_eq!(context[0].range.length, 4);

        // The surface's half of the round trip: the name texts re-interned
        // into the loading table, the file indexes re-pointed into its
        // file table, and the derived CSR columns rebuilt — so the probes
        // the query faces run answer exactly as the built index would.
        let greet = strings.intern("greet");
        let refs = surface
            .references_of(other, greet)
            .expect("the persisted ref run stands");
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].kind, RefKind::Invoked);
        assert_eq!(refs[0].file, main);
        assert_eq!(refs[0].target_container, StringId::NONE);
        assert_eq!(refs[0].qualifier, strings.intern("p"));
        let missing = strings.intern("Missing");
        let unresolved: Vec<_> = surface.unresolved(missing).collect();
        assert_eq!(unresolved.len(), 1);
        assert!(!unresolved[0].qualified);
        let edges = surface
            .supertypes_of(main, strings.intern("C"))
            .expect("the persisted supertype edge stands");
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].supertype, TargetRef::Name(missing));

        // The embedded fingerprint check: a different expected face never
        // serves, even though the store key would have matched the fold.
        let moved = ApiSignature::from_bytes([16, 15, 14, 13, 12, 11, 10, 9, 8, 7, 6, 5, 4, 3, 2, 1]);
        assert!(
            payload
                .clone()
                .materialize(&moved, &codes, &strings, &mut |path| {
                    files.try_file_id(path)
                })
                .is_none(),
            "a fingerprint mismatch rejects the payload wholesale"
        );

        // A file the loading engine never interned: the run cannot be
        // attributed faithfully, so nothing serves.
        let ghost_json = json.replace("/w/lib/other.dart", "/w/lib/ghost.dart");
        let ghost: PersistedLibraryRun =
            serde_json::from_str(&ghost_json).expect("decode");
        assert!(
            ghost
                .materialize(&fingerprint, &codes, &strings, &mut |path| {
                    files.try_file_id(path)
                })
                .is_none(),
            "an unknown file path rejects the payload wholesale"
        );
    }

    /// The lint-row content-store key's contract (#4205): deterministic
    /// (same inputs → same key across processes), fork-sensitive (any
    /// dimension's move re-keys), and domain-separated from
    /// `library_run_key` (same underlying dims, different store prefix).
    #[test]
    fn lint_rows_key_deterministic_and_forked() {
        let build = [1u8; 16];
        let grants = 42u64;
        let packages = [2u8; 16];
        let content = content_hash(b"int x = 1;");
        let fingerprint = 99u64;

        let key1 = lint_rows_key(build, grants, packages, content, fingerprint);
        let key2 = lint_rows_key(build, grants, packages, content, fingerprint);
        assert_eq!(key1, key2, "same inputs must produce the same key");

        // Each fork dimension independently re-keys.
        let key_build = lint_rows_key([9u8; 16], grants, packages, content, fingerprint);
        assert_ne!(key1, key_build, "build move re-keys");

        let key_grants = lint_rows_key(build, 43, packages, content, fingerprint);
        assert_ne!(key1, key_grants, "grants move re-keys");

        let key_packages = lint_rows_key(build, grants, [8u8; 16], content, fingerprint);
        assert_ne!(key1, key_packages, "packages move re-keys");

        let key_content = lint_rows_key(build, grants, packages, content_hash(b"int y = 2;"), fingerprint);
        assert_ne!(key1, key_content, "content move re-keys");

        let key_fp = lint_rows_key(build, grants, packages, content, 100);
        assert_ne!(key1, key_fp, "fingerprint move re-keys");
    }

    /// The library-run key's contract: deterministic across processes for
    /// identical inputs, and fork-sensitive on every column — in
    /// particular the member-path column (#4221): two byte-identical
    /// libraries at different paths (the overlay-introduced file carrying
    /// another member's exact bytes) must never share one stored run,
    /// because a run's rows anchor at the member paths verbatim.
    #[test]
    fn library_run_key_deterministic_and_path_forked() {
        use std::path::Path;

        let build = [1u8; 16];
        let semantic_version = 7_u32;
        let grants = 42u64;
        let packages = [2u8; 16];
        let fingerprint = ApiSignature::from_bytes([3u8; 16]);
        let content = [content_hash(b"void g() { y }\n")];
        let at_a = [Path::new("/w/lib/a.dart")];
        let at_b = [Path::new("/w/lib/b.dart")];

        let key1 = library_run_key(
            build,
            semantic_version,
            grants,
            packages,
            &fingerprint,
            &content,
            &at_a,
        );
        let key2 = library_run_key(
            build,
            semantic_version,
            grants,
            packages,
            &fingerprint,
            &content,
            &at_a,
        );
        assert_eq!(key1, key2, "same inputs must produce the same key");

        // The false-adoption guard: identical content and fingerprint,
        // different path — one stored run must not serve the other.
        let key_path = library_run_key(
            build,
            semantic_version,
            grants,
            packages,
            &fingerprint,
            &content,
            &at_b,
        );
        assert_ne!(key1, key_path, "member-path move re-keys (#4221)");

        // The remaining columns keep forking independently.
        let key_content = library_run_key(
            build,
            semantic_version,
            grants,
            packages,
            &fingerprint,
            &[content_hash(b"var ok = 1;\n")],
            &at_a,
        );
        assert_ne!(key1, key_content, "content move re-keys");

        let key_fp = library_run_key(
            build,
            semantic_version,
            grants,
            packages,
            &ApiSignature::from_bytes([4u8; 16]),
            &content,
            &at_a,
        );
        assert_ne!(key1, key_fp, "fingerprint move re-keys");

        // The member-list shape forks too: a part joining the library
        // changes the folded path set even when the defining path stands.
        let with_part = [Path::new("/w/lib/a.dart"), Path::new("/w/lib/a.g.dart")];
        let key_part = library_run_key(
            build,
            semantic_version,
            grants,
            packages,
            &fingerprint,
            &content,
            &with_part,
        );
        assert_ne!(key1, key_part, "member-set growth re-keys");
    }

    /// The answer record codec roundtrips and rejects foreign versions
    /// (#4243): the `encode_run` discipline's miss posture — a decode
    /// failure of any kind (torn bytes, a future `version`) is a miss,
    /// never an error.
    #[test]
    fn answer_record_codec_roundtrips_and_rejects_foreign_versions() {
        let record = AnsweredRecord {
            version: ANSWER_RECORD_VERSION,
            blob: content_hash(b"rendered answer").to_string(),
            stamps: vec![
                (
                    PathBuf::from("/w/lib/a.dart"),
                    AnswerStamp {
                        dev: 1,
                        ino: 2,
                        mtime_ns: 3,
                        size: 4,
                    },
                ),
                (
                    PathBuf::from("/w/pubspec.yaml"),
                    AnswerStamp {
                        dev: 1,
                        ino: 9,
                        mtime_ns: 8,
                        size: 7,
                    },
                ),
            ],
            summary: AnswerSummary {
                files: 12,
                loc: 3400,
                diagnostics: 56,
                libraries_analyzed: 10,
                libraries_from_cache: 9,
                has_error: true,
                has_warning: false,
                has_info: true,
            },
        };
        let bytes = encode_answer(&record).expect("encodes");
        assert_eq!(decode_answer(&bytes).as_ref(), Some(&record), "roundtrip");
        // A future codec version never serves.
        let mut future = record.clone();
        future.version = ANSWER_RECORD_VERSION + 1;
        let future = encode_answer(&future).expect("encodes");
        assert_eq!(decode_answer(&future), None, "future version misses");
        // Foreign bytes miss.
        assert_eq!(decode_answer(b"not bincode at all"), None);
    }

    /// The envelope key is stable for equal inputs and forks on every
    /// key-relevant dimension: each header column,
    /// the grants fingerprint, the face tag, and the aim set — while the
    /// canonicalized aim spelling (order, duplicates) folds to one key.
    #[test]
    fn envelope_key_is_stable_and_forks_on_every_dimension() {
        let header = dartq_snapshot::GenerationHeader {
            format_version: 3,
            semantic_version: 25,
            sdk_identity: "dart-sdk 3.13.1 revision x libraries y target vm".to_owned(),
            package_graph_fingerprint: [1u8; 16],
            context_salt: [2u8; 16],
        };
        let shape = AnswerShape::new(
            AnswerFace::AnalyzeLon,
            vec![AnswerAim::Dir(PathBuf::from("/w/lib"))],
        );
        let key = envelope_key(&header, 42, &shape);
        assert_eq!(key, envelope_key(&header, 42, &shape), "stable");

        // Aim canonicalization: order and duplicates fold to one key.
        let canonical = AnswerShape::new(
            AnswerFace::AnalyzeLon,
            vec![
                AnswerAim::File(PathBuf::from("/w/lib/a.dart")),
                AnswerAim::Dir(PathBuf::from("/w/lib")),
                AnswerAim::Dir(PathBuf::from("/w/lib")),
            ],
        );
        assert_eq!(
            canonical.aims,
            vec![
                AnswerAim::Dir(PathBuf::from("/w/lib")),
                AnswerAim::File(PathBuf::from("/w/lib/a.dart")),
            ],
            "sorted, deduped"
        );
        assert_ne!(
            key,
            envelope_key(&header, 42, &canonical),
            "a different aim set forks"
        );
        let folded = AnswerShape::new(
            AnswerFace::AnalyzeLon,
            vec![
                AnswerAim::Dir(PathBuf::from("/w/lib")),
                AnswerAim::Dir(PathBuf::from("/w/lib")),
                AnswerAim::File(PathBuf::from("/w/lib/a.dart")),
            ],
        );
        assert_eq!(
            envelope_key(&header, 42, &canonical),
            envelope_key(&header, 42, &folded),
            "aim order/duplicates fold to one key"
        );

        // Every dimension forks.
        let mut forked = header.clone();
        forked.sdk_identity.push('!');
        assert_ne!(key, envelope_key(&forked, 42, &shape), "sdk identity");
        let mut forked = header.clone();
        forked.package_graph_fingerprint = [3u8; 16];
        assert_ne!(key, envelope_key(&forked, 42, &shape), "package graph");
        let mut forked = header.clone();
        forked.context_salt = [4u8; 16];
        assert_ne!(key, envelope_key(&forked, 42, &shape), "context salt");
        let mut forked = header.clone();
        forked.semantic_version += 1;
        assert_ne!(key, envelope_key(&forked, 42, &shape), "semantic version");
        assert_ne!(key, envelope_key(&header, 43, &shape), "grants");
        let other_face = AnswerShape::new(
            AnswerFace::AnalyzeJson,
            vec![AnswerAim::Dir(PathBuf::from("/w/lib"))],
        );
        assert_ne!(key, envelope_key(&header, 42, &other_face), "face tag");
        let other_aims = AnswerShape::new(AnswerFace::AnalyzeLon, vec![]);
        assert_ne!(key, envelope_key(&header, 42, &other_aims), "aim set");
    }
}
