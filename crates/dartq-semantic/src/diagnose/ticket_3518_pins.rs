//! The #3518 face-2 / face-4 pins: the cast pattern's containment test and
//! the `Null` type test's canonical leaf.
//!
//! Face 2 — a cast pattern over an or-pattern. Oracle
//! (`~/.dartq-oracle/3.13.7/dart-sdk/bin/dart analyze`):
//!
//! ```dart
//! sealed class M {}
//! class A extends M {}
//! class B extends M {}
//! int f(M? m) => switch (m) { (A() || B()) as M => 0 };
//! ```
//! reports nothing. Every value of the cast type `M` is covered by the
//! sub-pattern's space (`A|B`), so the throw path covers the rest and the
//! cast matches the whole context `M?` (`_isContainedIn` over the union,
//! upstream `pkg/_fe_analyzer_shared/lib/src/exhaustiveness/shared.dart:
//! 660-698` + `createCastSpace` :704-735).
//!
//! Face 4 — a written `Null` type test is the `Null` static type. Oracle:
//!
//! ```dart
//! String g(int? v) => switch (v) {
//!   null => 'null',
//!   Null _ => 'null',   // <- unreachable_switch_case
//!   int _ => 'int',
//! };
//! ```
//! reports `unreachable_switch_case` on the `Null _` clause: the earlier
//! `null` constant already covers that space (`Null` is one static type,
//! `static_type.dart:21`), so the later clause is subsumed.

use super::test_rig::{CORE_PRELUDE, code_names, report_ops, rows};

/// A cast pattern whose cast type sits inside the sub-pattern's space
/// covers the whole matched context — no `non_exhaustive_switch_expression`.
///
/// Oracle: silent (the `(A() || B()) as M` over `M?` shape of
/// `patterns/exhaustiveness/cast_test`).
#[test]
fn cast_pattern_over_an_or_pattern_covers_the_whole_context() {
    let source = "sealed class M {}\n\
                  class A extends M {}\n\
                  class B extends M {}\n\
                  int f(M? m) => switch (m) {\n\
                    (A() || B()) as M => 0,\n\
                  };\n";
    let report = report_ops(source);
    assert_eq!(code_names(&report), Vec::<&str>::new(), "{:#?}", rows(&report));
}

/// A written `Null` type test and the `null` constant cover one space: the
/// later clause is subsumed by the earlier one.
///
/// Oracle:
///
/// ```dart
/// String g(int? v) => switch (v) {
///   null => 'null',
///   Null() => 'null',   // <- unreachable_switch_case <@ the arrow>
///   int _ => 'int',
/// };
/// ```
/// reports `unreachable_switch_case` on the `Null()` clause (`Null` is one
/// static type, `static_type.dart:21`), so the earlier `null` constant
/// covers it; the `int _` clause stays reachable. The switch-expression
/// row anchors at the clause's `=>`.
#[test]
fn null_constant_subsumes_the_null_type_test() {
    let source = "String g(int? v) => switch (v) {\n\
                  \x20 null => 'null',\n\
                  \x20 Null() => 'null',\n\
                  \x20 int _ => 'int',\n\
                  };\n";
    let report = report_ops(source);
    let names = code_names(&report);
    let unreachable: Vec<u32> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "unreachable_switch_case")
        .map(|d| d.range.offset)
        .collect();
    let full = format!("{CORE_PRELUDE}{source}");
    let subsumed = full.find("Null()").unwrap() as u32;
    let arrow = subsumed + full[subsumed as usize..].find("=>").unwrap() as u32;
    assert_eq!(unreachable, [arrow], "{:#?}", rows(&report));
    assert!(
        !names.iter().any(|name| *name == "non_exhaustive_switch_expression"),
        "{:#?}",
        rows(&report)
    );
}
