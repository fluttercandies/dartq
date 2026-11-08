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
//! 1. **Version gate**: the toolchain fingerprint's version stamp must
//!    name a line of [`SUPPORTED_FLUTTER_LINES`] — the args/option
//!    tables and the build-dir name formula are pinned to those lines'
//!    flutter_tools. Anything else passes through before the store is
//!    ever touched.
//! 2. **PRE**: compute the coarse config key (toolchain, filtered define
//!    vector, target, provenance file contents — no depfile closure and
//!    no file that exists only after a build, so a fresh worktree keys
//!    identically to the tree the entry was ingested from), look it up
//!    in the CAS, and on a hit materialize every replayable
//!    [`DirRecord`] of the entry into this tree — verbatim for
//!    tree-relative names, recomputed from the recorded template with
//!    the output dir AND define values re-aimed at this tree otherwise
//!    (a define carrying an absolute path under the worktree — Xcode's
//!    `-dSrcRoot`, an absolute `-dTargetFile` — hashes differently
//!    here, so the recomputed name must follow it), the `build bundle`
//!    asset dir into this invocation's asset dir, and the
//!    `flutter test` kernel cache into the build dir's `test_cache` —
//!    where a cross-tree hit rewrites the dill's embedded `file://` URI
//!    prefixes to this tree (see [`crate::dill_rewrite`]) so
//!    frontend_server's initialize-from-dill validation keeps the
//!    kernel.
//!
//!    The rewrite anchor is the WORKTREE root of each tree (git
//!    toplevel), not the project root: a melos/pub-workspace sibling
//!    `path:` package lives inside the worktree but outside the
//!    project, and an un-rewritten reference to the ingest tree's copy
//!    would let flutter's input validation skip on a file the target
//!    tree never built — tree B running tree A's compiled code. Dirs
//!    whose recorded bytes mention paths under NEITHER tree's
//!    worktree root NOR the machine-local cache roots (pub cache,
//!    flutter root) are refused by cross-tree replays
//!    ([`crate::types::PathAnchors`]); within-tree replay is unaffected.
//!    Placement itself is fill-only-missing with blobs verified present
//!    up front (see [`crate::materialize`]).
//!
//!    Content divergence the coarse key cannot see is delegated to
//!    flutter_tools — and that delegation is only sound for paths that
//!    resolve identically in both trees, which is exactly what the
//!    anchor rule guarantees: every build re-validates a dir's recorded
//!    input hashes and rebuilds on mismatch, and frontend_server
//!    invalidates a stale initialize-from-dill dill, so a wasted
//!    materialization is the entire cost of a stale hit. Any pre-phase
//!    failure degrades to a miss: the shim never blocks a build.
//! 3. **RUN**: spawn flutter with the ORIGINAL argv (never a lossy
//!    copy) while an assemble [`CaptureObserver`] polls the process
//!    table for the `flutter assemble` invocations the platform build
//!    systems spawn behind the shim (they invoke the SDK's absolute
//!    binary path, so no PATH interception can see them). A non-zero
//!    exit skips the build faces' POST; the test face still advances
//!    its kernel cache (see 4).
//! 4. **POST**: ingest EVERY dir the run touched, each as one
//!    [`DirRecord`] — hash dirs with name and template from an observed
//!    assemble invocation, or verbatim-replayable on the `build bundle`
//!    face, or not-replayable provenance otherwise — under the same
//!    coarse key; a `build bundle` run adds its asset output dir
//!    (`copy_flutter_bundle`'s outputs, recorded whenever present so a
//!    fresh tree's copy target skips too), and a `flutter test` run
//!    additionally ingests the
//!    fixed-path kernel-cache dir (`<buildDir>/test_cache`; after a
//!    rewritten cross-tree hit, flutter rewrites the dill with this
//!    tree's URIs, and the re-ingest converges the entry onto it — no
//!    special-casing needed). The test face ingests that dir even on a
//!    non-zero exit: `flutter test` exits non-zero both when
//!    compilation fails and when a compiled suite is red, and the
//!    scan's touch gate separates the two — the cache dill is only
//!    written after a successful compile, so a touched dill is valid
//!    kernel however red the suite, while a compile failure writes
//!    nothing. Every other face stays exit-0-gated (a failed build may
//!    leave partially-written flutter_build state); signal deaths
//!    arrive as 128 + signal and take the test-face path, where at
//!    worst a torn in-place dill copy is bounded by frontend_server's
//!    fall-back to a full compile when the initialize-from-dill file
//!    will not read. A file whose identity this invocation's
//!    materialization placed, the tree's placement record remembers
//!    from an earlier one ([`crate::placement`]), or the prior entry
//!    under the key stamped, is re-referenced without reading (see
//!    [`crate::ingest_scan::scan_entry_files`]) — a warm run stores only
//!    what flutter actually changed, in any tree the entry was ever
//!    replayed into. A hit whose rerun reproduced the
//!    key AND touched nothing needs no re-ingest, and a rerun whose
//!    outputs reproduce the stored snapshot byte for byte (flutter's
//!    every-build rewrites of `.filecache` and `outputs.json` included —
//!    `.filecache` up to the order a hash map persists it in; a
//!    cross-tree hit's re-rooted rendering of it counts) publishes
//!    nothing — the manifest stays the shared source, as ingested, and
//!    each tree's identities live in that tree's record; a tree whose
//!    first run did republish the entry under its own root (flutter
//!    regenerated what it could not validate) moves the frame once,
//!    and no later run in any tree moves it back while its bytes are
//!    the ones its record claims ([`frame_only`]); a rerun that
//!    changed outputs always
//!    re-ingests, so a hit can never freeze a stale entry. Then reclaim
//!    the blobs the re-ingest detached from the entry (the test face's
//!    kernel changes bytes on every run, so without this the store would
//!    grow one kernel per run — see [`CasStore::reclaim_detached`]), or
//!    run the full gc when the store is over budget.
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

use crate::args_model::{
    ArgModelError, ParsedFlutterCmd, flutter_build_dir_name, parse_flutter_args,
};
use crate::assemble::AssembleCapture;
use crate::error::Result;
#[cfg(unix)]
use crate::fsutil::TreeLock;
use crate::fsutil::{file_blake3, stamp_of};
use crate::ingest_scan::{
    DirScan, ScanSkips, find_touched_hash_dirs, scan_entry_files, scan_test_cache,
};
use crate::key;
use crate::materialize::{self, DirPlan, MaterializePlan, MaterializeReport};
use crate::observer::CaptureObserver;
use crate::placement::{self, PlacedDir, PlacementRecord};
use crate::store::CasStore;
use crate::types::{
    BlobId, ClosureFile, DirRecord, EntryManifest, FileIdentity, KeyInputs, ManifestFile, NameRule,
    PathAnchors, ToolchainFingerprint,
};

/// The flutter version lines whose flutter_tools this crate's embedded
/// model generation was derived from and verified against — the option
/// tables ([`crate::args_model`]), the `flutter assemble` argv model
/// ([`crate::assemble`]), the build-dir name formula
/// ([`crate::args_model::flutter_build_dir_name`]), and the bookkeeping
/// contract those models cite (`.filecache` v2, the every-build
/// rewrites, the test kernel-cache path; see [`crate::materialize`] and
/// [`crate::ingest_scan`]). Every other version passes through
/// untouched.
///
/// An entry names the MINOR line it covers — `(3, 44)` admits 3.44.0
/// and any later 3.44.x patch. A line lands here only after the pinned
/// flutter_tools regions of its reviewed patches were classified hunk
/// by hunk against the model. The harness pins every reviewed patch's
/// region digest (`REVIEWED_LINES` in the tests, one or more pins per
/// line) and its `supported_line_patches_match_their_reviewed_regions`
/// test fails for any installed patch of an enabled line that matches
/// none of its line's pins — patch releases do carry fixes into these
/// regions (3.24.0 → 3.24.5 changed `build web` source-map plumbing),
/// so an unreviewed patch is admitted on its line's evidence until the
/// tripwire sees it installed.
/// Cross-line safety needs no table discipline: the toolchain
/// fingerprint keys the full version stamp AND both hashed toolchain
/// artifacts, so patches of one line share the MODEL but never a stored
/// entry.
///
/// Verification evidence, per line. Drift classes: A — an option the
/// model knows is gone (flutter rejects the argv, so no entry can exist
/// under its key); B — an option the model does not know (kept verbatim
/// in the key); C — the semantics of a modeled option; D — a
/// bookkeeping or naming contract; E — the `package:args` parser.
///
/// * `3.47` — 3.47.0 and 3.47.1: all 21 regions byte-identical to the
///   3.47.1 baseline the model was derived from.
/// * `3.44` (3.44.0), `3.41` (3.41.9), `3.38` (3.38.1), `3.35` (3.35.0),
///   `3.32` (3.32.0) — every hunk of each region diff against the next
///   newer line classified; `package:args` 2.7.0 in every pubspec (no E):
///   * A/B only on the option surface: no model-known option changes
///     arity, negatability, abbreviation, or role, and no added option
///     takes a model-known spelling (`-D` abbreviates nothing before
///     3.38, so flutter rejects it there);
///   * C: before 3.41 an empty `--dart-define=` reaches the compilers as
///     a bare `-D` (the model keys it verbatim, see
///     [`crate::args_model::ParsedFlutterCmd::defines`]); before 3.35
///     `FLUTTER_APP_FLAVOR`, before 3.47 `FLUTTER_BUILD_NAME`/`_NUMBER`
///     are not injected, so a user define of that name and value keys
///     like the model's injection — every face hashes the full define
///     list into the name flutter looks up (build-dir names, the test
///     kernel-cache name), so the merge can only miss;
///   * D: the `.filecache` v2 store, stamps, `outputs.json` and
///     `.last_build_id` cleanup, the build-dir name formula, the
///     test-cache path, and `getBuildDirectory` are unchanged. Before
///     3.41 the assemble define map differs (3.38 parses raw
///     `--dart-define` values as `K=V` defines, 3.35 and older fold them
///     into `DartDefines` only): those templates fail the POST name
///     check and their dirs record as not replayable — a miss. Kernel
///     formats 124–130 share format 138's component layout
///     ([`crate::dill_rewrite`]), and a payload dill the rewriter
///     refuses is left out of a cross-tree replay
///     ([`crate::materialize`]).
///
/// The latest patch of each line carries its own pin after the same
/// hunk-by-hunk review of its region diff against the line's baseline
/// patch (the fleet keeps only latest patches installed; every hunk
/// below is the complete drift — no other region moved):
///
/// * `3.32.8` — `flutter_command.dart` backports 3.35.0's
///   `FLUTTER_APP_FLAVOR` injection block byte-identically (the model
///   already keys exactly that injection, so 3.32.8 converges onto the
///   model instead of away from it; a pubspec `default-flavor` rides
///   the keyed pubspec closure) and rejects a `FLUTTER_APP_FLAVOR`
///   user define or env var with `throwToolExit` — a narrowing of the
///   accepted surface: no entry can exist under an argv the patch
///   itself rejects, and cross-patch entries never share a key (the
///   version-stamped toolchain). Pubspec pins only; `args` stays 2.7.0.
/// * `3.35.7` — `build web` without `--wasm`: the extra
///   `WasmCompilerConfig` is added only under `--wasm-dry-run` (whose
///   default is true, so the default build is unchanged);
///   `--no-wasm-dry-run` — a keyed option, kept verbatim in the key —
///   stops compiling a stray full wasm target it never asked for. The
///   option declarations are byte-identical, the build-dir name formula
///   is untouched, and the web faces are capture-not-predict. Pubspec
///   pins only; `args` stays 2.7.0.
/// * `3.38.10` — an empty `--dart-define=` value is dropped instead of
///   forwarded (the model keeps the token verbatim in the key — the
///   fail-safe over-keying side: two argvs that now build identically
///   simply never share an entry); `assemble` skips an empty define
///   chunk it used to reject, with any name-recompute mismatch bounded
///   by the POST name check to a not-replayable miss; `run.dart` is a
///   nullability annotation and `build_system.dart` adds
///   `Environment.copyWith` (sole caller: the native-assets hooks
///   runner; the hash-dir name formula unchanged). No pubspec drift.
/// * `3.44.9` — all 21 regions byte-identical to the reviewed 3.44.0
///   (same digest, pinned under both patch names).
/// * `3.47.9` — `toEnvironmentConfig` adds `BUILD_NAME`/`BUILD_NUMBER`
///   for the generated CMake/xcconfig inputs (linux/windows/xcode
///   regenerate them on every build, and `--build-name`/`--build-number`
///   are keyed options whose values also ride the model's define
///   vector); `toBuildSystemEnvironment` — the define map the dir names
///   hash — is unchanged. Pubspec pins only; `args` stays 2.7.0.
///
/// Everything older passes through: 3.29.x (3.29.0 and 3.29.3 are
/// region-identical) and older were surveyed, not classified hunk by
/// hunk, and their kernel formats (122 and older) add a component-index
/// field the dill rewriter does not model.
pub const SUPPORTED_FLUTTER_LINES: &[(u32, u32)] =
    &[(3, 32), (3, 35), (3, 38), (3, 41), (3, 44), (3, 47)];

/// The (major, minor) line of a toolchain version stamp, when the stamp
/// is a plain `MAJOR.MINOR.PATCH` — non-empty decimal components and
/// nothing else. This is exactly the shape stable `frameworkVersion`
/// values carry (the stamp [`crate::key::fingerprint_toolchain`] reads
/// from `bin/cache/flutter.version.json`; a stamp with a pre-release or
/// build suffix, an `unknown`, or any malformed spelling matches no
/// line and the invocation passes through.
fn version_line(stamp: &str) -> Option<(u32, u32)> {
    fn number(part: Option<&str>) -> Option<u32> {
        let part = part?;
        (!part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))
            .then(|| part.parse().ok())
            .flatten()
    }
    let mut parts = stamp.split('.');
    let line = (number(parts.next())?, number(parts.next())?);
    number(parts.next())?;
    (parts.next().is_none()).then_some(line)
}

/// Whether `stamp` names a line of [`SUPPORTED_FLUTTER_LINES`] — the
/// shim's model gate.
#[must_use]
pub fn supported_flutter_version(stamp: &str) -> bool {
    version_line(stamp).is_some_and(|line| SUPPORTED_FLUTTER_LINES.contains(&line))
}

/// Everything the shim needs beyond the invocation itself.
#[derive(Debug, Clone)]
pub struct ShimConfig {
    /// CAS store root (default `$XDG_CACHE_HOME/dartq/cas`, falling
    /// back to `~/.cache/dartq/cas`; assembled by the config glue).
    pub store_root: PathBuf,
    /// Store size budget (default 96 GiB); gc runs when exceeded, or
    /// sooner when the store's volume runs short of its free-space reserve.
    pub budget_bytes: u64,
    /// Seconds after a hit during which an entry is unevictable.
    pub grace_secs: u64,
    /// The resolved flutter binary this invocation keys and spawns.
    /// `None` (discovery failed upstream) is a passthrough.
    pub flutter_bin: Option<PathBuf>,
    /// `memory.max_percent`: the machine-wide memory ledger's share the
    /// build's lease draws from.
    pub memory_max_percent: u8,
    /// `memory.max_concurrent`: the job cap a batch command's slot
    /// declares (`0` unlimited).
    pub memory_max_concurrent: u32,
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
    /// A modeled run ingested its dirs — every face on a zero exit,
    /// the test kernel cache alone on a non-zero one (the exit-code
    /// field carries the run's own outcome untouched).
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
    /// Files the post-run ingest read (hashed or verified) into the
    /// store — the `ingested <key> (<n> files)` line's `n`. A file
    /// re-referenced through a placed or prior identity is not counted;
    /// the count is deterministic: zero means the run changed nothing
    /// the store did not already hold.
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
    /// child's exit code. On the production runner the terminal signals
    /// are forwarded to the child while it runs (see [`CommandRunner`]).
    fn run(&mut self, argv: &[OsString], cwd: &Path) -> io::Result<i32>;

    /// The passthrough face: run flutter with NOTHING around it. The
    /// production runner replaces this process with the child
    /// (`exec(2)` — audit A11: no second process, signals reach flutter
    /// directly, 6–9 ms and 9–11 MiB RSS saved); a runner that cannot
    /// replace itself falls back to [`FlutterRunner::run`]. `Ok` never
    /// returns on an exec-ing runner — the process IS the child by then.
    fn exec(&mut self, argv: &[OsString], cwd: &Path) -> io::Result<i32> {
        self.run(argv, cwd)
    }
}

/// The tag [`crate::orchestrator`]'s CLI glue uses to recognize a shim
/// invocation. It must never ride into a child environment: a nested
/// `dartq` that inherits it would mistake itself for a shim (audit
/// A10), so every spawn here strips it.
const SHIM_ENV: &str = "DARTQ_SHIM";

/// The production [`FlutterRunner`]. The child stays in the shim's
/// (foreground) process group — creating a separate group would sever
/// Ctrl-C from it — and spawns with no environment tagging beyond
/// stripping [`SHIM_ENV`]: the nested assemble invocations it triggers
/// are captured from the process table, not by the child's cooperation.
///
/// While the child runs, SIGTERM/SIGHUP/SIGINT are forwarded to it
/// (audit A11: without forwarding, a signal to the shim killed only the
/// shim and orphaned flutter_tools under PPID 1). The mechanism is a
/// minimal sigaction forwarder, not the signal-hook crate: three
/// signals need no pipe machinery, and the block-spawn-unblock sequence
/// below closes the fork race that a handler-only approach leaves open.
/// The shim then reports the child's own exit status (128 + signal for
/// a signal death), so a `kill` of the shim tears down the build and
/// the exit code still tells the truth.
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
        #[cfg(unix)]
        {
            let forwarders = forward::install();
            let mut command = Command::new(&self.flutter);
            command.args(argv).current_dir(cwd).env_remove(SHIM_ENV);
            // The platform builds flutter spawns run `flutter assemble`
            // back through the shim: they join this invocation's slot.
            dartq_toolchain::ledger::inherit(&mut command);
            let result = (|| -> io::Result<i32> {
                let mut child = forward::spawn_blocked(&mut command)?;
                let status = child.wait()?;
                forward::child_gone();
                Ok(exit_code(status))
            })();
            drop(forwarders);
            result
        }
        #[cfg(not(unix))]
        {
            let status = Command::new(&self.flutter)
                .args(argv)
                .current_dir(cwd)
                .env_remove(SHIM_ENV)
                .status()?;
            Ok(exit_code(status))
        }
    }

    fn exec(&mut self, argv: &[OsString], cwd: &Path) -> io::Result<i32> {
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            // A successful exec never returns; the Err it returns IS
            // the exec failure.
            Err(Command::new(&self.flutter)
                .args(argv)
                .current_dir(cwd)
                .env_remove(SHIM_ENV)
                .exec())
        }
        #[cfg(not(unix))]
        {
            self.run(argv, cwd)
        }
    }
}

/// The SIGTERM/SIGHUP/SIGINT forwarder for the cacheable face (audit
/// A11). Small by design: one static pid, one async-signal-safe
/// handler, sigaction install/restore, and a block-spawn-unblock spawn
/// that makes a signal arriving anywhere between fork and pid-publish
/// stay pending until the pid is live (the handler would otherwise
/// race the spawn and silently drop the signal). Residual risk,
/// documented: a signal landing in the two-instruction window after
/// `wait` reaps the child but before the pid is cleared forwards to a
/// dead pid — `kill` returns ESRCH and nothing else happens unless the
/// kernel recycled the pid in that window, which requires a full pid
/// wrap between two adjacent instructions.
#[cfg(unix)]
mod forward {
    use std::io;
    use std::os::unix::process::CommandExt;
    use std::process::{Child, Command};
    use std::sync::atomic::{AtomicI32, Ordering};

    /// The signals a terminal or supervisor sends to stop the shim.
    const FORWARDED: [libc::c_int; 3] = [libc::SIGTERM, libc::SIGHUP, libc::SIGINT];

    /// The child the forwarder targets; 0 = no child in flight.
    static CHILD_PID: AtomicI32 = AtomicI32::new(0);

    extern "C" fn handler(signal: libc::c_int) {
        let pid = CHILD_PID.load(Ordering::SeqCst);
        if pid != 0 {
            // kill(2) is async-signal-safe; the child shares the shim's
            // process group, so a Ctrl-C from the terminal reaches it
            // directly and this forward is the second, redundant
            // delivery — flutter_tools' documented response to a
            // repeated SIGINT is the forced shutdown a repeated Ctrl-C
            // asks for.
            unsafe { libc::kill(pid, signal) };
        }
    }

    /// The installed forwarders. Dropping restores the previous
    /// dispositions (normally the defaults), so nothing the shim does
    /// outlives the child it spawned.
    pub(super) struct Forwarders {
        saved: [libc::sigaction; FORWARDED.len()],
    }

    pub(super) fn install() -> Forwarders {
        let mut saved = unsafe { std::mem::zeroed::<[libc::sigaction; 3]>() };
        for (slot, signal) in saved.iter_mut().zip(FORWARDED) {
            // SAFETY: sigaction(2) reads the new action and writes the
            // previous one; both live in this stack frame and outlive
            // the call. A failure leaves the default disposition in
            // place — the process then dies on the signal as it would
            // without this module, never worse.
            unsafe {
                let mut action: libc::sigaction = std::mem::zeroed();
                libc::sigemptyset(&mut action.sa_mask);
                action.sa_sigaction = handler as *const () as usize;
                action.sa_flags = libc::SA_RESTART;
                libc::sigaction(signal, &action, slot);
            }
        }
        Forwarders { saved }
    }

    impl Drop for Forwarders {
        fn drop(&mut self) {
            for (saved, signal) in self.saved.iter().zip(FORWARDED) {
                // SAFETY: as install(), restoring the saved action.
                unsafe { libc::sigaction(signal, saved, std::ptr::null_mut()) };
            }
        }
    }

    /// Spawns `command` with [`FORWARDED`] blocked in the parent,
    /// publishes the child's pid, then restores the previous mask: a
    /// pending signal is delivered exactly then, with the pid live.
    /// The child unblocks before exec — an inherited blocked mask would
    /// swallow flutter's own Ctrl-C.
    pub(super) fn spawn_blocked(command: &mut Command) -> io::Result<Child> {
        // SAFETY: the sigset_t dance below only touches stack values;
        // pre_exec's closure runs in the forked child before exec, where
        // pthread_sigmask is one of the async-signal-safe calls.
        let mut blocked: libc::sigset_t = unsafe { std::mem::zeroed() };
        unsafe {
            libc::sigemptyset(&mut blocked);
            for signal in FORWARDED {
                libc::sigaddset(&mut blocked, signal);
            }
        }
        let mut previous: libc::sigset_t = unsafe { std::mem::zeroed() };
        unsafe { libc::pthread_sigmask(libc::SIG_BLOCK, &blocked, &mut previous) };
        // SAFETY: pre_exec runs its closure in the forked child before
        // exec, where pthread_sigmask and the sigset_t initializers are
        // async-signal-safe; the closure only touches its moved-in
        // stack values. The outer block also covers the closure body's
        // calls, which execute under fork's suspended-thread semantics.
        let spawn = unsafe {
            command
                .pre_exec(move || {
                    let mut unblock: libc::sigset_t = std::mem::zeroed();
                    libc::sigemptyset(&mut unblock);
                    for signal in FORWARDED {
                        libc::sigaddset(&mut unblock, signal);
                    }
                    libc::pthread_sigmask(libc::SIG_UNBLOCK, &unblock, std::ptr::null_mut());
                    Ok(())
                })
                .spawn()
        };
        if let Ok(child) = &spawn {
            CHILD_PID.store(i32::try_from(child.id()).unwrap_or(0), Ordering::SeqCst);
        }
        unsafe { libc::pthread_sigmask(libc::SIG_SETMASK, &previous, std::ptr::null_mut()) };
        spawn
    }

    /// Clears the forwarded pid after the child was reaped.
    pub(super) fn child_gone() {
        CHILD_PID.store(0, Ordering::SeqCst);
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
    // The machine-wide job gate, for the invocation's whole life: a batch
    // command waits for its slot and memory; an interactive one books its
    // memory and never waits. Taken before anything else runs, so a
    // passthrough is gated exactly like a cached build.
    let gate = match gate(&lossy, cwd, config) {
        Ok(gate) => gate,
        Err(message) => {
            note(&message);
            return ShimReport {
                action: Action::Passthrough,
                exit_code: Some(1),
                key: None,
                materialize: None,
                ingest_files: 0,
                skipped_dirs: 0,
                reason: "gate_timeout",
            };
        }
    };
    let gated = gate.is_some();
    let parsed = match parse_flutter_args(&lossy) {
        Ok(parsed) => parsed,
        Err(err) => {
            let reason = if matches!(err, ArgModelError::Invalid(_)) {
                "invalid_argv"
            } else {
                "unmodeled_command"
            };
            return run_passthrough(argv, cwd, runner, reason, gated);
        }
    };

    let Some(flutter_bin) = config.flutter_bin.clone() else {
        return run_passthrough(argv, cwd, runner, "flutter_unresolved", gated);
    };
    let root = match key::flutter_root(&flutter_bin) {
        Ok(root) => root,
        Err(_) => return run_passthrough(argv, cwd, runner, "flutter_root_unreadable", gated),
    };
    let toolchain = match cached_fingerprint(&root, &config.store_root) {
        Ok(toolchain) => toolchain,
        Err(_) => return run_passthrough(argv, cwd, runner, "toolchain_unreadable", gated),
    };
    // The record lands before the gate refuses, so an unsupported SDK
    // pays its one rehash and never another: the negative record serves
    // every later invocation of the same SDK state.
    store_fingerprint_cache(&config.store_root, &root, &toolchain);
    if !supported_flutter_version(&toolchain.flutter_version) {
        return run_passthrough(argv, cwd, runner, "unsupported_flutter_version", gated);
    }

    let Some(cwd) = fs::canonicalize(cwd).ok() else {
        return run_passthrough(argv, cwd, runner, "cwd_unreadable", gated);
    };

    // The replay anchors of THIS tree: the worktree root (git toplevel
    // enclosing the project) is the rewrite target for a cross-tree
    // hit; the two cache roots classify machine-local paths. Shared by
    // PRE's name recomputation and POST's classification scans.
    let worktree = worktree_root(&cwd);
    let anchors = PathAnchors {
        worktree_root: worktree.clone(),
        pub_cache: pub_cache_root(),
        flutter_root: root.clone(),
    };

    let dart_defines = dart_define_vector(&parsed, &root, &toolchain);
    let key_defines = key::filter_defines(&dart_defines.iter().cloned().collect());
    let build_root = cwd.join(".dart_tool").join("flutter_build");
    // Debris sweep (independent of any hit): past versions of this
    // cache moved superseded dirs aside as `.bcache-stale-<ms>`
    // siblings and never removed them; placement is fill-only-missing
    // now, so all that is left is cleaning the leftovers by age.
    sweep_stale_aside_dirs(&build_root);
    // The `flutter test` kernel cache: a fixed-path dir under the build
    // dir, whose name flutter_tools honors `$FLUTTER_BUILD_DIR` for.
    let build_dir = flutter_build_dir(std::env::var_os("FLUTTER_BUILD_DIR"));
    let test_cache_dir = cwd.join(&build_dir).join("test_cache");
    let test_cache_rel = test_cache_dir
        .strip_prefix(&cwd)
        .map_or_else(|_| test_cache_dir.clone(), Path::to_path_buf);
    // The `build bundle` face's asset output dir (`--asset-dir`, else
    // `<buildDir>/flutter_assets`): `copy_flutter_bundle` writes there,
    // and a tree that lacks it reruns the copy however faithfully the
    // hash dir replays. A plain project-relative path only — an asset
    // dir elsewhere is not this tree's to replay.
    let asset_dir_rel = (parsed.target.command == "build bundle")
        .then(|| {
            flag_value(&parsed.target.extra_key_args, "asset-dir")
                .map_or_else(|| build_dir.join("flutter_assets"), PathBuf::from)
        })
        .filter(|path| {
            !path.as_os_str().is_empty()
                && path
                    .components()
                    .all(|component| matches!(component, std::path::Component::Normal(_)))
        });

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
    // Held from the lookup through the placement below: a concurrent
    // run's post-run reclaim cannot take a blob this hit is about to
    // place (see `CasStore::pin`). Released before the run.
    let mut pin = None;
    if let Ok(store) = CasStore::open(&config.store_root) {
        pin = store.pin();
        match store.lookup(&key) {
            Ok(Some(manifest)) => hit = Some(manifest),
            Ok(None) => {}
            Err(err) => note(&err),
        }
    }

    // The engine revision every build-dir name hashes: null under
    // `--local-engine` (the recorded templates were captured the same
    // way). Shared by the PRE plan names and the POST prior lookup.
    let engine_for_names = if local_engine(&parsed) {
        ""
    } else {
        toolchain.engine_revision.as_str()
    };
    // What PRE's materialization placed, by the name POST knows each dir
    // under — the hash-dir name for build faces, the project-relative
    // test-cache path for the test face. Empty unless a hit materialized.
    let mut placed: Vec<PlacedDir> = Vec::new();

    if let Some(manifest) = hit {
        // Cross-tree means different worktree roots: only then do the
        // recorded absolute paths need rewriting, and only then is a
        // foreign-paths dir refused (within-tree, its paths are valid
        // as recorded).
        let cross_tree =
            manifest.worktree_root_at_ingest.as_bytes() != worktree.as_os_str().as_encoded_bytes();
        let mut dirs = Vec::with_capacity(manifest.dirs.len());
        let mut plan_names = Vec::with_capacity(manifest.dirs.len());
        let mut skipped = 0u64;
        for dir in &manifest.dirs {
            match dir.name_rule {
                NameRule::VerbatimRelative | NameRule::RecomputeFromTemplate => {
                    if cross_tree && dir.files.iter().any(|file| file.foreign_paths) {
                        // Bookkeeping that names a path outside the
                        // ingest anchors would let flutter's input
                        // validation skip on a file this tree cannot
                        // re-derive — refuse the replay of this dir.
                        skipped += 1;
                        note(&format!(
                            "dir {} records paths outside the ingest anchors; cross-tree replay refused",
                            dir.name_observed
                        ));
                        continue;
                    }
                    let name = replayed_hash_name(
                        dir,
                        &manifest.worktree_root_at_ingest,
                        &worktree,
                        engine_for_names,
                    );
                    // A recomputed name differs from the recorded one
                    // whenever it embeds the ingest tree (absolute
                    // output dirs, path-carrying defines): recorded
                    // mentions of the old segment must follow the dir.
                    let name_rewrite = (name != dir.name_observed)
                        .then(|| (dir.name_observed.clone(), name.clone()));
                    dirs.push(hash_dir_plan(&name, dir.files.clone(), name_rewrite));
                    plan_names.push(name);
                }
                // The recorded name is provenance only: the replay path
                // is re-derived from THIS invocation's build dir, so a
                // changed `$FLUTTER_BUILD_DIR` still lands where flutter
                // looks. A dill with foreign URIs ingested as
                // `RewriteKind::None` already replays as the verbatim
                // clone the design calls for — correct, just slower.
                NameRule::TestKernelCache => {
                    dirs.push(DirPlan {
                        path: test_cache_rel.clone(),
                        name_rewrite: None,
                        files: dir.files.clone(),
                    });
                    plan_names.push(test_cache_rel.to_string_lossy().into_owned());
                }
                // Like the test cache, the recorded path is provenance:
                // the asset dir is this invocation's. Its files carry
                // the same anchor classification as a hash dir's.
                NameRule::AssetDir => {
                    let Some(asset_dir) = &asset_dir_rel else {
                        skipped += 1;
                        continue;
                    };
                    if cross_tree && dir.files.iter().any(|file| file.foreign_paths) {
                        skipped += 1;
                        note(&format!(
                            "dir {} records paths outside the ingest anchors; cross-tree replay refused",
                            dir.name_observed
                        ));
                        continue;
                    }
                    dirs.push(DirPlan {
                        path: asset_dir.clone(),
                        name_rewrite: None,
                        files: dir.files.clone(),
                    });
                    plan_names.push(asset_dir.to_string_lossy().into_owned());
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
                old_root: manifest.worktree_root_at_ingest.clone(),
                new_root: worktree.clone(),
            };
            // A14: the placement writes into this tree — hold the
            // per-tree lock so a second shim's scan or placement cannot
            // interleave. Contention past the cap degrades to a clean
            // miss: the build runs, nothing is placed.
            if let Some(_tree_lock) = acquire_tree_lock(&cwd) {
                let materialized = CasStore::open(&config.store_root).and_then(|store| {
                    let outcome = materialize::materialize(&store, &plan, &cwd)?;
                    store.record_hit(&key).map(|()| outcome)
                });
                match materialized {
                    Ok(outcome) => {
                        report.action = Action::PreHitMaterialized;
                        note(&format!(
                            "hit {} materialized {} dirs, {skipped} skipped{}",
                            &key[..8],
                            replayable,
                            if outcome.report.intact {
                                " (tree intact)"
                            } else {
                                ""
                            }
                        ));
                        let dills = &outcome.report;
                        if dills.dill_rewrites + dills.dill_rewrite_refused + dills.dill_omitted > 0
                        {
                            note(&format!(
                                "dill: {} rewritten; rewrite refused: {} placed verbatim, {} left \
                                 out for flutter to rebuild",
                                dills.dill_rewrites, dills.dill_rewrite_refused, dills.dill_omitted
                            ));
                        }
                        if outcome.report.filecache_patched > 0 {
                            note(&format!(
                                "filecache: {} md5 records re-aimed at rewritten outputs",
                                outcome.report.filecache_patched
                            ));
                        }
                        report.materialize = Some(outcome.report);
                        report.reason = "pre_hit";
                        placed = plan_names
                            .into_iter()
                            .zip(outcome.placed)
                            .map(|(name, files)| PlacedDir { name, files })
                            .collect();
                        // The tree's placement record learns the fresh
                        // identities now, overlaid on what it remembered
                        // of this entry, so a run whose POST never gets
                        // to write (a non-zero exit, a busy tree lock)
                        // still leaves the next run a record to vouch by.
                        store_placement(
                            &cwd,
                            &key,
                            placement::overlay(placement::known_dirs(&cwd, &key), placed.clone()),
                        );
                    }
                    Err(err) => {
                        report.reason = "materialize_failed";
                        note(&err);
                    }
                }
            } else {
                report.reason = "tree_lock_busy";
                note(
                    "another shim holds this tree's cache lock; skipping the pre-run materialization",
                );
            }
        }
    }
    drop(pin);

    // RUN: the original argv, the canonical cwd (flutter's bookkeeping
    // paths then match the POST scan's root), the child's exit code —
    // observed throughout for the assemble invocations the platform
    // build systems spawn behind the child. The run marker (audit A22)
    // is created immediately before the spawn: its mtime — full clock
    // resolution, later than anything written before the run — is the
    // touch gate the POST scans use. The old floored-to-whole-seconds
    // start misclassified pre-run writes in the same second as touches.
    let observer = CaptureObserver::start(cwd.clone(), toolchain.engine_revision.clone());
    let marker = RunMarker::create(&cwd);
    let run = runner.run(argv, &cwd);
    #[cfg(all(unix, not(test)))]
    if let Some(gate) = &gate {
        dartq_toolchain::ledger::record_peak(
            &gate.key,
            dartq_toolchain::ledger::Face::Build,
            dartq_toolchain::proc::children_peak_rss(),
        );
    }
    let observed = observer.finish();
    let run_started = marker
        .as_ref()
        .map_or_else(now_floored_to_secs, RunMarker::touch_time);
    drop(marker);
    match run {
        Ok(code) => report.exit_code = Some(code),
        Err(err) => {
            note(&format!("flutter spawn failed: {err}"));
            return report;
        }
    }

    // A flutter_tools defect (measured on 3.44.0) leaks the run's
    // frontend_server parentless (pid 1) — one idle process per build,
    // forever. The sweep is one ps spawn; anything it does not certainly
    // own (a live parent, a pre-run start) it leaves alone, and any
    // failure is a note on stderr. Gated out under the test harness: the
    // fake runner spawns no child that could leak, and a sweep firing
    // from one fixture test would reap another test's fixture — to the
    // sweep, any frontend_server-named orphan born during the run is a
    // leak, which is exactly what a concurrent test's fixture is. The
    // sweep itself is unit-tested in [`crate::reaper`].
    #[cfg(all(unix, not(test)))]
    crate::reaper::reap_orphaned_frontend_servers(run_started);

    // POST: ingest every dir the run touched; a hit whose key the rebuild
    // reproduced needs no re-ingest (the entry is already stored). A
    // non-zero exit narrows POST to the test face's kernel cache:
    // `flutter test` exits non-zero both when compilation fails and when
    // a compiled suite is red, and the scan itself tells the two apart —
    // frontend_server writes the cache dill only after a successful
    // compile, so a touched dill is valid kernel for the current sources
    // however red the suite (the harness compiles the closure before
    // running any test), while a compile failure leaves the cache
    // untouched and the scan yields nothing. The hash dirs stay
    // exit-0-gated: a failed build may leave partially-written
    // flutter_build state, and flutter's own content validation is the
    // replay safety net — ingesting known-partial state would only
    // waste materialization. A signal death arrives as 128 + signal
    // (the runner's shell-convention mapping) and takes the same path;
    // at worst it tears the in-place dill copy (dart:io File.copy
    // writes the destination directly, not through a rename), and a
    // torn dill costs one wasted materialization — frontend_server
    // falls back to a full compile whenever the initialize-from-dill
    // file will not read.
    let test_face = parsed.target.command == "test";
    let zero_exit = report.exit_code == Some(0);
    // A14: POST scans the very dirs another shim's placement writes and
    // republishes the shared entry — it runs under the same per-tree
    // lock as the placement. Contention past the cap skips the ingest:
    // the run's outputs stay in the tree and the next run re-ingests.
    if (zero_exit || test_face)
        && let Some(_tree_lock) = acquire_tree_lock(&cwd)
    {
        let touched = if zero_exit {
            match find_touched_hash_dirs(&build_root, run_started) {
                Ok(touched) => touched,
                Err(err) => {
                    note(&err);
                    Vec::new()
                }
            }
        } else {
            // A red test run ingests the kernel cache exclusively; the
            // hash-dir scan never runs, touched or not.
            Vec::new()
        };
        let post_key = coarse_key(&cwd, &parsed, &key_defines, &toolchain);
        // Every placed identity this tree knows for the ingest key: what
        // this run placed, overlaid on the tree's placement record of
        // earlier runs (see `crate::placement`). A record for a key the
        // run moved away from is not this entry's and is not consulted.
        let known = placement::overlay(placement::known_dirs(&cwd, &post_key), placed);
        // The test face: the kernel cache the run compiled into, when it
        // wrote one. Unlike the assemble faces nothing observes whether a
        // hit's rerun really recompiled — and frontend_server rewrites
        // the dill on every run — so a touched cache always goes back to
        // the store, keeping the entry converged on the tree's sources.
        // The entry currently stored under the post key supplies the
        // prior stamps, and a hit's placement supplies the placed ones: a
        // dill whose identity either record proves reuses its blob
        // without re-reading tens of MB; a rewritten dill re-hashes and
        // replaces the reference, and the post-run reclaim takes the
        // superseded blob (see `maybe_gc`).
        let test_cache = if test_face {
            match CasStore::open(&config.store_root).and_then(|store| {
                let prior = store.lookup(&post_key)?;
                let prior_dir = prior.as_ref().and_then(|manifest| {
                    manifest
                        .dirs
                        .iter()
                        .find(|dir| dir.name_rule == NameRule::TestKernelCache)
                });
                let placed_dir = known
                    .iter()
                    .find(|dir| Path::new(&dir.name) == test_cache_rel)
                    .map(|dir| dir.files.as_slice());
                scan_test_cache(
                    &test_cache_dir,
                    run_started,
                    &anchors,
                    prior_dir,
                    placed_dir,
                    &|id| store.blob_path(id).is_file(),
                )
            }) {
                Ok(scanned) => scanned,
                Err(err) => {
                    note(&err);
                    None
                }
            }
        } else {
            None
        };
        if !touched.is_empty() || test_cache.is_some() {
            // Only templates whose dir this run materialized carry
            // information; the rest (a foreign process, a failed
            // assemble) are dead observations.
            let captures: Vec<AssembleCapture> = observed
                .into_iter()
                .filter(|capture| touched.iter().any(|(name, _)| name == &capture.name))
                .collect();
            // A hit may skip the re-ingest ONLY when the run changed
            // nothing beyond the hit's own placement: every touched dir
            // is still byte-for-byte what PRE materialized. The touched
            // set alone cannot tell flutter's rewrites from the
            // placement's own writes; a stamp mismatch (or a file
            // beside the placement) means a rerun rewrote outputs, and
            // re-ingesting is what keeps the first tree to hit from
            // freezing the entry on its own outputs forever.
            let already_stored = report.action == Action::PreHitMaterialized
                && report.key.as_deref() == Some(post_key.as_str())
                && captures.is_empty()
                && test_cache.is_none()
                && placements_intact(&touched, &build_root, &known);
            if !already_stored {
                let test_scan = test_cache.map(|scan| TestCacheScan {
                    name: test_cache_rel.to_string_lossy().into_owned(),
                    scan,
                });
                // The asset dir rides along on every bundle-face ingest
                // where it exists, touched or not: a warm run skips
                // `copy_flutter_bundle` and writes nothing there, and an
                // entry that dropped the dir would cost the next fresh
                // tree that copy.
                let asset_dir = asset_dir_rel
                    .as_ref()
                    .map(|rel| (rel.to_string_lossy().into_owned(), cwd.join(rel)));
                let ingested = CasStore::open(&config.store_root).and_then(|store| {
                    let count = ingest_observed(
                        &store,
                        &build_root,
                        &touched,
                        &captures,
                        &cwd,
                        &anchors,
                        &parsed,
                        &key_defines,
                        &toolchain,
                        engine_for_names,
                        post_key.clone(),
                        &known,
                        asset_dir
                            .as_ref()
                            .map(|(name, dir)| (name.as_str(), dir.as_path())),
                        test_scan.as_ref(),
                    )?;
                    maybe_gc(&store, config);
                    Ok(count)
                });
                match ingested {
                    Ok((count, skips)) => {
                        if report.action != Action::PreHitMaterialized {
                            report.action = Action::Ingested;
                        }
                        report.ingest_files = count;
                        if report.key.is_none() {
                            report.key = Some(post_key.clone());
                        }
                        note(&format!("ingested {} ({count} files)", &post_key[..8]));
                        if skips.total() > 0 {
                            note(&format!(
                                "ingest scan skipped {} files: {skips}",
                                skips.total()
                            ));
                        }
                    }
                    Err(err) => note(&err),
                }
            }
        }
    } else if zero_exit || test_face {
        note("another shim holds this tree's cache lock; skipping the post-run ingest");
    }

    report
}

/// A hash-dir materialization plan: `.dart_tool/flutter_build/<name>`,
/// filling only missing files (see [`crate::materialize`]).
fn hash_dir_plan(
    name: &str,
    files: Vec<ManifestFile>,
    name_rewrite: Option<(String, String)>,
) -> DirPlan {
    DirPlan {
        path: Path::new(".dart_tool").join("flutter_build").join(name),
        name_rewrite,
        files,
    }
}

/// Whether every touched dir is still exactly what the tree's known
/// placements say: each known file keeps the stamp the placement (or
/// the tree's record of it) holds and the dir holds nothing beside
/// them. This is the "flutter changed nothing" signal for a hit —
/// every write moves the ns mtime (the shared identity semantics), so a
/// flutter rewrite, delete, or new file breaks the match and the rerun
/// re-ingests.
fn placements_intact(
    touched: &[(String, SystemTime)],
    build_root: &Path,
    known: &[PlacedDir],
) -> bool {
    for (name, _) in touched {
        let Some(files) = known
            .iter()
            .find(|dir| dir.name == *name)
            .map(|dir| &dir.files)
        else {
            // A dir this run fabricated or re-aimed, not the placement.
            return false;
        };
        let dir = build_root.join(name);
        for file in files {
            let Ok(meta) = fs::metadata(dir.join(&file.path)) else {
                return false;
            };
            if stamp_of(&meta) != file.stamp {
                return false;
            }
        }
        // Nothing beyond the placement: an extra entry is a file flutter
        // fabricated beside the placed ones.
        match fs::read_dir(&dir) {
            Ok(entries) => {
                if entries.count() != files.len() {
                    return false;
                }
            }
            Err(_) => return false,
        }
    }
    true
}

/// The rewrite anchor of a tree: the nearest ancestor of the project
/// root holding `.git` — a directory for a main checkout, a file for a
/// linked worktree — or the project root itself when no repo encloses
/// it (then workspace siblings outside the project classify as
/// foreign, so they are refused cross-tree rather than mis-rewritten).
/// `project_root` must be canonical (its ancestors then are too).
fn worktree_root(project_root: &Path) -> PathBuf {
    let mut current = Some(project_root);
    while let Some(dir) = current {
        if fs::symlink_metadata(dir.join(".git")).is_ok() {
            return dir.to_path_buf();
        }
        current = dir.parent();
    }
    project_root.to_path_buf()
}

/// The pub cache root pub itself resolves: `$PUB_CACHE` when set to an
/// absolute path, else the per-user default. Unresolvable → empty: an
/// empty anchor matches nothing, so pub-cache paths then classify
/// foreign and affected dirs lose cross-tree replay — a safe, slower
/// degradation, never a wrong one.
fn pub_cache_root() -> PathBuf {
    if let Some(value) = std::env::var_os("PUB_CACHE").filter(|value| !value.is_empty()) {
        let path = PathBuf::from(value);
        if path.is_absolute() {
            return fs::canonicalize(&path).unwrap_or(path);
        }
    }
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .filter(|home| !home.is_empty());
    let Some(home) = home else {
        return PathBuf::new();
    };
    let path = if cfg!(windows) {
        PathBuf::from(home)
            .join("AppData")
            .join("Local")
            .join("Pub")
            .join("Cache")
    } else {
        PathBuf::from(home).join(".pub-cache")
    };
    fs::canonicalize(&path).unwrap_or(path)
}

/// Sweeps `.bcache-stale-<unix_millis>` dirs older versions of this
/// cache moved aside next to the hash dirs. Fill-only-missing placement
/// creates none; this is the cleanup of historical leftovers, by the
/// same one-hour grace the store's debris sweeps use — a dir's own
/// mtime survives the rename, so the age measured is that of the
/// dir's last content change, which only makes the sweep later, never
/// earlier. Best-effort: failures are ignored.
fn sweep_stale_aside_dirs(build_root: &Path) {
    const STALE_GRACE: u64 = 60 * 60;
    let now = unix_now();
    let Ok(entries) = fs::read_dir(build_root) else {
        return;
    };
    for entry in entries.flatten() {
        if !entry
            .file_name()
            .to_string_lossy()
            .contains(".bcache-stale-")
        {
            continue;
        }
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if !metadata.is_dir() {
            continue;
        }
        if let Ok(modified) = metadata.modified()
            && now.saturating_sub(modified_unix(&modified)) >= STALE_GRACE
        {
            let _ = fs::remove_dir_all(entry.path());
        }
    }
}

fn modified_unix(time: &SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}

/// The flutter build dir of this invocation: `$FLUTTER_BUILD_DIR` when
/// set (flutter_tools routes every output path through it, the test
/// kernel cache included), else `build`. An empty value counts as
/// unset, as flutter_tools treats it.
fn flutter_build_dir(env: Option<OsString>) -> PathBuf {
    env.filter(|value| !value.is_empty())
        .map_or_else(|| PathBuf::from("build"), PathBuf::from)
}

/// Passthrough: become (or spawn) the child untouched and report. On
/// the production runner this never returns — the shim process IS
/// flutter from the exec on (audit A11), so the report only exists for
/// the exec-failure path and for test runners.
fn run_passthrough(
    argv: &[OsString],
    cwd: &Path,
    runner: &mut dyn FlutterRunner,
    reason: &'static str,
    gated: bool,
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
    // A gate lease is a locked file of this process: an exec would close
    // it (close-on-exec) and free the slot while flutter still runs, so a
    // gated passthrough spawns and waits instead.
    let run = if gated {
        runner.run(argv, cwd)
    } else {
        runner.exec(argv, cwd)
    };
    match run {
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
/// `pubspec.lock`, and every `--dart-define-from-file` (flutter_tools
/// folds their contents into the define list and no depfile records
/// them). `.dart_tool/package_config.json` is deliberately NOT keyed:
/// it exists only after flutter's implicit `pub get`, so keying it
/// would give a fresh worktree a pre-build key that never matches the
/// key the entry was stored under — the headline use case, dead on
/// arrival. Divergence is caught anyway: flutter treats package_config
/// as a build input, so its hash rides in every stamp/.filecache and
/// a replay over a diverged config rebuilds natively. Files that do
/// not exist (or cannot be read) are skipped — a fresh tree keys on
/// what is there.
fn provenance_closure(cwd: &Path, define_files: &[PathBuf]) -> Vec<ClosureFile> {
    let mut files = Vec::new();
    for relative in ["pubspec.yaml", "pubspec.lock"] {
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

/// Re-aims `text` at the target tree when it is an absolute path under
/// the ingest worktree root; a relative, foreign, or lookalike-prefixed
/// string replays verbatim. The boundary check (`/` or end of string
/// right after the root) keeps `/wt` from re-rooting `/wt-lookalike`.
fn rerooted(text: &str, ingest_root: &str, target_root: &Path) -> String {
    match text.strip_prefix(ingest_root) {
        Some(rest) if rest.is_empty() || rest.starts_with('/') => {
            format!("{}{}", target_root.display(), rest)
        }
        _ => text.to_owned(),
    }
}

/// The `.dart_tool/flutter_build` dir name a recorded hash dir replays
/// to in the target tree: verbatim for tree-relative names (the `build
/// bundle` face), recomputed from the recorded template otherwise —
/// with the output dir AND every define value re-aimed at the target
/// worktree root, because a define carrying an absolute path (Xcode's
/// `-dSrcRoot`, an absolute `-dTargetFile`, gradle's `flutter run`
/// stamp) hashes the tree into the name and the recomputation must
/// hash this tree the same way flutter here will. Shared by the PRE
/// plan (where the dir materializes under this name) and the POST
/// prior lookup (which must find the record whose replay lands at a
/// touched dir's name — the ingest-tree `name_observed` of a template
/// dir never names a dir in this tree).
fn replayed_hash_name(
    dir: &DirRecord,
    ingest_root: &str,
    target_root: &Path,
    engine_for_names: &str,
) -> String {
    if dir.name_rule == NameRule::RecomputeFromTemplate {
        let output_dir = rerooted(&dir.name_output_dir, ingest_root, target_root);
        let defines = dir
            .name_defines
            .iter()
            .map(|(name, value)| (name.clone(), rerooted(value, ingest_root, target_root)))
            .collect::<BTreeMap<String, String>>();
        flutter_build_dir_name(engine_for_names, &defines, Path::new(&output_dir))
    } else {
        dir.name_observed.clone()
    }
}

/// The scanned test kernel cache of a post-run ingest: the dir's
/// project-relative spelling (provenance for
/// [`NameRule::TestKernelCache`]) and the scan itself.
struct TestCacheScan {
    name: String,
    scan: DirScan,
}

/// Scans and ingests every observed build dir under `key`. Each dir
/// becomes one [`DirRecord`]: an assemble template whose name matches
/// supplies the define map and output dir
/// ([`NameRule::RecomputeFromTemplate`]); on the `build bundle` face
/// the dir is tree-relative and replays verbatim
/// ([`NameRule::VerbatimRelative`]); anything else is recorded as
/// [`NameRule::NotReplayable`] provenance. Templates whose dir never
/// materialized are dropped (a failed or partial assemble). Then the
/// fixed-path dirs join: the bundle face's `asset_dir` (`(name, path)`)
/// whenever it holds files ([`NameRule::AssetDir`]), and the test
/// kernel-cache scan when the run produced one.
///
/// Each dir's scan consults two identities (see
/// [`crate::ingest_scan`]): the placements this tree `known` — this
/// invocation's own materialization overlaid on the tree's placement
/// record of earlier ones — and the stamps of the entry currently
/// stored under `key` whose [`replayed_hash_name`] is the dir; a file
/// either record proves is re-referenced, so a warm run stores only
/// what flutter actually changed. A draft that records exactly the
/// stored snapshot stores nothing at all (see [`same_snapshot`]) —
/// and that holds across trees: a cross-tree placement's files are
/// vouched as this tree's re-rooted rendering of the entry's blobs
/// (first scan, references admitted), so a hit tree that reproduces
/// the entry leaves it exactly as ingested, in its one frame of
/// reference; only when the draft differs anyway is every such file
/// re-scanned to this tree's own bytes (second scan, references off),
/// so what gets published under this tree's root names only this
/// tree's bytes — unless that re-scan shows the draft carrying nothing
/// the tree's own record does not already claim ([`frame_only`]): then
/// the prior differs only in the frame it was last published in, the
/// first scan stands, and nothing publishes. Either way the tree's
/// placement record is rewritten with the scan's fresh identities
/// last, so the next run vouches by stamp alone. Returns the count of
/// files streamed into the store this run (reused references
/// excluded).
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
    anchors: &PathAnchors,
    parsed: &ParsedFlutterCmd,
    key_defines: &BTreeMap<String, String>,
    toolchain: &ToolchainFingerprint,
    engine_for_names: &str,
    key: String,
    known: &[PlacedDir],
    asset_dir: Option<(&str, &Path)>,
    test_cache: Option<&TestCacheScan>,
) -> Result<(u64, ScanSkips)> {
    let bundle_face = parsed.target.command == "build bundle";
    // Decision-time prior: the entry stored under the ingest key.
    let prior = store.lookup(&key)?;
    let blob_exists = |id: &BlobId| store.blob_path(id).is_file();
    let known_dir = |name: &str| {
        known
            .iter()
            .find(|dir| dir.name == name)
            .map(|dir| dir.files.as_slice())
    };
    // One scan of every dir the draft covers: the touched hash dirs,
    // the asset dir, and the test kernel cache (scanned once by the
    // caller; it never vouches across trees, see `scan_test_cache`).
    // `rerooted_ok` is `scan_entry_files`'s: references into the
    // entry's tree admitted or not.
    let scan_all = |rerooted_ok: bool| -> Result<ScannedDirs> {
        let mut scanned = ScannedDirs::with_capacity(observed.len() + 2);
        // `observed` is name-sorted, so the DirRecords come out name-sorted.
        for (name, _) in observed {
            // The prior's dir whose replay lands at this name, with the
            // tree it was recorded in.
            let prior_dir = prior.as_ref().and_then(|manifest| {
                manifest
                    .dirs
                    .iter()
                    .find(|dir| {
                        matches!(
                            dir.name_rule,
                            NameRule::VerbatimRelative | NameRule::RecomputeFromTemplate
                        ) && replayed_hash_name(
                            dir,
                            &manifest.worktree_root_at_ingest,
                            &anchors.worktree_root,
                            engine_for_names,
                        ) == *name
                    })
                    .map(|dir| (dir, manifest.worktree_root_at_ingest.as_str()))
            });
            let scan = scan_entry_files(
                &build_root.join(name),
                anchors,
                prior_dir.map(|(dir, _)| dir),
                known_dir(name),
                rerooted_ok,
                &blob_exists,
            )?;
            let (name_rule, name_defines, name_output_dir) = match captures
                .iter()
                .find(|capture| &capture.name == name)
            {
                Some(capture) => (
                    NameRule::RecomputeFromTemplate,
                    capture.defines.clone(),
                    capture.output_dir.clone(),
                ),
                None if bundle_face => (NameRule::VerbatimRelative, BTreeMap::new(), String::new()),
                // A missed capture for a dir the prior entry already
                // recorded keeps the prior record's naming (audit
                // A22): the absence of an observation this run says
                // nothing about replayability — without this arm one
                // missed assemble capture silently demoted the dir
                // to not-replayable provenance and the entry stopped
                // serving fresh trees. The template is carried in
                // THIS tree's frame — re-aimed from the prior's tree
                // the way its replay name was — so a draft published
                // under this tree's root never names another tree's
                // output dir.
                None => match prior_dir {
                    Some((previous, ingest_root)) => (
                        previous.name_rule,
                        previous
                            .name_defines
                            .iter()
                            .map(|(define, value)| {
                                (
                                    define.clone(),
                                    rerooted(value, ingest_root, &anchors.worktree_root),
                                )
                            })
                            .collect(),
                        rerooted(
                            &previous.name_output_dir,
                            ingest_root,
                            &anchors.worktree_root,
                        ),
                    ),
                    None => (NameRule::NotReplayable, BTreeMap::new(), String::new()),
                },
            };
            scanned.push(name.clone(), name_rule, name_defines, name_output_dir, scan);
        }
        if let Some((name, dir)) = asset_dir
            && dir.is_dir()
        {
            let prior_dir = prior.as_ref().and_then(|manifest| {
                manifest
                    .dirs
                    .iter()
                    .find(|record| record.name_rule == NameRule::AssetDir)
            });
            let scan = scan_entry_files(
                dir,
                anchors,
                prior_dir,
                known_dir(name),
                rerooted_ok,
                &blob_exists,
            )?;
            if !scan.files.is_empty() {
                scanned.push(
                    name.to_owned(),
                    NameRule::AssetDir,
                    BTreeMap::new(),
                    String::new(),
                    scan,
                );
            }
        }
        if let Some(scan) = test_cache {
            scanned.push(
                scan.name.clone(),
                NameRule::TestKernelCache,
                BTreeMap::new(),
                String::new(),
                scan.scan.clone(),
            );
        }
        Ok(scanned)
    };
    let now = unix_now();
    let draft_of = |dirs: Vec<DirRecord>| EntryManifest {
        key: key.clone(),
        project_root_at_ingest: cwd.to_string_lossy().into_owned(),
        worktree_root_at_ingest: anchors.worktree_root.to_string_lossy().into_owned(),
        pub_cache_root: anchors.pub_cache.to_string_lossy().into_owned(),
        flutter_root_at_ingest: anchors.flutter_root.to_string_lossy().into_owned(),
        dirs,
        defines: key_defines.clone(),
        target: parsed.target.clone(),
        toolchain: toolchain.clone(),
        // `created_at` is the entry's birth time, not this ingest's: a
        // rewrite of the same key carries the prior record's creation
        // forward so LRU aging reflects the entry's true age.
        created_at_unix: prior
            .as_ref()
            .map_or(now, |manifest| manifest.created_at_unix),
        last_hit_unix: now,
    };
    // A rerun that reproduced the stored snapshot byte for byte has
    // nothing to store. That is every warm validate-only build: flutter
    // rewrites `.filecache`, `outputs.json`, and runtime-skipped stamps
    // on EVERY build, with identical bytes when nothing changed
    // (flutter_tools 3.47.1 build_system.dart: `fileCache.persist()` in
    // `build`'s finally, `outputs.json` right after it) — `.filecache`
    // up to its hash-map order, which the scan's FileStore vouch
    // absorbs (see `crate::ingest_scan::scan_entry_files`) — so the publish
    // would only move those files' recorded stamps — which the next run
    // moves again. The tree's placement record takes the fresh stamps
    // instead (below); the manifest's stale stamp on a re-hashed file
    // only costs a re-hash while the record is missing, and a larger
    // such file is still published once so even a tree that keeps no
    // record is spared every later re-read. A vanished entry (gc
    // between lookup and here) is published afresh.
    let unchanged_against = |draft: &EntryManifest, sources: &[(PathBuf, BlobId)]| {
        prior
            .as_ref()
            .is_some_and(|prior| same_snapshot(prior, draft, engine_for_names))
            && sources.iter().all(|(path, blob)| {
                store.blob_path(blob).is_file()
                    && fs::metadata(path).is_ok_and(|metadata| metadata.len() <= STALE_STAMP_LIMIT)
            })
    };
    let mut scanned = scan_all(true)?;
    let mut draft = draft_of(std::mem::take(&mut scanned.dirs));
    let mut unchanged = unchanged_against(&draft, &scanned.sources);
    let mut flip_suppressed = false;
    if !unchanged && let Some(entry) = prior.as_ref() {
        if scanned.rerooted > 0 {
            // The entry changed under this tree: a reference into the
            // entry's tree cannot be published under this tree's root.
            // The second scan hashes every such file to this tree's own
            // bytes. The demotion is speculative: when it turns out the
            // draft carries nothing the tree's own record does not
            // already claim (`frame_only` below), the prior differed
            // only in the frame it was last published in and the first
            // scan stands.
            let vouched = (scanned, draft);
            scanned = scan_all(false)?;
            draft = draft_of(std::mem::take(&mut scanned.dirs));
            unchanged = unchanged_against(&draft, &scanned.sources);
            if !unchanged && frame_only(entry, &draft, known, engine_for_names) {
                (scanned, draft) = vouched;
                flip_suppressed = true;
            }
        } else if frame_only(entry, &draft, known, engine_for_names) {
            // No reference to demote, yet the draft diverges from the
            // prior while every file renders what the record claims:
            // the entry's own tree after another tree republished it
            // (the claimed blobs the flip detached may even be
            // reclaimed by now — the tree needs none of them stored on
            // its behalf, and no manifest is touched here).
            flip_suppressed = true;
        }
    }
    let record = placement::replace_dirs(known.to_vec(), scanned.record);
    if (unchanged || flip_suppressed) && store.record_hit(&key).is_ok() {
        store_placement(cwd, &key, record);
        return Ok((0, scanned.skips));
    }
    let count = scanned.sources.len() as u64;
    store.ingest(draft, &scanned.sources)?;
    store_placement(cwd, &key, record);
    Ok((count, scanned.skips))
}

/// Whether `draft` diverges from `prior` in frame alone: the two record
/// the same snapshot in every respect but file bytes ([`same_snapshot`]
/// over the draft carrying the prior's blobs, path for path), and every
/// draft file renders what this tree's record already claims — the
/// stored blob it was placed from, or the exact on-disk bytes the
/// record hashed at placement. One tree's first run republishes the
/// entry under its own root (flutter regenerates the re-rooted files it
/// cannot validate), and every later run in any other tree would flip
/// the frame back, each alternation paying a full republish of files
/// whose bytes never changed. Publishing such a draft serves no one — a
/// fresh tree regenerates its own frame regardless — so the entry stays
/// as ingested and the tree's record absorbs the scan's identities; a
/// run that rewrote real content hashes outside both claims, and a
/// prior missing a dir or a path is missing content, not a frame — both
/// still publish. Whether the store still holds a claimed blob is
/// immaterial: nothing is published off this verdict, so nothing can
/// dangle.
fn frame_only(
    prior: &EntryManifest,
    draft: &EntryManifest,
    known: &[PlacedDir],
    engine_for_names: &str,
) -> bool {
    let claimed = draft.dirs.iter().all(|dir| {
        let Some(files) = known
            .iter()
            .find(|record| record.name == dir.name_observed)
            .map(|record| record.files.as_slice())
        else {
            return false;
        };
        dir.files.len() == files.len()
            && dir.files.iter().all(|file| {
                files.iter().any(|claim| {
                    claim.path == file.path
                        && (file.blob == claim.blob || Some(file.blob) == claim.on_disk)
                })
            })
    });
    if !claimed || prior.dirs.len() != draft.dirs.len() {
        return false;
    }
    let mut shadow = draft.clone();
    for (dir, prior_dir) in shadow.dirs.iter_mut().zip(&prior.dirs) {
        if dir.files.len() != prior_dir.files.len() {
            return false;
        }
        for (file, prior_file) in dir.files.iter_mut().zip(&prior_dir.files) {
            if file.path != prior_file.path {
                return false;
            }
            file.blob = prior_file.blob;
        }
    }
    same_snapshot(prior, &shadow, engine_for_names)
}

/// The scan of every dir one ingest draft covers (see
/// [`ingest_observed`]): the dir records and ingest sources of the
/// draft, the scans' skip counts, the tree placement record's view of
/// the same dirs, and how many files were vouched as re-rooted
/// renderings of the entry's blobs ([`DirScan::rerooted`]).
struct ScannedDirs {
    dirs: Vec<DirRecord>,
    sources: Vec<(PathBuf, BlobId)>,
    skips: ScanSkips,
    record: Vec<PlacedDir>,
    rerooted: u64,
}

impl ScannedDirs {
    fn with_capacity(dirs: usize) -> Self {
        Self {
            dirs: Vec::with_capacity(dirs),
            sources: Vec::new(),
            skips: ScanSkips::default(),
            record: Vec::with_capacity(dirs),
            rerooted: 0,
        }
    }

    /// Adds one scanned dir under the name POST knows it by.
    fn push(
        &mut self,
        name: String,
        name_rule: NameRule,
        name_defines: BTreeMap<String, String>,
        name_output_dir: String,
        scan: DirScan,
    ) {
        self.sources.extend(scan.sources);
        self.skips.add(&scan.skipped);
        self.rerooted += scan.rerooted;
        self.record.push(PlacedDir {
            name: name.clone(),
            files: scan.record,
        });
        self.dirs.push(DirRecord {
            name_observed: name,
            name_rule,
            name_defines,
            name_output_dir,
            files: scan.files,
        });
    }
}

/// Writes `dirs` as this tree's placement record for `key` (see
/// [`crate::placement`]). Best-effort: a record that fails to land
/// costs the next run the re-hash the record would have spared, and
/// that run writes it again — one stderr line says so.
fn store_placement(project_root: &Path, key: &str, dirs: Vec<PlacedDir>) {
    let record = PlacementRecord {
        key: key.to_owned(),
        dirs,
    };
    if let Err(err) = placement::store(project_root, &record) {
        note(&format!(
            "placement record for {} not written: {err}",
            &key[..8]
        ));
    }
}

/// The largest re-hashed file an unchanged rerun leaves on a stale
/// recorded stamp (see [`ingest_observed`]): 1 MiB re-hashes in well
/// under a millisecond, below the cost of a manifest publish.
const STALE_STAMP_LIMIT: u64 = 1 << 20;

/// Whether `draft` records exactly `prior`'s snapshot: the same key,
/// defines, target, toolchain, and cache roots, and dir for dir the
/// same rule, naming, and file bytes (blobs) — file stamps, the entry
/// timestamps, and the tree the entry was ingested from aside, which
/// are bookkeeping of WHEN and WHERE, not WHAT. Naming is compared
/// across trees: a template dir's name embeds its tree's absolute
/// output dir, so the prior's naming is re-aimed at the draft's tree
/// first (exactly the replay's recomputation, [`replayed_hash_name`]).
/// A tree that re-roots the entry's bookkeeping and reproduces it
/// holds the entry's snapshot, and the entry stays as ingested.
fn same_snapshot(prior: &EntryManifest, draft: &EntryManifest, engine_for_names: &str) -> bool {
    let target = Path::new(&draft.worktree_root_at_ingest);
    let ingest_root = prior.worktree_root_at_ingest.as_str();
    let same_file = |prior: &ManifestFile, draft: &ManifestFile| {
        prior.path == draft.path
            && prior.blob == draft.blob
            && prior.executable == draft.executable
            && prior.rewrite == draft.rewrite
            && prior.foreign_paths == draft.foreign_paths
    };
    let same_dir = |prior: &DirRecord, draft: &DirRecord| {
        prior.name_rule == draft.name_rule
            && replayed_hash_name(prior, ingest_root, target, engine_for_names)
                == draft.name_observed
            && rerooted(&prior.name_output_dir, ingest_root, target) == draft.name_output_dir
            && prior.name_defines.len() == draft.name_defines.len()
            && prior.name_defines.iter().zip(&draft.name_defines).all(
                |((name, value), (draft_name, draft_value))| {
                    name == draft_name && rerooted(value, ingest_root, target) == *draft_value
                },
            )
            && prior.files.len() == draft.files.len()
            && prior
                .files
                .iter()
                .zip(&draft.files)
                .all(|(p, d)| same_file(p, d))
    };
    prior.key == draft.key
        && prior.pub_cache_root == draft.pub_cache_root
        && prior.flutter_root_at_ingest == draft.flutter_root_at_ingest
        && prior.defines == draft.defines
        && prior.target == draft.target
        && prior.toolchain == draft.toolchain
        && prior.dirs.len() == draft.dirs.len()
        && prior
            .dirs
            .iter()
            .zip(&draft.dirs)
            .all(|(p, d)| same_dir(p, d))
}

/// Post-ingest store upkeep. Over budget — or with the store's volume
/// short of the free-space reserve; the O(1) running usage counter plus
/// one `statvfs` decide, see [`CasStore::over_budget`] — the full gc runs
/// (which reclaims detached blobs first). Under budget, the targeted
/// [`CasStore::reclaim_detached`] takes the blobs this ingest (or an
/// earlier one a pin deferred) detached, without walking the blobs — a
/// warm `flutter test` loop then holds one kernel per entry instead of
/// one per run. Failures surface as one stderr line and never affect the
/// build.
fn maybe_gc(store: &CasStore, config: &ShimConfig) {
    match store.over_budget(config.budget_bytes) {
        Ok(true) => {
            if let Err(err) = store.gc(config.budget_bytes, config.grace_secs, unix_now()) {
                note(&err);
            }
        }
        Ok(false) => {
            if let Err(err) = store.reclaim_detached() {
                note(&err);
            }
        }
        Err(err) => note(&err),
    }
}

/// The toolchain fingerprint through the `<store_root>/meta/` cache, so
/// the ~40 MB of hashed artifacts is hashed once per SDK state instead
/// of once per invocation. A cached fingerprint is used only when the
/// flutter root and both hashed artifacts' mtime+size still match. Two
/// records exist: `toolchain.json` for supported SDKs and
/// `toolchain-unsupported.json` — the NEGATIVE record — for versions
/// the gate refuses; both use the same staleness rule, and a root only
/// ever consults one of them (a given root+stamps state is either
/// supported or it is not).
///
/// Safety: a stale cache yields a wrong fingerprint, hence a wrong
/// key, hence a lookup miss — entries live under their true keys, so a
/// stale cache can never serve a wrong hit. The same argument covers
/// the version gate: a gate that passes on a stale supported stamp only
/// produces misses, and the negative record only skips a rehash whose
/// result the gate would discard anyway.
fn cached_fingerprint(root: &Path, store_root: &Path) -> Result<ToolchainFingerprint> {
    let Some((dill_stamp, frontend_stamp)) = hashed_artifact_stamps(root) else {
        return key::fingerprint_toolchain(root);
    };
    for name in ["toolchain.json", "toolchain-unsupported.json"] {
        let cache_path = store_root.join("meta").join(name);
        if let Ok(bytes) = fs::read(&cache_path)
            && let Ok(cache) = serde_json::from_slice::<ToolchainCache>(&bytes)
            && cache.flutter_root == root
            && (&cache.dill, &cache.frontend) == (&dill_stamp, &frontend_stamp)
        {
            return Ok(cache.fingerprint);
        }
    }
    key::fingerprint_toolchain(root)
}

/// Writes the fingerprint cache (tmp + rename, best-effort: a failed
/// write only costs the next call a rehash). A record whose bytes
/// already sit on disk — the common warm-invocation case — skips the
/// rewrite (and its fsync) entirely.
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
        let name = if supported_flutter_version(&fingerprint.flutter_version) {
            "toolchain.json"
        } else {
            "toolchain-unsupported.json"
        };
        let path = dir.join(name);
        if fs::read(&path).is_ok_and(|prior| prior == bytes) {
            return Ok(());
        }
        let tmp = dir.join(format!(".toolchain.{}.dartq-tmp", std::process::id()));
        fs::write(&tmp, bytes)?;
        fs::rename(&tmp, &path)
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

/// Resolves the flutter binary the shim spawns, the way every dartq
/// face resolves it ([`dartq_toolchain::find_flutter`]): the config
/// override, then `$DARTQ_FLUTTER`, then a PATH search that skips the
/// running dartq binary itself (either an explicit override or a PATH
/// hit resolving to this executable would recurse — each shim spawning
/// another at ~210 processes/second, audit A10; the shared resolver's
/// refusal fires first, as one `dartq: refusing to run flutter at …`
/// stderr line naming both paths — the shim's old `dartq-bcache:`
/// prefix on that line is gone with the duplicate). Returns `None`
/// when nothing resolves or the resolution IS this executable; the
/// caller's not-found exit (127) then applies.
///
/// The shim's contract that symlinks resolve to the real SDK binary is
/// kept here: the shared PATH step returns the entry as found (an fvm
/// shim, a `~/fvm/default/bin/flutter` link), and this face
/// canonicalizes it once. Every later consumer keys through
/// [`key::flutter_root`], which canonicalizes anyway; the spawn runs
/// the real binary either way (flutter's wrapper script follows its
/// own links). An unresolvable override passes through verbatim — the
/// spawn, not the lookup, reports it.
#[must_use]
pub fn discover_flutter(explicit: Option<&Path>) -> Option<PathBuf> {
    dartq_toolchain::find_flutter(explicit).map(|found| fs::canonicalize(&found).unwrap_or(found))
}

/// `now`, floored to whole seconds: the conservative fallback touch
/// gate for the touched-dir scan when no run marker could be written
/// (whole-second-mtime file systems lose nothing by it).
fn now_floored_to_secs() -> SystemTime {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(UNIX_EPOCH, |since| {
            UNIX_EPOCH + Duration::from_secs(since.as_secs())
        })
}

/// A no-op stand-in for [`TreeLock`] on platforms without `flock`:
/// acquiring always succeeds and guards nothing.
#[cfg(not(unix))]
struct TreeLock;

/// Takes this tree's cache lock (audit A14), waiting up to the cap for
/// a concurrent shim. `None` on contention or when the lock itself is
/// unavailable (a lock failure degrades to the old unsynchronized
/// behavior — the pid-unique temp names keep concurrent placements
/// from corrupting anything; only the cache efficiency is lost).
fn acquire_tree_lock(cwd: &Path) -> Option<TreeLock> {
    #[cfg(unix)]
    {
        /// How long a shim waits for a concurrent shim's tree lock
        /// before running cache-less (test builds shorten it so the
        /// contention path is reachable without sleeping past a cap).
        #[cfg(not(test))]
        const TREE_LOCK_WAIT: Duration = Duration::from_secs(600);
        #[cfg(test)]
        const TREE_LOCK_WAIT: Duration = Duration::from_millis(300);
        match TreeLock::acquire(cwd, TREE_LOCK_WAIT) {
            Ok(lock) => lock,
            Err(err) => {
                note(&format!(
                    "tree lock unavailable ({err}); proceeding unsynchronized"
                ));
                None
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = cwd;
        Some(TreeLock)
    }
}

/// `.dart_tool/dartq-bcache-run-<pid>`: created immediately before the
/// flutter spawn and removed right after, the touch gate of the
/// post-run scans (audit A22). Its mtime carries full clock resolution
/// and postdates every write the shim or the user made before the run,
/// so a record rewritten in the same whole second as the run start —
/// which the old floored-to-seconds gate misread as a touch — stays
/// untouched. The pid suffix keeps two shims in one tree from sharing a
/// marker.
struct RunMarker {
    path: PathBuf,
}

impl RunMarker {
    /// Writes the marker under `cwd`'s `.dart_tool`; `None` (and the
    /// floored-clock fallback in the caller) when it cannot be written
    /// — a read-only tree must not fail the build over a cache gate.
    fn create(cwd: &Path) -> Option<RunMarker> {
        let path = cwd
            .join(".dart_tool")
            .join(format!("dartq-bcache-run-{}", std::process::id()));
        let dir = path.parent()?;
        fs::create_dir_all(dir).ok()?;
        fs::write(&path, b"").ok()?;
        Some(RunMarker { path })
    }

    /// The gate mtime. Statting our own just-written file cannot race
    /// anything meaningful, but a missing stat falls back to the
    /// floored clock rather than guessing.
    fn touch_time(&self) -> SystemTime {
        fs::metadata(&self.path)
            .and_then(|meta| meta.modified())
            .unwrap_or_else(|_| now_floored_to_secs())
    }
}

impl Drop for RunMarker {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}

/// How long a batch command waits at the job gate before it fails.
const GATE_WAIT: Duration = Duration::from_secs(30 * 60);

/// The flutter command `argv` runs and whether it is batch work: the
/// first non-option token (`build` with its target), walking pre-command
/// options as `package:args` would. `None` for a line without one.
fn shim_command(argv: &[String]) -> Option<(String, bool)> {
    let mut words = Vec::new();
    let mut i = 0;
    while i < argv.len() && words.len() < 2 {
        let token = argv[i].as_str();
        if token == "--" {
            break;
        }
        if token.starts_with('-') {
            let consumes_next = matches!(token, "-d" | "-P" | "--device-id" | "--local-engine")
                || token == "--local-engine-src-path"
                || token == "--local-engine-host"
                || token == "--local-web-sdk";
            i += usize::from(consumes_next) + 1;
            continue;
        }
        words.push(token);
        if words[0] != "build" {
            break;
        }
        i += 1;
    }
    let verb = *words.first()?;
    let batch = matches!(verb, "build" | "assemble" | "test" | "drive");
    Some((words.join(" "), batch))
}

/// A gate lease and the peak key it records under.
struct Gate {
    #[cfg(unix)]
    _lease: dartq_toolchain::ledger::Lease,
    #[cfg_attr(not(all(unix, not(test))), allow(dead_code))]
    key: String,
}

/// The job gate of one shim invocation over this process's tree (the
/// flutter child and everything it spawns):
///
/// * a batch command (`build *`, `assemble`, `test`, `drive`) takes a
///   slot and its memory — the recorded peak of its project and command,
///   else the largest live build's, else the default — waiting as the
///   gate decides; a batch command spawned under another dartq job's slot
///   (an `assemble` behind `flutter build`) joins that slot;
/// * an interactive command (`run`) books its memory at once and never
///   waits: debug work is never held behind analysis or tests;
/// * anything else (`attach`, `pub`, `doctor`, …) runs ungated.
///
/// `None` runs ungated; the error is a batch wait that timed out.
#[cfg(unix)]
fn gate(
    argv: &[String],
    cwd: &Path,
    config: &ShimConfig,
) -> std::result::Result<Option<Gate>, String> {
    use dartq_toolchain::ledger::{self, Face, Request};
    let Some((command, batch)) = shim_command(argv) else {
        return Ok(None);
    };
    if !batch && command != "run" {
        return Ok(None);
    }
    let cwd = fs::canonicalize(cwd).unwrap_or_else(|_| cwd.to_path_buf());
    let key = format!("build:{}:{command}", cwd.display());
    let (bytes, sized) = ledger::sizing(&key, Face::Build);
    let request = Request {
        face: if batch { Face::Build } else { Face::Debug },
        label: cwd.display().to_string(),
        bytes,
        pid: Some(std::process::id()),
        release: None,
        slot: batch,
        cap: ledger::cap(config.memory_max_concurrent),
        sized,
        peak_key: Some(key.clone()),
    };
    if !batch {
        return Ok(ledger::book(request).map(|lease| Gate { _lease: lease, key }));
    }
    let deadline = std::time::Instant::now() + GATE_WAIT;
    match ledger::acquire(
        request,
        bytes,
        config.memory_max_percent,
        deadline,
        |message| {
            eprintln!("{message}");
        },
    ) {
        Ok(lease) => Ok(lease.map(|lease| Gate { _lease: lease, key })),
        Err(error @ ledger::Error::Timeout { .. }) => Err(error.to_string()),
        Err(_) => Ok(None),
    }
}

#[cfg(not(unix))]
fn gate(_: &[String], _: &Path, _: &ShimConfig) -> std::result::Result<Option<Gate>, String> {
    Ok(None)
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
        let mut all = capture_templates(argv, cwd, engine_revision, true).expect("variants");
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
        write(
            &project.join("pubspec.yaml"),
            b"name: app\nversion: 1.0.0\n",
        );
        write(&project.join("pubspec.lock"), b"packages: {}\n");
        write(
            &project.join(".dart_tool").join("package_config.json"),
            br#"{"configVersion":2,"packages":[{"name":"app","rootUri":"../","packageUri":"lib/"}]}"#,
        );
        write(&project.join("lib").join("main.dart"), b"void main() {}\n");
        project
    }

    fn escape_path(path: &Path) -> String {
        path.display()
            .to_string()
            .replace('\\', "\\\\")
            .replace(' ', "\\ ")
    }

    /// Fabricates the `.dart_tool/flutter_build/<name>` dir a flutter run
    /// leaves behind: a payload dill, a depfile recording the tree's
    /// sources, and a stamp embedding the project root and the dir's own
    /// flutter_build path (the rewrite probes). Under
    /// capture-not-predict the name is whatever the run observed, so
    /// callers pick it.
    fn build_fixture(project: &Path, name: &str) -> PathBuf {
        let dir = project.join(".dart_tool").join("flutter_build").join(name);
        write(&dir.join("app.dill"), b"kernel payload bytes");
        let depfile = format!(
            "{}: {} {}\n",
            escape_path(&dir.join("app.dill")),
            escape_path(&project.join("lib").join("main.dart")),
            escape_path(&project.join(".dart_tool").join("package_config.json")),
        );
        write(&dir.join("kernel_snapshot.d"), depfile.as_bytes());
        let stamp = format!(
            r#"{{"inputs":["{}/lib/main.dart"],"outputs":["{}/app.dill"]}}"#,
            project.display(),
            dir.display()
        );
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
            memory_max_percent: 90,
            memory_max_concurrent: 0,
        }
    }

    const BUNDLE_ARGV: [&str; 5] = ["build", "bundle", "--release", "--dart-define", "API=prod"];
    const APK_ARGV: [&str; 3] = ["build", "apk", "--release"];

    /// An arbitrary config-hash dir name: under capture-not-predict the
    /// tests never need to derive it, only to reuse what a run observed.
    const DIR_MAIN: &str = "2fe6e51a1c0b3d4e5f60718293a4b5c6";
    /// A second arbitrary name: the contention test's dir that only a
    /// PRE materialization could ever place.
    const DIR_ALT: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    /// Set in the child [`isolated`] starts: the test runs its body there.
    const ISOLATED_ENV: &str = "DARTQ_TEST_ISOLATED";

    /// Runs test `name` of this binary alone in a fresh child process with
    /// a private ledger (a fresh directory, room to spare, no inherited
    /// job token or cap override), and panics with its output unless it
    /// passed. In-process, the ledger directory is process-wide
    /// environment and the slots a process holds are process-wide state:
    /// any other test's batch build would share both. `true` in the
    /// parent (the caller returns); `false` in the child.
    #[cfg(unix)]
    fn isolated(name: &str) -> bool {
        use dartq_toolchain::ledger;
        if std::env::var_os(ISOLATED_ENV).is_some() {
            return false;
        }
        let ledger_dir = tempfile::tempdir().expect("ledger dir");
        let output = std::process::Command::new(std::env::current_exe().expect("test binary"))
            .args([name, "--exact", "--test-threads", "1", "--nocapture"])
            .env(ISOLATED_ENV, "1")
            .env(ledger::DIR_ENV, ledger_dir.path())
            .env(ledger::AVAILABLE_ENV, (1u64 << 40).to_string())
            .env_remove(ledger::TOKEN_ENV)
            .env_remove(ledger::CAP_ENV)
            .output()
            .expect("run the isolated test");
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            output.status.success() && stdout.contains("1 passed"),
            "isolated test {name} failed ({}):\n{stdout}\n{}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
        true
    }

    /// A build holds a memory lease labelled with its tree for exactly the
    /// child's run: present while the runner runs, gone once it returned.
    #[cfg(unix)]
    #[test]
    fn a_build_holds_its_memory_lease_while_flutter_runs() {
        use dartq_toolchain::ledger::{Face, leases};
        if isolated("orchestrator::tests::a_build_holds_its_memory_lease_while_flutter_runs") {
            return;
        }
        let (_guard, tmp) = tempdir();
        let sdk = fake_flutter_sdk(&tmp, "3.47.1");
        let config = config_for(&tmp.join("cas"), &sdk.join("bin").join("flutter"));
        let tree = synthetic_project(&tmp, "lease");
        let label = tree.display().to_string();
        let mine = move || {
            leases()
                .into_iter()
                .filter(|(face, held, _)| *face == Face::Build && *held == label)
                .count()
        };
        let during = std::rc::Rc::new(std::cell::Cell::new(usize::MAX));
        let seen = std::rc::Rc::clone(&during);
        let probe = mine.clone();
        let mut runner = FakeRunner {
            code: 0,
            argv: Vec::new(),
            effect: Some(Box::new(move |_| seen.set(probe()))),
        };
        let report = run_shim(&raw_argv(&APK_ARGV), &tree, &config, &mut runner);
        assert_eq!(report.exit_code, Some(0));
        assert_eq!(during.get(), 1, "one lease while flutter runs");
        assert_eq!(mine(), 0, "released once flutter exited");
    }

    /// `flutter run` through the shim is debug work: under cap 1 with the
    /// only slot held elsewhere it starts at once, booking its memory
    /// without a slot.
    #[cfg(unix)]
    #[test]
    fn flutter_run_never_waits_for_a_job_slot() {
        use dartq_toolchain::ledger::{self, Face, Request, leases};
        if isolated("orchestrator::tests::flutter_run_never_waits_for_a_job_slot") {
            return;
        }
        let _slot = ledger::acquire(
            Request {
                face: Face::Test,
                label: "other".into(),
                bytes: 1 << 20,
                pid: None,
                release: None,
                slot: true,
                cap: 1,
                sized: true,
                peak_key: None,
            },
            1 << 20,
            90,
            std::time::Instant::now() + Duration::from_secs(5),
            |_| {},
        )
        .expect("the slot is free")
        .expect("a ledger");
        let (_guard, tmp) = tempdir();
        let sdk = fake_flutter_sdk(&tmp, "3.47.1");
        let mut config = config_for(&tmp.join("cas"), &sdk.join("bin").join("flutter"));
        config.memory_max_concurrent = 1;
        let tree = synthetic_project(&tmp, "run");
        let label = tree.display().to_string();
        let during = std::rc::Rc::new(std::cell::Cell::new(false));
        let seen = std::rc::Rc::clone(&during);
        let mut runner = FakeRunner {
            code: 0,
            argv: Vec::new(),
            effect: Some(Box::new(move |_| {
                seen.set(
                    leases()
                        .iter()
                        .any(|(face, held, _)| *face == Face::Debug && *held == label),
                );
            })),
        };
        let started = std::time::Instant::now();
        let report = run_shim(
            &raw_argv(&["run", "-d", "macos"]),
            &tree,
            &config,
            &mut runner,
        );
        assert!(started.elapsed() < Duration::from_secs(2), "no wait");
        assert_eq!(report.exit_code, Some(0));
        assert!(during.get(), "the app's memory is booked while it runs");
    }

    #[test]
    fn fresh_tree_pre_hit_materializes_recorded_dirs() {
        let (_guard, tmp) = tempdir();
        let sdk = fake_flutter_sdk(&tmp, "3.47.1");
        let store_root = tmp.join("cas");
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

        let dir_b = tree_b
            .join(".dart_tool")
            .join("flutter_build")
            .join(DIR_MAIN);
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
        let store_root = tmp.join("cas");
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
        write(
            &tree_b.join("lib").join("main.dart"),
            b"void main() { edited(); }\n",
        );
        let mut runner = FakeRunner::exits(0);
        let report = run_shim(&raw_argv(&BUNDLE_ARGV), &tree_b, &config, &mut runner);
        assert_eq!(report.action, Action::PreHitMaterialized);
        assert_eq!(report.key.as_deref(), Some(key.as_str()));
        assert!(
            tree_b
                .join(".dart_tool")
                .join("flutter_build")
                .join(DIR_MAIN)
                .join("app.dill")
                .is_file()
        );
    }

    /// Audit finding A1, end to end: a recorded dir whose bookkeeping
    /// names an absolute path under NONE of the ingest anchors (here, a
    /// path dependency inside another checkout) is refused by a
    /// cross-tree replay — flutter's input validation would resolve the
    /// recorded path, see it unchanged, and skip, running the ingest
    /// tree's compiled code — while a within-tree hit replays fine
    /// (the recorded paths are valid where they were recorded).
    #[test]
    fn foreign_path_dir_refused_cross_tree_replays_in_tree() {
        let (_guard, tmp) = tempdir();
        let sdk = fake_flutter_sdk(&tmp, "3.47.1");
        let store_root = tmp.join("cas");
        let config = config_for(&store_root, &sdk.join("bin").join("flutter"));

        // Tree A seeds an entry whose depfile also names an input
        // outside every anchor.
        let tree_a = synthetic_project(&tmp, "a");
        let mut runner = FakeRunner {
            code: 0,
            argv: Vec::new(),
            effect: Some(Box::new(|cwd| {
                let dir = build_fixture(cwd, DIR_MAIN);
                let depfile = format!(
                    "{}: {}\n",
                    escape_path(&dir.join("app.dill")),
                    // Fictional absolute path of another checkout: the
                    // classification only needs it outside the anchors.
                    "/opt/other-checkout/packages/tools/lib/a.dart"
                );
                write(&dir.join("foreign_input.d"), depfile.as_bytes());
            })),
        };
        let seeded = run_shim(&raw_argv(&BUNDLE_ARGV), &tree_a, &config, &mut runner);
        assert_eq!(seeded.action, Action::Ingested);
        let key = seeded.key.expect("seed key");
        let store = CasStore::open(&store_root).expect("open store");
        let manifest = store.lookup(&key).expect("lookup").expect("entry");
        assert!(
            manifest.dirs[0].files.iter().any(|file| file.foreign_paths),
            "the ingest classifies the foreign input"
        );

        // A different tree (no enclosing repo: each project dir is its
        // own worktree root, so the roots differ): the hit refuses the
        // dir instead of replaying bookkeeping this tree cannot honor.
        let tree_b = synthetic_project(&tmp, "b");
        let mut runner = FakeRunner::exits(0);
        let report = run_shim(&raw_argv(&BUNDLE_ARGV), &tree_b, &config, &mut runner);
        assert_eq!(report.reason, "entry_not_replayable");
        assert_eq!(report.skipped_dirs, 1);
        assert!(
            !tree_b
                .join(".dart_tool")
                .join("flutter_build")
                .join(DIR_MAIN)
                .exists(),
            "the foreign dir never materializes cross-tree"
        );

        // Within tree A the hit replays: fill-only-missing finds every
        // file present and leaves the valid dir alone.
        let mut runner = FakeRunner::exits(0);
        let report = run_shim(&raw_argv(&BUNDLE_ARGV), &tree_a, &config, &mut runner);
        assert_eq!(report.action, Action::PreHitMaterialized);
        assert_eq!(report.skipped_dirs, 0);
        assert_eq!(
            fs::read_to_string(
                tree_a
                    .join(".dart_tool")
                    .join("flutter_build")
                    .join(DIR_MAIN)
                    .join("app.dill")
            )
            .expect("the tree's own dir survives"),
            "kernel payload bytes"
        );
    }

    /// Audit finding A5, end to end: a fresh worktree has no
    /// `.dart_tool` at all (flutter's implicit `pub get` has not run),
    /// so its pre-build key must equal the ingest key — the hit fires
    /// before the child runs. `.dart_tool/package_config.json` is not
    /// in the key preimage for exactly this reason; flutter's own
    /// input validation catches a diverged config after a hit.
    #[test]
    fn fresh_worktree_without_dart_tool_hits_the_ingest_key() {
        let (_guard, tmp) = tempdir();
        let sdk = fake_flutter_sdk(&tmp, "3.47.1");
        let store_root = tmp.join("cas");
        let config = config_for(&store_root, &sdk.join("bin").join("flutter"));

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

        // The fresh worktree: checked-out files only.
        let tree_b = tmp.join("app-b");
        write(&tree_b.join("pubspec.yaml"), b"name: app\nversion: 1.0.0\n");
        write(&tree_b.join("pubspec.lock"), b"packages: {}\n");
        write(&tree_b.join("lib").join("main.dart"), b"void main() {}\n");
        assert!(!tree_b.join(".dart_tool").exists());
        let mut runner = FakeRunner {
            code: 0,
            argv: Vec::new(),
            effect: Some(Box::new(|cwd| {
                assert!(
                    cwd.join(".dart_tool")
                        .join("flutter_build")
                        .join(DIR_MAIN)
                        .join("app.dill")
                        .is_file(),
                    "the hit materializes before the child runs"
                );
            })),
        };
        let report = run_shim(&raw_argv(&BUNDLE_ARGV), &tree_b, &config, &mut runner);
        assert_eq!(report.action, Action::PreHitMaterialized);
        assert_eq!(report.key.as_deref(), Some(key.as_str()));
    }

    /// Audit finding A3's refresh leg, end to end: a hit whose rerun
    /// rewrote an output must re-ingest it — on the bundle face no
    /// assemble child is ever observed, so the old `already_stored`
    /// shortcut (hit + same key + no captures) froze the entry on the
    /// seeding tree's outputs and every later rebuild was replayed
    /// stale forever.
    #[test]
    fn hit_rerun_that_rewrites_outputs_refreshes_the_entry() {
        let (_guard, tmp) = tempdir();
        let sdk = fake_flutter_sdk(&tmp, "3.47.1");
        let store_root = tmp.join("cas");
        let config = config_for(&store_root, &sdk.join("bin").join("flutter"));

        let tree = synthetic_project(&tmp, "a");
        let mut runner = FakeRunner {
            code: 0,
            argv: Vec::new(),
            effect: Some(Box::new(|cwd| {
                build_fixture(cwd, DIR_MAIN);
            })),
        };
        let seeded = run_shim(&raw_argv(&BUNDLE_ARGV), &tree, &config, &mut runner);
        assert_eq!(seeded.action, Action::Ingested);
        let key = seeded.key.expect("seed key");
        let store = CasStore::open(&store_root).expect("open store");

        // The hit rerun rewrites ONLY the payload (the stale-hit shape:
        // flutter's validation rebuilt the kernel over diverged
        // sources). The entry must follow.
        let mut runner = FakeRunner {
            code: 0,
            argv: Vec::new(),
            effect: Some(Box::new(|cwd| {
                write(
                    &cwd.join(".dart_tool")
                        .join("flutter_build")
                        .join(DIR_MAIN)
                        .join("app.dill"),
                    b"kernel payload bytes v2",
                );
            })),
        };
        let report = run_shim(&raw_argv(&BUNDLE_ARGV), &tree, &config, &mut runner);
        assert_eq!(report.action, Action::PreHitMaterialized);
        assert_eq!(report.ingest_files, 1, "the rewritten payload re-stores");
        let manifest = store.lookup(&key).expect("lookup").expect("entry");
        let dill = manifest.dirs[0]
            .files
            .iter()
            .find(|file| file.path == Path::new("app.dill"))
            .expect("payload record");
        assert_eq!(
            dill.blob,
            BlobId(*blake3::hash(b"kernel payload bytes v2").as_bytes()),
            "the entry no longer freezes on the seed outputs"
        );
    }

    /// A6 at the name-recompute seam: define values that are absolute
    /// paths under the ingest worktree root re-aim at the target tree
    /// (Xcode's `-dSrcRoot` shape), everything else replays verbatim —
    /// with a boundary guard against byte-prefix lookalikes.
    #[test]
    fn replayed_hash_name_reroots_define_values_and_output_dir() {
        let defines = |src_root: &str| {
            BTreeMap::from([
                ("SrcRoot".to_owned(), src_root.to_owned()),
                ("BuildMode".to_owned(), "release".to_owned()),
            ])
        };
        let dir = |src_root: &str, output_dir: &str| DirRecord {
            name_observed: String::from("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
            name_rule: NameRule::RecomputeFromTemplate,
            name_defines: defines(src_root),
            name_output_dir: output_dir.to_owned(),
            files: Vec::new(),
        };
        let target = Path::new("/new/wt");

        // The plain case: both the define value and the output dir
        // re-root, and the name is exactly what flutter in the target
        // tree computes for itself.
        let record = dir("/old/wt/ios", "/old/wt/build/app/intermediates");
        let expected = flutter_build_dir_name(
            "engine",
            &defines("/new/wt/ios"),
            Path::new("/new/wt/build/app/intermediates"),
        );
        assert_eq!(
            replayed_hash_name(&record, "/old/wt", target, "engine"),
            expected
        );

        // Lookalike prefixes and foreign values stay verbatim.
        let record = dir("/old/wt-lookalike/ios", "/old/wt/build");
        let expected = flutter_build_dir_name(
            "engine",
            &defines("/old/wt-lookalike/ios"),
            Path::new("/new/wt/build"),
        );
        assert_eq!(
            replayed_hash_name(&record, "/old/wt", target, "engine"),
            expected
        );
        let record = dir("/fake/pub-cache/x", "/old/wt/build");
        let expected = flutter_build_dir_name(
            "engine",
            &defines("/fake/pub-cache/x"),
            Path::new("/new/wt/build"),
        );
        assert_eq!(
            replayed_hash_name(&record, "/old/wt", target, "engine"),
            expected
        );
    }

    /// The anchor itself: the worktree root is the nearest `.git`
    /// ancestor (directory or linked-worktree file), falling back to
    /// the project root when no repo encloses it.
    #[test]
    fn worktree_root_finds_nearest_git_ancestor() {
        let (_guard, tmp) = tempdir();
        let repo = tmp.join("repo");
        write(&repo.join(".git").join("HEAD"), b"ref: refs/heads/main\n");
        let project = repo.join("apps").join("my_app");
        write(&project.join("pubspec.yaml"), b"name: app\n");
        assert_eq!(worktree_root(&project), repo);

        // A linked worktree's `.git` is a file.
        let linked = tmp.join("linked-wt");
        write(&linked.join(".git"), b"gitdir: /nowhere/.git/worktrees/x\n");
        let nested = linked.join("packages").join("tools");
        write(&nested.join("pubspec.yaml"), b"name: tools\n");
        assert_eq!(worktree_root(&nested), linked);

        // No repo anywhere above: the project root is its own anchor.
        let alone = tmp.join("standalone");
        write(&alone.join("pubspec.yaml"), b"name: alone\n");
        assert_eq!(worktree_root(&alone), alone);
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
        let store_root = tmp.join("cas");
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
        let output_b = rerooted(
            &template.output_dir,
            &manifest.worktree_root_at_ingest,
            &tree_b,
        );
        let name_b =
            flutter_build_dir_name(&engine_revision, &template.defines, Path::new(&output_b));
        assert_ne!(
            name_b, template.name,
            "the name embeds the absolute output dir"
        );
        let dir_b = tree_b
            .join(".dart_tool")
            .join("flutter_build")
            .join(&name_b);
        assert!(
            dir_b.join("app.dill").is_file(),
            "materialized under {}",
            dir_b.display()
        );
        assert!(
            !tree_b
                .join(".dart_tool")
                .join("flutter_build")
                .join(&template.name)
                .exists(),
            "the ingest-tree name never replays"
        );
        let stamp = fs::read_to_string(dir_b.join("flutter_build.dill.stamp")).expect("stamp");
        assert!(
            stamp.contains(tree_b.to_str().expect("UTF-8 root")),
            "{stamp}"
        );
        // A4 roundtrip: the recorded `flutter_build/<tree-A name>`
        // segment inside the stamp followed the dir to tree B's
        // recomputed name, root and segment together.
        assert!(
            stamp.contains(
                tree_b
                    .join(".dart_tool")
                    .join("flutter_build")
                    .join(&name_b)
                    .join("app.dill")
                    .to_str()
                    .expect("UTF-8 path")
            ),
            "the recorded output mention re-roots AND renames: {stamp}"
        );
        assert!(
            !stamp.contains(&format!("flutter_build/{}", template.name)),
            "no ingest-tree name segment survives: {stamp}"
        );
    }

    /// The real gradle defect shape, end to end: the multi-arch define
    /// VALUE carries spaces, and the kernel argv spells it as ONE
    /// element — the observer's exact parse lands the full value (the
    /// ps whitespace split that once made it ambiguous is obsolete), the
    /// POST verifies it against the dir the "build" materialized, and it
    /// replays across trees.
    #[cfg(unix)]
    #[test]
    fn spaced_define_value_lands_through_name_arbitration() {
        let (_guard, tmp) = tempdir();
        let sdk = fake_flutter_sdk(&tmp, "3.47.1");
        let flutter_bin = sdk.join("bin").join("flutter");
        let engine_revision = key::fingerprint_toolchain(&sdk)
            .expect("fingerprint")
            .engine_revision;
        let store_root = tmp.join("cas");
        let config = config_for(&store_root, &flutter_bin);

        // The multi-arch tail of the default apk build, as the kernel
        // spells it: `-dAndroidArchs=android-arm android-arm64
        // android-x64` is ONE argv element.
        let tree_a = synthetic_project(&tmp, "a");
        let output_a = tree_a.join("build/app/intermediates/flutter/debug");
        let mut argv_a = assemble_argv(&flutter_bin, &output_a.to_string_lossy());
        let archs_at = argv_a
            .iter()
            .position(|token| token == "-dAndroidArchs=android-arm64")
            .expect("the archs define token");
        argv_a[archs_at] = "-dAndroidArchs=android-arm android-arm64 android-x64".to_owned();
        let exact = capture_templates(&argv_a, &tree_a, &engine_revision, false).expect("parse");
        assert_eq!(exact.len(), 1, "an exact argv yields the one true parse");
        let full = exact.into_iter().next().expect("the full-value capture");
        assert_eq!(
            full.defines.get("AndroidArchs").map(String::as_str),
            Some("android-arm android-arm64 android-x64")
        );

        // Real flutter_tools parsed the same element: the dir lands under
        // the full-value name.
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
        let output_b = rerooted(&full.output_dir, &manifest.worktree_root_at_ingest, &tree_b);
        let name_b = flutter_build_dir_name(&engine_revision, &full.defines, Path::new(&output_b));
        assert!(
            tree_b
                .join(".dart_tool")
                .join("flutter_build")
                .join(&name_b)
                .join("app.dill")
                .is_file()
        );
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
        let store_root = tmp.join("cas");
        let config = config_for(&store_root, &flutter_bin);

        let tree = synthetic_project(&tmp, "a");
        let orphan_argv = assemble_argv(&flutter_bin, "/opt/nowhere/failed");
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
        let store_root = tmp.join("cas");
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
        assert_eq!(
            by_rule(NameRule::RecomputeFromTemplate).name_observed,
            template.name
        );
        assert_eq!(by_rule(NameRule::NotReplayable).name_observed, internal);

        // The fresh tree's hit materializes the replayable dir and skips
        // the internal one; the action stays a materialized pre-hit.
        let tree_b = synthetic_project(&tmp, "b");
        let mut runner = FakeRunner::exits(0);
        let report = run_shim(&raw_argv(&APK_ARGV), &tree_b, &config, &mut runner);
        assert_eq!(report.action, Action::PreHitMaterialized);
        assert_eq!(report.skipped_dirs, 1);
        let output_b = rerooted(
            &template.output_dir,
            &manifest.worktree_root_at_ingest,
            &tree_b,
        );
        let name_b =
            flutter_build_dir_name(&engine_revision, &template.defines, Path::new(&output_b));
        assert!(
            tree_b
                .join(".dart_tool")
                .join("flutter_build")
                .join(&name_b)
                .join("app.dill")
                .is_file()
        );
        assert!(
            !tree_b
                .join(".dart_tool")
                .join("flutter_build")
                .join(internal)
                .exists()
        );
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
        let store_root = tmp.join("cas");
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
        let store_root = tmp.join("cas");
        let config = config_for(&store_root, &sdk.join("bin").join("flutter"));
        let mut runner = FakeRunner::exits(7);
        let report = run_shim(&raw_argv(&BUNDLE_ARGV), &tree, &config, &mut runner);

        assert_eq!(report.action, Action::Passthrough);
        assert_eq!(report.reason, "unsupported_flutter_version");
        assert_eq!(report.exit_code, Some(7), "the child's exit code wins");
        assert!(report.key.is_none());
        // The negative fingerprint record is the ONLY store side effect:
        // no blobs, no entries — the cache stays untouched as a cache.
        let meta = store_root.join("meta");
        assert!(meta.join("toolchain-unsupported.json").is_file());
        assert!(!meta.join("toolchain.json").exists());
        assert!(!store_root.join("blobs").exists());
        assert!(!store_root.join("entries").exists());
    }

    /// The negative record's purpose: a second invocation of the same
    /// unsupported SDK state serves the fingerprint from the record
    /// without rehashing — proven by rewriting the hashed artifact's
    /// CONTENTS at the same size and mtime (the identity the staleness
    /// rule trusts): a rehash would produce a different fingerprint, a
    /// cache hit returns the recorded one. A real artifact change
    /// (different size) invalidates the record and rehashes.
    #[test]
    fn unsupported_version_negative_cache_skips_the_rehash() {
        let (_guard, tmp) = tempdir();
        let sdk = fake_flutter_sdk(&tmp, "3.50.0");
        let store_root = tmp.join("cas");
        let tree = synthetic_project(&tmp, "app");
        let config = config_for(&store_root, &sdk.join("bin").join("flutter"));
        let mut runner = FakeRunner::exits(0);
        let report = run_shim(&raw_argv(&BUNDLE_ARGV), &tree, &config, &mut runner);
        assert_eq!(report.reason, "unsupported_flutter_version");

        let record = store_root.join("meta").join("toolchain-unsupported.json");
        let recorded: ToolchainCache =
            serde_json::from_slice(&fs::read(&record).expect("record")).expect("parse record");

        // Same bytes length, same mtime: identity-identical, content
        // different — only a cache hit can return the old fingerprint.
        let dill = sdk
            .join("bin")
            .join("cache")
            .join("dart-sdk")
            .join("lib")
            .join("_internal")
            .join("vm_platform_strong.dill");
        let metadata = fs::metadata(&dill).expect("stat dill");
        let mtime = metadata.modified().expect("mtime");
        let changed = vec![b'X'; metadata.len() as usize];
        assert_ne!(changed, fs::read(&dill).expect("read dill"));
        fs::write(&dill, &changed).expect("rewrite dill");
        let file = fs::File::options()
            .write(true)
            .open(&dill)
            .expect("open dill");
        file.set_times(fs::FileTimes::new().set_modified(mtime))
            .expect("restore mtime");
        drop(file);

        let cached = cached_fingerprint(&sdk, &store_root).expect("fingerprint");
        assert_eq!(
            cached, recorded.fingerprint,
            "the identity-matching negative record serves without a rehash"
        );

        // A real change (the size moves) invalidates the record.
        fs::write(&dill, b"platform dill bytes, changed").expect("grow dill");
        let fresh = cached_fingerprint(&sdk, &store_root).expect("fingerprint");
        assert_ne!(
            fresh.platform_strong_dill,
            recorded.fingerprint.platform_strong_dill
        );
    }

    // ---- the multi-line model gate ---------------------------------

    #[test]
    fn version_line_parses_release_stamps_only() {
        assert_eq!(version_line("3.47.1"), Some((3, 47)));
        assert_eq!(version_line("3.47.0"), Some((3, 47)));
        assert_eq!(version_line("0.0.0"), Some((0, 0)));
        // Pre-release and build suffixes, short stamps, and malformed
        // spellings match no line: the gate stays exact-shape.
        for stamp in [
            "3.47.0-1.0.pre",
            "3.47.1+hotfix",
            "3.47",
            "3.47.1.2",
            "3.47.x",
            "3.47.-1",
            "3.47.+1",
            "v3.47.1",
            "",
            "unknown",
        ] {
            assert_eq!(version_line(stamp), None, "{stamp}");
        }
    }

    #[test]
    fn version_gate_admits_exactly_the_supported_lines() {
        for stamp in [
            "3.32.0", "3.35.0", "3.38.1", "3.41.9", "3.44.0", "3.44.3", "3.47.0", "3.47.1",
        ] {
            assert!(supported_flutter_version(stamp), "{stamp}");
        }
        for stamp in [
            "3.29.3",
            "3.29.0",
            "3.27.0",
            "3.24.5",
            "3.19.6",
            "3.10.6",
            "3.33.0",
            "3.46.0",
            "3.48.0",
            "3.47.0-1.0.pre",
        ] {
            assert!(!supported_flutter_version(stamp), "{stamp}");
        }
    }

    /// The per-line golden: a toolchain stamped `stamp` runs the full
    /// modeled path — the gate admits its LINE — and the fingerprint
    /// record lands under the supported name.
    fn assert_full_shim_path(stamp: &str) {
        let (_guard, tmp) = tempdir();
        let sdk = fake_flutter_sdk(&tmp, stamp);
        let store_root = tmp.join("cas");
        let config = config_for(&store_root, &sdk.join("bin").join("flutter"));
        let tree = synthetic_project(&tmp, "app");
        let mut runner = FakeRunner {
            code: 0,
            argv: Vec::new(),
            effect: Some(Box::new(|cwd| {
                build_fixture(cwd, DIR_MAIN);
            })),
        };
        let report = run_shim(&raw_argv(&BUNDLE_ARGV), &tree, &config, &mut runner);
        assert_eq!(report.action, Action::Ingested, "{stamp}");
        assert_eq!(report.reason, "pre_miss", "{stamp}");
        assert!(report.key.is_some(), "the gate admitted {stamp}");
        let meta = store_root.join("meta");
        assert!(meta.join("toolchain.json").is_file(), "{stamp}");
        assert!(!meta.join("toolchain-unsupported.json").exists(), "{stamp}");
    }

    /// 3.47.0: the baseline line's other installed patch.
    #[test]
    fn supported_line_patch_runs_the_full_shim_path() {
        assert_full_shim_path("3.47.0");
    }

    #[test]
    fn supported_line_3_44_runs_the_full_shim_path() {
        assert_full_shim_path("3.44.0");
    }

    #[test]
    fn supported_line_3_41_runs_the_full_shim_path() {
        assert_full_shim_path("3.41.9");
    }

    #[test]
    fn supported_line_3_38_runs_the_full_shim_path() {
        assert_full_shim_path("3.38.1");
    }

    #[test]
    fn supported_line_3_35_runs_the_full_shim_path() {
        assert_full_shim_path("3.35.0");
    }

    #[test]
    fn supported_line_3_32_runs_the_full_shim_path() {
        assert_full_shim_path("3.32.0");
    }

    // The latest patch of each line, pinned after its region diff was
    // classified hunk by hunk (see [`REVIEWED_LINES`] and the per-line
    // notes on [`SUPPORTED_FLUTTER_LINES`]).
    #[test]
    fn supported_line_3_32_8_runs_the_full_shim_path() {
        assert_full_shim_path("3.32.8");
    }

    #[test]
    fn supported_line_3_35_7_runs_the_full_shim_path() {
        assert_full_shim_path("3.35.7");
    }

    #[test]
    fn supported_line_3_38_10_runs_the_full_shim_path() {
        assert_full_shim_path("3.38.10");
    }

    #[test]
    fn supported_line_3_44_9_runs_the_full_shim_path() {
        assert_full_shim_path("3.44.9");
    }

    #[test]
    fn supported_line_3_47_9_runs_the_full_shim_path() {
        assert_full_shim_path("3.47.9");
    }

    // ---- the flutter_tools source-region harness -------------------
    //
    // The mechanical basis of [`SUPPORTED_FLUTTER_LINES`]: each enabled
    // line pins the digests of the flutter_tools source regions of
    // every patch that was classified hunk by hunk, and every installed
    // patch of the line must hash to one of them. Everything here
    // except the table check
    // reads an fvm-managed SDK checkout and therefore SKIPS (or, for the
    // probe, refuses to run) when none is present — a CI machine has no
    // fvm. `DARTQ_FVM_HOME` relocates the versions root (default
    // `$HOME/fvm`).

    /// Each enabled line's reviewed flutter_tools: EVERY patch whose
    /// [`MODEL_REGIONS`] were classified hunk by hunk against the line's
    /// derivation baseline (see [`SUPPORTED_FLUTTER_LINES`]) with its
    /// [`region_digest`], oldest patch first. 3.47.1 is the model's
    /// derivation source. An installed patch of an enabled line passes
    /// the tripwire when its digest matches ANY of its line's pins; a
    /// line's later pins carry the patch-review notes recorded on the
    /// line in [`SUPPORTED_FLUTTER_LINES`].
    const REVIEWED_LINES: &[((u32, u32), &[(&str, &str)])] = &[
        (
            (3, 32),
            &[
                ("3.32.0", "1fe30f9f57945a3d22b456901db0fe7b"),
                ("3.32.8", "1e6686488d29d1bab4dedc53131ae307"),
            ],
        ),
        (
            (3, 35),
            &[
                ("3.35.0", "8cf08ac333fd85a13159e6202d0b6247"),
                ("3.35.7", "5827e14e6a1295cd73807ad7a4bd9f98"),
            ],
        ),
        (
            (3, 38),
            &[
                ("3.38.1", "2f3efc0a8d8518f49c4c9ea60ee49cd8"),
                ("3.38.10", "12531edd865367e7ae2f50b58018963f"),
            ],
        ),
        ((3, 41), &[("3.41.9", "8353dd6f37388a63c6d073d6ba4df1da")]),
        (
            (3, 44),
            &[
                ("3.44.0", "23a71b4ec7214f7726638d99cd27387f"),
                ("3.44.9", "23a71b4ec7214f7726638d99cd27387f"),
            ],
        ),
        (
            (3, 47),
            &[
                ("3.47.1", "2f814ff47408e470995eb5720ec8fbc4"),
                ("3.47.9", "02a9960e7b34d044503fc7d95f5490a8"),
            ],
        ),
    ];

    /// The flutter_tools source regions the model generation pins, as
    /// paths under `packages/flutter_tools/`:
    ///
    /// * the runner pair — the GLOBAL option table
    ///   (`flutter_command_runner.dart`) and every shared option helper
    ///   the command grammars call (`flutter_command.dart`);
    /// * the modeled commands' own declarations — the per-subcommand
    ///   option tables and default build modes (`build*.dart`, plus
    ///   `build.dart`'s command registry), `run.dart`, `test.dart`, and
    ///   the assemble argv model (`assemble.dart`);
    /// * the build-dir name formula and `.filecache`/persist contract
    ///   (`build_system.dart`, `file_store.dart`);
    /// * the deferred-components target names the assemble gate keys
    ///   (`targets/android.dart`);
    /// * `build_info.dart` — `getBuildDirectory`, the injected-define
    ///   set, `getDefaultCachedKernelPath`, and
    ///   `toBuildSystemEnvironment` (the bundle face's define map);
    /// * `bundle_builder.dart` — the `build bundle` define map and
    ///   asset-dir handling;
    /// * `test_compiler.dart` — the `<buildDir>/test_cache` kernel-cache
    ///   path;
    /// * `pubspec.yaml` — the `package:args` pin whose parser semantics
    ///   the argv model mirrors.
    ///
    /// A file that moved or vanished counts as drift (see
    /// [`region_digest`]).
    const MODEL_REGIONS: &[&str] = &[
        "lib/src/runner/flutter_command_runner.dart",
        "lib/src/runner/flutter_command.dart",
        "lib/src/commands/build.dart",
        "lib/src/commands/build_apk.dart",
        "lib/src/commands/build_appbundle.dart",
        "lib/src/commands/build_bundle.dart",
        "lib/src/commands/build_ios.dart",
        "lib/src/commands/build_linux.dart",
        "lib/src/commands/build_macos.dart",
        "lib/src/commands/build_web.dart",
        "lib/src/commands/build_windows.dart",
        "lib/src/commands/run.dart",
        "lib/src/commands/test.dart",
        "lib/src/commands/assemble.dart",
        "lib/src/build_system/build_system.dart",
        "lib/src/build_system/file_store.dart",
        "lib/src/build_system/targets/android.dart",
        "lib/src/build_info.dart",
        "lib/src/bundle_builder.dart",
        "lib/src/test/test_compiler.dart",
        "pubspec.yaml",
    ];

    /// The fvm versions root: `$DARTQ_FVM_HOME/versions`, else
    /// `$HOME/fvm/versions`; `None` when it does not exist (no fvm on
    /// this machine — the region tests then skip).
    fn fvm_versions_root() -> Option<PathBuf> {
        let root = match std::env::var_os("DARTQ_FVM_HOME") {
            Some(home) => PathBuf::from(home).join("versions"),
            None => PathBuf::from(std::env::var_os("HOME")?)
                .join("fvm")
                .join("versions"),
        };
        root.is_dir().then_some(root)
    }

    /// `frameworkVersion` of one version dir's flutter.version.json —
    /// the same stamp the toolchain fingerprint reads.
    fn version_stamp(root: &Path, version: &str) -> Option<String> {
        let manifest = fs::read(
            root.join(version)
                .join("bin")
                .join("cache")
                .join("flutter.version.json"),
        )
        .ok()?;
        serde_json::from_str::<serde_json::Value>(&String::from_utf8_lossy(&manifest))
            .ok()?
            .get("frameworkVersion")?
            .as_str()
            .filter(|stamp| !stamp.is_empty())
            .map(str::to_owned)
    }

    /// Component-wise numeric stamp order (`3.38.9` < `3.38.10`; a
    /// non-numeric component parses as `None` and sorts lowest).
    fn stamp_order(a: &str, b: &str) -> std::cmp::Ordering {
        let components = |stamp: &str| {
            stamp
                .split('.')
                .map(|part| part.parse::<u64>().ok())
                .collect::<Vec<_>>()
        };
        components(a).cmp(&components(b))
    }

    /// Every fvm version dir that carries a readable stamp, oldest
    /// stamp first (component-wise numeric; ties keep directory order —
    /// the order is presentation only, never a correctness input).
    fn installed_versions(root: &Path) -> Vec<(String, String)> {
        let mut versions: Vec<(String, String)> = fs::read_dir(root)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|entry| entry.path().is_dir())
            .filter_map(|entry| {
                let name = entry.file_name().into_string().ok()?;
                let stamp = version_stamp(root, &name)?;
                Some((name, stamp))
            })
            .collect();
        versions.sort_by(|a, b| stamp_order(&a.1, &b.1).then_with(|| a.0.cmp(&b.0)));
        versions
    }

    /// The md5 of `version`'s [`MODEL_REGIONS`]: per region in table
    /// order, its path, a NUL, its byte length (u64 LE), and its bytes.
    /// `None` when a region is missing — a moved or vanished file is
    /// drift.
    fn region_digest(root: &Path, version: &str) -> Option<String> {
        use md5::{Digest, Md5};
        let tools = root.join(version).join("packages").join("flutter_tools");
        let mut md5 = Md5::new();
        for region in MODEL_REGIONS {
            let bytes = fs::read(tools.join(region)).ok()?;
            md5.update(region.as_bytes());
            md5.update([0_u8]);
            md5.update((bytes.len() as u64).to_le_bytes());
            md5.update(&bytes);
        }
        Some(format!("{:x}", md5.finalize()))
    }

    /// The [`REVIEWED_LINES`] row of `stamp`'s line, if it is enabled:
    /// the line with its reviewed (patch, digest) pins.
    fn reviewed_line(
        stamp: &str,
    ) -> Option<&'static ((u32, u32), &'static [(&'static str, &'static str)])> {
        let line = version_line(stamp)?;
        REVIEWED_LINES
            .iter()
            .find(|(reviewed, _)| *reviewed == line)
    }

    /// The kernel files every flutter SDK ships in its own format: the
    /// Dart VM platform and flutter's patched-SDK platform.
    fn platform_dills(root: &Path, version: &str) -> [PathBuf; 2] {
        let cache = root.join(version).join("bin").join("cache");
        [
            cache
                .join("dart-sdk")
                .join("lib")
                .join("_internal")
                .join("vm_platform_strong.dill"),
            cache
                .join("artifacts")
                .join("engine")
                .join("common")
                .join("flutter_patched_sdk")
                .join("platform_strong.dill"),
        ]
    }

    /// Enabling a line pins its reviewed regions: the reviewed table and
    /// [`SUPPORTED_FLUTTER_LINES`] name the same lines in the same
    /// (ascending) order, every reviewed patch belongs to its line, and
    /// each line's pins are sorted oldest-patch-first and distinct.
    #[test]
    fn reviewed_lines_pin_exactly_the_supported_lines() {
        let lines: Vec<(u32, u32)> = REVIEWED_LINES.iter().map(|(line, _)| *line).collect();
        assert_eq!(lines, SUPPORTED_FLUTTER_LINES);
        assert!(
            SUPPORTED_FLUTTER_LINES
                .windows(2)
                .all(|pair| pair[0] < pair[1]),
            "supported lines are sorted and distinct"
        );
        for (line, pins) in REVIEWED_LINES {
            assert!(!pins.is_empty(), "{line:?} has no reviewed patch");
            let stamps = pins.iter().map(|(patch, _)| *patch).collect::<Vec<_>>();
            assert!(
                stamps
                    .windows(2)
                    .all(|pair| stamp_order(&pair[0], &pair[1]) != std::cmp::Ordering::Greater),
                "{line:?}: pins are sorted oldest-first: {stamps:?}"
            );
            for (patch, digest) in pins.iter() {
                assert_eq!(version_line(patch), Some(*line), "{patch}");
                assert!(
                    digest.len() == 32 && digest.bytes().all(|b| b.is_ascii_hexdigit()),
                    "{patch}: {digest}"
                );
            }
        }
    }

    /// The enabling backstop: every INSTALLED flutter whose stamp names
    /// a supported line must hash its regions to one of the line's
    /// reviewed digests — a patch of an enabled line whose flutter_tools
    /// drifted (or is simply not yet reviewed) fails here instead of
    /// being mis-modeled (see [`SUPPORTED_FLUTTER_LINES`]). Skips when
    /// no fvm versions root exists (CI machines have none).
    #[test]
    fn supported_line_patches_match_their_reviewed_regions() {
        let Some(root) = fvm_versions_root() else {
            return;
        };
        let drifted: Vec<String> = installed_versions(&root)
            .into_iter()
            .filter_map(|(name, stamp)| {
                let (_, pins) = reviewed_line(&stamp)?;
                let actual = region_digest(&root, &name);
                let matches = actual
                    .as_deref()
                    .is_some_and(|digest| pins.iter().any(|(_, pinned)| *pinned == digest));
                (!matches).then(|| {
                    let reviewed = pins
                        .iter()
                        .map(|(patch, digest)| format!("{patch} ({digest})"))
                        .collect::<Vec<_>>()
                        .join(", ");
                    format!(
                        "{name} ({stamp}): regions {} match no reviewed patch of its line \
                         (reviewed: {reviewed}) — classify its region diff or exclude the patch",
                        actual.as_deref().unwrap_or("missing")
                    )
                })
            })
            .collect();
        assert!(
            drifted.is_empty(),
            "installed patches of supported lines drifted from their reviewed \
             flutter_tools regions:\n{}",
            drifted.join("\n")
        );
    }

    /// The kernel files of every installed patch of an enabled line walk
    /// through [`crate::dill_rewrite`]: a no-match rewrite reproduces
    /// each byte for byte (every table boundary, padding run, and footer
    /// field lands where the official writer put it), relocating its
    /// `org-dartlang-sdk:///` URIs and back rewrites the same strings
    /// both ways, and the relocated dill re-walks byte for byte. Skips
    /// without an fvm versions root.
    #[test]
    fn supported_line_platform_dills_walk_through_the_rewriter() {
        use crate::dill_rewrite::rewrite_file;
        const ABSENT: &[u8] = b"file:///dartq-absent-root/";
        const OTHER: &[u8] = b"file:///dartq-other-root/";
        const SDK: &[u8] = b"org-dartlang-sdk:///";
        const RELOCATED: &[u8] = b"org-dartlang-sdk:///dartq-relocated/";
        let Some(root) = fvm_versions_root() else {
            return;
        };
        let scratch = tempfile::tempdir().expect("scratch dir");
        let out = |name: &str| scratch.path().join(name);
        let bytes = |path: &Path| fs::read(path).expect("read dill");
        for (name, stamp) in installed_versions(&root) {
            if !supported_flutter_version(&stamp) {
                continue;
            }
            for dill in platform_dills(&root, &name) {
                if !dill.is_file() {
                    continue;
                }
                let what = format!("{name} ({stamp}) {}", dill.display());
                let rewrite = |src: &Path, dst: &str, from: &[u8], to: &[u8]| {
                    rewrite_file(src, &out(dst), from, to)
                        .unwrap_or_else(|err| panic!("{what}: {err}"))
                };
                let noop = rewrite(&dill, "noop", ABSENT, OTHER);
                assert_eq!((noop.source_uris, noop.strings), (0, 0), "{what}");
                assert!(
                    bytes(&out("noop")) == bytes(&dill),
                    "{what}: a no-match rewrite is byte-identical"
                );
                let there = rewrite(&dill, "there", SDK, RELOCATED);
                assert!(there.source_uris > 0, "{what}: no SDK URI relocated");
                let back = rewrite(&out("there"), "back", RELOCATED, SDK);
                assert_eq!(
                    (back.source_uris, back.strings),
                    (there.source_uris, there.strings),
                    "{what}: relocation and its inverse rewrite the same strings"
                );
                rewrite(&out("there"), "again", ABSENT, OTHER);
                assert!(
                    bytes(&out("again")) == bytes(&out("there")),
                    "{what}: the relocated dill re-walks byte for byte"
                );
            }
        }
    }

    /// The probe for extending [`SUPPORTED_FLUTTER_LINES`]: prints every
    /// installed version's verdict — `reviewed` (a patch of an enabled
    /// line matching one of the line's reviewed digests, naming the
    /// pin), `DRIFT` (one that matches none), `passthrough` (a line not
    /// enabled: its region diff against the nearest reviewed line is the
    /// work before enabling it) — with its region digest and kernel
    /// format (the dill rewriter's gate). Ignored by default (evidence
    /// gathering, not an assertion):
    /// `cargo test -p dartq-bcache flutter_tools_region_verdicts -- --ignored --nocapture`.
    #[test]
    #[ignore = "probe: run with --ignored --nocapture to survey installed SDKs"]
    fn flutter_tools_region_verdicts() {
        use std::io::Read as _;
        let Some(root) = fvm_versions_root() else {
            panic!("no fvm versions root (set DARTQ_FVM_HOME to relocate it)");
        };
        let installed = installed_versions(&root);
        assert!(
            !installed.is_empty(),
            "no version dirs under {}",
            root.display()
        );
        for (name, stamp) in installed {
            let digest = region_digest(&root, &name);
            let verdict = match (reviewed_line(&stamp), digest.as_deref()) {
                (Some((line, pins)), Some(actual))
                    if pins.iter().any(|(_, pinned)| *pinned == actual) =>
                {
                    let matching = pins
                        .iter()
                        .filter(|(_, pinned)| *pinned == actual)
                        .map(|(patch, _)| *patch)
                        .collect::<Vec<_>>()
                        .join("/");
                    format!("reviewed (matches {matching} of {line:?})")
                }
                (Some((_, pins)), _) => {
                    let reviewed = pins
                        .iter()
                        .map(|(patch, _)| *patch)
                        .collect::<Vec<_>>()
                        .join(", ");
                    format!("DRIFT from reviewed {reviewed}")
                }
                (None, _) => "passthrough".to_owned(),
            };
            let mut head = [0_u8; 8];
            let format = fs::File::open(&platform_dills(&root, &name)[0])
                .and_then(|mut dill| dill.read_exact(&mut head))
                .map_or_else(
                    |_| "unknown".to_owned(),
                    |()| u32::from_be_bytes([head[4], head[5], head[6], head[7]]).to_string(),
                );
            println!(
                "{name} ({stamp}): {verdict}; regions {}; kernel format {format}",
                digest.as_deref().unwrap_or("missing")
            );
        }
    }

    /// A byte-identical fingerprint record is not rewritten: the file's
    /// mtime survives a second store call (a rewrite would bump it past
    /// the sleep), a changed record is.
    #[test]
    fn fingerprint_cache_skips_rewrite_of_identical_record() {
        let (_guard, tmp) = tempdir();
        let sdk = fake_flutter_sdk(&tmp, "3.47.1");
        let store_root = tmp.join("cas");
        let first = key::fingerprint_toolchain(&sdk).expect("fingerprint");

        store_fingerprint_cache(&store_root, &sdk, &first);
        let path = store_root.join("meta").join("toolchain.json");
        let mtime = fs::metadata(&path)
            .expect("stat record")
            .modified()
            .expect("mtime");
        std::thread::sleep(Duration::from_millis(25));
        store_fingerprint_cache(&store_root, &sdk, &first);
        assert_eq!(
            fs::metadata(&path)
                .expect("stat record")
                .modified()
                .expect("mtime"),
            mtime,
            "an identical record must not be rewritten"
        );

        // A changed SDK state rewrites: the artifact's size moves, so
        // the stamps — and the record — change.
        write(
            &sdk.join("bin")
                .join("cache")
                .join("dart-sdk")
                .join("lib")
                .join("_internal")
                .join("vm_platform_strong.dill"),
            b"platform dill bytes, changed",
        );
        let second = key::fingerprint_toolchain(&sdk).expect("fingerprint");
        store_fingerprint_cache(&store_root, &sdk, &second);
        let record: ToolchainCache =
            serde_json::from_slice(&fs::read(&path).expect("record")).expect("parse record");
        assert_eq!(record.fingerprint, second);
        assert_ne!(
            fs::metadata(&path)
                .expect("stat record")
                .modified()
                .expect("mtime"),
            mtime
        );
    }

    #[test]
    fn provenance_change_changes_the_pre_key() {
        let (_guard, tmp) = tempdir();
        let sdk = fake_flutter_sdk(&tmp, "3.47.1");
        let tree = synthetic_project(&tmp, "app");
        let config = config_for(&tmp.join("cas"), &sdk.join("bin").join("flutter"));

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
        let store_root = tmp.join("cas");
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

    /// A red suite still advances the kernel cache: `flutter test` exits
    /// non-zero both on a compile failure and on a compiled-but-failed
    /// suite, and the scan's touch gate tells them apart (a touched dill
    /// means the compile succeeded). The ingest is the kernel cache
    /// alone — a hash dir the run also touched never rides along — and
    /// the child's exit code propagates unchanged.
    #[test]
    fn red_suite_ingests_kernel_cache_only_and_propagates_exit() {
        let (_guard, tmp) = tempdir();
        let sdk = fake_flutter_sdk(&tmp, "3.47.1");
        let store_root = tmp.join("cas");
        let config = config_for(&store_root, &sdk.join("bin").join("flutter"));

        // The red suite: the harness compiled the closure (the dill is
        // fresh) and left a hash dir behind as well.
        let tree = synthetic_project(&tmp, "a");
        let mut runner = FakeRunner {
            code: 1,
            argv: Vec::new(),
            effect: Some(Box::new(|cwd| {
                write_test_cache(cwd);
                build_fixture(cwd, DIR_MAIN);
            })),
        };
        let report = run_shim(&raw_argv(&["test"]), &tree, &config, &mut runner);
        assert_eq!(report.exit_code, Some(1), "the child's exit code wins");
        assert_eq!(report.action, Action::Ingested);
        assert_eq!(report.ingest_files, 1);
        let key = report.key.clone().expect("post key");

        let store = CasStore::open(&store_root).expect("open store");
        let manifest = store.lookup(&key).expect("lookup").expect("entry");
        assert_eq!(manifest.dirs.len(), 1, "the kernel cache alone");
        assert_eq!(manifest.dirs[0].name_rule, NameRule::TestKernelCache);
        assert_eq!(manifest.dirs[0].name_observed, "build/test_cache");
        assert!(
            !manifest
                .dirs
                .iter()
                .any(|dir| dir.name_observed == DIR_MAIN),
            "hash dirs stay exit-0-gated"
        );
        assert_eq!(
            manifest.dirs[0].files[0].blob,
            BlobId(*blake3::hash(TEST_DILL_BYTES).as_bytes())
        );

        // The red-suite cache serves exactly the tree that iterates on
        // tests: the next run PRE-hits and materializes the dill before
        // the child runs.
        fs::remove_dir_all(tree.join("build").join("test_cache")).expect("wipe cache dir");
        let mut rerun = FakeRunner {
            code: 0,
            argv: Vec::new(),
            effect: Some(Box::new(|cwd| {
                assert_eq!(
                    fs::read(test_dill_path(cwd)).unwrap_or_default(),
                    TEST_DILL_BYTES,
                    "the red-suite dill materializes before the child runs"
                );
            })),
        };
        let report = run_shim(&raw_argv(&["test"]), &tree, &config, &mut rerun);
        assert_eq!(report.action, Action::PreHitMaterialized);
        assert_eq!(store.stats().expect("stats").entries, 1);
    }

    /// A compile failure ingests nothing: frontend_server writes the
    /// cache dill only after a successful compile, so a failed compile
    /// leaves the cache untouched — and the stale dill of an earlier
    /// run must not ride along on its old mtime.
    #[test]
    fn compile_failed_test_run_ingests_nothing() {
        let (_guard, tmp) = tempdir();
        let sdk = fake_flutter_sdk(&tmp, "3.47.1");
        let store_root = tmp.join("cas");
        let config = config_for(&store_root, &sdk.join("bin").join("flutter"));

        // The dill an earlier run left, parked before this run's start.
        let tree = synthetic_project(&tmp, "a");
        let stale = write_test_cache(&tree);
        let cache = tree.join("build").join("test_cache");
        let past = UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        for path in [stale, cache.join("build"), cache] {
            std::fs::File::open(&path)
                .and_then(|file| file.set_modified(past))
                .expect("park mtime");
        }

        let mut runner = FakeRunner::exits(1);
        let report = run_shim(&raw_argv(&["test"]), &tree, &config, &mut runner);
        assert_eq!(report.exit_code, Some(1));
        assert_eq!(report.action, Action::PreMiss);
        assert_eq!(report.ingest_files, 0);
        let stats = CasStore::open(&store_root)
            .expect("open store")
            .stats()
            .expect("stats");
        assert_eq!(stats.entries, 0, "a failed compile never stores an entry");
        assert_eq!(stats.blobs, 0);
    }

    /// A signal death takes the same test-face path: the runner maps it
    /// to 128 + signal (the shell convention), the kernel cache a
    /// killed run already compiled is still fresh, and the worst a kill
    /// can do mid-copy — dart:io File.copy writes the destination in
    /// place — is a torn dill whose replay cost is one wasted
    /// materialization (frontend_server falls back to a full compile
    /// when the initialize-from-dill file will not read).
    #[test]
    fn signal_death_test_run_still_ingests_kernel_cache() {
        let (_guard, tmp) = tempdir();
        let sdk = fake_flutter_sdk(&tmp, "3.47.1");
        let store_root = tmp.join("cas");
        let config = config_for(&store_root, &sdk.join("bin").join("flutter"));

        let tree = synthetic_project(&tmp, "a");
        let mut runner = FakeRunner {
            code: 128 + 9,
            argv: Vec::new(),
            effect: Some(Box::new(|cwd| {
                write_test_cache(cwd);
            })),
        };
        let report = run_shim(&raw_argv(&["test"]), &tree, &config, &mut runner);
        assert_eq!(report.exit_code, Some(137), "128 + SIGKILL propagates");
        assert_eq!(report.action, Action::Ingested);
        assert_eq!(report.ingest_files, 1);
        let key = report.key.expect("post key");
        let manifest = CasStore::open(&store_root)
            .expect("open store")
            .lookup(&key)
            .expect("lookup")
            .expect("entry");
        assert_eq!(manifest.dirs.len(), 1);
        assert_eq!(manifest.dirs[0].name_rule, NameRule::TestKernelCache);
    }

    /// The runner's exit-code mapping that funnels signal deaths into
    /// the non-zero path: the shell convention 128 + signal.
    #[cfg(unix)]
    #[test]
    fn signal_death_maps_to_shell_convention() {
        use std::os::unix::process::ExitStatusExt;
        assert_eq!(exit_code(ExitStatus::from_raw(0)), 0);
        assert_eq!(exit_code(ExitStatus::from_raw(0x0100)), 1);
        assert_eq!(exit_code(ExitStatus::from_raw(9)), 137);
    }

    /// The live test kernel-cache layout flutter_tools writes: the double
    /// `build` nesting of `join(buildDir, 'test_cache', <cached kernel
    /// path>)>`.
    const TEST_DILL_REL: &str = "build/ab.cache.dill.track.dill";
    const TEST_DILL_BYTES: &[u8] = b"test kernel bytes";

    fn write_test_cache(cwd: &Path) -> PathBuf {
        let dill = cwd.join("build").join("test_cache").join(TEST_DILL_REL);
        write(&dill, TEST_DILL_BYTES);
        dill
    }

    fn write_test_cache_bytes(cwd: &Path, bytes: &[u8]) -> PathBuf {
        let dill = cwd.join("build").join("test_cache").join(TEST_DILL_REL);
        write(&dill, bytes);
        dill
    }

    fn test_dill_path(cwd: &Path) -> PathBuf {
        cwd.join("build").join("test_cache").join(TEST_DILL_REL)
    }

    /// Cross-tree test-face replay: tree A seeds a kernel-shaped dill
    /// embedding its own canonical URIs; tree B's PRE hit rewrites the
    /// `file://` URIs to tree B before the child runs, and POST
    /// converges the entry onto tree B's re-rewritten dill.
    #[test]
    fn test_kernel_cache_cross_tree_hit_rewrites_uris_and_converges() {
        use crate::dill_rewrite::synthetic_component;

        let (_guard, tmp) = tempdir();
        let sdk = fake_flutter_sdk(&tmp, "3.47.1");
        let store_root = tmp.join("cas");
        let config = config_for(&store_root, &sdk.join("bin").join("flutter"));

        // Tree A: the "compile" leaves a kernel dill embedding tree A's
        // canonical URIs; POST ingests it as the entry's test dir.
        let tree_a = synthetic_project(&tmp, "a");
        let root_a = tree_a.to_str().expect("UTF-8 root").to_string();
        let dill_a = synthetic_component(
            &[(
                format!("file://{root_a}/test/cache_smoke_test.dart").as_str(),
                "package:app/main.dart",
            )],
            &[
                format!("file://{root_a}/lib/main.dart").as_str(),
                "dart:core",
            ],
            1,
        );
        let effect_a = dill_a.clone();
        let mut runner = FakeRunner {
            code: 0,
            argv: Vec::new(),
            effect: Some(Box::new(move |cwd| {
                write_test_cache_bytes(cwd, &effect_a);
            })),
        };
        let raw = raw_argv(&["test"]);
        let seeded = run_shim(&raw, &tree_a, &config, &mut runner);
        assert_eq!(seeded.action, Action::Ingested);
        assert_eq!(seeded.ingest_files, 1);
        let key = seeded.key.clone().expect("post key");
        let store = CasStore::open(&store_root).expect("open store");
        let manifest = store.lookup(&key).expect("lookup").expect("entry");
        let dir = manifest
            .dirs
            .iter()
            .find(|d| d.name_rule == NameRule::TestKernelCache)
            .expect("test cache dir");
        assert_eq!(dir.files[0].rewrite, crate::types::RewriteKind::DillUris);

        // Tree B (a different canonical root, no build dir): PRE hits
        // and materializes the dill with tree B's URIs BEFORE the child
        // runs; the child ("frontend_server") rewrites it with its own
        // tree-B dill; POST re-ingests, converging the entry.
        let tree_b = synthetic_project(&tmp, "b");
        let root_b = tree_b.to_str().expect("UTF-8 root").to_string();
        let dill_b = synthetic_component(
            &[(
                format!("file://{root_b}/test/cache_smoke_test.dart").as_str(),
                "package:app/main.dart",
            )],
            &[
                format!("file://{root_b}/lib/main.dart").as_str(),
                "dart:core",
            ],
            1,
        );
        let effect_b = dill_b.clone();
        let mut runner = FakeRunner {
            code: 0,
            argv: Vec::new(),
            effect: Some(Box::new(move |cwd| {
                // What the child initializes from: the hit's placement.
                let placed = fs::read(test_dill_path(cwd)).expect("hit placed the dill");
                assert!(
                    memchr::memmem::find(
                        &placed,
                        format!("file://{root_b}/lib/main.dart").as_bytes()
                    )
                    .is_some(),
                    "the placed dill carries this tree's URIs"
                );
                assert!(
                    memchr::memmem::find(
                        &placed,
                        format!("file://{root_a}/lib/main.dart").as_bytes()
                    )
                    .is_none(),
                    "no ingest-tree URI survives the rewrite"
                );
                assert_ne!(placed, dill_a, "the placement is a rewrite, not a clone");
                // frontend_server rewrites the dill with current-tree
                // URIs on every run.
                write_test_cache_bytes(cwd, &effect_b);
            })),
        };
        let report = run_shim(&raw, &tree_b, &config, &mut runner);
        assert_eq!(report.action, Action::PreHitMaterialized);
        assert_eq!(report.reason, "pre_hit");
        assert_eq!(report.exit_code, Some(0));
        assert_eq!(report.key.as_deref(), Some(key.as_str()));
        assert_eq!(
            report
                .materialize
                .as_ref()
                .expect("materialize report")
                .dill_rewrites,
            1
        );
        assert_eq!(
            fs::read(test_dill_path(&tree_b)).expect("final dill"),
            dill_b,
            "the run leaves its own dill behind"
        );

        // POST converged the entry onto the consuming tree's dill.
        let manifest = store.lookup(&key).expect("lookup").expect("entry");
        let dir = manifest
            .dirs
            .iter()
            .find(|d| d.name_rule == NameRule::TestKernelCache)
            .expect("test cache dir");
        assert_eq!(
            dir.files[0].blob,
            BlobId(*blake3::hash(&dill_b).as_bytes()),
            "entry converges onto tree B's dill"
        );
    }

    /// The test face end to end: a cold `flutter test` ingests the
    /// kernel-cache dir as the entry's fixed-path dir; after the dir is
    /// wiped, the next run PRE-hits and materializes the dill back
    /// byte-identical BEFORE the child runs.
    #[test]
    fn test_kernel_cache_cold_ingest_and_wiped_hit_replay() {
        let (_guard, tmp) = tempdir();
        let sdk = fake_flutter_sdk(&tmp, "3.47.1");
        let store_root = tmp.join("cas");
        let config = config_for(&store_root, &sdk.join("bin").join("flutter"));

        // Cold run: the child compiles, leaving the kernel cache behind;
        // POST ingests it as the entry's one dir.
        let tree = synthetic_project(&tmp, "a");
        let mut runner = FakeRunner {
            code: 0,
            argv: Vec::new(),
            effect: Some(Box::new(|cwd| {
                write_test_cache(cwd);
            })),
        };
        let raw = raw_argv(&["test"]);
        let report = run_shim(&raw, &tree, &config, &mut runner);
        assert_eq!(report.action, Action::Ingested);
        assert_eq!(report.exit_code, Some(0));
        assert_eq!(report.ingest_files, 1);
        let key = report.key.clone().expect("post key");

        let store = CasStore::open(&store_root).expect("open store");
        let manifest = store.lookup(&key).expect("lookup").expect("entry");
        assert_eq!(manifest.dirs.len(), 1);
        let dir = &manifest.dirs[0];
        assert_eq!(dir.name_observed, "build/test_cache");
        assert_eq!(dir.name_rule, NameRule::TestKernelCache);
        assert!(dir.name_defines.is_empty());
        assert!(dir.name_output_dir.is_empty());
        assert_eq!(dir.files.len(), 1);
        assert_eq!(dir.files[0].path, Path::new(TEST_DILL_REL));

        // The wiped tree: `rm -rf build`. The next run PRE-hits and
        // materializes the dill before the child starts — the effect
        // asserts the bytes the child would initialize from.
        fs::remove_dir_all(tree.join("build")).expect("wipe build dir");
        let mut runner = FakeRunner {
            code: 0,
            argv: Vec::new(),
            effect: Some(Box::new(|cwd| {
                let dill = cwd.join("build").join("test_cache").join(TEST_DILL_REL);
                assert_eq!(
                    fs::read(&dill).unwrap_or_default(),
                    TEST_DILL_BYTES,
                    "the hit must materialize the dill before the child runs"
                );
            })),
        };
        let report = run_shim(&raw, &tree, &config, &mut runner);
        assert_eq!(report.action, Action::PreHitMaterialized);
        assert_eq!(report.reason, "pre_hit");
        assert_eq!(report.exit_code, Some(0));
        assert_eq!(report.key.as_deref(), Some(key.as_str()));
        assert_eq!(report.skipped_dirs, 0);
        // Whether POST re-stores the just-materialized dill is timing
        // (whole-second mtimes); either way the entry count holds.
        assert_eq!(
            fs::read(tree.join("build").join("test_cache").join(TEST_DILL_REL))
                .expect("materialized dill"),
            TEST_DILL_BYTES
        );
        assert_eq!(store.stats().expect("stats").entries, 1);
    }

    /// The warm-run fast path end to end: when the dill on disk is the
    /// literal same file the stored entry recorded — the dir touched
    /// through another file, and PRE not a hit, so nothing re-placed
    /// it — POST reuses the recorded blob: no re-hash (the run stores
    /// zero files), no new blob version, the reference unchanged.
    #[test]
    fn post_reuses_recorded_dill_when_file_unchanged() {
        let (_guard, tmp) = tempdir();
        let sdk = fake_flutter_sdk(&tmp, "3.47.1");
        let store_root = tmp.join("cas");
        let config = config_for(&store_root, &sdk.join("bin").join("flutter"));
        let tree = synthetic_project(&tmp, "a");
        // The provenance state the entry is keyed on.
        write(&tree.join("pubspec.yaml"), b"name: app\nversion: 2.0.0\n");

        // Run 1 compiles the dill; the entry lands under this state's key.
        let raw = raw_argv(&["test"]);
        let mut runner = FakeRunner {
            code: 0,
            argv: Vec::new(),
            effect: Some(Box::new(|cwd| {
                write_test_cache(cwd);
            })),
        };
        let seeded = run_shim(&raw, &tree, &config, &mut runner);
        assert_eq!(seeded.action, Action::Ingested);
        assert_eq!(seeded.ingest_files, 1);
        let key = seeded.key.clone().expect("post key");
        let store = CasStore::open(&store_root).expect("open store");
        let dill_blob = BlobId(*blake3::hash(TEST_DILL_BYTES).as_bytes());
        let recorded = || {
            store
                .lookup(&key)
                .expect("lookup")
                .expect("entry")
                .dirs
                .into_iter()
                .find(|dir| dir.name_rule == NameRule::TestKernelCache)
                .expect("test cache dir")
        };
        assert_eq!(recorded().files[0].blob, dill_blob);
        assert!(
            recorded().files[0].stamp.is_some(),
            "the ingest records the stamp"
        );
        assert_eq!(store.stats().expect("stats").blobs, 1);

        // A foreign pre-run state moves the key (so PRE misses and the
        // dill is never re-placed); the run restores the recorded state
        // and touches the dir only through a non-dill file.
        write(&tree.join("pubspec.yaml"), b"name: app\nversion: 3.0.0\n");
        let mut runner = FakeRunner {
            code: 0,
            argv: Vec::new(),
            effect: Some(Box::new(|cwd| {
                write(&cwd.join("pubspec.yaml"), b"name: app\nversion: 2.0.0\n");
                write(
                    &cwd.join("build")
                        .join("test_cache")
                        .join("build")
                        .join("lock"),
                    b"transient",
                );
            })),
        };
        let report = run_shim(&raw, &tree, &config, &mut runner);
        assert_eq!(report.action, Action::Ingested, "pre-miss, post re-ingest");
        assert_eq!(
            report.ingest_files, 0,
            "the unchanged dill is not re-hashed"
        );
        let stats = store.stats().expect("stats");
        assert_eq!(stats.entries, 1);
        assert_eq!(stats.blobs, 1, "no new blob version");
        assert_eq!(recorded().files[0].blob, dill_blob, "the reference is kept");
        assert!(recorded().files[0].stamp.is_some());
    }

    /// The flagship warm apk shape: a same-tree hit whose rerun only
    /// validates (an assemble child observed, the dir touched, no
    /// output rewritten) stores ZERO new files — every placed file is
    /// re-referenced through its placed identity, and the references
    /// and blob set are exactly the seeded entry's.
    #[cfg(unix)]
    #[test]
    fn warm_hit_run_stores_nothing_when_flutter_only_validates() {
        let (_guard, tmp) = tempdir();
        let sdk = fake_flutter_sdk(&tmp, "3.47.1");
        let flutter_bin = sdk.join("bin").join("flutter");
        let engine_revision = key::fingerprint_toolchain(&sdk)
            .expect("fingerprint")
            .engine_revision;
        let store_root = tmp.join("cas");
        let config = config_for(&store_root, &flutter_bin);

        // Run 1 seeds the entry: the "build" runs the assemble lookalike
        // and leaves the observed hash dir.
        let tree = synthetic_project(&tmp, "a");
        let output = tree.join("build/app/intermediates/flutter/release");
        let argv = assemble_argv(&flutter_bin, &output.to_string_lossy());
        let template = base_capture(&argv, &tree, &engine_revision);
        let seed_argv = argv.clone();
        let seed_name = template.name.clone();
        let mut runner = FakeRunner {
            code: 0,
            argv: Vec::new(),
            effect: Some(Box::new(move |cwd| {
                run_assemble_lookalike(&seed_argv);
                build_fixture(cwd, &seed_name);
            })),
        };
        let seeded = run_shim(&raw_argv(&APK_ARGV), &tree, &config, &mut runner);
        assert_eq!(seeded.action, Action::Ingested);
        assert!(
            seeded.ingest_files >= 3,
            "the cold run stores the whole dir"
        );
        let key = seeded.key.clone().expect("post key");
        let store = CasStore::open(&store_root).expect("open store");
        let seed_manifest = store.lookup(&key).expect("lookup").expect("entry");
        let seed_blobs = store.stats().expect("stats").blobs;

        // The warm rerun: PRE hits (same tree, same provenance) and
        // re-places the dir — fresh inodes, so only the placed
        // identities can vouch for the files. A real assemble child is
        // observed exactly as gradle spawns one every run — without an
        // observation the reproduced key would spare the re-ingest. The
        // "validation" touches the dir without rewriting an output: a
        // transient file appears and is gone again.
        let warm_argv = argv.clone();
        let warm_name = template.name.clone();
        let mut runner = FakeRunner {
            code: 0,
            argv: Vec::new(),
            effect: Some(Box::new(move |cwd| {
                run_assemble_lookalike(&warm_argv);
                let transient = cwd
                    .join(".dart_tool")
                    .join("flutter_build")
                    .join(&warm_name)
                    .join(".transient");
                write(&transient, b"x");
                fs::remove_file(&transient).expect("remove transient");
            })),
        };
        let report = run_shim(&raw_argv(&APK_ARGV), &tree, &config, &mut runner);
        assert_eq!(report.action, Action::PreHitMaterialized);
        assert_eq!(report.reason, "pre_hit");
        assert_eq!(report.exit_code, Some(0));
        assert_eq!(report.ingest_files, 0, "validation alone re-hashes nothing");

        // Every reference and the blob set are exactly the seeded ones.
        let manifest = store.lookup(&key).expect("lookup").expect("entry");
        assert_eq!(manifest.dirs.len(), 1);
        let refs = |manifest: &EntryManifest| {
            manifest.dirs[0]
                .files
                .iter()
                .map(|file| (file.path.clone(), file.blob))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            refs(&manifest),
            refs(&seed_manifest),
            "references reused verbatim"
        );
        assert_eq!(
            store.stats().expect("stats").blobs,
            seed_blobs,
            "no new blob versions"
        );
    }

    /// The warm apk shape with one rewritten payload: only that file is
    /// re-hashed and stored, the bookkeeping references stay put — and
    /// the run's own upkeep reclaims the superseded payload blob while
    /// the entry survives.
    #[cfg(unix)]
    #[test]
    fn warm_hit_run_rehashes_only_the_file_flutter_rewrote() {
        let (_guard, tmp) = tempdir();
        let sdk = fake_flutter_sdk(&tmp, "3.47.1");
        let flutter_bin = sdk.join("bin").join("flutter");
        let engine_revision = key::fingerprint_toolchain(&sdk)
            .expect("fingerprint")
            .engine_revision;
        let store_root = tmp.join("cas");
        let config = config_for(&store_root, &flutter_bin);

        let tree = synthetic_project(&tmp, "a");
        let output = tree.join("build/app/intermediates/flutter/release");
        let argv = assemble_argv(&flutter_bin, &output.to_string_lossy());
        let template = base_capture(&argv, &tree, &engine_revision);
        let seed_argv = argv.clone();
        let seed_name = template.name.clone();
        let mut runner = FakeRunner {
            code: 0,
            argv: Vec::new(),
            effect: Some(Box::new(move |cwd| {
                run_assemble_lookalike(&seed_argv);
                build_fixture(cwd, &seed_name);
            })),
        };
        let seeded = run_shim(&raw_argv(&APK_ARGV), &tree, &config, &mut runner);
        assert_eq!(seeded.action, Action::Ingested);
        let key = seeded.key.clone().expect("post key");
        let store = CasStore::open(&store_root).expect("open store");
        let seed_manifest = store.lookup(&key).expect("lookup").expect("entry");

        // The warm rerun rewrites ONLY the payload dill (the stale-hit
        // shape: a source the coarse key cannot see changed, gradle
        // rebuilt the kernel; sizes differ, so the identity cannot
        // collide).
        let warm_argv = argv.clone();
        let warm_name = template.name.clone();
        let mut runner = FakeRunner {
            code: 0,
            argv: Vec::new(),
            effect: Some(Box::new(move |cwd| {
                run_assemble_lookalike(&warm_argv);
                write(
                    &cwd.join(".dart_tool")
                        .join("flutter_build")
                        .join(&warm_name)
                        .join("app.dill"),
                    b"kernel payload bytes v2",
                );
            })),
        };
        let report = run_shim(&raw_argv(&APK_ARGV), &tree, &config, &mut runner);
        assert_eq!(report.action, Action::PreHitMaterialized);
        assert_eq!(report.ingest_files, 1, "only the rewritten dill is stored");

        let manifest = store.lookup(&key).expect("lookup").expect("entry");
        let by_path = |manifest: &EntryManifest, path: &str| {
            manifest.dirs[0]
                .files
                .iter()
                .find(|file| file.path == Path::new(path))
                .unwrap_or_else(|| panic!("no {path} in the manifest"))
                .blob
        };
        assert_eq!(
            by_path(&manifest, "app.dill"),
            BlobId(*blake3::hash(b"kernel payload bytes v2").as_bytes()),
            "the rewritten payload got a fresh blob"
        );
        assert_eq!(
            by_path(&manifest, "kernel_snapshot.d"),
            by_path(&seed_manifest, "kernel_snapshot.d"),
            "the untouched depfile reference is kept"
        );
        assert_eq!(
            by_path(&manifest, "flutter_build.dill.stamp"),
            by_path(&seed_manifest, "flutter_build.dill.stamp"),
            "the untouched stamp reference is kept"
        );

        // The run's own upkeep reclaimed the superseded payload — no hour
        // in gc's in-flight window — and nothing else: a later full pass
        // finds no garbage, and the entry and its blobs survive.
        let superseded = BlobId(*blake3::hash(b"kernel payload bytes").as_bytes());
        assert!(
            !store.blob_path(&superseded).exists(),
            "the detached payload is reclaimed by the run itself"
        );
        assert_eq!(
            store.used_bytes().expect("used"),
            store.stats().expect("stats").blob_bytes,
            "the reclaim books the counter down"
        );
        let report = store.gc(u64::MAX, 0, unix_now() + 2 * 3600).expect("gc");
        assert_eq!(report.entries_evicted, 0, "the entry survives");
        assert_eq!(report.blobs_removed, 0, "nothing else was garbage");
        assert!(store.lookup(&key).expect("lookup").is_some());
    }

    /// A miss run (the key moved between PRE and POST — provenance
    /// edited before the run, restored during it) re-ingests under the
    /// prior entry's key, and the prior stamps cover everything the run
    /// did not rewrite: only the touched file stores, the payload and
    /// depfile references are kept. The build-face twin of the test
    /// face's warm fast path.
    #[test]
    fn miss_run_reuses_prior_entry_stamps_for_build_dirs() {
        let (_guard, tmp) = tempdir();
        let sdk = fake_flutter_sdk(&tmp, "3.47.1");
        let store_root = tmp.join("cas");
        let config = config_for(&store_root, &sdk.join("bin").join("flutter"));

        // The provenance state the entry is keyed on; run 1 seeds it.
        let tree = synthetic_project(&tmp, "a");
        write(&tree.join("pubspec.yaml"), b"name: app\nversion: 2.0.0\n");
        let mut runner = FakeRunner {
            code: 0,
            argv: Vec::new(),
            effect: Some(Box::new(|cwd| {
                build_fixture(cwd, DIR_MAIN);
            })),
        };
        let seeded = run_shim(&raw_argv(&BUNDLE_ARGV), &tree, &config, &mut runner);
        assert_eq!(seeded.action, Action::Ingested);
        let key = seeded.key.clone().expect("post key");
        let store = CasStore::open(&store_root).expect("open store");
        let seed_manifest = store.lookup(&key).expect("lookup").expect("entry");
        assert!(
            seed_manifest.dirs[0]
                .files
                .iter()
                .all(|file| file.stamp.is_some()),
            "build-face ingests record stamps"
        );
        let seed_blobs = store.stats().expect("stats").blobs;

        // A foreign pre-run state moves the key (PRE misses, nothing is
        // re-placed); the run restores the recorded state and rewrites
        // only one bookkeeping file — flutter's content-validation shape.
        write(&tree.join("pubspec.yaml"), b"name: app\nversion: 3.0.0\n");
        let mut runner = FakeRunner {
            code: 0,
            argv: Vec::new(),
            effect: Some(Box::new(|cwd| {
                write(&cwd.join("pubspec.yaml"), b"name: app\nversion: 2.0.0\n");
                write(
                    &cwd.join(".dart_tool")
                        .join("flutter_build")
                        .join(DIR_MAIN)
                        .join("flutter_build.dill.stamp"),
                    br#"{"inputs":[],"outputs":[]}"#,
                );
            })),
        };
        let report = run_shim(&raw_argv(&BUNDLE_ARGV), &tree, &config, &mut runner);
        assert_eq!(report.action, Action::Ingested, "pre-miss, post re-ingest");
        assert_eq!(report.ingest_files, 1, "only the rewritten stamp stores");

        // The other files ride along on their prior stamps.
        let manifest = store.lookup(&key).expect("lookup").expect("entry");
        let by_path = |manifest: &EntryManifest, path: &str| {
            manifest.dirs[0]
                .files
                .iter()
                .find(|file| file.path == Path::new(path))
                .unwrap_or_else(|| panic!("no {path} in the manifest"))
                .blob
        };
        assert_eq!(
            by_path(&manifest, "app.dill"),
            by_path(&seed_manifest, "app.dill"),
            "the payload is reused without reading"
        );
        assert_eq!(
            by_path(&manifest, "kernel_snapshot.d"),
            by_path(&seed_manifest, "kernel_snapshot.d")
        );
        assert_ne!(
            by_path(&manifest, "flutter_build.dill.stamp"),
            by_path(&seed_manifest, "flutter_build.dill.stamp"),
            "the rewritten file got a fresh blob"
        );
        assert_eq!(
            store.stats().expect("stats").blobs,
            seed_blobs,
            "the rewritten stamp's fresh blob replaced the superseded one, which the run reclaimed"
        );
    }

    /// A payload past [`STALE_STAMP_LIMIT`]: re-hashed as a source it
    /// forces a publish, so `ingested 0` with it in the dir proves the
    /// post-run scan never read it — the tree record vouched it by
    /// stamp.
    fn large_payload() -> Vec<u8> {
        vec![0x90u8; STALE_STAMP_LIMIT as usize + 1]
    }

    /// The `build bundle` fixture dir with a large payload (see
    /// [`large_payload`]).
    fn build_fixture_large(project: &Path, name: &str) -> PathBuf {
        let dir = build_fixture(project, name);
        write(&dir.join("app.dill"), &large_payload());
        dir
    }

    /// Flutter's every-build bookkeeping rewrite, as a run effect: the
    /// file is written back byte for byte and its mtime moved a second
    /// on, so the identity the placement recorded no longer matches and
    /// only the bytes can vouch for it.
    fn rewrite_byte_identical(path: &Path) {
        let bytes = fs::read(path).expect("read bookkeeping");
        fs::write(path, bytes).expect("rewrite bookkeeping");
        let file = fs::File::options()
            .write(true)
            .open(path)
            .expect("open for mtime");
        file.set_modified(SystemTime::now() + Duration::from_secs(1))
            .expect("move mtime");
    }

    /// Seeds the bundle entry from `tree` with the large fixture:
    /// the cold run, its key, and the store.
    fn seed_large_bundle(tree: &Path, config: &ShimConfig) -> (String, CasStore) {
        let mut runner = FakeRunner {
            code: 0,
            argv: Vec::new(),
            effect: Some(Box::new(|cwd| {
                build_fixture_large(cwd, DIR_MAIN);
            })),
        };
        let seeded = run_shim(&raw_argv(&BUNDLE_ARGV), tree, config, &mut runner);
        assert_eq!(seeded.action, Action::Ingested);
        assert_eq!(seeded.ingest_files, 3, "the cold run stores the whole dir");
        let key = seeded.key.expect("post key");
        let store = CasStore::open(&config.store_root).expect("open store");
        (key, store)
    }

    /// A warm run of the bundle face in `tree` whose only effect is
    /// flutter's byte-identical rewrite of the stamp file.
    fn validate_only_run(tree: &Path, config: &ShimConfig) -> ShimReport {
        let mut runner = FakeRunner {
            code: 0,
            argv: Vec::new(),
            effect: Some(Box::new(|cwd| {
                rewrite_byte_identical(
                    &cwd.join(".dart_tool")
                        .join("flutter_build")
                        .join(DIR_MAIN)
                        .join("flutter_build.dill.stamp"),
                );
            })),
        };
        run_shim(&raw_argv(&BUNDLE_ARGV), tree, config, &mut runner)
    }

    /// The F5 shape, closed: a wiped tree's hit re-places every file
    /// under fresh identities and publishes nothing (the entry is
    /// reproduced); the next run in that tree — which the manifest's
    /// stamps, naming the cold run's files, cannot vouch for — reads
    /// nothing and publishes nothing either, because the tree's
    /// placement record vouches the re-placed files by stamp and the
    /// rewritten bookkeeping by its bytes. The large payload makes a
    /// read observable: re-hashed, it would have forced a publish.
    #[test]
    fn wiped_tree_hit_then_same_tree_run_publishes_and_reads_nothing() {
        let (_guard, tmp) = tempdir();
        let sdk = fake_flutter_sdk(&tmp, "3.47.1");
        let store_root = tmp.join("cas");
        let config = config_for(&store_root, &sdk.join("bin").join("flutter"));
        let tree = synthetic_project(&tmp, "a");
        let (key, store) = seed_large_bundle(&tree, &config);
        let seed_manifest = store.lookup(&key).expect("lookup").expect("entry");
        let seed_blobs = store.stats().expect("stats").blobs;

        // The wipe-and-hit: everything is re-placed under new inodes.
        fs::remove_dir_all(tree.join(".dart_tool").join("flutter_build")).expect("wipe");
        let report = validate_only_run(&tree, &config);
        assert_eq!(report.action, Action::PreHitMaterialized);
        assert_eq!(report.reason, "pre_hit");
        assert_eq!(report.ingest_files, 0, "the hit reproduced the entry");
        let record = placement::load(&tree, &key).expect("the placement wrote the tree's record");
        assert_eq!(record.dirs.len(), 1);
        assert_eq!(record.dirs[0].name, DIR_MAIN);
        assert_eq!(record.dirs[0].files.len(), 3);
        let dill_stamp = stamp_of(
            &fs::metadata(
                tree.join(".dart_tool")
                    .join("flutter_build")
                    .join(DIR_MAIN)
                    .join("app.dill"),
            )
            .expect("stat placed dill"),
        );
        assert!(
            record.dirs[0]
                .files
                .iter()
                .any(|file| file.path == Path::new("app.dill") && file.stamp == dill_stamp),
            "the record names the re-placed identity"
        );

        // The same-tree run after it: nothing placed, nothing read.
        let report = validate_only_run(&tree, &config);
        assert_eq!(report.action, Action::PreHitMaterialized);
        assert_eq!(
            report.ingest_files, 0,
            "the record vouched every file: no re-hash, no republish"
        );
        let manifest = store.lookup(&key).expect("lookup").expect("entry");
        assert_eq!(
            manifest.dirs, seed_manifest.dirs,
            "the manifest still names the cold run's identities — it was never republished"
        );
        assert_eq!(store.stats().expect("stats").blobs, seed_blobs);
        let record = placement::load(&tree, &key).expect("record");
        assert!(
            record.dirs[0].files.iter().all(|file| {
                stamp_of(
                    &fs::metadata(
                        tree.join(".dart_tool")
                            .join("flutter_build")
                            .join(DIR_MAIN)
                            .join(&file.path),
                    )
                    .expect("stat"),
                ) == file.stamp
            }),
            "the record carries the tree's current identities, the rewritten stamp's included"
        );
    }

    /// The #4077 shape, closed: a cross-tree hit's post-run scan vouches
    /// the re-rooted bookkeeping as this tree's rendering of the
    /// entry's blobs — by stamp where flutter left it alone, by bytes
    /// where flutter rewrote it byte for byte — and publishes nothing,
    /// so the entry keeps the tree it was ingested from; and the ingest
    /// tree's next run vouches by its own record, so two trees sharing
    /// the entry never republish it between them.
    #[test]
    fn cross_tree_hit_publishes_nothing_and_trees_do_not_ping_pong() {
        let (_guard, tmp) = tempdir();
        let sdk = fake_flutter_sdk(&tmp, "3.47.1");
        let store_root = tmp.join("cas");
        let config = config_for(&store_root, &sdk.join("bin").join("flutter"));
        let tree_a = synthetic_project(&tmp, "a");
        let (key, store) = seed_large_bundle(&tree_a, &config);
        let seed_manifest = store.lookup(&key).expect("lookup").expect("entry");
        let seed_blobs = store.stats().expect("stats").blobs;

        // Tree B hits: the stamp and depfile land re-rooted, the
        // payload verbatim; flutter then rewrites the stamp as placed.
        let tree_b = synthetic_project(&tmp, "b");
        let report = validate_only_run(&tree_b, &config);
        assert_eq!(report.action, Action::PreHitMaterialized);
        assert_eq!(
            report.ingest_files, 0,
            "re-rooted files are vouched, not re-hashed and republished"
        );
        let dir_b = tree_b
            .join(".dart_tool")
            .join("flutter_build")
            .join(DIR_MAIN);
        let stamp = fs::read_to_string(dir_b.join("flutter_build.dill.stamp")).expect("stamp");
        assert!(stamp.contains(tree_b.to_str().expect("UTF-8")));
        let manifest = store.lookup(&key).expect("lookup").expect("entry");
        assert_eq!(
            manifest.worktree_root_at_ingest,
            tree_a.to_string_lossy(),
            "the entry keeps its ingest tree"
        );
        assert_eq!(
            manifest.dirs, seed_manifest.dirs,
            "and its blobs and stamps"
        );
        assert_eq!(store.stats().expect("stats").blobs, seed_blobs);
        let record_b = placement::load(&tree_b, &key).expect("tree B's record");
        let recorded = |path: &str| {
            record_b.dirs[0]
                .files
                .iter()
                .find(|file| file.path == Path::new(path))
                .unwrap_or_else(|| panic!("no {path} in the record"))
                .clone()
        };
        assert!(
            !recorded("flutter_build.dill.stamp").verbatim(),
            "the stamp is recorded as a re-rooted rendering"
        );
        assert!(recorded("app.dill").verbatim());
        let by_path = |manifest: &EntryManifest, path: &str| {
            manifest.dirs[0]
                .files
                .iter()
                .find(|file| file.path == Path::new(path))
                .unwrap_or_else(|| panic!("no {path} in the manifest"))
                .blob
        };
        assert_eq!(
            recorded("flutter_build.dill.stamp").blob,
            by_path(&seed_manifest, "flutter_build.dill.stamp"),
            "of the entry's blob"
        );

        // Back in tree A, and in tree B again: each vouches by its own
        // record, nothing is republished either way.
        let report = validate_only_run(&tree_a, &config);
        assert_eq!(report.action, Action::PreHitMaterialized);
        assert_eq!(report.ingest_files, 0, "tree A's record holds");
        let report = validate_only_run(&tree_b, &config);
        assert_eq!(report.ingest_files, 0, "tree B's record holds");
        let manifest = store.lookup(&key).expect("lookup").expect("entry");
        assert_eq!(manifest.worktree_root_at_ingest, tree_a.to_string_lossy());
        assert_eq!(manifest.dirs, seed_manifest.dirs);
        assert_eq!(store.stats().expect("stats").blobs, seed_blobs);
    }

    /// The record never masks a real change: a file flutter rewrote with
    /// new bytes after the placement re-hashes and republishes — in the
    /// same tree (only that file stores), and across trees, where the
    /// entry then moves to the hit tree whole: every re-rooted file is
    /// re-hashed to that tree's bytes, so the published manifest names
    /// one tree and one tree's bytes.
    #[test]
    fn rewritten_file_after_placement_is_rehashed_and_republished() {
        let (_guard, tmp) = tempdir();
        let sdk = fake_flutter_sdk(&tmp, "3.47.1");
        let store_root = tmp.join("cas");
        let config = config_for(&store_root, &sdk.join("bin").join("flutter"));
        let tree_a = synthetic_project(&tmp, "a");
        let (key, store) = seed_large_bundle(&tree_a, &config);
        let seed_manifest = store.lookup(&key).expect("lookup").expect("entry");
        let by_path = |manifest: &EntryManifest, path: &str| {
            manifest.dirs[0]
                .files
                .iter()
                .find(|file| file.path == Path::new(path))
                .unwrap_or_else(|| panic!("no {path} in the manifest"))
                .blob
        };

        // Same tree, wiped and re-placed; flutter rebuilds the payload.
        fs::remove_dir_all(tree_a.join(".dart_tool").join("flutter_build")).expect("wipe");
        let mut runner = FakeRunner {
            code: 0,
            argv: Vec::new(),
            effect: Some(Box::new(|cwd| {
                write(
                    &cwd.join(".dart_tool")
                        .join("flutter_build")
                        .join(DIR_MAIN)
                        .join("app.dill"),
                    b"kernel payload bytes v2",
                );
            })),
        };
        let report = run_shim(&raw_argv(&BUNDLE_ARGV), &tree_a, &config, &mut runner);
        assert_eq!(report.action, Action::PreHitMaterialized);
        assert_eq!(report.ingest_files, 1, "only the rewritten payload stores");
        let manifest = store.lookup(&key).expect("lookup").expect("entry");
        assert_eq!(
            by_path(&manifest, "app.dill"),
            BlobId(*blake3::hash(b"kernel payload bytes v2").as_bytes())
        );
        assert_eq!(
            by_path(&manifest, "kernel_snapshot.d"),
            by_path(&seed_manifest, "kernel_snapshot.d"),
            "the placed depfile is vouched by the record"
        );
        let record = placement::load(&tree_a, &key).expect("record");
        assert_eq!(
            record.dirs[0]
                .files
                .iter()
                .find(|file| file.path == Path::new("app.dill"))
                .expect("recorded dill")
                .blob,
            by_path(&manifest, "app.dill"),
            "the record follows the new content"
        );

        // Across trees: tree B hits, flutter rewrites the stamp with NEW
        // bytes. The re-rooted depfile cannot stay a reference into tree
        // A under a manifest rooted in tree B — it is re-hashed too.
        let tree_b = synthetic_project(&tmp, "b");
        let mut runner = FakeRunner {
            code: 0,
            argv: Vec::new(),
            effect: Some(Box::new(|cwd| {
                write(
                    &cwd.join(".dart_tool")
                        .join("flutter_build")
                        .join(DIR_MAIN)
                        .join("flutter_build.dill.stamp"),
                    br#"{"inputs":[],"outputs":[]}"#,
                );
            })),
        };
        let report = run_shim(&raw_argv(&BUNDLE_ARGV), &tree_b, &config, &mut runner);
        assert_eq!(report.action, Action::PreHitMaterialized);
        assert_eq!(
            report.ingest_files, 2,
            "the changed stamp and the re-rooted depfile store; the verbatim payload is reused"
        );
        let manifest = store.lookup(&key).expect("lookup").expect("entry");
        assert_eq!(
            manifest.worktree_root_at_ingest,
            tree_b.to_string_lossy(),
            "the entry moved to tree B"
        );
        let depfile = fs::read_to_string(store.blob_path(&by_path(&manifest, "kernel_snapshot.d")))
            .expect("stored depfile");
        assert!(
            depfile.contains(tree_b.to_str().expect("UTF-8"))
                && !depfile.contains(tree_a.to_str().expect("UTF-8")),
            "the stored depfile is tree B's bytes: {depfile}"
        );
    }

    /// A frame flip is paid once, never per alternation: after tree B
    /// published the entry under its root (real new bytes, as above),
    /// a plain run in tree A demotes its vouched draft, re-hashes, and
    /// finds every file rendering exactly what A's own record claims —
    /// the prior differed only in its frame. The flip back is
    /// suppressed: nothing publishes, the entry keeps B's frame, and
    /// B's next run is a plain same-snapshot hit.
    #[test]
    fn frame_flip_is_suppressed_instead_of_ping_ponging() {
        let (_guard, tmp) = tempdir();
        let sdk = fake_flutter_sdk(&tmp, "3.47.1");
        let store_root = tmp.join("cas");
        let config = config_for(&store_root, &sdk.join("bin").join("flutter"));
        let tree_a = synthetic_project(&tmp, "a");
        let (key, store) = seed_large_bundle(&tree_a, &config);

        // Tree B's first run rewrites the stamp with new bytes of the
        // same class (still naming the tree): the entry publishes under
        // B — the flip itself.
        let tree_b = synthetic_project(&tmp, "b");
        let mut runner = FakeRunner {
            code: 0,
            argv: Vec::new(),
            effect: Some(Box::new(|cwd| {
                let stamp = cwd
                    .join(".dart_tool")
                    .join("flutter_build")
                    .join(DIR_MAIN)
                    .join("flutter_build.dill.stamp");
                let mut bytes = fs::read(&stamp).expect("placed stamp");
                bytes.push(b'\n');
                write(&stamp, &bytes);
            })),
        };
        let report = run_shim(&raw_argv(&BUNDLE_ARGV), &tree_b, &config, &mut runner);
        assert_eq!(report.ingest_files, 2, "real new bytes publish");
        let manifest = store.lookup(&key).expect("lookup").expect("entry");
        assert_eq!(manifest.worktree_root_at_ingest, tree_b.to_string_lossy());
        let flipped_dirs = manifest.dirs.clone();
        let flipped_blobs = store.stats().expect("stats").blobs;

        // Tree A, unchanged since the seed: the demotion lands on A's
        // own claims, so the flip back is suppressed.
        let report = validate_only_run(&tree_a, &config);
        assert_eq!(report.action, Action::PreHitMaterialized);
        assert_eq!(
            report.ingest_files, 0,
            "a frame-only divergence never publishes"
        );
        let manifest = store.lookup(&key).expect("lookup").expect("entry");
        assert_eq!(
            manifest.worktree_root_at_ingest,
            tree_b.to_string_lossy(),
            "the entry keeps the frame it was last published in"
        );
        assert_eq!(manifest.dirs, flipped_dirs);
        assert_eq!(store.stats().expect("stats").blobs, flipped_blobs);
        let record_a = placement::load(&tree_a, &key).expect("tree A's record");
        assert!(
            record_a.dirs[0].files.iter().all(|file| {
                fs::metadata(
                    tree_a
                        .join(".dart_tool")
                        .join("flutter_build")
                        .join(DIR_MAIN)
                        .join(&file.path),
                )
                .is_ok_and(|meta| stamp_of(&meta) == file.stamp)
            }),
            "A's record absorbed the run's identities"
        );

        // B again, then A again: plain hits both ways, no publish.
        assert_eq!(validate_only_run(&tree_b, &config).ingest_files, 0);
        assert_eq!(validate_only_run(&tree_a, &config).ingest_files, 0);
        let manifest = store.lookup(&key).expect("lookup").expect("entry");
        assert_eq!(manifest.dirs, flipped_dirs);
        assert_eq!(store.stats().expect("stats").blobs, flipped_blobs);
    }

    /// A FileStore record naming the tree's files as flutter writes it
    /// into `dir`: an input, the payload, and the payload's byte copy
    /// outside every replayed dir (gradle's `kernel_blob.bin`).
    fn write_file_store(project: &Path, dir: &Path) {
        let record = serde_json::json!({
            "version": 2,
            "files": [
                {"path": format!("{}/lib/main.dart", project.display()), "hash": "11111111111111111111111111111111"},
                {"path": format!("{}/app.dill", dir.display()), "hash": "22222222222222222222222222222222"},
                {"path": format!("{}/build/flutter_assets/kernel_blob.bin", project.display()), "hash": "22222222222222222222222222222222"},
                {"path": format!("{}/.dart_tool/package_config.json", project.display()), "hash": "33333333333333333333333333333333"},
            ],
        });
        write(
            &dir.join(".filecache"),
            &serde_json::to_vec(&record).expect("encode record"),
        );
    }

    /// flutter's every-build rewrite of the FileStore record, as a run
    /// effect: the same records persisted in another hash-map order, the
    /// mtime moved a second on.
    fn rewrite_file_store_reordered(dir: &Path) {
        let path = dir.join(".filecache");
        let mut record: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).expect("read record")).expect("decode record");
        record["files"].as_array_mut().expect("records").reverse();
        fs::write(&path, serde_json::to_vec(&record).expect("encode record")).expect("rewrite");
        fs::File::options()
            .write(true)
            .open(&path)
            .expect("open for mtime")
            .set_modified(SystemTime::now() + Duration::from_secs(1))
            .expect("move mtime");
    }

    /// #4126, closed at the store: a cold hit — a fresh worktree, or a
    /// tree after `flutter clean`, which also takes the tree's placement
    /// record — whose flutter run rewrites `.filecache` in another order
    /// (a hash map's, which follows each tree's paths) stores nothing.
    /// Live, before the order-free identity, every such cold hit read
    /// the rewrite as new content and republished the whole re-rooted
    /// dir under the hit tree — 14 files a cycle, the kernel among them,
    /// alternating between two trees forever. Now the entry keeps its
    /// ingest frame and the store gains no blob, across trees and in
    /// the ingest tree alike.
    #[test]
    fn cold_hits_whose_flutter_run_reorders_the_file_store_store_nothing() {
        let (_guard, tmp) = tempdir();
        let sdk = fake_flutter_sdk(&tmp, "3.47.1");
        let store_root = tmp.join("cas");
        let config = config_for(&store_root, &sdk.join("bin").join("flutter"));
        let tree_a = synthetic_project(&tmp, "a");
        let mut runner = FakeRunner {
            code: 0,
            argv: Vec::new(),
            effect: Some(Box::new(|cwd| {
                let dir = build_fixture_large(cwd, DIR_MAIN);
                write_file_store(cwd, &dir);
            })),
        };
        let seeded = run_shim(&raw_argv(&BUNDLE_ARGV), &tree_a, &config, &mut runner);
        assert_eq!(seeded.ingest_files, 4, "the cold run stores the whole dir");
        let key = seeded.key.expect("post key");
        let store = CasStore::open(&store_root).expect("open store");
        let seed_manifest = store.lookup(&key).expect("lookup").expect("entry");
        let seed_blobs = store.stats().expect("stats").blobs;

        let cold_hit = |tree: &Path| {
            for wiped in ["flutter_build", "dartq-bcache"] {
                let _ = fs::remove_dir_all(tree.join(".dart_tool").join(wiped));
            }
            let mut runner = FakeRunner {
                code: 0,
                argv: Vec::new(),
                effect: Some(Box::new(|cwd| {
                    rewrite_file_store_reordered(
                        &cwd.join(".dart_tool").join("flutter_build").join(DIR_MAIN),
                    );
                })),
            };
            run_shim(&raw_argv(&BUNDLE_ARGV), tree, &config, &mut runner)
        };
        let tree_b = synthetic_project(&tmp, "b");
        for (tree, label) in [
            (&tree_b, "tree B"),
            (&tree_b, "tree B after a clean"),
            (&tree_a, "tree A after a clean"),
            (&tree_b, "tree B again"),
        ] {
            let report = cold_hit(tree.as_path());
            assert_eq!(report.action, Action::PreHitMaterialized, "{label}");
            assert_eq!(
                report.ingest_files, 0,
                "{label}: the reordered record is the placement's"
            );
            let manifest = store.lookup(&key).expect("lookup").expect("entry");
            assert_eq!(
                manifest.worktree_root_at_ingest,
                tree_a.to_string_lossy(),
                "{label}: the entry keeps its ingest frame"
            );
            assert_eq!(manifest.dirs, seed_manifest.dirs, "{label}");
            assert_eq!(
                store.stats().expect("stats").blobs,
                seed_blobs,
                "{label}: no blob written"
            );
        }
    }

    /// flutter's `link_hooks_result.json` as `LinkHooks` writes it into
    /// `dir`: two timestamps and the hook's dependencies.
    fn write_link_hooks_result(dir: &Path, timestamp: &str, dependency: &str) {
        let result = format!(
            r#"{{"build_start":"{timestamp}","build_end":"{timestamp}","dependencies":["{dependency}"],"code_assets":[],"data_assets":[]}}"#
        );
        write(&dir.join("link_hooks_result.json"), result.as_bytes());
    }

    /// The FileStore record of a dir holding a hook result: the result's
    /// md5 (which moves with every timestamp rewrite) and the payload's.
    fn write_file_store_with_hook(dir: &Path, hook_md5: &str) {
        let record = serde_json::json!({
            "version": 2,
            "files": [
                {"path": format!("{}/link_hooks_result.json", dir.display()), "hash": hook_md5},
                {"path": format!("{}/app.dill", dir.display()), "hash": "22222222222222222222222222222222"},
            ],
        });
        write(
            &dir.join(".filecache"),
            &serde_json::to_vec(&record).expect("encode record"),
        );
    }

    /// Moves a file's mtime a second on: flutter's rewrite as the
    /// post-run scan sees it, whatever the clock resolution.
    fn move_mtime(path: &Path) {
        fs::File::options()
            .write(true)
            .open(path)
            .expect("open for mtime")
            .set_modified(SystemTime::now() + Duration::from_secs(1))
            .expect("move mtime");
    }

    /// #4136: an Android hit's rerun rewrites `link_hooks_result.json`
    /// with fresh `build_start`/`build_end` (flutter_tools 3.47
    /// `LinkHooks.build`) and `.filecache` with the result's new md5 —
    /// live, every same-tree hit re-ingested those two files. A rewrite
    /// whose only change is the timestamps stores nothing, on a
    /// same-tree hit and on a wiped tree's hit alike; a rewrite that
    /// changed a real field stores the result AND the record, so the
    /// entry never pairs a fresh result with a record naming an older
    /// one.
    #[test]
    fn hook_result_timestamp_rewrite_after_a_hit_stores_nothing() {
        let (_guard, tmp) = tempdir();
        let sdk = fake_flutter_sdk(&tmp, "3.47.1");
        let store_root = tmp.join("cas");
        let config = config_for(&store_root, &sdk.join("bin").join("flutter"));
        let tree = synthetic_project(&tmp, "a");
        let mut runner = FakeRunner {
            code: 0,
            argv: Vec::new(),
            effect: Some(Box::new(|cwd| {
                let dir = build_fixture_large(cwd, DIR_MAIN);
                write_link_hooks_result(
                    &dir,
                    "2026-11-06T03:20:58.393988",
                    "file:///pub/hook.dart",
                );
                write_file_store_with_hook(&dir, "11111111111111111111111111111111");
            })),
        };
        let seeded = run_shim(&raw_argv(&BUNDLE_ARGV), &tree, &config, &mut runner);
        assert_eq!(seeded.ingest_files, 5, "the cold run stores the whole dir");
        let key = seeded.key.expect("post key");
        let store = CasStore::open(&store_root).expect("open store");
        let seed_manifest = store.lookup(&key).expect("lookup").expect("entry");
        let seed_blobs = store.stats().expect("stats").blobs;

        // A rerun's rewrite of the pair: new timestamps (and so a new
        // md5 in the record) over `dependency`.
        let rerun =
            |tree: &Path, timestamp: &'static str, dependency: &'static str, md5: &'static str| {
                let mut runner = FakeRunner {
                    code: 0,
                    argv: Vec::new(),
                    effect: Some(Box::new(move |cwd: &Path| {
                        let dir = cwd.join(".dart_tool").join("flutter_build").join(DIR_MAIN);
                        write_link_hooks_result(&dir, timestamp, dependency);
                        write_file_store_with_hook(&dir, md5);
                        move_mtime(&dir.join("link_hooks_result.json"));
                        move_mtime(&dir.join(".filecache"));
                    })),
                };
                run_shim(&raw_argv(&BUNDLE_ARGV), tree, &config, &mut runner)
            };

        let same_tree = rerun(
            &tree,
            "2026-11-07T09:01:02.000001",
            "file:///pub/hook.dart",
            "33333333333333333333333333333333",
        );
        assert_eq!(same_tree.action, Action::PreHitMaterialized);
        assert!(same_tree.materialize.as_ref().is_some_and(|m| m.intact));
        assert_eq!(
            same_tree.ingest_files, 0,
            "a timestamp-only rewrite is no new content"
        );

        fs::remove_dir_all(tree.join(".dart_tool").join("flutter_build")).expect("wipe");
        let wiped = rerun(
            &tree,
            "2026-11-07T09:05:00.000002",
            "file:///pub/hook.dart",
            "44444444444444444444444444444444",
        );
        assert_eq!(wiped.action, Action::PreHitMaterialized);
        assert_eq!(
            wiped.ingest_files, 0,
            "the placed pair is recognized under its rewrite too"
        );
        let manifest = store.lookup(&key).expect("lookup").expect("entry");
        assert_eq!(manifest.dirs, seed_manifest.dirs, "never republished");
        assert_eq!(store.stats().expect("stats").blobs, seed_blobs);

        let changed = rerun(
            &tree,
            "2026-11-07T09:10:00.000003",
            "file:///pub/other_hook.dart",
            "55555555555555555555555555555555",
        );
        assert_eq!(
            changed.ingest_files, 2,
            "a real change stores the result and the record naming it"
        );
        let manifest = store.lookup(&key).expect("lookup").expect("entry");
        let blob_of = |manifest: &EntryManifest, path: &str| {
            manifest.dirs[0]
                .files
                .iter()
                .find(|file| file.path == Path::new(path))
                .unwrap_or_else(|| panic!("no {path} in the manifest"))
                .blob
        };
        for path in ["link_hooks_result.json", ".filecache"] {
            assert_ne!(
                blob_of(&manifest, path),
                blob_of(&seed_manifest, path),
                "{path} was republished"
            );
        }
        assert_eq!(
            blob_of(&manifest, "app.dill"),
            blob_of(&seed_manifest, "app.dill"),
            "the payload is still referenced unread"
        );
    }

    /// A record is keyed: a run under another key (a provenance change)
    /// finds no record of its own and hashes the whole dir, however
    /// well the other key's record would have matched the same files.
    #[test]
    fn record_of_another_key_is_never_consulted() {
        let (_guard, tmp) = tempdir();
        let sdk = fake_flutter_sdk(&tmp, "3.47.1");
        let store_root = tmp.join("cas");
        let config = config_for(&store_root, &sdk.join("bin").join("flutter"));
        let tree = synthetic_project(&tmp, "a");
        let (key, _store) = seed_large_bundle(&tree, &config);
        fs::remove_dir_all(tree.join(".dart_tool").join("flutter_build")).expect("wipe");
        let report = validate_only_run(&tree, &config);
        assert_eq!(report.ingest_files, 0);
        assert!(placement::load(&tree, &key).is_some());

        // The key moves; the same files sit in the dir under the same
        // identities the other record names.
        write(&tree.join("pubspec.yaml"), b"name: app\nversion: 9.0.0\n");
        let report = validate_only_run(&tree, &config);
        assert_eq!(report.action, Action::Ingested, "a miss under the new key");
        let other = report.key.expect("post key");
        assert_ne!(other, key);
        assert_eq!(
            report.ingest_files, 3,
            "nothing vouched: the other key's record is not this entry's"
        );
        assert!(
            placement::load(&tree, &other).is_some(),
            "the new key got its own record"
        );
        assert!(
            placement::load(&tree, &key).is_some(),
            "the old one is untouched"
        );
    }

    /// A record that does not decode is ignored — the run proceeds as
    /// if the tree kept none (the manifest's stale stamps re-hash and
    /// republish once) — and the post-run scan writes a sound record
    /// over it.
    #[test]
    fn corrupt_record_is_ignored_and_rewritten() {
        let (_guard, tmp) = tempdir();
        let sdk = fake_flutter_sdk(&tmp, "3.47.1");
        let store_root = tmp.join("cas");
        let config = config_for(&store_root, &sdk.join("bin").join("flutter"));
        let tree = synthetic_project(&tmp, "a");
        let (key, _store) = seed_large_bundle(&tree, &config);
        fs::remove_dir_all(tree.join(".dart_tool").join("flutter_build")).expect("wipe");
        assert_eq!(validate_only_run(&tree, &config).ingest_files, 0);

        let record_path = tree
            .join(".dart_tool")
            .join("dartq-bcache")
            .join("placed")
            .join(format!("{key}.json"));
        assert!(record_path.is_file());
        fs::write(&record_path, b"{\"key\": \"not even").expect("corrupt");
        assert_eq!(placement::load(&tree, &key), None);

        let report = validate_only_run(&tree, &config);
        assert_eq!(report.action, Action::PreHitMaterialized);
        assert_eq!(
            report.ingest_files, 3,
            "no record to vouch: the re-placed identities re-hash and the large payload forces the publish"
        );
        let record = placement::load(&tree, &key).expect("a sound record again");
        assert_eq!(record.dirs[0].files.len(), 3);
        assert_eq!(
            validate_only_run(&tree, &config).ingest_files,
            0,
            "and the next run vouches by it"
        );
    }

    /// `created_at` survives a re-ingest of the same key: the rewritten
    /// manifest carries the prior record's birth time forward, so LRU
    /// aging reflects the entry's age, not the latest run's.
    #[test]
    fn reingest_preserves_the_entry_creation_time() {
        let (_guard, tmp) = tempdir();
        let sdk = fake_flutter_sdk(&tmp, "3.47.1");
        let store_root = tmp.join("cas");
        let config = config_for(&store_root, &sdk.join("bin").join("flutter"));

        let tree = synthetic_project(&tmp, "a");
        let mut runner = FakeRunner {
            code: 0,
            argv: Vec::new(),
            effect: Some(Box::new(|cwd| {
                build_fixture(cwd, DIR_MAIN);
            })),
        };
        let seeded = run_shim(&raw_argv(&BUNDLE_ARGV), &tree, &config, &mut runner);
        assert_eq!(seeded.action, Action::Ingested);
        let key = seeded.key.expect("key");

        // Age the entry: a creation time far in the past, everything
        // else recorded as-is.
        let store = CasStore::open(&store_root).expect("open store");
        let mut manifest = store.lookup(&key).expect("lookup").expect("entry");
        manifest.created_at_unix = 1_000;
        store.ingest(manifest, &[]).expect("age entry");

        // A warm rerun re-ingests under the same key (the run rewrites
        // one bookkeeping file).
        let mut runner = FakeRunner {
            code: 0,
            argv: Vec::new(),
            effect: Some(Box::new(|cwd| {
                write(
                    &cwd.join(".dart_tool")
                        .join("flutter_build")
                        .join(DIR_MAIN)
                        .join("flutter_build.dill.stamp"),
                    br#"{"inputs":["x"],"outputs":["y"]}"#,
                );
            })),
        };
        let report = run_shim(&raw_argv(&BUNDLE_ARGV), &tree, &config, &mut runner);
        assert_eq!(report.action, Action::PreHitMaterialized);

        let manifest = store.lookup(&key).expect("lookup").expect("entry");
        assert_eq!(manifest.created_at_unix, 1_000, "the birth time is kept");
        assert!(
            manifest.last_hit_unix > 1_000,
            "the hit clock still advances"
        );
    }

    /// An older test entry that predates the kernel-cache face (no
    /// test dir in its manifest) passes through sanely: the hit has
    /// nothing to materialize, the child runs, and POST re-ingests the
    /// entry with the dir restored.
    #[test]
    fn older_test_entry_without_cache_dir_runs_and_reingests() {
        let (_guard, tmp) = tempdir();
        let sdk = fake_flutter_sdk(&tmp, "3.47.1");
        let store_root = tmp.join("cas");
        let config = config_for(&store_root, &sdk.join("bin").join("flutter"));

        let tree = synthetic_project(&tmp, "a");
        let mut runner = FakeRunner {
            code: 0,
            argv: Vec::new(),
            effect: Some(Box::new(|cwd| {
                write_test_cache(cwd);
            })),
        };
        let seeded = run_shim(&raw_argv(&["test"]), &tree, &config, &mut runner);
        assert_eq!(seeded.action, Action::Ingested);
        let key = seeded.key.expect("key");

        // Surgically age the entry: drop its dir, keep the key.
        let store = CasStore::open(&store_root).expect("open store");
        let mut manifest = store.lookup(&key).expect("lookup").expect("entry");
        manifest.dirs.clear();
        store.ingest(manifest, &[]).expect("rewrite older entry");

        // The hit materializes nothing (no replayable dir) and the child
        // still runs to exit 0; POST restores the entry in full.
        let mut runner = FakeRunner {
            code: 0,
            argv: Vec::new(),
            effect: Some(Box::new(|cwd| {
                write_test_cache(cwd);
            })),
        };
        let report = run_shim(&raw_argv(&["test"]), &tree, &config, &mut runner);
        assert_eq!(report.reason, "entry_not_replayable");
        assert_eq!(report.exit_code, Some(0));
        assert_eq!(report.key.as_deref(), Some(key.as_str()));
        assert_eq!(report.action, Action::Ingested);
        let manifest = store.lookup(&key).expect("lookup").expect("entry");
        assert_eq!(manifest.dirs.len(), 1);
        assert_eq!(manifest.dirs[0].name_rule, NameRule::TestKernelCache);
    }

    /// The build-dir seam: the env wins when set and non-empty, `build`
    /// otherwise.
    #[test]
    fn flutter_build_dir_env_wins_over_default() {
        assert_eq!(flutter_build_dir(None), PathBuf::from("build"));
        assert_eq!(
            flutter_build_dir(Some(OsString::from("out/debug"))),
            PathBuf::from("out/debug")
        );
        assert_eq!(
            flutter_build_dir(Some(OsString::new())),
            PathBuf::from("build")
        );
    }

    /// The define-value/output-dir reroot replaces the ingest root only
    /// at a whole-path boundary: a value that merely STARTS with the
    /// root (`/old/root-lookalike`, a sibling directory) keeps its
    /// bytes — the recomputed name must hash this tree's true value.
    #[test]
    fn rerooted_replaces_only_whole_path_boundaries() {
        let target = Path::new("/new/root");
        assert_eq!(
            rerooted("/old/root/lib", "/old/root", target),
            "/new/root/lib"
        );
        assert_eq!(rerooted("/old/root", "/old/root", target), "/new/root");
        assert_eq!(
            rerooted("/old/root-lookalike", "/old/root", target),
            "/old/root-lookalike",
            "a sibling prefix is not rewritten"
        );
        assert_eq!(
            rerooted("/elsewhere/x", "/old/root", target),
            "/elsewhere/x"
        );
    }

    /// The shim's resolution is the shared one (A10 lives in
    /// `dartq-toolchain`, whose own tests cover the PATH scan and the
    /// self-spawn refusal): a configured flutter that IS this executable
    /// — directly or through a symlink, the `~/.dartq/bin/flutter` shape
    /// `dartq init` installs — yields None and no child is ever spawned;
    /// a different binary resolves, canonical, so the fingerprint and
    /// the spawn see the real SDK binary behind any link.
    #[cfg(unix)]
    #[test]
    fn discovery_refuses_this_binary_and_canonicalizes_the_rest() {
        let (_guard, tmp) = tempdir();
        let this = std::env::current_exe().expect("test binary");
        assert_eq!(discover_flutter(Some(&this)), None);
        let alias = tmp.join("flutter");
        std::os::unix::fs::symlink(&this, &alias).expect("symlink alias");
        assert_eq!(discover_flutter(Some(&alias)), None);

        let real = tmp.join("sdk").join("bin").join("flutter");
        write(&real, b"#!/bin/sh\nexit 0\n");
        let link = tmp.join("flutter-link");
        std::os::unix::fs::symlink(&real, &link).expect("symlink real");
        assert_eq!(
            discover_flutter(Some(&link)),
            Some(fs::canonicalize(&real).expect("canonicalize real")),
            "a link to a real binary resolves to the binary"
        );
        // Unresolvable passes through verbatim: the spawn reports it.
        let missing = tmp.join("nowhere").join("flutter");
        assert_eq!(discover_flutter(Some(&missing)), Some(missing));
    }

    #[test]
    fn fingerprint_cache_roundtrips_and_invalidates_on_change() {
        let (_guard, tmp) = tempdir();
        let sdk = fake_flutter_sdk(&tmp, "3.47.1");
        let store_root = tmp.join("cas");

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
        let parsed = parse_flutter_args(
            &raw_argv(&argv)
                .iter()
                .map(|t| t.to_string_lossy().into_owned())
                .collect::<Vec<_>>(),
        )
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
            &[
                "build",
                "bundle",
                "--dart-define",
                "FLUTTER_BUILD_NAME=9.9.9",
            ]
            .iter()
            .map(|token| (*token).to_owned())
            .collect::<Vec<_>>(),
        )
        .expect("parse");
        assert!(
            dart_define_vector(&over, &sdk, &toolchain)
                .contains(&("FLUTTER_BUILD_NAME".to_owned(), "9.9.9".to_owned()))
        );
    }

    /// A fake flutter as an executable script. The production runner
    /// spawns it for real; the body is inline so each test records
    /// exactly what it wants observed.
    #[cfg(unix)]
    fn script(path: &Path, body: &str) -> PathBuf {
        write(path, format!("#!/bin/sh\n{body}\n").as_bytes());
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).expect("chmod script");
        path.to_path_buf()
    }

    /// Serializes the tests that drive the production runner: the
    /// forwarder's static child pid and its install/restore sigaction
    /// cycle are single-instance by design (one flutter per shim
    /// process), so overlapping runner tests clobber each other's
    /// process-wide signal state.
    #[cfg(unix)]
    static RUNNER_TESTS: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// A10's env glue: the shim tag never rides into a child — a
    /// nested dartq that inherited it would mistake itself for a shim.
    #[cfg(unix)]
    #[test]
    fn command_runner_strips_the_shim_tag_from_the_child_env() {
        let _serial = RUNNER_TESTS
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let (_guard, tmp) = tempdir();
        let probe = tmp.join("env-probe");
        let flutter = script(
            &tmp.join("fake-flutter"),
            &format!(
                "if [ -n \"$DARTQ_SHIM\" ]; then echo present > {probe}; else echo stripped > {probe}; fi",
                probe = probe.display()
            ),
        );
        // SAFETY: a process-wide environment mutation while the test
        // binary's other threads may run: no other test in this binary
        // reads or writes DARTQ_SHIM, and the runner copies the
        // environment at spawn time, inside this test's thread.
        unsafe { std::env::set_var("DARTQ_SHIM", "1") };
        let mut runner = CommandRunner::new(flutter);
        let code = runner.run(&[], &tmp).expect("run");
        unsafe { std::env::remove_var("DARTQ_SHIM") };
        assert_eq!(code, 0);
        assert_eq!(
            fs::read_to_string(&probe).expect("probe written").trim(),
            "stripped",
            "the child environment must not carry the shim tag"
        );
    }

    /// The child half of [`exec_replaces_the_shim_process`]: invoked by
    /// the parent test with the fixture env set, it execs into the fake
    /// flutter binary. In a plain suite run (no env) it is a no-op, so
    /// the helper costs one passing test.
    #[cfg(unix)]
    #[test]
    fn exec_child_becomes_flutter() {
        let (Some(flutter), Some(out)) = (
            std::env::var_os("DARTQ_EXEC_FIXTURE"),
            std::env::var_os("DARTQ_EXEC_OUT"),
        ) else {
            return;
        };
        let mut runner = CommandRunner::new(PathBuf::from(flutter));
        match runner.exec(&[], Path::new("/")) {
            Ok(code) => std::process::exit(code),
            Err(err) => {
                let _ = fs::write(out, format!("exec failed: {err}"));
                std::process::exit(70);
            }
        }
    }

    /// A11's passthrough face: `CommandRunner::exec` REPLACES the
    /// calling process with flutter — the fake flutter records the pid
    /// it runs as, and that pid is the child process's own. A spawn
    /// (the old behavior) would record a grandchild's pid.
    #[cfg(unix)]
    #[test]
    fn exec_replaces_the_shim_process() {
        let (_guard, tmp) = tempdir();
        let out = tmp.join("exec-pid");
        let flutter = script(
            &tmp.join("fake-flutter"),
            &format!("echo $$ > {}", out.display()),
        );
        let mut child = std::process::Command::new(std::env::current_exe().expect("test binary"))
            .args([
                "--exact",
                "orchestrator::tests::exec_child_becomes_flutter",
                "--nocapture",
            ])
            .env("DARTQ_EXEC_FIXTURE", &flutter)
            .env("DARTQ_EXEC_OUT", &out)
            .spawn()
            .expect("spawn helper test");
        let pid = child.id();
        let status = child.wait().expect("wait helper");
        assert!(status.success(), "the exec'd flutter exits 0: {status}");
        let recorded = fs::read_to_string(&out)
            .expect("fake flutter ran")
            .trim()
            .parse::<u32>()
            .expect("pid");
        assert_eq!(
            recorded, pid,
            "the child process BECAME flutter via exec — same pid, not a grandchild"
        );
    }

    /// A11's cacheable face: SIGTERM to the shim is forwarded to the
    /// child, the child dies by that signal, and the shim reports the
    /// child's status (128 + signal) after reaping it — no orphaned
    /// flutter_tools under PPID 1.
    #[cfg(unix)]
    #[test]
    fn forwarded_sigterm_reaches_the_child_and_reports_its_status() {
        let _serial = RUNNER_TESTS
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let (_guard, tmp) = tempdir();
        let ready = tmp.join("child-pid");
        let flutter = script(
            &tmp.join("fake-flutter"),
            &format!("echo $$ > {}\nsleep 15", ready.display()),
        );
        let cwd = tmp.clone();
        let runner = std::thread::spawn(move || {
            let mut runner = CommandRunner::new(flutter);
            runner.run(&[], &cwd)
        });
        let mut child_pid = 0;
        for _ in 0..200 {
            if let Ok(text) = fs::read_to_string(&ready)
                && let Ok(parsed) = text.trim().parse::<i32>()
            {
                child_pid = parsed;
                break;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        assert_ne!(child_pid, 0, "the fake flutter must start");
        // SIGTERM this process — the shim's position in production.
        unsafe { libc::kill(std::process::id() as libc::pid_t, libc::SIGTERM) };
        let code = runner.join().expect("runner thread").expect("run");
        assert_eq!(
            code,
            128 + libc::SIGTERM,
            "the child's signal death is reported"
        );
        // The child is reaped and gone.
        // SAFETY: signal 0 only probes for the pid's existence.
        assert_eq!(unsafe { libc::kill(child_pid, 0) }, -1, "the child is gone");
    }

    /// The #4057 carry-over: the budget probe is the O(1) running
    /// counter (`used_bytes`), and crossing it runs gc (grace 0, tiny
    /// budget → the entry is evicted and its blobs removed).
    #[test]
    fn maybe_gc_probes_the_running_counter_and_collects_over_budget() {
        let (_guard, tmp) = tempdir();
        let sdk = fake_flutter_sdk(&tmp, "3.47.1");
        let store_root = tmp.join("cas");
        let mut config = config_for(&store_root, &sdk.join("bin").join("flutter"));
        config.budget_bytes = u64::MAX;
        config.grace_secs = 0;

        let tree = synthetic_project(&tmp, "a");
        let mut runner = FakeRunner {
            code: 0,
            argv: Vec::new(),
            effect: Some(Box::new(|cwd| {
                build_fixture(cwd, DIR_MAIN);
            })),
        };
        let report = run_shim(&raw_argv(&BUNDLE_ARGV), &tree, &config, &mut runner);
        assert_eq!(report.action, Action::Ingested);
        let key = report.key.expect("key");
        let store = CasStore::open(&store_root).expect("open store");
        let manifest = store.lookup(&key).expect("lookup").expect("entry");
        let blob = manifest.dirs[0].files[0].blob;
        assert!(store.blob_path(&blob).is_file());
        assert!(store.used_bytes().expect("used") > 0, "ingest books bytes");

        // Over budget by the counter alone: gc evicts (grace 0). The
        // blobs themselves are younger than the in-flight window and
        // stay; the counter heals to exactly the bytes that remain.
        config.budget_bytes = 0;
        maybe_gc(&store, &config);
        assert_eq!(
            store.stats().expect("stats").entries,
            0,
            "grace 0 + no budget evicts"
        );
        assert_eq!(
            store.used_bytes().expect("used after gc"),
            store.stats().expect("stats").blob_bytes,
            "the counter heals to the bytes the pass observed"
        );
    }

    /// A14 end to end: with the tree lock held by another shim, PRE
    /// degrades to a clean miss (nothing materialized, reason recorded)
    /// and POST skips the ingest — the build itself runs either way.
    #[cfg(unix)]
    #[test]
    fn tree_lock_contention_degrades_pre_and_post_to_clean_misses() {
        let (_guard, tmp) = tempdir();
        let sdk = fake_flutter_sdk(&tmp, "3.47.1");
        let store_root = tmp.join("cas");
        let config = config_for(&store_root, &sdk.join("bin").join("flutter"));

        // Seed the entry from tree A with two recorded dirs.
        let tree_a = synthetic_project(&tmp, "a");
        let mut runner = FakeRunner {
            code: 0,
            argv: Vec::new(),
            effect: Some(Box::new(|cwd| {
                build_fixture(cwd, DIR_MAIN);
                build_fixture(cwd, DIR_ALT);
            })),
        };
        let seeded = run_shim(&raw_argv(&BUNDLE_ARGV), &tree_a, &config, &mut runner);
        assert_eq!(seeded.action, Action::Ingested);

        // A second shim in tree B while tree B's lock is held: the run
        // proceeds (and fabricates ONE of the two dirs itself), both
        // cache sections degrade.
        let tree_b = synthetic_project(&tmp, "b");
        let _held = crate::fsutil::TreeLock::acquire(&tree_b, Duration::from_millis(50))
            .expect("acquire")
            .expect("lock");
        let mut runner = FakeRunner {
            code: 0,
            argv: Vec::new(),
            effect: Some(Box::new(|cwd| {
                build_fixture(cwd, DIR_MAIN);
            })),
        };
        let report = run_shim(&raw_argv(&BUNDLE_ARGV), &tree_b, &config, &mut runner);
        assert_eq!(report.reason, "tree_lock_busy", "PRE degraded to a miss");
        assert_eq!(report.action, Action::PreMiss);
        assert!(report.materialize.is_none(), "no materialization ran");
        assert!(
            tree_b
                .join(".dart_tool")
                .join("flutter_build")
                .join(DIR_MAIN)
                .join("app.dill")
                .is_file(),
            "the run's own output is untouched by the degradation"
        );
        assert!(
            !tree_b
                .join(".dart_tool")
                .join("flutter_build")
                .join(DIR_ALT)
                .exists(),
            "the dir only PRE could place never appears under contention"
        );
        assert_eq!(report.exit_code, Some(0), "the build itself ran");
        // POST skipped too: the entry population is unchanged.
        let entries = CasStore::open(&store_root)
            .expect("open")
            .stats()
            .expect("stats")
            .entries;
        assert_eq!(entries, 1, "no ingest happened under contention");
        // And the run left no marker behind.
        assert!(
            !tree_b
                .join(".dart_tool")
                .join(format!("dartq-bcache-run-{}", std::process::id()))
                .exists(),
            "the run marker is removed after the run"
        );
    }

    /// A22's gate: a file written immediately before the run — fresh
    /// mtime, same whole second as the old floored start — is NOT
    /// touched. The marker file's full-resolution mtime is the gate.
    #[test]
    fn pre_run_write_in_the_same_second_is_not_touched() {
        let (_guard, tmp) = tempdir();
        let sdk = fake_flutter_sdk(&tmp, "3.47.1");
        let store_root = tmp.join("cas");
        let config = config_for(&store_root, &sdk.join("bin").join("flutter"));

        // The dill an earlier moment left, with a completely fresh
        // mtime (the floored gate would see at >= floor(now) and call
        // it touched); the run itself writes nothing.
        let tree = synthetic_project(&tmp, "a");
        write_test_cache(&tree);
        std::thread::sleep(Duration::from_millis(20));
        let mut runner = FakeRunner::exits(1);
        let report = run_shim(&raw_argv(&["test"]), &tree, &config, &mut runner);
        assert_eq!(report.exit_code, Some(1));
        assert_eq!(report.action, Action::PreMiss);
        assert_eq!(report.ingest_files, 0, "a pre-run write is not a touch");
        let stats = CasStore::open(&store_root)
            .expect("open store")
            .stats()
            .expect("stats");
        assert_eq!(stats.entries, 0);
        assert_eq!(stats.blobs, 0);
    }

    /// A22's record half: a touched dir whose assemble capture was
    /// missed keeps the prior entry's naming instead of being demoted
    /// to not-replayable — the entry keeps serving fresh trees.
    #[cfg(unix)]
    #[test]
    fn missed_capture_keeps_the_prior_dir_record_replayable() {
        let (_guard, tmp) = tempdir();
        let sdk = fake_flutter_sdk(&tmp, "3.47.1");
        let flutter_bin = sdk.join("bin").join("flutter");
        let engine_revision = key::fingerprint_toolchain(&sdk)
            .expect("fingerprint")
            .engine_revision;
        let store_root = tmp.join("cas");
        let config = config_for(&store_root, &flutter_bin);

        // Seed with a REAL observation (the lookalike), as gradle would.
        let tree = synthetic_project(&tmp, "a");
        let output = tree.join("build/app/intermediates/flutter/release");
        let argv = assemble_argv(&flutter_bin, &output.to_string_lossy());
        let template = base_capture(&argv, &tree, &engine_revision);
        let seed_argv = argv.clone();
        let seed_name = template.name.clone();
        let mut runner = FakeRunner {
            code: 0,
            argv: Vec::new(),
            effect: Some(Box::new(move |cwd| {
                run_assemble_lookalike(&seed_argv);
                build_fixture(cwd, &seed_name);
            })),
        };
        let seeded = run_shim(&raw_argv(&APK_ARGV), &tree, &config, &mut runner);
        assert_eq!(seeded.action, Action::Ingested);
        let key = seeded.key.clone().expect("key");
        let store = CasStore::open(&store_root).expect("open store");
        let manifest = store.lookup(&key).expect("lookup").expect("entry");
        assert_eq!(manifest.dirs[0].name_rule, NameRule::RecomputeFromTemplate);
        let seed_defines = manifest.dirs[0].name_defines.clone();

        // The rerun touches the dir (flutter rewrote the payload) but
        // the observer captures NOTHING — a missed capture.
        let warm_name = template.name.clone();
        let mut runner = FakeRunner {
            code: 0,
            argv: Vec::new(),
            effect: Some(Box::new(move |cwd| {
                write(
                    &cwd.join(".dart_tool")
                        .join("flutter_build")
                        .join(&warm_name)
                        .join("app.dill"),
                    b"kernel payload bytes v2",
                );
            })),
        };
        let report = run_shim(&raw_argv(&APK_ARGV), &tree, &config, &mut runner);
        assert_eq!(report.action, Action::PreHitMaterialized);
        assert_eq!(report.ingest_files, 1, "the rewrite re-stores");
        let manifest = store.lookup(&key).expect("lookup").expect("entry");
        assert_eq!(
            manifest.dirs[0].name_rule,
            NameRule::RecomputeFromTemplate,
            "the missed capture must not demote the dir to provenance"
        );
        assert_eq!(manifest.dirs[0].name_defines, seed_defines);

        // The entry still serves a fresh tree under the template.
        let tree_b = synthetic_project(&tmp, "b");
        let mut runner = FakeRunner::exits(0);
        let report = run_shim(&raw_argv(&APK_ARGV), &tree_b, &config, &mut runner);
        assert_eq!(report.action, Action::PreHitMaterialized);
        let output_b = rerooted(
            &template.output_dir,
            &manifest.worktree_root_at_ingest,
            &tree_b,
        );
        let name_b =
            flutter_build_dir_name(&engine_revision, &template.defines, Path::new(&output_b));
        assert!(
            tree_b
                .join(".dart_tool")
                .join("flutter_build")
                .join(&name_b)
                .join("app.dill")
                .is_file()
        );
    }
}
