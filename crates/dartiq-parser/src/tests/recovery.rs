//! Recovery primitive tests.

use super::collector::setup;

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
    let semi = p.expect_kind(dartiq_scanner::TokenKind::Semicolon);
    assert_eq!(
        p.cursor.resolve(semi.id).0,
        dartiq_scanner::TokenKind::Semicolon
    );
    assert_eq!(p.listener.error_codes(), vec!["expected_token".to_string()]);
}

#[test]
fn skip_to_sync_stops_at_boundary() {
    let mut p = setup("a b ; c");
    p.skip_to_sync(&[dartiq_scanner::TokenKind::Semicolon]);
    assert_eq!(p.cursor.lexeme(p.peek_id()), ";");
}

#[test]
fn ensure_close_paren_adopts_scanner_synthetic_without_double_report() {
    // Unclosed `(` — the scanner inserts a synthetic `)` and reports once
    // (expected_token); the parser adopts that closer silently
    // (`moveSynthetic` semantics).
    let config = dartiq_scanner::ScanConfig {
        enable_triple_shift: true,
        enable_augmentations: true,
    };
    let mut files = dartiq_source::FileTable::new();
    let file = files.intern(std::path::Path::new("t.dart"));
    let result = dartiq_scanner::scan("(", config, file);
    assert_eq!(
        result.diagnostics.len(),
        1,
        "scanner reports the unclosed group"
    );
    let table: &'static dartiq_scanner::TokenTable = Box::leak(Box::new(result.tokens));
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
        dartiq_scanner::TokenKind::OpenParen
    );
    let close = p.ensure_close_paren(open);
    assert!(p.cursor.resolve(close.id).1, "adopted synthetic");
    assert!(
        p.listener.error_codes().is_empty(),
        "no parser-side double report"
    );
}

#[test]
fn parse_unit_consumes_everything_without_progress_hangs() {
    let mut p = setup("a + b )) ) }} ; ;");
    p.parse_unit();
    assert!(p.cursor.at_eof());
}
