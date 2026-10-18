//! The `dartq.yaml` workspace config and the compat face's
//! axis-root resolution.
//!
//! The config is all-optional: an absent file is [`load`]'s `Ok(None)` and
//! every downstream behavior stays byte-identical to the zero-config
//! default (the single `$DART_SDK` axis, default policies). The typed
//! model carries the fields — named SDK contexts, the generated-file
//! policy, and the memory/cache defaults — parsed here for
//! the faces that consume them; compat consumes the contexts as
//! matrix axes through [`compat_axis_roots`], the shared resolution edge
//! (explicit roots + context labels → one deduplicated root list) both
//! shells call before handing *explicit* roots to the core.
//!
//! Context declaration order is file order (a YAML mapping walked in
//! document order, never sorted), and locator forms are
//! `fvm:<version>`, `path:<dir>`, and `system`. An unsupported form is a
//! structured [`ConfigError::Invalid`], never a silent skip; a
//! resolvable-but-missing root (an uninstalled FVM version, a dangling
//! `path:` dir) is not an error here — it flows to the axis loader and
//! lands in the matrix as the isolated `sdk_unavailable` row (compat's
//! failure-isolation doctrine).

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use thiserror::Error;

/// Why a config could not be honored: a malformed file or locator is the
/// CLI's usage tier (`error: {0}`), an unreadable file the internal tier.
#[derive(Debug, Error)]
pub enum ConfigError {
    /// The config or one of its values is unusable (the message body is
    /// the CLI's usage-tier text).
    #[error("{0}")]
    Invalid(String),
    /// `dartq.yaml` exists but cannot be read.
    #[error("{0}")]
    Io(#[from] io::Error),
}

/// The parsed `dartq.yaml`. Every section is optional and
/// unknown keys are ignored — a reader that survives forward-compatible
/// config, matching the pub-tooling convention.
#[derive(Debug, Clone, Default)]
pub struct DartqConfig {
    /// The declared SDK contexts, in file order.
    pub contexts: Vec<ContextConfig>,
    /// The generated-file policy.
    pub generated: Option<GeneratedPolicy>,
    /// The memory defaults.
    pub memory: Option<MemoryDefaults>,
    /// The cache defaults.
    pub cache: Option<CacheDefaults>,
}

/// One named SDK context.
#[derive(Debug, Clone)]
pub struct ContextConfig {
    /// The label `--context` selects by.
    pub label: String,
    /// The SDK locator (`fvm:<v>` | `path:<dir>` | `system`), unresolved —
    /// resolution happens at the consumer's edge ([`compat_axis_roots`]).
    pub sdk: String,
    /// The context's target platform (default `io` at the consumer).
    pub target: Option<ContextTarget>,
    /// `-D` declared variables feeding the context's conditional env.
    pub define: BTreeMap<String, String>,
}

/// The context's target platform.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ContextTarget {
    /// Native I/O target.
    Io,
    /// The web target.
    Web,
    /// The WASM target.
    Wasm,
}

/// The generated-file policy. Absent keys stay `None` — consumers
/// apply their own defaults; this type invents none.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct GeneratedPolicy {
    /// Whether generated files are analyzed at all.
    pub include: Option<bool>,
    /// Whether fixes may rewrite generated files.
    pub editable: Option<bool>,
    /// The generated-file glob patterns (e.g. `*.g.dart`).
    pub patterns: Option<Vec<String>>,
}

/// The memory defaults.
#[derive(Debug, Clone, Copy, Deserialize)]
pub struct MemoryDefaults {
    /// The memory budget in MiB.
    pub limit_mib: u64,
}

/// The cache defaults.
#[derive(Debug, Clone, Copy, Deserialize)]
pub struct CacheDefaults {
    /// The on-disk cache size limit in GiB.
    pub size_limit_gib: u64,
}

/// Loads `dir/dartq.yaml` — the workspace root's config. (the full
/// lookup order — explicit `--config`, per-pub-project roots — lands with
/// its consumers; the root file is this round's contract.) `Ok(None)` is
/// the absent file: every consumer keeps its zero-config behavior,
/// byte-identical.
///
/// # Errors
///
/// [`ConfigError::Invalid`] on a malformed file; [`ConfigError::Io`] when
/// the file exists but cannot be read.
pub fn load(dir: &Path) -> Result<Option<DartqConfig>, ConfigError> {
    let text = match fs::read_to_string(dir.join("dartq.yaml")) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    parse(&text).map(Some)
}

/// The compat face's axis roots for one invocation: the explicit
/// `--sdk-root` roots in CLI order, then the selected contexts' resolved
/// locators in file order, deduplicated by root. `contexts` non-empty
/// picks those labels (every label must be declared — an unknown one is a
/// structured error); empty picks every declared context. No config, or a
/// config without contexts, yields just the explicit roots — possibly
/// empty, where the caller applies its `$DART_SDK` fallback.
///
/// Relative `path:` locators resolve against `dir` (the workspace root the
/// config was found at), not the process cwd — stable under the daemon.
///
/// # Errors
///
/// [`ConfigError::Invalid`] on a malformed config, an unknown label, or an
/// unresolvable locator; [`ConfigError::Io`] when the config exists but
/// cannot be read.
pub fn compat_axis_roots(
    dir: &Path,
    sdk_roots: &[PathBuf],
    contexts: &[String],
) -> Result<Vec<PathBuf>, ConfigError> {
    let config = load(dir)?;
    let mut axes: Vec<PathBuf> = Vec::with_capacity(sdk_roots.len() + contexts.len());
    for root in sdk_roots {
        push_unique(&mut axes, root.clone());
    }
    let Some(config) = config else {
        if contexts.is_empty() {
            return Ok(axes);
        }
        return Err(ConfigError::Invalid(format!(
            "no dartq.yaml in {} (--context needs a config declaring contexts)",
            dir.join("dartq.yaml").display()
        )));
    };
    if contexts.is_empty() {
        for context in &config.contexts {
            push_unique(
                &mut axes,
                resolve_locator(&context.label, &context.sdk, dir)?,
            );
        }
        return Ok(axes);
    }
    for label in contexts {
        let context = config
            .contexts
            .iter()
            .find(|context| &context.label == label)
            .ok_or_else(|| {
                let declared = config
                    .contexts
                    .iter()
                    .map(|context| context.label.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                ConfigError::Invalid(format!("unknown context `{label}` (declared: {declared})"))
            })?;
        push_unique(
            &mut axes,
            resolve_locator(&context.label, &context.sdk, dir)?,
        );
    }
    Ok(axes)
}

/// Resolves one context's SDK locator to a root directory. A
/// missing FVM version resolves to its canonical cache path so the axis
/// loader — not this function — reports it as an unavailable SDK.
///
/// # Errors
///
/// [`ConfigError::Invalid`] for an unsupported locator form, an empty
/// version/dir, `system` without `$DART_SDK`, or no HOME for `fvm:`.
pub fn resolve_locator(
    label: &str,
    locator: &str,
    config_dir: &Path,
) -> Result<PathBuf, ConfigError> {
    if locator == "system" {
        return match std::env::var_os("DART_SDK") {
            Some(root) if !root.is_empty() => Ok(PathBuf::from(root)),
            _ => Err(ConfigError::Invalid(format!(
                "context `{label}`: the `system` locator needs DART_SDK set"
            ))),
        };
    }
    if let Some(version) = locator.strip_prefix("fvm:") {
        if version.is_empty() {
            return Err(ConfigError::Invalid(format!(
                "context `{label}`: empty fvm version"
            )));
        }
        return fvm_version_root(version);
    }
    if let Some(dir) = locator.strip_prefix("path:") {
        if dir.is_empty() {
            return Err(ConfigError::Invalid(format!(
                "context `{label}`: empty path locator"
            )));
        }
        let root = Path::new(dir);
        return Ok(if root.is_absolute() {
            root.to_path_buf()
        } else {
            config_dir.join(root)
        });
    }
    Err(ConfigError::Invalid(format!(
        "context `{label}`: unsupported sdk locator `{locator}` \
         (expected fvm:<version> | path:<dir> | system)"
    )))
}

/// The FVM version's cache root: `$HOME/.fvm/versions/<version>` (FVM 3's
/// layout), or the pre-3.0 `$HOME/fvm/versions/<version>` when that is the
/// one on disk. No installed match → the modern path, which the axis
/// loader reports as `sdk_unavailable` (failure isolation).
///
/// # Errors
///
/// [`ConfigError::Invalid`] when the home directory cannot be determined.
fn fvm_version_root(version: &str) -> Result<PathBuf, ConfigError> {
    let Some(home) = std::env::home_dir() else {
        return Err(ConfigError::Invalid(format!(
            "cannot resolve fvm version `{version}`: no home directory"
        )));
    };
    let modern = home.join(".fvm").join("versions").join(version);
    let legacy = home.join("fvm").join("versions").join(version);
    Ok(if legacy.is_dir() && !modern.is_dir() {
        legacy
    } else {
        modern
    })
}

/// Dedup by exact root, first occurrence wins (axis order stays the
/// caller's: flags in CLI order, then contexts in file order).
fn push_unique(axes: &mut Vec<PathBuf>, root: PathBuf) {
    if !axes.contains(&root) {
        axes.push(root);
    }
}

fn parse(text: &str) -> Result<DartqConfig, ConfigError> {
    let value: serde_yaml::Value = serde_yaml::from_str(text)
        .map_err(|error| ConfigError::Invalid(format!("invalid dartq.yaml: {error}")))?;
    let raw: RawConfig = serde_yaml::from_value(value.clone())
        .map_err(|error| ConfigError::Invalid(format!("invalid dartq.yaml: {error}")))?;
    if raw.version.is_some_and(|version| version != 1) {
        return Err(ConfigError::Invalid(format!(
            "unsupported dartq.yaml version: {} (expected 1)",
            raw.version.expect("checked above")
        )));
    }
    let mut contexts = Vec::new();
    match value.get("contexts") {
        None | Some(serde_yaml::Value::Null) => {}
        Some(declared) => {
            let declared = declared.as_mapping().ok_or_else(|| {
                ConfigError::Invalid(
                    "dartq.yaml: `contexts` must be a label → context mapping".into(),
                )
            })?;
            for (key, entry) in declared {
                let Some(label) = key.as_str() else {
                    return Err(ConfigError::Invalid(
                        "dartq.yaml: `contexts` keys must be strings".into(),
                    ));
                };
                let context: RawContext =
                    serde_yaml::from_value(entry.clone()).map_err(|error| {
                        ConfigError::Invalid(format!("dartq.yaml: context `{label}`: {error}"))
                    })?;
                contexts.push(ContextConfig {
                    label: label.to_owned(),
                    sdk: context.sdk,
                    target: context.target,
                    define: context.define,
                });
            }
        }
    }
    Ok(DartqConfig {
        contexts,
        generated: raw.generated,
        memory: raw.memory,
        cache: raw.cache,
    })
}

/// The typed sections minus `contexts` (walked separately — a YAML mapping
/// in document order, which typed map fields would reorder or shuffle).
#[derive(Deserialize)]
struct RawConfig {
    version: Option<u32>,
    generated: Option<GeneratedPolicy>,
    memory: Option<MemoryDefaults>,
    cache: Option<CacheDefaults>,
}

#[derive(Deserialize)]
struct RawContext {
    sdk: String,
    target: Option<ContextTarget>,
    #[serde(default)]
    define: BTreeMap<String, String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The compat face's fixture shape (the CLI compat tests' twin): a real
    /// package directory with two stub SDKs. Per-axis differentiation rides
    /// the experiment whitelist — `new` grants package `fixture` the
    /// `variance` experiment in `allowed_experiments.json`, `old` does not,
    /// so `a.dart`'s `<in T>` modifier is clean under `new` and an
    /// `experiment_not_enabled_off_by_default` ERROR under `old`. (The
    /// `dart:` library table is the generated oracle-pinned flat face —
    /// axis-independent — so a `dart:` import can no longer differentiate
    /// stub SDKs; `variance` has no `experimentalReleaseVersion`, so the
    /// grant applies only at the current language version, pinned here to
    /// 3.13 by `package_config.json`.)
    fn fixture(tag: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("dq_cmd_config_{tag}_{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let write = |relative: &str, content: &str| {
            let path = root.join(relative);
            fs::create_dir_all(path.parent().expect("parent")).expect("dir");
            fs::write(path, content).expect("file");
        };
        write(
            "pubspec.yaml",
            "name: fixture\nversion: 0.1.0\nenvironment:\n  sdk: ^3.5.0\n",
        );
        write(
            "pubspec.lock",
            "packages: {}\nsdks:\n  dart: \">=3.5.0 <4.0.0\"\n",
        );
        write(
            ".dart_tool/package_config.json",
            r#"{"configVersion": 2, "packages": [{"name": "fixture", "rootUri": "../", "languageVersion": "3.13"}], "generator": "pub"}"#,
        );
        write("sdks/new/version", "3.13.1\n");
        write(
            "sdks/new/lib/libraries.json",
            r#"{"vm": {"libraries": {"core": {"uri": "core/core.dart"}, "async": {"uri": "async/async.dart"}}}}"#,
        );
        write("sdks/new/lib/async/async.dart", "library dart.async;\n");
        write("sdks/old/version", "3.5.0\n");
        write(
            "sdks/old/lib/libraries.json",
            r#"{"vm": {"libraries": {"core": {"uri": "core/core.dart"}}}}"#,
        );
        write(
            "sdks/new/lib/_internal/allowed_experiments.json",
            r#"{"version": 1, "experimentSets": {"grants": ["variance"]}, "sdk": {"default": {"experimentSet": "grants"}}, "packages": {"fixture": {"experimentSet": "grants"}}}"#,
        );
        write(
            "sdks/old/lib/_internal/allowed_experiments.json",
            r#"{"version": 1, "experimentSets": {"sdkExperiments": []}, "sdk": {"default": {"experimentSet": "sdkExperiments"}}, "packages": {}}"#,
        );
        write("lib/a.dart", "class Foo<in T> {}\n");
        root
    }

    fn write_config(root: &Path, yaml: &str) {
        fs::write(root.join("dartq.yaml"), yaml).expect("config");
    }

    /// The config's two contexts are the whole axis set — no explicit
    /// roots — in file order, and the core compat face turns them into the
    /// 2-axis matrix with differentiated verdicts.
    #[test]
    fn config_contexts_drive_the_two_axis_matrix() {
        let root = fixture("matrix");
        // `new` before `old`: file order, which is not the sorted order.
        write_config(
            &root,
            "version: 1\ncontexts:\n  new:\n    sdk: path:sdks/new\n  old:\n    sdk: path:sdks/old\n",
        );
        let axes = compat_axis_roots(&root, &[], &[]).expect("axes");
        assert_eq!(axes, vec![root.join("sdks/new"), root.join("sdks/old")]);

        let (report, _) =
            crate::compat::compat(&root, &root.join("lib/a.dart"), &axes).expect("run");
        assert_eq!(report.matrix.len(), 2);
        assert_eq!(report.matrix[0].sdk, "3.13.1");
        assert!(report.matrix[0].compatible);
        assert_eq!(report.matrix[1].sdk, "3.5.0");
        assert!(!report.matrix[1].compatible);
        fs::remove_dir_all(&root).ok();
    }

    /// The hard requirement: an absent file loads as `None` with an empty
    /// axis selection, and a defaults-only config changes no compat byte.
    #[test]
    fn absent_or_defaults_only_config_stays_byte_identical() {
        let root = fixture("absent");
        assert!(load(&root).expect("absent is none").is_none());
        assert!(
            compat_axis_roots(&root, &[], &[])
                .expect("no axes")
                .is_empty()
        );

        let target = root.join("lib/a.dart");
        let roots = vec![root.join("sdks/old"), root.join("sdks/new")];
        let without = crate::compat::compat(&root, &target, &roots)
            .expect("run")
            .0;
        let without = serde_json::to_string(&without).expect("json");

        write_config(
            &root,
            "version: 1\ngenerated:\n  include: false\n  patterns: [\"*.g.dart\"]\nmemory:\n  limit_mib: 2048\ncache:\n  size_limit_gib: 10\n",
        );
        let with = crate::compat::compat(&root, &target, &roots)
            .expect("run")
            .0;
        assert_eq!(
            serde_json::to_string(&with).expect("json"),
            without,
            "defaults-only config: byte-identical report"
        );
        // The defaults parsed through, and the axis selection stays empty.
        let config = load(&root).expect("config").expect("some");
        assert!(config.contexts.is_empty());
        assert_eq!(config.memory.expect("memory").limit_mib, 2048);
        assert_eq!(config.cache.expect("cache").size_limit_gib, 10);
        assert_eq!(
            config.generated.expect("generated").patterns,
            Some(vec!["*.g.dart".to_owned()])
        );
        assert!(
            compat_axis_roots(&root, &[], &[])
                .expect("no axes")
                .is_empty()
        );
        fs::remove_dir_all(&root).ok();
    }

    /// Context labels select a subset (an unknown one is a structured
    /// error); explicit roots and contexts combine in that order with
    /// duplicate roots collapsed.
    #[test]
    fn context_selection_combines_and_dedups() {
        let root = fixture("select");
        write_config(
            &root,
            "contexts:\n  new:\n    sdk: path:sdks/new\n  old:\n    sdk: path:sdks/old\n",
        );
        let axes =
            compat_axis_roots(&root, &[root.join("sdks/new")], &["old".to_owned()]).expect("axes");
        assert_eq!(axes, vec![root.join("sdks/new"), root.join("sdks/old")]);

        let axes = compat_axis_roots(
            &root,
            &[root.join("sdks/new")],
            &["new".to_owned(), "old".to_owned()],
        )
        .expect("axes");
        assert_eq!(
            axes,
            vec![root.join("sdks/new"), root.join("sdks/old")],
            "the explicit root is not duplicated by the `new` context"
        );

        let error = compat_axis_roots(&root, &[], &["nope".to_owned()]).expect_err("unknown");
        assert!(
            error.to_string().contains("unknown context `nope`"),
            "structured: {error}"
        );
        fs::remove_dir_all(&root).ok();
    }

    /// Unsupported locator forms, bad versions, and `--context` without a
    /// config are structured usage-tier errors, never silent skips.
    #[test]
    fn bad_config_shapes_are_structured_errors() {
        let root = fixture("errors");
        for (yaml, needle) in [
            (
                "contexts:\n  bad:\n    sdk: registry:foo\n",
                "unsupported sdk locator `registry:foo`",
            ),
            (
                "contexts:\n  bad:\n    sdk: \"fvm:\"\n",
                "empty fvm version",
            ),
            (
                "contexts: [a, b]\n",
                "`contexts` must be a label → context mapping",
            ),
            ("version: 2\n", "unsupported dartq.yaml version: 2"),
        ] {
            write_config(&root, yaml);
            let error = compat_axis_roots(&root, &[], &[]).expect_err(yaml);
            assert!(
                matches!(error, ConfigError::Invalid(_)) && error.to_string().contains(needle),
                "`{yaml}` → {error}"
            );
        }

        // `--context` with no config file at all.
        let bare = fixture("noconfig");
        let error = compat_axis_roots(&bare, &[], &["x".to_owned()]).expect_err("no config");
        assert!(error.to_string().contains("no dartq.yaml"), "{error}");
        // A malformed file is the same structured tier.
        write_config(&bare, "contexts: {a: [unclosed\n");
        assert!(matches!(load(&bare), Err(ConfigError::Invalid(_))));
        fs::remove_dir_all(&root).ok();
        fs::remove_dir_all(&bare).ok();
    }
}
