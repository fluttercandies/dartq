//! The memory-budget runtime (PRD §17.1): sampled-RSS accounting with the
//! tiered degradation ladder.
//!
//! # Accounting
//!
//! The budget samples the process's peak RSS through `getrusage` at phase
//! and library boundaries only — never per allocation, never on a hot path.
//! An unset limit (`None`) short-circuits every check at one `Option` test:
//! the sampler never runs and the output face is byte-identical to an
//! unbudgeted run. A failed or unsupported RSS sample degrades nothing —
//! the ladder acts only on data it has.
//!
//! # The ladder (PRD §17.1)
//!
//! Under pressure the engine descends one tier per boundary poll, in PRD's
//! order, each tier shedding one specific subsystem and emitting one
//! `dartq/memory_degraded` row (the tier and the limit in the message):
//!
//! 1. [`Tier::LazyParse`] — "lazy-parse results LRU eviction": the
//!    facts cache (memoized previous-content parse products) drains; live
//!    residency holds its own `Arc`s, so correctness is untouched.
//! 2. [`Tier::SemanticSlices`] — "semantic slices of non-active contexts
//!    swapped out (L2)": the scanned-face cache (token tables) drains;
//!    drained consumers fall back to a local scan on a miss (the state
//!    module's documented posture).
//! 3. [`Tier::AstCold`] — "AST cold regions swapped out": the warm-start
//!    staging map and the canonical-URI memo drop (both documented
//!    evictable — reformation reproduces them lazily) and the content
//!    store sweeps its dead bytes eagerly. Never `results` or residency:
//!    the last tier degrades capacity, never truth — diagnostics stay
//!    complete.
//!
//! The descent is sticky and one-way (fixed-size state: a limit plus a
//! tier counter). A tier fires at most once per engine: re-shedding an
//! emptied cache is a no-op, and re-emitting the row would only spam the
//! output face.

use dartq_diagnostics::{DARTQ_MEMORY_DEGRADED, Diagnostic, Severity};
use dartq_source::{FileId, SourceRange};

/// The degradation ladder in PRD §17.1's order — index `n` is the `n`-th
/// tier to descend.
const TIERS: [Tier; 3] = [Tier::LazyParse, Tier::SemanticSlices, Tier::AstCold];

/// One rung of the §17.1 ladder; the diagnostic message carries the PRD
/// tier name verbatim.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Tier {
    /// Lazy-parse results LRU eviction: the facts cache drains.
    LazyParse,
    /// Non-active-context semantic slices swap out (L2): the scanned faces
    /// drain.
    SemanticSlices,
    /// AST cold regions swap out: warm-start staging, the canonical-URI
    /// memo, and the store's dead bytes go.
    AstCold,
}

impl Tier {
    /// The shed description the diagnostic message carries — the PRD tier
    /// name, verbatim.
    pub(crate) const fn shed(self) -> &'static str {
        match self {
            Self::LazyParse => "lazy-parse results evicted (LRU)",
            Self::SemanticSlices => "non-active context semantic slices swapped out (L2)",
            Self::AstCold => "AST cold regions swapped out",
        }
    }
}

/// The budget state: the limit plus how far the ladder has descended.
/// Fixed-size; no atomics (the engine's `&mut` world makes the poll sites
/// total).
#[derive(Clone, Copy, Debug)]
pub(crate) struct MemoryBudget {
    limit_bytes: u64,
    limit_mib: u64,
    /// Descended-tier count — the index of the next tier to fire.
    fired: u8,
}

impl MemoryBudget {
    /// Arms the ladder at `limit_mib` MiB.
    pub(crate) fn new(limit_mib: u64) -> Self {
        Self {
            limit_bytes: limit_mib.saturating_mul(1024 * 1024),
            limit_mib,
            fired: 0,
        }
    }
}

/// The phase-boundary poll: descends at most one tier while over budget.
/// `None` budget short-circuits before the sampler runs — the zero-cost
/// shape an unbudgeted run pays.
pub(crate) fn poll(budget: &mut Option<MemoryBudget>) -> Option<(Tier, u64)> {
    poll_with(budget, sample_rss())
}

/// [`poll`] with the RSS reading injected — the pins' seam (a test cannot
/// force the process's real footprint).
fn poll_with(budget: &mut Option<MemoryBudget>, rss: Option<u64>) -> Option<(Tier, u64)> {
    let budget = budget.as_mut()?;
    let rss = rss?;
    if rss <= budget.limit_bytes || budget.fired >= TIERS.len() as u8 {
        return None;
    }
    let tier = TIERS[usize::from(budget.fired)];
    budget.fired += 1;
    Some((tier, budget.limit_mib))
}

/// The `dartq/memory_degraded` row: `INFO|TOOL` (a capacity note must not
/// fail runs through the error-severity exit tiers), anchored at `file`,
/// the tier name and the limit in the message.
pub(crate) fn degraded_diagnostic(tier: Tier, limit_mib: u64, file: FileId) -> Diagnostic {
    Diagnostic::new(
        Severity::Info,
        DARTQ_MEMORY_DEGRADED,
        format!(
            "Memory budget exceeded (limit {limit_mib} MiB): {}.",
            tier.shed()
        ),
        file,
        SourceRange {
            offset: 0,
            length: 0,
        },
        None,
    )
}

/// Peak RSS through `getrusage`; macOS reports bytes, Linux KiB (normalized
/// to bytes per benchmark-harness-spec §2.2). `None` off Unix or when the
/// kernel call fails — the ladder never acts on a missing reading.
#[cfg(unix)]
fn sample_rss() -> Option<u64> {
    let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
    if unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut usage) } != 0 {
        return None;
    }
    #[cfg(target_os = "macos")]
    let rss = u64::try_from(usage.ru_maxrss).ok()?; // bytes on macOS
    #[cfg(not(target_os = "macos"))]
    let rss = u64::try_from(usage.ru_maxrss).ok()? * 1024; // KiB on Linux
    Some(rss)
}

#[cfg(not(unix))]
fn sample_rss() -> Option<u64> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use dartq_source::FileTable;

    const MIB: u64 = 1024 * 1024;

    fn armed(limit_mib: u64) -> Option<MemoryBudget> {
        Some(MemoryBudget::new(limit_mib))
    }

    /// PRD §17.1's ladder descends in order, one tier per boundary poll:
    /// sustained pressure over a synthetic 1 MiB limit walks LazyParse →
    /// SemanticSlices → AstCold, then holds at the floor (no further
    /// shedding, no repeated emission).
    #[test]
    fn ladder_descends_in_prd_order_one_tier_per_boundary() {
        let mut budget = armed(1);
        let fired: Vec<Option<Tier>> = (0..5)
            .map(|_| poll_with(&mut budget, Some(2 * MIB)).map(|(tier, _)| tier))
            .collect();
        assert_eq!(
            fired,
            vec![
                Some(Tier::LazyParse),
                Some(Tier::SemanticSlices),
                Some(Tier::AstCold),
                None,
                None,
            ],
            "one tier per poll, PRD order, sticky floor"
        );
    }

    /// Under the limit nothing fires, however often the boundary is polled.
    #[test]
    fn under_budget_never_degrades() {
        let mut budget = armed(100);
        for _ in 0..4 {
            assert!(poll_with(&mut budget, Some(64 * MIB)).is_none());
        }
    }

    /// A missing RSS reading sheds nothing — the ladder acts only on data
    /// it has (unsupported targets, failed kernel calls).
    #[test]
    fn missing_sample_never_degrades() {
        let mut budget = armed(1);
        for _ in 0..4 {
            assert!(poll_with(&mut budget, None).is_none());
        }
        // The ladder is untouched: a later real reading over the limit
        // still starts at the first tier.
        assert_eq!(
            poll_with(&mut budget, Some(2 * MIB)).map(|(tier, _)| tier),
            Some(Tier::LazyParse)
        );
    }

    /// The zero-overhead shape: a `None` budget returns before the sampler
    /// would run — an unbudgeted engine executes no accounting at all (the
    /// strongest reading available off the hot path: the short-circuit is
    /// the function's first step, structurally).
    #[test]
    fn no_budget_short_circuits_before_accounting() {
        let mut budget = None;
        assert!(poll_with(&mut budget, Some(u64::MAX)).is_none());
    }

    /// The emitted row carries the registered `dartq/memory_degraded` code,
    /// the PRD tier name verbatim, and the limit — at INFO severity (a
    /// capacity note must not fail runs through the error exit tiers).
    #[test]
    fn degraded_row_names_tier_and_budget() {
        let mut files = FileTable::new();
        let anchor = files.intern(std::path::Path::new("lib/a.dart"));
        let row = degraded_diagnostic(Tier::AstCold, 2048, anchor);
        assert_eq!(row.code.name, "dartq/memory_degraded");
        assert_eq!(row.severity, Severity::Info);
        assert_eq!(row.file, anchor);
        assert!(
            row.message.contains("AST cold regions swapped out"),
            "the PRD tier name rides verbatim: {}",
            row.message
        );
        assert!(
            row.message.contains("2048"),
            "the limit rides in MiB: {}",
            row.message
        );

        let lazy = degraded_diagnostic(Tier::LazyParse, 64, anchor);
        assert!(lazy.message.contains("lazy-parse results evicted (LRU)"));
        assert!(lazy.message.contains("64"));
    }
}
