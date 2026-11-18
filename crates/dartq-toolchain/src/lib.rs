//! One toolchain resolution for every dartq face.
//!
//! Three faces spawn the `flutter` and `dart` executables — the `flutter`
//! PATH shim and `dartq flutter`, the test faces, and the debug launcher —
//! and a user who pins one SDK expects every face to run it. The precedence
//! the shim established is the one they all share:
//!
//! 1. the explicit pin a face has read from its config (`build_cache.flutter`,
//!    `~` already expanded by the config reader);
//! 2. the environment override, `DARTQ_FLUTTER` / `DARTQ_DART` (an empty
//!    value counts as unset);
//! 3. the project's `.fvmrc`: the first one found walking upward from the
//!    cwd pins an FVM version — the fvm 3.x/4.x JSON `{"flutter": "<v>"}`,
//!    or the legacy bare version fvm 1.x/2.x wrote — resolving to
//!    `~/.fvm/versions/<v>/bin/<tool>` (or the pre-3.0
//!    `~/fvm/versions/<v>`); a file pinning nothing, a flavor declaration
//!    dartq does not resolve, or an uninstalled pin is skipped, never
//!    fatal — one stderr note, and the walk continues;
//! 4. the first executable of that name on `PATH`.
//!
//! Every step passes over the running dartq binary itself. `dartq init` puts
//! a `flutter` link to dartq first on `PATH`, so the scan skips whatever
//! resolves to this executable; an override naming it is refused outright —
//! spawning it would fork-bomb dartq into itself, each spawned shim spawning
//! another at ~210 processes/second (the build-cache fork-bomb audit, A10).
//! Only the binary is excluded, never its directory: excluding the whole
//! directory also hid a real SDK flutter installed beside the shim.
//!
//! The second shared face is `run_queue` (unix): the machine-wide queue
//! every test execution joins before it spawns those binaries. The third is
//! `proc` (unix): the process-tree RSS sum and kill that bound an analyzer's
//! memory.

#![allow(clippy::semicolon_if_nothing_returned)]
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

// Unix-only: off unix there is no `flock`, and callers run unqueued.
#[cfg(unix)]
pub mod run_queue;

/// Process-tree RSS and kill (unix): the analyzer memory budget's substrate.
#[cfg(unix)]
pub mod proc;

/// The machine-wide memory ledger every heavy face leases from (unix).
#[cfg(unix)]
pub mod ledger;

/// The `dart` PATH shim: install/remove of `~/.dartq/bin/dart` and the
/// passthrough spawn for every non-`analyze` invocation.
pub mod shim;

/// One executable's resolution facts: its file name, the environment
/// override, and the refusal's fix hint naming every way a user can point
/// dartq at the real binary.
struct Tool {
    name: &'static str,
    env_var: &'static str,
    fix: &'static str,
}

const FLUTTER: Tool = Tool {
    name: "flutter",
    env_var: "DARTQ_FLUTTER",
    fix: "the build_cache `flutter` setting or DARTQ_FLUTTER",
};

const DART: Tool = Tool {
    name: "dart",
    env_var: "DARTQ_DART",
    fix: "DARTQ_DART",
};

/// Resolves the `flutter` binary a face spawns: `explicit` (the
/// `build_cache.flutter` pin), then `$DARTQ_FLUTTER`, then the `.fvmrc`
/// pin's `~/.fvm/versions/<v>/bin/flutter`, then the `PATH` scan.
/// `None` when nothing resolves, or — after one explanatory stderr line
/// naming both paths — when the override is this executable.
#[must_use]
pub fn find_flutter(explicit: Option<&Path>) -> Option<PathBuf> {
    resolve(&FLUTTER, explicit)
}

/// Resolves the `dart` binary a face spawns: `explicit` (no config key pins
/// dart today — faces pass `None`), then `$DARTQ_DART`, then the `.fvmrc`
/// pin's `~/.fvm/versions/<v>/bin/dart`, then the `PATH` scan; the same
/// `None` cases as [`find_flutter`].
#[must_use]
pub fn find_dart(explicit: Option<&Path>) -> Option<PathBuf> {
    resolve(&DART, explicit)
}

/// The fvm-chain resolution the `dartq fvm <tool>` face pins: the
/// environment override first (an explicit user override beats a project
/// pin), then the `.fvmrc` pin's version binary, then the fvm default
/// (`~/.fvm/default/bin/<tool>`, pre-3.0 `~/fvm/default/bin/<tool>`), and —
/// as the graceful last resort, never a refusal — the same `PATH` scan the
/// shared chain runs. A `build_cache.flutter`-style explicit pin is the
/// caller's step, applied ahead of this one.
#[must_use]
pub fn find_flutter_fvm() -> Option<PathBuf> {
    find_tool_fvm(&FLUTTER)
}

/// [`find_flutter_fvm`] for `dart`: `$DARTQ_DART`, `.fvmrc`, the fvm
/// default, `PATH`.
#[must_use]
pub fn find_dart_fvm() -> Option<PathBuf> {
    find_tool_fvm(&DART)
}

/// The fvm chain over a `Tool`: env override, `.fvmrc`, the fvm default,
/// `PATH` — every override through the self-spawn refusal.
fn find_tool_fvm(tool: &Tool) -> Option<PathBuf> {
    let own = own_binary();
    if let Some(path) = std::env::var_os(tool.env_var).filter(|value| !value.is_empty()) {
        return non_recursing(tool, Path::new(&path), own.as_deref());
    }
    if let Some(bin) = fvm_tool_binary(tool.name) {
        return non_recursing(tool, &bin, own.as_deref());
    }
    if let Some(bin) = fvm_default_root(std::env::home_dir().as_deref())
        .map(|root| root.join("bin").join(tool.name))
        && bin.is_file()
    {
        return non_recursing(tool, &bin, own.as_deref());
    }
    scan_in(path_dirs(), tool.name, own.as_deref())
}

/// The `PATH` step alone: the first executable named `name` on `PATH`, as
/// `which` finds it, never this binary itself.
#[must_use]
pub fn find_on_path(name: &str) -> Option<PathBuf> {
    scan_in(path_dirs(), name, own_binary().as_deref())
}

/// [`find_on_path`] over `dirs`, skipping every candidate that resolves to
/// `skip` (the running binary's canonical path; `None` skips nothing). A
/// candidate that does not resolve is not this binary. The hit is the
/// directory entry as found, not its symlink target.
#[must_use]
pub fn scan_in(
    dirs: impl IntoIterator<Item = PathBuf>,
    name: &str,
    skip: Option<&Path>,
) -> Option<PathBuf> {
    let names = candidate_names(name, cfg!(windows));
    dirs.into_iter().find_map(|dir| {
        names.iter().map(|name| dir.join(name)).find(|candidate| {
            candidate.is_file()
                && skip.is_none_or(|skip| fs::canonicalize(candidate).ok().as_deref() != Some(skip))
        })
    })
}

/// The file names one lookup tries for `name`, in order: the three
/// spellings a Windows tool ships under, the bare name elsewhere.
fn candidate_names(name: &str, windows: bool) -> Vec<String> {
    if windows {
        vec![
            format!("{name}.exe"),
            format!("{name}.bat"),
            format!("{name}.cmd"),
        ]
    } else {
        vec![name.to_owned()]
    }
}

/// [`resolve_in`] over the process environment.
fn resolve(tool: &Tool, explicit: Option<&Path>) -> Option<PathBuf> {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    resolve_in(
        tool,
        explicit,
        std::env::var_os(tool.env_var),
        Some(cwd.as_path()),
        std::env::home_dir().as_deref(),
        path_dirs(),
    )
}

/// The resolution core over injected inputs (the test seam): the pin, then
/// the environment override (empty is unset), then the `.fvmrc` pin's
/// version binary under `home`, then the scan over `dirs` — the overrides
/// through the self-spawn refusal, the scan past this binary.
fn resolve_in(
    tool: &Tool,
    explicit: Option<&Path>,
    env: Option<OsString>,
    cwd: Option<&Path>,
    home: Option<&Path>,
    dirs: impl IntoIterator<Item = PathBuf>,
) -> Option<PathBuf> {
    let own = own_binary();
    if let Some(path) = explicit {
        return non_recursing(tool, path, own.as_deref());
    }
    if let Some(path) = env.filter(|value| !value.is_empty()) {
        return non_recursing(tool, Path::new(&path), own.as_deref());
    }
    if let Some(bin) = fvmrc_tool_binary(cwd, home, tool.name) {
        return non_recursing_raw(tool.name, &bin, own.as_deref());
    }
    scan_in(dirs, tool.name, own.as_deref())
}

/// A resolved-by-override path, canonicalized when possible and refused
/// when it names this executable (the fork bomb above). The refusal names
/// both the requested and the current path so the misconfiguration is
/// visible in one line. An unresolvable override (absent, dangling symlink)
/// passes through verbatim — the spawn fails with its own diagnostic later.
fn non_recursing(tool: &Tool, path: &Path, own: Option<&Path>) -> Option<PathBuf> {
    non_recursing_named(tool.name, tool.fix, path, own)
}

/// [`non_recursing`] for a caller holding a bare name — the `dartq fvm`
/// face's `flutter`/`dart` and the `.fvmrc` step's version binary — whose
/// refusal names the config-free fix (the fvm pin itself, or an env
/// override).
fn non_recursing_raw(name: &str, path: &Path, own: Option<&Path>) -> Option<PathBuf> {
    non_recursing_named(name, "the project's .fvmrc or DARTQ_FLUTTER/DARTQ_DART", path, own)
}

/// The fork-bomb refusal core: canonicalize `path`, refuse it when it IS
/// this executable — naming `fix` as the way out — and hand the canonical
/// (or verbatim unresolvable) path back otherwise.
fn non_recursing_named(name: &str, fix: &str, path: &Path, own: Option<&Path>) -> Option<PathBuf> {
    let Ok(resolved) = fs::canonicalize(path) else {
        return Some(path.to_path_buf());
    };
    if let Some(own) = own
        && own == resolved.as_path()
    {
        eprintln!(
            "dartq: refusing to run {} at {}: it is this dartq binary itself ({}) — \
             point {} at the real SDK binary",
            name,
            resolved.display(),
            own.display(),
            fix
        );
        return None;
    }
    Some(resolved)
}

/// One `.fvmrc` read's verdict — the shared outcome both `.fvmrc` steps
/// (this crate's binary lookup and the commands crate's SDK-root step)
/// act on alike.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FvmrcPin {
    /// The version the file pins: the JSON object's `flutter` member, or
    /// the legacy bare-string content.
    Pin(String),
    /// A flavor declaration. fvm installs a flavored pin at
    /// `versions/<version>_<flavor>` — a layout dartq does not resolve —
    /// so callers note it and fall through rather than mispicking.
    Flavor(String),
    /// Nothing pinned: empty content, or a JSON object without a usable
    /// `flutter` member.
    Skip,
}

/// The pin one `.fvmrc`'s content declares. fvm 3.x/4.x writes a JSON
/// object — `{"flutter": "<version>"}`, optionally with a `"flavor"` —
/// while fvm 1.x/2.x wrote the bare version as the whole file; content
/// that is not a JSON object reads the legacy way (trimmed, non-empty =
/// the pin). An object pins its `flutter` member only when that is a
/// non-empty string; a flavor declaration outranks the version member —
/// see [`FvmrcPin::Flavor`].
#[must_use]
pub fn fvmrc_version(content: &str) -> FvmrcPin {
    if let Ok(serde_json::Value::Object(object)) = serde_json::from_str::<serde_json::Value>(content)
    {
        if let Some(flavor) = object
            .get("flavor")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|flavor| !flavor.is_empty())
        {
            return FvmrcPin::Flavor(flavor.to_owned());
        }
        return match object.get("flutter").and_then(|value| value.as_str()) {
            Some(version) if !version.trim().is_empty() => {
                FvmrcPin::Pin(version.trim().to_owned())
            }
            _ => FvmrcPin::Skip,
        };
    }
    let version = content.trim();
    if version.is_empty() {
        FvmrcPin::Skip
    } else {
        FvmrcPin::Pin(version.to_owned())
    }
}

/// The `.fvmrc` pins on the walk from `dir` upward, nearest first: each
/// readable file through [`fvmrc_version`]. Files that pin nothing are
/// passed over, and a flavor declaration prints one stderr note and is
/// passed over the same way — dartq does not resolve flavors. The caller
/// resolves each version against its install layout and notes the misses.
pub fn fvmrc_versions(dir: &Path) -> impl Iterator<Item = String> + '_ {
    dir.ancestors().filter_map(|dir| {
        match fvmrc_version(&fs::read_to_string(dir.join(".fvmrc")).ok()?) {
            FvmrcPin::Pin(version) => Some(version),
            FvmrcPin::Flavor(flavor) => {
                eprintln!(
                    "dartq: .fvmrc declares flavor {flavor} which dartq does not resolve; \
                     falling back"
                );
                None
            }
            FvmrcPin::Skip => None,
        }
    })
}

/// The `.fvmrc` step's binary for `tool`: each `.fvmrc` walking upward
/// from `cwd` (nearest first) pins a version through [`fvmrc_versions`];
/// when that version's fvm directory holds `<dir>/bin/<tool>` the binary
/// answers, otherwise one stderr note and the walk continues — an
/// uninstalled pin is never a refusal (the chain falls through to
/// `PATH`, matching fvm's own "version not installed" posture without
/// its hard stop). `None` covers every miss: no `.fvmrc` on the chain, no
/// pin in any of them, no `home`, or no installed version anywhere up
/// the walk.
fn fvmrc_tool_binary(cwd: Option<&Path>, home: Option<&Path>, tool: &str) -> Option<PathBuf> {
    for version in fvmrc_versions(cwd?) {
        if let Some(bin) = fvm_version_dir(home, &version)
            .map(|root| root.join("bin").join(tool))
            && bin.is_file()
        {
            return Some(bin);
        }
        eprintln!("dartq: .fvmrc pins {version} but it is not installed — falling back");
    }
    None
}

/// [`fvmrc_tool_binary`] over the process cwd and home — the
/// [`find_tool_fvm`] step.
fn fvm_tool_binary(tool: &str) -> Option<PathBuf> {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    fvmrc_tool_binary(Some(cwd.as_path()), std::env::home_dir().as_deref(), tool)
}

/// The one fvm layout fact every face shares — single-sourced here so the
/// locator (`dartq.yaml`'s `sdk: fvm:<v>`), the `.fvmrc` walk, and the
/// `dartq fvm` face never disagree on where a version lives:
/// `$HOME/.fvm/versions/<version>` (FVM 3's layout), or the pre-3.0
/// `$HOME/fvm/versions/<version>` when that is the one on disk. Neither
/// installed → `None` (the caller decides whether the miss is a skip or a
/// reported unavailability). `None` without a home too.
#[must_use]
pub fn fvm_version_dir(home: Option<&Path>, version: &str) -> Option<PathBuf> {
    let home = home?;
    let modern = home.join(".fvm").join("versions").join(version);
    let legacy = home.join("fvm").join("versions").join(version);
    if modern.is_dir() {
        Some(modern)
    } else if legacy.is_dir() {
        Some(legacy)
    } else {
        None
    }
}

/// The fvm default's root — a FLUTTER SDK root whose embedded
/// `bin/cache/dart-sdk` is the engine's last-resolution SDK: `$HOME/.fvm/
/// default` (FVM 3's layout), or the pre-3.0 `$HOME/fvm/default` when that
/// is the one on disk. `None` without a home directory; a missing default
/// resolves to the modern path (the probe downstream reports the miss —
/// the default is a symlink fvm may not have created yet, so existence is
/// no admission test here).
#[must_use]
pub fn fvm_default_root(home: Option<&Path>) -> Option<PathBuf> {
    let home = home?;
    let modern = home.join(".fvm").join("default");
    let legacy = home.join("fvm").join("default");
    Some(if legacy.is_dir() && !modern.is_dir() {
        legacy
    } else {
        modern
    })
}

/// `PATH`, split; unset reads as empty.
fn path_dirs() -> Vec<PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()).collect()
}

/// The running binary's canonical path, which every self-match compares
/// against (`current_exe` can itself sit behind a symlink).
fn own_binary() -> Option<PathBuf> {
    std::env::current_exe()
        .and_then(|exe| fs::canonicalize(&exe))
        .ok()
}

/// Whether a runner command line asks for nothing but an answer that runs
/// no tests: help and version, alone or beside dartq's own switches
/// (`--no-cache`, `--no-queue`, `--stats`, `--flutter`). Such a line must
/// not wait behind an hours-long run at the job gate — the runner prints
/// its text and exits. Any other token (a path, a filter, a runner flag,
/// anything past `--`) is execution intent and keeps the queue.
#[must_use]
pub fn zero_execution(args: &[String]) -> bool {
    let mut asks = false;
    for arg in args {
        match arg.as_str() {
            "--help" | "-h" | "--version" => asks = true,
            "--no-cache" | "--no-queue" | "--stats" | "--flutter" => {}
            _ => return false,
        }
    }
    asks
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Help and version lines skip the queue; any execution token keeps it.
    #[test]
    fn zero_execution_lines_are_help_and_version_only() {
        let line =
            |args: &[&str]| -> Vec<String> { args.iter().map(|a| (*a).to_owned()).collect() };
        assert!(zero_execution(&line(&["--help"])));
        assert!(zero_execution(&line(&["-h"])));
        assert!(zero_execution(&line(&["--version"])));
        assert!(zero_execution(&line(&[
            "--flutter",
            "--help",
            "--no-cache",
            "--stats"
        ])));
        assert!(
            !zero_execution(&line(&[])),
            "an empty line runs every suite"
        );
        assert!(
            !zero_execution(&line(&["--no-cache"])),
            "switches alone still run"
        );
        assert!(!zero_execution(&line(&["--help", "test/a_test.dart"])));
        assert!(!zero_execution(&line(&["--help", "-n", "x"])));
        assert!(!zero_execution(&line(&["--", "--help"])));
        assert!(!zero_execution(&line(&["--stats=verbose", "--help"])));
    }

    /// A scratch directory under the system temp root, removed on drop (std
    /// only — the crate carries no dependencies, test-side either).
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(tag: &str) -> Self {
            let dir =
                std::env::temp_dir().join(format!("dartq-toolchain-{tag}-{}", std::process::id()));
            fs::create_dir_all(&dir).expect("scratch dir");
            Self(dir)
        }

        /// A subdirectory holding a plain `flutter` file (an SDK stand-in).
        fn sdk(&self, name: &str) -> PathBuf {
            let dir = self.0.join(name);
            fs::create_dir_all(&dir).expect("sdk dir");
            fs::write(dir.join("flutter"), "").expect("sdk tool");
            dir
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// The pin beats the environment, the environment beats the scan; the
    /// overrides come back resolved, the scan hit as found.
    #[test]
    fn pin_then_env_then_path() {
        let scratch = Scratch::new("precedence");
        let pinned = scratch.sdk("pinned").join("flutter");
        let env = scratch.sdk("env").join("flutter");
        let path = scratch.sdk("path");
        let empty = scratch.0.join("empty-home");
        let env_value = Some(env.clone().into_os_string());
        assert_eq!(
            resolve_in(
                &FLUTTER,
                Some(&pinned),
                env_value.clone(),
                None,
                Some(&empty),
                vec![path.clone()]
            ),
            Some(fs::canonicalize(&pinned).expect("pinned resolves"))
        );
        assert_eq!(
            resolve_in(
                &FLUTTER,
                None,
                env_value,
                None,
                Some(&empty),
                vec![path.clone()]
            ),
            Some(fs::canonicalize(&env).expect("env resolves"))
        );
        assert_eq!(
            resolve_in(&FLUTTER, None, None, None, Some(&empty), vec![path.clone()]),
            Some(path.join("flutter"))
        );
    }

    /// An empty override is no override: the scan runs.
    #[test]
    fn empty_env_is_unset() {
        let scratch = Scratch::new("empty-env");
        let path = scratch.sdk("path");
        assert_eq!(
            resolve_in(
                &FLUTTER,
                None,
                Some(OsString::new()),
                None,
                None,
                vec![path.clone()]
            ),
            Some(path.join("flutter"))
        );
    }

    /// An override naming this very binary is refused — as the pin and as
    /// the environment value — rather than spawned into a fork bomb.
    #[test]
    fn override_naming_this_binary_is_refused() {
        let exe = std::env::current_exe().expect("test binary");
        assert_eq!(
            resolve_in(&FLUTTER, Some(&exe), None, None, None, Vec::new()),
            None
        );
        assert_eq!(
            resolve_in(&DART, None, Some(exe.into_os_string()), None, None, Vec::new()),
            None
        );
    }

    /// An override that resolves nowhere passes through verbatim: the
    /// spawn, not the lookup, reports it.
    #[test]
    fn unresolvable_override_passes_through() {
        let missing =
            std::env::temp_dir().join(format!("dartq-toolchain-missing-{}", std::process::id()));
        assert!(!missing.exists(), "test path must not exist");
        assert_eq!(
            resolve_in(&DART, Some(&missing), None, None, None, Vec::new()),
            Some(missing.clone())
        );
        assert_eq!(
            resolve_in(
                &DART,
                None,
                Some(missing.clone().into_os_string()),
                None,
                None,
                Vec::new()
            ),
            Some(missing)
        );
    }

    /// A fake home holding fvm version dirs whose `bin/<tool>` files the
    /// `.fvmrc` step resolves onto.
    struct FvmHome(PathBuf);

    impl FvmHome {
        fn new(scratch: &Scratch) -> Self {
            let home = scratch.0.join("home");
            fs::create_dir_all(&home).expect("home");
            Self(home)
        }

        /// Writes `<layout>/versions/<v>/bin/<tool>` — `layout` is `.fvm`
        /// (FVM 3) or `fvm` (pre-3.0). Returns the binary path.
        fn install(&self, layout: &str, version: &str, tool: &str) -> PathBuf {
            let bin = self
                .0
                .join(layout)
                .join("versions")
                .join(version)
                .join("bin");
            fs::create_dir_all(&bin).expect("version bin");
            let tool_path = bin.join(tool);
            fs::write(&tool_path, "").expect("version tool");
            tool_path
        }
    }

    /// The `.fvmrc` reader's verdicts: the fvm 3.x/4.x JSON shape pins its
    /// `flutter` member (surrounding whitespace tolerated), the legacy
    /// bare string still pins, and content carrying no usable version — an
    /// empty file, an empty/null/non-string `flutter`, an unrelated
    /// object — reads as a skip rather than a bogus version. A flavor
    /// declaration is its own verdict; malformed content reads the legacy
    /// way, so the install lookup (not the parser) is what rejects it.
    #[test]
    fn fvmrc_version_reads_json_and_legacy_shapes() {
        let pin = |version: &str| FvmrcPin::Pin(version.to_owned());
        assert_eq!(
            fvmrc_version("{\n  \"flutter\": \"3.32.0\"\n}\n"),
            pin("3.32.0"),
            "the real-world JSON shape pins"
        );
        assert_eq!(fvmrc_version("{\"flutter\": \" 3.32.0 \"}"), pin("3.32.0"));
        assert_eq!(fvmrc_version("3.29.3\n"), pin("3.29.3"), "legacy bare pins");
        assert_eq!(fvmrc_version(" 3.29.3 \n"), pin("3.29.3"));
        for skip in [
            "",
            " \n",
            "{}",
            "{\"flutter\": \"\"}",
            "{\"flutter\": null}",
            "{\"flutter\": 3}",
            "{\"dart\": \"3.29.3\"}",
        ] {
            assert_eq!(fvmrc_version(skip), FvmrcPin::Skip, "{skip:?} pins nothing");
        }
        assert_eq!(
            fvmrc_version("{\"flutter\": \"3.19.0\", \"flavor\": \"prod\"}"),
            FvmrcPin::Flavor("prod".to_owned())
        );
        assert_eq!(
            fvmrc_version("{\"flutter\": \"3.19.0\", \"flavor\": null}"),
            pin("3.19.0"),
            "a non-string flavor is no declaration"
        );
        assert_eq!(
            fvmrc_version("garbage\n"),
            pin("garbage"),
            "non-JSON content reads as the legacy bare pin — the install lookup rejects it"
        );
    }

    /// The `.fvmrc` step's walk and precedence: the nearest `.fvmrc` wins
    /// (a deeper dir's pin answers over an ancestor's), the version binary
    /// canonicalizes like an override, and a miss anywhere falls through to
    /// the `PATH` scan — never a refusal.
    #[test]
    fn fvmrc_pin_beats_path_first_found_wins() {
        let scratch = Scratch::new("fvmrc-walk");
        let home = FvmHome::new(&scratch);
        let pinned = home.install(".fvm", "3.29.3", "dart");
        home.install(".fvm", "3.0.0", "dart");
        let project = scratch.0.join("parent/child");
        fs::create_dir_all(&project).expect("project");
        fs::write(
            scratch.0.join("parent/.fvmrc"),
            "{\"flutter\": \"3.0.0\"}",
        )
        .expect("outer fvmrc");
        fs::write(project.join(".fvmrc"), "{\n  \"flutter\": \"3.29.3\"\n}\n")
            .expect("inner fvmrc");
        let path = scratch.sdk("path");
        assert_eq!(
            resolve_in(
                &DART,
                None,
                None,
                Some(&project),
                Some(&home.0),
                vec![path]
            ),
            Some(fs::canonicalize(&pinned).expect("pin resolves")),
            "the nearest .fvmrc pins the version, beating PATH"
        );
    }

    /// `.fvmrc` edge cases, each still falling through to `PATH`: a
    /// legacy bare-string ancestor pin answers; the real-world JSON shape
    /// answers too, from a nearer file; a JSON object with an empty
    /// `flutter` member defers to the ancestor's; an uninstalled pin is
    /// skipped the same way (stderr note aside); and no `.fvmrc` at all is
    /// the plain scan.
    #[test]
    fn fvmrc_walk_skips_empty_and_uninstalled() {
        let scratch = Scratch::new("fvmrc-skips");
        let home = FvmHome::new(&scratch);
        let pinned = home.install(".fvm", "3.29.3", "flutter");
        let canonical_pin = fs::canonicalize(&pinned).expect("pin resolves");
        let path = scratch.sdk("path");

        // A legacy bare-string ancestor pin answers from a bare cwd.
        let outer = scratch.0.join("outer");
        let child = outer.join("child");
        fs::create_dir_all(&child).expect("child");
        fs::write(outer.join(".fvmrc"), "3.29.3").expect("outer fvmrc");
        assert_eq!(
            resolve_in(&FLUTTER, None, None, Some(&child), Some(&home.0), Vec::new()),
            Some(canonical_pin.clone()),
            "the legacy bare ancestor pin resolves"
        );

        // The real-world JSON shape answers the same way.
        fs::write(child.join(".fvmrc"), "{\n  \"flutter\": \"3.29.3\"\n}\n")
            .expect("json fvmrc");
        assert_eq!(
            resolve_in(&FLUTTER, None, None, Some(&child), Some(&home.0), Vec::new()),
            Some(canonical_pin.clone()),
            "the fvm 3.x JSON pin resolves"
        );

        // An empty JSON pin defers to the ancestor's.
        fs::write(child.join(".fvmrc"), "{\"flutter\": \"\"}").expect("empty json fvmrc");
        assert_eq!(
            resolve_in(&FLUTTER, None, None, Some(&child), Some(&home.0), Vec::new()),
            Some(canonical_pin.clone()),
            "an empty JSON pin defers to the ancestor's installed pin"
        );

        // An uninstalled pin nearer the cwd defers to the ancestor's too.
        fs::write(child.join(".fvmrc"), "{\"flutter\": \"9.9.9\"}").expect("uninstalled fvmrc");
        assert_eq!(
            resolve_in(&FLUTTER, None, None, Some(&child), Some(&home.0), Vec::new()),
            Some(canonical_pin),
            "the uninstalled pin defers to the ancestor's installed pin"
        );

        // A lone uninstalled pin falls through to PATH, never an error.
        let bare = scratch.0.join("bare");
        fs::create_dir_all(&bare).expect("bare");
        fs::write(bare.join(".fvmrc"), "{\"flutter\": \"9.9.9\"}").expect("bare fvmrc");
        assert_eq!(
            resolve_in(
                &FLUTTER,
                None,
                None,
                Some(&bare),
                Some(&home.0),
                vec![path.clone()]
            ),
            Some(scratch.0.join("path/flutter")),
            "an uninstalled JSON pin falls through to PATH"
        );

        // No `.fvmrc` anywhere is the plain scan.
        let plain = scratch.0.join("plain");
        fs::create_dir_all(&plain).expect("plain");
        assert_eq!(
            resolve_in(
                &FLUTTER,
                None,
                None,
                Some(&plain),
                Some(&home.0),
                vec![path]
            ),
            Some(scratch.0.join("path/flutter"))
        );
    }

    /// A flavor declaration is a fall-through, not a pin: fvm installs a
    /// flavored version at `versions/<version>_<flavor>`, a layout dartq
    /// does not resolve — the walk notes it (the shared reader's stderr
    /// line) and keeps going: an ancestor's plain pin answers, and a lone
    /// flavor declaration falls through to `PATH`.
    #[test]
    fn fvmrc_flavor_falls_through() {
        let scratch = Scratch::new("fvmrc-flavor");
        let home = FvmHome::new(&scratch);
        let pinned = home.install(".fvm", "3.19.0", "flutter");
        let outer = scratch.0.join("outer");
        let child = outer.join("child");
        fs::create_dir_all(&child).expect("child");
        fs::write(outer.join(".fvmrc"), "{\"flutter\": \"3.19.0\"}").expect("outer fvmrc");
        let flavored = "{\"flutter\": \"3.19.0\", \"flavor\": \"prod\"}";
        fs::write(child.join(".fvmrc"), flavored).expect("flavored fvmrc");
        assert_eq!(
            resolve_in(&FLUTTER, None, None, Some(&child), Some(&home.0), Vec::new()),
            Some(fs::canonicalize(&pinned).expect("pin resolves")),
            "the flavor declaration defers to the ancestor's plain pin"
        );

        let bare = scratch.0.join("flavored-only");
        fs::create_dir_all(&bare).expect("bare");
        fs::write(bare.join(".fvmrc"), flavored).expect("lone flavored fvmrc");
        let path = scratch.sdk("flavor-path");
        assert_eq!(
            resolve_in(
                &FLUTTER,
                None,
                None,
                Some(&bare),
                Some(&home.0),
                vec![path]
            ),
            Some(scratch.0.join("flavor-path/flutter")),
            "a lone flavor declaration falls through to PATH"
        );
    }

    /// The fvm layout fact: modern `.fvm` wins over legacy `fvm` whenever
    /// both exist, each answers alone, and neither is a miss.
    #[test]
    fn fvm_version_dir_prefers_modern() {
        let scratch = Scratch::new("fvm-layout");
        let home = FvmHome::new(&scratch);
        assert_eq!(fvm_version_dir(Some(&home.0), "3.29.3"), None);
        home.install("fvm", "3.0.0", "dart");
        assert_eq!(
            fvm_version_dir(Some(&home.0), "3.0.0"),
            Some(home.0.join("fvm/versions/3.0.0")),
            "legacy alone answers"
        );
        home.install(".fvm", "3.0.0", "dart");
        assert_eq!(
            fvm_version_dir(Some(&home.0), "3.0.0"),
            Some(home.0.join(".fvm/versions/3.0.0")),
            "modern wins when both exist"
        );
        assert_eq!(fvm_version_dir(None, "3.0.0"), None, "no home is a miss");
    }

    /// The fvm default's layout mirror: the modern path is the default
    /// guess, the legacy one wins only when it alone is on disk, and no
    /// home is a miss.
    #[test]
    fn fvm_default_root_prefers_modern() {
        let scratch = Scratch::new("fvm-default");
        let home = FvmHome::new(&scratch);
        assert_eq!(
            fvm_default_root(Some(&home.0)),
            Some(home.0.join(".fvm/default"))
        );
        fs::create_dir_all(home.0.join("fvm/default")).expect("legacy default");
        assert_eq!(
            fvm_default_root(Some(&home.0)),
            Some(home.0.join("fvm/default")),
            "legacy wins when it alone is on disk"
        );
        fs::create_dir_all(home.0.join(".fvm/default")).expect("modern default");
        assert_eq!(
            fvm_default_root(Some(&home.0)),
            Some(home.0.join(".fvm/default")),
            "modern wins back once present"
        );
        assert_eq!(fvm_default_root(None), None);
    }

    /// The `dartq init` state: a `flutter` link to this very binary first on
    /// `PATH` is passed over for the real tool behind it; with nothing to
    /// skip the link is a file of the name and wins; and only the binary is
    /// excluded, so a tool beside the link is found.
    #[cfg(unix)]
    #[test]
    fn path_scan_skips_this_binary() {
        let scratch = Scratch::new("self-skip");
        let shim = scratch.0.join("shim");
        fs::create_dir_all(&shim).expect("shim dir");
        let exe = std::env::current_exe().expect("test binary");
        std::os::unix::fs::symlink(&exe, shim.join("flutter")).expect("shim link");
        let sdk = scratch.sdk("sdk");
        let this = own_binary().expect("test binary resolves");
        assert_eq!(
            scan_in([shim.clone(), sdk.clone()], "flutter", Some(&this)),
            Some(sdk.join("flutter"))
        );
        assert_eq!(
            scan_in([shim.clone(), sdk], "flutter", None),
            Some(shim.join("flutter"))
        );
        fs::write(shim.join("dart"), "").expect("tool beside the link");
        assert_eq!(
            scan_in([shim.clone()], "dart", Some(&this)),
            Some(shim.join("dart"))
        );
    }

    /// The spellings one lookup tries, in order: Windows tools ship as
    /// `.exe`/`.bat`/`.cmd` (flutter is a `.bat`, dart an `.exe`), everything
    /// else is the bare name.
    #[test]
    fn candidate_names_per_platform() {
        let table: [(&str, bool, &[&str]); 4] = [
            (
                "flutter",
                true,
                &["flutter.exe", "flutter.bat", "flutter.cmd"],
            ),
            ("dart", true, &["dart.exe", "dart.bat", "dart.cmd"]),
            ("flutter", false, &["flutter"]),
            ("dart", false, &["dart"]),
        ];
        for (name, windows, expected) in table {
            assert_eq!(
                candidate_names(name, windows),
                expected,
                "{name} windows={windows}"
            );
        }
    }
}
