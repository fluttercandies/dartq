//! #2990 pins: the `?[`/`?..[` null-aware index faces — the operator
//! anchor and the member-receiver continuation. Upstream
//! `visitIndexExpression` (`error_verifier.dart` :1370-1390) hands
//! `_checkForUnnecessaryNullAware` the index's `realTarget` — never the
//! bracket extent — and anchors the row at the operator token: the `?`
//! runs through its `next` token's end (`?[` = 2 columns, `? [` = 3), while
//! the cascade `?..` reports its own 3-column token. A following `?.`/`?[`
//! over the index result reads the PRE-LIFT element type — the index
//! node's own `staticType` is `E` (`visitIndexExpression` records the `[]`
//! return; only the flow-lifted face reads `E?`), so a non-nullable `E`
//! makes the operator dead — and takes the `after_short_circuit` variant
//! whose context is the index's `?` token (1 column —
//! `previousShortCircuitingOperator`'s IndexExpression arm answers the
//! question token, :7314-7318). Every expectation below is a verbatim
//! oracle 3.13.7 row.

use super::super::test_rig::report_ops_bare;
use super::super::types::FileReport;

/// The `invalid_null_aware_operator` rows of `report_ops_bare`, as
/// (offset, length, message) triples.
fn rows(source: &str) -> Vec<(u32, u32, String)> {
    null_aware(&report_ops_bare(source))
}

fn null_aware(report: &FileReport) -> Vec<(u32, u32, String)> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "invalid_null_aware_operator")
        .map(|d| (d.range.offset, d.range.length, d.message.to_string()))
        .collect()
}

/// The (offset, length, message) rows of every context message the
/// `invalid_null_aware_operator` diagnostics carry — the short-circuit
/// variant's "causing operator" location.
fn contexts(source: &str) -> Vec<(u32, u32, String)> {
    report_ops_bare(source)
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "invalid_null_aware_operator")
        .flat_map(|d| {
            d.context
                .iter()
                .flat_map(|set| set.iter())
                .map(|c| (c.range.offset, c.range.length, c.message.to_string()))
        })
        .collect()
}

const PLAIN_INDEX: &str =
    "The receiver can't be null, so the null-aware operator '?[' is unnecessary.";
const PLAIN_CASCADE: &str =
    "The receiver can't be null, so the null-aware operator '?..' is unnecessary.";
const SHORT_INDEX: &str =
    "The receiver can't be 'null' because of short-circuiting, so the null-aware operator '?[' can't be used.";
const SHORT_ACCESS: &str =
    "The receiver can't be 'null' because of short-circuiting, so the null-aware operator '?.' can't be used.";
const QUESTION_CAUSE: &str = "The operator '?' is causing the short circuiting.";

/// `x?[i]` anchors the operator pair: the `?` token's start through the
/// `[` token's end (2 columns), not the receiver and not the brackets.
#[test]
fn index_operator_anchor_is_question_through_bracket() {
    let source = "f(List<int> x) {\n  x?[0];\n}\n";
    assert_eq!(rows(source), vec![(20, 2, PLAIN_INDEX.to_string())]);
}

/// A space between the two operator tokens widens the span: the anchor
/// still runs `?` through `[`'s end (oracle `issue_39723` line 10:
/// `a! ? [42]` reports 3 columns).
#[test]
fn spaced_index_operator_anchor_covers_the_gap() {
    let source = "f(List<int> x) {\n  x ? [0];\n}\n";
    assert_eq!(rows(source), vec![(21, 3, PLAIN_INDEX.to_string())]);
}

/// `x ?[i]` (space before the pair only) keeps the 2-column anchor: the
/// gap precedes the `?`, so it is outside the operator span.
#[test]
fn leading_space_stays_outside_the_anchor() {
    let source = "f(List<int> x) {\n  x ?[0];\n}\n";
    assert_eq!(rows(source), vec![(21, 2, PLAIN_INDEX.to_string())]);
}

/// `x?..[i]`'s cascade section reports the `?..` token (3 columns) and
/// the cascade lexeme — the index brackets are not part of the anchor.
#[test]
fn cascade_index_anchor_is_the_cascade_operator() {
    let source = "f(List<int> x) {\n  x ?..[0];\n}\n";
    assert_eq!(rows(source), vec![(21, 3, PLAIN_CASCADE.to_string())]);
}

/// `a?[i]?.b`: the member receiver is the index RESULT, whose pre-lift
/// element type (`int`) is strictly non-nullable — the `?.` reports the
/// `after_short_circuit` variant and carries the index's `?` (1 column)
/// as the causing operator.
#[test]
fn member_after_index_reads_the_pre_lift_element() {
    let source = "f(List<int> x) {\n  x?[0]?.isEven;\n}\n";
    assert_eq!(
        rows(source),
        vec![
            (20, 2, PLAIN_INDEX.to_string()),
            (24, 2, SHORT_ACCESS.to_string()),
        ]
    );
    assert_eq!(
        contexts(source),
        vec![(20, 1, QUESTION_CAUSE.to_string())]
    );
}

/// The same law over a NULLABLE receiver: the `?[` itself is legal (a
/// nullable list is exactly what it exists for), while the `?.` over its
/// result reports — its receiver is non-null only because the index
/// shorted it. The causing operator is the index's `?` token (1 column),
/// never `?.` (`previousShortCircuitingOperator`'s IndexExpression arm
/// answers `target.question`, :7314-7318).
#[test]
fn nullable_receiver_index_short_circuit_uses_question_context() {
    let source = "f(List<int>? y) {\n  y?[0]?.isEven;\n}\n";
    assert_eq!(rows(source), vec![(25, 2, SHORT_ACCESS.to_string())]);
    assert_eq!(
        contexts(source),
        vec![(21, 1, QUESTION_CAUSE.to_string())]
    );
}

/// `a?[0]?.b` over a class element type: the `?.` reads the pre-lift `B`.
#[test]
fn member_after_class_index_reports() {
    let source = "class B {\n  List<int> b = [0];\n}\nf(List<B> a) {\n  a?[0]?.b;\n}\n";
    assert_eq!(
        rows(source),
        vec![
            (51, 2, PLAIN_INDEX.to_string()),
            (55, 2, SHORT_ACCESS.to_string()),
        ]
    );
    assert_eq!(
        contexts(source),
        vec![(51, 1, QUESTION_CAUSE.to_string())]
    );
}

/// `a?[0].b?[1]`: the plain `.` between the links breaks the
/// `previousShortCircuitingOperator` walk, so the second `?[` takes the
/// PLAIN message — each `?[`'s receiver is the preceding link's result,
/// and the anchor stays that operator's own pair.
#[test]
fn plain_link_breaks_the_short_circuit_walk() {
    let source = "class B {\n  List<int> b = [0];\n}\nf(List<B> a) {\n  a?[0].b?[1];\n}\n";
    assert_eq!(
        rows(source),
        vec![
            (51, 2, PLAIN_INDEX.to_string()),
            (57, 2, PLAIN_INDEX.to_string()),
        ]
    );
    assert_eq!(contexts(source), Vec::<(u32, u32, String)>::new());
}

/// `.b` on an index result is null-shorted: no
/// `unchecked_use_of_nullable_value` even though the flow face reads
/// `B?` — the continuation inherits the `?[`'s lift.
#[test]
fn plain_continuation_is_null_shorted() {
    let source = "class B {\n  List<int> b = [0];\n}\nf(List<B> a) {\n  a?[0].b;\n}\n";
    let report = report_ops_bare(source);
    assert_eq!(
        null_aware(&report),
        vec![(51, 2, PLAIN_INDEX.to_string())]
    );
    assert!(
        !report
            .diagnostics
            .iter()
            .any(|d| d.code.name == "unchecked_use_of_nullable_value"),
        "{report:?}"
    );
}

/// `x?[i]?[j]`: the second index's receiver is the first's result — a
/// non-nullable `int` — so it reports the short-circuit variant with the
/// first `?` as the causing operator.
///
/// BLOCKED (out of this lane's file scope): the second `?[` is judged by
/// the listener's index arm, which types its receiver from the folded
/// result row (`E?`). Feeding it the pre-lift element (`E`) — the same
/// `Engine::shorted_index_elem_ty` answer the member face reads in
/// `invocation.rs` — is the one-line hook that turns this pin green; the
/// expectation below is the verbatim oracle 3.13.7 row pair.
#[test]
#[ignore = "listener index arm types the chained receiver from the lifted row face"]
fn chained_index_reports_short_circuit_variant() {
    let source = "f(List<int> x) {\n  x?[0]?[1];\n}\n";
    assert_eq!(
        rows(source),
        vec![
            (20, 2, PLAIN_INDEX.to_string()),
            (24, 2, SHORT_INDEX.to_string()),
        ]
    );
}

/// `a?[i]?[j]` over nested lists anchors each operator pair at its own
/// `?[` — the receiver walks back exactly one link. Same blocked hook as
/// [`chained_index_reports_short_circuit_variant`].
#[test]
#[ignore = "listener index arm types the chained receiver from the lifted row face"]
fn nested_chain_walks_back_one_link() {
    let source = "f(List<List<int>> a) {\n  a?[0]?[0];\n}\n";
    assert_eq!(
        rows(source),
        vec![
            (26, 2, PLAIN_INDEX.to_string()),
            (30, 2, SHORT_INDEX.to_string()),
        ]
    );
}

/// `x?[i]![j]`: the `!` is not a null-aware operator, so the index pair
/// after it reports nothing — only the `?[` itself is dead.
#[test]
fn non_null_assert_link_reports_only_the_index() {
    let source = "f(List<int> x) {\n  x?[0]![0];\n}\n";
    assert_eq!(rows(source), vec![(20, 2, PLAIN_INDEX.to_string())]);
}

/// A nullable ELEMENT (`List<int?>`) keeps the `?.` legal: the pre-lift
/// element face is `int?`, exactly what the operator exists for.
#[test]
fn nullable_element_keeps_the_member_legal() {
    let source = "f(List<int?> x) {\n  x?[0]?.isEven;\n}\n";
    assert_eq!(rows(source), vec![(21, 2, PLAIN_INDEX.to_string())]);
}

/// `x?[i].m` with a non-nullable element: only the `?[` reports, and the
/// continuation raises no nullable-dereference row.
#[test]
fn non_nullable_element_continuation_is_quiet() {
    let source = "f(List<int> x) {\n  x?[0].isEven;\n}\n";
    let report = report_ops_bare(source);
    assert_eq!(
        null_aware(&report),
        vec![(20, 2, PLAIN_INDEX.to_string())]
    );
    assert!(
        !report
            .diagnostics
            .iter()
            .any(|d| d.code.name == "unchecked_use_of_nullable_value"),
        "{report:?}"
    );
}

/// A nullable receiver keeps `?[` legal — the access face is silent.
#[test]
fn nullable_receiver_index_is_quiet() {
    let source = "f(List<int>? y) {\n  y?[0];\n}\n";
    assert_eq!(rows(source), Vec::<(u32, u32, String)>::new());
}

/// The `?..[` cascade on a nullable receiver is legal as well.
#[test]
fn nullable_receiver_cascade_index_is_quiet() {
    let source = "f(List<int>? y) {\n  y?..[0];\n}\n";
    assert_eq!(rows(source), Vec::<(u32, u32, String)>::new());
}
