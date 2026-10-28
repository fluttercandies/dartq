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
/// Checkpoints fire at library granularity — before each library recompute
/// in `pipeline.rs` — and inside a library's recompute at member-file
/// cadence through [`Checkpoint`], so a flip mid-library lands within a
/// bounded window instead of waiting out the whole recompute.
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

    /// A token sharing a caller-owned cell: the caller keeps the
    /// `Arc<AtomicBool>` — e.g. a session serve loop's signal-hook
    /// flag — so one atomic carries both facts: the write cancels the
    /// engine run at its next checkpoint AND lands in the caller's own
    /// poll, with no watcher thread between them.
    #[must_use]
    pub fn from_flag(flag: Arc<AtomicBool>) -> Self {
        Self(flag)
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

/// A cadence counter for inner loops: [`tick`](Self::tick) polls the
/// token once every `STRIDE` iterations, so a hot loop pays one relaxed
/// atomic load per stride — not per item — and a flipped token surfaces
/// within `STRIDE` iterations of work (the bounded-window contract:
/// at member-file cadence a cancel lands inside one library's recompute
/// rather than waiting for its whole recompute).
pub(crate) struct Checkpoint<'a> {
    cancel: &'a CancellationToken,
    countdown: u32,
}

impl<'a> Checkpoint<'a> {
    /// Iterations between token polls. Member-file folds run
    /// millisecond-scale work, so 8 keeps a flip's continued work well
    /// under the release window while the per-iteration cost stays a
    /// decrement and a branch.
    const STRIDE: u32 = 8;

    /// A cadence counter over `cancel`; the first poll fires on the
    /// `STRIDE`-th tick.
    pub(crate) fn new(cancel: &'a CancellationToken) -> Self {
        Self {
            cancel,
            countdown: Self::STRIDE,
        }
    }

    /// One loop iteration; `Err` once the token polls flipped.
    #[inline]
    pub(crate) fn tick(&mut self) -> Result<(), Cancelled> {
        self.countdown -= 1;
        if self.countdown == 0 {
            self.countdown = Self::STRIDE;
            self.cancel.check()?;
        }
        Ok(())
    }
}
