//! Post-parse drain: `parse_unit_with` converts the parser-inserted side
//! table into scanner tokens for `TokenTable::append_inserted` — the fold
//! the AST bridge needs before any span read.

use super::collector::{Collector, latest_features, scan_ok};
use crate::parse_unit_with;
use dartq_scanner::{ScanConfig, TokenKind, scan};

/// `(kind, byte_offset, byte_len, utf16_offset, synthetic)` per token.
fn snapshot(tokens: &[dartq_scanner::Token]) -> Vec<(TokenKind, u32, u32, u32, bool)> {
    tokens
        .iter()
        .map(|t| {
            (
                t.kind,
                t.byte_offset,
                t.byte_len,
                t.utf16_offset,
                t.is_synthetic(),
            )
        })
        .collect()
}

fn parse(source: &'static str) -> (Collector, Vec<dartq_scanner::Token>) {
    let (table, text) = scan_ok(source);
    let mut listener = Collector::default();
    let tokens = parse_unit_with(text, table, latest_features(), &mut listener);
    (listener, tokens)
}

#[test]
fn closer_split_drains_as_non_synthetic_gt_tokens() {
    let src = "List<List<int>> x;";
    let (listener, tokens) = parse(src);
    assert!(listener.errors.is_empty());
    // `>>` at 13 splits into the consumed leading `>` and the queued
    // remainder — both plain code, so not synthetic.
    assert_eq!(
        snapshot(&tokens),
        vec![
            (TokenKind::Gt, 13, 1, 13, false),
            (TokenKind::Gt, 14, 1, 14, false),
        ]
    );
    // Full consumer round-trip: folding keeps every id resolvable and the
    // remainder's lexeme sliceable (the span-derivation precondition).
    let config = ScanConfig {
        enable_triple_shift: true,
        enable_augmentations: true,
    };
    let mut files = dartq_source::FileTable::new();
    let file = files.intern(std::path::Path::new("test.dart"));
    let mut table = scan(src, config, file).tokens;
    let orig_len = table.len();
    table.append_inserted(tokens);
    assert_eq!(table.last().kind, TokenKind::Eof);
    assert_eq!(table.get(orig_len - 1).map(|t| t.kind), Some(TokenKind::Gt));
    assert_eq!(table.lexeme(src, orig_len - 1), ">");
}

#[test]
fn byte_offsets_walk_multibyte_source() {
    // `'中文'` precedes the split: each CJK char is 1 UTF-16 unit but 3
    // UTF-8 bytes, so the remainders' byte offsets shift by +4 while their
    // UTF-16 offsets stay at 27/28.
    let src = "var s = '中文'; List<List<int>> x;";
    let (_, tokens) = parse(src);
    assert_eq!(
        snapshot(&tokens),
        vec![
            (TokenKind::Gt, 31, 1, 27, false),
            (TokenKind::Gt, 32, 1, 28, false),
        ]
    );
}

#[test]
fn recovery_synthetic_identifier_is_zero_width() {
    // Missing name in a `var` declaration: the official recovery inserts a
    // synthetic identifier at the offending token's offset — empty lexeme,
    // so zero-width (byte_len 0 keeps table slicing valid).
    let src = "void f() { var = 3; }";
    let (listener, tokens) = parse(src);
    assert_eq!(
        listener.error_codes(),
        vec!["missing_identifier".to_string()]
    );
    assert_eq!(
        snapshot(&tokens),
        vec![(TokenKind::Identifier, 15, 0, 15, true)]
    );
}

#[test]
fn clean_parse_drains_empty_and_listener_round_trips() {
    let (listener, tokens) = parse("var x = 1;");
    assert!(tokens.is_empty());
    assert!(listener.errors.is_empty());
    assert!(!listener.log.is_empty(), "listener received events");
}

#[test]
fn drain_preserves_insertion_id_order_including_consumed_entries() {
    // `>>>` splits over two consume_closer rounds: four side-table entries —
    // the GtGt remainder is consumed by the second round yet must survive
    // the drain (its id lives on in emitted events).
    let src = "List<List<List<int>>> x;";
    let (_, tokens) = parse(src);
    assert_eq!(
        snapshot(&tokens),
        vec![
            (TokenKind::Gt, 18, 1, 18, false),
            (TokenKind::GtGt, 19, 2, 19, false),
            (TokenKind::Gt, 19, 1, 19, false),
            (TokenKind::Gt, 20, 1, 20, false),
        ]
    );
}
