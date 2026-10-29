//! The status board: one row per registry entry, attributed from the
//! per-case comparison; `locked.txt` maintenance and the gate check.
//!
//! Attribution: a case counts for its id-prefix entry
//! (`diagnostics/<code>/…` → that code) and for every code appearing in
//! either side's rows. It passes for an entry iff its rows for that entry
//! match exactly on both sides; a case whose id prefix names an entry counts
//! for it even when neither side reports the code. Prefix families resolve
//! case-insensitively (the SDK corpus spells some lint families in camelCase
//! where the registry folds them to lowercase), and census-verified corpus
//! grouping ids (`exclusions`) get no row by design and never warn.

use crate::materialize::LoadedCase;
use crate::registry::{Entry, Kind, Registry};
use crate::rows::{Expected, Row};
use std::collections::{BTreeMap, BTreeSet};

pub const STATUS_HEADER: &str = "kind\tid\tcases\tpass\tfail\tcrash\tspec_drift\tstate";

#[derive(Clone)]
pub struct EntryStat {
    pub kind: Kind,
    pub id: String,
    pub cases: u32,
    pub pass: u32,
    pub fail: u32,
    pub crash: u32,
    pub spec_drift: u32,
    pub state: &'static str,
}

/// Input for one run comparison: dartq rows and crashes per case id, and
/// the oracle expectations. Cases expected to be `oracle_crash` are excluded
/// from every entry (listed in the report instead).
pub struct Comparison {
    pub expected: BTreeMap<String, Expected>,
    pub actual: BTreeMap<String, Vec<Row>>,
    pub crashed: BTreeSet<String>,
}

impl Comparison {
    pub fn new(
        expected: BTreeMap<String, Expected>,
        actual: BTreeMap<String, Vec<Row>>,
        crashed: BTreeSet<String>,
    ) -> Comparison {
        Comparison {
            expected,
            actual,
            crashed,
        }
    }
}

/// Computes the full status board over the registry.
pub fn compute(registry: &Registry, cases: &[LoadedCase], run: &Comparison) -> Vec<EntryStat> {
    let mut entries = registry.entries.clone();
    let mut unregistered: BTreeSet<String> = BTreeSet::new();
    for loaded in cases {
        let id = &loaded.case.id;
        let expected = match run.expected.get(id) {
            Some(Expected::Crash) | None => continue,
            Some(Expected::Rows(rows)) => rows,
        };
        let actual = run.actual.get(id).map(Vec::as_slice).unwrap_or(&[]);
        let is_crash = run.crashed.contains(id);
        let drift = spec_drift(loaded, expected);

        // Attributed entries: the id prefix plus every code on either side.
        let mut attributed: BTreeSet<(Kind, String)> = BTreeSet::new();
        if let Some(key) = prefix_entry(id) {
            attributed.insert(key);
        }
        for row in expected.iter().chain(actual.iter()) {
            if !row.code.is_empty() {
                attributed.insert((Kind::Diagnostic, row.code.clone()));
                attributed.insert((Kind::Lint, row.code.clone()));
            }
        }
        for key in &attributed {
            let Some(entry) = resolve(&entries, key).and_then(|k| entries.get_mut(&k)) else {
                // A code registered under neither the diagnostic nor the lint
                // namespace has no status row; feature ids only ever arrive
                // through case prefixes. Census-verified corpus grouping ids
                // are accounted exclusions, not gaps.
                if is_unregistered(&entries, key) {
                    unregistered.insert(key.1.clone());
                }
                continue;
            };
            entry.cases += 1;
            if is_crash {
                entry.crash += 1;
            } else {
                let code = &key.1;
                let matches = rows_for(expected, code) == rows_for(actual, code);
                if matches {
                    entry.pass += 1;
                } else {
                    entry.fail += 1;
                }
            }
            if drift {
                entry.spec_drift += 1;
            }
        }
    }
    if !unregistered.is_empty() {
        let sample: Vec<&String> = unregistered.iter().take(5).collect();
        eprintln!(
            "warn: {} case codes/families match no registry entry (no status row), e.g. {}",
            unregistered.len(),
            sample
                .iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    entries
        .into_iter()
        .map(|((kind, id), entry)| EntryStat {
            kind,
            state: state_of(&entry),
            id,
            cases: entry.cases,
            pass: entry.pass,
            fail: entry.fail,
            crash: entry.crash,
            spec_drift: entry.spec_drift,
        })
        .collect()
}

/// `diagnostics/<id>/…` | `lints/<id>/…` | `features/<id>/…`, with an
/// optional `manual/` infix for hand-written cases (whose kind is spelled
/// singular: `manual/diagnostic/<code>/…`).
fn prefix_entry(id: &str) -> Option<(Kind, String)> {
    let id = id.strip_prefix("manual/").unwrap_or(id);
    let mut parts = id.split('/');
    let kind = match parts.next()? {
        "diagnostics" | "diagnostic" => Kind::Diagnostic,
        "lints" | "lint" => Kind::Lint,
        "features" => Kind::Feature,
        _ => return None,
    };
    Some((kind, parts.next()?.to_string()))
}

/// Whether any registry kind carries this id.
fn known_code(entries: &BTreeMap<(Kind, String), Entry>, code: &str) -> bool {
    entries
        .keys()
        .any(|(_, id)| id == code)
}

/// The registry key an attributed key lands on: an exact hit, else a
/// case-folded hit (the SDK corpus spells some lint families in camelCase
/// where the registry folds them to lowercase).
fn resolve(entries: &BTreeMap<(Kind, String), Entry>, key: &(Kind, String)) -> Option<(Kind, String)> {
    if entries.contains_key(key) {
        return Some(key.clone());
    }
    let folded = key.1.to_lowercase();
    if folded != key.1 && entries.contains_key(&(key.0, folded.clone())) {
        return Some((key.0, folded));
    }
    None
}

/// Whether a registry-missing attributed key is a genuine accounting gap
/// (warned) rather than a designed exclusion.
fn is_unregistered(entries: &BTreeMap<(Kind, String), Entry>, key: &(Kind, String)) -> bool {
    !matches!(key, (Kind::Feature, _))
        && !known_code(entries, &key.1)
        && !crate::exclusions::is_excluded(&key.1)
}

fn rows_for<'a>(rows: &'a [Row], code: &str) -> Vec<&'a Row> {
    rows.iter().filter(|r| r.code == code).collect()
}

/// The case's inline markers disagree with the oracle on (code, line, col)
/// in any file section.
fn spec_drift(loaded: &LoadedCase, expected: &[Row]) -> bool {
    for (path, content) in &loaded.case.files {
        let file = path.trim_start_matches('/');
        let markers: BTreeSet<(String, u32, u32)> =
            crate::markers::extract_expectations(content)
                .into_iter()
                .map(|e| {
                    (
                        e.code,
                        u32::try_from(e.line).unwrap_or(u32::MAX),
                        e.column as u32,
                    )
                })
                .collect();
        let oracle: BTreeSet<(String, u32, u32)> = expected
            .iter()
            .filter(|r| r.file == file)
            .map(|r| (r.code.clone(), r.line, r.column))
            .collect();
        if markers != oracle {
            return true;
        }
    }
    false
}

fn state_of(entry: &Entry) -> &'static str {
    if entry.cases == 0 {
        "uncovered"
    } else if entry.fail == 0 && entry.crash == 0 {
        "green"
    } else {
        "red"
    }
}

/// Renders the status board (header + sorted rows).
pub fn render(stats: &[EntryStat]) -> String {
    let mut out = String::from(STATUS_HEADER);
    out.push('\n');
    for stat in stats {
        out.push_str(&format!(
            "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
            stat.kind.name(),
            stat.id,
            stat.cases,
            stat.pass,
            stat.fail,
            stat.crash,
            stat.spec_drift,
            stat.state
        ));
    }
    out
}

/// Parses a rendered status board back to rows
/// (`kind id cases pass fail crash spec_drift state`).
pub fn parse(text: &str) -> Result<Vec<EntryStat>, String> {
    let mut stats = Vec::new();
    for (no, line) in text.lines().enumerate() {
        if line.is_empty() || line == STATUS_HEADER {
            continue;
        }
        let fields: Vec<&str> = line.split('\t').collect();
        if fields.len() != 8 {
            return Err(format!("status line {}: expected 8 columns", no + 1));
        }
        let kind = match fields[0] {
            "diagnostic" => Kind::Diagnostic,
            "lint" => Kind::Lint,
            "feature" => Kind::Feature,
            other => return Err(format!("status line {}: unknown kind `{other}`", no + 1)),
        };
        let nums = fields[2..7]
            .iter()
            .map(|f| f.parse::<u32>().map_err(|_| format!("status line {}: bad number `{f}`", no + 1)))
            .collect::<Result<Vec<u32>, _>>()?;
        stats.push(EntryStat {
            kind,
            id: fields[1].to_string(),
            cases: nums[0],
            pass: nums[1],
            fail: nums[2],
            crash: nums[3],
            spec_drift: nums[4],
            state: match fields[7] {
                "green" => "green",
                "red" => "red",
                "uncovered" => "uncovered",
                other => return Err(format!("status line {}: unknown state `{other}`", no + 1)),
            },
        });
    }
    Ok(stats)
}

/// The `kind/id` spelling used by `locked.txt`.
pub fn locked_id(stat: &EntryStat) -> String {
    format!("{}/{}", stat.kind.name(), stat.id)
}

/// Monotonic lock merge: every currently green entry is added, existing
/// entries are never removed, output is sorted and deduplicated.
pub fn lock(existing_text: &str, stats: &[EntryStat]) -> String {
    let mut locked: BTreeSet<String> = existing_text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect();
    let mut added = 0usize;
    for stat in stats {
        if stat.state == "green" {
            let id = locked_id(stat);
            if locked.insert(id) {
                added += 1;
            }
        }
    }
    eprintln!("lock: {added} newly green entries added, {} already locked", locked.len() - added);
    let mut lines: Vec<String> = locked.into_iter().collect();
    lines.sort();
    let mut out = lines.join("\n");
    if !out.is_empty() {
        out.push('\n');
    }
    out
}

/// The gate: every locked entry must be green, and no case may have
/// crashed. Returns the violation lines (empty = pass).
pub fn gate(stats: &[EntryStat], locked_text: &str) -> Vec<String> {
    let mut violations = Vec::new();
    let by_id: BTreeMap<String, &EntryStat> = stats
        .iter()
        .map(|stat| (locked_id(stat), stat))
        .collect();
    for line in locked_text.lines().filter(|l| !l.trim().is_empty()) {
        match by_id.get(line.trim()) {
            Some(stat) if stat.state == "green" => {}
            Some(stat) => violations.push(format!(
                "locked entry {line} is {}: cases={} pass={} fail={} crash={}",
                stat.state, stat.cases, stat.pass, stat.fail, stat.crash
            )),
            None => violations.push(format!("locked entry {line} has no status row")),
        }
    }
    let total_crashes: u32 = stats.iter().map(|s| s.crash).sum();
    if total_crashes > 0 {
        violations.push(format!("{total_crashes} case crashes across entries"));
    }
    violations
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::case::Case;

    fn row(sev: &str, code: &str, file: &str, line: u32, col: u32, len: u64, msg: &str) -> Row {
        Row {
            severity: sev.into(),
            code: code.into(),
            file: file.into(),
            line,
            column: col,
            length: len,
            message: msg.into(),
        }
    }

    fn loaded(id: &str, content: &str) -> LoadedCase {
        LoadedCase {
            case: Case {
                id: id.into(),
                source: String::new(),
                language: None,
                packages: vec![],
                aux_packages: vec![],
                options: None,
                ignores: vec![],
                files: vec![("lib/test.dart".into(), content.into())],
            },
            hash: id.into(),
        }
    }

    fn registry() -> Registry {
        Registry::load(
            "C\tcode_a\tX\tY\tZ\ttrue\tm\tf\nC\tcode_b\tX\tY\tZ\ttrue\tm\tf\n",
            "RuleMeta { name: \"lint_x\", group: Group::Standalone },\n",
            "features:\n  feat_x:\n    help: h\n",
        )
        .expect("registry")
    }

    fn stats_map(stats: &[EntryStat]) -> BTreeMap<String, (u32, u32, u32, u32, u32, &'static str)> {
        stats
            .iter()
            .map(|s| {
                (
                    locked_id(s),
                    (s.cases, s.pass, s.fail, s.crash, s.spec_drift, s.state),
                )
            })
            .collect()
    }

    #[test]
    fn attribution_prefix_counts_even_when_silent() {
        let cases = vec![loaded("diagnostics/code_a/silent", "var x = 1;\n")];
        let run = Comparison::new(
            BTreeMap::from([("diagnostics/code_a/silent".to_string(), Expected::Rows(vec![]))]),
            BTreeMap::from([("diagnostics/code_a/silent".to_string(), vec![])]),
            BTreeSet::new(),
        );
        let map = stats_map(&compute(&registry(), &cases, &run));
        assert_eq!(map["diagnostic/code_a"], (1, 1, 0, 0, 0, "green"));
        assert_eq!(map["diagnostic/code_b"], (0, 0, 0, 0, 0, "uncovered"));
        assert_eq!(map["lint/lint_x"], (0, 0, 0, 0, 0, "uncovered"));
    }

    #[test]
    fn manual_prefix_attributes_to_the_code_entry() {
        let cases = vec![loaded(
            "manual/diagnostic/code_a/1",
            "var x = 1;\n//     ^^^\n// [diag.code_a] m\n",
        )];
        let rows = vec![row("WARNING", "code_a", "pubspec.yaml", 1, 1, 1, "m")];
        let run = Comparison::new(
            BTreeMap::from([(
                "manual/diagnostic/code_a/1".to_string(),
                Expected::Rows(rows.clone()),
            )]),
            BTreeMap::from([("manual/diagnostic/code_a/1".to_string(), rows)]),
            BTreeSet::new(),
        );
        let map = stats_map(&compute(&registry(), &cases, &run));
        assert_eq!(map["diagnostic/code_a"], (1, 1, 0, 0, 1, "green"));
        assert_eq!(map["diagnostic/code_b"], (0, 0, 0, 0, 0, "uncovered"));
    }

    #[test]
    fn attribution_by_row_codes_on_both_sides() {
        let cases = vec![loaded("diagnostics/code_a/t", "var x = 1;\n")];
        let run = Comparison::new(
            BTreeMap::from([(
                "diagnostics/code_a/t".to_string(),
                Expected::Rows(vec![row("ERROR", "code_a", "lib/test.dart", 1, 1, 1, "m")]),
            )]),
            BTreeMap::from([(
                "diagnostics/code_a/t".to_string(),
                vec![row("ERROR", "code_b", "lib/test.dart", 1, 1, 1, "m")],
            )]),
            BTreeSet::new(),
        );
        let map = stats_map(&compute(&registry(), &cases, &run));
        // code_a: expected has exactly its rows; actual (code_b) mismatches.
        // Marker-less content vs a non-empty oracle = spec drift.
        assert_eq!(map["diagnostic/code_a"], (1, 0, 1, 0, 1, "red"));
        // code_b appears only in actual rows, still attributed and failing.
        assert_eq!(map["diagnostic/code_b"], (1, 0, 1, 0, 1, "red"));
    }

    #[test]
    fn per_code_pass_is_independent_per_entry() {
        let cases = vec![loaded("diagnostics/code_a/t", "var x = 1;\n")];
        let rows = |code: &str| {
            vec![
                row("ERROR", code, "lib/test.dart", 1, 1, 1, "m"),
                row("WARNING", "lint_x", "lib/test.dart", 2, 1, 1, "m"),
            ]
        };
        let run = Comparison::new(
            BTreeMap::from([("diagnostics/code_a/t".to_string(), Expected::Rows(rows("code_a")))]),
            BTreeMap::from([("diagnostics/code_a/t".to_string(), rows("code_a"))]),
            BTreeSet::new(),
        );
        let map = stats_map(&compute(&registry(), &cases, &run));
        assert_eq!(map["diagnostic/code_a"], (1, 1, 0, 0, 1, "green"));
        // The lint code appears in rows of a case with no lints/ prefix.
        assert_eq!(map["lint/lint_x"], (1, 1, 0, 0, 1, "green"));
    }

    #[test]
    fn crash_counts_for_every_attributed_entry() {
        let cases = vec![loaded("diagnostics/code_a/t", "var x = 1;\n")];
        let run = Comparison::new(
            BTreeMap::from([(
                "diagnostics/code_a/t".to_string(),
                Expected::Rows(vec![row("ERROR", "code_b", "lib/test.dart", 1, 1, 1, "m")]),
            )]),
            BTreeMap::new(),
            BTreeSet::from(["diagnostics/code_a/t".to_string()]),
        );
        let map = stats_map(&compute(&registry(), &cases, &run));
        assert_eq!(map["diagnostic/code_a"], (1, 0, 0, 1, 1, "red"));
        assert_eq!(map["diagnostic/code_b"], (1, 0, 0, 1, 1, "red"));
    }

    #[test]
    fn oracle_crash_cases_are_excluded() {
        let cases = vec![loaded("diagnostics/code_a/t", "var x = 1;\n")];
        let run = Comparison::new(
            BTreeMap::from([("diagnostics/code_a/t".to_string(), Expected::Crash)]),
            BTreeMap::new(),
            BTreeSet::new(),
        );
        let map = stats_map(&compute(&registry(), &cases, &run));
        assert_eq!(map["diagnostic/code_a"], (0, 0, 0, 0, 0, "uncovered"));
    }

    #[test]
    fn spec_drift_detects_marker_disagreement() {
        // Marker promises a diagnostic the oracle does not report.
        let marked = "var x = 1;\n//     ^^^\n// [diag.code_a] m\n";
        let cases = vec![loaded("diagnostics/code_a/t", marked)];
        let run = Comparison::new(
            BTreeMap::from([("diagnostics/code_a/t".to_string(), Expected::Rows(vec![]))]),
            BTreeMap::new(),
            BTreeSet::new(),
        );
        let map = stats_map(&compute(&registry(), &cases, &run));
        assert_eq!(map["diagnostic/code_a"], (1, 1, 0, 0, 1, "green"));
        // Matching marker: no drift. The caret run `^^^` starts at column 7
        // and its expectation line follows retained line 0.
        let matching = vec![row("ERROR", "code_a", "lib/test.dart", 0, 7, 3, "m")];
        let run = Comparison::new(
            BTreeMap::from([("diagnostics/code_a/t".to_string(), Expected::Rows(matching.clone()))]),
            BTreeMap::from([("diagnostics/code_a/t".to_string(), matching)]),
            BTreeSet::new(),
        );
        let map = stats_map(&compute(&registry(), &cases, &run));
        assert_eq!(map["diagnostic/code_a"], (1, 1, 0, 0, 0, "green"));
    }

    #[test]
    fn message_only_diff_is_a_full_row_fail() {
        let cases = vec![loaded("diagnostics/code_a/t", "var x = 1;\n")];
        let run = Comparison::new(
            BTreeMap::from([(
                "diagnostics/code_a/t".to_string(),
                Expected::Rows(vec![row("ERROR", "code_a", "lib/test.dart", 1, 1, 1, "oracle text")]),
            )]),
            BTreeMap::from([(
                "diagnostics/code_a/t".to_string(),
                vec![row("ERROR", "code_a", "lib/test.dart", 1, 1, 1, "dartq text")],
            )]),
            BTreeSet::new(),
        );
        let map = stats_map(&compute(&registry(), &cases, &run));
        assert_eq!(map["diagnostic/code_a"], (1, 0, 1, 0, 1, "red"));
    }

    #[test]
    fn render_parse_roundtrip_and_gate_and_lock() {
        let cases: Vec<LoadedCase> = vec![
            loaded("diagnostics/code_a/t", "var x = 1;\n"),
            loaded("lints/lint_x/t", "var x = 1;\n"),
        ];
        let run = Comparison::new(
            BTreeMap::from([
                ("diagnostics/code_a/t".to_string(), Expected::Rows(vec![])),
                ("lints/lint_x/t".to_string(), Expected::Rows(vec![])),
            ]),
            BTreeMap::new(),
            BTreeSet::new(),
        );
        let stats = compute(&registry(), &cases, &run);
        let text = render(&stats);
        assert!(text.starts_with(STATUS_HEADER));
        let parsed = parse(&text).expect("parses");
        assert_eq!(render(&parsed), text);

        // Lock: greens enter, non-greens do not; monotonic across runs.
        let locked = lock("", &stats);
        assert_eq!(locked, "diagnostic/code_a\nlint/lint_x\n");
        let after = lock(&locked, &stats);
        assert_eq!(after, locked);
        // A stale lock line survives even when its entry is red now.
        let mut red = stats.clone();
        for stat in &mut red {
            if stat.id == "code_a" {
                stat.fail = 1;
                stat.state = "red";
            }
        }
        assert!(lock(&locked, &red).contains("diagnostic/code_a"));
        // Gate: red locked entry is a violation; unlocked red is not.
        let violations = gate(&red, &locked);
        assert_eq!(violations.len(), 1);
        assert!(violations[0].contains("diagnostic/code_a"));
        // A crash anywhere fails the gate even when green.
        let mut crashed = stats.clone();
        for stat in &mut crashed {
            if stat.id == "lint_x" {
                stat.crash = 2;
                stat.state = "red";
            }
        }
        assert_eq!(gate(&crashed, &locked).len(), 2);
        assert!(gate(&stats, &locked).is_empty());
    }

    #[test]
    fn parse_rejects_malformed_rows() {
        assert!(parse("nonsense").is_err());
        assert!(parse("diagnostic\tx\t1\t2\t3\t4\t5\tbogus").is_err());
        assert!(parse("kind\tx\t1\t2\t3\t4\t5\tgreen").is_err());
    }

    #[test]
    fn camel_case_family_resolves_to_folded_registry_entry() {
        let reg = Registry::load(
            "",
            "RuleMeta { name: \"prefer_iterable_wheretype\", group: Group::Standalone },\n",
            "features:\n",
        )
        .expect("registry");
        let cases = vec![loaded("lints/prefer_iterable_whereType/t", "var x = 1;\n")];
        let run = Comparison::new(
            BTreeMap::from([("lints/prefer_iterable_whereType/t".to_string(), Expected::Rows(vec![]))]),
            BTreeMap::new(),
            BTreeSet::new(),
        );
        let map = stats_map(&compute(&reg, &cases, &run));
        assert_eq!(map["lint/prefer_iterable_wheretype"], (1, 1, 0, 0, 0, "green"));
        assert!(!map.contains_key("lint/prefer_iterable_whereType"));
    }

    #[test]
    fn grouping_label_family_is_excluded_not_warned() {
        // `constructor_body` is a census-verified grouping label: no
        // registry entry, cases emit other codes, no warn, no status row.
        assert!(crate::exclusions::is_excluded("constructor_body"));
        let reg = registry();
        let entries = &reg.entries;
        let key = (Kind::Diagnostic, "constructor_body".to_string());
        assert!(resolve(entries, &key).is_none());
        assert!(!is_unregistered(entries, &key));
        // The same shape with a non-excluded unknown code is a genuine gap.
        let unknown = (Kind::Diagnostic, "totally_unknown_code".to_string());
        assert!(resolve(entries, &unknown).is_none());
        assert!(is_unregistered(entries, &unknown));
        // Feature ids stay outside the warn regardless.
        let feature = (Kind::Feature, "feat_x".to_string());
        assert!(!is_unregistered(entries, &feature));
    }

    #[test]
    fn case_folded_row_code_resolves() {
        let reg = Registry::load(
            "C\tcode_a\tX\tY\tZ\ttrue\tm\tf\n",
            "RuleMeta { name: \"no_runtimetype_tostring\", group: Group::Standalone },\n",
            "features:\n",
        )
        .expect("registry");
        let entries = &reg.entries;
        let key = (Kind::Lint, "no_runtimeType_toString".to_string());
        assert_eq!(resolve(entries, &key), Some((Kind::Lint, "no_runtimetype_tostring".to_string())));
        // Exact hits win without folding.
        let exact = (Kind::Diagnostic, "code_a".to_string());
        assert_eq!(resolve(entries, &exact), Some(exact.clone()));
        assert_eq!(resolve(entries, &(Kind::Diagnostic, "code_A".to_string())), Some(exact));
    }
}
