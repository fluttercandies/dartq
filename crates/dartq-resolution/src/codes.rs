//! The M1 resolution diagnostic code table, frozen at design time.
//!
//! One variant per frozen row. Every row carries the official analyzer code
//! name (`analyzerCode`), its `UPPERCASE` machine form, the verbatim oracle
//! message template from
//! `upstream Dart SDK source, pkg/analyzer/lib/src/diagnostic/diagnostic.g.dart` (line
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
//!
//! The six directive-combinator/duplicate static warnings
//! ([`UNDEFINED_HIDDEN_NAME`]/[`UNDEFINED_SHOWN_NAME`],
//! [`DUPLICATE_HIDDEN_NAME`]/[`DUPLICATE_SHOWN_NAME`],
//! [`DUPLICATE_IMPORT`]/[`DUPLICATE_EXPORT`]) and the export/re-export graph
//! family ([`EXPORT_INTERNAL_LIBRARY`],
//! [`DEFERRED_IMPORT_OF_EXTENSION`],
//! [`INVALID_EXPORT_OF_INTERNAL_ELEMENT`]/[`.._INDIRECTLY`],
//! [`RECURSIVE_COMPILE_TIME_CONSTANT`], [`RECURSIVE_CONSTRUCTOR_REDIRECT`],
//! the [`RECURSIVE_INTERFACE_INHERITANCE`] clause family,
//! [`TOP_LEVEL_CYCLE`]) are registered as companion descriptor consts beside
//! the table instead of variants, for one structural reason: the engine's
//! persisted directive-code slot (`PersistedResolutionCode`) exhaustively
//! mirrors [`ResolutionCode`] by design, and these codes never ride it —
//! they are not URI-target outcomes but directive-combinator checks the
//! semantic layer emits with the combinator name spans ([`crate::
//! library_graph::LibraryGraph::undefined_combinator_names`]),
//! cross-directive duplicate findings a per-directive slot cannot express,
//! or element-graph cycle findings ([`crate::export_graph`],
//! [`crate::cycles`]). Extending the frozen enum for them would trip the
//! persistence tripwire in a foreign crate for a slot they can never
//! occupy. [`ResolutionCode::AmbiguousExport`] predates the family and
//! stays a variant; its descriptor stays the single source for
//! `ambiguous_export`.

use dartq_diagnostics::DiagnosticType;

/// The frozen resolution diagnostic code set.
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
    /// when enhanced-parts is disabled.
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
    /// `COMPILE_TIME_ERROR`, `hasPublishedDocs`).
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

/// `import`/`export` `hide` clause names a member the target library's
/// export namespace does not define — registry row verbatim
/// (`diagnostic.g.dart:16936`, dart 3.15 main). `STATIC_WARNING`, published;
/// `{0}` is the target library's URI, `{1}` the hidden name. The oracle
/// checks both name spellings against `library.exportNamespace`
/// (`element_resolver.dart:407-432` `_resolveCombinators`) and reports only
/// when neither the getter nor the setter face resolves
/// (`dead_code_verifier.dart:141-158` `_checkCombinator`); the check is
/// suppressed entirely when the directive target has no library or its
/// origin file does not exist (`dead_code_verifier.dart:104-112`). The
/// emit-side decision face is [`crate::library_graph::
/// LibraryGraph::undefined_combinator_names`].
pub const UNDEFINED_HIDDEN_NAME: ResolutionCodeInfo = ResolutionCodeInfo {
    name: "undefined_hidden_name",
    machine: "UNDEFINED_HIDDEN_NAME",
    message: "The library '{0}' doesn't export a member with the hidden name '{1}'.",
    correction: Some("Try removing the name from the list of hidden members."),
    dtype: DiagnosticType::StaticWarning,
    has_published: true,
};

/// `import`/`export` `show` clause names a member the target library's
/// export namespace does not define — the `show`-clause twin of
/// [`UNDEFINED_HIDDEN_NAME`], registry row verbatim
/// (`diagnostic.g.dart:17198`, dart 3.15 main). Same decision face, same
/// suppression, `STATIC_WARNING`, published.
pub const UNDEFINED_SHOWN_NAME: ResolutionCodeInfo = ResolutionCodeInfo {
    name: "undefined_shown_name",
    machine: "UNDEFINED_SHOWN_NAME",
    message: "The library '{0}' doesn't export a member with the shown name '{1}'.",
    correction: Some("Try removing the name from the list of shown members."),
    dtype: DiagnosticType::StaticWarning,
    has_published: true,
};

/// An `import`/`export` combinator clause lists the same target-defined
/// member twice — registry row verbatim (`diagnostic.g.dart:4585`, dart
/// 3.15 main). `STATIC_WARNING`, published; the report anchors at each 2nd+
/// written occurrence inside one clause (`imports_verifier.dart:328-358`
/// `_addDuplicateShownHiddenNames` — a fresh seen-set per clause, defined
/// names only, so cross-clause repeats and undefined names never report).
/// The emit-side decision face is [`crate::library_graph::
/// LibraryGraph::duplicate_combinator_names`].
pub const DUPLICATE_HIDDEN_NAME: ResolutionCodeInfo = ResolutionCodeInfo {
    name: "duplicate_hidden_name",
    machine: "DUPLICATE_HIDDEN_NAME",
    message: "Duplicate hidden name.",
    correction: Some("Try removing the repeated name from the list of hidden members."),
    dtype: DiagnosticType::StaticWarning,
    has_published: true,
};

/// The `show`-clause twin of [`DUPLICATE_HIDDEN_NAME`] — registry row
/// verbatim (`diagnostic.g.dart:4757`), same decision face, `STATIC_WARNING`,
/// published.
pub const DUPLICATE_SHOWN_NAME: ResolutionCodeInfo = ResolutionCodeInfo {
    name: "duplicate_shown_name",
    machine: "DUPLICATE_SHOWN_NAME",
    message: "Duplicate shown name.",
    correction: Some("Try removing the repeated name from the list of shown members."),
    dtype: DiagnosticType::StaticWarning,
    has_published: true,
};

/// One unit imports the same library twice with syntactically identical
/// directives (same `as` prefix, same combinator shape) — registry row
/// verbatim (`diagnostic.g.dart:4619`, dart 3.15 main). `STATIC_WARNING`,
/// published; reported at each repeat directive's URI token
/// (`imports_verifier.dart:361-390` `_duplicates`: same resolved library +
/// `areSyntacticallyIdenticalExceptUri`, adjacent-pair walk).
pub const DUPLICATE_IMPORT: ResolutionCodeInfo = ResolutionCodeInfo {
    name: "duplicate_import",
    machine: "DUPLICATE_IMPORT",
    message: "Duplicate import.",
    correction: Some("Try removing all but one import of the library."),
    dtype: DiagnosticType::StaticWarning,
    has_published: true,
};

/// The export-side twin of [`DUPLICATE_IMPORT`] — registry row verbatim
/// (`diagnostic.g.dart:4537`), same decision shape, `STATIC_WARNING`,
/// published.
pub const DUPLICATE_EXPORT: ResolutionCodeInfo = ResolutionCodeInfo {
    name: "duplicate_export",
    machine: "DUPLICATE_EXPORT",
    message: "Duplicate export.",
    correction: Some("Try removing all but one export of the library."),
    dtype: DiagnosticType::StaticWarning,
    has_published: true,
};

/// One export directive exports a name a previous export directive of the
/// same library already exports as a **different** element — the frozen
/// row is [`ResolutionCode::AmbiguousExport`]'s (registry row verbatim,
/// `diagnostic.g.dart:264`, dart 3.13.7). `COMPILE_TIME_ERROR`,
/// published; `{0}` the conflicting name, `{1}`/`{2}` the two elements'
/// declaring-library URIs. One report per directive (the first conflicting
/// name only), anchored at the directive, accumulated across the library's
/// directives in source order (`error_verifier.dart:2593-2621`
/// `_checkForAmbiguousExport` over `LibraryVerificationContext
/// ._exportedElements`; the directive namespace is the target's export
/// scope under the directive's combinators, `scope.dart:99-110`
/// `createExportNamespaceForDirective2`). The decision face is
/// [`crate::export_graph`]`::LibraryGraph::ambiguous_exports`.

/// An export directive targets an internal SDK library (`dart:_…`) from a
/// non-system library — registry row verbatim (`diagnostic.g.dart:5332`,
/// dart 3.13.7). `COMPILE_TIME_ERROR`, published, no correction; `{0}` is
/// the directive's written URI string. The gate: the exported library's
/// canonical URI starts with `dart:_` (`element.dart:7042-7044`
/// `isInternalSdkLibrary`) and the reporting library is not itself a
/// `dart:`-scheme library (`error_verifier.dart:4651-4654`,
/// `_isInSystemLibrary` set from `uri.isScheme('dart')`, `:320`). The
/// decision face is
/// [`crate::export_graph`]`::LibraryGraph::export_internal_library_directives`.
pub const EXPORT_INTERNAL_LIBRARY: ResolutionCodeInfo = ResolutionCodeInfo {
    name: "export_internal_library",
    machine: "EXPORT_INTERNAL_LIBRARY",
    message: "The library '{0}' is internal and can't be exported.",
    correction: None,
    dtype: DiagnosticType::CompileTimeError,
    has_published: true,
};

/// A deferred import's namespace contains an extension — registry row
/// verbatim (`diagnostic.g.dart:3628`, dart 3.13.7). `COMPILE_TIME_ERROR`,
/// published; one report per directive at its URI, gated on the import
/// carrying a deferred prefix (`error_verifier.dart:1339-1356`
/// `visitImportDirective`'s `prefix?.isDeferred` probe) and any
/// `ExtensionElement` surviving the combinators
/// (`error_verifier.dart:4393-4405` `_checkForDeferredImportOfExtensions`).
/// The decision face is
/// [`crate::export_graph`]`::LibraryGraph::deferred_import_exports_extension`.
pub const DEFERRED_IMPORT_OF_EXTENSION: ResolutionCodeInfo = ResolutionCodeInfo {
    name: "deferred_import_of_extension",
    machine: "DEFERRED_IMPORT_OF_EXTENSION",
    message: "Deferred library imports must hide all extension declarations.",
    correction: Some(
        "Try adding either a show combinator listing the names you need to \
         reference or a hide combinator listing all of the extension \
         declarations.",
    ),
    dtype: DiagnosticType::CompileTimeError,
    has_published: true,
};

/// A public-API library's export directive exports an `@internal`-annotated
/// member — registry row verbatim (`diagnostic.g.dart:8644`, dart 3.13.7).
/// `STATIC_WARNING`, published; the gate is the workspace-package public-API
/// face (`best_practices_verifier.dart:96-99` `_inPackagePublicApi` over
/// `workspace/pub.dart:543-570` `sourceIsInPublicApi` — under the package's
/// `lib/`, not under `lib/src/`). The decision face is
/// [`crate::export_graph`]`::LibraryGraph::invalid_internal_export_findings`.
pub const INVALID_EXPORT_OF_INTERNAL_ELEMENT: ResolutionCodeInfo = ResolutionCodeInfo {
    name: "invalid_export_of_internal_element",
    machine: "INVALID_EXPORT_OF_INTERNAL_ELEMENT",
    message: "The member '{0}' can't be exported as a part of a package's public API.",
    correction: Some("Try using a hide clause to hide '{0}'."),
    dtype: DiagnosticType::StaticWarning,
    has_published: true,
};

/// The signature twin of [`INVALID_EXPORT_OF_INTERNAL_ELEMENT`]: an exported
/// executable member's signature mentions an `@internal` type alias —
/// registry row verbatim (`diagnostic.g.dart:8666`, dart 3.13.7).
/// `STATIC_WARNING`, published; `{0}` the alias name, `{1}` the exported
/// member. One report per matching signature type, no early exit
/// (`best_practices_verifier.dart:1029-1049` walks parameter types, return
/// type, and type-parameter bounds).
pub const INVALID_EXPORT_OF_INTERNAL_ELEMENT_INDIRECTLY: ResolutionCodeInfo =
    ResolutionCodeInfo {
        name: "invalid_export_of_internal_element_indirectly",
        machine: "INVALID_EXPORT_OF_INTERNAL_ELEMENT_INDIRECTLY",
        message: concat!(
            "The member '{0}' can't be exported as a part of a package's public ",
            "API, but is indirectly exported as part of the signature of '{1}'."
        ),
        correction: Some("Try using a hide clause to hide '{1}'."),
        dtype: DiagnosticType::StaticWarning,
        has_published: true,
    };

/// A constant's value recursively depends on itself — registry row verbatim
/// (`diagnostic.g.dart:14376`, dart 3.13.7). `COMPILE_TIME_ERROR`,
/// published, no correction; one report at each **variable** on the cycle
/// (`constant/evaluation.dart:433-459` `generateCycleError`; constructors on
/// a cycle report elsewhere). The cycle face is
/// [`crate::cycles::dependency_cycle`].
pub const RECURSIVE_COMPILE_TIME_CONSTANT: ResolutionCodeInfo = ResolutionCodeInfo {
    name: "recursive_compile_time_constant",
    machine: "RECURSIVE_COMPILE_TIME_CONSTANT",
    message: "The compile-time constant expression depends on itself.",
    correction: None,
    dtype: DiagnosticType::CompileTimeError,
    has_published: true,
};

/// A generative constructor redirects to itself directly or indirectly —
/// registry row verbatim (`diagnostic.g.dart:14404`, dart 3.13.7).
/// `COMPILE_TIME_ERROR`, published; reported at the redirecting initializer
/// when the successor walk from the constructor returns to **that
/// constructor** (`error_verifier.dart:6831-6852`
/// `_checkForRecursiveConstructorRedirect` over
/// `_hasRedirectingFactoryConstructorCycle`'s visited-set walk — a walk that
/// enters a cycle not containing the start stays silent). The face is
/// [`crate::cycles::cycle_through`].
pub const RECURSIVE_CONSTRUCTOR_REDIRECT: ResolutionCodeInfo = ResolutionCodeInfo {
    name: "recursive_constructor_redirect",
    machine: "RECURSIVE_CONSTRUCTOR_REDIRECT",
    message: "Constructors can't redirect to themselves either directly or indirectly.",
    correction: Some("Try changing one of the constructors in the loop to not redirect."),
    dtype: DiagnosticType::CompileTimeError,
    has_published: true,
};

/// An interface declaration is a superinterface of itself through a cycle —
/// registry row verbatim (`diagnostic.g.dart:14465`, dart 3.13.7).
/// `COMPILE_TIME_ERROR`, published, no correction; `{0}` the declaration's
/// display name, `{1}` the cycle members joined `', '`. The generic clause
/// form reports when no single clause names the element itself
/// (`inheritance_override.dart:705-790` `_checkForRecursiveInterfaceInheritance`
/// over `interfaceCycle`, computed as the Tarjan SCC in discovery-reversed
/// pop order, `summary2/interface_cycles.dart` +
/// `_fe_analyzer_shared/dependency_walker.dart`). The face is
/// [`crate::cycles::StronglyConnected::cyclic_component`].
///
/// The four clause-specific variants below share this row's code **name**
/// (the registry rows are all `recursive_interface_inheritance`; they differ
/// only in `uniqueName` and message — `diagnostic.g.dart:14489/14513/14529/
/// 14553`), so the (severity, code, file) tuple is this row's for the whole
/// family.
pub const RECURSIVE_INTERFACE_INHERITANCE: ResolutionCodeInfo = ResolutionCodeInfo {
    name: "recursive_interface_inheritance",
    machine: "RECURSIVE_INTERFACE_INHERITANCE",
    message: "'{0}' can't be a superinterface of itself: {1}.",
    correction: None,
    dtype: DiagnosticType::CompileTimeError,
    has_published: true,
};

/// The `extends`-clause variant of [`RECURSIVE_INTERFACE_INHERITANCE`] —
/// `uniqueName` `recursive_interface_inheritance_extends`, registry row
/// verbatim (`diagnostic.g.dart:14489`, dart 3.13.7). Same code name and
/// machine form as the base row; the direct `extends Self` shape.
pub const RECURSIVE_INTERFACE_INHERITANCE_EXTENDS: ResolutionCodeInfo = ResolutionCodeInfo {
    name: "recursive_interface_inheritance",
    machine: "RECURSIVE_INTERFACE_INHERITANCE",
    message: "'{0}' can't extend itself.",
    correction: None,
    dtype: DiagnosticType::CompileTimeError,
    has_published: true,
};

/// The `implements`-clause variant of [`RECURSIVE_INTERFACE_INHERITANCE`] —
/// `uniqueName` `recursive_interface_inheritance_implements`, registry row
/// verbatim (`diagnostic.g.dart:14513`, dart 3.13.7).
pub const RECURSIVE_INTERFACE_INHERITANCE_IMPLEMENTS: ResolutionCodeInfo = ResolutionCodeInfo {
    name: "recursive_interface_inheritance",
    machine: "RECURSIVE_INTERFACE_INHERITANCE",
    message: "'{0}' can't implement itself.",
    correction: None,
    dtype: DiagnosticType::CompileTimeError,
    has_published: true,
};

/// The mixin `on`-clause variant of [`RECURSIVE_INTERFACE_INHERITANCE`] —
/// `uniqueName` `recursive_interface_inheritance_on`, registry row verbatim
/// (`diagnostic.g.dart:14529`, dart 3.13.7).
pub const RECURSIVE_INTERFACE_INHERITANCE_ON: ResolutionCodeInfo = ResolutionCodeInfo {
    name: "recursive_interface_inheritance",
    machine: "RECURSIVE_INTERFACE_INHERITANCE",
    message: "'{0}' can't use itself as a superclass constraint.",
    correction: None,
    dtype: DiagnosticType::CompileTimeError,
    has_published: true,
};

/// The `with`-clause variant of [`RECURSIVE_INTERFACE_INHERITANCE`] —
/// `uniqueName` `recursive_interface_inheritance_with`, registry row
/// verbatim (`diagnostic.g.dart:14553`, dart 3.13.7).
pub const RECURSIVE_INTERFACE_INHERITANCE_WITH: ResolutionCodeInfo = ResolutionCodeInfo {
    name: "recursive_interface_inheritance",
    machine: "RECURSIVE_INTERFACE_INHERITANCE",
    message: "'{0}' can't use itself as a mixin.",
    correction: None,
    dtype: DiagnosticType::CompileTimeError,
    has_published: true,
};

/// A non-const top-level declaration's inferred type depends on itself
/// through a cycle — registry row verbatim (`diagnostic.g.dart:16012`, dart
/// 3.13.7). `COMPILE_TIME_ERROR`, published; `{0}` the variable's name,
/// `{1}` the cycle's member names joined `', '` (sorted,
/// `top_level_inference.dart:271-273`). Const variables are excluded — they
/// report [`RECURSIVE_COMPILE_TIME_CONSTANT`] instead
/// (`resolver.dart:4552-4572` `_checkTopLevelCycle`). The face is
/// [`crate::cycles::dependency_cycle`].
pub const TOP_LEVEL_CYCLE: ResolutionCodeInfo = ResolutionCodeInfo {
    name: "top_level_cycle",
    machine: "TOP_LEVEL_CYCLE",
    message: concat!(
        "The type of '{0}' can't be inferred because it depends on itself ",
        "through the cycle: {1}."
    ),
    correction: Some(
        "Try adding an explicit type to one or more of the variables in the \
         cycle in order to break the cycle.",
    ),
    dtype: DiagnosticType::CompileTimeError,
    has_published: true,
};

impl ResolutionCode {
    /// The frozen official descriptor of this code. Correction texts, families,
    /// and published flags are verbatim from the cited `diagnostic.g.dart` rows
    /// (dart 3.15 main). Length is the frozen table itself — a data table, not
    /// logic.
    #[must_use]
    #[expect(
        clippy::too_many_lines,
        reason = "the body is the frozen descriptor table transcribed from diagnostic.g.dart; \
                  its length is data, not logic"
    )]
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

    /// The frozen table: the sixteen rows less `unsupported_dart_ext`,
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

    /// Registry-true descriptors of the directive-combinator companion rows
    /// (pinned live registry: `undefinedHiddenName` diagnostic.g.dart:16936,
    /// `undefinedShownName` :17198 — both `STATIC_WARNING`, published, with
    /// corrections; messages oracle-verified against dart 3.13.1).
    #[test]
    fn combinator_rows_are_verbatim_oracle_descriptors() {
        assert_eq!(UNDEFINED_HIDDEN_NAME.name, "undefined_hidden_name");
        assert_eq!(UNDEFINED_HIDDEN_NAME.machine, "UNDEFINED_HIDDEN_NAME");
        assert_eq!(
            UNDEFINED_HIDDEN_NAME.message,
            "The library '{0}' doesn't export a member with the hidden name '{1}'."
        );
        assert_eq!(
            UNDEFINED_HIDDEN_NAME.correction,
            Some("Try removing the name from the list of hidden members.")
        );
        assert_eq!(UNDEFINED_HIDDEN_NAME.dtype, DiagnosticType::StaticWarning);
        const {
            assert!(UNDEFINED_HIDDEN_NAME.has_published);
        }

        assert_eq!(UNDEFINED_SHOWN_NAME.name, "undefined_shown_name");
        assert_eq!(UNDEFINED_SHOWN_NAME.machine, "UNDEFINED_SHOWN_NAME");
        assert_eq!(
            UNDEFINED_SHOWN_NAME.message,
            "The library '{0}' doesn't export a member with the shown name '{1}'."
        );
        assert_eq!(
            UNDEFINED_SHOWN_NAME.correction,
            Some("Try removing the name from the list of shown members.")
        );
        assert_eq!(UNDEFINED_SHOWN_NAME.dtype, DiagnosticType::StaticWarning);
        const {
            assert!(UNDEFINED_SHOWN_NAME.has_published);
        }
    }

    /// Registry-true descriptors of the duplicate-directive rows (pinned live
    /// registry: `duplicateHiddenName` diagnostic.g.dart:4584,
    /// `duplicateShownName` :4757, `duplicateImport` :4619,
    /// `duplicateExport` :4537 — all four `STATIC_WARNING`, published, with
    /// corrections; texts oracle-verified against the pinned dart 3.15 main
    /// registry).
    #[test]
    fn duplicate_rows_are_verbatim_oracle_descriptors() {
        assert_eq!(DUPLICATE_HIDDEN_NAME.name, "duplicate_hidden_name");
        assert_eq!(DUPLICATE_HIDDEN_NAME.machine, "DUPLICATE_HIDDEN_NAME");
        assert_eq!(DUPLICATE_HIDDEN_NAME.message, "Duplicate hidden name.");
        assert_eq!(
            DUPLICATE_HIDDEN_NAME.correction,
            Some("Try removing the repeated name from the list of hidden members.")
        );
        assert_eq!(DUPLICATE_HIDDEN_NAME.dtype, DiagnosticType::StaticWarning);
        const {
            assert!(DUPLICATE_HIDDEN_NAME.has_published);
        }

        assert_eq!(DUPLICATE_SHOWN_NAME.name, "duplicate_shown_name");
        assert_eq!(DUPLICATE_SHOWN_NAME.machine, "DUPLICATE_SHOWN_NAME");
        assert_eq!(DUPLICATE_SHOWN_NAME.message, "Duplicate shown name.");
        assert_eq!(
            DUPLICATE_SHOWN_NAME.correction,
            Some("Try removing the repeated name from the list of shown members.")
        );
        assert_eq!(DUPLICATE_SHOWN_NAME.dtype, DiagnosticType::StaticWarning);
        const {
            assert!(DUPLICATE_SHOWN_NAME.has_published);
        }

        assert_eq!(DUPLICATE_IMPORT.name, "duplicate_import");
        assert_eq!(DUPLICATE_IMPORT.machine, "DUPLICATE_IMPORT");
        assert_eq!(DUPLICATE_IMPORT.message, "Duplicate import.");
        assert_eq!(
            DUPLICATE_IMPORT.correction,
            Some("Try removing all but one import of the library.")
        );
        assert_eq!(DUPLICATE_IMPORT.dtype, DiagnosticType::StaticWarning);
        const {
            assert!(DUPLICATE_IMPORT.has_published);
        }

        assert_eq!(DUPLICATE_EXPORT.name, "duplicate_export");
        assert_eq!(DUPLICATE_EXPORT.machine, "DUPLICATE_EXPORT");
        assert_eq!(DUPLICATE_EXPORT.message, "Duplicate export.");
        assert_eq!(
            DUPLICATE_EXPORT.correction,
            Some("Try removing all but one export of the library.")
        );
        assert_eq!(DUPLICATE_EXPORT.dtype, DiagnosticType::StaticWarning);
        const {
            assert!(DUPLICATE_EXPORT.has_published);
        }
    }

    /// Registry-true descriptors of the export/re-export graph family,
    /// verified against the pinned oracle registry (dart 3.13.7
    /// `diagnostic.g.dart`; rows: ambiguousExport 264, deferredImportOf-
    /// Extension 3628, exportInternalLibrary 5332, invalidExportOfInternal-
    /// Element 8644, invalidExportOfInternalElementIndirectly 8666,
    /// recursiveCompileTimeConstant 14376, recursiveConstructorRedirect
    /// 14404, recursiveInterfaceInheritance 14465 + clause variants 14489 /
    /// 14513 / 14529 / 14553, topLevelCycle 16012). All published;
    /// `invalid_export_of_internal_element`(_indirectly) are
    /// `STATIC_WARNING`, the rest `COMPILE_TIME_ERROR`.
    #[test]
    fn export_graph_rows_are_verbatim_oracle_descriptors() {
        assert_eq!(
            ResolutionCode::AmbiguousExport.info().message,
            "The name '{0}' is defined in the libraries '{1}' and '{2}'."
        );
        assert_eq!(
            ResolutionCode::AmbiguousExport.info().dtype,
            DiagnosticType::CompileTimeError
        );

        assert_eq!(
            EXPORT_INTERNAL_LIBRARY.message,
            "The library '{0}' is internal and can't be exported."
        );
        assert_eq!(EXPORT_INTERNAL_LIBRARY.correction, None);
        assert_eq!(
            EXPORT_INTERNAL_LIBRARY.dtype,
            DiagnosticType::CompileTimeError
        );

        assert_eq!(
            DEFERRED_IMPORT_OF_EXTENSION.message,
            "Deferred library imports must hide all extension declarations."
        );
        assert_eq!(
            DEFERRED_IMPORT_OF_EXTENSION.correction,
            Some(
                "Try adding either a show combinator listing the names you need to \
                 reference or a hide combinator listing all of the extension \
                 declarations."
            )
        );
        assert_eq!(
            DEFERRED_IMPORT_OF_EXTENSION.dtype,
            DiagnosticType::CompileTimeError
        );

        assert_eq!(
            INVALID_EXPORT_OF_INTERNAL_ELEMENT.message,
            "The member '{0}' can't be exported as a part of a package's public API."
        );
        assert_eq!(
            INVALID_EXPORT_OF_INTERNAL_ELEMENT.correction,
            Some("Try using a hide clause to hide '{0}'.")
        );
        assert_eq!(
            INVALID_EXPORT_OF_INTERNAL_ELEMENT.dtype,
            DiagnosticType::StaticWarning
        );

        assert_eq!(
            INVALID_EXPORT_OF_INTERNAL_ELEMENT_INDIRECTLY.message,
            "The member '{0}' can't be exported as a part of a package's public \
             API, but is indirectly exported as part of the signature of '{1}'."
        );
        assert_eq!(
            INVALID_EXPORT_OF_INTERNAL_ELEMENT_INDIRECTLY.correction,
            Some("Try using a hide clause to hide '{1}'.")
        );
        assert_eq!(
            INVALID_EXPORT_OF_INTERNAL_ELEMENT_INDIRECTLY.dtype,
            DiagnosticType::StaticWarning
        );

        assert_eq!(
            RECURSIVE_COMPILE_TIME_CONSTANT.message,
            "The compile-time constant expression depends on itself."
        );
        assert_eq!(RECURSIVE_COMPILE_TIME_CONSTANT.correction, None);

        assert_eq!(
            RECURSIVE_CONSTRUCTOR_REDIRECT.message,
            "Constructors can't redirect to themselves either directly or indirectly."
        );
        assert_eq!(
            RECURSIVE_CONSTRUCTOR_REDIRECT.correction,
            Some("Try changing one of the constructors in the loop to not redirect.")
        );

        assert_eq!(
            TOP_LEVEL_CYCLE.message,
            "The type of '{0}' can't be inferred because it depends on itself \
             through the cycle: {1}."
        );
        assert_eq!(
            TOP_LEVEL_CYCLE.correction,
            Some(
                "Try adding an explicit type to one or more of the variables in the \
                 cycle in order to break the cycle."
            )
        );

        for row in [
            EXPORT_INTERNAL_LIBRARY,
            DEFERRED_IMPORT_OF_EXTENSION,
            INVALID_EXPORT_OF_INTERNAL_ELEMENT,
            INVALID_EXPORT_OF_INTERNAL_ELEMENT_INDIRECTLY,
            RECURSIVE_COMPILE_TIME_CONSTANT,
            RECURSIVE_CONSTRUCTOR_REDIRECT,
            RECURSIVE_INTERFACE_INHERITANCE,
            RECURSIVE_INTERFACE_INHERITANCE_EXTENDS,
            RECURSIVE_INTERFACE_INHERITANCE_IMPLEMENTS,
            RECURSIVE_INTERFACE_INHERITANCE_ON,
            RECURSIVE_INTERFACE_INHERITANCE_WITH,
            TOP_LEVEL_CYCLE,
        ] {
            assert!(row.has_published, "{} must be published", row.name);
            assert_eq!(row.machine, row.name.to_uppercase(), "{}", row.name);
        }
        assert!(ResolutionCode::AmbiguousExport.info().has_published);
    }

    /// The `recursive_interface_inheritance` clause family shares one code
    /// name (the registry rows differ only in `uniqueName` and message —
    /// dart 3.13.7 `diagnostic.g.dart:14465/14489/14513/14529/14553`), so
    /// the (severity, code, file) parity tuple is identical across the five
    /// messages and every row is `COMPILE_TIME_ERROR` with no correction.
    #[test]
    fn interface_inheritance_family_shares_one_code_name() {
        let family = [
            RECURSIVE_INTERFACE_INHERITANCE,
            RECURSIVE_INTERFACE_INHERITANCE_EXTENDS,
            RECURSIVE_INTERFACE_INHERITANCE_IMPLEMENTS,
            RECURSIVE_INTERFACE_INHERITANCE_ON,
            RECURSIVE_INTERFACE_INHERITANCE_WITH,
        ];
        for row in family {
            assert_eq!(row.name, "recursive_interface_inheritance");
            assert_eq!(row.machine, "RECURSIVE_INTERFACE_INHERITANCE");
            assert_eq!(row.dtype, DiagnosticType::CompileTimeError);
            assert_eq!(row.correction, None);
        }
        assert_eq!(
            RECURSIVE_INTERFACE_INHERITANCE.message,
            "'{0}' can't be a superinterface of itself: {1}."
        );
        assert_eq!(RECURSIVE_INTERFACE_INHERITANCE_EXTENDS.message, "'{0}' can't extend itself.");
        assert_eq!(
            RECURSIVE_INTERFACE_INHERITANCE_IMPLEMENTS.message,
            "'{0}' can't implement itself."
        );
        assert_eq!(
            RECURSIVE_INTERFACE_INHERITANCE_ON.message,
            "'{0}' can't use itself as a superclass constraint."
        );
        assert_eq!(
            RECURSIVE_INTERFACE_INHERITANCE_WITH.message,
            "'{0}' can't use itself as a mixin."
        );
    }
}
