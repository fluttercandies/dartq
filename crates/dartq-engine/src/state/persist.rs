//! Facts persistence (M3 cold-start, PRD §17.2): the plain-data projection
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

use dartq_diagnostics::{CodeInfo, CodeRegistry, Diagnostic, DiagnosticType, Severity};
use dartq_package::{PackageIdentity, PackageTable, SourceCategory};
use dartq_resolution::{CombinatorClause, Combinators, ResolutionCode, ResolvedTarget};
use dartq_sdk::SdkSnapshot;
use dartq_semantic::ApiSignature;
use dartq_semantic::persisted::{PersistedElements, PersistedRange};
use dartq_source::{FileId, FileTable, LineIndex, StringId, StringTable, content_hash};

use crate::state::{DirectiveFacts, DirectiveKind, UnitFacts};

/// blake3 truncated to 128 bits — the house digest width (§10.5).
fn blake3_128(bytes: &[u8]) -> [u8; 16] {
    let digest = blake3::hash(bytes);
    let mut id = [0u8; 16];
    id.copy_from_slice(&digest.as_bytes()[..16]);
    id
}

/// The package graph fingerprint (PRD §5.6): blake3-128 over every package
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

/// The context-fork salt the generation header carries (#482): blake3-128
/// over the fork dimensions [`sdk_identity`] and
/// [`package_graph_fingerprint`] do not already pin — the SDK target
/// segment, the conditional-import declared-variables map, and the
/// analysis-options content. Sorted keys and length-prefixed fields make
/// the digest a pure function of the dimension values (equal contexts
/// digest equally across processes); any difference forks the header, so
/// two contexts that would analyze differently can never collide on one
/// generation directory. Options fold the *located options file's* path
/// and raw bytes per root — the same content the merged options derive
/// from, the `context_key` precedent — so exclude-glob and lint edits
/// both move it, and an unreadable file contributes exactly what the
/// engine's own loader analyzed (empty bytes).
// The engine's declared-variables map is the face (its FxHasher is part
// of the engine's identity, not a generalization axis).
#[allow(clippy::implicit_hasher)]
#[must_use]
pub fn context_salt(
    target: &str,
    declared_variables: &FxHashMap<String, String>,
    roots: &[PathBuf],
) -> [u8; 16] {
    let mut bytes = Vec::new();
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
        let diagnostics = facts
            .diagnostics
            .iter()
            .map(|diagnostic| PersistedDiagnostic::of(diagnostic, &mut files))
            .collect();
        Self {
            api_signature: *facts.api_signature.as_bytes(),
            directives,
            elements: PersistedElements::of(&facts.elements, strings),
            diagnostics,
            files: files.paths,
        }
    }

    /// Restores the facts: re-interns the string slices into `strings`,
    /// mints file ids through `intern_file` (the load wiring drives its own
    /// file table), and rebuilds the line index from `source` — the source
    /// bytes whose content hash keys this entry, which the loader holds by
    /// construction. `codes` is the builtin registry, built **once per load**
    /// by the caller: `materialize` runs per entry, and a per-entry
    /// `CodeRegistry::builtin()` would rebuild the ~270-entry map for every
    /// snapshot file (the l1.rs #269 F2 anti-pattern).
    #[must_use]
    pub fn materialize(
        self,
        source: &str,
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
        let diagnostics = self
            .diagnostics
            .into_iter()
            .filter_map(|diagnostic| diagnostic.restore(codes, &files))
            .collect();
        UnitFacts {
            content: content_hash(source.as_bytes()),
            api_signature: ApiSignature::from_bytes(self.api_signature),
            directives,
            elements: Arc::new(self.elements.into_elements(strings)),
            diagnostics,
            line_index: LineIndex::new(source),
        }
    }
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
    /// The `show`/`hide` clauses in source order, names as written (bare
    /// identifiers — the constructor re-derives the `name=` setter
    /// spellings on restore).
    pub combinators: Vec<PersistedCombinator>,
    /// The dotted name of `library a.b;` / `part of a.b;`.
    pub name: Option<String>,
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
    pub names: Vec<String>,
}

/// Mirror of the frozen §4.2 resolution code set; an exhaustive match on
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
/// [`PersistedDiagnostic::restore`] drops as unattributable.
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
            uri: directive.uri.as_ref().map(|uri| uri.to_string()),
            uri_span: PersistedRange::of(directive.uri_span),
            target: match directive.target {
                ResolvedTarget::Resolved(file) => PersistedTarget::File(files.id(file)),
                ResolvedTarget::External => PersistedTarget::External,
                ResolvedTarget::Missing => PersistedTarget::Missing,
            },
            code: directive.code.map(PersistedResolutionCode::of),
            prefix: directive.prefix.map(|id| strings.resolve(id).to_owned()),
            deferred: directive.deferred,
            combinators: directive
                .combinators
                .clauses()
                .iter()
                .map(|clause| match clause {
                    CombinatorClause::Show(names) => PersistedCombinator {
                        hide: false,
                        names: written_names(names, strings),
                    },
                    CombinatorClause::Hide(names) => PersistedCombinator {
                        hide: true,
                        names: written_names(names, strings),
                    },
                })
                .collect(),
            name: directive.name.as_ref().map(|name| name.to_string()),
        }
    }

    fn restore(self, strings: &StringTable, files: &[FileId]) -> DirectiveFacts {
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
            combinators: Combinators::new(
                self.combinators.into_iter().map(|clause| {
                    let names: Box<[StringId]> = clause
                        .names
                        .iter()
                        .map(|text| strings.intern(text))
                        .collect();
                    if clause.hide {
                        CombinatorClause::Hide(names)
                    } else {
                        CombinatorClause::Show(names)
                    }
                }),
                strings,
            ),
            name: self.name.map(Box::from),
        }
    }
}

impl PersistedDiagnostic {
    fn of(diagnostic: &Diagnostic, files: &mut Files<'_>) -> Self {
        Self {
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
                DiagnosticType::Tool => PersistedDiagnosticType::Tool,
            },
            code_published: diagnostic.code.has_published_docs,
            message: diagnostic.message.clone(),
            file: files.id(diagnostic.file),
            range: PersistedRange::of(diagnostic.range),
            correction: diagnostic.correction.clone(),
        }
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
        Some(Diagnostic::new(
            severity,
            code,
            self.message,
            file,
            self.range.source_range(),
            self.correction,
        ))
    }
}

/// The file-id slice under construction: every referenced `FileId` gets one
/// path slot, first-use ordered.
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

    fn id(&mut self, file: FileId) -> u32 {
        if let Some(&at) = self.at.get(&file) {
            return at;
        }
        let path = self
            .table
            .path(file)
            .expect("facts only carry file ids the engine's own table interned")
            .to_path_buf();
        self.paths.push(path);
        let at = self.paths.len() as u32 - 1;
        self.at.insert(file, at);
        at
    }
}

/// The written (bare) names of one expanded clause: [`Combinators::new`]
/// appends each base name's `name=` setter spelling, and the combinator
/// grammar admits no `=`, so the `=`-suffixed entries are exactly the
/// derived spellings — dropping them keeps the payload at the written
/// surface, and the constructor re-expands identically on restore.
fn written_names(names: &[StringId], strings: &StringTable) -> Vec<String> {
    names
        .iter()
        .filter(|id| !strings.resolve(**id).ends_with('='))
        .map(|id| strings.resolve(*id).to_owned())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::DirectiveKind;
    use crate::{CancellationToken, Engine, EngineConfig, EngineEvent, memory_store};
    use dartq_pub::{LockFile, PackageConfig, Pubspec, build_graph};
    use dartq_snapshot::{SnapshotConfig, SnapshotStore};
    use dartq_source::SourceRange;
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
        r#""combinators":[{"hide":false,"names":["showOnly"]},{"hide":true,"names":["hideMe"]}],"#,
        r#""name":"main"}],"#,
        r#""elements":{"strings":[],"decls":[],"containers":[],"container_order":[],"#,
        r#""extension_faces":[],"duplicates":[],"arity_violations":[]},"diagnostics":["#,
        r#"{"severity":"Error","code_name":"uri_does_not_exist","code_machine":"URI_DOES_NOT_EXIST","#,
        r#""code_dtype":"CompileTimeError","code_published":false,"message":"Target of URI "#,
        r#"doesn't exist: 'package:a/a.dart'.","file":0,"range":{"offset":0,"length":18},"#,
        r#""correction":null},{"severity":"Warning","code_name":"dead_code","#,
        r#""code_machine":"DEAD_CODE","code_dtype":"StaticWarning","code_published":true,"#,
        r#""message":"Dead code.","file":1,"range":{"offset":4,"length":9},"#,
        r#""correction":"Try removing the dead code."}],"#,
        r#""files":["/w/lib/main.dart","/w/lib/other.dart"]}"#,
    );

    /// The representative payload the shape pin guards: one instance of every
    /// engine-side payload struct in the `PersistedFacts` tree, exercising
    /// the `Some`/`None` faces (`uri`, `code`, `prefix`, `name`,
    /// `correction`), both combinator directions, a file-target index, and
    /// two severities. The element model rides empty
    /// (`FileElements::default()` projects to empty columns): the semantic
    /// mirrors live in dartq-semantic with their own review surface, while
    /// the pin guards the engine-side layout — including
    /// `PersistedElements`' own column list.
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
                    },
                    PersistedCombinator {
                        hide: true,
                        names: vec!["hideMe".to_owned()],
                    },
                ],
                name: Some("main".to_owned()),
            }],
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
    /// Locking `a` as hosted flips its source category (Path → PubCache),
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
            }],
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
            }],
            files: vec![PathBuf::from("/w/lib/main.dart")],
        };

        let codes = CodeRegistry::builtin();
        let restored_strings = StringTable::new();
        let mut restored_files = FileTable::new();
        let restored = payload.materialize(
            "import 'package:a/a.dart';\n",
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

    /// The fingerprint is the whole-generation miss gate (PRD §5.6/§17.2):
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
    /// the target, a declared variable, and the located options content
    /// each move it.
    #[test]
    fn context_salt_is_stable_and_dimension_sensitive() {
        let plain = salt_config_dir("plain", None);
        let roots = |dir: &Path| vec![dir.to_path_buf()];
        let base = context_salt("vm", &FxHashMap::default(), &roots(&plain));
        assert_eq!(
            base,
            context_salt("vm", &FxHashMap::default(), &roots(&plain)),
            "equal dimensions digest equally"
        );
        assert_ne!(
            base,
            context_salt("js", &FxHashMap::default(), &roots(&plain)),
            "the target segment moves the salt"
        );
        let mut defined = FxHashMap::default();
        defined.insert("dart.vm.profile".to_owned(), "true".to_owned());
        assert_ne!(
            base,
            context_salt("vm", &defined, &roots(&plain)),
            "a declared variable moves the salt"
        );

        let options = salt_config_dir("options", Some("linter:\n  rules:\n    - avoid_print\n"));
        let with_options = context_salt("vm", &FxHashMap::default(), &roots(&options));
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
            context_salt("vm", &FxHashMap::default(), &roots(&options)),
            "an options edit moves the salt"
        );
    }

    /// The full warm-start seam (#482): save → fresh store load → ingest →
    /// the next apply serves the materialized facts — the warm face equals
    /// a from-scratch engine's on every observable column, and the
    /// directive re-resolves against the consuming file's directory —
    /// while an entry whose content moved since the save drops (its file
    /// recomputes) and the staging map drains whole.
    #[test]
    fn warm_start_round_trips_and_drops_stale_entries() {
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
                semdiag: None,
                target: "vm".to_owned(),
                declared_variables: FxHashMap::default(),
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

        assert!(
            warm_engine.state.warm_start.is_empty(),
            "single-shot staging: matched entries consumed, stale ones dropped"
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

    /// A directive with show/hide combinators, a file target, a prefix, and
    /// a diagnostic survive the payload round trip; the api signature rides
    /// byte-exact; the restored line index and content hash match a fresh
    /// compute over the same source.
    #[test]
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
        }];
        let diagnostics = vec![Diagnostic::new(
            Severity::Error,
            CodeRegistry::builtin()
                .get("uri_does_not_exist")
                .expect("registry code"),
            "Target of URI doesn't exist: 'package:other/other.dart'.",
            declared,
            range,
            None,
        )];
        let source = "import 'package:other/other.dart';\nvoid main() {}\n";
        let facts = UnitFacts {
            content: content_hash(source.as_bytes()),
            api_signature: ApiSignature::from_bytes([9; 16]),
            directives,
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
        let restored = decoded.materialize(source, &restored_strings, &codes, &mut |path| {
            restored_files.intern(path)
        });

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
        assert_eq!(diagnostic.message, original.message);
        assert_eq!(diagnostic.range, original.range);
        assert_eq!(diagnostic.correction, original.correction);

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
}
