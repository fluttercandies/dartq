//! Recovery primitive tests.

use super::collector::{Collector, latest_features, setup};

#[test]
fn ensure_identifier_synthesizes_on_error() {
    let mut p = setup(">");
    let id = p.ensure_identifier(crate::listener::IdentifierContext::TypeReference);
    assert_eq!(p.cursor.lexeme(id.id), ""); // synthetic, fixed empty lexeme
    assert_eq!(
        p.listener.error_codes(),
        vec!["missing_identifier".to_string()]
    );
}

#[test]
fn expect_kind_reports_expected_token() {
    let mut p = setup("x");
    let semi = p.expect_kind(dartq_scanner::TokenKind::Semicolon);
    assert_eq!(
        p.cursor.resolve(semi.id).0,
        dartq_scanner::TokenKind::Semicolon
    );
    assert_eq!(p.listener.error_codes(), vec!["expected_token".to_string()]);
}

#[test]
fn skip_to_sync_stops_at_boundary() {
    let mut p = setup("a b ; c");
    p.skip_to_sync(&[dartq_scanner::TokenKind::Semicolon]);
    assert_eq!(p.cursor.lexeme(p.peek_id()), ";");
}

#[test]
fn ensure_close_paren_adopts_scanner_synthetic_and_reports_the_row() {
    // Unclosed `(` — the scanner inserts the synthetic `)` and records the
    // unmatched pair on the table (official `unmatchedBeginGroup` records an
    // `UnmatchedToken` without emitting); the parser adopts that closer
    // (`moveSynthetic`) and reports the row itself at the relocation point.
    let config = dartq_scanner::ScanConfig {
        enable_triple_shift: true,
        enable_augmentations: true,
    };
    let mut files = dartq_source::FileTable::new();
    let file = files.intern(std::path::Path::new("t.dart"));
    let result = dartq_scanner::scan("(", config, file);
    assert!(
        result.diagnostics.is_empty(),
        "the scanner no longer reports the unclosed group"
    );
    assert_eq!(
        result.tokens.unmatched_groups().len(),
        1,
        "the pair rides the table"
    );
    let table: &'static dartq_scanner::TokenTable = Box::leak(Box::new(result.tokens));
    let text: &'static str = "(";
    let mut collector = super::collector::Collector::default();
    let mut p = crate::Parser::new(
        text,
        table,
        super::collector::latest_features(),
        &mut collector,
    );
    let open = p.peek_span();
    assert_eq!(
        p.cursor.resolve(open.id).0,
        dartq_scanner::TokenKind::OpenParen
    );
    let close = p.ensure_close_paren(open);
    assert!(p.cursor.resolve(close.id).1, "adopted synthetic");
    assert_eq!(
        p.listener.errors,
        vec![("expected_token".to_string(), 0, 1)],
        "one parser-side row for the adopted pair (relocated in front of the `(`)"
    );
}

/// Scans `source` without the `scan_ok` no-diagnostics assertion and leaks
/// the table for `'static` parser setup. Returns the scan result so tests
/// can assert the scanner-side diagnostics of truncated input.
fn scan_leaked(
    source: &'static str,
) -> (
    &'static dartq_scanner::TokenTable,
    Vec<dartq_diagnostics::Diagnostic>,
) {
    let config = dartq_scanner::ScanConfig {
        enable_triple_shift: true,
        enable_augmentations: true,
    };
    let mut files = dartq_source::FileTable::new();
    let file = files.intern(std::path::Path::new("t.dart"));
    let result = dartq_scanner::scan(source, config, file);
    let table: &'static dartq_scanner::TokenTable = Box::leak(Box::new(result.tokens));
    (table, result.diagnostics)
}

/// Full-unit parse of `source`'s table: returns the collected diagnostics
/// and whether the parse consumed the stream to EOF.
fn parse_collected(source: &'static str, table: &dartq_scanner::TokenTable) -> (Collector, bool) {
    let mut listener = Collector::default();
    let at_eof = {
        let mut p = crate::Parser::new(source, table, latest_features(), &mut listener);
        p.parse_unit();
        let eof = p.cursor.at_eof();
        eof
    };
    (listener, at_eof)
}

#[test]
fn junk_formal_name_reports_the_parameter_group_tail_only() {
    // 3.13.1 oracle probe of `void f(int 5) {}` (machine format): exactly
    // one row — `EXPECTED_TOKEN` `Expected to find ')'.` spanning the `5`
    // (1|12|1). The name slot takes the official `handleNoName` posture
    // (3.13.1 `parseFormalParameter` consumes a name only
    // `if (next.isIdentifier)`, parser_impl.dart:2229 — no recovery
    // report), and the parameter list's tail `ensureCloseParen` carries
    // the single row anchored on the real junk token. The 3.15 context
    // template (identifier_context_impl.dart:604-611) would add a
    // `missing_identifier` — the 3.13.1 oracle output is the arbiter and
    // has no such row.
    let source = "void f(int 5) {}";
    let (table, scan_diags) = scan_leaked(source);
    assert!(scan_diags.is_empty(), "{scan_diags:?}");
    let (listener, at_eof) = parse_collected(source, table);
    assert_eq!(
        listener.errors,
        vec![("expected_token".to_string(), 11, 12)],
        "one row, anchored on the junk `5`"
    );
    assert_eq!(listener.texts, vec!["Expected to find ')'."]);
    assert!(at_eof, "parse reaches EOF");
}

#[test]
fn no_type_junk_formal_reports_missing_identifier_at_the_junk() {
    // 3.13.1 oracle probe of `void f(5) {}` (machine format): exactly one
    // row — `MISSING_IDENTIFIER` "Expected an identifier." spanning the
    // `5` (1|8|1) — the official template's consume-in-doubt branch; the
    // no-type name slot rides the #184 recovery route (report at the
    // junk, synthetic consumed as the name event).
    let source = "void f(5) {}";
    let (table, scan_diags) = scan_leaked(source);
    assert!(scan_diags.is_empty(), "{scan_diags:?}");
    let (listener, at_eof) = parse_collected(source, table);
    assert_eq!(
        listener.errors,
        vec![("missing_identifier".to_string(), 7, 8)],
        "one row, anchored on the junk `5`"
    );
    assert_eq!(listener.texts, vec!["Expected an identifier."]);
    assert!(at_eof, "parse reaches EOF");
}

#[test]
fn reserved_word_formal_name_reports_the_keyword_face() {
    // 3.13.1 oracle probe of `void f(assert) {}` (machine format): exactly
    // one row — `EXPECTED_IDENTIFIER_BUT_GOT_KEYWORD` spanning `assert`
    // (1|8|6) — the keyword is consumed as the name and no tail row
    // follows. The keyword route is context-independent: it fires with no
    // type parsed here and behind one (`void t1(int assert)`).
    let source = "void f(assert) {}";
    let (table, scan_diags) = scan_leaked(source);
    assert!(scan_diags.is_empty(), "{scan_diags:?}");
    let (listener, at_eof) = parse_collected(source, table);
    assert_eq!(
        listener.errors,
        vec![("expected_identifier_but_got_keyword".to_string(), 7, 13)],
        "one row spanning the keyword"
    );
    assert_eq!(
        listener.texts,
        vec!["'assert' can't be used as an identifier because it's a keyword."]
    );
    assert!(at_eof, "parse reaches EOF");
}

#[test]
fn unclosed_block_body_at_eof_is_reported_once_by_the_parser() {
    // `void main() {` — the block never closes. Official pipeline: the
    // scanner's `unmatchedBeginGroup` inserts the synthetic `}` and records
    // the `UnmatchedToken` without emitting; the parse-end pass reports
    // `expected_token` "Expected to find '}'." pinned at the EOF offset
    // (oracle `dart analyze --format=json` on the fixture: one expected_token
    // at offset = source length; the parser's `ensureBlock`
    // (parser_impl.dart:4984) adopts the real `{` with its (synthetic)
    // endGroup silently — the parser never reports a second `}` row).
    let source = include_str!("fixtures/eof_unclosed_block.dart");
    let (table, scan_diags) = scan_leaked(source);
    assert!(scan_diags.is_empty(), "{scan_diags:?}");
    assert_eq!(
        table.unmatched_groups().len(),
        1,
        "the pair rides the table"
    );

    let (listener, at_eof) = parse_collected(source, table);
    assert_eq!(
        listener.errors,
        vec![(
            "expected_token".to_string(),
            source.len() as u32,
            source.len() as u32 + 1
        )],
        "one parse-end row, length 1 at the eof offset (translateErrorToken's \
         unconditional `length: 1`; the eof decrement never fires — the \
         front-chained error run defeats _isAtEnd)"
    );
    assert!(at_eof, "parse did not reach EOF");
}

#[test]
fn unclosed_class_body_at_eof_is_reported_once_by_the_parser() {
    // `class C {` — same official shape as the unclosed function body: the
    // parse-end pass reports the missing `}` once (unmatchedBeginGroup pair,
    // pinned at the EOF offset) and `ensureBlock` (BlockKind.class
    // declaration, parser_impl.dart:3127) adopts the real `{` + synthetic
    // endGroup without a second row.
    let source = include_str!("fixtures/eof_unclosed_class_body.dart");
    let (table, scan_diags) = scan_leaked(source);
    assert!(scan_diags.is_empty(), "{scan_diags:?}");

    let (listener, at_eof) = parse_collected(source, table);
    assert_eq!(
        listener.errors,
        vec![(
            "expected_token".to_string(),
            source.len() as u32,
            source.len() as u32 + 1
        )],
        "one parse-end row, length 1 at the eof offset"
    );
    assert!(at_eof, "parse did not reach EOF");
}

#[test]
fn unclosed_paren_and_square_at_eof_report_once_and_parse_cleanly() {
    // Cascade shapes: `(` and `[` left open at EOF go through the same
    // one-guard location as `{` — the scanner records the pair and the parse
    // reports once at the EOF offset. `void f(` additionally reports
    // `missing_function_body` during the parse (official `ensureBlock(token,
    // BlockKind.functionBody)` template carries
    // `pseudoSharedCode: PseudoSharedCode.missingFunctionBody`,
    // diagnostic.g.dart:795), and its `)` row relocates to the adoption
    // point — the cursor already sits at EOF, so the encoded row is length 1
    // at the eof offset; the unmatched row is a parse-end pass row (official
    // `reportAllErrorTokens`, parser_impl.dart:11433-11439), so it follows
    // the in-parse body row. `var x = [` closes the (empty) list literal at
    // the synthetic `]` and then trips `ensureSemicolon` at EOF (official
    // `expectedAfterButGot ';'` → analyzer `expected_token`), plus the
    // parse-end `]` row.
    for (source, closer, parser_codes) in [
        (
            "void f(",
            ")",
            vec![
                "missing_function_body".to_string(),
                "expected_token".to_string(),
            ],
        ),
        (
            "var x = [",
            "]",
            vec!["expected_token".to_string(), "expected_token".to_string()],
        ),
    ] {
        let (table, scan_diags) = scan_leaked(source);
        assert!(scan_diags.is_empty(), "{source}: {scan_diags:?}");
        assert_eq!(
            table.unmatched_groups().len(),
            1,
            "{source}: the pair rides the table"
        );

        let (listener, at_eof) = parse_collected(source, table);
        assert_eq!(
            listener.error_codes(),
            parser_codes,
            "{source}: parser diagnostic mismatch"
        );
        assert!(
            listener
                .texts
                .iter()
                .any(|t| t == &format!("Expected to find '{closer}'.")),
            "{source}: the unmatched row's text: {:?}",
            listener.texts
        );
        assert!(at_eof, "{source}: parse did not reach EOF");
    }
}

#[test]
fn parse_unit_consumes_everything_without_progress_hangs() {
    let mut p = setup("a + b )) ) }} ; ;");
    p.parse_unit();
    assert!(p.cursor.at_eof());
}

#[test]
fn expression_continuation_rewrites_new_as_identifier() {
    // `_tryRewriteNewToIdentifier` (parser_impl.dart:4101-4111): every
    // continuation context rewrites a `new` head to the
    // unnamed-constructor identifier with no report
    // (`allowsNewAsIdentifier => isContinuation`,
    // identifier_context_impl.dart:200/391). The cascade sections consume
    // their `..new` in `parse_send_continuation`; this is the remaining
    // dot-continuation gate (selector chains, dot shorthands, pattern
    // selectors).
    let mut p = setup("new");
    let name = p.ensure_identifier(crate::listener::IdentifierContext::ExpressionContinuation);
    assert!(
        !p.cursor.resolve(name.id).1,
        "the real `new` token, not a synthetic"
    );
    assert_eq!(p.cursor.lexeme(name.id), "new");
    assert!(p.listener.error_codes().is_empty(), "the rewrite is silent");
}

/// Logs the constructor-reference selector events next to the recovery
/// rows: the enum-constant empty-name face rides `handleIdentifier` on the
/// synthesized identifier (official `ensureIdentifier`,
/// parser_impl.dart:4083).
#[derive(Default)]
struct SelectorListener {
    errors: Vec<(String, u32, u32)>,
    selectors: Vec<(&'static str, u32, u32)>,
}

impl crate::listener::ParserListener for SelectorListener {
    fn handle_recoverable_error(
        &mut self,
        message: crate::message::Message,
        start: crate::token_id::TokenSpan,
        end: crate::token_id::TokenSpan,
    ) {
        self.errors.push((
            message.code.name.to_owned(),
            start.range.offset,
            end.range.offset + end.range.length,
        ));
    }

    fn handle_identifier(
        &mut self,
        token: crate::listener::Token,
        context: crate::listener::IdentifierContext,
    ) {
        let name = match context {
            crate::listener::IdentifierContext::ConstructorReferenceContinuation => {
                "constructorReferenceContinuation"
            }
            crate::listener::IdentifierContext::ConstructorReferenceContinuationAfterTypeArguments => {
                "constructorReferenceContinuationAfterTypeArguments"
            }
            _ => return,
        };
        self.selectors
            .push((name, token.range.offset, token.range.length));
    }
}

#[test]
fn enum_dot_selector_fires_the_empty_name_identifier_event() {
    // Corpus issue_48380_1 (the enum-dot selector with the name missing
    // before `;`): the official parse reports the selector's
    // `missing_identifier` at the `;` and the argument tail's
    // `expected_token` — `reportRecoverableError` anchors on the zero-width
    // synthetic identifier and `findNonZeroLengthToken` walks it onto the
    // `;` (oracle capture 5ad4a307: `expected_token` 2:34|1) — and the
    // template's returned synthetic rides `handleIdentifier` so the
    // semantic face (constructor lookup on the empty name) is reachable
    // (official trace: `handleIdentifier(,
    // constructorReferenceContinuationAfterTypeArguments)`).
    let source =
        "enum E<F> {\n  v<int>./*about to write foo()*/;\n\n  const E();\n  const E.foo();\n}\n";
    let (table, scan_diags) = scan_leaked(source);
    assert!(scan_diags.is_empty(), "{scan_diags:?}");
    let semi_at = 45usize; // the `;` closing the element (line 2 col 34)
    let mut listener = SelectorListener::default();
    {
        let mut p = crate::Parser::new(source, table, latest_features(), &mut listener);
        p.parse_unit();
    }
    assert_eq!(
        listener.errors,
        vec![
            (
                "missing_identifier".to_string(),
                semi_at as u32,
                semi_at as u32 + 1
            ),
            (
                "expected_token".to_string(),
                semi_at as u32,
                semi_at as u32 + 1
            ),
        ],
        "selector row at the `;`, argument tail walked to the `;`"
    );
    assert_eq!(
        listener.selectors,
        vec![(
            "constructorReferenceContinuationAfterTypeArguments",
            semi_at as u32,
            0,
        )],
        "the empty-name identifier event rides the synthetic at the `;`"
    );
}

// —— unmatched-group contract pins (oracle `dart analyze --format=machine`
// rows; the scanner records the pairs, the parser reports the rows) ———————

/// The `expected_token` rows of a full-unit parse (filtered from the whole
/// recovery cascade so the pins carry exactly the contract faces).
fn unmatched_rows(source: &'static str) -> Vec<(String, u32, u32)> {
    let (table, scan_diags) = scan_leaked(source);
    assert!(
        scan_diags.iter().all(|d| d.code.name != "expected_token"),
        "the scanner no longer reports unmatched groups: {scan_diags:?}"
    );
    let (listener, _) = parse_collected(source, table);
    listener
        .errors
        .into_iter()
        .filter(|(code, _, _)| code == "expected_token")
        .collect()
}

#[test]
fn pin_two_unclosed_curly_relocates_paren_dedupes_braces() {
    // sii-probes/two_unclosed_curly.dart `f('${{\n` (oracle machine rows):
    // ')' @1:4|1 (offset 3 — the `${`: the formals tail adopts the `(`'s
    // EOF-parked synthetic at the `${` follower, the moveSynthetic
    // relocation) and '}' @2:1|1 (offset 7 = eof, length 1). The `${` and
    // `{` groups both park at eof; the official face dedupes identical rows,
    // so ONE `}` row surfaces.
    assert_eq!(
        unmatched_rows("f('${{\n"),
        vec![
            ("expected_token".to_string(), 3, 4),
            ("expected_token".to_string(), 7, 8),
        ],
        "paren relocated to the interpolation head, one deduped brace row at eof"
    );
}

#[test]
fn pin_deep_close_bare_matches_oracle_rows() {
    // sii-probes/deep_close_bare.dart `f('${\n` (oracle machine rows):
    // ')' @1:4|1 (offset 3), '}' @2:1|1 (offset 6 = eof, length 1).
    assert_eq!(
        unmatched_rows("f('${\n"),
        vec![
            ("expected_token".to_string(), 3, 4),
            ("expected_token".to_string(), 6, 7),
        ],
    );
}

#[test]
fn pin_nested_unclosed_matches_oracle_rows() {
    // sii-probes/nested_unclosed.dart `f('${f('${')\n` (oracle machine rows):
    // ')' @1:4|1 (offset 3, the outer formals' relocation), ')' @1:9|1
    // (offset 8, the inner formals' relocation), '}' @2:1|1 (offset 13 = eof,
    // length 1; the two `${` groups dedupe).
    let mut rows = unmatched_rows("f('${f('${')\n");
    rows.sort_by_key(|&(_, off, _)| off);
    assert_eq!(
        rows,
        vec![
            ("expected_token".to_string(), 3, 4),
            ("expected_token".to_string(), 8, 9),
            ("expected_token".to_string(), 13, 14),
        ],
    );
}

#[test]
fn pin_bare_interp_reports_the_brace_at_eof() {
    // sii-probes/bare_interp.dart `'${\n` (oracle machine row): '}' @2:1|1
    // (offset 4 = eof, length 1).
    assert_eq!(
        unmatched_rows("'${\n"),
        vec![("expected_token".to_string(), 4, 5)],
    );
}

#[test]
fn pin_empty_interp_noparen_matches_oracle_rows() {
    // sii-probes/empty_interp_noparen.dart `g('${\n` (oracle machine rows):
    // ')' @1:4|1 (offset 3), '}' @2:1|1 (offset 6 = eof, length 1).
    assert_eq!(
        unmatched_rows("g('${\n"),
        vec![
            ("expected_token".to_string(), 3, 4),
            ("expected_token".to_string(), 6, 7),
        ],
    );
}

#[test]
fn pin_bracket_mismatch_06_relocates_to_the_semicolon() {
    // m0-syntax/error_recovery/bracket_mismatch_06.dart `main() {\n  new C(;\n}\n`:
    // ')' @2:9 (offset 17 — the `;`: the argument parse adopts the `(`'s
    // `}`-parked synthetic — inserted by the dual-option insert posture — at
    // the `;` follower). Exactly one expected_token row; the row text is the
    // shared `expected_token` template.
    let source = "main() {\n  new C(;\n}\n";
    let (table, scan_diags) = scan_leaked(source);
    assert!(scan_diags.is_empty(), "{scan_diags:?}");
    let (listener, _) = parse_collected(source, table);
    let rows: Vec<_> = listener
        .errors
        .iter()
        .filter(|(c, _, _)| c == "expected_token")
        .collect();
    assert_eq!(
        rows,
        vec![&("expected_token".to_string(), 17, 18)],
        "one row, relocated to the `;`"
    );
    assert!(
        listener.texts.iter().any(|t| t == "Expected to find ')'."),
        "{:?}",
        listener.texts
    );
}

#[test]
fn pin_issue50591_example2_reports_the_brace_at_eof() {
    // m0-syntax/patterns/issue50591_example2.dart: the function body `{` is
    // the only unclosed group (the two-pass walk declines — the switch's
    // closer is at equal indent); '}' @6:1 (offset = source length = eof,
    // length 1).
    let source = "void f(Object? x) {\n  (switch (x) {\n    const A() => 0,\n    _ => 1,\n  });\n";
    assert_eq!(
        unmatched_rows(source),
        vec![(
            "expected_token".to_string(),
            source.len() as u32,
            source.len() as u32 + 1
        )],
    );
}

#[test]
fn pin_object_recovery_reports_one_paren_row() {
    // m0-syntax/patterns/object_recovery_missingClosingParen.dart: the oracle
    // reports exactly one ')' row (3:28). Our row lands at the object-pattern
    // close's relocation anchor; the exact anchor is the pattern lane's
    // posture (the nested `int(` field parse consumes the stray `:` and the
    // keyword head), so this pin carries the row COUNT and text — the
    // position-exact face is the serial differential's adjudication.
    let source =
        "void f(x) {\n  switch (x) {\n    case dynamic(foo: int():\n      break;\n  }\n}\n";
    let (table, scan_diags) = scan_leaked(source);
    assert!(scan_diags.is_empty(), "{scan_diags:?}");
    let (listener, _) = parse_collected(source, table);
    let rows: Vec<_> = listener
        .errors
        .iter()
        .filter(|(c, _, _)| c == "expected_token")
        .collect();
    assert_eq!(rows.len(), 1, "one unmatched row: {rows:?}");
    assert!(
        listener.texts.iter().any(|t| t == "Expected to find ')'."),
        "{:?}",
        listener.texts
    );
}

#[test]
fn pin_double_dollar_unmatched_row_shapes() {
    // sii-probes/double_dollar.dart `f('$${')\n` (oracle machine rows):
    // ')' @1:4|1 (offset 3, the relocation) and '}' @2:1|1 (offset 9 = eof,
    // length 1; the two parked `}` groups dedupe). The oracle's `;`@1:5 row
    // is the ensureSemicolon anchor face, not an unmatched-group row: it
    // anchors past the relocation point, on the token the parse resumed at
    // (offset 4, the `$$` split head — the #2024 recovery-anchor rule; the
    // `findNonZeroLengthToken` forward walk keeps a zero-width synthetic
    // from capturing the anchor).
    let rows = unmatched_rows("f('$${')\n");
    let mut braces: Vec<_> = rows.iter().filter(|r| r.0 == "expected_token").collect();
    braces.sort_by_key(|&(_, off, _)| off);
    assert_eq!(
        braces,
        vec![
            &("expected_token".to_string(), 3, 4),
            &("expected_token".to_string(), 4, 6),
            &("expected_token".to_string(), 9, 10),
        ],
        "{rows:?}"
    );
}

#[test]
fn expected_semicolon_anchors_on_the_unterminated_string() {
    // Corpus f59c79c8 (string3.dart) / e0f42062 (string4.dart): a `return`
    // whose multiline string runs into EOF. The official
    // `findPreviousNonZeroLengthToken` walk (util.dart:39) skips only
    // `isSynthetic && length == 0`, so the ensureSemicolon row anchors on the
    // unterminated string itself — the official `SyntheticStringToken` shape
    // (synthetic but non-zero length), not on the `return` before it. The
    // unclosed `{` parks one parse-end `}` row, length 1 at the eof offset.
    for (source, string_start, string_len) in [
        ("void foo() {\n  return \"\"\"\næbler", 22, 9),
        ("void bar() {\n  return r\"\"\"\næbler", 22, 10),
    ] {
        let (table, scan_diags) = scan_leaked(source);
        assert_eq!(
            scan_diags.len(),
            1,
            "{source:?}: only the unterminated string is scanner-reported"
        );
        let (listener, at_eof) = parse_collected(source, table);
        let eof = source.chars().map(char::len_utf16).sum::<usize>() as u32;
        assert_eq!(
            listener.errors,
            vec![
                (
                    "expected_token".to_string(),
                    string_start,
                    string_start + string_len
                ),
                ("expected_token".to_string(), eof, eof + 1),
            ],
            "{source:?}: ';' anchors on the string; parse-end row length 1 at eof"
        );
        assert!(at_eof, "{source:?}: parse did not reach EOF");
    }
}

#[test]
fn pin_8e7bfa84_semicolon_anchors_and_eof_docking() {
    // Full corpus source (tests/corpus/regressions/cases/8e7bfa84.json,
    // m0-syntax string2.dart). Four terminated strings keep anchoring the
    // ensureSemicolon row on their last consumed segment (the tail `"""` for
    // the `$bar` interpolation, the whole literal otherwise) while the
    // unterminated tail anchors on its synthetic string; the unclosed `{`
    // parks one parse-end `}` row, length 1 at the eof offset — the oracle
    // capture's six expected_token rows, by span.
    let source = include_str!("fixtures/regression_8e7bfa84_unterminated_interpolations.dart");
    let (table, scan_diags) = scan_leaked(source);
    assert_eq!(
        scan_diags.len(),
        2,
        "the trailing backslash and the unterminated string: {scan_diags:?}"
    );
    let (listener, at_eof) = parse_collected(source, table);
    assert_eq!(
        listener.errors,
        vec![
            ("expected_token".to_string(), 45, 48),
            ("expected_token".to_string(), 74, 85),
            ("expected_token".to_string(), 111, 123),
            ("expected_token".to_string(), 149, 162),
            ("expected_token".to_string(), 207, 216),
            ("expected_token".to_string(), 216, 217),
        ],
        "the oracle's expected_token row spans, in emission order"
    );
    assert!(at_eof, "parse did not reach EOF");
}

#[test]
fn pin_report_dedup_identical_row_once_same_triple_different_text_distinct() {
    // admit_row's dedup face under the FxHashSet `(offset, length, code)`
    // pre-filter (cost class: O(k) expected per report versus the previous
    // O(k^2) per-cascade linear scan; the exact four-field comparison runs
    // only on triple collisions). Semantics unchanged: an identical
    // re-report is admitted exactly once, and a same-triple row with a
    // different rendered text stays a distinct row (oracle 76dd104c 12:23:
    // `Expected to find ','.` AND `Expected to find ';'.` over one span).
    let mut p = setup("x");
    let at = p.peek_span();
    p.report(crate::message::msg::expected_token(";"), at);
    p.report(crate::message::msg::expected_token(";"), at);
    p.report(crate::message::msg::expected_token(","), at);
    assert_eq!(
        p.listener.error_codes(),
        vec!["expected_token".to_string(), "expected_token".to_string()],
        "the duplicate collapses, the distinct-text row survives"
    );
    assert_eq!(
        p.listener.texts,
        vec![
            "Expected to find ';'.".to_string(),
            "Expected to find ','.".to_string(),
        ],
        "one ';' row and the distinct-text ',' row, in report order"
    );
}
