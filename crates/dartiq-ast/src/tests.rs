//! Unit tests: builder invariants (post-order, contiguous child ranges,
//! pairing table, payload slots, error propagation, speculation rollback),
//! table accessors, span derivation, and the kind-table lockstep guards
//! (layout spec §8 R4).

#![allow(clippy::cast_possible_truncation)] // ids/offsets are u32 by design
#![allow(clippy::enum_glob_use)] // tests read best over the full kind set

use std::path::Path;

use dartiq_scanner::{Keyword, ScanConfig, TokenKind, scan};
use dartiq_source::FileTable;

use super::derived;
use super::kind::{accepts_begin, leaf_shape, payload_len};
use crate::{
    AstBuilder, LeafShape, Marker, NodeKind, SyntaxTable, TOKEN_NONE, byte_span, utf16_span,
};

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
    // ClassDeclaration: 4 pool words (class/extends/with/implements — the
    // ModSet moved into the header flag bits, #174).
    b.begin(NodeKind::ClassDeclaration, 0);
    b.set_modifiers(0b10); // abstract bit (lane-owned semantics)
    b.patch(0, 7); // class keyword token
    b.patch(1, 9); // extends clause anchor
    b.patch(2, TOKEN_NONE); // with absent
    b.patch(3, TOKEN_NONE); // implements absent
    b.end(NodeKind::ClassDeclaration, 10);
    b.end(NodeKind::CompilationUnit, 10);
    let ast = b.finish();

    let payload = ast.payload(ast.root().unwrap() - 1);
    assert_eq!(payload.len(), 4);
    assert_eq!(payload[0], 7);
    assert_eq!(payload[1], 9);
    assert_eq!(payload[2], TOKEN_NONE);
    assert!(ast.payload(ast.root().unwrap()).is_empty());
    assert_eq!(
        derived::mod_flags(&ast, ast.root().unwrap() - 1),
        0b10,
        "modifier bits live in the header flags"
    );
    assert!(!ast.is_error(ast.root().unwrap() - 1));
}

#[test]
fn token_leaf_carries_no_payload() {
    let mut b = AstBuilder::new();
    b.begin(NodeKind::CompilationUnit, 0);
    b.leaf(NodeKind::LiteralInt, 3);
    b.end(NodeKind::CompilationUnit, 5);
    let ast = b.finish();
    // TokenLeaf (#151): no payload word — the token is header.first_token,
    // the span exactly [token, token + 1).
    assert!(ast.payload(0).is_empty());
    assert_eq!(ast.token_range(0), 3..4);
}

#[test]
fn fielded_leaf_patterns() {
    // Fielded kinds never go through leaf(): begin reserves the slot, patch
    // fills every word explicitly (a missed write trips the poison sweep at
    // seal time in debug builds).
    let mut b = AstBuilder::new();
    b.begin(NodeKind::CompilationUnit, 0);
    // Identifier { ctx } — the token was eliminated as the span head (#174).
    b.begin(NodeKind::Identifier, 1);
    b.patch(0, 2);
    b.end(NodeKind::Identifier, 2);
    // CascadeAccess { token, null_aware } — bool bit set.
    b.begin(NodeKind::CascadeAccess, 2);
    b.patch(0, 2);
    b.patch(1, 1);
    b.end(NodeKind::CascadeAccess, 3);
    b.end(NodeKind::CompilationUnit, 3);
    let ast = b.finish();
    assert_eq!(ast.payload(0), &[2]);
    assert_eq!(ast.payload(1), &[2, 1]);
}

#[test]
fn shared_begin_pairs() {
    // A `(`-opened structure seals as either shape (collapse map §2 #12/#13).
    // The canonical open is the wider RecordLiteral (#151 flip): it reserves
    // the const slot; ParenthesizedExpression narrow-seals under it.
    let mut b = AstBuilder::new();
    b.begin(NodeKind::RecordLiteral, 0);
    b.patch(0, TOKEN_NONE); // no `const`
    b.leaf(NodeKind::LiteralInt, 1);
    b.end(NodeKind::RecordLiteral, 2);
    let mut b2 = AstBuilder::new();
    b2.begin(NodeKind::RecordLiteral, 0);
    b2.leaf(NodeKind::LiteralInt, 1);
    b2.end(NodeKind::ParenthesizedExpression, 2);
    let ast = b.finish();
    assert_eq!(ast.kind(1), NodeKind::RecordLiteral);
    assert_eq!(
        ast.payload(1).len(),
        usize::from(payload_len(NodeKind::RecordLiteral))
    );
    let ast2 = b2.finish();
    assert_eq!(ast2.kind(1), NodeKind::ParenthesizedExpression);
    assert!(ast2.payload(1).is_empty(), "narrow seal to the width-0 end");
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
    assert!(accepts_begin(RecordLiteral, RecordLiteral));
    assert!(
        accepts_begin(ParenthesizedExpression, RecordLiteral),
        "#12/#13: canonical open is the wider RecordLiteral (#151)"
    );
    assert!(
        !accepts_begin(RecordLiteral, ParenthesizedExpression),
        "pairing is directional: the width-0 kind cannot reserve the slot"
    );
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
        ParenthesizedExpression | RecordLiteral => RecordLiteral,
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
        // Canonical first — the wider kind reserves the slot (#151 flip).
        &[NodeKind::RecordLiteral, NodeKind::ParenthesizedExpression],
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
    // The spec tolerates exactly one fat outlier (layout spec §1.5); no
    // other kind may reach it. #174 brought the outlier down from 7 to 5.
    let mut max = 0u8;
    for raw in 0..NodeKind::KIND_COUNT {
        max = max.max(payload_len(NodeKind::from_raw(raw)));
    }
    assert_eq!(max, 5);
    assert_eq!(payload_len(FormalParameter), 5);
}

#[test]
fn leaf_shape_width_lockstep() {
    // Fielded is exactly "has payload words"; TokenLeaf kinds are width 0
    // (their token lives in header.first_token); SpanOnly covers the rest of
    // the width-0 set (#151).
    for raw in 0..NodeKind::KIND_COUNT {
        let kind = NodeKind::from_raw(raw);
        let shape = leaf_shape(kind);
        let width = payload_len(kind);
        assert_eq!(
            shape == LeafShape::Fielded,
            width > 0,
            "{kind:?}: shape/width disagree"
        );
        if shape == LeafShape::TokenLeaf {
            assert_eq!(width, 0, "{kind:?}");
        }
    }
    // Spot checks across the three shapes.
    use NodeKind::*;
    assert_eq!(leaf_shape(LiteralInt), LeafShape::TokenLeaf);
    assert_eq!(leaf_shape(ThisExpression), LeafShape::TokenLeaf);
    assert_eq!(leaf_shape(StringLiteral), LeafShape::SpanOnly);
    assert_eq!(leaf_shape(Send), LeafShape::SpanOnly);
    // #174 width-0 flips stay SpanOnly (containers/multi-token handles).
    assert_eq!(leaf_shape(Type), LeafShape::SpanOnly);
    assert_eq!(leaf_shape(TopLevelFields), LeafShape::SpanOnly);
    assert_eq!(leaf_shape(ConstantPattern), LeafShape::SpanOnly);
    assert_eq!(leaf_shape(Identifier), LeafShape::Fielded);
    assert_eq!(leaf_shape(ParenthesizedCondition), LeafShape::Fielded);
}

#[test]
fn narrow_seal_paths() {
    // Shared-begin groups open as the canonical (widest) kind and may seal
    // as a narrower end — including the width-0 ends (#151/#174).
    let mut b = AstBuilder::new();
    b.begin(NodeKind::CompilationUnit, 0);
    // Fields (1 word: kind) → TopLevelFields (0 words: the ModSet lives in
    // the header flag bits).
    b.begin(NodeKind::Fields, 0);
    b.patch(0, 0);
    b.leaf(NodeKind::LiteralInt, 1);
    b.end(NodeKind::TopLevelFields, 2);
    // AwaitExpression (width 0) → InvalidAwaitExpression (width 0): the
    // width-0 flip needs no patching at all.
    b.begin(NodeKind::AwaitExpression, 2);
    b.leaf(NodeKind::LiteralInt, 3);
    b.end(NodeKind::InvalidAwaitExpression, 4);
    b.end(NodeKind::CompilationUnit, 4);
    let ast = b.finish();
    // Post-order: [leaf, TopLevelFields, leaf, InvalidAwaitExpression, unit].
    assert_eq!(ast.kind(1), NodeKind::TopLevelFields);
    assert!(
        ast.payload(1).is_empty(),
        "slot narrowed to the width-0 end"
    );
    assert_eq!(ast.kind(3), NodeKind::InvalidAwaitExpression);
    assert!(ast.payload(3).is_empty());
}

#[test]
#[cfg(debug_assertions)]
fn root_sealed_last() {
    // The compilation unit is sealed last; every event after the seal is a
    // parser bug and must fail loudly in debug builds.
    let build_after_root = |emit: fn(&mut AstBuilder)| {
        let mut b = AstBuilder::new();
        b.begin(NodeKind::CompilationUnit, 0);
        b.end(NodeKind::CompilationUnit, 1);
        emit(&mut b);
    };
    let result = std::panic::catch_unwind(|| {
        build_after_root(|b| b.leaf(NodeKind::LiteralInt, 0));
    });
    assert!(result.is_err(), "leaf after the root must fail");
    let result = std::panic::catch_unwind(|| {
        build_after_root(|b| b.begin(NodeKind::Block, 0));
    });
    assert!(result.is_err(), "begin after the root must fail");
    // Sealing with nothing open fails as well (pre-existing guard).
    let result = std::panic::catch_unwind(|| {
        let mut b = AstBuilder::new();
        b.end(NodeKind::CompilationUnit, 0);
    });
    assert!(result.is_err(), "end without a begin must fail");
}

#[test]
#[cfg(debug_assertions)]
fn leaf_rejects_fielded_kind() {
    // Fielded kinds reserve their slot at begin and fill it via patch — a
    // leaf emission could never back-fill it (#151/#174 contract).
    let result = std::panic::catch_unwind(|| {
        let mut b = AstBuilder::new();
        b.begin(NodeKind::CompilationUnit, 0);
        b.leaf(NodeKind::Identifier, 0);
    });
    assert!(result.is_err(), "leaf must reject Fielded kinds");
}

#[test]
#[cfg(debug_assertions)]
fn leaf_span_rejects_token_leaf_kind() {
    // TokenLeaf spans are forced to [token, token + 1) — leaf_span would
    // let a malformed span through.
    let result = std::panic::catch_unwind(|| {
        let mut b = AstBuilder::new();
        b.begin(NodeKind::CompilationUnit, 0);
        b.leaf_span(NodeKind::LiteralInt, 0, 1);
    });
    assert!(result.is_err(), "leaf_span must reject TokenLeaf kinds");
}

#[test]
#[cfg(debug_assertions)]
fn leaf_span_rejected_after_root_seal() {
    let result = std::panic::catch_unwind(|| {
        let mut b = AstBuilder::new();
        b.begin(NodeKind::CompilationUnit, 0);
        b.end(NodeKind::CompilationUnit, 1);
        b.leaf_span(NodeKind::StringLiteral, 0, 1);
    });
    assert!(result.is_err(), "leaf_span after the root must fail");
}

#[test]
#[cfg(debug_assertions)]
fn poison_sweep_catches_missed_patch() {
    // Every Fielded word must be written explicitly: begin poison-fills the
    // slot, end sweeps it — a missed patch fails at seal time, never reads
    // back as token 0.
    let result = std::panic::catch_unwind(|| {
        let mut b = AstBuilder::new();
        b.begin(NodeKind::CompilationUnit, 0);
        b.begin(NodeKind::Break, 0); // width 1: has_target
        b.end(NodeKind::Break, 1); // no patch
    });
    assert!(result.is_err(), "a missed patch must fail at seal time");
}

#[test]
fn derived_accessors_read_eliminated_words() {
    // ModSet (#174): header flag bits + derived first modifier.
    let source = "abstract class A {}";
    let mut files = FileTable::new();
    let result = scan_of(source, &mut files, "a.dart");
    let mut b = AstBuilder::new();
    b.begin(NodeKind::CompilationUnit, 0);
    b.begin(NodeKind::ClassDeclaration, 0);
    b.set_modifiers(0b10); // abstract bit (lane-owned semantics)
    b.patch(0, 1); // class keyword
    b.patch(1, TOKEN_NONE);
    b.patch(2, TOKEN_NONE);
    b.patch(3, TOKEN_NONE);
    b.end(NodeKind::ClassDeclaration, result.tokens.len() as u32);
    b.end(NodeKind::CompilationUnit, result.tokens.len() as u32);
    let ast = b.finish();
    let class_id = ast.root().unwrap() - 1;
    assert_eq!(derived::mod_flags(&ast, class_id), 0b10);
    assert_eq!(derived::first_modifier(&ast, class_id), Some(0));
    assert!(derived::head_is(
        &result.tokens,
        &ast,
        class_id,
        Keyword::Abstract
    ));

    // Type `?` (#174): nullable ⟺ the last span token is `?`.
    let mut files = FileTable::new();
    let nullable = scan_of("T? x;", &mut files, "b.dart");
    let mut b = AstBuilder::new();
    b.begin(NodeKind::CompilationUnit, 0);
    b.begin(NodeKind::Type, 0);
    b.end(NodeKind::Type, 2);
    b.end(NodeKind::CompilationUnit, 4);
    let ast = b.finish();
    assert!(derived::question(&nullable.tokens, &ast, 0));

    let mut files = FileTable::new();
    let plain = scan_of("T x;", &mut files, "c.dart");
    let mut b = AstBuilder::new();
    b.begin(NodeKind::CompilationUnit, 0);
    b.begin(NodeKind::Type, 0);
    b.end(NodeKind::Type, 1);
    b.end(NodeKind::CompilationUnit, 3);
    let ast = b.finish();
    assert!(!derived::question(&plain.tokens, &ast, 0));

    // Closing-token derivation: `break;` — the `;` is the last span token.
    let mut files = FileTable::new();
    let brk = scan_of("break;", &mut files, "d.dart");
    let mut b = AstBuilder::new();
    b.begin(NodeKind::CompilationUnit, 0);
    b.begin(NodeKind::Break, 0);
    b.patch(0, 0);
    b.end(NodeKind::Break, brk.tokens.len() as u32);
    b.end(NodeKind::CompilationUnit, brk.tokens.len() as u32);
    let ast = b.finish();
    let semi = derived::last_token(&brk.tokens, &ast, 0).unwrap();
    assert_eq!(
        brk.tokens.get(semi as usize).unwrap().kind,
        TokenKind::Semicolon
    );

    // `Indexed` open bracket after the null-aware `?`.
    let mut files = FileTable::new();
    let idx = scan_of("a?[0]", &mut files, "e.dart");
    let mut b = AstBuilder::new();
    b.begin(NodeKind::CompilationUnit, 0);
    b.begin(NodeKind::Indexed, 1); // span head is the `?`
    b.patch(0, 1); // question word
    b.end(NodeKind::Indexed, idx.tokens.len() as u32);
    b.end(NodeKind::CompilationUnit, idx.tokens.len() as u32);
    let ast = b.finish();
    assert_eq!(derived::indexed_open(&ast, 0, ast.payload(0)[0]), 2);

    // Head-when-present keyword: `late var x;` — `late` is the head.
    let mut files = FileTable::new();
    let late = scan_of("late var x;", &mut files, "f.dart");
    let mut b = AstBuilder::new();
    b.begin(NodeKind::CompilationUnit, 0);
    b.begin(NodeKind::VariablesDeclaration, 0);
    b.patch(0, 1); // modifier word: `var`
    b.end(NodeKind::VariablesDeclaration, late.tokens.len() as u32);
    b.end(NodeKind::CompilationUnit, late.tokens.len() as u32);
    let ast = b.finish();
    assert!(derived::head_is(&late.tokens, &ast, 0, Keyword::Late));
    assert!(!derived::head_is(&late.tokens, &ast, 0, Keyword::Var));
}

#[test]
fn payload_bearing_constructions() {
    // Representative Fielded kinds across domains: every word written
    // explicitly; absent Options read back TOKEN_NONE, never zero
    // (#151/#174; the derivation accessors recompute the eliminated words).
    let mut b = AstBuilder::new();
    b.begin(NodeKind::CompilationUnit, 0);
    // ForParts { lparen, left_sep, right_sep, update_count } — the `for`
    // token is the span head (#174).
    b.begin(NodeKind::ForParts, 1);
    b.patch(0, 2);
    b.patch(1, 4);
    b.patch(2, 6);
    b.patch(3, 2);
    b.leaf(NodeKind::LiteralInt, 3);
    b.leaf(NodeKind::LiteralInt, 5);
    b.end(NodeKind::ForParts, 7);
    // SwitchCase { default: O, colon: O, expression_count } — both absent.
    b.begin(NodeKind::SwitchCase, 7);
    b.patch(0, TOKEN_NONE);
    b.patch(1, TOKEN_NONE);
    b.patch(2, 1);
    b.leaf(NodeKind::LiteralInt, 7);
    b.end(NodeKind::SwitchCase, 8);
    // Break { has_target } / Continue { has_target } — keyword (span head)
    // and `;` (last span token) are derived (#174).
    b.begin(NodeKind::Break, 8);
    b.patch(0, 0);
    b.end(NodeKind::Break, 9);
    b.begin(NodeKind::Continue, 9);
    b.patch(0, 1);
    b.end(NodeKind::Continue, 10);
    // CatchClause { catch: O, comma: O } — `on` is head-when-present; here
    // the `on` form: head is `on`, catch/comma absent.
    b.begin(NodeKind::CatchClause, 10);
    b.patch(0, TOKEN_NONE);
    b.patch(1, TOKEN_NONE);
    b.end(NodeKind::CatchClause, 11);
    // TryStatement { finally: O } — the `try` head is derived; finally absent.
    b.begin(NodeKind::TryStatement, 12);
    b.patch(0, TOKEN_NONE);
    b.end(NodeKind::TryStatement, 13);
    // Conditional { question: T, colon: T }.
    b.begin(NodeKind::Conditional, 13);
    b.patch(0, 13);
    b.patch(1, 14);
    b.leaf(NodeKind::LiteralInt, 13);
    b.leaf(NodeKind::LiteralInt, 14);
    b.leaf(NodeKind::LiteralInt, 15);
    b.end(NodeKind::Conditional, 16);
    // Assert { kind, lparen: T, comma: O } — the `assert` head is derived;
    // comma absent.
    b.begin(NodeKind::Assert, 16);
    b.patch(0, 2); // AssertKind discriminant
    b.patch(1, 17);
    b.patch(2, TOKEN_NONE);
    b.end(NodeKind::Assert, 18);
    b.end(NodeKind::CompilationUnit, 18);
    let ast = b.finish();

    // Post-order ids: ForParts 2, SwitchCase 4, Break 5, Continue 6,
    // CatchClause 7, TryStatement 8, Conditional 12, Assert 13.
    assert_eq!(ast.payload(2), &[2, 4, 6, 2]);
    assert_eq!(
        ast.payload(4),
        &[u32::MAX, u32::MAX, 1],
        "absent Options read as TOKEN_NONE, not zero"
    );
    assert_eq!(ast.payload(5), &[0]);
    assert_eq!(ast.payload(6), &[1]);
    assert_eq!(ast.payload(7), &[u32::MAX, u32::MAX]);
    assert_eq!(ast.payload(8), &[u32::MAX]);
    assert_eq!(ast.payload(12), &[13, 14]);
    assert_eq!(ast.payload(13), &[2, 17, u32::MAX]);
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
    b.patch(1, TOKEN_NONE);
    b.patch(2, TOKEN_NONE);
    b.patch(3, TOKEN_NONE);
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
        4 * 24,
        "arena and pool fully truncated to the committed prefix (token leaves store no pool word)"
    );
}

#[test]
fn spans_derive_from_token_table() {
    let source = "ab // note\nf(é, \"x\")";
    let mut files = FileTable::new();
    let result = scan_of(source, &mut files, "a.dart");

    let mut b = AstBuilder::new();
    b.begin(NodeKind::CompilationUnit, 0);
    // Fielded Identifier: the begin/patch/end pattern (#151/#174 contract);
    // ctx is word 0, the token is the span head.
    b.begin(NodeKind::Identifier, 0);
    b.patch(0, 0);
    b.end(NodeKind::Identifier, 1);
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
    assert_eq!(a.parser_version.raw(), 3);
    assert_eq!(
        a.feature_set.raw(),
        latest.fingerprint(),
        "FeatureSetId wraps the language fingerprint"
    );
}
