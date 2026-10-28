//! The `Tier::Background` producer: post-analyze cache warming.
//!
//! rust-analyzer's `parallel_prime_caches` posture: after an analyze
//! settles, the engine holds lazily-populated faces a later query or
//! rebuild would pay serially on the critical path. The warm lane pays
//! them at background priority instead, in bounded chunks that yield to
//! every foreground task. The engine core is synchronous — there is no
//! warm thread here; the lane is a resumable plan the caller (the daemon's
//! scheduler drain) steps at its lowest tier, one bounded chunk per step.
//!
//! # Warm targets
//!
//! * **Reachability memo** — [`crate::reachability::build`], the
//!   whole-graph fixpoint a cold `Engine::reachability` read (or the
//!   `unreachable_from_main` lint mount) pays in one serialized build.
//!   Warming stores the same `Arc` the lazy read would produce.
//! * **Scanned faces** — residents with no persisted `ScannedFace` in the
//!   scanned cache ([`EngineState::settled_face`] misses: warm-start hits
//!   and facts-cache hits never ran `scan_unit`). A later surface rebuild
//!   on a site-cache miss or a clause-tree capture re-derives the face
//!   per file on demand; the warm lane fills the same cache entries a
//!   miss would, through the identical `l1::scan_unit` fallback.
//!
//! Both targets are pure functions of the settled state, so warming is
//! output-neutral: every consumer reads the same product whether the warm
//! lane or the miss path filled it.
//!
//! # Mechanics
//!
//! * **Priority** — the caller drains warm steps at `Tier::Background`;
//!   the lane itself never preempts. Each [`Engine::warm_step`] is bounded
//!   ([`WARM_STEP_FILES`] face items, or the single reachability build),
//!   so the drain between steps is the foreground re-check.
//! * **Cancellation** — a [`Checkpoint`] strides the face items and the
//!   reachability build carries the token inside; a flip stops the step
//!   mid-plan and the remainder stays armed for a later step.
//! * **Budget** — an armed memory budget that already descended a tier
//!   ([`budget::MemoryBudget::descended`]) pauses the lane: the plan
//!   drops, its unworked items count skipped, and the next settle re-arms.
//!   Growing caches under a degraded ladder is exactly the pressure the
//!   ladder exists to shed.
//! * **Observability** — [`Engine::warm_stats`] counts completed and
//!   skipped items, cumulative per engine.

use std::sync::PoisonError;

use dartq_source::FileId;

use crate::cancel::{CancellationToken, Checkpoint};
use crate::state::{EngineState, ScannedFace};
use crate::{Cancelled, l1};

/// The per-step bound on face-warming items: one scan unit per file is
/// millisecond-scale, so 256 keeps a step inside the release window a
/// foreground drain expects while still converging a restart-scale miss
/// set in a handful of idle ticks. The reachability build is its own
/// item and steps alone.
const WARM_STEP_FILES: usize = 256;

/// The warm lane's plan: what a settle armed and how far the steps got.
/// Fixed-size between materializations; `faces` materializes once per arm
/// (the sorted resident-id snapshot the cursor walks).
#[derive(Debug, Default)]
pub(crate) struct WarmPlan {
    /// A settle armed the lane. The flag is the O(1) `warm_pending`
    /// answer; the plan's items may still turn out empty at the step
    /// (a clean settle arms with nothing to do, which the first step
    /// finds and disarms).
    armed: bool,
    /// The reachability item: armed at settle when the memo needs a
    /// build — the same predicate the lazy read faces.
    reachability: bool,
    /// The face pass, materialized at the first step: resident files,
    /// sorted for determinism, that missed `settled_face` at arm time.
    /// `None` until the first step; a present vec is the cursor's
    /// remaining tail (`faces[cursor..]`).
    faces: Option<Vec<FileId>>,
    /// The walk cursor into `faces`.
    cursor: usize,
    /// Cumulative observability counters.
    completed: u64,
    skipped: u64,
}

/// What one [`Engine::warm_step`] did — the caller's re-queue decision.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WarmStep {
    /// The plan is empty (or was never armed): nothing stays queued.
    Done,
    /// Work remains armed: the caller re-queues at its background tier.
    More,
    /// The budget ladder already descended: the plan dropped (its
    /// unworked items counted skipped) and the lane yields until the
    /// next settle.
    Paused,
    /// The cancellation token fired mid-step: the remaining plan stays
    /// armed for a later step.
    Cancelled,
}

/// The cumulative warm counters ([`Engine::warm_stats`]): items the lane
/// produced (a cached face, the reachability map) and items it visited
/// without work — no longer resident, already warm, or dropped by a
/// budget pause.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WarmStats {
    /// Items the lane produced.
    pub completed: u64,
    /// Items visited without work (see the struct docs).
    pub skipped: u64,
}

impl EngineState {
    /// Arms the warm lane at a successful analyze's settle — the single
    /// arming site, reached by both output faces. Re-arming replaces any
    /// unconsumed plan: the snapshot is re-materialized at the next step
    /// (the previous cursor's residents may have left anyway, so a stale
    /// tail buys nothing). O(1): the face materialization defers to the
    /// step.
    pub(crate) fn arm_warm(&mut self) {
        // The reachability item rides the memo's own predicate: a build
        // helps only while the lazy read would pay one — armed, coherent
        // (no in-flight apply), and stale-or-never-built. The read's
        // standing-map arm never builds, so neither does the warm lane.
        let reachability = {
            let memo = self
                .reachability
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            memo.armed && !memo.applied && (memo.stale || memo.map.is_none())
        };
        self.warm.armed = true;
        self.warm.reachability = reachability;
        // A fresh arm re-materializes the face snapshot at the next
        // step; the previous cursor's residents may have left anyway.
        self.warm.faces = None;
        self.warm.cursor = 0;
        // `completed`/`skipped` are cumulative per engine: untouched.
    }

    /// Whether the warm lane is armed — the O(1) `Engine::warm_pending`
    /// answer. The flag is optimistic: a settle can arm a plan whose
    /// items all turn out warm already (the first step finds them
    /// skipped and disarms).
    pub(crate) fn warm_pending(&self) -> bool {
        self.warm.armed
    }

    /// The cumulative counters ([`WarmStats`], cumulative per engine).
    pub(crate) fn warm_stats(&self) -> WarmStats {
        WarmStats {
            completed: self.warm.completed,
            skipped: self.warm.skipped,
        }
    }

    /// One bounded warm chunk — see the module docs for the mechanics.
    /// The step is `&mut` only for the cache stores it fills; the work
    /// itself reads settled state exactly as the miss paths do.
    pub(crate) fn warm_step(&mut self, cancel: &CancellationToken) -> WarmStep {
        if !self.warm.armed {
            return WarmStep::Done;
        }
        // The budget pause: a descended ladder means the caches this
        // lane would grow are exactly what the budget is shedding — the
        // plan drops wholesale (its unworked items count skipped) and
        // the next settle re-arms. Probed before any work: `descended`
        // is read-only, so an under-limit armed budget pauses nothing.
        if self
            .budget
            .as_ref()
            .is_some_and(crate::budget::MemoryBudget::descended)
        {
            let remaining = self.warm.faces.as_ref().map_or(0, |faces| {
                u64::try_from(faces.len().saturating_sub(self.warm.cursor)).unwrap_or(u64::MAX)
            }) + u64::from(self.warm.reachability);
            self.warm.skipped += remaining;
            self.warm.armed = false;
            return WarmStep::Paused;
        }
        if cancel.check().is_err() {
            return WarmStep::Cancelled;
        }
        // The reachability item leads: one build, the whole cold-query
        // cost. The memo predicate re-checks at step time — an apply or
        // a serving read since the arm makes the item a skip (the read
        // already owns the map, or the build's coherence precondition
        // is gone until the next gate).
        if self.warm.reachability {
            self.warm.reachability = false;
            let mut memo = self
                .reachability
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            if memo.armed && !memo.applied && (memo.stale || memo.map.is_none()) {
                let prev = memo.map.take();
                match crate::reachability::build(self, prev, cancel) {
                    Ok(map) => {
                        memo.map = Some(map);
                        memo.stale = false;
                        self.warm.completed += 1;
                    }
                    Err(Cancelled) => {
                        // The build published nothing: the memo stays
                        // stale, so the next step — or the lazy read —
                        // retries the same product. Nothing consumed,
                        // nothing skipped.
                        return WarmStep::Cancelled;
                    }
                }
            } else {
                self.warm.skipped += 1;
            }
        }
        // The face pass: materialize the arm's snapshot on the first
        // step — the resident files missing a persisted scanned face,
        // sorted for a deterministic cursor. Files arriving later join
        // the NEXT arm's snapshot; files leaving since just skip.
        if self.warm.faces.is_none() {
            let mut missing: Vec<FileId> = self
                .residency
                .keys()
                .copied()
                .filter(|&file| self.settled_face(file).is_none())
                .collect();
            missing.sort_unstable();
            self.warm.faces = Some(missing);
        }
        let total = self.warm.faces.as_ref().expect("materialized above").len();
        let mut checkpoint = Checkpoint::new(cancel);
        let mut worked = 0usize;
        while self.warm.cursor < total {
            if checkpoint.tick().is_err() {
                // The remainder stays in the cursor: a later step
                // resumes exactly here, counting nothing it never
                // reached.
                return WarmStep::Cancelled;
            }
            if worked >= WARM_STEP_FILES {
                return WarmStep::More;
            }
            let file = self.warm.faces.as_ref().expect("materialized above")[self.warm.cursor];
            self.warm.cursor += 1;
            worked += 1;
            // Re-check at item time: a face filled since the arm (a
            // drain_captures pass, a consumer's own fallback) makes the
            // item a skip, as does a file that left residency.
            let Some(facts) = self.residency.get(&file) else {
                self.warm.skipped += 1;
                continue;
            };
            let key = l1::facts_key(&self.grants(), file, facts.content);
            if self.cached_scanned(key).is_some() {
                self.warm.skipped += 1;
                continue;
            }
            let Some(bytes) = self.store.get(facts.content) else {
                self.warm.skipped += 1;
                continue;
            };
            let Ok(text) = std::str::from_utf8(bytes) else {
                self.warm.skipped += 1;
                continue;
            };
            let version = l1::language_version_for(&self.packages, &self.files, file);
            let (feature_set, settled, scanned, _) =
                l1::scan_unit(text, version, file, &self.grants());
            self.cache_scanned(
                key,
                ScannedFace {
                    tokens: scanned.tokens,
                    features: feature_set,
                    version: settled,
                },
            );
            self.warm.completed += 1;
        }
        // The cursor drained: the plan is consumed and the lane disarms
        // until the next settle.
        self.warm.armed = false;
        WarmStep::Done
    }
}
