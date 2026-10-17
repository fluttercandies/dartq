//! The rename face's core: one anchor position resolved through the
//! engine's `resolve_at` substrate, joined with the by-name reference rows
//! (the same join the `query references` face serves), projected onto a
//! whole-identifier edit set — one edit per declaration name token and
//! resolved use, ordered by path then offset. The CLI's `dartq rename`
//! prints the set as the machine document and, under `--write`, splices it
//! into the on-disk files; the daemon shapes the same result into its RPC
//! answer.
//!
//! # Contract
//!
//! * The anchor is a `(path, position)` pair; the position converts
//!   against the analyzed snapshot's own text (the bytes and line index
//!   the rows were built from), so a position and the rows can never
//!   disagree.
//! * A position the rows do not cover answers the empty edit set (the
//!   query family's zero-results posture), not an error.
//! * The new name validates before any analysis work: a legal Dart
//!   identifier per the scanner's own rule (ASCII letter, `_`, or `$`
//!   start; ASCII alphanumerics, `_`, and `$` continue — identifiers are
//!   ASCII-only, the scanner's contract) and not a reserved word (the
//!   scanner's keyword classification; built-in and pseudo keywords stay
//!   legal identifier spellings). Shadowing is legal Dart semantics, so
//!   same-name collisions are deliberately not refused — the reference
//!   join already scopes the edit set to the resolved target.
//! * Edit rows carry UTF-16 offsets (the engine's external convention)
//!   plus 1-based line/column locators, file paths as the file table's
//!   canonical spellings — the identity spelling a `--write` splices
//!   against. Rows sort by canonical path bytes, then offset: same input,
//!   same output.
//! * `patched_text` applies one file's edits to its on-disk bytes:
//!   offset-descending splices over a length-verified text, every site
//!   checked against the expected identifier — a file that drifted since
//!   the analysis refuses instead of corrupting.

use std::path::{Path, PathBuf};

use dartq_diagnostics::FileLayoutProvider as _;
use dartq_engine::{DeclKind, Engine, PositionAnswer};
use dartq_scanner::{KeywordStyle, keyword_from_str};
use dartq_source::{FileId, SourceRange};
use serde_json::{Value, json};
use thiserror::Error;

use crate::analyze::{AnalyzeError, prelude};
use crate::layout::{EngineLayout, path_key};

/// Why a rename run refuses. The anchor and name refusals are the usage
/// tier (the invocation itself is wrong); drift and analysis failures are
/// the internal tier.
#[derive(Debug, Error)]
pub enum RenameError {
    /// The anchor file was never interned by the analyzed workspace's
    /// discovery (outside the package set, or excluded).
    #[error("not a file of the analyzed workspace: {0}")]
    NotResident(PathBuf),
    /// The anchor position does not land inside the file's text.
    #[error("{path}: anchor {anchor} is outside the file's text ({detail})")]
    OutOfRange {
        /// The anchor file.
        path: PathBuf,
        /// The position as the caller spelled it.
        anchor: String,
        /// Why the position is out of range.
        detail: String,
    },
    /// The new name is not a legal Dart identifier or is a reserved word.
    #[error("new name must be a legal Dart identifier and not a reserved word: {0}")]
    IllegalName(String),
    /// An edited file's on-disk text no longer matches the analyzed view
    /// (drifted since the analysis) — the write refuses rather than splice
    /// stale offsets.
    #[error("{0} changed since the analysis; re-run the rename against the current files")]
    Stale(PathBuf),
    /// The discover+analyze prelude failed.
    #[error(transparent)]
    Analyze(#[from] AnalyzeError),
}

/// The anchor position inside the target file. Both forms convert against
/// the analyzed text; the line/column column convention is the engine's
/// diagnostics convention (1-based line, 1-based UTF-16 code-unit column).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Anchor {
    /// 1-based line and column.
    LineCol { line: u32, column: u32 },
    /// 0-based byte offset (on a UTF-8 character boundary).
    ByteOffset(u64),
}

impl Anchor {
    /// The position as a message spells it (the error text's `anchor`).
    fn spelled(self) -> String {
        match self {
            Self::LineCol { line, column } => format!("{line}:{column}"),
            Self::ByteOffset(offset) => format!("offset {offset}"),
        }
    }
}

/// One rename run: the renamed declaration's identity plus the whole
/// edit set (the declaration's name token and every resolved use), in the
/// printed order — canonical path bytes, then offset.
pub struct RenameRun<'a> {
    /// The renamed declaration; `None` when the anchor lands on a position
    /// the maintained rows do not cover (the empty edit set answers).
    pub target: Option<RenameTarget<'a>>,
    /// The edit rows, path-then-offset order. The declaration row rides
    /// among them (a declaration's name token is an edit like any use).
    pub edits: Vec<RenameEdit>,
    /// Position/path resolution over the analyzed engine.
    pub layout: EngineLayout<'a>,
}

/// The renamed declaration's identity.
#[derive(Clone, Copy, Debug)]
pub struct RenameTarget<'a> {
    /// The declaring file.
    pub file: FileId,
    /// The declaration family.
    pub kind: DeclKind,
    /// The renamed identifier's display spelling (a setter's `name=`
    /// lookup spelling stripped to `name` — the `=` is the lookup key's
    /// marker, never part of the source identifier).
    pub name: &'a str,
    /// The enclosing container's name; [`None`] for top-level rows.
    pub container: Option<&'a str>,
    /// The declaration's name-token span (UTF-16).
    pub range: SourceRange,
}

/// One whole-identifier edit: the token's file and span. The new text is
/// the run's new name for every row (the old text is the target's display
/// spelling everywhere by construction).
#[derive(Clone, Copy, Debug)]
pub struct RenameEdit {
    /// The edited file.
    pub file: FileId,
    /// The identifier token's span (UTF-16, half-open).
    pub range: SourceRange,
}

/// Runs the rename face over a fresh engine: the name gate, the
/// discover+analyze prelude (the read-after-write the edit set reflects),
/// then the plan.
///
/// # Errors
///
/// [`RenameError::IllegalName`] before any analysis work;
/// [`RenameError::Analyze`] on prelude failure; the anchor refusals
/// otherwise.
pub fn rename<'a>(
    engine: &'a mut Engine,
    target: &Path,
    anchor: Anchor,
    new_name: &str,
) -> Result<RenameRun<'a>, RenameError> {
    if !is_legal_new_name(new_name) {
        return Err(RenameError::IllegalName(new_name.to_owned()));
    }
    let _ = prelude(engine)?;
    plan(engine, target, anchor)
}

/// The rename plan over an already-analyzed engine — the daemon path,
/// whose read task has just run the analysis pass.
///
/// # Panics
///
/// Panics when the anchor file lacks resident content or a line index —
/// an internal invariant breach (a file the discovery interned is
/// analyzed and resident, the same law [`crate::layout::EngineLayout`]
/// pins).
pub fn plan<'a>(
    engine: &'a Engine,
    target: &Path,
    anchor: Anchor,
) -> Result<RenameRun<'a>, RenameError> {
    let range = |error: &str| RenameError::OutOfRange {
        path: target.to_owned(),
        anchor: anchor.spelled(),
        detail: error.to_owned(),
    };
    let Some(file) = engine.file_table().try_file_id(target) else {
        return Err(RenameError::NotResident(target.to_owned()));
    };
    // The conversion reads the analyzed snapshot's own text — the bytes
    // the rows were built from — so an unswept disk drift cannot
    // desynchronize the position and the rows.
    let text = std::str::from_utf8(engine.content(file).expect("analyzed FileId content"))
        .expect("analyzed content is UTF-8");
    let index = engine.line_index(file).expect("analyzed FileId line index");
    let offset = match anchor {
        Anchor::ByteOffset(byte) => {
            let Ok(byte) = u32::try_from(byte) else {
                return Err(range("past the end of the file"));
            };
            if byte > text.len() as u32 {
                return Err(range(&format!(
                    "past the end of the file ({} bytes)",
                    text.len()
                )));
            }
            if !text.is_char_boundary(byte as usize) {
                return Err(range("inside a multi-byte character"));
            }
            index.byte_to_utf16(text, byte)
        }
        Anchor::LineCol { line, column } => {
            let lines = index.line_count();
            if line == 0 || column == 0 {
                return Err(range("line and column are 1-based"));
            }
            let Some((start, end)) = line_bounds(text, line) else {
                return Err(range(&format!(
                    "no line {line} (the file has {lines} lines)"
                )));
            };
            let (start, end) = (
                index.byte_to_utf16(text, start as u32),
                index.byte_to_utf16(text, end as u32),
            );
            let units = end - start;
            if column - 1 > units {
                return Err(range(&format!("line {line} is {units} code units long")));
            }
            start + (column - 1)
        }
    };

    // Every uncovered shape answers the empty edit set, not an error (the
    // query family's zero-results posture): an unresolved bare name, an
    // import prefix, a member the M1 chain does not model, or any
    // non-identifier position.
    let Some(answer) = engine.resolve_at(file, offset) else {
        return Ok(RenameRun {
            target: None,
            edits: Vec::new(),
            layout: EngineLayout::over(engine, [file]),
        });
    };
    let (target_file, lookup_name, declaration) = match answer {
        PositionAnswer::Declaration(decl) => (decl.file, decl.name, decl.range),
        PositionAnswer::Use(use_hit) => (use_hit.target_file, use_hit.name, use_hit.target_range),
    };

    // The edit rows: the references join's resolved uses filtered to the
    // target, plus the declaration's own name token. The join is
    // name-keyed, so the target filter scopes the set to the resolved
    // declaration — a shadowing declaration's uses target their own
    // declaration's file and stay out.
    let mut edits: Vec<RenameEdit> = engine
        .query_symbols(lookup_name)
        .refs
        .iter()
        .filter(|hit| hit.target_file == target_file)
        .map(|hit| RenameEdit {
            file: hit.file,
            range: hit.range,
        })
        .collect();
    edits.push(RenameEdit {
        file: target_file,
        range: declaration,
    });

    let layout = EngineLayout::over(engine, edits.iter().map(|edit| edit.file));
    // Output determinism: canonical path byte order, then position (the
    // query family's printed contract).
    edits.sort_by(|a, b| {
        path_key(layout.file_path(a.file))
            .cmp(path_key(layout.file_path(b.file)))
            .then_with(|| a.range.offset.cmp(&b.range.offset))
    });
    Ok(RenameRun {
        target: Some(RenameTarget {
            file: target_file,
            kind: match answer {
                PositionAnswer::Declaration(decl) => decl.kind,
                PositionAnswer::Use(use_hit) => use_hit.decl_kind,
            },
            name: lookup_name.strip_suffix('=').unwrap_or(lookup_name),
            container: match answer {
                PositionAnswer::Declaration(decl) => decl.container,
                PositionAnswer::Use(use_hit) => use_hit.container,
            },
            range: declaration,
        }),
        edits,
        layout,
    })
}

/// The new-name gate: the scanner's identifier rule (ASCII letter, `_`, or
/// `$` start; ASCII alphanumerics, `_`, and `$` continue — identifiers are
/// ASCII-only, the scanner's own contract) and not a reserved word (the
/// scanner's keyword classification — one table, no face fork; built-in
/// and pseudo keywords stay legal identifier spellings).
#[must_use]
pub fn is_legal_new_name(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    let shaped = (first.is_ascii_alphabetic() || first == '_' || first == '$')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$');
    shaped
        && !keyword_from_str(name).is_some_and(|keyword| keyword.style() == KeywordStyle::Reserved)
}

/// The first file outside `workspace` (canonical spellings both sides —
/// the file table interns canonical paths and the workspace assembly
/// returns the canonical directory), `None` when every file is inside.
/// The `--write` gate: only the analyzed workspace's own files are
/// writable.
pub fn outside_workspace<'a>(
    paths: impl IntoIterator<Item = &'a Path>,
    workspace: &Path,
) -> Option<&'a Path> {
    paths.into_iter().find(|path| !path.starts_with(workspace))
}

/// Applies one file's edits to its on-disk text and returns the patched
/// buffer: the UTF-16 edit ranges convert through the analyzed view's line
/// index, the text length is verified against the analyzed bytes first
/// (drift refuses before any conversion), and every spliced site is
/// checked against the expected identifier. Offset-descending splices keep
/// the earlier ranges valid; identifier tokens never overlap.
///
/// # Errors
///
/// [`RenameError::Stale`] when the disk text no longer matches the
/// analyzed view (length drift, or an edit site whose bytes are not the
/// identifier being renamed). A run without a target carries no edits, so
/// the patch is the text itself.
///
/// # Panics
///
/// Panics when `file` lacks a resident line index — an internal invariant
/// breach (an edited file came from the analyzed rows).
pub fn patched_text(
    run: &RenameRun<'_>,
    file: FileId,
    disk_text: &str,
    new_name: &str,
) -> Result<String, RenameError> {
    let Some(target) = &run.target else {
        return Ok(disk_text.to_owned());
    };
    let engine = run.layout.engine();
    let index = engine.line_index(file).expect("edited file line index");
    let edits: Vec<RenameEdit> = run
        .edits
        .iter()
        .filter(|edit| edit.file == file)
        .copied()
        .collect();
    if edits.is_empty() {
        return Ok(disk_text.to_owned());
    }
    let path = engine.file_table().path(file).expect("edited file path");
    // The length gate: the offsets belong to the analyzed text, so a disk
    // text of a different byte length is drift by definition — refused
    // before the conversions (which index the analyzed view) can misread.
    let analyzed_len = engine.content(file).expect("edited file content").len();
    if disk_text.len() != analyzed_len {
        return Err(RenameError::Stale(path.to_owned()));
    }
    let old = target.name;
    let mut patched = disk_text.to_owned();
    for edit in edits.iter().rev() {
        let start = index.utf16_to_byte(disk_text, edit.range.offset) as usize;
        let end = index.utf16_to_byte(disk_text, edit.range.offset + edit.range.length) as usize;
        if disk_text.get(start..end) != Some(old) {
            return Err(RenameError::Stale(path.to_owned()));
        }
        patched.replace_range(start..end, new_name);
    }
    Ok(patched)
}

/// The byte bounds of 1-based `line` in `text` (content plus its
/// terminator, the same `\n` / `\r\n` / `\r` breaks the engine's line
/// index records), `None` past the last line. One linear walk per call —
/// the anchor lookup runs once per invocation, never per row.
fn line_bounds(text: &str, line: u32) -> Option<(usize, usize)> {
    let bytes = text.as_bytes();
    let is_break = |b: u8| b == b'\n' || b == b'\r';
    let mut start = 0usize;
    for _ in 1..line {
        let at = start + bytes[start..].iter().position(|&b| is_break(b))?;
        let mut next = at + 1;
        if bytes[at] == b'\r' && bytes.get(next) == Some(&b'\n') {
            next += 1;
        }
        start = next;
    }
    let end = bytes[start..]
        .iter()
        .position(|&b| is_break(b))
        .map_or(bytes.len(), |at| start + at + 1);
    Some((start, end))
}

/// The JSON face: one object — `schema_version`, `new_name`, the
/// `target` identity (null on a position miss), and the `edits` array in
/// the run's order. Zero edits produce an empty array. The single
/// implementation of the rename output face: the CLI writes it verbatim
/// and the daemon returns it, so the two cannot drift.
#[must_use]
pub fn to_json(run: &RenameRun<'_>, new_name: &str) -> Value {
    let target = run.target.as_ref().map(|target| {
        let (line, column) = run.layout.line_col(target.file, target.range.offset);
        json!({
            "file": run.layout.file_path(target.file).to_string_lossy(),
            "kind": crate::query::decl_kind_name(target.kind),
            "name": target.name,
            "container": target.container,
            "offset": target.range.offset,
            "length": target.range.length,
            "line": line,
            "column": column,
        })
    });
    let edits = run
        .edits
        .iter()
        .map(|edit| {
            let (line, column) = run.layout.line_col(edit.file, edit.range.offset);
            json!({
                "file": run.layout.file_path(edit.file).to_string_lossy(),
                "offset": edit.range.offset,
                "length": edit.range.length,
                "line": line,
                "column": column,
            })
        })
        .collect::<Vec<_>>();
    json!({
        "schema_version": crate::SCHEMA_VERSION,
        "new_name": new_name,
        "target": target,
        "edits": edits,
    })
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::workspace::{open_engine, test_fixture::workspace};
    use dartq_engine::DeclKind;

    /// The declaring surface (a.dart) and its importing use site (b.dart).
    const A: &str = "void fa() {}\n";
    const B: &str = "import 'a.dart';\n\nvoid caller() {\n  fa();\n}\n";
    /// The shadowing shape: b.dart declares its own `fa` (the local hop
    /// beats the import), so its `fa()` call resolves locally; c.dart's
    /// call is the one that still resolves to a.dart's declaration.
    const SHADOW_B: &str = "import 'a.dart';\n\nvoid fa() {}\n\nvoid caller() {\n  fa();\n}\n";
    const C: &str = "import 'a.dart';\n\nvoid other() {\n  fa();\n}\n";

    fn canonical(root: &std::path::Path, relative: &str) -> PathBuf {
        root.canonicalize().expect("canonical root").join(relative)
    }

    /// The edit set for one anchor, borrowed paths resolved to strings.
    fn edit_files(run: &RenameRun<'_>) -> Vec<String> {
        run.edits
            .iter()
            .map(|edit| {
                run.layout
                    .file_path(edit.file)
                    .to_string_lossy()
                    .into_owned()
            })
            .collect()
    }

    /// The cross-file join: the anchor on a.dart's `fa` (the byte-offset
    /// anchor form) edits the declaration token and every resolved use,
    /// including the one in b.dart, in path-then-offset order.
    #[test]
    fn rename_edits_the_declaration_and_every_use_across_files() {
        let root = workspace("rename_cross", &[("lib/a.dart", A), ("lib/b.dart", B)]);
        let (mut engine, _) = open_engine(&root, Some(&root.join("sdk"))).expect("engine");
        let a = canonical(&root, "lib/a.dart");
        let b = canonical(&root, "lib/b.dart");

        let run = rename(
            &mut engine,
            &a,
            Anchor::ByteOffset(A.find("fa").expect("decl") as u64),
            "gb",
        )
        .expect("rename");
        let target = run.target.as_ref().expect("the renamed declaration");
        assert_eq!(target.name, "fa");
        assert_eq!(target.kind, DeclKind::TopLevelFn);
        assert_eq!(target.container, None);
        assert_eq!(target.range.offset as usize, A.find("fa").expect("decl"));

        assert_eq!(
            edit_files(&run),
            vec![a.to_string_lossy(), b.to_string_lossy()]
        );
        let use_offset = B.find("fa()").expect("use");
        let spans: Vec<(usize, usize)> = run
            .edits
            .iter()
            .map(|edit| (edit.range.offset as usize, edit.range.length as usize))
            .collect();
        assert_eq!(
            spans,
            vec![(A.find("fa").expect("decl"), 2), (use_offset, 2)],
            "path-major, offset-ascending within a file"
        );
        // Every row is the whole identifier token: the old spelling sits
        // exactly at each span in the file's own text.
        for (text, (offset, length)) in [A, B].iter().zip(spans) {
            assert_eq!(&text[offset..offset + length], "fa");
        }

        let json = to_json(&run, "gb");
        assert_eq!(json["schema_version"], 1);
        assert_eq!(json["new_name"], "gb");
        assert_eq!(json["target"]["name"], "fa");
        assert_eq!(json["target"]["line"], 1);
        assert_eq!(json["edits"].as_array().expect("edits array").len(), 2);
        assert_eq!(json["edits"][1]["file"], b.to_string_lossy().into_owned());

        std::fs::remove_dir_all(&root).ok();
    }

    /// The shadowing law: the edit set scopes to the resolved declaration
    /// only. Renaming a.dart's `fa` keeps b.dart's shadowing declaration
    /// and its uses untouched (they resolve locally) while c.dart's use
    /// joins; anchoring on the shadow renames only the shadow's own rows.
    #[test]
    fn rename_keeps_the_shadowing_declarations_uses() {
        let root = workspace(
            "rename_shadow",
            &[
                ("lib/a.dart", A),
                ("lib/b.dart", SHADOW_B),
                ("lib/c.dart", C),
            ],
        );
        let (mut engine, _) = open_engine(&root, Some(&root.join("sdk"))).expect("engine");
        let a = canonical(&root, "lib/a.dart");
        let b = canonical(&root, "lib/b.dart");
        let c = canonical(&root, "lib/c.dart");

        // a.dart's `fa` (1-based line 1, column 6 — the line-column anchor
        // form): a.dart's declaration plus c.dart's resolved use, and
        // nothing in b.dart.
        let run = rename(
            &mut engine,
            &a,
            Anchor::LineCol { line: 1, column: 6 },
            "gb",
        )
        .expect("rename");
        assert_eq!(
            edit_files(&run),
            vec![a.to_string_lossy(), c.to_string_lossy()]
        );

        // The shadow itself (b.dart line 3): its declaration and its own
        // call, and nothing beyond b.dart.
        let run = rename(
            &mut engine,
            &b,
            Anchor::LineCol { line: 3, column: 6 },
            "gb",
        )
        .expect("rename");
        assert_eq!(
            edit_files(&run),
            vec![b.to_string_lossy(), b.to_string_lossy()]
        );
        let offsets: Vec<usize> = run
            .edits
            .iter()
            .map(|edit| edit.range.offset as usize)
            .collect();
        assert_eq!(
            offsets,
            vec![
                SHADOW_B.find("void fa").expect("shadow decl") + 5,
                // The use site inside `caller` — `find` would land on the
                // declaration's own `fa()`.
                SHADOW_B.rfind("fa()").expect("shadow use"),
            ],
            "the shadow's declaration token, then its own use"
        );

        std::fs::remove_dir_all(&root).ok();
    }

    /// The new-name gate: legal identifiers pass, and reserved words fail
    /// while built-in and pseudo keywords stay legal — the scanner's own
    /// classification, pinned at its rename consumption point; the gate
    /// refuses before any analysis work end to end.
    #[test]
    fn the_new_name_gate_rejects_keywords_and_bad_identifiers() {
        assert!(is_legal_new_name("gb"));
        assert!(is_legal_new_name("_private"));
        assert!(is_legal_new_name("$dollar"));
        assert!(is_legal_new_name("a1_b$c"));
        assert!(is_legal_new_name("dynamic"), "built-in keyword");
        assert!(is_legal_new_name("await"), "pseudo keyword");
        assert!(!is_legal_new_name("class"), "reserved word");
        assert!(!is_legal_new_name("void"), "reserved word");
        assert!(!is_legal_new_name(""), "empty");
        assert!(!is_legal_new_name("1abc"), "digit start");
        assert!(!is_legal_new_name("a b"), "space");
        assert!(!is_legal_new_name("a-b"), "hyphen");
        assert!(
            !is_legal_new_name("é"),
            "non-ASCII (the scanner is ASCII-only)"
        );

        let root = workspace("rename_gate", &[("lib/a.dart", A)]);
        let (mut engine, _) = open_engine(&root, Some(&root.join("sdk"))).expect("engine");
        let a = canonical(&root, "lib/a.dart");
        for bad in ["class", "1abc", "a-b"] {
            let Err(error) = rename(&mut engine, &a, Anchor::ByteOffset(5), bad) else {
                panic!("the gate refuses `{bad}`");
            };
            let error = error.to_string();
            assert!(error.contains(bad), "{bad}: {error}");
        }
        std::fs::remove_dir_all(&root).ok();
    }

    /// A position the rows do not cover (any non-identifier position) is
    /// the empty edit set — `target` null, `edits` empty — not an error.
    #[test]
    fn a_miss_answers_the_empty_edit_set() {
        let root = workspace("rename_miss", &[("lib/a.dart", A)]);
        let (mut engine, _) = open_engine(&root, Some(&root.join("sdk"))).expect("engine");
        let a = canonical(&root, "lib/a.dart");

        let run = rename(&mut engine, &a, Anchor::ByteOffset(0), "gb").expect("rename");
        assert!(run.target.is_none());
        assert!(run.edits.is_empty());
        let json = to_json(&run, "gb");
        assert_eq!(json["target"], serde_json::Value::Null);
        assert_eq!(json["edits"].as_array().expect("edits array").len(), 0);

        std::fs::remove_dir_all(&root).ok();
    }

    /// The `--write` round trip: every edited file's patched buffer
    /// renames exactly the identifier tokens, on disk.
    #[test]
    fn patched_text_round_trips_the_rename_on_disk() {
        let root = workspace("rename_write", &[("lib/a.dart", A), ("lib/b.dart", B)]);
        let (mut engine, _) = open_engine(&root, Some(&root.join("sdk"))).expect("engine");
        let a = canonical(&root, "lib/a.dart");

        let run = rename(&mut engine, &a, Anchor::ByteOffset(5), "gb").expect("rename");
        for edit in run.edits.clone() {
            let path = run.layout.file_path(edit.file).to_owned();
            let text = std::fs::read_to_string(&path).expect("disk text");
            let patched = patched_text(&run, edit.file, &text, "gb").expect("patch");
            std::fs::write(&path, patched).expect("write back");
        }
        assert_eq!(
            std::fs::read_to_string(&a).expect("read"),
            "void gb() {}\n",
            "the declaration renamed in place"
        );
        assert_eq!(
            std::fs::read_to_string(canonical(&root, "lib/b.dart")).expect("read"),
            "import 'a.dart';\n\nvoid caller() {\n  gb();\n}\n",
            "the cross-file use renamed in place"
        );

        std::fs::remove_dir_all(&root).ok();
    }

    /// The drift refusal: a file rewritten after the analysis — different
    /// length, or equal length with the identifier gone — refuses with
    /// [`RenameError::Stale`] instead of splicing stale offsets.
    #[test]
    fn patched_text_refuses_a_drifted_file() {
        let root = workspace("rename_drift", &[("lib/a.dart", A)]);
        let (mut engine, _) = open_engine(&root, Some(&root.join("sdk"))).expect("engine");
        let a = canonical(&root, "lib/a.dart");

        let run = rename(&mut engine, &a, Anchor::ByteOffset(5), "gb").expect("rename");
        let file = run.edits[0].file;

        std::fs::write(&a, "void fa() {}\n// drift\n").expect("rewrite");
        let text = std::fs::read_to_string(&a).expect("read");
        assert!(matches!(
            patched_text(&run, file, &text, "gb"),
            Err(RenameError::Stale(_))
        ));

        std::fs::write(&a, "void FB() {}\n").expect("same-length rewrite");
        let text = std::fs::read_to_string(&a).expect("read");
        assert!(matches!(
            patched_text(&run, file, &text, "gb"),
            Err(RenameError::Stale(_))
        ));

        std::fs::remove_dir_all(&root).ok();
    }

    /// The write-boundary gate: every workspace file passes, the first
    /// outside path names itself.
    #[test]
    fn outside_workspace_gates_the_write_boundary() {
        let root = workspace("rename_boundary", &[]);
        let root = root.canonicalize().expect("canonical root");
        let inside = root.join("lib/a.dart");
        let outside = std::env::temp_dir().join("dq_rename_outside.dart");

        assert_eq!(outside_workspace([inside.as_path()], &root), None);
        // The first outside file names itself wherever it sits in the row
        // order.
        assert_eq!(
            outside_workspace([inside.as_path(), outside.as_path()], &root),
            Some(outside.as_path())
        );
        assert_eq!(
            outside_workspace([outside.as_path(), inside.as_path()], &root),
            Some(outside.as_path())
        );

        std::fs::remove_dir_all(&root).ok();
    }
}
