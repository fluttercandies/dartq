//! The M1 resolution diagnostic code table, frozen at
//! `docs/facts/resolution-design-spec.md` §4.2.
//!
//! One variant per §4.2 row. Every row carries the official analyzer code
//! name (`analyzerCode`), its `UPPERCASE` machine form, the verbatim oracle
//! message template from
//! `.third/dart-sdk/pkg/analyzer/lib/src/diagnostic/diagnostic.g.dart` (line
//! cited per code), and that row's registry-true descriptor: the
//! `correctionMessage` (verbatim, `None` where the row has none), the
//! diagnostic family (`dtype` — the machine TYPE value), and the
//! `hasPublishedDocs` flag (JSON `documentation` presence). All 15 rows were
//! verified against the pinned live registry (dart 3.15 main): every code is
//! published, and exactly [`ResolutionCode::UriDoesNotExistInDocImport`] is
//! `STATIC_WARNING`-typed — the other 14 are `COMPILE_TIME_ERROR`. Severity
//! at the emit layer derives from `dtype` (the `dtype_matches_oracle_severity`
//! invariant), so no separate severity map exists.
//!
//! Emission split: the URI layer ([`crate::uri`]) produces the
//! target-lookup codes (`invalid_uri`, `uri_with_interpolation`,
//! `uri_does_not_exist`, `uri_does_not_exist_in_doc_import`,
//! `uri_has_not_been_generated`, `use_of_native_extension`); the
//! part/part-of family, `duplicate_part`, and the `ambiguous_*` use-point
//! codes are emitted by the library-graph/namespace layer and only have
//! their frozen descriptors here.

use dartq_diagnostics::DiagnosticType;

/// The §4.2 frozen resolution diagnostic code set.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ResolutionCode {
    /// import/export: target source is null (unknown `dart:` library, unknown
    /// `package:` package, unrecognized scheme) or the mapped file does not
    /// exist; part: the target file is missing (diagnostic.g.dart:18170).
    UriDoesNotExist,
    /// Same trigger as [`ResolutionCode::UriDoesNotExist`] but on an
    /// `@docImport` (diagnostic.g.dart:18188). Spec-pinned severity:
    /// `STATIC_WARNING`.
    UriDoesNotExistInDocImport,
    /// The target is a generated-file path (`isGeneratedSource` suffix set)
    /// that has not been produced on disk (diagnostic.g.dart:18206).
    UriHasNotBeenGenerated,
    /// The URI string does not parse as a URI (diagnostic.g.dart:9890).
    InvalidUri,
    /// The directive URI is a string interpolation, so it carries no string
    /// to resolve (diagnostic.g.dart:18221).
    UriWithInterpolation,
    /// The selected URI starts with `dart-ext:` (diagnostic.g.dart:18232);
    /// also the CFE face of the same diagnostic
    /// (`fragment_factory_impl.dart:935`).
    UseOfNativeExtension,
    /// Target exists but is a part file, for `import` (diagnostic.g.dart:7668).
    ImportOfNonLibrary,
    /// Target exists but is a part file, for `export` (diagnostic.g.dart:5535).
    ExportOfNonLibrary,
    /// part target exists but is not itself a part file
    /// (diagnostic.g.dart:13950).
    PartOfNonPart,
    /// `part of` name/URI does not match the enclosing library
    /// (diagnostic.g.dart:13921). URI shape always reports; name shape only
    /// when enhanced-parts is disabled (§2.6).
    PartOfDifferentLibrary,
    /// `part of name` and the enclosing library has no `library name`
    /// directive (diagnostic.g.dart:13967).
    PartOfUnnamedLibrary,
    /// The same file is included as a part twice by one library
    /// (diagnostic.g.dart:4667).
    DuplicatePart,
    /// An ambiguous name was used at a use point (resolver-side, not a
    /// namespace-build-time diagnostic) (analyzer-8.4.1
    /// `error/codes.g.dart:136-138` — `{0}` name, `{1}` full library list;
    /// Dart 3.15 main later kind-parameterized it to
    /// `diagnostic.g.dart:397`'s 3-slot form).
    AmbiguousImport,
    /// Export-side twin of [`ResolutionCode::AmbiguousImport`]
    /// (diagnostic.g.dart:317).
    AmbiguousExport,
    /// The same prefix is imported deferred in one place and non-deferred in
    /// another (analyzer-8.4.1 `error/codes.g.dart:6651`;
    /// COMPILE_TIME_ERROR, `hasPublishedDocs`).
    SharedDeferredPrefix,
}

/// Official descriptor of one [`ResolutionCode`]: code name, machine form,
/// and the registry-true oracle row (message template, correction message,
/// diagnostic family, published-docs flag; `{0}`… are placeholders).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResolutionCodeInfo {
    /// Official `snake_case` code name, one-to-one with the analyzer's
    /// `analyzerCode` (e.g. `uri_does_not_exist`).
    pub name: &'static str,
    /// `UPPERCASE` machine form (e.g. `URI_DOES_NOT_EXIST`).
    pub machine: &'static str,
    /// Verbatim oracle `problemMessage` template.
    pub message: &'static str,
    /// Verbatim oracle `correctionMessage`; `None` where the registry row
    /// has none.
    pub correction: Option<&'static str>,
    /// The registry row's family — the machine-format TYPE value.
    pub dtype: DiagnosticType,
    /// The registry row's `hasPublishedDocs`: the oracle JSON emits a
    /// `documentation` key exactly for these codes.
    pub has_published: bool,
}

impl ResolutionCode {
    /// The frozen official descriptor of this code. Correction texts, families,
    /// and published flags are verbatim from the cited `diagnostic.g.dart` rows
    /// (dart 3.15 main). Length is the frozen table itself — a data table, not
    /// logic.
    #[must_use]
    #[allow(clippy::too_many_lines)]
    pub const fn info(self) -> ResolutionCodeInfo {
        match self {
            Self::UriDoesNotExist => ResolutionCodeInfo {
                name: "uri_does_not_exist",
                machine: "URI_DOES_NOT_EXIST",
                message: "Target of URI doesn't exist: '{0}'.",
                correction: Some(
                    "Try creating the file referenced by the URI, or try using a URI for a \
                     file that does exist.",
                ),
                dtype: DiagnosticType::CompileTimeError,
                has_published: true,
            },
            Self::UriDoesNotExistInDocImport => ResolutionCodeInfo {
                name: "uri_does_not_exist_in_doc_import",
                machine: "URI_DOES_NOT_EXIST_IN_DOC_IMPORT",
                message: "Target of URI doesn't exist: '{0}'.",
                correction: Some(
                    "Try creating the file referenced by the URI, or try using a URI for a \
                     file that does exist.",
                ),
                dtype: DiagnosticType::StaticWarning,
                has_published: true,
            },
            Self::UriHasNotBeenGenerated => ResolutionCodeInfo {
                name: "uri_has_not_been_generated",
                machine: "URI_HAS_NOT_BEEN_GENERATED",
                message: "Target of URI hasn't been generated: '{0}'.",
                correction: Some(
                    "Try running the generator that will generate the file referenced by \
                     the URI.",
                ),
                dtype: DiagnosticType::CompileTimeError,
                has_published: true,
            },
            Self::InvalidUri => ResolutionCodeInfo {
                name: "invalid_uri",
                machine: "INVALID_URI",
                message: "Invalid URI syntax: '{0}'.",
                correction: None,
                dtype: DiagnosticType::CompileTimeError,
                has_published: true,
            },
            Self::UriWithInterpolation => ResolutionCodeInfo {
                name: "uri_with_interpolation",
                machine: "URI_WITH_INTERPOLATION",
                message: "URIs can't use string interpolation.",
                correction: None,
                dtype: DiagnosticType::CompileTimeError,
                has_published: true,
            },
            Self::UseOfNativeExtension => ResolutionCodeInfo {
                name: "use_of_native_extension",
                machine: "USE_OF_NATIVE_EXTENSION",
                message: "Dart native extensions are deprecated and aren't available in Dart 2.15.",
                correction: Some("Try using dart:ffi for C interop."),
                dtype: DiagnosticType::CompileTimeError,
                has_published: true,
            },
            Self::ImportOfNonLibrary => ResolutionCodeInfo {
                name: "import_of_non_library",
                machine: "IMPORT_OF_NON_LIBRARY",
                message: "The imported library '{0}' can't have a part-of directive.",
                correction: Some("Try importing the library that the part is a part of."),
                dtype: DiagnosticType::CompileTimeError,
                has_published: true,
            },
            Self::ExportOfNonLibrary => ResolutionCodeInfo {
                name: "export_of_non_library",
                machine: "EXPORT_OF_NON_LIBRARY",
                message: "The exported library '{0}' can't have a part-of directive.",
                correction: Some("Try exporting the library that the part is a part of."),
                dtype: DiagnosticType::CompileTimeError,
                has_published: true,
            },
            Self::PartOfNonPart => ResolutionCodeInfo {
                name: "part_of_non_part",
                machine: "PART_OF_NON_PART",
                message: "The included part '{0}' must have a part-of directive.",
                correction: Some("Try adding a part-of directive to '{0}'."),
                dtype: DiagnosticType::CompileTimeError,
                has_published: true,
            },
            Self::PartOfDifferentLibrary => ResolutionCodeInfo {
                name: "part_of_different_library",
                machine: "PART_OF_DIFFERENT_LIBRARY",
                message: "Expected this library to be part of '{0}', not '{1}'.",
                correction: Some(
                    "Try including a different part, or changing the name of the library \
                     in the part's part-of directive.",
                ),
                dtype: DiagnosticType::CompileTimeError,
                has_published: true,
            },
            Self::PartOfUnnamedLibrary => ResolutionCodeInfo {
                name: "part_of_unnamed_library",
                machine: "PART_OF_UNNAMED_LIBRARY",
                message: concat!(
                    "The library is unnamed. A URI is expected, not a library ",
                    "name '{0}', in the part-of directive."
                ),
                correction: Some(
                    "Try changing the part-of directive to a URI, or try including a \
                     different part.",
                ),
                dtype: DiagnosticType::CompileTimeError,
                has_published: true,
            },
            Self::DuplicatePart => ResolutionCodeInfo {
                name: "duplicate_part",
                machine: "DUPLICATE_PART",
                message: "The library already contains a part with the URI '{0}'.",
                correction: Some("Try removing all except one of the duplicated part directives."),
                dtype: DiagnosticType::CompileTimeError,
                has_published: true,
            },
            Self::AmbiguousImport => ResolutionCodeInfo {
                name: "ambiguous_import",
                machine: "AMBIGUOUS_IMPORT",
                message: "The name '{0}' is defined in the libraries {1}.",
                correction: Some(
                    "Try using 'as prefix' for one of the import directives, or hiding the \
                     name from all but one of the imports.",
                ),
                dtype: DiagnosticType::CompileTimeError,
                has_published: true,
            },
            Self::AmbiguousExport => ResolutionCodeInfo {
                name: "ambiguous_export",
                machine: "AMBIGUOUS_EXPORT",
                message: "The name '{0}' is defined in the libraries '{1}' and '{2}'.",
                correction: Some(
                    "Try removing the export of one of the libraries, or explicitly hiding \
                     the name in one of the export directives.",
                ),
                dtype: DiagnosticType::CompileTimeError,
                has_published: true,
            },
            Self::SharedDeferredPrefix => ResolutionCodeInfo {
                name: "shared_deferred_prefix",
                machine: "SHARED_DEFERRED_PREFIX",
                message: "The prefix of a deferred import can't be used in other import directives.",
                correction: Some("Try renaming one of the prefixes."),
                dtype: DiagnosticType::CompileTimeError,
                has_published: true,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The frozen table: §4.2's sixteen rows less `unsupported_dart_ext`,
    /// deduplicated into [`ResolutionCode::UseOfNativeExtension`] on
    /// re-anchor (analyzer-8.4.1 `error/codes.g.dart:7666`).
    #[test]
    fn table_is_complete_and_official_named() {
        let codes = [
            ResolutionCode::UriDoesNotExist,
            ResolutionCode::UriDoesNotExistInDocImport,
            ResolutionCode::UriHasNotBeenGenerated,
            ResolutionCode::InvalidUri,
            ResolutionCode::UriWithInterpolation,
            ResolutionCode::UseOfNativeExtension,
            ResolutionCode::ImportOfNonLibrary,
            ResolutionCode::ExportOfNonLibrary,
            ResolutionCode::PartOfNonPart,
            ResolutionCode::PartOfDifferentLibrary,
            ResolutionCode::PartOfUnnamedLibrary,
            ResolutionCode::DuplicatePart,
            ResolutionCode::AmbiguousImport,
            ResolutionCode::AmbiguousExport,
            ResolutionCode::SharedDeferredPrefix,
        ];
        assert_eq!(codes.len(), 15);
        for code in codes {
            let info = code.info();
            assert_eq!(info.name.to_uppercase(), info.machine);
            assert!(!info.message.is_empty());
        }
    }

    /// Registry-true descriptors (audit #269 F1): every row verified in the
    /// pinned live registry `diagnostic.g.dart` (dart 3.15 main) — all 15
    /// codes are published (`hasPublishedDocs: true`), exactly the
    /// `@docImport` twin is `STATIC_WARNING`-typed, the other 14 are
    /// `COMPILE_TIME_ERROR` (rows: ambiguousExport 316, ambiguousImport 395,
    /// duplicatePart 4666, exportOfNonLibrary 5534, importOfNonLibrary 7667,
    /// invalidUri 9889, partOfDifferentLibrary 13920, partOfNonPart 13949,
    /// partOfUnnamedLibrary 13966, sharedDeferredPrefix 15678,
    /// uriDoesNotExist 18169, uriDoesNotExistInDocImport 18187,
    /// uriHasNotBeenGenerated 18205, uriWithInterpolation 18219,
    /// useOfNativeExtension 18231).
    #[test]
    fn dtypes_and_published_flags_match_oracle_registry() {
        for code in [
            ResolutionCode::UriDoesNotExist,
            ResolutionCode::UriDoesNotExistInDocImport,
            ResolutionCode::UriHasNotBeenGenerated,
            ResolutionCode::InvalidUri,
            ResolutionCode::UriWithInterpolation,
            ResolutionCode::UseOfNativeExtension,
            ResolutionCode::ImportOfNonLibrary,
            ResolutionCode::ExportOfNonLibrary,
            ResolutionCode::PartOfNonPart,
            ResolutionCode::PartOfDifferentLibrary,
            ResolutionCode::PartOfUnnamedLibrary,
            ResolutionCode::DuplicatePart,
            ResolutionCode::AmbiguousImport,
            ResolutionCode::AmbiguousExport,
            ResolutionCode::SharedDeferredPrefix,
        ] {
            let info = code.info();
            assert!(info.has_published, "{} must be published", info.name);
            let expected = if code == ResolutionCode::UriDoesNotExistInDocImport {
                DiagnosticType::StaticWarning
            } else {
                DiagnosticType::CompileTimeError
            };
            assert_eq!(info.dtype, expected, "{}", info.name);
        }
    }

    /// Correction messages are verbatim oracle `correctionMessage` texts;
    /// `invalid_uri` and `uri_with_interpolation` have none in the registry.
    #[test]
    fn corrections_are_verbatim_oracle_texts() {
        assert_eq!(
            ResolutionCode::UriDoesNotExist.info().correction,
            Some(
                "Try creating the file referenced by the URI, or try using a URI for a \
                 file that does exist."
            )
        );
        assert_eq!(
            ResolutionCode::PartOfNonPart.info().correction,
            Some("Try adding a part-of directive to '{0}'.")
        );
        assert_eq!(ResolutionCode::InvalidUri.info().correction, None);
        assert_eq!(ResolutionCode::UriWithInterpolation.info().correction, None);
    }

    /// Verbatim oracle texts for the codes the URI layer emits.
    #[test]
    fn uri_layer_message_texts_are_verbatim() {
        assert_eq!(
            ResolutionCode::UriDoesNotExist.info().message,
            "Target of URI doesn't exist: '{0}'."
        );
        assert_eq!(
            ResolutionCode::InvalidUri.info().message,
            "Invalid URI syntax: '{0}'."
        );
        assert_eq!(
            ResolutionCode::UriWithInterpolation.info().message,
            "URIs can't use string interpolation."
        );
    }
}
