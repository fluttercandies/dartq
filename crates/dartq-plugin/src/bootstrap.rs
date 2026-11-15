//! The official-plugin bootstrap: materializing the synthetic package that
//! runs `analysis_server_plugin`-style plugins on the dartq host.
//!
//! An official plugin (the `plugins:` section of `analysis_options.yaml`)
//! is a plain pub package whose `lib/main.dart` exposes a top-level
//! `plugin` variable — it has no process entrypoint and speaks no wire
//! protocol. The official analysis server bridges that gap by *generating*
//! a shared plugin package per context root: a pubspec carrying each
//! declared plugin as a dependency (plus `dependency_overrides`), a
//! `bin/plugin.dart` that imports every plugin's `lib/main.dart` and hands
//! the plugin objects to the server (`PluginPackageGenerator`, upstream
//! Dart SDK source, `pkg/analysis_server/lib/src/plugin2/generator.dart`),
//! materialized under the plugin manager's state folder and resolved with
//! `dart pub upgrade` (`plugin_watcher.dart:121-165`,
//! `plugin_manager.dart:347-375`).
//!
//! This module is dartq's counterpart, one divergence by design: the
//! official runs all of a context root's plugins inside one isolate; the
//! dartq isolation model is one sidecar process per plugin, so the
//! synthetic package carries one generated entrypoint per declared plugin
//! (`bin/<plugin>.dart`, each serving exactly that plugin over wire v1
//! through the `analysis_server_plugin` import shim of
//! `dartq-plugin-host`). Per-plugin isolation keeps a crashing plugin's
//! census, budgets, and crash row its own; the merged output rows are the
//! same either way (the upstream merge concatenates every plugin's
//! diagnostics verbatim).
//!
//! Resolution is cached: the synthetic directory is keyed by a content
//! hash of the generated pubspec, and `dart pub get` runs only when the
//! pubspec changed or pub's `package_config.json` is missing — a repeat
//! run over an unchanged declaration set pays no pub at all (the official
//! re-runs `pub upgrade` on every server start).
//!
//! Spawns are snapshot-cached the same way: a sidecar's cold cost is the
//! Dart VM compiling the analyzer-loaded host (source compile + JIT warm,
//! seconds-scale), so the bootstrap trains one **app-jit snapshot** per
//! generated entrypoint (`dart --snapshot-kind=app-jit`, the mechanism the
//! SDK ships `analysis_server.snapshot` through): it drives one real wire
//! session of the entrypoint under the snapshot flags — handshake, a
//! config push, one engine-diagnostics batch over a tiny in-package
//! payload — then closes stdin; the host's loop ends on EOF, the process
//! exits cleanly, and the VM writes the trained snapshot. Every later
//! spawn runs the snapshot (the VM recognizes it by magic bytes, the same
//! `dfe.cc` detection the kernel kind documents), skipping the source
//! compile and starting from the trained heap. The snapshot is keyed by
//! the entrypoint text, pub's resolution (`package_config.json` pins
//! every version the snapshot embeds), the host's lib tree, and each
//! path-sourced dependency's lib tree — a dev edit retrains, everything
//! else reuses. Training failures never fail the bootstrap: the spawn
//! falls back to the source entrypoint (logged once).
//! `DARTQ_PLUGIN_SNAPSHOT=0` disables the snapshot path entirely.
//!
//! Failure faces mirror the official posture: a failed `pub get` is a
//! typed [`BootstrapError::Pub`] carrying the bounded stdout/stderr tail
//! — the CLI/daemon faces surface it (stderr line + one
//! `dartq/plugin_crashed` row, the dartq containment convention) where
//! the official prints the `PluginException` text to stderr only
//! (dartdev `analysis_server.dart:306-315`).

#![allow(clippy::semicolon_if_nothing_returned)]
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use crate::discovery::DiscoveredPlugin;
use crate::manifest::{
    DEFAULT_IDLE_EVICT_MS, DEFAULT_MEMORY_BUDGET_MB, EntrypointKind, Manifest, Timeouts,
};

/// How long `dart pub get` may run before it is killed. Generous on
/// purpose: a cold cache resolving `analyzer` downloads tens of MB; a warm
/// one finishes in well under a second. `DARTQ_PLUGIN_PUB_TIMEOUT_MS`
/// overrides (tests set it small to keep a wedged pub visible).
const PUB_TIMEOUT: Duration = Duration::from_secs(180);

/// How long a snapshot training run may run before it is killed: it is one
/// full cold session (the very cost the snapshot removes), so the bound
/// prices a wedged plugin, not a slow one. `DARTQ_PLUGIN_TRAIN_TIMEOUT_MS`
/// overrides.
const TRAIN_TIMEOUT: Duration = Duration::from_secs(120);

/// The training payload: a tiny in-package file whose parse and rule run
/// warms the analyzer machinery a real cycle exercises. An `await`
/// expression is the smoke fixture's rule shape — visitors keyed on the
/// common node kinds run; a plugin whose rules match nothing still warms
/// the parse/context path (the dominant cost).
const TRAIN_PAYLOAD: &str = concat!(
    "// dartq plugin bootstrap training payload.\n",
    "Future<void> main() async {\n",
    "  await Future<void>.delayed(Duration.zero);\n",
    "}\n",
);

/// Cap on the captured pub output folded into [`BootstrapError::Pub`] —
/// the tail names the failure (resolution conflicts print at the end);
/// earlier bytes are dropped and counted, so a chatty pub cannot grow the
/// error without bound.
const PUB_OUTPUT_CAP: usize = 8 * 1024;

/// The poll step while waiting out [`PUB_TIMEOUT`] (std has no timed wait).
const PUB_POLL: Duration = Duration::from_millis(100);

/// One declared official plugin: the section key plus its parsed source.
/// The mirror of the engine's options model
/// (`dartq_engine::analysis_options::PluginConfig`) reduced to the spawn
/// face — rule gating rides the per-cycle config, not the bootstrap.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OfficialPlugin {
    /// The declared plugin name (the section key, as written).
    pub name: String,
    /// The declared source.
    pub source: OfficialSource,
}

/// A plugin's declared source — the mirror of the upstream
/// `PluginSource` sealed family (`analysis_options.dart:631-669`) carrying
/// the `toYaml` shapes the synthetic pubspec needs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OfficialSource {
    /// `name: <constraint>` or `name: {version: <constraint>, hosted: <url>}`.
    Versioned {
        /// The version constraint string.
        constraint: String,
        /// The `hosted:` URL, when present.
        hosted: Option<String>,
    },
    /// `name: {git: <url>}` or `name: {git: {url, ref, path, tag_pattern}}`.
    Git {
        /// The repository URL.
        url: String,
        /// `git.path`.
        path: Option<String>,
        /// `git.ref`.
        reference: Option<String>,
        /// `git.tag_pattern`.
        tag_pattern: Option<String>,
    },
    /// `name: {path: <dir>}` — absolute already (the options parse joined
    /// and normalized it against the declaring file).
    Path(PathBuf),
}

impl OfficialSource {
    /// The `dependencies:` entry of the synthetic pubspec — verbatim the
    /// upstream `toYaml(name:)` shapes (`analysis_options.dart:541-668`):
    /// a bare constraint scalar, `{version, hosted}`, the git block in
    /// `url, ref, path, tag_pattern` order, or the path block. Values are
    /// interpolated raw, exactly like upstream — the declaration came out
    /// of the user's own options file.
    fn to_pubspec_dependency(&self, name: &str) -> String {
        match self {
            Self::Versioned {
                constraint,
                hosted: None,
            } => format!("  {name}: {constraint}\n"),
            Self::Versioned {
                constraint,
                hosted: Some(hosted),
            } => format!("  {name}:\n    version: {constraint}\n    hosted: {hosted}\n"),
            Self::Git {
                url,
                path,
                reference,
                tag_pattern,
            } => {
                let mut block = format!("  {name}:\n    git:\n      url: {url}\n");
                if let Some(reference) = reference {
                    block.push_str(&format!("      ref: {reference}\n"));
                }
                if let Some(path) = path {
                    block.push_str(&format!("      path: {path}\n"));
                }
                if let Some(tag_pattern) = tag_pattern {
                    block.push_str(&format!("      tag_pattern: {tag_pattern}\n"));
                }
                block
            }
            Self::Path(path) => format!("  {name}:\n    path: {}\n", path.display()),
        }
    }
}

/// Bootstrap failures. Every variant is terminal for the declared set's
/// spawn — the caller surfaces it once (stderr + a `dartq/plugin_crashed`
/// row per face's convention) and the run stays alive.
#[derive(Debug, thiserror::Error)]
pub enum BootstrapError {
    /// The dartq plugin host package cannot be located — the bootstrap has
    /// nothing to serve the plugin over wire v1 with.
    #[error(
        "cannot locate the dartq plugin host package; set DARTQ_PLUGIN_HOST to the \
         dartq-plugin-host package directory"
    )]
    HostMissing,
    /// `dart pub get` failed or timed out in the synthetic package; the
    /// bounded output tail names why (resolution conflicts, network).
    #[error("`dart pub get` failed in {}: {}", dir.display(), output)]
    Pub {
        /// The synthetic package directory.
        dir: PathBuf,
        /// The bounded tail of pub's combined output (or the timeout note).
        output: String,
    },
    /// The synthetic package could not be written.
    #[error("cannot write the plugin bootstrap package at {}: {reason}", dir.display())]
    Io {
        /// The synthetic package directory.
        dir: PathBuf,
        /// The I/O failure.
        reason: String,
    },
}

/// Locates the `dartq-plugin-host` Dart package the generated entrypoints
/// serve over. Search order: the `DARTQ_PLUGIN_HOST` environment variable
/// (the explicit override), the packaged layout beside the running
/// executable (`<exe>/../libexec/dartq-plugin-host`), then the build-time
/// source tree (a dev-build fallback; an installed binary whose build
/// machine path is stale fails the `is_dir` probe and reports
/// [`BootstrapError::HostMissing`] honestly). A hit must carry the import
/// shim (`lib/src/analysis_server_plugin_shim/pubspec.yaml`) — the
/// bootstrap maps the official package name onto it.
#[must_use]
pub fn host_package() -> Option<PathBuf> {
    host_package_with(std::env::var_os("DARTQ_PLUGIN_HOST").as_deref())
}

/// [`host_package`] with the environment override injected (tests).
fn host_package_with(env_override: Option<&std::ffi::OsStr>) -> Option<PathBuf> {
    let is_host = |dir: &Path| {
        dir.join("lib")
            .join("src")
            .join("analysis_server_plugin_shim")
            .join("pubspec.yaml")
            .is_file()
    };
    if let Some(value) = env_override
        && !value.is_empty()
        && let dir = PathBuf::from(value)
        && is_host(&dir)
    {
        return Some(dir);
    }
    if let Ok(exe) = std::env::current_exe()
        && let Some(bin) = exe.parent()
    {
        let packaged = bin.join("..").join("libexec").join("dartq-plugin-host");
        if is_host(&packaged) {
            return Some(packaged);
        }
    }
    let dev = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../dartq-plugin-host");
    is_host(&dev).then_some(dev)
}

/// The plugin-manager cache root: `<XDG_CACHE_HOME|~/.cache>/dartq` — the
/// same convention the engine's stores use. `None` when no home can be
/// established (the bootstrap then reports [`BootstrapError::HostMissing`]
/// at its first write; there is nowhere else a cache belongs).
#[must_use]
pub fn plugin_manager_root() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("XDG_CACHE_HOME").filter(|dir| !dir.is_empty()) {
        return Some(PathBuf::from(dir).join("dartq"));
    }
    std::env::home_dir().map(|home| home.join(".cache").join("dartq"))
}

/// Materializes the synthetic package for `plugins` and returns one
/// spawn-ready [`DiscoveredPlugin`] per declaration — `dir` is the
/// synthetic package (its pub resolution serves every entrypoint), the
/// manifest names `bin/<plugin>.dart` with the manifest defaults, so
/// [`crate::SpawnSpec::dart`] launches it exactly like any discovered
/// plugin. `dependency_overrides` are the section's overrides map, riding
/// the synthetic pubspec verbatim (a user override of
/// `analysis_server_plugin` itself wins over the shim mapping).
///
/// Empty input is the absent path: no filesystem touch, no pub, `Ok([])`.
///
/// # Errors
///
/// [`BootstrapError::HostMissing`] without a locatable host package,
/// [`BootstrapError::Io`] when the synthetic package cannot be written,
/// [`BootstrapError::Pub`] when `dart pub get` fails or times out.
pub fn bootstrap(
    plugins: &[OfficialPlugin],
    dependency_overrides: &[(String, OfficialSource)],
) -> Result<Vec<DiscoveredPlugin>, BootstrapError> {
    if plugins.is_empty() {
        return Ok(Vec::new());
    }
    let host = host_package().ok_or(BootstrapError::HostMissing)?;
    let shim = host
        .join("lib")
        .join("src")
        .join("analysis_server_plugin_shim");
    let root = plugin_manager_root()
        .ok_or(BootstrapError::HostMissing)?
        .join("plugin-manager");
    let pubspec = pubspec_yaml(&host, &shim, plugins, dependency_overrides);
    let dir = root.join(format!("{:016x}", content_hash(&pubspec)));
    materialize(&dir, &pubspec, plugins)?;
    // The trained-snapshot face: each entrypoint gets an app-jit snapshot
    // when one can be produced, and the manifest then names the snapshot
    // (the VM recognizes it by magic; `SpawnSpec::dart`'s argv is
    // unchanged). A missing snapshot leaves the source entrypoint.
    let snapshots = prepare_snapshots(&dir, &host, plugins, dependency_overrides);
    Ok(plugins
        .iter()
        .enumerate()
        .map(|(index, plugin)| {
            let entrypoint = snapshots
                .get(index)
                .cloned()
                .flatten()
                .unwrap_or_else(|| entrypoint_path(index, plugin));
            DiscoveredPlugin {
                dir: dir.clone(),
                manifest: Manifest {
                    name: plugin.name.clone(),
                    version: "0.0.0".to_owned(),
                    entrypoint,
                    entrypoint_kind: EntrypointKind::Source,
                    capabilities: crate::capability::CapabilitySet::empty(),
                    timeouts: Timeouts::default(),
                    memory_budget_mb: DEFAULT_MEMORY_BUDGET_MB,
                    idle_evict_ms: DEFAULT_IDLE_EVICT_MS,
                },
            }
        })
        .collect())
}

/// The synthetic pubspec — the official generator's shape
/// (`generator.dart:62-87`) with the dartq resolution line: a path
/// dependency on the host package (the generated entrypoint imports it)
/// and the `analysis_server_plugin` override onto the shim (the plugin's
/// own official imports resolve to it).
fn pubspec_yaml(
    host: &Path,
    shim: &Path,
    plugins: &[OfficialPlugin],
    dependency_overrides: &[(String, OfficialSource)],
) -> String {
    let mut yaml = String::from("name: plugin_entrypoint\nversion: 0.0.1\n");
    yaml.push_str("environment:\n  sdk: ^3.10.0\n");
    yaml.push_str("dependencies:\n");
    yaml.push_str(&format!(
        "  dartq_plugin_host:\n    path: {}\n",
        host.display()
    ));
    for plugin in plugins {
        yaml.push_str(&plugin.source.to_pubspec_dependency(&plugin.name));
    }
    let shim_overridden = dependency_overrides
        .iter()
        .any(|(name, _)| name == "analysis_server_plugin");
    yaml.push_str("dependency_overrides:\n");
    if !shim_overridden {
        yaml.push_str(&format!(
            "  analysis_server_plugin:\n    path: {}\n",
            shim.display()
        ));
    }
    for (name, source) in dependency_overrides {
        yaml.push_str(&source.to_pubspec_dependency(name));
    }
    yaml
}

/// The generated entrypoint's path inside the synthetic package: the
/// index-prefixed sanitized stem, deterministic for a declaration set.
fn entrypoint_path(index: usize, plugin: &OfficialPlugin) -> String {
    let stem: String = plugin
        .name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect();
    format!("bin/p{index}_{stem}.dart")
}

/// The generated entrypoint: the official-bootstrap stand-in the shim
/// documents (`startPlugin`), serving exactly this plugin over wire v1.
/// The manifest rides inline (`PluginConfig`) — the synthetic package
/// serves N plugins from one directory, so the file-read convention of
/// `startPlugin` cannot name just one.
fn entrypoint_dart(plugin: &OfficialPlugin, entrypoint: &str) -> String {
    format!(
        "// Generated by dartq: the launch bootstrap for plugin `{name}`.\n\
         import 'package:analysis_server_plugin/plugin.dart' show startPlugin;\n\
         import 'package:dartq_plugin_host/dartq_plugin_host.dart' show PluginConfig;\n\
         import 'package:{name}/main.dart' as p0;\n\
         \n\
         Future<void> main(List<String> args) async {{\n\
         \x20 await startPlugin(\n\
         \x20   p0.plugin,\n\
         \x20   config: PluginConfig(\n\
         \x20     name: {name_literal},\n\
         \x20     version: '0.0.0',\n\
         \x20     entrypoint: {entrypoint_literal},\n\
         \x20   ),\n\
         \x20 );\n\
         }}\n",
        name = plugin.name,
        name_literal = dart_string_literal(&plugin.name),
        entrypoint_literal = dart_string_literal(entrypoint),
    )
}

/// `s` as a single-quoted Dart string literal: `\`, `'`, `$` (interpolation
/// even inside single quotes), and control characters escaped.
fn dart_string_literal(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\'' => out.push_str("\\'"),
            '$' => out.push_str("\\$"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\x{:02X}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('\'');
    out
}

/// Writes the pubspec and every entrypoint (compare-before-write, the
/// official's timestamp-preserving posture), then resolves the package
/// when anything changed or pub's resolution is missing.
fn materialize(
    dir: &Path,
    pubspec: &str,
    plugins: &[OfficialPlugin],
) -> Result<(), BootstrapError> {
    let io = |reason: String| BootstrapError::Io {
        dir: dir.to_path_buf(),
        reason,
    };
    std::fs::create_dir_all(dir.join("bin")).map_err(|e| io(e.to_string()))?;
    let mut changed =
        write_if_changed(&dir.join("pubspec.yaml"), pubspec).map_err(|e| io(e.to_string()))?;
    for (index, plugin) in plugins.iter().enumerate() {
        let entry = entrypoint_path(index, plugin);
        let body = entrypoint_dart(plugin, &entry);
        changed |= write_if_changed(&dir.join(&entry), &body).map_err(|e| io(e.to_string()))?;
    }
    let resolved = dir.join(".dart_tool").join("package_config.json").is_file();
    if changed || !resolved {
        pub_get(dir)?;
    }
    Ok(())
}

/// `dart pub get` under [`PUB_TIMEOUT`]: the child leads its own process
/// group (a pub descendant holding the output pipes must die with it),
/// both streams drain into bounded tail buffers (a full pipe must not
/// wedge pub), and an overrun is a group kill plus the timeout note.
fn pub_get(dir: &Path) -> Result<(), BootstrapError> {
    #[cfg(unix)]
    let mut command = {
        use std::os::unix::process::CommandExt;
        let mut command = Command::new("dart");
        command.args(["pub", "get"]).current_dir(dir);
        command.process_group(0);
        command
    };
    #[cfg(not(unix))]
    let mut command = {
        let mut command = Command::new("dart");
        command.args(["pub", "get"]).current_dir(dir);
        command
    };
    command.stdin(Stdio::null());
    let timeout = std::env::var("DARTQ_PLUGIN_PUB_TIMEOUT_MS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .map_or(PUB_TIMEOUT, Duration::from_millis);
    let mut child = match command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(error) => {
            return Err(BootstrapError::Pub {
                dir: dir.to_path_buf(),
                output: format!("cannot spawn `dart pub get`: {error}"),
            });
        }
    };
    let stdout = drain(child.stdout.take());
    let stderr = drain(child.stderr.take());
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => {
                let _ = stdout.join();
                let _ = stderr.join();
                return Ok(());
            }
            Ok(Some(status)) => {
                let out_tail = stdout.join().ok().flatten().unwrap_or_default();
                let err_tail = stderr.join().ok().flatten().unwrap_or_default();
                let mut output = format!("exit status {status}");
                for tail in [out_tail, err_tail]
                    .into_iter()
                    .filter(|tail| !tail.is_empty())
                {
                    output.push_str(":\n");
                    output.push_str(&tail);
                }
                return Err(BootstrapError::Pub {
                    dir: dir.to_path_buf(),
                    output,
                });
            }
            Ok(None) if Instant::now() >= deadline => {
                kill_and_reap(&mut child);
                let _ = stdout.join();
                let _ = stderr.join();
                return Err(BootstrapError::Pub {
                    dir: dir.to_path_buf(),
                    output: format!("timed out after {} ms (killed)", timeout.as_millis()),
                });
            }
            Ok(None) => thread::sleep(PUB_POLL),
            Err(error) => {
                kill_and_reap(&mut child);
                let _ = stdout.join();
                let _ = stderr.join();
                return Err(BootstrapError::Pub {
                    dir: dir.to_path_buf(),
                    output: format!("wait failed: {error}"),
                });
            }
        }
    }
}

/// The snapshot decision per declared plugin: `Some(rel_path)` names a
/// fresh app-jit snapshot the manifest should spawn (trained here when
/// stale), `None` keeps the source entrypoint (snapshots disabled, or the
/// training failed — logged, never fatal).
fn prepare_snapshots(
    dir: &Path,
    host: &Path,
    plugins: &[OfficialPlugin],
    dependency_overrides: &[(String, OfficialSource)],
) -> Vec<Option<String>> {
    if std::env::var("DARTQ_PLUGIN_SNAPSHOT").as_deref() == Ok("0") {
        return vec![None; plugins.len()];
    }
    let timeout = std::env::var("DARTQ_PLUGIN_TRAIN_TIMEOUT_MS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .map_or(TRAIN_TIMEOUT, Duration::from_millis);
    plugins
        .iter()
        .enumerate()
        .map(|(index, plugin)| {
            let entry = entrypoint_path(index, plugin);
            let snapshot = format!("{entry}.jit");
            let key = format!(
                "{:016x}",
                snapshot_key(dir, &entry, host, plugins, dependency_overrides)
            );
            let fresh = dir.join(&snapshot).is_file()
                && std::fs::read_to_string(dir.join(format!("{entry}.key")))
                    .is_ok_and(|stored| stored.trim() == key);
            if fresh {
                return Some(snapshot);
            }
            match train_snapshot(dir, &entry, timeout) {
                Ok(()) => {
                    let _ = std::fs::write(dir.join(format!("{entry}.key")), key);
                    Some(snapshot)
                }
                Err(reason) => {
                    eprintln!(
                        "dartq: plugin `{}` snapshot training failed ({reason}); \
                         spawning from source",
                        plugin.name
                    );
                    None
                }
            }
        })
        .collect()
}

/// The snapshot cache key: everything whose bytes the trained image embeds.
/// The entrypoint text (the generated serve loop), pub's resolution (the
/// `package_config.json` pins every resolved version and path — the
/// analyzer's, the host's, every hosted/git plugin's), the host's lib
/// tree, and each path-sourced dependency's lib tree (a path never changes
/// on a dev edit; the content does — the fingerprint catches it).
fn snapshot_key(
    dir: &Path,
    entry_rel: &str,
    host: &Path,
    plugins: &[OfficialPlugin],
    dependency_overrides: &[(String, OfficialSource)],
) -> u64 {
    let mut hasher = DefaultHasher::new();
    entry_rel.hash(&mut hasher);
    std::fs::read_to_string(dir.join(entry_rel))
        .unwrap_or_default()
        .hash(&mut hasher);
    std::fs::read_to_string(dir.join(".dart_tool").join("package_config.json"))
        .unwrap_or_default()
        .hash(&mut hasher);
    fingerprint_tree(&host.join("lib")).hash(&mut hasher);
    for source in plugins
        .iter()
        .map(|plugin| &plugin.source)
        .chain(dependency_overrides.iter().map(|(_, source)| source))
    {
        if let OfficialSource::Path(path) = source {
            path.hash(&mut hasher);
            fingerprint_tree(&path.join("lib")).hash(&mut hasher);
        }
    }
    hasher.finish()
}

/// A content fingerprint of a source tree: the fold of every file's name
/// and bytes under `root` (skipping build metadata directories),
/// order-stable by sorted walk — a restored edit restores the key (no
/// mtime sensitivity, no spurious retraining). The trees this folds are
/// package `lib/` trees (kilobytes), so the read is sub-millisecond. A
/// missing tree folds to the root's own hash — an absent dependency is
/// the caller's resolution problem, not a key change.
fn fingerprint_tree(root: &Path) -> u64 {
    fn walk(dir: &Path, hasher: &mut DefaultHasher) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        let mut entries: Vec<_> = entries.flatten().collect();
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let name = entry.file_name();
            if name == ".dart_tool" || name == ".git" {
                continue;
            }
            let path = entry.path();
            if path.is_dir() {
                walk(&path, hasher);
                continue;
            }
            name.hash(hasher);
            if let Ok(bytes) = std::fs::read(&path) {
                bytes.hash(hasher);
            }
        }
    }
    let mut hasher = DefaultHasher::new();
    root.hash(&mut hasher);
    walk(root, &mut hasher);
    hasher.finish()
}

/// Trains one app-jit snapshot of `entry_rel`: runs the entrypoint under
/// `dart --snapshot-kind=app-jit`, drives one real wire session (the
/// handshake, a config push, one engine-diagnostics batch over the
/// training payload), then closes stdin — the host's loop ends on EOF,
/// the process exits zero, and the VM writes the trained snapshot, which
/// is staged pid-uniquely and renamed into place (two bootstraps racing
/// on one cache directory cannot interleave a half-written snapshot).
fn train_snapshot(dir: &Path, entry_rel: &str, timeout: Duration) -> Result<(), String> {
    let payload = dir.join("lib").join("_train_payload.dart");
    if let Some(parent) = payload.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("payload dir: {e}"))?;
    }
    std::fs::write(&payload, TRAIN_PAYLOAD).map_err(|e| format!("payload write: {e}"))?;
    let staged = dir.join(format!(".train-{}.snapshot", std::process::id()));
    let _ = std::fs::remove_file(&staged);
    let snapshot = dir.join(format!("{entry_rel}.jit"));

    let mut command = Command::new("dart");
    command
        .arg("--snapshot-kind=app-jit")
        .arg(format!("--snapshot={}", staged.display()))
        .arg(entry_rel)
        .current_dir(dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = command
        .spawn()
        .map_err(|e| format!("cannot spawn the training run: {e}"))?;
    let mut stdin = child.stdin.take().expect("stdin piped");
    let stdout = child.stdout.take().expect("stdout piped");
    let stderr = drain(child.stderr.take());
    let (lines_tx, lines) = mpsc::channel::<String>();
    let reader = thread::spawn(move || {
        use std::io::BufRead;
        for line in std::io::BufReader::new(stdout).lines() {
            match line {
                Ok(line) => {
                    if lines_tx.send(line).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });
    let deadline = Instant::now() + timeout;
    let drove = drive_training(&mut stdin, &lines, &payload, deadline);
    // Close stdin whatever the drive's outcome: the host's loop ends on
    // EOF, and only a clean exit writes the snapshot.
    drop(stdin);
    let mut exited = None;
    while Instant::now() < deadline {
        match child.try_wait() {
            Ok(Some(status)) => {
                exited = Some(status);
                break;
            }
            Ok(None) => thread::sleep(PUB_POLL),
            Err(error) => {
                kill_and_reap(&mut child);
                let _ = reader.join();
                return Err(format!("training wait failed: {error}"));
            }
        }
    }
    let status = match (exited, drove) {
        (Some(status), Ok(())) => status,
        (Some(_), Err(reason)) => {
            let _ = reader.join();
            return Err(reason);
        }
        (None, _) => {
            kill_and_reap(&mut child);
            let _ = reader.join();
            return Err(format!(
                "timed out after {} ms (killed)",
                timeout.as_millis()
            ));
        }
    };
    let tail = stderr.join().ok().flatten().unwrap_or_default();
    let _ = reader.join();
    if !status.success() {
        return Err(format!(
            "training run exited {status}{}",
            if tail.is_empty() {
                String::new()
            } else {
                format!(":\n{tail}")
            }
        ));
    }
    if !staged.is_file() || staged.metadata().is_ok_and(|meta| meta.len() == 0) {
        return Err(format!(
            "the VM wrote no snapshot ({}){}",
            staged.display(),
            if tail.is_empty() {
                String::new()
            } else {
                format!(":\n{tail}")
            }
        ));
    }
    std::fs::rename(&staged, &snapshot).map_err(|e| format!("snapshot publish: {e}"))
}

/// Drives the training session's wire exchange: the `initialize`
/// handshake, one `notification/plugin_config`, and one
/// `notification/engine_diagnostics` naming the payload file, then parks
/// briefly for the plugin's findings (a rule-less plugin stays silent —
/// the parse/context path still ran, which is the training point). No
/// `shutdown` frame: closing stdin (the caller's next move) is the loop's
/// own end.
fn drive_training(
    stdin: &mut impl Write,
    lines: &mpsc::Receiver<String>,
    payload: &Path,
    deadline: Instant,
) -> Result<(), String> {
    let mut send = |frame: &crate::Frame| -> Result<(), String> {
        let mut bytes = crate::encode_frame(frame).map_err(|e| format!("encode: {e}"))?;
        bytes.push(b'\n');
        stdin
            .write_all(&bytes)
            .map_err(|e| format!("the training pipe broke: {e}"))
    };
    let mut initialize = serde_json::Map::new();
    initialize.insert(
        "protocol_version".to_owned(),
        serde_json::json!(crate::PROTOCOL_VERSION),
    );
    initialize.insert(
        "capabilities".to_owned(),
        serde_json::json!(["diagnostics", "fix", "query"]),
    );
    send(&crate::Frame::request(
        1,
        crate::METHOD_INITIALIZE,
        initialize,
    ))?;
    // The handshake reply (id 1); anything else this early is a contract
    // break worth failing on.
    match lines.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
        Ok(line) => {
            if let Some(error) = serde_json::from_str::<serde_json::Value>(&line)
                .ok()
                .and_then(|frame| frame.get("error").cloned())
            {
                return Err(format!("the plugin rejected initialize: {error}"));
            }
        }
        Err(_) => return Err("no initialize reply within the training budget".to_owned()),
    }
    let mut config = serde_json::Map::new();
    config.insert("enabled_rules".to_owned(), serde_json::json!([]));
    config.insert("severity_overrides".to_owned(), serde_json::json!({}));
    config.insert("overlays".to_owned(), serde_json::json!([]));
    send(&crate::Frame::notification(
        crate::METHOD_PLUGIN_CONFIG,
        config,
    ))?;
    let mut batch = serde_json::Map::new();
    batch.insert("diagnostics".to_owned(), serde_json::json!([]));
    batch.insert(
        "files".to_owned(),
        serde_json::json!([payload.display().to_string()]),
    );
    send(&crate::Frame::notification(
        crate::METHOD_ENGINE_DIAGNOSTICS,
        batch,
    ))?;
    // Park for the findings: the first line within 10 s (the rules run),
    // then until a 2 s quiet window — all bounded by the deadline. Silence
    // is a legal outcome (rule-less plugins).
    let started = Instant::now();
    let mut saw_output = false;
    loop {
        let budget = if saw_output {
            Duration::from_secs(2)
        } else {
            Duration::from_secs(10).min(deadline.saturating_duration_since(Instant::now()))
        };
        if budget.is_zero() {
            break;
        }
        match lines.recv_timeout(budget) {
            Ok(_) => {
                saw_output = true;
                if started.elapsed() > Duration::from_secs(30) {
                    break;
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => break,
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    Ok(())
}

/// Kills a pub or training child and reaps it: a group kill on Unix
/// (descendants that inherited the pipes go down with it), the direct
/// kill as fallback — the same posture the runtime's generation teardown
/// takes.
fn kill_and_reap(child: &mut Child) {
    #[cfg(unix)]
    if let Ok(pid) = i32::try_from(child.id()) {
        // Safety: `kill(2)` only signals; the negative pid targets the
        // child's own process group, never ours.
        unsafe {
            libc::kill(-pid, libc::SIGKILL);
        }
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// Spawns one bounded drain thread for a pub output pipe: reads to EOF,
/// keeping only the last [`PUB_OUTPUT_CAP`] bytes (earlier bytes counted
/// and noted), so the pipe never fills and the retained text is the tail
/// that names a failure.
fn drain<R: Read + Send + 'static>(pipe: Option<R>) -> thread::JoinHandle<Option<String>> {
    thread::spawn(move || {
        let Some(mut pipe) = pipe else {
            return None;
        };
        let mut tail: Vec<u8> = Vec::new();
        let mut dropped: u64 = 0;
        let mut chunk = [0u8; 4096];
        loop {
            match pipe.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    tail.extend_from_slice(&chunk[..n]);
                    let excess = tail.len().saturating_sub(PUB_OUTPUT_CAP);
                    if excess > 0 {
                        dropped = dropped.saturating_add(u64::try_from(excess).unwrap_or(u64::MAX));
                        tail.drain(..excess);
                    }
                }
            }
        }
        if tail.is_empty() {
            return None;
        }
        let mut text = String::new();
        if dropped > 0 {
            text.push_str(&format!("[{dropped} earlier bytes dropped]\n"));
        }
        text.push_str(&String::from_utf8_lossy(&tail));
        Some(text)
    })
}

/// Writes `contents` to `path` when it differs from what is there.
/// `Ok(true)` when the file changed (or was created), `Ok(false)` when
/// identical bytes were already on disk — the timestamp-preserving
/// compare of the official generator (`plugin_watcher.dart:141-155`).
fn write_if_changed(path: &Path, contents: &str) -> std::io::Result<bool> {
    if std::fs::read(path).is_ok_and(|current| current == contents.as_bytes()) {
        return Ok(false);
    }
    std::fs::write(path, contents)?;
    Ok(true)
}

/// The stable content hash of the pubspec text — the cache key. A moved
/// host path, a changed declaration, or new overrides all change the text
/// and therefore the directory; unchanged declarations reuse the resolved
/// package across runs. `DefaultHasher` is deterministic across processes
/// (fixed keys), which is all a cache key needs.
fn content_hash(text: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
mod tests;
