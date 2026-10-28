//! The analyze/check faces' cross-process renderers: the one writer per
//! output format that BOTH the CLI's cold path and the daemon's routed
//! face (`dartq/analyze`/`dartq/check` with a `format` parameter, #2627)
//! render through — one renderer per format, by relocation rather than
//! duplication, so the routed bytes are the cold bytes structurally.
//!
//! * [`lon_line`]/[`write_lon`] — one canonical LON line of a document
//!   value (the `lon` crate's encoder; the document itself comes from the
//!   shared [`crate::analyze::diags_document`] builder, #2657);
//! * [`write_human`] (+[`text_diag_row`]) — the analyze/check `text`
//!   face: the `root:` line, per-file groups, the count tail;
//! * the `json` face's oracle envelope writer stays in `dartq-diagnostics`
//!   (`write_json`) — it was already the one implementation.
//!
//! The daemon renders into a buffer and returns the bytes over the
//! session wire; the one-shot CLI relays them to stdout verbatim. The
//! severity-exit tier travels beside the bytes (`counts`), never inside
//! them.

use std::collections::BTreeMap;
use std::io::{self, Write};
use std::path::Path;

use serde_json::Value;

use dartq_diagnostics::{Diagnostic, FileLayoutProvider};

use crate::analyze::severity_word;

/// One LON line of `value`.
///
/// # Panics
///
/// The value graph does not encode as LON — non-finite floats and
/// codec-limit overflows are program bugs, not runtime cases.
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
