//! Plugin discovery: turning an on-disk workspace into spawn-ready plugin
//! manifests (PRD §14.2's one-host-process-per-plugin starts with finding
//! them).
//!
//! # Policy
//!
//! Two sources, both explicit about their rule:
//!
//! 1. **Convention**: `<workspace_root>/plugins/*/dartq_plugin.json` — every
//!    direct subdirectory of `plugins/` that holds a manifest is a plugin.
//!    Subdirectories without one are not plugins (notes, scratch, build
//!    output) and are ignored.
//! 2. **Overrides**: explicit paths supplied by the caller (CLI flag, daemon
//!    config). Each is either a manifest file or a directory containing one.
//!    A missing override path is a reported error, never a silent skip — the
//!    caller named it.
//!
//! Overrides take precedence over the convention scan (one manifest never
//! loads twice, matched by canonical path). Every load failure — unreadable
//! file, malformed JSON, invalid field, missing entrypoint script, duplicate
//! plugin name — lands in [`DiscoveryReport::skipped`] as structured data;
//! discovery never panics and never aborts the scan, because one broken
//! plugin must not hide the others (the §14.3 isolation posture, applied to
//! discovery).

use std::path::{Path, PathBuf};

use crate::manifest::{MANIFEST_FILE_NAME, Manifest, ManifestError};

/// One discovered plugin: its parsed manifest and the directory the
/// entrypoint resolves against ([`crate::SpawnSpec::dart`] consumes both).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiscoveredPlugin {
    /// The plugin directory (the manifest's parent).
    pub dir: PathBuf,
    /// The parsed and validated manifest.
    pub manifest: Manifest,
}

/// One skipped manifest: where and why. Structured, never a panic.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiscoveryIssue {
    /// The manifest path that failed.
    pub path: PathBuf,
    /// The parse/validation failure.
    pub error: ManifestError,
}

/// The outcome of one [`discover`] run: spawn-ready plugins plus everything
/// skipped with its structured reason.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DiscoveryReport {
    /// Valid plugins, convention hits first (path-sorted), then overrides in
    /// call order. Duplicate names keep the first occurrence.
    pub plugins: Vec<DiscoveredPlugin>,
    /// Every manifest that failed to load, with the reason.
    pub skipped: Vec<DiscoveryIssue>,
}

/// Discovers plugin manifests under `workspace_root/plugins/` and merges the
/// explicit `overrides` (each a manifest file or a plugin directory).
#[must_use]
pub fn discover(workspace_root: &Path, overrides: &[PathBuf]) -> DiscoveryReport {
    let mut report = DiscoveryReport::default();
    let mut loaded: Vec<PathBuf> = Vec::new();

    for override_path in overrides {
        let manifest_path = if override_path.is_dir() {
            override_path.join(MANIFEST_FILE_NAME)
        } else {
            override_path.clone()
        };
        let key = canonical(&manifest_path);
        if loaded.contains(&key) {
            continue;
        }
        match load_plugin(&manifest_path) {
            Ok(plugin) => {
                loaded.push(key);
                admit(&mut report, plugin, manifest_path);
            }
            Err(error) => report.skipped.push(DiscoveryIssue {
                path: manifest_path,
                error,
            }),
        }
    }

    let mut manifest_paths: Vec<PathBuf> = std::fs::read_dir(workspace_root.join("plugins"))
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|t| t.is_dir()))
        .map(|entry| entry.path().join(MANIFEST_FILE_NAME))
        .filter(|path| path.is_file())
        .collect();
    manifest_paths.sort();
    for manifest_path in manifest_paths {
        let key = canonical(&manifest_path);
        if loaded.contains(&key) {
            continue;
        }
        match load_plugin(&manifest_path) {
            Ok(plugin) => {
                loaded.push(key);
                admit(&mut report, plugin, manifest_path);
            }
            Err(error) => report.skipped.push(DiscoveryIssue {
                path: manifest_path,
                error,
            }),
        }
    }
    report
}

/// Loads and validates one manifest: parse, then confirm the entrypoint
/// resolves to a script inside the plugin directory (an absolute entrypoint
/// is honored; `Path::join` replaces the base with it).
fn load_plugin(manifest_path: &Path) -> Result<DiscoveredPlugin, ManifestError> {
    let manifest = Manifest::from_path(manifest_path)?;
    let dir = manifest_path
        .parent()
        .unwrap_or(Path::new(""))
        .to_path_buf();
    let script = dir.join(&manifest.entrypoint);
    if !script.is_file() {
        return Err(ManifestError::InvalidField(
            "entrypoint",
            format!("no script at `{}`", script.display()),
        ));
    }
    Ok(DiscoveredPlugin { dir, manifest })
}

/// Admits one loaded plugin, rejecting a duplicate plugin name (first wins).
fn admit(report: &mut DiscoveryReport, plugin: DiscoveredPlugin, path: PathBuf) {
    if report
        .plugins
        .iter()
        .any(|known| known.manifest.name == plugin.manifest.name)
    {
        report.skipped.push(DiscoveryIssue {
            path,
            error: ManifestError::InvalidField(
                "name",
                format!("duplicate plugin name `{}`", plugin.manifest.name),
            ),
        });
    } else {
        report.plugins.push(plugin);
    }
}

/// The dedupe key of a manifest path: its canonical form when the file
/// exists, the path itself otherwise (an override may point nowhere).
fn canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scratch workspace with `plugins/` subdirectories; files are plain
    /// manifests and scripts so `load_plugin` validation sees them.
    struct Workspace(PathBuf);

    impl Workspace {
        fn new(tag: &str) -> Self {
            let root =
                std::env::temp_dir().join(format!("dartq-discovery-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(root.join("plugins")).expect("create workspace");
            Self(root)
        }

        fn plugin(&self, name: &str, manifest: &str) -> PathBuf {
            let dir = self.0.join("plugins").join(name);
            std::fs::create_dir_all(&dir).expect("create plugin dir");
            std::fs::write(dir.join("script.dart"), "void main() {}").expect("write script");
            std::fs::write(dir.join(MANIFEST_FILE_NAME), manifest).expect("write manifest");
            dir
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for Workspace {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn valid_manifest(entrypoint: &str) -> String {
        format!(
            r#"{{"name": "demo", "version": "0.1.0", "entrypoint": "{entrypoint}", "capabilities": ["lint"]}}"#
        )
    }

    #[test]
    fn convention_scan_finds_plugins_and_ignores_non_plugin_dirs() {
        let workspace = Workspace::new("scan");
        let dir = workspace.plugin("demo", &valid_manifest("script.dart"));
        // A subdirectory without a manifest is not a plugin.
        std::fs::create_dir_all(workspace.path().join("plugins").join("notes")).expect("dir");

        let report = discover(workspace.path(), &[]);
        assert_eq!(report.skipped, Vec::new());
        assert_eq!(report.plugins.len(), 1);
        let plugin = &report.plugins[0];
        assert_eq!(plugin.dir, dir);
        assert_eq!(plugin.manifest.name, "demo");
        assert_eq!(plugin.manifest.entrypoint, "script.dart");
    }

    #[test]
    fn missing_plugins_directory_is_an_empty_report_not_an_error() {
        let workspace = Workspace::new("empty");
        std::fs::remove_dir(workspace.path().join("plugins")).expect("drop plugins dir");
        assert_eq!(discover(workspace.path(), &[]), DiscoveryReport::default());
    }

    #[test]
    fn broken_manifests_are_skipped_with_structured_errors() {
        let workspace = Workspace::new("broken");
        workspace.plugin(
            "bad_budget",
            r#"{"name": "bad_budget", "version": "0.1.0", "entrypoint": "script.dart", "memory_budget_mb": 0}"#,
        );
        workspace.plugin("no_script", &valid_manifest("missing.dart"));

        let report = discover(workspace.path(), &[]);
        assert!(report.plugins.is_empty());
        assert_eq!(report.skipped.len(), 2);
        assert!(report.skipped.iter().any(|issue| {
            issue.path.file_name().expect("file name") == "dartq_plugin.json"
                && issue.path.to_string_lossy().contains("bad_budget")
                && issue.error
                    == ManifestError::InvalidField(
                        "memory_budget_mb",
                        "must be a positive integer".to_owned(),
                    )
        }));
        assert!(
            report
                .skipped
                .iter()
                .any(|issue| issue.error.to_string().contains("missing.dart"))
        );
    }

    #[test]
    fn overrides_accept_manifest_files_and_directories_and_report_missing_paths() {
        let workspace = Workspace::new("overrides");
        let dir = workspace.plugin("demo", &valid_manifest("script.dart"));

        let report = discover(
            workspace.path(),
            &[dir.clone(), dir.join(MANIFEST_FILE_NAME)],
        );
        assert_eq!(report.plugins.len(), 1, "both override spellings load once");
        assert!(report.skipped.is_empty());

        let missing = workspace.path().join("plugins").join("nowhere");
        let report = discover(workspace.path(), &[missing]);
        // The convention scan still finds the fixture; only the named
        // override is reported.
        assert_eq!(report.plugins.len(), 1);
        assert_eq!(
            report.skipped.len(),
            1,
            "a named override must not skip silently"
        );
        assert!(report.skipped[0].path.to_string_lossy().contains("nowhere"));
    }

    #[test]
    fn duplicate_names_keep_the_first_plugin() {
        let workspace = Workspace::new("dupes");
        workspace.plugin("a_demo", &valid_manifest("script.dart"));
        // Same plugin name in a second directory (a valid one: only the name
        // clash may disqualify it).
        workspace.plugin("b_demo", &valid_manifest("script.dart"));

        let report = discover(workspace.path(), &[]);
        assert_eq!(report.plugins.len(), 1);
        assert!(report.plugins[0].dir.to_string_lossy().contains("a_demo"));
        assert_eq!(report.skipped.len(), 1);
        assert!(report.skipped[0].error.to_string().contains("duplicate"));
    }
}
