//! Content stamps for the files a test run depends on.
//!
//! Each file gets a blake3 digest and, for Dart sources, its directive
//! [`FileFacts`]. Warm runs are stat-driven: a file whose
//! `(len, mtime, ctime, ino)` stamp matches the persisted index reuses its
//! digest and facts without being read. A stamp taken within
//! [`RACY_WINDOW`] of the file's mtime is never trusted (the "racy git"
//! problem: a same-tick rewrite keeps the stamp), so such a file is
//! re-read on the next run.
//!
//! The persisted index only carries the files the current run touched, so
//! it stays bounded by the workspace's own dependency set.

use crate::deps::{FileFacts, data_paths, facts};
use rayon::prelude::*;
use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

/// A stamp this close to the file's mtime is re-verified next time.
const RACY_WINDOW: Duration = Duration::from_secs(2);

/// Persisted per-file stamp.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Stamp {
    len: u64,
    mtime: (i64, u32),
    ctime: (i64, u32),
    ino: u64,
    /// The stamp was taken safely after the last modification.
    trusted: bool,
    digest: String,
    facts: Option<Arc<FileFacts>>,
    /// Path-shaped words of a non-Dart text file ([`data_paths`]).
    refs: Option<Arc<Vec<String>>>,
}

/// One stamped file.
#[derive(Debug, Clone)]
pub struct Entry {
    pub digest: [u8; 32],
    /// Present for `.dart` files.
    pub facts: Option<Arc<FileFacts>>,
    /// Present for other text files up to [`MAX_DATA_FILE`] bytes: the
    /// relative paths the file's contents name (a fixture manifest, an
    /// `analysis_options.yaml` `include:`), which a test reading the file
    /// may go on to read.
    pub refs: Option<Arc<Vec<String>>>,
}

/// Largest non-Dart file whose path-shaped words are extracted; a larger
/// file is data, not a manifest.
pub const MAX_DATA_FILE: u64 = 1024 * 1024;

/// The stamp index of one run: the prior run's stamps plus everything
/// stamped so far.
#[derive(Debug)]
pub struct Stamps {
    prior: FxHashMap<PathBuf, Stamp>,
    /// `None` = the file does not exist (a missing import target is an
    /// input too: creating it changes the outcome).
    current: FxHashMap<PathBuf, Option<(Entry, Stamp)>>,
    started: SystemTime,
}

impl Stamps {
    /// Load the persisted index at `path`; a missing or unreadable index
    /// starts empty (every file is read once).
    #[must_use]
    pub fn load(path: &Path) -> Self {
        let prior = std::fs::read(path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Vec<(PathBuf, Stamp)>>(&bytes).ok())
            .map(|rows| rows.into_iter().collect())
            .unwrap_or_default();
        Self {
            prior,
            current: FxHashMap::default(),
            started: SystemTime::now(),
        }
    }

    /// Stamp every path not stamped yet, in parallel.
    ///
    /// # Errors
    ///
    /// I/O failures other than a file being absent.
    pub fn stamp(&mut self, paths: Vec<PathBuf>) -> io::Result<()> {
        let fresh: Vec<PathBuf> = paths
            .into_iter()
            .filter(|path| !self.current.contains_key(path))
            .collect();
        let prior = &self.prior;
        let started = self.started;
        let stamped: Vec<(PathBuf, Option<(Entry, Stamp)>)> = fresh
            .into_par_iter()
            .map(|path| {
                let entry = stamp_file(&path, prior.get(&path), started)?;
                Ok((path, entry))
            })
            .collect::<io::Result<_>>()?;
        self.current.extend(stamped);
        Ok(())
    }

    /// The stamped entry of `path`: `None` when it was never stamped or
    /// does not exist.
    #[must_use]
    pub fn get(&self, path: &Path) -> Option<&Entry> {
        self.current.get(path)?.as_ref().map(|(entry, _)| entry)
    }

    /// True when `path` was stamped (present or absent).
    #[must_use]
    pub fn contains(&self, path: &Path) -> bool {
        self.current.contains_key(path)
    }

    /// Start a fresh stamping pass: this pass's stamps become the prior
    /// index, so a re-stamp re-checks every file against the disk.
    pub fn refresh(&mut self) {
        for (path, entry) in self.current.drain() {
            if let Some((_, stamp)) = entry {
                self.prior.insert(path, stamp);
            }
        }
        self.started = SystemTime::now();
    }

    /// Persist this run's stamps (atomically: write, then rename).
    ///
    /// # Errors
    ///
    /// I/O failures writing the index.
    pub fn save(&self, path: &Path) -> io::Result<()> {
        let mut rows: Vec<(&PathBuf, &Stamp)> = self
            .current
            .iter()
            .filter_map(|(path, entry)| entry.as_ref().map(|(_, stamp)| (path, stamp)))
            .collect();
        rows.sort_unstable_by(|a, b| a.0.cmp(b.0));
        let bytes = serde_json::to_vec(&rows).map_err(io::Error::other)?;
        write_atomic(path, &bytes)
    }
}

/// Write `bytes` to `path` through a sibling temp file and a rename, so a
/// concurrent reader sees the old or the new file, never a torn one.
///
/// # Errors
///
/// I/O failures creating the directory, writing, or renaming.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension(format!("tmp.{}", std::process::id()));
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

/// Stamp one file, reading it only when the prior stamp cannot vouch.
fn stamp_file(
    path: &Path,
    prior: Option<&Stamp>,
    started: SystemTime,
) -> io::Result<Option<(Entry, Stamp)>> {
    let meta = match std::fs::metadata(path) {
        Ok(meta) if meta.is_file() => meta,
        Ok(_) => return Ok(None),
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let (mtime, ctime, ino) = stat_fields(&meta);
    let len = meta.len();
    if let Some(old) = prior
        && old.trusted
        && (old.len, old.mtime, old.ctime, old.ino) == (len, mtime, ctime, ino)
        && let Some(digest) = parse_digest(&old.digest)
    {
        let entry = Entry {
            digest,
            facts: old.facts.clone(),
            refs: old.refs.clone(),
        };
        return Ok(Some((entry, old.clone())));
    }
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let digest = *blake3::hash(&bytes).as_bytes();
    let dart = path.extension().is_some_and(|ext| ext == "dart");
    let facts = dart.then(|| Arc::new(facts(&bytes)));
    let refs = (!dart && len <= MAX_DATA_FILE).then(|| Arc::new(data_paths(&bytes)));
    let trusted = meta
        .modified()
        .ok()
        .and_then(|modified| started.duration_since(modified).ok())
        .is_some_and(|age| age >= RACY_WINDOW);
    let stamp = Stamp {
        len,
        mtime,
        ctime,
        ino,
        trusted,
        digest: hex::encode(digest),
        facts: facts.clone(),
        refs: refs.clone(),
    };
    let entry = Entry {
        digest,
        facts,
        refs,
    };
    Ok(Some((entry, stamp)))
}

fn parse_digest(text: &str) -> Option<[u8; 32]> {
    let mut digest = [0u8; 32];
    hex::decode_to_slice(text, &mut digest).ok()?;
    Some(digest)
}

#[cfg(unix)]
fn stat_fields(meta: &std::fs::Metadata) -> ((i64, u32), (i64, u32), u64) {
    use std::os::unix::fs::MetadataExt;
    let nanos = |n: i64| u32::try_from(n).unwrap_or(0);
    (
        (meta.mtime(), nanos(meta.mtime_nsec())),
        (meta.ctime(), nanos(meta.ctime_nsec())),
        meta.ino(),
    )
}

#[cfg(not(unix))]
fn stat_fields(meta: &std::fs::Metadata) -> ((i64, u32), (i64, u32), u64) {
    let time = |t: io::Result<SystemTime>| {
        t.ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or((0, 0), |d| {
                (
                    i64::try_from(d.as_secs()).unwrap_or(i64::MAX),
                    d.subsec_nanos(),
                )
            })
    };
    (time(meta.modified()), time(meta.created()), 0)
}

/// Every regular file under `root` (canonical), sorted. Excluded: every
/// `.git/` directory (VCS state), and the root's `.dart_tool/` (toolchain
/// state, this cache included) and `build/` (build output). A nested
/// fixture package's `.dart_tool/` stays in: an analyzer-driven test reads
/// its package config. Directory
/// symlinks are followed once per canonical target, so a link cycle
/// cannot loop the walk.
///
/// # Errors
///
/// I/O failures reading a directory.
pub fn package_files(root: &Path) -> io::Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    let mut seen = rustc_hash::FxHashSet::default();
    walk(root, root, &mut out, &mut seen)?;
    out.sort_unstable();
    Ok(out)
}

fn walk(
    root: &Path,
    dir: &Path,
    out: &mut Vec<PathBuf>,
    seen: &mut rustc_hash::FxHashSet<PathBuf>,
) -> io::Result<()> {
    if !seen.insert(dir.canonicalize()?) {
        return Ok(());
    }
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        // `metadata` follows symlinks: a link is walked as its target.
        let Ok(meta) = std::fs::metadata(&path) else {
            continue; // dangling link, or removed mid-walk
        };
        if meta.is_dir() {
            let name = entry.file_name();
            let skip = name == ".git" || (dir == root && (name == ".dart_tool" || name == "build"));
            if !skip {
                walk(root, &path, out, seen)?;
            }
        } else if meta.is_file() {
            out.push(path);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn package_walk_and_stamp_reuse() {
        let tmp = tempfile::tempdir().expect("tmp");
        let root = tmp.path().canonicalize().expect("canon");
        let write = |rel: &str, body: &str| {
            let path = root.join(rel);
            std::fs::create_dir_all(path.parent().expect("parent")).expect("dir");
            std::fs::write(path, body).expect("write");
        };
        write("lib/a.dart", "import 'b.dart';\n");
        write("lib/b.dart", "class B {}\n");
        write("pubspec.yaml", "name: x\n");
        write(".dart_tool/package_config.json", "{}");
        write("test/fixture/.dart_tool/package_config.json", "{}");
        write("build/out.txt", "x");
        write("test/build/kept.txt", "x");

        let files = package_files(&root).expect("walk");
        let rels: Vec<String> = files
            .iter()
            .map(|f| f.strip_prefix(&root).expect("inside").display().to_string())
            .collect();
        assert_eq!(
            rels,
            [
                "lib/a.dart",
                "lib/b.dart",
                "pubspec.yaml",
                "test/build/kept.txt",
                "test/fixture/.dart_tool/package_config.json"
            ]
        );

        let index = root.join(".dart_tool/dartq/index.json");
        let mut stamps = Stamps::load(&index);
        let missing = root.join("lib/missing.dart");
        stamps
            .stamp(files.iter().cloned().chain([missing.clone()]).collect())
            .expect("stamp");
        let a = stamps.get(&root.join("lib/a.dart")).expect("a");
        assert_eq!(a.facts.as_ref().expect("facts").uris, ["b.dart"]);
        assert!(
            stamps
                .get(&root.join("pubspec.yaml"))
                .expect("yaml")
                .facts
                .is_none()
        );
        assert!(stamps.contains(&missing) && stamps.get(&missing).is_none());
        stamps.save(&index).expect("save");

        // Fresh files sit inside the racy window and are never trusted: a
        // same-size rewrite is still seen through the persisted index.
        let before = stamps.get(&root.join("lib/b.dart")).expect("b").digest;
        write("lib/b.dart", "class C {}\n");
        let mut again = Stamps::load(&index);
        again.stamp(vec![root.join("lib/b.dart")]).expect("stamp");
        assert_ne!(
            again.get(&root.join("lib/b.dart")).expect("b").digest,
            before
        );
    }
}
