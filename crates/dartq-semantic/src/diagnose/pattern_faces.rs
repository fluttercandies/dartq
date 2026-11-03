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
//! * the record pattern's close (`ParserListener::handle_record_pattern`)
//!   shares the two field-name faces: upstream runs
//!   `buildSharedPatternFields` and `duplicateRecordPatternField` for
//!   record patterns too (`ast.dart:27938`/`:27982`,
//!   `mustBeNamed: false`), so `(a: 1, a: 2)` reports
//!   `duplicate_pattern_field` and `(: 0)` reports
//!   `missing_named_pattern_field_name` — while a positional record field
//!   is legal and never reports `positional_field_in_object_pattern`. The
//!   hook carries no end token (unlike the object close), so the record
//!   arm resolves the `)` through the scanner's group table
//!   (`TokenTable::matching_end`).

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
    shared_pattern_field_faces(listener, count, begin, Some(end), true);
}

/// The record pattern's close: the field-name faces plus the per-field
/// column judging (`analyzeRecordPattern` pushes each field's own slot
/// type as its sub-pattern's `matchedValueType`, upstream Dart SDK source,
/// pkg/_fe_analyzer_shared/lib/src/type_inference/type_analyzer.dart:
/// 1841-1855 — the record arm of `judge_object_field_rows`'s object face).
/// `mustBeNamed: false` upstream, so a positional field is legal and only
/// the name faces (`missing_named_pattern_field_name`,
/// `duplicate_pattern_field`) can fire. The hook carries no end token, so
/// `None` routes the `)` lookup through the scanner's group table.
pub(super) fn record_pattern_field_faces(
    listener: &mut DiagnoseListener<'_>,
    count: usize,
    begin: Token,
) {
    judge_record_field_rows(listener);
    shared_pattern_field_faces(listener, count, begin, None, false);
}

/// The per-field column face of a closing record pattern: each field's
/// deferred never-match candidates are judged against the record slot the
/// field reads — `matchedType.positional[i]` for the i-th positional
/// field, `matchedType.named[n]` for a named one (upstream
/// `analyzeRecordPattern`'s field dispatch, type_analyzer.dart:1841-1855).
/// A field whose slot the matched record shape does not carry, and a
/// matched type that is no record at all, read `dynamic` — the official's
/// no-column posture, which the verdict never proves dead.
fn judge_record_field_rows(listener: &mut DiagnoseListener<'_>) {
    let Some(matched) = listener.engine.pattern_matched_ty() else {
        return;
    };
    // The drained fields of this record's own bracket frame, index-aligned
    // with `PatternNodeState::fields` (both append once per
    // `handle_pattern_field` boundary): `ObjectField::name` is the field's
    // record-slot key — `Some` for `n: p` and the `: p` shorthand, `None`
    // for a positional field.
    let names: Vec<Option<StringId>> = match listener.engine.switch.pattern_stack.last() {
        Some(frame) => frame.fields.iter().map(|field| field.name).collect(),
        None => return,
    };
    let buckets = match listener.pattern_rows.last_mut() {
        Some(state) if state.fields.len() == names.len() => {
            std::mem::take(&mut state.fields)
        }
        _ => return,
    };
    // The slot columns, pre-read so the emit pass borrows nothing but the
    // engine: positional fields index only the positional slots (upstream's
    // `positionalFields[i]` walks positional entries alone).
    let mut columns: Vec<Ty> = Vec::with_capacity(names.len());
    let mut positional = 0usize;
    for name in &names {
        let column = match (name, listener.engine.fold_alias_ty(matched)) {
            (Some(name), Ty::Record { shape, .. }) => {
                let pool = listener.engine.boundary.pool();
                pool.record_shape(shape)
                    .named
                    .iter()
                    .find(|(slot, _)| slot == name)
                    .map(|(_, ty)| *ty)
                    .unwrap_or(Ty::Dynamic)
            }
            (None, Ty::Record { shape, .. }) => {
                let pool = listener.engine.boundary.pool();
                let column = pool
                    .record_shape(shape)
                    .positional
                    .get(positional)
                    .copied()
                    .unwrap_or(Ty::Dynamic);
                positional += 1;
                column
            }
            (None, _) => {
                positional += 1;
                Ty::Dynamic
            }
            (Some(_), _) => Ty::Dynamic,
        };
        columns.push(column);
    }
    for (rows, column) in buckets.into_iter().zip(columns) {
        for (at, required) in rows {
            listener
                .engine
                .judge_pattern_never_matches(at, required, column);
        }
    }
}

/// The shared field faces of `buildSharedPatternFields` +
/// `duplicateRecordPatternField`: `must_be_named` is the object-pattern
/// arm (`true`) vs the record-pattern arm (`false`), and `end` is the
/// group close the object hook passes — `None` resolves it through the
/// scanner's group table instead (the record hook carries no end token).
fn shared_pattern_field_faces(
    listener: &mut DiagnoseListener<'_>,
    count: usize,
    begin: Token,
    end: Option<Token>,
    must_be_named: bool,
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
        // The group close: the object hook's `end` token locates it by
        // offset; the record hook carries none, so the scanner's group
        // table answers the `)` the `(` pairs with (`None` on an
        // unbalanced recovery shape bails the same way).
        let close_idx = match end {
            Some(end) => {
                let idx = toks.partition_point(|t| t.utf16_offset < end.range.offset);
                if toks
                    .get(idx)
                    .is_none_or(|t| t.utf16_offset != end.range.offset)
                {
                    return;
                }
                idx
            }
            None => match engine.tokens.matching_end(open_idx) {
                Some(idx) => idx,
                None => return,
            },
        };
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
                // A positional field: the object-pattern arm reports it
                // whatever the sub-pattern binds (the oracle's `mustBeNamed`
                // arm runs before any getter fallback); the record-pattern
                // arm keeps it legal (no row, no ledger — a positional slot
                // carries no name).
                verdicts.push(FieldVerdict {
                    row: must_be_named.then(|| {
                        (
                            SemanticCode::PositionalFieldInObjectPattern,
                            span_of(engine, toks, s, e),
                            String::new(),
                        )
                    }),
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

// —— the binder/counter faces (the S2 residual eight, #3667) ——————————————
//
// These verdicts anchor inside hooks the fold already owns, and each needs
// a small ledger the parse events alone do not carry:
//
// * `empty_map_pattern` — `analyzeMapPattern`'s `elements.isEmpty` arm
//   (upstream Dart SDK source, pkg/_fe_analyzer_shared/lib/src/
//   type_inference/type_analyzer.dart:1360-1368 — `.at(pattern)`, the
//   whole `{...}` node including any written `<...>` group).
// * `expected_one_list_pattern_type_arguments` /
//   `expected_two_map_pattern_type_arguments` — the written `<...>`
//   group's element count (upstream Dart SDK source,
//   pkg/analyzer/lib/src/dart/resolver/
//   list_pattern_resolver.dart:22-32 and
//   pkg/analyzer/lib/src/generated/resolver.dart:1607-1619 —
//   `.at(typeArguments)`, the group node).
// * `duplicate_variable_pattern` — `VariableBinder.add`'s same-name arm
//   (upstream Dart SDK source, pkg/_fe_analyzer_shared/lib/src/
//   type_inference/variable_bindings.dart:30-44 — the duplicate's name
//   token, the first binder riding the context row,
//   `DiagnosticFactory().duplicateDefinitionForNodes`,
//   pkg/analyzer/lib/src/diagnostic/diagnostic_factory.dart:84-106).
// * `duplicate_pattern_assignment_variable` — the assignment context's
//   `assignedVariables` ledger (upstream Dart SDK source,
//   type_analyzer.dart:331-345 — `.at(duplicate)`, the second target's
//   name).
// * `missing_variable_pattern` / `inconsistent_pattern_variable_logical_or`
//   — `VariableBinder.logicalOrPatternFinish` and the joined-variable
//   consistency walk (upstream Dart SDK source,
//   variable_bindings.dart:86-136 and type_analyzer.dart:2870-2920 — the
//   omitting branch's whole operand node, and the later branch's binder
//   name token).

/// One case-pattern binder of the `||` ledger — the name-level mirror of a
/// `VariableBinder._variables` entry (`BindPatternVariableElementImpl`
/// upstream). `ty`/`fin` carry the declared-type and finality faces the
/// joined-variable consistency walk compares; `broken` marks a component
/// that already joined under an inconsistency — upstream's `errorType`
/// face, which structurally equals only itself.
pub(super) struct OrBinder {
    pub(super) name: StringId,
    pub(super) span: SourceRange,
    /// The resolved declared type (alias-folded); `None` for an untyped
    /// binder (`var x`) — the name level cannot infer the matched face,
    /// so the consistency compare stays silent on it (the under-report
    /// direction).
    pub(super) ty: Option<Ty>,
    /// `Some(is_final)`; `None` once a join went inconsistent on finality
    /// (upstream's `isFinalIfConsistent` nulls and stays silent).
    pub(super) fin: Option<bool>,
    /// The joined variable already carries an inconsistency (upstream's
    /// `errorType` face): consistent with another broken component,
    /// inconsistent with every intact one.
    pub(super) broken: bool,
}

/// `VariableBinder.add` (variable_bindings.dart:30-44): the declared binder
/// joins the innermost or-scope, reporting `duplicate_variable_pattern` at
/// the duplicate's name token with the first binder as context.
pub(super) fn note_declared_binder(
    rows: &mut [super::listener::PatternNodeState],
    engine: &mut Engine<'_>,
    name: StringId,
    span: SourceRange,
    ty: Option<Ty>,
    fin: Option<bool>,
) {
    let Some(root) = rows.first_mut() else {
        return;
    };
    let Some(scope) = root.or_scopes.last_mut() else {
        return;
    };
    if let Some(original) = scope.iter().find(|binder| binder.name == name) {
        let original_span = original.span;
        let text = engine.strings.resolve(name).to_string();
        engine.emit_with_context(
            SemanticCode::DuplicateVariablePattern,
            span,
            &text,
            "",
            vec![ContextMessage {
                message: SharedStr::from("The first definition of this name."),
                file: engine.file,
                range: original_span,
            }],
        );
        return;
    }
    scope.push(OrBinder {
        name,
        span,
        ty,
        fin,
        broken: false,
    });
}

/// The `assignedVariables` ledger of `analyzeAssignedVariablePattern`
/// (type_analyzer.dart:331-345): a second assignment target of the same
/// variable reports `duplicate_pattern_assignment_variable` at the
/// later target's name token, the first assignment riding the context
/// row. Runs only for a resolved LOCAL target — the
/// `pattern_assignment_not_local_variable` arm owns resolved non-locals
/// and `undefined_identifier` owns the misses.
pub(super) fn note_assigned_variable(
    rows: &mut [super::listener::PatternNodeState],
    engine: &mut Engine<'_>,
    name: StringId,
    span: SourceRange,
) {
    let Some(root) = rows.first_mut() else {
        return;
    };
    if let Some(&(_, original_span)) = root
        .assigned_vars
        .iter()
        .find(|(assigned, _)| *assigned == name)
    {
        let text = engine.strings.resolve(name).to_string();
        engine.emit_with_context(
            SemanticCode::DuplicatePatternAssignmentVariable,
            span,
            &text,
            "",
            vec![ContextMessage {
                message: SharedStr::from("The first assigned variable pattern."),
                file: engine.file,
                range: original_span,
            }],
        );
        return;
    }
    root.assigned_vars.push((name, span));
}

/// `logicalOrPatternStart` + `logicalOrPatternFinishLeft`
/// (variable_bindings.dart:139-147): the `||` left operand's binders
/// already sit in the innermost scope — the current frame's `binder_mark`
/// marks where they begin, and the right operand parses into a fresh
/// scope pushed here.
pub(super) fn begin_or_operand(rows: &mut [super::listener::PatternNodeState]) {
    let mark = rows.last().map_or(0, |state| state.binder_mark);
    let Some(root) = rows.first_mut() else {
        return;
    };
    root.or_splits.push(mark);
    root.or_scopes.push(Vec::new());
}

/// `logicalOrPatternFinish` (variable_bindings.dart:86-136) plus the
/// joined-variable consistency walk (type_analyzer.dart:2870-2920,
/// `JoinedPatternVariableLocation.singlePattern`): each name bound in
/// exactly one branch reports `missing_variable_pattern` at the OMITTING
/// branch's operand node; a name bound in both joins, and a
/// type/finality disagreement reports
/// `inconsistent_pattern_variable_logical_or` at the LATER branch's
/// binder. The joined entries re-enter the scope below — the next outer
/// `||` reads them as its own left operand's binders.
pub(super) fn end_or_operand(
    rows: &mut [super::listener::PatternNodeState],
    engine: &mut Engine<'_>,
    operator: Token,
    end: Token,
) {
    // The operand spans: the `||` lives in the innermost open frame, whose
    // begin token is the left operand's head (a left-associative chain
    // keeps the whole `a || b` left inside the frame); the right operand's
    // own frame just closed, so `last_pop_begin` is its head and `end` its
    // last token.
    let (left_begin, right_begin) = match rows.last() {
        Some(state) => match (state.begin_token, state.last_pop_begin) {
            (Some(left), Some(right)) => (left, right),
            _ => return,
        },
        None => return,
    };
    let left_span = SourceRange {
        offset: left_begin.range.offset,
        length: operator.range.offset.saturating_sub(left_begin.range.offset),
    };
    let right_span = SourceRange {
        offset: right_begin.range.offset,
        length: end.range.end().saturating_sub(right_begin.range.offset),
    };
    if left_span.length == 0 || right_span.length == 0 {
        return;
    }
    // The ledger joins (reads + bookkeeping, no emissions).
    let (right, left) = {
        let Some(root) = rows.first_mut() else {
            return;
        };
        let Some(right) = root.or_scopes.pop() else {
            return;
        };
        let Some(mark) = root.or_splits.pop() else {
            root.or_scopes.push(right);
            return;
        };
        let Some(top) = root.or_scopes.last_mut() else {
            return;
        };
        let left = if mark <= top.len() {
            top.split_off(mark)
        } else {
            Vec::new()
        };
        (right, left)
    };
    // The finish walk: left-first, then the right-only tail — the oracle's
    // per-map entry order. A shared name's components compare pairwise:
    // the left entry is the baseline, the right entry the reported
    // `component`.
    let mut joined: Vec<OrBinder> = Vec::with_capacity(left.len() + right.len());
    let mut right_remaining: Vec<OrBinder> = right;
    for left_binder in left {
        let Some(pos) = right_remaining
            .iter()
            .position(|b| b.name == left_binder.name)
        else {
            // Bound in the left branch only: the right operand omits it.
            let text = engine.strings.resolve(left_binder.name).to_string();
            engine.emit_corrected(SemanticCode::MissingVariablePattern, right_span, &text);
            joined.push(left_binder);
            continue;
        };
        let right_binder = right_remaining.remove(pos);
        let type_inconsistent = matches!(
            (left_binder.ty, right_binder.ty),
            (Some(a), Some(b)) if a != b
        ) || left_binder.broken != right_binder.broken;
        let fin_inconsistent = matches!(
            (left_binder.fin, right_binder.fin),
            (Some(a), Some(b)) if a != b
        );
        if type_inconsistent || fin_inconsistent {
            let text = engine.strings.resolve(left_binder.name).to_string();
            engine.emit_corrected(
                SemanticCode::InconsistentPatternVariableLogicalOr,
                right_binder.span,
                &text,
            );
        }
        joined.push(OrBinder {
            name: left_binder.name,
            span: left_binder.span,
            ty: if type_inconsistent {
                None
            } else {
                left_binder.ty.or(right_binder.ty)
            },
            fin: if fin_inconsistent {
                None
            } else {
                left_binder.fin.or(right_binder.fin)
            },
            broken: left_binder.broken || right_binder.broken || type_inconsistent,
        });
    }
    for right_binder in right_remaining {
        // Bound in the right branch only: the left operand omits it.
        let text = engine.strings.resolve(right_binder.name).to_string();
        engine.emit_corrected(SemanticCode::MissingVariablePattern, left_span, &text);
        joined.push(right_binder);
    }
    if let Some(top) = rows
        .first_mut()
        .and_then(|root| root.or_scopes.last_mut())
    {
        top.extend(joined);
    }
}

/// The written `<...>` group ahead of a list/map pattern's `[`/`{`: the
/// type-argument COUNT verdict (`expected_one_list_pattern_type_arguments`
/// / `expected_two_map_pattern_type_arguments`). The group resolves by a
/// token-table backward walk from the bracket: the `>`'s matching `<` is
/// found by angle-depth counting (`Gt`/`GtGt`/`GtGtGt` close one/two/three
/// levels — the scanner's combined closers never split in the stored
/// table), and the arguments are the depth-0 comma splits inside.
pub(super) fn pattern_type_argument_count(
    engine: &mut Engine<'_>,
    left: Token,
    expected: usize,
    code: SemanticCode,
) {
    let verdict: Option<(SourceRange, usize)> = {
        let engine = &*engine;
        let toks = engine.tokens.as_slice();
        let left_idx = toks.partition_point(|t| t.utf16_offset < left.range.offset);
        if toks
            .get(left_idx)
            .is_none_or(|t| t.utf16_offset != left.range.offset)
            || left_idx == 0
        {
            None
        } else {
            angle_group_before(toks, left_idx).map(|(lt, gt)| (lt, gt, toks))
        }
        .and_then(|(lt, gt, toks)| {
            let count = count_depth0_commas(toks, lt, gt);
            (count != expected).then(|| {
                (
                    SourceRange {
                        offset: toks[lt].utf16_offset,
                        length: toks[gt].utf16_offset + toks[gt].byte_len as u32
                            - toks[lt].utf16_offset,
                    },
                    count,
                )
            })
        })
    };
    if let Some((span, count)) = verdict {
        engine.emit_corrected(code, span, &count.to_string());
    }
}

/// `visitMapPattern`'s `elements.isEmpty` arm: `.at(pattern)`, the whole
/// `{...}` node — the written `<...>` group (when present) is part of the
/// node, so the span opens at the `<`, else at the `{`.
pub(super) fn empty_map_pattern_face(
    engine: &mut Engine<'_>,
    count: usize,
    left: Token,
    right: Token,
) {
    if count != 0 {
        return;
    }
    let start = {
        let engine = &*engine;
        let toks = engine.tokens.as_slice();
        let left_idx = toks.partition_point(|t| t.utf16_offset < left.range.offset);
        if toks
            .get(left_idx)
            .is_none_or(|t| t.utf16_offset != left.range.offset)
            || left_idx == 0
        {
            None
        } else {
            angle_group_before(toks, left_idx).map(|(lt, _)| toks[lt].utf16_offset)
        }
    };
    engine.emit_corrected(
        SemanticCode::EmptyMapPattern,
        SourceRange {
            offset: start.unwrap_or(left.range.offset),
            length: right
                .range
                .end()
                .saturating_sub(start.unwrap_or(left.range.offset)),
        },
        "",
    );
}

/// The `<...>` group immediately before token index `open`: `None` unless
/// `toks[open - 1]` is a `>`-family closer (`Gt`/`GtGt`/`GtGtGt` — the
/// scanner never splits combined closers in the stored table, so the
/// group's last token closes its own width of levels) whose matching `<`
/// the backward angle-depth walk balances. Returns `(lt_idx, gt_idx)`.
fn angle_group_before(toks: &[ScannedToken], open: usize) -> Option<(usize, usize)> {
    let gt = open - 1;
    let mut depth = match toks.get(gt).map(|t| t.kind) {
        Some(TokenKind::Gt) => 1,
        Some(TokenKind::GtGt) => 2,
        Some(TokenKind::GtGtGt) => 3,
        _ => return None,
    };
    for i in (0..gt).rev() {
        depth += match toks[i].kind {
            TokenKind::Gt => 1,
            TokenKind::GtGt => 2,
            TokenKind::GtGtGt => 3,
            TokenKind::Lt => -1,
            _ => 0,
        };
        if depth == 0 {
            return Some((i, gt));
        }
        if depth < 0 {
            return None;
        }
    }
    None
}

/// The number of comma-separated arguments inside `toks[lt + 1..gt]`: the
/// depth-0 comma count plus one, `0` when the group holds nothing (a
/// `<>` recovery). Depth counts angles (`<` opens, `>`-family closes by
/// lexeme width), brackets and interpolation groups alike.
fn count_depth0_commas(toks: &[ScannedToken], lt: usize, gt: usize) -> usize {
    if lt + 1 >= gt {
        return 0;
    }
    let mut depth = 0i32;
    let mut commas = 0usize;
    for i in lt + 1..gt {
        depth += match toks[i].kind {
            TokenKind::Lt => 1,
            TokenKind::Gt => -1,
            TokenKind::GtGt => -2,
            TokenKind::GtGtGt => -3,
            TokenKind::OpenParen
            | TokenKind::OpenSquare
            | TokenKind::OpenCurly
            | TokenKind::StringInterpolationExpression => 1,
            TokenKind::CloseParen | TokenKind::CloseSquare | TokenKind::CloseCurly => -1,
            _ => 0,
        };
        if depth == 0 && toks[i].kind == TokenKind::Comma {
            commas += 1;
        }
    }
    commas + 1
}
