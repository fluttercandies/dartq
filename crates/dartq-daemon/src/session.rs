//! The session: one [`Engine`] plus the open-document overlay and the
//! publish-diff baseline. All engine calls happen here, synchronously, on
//! the main-loop thread (CPU-bound work, no lock contention by construction).

use std::collections::hash_map::Entry;
use std::fs;
use std::io;
use std::path::PathBuf;

use dartq_diagnostics::{Diagnostic, Severity};
use dartq_engine::{CancellationToken, Cancelled, Engine};
use rustc_hash::FxHashMap;
use thiserror::Error;

use crate::lsp;
use crate::uri;

/// Session failures the server can only report, not fix.
#[derive(Debug, Error)]
pub enum SessionError {
    /// Workspace discovery or an overlay restore hit an unreadable file.
    #[error("filesystem access failed: {0}")]
    Io(#[from] io::Error),
    /// A cancellation checkpoint fired; completed work stays cached.
    #[error("analysis cancelled")]
    Cancelled,
}

impl From<Cancelled> for SessionError {
    fn from(_: Cancelled) -> Self {
        Self::Cancelled
    }
}

/// One pending `textDocument/publishDiagnostics` push.
#[derive(Debug)]
pub struct Publish {
    pub uri: String,
    pub version: Option<i64>,
    pub diagnostics: Vec<lsp::Diagnostic>,
}

/// The daemon's analysis state: engine + open (overlay) documents + the last
/// published diagnostics per file.
pub struct Session {
    engine: Engine,
    /// Open overlay documents: last-known client version per path. A closed
    /// or never-opened file is absent — publishes then carry no version.
    opened: FxHashMap<PathBuf, i64>,
    /// Baseline for publish diffing: a push only goes out when a file's
    /// diagnostic set differs from what was last published for it.
    published: FxHashMap<dartq_diagnostics::FileId, Vec<lsp::Diagnostic>>,
}

impl Session {
    /// Binds the pre-assembled engine (the CLI owns the pub/SDK context
    /// assembly; the daemon is shell only).
    #[must_use]
    pub fn new(engine: Engine) -> Self {
        Self {
            engine,
            opened: FxHashMap::default(),
            published: FxHashMap::default(),
        }
    }

    /// The `initialized`-time workspace sweep: discover every surface file,
    /// analyze, and publish initial diagnostics for everything that has any.
    ///
    /// # Errors
    ///
    /// Discovery I/O or a cancelled analysis.
    pub fn sweep(&mut self) -> Result<Vec<Publish>, SessionError> {
        self.engine.discover_workspace()?;
        let diagnostics = self.analyze()?;
        Ok(self.plan_publishes(&diagnostics))
    }

    /// `textDocument/didOpen`: installs the overlay text and reanalyzes.
    /// Non-`file:` URIs have no analysis path and are skipped (`Ok(None)`).
    ///
    /// # Errors
    ///
    /// A cancelled analysis.
    pub fn open(
        &mut self,
        uri: &str,
        text: &str,
        version: i64,
    ) -> Result<Option<Vec<Publish>>, SessionError> {
        let Some(path) = uri::to_path(uri) else {
            return Ok(None);
        };
        self.opened.insert(path.clone(), version);
        self.engine.set_content(&path, text.as_bytes());
        let diagnostics = self.analyze()?;
        Ok(Some(self.plan_publishes(&diagnostics)))
    }

    /// `textDocument/didChange` (whole-file replacement, M1 overlay
    /// semantics, PRD §16.3): installs the new text and reanalyzes.
    ///
    /// # Errors
    ///
    /// A cancelled analysis.
    pub fn change(
        &mut self,
        uri: &str,
        text: &str,
        version: i64,
    ) -> Result<Option<Vec<Publish>>, SessionError> {
        let Some(path) = uri::to_path(uri) else {
            return Ok(None);
        };
        self.opened.insert(path.clone(), version);
        self.engine.set_content(&path, text.as_bytes());
        let diagnostics = self.analyze()?;
        Ok(Some(self.plan_publishes(&diagnostics)))
    }

    /// `textDocument/didClose`: drops the overlay. The engine has no
    /// per-file "clear overlay" face, so the equivalent whole-file
    /// operation restores the on-disk bytes; an overlay-only file (opened,
    /// never saved) is removed from analysis. Closing a document that was
    /// never opened is a no-op per LSP.
    ///
    /// # Errors
    ///
    /// Disk read failure or a cancelled analysis.
    pub fn close(&mut self, uri: &str) -> Result<Option<Vec<Publish>>, SessionError> {
        let Some(path) = uri::to_path(uri) else {
            return Ok(None);
        };
        if self.opened.remove(&path).is_none() {
            return Ok(None);
        }
        match fs::read(&path) {
            Ok(bytes) => {
                self.engine.set_content(&path, &bytes);
            }
            // Overlay-only buffer: dropping the overlay removes the file.
            // `UnknownPath` cannot happen — `open` interned the path.
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let _ = self.engine.remove_file(&path);
            }
            Err(error) => return Err(error.into()),
        }
        let diagnostics = self.analyze()?;
        Ok(Some(self.plan_publishes(&diagnostics)))
    }

    /// The `dartq/analyze` pull face: a fresh full analysis, every entry
    /// tagged with its document URI.
    ///
    /// # Errors
    ///
    /// A cancelled analysis.
    pub fn analyze_report(&mut self) -> Result<Vec<lsp::AnalyzeEntry>, SessionError> {
        let diagnostics = self.analyze()?;
        let entries = diagnostics
            .iter()
            .filter_map(|diagnostic| {
                let converted = self.convert(diagnostic)?;
                let path = self.engine.file_table().path(diagnostic.file)?;
                Some(lsp::AnalyzeEntry {
                    uri: uri::from_path(path),
                    diagnostic: converted,
                })
            })
            .collect();
        Ok(entries)
    }

    /// Runs the engine barrier + recompute: `analyze(&[])` applies the
    /// overlay and returns the merged diagnostic set of every live library,
    /// canonically sorted.
    fn analyze(&mut self) -> Result<Vec<Diagnostic>, Cancelled> {
        self.engine
            .analyze(&[], &mut |_| {}, &CancellationToken::default())
    }

    /// Diffs the fresh diagnostic set against the published baseline and
    /// returns one [`Publish`] per changed file (empty set included — the
    /// client clears its squiggles through it). Order is by URI, so the
    /// push sequence is deterministic.
    fn plan_publishes(&mut self, diagnostics: &[Diagnostic]) -> Vec<Publish> {
        let mut current: FxHashMap<dartq_diagnostics::FileId, Vec<lsp::Diagnostic>> =
            FxHashMap::default();
        for diagnostic in diagnostics {
            if let Some(converted) = self.convert(diagnostic) {
                current.entry(diagnostic.file).or_default().push(converted);
            }
        }
        // Reconsider every diagnosed file plus every baseline file whose
        // diagnostics vanished entirely.
        let mut files: Vec<dartq_diagnostics::FileId> = current.keys().copied().collect();
        files.extend(
            self.published
                .iter()
                .filter(|(file, previous)| !previous.is_empty() && !current.contains_key(*file))
                .map(|(file, _)| *file),
        );

        let mut publishes: Vec<Publish> = Vec::new();
        for file in files {
            // Duplicated ids only when the same file appears in both sets —
            // the second pass then sees an empty `current` and no-ops.
            let now = match current.entry(file) {
                Entry::Occupied(occupied) => occupied.remove(),
                Entry::Vacant(_) => Vec::new(),
            };
            if self
                .published
                .get(&file)
                .is_some_and(|before| *before == now)
            {
                continue;
            }
            let Some(path) = self.engine.file_table().path(file) else {
                continue;
            };
            let path = path.to_path_buf();
            self.published.insert(file, now.clone());
            publishes.push(Publish {
                uri: uri::from_path(&path),
                version: self.opened.get(&path).copied(),
                diagnostics: now,
            });
        }
        publishes.sort_by(|a, b| a.uri.cmp(&b.uri));
        publishes
    }

    /// Converts one engine diagnostic to the LSP shape. Returns `None` only
    /// for a non-resident file, which the engine's "live libraries only"
    /// contract excludes.
    fn convert(&self, diagnostic: &Diagnostic) -> Option<lsp::Diagnostic> {
        let index = self.engine.line_index(diagnostic.file)?;
        let text = self.text(diagnostic.file)?;
        // `line_col_utf16` answers the oracle's 1-based (line, column); LSP
        // positions are zero-based.
        let end_offset = diagnostic
            .range
            .offset
            .checked_add(diagnostic.range.length)
            .expect("diagnostic range overflows u32");
        let (start_line, start_column) = index.line_col_utf16(&text, diagnostic.range.offset);
        let (end_line, end_column) = index.line_col_utf16(&text, end_offset);
        let severity = match diagnostic.severity {
            Severity::Error => 1,
            Severity::Warning => 2,
            Severity::Info => 3,
        };
        Some(lsp::Diagnostic {
            range: lsp::Range {
                start: lsp::Position {
                    line: start_line - 1,
                    character: start_column - 1,
                },
                end: lsp::Position {
                    line: end_line - 1,
                    character: end_column - 1,
                },
            },
            severity,
            code: diagnostic.code.name.to_owned(),
            source: "dartq".into(),
            message: diagnostic.message.clone(),
        })
    }

    /// The resident text of a file. `set_content` is the only write path and
    /// decodes lossily, so resident content is valid UTF-8 by construction.
    fn text(&self, file: dartq_diagnostics::FileId) -> Option<String> {
        let bytes = self.engine.content(file)?;
        Some(
            std::str::from_utf8(bytes)
                .expect("engine content is valid UTF-8")
                .to_owned(),
        )
    }
}
