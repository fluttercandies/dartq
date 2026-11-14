//! Chunked analysis orchestration shared by the oracle and the dartq run:
//! materialize a chunk of package roots, run one analyzer process over the
//! chunk directory, attribute its JSON rows back to cases by package-root
//! index, and bisect a crashed/timeout chunk down to the offending case.
//!
//! Every run gets its own package-root directory ([`fresh_chunk_dir`]): two
//! harness processes — a census beside a filtered probe is the normal case —
//! must never share roots, because the second materialization deletes the
//! first one's files while its analyzer is still walking them.

use crate::analyze_json;
use crate::materialize::{self, LoadedCase};
use crate::rows::Row;
use crate::watchdog;
use std::collections::{BTreeMap, BTreeSet};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

pub struct Analysis {
    /// Case index -> canonical rows (relative files).
    pub rows: BTreeMap<usize, Vec<Row>>,
    /// Case indices attributed to a chunk crash, watchdog kill, timeout or
    /// materialization failure.
    pub crashed: BTreeSet<usize>,
    pub peak_rss_kb: u64,
    pub processes: usize,
    /// A chunk-infrastructure failure (the root cannot be materialized). The
    /// run's verdicts are incomplete, so they must not be reported as a
    /// board: the caller aborts instead of writing `expected.tsv`/`status.tsv`.
    pub fatal: Option<String>,
}

pub struct Runner<'a> {
    pub cases: &'a [LoadedCase],
    pub chunk_size: usize,
    /// Directory for chunk package roots (e.g. `target/conformance/chunks`).
    pub work_root: PathBuf,
    /// Chunk subdirectory label, `oracle` or `run`.
    pub label: &'a str,
    pub mocks_root: PathBuf,
    pub timeout: Duration,
}

enum ChunkOutcome {
    Done {
        rows: BTreeMap<usize, Vec<Row>>,
        materialize_errors: BTreeMap<usize, String>,
    },
    /// The analyzer process crashed, hung or was watchdog-killed.
    Crashed { detail: String },
    /// The chunk root itself is unusable; no case verdict can be derived.
    Fatal { detail: String },
}

impl Runner<'_> {
    /// Analyzes the given case indices (any order; chunked in list order).
    pub fn analyze(&self, indices: &[usize], build: &dyn Fn(&Path) -> Command) -> Analysis {
        let mut analysis = Analysis {
            rows: BTreeMap::new(),
            crashed: BTreeSet::new(),
            peak_rss_kb: 0,
            processes: 0,
            fatal: None,
        };
        let started = Instant::now();
        let total = indices.len();
        // One directory for the whole run: chunks and bisected halves reuse
        // it sequentially, and no other process can observe it.
        let chunk_dir = fresh_chunk_dir(&self.work_root, self.label);
        for (chunk_no, batch) in indices.chunks(self.chunk_size.max(1)).enumerate() {
            let batch_no = chunk_no + 1;
            let batch_total = indices.len().div_ceil(self.chunk_size.max(1));
            let chunk_started = Instant::now();
            match self.run_chunk(batch, &chunk_dir, build, &mut analysis.peak_rss_kb) {
                ChunkOutcome::Done {
                    rows,
                    materialize_errors,
                } => {
                    let chunk_diags: usize = rows.values().map(Vec::len).sum();
                    analysis.rows.extend(rows);
                    for (i, error) in &materialize_errors {
                        analysis.crashed.insert(*i);
                        warn(&format!(
                            "case {} unanalyzable: {error}",
                            self.cases[*i].case.id
                        ));
                    }
                    let _ = writeln!(
                        std::io::stderr().lock(),
                        "{} chunk {batch_no}/{batch_total}: {} cases, {chunk_diags} diags, {:.1}s, peak RSS {:.2} GB",
                        self.label,
                        batch.len(),
                        chunk_started.elapsed().as_secs_f64(),
                        analysis.peak_rss_kb as f64 / (1024.0 * 1024.0),
                    );
                }
                ChunkOutcome::Crashed { detail } => {
                    warn(&format!(
                        "chunk {batch_no}/{batch_total} ({}/{} cases) failed: {}; bisecting",
                        batch.len(),
                        total,
                        detail
                    ));
                    self.bisect(batch, &chunk_dir, build, &mut analysis);
                    if analysis.fatal.is_some() {
                        break;
                    }
                    let _ = writeln!(
                        std::io::stderr().lock(),
                        "{} chunk {batch_no}/{batch_total}: {} cases, bisected, {} crashers so far, {:.1}s",
                        self.label,
                        batch.len(),
                        analysis.crashed.len(),
                        chunk_started.elapsed().as_secs_f64(),
                    );
                }
                ChunkOutcome::Fatal { detail } => {
                    warn(&format!("chunk {batch_no}/{batch_total} failed: {detail}"));
                    analysis.fatal = Some(detail);
                    break;
                }
            }
        }
        remove_chunk_dir(&chunk_dir);
        let _ = writeln!(
            std::io::stderr().lock(),
            "{}: {} cases, {} processes, peak RSS {:.2} GB, {:.1}s total",
            self.label,
            total,
            analysis.processes,
            analysis.peak_rss_kb as f64 / (1024.0 * 1024.0),
            started.elapsed().as_secs_f64(),
        );
        analysis
    }

    fn bisect(
        &self,
        indices: &[usize],
        chunk_dir: &Path,
        build: &dyn Fn(&Path) -> Command,
        analysis: &mut Analysis,
    ) {
        if indices.len() == 1 {
            let i = indices[0];
            analysis.crashed.insert(i);
            warn(&format!("crasher: {}", self.cases[i].case.id));
            return;
        }
        let mid = indices.len() / 2;
        for half in [&indices[..mid], &indices[mid..]] {
            match self.run_chunk(half, chunk_dir, build, &mut analysis.peak_rss_kb) {
                ChunkOutcome::Done {
                    rows,
                    materialize_errors,
                } => {
                    analysis.rows.extend(rows);
                    for (i, error) in &materialize_errors {
                        analysis.crashed.insert(*i);
                        warn(&format!(
                            "case {} unanalyzable: {error}",
                            self.cases[*i].case.id
                        ));
                    }
                }
                ChunkOutcome::Crashed { .. } => self.bisect(half, chunk_dir, build, analysis),
                ChunkOutcome::Fatal { detail } => {
                    analysis.fatal = Some(detail);
                    return;
                }
            }
        }
    }

    fn run_chunk(
        &self,
        indices: &[usize],
        chunk_dir: &Path,
        build: &dyn Fn(&Path) -> Command,
        peak_rss_kb: &mut u64,
    ) -> ChunkOutcome {
        let errors =
            match materialize::materialize(self.cases, indices, chunk_dir, &self.mocks_root) {
                Ok(errors) => errors,
                Err(detail) => return ChunkOutcome::Fatal { detail },
            };
        let runnable: Vec<usize> = indices
            .iter()
            .copied()
            .filter(|i| !errors.contains_key(i))
            .collect();
        if runnable.is_empty() {
            return ChunkOutcome::Done {
                rows: BTreeMap::new(),
                materialize_errors: errors,
            };
        }
        // The analyzers report absolute paths; canonicalize once and use
        // the same prefix for attribution.
        let abs_dir = match std::fs::canonicalize(chunk_dir) {
            Ok(dir) => dir,
            Err(e) => {
                return ChunkOutcome::Fatal {
                    detail: format!("cannot resolve chunk dir {}: {e}", chunk_dir.display()),
                };
            }
        };
        let mut command = build(&abs_dir);
        let result = match watchdog::run(&mut command, self.timeout, watchdog::RSS_LIMIT_KB) {
            Ok(result) => result,
            Err(e) => {
                return ChunkOutcome::Crashed {
                    detail: format!("cannot spawn analyzer: {e}"),
                };
            }
        };
        *peak_rss_kb = (*peak_rss_kb).max(result.peak_rss_kb);
        if result.killed {
            return ChunkOutcome::Crashed {
                detail: "watchdog kill (RSS cap or timeout)".to_string(),
            };
        }
        let stdout = String::from_utf8_lossy(&result.stdout).into_owned();
        let diags = analyze_json::parse_output(&stdout);
        if diags.is_empty() && !envelope_seen(&stdout) {
            let tail: String = {
                let text = String::from_utf8_lossy(&result.stderr);
                let start = text.len().saturating_sub(300);
                text[start..].replace('\n', " | ")
            };
            return ChunkOutcome::Crashed {
                detail: format!("no diagnostics envelope in output; stderr tail: {tail}"),
            };
        }
        ChunkOutcome::Done {
            rows: attribute(&diags, &abs_dir, indices),
            materialize_errors: errors,
        }
    }
}

/// A chunk root no other run can be using: `<work_root>/<label>-<pid>-<n>`.
///
/// The harness used to materialize every run into `<work_root>/<label>`, so
/// two harness processes (a census beside a filtered probe is the normal
/// shape) deleted each other's package roots mid-analysis. The analyzer then
/// hit ENOENT during discovery, the chunk "crashed", bisection re-ran the
/// halves into that same directory, and rows from roots that had already been
/// replaced were dropped as "outside the chunk package roots" — the
/// missing-rows/no-extra-rows signature that reads exactly like a semantic
/// regression.
pub fn fresh_chunk_dir(work_root: &Path, label: &str) -> PathBuf {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    work_root.join(format!("{label}-{}-{n}", std::process::id()))
}

/// Removes this run's chunk roots. A leftover directory is reported rather
/// than swallowed; it can never collide with another run's.
fn remove_chunk_dir(chunk_dir: &Path) {
    if !chunk_dir.is_dir() {
        return;
    }
    if let Err(e) = std::fs::remove_dir_all(chunk_dir) {
        if e.kind() != std::io::ErrorKind::NotFound {
            warn(&format!(
                "cannot remove chunk dir {}: {e}",
                chunk_dir.display()
            ));
        }
    }
}

fn envelope_seen(stdout: &str) -> bool {
    stdout
        .lines()
        .any(|line| line.trim_start().starts_with("{\"version\""))
}

/// Maps absolute diagnostic paths to (case index, rows with relative file).
/// Diagnostics outside any package root of this chunk are counted and
/// dropped (they have no case). Messages that embed the case's package root
/// (the workspace validator reporting the package root, the options walker
/// reporting including-file paths) lose the machine-local prefix — both the
/// oracle and the dartq run get `<chunk>`, so the row is stable across chunk
/// dirs and comparable between the sides.
fn attribute(
    diags: &[analyze_json::Diag],
    chunk_dir: &Path,
    indices: &[usize],
) -> BTreeMap<usize, Vec<Row>> {
    let mut rows: BTreeMap<usize, Vec<Row>> = indices.iter().map(|i| (*i, Vec::new())).collect();
    let mut outside = 0usize;
    for diag in diags {
        let path = Path::new(&diag.file);
        let Some(rel) = split_under_chunk(path, chunk_dir) else {
            outside += 1;
            continue;
        };
        if !rows.contains_key(&rel.0) {
            outside += 1;
            continue;
        }
        // The message embeds the case root in the diagnostic's own spelling
        // of the chunk prefix, so derive the needle from the reported path.
        let root = path
            .ancestors()
            .nth(rel.1.len())
            .map(|p| p.to_string_lossy().into_owned())
            .filter(|root| !root.is_empty())
            .unwrap_or_else(|| format!("{}/{}", chunk_dir.to_string_lossy(), rel.0));
        let message = diag.message.replace(root.as_str(), "<chunk>");
        rows.get_mut(&rel.0).expect("checked above").push(Row::new(
            diag.severity.clone(),
            diag.code.clone(),
            rel.1.join("/"),
            diag.line,
            diag.column,
            diag.length,
            message,
        ));
    }
    if outside > 0 {
        warn(&format!(
            "{outside} diagnostics outside the chunk package roots were dropped"
        ));
    }
    for batch in rows.values_mut() {
        crate::rows::sort_rows(batch);
    }
    rows
}

/// Splits a diagnostic path into `(case index, relative file components)`
/// under the chunk dir: `<chunk>/<index>/<file…>`. The analyzer may report
/// files under a different symlink spelling than the directory passed on its
/// command line (macOS resolves `/tmp` to `/private/tmp`), so when the
/// direct prefix strip fails the fully resolved forms of both sides are
/// compared before the row is given up on.
fn split_under_chunk(path: &Path, chunk_dir: &Path) -> Option<(usize, Vec<String>)> {
    let rest = match path.strip_prefix(chunk_dir) {
        Ok(rest) => rest.to_owned(),
        Err(_) => {
            let resolved = std::fs::canonicalize(path).ok()?;
            let resolved_chunk = std::fs::canonicalize(chunk_dir).ok()?;
            resolved.strip_prefix(resolved_chunk).ok()?.to_owned()
        }
    };
    let mut comps = rest.components();
    let index = comps.next()?.as_os_str().to_string_lossy().into_owned();
    let index: usize = index.parse().ok()?;
    let file: Vec<String> = comps
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    if file.is_empty() {
        return None;
    }
    Some((index, file))
}

fn warn(message: &str) {
    let _ = writeln!(std::io::stderr().lock(), "warn: {message}");
}

/// The oracle command: `<sdk>/bin/dart analyze --format=json <chunk>`.
pub fn oracle_command(sdk_bin: &Path, chunk_dir: &Path) -> Command {
    let mut command = Command::new(sdk_bin);
    command.arg("analyze").arg("--format=json").arg(chunk_dir);
    command
}

/// The dartq command: `<bin> analyze --engine --cold --format=json [--sdk-path S] <chunk>`.
pub fn dartq_command(bin: &Path, sdk: Option<&Path>, chunk_dir: &Path) -> Command {
    let mut command = Command::new(bin);
    command
        .arg("analyze")
        .arg("--engine")
        .arg("--cold")
        .arg("--format=json");
    if let Some(sdk) = sdk {
        command.arg(format!("--sdk-path={}", sdk.display()));
    }
    command.arg(chunk_dir);
    command
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::case::Case;
    use std::path::PathBuf;

    fn diag(file: &str) -> analyze_json::Diag {
        analyze_json::Diag {
            severity: "ERROR".into(),
            code: "undefined_identifier".into(),
            file: file.into(),
            line: 1,
            column: 5,
            length: 1,
            message: "m".into(),
        }
    }

    fn make_runner<'a>(cases: &'a [LoadedCase], label: &'a str) -> Runner<'a> {
        Runner {
            cases,
            chunk_size: 2,
            work_root: std::env::temp_dir()
                .join(format!("conf-runner-{label}-{}", std::process::id())),
            label,
            mocks_root: std::env::temp_dir().join("conf-runner-mocks-missing"),
            timeout: Duration::from_secs(60),
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
    fn attribution_maps_paths_to_roots_and_relativizes() {
        let chunk = Path::new("/tmp/chunk");
        let diags = vec![
            diag("/tmp/chunk/1/lib/test.dart"),
            diag("/tmp/chunk/0/lib/a.dart"),
            diag("/tmp/elsewhere/lib/test.dart"),
        ];
        let rows = attribute(&diags, chunk, &[0, 1]);
        assert_eq!(rows[&0].len(), 1);
        assert_eq!(rows[&0][0].file, "lib/a.dart");
        assert_eq!(rows[&1].len(), 1);
        assert_eq!(rows[&1][0].file, "lib/test.dart");
    }

    #[test]
    fn attribution_normalizes_case_root_in_messages() {
        let chunk = Path::new("/tmp/chunk");
        let mut workspace = diag("/tmp/chunk/7/pubspec.yaml");
        workspace.message = "Workspace values must be a subdirectory of '/tmp/chunk/7'.".into();
        let mut include = diag("/tmp/chunk/7/analysis_options.yaml");
        include.message =
            "The URI 'x.yaml' included in '/tmp/chunk/7/analysis_options.yaml' can't be found."
                .into();
        let rows = attribute(&[workspace, include], chunk, &[7]);
        // Rows come back sorted by file: analysis_options.yaml before pubspec.yaml.
        assert_eq!(
            rows[&7][0].message,
            "The URI 'x.yaml' included in '<chunk>/analysis_options.yaml' can't be found."
        );
        assert_eq!(
            rows[&7][1].message,
            "Workspace values must be a subdirectory of '<chunk>'."
        );
    }

    #[test]
    fn attribution_resolves_a_symlinked_chunk_spelling() {
        // The chunk is passed to the analyzer (and to `attribute`) through a
        // symlink, while the analyzer reports the fully resolved spelling of
        // the same directory; the rows must still attribute.
        let base = std::env::temp_dir().join(format!("conf-attr-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let real = base.join("real");
        std::fs::create_dir_all(real.join("7/lib")).expect("creates package root");
        std::fs::write(real.join("7/lib/test.dart"), "var x = 1;\n").expect("writes file");
        std::fs::create_dir_all(&base).expect("creates base");
        std::os::unix::fs::symlink(&real, base.join("link")).expect("links chunk dir");
        let mut reported = diag(real.join("7/lib/test.dart").to_str().unwrap());
        reported.message = format!("Something is wrong with '{}'.", real.join("7").display());
        let rows = attribute(&[reported], &base.join("link"), &[7]);
        assert_eq!(rows[&7].len(), 1);
        assert_eq!(rows[&7][0].file, "lib/test.dart");
        assert_eq!(rows[&7][0].message, "Something is wrong with '<chunk>'.");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn bisect_attributes_every_failing_case() {
        let cases: Vec<LoadedCase> = (0..4).map(|i| loaded(&format!("c{i}"))).collect();
        let runner = make_runner(&cases, "bisect");
        // Chunk dirs materialize fine (mocks root unused: no packages); the
        // "analyzer" crashes whenever case 3 is in the chunk.
        let crasher = 3usize;
        let build = move |dir: &Path| {
            let marker = dir.join(format!("{crasher}"));
            let mut command = Command::new("/bin/sh");
            command.arg("-c").arg(format!(
                "if [ -d \"{}/\" ]; then echo boom >&2; exit 70; else echo '{{\"version\":1,\"diagnostics\":[]}}'; fi",
                marker.display()
            ));
            command
        };
        let analysis = runner.analyze(&[0, 1, 2, 3], &build);
        assert_eq!(analysis.crashed, BTreeSet::from([3]));
        assert_eq!(analysis.rows.len(), 3);
        let _ = std::fs::remove_dir_all(&runner.work_root);
    }

    #[test]
    fn commands_carry_the_documented_flags() {
        let sdk = PathBuf::from("/sdk");
        let debug = format!(
            "{:?}",
            oracle_command(&sdk.join("bin/dart"), Path::new("/chunk"))
        );
        assert_eq!(
            debug,
            r#""/sdk/bin/dart" "analyze" "--format=json" "/chunk""#
        );
        let debug = format!(
            "{:?}",
            dartq_command(Path::new("/bin/dartq"), Some(&sdk), Path::new("/chunk"))
        );
        assert_eq!(
            debug,
            r#""/bin/dartq" "analyze" "--engine" "--cold" "--format=json" "--sdk-path=/sdk" "/chunk""#
        );
        let debug = format!(
            "{:?}",
            dartq_command(Path::new("/bin/dartq"), None, Path::new("/chunk"))
        );
        assert!(!debug.contains("sdk-path"), "{debug}");
    }
}
