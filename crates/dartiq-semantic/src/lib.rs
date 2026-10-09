//! The semantic element model: what each compilation unit declares, and the
//! per-library member enumeration over it (ticket #119, implementing the
//! four-essential model adjudicated in #131).
//!
//! # Layout
//!
//! * [`model`] — the per-unit element model: top-level name set with
//!   micro-kind tags, class member sets with static/instance bits, direct
//!   superclass name edges, and per-executable named-formal tables.
//! * [`collector`] — the collecting [`ParserListener`]: consumes the
//!   parser's declaration-surface event stream into a model
//!   ([`collector::collect`]). The parser event stream is the stable face —
//!   no `dartiq-ast` dependency (that crate's payload contract is under
//!   revision; a later lane re-bases this onto the arena AST).
//! * [`library`] — per-library enumeration wired to `dartiq-resolution`
//!   (#117): the public scope face ([`library::LibraryScopes`]), the
//!   in-library scoped lookup ([`library::lookup_scoped`]), and the
//!   superclass-aware member walk ([`library::member_lookup`]).
//!
//! # What the `undefined_*` / `wrong_number_*` tickets (#120/#121) can
//!   assume
//!
//! ## Names and spellings
//!
//! * Setters live under `name=` (members and top-level alike), matching the
//!   resolution layer. Look up assignment targets under `name=`.
//! * Top-level getters/setters are tagged [`MicroKind::TopLevelFn`] — the
//!   adjudicated kind set has no finer top-level tag, so a "not a function"
//!   verdict must not rest on a top-level kind alone.
//! * Duplicate names keep the first entry; `duplicate_*` diagnosis is the
//!   tickets' job and must not double-count here.
//! * Type parameters appear in the per-unit name set tagged
//!   [`MicroKind::TypeParam`]. They are declaration-scoped in truth; do not
//!   treat them as library-visible declarations (and skip them when feeding
//!   `LibraryGraphBuilder::declare`).
//! * Operator names keep their operator spelling (`==`).
//!
//! ## Lookup composition
//!
//! * Inside a library: [`library::lookup_scoped`] — current-file
//!   declarations first (private included), then the #117 namespace (locals
//!   beat imports; ambiguity surfaces as [`library::ScopedHit::Ambiguous`]).
//!   Report `undefined_*` only on [`library::ScopedHit::NotFound`].
//! * Across libraries: [`library::LibraryScopes::face`] — the public face.
//!   [`library::ScopeKind::External`] means "present, kind unknown": a name
//!   declared outside the analyzed file set (`dart:` SDK, unanalyzed
//!   dependencies). Suppress kind-sensitive codes on it — under-reporting
//!   is the M1 contract.
//! * Members: [`library::member_lookup`] walks superclass name edges
//!   (cycle-guarded, cross-file through the namespace). A `None` is **not**
//!   proof of undefined — check the extension pool
//!   ([`FileElements::extension_member`]) of the library's files first:
//!   extension application is name-level opaque, and its members are merged
//!   conservatively so that reporting stays under-reporting, never a false
//!   positive.
//! * Named formals ([`TopDecl::named_formals`],
//!   [`MemberEntry::named_formals`]) are `Some` exactly when the executable
//!   declares ≥ 1 named formal (`None` = definitely none, not unknown).
//!   Only methods and top-level functions carry them: **constructors are
//!   outside the M1 model** (not in the adjudicated essentials), so
//!   `wrong_number_of_*)` checks against constructor invocations cannot be
//!   served yet.
//!
//! ## Known gaps (each under-reports, never false-positives)
//!
//! * Enum synthesized members (`values`, `index`, …) are absent — suppress
//!   instance-scope undefined checks on enum-valued receivers accordingly.
//! * Mixin `on` constraints and `implements`/`with` faces are not edges;
//!   only direct superclasses are.
//! * Extension-type representation members are not captured.
//! * Function-body locals (variables, local functions) are not captured.
//!
//! ## The part/part-of seam
//!
//! The parser consumes directives diet-style and emits no `part` events
//! yet, so file→library assignment stays with the caller: pass the owning
//! [`LibraryId`] and current [`FileId`] explicitly. Consequences: (a)
//! sibling-part private names are not merged into a part file's in-library
//! lookup (only its own declarations plus the public face); (b) the caller
//! folds part trees when calling `LibraryGraphBuilder::declare` — the face
//! and member queries here need no part information. The `part`/`part-of`
//! diagnostic family codes are frozen in
//! [`dartiq_resolution::ResolutionCode`] (`PartOf*`) and remain with the
//! diagnostic tickets; when the parser's directive milestone lands, the
//! fold moves behind this API without changing its shape.

pub mod collector;
pub mod library;
pub mod model;

pub use collector::collect;
pub use library::{
    ElementIndex, LibraryScopes, ScopeFace, ScopeKind, ScopedHit, lookup_scoped, member_lookup,
};
pub use model::{
    Container, ContainerKind, FileElements, MemberEntry, MemberKind, MicroKind, TopDecl,
};
