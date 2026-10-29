//! Universe discovery — the pubspec-declared package boundary and the
//! work-point inventory of a tree.
//!
//! A *universe* is pub's resolution unit: a pubspec-declared workspace
//! root (whose `workspace:` list covers its members) or one independent
//! package. The boundary is purely declarative — a directory carrying a
//! `pubspec.yaml`. `.dart_tool/` never defines it: the resolved-state
//! artifacts are a fast path only ([`enclosing_universe`] consults
//! `workspace_ref.json` as a hint and verifies it against pubspecs), so a
//! fresh clone is fully discoverable before `pub get` ever runs.
//!
//! [`enclosing_universe`] is the walk-up ladder: pub's own root walk
//! ([`find_workspace_root`]) once a pubspec is met, capped below that by
//! the nearest `.git` marker — a directory with no pubspec of its own
//! that carries `.git` is a foreign tree (a nested checkout), and the
//! ladder never resolves files below it against a pubspec above it.
//!
//! [`scan`] is the walk-down face: the work points of a tree — every
//! package root, nested independent packages, and the pruned boundaries
//! (nested checkouts, hidden and excluded trees) each registered as an
//! entry so nothing silently disappears. The descent is level-parallel
//! (rayon), never follows symlinked directories, and keeps a visited set
//! over canonical paths — with symlinks out of the picture the remaining
//! cycle shapes (bind mounts, FUSE loops) revisit a path already walked,
//! which the set cuts.

use std::fs;
use std::path::{Path, PathBuf};

use rayon::prelude::*;
use rustc_hash::{FxHashMap, FxHashSet};

use crate::PubError;
use crate::config::PackageConfig;
use crate::glob::Glob;
use crate::pubspec::{DependencySpec, Pubspec};
use crate::workspace::{WorkspaceLayout, find_workspace_root, normalize};

/// The enclosing universe of a directory: the resolution unit its files
/// belong to. `layout` is `None` for the foreign-tree floor — a `.git`
/// boundary with no pubspec inside — where files resolve against nothing.
#[derive(Clone, Debug, PartialEq)]
pub struct Universe {
    /// The universe root: the workspace root pubspec's directory, the
    /// independent package's directory, or the `.git`-capped floor.
    pub root: PathBuf,
    /// The pub resolution layout, when a pubspec unit exists.
    pub layout: Option<WorkspaceLayout>,
}

/// Finds the universe enclosing `from` — the walk-up ladder.
///
/// The fast path first: when `from` is itself a package root holding
/// `.dart_tool/pub/workspace_ref.json`, the ref names the workspace root
/// candidate; it is accepted only after pub's own walk verifies it (the
/// ref is resolved state, never the boundary — a stale or fabricated ref
/// falls through to the declarative walk, byte-identical to running
/// without it). The declarative walk then climbs from `from`: the first
/// pubspec met roots through [`find_workspace_root`] (pub semantics
/// verbatim, `resolution: workspace` members included through their
/// covering root); before any pubspec is met, a `.git` marker caps the
/// ladder — a nested checkout shadows the outer tree's pubspecs.
///
/// # Errors
///
/// [`PubError::NoRoot`] when neither a pubspec nor a `.git` marker exists
/// on the whole climb — the caller's navigation case.
///
/// The returned root is always absolute: a relative invocation (`dartq
/// packages` from the package directory, `from == "."`) would otherwise
/// carry pub's relative spelling into the scan, where lexical
/// normalization collapses `"."` to `""` and every child walk reads from
/// nowhere.
pub fn enclosing_universe(from: &Path) -> Result<Universe, PubError> {
    // #2593: the ladder walks the absolute spelling. A relative `from`
    // (the `dartq packages` default `.`) breaks twice — the workspace-ref
    // fast path normalizes `./.dart_tool/pub/../../..` against nothing
    // (collapsing to the invocation directory itself), and the
    // declarative climb dies on `Path::parent(".") == ""` without ever
    // reaching the covering root.
    let anchored = crate::workspace::walk_start(from);
    let from = anchored.as_path();
    // The fast path: `from` is a package root whose resolved state names
    // its workspace. Verified membership: the ref must point at a real
    // root whose layout covers `from`.
    let reference = from
        .join(".dart_tool")
        .join("pub")
        .join("workspace_ref.json");
    if let Ok(text) = fs::read_to_string(&reference)
        && let Some(relative) = serde_json::from_str::<serde_json::Value>(&text)
            .ok()
            .and_then(|value| value.get("workspaceRoot")?.as_str().map(str::to_owned))
    {
        let candidate = normalize(
            &reference
                .parent()
                .expect("file has a parent")
                .join(&relative),
        );
        if candidate.join("pubspec.yaml").is_file()
            && let Ok(layout) = find_workspace_root(&candidate)
            && normalize(&layout.root) == candidate
            && std::iter::once(&layout.root)
                .chain(layout.members.iter())
                .any(|dir| from.starts_with(dir))
        {
            return Ok(Universe {
                root: absolute(&candidate),
                layout: Some(layout),
            });
        }
    }
    let mut dir = from;
    loop {
        if dir.join("pubspec.yaml").is_file() {
            let layout = find_workspace_root(dir)?;
            let root = absolute(&layout.root);
            return Ok(Universe {
                root,
                layout: Some(layout),
            });
        }
        if dir.join(".git").exists() {
            return Ok(Universe {
                root: absolute(dir),
                layout: None,
            });
        }
        dir = dir.parent().ok_or_else(|| {
            PubError::NoRoot(format!(
                "no pubspec.yaml or .git marker in `{}` or parent directories",
                from.display()
            ))
        })?;
    }
}

/// The absolute spelling of a path that exists: canonicalized, or — if
/// it races away between the ladder's probe and now — absolutized
/// against the current directory and lexically normalized (an absolute
/// right-hand side replaces the join, so the fallback is identity for
/// absolute inputs).
fn absolute(path: &Path) -> PathBuf {
    std::fs::canonicalize(path)
        .unwrap_or_else(|_| normalize(&std::env::current_dir().unwrap_or_default().join(path)))
}

/// One discovered work point — a directory that is a package root for
/// some resolution.
#[derive(Clone, Debug, PartialEq)]
pub struct PackageEntry {
    /// The package root directory (normalized).
    pub dir: PathBuf,
    /// The pubspec `name`; `None` when the pubspec could not be read.
    pub name: Option<String>,
    pub kind: PackageKind,
    /// The raw `environment.sdk` constraint text as written.
    pub sdk: Option<String>,
    /// A usable `package_config.json` resolves this package: it exists at
    /// the resolution root, parses, and carries an entry whose root is
    /// this directory. `false` on fresh clones — the navigation case.
    pub resolved: bool,
    /// Why the entry is degraded (a pubspec that failed to parse, an
    /// orphan workspace member); the navigation text.
    pub note: Option<String>,
}

/// How a work point relates to its resolution.
#[derive(Clone, Debug, PartialEq)]
pub enum PackageKind {
    /// A pubspec with `resolution` unspecified whose workspace covers the
    /// members listed with it.
    WorkspaceRoot,
    /// A pubspec with `resolution: workspace`, resolving through `root`.
    WorkspaceMember { root: PathBuf },
    /// A pubspec outside every workspace — its own resolution unit.
    Independent,
    /// A `path:` dependency target of `consumer`, outside the scanned
    /// tree: not discovered by the walk, but a work point the consumer's
    /// resolution reaches.
    PathDependency { consumer: PathBuf },
}

impl PackageKind {
    /// The row spelling (`workspace-root` | `workspace-member` |
    /// `independent` | `path-dependency`).
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::WorkspaceRoot => "workspace-root",
            Self::WorkspaceMember { .. } => "workspace-member",
            Self::Independent => "independent",
            Self::PathDependency { .. } => "path-dependency",
        }
    }
}

/// One pruned boundary of the walk, registered so nothing silently
/// disappears from the inventory.
#[derive(Clone, Debug, PartialEq)]
pub struct BoundaryEntry {
    /// The pruned directory (normalized).
    pub dir: PathBuf,
    pub reason: BoundaryReason,
    /// One-line context (the pubspec a hidden tree contains, the dangling
    /// `gitdir:` target of a stale worktree pointer).
    pub detail: Option<String>,
}

/// Why the walk pruned a tree.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BoundaryReason {
    /// A nested `.git` directory — a foreign checkout below the scan
    /// root; its own tree is scanned by pointing the scan at it.
    NestedCheckout,
    /// A `.git` *file* — a linked worktree pointer (`gitdir: <path>`);
    /// the detail says when the target is gone (stale, safe to delete).
    WorktreePointer,
    /// A dot-prefixed directory that contains a pubspec.
    HiddenTree,
    /// A directory an exclude glob pruned that contains a pubspec.
    ExcludedTree,
    /// A `build` output directory that contains a pubspec.
    BuildTree,
    /// A symlinked directory that directly contains a pubspec (the walk
    /// never follows symlinks; cycle safety).
    SymlinkDir,
    /// A directory the walk could not read.
    ScanError,
}

impl BoundaryReason {
    /// The row spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NestedCheckout => "nested-checkout",
            Self::WorktreePointer => "worktree-pointer",
            Self::HiddenTree => "hidden-tree",
            Self::ExcludedTree => "excluded-tree",
            Self::BuildTree => "build-tree",
            Self::SymlinkDir => "symlink-dir",
            Self::ScanError => "scan-error",
        }
    }
}

/// The walk-down knobs: include limits which packages are *listed* (the
/// walk still discovers every universe, so members resolve), exclude
/// prunes trees from the walk. Patterns are `package:glob` relative
/// POSIX paths against the scan root; empty sets discover everything.
#[derive(Debug, Default)]
pub struct ScanOptions {
    pub include: Vec<Glob>,
    pub exclude: Vec<Glob>,
}

/// The inventory of one tree.
#[derive(Clone, Debug, Default)]
pub struct ScanReport {
    /// The scan root.
    pub root: PathBuf,
    /// Work points, sorted by directory.
    pub packages: Vec<PackageEntry>,
    /// Pruned boundaries, sorted by directory.
    pub boundaries: Vec<BoundaryEntry>,
}

/// One working package entry: the report row plus the parsed pubspec
/// the post-passes consume (path dependencies, resolution).
struct Found {
    entry: PackageEntry,
    pubspec: Option<Pubspec>,
}

/// The deepest probe into a pruned tree looking for a hidden pubspec.
const PROBE_DEPTH: usize = 3;
/// The widest probe (directories visited) into a pruned tree.
const PROBE_DIRS: usize = 64;

/// Walks `root`'s tree and inventories every work point and boundary.
/// Read errors degrade into boundary entries — a tree with unreadable
/// corners still reports what it could see.
#[must_use]
pub fn scan(root: &Path, options: &ScanOptions) -> ScanReport {
    let root = normalize(root);
    let mut boundaries: Vec<BoundaryEntry> = Vec::new();
    let mut found: Vec<Found> = Vec::new();
    let mut registered: FxHashSet<PathBuf> = FxHashSet::default();
    let mut visited: FxHashSet<PathBuf> = FxHashSet::from_iter(std::iter::once(root.clone()));

    // The root itself is classified like any directory (a pubspec at the
    // scan root is the common entry), but never pruned for carrying .git.
    if root.join("pubspec.yaml").is_file() {
        if let Ok(root_found) = classify(&root) {
            register(&mut found, &mut registered, root_found);
        }
    }

    let mut level = child_dirs(&root, &root, options, &mut boundaries);
    level.retain(|dir| visited.insert(dir.clone()));
    level.sort();
    while !level.is_empty() {
        // One parallel pass per level: each directory classifies its
        // pubspec universe (if any) and prunes its children. The serial
        // merge below deduplicates — two branches reaching one workspace
        // through different members register it once — and applies the
        // visited-set cycle cut to the merged children.
        let findings: Vec<(Vec<Found>, Vec<PathBuf>, Vec<BoundaryEntry>)> = level
            .par_iter()
            .map(|dir| {
                let mut local_boundaries = Vec::new();
                let children = child_dirs(dir, &root, options, &mut local_boundaries);
                let dirs = classify(dir).unwrap_or_default();
                (dirs, children, local_boundaries)
            })
            .collect();
        let mut children: Vec<PathBuf> = Vec::new();
        for (dirs, mut next, mut local_boundaries) in findings {
            register(&mut found, &mut registered, dirs);
            boundaries.append(&mut local_boundaries);
            children.append(&mut next);
        }
        level = children
            .into_iter()
            .filter(|dir| visited.insert(dir.clone()))
            .collect();
        level.sort();
    }
    let deps = path_dependencies(&found, &mut registered);
    found.extend(deps);
    mark_resolved(&mut found);
    let mut report = ScanReport {
        root: root.clone(),
        ..ScanReport::default()
    };
    if !options.include.is_empty() {
        found.retain(|one| {
            relative_posix(&root, &one.entry.dir)
                .is_some_and(|rel| options.include.iter().any(|glob| glob.matches_path(&rel)))
        });
    }
    found.sort_by(|a, b| a.entry.dir.cmp(&b.entry.dir));
    boundaries.sort_by(|a, b| a.dir.cmp(&b.dir));
    report.packages = found.into_iter().map(|one| one.entry).collect();
    report.boundaries = boundaries;
    report
}

/// Adds one classified universe (root + members) to the working set,
/// skipping directories already registered.
fn register(found: &mut Vec<Found>, registered: &mut FxHashSet<PathBuf>, mut dirs: Vec<Found>) {
    for one in dirs.drain(..) {
        if registered.insert(one.entry.dir.clone()) {
            found.push(one);
        }
    }
}

/// Classifies one directory: when it carries a `pubspec.yaml`, its whole
/// universe (the root and every member, pub semantics through
/// [`find_workspace_root`]); an empty vector otherwise (the caller still
/// descends into it looking for nested trees). `Err` is an unreadable
/// pubspec (mapped by the caller to a skip).
fn classify(dir: &Path) -> std::io::Result<Vec<Found>> {
    let text = match fs::read_to_string(dir.join("pubspec.yaml")) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let parsed = Pubspec::parse(&text);
    let mut dirs: Vec<Found> = Vec::new();
    match find_workspace_root(dir) {
        Ok(layout) => {
            // `member_pubspecs` parallels `members` (the layout's own
            // contract): each member's parsed pubspec rides with it for
            // the path-dependency pass; the display fields come from the
            // member's own pubspec via `package_entry`.
            for (member, spec) in layout.members.iter().zip(&layout.member_pubspecs) {
                let entry = package_entry(
                    member,
                    &PackageKind::WorkspaceMember {
                        root: normalize(&layout.root),
                    },
                );
                dirs.push(Found {
                    entry,
                    pubspec: Some(spec.clone()),
                });
            }
            // A layout without members is one independent package
            // (pub's single-package workspace) — the inventory's
            // `independent`, not a degenerate workspace root.
            let root_kind = if layout.members.is_empty() {
                PackageKind::Independent
            } else {
                PackageKind::WorkspaceRoot
            };
            dirs.push(Found {
                entry: package_entry(&layout.root, &root_kind),
                pubspec: root_pubspec(&layout.root),
            });
        }
        Err(error) => {
            // The pubspec exists but no workspace accepts it: an orphan
            // member, a parse failure, a stray. It stays a work point —
            // an independent unit with the refusal as its note.
            let mut entry = package_entry(dir, &PackageKind::Independent);
            entry.note = Some(error.to_string());
            dirs.push(Found {
                entry,
                pubspec: parsed.ok(),
            });
        }
    }
    Ok(dirs)
}

/// Builds one package entry from a directory's own pubspec text: name
/// and raw SDK constraint for display.
fn package_entry(dir: &Path, kind: &PackageKind) -> PackageEntry {
    let dir = normalize(dir);
    let (name, sdk) = display_fields(&dir);
    PackageEntry {
        dir,
        name,
        kind: kind.clone(),
        sdk,
        resolved: false,
        note: None,
    }
}

/// The workspace root's pubspec for the path-dependency pass (the layout
/// carries member pubspecs, not the root's own).
fn root_pubspec(root: &Path) -> Option<Pubspec> {
    let text = fs::read_to_string(root.join("pubspec.yaml")).ok()?;
    Pubspec::parse(&text).ok()
}

/// The `(name, raw sdk constraint)` a directory's pubspec declares;
/// `(None, None)` when it cannot be read.
fn display_fields(dir: &Path) -> (Option<String>, Option<String>) {
    let Ok(text) = fs::read_to_string(dir.join("pubspec.yaml")) else {
        return (None, None);
    };
    let Ok(value) = serde_yaml::from_str::<serde_yaml::Value>(&text) else {
        return (None, None);
    };
    let name = value
        .get("name")
        .and_then(serde_yaml::Value::as_str)
        .map(str::to_owned);
    let sdk = value
        .get("environment")
        .and_then(|env| env.get("sdk"))
        .and_then(serde_yaml::Value::as_str)
        .map(str::to_owned);
    (name, sdk)
}

/// The descentable child directories of `dir`, one boundary entry per
/// pruned tree (only when something package-like was hidden — a probe
/// bounded by [`PROBE_DEPTH`] and [`PROBE_DIRS`] looks for a pubspec).
fn child_dirs(
    dir: &Path,
    root: &Path,
    options: &ScanOptions,
    boundaries: &mut Vec<BoundaryEntry>,
) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(dir) else {
        boundaries.push(BoundaryEntry {
            dir: normalize(dir),
            reason: BoundaryReason::ScanError,
            detail: None,
        });
        return Vec::new();
    };
    let mut children = Vec::new();
    for entry in entries.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if name.starts_with('.') {
            if name == ".git" {
                continue; // the parent's marker, not a tree to probe
            }
            probe_boundary(&path, BoundaryReason::HiddenTree, boundaries);
            continue;
        }
        if file_type.is_symlink() {
            if path.join("pubspec.yaml").is_file() {
                boundaries.push(BoundaryEntry {
                    dir: normalize(&path),
                    reason: BoundaryReason::SymlinkDir,
                    detail: None,
                });
            }
            continue;
        }
        if !file_type.is_dir() {
            continue;
        }
        // A nested checkout declares itself with `.git` — a directory
        // (a clone) or a file (a linked worktree pointer).
        let git = path.join(".git");
        if let Ok(meta) = fs::symlink_metadata(&git) {
            if meta.is_dir() {
                boundaries.push(BoundaryEntry {
                    dir: normalize(&path),
                    reason: BoundaryReason::NestedCheckout,
                    detail: None,
                });
                continue;
            }
            if meta.is_file() {
                boundaries.push(BoundaryEntry {
                    dir: normalize(&path),
                    reason: BoundaryReason::WorktreePointer,
                    detail: stale_gitdir(&git),
                });
                continue;
            }
        }
        if name == "build" {
            probe_boundary(&path, BoundaryReason::BuildTree, boundaries);
            continue;
        }
        if let Some(rel) = relative_posix(root, &path)
            && options.exclude.iter().any(|glob| glob.matches_path(&rel))
        {
            probe_boundary(&path, BoundaryReason::ExcludedTree, boundaries);
            continue;
        }
        children.push(path);
    }
    children.sort();
    children
}

/// Registers a pruned tree as a boundary when it hides a pubspec (the
/// probe's find), so an excluded or hidden package tree never silently
/// disappears from the inventory.
fn probe_boundary(path: &Path, reason: BoundaryReason, boundaries: &mut Vec<BoundaryEntry>) {
    if let Some(hidden) = probe_pubspec(path) {
        boundaries.push(BoundaryEntry {
            dir: normalize(path),
            reason,
            detail: Some(format!("contains {}", hidden.display())),
        });
    }
}

/// The first `pubspec.yaml` under `dir`, within the depth and width
/// bounds — `None` when the pruned tree hides nothing package-like.
fn probe_pubspec(dir: &Path) -> Option<PathBuf> {
    let mut level = vec![dir.to_path_buf()];
    for _ in 0..PROBE_DEPTH {
        let mut next = Vec::new();
        for current in &level {
            let mut entries: Vec<_> = fs::read_dir(current).ok()?.flatten().collect();
            entries.sort_by_key(|entry| entry.file_name());
            for entry in entries {
                let path = entry.path();
                if path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with('.'))
                {
                    continue;
                }
                if path.join("pubspec.yaml").is_file() {
                    return Some(path.join("pubspec.yaml"));
                }
                if entry.file_type().is_ok_and(|t| t.is_dir()) {
                    next.push(path);
                    if next.len() + level.len() > PROBE_DIRS {
                        return None;
                    }
                }
            }
        }
        if next.is_empty() {
            return None;
        }
        level = next;
    }
    None
}

/// Reads a linked worktree's `.git` pointer file: the detail marks a
/// dangling `gitdir:` target (a stale worktree, safe to delete) and an
/// unparseable file stays plain.
fn stale_gitdir(git_file: &Path) -> Option<String> {
    let text = fs::read_to_string(git_file).ok()?;
    let target = text
        .lines()
        .find_map(|line| line.trim().strip_prefix("gitdir:"))
        .map(str::trim)?;
    if Path::new(target).exists() {
        Some(format!("gitdir: {target}"))
    } else {
        Some(format!(
            "gitdir: {target} (missing — stale worktree file, safe to delete)"
        ))
    }
}

/// `path` as a POSIX-relative string against `root` (the glob
/// vocabulary); `None` when `path` is not under `root`.
fn relative_posix(root: &Path, path: &Path) -> Option<String> {
    let rel = path.strip_prefix(root).ok()?;
    let mut out = String::new();
    for component in rel.iter() {
        let part = component.to_str()?;
        if !out.is_empty() {
            out.push('/');
        }
        out.push_str(part);
    }
    (!out.is_empty()).then_some(out)
}

/// The `path:` dependency targets that lie outside the scanned tree —
/// work points the walk cannot reach but the consumer's resolution does.
fn path_dependencies(found: &[Found], registered: &mut FxHashSet<PathBuf>) -> Vec<Found> {
    let mut out: Vec<Found> = Vec::new();
    for one in found {
        let Some(spec) = &one.pubspec else {
            continue;
        };
        for section in [&spec.dependencies, &spec.dev_dependencies] {
            for dependency in section.values() {
                let DependencySpec::Path { path } = dependency else {
                    continue;
                };
                let target = normalize(&one.entry.dir.join(path));
                if !target.join("pubspec.yaml").is_file() || !registered.insert(target.clone()) {
                    continue;
                }
                let mut entry = package_entry(
                    &target,
                    &PackageKind::PathDependency {
                        consumer: one.entry.dir.clone(),
                    },
                );
                if let Ok(text) = fs::read_to_string(target.join("pubspec.yaml"))
                    && let Ok(dep_spec) = Pubspec::parse(&text)
                {
                    entry.name = Some(dep_spec.name);
                }
                out.push(Found {
                    entry,
                    pubspec: None,
                });
            }
        }
    }
    out
}

/// Marks every entry's `resolved` bit: a usable `package_config.json` at
/// the entry's resolution root (the workspace root for members, the
/// package itself otherwise, the consumer's root for path dependencies)
/// that carries an entry whose resolved root is this directory.
fn mark_resolved(found: &mut [Found]) {
    fn resolution_root<'a>(one: &'a Found, found: &'a [Found]) -> PathBuf {
        match &one.entry.kind {
            PackageKind::WorkspaceMember { root } => root.clone(),
            PackageKind::PathDependency { consumer } => found
                .iter()
                .find(|other| &other.entry.dir == consumer)
                .map_or_else(
                    || consumer.clone(),
                    |other| match &other.entry.kind {
                        PackageKind::WorkspaceMember { root } => root.clone(),
                        _ => other.entry.dir.clone(),
                    },
                ),
            _ => one.entry.dir.clone(),
        }
    }
    let mut roots: Vec<PathBuf> = found
        .iter()
        .map(|one| resolution_root(one, found))
        .collect();
    roots.sort();
    roots.dedup();
    let configs: FxHashMap<&Path, Option<PackageConfig>> = roots
        .par_iter()
        .map(|root| {
            let config = fs::read_to_string(root.join(".dart_tool").join("package_config.json"))
                .ok()
                .and_then(|text| PackageConfig::parse(&text).ok());
            (root.as_path(), config)
        })
        .collect();
    let roots: Vec<PathBuf> = found
        .iter()
        .map(|one| resolution_root(one, found))
        .collect();
    for (one, root) in found.iter_mut().zip(&roots) {
        let Some(config) = configs.get(root.as_path()).and_then(Option::as_ref) else {
            continue;
        };
        let Some(name) = &one.entry.name else {
            continue;
        };
        let dart_tool = root.join(".dart_tool");
        one.entry.resolved = config
            .entry(name)
            .and_then(|entry| entry.resolve_root(&dart_tool).ok())
            .is_some_and(|resolved| normalize(&resolved) == one.entry.dir);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh temp directory per test invocation.
    fn temp_dir(label: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("dartq-universe-{}-{}", label, std::process::id()));
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

    fn write_config(root: &Path, names: &[(&Path, &str)]) {
        // A package_config.json at `root` whose entries resolve to each
        // (dir, name) pair; rootUri is relative to root/.dart_tool.
        let entries: Vec<String> = names
            .iter()
            .map(|(dir, name)| {
                let rel = pathdiff_rel(&root.join(".dart_tool"), dir);
                format!(
                    r#"{{"name":"{name}","rootUri":"{rel}","packageUri":"lib/","languageVersion":"3.5"}}"#
                )
            })
            .collect();
        write(
            &root.join(".dart_tool").join("package_config.json"),
            &format!(
                r#"{{"configVersion":2,"packages":[{}],"generator":"pub"}}"#,
                entries.join(",")
            ),
        );
    }

    /// `to` relative to `from`, POSIX separators (test-side helper for
    /// rootUri values).
    fn pathdiff_rel(from: &Path, to: &Path) -> String {
        let from: Vec<_> = from.components().collect();
        let to: Vec<_> = to.components().collect();
        let common = from.iter().zip(&to).take_while(|(a, b)| a == b).count();
        let mut parts: Vec<String> = vec!["..".into(); from.len() - common];
        parts.extend(
            to[common..]
                .iter()
                .map(|c| c.as_os_str().to_string_lossy().into_owned()),
        );
        parts.join("/")
    }

    fn options(exclude: &[&str]) -> ScanOptions {
        ScanOptions {
            include: Vec::new(),
            exclude: exclude
                .iter()
                .map(|p| Glob::new(p).expect("glob"))
                .collect(),
        }
    }

    fn package<'a>(report: &'a ScanReport, dir: &Path) -> &'a PackageEntry {
        report
            .packages
            .iter()
            .find(|entry| entry.dir == normalize(dir))
            .unwrap_or_else(|| panic!("package {} in report", dir.display()))
    }

    /// The ladder: a member resolves to its covering root; the ref fast
    /// path verifies; a stale ref falls through to the same answer.
    #[test]
    fn ladder_roots_members_through_the_covering_pubspec() {
        let root = temp_dir("ladder");
        write(
            &root.join("pubspec.yaml"),
            "name: ws\nenvironment:\n  sdk: ^3.5.0\nworkspace:\n  - pkg_a\n",
        );
        write(
            &root.join("pkg_a").join("pubspec.yaml"),
            "name: pkg_a\nenvironment:\n  sdk: ^3.5.0\nresolution: workspace\n",
        );
        let from = root.join("pkg_a");
        let universe = enclosing_universe(&from).expect("universe");
        assert_eq!(universe.root, root.canonicalize().expect("canonical"));
        assert_eq!(universe.layout.expect("layout").members, [from.clone()]);

        let canonical = root.canonicalize().expect("canonical");
        // The resolved-state fast path: a ref naming the true root.
        write(
            &from
                .join(".dart_tool")
                .join("pub")
                .join("workspace_ref.json"),
            r#"{"workspaceRoot":"../.."}"#,
        );
        assert_eq!(enclosing_universe(&from).expect("universe").root, canonical);

        // A ref naming anywhere else is ignored — pubspecs decide.
        write(
            &from
                .join(".dart_tool")
                .join("pub")
                .join("workspace_ref.json"),
            r#"{"workspaceRoot":"../../pkg_a"}"#,
        );
        assert_eq!(enclosing_universe(&from).expect("universe").root, canonical);
    }

    /// The ladder never crosses into a foreign tree above: a nested
    /// checkout (`.git`, no pubspec) shadows the outer pubspec.
    #[test]
    fn ladder_stops_at_the_git_foreign_tree() {
        let root = temp_dir("foreign");
        write(
            &root.join("pubspec.yaml"),
            "name: outer\nenvironment:\n  sdk: ^3.5.0\n",
        );
        let inner = root.join("vendor").join("inner");
        std::fs::create_dir_all(inner.join("lib")).expect("mkdir");
        std::fs::create_dir_all(inner.join(".git")).expect("mkdir");

        let universe = enclosing_universe(&inner.join("lib")).expect("capped universe");
        assert_eq!(universe.root, inner.canonicalize().expect("canonical"));
        assert!(
            universe.layout.is_none(),
            "no pubspec inside the foreign tree"
        );
    }

    /// Neither pubspec nor `.git` anywhere: the navigation error.
    #[test]
    fn ladder_without_any_boundary_is_no_root() {
        let dir = temp_dir("bare");
        assert!(matches!(enclosing_universe(&dir), Err(PubError::NoRoot(_))));
    }

    /// A relative invocation (`from == "."`, the bare `dartq packages`
    /// case) must still yield an absolute universe root: pub's own walk
    /// answers in the caller's spelling, and lexical normalization
    /// collapses `"."` to `""` — the scan would then read children from
    /// nowhere and fabricate scan-error boundaries.
    /// #2593's live repro shape: `dartq packages` from inside a nested
    /// workspace member (the bare `.` default). The walk must ascend
    /// through the intermediate `resolution: workspace` member to the
    /// covering root — a relative `.` previously died on
    /// `Path::parent(".") == ""` and reported the first-met member as
    /// orphaned (rc 64 with pub's no-root text), while real pub resolved
    /// the same directory fine.
    #[test]
    fn relative_invocation_inside_a_nested_member_reaches_the_root() {
        let root = temp_dir("relative-member");
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
        let here = std::env::current_dir().expect("cwd");
        std::env::set_current_dir(root.join("examples").join("mid").join("cockpit"))
            .expect("chdir");
        let universe = enclosing_universe(Path::new("."));
        std::env::set_current_dir(here).expect("chdir back");
        let universe = universe.expect("the nested member resolves through `.`");
        assert!(universe.root.is_absolute());
        assert_eq!(
            universe.root,
            std::fs::canonicalize(&root).expect("canonical root"),
            "the root is the workspace root"
        );
        let layout = universe
            .layout
            .expect("a pubspec universe carries its layout");
        assert_eq!(layout.root, universe.root);
        assert_eq!(
            layout.members,
            vec![
                universe.root.join("examples").join("mid"),
                universe.root.join("examples").join("mid").join("cockpit"),
            ]
        );
    }

    #[test]
    fn relative_invocation_roots_absolutely() {
        let root = temp_dir("relative");
        write(
            &root.join("pubspec.yaml"),
            "name: solo\nenvironment:\n  sdk: ^3.5.0\n",
        );
        let here = std::env::current_dir().expect("cwd");
        std::env::set_current_dir(&root).expect("chdir");
        let universe = enclosing_universe(Path::new("."));
        let report = universe.as_ref().map_or(ScanReport::default(), |universe| {
            scan(&universe.root, &ScanOptions::default())
        });
        std::env::set_current_dir(here).expect("chdir back");
        let universe = universe.expect("universe");
        assert!(
            universe.root.is_absolute() && !universe.root.as_os_str().is_empty(),
            "the root is absolute, not `{}`",
            universe.root.display()
        );
        assert_eq!(report.root, universe.root);
        assert!(
            !report
                .boundaries
                .iter()
                .any(|boundary| boundary.reason == BoundaryReason::ScanError),
            "a readable tree fabricates no scan errors: {:?}",
            report.boundaries
        );
        assert!(
            report
                .packages
                .iter()
                .any(|entry| entry.dir == universe.root),
            "the package still registers: {:?}",
            report.packages
        );
    }

    /// The inventory: a workspace root with members, a nested independent
    /// package, and the resolution state per entry.
    #[test]
    fn scan_inventories_workspaces_members_and_independents() {
        let root = temp_dir("inventory");
        write(
            &root.join("pubspec.yaml"),
            "name: ws\nenvironment:\n  sdk: ^3.5.0\nworkspace:\n  - pkgs/a\n",
        );
        write(
            &root.join("pkgs").join("a").join("pubspec.yaml"),
            "name: a\nenvironment:\n  sdk: ^3.5.0\nresolution: workspace\n",
        );
        write(
            &root.join("vendor").join("solo").join("pubspec.yaml"),
            "name: solo\nenvironment:\n  sdk: '>=3.0.0 <4.0.0'\n",
        );
        // A fresh clone: no package_config anywhere.
        let report = scan(&root, &ScanOptions::default());
        assert_eq!(report.packages.len(), 3);
        let ws = package(&report, &root);
        assert_eq!(ws.kind, PackageKind::WorkspaceRoot);
        assert_eq!(ws.name.as_deref(), Some("ws"));
        assert_eq!(ws.sdk.as_deref(), Some("^3.5.0"));
        assert!(!ws.resolved, "fresh clone: unresolved");
        let member = package(&report, &root.join("pkgs").join("a"));
        assert_eq!(
            member.kind,
            PackageKind::WorkspaceMember {
                root: normalize(&root)
            }
        );
        assert!(!member.resolved);
        let solo = package(&report, &root.join("vendor").join("solo"));
        assert_eq!(solo.kind, PackageKind::Independent);

        // After pub get (a config at the root covering root and member),
        // root and member resolve; the independent package does not.
        write_config(&root, &[(&root, "ws"), (&root.join("pkgs").join("a"), "a")]);
        let report = scan(&root, &ScanOptions::default());
        assert!(package(&report, &root).resolved);
        assert!(package(&report, &root.join("pkgs").join("a")).resolved);
        assert!(!package(&report, &root.join("vendor").join("solo")).resolved);
    }

    /// Nested checkouts — a clone's `.git` directory, a live worktree
    /// pointer, a stale one — are pruned and registered, never descended.
    #[test]
    fn scan_registers_nested_checkout_boundaries() {
        let root = temp_dir("checkouts");
        write(
            &root.join("pubspec.yaml"),
            "name: outer\nenvironment:\n  sdk: ^3.5.0\n",
        );
        let clone = root.join("vendor").join("clone");
        write(
            &clone.join("pubspec.yaml"),
            "name: clone\nenvironment:\n  sdk: ^3.5.0\n",
        );
        std::fs::create_dir_all(clone.join(".git")).expect("mkdir");

        let worktree = root.join("vendor").join("linked");
        write(
            &worktree.join("pubspec.yaml"),
            "name: linked\nenvironment:\n  sdk: ^3.5.0\n",
        );
        let live_git = temp_dir("main-git");
        std::fs::create_dir_all(&live_git).expect("mkdir");
        write(
            &worktree.join(".git"),
            &format!("gitdir: {}\n", live_git.display()),
        );

        let stale = root.join("vendor").join("stale");
        write(
            &stale.join("pubspec.yaml"),
            "name: stale\nenvironment:\n  sdk: ^3.5.0\n",
        );
        write(&stale.join(".git"), "gitdir: /gone/.git/worktrees/x\n");

        let report = scan(&root, &ScanOptions::default());
        assert_eq!(
            report.packages.len(),
            1,
            "foreign trees are not work points"
        );
        let boundary = |dir: &Path| {
            report
                .boundaries
                .iter()
                .find(|entry| entry.dir == normalize(dir))
                .unwrap_or_else(|| panic!("boundary {} in report", dir.display()))
        };
        assert_eq!(boundary(&clone).reason, BoundaryReason::NestedCheckout);
        assert_eq!(boundary(&worktree).reason, BoundaryReason::WorktreePointer);
        let stale_detail = boundary(&stale).detail.as_deref().expect("stale detail");
        assert!(stale_detail.contains("missing"), "{stale_detail}");
    }

    /// A plain copy or a tarball extraction (a pubspec with no `.git`
    /// under a foreign tree) is a nested independent work point.
    #[test]
    fn scan_lists_plain_copies_and_tarball_extractions() {
        let root = temp_dir("copies");
        write(
            &root.join("pubspec.yaml"),
            "name: outer\nenvironment:\n  sdk: ^3.5.0\n",
        );
        write(
            &root.join("vendor").join("copy").join("pubspec.yaml"),
            "name: copy\nenvironment:\n  sdk: ^3.5.0\n",
        );
        write(
            &root
                .join("vendor")
                .join("some_pkg-1.2.3")
                .join("pubspec.yaml"),
            "name: some_pkg\nenvironment:\n  sdk: ^3.5.0\n",
        );
        let report = scan(&root, &ScanOptions::default());
        assert_eq!(
            package(&report, &root.join("vendor").join("copy")).kind,
            PackageKind::Independent
        );
        assert_eq!(
            package(&report, &root.join("vendor").join("some_pkg-1.2.3")).kind,
            PackageKind::Independent
        );
    }

    /// `.git`-less nesting: pubspecs stacked without markers stay two
    /// independent universes; the ladder roots at the nearest pubspec.
    #[test]
    fn scan_handles_gitless_nesting() {
        let root = temp_dir("gitless");
        write(
            &root.join("a").join("pubspec.yaml"),
            "name: a\nenvironment:\n  sdk: ^3.5.0\n",
        );
        write(
            &root.join("a").join("b").join("pubspec.yaml"),
            "name: b\nenvironment:\n  sdk: ^3.5.0\n",
        );
        let universe = enclosing_universe(&root.join("a").join("b").join("lib"))
            .expect("nearest pubspec roots");
        assert_eq!(
            universe.root,
            root.join("a").join("b").canonicalize().expect("canonical")
        );
        let report = scan(&root, &ScanOptions::default());
        assert_eq!(report.packages.len(), 2);
        assert_eq!(
            package(&report, &root.join("a")).kind,
            PackageKind::Independent
        );
        assert_eq!(
            package(&report, &root.join("a").join("b")).kind,
            PackageKind::Independent
        );
    }

    /// A `path:` dependency crossing the scan root's boundary is a work
    /// point the walk cannot reach — registered with its consumer.
    #[test]
    fn scan_registers_path_dependencies_crossing_out() {
        let home = temp_dir("pathdep");
        let root = home.join("repo");
        let outside = home.join("sibling");
        write(
            &root.join("pubspec.yaml"),
            "name: repo\nenvironment:\n  sdk: ^3.5.0\ndependencies:\n  shared:\n    path: ../sibling\n",
        );
        write(
            &outside.join("pubspec.yaml"),
            "name: shared\nenvironment:\n  sdk: ^3.5.0\n",
        );
        let report = scan(&root, &ScanOptions::default());
        let dep = package(&report, &outside);
        assert_eq!(
            dep.kind,
            PackageKind::PathDependency {
                consumer: normalize(&root)
            }
        );
        assert_eq!(dep.name.as_deref(), Some("shared"));
    }

    /// Hidden, excluded, and build trees that contain a pubspec are
    /// pruned but registered — the inventory never silently loses a
    /// package-shaped tree.
    #[test]
    fn scan_registers_pruned_trees_that_hide_pubspecs() {
        let root = temp_dir("pruned");
        write(
            &root.join("pubspec.yaml"),
            "name: outer\nenvironment:\n  sdk: ^3.5.0\n",
        );
        write(
            &root.join(".dot").join("pkg").join("pubspec.yaml"),
            "name: dot\nenvironment:\n  sdk: ^3.5.0\n",
        );
        write(
            &root
                .join("test")
                .join("fixtures")
                .join("pkg")
                .join("pubspec.yaml"),
            "name: fixture\nenvironment:\n  sdk: ^3.5.0\n",
        );
        write(
            &root.join("build").join("pkg").join("pubspec.yaml"),
            "name: built\nenvironment:\n  sdk: ^3.5.0\n",
        );

        let report = scan(&root, &options(&["test/fixtures"]));
        let reason = |dir: &Path| {
            report
                .boundaries
                .iter()
                .find(|entry| entry.dir == normalize(dir))
                .map(|entry| entry.reason)
        };
        assert_eq!(reason(&root.join(".dot")), Some(BoundaryReason::HiddenTree));
        assert_eq!(
            reason(&root.join("test").join("fixtures")),
            Some(BoundaryReason::ExcludedTree)
        );
        assert_eq!(reason(&root.join("build")), Some(BoundaryReason::BuildTree));
        assert_eq!(
            report.packages.len(),
            1,
            "the pruned trees' pubspecs are not work points"
        );

        // A hidden tree with nothing package-like stays silent — no noise.
        std::fs::create_dir_all(root.join(".cache").join("deep")).expect("mkdir");
        let report = scan(&root, &ScanOptions::default());
        assert!(
            !report
                .boundaries
                .iter()
                .any(|entry| entry.dir == normalize(&root.join(".cache"))),
            "empty hidden trees are not registered"
        );
    }

    /// Include limits the listing (the walk still resolves every
    /// universe; only the report narrows).
    #[test]
    fn include_globs_limit_the_listing() {
        let root = temp_dir("include");
        write(
            &root.join("pubspec.yaml"),
            "name: ws\nenvironment:\n  sdk: ^3.5.0\nworkspace:\n  - pkgs/a\n",
        );
        write(
            &root.join("pkgs").join("a").join("pubspec.yaml"),
            "name: a\nenvironment:\n  sdk: ^3.5.0\nresolution: workspace\n",
        );
        let scan_options = ScanOptions {
            include: vec![Glob::new("pkgs/**").expect("glob")],
            exclude: Vec::new(),
        };
        let report = scan(&root, &scan_options);
        assert_eq!(report.packages.len(), 1);
        assert_eq!(report.packages[0].name.as_deref(), Some("a"));
    }

    /// An orphan member — `resolution: workspace` no root includes —
    /// stays a work point carrying the refusal as its navigation note.
    #[test]
    fn orphan_member_keeps_its_refusal_note() {
        let root = temp_dir("orphan");
        write(
            &root.join("pkg").join("pubspec.yaml"),
            "name: pkg\nenvironment:\n  sdk: ^3.5.0\nresolution: workspace\n",
        );
        let report = scan(&root, &ScanOptions::default());
        let pkg = package(&report, &root.join("pkg"));
        assert_eq!(pkg.kind, PackageKind::Independent);
        assert!(
            pkg.note
                .as_deref()
                .unwrap_or_default()
                .contains("no workspace root"),
            "{pkg:?}"
        );
    }

    /// A symlinked package directory is never descended (cycle safety)
    /// but is registered when its target directly carries a pubspec.
    #[cfg(unix)]
    #[test]
    fn symlinked_package_dirs_register_as_boundaries() {
        let root = temp_dir("symlink");
        write(
            &root.join("pubspec.yaml"),
            "name: outer\nenvironment:\n  sdk: ^3.5.0\n",
        );
        write(
            &root.join("real").join("pubspec.yaml"),
            "name: real\nenvironment:\n  sdk: ^3.5.0\n",
        );
        std::os::unix::fs::symlink(root.join("real"), root.join("alias")).expect("symlink");
        let report = scan(&root, &ScanOptions::default());
        assert!(
            report
                .boundaries
                .iter()
                .any(|entry| entry.dir == normalize(&root.join("alias"))
                    && entry.reason == BoundaryReason::SymlinkDir),
            "symlink boundary: {:?}",
            report.boundaries
        );
        assert_eq!(
            report.packages.len(),
            2,
            "the real dir registers, the alias does not"
        );
    }

    /// The other half of the symlink rule: a symlinked directory carrying
    /// no pubspec registers nothing — the boundary exists only when a
    /// package-shaped tree silently disappeared.
    #[cfg(unix)]
    #[test]
    fn symlinked_dirs_without_a_pubspec_stay_silent() {
        let root = temp_dir("symlink-bare");
        write(
            &root.join("pubspec.yaml"),
            "name: outer\nenvironment:\n  sdk: ^3.5.0\n",
        );
        std::fs::create_dir_all(root.join("plain")).expect("mkdir");
        std::os::unix::fs::symlink(root.join("plain"), root.join("alias")).expect("symlink");
        let report = scan(&root, &ScanOptions::default());
        assert!(
            !report
                .boundaries
                .iter()
                .any(|entry| entry.reason == BoundaryReason::SymlinkDir),
            "no package hidden, no boundary: {:?}",
            report.boundaries
        );
        assert_eq!(report.packages.len(), 1);
    }

    /// The probe's depth budget: a pubspec up to three directories inside
    /// a pruned tree is found and named in the boundary's detail; one
    /// deeper is beyond the budget and the tree stays silent.
    #[test]
    fn probe_depth_bounds_boundary_registration() {
        let root = temp_dir("probe-depth");
        write(
            &root.join("pubspec.yaml"),
            "name: outer\nenvironment:\n  sdk: ^3.5.0\n",
        );
        // Three levels below the hidden root: inside the budget.
        write(
            &root
                .join(".dot")
                .join("a")
                .join("b")
                .join("c")
                .join("pubspec.yaml"),
            "name: deep\nenvironment:\n  sdk: ^3.5.0\n",
        );
        let report = scan(&root, &ScanOptions::default());
        let boundary = report
            .boundaries
            .iter()
            .find(|entry| entry.dir == normalize(&root.join(".dot")))
            .expect("the shallow-hidden pubspec is registered");
        assert_eq!(boundary.reason, BoundaryReason::HiddenTree);
        assert!(
            boundary
                .detail
                .as_deref()
                .unwrap_or_default()
                .contains("pubspec.yaml"),
            "the detail names the hidden pubspec: {:?}",
            boundary.detail
        );

        // Four levels below: beyond the budget, silently pruned.
        let deep = temp_dir("probe-deeper");
        write(
            &deep.join("pubspec.yaml"),
            "name: outer\nenvironment:\n  sdk: ^3.5.0\n",
        );
        write(
            &deep
                .join(".dot")
                .join("a")
                .join("b")
                .join("c")
                .join("d")
                .join("pubspec.yaml"),
            "name: deeper\nenvironment:\n  sdk: ^3.5.0\n",
        );
        let report = scan(&deep, &ScanOptions::default());
        assert!(
            !report
                .boundaries
                .iter()
                .any(|entry| entry.dir == normalize(&deep.join(".dot"))),
            "beyond PROBE_DEPTH the tree is not registered: {:?}",
            report.boundaries
        );
    }

    /// The probe's width budget: a pruned tree with more than
    /// [`PROBE_DIRS`] directories to visit stops the search (nothing
    /// package-shaped is reported), while a wide-but-within-budget tree
    /// still finds a pubspec in its last-sorted corner.
    #[test]
    fn probe_width_budget_bounds_the_search() {
        let root = temp_dir("probe-wide");
        write(
            &root.join("pubspec.yaml"),
            "name: outer\nenvironment:\n  sdk: ^3.5.0\n",
        );
        let hidden = root.join(".dot");
        for index in 0..70 {
            std::fs::create_dir_all(hidden.join(format!("d{index:02}"))).expect("mkdir");
        }
        let report = scan(&root, &ScanOptions::default());
        assert!(
            !report
                .boundaries
                .iter()
                .any(|entry| entry.dir == normalize(&hidden)),
            "the budget stops the search: {:?}",
            report.boundaries
        );

        let wide = temp_dir("probe-wide-ok");
        write(
            &wide.join("pubspec.yaml"),
            "name: outer\nenvironment:\n  sdk: ^3.5.0\n",
        );
        let hidden = wide.join(".dot");
        for index in 0..60 {
            std::fs::create_dir_all(hidden.join(format!("d{index:02}"))).expect("mkdir");
        }
        write(
            &hidden.join("zz").join("pubspec.yaml"),
            "name: z\nenvironment:\n  sdk: ^3.5.0\n",
        );
        let report = scan(&wide, &ScanOptions::default());
        assert!(
            report
                .boundaries
                .iter()
                .any(|entry| entry.dir == normalize(&hidden) && entry.detail.is_some()),
            "within the budget the sweep reaches the last corner: {:?}",
            report.boundaries
        );
    }

    /// Include and exclude combine: exclude prunes the walk (a pruned tree
    /// registers as a boundary and its packages never list, include
    /// cannot resurrect them), include narrows the report.
    #[test]
    fn include_and_exclude_combine() {
        let root = temp_dir("combine");
        write(
            &root.join("pubspec.yaml"),
            "name: ws\nenvironment:\n  sdk: ^3.5.0\nworkspace:\n  - pkgs/a\n",
        );
        write(
            &root.join("pkgs").join("a").join("pubspec.yaml"),
            "name: a\nenvironment:\n  sdk: ^3.5.0\nresolution: workspace\n",
        );
        // One level inside the pruned tree — the boundary probe searches
        // the pruned dir's children, matching the other prune fixtures.
        write(
            &root
                .join("vendor")
                .join("fixture")
                .join("pkg")
                .join("pubspec.yaml"),
            "name: fixture\nenvironment:\n  sdk: ^3.5.0\n",
        );
        let scan_options = ScanOptions {
            include: vec![
                Glob::new("pkgs/**").expect("glob"),
                Glob::new("vendor/fixture").expect("glob"),
            ],
            exclude: vec![Glob::new("vendor/fixture").expect("glob")],
        };
        let report = scan(&root, &scan_options);
        assert_eq!(
            report
                .packages
                .iter()
                .map(|entry| entry.name.as_deref())
                .collect::<Vec<_>>(),
            [Some("a")],
            "the excluded tree never lists, include cannot resurrect it"
        );
        assert!(
            report.boundaries.iter().any(|entry| entry.dir
                == normalize(&root.join("vendor").join("fixture"))
                && entry.reason == BoundaryReason::ExcludedTree
                && entry.detail.is_some()),
            "the pruned pubspec is a boundary with its detail: {:?}",
            report.boundaries
        );
    }

    /// An orphan member — `resolution: workspace` no root accepts — is
    /// the ladder's refusal, from its own directory and from below it:
    /// the pubspec exists but no universe answers, which is the
    /// navigation error the caller renders.
    #[test]
    fn orphan_member_universe_is_the_refusal() {
        let root = temp_dir("orphan-ladder");
        write(
            &root.join("pkg").join("pubspec.yaml"),
            "name: pkg\nenvironment:\n  sdk: ^3.5.0\nresolution: workspace\n",
        );
        for from in [root.join("pkg"), root.join("pkg").join("lib")] {
            assert!(
                matches!(enclosing_universe(&from), Err(PubError::OrphanMember(_))),
                "the orphan refuses from {}",
                from.display()
            );
        }
    }

    /// The resolved-state fast path falls through to the declarative walk
    /// on every unverifiable ref: no `workspaceRoot` key, a root without
    /// a pubspec — the ladder's answer is byte-identical to running
    /// without the ref.
    #[test]
    fn ref_fast_path_falls_through_on_unverifiable_refs() {
        let root = temp_dir("ref-fallthrough");
        write(
            &root.join("pubspec.yaml"),
            "name: ws\nenvironment:\n  sdk: ^3.5.0\nworkspace:\n  - pkg_a\n",
        );
        write(
            &root.join("pkg_a").join("pubspec.yaml"),
            "name: pkg_a\nenvironment:\n  sdk: ^3.5.0\nresolution: workspace\n",
        );
        let from = root.join("pkg_a");
        let expected = enclosing_universe(&from).expect("universe").root;
        let reference = from
            .join(".dart_tool")
            .join("pub")
            .join("workspace_ref.json");
        write(&reference, "{}");
        assert_eq!(
            enclosing_universe(&from).expect("universe").root,
            expected,
            "no key: ignored"
        );
        write(&reference, r#"{"workspaceRoot":"../../empty"}"#);
        assert_eq!(
            enclosing_universe(&from).expect("universe").root,
            expected,
            "a ref naming a pubspec-less dir is ignored"
        );
    }

    /// A scan root without a pubspec of its own is a vantage point, not a
    /// work point: its children's universes list, the hidden trees under
    /// it register, and the root itself never appears as a package.
    #[test]
    fn bare_scan_root_lists_child_packages() {
        let root = temp_dir("bare-root");
        write(
            &root.join("app").join("pubspec.yaml"),
            "name: app\nenvironment:\n  sdk: ^3.5.0\n",
        );
        write(
            &root.join(".dot").join("pkg").join("pubspec.yaml"),
            "name: dot\nenvironment:\n  sdk: ^3.5.0\n",
        );
        let report = scan(&root, &ScanOptions::default());
        assert_eq!(report.root, normalize(&root));
        assert_eq!(report.packages.len(), 1);
        assert_eq!(report.packages[0].name.as_deref(), Some("app"));
        assert!(
            !report
                .packages
                .iter()
                .any(|entry| entry.dir == normalize(&root)),
            "the bare root is not a package"
        );
        assert!(
            report
                .boundaries
                .iter()
                .any(|entry| entry.dir == normalize(&root.join(".dot"))
                    && entry.reason == BoundaryReason::HiddenTree),
            "the hidden tree registers: {:?}",
            report.boundaries
        );
    }
}
