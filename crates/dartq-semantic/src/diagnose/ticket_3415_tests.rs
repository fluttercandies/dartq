//! #3415 — the flow staging source for a call result read the wrong fact.
//!
//! The send close staged `callee_return_ty(row)` into the flow recorder
//! while every verdict over the same row reads
//! [`Engine::operand_static_ty`]. The two answers agree for a declared
//! callee but not for a row whose face the send close itself folded: an
//! invoked formal typed `int Function()` has its return (`int`) written
//! onto the callee row, so `intFunction() is int` typed `int` at the
//! verdict — and the oracle's unreachable `else` arm disappeared from the
//! report, because the staging read answered `Unknown` and staged nothing.
//!
//! The staging now reads [`Engine::operand_static_ty`], the one fact the
//! verdicts read, so the two cannot disagree by construction.
//!
//! The rider in the same ticket publishes a NAMED-selector creation's
//! argument-solved instantiation (`G.value(1)` over `class G<T>`) where the
//! created row's readers resolve it, so `callee_return_ty`'s
//! generic-constructor arm answers `G<int>` for a send that a solved call
//! site decided — and keeps `Unknown` for one it did not.

use super::test_rig::{CORE_PRELUDE, report_ops};
use super::*;

/// The unit source as the rig analyzed it (the prelude prefix included, so
/// diagnostic offsets index this string).
fn unit_source(body: &str) -> String {
    format!("{CORE_PRELUDE}{body}")
}

/// The UTF-16 slice of `[offset, offset + length)` over `source`.
fn utf16_slice(source: &str, offset: u32, length: u32) -> String {
    let end = offset + length;
    let mut out = String::new();
    let mut at = 0u32;
    for ch in source.chars() {
        let width = ch.len_utf16() as u32;
        if at >= end {
            break;
        }
        if at >= offset {
            out.push(ch);
        }
        at += width;
    }
    out
}

/// Every `code` row of `report` as `(offset, length, message)`.
fn rows_of(report: &FileReport, code: &str) -> Vec<(u32, u32, String)> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == code)
        .map(|d| (d.range.offset, d.range.length, d.message.to_string()))
        .collect()
}

/// The source text a row covers, over the prelude-prefixed unit.
fn row_text(source: &str, row: &(u32, u32, String)) -> String {
    utf16_slice(source, row.0, row.1)
}

// —— the staging/verdict agreement ——

/// A function-typed formal's call result is `int`, and the flow recorder
/// now sees it: the null comparison over a non-nullable result is constant
/// false, so the `then` arm is dead and the row anchors there. The pin
/// reads the anchored text, so a dead `else` arm — the opposite answer —
/// cannot satisfy it.
#[test]
fn function_typed_formal_call_result_reaches_the_flow_fold() {
    let body = "\
void f({required int Function() intFunction}) {
  if (intFunction() == null) {
    int deadThen = 0;
  } else {
    int liveElse = 0;
  }
}
";
    let source = unit_source(body);
    let report = report_ops(body);
    let dead = rows_of(&report, "dead_code");
    assert_eq!(dead.len(), 1, "rows: {:#?}", report.diagnostics);
    let text = row_text(&source, &dead[0]);
    assert!(
        text.contains("deadThen"),
        "the then arm must be the dead one, got {text:?} — rows: {:#?}",
        report.diagnostics
    );
}

/// The ticket's literal shape: the same call result types `int` (asserted
/// above), so `intFunction() is int` is a trivially satisfied check and
/// the oracle deads the `else` arm
/// (`trivially_satisfied_type_check_test.dart:37`).
///
/// The flow side this once parked on has landed (#3435): the `Ev::Send`
/// close drops the callee identifier's `reference`/`prop` slots, so
/// `Ev::IsEnd` takes the non-reference arm and the staged `int` folds the
/// else dead.
#[test]
fn function_typed_formal_call_result_deads_the_is_else_arm() {
    let body = "\
void f({required int Function() intFunction}) {
  if (intFunction() is int) {
    int liveThen = 0;
  } else {
    int deadElse = 0;
  }
}
";
    let source = unit_source(body);
    let report = report_ops(body);
    let dead = rows_of(&report, "dead_code");
    assert_eq!(dead.len(), 1, "rows: {:#?}", report.diagnostics);
    let text = row_text(&source, &dead[0]);
    assert!(
        text.contains("deadElse"),
        "the else arm must be the dead one, got {text:?} — rows: {:#?}",
        report.diagnostics
    );
}

/// The negative twin: the same shape with a callee whose return genuinely
/// cannot be decided (`T Function<T>(T)` — a generic function type's result
/// is the inference's, and the type variable is no named face). Nothing is
/// staged and neither arm dies, so no fabricated type ever reaches the
/// recorder and no type-check verdict fires.
#[test]
fn undecidable_call_result_deads_nothing() {
    let body = "\
void f<T>({required T Function<T>(T x) make}) {
  if (make(0) is int) {
    int liveThen = 0;
  } else {
    int liveElse = 0;
  }
}
";
    let report = report_ops(body);
    assert!(
        rows_of(&report, "dead_code").is_empty(),
        "rows: {:#?}",
        report.diagnostics
    );
    let checks: Vec<&'static str> = report
        .diagnostics
        .iter()
        .map(|d| d.code.name)
        .filter(|name| name.starts_with("unnecessary_type_check"))
        .collect();
    assert!(checks.is_empty(), "rows: {:#?}", report.diagnostics);
}

// —— the #3394 regression: a generic constructor send stays an instance ——

/// #3394's face, re-pinned after the staging unification: a generic head's
/// named-constructor send is a CREATED instance, so a cascade section
/// resolves its member on the created class.
#[test]
fn generic_constructor_send_result_keeps_the_created_instance() {
    let report = report_ops(
        "class G<T> { G.value(T t); void n() {} } \
         void f() { G.value(1)..n(); }",
    );
    assert!(
        report.diagnostics.is_empty(),
        "rows: {:#?}",
        report.diagnostics
    );
}

/// The miss side keeps naming the created class, never the `Type` literal
/// the bare head answers.
#[test]
fn generic_constructor_send_result_miss_names_the_class() {
    let report = report_ops(
        "class G<T> { G.value(T t); } \
         void f() { G.value(1)..gone(); }",
    );
    let rows = rows_of(&report, "undefined_method");
    assert_eq!(rows.len(), 1, "rows: {:#?}", report.diagnostics);
    assert!(
        rows[0].2.contains("isn't defined for the type 'G'."),
        "got {:?}",
        rows[0].2
    );
}

// —— the rider: the named-selector creation's published instantiation ——

/// `G.value(1)` solves `T := int` at the closed argument group, and the
/// solve publishes where `callee_return_ty`'s generic-constructor arm
/// resolves the class segment: the send types `G<int>`, so an unrelated
/// sink reports the mismatch naming the send result — never a raw head or a
/// to-bounds fill. The sink is deliberately NOT a `G<...>` context: a
/// `G<String>` sink seeds the creation downward and would report the
/// argument instead, which is a different face.
#[test]
fn named_constructor_solve_reaches_the_send_result() {
    let report = report_ops(
        "class G<T> { G.value(T t); } \
         void f(String s) { s = G.value(1); }",
    );
    let rows = rows_of(&report, "invalid_assignment");
    assert_eq!(rows.len(), 1, "rows: {:#?}", report.diagnostics);
    assert!(
        rows[0].2.contains("'G<int>'") && rows[0].2.contains("'String'"),
        "got {:?}",
        rows[0].2
    );
}

/// The agreeing twin: the published `G<int>` fits a `G<int>` sink, so the
/// send adds no row.
#[test]
fn named_constructor_solve_fits_its_own_instantiation() {
    let report = report_ops(
        "class G<T> { G.value(T t); } \
         void f() { G<int> g = G.value(1); }",
    );
    assert!(
        rows_of(&report, "invalid_assignment").is_empty(),
        "rows: {:#?}",
        report.diagnostics
    );
}

/// Nothing solvable — no argument constrains the class parameter — so
/// nothing publishes and the send keeps `Unknown`: an unknown source stages
/// no assignment row (the under-report face, pinned as hard as the
/// positive one).
#[test]
fn unsolved_named_constructor_stays_unknown() {
    let report = report_ops(
        "class G<T> { G.value(); } \
         void f(String s) { s = G.value(); }",
    );
    assert!(
        rows_of(&report, "invalid_assignment").is_empty(),
        "rows: {:#?}",
        report.diagnostics
    );
}

/// A STATIC member send on the same head is no creation: its declared
/// return types the send, and the class's parameters never enter the result.
#[test]
fn static_member_send_publishes_no_instantiation() {
    let report = report_ops(
        "class G<T> { static int make() => 0; } \
         void f(String s) { s = G.make(); }",
    );
    let rows = rows_of(&report, "invalid_assignment");
    assert_eq!(rows.len(), 1, "rows: {:#?}", report.diagnostics);
    assert!(
        rows[0].2.contains("'int'") && !rows[0].2.contains("'G<"),
        "got {:?}",
        rows[0].2
    );
}
