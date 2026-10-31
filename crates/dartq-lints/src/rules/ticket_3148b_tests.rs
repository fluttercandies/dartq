//! #3148 fold-in pins against the pinned `dart analyze` 3.13.7 oracle:
//! batch B's `prefer_is_empty` report span, batch C's `prefer_final_fields`
//! write-scan faces and `unnecessary_statements` prefix faces.
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

// —— prefer_final_fields: the write-scan faces ————————————————————————
// Upstream `pkg/linter/lib/src/rules/prefer_final_fields.dart`: the mutated
// set is every `writeElement` of the field (assignment targets whatever the
// receiver, `++`/`--` operands); a constructor initializer-list write
// (`C() : _x = 7`) is not one.

/// Oracle 3.13.7 (`test_instanceFieldWrite` shape): `c._x = 42` mutates the
/// field, so the field stays non-final and silent.
#[test]
fn pff_instance_qualified_write_is_a_mutation() {
    let f = Fixture::parse(
        "class C {\n  int _x = 0;\n}\n\nvoid f() {\n  var c = C();\n  c._x = 42;\n}\n",
    );
    assert!(f.run(&one_rule("prefer_final_fields")).is_empty());
}

/// `_x++` / `--_x` mutate the field: the bare name completes as a send
/// group, whose head is the write target.
#[test]
fn pff_increment_and_decrement_are_mutations() {
    for source in [
        "class C { int _x = 1; void f() { _x++; } }",
        "class C { int _x = 1; void f() { --_x; } }",
    ] {
        let f = Fixture::parse(source);
        assert!(
            f.run(&one_rule("prefer_final_fields")).is_empty(),
            "{source:?}"
        );
    }
}

/// Oracle 3.13.7 (`test_initializedInConstructor` shape): a field written
/// only in a constructor initializer-list can be `final`, so it reports.
#[test]
fn pff_initializer_list_write_still_reports() {
    let f = Fixture::parse("class C {\n  int _x;\n  C() : _x = 7;\n}\n");
    let lints = f.run(&one_rule("prefer_final_fields"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(lints[0].message, "The private field _x could be 'final'.");
}

// —— unnecessary_statements: the prefix faces ————————————————————————
// Upstream `pkg/linter/lib/src/rules/unnecessary_statements.dart:114-116`:
// `super.visitPrefixExpression` routes to `visitNode(node)`, so every prefix
// except `++`/`--` reports at the whole prefix expression.

/// One row for `source`, anchored at the `nth` `needle` token.
fn assert_statement_row(source: &str, needle: &str, nth: usize) {
    let f = Fixture::parse(source);
    let lints = f.run_semantic(&one_rule("unnecessary_statements"));
    assert_eq!(lints.len(), 1, "{source:?}: {lints:?}");
    assert_eq!(lints[0].message, "Unnecessary statement.", "{source:?}");
    assert_eq!(
        lints[0].range.start,
        utf16_at(&f, tok(&f.source, &f.tokens, needle, nth)),
        "{source:?}"
    );
}

/// Oracle 3.13.7: `INFO|…|3:3|2` for `~1;`.
#[test]
fn us_prefix_tilde_statement_reports() {
    assert_statement_row("void f() {\n  ~1;\n}\n", "~", 0);
}

/// Oracle 3.13.7: `INFO|…|3:3|9` for `!x.isEven;` (the prefix's own leaf
/// seals last with the operator alone, so the containment root walk fails
/// without the prefix fallback).
#[test]
fn us_prefix_bang_statement_reports() {
    assert_statement_row("void f(int x) {\n  !x.isEven;\n}\n", "!", 0);
}

/// Oracle 3.13.7: `INFO|…|3:3|2` for `-x;`.
#[test]
fn us_prefix_minus_statement_reports() {
    assert_statement_row("void f(int x) {\n  -x;\n}\n", "-", 0);
}

/// `++`/`--` have a clear effect (`:141-147`) and stay silent.
#[test]
fn us_prefix_increment_stays_silent() {
    let f = Fixture::parse("void f(int x) {\n  ++x;\n}\n");
    assert!(
        f.run_semantic(&one_rule("unnecessary_statements"))
            .is_empty(),
        "postfix/prefix increments are clear effects"
    );
}
