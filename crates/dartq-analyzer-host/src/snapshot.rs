//! The freshness check of the resident server: one parallel stat walk over
//! every file an analysis can read, diffed against the previous walk.
//!
//! The server watches its roots itself, but a watcher gives no guarantee
//! at request time: an edit saved a millisecond before `dartq analyze`
//! may not have reached it. The walk is the request-time truth. It covers
//!
//! * the analysis roots, minus dot-directories (the analyzer's own
//!   exclusion, `ContextRoot._isExcluded`);
//! * every package root of the governing `package_config.json` that lives
//!   outside the pub cache and the SDK (path dependencies, workspace
//!   siblings) — an import reaches them, so their edits change results;
//! * the `package_config.json`, `analysis_options.yaml` and `pubspec.yaml`
//!   of every ancestor directory of a root, which decide the contexts.
//!
//! A snapshot is a sorted `(path, size, mtime)` vector: the diff is one
//! linear merge, and the vector costs no hash buckets.

use std::cmp::Ordering;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::time::UNIX_EPOCH;

use parking_lot::Mutex;
use rayon::prelude::*;
use rustc_hash::FxHashSet;

/// One file's stat facts.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Entry {
    path: Box<Path>,
    size: u64,
    mtime_ns: u64,
}

/// One walk's result, sorted by path.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Snapshot(Vec<Entry>);

/// What changed between two walks.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Diff {
    /// Dart files whose size or mtime changed.
    pub changed: Vec<PathBuf>,
    /// Whether the change set needs the contexts rebuilt: a file appeared
    /// or vanished, or a non-Dart input the analyzer reads changed.
    pub rebuild: bool,
}

impl Diff {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.changed.is_empty() && !self.rebuild
    }
}

/// Whether a changed non-Dart file can change analysis results: the
/// options, pubspec, package config, data-driven fix files and the
/// Android manifest the analyzer checks.
fn analyzer_input(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|ext| ext.to_str()),
        Some("yaml" | "yml" | "json" | "xml")
    )
}

fn is_dart(path: &Path) -> bool {
    path.extension().is_some_and(|ext| ext == "dart")
}

impl Snapshot {
    /// Walks `roots` (canonical files or directories) plus the package
    /// roots and ancestor configuration files described in the module
    /// docs. `sdk_home` is the installation the SDK lives in (a Flutter
    /// checkout or the Dart SDK root); its files never change under one
    /// key and are not walked.
    #[must_use]
    pub fn take(roots: &[PathBuf], packages: Option<&Path>, sdk_home: &Path) -> Self {
        let mut files = Vec::new();
        let mut walked: Vec<PathBuf> = roots.to_vec();
        let mut configs: Vec<PathBuf> = packages.map(Path::to_path_buf).into_iter().collect();
        for root in roots {
            let mut nearest = packages.is_some();
            for dir in root.ancestors() {
                for name in ["analysis_options.yaml", "pubspec.yaml"] {
                    files.push(dir.join(name));
                }
                let config = dir.join(".dart_tool").join("package_config.json");
                files.push(config.clone());
                // The analyzer resolves each file through its nearest
                // package config; only that one names reachable roots.
                if !nearest && config.is_file() {
                    nearest = true;
                    if !configs.contains(&config) {
                        configs.push(config);
                    }
                }
            }
        }
        let pub_cache = pub_cache();
        let local = |package: &Path| {
            !(package.starts_with(sdk_home)
                || pub_cache
                    .as_deref()
                    .is_some_and(|cache| package.starts_with(cache))
                || package
                    .components()
                    .any(|c| c == Component::Normal(".pub-cache".as_ref())))
        };
        for config in &configs {
            walked.extend(package_roots(config).into_iter().filter(|p| local(p)));
        }
        let visited = Mutex::new(FxHashSet::default());
        let mut entries = walk_all(walked.clone(), &visited);
        // Nested packages (a monorepo under one root) resolve through their
        // own configs: their local roots outside the walked set join it.
        let nested: Vec<PathBuf> = entries
            .iter()
            .filter(|e| e.path.ends_with(".dart_tool/package_config.json"))
            .flat_map(|e| package_roots(&e.path))
            .filter(|p| local(p) && !walked.iter().any(|outer| p.starts_with(outer)))
            .collect();
        if !nested.is_empty() {
            entries.extend(walk_all(nested, &visited));
        }
        for file in files {
            stat_into(&file, &mut entries);
        }
        entries.par_sort_unstable_by(|a, b| a.path.cmp(&b.path));
        entries.dedup_by(|a, b| a.path == b.path);
        Self(entries)
    }

    /// The change set from `self` (older) to `newer`.
    #[must_use]
    pub fn diff(&self, newer: &Self) -> Diff {
        let mut diff = Diff::default();
        let (mut old, mut new) = (self.0.iter().peekable(), newer.0.iter().peekable());
        loop {
            let order = match (old.peek(), new.peek()) {
                (None, None) => break,
                (Some(_), None) => Ordering::Less,
                (None, Some(_)) => Ordering::Greater,
                (Some(a), Some(b)) => a.path.cmp(&b.path),
            };
            match order {
                Ordering::Equal => {
                    let (a, b) = (old.next().expect("peeked"), new.next().expect("peeked"));
                    if (a.size, a.mtime_ns) != (b.size, b.mtime_ns) {
                        if is_dart(&b.path) {
                            diff.changed.push(b.path.to_path_buf());
                        } else if analyzer_input(&b.path) {
                            diff.rebuild = true;
                        }
                    }
                }
                Ordering::Less => {
                    let gone = old.next().expect("peeked");
                    diff.rebuild |= is_dart(&gone.path) || analyzer_input(&gone.path);
                }
                Ordering::Greater => {
                    let added = new.next().expect("peeked");
                    diff.rebuild |= is_dart(&added.path) || analyzer_input(&added.path);
                }
            }
        }
        diff
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// Walks `paths` (directories or files) in parallel, outermost first so a
/// nested path is never walked twice.
fn walk_all(mut paths: Vec<PathBuf>, visited: &Mutex<FxHashSet<PathBuf>>) -> Vec<Entry> {
    paths.sort();
    paths.dedup();
    let mut kept: Vec<PathBuf> = Vec::with_capacity(paths.len());
    for path in paths {
        if !kept.iter().any(|outer| path.starts_with(outer)) {
            kept.push(path);
        }
    }
    kept.par_iter()
        .flat_map_iter(|path| {
            let mut out = Vec::new();
            if path.is_dir() {
                walk(path, visited, &mut out);
            } else {
                stat_into(path, &mut out);
            }
            out
        })
        .collect()
}

/// One directory, its subdirectories in parallel. Dot-entries are skipped;
/// symlinked directories are followed once per canonical target.
fn walk(dir: &Path, visited: &Mutex<FxHashSet<PathBuf>>, out: &mut Vec<Entry>) {
    if let Ok(canonical) = fs::canonicalize(dir)
        && !visited.lock().insert(canonical)
    {
        return;
    }
    let Ok(read) = fs::read_dir(dir) else {
        return;
    };
    let mut dirs = Vec::new();
    for entry in read.flatten() {
        let name = entry.file_name();
        if name.as_encoded_bytes().first() == Some(&b'.') {
            // A nested package's resolution lives under its dot-directory.
            if name == ".dart_tool" {
                stat_into(&entry.path().join("package_config.json"), out);
            }
            continue;
        }
        let path = entry.path();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        let metadata = if kind.is_symlink() {
            fs::metadata(&path)
        } else {
            entry.metadata()
        };
        let Ok(metadata) = metadata else {
            continue;
        };
        if metadata.is_dir() {
            dirs.push(path);
        } else {
            out.push(entry_of(path, &metadata));
        }
    }
    let nested: Vec<Vec<Entry>> = dirs
        .par_iter()
        .map(|dir| {
            let mut nested = Vec::new();
            walk(dir, visited, &mut nested);
            nested
        })
        .collect();
    out.extend(nested.into_iter().flatten());
}

fn stat_into(path: &Path, out: &mut Vec<Entry>) {
    if let Ok(metadata) = fs::metadata(path)
        && metadata.is_file()
    {
        out.push(entry_of(path.to_path_buf(), &metadata));
    }
}

fn entry_of(path: PathBuf, metadata: &fs::Metadata) -> Entry {
    let mtime_ns = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |since| {
            u64::try_from(since.as_nanos()).unwrap_or(u64::MAX)
        });
    Entry {
        path: path.into_boxed_path(),
        size: metadata.len(),
        mtime_ns,
    }
}

/// The pub cache root: `$PUB_CACHE`, else `~/.pub-cache` (unix) or
/// `%LOCALAPPDATA%\Pub\Cache` (Windows).
fn pub_cache() -> Option<PathBuf> {
    if let Some(cache) = std::env::var_os("PUB_CACHE").filter(|value| !value.is_empty()) {
        return Some(PathBuf::from(cache));
    }
    if cfg!(windows) {
        return std::env::var_os("LOCALAPPDATA")
            .map(|dir| PathBuf::from(dir).join("Pub").join("Cache"));
    }
    std::env::home_dir().map(|home| home.join(".pub-cache"))
}

/// The package roots a `package_config.json` names, resolved against the
/// file's directory. An unreadable file names none.
fn package_roots(config: &Path) -> Vec<PathBuf> {
    #[derive(serde::Deserialize)]
    struct Config {
        packages: Vec<Package>,
    }
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Package {
        root_uri: String,
    }
    let Some(base) = config.parent() else {
        return Vec::new();
    };
    let Ok(config) = fs::read(config)
        .map_err(|_| ())
        .and_then(|bytes| serde_json::from_slice::<Config>(&bytes).map_err(|_| ()))
    else {
        return Vec::new();
    };
    config
        .packages
        .into_iter()
        .filter_map(|package| {
            let uri = package.root_uri;
            let path = match uri.strip_prefix("file://") {
                Some(absolute) => PathBuf::from(percent_decode(absolute)),
                None if uri.contains(':') => return None,
                None => base.join(percent_decode(&uri)),
            };
            Some(lexical(&path))
        })
        .collect()
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let Some(byte) = text
                .get(i + 1..i + 3)
                .and_then(|hex| u8::from_str_radix(hex, 16).ok())
        {
            out.push(byte);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// `..` and `.` folded without touching the file system.
fn lexical(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The diff over edits, additions, deletions, configuration changes,
    /// dot-directories and a path dependency outside the root.
    #[test]
    fn the_walk_diff_classifies_edits_additions_and_configuration() {
        let dir = tempfile::tempdir().expect("tempdir");
        let base = dir.path().canonicalize().expect("canonical");
        let app = base.join("app");
        let dep = base.join("dep");
        for path in [
            app.join("lib"),
            app.join(".dart_tool"),
            app.join(".git"),
            dep.join("lib"),
        ] {
            fs::create_dir_all(path).expect("mkdir");
        }
        fs::write(app.join("lib/a.dart"), "var a;").expect("write");
        fs::write(app.join("pubspec.yaml"), "name: app").expect("write");
        fs::write(app.join("README.md"), "x").expect("write");
        fs::write(dep.join("lib/d.dart"), "var d;").expect("write");
        fs::write(
            app.join(".dart_tool/package_config.json"),
            r#"{"configVersion":2,"packages":[{"name":"app","rootUri":"../"},{"name":"dep","rootUri":"../../dep"}]}"#,
        )
        .expect("write");
        let sdk = base.join("sdk");
        let take = || Snapshot::take(std::slice::from_ref(&app), None, &sdk);
        let first = take();
        assert!(
            first.0.iter().any(|e| &*e.path == dep.join("lib/d.dart")),
            "path dependency walked"
        );
        assert!(
            first.diff(&take()).is_empty(),
            "an unchanged tree diffs empty"
        );

        fs::write(app.join(".git/HEAD"), "ref").expect("write");
        fs::write(app.join("README.md"), "longer").expect("write");
        assert!(
            first.diff(&take()).is_empty(),
            "dot-dirs and non-inputs are ignored"
        );

        fs::write(dep.join("lib/d.dart"), "var dd;").expect("write");
        let edited = take();
        assert_eq!(
            first.diff(&edited),
            Diff {
                changed: vec![dep.join("lib/d.dart")],
                rebuild: false
            }
        );

        fs::write(app.join("lib/b.dart"), "var b;").expect("write");
        let added = take();
        assert!(edited.diff(&added).rebuild, "a new Dart file rebuilds");
        fs::remove_file(app.join("lib/b.dart")).expect("rm");
        assert!(added.diff(&take()).rebuild, "a deleted Dart file rebuilds");

        let before = take();
        fs::write(app.join("pubspec.yaml"), "name: app2").expect("write");
        assert!(before.diff(&take()).rebuild, "a pubspec change rebuilds");
    }
}
