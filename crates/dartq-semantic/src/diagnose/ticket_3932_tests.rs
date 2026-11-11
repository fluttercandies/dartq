//! Ticket #3932 pins: the core-constructor tear-off FORMALS face — the
//! SDK factories' signature table (`crate::member_face::
//! CORE_CTOR_FORMALS`) consumed at `ctor_selector_ty`'s resolve-miss arm
//! (`core_ctor_face`), covering the co19 `tearing_off_from_typedef`
//! family's thirteen census rows (every `List.filled` tear-off through a
//! typedef: the argument rows land; the one alias-bounds row routes to
//! the bounds face's own lane).
//!
//! The SDK-head fixtures run [`report_ops_core_ctor`] — the rig's
//! names-only `dart:core` stub extended with the container heads, the
//! CLI's unindexed-SDK grade (the names resolve, no element model
//! answers), the only posture where `resolve_container` chain-breaks at
//! the receiver exactly as over the real SDK projection and this face
//! owns the row. Counts quoted are the gated census
//! `census-20261104-092425` (`target/conformance/`, in-workspace only);
//! renderings are the oracle's own machine rows over the same fixtures.

use super::test_rig::{report_ops, report_ops_core_ctor};
use super::*;

/// Every `code` row of `report` as its message text.
fn messages_of(report: &FileReport, code: &str) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == code)
        .map(|d| d.message.to_string())
        .collect()
}

// —— the family: `List.filled` through a plain typedef ———————————————
//
// `typedef IntList = List<int>`'s `IntList.filled` reads `List<int>
// Function(int, int, {bool growable})` — the alias's right-hand-side
// arguments instantiate the factory's `E` (co19
// `tearing_off_from_typedef_A01_t02`'s two rows: 'String'→'int' and
// 'Null'→'int').

/// The plain-alias tear-off's arguments check against the instantiated
/// formals. Fails before: the receiver resolved to no container and the
/// tear-off kept the under-report `None` (both census rows missing).
#[test]
fn plain_alias_filled_tearoff_checks_arguments() {
    let source = r#"
typedef IntList = List<int>;
main() {
  var v = IntList.filled;
  v(3, "This is a String");
  v(3, null);
}
"#;
    let report = report_ops_core_ctor(source);
    let rows = messages_of(&report, "argument_type_not_assignable");
    assert_eq!(rows.len(), 2, "{:#?}", report.diagnostics);
    assert!(
        rows.iter()
            .any(|m| m.contains("'String'") && m.contains("'int'")),
        "{rows:#?}"
    );
    assert!(
        rows.iter()
            .any(|m| m.contains("'Null'") && m.contains("'int'")),
        "{rows:#?}"
    );
}

// —— the generic typedef: written arguments substitute ———————————————
//
// `typedef MyList<T> = List<T>` written `MyList<int>.filled` is
// `List<int> Function(int, int, {bool growable})` — the alias's own `T`
// reads the written `int` before the right-hand `List<T>` instantiates
// (co19 `tearing_off_from_typedef_A03_t02`'s three rows).

/// The generic alias's written group substitutes the right-hand side's
/// parameters. Fails before: every census row of the family was missing.
#[test]
fn generic_alias_filled_tearoff_substitutes_written_args() {
    let source = r#"
typedef MyList<T> = List<T>;
main() {
  var v = MyList<int>.filled;
  v(3, "");
  var v1 = MyList<String>.filled;
  v1(3, null);
  v1(3, 4);
}
"#;
    let report = report_ops_core_ctor(source);
    let rows = messages_of(&report, "argument_type_not_assignable");
    assert_eq!(rows.len(), 3, "{:#?}", report.diagnostics);
    assert!(
        rows.iter()
            .any(|m| m.contains("'String'") && m.contains("'int'")),
        "{rows:#?}"
    );
    assert!(
        rows.iter()
            .any(|m| m.contains("'Null'") && m.contains("'String'")),
        "{rows:#?}"
    );
    assert!(
        rows.iter()
            .any(|m| m.contains("'int'") && m.contains("'String'")),
        "{rows:#?}"
    );
}

// —— the bounded generic typedef ————————————————————————————————————
//
// `typedef MyList<T extends num> = List<T>` instantiates the same way —
// the alias's BOUND never gates the tear-off's own type (the
// written-argument bound check is the bounds face's report, routed);
// only the argument rows are this face's (co19
// `tearing_off_from_typedef_A03_t05`'s three argument rows and
// `A03_t08`'s one).

/// The bounded alias's tear-off checks arguments under the bound's
/// instantiations (`num`, `int`). Fails before: all four census rows
/// missing.
#[test]
fn bounded_alias_filled_tearoff_reads_instantiation() {
    let source = r#"
typedef MyList<T extends num> = List<T>;
String s = "";
main() {
  var v = MyList<num>.filled;
  v(3, "");
  var v1 = MyList<int>.filled;
  v1(3, null);
  v1(7, 4.3);
  var v2 = MyList<int>.filled;
  v2(1, s);
}
"#;
    let report = report_ops_core_ctor(source);
    let rows = messages_of(&report, "argument_type_not_assignable");
    assert_eq!(rows.len(), 4, "{:#?}", report.diagnostics);
    assert!(
        rows.iter()
            .any(|m| m.contains("'String'") && m.contains("'num'")),
        "{rows:#?}"
    );
    assert!(
        rows.iter()
            .any(|m| m.contains("'Null'") && m.contains("'int'")),
        "{rows:#?}"
    );
    assert!(
        rows.iter()
            .any(|m| m.contains("'double'") && m.contains("'int'")),
        "{rows:#?}"
    );
}

// —— the signature table: every row renders the official face ————————
//
// The oracle's own machine rows over the same nine shapes (the
// reference SDK's `sdk/lib/core/list.dart`:151/:165/:194/:210/:248,
// `map.dart`:64/:76, `set.dart`:82/:97).

/// Each tabulated factory's tear-off type renders exactly the oracle's
/// spelling. Fails before: every head resolved to nothing and each
/// assignment kept its silence.
#[test]
fn sdk_ctor_tearoff_faces_render_official_signatures() {
    let source = r#"
main() {
  var a = List<int>.filled;
  var b = List<int>.empty;
  var c = List<int>.of;
  var d = List<int>.from;
  var e = Map<String, int>.from;
  var f = Map<String, int>.of;
  var g = Set<String>.from;
  var h = Set<String>.of;
  var i = List<int>.unmodifiable;
  int za = a;
  int zb = b;
  int zc = c;
  int zd = d;
  int ze = e;
  int zf = f;
  int zg = g;
  int zh = h;
  int zi = i;
}
"#;
    let report = report_ops_core_ctor(source);
    let expected = [
        "List<int> Function(int, int, {bool growable})",
        "List<int> Function({bool growable})",
        "List<int> Function(Iterable<int>, {bool growable})",
        "List<int> Function(Iterable<Object?>, {bool growable})",
        "Map<String, int> Function(Map<dynamic, dynamic>)",
        "Map<String, int> Function(Map<String, int>)",
        "Set<String> Function(Iterable<dynamic>)",
        "Set<String> Function(Iterable<String>)",
        "List<int> Function(Iterable<dynamic>)",
    ];
    let rows = messages_of(&report, "invalid_assignment");
    assert_eq!(rows.len(), expected.len(), "{:#?}", report.diagnostics);
    for want in expected {
        assert!(
            rows.iter().any(|m| m.contains(&format!("'{want}'"))),
            "missing {want:?} in {rows:#?}"
        );
    }
}

// —— the face's own boundaries ———————————————————————————————————————
//
// The negative-evidence pins: the shapes the table does NOT claim stay
// silent — the bare SDK head (the generic tear-off form is outside the
// table's claim) and the spelled group on a NON-generic alias (the
// wrong-type-argument-count face's own report, never an instantiation).

/// The bare `List.filled` keeps the under-report — the generic tear-off
/// form (`List<E> Function<E>(...)`) is not a table claim.
#[test]
fn bare_sdk_list_filled_tearoff_stays_silent() {
    let source = r#"
main() {
  var v = List.filled;
  v(3, "str");
}
"#;
    let report = report_ops_core_ctor(source);
    assert!(
        messages_of(&report, "argument_type_not_assignable").is_empty(),
        "{:#?}",
        report.diagnostics
    );
}

/// A spelled group on a NON-generic alias declines — the count face owns
/// that report and the tear-off face must not stand a type in for it.
#[test]
fn non_generic_alias_with_spelled_group_declines() {
    let source = r#"
typedef MyList = List<int>;
main() {
  var v = MyList<int>.filled;
  v(1, "str");
}
"#;
    let report = report_ops_core_ctor(source);
    assert!(
        messages_of(&report, "argument_type_not_assignable").is_empty(),
        "{:#?}",
        report.diagnostics
    );
}

// —— the control: the user-class face is unchanged ———————————————————
//
// The indexed-container path `ctor_selector_ty` always served (the
// regression guard the brief asks for): a plain alias to a user class
// answers the declared formals exactly as before.

/// `A.new` through `typedef AA = A` keeps its argument row — the
/// core-table arm never touches an indexed container.
#[test]
fn user_class_alias_ctor_tearoff_still_answers() {
    let source = r#"
class A {
  A(int a, int b);
}
typedef AA = A;
main() {
  var v = AA.new;
  v(1, "str");
}
"#;
    let report = report_ops(source);
    let rows = messages_of(&report, "argument_type_not_assignable");
    assert_eq!(rows.len(), 1, "{:#?}", report.diagnostics);
    assert!(
        rows[0].contains("'String'") && rows[0].contains("'int'"),
        "{}",
        rows[0]
    );
}
