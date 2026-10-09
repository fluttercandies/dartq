//! Pub workspace layout discovery (facts §4).
//!
//! Walks up from a directory to the workspace root — the first pubspec
//! whose `resolution` is unspecified (pub's `Resolution.none`;
//! entrypoint.dart:85-153: an explicit `local`/`external` does *not* stop
//! the walk) — and collects the transitive member list in pub's
//! `transitiveWorkspace` DFS order (root first, children in declaration
//! order). Every pubspec is loaded together with its optional
//! `pubspec_overrides.yaml`, whose `workspace`/`resolution` keys take
//! precedence (pubspec.dart:87-91, 108-112). All filesystem access is
//! read-only; artifact cross-validation only reads
//! `.dart_tool/pub/workspace_ref.json`.

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

use crate::PubError;
use crate::pubspec::{Pubspec, Resolution};

/// A workspace layout: the resolution root and its member directories.
///
/// A package without a `workspace:` field is a valid layout with no
/// members (a "single-package workspace").
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkspaceLayout {
    /// Directory of the workspace root pubspec (the first one walking up
    /// with an unspecified `resolution`).
    pub root: PathBuf,
    /// Transitive member directories in DFS declaration order, excluding
    /// the root itself.
    pub members: Vec<PathBuf>,
}

/// Finds the workspace root for `from` and the transitive member set.
///
/// # Errors
///
/// Returns [`PubError::Workspace`] when no pubspec exists anywhere on the
/// walk, when every pubspec met carries an explicit `resolution` (pub's
/// "found a pubspec but no workspace root" FileException in
/// `_loadWorkspace`, entrypoint.dart:141-152), when a member pubspec is
/// missing, unparseable, or unreadable, or when the member graph is not a
/// tree — duplicate inclusion or duplicate member names, per pub's
/// `validateWorkspace` (package.dart:486-524).
pub fn find_workspace_root(from: &Path) -> Result<WorkspaceLayout, PubError> {
    let mut dir = from.to_path_buf();
    // The first pubspec met on the walk, for the no-root error.
    let mut first_met: Option<(PathBuf, Resolution)> = None;
    loop {
        let pubspec_path = dir.join("pubspec.yaml");
        if pubspec_path.is_file() {
            let spec = load_pubspec(&dir)?;
            if first_met.is_none() {
                first_met = Some((dir.clone(), spec.resolution));
            }
            // pub's root test is strictly `resolution == Resolution.none`
            // (entrypoint.dart:124) — the key's absence, nothing else.
            if spec.resolution == Resolution::Unspecified {
                let members = collect_members(&dir, &spec)?;
                return Ok(WorkspaceLayout { root: dir, members });
            }
        }
        match dir.parent() {
            Some(parent) => dir = parent.to_path_buf(),
            None => {
                return Err(match first_met {
                    None => PubError::Workspace(format!(
                        "Found no `pubspec.yaml` file in `{}` or parent directories",
                        from.display()
                    )),
                    Some((dir, resolution)) => PubError::Workspace(format!(
                        "Found a pubspec.yaml at {}. But it has resolution `{}`. \
                         But found no workspace root including it in parent directories.",
                        dir.display(),
                        resolution.as_str()
                    )),
                });
            }
        }
    }
}

/// Loads `<dir>/pubspec.yaml` with `<dir>/pubspec_overrides.yaml` applied
/// when present (pub walks with `allowOverridesFile: true`,
/// entrypoint.dart:95-101).
fn load_pubspec(dir: &Path) -> Result<Pubspec, PubError> {
    let text = read_file(&dir.join("pubspec.yaml"))?;
    let overrides_path = dir.join("pubspec_overrides.yaml");
    let overrides = if overrides_path.is_file() {
        Some(read_file(&overrides_path)?)
    } else {
        None
    };
    Pubspec::parse_with_overrides(&text, overrides.as_deref())
}

/// Member directories named by `spec.workspace`, expanded transitively:
/// DFS with a stack so that popping yields root-first, declared-order
/// traversal (package.dart:69-79). A second arrival at an already-included
/// directory — the same entry listed twice, a diamond, a cycle, or a
/// symlink alias — is a hard error, matching pub's `validateWorkspace`
/// (package.dart:486-524) rather than a silent visited-set cut; so is a
/// duplicate member name (package.dart:517-524, the root included).
///
/// Duplicate detection keys on canonicalized paths (pub canonicalizes
/// before comparing, package.dart:496,513): two lexically distinct
/// spellings that resolve to one directory collide. Discovery itself stays
/// lexical — only the inclusion map's keys are canonical.
fn collect_members(root: &Path, spec: &Pubspec) -> Result<Vec<PathBuf>, PubError> {
    let mut members = Vec::new();
    // Included member directory -> the pubspec directory that included it.
    // Keys are canonical (`inclusion_key`), values lexically normalized.
    // Seeded with the root: a member naming the root is a second arrival
    // at an already-included directory.
    let normalized_root = normalize(root);
    let mut included_from: BTreeMap<PathBuf, PathBuf> =
        BTreeMap::from([(inclusion_key(root), normalized_root)]);
    let mut names_seen: BTreeMap<String, PathBuf> =
        BTreeMap::from([(spec.name.clone(), root.to_path_buf())]);

    // Pushed in reverse so the first declared child pops first; each entry
    // carries the directory whose `workspace:` list named it.
    let mut stack: Vec<(PathBuf, PathBuf)> = spec
        .workspace
        .iter()
        .rev()
        .map(|child| (root.join(child), root.to_path_buf()))
        .collect();

    while let Some((dir, parent)) = stack.pop() {
        let key = inclusion_key(&dir);
        if let Some(previous) = included_from.get(&key).cloned() {
            return Err(if previous == normalize(&parent) {
                PubError::Workspace(format!(
                    "Packages can only be included in the workspace once: \
                     `{}` is included twice into the workspace of `{}`.",
                    dir.join("pubspec.yaml").display(),
                    parent.join("pubspec.yaml").display(),
                ))
            } else {
                PubError::Workspace(format!(
                    "Packages can only be included in the workspace once: \
                     `{}` is included in the workspace, both from `{}` and `{}`.",
                    dir.join("pubspec.yaml").display(),
                    parent.join("pubspec.yaml").display(),
                    previous.join("pubspec.yaml").display(),
                ))
            });
        }
        included_from.insert(key, normalize(&parent));

        let child_spec = load_pubspec(&dir)?;
        if let Some(previous_dir) = names_seen.get(&child_spec.name).cloned() {
            return Err(PubError::Workspace(format!(
                "Workspace members must have unique names: `{}` and `{}` are both called `{}`.",
                previous_dir.join("pubspec.yaml").display(),
                dir.join("pubspec.yaml").display(),
                child_spec.name,
            )));
        }
        names_seen.insert(child_spec.name.clone(), dir.clone());
        members.push(dir.clone());
        for grandchild in child_spec.workspace.iter().rev() {
            stack.push((dir.join(grandchild), dir.clone()));
        }
    }
    Ok(members)
}

fn read_file(path: &Path) -> Result<String, PubError> {
    std::fs::read_to_string(path)
        .map_err(|e| PubError::Workspace(format!("cannot read {}: {e}", path.display())))
}

/// `.dart_tool/pub/workspace_ref.json` is written for every workspace
/// package *including the root* (facts §4/§8.7); its `workspaceRoot` is a
/// relative path from `<member>/.dart_tool/pub` back to the root.
///
/// Returns the member directories (root included) whose ref file is
/// missing, malformed, or points somewhere else — each a staleness signal
/// for the caller, not a hard error. The check is only meaningful on a
/// layout obtained from [`find_workspace_root`]: it reads whatever sits on
/// disk under the given directories and cannot tell a fabricated layout
/// from a real one.
#[must_use]
pub fn workspace_ref_mismatches(layout: &WorkspaceLayout) -> Vec<PathBuf> {
    let mut mismatched = Vec::new();
    for dir in std::iter::once(&layout.root).chain(layout.members.iter()) {
        let ref_path = dir
            .join(".dart_tool")
            .join("pub")
            .join("workspace_ref.json");
        let points_to_root = std::fs::read_to_string(&ref_path).ok().and_then(|text| {
            let value: serde_json::Value = serde_json::from_str(&text).ok()?;
            let relative = value.get("workspaceRoot")?.as_str()?;
            Some(
                normalize(&dir.join(".dart_tool").join("pub").join(relative))
                    == normalize(&layout.root),
            )
        });
        if points_to_root != Some(true) {
            mismatched.push(dir.clone());
        }
    }
    mismatched
}

/// The duplicate-detection key for an included member directory: the
/// lexically normalized path with symlinks resolved, so symlink aliases of
/// one directory collide exactly as pub's `p.canonicalize` map key treats
/// them (package.dart:496,513). Only the inclusion map uses this —
/// discovery stays lexical. A path that cannot be canonicalized (a
/// dangling symlink) falls back to its lexical form; the subsequent
/// pubspec read fails loudly on it either way.
fn inclusion_key(path: &Path) -> PathBuf {
    let normalized = normalize(path);
    std::fs::canonicalize(&normalized).unwrap_or(normalized)
}

/// Lexically normalizes a path: collapses `.` and inner `..` components
/// without touching the filesystem, so two spellings of one directory
/// compare equal and symlinked temp dirs (/tmp -> /private/tmp) never skew
/// comparisons. A `..` pops only a plain trailing component; at a relative
/// root (or past an absolute root) it is kept verbatim, so leading `..`
/// runs survive. Windows prefixes and the absolute root pass through
/// untouched. Public because downstream identity layers must normalize
/// with the same discipline — one normalizer, one behavior.
#[must_use]
pub fn normalize(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                // Pop only a plain trailing component; `..` at a relative
                // root — or past an absolute root — stays verbatim.
                match normalized.components().next_back() {
                    Some(Component::Normal(_)) => {
                        normalized.pop();
                    }
                    _ => normalized.push(".."),
                }
            }
            other => normalized.push(other),
        }
    }
    normalized
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Creates a fresh temp directory; unique per test invocation.
    fn temp_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("dartiq-pub-{}-{}", label, std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    fn write(path: &Path, text: &str) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("mkdir");
        }
        std::fs::write(path, text).expect("write");
    }

    #[test]
    fn normalizes_lexically_without_touching_the_filesystem() {
        assert_eq!(normalize(Path::new("/a/./b//c")), PathBuf::from("/a/b/c"));
        assert_eq!(
            normalize(Path::new("/proj/.dart_tool/..")),
            PathBuf::from("/proj")
        );
        assert_eq!(normalize(Path::new("a/../b")), PathBuf::from("b"));
        // Leading `..` runs survive: they climb past a relative root.
        assert_eq!(normalize(Path::new("../up")), PathBuf::from("../up"));
        assert_eq!(normalize(Path::new("../../..")), PathBuf::from("../../.."));
    }

    #[test]
    fn single_package_layout() {
        let dir = temp_dir("single");
        write(
            &dir.join("pubspec.yaml"),
            "name: solo\nenvironment:\n  sdk: ^3.5.0\n",
        );
        let layout = find_workspace_root(&dir).expect("layout");
        assert_eq!(layout.root, dir);
        assert!(layout.members.is_empty());
        // Nested path resolves up to the same root.
        let nested = dir.join("lib").join("src");
        std::fs::create_dir_all(&nested).expect("mkdir");
        assert_eq!(find_workspace_root(&nested).expect("layout").root, dir);
    }

    #[test]
    fn workspace_root_found_through_member() {
        let root = temp_dir("ws");
        write(
            &root.join("pubspec.yaml"),
            "name: ws_root\nenvironment:\n  sdk: ^3.5.0\nworkspace:\n  - pkg_b\n  - pkg_c\n",
        );
        write(
            &root.join("pkg_b").join("pubspec.yaml"),
            "name: pkg_b\nenvironment:\n  sdk: ^3.5.0\nresolution: workspace\n",
        );
        write(
            &root.join("pkg_c").join("pubspec.yaml"),
            "name: pkg_c\nenvironment:\n  sdk: ^3.5.0\nresolution: workspace\nworkspace:\n  - nested\n",
        );
        write(
            &root.join("pkg_c").join("nested").join("pubspec.yaml"),
            "name: nested\nenvironment:\n  sdk: ^3.5.0\nresolution: workspace\n",
        );

        // Walking up from inside a member finds the root.
        let layout = find_workspace_root(&root.join("pkg_b")).expect("layout");
        assert_eq!(layout.root, root);
        // DFS order: declared order, nested members after their parent.
        assert_eq!(
            layout.members,
            vec![
                root.join("pkg_b"),
                root.join("pkg_c"),
                root.join("pkg_c").join("nested"),
            ]
        );
    }

    #[test]
    fn missing_pubspec_anywhere_is_an_error() {
        let dir = temp_dir("empty");
        assert!(matches!(
            find_workspace_root(&dir),
            Err(PubError::Workspace(_))
        ));
    }

    #[test]
    fn explicit_local_resolution_does_not_root_the_walk() {
        // A `resolution: local` package inside a workspace tree is not a
        // root: pub keeps ascending to the unspecified pubspec above it
        // (entrypoint.dart:124, `resolution == Resolution.none` only).
        let root = temp_dir("local-below");
        write(
            &root.join("pubspec.yaml"),
            "name: ws_root\nenvironment:\n  sdk: ^3.5.0\nworkspace:\n  - opted_out\n",
        );
        write(
            &root.join("opted_out").join("pubspec.yaml"),
            "name: opted_out\nenvironment:\n  sdk: ^3.5.0\nresolution: local\n",
        );
        let layout = find_workspace_root(&root.join("opted_out")).expect("layout");
        assert_eq!(layout.root, root);

        // Ascending past `local`/`external` pubspecs with no unspecified
        // pubspec anywhere is pub's "found a pubspec but no root" error,
        // not a false root at the `local` package.
        let standalone = temp_dir("local-standalone");
        write(
            &standalone.join("pubspec.yaml"),
            "name: opted_out\nenvironment:\n  sdk: ^3.5.0\nresolution: local\n",
        );
        let err = find_workspace_root(&standalone).expect_err("no root anywhere");
        assert!(
            err.to_string().contains("resolution `local`"),
            "unexpected error: {err}"
        );
        let external = temp_dir("external-standalone");
        write(
            &external.join("pubspec.yaml"),
            "name: ext\nenvironment:\n  sdk: ^3.5.0\nresolution: external\n",
        );
        let err = find_workspace_root(&external).expect_err("no root anywhere");
        assert!(
            err.to_string().contains("resolution `external`"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn overrides_file_controls_discovery() {
        // A member declared `resolution: workspace` only in
        // pubspec_overrides.yaml is, to pub, a workspace member — the walk
        // must not root there (pubspec.dart:87-91, 108-112).
        let root = temp_dir("overrides");
        write(
            &root.join("pubspec.yaml"),
            "name: ws_root\nenvironment:\n  sdk: ^3.5.0\nworkspace:\n  - pkg_b\n  - pkg_c\n",
        );
        write(
            &root.join("pkg_b").join("pubspec.yaml"),
            "name: pkg_b\nenvironment:\n  sdk: ^3.5.0\n",
        );
        write(
            &root.join("pkg_b").join("pubspec_overrides.yaml"),
            "resolution: workspace\n",
        );
        // pkg_c likewise merges to `workspace` via its overrides file.
        write(
            &root.join("pkg_c").join("pubspec.yaml"),
            "name: pkg_c\nenvironment:\n  sdk: ^3.5.0\n",
        );
        write(
            &root.join("pkg_c").join("pubspec_overrides.yaml"),
            "resolution: workspace\n",
        );

        // Walking up from pkg_b: pkg_b merges to `workspace`, pkg_c merges
        // to `workspace`, so the walk passes both and roots at `root`.
        let layout = find_workspace_root(&root.join("pkg_b")).expect("layout");
        assert_eq!(layout.root, root);
        assert_eq!(layout.members, vec![root.join("pkg_b"), root.join("pkg_c")]);

        // The member list itself honors the overrides `workspace:` key.
        let overrides_root = temp_dir("overrides-root");
        write(
            &overrides_root.join("pubspec.yaml"),
            "name: ws_root\nenvironment:\n  sdk: ^3.5.0\n",
        );
        write(
            &overrides_root.join("pubspec_overrides.yaml"),
            "workspace:\n  - member\n",
        );
        write(
            &overrides_root.join("member").join("pubspec.yaml"),
            "name: member\nenvironment:\n  sdk: ^3.5.0\n",
        );
        let layout = find_workspace_root(&overrides_root).expect("layout");
        assert_eq!(layout.root, overrides_root);
        assert_eq!(layout.members, vec![overrides_root.join("member")]);
    }

    #[test]
    fn duplicate_entry_in_one_workspace_list_is_rejected() {
        // pub: "`...` is included twice into the workspace of `...`"
        // (package.dart:498-505).
        let root = temp_dir("dup-entry");
        write(
            &root.join("pubspec.yaml"),
            "name: ws_root\nenvironment:\n  sdk: ^3.5.0\nworkspace:\n  - pkg_a\n  - pkg_a\n",
        );
        write(
            &root.join("pkg_a").join("pubspec.yaml"),
            "name: pkg_a\nenvironment:\n  sdk: ^3.5.0\nresolution: workspace\n",
        );
        let err = find_workspace_root(&root).expect_err("duplicate entry");
        assert!(
            err.to_string().contains("included twice"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn diamond_inclusion_is_rejected() {
        // The same directory reached from two parents (root -> pkg_a/pkg_b
        // and root -> pkg_a -> pkg_b): pub fails with "both from"
        // (package.dart:506-512) instead of silently collecting it once.
        let root = temp_dir("diamond");
        write(
            &root.join("pubspec.yaml"),
            "name: ws_root\nenvironment:\n  sdk: ^3.5.0\nworkspace:\n  - pkg_a\n  - pkg_a/pkg_b\n",
        );
        write(
            &root.join("pkg_a").join("pubspec.yaml"),
            "name: pkg_a\nenvironment:\n  sdk: ^3.5.0\nresolution: workspace\nworkspace:\n  - pkg_b\n",
        );
        write(
            &root.join("pkg_a").join("pkg_b").join("pubspec.yaml"),
            "name: pkg_b\nenvironment:\n  sdk: ^3.5.0\nresolution: workspace\n",
        );
        let err = find_workspace_root(&root).expect_err("diamond");
        assert!(
            err.to_string().contains("both from"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn cycle_inclusion_is_rejected() {
        // A cycle cannot be expressed through parsed pubspecs: ascent (`..`)
        // is rejected at parse time, matching pub (pubspec.dart:97-101). The
        // back edge is therefore injected into the in-memory root spec; the
        // traversal must still refuse the second arrival — the last line of
        // defense, mirroring pub's `validateWorkspace`.
        let root = temp_dir("cycle");
        write(
            &root.join("pubspec.yaml"),
            "name: ws_root\nenvironment:\n  sdk: ^3.5.0\nworkspace:\n  - pkg_a\n",
        );
        write(
            &root.join("pkg_a").join("pubspec.yaml"),
            "name: pkg_a\nenvironment:\n  sdk: ^3.5.0\nresolution: workspace\n",
        );
        let text = std::fs::read_to_string(root.join("pubspec.yaml")).expect("read");
        let mut spec = Pubspec::parse(&text).expect("parse");
        spec.workspace.push("pkg_a/..".to_owned()); // normalizes back to the root
        let err = collect_members(&root, &spec).expect_err("cycle");
        assert!(
            matches!(err, PubError::Workspace(_)),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn duplicate_member_names_are_rejected() {
        // Two members with the same name…
        let root = temp_dir("dup-names");
        write(
            &root.join("pubspec.yaml"),
            "name: ws_root\nenvironment:\n  sdk: ^3.5.0\nworkspace:\n  - one\n  - two\n",
        );
        write(
            &root.join("one").join("pubspec.yaml"),
            "name: dup\nenvironment:\n  sdk: ^3.5.0\nresolution: workspace\n",
        );
        write(
            &root.join("two").join("pubspec.yaml"),
            "name: dup\nenvironment:\n  sdk: ^3.5.0\nresolution: workspace\n",
        );
        let err = find_workspace_root(&root).expect_err("duplicate names");
        assert!(
            err.to_string().contains("unique names"),
            "unexpected error: {err}"
        );

        // …and a member named like the root (pub checks the whole
        // transitive workspace, root included; package.dart:517-524).
        let root = temp_dir("dup-root-name");
        write(
            &root.join("pubspec.yaml"),
            "name: ws_root\nenvironment:\n  sdk: ^3.5.0\nworkspace:\n  - member\n",
        );
        write(
            &root.join("member").join("pubspec.yaml"),
            "name: ws_root\nenvironment:\n  sdk: ^3.5.0\nresolution: workspace\n",
        );
        let err = find_workspace_root(&root).expect_err("root name collision");
        assert!(
            err.to_string().contains("unique names"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn symlinked_duplicate_member_dir_is_rejected() {
        // Two lexically distinct entries that resolve to one directory are
        // a duplicate inclusion: pub's validateWorkspace compares
        // canonicalized map keys (package.dart:496,513), so a symlink
        // alias cannot smuggle a member in twice.
        let root = temp_dir("symlink-dup");
        write(
            &root.join("pubspec.yaml"),
            "name: ws_root\nenvironment:\n  sdk: ^3.5.0\nworkspace:\n  - pkg_a\n  - alias\n",
        );
        write(
            &root.join("pkg_a").join("pubspec.yaml"),
            "name: pkg_a\nenvironment:\n  sdk: ^3.5.0\nresolution: workspace\n",
        );
        std::os::unix::fs::symlink(root.join("pkg_a"), root.join("alias")).expect("symlink");
        let err = find_workspace_root(&root).expect_err("symlinked duplicate");
        assert!(
            err.to_string().contains("included twice"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn broken_symlink_member_falls_back_to_lexical_key() {
        // canonicalize fails on a dangling link; the duplicate key falls
        // back to the lexical path and the pubspec read reports the real
        // problem instead.
        let root = temp_dir("symlink-broken");
        write(
            &root.join("pubspec.yaml"),
            "name: ws_root\nenvironment:\n  sdk: ^3.5.0\nworkspace:\n  - ghost\n",
        );
        std::os::unix::fs::symlink(root.join("missing"), root.join("ghost")).expect("symlink");
        let err = find_workspace_root(&root).expect_err("broken symlink member");
        assert!(
            err.to_string().contains("cannot read"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn workspace_ref_mismatch_detection() {
        let root = temp_dir("refs");
        write(
            &root.join("pubspec.yaml"),
            "name: ws_root\nenvironment:\n  sdk: ^3.5.0\nworkspace:\n  - pkg_b\n",
        );
        write(
            &root.join("pkg_b").join("pubspec.yaml"),
            "name: pkg_b\nenvironment:\n  sdk: ^3.5.0\nresolution: workspace\n",
        );

        // No refs written at all: root and member both flagged.
        let layout = find_workspace_root(&root).expect("layout");
        assert_eq!(
            workspace_ref_mismatches(&layout),
            vec![root.clone(), root.join("pkg_b")]
        );

        // Correct refs (root: "../..", member: "../../..") clear both.
        write(
            &root
                .join(".dart_tool")
                .join("pub")
                .join("workspace_ref.json"),
            r#"{"workspaceRoot":"../.."}"#,
        );
        write(
            &root
                .join("pkg_b")
                .join(".dart_tool")
                .join("pub")
                .join("workspace_ref.json"),
            r#"{"workspaceRoot":"../../.."}"#,
        );
        assert!(workspace_ref_mismatches(&layout).is_empty());

        // A wrong pointer is flagged.
        write(
            &root
                .join("pkg_b")
                .join(".dart_tool")
                .join("pub")
                .join("workspace_ref.json"),
            r#"{"workspaceRoot":"../.."}"#,
        );
        assert_eq!(workspace_ref_mismatches(&layout), vec![root.join("pkg_b")]);
    }
}
