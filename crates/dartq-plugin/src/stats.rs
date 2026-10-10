//! Per-plugin isolation counters (PRD §14.3): the observable face of the
//! resource-isolation substrate.
//!
//! Counters are process-wide atomics shared by the runtime's threads — the
//! reader thread counts wire traffic and guard drops, the monitor thread
//! tracks the child's peak RSS, and the runtime handle counts restarts. They
//! only ever increase (peak RSS by definition, the rest are totals), so there
//! is no snapshot-consistency problem: [`PluginStats::snapshot`] reads each
//! atomic once and the result is a valid point-in-time lower bound.

use std::sync::atomic::{AtomicU64, Ordering};

/// Internal, shared counter set. Cheap to clone (`Arc` at the call sites).
#[derive(Debug, Default)]
pub struct PluginStats {
    peak_rss_bytes: AtomicU64,
    frames_in: AtomicU64,
    bytes_in: AtomicU64,
    frames_out: AtomicU64,
    bytes_out: AtomicU64,
    frames_dropped: AtomicU64,
    restarts: AtomicU64,
}

/// One point-in-time read of every isolation counter. The face the daemon and
/// CLI read (`runtime.stats()`); plain data, no locks, no references into the
/// runtime.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PluginStatsSnapshot {
    /// Largest resident set the child process was ever seen holding, in
    /// bytes. Linux reads the kernel's `VmHWM` high-water mark (exact); macOS
    /// samples `proc_pid_rusage` `ri_resident_size` at a fixed poll interval;
    /// other targets do not sample (stays 0).
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
    /// Frames rejected by the 16 MiB frame guard before buffering. Every
    /// drop is fatal to the session (no resynchronization), so this counter
    /// also explains restarts.
    pub frames_dropped: u64,
    /// Times the child process died and was relaunched during this runtime's
    /// lifetime (PRD §14.3 crash isolation: a crash never takes the engine
    /// down; restarts beyond the manifest's retry budget end in
    /// [`crate::RuntimeError::Crashed`]).
    pub restarts: u64,
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

    /// Records one child relaunch.
    pub fn restart(&self) {
        self.restarts.fetch_add(1, Ordering::Relaxed);
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
            restarts: load(&self.restarts),
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

        let snap = stats.snapshot();
        assert_eq!(snap.frames_in, 2);
        assert_eq!(snap.bytes_in, 15);
        assert_eq!(snap.frames_out, 1);
        assert_eq!(snap.bytes_out, 7);
        assert_eq!(snap.frames_dropped, 1);
        assert_eq!(snap.peak_rss_bytes, 300);
        assert_eq!(snap.restarts, 1);
    }
}
