//! Plugin discovery: turning an on-disk workspace into spawn-ready plugin
//! manifests (one-host-process-per-plugin starts with finding
//! them).
//!
//! # Policy
//!
//! Three sources, each explicit about its rule:
//!
//! 1. **Overrides**: explicit paths supplied by the caller (CLI flag, daemon
//!    config). Each is either a manifest file or a directory containing one.
//!    A missing override path is a reported error, never a silent skip — the
//!    caller named it.
//! 2. **Convention**: `<workspace_root>/plugins/*/dartq_plugin.json` — every
//!    direct subdirectory of `plugins/` that holds a manifest is a plugin.
//!    Subdirectories without one are not plugins (notes, scratch, build
//!    output) and are ignored.
//! 3. **Pub-native**: the `plugins:` section of the workspace's governing
//!    `analysis_options.yaml` (upstream Dart SDK source,
//!    `pkg/analysis_server_plugin/doc/using_plugins.md` — a plugin is a normal
//!    pub dependency, declared by package name and resolved by pub into
//!    `package_config.json`). Each declared name resolves to its package
//!    root; a root holding a manifest loads exactly like a convention hit
//!    (a ported plugin), otherwise the manifest the official convention
//!    implies is derived (entrypoint `lib/main.dart`, the manifest's default
//!    budgets). A declared name pub never resolved is a reported issue,
//!    never a silent skip — the user declared it.
//!
//! Precedence follows source order — overrides first, then the convention
//! scan, then pub-native; one manifest never loads twice (matched by
//! canonical path), and duplicate plugin names keep the first occurrence.
//! Every load failure — unreadable file, malformed JSON, invalid field,
//! missing entrypoint script, duplicate plugin name, unresolved declaration
//! — lands in [`DiscoveryReport::skipped`] as structured data; discovery
//! never panics and never aborts the scan, because one broken plugin must
//! not hide the others.

use std::path::{Path, PathBuf};

use dartq_pub::{PackageConfig, normalize};

use crate::capability::CapabilitySet;
use crate::manifest::{
    DEFAULT_IDLE_EVICT_MS, DEFAULT_MEMORY_BUDGET_MB, EntrypointKind, MANIFEST_FILE_NAME, Manifest,
    ManifestError, Timeouts,
};

/// The entrypoint the official plugin convention fixes for a pub-native
/// plugin package (upstream Dart SDK source,
/// `pkg/analysis_server_plugin/doc/writing_a_plugin.md`: the package's single
/// entry is a top-level `plugin` variable in `lib/main.dart`).
const PUB_PLUGIN_ENTRYPOINT: &str = "lib/main.dart";

/// One discovered plugin: its parsed manifest and the directory the
/// entrypoint resolves against ([`crate::SpawnSpec::dart`] consumes both).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiscoveredPlugin {
    /// The plugin directory (the manifest's parent).
    pub dir: PathBuf,
    /// The parsed and validated manifest.
    pub manifest: Manifest,
}

/// One discovery failure: a manifest-level parse/validation error, or a
/// pub-native declaration that pub's resolution cannot back.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum DiscoveryError {
    /// The manifest failed to load or validate (the manifest face's own
    /// typed error).
    #[error(transparent)]
    Manifest(#[from] ManifestError),
    /// The `plugins:` section declares the package, but
    /// `package_config.json` has no entry for it — pub never resolved it.
    #[error(
        "plugin package `{0}` is declared in analysis_options but not resolved by pub; run `pub get`"
    )]
    Unresolved(String),
    /// The resolved package's `rootUri` cannot be turned into a path (an
    /// unsupported scheme or malformed escape — a broken pub artifact).
    #[error("plugin package `{0}` has an unusable rootUri: {1}")]
    RootUri(String, String),
}

/// One skipped plugin: where and why. Structured, never a panic.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiscoveryIssue {
    /// The anchor of the failure: the manifest path for manifest-level
    /// errors, the declaring options file for an unresolved declaration,
    /// the entrypoint for a derived plugin with no script.
    pub path: PathBuf,
    /// The failure.
    pub error: DiscoveryError,
}

/// The outcome of one [`discover`] run: spawn-ready plugins plus everything
/// skipped with its structured reason.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DiscoveryReport {
    /// Valid plugins, overrides in call order, then convention hits
    /// (path-sorted), then pub-native hits (declaration order). Duplicate
    /// names keep the first occurrence.
    pub plugins: Vec<DiscoveredPlugin>,
    /// Every plugin that failed to load, with the reason.
    pub skipped: Vec<DiscoveryIssue>,
}

/// The pub-native source: the `plugins:` section of the workspace's
/// governing `analysis_options.yaml`, resolved against the parsed
/// `package_config.json`. The caller supplies both already parsed (the
/// daemon locates the options file and reuses the existing options and pub
/// readers); discovery owns what each declaration means.
#[derive(Clone, Debug)]
pub struct PubNative<'a> {
    /// Declared plugin package names. Empty when the options file is
    /// absent or declares no `plugins:` section — the source then
    /// contributes nothing and costs nothing beyond the caller's read.
    pub declared: &'a [String],
    /// The parsed `package_config.json` (pub's resolution): where each
    /// declared package lives.
    pub config: &'a PackageConfig,
    /// The directory holding `package_config.json` (the `rootUri`
    /// resolution base, normally the workspace's `.dart_tool/`).
    pub config_dir: &'a Path,
    /// The options file the declarations came from — the issue anchor for
    /// declarations pub cannot resolve. Unused when `declared` is empty.
    pub options_file: &'a Path,
}

/// Discovers plugin manifests from the three explicit sources: the
/// `overrides` (each a manifest file or a plugin directory), the
/// `workspace_root/plugins/` convention scan, and the pub-native
/// `pub_native.declared` packages resolved through `pub_native.config`.
#[must_use]
#[expect(clippy::too_many_lines)] // long diagnostic-table dispatch
pub fn discover(
    workspace_root: &Path,
    overrides: &[PathBuf],
    pub_native: &PubNative<'_>,
) -> DiscoveryReport {
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
                error: error.into(),
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
                error: error.into(),
            }),
        }
    }

    // Pub-native: each declared name resolves through pub's package_config.
    // Nothing here runs when no section is declared (`declared` empty) — the
    // absent-source path adds no filesystem walk and no spawn.
    for name in pub_native.declared {
        let Some(entry) = pub_native.config.entry(name) else {
            report.skipped.push(DiscoveryIssue {
                path: pub_native.options_file.to_path_buf(),
                error: DiscoveryError::Unresolved(name.clone()),
            });
            continue;
        };
        let root = match entry.resolve_root(pub_native.config_dir) {
            // `resolve_root` only lexically joins (the dartq-pub contract);
            // normalize so the relative `../` rootUri pub writes for
            // workspace members yields the plain package path — the dedup
            // key canonicalizes, and plugin dirs / issue anchors spell the
            // one real directory instead of `.dart_tool/../pkgs/x`.
            Ok(root) => normalize(&root),
            Err(reason) => {
                report.skipped.push(DiscoveryIssue {
                    path: pub_native.options_file.to_path_buf(),
                    error: DiscoveryError::RootUri(name.clone(), reason.to_string()),
                });
                continue;
            }
        };
        // A ported plugin ships `dartq_plugin.json`; it wins over the
        // derived default() and loads exactly like a convention hit (same
        // validation, same dedup keys).
        let manifest_path = root.join(MANIFEST_FILE_NAME);
        let key = canonical(&manifest_path);
        if loaded.contains(&key) {
            continue;
        }
        if manifest_path.exists() && !manifest_path.is_file() {
            // A directory-shaped manifest is a malformed ported plugin, not
            // an unported package: anchor the issue at the manifest itself
            // instead of deriving the entrypoint it shadows.
            report.skipped.push(DiscoveryIssue {
                error: ManifestError::Io {
                    path: manifest_path.display().to_string(),
                    reason: "the manifest is not a regular file".to_owned(),
                }
                .into(),
                path: manifest_path,
            });
            continue;
        }
        if manifest_path.is_file() {
            match load_plugin(&manifest_path) {
                Ok(plugin) => {
                    loaded.push(key);
                    admit(&mut report, plugin, manifest_path);
                }
                Err(error) => report.skipped.push(DiscoveryIssue {
                    path: manifest_path,
                    error: error.into(),
                }),
            }
            continue;
        }
        // Unported (the element-model gap is the runtime's adjudication, not
        // discovery's): derive the manifest the official convention implies
        // — the package itself is the plugin directory and `lib/main.dart`
        // is the entry. The package's pubspec version is deliberately not
        // read; identity comes from the declaration.
        let manifest = Manifest {
            name: name.clone(),
            version: "0.0.0".to_owned(),
            entrypoint: PUB_PLUGIN_ENTRYPOINT.to_owned(),
            entrypoint_kind: EntrypointKind::Source,
            capabilities: CapabilitySet::empty(),
            timeouts: Timeouts::default(),
            memory_budget_mb: DEFAULT_MEMORY_BUDGET_MB,
            idle_evict_ms: DEFAULT_IDLE_EVICT_MS,
        };
        let script = root.join(PUB_PLUGIN_ENTRYPOINT);
        if !script.is_file() {
            report.skipped.push(DiscoveryIssue {
                error: ManifestError::InvalidField(
                    "entrypoint",
                    format!("no script at `{}`", script.display()),
                )
                .into(),
                path: script,
            });
            continue;
        }
        loaded.push(canonical(&script));
        admit(
            &mut report,
            DiscoveredPlugin {
                dir: root,
                manifest,
            },
            script,
        );
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
            )
            .into(),
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

        /// A pub-resolvable package root beside `plugins/` (the shape a path or
        /// cached dependency resolves to).
        fn package(&self, name: &str) -> PathBuf {
            let root = self.0.join("pkgs").join(name);
            std::fs::create_dir_all(&root).expect("create package root");
            root
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

    fn empty_config() -> PackageConfig {
        PackageConfig::parse(r#"{"configVersion": 2, "packages": []}"#).expect("valid config")
    }

    fn empty_pub_native(config: &PackageConfig) -> PubNative<'_> {
        PubNative {
            declared: &[],
            config,
            config_dir: Path::new(""),
            options_file: Path::new(""),
        }
    }

    /// The pre-pub-native discovery: the shape every source-less call had
    /// before the third source existed.
    fn discover_without_pub_native(
        workspace_root: &Path,
        overrides: &[PathBuf],
    ) -> DiscoveryReport {
        let config = empty_config();
        discover(workspace_root, overrides, &empty_pub_native(&config))
    }

    /// Discovers with one pub-native declaration; the resolved root is the
    /// package of that name under the workspace's `pkgs/`, written as the
    /// absolute `file:` rootUri shape pub emits for resolved packages.
    fn discover_pub_native(
        workspace: &Workspace,
        declared: &[&str],
        options_file: &Path,
    ) -> DiscoveryReport {
        let packages = declared
            .iter()
            .map(|name| {
                let root = workspace.path().join("pkgs").join(name);
                format!(
                    r#"{{"name": "{name}", "rootUri": "file://{}", "languageVersion": "3.5"}}"#,
                    root.display()
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        let config = PackageConfig::parse(&format!(
            r#"{{"configVersion": 2, "packages": [{packages}]}}"#
        ))
        .expect("valid config");
        let declared: Vec<String> = declared.iter().map(|name| (*name).to_owned()).collect();
        discover(
            workspace.path(),
            &[],
            &PubNative {
                declared: &declared,
                config: &config,
                config_dir: &workspace.path().join(".dart_tool"),
                options_file,
            },
        )
    }

    #[test]
    fn convention_scan_finds_plugins_and_ignores_non_plugin_dirs() {
        let workspace = Workspace::new("scan");
        let dir = workspace.plugin("demo", &valid_manifest("script.dart"));
        // A subdirectory without a manifest is not a plugin.
        std::fs::create_dir_all(workspace.path().join("plugins").join("notes")).expect("dir");

        let report = discover_without_pub_native(workspace.path(), &[]);
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
        assert_eq!(
            discover_without_pub_native(workspace.path(), &[]),
            DiscoveryReport::default()
        );
    }

    #[test]
    fn broken_manifests_are_skipped_with_structured_errors() {
        let workspace = Workspace::new("broken");
        workspace.plugin(
            "bad_budget",
            r#"{"name": "bad_budget", "version": "0.1.0", "entrypoint": "script.dart", "memory_budget_mb": 0}"#,
        );
        workspace.plugin("no_script", &valid_manifest("missing.dart"));

        let report = discover_without_pub_native(workspace.path(), &[]);
        assert!(report.plugins.is_empty());
        assert_eq!(report.skipped.len(), 2);
        assert!(report.skipped.iter().any(|issue| {
            issue.path.file_name().expect("file name") == "dartq_plugin.json"
                && issue.path.to_string_lossy().contains("bad_budget")
                && issue.error
                    == DiscoveryError::Manifest(ManifestError::InvalidField(
                        "memory_budget_mb",
                        "must be a positive integer".to_owned(),
                    ))
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

        let report = discover_without_pub_native(
            workspace.path(),
            &[dir.clone(), dir.join(MANIFEST_FILE_NAME)],
        );
        assert_eq!(report.plugins.len(), 1, "both override spellings load once");
        assert!(report.skipped.is_empty());

        let missing = workspace.path().join("plugins").join("nowhere");
        let report = discover_without_pub_native(workspace.path(), &[missing]);
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

        let report = discover_without_pub_native(workspace.path(), &[]);
        assert_eq!(report.plugins.len(), 1);
        assert!(report.plugins[0].dir.to_string_lossy().contains("a_demo"));
        assert_eq!(report.skipped.len(), 1);
        assert!(report.skipped[0].error.to_string().contains("duplicate"));
    }

    #[test]
    fn pub_native_ported_package_loads_through_its_manifest() {
        let workspace = Workspace::new("pub_ported");
        let root = workspace.package("ported");
        std::fs::write(root.join("script.dart"), "void main {}").expect("write script");
        std::fs::write(
            root.join(MANIFEST_FILE_NAME),
            r#"{"name": "ported", "version": "0.2.0", "entrypoint": "script.dart"}"#,
        )
        .expect("write manifest");
        let options = workspace.path().join("analysis_options.yaml");

        let report = discover_pub_native(&workspace, &["ported"], &options);
        assert_eq!(report.skipped, Vec::new());
        assert_eq!(report.plugins.len(), 1);
        let plugin = &report.plugins[0];
        assert_eq!(plugin.dir, root);
        assert_eq!(plugin.manifest.name, "ported");
        // The package's own manifest wins over the derived default entrypoint.
        assert_eq!(plugin.manifest.entrypoint, "script.dart");
    }

    #[test]
    fn pub_native_unported_package_gets_the_derived_default_manifest() {
        let workspace = Workspace::new("pub_unported");
        let root = workspace.package("unported");
        std::fs::create_dir_all(root.join("lib")).expect("create lib");
        std::fs::write(root.join("lib").join("main.dart"), "void main {}")
            .expect("write entrypoint");
        let options = workspace.path().join("analysis_options.yaml");

        let report = discover_pub_native(&workspace, &["unported"], &options);
        assert_eq!(report.skipped, Vec::new());
        assert_eq!(report.plugins.len(), 1);
        let plugin = &report.plugins[0];
        assert_eq!(plugin.dir, root);
        assert_eq!(plugin.manifest.name, "unported");
        assert_eq!(plugin.manifest.entrypoint, PUB_PLUGIN_ENTRYPOINT);
        // The budgets are the manifest's own defaults for absent fields; the
        // package's pubspec version is deliberately not read.
        assert_eq!(plugin.manifest.version, "0.0.0");
        assert_eq!(plugin.manifest.timeouts, Timeouts::default());
        assert_eq!(plugin.manifest.memory_budget_mb, DEFAULT_MEMORY_BUDGET_MB);
        assert!(plugin.manifest.capabilities.is_empty());
    }

    #[test]
    fn pub_native_directory_shaped_manifest_anchors_at_the_manifest() {
        let workspace = Workspace::new("pub_dir_manifest");
        let root = workspace.package("derped");
        // A pathological ported plugin: `dartq_plugin.json` exists, but as a
        // directory. The issue must name the manifest, not fall through to
        // the derived entrypoint it shadows.
        std::fs::create_dir_all(root.join(MANIFEST_FILE_NAME)).expect("create manifest dir");
        let options = workspace.path().join("analysis_options.yaml");

        let report = discover_pub_native(&workspace, &["derped"], &options);
        assert!(report.plugins.is_empty());
        assert_eq!(report.skipped.len(), 1);
        assert_eq!(report.skipped[0].path, root.join(MANIFEST_FILE_NAME));
        assert!(
            report.skipped[0]
                .error
                .to_string()
                .contains("not a regular file"),
            "the manifest itself is the anomaly: {:?}",
            report.skipped[0].error
        );
    }

    #[test]
    fn pub_native_declared_but_unresolved_is_a_structured_issue() {
        let workspace = Workspace::new("pub_ghost");
        let options = workspace.path().join("analysis_options.yaml");
        // The declaration has no package_config entry at all: pub never
        // resolved it (no `pub get`, or the dependency was never added).
        let config = empty_config();
        let declared = vec!["ghost".to_owned()];

        let report = discover(
            workspace.path(),
            &[],
            &PubNative {
                declared: &declared,
                config: &config,
                config_dir: &workspace.path().join(".dart_tool"),
                options_file: &options,
            },
        );
        assert!(report.plugins.is_empty());
        assert_eq!(report.skipped.len(), 1);
        assert_eq!(report.skipped[0].path, options);
        assert_eq!(
            report.skipped[0].error,
            DiscoveryError::Unresolved("ghost".to_owned())
        );
        assert!(report.skipped[0].error.to_string().contains("pub get"));
    }

    #[test]
    fn pub_native_package_without_entry_script_is_a_structured_issue() {
        let workspace = Workspace::new("pub_hollow");
        let root = workspace.package("hollow");
        let options = workspace.path().join("analysis_options.yaml");

        let report = discover_pub_native(&workspace, &["hollow"], &options);
        assert!(report.plugins.is_empty());
        assert_eq!(report.skipped.len(), 1);
        // The anchor is the missing entrypoint itself.
        assert_eq!(report.skipped[0].path, root.join("lib").join("main.dart"));
        assert!(
            report.skipped[0]
                .error
                .to_string()
                .contains("lib/main.dart")
        );
    }

    #[test]
    fn absent_plugins_declaration_changes_nothing() {
        let workspace = Workspace::new("pub_absent");
        workspace.plugin("demo", &valid_manifest("script.dart"));
        let config = empty_config();

        let with = discover(workspace.path(), &[], &empty_pub_native(&config));
        let without = discover_without_pub_native(workspace.path(), &[]);
        assert_eq!(with, without);
    }

    #[test]
    fn pub_native_duplicates_follow_the_existing_sources() {
        let workspace = Workspace::new("pub_dupes");
        // A convention plugin and a pub package, both naming themselves
        // "demo"; the override points at the pub package.
        workspace.plugin("demo", &valid_manifest("script.dart"));
        let ported = workspace.package("demo");
        std::fs::write(ported.join("script.dart"), "void main {}").expect("write script");
        std::fs::write(
            ported.join(MANIFEST_FILE_NAME),
            valid_manifest("script.dart"),
        )
        .expect("write manifest");
        let options = workspace.path().join("analysis_options.yaml");
        let config = PackageConfig::parse(
            r#"{"configVersion": 2, "packages": [{"name": "demo", "rootUri": "../pkgs/demo", "languageVersion": "3.5"}]}"#,
        )
.expect("valid config");
        let declared = vec!["demo".to_owned()];

        let report = discover(
            workspace.path(),
            std::slice::from_ref(&ported),
            &PubNative {
                declared: &declared,
                config: &config,
                config_dir: &workspace.path().join(".dart_tool"),
                options_file: &options,
            },
        );
        // The override (first source) wins; the pub-native pass then dedups
        // it by canonical path, silently. The convention hit is the name
        // duplicate that gets reported.
        assert_eq!(report.plugins.len(), 1);
        assert_eq!(report.plugins[0].dir, ported);
        assert_eq!(report.skipped.len(), 1);
        assert!(report.skipped[0].error.to_string().contains("duplicate"));
    }
}
