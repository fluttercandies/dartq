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
//! [`sdk_identity`] and [`package_graph_fingerprint`]. Load shape (#344
//! seam): `Snapshot<PersistedFacts>` → [`PersistedFacts::materialize`]
//! per entry.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use dartq_diagnostics::{CodeInfo, CodeRegistry, Diagnostic, DiagnosticType, Severity};
use dartq_package::{PackageIdentity, PackageTable, SourceCategory};
use dartq_resolution::{CombinatorClause, Combinators, ResolutionCode, ResolvedTarget};
use dartq_sdk::SdkSnapshot;
use dartq_semantic::ApiSignature;
use dartq_semantic::persisted::{PersistedElements, PersistedRange};
use dartq_source::{FileId, FileTable, LineIndex, StringId, StringTable, content_hash};

use crate::state::{DirectiveFacts, DirectiveKind, SDK_TARGET, UnitFacts};

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
/// these.
#[must_use]
pub fn sdk_identity(sdk: &SdkSnapshot) -> String {
    format!(
        "dart-sdk {} revision {} libraries {} target {}",
        sdk.dart.version,
        sdk.dart.revision.as_deref().unwrap_or("-"),
        sdk.dart.libraries_json_hash,
        SDK_TARGET,
    )
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
    /// construction.
    #[must_use]
    pub fn materialize(
        self,
        source: &str,
        strings: &StringTable,
        intern_file: &mut dyn FnMut(&Path) -> FileId,
    ) -> UnitFacts {
        let files: Vec<FileId> = self.files.iter().map(|path| intern_file(path)).collect();
        let codes = CodeRegistry::builtin();
        let directives = self
            .directives
            .into_iter()
            .map(|directive| directive.restore(strings, &files))
            .collect();
        let diagnostics = self
            .diagnostics
            .into_iter()
            .map(|diagnostic| diagnostic.restore(&codes, &files))
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
/// the stored columns so no diagnostic is ever dropped on load.
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
                PersistedTarget::File(at) => ResolvedTarget::Resolved(files[at as usize]),
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

    fn restore(self, codes: &CodeRegistry, files: &[FileId]) -> Diagnostic {
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
        Diagnostic::new(
            severity,
            code,
            self.message,
            files[self.file as usize],
            self.range.source_range(),
            self.correction,
        )
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
    use dartq_source::SourceRange;
    use std::path::Path;

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

        // Restore into fresh tables — a new process's world.
        let restored_strings = StringTable::new();
        let mut restored_files = FileTable::new();
        let restored = decoded.materialize(source, &restored_strings, &mut |path| {
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
