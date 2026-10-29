//! The semantic element model: what each compilation unit declares, and the
//! per-library member enumeration over it (ticket #119, implementing the
//! four-essential model adjudicated in #131).
//!
//! # Layout
//!
//! * [`model`] — the per-unit element model: top-level name set with
//!   micro-kind tags, class member sets with static/instance bits, direct
//!   superclass name edges, per-executable formal tables (positional +
//!   named, with declared types), declared types (fields, variables, return
//!   types), type-parameter counts, and the capture-time arity log.
//! * `capture` — the shared written-type capture (`capture::TypeTree`,
//!   `capture::TreeSink`): one parse-event sink shape for the boundedness
//!   face's windows and the collector's declaration-annotation feeding
//!   (#728).
//! * [`collector`] — the collecting [`dartq_parser::ParserListener`]: consumes the
//!   parser's declaration-surface event stream into a model
//!   ([`collector::collect`]). The parser event stream is the stable face —
//!   no `dartq-ast` dependency (that crate's payload contract is under
//!   revision; a later lane re-bases this onto the arena AST).
//! * [`library`] — per-library enumeration wired to `dartq-resolution`
//!   (#117): the public scope face ([`library::LibraryScopes`]), the
//!   in-library scoped lookup ([`library::lookup_scoped`]), and the
//!   superclass-aware member walk ([`library::member_lookup`]).
//! * [`signature`] — the per-file API signature ([`ApiSignature`], #282):
//!   the canonical blake3 hash of the element face the L2 strong-incremental
//!   gate compares ([`FileElements::api_signature`]).
//! * [`persisted`] — the facts-snapshot payload projection of the model
//!   (#341): plain-data mirrors with names on a per-entry string slice, the
//!   compact columns the M3 cold-start load re-interns.
//! * [`codes`] — the M1 semantic code table: the frozen families with tier
//!   and verbatim oracle message.
//! * `const_tracker` — the engine's constant-pattern fold state
//!   (`const_tracker::ConstTracker`): the armed static type and anchor
//!   the diagnoser's literal events arm and the pattern's end consumes.
//! * `switch_tracker` — the engine's switch/pattern state
//!   (`switch_tracker::SwitchTracker`) and the name-level exhaustiveness
//!   world (`switch_tracker::NameWorld`, `switch_tracker::NameTy`) the
//!   diagnoser's pattern/switch events feed.
//! * `member_face` — the engine's member-face subsystem
//!   (`member_face::MemberFace`): the interned member/type spellings, the
//!   member-invocation latches (`==` operator parameter, `noSuchMethod`
//!   body shape, `@mustCallSuper`), and the member-spelling tables the
//!   diagnoser's member resolution reads.
//! * [`mod@diagnose`] — the M1 diagnosis engine (#120 batch 1 + #121 batch 2):
//!   a reference-collecting pass over the parser event stream that resolves
//!   every reference against #117/#119 and emits the implemented frozen
//!   codes — the `undefined_*`/`duplicate_definition` family plus the
//!   `wrong_number_of_*` family and the static-nullability four — recording
//!   every other miss as the index names-table rows
//!   ([`diagnose::FileReport::names`]). The analyzer
//!   language-strictness scalars (#480) consume at the verdict
//!   layer through [`diagnose_with`]: `strict-casts` rejects implicit
//!   downcasts (the ordinary assignability codes mint), `strict-inference`
//!   reports the untyped-declaration face of `inference_failure_on_*`, and
//!   `strict-raw-types` reports `strict_raw_type` — all off by default, the
//!   official posture.
//! * [`analysis`] — the semantic query face (#1056): one unit's fold
//!   result as an immutable, query-able projection
//!   ([`analysis::Analysis`], built by [`analysis::analyze_unit`] — the
//!   same fold [`diagnose::diagnose_unit`] runs, with the intermediates
//!   kept instead of dropped). Read-type/nullability, bare-reference
//!   bindings, constructor and member faces, and the reassignment
//!   tracking, answered by `&self` lookups over offset-sorted rows;
//!   invalidation is whole-snapshot replacement.
//! * [`use_face`] — the library-scope use-face substrate (#767): the
//!   per-unit observable-use record ([`use_face::UnitUseFace`],
//!   [`diagnose::diagnose_unit`]) and the per-library fold
//!   ([`use_face::LibraryUsages::close_library_usage`]) the
//!   unused-element/unused-import consumers and the M3 incremental layer
//!   close over.
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
//!   ([`FileElements::extension_faces`]) of the library's files first:
//!   extension application is name-level opaque, and its members are merged
//!   conservatively so that reporting stays under-reporting, never a false
//!   positive.
//! * Formal tables ([`TopDecl::formals`], [`MemberEntry::formals`]) are
//!   `Some` exactly when the executable declares a parameter list (`None` =
//!   no list — getters — not unknown). Positional formals keep declaration
//!   order with their declared types; named formals keep theirs.
//!   Only methods and top-level functions carry them: **constructors are
//!   outside the M1 model** (not in the adjudicated essentials).
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
//! [`dartq_resolution::LibraryId`] and current [`dartq_source::FileId`]
//! explicitly. Consequences: (a)
//! sibling-part private names are not merged into a part file's in-library
//! lookup (only its own declarations plus the public face); (b) the caller
//! folds part trees when calling `LibraryGraphBuilder::declare` — the face
//! and member queries here need no part information. The `part`/`part-of`
//! diagnostic family codes are frozen in
//! [`dartq_resolution::ResolutionCode`] (`PartOf*`) and remain with the
//! diagnostic tickets; when the parser's directive milestone lands, the
//! fold moves behind this API without changing its shape.

pub mod analysis;
pub mod annotation;
pub mod bounds;
pub(crate) mod capture;
pub(crate) mod clause_capture;
pub mod codes;
pub mod collector;
pub mod const_eval;
pub(crate) mod const_tracker;
pub mod diagnose;
pub mod library;
pub(crate) mod member_face;
pub mod model;
pub mod persisted;
pub mod resolved_ops;
pub mod signature;
pub(crate) mod switch_tracker;
pub mod ty_fold;
pub mod use_face;

pub use analysis::{Analysis, Binding, BindingKind, CtorBinding, MemberBinding, analyze_unit};
pub use clause_capture::{capture_clause_trees, extend_clause_trees};
pub use collector::{DeclarationCollector, collect, container_modifiers};
pub use diagnose::{
    FileReport, MemberRef, MemberRefKind, NameRow, NameUse, StrictModes, UnitFold, diagnose,
    diagnose_unit, diagnose_unit_with_ops, diagnose_with, duplicate_definition_diagnostics,
    fold_unit, fold_unit_with_ops,
};
pub use library::{
    ElementIndex, LibraryScopes, ScopeFace, ScopeKind, ScopedHit, lookup_scoped, member_lookup,
};
pub use model::{
    Container, ContainerKind, DuplicateDecl, FileElements, FnSig, MemberEntry, MemberKind,
    MicroKind, TopDecl,
};
pub use resolved_ops::{ClauseTrees, LibraryTypes, ResolvedTypeOps};
pub use signature::{ApiSignature, SIGNATURE_VERSION};
pub use use_face::{ExportUseFace, ImportUseFace, LibraryUsages, LibraryUseSet, UnitUseFace};
