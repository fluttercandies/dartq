//! The structural pattern faces (#3629 sub-cluster S2): the
//! pattern-shape diagnostics whose verdicts are pure parse-shape facts —
//! no resolution, no types. Two listener hooks carry all of them (the
//! hook methods live in the fold's `ParserListener` impl in `listener.rs`;
//! the verdicts live here):
//!
//! * the object pattern's field-group close (`ParserListener::
//!   handle_object_pattern_fields`, official `parseObjectPatternRest`'s
//!   trailing event). The fields are already drained into the pattern's
//!   own bracket frame (one [`ObjectField`] per `handle_pattern_field`
//!   boundary, parse order), so the field facts meet the per-field token
//!   slices here: a nameless `: pattern` field whose sub-pattern binds no
//!   variable reports `missing_named_pattern_field_name`
//!   (upstream Dart SDK source,
//!   pkg/analyzer/lib/src/generated/resolver.dart:494-502,
//!   `buildSharedPatternFields`'s null-name arm —
//!   `.at(field)`, the whole field node), a positional field reports
//!   `positional_field_in_object_pattern` (upstream Dart SDK source,
//!   pkg/analyzer/lib/src/generated/resolver.dart:503-508, the
//!   `mustBeNamed` arm — `.at(field)`), and a repeated field name reports
//!   `duplicate_pattern_field` (`duplicateRecordPatternField`,
//!   upstream Dart SDK source,
//!   pkg/analyzer/lib/src/dart/resolver/shared_type_analyzer.dart:78-91 —
//!   at the duplicate's name token, or
//!   its colon when the name is implicit).
//!
//! * `ParserListener::handle_rest_pattern` — a `...` element of a list or
//!   map pattern. The enclosing bracket (the innermost unclosed group, the
//!   token-table backward walk) decides the verdict: inside `{}` every
//!   rest reports `rest_element_in_map_pattern` (`restPatternInMap`,
//!   upstream Dart SDK source,
//!   pkg/analyzer/lib/src/dart/resolver/shared_type_analyzer.dart:228-237 —
//!   `.at(element)`, the `...` through
//!   its sub-pattern), and inside `[]` a second rest reports
//!   `duplicate_rest_element_in_pattern` (`duplicateRestPattern`,
//!   upstream Dart SDK source,
//!   pkg/analyzer/lib/src/dart/resolver/shared_type_analyzer.dart:92-105 —
//!   at the duplicate element; the map
//!   walk keeps no duplicate ledger, so a repeated map rest reports the
//!   per-element row twice and no duplicate row).
//!
//! The verdicts re-derive the field/element boundaries from the token
//! table because the parser's own events carry no per-field spans: the
//! `handle_pattern_field` payload is the label token only, and the group
//! close carries just the count. The walk mirrors the fold's established
//! token-table idiom ([`Engine::initializer_node_span`]): brackets are
//! scanner-paired, comments and whitespace sit outside the table, and
//! `${`/`}` interpolation groups balance like any bracket pair.
//!
//! Recovery shapes bail silently (a field slice that consumed nothing, an
//! unbalanced group, a count that disagrees with the drained buckets): the
//! oracle's own rows there ride its recovery diagnostics, and a verdict
//! guessed from a half-parsed shape risks an extra the census cannot
//! forgive.
//!
//! RESIDUAL — record patterns. Upstream runs `buildSharedPatternFields`
//! and `duplicateRecordPatternField` for RECORD patterns too, so the two
//! field-name faces also apply to `(a: 1, a: 2)` / `(: 0)`; the parser
//! fires `handle_object_pattern_fields` only from `parseObjectPatternRest`
//! (object patterns), and record patterns close through
//! `handle_record_pattern`, whose body is a pre-existing override. That
//! residual is 44 of the 312 oracle rows these five codes carry in
//! `conformance/expected.tsv` (30 of `missing_named_pattern_field_name`'s
//! 173 and 14 of `duplicate_pattern_field`'s 27); the object-pattern face
//! covers the other 268.

use super::*;
use dartq_scanner::{Token as ScannedToken, TokenKind};

/// One field's structural verdict, derived in the token-table phase and
/// emitted after the borrow ends: the row to report (code, anchor span,
/// message name slot), and the duplicate-ledger entry the field
/// contributes (the inferred or written name with its anchor — the oracle
/// pairs every duplicate against the name's first occurrence).
struct FieldVerdict {
    row: Option<(SemanticCode, SourceRange, String)>,
    ledger: Option<(StringId, SourceRange)>,
}

/// The object pattern's field-group close: the field faces. Hooked from
/// the fold's `handle_object_pattern_fields`.
pub(super) fn object_pattern_field_faces(
    listener: &mut DiagnoseListener<'_>,
    count: usize,
    begin: Token,
    end: Token,
) {
    if count == 0 {
        return;
    }
    // The drained fields of this pattern's own bracket frame (one per
    // `handle_pattern_field` boundary, parse order). The object close — the
    // very next event — consumes them, so the binder facts are read into an
    // owned buffer first.
    let binders: Vec<Option<(StringId, SourceRange)>> = {
        let Some(frame) = listener.engine.switch.pattern_stack.last() else {
            return;
        };
        if frame.fields.len() != count {
            // A recovery shape whose field boundaries drained into a
            // foreign frame: no verdict is derivable.
            return;
        }
        frame.fields.iter().map(|field| field.binder).collect()
    };
    // Phase 1 — the token-table walk, all reads.
    let verdicts: Vec<FieldVerdict> = {
        let engine = &listener.engine;
        let toks = engine.tokens.as_slice();
        let open_idx = toks.partition_point(|t| t.utf16_offset < begin.range.offset);
        if toks.get(open_idx).is_none_or(|t| {
            t.utf16_offset != begin.range.offset || t.kind != TokenKind::OpenParen
        }) {
            return;
        }
        let close_idx = toks.partition_point(|t| t.utf16_offset < end.range.offset);
        if toks
            .get(close_idx)
            .is_none_or(|t| t.utf16_offset != end.range.offset)
        {
            return;
        }
        // Split the group at its depth-0 commas: one slice per field, in
        // parse order (the `(...)` brackets of nested sub-patterns and the
        // `${` groups of string constants keep their commas out of sight).
        // A trailing comma closes no extra field.
        let mut slices: Vec<(usize, usize)> = Vec::with_capacity(count);
        let mut start = open_idx + 1;
        let mut depth = 0i32;
        for i in open_idx + 1..close_idx {
            match toks[i].kind {
                TokenKind::OpenParen
                | TokenKind::OpenSquare
                | TokenKind::OpenCurly
                | TokenKind::StringInterpolationExpression => depth += 1,
                TokenKind::CloseParen | TokenKind::CloseSquare | TokenKind::CloseCurly => {
                    depth -= 1
                }
                TokenKind::Comma if depth == 0 => {
                    if start < i {
                        slices.push((start, i));
                    }
                    start = i + 1;
                }
                _ => {}
            }
        }
        if depth != 0 {
            // The group never balanced (a recovery jump-off): bail.
            return;
        }
        if start < close_idx {
            slices.push((start, close_idx));
        }
        if slices.len() != count {
            return;
        }
        let mut verdicts = Vec::with_capacity(count);
        for (fi, &(s, e)) in slices.iter().enumerate() {
            if s >= e {
                // A field slice that consumed nothing (`A(:)` recovery
                // keeps the blocker in place): the sub-pattern's own rows
                // answer; a guessed verdict here risks an extra.
                return;
            }
            let first = toks[s].kind;
            if first == TokenKind::Colon {
                // The `: pattern` shape: the getter name is implicit —
                // inferable only from a variable-pattern sub-pattern (the
                // drained binder). Upstream falls back to
                // `field.pattern.variablePattern`, and only the transparent
                // wrappers resolve through to one.
                let row = binders[fi].is_none().then(|| {
                    (
                        SemanticCode::MissingNamedPatternFieldName,
                        span_of(engine, toks, s, e),
                        String::new(),
                    )
                });
                // An implicit name joins the duplicate ledger under the
                // binder's own name; the oracle's anchor is the field's
                // colon (`duplicateTarget = name ?? colon`).
                let ledger = binders[fi].map(|(name, _)| {
                    (
                        name,
                        SourceRange {
                            offset: toks[s].utf16_offset,
                            length: 1,
                        },
                    )
                });
                verdicts.push(FieldVerdict { row, ledger });
            } else if e - s >= 2
                && toks[s + 1].kind == TokenKind::Colon
                && first == TokenKind::Identifier
            {
                // The `name: pattern` shape: the written name joins the
                // ledger; the duplicate anchor is the name token.
                let name = intern_slice(engine, &toks[s]);
                verdicts.push(FieldVerdict {
                    row: None,
                    ledger: Some((name, span_of(engine, toks, s, s + 1))),
                });
            } else {
                // A positional field in an object pattern — reported
                // whatever the sub-pattern binds (the oracle's `mustBeNamed`
                // arm runs before any getter fallback).
                verdicts.push(FieldVerdict {
                    row: Some((
                        SemanticCode::PositionalFieldInObjectPattern,
                        span_of(engine, toks, s, e),
                        String::new(),
                    )),
                    ledger: None,
                });
            }
        }
        verdicts
    };
    // Phase 2 — the emissions (the token-table borrow has ended).
    let mut seen: Vec<(StringId, SourceRange)> = Vec::new();
    for verdict in verdicts {
        if let Some((code, span, a0)) = verdict.row {
            listener.engine.emit_corrected(code, span, &a0);
        }
        if let Some((name, anchor)) = verdict.ledger {
            if seen.iter().any(|(seen_name, _)| *seen_name == name) {
                let name_text = listener.engine.strings.resolve(name).to_string();
                listener
                    .engine
                    .emit_corrected(SemanticCode::DuplicatePatternField, anchor, &name_text);
            } else {
                seen.push((name, anchor));
            }
        }
    }
}

/// A `...` element of a list or map pattern: the enclosing-bracket
/// verdicts. Hooked from the fold's `handle_rest_pattern`.
pub(super) fn rest_pattern_faces(
    listener: &mut DiagnoseListener<'_>,
    dots: Token,
    has_sub_pattern: bool,
) {
    // Phase 1 — the token-table walk, all reads.
    let verdict: Option<(SemanticCode, SourceRange)> = {
        let engine = &listener.engine;
        let toks = engine.tokens.as_slice();
        let dots_idx = toks.partition_point(|t| t.utf16_offset < dots.range.offset);
        if toks
            .get(dots_idx)
            .is_none_or(|t| t.utf16_offset != dots.range.offset)
        {
            return;
        }
        // The innermost unclosed bracket before the dots: the list/map
        // suffix's own group (the parse position guarantees exactly one
        // bracket of it is open here).
        let mut depth = 0i32;
        let mut enclosing = None;
        for i in (0..dots_idx).rev() {
            match toks[i].kind {
                TokenKind::CloseParen | TokenKind::CloseSquare | TokenKind::CloseCurly => {
                    depth += 1
                }
                TokenKind::OpenParen
                | TokenKind::OpenSquare
                | TokenKind::OpenCurly
                | TokenKind::StringInterpolationExpression => {
                    if depth == 0 {
                        enclosing = Some(i);
                        break;
                    }
                    depth -= 1;
                }
                _ => {}
            }
        }
        let Some(enc) = enclosing else {
            return;
        };
        let span = rest_element_span(engine, toks, dots, has_sub_pattern, dots_idx, enc);
        match toks[enc].kind {
            // Every rest element inside a map pattern reports (the oracle's
            // per-element walk keeps no duplicate ledger — a repeated map
            // rest reports twice, never a duplicate row).
            TokenKind::OpenCurly => Some((SemanticCode::RestElementInMapPattern, span)),
            TokenKind::OpenSquare => {
                // A second rest in one list pattern reports at the
                // duplicate element (the first stays silent).
                let mut previous = 0usize;
                let mut inner = 0i32;
                for i in enc + 1..dots_idx {
                    match toks[i].kind {
                        TokenKind::PeriodPeriodPeriod if inner == 0 => previous += 1,
                        TokenKind::OpenParen
                        | TokenKind::OpenSquare
                        | TokenKind::OpenCurly
                        | TokenKind::StringInterpolationExpression => inner += 1,
                        TokenKind::CloseParen | TokenKind::CloseSquare | TokenKind::CloseCurly => {
                            inner -= 1
                        }
                        _ => {}
                    }
                }
                (previous >= 1).then_some((SemanticCode::DuplicateRestElementInPattern, span))
            }
            // A recovery shape (`handle_rest_pattern` fires only from the
            // list/map suffixes, so anything else is a recovery jump-off):
            // no verdict.
            _ => None,
        }
    };
    // Phase 2 — the emission (the token-table borrow has ended).
    if let Some((code, span)) = verdict {
        listener.engine.emit_corrected(code, span, "");
    }
}

/// The UTF-16 span of the token slice `s..e` (exclusive) — the field node
/// the oracle anchors `.at(field)`.
fn span_of(engine: &Engine<'_>, toks: &[ScannedToken], s: usize, e: usize) -> SourceRange {
    let last = &toks[e - 1];
    SourceRange {
        offset: toks[s].utf16_offset,
        length: engine.utf16_of(last.byte_offset as usize + last.byte_len as usize)
            - toks[s].utf16_offset,
    }
}

/// The rest element's span: the `...` alone, or through its sub-pattern's
/// last token (the first depth-0 `,` or the enclosing group's closer ends
/// the element). `open_idx`'s scanner-paired closer bounds the walk when
/// the group never closes in recovery.
fn rest_element_span(
    engine: &Engine<'_>,
    toks: &[ScannedToken],
    dots: Token,
    has_sub_pattern: bool,
    dots_idx: usize,
    open_idx: usize,
) -> SourceRange {
    let limit = engine.tokens.matching_end(open_idx).unwrap_or(toks.len());
    let mut last = dots_idx;
    if has_sub_pattern {
        let mut depth = 0i32;
        for i in dots_idx + 1..limit {
            let kind = toks[i].kind;
            if depth == 0
                && matches!(
                    kind,
                    TokenKind::Comma
                        | TokenKind::CloseParen
                        | TokenKind::CloseSquare
                        | TokenKind::CloseCurly
                )
            {
                break;
            }
            match kind {
                TokenKind::OpenParen
                | TokenKind::OpenSquare
                | TokenKind::OpenCurly
                | TokenKind::StringInterpolationExpression => depth += 1,
                TokenKind::CloseParen | TokenKind::CloseSquare | TokenKind::CloseCurly => {
                    depth -= 1
                }
                _ => {}
            }
            last = i;
        }
    }
    let end = if last == dots_idx {
        dots.range.end()
    } else {
        let t = &toks[last];
        engine.utf16_of(t.byte_offset as usize + t.byte_len as usize)
    };
    SourceRange {
        offset: dots.range.offset,
        length: end.saturating_sub(dots.range.offset),
    }
}

/// Interns a token's lexeme (the duplicate ledger's name space).
fn intern_slice(engine: &Engine<'_>, t: &ScannedToken) -> StringId {
    let start = t.byte_offset as usize;
    let end = start + t.byte_len as usize;
    engine.strings.intern(&engine.source[start..end])
}
