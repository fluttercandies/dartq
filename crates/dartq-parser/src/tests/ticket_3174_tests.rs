//! #3174 pins: the residual cluster folded from the #3169 lane — the
//! `super. <keyword> =` `=`-arm dispatch and the `super. <literal>`
//! literal-head initializer rejection (oracle 3.13.7). Upstream citations
//! live per-test.

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

/// The `=`/`(`/`?.` dispatch after `super.` reads the token FOLLOWING the
/// name slot. A name spliced at a keyword head sits at that keyword's own
/// offset, so its follower is the keyword itself — `insertParens(
/// includeIdentifier: false)` (parser_impl.dart:4918) makes the spliced
/// identifier's `next` `var`, never the `=` after it. `super. var = 0`
/// therefore routes to the `(`-expect arm (never the `=` declaring-class
/// rider, which fires only for a real name), producing `super.<syn>()` and
/// leaving `var = 0 ;` for the member-loop reparse — `var` reparses as a
/// member with `var` return type + `missing_function_body`, `=` as an
/// operator declaration missing its keyword, and `0 ;` as junk members.
#[test]
fn super_dot_keyword_eq_routes_to_paren_expect() {
    let rows = sorted_rows("class E {E() : super. var = 0;}\n");
    assert_eq!(
        rows,
        vec![
            code("expected_class_member", 28, 29),
            code("expected_class_member", 29, 30),
            code("expected_token", 22, 25),
            code("invalid_operator", 26, 27),
            code("missing_function_body", 22, 25),
            code("missing_function_body", 28, 29),
            code("missing_identifier", 22, 25),
            code("missing_keyword_operator", 26, 27),
            code("missing_method_parameters", 26, 27),
            code("var_return_type", 22, 25),
        ],
        "oracle: `(`-expect + `missing_identifier` + member-reparse garbage \
         — `var` carries `var_return_type` + `missing_function_body`, `=` \
         carries the operator trio, `0 ;` carry `expected_class_member`"
    );
}

/// A real name still takes the `=` declaring-class rider
/// (`field_initialized_outside_declaring_class`, parser_impl.dart:4913-4916)
/// — `super. foo = 0` is a complete `super.foo = 0` expression, no
/// `(`-expect, no member reparse.
#[test]
fn super_dot_real_name_eq_keeps_declaring_class_rider() {
    let rows = sorted_rows("class G {G() : super. foo = 0;}\n");
    assert_eq!(
        rows,
        vec![code("field_initialized_outside_declaring_class", 22, 25)],
        "oracle: `super. foo = 0` keeps the real name — the `=` arm reports \
         `field_initialized_outside_declaring_class` at `foo` and nothing else"
    );
}

/// A literal/bracket operand after `super.` is not a
/// `super()`/`super.named()` head — `buildInitializer` rejects the node, so
/// `endInitializers` reports `invalid_super_in_initializer` at `super`
/// (ast_builder.dart:786-800) and the `;` body position carries
/// `missing_function_body` (the rejected initializer leaves no valid
/// constructor body). The operand itself is consumed (`doDotExpression`,
/// ast_builder.dart:918) with the `missing_identifier` row, and the
/// `(`-expect anchors on the `;` follower.
#[test]
fn super_dot_literal_head_reports_invalid_super_and_body() {
    let rows = sorted_rows("class K {K() : super. 3;}\n");
    assert_eq!(
        rows,
        vec![
            code("expected_token", 23, 24),
            code("invalid_super_in_initializer", 15, 20),
            code("missing_function_body", 23, 24),
            code("missing_identifier", 22, 23),
        ],
        "oracle: `invalid_super_in_initializer` at `super` (15,20) + \
         `missing_identifier` at `3` (22,23) + `missing_function_body` and \
         `expected_token '('` at `;` (23,24)"
    );
}
