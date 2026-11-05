//! Process-table observation of the nested `flutter assemble` runs: the
//! delivery channel of the assemble face.
//!
//! The platform build systems behind `flutter build|run` — Gradle
//! (`BaseFlutterTaskHelper.kt`), Xcode (`xcode_backend.dart`), CMake
//! (`tool_backend.dart`) — invoke `$FLUTTER_ROOT/bin/flutter assemble …`
//! through the SDK's absolute path, bypassing PATH, so no shim ever sees
//! those invocations as commands. The outer shim closes the gap from
//! above: while the wrapped flutter child runs, a background thread
//! polls the process table (macOS: `ps -axo command=`; Linux:
//! `/proc/<pid>/cmdline`, the cheaper source) every [`POLL_INTERVAL`],
//! picks every command line that is a flutter process carrying an
//! `assemble` command token, and feeds each NEWLY SEEN argv
//! (deduplicated across polls by its blake3) through
//! [`assemble::capture_templates`]. The orchestrator drains the collected
//! templates after the child exits.
//!
//! # Miss-window honesty
//!
//! A poll can miss an invocation shorter than the interval. A
//! `ps`-sourced command line splits on whitespace, so an argument with
//! embedded spaces arrives as several tokens — [`assemble::capture_templates`]
//! enumerates the join-variants of every such split and the post-run
//! name check accepts exactly the variant whose dir materialized, so
//! the split cannot misplace a capture. The line's WIDTH is not a
//! miss mode either: the macOS read pins unlimited width (`-ww`),
//! because ps clips ax-mode output to the terminal width when writing
//! to a tty (~1.5K of a 2.8K argv observed) — enough to eat the tail of
//! the 3-6K-char assemble argv — while a read piped to this thread
//! shows no clip even at 38K chars, and the flag makes that
//! unconditional; the Linux `/proc` source is NUL-split and never
//! truncated. Every failure mode is safe by
//! construction: a missing or misread template leaves that build dir
//! [`crate::types::NameRule::NotReplayable`] (counted in the shim
//! report's skipped dirs) — never a wrong replay, because the post-run
//! merge accepts a template only when its computed name matches a dir
//! that actually materialized. Capture reliably fires exactly when it
//! matters: a cold build, whose assemble steps run for seconds (kernel
//! compilation alone is tens of them), and the first build in a tree is
//! the one the cache needs.
//!
//! # Cost
//!
//! One poll is one `ps` spawn (~5 ms of CPU on a quiet macOS host) or a
//! `/proc` walk with no process spawn at all; a 60 s build pays for
//! ~300 polls, well under one CPU-second, against a child that is
//! itself using cores continuously. A long interactive `flutter run`
//! keeps the same polling rate for the session's lifetime — roughly 2%
//! of one core — which stays well inside the shim's budget. The
//! observer thread joins before the shim returns, so no thread ever
//! outlives a build.
//!
//! Platforms without a process-table source disable the observer: no
//! thread is spawned, and every assemble-face dir degrades to
//! not-replayable.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
#[cfg(any(target_os = "macos", target_os = "linux"))]
use std::thread::JoinHandle;

use rustc_hash::FxHashSet;

use crate::assemble::{self, AssembleCapture};

/// Interval between process-table polls. 200 ms balances the miss
/// window (an assemble step shorter than this can slip between two
/// polls; see the module docs for why that is safe) against the polling
/// cost (a 60 s build pays for ~300 `ps` spawns, under one CPU-second).
#[cfg(any(target_os = "macos", target_os = "linux"))]
const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(200);

/// A running process-table watch: the background thread it owns polls
/// for nested `flutter assemble` invocations until [`CaptureObserver::finish`]
/// joins it.
pub struct CaptureObserver {
    /// Dropping this wakes the thread between polls.
    stop: Option<Sender<()>>,
    /// The polling thread; absent where no process-table source exists.
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    handle: Option<JoinHandle<()>>,
    captures: Receiver<AssembleCapture>,
}

impl CaptureObserver {
    /// Starts watching for assemble invocations of the build running in
    /// `cwd`, hashing build-dir names with `engine_revision` (a
    /// `--local-engine*` invocation hashes a null revision on its own;
    /// see [`assemble::capture_templates`]). On a platform with no
    /// process-table source this starts nothing and
    /// [`CaptureObserver::finish`] returns none.
    #[must_use]
    pub fn start(cwd: PathBuf, engine_revision: String) -> Self {
        let (stop, stop_rx) = mpsc::channel::<()>();
        let (sent, captures) = mpsc::channel::<AssembleCapture>();
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        let handle = {
            use std::sync::mpsc::RecvTimeoutError;
            std::thread::Builder::new()
                .name("dartq-bcache-observer".to_owned())
                .spawn(move || {
                    let mut seen: FxHashSet<[u8; 32]> = FxHashSet::default();
                    loop {
                        for capture in sweep(&cwd, &engine_revision, &mut seen) {
                            if sent.send(capture).is_err() {
                                return; // the collector went away
                            }
                        }
                        match stop_rx.recv_timeout(POLL_INTERVAL) {
                            Ok(()) | Err(RecvTimeoutError::Disconnected) => break,
                            Err(RecvTimeoutError::Timeout) => {}
                        }
                    }
                    // A last sweep catches an assemble child still alive
                    // just after the outer build exited.
                    for capture in sweep(&cwd, &engine_revision, &mut seen) {
                        let _ = sent.send(capture);
                    }
                })
                .ok()
        };
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        drop(stop_rx); // nobody listens on an unsupported platform
        Self {
            stop: Some(stop),
            #[cfg(any(target_os = "macos", target_os = "linux"))]
            handle,
            captures,
        }
    }

    /// Joins the thread and returns every capture it collected, sorted
    /// by name. The watch ends with this call.
    #[must_use]
    pub fn finish(mut self) -> Vec<AssembleCapture> {
        drop(self.stop.take());
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
        let mut captures: Vec<AssembleCapture> = self.captures.iter().collect();
        captures.sort_unstable_by(|a, b| a.name.cmp(&b.name));
        captures
    }
}

/// One pass over the live process table.
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn sweep(
    cwd: &Path,
    engine_revision: &str,
    seen: &mut FxHashSet<[u8; 32]>,
) -> Vec<AssembleCapture> {
    collect(process_argv(), cwd, engine_revision, seen)
}

/// One poll's classification and parsing over an injected argv list (the
/// test seam of [`sweep`]): every newly seen flutter-assemble argv
/// becomes its templates — one, or several join-variants where the
/// whitespace split of a spaced define value is ambiguous (see
/// [`assemble::capture_templates`]). An argv that parses to nothing (an
/// unmodeled shape, a misread token) still counts as seen — retrying it
/// every poll would only burn the same parse, and skipping it can cost
/// that dir its template, never misplace one.
#[cfg_attr(
    not(any(target_os = "macos", target_os = "linux")),
    allow(dead_code)
)]
fn collect(
    argvs: Vec<Vec<String>>,
    cwd: &Path,
    engine_revision: &str,
    seen: &mut FxHashSet<[u8; 32]>,
) -> Vec<AssembleCapture> {
    let mut found = Vec::new();
    for argv in argvs {
        if !is_flutter_process(&argv) || !assemble::is_assemble_argv(&argv) {
            continue;
        }
        let mut hasher = blake3::Hasher::new();
        for token in &argv {
            hasher.update(token.as_bytes());
            hasher.update(&[0]);
        }
        if !seen.insert(*hasher.finalize().as_bytes()) {
            continue; // already captured (or skipped) on an earlier poll
        }
        if let Ok(captures) = assemble::capture_templates(&argv, cwd, engine_revision) {
            found.extend(captures);
        }
    }
    found
}

/// Whether `argv` belongs to a flutter tool process: a `flutter` binary
/// token (the wrapper script gradle/Xcode/CMake invoke, however spelled)
/// or a flutter_tools snapshot/source token (the dart process the
/// wrapper execs).
fn is_flutter_process(argv: &[String]) -> bool {
    argv.iter().any(|token| {
        let token = token.as_str();
        token == "flutter"
            || token.ends_with("/flutter")
            || token.contains("flutter_tools.snapshot")
            || token.contains("flutter_tools.dart")
    })
}

/// Every process's argv, as the platform's process table spells it.
/// macOS: `ps -axo command= -ww` — one whitespace-joined line per
/// process at unlimited width, so an argument with embedded spaces is
/// unrecoverable but a long argv never loses its tail. Linux:
/// `/proc/<pid>/cmdline` — NUL-separated, lossless, never truncated.
/// A failed read yields an empty table for that poll.
#[cfg(target_os = "macos")]
fn process_argv() -> Vec<Vec<String>> {
    // `-ww` pins the read to unlimited width: ps clips ax-mode output
    // to the terminal width when writing to a tty (~1.5K of a 2.8K
    // argv observed), and the 3-6K-char gradle assemble argv must never
    // lose its `--output` tail to a width limit, whatever the context.
    let Ok(output) = std::process::Command::new("ps")
        .args(["-axo", "command=", "-ww"])
        .output()
    else {
        return Vec::new();
    };
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(|line| line.split_whitespace().map(str::to_owned).collect())
        .filter(|argv: &Vec<String>| !argv.is_empty())
        .collect()
}

#[cfg(target_os = "linux")]
fn process_argv() -> Vec<Vec<String>> {
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if !name.bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }
        let Ok(bytes) = std::fs::read(entry.path().join("cmdline")) else {
            continue;
        };
        let argv: Vec<String> = bytes
            .split(|&byte| byte == 0)
            .filter(|token| !token.is_empty())
            .map(|token| String::from_utf8_lossy(token).into_owned())
            .collect();
        if !argv.is_empty() {
            out.push(argv);
        }
    }
    out
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn process_argv() -> Vec<Vec<String>> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::args_model::flutter_build_dir_name;

    fn argv(tokens: &[&str]) -> Vec<String> {
        tokens.iter().map(|token| (*token).to_owned()).collect()
    }

    fn poll(
        argvs: &[Vec<String>],
        cwd: &Path,
        seen: &mut FxHashSet<[u8; 32]>,
    ) -> Vec<AssembleCapture> {
        collect(argvs.to_vec(), cwd, "enginerev0000000000000000", seen)
    }

    /// A gradle-shaped assemble argv, as the observer receives it.
    fn gradle_assemble(flutter: &str, output: &str) -> Vec<String> {
        argv(&[
            flutter,
            "--quiet",
            "assemble",
            "--no-version-check",
            "--depfile",
            "flutter_build.d",
            "--output",
            output,
            "-dTargetFile=lib/main.dart",
            "-dTargetPlatform=android",
            "-dBuildMode=release",
            "--DartDefines=QVBJPXByb2Q=",
            "-dAndroidArchs=android-arm64",
            "-dMinSdkVersion=21",
            "android_aot_bundle_release_android-arm64",
        ])
    }

    #[test]
    fn ps_line_argv_yields_the_template_of_its_clean_tokens() {
        // Exactly the line a macOS poll sees: the shebang interpreter,
        // the absolute flutter binary, then the assemble argv — one
        // whitespace-joined `ps -axo command=` line.
        let tmp = tempfile::tempdir().expect("tempdir");
        // The flutter path token is just a string to the parser; the
        // file itself never needs to exist.
        let flutter = tmp.path().join("sdk").join("bin").join("flutter");
        let output = tmp.path().join("build/app/intermediates/flutter/release");
        let line = format!(
            "/bin/sh {} --quiet assemble --no-version-check --depfile flutter_build.d --output {} -dTargetFile=lib/main.dart -dTargetPlatform=android -dBuildMode=release --DartDefines=QVBJPXByb2Q= -dAndroidArchs=android-arm64 -dMinSdkVersion=21 android_aot_bundle_release_android-arm64",
            flutter.display(),
            output.display(),
        );
        let ps_argv: Vec<String> = line.split_whitespace().map(str::to_owned).collect();
        assert!(is_flutter_process(&ps_argv));
        assert!(assemble::is_assemble_argv(&ps_argv));

        let mut seen = FxHashSet::default();
        let found = poll(&[ps_argv], tmp.path(), &mut seen);
        // The as-tokenized parse plus the target-fold join-variant (the
        // trailing target after `-dMinSdkVersion` is an orphan run).
        assert_eq!(found.len(), 2);
        let capture = found[0].clone();
        // The interpreter tokens are positional junk the parser skips:
        // the base template is exactly what the clean argv yields.
        let clean = gradle_assemble(
            &flutter.to_string_lossy(),
            &output.to_string_lossy(),
        );
        let mut seen = FxHashSet::default();
        assert_eq!(poll(&[clean], tmp.path(), &mut seen)[0], capture);
        assert_eq!(
            capture.name,
            flutter_build_dir_name(
                "enginerev0000000000000000",
                &capture.defines,
                Path::new(&capture.output_dir),
            )
        );
        assert_eq!(
            capture.defines.get("API").map(String::as_str),
            Some("prod"),
            "the base64 DartDefines decoded"
        );
    }

    #[test]
    fn collect_dedupes_repeated_argv_across_polls() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let one = gradle_assemble("/opt/flutter/bin/flutter", "/out/one");
        let two = gradle_assemble("/opt/flutter/bin/flutter", "/out/two");
        let mut seen = FxHashSet::default();

        let first = poll(&[one.clone(), two.clone()], tmp.path(), &mut seen);
        // Two argvs, each the base parse plus its target-fold variant.
        assert_eq!(first.len(), 4);
        // The next poll sees both processes still running: nothing new.
        assert!(poll(&[one, two], tmp.path(), &mut seen).is_empty());
    }

    #[test]
    fn collect_ignores_non_flutter_and_non_assemble_processes() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let processes = [
            // flutter_tester: a flutter-named artifact that is not the
            // tool, even with assemble-looking tokens behind it.
            argv(&[
                "/opt/flutter/bin/cache/artifacts/engine/darwin-x64/flutter_tester",
                "--quiet",
                "assemble",
                "-o",
                "/out",
            ]),
            // A dart process with a bare `assemble` word.
            argv(&["dart", "tool", "assemble"]),
            // The outer wrapped build itself (no assemble token).
            argv(&["/bin/sh", "/opt/flutter/bin/flutter", "build", "apk"]),
            // `assemble` as an option value, consumed by its flag.
            argv(&["/opt/flutter/bin/flutter", "--frob", "assemble"]),
        ];
        let mut seen = FxHashSet::default();
        assert!(poll(&processes, tmp.path(), &mut seen).is_empty());
        // And none of them burned a dedupe slot that would hide a real
        // capture: the same table plus a real assemble still yields it
        // (the base parse and its target-fold variant).
        let mut table = processes.to_vec();
        table.push(gradle_assemble("/opt/flutter/bin/flutter", "/out/real"));
        assert_eq!(poll(&table, tmp.path(), &mut seen).len(), 2);
    }

    #[test]
    fn collect_skips_an_unparseable_assemble_argv_without_retry() {
        let tmp = tempfile::tempdir().expect("tempdir");
        // `--dart-define-from-file` folds file contents this crate does
        // not model: no capture, and the argv counts as seen so later
        // polls do not re-parse it.
        let unmodeled = argv(&[
            "/opt/flutter/bin/flutter",
            "assemble",
            "-o",
            "/out",
            "--dart-define-from-file=env.json",
        ]);
        let mut seen = FxHashSet::default();
        assert!(poll(&[unmodeled.clone()], tmp.path(), &mut seen).is_empty());
        assert!(poll(&[unmodeled], tmp.path(), &mut seen).is_empty());
    }

    /// The real platform source lists this very test process.
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn process_argv_lists_running_processes() {
        let argvs = process_argv();
        assert!(
            argvs.iter().any(|argv| argv
                .iter()
                .any(|token| token.contains("dartq"))),
            "the test binary's own process must be listed"
        );
    }

    /// The full observer lifecycle on a live process table: starting and
    /// finishing must join the thread (no leak) and return name-sorted,
    /// duplicate-free captures — none, since no assemble runs.
    #[test]
    fn observer_start_finish_joins_cleanly() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let observer = CaptureObserver::start(
            tmp.path().to_path_buf(),
            "enginerev0000000000000000".to_owned(),
        );
        let captures = observer.finish();
        assert!(
            captures.windows(2).all(|pair| pair[0].name <= pair[1].name),
            "captures come out name-sorted"
        );
    }

    /// A real decoy child whose assemble-like argv sits in the gradle
    /// range (~4K chars), with the fake defines and `--output` in the
    /// TAIL: only an untruncated process-table read can parse them. The
    /// tail is exactly what a width clip eats first — ps ax mode clips
    /// to the terminal width on a tty (~1.5K of a 2.8K argv observed).
    /// This test reads through a pipe, where no clip occurs even at 38K
    /// chars with or without `-ww`, so it pins tail preservation
    /// end-to-end through the real platform source rather than the flag
    /// itself.
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn collect_keeps_the_tail_of_a_long_assemble_argv() {
        use std::time::Duration;

        let tmp = tempfile::tempdir().expect("tempdir");
        // 120 defines x ~31 chars plus the tokens: ~4.3K of argv.
        const DEFINES: usize = 120;
        let value = "v".repeat(20);
        let mut tokens: Vec<String> = [
            "/opt/flutter/bin/flutter",
            "--quiet",
            "assemble",
            "--no-version-check",
        ]
        .iter()
        .map(|token| (*token).to_owned())
        .collect();
        for index in 0..DEFINES {
            tokens.push(format!("-dDefine=K{index}={value}"));
        }
        let output = tmp.path().join("build/app/intermediates/flutter/release");
        let output_text = output.to_string_lossy().into_owned();
        tokens.push("--output".to_owned());
        tokens.push(output_text.clone());
        tokens.push("android_aot_bundle_release_android-arm64".to_owned());
        assert!(
            tokens.iter().map(String::len).sum::<usize>() > 3500,
            "the decoy argv must sit in the gradle range, past the tty clip width"
        );

        let mut command = std::process::Command::new("/bin/sh");
        command.arg("-c").arg("sleep 1.5; true");
        for token in &tokens {
            command.arg(token);
        }
        let mut child = command.spawn().expect("spawn the long-argv lookalike");

        // Poll the REAL process table (the platform source under test)
        // until the decoy's capture appears.
        let mut seen = FxHashSet::default();
        let mut found = None;
        for _ in 0..12 {
            for capture in collect(
                process_argv(),
                tmp.path(),
                "enginerev0000000000000000",
                &mut seen,
            ) {
                if capture.output_dir == output_text {
                    found = Some(capture);
                }
            }
            if found.is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(250));
        }
        let status = child.wait().expect("reap the lookalike");
        assert!(status.success());

        let capture = found.expect("the long-argv decoy must be captured");
        // The tail survived: the LAST define won the map slot and the
        // trailing --output was hashed verbatim.
        assert_eq!(
            capture.defines.get("Define").map(String::as_str),
            Some(format!("K{}={value}", DEFINES - 1).as_str())
        );
        assert_eq!(capture.output_dir, output_text);
    }
}
