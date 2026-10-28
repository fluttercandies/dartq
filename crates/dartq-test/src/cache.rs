//! Content-addressed suite result cache.
//!
//! A test suite's outcome is a function of the bytes it can observe. The
//! cache keys each suite on a blake3 digest over:
//!
//! - the **toolchain**: the runner identity (dart/flutter binary and its
//!   SDK `version`/`revision` files), the passthrough arguments, and the
//!   environment's toolchain half (`DART_*`/`FLUTTER_*`); the variables a
//!   suite's closure reads by literal key join per-suite below, and an
//!   unnamable environment read (`Platform.environment` iterated, aliased,
//!   computed-keyed) refuses caching only when the reading file belongs to
//!   the suite's own package (the package under test and its path
//!   dependencies — the workspace-authored code the run can hold
//!   responsible for the read). A dependency package's unnamable read
//!   folds a digest of the whole current environment into that suite's
//!   key instead, so an environment change re-keys and an identical one
//!   replays; the toolchain tier's unnamable reads (the tester binding's
//!   whole-map default) stay exempt plumbing, its named reads keying
//!   like any other file's;
//! - the **package resolution**: `pubspec.yaml`, `pubspec.lock`,
//!   `.dart_tool/package_config.json`, and `dart_test.yaml`;
//! - the suite's **import closure**: every Dart file transitively reached
//!   through `import`/`export`/`part` edges, resolved through the package
//!   config (a missing target is an input too);
//! - its **run-time inputs**: every package file a path-shaped literal in
//!   the suite-side closure names (a directory literal covers its subtree;
//!   the package's own `lib/` is library code reading caller-supplied
//!   paths, so its literals are not inputs), and — when any file of the
//!   closure reads the process location (`Directory.current`, `Process.*`,
//!   `Uri.base`, …) — every file of the package.
//!
//! ## Sharing across checkouts
//!
//! Every path enters a key relative to the package root, so two checkouts
//! of the same content (sibling `git worktree`s, a copy elsewhere on disk)
//! compute the same keys. Results live in one per-user, content-addressed
//! store ([`crate::host::cache_dir`]), one immutable file per key: a suite
//! green in one worktree replays in every other worktree at the same
//! content. A suite whose suite-side closure (its tests and their helpers,
//! not the package's `lib/`) derives absolute locations
//! ([`FileFacts::located`]) can see the one difference between two such
//! checkouts — the location string itself — in ways no static rule bounds
//! (its length, its depth, any substring), so its key adds the absolute
//! package root and the result replays only in its own checkout. A file
//! whose author attests `// dartq: portable` lifts that pin for the
//! derivations it makes (see [`FileFacts::portable`]).
//!
//! ## What is never cached
//!
//! Replay is sound only when the outcome is a function of the key. The
//! cache refuses (the suite always runs) whenever that cannot be shown:
//!
//! - the closure reads drifting state no input covers
//!   ([`FileFacts::nondeterministic`]: the wall clock, the network, child
//!   processes, standard input, the host name, native code, or a package
//!   whose purpose is one of those) — unless the reading file attests
//!   `// dartq: deterministic`;
//! - the closure spells an absolute path naming an existing directory
//!   (whose listing no key covers). An absolute path naming a file, or
//!   nothing, is an input like any other — its bytes or its absence —
//!   so a mock path in fixture data costs nothing and a real
//!   `/etc/hosts` read invalidates when the file changes
//!   ([`FileFacts::absolute_paths`]);
//! - any file the suite reaches carries `// dartq: no-cache`;
//! - a directive the scanner cannot read with certainty, a `package:` URI
//!   outside the resolution, or a non-`file:` package root.
//!
//! Drift is checked over every package-local file of the closure — the
//! suite, its helpers, and the package's own `lib/`. Hosted dependencies
//! are immutable per pinned version and are not entered; a direct import
//! of a package whose purpose is drift (`clock`, HTTP clients and servers,
//! process spawning — [`crate::deps::DRIFT_PACKAGES`]) counts as a drifting
//! read. Measured on a 171-suite package, extending the check into every
//! hosted file would refuse 159 suites, nearly all through the analyzer's
//! internal modification stamps, which never reach a test verdict; the
//! `// dartq: no-cache` marker covers a dependency that does drift a
//! verdict. The per-checkout stamp
//! index stays under the package's `.dart_tool`: it maps absolute paths to
//! digests and is meaningless elsewhere.
//!
//! A suite is never cached when the key cannot be trusted: a directive
//! the scanner cannot read with certainty, a `package:` URI outside the
//! resolution, or a non-`file:` package root. Only suites whose run
//! completed (the reporter's `done` event) and whose every test succeeded
//! or was skipped are stored — failures always rerun, so a flaky failure
//! is never frozen into a cached verdict.

use crate::deps::FileFacts;
use crate::stamp::{Stamps, package_files, write_atomic};
use rayon::prelude::*;
use rustc_hash::{FxHashMap, FxHashSet};
use serde::{Deserialize, Serialize};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Cache layout version: bump on any key or entry format change.
const FORMAT: &str = "dartq-test-cache-v11";

/// Stored outcomes kept in the shared store; the least recently used are
/// pruned past it. Every revision of every suite of every checkout is one
/// entry, so this bounds the store at a few hundred megabytes.
const MAX_ENTRIES: usize = 50_000;

/// Minimum spacing between two prune passes over the shared store.
const PRUNE_EVERY: std::time::Duration = std::time::Duration::from_secs(24 * 60 * 60);

/// Stand-ins for the package root in stored events (`url` fields, suite
/// paths and messages of runners that spell suites absolutely), as a path
/// and as a `file:` URI. Both contain characters no path the runner
/// prints does (a NUL).
const ROOT_MARK: &str = "\u{0}dartq:root\u{0}/";
const ROOT_URI_MARK: &str = "\u{0}dartq:root-uri\u{0}/";

/// The root whose `.dart_tool/package_config.json` resolves the package at
/// `root`: the package itself, or — for a pub workspace member — the
/// workspace root its `.dart_tool/pub/workspace_ref.json` names. `None`
/// when the member's pointer is unreadable.
#[must_use]
pub fn workspace_root(root: &Path) -> Option<PathBuf> {
    let reference = root.join(".dart_tool").join("pub").join("workspace_ref.json");
    let Ok(text) = std::fs::read_to_string(&reference) else {
        return Some(root.to_path_buf());
    };
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    let relative = value.get("workspaceRoot")?.as_str()?;
    let dir = reference.parent()?.join(relative);
    Some(dartq_source::normalize_lexical(&dir))
}

/// The package configuration resolving the package at `root`.
#[must_use]
pub fn package_config_path(root: &Path) -> PathBuf {
    workspace_root(root).unwrap_or_else(|| root.to_path_buf()).join(".dart_tool").join("package_config.json")
}

/// One path segment as `Uri.file` writes it: unreserved characters and
/// the sub-delimiters Dart leaves alone stay, everything else is
/// percent-encoded byte by byte.
pub(crate) fn encode_uri_segment(segment: &str) -> String {
    let mut out = String::with_capacity(segment.len());
    for byte in segment.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~!$&'()*+,;=:@".contains(&byte) {
            out.push(char::from(byte));
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// Environment variables a shell or terminal rewrites around every command
/// or session; no test runner or test reads them.
const VOLATILE_ENV: &[&str] = &[
    "_",
    "OLDPWD",
    "PWD",
    "SHLVL",
    "TERM_SESSION_ID",
    "ITERM_SESSION_ID",
    "SECURITYSESSIONID",
    "SSH_AUTH_SOCK",
    "XPC_SERVICE_NAME",
    "XPC_FLAGS",
    "__CFBundleIdentifier",
];

/// Comma-separated extra variable names left out of every key; a trailing
/// `*` matches a prefix (`MY_AGENT_*`). For hosts that inject per-session
/// identifiers a test never reads.
const IGNORE_ENV_VAR: &str = "DARTQ_CACHE_IGNORE_ENV";

/// Package-root files whose bytes shape every suite: the resolution, the
/// runner configuration, the analysis configuration analyzer-driven tests
/// discover from the package root.
const PACKAGE_INPUTS: &[&str] = &[
    "pubspec.yaml",
    "pubspec.lock",
    "pubspec_overrides.yaml",
    ".dart_tool/package_config.json",
    "dart_test.yaml",
    "analysis_options.yaml",
];

/// The runner a result came from.
#[derive(Debug, Clone, Copy)]
pub struct Runner<'a> {
    /// The `dart`/`flutter` entry point invoked.
    pub bin: &'a Path,
    /// Arguments passed through to the runner.
    pub args: &'a [String],
    pub flutter: bool,
}

/// What `flutter test` feeds a suite beyond its import closure.
///
/// - `flutter_test_config.dart`: the nearest one in the suite's directory
///   or an ancestor up to the package root wraps the suite's `main`
///   (`testExecutable`), so it and its closure are inputs; every closer
///   candidate's absence is an input too (creating one changes which
///   wraps the suite).
/// - The test asset bundle (`build/unit_test_assets`, built from the
///   pubspec's `flutter: assets:` and fonts) is readable from any suite
///   through `rootBundle`: every file a pubspec asset entry names is an
///   input of every suite.
pub fn flutter_inputs(root: &Path, suite: &Path) -> (Vec<PathBuf>, Vec<PathBuf>) {
    let mut absent = Vec::new();
    let mut configs = Vec::new();
    let mut dir = suite.parent();
    while let Some(current) = dir {
        let candidate = current.join("flutter_test_config.dart");
        if candidate.is_file() {
            configs.push(candidate);
            break;
        }
        absent.push(candidate);
        if current == root || current.join("pubspec.yaml").is_file() {
            break;
        }
        dir = current.parent();
    }
    (configs, absent)
}

/// Files the pubspec's `flutter:` section bundles as test assets (`assets`
/// entries — files, or directories whose direct files are bundled — and
/// font files), plus the pubspec itself (already a package input).
pub(crate) fn flutter_assets(root: &Path) -> Vec<PathBuf> {
    let Ok(text) = std::fs::read_to_string(root.join("pubspec.yaml")) else {
        return Vec::new();
    };
    let Ok(value) = serde_yaml::from_str::<serde_yaml::Value>(&text) else {
        return Vec::new();
    };
    let Some(flutter) = value.get("flutter") else {
        return Vec::new();
    };
    let mut named: Vec<String> = Vec::new();
    if let Some(serde_yaml::Value::Sequence(assets)) = flutter.get("assets") {
        for asset in assets {
            let path = asset.as_str().or_else(|| asset.get("path").and_then(serde_yaml::Value::as_str));
            if let Some(path) = path {
                named.push(path.to_owned());
            }
        }
    }
    if let Some(serde_yaml::Value::Sequence(fonts)) = flutter.get("fonts") {
        for family in fonts {
            if let Some(serde_yaml::Value::Sequence(files)) = family.get("fonts") {
                for file in files {
                    if let Some(asset) = file.get("asset").and_then(serde_yaml::Value::as_str) {
                        named.push(asset.to_owned());
                    }
                }
            }
        }
    }
    let mut out = Vec::new();
    for entry in named {
        let path = root.join(entry.trim_end_matches('/'));
        if path.is_dir() {
            // A directory entry bundles its direct files (and the
            // resolution variants under `2.0x/`-style subdirectories): the
            // whole subtree is covered.
            let mut stack = vec![path];
            while let Some(dir) = stack.pop() {
                let Ok(entries) = std::fs::read_dir(&dir) else { continue };
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.is_dir() {
                        stack.push(path);
                    } else {
                        out.push(path);
                    }
                }
            }
        } else {
            out.push(path);
        }
    }
    out.sort_unstable();
    out
}

/// One stored outcome: the suite's rows under one key.
/// One stored entry: the suite's reporter events exactly as the runner
/// wrote them, one JSON line each (ids and times as in the run that
/// recorded it; a replay renumbers and re-times them).
#[derive(Debug, Serialize, Deserialize)]
struct Stored {
    format: String,
    events: Vec<crate::event::Json>,
}

/// The result cache of one package.
#[derive(Debug)]
pub struct Cache {
    root: PathBuf,
    /// Per-checkout state: the stamp index.
    dir: PathBuf,
    /// The content-addressed result store, shared by every checkout.
    store: PathBuf,
    stamps: Stamps,
    /// This process's private staging directory under the store: entries
    /// are written here as suites settle and only renamed into the store
    /// once the suite's inputs are confirmed unchanged by the run
    /// ([`Self::commit`]), so no other process ever replays an entry whose
    /// verdict was not proven for its key.
    staging: PathBuf,
    staged: FxHashSet<String>,
    /// Per-suite [`Refusal`] of the last [`Self::keys`]/[`Self::rekey`]
    /// call, aligned to that call's suite order (`None` where the suite
    /// keyed). Empty before the first call.
    refusals: Vec<Option<Refusal>>,
}

impl Cache {
    /// Open the cache of the package at `root` (canonical), backed by the
    /// per-user shared store (or the package's own `.dart_tool` when the
    /// environment names no cache location).
    #[must_use]
    pub fn open(root: &Path) -> Self {
        let dir = root.join(".dart_tool").join("dartq").join("test");
        let store = crate::host::cache_dir()
            .map_or_else(|| dir.join("results"), |shared| shared.join("test").join("results"));
        Self::with_store(root, store)
    }

    /// [`Self::open`] with an explicit result store directory.
    #[must_use]
    pub fn with_store(root: &Path, store: PathBuf) -> Self {
        let dir = root.join(".dart_tool").join("dartq").join("test");
        Self {
            stamps: Stamps::load(&dir.join("index.json")),
            root: root.to_path_buf(),
            dir,
            store: store.clone(),
            staging: store.join(format!(".staging-{}", std::process::id())),
            staged: FxHashSet::default(),
            refusals: Vec::new(),
        }
    }

    /// The cache key of every suite (absolute paths under the root), in
    /// order; `None` where the suite's inputs cannot be established with
    /// certainty, which means: always run it.
    ///
    /// # Errors
    ///
    /// I/O failures reading the package tree.
    pub fn keys(&mut self, suites: &[PathBuf], runner: Runner<'_>) -> io::Result<Vec<Option<String>>> {
        // A resolution-level failure refuses every suite at once, with the
        // same reason recorded for each.
        let none = |cache: &mut Self| {
            cache.refusals = vec![Some(Refusal::Resolution); suites.len()];
            vec![None; suites.len()]
        };
        let config_path = package_config_path(&self.root);
        let text = match std::fs::read_to_string(&config_path) {
            Ok(text) => text,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(none(self)),
            Err(error) => return Err(error),
        };
        let Ok(config) = dartq_pub::PackageConfig::parse(&text) else {
            return Ok(none(self));
        };
        let Some(dart_tool) = config_path.parent() else {
            return Ok(none(self));
        };
        let Ok(packages) = config.resolve(dart_tool) else {
            return Ok(none(self));
        };
        let lib_dirs: FxHashMap<&str, &Path> = packages
            .iter()
            .map(|package| (package.name.as_str(), package.lib_dir.as_path()))
            .collect();
        // Hosted packages are leaves (immutable per version, pinned by the
        // lock). The Flutter SDK's packages are a git checkout a developer
        // may edit, so they stay inputs, but like hosted code they are the
        // toolchain's: their drift and path literals are not the suite's.
        // The config's `flutterRoot` property names the SDK, but pub only
        // writes it when it resolves inside the tool (`FLUTTER_ROOT` set —
        // a `dart pub get` resolution or another generator's config
        // carries none), so on a flutter run the binary's own SDK names
        // the same checkout the toolchain stamps pin and classifies it
        // too.
        let immutable: Vec<PathBuf> = config.pub_cache_root().into_iter().collect();
        let mut toolchain: Vec<PathBuf> = config
            .additional_properties
            .get("flutterRoot")
            .and_then(serde_json::Value::as_str)
            .and_then(|uri| uri.strip_prefix("file://"))
            .map(|root| PathBuf::from(root).join("packages"))
            .into_iter()
            .collect();
        if runner.flutter {
            let bin = runner.bin.canonicalize().unwrap_or_else(|_| runner.bin.to_path_buf());
            if let Some(packages) = bin.parent().and_then(Path::parent).map(|sdk| sdk.join("packages"))
                && !toolchain.contains(&packages)
            {
                toolchain.push(packages);
            }
        }

        let base = self.base_key(runner)?;

        // Ownership tier of every closure file for the environment rules:
        // the suite's *own* package is the package under test (`self.root`,
        // which covers its `test/` trees too) plus its path dependencies —
        // workspace-authored code whose reads are the suite's own to key.
        // Everything else a suite can reach is a *dependency* (hosted,
        // git and sdk sources, or anything the resolution names outside
        // the workspace); the Flutter SDK's packages keep their separate
        // toolchain tier. The lock — already a package input — names the
        // source of every resolved package; a missing or unreadable lock
        // leaves only the package under test itself "own", a conservative
        // floor that errs toward digest-keying rather than refusing.
        let own_names: FxHashSet<String> = dart_tool
            .parent()
            .and_then(|root| std::fs::read_to_string(root.join("pubspec.lock")).ok())
            .and_then(|text| dartq_pub::LockFile::parse(&text).ok())
            .map(|lock| {
                lock.packages
                    .into_iter()
                    .filter(|(_, package)| package.source == dartq_pub::SourceKind::Path)
                    .map(|(name, _)| name)
                    .collect()
            })
            .unwrap_or_default();
        let own: Vec<PathBuf> = packages
            .iter()
            .filter(|package| own_names.contains(package.name.as_str()))
            .map(|package| package.root.clone())
            .collect();

        // `flutter test` wraps each suite in its `flutter_test_config.dart`:
        // the configs' closures join the graph.
        let wrappers: Vec<(Vec<PathBuf>, Vec<PathBuf>)> = if runner.flutter {
            suites.iter().map(|suite| flutter_inputs(&self.root, suite)).collect()
        } else {
            Vec::new()
        };
        let assets = if runner.flutter { flutter_assets(&self.root) } else { Vec::new() };
        let mut roots: Vec<PathBuf> = suites.to_vec();
        roots.extend(wrappers.iter().flat_map(|(configs, _)| configs.iter().cloned()));
        let graph = self.import_graph(&roots, &lib_dirs, &immutable)?;
        let package = package_files(&self.root)?;
        let index = PathIndex::new(&self.root, &package);

        // Inputs per suite as interned ids; `Err` = refused, with the
        // reason the closure walk met first.
        let mut paths = Interner::default();
        let package_ids: Vec<u32> = package.iter().map(|path| paths.intern(path)).collect();
        let mut memo = FxHashMap::default();
        let dense = Dense::new(&self.root, &graph, &toolchain, &own, &mut paths);
        let asset_ids: Vec<u32> = assets.iter().map(|path| paths.intern(path)).collect();
        let mut inputs: Vec<Result<SuiteInputs, Refusal>> = suites
            .iter()
            .enumerate()
            .map(|(index_in_run, suite)| {
                let mut inputs = dense.suite_inputs(&self.root, suite, &package_ids, &index, &mut paths, &mut memo)?;
                if let Some((configs, absent)) = wrappers.get(index_in_run) {
                    for config in configs {
                        let wrapped = dense.suite_inputs(&self.root, config, &package_ids, &index, &mut paths, &mut memo)?;
                        inputs.ids.extend(wrapped.ids);
                        inputs.pinned |= wrapped.pinned;
                    }
                    inputs.ids.extend(absent.iter().map(|path| paths.intern(path)));
                    inputs.ids.extend_from_slice(&asset_ids);
                }
                Ok(inputs)
            })
            .collect();
        self.stamps.stamp(paths.paths.clone())?;
        self.follow_data_refs(&mut inputs, &index, &package_ids, &mut paths, &mut memo)?;
        let rank = paths.byte_order_ranks();

        // One leaf per input: its portable spelling and digest, hashed once
        // and shared by every suite that reaches it. An absent candidate
        // has no leaf: every run derives the same candidate set, so creating
        // the file still changes the key, while checkouts at different
        // depths (whose ancestor `analysis_options.yaml` candidates differ
        // in count) still key alike.
        let leaves: Vec<Option<[u8; 32]>> = paths
            .paths
            .par_iter()
            .map(|path| {
                let entry = self.stamps.get(path)?;
                let mut leaf = blake3::Hasher::new();
                hash_path(&mut leaf, &portable(&self.root, path));
                leaf.update(&entry.digest);
                Some(*leaf.finalize().as_bytes())
            })
            .collect();
        let root = &self.root;
        let env_digest = LazyDigest::new();
        let keys: Vec<Result<String, Refusal>> = inputs
            .into_par_iter()
            .zip(suites.par_iter())
            .map(|(inputs, suite)| {
                let SuiteInputs { mut ids, pinned } = inputs?;
                ids.sort_unstable_by_key(|&id| rank[id as usize]);
                ids.dedup();
                // The environment's suite half: the names this suite's
                // closure reads by literal key, each keyed with its
                // current value. An unnamable environment read's outcome
                // depends on the reading file's tier: the suite's own
                // package refuses (any variable could reach a verdict),
                // a dependency package keys the whole current environment
                // instead, and the toolchain tier stays exempt — the
                // tester binding aliasing the whole `Platform.environment`
                // map as its default is the tool's plumbing (the tool
                // builds the tester's environment; no dartq-side value
                // names it), not a suite author's read. Without the
                // toolchain exemption every flutter suite on earth
                // refuses, `flutter_test`'s `_binding_io.dart` in the
                // closure.
                let mut env_names: Vec<&str> = Vec::new();
                let mut env_opaque_own = false;
                let mut env_opaque_dep = false;
                for &id in &ids {
                    let Some((file_facts, tier)) = dense.env_facts(id) else {
                        continue;
                    };
                    match tier {
                        Tier::Toolchain => {}
                        Tier::Own => env_opaque_own |= file_facts.env_opaque,
                        Tier::Dependency => env_opaque_dep |= file_facts.env_opaque,
                    }
                    env_names.extend(file_facts.env_reads.iter().map(String::as_str));
                }
                if env_opaque_own {
                    return Err(Refusal::EnvOpaque);
                }
                env_names.sort_unstable();
                env_names.dedup();
                let mut hasher = blake3::Hasher::new();
                hasher.update(base.as_bytes());
                hasher.update(&[u8::from(pinned)]);
                if pinned {
                    hash_path(&mut hasher, root);
                }
                hash_path(&mut hasher, &portable(root, suite));
                for id in ids {
                    if let Some(leaf) = &leaves[id as usize] {
                        hasher.update(leaf);
                    }
                }
                hasher.update(&(env_names.len() as u64).to_le_bytes());
                for name in env_names {
                    hasher.update(&(name.len() as u64).to_le_bytes());
                    hasher.update(name.as_bytes());
                    let value = std::env::var_os(name);
                    hasher.update(&[u8::from(value.is_some())]);
                    if let Some(value) = value {
                        let bytes = value.as_encoded_bytes();
                        hasher.update(&(bytes.len() as u64).to_le_bytes());
                        hasher.update(bytes);
                    }
                }
                // A dependency package's whole-map read can observe any
                // variable: fold the whole current environment's digest
                // into the key. An environment change re-keys the suite
                // and it runs again; an identical environment replays —
                // the read can observe nothing the key does not cover.
                if env_opaque_dep {
                    hasher.update(&env_digest.get());
                }
                Ok(hasher.finalize().to_hex().to_string())
            })
            .collect();
        self.refusals = keys.iter().map(|key| key.as_ref().err().copied()).collect();
        Ok(keys.into_iter().map(Result::ok).collect())
    }

    /// Why each suite of the last [`Self::keys`]/[`Self::rekey`] call had
    /// no key (`None` where it did), in that call's suite order. Empty
    /// before the first call.
    #[must_use]
    pub fn refusals(&self) -> &[Option<Refusal>] {
        &self.refusals
    }
}

/// Paths interned to dense `u32` ids, so per-suite input sets are integer
/// vectors instead of trees of path comparisons.
#[derive(Default)]
struct Interner {
    paths: Vec<PathBuf>,
    ids: FxHashMap<PathBuf, u32>,
}

impl Interner {
    fn intern(&mut self, path: &Path) -> u32 {
        if let Some(&id) = self.ids.get(path) {
            return id;
        }
        let id = u32::try_from(self.paths.len()).expect("interned path count fits u32");
        self.paths.push(path.to_path_buf());
        self.ids.insert(path.to_path_buf(), id);
        id
    }

    /// Each id's position in byte order of its path: the canonical input
    /// order a key hashes in, independent of discovery order.
    fn byte_order_ranks(&self) -> Vec<u32> {
        let mut order: Vec<u32> = (0..self.paths.len() as u32).collect();
        order.sort_unstable_by(|&a, &b| {
            let a = self.paths[a as usize].as_os_str().as_encoded_bytes();
            let b = self.paths[b as usize].as_os_str().as_encoded_bytes();
            a.cmp(b)
        });
        let mut rank = vec![0u32; order.len()];
        for (position, id) in order.into_iter().enumerate() {
            rank[id as usize] = position as u32;
        }
        rank
    }
}

/// `path` as a key spells it: lexically relative to the package root
/// (climbing with `..` for files outside it), so the same layout keys the
/// same in every checkout. The file's digest is hashed next to it, so a
/// relative spelling that reaches different bytes keys differently.
fn portable(root: &Path, path: &Path) -> PathBuf {
    use std::path::Component;
    let from: Vec<Component<'_>> = root.components().collect();
    let to: Vec<Component<'_>> = path.components().collect();
    let common = from.iter().zip(&to).take_while(|(a, b)| a == b).count();
    if common == 0 {
        return path.to_path_buf();
    }
    let mut out = PathBuf::new();
    for _ in common..from.len() {
        out.push("..");
    }
    out.extend(&to[common..]);
    out
}

/// Feed one path into a key, length-prefixed so adjacent paths cannot run
/// together.
fn hash_path(hasher: &mut blake3::Hasher, path: &Path) {
    let bytes = path.as_os_str().as_encoded_bytes();
    hasher.update(&(bytes.len() as u64).to_le_bytes());
    hasher.update(bytes);
}

/// One import-graph node.
#[derive(Debug)]
enum Node {
    /// A readable file (or an absent one: no facts, no edges).
    File {
        deps: Vec<PathBuf>,
        facts: Option<Arc<FileFacts>>,
    },
    /// The file's edges cannot be established: an opaque header or a URI
    /// the resolution cannot map.
    Untrusted,
}

impl Cache {
    /// The key prefix every suite shares: format, runner, environment.
    fn base_key(&mut self, runner: Runner<'_>) -> io::Result<String> {
        let mut hasher = blake3::Hasher::new();
        hasher.update(FORMAT.as_bytes());
        hasher.update(&[u8::from(runner.flutter)]);
        let bin = runner.bin.canonicalize().unwrap_or_else(|_| runner.bin.to_path_buf());
        let mut identity = toolchain_identity(&bin);
        identity.insert(0, bin);
        self.stamps.stamp(identity.clone())?;
        for path in &identity {
            hash_path(&mut hasher, path);
            match self.stamps.get(path) {
                Some(entry) => hasher.update(&[1]).update(&entry.digest),
                None => hasher.update(&[0]),
            };
        }
        for arg in runner.args {
            hasher.update(&(arg.len() as u64).to_le_bytes());
            hasher.update(arg.as_bytes());
        }
        // The environment's toolchain half: `DART_*`/`FLUTTER_*` variables
        // change how the runner itself behaves (the VM's and flutter
        // tool's knobs), so they stay keyed for every suite. Everything
        // else joins only through the names a suite's closure reads
        // (per-suite, in [`Self::keys`]) — an IDE terminal's ambient
        // variables (`TERM_PROGRAM`, `VSCODE_*`, …) no longer invalidate
        // replay.
        let ignored = env_ignore_patterns();
        let mut env: Vec<(std::ffi::OsString, std::ffi::OsString)> = std::env::vars_os()
            .filter(|(name, _)| {
                let name = name.to_string_lossy();
                (name.starts_with("DART_") || name.starts_with("FLUTTER_")) && !env_ignored(name.as_ref(), &ignored)
            })
            .collect();
        env.sort_unstable();
        for (name, value) in env {
            for part in [name, value] {
                let bytes = part.as_encoded_bytes();
                hasher.update(&(bytes.len() as u64).to_le_bytes());
                hasher.update(bytes);
            }
        }
        Ok(hasher.finalize().to_hex().to_string())
    }
}

/// The `IGNORE_ENV_VAR` opt-out, parsed into name/prefix patterns.
fn env_ignore_patterns() -> Vec<String> {
    std::env::var(IGNORE_ENV_VAR)
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
        .collect()
}

/// A variable every key ignores: a shell/terminal volatile
/// ([`VOLATILE_ENV`]) or one the operator opted out of through
/// `IGNORE_ENV_VAR` (a trailing `*` matches a prefix).
fn env_ignored(name: &str, ignored: &[String]) -> bool {
    VOLATILE_ENV.contains(&name)
        || ignored.iter().any(|pattern| match pattern.strip_suffix('*') {
            Some(prefix) => name.starts_with(prefix),
            None => name == *pattern,
        })
}

/// The environment digest shared across a `keys` call, computed on first
/// use (most runs have no dependency-tier opaque reader and never pay the
/// `vars_os` scan) and `Sync` for the parallel per-suite hashers.
struct LazyDigest(std::sync::OnceLock<[u8; 32]>);

impl LazyDigest {
    fn new() -> Self {
        Self(std::sync::OnceLock::new())
    }

    fn get(&self) -> [u8; 32] {
        *self.0.get_or_init(environment_digest)
    }
}

/// Which package tier a closure file's environment reads belong to.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Tier {
    /// The suite's own package and its path dependencies: unnamable reads
    /// refuse caching.
    Own,
    /// Any other reachable package (hosted, git, sdk, or unattributed):
    /// unnamable reads key the whole environment digest.
    Dependency,
    /// The Flutter SDK's packages: unnamable reads are tool plumbing,
    /// exempt.
    Toolchain,
}

/// A digest of the whole current process environment (name+value pairs,
/// sorted), minus the variables every key already ignores. A suite whose
/// closure holds an unnamable environment read inside a *dependency*
/// package folds this into its key: any environment change re-keys the
/// suite and it runs again, while an identical environment replays — the
/// read can observe nothing the key does not cover. (The toolchain tier's
/// plumbing stays exempt without this digest, and the suite's own package
/// keeps refusing outright: a whole-map read authored in the workspace is
/// the suite's own to fix.)
fn environment_digest() -> [u8; 32] {
    let ignored = env_ignore_patterns();
    let mut env: Vec<(std::ffi::OsString, std::ffi::OsString)> = std::env::vars_os()
        .filter(|(name, _)| !env_ignored(name.to_string_lossy().as_ref(), &ignored))
        .collect();
    env.sort_unstable();
    let mut hasher = blake3::Hasher::new();
    for (name, value) in env {
        for part in [name, value] {
            let bytes = part.as_encoded_bytes();
            hasher.update(&(bytes.len() as u64).to_le_bytes());
            hasher.update(bytes);
        }
    }
    *hasher.finalize().as_bytes()
}

impl Cache {
    /// Close every suite's inputs over the paths its data files name: a
    /// fixture manifest (`files: [test/resources/a.dart]`) or an
    /// `analysis_options.yaml` `include:` that a test reads and follows.
    /// Each named path widens the inputs exactly as a Dart path literal
    /// does; newly reached files are stamped and followed in turn, so a
    /// chain of manifests closes. Terminates: each round only adds files of
    /// the finite package (or absent candidates, which name nothing).
    fn follow_data_refs(
        &mut self,
        inputs: &mut [Result<SuiteInputs, Refusal>],
        index: &PathIndex,
        package: &[u32],
        paths: &mut Interner,
        memo: &mut FxHashMap<String, Vec<u32>>,
    ) -> io::Result<()> {
        // The package-level manifests every key already covers name the
        // package's layout (`lib/`, `../`, SDK constraints), not fixtures:
        // following them would widen every suite to the whole package.
        let manifests: FxHashSet<u32> = PACKAGE_INPUTS
            .iter()
            .map(|rel| self.root.join(rel))
            .chain(self.root.ancestors().skip(1).map(|dir| dir.join("analysis_options.yaml")))
            .map(|path| paths.intern(&path))
            .collect();
        let mut named_by: FxHashMap<u32, Vec<u32>> = FxHashMap::default();
        // Per-suite visited marks as generation stamps over the interned ids
        // (grown as new candidates are interned), so a closure walk never
        // hashes a path.
        let mut mark: Vec<u32> = Vec::new();
        for (generation, suite) in inputs.iter_mut().filter_map(|inputs| inputs.as_mut().ok()).enumerate() {
            let generation = u32::try_from(generation + 1).expect("suite count fits u32");
            mark.resize(paths.paths.len(), 0);
            let mut frontier: Vec<u32> = Vec::with_capacity(suite.ids.len());
            for &id in &suite.ids {
                if mark[id as usize] != generation {
                    mark[id as usize] = generation;
                    if !manifests.contains(&id) {
                        frontier.push(id);
                    }
                }
            }
            while !frontier.is_empty() {
                let mut next = Vec::new();
                for id in frontier {
                    if !named_by.contains_key(&id) {
                        let refs = self.stamps.get(&paths.paths[id as usize]).and_then(|entry| entry.refs.clone());
                        let mut named = Vec::new();
                        for literal in refs.iter().flat_map(|refs| refs.iter()) {
                            if !memo.contains_key(literal) {
                                let ids = literal_inputs(&self.root, literal, package, index, paths);
                                memo.insert(literal.clone(), ids);
                            }
                            named.extend_from_slice(&memo[literal]);
                        }
                        named_by.insert(id, named);
                    }
                    mark.resize(paths.paths.len(), 0);
                    for &named in &named_by[&id] {
                        if mark[named as usize] != generation {
                            mark[named as usize] = generation;
                            next.push(named);
                        }
                    }
                }
                if !next.is_empty() {
                    let fresh: Vec<PathBuf> = next
                        .iter()
                        .map(|&id| paths.paths[id as usize].clone())
                        .filter(|path| !self.stamps.contains(path))
                        .collect();
                    self.stamps.stamp(fresh)?;
                    suite.ids.extend_from_slice(&next);
                }
                frontier = next;
            }
        }
        Ok(())
    }

    /// Breadth-first import graph from every suite, stamping one frontier
    /// at a time (each wave stats/reads in parallel). Files under the
    /// `immutable` roots are leaves never entered: hosted packages are
    /// immutable per version (pinned by `package_config.json`), the Flutter
    /// SDK's per SDK (pinned by its version stamps); every key covers both.
    fn import_graph(
        &mut self,
        suites: &[PathBuf],
        lib_dirs: &FxHashMap<&str, &Path>,
        immutable: &[PathBuf],
    ) -> io::Result<FxHashMap<PathBuf, Node>> {
        let in_cache = |path: &Path| immutable.iter().any(|root| path.starts_with(root));
        let mut graph: FxHashMap<PathBuf, Node> = FxHashMap::default();
        let mut frontier: Vec<PathBuf> = suites.to_vec();
        let mut queued: FxHashSet<PathBuf> = frontier.iter().cloned().collect();
        while !frontier.is_empty() {
            self.stamps.stamp(frontier.clone())?;
            let mut next = Vec::new();
            for path in frontier.drain(..) {
                let facts = self.stamps.get(&path).and_then(|entry| entry.facts.clone());
                let node = match &facts {
                    Some(facts) if facts.opaque => Node::Untrusted,
                    Some(facts) => {
                        // Outer `None` (untrusted URI) poisons the file;
                        // inner `None` (`dart:`) contributes no edge.
                        let resolved: Option<Vec<PathBuf>> = facts
                            .uris
                            .iter()
                            .map(|uri| resolve_uri(&path, uri, lib_dirs))
                            .collect::<Option<Vec<Option<PathBuf>>>>()
                            .map(|deps| deps.into_iter().flatten().collect());
                        match resolved {
                            Some(mut deps) => {
                                deps.retain(|dep| !in_cache(dep));
                                for dep in &deps {
                                    if queued.insert(dep.clone()) {
                                        next.push(dep.clone());
                                    }
                                }
                                Node::File { deps, facts: Some(facts.clone()) }
                            }
                            None => Node::Untrusted,
                        }
                    }
                    None => Node::File { deps: Vec::new(), facts: None },
                };
                graph.insert(path, node);
            }
            frontier = next;
        }
        Ok(graph)
    }
}

/// The files that pin the toolchain's version next to its entry point:
/// the Dart SDK `version`/`revision` beside a `dart` binary or inside a
/// Flutter SDK's cache, and Flutter's own version stamp. Empty when the
/// SDK layout is unknown — the binary's bytes and path still key it.
fn toolchain_identity(bin: &Path) -> Vec<PathBuf> {
    let Some(bin_dir) = bin.parent() else {
        return Vec::new();
    };
    let mut sdks = vec![bin_dir.join("cache").join("dart-sdk")];
    if let Some(sdk) = bin_dir.parent() {
        sdks.push(sdk.to_path_buf());
    }
    let mut out = Vec::new();
    for sdk in sdks {
        for name in ["version", "revision"] {
            let path = sdk.join(name);
            if path.is_file() {
                out.push(path);
            }
        }
    }
    // The Flutter SDK's framework and engine version stamps.
    for name in ["flutter.version.json", "engine.stamp", "engine.realm"] {
        let path = bin_dir.join("cache").join(name);
        if path.is_file() {
            out.push(path);
        }
    }
    out
}

/// Resolve one directive URI of `from`: `Ok(None)` for `dart:` libraries
/// (the toolchain key covers the SDK), `None` (outer) when the URI is
/// outside what the cache can map.
pub(crate) fn resolve_uri(from: &Path, uri: &str, lib_dirs: &FxHashMap<&str, &Path>) -> Option<Option<PathBuf>> {
    if uri.starts_with("dart:") {
        return Some(None);
    }
    if let Some(rest) = uri.strip_prefix("package:") {
        let (name, path) = rest.split_once('/')?;
        let lib = lib_dirs.get(name)?;
        return Some(Some(dartq_source::normalize_lexical(&lib.join(path))));
    }
    // Any other scheme (`file:`, `data:`, `http:`) or a percent escape is
    // a URI shape the cache does not interpret.
    if uri.contains(':') || uri.contains('%') || uri.is_empty() {
        return None;
    }
    let dir = from.parent()?;
    Some(Some(dartq_source::normalize_lexical(&dir.join(uri))))
}

/// Package files by every contiguous run of their path components: a
/// path literal `a/b` names every file whose relative path contains the
/// components `a`, `b` in a row — the file itself (`…/a/b`), or a
/// directory's subtree (`a/b/…`), wherever a base path was joined in
/// front of it at run time.
struct PathIndex {
    runs: FxHashMap<String, Vec<u32>>,
}

/// Deepest component run indexed; longer literals match on their tail.
const MAX_RUN: usize = 12;

impl PathIndex {
    fn new(root: &Path, package: &[PathBuf]) -> Self {
        let mut runs: FxHashMap<String, Vec<u32>> = FxHashMap::default();
        for (id, path) in package.iter().enumerate() {
            let Ok(rel) = path.strip_prefix(root) else {
                continue;
            };
            let parts: Vec<&str> = rel.iter().filter_map(|part| part.to_str()).collect();
            let id = u32::try_from(id).expect("package file count fits u32");
            for start in 0..parts.len() {
                for end in start + 1..=parts.len().min(start + MAX_RUN) {
                    let ids = runs.entry(parts[start..end].join("/")).or_default();
                    if ids.last() != Some(&id) {
                        ids.push(id);
                    }
                }
            }
        }
        Self { runs }
    }

    fn get(&self, run: &str) -> &[u32] {
        self.runs.get(run).map_or(&[], Vec::as_slice)
    }
}

/// Why a suite has no key — the one refusal the run reports per suite in
/// `--stats`. Ordered by where the closure walk meets it, not by severity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Refusal {
    /// The package resolution itself is unusable: no
    /// `.dart_tool/package_config.json`, an unparseable or unresolvable
    /// one. Refuses every suite of the run at once.
    Resolution,
    /// The suite's own file never entered the import graph (a test file
    /// the resolution cannot place is still reported, just never keyed).
    SuiteUnknown,
    /// A closure file's directive header could not be read with certainty
    /// (an interpolated/escaped URI, an unterminated directive) or names a
    /// URI outside what the cache can map.
    Untrusted,
    /// A closure file carries `// dartq: no-cache`.
    NoCache,
    /// A closure file reads drifting state (the wall clock, the network,
    /// child processes, standard input, the host name, native code) with
    /// no `// dartq: deterministic` attestation.
    Nondeterministic,
    /// A closure file spells an absolute path naming an existing
    /// directory, whose listing no key covers.
    AbsoluteDirectory,
    /// A file of the suite's own package (or its path dependencies) reads
    /// the environment through a shape no literal name covers — iterating
    /// `Platform.environment`, a computed key, an alias of the map.
    EnvOpaque,
}

impl Refusal {
    /// The stable label `--stats` prints.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Resolution => "resolution",
            Self::SuiteUnknown => "unknown-suite",
            Self::Untrusted => "untrusted",
            Self::NoCache => "no-cache",
            Self::Nondeterministic => "nondeterministic",
            Self::AbsoluteDirectory => "absolute-directory",
            Self::EnvOpaque => "environment",
        }
    }
}

/// Everything one suite's outcome can depend on.
struct SuiteInputs {
    /// Interned input paths, in discovery order, possibly repeated.
    ids: Vec<u32>,
    /// The outcome may differ between checkouts of the same content.
    pinned: bool,
}

/// The import graph over dense ids: each file is interned once and its
/// edges and facts are read once, so a suite's closure walk touches only
/// integers and bit sets, never path comparisons.
struct Dense {
    /// Graph node per interned path id (`u32::MAX` = not a node).
    node_of: Vec<u32>,
    /// Interned path id of each node.
    path_of: Vec<u32>,
    deps: Vec<Vec<u32>>,
    facts: Vec<Option<Arc<FileFacts>>>,
    /// `None` = untrusted (opaque header, unmappable URI).
    trusted: Vec<bool>,
    /// The node is library code of the package (`lib/`).
    in_lib: Vec<bool>,
    /// The node's package tier for the environment rules.
    tier: Vec<Tier>,
    /// The package-level inputs every suite carries.
    base_inputs: Vec<u32>,
}

impl Dense {
    fn new(
        root: &Path,
        graph: &FxHashMap<PathBuf, Node>,
        toolchain: &[PathBuf],
        own: &[PathBuf],
        paths: &mut Interner,
    ) -> Self {
        let lib = root.join("lib");
        let mut ordered: Vec<(&PathBuf, &Node)> = graph.iter().collect();
        ordered.sort_unstable_by(|a, b| a.0.cmp(b.0));
        let path_of: Vec<u32> = ordered.iter().map(|(path, _)| paths.intern(path)).collect();
        let mut node_of = vec![u32::MAX; paths.paths.len()];
        for (node, &id) in path_of.iter().enumerate() {
            node_of[id as usize] = u32::try_from(node).expect("graph size fits u32");
        }
        let node_id = |path: &Path, paths: &Interner| {
            paths.ids.get(path).and_then(|&id| node_of.get(id as usize).copied()).filter(|&n| n != u32::MAX)
        };
        let mut deps = Vec::with_capacity(ordered.len());
        let mut facts = Vec::with_capacity(ordered.len());
        let mut trusted = Vec::with_capacity(ordered.len());
        let mut in_lib = Vec::with_capacity(ordered.len());
        let mut tier = Vec::with_capacity(ordered.len());
        for (path, node) in &ordered {
            in_lib.push(path.starts_with(&lib));
            // The toolchain wins over the workspace: a path dependency
            // pointed inside the Flutter SDK is still the tool's own code.
            tier.push(if toolchain.iter().any(|dir| path.starts_with(dir)) {
                Tier::Toolchain
            } else if path.starts_with(root) || own.iter().any(|dir| path.starts_with(dir)) {
                Tier::Own
            } else {
                Tier::Dependency
            });
            match node {
                Node::File { deps: edges, facts: file_facts } => {
                    // Every edge target was queued into the graph.
                    deps.push(edges.iter().filter_map(|dep| node_id(dep, paths)).collect());
                    facts.push(file_facts.clone());
                    trusted.push(true);
                }
                Node::Untrusted => {
                    deps.push(Vec::new());
                    facts.push(None);
                    trusted.push(false);
                }
            }
        }
        let mut base_inputs: Vec<u32> = PACKAGE_INPUTS.iter().map(|rel| paths.intern(&root.join(rel))).collect();
        // A pub workspace member resolves through the workspace root's
        // configuration and lock.
        let workspace = workspace_root(root);
        if workspace.as_deref() != Some(root) {
            if let Some(workspace) = &workspace {
                for rel in ["pubspec.yaml", "pubspec.lock", ".dart_tool/package_config.json"] {
                    base_inputs.push(paths.intern(&workspace.join(rel)));
                }
            }
            base_inputs.push(paths.intern(&root.join(".dart_tool/pub/workspace_ref.json")));
        }
        // The analyzer's options discovery walks up from the package.
        for dir in root.ancestors().skip(1) {
            base_inputs.push(paths.intern(&dir.join("analysis_options.yaml")));
        }
        Self {
            node_of,
            path_of,
            deps,
            facts,
            trusted,
            in_lib,
            tier,
            base_inputs,
        }
    }

    /// The Dart facts of the interned path `id` and the package tier its
    /// environment reads belong to, when it is a graph node whose facts
    /// were read (`None` for package inputs, absent candidates, and
    /// untrusted headers).
    fn env_facts(&self, id: u32) -> Option<(&FileFacts, Tier)> {
        let node = self.node_of.get(id as usize).copied().filter(|&n| n != u32::MAX)?;
        self.facts[node as usize].as_deref().map(|facts| (facts, self.tier[node as usize]))
    }

    /// The inputs of one suite, or the [`Refusal`] its import closure met
    /// first (a stack walk's order is deterministic, so two runs of the
    /// same closure name the same reason).
    fn suite_inputs(
        &self,
        root: &Path,
        suite: &Path,
        package: &[u32],
        index: &PathIndex,
        paths: &mut Interner,
        memo: &mut FxHashMap<String, Vec<u32>>,
    ) -> Result<SuiteInputs, Refusal> {
        let start = *self.node_of.get(*paths.ids.get(suite).ok_or(Refusal::SuiteUnknown)? as usize)
            .ok_or(Refusal::SuiteUnknown)?;
        if start == u32::MAX {
            return Err(Refusal::SuiteUnknown);
        }
        let mut inputs = self.base_inputs.clone();
        let mut seen = vec![false; self.path_of.len()];
        let mut stack = vec![start];
        let mut whole_package = false;
        let mut pinned = false;
        while let Some(node) = stack.pop() {
            let node = node as usize;
            if std::mem::replace(&mut seen[node], true) {
                continue;
            }
            if !self.trusted[node] {
                return Err(Refusal::Untrusted);
            }
            inputs.push(self.path_of[node]);
            stack.extend(self.deps[node].iter().copied().filter(|&dep| !seen[dep as usize]));
            let Some(facts) = &self.facts[node] else {
                continue;
            };
            if self.tier[node] == Tier::Toolchain {
                continue;
            }
            if facts.no_cache {
                return Err(Refusal::NoCache);
            }
            if facts.nondeterministic && !facts.attested {
                return Err(Refusal::Nondeterministic);
            }
            for absolute in &facts.absolute_paths {
                let path = Path::new(absolute);
                if path.is_dir() {
                    return Err(Refusal::AbsoluteDirectory);
                }
                inputs.push(paths.intern(path));
            }
            whole_package |= facts.ambient;
            // Library code reads the paths its callers hand it; its own
            // string literals are separators, names and messages (`'.'`,
            // `'lib'`), not package reads, and its absolute-path derivations
            // resolve those caller paths against the checkout the same way
            // in every checkout. The suite side (tests and their helpers) is
            // where fixture paths are spelled and where a location can reach
            // an assertion.
            if self.in_lib[node] {
                continue;
            }
            pinned |= facts.located && !facts.portable;
            for literal in &facts.path_literals {
                if !memo.contains_key(literal) {
                    let named = literal_inputs(root, literal, package, index, paths);
                    memo.insert(literal.clone(), named);
                }
                let named = &memo[literal];
                // Empty only for `.`: the package root itself.
                whole_package |= named.is_empty();
                inputs.extend_from_slice(named);
            }
        }
        if whole_package {
            inputs.extend_from_slice(package);
        }
        Ok(SuiteInputs { ids: inputs, pinned })
    }
}

/// The files one path literal can name: its root-relative spelling (an
/// absent file counts — creating it changes what the test sees) plus
/// every package file whose path contains the literal's components.
/// Empty exactly for the package root (`.`).
fn literal_inputs(
    root: &Path,
    literal: &str,
    package: &[u32],
    index: &PathIndex,
    paths: &mut Interner,
) -> Vec<u32> {
    let parts: Vec<&str> = literal
        .split('/')
        .filter(|part| !part.is_empty() && *part != "." && *part != "..")
        .collect();
    if parts.is_empty() {
        return Vec::new();
    }
    let tail = &parts[parts.len().saturating_sub(MAX_RUN)..];
    let mut out: Vec<u32> = index
        .get(&tail.join("/"))
        .iter()
        .map(|&position| package[position as usize])
        .collect();
    if !literal.starts_with("..") {
        out.push(paths.intern(&root.join(parts.join("/"))));
    }
    out
}

impl Cache {
    /// Re-key `suites` against the disk as it is now. A suite whose inputs
    /// changed while it ran keeps the outcome of neither version, so the
    /// caller stores a result only when its pre-run and post-run keys agree.
    ///
    /// # Errors
    ///
    /// I/O failures reading the package tree.
    pub fn rekey(&mut self, suites: &[PathBuf], runner: Runner<'_>) -> io::Result<Vec<Option<String>>> {
        self.stamps.refresh();
        self.keys(suites, runner)
    }

    /// The stored rows under `key` (which names its suite), if any.
    ///
    /// # Errors
    ///
    /// None: an unreadable or foreign entry is a miss.
    #[must_use]
    pub fn lookup(&self, key: &str) -> Option<Vec<crate::event::Json>> {
        let path = self.entry_path(key);
        let mut stored = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Stored>(&bytes).ok())
            .filter(|stored| stored.format == FORMAT)?;
        touch(&path);
        let (root, uri) = self.spellings();
        for event in &mut stored.events {
            crate::event::replace_all(event, &[(ROOT_URI_MARK, &uri), (ROOT_MARK, &root)]);
        }
        Some(stored.events)
    }

    /// The package root as events spell it, each with its trailing
    /// separator (so `/a/` never matches inside `/ab/`): a path, and a
    /// `file:` URI.
    fn spellings(&self) -> (String, String) {
        let root = self.root.to_string_lossy().into_owned();
        let uri = format!("file://{}/", root.split('/').map(encode_uri_segment).collect::<Vec<_>>().join("/"));
        (format!("{root}/"), uri)
    }

    /// Write a green suite's events under `key` into this process's
    /// staging area, holding nothing in memory. The entry becomes visible
    /// to lookups only through [`Self::commit`].
    ///
    /// # Errors
    ///
    /// I/O failures writing the staged entry.
    pub fn stage(&mut self, key: &str, events: &[crate::event::Json]) -> io::Result<()> {
        #[derive(Serialize)]
        struct Staged<'a> {
            format: &'a str,
            events: &'a [crate::event::Json],
        }
        // Entries are shared across checkouts of the same content: the
        // checkout's own root becomes a mark the replaying checkout fills
        // with its root. A suite whose verdict could see the root itself
        // is keyed on it (pinned), so the mark never hides a difference.
        let (root, uri) = self.spellings();
        let events: Vec<crate::event::Json> = events
            .iter()
            .map(|event| {
                let mut event = event.clone();
                crate::event::replace_all(&mut event, &[(&uri, ROOT_URI_MARK), (&root, ROOT_MARK)]);
                event
            })
            .collect();
        let bytes = serde_json::to_vec(&Staged { format: FORMAT, events: &events }).map_err(io::Error::other)?;
        write_atomic(&self.staging.join(format!("{key}.json")), &bytes)?;
        self.staged.insert(key.to_owned());
        Ok(())
    }

    /// Publish the staged entries whose keys are in `verified` and discard
    /// every other staged entry.
    ///
    /// # Errors
    ///
    /// I/O failures moving an entry into the store.
    pub fn commit(&mut self, verified: &FxHashSet<String>) -> io::Result<()> {
        for key in std::mem::take(&mut self.staged) {
            let staged = self.staging.join(format!("{key}.json"));
            if verified.contains(&key) {
                let target = self.entry_path(&key);
                if let Some(dir) = target.parent() {
                    std::fs::create_dir_all(dir)?;
                }
                std::fs::rename(&staged, &target)?;
            } else {
                let _ = std::fs::remove_file(&staged);
            }
        }
        let _ = std::fs::remove_dir(&self.staging);
        Ok(())
    }

    /// The keys staged so far.
    #[must_use]
    pub fn staged(&self) -> &FxHashSet<String> {
        &self.staged
    }

    /// The facts stamped for `path` (the same pass that keyed this run
    /// stamped every suite and its closure), so a caller need not read and
    /// scan the file again.
    #[must_use]
    pub fn facts(&self, path: &Path) -> Option<std::sync::Arc<FileFacts>> {
        self.stamps.get(path).and_then(|entry| entry.facts.clone())
    }

    /// Persist the stamp index, then prune the shared store's least
    /// recently used entries when it outgrew its bound (at most once per
    /// [`PRUNE_EVERY`]).
    ///
    /// # Errors
    ///
    /// I/O failures writing the index or an entry.
    pub fn save(&mut self) -> io::Result<()> {
        self.stamps.save(&self.dir.join("index.json"))?;
        prune(&self.store, MAX_ENTRIES)
    }

    /// `<store>/<first two hex digits>/<key>.json`: 256 shards keep every
    /// directory small.
    fn entry_path(&self, key: &str) -> PathBuf {
        let shard = key.get(..2).unwrap_or("__");
        self.store.join(shard).join(format!("{key}.json"))
    }
}

impl Drop for Cache {
    /// A run that ends without committing (an error, a panic) leaves no
    /// staged entry behind.
    fn drop(&mut self) {
        if !self.staged.is_empty() {
            let _ = std::fs::remove_dir_all(&self.staging);
        }
    }
}

/// Mark an entry used now; the prune pass evicts by modification time.
fn touch(path: &Path) {
    if let Ok(file) = std::fs::File::options().append(true).open(path) {
        let _ = file.set_modified(std::time::SystemTime::now());
    }
}

/// Evict the least recently used entries of `store` beyond `bound`. A
/// `.pruned` marker spaces passes [`PRUNE_EVERY`] apart, so the directory
/// walk is not paid per run. Entries another process removes first are
/// simply gone.
fn prune(store: &Path, bound: usize) -> io::Result<()> {
    let marker = store.join(".pruned");
    let recent = std::fs::metadata(&marker)
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|at| at.elapsed().ok())
        .is_some_and(|age| age < PRUNE_EVERY);
    if recent {
        return Ok(());
    }
    std::fs::create_dir_all(store)?;
    std::fs::write(&marker, b"")?;
    let mut entries: Vec<(std::time::SystemTime, PathBuf)> = Vec::new();
    for shard in std::fs::read_dir(store)? {
        let shard = shard?.path();
        let hidden = shard.file_name().is_some_and(|name| name.to_string_lossy().starts_with('.'));
        if hidden || !shard.is_dir() {
            continue;
        }
        for entry in std::fs::read_dir(&shard)? {
            let path = entry?.path();
            if let Ok(modified) = std::fs::metadata(&path).and_then(|meta| meta.modified()) {
                entries.push((modified, path));
            }
        }
    }
    if entries.len() <= bound {
        return Ok(());
    }
    entries.sort_unstable();
    let excess = entries.len() - bound;
    for (_, path) in entries.into_iter().take(excess) {
        match std::fs::remove_file(&path) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error),
            _ => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A package with a lib, two suites, a fixture directory one suite
    /// reads by literal path, and a suite with an untrusted directive.
    fn fixture() -> (tempfile::TempDir, PathBuf) {
        let tmp = tempfile::tempdir().expect("tmp");
        let root = tmp.path().canonicalize().expect("canon");
        let write = |rel: &str, body: &str| {
            let path = root.join(rel);
            std::fs::create_dir_all(path.parent().expect("parent")).expect("dir");
            std::fs::write(path, body).expect("write");
        };
        write("pubspec.yaml", "name: fx\n");
        write(
            ".dart_tool/package_config.json",
            r#"{"configVersion":2,"packages":[{"name":"fx","rootUri":"../","packageUri":"lib/","languageVersion":"3.5"}]}"#,
        );
        write("lib/a.dart", "import 'b.dart';\nint a() => b();\n");
        write("lib/b.dart", "int b() => 1;\n");
        write("lib/other.dart", "int other() => 2;\n");
        write("test/a_test.dart", "import 'package:fx/a.dart';\nvoid main() {}\n");
        write("test/data_test.dart", "void main() { read('test/data'); }\n");
        write("test/data/input.txt", "one\n");
        write("test/bad_test.dart", "import 'x${y}.dart';\nvoid main() {}\n");
        (tmp, root)
    }

    fn runner(bin: &Path) -> Runner<'_> {
        Runner {
            bin,
            args: &[],
            flutter: false,
        }
    }

    #[test]
    fn keys_track_closures_and_literal_inputs() {
        let (tmp, root) = fixture();
        let store = tmp.path().join("store");
        let bin = root.join("pubspec.yaml");
        let suites = ["test/a_test.dart", "test/data_test.dart", "test/bad_test.dart"]
            .map(|rel| root.join(rel))
            .to_vec();
        let mut cache = Cache::with_store(&root, store.clone());
        let first = cache.keys(&suites, runner(&bin)).expect("keys");
        assert!(first[0].is_some() && first[1].is_some());
        assert!(first[2].is_none(), "an interpolated URI is untrusted");
        assert_eq!(cache.rekey(&suites, runner(&bin)).expect("keys"), first, "stable");

        // A file outside every closure changes nothing.
        std::fs::write(root.join("lib/other.dart"), "int other() => 3;\n").expect("write");
        assert_eq!(cache.rekey(&suites, runner(&bin)).expect("keys"), first);

        // A transitive import changes only the importing suite.
        std::fs::write(root.join("lib/b.dart"), "int b() => 2;\n").expect("write");
        let second = cache.rekey(&suites, runner(&bin)).expect("keys");
        assert_ne!(second[0], first[0]);
        assert_eq!(second[1], first[1]);

        // A file under a literal directory changes only its reader.
        std::fs::write(root.join("test/data/input.txt"), "two\n").expect("write");
        let third = cache.rekey(&suites, runner(&bin)).expect("keys");
        assert_eq!(third[0], second[0]);
        assert_ne!(third[1], second[1]);

        // Store, persist, reopen: the entry replays under the same key;
        // the previous revision's entry stays addressable too.
        let key = third[0].clone().expect("key");
        let old = first[0].clone().expect("key");
        cache.stage(&old, &[]).expect("stage");
        cache.stage(&key, &[]).expect("stage");
        cache.stage("unverified", &[]).expect("stage");
        let verified: FxHashSet<String> = [old.clone(), key.clone()].into_iter().collect();
        cache.commit(&verified).expect("commit");
        cache.save().expect("save");
        assert!(cache.lookup("unverified").is_none(), "an unverified entry is never published");
        let reopened = Cache::with_store(&root, store);
        assert!(reopened.lookup(&key).is_some());
        assert!(reopened.lookup(&old).is_some(), "a reverted edit replays");
        assert!(reopened.lookup("other").is_none());
    }

    /// Two checkouts of the same content key alike, so one checkout's
    /// green outcome replays in the other — except for a suite that
    /// observes its absolute location, which stays pinned to its checkout.
    #[test]
    fn keys_are_portable_across_checkouts() {
        let (tmp_a, root_a) = fixture();
        let (tmp_b, root_b) = fixture();
        let write = |root: &Path| {
            std::fs::write(
                root.join("test/where_test.dart"),
                "import 'dart:io';\nvoid main() { File('x').absolute; }\n",
            )
            .expect("write");
        };
        write(&root_a);
        write(&root_b);
        let shared = tempfile::tempdir().expect("tmp");
        let store = shared.path().join("store");
        // The runner binary is outside both checkouts, so it keys alike.
        let bin = shared.path().join("dart");
        std::fs::write(&bin, "bin").expect("bin");
        let suites = |root: &Path| {
            ["test/a_test.dart", "test/where_test.dart"].map(|rel| root.join(rel)).to_vec()
        };
        let mut a = Cache::with_store(&root_a, store.clone());
        let keys_a = a.keys(&suites(&root_a), runner(&bin)).expect("keys");
        let mut b = Cache::with_store(&root_b, store.clone());
        let keys_b = b.keys(&suites(&root_b), runner(&bin)).expect("keys");
        assert_eq!(keys_a[0], keys_b[0], "same content, same key");
        assert_ne!(keys_a[1], keys_b[1], "a location observer is pinned to its checkout");

        // A `// dartq: portable` attestation lifts the pin.
        for root in [&root_a, &root_b] {
            std::fs::write(
                root.join("test/where_test.dart"),
                "// dartq: portable\nimport 'dart:io';\nvoid main() { File('x').absolute; }\n",
            )
            .expect("write");
        }
        let attested_a = a.rekey(&suites(&root_a), runner(&bin)).expect("keys");
        let attested_b = b.rekey(&suites(&root_b), runner(&bin)).expect("keys");
        assert_eq!(attested_a[1], attested_b[1], "attested portable suites share");

        let shared_key = keys_a[0].clone().expect("key");
        a.stage(&shared_key, &[]).expect("stage");
        assert!(b.lookup(&shared_key).is_none(), "a staged entry is private");
        a.commit(&[shared_key].into_iter().collect()).expect("commit");
        a.save().expect("save");
        assert!(b.lookup(keys_b[0].as_deref().expect("key")).is_some(), "shared store hit");

        // Diverging content diverges the key.
        std::fs::write(root_b.join("lib/b.dart"), "int b() => 9;\n").expect("write");
        let moved = b.rekey(&suites(&root_b), runner(&bin)).expect("keys");
        assert_ne!(moved[0], keys_a[0]);
        drop((tmp_a, tmp_b));
    }

    /// The invalidation floor, end to end through the stamp index (each step
    /// re-keys against the disk as a fresh run would): a fixture under a
    /// literal directory being edited, deleted, recreated with the old
    /// bytes, or joined by a new sibling; a manifest-named file changing; a
    /// drifting or marked closure never keying at all.
    #[test]
    fn invalidation_covers_deletion_creation_and_refusals() {
        let (tmp, root) = fixture();
        let bin = root.join("pubspec.yaml");
        let write = |rel: &str, body: &str| {
            let path = root.join(rel);
            std::fs::create_dir_all(path.parent().expect("parent")).expect("dir");
            std::fs::write(path, body).expect("write");
        };
        write("test/manifest_test.dart", "void main() { read('test/manifest.yaml'); }\n");
        write("test/manifest.yaml", "fixtures:\n  - test/named/a.txt\n");
        write("test/named/a.txt", "a\n");
        write("test/clock_test.dart", "void main() { DateTime.now(); }\n");
        write("test/clock_attested_test.dart", "// dartq: deterministic\nvoid main() { DateTime.now(); }\n");
        write("test/marked_test.dart", "import 'helper.dart';\nvoid main() {}\n");
        write("test/helper.dart", "// dartq: no-cache shared server\nvoid help() {}\n");
        let outside = tmp.path().join("outside.txt");
        std::fs::write(&outside, "v1\n").expect("write");
        write("test/etc_test.dart", &format!("void main() {{ File('{}'); File('/dartq/mock/path.dart'); }}\n", outside.display()));
        write("test/dir_test.dart", &format!("void main() {{ Directory('{}'); }}\n", tmp.path().display()));
        let suites: Vec<PathBuf> = [
            "test/data_test.dart",
            "test/manifest_test.dart",
            "test/clock_test.dart",
            "test/clock_attested_test.dart",
            "test/marked_test.dart",
            "test/etc_test.dart",
            "test/dir_test.dart",
        ]
        .map(|rel| root.join(rel))
        .to_vec();
        let mut cache = Cache::with_store(&root, tmp.path().join("store"));
        let rekey = |cache: &mut Cache| cache.rekey(&suites, runner(&bin)).expect("keys");
        let first = cache.keys(&suites, runner(&bin)).expect("keys");
        assert!(first[0].is_some() && first[1].is_some() && first[3].is_some());
        assert_eq!(first[2], None, "a wall-clock read is never cached");
        assert_eq!(first[4], None, "a no-cache marker anywhere in the closure refuses");
        assert!(first[5].is_some(), "absolute file paths and mock paths are inputs");
        assert_eq!(first[6], None, "an absolute directory's listing is not keyed");

        // Deleting the fixture changes the reader's key, not the others'.
        std::fs::remove_file(root.join("test/data/input.txt")).expect("rm");
        let deleted = rekey(&mut cache);
        assert_ne!(deleted[0], first[0]);
        assert_eq!(deleted[1], first[1]);
        // Recreating it with the original bytes restores the original key:
        // the green outcome of that content replays again.
        write("test/data/input.txt", "one\n");
        assert_eq!(rekey(&mut cache)[0], first[0]);
        // A new sibling under the literal directory is a new input.
        write("test/data/extra.txt", "x\n");
        assert_ne!(rekey(&mut cache)[0], first[0]);

        // An absolute file outside the package is an input.
        std::fs::write(&outside, "v2\n").expect("write");
        assert_ne!(rekey(&mut cache)[5], first[5]);

        // A file the manifest names is an input of the manifest's reader.
        write("test/named/a.txt", "b\n");
        let named = rekey(&mut cache);
        assert_ne!(named[1], first[1]);
        std::fs::remove_file(root.join("test/named/a.txt")).expect("rm");
        assert_ne!(rekey(&mut cache)[1], named[1], "deleting a named file invalidates");
    }

    /// The prune pass keeps the most recently used entries.
    #[test]
    fn prune_evicts_least_recently_used() {
        let tmp = tempfile::tempdir().expect("tmp");
        let store = tmp.path().to_path_buf();
        let shard = store.join("ab");
        std::fs::create_dir_all(&shard).expect("dir");
        let now = std::time::SystemTime::now();
        for (index, age) in [30u64, 10, 20].into_iter().enumerate() {
            let path = shard.join(format!("ab{index}.json"));
            let file = std::fs::File::create(&path).expect("create");
            file.set_modified(now - std::time::Duration::from_secs(age)).expect("mtime");
        }
        prune(&store, 2).expect("prune");
        assert!(!shard.join("ab0.json").exists(), "the oldest is evicted");
        assert!(shard.join("ab1.json").exists() && shard.join("ab2.json").exists());
        // A second pass within the spacing window does nothing.
        std::fs::File::create(shard.join("ab3.json")).expect("create");
        prune(&store, 1).expect("prune");
        assert_eq!(std::fs::read_dir(&shard).expect("read").count(), 3);
    }

    #[test]
    fn portable_paths_are_root_relative() {
        let root = Path::new("/ws/pkg");
        assert_eq!(portable(root, Path::new("/ws/pkg/test/a.dart")), Path::new("test/a.dart"));
        assert_eq!(portable(root, Path::new("/ws/other/b.dart")), Path::new("../other/b.dart"));
        assert_eq!(portable(root, Path::new("/ws/analysis_options.yaml")), Path::new("../analysis_options.yaml"));
    }

    #[test]
    fn missing_package_config_disables_keys() {
        let (_tmp, root) = fixture();
        std::fs::remove_file(root.join(".dart_tool/package_config.json")).expect("rm");
        let bin = root.join("pubspec.yaml");
        let keys = Cache::with_store(&root, root.join("store"))
            .keys(&[root.join("test/a_test.dart")], runner(&bin))
            .expect("keys");
        assert_eq!(keys, [None]);
    }

    /// A stored entry spells the package root as a mark: the checkout that
    /// replays it sees its own root in the events, as a path and as a
    /// `file:` URI.
    #[test]
    fn replayed_entries_spell_the_replaying_root() {
        let (tmp_a, root_a) = fixture();
        let (tmp_b, root_b) = fixture();
        let shared = tempfile::tempdir().expect("tmp");
        let store = shared.path().join("store");
        // The runner binary is outside both checkouts, so it keys alike.
        let bin = shared.path().join("dart");
        std::fs::write(&bin, "bin").expect("bin");
        let suites = |root: &Path| vec![root.join("test/a_test.dart")];
        let mut a = Cache::with_store(&root_a, store.clone());
        let key_a = a.keys(&suites(&root_a), runner(&bin)).expect("keys");
        let mut b = Cache::with_store(&root_b, store.clone());
        let key_b = b.keys(&suites(&root_b), runner(&bin)).expect("keys");
        assert_eq!(key_a[0], key_b[0], "same content keys alike");
        let key = key_a[0].clone().expect("key");
        let events = |root: &Path| {
            [
                format!(r#"{{"suite":{{"id":0,"platform":"vm","path":"{}/test/a_test.dart"}},"type":"suite","time":0}}"#, root.display()),
                format!(r#"{{"type":"print","message":"loaded file://{}/test/a_test.dart"}}"#, root.display()),
            ]
            .map(|line| crate::event::parse(&line).expect("event"))
            .to_vec()
        };
        a.stage(&key, &events(&root_a)).expect("stage");
        let verified: FxHashSet<String> = [key.clone()].into_iter().collect();
        a.commit(&verified).expect("commit");

        let from_b = Cache::with_store(&root_b, store.clone()).lookup(&key).expect("hit");
        let text = from_b.iter().map(|event| event.to_line()).collect::<String>();
        assert!(text.contains(&format!("{}/test/a_test.dart", root_b.display())));
        assert!(text.contains(&format!("file://{}/test/a_test.dart", root_b.display())));
        let staged_root = root_a.to_string_lossy();
        assert!(!text.contains(staged_root.as_ref()), "the staging root never leaks");

        let from_a = Cache::with_store(&root_a, store).lookup(&key).expect("hit");
        let own = format!("{}/test/a_test.dart", root_a.display());
        assert!(from_a.iter().any(|event| event.str_at(&["suite", "path"]) == Some(own.as_str())));
        drop((tmp_a, tmp_b));
    }

    /// A pub workspace member resolves through the workspace root's
    /// package configuration; a broken pointer falls back to the member.
    #[test]
    fn workspace_members_resolve_through_the_workspace_root() {
        let tmp = tempfile::tempdir().expect("tmp");
        let write = |rel: &str, body: &str| {
            let path = tmp.path().join(rel);
            std::fs::create_dir_all(path.parent().expect("parent")).expect("dir");
            std::fs::write(&path, body).expect("write");
        };
        write("ws/.dart_tool/package_config.json", "{}");
        // The pointer resolves from its `.dart_tool/pub` directory.
        write("ws/pkg/a/.dart_tool/pub/workspace_ref.json", r#"{"workspaceRoot": "../../../.."}"#);
        let member = tmp.path().join("ws/pkg/a");
        assert_eq!(workspace_root(&member), Some(tmp.path().join("ws")));
        assert_eq!(package_config_path(&member), tmp.path().join("ws/.dart_tool/package_config.json"));
        // A package without a pointer is its own workspace.
        let plain = tmp.path().join("plain");
        std::fs::create_dir_all(&plain).expect("dir");
        assert_eq!(workspace_root(&plain), Some(plain.clone()));
        assert_eq!(package_config_path(&plain), plain.join(".dart_tool/package_config.json"));
        // An unreadable pointer names nothing; the member's own
        // configuration is the fallback.
        write("ws/pkg/b/.dart_tool/pub/workspace_ref.json", "not json");
        let broken = tmp.path().join("ws/pkg/b");
        assert_eq!(workspace_root(&broken), None);
        assert_eq!(package_config_path(&broken), broken.join(".dart_tool/package_config.json"));
    }

    /// `flutter test` wraps the nearest `flutter_test_config.dart` (every
    /// closer candidate's absence an input too) and bundles the pubspec's
    /// assets: both reach every suite's inputs.
    #[test]
    fn flutter_config_and_asset_inputs() {
        let tmp = tempfile::tempdir().expect("tmp");
        let root = tmp.path().canonicalize().expect("canon");
        let write = |rel: &str, body: &str| {
            let path = root.join(rel);
            std::fs::create_dir_all(path.parent().expect("parent")).expect("dir");
            std::fs::write(&path, body).expect("write");
        };
        write("pubspec.yaml", "name: fx\n");
        write("test/flutter_test_config.dart", "void main() {}\n");
        let suite = root.join("test/sub/a_test.dart");
        let (configs, absent) = flutter_inputs(&root, &suite);
        assert_eq!(configs, vec![root.join("test/flutter_test_config.dart")]);
        assert_eq!(absent, vec![root.join("test/sub/flutter_test_config.dart")]);
        // Without it the walk reaches the package root, every absent
        // candidate along the way an input.
        std::fs::remove_file(root.join("test/flutter_test_config.dart")).expect("rm");
        let (configs, absent) = flutter_inputs(&root, &suite);
        assert!(configs.is_empty());
        assert_eq!(
            absent,
            [
                root.join("test/sub/flutter_test_config.dart"),
                root.join("test/flutter_test_config.dart"),
                root.join("flutter_test_config.dart"),
            ]
        );
        // Assets: files, directory subtrees and font files.
        write(
            "pubspec.yaml",
            "name: fx\nflutter:\n  assets:\n    - assets/data.txt\n    - assets/dir/\n  fonts:\n    - family: X\n      fonts:\n        - asset: fonts/X.ttf\n",
        );
        write("assets/data.txt", "x");
        write("assets/dir/a.txt", "a");
        write("assets/dir/sub/b.txt", "b");
        write("fonts/X.ttf", "font");
        assert_eq!(
            flutter_assets(&root),
            [
                root.join("assets/data.txt"),
                root.join("assets/dir/a.txt"),
                root.join("assets/dir/sub/b.txt"),
                root.join("fonts/X.ttf"),
            ]
        );
        // Suite URLs percent-encode everything but the characters Dart's
        // `Uri` leaves alone.
        assert_eq!(encode_uri_segment("a b"), "a%20b");
        assert_eq!(encode_uri_segment("a+b'"), "a+b'");
        assert_eq!(encode_uri_segment("a/b"), "a%2Fb");
    }

    /// The key covers the runner invocation: arguments and the tool reach
    /// it.
    #[test]
    fn keys_cover_the_runner_arguments_and_tool() {
        let (tmp, root) = fixture();
        let bin = root.join("pubspec.yaml");
        let suites = vec![root.join("test/a_test.dart")];
        let mut cache = Cache::with_store(&root, tmp.path().join("store"));
        let plain = cache.keys(&suites, runner(&bin)).expect("keys");
        let named = cache
            .keys(&suites, Runner { bin: &bin, args: &["--name=x".to_owned()], flutter: false })
            .expect("keys");
        let flutter = cache.keys(&suites, Runner { bin: &bin, args: &[], flutter: true }).expect("keys");
        assert_ne!(plain[0], named[0], "arguments are inputs");
        assert_ne!(plain[0], flutter[0], "the tool is an input");
    }

    /// A run that ends without committing leaves no staged entry behind,
    /// and an entry of a foreign format never replays.
    #[test]
    fn uncommitted_staging_is_removed_on_drop() {
        let (tmp, root) = fixture();
        let store = tmp.path().join("store");
        let mut cache = Cache::with_store(&root, store.clone());
        cache.stage("deadbeef", &[]).expect("stage");
        assert!(cache.staging.is_dir());
        drop(cache);
        assert!(!store.join(format!(".staging-{}", std::process::id())).is_dir());
        std::fs::create_dir_all(store.join("ab")).expect("dir");
        std::fs::write(store.join("ab/abcd.json"), br#"{"format":"dartq-test-cache-v0","events":[]}"#).expect("write");
        assert!(Cache::with_store(&root, store).lookup("abcd").is_none(), "a foreign format never replays");
    }

    /// The environment enters a key only through the names the suite's
    /// closure reads: a read variable's flip re-keys its suite alone, an
    /// ambient terminal variable's flip re-keys nothing, and an unnamable
    /// read refuses caching. The probe names deliberately avoid the
    /// `DART_`/`FLUTTER_` prefixes — a toolchain-half probe would flip
    /// every parallel test's base key mid-run.
    #[test]
    fn environment_joins_keys_through_read_names_only() {
        let (tmp, root) = fixture();
        let store = tmp.path().join("store");
        let bin = root.join("pubspec.yaml");
        std::fs::write(
            root.join("test/env_test.dart"),
            "void main() { print(Platform.environment['dartq_probe_read']); }\n",
        )
        .expect("write");
        std::fs::write(
            root.join("test/opaque_test.dart"),
            "void main() { for (final e in Platform.environment.entries) {} }\n",
        )
        .expect("write");
        let suites = ["test/a_test.dart", "test/env_test.dart", "test/opaque_test.dart"]
            .map(|rel| root.join(rel))
            .to_vec();
        let mut cache = Cache::with_store(&root, store);
        unsafe {
            std::env::set_var("dartq_probe_read", "one");
            std::env::set_var("dartq_probe_ambient", "x");
        }
        let first = cache.keys(&suites, runner(&bin)).expect("keys");
        assert!(first[0].is_some() && first[1].is_some());
        assert!(first[2].is_none(), "an unnamable environment read is never cached");

        // An ambient variable no closure reads changes nothing.
        unsafe { std::env::set_var("dartq_probe_ambient", "y"); }
        assert_eq!(cache.rekey(&suites, runner(&bin)).expect("keys"), first);

        // The read variable's flip re-keys only its reader.
        unsafe { std::env::set_var("dartq_probe_read", "two"); }
        let second = cache.rekey(&suites, runner(&bin)).expect("keys");
        assert_eq!(second[0], first[0], "a suite without the read is untouched");
        assert_ne!(second[1], first[1], "the reader re-keys");
        // Removing the variable re-keys again: presence is part of the key.
        unsafe { std::env::remove_var("dartq_probe_read"); }
        assert_ne!(cache.rekey(&suites, runner(&bin)).expect("keys")[1], second[1]);
        unsafe { std::env::remove_var("dartq_probe_ambient"); }
    }

    /// A dependency package's unnamable environment read no longer refuses:
    /// the suite keys with the whole environment's digest folded in, so an
    /// environment change re-keys it while an identical one replays. The
    /// suite's own package keeps refusing outright, and a closure mixing
    /// both tiers keys. The fixture spells no `pubspec.lock`, so every
    /// package outside the suite's own is a dependency — the conservative
    /// floor a missing lock gives.
    #[test]
    fn dependency_opaque_env_reads_key_the_environment() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dep = tmp.path().join("dep");
        std::fs::create_dir_all(dep.join("lib")).expect("dir");
        let dep = dep.canonicalize().expect("canon");
        std::fs::create_dir_all(tmp.path().join("fx")).expect("dir");
        let root = tmp.path().join("fx").canonicalize().expect("canon");
        let write = |path: &std::path::Path, body: &str| {
            std::fs::create_dir_all(path.parent().expect("parent")).expect("dir");
            std::fs::write(path, body).expect("write");
        };
        write(&root.join("pubspec.yaml"), "name: fx\n");
        write(
            &root.join(".dart_tool/package_config.json"),
            &format!(
                concat!(
                    r#"{{"configVersion": 2, "packages": [{{"name": "fx", "rootUri": "../", "packageUri": "lib/", "languageVersion": "3.5"}}, "#,
                    r#"{{"name": "dep", "rootUri": "file://{}", "packageUri": "lib/", "languageVersion": "3.5"}}]}}"#
                ),
                dep.display(),
            ),
        );
        // The dependency reads the whole environment map; the suite's own
        // helper reads one variable by name.
        write(
            &dep.join("lib/dep.dart"),
            "import 'dart:io';\nMap<String, String> env() => Platform.environment;\n",
        );
        write(&root.join("lib/helper.dart"), "import 'dart:io';\nString? probe() => Platform.environment['dartq_probe_dep'];\n");
        write(&root.join("test/dep_test.dart"), "import 'package:dep/dep.dart';\nvoid main() {}\n");
        write(
            &root.join("test/mixed_test.dart"),
            "import 'package:dep/dep.dart';\nimport 'package:fx/helper.dart';\nvoid main() {}\n",
        );
        write(
            &root.join("test/own_test.dart"),
            "import 'dart:io';\nvoid main() { final e = Platform.environment; }\n",
        );
        let suites = ["test/dep_test.dart", "test/mixed_test.dart", "test/own_test.dart"]
            .map(|rel| root.join(rel))
            .to_vec();
        let bin = root.join("pubspec.yaml");
        let mut cache = Cache::with_store(&root, tmp.path().join("store"));
        unsafe { std::env::set_var("dartq_probe_dep", "one"); }
        let first = cache.keys(&suites, runner(&bin)).expect("keys");
        assert!(first[0].is_some(), "a dependency's whole-map read keys");
        assert!(first[1].is_some(), "a mixed own/dependency closure keys");
        assert!(first[2].is_none(), "the suite's own whole-map read still refuses");

        // A changed variable re-keys — the dependency's read can observe
        // it, so the suite runs again. (No "identical environment replays"
        // assertion: sibling tests mutate the process environment in
        // parallel, and the digest covers it all.)
        unsafe { std::env::set_var("dartq_probe_dep", "two"); }
        let second = cache.rekey(&suites, runner(&bin)).expect("keys");
        assert_ne!(second[0], first[0], "the digest tracks the environment");
        assert_ne!(second[1], first[1], "the mixed suite re-keys too");
        unsafe { std::env::remove_var("dartq_probe_dep"); }
    }

    /// The base key's environment half is the toolchain's alone: IDE and
    /// terminal noise (`VSCODE_*`, `TERM_PROGRAM`) never enters a key, so
    /// a terminal switch or an editor restart never invalidates replay —
    /// a variable joins only through a closure's literal read.
    #[test]
    fn base_key_ignores_ide_terminal_noise() {
        let (tmp, root) = fixture();
        let bin = root.join("pubspec.yaml");
        let suites = vec![root.join("test/a_test.dart")];
        let mut cache = Cache::with_store(&root, tmp.path().join("store"));
        unsafe {
            std::env::set_var("VSCODE_DARTQ_PROBE", "one");
            std::env::set_var("TERM_PROGRAM_DARTQ", "terminal-a");
        }
        let first = cache.keys(&suites, runner(&bin)).expect("keys");
        unsafe {
            std::env::set_var("VSCODE_DARTQ_PROBE", "two");
            std::env::set_var("TERM_PROGRAM_DARTQ", "terminal-b");
        }
        assert_eq!(
            cache.rekey(&suites, runner(&bin)).expect("keys"),
            first,
            "terminal and editor noise is not an input"
        );
        unsafe {
            std::env::remove_var("VSCODE_DARTQ_PROBE");
            std::env::remove_var("TERM_PROGRAM_DARTQ");
        }
    }

    /// The flutter replay rule (#2656): the Flutter SDK's own packages
    /// are the toolchain tier — keyed by content (an SDK edit re-keys)
    /// but never refused for their internal clock or environment
    /// plumbing, and the tier is named by the run's own `flutter` binary
    /// when the config carries no `flutterRoot` property (the
    /// `dart pub get` shape). The first-party binding shapes that used
    /// to refuse every flutter suite — `flutter_test`'s `_binding_io
    /// .dart` aliasing the whole environment map beside a wall-clock
    /// read — key here; a suite-side file with the same shapes still
    /// refuses.
    #[test]
    fn flutter_suites_key_through_the_toolchain_tier() {
        let tmp = tempfile::tempdir().expect("tmp");
        let sdk = tmp.path().join("sdk");
        let root = tmp.path().join("fx");
        let write = |path: &std::path::Path, body: &str| {
            std::fs::create_dir_all(path.parent().expect("parent")).expect("dir");
            std::fs::write(path, body).expect("write");
        };
        write(&sdk.join("bin/flutter"), "#!/stub\n");
        write(
            &sdk.join("packages/ft/lib/ft.dart"),
            concat!(
                "import 'dart:io';\n",
                "Object? environment;\n",
                "void bind() {\n",
                "  environment ??= Platform.environment;\n",
                "  print(DateTime.now());\n",
                "  print(Platform.environment['UNIT_TEST_ASSETS']);\n",
                "}\n",
            ),
        );
        write(&root.join("pubspec.yaml"), "name: fx\n");
        write(
            &root.join(".dart_tool/package_config.json"),
            &format!(
                concat!(
                    r#"{{"configVersion": 2, "packages": [{{"name": "fx", "rootUri": "../", "packageUri": "lib/", "languageVersion": "3.5"}}, "#,
                    r#"{{"name": "ft", "rootUri": "file://{}", "packageUri": "lib/", "languageVersion": "3.5"}}], "generator": "dart"}}"#
                ),
                sdk.display()
            ),
        );
        write(
            &root.join("test/a_test.dart"),
            "import 'package:ft/ft.dart';\nvoid main() {}\n",
        );
        write(
            &root.join("test/local_test.dart"),
            "void main() { final e = Platform.environment; }\n",
        );
        let root = root.canonicalize().expect("canon");
        let bin = sdk.join("bin/flutter");
        let suites = ["test/a_test.dart", "test/local_test.dart"]
            .map(|rel| root.join(rel))
            .to_vec();
        let runner = Runner { bin: &bin, args: &[], flutter: true };
        let store = tmp.path().join("store");
        let mut cache = Cache::with_store(&root, store.clone());
        let keys = cache.keys(&suites, runner).expect("keys");
        assert!(
            keys[0].is_some(),
            "the toolchain tier's plumbing does not refuse the suite"
        );
        assert!(
            keys[1].is_none(),
            "a suite-side unnamable environment read still refuses"
        );
        // The replay round trip: stage, verify against an unchanged
        // disk, commit, and a fresh cache answers the stored events —
        // the second run replays instead of running stock.
        let key = keys[0].clone().expect("key");
        cache.stage(&key, &[]).expect("stage");
        let after = cache.rekey(&suites, runner).expect("rekey");
        assert_eq!(after[0].as_deref(), Some(key.as_str()), "stable across a run");
        let verified: FxHashSet<String> = [key.clone()].into_iter().collect();
        cache.commit(&verified).expect("commit");
        cache.save().expect("save");
        assert!(
            Cache::with_store(&root, store).lookup(&key).is_some(),
            "the next run replays"
        );
    }
}
