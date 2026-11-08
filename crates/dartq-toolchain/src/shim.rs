//! The `dart` PATH shim: the opt-in `~/.dartq/bin/dart` link and the
//! passthrough spawn for every invocation that is not `dart analyze`.
//!
//! The flutter shim's sibling ([`crate::run_queue`] is the queue both
//! faces share): `dartq shim install dart` links `~/.dartq/bin/dart` at
//! this executable, and a bare `dart …` on `PATH` then lands here. The
//! dispatch rule is deliberately narrow — only `dart analyze` routes
//! through dartq (the run queue plus the analyzer RSS budget); every
//! other dart subcommand execs the real SDK `dart` found past the shim,
//! argv verbatim. Installing is always explicit: nothing here runs
//! without `dartq shim install dart`.

use std::ffi::{OsStr, OsString};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Whether this process was invoked as the `dart` shim: argv0's basename
/// is `dart` (the PATH symlink's name) or `dart.exe` (the Windows
/// spelling), or `DARTQ_SHIM` is `dart` (the explicit spelling for
/// wrappers that rename argv0 — the flutter shim claims every other
/// non-empty value, so `dart` is the one disambiguating spelling).
#[must_use]
pub fn is_dart_shim_invocation(argv0: Option<&OsStr>) -> bool {
    if std::env::var_os("DARTQ_SHIM").is_some_and(|value| value == "dart") {
        return true;
    }
    argv0
        .and_then(|argv0| Path::new(argv0).file_name())
        .is_some_and(|basename| {
            basename == OsStr::new("dart") || basename == OsStr::new("dart.exe")
        })
}

/// What `dartq shim install dart|remove dart` found and did — the dart
/// twin of the flutter face's report.
#[derive(Debug, Clone)]
pub struct DartShimReport {
    /// The `~/.dartq/bin/dart` path in question.
    pub shim_path: PathBuf,
    /// What the face found and did.
    pub outcome: DartShimOutcome,
    /// The exact `export PATH=...` line to print when the shim's bin
    /// directory is not on PATH (`install` only).
    pub path_missing: Option<String>,
}

/// What installing or removing the `~/.dartq/bin/dart` shim did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DartShimOutcome {
    /// The link was created (or a stale one re-aimed at this executable).
    Installed,
    /// A link to this very executable was already in place, untouched.
    AlreadyPresent,
    /// `remove` only: the link was removed.
    Removed,
    /// `remove` only: nothing was installed at the path.
    NotInstalled,
    /// A real file (never installed by dartq) occupies the path; it was
    /// left untouched.
    Blocked,
}

/// Why a dart-shim setup face could not finish.
#[derive(Debug)]
pub enum DartShimError {
    Io(io::Error),
    NoHome,
}

impl std::fmt::Display for DartShimError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(f, "io: {error}"),
            Self::NoHome => {
                write!(f, "no home directory: the shim installs into ~/.dartq/bin")
            }
        }
    }
}

impl std::error::Error for DartShimError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::NoHome => None,
        }
    }
}

impl From<io::Error> for DartShimError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

/// `dartq shim install dart`: links `~/.dartq/bin/dart` at this
/// executable — the same opt-in interception the flutter shim offers,
/// for the analyze face. Idempotent; a real file is never clobbered; no
/// shell rc file is ever touched.
///
/// # Errors
///
/// [`DartShimError::NoHome`] with no home directory,
/// [`DartShimError::Io`] when the link cannot be created.
pub fn dart_shim_install() -> Result<DartShimReport, DartShimError> {
    let home = std::env::home_dir().ok_or(DartShimError::NoHome)?;
    let exe = std::env::current_exe()?;
    let (outcome, shim_path) = dart_shim_install_in(&home, &exe)?;
    let path_missing = path_advice(
        shim_path.parent().expect("the shim lives in ~/.dartq/bin"),
        std::env::var_os("PATH"),
    );
    Ok(DartShimReport {
        shim_path,
        outcome,
        path_missing,
    })
}

/// The install core over injected home and executable (the testable
/// seam) — the flutter shim's own rules, name `dart`.
fn dart_shim_install_in(
    home: &Path,
    exe: &Path,
) -> Result<(DartShimOutcome, PathBuf), DartShimError> {
    let bin_dir = home.join(".dartq").join("bin");
    fs::create_dir_all(&bin_dir)?;
    let shim = bin_dir.join("dart");
    match fs::symlink_metadata(&shim) {
        Ok(meta)
            if meta.file_type().is_symlink()
                && fs::read_link(&shim).is_ok_and(|target| target == exe) =>
        {
            Ok((DartShimOutcome::AlreadyPresent, shim))
        }
        Ok(meta) if meta.file_type().is_symlink() => {
            fs::remove_file(&shim)?;
            link_at(exe, &shim)?;
            Ok((DartShimOutcome::Installed, shim))
        }
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            link_at(exe, &shim)?;
            Ok((DartShimOutcome::Installed, shim))
        }
        _ => Ok((DartShimOutcome::Blocked, shim)),
    }
}

/// `dartq shim remove dart`: removes `~/.dartq/bin/dart` when it is the
/// installed link — the only thing dartq ever puts there.
///
/// # Errors
///
/// As [`dart_shim_install`].
pub fn dart_shim_remove() -> Result<DartShimReport, DartShimError> {
    let home = std::env::home_dir().ok_or(DartShimError::NoHome)?;
    let (outcome, shim_path) = dart_shim_remove_at(&home)?;
    Ok(DartShimReport {
        shim_path,
        outcome,
        path_missing: None,
    })
}

/// The removal core over an injected home (the testable seam).
fn dart_shim_remove_at(home: &Path) -> Result<(DartShimOutcome, PathBuf), DartShimError> {
    let shim = home.join(".dartq").join("bin").join("dart");
    match fs::symlink_metadata(&shim) {
        Ok(meta) if meta.file_type().is_symlink() => {
            fs::remove_file(&shim)?;
            Ok((DartShimOutcome::Removed, shim))
        }
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            Ok((DartShimOutcome::NotInstalled, shim))
        }
        _ => Ok((DartShimOutcome::Blocked, shim)),
    }
}

/// Links `exe` at `target`: a symlink on unix; a file copy elsewhere
/// (symlink creation needs privileges on Windows).
fn link_at(exe: &Path, target: &Path) -> Result<(), DartShimError> {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(exe, target)?;
    }
    #[cfg(not(unix))]
    {
        fs::copy(exe, target)?;
    }
    Ok(())
}

/// The `export PATH=...` line to print when `bin_dir` is not on the
/// given PATH (exact directory match). `None` when it already is.
#[must_use]
fn path_advice(bin_dir: &Path, path_env: Option<OsString>) -> Option<String> {
    let on_path =
        path_env.is_some_and(|path| std::env::split_paths(&path).any(|dir| dir == bin_dir));
    (!on_path).then(|| format!("export PATH=\"{}:$PATH\"", bin_dir.display()))
}

/// The dart shim's passthrough: every non-`analyze` invocation execs the
/// real SDK `dart` — resolved past this shim's own link — with the
/// original argv verbatim and the shim tag stripped, so a nested dartq
/// never mistakes itself for a shim. `127` when no dart resolves.
#[must_use]
pub fn dart_passthrough(argv: &[OsString]) -> i32 {
    // The resolution skips this executable: the shim's own `dart` link
    // resolves to dartq, so the PATH scan lands on the real SDK dart.
    let Some(dart) = crate::find_dart(None) else {
        eprintln!(
            "dartq: no dart executable found outside this shim — install the Dart SDK or remove the shim"
        );
        return 127;
    };
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        // A successful exec never returns; the Err it returns IS the
        // exec failure.
        let error = Command::new(&dart)
            .args(argv)
            .current_dir(&cwd)
            .env_remove("DARTQ_SHIM")
            .exec();
        eprintln!("dartq: cannot run {}: {error}", dart.display());
        127
    }
    #[cfg(not(unix))]
    {
        match Command::new(&dart)
            .args(argv)
            .current_dir(&cwd)
            .env_remove("DARTQ_SHIM")
            .status()
        {
            Ok(status) => status.code().unwrap_or(1),
            Err(error) => {
                eprintln!("dartq: cannot run {}: {error}", dart.display());
                127
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scratch directory under the system temp root, removed on drop.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir()
                .join(format!("dartq-toolchain-shim-{tag}-{}", std::process::id()));
            fs::create_dir_all(&dir).expect("scratch dir");
            Self(dir)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn argv0_dispatch_decision_for_dart() {
        assert!(is_dart_shim_invocation(Some(OsStr::new("dart"))));
        assert!(is_dart_shim_invocation(Some(OsStr::new(
            "/opt/dartq-shims/dart"
        ))));
        assert!(is_dart_shim_invocation(Some(OsStr::new("dart.exe"))));
        assert!(!is_dart_shim_invocation(Some(OsStr::new("dartq"))));
        assert!(!is_dart_shim_invocation(Some(OsStr::new(
            "/usr/local/bin/flutter"
        ))));
        assert!(!is_dart_shim_invocation(Some(OsStr::new("dart.bak"))));
        assert!(!is_dart_shim_invocation(None));
    }

    /// The install lifecycle mirrors the flutter shim's: fresh →
    /// Installed; same executable again → AlreadyPresent; a stale link is
    /// refreshed; a real file is never clobbered; remove takes down the
    /// link only.
    #[test]
    fn dart_shim_install_and_remove_never_touch_a_real_file() {
        let scratch = Scratch::new("lifecycle");
        let home = scratch.0.join("home");
        fs::create_dir_all(&home).expect("mkdir home");
        let exe = scratch.0.join("bin").join("dartq");
        fs::create_dir_all(exe.parent().expect("parent")).expect("mkdir");
        fs::write(&exe, b"dartq binary bytes").expect("exe");
        let shim = home.join(".dartq").join("bin").join("dart");

        let (outcome, path) = dart_shim_install_in(&home, &exe).expect("install");
        assert_eq!(outcome, DartShimOutcome::Installed);
        assert_eq!(path, shim);
        #[cfg(unix)]
        assert_eq!(fs::read_link(&shim).expect("symlink"), exe);

        let (outcome, _) = dart_shim_install_in(&home, &exe).expect("install again");
        assert_eq!(outcome, DartShimOutcome::AlreadyPresent);

        let moved = scratch.0.join("bin").join("dartq-moved");
        fs::rename(&exe, &moved).expect("move");
        let (outcome, _) = dart_shim_install_in(&home, &moved).expect("refresh");
        assert_eq!(outcome, DartShimOutcome::Installed);
        #[cfg(unix)]
        assert_eq!(fs::read_link(&shim).expect("symlink"), moved);

        let (outcome, _) = dart_shim_remove_at(&home).expect("remove");
        assert_eq!(outcome, DartShimOutcome::Removed);
        assert!(!shim.exists());
        let (outcome, _) = dart_shim_remove_at(&home).expect("remove again");
        assert_eq!(outcome, DartShimOutcome::NotInstalled);

        fs::create_dir_all(shim.parent().expect("bin")).expect("bin");
        fs::write(&shim, b"a real dart").expect("real file");
        let (outcome, _) = dart_shim_install_in(&home, &moved).expect("install blocked");
        assert_eq!(outcome, DartShimOutcome::Blocked);
        let (outcome, _) = dart_shim_remove_at(&home).expect("remove blocked");
        assert_eq!(outcome, DartShimOutcome::Blocked);
        assert_eq!(fs::read(&shim).expect("untouched"), b"a real dart".to_vec());
    }
}
