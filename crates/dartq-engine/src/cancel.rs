//! Cooperative cancellation.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Why an [`Engine::analyze`](crate::Engine::analyze) run stopped early.
///
/// Carries no partial diagnostics: libraries that finished their recompute
/// before the checkpoint keep their cached results (a complete unit per the
/// per-library cache), everything else stays dirty and is recomputed by
/// the next `analyze`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cancelled;

/// Checkpoint token shared between the canceller and the pipeline.
///
/// Checkpoints fire at **library granularity only** — before each library
/// recompute in `pipeline.rs`; scan/parse are single passes with no
/// checkpoint, matching checkpoint density to work granularity.
/// No async runtime, no watcher thread: the M1 CLI constructs a
/// never-cancelled token; the type exists for tests and the future daemon.
#[derive(Clone, Default)]
pub struct CancellationToken(Arc<AtomicBool>);

impl CancellationToken {
    /// A token that never cancels unless [`cancel`](Self::cancel) is called.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Flags the token; the next checkpoint stops the run.
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    /// Whether cancellation was requested.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }

    /// The pipeline-side checkpoint: `Err` when the run should stop.
    pub(crate) fn check(&self) -> Result<(), Cancelled> {
        if self.is_cancelled() {
            Err(Cancelled)
        } else {
            Ok(())
        }
    }
}
