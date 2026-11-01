//! The #3400 pins — a const constructor's `x = e` field-initializer RHS
//! runs the oracle's `_reportNotPotentialConstants` scan at the
//! initializer list's close, with the constructor's formals bound opaque
//! minus the wildcards that bind no readable name.
//!
//! Oracle anchor (`dart analyze` 3.13.7, probes `/tmp/dq3400/w3.dart`,
//! `/tmp/dq3400/w.dart`):
//!
//! - `f = g` over an instance field reports `invalid_constant` at `g`
//!   beside `implicit_this_reference_in_initializer` at the same read;
//!   `f = s` over `static const s` and `y = x` over a formal stay silent.
//! - `f = top` over a non-const top-level variable reports
//!   `invalid_constant` at `top`.
//! - `y = _` where `_` is a wildcard field reports the
//!   `implicit_this_reference_in_initializer` + `invalid_constant` pair
//!   at the RHS `_` (the wildcard FORMAL `this._` never binds a readable
//!   `_`).
//!
//! Runs under the core-preloaded rig (`report_ops`): the fixtures declare
//! their own classes; `int` is the rig stub. Registered through
//! `diagnose/tests.rs`'s `#[path]` block; the module stays inside the
//! `diagnose` subtree so `crate::diagnose::test_rig` resolves.

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

/// A formal bound opaque stays silent — `y = x` over `const A(int x)` is
/// potentially constant, so the scan reports nothing.
#[test]
fn formal_read_in_initializer_list_is_silent() {
    let fixture = "class A {\n\
                  \x20 final int y;\n\
                  \x20 const A(int x) : y = x;\n\
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

/// A static const member's bare spelling stays silent — `f = s` over
/// `static const s` is the class's own scope, not an implicit `this`.
#[test]
fn static_const_read_in_initializer_list_is_silent() {
    let fixture = "class A {\n\
                  \x20 final int f;\n\
                  \x20 static const int s = 0;\n\
                  \x20 const A() : f = s;\n\
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

/// A bare instance-member read is not potentially constant — `f = g`
/// over instance `g` reports `invalid_constant` at `g` beside the
/// implicit-`this` face on the same span, exactly as the oracle pairs
/// them. The class's non-final `g` also fires the
/// const-constructor-with-non-final-field row.
#[test]
fn instance_member_read_in_initializer_list_reports_the_pair() {
    let fixture = "class A {\n\
                  \x20 final int f;\n\
                  \x20 int g = 0;\n\
                  \x20 const A() : f = g;\n\
                  }\n";
    let source = format!("{CORE_PRELUDE}{fixture}");
    let read = source.rfind('g').expect("fixture has the RHS g") as u32;
    // The oracle anchors the non-final-field row at the constructor's
    // name (`A` in `const A() : ...`), not the `const` keyword.
    let ctor = source.find("A() :").expect("fixture has the const ctor") as u32;
    let rep = report_ops_bare(&source);
    assert_eq!(
        oracle_rows(&rep, &source),
        vec![
            ("const_constructor_with_non_final_field", ctor, "A"),
            ("implicit_this_reference_in_initializer", read, "g"),
            ("invalid_constant", read, "g"),
        ],
        "{:#?}",
        oracle_rows(&rep, &source)
    );
}

/// A non-const top-level variable is not potentially constant — `f = top`
/// reports `invalid_constant` at `top`.
#[test]
fn nonconst_top_level_read_reports_invalid_constant() {
    let fixture = "int top = 0;\n\
                  class D {\n\
                  \x20 final int f;\n\
                  \x20 const D() : f = top;\n\
                  }\n";
    let source = format!("{CORE_PRELUDE}{fixture}");
    let read = source.rfind("top").expect("fixture has the RHS top") as u32;
    let rep = report_ops_bare(&source);
    assert_eq!(
        oracle_rows(&rep, &source),
        vec![("invalid_constant", read, "top"),],
        "{:#?}",
        oracle_rows(&rep, &source)
    );
}

/// A wildcard FORMAL (`this._`) binds no readable `_`, so `y = _` reads
/// the instance FIELD `_` — the `implicit_this_reference_in_initializer`
/// + `invalid_constant` pair at the RHS `_`, never at the formal's.
/// Corpus shape `InstanceCreationEvaluatorTest/
/// test_wildcard_regularInitializer_initializerList#0` (upstream Dart SDK
/// source, pkg/analyzer/test/src/dart/constant/evaluation_test.dart).
#[test]
fn wildcard_field_read_in_initializer_list_reports_at_the_read() {
    let fixture = "class T {}\n\
                  class A {\n\
                  \x20 final T _;\n\
                  \x20 final T y;\n\
                  \x20 const A(this._): y = _;\n\
                  }\n";
    let source = format!("{CORE_PRELUDE}{fixture}");
    let read = source.rfind('_').expect("fixture carries the RHS _") as u32;
    let rep = report_ops_bare(&source);
    assert_eq!(
        oracle_rows(&rep, &source),
        vec![
            ("implicit_this_reference_in_initializer", read, "_"),
            ("invalid_constant", read, "_"),
        ],
        "{:#?}",
        oracle_rows(&rep, &source)
    );
}
