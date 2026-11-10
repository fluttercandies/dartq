//! #3854 pins — every cascade section resolves against the cascade target.
//!
//! Upstream analyzes one target per cascade and every section is a member
//! access on it (`resolver.dart`'s `visitCascadeExpression`; a section's
//! access reaches the resolver with a null target — "the property access
//! target is the target of a cascade" — and `static_type_analyzer.dart`
//! types the whole cascade as the target). No section ever chains on the
//! PREVIOUS section's value, so `A()..x..x..x` reads `x` on `A` three times
//! and stays silent.
//!
//! `merge_member`'s cascade-target latch armed correctly but then disarmed
//! itself whenever the popped receiver row sat at an offset registered in
//! `cascade_sections`. The listener registers every section's merged row
//! there, so from section 2 on the popped row WAS the previous section's
//! row: the latch dropped and the member folded against the previous
//! section's value type — the fixture below reported
//! `The getter 'x' isn't defined for the type 'int'.` at the 2nd and 3rd
//! `x`. That disarm existed for the chained-send shape
//! (`..headers.addAll(x)`), which the enclosing `(in_cascade && is_section)`
//! gate already excludes: a chained access merges behind a plain `.`/`?.`,
//! never a section operator.
//!
//! The oracle's (probe 3.13.7, `dart analyze --format=machine` over a
//! single-file package) exact rows for the fixtures below:
//!
//! ```text
//! // var a = A()..x..x..x; class A { final int x = 0; }
//! (no rows)
//! // void f(A a) { a..x..x..x; }
//! (no rows)
//! // var a = A()..x = 1..x..x; class A { int x = 0; }
//! (no rows)
//! // var b = Buf()..headers.check(1)..headers.check(2);
//! //   class Buf { final Inner headers = Inner(); }
//! //   class Inner { bool check(int x) => x > 0; }
//! (no rows)
//! // var a = A()?..x..x; class A { final int x = 0; }
//! WARNING|STATIC_WARNING|INVALID_NULL_AWARE_OPERATOR|1|12|3|The receiver
//!   can't be null, so the null-aware operator '?..' is unnecessary.
//! // var a = A()..x..zzz; class A { final int x = 0; }
//! ERROR|COMPILE_TIME_ERROR|UNDEFINED_GETTER|1|17|3|The getter 'zzz' isn't
//!   defined for the type 'A'.
//! // var a = A()..x = 1; var b = B()..x..x;
//! //   class A { int x = 0; } class B { final int x = 0; }
//! (no rows)
//! ```
//!
//! The miss direction is pinned as hard as the silence: a later section's
//! genuinely undefined member still reports against the cascade target, and
//! the `?..` operator verdict keeps its single row.
//!
//! Registered from `invocation.rs` via `#[path]`, not in `tests.rs` (other
//! lanes' file).

use super::FileReport;
use super::test_rig::report_ops;

/// Every diagnostic's `(code name, message)` in report order.
fn rows(report: &FileReport) -> Vec<(String, String)> {
    report
        .diagnostics
        .iter()
        .map(|row| (row.code.name.to_string(), row.message.to_string()))
        .collect()
}

/// Messages of one code, in report order.
fn messages_named(report: &FileReport, name: &str) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .filter(|row| row.code.name == name)
        .map(|row| row.message.to_string())
        .collect()
}

/// `A()..x..x..x` — sections 2 and 3 read `x` on `A`, not on section 1's
/// `int` (the #3854 defect: two spurious `undefined_getter` rows).
#[test]
fn cascade_repeat_getter_reads_resolve_against_target() {
    let report = report_ops("var a = A()..x..x..x;\nclass A { final int x = 0; }\n");
    assert_eq!(rows(&report), Vec::<(String, String)>::new());
}

/// The same shape inside a function body, where the target is a parameter
/// rather than a creation.
#[test]
fn cascade_repeat_reads_in_body_resolve_against_target() {
    let report = report_ops("void f(A a) {\n  a..x..x..x;\n}\nclass A { final int x = 0; }\n");
    assert_eq!(rows(&report), Vec::<(String, String)>::new());
}

/// An assignment section's RHS value is not the next section's receiver
/// (`..x = 1..x..x`), the twin shape the existing S4-B latch covered.
#[test]
fn cascade_assignment_value_is_not_the_next_receiver() {
    let report = report_ops("var a = A()..x = 1..x..x;\nclass A { int x = 0; }\n");
    assert_eq!(rows(&report), Vec::<(String, String)>::new());
}

/// The regression guard for the removed disarm: a send chained ON a section
/// (`..headers.check(x)`) must keep resolving `check` against the section's
/// member type (`Inner`), never the cascade target (`Buf`). The member
/// returns a non-`void` value so the cascade-section void face cannot
/// pollute this pin.
#[test]
fn cascade_chained_send_keeps_section_member_type() {
    let report = report_ops(
        "var b = Buf()..headers.check(1)..headers.check(2);\n\
         class Buf { final Inner headers = Inner(); }\n\
         class Inner { bool check(int x) => x > 0; }\n",
    );
    assert_eq!(rows(&report), Vec::<(String, String)>::new());
}

/// A null-aware cascade keeps exactly the operator verdict — the repeated
/// reads stay silent and no `undefined_getter` sneaks in.
#[test]
fn cascade_null_aware_repeat_reads_report_only_the_operator() {
    let report = report_ops("var a = A()?..x..x;\nclass A { final int x = 0; }\n");
    assert_eq!(
        messages_named(&report, "invalid_null_aware_operator"),
        ["The receiver can't be null, so the null-aware operator '?..' is unnecessary."]
    );
    assert!(messages_named(&report, "undefined_getter").is_empty());
}

/// The miss direction: a later section's genuinely undefined member reports
/// once, naming the cascade target.
#[test]
fn cascade_later_section_miss_still_reports() {
    let report = report_ops("var a = A()..x..zzz;\nclass A { final int x = 0; }\n");
    assert_eq!(
        messages_named(&report, "undefined_getter"),
        ["The getter 'zzz' isn't defined for the type 'A'."]
    );
}

/// A fresh cascade after a `;` keeps its own target: the latch's separator
/// probe still separates the two expressions.
#[test]
fn fresh_cascade_after_semicolon_keeps_its_own_target() {
    let report = report_ops(
        "var a = A()..x = 1; var b = B()..x..x;\n\
         class A { int x = 0; }\nclass B { final int x = 0; }\n",
    );
    assert_eq!(rows(&report), Vec::<(String, String)>::new());
}
