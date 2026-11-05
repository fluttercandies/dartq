//! The shim orchestrator: one `flutter build|run|test` invocation end to
//! end — pre-run key lookup, the flutter spawn under process-table
//! observation, post-run ingest.
//!
//! The caller (the CLI's argv0 dispatch) hands over the original argv
//! and a [`FlutterRunner`] that spawns the real flutter binary with
//! inherited stdio. The argv is modeled by [`parse_flutter_args`]; an
//! unmodeled or rejected argv runs as a pure passthrough. A modeled
//! invocation:
//!
//! 1. **Version gate**: the toolchain fingerprint must name
//!    [`SUPPORTED_FLUTTER_VERSION`] — the args/option tables and the
//!    build-dir name formula are pinned to it. Anything else passes
//!    through before the store is ever touched.
//! 2. **PRE**: compute the coarse config key (toolchain, filtered define
//!    vector, target, provenance file contents — no depfile closure, so
//!    a fresh worktree keys identically to the tree the entry was
//!    ingested from), look it up in the CAS, and on a hit materialize
//!    every replayable [`DirRecord`] of the entry into this tree —
//!    verbatim for tree-relative names, recomputed from the recorded
//!    template and the root-rewritten output dir otherwise. Content
//!    divergence the coarse key cannot see is delegated to
//!    flutter_tools: every build re-validates a dir's recorded input
//!    hashes and rebuilds on mismatch, so a wasted materialization is
//!    the entire cost of a stale hit. Any pre-phase failure degrades to
//!    a miss: the shim never blocks a build.
//! 3. **RUN**: spawn flutter with the ORIGINAL argv (never a lossy
//!    copy) while an assemble [`CaptureObserver`] polls the process
//!    table for the `flutter assemble` invocations the platform build
//!    systems spawn behind the shim (they invoke the SDK's absolute
//!    binary path, so no PATH interception can see them). A non-zero
//!    exit skips POST entirely.
//! 4. **POST**: ingest EVERY hash dir the run touched, each as one
//!    [`DirRecord`] — name and template from an observed assemble
//!    invocation, or verbatim-replayable on the `build bundle` face, or
//!    not-replayable provenance otherwise — under the same coarse key;
//!    then run gc opportunistically when the store is over budget.
//!
//! Zero-config lives above this module: with no `build_cache` section
//! the caller never reaches [`run_shim`] at all.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fmt::Display;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::args_model::{ArgModelError, ParsedFlutterCmd, flutter_build_dir_name, parse_flutter_args};
use crate::assemble::AssembleCapture;
use crate::error::Result;
use crate::fsutil::file_blake3;
use crate::ingest_scan::{find_touched_hash_dirs, scan_entry_files};
use crate::key;
use crate::materialize::{self, DirPlan, MaterializePlan, MaterializeReport};
use crate::observer::CaptureObserver;
use crate::store::CasStore;
use crate::types::{
    BlobId, ClosureFile, DirRecord, EntryManifest, FileIdentity, KeyInputs, NameRule,
    ToolchainFingerprint,
};

/// The one flutter version whose option tables and build-dir name
/// formula this crate models (flutter_tools 3.47.1). Every other
/// version passes through untouched.
pub const SUPPORTED_FLUTTER_VERSION: &str = "3.47.1";

/// Everything the shim needs beyond the invocation itself.
#[derive(Debug, Clone)]
pub struct ShimConfig {
    /// CAS store root (default `$XDG_CACHE_HOME/dartq/cas/v1`, falling
    /// back to `~/.cache/dartq/cas/v1`; assembled by the config glue).
    pub store_root: PathBuf,
    /// Store size budget (default 20 GiB); gc runs when exceeded.
    pub budget_bytes: u64,
    /// Seconds after a hit during which an entry is unevictable.
    pub grace_secs: u64,
    /// The resolved flutter binary this invocation keys and spawns.
    /// `None` (discovery failed upstream) is a passthrough.
    pub flutter_bin: Option<PathBuf>,
}

/// What one shim invocation did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Ran flutter with no cache logic around it.
    Passthrough,
    /// PRE found a stored entry and materialized it into this tree.
    PreHitMaterialized,
    /// PRE computed a key but the store had no entry.
    PreMiss,
    /// A modeled run exited 0 and its build dirs were ingested.
    Ingested,
}

/// The outcome of one shim invocation: what the orchestrator did, the
/// child's exit code (`None` when the spawn itself failed), the key it
/// computed, and what the materialization did.
#[derive(Debug, Clone)]
pub struct ShimReport {
    pub action: Action,
    pub exit_code: Option<i32>,
    /// The coarse config key (PRE's, which POST reuses when it ingests).
    pub key: Option<String>,
    pub materialize: Option<MaterializeReport>,
    /// Files stored by the post-run ingest.
    pub ingest_files: u64,
    /// Entry dirs a hit skipped because their names are not replayable
    /// in this tree.
    pub skipped_dirs: u64,
    pub reason: &'static str,
}

/// Spawns the real flutter binary. The seam between the orchestrator
/// and the process: production uses [`CommandRunner`], tests use fakes.
pub trait FlutterRunner {
    /// Runs flutter with `argv` (the original arguments after the
    /// `flutter` token) in `cwd`, inheriting stdio, and returns the
    /// child's exit code.
    fn run(&mut self, argv: &[OsString], cwd: &Path) -> io::Result<i32>;
}

/// The production [`FlutterRunner`]: `Command::status` with inherited
/// stdio, so an interactive `flutter run` streams exactly as a direct
/// invocation would. The child stays in the shim's (foreground)
/// process group — creating a separate group would sever Ctrl-C from
/// it — and spawns with no environment tagging: the nested assemble
/// invocations it triggers are captured from the process table, not by
/// the child's cooperation.
pub struct CommandRunner {
    flutter: PathBuf,
}

impl CommandRunner {
    pub fn new(flutter: PathBuf) -> Self {
        Self { flutter }
    }
}

impl FlutterRunner for CommandRunner {
    fn run(&mut self, argv: &[OsString], cwd: &Path) -> io::Result<i32> {
        let status = Command::new(&self.flutter).args(argv).current_dir(cwd).status()?;
        Ok(exit_code(status))
    }
}

/// The shell's exit-code convention for a signal death: 128 + signal.
fn exit_code(status: ExitStatus) -> i32 {
    status.code().unwrap_or_else(|| {
        #[cfg(unix)]
        {
            use std::os::unix::process::ExitStatusExt;
            128 + status.signal().unwrap_or(0)
        }
        #[cfg(not(unix))]
        {
            1
        }
    })
}

/// Runs one shim invocation end to end. Diagnostics go to stderr as
/// single `dartq-bcache: ...` lines (stdout stays the child's); every
/// cache-side failure degrades to a miss or a passthrough — the build
/// itself is never blocked, and its exit code always wins.
pub fn run_shim(
    argv: &[OsString],
    cwd: &Path,
    config: &ShimConfig,
    runner: &mut dyn FlutterRunner,
) -> ShimReport {
    let lossy: Vec<String> = argv
        .iter()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    let parsed = match parse_flutter_args(&lossy) {
        Ok(parsed) => parsed,
        Err(err) => {
            let reason = if matches!(err, ArgModelError::Invalid(_)) {
                "invalid_argv"
            } else {
                "unmodeled_command"
            };
            return run_passthrough(argv, cwd, runner, reason);
        }
    };

    let Some(flutter_bin) = config.flutter_bin.clone() else {
        return run_passthrough(argv, cwd, runner, "flutter_unresolved");
    };
    let root = match key::flutter_root(&flutter_bin) {
        Ok(root) => root,
        Err(_) => return run_passthrough(argv, cwd, runner, "flutter_root_unreadable"),
    };
    let toolchain = match cached_fingerprint(&root, &config.store_root) {
        Ok(toolchain) => toolchain,
        Err(_) => return run_passthrough(argv, cwd, runner, "toolchain_unreadable"),
    };
    if toolchain.flutter_version != SUPPORTED_FLUTTER_VERSION {
        return run_passthrough(argv, cwd, runner, "unsupported_flutter_version");
    }
    store_fingerprint_cache(&config.store_root, &root, &toolchain);

    let Some(cwd) = fs::canonicalize(cwd).ok() else {
        return run_passthrough(argv, cwd, runner, "cwd_unreadable");
    };

    let dart_defines = dart_define_vector(&parsed, &root, &toolchain);
    let key_defines = key::filter_defines(&dart_defines.iter().cloned().collect());
    let build_root = cwd.join(".dart_tool").join("flutter_build");

    let mut report = ShimReport {
        action: Action::PreMiss,
        exit_code: None,
        key: None,
        materialize: None,
        ingest_files: 0,
        skipped_dirs: 0,
        reason: "pre_miss",
    };

    // PRE: the coarse config key needs no build dir, so it is computable
    // on any tree — a fresh worktree hits as long as the config (toolchain,
    // defines, target, provenance contents) matches the ingested entry.
    let key = coarse_key(&cwd, &parsed, &key_defines, &toolchain);
    report.key = Some(key.clone());
    let mut hit: Option<EntryManifest> = None;
    if let Ok(store) = CasStore::open(&config.store_root) {
        match store.lookup(&key) {
            Ok(Some(manifest)) => hit = Some(manifest),
            Ok(None) => {}
            Err(err) => note(&err),
        }
    }

    if let Some(manifest) = hit {
        let engine_for_names = if local_engine(&parsed) {
            ""
        } else {
            toolchain.engine_revision.as_str()
        };
        let mut dirs = Vec::with_capacity(manifest.dirs.len());
        let mut skipped = 0u64;
        for dir in &manifest.dirs {
            match dir.name_rule {
                NameRule::VerbatimRelative => dirs.push(DirPlan {
                    name: dir.name_observed.clone(),
                    files: dir.files.clone(),
                }),
                NameRule::RecomputeFromTemplate => {
                    let output_dir = rewrite_output_dir(
                        &dir.name_output_dir,
                        &manifest.project_root_at_ingest,
                        &cwd,
                    );
                    dirs.push(DirPlan {
                        name: flutter_build_dir_name(
                            engine_for_names,
                            &dir.name_defines,
                            Path::new(&output_dir),
                        ),
                        files: dir.files.clone(),
                    });
                }
                NameRule::NotReplayable => skipped += 1,
            }
        }
        report.skipped_dirs = skipped;
        if dirs.is_empty() {
            report.reason = "entry_not_replayable";
            note("hit entry has no replayable build dir");
        } else {
            let replayable = dirs.len();
            let plan = MaterializePlan {
                dirs,
                old_root: manifest.project_root_at_ingest.clone(),
                new_root: cwd.clone(),
            };
            let placed = CasStore::open(&config.store_root).and_then(|store| {
                let placed = materialize::materialize(&store, &plan, &cwd)?;
                store.record_hit(&key).map(|()| placed)
            });
            match placed {
                Ok(placed) => {
                    report.action = Action::PreHitMaterialized;
                    report.materialize = Some(placed);
                    report.reason = "pre_hit";
                    note(&format!(
                        "hit {} materialized {} dirs, {skipped} skipped",
                        &key[..8],
                        replayable
                    ));
                }
                Err(err) => {
                    report.reason = "materialize_failed";
                    note(&err);
                }
            }
        }
    }

    // RUN: the original argv, the canonical cwd (flutter's bookkeeping
    // paths then match the POST scan's root), the child's exit code —
    // observed throughout for the assemble invocations the platform
    // build systems spawn behind the child.
    let run_started = now_floored_to_secs();
    let observer = CaptureObserver::start(cwd.clone(), toolchain.engine_revision.clone());
    let run = runner.run(argv, &cwd);
    let observed = observer.finish();
    match run {
        Ok(code) => report.exit_code = Some(code),
        Err(err) => {
            note(&format!("flutter spawn failed: {err}"));
            return report;
        }
    }

    // POST: ingest every dir the run touched; a hit whose key the rebuild
    // reproduced needs no re-ingest (the entry is already stored).
    if report.exit_code == Some(0) {
        let touched = match find_touched_hash_dirs(&build_root, run_started) {
            Ok(touched) => touched,
            Err(err) => {
                note(&err);
                Vec::new()
            }
        };
        if !touched.is_empty() {
            // Only templates whose dir this run materialized carry
            // information; the rest (a foreign process, a failed
            // assemble) are dead observations.
            let captures: Vec<AssembleCapture> = observed
                .into_iter()
                .filter(|capture| {
                    touched.iter().any(|(name, _)| name == &capture.name)
                })
                .collect();
            let post_key = coarse_key(&cwd, &parsed, &key_defines, &toolchain);
            let already_stored = report.action == Action::PreHitMaterialized
                && report.key.as_deref() == Some(post_key.as_str())
                && captures.is_empty();
            if !already_stored {
                let ingested = CasStore::open(&config.store_root).and_then(|store| {
                    let count = ingest_observed(
                        &store,
                        &build_root,
                        &touched,
                        &captures,
                        &cwd,
                        &parsed,
                        &key_defines,
                        &toolchain,
                        post_key.clone(),
                    )?;
                    maybe_gc(&store, config);
                    Ok(count)
                });
                match ingested {
                    Ok(count) => {
                        if report.action != Action::PreHitMaterialized {
                            report.action = Action::Ingested;
                        }
                        report.ingest_files = count;
                        if report.key.is_none() {
                            report.key = Some(post_key.clone());
                        }
                        note(&format!("ingested {} ({count} files)", &post_key[..8]));
                    }
                    Err(err) => note(&err),
                }
            }
        }
    }

    report
}

/// Passthrough: spawn the child untouched and report.
fn run_passthrough(
    argv: &[OsString],
    cwd: &Path,
    runner: &mut dyn FlutterRunner,
    reason: &'static str,
) -> ShimReport {
    let mut report = ShimReport {
        action: Action::Passthrough,
        exit_code: None,
        key: None,
        materialize: None,
        ingest_files: 0,
        skipped_dirs: 0,
        reason,
    };
    match runner.run(argv, cwd) {
        Ok(code) => report.exit_code = Some(code),
        Err(err) => note(&format!("flutter spawn failed: {err}")),
    }
    report
}

/// The coarse config key of one invocation (see [`key::config_key`]):
/// toolchain, filtered define vector, target, and the contents of the
/// provenance files. Nothing in it exists only after a build, so PRE
/// and POST compute the identical key on any tree.
fn coarse_key(
    cwd: &Path,
    parsed: &ParsedFlutterCmd,
    key_defines: &BTreeMap<String, String>,
    toolchain: &ToolchainFingerprint,
) -> String {
    key::config_key(&KeyInputs {
        toolchain: toolchain.clone(),
        defines: key_defines.clone(),
        closure: provenance_closure(cwd, &parsed.define_files),
        target: parsed.target.clone(),
    })
}

/// The provenance files hashed into every key: `pubspec.yaml`,
/// `pubspec.lock`, `.dart_tool/package_config.json`, and every
/// `--dart-define-from-file` (flutter_tools folds their contents into
/// the define list and no depfile records them). Files that do not
/// exist (or cannot be read) are skipped — a fresh tree keys on what
/// is there.
fn provenance_closure(cwd: &Path, define_files: &[PathBuf]) -> Vec<ClosureFile> {
    let mut files = Vec::new();
    for relative in ["pubspec.yaml", "pubspec.lock", ".dart_tool/package_config.json"] {
        push_provenance(&cwd.join(relative), Some(Path::new(relative)), &mut files);
    }
    for define_file in define_files {
        let absolute = if define_file.is_absolute() {
            define_file.clone()
        } else {
            lexical_normalize(&cwd.join(define_file))
        };
        let identity_relative = absolute
            .strip_prefix(cwd)
            .ok()
            .map(|relative| relative.to_path_buf());
        push_provenance(&absolute, identity_relative.as_deref(), &mut files);
    }
    files
}

/// Hashes one provenance file into `files` (deduplicated by identity).
fn push_provenance(absolute: &Path, relative: Option<&Path>, files: &mut Vec<ClosureFile>) {
    let Ok(metadata) = fs::metadata(absolute) else {
        return;
    };
    if !metadata.is_file() {
        return;
    }
    let Ok(content) = file_blake3(absolute) else {
        return;
    };
    let identity = match relative {
        Some(relative) => FileIdentity::TreeRelative(relative.to_path_buf()),
        None => FileIdentity::Absolute(absolute.to_path_buf()),
    };
    if !files.iter().any(|file| file.identity == identity) {
        files.push(ClosureFile {
            identity,
            content,
            size: metadata.len(),
        });
    }
}

/// Lexical normalization (the key engine's spelling): drops `.` and
/// folds `..` without touching the filesystem.
fn lexical_normalize(path: &Path) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => match out.components().next_back() {
                Some(std::path::Component::Normal(_)) => {
                    out.pop();
                }
                Some(Component::RootDir) => {}
                _ => out.push(".."),
            },
            other => out.push(other),
        }
    }
    out
}

/// The dart defines of one invocation in flutter's injection order:
/// the injected FLUTTER_* set first (the args model's documented
/// order), then the user's (name order). A user define replaces an
/// injected value in place. `FLUTTER_GIT_URL` is left out — it is SDK
/// provenance, and `filter_defines` drops it anyway. The SDK-constant
/// values a missing version manifest cannot supply are simply omitted:
/// they cannot vary within one install, and the toolchain fingerprint
/// already keys the install, so omission cannot cause a wrong hit.
fn dart_define_vector(
    parsed: &ParsedFlutterCmd,
    root: &Path,
    toolchain: &ToolchainFingerprint,
) -> Vec<(String, String)> {
    let mut defines: Vec<(String, String)> = Vec::new();
    let push = |name: &str, value: String, defines: &mut Vec<(String, String)>| {
        if !defines.iter().any(|(existing, _)| existing == name) {
            defines.push((name.to_owned(), value));
        }
    };
    if let Some(flavor) = &parsed.target.flavor {
        push("FLUTTER_APP_FLAVOR", flavor.clone(), &mut defines);
    }
    let extra = &parsed.target.extra_key_args;
    if let Some(name) = flag_value(extra, "build-name") {
        push("FLUTTER_BUILD_NAME", name, &mut defines);
    }
    if let Some(number) = flag_value(extra, "build-number") {
        push("FLUTTER_BUILD_NUMBER", number, &mut defines);
    }
    push(
        "FLUTTER_VERSION",
        toolchain.flutter_version.clone(),
        &mut defines,
    );
    for (name, value) in version_json_extras(root) {
        push(name, value, &mut defines);
    }
    push(
        "FLUTTER_ENGINE_REVISION",
        toolchain.engine_revision.clone(),
        &mut defines,
    );
    for (name, value) in &parsed.defines {
        if name == "FLUTTER_GIT_URL" {
            continue;
        }
        match defines.iter_mut().find(|(existing, _)| existing == name) {
            Some(slot) => slot.1 = value.clone(),
            None => defines.push((name.clone(), value.clone())),
        }
    }
    defines
}

/// The SDK-constant extras `bin/cache/flutter.version.json` carries
/// beyond the fingerprint: channel, framework revision, dart version.
/// Unreadable or absent → none (see [`dart_define_vector`]'s safety
/// note).
fn version_json_extras(root: &Path) -> Vec<(&'static str, String)> {
    #[derive(Deserialize)]
    struct VersionJson {
        channel: Option<String>,
        #[serde(rename = "frameworkRevision")]
        framework_revision: Option<String>,
        #[serde(rename = "dartSdkVersion")]
        dart_sdk_version: Option<String>,
    }
    let bytes = fs::read(root.join("bin").join("cache").join("flutter.version.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<VersionJson>(&bytes).ok());
    let Some(json) = bytes else {
        return Vec::new();
    };
    [
        ("FLUTTER_CHANNEL", json.channel),
        ("FLUTTER_FRAMEWORK_REVISION", json.framework_revision),
        ("FLUTTER_DART_VERSION", json.dart_sdk_version),
    ]
    .into_iter()
    .filter_map(|(name, value)| value.map(|value| (name, value)))
    .collect()
}

/// The last `--name` value among the verbatim keyed tokens
/// (`--name=v` or `--name v`).
fn flag_value(extra: &[String], name: &str) -> Option<String> {
    let mut value = None;
    let mut tokens = extra.iter();
    while let Some(token) = tokens.next() {
        if let Some(inline) = token.strip_prefix(&format!("--{name}=")) {
            value = Some(inline.to_owned());
        } else if token == &format!("--{name}") {
            if let Some(next) = tokens.next() {
                value = Some(next.clone());
            }
        }
    }
    value
}

/// Whether the outer argv selects local artifacts (`--local-engine`
/// and kin), which makes the engine revision every build-dir name
/// hashes null — the recorded templates were captured the same way.
fn local_engine(parsed: &ParsedFlutterCmd) -> bool {
    const OPTIONS: [&str; 4] = [
        "--local-engine",
        "--local-engine-src-path",
        "--local-engine-host",
        "--local-web-sdk",
    ];
    parsed.target.extra_key_args.iter().any(|token| {
        OPTIONS
            .iter()
            .any(|option| token == *option || token.starts_with(&format!("{option}=")))
    })
}

/// The recorded output-dir string re-aimed at the target tree: the
/// ingest-root prefix replaced with the target root; a relative or
/// foreign string replays verbatim (a tree-relative output dir is
/// tree-independent by construction).
fn rewrite_output_dir(recorded: &str, ingest_root: &str, target_root: &Path) -> String {
    recorded.strip_prefix(ingest_root).map_or_else(
        || recorded.to_owned(),
        |rest| format!("{}{}", target_root.display(), rest),
    )
}

/// Scans and ingests every observed build dir under `key`. Each dir
/// becomes one [`DirRecord`]: an assemble template whose name matches
/// supplies the define map and output dir
/// ([`NameRule::RecomputeFromTemplate`]); on the `build bundle` face
/// the dir is tree-relative and replays verbatim
/// ([`NameRule::VerbatimRelative`]); anything else is recorded as
/// [`NameRule::NotReplayable`] provenance. Templates whose dir never
/// materialized are dropped (a failed or partial assemble). Returns the
/// file count stored.
///
/// # Errors
///
/// Whatever the scan or the store's ingest reports.
fn ingest_observed(
    store: &CasStore,
    build_root: &Path,
    observed: &[(String, SystemTime)],
    captures: &[AssembleCapture],
    cwd: &Path,
    parsed: &ParsedFlutterCmd,
    key_defines: &BTreeMap<String, String>,
    toolchain: &ToolchainFingerprint,
    key: String,
) -> Result<u64> {
    let bundle_face = parsed.target.command == "build bundle";
    let mut dirs = Vec::with_capacity(observed.len());
    let mut sources: Vec<(PathBuf, BlobId)> = Vec::new();
    // `observed` is name-sorted, so the DirRecords come out name-sorted.
    for (name, _) in observed {
        let scanned = scan_entry_files(&build_root.join(name), cwd)?;
        let (name_rule, name_defines, name_output_dir) =
            match captures.iter().find(|capture| &capture.name == name) {
                Some(capture) => (
                    NameRule::RecomputeFromTemplate,
                    capture.defines.clone(),
                    capture.output_dir.clone(),
                ),
                None if bundle_face => (
                    NameRule::VerbatimRelative,
                    BTreeMap::new(),
                    String::new(),
                ),
                None => (NameRule::NotReplayable, BTreeMap::new(), String::new()),
            };
        dirs.push(DirRecord {
            name_observed: name.clone(),
            name_rule,
            name_defines,
            name_output_dir,
            files: scanned.iter().map(|(_, file)| file.clone()).collect(),
        });
        sources.extend(scanned.iter().map(|(path, file)| (path.clone(), file.blob)));
    }
    let now = unix_now();
    let draft = EntryManifest {
        key,
        project_root_at_ingest: cwd.to_string_lossy().into_owned(),
        dirs,
        defines: key_defines.clone(),
        target: parsed.target.clone(),
        toolchain: toolchain.clone(),
        created_at_unix: now,
        last_hit_unix: now,
    };
    store.ingest(draft, &sources)?;
    Ok(sources.len() as u64)
}

/// GC when the store is over budget (cheap stats check first; failures
/// surface as one stderr line and never affect the build).
fn maybe_gc(store: &CasStore, config: &ShimConfig) {
    match store.stats() {
        Ok(stats) if stats.blob_bytes > config.budget_bytes => {
            if let Err(err) = store.gc(config.budget_bytes, config.grace_secs, unix_now()) {
                note(&err);
            }
        }
        Ok(_) => {}
        Err(err) => note(&err),
    }
}

/// The toolchain fingerprint through the
/// `<store_root>/meta/toolchain.json` cache, so the ~40 MB of hashed
/// artifacts is hashed once per SDK state instead of once per
/// invocation. A cached fingerprint is used only when the flutter root
/// and both hashed artifacts' mtime+size still match.
///
/// Safety: a stale cache yields a wrong fingerprint, hence a wrong
/// key, hence a lookup miss — entries live under their true keys, so a
/// stale cache can never serve a wrong hit. The same argument covers
/// the version gate: a gate that passes on a stale `3.47.1` only
/// produces misses.
fn cached_fingerprint(root: &Path, store_root: &Path) -> Result<ToolchainFingerprint> {
    let Some((dill_stamp, frontend_stamp)) = hashed_artifact_stamps(root) else {
        return key::fingerprint_toolchain(root);
    };
    let cache_path = store_root.join("meta").join("toolchain.json");
    if let Ok(bytes) = fs::read(&cache_path)
        && let Ok(cache) = serde_json::from_slice::<ToolchainCache>(&bytes)
        && cache.flutter_root == root
        && (cache.dill, cache.frontend) == (dill_stamp, frontend_stamp)
    {
        return Ok(cache.fingerprint);
    }
    key::fingerprint_toolchain(root)
}

/// Writes the fingerprint cache (tmp + rename, best-effort: a failed
/// write only costs the next call a rehash).
fn store_fingerprint_cache(store_root: &Path, root: &Path, fingerprint: &ToolchainFingerprint) {
    let write = || -> io::Result<()> {
        let Some((dill, frontend)) = hashed_artifact_stamps(root) else {
            return Ok(());
        };
        let cache = ToolchainCache {
            flutter_root: root.to_path_buf(),
            dill,
            frontend,
            fingerprint: fingerprint.clone(),
        };
        let dir = store_root.join("meta");
        fs::create_dir_all(&dir)?;
        let bytes = serde_json::to_vec(&cache)
            .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
        let tmp = dir.join(format!(".toolchain.{}.dartq-tmp", std::process::id()));
        fs::write(&tmp, bytes)?;
        fs::rename(&tmp, dir.join("toolchain.json"))
    };
    let _ = write();
}

/// mtime+size identity of the two hashed artifacts, when both exist
/// (the aot snapshot, the one current layout ships).
fn hashed_artifact_stamps(root: &Path) -> Option<(ArtifactStamp, ArtifactStamp)> {
    let cache = root.join("bin").join("cache").join("dart-sdk");
    let dill = cache
        .join("lib")
        .join("_internal")
        .join("vm_platform_strong.dill");
    let frontend = cache
        .join("bin")
        .join("snapshots")
        .join("frontend_server_aot.dart.snapshot");
    Some((artifact_stamp(&dill)?, artifact_stamp(&frontend)?))
}

/// One hashed artifact's cache identity.
#[derive(Serialize, Deserialize, PartialEq, Eq)]
struct ArtifactStamp {
    /// Modification time in nanoseconds since the epoch.
    mtime_ns: u64,
    size: u64,
}

/// The on-disk toolchain fingerprint cache.
#[derive(Serialize, Deserialize)]
struct ToolchainCache {
    flutter_root: PathBuf,
    dill: ArtifactStamp,
    frontend: ArtifactStamp,
    fingerprint: ToolchainFingerprint,
}

fn artifact_stamp(path: &Path) -> Option<ArtifactStamp> {
    let metadata = fs::metadata(path).ok()?;
    let nanos = metadata
        .modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_nanos();
    Some(ArtifactStamp {
        mtime_ns: u64::try_from(nanos).unwrap_or(u64::MAX),
        size: metadata.len(),
    })
}

/// Resolves the flutter binary the shim spawns: the config override,
/// then `$DARTQ_FLUTTER`, then a PATH search that skips the shim's own
/// directory and the shim binary itself (either would recurse into the
/// shim). Symlinks resolve to the real SDK binary.
pub fn discover_flutter(explicit: Option<&Path>, own_exe: Option<&Path>) -> Option<PathBuf> {
    if let Some(path) = explicit {
        return Some(path.to_path_buf());
    }
    if let Some(path) = std::env::var_os("DARTQ_FLUTTER").filter(|value| !value.is_empty()) {
        return Some(PathBuf::from(path));
    }
    let dirs: Vec<PathBuf> = std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .collect();
    let exclude_dir = own_exe
        .and_then(|exe| exe.parent())
        .and_then(|dir| fs::canonicalize(dir).ok());
    let exclude_file = own_exe.and_then(|exe| fs::canonicalize(exe).ok());
    first_flutter_on_path(&dirs, exclude_dir.as_deref(), exclude_file.as_deref())
}

/// The PATH-search core of [`discover_flutter`]: the first `flutter`
/// on `dirs` that is neither in `exclude_dir` nor `exclude_file`
/// (compared after symlink resolution).
fn first_flutter_on_path(
    dirs: &[PathBuf],
    exclude_dir: Option<&Path>,
    exclude_file: Option<&Path>,
) -> Option<PathBuf> {
    for dir in dirs {
        let canonical_dir = fs::canonicalize(dir).unwrap_or_else(|_| dir.clone());
        if exclude_dir == Some(canonical_dir.as_path()) {
            continue;
        }
        let candidate = dir.join("flutter");
        if !candidate.is_file() {
            continue;
        }
        match fs::canonicalize(&candidate) {
            Ok(resolved) => {
                if exclude_file == Some(resolved.as_path()) {
                    continue;
                }
                return Some(resolved);
            }
            Err(_) => continue,
        }
    }
    None
}

/// `now`, floored to whole seconds: the run-start marker for the
/// touched-dir scan (whole-second-mtime file systems).
fn now_floored_to_secs() -> SystemTime {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(UNIX_EPOCH, |since| {
            UNIX_EPOCH + Duration::from_secs(since.as_secs())
        })
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}

/// One stderr diagnostic line; stdout stays the child's.
fn note(message: &(impl Display + ?Sized)) {
    eprintln!("dartq-bcache: {message}");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assemble::capture_templates;

    /// The as-tokenized (base) capture of one argv — the first of the
    /// enumerated join-variants.
    fn base_capture(argv: &[String], cwd: &Path, engine_revision: &str) -> AssembleCapture {
        let mut all = capture_templates(argv, cwd, engine_revision).expect("variants");
        assert!(!all.is_empty(), "an Ok capture always has the base parse");
        all.remove(0)
    }

    /// A tempdir and its canonical path (macOS tempdirs sit behind the
    /// `/var` -> `/private/var` symlink, and closure roots are
    /// canonical).
    fn tempdir() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = fs::canonicalize(dir.path()).expect("canonicalize tempdir");
        (dir, root)
    }

    fn write(path: &Path, bytes: &[u8]) {
        fs::create_dir_all(path.parent().expect("fixture parent")).expect("create dirs");
        fs::write(path, bytes).expect("write fixture");
    }

    /// A fake flutter SDK at `tmp/flutter-sdk` pinned to `version`, with
    /// the current cache layout and a `bin/flutter` binary.
    fn fake_flutter_sdk(tmp: &Path, version: &str) -> PathBuf {
        let sdk = tmp.join("flutter-sdk");
        let cache = sdk.join("bin").join("cache");
        write(
            &cache.join("engine-dart-sdk.stamp"),
            b"enginerevision0000000000000000\n",
        );
        write(
            &cache.join("flutter.version.json"),
            format!(
                r#"{{"frameworkVersion": "{version}", "channel": "stable", "frameworkRevision": "frameworkrevision000", "dartSdkVersion": "3.13.7", "repositoryUrl": "https://github.com/flutter/flutter.git"}}"#
            )
            .as_bytes(),
        );
        write(
            &cache
                .join("dart-sdk")
                .join("lib")
                .join("_internal")
                .join("vm_platform_strong.dill"),
            b"platform dill bytes",
        );
        write(
            &cache
                .join("dart-sdk")
                .join("bin")
                .join("snapshots")
                .join("frontend_server_aot.dart.snapshot"),
            b"aot frontend server bytes",
        );
        write(&sdk.join("bin").join("flutter"), b"#!/bin/sh\nexit 0\n");
        sdk
    }

    /// A synthetic flutter project tree: provenance files plus one
    /// source file.
    fn synthetic_project(tmp: &Path, tag: &str) -> PathBuf {
        let project = tmp.join(format!("app-{tag}"));
        write(&project.join("pubspec.yaml"), b"name: app\nversion: 1.0.0\n");
        write(&project.join("pubspec.lock"), b"packages: {}\n");
        write(
            &project.join(".dart_tool").join("package_config.json"),
            br#"{"configVersion":2,"packages":[{"name":"app","rootUri":"../","packageUri":"lib/"}]}"#,
        );
        write(&project.join("lib").join("main.dart"), b"void main() {}\n");
        project
    }

    fn escape_path(path: &Path) -> String {
        path.display().to_string().replace('\\', "\\\\").replace(' ', "\\ ")
    }

    /// Fabricates the `.dart_tool/flutter_build/<name>` dir a flutter run
    /// leaves behind: a payload dill, a depfile recording the tree's
    /// sources, and a stamp embedding the project root (the rewrite
    /// probe). Under capture-not-predict the name is whatever the run
    /// observed, so callers pick it.
    fn build_fixture(project: &Path, name: &str) -> PathBuf {
        let dir = project
            .join(".dart_tool")
            .join("flutter_build")
            .join(name);
        write(&dir.join("app.dill"), b"kernel payload bytes");
        let depfile = format!(
            "{}: {} {}\n",
            escape_path(&dir.join("app.dill")),
            escape_path(&project.join("lib").join("main.dart")),
            escape_path(&project.join(".dart_tool").join("package_config.json")),
        );
        write(&dir.join("kernel_snapshot.d"), depfile.as_bytes());
        let stamp = format!(r#"{{"inputs":["{}/lib/main.dart"]}}"#, project.display());
        write(&dir.join("flutter_build.dill.stamp"), stamp.as_bytes());
        dir
    }

    /// A runner that records the argv it got, optionally fabricates the
    /// build dir as its run effect, and exits with `code`.
    struct FakeRunner {
        code: i32,
        argv: Vec<OsString>,
        effect: Option<Box<dyn FnOnce(&Path)>>,
    }

    impl FakeRunner {
        fn exits(code: i32) -> Self {
            Self {
                code,
                argv: Vec::new(),
                effect: None,
            }
        }
    }

    impl FlutterRunner for FakeRunner {
        fn run(&mut self, argv: &[OsString], cwd: &Path) -> io::Result<i32> {
            self.argv = argv.to_vec();
            if let Some(effect) = self.effect.take() {
                effect(cwd);
            }
            Ok(self.code)
        }
    }

    fn raw_argv(tokens: &[&str]) -> Vec<OsString> {
        tokens.iter().map(|token| OsString::from(*token)).collect()
    }

    fn config_for(store_root: &Path, flutter_bin: &Path) -> ShimConfig {
        ShimConfig {
            store_root: store_root.to_path_buf(),
            budget_bytes: 20 << 30,
            grace_secs: 3600,
            flutter_bin: Some(flutter_bin.to_path_buf()),
        }
    }

    const BUNDLE_ARGV: [&str; 5] = ["build", "bundle", "--release", "--dart-define", "API=prod"];
    const APK_ARGV: [&str; 3] = ["build", "apk", "--release"];

    /// An arbitrary config-hash dir name: under capture-not-predict the
    /// tests never need to derive it, only to reuse what a run observed.
    const DIR_MAIN: &str = "2fe6e51a1c0b3d4e5f60718293a4b5c6";

    #[test]
    fn fresh_tree_pre_hit_materializes_recorded_dirs() {
        let (_guard, tmp) = tempdir();
        let sdk = fake_flutter_sdk(&tmp, "3.47.1");
        let store_root = tmp.join("cas").join("v1");
        let config = config_for(&store_root, &sdk.join("bin").join("flutter"));

        // Tree A builds: pre-miss on an empty store, POST ingests the
        // one dir the run fabricated.
        let tree_a = synthetic_project(&tmp, "a");
        let mut runner = FakeRunner {
            code: 0,
            argv: Vec::new(),
            effect: Some(Box::new(|cwd| {
                build_fixture(cwd, DIR_MAIN);
            })),
        };
        let report = run_shim(&raw_argv(&BUNDLE_ARGV), &tree_a, &config, &mut runner);
        assert_eq!(report.action, Action::Ingested);
        assert_eq!(report.reason, "pre_miss");
        assert_eq!(report.exit_code, Some(0));
        assert!(report.ingest_files >= 3);
        let key = report.key.clone().expect("post key");
        let store = CasStore::open(&store_root).expect("open store");
        let manifest = store.lookup(&key).expect("lookup").expect("entry");
        assert_eq!(manifest.dirs.len(), 1);
        assert_eq!(manifest.dirs[0].name_observed, DIR_MAIN);
        assert_eq!(manifest.dirs[0].name_rule, NameRule::VerbatimRelative);
        let seeded_hit = manifest.last_hit_unix;

        // Tree B is FRESH: no .dart_tool/flutter_build at all. The coarse
        // config key needs no build dir, so the PRE lookup hits and
        // materializes the recorded dir under its observed (tree-relative,
        // hence tree-independent) name.
        let tree_b = synthetic_project(&tmp, "b");
        assert!(!tree_b.join(".dart_tool").join("flutter_build").exists());
        let raw = raw_argv(&BUNDLE_ARGV);
        let mut runner = FakeRunner::exits(0);
        let report = run_shim(&raw, &tree_b, &config, &mut runner);

        assert_eq!(report.action, Action::PreHitMaterialized);
        assert_eq!(report.reason, "pre_hit");
        assert_eq!(report.exit_code, Some(0));
        assert_eq!(report.key.as_deref(), Some(key.as_str()));
        assert_eq!(report.skipped_dirs, 0);
        assert_eq!(runner.argv, raw, "the child gets the original argv");

        let dir_b = tree_b.join(".dart_tool").join("flutter_build").join(DIR_MAIN);
        let stamp = fs::read_to_string(dir_b.join("flutter_build.dill.stamp")).expect("stamp");
        assert!(
            stamp.contains(tree_b.to_str().expect("UTF-8 root")),
            "rewritten to the hit tree: {stamp}"
        );
        assert!(
            !stamp.contains(tree_a.to_str().expect("UTF-8 root")),
            "ingest-tree root gone: {stamp}"
        );
        assert_eq!(
            fs::read(dir_b.join("app.dill")).expect("materialized dill"),
            b"kernel payload bytes".to_vec()
        );
        // The rerun's build skipped everything (no new touches) and the
        // reproduced key spares a re-ingest; only the hit stamp moved.
        assert_eq!(report.ingest_files, 0);
        assert_eq!(store.stats().expect("stats").entries, 1);
        let manifest = store.lookup(&key).expect("lookup").expect("entry");
        assert!(manifest.last_hit_unix >= seeded_hit);
    }

    #[test]
    fn stale_source_change_keeps_coarse_key_and_delegates_validation() {
        let (_guard, tmp) = tempdir();
        let sdk = fake_flutter_sdk(&tmp, "3.47.1");
        let store_root = tmp.join("cas").join("v1");
        let config = config_for(&store_root, &sdk.join("bin").join("flutter"));

        // Tree A seeds the entry.
        let tree_a = synthetic_project(&tmp, "a");
        let mut runner = FakeRunner {
            code: 0,
            argv: Vec::new(),
            effect: Some(Box::new(|cwd| {
                build_fixture(cwd, DIR_MAIN);
            })),
        };
        let seeded = run_shim(&raw_argv(&BUNDLE_ARGV), &tree_a, &config, &mut runner);
        assert_eq!(seeded.action, Action::Ingested);
        let key = seeded.key.expect("seed key");

        // Tree B edited a SOURCE the coarse key cannot see (main.dart is
        // no provenance file): the config key is unchanged, so the hit
        // still materializes. Correctness is delegated to flutter_tools —
        // its content validation rebuilds what diverged; the cost of the
        // stale hit is one materialization, never a wrong artifact.
        let tree_b = synthetic_project(&tmp, "b");
        write(&tree_b.join("lib").join("main.dart"), b"void main() { edited(); }\n");
        let mut runner = FakeRunner::exits(0);
        let report = run_shim(&raw_argv(&BUNDLE_ARGV), &tree_b, &config, &mut runner);
        assert_eq!(report.action, Action::PreHitMaterialized);
        assert_eq!(report.key.as_deref(), Some(key.as_str()));
        assert!(tree_b
            .join(".dart_tool")
            .join("flutter_build")
            .join(DIR_MAIN)
            .join("app.dill")
            .is_file());
    }

    /// Spawns a short-lived REAL process whose argv IS an assemble
    /// invocation: a shell (two commands, so it never exec-optimizes
    /// itself away) that sleeps past several observer polls with the
    /// assemble tokens as its positional parameters. Blocks until the
    /// child exits — the fake "build" duration during which the
    /// observer polls the process table.
    #[cfg(unix)]
    fn run_assemble_lookalike(assemble_argv: &[String]) {
        let mut command = std::process::Command::new("/bin/sh");
        command.arg("-c").arg("sleep 1.5; true");
        for token in assemble_argv {
            command.arg(token);
        }
        let status = command.status().expect("spawn the assemble lookalike");
        assert!(status.success(), "the lookalike shell must exit cleanly");
    }

    /// A gradle-shaped assemble argv against the fake SDK's flutter
    /// binary and `output`.
    fn assemble_argv(flutter_bin: &Path, output: &str) -> Vec<String> {
        [
            flutter_bin.to_string_lossy().into_owned(),
            "--quiet".to_owned(),
            "assemble".to_owned(),
            "--no-version-check".to_owned(),
            "--depfile".to_owned(),
            "flutter_build.d".to_owned(),
            "--output".to_owned(),
            output.to_owned(),
            "-dTargetFile=lib/main.dart".to_owned(),
            "-dTargetPlatform=android".to_owned(),
            "-dBuildMode=release".to_owned(),
            "--DartDefines=QVBJPXByb2Q=".to_owned(),
            "-dAndroidArchs=android-arm64".to_owned(),
            "-dMinSdkVersion=21".to_owned(),
            "android_aot_bundle_release_android-arm64".to_owned(),
        ]
        .to_vec()
    }

    /// The flagship regression: the observer reads a REAL assemble-like
    /// child off the process table during the run, the entry carries the
    /// template, and a fresh tree replays the dir under the
    /// root-rewritten recomputed name.
    #[cfg(unix)]
    #[test]
    fn observer_captures_live_assemble_child_and_template_replays_across_trees() {
        let (_guard, tmp) = tempdir();
        let sdk = fake_flutter_sdk(&tmp, "3.47.1");
        let flutter_bin = sdk.join("bin").join("flutter");
        let engine_revision = key::fingerprint_toolchain(&sdk)
            .expect("fingerprint")
            .engine_revision;
        let store_root = tmp.join("cas").join("v1");
        let config = config_for(&store_root, &flutter_bin);

        // Tree A: the fake run's "build" spawns the assemble lookalike
        // and leaves the observed hash dir under the computed name.
        let tree_a = synthetic_project(&tmp, "a");
        let output_a = tree_a.join("build/app/intermediates/flutter/release");
        let argv_a = assemble_argv(&flutter_bin, &output_a.to_string_lossy());
        let template = base_capture(&argv_a, &tree_a, &engine_revision);
        let effect_template = template.clone();
        let effect_argv = argv_a.clone();
        let mut runner = FakeRunner {
            code: 0,
            argv: Vec::new(),
            effect: Some(Box::new(move |cwd| {
                run_assemble_lookalike(&effect_argv);
                build_fixture(cwd, &effect_template.name);
            })),
        };
        let report = run_shim(&raw_argv(&APK_ARGV), &tree_a, &config, &mut runner);
        assert_eq!(report.action, Action::Ingested);
        let key = report.key.clone().expect("post key");

        // The entry carries the template the observer captured.
        let store = CasStore::open(&store_root).expect("open store");
        let manifest = store.lookup(&key).expect("lookup").expect("entry");
        assert_eq!(manifest.dirs.len(), 1);
        let dir = &manifest.dirs[0];
        assert_eq!(dir.name_observed, template.name);
        assert_eq!(dir.name_rule, NameRule::RecomputeFromTemplate);
        assert_eq!(dir.name_defines, template.defines);
        assert_eq!(dir.name_output_dir, template.output_dir);

        // Tree B is fresh: the PRE hit recomputes the dir name from the
        // template with the output dir re-rooted at tree B — never the
        // ingest-tree name, which embeds tree A.
        let tree_b = synthetic_project(&tmp, "b");
        let mut runner = FakeRunner::exits(0);
        let report = run_shim(&raw_argv(&APK_ARGV), &tree_b, &config, &mut runner);
        assert_eq!(report.action, Action::PreHitMaterialized);
        let output_b = rewrite_output_dir(&template.output_dir, &manifest.project_root_at_ingest, &tree_b);
        let name_b = flutter_build_dir_name(&engine_revision, &template.defines, Path::new(&output_b));
        assert_ne!(name_b, template.name, "the name embeds the absolute output dir");
        let dir_b = tree_b.join(".dart_tool").join("flutter_build").join(&name_b);
        assert!(dir_b.join("app.dill").is_file(), "materialized under {}", dir_b.display());
        assert!(
            !tree_b.join(".dart_tool").join("flutter_build").join(&template.name).exists(),
            "the ingest-tree name never replays"
        );
        let stamp = fs::read_to_string(dir_b.join("flutter_build.dill.stamp")).expect("stamp");
        assert!(stamp.contains(tree_b.to_str().expect("UTF-8 root")), "{stamp}");
    }

    /// The real gradle defect shape, end to end: the decoy's argv carries
    /// a spaced multi-arch define, so the observer reads it space-split;
    /// the template that lands must be the FULL-value join variant,
    /// selected by the POST name verification against the dir the "build"
    /// materialized, and it replays across trees.
    #[cfg(unix)]
    #[test]
    fn spaced_define_value_lands_through_name_arbitration() {
        let (_guard, tmp) = tempdir();
        let sdk = fake_flutter_sdk(&tmp, "3.47.1");
        let flutter_bin = sdk.join("bin").join("flutter");
        let engine_revision = key::fingerprint_toolchain(&sdk)
            .expect("fingerprint")
            .engine_revision;
        let store_root = tmp.join("cas").join("v1");
        let config = config_for(&store_root, &flutter_bin);

        // The multi-arch tail of the default apk build, as a `ps` poll
        // splits it: `-dAndroidArchs=android-arm android-arm64
        // android-x64` arrives as three tokens.
        let tree_a = synthetic_project(&tmp, "a");
        let output_a = tree_a.join("build/app/intermediates/flutter/debug");
        let mut argv_a = assemble_argv(&flutter_bin, &output_a.to_string_lossy());
        let archs_at = argv_a
            .iter()
            .position(|token| token == "-dAndroidArchs=android-arm64")
            .expect("the archs define token");
        argv_a.splice(
            archs_at..archs_at + 1,
            [
                "-dAndroidArchs=android-arm".to_owned(),
                "android-arm64".to_owned(),
                "android-x64".to_owned(),
            ],
        );
        let variants =
            capture_templates(&argv_a, &tree_a, &engine_revision).expect("variants");
        let full = variants
            .iter()
            .find(|capture| {
                capture.defines.get("AndroidArchs").map(String::as_str)
                    == Some("android-arm android-arm64 android-x64")
            })
            .expect("the full-value variant");
        let base = base_capture(&argv_a, &tree_a, &engine_revision);
        assert_ne!(base.name, full.name, "the split parse and the true parse disagree");

        // Real flutter_tools parsed the UNSPLIT argv: the dir lands under
        // the full-value name, never the base variant's.
        let effect_name = full.name.clone();
        let effect_argv = argv_a.clone();
        let mut runner = FakeRunner {
            code: 0,
            argv: Vec::new(),
            effect: Some(Box::new(move |cwd| {
                run_assemble_lookalike(&effect_argv);
                build_fixture(cwd, &effect_name);
            })),
        };
        let report = run_shim(&raw_argv(&APK_ARGV), &tree_a, &config, &mut runner);
        assert_eq!(report.action, Action::Ingested);
        let key = report.key.clone().expect("post key");

        // Name arbitration picked the full-value template: the base
        // variant's name never materialized, so it is gone.
        let store = CasStore::open(&store_root).expect("open store");
        let manifest = store.lookup(&key).expect("lookup").expect("entry");
        assert_eq!(manifest.dirs.len(), 1);
        let dir = &manifest.dirs[0];
        assert_eq!(dir.name_observed, full.name);
        assert_eq!(dir.name_rule, NameRule::RecomputeFromTemplate);
        assert_eq!(dir.name_defines, full.defines);
        assert_eq!(dir.name_output_dir, full.output_dir);

        // A fresh tree replays it under the re-rooted name.
        let tree_b = synthetic_project(&tmp, "b");
        let mut rerun = FakeRunner::exits(0);
        let report = run_shim(&raw_argv(&APK_ARGV), &tree_b, &config, &mut rerun);
        assert_eq!(report.action, Action::PreHitMaterialized);
        let output_b =
            rewrite_output_dir(&full.output_dir, &manifest.project_root_at_ingest, &tree_b);
        let name_b =
            flutter_build_dir_name(&engine_revision, &full.defines, Path::new(&output_b));
        assert!(tree_b
            .join(".dart_tool")
            .join("flutter_build")
            .join(&name_b)
            .join("app.dill")
            .is_file());
    }

    /// An observed assemble whose dir never materialized (a failed
    /// assemble) contributes nothing; an uncovered dir degrades to
    /// not-replayable provenance and never replays.
    #[cfg(unix)]
    #[test]
    fn template_without_dir_dropped_and_unknown_dir_not_replayable() {
        let (_guard, tmp) = tempdir();
        let sdk = fake_flutter_sdk(&tmp, "3.47.1");
        let flutter_bin = sdk.join("bin").join("flutter");
        let engine_revision = key::fingerprint_toolchain(&sdk)
            .expect("fingerprint")
            .engine_revision;
        let store_root = tmp.join("cas").join("v1");
        let config = config_for(&store_root, &flutter_bin);

        let tree = synthetic_project(&tmp, "a");
        let orphan_argv =
            assemble_argv(&flutter_bin, "/opt/nowhere/failed");
        let orphan = base_capture(&orphan_argv, &tree, &engine_revision);
        let observed = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let mut runner = FakeRunner {
            code: 0,
            argv: Vec::new(),
            effect: Some(Box::new(move |cwd| {
                // The lookalike runs (and is observed) but its dir never
                // appears; only an uncovered dir materializes.
                run_assemble_lookalike(&orphan_argv);
                build_fixture(cwd, observed);
            })),
        };
        let report = run_shim(&raw_argv(&APK_ARGV), &tree, &config, &mut runner);
        assert_eq!(report.action, Action::Ingested);
        let key = report.key.expect("key");

        let store = CasStore::open(&store_root).expect("open store");
        let manifest = store.lookup(&key).expect("lookup").expect("entry");
        assert_eq!(manifest.dirs.len(), 1, "the orphan template was dropped");
        assert_eq!(manifest.dirs[0].name_observed, observed);
        assert_ne!(manifest.dirs[0].name_observed, orphan.name);
        assert_eq!(manifest.dirs[0].name_rule, NameRule::NotReplayable);
        assert!(manifest.dirs[0].name_defines.is_empty());

        // A hit on that entry materializes nothing: the dir's name is not
        // replayable in another tree, and the key is still the ingest key.
        // (The rerun goes to a FRESH tree: its build root does not exist,
        // so POST deterministically sees no touched dirs.)
        let tree_b = synthetic_project(&tmp, "b");
        let mut rerun = FakeRunner::exits(0);
        let report = run_shim(&raw_argv(&APK_ARGV), &tree_b, &config, &mut rerun);
        assert_eq!(report.action, Action::PreMiss);
        assert_eq!(report.reason, "entry_not_replayable");
        assert_eq!(report.key.as_deref(), Some(key.as_str()));
        assert_eq!(report.skipped_dirs, 1);
        assert!(!tree_b.join(".dart_tool").join("flutter_build").exists());
    }

    /// One replayable assemble dir (observed template + dir) and one
    /// run-internal dir without a template: the hit materializes the
    /// first and skips the second.
    #[cfg(unix)]
    #[test]
    fn mixed_dirs_hit_materializes_replayable_dir_only() {
        let (_guard, tmp) = tempdir();
        let sdk = fake_flutter_sdk(&tmp, "3.47.1");
        let flutter_bin = sdk.join("bin").join("flutter");
        let engine_revision = key::fingerprint_toolchain(&sdk)
            .expect("fingerprint")
            .engine_revision;
        let store_root = tmp.join("cas").join("v1");
        let config = config_for(&store_root, &flutter_bin);

        let tree_a = synthetic_project(&tmp, "a");
        let output = tree_a.join("build/app/intermediates/flutter/release");
        let argv = assemble_argv(&flutter_bin, &output.to_string_lossy());
        let template = base_capture(&argv, &tree_a, &engine_revision);
        let effect_template = template.clone();
        let effect_argv = argv.clone();
        let internal = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let mut runner = FakeRunner {
            code: 0,
            argv: Vec::new(),
            effect: Some(Box::new(move |cwd| {
                run_assemble_lookalike(&effect_argv);
                build_fixture(cwd, &effect_template.name);
                build_fixture(cwd, internal);
            })),
        };
        let report = run_shim(&raw_argv(&APK_ARGV), &tree_a, &config, &mut runner);
        assert_eq!(report.action, Action::Ingested);
        let key = report.key.expect("key");
        let store = CasStore::open(&store_root).expect("open store");
        let manifest = store.lookup(&key).expect("lookup").expect("entry");
        assert_eq!(manifest.dirs.len(), 2);
        let by_rule = |rule: NameRule| {
            manifest
                .dirs
                .iter()
                .find(|dir| dir.name_rule == rule)
                .expect("dir of that rule")
        };
        assert_eq!(by_rule(NameRule::RecomputeFromTemplate).name_observed, template.name);
        assert_eq!(by_rule(NameRule::NotReplayable).name_observed, internal);

        // The fresh tree's hit materializes the replayable dir and skips
        // the internal one; the action stays a materialized pre-hit.
        let tree_b = synthetic_project(&tmp, "b");
        let mut runner = FakeRunner::exits(0);
        let report = run_shim(&raw_argv(&APK_ARGV), &tree_b, &config, &mut runner);
        assert_eq!(report.action, Action::PreHitMaterialized);
        assert_eq!(report.skipped_dirs, 1);
        let output_b = rewrite_output_dir(&template.output_dir, &manifest.project_root_at_ingest, &tree_b);
        let name_b = flutter_build_dir_name(&engine_revision, &template.defines, Path::new(&output_b));
        assert!(tree_b
            .join(".dart_tool")
            .join("flutter_build")
            .join(&name_b)
            .join("app.dill")
            .is_file());
        assert!(!tree_b
            .join(".dart_tool")
            .join("flutter_build")
            .join(internal)
            .exists());
    }

    /// A user-run `flutter assemble` is an unmodeled command: a plain
    /// passthrough with no store access and no tree side effects (the
    /// observation capture belongs to the OUTER shim of a
    /// `flutter build|run`, which this invocation is not).
    #[test]
    fn assemble_argv_runs_as_a_plain_passthrough() {
        let (_guard, tmp) = tempdir();
        let sdk = fake_flutter_sdk(&tmp, "3.47.1");
        let tree = synthetic_project(&tmp, "a");
        let store_root = tmp.join("cas").join("v1");
        let config = config_for(&store_root, &sdk.join("bin").join("flutter"));

        let raw = raw_argv(&["assemble", "-o", "/opt/out", "-dBuildMode=release"]);
        let mut runner = FakeRunner::exits(0);
        let report = run_shim(&raw, &tree, &config, &mut runner);
        assert_eq!(report.action, Action::Passthrough);
        assert_eq!(report.reason, "unmodeled_command");
        assert_eq!(report.exit_code, Some(0));
        assert_eq!(runner.argv, raw);
        assert!(!tree.join(".dart_tool").join("dartq-bcache").exists());
        assert!(!store_root.exists(), "no store side effects");
    }

    #[test]
    fn unsupported_flutter_version_passes_through_with_store_untouched() {
        let (_guard, tmp) = tempdir();
        let sdk = fake_flutter_sdk(&tmp, "3.50.0");
        let tree = synthetic_project(&tmp, "app");
        let store_root = tmp.join("cas").join("v1");
        let config = config_for(&store_root, &sdk.join("bin").join("flutter"));
        let mut runner = FakeRunner::exits(7);
        let report = run_shim(&raw_argv(&BUNDLE_ARGV), &tree, &config, &mut runner);

        assert_eq!(report.action, Action::Passthrough);
        assert_eq!(report.reason, "unsupported_flutter_version");
        assert_eq!(report.exit_code, Some(7), "the child's exit code wins");
        assert!(report.key.is_none());
        assert!(!store_root.exists(), "no store directory is created");
    }

    #[test]
    fn provenance_change_changes_the_pre_key() {
        let (_guard, tmp) = tempdir();
        let sdk = fake_flutter_sdk(&tmp, "3.47.1");
        let tree = synthetic_project(&tmp, "app");
        let config = config_for(&tmp.join("cas").join("v1"), &sdk.join("bin").join("flutter"));

        // A non-zero exit isolates the PRE key (POST never runs), and the
        // coarse key computes with no build dir in the tree at all.
        let mut first = FakeRunner::exits(1);
        let report = run_shim(&raw_argv(&BUNDLE_ARGV), &tree, &config, &mut first);
        assert_eq!(report.reason, "pre_miss");
        let key = report.key.expect("pre key");

        write(&tree.join("pubspec.yaml"), b"name: app\nversion: 1.0.1\n");
        let mut second = FakeRunner::exits(1);
        let report = run_shim(&raw_argv(&BUNDLE_ARGV), &tree, &config, &mut second);
        assert_ne!(report.key.as_deref(), Some(key.as_str()));
    }

    #[test]
    fn nonzero_run_exit_skips_ingest() {
        let (_guard, tmp) = tempdir();
        let sdk = fake_flutter_sdk(&tmp, "3.47.1");
        let tree = synthetic_project(&tmp, "app");
        let store_root = tmp.join("cas").join("v1");
        let config = config_for(&store_root, &sdk.join("bin").join("flutter"));
        let mut runner = FakeRunner {
            code: 1,
            argv: Vec::new(),
            effect: Some(Box::new(|cwd| {
                build_fixture(cwd, DIR_MAIN);
            })),
        };
        let report = run_shim(&raw_argv(&BUNDLE_ARGV), &tree, &config, &mut runner);

        assert_eq!(report.action, Action::PreMiss);
        assert_eq!(report.exit_code, Some(1));
        assert_eq!(report.ingest_files, 0);
        // The PRE lookup opened (and skeleton-created) the store; a
        // failed build leaves it empty.
        let stats = CasStore::open(&store_root)
            .expect("open store")
            .stats()
            .expect("stats");
        assert_eq!(stats.entries, 0, "a failed build never stores an entry");
        assert_eq!(stats.blobs, 0, "a failed build never stores a blob");
    }

    #[cfg(unix)]
    #[test]
    fn path_resolution_skips_the_shim_directory() {
        let (_guard, tmp) = tempdir();
        let shim_dir = tmp.join("shims");
        let real_dir = tmp.join("sdk").join("bin");
        write(&shim_dir.join("flutter"), b"#!/bin/sh\n# the shim\n");
        write(&real_dir.join("flutter"), b"#!/bin/sh\n# the real one\n");
        let exclude_dir = fs::canonicalize(&shim_dir).expect("canonicalize");

        let found = first_flutter_on_path(
            &[shim_dir.clone(), tmp.join("empty"), real_dir.clone()],
            Some(&exclude_dir),
            None,
        )
        .expect("the real binary is found");
        assert_eq!(found, fs::canonicalize(real_dir.join("flutter")).expect("canonicalize"));

        // A symlink to the shim binary in another directory is skipped
        // by file identity, not just by directory.
        let alias_dir = tmp.join("alias");
        fs::create_dir_all(&alias_dir).expect("mkdir alias");
        std::os::unix::fs::symlink(shim_dir.join("flutter"), alias_dir.join("flutter"))
            .expect("symlink alias");
        let shim_exe = fs::canonicalize(shim_dir.join("flutter")).expect("canonicalize");
        let found = first_flutter_on_path(
            &[alias_dir, real_dir.clone()],
            None,
            Some(&shim_exe),
        )
        .expect("the alias is skipped, the real one found");
        assert_eq!(found, fs::canonicalize(real_dir.join("flutter")).expect("canonicalize"));

        assert_eq!(
            first_flutter_on_path(&[shim_dir], Some(&exclude_dir), None),
            None
        );
    }

    #[test]
    fn fingerprint_cache_roundtrips_and_invalidates_on_change() {
        let (_guard, tmp) = tempdir();
        let sdk = fake_flutter_sdk(&tmp, "3.47.1");
        let store_root = tmp.join("cas").join("v1");

        let first = cached_fingerprint(&sdk, &store_root).expect("fingerprint");
        store_fingerprint_cache(&store_root, &sdk, &first);
        assert!(store_root.join("meta").join("toolchain.json").is_file());
        let second = cached_fingerprint(&sdk, &store_root).expect("fingerprint");
        assert_eq!(second, first);

        // A changed artifact invalidates the cache: the size differs.
        write(
            &sdk.join("bin")
                .join("cache")
                .join("dart-sdk")
                .join("lib")
                .join("_internal")
                .join("vm_platform_strong.dill"),
            b"platform dill bytes, changed",
        );
        let third = cached_fingerprint(&sdk, &store_root).expect("fingerprint");
        assert_ne!(third.platform_strong_dill, first.platform_strong_dill);
    }

    #[test]
    fn dart_defines_inject_toolchain_set_in_doc_order() {
        let (_guard, tmp) = tempdir();
        let sdk = fake_flutter_sdk(&tmp, "3.47.1");
        // `build apk`'s grammar carries flavor, build name/number, and
        // dart defines (`build bundle` declares none of the first two).
        let argv = [
            "build",
            "apk",
            "--flavor",
            "free",
            "--build-name",
            "1.2.3",
            "--build-number",
            "42",
            "--dart-define",
            "API=prod",
        ];
        let parsed = parse_flutter_args(&raw_argv(&argv).iter().map(|t| t.to_string_lossy().into_owned()).collect::<Vec<_>>())
            .expect("parse");
        let toolchain = key::fingerprint_toolchain(&sdk).expect("fingerprint");
        let ordered = dart_define_vector(&parsed, &sdk, &toolchain);
        let rendered: Vec<(&str, &str)> = ordered
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_str()))
            .collect();
        assert_eq!(
            rendered,
            [
                ("FLUTTER_APP_FLAVOR", "free"),
                ("FLUTTER_BUILD_NAME", "1.2.3"),
                ("FLUTTER_BUILD_NUMBER", "42"),
                ("FLUTTER_VERSION", "3.47.1"),
                ("FLUTTER_CHANNEL", "stable"),
                ("FLUTTER_FRAMEWORK_REVISION", "frameworkrevision000"),
                ("FLUTTER_DART_VERSION", "3.13.7"),
                ("FLUTTER_ENGINE_REVISION", "enginerevision0000000000000000"),
                ("API", "prod"),
            ]
        );
        // A user define replaces the injected value in place.
        let over = parse_flutter_args(
            &["build", "bundle", "--dart-define", "FLUTTER_BUILD_NAME=9.9.9"]
                .iter()
                .map(|token| (*token).to_owned())
                .collect::<Vec<_>>(),
        )
        .expect("parse");
        assert!(dart_define_vector(&over, &sdk, &toolchain)
            .contains(&("FLUTTER_BUILD_NAME".to_owned(), "9.9.9".to_owned())));
    }
}
