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
//! rides on, `dartq/analyze` attaches to (or enqueues) one read task,
//! stamped with the context's epoch at enqueue. Execution
//! happens at the single [`Session::service`] drain point, in the
//! scheduler's writer-priority order — every staged write applies before any
//! read analyzes, so N batched edits collapse into one apply + one analysis,
//! and a read enqueued before a write still answers with the post-write
//! view. A task superseded by a [`Runtime::cancel`] drops at the drain: its
//! requests answer `RequestCancelled` and produce nothing.
//!
//! # Snapshot persistence (#306)
//!
//! The sink is debounced by a last-apply timestamp checked at turn
//! boundaries (the stdio loop is turn-based — the debounce window elapses
//! across the blocking receive, so no timer thread exists) and flushed
//! regardless of the window at session end. Save failures log and continue:
//! the accelerator invariant makes an unwritten generation a future miss,
//! never an error.

use std::collections::hash_map::Entry;
use std::fs;
use std::io;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use dartq_diagnostics::{Diagnostic, Severity};
use dartq_engine::{
    CancellationToken, Cancelled, ContextConfig, ContextId, Engine, EngineConfig, EngineError,
    OverlayChange, Runtime,
};
use rustc_hash::FxHashMap;
use thiserror::Error;

use dartq_snapshot::{SnapshotConfig, SnapshotStore};

use crate::jsonrpc::RequestId;
use crate::lsp;
use crate::uri;

/// The debounce window between the last overlay apply and the snapshot
/// generation save (#306): 500 ms, elapsed across the main loop's blocking
/// receive — the stdio loop is turn-based, so no timer thread exists.
const SNAPSHOT_DEBOUNCE: Duration = Duration::from_millis(500);

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
    /// stale at the drain (cancelled) and produced nothing.
    pub answers: Vec<(RequestId, Result<Vec<lsp::AnalyzeEntry>, SessionError>)>,
}

/// A queued read task and the requests waiting on its answer. Reads coalesce:
/// one analysis serves every attached request plus the publish diff.
struct PendingRead {
    requests: Vec<RequestId>,
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

    /// Saves when the debounce window has elapsed since the last apply;
    /// true when a save was attempted.
    fn flush_if_due(&mut self, now: Instant) -> bool {
        let settled = self.dirty
            && self
                .last_apply
                .is_some_and(|at| now.duration_since(at) >= SNAPSHOT_DEBOUNCE);
        if !settled {
            return false;
        }
        self.flush();
        true
    }

    fn flush(&mut self) {
        self.dirty = false;
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
        let context = runtime.open_context(ContextConfig {
            engine: config,
            flags: 0,
        })?;
        Ok(Self {
            runtime,
            context,
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
        self.staged.push(OverlayChange::Set {
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
    /// # Errors
    ///
    /// Disk read failure.
    pub fn queue_close(&mut self, uri: &str) -> io::Result<bool> {
        let Some(path) = uri::to_path(uri) else {
            return Ok(false);
        };
        if self.opened.remove(&path).is_none() {
            return Ok(false);
        }
        match fs::read(&path) {
            Ok(bytes) => self.staged.push(OverlayChange::Set { path, bytes }),
            // Overlay-only buffer: dropping the overlay removes the file.
            // `UnknownPath` cannot happen — `open` interned the path.
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                self.staged.push(OverlayChange::Remove { path });
            }
            Err(error) => return Err(error),
        }
        self.ensure_write();
        self.ensure_read();
        Ok(true)
    }

    /// `dartq/analyze`: attaches the request to the queued read task (or
    /// queues one), stamped with the context's epoch at enqueue. Answered at
    /// the drain, in drain order.
    pub fn queue_analyze(&mut self, id: RequestId) {
        self.ensure_read();
        if let Some(pending) = self.read.as_mut() {
            pending.requests.push(id);
        }
    }

    /// `$/cancelRequest`: bumps the context's epoch, so every task enqueued
    /// so far drops at the drain and its requests answer `RequestCancelled`.
    /// Cancelling an unknown or already-answered id is ignored (LSP
    /// §cancelRequest); the granularity is the context's pending turn work.
    ///
    /// The staged overlay edits are *not* cancellable (LSP notifications
    /// carry no ids): the write task re-stamps at the fresh epoch, so a read
    /// enqueued after the cancel still drains behind the writes.
    ///
    /// # Panics
    ///
    /// Never in practice: the re-stamp enqueues against the root context,
    /// which lives for the session.
    pub fn cancel_request(&mut self, id: &RequestId) {
        let Some(pending) = &self.read else {
            return;
        };
        if !pending.requests.contains(id) {
            return;
        }
        let _ = self.runtime.cancel(self.context);
        if let Some(pending) = self.read.take() {
            self.cancelled.extend(pending.requests);
        }
        if let Some(cookie) = self.write.take() {
            self.runtime
                .enqueue(self.context, true, cookie)
                .expect("the root context lives for the session; write re-stamp cannot fail");
            self.write = Some(cookie);
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

    /// Turn-start debounce check; true when a save was attempted. The window
    /// elapses across the main loop's blocking receive, so this runs once
    /// per turn with the current instant.
    #[must_use]
    pub fn flush_if_due(&mut self, now: Instant) -> bool {
        self.snapshot.flush_if_due(now)
    }

    /// One read task's execution: a fresh full analysis — publishes diffed
    /// against the baseline, entries cloned for every attached request.
    fn read_turn(&mut self, turn: &mut Turn) {
        let pending = self
            .read
            .take()
            .expect("a drain-live read task is still pending");
        match self.analyze() {
            Ok(diagnostics) => {
                turn.publishes.extend(self.plan_publishes(&diagnostics));
                let entries: Vec<lsp::AnalyzeEntry> = diagnostics
                    .iter()
                    .filter_map(|diagnostic| {
                        let converted = self.convert(diagnostic)?;
                        let path = self.engine().file_table().path(diagnostic.file)?;
                        Some(lsp::AnalyzeEntry {
                            uri: uri::from_path(path),
                            diagnostic: converted,
                        })
                    })
                    .collect();
                for id in pending.requests {
                    turn.answers.push((id, Ok(entries.clone())));
                }
            }
            // The analysis token is the engine's own cancellation
            // checkpoint; stale work never surfaces as a partial answer.
            Err(Cancelled) => {
                for id in pending.requests {
                    turn.answers.push((id, Err(SessionError::Cancelled)));
                }
            }
        }
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

    /// Converts one engine diagnostic to the LSP shape. Returns `None` only
    /// for a non-resident file, which the engine's "live libraries only"
    /// contract excludes.
    fn convert(&self, diagnostic: &Diagnostic) -> Option<lsp::Diagnostic> {
        let index = self.engine().line_index(diagnostic.file)?;
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
        let bytes = self.engine().content(file)?;
        Some(
            std::str::from_utf8(bytes)
                .expect("engine content is valid UTF-8")
                .to_owned(),
        )
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

    use crate::test_fixtures::{BROKEN_MAIN, CLEAN_MAIN, Workspace, store_root};
    use dartq_snapshot::DEFAULT_CAPACITY_BYTES;

    /// A session over a throwaway workspace with a throwaway snapshot root.
    fn fixture(name: &str) -> (Workspace, Session) {
        let workspace = Workspace::new(BROKEN_MAIN);
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
        session.queue_analyze(RequestId::Num(2));
        assert!(session.queue_edit(&uri, CLEAN_MAIN, 2));
        session.queue_analyze(RequestId::Num(3));
        let turn = session.service();

        assert_eq!(turn.publishes.len(), 1, "the write's clear push");
        assert_eq!(turn.publishes[0].version, Some(2));
        assert!(turn.publishes[0].diagnostics.is_empty());
        assert_eq!(
            turn.answers.iter().map(|(id, _)| id).collect::<Vec<_>>(),
            [&RequestId::Num(2), &RequestId::Num(3)]
        );
        for (_, answer) in &turn.answers {
            let entries = answer.as_ref().expect("both requests answer");
            assert!(entries.is_empty(), "post-write view: {entries:?}");
        }
    }

    /// Epoch cancellation (PRD §16.6), deterministically: a request
    /// superseded by `$/cancelRequest` drops at the drain and answers
    /// cancelled with no result, while the overlay edit sharing the batch
    /// still lands (writes carry no ids) — the follow-up read publishes it
    /// and answers the post-write view.
    #[test]
    fn cancel_drops_queued_reads_but_writes_survive() {
        let (workspace, mut session) = fixture("cancel-unit");
        let uri = uri::from_path(&workspace.path("lib/main.dart"));

        session.sweep().expect("sweep");
        let _ = session.service();

        // Request + edit + cancel in one batch.
        session.queue_analyze(RequestId::Num(2));
        assert!(session.queue_edit(&uri, CLEAN_MAIN, 2));
        session.cancel_request(&RequestId::Num(2));
        let turn = session.service();
        assert!(
            turn.publishes.is_empty(),
            "no read survived the cancel: {:?}",
            turn.publishes
        );
        let [(id, answer)] = turn.answers.as_slice() else {
            panic!("one request, one answer: {:?}", turn.answers);
        };
        assert_eq!(*id, RequestId::Num(2));
        assert!(
            matches!(answer, Err(SessionError::Cancelled)),
            "the stale request answers cancelled: {answer:?}"
        );

        // The write survived: the follow-up read publishes the clear and
        // answers the post-write view.
        session.queue_analyze(RequestId::Num(3));
        let turn = session.service();
        assert_eq!(turn.publishes.len(), 1, "the surviving write's clear push");
        assert_eq!(turn.publishes[0].version, Some(2));
        assert!(turn.publishes[0].diagnostics.is_empty());
        let entries = turn.answers[0].1.as_ref().expect("post-write answer");
        assert!(entries.is_empty(), "post-write view: {entries:?}");
    }
}
