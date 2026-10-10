//! Shared helpers for the live-process test suites.

use std::process::Command;

/// Whether a `dart` executable answers on `PATH`; a CI runner without the
/// Dart SDK skips the live suites instead of going red.
#[must_use]
pub fn dart_available() -> bool {
    Command::new("dart")
        .arg("--version")
        .output()
        .map(|out| out.status.success())
        .unwrap_or(false)
}
