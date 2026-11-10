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
    /// The analyze face's engine choice and official-engine knobs.
    pub analyzer: Option<AnalyzerConfig>,
    /// The package-discovery policy (the `packages` face).
    pub packages: Option<PackagesConfig>,
    /// The Flutter build cache's configuration (on by default).
    pub build_cache: Option<BuildCacheConfig>,
}

impl DartqConfig {
    /// How many heavy jobs may run at once machine-wide, across every
    /// worktree and kind: `memory.max_concurrent`, else `0`, unlimited.
    /// Validated to an integer ≥ 0 at parse. The `DARTQ_MAX_CONCURRENT`
    /// override is the gate's (`dartq_toolchain::ledger::cap`).
    #[must_use]
    pub fn memory_max_concurrent(&self) -> u32 {
        self.memory
            .and_then(|memory| memory.max_concurrent)
            .and_then(|cap| u32::try_from(cap).ok())
            .unwrap_or(0)
    }

    /// The machine-wide memory share every heavy dartq face together may
    /// hold, in percent: `memory.max_percent`, 90 when unset (validated to
    /// 1–100 at parse).
    #[must_use]
    pub fn memory_max_percent(&self) -> u8 {
        self.memory
            .and_then(|memory| memory.max_percent)
            .and_then(|percent| u8::try_from(percent).ok())
            .unwrap_or(DEFAULT_MEMORY_MAX_PERCENT)
    }

    /// The `analyzer.official` knobs, defaults filled in.
    #[must_use]
    pub fn official(&self) -> OfficialSettings {
        let official = self
            .analyzer
            .and_then(|analyzer| analyzer.official)
            .unwrap_or_default();
        OfficialSettings {
            memory_budget_bytes: official
                .memory_budget_gb
                .map_or(0, |gb| gb.saturating_mul(1 << 30)),
            resident: official.resident.unwrap_or(true),
            idle_minutes: official.idle_minutes.unwrap_or(15),
        }
    }

    /// The `analyzer.engine` choice, when the config makes one.
    #[must_use]
    pub fn engine(&self) -> Option<Engine> {
        self.analyzer.and_then(|analyzer| analyzer.engine)
    }

    /// The shared-analyzer pool posture: `enabled` opts the worktree in
    /// (default off), `max_roots` the most worktrees one pooled server
    /// serves (default 3, clamped 1–64 at read).
    #[must_use]
    pub fn pool(&self) -> (bool, u32) {
        let pool = self.analyzer.and_then(|analyzer| analyzer.pool);
        let enabled = pool.and_then(|pool| pool.enabled).unwrap_or(false);
        let max_roots = pool
            .and_then(|pool| pool.max_roots)
            .and_then(|cap| u32::try_from(cap).ok())
            .unwrap_or(3)
            .clamp(1, 64);
        (enabled, max_roots)
    }
}

/// Which analyzer `dartq analyze` runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Engine {
    /// The official Dart analyzer (the default).
    Official,
    /// The in-process dartq engine.
    Dartq,
}

/// The `analyzer:` section. Unknown keys inside it are a structured
/// [`ConfigError::Invalid`] (a typo must not silently fall back to a
/// default).
#[derive(Debug, Clone, Copy, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AnalyzerConfig {
    /// `official` or `dartq`; a `--engine` flag overrides it.
    pub engine: Option<Engine>,
    /// The official engine's knobs.
    pub official: Option<OfficialConfig>,
    /// The shared-analyzer pool's knobs.
    pub pool: Option<PoolConfig>,
}

/// The `analyzer.pool:` section — the opt-in shared analyzer: one
/// resident analysis server serves several worktrees instead of one
/// daemon per worktree.
#[derive(Debug, Clone, Copy, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PoolConfig {
    /// Opt the worktree's `dartq analyze` into the shared pool (default
    /// false — a worktree runs its own resident daemon).
    pub enabled: Option<bool>,
    /// The most worktrees one pooled server serves (default 3, 1–64).
    pub max_roots: Option<u64>,
}

/// The `analyzer.official:` section.
#[derive(Debug, Clone, Copy, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OfficialConfig {
    /// The RSS budget of the official analyzer's process tree, in GiB.
    /// Unset or `0` is unlimited, as `dart analyze` itself is: a large
    /// workspace legitimately needs tens of GiB, and a kill near the end
    /// of a long analysis turns a slow success into a failure.
    pub memory_budget_gb: Option<u64>,
    /// Keep an analysis server resident between runs (default true).
    pub resident: Option<bool>,
    /// Minutes an unused resident server stays up (default 15).
    pub idle_minutes: Option<u64>,
}

/// [`OfficialConfig`] with its defaults filled in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OfficialSettings {
    /// `0` is unlimited.
    pub memory_budget_bytes: u64,
    pub resident: bool,
    pub idle_minutes: u64,
}

/// The `build_cache:` section — the Flutter build cache every `dartq
/// flutter` face runs (the explicit subcommand and the opt-in PATH
/// shim). Default-on: an absent (or all-commented, i.e. null) section,
/// like an absent key, runs the cache with the section's defaults —
/// `enabled: false` is the one spelling that turns it off.
///
/// Unlike the top level (unknown keys ignored for forward
/// compatibility), unknown keys inside this section are a structured
/// [`ConfigError::Invalid`]: the section gates shell-visible behavior,
/// so a typo must fail loud rather than silently disable the cache.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BuildCacheConfig {
    /// Whether the cache runs at all. `None` (absent) is on; `false`
    /// passes every flutter face through with no store access.
    pub enabled: Option<bool>,
    /// The CAS store root; `~` expands at consumption, and the store's
    /// directories sit directly under it (no version segment). Default
    /// `$XDG_CACHE_HOME/dartq/cas`, falling back to
    /// `~/.cache/dartq/cas`.
    pub root: Option<PathBuf>,
    /// The store size budget in GiB. Default 96.
    pub budget_gb: Option<u64>,
    /// Seconds after a hit during which an entry is unevictable.
    /// Default 3600.
    pub grace_secs: Option<u64>,
    /// An explicit flutter binary (default: `$DARTQ_FLUTTER`, then a
    /// PATH search that skips the shim's own directory).
    pub flutter: Option<PathBuf>,
}

/// The `packages:` section — how `dartq packages` (and every
/// multi-package face) sees the tree. Globs are `package:glob` relative
/// POSIX paths against the config root; `exclude` prunes fixture trees
/// and vendored noise from the walk (a pruned tree that still contains a
/// pubspec is registered as a boundary, never silently dropped);
/// `default` names the package set bare invocations operate on when the
/// tree holds more than one work point.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct PackagesConfig {
    /// Only matching package directories are listed.
    #[serde(default)]
    pub include: Vec<String>,
    /// Matching trees are pruned from the walk.
    #[serde(default)]
    pub exclude: Vec<String>,
    /// The default package set (relative directories) for bare
    /// multi-package invocations.
    #[serde(default)]
    pub default: Vec<String>,
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
#[derive(Debug, Clone, Copy, Default, Deserialize)]
pub struct MemoryDefaults {
    /// The memory budget in MiB.
    pub limit_mib: Option<u64>,
    /// The share of the machine's memory, in percent (1–100), every heavy
    /// dartq face together may hold: resident analyzers, analyze and test
    /// runs, session daemons, Flutter builds and debug apps (resolved by
    /// [`DartqConfig::memory_max_percent`]). Any other value is a
    /// [`ConfigError::Invalid`].
    pub max_percent: Option<i64>,
    /// How many heavy jobs run at once machine-wide, across every worktree
    /// and kind (resolved by [`DartqConfig::memory_max_concurrent`]): `0`
    /// or unset is unlimited; anything but an integer ≥ 0 is a
    /// [`ConfigError::Invalid`].
    pub max_concurrent: Option<i64>,
}

/// The `memory.max_percent` default.
pub const DEFAULT_MEMORY_MAX_PERCENT: u8 = 90;

/// The cache defaults.
#[derive(Debug, Clone, Copy, Deserialize)]
pub struct CacheDefaults {
    /// The on-disk cache size limit in GiB.
    pub size_limit_gib: u64,
}

/// The config file's name, the same in both tiers: the project tier's up
/// the directory chain ([`project_config_file`]), the global tier's under
/// `<home>/.dartq` ([`global_config_file`]).
pub const FILE_NAME: &str = "dartq.yaml";

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
    load_file(&dir.join(FILE_NAME))
}

/// Reads and parses one config file; `Ok(None)` when it is absent.
fn load_file(path: &Path) -> Result<Option<DartqConfig>, ConfigError> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    parse(&text).map(Some)
}

/// The project tier's file governing `dir`: the nearest `dartq.yaml` up
/// the chain from `dir` to the filesystem root, `None` when no directory on
/// the chain holds one. A candidate counts unless it is provably absent
/// (`fs::exists` answering `Ok(false)`): a file whose existence cannot be
/// checked still governs, so its loader surfaces the error instead of
/// silently falling through to a farther file. The one walk every
/// project-tier consumer shares — the loader below and the daemon's watch
/// on the governing file — so the two cannot drift.
#[must_use]
pub fn project_config_file(dir: &Path) -> Option<PathBuf> {
    dir.ancestors()
        .map(|dir| dir.join(FILE_NAME))
        .find(|candidate| !matches!(fs::exists(candidate), Ok(false)))
}

/// The global tier's file: `<home>/.dartq/dartq.yaml`.
#[must_use]
pub fn global_config_file(home: &Path) -> PathBuf {
    home.join(".dartq").join(FILE_NAME)
}

/// Finds the `dartq.yaml` governing `dir`: the nearest file up the chain
/// from `dir` to the filesystem root, overlaid on the user-level global
/// config (`~/.dartq/dartq.yaml`) key by key — the project file wins every
/// key it declares, the global fills the rest, built-in defaults the
/// remainder. `Ok(None)`: neither exists. The faces that key off workspace
/// config (the cache maintenance faces, the build cache) resolve every
/// section from this one governing result, so one walk answers them all.
/// The global file is read-only to every face: nothing a project run does
/// ever writes it, so one tree's config cannot leak into another's or into
/// the machine's.
///
/// # Errors
///
/// [`ConfigError::Invalid`] on a malformed file (either tier);
/// [`ConfigError::Io`] when a file exists but cannot be read.
pub fn discover(dir: &Path) -> Result<Option<DartqConfig>, ConfigError> {
    discover_with(dir, std::env::home_dir().as_deref())
}

/// [`discover`] against an explicit home directory, so the global tier is
/// testable without touching the user's real `~/.dartq`.
pub(crate) fn discover_with(
    dir: &Path,
    home: Option<&Path>,
) -> Result<Option<DartqConfig>, ConfigError> {
    let project = discover_project(dir)?;
    let global = load_global_at(home)?;
    Ok(overlay(global, project))
}

/// The project tier on its own: the file [`project_config_file`] finds
/// from `dir`, loaded — `Ok(None)` when the walk finds none, or when the
/// file it found vanishes before the read. Faces that must resolve paths
/// against the file's own directory (context `path:` locators) still want
/// this root.
///
/// # Errors
///
/// [`ConfigError::Invalid`] on a malformed file; [`ConfigError::Io`] when
/// a file exists but cannot be read.
fn discover_project(dir: &Path) -> Result<Option<DartqConfig>, ConfigError> {
    project_config_file(dir).map_or(Ok(None), |path| load_file(&path))
}

/// Loads the global tier, [`global_config_file`] — machine-level defaults
/// (store budgets, memory caps) a project file overrides key by key.
/// `Ok(None)` when the file or the home directory does not exist.
///
/// # Errors
///
/// [`ConfigError::Invalid`] on a malformed file; [`ConfigError::Io`] when
/// the file exists but cannot be read.
fn load_global_at(home: Option<&Path>) -> Result<Option<DartqConfig>, ConfigError> {
    home.map_or(Ok(None), |home| load_file(&global_config_file(home)))
}

/// Two tiers folded into one governing config: every key the project tier
/// declares wins; the global tier fills the keys it leaves unset. Scalar
/// sections (`cache`, `analyzer`) and list-shaped ones (`contexts`,
/// `packages`) are whole-section — the project tier replaces the global's
/// section when present. `build_cache`, `generated` and `memory` merge key by key
/// inside the section, so a machine-wide `budget_gb` survives a project
/// that only pins `root`.
fn overlay(global: Option<DartqConfig>, project: Option<DartqConfig>) -> Option<DartqConfig> {
    let (mut governing, project) = match (global, project) {
        (Some(global), Some(project)) => (global, project),
        (global, project) => return global.or(project),
    };
    if !project.contexts.is_empty() {
        governing.contexts = project.contexts;
    }
    governing.generated = match (governing.generated, project.generated) {
        (None, project) => project,
        (global, None) => global,
        (Some(mut global), Some(project)) => {
            if project.include.is_some() {
                global.include = project.include;
            }
            if project.editable.is_some() {
                global.editable = project.editable;
            }
            if project.patterns.is_some() {
                global.patterns = project.patterns;
            }
            Some(global)
        }
    };
    governing.memory = match (governing.memory, project.memory) {
        (None, project) => project,
        (global, None) => global,
        (Some(mut global), Some(project)) => {
            if project.limit_mib.is_some() {
                global.limit_mib = project.limit_mib;
            }
            if project.max_percent.is_some() {
                global.max_percent = project.max_percent;
            }
            if project.max_concurrent.is_some() {
                global.max_concurrent = project.max_concurrent;
            }
            Some(global)
        }
    };
    if project.cache.is_some() {
        governing.cache = project.cache;
    }
    if project.analyzer.is_some() {
        governing.analyzer = project.analyzer;
    }
    if project.packages.is_some() {
        governing.packages = project.packages;
    }
    governing.build_cache = match (governing.build_cache, project.build_cache) {
        (None, project) => project,
        (global, None) => global,
        (Some(mut global), Some(project)) => {
            if project.enabled.is_some() {
                global.enabled = project.enabled;
            }
            if project.root.is_some() {
                global.root = project.root;
            }
            if project.budget_gb.is_some() {
                global.budget_gb = project.budget_gb;
            }
            if project.grace_secs.is_some() {
                global.grace_secs = project.grace_secs;
            }
            if project.flutter.is_some() {
                global.flutter = project.flutter;
            }
            Some(global)
        }
    };
    Some(governing)
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
    if let Some(percent) = value
        .get("memory")
        .and_then(|memory| memory.get("max_percent"))
        .filter(|percent| !percent.is_null())
        && !percent
            .as_i64()
            .is_some_and(|percent| (1..=100).contains(&percent))
    {
        let shown = serde_yaml::to_string(percent).unwrap_or_default();
        return Err(ConfigError::Invalid(format!(
            "dartq.yaml: `memory.max_percent` must be an integer from 1 to 100, got {}",
            shown.trim()
        )));
    }
    if let Some(cap) = value
        .get("memory")
        .and_then(|memory| memory.get("max_concurrent"))
        .filter(|cap| !cap.is_null())
        && !cap.as_i64().is_some_and(|cap| cap >= 0)
    {
        let shown = serde_yaml::to_string(cap).unwrap_or_default();
        return Err(ConfigError::Invalid(format!(
            "dartq.yaml: `memory.max_concurrent` must be an integer of 0 or more (0 is unlimited), got {}",
            shown.trim()
        )));
    }
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
        analyzer: raw.analyzer,
        packages: raw.packages,
        build_cache: raw.build_cache,
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
    analyzer: Option<AnalyzerConfig>,
    packages: Option<PackagesConfig>,
    build_cache: Option<BuildCacheConfig>,
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

    /// A home directory whose `~/.dartq/dartq.yaml` holds `yaml` (`None`
    /// leaves the global tier absent), plus a project root.
    fn tiered(global_yaml: Option<&str>) -> (tempfile::TempDir, tempfile::TempDir) {
        let home = tempfile::tempdir().expect("home");
        if let Some(yaml) = global_yaml {
            let dir = home.path().join(".dartq");
            fs::create_dir_all(&dir).expect("dir");
            fs::write(dir.join("dartq.yaml"), yaml).expect("global");
        }
        let project = tempfile::tempdir().expect("project");
        (home, project)
    }

    #[test]
    fn global_fills_keys_the_project_leaves_unset() {
        let (home, project) = tiered(Some(
            "version: 1\nbuild_cache:\n  budget_gb: 8\nmemory:\n  limit_mib: 2048\n",
        ));
        write_config(project.path(), "version: 1\nbuild_cache:\n  root: /store\n");
        let config = discover_with(project.path(), Some(home.path()))
            .expect("discover")
            .expect("config");
        let build = config.build_cache.expect("build_cache");
        assert_eq!(build.root, Some(PathBuf::from("/store")));
        assert_eq!(build.budget_gb, Some(8));
        assert_eq!(
            config.memory.expect("memory").limit_mib.expect("limit_mib"),
            2048,
            "a project section the project omits falls through to the global"
        );
    }

    #[test]
    fn project_overrides_the_global_key_by_key() {
        let (home, project) = tiered(Some(
            "version: 1\nbuild_cache:\n  budget_gb: 8\n  grace_secs: 60\n",
        ));
        write_config(
            project.path(),
            "version: 1\nbuild_cache:\n  budget_gb: 96\n",
        );
        let build = discover_with(project.path(), Some(home.path()))
            .expect("discover")
            .expect("config")
            .build_cache
            .expect("build_cache");
        assert_eq!(build.budget_gb, Some(96));
        assert_eq!(
            build.grace_secs,
            Some(60),
            "a key the project omits keeps the global value"
        );
    }

    #[test]
    fn global_alone_governs_and_nothing_yields_none() {
        let (home, project) = tiered(Some("version: 1\ncache:\n  size_limit_gib: 4\n"));
        let config = discover_with(project.path(), Some(home.path()))
            .expect("discover")
            .expect("config");
        assert_eq!(config.cache.expect("cache").size_limit_gib, 4);
        let (home, project) = tiered(None);
        assert!(
            discover_with(project.path(), Some(home.path()))
                .expect("discover")
                .is_none()
        );
        assert!(
            discover_with(project.path(), None)
                .expect("discover")
                .is_none(),
            "no home directory at all still resolves a project-only config world"
        );
    }

    #[test]
    fn malformed_global_is_loud() {
        let (home, project) = tiered(Some("version: 1\nbuild_cache:\n  budget_gb: [oops\n"));
        assert!(discover_with(project.path(), Some(home.path())).is_err());
    }

    /// The project tier's walk ([`project_config_file`], shared with the
    /// daemon's watch): the nearest file up the chain governs and a nearer
    /// one shadows a farther one — and a candidate that exists but cannot
    /// be read still governs, so the loader reports it instead of falling
    /// through to the farther file.
    #[test]
    fn project_config_file_takes_the_nearest_up_the_chain() {
        let project = tempfile::tempdir().expect("project");
        let root = project.path();
        let package = root.join("pkg");
        let nested = package.join("lib");
        fs::create_dir_all(&nested).expect("dirs");
        write_config(root, "version: 1\n");
        assert_eq!(
            project_config_file(&nested),
            Some(root.join(FILE_NAME)),
            "the nearest file up the chain governs"
        );
        write_config(&package, "version: 1\n");
        assert_eq!(
            project_config_file(&nested),
            Some(package.join(FILE_NAME)),
            "a nearer file shadows a farther one"
        );
        // A directory in the nearer file's place exists but cannot be read
        // as a config: it still governs, and the loader says so.
        fs::remove_file(package.join(FILE_NAME)).expect("remove the nearer file");
        fs::create_dir(package.join(FILE_NAME)).expect("a directory in its place");
        assert_eq!(project_config_file(&nested), Some(package.join(FILE_NAME)));
        assert!(
            matches!(discover_project(&nested), Err(ConfigError::Io(_))),
            "an unreadable governing file is loud, never skipped"
        );
    }

    #[test]
    fn contexts_and_packages_replace_the_whole_section() {
        let (home, project) = tiered(Some(
            "version: 1\ncontexts:\n  ci:\n    sdk: path:sdks/ci\npackages:\n  include:\n    - packages/*\n",
        ));
        write_config(
            project.path(),
            "version: 1\ncontexts:\n  local:\n    sdk: system\npackages:\n  exclude:\n    - fixtures/*\n",
        );
        let config = discover_with(project.path(), Some(home.path()))
            .expect("discover")
            .expect("config");
        let labels: Vec<_> = config.contexts.iter().map(|c| c.label.as_str()).collect();
        assert_eq!(
            labels,
            ["local"],
            "the project's contexts replace the global's"
        );
        let packages = config.packages.expect("packages");
        assert!(
            packages.include.is_empty(),
            "section replacement, not a merge"
        );
        assert_eq!(packages.exclude, ["fixtures/*"]);
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
        assert_eq!(config.memory.expect("memory").limit_mib, Some(2048));
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

    /// The `build_cache` section parses through with its typed fields.
    #[test]
    fn build_cache_section_parses_with_typed_fields() {
        let root = fixture("bcache");
        write_config(
            &root,
            "build_cache:\n  enabled: false\n  root: ~/cas-root\n  budget_gb: 5\n  grace_secs: 60\n  flutter: /opt/flutter/bin/flutter\n",
        );
        let config = load(&root).expect("config").expect("some");
        let section = config.build_cache.expect("build_cache");
        assert_eq!(section.enabled, Some(false));
        assert_eq!(section.root, Some(PathBuf::from("~/cas-root")));
        assert_eq!(section.budget_gb, Some(5));
        assert_eq!(section.grace_secs, Some(60));
        assert_eq!(
            section.flutter,
            Some(PathBuf::from("/opt/flutter/bin/flutter"))
        );
        fs::remove_dir_all(&root).ok();
    }

    /// Default-on at the parse tier: an absent section and a section
    /// without `enabled` both leave the key unset — the consumer treats
    /// that as on, so only an explicit `false` opts out.
    #[test]
    fn build_cache_enabled_key_defaults_to_unset() {
        let root = fixture("benabled");
        write_config(&root, "build_cache:\n  budget_gb: 5\n");
        let section = load(&root)
            .expect("config")
            .expect("some")
            .build_cache
            .expect("build_cache");
        assert_eq!(section.enabled, None, "absent key is on-by-default");
        write_config(&root, "build_cache:\n  enabled: true\n");
        let section = load(&root)
            .expect("config")
            .expect("some")
            .build_cache
            .expect("build_cache");
        assert_eq!(section.enabled, Some(true));
        fs::remove_dir_all(&root).ok();
    }

    /// `enabled` merges like every other `build_cache` key: a global
    /// opt-out survives a project section that pins only its store, and
    /// a project opt-out wins over a global that leaves the key unset.
    #[test]
    fn build_cache_enabled_merges_key_by_key() {
        let (home, project) = tiered(Some(
            "version: 1\nbuild_cache:\n  enabled: false\n  budget_gb: 8\n",
        ));
        write_config(project.path(), "version: 1\nbuild_cache:\n  root: /store\n");
        let build = discover_with(project.path(), Some(home.path()))
            .expect("discover")
            .expect("config")
            .build_cache
            .expect("build_cache");
        assert_eq!(build.enabled, Some(false));
        assert_eq!(build.root, Some(PathBuf::from("/store")));

        let (home, project) = tiered(Some("version: 1\nbuild_cache:\n  root: /global\n"));
        write_config(
            project.path(),
            "version: 1\nbuild_cache:\n  enabled: false\n",
        );
        let build = discover_with(project.path(), Some(home.path()))
            .expect("discover")
            .expect("config")
            .build_cache
            .expect("build_cache");
        assert_eq!(build.enabled, Some(false));
        assert_eq!(build.root, Some(PathBuf::from("/global")));
    }

    /// Absent file, absent section, and an all-commented (null) section
    /// all read as `None` — which the consumer resolves to the
    /// default-on store, not to a passthrough.
    #[test]
    fn build_cache_absent_or_null_section_is_none() {
        let root = fixture("babsent");
        assert!(load(&root).expect("absent file").is_none());
        write_config(&root, "version: 1\ncache:\n  size_limit_gib: 10\n");
        let config = load(&root).expect("config").expect("some");
        assert!(config.build_cache.is_none());
        write_config(&root, "build_cache:\n  # root: ~/.cache/dartq/cas\n");
        let config = load(&root).expect("config").expect("some");
        assert!(
            config.build_cache.is_none(),
            "a null (all-commented) section is absent"
        );
        fs::remove_dir_all(&root).ok();
    }

    /// An unknown key inside the section is a structured error, never a
    /// silent skip.
    #[test]
    fn build_cache_unknown_key_is_invalid() {
        let root = fixture("binvalid");
        write_config(&root, "build_cache:\n  budget_gib: 5\n");
        let error = load(&root).expect_err("unknown key must be Invalid");
        assert!(
            matches!(error, ConfigError::Invalid(ref message) if message.contains("budget_gib")),
            "structured: {error}"
        );
        fs::remove_dir_all(&root).ok();
    }

    /// `memory.max_concurrent` resolves key by key through both tiers, is
    /// 0 (unlimited) when unset anywhere, and anything but an integer of 0
    /// or more is a structured error. `test.max_concurrent` has no effect.
    #[test]
    fn memory_max_concurrent_resolves_tiers_and_validates() {
        let resolve = |project: &Path, home: &Path| {
            discover_with(project, Some(home))
                .expect("discover")
                .expect("config")
                .memory_max_concurrent()
        };
        let (home, project) = tiered(Some("version: 1\nmemory:\n  max_concurrent: 2\n"));
        write_config(project.path(), "version: 1\nmemory:\n  max_percent: 80\n");
        assert_eq!(
            resolve(project.path(), home.path()),
            2,
            "the global tier fills a key the project omits"
        );
        write_config(project.path(), "version: 1\nmemory:\n  max_concurrent: 1\n");
        assert_eq!(
            resolve(project.path(), home.path()),
            1,
            "the project tier wins"
        );

        let (home, project) = tiered(None);
        write_config(project.path(), "version: 1\n");
        assert_eq!(
            resolve(project.path(), home.path()),
            0,
            "unset is unlimited"
        );
        write_config(project.path(), "version: 1\ntest:\n  max_concurrent: 3\n");
        assert_eq!(
            resolve(project.path(), home.path()),
            0,
            "`test.max_concurrent` is not read"
        );
        for bad in ["-1", "1.5", "two"] {
            write_config(
                project.path(),
                &format!("version: 1\nmemory:\n  max_concurrent: {bad}\n"),
            );
            assert!(
                matches!(
                    discover_with(project.path(), Some(home.path())),
                    Err(ConfigError::Invalid(ref message)) if message.contains("memory.max_concurrent")
                ),
                "memory.max_concurrent {bad} is a config error"
            );
        }
    }

    /// `memory.max_percent` resolves key by key through both tiers, is 90
    /// when unset anywhere, and anything but an integer from 1 to 100 is a
    /// structured error.
    #[test]
    fn memory_max_percent_resolves_tiers_defaults_and_validates() {
        let resolve = |project: &Path, home: &Path| {
            discover_with(project, Some(home))
                .expect("discover")
                .expect("config")
                .memory_max_percent()
        };
        let (home, project) = tiered(Some("version: 1\nmemory:\n  max_percent: 70\n"));
        write_config(project.path(), "version: 1\nmemory:\n  limit_mib: 2048\n");
        assert_eq!(
            resolve(project.path(), home.path()),
            70,
            "the global tier fills a key the project omits"
        );
        write_config(project.path(), "version: 1\nmemory:\n  max_percent: 100\n");
        assert_eq!(
            resolve(project.path(), home.path()),
            100,
            "the project tier wins"
        );

        let (home, project) = tiered(None);
        write_config(project.path(), "version: 1\n");
        assert_eq!(resolve(project.path(), home.path()), 90, "unset is 90");
        for bad in ["0", "101", "-5", "50.5", "half"] {
            write_config(
                project.path(),
                &format!("version: 1\nmemory:\n  max_percent: {bad}\n"),
            );
            assert!(
                matches!(
                    discover_with(project.path(), Some(home.path())),
                    Err(ConfigError::Invalid(ref message)) if message.contains("max_percent")
                ),
                "max_percent {bad} is a config error"
            );
        }
    }

    /// The `analyzer:` section: the engine choice and the official knobs
    /// resolve through both tiers (the project section governs whole),
    /// defaults fill unset keys, `0` is the unlimited budget, and a typo is
    /// loud.
    #[test]
    fn analyzer_section_resolves_engine_and_official_knobs() {
        const GIB: u64 = 1 << 30;
        let resolve = |project: &Path, home: &Path| {
            discover_with(project, Some(home))
                .expect("discover")
                .expect("config")
        };
        let defaults = OfficialSettings {
            memory_budget_bytes: 0,
            resident: true,
            idle_minutes: 15,
        };
        let (home, project) = tiered(Some(
            "version: 1\nanalyzer:\n  engine: dartq\n  official:\n    memory_budget_gb: 4\n",
        ));
        write_config(project.path(), "version: 1\n");
        let config = resolve(project.path(), home.path());
        assert_eq!(
            config.engine(),
            Some(Engine::Dartq),
            "the global tier fills the gap"
        );
        assert_eq!(config.official().memory_budget_bytes, 4 * GIB);

        write_config(
            project.path(),
            "version: 1\nanalyzer:\n  engine: official\n  official:\n    memory_budget_gb: 0\n    resident: false\n    idle_minutes: 3\n",
        );
        let config = resolve(project.path(), home.path());
        assert_eq!(
            config.engine(),
            Some(Engine::Official),
            "the project tier wins"
        );
        assert_eq!(
            config.official(),
            OfficialSettings {
                memory_budget_bytes: 0,
                resident: false,
                idle_minutes: 3
            }
        );

        let (home, project) = tiered(None);
        write_config(project.path(), "version: 1\n");
        let config = resolve(project.path(), home.path());
        assert_eq!((config.engine(), config.official()), (None, defaults));

        for bad in [
            "version: 1\nanalyzer:\n  engine: rust\n",
            "version: 1\nanalyzer:\n  official:\n    residnet: true\n",
        ] {
            write_config(project.path(), bad);
            assert!(
                matches!(
                    discover_with(project.path(), Some(home.path())),
                    Err(ConfigError::Invalid(_))
                ),
                "{bad:?} is a structured error"
            );
        }
    }
}
