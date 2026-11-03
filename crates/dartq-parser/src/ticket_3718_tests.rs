//! Ticket #3718 — recovery-splice ordering: inserted tokens must precede
//! pending remainders.
//!
//! The official `insertParens`/`insertBlock`/`insertSyntheticToken` family
//! (`token_stream_rewriter.dart:29`, `parser_impl.dart:5006`) splices
//! synthetic tokens right after `token` — BEFORE `token.next` — so a
//! split-closer remainder still pending in the stream stays the next token
//! the parser sees. The cursor's side table instead appended recovery
//! synthetics at the END of the pending queue, after the remainder; the
//! drain-to-synthetic advances then ate the remainder silently, and
//! recovery reports anchored on the zero-width synthetic the remainder hid
//! behind (`f<T>>()`: `missing_function_body` printed `2:5|0` against the
//! oracle's `2:5|1`, and the `>` remainder never reached the member loop's
//! `top_level_operator` arm).
//!
//! `detach_queue`/`attach_queue` around the synthesis restores the official
//! splice order — `(` `)` (or `{` `}`) first, then the remainder — in
//! `parse_getter_or_formal_parameters` (members.rs) and
//! `ensure_function_body_block` (statements.rs).

use std::collections::BTreeSet;

use crate::parser::Parser;
use crate::tests::collector::Collector;
use dartq_language::{Feature, FeatureSet};
use dartq_scanner::ScanConfig;

/// Parses `source` at `set` and returns the scanner+parser rows as
/// `(code, start, end)` — UTF-16 offsets, matching the ticket_3674 harness.
fn rows_at(source: &'static str, set: FeatureSet) -> BTreeSet<(String, u32, u32)> {
    let config = ScanConfig::from_feature_set(set);
    let features = crate::ParserFeatures::from_feature_set(set);
    let mut files = dartq_source::FileTable::new();
    let file = files.intern(std::path::Path::new("test.dart"));
    let result = dartq_scanner::scan(source, config, file);
    let mut rows: BTreeSet<(String, u32, u32)> = result
        .diagnostics
        .iter()
        .map(|d| {
            (
                d.code.name.to_owned(),
                d.range.offset,
                d.range.offset + d.range.length,
            )
        })
        .collect();
    let mut parser = Parser::new(
        source,
        Box::leak(Box::new(result.tokens)),
        features,
        Box::leak(Box::new(Collector::default())),
    );
    parser.parse_unit();
    rows.extend(parser.listener.errors.iter().cloned());
    rows
}

fn augment_set() -> FeatureSet {
    FeatureSet::latest().with_experiment(Feature::Augmentations)
}

// ------------------------------------------------------------------
// zero-length synthetic anchor walk
// ------------------------------------------------------------------

#[test]
fn extra_gt_reports_body_on_the_real_gt() {
    // Oracle (`test_typeParameters_extraGt`, paired_tokens_test.dart): the
    // file is `\nf<T>>() => null;` — `f` at offset 1, the extra `>` at
    // offset 5. Recovery parses `f` as a bodiless function and reports
    // `missing_function_body` at offset 5: a zero-length synthetic before
    // the splice-order fix (the `>` remainder was stranded behind the
    // `()`/`{}` pair), the real `>` remainder (offset 5, len 1) after it.
    let rows = rows_at("\nf<T>>() => null;\n", augment_set());
    assert!(
        rows.contains(&("missing_function_body".to_owned(), 5, 6)),
        "expected missing_function_body at (5,6), got {rows:?}"
    );
    assert!(
        !rows.iter().any(|(c, s, e)| c == "missing_function_body" && e == s),
        "length-0 missing_function_body survived the splice: {rows:?}"
    );
    // The `>` remainder stays the top-level follower, so the oracle's
    // `top_level_operator` at (5,6) fires as well.
    assert!(
        rows.contains(&("top_level_operator".to_owned(), 5, 6)),
        "expected top_level_operator at (5,6), got {rows:?}"
    );
}

#[test]
fn non_recovery_semicolon_body_keeps_real_span() {
    // Negative control: `void f();` reports `missing_function_body` on the
    // real `;` (offset 8, len 1) — the walk must not shift real anchors.
    let rows = rows_at("void f();\n", augment_set());
    assert!(
        rows.contains(&("missing_function_body".to_owned(), 8, 9)),
        "expected missing_function_body at (8,9), got {rows:?}"
    );
}

#[test]
fn unclosed_angle_member_reports_at_real_tokens() {
    // `issue_000032` shape: `C<` inside a class and at top level. The
    // oracle anchors every row of this recovery on a real token; pin the
    // absence of zero-width `missing_function_body` rows so the splice fix
    // can't regress into synthetic anchors here.
    let rows = rows_at("class C {\n  C<\n}\n\nmain() {\n  C<\n}\n", augment_set());
    assert!(
        rows.iter().all(|(c, s, e)| c != "missing_function_body" || e > s),
        "length-0 missing_function_body row: {rows:?}"
    );
}

#[test]
fn eof_anchored_report_keeps_eof_span() {
    // `findNonZeroLengthToken` stops at EOF (`!token.isEof` guard,
    // util.dart:57): a body missing entirely at end-of-file anchors on the
    // zero-width EOF token — the walk must not manufacture a span.
    let rows = rows_at("void f()", augment_set());
    assert!(
        rows.iter().any(|(c, s, e)| c == "missing_function_body" && e - s <= 1),
        "expected an EOF-anchored missing_function_body, got {rows:?}"
    );
}
