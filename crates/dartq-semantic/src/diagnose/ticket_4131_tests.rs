//! Ticket #4131 pins: residual parity faces re-adjudicated against the
//! oracle.
//!
//! * A qualified implicit creation over a type-argument group
//!   (`const x = A<T>.named()`) keeps the `const_with_type_parameters`
//!   check: the selector after the group's closing `>` has a receiver
//!   (the instantiated type), so it is never a leading-dot shorthand.
//! * The core-member table claims a member the walk cannot see only in a
//!   universe that carries `dart:core`: a core-spelled local class in a
//!   graph with no SDK node is a plain declaration whose members are its
//!   whole face, so the miss reports.
//! * A linear receiver chain past the former recursion cap re-derives the
//!   closure formal's schema (the pass-2 receiver walk is memoized), so the
//!   closure body's miss reports against the solved formal type.

use super::test_rig::{CORE_PRELUDE, report_ops};
use super::*;

/// The rows of `code` as the exact anchored source text, source order.
fn spans_of(report: &FileReport, full: &str, code: &str) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == code)
        .map(|d| {
            full[d.range.offset as usize..(d.range.offset + d.range.length) as usize].to_string()
        })
        .collect()
}

/// `const x = A<List<T>>.named()`: the nested type-parameter leaf of a
/// qualified implicit creation reports at its own span. Oracle:
/// `CONST_WITH_TYPE_PARAMETERS` at the inner `T` ("A constant creation
/// can't use a type parameter as a type argument.").
#[test]
fn qualified_implicit_creation_nested_type_param_arg() {
    let fixture = "class A<X> { const A.named(); }\n\
                   class B<T> {\n\
                   \x20 void m() {\n\
                   \x20   const x = A<List<T>>.named();\n\
                   \x20 }\n\
                   }\n";
    let full = format!("{CORE_PRELUDE}{fixture}");
    let report = report_ops(fixture);
    assert_eq!(
        spans_of(&report, &full, "const_with_type_parameters"),
        ["T"],
        "the nested argument's `T` leaf reports"
    );
}

/// `const x = A<int>.named()`: a concrete argument over a const named
/// constructor is a valid constant. The oracle reports no const or
/// resolution row, so the creation's non-shorthand classification must
/// not invent one.
#[test]
fn qualified_implicit_creation_concrete_arg_is_silent() {
    let fixture = "class A<X> { const A.named(); }\n\
                   void m() {\n\
                   \x20 const x = A<int>.named();\n\
                   }\n";
    let report = report_ops(fixture);
    let faults: Vec<&str> = report
        .diagnostics
        .iter()
        .map(|d| d.code.name)
        .filter(|name| name.starts_with("const_") || name.starts_with("undefined_"))
        .collect();
    assert!(faults.is_empty(), "{faults:?}");
}

/// A core-spelled local class in a graph with no `dart:core` node keeps its
/// own declared face: `String` here declares no `length`, so the read
/// reports. Oracle on the same text: `UNDEFINED_GETTER` "The getter
/// 'length' isn't defined for the type 'String'.".
#[test]
fn core_member_table_stays_off_without_dart_core() {
    let r = report("class String {} void f(String s) { s.length; }");
    assert_eq!(code_names(&r), ["undefined_getter"], "{:?}", code_names(&r));
    assert_eq!(
        r.diagnostics[0].message,
        "The getter 'length' isn't defined for the type 'String'."
    );
}

/// The promoted-receiver send `if (b is Box) b.next…next.takeCb((e) =>
/// e.miss)` 1100 hops deep: the memoized pass-2 receiver walk resolves the
/// promoted `Box` face without recursing per hop, so the recursion cap
/// never fires on the linear chain, the closure formal solves to `Idx`,
/// and the body's miss reports. Oracle at 1100 hops: `UNDEFINED_GETTER`
/// "The getter 'miss' isn't defined for the type 'Idx'.".
#[test]
fn recv_walk_past_former_cap_reports_the_formal_schema() {
    let mut src = String::from(
        "class Box {\n\
         \x20 Box get next => this;\n\
         \x20 void takeCb(void f(Idx x)) {}\n\
         }\n\
         class Idx {}\n\
         void main() {\n\
         \x20 dynamic b = Box();\n\
         \x20 if (b is Box) {\n\
         \x20   b",
    );
    for _ in 0..1100 {
        src.push_str(".next");
    }
    src.push_str(".takeCb((e) => e.miss);\n  }\n}\n");
    let r = report(&src);
    assert_eq!(code_names(&r), ["undefined_getter"], "{:?}", code_names(&r));
    assert_eq!(
        r.diagnostics[0].message,
        "The getter 'miss' isn't defined for the type 'Idx'."
    );
}
