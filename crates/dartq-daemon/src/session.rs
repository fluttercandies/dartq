//! The session: one [`Runtime`] (the root analysis context) plus the
//! open-document overlay, the publish-diff baseline, and the debounced
//! snapshot sink. All engine and scheduler calls happen here, synchronously,
//! on the main-loop thread (CPU-bound work, no lock contention by
//! construction).
//!
//! # Task flow (PRD §16.5, §16.6)
//!
//! Intake (a notification or request arrives) only *stages* work: overlay
//! edits enqueue one write task plus the diagnostics read the publish push
//! rides on, a `dartq/*` request attaches to (or enqueues) one read task,
//! stamped with the context's epoch at enqueue. Execution
//! happens at the single [`Session::service`] drain point, in the
//! scheduler's writer-priority order — every staged write applies before any
//! read analyzes, so N batched edits collapse into one apply + one analysis,
//! and a read enqueued before a write still answers with the post-write
//! view. `$/cancelRequest` cancels per id (rust-analyzer's posture): the
//! coalesced read is partitioned — the cancelled id answers
//! `RequestCancelled`, its co-riders re-attach to a read re-stamped at the
//! fresh epoch (mirroring the write re-stamp), and when a write survives
//! with no request left riding on the read, a *zero-request read* is
//! re-stamped with it so the publish diff still drains — an edit+cancel
//! batch can never leave the client's diagnostics stale.
//!
//! # Snapshot persistence (#306)
//!
//! The sink is debounced by a last-apply timestamp: while it is dirty, the
//! main loop bounds its receive by the window's remaining time, so the save
//! runs at the deadline even when the daemon then sits idle (and regardless
//! of the window at session end). Save failures log and continue: the
//! accelerator invariant makes an unwritten generation a future miss, never
//! an error.

use std::collections::hash_map::Entry;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use dartq_diagnostics::{Diagnostic, Severity};
use dartq_engine::{
    CancellationToken, Cancelled, ContextConfig, ContextId, Engine, EngineConfig, EngineError,
    OverlayChange, Runtime,
};
use rustc_hash::FxHashMap;
use thiserror::Error;

use dartq_snapshot::{SnapshotConfig, SnapshotStore};

use crate::commands::{self, Request};
use crate::jsonrpc::RequestId;
use crate::lsp;
use crate::uri;

/// The debounce window between the last overlay apply and the snapshot
/// generation save (#306): 500 ms. While a save is pending the main loop's
/// idle receive is bounded by the window's remainder, so an idle daemon
/// wakes to save — no timer thread.
pub(crate) const SNAPSHOT_DEBOUNCE: Duration = Duration::from_millis(500);

/// Session failures the server can only report, not fix.
#[derive(Debug, Error)]
pub enum SessionError {
    /// Workspace discovery or an overlay restore hit an unreadable file.
    #[error("filesystem access failed: {0}")]
    Io(#[from] io::Error),
    /// The request's task was stale at the drain (its epoch was superseded
    /// by a cancel); completed work stays cached, and nothing was produced.
    #[error("analysis cancelled")]
    Cancelled,
    /// A `dartq/*` request's parameters name no valid target (the CLI's
    /// usage tier, surfaced as `INVALID_PARAMS`).
    #[error("{0}")]
    InvalidParams(String),
    /// A `dartq/*` command aborted after a usable request (the CLI's
    /// internal tier, surfaced as `INTERNAL_ERROR`).
    #[error("{0}")]
    Internal(String),
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

/// What one [`Session::service`] turn produced, in drain-execution order.
#[derive(Default)]
pub struct Turn {
    /// Publishes from the turn's read task (a no-op diff when nothing
    /// changed).
    pub publishes: Vec<Publish>,
    /// One entry per attached request; `Err` marks a request whose task was
    /// stale at the drain (cancelled) or whose parameters named no valid
    /// target, and produced nothing.
    pub answers: Vec<(RequestId, Result<serde_json::Value, SessionError>)>,
}

/// A queued read task and the requests waiting on its answer. Reads coalesce:
/// one analysis serves every attached request plus the publish diff.
struct PendingRead {
    requests: Vec<(RequestId, Request)>,
}

/// Debounced snapshot persistence state: dirty since the last overlay apply,
/// the instant that apply settled, and the store to save into.
struct SnapshotSink {
    store: Option<SnapshotStore>,
    dirty: bool,
    last_apply: Option<Instant>,
}

impl SnapshotSink {
    fn note_apply(&mut self) {
        self.dirty = true;
        self.last_apply = Some(Instant::now());
    }

    /// When the pending save comes due (`last_apply + window`); `None` when
    /// the sink is clean, meaning the main loop's receive may block
    /// indefinitely. `dirty` implies `last_apply` — both are set together
    /// by [`Self::note_apply`] — so this is the single source of truth for
    /// the window math, shared by the idle receive bound and `flush_if_due`.
    fn deadline(&self) -> Option<Instant> {
        self.last_apply
            .filter(|_| self.dirty)
            .map(|at| at + SNAPSHOT_DEBOUNCE)
    }

    /// Saves when the debounce window has elapsed since the last apply;
    /// true when a save was attempted.
    fn flush_if_due(&mut self, now: Instant) -> bool {
        match self.deadline() {
            Some(deadline) if now >= deadline => {
                self.flush();
                true
            }
            _ => false,
        }
    }

    fn flush(&mut self) {
        self.dirty = false;
        #[cfg(test)]
        crate::probe::count_flush();
        self.save_generation();
    }

    /// The generation save (#309, #306): one [`dartq_snapshot::GenerationHeader`]
    /// plus per-file fact entries, written through [`SnapshotStore::save`].
    ///
    /// Blocked on four engine faces that do not exist yet (the M3
    /// facts-snapshot ticket must land them):
    ///
    /// 1. `FACTS_SEMANTIC_VERSION` is `pub(crate)` (dartq-engine `state.rs`)
    ///    — `GenerationHeader::semantic_version` and `EntryKey::semantic_version`
    ///    need it exported;
    /// 2. no face computes the package-graph fingerprint
    ///    (`GenerationHeader::package_graph_fingerprint`);
    /// 3. `UnitFacts` (with its `FileElements` / `LineIndex` /
    ///    `DirectiveFacts` members) is not `Serialize` — the entry payload;
    /// 4. the SDK identity's canonical string encoding is unpinned
    ///    (`SdkSnapshot.dart` carries version/revision/hash).
    ///
    /// Until then every flush logs once and continues: the accelerator
    /// invariant makes an unwritten generation a future miss, never an error.
    fn save_generation(&self) {
        let root = self.store.as_ref().map_or_else(
            || "<unavailable>".into(),
            |store| store.root().display().to_string(),
        );
        eprintln!(
            "dartq-daemon: snapshot save skipped (engine faces missing: FACTS_SEMANTIC_VERSION \
             export, package-graph fingerprint, UnitFacts Serialize, sdk identity encoding); \
             store root {root}"
        );
    }
}

/// The daemon's analysis state: the runtime + root context, open (overlay)
/// documents, the last published diagnostics per file, the staged-but-
/// undrained task set, and the snapshot sink.
pub struct Session {
    runtime: Runtime,
    /// The root context; the daemon is shell only and never closes it.
    context: ContextId,
    /// The workspace root the context was assembled from (relative
    /// `dartq/*` request paths resolve against it).
    workspace_root: PathBuf,
    next_cookie: u64,
    /// Open overlay documents: last-known client version per path. A closed
    /// or never-opened file is absent — publishes then carry no version.
    opened: FxHashMap<PathBuf, i64>,
    /// Baseline for publish diffing: a push only goes out when a file's
    /// diagnostic set differs from what was last published for it.
    published: FxHashMap<dartq_diagnostics::FileId, Vec<lsp::Diagnostic>>,
    /// Staged overlay mutations riding the queued write task (empty ⇔ no
    /// write task queued — the two change together).
    staged: Vec<OverlayChange>,
    /// The queued write task's cookie.
    write: Option<u64>,
    /// The queued read task, if any.
    read: Option<PendingRead>,
    /// Requests cancelled while queued, answered cancelled at the drain.
    cancelled: Vec<RequestId>,
    snapshot: SnapshotSink,
}

impl Session {
    /// Assembles the root context from the pre-validated configuration (the
    /// CLI owns SDK and workspace discovery; the daemon is shell only) and
    /// opens the XDG-rooted snapshot store. A missing cache root only means
    /// no persistence (accelerator semantics), never a failed session.
    ///
    /// # Errors
    ///
    /// [`EngineError::PackageConfig`] when the config cannot back the
    /// URI-resolution table.
    pub fn new(config: EngineConfig) -> Result<Self, EngineError> {
        let store = match SnapshotStore::open(SnapshotConfig::default()) {
            Ok(store) => Some(store),
            Err(error) => {
                eprintln!("dartq-daemon: snapshot store unavailable: {error}");
                None
            }
        };
        Self::assemble(config, store)
    }

    /// [`Session::new`] with an explicit snapshot store (tests pin a
    /// throwaway root).
    ///
    /// # Errors
    ///
    /// [`EngineError::PackageConfig`] when the config cannot back the
    /// URI-resolution table.
    pub fn with_store(config: EngineConfig, store: SnapshotStore) -> Result<Self, EngineError> {
        Self::assemble(config, Some(store))
    }

    fn assemble(config: EngineConfig, store: Option<SnapshotStore>) -> Result<Self, EngineError> {
        let mut runtime = Runtime::new();
        // The context's workspace root (`config_dir` = root/.dart_tool);
        // captured before the config moves into the context.
        let workspace_root = config
            .config_dir
            .parent()
            .map_or_else(Default::default, Path::to_path_buf);
        let context = runtime.open_context(ContextConfig {
            engine: config,
            flags: 0,
        })?;
        Ok(Self {
            runtime,
            context,
            workspace_root,
            next_cookie: 0,
            opened: FxHashMap::default(),
            published: FxHashMap::default(),
            staged: Vec::new(),
            write: None,
            read: None,
            cancelled: Vec::new(),
            snapshot: SnapshotSink {
                store,
                dirty: false,
                last_apply: None,
            },
        })
    }

    /// The `initialized`-time workspace sweep: discover every surface file
    /// and enqueue the initial analysis read (its publishes flow from the
    /// turn's drain).
    ///
    /// # Errors
    ///
    /// Discovery I/O.
    pub fn sweep(&mut self) -> Result<(), SessionError> {
        self.engine_mut().discover_workspace()?;
        self.ensure_read();
        Ok(())
    }

    /// `textDocument/didOpen` / `textDocument/didChange` (whole-file
    /// replacement, M1 overlay semantics, PRD §16.3): stages the overlay
    /// text on the queued write task and co-queues the diagnostics read the
    /// publish push rides on (reads coalesce, so N batched edits still
    /// reanalyze once). False for non-`file:` URIs, which have no analysis
    /// path and are skipped.
    pub fn queue_edit(&mut self, uri: &str, text: &str, version: i64) -> bool {
        let Some(path) = uri::to_path(uri) else {
            return false;
        };
        self.opened.insert(path.clone(), version);
        self.stage(OverlayChange::Set {
            path,
            bytes: text.as_bytes().to_vec(),
        });
        self.ensure_write();
        self.ensure_read();
        true
    }

    /// `textDocument/didClose`: stages the overlay drop. The runtime overlay
    /// has no per-file "restore" change, so the equivalent whole-file
    /// operation stages the on-disk bytes; an overlay-only file (opened,
    /// never saved) stages a removal. Closing a document that was never
    /// opened is a no-op per LSP.
    ///
    /// The on-disk read happens before any state mutation: on a read error
    /// that is not `NotFound` the document stays open and any staged edits
    /// stay queued — the overlay bookkeeping and the staged writes only ever
    /// diverge from what a successful close would have done.
    ///
    /// # Errors
    ///
    /// Disk read failure.
    pub fn queue_close(&mut self, uri: &str) -> io::Result<bool> {
        let Some(path) = uri::to_path(uri) else {
            return Ok(false);
        };
        if !self.opened.contains_key(&path) {
            return Ok(false);
        }
        match fs::read(&path) {
            Ok(bytes) => {
                self.opened.remove(&path);
                self.stage(OverlayChange::Set { path, bytes });
            }
            // Overlay-only buffer: dropping the overlay removes the file.
            // `UnknownPath` cannot happen — `open` interned the path.
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                self.opened.remove(&path);
                self.stage(OverlayChange::Remove { path });
            }
            Err(error) => return Err(error),
        }
        self.ensure_write();
        self.ensure_read();
        Ok(true)
    }

    /// A `dartq/*` request: attaches to the queued read task (or queues
    /// one), stamped with the context's epoch at enqueue. Answered at the
    /// drain, in drain order — the coalesced analysis runs once, and each
    /// request's command shapes its own answer from the post-write state.
    /// Crate-internal: the wire face is [`crate::server`]'s method table.
    pub(crate) fn queue_command(&mut self, id: RequestId, request: Request) {
        self.ensure_read();
        if let Some(pending) = self.read.as_mut() {
            pending.requests.push((id, request));
        }
    }

    /// The workspace root the session's context was assembled from.
    pub(crate) fn workspace_root(&self) -> &Path {
        &self.workspace_root
    }

    /// `$/cancelRequest`: cancels one queued request by id, precisely
    /// (rust-analyzer cancels per id, not per batch). The coalesced read
    /// task is partitioned: the cancelled id answers `RequestCancelled` at
    /// the drain, while co-riding requests re-attach to a read task
    /// re-stamped at the fresh epoch — mirroring the write re-stamp below.
    /// When a write task survives (staged, unobserved edits) and no request
    /// is left riding on the read, a zero-request read is re-stamped with
    /// it, so the publish diff the cancelled read was carrying still runs.
    ///
    /// Cancelling an unknown or already-answered id is ignored (LSP
    /// §cancelRequest). The staged overlay edits are *not* cancellable (LSP
    /// notifications carry no ids).
    ///
    /// # Panics
    ///
    /// Never in practice: the re-stamps enqueue against the root context,
    /// which lives for the session.
    pub fn cancel_request(&mut self, id: &RequestId) {
        let Some(pending) = &self.read else {
            return;
        };
        let Some(position) = pending
            .requests
            .iter()
            .position(|(queued, _)| queued == id)
        else {
            return;
        };
        let mut pending = self
            .read
            .take()
            .expect("position proved the read is queued");
        let (cancelled, _) = pending.requests.remove(position);
        self.cancelled.push(cancelled);
        let _ = self.runtime.cancel(self.context);
        // Re-stamp the surviving work at the fresh epoch — the stale
        // (superseded) tasks drop at the drain.
        if let Some(cookie) = self.write.take() {
            self.runtime
                .enqueue(self.context, true, cookie)
                .expect("the root context lives for the session; write re-stamp cannot fail");
            self.write = Some(cookie);
        }
        let survivors = pending.requests;
        if !survivors.is_empty() || self.write.is_some() {
            let cookie = self.next_cookie();
            self.runtime
                .enqueue(self.context, false, cookie)
                .expect("the root context lives for the session; read re-stamp cannot fail");
            self.read = Some(PendingRead {
                requests: survivors,
            });
        }
    }

    /// The turn's drain point: pops the scheduler in writer-priority order,
    /// applies every staged write, runs the coalesced read once, and drops
    /// stale tasks — dropped requests answer [`SessionError::Cancelled`].
    ///
    /// # Panics
    ///
    /// Never in practice: every drained task references the root context,
    /// which lives for the session.
    #[must_use]
    pub fn service(&mut self) -> Turn {
        let report = self.runtime.drain();
        let mut turn = Turn::default();
        // Cancelled requests were enqueued before every surviving task, so
        // their answers come first.
        turn.answers.extend(
            self.cancelled
                .drain(..)
                .map(|id| (id, Err(SessionError::Cancelled))),
        );
        for task in report.tasks {
            if task.write {
                let changes = std::mem::take(&mut self.staged);
                self.write = None;
                self.runtime
                    .apply_overlay(self.context, &changes, &mut |_| {})
                    .expect("a drain-live write references the open root context");
                self.snapshot.note_apply();
            } else {
                self.read_turn(&mut turn);
            }
        }
        turn
    }

    /// End-of-session flush: a dirty sink saves regardless of the debounce
    /// window (#306: the clean-shutdown write).
    pub fn flush_snapshot(&mut self) {
        if self.snapshot.dirty {
            self.snapshot.flush();
        }
    }

    /// Turn-start debounce check; true when a save was attempted.
    #[must_use]
    pub fn flush_if_due(&mut self, now: Instant) -> bool {
        self.snapshot.flush_if_due(now)
    }

    /// When the pending debounced save comes due: the main loop bounds its
    /// idle receive by this, so a daemon left idle wakes at the deadline and
    /// saves instead of blocking forever. `None` when the sink is clean —
    /// the receive blocks until the next frame.
    #[must_use]
    pub fn flush_deadline(&self) -> Option<Instant> {
        self.snapshot.deadline()
    }

    /// One read task's execution: a fresh full analysis — converted once,
    /// then shared by the publish diff, the `dartq/analyze` answers, and
    /// every other attached `dartq/*` command (which run over the analyzed
    /// state without re-driving the pass).
    fn read_turn(&mut self, turn: &mut Turn) {
        let pending = self
            .read
            .take()
            .expect("a drain-live read task is still pending");
        match self.analyze() {
            Ok(diagnostics) => {
                // The one conversion pass: the engine→LSP shape change is
                // the expensive half (line-index math over the resident
                // text), so the publish plan and the analyze entries both
                // consume it instead of converting twice.
                let converted: Vec<Option<lsp::Diagnostic>> = diagnostics
                    .iter()
                    .map(|diagnostic| self.convert(diagnostic))
                    .collect();
                turn.publishes
                    .extend(self.plan_publishes(&diagnostics, &converted));
                let entries: Vec<lsp::AnalyzeEntry> = diagnostics
                    .into_iter()
                    .zip(converted)
                    .filter_map(|(engine_diag, converted)| {
                        let diagnostic = converted?;
                        let path = self.engine().file_table().path(engine_diag.file)?;
                        Some(lsp::AnalyzeEntry {
                            uri: uri::from_path(path),
                            diagnostic,
                        })
                    })
                    .collect();
                let drain = commands::Drain {
                    engine: self.engine(),
                    root: &self.workspace_root,
                    entries: &entries,
                };
                for (id, request) in pending.requests {
                    let answer = commands::answer(&drain, &request);
                    turn.answers.push((id, answer));
                }
            }
            // Known semantics until the M4 worker pool: in-flight
            // cancellation is NOT wired — the token handed to `analyze` is
            // never cancelled, and the epoch only drops *queued* tasks at
            // the drain, before they execute. This arm exists for the
            // engine's contract (`analyze` can fail cancelled); stale work
            // never surfaces as a partial answer because it never starts.
            Err(Cancelled) => {
                for (id, _) in pending.requests {
                    turn.answers.push((id, Err(SessionError::Cancelled)));
                }
            }
        }
    }

    /// Stages one overlay mutation on the queued write task, last-wins per
    /// path: a batch's earlier ops for the same path are unobservable (ops
    /// on distinct paths commute, and the overlay declares the end state),
    /// so K batched edits of one file stage one text, not K full copies.
    fn stage(&mut self, change: OverlayChange) {
        let path = match &change {
            OverlayChange::Set { path, .. } | OverlayChange::Remove { path } => path,
        };
        self.staged.retain(|staged| match staged {
            OverlayChange::Set { path: old, .. } | OverlayChange::Remove { path: old } => {
                old != path
            }
        });
        self.staged.push(change);
    }

    fn ensure_write(&mut self) {
        if self.write.is_none() {
            let cookie = self.next_cookie();
            self.runtime
                .enqueue(self.context, true, cookie)
                .expect("the root context lives for the session");
            self.write = Some(cookie);
        }
    }

    fn ensure_read(&mut self) {
        if self.read.is_none() {
            let cookie = self.next_cookie();
            self.runtime
                .enqueue(self.context, false, cookie)
                .expect("the root context lives for the session");
            self.read = Some(PendingRead {
                requests: Vec::new(),
            });
        }
    }

    fn next_cookie(&mut self) -> u64 {
        self.next_cookie += 1;
        self.next_cookie
    }

    /// The root context's engine: analyze, queries, content reads.
    fn engine(&self) -> &Engine {
        self.runtime
            .context_state(self.context)
            .expect("the root context lives for the session")
    }

    fn engine_mut(&mut self) -> &mut Engine {
        self.runtime
            .context_state_mut(self.context)
            .expect("the root context lives for the session")
    }

    /// Runs the engine barrier + recompute: `analyze(&[])` applies the
    /// overlay and returns the merged diagnostic set of every live library,
    /// canonically sorted.
    fn analyze(&mut self) -> Result<Vec<Diagnostic>, Cancelled> {
        self.engine_mut()
            .analyze(&[], &mut |_| {}, &CancellationToken::default())
    }

    /// Diffs the fresh diagnostic set against the published baseline and
    /// returns one [`Publish`] per changed file (empty set included — the
    /// client clears its squiggles through it). Consumes the read task's
    /// shared conversions rather than converting again. Order is by URI, so
    /// the push sequence is deterministic.
    fn plan_publishes(
        &mut self,
        diagnostics: &[Diagnostic],
        converted: &[Option<lsp::Diagnostic>],
    ) -> Vec<Publish> {
        let mut current: FxHashMap<dartq_diagnostics::FileId, Vec<lsp::Diagnostic>> =
            FxHashMap::default();
        for (diagnostic, converted) in diagnostics.iter().zip(converted) {
            if let Some(converted) = converted {
                current
                    .entry(diagnostic.file)
                    .or_default()
                    .push(converted.clone());
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
            let Some(path) = self.engine().file_table().path(file) else {
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

    /// Converts one engine diagnostic to the LSP shape. The line index and
    /// the resident text are borrowed, never cloned — a per-diagnostic
    /// whole-file copy would make a read task O(D × file bytes). Returns
    /// `None` only for a non-resident file, which the engine's "live
    /// libraries only" contract excludes.
    fn convert(&self, diagnostic: &Diagnostic) -> Option<lsp::Diagnostic> {
        #[cfg(test)]
        crate::probe::count_conversion();
        let index = self.engine().line_index(diagnostic.file)?;
        let bytes = self.engine().content(diagnostic.file)?;
        let text = std::str::from_utf8(bytes).expect("engine content is valid UTF-8");
        // `line_col_utf16` answers the oracle's 1-based (line, column); LSP
        // positions are zero-based.
        let end_offset = diagnostic
            .range
            .offset
            .checked_add(diagnostic.range.length)
            .expect("diagnostic range overflows u32");
        let (start_line, start_column) = index.line_col_utf16(text, diagnostic.range.offset);
        let (end_line, end_column) = index.line_col_utf16(text, end_offset);
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
}

#[cfg(test)]
mod tests {
    use super::*;

    /// N applies inside the debounce window collapse into one save trigger;
    /// a second check after the flush does not re-trigger.
    #[test]
    fn debounce_collapses_applies_into_one_flush() {
        let mut sink = SnapshotSink {
            store: None,
            dirty: false,
            last_apply: None,
        };
        let applied = Instant::now();

        // Three applies across three turns, all inside the window; pin the
        // last apply to the test's base instant so the window math is exact
        // (note_apply stamps the real clock).
        sink.note_apply();
        sink.note_apply();
        sink.note_apply();
        sink.last_apply = Some(applied);
        assert!(sink.dirty);
        assert!(
            !sink.flush_if_due(applied + Duration::from_millis(499)),
            "inside the window nothing saves"
        );
        assert!(sink.dirty, "an unflushed sink stays dirty");

        // Past the window: exactly one save trigger, and the sink is clean.
        assert!(sink.flush_if_due(applied + SNAPSHOT_DEBOUNCE));
        assert!(!sink.dirty);
        assert!(
            !sink.flush_if_due(applied + Duration::from_secs(1)),
            "a clean sink never re-triggers"
        );
    }

    /// The end-of-session flush saves a dirty sink regardless of the window.
    #[test]
    fn flush_saves_dirty_sink_regardless_of_age() {
        let mut sink = SnapshotSink {
            store: None,
            dirty: false,
            last_apply: None,
        };
        sink.note_apply();
        // A fresh apply (0 ms old) still flushes at session end.
        sink.flush();
        assert!(!sink.dirty);
    }

    /// The deadline is the single source of truth for the window: clean
    /// sinks never bound the main loop's receive, dirty sinks do until the
    /// flush lands (#347: an idle daemon saves at the deadline instead of
    /// blocking forever with a pending save).
    #[test]
    fn flush_deadline_tracks_the_dirty_window() {
        let mut sink = SnapshotSink {
            store: None,
            dirty: false,
            last_apply: None,
        };
        assert!(
            sink.deadline().is_none(),
            "a clean sink never bounds the receive"
        );

        let applied = Instant::now();
        sink.note_apply();
        let deadline = sink.deadline().expect("a dirty sink bounds the receive");
        assert!(deadline > applied, "the deadline lies past the apply");
        assert!(
            deadline <= applied + SNAPSHOT_DEBOUNCE + Duration::from_millis(50),
            "the deadline is the apply instant plus the window"
        );

        sink.flush();
        assert!(
            sink.deadline().is_none(),
            "flushed: the receive blocks freely again"
        );
    }

    use crate::test_fixtures::{BROKEN_MAIN, CLEAN_MAIN, Workspace, store_root};
    use crate::commands::Request;
    use dartq_commands::Verbosity;
    use dartq_snapshot::DEFAULT_CAPACITY_BYTES;

    /// A session over a throwaway workspace with a throwaway snapshot root.
    fn fixture(name: &str) -> (Workspace, Session) {
        fixture_with(name, BROKEN_MAIN)
    }

    /// [`fixture`] over custom on-disk `lib/main.dart` content.
    fn fixture_with(name: &str, main_dart: &str) -> (Workspace, Session) {
        let workspace = Workspace::new(main_dart);
        let store = SnapshotStore::open(SnapshotConfig {
            root: Some(store_root(name)),
            capacity_bytes: DEFAULT_CAPACITY_BYTES,
        })
        .expect("store opens on a temp root");
        let session = Session::with_store(workspace.engine_config(), store).expect("session");
        (workspace, session)
    }

    /// Writer priority (PRD §16.5), deterministically: two `dartq/analyze`
    /// requests bracketing an overlay edit drain in one turn, and the
    /// request enqueued BEFORE the edit still answers with the post-write
    /// view — every write executes before any read.
    #[test]
    fn writer_priority_drains_writes_before_reads() {
        let (workspace, mut session) = fixture("writer-priority-unit");
        let uri = uri::from_path(&workspace.path("lib/main.dart"));

        session.sweep().expect("sweep");
        let turn = session.service();
        assert_eq!(
            turn.publishes.len(),
            1,
            "the sweep publishes the seeded error"
        );

        // One batch: analyze, edit, analyze — one drain serves all three.
        session.queue_command(
            RequestId::Num(2),
            Request::Analyze {
                verbosity: Verbosity::Compact,
            },
        );
        assert!(session.queue_edit(&uri, CLEAN_MAIN, 2));
        session.queue_command(
            RequestId::Num(3),
            Request::Analyze {
                verbosity: Verbosity::Compact,
            },
        );
        let turn = session.service();

        assert_eq!(turn.publishes.len(), 1, "the write's clear push");
        assert_eq!(turn.publishes[0].version, Some(2));
        assert!(turn.publishes[0].diagnostics.is_empty());
        assert_eq!(
            turn.answers.iter().map(|(id, _)| id).collect::<Vec<_>>(),
            [&RequestId::Num(2), &RequestId::Num(3)]
        );
        for (_, answer) in &turn.answers {
            let entries = answer
                .as_ref()
                .expect("both requests answer")["diagnostics"]
                .as_array()
                .expect("diagnostics array");
            assert!(entries.is_empty(), "post-write view: {entries:?}");
        }
    }

    /// Epoch cancellation (PRD §16.6), deterministically: a request
    /// superseded by `$/cancelRequest` drops at the drain and answers
    /// cancelled with no result, while the overlay edit sharing the batch
    /// still lands (writes carry no ids) — and the publish the cancelled
    /// read was carrying is re-stamped onto a zero-request read, so the
    /// clear push reaches the client in the same turn even if it goes
    /// silent afterwards.
    #[test]
    fn cancel_drops_queued_reads_but_writes_and_publishes_survive() {
        let (workspace, mut session) = fixture("cancel-unit");
        let uri = uri::from_path(&workspace.path("lib/main.dart"));

        session.sweep().expect("sweep");
        let _ = session.service();

        // Request + edit + cancel in one batch, then silence.
        session.queue_command(
            RequestId::Num(2),
            Request::Analyze {
                verbosity: Verbosity::Compact,
            },
        );
        assert!(session.queue_edit(&uri, CLEAN_MAIN, 2));
        session.cancel_request(&RequestId::Num(2));
        let turn = session.service();

        let [(id, answer)] = turn.answers.as_slice() else {
            panic!("one request, one answer: {:?}", turn.answers);
        };
        assert_eq!(*id, RequestId::Num(2));
        assert!(
            matches!(answer, Err(SessionError::Cancelled)),
            "the stale request answers cancelled: {answer:?}"
        );
        assert_eq!(
            turn.publishes.len(),
            1,
            "the write's clear push survives the cancel: {:?}",
            turn.publishes
        );
        assert_eq!(turn.publishes[0].version, Some(2));
        assert!(turn.publishes[0].diagnostics.is_empty());

        // The publish baseline caught up in that turn: a follow-up analysis
        // (had the client not stayed silent) pushes nothing new.
        session.queue_command(
            RequestId::Num(3),
            Request::Analyze {
                verbosity: Verbosity::Compact,
            },
        );
        let turn = session.service();
        assert!(
            turn.publishes.is_empty(),
            "no duplicate push: {:?}",
            turn.publishes
        );
        let entries = turn.answers[0]
            .1
            .as_ref()
            .expect("post-write answer")["diagnostics"]
            .as_array()
            .expect("diagnostics array");
        assert!(entries.is_empty(), "post-write view: {entries:?}");
    }

    /// Cancel partitions the coalesced read per id: the cancelled request
    /// answers cancelled, its co-rider survives on the re-stamped read and
    /// answers with the real analysis.
    #[test]
    fn cancel_partitions_the_coalesced_read() {
        let (_workspace, mut session) = fixture("cancel-partition-unit");

        session.sweep().expect("sweep");
        let _ = session.service();

        // Two analyses coalesce onto one read; cancelling one spares the
        // other.
        session.queue_command(
            RequestId::Num(2),
            Request::Analyze {
                verbosity: Verbosity::Compact,
            },
        );
        session.queue_command(
            RequestId::Num(3),
            Request::Analyze {
                verbosity: Verbosity::Compact,
            },
        );
        session.cancel_request(&RequestId::Num(2));
        let turn = session.service();

        assert!(
            turn.publishes.is_empty(),
            "nothing changed since the sweep: {:?}",
            turn.publishes
        );
        assert_eq!(
            turn.answers.iter().map(|(id, _)| id).collect::<Vec<_>>(),
            [&RequestId::Num(2), &RequestId::Num(3)],
            "cancelled answers first, survivor after"
        );
        assert!(
            matches!(&turn.answers[0].1, Err(SessionError::Cancelled)),
            "the cancelled id: {:?}",
            turn.answers[0]
        );
        let entries = turn.answers[1]
            .1
            .as_ref()
            .expect("survivor answers Ok")["diagnostics"]
            .as_array()
            .expect("diagnostics array");
        assert!(
            entries
                .iter()
                .any(|entry| entry["code"] == "uri_does_not_exist"),
            "the survivor got the real analysis: {entries:?}"
        );
    }

    /// Cancelling an unknown id, or one that already answered, is silently
    /// ignored (LSP §cancelRequest): no cancelled answer, no state change.
    #[test]
    fn cancel_of_unknown_or_answered_id_is_ignored() {
        let (_workspace, mut session) = fixture("cancel-ignore-unit");

        session.sweep().expect("sweep");
        let _ = session.service();

        // Unknown id while a read is queued: the queued request still
        // answers normally.
        session.queue_command(
            RequestId::Num(2),
            Request::Analyze {
                verbosity: Verbosity::Compact,
            },
        );
        session.cancel_request(&RequestId::Num(99));
        let turn = session.service();
        assert_eq!(
            turn.answers.len(),
            1,
            "only the queued request: {:?}",
            turn.answers
        );
        assert!(turn.answers[0].1.is_ok(), "the cancel never landed");

        // An id that already answered: the read drained; the cancel is a
        // no-op and the next analysis still answers.
        session.cancel_request(&RequestId::Num(2));
        session.queue_command(
            RequestId::Num(3),
            Request::Analyze {
                verbosity: Verbosity::Compact,
            },
        );
        let turn = session.service();
        assert_eq!(
            turn.answers.len(),
            1,
            "only the new request: {:?}",
            turn.answers
        );
        assert_eq!(turn.answers[0].0, RequestId::Num(3));
        assert!(turn.answers[0].1.is_ok());
    }

    /// Staged edits merge last-wins per path (#347): three batched
    /// `didChange` frames for one file stage ONE whole-text op, not three —
    /// and the batch publishes exactly what a single edit would (the
    /// terminal state is order-independent for distinct paths, so the
    /// earlier same-path ops are unobservable).
    #[test]
    fn staged_edits_of_one_file_merge_last_wins() {
        let (workspace, mut session) = fixture("staged-merge-unit");
        let uri = uri::from_path(&workspace.path("lib/main.dart"));

        session.sweep().expect("sweep");
        let _ = session.service();

        // Three didChange frames land inside one batch window: three
        // queue_edit calls before the drain.
        assert!(session.queue_edit(&uri, "}\n", 2));
        assert!(session.queue_edit(&uri, ")\n", 3));
        assert!(session.queue_edit(&uri, CLEAN_MAIN, 4));
        assert_eq!(
            session.staged.len(),
            1,
            "same-path ops converge to the last text"
        );

        let turn = session.service();
        assert_eq!(
            turn.publishes.len(),
            1,
            "one batch, one push — as if a single clean edit arrived: {:?}",
            turn.publishes
        );
        assert_eq!(turn.publishes[0].version, Some(4));
        assert!(turn.publishes[0].diagnostics.is_empty());

        // The survivor is the LAST text: an earlier broken one winning
        // would surface syntax diagnostics here.
        session.queue_command(
            RequestId::Num(2),
            Request::Analyze {
                verbosity: Verbosity::Compact,
            },
        );
        let turn = session.service();
        let entries = turn.answers[0]
            .1
            .as_ref()
            .expect("post-batch answer")["diagnostics"]
            .as_array()
            .expect("diagnostics array");
        assert!(
            entries.is_empty(),
            "the last edit is the applied one: {entries:?}"
        );
    }

    /// The read task converts each diagnostic exactly once (#347): the
    /// publish plan and the analyze entries share one conversion pass (the
    /// pre-#347 shape converted twice), and conversion borrows the resident
    /// text — a per-diagnostic whole-file clone would register on the bulk
    /// allocation probe.
    #[test]
    fn read_turn_converts_once_and_never_clones_file_text() {
        // An 8 KiB broken file: the whole text is bulk-sized, so any
        // per-diagnostic copy would show up in the probe.
        let big_broken = format!("{BROKEN_MAIN}// {}\n", "x".repeat(8 * 1024));
        let (_workspace, mut session) = fixture_with("convert-probe-unit", &big_broken);

        session.sweep().expect("sweep");
        let _ = session.service();

        crate::probe::reset_counters();
        session.queue_command(
            RequestId::Num(2),
            Request::Analyze {
                verbosity: Verbosity::Compact,
            },
        );
        let turn = session.service();
        let entries = turn.answers[0]
            .1
            .as_ref()
            .expect("analyze answers")["diagnostics"]
            .as_array()
            .expect("diagnostics array");
        assert!(
            !entries.is_empty(),
            "the fixture must carry diagnostics for the probe"
        );

        let (conversions, _) = crate::probe::counters();
        assert_eq!(
            conversions,
            entries.len(),
            "one conversion per diagnostic — not one for the publish plan plus one for the \
             entries"
        );

        // Direct conversion allocates nothing bulk-sized: the line index
        // and the text are borrowed, never owned per diagnostic.
        let diagnostics = session.analyze().expect("re-analysis");
        crate::probe::arm_bulk();
        for diagnostic in &diagnostics {
            assert!(session.convert(diagnostic).is_some());
        }
        assert_eq!(
            crate::probe::bulk_bytes(),
            0,
            "convert borrows the resident text; a whole-file clone would register"
        );
    }

    /// A `dartq/check` request rides the same read task and epoch machinery
    /// as `dartq/analyze`: a stale epoch aborts at the drain, before the
    /// command executes — the request answers cancelled with no result
    /// (PRD §15.1: every `dartq/*` face is cancellable).
    #[test]
    fn cancel_drops_a_queued_dartq_check_request() {
        let (workspace, mut session) = fixture("cancel-command-unit");

        session.sweep().expect("sweep");
        let _ = session.service();

        session.queue_command(
            RequestId::Num(2),
            Request::Check {
                paths: vec![workspace.path("lib")],
                verbosity: Verbosity::Compact,
            },
        );
        session.cancel_request(&RequestId::Num(2));
        let turn = session.service();

        assert!(
            turn.publishes.is_empty(),
            "nothing changed since the sweep: {:?}",
            turn.publishes
        );
        let [(id, answer)] = turn.answers.as_slice() else {
            panic!("one request, one answer: {:?}", turn.answers);
        };
        assert_eq!(*id, RequestId::Num(2));
        assert!(
            matches!(answer, Err(SessionError::Cancelled)),
            "the stale dartq/check answers cancelled: {answer:?}"
        );
    }

    /// A `dartq/*` request whose parameters name no valid target answers
    /// `InvalidParams` (the CLI's usage tier over RPC), and the session
    /// stays healthy — a follow-up request answers normally.
    #[test]
    fn dartq_command_usage_failure_answers_invalid_params() {
        let (workspace, mut session) = fixture("command-usage-unit");

        session.sweep().expect("sweep");
        let _ = session.service();

        session.queue_command(
            RequestId::Num(2),
            Request::Check {
                paths: vec![workspace.path("lib/nope.dart")],
                verbosity: Verbosity::Compact,
            },
        );
        let turn = session.service();
        let [(id, answer)] = turn.answers.as_slice() else {
            panic!("one request, one answer: {:?}", turn.answers);
        };
        assert_eq!(*id, RequestId::Num(2));
        assert!(
            matches!(answer, Err(SessionError::InvalidParams(_))),
            "a missing target is the usage tier: {answer:?}"
        );

        // The session is unharmed: the next command answers normally.
        session.queue_command(
            RequestId::Num(3),
            Request::Check {
                paths: vec![workspace.path("lib")],
                verbosity: Verbosity::Compact,
            },
        );
        let turn = session.service();
        assert!(
            turn.answers[0].1.is_ok(),
            "the session answers after a usage-tier failure: {:?}",
            turn.answers
        );
    }
}
