//! Protocol violations end-to-end: every wire-contract break —
//! an unknown method, a foreign `schema_version`, non-UTF-8 bytes, a frame
//! beyond the 16 MiB bound — trips the reader into the death slot, consumes
//! the restart budget through the ordinary crash path, and surfaces the
//! terminal [`RuntimeError::Crashed`] whose death record names the violation
//! and the exhausted budget. [`crash_diagnostic`] renders it as the
//! `dartq/plugin_crashed` tool diagnostic the output layer merges. The
//! oversized form additionally pins the inbound frame guard's drop counter
//! (its outbound twin lives in `outbound_frame_guard.rs`).
//!
//! Each violating child handshakes cleanly, eats one fed batch, answers it
//! with the violation, and idles — so every relaunch repeats the violation
//! deterministically: batch 1 crosses (the violation lands right after), the
//! death restarts within budget on batch 2, and batch 3 finds the budget
//! spent. A codec-level break counts a frame-guard drop; a session-level
//! break (unknown method) does not — the frame decoded fine.
//!
//! Unix-gated: the children are `sh` one-liners, deliberately not Dart
//! processes (same posture as `crash_restart.rs`).
#![cfg(unix)]

use std::path::Path;
use std::time::{Duration, Instant};

use dartq_diagnostics::{Diagnostic, Severity, SourceRange, URI_DOES_NOT_EXIST};
use dartq_plugin::{
    CyclePush, MAX_FRAME_SIZE, Manifest, PluginRuntime, RuntimeError, SpawnSpec, StdioPolicy,
    crash_diagnostic,
};
use dartq_source::FileTable;

/// The handshake reply every violating child sends (the core's request id is
/// 1), so the spawn itself stays clean and the violation is a mid-session
/// break, not a failed startup.
const HANDSHAKE_REPLY: &str = r#"{"schema_version":1,"id":1,"method":"initialize","params":{"protocol_version":1,"capabilities":[]}}"#;

/// The violation is emitted after one stdin line is eaten (the first fed
/// batch), so the chain below cannot race the spawn. The trailing sleep keeps
/// the child alive until the enforcement kill; its stdout is redirected so no
/// orphan could hold the pipe (same discipline as `write_timeout.rs`).
fn violation_script(emit: &str) -> String {
    format!("read line; printf '%s\\n' '{HANDSHAKE_REPLY}'; read line; {emit}; sleep 30 >/dev/null")
}

fn violating_runtime(emit: &str) -> PluginRuntime {
    // The callback budget stays far above the test's runtime so the analyze
    // arm never fires; the violation itself is the only death at work.
    let manifest = Manifest::parse(
        r#"{"name": "violator", "version": "0.1.0", "entrypoint": "bin/nothing",
            "timeouts": {"callback_ms": 30000, "request_ms": 5000, "request_retries": 1}}"#,
    )
    .expect("manifest parses");
    PluginRuntime::spawn(SpawnSpec {
        program: "sh".to_owned(),
        args: vec!["-c".to_owned(), violation_script(emit)],
        current_dir: Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf(),
        manifest,
        stderr: StdioPolicy::Null,
        serve_queries: false,
    })
    .expect("the violating child still handshakes cleanly")
}

/// Waits out one violation's death with bounded polling. Each short
/// `wait_diagnostics` window's alive gates convert a landed death into
/// exactly one observable: its restart (budget left — visible as a
/// `stats.restarts` bump) or the terminal crash (budget spent — visible as
/// the `Err`). Fixed sleeps cannot do this job: how long a death takes to
/// land is form-specific (the oversize form's 17 MiB must drain through the
/// pipe before the decoder guard trips), and a too-short constant turns the
/// terminal feed into a silent `Ok` against the still-alive generation.
fn await_death_settled(runtime: &mut PluginRuntime, cap: Duration) {
    let deadline = Instant::now() + cap;
    while Instant::now() < deadline {
        let restarts_before = runtime.stats().restarts;
        match runtime.wait_output(Duration::from_millis(50)) {
            // Budget spent: the death surfaced terminally.
            Err(RuntimeError::Crashed { .. }) => return,
            Err(error) => {
                panic!("unexpected error while awaiting the violation's death: {error:?}")
            }
            Ok(empty) => {
                assert!(
                    empty.diagnostics.is_empty(),
                    "the violator never produces diagnostics"
                );
                if runtime.stats().restarts > restarts_before {
                    return; // the death surfaced and its restart was consumed
                }
            }
        }
    }
    panic!("the violation's death did not settle within {cap:?}");
}

/// The full violation chain, pinned for every form: batch 1 crosses and the
/// violation kills the generation; batch 2 restarts within budget; batch 3
/// finds the budget spent and surfaces the terminal crash naming the
/// violation and the exhausted budget, rendered as the crash diagnostic.
fn pin_violation_chain(emit: &str, reason_needle: &str, expected_drops: u64) {
    let mut runtime = violating_runtime(emit);
    let mut files = FileTable::new();
    let anchor = files.intern(Path::new("lib/main.dart"));
    let batch = [Diagnostic::new(
        Severity::Info,
        URI_DOES_NOT_EXIST,
        "context for the violator",
        anchor,
        SourceRange {
            offset: 0,
            length: 0,
        },
        None,
    )];

    runtime
        .feed(&batch, &files, &CyclePush::default())
        .expect("batch 1 crosses; the violation lands after it");
    await_death_settled(&mut runtime, Duration::from_secs(5));
    runtime
        .feed(&batch, &files, &CyclePush::default())
        .expect("the violation's death restarted within budget");
    await_death_settled(&mut runtime, Duration::from_secs(5));

    let terminal = runtime
        .feed(&batch, &files, &CyclePush::default())
        .expect_err("a deterministically violating plugin spends the budget");
    assert!(
        matches!(&terminal, RuntimeError::Crashed { reason }
            if reason.contains(reason_needle) && reason.contains("restart budget of 1 exhausted")),
        "the death record names the violation and the spent budget: {terminal:?}"
    );
    let stats = runtime.stats();
    assert_eq!(
        stats.restarts, 1,
        "exactly one restart was consumed: {stats:?}"
    );
    assert_eq!(
        stats.frames_dropped, expected_drops,
        "the frame guard's drop count: {stats:?}"
    );

    // The terminal form renders as the crash diagnostic the output layer
    // merges.
    let diagnostic = crash_diagnostic("violator", &terminal, anchor);
    assert_eq!(diagnostic.code.name, "dartq/plugin_crashed");
    assert!(diagnostic.message.contains("violator"));
}

#[test]
fn unknown_method_violation_runs_the_full_crash_chain() {
    pin_violation_chain(
        r#"printf '%s\n' '{"schema_version":1,"id":null,"method":"plugin/exotic","params":{}}'"#,
        "unknown method `plugin/exotic`",
        0,
    );
}

#[test]
fn foreign_schema_version_violation_runs_the_full_crash_chain() {
    pin_violation_chain(
        r#"printf '%s\n' '{"schema_version":99,"id":null,"method":"initialize","params":{}}'"#,
        "unsupported schema_version 99",
        0,
    );
}

#[test]
fn non_utf8_line_violation_runs_the_full_crash_chain() {
    // Raw bytes no UTF-8 decoder accepts, terminated like any frame.
    pin_violation_chain(r"printf '\377\376\n'", "not valid UTF-8", 0);
}

#[test]
fn oversize_inbound_frame_runs_the_full_crash_chain_and_counts_the_drop() {
    // 17 MiB of newline-free bytes: the decoder trips the bound before any
    // buffering — once per generation, each counted by the frame guard.
    pin_violation_chain(
        "dd if=/dev/zero bs=1048576 count=17",
        &format!("frame exceeds the {MAX_FRAME_SIZE}-byte frame limit"),
        2,
    );
}
