//! #3148 fold-in (batch B) pins: the `prefer_is_empty` report span against
//! the pinned `dart analyze` 3.13.7 oracle.
//!
//! The official visitor reports the whole binary expression
//! (`rule.reportAtNode(expression)`, `pkg/linter/lib/src/rules/
//! prefer_is_empty.dart:64-146`), so the row opens at the expression's
//! first token. dartq anchored it at the left operand's delta-tile member
//! row instead, which starts at the member name — `[].length == -1` reported
//! at `length` (column 12) where the oracle reports at `[]` (column 9). The
//! pins below fix the anchor and fence the shapes that were already right.

use crate::testsupport::{Fixture, one_rule, tok, utf16_at};

/// One row for `source`, anchored at the `nth` `needle` token, with
/// `problem` as its message.
fn assert_row(source: &str, needle: &str, nth: usize, problem: &str) {
    let f = Fixture::parse(source);
    let lints = f.run(&one_rule("prefer_is_empty"));
    assert_eq!(lints.len(), 1, "{source:?}: {lints:?}");
    assert_eq!(lints[0].message, problem, "{source:?}");
    assert_eq!(
        lints[0].range.start,
        utf16_at(&f, tok(&f.source, &f.tokens, needle, nth)),
        "{source:?}"
    );
}

const ALWAYS_FALSE: &str = "The comparison is always 'false' because the length is always \
                            greater than or equal to 0.";
const USE_IS_EMPTY: &str =
    "Use 'isEmpty' instead of 'length' to test whether the collection is empty.";
const USE_IS_NOT_EMPTY: &str =
    "Use 'isNotEmpty' instead of 'length' to test whether the collection is empty.";

/// Oracle 3.13.7 (upstream `test_alwaysFalse_length`): the row opens at
/// `[]`, not at `length`.
#[test]
fn pist_empty_negative_length_anchors_at_the_receiver() {
    assert_row(
        "void f() { var x = [].length == -1; }",
        "[]",
        0,
        ALWAYS_FALSE,
    );
}

/// Oracle 3.13.7 (`test_preferIsEmpty_length`, `== 0`).
#[test]
fn pist_zero_comparison_anchors_at_the_receiver() {
    assert_row(
        "void f() { var x = [].length == 0; }",
        "[]",
        0,
        USE_IS_EMPTY,
    );
}

/// Oracle 3.13.7 (`test_preferIsNotEmpty_length`, `> 0`).
#[test]
fn pist_not_empty_comparison_anchors_at_the_receiver() {
    assert_row(
        "void f() { var x = [].length > 0; }",
        "[]",
        0,
        USE_IS_NOT_EMPTY,
    );
}

/// A non-empty list literal receiver: the walk crosses the literal's own
/// bracket group back to its opener (oracle: the row opens at `[`).
#[test]
fn pist_anchors_at_a_non_empty_list_literal() {
    assert_row(
        "void f() { var x = [1, 2].length == 0; }",
        "[",
        0,
        USE_IS_EMPTY,
    );
}

/// A string receiver (`String` is one of the three core families,
/// `prefer_is_empty.dart:229-234`).
#[test]
fn pist_anchors_at_a_string_receiver() {
    assert_row(
        "void f() { var x = 'abc'.length > 0; }",
        "'abc'",
        0,
        USE_IS_NOT_EMPTY,
    );
}

/// The declaration face must not walk past the operand into the
/// declaration's own tokens (`final`/`y`/`=` are boundaries).
#[test]
fn pist_does_not_anchor_at_the_declaration() {
    assert_row(
        "void f() { final y = [].length != 0; }",
        "[]",
        0,
        USE_IS_NOT_EMPTY,
    );
}

/// A `length` read with no comparison has no row at all.
#[test]
fn pist_ignores_a_bare_length_read() {
    let f = Fixture::parse("void f() { var x = [].isEmpty; }");
    assert!(f.run(&one_rule("prefer_is_empty")).is_empty());
}
