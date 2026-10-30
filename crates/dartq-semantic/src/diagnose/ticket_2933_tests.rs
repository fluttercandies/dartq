//! The #2933 nameless-declaration pins: the parser's synthetic-tail gate
//! (#2932) fires `handle_identifier` with a synthetic name on every
//! nameless declaration context, and the listener now registers a real
//! element under the empty name (`name ?? '<unnamed>'`, element.dart:2093)
//! instead of dropping the event. A null-name element is PRIVATE upstream
//! (`ElementImpl.isPrivate`'s `name == null → true`, element.dart:2179), so
//! the unused-family gates treat it exactly like a private spelling.
//!
//! Upstream truth (dart-sdk-3.13.7, pkg/analyzer):
//! * `element.dart:2179` — `isPrivate` maps `name == null` to `true`;
//! * `element.dart:2093` — `displayName` renders `name ?? '<unnamed>'`;
//! * `unused_local_elements_verifier.dart`'s `_isUsedElement` /
//!   `_isReadMember` / `_isUsedMember` read `element.isPrivate`/`isPublic`,
//!   so a null-name field/local/member lands on the private arm.

use super::*;
use crate::diagnose::test_rig::{CORE_PRELUDE, report_ops, rows};

/// The written span of `text` inside the fixture `source`, shifted by the
/// [`CORE_PRELUDE`] the rig mounts ahead of it (UTF-16 offsets equal byte
/// offsets for the ASCII fixtures below).
fn span_of(source: &str, text: &str) -> (u32, u32) {
    let offset =
        CORE_PRELUDE.len() as u32 + source.find(text).expect("fixture text present") as u32;
    (offset, text.len() as u32)
}

/// The diagnostic rows of `report` for `code`, as sorted `(offset, length)`
/// pairs.
fn code_rows(report: &FileReport, code: &str) -> Vec<(u32, u32)> {
    let mut out: Vec<(u32, u32)> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == code)
        .map(|d| (d.range.offset, d.range.length))
        .collect();
    out.sort();
    out
}

/// The diagnostic messages of `report` for `code`, in source order.
fn code_messages(report: &FileReport, code: &str) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == code)
        .map(|d| d.message.to_string())
        .collect()
}

// —— 1a: nameless local variable ——

/// A nameless local variable (`var = 0;` — the declaration's name slot is
/// junk) registers a real `LocalVariableElement` upstream, not a dropped
/// one: the unused-local face reports `<unnamed>` at the name slot
/// (`displayName`'s `name ?? '<unnamed>'`).
#[test]
fn nameless_local_variable_reports_unnamed() {
    let source = "void f() {\n  var = 0;\n}\n";
    let report = report_ops(source);
    let messages = code_messages(&report, "unused_local_variable");
    assert!(
        messages.iter().any(|m| m.contains("<unnamed>")),
        "the nameless local reports `<unnamed>`: {:#?}",
        rows(&report)
    );
}

/// A NAMED unused local still reports its spelling — the nameless gate
/// changes nothing for the written-name face (regression).
#[test]
fn named_local_variable_still_reports() {
    let source = "void f() {\n  var x = 0;\n}\n";
    let report = report_ops(source);
    let messages = code_messages(&report, "unused_local_variable");
    assert!(
        messages.iter().any(|m| m.contains("'x'")),
        "the named local reports its own spelling: {:#?}",
        rows(&report)
    );
}

// —— 1b: nameless field ——

/// A nameless instance field inside a class (`var ;` — the keyword
/// commits the fields parse, so the head-scan synthetic name flows
/// through the slot like a parsed one, parser_impl.dart:4109
/// unconditional `handleIdentifier`) registers a real `FieldElement`
/// upstream; `isPrivate` maps its null name private (element.dart:2179),
/// so `_isReadMember`'s gate owns it and the unused-field face reports
/// `<unnamed>` at the declaration site.
#[test]
fn nameless_field_reports_unnamed_unused() {
    let source = "class C {\n  var ;\n}\n";
    let report = report_ops(source);
    let messages = code_messages(&report, "unused_field");
    assert!(
        messages.iter().any(|m| m.contains("<unnamed>")),
        "the nameless field reports `<unnamed>`: {:#?}",
        rows(&report)
    );
}

/// A named private field still reports; the nameless arm only adds the
/// missing registration, it does not alter the named face.
#[test]
fn named_private_field_still_reports() {
    let source = "class C {\n  int _f = 0;\n}\n";
    let report = report_ops(source);
    let messages = code_messages(&report, "unused_field");
    assert!(
        messages.iter().any(|m| m.contains("'_f'")),
        "the named private field reports its spelling: {:#?}",
        rows(&report)
    );
}

// —— 1c: nameless method ——

/// A nameless method inside a class registers a real `MethodElement`
/// upstream; the null name maps private (element.dart:2179), so the
/// `_isUsedMember` gate owns it and the unused-element face reports
/// `<unnamed>`.
#[test]
fn nameless_method_reports_unnamed_unused() {
    let source = "class C {\n  () {}\n}\n";
    let report = report_ops(source);
    let messages = code_messages(&report, "unused_element");
    assert!(
        messages.iter().any(|m| m.contains("<unnamed>")),
        "the nameless method reports `<unnamed>`: {:#?}",
        rows(&report)
    );
}

/// A named private method still reports its spelling (regression).
#[test]
fn named_private_method_still_reports() {
    let source = "class C {\n  void _m() {}\n}\n";
    let report = report_ops(source);
    let messages = code_messages(&report, "unused_element");
    assert!(
        messages.iter().any(|m| m.contains("'_m'")),
        "the named private method reports its spelling: {:#?}",
        rows(&report)
    );
}

// —— 1d: nameless local function ——

/// A bare lambda (`() => 0;`) inside a body is an expression statement,
/// not a declaration — upstream `looksLikeLocalFunction` requires an
/// identifier at the name slot, so a nameless `LocalFunctionElement` is
/// never minted for it and the unused-element face must not surface a
/// spurious `<unnamed>` (the registration arm exists for upstream parity
/// but stays silent on non-declaration shapes).
#[test]
fn anonymous_lambda_registers_no_unnamed_element() {
    let source = "void f() {\n  () => 0;\n}\n";
    let report = report_ops(source);
    let messages = code_messages(&report, "unused_element");
    assert!(
        messages.iter().all(|m| !m.contains("<unnamed>")),
        "a bare lambda mints no nameless element: {:#?}",
        rows(&report)
    );
}

/// A named local function still reports its spelling (regression).
#[test]
fn named_local_function_still_reports() {
    let source = "void f() {\n  int g() => 0;\n}\n";
    let report = report_ops(source);
    let messages = code_messages(&report, "unused_element");
    assert!(
        messages.iter().any(|m| m.contains("'g'")),
        "the named local function reports its spelling: {:#?}",
        rows(&report)
    );
}

// —— 1e: nameless catch parameter ——

/// A nameless catch exception slot (`on E catch (5, s)` — the `5` is
/// non-identifier junk, so `ensure_catch_parameter` splices a synthetic
/// name for the exception slot, identifier_context_impl.dart:36-38)
/// registers a real catch element upstream, not a dropped one. On an
/// `on`-typed clause the exception is not auto-used, so the oracle's
/// nameless row reports `unused_catch_clause` rendered `<unnamed>`.
#[test]
fn nameless_catch_exception_reports_unnamed() {
    let source = "void f() {\n  try {} on E catch (5, s) {}\n}\n";
    let report = report_ops(source);
    let messages = code_messages(&report, "unused_catch_clause");
    assert!(
        messages.iter().any(|m| m.contains("<unnamed>")),
        "the nameless exception slot reports `<unnamed>`: {:#?}",
        rows(&report)
    );
}
