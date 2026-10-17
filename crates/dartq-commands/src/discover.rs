//! The canonical, sorted, deduplicated `.dart` file-set walk shared by the
//! check and fix faces. Moved verbatim from the CLI's discovery: the
//! oracle's only built-in exclusion is the dot-basename rule, explicit file
//! arguments bypass the walk rules, and symlinked directories are never
//! descended into — see [`discover`]'s contract for the full oracle parity
//! notes.

use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use thiserror::Error;

/// Why a discovery failed: a missing path argument is the CLI's usage tier
/// (`Directory or file doesn't exist: …`, the oracle's `usageException`
/// text), an I/O failure the internal tier.
#[derive(Debug, Error)]
pub enum DiscoverError {
    /// Path argument does not exist; the message body is the oracle's
    /// `usageException` text (analyze.dart:191).
    #[error("Directory or file doesn't exist: {0}")]
    Missing(String),
    /// Walk I/O failure.
    #[error("{0}")]
    Io(#[from] io::Error),
}

/// Canonical, sorted, deduplicated `.dart` file set under `paths`. A file
/// argument is taken as-is (top-level `fs::metadata` follows links) and is
/// unfiltered except for the `.dart` extension: the oracle yields explicit
/// files without applying the walk rules (context_root.dart:71-72) but only
/// `.dart` ones reach the driver (context_manager.dart:748-754 via
/// file_paths.dart:75-78 `isDart`) — without the filter a non-Dart arg would
/// parse as Dart and emit phantom diagnostics. A directory argument is
/// walked recursively with the oracle's only built-in exclusion — the
/// dot-basename rule, checked on every child path, directories and files
/// alike (`context_root.dart:116` + 146-149). `build`/`node_modules` are
/// `analysis_options` concerns (excluded globs), not walk-level ones.
///
/// Symlinked directories are never descended into (cycle safety). This is a
/// deliberate subset of the oracle's walk, which descends symlinked
/// directories with canonical-path dedup (context_root.dart:119-129); the
/// differential harness canonicalizes both sides, so the gate never sees the
/// difference. Symlinked `.dart` files are included — the walk stats the
/// link target and accepts regular files, matching the oracle's walk, which
/// analyzes linked files too.
///
/// # Errors
///
/// [`DiscoverError::Missing`] when a path argument does not exist;
/// [`DiscoverError::Io`] when a directory walk fails.
pub fn discover(paths: &[PathBuf]) -> Result<BTreeSet<PathBuf>, DiscoverError> {
    fn walk(dir: &Path, files: &mut BTreeSet<PathBuf>) -> io::Result<()> {
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let file_type = entry.file_type()?;
            let path = entry.path();
            // Byte-prefix test: the dot rule is ASCII, and reading the raw
            // name bytes avoids a per-entry lossy-name allocation.
            if entry.file_name().as_encoded_bytes().starts_with(b".") {
                continue;
            }
            if file_type.is_dir() {
                walk(&path, files)?;
            } else if path.extension().is_some_and(|ext| ext == "dart")
                // `entry.file_type()` reports link-host facts (it does not
                // follow symlinks): a symlinked `.dart` file is neither
                // `is_dir` nor `is_file` there, yet the oracle analyzes it —
                // stat the target (`fs::metadata` follows the link); a
                // dangling link has no target and is skipped. Regular files
                // take the cached-type path without a syscall.
                && (file_type.is_file()
                    || (file_type.is_symlink()
                        && fs::metadata(&path).is_ok_and(|meta| meta.is_file())))
            {
                files.insert(path);
            }
        }
        Ok(())
    }

    let mut files = BTreeSet::new();
    for path in paths {
        let meta =
            fs::metadata(path).map_err(|_| DiscoverError::Missing(path.display().to_string()))?;
        if meta.is_file() {
            // `.dart` only (oracle isDart parity — see the fn doc).
            if path.extension().is_some_and(|ext| ext == "dart") {
                files.insert(path.clone());
            }
        } else {
            walk(path, &mut files).map_err(DiscoverError::Io)?;
        }
    }
    Ok(files)
}
