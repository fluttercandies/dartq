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
//! 3. the first executable of that name on `PATH`.
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
//! every test execution joins before it spawns those binaries.

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

// Unix-only: off unix there is no `flock`, and callers run unqueued.
#[cfg(unix)]
pub mod run_queue;

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
/// `build_cache.flutter` pin), then `$DARTQ_FLUTTER`, then the `PATH` scan.
/// `None` when nothing resolves, or — after one explanatory stderr line
/// naming both paths — when the override is this executable.
#[must_use]
pub fn find_flutter(explicit: Option<&Path>) -> Option<PathBuf> {
    resolve(&FLUTTER, explicit)
}

/// Resolves the `dart` binary a face spawns: `explicit` (no config key pins
/// dart today — faces pass `None`), then `$DARTQ_DART`, then the `PATH`
/// scan; the same `None` cases as [`find_flutter`].
#[must_use]
pub fn find_dart(explicit: Option<&Path>) -> Option<PathBuf> {
    resolve(&DART, explicit)
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
    resolve_in(tool, explicit, std::env::var_os(tool.env_var), path_dirs())
}

/// The resolution core over injected inputs (the test seam): the pin, then
/// the environment override (empty is unset), then the scan over `dirs` —
/// the overrides through the self-spawn refusal, the scan past this binary.
fn resolve_in(
    tool: &Tool,
    explicit: Option<&Path>,
    env: Option<OsString>,
    dirs: impl IntoIterator<Item = PathBuf>,
) -> Option<PathBuf> {
    let own = own_binary();
    if let Some(path) = explicit {
        return non_recursing(tool, path, own.as_deref());
    }
    if let Some(path) = env.filter(|value| !value.is_empty()) {
        return non_recursing(tool, Path::new(&path), own.as_deref());
    }
    scan_in(dirs, tool.name, own.as_deref())
}

/// A resolved-by-override path, canonicalized when possible and refused
/// when it names this executable (the fork bomb above). The refusal names
/// both the requested and the current path so the misconfiguration is
/// visible in one line. An unresolvable override (absent, dangling symlink)
/// passes through verbatim — the spawn fails with its own diagnostic later.
fn non_recursing(tool: &Tool, path: &Path, own: Option<&Path>) -> Option<PathBuf> {
    let resolved = match fs::canonicalize(path) {
        Ok(resolved) => resolved,
        Err(_) => return Some(path.to_path_buf()),
    };
    if let Some(own) = own
        && own == resolved.as_path()
    {
        eprintln!(
            "dartq: refusing to run {} at {}: it is this dartq binary itself ({}) — \
             point {} at the real SDK binary",
            tool.name,
            resolved.display(),
            own.display(),
            tool.fix
        );
        return None;
    }
    Some(resolved)
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

#[cfg(test)]
mod tests {
    use super::*;

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
        let env_value = Some(env.clone().into_os_string());
        assert_eq!(
            resolve_in(
                &FLUTTER,
                Some(&pinned),
                env_value.clone(),
                vec![path.clone()]
            ),
            Some(fs::canonicalize(&pinned).expect("pinned resolves"))
        );
        assert_eq!(
            resolve_in(&FLUTTER, None, env_value, vec![path.clone()]),
            Some(fs::canonicalize(&env).expect("env resolves"))
        );
        assert_eq!(
            resolve_in(&FLUTTER, None, None, vec![path.clone()]),
            Some(path.join("flutter"))
        );
    }

    /// An empty override is no override: the scan runs.
    #[test]
    fn empty_env_is_unset() {
        let scratch = Scratch::new("empty-env");
        let path = scratch.sdk("path");
        assert_eq!(
            resolve_in(&FLUTTER, None, Some(OsString::new()), vec![path.clone()]),
            Some(path.join("flutter"))
        );
    }

    /// An override naming this very binary is refused — as the pin and as
    /// the environment value — rather than spawned into a fork bomb.
    #[test]
    fn override_naming_this_binary_is_refused() {
        let exe = std::env::current_exe().expect("test binary");
        assert_eq!(resolve_in(&FLUTTER, Some(&exe), None, Vec::new()), None);
        assert_eq!(
            resolve_in(&DART, None, Some(exe.into_os_string()), Vec::new()),
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
            resolve_in(&DART, Some(&missing), None, Vec::new()),
            Some(missing.clone())
        );
        assert_eq!(
            resolve_in(
                &DART,
                None,
                Some(missing.clone().into_os_string()),
                Vec::new()
            ),
            Some(missing)
        );
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
