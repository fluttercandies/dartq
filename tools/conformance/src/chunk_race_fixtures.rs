//! Deterministic fixtures for the chunk-directory races behind #3746/#3756.
//!
//! These races live in the harness, not in Dart semantics, so no
//! `conformance/manual/**` case can express them: a `.case` file cannot make
//! two harness processes share a chunk root, and it cannot make
//! `remove_dir_all` fail. Each fixture below drives the real runner and
//! materializer over a private temp work root and reproduces the interleaving
//! by ordering the calls explicitly — no threads, no sleeps, no oracle — so
//! every run is byte-identical.
//!
//! | fixture | race it reproduces |
//! |---|---|
//! | `interleaved_runs_never_share_a_chunk_root` | Two harness processes on one `work_root` materialized into the same `chunks/<label>` directory. The later writer's `remove_dir_all` deleted the earlier run's package roots while its analyzer walked them; the analyzer died on discovery (ENOENT), the chunk "crashed", bisection re-ran the halves into that same directory, and the survivors' rows were dropped as "outside the chunk package roots" — reported as missing rows with no extras, i.e. a phantom regression. |
//! | `adjacent_cases_keep_same_named_libraries_and_packages_apart` | One chunk holds every case in its own package root but a single analyzer process sees them all. Adjacent cases that both define `package:test/lib/test.dart` and an inline `package:aaa` must resolve against their own root; a shared root or a misattributed row attaches one case's diagnostics to its neighbour. |
//! | `unclearable_chunk_root_aborts_the_run` | `remove_dir_all` errors were swallowed (`let _ =`), so a root that could not be created was still written into and analyzed; and a chunk-infrastructure failure was reported through the per-case channel. It must abort the run with no case verdicts instead of fabricating a report. |

use crate::case::Case;
use crate::materialize::{self, LoadedCase};
use crate::runner::{Runner, fresh_chunk_dir};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

/// Emits one diagnostic per package root, its message carrying the root's own
/// `package:aaa` content — exactly what the two same-named fixtures need to
/// prove they resolved separately. `$1` is the chunk directory; the envelope
/// shape is the one both analyzers write (`location.range`, `problemMessage`).
const SAME_NAME_ANALYZER: &str = r#"for i in 0 1; do
  printf '{"version":1,"diagnostics":[{"code":"same_named_root","severity":"ERROR","location":{"file":"%s/%s/lib/test.dart","range":{"start":{"offset":0,"line":1,"column":1},"end":{"offset":1,"line":1,"column":2}}},"problemMessage":"%s"}]}\n' \
    "$1" "$i" "$(tr -d '\n' < "$1/$i/.aux/aaa/lib/a.dart")"
done"#;

fn same_name_analyzer(chunk_dir: &Path) -> Command {
    let mut command = Command::new("/bin/sh");
    command
        .arg("-c")
        .arg(SAME_NAME_ANALYZER)
        .arg("same-name-analyzer")
        .arg(chunk_dir);
    command
}

fn loaded(id: &str, files: &[(&str, &str)]) -> LoadedCase {
    LoadedCase {
        case: Case {
            id: id.into(),
            source: String::new(),
            language: None,
            packages: vec![],
            aux_packages: vec![],
            options: None,
            ignores: vec![],
            files: files
                .iter()
                .map(|(path, content)| ((*path).to_string(), (*content).to_string()))
                .collect(),
        },
        hash: id.into(),
    }
}

fn temp_dir(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("conf-race-{label}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("creates temp dir");
    dir
}

fn runner<'a>(cases: &'a [LoadedCase], work_root: PathBuf, mocks_root: PathBuf) -> Runner<'a> {
    Runner {
        cases,
        chunk_size: 2,
        work_root,
        label: "run",
        mocks_root,
        timeout: Duration::from_secs(60),
    }
}

/// Two runs on one work root never materialize into the same directory, so a
/// concurrent writer cannot delete the roots an analyzer is reading.
#[test]
fn interleaved_runs_never_share_a_chunk_root() {
    let cases = vec![
        loaded("diagnostics/race/one", &[("lib/test.dart", "one\n")]),
        loaded("diagnostics/race/two", &[("lib/test.dart", "two\n")]),
    ];
    let work = temp_dir("interleave");
    let mocks = temp_dir("interleave-mocks");

    let run_a = fresh_chunk_dir(&work, "run");
    let run_b = fresh_chunk_dir(&work, "run");
    assert_ne!(
        run_a, run_b,
        "two runs must never materialize into the same chunk root"
    );

    // Run A materializes its chunk, then the concurrent run (a census beside
    // a filtered probe, the shape that produced the phantom regression)
    // materializes its own. Under the shared `chunks/run` root this second
    // call deleted A's package roots.
    materialize::materialize(&cases, &[0], &run_a, &mocks).expect("materializes run A");
    materialize::materialize(&cases, &[1], &run_b, &mocks).expect("materializes run B");

    assert_eq!(
        std::fs::read_to_string(run_a.join("0/lib/test.dart")).expect("run A root survived"),
        "one\n",
        "the concurrent run must not delete run A's roots"
    );
    assert_eq!(
        std::fs::read_to_string(run_b.join("1/lib/test.dart")).expect("run B root"),
        "two\n"
    );
    assert!(
        !run_b.join("0").exists(),
        "a run must materialize only its own indices"
    );

    let _ = std::fs::remove_dir_all(&work);
    let _ = std::fs::remove_dir_all(&mocks);
}

/// Same-named libraries and same-named inline packages in adjacent cases of
/// one chunk stay attached to their own case.
#[test]
fn adjacent_cases_keep_same_named_libraries_and_packages_apart() {
    let cases = vec![
        loaded(
            "diagnostics/race/same/one",
            &[
                ("lib/test.dart", "one\n"),
                ("package:aaa/lib/a.dart", "const a = 1;\n"),
            ],
        ),
        loaded(
            "diagnostics/race/same/two",
            &[
                ("lib/test.dart", "two\n"),
                ("package:aaa/lib/a.dart", "const a = 2;\n"),
            ],
        ),
    ];
    let work = temp_dir("same-name");
    let mocks = temp_dir("same-name-mocks");
    let runner = runner(&cases, work.clone(), mocks.clone());

    let analysis = runner.analyze(&[0, 1], &same_name_analyzer);
    assert!(analysis.fatal.is_none(), "{:?}", analysis.fatal);
    assert!(analysis.crashed.is_empty(), "{:?}", analysis.crashed);
    assert_eq!(analysis.rows[&0].len(), 1);
    assert_eq!(analysis.rows[&1].len(), 1);
    // Both roots report `lib/test.dart`; attribution must key on the root,
    // not on the file name.
    assert_eq!(analysis.rows[&0][0].file, "lib/test.dart");
    assert_eq!(analysis.rows[&1][0].file, "lib/test.dart");
    // The message carries the case's own `package:aaa` content.
    assert_eq!(analysis.rows[&0][0].message, "const a = 1;");
    assert_eq!(analysis.rows[&1][0].message, "const a = 2;");

    let _ = std::fs::remove_dir_all(&work);
    let _ = std::fs::remove_dir_all(&mocks);
}

/// A chunk root that cannot be created is a run-level failure: no case
/// verdict, no bisection, no report.
#[test]
fn unclearable_chunk_root_aborts_the_run() {
    let cases = vec![loaded(
        "diagnostics/race/blocked",
        &[("lib/test.dart", "var x = 1;\n")],
    )];
    let parent = temp_dir("blocked");
    // The work root is a regular file, so every per-run chunk dir under it
    // fails to be created.
    let work = parent.join("work-root");
    std::fs::write(&work, "not a directory").expect("writes blocker");
    let mocks = temp_dir("blocked-mocks");
    let runner = runner(&cases, work.clone(), mocks.clone());
    let build = |_: &Path| Command::new("/nonexistent/analyzer-must-not-run");

    let analysis = runner.analyze(&[0], &build);
    let detail = analysis
        .fatal
        .expect("a chunk root that cannot be created is fatal");
    assert!(detail.contains("chunk dir"), "{detail}");
    assert!(
        analysis.crashed.is_empty(),
        "an infrastructure failure is not a case crash: {:?}",
        analysis.crashed
    );
    assert!(analysis.rows.is_empty());

    let _ = std::fs::remove_file(&work);
    let _ = std::fs::remove_dir_all(&parent);
    let _ = std::fs::remove_dir_all(&mocks);
}
