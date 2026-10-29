//! `conformance oracle`: produce the tracked expected outputs. Cases are
//! materialized into chunks, analyzed by the pinned SDK's `dart analyze`,
//! and their canonical rows land in `conformance/expected.tsv`. Per-case
//! results are cached by content under `target/conformance/oracle-cache/`,
//! so reruns only analyze changed cases.

use crate::materialize;
use crate::rows::{Expected, Row};
use crate::runner::{self, Runner};
use std::collections::{BTreeMap, BTreeSet};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;

pub struct OracleArgs {
    /// The pinned released SDK root (`…/dart-sdk`), providing `bin/dart`.
    pub oracle_sdk: PathBuf,
    pub cases_dir: PathBuf,
    /// The hand-written manual corpus loaded alongside `cases_dir`
    /// (`None` disables it).
    pub manual_dir: Option<PathBuf>,
    pub mocks_root: PathBuf,
    pub pin_path: PathBuf,
    /// `conformance/expected.tsv`.
    pub out_path: PathBuf,
    pub cache_dir: PathBuf,
    pub work_root: PathBuf,
    pub filter: Option<String>,
    pub chunk: usize,
}

pub fn oracle_command(args: &OracleArgs) -> i32 {
    let started = std::time::Instant::now();
    let dart_bin = args.oracle_sdk.join("bin").join("dart");
    if !dart_bin.is_file() {
        eprintln!("error: no `bin/dart` under {}", args.oracle_sdk.display());
        return 2;
    }
    // The cache and `expected.tsv` are keyed by the pin's identity, not by
    // the binary actually run — refuse to analyze with a different SDK so a
    // wrong `--oracle-sdk` cannot silently poison the tracked expectations.
    let pinned = match pinned_sdk_version(&args.pin_path) {
        Ok(version) => version,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };
    let actual = match dart_version(&dart_bin) {
        Ok(version) => version,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };
    if actual != pinned {
        eprintln!(
            "error: the oracle SDK {} reports version {actual}, but {} pins {pinned}; point --oracle-sdk at the pinned SDK — oracle rows are stored under the pin's identity, so a mismatched binary silently writes wrong expectations",
            args.oracle_sdk.display(),
            args.pin_path.display()
        );
        return 2;
    }
    let sdk_revision = match materialize::sdk_revision(&args.pin_path) {
        Ok(revision) => revision,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };
    let mocks_hash = match materialize::mocks_tree_hash(&args.mocks_root) {
        Ok(hash) => hash,
        Err(e) => {
            eprintln!("error: {e}");
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

    // Cache pass: reuse every case whose key already has stored rows.
    let cache = Cache::load(&args.cache_dir);
    let mut expected: BTreeMap<String, Expected> = BTreeMap::new();
    let mut to_analyze: Vec<usize> = Vec::new();
    let mut cached = 0usize;
    for &i in &selected {
        let loaded = &all_cases[i];
        let key = materialize::cache_key(&loaded.hash, &mocks_hash, &sdk_revision);
        match cache.get(&key) {
            Some(rows) => {
                expected.insert(loaded.case.id.clone(), rows);
                cached += 1;
            }
            None => to_analyze.push(i),
        }
    }

    let analysis = if to_analyze.is_empty() {
        eprintln!("oracle: all {} selected cases cached", selected.len());
        crate::runner::Analysis {
            rows: BTreeMap::new(),
            crashed: BTreeSet::new(),
            peak_rss_kb: 0,
            processes: 0,
        }
    } else {
        let runner = Runner {
            cases: &all_cases,
            chunk_size: args.chunk,
            work_root: args.work_root.clone(),
            label: "oracle",
            mocks_root: args.mocks_root.clone(),
            timeout: crate::watchdog::CHUNK_TIMEOUT,
        };
        let build = |chunk_dir: &Path| runner::oracle_command(&dart_bin, chunk_dir);
        let analysis = runner.analyze(&to_analyze, &build);
        // Store every analyzed case in the cache.
        for &i in &to_analyze {
            let loaded = &all_cases[i];
            let key = materialize::cache_key(&loaded.hash, &mocks_hash, &sdk_revision);
            if analysis.crashed.contains(&i) {
                cache.store(&key, "oracle_crash");
            } else {
                let rows = analysis.rows.get(&i).cloned().unwrap_or_default();
                let text = if rows.is_empty() {
                    "-".to_string()
                } else {
                    rows.iter().map(Row::text).collect::<Vec<_>>().join("\n")
                };
                cache.store(&key, &text);
            }
        }
        analysis
    };

    for &i in &selected {
        let loaded = &all_cases[i];
        if analysis.crashed.contains(&i) {
            expected.insert(loaded.case.id.clone(), Expected::Crash);
        } else if let Some(rows) = analysis.rows.get(&i) {
            expected.insert(loaded.case.id.clone(), Expected::Rows(rows.clone()));
        }
        // Otherwise the cache pass already filled this entry.
    }

    // A filtered run merges into the tracked file; a full run rewrites it
    // from the corpus (stale rows for deleted cases drop out).
    if args.filter.is_some() {
        if let Ok(text) = std::fs::read_to_string(&args.out_path) {
            for (id, cell) in parse_expected_tsv(&text) {
                expected.entry(id).or_insert(cell);
            }
        }
    }
    let mut text = String::new();
    for (id, rows) in &expected {
        text.push_str(id);
        text.push('\t');
        text.push_str(&rows.render());
        text.push('\n');
    }
    if let Some(parent) = args.out_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Err(e) = std::fs::write(&args.out_path, &text) {
        eprintln!("error: cannot write {}: {e}", args.out_path.display());
        return 2;
    }
    let crashes = expected
        .values()
        .filter(|e| matches!(e, Expected::Crash))
        .count();
    let rows: usize = expected.values().map(|e| e.rows().len()).sum();
    let _ = writeln!(
        std::io::stderr().lock(),
        "oracle: {} cases ({} cached, {} analyzed, {crashes} oracle crashes), {rows} rows -> {} in {:.1}s",
        expected.len(),
        cached,
        to_analyze.len(),
        args.out_path.display(),
        started.elapsed().as_secs_f64(),
    );
    0
}

fn parse_expected_tsv(text: &str) -> Vec<(String, Expected)> {
    text.lines()
        .filter(|line| !line.is_empty())
        .filter_map(|line| {
            let (id, cell) = line.split_once('\t')?;
            Expected::parse(cell).map(|e| (id.to_string(), e))
        })
        .collect()
}

/// `sdk_version` from `conformance/pin.toml`.
fn pinned_sdk_version(pin_path: &Path) -> Result<String, String> {
    let text =
        std::fs::read_to_string(pin_path).map_err(|e| format!("cannot read pin.toml: {e}"))?;
    materialize::pin_value(&text, "sdk_version")
        .ok_or_else(|| "pin.toml has no `sdk_version`".to_string())
}

/// The version token of a `dart --version` banner
/// (`Dart SDK version: 3.13.7 (stable) … on "macos_arm64"`).
fn version_from_banner(banner: &str) -> Option<String> {
    let rest = banner.strip_prefix("Dart SDK version: ")?;
    let version = rest.split_whitespace().next()?;
    if version.is_empty() {
        None
    } else {
        Some(version.to_string())
    }
}

/// Runs `<sdk>/bin/dart --version` and extracts its version token.
fn dart_version(dart_bin: &Path) -> Result<String, String> {
    let output = Command::new(dart_bin)
        .arg("--version")
        .output()
        .map_err(|e| format!("cannot run {}: {e}", dart_bin.display()))?;
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    version_from_banner(stderr.trim())
        .or_else(|| version_from_banner(stdout.trim()))
        .ok_or_else(|| {
            format!(
                "{} --version printed no recognizable version banner (stderr: {})",
                dart_bin.display(),
                stderr.trim()
            )
        })
}

/// Read/write handle over the per-case cache files.
struct Cache {
    dir: PathBuf,
}

impl Cache {
    fn load(dir: &Path) -> Cache {
        let _ = std::fs::create_dir_all(dir);
        Cache {
            dir: dir.to_path_buf(),
        }
    }

    fn get(&self, key: &str) -> Option<Expected> {
        let text = std::fs::read_to_string(self.dir.join(key)).ok()?;
        match text.trim_end_matches('\n') {
            "-" => Some(Expected::Rows(Vec::new())),
            "oracle_crash" => Some(Expected::Crash),
            "" => Some(Expected::Rows(Vec::new())),
            _ => {
                let mut rows = Vec::new();
                for line in text.lines() {
                    rows.push(Row::parse_text(line)?);
                }
                Some(Expected::Rows(rows))
            }
        }
    }

    fn store(&self, key: &str, content: &str) {
        let _ = std::fs::write(self.dir.join(key), format!("{content}\n"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_roundtrip_all_shapes() {
        let dir = std::env::temp_dir().join(format!("conf-cache-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let cache = Cache::load(&dir);
        assert_eq!(cache.get("missing"), None);
        cache.store("k1", "-");
        assert_eq!(cache.get("k1"), Some(Expected::Rows(Vec::new())));
        cache.store("k2", "oracle_crash");
        assert_eq!(cache.get("k2"), Some(Expected::Crash));
        cache.store(
            "k3",
            "ERROR|code_a|lib/test.dart|1:5|3|m\nINFO|code_b|lib/other.dart|2:1|0|n",
        );
        let rows = match cache.get("k3") {
            Some(Expected::Rows(rows)) => rows,
            other => panic!("{other:?}"),
        };
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[1].code, "code_b");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn expected_tsv_parse_roundtrip() {
        let text = "diagnostics/a/t\t-\ndiagnostics/b/t\toracle_crash\ndiagnostics/c/t\tERROR|code_a|lib/test.dart|1:5|3|m\n";
        let parsed = parse_expected_tsv(text);
        assert_eq!(parsed.len(), 3);
        assert_eq!(parsed[0].1, Expected::Rows(Vec::new()));
        assert_eq!(parsed[1].1, Expected::Crash);
        assert_eq!(parsed[2].1.rows().len(), 1);
    }

    #[test]
    fn version_guard_parses_banner_and_pin() {
        // Real banner shape, plus a fake wrong version to assert mismatch.
        let banner =
            "Dart SDK version: 3.13.7 (stable) (Thu Oct 22 14:28:57 2026) on \"macos_arm64\"";
        assert_eq!(version_from_banner(banner).as_deref(), Some("3.13.7"));
        let fake = "Dart SDK version: 3.13.5 (stable) (fake-build-id) on \"linux_x64\"";
        assert_eq!(version_from_banner(fake).as_deref(), Some("3.13.5"));
        assert_ne!(version_from_banner(fake).as_deref(), Some("3.13.7"));
        assert_eq!(version_from_banner("no banner here"), None);
        assert_eq!(version_from_banner("Dart SDK version:   "), None);

        let dir = std::env::temp_dir().join(format!("conf-pinver-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("creates");
        let pin = dir.join("pin.toml");
        std::fs::write(
            &pin,
            "sdk_version = \"3.13.7\"\nsdk_revision = \"416e33b0\"\n",
        )
        .expect("writes");
        assert_eq!(pinned_sdk_version(&pin).as_deref(), Ok("3.13.7"));
        std::fs::write(&pin, "# no version key\nsdk_revision = \"416e33b0\"\n").expect("writes");
        assert!(pinned_sdk_version(&pin).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
