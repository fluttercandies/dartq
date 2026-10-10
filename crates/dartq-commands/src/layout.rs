//! Layout resolution over the engine's read faces, shared by every output
//! face that prints 1-based positions: paths from the file table, positions
//! from the resident line index, text borrowed straight from the content
//! store — no per-diagnostic file reads or copies.

use std::collections::BTreeMap;
use std::path::Path;

use dartq_diagnostics::{FileId, FileLayoutProvider, Severity};
use dartq_engine::Engine;
use dartq_source::LineIndex;

/// [`FileLayoutProvider`] over the engine's read faces.
///
/// The (text, line index) pair is snapshotted once per distinct file in the
/// output set: the writers call [`FileLayoutProvider::line_col`] up to twice
/// per diagnostic (dartq-diagnostics writer.rs:128-129) and the content face
/// re-validates UTF-8 on every call, so uncached resolution is O(D·N) on a
/// diagnostic-heavy file. The check face's [`crate::check::FileSet`] caches
/// the same pair per file.
pub struct EngineLayout<'a> {
    engine: &'a Engine,
    /// Exactly the FileIds the caller will query, each with its resident
    /// text and line index.
    cache: BTreeMap<FileId, (&'a str, &'a LineIndex)>,
}

impl<'a> EngineLayout<'a> {
    /// Snapshots (text, index) once per distinct file in `files`.
    #[must_use]
    pub fn over(engine: &'a Engine, files: impl IntoIterator<Item = FileId>) -> Self {
        let mut cache = BTreeMap::new();
        for file in files {
            cache.entry(file).or_insert_with(|| {
                // Engine content decoded as UTF-8 at set_content time
                // (engine doc); validated exactly once per file here.
                let text = std::str::from_utf8(engine.content(file).expect("analyzed FileId"))
                    .expect("engine content is UTF-8");
                (text, engine.line_index(file).expect("analyzed FileId"))
            });
        }
        Self { engine, cache }
    }
}

impl FileLayoutProvider for EngineLayout<'_> {
    fn file_path(&self, file: FileId) -> &Path {
        self.engine
            .file_table()
            .path(file)
            .expect("analyzed FileId")
    }

    fn line_col(&self, file: FileId, utf16_offset: u32) -> (u32, u32) {
        let (text, index) = self.cache.get(&file).expect("output-set FileId");
        // Diagnostic offsets are UTF-16; the fused conversion reports the
        // official `LineInfo.getLocation` (line, UTF-16 column) directly.
        index.line_col_utf16(text, utf16_offset)
    }
}

/// Oracle severity priority (`DiagnosticWithPath._severityPriority`): Error
/// first, then Warning, then Info. Deliberately NOT the dtype→Severity
/// derivation (`DiagnosticType::severity` in dartq-diagnostics, shared by
/// dartq-engine): this only orders already-derived severities to pick the
/// output order.
pub(crate) fn severity_rank(severity: Severity) -> u8 {
    match severity {
        Severity::Error => 0,
        Severity::Warning => 1,
        Severity::Info => 2,
    }
}

/// Plain byte key of a path — the oracle's `filePath.compareTo` string order.
pub(crate) fn path_key(path: &Path) -> &[u8] {
    path.as_os_str().as_encoded_bytes()
}
