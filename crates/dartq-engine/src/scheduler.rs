//! Per-context task queues with writer-priority drain and epoch-based
//! staleness (PRD §16.5, §16.6).
//!
//! The scheduler is pure data plumbing: tasks are ordered records, not
//! closures. The daemon/server owns execution and threading (runtime policy:
//! synchronous core); this module only guarantees the two properties the
//! substrate needs — a deterministic writer-priority drain order, and stale
//! tasks (enqueued under an epoch a later write superseded) dropped at the
//! single drain point so cancelled work never produces results.

use std::collections::VecDeque;

use crate::context::ContextId;

/// One unit of scheduled work (PRD §16.5). A plain record: `cookie` names the
/// job in the caller's own vocabulary, everything else is scheduling data.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Task {
    /// The context the task analyzes against.
    pub context: ContextId,
    /// The context epoch at enqueue time; a task whose epoch no longer
    /// matches is stale and dropped at drain (PRD §16.6).
    pub epoch: u64,
    /// Whether the task writes the context's view (`true`: overlay apply —
    /// every pending write drains before any read) or reads it.
    pub write: bool,
    /// Opaque caller cookie naming the job (sequence number, request id…).
    pub cookie: u64,
}

/// What one drain produced.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct DrainReport {
    /// Live tasks in execution order: every pending write first (context
    /// ascending, arrival order within a context), then every read (same
    /// order) — the writer-priority pass of PRD §16.5, deterministic.
    pub tasks: Vec<Task>,
    /// Stale tasks dropped at the drain point (PRD §16.6): their epoch was
    /// superseded by a later write into their context, so executing them
    /// would analyze a view that no longer exists.
    pub stale_dropped: usize,
}

/// One queue per context, drained writer-first. Queue depth is bounded by
/// the caller's enqueue discipline: entries live only between `enqueue` and
/// the next `drain`/`drop_context`, exactly like the engine's pending
/// overlay between `set_content` and `apply_pending`.
#[derive(Debug, Default)]
pub(crate) struct Scheduler {
    queues: Vec<VecDeque<Task>>,
}

impl Scheduler {
    /// The queue of `id`, grown on demand (arena slots are stable, so a
    /// queue once created is only ever cleared, never moved).
    fn queue(&mut self, id: ContextId) -> &mut VecDeque<Task> {
        let index = id.index() as usize;
        if self.queues.len() <= index {
            self.queues.resize(index + 1, VecDeque::new());
        }
        &mut self.queues[index]
    }

    /// Enqueues `task` at the tail of its context's queue.
    pub(crate) fn enqueue(&mut self, task: Task) {
        self.queue(task.context).push_back(task);
    }

    /// Clears the queue of a closed context (its tasks can never run).
    pub(crate) fn drop_context(&mut self, id: ContextId) {
        if let Some(queue) = self.queues.get_mut(id.index() as usize) {
            queue.clear();
        }
    }

    /// The single drain point: pops every queued task, drops the stale ones,
    /// and returns the live set in writer-priority order.
    ///
    /// `epoch_of` maps a context to its current epoch. Invariant: every
    /// queued task references a live context (enqueue checks liveness and
    /// `drop_context` clears the queue), so the caller's lookup cannot miss.
    pub(crate) fn drain(&mut self, epoch_of: &dyn Fn(ContextId) -> u64) -> DrainReport {
        // Collection order across queues is context-ascending, and `append`
        // keeps arrival order within a queue — the drain's deterministic
        // tiebreak.
        let mut all = Vec::new();
        for queue in &mut self.queues {
            all.extend(queue.drain(..));
        }
        let mut writes = Vec::new();
        let mut reads = Vec::new();
        let mut stale_dropped = 0;
        for task in all {
            if epoch_of(task.context) != task.epoch {
                stale_dropped += 1;
            } else if task.write {
                writes.push(task);
            } else {
                reads.push(task);
            }
        }
        writes.extend(reads);
        DrainReport {
            tasks: writes,
            stale_dropped,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(context: u32, write: bool, cookie: u64) -> Task {
        Task {
            context: ContextId::from_index(context),
            epoch: 0,
            write,
            cookie,
        }
    }

    #[test]
    fn drain_orders_all_writes_before_all_reads() {
        let mut scheduler = Scheduler::default();
        scheduler.enqueue(task(0, false, 101));
        scheduler.enqueue(task(0, true, 102));
        scheduler.enqueue(task(0, false, 103));
        scheduler.enqueue(task(1, false, 201));
        scheduler.enqueue(task(1, true, 202));

        let report = scheduler.drain(&|id| match id.index() {
            0 | 1 => 0,
            _ => unreachable!("no other context enqueued"),
        });

        assert_eq!(report.stale_dropped, 0);
        let order: Vec<(u32, bool, u64)> = report
            .tasks
            .iter()
            .map(|task| (task.context.index(), task.write, task.cookie))
            .collect();
        // Writes first (context ascending, arrival order), then reads.
        assert_eq!(
            order,
            vec![
                (0, true, 102),
                (1, true, 202),
                (0, false, 101),
                (0, false, 103),
                (1, false, 201)
            ]
        );
    }

    #[test]
    fn drain_drops_tasks_from_superseded_epochs() {
        let mut scheduler = Scheduler::default();
        scheduler.enqueue(task(0, false, 1));
        scheduler.enqueue(task(0, true, 2));
        scheduler.enqueue(task(1, false, 3));

        // Context 0 moved to epoch 1; context 1 still at epoch 0.
        let report = scheduler.drain(&|id| u64::from(id.index() == 0));

        assert_eq!(
            report.stale_dropped, 2,
            "both epoch-0 tasks of context 0 drop"
        );
        assert_eq!(report.tasks, vec![task(1, false, 3)]);
    }

    #[test]
    fn drop_context_clears_its_queue() {
        let mut scheduler = Scheduler::default();
        scheduler.enqueue(task(0, false, 1));
        scheduler.enqueue(task(1, false, 2));
        scheduler.drop_context(ContextId::from_index(0));

        let report = scheduler.drain(&|id| {
            let _ = id;
            0
        });
        assert_eq!(report.tasks, vec![task(1, false, 2)]);
        assert_eq!(report.stale_dropped, 0);
    }
}
