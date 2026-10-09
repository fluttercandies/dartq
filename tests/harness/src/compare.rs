//! (code, file) entry-set comparison (PRD §19 semantics): codes are lowercased, files are
//! normalized to corpus-relative paths, Jaccard is computed per severity tier, and per-side
//! diff details are produced. Entry ordering is always lexicographic.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use crate::machine::{EntryKeyBuilder, MachineDiag, Severity};

pub(crate) type EntryKey = (String, String);

/// One side's diagnostic set: full entry keys + grouping by severity + per-key sample (occurrence count and first message).
pub(crate) struct Side {
    /// Diagnostic instances before the severity/type/codes filters
    pub(crate) pre_filter_total: usize,
    /// Diagnostic instances entering the comparison (post-filter)
    pub(crate) total_diags: usize,
    /// `pre_filter_total - total_diags`: instances dropped by the filters.
    /// Kept explicit so a silent narrowing (e.g. a truncated codes file)
    /// leaves a visible trace in the report instead of passing unnoticed.
    pub(crate) dropped_by_filter: usize,
    pub(crate) entries: BTreeSet<EntryKey>,
    pub(crate) per_severity: BTreeMap<Severity, BTreeSet<EntryKey>>,
    samples: BTreeMap<EntryKey, Sample>,
    /// Instance count per (severity, key). `samples` totals all instances of a key regardless of
    /// tier, so per-tier instance aggregation ([`distribution`]`::code_by_severity`) sources its
    /// counts from here: a key spanning two tiers must not leak its cross-tier total into either
    /// tier's count.
    tier_samples: BTreeMap<(Severity, EntryKey), usize>,
}

pub(crate) struct Sample {
    pub(crate) count: usize,
    pub(crate) dtype: String,
    pub(crate) line: u32,
    pub(crate) col: u32,
    pub(crate) length: u32,
    pub(crate) message: String,
}

/// `label`: disambiguation prefix for file keys when merging multiple corpora (the corpus dir name, e.g. `m0`);
/// always `None` for a single corpus, keeping the key shape identical to the historical behavior.
/// `keep`: the stacked severity/type/codes filter predicate; pre-filter totals
/// and dropped counts are recorded on the Side (see [`Side::dropped_by_filter`]).
pub(crate) fn build_side_labeled(
    diags: &[MachineDiag],
    corpus_root: &Path,
    label: Option<&str>,
    keep: impl Fn(&MachineDiag) -> bool,
) -> Side {
    let mut side = Side {
        pre_filter_total: diags.len(),
        total_diags: 0,
        dropped_by_filter: 0,
        entries: BTreeSet::new(),
        per_severity: BTreeMap::new(),
        samples: BTreeMap::new(),
        tier_samples: BTreeMap::new(),
    };
    let mut keys = EntryKeyBuilder::new(corpus_root);
    for d in diags {
        if !keep(d) {
            side.dropped_by_filter += 1;
            continue;
        }
        side.total_diags += 1;
        let mut key = keys.key(d);
        if let Some(label) = label {
            key.1 = format!("{label}/{}", key.1);
        }
        side.entries.insert(key.clone());
        side.per_severity
            .entry(d.severity)
            .or_default()
            .insert(key.clone());
        *side
            .tier_samples
            .entry((d.severity, key.clone()))
            .or_default() += 1;
        let sample = side.samples.entry(key).or_insert_with(|| Sample {
            count: 0,
            dtype: d.dtype.clone(),
            line: d.line,
            col: d.col,
            length: d.length,
            message: d.message.clone(),
        });
        sample.count += 1;
    }
    side
}

/// Multi-corpus merge: keys already carry corpus labels so they cannot collide across corpora; merging is a
/// set union + count sum (the first sample in argument order is kept, matching the single-corpus
/// "first message per key" semantics).
pub(crate) fn merge_sides(sides: Vec<Side>) -> Side {
    let mut sides = sides.into_iter();
    let Some(mut merged) = sides.next() else {
        panic!("merge_sides needs at least one side");
    };
    for side in sides {
        merged.pre_filter_total += side.pre_filter_total;
        merged.total_diags += side.total_diags;
        merged.dropped_by_filter += side.dropped_by_filter;
        merged.entries.extend(side.entries);
        for (severity, keys) in side.per_severity {
            merged
                .per_severity
                .entry(severity)
                .or_default()
                .extend(keys);
        }
        for (key, sample) in side.samples {
            match merged.samples.get_mut(&key) {
                Some(existing) => existing.count += sample.count,
                None => {
                    merged.samples.insert(key, sample);
                }
            }
        }
        for (tier_key, count) in side.tier_samples {
            *merged.tier_samples.entry(tier_key).or_default() += count;
        }
    }
    merged
}

/// Jaccard = |A∩B| / |A∪B|; both sides empty is defined as full agreement (1.0).
#[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
// entry counts are far below 2^53; integer-to-f64 conversion has no precision/truncation loss
pub(crate) fn jaccard(a: &BTreeSet<EntryKey>, b: &BTreeSet<EntryKey>) -> f64 {
    if a.is_empty() && b.is_empty() {
        return 1.0;
    }
    let inter = a.intersection(b).count();
    let union = a.union(b).count();
    inter as f64 / union as f64
}

pub(crate) struct DiffEntry {
    pub(crate) code: String,
    pub(crate) file: String,
    pub(crate) count: usize,
    pub(crate) dtype: String,
    /// Position of the first sample (for regression triage)
    pub(crate) line: u32,
    pub(crate) col: u32,
    pub(crate) length: u32,
    pub(crate) sample_message: String,
}

/// One row of a side's full post-filter entry list: the (code, file) key plus its severity tier
/// and diagnostic instance count. Rows are sorted lexicographic by (code, file).
pub(crate) struct SideEntry {
    pub(crate) code: String,
    pub(crate) file: String,
    pub(crate) severity: Severity,
    pub(crate) count: usize,
}

pub(crate) struct SeverityStats {
    pub(crate) severity: Severity,
    pub(crate) jaccard: f64,
    pub(crate) intersection: usize,
    pub(crate) union: usize,
    pub(crate) oracle_only: usize,
    pub(crate) dartq_only: usize,
}

pub(crate) struct Comparison {
    pub(crate) oracle_total: usize,
    pub(crate) dartq_total: usize,
    /// Per-side pre-filter instance totals and dropped-by-filter counts
    /// (see [`Side::dropped_by_filter`])
    pub(crate) oracle_pre_filter: usize,
    pub(crate) oracle_dropped: usize,
    pub(crate) dartq_pre_filter: usize,
    pub(crate) dartq_dropped: usize,
    /// Distinct files touched by the union of entries
    pub(crate) file_count: usize,
    /// Fixed order ERROR / WARNING / INFO
    pub(crate) severities: Vec<SeverityStats>,
    /// Lexicographic
    pub(crate) oracle_only: Vec<DiffEntry>,
    pub(crate) dartq_only: Vec<DiffEntry>,
    /// Full post-filter per-side entry lists (sorted by (code, file)): the machine-side data
    /// plane for per-diff gate attribution, populated even when both diff lists are empty.
    pub(crate) oracle_entries: Vec<SideEntry>,
    pub(crate) dartq_entries: Vec<SideEntry>,
    /// Oracle-side distribution stats (based on the filtered oracle entry set; for baseline analysis)
    pub(crate) oracle_distribution: Distribution,
}

impl Comparison {
    /// Exit-code gate metric: ERROR-tier Jaccard (M0 acceptance criterion).
    pub(crate) fn error_jaccard(&self) -> f64 {
        self.severities
            .iter()
            .find(|s| s.severity == Severity::Error)
            .map_or(1.0, |s| s.jaccard)
    }
}

/// Oracle-side distribution stats (for corpus baseline analysis). Everything is aggregated by
/// "diagnostic instance count" (multiple occurrences of the same key are summed), not deduplicated
/// entry counts; codes use the lowercase entry-key form.
#[derive(Default)]
pub(crate) struct Distribution {
    /// TYPE -> diagnostic instance count
    pub(crate) by_type: BTreeMap<String, usize>,
    /// severity -> code -> diagnostic instance count (code frequencies that drive gate weighting, e.g. ERROR-tier TOP20)
    pub(crate) code_by_severity: BTreeMap<Severity, BTreeMap<String, usize>>,
    /// code -> distinct files involved
    pub(crate) files_by_code: BTreeMap<String, usize>,
    /// TYPE -> file -> diagnostic instance count (locating files hit hardest by syntax errors)
    pub(crate) files_by_type: BTreeMap<String, BTreeMap<String, usize>>,
}

/// Aggregate the distribution from a single side's entry set and sample counts; the input is the
/// filtered side, so the distribution scope follows the filters.
pub(crate) fn distribution(side: &Side) -> Distribution {
    let mut dist = Distribution::default();
    for ((severity, key), count) in &side.tier_samples {
        *dist
            .code_by_severity
            .entry(*severity)
            .or_default()
            .entry(key.0.clone())
            .or_default() += count;
    }
    // files_by_code counts distinct (code, file) pairs: a key can appear in
    // more than one severity tier, so counting from the per-tier loops above
    // would double-count its file.
    for key in &side.entries {
        let sample = &side.samples[key];
        *dist.by_type.entry(sample.dtype.clone()).or_default() += sample.count;
        *dist.files_by_code.entry(key.0.clone()).or_default() += 1;
        *dist
            .files_by_type
            .entry(sample.dtype.clone())
            .or_default()
            .entry(key.1.clone())
            .or_default() += sample.count;
    }
    dist
}

pub(crate) fn compare(oracle: &Side, dartq: &Side) -> Comparison {
    let oracle_only: Vec<DiffEntry> = oracle
        .entries
        .difference(&dartq.entries)
        .map(|k| diff_entry(k, oracle))
        .collect();
    let dartq_only: Vec<DiffEntry> = dartq
        .entries
        .difference(&oracle.entries)
        .map(|k| diff_entry(k, dartq))
        .collect();
    let severities = [Severity::Error, Severity::Warning, Severity::Info]
        .into_iter()
        .map(|sev| {
            let empty = BTreeSet::new();
            let a = oracle.per_severity.get(&sev).unwrap_or(&empty);
            let b = dartq.per_severity.get(&sev).unwrap_or(&empty);
            let intersection = a.intersection(b).count();
            let union = a.union(b).count();
            SeverityStats {
                severity: sev,
                jaccard: jaccard(a, b),
                intersection,
                union,
                oracle_only: a.len() - intersection,
                dartq_only: b.len() - intersection,
            }
        })
        .collect();
    let file_count = oracle
        .entries
        .union(&dartq.entries)
        .map(|k| k.1.clone())
        .collect::<BTreeSet<_>>()
        .len();
    Comparison {
        oracle_total: oracle.total_diags,
        dartq_total: dartq.total_diags,
        oracle_pre_filter: oracle.pre_filter_total,
        oracle_dropped: oracle.dropped_by_filter,
        dartq_pre_filter: dartq.pre_filter_total,
        dartq_dropped: dartq.dropped_by_filter,
        file_count,
        severities,
        oracle_only,
        dartq_only,
        oracle_entries: side_entries(oracle),
        dartq_entries: side_entries(dartq),
        oracle_distribution: distribution(oracle),
    }
}

/// Full post-filter entry list of one side, sorted by (code, file). A key is tagged with the
/// first severity tier it appears in (iteration order ERROR/WARNING/INFO); one analyzer gives
/// one tier per code, so this is 1:1 in practice.
fn side_entries(side: &Side) -> Vec<SideEntry> {
    let mut severity_of: BTreeMap<&EntryKey, Severity> = BTreeMap::new();
    for (severity, keys) in &side.per_severity {
        for key in keys {
            severity_of.entry(key).or_insert(*severity);
        }
    }
    side.entries
        .iter()
        .map(|key| SideEntry {
            code: key.0.clone(),
            file: key.1.clone(),
            severity: severity_of[key],
            count: side.samples[key].count,
        })
        .collect()
}

fn diff_entry(key: &EntryKey, side: &Side) -> DiffEntry {
    let sample = &side.samples[key];
    DiffEntry {
        code: key.0.clone(),
        file: key.1.clone(),
        count: sample.count,
        dtype: sample.dtype.clone(),
        line: sample.line,
        col: sample.col,
        length: sample.length,
        sample_message: sample.message.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn diag(code: &str, file: &str, severity: Severity, message: &str) -> MachineDiag {
        MachineDiag {
            severity,
            dtype: "SYNTACTIC_ERROR".into(),
            code: code.into(),
            file: PathBuf::from(file),
            line: 1,
            col: 1,
            length: 1,
            message: message.into(),
        }
    }

    fn set(entries: &[(&str, &str)]) -> BTreeSet<EntryKey> {
        entries
            .iter()
            .map(|(c, f)| (c.to_string(), f.to_string()))
            .collect()
    }

    #[test]
    #[allow(clippy::float_cmp)] // 1.0/0.0 come from integer counts of identical/disjoint sets; results are exact
    fn jaccard_math() {
        let empty = BTreeSet::new();
        assert_eq!(jaccard(&empty, &empty), 1.0, "both empty = full agreement");
        let a = set(&[("e1", "a.dart"), ("e2", "a.dart")]);
        let b = set(&[("e1", "a.dart"), ("e2", "a.dart")]);
        assert_eq!(jaccard(&a, &b), 1.0, "identical sets");
        let c = set(&[("x", "b.dart")]);
        assert_eq!(jaccard(&a, &c), 0.0, "disjoint sets");
        let d = set(&[("e1", "a.dart"), ("e9", "z.dart")]);
        // |∩|=1, |∪|=3
        assert!((jaccard(&a, &d) - 1.0 / 3.0).abs() < 1e-12);
    }

    #[test]
    fn compare_produces_sorted_diffs_and_severity_stats() {
        // Key normalization: code is case-insensitive
        let oracle = vec![
            diag("E_A", "/corpus/a.dart", Severity::Error, "msg a"),
            diag("E_B", "/corpus/b.dart", Severity::Error, "msg b"),
            diag("W_A", "/corpus/a.dart", Severity::Warning, "warn a"),
        ];
        let dartq = vec![
            diag("e_a", "/corpus/a.dart", Severity::Error, "msg a"),
            diag("e_c", "/corpus/c.dart", Severity::Error, "msg c"),
            diag("w_a", "/corpus/a.dart", Severity::Warning, "warn a"),
        ];
        let root = Path::new("/corpus");
        let cmp = compare(
            &build_side_labeled(&oracle, root, None, |_| true),
            &build_side_labeled(&dartq, root, None, |_| true),
        );

        assert_eq!(cmp.oracle_total, 3);
        assert_eq!(cmp.dartq_total, 3);
        assert_eq!(cmp.file_count, 3);

        let err = &cmp.severities[0];
        assert_eq!(err.severity, Severity::Error);
        assert_eq!(err.intersection, 1);
        assert_eq!(err.union, 3);
        assert_eq!(err.oracle_only, 1);
        assert_eq!(err.dartq_only, 1);
        assert!((err.jaccard - 1.0 / 3.0).abs() < 1e-12);

        // Lexicographic: b < c
        assert_eq!(cmp.oracle_only.len(), 1);
        assert_eq!(cmp.oracle_only[0].code, "e_b");
        assert_eq!(cmp.oracle_only[0].file, "b.dart");
        assert_eq!(cmp.oracle_only[0].sample_message, "msg b");
        assert_eq!(cmp.dartq_only.len(), 1);
        assert_eq!(cmp.dartq_only[0].code, "e_c");

        assert!((cmp.error_jaccard() - 1.0 / 3.0).abs() < 1e-12);
    }

    #[test]
    fn count_and_sample_aggregate_per_key() {
        let oracle = vec![
            diag("E_A", "/corpus/a.dart", Severity::Error, "first"),
            diag("E_A", "/corpus/a.dart", Severity::Error, "second"),
        ];
        let side = build_side_labeled(&oracle, Path::new("/corpus"), None, |_| true);
        assert_eq!(side.total_diags, 2);
        assert_eq!(side.entries.len(), 1, "same (code,file) dedups");
        let key = side.entries.iter().next().unwrap();
        assert_eq!(side.samples[key].count, 2);
        assert_eq!(side.samples[key].message, "first", "keeps first message");
    }

    /// Full per-side entry lists: every post-filter (code,file) key exactly once, sorted,
    /// severity-tagged with instance counts -- populated even when the diff lists are empty,
    /// so a passing gate still records what each side saw.
    #[test]
    fn side_entries_list_every_post_filter_key_sorted() {
        let oracle = vec![
            diag("E_B", "/corpus/b.dart", Severity::Error, "b"),
            diag("E_A", "/corpus/a.dart", Severity::Error, "a1"),
            diag("E_A", "/corpus/a.dart", Severity::Error, "a2"),
            diag("W_A", "/corpus/a.dart", Severity::Warning, "w"),
        ];
        let dartq = vec![diag("e_a", "/corpus/a.dart", Severity::Error, "a")];
        let root = Path::new("/corpus");
        let cmp = compare(
            &build_side_labeled(&oracle, root, None, |_| true),
            &build_side_labeled(&dartq, root, None, |_| true),
        );
        let keys: Vec<(&str, &str)> = cmp
            .oracle_entries
            .iter()
            .map(|e| (e.code.as_str(), e.file.as_str()))
            .collect();
        assert_eq!(
            keys,
            [("e_a", "a.dart"), ("e_b", "b.dart"), ("w_a", "a.dart"),],
            "every key once, sorted by (code, file)"
        );
        let a = &cmp.oracle_entries[0];
        assert_eq!(a.severity, Severity::Error);
        assert_eq!(a.count, 2, "instance count aggregated per key");
        assert_eq!(cmp.oracle_entries[2].severity, Severity::Warning);
        assert_eq!(cmp.dartq_entries.len(), 1);
        assert_eq!(cmp.dartq_entries[0].count, 1);
    }

    /// M0 gate semantics: after `--type-filter syntactic_error`, semantic codes (`COMPILE_TIME_ERROR`)
    /// do not enter the ERROR-tier Jaccard -- the gate judges only the filtered syntactic set.
    #[test]
    #[allow(clippy::float_cmp)] // count ratios 1.0/2.0/0.4 are exact
    fn type_filtered_comparison_drives_gate() {
        let mk = |code: &str, file: &str, dtype: &str| MachineDiag {
            severity: Severity::Error,
            dtype: dtype.into(),
            code: code.into(),
            file: PathBuf::from(file),
            line: 1,
            col: 1,
            length: 1,
            message: "m".into(),
        };
        // oracle: 2 syntactic + 2 semantic; dartq: 1 syntactic hit, 1 semantic hit, 1 syntactic miss
        let oracle = vec![
            mk("E_A", "/corpus/a.dart", "SYNTACTIC_ERROR"),
            mk("E_B", "/corpus/b.dart", "SYNTACTIC_ERROR"),
            mk("S_A", "/corpus/a.dart", "COMPILE_TIME_ERROR"),
            mk("S_B", "/corpus/c.dart", "COMPILE_TIME_ERROR"),
        ];
        let dartq = vec![
            mk("E_A", "/corpus/a.dart", "SYNTACTIC_ERROR"),
            mk("E_C", "/corpus/c.dart", "SYNTACTIC_ERROR"),
            mk("S_A", "/corpus/a.dart", "COMPILE_TIME_ERROR"),
        ];
        let root = Path::new("/corpus");
        let unfiltered = compare(
            &build_side_labeled(&oracle, root, None, |_| true),
            &build_side_labeled(&dartq, root, None, |_| true),
        );
        assert!(
            (unfiltered.error_jaccard() - 0.4).abs() < 1e-12,
            "2/5 with semantic codes"
        );

        let keep_syn = |d: &MachineDiag| d.dtype == "SYNTACTIC_ERROR";
        let filtered = compare(
            &build_side_labeled(&oracle, root, None, keep_syn),
            &build_side_labeled(&dartq, root, None, keep_syn),
        );
        assert_eq!(filtered.oracle_total, 2, "post-filter instance counts");
        assert_eq!(filtered.dartq_total, 2);
        // ∩ = {E_A@a.dart}, ∪ = {E_A, E_B, E_C}
        assert!(
            (filtered.error_jaccard() - 1.0 / 3.0).abs() < 1e-12,
            "1/3 syntactic-only"
        );
    }

    /// Empty-set semantics (pinned rule): when a filter removes all diagnostics from both sides,
    /// both entry sets are empty, and Jaccard keeps the existing rule "both empty = full agreement = 1.0",
    /// so the gate still passes (consistent with severity-filter dropping the ERROR tier in pure-report mode).
    #[test]
    #[allow(clippy::float_cmp)]
    fn fully_filtered_empty_sets_agree_at_one() {
        let oracle = vec![diag("E_A", "/corpus/a.dart", Severity::Error, "m")];
        let dartq = vec![diag("E_B", "/corpus/b.dart", Severity::Error, "m")];
        let root = Path::new("/corpus");
        let keep_lint = |d: &MachineDiag| d.dtype == "LINT";
        let cmp = compare(
            &build_side_labeled(&oracle, root, None, keep_lint),
            &build_side_labeled(&dartq, root, None, keep_lint),
        );
        assert_eq!(cmp.oracle_total, 0);
        assert_eq!(cmp.dartq_total, 0);
        assert_eq!(cmp.severities[0].union, 0, "ERROR tier empty after filter");
        assert_eq!(cmp.error_jaccard(), 1.0, "empty vs empty = full agreement");
    }

    #[test]
    fn distribution_aggregates_instances_and_files() {
        let oracle = vec![
            diag("E_A", "/corpus/a.dart", Severity::Error, "1st"),
            diag("E_A", "/corpus/a.dart", Severity::Error, "2nd"),
            {
                let mut d = diag("E_A", "/corpus/b.dart", Severity::Error, "3rd");
                d.dtype = "COMPILE_TIME_ERROR".into();
                d
            },
            {
                let mut d = diag("W_A", "/corpus/a.dart", Severity::Warning, "w");
                d.dtype = "STATIC_WARNING".into();
                d
            },
        ];
        let dist = distribution(&build_side_labeled(
            &oracle,
            Path::new("/corpus"),
            None,
            |_| true,
        ));

        assert_eq!(
            dist.by_type["SYNTACTIC_ERROR"], 2,
            "instance counts, not entries"
        );
        assert_eq!(dist.by_type["COMPILE_TIME_ERROR"], 1);
        assert_eq!(dist.by_type["STATIC_WARNING"], 1);
        assert_eq!(dist.code_by_severity[&Severity::Error]["e_a"], 3);
        assert_eq!(dist.code_by_severity[&Severity::Warning]["w_a"], 1);
        assert_eq!(dist.files_by_code["e_a"], 2, "distinct files per code");
        assert_eq!(dist.files_by_type["SYNTACTIC_ERROR"]["a.dart"], 2);
        assert!(!dist.files_by_type["SYNTACTIC_ERROR"].contains_key("b.dart"));
        assert_eq!(dist.files_by_type["STATIC_WARNING"]["a.dart"], 1);
        assert!(!dist.files_by_type["COMPILE_TIME_ERROR"].contains_key("a.dart"));
    }

    /// files_by_code counts distinct (code, file) entries: the same key in two
    /// severity tiers (one analyzer emitting a code under different severities)
    /// must count its file once, while per-tier instance counts stay tier-scoped.
    #[test]
    fn files_by_code_dedups_across_severity_tiers() {
        let oracle = vec![
            diag("E_A", "/corpus/a.dart", Severity::Error, "as error"),
            diag("E_A", "/corpus/a.dart", Severity::Warning, "as warning"),
            diag("E_A", "/corpus/b.dart", Severity::Error, "other file"),
        ];
        let dist = distribution(&build_side_labeled(
            &oracle,
            Path::new("/corpus"),
            None,
            |_| true,
        ));
        assert_eq!(
            dist.files_by_code["e_a"], 2,
            "a.dart counted once across tiers"
        );
        assert_eq!(dist.code_by_severity[&Severity::Error]["e_a"], 2);
        assert_eq!(dist.code_by_severity[&Severity::Warning]["e_a"], 1);
    }

    /// Multi-corpus merge: labels disambiguate (identical relative paths do not collide), counts sum, first sample kept.
    #[test]
    fn merge_sides_disambiguates_by_label_and_sums_counts() {
        let a = vec![diag("E_A", "/c1/lib/x.dart", Severity::Error, "from c1")];
        let b = vec![diag("E_A", "/c2/lib/x.dart", Severity::Error, "from c2")];
        let merged = merge_sides(vec![
            build_side_labeled(&a, Path::new("/c1"), Some("m0"), |_| true),
            build_side_labeled(&b, Path::new("/c2"), Some("m0-syntax"), |_| true),
        ]);
        assert_eq!(merged.total_diags, 2);
        assert_eq!(merged.entries.len(), 2, "same rel path, labels differ");
        assert!(
            merged
                .entries
                .contains(&("e_a".into(), "m0/lib/x.dart".into()))
        );
        assert!(
            merged
                .entries
                .contains(&("e_a".into(), "m0-syntax/lib/x.dart".into()))
        );

        // Same corpus, same key: counts accumulate across merges
        let a2 = vec![
            diag("E_A", "/c1/lib/x.dart", Severity::Error, "first"),
            diag("E_A", "/c1/lib/x.dart", Severity::Error, "second"),
        ];
        let merged = merge_sides(vec![
            build_side_labeled(&a, Path::new("/c1"), Some("m0"), |_| true),
            build_side_labeled(&a2, Path::new("/c1"), Some("m0"), |_| true),
        ]);
        assert_eq!(merged.entries.len(), 1);
        assert_eq!(
            merged.samples[&("e_a".into(), "m0/lib/x.dart".into())].count,
            3
        );
        assert_eq!(
            merged.samples[&("e_a".into(), "m0/lib/x.dart".into())].message,
            "from c1",
            "keeps first sample"
        );
    }

    /// Filter-drop accounting: pre-filter totals and dropped counts must
    /// reflect exactly what the filters removed -- a truncated codes file
    /// silently narrowing the gate shows up as a nonzero dropped count
    /// instead of passing unnoticed.
    #[test]
    #[allow(clippy::float_cmp)] // count ratio 1.0 is exact
    fn side_records_pre_filter_and_dropped_counts() {
        let mk = |code: &str| MachineDiag {
            severity: Severity::Error,
            dtype: "COMPILE_TIME_ERROR".into(),
            code: code.into(),
            file: PathBuf::from("/corpus/a.dart"),
            line: 1,
            col: 1,
            length: 1,
            message: "m".into(),
        };
        // Simulates a truncated codes file: only expected_token survives the filter
        let oracle = vec![
            mk("EXPECTED_TOKEN"),
            mk("UNDEFINED_CLASS"),
            mk("EXPECTED_TOKEN"),
        ];
        let dartq = vec![mk("expected_token")];
        let in_set = |d: &MachineDiag| d.code.to_lowercase() == "expected_token";
        let oracle_side = build_side_labeled(&oracle, Path::new("/corpus"), None, in_set);
        let dartq_side = build_side_labeled(&dartq, Path::new("/corpus"), None, in_set);
        assert_eq!(oracle_side.pre_filter_total, 3);
        assert_eq!(oracle_side.total_diags, 2);
        assert_eq!(oracle_side.dropped_by_filter, 1);
        assert_eq!(dartq_side.dropped_by_filter, 0);

        let cmp = compare(&oracle_side, &dartq_side);
        assert_eq!(cmp.oracle_pre_filter, 3);
        assert_eq!(cmp.oracle_dropped, 1);
        assert_eq!(cmp.dartq_pre_filter, 1);
        assert_eq!(cmp.dartq_dropped, 0);
        assert_eq!(cmp.error_jaccard(), 1.0);
    }
}
