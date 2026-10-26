//! The position faces' shared core: `dartq/resolveAt` (what the analyzed
//! view's maintained index holds under one offset) and `dartq/completion`
//! (the visible-name set at one cursor). One implementation serves both
//! consumers — the daemon's RPC answers and the CLI's one-shot
//! `resolve-at` / `completion` subcommands — so the two faces cannot
//! drift.
//!
//! The JSON answer is the wire contract (`v`/`off`/`{off, len}` ranges), tier-invariant (every field a
//! locator or a verdict — the impact posture; the canonical tables trim
//! nothing here), so there is no verbosity axis and no frozen schema
//! Domain: the shape is the locator set the daemon already serves, kept
//! byte-identical by this move (the daemon's answer layer adds nothing on
//! top). The one-shot entry points run the discover+analyze prelude on a
//! freshly assembled engine and spell the cursor as 1-based
//! `(line, column)` — UTF-16 columns, the engine's external convention —
//! while the daemon paths (whose read task has just run the analysis pass)
//! enter at the `_after_analysis` faces with the raw UTF-16 offset.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use dartq_engine::{CompletionOrigin, Engine, PositionAnswer};

use crate::SCHEMA_VERSION;
use crate::analyze::{AnalyzeError, prelude};
use crate::query::{decl_kind_name, ref_kind_name};

/// The position faces' failure set: the prelude's own failures (the
/// one-shot path only) plus the fail-loud residency rule.
#[derive(Debug, thiserror::Error)]
pub enum ResolveError {
    /// A `file` the analyzed view never interned (the daemon's
    /// INVALID_PARAMS tier — the fail-loud rule for a typo'd path; a
    /// resident-but-unanalyzed file answers `hit: null` like every other
    /// uncovered position).
    #[error("not a resident file: {0}")]
    NotResident(PathBuf),
    /// Workspace discovery or the analysis pass failed (the one-shot
    /// path's internal tier).
    #[error(transparent)]
    Analyze(#[from] AnalyzeError),
}

/// `dartq/resolveAt`, one-shot: the prelude runs (the read-after-write the
/// answer reflects), the `PATH:LINE:COL` cursor converts against the
/// analyzed text, then the shared answer builds.
///
/// # Errors
///
/// [`ResolveError::NotResident`] for a file outside the analyzed view;
/// [`ResolveError::Analyze`] on discovery or analysis failure.
pub fn resolve_at(
    engine: &mut Engine,
    file: &Path,
    line: u32,
    column: u32,
) -> Result<Value, ResolveError> {
    prelude(engine)?;
    let offset = position_offset(engine, file, line, column)?;
    resolve_at_after_analysis(engine, file, offset)
}

/// `dartq/completion`, one-shot: [`resolve_at`]'s posture for the visible-
/// name set at one cursor.
///
/// # Errors
///
/// [`ResolveError::NotResident`] for a file outside the analyzed view;
/// [`ResolveError::Analyze`] on discovery or analysis failure.
pub fn completion(
    engine: &mut Engine,
    file: &Path,
    line: u32,
    column: u32,
) -> Result<Value, ResolveError> {
    prelude(engine)?;
    let offset = position_offset(engine, file, line, column)?;
    completion_after_analysis(engine, file, offset)
}

/// Resolves the 1-based `(line, column)` cursor against the analyzed
/// view's own text. Line breaks follow the line index's semantics (`\n`,
/// `\r\n`, or a lone `\r`); the column counts UTF-16 code units, so a
/// column just past a line's last character is the legitimate cursor
/// position completion answers at.
///
/// # Panics
///
/// Panics when the resident content is not UTF-8 or is missing — both
/// internal invariant breaches (content is validated at `set_content`
/// time; a resident file of the just-analyzed view carries its text).
fn position_offset(
    engine: &Engine,
    file: &Path,
    line: u32,
    column: u32,
) -> Result<u32, ResolveError> {
    let Some(file_id) = engine.file_table().try_file_id(file) else {
        return Err(ResolveError::NotResident(file.to_path_buf()));
    };
    let text = std::str::from_utf8(
        engine
            .content(file_id)
            .expect("analyzed FileId carries its content"),
    )
    .expect("engine content is UTF-8");
    Ok(position_to_offset(text, line, column))
}

/// The UTF-16 offset of a 1-based `(line, column)` cursor over `text`,
/// clamped into the text: an out-of-range line lands past the last line,
/// an out-of-range column on the line's just-past-end position. The clamp
/// keeps the engine's uncovered-position contract (a past-end offset
/// answers the null hit / the empty set, never an error — the daemon's
/// rule), and the envelope echoes the offset actually queried.
#[must_use]
fn position_to_offset(text: &str, line: u32, column: u32) -> u32 {
    let target = line.saturating_sub(1);
    let mut line_no = 0u32;
    // UTF-16 units consumed so far / on the target line.
    let mut offset = 0usize;
    let mut line_units = 0usize;
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        // Line breaks mirror LineIndex::new: `\r\n` is one break, a lone
        // `\r` breaks at itself. On the target line the break character is
        // where a column past the last character lands — the cursor clamps
        // there, before the break is consumed.
        let ends_line = match ch {
            '\n' => true,
            '\r' if chars.peek() == Some(&'\n') => {
                if line_no == target {
                    break;
                }
                chars.next();
                offset += 1;
                true
            }
            '\r' => true,
            _ => false,
        };
        if line_no == target {
            let column_limit = usize::try_from(column.saturating_sub(1)).unwrap_or(usize::MAX);
            if ends_line || line_units >= column_limit {
                break;
            }
            let width = ch.len_utf16();
            line_units += width;
            offset += width;
        } else {
            offset += ch.len_utf16();
            if ends_line {
                line_no += 1;
            }
        }
    }
    u32::try_from(offset).expect("cursor offset within u32 (the engine's range domain)")
}

// ---------------------------------------------------------------------------
// dartq/resolveAt — the position-resolution face
// ---------------------------------------------------------------------------

/// The `dartq/resolveAt` answer over an already-analyzed engine — the
/// daemon path, whose read task has just run the analysis pass (running it
/// twice would double the read's cost). What the analyzed view's
/// maintained index rows hold under `offset` (UTF-16 code units — the
/// engine's external convention, the same units the query rows' `offset`
/// fields carry) in `file`. The engine face carries the full contract
/// (freshness, answer domain, cost shape — `Engine::resolve_at`); this
/// layer adds the wire shape: `hit` is `null` for every uncovered position
/// (a non-identifier, an unresolved use, an out-of-range offset — none is
/// a params error), and a hit carries the identifier's own span plus the
/// target declaration's identity and name span — goto-def's target,
/// hover's subject, rename's anchor.
///
/// # Errors
///
/// A `file` the analyzed view never interned (the fail-loud rule for a
/// typo'd path; a resident-but-unanalyzed file answers `hit: null` like
/// every other uncovered position).
pub fn resolve_at_after_analysis(
    engine: &Engine,
    file: &Path,
    offset: u32,
) -> Result<Value, ResolveError> {
    let Some(file_id) = engine.file_table().try_file_id(file) else {
        return Err(ResolveError::NotResident(file.to_path_buf()));
    };
    // Engine answers carry ids the file table issued; a `forget` between
    // the analyze and this read severs the id's spelling (the immortal-id
    // contract). The locator degrades to the daemon's `<unknown>` spelling
    // (the server's client-label convention), never a panic.
    let path_of = |id| match engine.file_table().path(id) {
        Some(path) => path.display().to_string(),
        None => "<unknown>".to_owned(),
    };
    let hit = match engine.resolve_at(file_id, offset) {
        // The verdict slot: an uncovered position stays an explicit null (not omit-on-absent).
        None => Value::Null,
        Some(PositionAnswer::Declaration(decl)) => {
            let mut hit = json!({
                "kind": "declaration",
                "name": decl.name,
                "decl_kind": decl_kind_name(decl.kind),
                "file": path_of(decl.file),
                "range": { "off": decl.range.offset, "len": decl.range.length },
            });
            // The convenience column is omit-on-absent: a top-level
            // declaration has no container, so the key drops rather than
            // spelling a null.
            if let Some(container) = decl.container {
                hit["container"] = json!(container);
            }
            hit
        }
        Some(PositionAnswer::Use(use_hit)) => {
            let mut hit = json!({
                "kind": "reference",
                "name": use_hit.name,
                "decl_kind": decl_kind_name(use_hit.decl_kind),
                "file": path_of(use_hit.target_file),
                "range": { "off": use_hit.range.offset, "len": use_hit.range.length },
                "target_range": {
                    "off": use_hit.target_range.offset,
                    "len": use_hit.target_range.length,
                },
                "ref_kind": ref_kind_name(use_hit.kind.into()),
                "qualified": use_hit.qualified,
            });
            if let Some(container) = use_hit.container {
                hit["container"] = json!(container);
            }
            if let Some(prefix) = use_hit.prefix {
                hit["prefix"] = json!(prefix);
            }
            hit
        }
    };
    Ok(json!({
        "v": SCHEMA_VERSION,
        "file": file.display().to_string(),
        "off": offset,
        "hit": hit,
    }))
}

// ---------------------------------------------------------------------------
// dartq/completion — the built-in completion face (the visible-name set the
// maintained index proves at one cursor; the charter has no plugin escape)
// ---------------------------------------------------------------------------

/// The stable machine spelling of a completion origin.
#[must_use]
pub fn completion_origin_name(origin: CompletionOrigin) -> &'static str {
    match origin {
        CompletionOrigin::Declaration => "declaration",
        CompletionOrigin::Reference => "reference",
        CompletionOrigin::Prefix => "prefix",
    }
}

/// The `dartq/completion` answer over an already-analyzed engine — the
/// daemon path (see [`resolve_at_after_analysis`]). The candidates the
/// analyzed view's maintained index proves visible at `offset` (UTF-16
/// code units — the engine's external convention) in `file`,
/// prefix-filtered by the identifier fragment the engine extracts from the
/// analyzed text at the cursor. The engine face carries the full contract
/// (freshness, candidate domain, the M1 member/local boundary, cost shape
/// — `Engine::complete_at`); this layer adds the wire shape.
/// Tier-invariant: every field is a locator or a verdict (the impact
/// posture).
///
/// # Errors
///
/// A `file` the analyzed view never interned (the fail-loud rule for a
/// typo'd path, the `dartq/resolveAt` rule). An uncovered position — a
/// past-end offset, a file cold in the view, a member-access cursor —
/// answers the empty set, never an error.
pub fn completion_after_analysis(
    engine: &Engine,
    file: &Path,
    offset: u32,
) -> Result<Value, ResolveError> {
    let Some(file_id) = engine.file_table().try_file_id(file) else {
        return Err(ResolveError::NotResident(file.to_path_buf()));
    };
    // Engine answers carry ids the file table issued; a `forget` between
    // the analyze and this read severs the id's spelling (the immortal-id
    // contract). The locator degrades to the daemon's `<unknown>` spelling
    // (the server's client-label convention), never a panic.
    let path_of = |id| match engine.file_table().path(id) {
        Some(path) => path.display().to_string(),
        None => "<unknown>".to_owned(),
    };
    let answer = engine.complete_at(file_id, offset);
    let (prefix, items) = match &answer {
        Some(answer) => (
            answer.prefix,
            answer
                .items
                .iter()
                .map(|item| {
                    let mut row = json!({
                        "name": item.name,
                        "origin": completion_origin_name(item.origin),
                    });
                    // Omit-on-absent identity columns: a prefix item is a
                    // wire word, not a declaration — no decl_kind,
                    // container, file, or range — so those keys drop
                    // rather than spelling nulls.
                    if let Some(kind) = item.decl_kind {
                        row["decl_kind"] = json!(decl_kind_name(kind));
                    }
                    if let Some(container) = item.container {
                        row["container"] = json!(container);
                    }
                    if let Some(file) = item.file {
                        row["file"] = json!(path_of(file));
                    }
                    if let Some(range) = item.range {
                        row["range"] = json!({ "off": range.offset, "len": range.length });
                    }
                    row
                })
                .collect(),
        ),
        // The uncovered set (a cold file, a past-end offset): the empty
        // answer, never an error.
        None => ("", Vec::new()),
    };
    Ok(json!({
        "v": SCHEMA_VERSION,
        "file": file.display().to_string(),
        "off": offset,
        "prefix": prefix,
        "items": items,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::test_fixture::workspace;

    /// The (line, column) → offset conversions the CLI's `PATH:LINE:COL`
    /// cursor spelling rides: 1-based line, 1-based UTF-16 column, `\r\n`
    /// one break, astral characters two column units, and the clamp
    /// contract (out-of-range line → past the last line; out-of-range
    /// column → the line's just-past-end position).
    #[test]
    fn position_to_offset_pins_the_cursor_grammar() {
        let text = "ab\ncd\r\nef\rgh\n\nij";
        let at = |line: u32, column: u32| position_to_offset(text, line, column);

        assert_eq!(at(1, 1), 0, "start of text");
        assert_eq!(at(1, 3), 2, "just-past-end of line 1 (the break char)");
        assert_eq!(at(2, 1), 3, "line 2 starts after the \\n");
        assert_eq!(at(2, 3), 5, "just-past-end of line 2");
        assert_eq!(at(3, 1), 7, "\\r\\n is one break");
        assert_eq!(at(4, 1), 10, "a lone \\r breaks at itself");
        assert_eq!(at(5, 1), 13, "the empty line");
        assert_eq!(at(5, 2), 13, "past the empty line's end clamps to it");
        assert_eq!(at(6, 1), 14, "the final line without a terminator");
        assert_eq!(at(6, 3), 16, "just-past-end of the final line");

        assert_eq!(at(7, 1), 16, "an out-of-range line clamps to text end");
        assert_eq!(
            at(6, 99),
            16,
            "an out-of-range column clamps to the line end"
        );
        assert_eq!(at(1, 99), 2, "the clamp holds on a broken line too");

        // The column counts UTF-16 units: an astral character is two.
        let wide = "a𐐷b\nc";
        assert_eq!(position_to_offset(wide, 1, 2), 1, "before the astral char");
        assert_eq!(
            position_to_offset(wide, 1, 3),
            3,
            "the astral char is two units"
        );
        assert_eq!(position_to_offset(wide, 1, 4), 3, "column 4 lands on `b`");
        assert_eq!(
            position_to_offset(wide, 1, 5),
            4,
            "past it: the line's break char"
        );
        assert_eq!(position_to_offset(wide, 2, 1), 5);
    }

    /// End to end over the fixture workspace: a declaration position
    /// answers the declaration itself, a use position answers the resolved
    /// reference with the target's identity and name span, an
    /// out-of-range offset answers the explicit null, and a file outside
    /// the analyzed view is the usage-tier error. The resolves ride
    /// library-scope names — the M1 fidelity's member-access boundary
    /// (`Engine::resolve_at`) leaves a `.bar()` select unresolved, the
    /// null-hit verdict.
    #[test]
    fn resolve_at_answers_the_position_faces() {
        let root = workspace(
            "resolve_at",
            &[(
                "lib/a.dart",
                "class Foo {}\nvoid foo() {}\nvoid main() {\n  foo();\n}\n",
            )],
        );
        let file = root.canonicalize().expect("root").join("lib/a.dart");
        let sdk = root.join("sdk");
        let (mut engine, _) = crate::workspace::open_engine(&root, Some(&sdk)).expect("engine");

        // Line 1, column 7: the `Foo` declaration's name token (UTF-16
        // offset 6).
        let answer = resolve_at(&mut engine, &file, 1, 7).expect("answer");
        assert_eq!(answer["v"], serde_json::json!(SCHEMA_VERSION));
        assert_eq!(answer["hit"]["kind"], "declaration");
        assert_eq!(answer["hit"]["name"], "Foo");
        assert_eq!(answer["hit"]["decl_kind"], "class");
        assert_eq!(answer["hit"]["range"]["off"], 6);
        assert_eq!(answer["hit"]["range"]["len"], 3);
        assert_eq!(answer["file"], json!(file.display().to_string()));

        // Line 4, column 3: the resolved use of `foo` (offset 43), whose
        // target span is the declaration's own name token (18).
        let answer = resolve_at(&mut engine, &file, 4, 3).expect("answer");
        assert_eq!(answer["hit"]["kind"], "reference");
        assert_eq!(answer["hit"]["name"], "foo");
        assert_eq!(answer["hit"]["ref_kind"], "invoked");
        assert_eq!(answer["hit"]["range"]["off"], 43);
        assert_eq!(answer["hit"]["target_range"]["off"], 18);
        assert_eq!(answer["hit"]["target_range"]["len"], 3);

        // A non-identifier position answers the explicit null.
        let answer = resolve_at(&mut engine, &file, 1, 1).expect("answer");
        assert!(answer["hit"].is_null(), "the verdict slot stays explicit");

        // A file the view never interned is the usage-tier error.
        let stranger = root.join("missing.dart");
        assert!(matches!(
            resolve_at(&mut engine, &stranger, 1, 1),
            Err(ResolveError::NotResident(_))
        ));

        std::fs::remove_dir_all(&root).ok();
    }

    /// End to end: the visible-name set at a cursor — the local
    /// declarations and the import-proven names, prefix-filtered by the
    /// fragment before the cursor; a member-access cursor answers the
    /// empty set.
    #[test]
    fn completion_answers_the_visible_names() {
        let root = workspace(
            "completion",
            &[("lib/a.dart", "class Foo {}\nvoid main() {\n  Fo\n}\n")],
        );
        let file = root.canonicalize().expect("root").join("lib/a.dart");
        let sdk = root.join("sdk");
        let (mut engine, _) = crate::workspace::open_engine(&root, Some(&sdk)).expect("engine");

        // Line 3, column 5: right after the `Fo` fragment (offset 30, the
        // line's end — the fragment the cursor completes is `Fo`).
        let answer = completion(&mut engine, &file, 3, 5).expect("answer");
        assert_eq!(answer["prefix"], "Fo");
        let items = answer["items"].as_array().expect("items");
        assert_eq!(items.len(), 1, "the fragment filters to the local class");
        assert_eq!(items[0]["name"], "Foo");
        assert_eq!(items[0]["origin"], "declaration");
        assert_eq!(items[0]["decl_kind"], "class");
        assert_eq!(items[0]["file"], json!(file.display().to_string()));

        // A bare cursor (no fragment) answers every visible name.
        let answer = completion(&mut engine, &file, 3, 1).expect("answer");
        assert_eq!(answer["prefix"], "");
        let names: Vec<&str> = answer["items"]
            .as_array()
            .expect("items")
            .iter()
            .map(|row| row["name"].as_str().expect("name"))
            .collect();
        assert!(names.contains(&"Foo"), "the local declaration answers");
        assert!(
            names.contains(&"main"),
            "the enclosing library's own row answers"
        );

        std::fs::remove_dir_all(&root).ok();
    }
}
