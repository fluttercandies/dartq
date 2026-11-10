//! The #3382 pins: the collection-literal write-face predicate
//! (`diagnose/collection_literal.rs`) and the promotion it gates.
//!
//! Oracle semantics: a list/set/map literal keeps its own inferred
//! `List`/`Set`/`Map` type, and the assignment is then decided against the
//! target's declared type (upstream Dart SDK source,
//! `pkg/analyzer/lib/src/dart/resolver/assignment_expression_resolver.dart`
//! — the write carries the assignment expression's static type). Where the
//! target's declared type *is* the literal's own head, the two coincide and
//! the write reads non-null, which is what promotes a nullable local. The
//! predicate answers exactly that coincidence question by spelling, so the
//! pins below observe it through the promotion: a promoted local accepts the
//! member call, an unpromoted one reports the nullable-receiver row.
//!
//! Two spellings used to be missed: the head behind a comment (`xs =
//! /* note */ [1]`) — the old probe skipped ASCII whitespace only — and the
//! head behind `const` once any non-ASCII character preceded the site in the
//! file (the probe sliced the source bytes at a UTF-16 offset, so the
//! `const` look-ahead drifted). Both now ride the token table, where the
//! scanner already dropped trivia.
//!
//! The element-typing faces of the recon's buckets I (null-aware elements
//! `?e`), H (spread element typing) and U (set element typing) are not owned
//! by this file: they need the literal's own element inference and, for U,
//! the write face's decision against it rather than against the target type.
//! Those stay residual against the owning lanes; the pins here fix the
//! predicate's contract so that change cannot silently regress it.

use super::test_rig::{CORE_PRELUDE, report_ops};
use crate::diagnose::FileReport;

/// The fixture offset of `needle`, shifted by the rig's core prelude (the
/// fixture rides behind it on one line).
fn at(source: &str, needle: &str) -> u32 {
    (CORE_PRELUDE.len() + source.find(needle).expect("needle in fixture")) as u32
}

/// Whether `code` fired anywhere in `report`.
fn reported(report: &FileReport, code: &str) -> bool {
    report
        .diagnostics
        .iter()
        .any(|diagnostic| diagnostic.code.name == code)
}

/// A list literal head at the write's operand position types as the
/// target's non-null declared type, so the write promotes the nullable
/// local and the following member call needs no assertion.
#[test]
fn ticket_3382_list_literal_head_promotes_the_target() {
    let source = r#"
void f() {
  List<int>? xs;
  xs = [1];
  xs.add(2);
}
"#;
    let report = report_ops(source);
    assert!(
        report.diagnostics.is_empty(),
        "the list literal promotes the write target: {:?}",
        report.diagnostics
    );
}

/// The same head for the set and the map spelling: `{` covers both, and the
/// ambiguity between them is resolved by the target type the predicate
/// hands the write — either way the argument row stays silent.
#[test]
fn ticket_3382_set_and_map_literal_heads_promote_the_target() {
    let source = r#"
void takeSet(Set<int> s) {}
void takeMap(Map<int, int> m) {}
void f() {
  Set<int>? s;
  s = {1};
  takeSet(s);
  Map<int, int>? m;
  m = {1: 2};
  takeMap(m);
}
"#;
    let report = report_ops(source);
    assert!(
        report.diagnostics.is_empty(),
        "the set and map heads promote the write target: {:?}",
        report.diagnostics
    );
}

/// The `<T>` head: `<int>[1]` starts with a type-argument group, not a
/// bracket, so the probe's third arm is what sees it.
#[test]
fn ticket_3382_type_argument_head_promotes_the_target() {
    let source = r#"
void f() {
  List<int>? xs;
  xs = <int>[1];
  xs.add(2);
}
"#;
    let report = report_ops(source);
    assert!(
        report.diagnostics.is_empty(),
        "the `<T>` head promotes the write target: {:?}",
        report.diagnostics
    );
}

/// The `const` head: `const` is trivia for the literal's static type, so the
/// probe steps over the keyword and still sees the bracket.
#[test]
fn ticket_3382_const_head_promotes_the_target() {
    let source = r#"
void f() {
  List<int>? xs;
  xs = const [1];
  xs.add(2);
}
"#;
    let report = report_ops(source);
    assert!(
        report.diagnostics.is_empty(),
        "the `const` head promotes the write target: {:?}",
        report.diagnostics
    );
}

/// A comment between the operator and the head is trivia, exactly as the
/// oracle treats it: the `const` look-ahead and the bracket test both read
/// the first *token*, never the next non-whitespace byte.
#[test]
fn ticket_3382_comment_before_the_head_still_promotes() {
    let source = r#"
void f() {
  List<int>? xs;
  xs = /* note */ [1];
  xs.add(2);
}
"#;
    let report = report_ops(source);
    assert!(
        report.diagnostics.is_empty(),
        "a comment before the head is trivia: {:?}",
        report.diagnostics
    );
}

/// The negative: a nullable operand is not a literal head, so the write
/// keeps the operand's own type and the local stays nullable — the member
/// call is the row the predicate must never suppress.
#[test]
fn ticket_3382_nullable_operand_does_not_promote() {
    let source = r#"
void f(List<int>? other) {
  List<int>? xs;
  xs = other;
  xs.add(2);
}
"#;
    let report = report_ops(source);
    assert!(
        reported(&report, "unchecked_use_of_nullable_value"),
        "a nullable operand keeps the write target nullable: {:?}",
        report.diagnostics
    );
    let call = at(source, "xs.add(2);");
    assert!(
        report
            .diagnostics
            .iter()
            .any(
                |diagnostic| diagnostic.code.name == "unchecked_use_of_nullable_value"
                    && diagnostic.range.offset == call + 3
            ),
        "the row anchors the `add` selector after `xs.`: {:?}",
        report.diagnostics
    );
}
