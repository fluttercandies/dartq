//! #3092 pins: the statement-level `illegalAssignmentToNonAssignable` rider
//! (`handleExpressionStatement`, ast_builder.dart:4209-4221) and the
//! constructor-initializer lane it must stay out of.
//!
//! Upstream reports the illegal row only for a STATEMENT whose whole
//! expression is an `AssignmentExpression` whose LHS is not assignable
//! (:4216-4221; the assignment arm itself only carries
//! `missingAssignableSelector`, :3824-3841). A bare-`this`/`super` target
//! inside a constructor initializer list is not a statement expression, so
//! no illegal row exists there: the corpus pin `issue_48411_prime_1` carries
//! only `missingAssignmentInInitializer`/`expected_token` for
//! `C() : this = 0;` and `missingAssignableSelector`/`invalidInitializer`
//! for `C() : super = 0;`.
//!
//! dartq's initializer-lane context flag is `Parser::assignment_op_spliced`
//! (parser.rs): the redirecting-initializer head recovery re-reads
//! `<synthetic> = this …` through the expression engine, and the arm keeps
//! that re-read's bare-`this` target selector-silent while every real target
//! keeps the row. The statement rider itself keys on the statement head +
//! assignment operator, so it can never fire from the initializer lane.
//!
//! The for-initializer split mirrors the same statement-context rule: the
//! loop's INIT clause parses through `parseExpression` +
//! `handleForInitializerExpressionStatement` (no expression statement, no
//! rider), while the CONDITION clause parses through
//! `parseExpressionStatement` (`parser_impl.dart:10318-10324`) and therefore
//! carries the rider.

use super::collector::{latest_features, scan_ok};

/// Parser rows of `source` as (code, start, end) UTF-16 triples, sorted.
fn sorted_rows(source: &'static str) -> Vec<(String, u32, u32)> {
    let (table, text) = scan_ok(source);
    let mut collector = super::collector::Collector::default();
    let mut parser = crate::Parser::new(text, table, latest_features(), &mut collector);
    parser.parse_unit();
    let mut rows = parser.listener.errors.clone();
    rows.sort();
    rows
}

fn code(c: &str, start: u32, end: u32) -> (String, u32, u32) {
    (c.to_string(), start, end)
}

fn mas(start: u32, end: u32) -> (String, u32, u32) {
    code("missing_assignable_selector", start, end)
}

fn illegal(start: u32, end: u32) -> (String, u32, u32) {
    code("illegal_assignment_to_non_assignable", start, end)
}

/// A real statement's bare-keyword target carries BOTH rows: the selector
/// row from the assignment arm and the illegal rider from the statement
/// (`test_left_super`, oracle 4:5|5; `this = 1` — the oracle's
/// `test_illegalAssignmentToNonAssignable_assign_this`).
#[test]
fn statement_keyword_target_carries_both_rows() {
    assert_eq!(
        sorted_rows("void f() { this = 1; }\n"),
        vec![illegal(11, 15), mas(11, 15)],
        "oracle: both rows anchor on the bare `this` target"
    );
    assert_eq!(
        sorted_rows("void f() { super = 1; }\n"),
        vec![illegal(11, 16), mas(11, 16)],
        "oracle: both rows anchor on the bare `super` target"
    );
}

/// `C() : this = 0;` — the redirecting-initializer recovery. The re-read
/// `<synthetic> = this = 0` builds a `this`-target assignment the oracle
/// never tracks: only the recovery faces remain (expected `.` at the `=`,
/// the missing-assignment row at `this`), never the selector or the illegal
/// rider (`issue_48411_prime_1` 16:9|4 + 16:14|1).
#[test]
fn initializer_this_target_stays_silent() {
    assert_eq!(
        sorted_rows("class A { A() : this = 0; }\n"),
        vec![
            code("expected_token", 21, 22),
            code("missing_assignment_in_initializer", 16, 20)
        ],
        "oracle: recovery rows only — no missing_assignable_selector, no \
         illegal_assignment_to_non_assignable"
    );
}

/// `C() : super = 0;` — the super-initializer dispatch keeps the selector
/// row (the arm's real target) and the invalid-initializer face, but no
/// illegal rider: the initializer lane has no statement (`issue_48411_prime_1`
/// 12:9|5 + 12:9|9). The invalid-initializer row anchors on the WHOLE entry
/// `super = 0` (16..25) since #3193, not on the bare `super` head.
#[test]
fn initializer_super_target_has_no_rider() {
    let rows = sorted_rows("class A { A() : super = 0; }\n");
    assert_eq!(
        rows,
        vec![code("invalid_initializer", 16, 25), mas(16, 21)],
        "oracle: selector + invalid-initializer rows only, never the rider"
    );
}

/// A REAL assignment inside an initializer RHS (`C() : x = this = 0;`) keeps
/// the arm's selector row — the enclosing `=` is real, not the recovery's
/// spliced one — and still carries no illegal rider (no statement context).
#[test]
fn initializer_rhs_assignment_keeps_selector_only() {
    assert_eq!(
        sorted_rows("class A { A() : x = this = 0; }\n"),
        vec![mas(20, 24)],
        "oracle: the real `this = 0` RHS keeps missing_assignable_selector; \
         the rider is statement-only"
    );
}

/// The for-init / for-condition split: the INIT clause is not an expression
/// statement upstream (`parseExpression` +
/// `handleForInitializerExpressionStatement`), so only the arm's selector row
/// fires; the CONDITION clause goes through `parseExpressionStatement`, so it
/// carries both rows.
#[test]
fn for_clause_statement_context_split() {
    assert_eq!(
        sorted_rows("void f() { for (this = 0;;) {} }\n"),
        vec![mas(16, 20)],
        "oracle: the for-initializer clause has no expression-statement rider"
    );
    assert_eq!(
        sorted_rows("void f() { for (; this = 0;) {} }\n"),
        vec![illegal(18, 22), mas(18, 22)],
        "oracle: the for-condition clause is an expression statement — both rows"
    );
}
