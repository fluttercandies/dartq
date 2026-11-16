//! The cross-process renderers: the one writer per output format that
//! BOTH the CLI's cold path and the daemon's routed faces (the
//! `dartq/*` requests carrying a `format` parameter, #2627/#2629) render
//! through — one renderer per format, by relocation rather than
//! duplication, so the routed bytes are the cold bytes structurally.
//!
//! * [`lon_line`]/[`write_lon`] — one canonical LON line of a document
//!   value (the `lon` crate's encoder; the analyze document itself comes
//!   from the shared [`crate::analyze::diags_document`] builder, #2657);
//! * [`write_human`] (+[`text_diag_row`]) — the analyze/check `text`
//!   face: the `root:` line, per-file groups, the count tail;
//! * [`write_json_value`]/[`write_text_value`] — the generic value
//!   faces: the single-line JSON spelling and the indented text spelling
//!   every answer-valued face (query, explain, impact, rename, resolve,
//!   completion) prints through on both paths (#2629);
//! * the `json` face's oracle envelope writer stays in `dartq-diagnostics`
//!   (`write_json`) — it was already the one implementation.
//!
//! The daemon renders into a buffer and returns the bytes over the
//! session wire; the one-shot CLI relays them to stdout verbatim. The
//! severity-exit tier travels beside the bytes (`counts`), never inside
//! them.

#![allow(clippy::semicolon_if_nothing_returned)]
use std::collections::BTreeMap;
use std::io::{self, Write};
use std::path::Path;

use serde_json::Value;

use dartq_analyzer_host::face::{Face, FaceFormat, FaceMeta, FaceRender};
use dartq_analyzer_host::{Row, report};
use dartq_diagnostics::{AnalysisOutput, Diagnostic, FileLayoutProvider};

use crate::analyze::severity_word;

/// One LON line of `value`.
///
/// # Panics
///
/// The value graph does not encode as LON — non-finite floats and
/// codec-limit overflows are program bugs, not runtime cases.
#[must_use]
pub fn lon_line(value: &Value) -> String {
    // The values are serde_json graphs: no non-finite floats exist in the
    // model, and every producer's depth sits far under the codec's limits
    // — an error here is a program bug, not a runtime case.
    lon::to_string(value).expect("the value graph encodes as LON")
}

/// The `lon` face: one canonical line.
pub fn write_lon(out: &mut impl Write, value: &Value) -> io::Result<()> {
    writeln!(out, "{}", lon_line(value))
}

/// The analyze/check `lon` document streamed row-by-row — the byte-identical
/// twin of `write_lon(out, &diags_document(output))` that never materializes
/// the `serde_json::Value` graph. `lon::to_string` expands the whole document
/// into one node tree and counts every element against its 1,000,000-element
/// budget; a whole-repo diagnostic set crosses that budget (hundreds of
/// thousands of rows × seven fields) and the codec's `expect` guard panics —
/// the rc=4 crash the zero-panic red line forbids. This writer emits the same
/// `{v:1 root? diags:[…]}` bytes one row at a time through `out`, so neither a
/// value graph nor a whole-output buffer exists and the bytes stay identical
/// at every size.
///
/// The row grammar is the encoder's own record-table form (the shape
/// [`write_lon_face`] already pins for the analyzer-host `Row` face): the
/// `diags` array is `[code sev file line col len msg;cell …]` at ≥2 rows and
/// the single object at 1, because every row shares one schema — the codec's
/// `should_use_table` gate always factors a ≥2-row uniform schema (seven
/// fields ≥ 3). The scalar spellings reuse the ported codec rules below
/// ([`lon_string`]/[`lon_number`]); each row renders into a reused scratch
/// buffer flushed to `out`, so peak memory is one row, not the document.
pub fn write_diags_lon(out: &mut impl Write, output: &AnalysisOutput<'_>) -> io::Result<()> {
    let root = output.files.analysis_root();
    let diags = output.diagnostics;
    // One scratch row buffer, cleared per row — the scalar helpers spell
    // into a `Vec<u8>`, so a row's cells/members land here before the flush.
    let mut scratch = Vec::new();
    out.write_all(b"{v:1 ")?;
    if let Some(root) = root.filter(|_| !diags.is_empty()) {
        scratch.clear();
        lon_string(&mut scratch, &root.display().to_string());
        out.write_all(b"root:")?;
        out.write_all(&scratch)?;
        out.write_all(b" ")?;
    }
    match diags.len() {
        0 => out.write_all(b"diags:[]}")?,
        1 => {
            out.write_all(b"diags:[{")?;
            scratch.clear();
            lon_diag_members(&mut scratch, &diags[0], output, root);
            out.write_all(&scratch)?;
            out.write_all(b"}]}")?;
        }
        _ => {
            out.write_all(b"diags:[code sev file line col len msg")?;
            for diag in diags {
                scratch.clear();
                lon_diag_cells(&mut scratch, diag, output, root);
                out.write_all(b";")?;
                out.write_all(&scratch)?;
            }
            out.write_all(b"]}")?;
        }
    }
    // `write_lon`'s `writeln!` — the line is the face payload's own.
    out.write_all(b"\n")?;
    out.flush()
}

/// One `diags` object's member list in canonical order (object form):
/// `key:value` pairs space-separated — `diag_row`'s field order.
fn lon_diag_members(
    out: &mut Vec<u8>,
    diag: &Diagnostic,
    output: &AnalysisOutput<'_>,
    root: Option<&Path>,
) {
    out.extend_from_slice(b"code:");
    lon_string(out, diag.code.name);
    out.extend_from_slice(b" sev:");
    out.extend_from_slice(severity_word(diag.severity).as_bytes());
    out.extend_from_slice(b" file:");
    lon_string(out, &diag_file_spelling(diag, output, root));
    let (line, col) = diag_line_col(diag, output);
    out.extend_from_slice(b" line:");
    lon_number(out, u64::from(line));
    out.extend_from_slice(b" col:");
    lon_number(out, u64::from(col));
    out.extend_from_slice(b" len:");
    lon_number(out, u64::from(diag.range.length));
    out.extend_from_slice(b" msg:");
    lon_string(out, diag.message.as_str());
}

/// One record-table row's cell values (space-separated, the same order as
/// [`lon_diag_members`]).
fn lon_diag_cells(
    out: &mut Vec<u8>,
    diag: &Diagnostic,
    output: &AnalysisOutput<'_>,
    root: Option<&Path>,
) {
    lon_string(out, diag.code.name);
    out.push(b' ');
    out.extend_from_slice(severity_word(diag.severity).as_bytes());
    out.push(b' ');
    lon_string(out, &diag_file_spelling(diag, output, root));
    out.push(b' ');
    let (line, col) = diag_line_col(diag, output);
    lon_number(out, u64::from(line));
    out.push(b' ');
    lon_number(out, u64::from(col));
    out.push(b' ');
    lon_number(out, u64::from(diag.range.length));
    out.push(b' ');
    lon_string(out, diag.message.as_str());
}

/// The `file` column's spelling through the one locator — `diag_row`'s
/// `file`: relative inside `root`, absolute outside.
fn diag_file_spelling(
    diag: &Diagnostic,
    output: &AnalysisOutput<'_>,
    root: Option<&Path>,
) -> String {
    let path = output.files.file_path(diag.file);
    match root {
        Some(root) => crate::locate(root, path),
        None => path.display().to_string(),
    }
}

/// `diag_row`'s `(line, col)` — the provider's 1-based position of the
/// diagnostic's UTF-16 offset.
fn diag_line_col(diag: &Diagnostic, output: &AnalysisOutput<'_>) -> (u32, u32) {
    output.files.line_col(diag.file, diag.range.offset)
}

/// The analyze `text` face's row spelling: `sev code line:col msg` under
/// the file heading, `sev` lowercase like every non-oracle face (the
/// grouped human renderer in `write_human` owns headings and the count
/// tail; this is the row).
pub fn text_diag_row(diag: &Diagnostic, layout: &dyn FileLayoutProvider) -> String {
    let (line, col) = layout.line_col(diag.file, diag.range.offset);
    format!(
        "{} {} {}:{} {}",
        severity_word(diag.severity),
        diag.code.name,
        line,
        col,
        diag.message
    )
}

/// Human format: one `root: <absolute>` line when rows exist and the run
/// has one analysis root (the lon face's own gate — the base the located
/// headings spell against), diagnostics grouped by file (canonical path
/// order; headings spelled through the locator — root-relative inside,
/// absolute outside), the row grammar `sev code line:col msg` under each
/// group, the count tail (the oracle's spelling). Not a differential
/// face — layout may change freely.
pub fn write_human(
    out: &mut impl Write,
    diags: &[Diagnostic],
    layout: &dyn FileLayoutProvider,
) -> io::Result<()> {
    let root = layout.analysis_root();
    let mut groups: BTreeMap<&Path, Vec<&Diagnostic>> = BTreeMap::new();
    for diag in diags {
        groups
            .entry(layout.file_path(diag.file))
            .or_default()
            .push(diag);
    }
    if let Some(root) = root.filter(|_| !groups.is_empty()) {
        writeln!(out, "root: {}", root.display())?;
    }
    for (path, group) in &groups {
        let heading = match root {
            Some(root) => crate::locate(root, path),
            None => path.display().to_string(),
        };
        writeln!(out, "{heading}")?;
        for diag in group {
            writeln!(out, "  {}", text_diag_row(diag, layout))?;
        }
    }
    match diags.len() {
        // The oracle's clean-set tail carries the exclamation mark
        // (`dart analyze`, 3.13.1: `No issues found!`); the counted tails
        // keep the period (`N issues found.`).
        0 => writeln!(out, "No issues found!"),
        1 => writeln!(out, "1 issue found."),
        n => writeln!(out, "{n} issues found."),
    }
}

/// The generic `json` face over one answer value: the value's single-line
/// JSON spelling plus the newline. The one writer the answer-valued faces
/// (query, explain, impact, rename, resolve, completion) print through on
/// both the cold CLI path and the daemon's routed arm (#2629) — the
/// `Display` of a `Value` is its `to_string`, so every face's bespoke
/// "write the value plus a newline" reduces to this.
pub fn write_json_value(out: &mut impl Write, value: &Value) -> io::Result<()> {
    writeln!(out, "{value}")
}

/// The generic `text` face over one answer value: the indented pretty
/// spelling plus the newline (the read verbs without a bespoke human
/// renderer — the same face the CLI's `--format text` prints).
pub fn write_text_value(out: &mut impl Write, value: &Value) -> io::Result<()> {
    writeln!(
        out,
        "{}",
        serde_json::to_string_pretty(value).expect("the value pretty-prints")
    )
}

// ---------------------------------------------------------------------------
// The official analyze face writer — the one render the cold in-process path
// and the analyzer-host daemon share (#2627: no second renderer exists). The
// daemon injects this function as its `FaceRenderer`; the CLI calls it over
// the per-run rows for every other path. Rows arrive in dartdev order and the
// bytes are identical either way.
// ---------------------------------------------------------------------------

/// One modelable row's face facts (the diagnostic shape minus the model —
/// positions come straight from the row's reported line/column, so a render
/// never opens the source).
struct FaceRow<'a> {
    row: &'a Row,
    /// The `sev` spelling (`error`/`warning`/`info`, lowercase — every
    /// non-oracle face's own severity vocabulary).
    sev: &'static str,
    /// The `(file, offset)` position's reported line and column.
    line: u32,
    col: u32,
}

/// Splits `rows` (the report's kept rows, dartdev order) into the modelable
/// face rows and the unmodelable lines the client prints as advisories, and
/// counts the severities the exit ladder and note gate read. This is
/// `OfficialRun::of_rows`' classification without the diagnostic model —
/// one pass, the three faces' shared step.
fn face_rows<'a>(rows: &[&'a Row]) -> (Vec<FaceRow<'a>>, FaceMeta) {
    let mut meta = FaceMeta::default();
    let mut kept = Vec::with_capacity(rows.len());
    for &row in rows {
        meta.has_error |= row.severity == "ERROR";
        meta.has_warning |= row.severity == "WARNING";
        meta.has_info |= row.severity == "INFO";
        let Some(sev) = modelable_severity(&row.severity) else {
            meta.unparsed
                .push(serde_json::to_string(row).expect("a row serializes"));
            continue;
        };
        if modelable_type(&row.kind).is_none() {
            meta.unparsed
                .push(serde_json::to_string(row).expect("a row serializes"));
            continue;
        }
        meta.has_diag_error |= row.severity == "ERROR";
        kept.push(FaceRow {
            row,
            sev,
            line: row.location.start_line.unwrap_or(0),
            col: row.location.start_column.unwrap_or(0),
        });
    }
    (kept, meta)
}

/// The `sev` spelling for a modelable severity — the diagnostic model's
/// own vocabulary (`Layout::diagnostic`'s severity gate).
fn modelable_severity(severity: &str) -> Option<&'static str> {
    match severity {
        "ERROR" => Some("error"),
        "WARNING" => Some("warning"),
        "INFO" => Some("info"),
        _ => None,
    }
}

/// The `type` family gate (`Layout::diagnostic`'s `diagnostic_type`):
/// the plugin protocol's two extra spellings fold onto their family here
/// only — the json face prints the row's own type regardless.
fn modelable_type(kind: &str) -> Option<()> {
    match kind {
        "SYNTACTIC_ERROR"
        | "COMPILE_TIME_ERROR"
        | "CHECKED_MODE_COMPILE_TIME_ERROR"
        | "STATIC_WARNING"
        | "STATIC_TYPE_WARNING"
        | "LINT"
        | "HINT"
        | "TODO" => Some(()),
        _ => None,
    }
}

/// The official analyze faces' one render: `lon`'s canonical document,
/// `json`'s oracle envelope, `text`'s grouped human lines — whichever
/// `face.format` names. The stdout payload in `bytes` plus the counts the
/// caller finishes the run with in `meta` (`priority_errors` is the
/// daemon's splice — it never enters the row set this function sees).
///
/// This is the daemon's `FaceRenderer` and the cold path's writer at
/// once: same rows in, same bytes out, so the resident hit and the
/// per-run render are byte-identical by construction.
#[must_use]
pub fn render_official_face(rows: &[&Row], face: &Face) -> FaceRender {
    let (kept, meta) = face_rows(rows);
    // Pre-size the payload buffer: the writer appends every row's fields
    // plus syntax, so the kept rows' own text sizes bound the render's
    // scale — a from-empty Vec would pay ~2x its final size across
    // doublings on a 200 MB-class whole-repo face. The estimate errs on
    // the json face's side (its envelope re-encodes fields); an
    // undershoot costs one grow, never a wrong byte.
    let hint = 256
        + kept.iter().fold(0usize, |acc, diag| {
            acc + diag.row.message.len()
                + diag.row.code.len()
                + diag.row.location.file.len()
                + diag.row.location.length as usize
                + 160
        });
    let mut bytes = Vec::with_capacity(hint);
    match face.format {
        FaceFormat::Lon => write_lon_face(&mut bytes, &kept, face.root.as_deref()),
        FaceFormat::Json => write_json_face(&mut bytes, &kept),
        FaceFormat::Text => write_text_face(&mut bytes, &kept, face.root.as_deref()),
    }
    FaceRender { meta, bytes }
}

/// `diags_document` + `write_lon`'s exact shape, written directly: the
/// `{v:1 root? diags:[…]}` object, the `diags` rows in the lon encoder's
/// record-table form at ≥2 rows and object form at 1. One streaming pass —
/// no `serde_json::Value` DOM, no per-row allocation beyond the located
/// `file` string the row itself needs.
fn write_lon_face(out: &mut Vec<u8>, diags: &[FaceRow<'_>], root: Option<&Path>) {
    out.extend_from_slice(b"{v:1 ");
    if let Some(root) = root.filter(|_| !diags.is_empty()) {
        out.extend_from_slice(b"root:");
        lon_string(out, &root.display().to_string());
        out.push(b' ');
    }
    match diags.len() {
        0 => out.extend_from_slice(b"diags:[]}"),
        1 => {
            out.extend_from_slice(b"diags:[{");
            lon_members(out, &diags[0], root);
            out.extend_from_slice(b"}]}");
        }
        _ => {
            out.extend_from_slice(b"diags:[code sev file line col len msg");
            for diag in diags {
                out.push(b';');
                lon_cells(out, diag, root);
            }
            out.extend_from_slice(b"]}");
        }
    }
    // `write_lon`'s `writeln!` — the line is the face payload's own.
    out.push(b'\n');
}

/// One `diags` object's member list in canonical order (object form):
/// `key:value` pairs space-separated.
fn lon_members(out: &mut Vec<u8>, diag: &FaceRow<'_>, root: Option<&Path>) {
    out.extend_from_slice(b"code:");
    lon_string(out, &diag.row.code);
    out.extend_from_slice(b" sev:");
    out.extend_from_slice(diag.sev.as_bytes());
    out.extend_from_slice(b" file:");
    lon_string(out, &file_spelling(diag.row, root));
    out.extend_from_slice(b" line:");
    lon_number(out, u64::from(diag.line));
    out.extend_from_slice(b" col:");
    lon_number(out, u64::from(diag.col));
    out.extend_from_slice(b" len:");
    lon_number(out, u64::from(diag.row.location.length));
    out.extend_from_slice(b" msg:");
    lon_string(out, &diag.row.message);
}

/// One record-table row's cell values (space-separated, the same order as
/// [`lon_members`]).
fn lon_cells(out: &mut Vec<u8>, diag: &FaceRow<'_>, root: Option<&Path>) {
    lon_string(out, &diag.row.code);
    out.push(b' ');
    out.extend_from_slice(diag.sev.as_bytes());
    out.push(b' ');
    lon_string(out, &file_spelling(diag.row, root));
    out.push(b' ');
    lon_number(out, u64::from(diag.line));
    out.push(b' ');
    lon_number(out, u64::from(diag.col));
    out.push(b' ');
    lon_number(out, u64::from(diag.row.location.length));
    out.push(b' ');
    lon_string(out, &diag.row.message);
}

/// The `file` column's spelling through the one locator: relative inside
/// `root`, absolute outside — `diags_document`'s `diag_row` file.
fn file_spelling(row: &Row, root: Option<&Path>) -> String {
    let path = Path::new(&*row.location.file);
    match root {
        Some(root) => crate::locate(root, path),
        None => path.to_string_lossy().into_owned(),
    }
}

/// A number's LON lexeme — the serde `u64::to_string` lexeme verbatim.
fn lon_number(out: &mut Vec<u8>, value: u64) {
    out.extend_from_slice(value.to_string().as_bytes());
}

// ---------------------------------------------------------------------------
// The LON scalar spelling (ported spec-level from the `lon` codec — bare
// eligibility, reserved tokens, canonical JSON escapes; `write_json_string`
// and `bare_eligible`/`is_reserved` are the codec's own rules over a `&str`,
// which never carries the lone surrogates the codec's code-unit API admits).
// ---------------------------------------------------------------------------

/// One string's LON spelling: bare when it can be (not empty, no
/// delimiter/quote/backslash/visibility-sensitive code point, no edge `'`,
/// and not a reserved token), else the canonical JSON-escaped string.
fn lon_string(out: &mut Vec<u8>, text: &str) {
    if bare_eligible(text) && !is_reserved(text) {
        out.extend_from_slice(text.as_bytes());
    } else {
        json_escaped(out, text);
    }
}

/// `bare_eligible_token`/`bare_eligible_units` over a `&str`: the string
/// can stay unquoted when no char is a C0 control, DEL, `"`, `\`, a LON
/// token delimiter (whitespace or `( ) , : ; [ ] { | }`), a
/// visibility-escaped code point, or a `'` at either edge.
fn bare_eligible(text: &str) -> bool {
    if text.is_empty() {
        return false;
    }
    let chars = text.char_indices();
    let last = text.len();
    for (index, c) in chars {
        let code = c as u32;
        let at_edge = index == 0 || index + c.len_utf8() == last;
        if code < 0x80 {
            if code < 0x20
                || code == 0x7F
                || code == 0x22
                || code == 0x5C
                || matches!(
                    code,
                    0x20 | 0x09
                        ..=0x0D
                            | 0x28
                            | 0x29
                            | 0x2C
                            | 0x3A
                            | 0x3B
                            | 0x5B
                            | 0x5D
                            | 0x7B
                            | 0x7C
                            | 0x7D
                )
                || (code == 0x27 && at_edge)
            {
                return false;
            }
            continue;
        }
        if is_bare_forbidden(code) || requires_visibility_escape(code) {
            return false;
        }
    }
    true
}

/// The bare-token delimiter set for non-ASCII code points: the LON
/// whitespace plus `( ) , : ; [ ] { | }`, or `"`/`\`.
fn is_bare_forbidden(code: u32) -> bool {
    code == 0x22
        || code == 0x5C
        || matches!(
            code,
            0x09..=0x0D
                | 0x20
                | 0x85
                | 0xA0
                | 0x1680
                | 0x2000..=0x200A
                | 0x2028
                | 0x2029
                | 0x202F
                | 0x205F
                | 0x3000
                | 0x28
                | 0x29
                | 0x2C
                | 0x3A
                | 0x3B
                | 0x5B
                | 0x5D
                | 0x7B
                | 0x7C
                | 0x7D
        )
}

/// `requires_visibility_escape`: code points that must appear as
/// `\uXXXX` inside any quoted string — controls, invisible separators,
/// directional marks, noncharacters.
fn requires_visibility_escape(rune: u32) -> bool {
    rune < 0x20
        || (0x7F..=0x9F).contains(&rune)
        || (0xFDD0..=0xFDEF).contains(&rune)
        || (rune & 0xFFFE) == 0xFFFE
        || matches!(
            rune,
            0x00AD
                | 0x034F
                | 0x061C
                | 0x180E
                | 0x200B
                | 0x200E
                | 0x200F
                | 0xFEFF
                | 0x2028..=0x202E
                | 0x2060..=0x206F
                | 0xFFF9..=0xFFFB
                | 0x13430..=0x13455
                | 0x1BCA0..=0x1BCA3
                | 0x1D173..=0x1D17A
                | 0xE0000..=0xE007F
        )
}

/// `is_reserved_lon_token` over a `&str`: tokens a reader could confuse
/// with a literal, a number, or a non-finite spelling stay quoted.
fn is_reserved(text: &str) -> bool {
    let bytes = text.as_bytes();
    if text.is_ascii() {
        return is_reserved_ascii(bytes);
    }
    let folded = text.to_lowercase();
    AMBIGUOUS_LITERALS.contains(&folded.as_str())
        || is_non_finite(&folded)
        || is_numeric_like(bytes)
}

fn is_reserved_ascii(bytes: &[u8]) -> bool {
    const LONGEST_LITERAL: usize = 9; // "undefined"
    const LONGEST_NON_FINITE: usize = 10; // "+.infinity"
    if bytes.len() <= LONGEST_LITERAL {
        for literal in AMBIGUOUS_LITERALS {
            if bytes.eq_ignore_ascii_case(literal.as_bytes()) {
                return true;
            }
        }
    }
    if bytes.len() <= LONGEST_NON_FINITE && is_non_finite_ascii(bytes) {
        return true;
    }
    is_numeric_like(bytes)
}

const AMBIGUOUS_LITERALS: [&str; 13] = [
    "null",
    "true",
    "false",
    "undefined",
    "none",
    "nil",
    "yes",
    "no",
    "on",
    "off",
    "y",
    "n",
    "~",
];

fn is_non_ascii_nan_inf(body: &[u8]) -> bool {
    body.eq_ignore_ascii_case(b"nan")
        || body.eq_ignore_ascii_case(b"inf")
        || body.eq_ignore_ascii_case(b"infinity")
}

fn is_non_finite(folded: &str) -> bool {
    let unsigned = folded
        .strip_prefix('+')
        .or_else(|| folded.strip_prefix('-'))
        .unwrap_or(folded);
    let bare = unsigned.strip_prefix('.').unwrap_or(unsigned);
    matches!(bare, "nan" | "inf" | "infinity")
}

fn is_non_finite_ascii(bytes: &[u8]) -> bool {
    let mut body = bytes;
    if matches!(body.first(), Some(b'+' | b'-')) {
        body = &body[1..];
    }
    if body.first() == Some(&b'.') {
        body = &body[1..];
    }
    is_non_ascii_nan_inf(body)
}

/// `is_numeric_like`: decimal/underscore-fraction/radix spellings a reader
/// could parse as a number rather than a string.
fn is_numeric_like(bytes: &[u8]) -> bool {
    let body = match bytes.first() {
        Some(b'+' | b'-') => &bytes[1..],
        _ => bytes,
    };
    decimal_like(body).is_some_and(|end| end == body.len())
        || is_underscore_fraction(body)
        || is_radix_like(body)
}

fn decimal_like(body: &[u8]) -> Option<usize> {
    let mut i = digit_run(body, 0)?;
    while body.get(i) == Some(&b'_') {
        i = digit_run(body, i + 1)?;
    }
    if body.get(i) == Some(&b'.') {
        i = digit_or_underscore_run(body, i + 1);
    }
    if matches!(body.get(i), Some(&b'e' | &b'E')) {
        i += 1;
        if matches!(body.get(i), Some(&b'+' | &b'-')) {
            i += 1;
        }
        i = digit_or_underscore_run(body, i);
    }
    Some(i)
}

fn is_underscore_fraction(body: &[u8]) -> bool {
    body.first() == Some(&b'.')
        && body.len() > 1
        && body[1..].iter().all(|&d| d.is_ascii_digit() || d == b'_')
}

fn is_radix_like(body: &[u8]) -> bool {
    if body.len() < 3 || body[0] != b'0' {
        return false;
    }
    let rest = &body[2..];
    match body[1] {
        b'x' | b'X' => rest.iter().all(|&d| d.is_ascii_hexdigit() || d == b'_'),
        b'b' | b'B' => rest.iter().all(|&d| matches!(d, b'0' | b'1' | b'_')),
        b'o' | b'O' => rest.iter().all(|&d| matches!(d, b'0'..=b'7' | b'_')),
        _ => false,
    }
}

fn digit_run(bytes: &[u8], start: usize) -> Option<usize> {
    let mut i = start;
    while matches!(bytes.get(i), Some(&d) if d.is_ascii_digit()) {
        i += 1;
    }
    (i > start).then_some(i)
}

fn digit_or_underscore_run(bytes: &[u8], mut i: usize) -> usize {
    while matches!(bytes.get(i), Some(d) if d.is_ascii_digit() || *d == b'_') {
        i += 1;
    }
    i
}

/// `write_json_string` over a `&str`: the short escapes, `\uXXXX` for
/// visibility-sensitive code points (a non-BMP point escapes as its
/// surrogate pair), raw characters everywhere else.
fn json_escaped(out: &mut Vec<u8>, text: &str) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let escape_u16 = |out: &mut Vec<u8>, unit: u32| {
        out.extend_from_slice(b"\\u");
        for shift in [12, 8, 4, 0] {
            out.push(HEX[((unit >> shift) & 0xF) as usize]);
        }
    };
    out.push(b'"');
    for c in text.chars() {
        match c {
            '"' => out.extend_from_slice(b"\\\""),
            '\\' => out.extend_from_slice(b"\\\\"),
            '\u{08}' => out.extend_from_slice(b"\\b"),
            '\t' => out.extend_from_slice(b"\\t"),
            '\n' => out.extend_from_slice(b"\\n"),
            '\u{0C}' => out.extend_from_slice(b"\\f"),
            '\r' => out.extend_from_slice(b"\\r"),
            _ => {
                let rune = c as u32;
                if requires_visibility_escape(rune) {
                    if rune <= 0xFFFF {
                        escape_u16(out, rune);
                    } else {
                        let v = rune - 0x10000;
                        escape_u16(out, 0xD800 + (v >> 10));
                        escape_u16(out, 0xDC00 + (v & 0x3FF));
                    }
                } else {
                    let mut buf = [0u8; 4];
                    out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
                }
            }
        }
    }
    out.push(b'"');
}

// ---------------------------------------------------------------------------
// The `json` and `text` faces over the modelable rows.
// ---------------------------------------------------------------------------

/// `write_json`'s oracle envelope over the modelable rows (the kept set —
/// `of_rows` keeps only modelable rows in `run.rows`, so the daemon's
/// envelope and the per-run one carry the same diagnostics).
fn write_json_face(out: &mut Vec<u8>, diags: &[FaceRow<'_>]) {
    // `write_json` takes `&[Row]`; the render clones the kept rows once —
    // one clone per row, no Value DOM, no re-parse.
    let kept: Vec<Row> = diags.iter().map(|face| face.row.clone()).collect();
    report::write_json(out, &kept).expect("a face render never fails on a Vec");
}

/// `write_human`'s grouped text over the modelable rows: `root:` when a
/// root exists and rows remain, one located heading per file group in
/// path order, `  sev code line:col msg` rows, the oracle tail.
fn write_text_face(out: &mut Vec<u8>, diags: &[FaceRow<'_>], root: Option<&Path>) {
    // `write_human` groups by `BTreeMap<&Path>` — path-component order,
    // the order its headings print. Same key, same order.
    let mut groups: BTreeMap<&Path, Vec<&FaceRow<'_>>> = BTreeMap::new();
    for diag in diags {
        groups
            .entry(Path::new(&*diag.row.location.file))
            .or_default()
            .push(diag);
    }
    if let Some(root) = root.filter(|_| !groups.is_empty()) {
        out.extend_from_slice(b"root: ");
        out.extend_from_slice(root.display().to_string().as_bytes());
        out.push(b'\n');
    }
    for (path, group) in &groups {
        let heading = match root {
            Some(root) => crate::locate(root, path),
            None => path.to_string_lossy().into_owned(),
        };
        out.extend_from_slice(heading.as_bytes());
        out.push(b'\n');
        for diag in group {
            out.extend_from_slice(b"  ");
            out.extend_from_slice(diag.sev.as_bytes());
            out.push(b' ');
            out.extend_from_slice(diag.row.code.as_bytes());
            out.push(b' ');
            out.extend_from_slice(diag.line.to_string().as_bytes());
            out.push(b':');
            out.extend_from_slice(diag.col.to_string().as_bytes());
            out.push(b' ');
            out.extend_from_slice(diag.row.message.as_bytes());
            out.push(b'\n');
        }
    }
    match diags.len() {
        0 => out.extend_from_slice(b"No issues found!\n"),
        1 => out.extend_from_slice(b"1 issue found.\n"),
        n => out.extend_from_slice(n.to_string().as_bytes()),
    }
    if diags.len() > 1 {
        out.extend_from_slice(b" issues found.\n");
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use dartq_analyzer_host::Report;
    use dartq_analyzer_host::face::FaceFormat;
    use dartq_analyzer_host::report::{ContextMessage, Location};
    use dartq_diagnostics::{CodeInfo, DiagnosticType, FileId, Severity, SourceRange};
    use dartq_source::FileTable;

    use super::*;
    use crate::analyze::diags_document;
    use crate::analyze::official::OfficialRun;

    /// One protocol row; `correction`/`url`/`context` ride along so the
    /// json face's oracle envelope is pinned on every field, not only the
    /// ones the lon/text faces print.
    fn row(
        severity: &str,
        kind: &str,
        file: &str,
        offset: u32,
        line: u32,
        col: u32,
        len: u32,
        message: &str,
        code: &str,
    ) -> Row {
        Row {
            severity: severity.into(),
            kind: kind.into(),
            location: Location {
                file: file.into(),
                offset,
                length: len,
                start_line: Some(line),
                start_column: Some(col),
                end_line: Some(line),
                end_column: Some(col + len),
            },
            message: message.into(),
            correction: Some("fix it".into()),
            code: code.into(),
            url: Some("https://dart.dev/diagnostics/x".into()),
            context_messages: vec![ContextMessage {
                message: "ctx".into(),
                location: Location {
                    file: file.into(),
                    offset: offset + 50,
                    length: 2,
                    start_line: Some(9),
                    start_column: Some(1),
                    end_line: Some(9),
                    end_column: Some(3),
                },
            }],
        }
    }

    /// Rows exercising every string rule the faces have: reserved LON
    /// tokens, escapes, unicode (BMP + astral + visibility-escaped), an
    /// edge `'`, a delimiter-bearing message, a file outside the root.
    fn adversarial() -> Vec<Row> {
        vec![
            row(
                "ERROR",
                "COMPILE_TIME_ERROR",
                "/p/lib/a.dart",
                30,
                4,
                3,
                2,
                "boom \"quoted\" \\ slash",
                "null",
            ),
            row(
                "WARNING",
                "STATIC_WARNING",
                "/p/lib/a.dart",
                10,
                2,
                1,
                5,
                "careful [brackets] {braces}",
                "yes",
            ),
            row(
                "INFO",
                "LINT",
                "/p/lib/é.dart",
                1,
                1,
                1,
                3,
                "á unicode \u{2028}sep \u{1F600} emoji",
                "avoid_print",
            ),
            row(
                "ERROR",
                "COMPILE_TIME_ERROR",
                "/outside/x.dart",
                0,
                1,
                1,
                1,
                "'edge quote'",
                "dead_code",
            ),
        ]
    }

    /// The DOM path's expected bytes for one face over `files`' rows —
    /// `OfficialRun` is the single source the CLI printed from.
    fn dom_bytes(files: &[(&str, Vec<Row>)], root: Option<PathBuf>, format: FaceFormat) -> Vec<u8> {
        let report = Report::from_files(files.iter().map(|(f, r)| (*f, r.as_slice())));
        let run = OfficialRun::of_report(report, true, false, root);
        let output = run.output();
        let mut out = Vec::new();
        match format {
            FaceFormat::Lon => write_lon(&mut out, &diags_document(&output)).expect("write"),
            FaceFormat::Json => run.write_json(&mut out).expect("write"),
            FaceFormat::Text => {
                write_human(&mut out, output.diagnostics, output.files).expect("write");
            }
        }
        out
    }

    /// The streaming writer's bytes over the same report rows.
    fn face_bytes(
        files: &[(&str, Vec<Row>)],
        root: Option<PathBuf>,
        format: FaceFormat,
    ) -> Vec<u8> {
        let report = Report::from_files(files.iter().map(|(f, r)| (*f, r.as_slice())));
        let rows: Vec<&Row> = report.rows.iter().collect();
        render_official_face(&rows, &Face { format, root }).bytes
    }

    /// The byte-identity pin on which the single-writer contract stands:
    /// every face renders the DOM path's exact bytes at 0/1/2/N rows,
    /// root and root-less, including every escaping edge the codec owns.
    #[test]
    fn the_face_writer_is_byte_identical_to_the_dom_path() {
        let all = adversarial();
        let files_a: Vec<(&str, Vec<Row>)> = vec![("/p/lib/a.dart", all[..2].to_vec())];
        let files_b: Vec<(&str, Vec<Row>)> = vec![
            ("/p/lib/a.dart", all[..2].to_vec()),
            ("/p/lib/é.dart", all[2..3].to_vec()),
            ("/outside/x.dart", all[3..].to_vec()),
        ];
        let empty: Vec<(&str, Vec<Row>)> = vec![("/p/lib/a.dart", Vec::new())];
        let one: Vec<(&str, Vec<Row>)> = vec![("/p/lib/a.dart", all[..1].to_vec())];
        let cases: Vec<Vec<(&str, Vec<Row>)>> = vec![empty, one, files_a, files_b];
        for files in &cases {
            for root in [Some(PathBuf::from("/p")), None] {
                for format in [FaceFormat::Lon, FaceFormat::Json, FaceFormat::Text] {
                    assert_eq!(
                        face_bytes(files, root.clone(), format),
                        dom_bytes(files, root.clone(), format),
                        "{format:?} over {} file(s), root {root:?}",
                        files.len(),
                    );
                }
            }
        }
    }

    /// The meta counts: report severities (priority rows counted by the
    /// daemon's splice, not here), the modelable-only `has_diag_error`,
    /// and the unmodelable rows as serialized advisories.
    #[test]
    fn the_face_meta_counts_report_severities_and_keeps_unmodelable() {
        let files: Vec<(&str, Vec<Row>)> = vec![(
            "/p/lib/a.dart",
            vec![
                row(
                    "FATAL",
                    "UNKNOWN_KIND",
                    "/p/lib/a.dart",
                    0,
                    1,
                    1,
                    1,
                    "weird",
                    "x",
                ),
                row(
                    "WARNING",
                    "STATIC_WARNING",
                    "/p/lib/a.dart",
                    1,
                    1,
                    2,
                    1,
                    "w",
                    "dead_code",
                ),
            ],
        )];
        let report = Report::from_files(files.iter().map(|(f, r)| (*f, r.as_slice())));
        let rows: Vec<&Row> = report.rows.iter().collect();
        let render = render_official_face(
            &rows,
            &Face {
                format: FaceFormat::Lon,
                root: None,
            },
        );
        assert!(render.meta.has_warning && !render.meta.has_error && !render.meta.has_info);
        assert!(!render.meta.has_diag_error, "no modelable ERROR");
        assert_eq!(render.meta.unparsed.len(), 1, "the FATAL row is advisory");
        assert!(render.meta.unparsed[0].contains("FATAL"));
        // Only the modelable row reaches the bytes.
        let text = String::from_utf8(render.bytes).expect("utf8");
        assert!(text.contains("dead_code") && !text.contains("weird"));
    }

    // -----------------------------------------------------------------------
    // write_diags_lon — the streaming analyze/check lon document
    // -----------------------------------------------------------------------

    /// A `FileLayoutProvider` over an interned [`FileTable`], optionally
    /// carrying an analysis root. `line_col` derives a deterministic 1-based
    /// `(line, col)` from the UTF-16 offset so every row's position is
    /// reproducible without a source index — the DOM path and the streaming
    /// path read the same provider, so byte-identity holds regardless of the
    /// concrete values.
    struct DiagLayout {
        table: FileTable,
        root: Option<PathBuf>,
    }

    impl FileLayoutProvider for DiagLayout {
        fn file_path(&self, file: FileId) -> &Path {
            self.table.path(file).expect("interned FileId")
        }

        fn analysis_root(&self) -> Option<&Path> {
            self.root.as_deref()
        }

        fn line_col(&self, _file: FileId, utf16_offset: u32) -> (u32, u32) {
            (utf16_offset + 1, 7)
        }
    }

    /// A test code descriptor; `name` is the `code` column's spelling.
    fn test_code(name: &'static str) -> CodeInfo {
        CodeInfo::new(name, name, DiagnosticType::CompileTimeError, false)
    }

    /// Interns `path` and builds one engine diagnostic at `offset`/`length`
    /// with the given severity/code/message. `message` accepts a `String`
    /// (shared) or `&'static str` (static template) — the `Into<SharedStr>`
    /// seam picks the storage.
    fn engine_diag(
        table: &mut FileTable,
        path: &str,
        offset: u32,
        length: u32,
        severity: Severity,
        code: CodeInfo,
        message: impl Into<dartq_diagnostics::SharedStr>,
    ) -> Diagnostic {
        Diagnostic::new(
            severity,
            code,
            message,
            table.intern(Path::new(path)),
            SourceRange { offset, length },
            None,
        )
    }

    /// The byte-identity contract on small outputs: `write_diags_lon` must
    /// emit exactly the bytes `write_lon(diags_document(..))` produces — the
    /// DOM path stays the reference wherever it fits under the codec's
    /// element budget. Covers 0/1/2/N rows, root and root-less, and the
    /// string-escaping edges (reserved tokens, quotes, unicode, edge `'`).
    #[test]
    fn diags_lon_stream_is_byte_identical_to_the_dom_document() {
        let cases: Vec<(
            Vec<(&'static str, u32, u32, Severity, &'static str, String)>,
            Option<&'static str>,
        )> = vec![
            (vec![], Some("/w/pkg")),
            (vec![], None),
            (
                vec![(
                    "/w/pkg/lib/a.dart",
                    0,
                    1,
                    Severity::Error,
                    "undefined_identifier",
                    "Expected an identifier.".into(),
                )],
                Some("/w/pkg"),
            ),
            (
                vec![
                    (
                        "/w/pkg/lib/a.dart",
                        0,
                        1,
                        Severity::Error,
                        "undefined_identifier",
                        "Expected an identifier.".into(),
                    ),
                    (
                        "/w/pkg/lib/a.dart",
                        10,
                        2,
                        Severity::Warning,
                        "dead_code",
                        "careful [brackets] {braces}".into(),
                    ),
                ],
                Some("/w/pkg"),
            ),
            (
                vec![
                    (
                        "/w/pkg/lib/a.dart",
                        0,
                        1,
                        Severity::Error,
                        "null",
                        "boom \"quoted\" \\ slash".into(),
                    ),
                    (
                        "/w/pkg/lib/é.dart",
                        1,
                        3,
                        Severity::Info,
                        "avoid_print",
                        "á unicode \u{2028}sep \u{1F600} emoji".into(),
                    ),
                    (
                        "/outside/x.dart",
                        5,
                        1,
                        Severity::Error,
                        "dead_code",
                        "'edge quote'".into(),
                    ),
                ],
                Some("/w/pkg"),
            ),
            (
                vec![
                    (
                        "/w/pkg/lib/a.dart",
                        0,
                        1,
                        Severity::Error,
                        "yes",
                        "first".into(),
                    ),
                    (
                        "/w/pkg/lib/b.dart",
                        20,
                        4,
                        Severity::Warning,
                        "no",
                        "second row".into(),
                    ),
                    (
                        "/w/pkg/lib/c.dart",
                        40,
                        2,
                        Severity::Info,
                        "on",
                        "third".into(),
                    ),
                ],
                None,
            ),
        ];
        for (rows, root) in &cases {
            let mut table = FileTable::new();
            let diagnostics: Vec<Diagnostic> = rows
                .iter()
                .map(|row| {
                    engine_diag(
                        &mut table,
                        row.0,
                        row.1,
                        row.2,
                        row.3,
                        test_code(row.4),
                        row.5.clone(),
                    )
                })
                .collect();
            let files = DiagLayout {
                table,
                root: root.map(PathBuf::from),
            };
            let output = AnalysisOutput {
                diagnostics: &diagnostics,
                files: &files,
            };
            // The DOM reference: the shared builder + the codec's own line.
            let mut expected = Vec::new();
            write_lon(&mut expected, &diags_document(&output)).expect("dom lon");
            // The streaming writer under test.
            let mut actual = Vec::new();
            write_diags_lon(&mut actual, &output).expect("stream lon");
            assert_eq!(
                String::from_utf8_lossy(&actual),
                String::from_utf8_lossy(&expected),
                "stream != dom over {} row(s), root {root:?}",
                diagnostics.len(),
            );
        }
    }

    /// The scale contract: a diagnostic set large enough that the DOM +
    /// codec path exceeds its 1,000,000 expanded-element budget must still
    /// render through `write_diags_lon` — no value graph, no codec limit,
    /// no panic. Each row contributes eight counted elements (the row plus
    /// its seven field values), so ~130k rows crosses 1M; the codec's
    /// `to_node` budget errors on the same document while the stream emits
    /// every byte. The expected bytes come from an independent in-test
    /// generator over the same bare-safe rows, so the check is not
    /// circular.
    #[test]
    fn diags_lon_stream_passes_the_codec_element_budget() {
        const ROWS: usize = 130_000; // 8 elements/row → ~1.04M > 1,000,000 budget
        let mut table = FileTable::new();
        // One shared file keeps every row bare-safe (no quoting) so the
        // golden generator spells cells verbatim; `offset` varies the line.
        let file = table.intern(Path::new("/w/pkg/lib/a.dart"));
        let code = test_code("undefined_identifier");
        let diagnostics: Vec<Diagnostic> = (0..ROWS as u32)
            .map(|i| {
                Diagnostic::new(
                    Severity::Error,
                    code,
                    "boom", // &'static str → SharedStr::Static, zero heap/row
                    file,
                    SourceRange {
                        offset: i,
                        length: 3,
                    },
                    None,
                )
            })
            .collect();
        let files = DiagLayout {
            table,
            root: Some(PathBuf::from("/w/pkg")),
        };
        let output = AnalysisOutput {
            diagnostics: &diagnostics,
            files: &files,
        };

        // The old path's codec budget: the same document as a Value graph
        // exceeds 1M expanded elements and `to_string` refuses it (the
        // `expect` in `lon_line` is what turned that refusal into rc=4).
        let document = diags_document(&output);
        assert!(
            lon::to_string(&document).is_err(),
            "the DOM document must exceed the codec's 1M element budget"
        );
        // Free the ~1M-element DOM before materializing the stream bytes and
        // the golden — the test's peak stays near the output size, not the
        // sum of both graphs.
        drop(document);

        // The streaming writer completes and matches the independent golden.
        let mut actual = Vec::new();
        write_diags_lon(&mut actual, &output).expect("stream lon at scale");
        let actual = String::from_utf8(actual).expect("utf8");
        let mut expected = String::with_capacity(actual.len());
        expected.push_str("{v:1 root:/w/pkg diags:[code sev file line col len msg");
        for i in 0..ROWS as u32 {
            // line_col(offset)= (offset+1, 7); len=3; every field bare-safe.
            expected.push_str(&format!(
                ";undefined_identifier error lib/a.dart {} 7 3 boom",
                i + 1
            ));
        }
        expected.push_str("]}\n");
        assert_eq!(
            actual, expected,
            "stream bytes diverge from the golden at scale"
        );
    }
}
