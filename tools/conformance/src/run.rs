//! `conformance run`: run dartq over the same materialized cases, compare
//! each case against the oracle expectations, and write the failure report
//! plus the tracked status board. A full (unfiltered) run regenerates
//! `conformance/status.tsv`; a filtered run only produces the per-case
//! report, since the board needs the whole corpus.

use crate::materialize::{self, LoadedCase};
use crate::registry::Registry;
use crate::rows::{diff_sorted, Expected, Row};
use crate::runner::{self, Runner};
use crate::status::{self, Comparison};
use std::collections::{BTreeMap, BTreeSet};
use std::io::Write as _;
use std::path::{Path, PathBuf};

pub struct RunArgs {
    pub dartq_bin: PathBuf,
    /// Released SDK root passed to dartq as `--sdk-path` (optional; dartq
    /// falls back to `$DART_SDK`).
    pub sdk: Option<PathBuf>,
    pub cases_dir: PathBuf,
    /// The hand-written manual corpus loaded alongside `cases_dir`
    /// (`None` disables it).
    pub manual_dir: Option<PathBuf>,
    pub mocks_root: PathBuf,
    /// `conformance/expected.tsv`.
    pub expected_path: PathBuf,
    /// Registry file paths.
    pub codes_tsv: PathBuf,
    pub rule_meta_rs: PathBuf,
    pub features_yaml: PathBuf,
    /// `target/conformance/report.tsv`.
    pub report_path: PathBuf,
    /// `conformance/status.tsv`.
    pub status_path: PathBuf,
    pub work_root: PathBuf,
    pub filter: Option<String>,
    pub chunk: usize,
}

pub fn run_command(args: &RunArgs) -> i32 {
    let started = std::time::Instant::now();
    if !args.dartq_bin.is_file() {
        eprintln!("error: dartq binary {} not found", args.dartq_bin.display());
        return 2;
    }
    let expected = match std::fs::read_to_string(&args.expected_path) {
        Ok(text) => parse_expected(&text),
        Err(_) => {
            eprintln!(
                "error: {} not found; run `conformance oracle` first",
                args.expected_path.display()
            );
            return 2;
        }
    };
    let all_cases = match materialize::load_corpus(&args.cases_dir, args.manual_dir.as_deref()) {
        Ok(cases) => cases,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };
    let selected: Vec<usize> = all_cases
        .iter()
        .enumerate()
        .filter(|(_, loaded)| match &args.filter {
            Some(prefix) => loaded.case.id.starts_with(prefix.as_str()),
            None => true,
        })
        .map(|(i, _)| i)
        .collect();
    if selected.is_empty() {
        eprintln!("error: filter selects no cases");
        return 2;
    }

    // Cases the oracle itself crashed on are excluded from both sides.
    let runnable: Vec<usize> = selected
        .iter()
        .copied()
        .filter(|i| !matches!(expected.get(&all_cases[*i].case.id), Some(Expected::Crash) | None))
        .collect();
    let runner = Runner {
        cases: &all_cases,
        chunk_size: args.chunk,
        work_root: args.work_root.clone(),
        label: "run",
        mocks_root: args.mocks_root.clone(),
        timeout: crate::watchdog::CHUNK_TIMEOUT,
    };
    let build = |chunk_dir: &Path| runner::dartq_command(&args.dartq_bin, args.sdk.as_deref(), chunk_dir);
    let analysis = runner.analyze(&runnable, &build);

    let mut actual: BTreeMap<String, Vec<Row>> = BTreeMap::new();
    let mut crashed: BTreeSet<String> = BTreeSet::new();
    for &i in &runnable {
        let id = all_cases[i].case.id.clone();
        if analysis.crashed.contains(&i) {
            crashed.insert(id);
        } else {
            actual.insert(id, analysis.rows.get(&i).cloned().unwrap_or_default());
        }
    }

    let report = write_report(args, &all_cases, &selected, &expected, &actual, &crashed);
    if let Err(e) = report {
        eprintln!("error: {e}");
        return 2;
    }

    if args.filter.is_none() {
        let registry = match read_registry(args) {
            Ok(registry) => registry,
            Err(e) => {
                eprintln!("error: {e}");
                return 2;
            }
        };
        let comparison = Comparison::new(expected, actual, crashed);
        let stats = status::compute(&registry, &all_cases, &comparison);
        if let Err(e) = std::fs::write(&args.status_path, status::render(&stats)) {
            eprintln!("error: cannot write {}: {e}", args.status_path.display());
            return 2;
        }
        let (pass, fail, crash): (u32, u32, u32) = stats
            .iter()
            .fold((0, 0, 0), |(p, f, c), s| (p + s.pass, f + s.fail, c + s.crash));
        let greens = stats.iter().filter(|s| s.state == "green").count();
        let covered = stats.iter().filter(|s| s.state != "uncovered").count();
        let _ = writeln!(
            std::io::stderr().lock(),
            "status: {} entries, {covered} covered, {greens} green; per-case pass={pass} fail={fail} crash={crash} -> {} in {:.1}s",
            stats.len(),
            args.status_path.display(),
            started.elapsed().as_secs_f64(),
        );
    } else {
        let _ = writeln!(
            std::io::stderr().lock(),
            "run (filtered): report only; status.tsv needs an unfiltered run, {:.1}s",
            started.elapsed().as_secs_f64(),
        );
    }
    0
}

struct CaseVerdict {
    verdict: &'static str,
    missing: String,
    extra: String,
    key_match: bool,
}

fn verdict_for(
    loaded: &LoadedCase,
    expected_map: &BTreeMap<String, Expected>,
    actual: &BTreeMap<String, Vec<Row>>,
    crashed: &BTreeSet<String>,
) -> CaseVerdict {
    let id = &loaded.case.id;
    if crashed.contains(id) {
        return CaseVerdict {
            verdict: "crash",
            missing: String::new(),
            extra: String::new(),
            key_match: false,
        };
    }
    let exp_rows = match expected_map.get(id) {
        Some(Expected::Rows(rows)) => rows,
        _ => {
            return CaseVerdict {
                verdict: "fail",
                missing: "<no expected rows>".to_string(),
                extra: String::new(),
                key_match: false,
            }
        }
    };
    let act_rows = actual.get(id).map(Vec::as_slice).unwrap_or(&[]);
    let (missing, extra) = diff_sorted(exp_rows, act_rows);
    if missing.is_empty() && extra.is_empty() {
        return CaseVerdict {
            verdict: "pass",
            missing: String::new(),
            extra: String::new(),
            key_match: true,
        };
    }
    let key_match = key_set(exp_rows) == key_set(act_rows);
    CaseVerdict {
        verdict: "fail",
        missing: missing.iter().map(|r| r.text()).collect::<Vec<_>>().join(";;"),
        extra: extra.iter().map(|r| r.text()).collect::<Vec<_>>().join(";;"),
        key_match,
    }
}

fn key_set(rows: &[Row]) -> BTreeSet<(String, String, String, u32, u32)> {
    rows.iter().map(Row::key).collect()
}

fn write_report(
    args: &RunArgs,
    all_cases: &[LoadedCase],
    selected: &[usize],
    expected_map: &BTreeMap<String, Expected>,
    actual: &BTreeMap<String, Vec<Row>>,
    crashed: &BTreeSet<String>,
) -> Result<(), String> {
    let mut text = String::from("case_id\tverdict\tmissing\textra\n");
    let mut passes = 0usize;
    let mut fails = 0usize;
    let mut crashes = 0usize;
    let mut oracle_crashes = 0usize;
    let mut key_matches = 0usize;
    for &i in selected {
        let loaded = &all_cases[i];
        let id = &loaded.case.id;
        if matches!(expected_map.get(id), Some(Expected::Crash) | None) {
            oracle_crashes += 1;
            text.push_str(&format!("{id}\toracle_crash\t\t\n"));
            continue;
        }
        let verdict = verdict_for(loaded, expected_map, actual, crashed);
        match verdict.verdict {
            "pass" => passes += 1,
            "crash" => crashes += 1,
            _ => {
                fails += 1;
                if verdict.key_match {
                    key_matches += 1;
                }
            }
        }
        text.push_str(&format!(
            "{id}\t{}\t{}\t{}\n",
            verdict.verdict, verdict.missing, verdict.extra
        ));
    }
    if let Some(parent) = args.report_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    std::fs::write(&args.report_path, &text)
        .map_err(|e| format!("cannot write {}: {e}", args.report_path.display()))?;
    let _ = writeln!(
        std::io::stderr().lock(),
        "report: {passes} pass, {fails} fail ({key_matches} key-only match, message diffs), {crashes} dartq crashes, {oracle_crashes} oracle-crashed -> {}",
        args.report_path.display()
    );
    Ok(())
}

/// Loads the three registries with real read errors (not silently empty
/// inputs).
fn read_registry(args: &RunArgs) -> Result<Registry, String> {
    let read = |path: &Path| {
        std::fs::read_to_string(path).map_err(|e| format!("cannot read {}: {e}", path.display()))
    };
    let codes = read(&args.codes_tsv)?;
    let meta = read(&args.rule_meta_rs)?;
    let features = read(&args.features_yaml)?;
    let registry = Registry::load(&codes, &meta, &features)?;
    if registry.is_empty() {
        return Err("the registries define no entries".to_string());
    }
    Ok(registry)
}

fn parse_expected(text: &str) -> BTreeMap<String, Expected> {
    text.lines()
        .filter(|line| !line.is_empty())
        .filter_map(|line| {
            let (id, cell) = line.split_once('\t')?;
            Expected::parse(cell).map(|e| (id.to_string(), e))
        })
        .collect()
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

    fn loaded(id: &str) -> LoadedCase {
        LoadedCase {
            case: Case {
                id: id.into(),
                source: String::new(),
                language: None,
                packages: vec![],
                aux_packages: vec![],
                options: None,
                ignores: vec![],
                files: vec![("lib/test.dart".into(), "var x = 1;\n".into())],
            },
            hash: id.into(),
        }
    }

    #[test]
    fn verdicts_cover_every_shape() {
        let case = loaded("diagnostics/a/t");
        let mut expected = BTreeMap::new();
        expected.insert(
            "diagnostics/a/t".to_string(),
            Expected::Rows(vec![row("ERROR", "c", "lib/test.dart", 1, 1, 1, "m")]),
        );

        // Exact pass.
        let mut actual = BTreeMap::new();
        actual.insert(
            "diagnostics/a/t".to_string(),
            vec![row("ERROR", "c", "lib/test.dart", 1, 1, 1, "m")],
        );
        let verdict = verdict_for(&case, &expected, &actual, &BTreeSet::new());
        assert_eq!(verdict.verdict, "pass");

        // Message-only diff: fail, key match.
        let mut actual = BTreeMap::new();
        actual.insert(
            "diagnostics/a/t".to_string(),
            vec![row("ERROR", "c", "lib/test.dart", 1, 1, 1, "other")],
        );
        let verdict = verdict_for(&case, &expected, &actual, &BTreeSet::new());
        assert_eq!(verdict.verdict, "fail");
        assert!(verdict.key_match);
        assert!(verdict.extra.contains("other"));

        // Position diff: fail without key match.
        let mut actual = BTreeMap::new();
        actual.insert(
            "diagnostics/a/t".to_string(),
            vec![row("ERROR", "c", "lib/test.dart", 2, 1, 1, "m")],
        );
        let verdict = verdict_for(&case, &expected, &actual, &BTreeSet::new());
        assert_eq!(verdict.verdict, "fail");
        assert!(!verdict.key_match);

        // dartq crash.
        let crashed = BTreeSet::from(["diagnostics/a/t".to_string()]);
        let verdict = verdict_for(&case, &expected, &BTreeMap::new(), &crashed);
        assert_eq!(verdict.verdict, "crash");

        // Missing dartq rows entirely: every expected row is missing.
        let verdict = verdict_for(&case, &expected, &BTreeMap::new(), &BTreeSet::new());
        assert_eq!(verdict.verdict, "fail");
        assert!(verdict.missing.contains("lib/test.dart"));
        assert!(verdict.extra.is_empty());
    }

    #[test]
    fn parse_expected_reads_the_tracked_file() {
        let text = "diagnostics/a/t\t-\ndiagnostics/b/t\toracle_crash\n";
        let map = parse_expected(text);
        assert_eq!(map.len(), 2);
        assert!(matches!(map["diagnostics/b/t"], Expected::Crash));
    }
}
