//! Which SDKs the resident server runs on: the SDK behind a `dart`
//! binary, its version, and the version gate.
//!
//! The resident path speaks the analysis server's classic protocol exactly
//! as `dart analyze` does (upstream Dart SDK source,
//! `pkg/dartdev/lib/src/analysis_server.dart`). The request sequence
//! (`server.setSubscriptions[STATUS]`, `analysis.setAnalysisRoots`), the
//! launch flags (`--cache`, `--disable-status-notification-debouncing`,
//! …) and the `AnalysisError` fields the json face prints (`correction`,
//! `contextMessages`, `url`, end positions) are identical from Dart 3.8.1
//! through 3.13.7 — the floor is Dart 3.8.0. `analysis_server_plugin`
//! plugins (the top-level `plugins:` options key) load from Dart 3.9.0,
//! the floor of the first published `analysis_server_plugin` package.

use std::path::{Path, PathBuf};

/// The oldest SDK the resident path serves.
pub const FLOOR: Version = Version(3, 8, 0);
/// The first SDK whose server loads `analysis_server_plugin` plugins.
pub const PLUGINS: Version = Version(3, 9, 0);

/// A release version `major.minor.patch`; pre-releases do not parse.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version(pub u32, pub u32, pub u32);

impl Version {
    /// Parses a `version` file's text: `3.13.7`, surrounding whitespace
    /// allowed. A pre-release or dev build (`3.10.0-12.0.dev`) is `None`:
    /// it may predate the release behavior the gate promises.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        let mut parts = text.trim().split('.');
        let version = Self(
            parts.next()?.parse().ok()?,
            parts.next()?.parse().ok()?,
            parts.next()?.parse().ok()?,
        );
        parts.next().is_none().then_some(version)
    }
}

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.0, self.1, self.2)
    }
}

/// How an SDK may be served.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Support {
    /// Below the floor or unparsable: per-run delegation only.
    PerRun,
    /// Resident; `plugins` when the server loads `analysis_server_plugin`
    /// plugins.
    Resident { plugins: bool },
}

/// The gate over a `version` file's text.
#[must_use]
pub fn support(version_text: &str) -> Support {
    match Version::parse(version_text) {
        Some(version) if version >= FLOOR => Support::Resident {
            plugins: version >= PLUGINS,
        },
        _ => Support::PerRun,
    }
}

/// One Dart SDK: its root and version.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sdk {
    pub root: PathBuf,
    pub version: Version,
}

impl Sdk {
    /// The SDK behind the `dart` binary the toolchain resolved: the plain
    /// SDK layout (`<sdk>/bin/dart`) or Flutter's wrapper script
    /// (`<flutter>/bin/dart`, SDK at `<flutter>/bin/cache/dart-sdk`).
    /// `None` when neither layout holds or the version does not parse.
    #[must_use]
    pub fn of_dart(dart: &Path) -> Option<Self> {
        let bin = std::fs::canonicalize(dart).ok()?.parent()?.to_path_buf();
        let root = [
            bin.join("cache").join("dart-sdk"),
            bin.parent()?.to_path_buf(),
        ]
        .into_iter()
        .find(|root| root.join("bin").join("snapshots").is_dir())?;
        let version = Version::parse(&std::fs::read_to_string(root.join("version")).ok()?)?;
        Some(Self { root, version })
    }

    /// The analysis server command: the AOT snapshot under `dartaotruntime`
    /// when the SDK ships it (Dart 3.9 and later — `dart analyze`'s own
    /// default), else the JIT snapshot under `dart`.
    #[must_use]
    pub fn server_command(&self) -> (PathBuf, PathBuf) {
        let bin = self.root.join("bin");
        let aot = bin
            .join("snapshots")
            .join("analysis_server_aot.dart.snapshot");
        let runtime = bin.join(exe("dartaotruntime"));
        if aot.is_file() && runtime.is_file() {
            return (runtime, aot);
        }
        (
            bin.join(exe("dart")),
            bin.join("snapshots").join("analysis_server.dart.snapshot"),
        )
    }
}

fn exe(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The gate: below 3.8 and anything unparsable run per-run; 3.8 is
    /// resident without the plugin face; 3.9 and later load plugins.
    #[test]
    fn the_version_gate_decides_resident_and_plugins() {
        assert_eq!(support("3.7.3"), Support::PerRun);
        assert_eq!(support("2.19.6\n"), Support::PerRun);
        assert_eq!(support("3.8.0"), Support::Resident { plugins: false });
        assert_eq!(support("3.8.1\n"), Support::Resident { plugins: false });
        assert_eq!(support("3.9.0"), Support::Resident { plugins: true });
        assert_eq!(support("3.10.9"), Support::Resident { plugins: true });
        assert_eq!(support("3.13.7"), Support::Resident { plugins: true });
        assert_eq!(support("4.0.0"), Support::Resident { plugins: true });
        for garbage in ["", "abc", "3.8", "3.8.0.1", "3.10.0-12.0.dev", "3.x.0"] {
            assert_eq!(support(garbage), Support::PerRun, "{garbage:?}");
        }
    }
}
