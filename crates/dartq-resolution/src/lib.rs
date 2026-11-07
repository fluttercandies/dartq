//! URI, import, export, part, and conditional import resolution with
//! namespace computation.
//!
//! The crate carries the URI layer (M1 ticket #65) and the namespace/graph
//! layer (M1 ticket #117):
//!
//! - [`uri`] — directive URI parsing/validation under Dart `Uri.parse`
//!   rules, relative composition against the container's canonical URI,
//!   scheme dispatch (`dart:`/`package:`/`file:`), interning to
//!   [`dartq_source::FileId`], and the [`ResolvedTarget`] projection.
//! - [`conditional`] — conditional-URI selection for configurable
//!   import/export directives: first-match-wins selection over
//!   the declared-variables map with the oracle's primary-URI fallback.
//! - [`doc_import`] — `@docImport` directive extraction from documentation
//!   comments (line-splitting, code-block swallowing, import-text string
//!   literals), feeding the [`uri`]'s doc-import resolution face.
//! - [`library_graph`] — interned library nodes keyed by canonical URI,
//!   import/export edges, and the eager export-scope fixpoint pinned to the
//!   analyzer's worklist behavior (locals beat re-exports, conflicting
//!   re-exports resolve first-come-wins), plus the M2 cross-library
//!   modifier facts (per-class declared modifiers and the
//!   name → declaring-libraries index).
//! - [`namespace`] — the query-time namespace combinator over compact
//!   per-library import tables (CSR rows, no merged maps), composing the
//!   `Found`/`Ambiguous`/`NotFound` lookup face the `undefined_*`
//!   diagnostics consume; oracle chain order locals → prefixes → imports
//!   first-come-wins.
//! - [`export_graph`] — the export/re-export graph diagnostic faces over
//!   the frozen graph: `ambiguous_export`'s cross-directive seen-map,
//!   `duplicate_export`'s sorted adjacent-pair walk, the
//!   `export_internal_library` and `invalid_export_of_internal_element`
//!   gates, the `deferred_import_of_extension` namespace probe, and the
//!   `shared_deferred_prefix` per-prefix import grouping.
//! - [`cycles`] — the visited-set-bounded graph walks behind the element
//!   recursion codes: self-reachability (`recursive_constructor_redirect`),
//!   the on-stack dependency slice (`top_level_cycle`,
//!   `recursive_compile_time_constant`), and Tarjan cyclic components in
//!   the oracle's pop order (`recursive_interface_inheritance`).
//! - [`codes`] — the frozen resolution diagnostic code table.
//! - [`modifiers`] — the M2 class-modifier decisions (ticket #281):
//!   declaration-site combination legality, cross-library use-site
//!   restrictions, the base/final subtype requirement, induced modifiers,
//!   and the official diagnostic code descriptors — pure functions over
//!   resolved facts, ready for the semantic layer to wire to diagnostics.
//! - [`file_role`] — the file-kind classification and part-of role matrix
//!   (ticket #2981, the #119 file-role boundary): the role a file's
//!   directives give it, the SDK-content probe for external `dart:`
//!   targets, and the pure include-side row selection the part-family
//!   emitters consume.
//!
//! The part/part-of emitter wiring (`part_of_*`, `duplicate_part` rows into
//! diagnostics) lives with the engine's pipeline.

pub mod codes;
pub mod conditional;
pub mod cycles;
pub mod doc_import;
pub mod export_graph;
pub mod file_role;
pub mod library_graph;
pub mod modifiers;
pub mod namespace;
pub mod uri;

pub use codes::{
    DEFERRED_IMPORT_OF_EXTENSION, DUPLICATE_EXPORT, DUPLICATE_HIDDEN_NAME, DUPLICATE_IMPORT,
    DUPLICATE_SHOWN_NAME, EXPORT_INTERNAL_LIBRARY, INVALID_EXPORT_OF_INTERNAL_ELEMENT,
    INVALID_EXPORT_OF_INTERNAL_ELEMENT_INDIRECTLY, RECURSIVE_COMPILE_TIME_CONSTANT,
    RECURSIVE_CONSTRUCTOR_REDIRECT, RECURSIVE_INTERFACE_INHERITANCE,
    RECURSIVE_INTERFACE_INHERITANCE_EXTENDS, RECURSIVE_INTERFACE_INHERITANCE_IMPLEMENTS,
    RECURSIVE_INTERFACE_INHERITANCE_ON, RECURSIVE_INTERFACE_INHERITANCE_WITH, ResolutionCode,
    ResolutionCodeInfo, TOP_LEVEL_CYCLE, UNDEFINED_HIDDEN_NAME, UNDEFINED_SHOWN_NAME,
};
pub use conditional::{Configuration, SelectedUri, resolve_conditional};
pub use cycles::{StronglyConnected, cycle_through, dependency_cycle, strongly_connected};
pub use doc_import::{CommentLexeme, DocComment, DocImportDirective, doc_imports};
pub use export_graph::{
    AmbiguousExportFinding, InternalExportFinding, is_internal_sdk_library,
};
pub use file_role::{FileRole, PartOfRow, part_of_row};
pub use uri::{SdkFileKind, SdkFileKinds, canonical_dart_uri};
pub use library_graph::{
    CombinatorClause, Combinators, DuplicateCombinatorName, ExportEdge, ExportEntry, FaceKind,
    ImportEdge, LibraryGraph, LibraryGraphBuilder, LibraryId, LocalLookup,
    UndefinedCombinatorName,
};
pub use modifiers::{
    ClassModifiers, DeclKind, DeclarationViolation, ExplicitBaseFinal, ImplementsStep,
    InducedModifier, LibraryClassModifiers, LibraryFeatureFacts, ModifierCode, ModifierCodeInfo,
    ModifierHierarchy, ModifierKeyword, Relation, SubtypeRestrictionInput, SuperFacts,
    base_implemented_candidate, declaration_violations, final_extends_violation,
    final_implements_candidate, final_mixin_constraint_violation, induced_modifier,
    interface_extends_violation, may_ignore_class_modifiers, nearest_explicit_base_or_final,
    sealed_supertype_violation, subtype_restriction_violation,
};
pub use namespace::{Lookup, NamespaceEntry, Namespaces};
pub use uri::{
    DirectiveKind, PackageTable, RawUri, ResolvedTarget, ResolvedUri, UnresolvedKind,
    UriResolution, UriResolver, compose_canonical_uri, parse_directive_uri,
};
