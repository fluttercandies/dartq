//! Unit tests: builder invariants (post-order, contiguous child ranges,
//! pairing table, payload slots, error propagation, speculation rollback),
//! table accessors, span derivation, and the kind-table lockstep guards
//! (layout spec §8 R4).

#![allow(clippy::cast_possible_truncation)] // ids/offsets are u32 by design
#![allow(clippy::enum_glob_use)] // tests read best over the full kind set

use std::path::Path;

use dartiq_scanner::{ScanConfig, scan};
use dartiq_source::FileTable;

use super::kind::{accepts_begin, payload_len};
use crate::{AstBuilder, Marker, NodeKind, SyntaxTable, byte_span, utf16_span};

const CONFIG: ScanConfig = ScanConfig {
    enable_triple_shift: true,
    enable_augmentations: true,
};

fn scan_of(source: &str, files: &mut FileTable, name: &str) -> dartiq_scanner::ScanResult {
    scan(source, CONFIG, files.intern(Path::new(name)))
}

/// Builds `unit [ block [ leaf ×3 ] ]` over token indices 0..5.
fn sample_unit() -> SyntaxTable {
    let mut b = AstBuilder::new();
    b.begin(NodeKind::CompilationUnit, 0);
    b.begin(NodeKind::Block, 1);
    b.leaf(NodeKind::LiteralInt, 1);
    b.leaf(NodeKind::LiteralInt, 2);
    b.leaf(NodeKind::LiteralInt, 3);
    b.end(NodeKind::Block, 4);
    b.end(NodeKind::CompilationUnit, 5);
    b.finish()
}

#[test]
fn post_order_layout_and_accessors() {
    let ast = sample_unit();
    assert_eq!(ast.len(), 5);
    assert_eq!(ast.root(), Some(4));

    // Post-order: leaves 0-2, block 3, unit 4.
    assert_eq!(ast.kind(0), NodeKind::LiteralInt);
    assert_eq!(ast.kind(3), NodeKind::Block);
    assert_eq!(ast.kind(4), NodeKind::CompilationUnit);

    assert_eq!(ast.token_range(0), 1..2);
    assert_eq!(ast.token_range(3), 1..4);
    assert_eq!(ast.token_range(4), 0..5);

    assert_eq!(ast.children(3), 0..3);
    assert_eq!(ast.children(0), 0..0, "leaf has an empty child range");
    assert_eq!(ast.children(4), 0..4);

    assert_eq!(ast.parent(0), Some(3));
    assert_eq!(ast.parent(3), Some(4));
    assert_eq!(ast.parent(4), None, "root has no parent");

    assert!(!ast.is_error(3));
}

#[test]
fn payload_slot_patch_and_read() {
    let mut b = AstBuilder::new();
    b.begin(NodeKind::CompilationUnit, 0);
    // ClassDeclaration: 6 pool words (ModSet 2 + class/extends/with/implements).
    b.begin(NodeKind::ClassDeclaration, 0);
    b.patch(2, 7); // class keyword token
    b.patch(3, 9); // extends clause anchor
    b.end(NodeKind::ClassDeclaration, 10);
    b.end(NodeKind::CompilationUnit, 10);
    let ast = b.finish();

    let payload = ast.payload(ast.root().unwrap() - 1);
    assert_eq!(payload.len(), 6);
    assert_eq!(payload[2], 7);
    assert_eq!(payload[3], 9);
    assert_eq!(payload[0], 0, "unpatched fields stay zero");
    assert!(ast.payload(ast.root().unwrap()).is_empty());
}

#[test]
fn leaf_writes_token_into_payload() {
    let mut b = AstBuilder::new();
    b.begin(NodeKind::CompilationUnit, 0);
    b.leaf(NodeKind::Identifier, 0);
    b.end(NodeKind::CompilationUnit, 1);
    let ast = b.finish();
    // Identifier: (token, ctx); leaf auto-fills word 0 with the token.
    assert_eq!(ast.payload(0), &[0, 0]);
}

#[test]
fn shared_begin_pairs() {
    // A `(`-opened structure can seal as either shape (collapse map §2 #12/#13).
    let mut b = AstBuilder::new();
    b.begin(NodeKind::ParenthesizedExpression, 0);
    b.leaf(NodeKind::LiteralInt, 1);
    b.end(NodeKind::RecordLiteral, 2);
    let ast = b.finish();
    assert_eq!(ast.kind(1), NodeKind::RecordLiteral);
    assert_eq!(
        ast.payload(1).len(),
        usize::from(payload_len(NodeKind::RecordLiteral))
    );
}

#[test]
#[cfg(debug_assertions)]
fn pairing_rejects_mismatched_end() {
    let mut b = AstBuilder::new();
    b.begin(NodeKind::Block, 0);
    let result = std::panic::catch_unwind(move || {
        b.end(NodeKind::ParenthesizedExpression, 1);
    });
    assert!(
        result.is_err(),
        "Block must not seal as ParenthesizedExpression"
    );
}

#[test]
fn pairing_table_shape() {
    use NodeKind::*;
    // Same-kind pairing is the default.
    assert!(accepts_begin(Block, Block));
    assert!(!accepts_begin(Block, IfStatement));
    // The five shared-begin groups (collapse map §2 #6, #7, #10, #11, #12/#13).
    assert!(accepts_begin(TopLevelFields, Fields));
    assert!(accepts_begin(TopLevelFields, TopLevelFields));
    assert!(accepts_begin(ForIn, ForStatement));
    assert!(accepts_begin(InvalidAwaitExpression, AwaitExpression));
    assert!(accepts_begin(InvalidYieldStatement, YieldStatement));
    assert!(accepts_begin(RecordLiteral, ParenthesizedExpression));
    assert!(
        !accepts_begin(ForStatement, ForIn),
        "pairing is directional"
    );
}

#[test]
fn every_end_accepts_its_canonical_begin() {
    // The builder contract on `end`: the parser opens the canonical kind of a
    // shared-begin group, so every end kind must accept its own canonical
    // open. Same-kind acceptance makes this trivially true except for the
    // five shared-begin ends, checked directly.
    for raw in 0..NodeKind::KIND_COUNT {
        let end = NodeKind::from_raw(raw);
        assert!(accepts_begin(end, canonical_of(end)), "{end:?}");
    }
}

fn canonical_of(end: NodeKind) -> NodeKind {
    use NodeKind::*;
    match end {
        TopLevelFields => Fields,
        ForIn => ForStatement,
        InvalidAwaitExpression => AwaitExpression,
        InvalidYieldStatement => YieldStatement,
        RecordLiteral => ParenthesizedExpression,
        other => other,
    }
}

#[test]
fn canonical_begin_is_never_narrower_than_its_ends() {
    // Slot reservation happens at begin; the canonical (first) begin of each
    // shared-begin group must be at least as wide as every end it may seal
    // as — else `end`'s debug assertion fires (builder contract).
    let groups: [&[NodeKind]; 5] = [
        &[NodeKind::Fields, NodeKind::TopLevelFields],
        &[NodeKind::ForStatement, NodeKind::ForIn],
        &[NodeKind::AwaitExpression, NodeKind::InvalidAwaitExpression],
        &[NodeKind::YieldStatement, NodeKind::InvalidYieldStatement],
        &[NodeKind::ParenthesizedExpression, NodeKind::RecordLiteral],
    ];
    for group in groups {
        let wide = payload_len(group[0]);
        for &end in group {
            assert!(wide >= payload_len(end), "{:?} group width", group[0]);
        }
    }
}

#[test]
fn payload_width_ceiling() {
    use NodeKind::*;
    // The spec tolerates exactly one fat outlier (7 pool words, layout spec
    // §1.5); no other kind may reach it.
    let mut max = 0u8;
    for raw in 0..NodeKind::KIND_COUNT {
        max = max.max(payload_len(NodeKind::from_raw(raw)));
    }
    assert_eq!(max, 7);
    assert_eq!(payload_len(FormalParameter), 7);
}

#[test]
fn error_flag_propagates_to_all_ancestors() {
    let mut b = AstBuilder::new();
    b.begin(NodeKind::CompilationUnit, 0);
    b.begin(NodeKind::Block, 0);
    b.leaf(NodeKind::LiteralInt, 1);
    b.mark_error();
    b.leaf(NodeKind::LiteralInt, 2);
    b.end(NodeKind::Block, 3);
    b.end(NodeKind::CompilationUnit, 3);
    let ast = b.finish();
    // Post-order: [leaf, leaf, block, unit]; the block was marked, the unit
    // inherited the bit at seal time.
    assert!(!ast.is_error(0));
    assert!(!ast.is_error(1));
    assert!(ast.is_error(2), "marked node");
    assert!(ast.is_error(3), "root");
}

#[test]
fn rollback_truncates_nodes_pool_and_error_bits() {
    let mut b = AstBuilder::new();
    b.begin(NodeKind::CompilationUnit, 0);
    b.leaf(NodeKind::LiteralInt, 0);

    // Speculated subtree: a node, a patch, and an error mark.
    let marker: Marker = b.mark();
    b.begin(NodeKind::ClassDeclaration, 1);
    b.patch(0, 42);
    b.mark_error();
    b.leaf(NodeKind::LiteralInt, 1);
    b.end(NodeKind::ClassDeclaration, 2);
    b.rollback(marker);

    // Committed suffix still builds cleanly and carries no phantom error.
    b.begin(NodeKind::Block, 1);
    b.leaf(NodeKind::LiteralInt, 1);
    b.end(NodeKind::Block, 2);
    b.end(NodeKind::CompilationUnit, 2);
    let ast = b.finish();

    assert_eq!(ast.len(), 4, "unit + leaf + block + leaf");
    // Post-order: [leaf, leaf, block, unit].
    assert_eq!(ast.kind(2), NodeKind::Block);
    assert!(!ast.is_error(2), "speculated error mark must not leak");
    assert!(
        !ast.is_error(3),
        "root must not inherit a rolled-back error"
    );
    assert!(ast.payload(2).is_empty(), "block is span-only");
    assert_eq!(
        ast.memory_footprint(),
        4 * 24 + 2 * 4,
        "arena and pool fully truncated to the committed prefix"
    );
}

#[test]
fn spans_derive_from_token_table() {
    let source = "ab // note\nf(é, \"x\")";
    let mut files = FileTable::new();
    let result = scan_of(source, &mut files, "a.dart");

    let mut b = AstBuilder::new();
    b.begin(NodeKind::CompilationUnit, 0);
    b.leaf(NodeKind::Identifier, 0); // f
    b.end(NodeKind::CompilationUnit, result.tokens.len() as u32);
    let ast = b.finish();

    // CompilationUnit exception: [0, eof.end] regardless of leading trivia.
    let root = ast.root().unwrap();
    assert_eq!(byte_span(&result.tokens, &ast, root), 0..source.len());
    let utf16_len: usize = source.chars().map(char::len_utf16).sum();
    assert_eq!(
        utf16_span(source, &result.tokens, &ast, root),
        0..utf16_len as u32
    );

    // Leaf `f`: spans straight from the token record.
    assert_eq!(byte_span(&result.tokens, &ast, 0), 0..2);
    assert_eq!(utf16_span(source, &result.tokens, &ast, 0), 0..2);
}

#[test]
fn empty_token_range_is_zero_width() {
    let source = "a b";
    let mut files = FileTable::new();
    let result = scan_of(source, &mut files, "b.dart");

    let mut b = AstBuilder::new();
    b.begin(NodeKind::CompilationUnit, 0);
    // All-synthetic node anchored at token 1 (zero-width at that token).
    b.begin(NodeKind::Block, 1);
    b.end(NodeKind::Block, 1);
    b.end(NodeKind::CompilationUnit, result.tokens.len() as u32);
    let ast = b.finish();

    assert_eq!(byte_span(&result.tokens, &ast, 0), 2..2);
    assert_eq!(utf16_span(source, &result.tokens, &ast, 0), 2..2);
}

#[test]
fn ast_key_equality() {
    use dartiq_language::{FeatureSet, LanguageVersion};
    use dartiq_source::content_hash;

    let key = |bytes: &[u8], features: &FeatureSet| {
        crate::AstKey::new(content_hash(bytes), crate::FeatureSetId::of(features))
    };
    let latest = FeatureSet::latest();
    let dart3 = FeatureSet::for_language_version(LanguageVersion::new(3, 0));
    let a = key(b"x", &latest);
    assert_eq!(a, key(b"x", &latest));
    assert_ne!(a, key(b"y", &latest));
    assert_ne!(a, key(b"x", &dart3), "feature fingerprint participates");
    assert_eq!(a.parser_version.raw(), 1);
    assert_eq!(
        a.feature_set.raw(),
        latest.fingerprint(),
        "FeatureSetId wraps the language fingerprint"
    );
}
