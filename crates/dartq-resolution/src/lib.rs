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
//! - [`codes`] — the frozen resolution diagnostic code table.
//! - [`modifiers`] — the M2 class-modifier decisions (ticket #281):
//!   declaration-site combination legality, cross-library use-site
//!   restrictions, the base/final subtype requirement, induced modifiers,
//!   and the official diagnostic code descriptors — pure functions over
//!   resolved facts, ready for the semantic layer to wire to diagnostics.
//!
//! The part/part-of family (`part_of_*`, `duplicate_part`) and its file-role
//! graph remain ticket #119.

pub mod codes;
pub mod conditional;
pub mod library_graph;
pub mod modifiers;
pub mod namespace;
pub mod uri;

pub use codes::{
    DUPLICATE_EXPORT, DUPLICATE_HIDDEN_NAME, DUPLICATE_IMPORT, DUPLICATE_SHOWN_NAME,
    ResolutionCode, ResolutionCodeInfo, UNDEFINED_HIDDEN_NAME, UNDEFINED_SHOWN_NAME,
};
pub use conditional::{Configuration, SelectedUri, resolve_conditional};
pub use library_graph::{
    CombinatorClause, Combinators, DuplicateCombinatorName, ExportEdge, ExportEntry, ImportEdge,
    LibraryGraph, LibraryGraphBuilder, LibraryId, LocalLookup, UndefinedCombinatorName,
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
