//! #2995 pins: the type-variable-declaration keyword keep-set and the
//! `missing_assignable_selector` faces (upstream Dart SDK source,
//! `pkg/_fe_analyzer_shared/lib/src/parser/identifier_context_impl.dart`
//! `TypeVariableDeclarationIdentifierContext.ensureIdentifier` :1589-1647,
//! and `pkg/analyzer/lib/src/fasta/ast_builder.dart`).
//!
//! Keep-set faces: the upstream `ensureIdentifier` tests the keep-set
//! (top-level declaration heads `isTopLevelKeyword`, class-member heads
//! `isModifier` + `get`/`set`/`void`, statement starts, and the type-variable
//! followers `<` `>` `>>` `>>>` `;` `}` `extends` `super` `=` `>=` EOF)
//! BEFORE the built-in face, so a built-in head (`typedef`, `get`, `set`,
//! `required`, ...) splices a synthetic name and reports
//! `missing_identifier` — never `builtInIdentifierInDeclaration` +
//! consume, and never `expectedIdentifierButGotKeyword` for the reserved
//! keep members.
//!
//! Selector faces: `missingAssignableSelector` rows span the whole
//! non-assignable LHS (`handleRecoverableError(…, lhs.beginToken,
//! lhs.endToken)`, ast_builder.dart:3831-3836), a bare `this` target is
//! non-assignable too, prefix `++super` anchors on the operand's end token
//! (:5643-5659), postfix `super++` anchors `illegalAssignmentToNonAssignable`
//! on the operator (:5627-5642), and the `reportErrorIfSuper` family
//! (:961 arguments, :1061 await, :1079 binary right, :1300/:1301 conditional
//! arms, :3653 assignment RHS, :5795 prefix `!`) reports on bare `super`
//! sub-expressions.

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

/// Selector faces on the assignment arm: the row spans the whole LHS run
/// (`super + 1 = 3` reports (11,20), never the bare `super` (11,16)), a
/// single-token LHS keeps its own extent, and the bare-keyword statement
/// targets carry their full oracle row sets (#3091 closed the statement
/// faces: the `this` selector row and the illegal rider live on
/// `handleExpressionStatement`, ast_builder.dart:4209-4221).
#[test]
fn assignment_lhs_selector_rows_span_the_lhs() {
    assert_eq!(
        sorted_rows("void f() { super + 1 = 3; }\n"),
        vec![
            code("illegal_assignment_to_non_assignable", 11, 20),
            code("missing_assignable_selector", 11, 20),
        ],
        "oracle 3.13.7: MAS + illegal at `super + 1` (12,9)"
    );
    assert_eq!(
        sorted_rows("void f() { 1 = 2; }\n"),
        vec![
            code("illegal_assignment_to_non_assignable", 11, 12),
            code("missing_assignable_selector", 11, 12),
        ],
        "oracle: single-token LHS keeps (12,1)"
    );
    assert_eq!(
        sorted_rows("void f() { this = 3; }\n"),
        vec![
            code("illegal_assignment_to_non_assignable", 11, 15),
            code("missing_assignable_selector", 11, 15),
        ],
        "oracle: both rows on the `this` target; the statement context is \
         what separates them from the redirecting-initializer lane \
         (`C() : this = 0;` keeps its silent face)"
    );
    assert_eq!(
        sorted_rows("void f() { super = 0; }\n"),
        vec![
            code("illegal_assignment_to_non_assignable", 11, 16),
            code("missing_assignable_selector", 11, 16),
        ],
        "oracle MAS at `super` (12,5) from the assignment arm; the illegal \
         rider is the statement-level face"
    );
}

/// Prefix/postfix assignment faces: `++super` anchors MAS on the operand's
/// end token, `super++` anchors the illegal row on the `++` operator.
#[test]
fn prefix_postfix_assignment_rows_match_oracle() {
    assert_eq!(
        sorted_rows("void f() { ++super; }\n"),
        vec![code("missing_assignable_selector", 13, 18)],
        "oracle: MAS at `super` (14,5)"
    );
    assert_eq!(
        sorted_rows("void f() { super++; }\n"),
        vec![code("illegal_assignment_to_non_assignable", 16, 18)],
        "oracle: illegal at `++` (col 17, len 2)"
    );
}

/// `reportErrorIfSuper` faces: a bare `super` sub-expression carries the
/// selector row at its own span; wrappers (`-super`, `(super)`,
/// `super.x`) fail the shape probe and stay silent, like the official
/// expression-shape test.
#[test]
fn report_error_if_super_faces_match_oracle() {
    assert_eq!(
        sorted_rows("void f() { g(super); }\n"),
        vec![code("missing_assignable_selector", 13, 18)],
        "oracle: argument face (14,5)"
    );
    assert_eq!(
        sorted_rows("void f() { x = super; }\n"),
        vec![code("missing_assignable_selector", 15, 20)],
        "oracle: assignment-RHS face (16,5)"
    );
    assert_eq!(
        sorted_rows("void f() { !super; }\n"),
        vec![code("missing_assignable_selector", 12, 17)],
        "oracle: prefix `!` face (13,5); `-super`/`~super` stay silent"
    );
    assert_eq!(
        sorted_rows("void f() async { await super; }\n"),
        vec![code("missing_assignable_selector", 23, 28)],
        "oracle: await face (24,5)"
    );
    assert!(
        sorted_rows("void f() { c ? super : 1; }\n")
            .contains(&code("missing_assignable_selector", 15, 20)),
        "oracle: then-arm face (16,5); a pre-existing extra \
         `missing_identifier` on the kept `super` predates #2995"
    );
    assert!(
        sorted_rows("void f() { c ? 1 : super; }\n")
            .contains(&code("missing_assignable_selector", 19, 24)),
        "oracle: else-arm face (20,5)"
    );
    assert_eq!(
        sorted_rows("void f() { -super; }\n"),
        Vec::<(String, u32, u32)>::new(),
        "official `handleUnaryPrefixExpression` skips `-`/`~`"
    );
}

/// Built-in keep-set members: `typedef`/`get`/`set`/`required` splice the
/// synthetic name (`missing_identifier`), never the built-in consume face.
#[test]
fn builtin_type_variable_heads_take_the_keep_face() {
    // The single-token `< set >` group shape stays on the consume face —
    // upstream classifies it Complex (type_info.dart:445 admits only
    // IDENTIFIER/pseudo names) and keeps the keyword, but dartq's Simple1
    // fast path asserts a closer directly after the name; mirroring the
    // upstream gate is the type_args.rs owner's face (#2995 residual).
    assert_eq!(
        sorted_rows("class A<set> {}\n"),
        vec![code("built_in_identifier_in_declaration", 8, 11)],
        "oracle `missing_identifier` blocked on the Simple1 scan gate"
    );
    assert_eq!(
        sorted_rows("void b< set>();\n"),
        vec![
            code("built_in_identifier_in_declaration", 8, 11),
            code("missing_function_body", 14, 15),
        ],
        "oracle `missing_identifier` blocked on the Simple1 scan gate"
    );
    // The keep face lands (`missing_identifier` at `get`, no built-in row);
    // the bound-region second row and the missing-closer cascade pre-date
    // #2995 (present on the pre-edit tree) and ride the class-header
    // cascade owner.
    assert!(
        sorted_rows("class A<get extends int> {}\n")
            .contains(&code("missing_identifier", 8, 11))
            && !sorted_rows("class A<get extends int> {}\n")
                .contains(&code("built_in_identifier_in_declaration", 8, 11)),
        "oracle: keep face, the bound parses, the `>` closes"
    );
    assert_eq!(
        sorted_rows("typedef F = void Function< get >();\n"),
        vec![code("built_in_identifier_in_declaration", 27, 30)],
        "oracle `missing_identifier` blocked on the Simple1 scan gate"
    );
}

/// Reserved keep-set members keep the extends/super posture. The
/// missing-closer cascade after the spliced name is silent upstream
/// (`parseUnexpectedEnd`'s `errorReported = token.isSynthetic`,
/// type_info_impl.dart:1589) — dartq still reports that one row until the
/// type-args caller threads the synthetic token3 bit (#2995 residual).
#[test]
fn reserved_type_variable_heads_take_the_keep_face() {
    let rows = sorted_rows("void b< typedef int>();\n");
    assert!(rows.contains(&code("missing_identifier", 8, 15)));
    assert!(rows.contains(&code("missing_function_body", 22, 23)));
    assert!(
        !rows.contains(&code("built_in_identifier_in_declaration", 8, 15)),
        "the built-in consume face must not fire for keep-set members"
    );
    let rows = sorted_rows("void b< required int>();\n");
    assert!(rows.contains(&code("missing_identifier", 8, 16)));
    assert!(rows.contains(&code("missing_function_body", 23, 24)));
    assert!(
        !rows.contains(&code("built_in_identifier_in_declaration", 8, 16)),
        "required heads a class member upstream (isModifier)"
    );
}

/// Non-keep-set reserved heads keep the tolerant consume face
/// (`expectedIdentifierButGotKeyword`) plus the verifier's compile-time
/// name face (`_checkForBuiltInIdentifierAsName`,
/// error_verifier.dart:3365-3377 fires on every non-pseudo keyword name) —
/// `is`/`in`/`with`/`new`/`null` belong to no upstream keep-set member.
#[test]
fn non_keep_set_reserved_heads_stay_consumed() {
    assert_eq!(
        sorted_rows("void b< is int>();\n"),
        vec![
            code("built_in_identifier_in_declaration", 8, 10),
            code("expected_identifier_but_got_keyword", 8, 10),
            code("expected_token", 11, 14),
            code("missing_function_body", 17, 18),
        ],
        "oracle: consume face + built-in name face on the `is` (col 9), the \
         missing-comma row at `int` (col 12) and the body row at `;` (col 18)"
    );
}

/// The `reportErrorIfSuper` wrappers beyond the core arm set: the is/as
/// operand (`handleIsOperator` ast_builder.dart:4619,
/// `handleAsOperator` :3619 — the operand, never the type), the bracket
/// content (`handleIndexedExpression` :4504 — `g[super]` reports, `super[0]`
/// checks the `0`), and the parenthesized reading
/// (`endParenthesizedExpression` :2613 — the exact `(super)` inner shape,
/// recursive through nested groups).
#[test]
fn selector_wrapper_faces_match_oracle() {
    assert_eq!(
        sorted_rows("void f() { super is int; }\n"),
        vec![code("missing_assignable_selector", 11, 16)],
        "oracle: is-operand face (12,5)"
    );
    assert_eq!(
        sorted_rows("void f() { super as int; }\n"),
        vec![code("missing_assignable_selector", 11, 16)],
        "oracle: as-operand face (12,5)"
    );
    assert_eq!(
        sorted_rows("void f() { g[super]; }\n"),
        vec![code("missing_assignable_selector", 13, 18)],
        "oracle: index-content face (14,5)"
    );
    assert_eq!(
        sorted_rows("void f() { (super); }\n"),
        vec![code("missing_assignable_selector", 12, 17)],
        "oracle: parenthesized face (13,5)"
    );
    assert_eq!(
        sorted_rows("void f() { ((super)); }\n"),
        vec![code("missing_assignable_selector", 13, 18)],
        "oracle: one row from the innermost group"
    );
    assert_eq!(
        sorted_rows("void f() { super[0]; }\n"),
        vec![],
        "a selector-attached target is not a bare super"
    );
    assert_eq!(
        sorted_rows("void f() { super is! int; }\n"),
        vec![code("missing_assignable_selector", 11, 16)],
        "oracle: the `is!` face rides the same handler"
    );
}
