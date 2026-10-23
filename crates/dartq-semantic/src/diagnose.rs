//! The M1 semantic diagnosis engine: the frozen `undefined_*` /
//! `duplicate_definition` family (batch 1, #120) plus the
//! `wrong_number_of_*` family and the static-nullability four (batch 2,
//! #121) — a pass over the parser's event stream that resolves every
//! reference a unit makes against the #117 lookup surface and the #119
//! element model, and emits the implemented frozen rows as [`Diagnostic`]s.
//!
//! # Architecture
//!
//! One parse with a reference-collecting listener (declaration capture stays
//! in [`crate::collector::collect`] — the model and the diagnostic surface
//! share the parser event stream, not the listener). The listener keeps an
//! expression row stack (the same shape the AST builder reconstructs from
//! these events): bare identifiers, member accesses merged at the dot event,
//! typed receivers (`this`/`super`/container names/declaration-typed names),
//! and per-row access intent (read / write / invoke / operator) finalized by
//! the send/assignment/operator events. Rows snapshot their resolution
//! verdict and declared type at push time (the frames tear down before
//! pass 2) and resolve in source order. Declaration-surface diagnostics —
//! `duplicate_definition` and the arity log — fold into the report from the
//! collector's capture-time logs.
//!
//! # Batch 2: arity and static nullability (#121)
//!
//! The type-argument family keys off `begin/end_type_arguments`, with the
//! code chosen by the use-site shape (the pinned oracle registry,
//! upstream Dart SDK source `diagnostic.g.dart`):
//! a pending *type head* (a `TypeReference` identifier that has not seen
//! its `handle_type` yet) makes the group an annotation or a
//! creation type reference → the base code; a head consumed inside an
//! enum constant → `_enum`; an expression head under `new`/`const`/
//! implicit-creation position → base for a resolved container, `_constructor`
//! for a named constructor segment; any other expression head defers to the
//! send or to the type-argument application event, both of which report
//! `_element` for a resolved executable — the message names the
//! executable's oracle kind word, `function` or `method`
//! (`type_instantiation_target.dart`,
//! `InvocationTargetExecutableElement`) — or `_extension` for extension
//! overrides. The `_function` row (a function-typed *expression*
//! instantiation) displays the function type, a shape M1 cannot render, so
//! it fires only where the captured method signature renders the type (the
//! property-access tear-off shape). Only resolvable *functions and methods*
//! produce `_element` counts — unresolved heads suppress, and the external
//! (`dart:`) annotation heads answer through the frozen
//! [`EXTERNAL_CORE_TYPE_PARAMS`] face. Getter- and field-headed
//! applications route to the `_function` row in the oracle
//! (`type_instantiation_target.dart`, `InvocationTargetFunctionTypedExpression`:
//! the head's static type is the getter's return type, and a function type
//! there is the function-typed-expression target) — the display needs the
//! function-type face the model cannot render, so they suppress: the
//! under-report direction.
//!
//! The call-site argument-count family (`extra_positional_arguments[_could_be_named]`
//! / `not_enough_positional_arguments`) rides the resolved-callee formals in
//! [`Engine::check_arguments`]: the parser's argument total less the named
//! entries decides the deficit against the required count and the surplus
//! against the positional capacity, mutually exclusive
//! (`resolveArgumentsToParameters`, upstream Dart SDK source,
//! `pkg/analyzer/lib/src/generated/resolver.dart` :5594-5725 in the corpus
//! registry). Unresolved callees and creation lists never reach the check;
//! a compound operand's window keeps the count exact and only degrades the
//! surplus anchor to the closing paren.
//!
//! The static-nullability checks decide on declared types and literal
//! types: the engine tracks the declared
//! type of locals, formals, fields, top-level variables, and return types
//! (armed by `handle_type`/`handle_void_keyword`, disarmed by
//! `handle_no_type` — every declaration head runs one `parse_type`, so the
//! arm cannot leak), and types expressions minimally: the `null` literal is
//! `Null`, the string/int/double/bool literals carry their final primitives,
//! a name with a captured declared type is that type, everything else is
//! `Unknown` — the oracle's `InvalidType`, which suppresses every question
//! it touches. The decidable set is the nullability decisions
//! (`null`/`T?` into a non-nullable target) plus the final-primitive pairs
//! (`crate::member_face::FINAL_PRIMITIVES`), and `?.`/`?[`/`?..` on a
//! strictly non-nullable
//! receiver (`invalid_null_aware_operator`, a `STATIC_WARNING`).
//! Assignment/initializer/argument/return checks apply only to single-row
//! expressions (one pushed row) so compound operands never lend their type
//! to the whole expression.
//!
//! # The inheritance-override family (#453)
//!
//! Name-level `invalid_override` (`inheritance_override.dart:498-546`
//! `_checkDeclaredMember`): each instance-member declaration of a class,
//! mixin, or enum is compared against the nearest same-named non-static
//! member of every direct super-face (the supertype, the `with` mixins in
//! application order, the `implements` interfaces, and a mixin's `on`
//! constraints). The verdict chain is kind equality, positional arity,
//! named-parameter set, then type comparability — a kind mismatch skips
//! (other checks own it) and an unresolvable or cyclic face chain
//! suppresses. The type faces decide in two hops: the M1 assignability
//! (nullability, the final-primitive edges), then the name lattice
//! (`crate::switch_tracker::NameWorld::decide_subtype`, the declared
//! and core subtype edges); only a pair neither hop can decide — an
//! unresolvable head or a cut walk — never reports, the under-report
//! direction. Emissions run from the parse listener off the completed element
//! model, anchored at the member's name span, once per failing face.
//! Signature texts in the message render the name-level shape we have
//! (`void Function(int)`-form); the oracle's instantiated function types
//! are a documented text divergence. Generic-method type-parameter
//! compatibility and getter/setter cross-shape conflicts under-report.
//!
//! # The abstract-completeness and interface-conflict families (#498)
//!
//! Both ride the #453 face machinery at container granularity, mounted at
//! the container's name event (`inheritance_override.dart:412-486` and
//! `:160-207`). *Completeness* (the five-tier
//! `non_abstract_class_inherits_abstract_member`): a concrete class or enum
//! reports every interface name without a concrete implementation, the
//! implemented sources being own members, the `with` mixins, and the super
//! chain — `implements` and `on` faces contribute interface members only
//! (`_addImplemented`). The exemptions mirror the official walk: `Object`
//! members, an enum's `values`/`values=`, own-declared names (the
//! `concrete_class_with_abstract_member` code owns those — under-report
//! here), names a concrete direct superclass carries, cross-library private
//! names (`isAccessibleFor`), and a concrete `noSuchMethod` anywhere on the
//! implemented sources (the forwarder gate). A name the candidate merge
//! dropped (a conflict) is absent from the interface — never reported
//! missing. Abstract members are captured at the declaration
//! (body-less/`abstract`); an abstract field's non-final shape misses both
//! its getter and its setter. *Conflicts* (`inconsistent_inheritance`
//! and `_getter_and_method`): candidates accumulate per name in the
//! official face order with a `with` mixin's own member replacing the
//! accumulated list; a getter/method kind mix is unresolvable and reports
//! even under a class-local declaration (oracle-verified), a same-kind
//! merge without a valid override reports unless the class declares the
//! name. A mixin declaration participates in the conflicts only. The
//! conflict rows anchor at the class name (the oracle moves the
//! getter/method row onto a local declaration when one exists — a span
//! divergence, metric-neutral on the (code, file) gate). Every
//! unresolvable face chain suppresses its container's whole check.
//!
//! # The flow-analysis mount (#449)
//!
//! Every flow-relevant listener event double-dispatches to a per-unit
//! [`dartq_flow::Flow`] recorder alongside the reference work — no second
//! pass, no engine state: the recorder is constructed in [`diagnose`],
//! lives only for the parse, and its [`dartq_flow::FlowFact`]s fold into
//! the report at teardown. The first consumed family is
//! [`SemanticCode::DeadCode`]: the fact layer folds each dead region
//! into one diagnostic spanning from the first dead node's anchor to
//! the covering construct's end (the `DeadWindow` mirror of
//! `dead_code_verifier.dart`'s `_firstDead`); the declaration anchors
//! remain a documented divergence (variable names, not the declaration
//! head). The
//! `UnassignedRead` facts feed the `cast_from_nullable_always_fails`
//! join (#746); the unassigned non-nullable-local compile error remains
//! unconsumed territory.

//! # The frozen-family emit matrix (batch 1 #120, batch 2 #197)
//!
//! The batch-1 core (bare names, type positions, prefixed invocations,
//! `duplicate_definition`) plus the batch-2 member family cover the frozen
//! set's live triggers. Every member emission keys off the access intent and
//! the receiver domain, mirroring the oracle's single-point dispatch
//! (upstream Dart SDK source `lookup_failure_reporter.dart`, domains ×
//! read/write × read-syntax):
//!
//! * instance receivers (`c.m` on a `C c`, `this.m`) report
//!   `undefined_method` / `undefined_getter` / `undefined_setter`
//!   (`InstanceLookupDomain`); a write survives when any same-named
//!   declaration exists (the oracle's assignment-to-* family) or a setter
//!   spelling (`name=`) defines it;
//! * the static face (`C.m`, `E.x`, `Ext.g`) reports the same interface
//!   trio — the type-literal invocation shape shares the `undefined_method`
//!   registry name and message — plus `undefined_enum_constant` on enum
//!   reads, and the extension trio on the extension face
//!   (`StaticLookupDomain`);
//! * `super.member` reports `undefined_super_member` with the per-shape
//!   message (`SuperLookupDomain`; one analyzerCode, four texts);
//! * prefixed reads and writes report `undefined_prefixed_name`
//!   (`PrefixedLookupDomain`);
//! * a bare invocation miss inside a class or enum instance member reports
//!   `undefined_method` naming `this`'s type (`UnqualifiedLookupDomain`);
//! * a named argument label on a resolved callee reports
//!   `undefined_named_parameter` (`type_analyzer.dart:5820` — an unresolved
//!   callee stays silent, its own invocation code reports);
//! * `await` in a sync body reports `undefined_identifier_await`;
//! * user-definable operators report `undefined_operator` at the operator
//!   token (index accesses at the bracket pair) when the receiver's
//!   declared type — direct (`c + d`, `c[i]`) or a member result
//!   (`c.j + 1`, `c.j[i] = v`) — genuinely lacks the read/write spelling.
//!
//! Under-reported by adjudication (each emits no code): operator intents on
//! static faces and prefix receivers, index chains through unknown
//! receivers (`f()[i]`), and `this[i]`-style accesses inside extension and
//! mixin bodies, whose `on` faces the model cannot see.
//!
//! # The names table seam
//!
//! Unresolved names are a **byproduct of lookup**, written into
//! [`FileReport::names`] — the same rows the `dartq-index` names table wants
//! (`(name, kind, range, qualified)`), kept here until #122 mounts
//! `dartq-index` and the table moves behind it. Resolved references are not
//! recorded: they are the reference index's rows, not the recovery surface's.
//!
//! # The undefined guard (analyzer `shouldIgnoreUndefined`)
//!
//! `element.dart:8217-8250`: a name referenced through an import whose target
//! did not resolve is suppressed — but the filter face is show-list-only. The
//! mirror reads the graph's import edges (`target: None`) and suppresses a
//! name listed in a broken import's `show` clause, or a prefixed name whose
//! broken import has no `show` clause; a bare miss under an unfiltered or
//! hide-only broken import still reports ([`UndefinedGuard`]). The
//! `_$`-generated-part clause needs the part tree; the parser has no part
//! events yet, so it is deferred with the directive milestone (the clause
//! only ever suppresses). Harnesses model core-library names through
//! concrete declarations or shown broken edges.
//!
//! # False-positive defenses (each mirrors a verified analyzer behavior)
//!
//! * Locals, parameters, catch/pattern variables, and type parameters are
//!   scope-tracked in this pass (the model does not capture them) — a body
//!   reference to a local never reaches the library lookup.
//! * `dynamic` is a language builtin, not an import — never `undefined_class`.
//! * `await` reports `undefined_identifier_await` instead of the family's
//!   own code at both oracle sites: a bare read inside an executable
//!   (`property_element_resolver.dart:2143`) and the unresolved simple
//!   type name of a declaration parse (`named_type_resolver.dart:814-818`
//!   — `await x;` in a sync body keeps the declaration parse).
//! * `is`/`as` type operands and type arguments carry other oracle codes
//!   (`type_test_with_undefined_name`, `cast_to_non_type`,
//!   `non_type_as_type_argument`) — none frozen, so this pass under-reports
//!   them rather than emitting a wrong code.
//! * Member walks distinguish a cleanly walked chain (member genuinely
//!   absent) from a broken one (an unresolvable superclass — external type or
//!   mixin `on` constraint) and record only the former; enum synthesized
//!   members (`values`/`index`/`name`) and `Object` members are defined on
//!   every receiver shape the walk can reach.
//! * Member checks need a *declaration* receiver type (`this`, `super`, an
//!   extension or class name, or a name whose declared type is an indexed
//!   container — M1 deliberately under-reports inferred receivers). A named
//!   constructor is invisible to the model,
//!   so `@Class.member` second-part checks are suppressed for classes
//!   (extensions have no constructors and stay checked).
//! * Constructor invocations are outside the model: a class-name callee is
//!   defined without inspection, and `new`/`const`-position callees resolve
//!   as type references (the oracle's `undefined_class`), never invocations —
//!   their constructor-family codes stay deferred. Implicit creations
//!   (`C<T>(...)` without `new`) bracket the same suppression window.
//! * Locals leak only where the parser offers no frame pairing (switch-case
//!   bodies share the switch block's frame) — a leak can only mask a miss
//!   (under-report), never fabricate one.
//!
//! # The #350 gate closure (the remaining frozen rows)
//!
//! * Assignability trio: literals carry their final-primitive types and
//!   `Engine::assignable` extends [`Ty::assignable_from`] with the
//!   covariant written-argument rule and the
//!   final-primitive table (a value of one final primitive never fits
//!   another; `int`/`double` fit `num`; `int`→`double` is the oracle's
//!   literal conversion — `analyzeIntLiteral`'s `convertToDouble`, decided
//!   on the literal row, never on the value).
//! * `undefined_annotation`: a fully missing `MetadataReference` name,
//!   anchored over the `@` (the 3.13.1 caliber — removed upstream in 3.14).
//! * `undefined_constructor_in_initializer`: `super.named()` inside a
//!   constructor-initializer list checks the superclass's constructor face
//!   (captured per container: named selectors + the unnamed flag) and folds
//!   the row before pass 2's super-member dispatch can mis-report it. The
//!   F-C routing completes the face: only the argument-group shape decides
//!   (the `super().foo` recovery residue and the `super.name = v`
//!   field-initializer shape drop), `this.` selectors and bare LHS names
//!   fold into the initializer world (`init_rows`) instead of the member
//!   and `undefined_identifier` faces, and the plain assignment check
//!   suppresses inside the list.
//! * `undefined_enum_constructor`: an enum constant's constructor selector
//!   checks the enum's constructor face.
//! * The extension-override face: a container-callee send (`E('x')`,
//!   `C()`) folds into a `Receiver::Created` row, so member and operator
//!   accesses resolve against the instance-shaped face — extensions report
//!   their dedicated trio and operator row, class creations walk the
//!   superclass chain with the `Object` members defined.
//! * The external-primitive member face: a receiver type that resolves to
//!   no container but names a dart:core primitive checks
//!   `CORE_TYPE_MEMBERS` instead of suppressing.
//! * The `_function` tear-off row renders the function type from the
//!   captured member signature (type-parameter names joined the model);
//!   `C<...>.named`-shaped constructor references fall back to the base
//!   type-argument code. Both fire from the type-argument application
//!   event — the current parser consumes `id.id<...>` as a type head
//!   (#351's gap), so the emission activates when that parse lands.
//!
//! # Still deferred
//!
//! `undefined_referenced_parameter` (`@UseResult(unless:)` check,
//! `annotation_verifier.dart:519-541`) and the unnamed-selector message
//! variants of the two constructor rows. (`undefined_hidden_name` /
//! `undefined_shown_name` left this list with the directive milestone: the
//! engine pipeline emits them from the resolution graph's export scopes —
//! `dartq-engine` `diagnose_library` + `LibraryGraph::
//! undefined_combinator_names`.) `wrong_number_of_parameters_for_setter`
//! left this list with the parser's emitter landing: the oracle fires it at
//! the parse tier (`_ensureSetterFormalParameter`), dartq's parser now
//! mirrors that at the setter name, and the collector-side log entry is
//! retired (a capture-time row would double the parse row in the merged
//! report). Every deferral under-reports.
//!
//! # The use-face substrate (#767)
//!
//! The pass's use sets (`used_names`/`read_names`, plus resolved
//! annotation names and the import/export directive faces) are the unit's
//! observable-use record: [`diagnose_unit`] moves them into a
//! [`crate::use_face::UnitUseFace`] alongside the report, and the caller
//! folds the library's units through
//! [`crate::use_face::LibraryUsages::close_library_usage`] into the
//! per-library consumed set — the official gather-per-file /
//! merge-per-library pair (library_analyzer.dart:305-315). The recording
//! rides the sets the pass already kept (zero hot-path cost); the
//! per-file diagnostic behavior is unchanged.

use std::cell::RefCell;
use std::collections::VecDeque;

use dartq_diagnostics::{
    CodeInfo, ContextMessage, DARTQ_INTERNAL_ERROR, Diagnostic, DiagnosticType, Severity, SharedStr,
};
use dartq_flow::facts::{NonPromotionReason, PropertyNonPromotability};
use dartq_flow::ty::{Base as FlowBase, Ty as FlowTy};
use dartq_flow::{Flow, FlowFact};
use dartq_parser::{
    AssertKind, BlockKind, ConstructorReferenceContext, DeclarationKind, ExperimentalFlag,
    FormalParameterKind, IdentifierContext, Message, Parser, ParserFeatures, ParserListener,
    TokenSpan as Token,
};
use dartq_resolution::modifiers::{
    ClassModifiers, ExplicitBaseFinal, ImplementsStep, InducedModifier, ModifierCode,
    ModifierHierarchy, Relation, SubtypeRestrictionInput, SuperFacts, base_implemented_candidate,
    final_extends_violation, final_implements_candidate, final_mixin_constraint_violation,
    induced_modifier, interface_extends_violation, may_ignore_class_modifiers,
    nearest_explicit_base_or_final, sealed_supertype_violation, subtype_restriction_violation,
};
use dartq_resolution::{
    ImportEdge, LibraryClassModifiers, LibraryFeatureFacts, LibraryGraph, LibraryId, Lookup,
    NamespaceEntry, Namespaces,
};
use dartq_scanner::TokenTable;
use dartq_source::{FileId, SourceRange, StringId, StringTable};
use rustc_hash::{FxHashMap, FxHashSet};

use crate::annotation::is_meta_getter;
use crate::bounds::{Bounds, Clause, TypeTree};
use crate::codes::{ArgCountShape, ExecKind, SemanticCode, SuperShape};
use crate::collector::container_modifiers;
use crate::const_eval::{
    BinOp, ConstCapture, ConstFault, ConstLookup, ConstantValue, Expr, IdentVerdict, UnOp, Verdict,
    collect_not_potentially_constant, eval,
};
use crate::const_tracker::ConstTracker;
use crate::library::{
    ElementIndex, ScopedHit, implements_interface, lookup_scoped, member_lookup,
    member_lookup_defining, owner_library, prefixed_container, resolve_container,
    resolve_joined_container, supertype_hop_dead,
};
use crate::member_face::{
    COLLECTION_INTERFACE_ALIASES, CORE_CONST_CTOR, CORE_CONTAINER_PARAMS, CORE_INTERFACE_MEMBERS,
    CORE_STATIC_RETURNS, CORE_TYPE_MEMBERS, EqualsCheck, MemberFace, NsmCheck, NsmShape,
    OBJECT_GETTER_RETURNS, OBJECT_INSTANCE_RETURNS,
};
use crate::model::{
    ArgListId, ArgPool, ArityViolation, Container, ContainerKind, ExtensionFace, FileElements,
    Formal, Formals, MemberEntry, MemberKind, MicroKind, TopDecl, Ty,
};
use crate::switch_tracker::{
    NameCase, NameScope, NameTy, NameWorld, ObjectField, PatternFrame, Restriction, SwitchTracker,
    case_space, is_always_exhaustive, restriction_is_constant,
};
// The tests construct a `NameWorld` over the engine's interned core chain;
// the lib build's only consumer is `MemberFace::new`, so this import is
// test-only.
#[cfg(test)]
use crate::switch_tracker::CORE_SUPERTYPES;
use crate::use_face::{ExportUseFace, ImportUseFace, UnitUseFace};
use dartq_types::{
    Case, Exhaustiveness, ExhaustivenessWorld, Variance, Witness as EngineWitness, compute,
};

mod assignability;
mod const_eval_faces;
mod call_result;
mod collection_literal;
mod creation_args;
mod engine_core;
mod invocation;
mod index_result;
mod listener;
mod never_send;
mod pattern_scope;
mod primitive_lattice;
mod receiver_face;
mod send_result;
mod operand_face;
mod overrides;
mod resolution;
mod row_resolution;
mod scopes_bodies;
mod strict_faces;
mod type_capture;
pub(crate) mod types;
mod unused_finalization;

/// The W7 `non_const_call_to_literal_constructor` producer tests (#749).
/// Standalone module like the W6 rig: the firing face needs the vendored
/// meta stand-in (`package:meta/…` graph node declaring the `literal`
/// face), which the single-file rig cannot express.
///
/// Behavioral oracle: dart 3.13.1 `dart analyze --format=machine` probes
/// (see the fixture under tests/corpus/m2-warning for the live row).
/// Official source: best_practices_verifier.dart:1276-1290
/// (`_checkForLiteralConstructorUse`).
#[cfg(test)]
mod literal_constructor_tests;
/// The W6 `@sealed` supertype-contract producer tests (#702). Kept as a
/// standalone module so the shared `tests` rig stays untouched: the faces
/// here need a second analyzed library and the meta stand-in node, which
/// the single-file rig cannot express.
#[cfg(test)]
mod sealed_contract_tests;
#[cfg(test)]
mod tests;

/// The #2216-family producer pins (#2216/#2193/#2188 faces verified
/// end-to-end): kept as a standalone module so the shared `tests` rig
/// stays untouched while its lane is in flight.
#[cfg(test)]
mod ticket_2216_tests;

/// The #1351 faces pinned end-to-end: the function-type-alias boundedness
/// gate, the const-eval deferred-creation walk, the formals' reach into
/// the argument check, and the flow-fact joins (the nullable-condition
/// verdict and the matched-type re-decision).
#[cfg(test)]
mod ticket_1351_tests;

/// The undefined-name/resolution differential pins: constructor-reference
/// prefix heads (`fb.RuleState.stable()`), record `on`-type field getters,
/// and extension invoke-miss naming (the `on` type's `this`, an
/// `InvalidType` `this` stays silent). Kept as a standalone module so the
/// shared `tests` rig stays untouched while its lane is in flight.
#[cfg(test)]
mod ticket_undefined_family_tests;

/// The #2313 queued-interface-face pins: a supertype climb drains its
/// pending `implements` faces through each face's writer library —
/// `implements A, B` keeps both faces across multi-hop drains, and the
/// `@override entityName` sibling (drift `versioned_schema.dart`) spares.
#[cfg(test)]
mod ticket_2313_tests;

/// The #2316 recipe-(a) pins: a subclass's bare read of an inherited
/// field records against the declaring container, sparing `unused_field`
/// on `Base._f` while unread siblings and static-reads still report.
/// Kept as a standalone module so the shared `tests` rig stays untouched.
#[cfg(test)]
mod ticket_2316_tests;

/// The dead/unused-family differential pins: the `InvalidType` checked
/// type's `factor` remainder (the losing `is`/`is!` arm's `dead_code`),
/// `unused_field`'s `unresolvedReadMembers` spelling fallback and the
/// enum `values` credit, and the extension `on` resolvability gate on
/// `unused_element` member credits.
#[cfg(test)]
mod ticket_dead_family_tests;

/// The assignability-family differential pins (W3 real-corpus burn): the
/// `FutureOr<T>` union rule, the async matrix's Right Bottom leg, the
/// strict function-shape and covariant-argument slots, and the record
/// poison. Kept as a standalone module so the shared `tests` rig stays
/// untouched while its lane is in flight.
#[cfg(test)]
mod ticket_assignability_family_tests;

/// The #2243 promotion differential pins: an args-carrying promoted local
/// read on the return path, and the if-null assignment's constructor-RHS
/// promotion. Kept as a standalone module so the shared `tests` rig stays
/// untouched while its lane is in flight.
#[cfg(test)]
mod ticket_2243_tests;

/// The #2245 composite-argument pins: a comparison (`==`/`!=`/`&&`/`||`)
/// argument folds no single row, so the argument windows recover the
/// statically-`bool` composite face at its own extent instead of dropping
/// the window (the dropped window shifted every later argument onto the
/// wrong formal — the multiline `_checkAndThrow` misalignment). Kept as a
/// standalone module so the shared `tests` rig stays untouched.
#[cfg(test)]
mod ticket_2245_tests;

/// The #2241 unchecked-use pins: the member walk's receiver verdict
/// re-decides against the flow's staged variable promotions (the opaque
/// promoted head of a `var`-inferred nullable local), and the `== null`,
/// else-branch, and unguarded cells keep the report.
#[cfg(test)]
mod ticket_2241_tests;


/// The #2326 differential pins: the with-clause override producer wired
/// from the three with-clause listener handlers — the written NamedType
/// span anchors each `invalid_override` row (type arguments included),
/// the mixin field contributes both getter and setter faces, the
/// accumulated set is `[superclass] + prior mixins` (never
/// `implements`/`on`), and a foreign single-parameter mixin instantiates
/// `T ↦ XmlNode` at the application site.
#[cfg(test)]
mod ticket_2326_tests;

/// The const NON_CONSTANT-family differential pins (#2242): unresolved
/// callees inside const literals report the element faces at the
/// invocation and anchor the declaration row at the first failure, and a
/// qualified const creation (`C.named(...)`) validates its arguments
/// against the in-file const constructor. Kept as a standalone module so
/// the shared `tests` rig stays untouched.
#[cfg(test)]
mod ticket_const_family_tests;



/// The #2337 differential pins: the `super.name` formal's associated-type
/// face — a positional forward reads the super constructor's parameter at
/// its own index among the super formals (a field-formal parameter takes
/// its field's declared type, a chained forward climbs hop by hop, and a
/// same-named local field never wins), so a nullable argument at a
/// creation site reports against the associated non-nullable parameter
/// while the written annotation, the matching nullable parameter, and the
/// unassociated forward keep the silence — plus the error-recovery
/// operator faces that already stand (an invalid-chain receiver keeps the
/// binary node's fold unresolved; a known-typed `<<` operand reports the
/// argument face).
#[cfg(test)]
mod ticket_2337_tests;

/// The #2348 differential pins: the RETURN_OF_INVALID_TYPE over-fire
/// census classified per line — every d-only line's actual side carried a
/// recovery artifact whose pair an oracle-reported twin shares, so the
/// pins lock the earned twins the return gate must keep judging (no
/// pair-level suppression is sound) beside the P0 pool-guard panic fix
/// (`covariant_args`' narrow borrow) driving the full instantiated-alias
/// recursion to its verdict; the recovery faces themselves are the flow
/// promotion interpreter's, the resolution read rows', and the
/// implicit-`call` face's to fix.
#[cfg(test)]
mod ticket_2348_tests;

/// The #2328 differential pins: the chain-broken member/on-face climb and
/// the bare-name faces it must not own — the bare creation send resolves
/// through the scoped lookup instead of masquerading as an implicit-`this`
/// member, and an annotation name resolves through the scope alone, so its
/// miss reports `undefined_annotation` instead of the broken climb's
/// blanket suppression, while real member calls over chain-broken
/// receivers keep reporting.
#[cfg(test)]
mod ticket_2328_tests;

/// The #2362 differential pins: the generic-supertype instantiation's
/// whole-`Ty::Named` unwrap — `instantiate_inherited` binds each declaring
/// hop's parameter to the stored super face's own argument list, never to
/// the one-element wrapping, so a same-file `Sub<X> extends Box<X>` face
/// substitutes the written argument, a nested `Outer<Inner<E>>` argument
/// substitutes into the member, and the foreign name-forced hop
/// (`XmlNodeList<E> extends DelegatingList<E>`) stops false-firing on
/// every inherited member while a genuine mismatch across the edge keeps
/// reporting.
#[cfg(test)]
mod ticket_2362_tests;

pub(crate) use operand_face::OperandFace;
pub(crate) use types::*;
pub use types::{FileReport, NameRow, NameUse, RefTarget, UndefinedGuard, UnitRef};

/// Parses an int literal body: decimal or `0x` hex, `_` separators
/// stripped; `None` when the value exceeds `i64` (an unobservable edge —
/// the literal defers to an opaque constant).
pub(crate) fn parse_int_literal(text: &str) -> Option<i64> {
    let cleaned: String = text.chars().filter(|c| *c != '_').collect();
    let (radix, digits) = if let Some(rest) = cleaned
        .strip_prefix("0x")
        .or_else(|| cleaned.strip_prefix("0X"))
    {
        (16, rest)
    } else {
        (10, cleaned.as_str())
    };
    i64::from_str_radix(digits, radix).ok()
}

/// Substitutes a type-parameter map into a written type tree: every leaf
/// whose head names a parameter becomes a clone of its argument tree
/// (`B<X> = A<X>` binds `X` to the use site's tree). Named/record/function
/// nodes substitute their children; an empty map returns the tree as-is.
pub(crate) fn subst_tree(tree: &TypeTree, map: &FxHashMap<StringId, TypeTree>) -> TypeTree {
    if map.is_empty() {
        return tree.clone();
    }
    if let Some(replacement) = map.get(&tree.head)
        && tree.args.is_empty()
        && tree.record.is_none()
        && tree.function.is_none()
    {
        let mut out = replacement.clone();
        out.nullable |= tree.nullable;
        return out;
    }
    let mut out = tree.clone();
    out.args = tree.args.iter().map(|arg| subst_tree(arg, map)).collect();
    if let Some((positional, named)) = &tree.record {
        out.record = Some((
            positional.iter().map(|f| subst_tree(f, map)).collect(),
            named
                .iter()
                .map(|(n, f)| (*n, subst_tree(f, map)))
                .collect(),
        ));
    }
    if let Some(f) = &tree.function {
        out.function = Some(crate::capture::FnTree {
            ret: f.ret.as_ref().map(|r| Box::new(subst_tree(r, map))),
            type_params: f.type_params,
            positional: f.positional.iter().map(|p| subst_tree(p, map)).collect(),
            named: f
                .named
                .iter()
                .map(|(n, p)| (*n, subst_tree(p, map)))
                .collect(),
        });
    }
    out
}

/// The capture operator for a binary operator spelling (`??=` and
/// friends never reach `begin_binary_expression` as plain binaries).
#[expect(clippy::match_same_arms)] // "+" folds to Add — the wildcard's identity default
pub(crate) fn bin_op_of(text: &str) -> BinOp {
    match text {
        "+" => BinOp::Add,
        "-" => BinOp::Sub,
        "*" => BinOp::Mul,
        "/" => BinOp::Div,
        "~/" => BinOp::IntDiv,
        "%" => BinOp::Mod,
        "&" => BinOp::BitAnd,
        "|" => BinOp::BitOr,
        "^" => BinOp::BitXor,
        "<<" => BinOp::Shl,
        ">>" => BinOp::Shr,
        ">>>" => BinOp::Ushr,
        "<" => BinOp::Lt,
        ">" => BinOp::Gt,
        "<=" => BinOp::Le,
        ">=" => BinOp::Ge,
        "==" => BinOp::Eq,
        "!=" => BinOp::Ne,
        "&&" => BinOp::And,
        "||" => BinOp::Or,
        "??" => BinOp::IfNull,
        _ => BinOp::Add, // unreachable: the expression grammar's binary set
    }
}

/// The reference-collecting [`ParserListener`] feeding an owned [`Engine`]:
/// one parse over the same token table `collect` used, then pass 2 resolves
/// the rows. Every flow-relevant event double-dispatches to the per-unit
/// [`Flow`] recorder (the #449 fan-out), whose facts fold into the report
/// after the parse. The engine and the flow recorder are plain owned
/// fields — [`fold_unit`] keeps the listener in scope, lends it to the
/// parse through the multiplexer, and takes both back by value at
/// teardown.
pub(crate) struct DiagnoseListener<'a> {
    pub(crate) engine: Engine<'a>,
    pub(crate) flow: Flow<'a>,
    /// A resolved bottom face (`Flow::handle_exit`) not yet recorded: the
    /// send's exit token, held back so the send's own statement records first
    /// (see [`Self::flow`]).
    pub(crate) pending_exit: Option<Token>,
    /// The unit's file, stamped onto every parse-recovery row.
    pub(crate) file: FileId,
    /// The parse-recovery rows (`handle_recoverable_error` /
    /// `handle_experiment_not_enabled`), in arrival order. The production
    /// pipeline serves the identical rows from the l1 collector's facts
    /// (same tokens, same features, one parse), so [`diagnose_unit`]'s
    /// report stays clean; only [`UnitFold::parse_diagnostics`] carries
    /// them — the full-oracle set the unit pins merge ahead of the
    /// semantic rows.
    pub(crate) parse_diagnostics: Vec<Diagnostic>,
}

impl<'a> DiagnoseListener<'a> {
    /// The flow recorder accessor. A Never-typed send's `handle_exit` must
    /// not precede its own statement's wrapper in the recorded buffer: the
    /// parser fires the wrapper after the body (`;` only exists then), so
    /// the exit latches instead of recording, and the NEXT forward drains
    /// it first — its position relative to every other event stays the
    /// official one (the exit fires at the send's end), except across its
    /// own statement's wrapper, which now sees the reachable state and
    /// cannot open the dead window at the call's own statement.
    pub(crate) fn flow<'s>(&'s mut self) -> &'s mut Flow<'a> {
        self.flush_flow_exit();
        &mut self.flow
    }

    pub(crate) fn flush_flow_exit(&mut self) {
        if let Some(end) = self.pending_exit.take() {
            self.flow.handle_exit(end);
        }
    }
}

/// Stably merges two source-ordered diagnostic lists by range offset
/// (ties keep the base list's rows first).
pub(crate) fn merge_source_ordered(
    base: Vec<Diagnostic>,
    extra: Vec<Diagnostic>,
) -> Vec<Diagnostic> {
    let mut merged = Vec::with_capacity(base.len() + extra.len());
    let mut base = base.into_iter().peekable();
    let mut extra = extra.into_iter().peekable();
    loop {
        let take_base = match (base.peek(), extra.peek()) {
            (Some(b), Some(e)) => b.range.offset <= e.range.offset,
            (Some(_), None) => true,
            (None, Some(_)) => false,
            (None, None) => break,
        };
        if take_base {
            merged.push(base.next().expect("peeked"));
        } else {
            merged.push(extra.next().expect("peeked"));
        }
    }
    merged
}

/// The analyzer language-strictness scalars (`analysis_options.yaml`
/// `analyzer.language.*`), consumed at the semantic verdict
/// layer. Every mode defaults to `false` — the official posture
/// (`AnalysisOptionsImpl`, `analysis_options.dart:44-52`).
///
/// * `casts` — `strict-casts` (`type_system.dart:891-930`): an implicit
///   downcast rejects; the failure surfaces through the ordinary
///   assignability codes (`invalid_assignment`,
///   `argument_type_not_assignable`, `return_of_invalid_type`), exactly as
///   the oracle reports through `isAssignableTo`'s `strictCasts` arm — the
///   mode mints no code of its own.
/// * `inference` — `strict-inference`: an inference failure reports its
///   `inference_failure_on_*` row; the M1-decidable face is the
///   declaration-shape one (untyped top-level functions/getters).
/// * `raw_types` — `strict-raw-types`: a raw generic type annotation
///   reports `strict_raw_type`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StrictModes {
    /// `analyzer.language.strict-casts`.
    pub casts: bool,
    /// `analyzer.language.strict-inference`.
    pub inference: bool,
    /// `analyzer.language.strict-raw-types`.
    pub raw_types: bool,
}

/// Diagnoses one compilation unit against its library scope (#120, batch 1).
///
/// The unit must already be collected and indexed: `index` carries the
/// file's element model (its duplicate log folds into the front of the
/// report as `duplicate_definition`s) and its non-type-parameter
/// declarations are already declared into the library graph. The parse is a
/// second pass over the same token table `collect` used. The language
/// strictness modes are the official defaults (all off), and the
/// `class-modifiers` posture is the modern default (every library carries
/// the feature) — the caller-resolved faces ride [`diagnose_with`].
///
#[allow(clippy::too_many_arguments)] // the analysis context, one field each
pub fn diagnose(
    source: &str,
    tokens: &TokenTable,
    features: ParserFeatures,
    strings: &StringTable,
    file: FileId,
    lib: LibraryId,
    graph: &LibraryGraph,
    namespaces: &Namespaces,
    index: &ElementIndex,
) -> FileReport {
    let posture = LibraryClassModifiers::default();
    diagnose_with(
        source,
        tokens,
        features,
        strings,
        file,
        lib,
        graph,
        namespaces,
        index,
        StrictModes::default(),
        &posture,
        // The default posture is the modern grant (shipped 3.2); the
        // caller-resolved face rides [`diagnose_with`].
        true,
    )
}

/// [`diagnose`] with the caller-resolved language faces: the
/// language-strictness modes the engine maps from the library's analysis
/// options (`AnalysisOptions::{strict_casts, strict_inference,
/// strict_raw_types}` — `unwrap_or(false)`, the official off posture), and
/// the per-library `class-modifiers` posture — each analyzed library's
/// defining file's settled language version (the `@dart` override applied)
/// resolved through the feature registry (`class-modifiers`, `enabledIn
/// 3.0.0`), consumed by the outside-library modifier exemption gate
/// (`_mayIgnoreClassModifiers`). Libraries absent from the map answer the
/// modern posture.
///
#[allow(clippy::too_many_arguments)] // the analysis context, one field each
pub fn diagnose_with(
    source: &str,
    tokens: &TokenTable,
    features: ParserFeatures,
    strings: &StringTable,
    file: FileId,
    lib: LibraryId,
    graph: &LibraryGraph,
    namespaces: &Namespaces,
    index: &ElementIndex,
    strict: StrictModes,
    class_modifiers: &LibraryClassModifiers,
    field_promotion_enabled: bool,
) -> FileReport {
    diagnose_unit(
        source,
        tokens,
        features,
        strings,
        file,
        lib,
        graph,
        namespaces,
        index,
        strict,
        class_modifiers,
        field_promotion_enabled,
    )
    .0
}

/// [`diagnose_with`], plus the unit's use-face (the M3 U1 substrate,
/// [`crate::use_face::UnitUseFace`]): every observable use the pass recorded,
/// for the caller to fold into its per-library consumed set
/// ([`crate::use_face::LibraryUsages::close_library_usage`]). The report side is
/// byte-identical to [`diagnose_with`].
///
#[allow(clippy::too_many_arguments)] // the analysis context, one field each
pub fn diagnose_unit(
    source: &str,
    tokens: &TokenTable,
    features: ParserFeatures,
    strings: &StringTable,
    file: FileId,
    lib: LibraryId,
    graph: &LibraryGraph,
    namespaces: &Namespaces,
    index: &ElementIndex,
    strict: StrictModes,
    class_modifiers: &LibraryClassModifiers,
    field_promotion_enabled: bool,
) -> (FileReport, UnitUseFace) {
    diagnose_unit_with_ops(
        source,
        tokens,
        features,
        strings,
        file,
        lib,
        graph,
        namespaces,
        index,
        strict,
        class_modifiers,
        field_promotion_enabled,
        None,
    )
}

/// [`diagnose_unit`] with the resolved-type face mounted (#2204 phase A):
/// `ops`, when `Some`, is the library's [`crate::resolved_ops::ResolvedTypeOps`]
/// over a per-library `LibraryTypes` snapshot; `None` keeps the
/// name-level `NameTypeOps` default. The ops object's lifetime must cover
/// the fold — it is owned by reference only.
#[allow(clippy::too_many_arguments)] // the analysis context, one field each
pub fn diagnose_unit_with_ops<'t>(
    source: &'t str,
    tokens: &'t TokenTable,
    features: ParserFeatures,
    strings: &StringTable,
    file: FileId,
    lib: LibraryId,
    graph: &LibraryGraph,
    namespaces: &Namespaces,
    index: &ElementIndex,
    strict: StrictModes,
    class_modifiers: &LibraryClassModifiers,
    field_promotion_enabled: bool,
    ops: Option<&'t crate::resolved_ops::ResolvedTypeOps<'t>>,
) -> (FileReport, UnitUseFace) {
    let fold = fold_unit_with_ops(
        source,
        tokens,
        features,
        strings,
        file,
        lib,
        graph,
        namespaces,
        index,
        strict,
        class_modifiers,
        field_promotion_enabled,
        ops,
    );
    (fold.report, fold.face)
}

/// The full product of one unit's diagnosing fold: everything
/// [`diagnose_unit`] returns, plus the intermediates the query face
/// ([`crate::analysis::Analysis`]) keeps — the flow facts verbatim, the
/// published bare-reference resolutions, and the published type-reference
/// positions.
pub struct UnitFold {
    /// The diagnosis ([`diagnose_unit`]'s report side).
    pub report: FileReport,
    /// The per-unit observable-use record.
    pub face: UnitUseFace,
    /// Every flow fact of the unit, offset-sorted
    /// ([`dartq_flow::Flow::take_facts`]).
    pub facts: Vec<FlowFact>,
    /// The read-type rows joined into the semantic `Ty` space — one per
    /// reachable identifier read, offset-sorted (the query face's
    /// family-a table).
    pub read_types: Vec<(SourceRange, Ty)>,
    /// The published bare-reference resolutions, parse order.
    pub refs: Vec<UnitRef>,
    /// The published type-reference positions, parse order.
    pub type_uses: Vec<(SourceRange, StringId)>,
    /// The parse-recovery rows, in arrival order — the production
    /// pipeline's l1 facts carry the identical rows, so the fold's own
    /// report stays clean and the unit consumers that express the full
    /// oracle set merge them ahead of the semantic rows (parser rows
    /// first at a shared anchor: the parse precedes resolution).
    pub parse_diagnostics: Vec<Diagnostic>,
}

/// Builds the capture-time duplicate log's `duplicate_definition` rows —
/// the one construction both emission faces render (the fold's lead rows
/// and the engine's `l1::duplicate_diagnostics` bake): ERROR at the second
/// declaration's name token, setters named by their bare identifier, plus
/// the oracle's first-declaration context row when `first_decl` resolves
/// the kept declaration's name span (`diagnostic_factory.dart`
/// `duplicateDefinition` — "The first definition of this name." at the
/// original element's fragment). The model keeps no declaration spans, so
/// each face resolves from its own capture grade; an unresolved anchor
/// degrades to the plain row, never a dropped diagnostic.
pub fn duplicate_definition_diagnostics(
    file: FileId,
    elements: &FileElements,
    strings: &StringTable,
    first_decl: impl Fn(StringId, SourceRange) -> Option<SourceRange>,
) -> Vec<Diagnostic> {
    elements
        .duplicates()
        .iter()
        .map(|dup| {
            let text = strings.resolve(dup.name);
            let display = text.strip_suffix('=').unwrap_or(text);
            Diagnostic::new(
                Severity::Error,
                SemanticCode::DuplicateDefinition.code_info(),
                SemanticCode::DuplicateDefinition.message(display, ""),
                file,
                dup.range,
                None,
            )
            .with_context(first_decl(dup.name, dup.range).map_or_else(
                Vec::new,
                |range| {
                    vec![ContextMessage {
                        message: SharedStr::from("The first definition of this name."),
                        file,
                        range,
                    }]
                },
            ))
        })
        .collect()
}

/// The fold face's first-declaration lookup for the duplicate log: the
/// unique library-level site of `name` preceding the duplicate
/// ([`Engine::decl_sites`]). Type parameters park in the model under a
/// bare tag and named formals carry no sites, so those faces degrade;
/// several same-name sites mean the duplicate's scope is not derivable
/// here and degrade too — the context row attaches only when it cannot
/// anchor wrong.
pub(crate) fn first_decl_site(
    engine: &Engine<'_>,
    elements: &FileElements,
    name: StringId,
    duplicate: SourceRange,
) -> Option<SourceRange> {
    if matches!(
        elements.decl(name),
        Some(TopDecl {
            kind: MicroKind::TypeParam,
            ..
        })
    ) {
        return None;
    }
    // A named-formal duplicate's kept formal has no site: any preceding
    // same-name parameter local marks the face.
    if engine.locals.iter().any(|decl| {
        decl.name == name
            && matches!(decl.kind, LocalKind::Parameter { .. })
            && decl.range.offset < duplicate.offset
    }) {
        return None;
    }
    let mut sites = engine
        .decl_sites
        .iter()
        .filter(|site| site.name == name && site.range.offset < duplicate.offset);
    let first = sites.next()?;
    sites.next().is_none().then_some(first.range)
}

/// The `invalid_override` family's context row: the overridden member's
/// declaration anchor in its own file, cross-file capable (upstream Dart SDK
/// source, `pkg/analyzer/lib/src/diagnostic/diagnostic_factory.dart`
/// `invalidOverride` — "The member being overridden." / "The setter being
/// overridden." at the super member's fragment; deliberately code-gated, so
/// `invalid_implementation_override` carries none and this helper never
/// feeds it). The anchor rides the member entry itself
/// ([`MemberEntry::name_range`]), so the boundary projection carries it with
/// the deep-copied entry. A missing span (a payload restored before the face
/// shipped) degrades to no rows — the diagnostic never drops, the anchor
/// never guesses.
pub(crate) fn overridden_member_context(
    inherited: &MemberEntry,
    origin: FileId,
    setter: bool,
) -> Vec<ContextMessage> {
    if inherited.name_range.offset == u32::MAX {
        return Vec::new();
    }
    vec![ContextMessage {
        message: SharedStr::from(if setter {
            "The setter being overridden."
        } else {
            "The member being overridden."
        }),
        file: origin,
        range: inherited.name_range,
    }]
}

/// [`diagnose_unit`]'s whole pipeline, exposed for the query face: the
/// same fold, with the intermediates kept instead of dropped.
#[allow(clippy::too_many_arguments)] // the analysis context, one field each
pub fn fold_unit(
    source: &str,
    tokens: &TokenTable,
    features: ParserFeatures,
    strings: &StringTable,
    file: FileId,
    lib: LibraryId,
    graph: &LibraryGraph,
    namespaces: &Namespaces,
    index: &ElementIndex,
    strict: StrictModes,
    class_modifiers: &LibraryClassModifiers,
    field_promotion_enabled: bool,
) -> UnitFold {
    fold_unit_with_ops(
        source,
        tokens,
        features,
        strings,
        file,
        lib,
        graph,
        namespaces,
        index,
        strict,
        class_modifiers,
        field_promotion_enabled,
        None,
    )
}

/// [`fold_unit`] with the resolved-type face mounted: `ops`, when `Some`,
/// is the library's [`crate::resolved_ops::ResolvedTypeOps`] over a
/// per-library `LibraryTypes` snapshot — the flow replay asks it through
/// the [`dartq_flow::TypeOps`] face and the diagnosing pass's structural
/// subtype questions answer through it directly; `None` keeps the
/// name-level [`dartq_flow::ty::NameTypeOps`] default the flow layer
/// falls back to. The ops object's lifetime must cover the parse — the
/// fold owns it only by reference.
#[allow(clippy::too_many_arguments)] // the analysis context, one field each
#[allow(clippy::too_many_lines)] // one linear pipeline: parse, resolve, fold, merge
pub fn fold_unit_with_ops<'t>(
    source: &'t str,
    tokens: &'t TokenTable,
    features: ParserFeatures,
    strings: &StringTable,
    file: FileId,
    lib: LibraryId,
    graph: &LibraryGraph,
    namespaces: &Namespaces,
    index: &ElementIndex,
    strict: StrictModes,
    class_modifiers: &LibraryClassModifiers,
    field_promotion_enabled: bool,
    ops: Option<&'t crate::resolved_ops::ResolvedTypeOps<'t>>,
) -> UnitFold {
    let mut listener = DiagnoseListener {
        engine: Engine::new(
            graph,
            namespaces,
            index,
            strings,
            source,
            tokens,
            file,
            lib,
            features.primary_constructors_enabled,
            features.variance_enabled,
            field_promotion_enabled,
            features.patterns_enabled,
            strict,
            class_modifiers,
            ops,
        ),
        flow: {
            let mut flow = Flow::new(tokens, source);
            // The resolved-type face (#2204 phase A): when the library's
            // `LibraryTypes` snapshot mounted an adapter, every type
            // question the replay asks answers through it; the `None`
            // call sites keep the name-level default.
            if let Some(ops) = ops {
                flow.set_ops(ops as &dyn dartq_flow::TypeOps);
            }
            flow
        },
        pending_exit: None,
        file,
        parse_diagnostics: Vec::new(),
    };
    // The parse runs inside the argument-aware fold window (#2462): the
    // is-check folds over argument-carrying types re-ask the semantic
    // subtype face through the engine while it lives here. The guard
    // closes before the destructure below moves the engine out of the
    // listener — the registered address is valid for the window's whole
    // life.
    let parse_window = crate::switch_tracker::push_fold_engine(&listener.engine);
    // The parse borrows the listener; `drop(parser)` ends the borrow and
    // the destructure below takes the engine and flow recorder back.
    let mut parser = Parser::new(source, tokens, features, &mut listener);
    parser.parse_unit();
    drop(parser);
    drop(parse_window);
    // The pending exit is dropped unflushed — the old listener drop
    // did exactly the same (a latch with no following flow event never
    // records).
    let DiagnoseListener {
        mut engine,
        mut flow,
        pending_exit: _,
        file: _,
        parse_diagnostics,
    } = listener;
    // The flow layer's facts fold in exactly once, before the engine
    // unwraps (#477 S1): the promoted-read rows stage for the assignability
    // families' teardown re-decision, the dead-code rows collect for the
    // report tail's merge below. The invalid-type name set (#2193) is
    // snapshot-true already — the `TypeUse` verdicts latched at push
    // time — so the staged spellings intern against it below.
    let invalid_names = invalid_type_names(&engine.rows);
    let facts = flow.take_facts();
    let mut promotions = PromotionStaging::default();
    let mut unassigned = UnassignedStaging::default();
    // The promoted-property spans (#1409): the field-promotion rows the
    // teardown join re-decides the staged families against.
    let mut prop_promotions = PropPromotionStaging::default();
    // The query face's read-type rows (#1056): every read row joins into
    // the diagnosis-local pool (`staged_promotion`'s mapping — the flow
    // layer's interned names re-intern here), so the queries answer in
    // the semantic `Ty` space.
    let mut read_type_rows: Vec<(SourceRange, Ty)> = Vec::new();
    // The condition-operand and scrutinee flow types (#1349): the
    // diagnoser's condition/scrutinee faces answer by the read/clause
    // anchor offset — the promoted type when a promotion stands, the
    // declared type otherwise (the flow-aware static type the official
    // verdicts read).
    let mut condition_types: Vec<(SourceRange, Ty)> = Vec::new();
    let mut matched_types: Vec<(SourceRange, Ty)> = Vec::new();
    let mut flow_diagnostics = Vec::new();
    let mut body_rows = Vec::new();
    for fact in &facts {
        match *fact {
            FlowFact::DeadCode { span } => {
                // The `??` right operand's dedicated row
                // (`_checkForDeadNullCoalesce`'s
                // `dead_null_aware_expression`) coexists with the generic
                // dead window — the official emits both, so the row never
                // drops. What does need the staged row is the span end:
                // the flow fact rides the parser's expression-end token,
                // which sits one token past the operand's own end for a
                // bare-identifier right operand (`x ?? y)` ends after `)`).
                // The staged row's range IS the right operand's node
                // range, so the dead row's end clamps to it — the
                // official's node boundary. The gate is the anchor token:
                // only a `??`-anchored fact is that construct's arm row
                // (dead windows and `?:`/`&&`/`||` arms anchor elsewhere
                // and may legitimately cover a staged row whole). Nested
                // `??` arms stage several contained rows; the right
                // operand's own row is the one with the furthest end.
                let mut range = span.range;
                if tokens
                    .get(span.id as usize)
                    .is_some_and(|t| t.kind == dartq_scanner::TokenKind::QuestionQuestion)
                {
                    let dead_null_aware = SemanticCode::DeadNullAwareExpression.code_info().name;
                    if let Some(rhs_end) = engine
                        .report
                        .usage
                        .iter()
                        .filter(|d| {
                            d.code.name == dead_null_aware
                                && d.range.offset >= range.offset
                                && d.range.end() <= range.end()
                        })
                        .map(|d| d.range.end())
                        .max()
                        && rhs_end < range.end()
                        && rhs_end > range.offset
                    {
                        range.length = rhs_end - range.offset;
                    }
                }
                let code = SemanticCode::DeadCode;
                flow_diagnostics.push(Diagnostic::new(
                    code.severity(),
                    code.code_info(),
                    code.message("", ""),
                    file,
                    range,
                    code.correction_message(""),
                ));
            }
            FlowFact::PromotedRead { span, ty, args } => {
                // The staged type joins the diagnosing universe's pool
                // ([`Boundary::args`]) — the same universe
                // [`Engine::filter_staged`]'s re-decision reads. The staged
                // type folds through a type alias first (the flow type at a
                // span is the resolved interface type).
                let staged = staged_promotion_typed(
                    ty,
                    args,
                    &flow,
                    strings,
                    &invalid_names,
                    &mut engine.boundary.args.borrow_mut(),
                );
                if let Some(promoted) = staged.map(|row| engine.fold_alias_ty(row)) {
                    promotions.stage(span.range, promoted);
                }
            }
            // The definite-unassignment face (#746): the cast join reads it
            // after the fold, when the promotions are complete too.
            // A `late` local read while definitely unassigned reports
            // `definitely_unassigned_late_local_variable` at the read
            // (type_analyzer.dart `_checkReadOfNotAssignedLocalVariable`:
            // the late arm gates on `isDefinitelyUnassigned` alone).
            FlowFact::UnassignedRead {
                span,
                is_late,
                definitely_unassigned,
                ..
            } => {
                if is_late && definitely_unassigned {
                    let range = span.range;
                    let name = &source[range.offset as usize..range.end() as usize];
                    let code = SemanticCode::DefinitelyUnassignedLateLocalVariable;
                    flow_diagnostics.push(Diagnostic::new(
                        code.severity(),
                        code.code_info(),
                        code.message(name, ""),
                        file,
                        range,
                        code.correction_message(""),
                    ));
                }
                unassigned.stage(span.range, definitely_unassigned);
            }
            // The promoted-property spans (#1409): the offset-keyed rows
            // the staged families re-decide against — boolean rows, the
            // non-null substitution happens at the join where the
            // eligibility gate can consult the member resolution.
            FlowFact::PromotedPropertyRead { span } => {
                prop_promotions.stage(span.range);
            }
            // The body-completion fact joins the executable-completion
            // staging: the row carries the declared return type and the
            // declaring name span the official row reports at.
            FlowFact::BodyMayCompleteNormally { span } => {
                if let Some((code, ret, anchor)) = engine.body_may_complete_report(span.range) {
                    body_rows.push(Diagnostic::new(
                        code.severity(),
                        code.code_info(),
                        code.message(&engine.render_ty(ret), ""),
                        file,
                        anchor,
                        code.correction_message(""),
                    ));
                }
            }
            // The query-face rows (#1056): no diagnostic behavior — the
            // report side is unchanged. The read-type row joins into the
            // semantic `Ty` space beside its promotion sibling; a
            // void/never-typed read (no real read can carry one) drops. The
            // staged type folds through a type alias first (`type_of`
            // answers the resolved interface type).
            FlowFact::ReadType { span, ty, args } => {
                let staged = staged_promotion_typed(
                    ty,
                    args,
                    &flow,
                    strings,
                    &invalid_names,
                    &mut engine.boundary.args.borrow_mut(),
                );
                if let Some(row) = staged.map(|row| engine.fold_alias_ty(row)) {
                    read_type_rows.push((span.range, row));
                }
            }
            // The condition-operand rows (#1349): the flow-aware static
            // type at the operand read joins the nullable-condition face's
            // teardown join, keyed by the read's offset. The staged type
            // folds through a type alias first.
            FlowFact::ConditionType { span, ty, args } => {
                let staged = staged_promotion_typed(
                    ty,
                    args,
                    &flow,
                    strings,
                    &invalid_names,
                    &mut engine.boundary.args.borrow_mut(),
                );
                if let Some(row) = staged.map(|row| engine.fold_alias_ty(row)) {
                    condition_types.push((span.range, row));
                }
            }
            // The scrutinee rows (#1349): the promoted matched type at
            // each case clause joins the constant-pattern re-decision,
            // keyed by the clause anchor's offset. The staged type folds
            // through a type alias first.
            FlowFact::MatchedType { span, ty, args } => {
                let staged = staged_promotion_typed(
                    ty,
                    args,
                    &flow,
                    strings,
                    &invalid_names,
                    &mut engine.boundary.args.borrow_mut(),
                );
                if let Some(row) = staged.map(|row| engine.fold_alias_ty(row)) {
                    matched_types.push((span.range, row));
                }
            }
            // The async-gap rows (`use_build_context_synchronously`'s
            // core) publish verbatim to their downstream consumer — the
            // lints face's mounted-guard verdict — and carry no semantic
            // diagnostic of their own; write sites are likewise the
            // assignment face's payload only.
            FlowFact::WriteSite { .. } | FlowFact::AsyncContextUse { .. } => {}
            // The non-promotion substrate rows (the flow layer's W4, the
            // diagnoser's W4-attach #1877): each row stages into the
            // context face's table — the reasons resolve through the
            // element-layer verdict and the nullable-key filter — and the
            // parse-time emissions' deferred requests attach below.
            FlowFact::WhyNotPromoted { span, .. }
            | FlowFact::NonPromotedPropertyRead { span, .. }
            | FlowFact::ThisNotPromoted { span, .. } => {
                engine.stage_non_promotion(&flow, &fact, span);
            }
        }
    }
    // The #1877 deferred contexts: the fact rows are staged, so the
    // parse-time emissions take their why-not-promoted rows (before the
    // tail merges re-home the report — the recorded indices hold only in
    // parse order).
    engine.attach_non_promotion_contexts();
    // The #746 cast join: a candidate fires when the flow layer saw its
    // read definitely unassigned and no promotion stands at the span — the
    // official gate reads the flow-aware static type
    // (type_analyzer.dart:2118), and a promotion to `Null` is exactly the
    // face the staging-time declared type cannot see.
    let candidates = std::mem::take(&mut engine.cast_nullable);
    let mut cast_rows = Vec::with_capacity(candidates.len());
    for (range, name) in candidates {
        if !unassigned.staged(range) || promotions.promoted(range) == Some(Ty::Null) {
            continue;
        }
        let code = SemanticCode::CastFromNullableAlwaysFails;
        cast_rows.push(Diagnostic::new(
            code.severity(),
            code.code_info(),
            code.message(strings.resolve(name), ""),
            file,
            range,
            code.correction_message(""),
        ));
    }
    // The null-aware dead-chain face (`_verifyUnassignedVariable`,
    // dead_code_verifier.dart:460-505): a chain latched over a nullable
    // local's read reports when the flow facts confirm the read is
    // definitely unassigned and no promotion stands non-null at it (a
    // promoted receiver's static type is no longer `?`-suffixed).
    let chains = std::mem::take(&mut engine.dead_null_chains);
    let mut chain_rows = Vec::with_capacity(chains.len());
    for (op_offset, chain_end, recv) in chains {
        let promoted_non_null = promotions
            .promoted(recv)
            .is_some_and(Ty::is_strictly_non_nullable);
        if promoted_non_null || !unassigned.staged(recv) {
            continue;
        }
        let code = SemanticCode::DeadCode;
        chain_rows.push(Diagnostic::new(
            code.severity(),
            code.code_info(),
            code.message("", ""),
            file,
            SourceRange {
                offset: op_offset,
                length: chain_end.saturating_sub(op_offset),
            },
            code.correction_message(""),
        ));
    }
    // The staged null-aware faces (`_checkForUnnecessaryNullAware`), keyed
    // on the receiver reads the promotions are now complete for.
    engine.flush_null_aware_staged(&promotions);
    // The parser held the only other strong reference and is gone, so the
    // unwrap cannot fail.
    // The promoted-receiver re-decision (#1465) reads the joined read
    // types throughout pass 2; the rows re-home into the fold product
    // below (the sort here makes the join's offset walk exact).
    engine.read_types = std::mem::take(&mut read_type_rows);
    // The semantic-side query rows (#1869-2) join the flow rows before the
    // sort: the flow layer types neither the `this` keyword nor an
    // implicit-`this` bare member read, so the merge cannot collide at an
    // offset and the joint sort feeds both faces from one table.
    engine
        .read_types
        .extend(std::mem::take(&mut engine.query_type_rows));
    engine
        .read_types
        .sort_unstable_by_key(|(range, _)| range.offset);
    // The promoted-property spans ride into pass 2 beside the read types:
    // `member_defined`'s nullable-dereference verdict re-decides against
    // them through `promotable_field_substitution` (a promoted private
    // final field's receiver is non-null at the use). The local table
    // stays for `filter_staged`'s own join below.
    engine.prop_promotion_rows = PropPromotionStaging {
        rows: prop_promotions.rows.clone(),
    };
    engine.resolve_rows(&promotions);
    // The discarded-value folds (`checkConstructorInvocation`) and the
    // `unless` parameter co-row (`_findUndefinedUseResultParameter`) run
    // after the resolve arms: the folds read the final used set, the co-row
    // reads the settled formal tables.
    engine.finalize_unused_result();
    engine.finalize_undefined_use_result_params();
    // The #610 boundedness faces run after the parse — the snapshot must
    // see every in-file container — folding the deferred sites, the enum
    // values-field checks, and the `strict_raw_type` displays.
    engine.finalize_bounds();
    engine.finalize_ext_faces();
    engine.finalize_pattern_getters();
    engine.finalize_mixin_uses();
    // The F-C redirect cycle walk needs every constructor's initializer
    // edges (a later declaration can close a cycle an earlier one opened).
    engine.finalize_redirects();
    // The `const_with_non_const` verdicts deferred per expression, judged
    // against the resolved container's const-constructor bit — staged for
    // the source-ordered merge like the other deferred faces.
    for (name, kw) in std::mem::take(&mut engine.pending_const_non_const) {
        if !engine.const_ctor_target(name) {
            engine.stage_usage(SemanticCode::ConstWithNonConst, kw, "");
        }
    }
    // The const-declaration initializer creations (deferred for the
    // whole-file const-ctor set): each invalid entry's face keys on WHERE
    // its creation sits in the initializer's evaluation — the variable
    // face at a root creation or the root literal's first failing element,
    // `invalid_constant` inside a resolved creation's arguments, silence
    // past a wholesale rejection — and every invalid creation whose
    // constructor exists but is non-const adds the `const_with_non_const`
    // twin at the creation.
    {
        // Closes push innermost-first (`Missing(Missing2())` stages the
        // argument's entry before the call's own), so the per-initializer
        // anchoring sorts the entries into source order — the oracle's
        // evaluation walks outward and the first failure anchors.
        let mut entries = std::mem::take(&mut engine.pending_const_creations);
        entries.sort_by_key(|(_, span, _)| span.offset);
        let mut reported: FxHashSet<StringId> = FxHashSet::default();
        // The collections whose variable-face twin already staged: a
        // literal's evaluation aborts at its first failing element, so a
        // later element's own error never surfaces (`[Missing(),
        // Missing2()]` pairs the face with the first only).
        let mut anchored: FxHashSet<u32> = FxHashSet::default();
        for (init, span, kind) in entries {
            match kind {
                ConstCreationEntry::ArgFailure => {
                    // A known non-constant LEAF argument (the failing span
                    // is not call-shaped: `C(v)`, the `v` head of
                    // `C(v + 1)`) pairs its already-emitted argument face
                    // with the variable face at the same span — the
                    // oracle's `_validateConstantArguments` twin. The
                    // call-shaped entries are the unresolved creations,
                    // keyed on the evaluation site below.
                    if !engine.creation_span_is_call(span) {
                        if reported.insert(init) {
                            engine.stage_usage(
                                SemanticCode::ConstInitializedWithNonConstantValue,
                                span,
                                "",
                            );
                        }
                        continue;
                    }
                    match engine.creation_failure_site(span) {
                        CreationFailureSite::Root => {
                            // The root unresolved creation's variable face
                            // rides the callee's own resolution row — a
                            // resolved-but-unindexed container (an SDK
                            // class without a const constructor) carries
                            // none and stays silent. The row sits inside
                            // the creation span, not always at its head:
                            // `Gone(`'s `undefined_function` anchors at the
                            // bare callee (the span's start), `C.named(`'s
                            // `undefined_method` anchors at the selector
                            // (the oracle's
                            // `const_with_undefined_constructor` world).
                            if engine.const_failure_row_within(span) && reported.insert(init) {
                                engine.stage_usage(
                                    SemanticCode::ConstInitializedWithNonConstantValue,
                                    span,
                                    "",
                                );
                            }
                        }
                        CreationFailureSite::Collection { open } => {
                            if engine.const_failure_row_between(open, span.offset)
                                || !anchored.insert(open)
                            {
                                continue;
                            }
                            if reported.insert(init) {
                                engine.stage_usage(
                                    SemanticCode::ConstInitializedWithNonConstantValue,
                                    span,
                                    "",
                                );
                            }
                        }
                        CreationFailureSite::ConstCreation { open } => {
                            // Each unresolved argument evaluates on its own
                            // placeholder (`C(Missing(), Missing2())`
                            // reports both) — unless an earlier argument's
                            // known non-constant failure aborted the
                            // evaluation first (`C(v, Missing())` stays
                            // silent), or the containing argument already
                            // anchored its own single error deeper-left
                            // (`C(missing + Missing())` reports once, at
                            // `missing`).
                            if !engine.const_failure_row_between(open, span.offset)
                                && !engine.const_arg_failure_anchored(span)
                            {
                                engine.stage_usage(SemanticCode::InvalidConstant, span, "");
                            }
                        }
                        // A wholesale rejection above (`f(Missing())`'s
                        // method-invocation fault) carries the subtree.
                        CreationFailureSite::Other => {}
                    }
                }
                ConstCreationEntry::Creation { container } => {
                    // A staged creation is the deferred candidate the
                    // parse-time verdict could not settle: invalid once the
                    // whole-file const-ctor set is known, it fails the
                    // initializer (the oracle's creation-level row); a
                    // valid creation reports nothing.
                    if engine.const_creation_valid(container, span) {
                        continue;
                    }
                    if reported.insert(init) {
                        engine.stage_usage(
                            SemanticCode::ConstInitializedWithNonConstantValue,
                            span,
                            "",
                        );
                    }
                    if engine.const_non_const_ctor_answered(container, span) {
                        engine.stage_usage(SemanticCode::ConstWithNonConst, span, "");
                    }
                }
            }
        }
    }
    // The #612 variance family runs after the parse — the capture must see
    // every container header — staged into the usage rows for the
    // source-ordered merge. One branch without the experiment.
    engine.finalize_variance();
    // The usage cluster's library-level pass runs after every use indexed:
    // declaration sites and import directives walk the index, staging into
    // the report; the staged emissions merge source-ordered into the tail.
    engine.finalize_usage();
    // The use-face extraction (#767): the per-file consumers have read
    // their sets; the sets re-home into the face for the caller's
    // per-library close.
    let face = engine.take_use_face();
    let mut report = std::mem::take(&mut engine.report);
    // The staged assignability verdicts re-decide against the promotions
    // and merge ahead of the flow rows: the assignability families are
    // resolution-phase diagnostics while the dead-code rows and the usage
    // cluster are the verifier passes, so at a shared anchor the official
    // collection order is assignability before dead_code before
    // unused_local_variable — each merge's `<=` tie-break keeps that order
    // into the final list.
    // The matched-type re-decision (#1351): the passing parse-time
    // constant-pattern candidates re-check against the flow's promoted
    // matched type (`getMatchedValueType`'s head) — the narrowed type can
    // never equal the constant where the declared type could. The rows
    // stage into the usage tail like their parse-time siblings.
    let pending_matched = std::mem::take(&mut engine.pending_matched);
    for cand in pending_matched {
        let matched = matched_types
            .iter()
            .rev()
            .find(|(range, _)| range.offset == cand.clause.offset)
            .map(|(_, ty)| *ty);
        let Some(matched) = matched else {
            continue;
        };
        if engine.constant_never_matches(cand.constant, matched) {
            let a0 = engine.ty_display(matched);
            let a1 = engine.ty_display(cand.constant);
            // The row stages into the TAKEN report's usage tail: the
            // engine's own report left with `let mut report` above, so an
            // `engine.stage_typed` push here would land in the fresh
            // engine-side usage and never merge (#1351 teardown drop).
            let code = SemanticCode::ConstantPatternNeverMatchesValueType;
            report.usage.push(Diagnostic::new(
                code.severity(),
                code.code_info(),
                code.message(&a0, &a1),
                file,
                cand.at,
                code.correction_message(""),
            ));
        }
    }
    // The deferred switch-covering verdicts (#2538): the nullable
    // scrutinee's parse-time row re-decides against the flow's matched
    // type — a promotion that strips the null witness drops the row, a
    // still-uncovered refined value re-renders it.
    let pending_coverings = std::mem::take(&mut engine.pending_switch_coverings);
    for pending in &pending_coverings {
        let matched = matched_types
            .iter()
            .rev()
            .find(|(range, _)| range.offset == pending.first_anchor.offset)
            .map(|(_, ty)| *ty);
        if let Some(row) = engine.teardown_covering(pending, matched) {
            report.usage.push(row);
        }
    }
    let survivors = engine.filter_staged(&promotions, &condition_types, &prop_promotions);
    report.diagnostics = merge_source_ordered(report.diagnostics, survivors);
    // The flow layer's dead-code facts join the source-ordered tail of the
    // report (both sides are offset-sorted; the merge keeps the invariant).
    report.diagnostics = merge_source_ordered(report.diagnostics, flow_diagnostics);
    // The #746 flow-fact consumers join the same tail: the cast join's rows
    // and the body-completion rows are each offset-sorted, and the merges
    // keep the invariant.
    report.diagnostics = merge_source_ordered(report.diagnostics, cast_rows);
    report.diagnostics = merge_source_ordered(report.diagnostics, chain_rows);
    report.diagnostics = merge_source_ordered(report.diagnostics, body_rows);
    let mut staged = std::mem::take(&mut report.usage);
    staged.sort_by_key(|d| d.range.offset);
    report.diagnostics = merge_source_ordered(report.diagnostics, staged);
    // The #2336 InvalidType-scrutinee gate runs once the parse-time
    // emissions (the covering rows in `diagnostics`, the per-case rows
    // staged into `usage`) share one list: a switch whose scrutinee type
    // never resolved is the oracle's `InvalidType` — its whole
    // exhaustiveness pass (reachability and covering alike) gates off
    // behind `hasInvalidType` (constant_verifier.dart:1035/:1095).
    gate_invalid_scrutinee_switches(
        &mut report.diagnostics,
        &engine.read_types,
        tokens,
        graph,
        namespaces,
        index,
        file,
        lib,
    );
    // The #2340 InvalidType-scrutinee coverage face runs on the same joined
    // list: an invalid-posture scrutinee makes the oracle's
    // `promoteForPattern` absorb the match at the first guard-free
    // resolvable-typed case (a vacuous subtype, subtype.dart:274-276), so
    // every following clause keyword and its first body statement reports
    // `dead_code` — rows dartq's unknown-face flow never produced.
    let dead_cases = synthesize_invalid_scrutinee_dead_cases(
        &report.diagnostics,
        &engine.read_types,
        tokens,
        source,
        strings,
        graph,
        namespaces,
        index,
        file,
        lib,
    );
    report.diagnostics = merge_source_ordered(report.diagnostics, dead_cases);
    // The #2450 unknown-scrutinee face runs on the same joined list: a
    // scrutinee expression whose latched type is the unknown face (the
    // oracle's `InvalidType`, chained through an unresolved initializer or
    // receiver) absorbs the match at the first guard-free resolvable-typed
    // clause exactly like the written-annotation face above, and switch
    // EXPRESSIONS join the walk (their arms the statement-only scan never
    // produced).
    let dead_cases = synthesize_unknown_scrutinee_dead_cases(
        &report.diagnostics,
        &engine.read_types,
        tokens,
        source,
        strings,
        graph,
        namespaces,
        index,
        file,
        lib,
    );
    report.diagnostics = merge_source_ordered(report.diagnostics, dead_cases);
    // `duplicate_definition`s lead the report, each pointing at the SECOND
    // declaration's name token (the analyzer's diagnose-at-second rule).
    // Setters live under `name=` in the model; the oracle's message names
    // the bare identifier. The first-declaration context row resolves from
    // the fold's own capture grade ([`first_decl_site`]).
    let mut diagnostics = Vec::with_capacity(
        index
            .get(file)
            .map_or(0, |f| f.duplicates().len() + f.arity_violations().len()),
    );
    if let Some(elements) = index.get(file) {
        diagnostics.extend(duplicate_definition_diagnostics(
            file,
            elements,
            strings,
            |name, duplicate| first_decl_site(&engine, elements, name, duplicate),
        ));
        // The declaration-arity log folds next (capture-time order =
        // source order): operator parameter counts and the setter shape
        // rule.
        for arity in elements.arity_violations() {
            let (code, message) = match arity.violation {
                ArityViolation::Operator {
                    name,
                    expected,
                    found,
                } => {
                    let code = SemanticCode::WrongNumberOfParametersForOperator;
                    (
                        code,
                        code.message3(
                            strings.resolve(name),
                            &expected.to_string(),
                            &found.to_string(),
                        ),
                    )
                }
                ArityViolation::OperatorMinus { found } => {
                    let code = SemanticCode::WrongNumberOfParametersForOperatorMinus;
                    (code, code.message3(&found.to_string(), "", ""))
                }
                ArityViolation::Setter => (
                    SemanticCode::WrongNumberOfParametersForSetter,
                    SemanticCode::WrongNumberOfParametersForSetter.message("", ""),
                ),
            };
            diagnostics.push(Diagnostic::new(
                code.severity(),
                code.code_info(),
                message,
                file,
                arity.range,
                None,
            ));
        }
    }
    diagnostics.append(&mut report.diagnostics);
    apply_ignore_suppression(&mut diagnostics, source, tokens);
    cap_diagnostics(&mut diagnostics, file);
    report.diagnostics = diagnostics;
    UnitFold {
        report,
        face,
        facts,
        read_types: std::mem::take(&mut engine.read_types),
        refs: std::mem::take(&mut engine.refs),
        type_uses: std::mem::take(&mut engine.type_uses),
        parse_diagnostics,
    }
}

/// Per-file emission cap (the parser's and the lints engine's 10k fuses,
/// #1879's D3, applied to the semantic fold): past this many rows the face's
/// output truncates and one terminal marker rides the tail. Every emission
/// path in this module converges on the fold's single assembly point — the
/// parse-time emit helpers, the staged re-decision survivors, the flow-fact
/// rows, the usage cluster, and the declaration-surface rows — so this one
/// gate covers them all; context rows ride their diagnostic and never count
/// separately. The cut is a `truncate` at one call site (the parser fuse's
/// idempotent-terminal shape): structural termination, never a loop, and an
/// under-cap file pays one comparison.
pub(crate) const MAX_DIAGNOSTICS_PER_FILE: usize = 10_000;

/// The `// ignore:` / `// ignore_for_file:` suppression face over the
/// assembled semantic run (`ignore_info.dart`'s line attribution, mirrored
/// in dartq-lints for the lint rows): a standalone comment line suppresses
/// the NEXT line, `ignore_for_file:` the whole file; names match the
/// diagnostic code's `name` OR `uniqueName` case-insensitively
/// (`IgnoredDiagnosticName._matches`, `ignore_info.dart`:39-52 — the
/// uniqueName leg resolves through [`canonical_diagnostic_name`]); ERROR
/// rows never suppress (the official severity gate — only
/// warnings/hints/lints are ignorable). Comments are extracted from the
/// inter-token gaps (string-safe — a `//` inside a literal lies inside a
/// token's byte range), and a comment alone on its line suppresses the
/// NEXT line while a trailing comment suppresses its own
/// (`IgnoreInfo.forDart`'s line attribution, ignore_info.dart:118-141).
pub(crate) struct IgnoreSuppression {
    /// 1-based line -> entries (`ignore:` citations).
    pub(crate) on_line: FxHashMap<u32, Box<[Entry]>>,
    /// `ignore_for_file:` entries.
    pub(crate) for_file: Box<[Entry]>,
}

impl IgnoreSuppression {
    /// Parses the suppression table from `source`'s `//`-line comments,
    /// extracted string-safely from the inter-token gaps of `tokens`. The
    /// comment head follows `isIgnoreComment` / `isIgnoreForFileComment`
    /// (ignore_info.dart:217-289): after `//`, the line form tolerates
    /// extra slashes (doc comments) while the for-file form requires
    /// exactly two, and both skip ASCII spaces only. A comment alone on
    /// its line suppresses the next line; a trailing comment suppresses
    /// its own (`IgnoreInfo.forDart`'s beforeMatch gate,
    /// ignore_info.dart:118-141), so `return x; // ignore: dead_code`
    /// carries its own suppression.
    pub(crate) fn parse(source: &str, tokens: &TokenTable) -> Self {
        let mut on_line: FxHashMap<u32, Vec<Entry>> = FxHashMap::default();
        let mut for_file: Vec<Entry> = Vec::new();
        // Comments come out in source order, so one amortized pass tracks
        // the line cursor and whether the comment stands alone on its
        // line (only whitespace between the line start and the `//`).
        let mut cursor = 0usize;
        let mut line = 1u32;
        for (start, end) in line_comments_of(source, tokens) {
            let gap = &source[cursor..start as usize];
            let mut line_head = 0usize;
            for (i, b) in gap.bytes().enumerate() {
                if b == b'\n' {
                    line += 1;
                    line_head = i + 1;
                }
            }
            let alone = gap[line_head..].trim().is_empty();
            cursor = start as usize;
            let lexeme = &source[start as usize..end as usize];
            // The two heads (ignore_info.dart:217-289): the for-file form
            // pins exactly two slashes and skips ASCII spaces only; the
            // line form tolerates one extra-slash run first (`/// ignore:`).
            let body = &lexeme[2..];
            if let Some(rest) = body
                .trim_start_matches(' ')
                .strip_prefix("ignore_for_file:")
            {
                for_file.extend(ignored_elements(rest));
                continue;
            }
            let head = body.trim_start_matches('/').trim_start_matches(' ');
            if let Some(rest) = head.strip_prefix("ignore:") {
                let target = if alone { line + 1 } else { line };
                on_line
                    .entry(target)
                    .or_default()
                    .extend(ignored_elements(rest));
            }
        }
        Self {
            on_line: on_line
                .into_iter()
                .map(|(line, names)| (line, names.into_boxed_slice()))
                .collect(),
            for_file: for_file.into_boxed_slice(),
        }
    }

    /// Whether `code` (the diagnostic's lowercased code name) or its
    /// `dtype` family is suppressed at `offset` (UTF-16). A name citation
    /// matches on the diagnostic's `name` OR on its `uniqueName`
    /// (`IgnoredDiagnosticName._matches`, `ignore_info.dart`:39-52): the
    /// entries are the comment's lowercased spellings, so the second leg
    /// canonicalizes each spelling through [`canonical_diagnostic_name`]
    /// (a uniqueName citation suppresses every diagnostic reported under
    /// the shared name; the name-level join is the fold's resolution, the
    /// variant split lives in the upstream constant, not in our
    /// `CodeInfo`). A family citation matches only its own type
    /// (`IgnoredDiagnosticType._matches`, ignore_info.dart:67-76).
    pub(crate) fn suppresses(
        &self,
        offset: u32,
        line_of: impl Fn(u32) -> u32,
        code: &str,
        dtype: DiagnosticType,
    ) -> bool {
        let hit = |entry: &Entry| entry_matches(entry, code, dtype);
        if self.for_file.iter().any(hit) {
            return true;
        }
        self.on_line
            .get(&line_of(offset))
            .is_some_and(|entries| entries.iter().any(hit))
    }
}

/// One ignore-comment entry: a diagnostic-name citation or a diagnostic-
/// family citation — the upstream `IgnoredDiagnosticName` /
/// `IgnoredDiagnosticType` pair (ignore_info.dart:15-82).
pub(crate) enum Entry {
    /// The lowercased citation spelling.
    Name(String),
    /// The cited family; only the ignorable types parse into entries
    /// (`IgnoredDiagnosticType._matches`'s switch — other families never
    /// match, so their citations register nothing).
    Type(DiagnosticType),
}

/// One entry vs a diagnostic: the `_matches` verdict of the upstream
/// element it came from.
fn entry_matches(entry: &Entry, code: &str, dtype: DiagnosticType) -> bool {
    match entry {
        Entry::Name(name) => citation_matches(name, code),
        Entry::Type(family) => *family == dtype,
    }
}

/// One ignore-comment citation spelling vs a diagnostic's registry `name`:
/// the upstream `IgnoredDiagnosticName._matches` verdict
/// (`ignore_info.dart`:39-52) — equal to the `name`, or a `uniqueName`
/// spelling whose canonical name it is. The name-level join is the fold's
/// resolution: the variant split lives in the upstream constant, not in
/// our `CodeInfo` — a `uniqueName` citation suppresses every diagnostic
/// reported under the shared `name` on its scope.
fn citation_matches(entry: &str, code: &str) -> bool {
    entry.eq_ignore_ascii_case(code)
        || canonical_diagnostic_name(entry).is_some_and(|c| c.eq_ignore_ascii_case(code))
}

/// The `//`-line comments of one unit, in source order — the inter-token
/// gaps are trivia, so a `//` found there is exactly a comment (the
/// scanner keeps comments out of the token table; a `//` inside a string
/// literal lies inside a token's byte range and is never scanned). Block
/// comments skip depth-counted, so a `//` inside `/* … */` never
/// qualifies. Spans are byte ranges `[start, end)`, line comments ending
/// before the line terminator (`\r` included — CRLF bodies keep the
/// carriage return out of the citation walk).
/// Collects `//`-comment spans in `[start, end)` of `bytes`, skipping the
/// bodies of `/* */` block comments (nested). The `//` head may sit
/// anywhere — a leading `//` inside a block comment never counts because
/// the block arm consumes its whole span.
fn scan_line_comment_gaps(
    bytes: &[u8],
    mut pos: usize,
    end: usize,
    spans: &mut Vec<(u32, u32)>,
) -> usize {
    while pos < end {
        match bytes[pos] {
            b'/' if bytes.get(pos + 1) == Some(&b'/') => {
                let body = &bytes[pos + 2..end];
                let comment_end = pos
                    + 2
                    + body
                        .iter()
                        .position(|&b| b == b'\n' || b == b'\r')
                        .unwrap_or(body.len());
                spans.push((pos as u32, comment_end as u32));
                pos = comment_end;
            }
            b'/' if bytes.get(pos + 1) == Some(&b'*') => {
                let mut depth = 1usize;
                pos += 2;
                while pos < end && depth > 0 {
                    if bytes[pos] == b'/' && bytes.get(pos + 1) == Some(&b'*') {
                        depth += 1;
                        pos += 2;
                    } else if bytes[pos] == b'*' && bytes.get(pos + 1) == Some(&b'/') {
                        depth -= 1;
                        pos += 2;
                    } else {
                        pos += 1;
                    }
                }
            }
            _ => pos += 1,
        }
    }
    pos
}

fn line_comments_of(source: &str, tokens: &TokenTable) -> Vec<(u32, u32)> {
    let bytes = source.as_bytes();
    let mut spans = Vec::new();
    let mut cursor = 0usize;
    for token in tokens.as_slice() {
        let gap_end = (token.byte_offset as usize).min(bytes.len());
        scan_line_comment_gaps(bytes, cursor.min(gap_end), gap_end, &mut spans);
        cursor = gap_end + token.byte_len as usize;
    }
    // The tail gap after the last token: a trailing `// ignore:` /
    // `ignore_for_file:` comment at end-of-file has no following token, so
    // without this flush it was silently dropped from the table.
    scan_line_comment_gaps(bytes, cursor.min(bytes.len()), bytes.len(), &mut spans);
    spans
}

/// The entries after an `ignore:`/`ignore_for_file:` head — the upstream
/// `ignoredElements` walk (ignore_info.dart:292-472): comma-separated
/// words of letters/digits/underscores, each word complete only when the
/// list continues correctly (end, space/tab, comma) — trailing prose
/// (`http://x`, free text) ends the list. `type=hint|lint|warning` cites
/// a family; a `plugin/name` word cites another plugin's diagnostics,
/// which never match dartq's rows (the pluginName gate,
/// ignore_info.dart:40-42), so the walk consumes it without registering.
fn ignored_elements(rest: &str) -> Vec<Entry> {
    // Upstream `isSpace` (string.dart:33) — a comment lexeme carries no
    // line terminators, so the EOL faces never appear here.
    const fn is_space(b: u8) -> bool {
        b == b' ' || b == b'\t'
    }
    let bytes = rest.as_bytes();
    let n = bytes.len();
    let mut entries = Vec::new();
    let mut o = 0usize;
    loop {
        while o < n && is_space(bytes[o]) {
            o += 1;
        }
        if o == n {
            return entries;
        }
        let word_start = o;
        // A word starts with a letter or underscore and continues with
        // letters, digits, underscores (upstream `readWord`).
        if !(bytes[o].is_ascii_alphabetic() || bytes[o] == b'_') {
            return entries;
        }
        o += 1;
        while o < n && (bytes[o].is_ascii_alphanumeric() || bytes[o] == b'_') {
            o += 1;
        }
        let word = &rest[word_start..o];
        if word.eq_ignore_ascii_case("type") {
            // `type=<family>` (ignore_info.dart:349-397): the `=` is
            // mandatory, the family word ends like a name.
            while o < n && is_space(bytes[o]) {
                o += 1;
            }
            if o == n || bytes[o] != b'=' {
                return entries;
            }
            o += 1;
            while o < n && is_space(bytes[o]) {
                o += 1;
            }
            if o == n || !(bytes[o].is_ascii_alphabetic() || bytes[o] == b'_') {
                return entries;
            }
            let family_start = o;
            o += 1;
            while o < n && (bytes[o].is_ascii_alphanumeric() || bytes[o] == b'_') {
                o += 1;
            }
            if o < n && !is_space(bytes[o]) && bytes[o] != b',' {
                return entries;
            }
            match &rest[family_start..o].to_ascii_lowercase()[..] {
                "hint" => entries.push(Entry::Type(DiagnosticType::Hint)),
                "lint" => entries.push(Entry::Type(DiagnosticType::Lint)),
                "warning" => entries.push(Entry::Type(DiagnosticType::StaticWarning)),
                _ => {}
            }
        } else {
            if o < n && bytes[o] == b'/' {
                // `plugin/name` (ignore_info.dart:398-423): the citation
                // binds to the named plugin's diagnostics only.
                o += 1;
                if o == n || !(bytes[o].is_ascii_alphabetic() || bytes[o] == b'_') {
                    return entries;
                }
                o += 1;
                while o < n && (bytes[o].is_ascii_alphanumeric() || bytes[o] == b'_') {
                    o += 1;
                }
            }
            if o < n && !is_space(bytes[o]) && bytes[o] != b',' {
                return entries;
            }
            entries.push(Entry::Name(word.to_ascii_lowercase()));
        }
        // The list continues only through a comma (ignore_info.dart
        // :450-469): whitespace followed by anything else is unstructured
        // comment text.
        if o == n {
            return entries;
        }
        while o < n && is_space(bytes[o]) {
            o += 1;
        }
        if o == n || bytes[o] != b',' {
            return entries;
        }
        o += 1;
        if o == n {
            return entries;
        }
    }
}

/// The registry `name` a lowercased `uniqueName` citation spelling resolves
/// to — `None` when the spelling is not a registry `uniqueName`. The
/// suppression join's second leg (`ignore_info.dart`:39-52): a citation
/// names either spelling of the diagnostic it suppresses.
///
/// The table is the pinned registry's 225 `(uniqueName, name)` divergent
/// constants — the same upstream derivation `dartq-engine`'s
/// `known_error_codes::UNIQUE_NAME_ALIASES` captures for the
/// `analyzer.errors`/`unnecessary_ignore` faces. The crate boundary keeps
/// the shared source out of reach (semantic cannot depend on the engine),
/// so the fold carries its own copy: version-pinned oracle data, sorted by
/// the `uniqueName` column for binary search. Keep both copies in lockstep
/// with the registry pin — a hoisted shared home is the dartq-diagnostics
/// code registry's to give.
fn canonical_diagnostic_name(unique: &str) -> Option<&'static str> {
    UNIQUE_NAME_ALIASES
        .binary_search_by(|&(spelling, _)| spelling.cmp(unique))
        .ok()
        .map(|index| UNIQUE_NAME_ALIASES[index].1)
}

/// `(uniqueName, name)` pairs of the pinned diagnostic registry whose two
/// spellings diverge (225 rows, sorted by `uniqueName`; the other 935
/// constants spell both alike and never reach this lookup — the first leg
/// covers them). Upstream derivation: the registry's `lowerCaseUniqueName`
/// minus its class prefix vs `lowerCaseName`
/// (`IgnoredDiagnosticName._matches`, `ignore_info.dart`:44-51).
const UNIQUE_NAME_ALIASES: &[(&str, &str)] = &[
    (
        "abstract_field_constructor_initializer",
        "abstract_field_initializer",
    ),
    (
        "ambiguous_comment_reference_name",
        "ambiguous_comment_reference",
    ),
    (
        "ambiguous_comment_reference_prefix",
        "ambiguous_comment_reference",
    ),
    (
        "ambiguous_extension_member_access_three_or_more",
        "ambiguous_extension_member_access",
    ),
    (
        "ambiguous_extension_member_access_two",
        "ambiguous_extension_member_access",
    ),
    (
        "analysis_option_deprecated_with_replacement",
        "analysis_option_deprecated",
    ),
    (
        "augmentation_induced_getter_already_complete",
        "declaration_already_complete",
    ),
    (
        "augmentation_induced_getter_return_type_mismatch",
        "augmentation_return_type_mismatch",
    ),
    (
        "augmentation_induced_setter_already_complete",
        "declaration_already_complete",
    ),
    (
        "augmentation_without_getter_declaration",
        "augmentation_without_declaration",
    ),
    (
        "augmentation_without_setter_declaration",
        "augmentation_without_declaration",
    ),
    (
        "base_class_implemented_outside_of_library",
        "invalid_use_of_type_outside_library",
    ),
    (
        "base_mixin_implemented_outside_of_library",
        "invalid_use_of_type_outside_library",
    ),
    (
        "built_in_identifier_as_extension_name",
        "built_in_identifier_in_declaration",
    ),
    (
        "built_in_identifier_as_extension_type_name",
        "built_in_identifier_in_declaration",
    ),
    (
        "built_in_identifier_as_prefix_name",
        "built_in_identifier_in_declaration",
    ),
    (
        "built_in_identifier_as_type_name",
        "built_in_identifier_in_declaration",
    ),
    (
        "built_in_identifier_as_type_parameter_name",
        "built_in_identifier_in_declaration",
    ),
    (
        "built_in_identifier_as_typedef_name",
        "built_in_identifier_in_declaration",
    ),
    (
        "class_instantiation_access_to_instance_member",
        "class_instantiation_access_to_member",
    ),
    (
        "class_instantiation_access_to_static_member",
        "class_instantiation_access_to_member",
    ),
    (
        "class_instantiation_access_to_unknown_member",
        "class_instantiation_access_to_member",
    ),
    (
        "conflicting_constructor_and_static_field",
        "conflicting_constructor_and_static_member",
    ),
    (
        "conflicting_constructor_and_static_getter",
        "conflicting_constructor_and_static_member",
    ),
    (
        "conflicting_constructor_and_static_method",
        "conflicting_constructor_and_static_member",
    ),
    (
        "conflicting_constructor_and_static_setter",
        "conflicting_constructor_and_static_member",
    ),
    (
        "conflicting_type_variable_and_class",
        "conflicting_type_variable_and_container",
    ),
    (
        "conflicting_type_variable_and_enum",
        "conflicting_type_variable_and_container",
    ),
    (
        "conflicting_type_variable_and_extension",
        "conflicting_type_variable_and_container",
    ),
    (
        "conflicting_type_variable_and_extension_type",
        "conflicting_type_variable_and_container",
    ),
    (
        "conflicting_type_variable_and_member_class",
        "conflicting_type_variable_and_member",
    ),
    (
        "conflicting_type_variable_and_member_enum",
        "conflicting_type_variable_and_member",
    ),
    (
        "conflicting_type_variable_and_member_extension",
        "conflicting_type_variable_and_member",
    ),
    (
        "conflicting_type_variable_and_member_extension_type",
        "conflicting_type_variable_and_member",
    ),
    (
        "conflicting_type_variable_and_member_mixin",
        "conflicting_type_variable_and_member",
    ),
    (
        "conflicting_type_variable_and_mixin",
        "conflicting_type_variable_and_container",
    ),
    (
        "const_constructor_constant_from_deferred_library",
        "collection_element_from_deferred_library",
    ),
    (
        "const_constructor_with_mixin_with_fields",
        "const_constructor_with_mixin_with_field",
    ),
    (
        "const_field_initializer_not_assignable",
        "field_initializer_not_assignable",
    ),
    (
        "const_primary_constructor_with_block_body",
        "const_primary_constructor_with_body",
    ),
    (
        "const_primary_constructor_with_expression_body",
        "const_primary_constructor_with_body",
    ),
    ("const_with_non_type", "creation_with_non_type"),
    (
        "const_with_type_parameters_constructor_tearoff",
        "const_with_type_parameters",
    ),
    (
        "const_with_type_parameters_function_tearoff",
        "const_with_type_parameters",
    ),
    (
        "constructor_already_complete",
        "declaration_already_complete",
    ),
    ("dead_code_late_wildcard_variable_initializer", "dead_code"),
    (
        "deprecated_extends_function",
        "deprecated_subtype_of_function",
    ),
    (
        "deprecated_implements_function",
        "deprecated_subtype_of_function",
    ),
    (
        "deprecated_member_use_implicit_super_constructor_invocation",
        "deprecated_member_use",
    ),
    (
        "deprecated_member_use_implicit_super_constructor_invocation_with_message",
        "deprecated_member_use",
    ),
    (
        "deprecated_member_use_implicit_type",
        "deprecated_member_use",
    ),
    (
        "deprecated_member_use_implicit_type_with_message",
        "deprecated_member_use",
    ),
    (
        "deprecated_member_use_with_message",
        "deprecated_member_use",
    ),
    (
        "deprecated_mixin_function",
        "deprecated_subtype_of_function",
    ),
    (
        "doc_directive_missing_one_argument",
        "doc_directive_missing_argument",
    ),
    (
        "doc_directive_missing_three_arguments",
        "doc_directive_missing_argument",
    ),
    (
        "doc_directive_missing_two_arguments",
        "doc_directive_missing_argument",
    ),
    (
        "dot_shorthand_undefined_getter",
        "dot_shorthand_undefined_member",
    ),
    (
        "dot_shorthand_undefined_invocation",
        "dot_shorthand_undefined_member",
    ),
    ("duplicate_constructor_default", "duplicate_constructor"),
    ("duplicate_constructor_name", "duplicate_constructor"),
    ("expected_catch_clause_body", "expected_body"),
    ("expected_class_body", "expected_body"),
    ("expected_extension_body", "expected_body"),
    ("expected_extension_type_body", "expected_body"),
    ("expected_finally_clause_body", "expected_body"),
    ("expected_mixin_body", "expected_body"),
    ("expected_named_type_extends", "expected_named_type"),
    ("expected_named_type_implements", "expected_named_type"),
    ("expected_named_type_on", "expected_named_type"),
    ("expected_named_type_with", "expected_named_type"),
    ("expected_switch_expression_body", "expected_body"),
    ("expected_switch_statement_body", "expected_body"),
    ("expected_try_statement_body", "expected_body"),
    ("extends_deferred_class", "subtype_of_deferred_class"),
    ("extends_disallowed_class", "subtype_of_disallowed_type"),
    (
        "extends_type_alias_expands_to_type_parameter",
        "supertype_expands_to_type_parameter",
    ),
    (
        "external_field_constructor_initializer",
        "external_with_initializer",
    ),
    ("external_field_initializer", "external_with_initializer"),
    ("external_variable_initializer", "external_with_initializer"),
    (
        "final_class_extended_outside_of_library",
        "invalid_use_of_type_outside_library",
    ),
    (
        "final_class_implemented_outside_of_library",
        "invalid_use_of_type_outside_library",
    ),
    (
        "final_class_used_as_mixin_constraint_outside_of_library",
        "invalid_use_of_type_outside_library",
    ),
    (
        "final_not_initialized_constructor_1",
        "final_not_initialized_constructor",
    ),
    (
        "final_not_initialized_constructor_2",
        "final_not_initialized_constructor",
    ),
    (
        "final_not_initialized_constructor_3_plus",
        "final_not_initialized_constructor",
    ),
    ("function_already_complete", "declaration_already_complete"),
    (
        "illegal_concrete_enum_member_declaration",
        "illegal_concrete_enum_member",
    ),
    (
        "illegal_concrete_enum_member_inheritance",
        "illegal_concrete_enum_member",
    ),
    ("illegal_enum_values_declaration", "illegal_enum_values"),
    ("illegal_enum_values_inheritance", "illegal_enum_values"),
    ("implements_deferred_class", "subtype_of_deferred_class"),
    ("implements_disallowed_class", "subtype_of_disallowed_type"),
    (
        "implements_type_alias_expands_to_type_parameter",
        "supertype_expands_to_type_parameter",
    ),
    ("incompatible_lint_files", "incompatible_lint"),
    ("incompatible_lint_included", "incompatible_lint"),
    (
        "instance_access_to_static_member_of_unnamed_extension",
        "instance_access_to_static_member",
    ),
    (
        "interface_class_extended_outside_of_library",
        "invalid_use_of_type_outside_library",
    ),
    ("invalid_field_name_from_object", "invalid_field_name"),
    ("invalid_field_name_positional", "invalid_field_name"),
    ("invalid_field_name_private", "invalid_field_name"),
    (
        "invalid_implementation_override_setter",
        "invalid_implementation_override",
    ),
    (
        "invalid_language_version_override_at_sign",
        "invalid_language_version_override",
    ),
    (
        "invalid_language_version_override_equals",
        "invalid_language_version_override",
    ),
    (
        "invalid_language_version_override_greater",
        "invalid_language_version_override",
    ),
    (
        "invalid_language_version_override_location",
        "invalid_language_version_override",
    ),
    (
        "invalid_language_version_override_lower_case",
        "invalid_language_version_override",
    ),
    (
        "invalid_language_version_override_number",
        "invalid_language_version_override",
    ),
    (
        "invalid_language_version_override_prefix",
        "invalid_language_version_override",
    ),
    (
        "invalid_language_version_override_trailing_characters",
        "invalid_language_version_override",
    ),
    (
        "invalid_language_version_override_two_slashes",
        "invalid_language_version_override",
    ),
    ("invalid_null_aware_element", "invalid_null_aware_operator"),
    (
        "invalid_null_aware_map_entry_key",
        "invalid_null_aware_operator",
    ),
    (
        "invalid_null_aware_map_entry_value",
        "invalid_null_aware_operator",
    ),
    (
        "invalid_null_aware_operator_after_short_circuit",
        "invalid_null_aware_operator",
    ),
    ("invalid_override_setter", "invalid_override"),
    (
        "invalid_reference_to_generative_enum_constructor_tearoff",
        "invalid_reference_to_generative_enum_constructor",
    ),
    (
        "invalid_type_argument_in_const_list",
        "invalid_type_argument_in_const_literal",
    ),
    (
        "invalid_type_argument_in_const_map",
        "invalid_type_argument_in_const_literal",
    ),
    (
        "invalid_type_argument_in_const_set",
        "invalid_type_argument_in_const_literal",
    ),
    (
        "list_element_type_not_assignable_nullability",
        "list_element_type_not_assignable",
    ),
    (
        "map_key_type_not_assignable_nullability",
        "map_key_type_not_assignable",
    ),
    (
        "map_value_type_not_assignable_nullability",
        "map_value_type_not_assignable",
    ),
    (
        "missing_default_value_for_parameter_positional",
        "missing_default_value_for_parameter",
    ),
    (
        "missing_default_value_for_parameter_with_annotation",
        "missing_default_value_for_parameter",
    ),
    (
        "missing_override_of_must_be_overridden_one",
        "missing_override_of_must_be_overridden",
    ),
    (
        "missing_override_of_must_be_overridden_three_plus",
        "missing_override_of_must_be_overridden",
    ),
    (
        "missing_override_of_must_be_overridden_two",
        "missing_override_of_must_be_overridden",
    ),
    (
        "missing_required_param_with_details",
        "missing_required_param",
    ),
    (
        "mixin_application_no_concrete_super_invoked_setter",
        "mixin_application_no_concrete_super_invoked_member",
    ),
    ("mixin_deferred_class", "subtype_of_deferred_class"),
    ("mixin_of_disallowed_class", "subtype_of_disallowed_type"),
    (
        "mixin_of_type_alias_expands_to_type_parameter",
        "supertype_expands_to_type_parameter",
    ),
    (
        "mixin_on_type_alias_expands_to_type_parameter",
        "supertype_expands_to_type_parameter",
    ),
    (
        "mixin_subtype_of_base_is_not_base",
        "subtype_of_base_or_final_is_not_base_final_or_sealed",
    ),
    (
        "mixin_subtype_of_final_is_not_base",
        "subtype_of_base_or_final_is_not_base_final_or_sealed",
    ),
    (
        "mixin_super_class_constraint_disallowed_class",
        "subtype_of_disallowed_type",
    ),
    ("mixins_super_class", "implements_super_class"),
    ("multiple_combinators_deprecated", "multiple_combinators"),
    ("new_with_non_type", "creation_with_non_type"),
    (
        "no_default_super_constructor_explicit",
        "no_default_super_constructor",
    ),
    (
        "no_default_super_constructor_implicit",
        "no_default_super_constructor",
    ),
    (
        "non_abstract_class_inherits_abstract_member_five_plus",
        "non_abstract_class_inherits_abstract_member",
    ),
    (
        "non_abstract_class_inherits_abstract_member_four",
        "non_abstract_class_inherits_abstract_member",
    ),
    (
        "non_abstract_class_inherits_abstract_member_one",
        "non_abstract_class_inherits_abstract_member",
    ),
    (
        "non_abstract_class_inherits_abstract_member_three",
        "non_abstract_class_inherits_abstract_member",
    ),
    (
        "non_abstract_class_inherits_abstract_member_two",
        "non_abstract_class_inherits_abstract_member",
    ),
    (
        "non_const_call_to_literal_constructor_using_new",
        "non_const_call_to_literal_constructor",
    ),
    (
        "non_constant_list_element_from_deferred_library",
        "collection_element_from_deferred_library",
    ),
    (
        "non_constant_map_key_from_deferred_library",
        "collection_element_from_deferred_library",
    ),
    (
        "non_constant_map_value_from_deferred_library",
        "collection_element_from_deferred_library",
    ),
    (
        "non_exhaustive_switch_expression_private",
        "non_exhaustive_switch_expression",
    ),
    (
        "non_exhaustive_switch_statement_private",
        "non_exhaustive_switch_statement",
    ),
    (
        "not_enough_positional_arguments_name_plural",
        "not_enough_positional_arguments",
    ),
    (
        "not_enough_positional_arguments_name_singular",
        "not_enough_positional_arguments",
    ),
    (
        "not_enough_positional_arguments_plural",
        "not_enough_positional_arguments",
    ),
    (
        "not_enough_positional_arguments_singular",
        "not_enough_positional_arguments",
    ),
    (
        "not_initialized_non_nullable_instance_field_constructor",
        "not_initialized_non_nullable_instance_field",
    ),
    (
        "override_on_non_overriding_field",
        "override_on_non_overriding_member",
    ),
    (
        "override_on_non_overriding_getter",
        "override_on_non_overriding_member",
    ),
    (
        "override_on_non_overriding_method",
        "override_on_non_overriding_member",
    ),
    (
        "override_on_non_overriding_setter",
        "override_on_non_overriding_member",
    ),
    (
        "pattern_variable_shared_case_scope_different_finality_or_type",
        "invalid_pattern_variable_in_shared_case_scope",
    ),
    (
        "pattern_variable_shared_case_scope_has_label",
        "invalid_pattern_variable_in_shared_case_scope",
    ),
    (
        "pattern_variable_shared_case_scope_not_all_cases",
        "invalid_pattern_variable_in_shared_case_scope",
    ),
    (
        "record_literal_one_positional_no_trailing_comma_by_type",
        "record_literal_one_positional_no_trailing_comma",
    ),
    (
        "recursive_factory_redirect",
        "recursive_constructor_redirect",
    ),
    (
        "recursive_interface_inheritance_extends",
        "recursive_interface_inheritance",
    ),
    (
        "recursive_interface_inheritance_implements",
        "recursive_interface_inheritance",
    ),
    (
        "recursive_interface_inheritance_on",
        "recursive_interface_inheritance",
    ),
    (
        "recursive_interface_inheritance_with",
        "recursive_interface_inheritance",
    ),
    (
        "return_of_invalid_type_from_catch_error",
        "invalid_return_type_for_catch_error",
    ),
    (
        "return_of_invalid_type_from_constructor",
        "return_of_invalid_type",
    ),
    (
        "return_of_invalid_type_from_function",
        "return_of_invalid_type",
    ),
    (
        "return_of_invalid_type_from_method",
        "return_of_invalid_type",
    ),
    (
        "return_of_invalid_type_from_then",
        "invalid_return_type_for_then",
    ),
    (
        "return_type_invalid_for_catch_error",
        "invalid_return_type_for_catch_error",
    ),
    (
        "return_type_invalid_for_then",
        "invalid_return_type_for_then",
    ),
    (
        "sealed_class_subtype_outside_of_library",
        "invalid_use_of_type_outside_library",
    ),
    (
        "set_element_from_deferred_library",
        "collection_element_from_deferred_library",
    ),
    (
        "set_element_type_not_assignable_nullability",
        "set_element_type_not_assignable",
    ),
    (
        "subtype_of_base_is_not_base_final_or_sealed",
        "subtype_of_base_or_final_is_not_base_final_or_sealed",
    ),
    (
        "subtype_of_final_is_not_base_final_or_sealed",
        "subtype_of_base_or_final_is_not_base_final_or_sealed",
    ),
    (
        "subtype_of_struct_class_in_extends",
        "subtype_of_struct_class",
    ),
    (
        "subtype_of_struct_class_in_implements",
        "subtype_of_struct_class",
    ),
    ("subtype_of_struct_class_in_with", "subtype_of_struct_class"),
    ("type_check_is_not_null", "type_check_with_null"),
    ("type_check_is_null", "type_check_with_null"),
    (
        "unchecked_invocation_of_nullable_value",
        "unchecked_use_of_nullable_value",
    ),
    (
        "unchecked_method_invocation_of_nullable_value",
        "unchecked_use_of_nullable_value",
    ),
    (
        "unchecked_operator_invocation_of_nullable_value",
        "unchecked_use_of_nullable_value",
    ),
    (
        "unchecked_property_access_of_nullable_value",
        "unchecked_use_of_nullable_value",
    ),
    (
        "unchecked_use_of_nullable_value_as_condition",
        "unchecked_use_of_nullable_value",
    ),
    (
        "unchecked_use_of_nullable_value_as_iterator",
        "unchecked_use_of_nullable_value",
    ),
    (
        "unchecked_use_of_nullable_value_in_spread",
        "unchecked_use_of_nullable_value",
    ),
    (
        "unchecked_use_of_nullable_value_in_yield_each",
        "unchecked_use_of_nullable_value",
    ),
    ("undefined_class_boolean", "undefined_class"),
    (
        "undefined_constructor_in_initializer_default",
        "undefined_constructor_in_initializer",
    ),
    (
        "undefined_enum_constructor_named",
        "undefined_enum_constructor",
    ),
    (
        "undefined_enum_constructor_unnamed",
        "undefined_enum_constructor",
    ),
    ("undefined_getter_on_function_type", "undefined_getter"),
    ("undefined_method_on_function_type", "undefined_method"),
    ("undefined_method_on_type_literal", "undefined_method"),
    ("undefined_setter_on_function_type", "undefined_setter"),
    ("undefined_super_getter", "undefined_super_member"),
    ("undefined_super_method", "undefined_super_member"),
    ("undefined_super_operator", "undefined_super_member"),
    ("undefined_super_setter", "undefined_super_member"),
    (
        "unnecessary_nan_comparison_false",
        "unnecessary_nan_comparison",
    ),
    (
        "unnecessary_nan_comparison_true",
        "unnecessary_nan_comparison",
    ),
    (
        "unnecessary_null_comparison_always_null_false",
        "unnecessary_null_comparison",
    ),
    (
        "unnecessary_null_comparison_always_null_true",
        "unnecessary_null_comparison",
    ),
    (
        "unnecessary_null_comparison_never_null_false",
        "unnecessary_null_comparison",
    ),
    (
        "unnecessary_null_comparison_never_null_true",
        "unnecessary_null_comparison",
    ),
    ("unnecessary_type_check_false", "unnecessary_type_check"),
    ("unnecessary_type_check_true", "unnecessary_type_check"),
    ("unsupported_option_with_legal_value", "unsupported_option"),
    ("unsupported_option_with_legal_values", "unsupported_option"),
    ("unsupported_option_without_values", "unsupported_option"),
    ("unused_result_with_message", "unused_result"),
    ("use_of_private_parameter_name", "undefined_named_parameter"),
    (
        "wrong_number_of_parameters_for_operator_minus",
        "wrong_number_of_parameters_for_operator",
    ),
    (
        "wrong_number_of_type_arguments_dot_shorthand_constructor",
        "wrong_number_of_type_arguments_constructor",
    ),
    ("yield_each_in_non_generator", "yield_in_non_generator"),
    ("yield_each_of_invalid_type", "yield_of_invalid_type"),
];

/// Applies the ignore-comment face to one file's assembled run: non-ERROR
/// rows whose code a suppression entry names — or whose family a `type=`
/// entry cites — drop (`_filterIgnoredDiagnostics` — errors stay,
/// warnings/hints/lints suppress). A table-less file pays one scan and
/// exits; otherwise a single source walk resolves every row's line (the
/// run is source-ordered, so the cursor only advances).
pub(crate) fn apply_ignore_suppression(
    diagnostics: &mut Vec<Diagnostic>,
    source: &str,
    tokens: &TokenTable,
) {
    let face = IgnoreSuppression::parse(source, tokens);
    if face.on_line.is_empty() && face.for_file.is_empty() {
        return;
    }
    // The UTF-16 line-start table: one pass, then a binary search per
    // suppressed-candidate row.
    let mut line_starts: Vec<u32> = Vec::new();
    let mut u16_pos: u32 = 0;
    line_starts.push(0);
    for ch in source.chars() {
        if ch == '\n' {
            line_starts.push(u16_pos + 1);
        }
        u16_pos += ch.len_utf16() as u32;
    }
    let line_of =
        |offset: u32| -> u32 { line_starts.partition_point(|start| *start <= offset) as u32 };
    diagnostics.retain(|d| {
        d.severity == Severity::Error
            || !face.suppresses(d.range.offset, line_of, d.code.name, d.code.dtype)
    });
}

/// Applies [`MAX_DIAGNOSTICS_PER_FILE`] to one file's assembled run: the
/// first `CAP` rows in source order survive, the rest drop, and a capped
/// file gains one terminal `dartq/internal_error` row anchored at the last
/// surviving row (where the cap latched — the parser fuse anchors where it
/// stopped). The engine's aggregation sink stays the outer belt; this is the
/// face's own fuse, bounding the diagnose output itself.
pub(crate) fn cap_diagnostics(diagnostics: &mut Vec<Diagnostic>, file: FileId) {
    if diagnostics.len() <= MAX_DIAGNOSTICS_PER_FILE {
        return;
    }
    let anchor = diagnostics[MAX_DIAGNOSTICS_PER_FILE - 1].range;
    diagnostics.truncate(MAX_DIAGNOSTICS_PER_FILE);
    diagnostics.push(Diagnostic::new(
        DARTQ_INTERNAL_ERROR.dtype.severity(),
        DARTQ_INTERNAL_ERROR,
        format!("Too many diagnostics ({MAX_DIAGNOSTICS_PER_FILE}); the rest were suppressed."),
        file,
        anchor,
        None,
    ));
}

/// The names-table use kind of a row's finalized intent.
pub(crate) fn use_kind_of(intent: &Intent) -> NameUse {
    match intent {
        Intent::Invoke => NameUse::Invoke,
        Intent::Write | Intent::ReadWrite => NameUse::Write,
        _ => NameUse::Read,
    }
}

/// Drops the switch-exhaustiveness rows whose switch the pass never had the
/// right to decide (#2336).
///
/// The oracle's exhaustiveness verifier reads the scrutinee's resolved
/// static type first: an `InvalidType` scrutinee
/// (`patternConverter.hasInvalidType`, constant_verifier.dart:1035) gates
/// the whole pass off — `computeExhaustiveness`, the per-case
/// `unreachable_switch_case` walk, and the covering report all sit behind
/// `if (!patternConverter.hasInvalidType)` (:1095). dartq's engine answers
/// before that fact is available: the scrutinee's push-time latch keeps an
/// as-written `Ty::Named` for a type annotation the unit's imports never
/// brought in, and the world's unique-declaration fallback (a sealed-walk
/// aid — `NameWorld::resolve_foreign`) then hands the verdict facts for a
/// name the diagnosing scope cannot see, while the case patterns' own
/// spellings stay unresolved. The rows that survive that mixed-resolution
/// verdict are exactly the rows the oracle never emits.
///
/// The teardown join reads the same scope face the `undefined_class` rows
/// came from ([`resolve_container`], namespace-only — never the fallback):
/// a scrutinee identifier whose latched `Ty::Named` head resolves nowhere
/// is the `InvalidType` shape, and its switch's
/// `non_exhaustive_switch_statement` / `non_exhaustive_switch_expression` /
/// `unreachable_switch_case` rows drop. Everything else keeps verbatim — a
/// resolved head stays decidable (the promotion-narrowed unions the oracle
/// exhausts are a separate face), and an unattributable header or a
/// non-identifier scrutinee keeps its rows (fail-open, the pre-existing
/// behavior).
#[allow(clippy::too_many_arguments)] // the analysis context, one field each
fn gate_invalid_scrutinee_switches(
    diagnostics: &mut Vec<Diagnostic>,
    read_types: &[(SourceRange, Ty)],
    tokens: &TokenTable,
    graph: &LibraryGraph,
    namespaces: &Namespaces,
    index: &ElementIndex,
    file: FileId,
    lib: LibraryId,
) {
    let ne_statement = SemanticCode::NonExhaustiveSwitchStatement.code_info().name;
    let ne_expression = SemanticCode::NonExhaustiveSwitchExpression.code_info().name;
    let unreachable = SemanticCode::UnreachableSwitchCase.code_info().name;
    // One resolution per distinct head: a corpus unit carries many rows on
    // the same broken name (the analyzer corpus' `DeclarationKind` alone
    // anchored three).
    let mut resolvable: FxHashMap<StringId, bool> = FxHashMap::default();
    diagnostics.retain_mut(|row| {
        if row.code.name != ne_statement
            && row.code.name != ne_expression
            && row.code.name != unreachable
        {
            return true;
        }
        // The verdict read the scrutinee's push-time type; the joined read
        // rows carry the same identifier's type at teardown.
        let Some(head) = switch_scrutinee_head(row.range.offset, tokens) else {
            return true;
        };
        let Ok(join) = read_types.binary_search_by_key(&head, |(range, _)| range.offset) else {
            return true;
        };
        let Ty::Named { name, .. } = read_types[join].1 else {
            return true;
        };
        *resolvable.entry(name).or_insert_with(|| {
            resolve_container(graph, namespaces, index, lib, file, name).is_some()
        })
    })
}

/// The scrutinee identifier a switch-family row answers for: the single
/// identifier token between the header's parentheses — directly after a
/// covering row's anchor (that row rides the `switch` keyword) or, for a
/// per-case row, ahead of the enclosing switch body that a backward group
/// scan recovers. The offset is UTF-16 (the `SourceRange` units the read
/// rows join by). `None` keeps the row's rows (a complex scrutinee, or a
/// header the token walk cannot attribute — never a verdict input here).
fn switch_scrutinee_head(offset: u32, tokens: &TokenTable) -> Option<u32> {
    const SWITCH: dartq_scanner::TokenKind =
        dartq_scanner::TokenKind::Keyword(dartq_scanner::Keyword::Switch);
    let slice = tokens.as_slice();
    let anchor = slice.partition_point(|t| t.utf16_offset < offset) as isize;
    // The header's `(`: one token past the `switch` keyword for the
    // covering rows; for the per-case rows the first unmatched `{` walking
    // backward is the switch body's open brace (every nested group closes
    // before its anchor, so only the enclosing body stays open), and the
    // scrutinee parens sit immediately before it.
    let open_paren = match slice.get(anchor as usize) {
        Some(t) if t.kind == SWITCH => anchor + 1,
        _ => {
            let mut depth = 0i32;
            let mut body = None;
            for i in (0..anchor).rev() {
                let step = match slice[i as usize].kind {
                    dartq_scanner::TokenKind::CloseCurly
                    | dartq_scanner::TokenKind::CloseParen
                    | dartq_scanner::TokenKind::CloseSquare => 1,
                    dartq_scanner::TokenKind::OpenCurly
                    | dartq_scanner::TokenKind::StringInterpolationExpression
                    | dartq_scanner::TokenKind::OpenParen
                    | dartq_scanner::TokenKind::OpenSquare => -1,
                    _ => 0,
                };
                depth += step;
                if depth == -1 {
                    body = Some(i as usize);
                    break;
                }
            }
            let body = body?;
            // The body brace must close a `switch (...)` header; anything
            // else (a block-typed anchor walk-off) is unattributable.
            if slice[body].kind != dartq_scanner::TokenKind::OpenCurly {
                return None;
            }
            if !slice
                .get(body.wrapping_sub(1))
                .is_some_and(|t| t.kind == dartq_scanner::TokenKind::CloseParen)
            {
                return None;
            }
            let mut paren = 0i32;
            let mut header_open = None;
            for i in (0..body - 1).rev() {
                match slice[i].kind {
                    dartq_scanner::TokenKind::CloseParen => paren += 1,
                    dartq_scanner::TokenKind::OpenParen => {
                        paren -= 1;
                        if paren == -1 {
                            header_open = Some(i);
                            break;
                        }
                    }
                    _ => {}
                }
            }
            let header_open = header_open? as usize;
            if !slice
                .get(header_open.wrapping_sub(1))
                .is_some_and(|t| t.kind == SWITCH)
            {
                return None;
            }
            header_open as isize
        }
    };
    let close = tokens.matching_end(open_paren as usize)?;
    let inner = slice.get((open_paren + 1) as usize..close)?;
    let [only] = inner else {
        return None;
    };
    (only.kind == dartq_scanner::TokenKind::Identifier).then_some(only.utf16_offset)
}

/// Synthesizes the switch dead-case rows the oracle's InvalidType coverage
/// face owes (#2340).
///
/// Upstream `promoteForPattern` (flow_analysis.dart:7708-7768) marks the
/// matched type covered when the scrutinee's static type is a subtype of
/// the case's required type, and `InvalidType` is a vacuous subtype of
/// everything (subtype.dart:274-276) — so the first guard-free case whose
/// pattern names a resolvable type absorbs the match, the unmatched state
/// goes unreachable, and every following clause reports `dead_code` at its
/// keyword and at its first body statement (the dead-code verifier's
/// `checkUnreachableNode`; probed against `dart analyze`: `case` keyword
/// rows of length 4, `default` of length 7, one body row per clause, empty
/// bodies contributing the head row only). dartq's flow records a
/// broken-chain scrutinee as the unknown face, whose dynamic answer keeps
/// every case live — the oracle's rows never appear.
///
/// The teardown re-decides under the oracle's own posture: a switch whose
/// scrutinee is a bare identifier, or a `<id>.<id>` select's receiver
/// identifier, whose latched `Ty::Named` head resolves nowhere through
/// [`resolve_container`] (namespace-only — the same face the #2336 gate
/// reads) is that `InvalidType`. The clause walk fails the whole switch
/// open on every shape the verdict cannot mirror: a `default` or a
/// constant/wildcard/`p.c`-pattern clause before the absorber, a `when`
/// guard on it (the guard-false arm joins the unmatched state — probed:
/// the oracle keeps the later clauses live), a dynamic-shaped scrutinee
/// head (`var x; switch (x.y)` stays live), a switch expression, or an
/// unattributable header. A synthesized row never lands on an offset the
/// flow's own dead face already anchored.
#[allow(clippy::too_many_arguments)] // the analysis context, one field each
fn synthesize_invalid_scrutinee_dead_cases(
    diagnostics: &[Diagnostic],
    read_types: &[(SourceRange, Ty)],
    tokens: &TokenTable,
    source: &str,
    strings: &StringTable,
    graph: &LibraryGraph,
    namespaces: &Namespaces,
    index: &ElementIndex,
    file: FileId,
    lib: LibraryId,
) -> Vec<Diagnostic> {
    let dead_code = SemanticCode::DeadCode;
    let dead_name = dead_code.code_info().name;
    let anchored: FxHashSet<u32> = diagnostics
        .iter()
        .filter(|d| d.code.name == dead_name)
        .map(|d| d.range.offset)
        .collect();
    // One resolution per distinct head: a corpus unit carries many rows on
    // the same broken name.
    let mut resolvable: FxHashMap<StringId, bool> = FxHashMap::default();
    let slice = tokens.as_slice();
    let mut rows: Vec<Diagnostic> = Vec::new();
    for (i, token) in slice.iter().enumerate() {
        if token.kind != dartq_scanner::TokenKind::Keyword(dartq_scanner::Keyword::Switch) {
            continue;
        }
        if !slice
            .get(i + 1)
            .is_some_and(|t| t.kind == dartq_scanner::TokenKind::OpenParen)
        {
            continue;
        }
        let Some(close) = tokens.matching_end(i + 1) else {
            continue;
        };
        // The posture head: a bare identifier, or the receiver identifier
        // of a `<id>.<id>` select — anything else (calls, prefixed chains,
        // deeper selects) is not decidable at this face.
        let head = match slice.get(i + 2..close) {
            Some([only]) if only.kind == dartq_scanner::TokenKind::Identifier => only.utf16_offset,
            Some([recv, dot, _member])
                if recv.kind == dartq_scanner::TokenKind::Identifier
                    && dot.kind == dartq_scanner::TokenKind::Period =>
            {
                recv.utf16_offset
            }
            _ => continue,
        };
        // The latched read row answers the head's as-written type (a dead
        // read carries no row — fail open with it).
        let Ok(join) = read_types.binary_search_by_key(&head, |(range, _)| range.offset) else {
            continue;
        };
        let Ty::Named { name, .. } = read_types[join].1 else {
            continue;
        };
        if *resolvable.entry(name).or_insert_with(|| {
            resolve_container(graph, namespaces, index, lib, file, name).is_some()
        }) {
            continue;
        }
        // The unresolved-CALL chain face (`var x = nowheresfn();` — the
        // flow's invalid-call typing latches the callee's own head as the
        // row) belongs to the joined synthesizer below, whose
        // undefined-chain verdict owns the initializer face; a plain
        // written annotation has no initializer chain and stays here.
        if undefined_in_scrutinee_chain(diagnostics, tokens, source, slice, i) {
            continue;
        }
        if !slice
            .get(close + 1)
            .is_some_and(|t| t.kind == dartq_scanner::TokenKind::OpenCurly)
        {
            continue;
        }
        let Some(body_close) = tokens.matching_end(close + 1) else {
            continue;
        };
        // The body's top-level clauses: `case`/`default` keywords at group
        // depth 0, each keeping its header colon (the first depth-0 one).
        let mut depth = 0i32;
        let mut clauses: Vec<(usize, Option<usize>)> = Vec::new();
        for j in close + 2..body_close {
            depth += group_depth_step(slice[j].kind);
            if depth != 0 {
                continue;
            }
            match slice[j].kind {
                dartq_scanner::TokenKind::Keyword(dartq_scanner::Keyword::Case)
                | dartq_scanner::TokenKind::Keyword(dartq_scanner::Keyword::Default) => {
                    clauses.push((j, None));
                }
                dartq_scanner::TokenKind::Colon => {
                    if let Some(last) = clauses.last_mut().filter(|(_, colon)| colon.is_none()) {
                        last.1 = Some(j);
                    }
                }
                _ => {}
            }
        }
        // The absorbing case: the first guard-free clause whose pattern
        // names a resolvable type. Any earlier clause shape the verdict
        // cannot mirror abandons the switch; an unresolved pattern type
        // merely does not absorb (the oracle joins it unmatched without a
        // verdict and the scan continues).
        let mut absorber: Option<usize> = None;
        for (idx, &(kw, colon)) in clauses.iter().enumerate() {
            let Some(colon) = colon else {
                break;
            };
            if slice[kw].kind == dartq_scanner::TokenKind::Keyword(dartq_scanner::Keyword::Default)
            {
                break;
            }
            let mut depth = 0i32;
            let mut guarded = false;
            for &t in &slice[kw + 1..colon] {
                depth += group_depth_step(t.kind);
                if depth == 0
                    && t.kind == dartq_scanner::TokenKind::Keyword(dartq_scanner::Keyword::When)
                {
                    guarded = true;
                    break;
                }
            }
            if guarded {
                break;
            }
            // The pattern head: `[final|var] Type (name | _ | ()`. A
            // constant (`case foo:`), bare wildcard (`case _:`) or
            // prefixed (`case p.c:`) pattern has no probed verdict here.
            let mut p = kw + 1;
            if matches!(
                slice.get(p).map(|t| t.kind),
                Some(
                    dartq_scanner::TokenKind::Keyword(dartq_scanner::Keyword::Final)
                        | dartq_scanner::TokenKind::Keyword(dartq_scanner::Keyword::Var)
                )
            ) {
                p += 1;
            }
            let typed = slice
                .get(p)
                .is_some_and(|t| t.kind == dartq_scanner::TokenKind::Identifier)
                && slice.get(p + 1).is_some_and(|t| {
                    matches!(
                        t.kind,
                        dartq_scanner::TokenKind::Identifier | dartq_scanner::TokenKind::OpenParen
                    )
                });
            if !typed {
                break;
            }
            let name = strings.intern(tokens.lexeme(source, p));
            if *resolvable.entry(name).or_insert_with(|| {
                resolve_container(graph, namespaces, index, lib, file, name).is_some()
            }) && !object_pattern_property_invalid(
                slice,
                tokens,
                source,
                strings,
                p,
                graph,
                namespaces,
                index,
                file,
                lib,
                name,
                &mut resolvable,
            ) {
                absorber = Some(idx);
                break;
            }
        }
        let Some(absorber) = absorber else {
            continue;
        };
        for (idx, &(kw, colon)) in clauses.iter().enumerate().skip(absorber + 1) {
            let clause_end = clauses
                .get(idx + 1)
                .map_or(body_close, |&(next_kw, _)| next_kw);
            let kw_token = &slice[kw];
            if !anchored.contains(&kw_token.utf16_offset) {
                rows.push(Diagnostic::new(
                    dead_code.severity(),
                    dead_code.code_info(),
                    dead_code.message("", ""),
                    file,
                    SourceRange {
                        offset: kw_token.utf16_offset,
                        length: kw_token.byte_len,
                    },
                    dead_code.correction_message(""),
                ));
            }
            // The first body node: past the header colon, over leading
            // labels (`l:`) and stray `;`s; an empty body keeps only its
            // head row.
            let Some(colon) = colon else {
                continue;
            };
            let mut b = colon + 1;
            loop {
                match slice.get(b).map(|t| t.kind) {
                    Some(dartq_scanner::TokenKind::Identifier)
                        if slice
                            .get(b + 1)
                            .is_some_and(|t| t.kind == dartq_scanner::TokenKind::Colon) =>
                    {
                        b += 2;
                    }
                    Some(dartq_scanner::TokenKind::Semicolon) => b += 1,
                    _ => break,
                }
            }
            if b >= clause_end || anchored.contains(&slice[b].utf16_offset) {
                continue;
            }
            // The statement interval: to the depth-0 `;` (the row ends at
            // its boundary), else to the clause's end.
            let mut depth = 0i32;
            let mut end_off = slice[clause_end].utf16_offset;
            for k in b..clause_end {
                depth += group_depth_step(slice[k].kind);
                if depth == 0 && slice[k].kind == dartq_scanner::TokenKind::Semicolon {
                    // The oracle's dead interval ends at the terminator
                    // itself (probed: the body row's length covers the
                    // statement through its `;`).
                    end_off = slice[k].utf16_offset + slice[k].byte_len;
                    break;
                }
            }
            rows.push(Diagnostic::new(
                dead_code.severity(),
                dead_code.code_info(),
                dead_code.message("", ""),
                file,
                SourceRange {
                    offset: slice[b].utf16_offset,
                    length: end_off - slice[b].utf16_offset,
                },
                dead_code.correction_message(""),
            ));
        }
    }
    rows
}

/// One switch body's top-level clause, normalized across the statement
/// (`case <pattern>: <statements>`) and expression (`<pattern> => <arm>`)
/// body shapes: the pattern-run span plus the clause's end boundary.
struct SwitchClause {
    /// The first pattern token (the `case`/`default` keyword for a
    /// statement clause, the first pattern token for an expression arm).
    head: usize,
    /// One past the pattern run (the header colon's index for a statement
    /// clause, the `=>` index for an arm).
    pattern_end: usize,
    /// The clause's end boundary index (the next clause's head, else the
    /// body's closing brace).
    end: usize,
    /// A `default` group — never absorbs.
    is_default: bool,
}

/// Synthesizes the switch dead-case rows the oracle owes when the
/// SCRUTINEE EXPRESSION's own static type is the invalid face (#2450) —
/// the row shapes [`synthesize_invalid_scrutinee_dead_cases`] covers for
/// a written-but-unresolvable head annotation, extended to:
///
/// - a bare-identifier scrutinee whose latched read row is `Ty::Unknown`
///   — the unresolved-initializer face (`var x = pop();` under an
///   unresolvable receiver chains the call's `InvalidType` into the
///   variable; the corpus census's `ast_builder.dart`
///   `switch (typeOrFunctionTypedParameter)` / `switch (preliminaryName)`
///   rows), and
/// - switch EXPRESSIONS, whose `pattern => arm` bodies the statement-only
///   walk above never scans (the census's `generic_inferrer.dart` 631-638
///   rows, whose `<unresolvable>.member` receiver carries the written
///   invalid annotation).
///
/// Upstream `promoteForPattern` (flow_analysis.dart:7708-7768) marks the
/// matched type covered when `isSubtypeOf(matched, required)` holds, and
/// the invalid scrutinee is a vacuous subtype of everything
/// (subtype.dart:274-276) — so the first guard-free case whose pattern
/// names a resolvable type absorbs the match, the unmatched state goes
/// unreachable, and every following clause reports `dead_code`. An
/// equality head (`case null:`, `case 42:`) runs the constant-equality
/// face instead (`constantPattern_end` → `_handleEqualityCheckPattern`)
/// and never covers, so it neither absorbs nor abandons the walk — the
/// corpus's `switch (preliminaryName) { case null: … case Token(): … }`
/// reports only the clauses after `Token()` (its 6143-6146 rows). A
/// `when` guard keeps the later clauses live (the guard-false arm joins
/// the unmatched state) and abandons, as does a `default` group.
///
/// The dynamic face stays silent: a written `dynamic` scrutinee latches
/// `Ty::Dynamic`, whose every-case-can-match answer keeps all clauses
/// live (probed: `var x; switch (x.y)`). For a `<id>.<id>` head the row
/// read is the RECEIVER's, so only the written-annotation face fires
/// there — and only for switch expressions, whose rows the statement arm
/// above never produced; the statement face stays that arm's exclusive.
/// Whether the spelling names a dart:core type — the always-resolving face
/// the container lookup cannot see in an analysis rig that carries no core
/// library (`Object` in a switch-expression scrutinee must never take the
/// unresolved-annotation dead-arm face).
fn resolves_as_core_type(lexeme: &str) -> bool {
    crate::member_face::CORE_TYPE_MEMBERS
        .iter()
        .any(|(name, _)| *name == lexeme)
        || super::diagnose::types::EXTERNAL_CORE_TYPE_PARAMS.iter().any(|(name, _)| *name == lexeme)
}

/// Whether an object-pattern clause (`C1(:var name)`) carries a property
/// sub-pattern whose required type resolves nowhere: the official's
/// `promoteForPattern` refuses the invalid known type outright
/// (`flow_analysis.dart:7720-7723`), so the property sub-pattern may fail
/// and the clause cannot absorb vacuously however invalid the scrutinee
/// (oracle-probed on the getter and field variants: the later clauses stay
/// live). `head` indexes the pattern's owner identifier; the property list
/// is the balanced paren group right after it. Non-shorthand labels and
/// unparsable shapes answer `false` — the under-report direction.
fn object_pattern_property_invalid(
    slice: &[dartq_scanner::Token],
    tokens: &TokenTable,
    source: &str,
    strings: &StringTable,
    head: usize,
    graph: &LibraryGraph,
    namespaces: &Namespaces,
    index: &ElementIndex,
    file: FileId,
    lib: LibraryId,
    owner: StringId,
    resolvable: &mut FxHashMap<StringId, bool>,
) -> bool {
    let Some(open) = slice.get(head + 1) else {
        return false;
    };
    if open.kind != dartq_scanner::TokenKind::OpenParen {
        return false;
    }
    let Some(close) = tokens.matching_end(head + 1) else {
        return false;
    };
    // The property segments: depth-0 comma splits inside the group.
    let mut segments: Vec<(usize, usize)> = Vec::new();
    let mut start = head + 2;
    let mut depth = 0i32;
    for j in head + 2..close {
        depth += group_depth_step(slice[j].kind);
        if depth == 0 && slice[j].kind == dartq_scanner::TokenKind::Comma {
            segments.push((start, j));
            start = j + 1;
        }
    }
    segments.push((start, close));
    for &(lo, hi) in &segments {
        if lo >= hi {
            continue;
        }
        // The member name: a shorthand `:var name` binds the member's own
        // name (the LAST identifier of the segment); an explicit
        // `name: <sub-pattern>` carries it first.
        let member = if slice[lo].kind == dartq_scanner::TokenKind::Colon {
            slice[lo + 1..hi]
                .iter()
                .rposition(|t| t.kind == dartq_scanner::TokenKind::Identifier)
                .map(|at| lo + 1 + at)
        } else if slice[lo].kind == dartq_scanner::TokenKind::Identifier
            && slice
                .get(lo + 1)
                .is_some_and(|t| t.kind == dartq_scanner::TokenKind::Colon)
        {
            Some(lo)
        } else {
            None
        };
        let Some(member) = member else {
            continue;
        };
        let member_name = strings.intern(tokens.lexeme(source, member));
        let Some(entry) = member_lookup(graph, namespaces, index, lib, file, owner, member_name)
        else {
            continue;
        };
        let ty = match entry.kind {
            MemberKind::Field => entry.var_ty,
            MemberKind::Getter | MemberKind::Method => entry.return_ty,
            MemberKind::Setter => continue,
        };
        if let Ty::Named { name, .. } = ty
            && !*resolvable.entry(name).or_insert_with(|| {
                resolve_container(graph, namespaces, index, lib, file, name).is_some()
            })
            && !resolves_as_core_type(strings.resolve(name))
        {
            return true;
        }
    }
    false
}

/// Whether an undefined-name row anchors inside the bare scrutinee
/// identifier's value chain — its nearest declaring initializer
/// (`name = expr;`, a later assignment shadowing an earlier declaration) or
/// for-in iterable (`for (... name in iterable)`). The InvalidType posture
/// the synthesizer mirrors rides an unresolved reference in that chain
/// (upstream `InvalidType` propagates only through unresolved names — the
/// `var preliminaryName = pop();` face, where the undefined-method row the
/// oracle also fires poisons the static type); an `Unknown` that is only a
/// local inference gap — an extension member or a spread literal our faces
/// never typed — keeps every clause live, because the oracle holds a real
/// type there and reports nothing.
fn undefined_in_scrutinee_chain(
    diagnostics: &[Diagnostic],
    tokens: &TokenTable,
    source: &str,
    slice: &[dartq_scanner::Token],
    switch_idx: usize,
) -> bool {
    // The bare-head token sits two past the `switch` keyword.
    let Some(head) = slice.get(switch_idx + 2) else {
        return false;
    };
    if head.kind != dartq_scanner::TokenKind::Identifier {
        return false;
    }
    let lexeme = tokens.lexeme(source, switch_idx + 2);
    let mut j = switch_idx;
    while j > 0 {
        j -= 1;
        let token = &slice[j];
        if token.kind != dartq_scanner::TokenKind::Identifier
            || tokens.lexeme(source, j) != lexeme
        {
            continue;
        }
        // The declaring occurrence: `name = initializer;` or a for-in loop
        // variable. A read occurrence (`name.foo`) declares nothing — keep
        // scanning backward.
        let span = if slice
            .get(j + 1)
            .is_some_and(|t| t.kind == dartq_scanner::TokenKind::Eq)
        {
            let Some(start) = slice.get(j + 2).map(|t| t.utf16_offset) else {
                continue;
            };
            let mut depth = 0i32;
            let mut k = j + 2;
            while k < slice.len() {
                depth += group_depth_step(slice[k].kind);
                if depth == 0 && slice[k].kind == dartq_scanner::TokenKind::Semicolon {
                    break;
                }
                k += 1;
            }
            slice
                .get(k)
                .map(|t| (start, t.utf16_offset + t.byte_len))
        } else if j > 0
            && slice[j - 1].kind
                == dartq_scanner::TokenKind::Keyword(dartq_scanner::Keyword::In)
        {
            // The iterable runs to the for-header's closing `)`.
            let Some(start) = slice.get(j + 1).map(|t| t.utf16_offset) else {
                continue;
            };
            let mut depth = 0i32;
            let mut k = j + 1;
            let mut end = None;
            while k < slice.len() {
                if depth == 0 && slice[k].kind == dartq_scanner::TokenKind::CloseParen {
                    end = Some((start, slice[k].utf16_offset + slice[k].byte_len));
                    break;
                }
                depth += group_depth_step(slice[k].kind);
                k += 1;
            }
            end
        } else {
            continue;
        };
        let Some((start, end)) = span else {
            continue;
        };
        return diagnostics.iter().any(|d| {
            d.code.name.starts_with("undefined_")
                && d.range.offset >= start
                && d.range.offset < end
        });
    }
    false
}

#[allow(clippy::too_many_arguments)] // the analysis context, one field each
fn synthesize_unknown_scrutinee_dead_cases(
    diagnostics: &[Diagnostic],
    read_types: &[(SourceRange, Ty)],
    tokens: &TokenTable,
    source: &str,
    strings: &StringTable,
    graph: &LibraryGraph,
    namespaces: &Namespaces,
    index: &ElementIndex,
    file: FileId,
    lib: LibraryId,
) -> Vec<Diagnostic> {
    let dead_code = SemanticCode::DeadCode;
    let dead_name = dead_code.code_info().name;
    let anchored: FxHashSet<u32> = diagnostics
        .iter()
        .filter(|d| d.code.name == dead_name)
        .map(|d| d.range.offset)
        .collect();
    // One resolution per distinct head: a corpus unit carries many rows on
    // the same broken name.
    let mut resolvable: FxHashMap<StringId, bool> = FxHashMap::default();
    let slice = tokens.as_slice();
    let mut rows: Vec<Diagnostic> = Vec::new();
    for (i, token) in slice.iter().enumerate() {
        if token.kind != dartq_scanner::TokenKind::Keyword(dartq_scanner::Keyword::Switch) {
            continue;
        }
        if !slice
            .get(i + 1)
            .is_some_and(|t| t.kind == dartq_scanner::TokenKind::OpenParen)
        {
            continue;
        }
        let Some(close) = tokens.matching_end(i + 1) else {
            continue;
        };
        if !slice
            .get(close + 1)
            .is_some_and(|t| t.kind == dartq_scanner::TokenKind::OpenCurly)
        {
            continue;
        }
        let Some(body_close) = tokens.matching_end(close + 1) else {
            continue;
        };
        // The posture head: a bare identifier, or the receiver identifier
        // of a `<id>.<id>` select — anything else (calls, prefixed chains,
        // deeper selects) is not decidable at this face.
        let (head, head_is_receiver) = match slice.get(i + 2..close) {
            Some([only]) if only.kind == dartq_scanner::TokenKind::Identifier => {
                (only.utf16_offset, false)
            }
            Some([recv, dot, _member])
                if recv.kind == dartq_scanner::TokenKind::Identifier
                    && dot.kind == dartq_scanner::TokenKind::Period =>
            {
                (recv.utf16_offset, true)
            }
            _ => continue,
        };
        // The latched read row answers the head's as-written type (a dead
        // read carries no row — fail open with it). A bare head fires on
        // the unknown face; a receiver head fires only on the
        // written-annotation face (its unknown rows are the probed-live
        // `var x; switch (x.y)` posture), and the receiver face covers
        // only the expression bodies the #2340 statement walk never
        // scanned.
        let Ok(join) = read_types.binary_search_by_key(&head, |(range, _)| range.offset) else {
            continue;
        };
        let mut clauses: Vec<SwitchClause> = Vec::new();
        let mut statement = false;
        build_switch_clauses(slice, close + 2, body_close, &mut clauses, &mut statement);
        if clauses.is_empty() {
            continue;
        }
        // A type parameter spelling is neither a container nor a core type,
        // but the oracle holds a real `TypeParameter` static type there —
        // never the invalid face (oracle-probed: xml's
        // `visit<T extends XmlHasVisitor>(T node) => switch (node)` keeps
        // every typed arm live). Both Named arms below must refuse it.
        let head_type_param = match read_types[join].1 {
            Ty::Named { name, .. } => index.get(file).is_some_and(|els| {
                els.declarations()
                    .any(|(n, decl)| n == name && decl.kind == MicroKind::TypeParam)
            }),
            _ => false,
        };
        let fires = match read_types[join].1 {
            Ty::Unknown => {
                !head_is_receiver
                    && undefined_in_scrutinee_chain(diagnostics, tokens, source, slice, i)
            }
            // The unresolved-call initializer's flow row latches the
            // callee's own head (the invalid-call face): the same
            // undefined-chain verdict admits it, and the row's own head
            // must resolve nowhere as a type — a var whose row names a
            // RESOLVED type (a field read the chain merely fed, a
            // resolvable annotation) keeps every clause live no matter
            // what undefined reference sits in its initializer chain.
            // The no-chain face beside it: a scrutinee whose WRITTEN type
            // (a parameter annotation) resolves nowhere is the oracle's
            // `InvalidType` too — `promoteForPattern`'s 0c relation
            // (subtype.dart:274) makes the first resolvable unguarded
            // clause cover vacuously and every later arm die. The
            // statement shape of that face belongs to the first
            // synthesizer above (its undefined-chain gate delegates
            // there); only the switch EXPRESSION reaches here with no
            // chain.
            Ty::Named { name, .. } if !head_is_receiver => {
                !head_type_param
                    && (undefined_in_scrutinee_chain(diagnostics, tokens, source, slice, i)
                        // A written annotation that resolves nowhere is the
                        // invalid face in a switch EXPRESSION too — but a
                        // dart:core spelling always resolves (the analysis
                        // rigs carry no core library, so the container lookup
                        // alone would misread `Object` as unresolved).
                        || (!statement
                            && !resolves_as_core_type(strings.resolve(name))))
                    && !*resolvable.entry(name).or_insert_with(|| {
                        resolve_container(graph, namespaces, index, lib, file, name).is_some()
                    })
            }
            Ty::Named { name, .. } => {
                head_is_receiver
                    && !head_type_param
                    && !statement
                    && !*resolvable.entry(name).or_insert_with(|| {
                        resolve_container(graph, namespaces, index, lib, file, name).is_some()
                    })
            }
            _ => false,
        };
        if !fires {
            continue;
        }
        let Some(absorber) = switch_clause_absorber(
            slice, tokens, source, strings, &clauses, &mut resolvable, graph, namespaces, index,
            file, lib,
        ) else {
            continue;
        };
        for clause in &clauses[absorber + 1..] {
            if statement {
                let kw_token = &slice[clause.head];
                if !anchored.contains(&kw_token.utf16_offset) {
                    rows.push(Diagnostic::new(
                        dead_code.severity(),
                        dead_code.code_info(),
                        dead_code.message("", ""),
                        file,
                        SourceRange {
                            offset: kw_token.utf16_offset,
                            length: kw_token.byte_len,
                        },
                        dead_code.correction_message(""),
                    ));
                }
                // The first body node: past the header colon, over leading
                // labels (`l:`) and stray `;`s; an empty body keeps only
                // its head row.
                let mut b = clause.pattern_end + 1;
                loop {
                    match slice.get(b).map(|t| t.kind) {
                        Some(dartq_scanner::TokenKind::Identifier)
                            if slice
                                .get(b + 1)
                                .is_some_and(|t| t.kind == dartq_scanner::TokenKind::Colon) =>
                        {
                            b += 2;
                        }
                        Some(dartq_scanner::TokenKind::Semicolon) => b += 1,
                        _ => break,
                    }
                }
                if b >= clause.end || anchored.contains(&slice[b].utf16_offset) {
                    continue;
                }
                // The statement interval: to the depth-0 `;` (the row ends
                // at its boundary), else to the clause's end.
                let mut depth = 0i32;
                let mut end_off = slice[clause.end].utf16_offset;
                for k in b..clause.end {
                    depth += group_depth_step(slice[k].kind);
                    if depth == 0 && slice[k].kind == dartq_scanner::TokenKind::Semicolon {
                        // The oracle's dead interval ends at the
                        // terminator itself (probed: the body row's
                        // length covers the statement through its `;`).
                        end_off = slice[k].utf16_offset + slice[k].byte_len;
                        break;
                    }
                }
                rows.push(Diagnostic::new(
                    dead_code.severity(),
                    dead_code.code_info(),
                    dead_code.message("", ""),
                    file,
                    SourceRange {
                        offset: slice[b].utf16_offset,
                        length: end_off - slice[b].utf16_offset,
                    },
                    dead_code.correction_message(""),
                ));
            } else {
                // The expression arm's row spans the whole case — pattern
                // through the arm expression, the trailing `,` excluded
                // (the oracle's `_ => 2` row shape,
                // dead_code_test.dart's switch-expression faces).
                let offset = slice[clause.head].utf16_offset;
                let last = clause.end - 1;
                let length = slice[last].utf16_offset + slice[last].byte_len - offset;
                if length == 0 || anchored.contains(&offset) {
                    continue;
                }
                rows.push(Diagnostic::new(
                    dead_code.severity(),
                    dead_code.code_info(),
                    dead_code.message("", ""),
                    file,
                    SourceRange { offset, length },
                    dead_code.correction_message(""),
                ));
            }
        }
    }
    rows
}

/// Collects one switch body's top-level clauses into [`SwitchClause`]s.
/// A statement body (a depth-0 `case`/`default` keyword) yields one clause
/// per keyword, each keeping its header colon; an expression body yields
/// one clause per depth-0 `=>` arm, bounded by the next depth-0 `,` (or
/// the body's `}`). Sets `statement` when the body is the statement shape.
fn build_switch_clauses(
    slice: &[dartq_scanner::Token],
    from: usize,
    body_close: usize,
    clauses: &mut Vec<SwitchClause>,
    statement: &mut bool,
) {
    let mut depth = 0i32;
    let mut arm_start = from;
    let mut arm_arrow: Option<usize> = None;
    for j in from..body_close {
        depth += group_depth_step(slice[j].kind);
        if depth != 0 {
            continue;
        }
        match slice[j].kind {
            dartq_scanner::TokenKind::Keyword(dartq_scanner::Keyword::Case)
            | dartq_scanner::TokenKind::Keyword(dartq_scanner::Keyword::Default) => {
                *statement = true;
                clauses.push(SwitchClause {
                    head: j,
                    pattern_end: j,
                    end: body_close,
                    is_default: slice[j].kind
                        == dartq_scanner::TokenKind::Keyword(dartq_scanner::Keyword::Default),
                });
            }
            dartq_scanner::TokenKind::Colon if *statement => {
                if let Some(last) = clauses.last_mut().filter(|c| c.pattern_end == c.head) {
                    last.pattern_end = j;
                }
            }
            dartq_scanner::TokenKind::Function if !*statement => {
                arm_arrow = Some(j);
            }
            dartq_scanner::TokenKind::Comma if !*statement => {
                if let Some(arrow) = arm_arrow.take() {
                    clauses.push(SwitchClause {
                        head: arm_start,
                        pattern_end: arrow,
                        end: j,
                        is_default: false,
                    });
                }
                arm_start = j + 1;
            }
            _ => {}
        }
    }
    if !*statement
        && let Some(arrow) = arm_arrow
    {
        clauses.push(SwitchClause {
            head: arm_start,
            pattern_end: arrow,
            end: body_close,
            is_default: false,
        });
    }
    if *statement {
        // A statement clause's body ends where the next clause begins.
        for i in 0..clauses.len() {
            clauses[i].end = clauses.get(i + 1).map_or(body_close, |c| c.head);
        }
    }
}

/// The absorbing clause: the first guard-free clause whose pattern names a
/// resolvable type. Constant (`case foo:`), bare wildcard (`case _:`) and
/// prefixed (`case p.c:`) heads never cover — the scan continues past
/// them; a `when` guard or a `default` group abandons the whole switch
/// (the verdict cannot mirror it).
fn switch_clause_absorber(
    slice: &[dartq_scanner::Token],
    tokens: &TokenTable,
    source: &str,
    strings: &StringTable,
    clauses: &[SwitchClause],
    resolvable: &mut FxHashMap<StringId, bool>,
    graph: &LibraryGraph,
    namespaces: &Namespaces,
    index: &ElementIndex,
    file: FileId,
    lib: LibraryId,
) -> Option<usize> {
    for (idx, clause) in clauses.iter().enumerate() {
        if clause.is_default {
            return None;
        }
        // A statement clause without its header colon is a shape the
        // verdict cannot mirror.
        if clause.pattern_end == clause.head {
            return None;
        }
        let mut depth = 0i32;
        let mut guarded = false;
        for &t in &slice[clause.head..clause.pattern_end] {
            depth += group_depth_step(t.kind);
            if depth == 0
                && t.kind == dartq_scanner::TokenKind::Keyword(dartq_scanner::Keyword::When)
            {
                guarded = true;
                break;
            }
        }
        if guarded {
            return None;
        }
        // The pattern head: `[final|var] Type (name | _ | ()`. A statement
        // clause's span starts at the `case` keyword — step over it.
        let mut p = clause.head;
        if matches!(
            slice[p].kind,
            dartq_scanner::TokenKind::Keyword(dartq_scanner::Keyword::Case)
        ) {
            p += 1;
        }
        if matches!(
            slice.get(p).map(|t| t.kind),
            Some(
                dartq_scanner::TokenKind::Keyword(dartq_scanner::Keyword::Final)
                    | dartq_scanner::TokenKind::Keyword(dartq_scanner::Keyword::Var)
            )
        ) {
            p += 1;
        }
        let typed = slice
            .get(p)
            .is_some_and(|t| t.kind == dartq_scanner::TokenKind::Identifier)
            && slice.get(p + 1).is_some_and(|t| {
                matches!(
                    t.kind,
                    dartq_scanner::TokenKind::Identifier | dartq_scanner::TokenKind::OpenParen
                )
            });
        if !typed {
            continue;
        }
        let name = strings.intern(tokens.lexeme(source, p));
        if *resolvable.entry(name).or_insert_with(|| {
            resolve_container(graph, namespaces, index, lib, file, name).is_some()
        }) && !object_pattern_property_invalid(
            slice, tokens, source, strings, p, graph, namespaces, index, file, lib, name,
            resolvable,
        ) {
            return Some(idx);
        }
    }
    None
}

/// The forward group-depth step: openers descend a level, closers climb
/// back, every other token is depth-neutral. (An interpolation expression
/// is one self-contained token — its internal braces are its own.)
fn group_depth_step(kind: dartq_scanner::TokenKind) -> i32 {
    match kind {
        dartq_scanner::TokenKind::OpenCurly
        | dartq_scanner::TokenKind::OpenParen
        | dartq_scanner::TokenKind::OpenSquare => 1,
        dartq_scanner::TokenKind::CloseCurly
        | dartq_scanner::TokenKind::CloseParen
        | dartq_scanner::TokenKind::CloseSquare => -1,
        _ => 0,
    }
}

#[cfg(test)]
mod suppression_tests {
    //! The `IgnoreSuppression` join pins (#2137, #2277): the citation match
    //! is the upstream `IgnoredDiagnosticName._matches` two-leg verdict
    //! (`ignore_info.dart`:39-52) — a comment name suppresses a diagnostic
    //! whose `name` OR `uniqueName` it equals — beside the line-attribution
    //! faces (`forDart`'s beforeMatch gate), the string-/block-safety of the
    //! comment extraction, and the `ignoredElements` grammar (comma-only
    //! continuation, `type=` families).

    use super::*;

    /// One parsed face, one `line_of` table: the `// ignore:` on line 1
    /// targets line 2, the `ignore_for_file:` covers every line. Offsets
    /// are UTF-16 — the corpus is ASCII, so byte offsets serve.
    fn parse_face(source: &str) -> (IgnoreSuppression, impl Fn(u32) -> u32 + '_) {
        let file = dartq_source::FileTable::new().intern(std::path::Path::new("t.dart"));
        let scanned = dartq_scanner::scan(
            source,
            dartq_scanner::ScanConfig {
                enable_triple_shift: true,
                enable_augmentations: true,
            },
            file,
        );
        let face = IgnoreSuppression::parse(source, &scanned.tokens);
        let starts: Vec<u32> = std::iter::once(0)
            .chain(source.match_indices('\n').map(|(i, _)| (i + 1) as u32))
            .collect();
        let line_of = move |offset: u32| starts.partition_point(|s| *s <= offset) as u32;
        (face, line_of)
    }

    /// The divergence this ticket closes: a citation spelling the
    /// diagnostic's `uniqueName` (`invalid_null_aware_operator`'s second
    /// registry constant spells `..._after_short_circuit`) suppresses the
    /// row reported under the shared `name` — upstream `_matches`'s second
    /// leg, which the name-only join let fire.
    #[test]
    fn unique_name_citation_suppresses_the_shared_name_row() {
        let (face, line_of) = parse_face(
            "// ignore: invalid_null_aware_operator_after_short_circuit\nvar x = a?.b ?? c;\n",
        );
        assert!(face.suppresses(
            59,
            &line_of,
            "invalid_null_aware_operator",
            DiagnosticType::StaticWarning
        ));
        // The same citation never reaches an unrelated code on that line.
        assert!(!face.suppresses(59, &line_of, "dead_code", DiagnosticType::StaticWarning));
    }

    /// `ignore_for_file:` joins the same canonical leg file-wide.
    #[test]
    fn unique_name_for_file_citation_suppresses_every_line() {
        let (face, line_of) =
            parse_face("// ignore_for_file: undefined_super_getter\nclass C {}\nvar x = 0;\n");
        // `undefined_super_getter` is the getter variant's uniqueName; the
        // row reports under `undefined_super_member`.
        assert!(face.suppresses(
            43,
            &line_of,
            "undefined_super_member",
            DiagnosticType::CompileTimeError
        ));
        assert!(face.suppresses(
            54,
            &line_of,
            "undefined_super_member",
            DiagnosticType::CompileTimeError
        ));
    }

    /// The name leg is unchanged: a `name` spelling still suppresses, a
    /// spelling that is neither `name` nor a registry `uniqueName`
    /// resolving to it never does (the under-report direction — no
    /// widening past the oracle's match).
    #[test]
    fn name_citation_still_suppresses_and_unknown_spellings_never_do() {
        let (face, line_of) = parse_face("// ignore: dead_code\nvar x = 0;\n");
        assert!(face.suppresses(21, &line_of, "dead_code", DiagnosticType::StaticWarning));
        assert!(face.suppresses(21, &line_of, "DEAD_CODE", DiagnosticType::StaticWarning));
        assert!(!face.suppresses(21, &line_of, "unused_local_variable", DiagnosticType::Hint));
        // A uniqueName of a DIFFERENT code family never crosses.
        let (face, line_of) = parse_face("// ignore: undefined_super_setter\nvar x = 0;\n");
        assert!(face.suppresses(
            34,
            &line_of,
            "undefined_super_member",
            DiagnosticType::CompileTimeError
        ));
        assert!(!face.suppresses(
            34,
            &line_of,
            "undefined_method",
            DiagnosticType::CompileTimeError
        ));
        // Registry-silent spellings resolve to nothing.
        assert_eq!(canonical_diagnostic_name("not_a_real_code"), None);
        assert_eq!(canonical_diagnostic_name("dead_code"), None);
    }

    /// The trailing-comment face (`forDart`'s beforeMatch gate,
    /// ignore_info.dart:126-133): a comment after code suppresses its OWN
    /// line — the archive bzip2 row's shape — never the next one.
    #[test]
    fn trailing_comment_suppresses_its_own_line() {
        let (face, line_of) = parse_face("var x = 0; // ignore: dead_code\nvar y = 0;\n");
        assert!(face.suppresses(0, &line_of, "dead_code", DiagnosticType::StaticWarning));
        assert!(!face.suppresses(32, &line_of, "dead_code", DiagnosticType::StaticWarning));
    }

    /// String safety: an ignore citation inside a string literal lies in
    /// the token's byte range, never in an inter-token gap — no entry.
    #[test]
    fn ignore_text_inside_a_string_literal_never_suppresses() {
        let (face, line_of) = parse_face("var s = '// ignore: dead_code';\nvar x = 0;\n");
        assert!(!face.suppresses(32, &line_of, "dead_code", DiagnosticType::StaticWarning));
    }

    /// A `//` inside a block comment is block body, not a line comment.
    #[test]
    fn ignore_text_inside_a_block_comment_never_suppresses() {
        let (face, line_of) = parse_face("/* // ignore: dead_code */\nvar x = 0;\n");
        assert!(!face.suppresses(27, &line_of, "dead_code", DiagnosticType::StaticWarning));
    }

    /// The list continues only through a comma (`ignoredElements`,
    /// ignore_info.dart:450-467): whitespace-then-word is unstructured
    /// prose, so `dead_code unused_local_variable` registers `dead_code`
    /// alone.
    #[test]
    fn prose_after_a_name_never_joins_the_list() {
        let (face, line_of) =
            parse_face("// ignore: dead_code unused_local_variable\nvar x = 0;\n");
        assert!(face.suppresses(43, &line_of, "dead_code", DiagnosticType::StaticWarning));
        assert!(!face.suppresses(43, &line_of, "unused_local_variable", DiagnosticType::Hint));
    }

    /// The `type=` family face (`IgnoredDiagnosticType._matches`):
    /// `type=warning` hits the STATIC_WARNING rows of the line, `type=lint`
    /// the lints; hints and non-ignorable families stay.
    #[test]
    fn type_family_citations_match_their_family_only() {
        let (face, line_of) = parse_face("// ignore: type=warning, type=lint\nvar x = 0;\n");
        assert!(face.suppresses(35, &line_of, "dead_code", DiagnosticType::StaticWarning));
        assert!(face.suppresses(35, &line_of, "some_lint_rule", DiagnosticType::Lint));
        assert!(!face.suppresses(35, &line_of, "unused_local_variable", DiagnosticType::Hint));
        assert!(!face.suppresses(
            35,
            &line_of,
            "expected_semicolon",
            DiagnosticType::SyntacticError
        ));
    }

    /// The alias table walks upstream-clean: sorted by the `uniqueName`
    /// column (the binary search's precondition), row count pinned at the
    /// registry's 225 divergent constants, no self-alias row.
    #[test]
    fn the_alias_table_walks_upstream_clean() {
        assert_eq!(UNIQUE_NAME_ALIASES.len(), 225);
        for window in UNIQUE_NAME_ALIASES.windows(2) {
            assert!(
                window[0].0 < window[1].0,
                "alias column unsorted at {}: {}",
                window[0].0,
                window[1].0
            );
        }
        for &(unique, name) in UNIQUE_NAME_ALIASES {
            assert_ne!(unique, name, "self-alias row {unique}");
        }
        assert_eq!(
            canonical_diagnostic_name("invalid_null_aware_operator_after_short_circuit"),
            Some("invalid_null_aware_operator")
        );
    }

    /// A `// ignore:` comment in the tail gap after the LAST token still
    /// joins the table — `line_comments_of` used to flush only inter-token
    /// gaps and dropped a comment at end-of-file (upstream
    /// `IgnoreInfo.forDart` reads the unit's whole comment list). A
    /// trailing comment suppresses its own line; the `}` is the last token,
    /// so the comment's gap is the post-token tail.
    #[test]
    fn tail_gap_comment_after_the_last_token_suppresses() {
        let (face, line_of) =
            parse_face("void f() {}\n// ignore: unused_local_variable\nvar x = 0;");
        // `var x` sits on line 3; the alone-on-line `// ignore:` targets
        // line 3 — which only registers once the tail gap is scanned.
        assert!(face.suppresses(45, &line_of, "unused_local_variable", DiagnosticType::Hint));
    }
}

#[cfg(test)]
mod ticket_2231_tests;

#[cfg(test)]
mod ticket_2297_tests;












/// The #2414 pins (U1/R1): the `?.` short-circuit's result nullability —
/// `a?.b` types `B?` and the `?` propagates through the whole guarded
/// chain (`a?.b.c` types `C?`), so the assignment, return, argument, and
/// inference faces see the nullable result; the continuation's receiver
/// check still reads the member's own written type (`a?.b.c` on `B?`
/// reports `unchecked_use_of_nullable_value`, on `B` stays silent).
#[cfg(test)]
mod ticket_2414_tests;











/// The #2433 pins (LAW A phase A3): a member face read through a
/// parameterized receiver substitutes the receiver's WRITTEN type
/// arguments — `Box<int> b; b.set(v)` checks `v` against `int`. Bare-head
/// receivers keep the declared face, and nested instantiations substitute
/// one level deep.
#[cfg(test)]
mod ticket_2433_tests;




/// The #2440 pins (LAW A phase A2): a closure literal passed where a
/// `Function(...)`-typed formal is expected binds its implicit parameters
/// from that formal's schema, instantiating the callee's own inference
/// parameters through this call's solution (explicit type arguments, the
/// receiver's written instantiation, and the upward unification of the
/// arguments already typed). Cross-file callees are the census shape: their
/// parameter names prove nothing at the call site, so the unresolvable-schema
/// recovery left every callback parameter `Object?` (339
/// `unchecked_use_of_nullable_value` rows).
#[cfg(test)]
mod ticket_2440_tests;


























/// The #2481 pins: the `var`-inference window's extension-face receiver
/// typing — a member read the merge-time field face leaves `Unknown`
/// answers its accessor's declared return (the instance accessor face,
/// then the applied extension face under the unique-declarer gate), so an
/// extension use reached through the inferred local credits; the unused
/// member beside a typed var still reports, the ambiguous two-face
/// application credits nothing, and the `?.`-merged initializer keeps the
/// nullable face.
#[cfg(test)]
mod ticket_2481_tests;

/// The #2491 pins: a switch-case object pattern's named type never lends
/// its type-argument group to an older expression's row — the group-close
/// deferral anchors to a trivia-adjacent head row only, so the pattern's
/// `<` reports nothing against the previous case body's last send (the
/// census's `wrong_number_of_type_arguments_element` at the NEXT case's
/// pattern), while send-position arity keeps every adjacent-head anchor.
#[cfg(test)]
mod ticket_2491_tests;

/// The #2471 pins: the switch return-exhaustiveness face
/// `body_might_complete_normally` reads — a switch over a sealed
/// scrutinee whose every case returns cannot complete normally. The
/// generic-extension `this` scrutinee closes through the recorder's
/// `on`-type head capture; the open-scrutinee reporting direction and
/// the plain-method/non-generic keep-greens guard the capture's edges.
#[cfg(test)]
mod ticket_2471_tests;


/// The #2324 pins: the dead-coalesce operand faces (the `?.`-merged
/// read/send keeps the null union, an invoked operand reads its send's
/// declared result, a generic key send never anchors) and the extension
/// on-type field-read owner face (#2282's spare, unnamed-extension
/// spelling). Kept as a standalone module so the shared `tests` rig stays
/// untouched.
#[cfg(test)]
mod ticket_2324_tests;
/// The #2336 pins: the InvalidType-scrutinee gate over the switch
/// exhaustiveness rows — a broken-import scrutinee (the oracle's
/// `hasInvalidType` posture, constant_verifier.dart:1035/:1095) suppresses
/// the covering and per-case rows, while every resolved-scrutinee verdict
/// (missing constant, full coverage, the nullable-arm reachability row)
/// keeps its official shape.
#[cfg(test)]
mod ticket_2336_tests;
/// The #2340 pins: the InvalidType-scrutinee coverage face over the switch
/// dead-case rows — the oracle's vacuous-subtype absorption
/// (subtype.dart:274-276) deads every clause after the first guard-free
/// resolvable-typed case, while the twin over-report face
/// (`pattern_never_matches_value_type`/`unreachable_switch_default` on an
/// unresolved pattern type) stays silent.
#[cfg(test)]
mod ticket_2340_tests;
/// The #2359 pins: the body-position function-type capture — written
/// function types inside block bodies intern real shapes so the
/// shape-keyed sentinels decide same-signature type checks.
#[cfg(test)]
mod ticket_2359_tests;
/// The #2318 pins: the extension-member element-identity credit — a
/// resolved implicit application spares its own member only, so
/// same-spelled members of sibling extensions (and unresolved accesses)
/// keep reporting.
#[cfg(test)]
mod ticket_2318_tests;
/// The #2344 pins: the member-read gate on the nullable-condition join —
/// a staged condition at a member token decides on the property face
/// alone, so a same-named promoted local never hides the member's
/// `unchecked_use_of_nullable_value`, while shadow-free members, the
/// bare local face, and promotable-field promotions keep their verdicts.
#[cfg(test)]
mod ticket_2344_tests;
/// The #2365 pins: the bare-identifier implicit-`call` invocation's
/// result type — a callable class instance invoked bare types the send
/// with the `call` method's return (substituted when the method is
/// generic), so `return body(inner)` stops over-firing
/// `return_of_invalid_type` on the callee's own class, while
/// non-callable callees and the dot-spelled control keep their faces.
#[cfg(test)]
mod ticket_2365_tests;
/// The #2325 pins: a mixin application's member face substitutes the
/// mixin's own type parameters with the application's written `with`
/// arguments at the member-table fold — `XmlElement`'s `children` reads
/// `XmlNodeList<XmlNode>`, never the mixin's `XmlNodeList<T>` spelling.
#[cfg(test)]
mod ticket_2325_tests;
/// The #2329 pins: the annotation name's resolution order — locals, the
/// container's type parameters, the own-container member face (own statics
/// for class bodies, every own member inside an extension body), then the
/// scoped top-level/import lookup. The climbed faces — the superclass
/// `static const`, the own instance members, the method's own type
/// parameters, the object members — never answer, so their names miss into
/// `undefined_annotation` exactly where the 3.13.1 oracle reports.
#[cfg(test)]
mod ticket_2329_tests;
/// The #2159 pins: the `unused_field` read-attribution faces of the
/// census's reads-side family — the `?.` chain's nested private-field
/// read, the private-field read inside a getter body, the const-list
/// field's read, the enum constant's access — each locks the oracle's
/// `readMembers`/`unresolvedReadMembers` sparing against regression. The
/// cross-part close itself lives at `semdiag.rs`'s
/// `part_file_unused_field_closes_library_wide` (#2286).
#[cfg(test)]
mod ticket_2159_tests;
/// The #2405 pins: the `ambiguous_set_or_map_literal_both` over-report
/// cluster — a typed `<int, dynamic>{}` entry value whose `<...>` group
/// comma mis-split the leaf scan into a phantom set side. See the module
/// for the upstream `_computeSetOrMapResolution` three-leg order.
#[cfg(test)]
mod ticket_2405_tests;
/// The #2406 pins: the `equal_elements_in_const_set` /
/// `equal_keys_in_const_map` equality face over structural values —
/// records/collections/symbols fold to payload-carrying constants so the
/// duplicate twins compare by value, never by opaque kind (the html
/// package's 103-row record over-report cluster).
#[cfg(test)]
mod ticket_2406_tests;
/// The #2409 pins: the statement-window read discard on the
/// `unused_field`/`unused_local_variable` faces — a bare identifier under a
/// compound-assignment LHS, prefix, or postfix whose parent is the
/// ExpressionStatement joins no read set (`_x += 1;` reports), while `??=`,
/// member targets, binary reads, and later genuine reads keep theirs.
#[cfg(test)]
mod ticket_2409_tests;
/// The #2413 pins: cross-library typedef call resolution (N5+N9 — a foreign
/// file's alias resolves its callable signature at the use site) and the
/// callable-class invocation return type (R4 — `c(3)` types as the `call`
/// method's return and anchors the whole invocation).
#[cfg(test)]
mod ticket_2413_tests;
/// The #2416 pins: the record literal's named labels ride the record
/// type — `return (entryIndex: 1, header: null);` against a declared
/// `({int entryIndex, String? header})` stays silent (the record row's
/// `named` split feeds the assignability face, never a positional-only
/// `(int, String?)` shadow), and `_loading = (progress: progress);`
/// against `({num? progress})` field-initializer faces the named shape.
#[cfg(test)]
mod ticket_2416_tests;
/// The #2419 pins: every depth-0 formal-parameter group opens a fresh
/// pool — sibling top-level `Function({…})` type literals (the
/// `diagnostic.g.dart` const table, typedef RHSes) each clear the
/// accumulated formals at their own begin, so named-formal reuse across
/// independent GFT lists no longer reports `duplicate_definition`, while
/// the within-list check, the executable-vs-annotation split, and the
/// header latch for `<T>` after a GFT return annotation all hold.
#[cfg(test)]
mod ticket_2419_tests;
/// The #2417 pins: the cross-file constructor-formals projection — a
/// foreign container's `ctor_formals` table cloned verbatim carried
/// `ArgListId`/`FnShapeId` slots that index the *foreign* `ArgPool`, so the
/// super-invocation/enum-const argument walks read just-past-end and
/// panicked (the xml/intl corpus zero-diagnostics abort). Every `formal.ty`
/// now re-interns through `boundary.ty(owner, ty)` before the walk derefs
/// it. See the module for the upstream `resolve_container` owner contract.
#[cfg(test)]
mod ticket_2417_tests;
/// The #2422 pins: the explicit-keyword unnamed creation (`const C(...)`,
/// `new C(...)`) stacks its value row so a following member merge resolves
/// against the class, the type-argumented unqualified invocation
/// (`Undefined<X>(...)`) codes the method/function invocation face instead
/// of `undefined_identifier`, and an invalid written type (a head that
/// resolves to no type) asks no assignability question on the assignment,
/// declaration-initializer, and return faces.
#[cfg(test)]
mod ticket_2422_tests;
/// The #2425 pins: the function-involved cross-family assignability face —
/// `super('x')` against a function-typed formal (foreign or same-file)
/// reports `argument_type_not_assignable` the way the oracle's
/// `isAssignableTo` decides it, a function argument against a non-function
/// formal reports, and the 'call' tearoff arm keeps a compatible callable
/// class instance silent while an incompatible `call` signature reports.
#[cfg(test)]
mod ticket_2425_tests;
/// The #2423 pins: the extension-pool family — the specificity step reads
/// the applicable faces' full `on` types argument-wise (a non-nullable or
/// subtype argument dominates its twin, so a single applicable face
/// resolves instead of reading ambiguous), the genuine equal-`on`-type
/// ambiguity stays, and a nested index-assignment target's index argument
/// keeps its local read credit.
#[cfg(test)]
mod ticket_2423_tests;
/// The #2420 pins: the supertype member walk survives the dead supertype
/// hop — `extends Dead implements Live` still merges the live interface's
/// members (the analyzer corpus's `AstNodeImpl extends SyntacticEntity
/// implements AstNode` shape) while a dead edge with no interfaces reports
/// cleanly — and a resolved-but-unindexed supertype hop (`extends
/// Iterable<MissingItem>`) answers through the frozen `dart:core` member
/// tables instead of the under-report.
#[cfg(test)]
mod ticket_2420_tests;
/// The #2429 pins: a prefixed supertype-clause face (`implements idl.X`)
/// binds through the prefix's export scope — the @override gate spares a
/// member the prefixed interface declares, reports one the interface lacks,
/// and a prefixed dead edge behaves like the unprefixed dead edge (#2420's
/// verdicts).
#[cfg(test)]
mod ticket_2429_tests;
/// The #2426 pins: the closure-formal inference's receiver-head gate — the
/// unindexed receiver's first type argument stands in for `map`'s `E` only
/// when the receiver's written head is provably a type — and the staged
/// static type of `this` closing an exhaustive `switch (this)` body.
#[cfg(test)]
mod ticket_2426_tests;
/// The #2430 pins: the parameter-bound materialization at the
/// nullability and never-match gates — a written bound (`T extends
/// AstNodeImpl?`) decides the `?.`/`!`/null-comparison/null-check faces
/// through its nullability, and the never-match verdict reads the bound
/// as the matched parameter's stand-in while an unbounded or
/// parameter-mentioning bound suppresses.
#[cfg(test)]
mod ticket_2430_tests;
/// The #2431 pins: the extension face's type-parameter bounds — the
/// applicability gate drops a face whose bound rejects the receiver's
/// argument (the raw-`List` receiver's `dynamic` instantiation fails every
/// concrete bound), and the specificity step's instantiate-to-bounds tail
/// orders a bounded face against its unbounded twin while equally-bounded
/// twins stay ambiguous.
#[cfg(test)]
mod ticket_2431_tests;
/// The #2432 pins: invocation-level generic type-argument inference — the
/// member-send/top-level upward unification materializes the call's solved
/// bindings into the return face, ambiguous candidates poison instead of
/// first-winning, and the prefixed type head counts its argument group
/// against the prefix scope's element.
#[cfg(test)]
mod ticket_2432_tests;
/// The #2436 pins: the concrete-implementation completeness loop skips the
/// signature comparison when either compared side carries its declaring
/// container's free type parameters (the unsubstituted read — clause type
/// arguments stripped — false-breaks on the free heads, the #2435 audit's
/// 74 `invalid_implementation_override` corpus rows), while a genuine
/// concrete-type mismatch keeps its row.
#[cfg(test)]
mod ticket_2436_tests;
/// The #2439 pins: the constant-pattern never-match verdict is the
/// disjointness gate (`can_be_subtype_of` per single) instead of
/// witness-set equality — an enum constant over an `Enum`-root or
/// `T extends Enum` scrutinee may overlap the matched type, so its case
/// body stays live — and the `Enum` root's covering face decides without
/// enumerating enum declarations as coverable subtypes (the oracle's
/// `non_exhaustive` witness over the root is `Enum()` itself).
#[cfg(test)]
mod ticket_2439_tests;
/// The #2410 pins: the typedef RHS seam — the four RHS shapes that landed
/// opaque at the chase site (an argument-bearing `Named`
/// (`typedef A = List<int>`), a function shape (`typedef F = int
/// Function(String)`), a record shape (`typedef R = (int, String)`), and a
/// parameterized alias (`typedef P<T> = ...`)) now resolve through the
/// snapshot's flattened RHS shapes, and a parameterized use substitutes its
/// written arguments into the captured RHS at the diagnosing fold (the
/// #2413 red-pin posture flips to the oracle's
/// `argument_type_not_assignable`).
#[cfg(test)]
mod ticket_2410_tests;
/// The #2441 pins: one argument verdict — [`Engine::argument_verdict`] —
/// behind every argument face (the two operator folds, the two
/// const/enum-constructor walks, the fn-signature faces, and the staged
/// teardown's standing rejection). The relation gates a projection scar
/// (a source head spelling an import prefix — the parser armed the
/// `ast.AstNode?` continuation's prefix as the `Ty` head, so a verdict
/// over it is fiction), runs the assignability matrix (where the
/// three-hop `implements` chain of a narrowed argument already proves
/// through `implements_interface`), and falls through to
/// [`NameWorld::decide_subtype`] on the matrix's bare-named `None` cut —
/// the same declared-edge lattice the switch tracker walks.
#[cfg(test)]
mod ticket_2441_tests;
/// The #2444 pins: the switch body-death cut narrowed to the
/// nullability-disjoint face — a genuinely value-disjoint constant root
/// (`case true` over an `int`, another enum's constant over an enum, a
/// mixed `null || true` union) reports the
/// `constant_pattern_never_matches_value_type` warning alone and its body
/// stays reachable (the flow's `_equalityCheck` known-false arm is
/// `_typesAreDisjointDueToNullability` only), while the two null faces
/// keep dying (`case null` over a non-nullable scrutinee; a non-null
/// constant over a `Null`-typed scrutinee).
#[cfg(test)]
mod ticket_2444_tests;
/// The #2445 pins: the constant-pattern verdict over an unsolved generic
/// call's residual type-parameter scrutinee — a matched head no declared
/// face resolves (the out-of-scope method parameter's `T`, an undefined
/// name) is unprovable disjointness and stays silent, while a bound
/// visible in scope still substitutes and decides and a declared
/// unrelated scrutinee still reports.
#[cfg(test)]
mod ticket_2445_tests;
/// The #2447 pins: a prefixed annotation arms its full qualified name —
/// the collector's `handle_type` join rewrites the prefix-token head to
/// the dot-joined spelling (`ast.AstNode`), so the projected `Ty` crosses
/// file boundaries truthfully and the #2441 prefix-head gate goes inert
/// for this shape (defense in depth kept).
#[cfg(test)]
mod ticket_2447_tests;
/// The #2449 pins: receiver static types for top-level-getter references
/// and compound receiver chains (census F1) — the getter's declared
/// return types the member and null-aware faces (`g.missing()`,
/// `g?.missing()`, the null-aware verdicts), a creation receiver's field
/// read composes the same way (`Holder().field?.m()`), and a top-level
/// function read keeps the unknown tear-off face.
#[cfg(test)]
mod ticket_2449_tests;
/// The #2442 pins: the analyzer-census member-resolution false-positive
/// family — an extension `this` whose `on` name is the oracle's
/// `InvalidType` (unresolvable through a broken export) stays silent on
/// every member face; `Stream.transform` rides the frozen `dart:async`
/// face; and the operator-merged row runs the same on-face union the
/// plain tail does, naming the `on` type on a decidable miss.
#[cfg(test)]
mod ticket_2442_tests;
/// The #2461 pins: chained member access through implicit-call sends
/// (census family A4) — a value invocation over a `call`-bearing static
/// type types the send with the call method's declared return (the
/// bare-spelling face reads the callee's static type, not the
/// information-less row type), so `body(ctx).effectiveBooleanValue`,
/// `key!(ctx).atomize()`, the two-hop chain, and the `yield*` generator
/// shape resolve through the call's return, and a member missing from
/// that return reports on it.
#[cfg(test)]
mod ticket_2461_tests;
/// The #2458 pins: comment trivia between the last literal element and
/// the closing `}` is no element — a comment-only leaf segment no longer
/// flips a context-less all-entry literal to
/// `ambiguous_set_or_map_literal_both`, while a genuine mix still
/// reports `_both` with its trailing comment intact.
#[cfg(test)]
mod ticket_2458_tests;
/// The #2453 pins: the num-fallback binary-operator typing over an
/// invalid/dynamic operand (census F5) — the resolved-left arithmetic
/// fold refines through the `S <: double?` branch instead of degrading
/// (`int - <invalid>` answers `double` and the downstream assignability
/// faces fire), the non-refined operators keep the declared return, and
/// the compound-assignment face types the augmented read the same way.
#[cfg(test)]
mod ticket_2453_tests;
/// The #2448 pins: the constant-pattern never-match anchor spans the full
/// constant node — a qualified `case A.a:` row anchors `A.a` whole and an
/// enum-constant `case E.a:` row anchors the full pattern node (the
/// upstream `visitConstantPattern` `.at(node)` face), with the literal
/// forms keeping their token anchors.
#[cfg(test)]
mod ticket_2448_tests;
/// The #2451 pins: closure parameters typed through generic iterable chains
/// (`m.values.map((n) => …)`, an in-package getter's `List<E>` face) bind
/// from the chain's element — the nullable element fires the
/// `unchecked_use_of_nullable_value` gate, the non-null/`dynamic` guards
/// stay silent, and the nullable missing-member read collapses to the same
/// unchecked row the oracle emits.
#[cfg(test)]
mod ticket_2451_tests;
/// The #2460 pins: the flow-promotion batch A2 — the two faces already
/// oracle-exact (a `!` narrows the member receiver, a written non-nullable
/// field reads non-null) ride active, and the four divergent faces
/// (generator-yield staging, the `e!` assignment stager, the
/// `staged_write_tys` inference-feed leak, the assert-message
/// negated-condition state) ride `#[ignore]`d with their semantic fix
/// sites named — the flow-side promotion facts are pinned green in
/// `dartq-flow`'s own `ticket_2460_tests`.
#[cfg(test)]
mod ticket_2460_tests;
/// The #2468 pins: the constant-pattern machinery arms for class static
/// consts — `case A.a:` types the constant from the member's captured
/// value and reports the never-match row the enum-only tail
/// classification swallowed, a matching scrutinee stays silent, the enum
/// form keeps its row, and the `const`-prefixed spelling anchors at the
/// keyword the ConstantPattern node starts at.
#[cfg(test)]
mod ticket_2468_tests;
/// The #2450 pins: the invalid-scrutinee switch absorption over the
/// unknown/invalid read faces (statements and expressions, the constant
/// head skip) and the invalid-operand is-check promotion, plus the
/// deleted undefined-tested always-true fold — both oracle arms of an
/// `x is <undefined>` check stay live behind `type_test_with_undefined_name`.
#[cfg(test)]
mod ticket_2450_tests;
/// The #2459 pins: the cascade-resolution leakage family A3 — a cascade
/// section's RHS member chain resolves through its own receiver instead of
/// the latched cascade target (`..sourceSpan = token.span` answers `span`
/// on the token type, never a phantom on the target), an extension getter
/// inside an index-section cascade chain resolves (`..['k'] = s.toJS`
/// finds the `on String` extension), an index-assign `..[i] = v` over a
/// `List` target stays silent, and a missing `[]=` reports per section at
/// the bracket against the TARGET's type.
#[cfg(test)]
mod ticket_2459_tests;
/// The #2465 pins: the unused-element scan's extension-member and
/// part-file shapes — a bare or explicit-`this` self-use inside an
/// extension body and a member access through a universal
/// `extension<T> on T`-style face re-attribute onto the element-attributed
/// application credit (the census's implicit-`this` and on-type-parameter
/// over-reports), the genuinely unused members and sibling-spelling
/// decoys keep reporting, and the part-file use lands in the part's use
/// face the library-wide close merges.
#[cfg(test)]
mod ticket_2465_tests;
/// The #2478 pins: the const constructor's record initializer fields ride
/// the potentially-constant gate — a formal-parameter reference of the
/// enclosing const constructor is potentially constant, so the riverpod
/// `async_value.dart` `const L({num? progress}) : rec = (progress:
/// progress);` shape stays silent (the element verdicts evaluate with the
/// constructor's formals bound as potentially-constant names, the
/// initializing formal included) while a genuinely non-constant field in
/// a true const declaration context keeps its `non_constant_record_field`
/// row, and a genuine discarded `@useResult` top-level invocation keeps
/// its `unused_result` row (the honesty guard for the discarded-value
/// verifier's shadowing fix in its owning lane).
#[cfg(test)]
mod ticket_2478_tests;
/// The #2477 pins: the oracle's second-order derivations over undefined
/// types (census G6+G8) — the undefined-scrutinee switch keeps its
/// fall-through so `body_might_complete_normally` reports, the
/// `FutureOr<Undefined> is Future<Undefined>` shape folds constant-true
/// through the written-argument `Invalid` face, the bare two-argument
/// dart:core `identical(v, null)` routes through the equality fold and
/// deads the impossible branch on a non-nullable operand, and the
/// keep-honest faces (a resolvable enum switch closes, well-typed and
/// `dynamic` operands stay live) hold.
#[cfg(test)]
mod ticket_2477_tests;
/// The #2462 pins: the is-check fold over argument-carrying types (census
/// family A5) — differing written arguments keep the test live with both
/// arms reachable, the provable same-type and covariant folds stay
/// constant-true, the type-variable face stays conservative, and the
/// nullability gate keeps its verdict.
#[cfg(test)]
mod ticket_2462_tests;
/// The #2485 pins: behavior guards over the member-read credit loop's
/// clone removal and the numeric shadow-guard's per-head verdict memo —
/// the unshadowed core-numeric head keeps refining across repeated
/// sends, a shadowing container declaration keeps the declared return,
/// and a many-member-read file's extension-use adjudication is
/// unchanged.
#[cfg(test)]
mod ticket_2485_tests;
/// The #2472 pins: instance-creation typing from the constructor's return
/// (census families G4+G9) — an explicit-keyword creation with written
/// type arguments folds to a receiver typed by the instantiated
/// constructor return (`const ListEquality<Attr>().equals/hash(...)`
/// resolves on `ListEquality<Attr>`, oracle-silent at every census site),
/// a genuine miss on the created type still reports naming the
/// constructor's type, and the initializing formal's named slot takes the
/// field's declared type while the neighboring `absoluteFilePath:` slot
/// keeps enforcing `String`.
#[cfg(test)]
mod ticket_2472_tests;
/// The #2484 pins: the listener-family residual batch (#2478/#2480) — the
/// super/redirect initializer argument lists scan under the const
/// constructor's `CtorScope` (a formal passed through `: super(f)` is
/// potentially constant, the scan is const-constructor-only), the
/// `yield*` verdict stages through the promoted read (the official
/// implied type is the source's flow-aware static type), and a record
/// field inside a const-constructor initializer reports the initializer
/// scan's `invalid_constant`, the element face owning the true const
/// literal context alone.
#[cfg(test)]
mod ticket_2484_tests;
/// The #2489 pins: `unnecessary_type_check` reads the operand's resolved
/// face — `FutureOr<UndefinedNav> rv; rv is Future<UndefinedNav>` folds
/// constant-true through rule 6's legs plus the 0c `InvalidType`
/// vacuity, while the `InvalidType` operand/checked gates keep an
/// undefined head's own spelling silent (`type_test_with_undefined_name`
/// owns that report), and the well-typed/`dynamic` operands keep both
/// branches live.
#[cfg(test)]
mod ticket_2489_tests;

/// The #2497 pins: the extension-applicability instantiated-bound gate —
/// an `on`-clause argument spelling the applied face's own parameter
/// binds the receiver's argument and must satisfy the parameter's
/// declared bound, or the face never applies and the access keeps its
/// `undefined_*` face. An in-scope parameter argument answers through
/// its own bound across the `List`-to-`Iterable` hop and the same head.
#[cfg(test)]
mod ticket_2497_tests;

/// The #2452 pins: invocation/creation through a type alias whose RHS
/// names an unresolved class behind an import prefix — the bare head
/// reports `invocation_of_non_function`, the named-selector head reports
/// `undefined_method` on `Type`, the `const` head reports
/// `creation_with_non_type`, and a prefix-resolvable RHS grounds the
/// chain silent.
#[cfg(test)]
mod ticket_2452_tests;

/// The #2474 pins: bare names inside an `extension` body resolve against
/// the `on` type's frozen member face when the clause walk cannot reach
/// it — `on Stream<S>` whose `Stream` head walks `Ambiguous` across the
/// `dart:core`/`dart:async` synthetic face files suppresses `listen`/
/// `isBroadcast`, while a name the frozen face lacks still reports.
#[cfg(test)]
mod ticket_2474_tests;

/// The #2473 pins: the bare member scope inside `mixin`/`extension`
/// bodies — the invocation face reads the same `walk_members` ∪
/// `on_face_walk` union `resolve_bare` computes, so a generic send
/// through the `on` type instantiates its return and a mixin's
/// `implements` face joins the member scope.
#[cfg(test)]
mod ticket_2473_tests;

/// The #2508 pins: the extension-application row's structural face — the
/// mounted `LibraryTypes` snapshot answers applicability (a generic hop's
/// argument projection), the instantiated parameter-bound check, and the
/// `_isMoreSpecific` steps 3-5 through the real type lattice where the
/// name-level reads only pattern-matched heads.
#[cfg(test)]
mod ticket_2508_tests;

/// The #2564 pins: the nesting-safe creation-head latch — a keyword
/// creation whose argument list contains its own invocation
/// (`const C<T>(inner())`) keeps its own head through the inner group's
/// open/close cycle, folds to the created type instantiated with the
/// written type arguments, and a genuine miss names the constructor's
/// type, never the nested argument's head.
#[cfg(test)]
mod ticket_2564_tests;

/// The #2565 pins: the promotion-latched initializer's re-bind
/// propagating down a `var` chain, the re-bind map's scope-safe keys,
/// and the implicitly typed closure formal's context re-bind.
#[cfg(test)]
mod ticket_2565_tests;

/// The #2566 pins: UNDEFINED_* member-resolution sweep.
#[cfg(test)]
mod ticket_2566_tests;

/// The #2567 pins: assignability matrix sweep.
#[cfg(test)]
mod ticket_2567_tests;

/// The #2568 pins: nullable-usage sweep.
#[cfg(test)]
mod ticket_2568_tests;

/// The #2569 pins: dead-code and switch-exhaustiveness sweep.
#[cfg(test)]
mod ticket_2569_tests;

/// The #2570 pins: usage-cluster and misc tail sweep.
#[cfg(test)]
mod ticket_2570_tests;


/// The operand-face pins: the numeric refinement reads an operand the
/// engine cannot type as a gap that suppresses the fold (an untyped
/// `static const` field, a declared member without a rendered type), not
/// as the oracle's `InvalidType`; a genuine resolution failure keeps the
/// `double` branch, and `int - <dynamic>` is `num` at every sink.
#[cfg(test)]
mod operand_face_tests;

/// The #2521 Object-face pins: an instance-member send on a receiver the
/// classifier cannot name a container for (a function value, a `dynamic`)
/// types `Object`'s declared return, so the write promotes the target and
/// the staged argument verdict stays cleaned; the unpromoted control
/// keeps the one diagnostic.
#[cfg(test)]
mod ticket_2521_tests;
#[cfg(test)]
mod ticket_2536_tests;

/// The shared single-file rig with the resolved-type snapshot mounted
/// (`report_ops`), the shape the per-ticket modules each carried.
#[cfg(test)]
mod test_rig;

/// The `is`-promotion pins over bounded generic hierarchies: a raw tested
/// spelling instantiates to its bounds on both sides of the strict
/// promotion gate, so the promotion rides a `Column<T>` hop or a mixin
/// `on` chain and the promoted argument satisfies the narrower sink.
#[cfg(test)]
mod promotion_bounds_tests;

/// The flow pins behind the `args` corpus rows: `Never`-returning sends
/// end their branch (bare and through a `!` receiver), a `!` on an index
/// expression promotes the assigned local, a `!` on a property chain
/// reads non-null for the next selector.
#[cfg(test)]
mod never_send_tests;

/// The if-null assignment's promotion over a literal right side (the
/// `collection` corpus `(chunk ??= []).add(e); … yield chunk;` shape).
#[cfg(test)]
mod if_null_literal_tests;

/// The switch-expression exhaustiveness over a top-typed scrutinee: a
/// type-free variable pattern covers the remaining space.
#[cfg(test)]
mod exhaustive_dynamic_tests;

/// A cascade section's argument resolves its member chain against its own
/// receiver, never the cascade target.
#[cfg(test)]
mod cascade_argument_tests;

/// The engine faces the indexed SDK sources exposed (#2515).
#[cfg(test)]
mod sdk_exposure_tests;
#[cfg(test)]
mod receiver_face_tests;
#[cfg(test)]
mod flow_scope_tests;
#[cfg(test)]
mod scope_inference_tests;
