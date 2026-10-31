//! #3169 pins: the residual cluster folded from the #3166 lane — the
//! `super.`-initializer `(`-expect member reparse, the `catch (` head's
//! synthetic-`)` follower adoption, and the `assert`-statement `;` anchor
//! (oracle 3.13.7). Upstream citations live per-test.

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

/// `insertParens(includeIdentifier: false)` (parser_impl.dart:4948-4952)
/// produces `super.<synthetic-name>()` for a keyword/junk name head — the
/// `()` wraps the token AFTER the name, so `var f;` survives as the member
/// head for the class-body reparse. dartq splices synthetics at the cursor
/// only, so the `super.<name>(args)` initializer is driven through the send
/// machinery (`handleSuperExpression` + `.` + the spliced name + the empty
/// `()` + `handleDotAccess`); `var f;` then reparses as a member field,
/// carrying `missing_function_body` at `var`. The `(`-expect and the spliced
/// `missing_identifier` anchor there too — no `expected_class_member` rows.
#[test]
fn super_dot_keyword_name_reparses_member() {
    let rows = sorted_rows("class C {C() : super. var f;}\n");
    assert_eq!(
        rows,
        vec![
            code("expected_token", 22, 25),
            code("missing_function_body", 22, 25),
            code("missing_identifier", 22, 25),
        ],
        "oracle: `missing_function_body` + `missing_identifier` + \
         `expected_token '('` all at `var` (22,25); `var f;` reparses as a \
         member field — no `expected_class_member` rows"
    );
}

/// A real member name (`super. foo f;`) drives the same `super.foo()` send;
/// the leftover `f;` reparses as a member field — `missing_const_final_var_or_type`
/// + `missing_function_body` at `f`, with the `(`-expect on `f`.
#[test]
fn super_dot_real_name_reparses_member() {
    let rows = sorted_rows("class D {D() : super. foo f;}\n");
    assert_eq!(
        rows,
        vec![
            code("expected_token", 26, 27),
            code("missing_const_final_var_or_type", 26, 27),
            code("missing_function_body", 26, 27),
        ],
        "oracle: `(`-expect + `missing_const_final_var_or_type` + \
         `missing_function_body` all at `f` (26,27); `f;` reparses as an \
         unnamed member field"
    );
}

/// The `catch (` head reads the name slot as the real next token — a
/// scanner-parked synthetic `)` end-group is zero-width and sits inline at
/// the follower's offset, while upstream `catchSyntax` reports the follower's
/// full span (parser_impl.dart:10739-10822, `catchSyntax` translate:
/// `reportRecoverableError` re-anchors through `findNonZeroLengthToken`,
/// util.dart:56). `try {} catch ( }` anchors `catch_syntax`, `expected_body`,
/// and `expected_token ')'` on `}` (28,29) — never the synthetic's zero
/// width.
#[test]
fn catch_paren_reports_follower_span_not_synthetic() {
    let rows = sorted_rows("void f() {\n  try {} catch ( }\n}\n");
    assert_eq!(
        rows,
        vec![
            code("catch_syntax", 28, 29),
            code("expected_body", 28, 29),
            code("expected_executable", 30, 31),
            code("expected_token", 28, 29),
        ],
        "oracle: `catch_syntax` + `expected_body` + `expected_token ')'` at \
         the `}}` (28,29), `expected_executable` at the outer `}}` (30,31)"
    );
}

/// The `assert` statement's `;` anchors on the assert's own last real token
/// — the condition/message expression end (`x` in `assert(x }`) — through
/// `ensureSemicolon`'s `findPreviousNonZeroLengthToken` walk-back
/// (util.dart:39). A `)`-relocation's spill mark must not forward that
/// anchor onto the follower: `x` is the construct's genuine tail, so
/// `expected_token ';'` lands at `x` (20,21) while the relocated `)`'s
/// `expected_token ')'` lands at `}` (22,23).
#[test]
fn assert_statement_semicolon_anchors_on_expression_end() {
    let rows = sorted_rows("void f() {\n  assert(x }\n}\n");
    assert_eq!(
        rows,
        vec![
            code("expected_executable", 24, 25),
            code("expected_token", 20, 21),
            code("expected_token", 22, 23),
        ],
        "oracle: `expected_token ';'` at `x` (20,21) + `expected_token ')'` \
         at `}}` (22,23); `expected_executable` at the outer `}}` (24,25)"
    );
}
