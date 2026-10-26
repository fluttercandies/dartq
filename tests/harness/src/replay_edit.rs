//! Pure replay logic: the edit schema,
//! UTF-16-span application, and the canonical normalization that makes the
//! daemon's `dartq/analyze` LSP entries and the batch machine output
//! byte-comparable. No processes and no I/O beyond string handling — every
//! function here is unit-tested.
//!
//! The schema has three step forms, one JSON object
//! per JSONL line: the in-file UTF-16 splice (`file`/`line`/`column`/
//! `length`/`new_text`), the workspace file delete (`remove`), and the
//! workspace file create (`create` + `text`). See [`EditStep`].
//!
//! Volatile-field normalization (the two faces, per the daemon/engine
//! sources): the machine face prints
//! `SEVERITY|TYPE|CODE|FILE|LINE|COL|LENGTH|MESSAGE` (1-based line/column,
//! UTF-16 length, uppercase codes, absolute paths); the LSP face returns
//! `{uri (file://), range {start,end} (0-based UTF-16 line/character),
//! severity (1/2/3), code (snake_case), source ("dartq"), message}`. Both
//! carry no timestamps or internal IDs on their outputs; the representation
//! differences normalize away as: path → corpus-relative (or canonical
//! absolute outside the corpus), severity vocabulary → LSP 1/2/3 scale,
//! code case → lowercase, span → 0-based start + end positions (the
//! machine side's end computed with the engine's own
//! [`dartq_source::LineIndex`], so terminator conventions cannot drift).
//! The machine-only TYPE field and the LSP-only constant `source` field
//! are dropped from the canonical form.

use std::path::{Component, Path, PathBuf};

use serde_json::{Value, json};

use crate::machine::{MachineDiag, Severity};
use dartq_source::{LineIndex, utf16_len_of, utf16_offset_to_byte};

/// One replay step that replaces `length` UTF-16 code units starting at
/// 1-based (`line`, `column`) with `new_text` (empty = deletion, `length`
/// 0 = pure insertion). Coordinates follow the machine-format convention —
/// 1-based lines, 1-based UTF-16 columns, UTF-16 lengths — so a step can be
/// read straight off a differential report. `length` may span line
/// terminators; `column` must address within its line's content (one past
/// the end = insert at end of line).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Edit {
    pub(crate) file: String,
    pub(crate) line: u32,
    pub(crate) column: u32,
    pub(crate) length: u32,
    pub(crate) new_text: String,
}

/// One replay step, in one of the three supported forms:
///
/// - [`EditStep::Splice`] — `{"file":…,"line":…,"column":…,"length":…,
///   "new_text":…}`: an in-file UTF-16 splice (see [`Edit`]).
/// - [`EditStep::Remove`] — `{"remove":"lib/a.dart"}`: deletes the file
///   from the workspace. The driver errors on a target that is neither in
///   the corpus nor created by an earlier step.
/// - [`EditStep::Create`] — `{"create":"lib/b.dart","text":"…"}`: creates
///   the file with `text` as its full content. The driver errors on a
///   target that already exists. `text` is UTF-8 by construction (JSON
///   strings decode and validate), so no separate byte check exists.
///
/// All paths are corpus-relative plain segments (`checked_rel_path`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum EditStep {
    Splice(Edit),
    Remove { file: String },
    Create { file: String, text: String },
}

impl EditStep {
    /// The step's report-facing name (`splice` / `remove` / `create`).
    pub(crate) fn kind(&self) -> &'static str {
        match self {
            EditStep::Splice(_) => "splice",
            EditStep::Remove { .. } => "remove",
            EditStep::Create { .. } => "create",
        }
    }

    /// The workspace file the step addresses; every form carries one.
    pub(crate) fn file(&self) -> &str {
        match self {
            EditStep::Splice(edit) => &edit.file,
            EditStep::Remove { file } | EditStep::Create { file, .. } => file,
        }
    }
}

/// Per-line scalar UTF-16 offsets and content lengths (terminators
/// excluded). Terminator set matches the engine's line index: `\n`,
/// `\r\n`, and lone `\r` (`dartq-source` `source_range` tests pin that set).
#[derive(Debug, Clone)]
pub(crate) struct LineTable {
    /// Scalar UTF-16 offset of each line start; `starts[0] == 0`.
    pub(crate) starts: Vec<u32>,
    /// UTF-16 length of each line's content, terminator excluded. A
    /// trailing unterminated line contributes its content length.
    pub(crate) content_lens: Vec<u32>,
}

/// Walks `text` once, splitting lines on `\n`, `\r\n`, and lone `\r`.
pub(crate) fn line_table(text: &str) -> LineTable {
    let mut starts = vec![0u32];
    let mut content_lens = Vec::new();
    let mut content = 0u32;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\n' => {
                content_lens.push(content);
                starts.push(starts.last().expect("starts never empty") + content + 1);
                content = 0;
            }
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                    content_lens.push(content);
                    starts.push(starts.last().expect("starts never empty") + content + 2);
                } else {
                    content_lens.push(content);
                    starts.push(starts.last().expect("starts never empty") + content + 1);
                }
                content = 0;
            }
            other => {
                content += u32::try_from(other.len_utf16()).expect("utf16 length fits u32");
            }
        }
    }
    content_lens.push(content);
    LineTable {
        starts,
        content_lens,
    }
}

/// The scalar UTF-16 range an [`Edit`] addresses in `source`: validated
/// start (within the addressed line's content) and end (within the text).
pub(crate) fn edit_span_utf16(source: &str, edit: &Edit) -> Result<(u32, u32), String> {
    let table = line_table(source);
    if edit.line == 0 || edit.line as usize > table.starts.len() {
        return Err(format!(
            "line {} out of range (file has {} lines)",
            edit.line,
            table.starts.len()
        ));
    }
    if edit.column == 0 {
        return Err("column is 1-based; 0 is not a column".into());
    }
    let content = table.content_lens[(edit.line - 1) as usize];
    if edit.column - 1 > content {
        return Err(format!(
            "column {} exceeds line {} content length {}",
            edit.column,
            edit.line,
            content + 1
        ));
    }
    let start = table.starts[(edit.line - 1) as usize] + (edit.column - 1);
    let end = start
        .checked_add(edit.length)
        .ok_or_else(|| "edit span overflows u32".to_string())?;
    let total = utf16_len_of(source);
    if end > total {
        return Err(format!(
            "edit span [{start}, {end}) exceeds the text's UTF-16 length {total}"
        ));
    }
    Ok((start, end))
}

/// Applies one [`Edit`] to `source`: UTF-16 range → byte range via the
/// engine's own conversion (surrogate-safe), then splice.
pub(crate) fn apply_edit(source: &str, edit: &Edit) -> Result<String, String> {
    let (start, end) = edit_span_utf16(source, edit)?;
    let start_byte = utf16_offset_to_byte(source, start);
    let end_byte = utf16_offset_to_byte(source, end);
    let mut out = String::with_capacity(source.len() + edit.new_text.len());
    out.push_str(&source[..start_byte]);
    out.push_str(&edit.new_text);
    out.push_str(&source[end_byte..]);
    Ok(out)
}

/// Parses the `--edits` JSONL file: one step per line, blank lines skipped.
/// The whole sequence validates up front so a malformed step fails the run
/// before any process spawns.
pub(crate) fn parse_edits(text: &str) -> Result<Vec<EditStep>, String> {
    let mut edits = Vec::new();
    for (no, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let value: Value = serde_json::from_str(line)
            .map_err(|e| format!("edits line {}: not valid JSON: {e}", no + 1))?;
        edits.push(parse_edit(&value).map_err(|e| format!("edits line {}: {e}", no + 1))?);
    }
    Ok(edits)
}

/// Dispatches on shape: a `remove` or `create` key marks a file-set step,
/// anything else must be the splice form. Mixing forms in one object is a
/// real authoring mistake (the extra fields would be silently unread), so
/// it is rejected rather than ignored.
fn parse_edit(value: &Value) -> Result<EditStep, String> {
    let object = value
        .as_object()
        .ok_or_else(|| "expected a JSON object".to_string())?;
    match (object.get("remove"), object.get("create")) {
        (Some(_), Some(_)) => Err(
            "a step carries either `remove` or `create`, not both; deleting and creating are \
             separate steps"
                .into(),
        ),
        (Some(raw), None) => {
            reject_splice_fields(object, "remove")?;
            let file = raw
                .as_str()
                .ok_or("`remove` must be a string: the corpus-relative file to delete")?
                .to_owned();
            checked_rel_path("remove", &file)?;
            Ok(EditStep::Remove { file })
        }
        (None, Some(raw)) => {
            reject_splice_fields(object, "create")?;
            let file = raw
                .as_str()
                .ok_or("`create` must be a string: the corpus-relative file to create")?
                .to_owned();
            checked_rel_path("create", &file)?;
            let text = match object.get("text").and_then(Value::as_str) {
                Some(text) => text.to_owned(),
                None => {
                    return Err(
                        "`create` needs a string `text`: the new file's full content \
                                    (an empty string is a valid empty file)"
                            .into(),
                    );
                }
            };
            Ok(EditStep::Create { file, text })
        }
        (None, None) => parse_splice(object).map(EditStep::Splice),
    }
}

/// A remove/create step must not carry fields outside its shape: one step
/// is one shape, and the extra fields would be silently unread. (`text` is
/// the `create` step's own content field, so only `remove` rejects it.)
fn reject_splice_fields(object: &serde_json::Map<String, Value>, kind: &str) -> Result<(), String> {
    for name in ["file", "line", "column", "length", "new_text", "text"] {
        if kind == "create" && name == "text" {
            continue;
        }
        if object.contains_key(name) {
            return Err(format!(
                "a `{kind}` step carries only the `{kind}` path field{}; found field \
                 `{name}` — one step is exactly one of: {{file,line,column,length,new_text}} | \
                 {{remove}} | {{create,text}}",
                if kind == "create" { " plus `text`" } else { "" },
            ));
        }
    }
    Ok(())
}

fn parse_splice(object: &serde_json::Map<String, Value>) -> Result<Edit, String> {
    let field = |name: &str| -> Result<&Value, String> {
        object
            .get(name)
            .ok_or_else(|| format!("missing field `{name}`"))
    };
    let file = field("file")?
        .as_str()
        .ok_or("`file` must be a string")?
        .to_owned();
    checked_rel_path("file", &file)?;
    let number = |name: &str| -> Result<u32, String> {
        let raw = field(name)?
            .as_u64()
            .ok_or_else(|| format!("`{name}` must be a non-negative integer"))?;
        u32::try_from(raw).map_err(|_| format!("`{name}` exceeds u32"))
    };
    let edit = Edit {
        file,
        line: number("line")?,
        column: number("column")?,
        length: number("length")?,
        new_text: field("new_text")?
            .as_str()
            .ok_or("`new_text` must be a string")?
            .to_owned(),
    };
    if edit.line == 0 || edit.column == 0 {
        return Err("`line` and `column` are 1-based; 0 is out of range".into());
    }
    Ok(edit)
}

/// Step paths are corpus-relative with plain segments only: no absolute
/// paths, no `..`/`.` components, no separators other than `/`.
fn checked_rel_path(field: &str, raw: &str) -> Result<(), String> {
    if raw.is_empty() {
        return Err(format!("`{field}` must not be empty"));
    }
    if raw.contains('\\') {
        return Err(format!("`{field}` `{raw}` uses `\\`; use `/` separators"));
    }
    let path = Path::new(raw);
    if path.is_absolute() {
        return Err(format!("`{field}` `{raw}` must be corpus-relative"));
    }
    for component in path.components() {
        if !matches!(component, Component::Normal(_)) {
            return Err(format!(
                "`{field}` `{raw}` must be plain corpus-relative segments (no `..`, `.`, or root)"
            ));
        }
    }
    Ok(())
}

/// The one diagnostic shape both faces normalize to: 0-based UTF-16 start
/// and end positions, the LSP severity scale, lowercase code, verbatim
/// message. `key()` serializes it deterministically — the byte string the
/// per-step comparison diffs.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct CanonicalDiag {
    pub(crate) code: String,
    pub(crate) severity: u8,
    pub(crate) start_line: u32,
    pub(crate) start_col: u32,
    pub(crate) end_line: u32,
    pub(crate) end_col: u32,
    pub(crate) message: String,
}

impl CanonicalDiag {
    pub(crate) fn key(&self) -> String {
        serde_json::to_string(&json!([
            self.code,
            self.severity,
            self.start_line,
            self.start_col,
            self.end_line,
            self.end_col,
            self.message,
        ]))
        .unwrap_or_else(|_| "<unserializable>".into())
    }
}

fn tier(severity: Severity) -> u8 {
    match severity {
        Severity::Error => 1,
        Severity::Warning => 2,
        Severity::Info => 3,
    }
}

/// Normalizes one machine-format diagnostic against its file's post-edit
/// text: the end position is computed with the engine's own
/// [`LineIndex`], so the two faces' span conventions cannot drift apart.
pub(crate) fn canonical_from_machine(
    diag: &MachineDiag,
    source: &str,
) -> Result<CanonicalDiag, String> {
    canonical_from_locator(
        &diag.code,
        tier(diag.severity),
        diag.line,
        diag.col,
        diag.length,
        &diag.message,
        source,
    )
}

/// The shared locator→canonical core of both comparison faces: a 1-based
/// line/col plus UTF-16 length over `source`, normalized to the canonical
/// 0-based UTF-16 positions (the end position derived with the engine's
/// own [`LineIndex`], so the faces' span conventions cannot drift apart).
fn canonical_from_locator(
    code: &str,
    severity: u8,
    line: u32,
    col: u32,
    length: u32,
    message: &str,
    source: &str,
) -> Result<CanonicalDiag, String> {
    let table = line_table(source);
    if line == 0 || line as usize > table.starts.len() || col == 0 {
        return Err(format!(
            "diagnostic has out-of-range position {line}:{col}"
        ));
    }
    let start = table.starts[(line - 1) as usize] + (col - 1);
    let end = start
        .checked_add(length)
        .ok_or_else(|| "diagnostic span overflows u32".to_string())?;
    if end > utf16_len_of(source) {
        return Err(format!(
            "diagnostic span [{start}, {end}) exceeds the text length"
        ));
    }
    let index = LineIndex::new(source);
    let (end_line, end_col) = index.line_col_utf16(source, end);
    Ok(CanonicalDiag {
        code: code.to_lowercase(),
        severity,
        start_line: line - 1,
        start_col: col - 1,
        end_line: end_line - 1,
        end_col: end_col - 1,
        message: message.to_owned(),
    })
}

/// Decodes one `dartq/analyze` compact row's `file` field (the entry's
/// `file://` URI) to a path — the row's location key.
pub(crate) fn row_path(entry: &Value) -> Result<PathBuf, String> {
    let file = entry
        .get("file")
        .and_then(Value::as_str)
        .ok_or_else(|| "analyze row field `file` missing or not a string".to_string())?;
    file_uri_to_path(file)
}

/// Normalizes one `dartq/analyze` compact row (`{code, sev, file, line,
/// col, len, msg}` — the short-vocabulary pull face) against its file's
/// post-edit text. The locator caliber matches the batch face's row, so
/// both normalize through the same [`canonical_from_locator`] core.
pub(crate) fn canonical_from_row(entry: &Value, source: &str) -> Result<CanonicalDiag, String> {
    let object = entry
        .as_object()
        .ok_or_else(|| "analyze row is not an object".to_string())?;
    let string_field = |name: &str| -> Result<String, String> {
        object
            .get(name)
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| format!("analyze row field `{name}` missing or not a string"))
    };
    let num_field = |name: &str| -> Result<u32, String> {
        object
            .get(name)
            .and_then(Value::as_u64)
            .and_then(|n| u32::try_from(n).ok())
            .ok_or_else(|| format!("analyze row field `{name}` missing or not a u32"))
    };
    let severity = match string_field("sev")?.as_str() {
        "error" => 1,
        "warning" => 2,
        "info" => 3,
        other => return Err(format!("analyze row sev `{other}` outside error|warning|info")),
    };
    canonical_from_locator(
        &string_field("code")?,
        severity,
        num_field("line")?,
        num_field("col")?,
        num_field("len")?,
        &string_field("msg")?,
        source,
    )
}

pub(crate) fn file_uri_to_path(uri: &str) -> Result<PathBuf, String> {
    let rest = uri
        .strip_prefix("file://")
        .ok_or_else(|| format!("not a file:// URI: {uri}"))?;
    let bytes = rest.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            if i + 3 > bytes.len() {
                return Err(format!("truncated % escape in URI: {uri}"));
            }
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3])
                .map_err(|_| format!("invalid % escape in URI: {uri}"))?;
            let byte = u8::from_str_radix(hex, 16)
                .map_err(|_| format!("invalid % escape in URI: {uri}"))?;
            decoded.push(byte);
            i += 3;
        } else {
            decoded.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(decoded)
        .map(PathBuf::from)
        .map_err(|_| format!("file:// URI does not decode to UTF-8: {uri}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::machine::OutputFormat;

    fn edit(file: &str, line: u32, column: u32, length: u32, new_text: &str) -> Edit {
        Edit {
            file: file.into(),
            line,
            column,
            length,
            new_text: new_text.into(),
        }
    }

    const SAMPLE: &str = "int a = 1;\nint b = 2;\r\nint c = 3;\rint d = 4;\n";

    #[test]
    fn line_table_matches_the_engine_index() {
        // Every text position the walk can address must resolve to the same
        // (line, column) the engine's LineIndex reports — terminator set
        // (\n, \r\n, lone \r) and wide-char arithmetic included.
        let sources = [
            SAMPLE.to_string(),
            "汉字测试\n日本語テキスト\r\n한국어\n".to_string(),
            "\u{FEFF}a\u{1F600}b\n\u{1F600}\r\nx".to_string(),
            "a\r\nb😊c\n".to_string(),
            "no trailing newline".to_string(),
            "\r\n\r\n".to_string(),
        ];
        for source in sources {
            let table = line_table(&source);
            let index = LineIndex::new(&source);
            for (line_no, &start) in table.starts.iter().enumerate() {
                let content = table.content_lens[line_no];
                for column in 1..=content + 1 {
                    let scalar = start + column - 1;
                    let (engine_line, engine_col) = index.line_col_utf16(&source, scalar);
                    assert_eq!(
                        (engine_line, engine_col),
                        (
                            u32::try_from(line_no + 1).expect("table rows fit u32"),
                            column,
                        ),
                        "engine index disagrees with the walk at scalar {scalar} of {source:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn apply_edit_replaces_inserts_and_deletes() {
        let text = "abc\ndef\n";
        // Replace `cd`... positions: line 1 col 2 length 1 replaces `b`.
        assert_eq!(
            apply_edit(text, &edit("f", 1, 2, 1, "X")).expect("replace"),
            "aXc\ndef\n"
        );
        // Insert at end of line 1 (col 4 = one past `c`).
        assert_eq!(
            apply_edit(text, &edit("f", 1, 4, 0, "Z")).expect("insert"),
            "abcZ\ndef\n"
        );
        // Delete `de` (line 2, col 1, length 2).
        assert_eq!(
            apply_edit(text, &edit("f", 2, 1, 2, "")).expect("delete"),
            "abc\nf\n"
        );
    }

    #[test]
    fn apply_edit_spans_terminators_and_wide_chars() {
        // \r\n terminator: line 2 starts after both bytes (scalar math).
        let text = "a\r\nb😊c\n";
        // 😊 is 2 UTF-16 units: col 2 length 2 on line 2 replaces it.
        assert_eq!(
            apply_edit(text, &edit("f", 2, 2, 2, "X")).expect("wide char"),
            "a\r\nbXc\n"
        );
        // Line 2 content is `b😊c` = 4 units: length 4 covers exactly the
        // content and stops before the `\n`.
        assert_eq!(
            apply_edit(text, &edit("f", 2, 1, 4, "Q")).expect("content span"),
            "a\r\nQ\n"
        );
        // Length 5 actually crosses the `\n` terminator: span [3, 8) eats
        // it, so the result loses its trailing newline entirely.
        assert_eq!(
            apply_edit(text, &edit("f", 2, 1, 5, "Q")).expect("cross terminator"),
            "a\r\nQ"
        );
        // An odd length over an astral char (col 2 = 😊 start, length 3 =
        // 😊 + `c`): lengths count UTF-16 units, so the splice removes 😊
        // and `c` exactly — every byte offset stays on a char boundary.
        assert_eq!(
            apply_edit(text, &edit("f", 2, 2, 3, "Q")).expect("odd span over astral"),
            "a\r\nbQ\n"
        );
    }

    #[test]
    fn apply_edit_rejects_out_of_range_coordinates() {
        let text = "ab\n";
        assert!(
            apply_edit(text, &edit("f", 0, 1, 0, "x")).is_err(),
            "line 0"
        );
        assert!(apply_edit(text, &edit("f", 1, 0, 0, "x")).is_err(), "col 0");
        assert!(
            apply_edit(text, &edit("f", 5, 1, 0, "x")).is_err(),
            "line past EOF"
        );
        assert!(
            apply_edit(text, &edit("f", 1, 4, 0, "x")).is_err(),
            "column past line content (content is `ab`, one-past-end is col 3)"
        );
        assert!(
            apply_edit(text, &edit("f", 1, 1, 99, "x")).is_err(),
            "length past EOF"
        );
    }

    #[test]
    fn parse_edits_reads_jsonl_and_validates_paths() {
        let edits = parse_edits(
            "{\"file\":\"lib/a.dart\",\"line\":3,\"column\":8,\"length\":2,\"new_text\":\"x\"}\n\n",
        )
        .expect("valid edits");
        assert_eq!(
            edits,
            vec![EditStep::Splice(edit("lib/a.dart", 3, 8, 2, "x"))],
            "blank lines skipped, fields mapped"
        );
        for bad in [
            "{\"file\":\"../esc.dart\",\"line\":1,\"column\":1,\"length\":0,\"new_text\":\"\"}",
            "{\"file\":\"/abs.dart\",\"line\":1,\"column\":1,\"length\":0,\"new_text\":\"\"}",
            "{\"file\":\"a\\\\b.dart\",\"line\":1,\"column\":1,\"length\":0,\"new_text\":\"\"}",
            "{\"file\":\"a.dart\",\"line\":0,\"column\":1,\"length\":0,\"new_text\":\"\"}",
            "{\"file\":\"a.dart\",\"line\":1,\"column\":1,\"length\":0}",
            "{not json}",
        ] {
            assert!(parse_edits(bad).is_err(), "must reject: {bad}");
        }
        assert!(parse_edits("").expect("empty file").is_empty());
    }

    #[test]
    fn parse_edits_reads_remove_and_create_steps() {
        let edits = parse_edits(
            "{\"remove\":\"lib/old.dart\"}\n\n{\"create\":\"lib/new.dart\",\"text\":\"int x = 1;\\n\"}\n",
        )
.expect("valid file-set steps");
        assert_eq!(
            edits,
            vec![
                EditStep::Remove {
                    file: "lib/old.dart".into(),
                },
                EditStep::Create {
                    file: "lib/new.dart".into(),
                    text: "int x = 1;\n".into(),
                },
            ],
            "both file-set forms map to their variants"
        );
        // An empty created file is legitimate; the path rules apply to the
        // new forms with the same rigor as to splices.
        let empty = parse_edits("{\"create\":\"b.dart\",\"text\":\"\"}").expect("empty file");
        assert_eq!(
            empty,
            vec![EditStep::Create {
                file: "b.dart".into(),
                text: String::new(),
            }],
        );
        for bad in [
            "{\"remove\":5}",
            "{\"remove\":\"../esc.dart\"}",
            "{\"remove\":\"a.dart\",\"line\":1}",
            "{\"remove\":\"a.dart\",\"text\":\"stray\"}",
            "{\"create\":\"a.dart\"}",
            "{\"create\":\"a.dart\",\"text\":7}",
            "{\"create\":\"/abs.dart\",\"text\":\"\"}",
            "{\"create\":\"a.dart\",\"text\":\"\",\"remove\":\"b.dart\"}",
            "{\"remove\":\"a.dart\",\"create\":\"b.dart\",\"text\":\"\"}",
        ] {
            assert!(parse_edits(bad).is_err(), "must reject: {bad}");
        }
    }

    fn machine_diag(
        code: &str,
        file: &str,
        line: u32,
        col: u32,
        length: u32,
        message: &str,
    ) -> MachineDiag {
        MachineDiag {
            severity: Severity::Error,
            dtype: "SYNTACTIC_ERROR".into(),
            code: code.into(),
            file: std::path::PathBuf::from(file),
            line,
            col,
            length,
            message: message.into(),
        }
    }

    /// Both faces describing the same diagnostic must normalize to the
    /// same canonical bytes — the comparison's core pin.
    #[test]
    fn machine_and_row_faces_normalize_to_one_canonical_form() {
        let source = "int a = 1;\nint b = 2;\n";
        let machine = canonical_from_machine(
            &machine_diag("EXPECTED_TOKEN", "/c/lib/a.dart", 2, 5, 3, "Expected ';'."),
            source,
        )
        .expect("machine canonical");
        let row = r#"{"code":"expected_token","sev":"error","file":"file:///c/lib/a.dart","line":2,"col":5,"len":3,"msg":"Expected ';'."}"#;
        let row_value = serde_json::from_str::<Value>(row).expect("json");
        assert_eq!(
            row_path(&row_value).expect("row path"),
            PathBuf::from("/c/lib/a.dart")
        );
        let row_canonical = canonical_from_row(&row_value, source).expect("row canonical");
        assert_eq!(machine.key(), row_canonical.key(), "keys must be equal");
        // End position came from the engine index: line 2 col 5 + 3 units
        // of `b =` → end lands at col 8 (one-based), i.e. 0-based 7.
        assert_eq!(machine.end_line, 1);
        assert_eq!(machine.end_col, 7);
    }

    /// Real drift must stay visible: a moved end, a different severity, or
    /// a different message each break equality.
    #[test]
    fn canonical_form_detects_end_severity_and_message_drift() {
        let source = "int a = 1;\nint b = 2;\n";
        let base = canonical_from_machine(
            &machine_diag("EXPECTED_TOKEN", "/c/a.dart", 2, 5, 3, "Expected ';'."),
            source,
        )
        .expect("canonical");
        let row_for = |len: u64, sev: &str, message: &str| {
            let json = format!(
                r#"{{"code":"expected_token","sev":"{sev}","file":"file:///c/a.dart","line":2,"col":5,"len":{len},"msg":"{message}"}}"#
            );
            canonical_from_row(&serde_json::from_str(&json).expect("json"), source)
                .expect("canonical")
        };
        assert_ne!(
            base.key(),
            row_for(4, "error", "Expected ';'.").key(),
            "end drift (length)"
        );
        assert_ne!(
            base.key(),
            row_for(3, "warning", "Expected ';'.").key(),
            "severity drift"
        );
        assert_ne!(base.key(), row_for(3, "error", "Other.").key(), "message drift");
        assert_eq!(
            base.key(),
            row_for(3, "error", "Expected ';'.").key(),
            "equal faces"
        );
    }

    #[test]
    fn multi_line_span_end_comes_from_the_engine_index() {
        // A diagnostic whose length crosses the `\n`: end position is on
        // the next line, computed by the engine index (not scalar math).
        let source = "ab\ncd\n";
        let diag = canonical_from_machine(&machine_diag("X", "/c/a.dart", 1, 2, 3, "m"), source)
            .expect("canonical");
        assert_eq!((diag.start_line, diag.start_col), (0, 1));
        assert_eq!(
            (diag.end_line, diag.end_col),
            (1, 1),
            "`b\\nc` ends on line 2 col 1 zero-based"
        );
    }

    #[test]
    fn file_uris_decode_and_reject_other_schemes() {
        assert_eq!(
            file_uri_to_path("file:///tmp/x%20y/a.dart").expect("decoded"),
            PathBuf::from("/tmp/x y/a.dart")
        );
        assert_eq!(
            file_uri_to_path("file:///plain.dart").expect("plain"),
            PathBuf::from("/plain.dart")
        );
        assert!(file_uri_to_path("http://x/a.dart").is_err());
        assert!(file_uri_to_path("file:///trunc%.dart").is_err());
    }

    /// The machine parser the batch side reuses must accept the canonical
    /// output shape this module normalizes (guards an accidental drift in
    /// the shared `crate::machine` dependency).
    #[test]
    fn machine_side_parses_through_the_shared_parser() {
        let line = "ERROR|SYNTACTIC_ERROR|EXPECTED_TOKEN|/c/lib/a.dart|2|5|3|Expected to find ';'.";
        let diags = crate::machine::parse_output(line, OutputFormat::Machine).expect("parses");
        assert_eq!(diags.len(), 1);
        let canonical =
            canonical_from_machine(&diags[0], "int a = 1;\nint b = 2;\n").expect("canonical");
        assert_eq!(canonical.code, "expected_token");
    }
}
