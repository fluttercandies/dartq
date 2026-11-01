//! The #3449 pins — a cascade section's target is its VALUE face, so a
//! non-bare target (`f()..x`, `C()..x`) checks members against the
//! target's own type, not against a synthesized or `Unknown` receiver.
//!
//! Oracle anchor (`dart analyze` 3.13.7, probe `/tmp/dq3449/c.dart`):
//! `f()..gone()` reports `undefined_method` 'gone' for the type 'C' at
//! `gone`, `C()..gone()` the same, and `C.new..gone()` reports 'gone' for
//! the type 'Function' (the `new` tear-off's static type). `f()..ok()`
//! stays silent. The pre-fix face answered the cascade's selector with a
//! receiver the target never supplied, missing the section's real
//! instance face.
//!
//! Runs under the core-preloaded rig (`report_ops`): the fixtures declare
//! their own `C`, and the cascade target's member check reads the class's
//! instance members. Registered through `diagnose/tests.rs`'s `#[path]`
//! block; the module stays inside the `diagnose` subtree so
//! `crate::diagnose::test_rig` resolves.

use super::test_rig::{CORE_PRELUDE, report_ops_bare};
use super::*;

/// `(code, offset, written text at the row)` — the span-exact projection:
/// code + offset + text pin a row's code, anchor, and length together.
/// The vector is sorted by `(code, offset, text)`, so expected rows are
/// listed in code-name order.
fn oracle_rows<'a>(report: &FileReport, source: &'a str) -> Vec<(&'static str, u32, &'a str)> {
    let mut rows: Vec<(&'static str, u32, &'a str)> = report
        .diagnostics
        .iter()
        .map(|d| {
            (
                d.code.name,
                d.range.offset,
                &source[d.range.offset as usize..d.range.end() as usize],
            )
        })
        .collect();
    rows.sort();
    rows
}

/// A cascade over a function-call target (`f()..gone()`) checks the
/// section against the send's result type — `undefined_method` on `C`,
/// at the section name, exactly as the oracle reports.
#[test]
fn cascade_over_call_target_checks_the_result_face() {
    let fixture = "class C {\n\
                  \x20 void ok() {}\n\
                  }\n\
                  C f() => C();\n\
                  void g() {\n\
                  \x20 f()..gone();\n\
                  }\n";
    let source = format!("{CORE_PRELUDE}{fixture}");
    let section = source.find("gone").expect("fixture has the section") as u32;
    let rep = report_ops_bare(&source);
    assert_eq!(
        oracle_rows(&rep, &source),
        vec![("undefined_method", section, "gone"),],
        "{:#?}",
        oracle_rows(&rep, &source)
    );
}

/// A cascade over a creation target (`C()..gone()`) checks the section
/// against the created instance face — `undefined_method` on `C` at the
/// section name.
#[test]
fn cascade_over_creation_target_checks_the_instance_face() {
    let fixture = "class C {\n\
                  \x20 void ok() {}\n\
                  }\n\
                  void g() {\n\
                  \x20 C()..gone();\n\
                  }\n";
    let source = format!("{CORE_PRELUDE}{fixture}");
    let section = source.find("gone").expect("fixture has the section") as u32;
    let rep = report_ops_bare(&source);
    assert_eq!(
        oracle_rows(&rep, &source),
        vec![("undefined_method", section, "gone"),],
        "{:#?}",
        oracle_rows(&rep, &source)
    );
}

/// The defined section stays silent: `f()..ok()` resolves `ok` on the
/// send's `C` face — no row.
#[test]
fn cascade_over_call_target_defined_section_is_silent() {
    let fixture = "class C {\n\
                  \x20 void ok() {}\n\
                  }\n\
                  C f() => C();\n\
                  void g() {\n\
                  \x20 f()..ok();\n\
                  }\n";
    let source = format!("{CORE_PRELUDE}{fixture}");
    let rep = report_ops_bare(&source);
    assert_eq!(
        oracle_rows(&rep, &source),
        Vec::<(&'static str, u32, &'static str)>::new(),
        "{:#?}",
        oracle_rows(&rep, &source)
    );
}
