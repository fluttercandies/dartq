//! #3339 pins — the subtype-verdict layer behind the
//! `argument_type_not_assignable` shape families.
//!
//! The audit target was `resolved_ops.rs`'s verdict helpers, and the audit
//! found the family's verdict is decided elsewhere:
//! `assignability::Engine::assignable` / `slot_subtype` and
//! `engine_core::Engine::resolved_subtype` / `strict_subtype` /
//! `same_head_args`. What this file's adapter owns is the flow-facing half
//! (`TypeOps::is_subtype`, `try_promote_to`, `classify`), which feeds every
//! join and promotion an argument position later reads. Both halves are
//! pinned here against the dominant expected type-pair shapes of
//! `conformance/expected.tsv` (819 rows over 377 cases as of 2026-11-01:
//! `String -> int` 105, `Object? -> Object` 84, `int -> String` 65,
//! `String -> num` 33, `double -> int` 23, `int? -> int` 23, `A -> B` 19,
//! `A? -> A` 16, function-shape pairs 15+).
//!
//! Upstream references: `subtype.dart` (the relation the adapter delegates
//! to `dartq-types`' `TypeSystem::is_subtype`), `type_system.dart:1729-1760`
//! (`tryPromoteToType`'s strict gate) and `flow_analysis_visitor.dart:615-632`
//! (`classifyType`).
//!
//! Three divergences came out of the audit, all routed (none is reachable
//! from this ticket's write scope):
//! 1. an `Object?`-sourced argument yields no verdict at all, while the
//!    assignment face reports the same pair through the same engine — the
//!    argument window / `arg_expression_ty` read (`assignability.rs`,
//!    `invocation.rs`; live lanes #3312/#3329);
//! 2. an unbounded type-parameter source abstains at every position
//!    (`int x = t;` is silent too) where upstream's `isAssignableTo` answers
//!    false (`assignability.rs`, `engine_core.rs`; live lanes #3312/#3316);
//! 3. the declared-bound rule (`T extends num` ⇒ `T <: num`) was disarmed
//!    on this file's `TypeSystem` because `LibraryTypes::facts` carried
//!    only the `dynamic` default — fixed by #3358 on the container face
//!    (the capture's bound trees now reach `facts`, and the adapter arms
//!    `with_param_facts`); a function-scope `<T extends num>` stays
//!    dormant (the element model discards those bound trees).
//!
//! Registered through `diagnose/tests.rs`'s `#[path]` block (`diagnose.rs`
//! is a live-lane file out of this ticket's write scope).

use std::path::Path;

use dartq_flow::ty::{Base, Classification, Ty as FlowTy, TypeOps};
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_parser::ParserFeatures;
use dartq_resolution::{LibraryGraphBuilder, Namespaces};
use dartq_scanner::ScanConfig;
use dartq_source::{FileTable, StringTable};

use crate::FileReport;
use crate::collector::collect;
use crate::diagnose::test_rig::report_ops;
use crate::library::ElementIndex;
use crate::model::MicroKind;
use crate::resolved_ops::{LibraryTypes, ResolvedTypeOps};

/// The messages of one code, in report order.
fn rows<'a>(report: &'a FileReport, code: &str) -> Vec<&'a str> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == code)
        .map(|d| d.message.as_str())
        .collect()
}

/// The adapter over one unit's own declarations, `names` observed the way a
/// replay observes them — the resolved face every `is_subtype` /
/// `try_promote_to` / `classify` ask below answers through the same
/// snapshot the engine mounts.
fn ops_for(source: &str, names: &[&str]) -> ResolvedTypeOps<'static> {
    ops_for_impl(source, names, false)
}

/// [`ops_for`] over the production-shaped clause capture
/// (`capture_clause_trees`, exactly what the engine's fold feeds
/// [`LibraryTypes::build_with_clauses`]): the written supertype and
/// parameter-bound trees the element model strips to names.
fn ops_for_captured(source: &str, names: &[&str]) -> ResolvedTypeOps<'static> {
    ops_for_impl(source, names, true)
}

fn ops_for_impl(source: &str, names: &[&str], captured: bool) -> ResolvedTypeOps<'static> {
    // The adapter borrows the string table and the type snapshot for a
    // lifetime the pin owns for its whole body.
    let strings: &'static StringTable = Box::leak(Box::new(StringTable::new()));
    let mut files = FileTable::new();
    let file = files.intern(Path::new("main.dart"));
    let scanned = dartq_scanner::scan(
        source,
        ScanConfig {
            enable_triple_shift: true,
            enable_augmentations: true,
        },
        file,
    );
    assert!(scanned.diagnostics.is_empty(), "{source:?}");
    let features = ParserFeatures::from_feature_set(FeatureSet::for_language_version(
        LanguageVersion::new(3, 15),
    ));
    let elements = collect(source, &scanned.tokens, features, strings);
    let mut builder = LibraryGraphBuilder::new(strings);
    let lib = builder.intern("package:t/main.dart");
    builder.unit(lib, file);
    let mut index = ElementIndex::default();
    index.insert(file, elements.clone());
    for (name, decl) in elements.declarations() {
        if decl.kind == MicroKind::TypeParam {
            continue;
        }
        builder.declare(lib, name, file);
    }
    let graph = builder.finish();
    let namespaces = Namespaces::build(&graph);
    let mut clauses = crate::resolved_ops::ClauseTrees::empty();
    if captured {
        crate::capture_clause_trees(
            file,
            source,
            &scanned.tokens,
            features,
            strings,
            &mut clauses,
        );
    }
    let types: &'static LibraryTypes = Box::leak(Box::new(LibraryTypes::build_with_clauses(
        &graph,
        &namespaces,
        &index,
        strings,
        lib,
        &clauses,
    )));
    let ops = ResolvedTypeOps::new(types, strings, file);
    let table: Vec<Box<str>> = names.iter().map(|n| (*n).into()).collect();
    ops.observe_names(&table);
    ops
}

/// One name's `FlowTy` at its observed id.
fn named(names: &[&str], name: &str, nullable: bool) -> FlowTy {
    let id = names
        .iter()
        .position(|n| *n == name)
        .unwrap_or_else(|| panic!("the observed table carries `{name}`")) as u32;
    FlowTy {
        base: Base::Name(id),
        nullable,
    }
}

/// The observed-name table the adapter asks are keyed on: the builtin
/// spellings (registered by the snapshot's curated `dart:core` face) plus
/// the two declarations of the fixture unit.
const LATTICE_NAMES: &[&str] = &[
    "String",   // 0
    "num",      // 1
    "int",      // 2
    "Object",   // 3
    "Base",     // 4
    "Derived",  // 5
    "List",     // 6
    "Iterable", // 7
];

/// The primitive / nullability / top-and-bottom lattice (#3339): every
/// answer below is the upstream relation over the curated `dart:core` face
/// (`int`/`double` subtype `num`, `List<E>` implements `Iterable<E>`) plus
/// the declaration edge `Derived extends Base`.
#[test]
fn resolved_verdict_lattice_matches_upstream_subtype() {
    let ops = ops_for(
        "class Base {}\nclass Derived extends Base {}\n",
        LATTICE_NAMES,
    );
    let string_ty = named(LATTICE_NAMES, "String", false);
    let num_ty = named(LATTICE_NAMES, "num", false);
    let int_ty = named(LATTICE_NAMES, "int", false);
    let int_question = FlowTy {
        base: int_ty.base,
        nullable: true,
    };
    let object_none = named(LATTICE_NAMES, "Object", false);
    let object_question = named(LATTICE_NAMES, "Object", true);
    let base_ty = named(LATTICE_NAMES, "Base", false);
    let derived_ty = named(LATTICE_NAMES, "Derived", false);
    let list_ty = named(LATTICE_NAMES, "List", false);
    let iterable_ty = named(LATTICE_NAMES, "Iterable", false);
    let dynamic_ty = FlowTy {
        base: Base::Dynamic,
        nullable: false,
    };
    let null_ty = FlowTy {
        base: Base::Null,
        nullable: false,
    };
    let never_ty = FlowTy {
        base: Base::Never,
        nullable: false,
    };

    // Primitive lattice: the `String -> int` / `int -> String` rejects and
    // the `int -> num` accept the corpus expects in argument position.
    assert!(
        !ops.is_subtype(string_ty, int_ty),
        "`String` is not an `int`"
    );
    assert!(
        !ops.is_subtype(int_ty, string_ty),
        "`int` is not a `String`"
    );
    assert!(ops.is_subtype(int_ty, num_ty), "`int <: num`");
    assert!(!ops.is_subtype(num_ty, int_ty), "`num` is not an `int`");
    assert!(
        !ops.is_subtype(string_ty, num_ty),
        "`String` is not a `num`"
    );

    // Nullability: `int -> int?` accepts, `int? -> int` rejects, and `Null`
    // rides only the nullable faces.
    assert!(ops.is_subtype(int_ty, int_question), "`int <: int?`");
    assert!(!ops.is_subtype(int_question, int_ty), "`int?` is not `int`");
    assert!(ops.is_subtype(null_ty, int_question), "`Null <: int?`");
    assert!(!ops.is_subtype(null_ty, int_ty), "`Null` is not `int`");

    // `Object? -> Object` — the 84-row expected pair: the nullable source
    // never reaches the non-nullable root, the reverse always does.
    assert!(
        !ops.is_subtype(object_question, object_none),
        "`Object?` is not `Object`"
    );
    assert!(ops.is_subtype(object_none, object_question));

    // Top and bottom (subtype.dart's Left Bottom / Left Top arms): `Never`
    // subtypes everything, `dynamic` only the tops.
    assert!(ops.is_subtype(never_ty, int_ty), "`Never <: int`");
    assert!(ops.is_subtype(int_ty, dynamic_ty), "`int <: dynamic`");
    assert!(
        !ops.is_subtype(dynamic_ty, int_ty),
        "`dynamic` is not `int`"
    );
    assert!(ops.is_subtype(dynamic_ty, object_question));

    // Declaration hierarchy (the `A -> B` / `A? -> A` pairs): the resolved
    // face answers the real edge, which the name level cannot.
    assert!(ops.is_subtype(derived_ty, base_ty), "`Derived <: Base`");
    assert!(
        !ops.is_subtype(base_ty, derived_ty),
        "`Base` is not `Derived`"
    );
    assert!(
        !ops.is_subtype(
            FlowTy {
                base: derived_ty.base,
                nullable: true
            },
            base_ty
        ),
        "`Derived?` is not `Base`"
    );

    // Generic heads: a bare spelling instantiates to bounds, so the declared
    // edge decides (`List<dynamic> <: Iterable<dynamic>`) and its reverse
    // never does.
    assert!(ops.is_subtype(list_ty, iterable_ty), "`List <: Iterable`");
    assert!(
        !ops.is_subtype(iterable_ty, list_ty),
        "`Iterable` is not `List`"
    );

    // `tryPromoteToType`'s strict gate (type_system.dart:1729-1760): a
    // covered subtype is a step, a mutual subtype is not.
    assert_eq!(ops.try_promote_to(int_ty, int_question), Some(int_ty));
    assert_eq!(ops.try_promote_to(int_question, int_ty), None);
    assert_eq!(ops.try_promote_to(int_ty, int_ty), None);

    // `classifyType`'s nullability split over the plain faces
    // (flow_analysis_visitor.dart:615-632).
    assert_eq!(ops.classify(int_ty), Classification::NonNullable);
    assert_eq!(
        ops.classify(int_question),
        Classification::PotentiallyNullable
    );
    assert_eq!(ops.classify(null_ty), Classification::NullOrEquivalent);
    assert_eq!(
        ops.classify(dynamic_ty),
        Classification::PotentiallyNullable
    );
}

/// The one in-file verdict divergence the audit found (routed, fixed by
/// #3358 on the container face, still dormant on this fixture): the
/// adapter's [`dartq_types::TypeSystem`] now arms the left-type-variable
/// rule (`subtype.dart:490-500`) with the declared bounds the snapshot's
/// facts carry — see
/// [`container_declared_bound_answers_through_the_spelling_face`].
///
/// This fixture stays dormant because its parameter is a **function-scope**
/// group (`num id<T extends num>(T t)`), and the element model keeps no
/// bound column for one: the collector discards a top-level function's
/// bound buffer at the group's close, so `TopDecl` carries the spellings
/// only. The parse-time window capture in `crate::bounds` is the only
/// surviving source of those trees and has no transport into
/// [`LibraryTypes`] — building one is a model/capture change outside this
/// ticket's scope, so the pin records the gap instead of the wiring
/// borrowing a bound the function never declared.
#[ignore = "dormant: a function-scope `<T extends num>` has no bound tree in the element model (TopDecl keeps spellings only); the container face is armed"]
#[test]
fn type_parameter_left_answers_through_its_declared_bound() {
    let names = &["num", "T"];
    let ops = ops_for("num id<T extends num>(T t) => t;\n", names);
    let t_ty = named(names, "T", false);
    let num_ty = named(names, "num", false);
    assert!(
        ops.is_subtype(t_ty, num_ty),
        "`T extends num` makes `T <: num` (subtype.dart:490-500)"
    );
    assert!(!ops.is_subtype(num_ty, t_ty), "`num` is not a `T`");
}

/// The #3358 pin: a *container's* declared bound reaches the name-keyed
/// parameter id the adapter answers a body's `T` with (the face
/// `TypeOps::is_subtype`, `try_promote_to` and every flow join read), so
/// the left-type-variable rule is live over the production-shaped clause
/// capture. The bound narrows without widening: `T extends num` gives
/// `T <: num`, never `T <: String`, and `num <: T` holds for no `T`.
#[test]
fn container_declared_bound_answers_through_the_spelling_face() {
    let names = &["num", "String", "T", "Box"];
    let ops = ops_for_captured(
        "class Box<T extends num> {\n  T widen(T other) => other;\n}\n",
        names,
    );
    let t_ty = named(names, "T", false);
    let num_ty = named(names, "num", false);
    let string_ty = named(names, "String", false);
    assert!(
        ops.is_subtype(t_ty, num_ty),
        "`class Box<T extends num>` makes `T <: num` (subtype.dart:490-500)"
    );
    assert!(
        !ops.is_subtype(t_ty, string_ty),
        "the declared bound narrows: `num <: String` fails"
    );
    assert!(!ops.is_subtype(num_ty, t_ty), "`num` is not a `T`");

    // The unbounded twin keeps the legacy face: an undecided bound never
    // arms the rule (`T` reads as the unbounded `Object?`).
    let unbound = ops_for_captured("class Holder<T> {\n  T id(T t) => t;\n}\n", names);
    assert!(
        !unbound.is_subtype(t_ty, num_ty),
        "an unbounded `T` carries no `num` bound"
    );
}

/// The routing evidence for the two argument-position divergences below:
/// the *assignment* face reads the same pair through the same verdict and
/// reports, so `Object? -> Object` is decided correctly and the argument
/// face is what drops it. Green by construction — it goes red only if the
/// verdict itself regresses.
#[test]
fn assignment_face_decides_the_pair_the_argument_face_drops() {
    let fixture = "\
void m(Object? v) {
  Object x = v;
}
";
    let report = report_ops(fixture);
    let messages = rows(&report, "invalid_assignment");
    assert_eq!(
        messages.len(),
        1,
        "the assignment face reports `Object? -> Object`; got: {:?}",
        report.diagnostics
    );
    assert!(
        messages[0].contains("'Object?'"),
        "the message names the nullable source; got: {}",
        messages[0]
    );
}

/// The dominant reject shape end to end: a `String`-typed parameter given an
/// `int` argument reports, while the same argument into a `num` parameter
/// stays silent (`int <: num`) — the `String -> int` / `int -> num` split of
/// the expected corpus.
#[test]
fn primitive_argument_shape_rejects_and_accepts() {
    let fixture = "\
void takeString(String s) {}
void takeNum(num n) {}

void m() {
  takeNum(1);
  takeString(1);
}
";
    let report = report_ops(fixture);
    assert_eq!(
        rows(&report, "argument_type_not_assignable").len(),
        1,
        "one rejection over the `String` parameter; got: {:?}",
        report.diagnostics
    );
}

/// Nullability end to end: `null` into a non-nullable parameter reports,
/// `null` into its nullable form stays silent (`Null <: int?`).
#[test]
fn nullable_argument_shape_rejects_and_accepts() {
    let fixture = "\
void takeInt(int i) {}
void takeIntQuestion(int? i) {}

void m() {
  takeIntQuestion(null);
  takeInt(null);
}
";
    let report = report_ops(fixture);
    assert_eq!(
        rows(&report, "argument_type_not_assignable").len(),
        1,
        "one rejection over the non-nullable parameter; got: {:?}",
        report.diagnostics
    );
}

/// The `Object? -> Object` pair end to end: an `Object?` source into an
/// `Object` parameter reports, the `Object`-family source into the nullable
/// parameter stays silent.
///
/// Ignored on an observed divergence (2026-11-01): the argument position
/// yields nothing for `takeObject(value)` while the assignment position
/// reports the same pair (`assignment_face_decides_the_pair_the_argument_face_drops`
/// is green), so the verdict is right and the argument face's read of a
/// `Object?` source is what drops it — `assignability::Engine::
/// arg_expression_ty` / `invocation`'s argument window, live lanes
/// (#3312/#3329). Ninety-odd expected corpus rows carry this pair.
#[ignore = "dormant: the `Object?`-sourced argument face yields no verdict; the assignment face reports the same pair (routed to the assignability/invocation lanes)"]
#[test]
fn nullable_source_into_non_nullable_target_rejects() {
    let fixture = "\
void takeObject(Object o) {}
void takeObjectQuestion(Object? o) {}

void m(Object? value) {
  takeObjectQuestion('s');
  takeObject(value);
}
";
    let report = report_ops(fixture);
    assert_eq!(
        rows(&report, "argument_type_not_assignable").len(),
        1,
        "one rejection over the `Object` parameter; got: {:?}",
        report.diagnostics
    );
}

/// The `A -> B` pair end to end: a `Derived` argument into a `Base`
/// parameter stays silent (the hierarchy the name level cannot see), the
/// reverse direction reports.
#[test]
fn hierarchy_argument_shape_rejects_and_accepts() {
    let fixture = "\
class Base {}
class Derived extends Base {}

void takeBase(Base v) {}
void takeDerived(Derived v) {}

void m(Derived d, Base b) {
  takeBase(d);
  takeDerived(b);
}
";
    let report = report_ops(fixture);
    assert_eq!(
        rows(&report, "argument_type_not_assignable").len(),
        1,
        "one rejection over the `Derived` parameter; got: {:?}",
        report.diagnostics
    );
}

/// The unbounded type-parameter direction end to end: `T` has no relation to
/// `int` (its implicit bound is `Object?`), so the argument reports — the
/// reject half of the type-parameter cluster. The accept half
/// (`T extends num` into a `num` parameter) is the routed divergence
/// [`type_parameter_left_answers_through_its_declared_bound`] pins.
///
/// Ignored on a second observed divergence (2026-11-01): both the argument
/// AND the assignment position (`int x = t;`) stay silent for an unbounded
/// `T`, so the verdict itself abstains (`None`) where upstream's
/// `isAssignableTo` answers false — the type-parameter abstention in
/// `assignability`/`engine_core` (live lanes #3312/#3316), upstream
/// `subtype.dart:490-500` + `type_system.dart:860-891`.
// un-ignored 2026-11-07: passing per stale-ignore triage (ticket #3973)
#[test]
fn unbounded_type_parameter_argument_rejects() {
    let fixture = "\
void takeInt(int v) {}

void m<T>(T t) {
  takeInt(t);
}
";
    let report = report_ops(fixture);
    assert_eq!(
        rows(&report, "argument_type_not_assignable").len(),
        1,
        "one rejection over the unbounded `T`; got: {:?}",
        report.diagnostics
    );
}
