//! Startup stderr surfacing: a host that fails before readiness has its
//! stderr captured (bounded tail) and folded into the surfaced error, so a
//! startup failure carries the host's own error output instead of a bare
//! "host died". The observable semantic mirrors the official plugin
//! manager's failure report, which includes the subprocess's stderr
//! (`plugin_manager.dart`: `buffer.writeln('  stderr = $stderr')`).
//!
//! Unix-gated: the children are `sh` one-liners, deliberately not Dart
//! processes (same posture as `crash_restart.rs`).
#![cfg(unix)]

use std::path::Path;

use dartq_plugin::{Manifest, PluginRuntime, RuntimeError, SpawnSpec, StdioPolicy};

fn spec(script: &str, retries: u32, request_ms: u64, stderr: StdioPolicy) -> SpawnSpec {
    let manifest = Manifest::parse(&format!(
        r#"{{"name": "chatty", "version": "0.1.0", "entrypoint": "bin/nothing",
            "timeouts": {{"request_ms": {request_ms}, "request_retries": {retries}}}}}"#
    ))
    .expect("manifest parses");
    SpawnSpec {
        program: "sh".to_owned(),
        args: vec!["-c".to_owned(), script.to_owned()],
        current_dir: Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf(),
        manifest,
        stderr,
        serve_queries: false,
    }
}

#[test]
fn startup_death_surfaces_the_host_stderr_in_the_crash_reason() {
    let error = PluginRuntime::spawn(spec(
        "echo boom >&2; exit 1",
        0,
        5_000,
        StdioPolicy::Capture,
    ))
    .expect_err("a child that dies at startup cannot handshake");
    match &error {
        RuntimeError::Crashed { reason } => {
            assert!(
                reason.contains("boom"),
                "the host's stderr reached the error: {reason}"
            );
            assert!(
                reason.contains("plugin stderr"),
                "the stderr is marked as the host's output: {reason}"
            );
        }
        other => panic!("terminal startup death is Crashed, got {other:?}"),
    }
}

#[test]
fn exhausted_restart_budget_still_surfaces_the_last_attempt_stderr() {
    // Every attempt dies the same way; the terminal error carries the last
    // attempt's capture, not an empty slot.
    let error = PluginRuntime::spawn(spec(
        "echo boom >&2; exit 1",
        2,
        5_000,
        StdioPolicy::Capture,
    ))
    .expect_err("a child that dies at startup cannot handshake");
    match &error {
        RuntimeError::Crashed { reason } => {
            assert!(
                reason.contains("boom"),
                "the last capture reached the error: {reason}"
            );
            assert!(
                reason.contains("restart budget"),
                "the budget note survived the fold: {reason}"
            );
        }
        other => panic!("terminal startup death is Crashed, got {other:?}"),
    }
}

#[test]
fn handshake_timeout_carries_the_host_stderr() {
    // Writes a diagnostic to stderr, then hangs past the request budget: the
    // typed timeout names the budget AND what the host said before going
    // quiet.
    let error = PluginRuntime::spawn(spec(
        "echo boom >&2; sleep 30",
        0,
        500,
        StdioPolicy::Capture,
    ))
    .expect_err("a silent child cannot complete the handshake");
    match &error {
        RuntimeError::HandshakeTimeout { ms, stderr } => {
            assert_eq!(*ms, 500);
            assert!(
                stderr.contains("boom"),
                "the host's stderr reached the timeout: {stderr}"
            );
        }
        other => panic!("the typed timeout with the budget surfaces, got {other:?}"),
    }
}

#[test]
fn null_policy_keeps_stderr_out_of_the_error() {
    let error = PluginRuntime::spawn(spec("echo boom >&2; exit 1", 0, 5_000, StdioPolicy::Null))
        .expect_err("a child that dies at startup cannot handshake");
    match &error {
        RuntimeError::Crashed { reason } => {
            assert!(
                !reason.contains("plugin stderr"),
                "a discarded capture must not be invented: {reason}"
            );
        }
        other => panic!("terminal startup death is Crashed, got {other:?}"),
    }
}
