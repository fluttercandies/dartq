//! Process-table observation of the nested `flutter assemble` runs: the
//! delivery channel of the assemble face.
//!
//! The platform build systems behind `flutter build|run` — Gradle
//! (`BaseFlutterTaskHelper.kt`), Xcode (`xcode_backend.dart`), CMake
//! (`tool_backend.dart`) — invoke `$FLUTTER_ROOT/bin/flutter assemble …`
//! through the SDK's absolute path, bypassing PATH, so no shim ever sees
//! those invocations as commands. The outer shim closes the gap from
//! above: while the wrapped flutter child runs, a background thread
//! reads the process table IN PROCESS every [`POLL_INTERVAL`] — no `ps`
//! process is spawned — picks every argv that is a flutter process
//! carrying an `assemble` command token, and feeds each NEWLY SEEN argv
//! (deduplicated across polls by its blake3) through
//! [`assemble::capture_templates`]. The orchestrator drains the
//! collected templates after the child exits.
//!
//! The read is two-stage, so a poll pays the full-argv price only for
//! the handful of processes that can carry the tool: a cheap name pass
//! over every process (macOS: `proc_listallpids` + `proc_name`; Linux:
//! `/proc/<pid>/comm`) keeps only processes running under a name a
//! flutter tool invocation uses — the interpreter the
//! `#!/usr/bin/env bash` wrapper script runs as (`sh`/`bash`/`zsh`/
//! `dash`) and the `dart` VM the wrapper execs — and only the matches
//! get their argv read (macOS: `KERN_PROCARGS2`; Linux:
//! `/proc/<pid>/cmdline`).
//!
//! The kernel spells argv as true NUL-separated elements, so a define
//! VALUE with embedded spaces (the default multi-arch apk build's
//! `-dAndroidArchs=android-arm android-arm64 android-x64`) arrives as
//! ONE element — the whitespace ambiguity of a `ps`-sourced line does
//! not exist at this source, so kernel argv is captured without the
//! join-variant enumeration (whitespace-split fixtures replayed
//! through the [`collect`] test seam keep it, and the post-run name
//! check accepts exactly the variant whose dir materialized).
//!
//! # Miss-window honesty
//!
//! A poll can miss an invocation shorter than the interval, and the
//! name pass skips a flutter token carried under some other process
//! name — every real producer (the SDK's bash wrapper and the dart VM
//! it execs) matches by construction. Every failure mode is safe by
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
//! One poll is one in-process table pass: measured at ~0.9 ms on a
//! 650-process host, where the `ps` spawn it replaced cost 29.35 ms —
//! at one poll per 200 ms that was ~15% of a core for a child's whole
//! lifetime (hours, for an interactive `flutter run`), now under
//! 0.5%. The observer thread ends with a join capped at
//! [`JOIN_TIMEOUT`]: a process-table read stuck in the kernel can cost
//! that build's uncollected captures (logged, then dropped), never the
//! shim's return.
//!
//! Platforms without a process-table source disable the observer: no
//! thread is spawned, and every assemble-face dir degrades to
//! not-replayable.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
#[cfg(any(target_os = "macos", target_os = "linux"))]
use std::sync::mpsc::RecvTimeoutError;
#[cfg(any(target_os = "macos", target_os = "linux"))]
use std::thread::JoinHandle;
#[cfg(any(target_os = "macos", target_os = "linux"))]
use std::time::Duration;

use rustc_hash::FxHashSet;

use crate::assemble::{self, AssembleCapture};

/// Interval between process-table polls. 200 ms balances the miss
/// window (an assemble step shorter than this can slip between two
/// polls; see the module docs for why that is safe) against the polling
/// cost (an in-process table pass is a fraction of a millisecond; a
/// hours-long `flutter run` session stays well under 1% of one core).
#[cfg(any(target_os = "macos", target_os = "linux"))]
const POLL_INTERVAL: Duration = Duration::from_millis(200);

/// Hard cap on how long [`CaptureObserver::finish`] waits for the
/// polling thread's final sweep: a process-table read stuck in the
/// kernel can cost that build's uncollected captures (logged, then
/// dropped), never the shim's return. The cap dwarfs a healthy final
/// pass (a fraction of a millisecond) plus one poll interval of wake
/// latency.
#[cfg(any(target_os = "macos", target_os = "linux"))]
const JOIN_TIMEOUT: Duration = Duration::from_millis(500);

/// Test seam of the finish-timeout test: when armed, the NEXT started
/// observer stalls its thread at every poll — a stand-in for a
/// process-table read stuck in the kernel. `start` reads-and-clears the
/// flag, so exactly that one observer is affected: a globally armed
/// stall would also park every OTHER test's concurrently running
/// observer (observed: empty capture sets under a parallel module run).
/// Absent from non-test builds.
#[cfg(all(test, any(target_os = "macos", target_os = "linux")))]
static STALL_NEXT: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Whether the next started observer should stall (the test seam of
/// [`STALL_NEXT`]); always `false` in non-test builds.
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn stall_next() -> bool {
    #[cfg(test)]
    {
        STALL_NEXT.swap(false, std::sync::atomic::Ordering::Relaxed)
    }
    #[cfg(not(test))]
    {
        false
    }
}

/// A stall point of the armed observer thread (see [`STALL_NEXT`]); a
/// no-op in non-test builds and for unarmed observers.
#[cfg(any(target_os = "macos", target_os = "linux"))]
#[inline]
fn stall_point(stall: bool) {
    #[cfg(test)]
    if stall {
        std::thread::sleep(std::time::Duration::from_secs(5));
    }
    #[cfg(not(test))]
    let _ = stall;
}

/// A running process-table watch: the background thread it owns polls
/// for nested `flutter assemble` invocations until
/// [`CaptureObserver::finish`] joins it.
pub struct CaptureObserver {
    /// Dropping this wakes the thread between polls.
    stop: Option<Sender<()>>,
    /// The polling thread; absent where no process-table source exists.
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    handle: Option<JoinHandle<()>>,
    /// The thread's end-of-collection signal, on which
    /// [`CaptureObserver::finish`] enforces [`JOIN_TIMEOUT`].
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    done: Option<Receiver<()>>,
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
        let (done_sent, done) = mpsc::channel::<()>();
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        let stall = stall_next();
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        let handle = std::thread::Builder::new()
            .name("dartq-bcache-observer".to_owned())
            .spawn(move || {
                let mut seen: FxHashSet<[u8; 32]> = FxHashSet::default();
                loop {
                    stall_point(stall);
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
                // just after the outer build exited; `done` then tells
                // `finish` the thread has ended its collection.
                for capture in sweep(&cwd, &engine_revision, &mut seen) {
                    let _ = sent.send(capture);
                }
                let _ = done_sent.send(());
            })
            .ok();
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        drop(stop_rx); // nobody listens on an unsupported platform
        Self {
            stop: Some(stop),
            #[cfg(any(target_os = "macos", target_os = "linux"))]
            handle,
            #[cfg(any(target_os = "macos", target_os = "linux"))]
            done: Some(done),
            captures,
        }
    }

    /// Joins the thread and returns every capture it collected, sorted
    /// by name. The join is capped at [`JOIN_TIMEOUT`]: on timeout the
    /// uncollected captures are dropped (with a note) and the shim
    /// returns — a process-table read stuck in the kernel never hangs
    /// the build's epilogue. The watch ends with this call.
    #[must_use]
    pub fn finish(mut self) -> Vec<AssembleCapture> {
        drop(self.stop.take());
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        if let Some(done) = self.done.take() {
            match done.recv_timeout(JOIN_TIMEOUT) {
                Ok(()) | Err(RecvTimeoutError::Disconnected) => {
                    if let Some(handle) = self.handle.take() {
                        let _ = handle.join();
                    }
                }
                Err(RecvTimeoutError::Timeout) => note(&format!(
                    "process-table observer did not finish within {} ms; \
                     dropping uncollected assemble captures",
                    JOIN_TIMEOUT.as_millis()
                )),
            }
        }
        // try_iter, never iter: on the timeout path the abandoned
        // thread still holds its capture sender, and a blocking drain
        // would hang exactly where the cap just refused to.
        let mut captures: Vec<AssembleCapture> = self.captures.try_iter().collect();
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
    collect(process_argv(), cwd, engine_revision, seen, false)
}

/// One poll's classification and parsing over an injected argv list (the
/// test seam of [`sweep`]): every newly seen flutter-assemble argv
/// becomes its templates. `whitespace_split` tells whether the tokens
/// came from a whitespace-split line — the `ps`-fixture shape, whose
/// spaced define values are ambiguous and enumerate join-variants (see
/// [`assemble::capture_templates`]); the kernel argv of [`sweep`] is
/// exact and takes the as-tokenized parse alone. An argv that parses to
/// nothing (an unmodeled shape, a misread token) still counts as seen —
/// retrying it every poll would only burn the same parse, and skipping
/// it can cost that dir its template, never misplace one.
#[cfg_attr(
    not(any(target_os = "macos", target_os = "linux")),
    allow(dead_code)
)]
fn collect(
    argvs: Vec<Vec<String>>,
    cwd: &Path,
    engine_revision: &str,
    seen: &mut FxHashSet<[u8; 32]>,
    whitespace_split: bool,
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
        if let Ok(captures) =
            assemble::capture_templates(&argv, cwd, engine_revision, whitespace_split)
        {
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

/// Kernel process names (`p_comm` / comm) a flutter tool invocation runs
/// under: the `#!/usr/bin/env bash` wrapper script runs as the
/// interpreter's name, the tool the wrapper execs runs as `dart`, and
/// `flutter` covers a hypothetical compiled launcher. The kernel
/// truncates the name to 15 chars — every name here fits.
#[cfg(any(target_os = "macos", target_os = "linux"))]
const WATCHED_COMM: &[&str] = &["sh", "bash", "zsh", "dash", "dart", "flutter"];

/// The argv of every watched-name process, as the kernel spells it:
/// true NUL-separated elements, never truncated, never
/// whitespace-joined. A failed read yields an empty table for that
/// poll.
#[cfg(target_os = "macos")]
fn process_argv() -> Vec<Vec<String>> {
    candidate_pids()
        .into_iter()
        .filter_map(read_argv)
        .collect()
}

/// The watched-name pids: one `proc_listallpids` listing plus one
/// `proc_name` call per process.
#[cfg(target_os = "macos")]
fn candidate_pids() -> Vec<libc::pid_t> {
    all_pids()
        .into_iter()
        .filter(|&pid| is_watched_name(pid))
        .collect()
}

/// Every pid on the system: one `proc_listallpids` size query and one
/// read; a table that grew between the two calls is reread once at the
/// larger size.
#[cfg(target_os = "macos")]
fn all_pids() -> Vec<libc::pid_t> {
    let mut capacity = usize::try_from(unsafe {
        // SAFETY: a null buffer makes the call a pure size query.
        libc::proc_listallpids(std::ptr::null_mut(), 0)
    })
    .unwrap_or(0);
    for _ in 0..2 {
        let mut pids: Vec<libc::pid_t> = vec![0; capacity];
        let byte_len =
            libc::c_int::try_from(pids.len() * std::mem::size_of::<libc::pid_t>()).unwrap_or(0);
        let got = usize::try_from(unsafe {
            // SAFETY: `pids` owns `capacity` pids' worth of writable
            // bytes, and `byte_len` says exactly that many.
            libc::proc_listallpids(pids.as_mut_ptr().cast(), byte_len)
        })
        .unwrap_or(0);
        if got == 0 {
            return Vec::new();
        }
        if got > pids.len() {
            capacity = got;
            continue;
        }
        pids.truncate(got);
        return pids;
    }
    Vec::new()
}

/// Whether the process's kernel name is one of [`WATCHED_COMM`]. One
/// `proc_name` call; the NUL scan reads the name without depending on
/// the call's return-length conventions.
#[cfg(target_os = "macos")]
fn is_watched_name(pid: libc::pid_t) -> bool {
    // MAXCOMLEN is 16; 32 is comfortable headroom.
    let mut name = [0u8; 32];
    let written = usize::try_from(unsafe {
        // SAFETY: `name` is 32 writable bytes and the call never writes
        // more than the passed size.
        libc::proc_name(pid, name.as_mut_ptr().cast(), 32)
    })
    .unwrap_or(0);
    if written == 0 {
        return false;
    }
    let Some(nul) = memchr::memchr(0, &name) else {
        return false;
    };
    std::str::from_utf8(&name[..nul]).is_ok_and(|name| WATCHED_COMM.contains(&name))
}

/// The true argv of one process via `KERN_PROCARGS2`: a size query,
/// then the read into a zeroed buffer — no uninitialized byte is ever
/// read — and one retry at a fresh size when the argv grew between the
/// two calls (`ENOMEM`). `None` for a process whose arguments the
/// kernel will not hand out (pid 0, exiting processes): skipped for
/// this poll, not an error.
#[cfg(target_os = "macos")]
fn read_argv(pid: libc::pid_t) -> Option<Vec<String>> {
    let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid];
    for _ in 0..3 {
        let mut size = 0_usize;
        // SAFETY: a null oldp makes the call a pure size query into
        // `size`; the mib is the literal `kern.procargs2.<pid>` triple.
        if unsafe {
            libc::sysctl(
                mib.as_mut_ptr(),
                3,
                std::ptr::null_mut(),
                &mut size,
                std::ptr::null_mut(),
                0,
            )
        } != 0
            || size == 0
        {
            return None;
        }
        let mut blob = vec![0u8; size];
        let mut filled = size;
        // SAFETY: `blob` owns `size` writable bytes; sysctl writes at
        // most that many and stores the real count in `filled`.
        let rc = unsafe {
            libc::sysctl(
                mib.as_mut_ptr(),
                3,
                blob.as_mut_ptr().cast(),
                &mut filled,
                std::ptr::null_mut(),
                0,
            )
        };
        if rc == 0 {
            blob.truncate(filled);
            return parse_procargs2(&blob);
        }
        // SAFETY: `__error` points at the thread-local errno cell.
        if unsafe { *libc::__error() } != libc::ENOMEM {
            return None;
        }
    }
    None
}

/// Splits a `KERN_PROCARGS2` blob: a native-endian argc, then the
/// process's EXECUTABLE PATH as the first string (verified against this
/// very process's kernel blob — argv[0] comes behind it, with empty
/// padding strings possible in between), then the argc argv strings;
/// the environment strings behind them are never read. Empty strings
/// belong to no argument and are skipped. A blob that promises more
/// strings than it holds — a process with a rewritten argument area —
/// yields `None`: skipped, never misparsed.
#[cfg(target_os = "macos")]
fn parse_procargs2(blob: &[u8]) -> Option<Vec<String>> {
    const MAX_ARGC: usize = 8192;
    let argc = usize::try_from(i32::from_ne_bytes(
        blob.get(..4)?.try_into().expect("four bytes make an i32"),
    ))
    .ok()?;
    if argc == 0 || argc > MAX_ARGC {
        return None;
    }
    let mut rest = blob.get(4..)?;
    // The executable path leads; argv starts behind it.
    let path_end = memchr::memchr(0, rest)?;
    rest = rest.get(path_end + 1..)?;
    let mut argv = Vec::new();
    while argv.len() < argc {
        let end = memchr::memchr(0, rest)?;
        if end > 0 {
            argv.push(String::from_utf8_lossy(&rest[..end]).into_owned());
        }
        rest = rest.get(end + 1..)?;
    }
    Some(argv)
}

/// The argv of every watched-name process, as the kernel spells it:
/// `/proc/<pid>/cmdline`'s NUL-separated elements, never truncated.
/// A failed read yields an empty table for that poll.
#[cfg(target_os = "linux")]
fn process_argv() -> Vec<Vec<String>> {
    candidate_pids()
        .into_iter()
        .filter_map(read_argv)
        .collect()
}

/// The watched-name pids: the numeric `/proc` entries whose comm
/// matches.
#[cfg(target_os = "linux")]
fn candidate_pids() -> Vec<i32> {
    all_pids()
        .into_iter()
        .filter(|&pid| is_watched_name(pid))
        .collect()
}

/// Every pid directory under `/proc`.
#[cfg(target_os = "linux")]
fn all_pids() -> Vec<i32> {
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|entry| entry.file_name().to_str()?.parse().ok())
        .collect()
}

/// Whether `/proc/<pid>/comm` is one of [`WATCHED_COMM`]: one 15-byte
/// read instead of the kilobytes of a full cmdline.
#[cfg(target_os = "linux")]
fn is_watched_name(pid: i32) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/comm"))
        .ok()
        .is_some_and(|comm| WATCHED_COMM.contains(&comm.trim_end()))
}

/// The true argv of one process: `/proc/<pid>/cmdline`'s NUL-separated
/// elements, empty ones dropped (the blob's trailing split). A kernel
/// thread or an exiting process has no cmdline: `None`, skipped.
#[cfg(target_os = "linux")]
fn read_argv(pid: i32) -> Option<Vec<String>> {
    let bytes = std::fs::read(format!("/proc/{pid}/cmdline")).ok()?;
    let mut argv = Vec::new();
    let mut start = 0;
    for end in memchr::memchr_iter(0, &bytes) {
        if end > start {
            argv.push(String::from_utf8_lossy(&bytes[start..end]).into_owned());
        }
        start = end + 1;
    }
    (!argv.is_empty()).then_some(argv)
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn process_argv() -> Vec<Vec<String>> {
    Vec::new()
}

/// One shim note on stderr, in the same `dartq-bcache: …` shape the
/// orchestrator's notes use (that `note` is private to its module).
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn note(message: &str) {
    eprintln!("dartq-bcache: {message}");
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
        // Fixture replay keeps the ps heritage: a spaced define value
        // is split tokens, so the join-variant enumeration runs.
        collect(argvs.to_vec(), cwd, "enginerev0000000000000000", seen, true)
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
        // Exactly the shape a whitespace-split ps line yields (the
        // fixture form this crate's conformance tests replay): the
        // shebang interpreter, the absolute flutter binary, then the
        // assemble argv — one whitespace-joined `ps -axo command=` line.
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

    /// The dartvm shape — the LONG-LIVED process the wrapper execs, the
    /// one the observer actually reads for most of an assemble's
    /// lifetime — must yield exactly the templates the wrapper-shell
    /// shape yields for the same assemble tail.
    #[test]
    fn dartvm_snapshot_argv_yields_the_same_templates_as_the_wrapper_shape() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let tail = [
            "--quiet",
            "assemble",
            "--no-version-check",
            "--depfile",
            "flutter_build.d",
            "--output",
            "/out/release",
            "-dTargetFile=lib/main.dart",
            "-dTargetPlatform=android",
            "-dBuildMode=release",
            "-dAndroidArchs=android-arm64",
            "-dMinSdkVersion=21",
            "android_aot_bundle_release_android-arm64",
        ];
        // (a) The wrapper shell the build systems invoke.
        let mut wrapper = argv(&["/bin/sh", "/opt/flutter/bin/flutter"]);
        wrapper.extend(argv(&tail));
        // (b) The dart process the wrapper execs: the pre-command
        // segment only has to walk to `assemble` without eating it.
        let mut dartvm = argv(&[
            "/opt/flutter/bin/cache/dart-sdk/bin/dart",
            "--disable-analytics",
            "--packages=/proj/.dart_tool/package_config.json",
            "/opt/flutter/bin/cache/flutter_tools.snapshot",
        ]);
        dartvm.extend(argv(&tail));
        assert!(is_flutter_process(&wrapper) && assemble::is_assemble_argv(&wrapper));
        assert!(is_flutter_process(&dartvm) && assemble::is_assemble_argv(&dartvm));

        let mut seen_wrapper = FxHashSet::default();
        let mut seen_dartvm = FxHashSet::default();
        let from_wrapper = poll(&[wrapper], tmp.path(), &mut seen_wrapper);
        let from_dartvm = poll(&[dartvm], tmp.path(), &mut seen_dartvm);
        assert!(!from_wrapper.is_empty());
        assert_eq!(
            from_wrapper, from_dartvm,
            "the dartvm shape and the wrapper shape capture identically"
        );
    }

    /// The kernel hands argv over as true NUL-separated elements, so a
    /// spaced define VALUE arrives whole — the ambiguity class of the
    /// ps whitespace split does not exist at this source, and the base
    /// capture needs no join-variant to recover the value.
    #[test]
    fn true_argv_spaced_define_arrives_whole() {
        let tmp = tempfile::tempdir().expect("tempdir");
        // The default multi-arch apk build's define, as ONE argv
        // element (the fixture comments a kernel-spelled argv in: a
        // fictional SDK root, never a real path).
        let multi_arch = argv(&[
            "/sdks/fictional/bin/flutter",
            "--quiet",
            "assemble",
            "--output",
            "/out/release",
            "-dAndroidArchs=android-arm android-arm64 android-x64",
        ]);
        let mut seen = FxHashSet::default();
        let found = poll(&[multi_arch], tmp.path(), &mut seen);
        // The define is the last token: no orphan run exists, so no
        // join-variants are enumerated at all.
        assert_eq!(found.len(), 1);
        assert_eq!(
            found[0].defines.get("AndroidArchs").map(String::as_str),
            Some("android-arm android-arm64 android-x64"),
            "the spaced value is captured whole, not split"
        );
    }

    /// The raw pid listing sees this very test process.
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn all_pids_lists_this_process() {
        let pid = i32::try_from(std::process::id()).expect("a pid fits an i32");
        assert!(all_pids().contains(&pid));
    }

    /// The full-argv read returns this very test process's argv, with
    /// its true NUL-separated elements intact.
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn read_argv_lists_this_process_argv() {
        let pid = i32::try_from(std::process::id()).expect("a pid fits an i32");
        let argv = read_argv(pid).expect("this process's argv must be readable");
        assert!(
            argv.iter().any(|token| token.contains("dartq")),
            "the test binary's own argv must be listed"
        );
    }

    /// The `KERN_PROCARGS2` blob parser, against the kernel's real
    /// layout (verified on this very process): the executable path
    /// leads, empty padding strings can follow, then exactly argc argv
    /// strings; environment strings behind them are never read, and a
    /// malformed argument area is skipped rather than misparsed.
    #[cfg(target_os = "macos")]
    #[test]
    fn parse_procargs2_splits_true_argv_elements() {
        let mut blob = 3_i32.to_ne_bytes().to_vec();
        blob.extend_from_slice(b"/bin/bash\0"); // the exec path, not argv
        blob.extend_from_slice(b"\0\0"); // padding: belongs to no argument
        blob.extend_from_slice(b"/bin/sh\0");
        blob.extend_from_slice(b"-c\0");
        blob.extend_from_slice(b"sleep 1.5; true\0");
        blob.extend_from_slice(b"PATH=/nonexistent\0"); // env: never read
        assert_eq!(
            parse_procargs2(&blob).expect("a well-formed blob parses"),
            argv(&["/bin/sh", "-c", "sleep 1.5; true"]),
            "the spaced -c value stays ONE argv element"
        );
        // Fewer strings than argc promises, and a zero argc: skipped.
        assert_eq!(parse_procargs2(&4_i32.to_ne_bytes()), None);
        assert_eq!(parse_procargs2(&0_i32.to_ne_bytes()), None);
        assert_eq!(parse_procargs2(&[]), None);
    }

    /// The real platform source lists this very test process's table
    /// neighbors: candidate pids exist and their argvs are whole.
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn process_argv_lists_whole_kernel_spelled_argv() {
        let argvs = process_argv();
        for argv in &argvs {
            assert!(
                argv.iter().all(|token| !token.is_empty()),
                "true argv elements are never empty"
            );
        }
    }

    /// A real decoy child whose assemble-like argv sits in the gradle
    /// range (~4K chars), with the fake defines and `--output` in the
    /// TAIL: only an untruncated process-table read can parse them.
    /// The kernel-spelled argv has no width limit to eat a tail, so
    /// this pins tail preservation end-to-end through the real
    /// platform source.
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
            "the decoy argv must sit in the gradle range"
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
                false,
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

    /// A collection stuck in the kernel (injected through the test
    /// seam) must not hang the shim's epilogue: `finish` returns
    /// within the join cap, dropping the uncollected captures. The
    /// stall is handed to exactly the next started observer, so a
    /// concurrently running one is untouched.
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn finish_returns_within_the_cap_when_collection_stalls() {
        use std::time::{Duration, Instant};

        let tmp = tempfile::tempdir().expect("tempdir");
        STALL_NEXT.store(true, std::sync::atomic::Ordering::Relaxed);
        let observer = CaptureObserver::start(
            tmp.path().to_path_buf(),
            "enginerev0000000000000000".to_owned(),
        );
        // Let the thread reach its stall point (it parks there for 5 s
        // the moment it starts).
        std::thread::sleep(Duration::from_millis(100));

        let started = Instant::now();
        let captures = observer.finish();
        let elapsed = started.elapsed();

        assert!(captures.is_empty());
        assert!(
            elapsed >= Duration::from_millis(400) && elapsed < Duration::from_millis(1500),
            "finish took {elapsed:?}: the join cap must bound a stalled collector"
        );
    }

    /// The full thread path against a live lookalike child: the
    /// observer thread itself (not the direct `collect` seam) must read
    /// the decoy's true argv off the process table — including its
    /// FIRST poll, which runs before the shell rewrites its argument
    /// area — and land the same captures the argv yields when handed
    /// over directly.
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn observer_thread_captures_a_live_lookalike() {
        use std::time::Duration;

        let tmp = tempfile::tempdir().expect("tempdir");
        let flutter = tmp.path().join("sdk").join("bin").join("flutter");
        let output = tmp.path().join("build/app/intermediates/flutter/release");
        let mut tokens: Vec<String> = [
            "--quiet",
            "assemble",
            "--no-version-check",
            "--output",
        ]
        .iter()
        .map(|token| (*token).to_owned())
        .collect();
        tokens.push(output.to_string_lossy().into_owned());
        tokens.push("-dTargetFile=lib/main.dart".to_owned());
        tokens.push("kernel_snapshot".to_owned());
        let mut command = std::process::Command::new("/bin/sh");
        command.arg("-c").arg("sleep 1.5; true");
        command.arg(flutter.to_string_lossy().into_owned());
        for token in &tokens {
            command.arg(token);
        }
        let mut child = command.spawn().expect("spawn lookalike");

        let observer = CaptureObserver::start(
            tmp.path().to_path_buf(),
            "enginerev0000000000000000".to_owned(),
        );
        // The decoy outlives several poll intervals.
        std::thread::sleep(Duration::from_millis(700));
        let captures = observer.finish();
        let _ = child.kill();
        let _ = child.wait();

        // The same argv through the direct seam, for the capture the
        // thread must land. Equality of the whole capture set is NOT
        // asserted: a poll can read the kernel blob mid-way through the
        // shell rewriting its argument area, and the hybrid capture
        // that yields is dropped by the post-run name check in
        // production — this test only pins the true-argv capture.
        let mut direct_argv: Vec<String> = vec![
            "/bin/sh".to_owned(),
            "-c".to_owned(),
            "sleep 1.5; true".to_owned(),
            flutter.to_string_lossy().into_owned(),
        ];
        direct_argv.extend(tokens.iter().cloned());
        let mut seen = FxHashSet::default();
        // Handed over as the kernel spells it: the exact parse alone,
        // no join-variants enumerated for the trailing target.
        let direct = collect(
            vec![direct_argv],
            tmp.path(),
            "enginerev0000000000000000",
            &mut seen,
            false,
        );
        assert_eq!(direct.len(), 1, "the clean kernel argv: the exact parse");
        let base = &direct[0];
        assert!(
            captures
                .iter()
                .any(|capture| capture == base),
            "the thread landed the direct-seam base capture: {captures:?}"
        );
    }

    /// The per-poll cost budget: one in-process table pass over the
    /// live process table must stay under 5 ms at the median (the audit
    /// measured 0.343 ms at 605 processes; the multiple keeps the gate
    /// robust on a loaded host, where a rerun is the right response).
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn process_table_poll_stays_under_budget() {
        use std::time::{Duration, Instant};

        const POLLS: usize = 21;
        process_argv(); // warm the sysctl/proc caches
        let mut samples: Vec<Duration> = Vec::with_capacity(POLLS);
        for _ in 0..POLLS {
            let started = Instant::now();
            let table = process_argv();
            samples.push(started.elapsed());
            drop(table);
        }
        samples.sort_unstable();
        let median = samples[POLLS / 2];
        assert!(
            median < Duration::from_millis(5),
            "median poll {median:?} over {POLLS} live-table polls exceeds \
             the 5 ms budget"
        );
    }
}
