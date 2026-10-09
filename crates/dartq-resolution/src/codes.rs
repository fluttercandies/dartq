//! The M1 resolution diagnostic code table, frozen at
//! `docs/facts/resolution-design-spec.md` §4.2.
//!
//! One variant per §4.2 row. Every row carries the official analyzer code
//! name (`analyzerCode`), its `UPPERCASE` machine form, and the verbatim
//! oracle message template from
//! `.third/dart-sdk/pkg/analyzer/lib/src/diagnostic/diagnostic.g.dart` (line
//! cited per code). Severity is pinned by the spec only where it lists one:
//! [`ResolutionCode::UriDoesNotExist`] is `COMPILE_TIME_ERROR` and
//! [`ResolutionCode::UriDoesNotExistInDocImport`] is `STATIC_WARNING`; the
//! remaining severities are an emit-layer decision and are not fixed here.
//!
//! Emission split: the URI layer ([`crate::uri`]) produces the
//! target-lookup codes (`invalid_uri`, `uri_with_interpolation`,
//! `uri_does_not_exist`, `uri_does_not_exist_in_doc_import`,
//! `uri_has_not_been_generated`, `use_of_native_extension`); the
//! part/part-of family, `duplicate_part`, and the `ambiguous_*` use-point
//! codes are emitted by the library-graph/namespace layer and only have
//! their frozen descriptors here.

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
/// and the verbatim oracle message template (`{0}`… are placeholders).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResolutionCodeInfo {
    /// Official `snake_case` code name, one-to-one with the analyzer's
    /// `analyzerCode` (e.g. `uri_does_not_exist`).
    pub name: &'static str,
    /// `UPPERCASE` machine form (e.g. `URI_DOES_NOT_EXIST`).
    pub machine: &'static str,
    /// Verbatim oracle `problemMessage` template.
    pub message: &'static str,
}

impl ResolutionCode {
    /// The frozen official descriptor of this code.
    #[must_use]
    pub const fn info(self) -> ResolutionCodeInfo {
        match self {
            Self::UriDoesNotExist => ResolutionCodeInfo {
                name: "uri_does_not_exist",
                machine: "URI_DOES_NOT_EXIST",
                message: "Target of URI doesn't exist: '{0}'.",
            },
            Self::UriDoesNotExistInDocImport => ResolutionCodeInfo {
                name: "uri_does_not_exist_in_doc_import",
                machine: "URI_DOES_NOT_EXIST_IN_DOC_IMPORT",
                message: "Target of URI doesn't exist: '{0}'.",
            },
            Self::UriHasNotBeenGenerated => ResolutionCodeInfo {
                name: "uri_has_not_been_generated",
                machine: "URI_HAS_NOT_BEEN_GENERATED",
                message: "Target of URI hasn't been generated: '{0}'.",
            },
            Self::InvalidUri => ResolutionCodeInfo {
                name: "invalid_uri",
                machine: "INVALID_URI",
                message: "Invalid URI syntax: '{0}'.",
            },
            Self::UriWithInterpolation => ResolutionCodeInfo {
                name: "uri_with_interpolation",
                machine: "URI_WITH_INTERPOLATION",
                message: "URIs can't use string interpolation.",
            },
            Self::UseOfNativeExtension => ResolutionCodeInfo {
                name: "use_of_native_extension",
                machine: "USE_OF_NATIVE_EXTENSION",
                message: "Dart native extensions are deprecated and aren't available in Dart 2.15.",
            },
            Self::ImportOfNonLibrary => ResolutionCodeInfo {
                name: "import_of_non_library",
                machine: "IMPORT_OF_NON_LIBRARY",
                message: "The imported library '{0}' can't have a part-of directive.",
            },
            Self::ExportOfNonLibrary => ResolutionCodeInfo {
                name: "export_of_non_library",
                machine: "EXPORT_OF_NON_LIBRARY",
                message: "The exported library '{0}' can't have a part-of directive.",
            },
            Self::PartOfNonPart => ResolutionCodeInfo {
                name: "part_of_non_part",
                machine: "PART_OF_NON_PART",
                message: "The included part '{0}' must have a part-of directive.",
            },
            Self::PartOfDifferentLibrary => ResolutionCodeInfo {
                name: "part_of_different_library",
                machine: "PART_OF_DIFFERENT_LIBRARY",
                message: "Expected this library to be part of '{0}', not '{1}'.",
            },
            Self::PartOfUnnamedLibrary => ResolutionCodeInfo {
                name: "part_of_unnamed_library",
                machine: "PART_OF_UNNAMED_LIBRARY",
                message: concat!(
                    "The library is unnamed. A URI is expected, not a library ",
                    "name '{0}', in the part-of directive."
                ),
            },
            Self::DuplicatePart => ResolutionCodeInfo {
                name: "duplicate_part",
                machine: "DUPLICATE_PART",
                message: "The library already contains a part with the URI '{0}'.",
            },
            Self::AmbiguousImport => ResolutionCodeInfo {
                name: "ambiguous_import",
                machine: "AMBIGUOUS_IMPORT",
                message: "The name '{0}' is defined in the libraries {1}.",
            },
            Self::AmbiguousExport => ResolutionCodeInfo {
                name: "ambiguous_export",
                machine: "AMBIGUOUS_EXPORT",
                message: "The name '{0}' is defined in the libraries '{1}' and '{2}'.",
            },
            Self::SharedDeferredPrefix => ResolutionCodeInfo {
                name: "shared_deferred_prefix",
                machine: "SHARED_DEFERRED_PREFIX",
                message: "The prefix of a deferred import can't be used in other import directives.",
            },
        }
    }

    /// Spec-pinned severity (`§4.2` `级别` column), where the spec pins one.
    #[must_use]
    pub const fn pinned_severity(self) -> Option<&'static str> {
        match self {
            Self::UriDoesNotExist => Some("COMPILE_TIME_ERROR"),
            Self::UriDoesNotExistInDocImport => Some("STATIC_WARNING"),
            _ => None,
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

    /// The spec's pinned severities.
    #[test]
    fn pinned_severities_match_spec() {
        assert_eq!(
            ResolutionCode::UriDoesNotExist.pinned_severity(),
            Some("COMPILE_TIME_ERROR")
        );
        assert_eq!(
            ResolutionCode::UriDoesNotExistInDocImport.pinned_severity(),
            Some("STATIC_WARNING")
        );
        assert_eq!(ResolutionCode::InvalidUri.pinned_severity(), None);
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
