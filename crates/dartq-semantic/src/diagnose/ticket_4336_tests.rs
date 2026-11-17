//! Ticket #4336 pins: the could_not_infer invocation-seam cluster — the
//! generic function-tear-off argument's INSTANTIATED decision face, the
//! alias-formal creation gather, the mutual-bounds window, the
//! receiver-seeded method bound, and the bare creation's dynamic-only
//! solution.
//!
//! * A generic tear-off argument under a function-typed formal decides on
//!   the instantiated signature (`inferTearOff`'s return feeds the
//!   per-argument walk — resolver.dart's `constrainArgument` rounds):
//!   `g(f)` over `f<T extends num>(T, T)` reports `num Function(num,
//!   num)`, never the written `T Function<T extends num>(T, T)`; a
//!   bound-gate failure (Object parameters) reports ATNA alone, a
//!   viable-candidate failure (`returnIsOne`) co-reports couldNotInfer's
//!   Tried-to-infer form, and satisfying formals stay silent. The bare,
//!   prefixed (`a.f`), and instance (`C().m`) faces answer alike.
//! * A creation's alias formal gathers against the unfolded right-hand
//!   side (`C(F<T> f)` with `NotA myF()` reports `NotA Function()` vs
//!   `F<A>` — the gate keeps `NotA` off `T extends A`, i2b settles `T :=
//!   A`).
//! * A mutual-bounds window (`X extends Y, Y extends A<X>`) reports ONLY
//!   the call's couldNotInfer — the declaration-site
//!   `type_argument_not_matching_bounds` the window's one-step bound
//!   read kept is the same violation's duplicate (the transitive bound
//!   `A<X>` subsumes the required bound).
//! * A receiver instantiation grounds the method's declared bound
//!   (`new Foo<MyStr>().method(42)` over `U extends T` reports ATNA
//!   `int` vs `MyStr`, silent on couldNotInfer — the oracle's
//!   failAtError recovery).
//! * A bare creation's dynamic-only solution reads its bound (`C([])` over
//!   `C<P extends num>(Iterable<P>)` reports `List<dynamic>` vs
//!   `Iterable<num>`).

#![allow(clippy::semicolon_if_nothing_returned)]
use super::test_rig::{CORE_PRELUDE, RigImport, report_ops, report_ops_imports};
use super::*;

/// `(code, anchored text)` pairs, sorted — the emission order of one
/// site's sibling rows is not part of the contract.
fn code_spans(report: &FileReport, full: &str) -> Vec<(&'static str, String)> {
    let mut spans: Vec<(&'static str, String)> = report
        .diagnostics
        .iter()
        .map(|d| {
            (
                d.code.name,
                full[d.range.offset as usize..(d.range.offset + d.range.length) as usize]
                    .to_string(),
            )
        })
        .collect();
    spans.sort();
    spans
}

fn rows(report: &FileReport) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .map(|d| d.message.to_string())
        .collect()
}

/// `g(f)` over `f<T extends num>(T, T)` with Object formal parameters:
/// the bound gate drops both lowers, `T := num` stands, and the walk
/// reports the INSTANTIATED signature against the formal.
#[test]
fn tearoff_object_params_report_instantiated() {
    let fixture = "external T f<T extends num>(T a, T b);\n\
                   void g(num cb(Object a, Object b)) {}\n\
                   void main() {\n\
                   \x20 g(f);\n\
                   }\n";
    let full = format!("{CORE_PRELUDE}{fixture}");
    let report = report_ops(fixture);
    assert_eq!(
        code_spans(&report, &full),
        vec![("argument_type_not_assignable", "f".to_string())],
        "rows: {:?}",
        rows(&report)
    );
    assert!(rows(&report).iter().any(|row| {
        row.contains("The argument type 'num Function(num, num)'")
            && row.contains("parameter type 'num Function(Object, Object)'")
    }));
}

/// The prefixed face: `g(a.f)` — the same instantiation decision keyed
/// by the prefix's target declaration, reported at the whole `a.f`.
#[test]
fn tearoff_prefixed_face_reports_instantiated() {
    let fixture = "void g(num cb(Object a, Object b)) {}\n\
                   void main() {\n\
                   \x20 g(a.f);\n\
                   }\n";
    let full = format!("{CORE_PRELUDE}{fixture}");
    let report = report_ops_imports(
        fixture,
        &[RigImport {
            uri: "package:t/a.dart",
            prefix: Some("a"),
            source: "external T f<T extends num>(T a, T b);\n",
        }],
    );
    assert_eq!(
        code_spans(&report, &full),
        vec![("argument_type_not_assignable", "a.f".to_string())],
        "rows: {:?}",
        rows(&report)
    );
    assert!(rows(&report).iter().any(|row| {
        row.contains("The argument type 'num Function(num, num)'")
            && row.contains("parameter type 'num Function(Object, Object)'")
    }));
}

/// The instance face: `g(C().m)` over `T m<T extends num>(T x, T y)` —
/// the created receiver's non-static method answers the same decision.
#[test]
fn tearoff_instance_method_face_reports_instantiated() {
    let fixture = "class C {\n\
                   \x20 T m<T extends num>(T x, T y) {\n\
                   \x20   throw 'error';\n\
                   \x20 }\n\
                   }\n\
                   void g(int cb(Object a, Object b)) {}\n\
                   void main() {\n\
                   \x20 g(C().m);\n\
                   }\n";
    let full = format!("{CORE_PRELUDE}{fixture}");
    let report = report_ops(fixture);
    assert_eq!(
        code_spans(&report, &full),
        vec![("argument_type_not_assignable", "C().m".to_string())],
        "rows: {:?}",
        rows(&report)
    );
    assert!(rows(&report).iter().any(|row| {
        row.contains("The argument type 'num Function(num, num)'")
            && row.contains("parameter type 'int Function(Object, Object)'")
    }));
}

/// The satisfying gallery: formal parameters whose types pass the gate
/// (`int`/`double` under `T extends num`) solve a real `T` and the
/// instantiated signature IS assignable — silent, never the raw
/// `T Function<T extends num>(T, T)` rejection the pre-#4336 face kept.
#[test]
fn tearoff_subtype_params_stay_silent() {
    let fixture = "external T f<T extends num>(T a, T b);\n\
                   void g1(num cb(int a, int b)) {}\n\
                   void g2(Object cb(int a, int b)) {}\n\
                   void g3(num cb(int a, double b)) {}\n\
                   void main() {\n\
                   \x20 g1(f);\n\
                   \x20 g2(f);\n\
                   \x20 g3(f);\n\
                   }\n";
    let full = format!("{CORE_PRELUDE}{fixture}");
    let report = report_ops(fixture);
    assert_eq!(
        code_spans(&report, &full),
        Vec::<(&'static str, String)>::new(),
        "rows: {:?}",
        rows(&report)
    );
}

/// The viable-candidate failure: `int`/`double` lowers disagree under an
/// `int` return — couldNotInfer's Tried-to-infer form plus the ATNA on
/// the instantiated signature.
#[test]
fn tearoff_return_is_one_reports_tried_to_infer() {
    let fixture = "external T f<T extends num>(T a, T b);\n\
                   void g(int cb(int a, double b)) {}\n\
                   void main() {\n\
                   \x20 g(f);\n\
                   }\n";
    let full = format!("{CORE_PRELUDE}{fixture}");
    let report = report_ops(fixture);
    assert_eq!(
        code_spans(&report, &full),
        vec![
            ("argument_type_not_assignable", "f".to_string()),
            ("could_not_infer", "f".to_string()),
        ],
        "rows: {:?}",
        rows(&report)
    );
    assert!(rows(&report).iter().any(|row| {
        row.contains("Tried to infer 'num' for 'T'")
            && row.contains("'int Function(int, double)' is required")
    }));
    assert!(rows(&report).iter().any(|row| {
        row.contains("The argument type 'num Function(num, num)'")
            && row.contains("parameter type 'int Function(int, double)'")
    }));
}

/// The alias-formal creation: `C(F<T> f)` gathers `myF` against the
/// unfolded `T Function()` — the gate keeps `NotA` off `T extends A`,
/// i2b settles `T := A`, and the walk reports the substituted alias
/// formal (`F<A>`), never the raw `F<T>`.
#[test]
fn creation_alias_formal_reports_instantiated() {
    let fixture = "class A {}\n\
                   typedef T F<T>();\n\
                   class C<T extends A> {\n\
                   \x20 C(F<T> f);\n\
                   }\n\
                   class NotA {}\n\
                   NotA myF() => null;\n\
                   main() {\n\
                   \x20 var x = C(myF);\n\
                   }\n";
    let full = format!("{CORE_PRELUDE}{fixture}");
    let report = report_ops(fixture);
    let atna: Vec<_> = code_spans(&report, &full)
        .into_iter()
        .filter(|(code, _)| *code == "argument_type_not_assignable")
        .collect();
    assert_eq!(
        atna,
        vec![("argument_type_not_assignable", "myF".to_string())],
        "rows: {:?}",
        rows(&report)
    );
    assert!(rows(&report).iter().any(|row| {
        row.contains("The argument type 'NotA Function()'") && row.contains("parameter type 'F<A>'")
    }));
}

/// The mutual-bounds window: `foo<X extends Y, Y extends A<X>>()` reports
/// ONLY the call's couldNotInfer — the declaration-site
/// `type_argument_not_matching_bounds` on the window's own `X` is the
/// duplicate the transitive bound (`A<X>`) subsumes.
#[test]
fn mutual_bounds_window_reports_could_not_infer_alone() {
    let fixture = "class A<X extends A<X>> {}\n\
                   void foo<X extends Y, Y extends A<X>>() {}\n\
                   void f() {\n\
                   \x20 foo();\n\
                   }\n";
    let full = format!("{CORE_PRELUDE}{fixture}");
    let report = report_ops(fixture);
    assert_eq!(
        code_spans(&report, &full),
        vec![("could_not_infer", "foo".to_string())],
        "rows: {:?}",
        rows(&report)
    );
    assert!(rows(&report).iter().any(|row| {
        row.contains("Couldn't infer type parameter 'Y'")
            && row.contains("instantiated from 'A<X>' using type arguments")
    }));
}

/// The receiver-seeded method bound: `new Foo<MyStr>().method(42)` over
/// `U method<U extends T>(U u)` — the container's `T := MyStr` (the
/// created receiver's written instantiation) grounds the method's bound,
/// the gate drops `int`, and the failAtError-silent recovery settles the
/// formal at `MyStr`: ATNA alone. The bound interface is the rig's own
/// (`MyStr implements Ptn`) — the corpus's `Pattern` face is dart:core's,
/// which the prelude's `String` cannot mirror.
#[test]
fn receiver_seeded_method_bound_reports_atna_alone() {
    let fixture = "class Ptn {}\n\
                   class MyStr implements Ptn {}\n\
                   class Foo<T extends Ptn> {\n\
                   \x20 U method<U extends T>(U u) => u;\n\
                   }\n\
                   main() {\n\
                   \x20 new Foo<MyStr>().method(42);\n\
                   }\n";
    let full = format!("{CORE_PRELUDE}{fixture}");
    let report = report_ops(fixture);
    assert_eq!(
        code_spans(&report, &full),
        vec![("argument_type_not_assignable", "42".to_string())],
        "rows: {:?}",
        rows(&report)
    );
    assert!(rows(&report).iter().any(|row| {
        row.contains("The argument type 'int'") && row.contains("parameter type 'MyStr'")
    }));
}

/// The bare creation's dynamic-only solution: `C([])` over
/// `C<P extends num>(Iterable<P> p)` — the empty literal's `dynamic`
/// element constrains nothing, i2b settles `P := num`, and the effective
/// formal reads `Iterable<num>`.
#[test]
fn bare_creation_empty_list_reports_bound() {
    let fixture = "class C<P extends num> {\n\
                   \x20 factory C(Iterable<P> p) => C._();\n\
                   \x20 C._();\n\
                   }\n\
                   var c = C([]);\n";
    let full = format!("{CORE_PRELUDE}{fixture}");
    let report = report_ops(fixture);
    assert_eq!(
        code_spans(&report, &full),
        vec![("argument_type_not_assignable", "[]".to_string())],
        "rows: {:?}",
        rows(&report)
    );
    assert!(rows(&report).iter().any(|row| {
        row.contains("The argument type 'List<dynamic>'")
            && row.contains("parameter type 'Iterable<num>'")
    }));
}
