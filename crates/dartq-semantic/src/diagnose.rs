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
    CodeInfo, ContextMessage, DARTQ_INTERNAL_ERROR, Diagnostic, Severity, SharedStr,
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
    ElementIndex, ScopedHit, implements_interface, lookup_scoped, owner_library,
    prefixed_container, resolve_container, supertype_hop_dead,
};
use crate::member_face::{
    COLLECTION_INTERFACE_ALIASES, CORE_CONST_CTOR, CORE_INTERFACE_MEMBERS, CORE_STATIC_RETURNS,
    CORE_TYPE_MEMBERS, EqualsCheck, MemberFace, NsmCheck, NsmShape,
};
use crate::model::{
    ArgListId, ArgPool, ArityViolation, Container, ContainerKind, ExtensionFace, FileElements,
    Formal, Formals, MemberEntry, MemberKind, MicroKind, TopDecl, Ty,
};
use crate::switch_tracker::{
    NameCase, NameScope, NameTy, NameWorld, ObjectField, PatternFrame, Restriction, SwitchTracker, case_space,
    is_always_exhaustive, restriction_is_constant,
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
mod engine_core;
mod invocation;
mod listener;
mod overrides;
mod resolution;
mod row_resolution;
mod scopes_bodies;
mod type_capture;
mod types;
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

/// The const NON_CONSTANT-family differential pins (#2242): unresolved
/// callees inside const literals report the element faces at the
/// invocation and anchor the declaration row at the first failure, and a
/// qualified const creation (`C.named(...)`) validates its arguments
/// against the in-file const constructor. Kept as a standalone module so
/// the shared `tests` rig stays untouched.
#[cfg(test)]
mod ticket_const_family_tests;

pub use types::{FileReport, NameRow, NameUse, RefTarget, UndefinedGuard, UnitRef};
pub(crate) use types::*;


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
            ret: f
                .ret
                .as_ref()
                .map(|r| Box::new(subst_tree(r, map))),
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
pub(crate) fn merge_source_ordered(base: Vec<Diagnostic>, extra: Vec<Diagnostic>) -> Vec<Diagnostic> {
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
        ops.map(|ops| ops as &dyn dartq_flow::TypeOps),
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
/// is the library's [`dartq_flow::TypeOps`] back-end (the design-2204
/// `ResolvedTypeOps` over a per-library `LibraryTypes` snapshot); `None`
/// keeps the name-level [`dartq_flow::ty::NameTypeOps`] default the flow
/// layer falls back to. The ops object's lifetime must cover the parse —
/// the fold owns it only by reference.
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
    ops: Option<&'t dyn dartq_flow::TypeOps>,
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
        ),
        flow: {
            let mut flow = Flow::new(tokens, source);
            // The resolved-type face (#2204 phase A): when the library's
            // `LibraryTypes` snapshot mounted an adapter, every type
            // question the replay asks answers through it; the `None`
            // call sites keep the name-level default.
            if let Some(ops) = ops {
                flow.set_ops(ops);
            }
            flow
        },
        pending_exit: None,
        file,
        parse_diagnostics: Vec::new(),
    };
    // The parse borrows the listener; `drop(parser)` ends the borrow and
    // the destructure below takes the engine and flow recorder back.
    let mut parser = Parser::new(source, tokens, features, &mut listener);
    parser.parse_unit();
    drop(parser);
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
                    let dead_null_aware =
                        SemanticCode::DeadNullAwareExpression.code_info().name;
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
                unassigned.stage(span.range);
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
    engine.resolve_rows();
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
    // whole-file const-ctor set): the first invalid entry per initializer
    // anchors `const_initialized_with_non_constant_value` (the oracle's
    // one-row-per-initializer verdict at the deepest failing node), and
    // every invalid creation whose constructor exists but is non-const
    // adds the `const_with_non_const` twin at the creation.
    {
        let mut reported: FxHashSet<StringId> = FxHashSet::default();
        for (init, span, kind) in std::mem::take(&mut engine.pending_const_creations) {
            let invalid = match kind {
                ConstCreationEntry::ArgFailure => true,
                ConstCreationEntry::Creation { container } => {
                    !engine.const_creation_valid(container, span)
                }
            };
            if !invalid {
                continue;
            }
            if reported.insert(init) {
                engine.stage_usage(
                    SemanticCode::ConstInitializedWithNonConstantValue,
                    span,
                    "",
                );
            }
            if let ConstCreationEntry::Creation { container } = kind
                && !engine.const_ctor_target(container)
            {
                engine.stage_usage(SemanticCode::ConstWithNonConst, span, "");
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
            engine.stage_typed(
                SemanticCode::ConstantPatternNeverMatchesValueType,
                cand.at,
                &a0,
                &a1,
            );
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
    apply_ignore_suppression(&mut diagnostics, source);
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
/// warnings/hints/lints are ignorable). The comment recognition covers
/// standalone comment lines only — a line whose first non-whitespace
/// characters are `//` (trailing comments stay a dartq-lints face); the
/// under-report direction.
pub(crate) struct IgnoreSuppression {
    /// 1-based line -> lowercased names (`ignore:` entries).
    pub(crate) on_line: FxHashMap<u32, Box<[String]>>,
    /// Lowercased `ignore_for_file:` names.
    pub(crate) for_file: Box<[String]>,
}

impl IgnoreSuppression {
    /// Parses the suppression table from `source`'s standalone comment
    /// lines. The element walk follows the official grammar's skeleton
    /// (`ignoredElements`, ignore_info.dart:292-472): a comma-separated
    /// list of diagnostic names, terminated by the first character that
    /// cannot continue one. A standalone ignore comment suppresses its
    /// immediate next line — a later code/plain comment line consumes the
    /// pending entry at its own (diagnostic-free) line, never beyond.
    pub(crate) fn parse(source: &str) -> Self {
        let mut on_line: FxHashMap<u32, Box<[String]>> = FxHashMap::default();
        let mut for_file: Vec<String> = Vec::new();
        let mut line_no: u32 = 0;
        let mut pending: Option<Vec<String>> = None;
        for line in source.lines() {
            line_no += 1;
            let trimmed = line.trim_start();
            // `ignore:` tolerates extra leading slashes (`/// ignore:`);
            // `ignore_for_file:` requires exactly two (ignore_info.dart
            // :217-289 — the extra-slash loop is commented out upstream for
            // the for-file form).
            let entry = trimmed.strip_prefix("//").map(|rest| {
                let bare = rest.trim_start_matches('/').trim_start();
                // The for-file form requires exactly two slashes; the
                // line form tolerates extra ones.
                if bare.starts_with("ignore_for_file:") && !trimmed.starts_with("///") {
                    Some((bare.strip_prefix("ignore_for_file:").unwrap_or(""), true))
                } else {
                    bare.strip_prefix("ignore:").map(|rest| (rest, false))
                }
            });
            match entry {
                Some(Some((rest, for_file_form))) => {
                    let names: Vec<String> = rest
                        .split(',')
                        .filter_map(|name| {
                            let name = name.trim();
                            // A name is letters/digits/underscore; the
                            // first character outside that set terminates
                            // the list (trailing prose is never a name).
                            let end = name
                                .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                                .unwrap_or(name.len());
                            (end > 0).then(|| name[..end].to_ascii_lowercase())
                        })
                        .collect();
                    if for_file_form {
                        for_file.extend(names);
                    } else {
                        pending = Some(names);
                    }
                }
                _ => {
                    if let Some(pending) = pending.take() {
                        on_line.insert(line_no, pending.into_boxed_slice());
                    }
                }
            }
        }
        Self {
            on_line,
            for_file: for_file.into_boxed_slice(),
        }
    }

    /// Whether `code` (the diagnostic's lowercased code name) is suppressed
    /// at `offset` (UTF-16). A citation matches on the diagnostic's `name`
    /// OR on its `uniqueName` (`IgnoredDiagnosticName._matches`,
    /// `ignore_info.dart`:39-52): the entries are the comment's lowercased
    /// spellings, so the second leg canonicalizes each spelling through
    /// [`canonical_diagnostic_name`] — the name a `uniqueName` citation
    /// resolves to — before comparing against `code` (a uniqueName
    /// citation suppresses every diagnostic reported under the shared
    /// name; the name-level join is the fold's resolution, the variant
    /// split lives in the upstream constant, not in our `CodeInfo`).
    pub(crate) fn suppresses(&self, offset: u32, line_of: impl Fn(u32) -> u32, code: &str) -> bool {
        if self
            .for_file
            .iter()
            .any(|name| citation_matches(name, code))
        {
            return true;
        }
        self.on_line
            .get(&line_of(offset))
            .is_some_and(|names| names.iter().any(|name| citation_matches(name, code)))
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
/// rows whose code a suppression entry names drop (`_filterIgnoredDiagnostics`
/// — errors stay, warnings/hints/lints suppress). A table-less file pays one
/// scan and exits; otherwise a single source walk resolves every row's line
/// (the run is source-ordered, so the cursor only advances).
pub(crate) fn apply_ignore_suppression(diagnostics: &mut Vec<Diagnostic>, source: &str) {
    let face = IgnoreSuppression::parse(source);
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
        d.severity == Severity::Error || !face.suppresses(d.range.offset, line_of, d.code.name)
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

#[cfg(test)]
mod suppression_tests {
    //! The `IgnoreSuppression` join pins (#2137): the citation match is the
    //! upstream `IgnoredDiagnosticName._matches` two-leg verdict
    //! (`ignore_info.dart`:39-52) — a comment name suppresses a diagnostic
    //! whose `name` OR `uniqueName` it equals.

    use super::*;

    /// One parsed face, one `line_of` table: the `// ignore:` on line 1
    /// targets line 2, the `ignore_for_file:` covers every line. Offsets
    /// are UTF-16 — the corpus is ASCII, so byte offsets serve.
    fn parse_face(source: &str) -> (IgnoreSuppression, impl Fn(u32) -> u32 + '_) {
        let face = IgnoreSuppression::parse(source);
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
        assert!(face.suppresses(59, &line_of, "invalid_null_aware_operator"));
        // The same citation never reaches an unrelated code on that line.
        assert!(!face.suppresses(59, &line_of, "dead_code"));
    }

    /// `ignore_for_file:` joins the same canonical leg file-wide.
    #[test]
    fn unique_name_for_file_citation_suppresses_every_line() {
        let (face, line_of) =
            parse_face("// ignore_for_file: undefined_super_getter\nclass C {}\nvar x = 0;\n");
        // `undefined_super_getter` is the getter variant's uniqueName; the
        // row reports under `undefined_super_member`.
        assert!(face.suppresses(43, &line_of, "undefined_super_member"));
        assert!(face.suppresses(54, &line_of, "undefined_super_member"));
    }

    /// The name leg is unchanged: a `name` spelling still suppresses, a
    /// spelling that is neither `name` nor a registry `uniqueName`
    /// resolving to it never does (the under-report direction — no
    /// widening past the oracle's match).
    #[test]
    fn name_citation_still_suppresses_and_unknown_spellings_never_do() {
        let (face, line_of) = parse_face("// ignore: dead_code\nvar x = 0;\n");
        assert!(face.suppresses(21, &line_of, "dead_code"));
        assert!(face.suppresses(21, &line_of, "DEAD_CODE"));
        assert!(!face.suppresses(21, &line_of, "unused_local_variable"));
        // A uniqueName of a DIFFERENT code family never crosses.
        let (face, line_of) = parse_face("// ignore: undefined_super_setter\nvar x = 0;\n");
        assert!(face.suppresses(34, &line_of, "undefined_super_member"));
        assert!(!face.suppresses(34, &line_of, "undefined_method"));
        // Registry-silent spellings resolve to nothing.
        assert_eq!(canonical_diagnostic_name("not_a_real_code"), None);
        assert_eq!(canonical_diagnostic_name("dead_code"), None);
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
}

#[cfg(test)]
mod ticket_2231_tests;
