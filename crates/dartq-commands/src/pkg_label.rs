//! The output faces' file references — two grammars over one package table:
//!
//! * [`locate`] — the CLI's lon/text file locator (#2613): a file inside
//!   the analysis root is spelled relative to that root (forward slashes,
//!   no leading `./`), a file outside it (SDK, pub cache, another
//!   package's checkout) keeps its absolute spelling; the answer's
//!   envelope carries the root once (`root:<absolute path>`) so the
//!   relative spellings are unambiguous. No `package:` URIs: the locator
//!   is self-contained on paths, one spelling per file.
//! * [`pkg_label`] — the frozen reference-label grammar the exchange
//!   faces print (the index export's schema-frozen `path`/symbol
//!   columns, the daemon's tier-agnostic labels): `package:<name>/<path
//!   under the package's lib directory>` with the `|name` suffix reserved
//!   for the inline symbol references built on top of it. That grammar is
//!   a frozen contract and stays as-is; faces migrating onto the locator
//!   do so by switching their call sites, never by editing this grammar.
//!
//! Projection happens once, at the report assembly boundary; the engine's
//! and index's internal forms are unchanged.

use std::path::{Path, PathBuf};

use dartq_package::PackageTable;

/// Spells `path` through the lon/text file locator: relative to `root`
/// when the file lies inside it (the tail joined on forward slashes, so
/// the locator reads the same on every host; no leading `./`), otherwise
/// the absolute path unchanged (the SDK, the pub cache, another
/// package's checkout — everything the root does not cover). The root
/// itself strips to an empty tail and keeps the absolute spelling.
///
/// Inputs are the file table's canonical spellings and the canonical
/// analysis root (the
/// `dartq_diagnostics::FileLayoutProvider::analysis_root`
/// implementations); a non-canonical root simply fails the prefix strip
/// and degrades to absolute spellings — never a wrong relative path.
/// Non-UTF-8 paths spell lossily, the same posture as every other string
/// column.
#[must_use]
pub fn locate(root: &Path, path: &Path) -> String {
    match path.strip_prefix(root) {
        Ok(tail) if !tail.as_os_str().is_empty() => tail
            .components()
            .map(|component| component.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/"),
        _ => path.to_string_lossy().into_owned(),
    }
}

/// The deepest directory that contains every file in `files` — the check
/// face's analysis root when no target directory is known (its discovered
/// set is the only root notion that face has). Inputs are the run's
/// canonical file paths; `None` when the set is empty or the common
/// ancestor degenerates to a filesystem root (disjoint trees), where a
/// relative spelling would claim every path on the host and the absolute
/// spellings stand instead.
#[must_use]
pub fn common_root(files: &[PathBuf]) -> Option<PathBuf> {
    // Every file anchors at its parent directory; a bare-root entry (not a
    // shape any read layer produces) drops out rather than poisoning the
    // walk.
    covering(files.iter().filter_map(|file| file.parent()))
}

/// The analysis root a target-aware call site knows: the deepest directory
/// covering the discovery targets — a directory target anchors at itself,
/// a file target at its parent (the directory the run was pointed at, a
/// package root in the common CLI shape). Canonical spelling throughout
/// (`std::fs::canonicalize`, the file table's own normalization), so the
/// locator's prefix strip matches the file table byte-for-byte; a target
/// that cannot canonicalize anchors nowhere. `None` for an empty target
/// list or disjoint targets degenerating to a filesystem root — the caller
/// falls back to [`common_root`] over the file set.
#[must_use]
pub fn target_root(targets: &[PathBuf]) -> Option<PathBuf> {
    let anchors = targets.iter().filter_map(|target| {
        let canonical = target.canonicalize().ok()?;
        if canonical.is_dir() {
            Some(canonical)
        } else {
            canonical.parent().map(Path::to_path_buf)
        }
    });
    covering(anchors)
}

/// The deepest directory covering every entry of `dirs` — the shared walk
/// behind [`common_root`] and [`target_root`]. `None` when there is nothing
/// to cover or the answer degenerates to a filesystem root.
fn covering<P: AsRef<Path>>(mut dirs: impl Iterator<Item = P>) -> Option<PathBuf> {
    let mut root = dirs.next()?.as_ref().to_path_buf();
    for dir in dirs {
        let dir = dir.as_ref();
        while !dir.starts_with(&root) {
            root = root.parent()?.to_path_buf();
        }
    }
    // The degeneracy check's borrow ends with the condition's temporaries,
    // so `root` moves into the arm freely: a filesystem root claims every
    // path on the host — no root, absolute spellings stand.
    if root.parent().is_some() {
        Some(root)
    } else {
        None
    }
}

/// Projects `path` into its stable reference label (the frozen grammar —
/// see the module docs).
///
/// Attribution rides [`PackageTable::containing`] — the memoized file→
/// package walk, so repeated labels over one tree amortize to a hash hit
/// per directory — and the lib-relative tail joins on forward slashes so
/// the label reads as a package URI on every host. Inputs are the file
/// table's canonical spellings (the only shape the faces hold); a
/// non-canonical spelling degrades to the fallback rather than a
/// mislabel. Non-UTF-8 paths label lossily — the JSON string columns
/// cannot carry them raw either way.
#[must_use]
pub(crate) fn pkg_label(table: &PackageTable, path: &Path) -> String {
    if let Some(identity) = table.containing(path) {
        if let Ok(rel) = path.strip_prefix(&identity.lib_dir) {
            let rel = rel
                .components()
                .map(|component| component.as_os_str().to_string_lossy())
                .collect::<Vec<_>>()
                .join("/");
            if !rel.is_empty() {
                return format!("package:{}/{rel}", identity.name.as_str());
            }
        }
    }
    path.to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use dartq_package::PackageTable;

    use super::{common_root, locate, pkg_label};

    /// The single-package floor: `fixture` rooted at `root` — enough table
    /// for every label shape (lib hit, non-lib fallback, outside fallback).
    fn table(root: &Path) -> PackageTable {
        PackageTable::basic(root, Some(("fixture", None))).expect("table")
    }

    fn lib_file(root: &Path) -> PathBuf {
        root.join("lib/a.dart")
    }

    #[test]
    fn lib_files_project_to_the_package_form() {
        let root = Path::new("/proj");
        assert_eq!(
            pkg_label(&table(root), &lib_file(root)),
            "package:fixture/a.dart"
        );
        // Nested under lib keeps the tail.
        assert_eq!(
            pkg_label(&table(root), &root.join("lib/src/deep/b.dart")),
            "package:fixture/src/deep/b.dart"
        );
    }

    #[test]
    fn non_lib_and_foreign_files_keep_the_canonical_path() {
        let root = Path::new("/proj");
        let plain = |path: &Path| pkg_label(&table(root), path);
        // Under the package but outside lib: no `package:` URI addresses it.
        assert_eq!(
            plain(&root.join("test/a_test.dart")),
            "/proj/test/a_test.dart"
        );
        // Outside every package root: the bare-file fallback.
        assert_eq!(
            plain(Path::new("/elsewhere/lib/c.dart")),
            "/elsewhere/lib/c.dart"
        );
    }

    #[test]
    fn the_package_directory_itself_is_not_a_lib_label() {
        // A degenerate strip (the path equals the lib dir, or is the
        // package root) yields no empty tail: the canonical path rides.
        let root = Path::new("/proj");
        let plain = |path: &Path| pkg_label(&table(root), path);
        assert_eq!(plain(&root.join("lib")), "/proj/lib");
        assert_eq!(plain(root), "/proj");
    }

    /// The locator's policy: inside the root → the forward-slash tail with
    /// no leading `./`; outside → the absolute spelling unchanged; the
    /// root itself → absolute.
    #[test]
    fn locate_spells_inside_relative_and_outside_absolute() {
        let root = Path::new("/proj/pkg");
        assert_eq!(locate(root, &root.join("lib/src/deep/a.dart")), "lib/src/deep/a.dart");
        assert_eq!(locate(root, &root.join("main.dart")), "main.dart");
        // No `package:` form, no leading `./`: a plain tail only.
        assert_eq!(locate(root, &root.join("lib/a.dart")), "lib/a.dart");
        // Outside the root: SDK, pub cache, sibling checkouts.
        assert_eq!(
            locate(root, Path::new("/pub-cache/hosted/x/lib/a.dart")),
            "/pub-cache/hosted/x/lib/a.dart"
        );
        assert_eq!(locate(root, Path::new("/proj/other/lib/a.dart")), "/proj/other/lib/a.dart");
        // The root itself strips to an empty tail: absolute.
        assert_eq!(locate(root, root), "/proj/pkg");
    }

    /// `common_root`: the deepest directory covering every file; a
    /// filesystem root or an empty set is no root at all.
    #[test]
    fn common_root_is_the_deepest_covering_directory() {
        let files = |names: &[&str]| {
            names
                .iter()
                .map(|name| PathBuf::from(format!("/w/pkg/{name}")))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            common_root(&files(&["lib/a.dart", "lib/src/b.dart"])),
            Some(PathBuf::from("/w/pkg/lib"))
        );
        assert_eq!(
            common_root(&files(&["lib/a.dart", "test/b.dart"])),
            Some(PathBuf::from("/w/pkg"))
        );
        assert_eq!(common_root(&files(&["main.dart"])), Some(PathBuf::from("/w/pkg")));
        assert_eq!(common_root(&[]), None);
        // Disjoint trees degrade to the filesystem root: no root.
        assert_eq!(
            common_root(&[PathBuf::from("/a/x.dart"), PathBuf::from("/b/y.dart")]),
            None
        );
    }
}
