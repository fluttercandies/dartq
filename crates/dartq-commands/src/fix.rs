//! The fix face's substrate (PRD §15.6): the anchored patch document, the
//! drift-correction applier, and the row source mapping. Both the CLI's
//! `dartq fix` and the daemon's `dartq/fix` consume this one implementation;
//! the CLI adds apply-to-disk and human formatting, the daemon adds the
//! JSON-RPC envelope.
//!
//! The machine document is §15.6 verbatim:
//!
//! ```json
//! { "file": "lib/foo.dart", "schema_version": 1,
//!   "edits": [
//!     { "offset": 123, "length": 8, "replacement": "Bar",
//!       "anchor": { "line": 10, "column": 7, "context_before": "final Fo",
//!                   "context_after": "() = " } } ] }
//! ```
//!
//! `offset`/`length` are the engine's UTF-16 `[offset, offset + length)`
//! span, directly applicable. The `anchor` (1-based line/column of the
//! edit start plus up to eight characters of context on each side, never
//! crossing a line) is the §15.6 drift-correction face: when concurrent
//! overlays shifted offsets, the applier re-locates the edit by its
//! context (and internal original-slice match); an anchor that is gone or
//! ambiguous marks the edit `stale` — the applier never writes at a
//! position it cannot prove.
//!
//! # Fix sources
//!
//! Patches materialize from the dartq-lints layer's diagnostic rows (its
//! `run` face): each wired rule maps a hit to a span-exact rewrite whose
//! source slice is verified before the patch is offered —
//! `unnecessary_new` and `unnecessary_late` drop the keyword plus its
//! trailing horizontal whitespace, `avoid_empty_else` rewrites the else
//! `;` to `{}`, `unnecessary_brace_in_string_interps` drops the braces of
//! a simple interpolation. Rules without a mechanical rewrite are never
//! claimed, and a wired hit whose slice no longer verifies is dropped
//! rather than patched on faith.
//!
//! The rows reach [`from_lints`] through the lints layer's diagnostic
//! face; driving that face's `run` needs the layer's `Unit`, whose AST
//! input is the parser→AST seam (the engine-side combined-listener face
//! #120/#121 build toward) and whose feature bits are not part of the
//! crate's public face — so this build has no row source wired and
//! [`collect`] reports zero patch sets. The materializer and the
//! anchoring/apply substrate are complete and tested against the layer's
//! row contract (see the tests: every row carries a real registry rule id
//! and the official message, and the slice verification gates each
//! patch), so activation is one call site in [`collect`]'s source stage.
//! The built-in semantic fix set and the plugin source (PRD §15.6, §14)
//! are their own faces (dartq-ai / plugin host).

use std::path::PathBuf;

use dartq_lints::{Lint, RULES};
use dartq_source::{LineIndex, utf16_len_of, utf16_offset_to_byte};

use crate::discover::{self, DiscoverError};

/// The patch document version (PRD §15.6; §15.7 versioning discipline).
pub const SCHEMA_VERSION: u32 = 1;

/// Context characters captured on each side of an edit. §15.6's example
/// shows eight before and five after; eight covers a keyword plus its
/// neighbor and the exact budget is ours to pick.
const ANCHOR_CONTEXT: usize = 8;

/// The edit's secondary locator: 1-based line/column of the edit start
/// plus the source context flanking it (empty at line boundaries, where
/// the machine document omits the field).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Anchor {
    pub line: u32,
    pub column: u32,
    pub context_before: String,
    pub context_after: String,
}

/// One anchored replacement over the UTF-16 span `[offset, offset +
/// length)`. `source` is the verified original slice — internal drift
/// matching, deliberately not part of the §15.6 document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edit {
    pub offset: u32,
    pub length: u32,
    pub source: String,
    pub replacement: String,
    pub anchor: Anchor,
}

/// One materialized fix: the anchored edit plus the human-facing row.
#[derive(Debug, Clone)]
pub struct RuleFix {
    pub rule: &'static str,
    pub message: String,
    pub edit: Edit,
}

/// One file's fix set.
#[derive(Debug, Clone)]
pub struct FileFixes {
    pub path: String,
    pub fixes: Vec<RuleFix>,
}

/// Outcome of one edit against the file's current text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditOutcome {
    /// Written at the claimed offset or re-located by its anchor.
    Applied,
    /// The anchor is gone or ambiguous — skipped, per §15.6.
    Stale,
}

/// The fix source stage: validates the targets through discovery and
/// returns one fix set per patched file (files without fixes never enter
/// the set). The row source (lint rows, then the semantic/plugin sets)
/// plugs into the single loop below when the parser→AST bridge hands the
/// unit trees over; this build's runs are clean zero-fix runs.
///
/// # Errors
///
/// [`DiscoverError`] from target discovery.
pub fn collect(paths: &[PathBuf]) -> Result<Vec<FileFixes>, DiscoverError> {
    let _files = discover::discover(paths)?;
    Ok(Vec::new())
}

// ---------------------------------------------------------------------------
// Anchoring substrate
// ---------------------------------------------------------------------------

/// Anchors an edit over the UTF-16 span `[offset16, offset16 + length16)`.
#[must_use]
pub fn anchored_edit(
    text: &str,
    lines: &LineIndex,
    offset16: u32,
    length16: u32,
    replacement: &str,
) -> Edit {
    let (line, column) = lines.line_col_utf16(text, offset16);
    let start = utf16_offset_to_byte(text, offset16);
    let end = utf16_offset_to_byte(text, offset16 + length16);
    Edit {
        offset: offset16,
        length: length16,
        source: text[start..end].to_owned(),
        replacement: replacement.to_owned(),
        anchor: Anchor {
            line,
            column,
            context_before: line_context_before(text, start),
            context_after: line_context_after(text, end),
        },
    }
}

/// Up to [`ANCHOR_CONTEXT`] characters before `start`, never crossing the
/// line start.
fn line_context_before(text: &str, start: usize) -> String {
    let line_start = text[..start].rfind('\n').map_or(0, |i| i + 1);
    let taken: Vec<char> = text[line_start..start]
        .chars()
        .rev()
        .take(ANCHOR_CONTEXT)
        .collect();
    taken.into_iter().rev().collect()
}

/// Up to [`ANCHOR_CONTEXT`] characters after `end`, never crossing the
/// line end.
fn line_context_after(text: &str, end: usize) -> String {
    let line_end = text[end..].find('\n').map_or(text.len(), |i| end + i);
    text[end..line_end].chars().take(ANCHOR_CONTEXT).collect()
}

/// Whether `text` still holds `edit` at byte span `[start, end)`: the
/// original slice intact and the anchor context flanking it.
fn holds_at(text: &str, edit: &Edit, start: usize, end: usize) -> bool {
    text.get(start..end) == Some(edit.source.as_str())
        && text[..start].ends_with(edit.anchor.context_before.as_str())
        && text[end..].starts_with(edit.anchor.context_after.as_str())
}

/// The byte position to write `edit` at in `text`, or `None` for stale:
/// the claimed offset when the original slice still sits there flanked by
/// the anchor context, else the unique re-location — the drift-correction
/// pass of §15.6. More than one candidate (on the anchor line, or anywhere
/// when no candidate sits on it) is ambiguous and goes stale.
#[must_use]
fn locate(text: &str, edit: &Edit) -> Option<usize> {
    // Exact position first, guarded against offsets drifted past the end.
    if edit.offset + edit.length <= utf16_len_of(text) {
        let start = utf16_offset_to_byte(text, edit.offset);
        let end = start + edit.source.len();
        if holds_at(text, edit, start, end) {
            return Some(start);
        }
    }
    // Re-location: every position where the original slice survives with
    // the anchor context flanking it. Line numbers come from a fresh
    // [`LineIndex`] of the current text — the same index the anchor was
    // born from in [`anchored_edit`], so both faces agree on what "the
    // anchor line" means (the applier's text mutates per write, so an
    // index shared across edits would read stale line starts).
    let lines = LineIndex::new(text);
    let mut line_match: Option<usize> = None;
    let mut line_matches = 0;
    let mut any_match: Option<usize> = None;
    let mut total = 0;
    for (start, _) in text.match_indices(edit.source.as_str()) {
        let end = start + edit.source.len();
        if !holds_at(text, edit, start, end) {
            continue;
        }
        total += 1;
        if any_match.is_none() {
            any_match = Some(start);
        }
        let (line, _) = lines.byte_to_line_col(u32::try_from(start).expect("offset exceeds u32"));
        if line == edit.anchor.line {
            line_matches += 1;
            line_match = Some(start);
        }
    }
    match (line_matches, total) {
        (1, _) => line_match,
        (0, 1) => any_match,
        _ => None,
    }
}

/// Applies `edits` to `text`: exact-position validation, then anchor
/// re-location; overlapping edits and stale anchors are skipped. Returns
/// the patched text plus one outcome per edit, in input order.
///
/// Edits apply in descending offset order (each write is independent of
/// the lower-offset text); the overlap guard runs in snapshot coordinates,
/// so a re-located write never shifts a later edit's decision about its
/// own original span.
#[must_use]
pub fn apply(text: &str, edits: &[Edit]) -> (String, Vec<EditOutcome>) {
    let mut order: Vec<usize> = (0..edits.len()).collect();
    order.sort_by(|&a, &b| edits[b].offset.cmp(&edits[a].offset));
    let mut patched = text.to_owned();
    let mut outcomes: Vec<EditOutcome> = vec![EditOutcome::Stale; edits.len()];
    // Snapshot UTF-16 start of the lowest already-placed edit above.
    let mut applied_floor = u32::MAX;
    for &i in &order {
        let edit = &edits[i];
        if edit.offset.saturating_add(edit.length) > applied_floor {
            continue; // overlaps a write already placed above it
        }
        let Some(position) = locate(&patched, edit) else {
            continue;
        };
        let end = position + edit.source.len();
        patched.replace_range(position..end, &edit.replacement);
        applied_floor = edit.offset;
        outcomes[i] = EditOutcome::Applied;
    }
    (patched, outcomes)
}

// ---------------------------------------------------------------------------
// Fix source: the dartq-lints layer's diagnostic rows
// ---------------------------------------------------------------------------

/// Maps the lints layer's rows to anchored edits — the fix source of this
/// round (module docs). Rules without a span-exact mechanical rewrite
/// never produce an edit; a wired hit whose slice no longer verifies is
/// dropped rather than patched on faith.
#[must_use]
pub fn from_lints(hits: &[Lint], text: &str, lines: &LineIndex) -> Vec<RuleFix> {
    hits.iter()
        .filter_map(|hit| {
            let rule = RULES[hit.rule.0 as usize].name;
            let (offset, length, replacement) = match rule {
                "unnecessary_new" => drop_keyword(text, hit.range.start, "new")?,
                "unnecessary_late" => drop_keyword(text, hit.range.start, "late")?,
                "avoid_empty_else" => replace_exact(text, &hit.range, ";", "{}")?,
                "unnecessary_brace_in_string_interps" => unbrace(text, &hit.range)?,
                _ => return None,
            };
            Some(RuleFix {
                rule,
                message: hit.message.clone(),
                edit: anchored_edit(text, lines, offset, length, &replacement),
            })
        })
        .collect()
}

/// A width-0 keyword hit (`new`, `late`): the rewrite drops the keyword
/// plus its trailing horizontal whitespace (a newline stays — joining
/// lines is a formatting decision, not a fix). The keyword is ASCII, so
/// its lexeme length is its UTF-16 length.
fn drop_keyword(text: &str, offset16: u32, keyword: &str) -> Option<(u32, u32, String)> {
    debug_assert_eq!(
        keyword.len(),
        keyword.chars().map(char::len_utf16).sum::<usize>()
    );
    let start = utf16_offset_to_byte(text, offset16);
    let rest = text.get(start..)?.strip_prefix(keyword)?;
    let ws = rest.len() - rest.trim_start_matches([' ', '\t']).len();
    let length = u32::try_from(keyword.len() + ws).ok()?;
    Some((offset16, length, String::new()))
}

/// A span-exact rewrite: the hit span must slice to exactly `expect`.
fn replace_exact(
    text: &str,
    range: &core::ops::Range<u32>,
    expect: &str,
    replacement: &str,
) -> Option<(u32, u32, String)> {
    let start = utf16_offset_to_byte(text, range.start);
    let end = utf16_offset_to_byte(text, range.end);
    text.get(start..end).filter(|slice| *slice == expect)?;
    Some((range.start, range.end - range.start, replacement.to_owned()))
}

/// `${id}` → `$id`: the hit span is the whole interpolation group (the
/// rule reports it exactly); the `${…}` shape and a bare ASCII identifier
/// body are verified before rewriting.
fn unbrace(text: &str, range: &core::ops::Range<u32>) -> Option<(u32, u32, String)> {
    let start = utf16_offset_to_byte(text, range.start);
    let end = utf16_offset_to_byte(text, range.end);
    let slice = text.get(start..end)?;
    let inner = slice.strip_prefix("${")?.strip_suffix('}')?;
    if inner.is_empty() || !inner.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return None;
    }
    Some((range.start, range.end - range.start, format!("${inner}")))
}

// ---------------------------------------------------------------------------
// Document
// ---------------------------------------------------------------------------

/// The §15.6 document for one file — exactly `file`, `schema_version`, and
/// the `edits` with their `anchor` objects; empty context strings are
/// omitted (§15.7). Files without fixes never enter the document set.
#[must_use]
pub fn patch_json(fixes: &FileFixes) -> serde_json::Value {
    let edits: Vec<serde_json::Value> = fixes
        .fixes
        .iter()
        .map(|fix| {
            let anchor = &fix.edit.anchor;
            let mut anchor_json = serde_json::Map::new();
            anchor_json.insert("line".into(), anchor.line.into());
            anchor_json.insert("column".into(), anchor.column.into());
            if !anchor.context_before.is_empty() {
                anchor_json.insert(
                    "context_before".into(),
                    anchor.context_before.clone().into(),
                );
            }
            if !anchor.context_after.is_empty() {
                anchor_json.insert("context_after".into(), anchor.context_after.clone().into());
            }
            serde_json::json!({
                "offset": fix.edit.offset,
                "length": fix.edit.length,
                "replacement": fix.edit.replacement,
                "anchor": serde_json::Value::Object(anchor_json),
            })
        })
        .collect();
    serde_json::json!({
        "file": fixes.path,
        "schema_version": SCHEMA_VERSION,
        "edits": edits,
    })
}

#[cfg(test)]
mod tests {
    //! The rows here sit at dartq-lints' diagnostic output face (`Lint` is
    //! the crate's externally constructible row; its `Unit` feature bits
    //! are not part of the public face, so a run cannot be driven from
    //! outside). Each row carries the real registry rule id and the
    //! official message, and [`super::from_lints`]' slice verification is
    //! the correctness gate: a row that does not match the source produces
    //! no patch at all.

    use dartq_lints::{Lint, rule_by_name};
    use dartq_source::LineIndex;

    use super::*;

    /// The single fix of `hits[0]`, materialized over `source`.
    fn one_fix(source: &str, hits: &[Lint]) -> RuleFix {
        let lines = LineIndex::new(source);
        let fixes = from_lints(hits, source, &lines);
        assert_eq!(fixes.len(), 1, "expected exactly one fix for {source:?}");
        fixes.into_iter().next().expect("one fix")
    }

    /// A width-0 `unnecessary_new` row over the keyword at `offset` — the
    /// span and message the rule reports (`test/rules/unnecessary_new`).
    fn new_row(offset: u32) -> Lint {
        Lint {
            rule: rule_by_name("unnecessary_new").expect("wired registry"),
            range: offset..offset,
            message: "Unnecessary 'new' keyword.".to_owned(),
            correction: None,
        }
    }

    /// The `avoid_empty_else` row over the else `;` at `offset..offset+1`.
    fn empty_else_row(offset: u32) -> Lint {
        Lint {
            rule: rule_by_name("avoid_empty_else").expect("wired registry"),
            range: offset..offset + 1,
            message: "Empty statements are not allowed in an 'else' clause.".to_owned(),
            correction: None,
        }
    }

    /// End to end: the `unnecessary_new` row becomes an anchored edit whose
    /// application rewrites the source; the anchor flanks the keyword.
    #[test]
    fn new_expression_end_to_end() {
        let source = "void f() { var a = new B(); }\n";
        let fix = one_fix(source, &[new_row(19)]);
        assert_eq!(fix.rule, "unnecessary_new");
        assert_eq!(fix.edit.offset, 19);
        assert_eq!(fix.edit.length, 4, "the keyword plus its trailing space");
        assert_eq!(fix.edit.replacement, "");
        assert_eq!(fix.edit.anchor.line, 1);
        assert_eq!(fix.edit.anchor.column, 20);
        assert_eq!(fix.edit.anchor.context_before, "var a = ");
        assert_eq!(fix.edit.anchor.context_after, "B(); }");

        let (patched, outcomes) = apply(source, std::slice::from_ref(&fix.edit));
        assert_eq!(patched, "void f() { var a = B(); }\n");
        assert_eq!(outcomes, &[EditOutcome::Applied]);
    }

    /// Second rule end to end: the else `;` rewrites to `{}`.
    #[test]
    fn avoid_empty_else_end_to_end() {
        let source = "if (b) ; else ;";
        let fix = one_fix(source, &[empty_else_row(14)]);
        assert_eq!(fix.rule, "avoid_empty_else");
        assert_eq!(fix.edit.offset, 14);
        assert_eq!(fix.edit.length, 1);
        assert_eq!(fix.edit.replacement, "{}");

        let (patched, outcomes) = apply(source, std::slice::from_ref(&fix.edit));
        assert_eq!(patched, "if (b) ; else {}");
        assert_eq!(outcomes, &[EditOutcome::Applied]);
    }

    /// §15.6's drift case: an unrelated overlay above the edit shifts its
    /// offset; the anchor re-locates it (a unique context match off the
    /// anchor line) and the rewrite lands on the moved construct.
    #[test]
    fn drift_relocates_by_anchor() {
        let source = "void f() { var a = new B(); }\n";
        let edit = one_fix(source, &[new_row(19)]).edit;

        let drifted = format!("// header overlay\n{source}");
        let (patched, outcomes) = apply(&drifted, &[edit]);
        assert_eq!(outcomes, &[EditOutcome::Applied]);
        assert_eq!(patched, "// header overlay\nvoid f() { var a = B(); }\n");
    }

    /// A vanished construct goes stale — never a blind write; two
    /// equally-flanked candidates on the anchor line are ambiguous and go
    /// stale too. Both scenarios shift the claimed position first (an
    /// exact-position match is provable wherever it still holds).
    #[test]
    fn stale_when_gone_or_ambiguous() {
        let source = "x(new B());\n";
        let edit = one_fix(source, &[new_row(2)]).edit;
        assert_eq!(edit.anchor.context_before, "x(");

        let (patched, outcomes) = apply("void f() { var a = C(); }\n", std::slice::from_ref(&edit));
        assert_eq!(outcomes, &[EditOutcome::Stale]);
        assert_eq!(patched, "void f() { var a = C(); }\n", "stale never writes");

        // Two spaces shift both occurrences off the claimed offset; both
        // flank equally on the anchor line → ambiguous → stale.
        let (patched, outcomes) = apply("  x(new B()); x(new B());\n", std::slice::from_ref(&edit));
        assert_eq!(outcomes, &[EditOutcome::Stale]);
        assert_eq!(patched, "  x(new B()); x(new B());\n");
    }

    /// Two edits claiming one span: the first (highest offset, input order
    /// on ties) applies, the overlap goes stale.
    #[test]
    fn overlap_goes_stale() {
        let source = "if (b) ; else ;";
        let fix = one_fix(source, &[empty_else_row(14)]);
        let overwrite = Edit {
            offset: fix.edit.offset,
            length: 1,
            source: ";".to_owned(),
            replacement: String::new(),
            anchor: fix.edit.anchor.clone(),
        };
        let (patched, outcomes) = apply(source, &[fix.edit.clone(), overwrite]);
        assert_eq!(outcomes, &[EditOutcome::Applied, EditOutcome::Stale]);
        assert_eq!(patched, "if (b) ; else {}");
    }

    /// The machine document is §15.6 verbatim: exactly `file`,
    /// `schema_version`, and per-edit `offset`/`length`/`replacement`/
    /// `anchor` — the anchor carrying `line`/`column`/`context_before`/
    /// `context_after`; an empty context is omitted (§15.7).
    #[test]
    fn schema_pins_section_15_6_fields() {
        let source = "void f() { var a = new B(); }\n";
        let lines = LineIndex::new(source);
        let doc = patch_json(&FileFixes {
            path: "/abs/t.dart".to_owned(),
            fixes: from_lints(&[new_row(19)], source, &lines),
        });

        let keys: Vec<_> = doc.as_object().expect("document object").keys().collect();
        assert_eq!(keys, ["edits", "file", "schema_version"]);
        assert_eq!(doc["schema_version"], 1);
        assert_eq!(doc["file"], "/abs/t.dart");

        let edit = &doc["edits"][0];
        let keys: Vec<_> = edit.as_object().expect("edit object").keys().collect();
        assert_eq!(keys, ["anchor", "length", "offset", "replacement"]);
        assert_eq!(edit["offset"], 19);
        assert_eq!(edit["length"], 4);
        assert_eq!(edit["replacement"], "");

        let anchor = &edit["anchor"];
        let keys: Vec<_> = anchor.as_object().expect("anchor object").keys().collect();
        assert_eq!(keys, ["column", "context_after", "context_before", "line"]);
        assert_eq!(anchor["line"], 1);
        assert_eq!(anchor["column"], 20);
        assert_eq!(anchor["context_before"], "var a = ");
        assert_eq!(anchor["context_after"], "B(); }");

        // An edit at a line start carries no context_before: the empty
        // string is omitted from the document (§15.7).
        let text = "var a;\n";
        let lines = LineIndex::new(text);
        let doc = patch_json(&FileFixes {
            path: "t.dart".to_owned(),
            fixes: vec![RuleFix {
                rule: "unnecessary_new",
                message: String::new(),
                edit: anchored_edit(text, &lines, 0, 3, "out"),
            }],
        });
        let anchor = &doc["edits"][0]["anchor"];
        let keys: Vec<_> = anchor.as_object().expect("anchor object").keys().collect();
        assert_eq!(keys, ["column", "context_after", "line"]);
        assert_eq!(anchor["context_after"], " a;");
    }

    /// Rules without a mechanical rewrite are never claimed: a hit of a
    /// non-wired rule produces no fix, and a wired rule's row whose slice
    /// does not verify produces none either.
    #[test]
    fn unverified_rows_produce_no_fix() {
        let source = "class foo {}\n";
        let lines = LineIndex::new(source);
        let non_wired = vec![Lint {
            rule: rule_by_name("camel_case_types").expect("wired registry"),
            range: 6..9,
            message: "Type names should be CamelCase.".to_owned(),
            correction: None,
        }];
        assert!(from_lints(&non_wired, source, &lines).is_empty());

        // `unnecessary_new` over text that does not start with the keyword.
        let wrong = vec![new_row(6)];
        assert!(from_lints(&wrong, source, &lines).is_empty());

        // `avoid_empty_else` over a span that is not the `;`.
        let wrong = vec![empty_else_row(0)];
        assert!(from_lints(&wrong, source, &lines).is_empty());
    }
}
