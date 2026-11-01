//! The #3457 pins — the extension consult and instance-member faces now
//! route receivers the named-container walk never owned: a type-variable
//! receiver resolves through its bound, and a `FutureOr` receiver checks
//! its nullable face against the extension `on` constraint.
//!
//! Oracle anchor (`dart analyze` 3.13.7, probes `/tmp/dq3457/e.dart`,
//! `/tmp/dq3457/f.dart`):
//!
//! - `T extends Bound`: `t.gone()` reports `undefined_method` for the
//!   type `<unknown>` (the oracle's parameter display), `t.m()` stays
//!   silent (the bound defines `m`), and `t.ext()` over
//!   `extension E on Bound` stays silent (the extension applies through
//!   the bound).
//! - `T extends Bound?`: `t.m()` reports `unchecked_use_of_nullable_value`
//!   at `m` (the nullable bound's member is still found, but the
//!   receiver can be null).
//! - `FutureOr<int?>`: `x.gone()` reports `unchecked_use_of_nullable_value`
//!   (the `Future`/`on Object` extension face a nullable FutureOr cannot
//!   unconditionally invoke); `FutureOr<int>` `y.gone()` reports
//!   `undefined_method` for the type 'FutureOr'.
//!
//! Runs under the core-preloaded rig (`report_ops`): `FutureOr` is a
//! names-only stub, and `Bound` declares its own members. Registered
//! through `diagnose/tests.rs`'s `#[path]` block; the module stays inside
//! the `diagnose` subtree so `crate::diagnose::test_rig` resolves.

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

/// A type-variable receiver resolves its member through the bound: the
/// undefined name reports `undefined_method` for the `<unknown>` display
/// the oracle uses for a bounded parameter.
#[test]
fn type_variable_receiver_reports_unknown_display() {
    let fixture = "class Bound { void m() {} }\n\
                  void f<T extends Bound>(T t) {\n\
                  \x20 t.gone();\n\
                  }\n";
    let source = format!("{CORE_PRELUDE}{fixture}");
    let name = source.rfind("gone").expect("fixture has the member") as u32;
    let rep = report_ops_bare(&source);
    assert_eq!(
        oracle_rows(&rep, &source),
        vec![("undefined_method", name, "gone"),],
        "{:#?}",
        oracle_rows(&rep, &source)
    );
}

/// The bound-defined member stays silent — `t.m()` over
/// `T extends Bound` resolves `Bound.m`.
#[test]
fn type_variable_receiver_bound_member_is_silent() {
    let fixture = "class Bound { void m() {} }\n\
                  void f<T extends Bound>(T t) {\n\
                  \x20 t.m();\n\
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

/// An extension whose `on` constraint is the bound applies to the
/// type-variable receiver — `t.ext()` over `extension E on Bound` is
/// silent.
#[test]
fn type_variable_receiver_extension_through_bound_is_silent() {
    let fixture = "class Bound { void m() {} }\n\
                  extension E on Bound { void ext() {} }\n\
                  void f<T extends Bound>(T t) {\n\
                  \x20 t.ext();\n\
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

/// A nullable bound (`T extends Bound?`) still finds the member, but the
/// receiver can be null — `unchecked_use_of_nullable_value` at the name.
#[test]
fn type_variable_nullable_bound_reports_unchecked() {
    let fixture = "class Bound { void m() {} }\n\
                  void f<T extends Bound?>(T t) {\n\
                  \x20 t.m();\n\
                  }\n";
    let source = format!("{CORE_PRELUDE}{fixture}");
    let name = source.rfind("m()").expect("fixture has the member") as u32;
    let rep = report_ops_bare(&source);
    assert_eq!(
        oracle_rows(&rep, &source),
        vec![("unchecked_use_of_nullable_value", name, "m"),],
        "{:#?}",
        oracle_rows(&rep, &source)
    );
}

/// `FutureOr<int?>`'s nullable face cannot unconditionally invoke the
/// extension/`Future` member — `unchecked_use_of_nullable_value`.
#[test]
fn future_or_nullable_receiver_reports_unchecked() {
    let fixture = "class Bound { void m() {} }\n\
                  extension E on Object { void gone() {} }\n\
                  void f(FutureOr<int?> x) {\n\
                  \x20 x.gone();\n\
                  }\n";
    let source = format!("{CORE_PRELUDE}{fixture}");
    let name = source.rfind("gone").expect("fixture has the member") as u32;
    let rep = report_ops_bare(&source);
    assert_eq!(
        oracle_rows(&rep, &source),
        vec![("unchecked_use_of_nullable_value", name, "gone"),],
        "{:#?}",
        oracle_rows(&rep, &source)
    );
}

/// `FutureOr<int>`'s non-null face answers the extension member but the
/// name is still not `FutureOr`'s own — `undefined_method` for the type
/// 'FutureOr'.
#[test]
fn future_or_non_null_receiver_reports_undefined_method() {
    let fixture = "void f(FutureOr<int> y) {\n\
                  \x20 y.gone();\n\
                  }\n";
    let source = format!("{CORE_PRELUDE}{fixture}");
    let name = source.rfind("gone").expect("fixture has the member") as u32;
    let rep = report_ops_bare(&source);
    assert_eq!(
        oracle_rows(&rep, &source),
        vec![("undefined_method", name, "gone"),],
        "{:#?}",
        oracle_rows(&rep, &source)
    );
}
