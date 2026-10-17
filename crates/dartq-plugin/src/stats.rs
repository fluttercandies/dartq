//! Per-plugin isolation counters: the observable face of the
//! resource-isolation substrate.
//!
//! Counters are process-wide atomics shared by the runtime's threads — the
//! reader thread counts wire traffic and guard drops, the monitor thread
//! tracks the child's peak RSS, and the runtime handle counts restarts. They
//! only ever increase (peak RSS by definition, the rest are totals), so there
//! is no snapshot-consistency problem: [`PluginStats::snapshot`] reads each
//! atomic once and the result is a valid point-in-time lower bound.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

/// Internal, shared counter set. Cheap to clone (`Arc` at the call sites).
#[derive(Debug, Default)]
pub struct PluginStats {
    peak_rss_bytes: AtomicU64,
    frames_in: AtomicU64,
    bytes_in: AtomicU64,
    frames_out: AtomicU64,
    bytes_out: AtomicU64,
    frames_dropped: AtomicU64,
    rss_disarms: AtomicU64,
    restarts: AtomicU64,
    evictions: AtomicU64,
    cycles: AtomicU64,
    degraded_cycles: AtomicU64,
    busy_nanos: AtomicU64,
}

/// One point-in-time read of every isolation counter. The face the daemon and
/// CLI read (`runtime.stats()`); plain data, no locks, no references into the
/// runtime.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PluginStatsSnapshot {
    /// Largest resident set the child process was ever seen holding, in
    /// bytes. Linux reads the kernel's `VmHWM` high-water mark — exact, no
    /// allocation spike can escape it. macOS samples `proc_pid_rusage`
    /// `ri_resident_size` at the monitor's poll interval, so the recorded
    /// peak is a lower bound: a spike fitting between two samples is
    /// invisible. Other targets do not sample (stays 0); every gap in this
    /// face is counted in [`Self::rss_disarms`].
    pub peak_rss_bytes: u64,
    /// Complete frames received from the plugin (handshake replies, plugin
    /// diagnostics, shutdown reply).
    pub frames_in: u64,
    /// Raw bytes read from the plugin's stdout, line terminators included.
    pub bytes_in: u64,
    /// Complete frames written to the plugin's stdin.
    pub frames_out: u64,
    /// Raw bytes written to the plugin's stdin, line terminators included.
    pub bytes_out: u64,
    /// Frames rejected by the 16 MiB frame guard before they crossed the
    /// wire. Inbound drops are fatal to the session (no resynchronization);
    /// outbound drops are rejected at the handle with the same typed error
    /// and leave the child running. Both explain an error the caller sees.
    pub frames_dropped: u64,
    /// Times the monitor disarmed the RSS watch for a generation: once per
    /// generation on platforms with no RSS sampler, or after repeated failed
    /// samples on a live child (a bounded tolerance rides out transient
    /// read failures first). Nonzero means the manifest's `memory_budget_mb`
    /// was unenforced — and `peak_rss_bytes` frozen — for those generations;
    /// the analyze callback clock stays armed regardless.
    pub rss_disarms: u64,
    /// Times the child process died and was relaunched during this runtime's
    /// lifetime (crash isolation: a crash never takes the engine
    /// down; restarts beyond the manifest's retry budget end in
    /// [`crate::RuntimeError::Crashed`]).
    pub restarts: u64,
    /// Times the monitor evicted an idle child (the manifest's
    /// `idle_evict_ms` elapsed with no interaction). An eviction is a host
    /// memory action, not a failure: the child is killed to release its
    /// resident set and relaunches budget-free at the next interaction —
    /// it never counts into [`Self::restarts`] and never surfaces a crash.
    pub evictions: u64,
    /// Analyze cycles the daemon hosted on this plugin (one `feed` + bounded
    /// drain each). The per-plugin overhead attribution face: alongside
    /// [`Self::busy_nanos`] and [`Self::degraded_cycles`] it prices the
    /// plugin subsystem's share of a benchmark cycle. Zero without plugins —
    /// no runtime, no counter.
    pub cycles: u64,
    /// Cycles whose drain budget expired before the plugin quiesced — the
    /// degraded shape: the plugin's remaining rows publish on a later cycle
    /// (the core-first publish invariant's skip-don't-wait posture).
    /// Persistently nonzero is the slow-plugin signature.
    pub degraded_cycles: u64,
    /// Total wall time the daemon spent interacting with this plugin (feed
    /// plus drain windows, summed over cycles), in nanoseconds. The absent
    /// path never arms a timer: this counter exists only on a live runtime.
    pub busy_nanos: u64,
}

impl PluginStats {
    /// Records a live RSS sample, keeping the maximum.
    pub fn observe_rss(&self, bytes: u64) {
        self.peak_rss_bytes.fetch_max(bytes, Ordering::Relaxed);
    }

    /// Records raw bytes read from the plugin's stdout (counted even for
    /// frames the codec then rejects, so `bytes_in` is a transport truth).
    pub fn bytes_in(&self, bytes: u64) {
        self.bytes_in.fetch_add(bytes, Ordering::Relaxed);
    }

    /// Records one complete inbound frame.
    pub fn frame_in(&self) {
        self.frames_in.fetch_add(1, Ordering::Relaxed);
    }

    /// Records raw bytes written to the plugin's stdin.
    pub fn bytes_out(&self, bytes: u64) {
        self.bytes_out.fetch_add(bytes, Ordering::Relaxed);
    }

    /// Records one complete outbound frame.
    pub fn frame_out(&self) {
        self.frames_out.fetch_add(1, Ordering::Relaxed);
    }

    /// Records one frame dropped by the size guard.
    pub fn frame_dropped(&self) {
        self.frames_dropped.fetch_add(1, Ordering::Relaxed);
    }

    /// Records one disarm of the RSS watch: the memory row is unenforced
    /// for that generation (no sampler on this platform, or sampling the
    /// child failed past tolerance).
    pub fn rss_disarm(&self) {
        self.rss_disarms.fetch_add(1, Ordering::Relaxed);
    }

    /// Records one child relaunch.
    pub fn restart(&self) {
        self.restarts.fetch_add(1, Ordering::Relaxed);
    }

    /// Records one idle eviction (the monitor killed an idle child; the
    /// next interaction relaunches budget-free).
    pub fn evict(&self) {
        self.evictions.fetch_add(1, Ordering::Relaxed);
    }

    /// Records one hosted analyze cycle: its busy wall time (the caller's
    /// feed-plus-drain window) and whether the drain budget expired before
    /// the plugin quiesced (a degraded cycle — rows deferred to a later
    /// cycle).
    pub fn observe_cycle(&self, busy: Duration, degraded: bool) {
        self.cycles.fetch_add(1, Ordering::Relaxed);
        let nanos = u64::try_from(busy.as_nanos()).unwrap_or(u64::MAX);
        self.busy_nanos.fetch_add(nanos, Ordering::Relaxed);
        if degraded {
            self.degraded_cycles.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Reads every counter once.
    #[must_use]
    pub fn snapshot(&self) -> PluginStatsSnapshot {
        let load = |cell: &AtomicU64| cell.load(Ordering::Relaxed);
        PluginStatsSnapshot {
            peak_rss_bytes: load(&self.peak_rss_bytes),
            frames_in: load(&self.frames_in),
            bytes_in: load(&self.bytes_in),
            frames_out: load(&self.frames_out),
            bytes_out: load(&self.bytes_out),
            frames_dropped: load(&self.frames_dropped),
            rss_disarms: load(&self.rss_disarms),
            restarts: load(&self.restarts),
            evictions: load(&self.evictions),
            cycles: load(&self.cycles),
            degraded_cycles: load(&self.degraded_cycles),
            busy_nanos: load(&self.busy_nanos),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counters_accumulate_and_peak_keeps_maximum() {
        let stats = PluginStats::default();
        stats.frame_in();
        stats.bytes_in(10);
        stats.frame_in();
        stats.bytes_in(5);
        stats.frame_out();
        stats.bytes_out(7);
        stats.frame_dropped();
        stats.observe_rss(100);
        stats.observe_rss(300);
        stats.observe_rss(200);
        stats.restart();
        stats.evict();
        stats.rss_disarm();

        let snap = stats.snapshot();
        assert_eq!(snap.frames_in, 2);
        assert_eq!(snap.bytes_in, 15);
        assert_eq!(snap.frames_out, 1);
        assert_eq!(snap.bytes_out, 7);
        assert_eq!(snap.frames_dropped, 1);
        assert_eq!(snap.peak_rss_bytes, 300);
        assert_eq!(snap.restarts, 1);
        assert_eq!(snap.evictions, 1);
        assert_eq!(snap.rss_disarms, 1);

        // The cycle face: every observed cycle counts, its wall time
        // accumulates, and only budget-expired drains count degraded.
        stats.observe_cycle(Duration::from_millis(5), false);
        stats.observe_cycle(Duration::from_millis(7), true);
        let snap = stats.snapshot();
        assert_eq!(snap.cycles, 2);
        assert_eq!(snap.degraded_cycles, 1);
        assert!(snap.busy_nanos >= 12_000_000, "busy time accumulates");
    }
}
