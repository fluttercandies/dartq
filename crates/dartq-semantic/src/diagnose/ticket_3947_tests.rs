//! Ticket #3947 — a written core-type name must resolve exactly like any
//! other written type position, and a declaration-scoped type-parameter tag
//! must never shadow the library namespace for the rest of the file.
//!
//! Two faces live here.
//!
//! 1. **Closure formals.** `var f = (int x) { return x; };` and its
//!    siblings are silent on the oracle. They are silent on the current
//!    tree too — the exact shape the ticket was filed against healed while
//!    the round was in flight — so these pins are regression guards, not
//!    fix witnesses. They ride [`report_ops_core_ctor`], whose `dart:core`
//!    stub is the CLI's real posture: the core heads resolve as names-only
//!    declarations in ANOTHER library. [`report_ops`] declares them in this
//!    unit's own prelude and therefore cannot see this face at all. Each
//!    pin asserts the absence of the two type-resolution codes rather than
//!    total silence, because the rig's prelude-free posture legitimately
//!    adds its own unrelated rows (`unused_local_variable` on an unread
//!    local, and so on).
//!
//! 2. **The type-parameter tag.** `Engine::type_resolution` answered
//!    `Missing` for any file-scoped `TypeParam` hit. The collector registers
//!    every type variable unit-wide only so `duplicate_definition` can
//!    compare per list (`collector.rs`, the `decls.or_insert_with(...,
//!    TopDecl::bare(MicroKind::TypeParam))` arm) — it is a
//!    DECLARATION-scoped placeholder, never a name in scope outside its own
//!    declaration. Because `lookup_scoped` returns the file's own
//!    declaration before the namespace walk, a generic declaration's
//!    variable spelled `int` (or any core name) poisoned every EARLIER use
//!    of that name in the file: `void f(int x) {} class C<int> {}` reported
//!    `undefined_class` on the parameter's `int`, and
//!    `<V>(int x, V v) => v` followed by `<int>(int x) => x` reported it on
//!    the second literal's type-argument group. The fix lets the
//!    placeholder fall through to the namespace; a genuinely undeclared
//!    type variable (`class C<T> {}` then `T x;`) still reports, because
//!    the namespace misses there.
//!
//! Oracle provenance: `dart analyze --format=machine` (3.13.7) over each
//! fixture below; every silence pin carries zero rows, and the
//! `undefined_class` pin carries exactly the row quoted.

use super::test_rig::{report_ops, report_ops_core_ctor, rows};

/// The two codes a written type position reports when its head fails to
/// resolve as a type. Both must be absent from every silence pin here.
fn type_resolution_rows(report: &super::FileReport) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .filter(|d| matches!(d.code.name, "undefined_class" | "non_type_as_type_argument"))
        .map(|d| format!("{} @{}: {}", d.code.name, d.range.offset, d.message))
        .collect()
}

#[track_caller]
fn assert_type_positions_resolve(report: &super::FileReport) {
    let found = type_resolution_rows(report);
    assert!(
        found.is_empty(),
        "type positions must resolve, got {found:#?}"
    );
}

// —— the closure-formal face (regression guards) ——————————————————————

/// The ticket's original shape: a closure formal written with a core type
/// name.
#[test]
fn closure_formal_written_core_type_stays_silent() {
    let report = report_ops_core_ctor("void main() {\n  var f = (int x) { return x; };\n}\n");
    assert_type_positions_resolve(&report);
}

/// A generic core type in the formal slot: both the `List` head and its
/// `int` argument are written type positions, so neither may report
/// (`undefined_class` or `non_type_as_type_argument`).
#[test]
fn closure_formal_written_generic_core_type_stays_silent() {
    let report = report_ops_core_ctor("void main() {\n  var f = (List<int> l) { return l; };\n}\n");
    assert_type_positions_resolve(&report);
}

/// Two formals, two core spellings — the positional pair resolves
/// independently.
#[test]
fn closure_formal_two_written_core_types_stay_silent() {
    let report =
        report_ops_core_ctor("void main() {\n  var f = (String s, double d) { return s; };\n}\n");
    assert_type_positions_resolve(&report);
}

/// Control: a fixture-declared class in the same formal slot — the
/// regression was the core-name arm, never the formal-list walk.
#[test]
fn closure_formal_fixture_class_stays_silent() {
    let report =
        report_ops_core_ctor("class K {}\nvoid main() {\n  var f = (K k) { return k; };\n}\n");
    assert_type_positions_resolve(&report);
}

/// The nullability-marked spelling rides the same slot (`int?`).
#[test]
fn closure_formal_nullable_core_type_stays_silent() {
    let report = report_ops_core_ctor("void main() {\n  var f = (int? x) { return x; };\n}\n");
    assert_type_positions_resolve(&report);
}

// —— the type-parameter tag face (the fix witnesses) ————————————————

/// The minimal witness: a generic declaration whose variable is spelled
/// `int` must not poison an earlier `int` parameter. The oracle is silent
/// for this file.
///
/// These pins ride [`report_ops_core_ctor`] for the same reason the
/// closure pins do: [`report_ops`]' prelude declares `int` as this unit's
/// OWN class, so `lookup_scoped` answers that class declaration and the
/// file-scoped `TypeParam` tag is never reached — the face is invisible
/// there.
#[test]
fn later_core_named_type_parameter_does_not_poison_an_earlier_core_use() {
    let report = report_ops_core_ctor("void f(int x) {}\nclass C<int> {}\n");
    assert_type_positions_resolve(&report);
}

/// The same poison through a closure's type-argument group — the shape the
/// round's probes surfaced: the second literal's `<int>` group reported
/// `undefined_class` because a sibling literal declared `<int>`. The
/// oracle is silent for this file.
#[test]
fn core_named_literal_type_parameter_does_not_poison_a_sibling_literal() {
    let report = report_ops_core_ctor(
        "class C<T> {\n  void m<U>() {\n    var e = <V>(V v) => v;\n    var f = <int>(int x) => x;\n    e;\n    f;\n  }\n}\n",
    );
    assert_type_positions_resolve(&report);
}

/// The top-level statement form of the same witness: the type-argument
/// group is a written type position regardless of the surrounding scope.
#[test]
fn core_named_literal_type_parameter_does_not_poison_a_sibling_top_level_literal() {
    let report = report_ops_core_ctor(
        "void main() {\n  var a = (int y) => y;\n  var f = <int>(int x) => x;\n  a;\n  f;\n}\n",
    );
    assert_type_positions_resolve(&report);
}

/// A genuinely undeclared type variable still reports: the placeholder
/// falls through to the namespace and misses there. The oracle reports
/// `Undefined class 'T'.` at the use. This pin is load-bearing in the
/// opposite direction — it fails if the fall-through ever turns into an
/// unconditional silence.
#[test]
fn out_of_scope_type_parameter_still_reports() {
    let report = report_ops_core_ctor("class C<T> {}\nT x;\n");
    assert_eq!(
        report
            .diagnostics
            .iter()
            .filter(|d| d.code.name == "undefined_class")
            .map(|d| d.message.to_string())
            .collect::<Vec<_>>(),
        ["Undefined class 'T'.".to_string()],
        "{:?}",
        rows(&report)
    );
}

/// The same report for a non-core spelling, through a parameter position —
/// the tag withdrawal is name-independent.
#[test]
fn out_of_scope_type_parameter_in_a_parameter_still_reports() {
    let report = report_ops_core_ctor("class C<K> {}\nvoid f(K x) {}\n");
    assert_eq!(
        report
            .diagnostics
            .iter()
            .filter(|d| d.code.name == "undefined_class")
            .map(|d| d.message.to_string())
            .collect::<Vec<_>>(),
        ["Undefined class 'K'.".to_string()],
        "{:?}",
        rows(&report)
    );
}

/// The user-class control: a class named `T` stays reachable behind a
/// same-spelled type-parameter tag, so the tag no longer shadows the
/// namespace. (The oracle's remaining row here is the uninitialized
/// variable, a different face.)
#[test]
fn same_spelled_class_behind_a_type_parameter_tag_stays_resolvable() {
    let report = report_ops_core_ctor("class T {}\nclass C<T> {}\nT x = T();\n");
    assert_type_positions_resolve(&report);
}

/// An in-scope type variable keeps resolving as a type — the fix only
/// withdraws the placeholder OUTSIDE its declaration.
#[test]
fn in_scope_type_parameter_still_resolves() {
    let report = report_ops_core_ctor("class C<T> { T? t; }\n");
    assert_type_positions_resolve(&report);
}
