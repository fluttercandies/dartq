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
fn ensure_close_paren_adopts_scanner_synthetic_without_double_report() {
    // Unclosed `(` — the scanner inserts a synthetic `)` and reports once
    // (expected_token); the parser adopts that closer silently
    // (`moveSynthetic` semantics).
    let config = dartq_scanner::ScanConfig {
        enable_triple_shift: true,
        enable_augmentations: true,
    };
    let mut files = dartq_source::FileTable::new();
    let file = files.intern(std::path::Path::new("t.dart"));
    let result = dartq_scanner::scan("(", config, file);
    assert_eq!(
        result.diagnostics.len(),
        1,
        "scanner reports the unclosed group"
    );
    let table: &'static dartq_scanner::TokenTable = Box::leak(Box::new(result.tokens));
    let text: &'static str = "(";
    let mut p = crate::Parser::new(
        text,
        table,
        super::collector::latest_features(),
        super::collector::Collector::default(),
    );
    let open = p.peek_span();
    assert_eq!(
        p.cursor.resolve(open.id).0,
        dartq_scanner::TokenKind::OpenParen
    );
    let close = p.ensure_close_paren(open);
    assert!(p.cursor.resolve(close.id).1, "adopted synthetic");
    assert!(
        p.listener.error_codes().is_empty(),
        "no parser-side double report"
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
        let mut p = crate::Parser::new(source, table, latest_features(), listener);
        p.parse_unit();
        let eof = p.cursor.at_eof();
        listener = p.listener;
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
fn unclosed_block_body_at_eof_is_reported_once_by_the_scanner() {
    // `void main() {` — the block never closes. Official pipeline: the
    // scanner's `unmatchedBeginGroup` inserts the synthetic `}` and reports
    // `expected_token` "Expected to find '}'." pinned at the EOF offset
    // (oracle `dart analyze --format=json` on the fixture: one expected_token
    // at offset = source length); the parser's `ensureBlock`
    // (parser_impl.dart:4984) adopts the real `{` with its (synthetic)
    // endGroup silently — the parser never re-reports a missing `}`.
    let source = include_str!("fixtures/eof_unclosed_block.dart");
    let (table, scan_diags) = scan_leaked(source);
    assert_eq!(scan_diags.len(), 1, "{scan_diags:?}");
    let d = &scan_diags[0];
    assert_eq!(d.code.name, "expected_token", "{scan_diags:?}");
    assert_eq!(d.message, "Expected to find '}'.");
    assert_eq!(d.range.offset as usize, source.len(), "{scan_diags:?}");
    assert_eq!(d.range.length, 0, "{scan_diags:?}");

    // Parser side: no double report; the synthetic closer terminates the
    // body loop and the unit consumes to EOF.
    let (listener, at_eof) = parse_collected(source, table);
    assert!(
        listener.error_codes().is_empty(),
        "parser re-reported the scanner's missing close brace: {:?}",
        listener.error_codes()
    );
    assert!(at_eof, "parse did not reach EOF");
}

#[test]
fn unclosed_class_body_at_eof_is_reported_once_by_the_scanner() {
    // `class C {` — same official shape as the unclosed function body: the
    // scanner reports the missing `}` once (unmatchedBeginGroup, pinned at
    // the EOF offset) and the parser's `ensureBlock` (BlockKind.class
    // declaration, parser_impl.dart:3127) adopts the real `{` + synthetic
    // endGroup without a parser-side report.
    let source = include_str!("fixtures/eof_unclosed_class_body.dart");
    let (table, scan_diags) = scan_leaked(source);
    assert_eq!(scan_diags.len(), 1, "{scan_diags:?}");
    let d = &scan_diags[0];
    assert_eq!(d.code.name, "expected_token", "{scan_diags:?}");
    assert_eq!(d.message, "Expected to find '}'.");
    assert_eq!(d.range.offset as usize, source.len(), "{scan_diags:?}");

    let (listener, at_eof) = parse_collected(source, table);
    assert!(
        listener.error_codes().is_empty(),
        "parser re-reported the scanner's missing close brace: {:?}",
        listener.error_codes()
    );
    assert!(at_eof, "parse did not reach EOF");
}

#[test]
fn unclosed_paren_and_square_at_eof_report_once_and_parse_cleanly() {
    // Cascade shapes: `(` and `[` left open at EOF go through the same
    // one-guard location as `{` — the scanner's unmatchedBeginGroup inserts
    // the synthetic closer and reports once at the EOF offset, and the
    // parser adopts the synthetic (`moveSynthetic`/`ensureCloseParen`).
    // `void f(` additionally reports `missing_function_body`: with the
    // synthetic `)` in place the body is genuinely absent, and the official
    // `ensureBlock(token, BlockKind.functionBody)` template carries
    // `pseudoSharedCode: PseudoSharedCode.missingFunctionBody`
    // (diagnostic.g.dart:795) — the parser-side report is official, not a
    // double report of the scanner's `)`. `var x = [` closes the (empty)
    // list literal at the synthetic `]` and then trips `ensureSemicolon`
    // at EOF: official `expectedAfterButGot ';'` → analyzer `expected_token`.
    for (source, closer, parser_codes) in [
        ("void f(", ")", vec!["missing_function_body"]),
        ("var x = [", "]", vec!["expected_token"]),
    ] {
        let (table, scan_diags) = scan_leaked(source);
        assert_eq!(scan_diags.len(), 1, "{source}: {scan_diags:?}");
        let d = &scan_diags[0];
        assert_eq!(d.code.name, "expected_token", "{source}: {scan_diags:?}");
        assert_eq!(d.message, format!("Expected to find '{closer}'."));
        assert_eq!(d.range.offset as usize, source.len(), "{source}");

        let (listener, at_eof) = parse_collected(source, table);
        assert_eq!(
            listener.error_codes(),
            parser_codes,
            "{source}: parser diagnostic mismatch"
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
