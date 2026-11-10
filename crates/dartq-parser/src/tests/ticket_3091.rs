//! #3091 pins: the statement/declaration wrapper-selector faces of the
//! `reportErrorIfSuper` family (upstream Dart SDK source,
//! `pkg/analyzer/lib/src/fasta/ast_builder.dart`) that live outside the
//! expression engine: the bare-`super` expression statement (:4202), the
//! statement-level `illegalAssignmentToNonAssignable` rider on a bare
//! keyword assignment target (:4209-4221), the variable-initializer value
//! (:3459, shared with the member field flavor), the arrow function body
//! (:4179), the for-in iterable (:4282), the parenthesized condition of
//! if/while/do/switch statements and switch expressions (:5229), and the
//! bare-`super` receiver of a standalone type-argument group
//! (:5608 `handleTypeArgumentApplication`).
//!
//! Silence faces pinned alongside: binary statement expressions stay
//! silent (`super == 3;`, `super + 1;` — the statement wrapper is no longer
//! a bare `super` and the binary face checks the right operand only),
//! `this;`/`var x = this;` carry no syntactic rows, the for-initializer
//! bare `super` is silent (`for (super;;)` — no handler in the
//! for-initializer chain), `return super;`/`throw super;` have no
//! `reportErrorIfSuper` caller, and the invoked type-args flavor
//! (`super<int>(3)`) checks its arguments instead.

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

/// The bare-`super` statement face (`super;`) plus the statement-level
/// assignment faces: the `this` target's selector row and the
/// `illegalAssignmentToNonAssignable` rider for both keyword targets
/// (oracle `super = 3;`/`this = 3;` both anchor the rider on the bare
/// keyword token), the compound forms (`+=`, `??=`), and the branch
/// position (`if (c) super = 3;`).
#[test]
fn expression_statement_faces_match_oracle() {
    assert_eq!(
        sorted_rows("void f() { super; }\n"),
        vec![mas(11, 16)],
        "oracle: the bare-super statement is itself the selector-missing face"
    );
    assert_eq!(
        sorted_rows("void f() { super = 3; }\n"),
        vec![illegal(11, 16), mas(11, 16)],
        "oracle: selector from the assignment arm, rider from the statement"
    );
    assert_eq!(
        sorted_rows("void f() { this = 3; }\n"),
        vec![illegal(11, 15), mas(11, 15)],
        "oracle: both rows on the `this` target; the statement context is \
         what separates them from the redirecting-initializer lane"
    );
    assert_eq!(
        sorted_rows("void f() { super += 3; }\n"),
        vec![illegal(11, 16), mas(11, 16)],
        "oracle: the compound assignment keeps both faces"
    );
    assert_eq!(
        sorted_rows("void f() { super ??= 3; }\n"),
        vec![illegal(11, 16), mas(11, 16)],
        "oracle: `??=` rides the same assignment face"
    );
    assert_eq!(
        sorted_rows("void f() { if (c) super = 3; }\n"),
        vec![illegal(18, 23), mas(18, 23)],
        "oracle: the branch statement carries both faces"
    );
    assert_eq!(
        sorted_rows("void f() { while (c) super; }\n"),
        vec![mas(21, 26)],
        "oracle: the loop-body statement carries the selector row"
    );
}

/// Wrapper statements stay silent: the statement wrapper face fires only
/// when the WHOLE statement expression is the bare `super`; binary
/// wrappers, `this;`, and the no-handler statements
/// (`return`/`throw`/for-initializer) carry no syntactic rows.
#[test]
fn wrapper_statement_silence_faces_match_oracle() {
    assert_eq!(
        sorted_rows("void f() { super == 3; }\n"),
        Vec::<(String, u32, u32)>::new(),
        "oracle: a binary statement expression is not a bare super"
    );
    assert_eq!(
        sorted_rows("void f() { super + 1; }\n"),
        Vec::<(String, u32, u32)>::new(),
        "oracle: the binary face checks the right operand only"
    );
    assert_eq!(
        sorted_rows("void f() { this; }\n"),
        Vec::<(String, u32, u32)>::new(),
        "oracle: `reportErrorIfSuper` never checks `this`"
    );
    assert_eq!(
        sorted_rows("void f() { var x = this; }\n"),
        Vec::<(String, u32, u32)>::new(),
        "oracle: silent initializer value"
    );
    assert_eq!(
        sorted_rows("void f() { return super; }\n"),
        Vec::<(String, u32, u32)>::new(),
        "oracle: no reportErrorIfSuper caller on return values"
    );
    assert_eq!(
        sorted_rows("void f() { throw super; }\n"),
        Vec::<(String, u32, u32)>::new(),
        "oracle: no reportErrorIfSuper caller on throw expressions"
    );
    assert_eq!(
        sorted_rows("void f() { for (super;;) {} }\n"),
        Vec::<(String, u32, u32)>::new(),
        "oracle: the for-initializer chain has no reportErrorIfSuper caller"
    );
}

/// The variable-initializer face (`endVariableInitializer`,
/// ast_builder.dart:3459) on the local flavor, the member field flavor
/// (shared call), and the for-in iterable face (`endForIn`, :4282).
#[test]
fn initializer_and_for_in_faces_match_oracle() {
    assert_eq!(
        sorted_rows("void f() { var x = super; }\n"),
        vec![mas(19, 24)],
        "oracle: the initializer value face"
    );
    assert_eq!(
        sorted_rows("class C { int x = super; }\n"),
        vec![mas(18, 23)],
        "oracle: the field flavor rides the shared initializer call"
    );
    assert_eq!(
        sorted_rows("void f() { for (var x in super) {} }\n"),
        vec![mas(25, 30)],
        "oracle: the iterable face"
    );
    assert_eq!(
        sorted_rows("void f() { for (super = 3;;) {} }\n"),
        vec![mas(16, 21)],
        "oracle: for-init keeps the assignment-arm selector only — the \
         statement rider never fires outside statements"
    );
}

/// The arrow-body face (`handleExpressionFunctionBody`,
/// ast_builder.dart:4179) on the function-expression and named-function
/// flavors, and the parenthesized-condition face
/// (`endParenthesizedCondition`, :5229) across if/while/switch statements,
/// the switch expression, and the synthetic-group recovery
/// (`if super) {}`).
#[test]
fn body_and_condition_faces_match_oracle() {
    assert_eq!(
        sorted_rows("void f() { var g = () => super; }\n"),
        vec![mas(25, 30)],
        "oracle: the arrow-body face"
    );
    assert_eq!(
        sorted_rows("void g() => super;\n"),
        vec![mas(12, 17)],
        "oracle: the named-function flavor rides the same body parser"
    );
    assert_eq!(
        sorted_rows("void f() { if (super) {} }\n"),
        vec![mas(15, 20)],
        "oracle: the if-condition face"
    );
    assert_eq!(
        sorted_rows("void f() { while (super) {} }\n"),
        vec![mas(18, 23)],
        "oracle: the while-condition face"
    );
    assert_eq!(
        sorted_rows("void f() { switch (super) {} }\n"),
        vec![mas(19, 24)],
        "oracle: the switch-statement condition face"
    );
    assert_eq!(
        sorted_rows("void f() { var y = switch (super) { _ => 1 }; }\n"),
        vec![mas(27, 32)],
        "oracle: the switch-expression condition rides the shared \
         parenthesized-condition parser"
    );
    assert!(
        sorted_rows("void f() { if super) {} }\n").contains(&mas(14, 19)),
        "oracle: the synthetic-group recovery keeps the condition face on \
         the condition token"
    );
}

/// The type-argument-application receiver face
/// (`handleTypeArgumentApplication`, ast_builder.dart:5608): a standalone
/// group on a bare `super` receiver reports; the invoked flavor
/// (`super<int>(3)`) checks its arguments and stays silent.
#[test]
fn type_args_receiver_face_matches_oracle() {
    assert_eq!(
        sorted_rows("void f() { super<int>; }\n"),
        vec![mas(11, 16)],
        "oracle: the applied, non-invoked group face"
    );
    assert_eq!(
        sorted_rows("void f() { super<int>.x; }\n"),
        vec![mas(11, 16)],
        "oracle: the selector follower keeps the receiver face"
    );
    assert_eq!(
        sorted_rows("void f() { var y = super<int>; }\n"),
        vec![mas(19, 24)],
        "oracle: the receiver face in expression position"
    );
    assert_eq!(
        sorted_rows("void f() { super<int>(3); }\n"),
        Vec::<(String, u32, u32)>::new(),
        "oracle: the invoked flavor checks the arguments only"
    );
}
