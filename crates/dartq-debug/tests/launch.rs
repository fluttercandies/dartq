//! Hermetic launch-path tests: the `dart` binary is never started —
//! only discovery failures are exercised.

use dartq_debug::{DebugSession, Error, LaunchConfig};

#[test]
fn launch_reports_missing_dart_binary() {
    let missing =
        std::env::temp_dir().join(format!("dartq-debug-missing-dart-{}", std::process::id()));
    assert!(!missing.exists(), "test path must not exist");
    let previous = std::env::var_os("DARTQ_DART_BIN");
    // SAFETY: this test binary contains exactly one test, so no other
    // thread reads or writes the process environment concurrently.
    unsafe { std::env::set_var("DARTQ_DART_BIN", &missing) };
    let result = DebugSession::launch(LaunchConfig::new("main.dart"));
    match previous {
        Some(value) => unsafe { std::env::set_var("DARTQ_DART_BIN", value) },
        None => unsafe { std::env::remove_var("DARTQ_DART_BIN") },
    }
    let error = match result {
        Ok(_session) => panic!("a missing dart binary must fail clearly"),
        Err(error) => error,
    };
    assert!(
        matches!(error, Error::DartBinaryNotFound(_)),
        "got {error:?}"
    );
}
