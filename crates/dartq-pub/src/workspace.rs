//! Pub workspace layout discovery.
//!
//! Walks up from a directory to the workspace root — the first pubspec
//! whose `resolution` is unspecified (pub's `Resolution.none`;
//! entrypoint.dart:85-153: an explicit `local`/`external` does *not* stop
//! the walk) — and collects the transitive member list in pub's
//! `transitiveWorkspace` DFS order (root first, children in declaration
//! order). Glob workspace entries are expanded by filesystem listing with
//! pub's semantics (package.dart:183-238, gated on language version >= 3.11
//! via the pinned `package:glob` port in [`crate::glob`]). Every pubspec is
//! loaded together with its optional `pubspec_overrides.yaml`, whose
//! `workspace`/`resolution` keys take precedence (pubspec.dart:87-91,
//! 108-112). Members must opt in with `resolution: workspace`
//! (package.dart:241-249), overrides must be unique and may not target
//! workspace packages, and pubspecs may not sit interstitially between the
//! root and a member (package.dart:531-592). All filesystem access is
//! read-only; artifact cross-validation only reads
//! `.dart_tool/pub/workspace_ref.json`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use crate::PubError;
use crate::glob::{Glob, has_glob_wildcards};
use crate::pubspec::{Pubspec, Resolution};
use crate::version::LanguageVersion;

/// A workspace layout: the resolution root and its member directories.
///
/// A package without a `workspace:` field is a valid layout with no
/// members (a "single-package workspace").
#[derive(Clone, Debug, PartialEq)]
pub struct WorkspaceLayout {
    /// Directory of the workspace root pubspec (the first one walking up
    /// with an unspecified `resolution`).
    pub root: PathBuf,
    /// Transitive member directories in DFS declaration order, excluding
    /// the root itself.
    pub members: Vec<PathBuf>,
    /// The parsed member pubspecs, parallel to `members`: discovery loads
    /// and validates every member pubspec, so the parse is carried instead
    /// of dropped for consumers to re-read the same files.
    pub member_pubspecs: Vec<Pubspec>,
}

/// Finds the workspace root for `from` and the transitive member set.
///
/// # Errors
///
/// Returns [`PubError::NoRoot`] when no pubspec exists anywhere on the
/// walk, [`PubError::Workspace`] when the first pubspec met carries an
/// explicit `resolution` and is
/// not the root or a workspace member (pub's "found a pubspec but no
/// workspace root" FileException in `_loadWorkspace`,
/// entrypoint.dart:127-152), when a member pubspec is
/// missing, unparseable, or unreadable, when a `workspace:` entry matches
/// zero packages (package.dart:233-238) or a glob pattern fails to parse,
/// when a member has not opted in with `resolution: workspace`
/// (package.dart:241-249), or when `validateWorkspace` rejects the layout —
/// duplicate inclusion, duplicate member names, duplicate or
/// workspace-targeting overrides, or a stray interstitial pubspec
/// (package.dart:486-592).
///
/// # Panics
///
/// Panics if the root branch is reached without a previously met pubspec —
/// impossible by construction (the branch only runs after `load_pubspec`
/// succeeded, which is what sets `first_met`); the `expect` pins that
/// invariant.
pub fn find_workspace_root(from: &Path) -> Result<WorkspaceLayout, PubError> {
    // pub's own walk runs from the CLI's absolute working directory; a
    // relative spelling (#2593: the `dartq packages` default `.`) dies on
    // `Path::parent`'s empty form without ever ascending — "." has no
    // parent but "", and "" has none at all, so the ladder never reaches
    // the covering root and reports the first-met member as orphaned.
    // Walk the absolute spelling, lexically: symlink spellings of the
    // caller's path are preserved (the walk reports what it was given),
    // and a relative input anchors to the process working directory.
    let mut dir = walk_start(from);
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
                // pub accepts the root only when the first-met pubspec is
                // the root's own or a transitive workspace member's
                // (entrypoint.dart:127-137: `identical(pubspecsMet.entries
                // .first.value, package.pubspec)` — the load cache is keyed
                // on the canonicalized path, so identity means path
                // equality). Anything else keeps pub walking and ends in
                // the FileException at entrypoint.dart:143-152.
                // `first_met` is always set here: this branch only runs on
                // a met pubspec.
                let (first_dir, first_resolution) =
                    first_met.expect("first pubspec met before the root");
                let first_key = inclusion_key(&first_dir);
                if first_key != inclusion_key(&dir)
                    && !members
                        .iter()
                        .any(|(member, _)| inclusion_key(member) == first_key)
                {
                    return Err(PubError::OrphanMember(format!(
                        "Found a pubspec.yaml at {}. But it has resolution `{}`. \
                         But found no workspace root including it in parent directories.\n\n\
                         See https://dart.dev/go/pub-workspaces for more information.",
                        first_dir.display(),
                        first_resolution.as_str(),
                    )));
                }
                let (members, member_pubspecs): (Vec<PathBuf>, Vec<Pubspec>) =
                    members.into_iter().unzip();
                return Ok(WorkspaceLayout {
                    root: dir,
                    members,
                    member_pubspecs,
                });
            }
        }
        match dir.parent() {
            Some(parent) => dir = parent.to_path_buf(),
            None => {
                return Err(match first_met {
                    None => PubError::NoRoot(format!(
                        "Found no `pubspec.yaml` file in `{}` or parent directories",
                        from.display()
                    )),
                    Some((dir, resolution)) => PubError::OrphanMember(format!(
                        "Found a pubspec.yaml at {}. But it has resolution `{}`. \
                         But found no workspace root including it in parent directories.\n\n\
                         See https://dart.dev/go/pub-workspaces for more information.",
                        dir.display(),
                        resolution.as_str(),
                    )),
                });
            }
        }
    }
}

/// The absolute lexical spelling the ladder walks from: an absolute
/// input is normalized in place, a relative one anchored to the process
/// working directory (#2593 — `Path::parent` cannot ascend past `"."`).
/// Lexical only: the caller's symlink spelling survives, so the walk's
/// answers keep the spelling they were asked with.
pub(crate) fn walk_start(from: &Path) -> PathBuf {
    if from.is_absolute() {
        normalize(from)
    } else {
        normalize(&std::env::current_dir().unwrap_or_default().join(from))
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

/// Members named by `spec.workspace`, expanded transitively, each with its
/// parsed pubspec:
/// DFS with a stack so that popping yields root-first, declared-order
/// traversal (package.dart:69-79). Entries containing glob wildcards are
/// expanded by filesystem listing (pub's semantics: matches without a
/// `pubspec.yaml` are skipped, and an entry yielding zero packages is a hard
/// error — package.dart:183-238; wildcards on a pubspec below language
/// version 3.11 are matched literally, with pub's hint appended). A second
/// arrival at an already-included directory — the same entry listed twice, a
/// diamond, a cycle, or a symlink alias — is a hard error, matching pub's
/// `validateWorkspace` (package.dart:486-524) rather than a silent
/// visited-set cut: a lexically identical repeat reports pub's
/// duplicate-inclusion text, a symlink alias (whose spellings differ)
/// reports pub's duplicate-NAME error, the rejection pub itself produces
/// for that layout (package.dart:517-524); so is a duplicate member name
/// (package.dart:517-524, the
/// root included), a member without `resolution: workspace`
/// (package.dart:241-249), two members overriding the same package or an
/// override targeting a workspace package (package.dart:531-555), and a
/// pubspec sitting between the root and a member (package.dart:557-592).
///
/// Duplicate detection keys on canonicalized paths (pub canonicalizes before
/// comparing, package.dart:496,513): two lexically distinct spellings that
/// resolve to one directory collide. Discovery itself stays lexical — only
/// the inclusion map's keys are canonical.
fn collect_members(root: &Path, spec: &Pubspec) -> Result<Vec<(PathBuf, Pubspec)>, PubError> {
    let mut members = Vec::new();
    // Included member directory (canonical key) -> (the member's lexical
    // directory as first included, its package name, the including pubspec
    // directory, lexically normalized). Keys are canonical
    // (`inclusion_key`). Seeded with the root: a member naming the root is
    // a second arrival at an already-included directory.
    let normalized_root = normalize(root);
    let mut included_from: BTreeMap<PathBuf, (PathBuf, String, PathBuf)> = BTreeMap::from([(
        inclusion_key(root),
        (
            normalized_root.clone(),
            spec.name.clone(),
            normalized_root.clone(),
        ),
    )]);
    let mut names_seen: BTreeMap<String, PathBuf> =
        BTreeMap::from([(spec.name.clone(), root.to_path_buf())]);
    // Every workspace package with an overrides map, in `transitiveWorkspace`
    // order — the input of the override checks (package.dart:531-555).
    let mut loaded: Vec<(String, PathBuf, Vec<String>)> = vec![(
        spec.name.clone(),
        root.to_path_buf(),
        spec.dependency_overrides.keys().cloned().collect(),
    )];

    // Pushed in reverse so the first declared child pops first; each entry
    // carries the directory whose `workspace:` list named it. Glob entries
    // push one slot per match, in match order.
    let language_version = LanguageVersion::from_sdk_constraint(spec.environment.sdk.as_ref());
    let mut stack: Vec<(PathBuf, PathBuf)> = Vec::new();
    expand_entries(root, spec, language_version, &mut stack)?;

    while let Some((dir, parent)) = stack.pop() {
        let key = inclusion_key(&dir);
        if let Some((first_dir, first_name, previous)) = included_from.get(&key).cloned() {
            return Err(if normalize(&dir) != first_dir {
                // A lexically distinct spelling of an already-included
                // directory is a symlink alias. Pub's canonical keys are
                // purely lexical (`p.canonicalize`, `package:path`), so pub
                // never collides here — the alias is rejected by the
                // duplicate-name check instead (package.dart:517-524), and
                // this reports that error verbatim.
                PubError::Workspace(format!(
                    "Workspace members must have unique names.\n\
                     `{}` and `{}` are both called \"{}\".\n",
                    first_dir.join("pubspec.yaml").display(),
                    dir.join("pubspec.yaml").display(),
                    first_name,
                ))
            } else if previous == normalize(&parent) {
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

        let child_spec = load_pubspec(&dir)?;
        if child_spec.resolution != Resolution::Workspace {
            return Err(PubError::Workspace(format!(
                "`{}` is included in the workspace from `{}`, but does not have \
                 `resolution: workspace`. See https://dart.dev/go/pub-workspaces \
                 for more information.",
                dir.join("pubspec.yaml").display(),
                parent.join("pubspec.yaml").display(),
            )));
        }
        if let Some(previous_dir) = names_seen.get(&child_spec.name).cloned() {
            return Err(PubError::Workspace(format!(
                "Workspace members must have unique names.\n\
                 `{}` and `{}` are both called \"{}\".\n",
                previous_dir.join("pubspec.yaml").display(),
                dir.join("pubspec.yaml").display(),
                child_spec.name,
            )));
        }
        names_seen.insert(child_spec.name.clone(), dir.clone());
        included_from.insert(
            key,
            (normalize(&dir), child_spec.name.clone(), normalize(&parent)),
        );
        loaded.push((
            child_spec.name.clone(),
            dir.clone(),
            child_spec.dependency_overrides.keys().cloned().collect(),
        ));
        let child_language_version =
            LanguageVersion::from_sdk_constraint(child_spec.environment.sdk.as_ref());
        expand_entries(&dir, &child_spec, child_language_version, &mut stack)?;
        members.push((dir, child_spec));
    }

    // validateWorkspace skips the override and stray-pubspec checks on a
    // single-package workspace (package.dart:487).
    if !members.is_empty() {
        check_overrides(&loaded, &names_seen)?;
        let member_dirs: Vec<PathBuf> = members.iter().map(|(dir, _)| dir.clone()).collect();
        check_stray_pubspecs(root, &member_dirs, &included_from)?;
    }
    Ok(members)
}

/// Expands one pubspec's `workspace:` entries into (child, parent) stack
/// slots, in pop order. Glob entries (wildcard characters, language
/// version `>= 3.11`) are listed on the filesystem; both modes fail when
/// the entry yields zero packages, with pub's wording and — below 3.11 —
/// its hint (package.dart:183-238).
fn expand_entries(
    dir: &Path,
    spec: &Pubspec,
    language_version: LanguageVersion,
    stack: &mut Vec<(PathBuf, PathBuf)>,
) -> Result<(), PubError> {
    let pubspec_path = dir.join("pubspec.yaml").display().to_string();
    let mut children: Vec<(PathBuf, PathBuf)> = Vec::new();
    for entry in &spec.workspace {
        let before = children.len();
        // The glob hint is only ever set in the literal branch: the glob
        // branch implies language version >= 3.11 (package.dart:187-209).
        let mut hint = String::new();
        if has_glob_wildcards(entry)
            && language_version >= LanguageVersion::FIRST_WITH_WORKSPACE_GLOBS
        {
            let glob = Glob::new(entry).map_err(|error| {
                PubError::Workspace(format!("Failed to parse glob `{entry}`. {error}"))
            })?;
            let matches = glob.list_sync(dir).map_err(|error| {
                PubError::Workspace(format!(
                    "cannot list `{}`: {error}",
                    dir.join(entry).display()
                ))
            })?;
            for path in matches {
                // Glob matches without a pubspec.yaml are skipped
                // (package.dart:198-200).
                if path.join("pubspec.yaml").is_file() {
                    children.push((path, dir.to_path_buf()));
                }
            }
        } else {
            let child = dir.join(entry);
            if child.join("pubspec.yaml").is_file() {
                children.push((child, dir.to_path_buf()));
            } else if has_glob_wildcards(entry) {
                // Reaching the literal branch with wildcards means the
                // language version predates globs (package.dart:216-222).
                hint = format!(
                    " Glob syntax is only supported from language version {}. \
                     Consider changing the language version of `{pubspec_path}` to {}.",
                    LanguageVersion::FIRST_WITH_WORKSPACE_GLOBS,
                    LanguageVersion::FIRST_WITH_WORKSPACE_GLOBS,
                );
            }
        }
        // The zero-packages check runs for both branches
        // (package.dart:233-238): the glob branch lands here whenever no
        // match carried a pubspec.yaml, the literal branch when its
        // pubspec.yaml is missing.
        if children.len() == before {
            return Err(PubError::Workspace(format!(
                "No workspace packages matching `{entry}`. \
                 That was included in the workspace of `{pubspec_path}`.{hint}"
            )));
        }
    }
    stack.extend(children.into_iter().rev());
    Ok(())
}

/// The `validateWorkspace` override checks (package.dart:531-555): each
/// package may be overridden once across the workspace, and workspace
/// packages may not be overridden at all. `loaded` is in
/// `transitiveWorkspace` order (root first); `names_seen` maps every
/// workspace package name to its directory.
fn check_overrides(
    loaded: &[(String, PathBuf, Vec<String>)],
    names_seen: &BTreeMap<String, PathBuf>,
) -> Result<(), PubError> {
    let mut overrides_seen: BTreeMap<&str, (&str, &Path)> = BTreeMap::new();
    for (name, dir, override_names) in loaded {
        for override_name in override_names {
            if let Some((collision, collision_dir)) = overrides_seen.get(override_name.as_str()) {
                return Err(PubError::Workspace(format!(
                    "The package `{override_name}` is overridden in both: \
                     package `{collision}` at `{}` and '{name}' at `{}`. \
                     Consider removing one of the overrides.",
                    collision_dir.display(),
                    dir.display(),
                )));
            }
            overrides_seen.insert(override_name, (name.as_str(), dir));
            if let Some(overridden_dir) = names_seen.get(override_name) {
                return Err(PubError::Workspace(format!(
                    "Cannot override workspace packages. \
                     Package `{override_name}` at `{}` is overridden in `{}`.",
                    overridden_dir.display(),
                    dir.join("pubspec.yaml").display(),
                )));
            }
        }
    }
    Ok(())
}

/// The stray-pubspec check (package.dart:557-592): walking up from each
/// member, any pubspec.yaml between it and the root (or another member)
/// blocks resolution and is an error.
fn check_stray_pubspecs(
    root: &Path,
    members: &[PathBuf],
    included_from: &BTreeMap<PathBuf, (PathBuf, String, PathBuf)>,
) -> Result<(), PubError> {
    // By seeding with the root the walk never ascends past it
    // (package.dart:558-561).
    let mut visited: BTreeSet<PathBuf> = BTreeSet::from([inclusion_key(root)]);
    for member in members {
        let mut current = member.parent().map(Path::to_path_buf);
        while let Some(dir) = current {
            let key = inclusion_key(&dir);
            if included_from.contains_key(&key) {
                break; // another member reached
            }
            if !visited.insert(key) {
                break; // already inspected from an earlier member
            }
            let pubspec_candidate = dir.join("pubspec.yaml");
            if pubspec_candidate.is_file() {
                return Err(PubError::Workspace(format!(
                    "The file `{}` is located in a directory between the workspace \
                     root at `{}` and a workspace package at `{}`. But is not a member \
                     of the workspace. This blocks the resolution of the package at \
                     `{}`. Consider removing it. See \
                     https://dart.dev/go/workspaces-stray-files for details.",
                    pubspec_candidate.display(),
                    root.display(),
                    member.display(),
                    member.display(),
                )));
            }
            current = dir.parent().map(Path::to_path_buf);
        }
    }
    Ok(())
}

fn read_file(path: &Path) -> Result<String, PubError> {
    std::fs::read_to_string(path)
        .map_err(|e| PubError::Workspace(format!("cannot read {}: {e}", path.display())))
}

/// `.dart_tool/pub/workspace_ref.json` is written for every workspace
/// package *including the root* — but only for non-trivial workspaces: a
/// single-package workspace gets no ref file at all (entrypoint.dart:441),
/// so an empty `layout.members` short-circuits to "no mismatches".
///
/// Returns the member directories (root included) whose ref file is
/// missing, malformed, or points somewhere else — each a staleness signal
/// for the caller, not a hard error. The check is only meaningful on a
/// layout obtained from [`find_workspace_root`]: it reads whatever sits on
/// disk under the given directories and cannot tell a fabricated layout
/// from a real one.
#[must_use]
pub fn workspace_ref_mismatches(layout: &WorkspaceLayout) -> Vec<PathBuf> {
    if layout.members.is_empty() {
        return Vec::new();
    }
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
/// lexically normalized path with symlinks resolved. Pub's `p.canonicalize`
/// is purely lexical (normalize + absolutize, `package:path`), so its map
/// keys do not collapse symlink aliases — the same alias layout is rejected
/// there by the duplicate-*name* check instead, with different error text
/// but the same outcome. Resolving symlinks here keeps dartq's rejection at
/// the inclusion map, and a collision whose lexical spellings differ is
/// reported with that duplicate-name text (see `collect_members`); a path
/// that cannot be canonicalized (a dangling
/// symlink) falls back to its lexical form and the subsequent load fails on
/// it either way. Only the inclusion map uses this — discovery stays
/// lexical.
fn inclusion_key(path: &Path) -> PathBuf {
    let normalized = normalize(path);
    std::fs::canonicalize(&normalized).unwrap_or(normalized)
}

/// Lexically normalizes a path: collapses `.` and `..` components without
/// touching the filesystem, so two spellings of one directory compare equal
/// and symlinked temp dirs (/tmp -> /private/tmp) never skew comparisons.
/// A `..` pops a plain trailing component; on a relative path a leading `..`
/// run is kept verbatim, while `..` past an absolute root clamps at the
/// root. Windows prefixes and the absolute root pass through untouched.
/// Delegates to dartq-source's [`normalize_lexical`](dartq_source) — one
/// implementation of the discipline, shared with the FileTable rule so the
/// package universe and the file universe clean spellings identically.
///
/// The `..`-past-root discipline is the oracle's drop semantics:
/// RFC 3986 §5.2.4 `remove_dot_segments` clamps absolute paths at their root, and so
/// do Dart (`Uri.normalizePath`, `tests/corelib/uri_normalize_path_test.dart`)
/// and package:path's `Context.normalize` — pub's `p.normalize`, the only
/// path tooling its workspace walk uses (`package.dart:199`,
/// `entrypoint.dart` relative-root writes). No pub call site keeps `..`
/// past an absolute root: pubspec `workspace:` entries reject `..` ascent
/// at parse time (pubspec.dart:97-101), and `workspace_ref.json` values are
/// compared normalize-vs-normalize here, so the shared discipline is
/// self-consistent. Relative paths keep leading `..` in every discipline.
#[must_use]
pub fn normalize(path: &Path) -> PathBuf {
    dartq_source::normalize_lexical(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Creates a fresh temp directory; unique per test invocation.
    fn temp_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("dartq-pub-{}-{}", label, std::process::id()));
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
        // `..` past an absolute root clamps at the root (RFC 3986 §5.2.4,
        // `Uri.normalizePath`, package:path `Context.normalize`).
        assert_eq!(normalize(Path::new("/a/../..")), PathBuf::from("/"));
        assert_eq!(normalize(Path::new("/../a")), PathBuf::from("/a"));
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

    /// #2593: the cockpit-demo shape — the root lists BOTH an
    /// intermediate `resolution: workspace` member and a nested member
    /// directly — resolves from the nested member's directory (and from
    /// deep inside it) to the root. The intermediate member's pubspec is
    /// walked through, never rooted at.
    #[test]
    fn nested_member_under_an_intermediate_member_resolves_to_the_root() {
        let root = temp_dir("nested-under-member");
        write(
            &root.join("pubspec.yaml"),
            "name: ws_root\nenvironment:\n  sdk: ^3.5.0\nworkspace:\n  - examples/mid\n  - examples/mid/cockpit\n",
        );
        write(
            &root.join("examples").join("mid").join("pubspec.yaml"),
            "name: mid\nenvironment:\n  sdk: ^3.5.0\nresolution: workspace\n",
        );
        write(
            &root
                .join("examples")
                .join("mid")
                .join("cockpit")
                .join("pubspec.yaml"),
            "name: nested\nenvironment:\n  sdk: ^3.5.0\nresolution: workspace\n",
        );

        // From the nested member itself…
        let layout = find_workspace_root(&root.join("examples").join("mid").join("cockpit"))
            .expect("the nested member resolves");
        assert_eq!(layout.root, root);
        assert_eq!(
            layout.members,
            vec![
                root.join("examples").join("mid"),
                root.join("examples").join("mid").join("cockpit")
            ]
        );

        // …and from a subdirectory of it (no pubspec met until the member).
        std::fs::create_dir_all(
            root.join("examples")
                .join("mid")
                .join("cockpit")
                .join("lib"),
        )
        .expect("mkdir");
        let layout = find_workspace_root(
            &root
                .join("examples")
                .join("mid")
                .join("cockpit")
                .join("lib"),
        )
        .expect("the nested member's subdirectory resolves");
        assert_eq!(layout.root, root);
    }

    #[test]
    fn missing_pubspec_anywhere_is_an_error() {
        let dir = temp_dir("empty");
        assert!(matches!(
            find_workspace_root(&dir),
            Err(PubError::NoRoot(_))
        ));
    }

    #[test]
    fn member_without_workspace_resolution_is_rejected() {
        // A member listed in the workspace whose pubspec (after overrides
        // merging) does not opt in with `resolution: workspace` is pub's
        // package.dart:241-249 error.
        let root = temp_dir("local-member");
        write(
            &root.join("pubspec.yaml"),
            "name: ws_root\nenvironment:\n  sdk: ^3.5.0\nworkspace:\n  - opted_out\n",
        );
        write(
            &root.join("opted_out").join("pubspec.yaml"),
            "name: opted_out\nenvironment:\n  sdk: ^3.5.0\nresolution: local\n",
        );
        let err = find_workspace_root(&root).expect_err("local member");
        assert!(
            err.to_string()
                .contains("does not have `resolution: workspace`"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn explicit_standalone_resolution_yields_no_root() {
        // Ascending past `local`/`external` pubspecs with no unspecified
        // pubspec anywhere is pub's "found a pubspec but no root" error,
        // not a false root at the `local` package (entrypoint.dart:124,
        // `resolution == Resolution.none` only).
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
    fn first_pubspec_met_below_root_must_be_in_the_workspace() {
        // pub accepts a root only when the first-met pubspec is the root's
        // own or a member's (entrypoint.dart:127-137 `identical` check): a
        // `resolution: local` pubspec between the working dir and the root
        // ends in the FileException at entrypoint.dart:143-152.
        let root = temp_dir("first-met");
        write(
            &root.join("pubspec.yaml"),
            "name: ws_root\nenvironment:\n  sdk: ^3.5.0\nworkspace:\n  - other\n",
        );
        write(
            &root.join("other").join("pubspec.yaml"),
            "name: other\nenvironment:\n  sdk: ^3.5.0\nresolution: workspace\n",
        );
        write(
            &root.join("nested").join("pubspec.yaml"),
            "name: nested\nenvironment:\n  sdk: ^3.5.0\nresolution: local\n",
        );
        let err = find_workspace_root(&root.join("nested").join("deep"))
            .expect_err("non-member first pubspec");
        let text = err.to_string();
        assert!(
            text.contains("Found a pubspec.yaml at"),
            "unexpected error: {text}"
        );
        assert!(
            text.contains("resolution `local`"),
            "unexpected error: {text}"
        );
        assert!(
            text.contains("found no workspace root including it"),
            "unexpected error: {text}"
        );
        assert!(
            text.contains("See https://dart.dev/go/pub-workspaces for more information."),
            "unexpected error: {text}"
        );

        // A first-met pubspec that IS a member resolves to the root
        // normally.
        let layout = find_workspace_root(&root.join("other")).expect("layout");
        assert_eq!(layout.root, root);
        assert_eq!(layout.members, vec![root.join("other")]);
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

        // The member list itself honors the overrides `workspace:` key. The
        // member still has to opt in with `resolution: workspace`
        // (package.dart:241-249 applies to overrides-included members too).
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
            "name: member\nenvironment:\n  sdk: ^3.5.0\nresolution: workspace\n",
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
        let text = err.to_string();
        assert!(
            text.contains("Workspace members must have unique names."),
            "unexpected error: {text}"
        );
        assert!(
            text.contains("both called \"dup\""),
            "unexpected error: {text}"
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
        // a duplicate inclusion: dartq's canonical inclusion map collapses
        // the alias, where pub's purely lexical `p.canonicalize`
        // (`package:path`) does not — pub rejects the same layout through
        // the duplicate-NAME check (package.dart:517-524), and the error
        // text reported here is that message verbatim.
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
        let text = err.to_string();
        assert!(
            text.contains("Workspace members must have unique names."),
            "unexpected error: {text}"
        );
        assert!(
            text.contains("both called \"pkg_a\""),
            "unexpected error: {text}"
        );
    }

    #[test]
    fn dangling_symlink_member_is_a_zero_packages_error() {
        // A literal entry whose pubspec.yaml is missing — including through
        // a dangling symlink, whose canonicalize falls back to the lexical
        // key before the load fails — is pub's zero-packages error
        // (package.dart:215-238), not a read error.
        let root = temp_dir("symlink-broken");
        write(
            &root.join("pubspec.yaml"),
            "name: ws_root\nenvironment:\n  sdk: ^3.5.0\nworkspace:\n  - ghost\n",
        );
        std::os::unix::fs::symlink(root.join("missing"), root.join("ghost")).expect("symlink");
        let err = find_workspace_root(&root).expect_err("broken symlink member");
        assert!(
            err.to_string()
                .contains("No workspace packages matching `ghost`"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn glob_entries_expand_to_member_directories() {
        // package.dart:183-208: `pkgs/*` lists matches, skipping entries
        // without a pubspec.yaml.
        let root = temp_dir("glob");
        write(
            &root.join("pubspec.yaml"),
            "name: ws_root\nenvironment:\n  sdk: ^3.11.0\nworkspace:\n  - pkgs/*\n",
        );
        for name in ["b", "a"] {
            write(
                &root.join("pkgs").join(name).join("pubspec.yaml"),
                &format!("name: {name}\nenvironment:\n  sdk: ^3.5.0\nresolution: workspace\n"),
            );
        }
        write(&root.join("pkgs").join("notes.txt"), "not a package\n");
        let layout = find_workspace_root(&root).expect("layout");
        let mut members = layout.members.clone();
        members.sort();
        assert_eq!(members, vec![root.join("pkgs/a"), root.join("pkgs/b")]);

        // Options with distinct literal prefixes expand in declaration order
        // (separate tree branches); same-level options would merge into one
        // validator and list in readdir order, which has no guarantee.
        let root = temp_dir("glob-order");
        write(
            &root.join("pubspec.yaml"),
            "name: ws_root\nenvironment:\n  sdk: ^3.11.0\nworkspace:\n  - '{pkgs_b/*,pkgs_a/*}'\n",
        );
        for dir in ["pkgs_b", "pkgs_a"] {
            write(
                &root.join(dir).join("only").join("pubspec.yaml"),
                &format!("name: {dir}_only\nenvironment:\n  sdk: ^3.5.0\nresolution: workspace\n"),
            );
        }
        let layout = find_workspace_root(&root).expect("layout");
        assert_eq!(
            layout.members,
            vec![
                root.join("pkgs_b").join("only"),
                root.join("pkgs_a").join("only"),
            ]
        );
    }

    #[test]
    fn glob_entry_matching_zero_packages_is_an_error() {
        let root = temp_dir("glob-empty");
        write(
            &root.join("pubspec.yaml"),
            "name: ws_root\nenvironment:\n  sdk: ^3.11.0\nworkspace:\n  - pkgs/*\n",
        );
        let err = find_workspace_root(&root).expect_err("no matches");
        assert!(
            err.to_string()
                .contains("No workspace packages matching `pkgs/*`"),
            "unexpected error: {err}"
        );
        // A directory match without a pubspec.yaml does not count either
        // (package.dart:198-200).
        std::fs::create_dir_all(root.join("pkgs").join("empty")).expect("mkdir");
        assert!(find_workspace_root(&root).is_err());
    }

    #[test]
    fn glob_entry_on_old_language_version_is_literal_with_hint() {
        // Below 3.11 pub matches the raw path literally: a directory named
        // `*` becomes the member (pub's workspace_test "globs are not
        // resolved in older language versions").
        let root = temp_dir("glob-old");
        write(
            &root.join("pubspec.yaml"),
            "name: ws_root\nenvironment:\n  sdk: ^3.10.0\nworkspace:\n  - pkgs/*\n",
        );
        write(
            &root.join("pkgs").join("*").join("pubspec.yaml"),
            "name: starred\nenvironment:\n  sdk: ^3.5.0\nresolution: workspace\n",
        );
        let layout = find_workspace_root(&root).expect("layout");
        assert_eq!(layout.members, vec![root.join("pkgs").join("*")]);

        // Without such a directory: zero packages, with pub's hint
        // (package.dart:216-222).
        let root = temp_dir("glob-old-hint");
        write(
            &root.join("pubspec.yaml"),
            "name: ws_root\nenvironment:\n  sdk: ^3.10.0\nworkspace:\n  - pkgs/*\n",
        );
        let err = find_workspace_root(&root).expect_err("no matches");
        let text = err.to_string();
        assert!(
            text.contains("No workspace packages matching `pkgs/*`"),
            "unexpected error: {text}"
        );
        assert!(
            text.contains("Glob syntax is only supported from language version 3.11"),
            "unexpected error: {text}"
        );
        assert!(
            text.contains("Consider changing the language version"),
            "unexpected error: {text}"
        );
    }

    #[test]
    fn malformed_glob_entry_is_an_error() {
        // package.dart:186-190 wraps package:glob's scanner error verbatim.
        let root = temp_dir("glob-bad");
        write(
            &root.join("pubspec.yaml"),
            "name: ws_root\nenvironment:\n  sdk: ^3.11.0\nworkspace:\n  - 'pkgs/{*'\n",
        );
        let err = find_workspace_root(&root).expect_err("malformed glob");
        let text = err.to_string();
        assert!(
            text.contains("Failed to parse glob `pkgs/{*`"),
            "unexpected error: {text}"
        );
        assert!(
            text.contains("Error on line 1, column 8"),
            "unexpected error: {text}"
        );
    }

    #[test]
    fn glob_match_and_literal_entry_collide() {
        // A glob match and a later literal entry for the same directory are
        // two inclusions from the same parent (package.dart:498-505).
        let root = temp_dir("glob-dup");
        write(
            &root.join("pubspec.yaml"),
            "name: ws_root\nenvironment:\n  sdk: ^3.11.0\nworkspace:\n  - pkgs/*\n  - pkgs/a\n",
        );
        write(
            &root.join("pkgs").join("a").join("pubspec.yaml"),
            "name: a\nenvironment:\n  sdk: ^3.5.0\nresolution: workspace\n",
        );
        let err = find_workspace_root(&root).expect_err("duplicate inclusion");
        assert!(
            err.to_string().contains("included twice"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn duplicate_overrides_across_the_workspace_are_rejected() {
        // package.dart:531-545.
        let root = temp_dir("override-dup");
        write(
            &root.join("pubspec.yaml"),
            "name: ws_root\nenvironment:\n  sdk: ^3.5.0\nworkspace:\n  - pkg_a\n\
             dependency_overrides:\n  shared: ^1.0.0\n",
        );
        write(
            &root.join("pkg_a").join("pubspec.yaml"),
            "name: pkg_a\nenvironment:\n  sdk: ^3.5.0\nresolution: workspace\n\
             dependency_overrides:\n  shared: ^2.0.0\n",
        );
        let err = find_workspace_root(&root).expect_err("duplicate overrides");
        assert!(
            err.to_string().contains("is overridden in both"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn overriding_a_workspace_package_is_rejected() {
        // package.dart:547-553.
        let root = temp_dir("override-ws");
        write(
            &root.join("pubspec.yaml"),
            "name: ws_root\nenvironment:\n  sdk: ^3.5.0\nworkspace:\n  - pkg_a\n\
             dependency_overrides:\n  pkg_a: ^9.0.0\n",
        );
        write(
            &root.join("pkg_a").join("pubspec.yaml"),
            "name: pkg_a\nenvironment:\n  sdk: ^3.5.0\nresolution: workspace\n",
        );
        let err = find_workspace_root(&root).expect_err("workspace override");
        assert!(
            err.to_string()
                .contains("Cannot override workspace packages"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn stray_interstitial_pubspec_is_rejected() {
        // pkgs/pubspec.yaml sits between the root and the member pkgs/a
        // (package.dart:557-592).
        let root = temp_dir("stray");
        write(
            &root.join("pubspec.yaml"),
            "name: ws_root\nenvironment:\n  sdk: ^3.5.0\nworkspace:\n  - pkgs/a\n",
        );
        write(
            &root.join("pkgs").join("pubspec.yaml"),
            "name: interstitial\nenvironment:\n  sdk: ^3.5.0\n",
        );
        write(
            &root.join("pkgs").join("a").join("pubspec.yaml"),
            "name: a\nenvironment:\n  sdk: ^3.5.0\nresolution: workspace\n",
        );
        let err = find_workspace_root(&root).expect_err("stray pubspec");
        assert!(
            err.to_string()
                .contains("is located in a directory between the workspace root"),
            "unexpected error: {err}"
        );

        // When the interstitial directory is itself a member the walk stops
        // there and its pubspec is fine (package.dart:568-571).
        let root = temp_dir("stray-member");
        write(
            &root.join("pubspec.yaml"),
            "name: ws_root\nenvironment:\n  sdk: ^3.5.0\nworkspace:\n  - pkgs\n  - pkgs/a\n",
        );
        write(
            &root.join("pkgs").join("pubspec.yaml"),
            "name: pkgs\nenvironment:\n  sdk: ^3.5.0\nresolution: workspace\n",
        );
        write(
            &root.join("pkgs").join("a").join("pubspec.yaml"),
            "name: a\nenvironment:\n  sdk: ^3.5.0\nresolution: workspace\n",
        );
        let layout = find_workspace_root(&root).expect("layout");
        assert_eq!(layout.members, vec![root.join("pkgs"), root.join("pkgs/a")]);
    }

    #[test]
    fn single_package_workspace_needs_no_ref_file() {
        // pub writes no workspace_ref.json when the root has no members
        // (entrypoint.dart:441), so a missing ref is not a mismatch there.
        let dir = temp_dir("refs-single");
        write(
            &dir.join("pubspec.yaml"),
            "name: solo\nenvironment:\n  sdk: ^3.5.0\n",
        );
        let layout = find_workspace_root(&dir).expect("layout");
        assert!(layout.members.is_empty());
        assert!(workspace_ref_mismatches(&layout).is_empty());
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
