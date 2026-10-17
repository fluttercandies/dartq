//! The stable package-form file reference shared by the output faces —
//! the one implementation of the reference-label grammar, consumed by the
//! query, explain, and impact shapers so the faces cannot drift.
//!
//! The grammar: every file reference is `package:<name>/<path under the
//! package's lib directory>` — the same spelling universe the resolution
//! layer's `package:` table addresses — with the `|name` suffix reserved
//! for the inline symbol references built on top of it (the impact
//! report's `changed[].symbol`). Projection happens once, at the report
//! assembly boundary; the engine's and index's internal forms are
//! unchanged. Files that attribute to no package, or that live outside a
//! package's `lib` tree (`test/`, `bin/`, `tool/` — content no `package:`
//! URI addresses), keep the canonical path: the contract defines no
//! fallback form, so the canonical path is the identity fallback, and the
//! sort orders over rows keep keying on canonical paths (stable identity)
//! while the printed labels are projected.

use std::path::Path;

use dartq_package::PackageTable;

/// Projects `path` into its stable reference label (see the module docs).
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

    use super::pkg_label;

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
}
