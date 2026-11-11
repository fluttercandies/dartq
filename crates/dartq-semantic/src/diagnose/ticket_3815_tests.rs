//! #3815 — `armed_ty_span` anchors the type HEAD only (`List` for
//! `List<B>`), while the oracle's `checkPatternNeverMatchesValueType`
//! anchors `pattern.type` — the WHOLE written type node (upstream Dart SDK
//! source, pkg/analyzer/lib/src/generated/resolver.dart:624-653: the
//! Cast/DeclaredVariable/Object/Wildcard kinds all report
//! `.at(pattern.type)`). The latch widens closure-driven: the type-args
//! group end joins the armed head at `handle_type`'s re-fired-group
//! extent, the record close extends it to `(`..`)` (+`?`), and the
//! function-type close extends it from the return head through the
//! formals' closer (a bare `Function()` arms from the keyword — no head
//! ever completed).
//!
//! Oracle provenance: `dart analyze --format=machine` (3.13.7 SDK) on the
//! standalone switch/cast fixtures these sources mirror —
//! `PATTERN_NEVER_MATCHES_VALUE_TYPE …|4|14|7` (`List<B>`), `…|7|14|11`
//! (`FutureOr<B>`), `…|3|19|3` (`_ as int`, engine-core-owned anchor —
//! out of scope here), `…|3|14|9` (`(String,)`), `…|3|14|15`
//! (`void Function()`), `…|3|14|10` (`Function()`), `UNNECESSARY_CAST
//! …|2|3|14`, `…|2|3|20`, `INVALID_FIELD_NAME …|1|14|2`.
//!
//! The pinned reader guards: a trailing `?` already spans past the group
//! (`List<B>?` is green at HEAD), `unnecessary_cast_pattern` anchors the
//! `as` token (upstream shared_type_analyzer.dart:151), the is-verdict
//! span takes the max over the armed/group/written ends, and the record
//! field-names walk reads the last field's head span at the record's own
//! close — the widened closures must never move those.

use crate::diagnose::FileReport;
use crate::diagnose::test_rig::{CORE_PRELUDE, report_ops, rows};

/// The absolute fixture offset of `needle` (the rig's prelude included).
fn at(fixture: &str, needle: &str) -> usize {
    CORE_PRELUDE.len() + fixture.find(needle).unwrap()
}

/// Exactly one `code` row anchored at `offset` with `length`, carrying
/// `message` — the oracle's `(code, offset, length, message)` contract.
#[track_caller]
fn assert_row(report: &FileReport, code: &str, offset: usize, length: u32, message: &str) {
    let hits: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| {
            d.code.name == code && d.range.offset as usize == offset && d.range.length == length
        })
        .collect();
    assert_eq!(
        hits.len(),
        1,
        "expected one {code} row at {offset}+{length}: {:#?}",
        rows(report)
    );
    assert_eq!(hits[0].message, message, "{:#?}", rows(report));
}

const NEVER_MATCHES: &str = "pattern_never_matches_value_type";

/// `case List<B> _` over an `int` scrutinee: the required type's whole
/// written node (`List<B>`, 7 columns), not the bare head. Oracle
/// `…|4|14|7`. **Red at HEAD**: the latch stops at `List`.
#[test]
fn wildcard_type_args_anchor_full_node() {
    let src = "class B {}\n\
               void f(int x) {\n\
                 switch (x) {\n\
                   case List<B> _:\n\
                     break;\n\
                 }\n\
               }\n";
    let report = report_ops(src);
    assert_row(
        &report,
        NEVER_MATCHES,
        at(src, "List<B>"),
        7,
        "The matched value type 'int' can never match the required type 'List<B>'.",
    );
}

/// `case FutureOr<B> _`: the group rides the armed `FutureOr` head the
/// same way. Oracle `…|7|14|11`. **Red at HEAD** (`|8`).
#[test]
fn future_or_type_args_anchor_full_node() {
    let src = "import 'dart:async';\n\
               \n\
               class B {}\n\
               \n\
               void f(int x) {\n\
                 switch (x) {\n\
                   case FutureOr<B> _:\n\
                     break;\n\
                 }\n\
               }\n";
    let report = report_ops(src);
    assert_row(
        &report,
        NEVER_MATCHES,
        at(src, "FutureOr<B>"),
        11,
        "The matched value type 'int' can never match the required type 'FutureOr<B>'.",
    );
}

/// `case List<B> y`: the DeclaredVariable kind anchors `pattern.type` the
/// same way (resolver.dart:637-639). Oracle `…|4|14|7`. **Red at HEAD**.
#[test]
fn declared_type_args_anchor_full_node() {
    let src = "class B {}\n\
               void f(int x) {\n\
                 switch (x) {\n\
                   case List<B> y:\n\
                     break;\n\
                 }\n\
               }\n";
    let report = report_ops(src);
    assert_row(
        &report,
        NEVER_MATCHES,
        at(src, "List<B>"),
        7,
        "The matched value type 'int' can never match the required type 'List<B>'.",
    );
}

/// `case (String,) _`: the record close extends the latch from the last
/// field's head to the whole `(String,)` node. Oracle `…|3|14|9`.
/// **Red at HEAD** (`3:15|6` — the last field leaf).
#[test]
fn record_type_anchor_full_node() {
    let src = "void f(int x) {\n\
                 switch (x) {\n\
                   case (String,) _:\n\
                     break;\n\
                 }\n\
               }\n";
    let report = report_ops(src);
    assert_row(
        &report,
        NEVER_MATCHES,
        at(src, "(String,)"),
        9,
        "The matched value type 'int' can never match the required type '(String,)'.",
    );
}

/// `case void Function() _`: the function-type close extends the latch
/// from the `void` return head through the formals' `)`. Oracle
/// `…|3|14|15`. **Red at HEAD** (`|4` — the return head alone).
#[test]
fn function_type_anchor_full_node() {
    let src = "void f(int x) {\n\
                 switch (x) {\n\
                   case void Function() _:\n\
                     break;\n\
                 }\n\
               }\n";
    let report = report_ops(src);
    assert_row(
        &report,
        NEVER_MATCHES,
        at(src, "void Function()"),
        15,
        "The matched value type 'int' can never match the required type 'void Function()'.",
    );
}

/// `case Function() _`: no return head ever completes, so the close arms
/// the latch from the `Function` keyword itself. Oracle `…|3|14|10`.
/// **Red at HEAD** (the bare `_` fallback).
#[test]
fn bare_function_type_anchor_from_keyword() {
    let src = "void f(int x) {\n\
                 switch (x) {\n\
                   case Function() _:\n\
                     break;\n\
                 }\n\
               }\n";
    let report = report_ops(src);
    assert_row(
        &report,
        NEVER_MATCHES,
        at(src, "Function()"),
        10,
        "The matched value type 'int' can never match the required type 'dynamic Function()'.",
    );
}

/// `x as List<int>`: `visitAsExpression`'s `.at(node)` spans the operand
/// through the WRITTEN cast type (best_practices_verifier.dart:177-186) —
/// the armed extent rides the cast anchor. Oracle `UNNECESSARY_CAST
/// …|2|3|14`. **Red at HEAD** (`|9` — stops at `List`).
#[test]
fn as_expression_type_args_anchor_full_node() {
    let src = "void f(List<int> x) {\n\
                 x as List<int>;\n\
               }\n";
    let report = report_ops(src);
    assert_row(
        &report,
        "unnecessary_cast",
        at(src, "x as"),
        14,
        "Unnecessary cast.",
    );
}

/// `g as void Function()`: the same cast anchor through the function
/// type's written extent. Oracle `UNNECESSARY_CAST …|2|3|20`.
/// **Red at HEAD** (`|9` — stops at `void`).
#[test]
fn as_expression_function_type_anchor_full_node() {
    let src = "void f(void Function() g) {\n\
                 g as void Function();\n\
               }\n";
    let report = report_ops(src);
    assert_row(
        &report,
        "unnecessary_cast",
        at(src, "g as"),
        20,
        "Unnecessary cast.",
    );
}

// —— reader guards: the widening must never move these ——————————————————

/// `case List<B>? _`: a trailing `?` already spans past the group at the
/// head's own completion — the longer face wins and the widened group end
/// must not disturb it. Oracle `…|4|14|8`. Green at HEAD.
#[test]
fn nullable_wildcard_keeps_question_extent() {
    let src = "class B {}\n\
               void f(int x) {\n\
                 switch (x) {\n\
                   case List<B>? _:\n\
                     break;\n\
                 }\n\
               }\n";
    let report = report_ops(src);
    assert_row(
        &report,
        NEVER_MATCHES,
        at(src, "List<B>?"),
        8,
        "The matched value type 'int' can never match the required type 'List<B>?'.",
    );
}

/// `case _ as int` over an `int` scrutinee: `unnecessary_cast_pattern`
/// anchors `pattern.asToken` (upstream shared_type_analyzer.dart:151) —
/// never the written type. Oracle `…|3|16|2`. Green at HEAD.
#[test]
fn unnecessary_cast_pattern_keeps_as_token() {
    let src = "void f(int x) {\n\
                 switch (x) {\n\
                   case _ as int:\n\
                     break;\n\
                 }\n\
               }\n";
    let report = report_ops(src);
    assert_row(
        &report,
        "unnecessary_cast_pattern",
        at(src, "as int"),
        2,
        "Unnecessary cast pattern.",
    );
}

/// `x is List<int>`: the is-verdict span takes the max over the armed,
/// group-end, and written ends — the widened armed end never shrinks it.
/// Oracle `UNNECESSARY_TYPE_CHECK …|2|7|14`. Green at HEAD.
#[test]
fn unnecessary_type_check_keeps_whole_node() {
    let src = "void f(List<int> x) {\n\
                 if (x is List<int>) {}\n\
               }\n";
    let report = report_ops(src);
    assert_row(
        &report,
        "unnecessary_type_check",
        at(src, "x is"),
        14,
        "Unnecessary type check; the result is always 'true'.",
    );
}

/// A named record type whose last field is a function type: the
/// field-names walk reads the last field's HEAD span at the record's own
/// close — the function close's widening is gated off inside record types
/// so the walk still finds the record's `(`. Oracle
/// `INVALID_FIELD_NAME …|1|14|2`. Green at HEAD.
#[test]
fn record_field_names_walk_survives_function_field() {
    let src = "void f(({int _a, void Function() b}) r) {}\n";
    let report = report_ops(src);
    assert_row(
        &report,
        "invalid_field_name",
        at(src, "_a"),
        2,
        "Record field names can't be private.",
    );
}

/// The same walk guard with a record-typed last field: the nested
/// record's close also stays gated. Oracle `INVALID_FIELD_NAME …|1|14|2`.
/// Green at HEAD.
#[test]
fn record_field_names_walk_survives_record_field() {
    let src = "void f(({int _a, (int, int) b}) r) {}\n";
    let report = report_ops(src);
    assert_row(
        &report,
        "invalid_field_name",
        at(src, "_a"),
        2,
        "Record field names can't be private.",
    );
}
