//! The `void_type_usage` face pins: a `void`-typed operand consumed by a
//! cascade section or a `do`-while condition reports `use_of_void_result`
//! and NO dead interval (oracle 3.13.7 reports the type error only), while
//! a dead region that starts on such a statement still reports exactly one
//! dead row for the tail.

use crate::diagnose::test_rig::{code_names, report_ops, rows};

/// A live cascade over a `void` member: the `use_of_void_result` row
/// survives (oracle `3:3` over `a.x`), and no `dead_code` stands.
#[test]
fn void_cascade_live_keeps_use_of_void_without_dead() {
    let report = report_ops("class A { void x; }\nvoid f(A a) {\n  a.x..toString();\n}\n");
    let names = code_names(&report);
    assert!(names.contains(&"use_of_void_result"), "{:?}", rows(&report));
    assert!(!names.contains(&"dead_code"), "{:?}", rows(&report));
}

/// The `do`-while twin (`do {} while (a.x);`): same split — the type error
/// stays, the dead interval is gone.
#[test]
fn void_do_while_live_keeps_use_of_void_without_dead() {
    let report = report_ops("class A { void x; }\nvoid f(A a) {\n  do {} while (a.x);\n}\n");
    let names = code_names(&report);
    assert!(names.contains(&"use_of_void_result"), "{:?}", rows(&report));
    assert!(!names.contains(&"dead_code"), "{:?}", rows(&report));
}

/// The dead-region shape that produced the eight `void_type_usage` extras:
/// a tail that begins on a `?.` statement over a receiver with no read
/// span and continues into a `void` cascade and a print must be ONE dead
/// row (oracle 3.13.7 `4:3|73`), never a split at the chain's end.
#[test]
fn dead_region_short_and_void_cascade_is_one_row() {
    let report = report_ops(
        "void f() {\n  void x;\n  throw 1;\n  (true ? 499 : x)?.toString();\n  (true ? 499 : x)..toString();\n  print(1);\n}\n",
    );
    let dead: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "dead_code")
        .collect();
    assert_eq!(dead.len(), 1, "{:?}", rows(&report));
    assert_eq!(dead[0].range.length, 73, "{:?}", rows(&report));
}
