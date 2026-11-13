//! The freshness check of the resident server: one parallel stat walk over
//! every file an analysis can read, diffed against the previous walk.
//!
//! The server watches its roots itself, but a watcher gives no guarantee
//! at request time: an edit saved a millisecond before `dartq analyze`
//! may not have reached it. The walk is the request-time truth. It covers
//!
//! * the analysis roots, minus dot-entries (the analyzer's own
//!   exclusion, `ContextRoot._isExcluded`) — of a dot-entry only the
//!   `<dir>/.dart_tool/package_config.json` a resolution can depend
//!   on is read;
//! * every package root of the governing `package_config.json` that lives
//!   outside the pub cache and the SDK (path dependencies, workspace
//!   siblings) — an import reaches them, so their edits change results;
//! * the `package_config.json`, `analysis_options.yaml` and `pubspec.yaml`
//!   of every ancestor directory of a root, which decide the contexts.
//!
//! A snapshot is a sorted `(path, size, mtime)` vector: the diff is one
//! linear merge, and the vector costs no hash buckets.
//!
//! The companion [`WalkPlan`] precomputes the walked root set and the
//! config files the walk re-stats, so the watch substrate's journal
//! update path (`watch::Freshness`) reuses the exact surface a full take
//! covers — never a second derivation to drift apart.

use std::cmp::Ordering;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};
use std::time::UNIX_EPOCH;

use parking_lot::Mutex;
use rayon::prelude::*;
use rustc_hash::FxHashSet;

/// The freshness check's total stat calls since process start — shared
/// with the watch substrate's journal path and read by the examples'
/// measurement table.
pub(crate) static STAT_CALLS: AtomicU64 = AtomicU64::new(0);

/// The state a freshness diff needs — (size, mtime) vs absent. A
/// non-regular file or an unreadable path is `None`, identical to absent
/// in `take` and to the walk's filtering.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Stat {
    size: u64,
    mtime_ns: u64,
}

/// One `fs::metadata` stat as the walk sees it. Errors and non-regular
/// targets are `None` — identical to absent in `take` and to the walk's
/// filtering.
#[must_use]
pub(crate) fn stat(path: &Path) -> Option<Stat> {
    STAT_CALLS.fetch_add(1, AtomicOrdering::Relaxed);
    let metadata = fs::metadata(path).ok().filter(|m| m.is_file())?;
    Some(Stat::of(&metadata))
}

impl Stat {
    fn of(metadata: &fs::Metadata) -> Self {
        let mtime_ns = metadata
            .modified()
            .ok()
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |since| {
                u64::try_from(since.as_nanos()).unwrap_or(u64::MAX)
            });
        Self {
            size: metadata.len(),
            mtime_ns,
        }
    }

    /// The file's size — the ledger's sizing figure.
    fn size(&self) -> u64 {
        self.size
    }
}

/// One file's stat facts.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Entry {
    path: Box<Path>,
    stat: Stat,
}

/// One walk's result, sorted by path.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
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

/// Whether a changed non-Dart file can change analysis results: every
/// `.yaml`/`.yml` (options include chains, pubspec, data-driven fix
/// files), the `package_config.json` a resolution depends on, and the
/// files named `AndroidManifest.xml` the manifest checks read. Other
/// `.json`/`.xml` — build output, assets, IDE state — are not inputs.
fn analyzer_input(path: &Path) -> bool {
    match path.extension().and_then(|ext| ext.to_str()) {
        Some("yaml" | "yml") => true,
        Some("xml") => path
            .file_name()
            .is_some_and(|name| name == "AndroidManifest.xml"),
        Some("json") => path
            .file_name()
            .is_some_and(|name| name == "package_config.json"),
        _ => false,
    }
}

fn is_dart(path: &Path) -> bool {
    path.extension().is_some_and(|ext| ext == "dart")
}

/// Whether the walk records `path` at all — a Dart source or a non-Dart
/// analyzer input. One rule for both sides of the diff: a file the walk
/// would never emit can never enter the snapshot through a dirty stat.
pub(crate) fn recorded(path: &Path) -> bool {
    is_dart(path) || analyzer_input(path)
}

/// The snapshot entry a dirty path maps to: the path itself when its
/// root-relative spelling holds no dot segment; the
/// `<dir>/.dart_tool/package_config.json` the walk reads under a dot
/// prefix when the path names the config (or the `.dart_tool` dir
/// itself — a created, removed or renamed tree moves as one event);
/// `None` for every other dot path and for a path outside the walked
/// set — the walk never reaches either.
pub(crate) fn merge_target(plan: &WalkPlan, path: &Path) -> Option<PathBuf> {
    let root = plan.walked.iter().find(|root| path.starts_with(root))?;
    let rel = path.strip_prefix(root).expect("prefix checked");
    let mut rest = rel.components();
    while let Some(component) = rest.next() {
        let name = component.as_os_str();
        if name.as_encoded_bytes().first() != Some(&b'.') {
            continue;
        }
        // The first dot segment decides: only `.dart_tool` reaches the
        // snapshot, and of it only `package_config.json`.
        if name != ".dart_tool" {
            return None;
        }
        return match (rest.next(), rest.next()) {
            (None, _) => Some(path.join("package_config.json")),
            (Some(file), None) if file.as_os_str() == "package_config.json" => {
                Some(path.to_path_buf())
            }
            _ => None,
        };
    }
    Some(path.to_path_buf())
}

/// Everything a freshness pass needs precomputed — the walked root set
/// (analysis roots plus the local package roots the reachable configs
/// name) and the individually stat'ed config files. Computed once per
/// snapshot; the journal-maintained path reuses the same plan so both
/// paths always cover the identical surface.
///
/// The plan's own inputs are kept beside it so a config move can re-run
/// the discovery `take` runs — the same code path, never a second
/// implementation to drift.
pub(crate) struct WalkPlan {
    /// The directory (and single-file) set `walk_all` covers, outermost
    /// first — the analysis roots plus their local package roots.
    pub(crate) walked: Vec<PathBuf>,
    /// The config files stat'ed individually at every take — the
    /// `packages` override plus each ancestor directory's
    /// `analysis_options.yaml` / `pubspec.yaml` /
    /// `.dart_tool/package_config.json` candidates.
    pub(crate) configs: Vec<PathBuf>,
    /// Which of `configs` sit under a watched tree. The journal path
    /// leaves those to the watcher (their moves arrive as dirty paths)
    /// and re-stats only the rest — the out-of-root ancestors — per
    /// request. `watched[i]` is `false` when the corresponding `configs`
    /// path lies outside `walked`.
    pub(crate) watched: Vec<bool>,
    /// The symlinked directories the last walk followed — `(canonical
    /// target, lexical path)` pairs, refreshed by every full take. The
    /// watch substrate arms an extra recursive watch on each target (the
    /// OS does not report events through a symlink) and maps the reported
    /// canonical paths back to the walk's lexical spelling.
    pub(crate) links: Vec<(PathBuf, PathBuf)>,
    roots: Vec<PathBuf>,
    packages: Option<PathBuf>,
    sdk_home: PathBuf,
}

impl WalkPlan {
    /// Re-runs this plan's discovery over the current snapshot: the
    /// ancestor config scan and `package_config.json` parses re-derive
    /// `walked` and `configs` — the same code path a `take` runs, so a
    /// config edit can never desynchronize the plan from the walk.
    fn refresh(&mut self, snapshot: &Snapshot) {
        let next = plan_of(&self.roots, self.packages.as_deref(), &self.sdk_home, Some(snapshot));
        self.walked = next.walked;
        self.configs = next.configs;
        self.watched = next.watched;
    }

    /// The analysis roots this plan was computed for.
    pub(crate) fn roots(&self) -> &[PathBuf] {
        &self.roots
    }

    /// The `packages` override the spec carries, if any.
    pub(crate) fn packages(&self) -> Option<&Path> {
        self.packages.as_deref()
    }

    /// The installation the SDK lives in.
    pub(crate) fn sdk_home(&self) -> &Path {
        &self.sdk_home
    }
}

/// The plan `take` would compute over `roots` — the analysis roots plus
/// their local package roots, and the ancestor config files re-stat'ed
/// per pass. `snapshot` seeds the nested-package scan (its package
/// configs under `.dart_tool` are parsed for out-of-tree roots); `None`
/// runs the nested discovery pass inline as `take` always did.
fn plan_of(
    roots: &[PathBuf],
    packages: Option<&Path>,
    sdk_home: &Path,
    snapshot: Option<&Snapshot>,
) -> WalkPlan {
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
    let local = local_filter(sdk_home);
    for config in &configs {
        walked.extend(package_roots(config).into_iter().filter(|p| local(p)));
    }
    // Nested packages (a monorepo under one root) resolve through their
    // own configs: their local roots outside the walked set join it. The
    // plan's nested scan reads the snapshot's own `.dart_tool` entries —
    // the same set `take` discovered on its own pass.
    let nested_source: Vec<PathBuf> = match snapshot {
        Some(snapshot) => snapshot
            .0
            .iter()
            .filter(|e| e.path.ends_with(".dart_tool/package_config.json"))
            .map(|e| e.path.to_path_buf())
            .collect(),
        None => Vec::new(),
    };
    let nested: Vec<PathBuf> = nested_source
        .iter()
        .flat_map(|e| package_roots(e))
        .filter(|p| local(p) && !walked.iter().any(|outer| p.starts_with(outer)))
        .collect();
    walked.extend(nested);
    let watched: Vec<bool> = files
        .iter()
        .map(|file| walked.iter().any(|root| file.starts_with(root)))
        .collect();
    WalkPlan {
        walked,
        configs: files,
        watched,
        links: Vec::new(),
        roots: roots.to_vec(),
        packages: packages.map(Path::to_path_buf),
        sdk_home: sdk_home.to_path_buf(),
    }
}

/// The pub-cache/SDK-locality predicate of `take` — a package root
/// outside the SDK and the pub cache is a local path dependency whose
/// edits reach the analysis.
fn local_filter(sdk_home: &Path) -> impl Fn(&Path) -> bool {
    let pub_cache = pub_cache();
    let sdk_home = sdk_home.to_path_buf();
    move |package: &Path| {
        !(package.starts_with(&sdk_home)
            || pub_cache
                .as_deref()
                .is_some_and(|cache| package.starts_with(cache))
            || package
                .components()
                .any(|c| c == Component::Normal(".pub-cache".as_ref())))
    }
}

impl Snapshot {
    /// Walks `roots` (canonical files or directories) plus the package
    /// roots and ancestor configuration files described in the module
    /// docs. `sdk_home` is the installation the SDK lives in (a Flutter
    /// checkout or the Dart SDK root); its files never change under one
    /// key and are not walked.
    #[must_use]
    pub fn take(roots: &[PathBuf], packages: Option<&Path>, sdk_home: &Path) -> Self {
        let mut snapshot = Self::default();
        let mut plan = plan_of(roots, packages, sdk_home, None);
        take_parts(&mut snapshot, &mut plan);
        snapshot
    }

    /// A [`take`] that also returns the [`WalkPlan`] it ran — the
    /// snapshot and its companion stay in lockstep for the
    /// journal-maintained path (`watch::Freshness`).
    pub(crate) fn take_with_plan(
        roots: &[PathBuf],
        packages: Option<&Path>,
        sdk_home: &Path,
    ) -> (Self, WalkPlan) {
        let mut snapshot = Self::default();
        let mut plan = plan_of(roots, packages, sdk_home, None);
        take_parts(&mut snapshot, &mut plan);
        (snapshot, plan)
    }

    /// The plan `take` opens with, computed without walking: the
    /// analysis roots, the ancestor config candidates, and the local
    /// package roots the root-level configs name. The freshness oracle
    /// arms its watcher over this surface before the first take runs,
    /// so the take happens entirely behind live coverage (the surface
    /// the take discovers beyond this joins the watch when the take
    /// installs, its unread window re-covered through the dirty set).
    pub(crate) fn preplan(roots: &[PathBuf], packages: Option<&Path>, sdk_home: &Path) -> WalkPlan {
        plan_of(roots, packages, sdk_home, None)
    }

    /// The journal-maintained freshness pass: `plan` is this snapshot's
    /// companion (refreshed in place when the config files moved),
    /// `dirty` the paths the watcher observed since the last pass, and
    /// `unwatched` the out-of-root config files re-stat'ed every
    /// request. `watcher_armed` reports whether the watcher covers
    /// `plan.walked`. Returns the [`Diff`] this pass applied — the same
    /// diff `self`'s previous state would report against a fresh `take`.
    ///
    /// Byte-identical to `take` over the same tree: a dirty path under a
    /// walked tree is a stat (present or absent) or a subtree walk when
    /// it names a directory (created, deleted, or renamed trees move as
    /// one event), a dirty path outside the walked set or touching a
    /// link falls back to a full take so the walk's own reachability
    /// decides, a dot-path merges through the walk's own `.dart_tool`
    /// rule ([`merge_target`]), and a config move re-plans through the
    /// same code `take` runs. Returns the applied [`Diff`] plus the
    /// directories the merge subtree-walked — the caller keeps those
    /// dirty one extra request, the window a new dir's own watch may
    /// not yet cover.
    pub(crate) fn take_into(
        &mut self,
        plan: &mut WalkPlan,
        dirty: &[PathBuf],
        unwatched: &[PathBuf],
        watcher_armed: bool,
    ) -> (Diff, Vec<PathBuf>) {
        // Any dirty path outside a watched tree (a watcher that never
        // armed or an out-of-tree event) cannot be trusted to a
        // per-path re-stat — the walk decides. A path spelled through
        // a link's lexical alias or naming a symlinked directory falls
        // back too: the walk emits link trees under the canonical
        // spelling, so the take is the truth for anything whose
        // reachability runs through a link. Dot-paths stay: their
        // merge mirrors the walk's own `.dart_tool` rule.
        let safe = watcher_armed
            && dirty.iter().all(|path| {
                plan.walked.iter().any(|root| path.starts_with(root))
                    && !plan
                        .links
                        .iter()
                        .any(|(_, lexical)| path.starts_with(lexical))
                    && !Self::is_linked_dir(path)
            });
        if !safe {
            return (self.retake(plan), Vec::new());
        }
        // Config files that moved since the last pass can rename the
        // walked set: refresh the plan when any config (in-tree from the
        // dirty set, out-of-tree from `unwatched`) has a different stat.
        let mut moved = false;
        for (index, file) in plan.configs.iter().enumerate() {
            let in_dirty = plan.watched.get(index).copied().unwrap_or(false)
                && dirty
                    .iter()
                    .any(|path| path == file || file.starts_with(path));
            let out_of_tree = !plan.watched.get(index).copied().unwrap_or(false);
            if !in_dirty && !out_of_tree {
                continue;
            }
            if stat(file) != self.stat_of(file) {
                moved = true;
                break;
            }
        }
        if moved {
            let walked = std::mem::take(&mut plan.walked);
            let configs = std::mem::take(&mut plan.configs);
            plan.refresh(self);
            // The plan's surface renamed — new walked roots carry files
            // no dirty path names, and dropped roots leave entries the
            // walk would never emit. The full take decides both.
            if plan.walked != walked || plan.configs != configs {
                plan.walked = walked;
                plan.configs = configs;
                return (self.retake(plan), Vec::new());
            }
        }
        self.merge_dirty(plan, dirty, unwatched)
    }

    /// Whether `path` is a symlink to a directory — a dirty path whose
    /// merge would walk it under its lexical spelling while the full
    /// take may dedup it under another. Falls back to the take.
    fn is_linked_dir(path: &Path) -> bool {
        let Ok(kind) = fs::symlink_metadata(path) else {
            return false;
        };
        kind.file_type().is_symlink() && fs::metadata(path).is_ok_and(|m| m.is_dir())
    }

    /// `path` resolved to the snapshot's canonical spelling: canonicalize
    /// the nearest existing ancestor and re-join the tail, so a deleted
    /// path (or one inside a just-deleted tree) still lands on the entry
    /// the walk emitted. Falls back to the raw path when nothing
    /// canonicalizes.
    fn canonicalish(path: &Path) -> PathBuf {
        let mut dir = path.to_path_buf();
        let mut tail: Vec<std::ffi::OsString> = Vec::new();
        loop {
            if let Ok(canonical) = fs::canonicalize(&dir) {
                let mut out = canonical;
                for part in tail.iter().rev() {
                    out.push(part);
                }
                return out;
            }
            match dir.file_name() {
                Some(name) => {
                    tail.push(name.to_os_string());
                    dir.pop();
                }
                None => return path.to_path_buf(),
            }
        }
    }

    /// The fallback pass: re-plan over the current snapshot, re-walk the
    /// whole surface through the same code `take` runs, and diff old
    /// against new — the answer a fresh `take` would give, byte for
    /// byte. The caller's `plan` leaves holding the take's own
    /// discovery (walked set, configs, symlinked dirs).
    fn retake(&mut self, plan: &mut WalkPlan) -> Diff {
        plan.refresh(self);
        let mut fresh = Self::default();
        take_parts(&mut fresh, plan);
        let diff = self.diff(&fresh);
        *self = fresh;
        diff
    }

    /// The entry `path` holds in the snapshot — `None` for a missing or
    /// non-regular file, the same absent the walk reports.
    fn stat_of(&self, path: &Path) -> Option<Stat> {
        self.0
            .binary_search_by(|entry| entry.path.as_ref().cmp(path))
            .ok()
            .map(|index| self.0[index].stat)
    }

    /// Applies the dirty set in place and returns the [`Diff`] it made
    /// plus the dirs it subtree-walked: each touched path's current
    /// stat replaces (or drops) the entry, and a new file inserts at
    /// its sorted slot. A touched path that names a directory moves as
    /// a subtree — the watcher reports created, deleted and renamed
    /// trees by their root alone — so the entries under its prefix are
    /// replaced by exactly what a subtree walk finds; symlinked dirs
    /// the subtree walk discovers join `plan.links` so the watcher can
    /// cover their real trees. `dirty` is guaranteed under
    /// `plan.walked` (the caller checked); dot-paths map through
    /// [`merge_target`] to the one `.dart_tool` file the walk reads;
    /// `unwatched` carries the out-of-root configs' own re-stat.
    fn merge_dirty(
        &mut self,
        plan: &mut WalkPlan,
        dirty: &[PathBuf],
        unwatched: &[PathBuf],
    ) -> (Diff, Vec<PathBuf>) {
        let mut diff = Diff::default();
        // Dirty events can name a file through a symlinked directory's
        // lexical alias while the snapshot stores the walk's canonical
        // spelling — resolve each touched path to its canonical form so
        // every merge hits the entry the walk emitted. `unwatched`
        // paths are plan-computed spellings, already canonical. The
        // spelling side stays: the caller carries plan-spelled dirs,
        // not canonical ones, into the next request's dirty set.
        let mut touched: Vec<(PathBuf, PathBuf)> = dirty
            .iter()
            .filter_map(|path| {
                merge_target(plan, path).map(|target| (Self::canonicalish(&target), target))
            })
            .collect();
        touched.extend(unwatched.iter().map(|path| (path.clone(), path.clone())));
        touched.sort_unstable();
        touched.dedup();
        // A subtree merge rewrites every entry under its prefix; a
        // touched path inside a merged subtree is already covered by
        // it, so its own stat is skipped.
        let mut subtrees: Vec<PathBuf> = Vec::new();
        let mut merged_dirs: Vec<PathBuf> = Vec::new();
        for (path, spelling) in touched {
            if subtrees.iter().any(|dir| path.starts_with(dir)) {
                continue;
            }
            STAT_CALLS.fetch_add(1, AtomicOrdering::Relaxed);
            match fs::metadata(&path) {
                Ok(meta) if meta.is_dir() => {
                    self.merge_subtree(&path, plan, &mut diff);
                    subtrees.push(path);
                    merged_dirs.push(spelling);
                }
                Ok(meta) => {
                    if !recorded(&path) && !plan.configs.contains(&path) {
                        // Not an analyzer input — the walk never emits
                        // it, so the merge never records it either.
                        continue;
                    }
                    let stat = Stat::of(&meta);
                    match self.index(&path) {
                        Ok(index) => {
                            if self.0[index].stat != stat {
                                Self::classify(&mut diff, &path);
                                self.0[index].stat = stat;
                            }
                        }
                        Err(index) => {
                            // A file the snapshot never held is the
                            // walk's add arm — `rebuild`, not `changed`.
                            Self::classify_gone(&mut diff, &path);
                            self.0.insert(
                                index,
                                Entry {
                                    path: path.into_boxed_path(),
                                    stat,
                                },
                            );
                        }
                    }
                }
                Err(_) => self.drop_path(&path, &mut diff),
            }
        }
        (diff, merged_dirs)
    }

    /// The diff classification of a touched path: a `.dart` file whose
    /// stat moved is a `changed` row; a non-Dart analyzer input moves
    /// the `rebuild` flag. Appearances and vanishings mark `rebuild` on
    /// either kind (the walk's add/remove arms both do), and a `.dart`
    /// path that is not a file (a directory took its name) counts as a
    /// vanishing — the walk emits no file there.
    fn classify(diff: &mut Diff, path: &Path) {
        if is_dart(path) {
            diff.changed.push(path.to_path_buf());
        } else if analyzer_input(path) {
            diff.rebuild = true;
        }
    }

    /// The diff classification of a vanished path: the walk's add/remove
    /// arms mark `rebuild` for a `.dart` or an analyzer input.
    fn classify_gone(diff: &mut Diff, path: &Path) {
        diff.rebuild |= is_dart(path) || analyzer_input(path);
    }

    /// `binary_search` on the sorted entry vector.
    fn index(&self, path: &Path) -> Result<usize, usize> {
        self.0.binary_search_by(|entry| entry.path.as_ref().cmp(path))
    }

    /// Drops the entry at `path` plus every entry under it (a deleted
    /// directory's files leave no metadata — both shapes are covered),
    /// classifying each removal into `diff`.
    fn drop_path(&mut self, path: &Path, diff: &mut Diff) {
        let index = match self.index(path) {
            Ok(index) => {
                let gone = self.0.remove(index);
                Self::classify_gone(diff, &gone.path);
                index
            }
            Err(index) => index,
        };
        while index < self.0.len() && self.0[index].path.starts_with(path) {
            let gone = self.0.remove(index);
            Self::classify_gone(diff, &gone.path);
        }
    }

    /// Replaces what a walk of `dir`'s subtree emits — the same
    /// discovery `take` runs (dot-rules, `.dart_tool` configs,
    /// symlinks). `dir` itself is never an entry: the walk emits files
    /// alone. Because link trees emit under the canonical spelling, the
    /// emitted set can land outside `dir`'s own prefix: each emitted
    /// entry upserts at its sorted slot, every stale entry under `dir`
    /// drops, and the whole swap diffs like the whole-snapshot merge
    /// does. A canonical dir that is ONLY reachable through this
    /// subtree (an out-of-plan link target) and is no longer emitted —
    /// a retargeted or vanished link — has its stale span dropped too;
    /// a canonical that is itself walked is another walk's business.
    /// Newly discovered links join `plan.links`.
    fn merge_subtree(&mut self, dir: &Path, plan: &mut WalkPlan, diff: &mut Diff) {
        // The canonical prefixes this subtree alone could have
        // emitted: every recorded link whose lexical alias sits under
        // `dir` — retargeting or removing the link makes them stale.
        let old_canonicals: Vec<PathBuf> = plan
            .links
            .iter()
            .filter(|(_, lexical)| lexical.starts_with(dir))
            .map(|(canonical, _)| canonical.clone())
            .collect();
        let mut entries = Vec::new();
        let visited = Mutex::new(FxHashSet::default());
        let found = Mutex::new(Vec::new());
        walk(dir, dir, &visited, &found, &mut entries);
        entries.sort_unstable_by(|a, b| a.path.cmp(&b.path));
        entries.dedup_by(|a, b| a.path == b.path);
        for link in found.into_inner() {
            if !plan.links.contains(&link) {
                plan.links.push(link);
            }
        }
        // The emitted set is the subtree's truth. Anything under `dir`
        // it no longer spells is gone; anything it spells under a
        // link's canonical upserts wherever it lands — the same inode
        // can only be emitted once per take, so an existing row at that
        // path is this file's earlier form.
        let mut index = self.index(dir).unwrap_or_else(|index| index);
        while index < self.0.len() && self.0[index].path.starts_with(dir) {
            if entries
                .iter()
                .any(|entry| entry.path == self.0[index].path)
            {
                index += 1;
            } else {
                let gone = self.0.remove(index);
                Self::classify_gone(diff, &gone.path);
            }
        }
        for canonical in old_canonicals {
            // Only an out-of-walk canonical's span is this subtree's to
            // drop; a walked root's own coverage re-emits it.
            if plan.walked.iter().any(|root| canonical.starts_with(root)) {
                continue;
            }
            let mut index = self.index(&canonical).unwrap_or_else(|index| index);
            while index < self.0.len() && self.0[index].path.starts_with(&canonical) {
                if entries
                    .iter()
                    .any(|entry| entry.path == self.0[index].path)
                {
                    index += 1;
                } else {
                    let gone = self.0.remove(index);
                    Self::classify_gone(diff, &gone.path);
                }
            }
        }
        for entry in entries {
            match self.index(&entry.path) {
                Ok(index) => {
                    if self.0[index].stat != entry.stat {
                        Self::classify(diff, &entry.path);
                        self.0[index].stat = entry.stat;
                    }
                }
                Err(index) => {
                    Self::classify_gone(diff, &entry.path);
                    self.0.insert(index, entry);
                }
            }
        }
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
                    if a.stat != b.stat {
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

    /// The total size of the `.dart` files the snapshot records — the
    /// ledger's source-size figure off the walk the oracle already paid,
    /// never a second one.
    #[must_use]
    pub fn dart_bytes(&self) -> u64 {
        self.0
            .iter()
            .filter(|entry| is_dart(&entry.path))
            .map(|entry| entry.stat.size())
            .sum()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// The walk `take` runs over a plan: `walk_all` over `plan.walked` (the
/// roots plus their package roots), the nested-package scan for roots
/// the snapshot's `.dart_tool` entries name, and the per-file stat of
/// `plan.configs`. `plan` refreshes in place when the nested pass adds
/// walked roots and collects the symlinked directories the walk
/// followed into `plan.links`, so the caller's plan matches the
/// snapshot produced.
fn take_parts(snapshot: &mut Snapshot, plan: &mut WalkPlan) {
    let local = local_filter(&plan.sdk_home);
    let visited = Mutex::new(FxHashSet::default());
    let links = Mutex::new(Vec::new());
    let mut entries = walk_all(plan.walked.clone(), &visited, &links);
    // Nested packages (a monorepo under one root) resolve through their
    // own configs: their local roots outside the walked set join it.
    let nested: Vec<PathBuf> = entries
        .iter()
        .filter(|e| e.path.ends_with(".dart_tool/package_config.json"))
        .flat_map(|e| package_roots(&e.path))
        .filter(|p| local(p) && !plan.walked.iter().any(|outer| p.starts_with(outer)))
        .collect();
    if !nested.is_empty() {
        plan.walked.extend(nested.iter().cloned());
        entries.extend(walk_all(nested, &visited, &links));
    }
    for file in &plan.configs {
        if let Some(stat) = stat(file) {
            entries.push(Entry {
                path: file.clone().into_boxed_path(),
                stat,
            });
        }
    }
    entries.par_sort_unstable_by(|a, b| a.path.cmp(&b.path));
    entries.dedup_by(|a, b| a.path == b.path);
    *snapshot = Snapshot(entries);
    plan.links = links.into_inner();
}

/// Walks `paths` (directories or files) in parallel, outermost first so a
/// nested path is never walked twice. `links` collects the symlinked
/// directories the walk followed — `(canonical target, lexical path)`.
fn walk_all(
    mut paths: Vec<PathBuf>,
    visited: &Mutex<FxHashSet<PathBuf>>,
    links: &Mutex<Vec<(PathBuf, PathBuf)>>,
) -> Vec<Entry> {
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
                walk(path, path, visited, links, &mut out);
            } else {
                stat_into(path, path, &mut out);
            }
            out
        })
        .collect()
}

/// One directory, its subdirectories in parallel. Dot-entries are skipped;
/// symlinked directories are followed once per canonical target — and
/// emitted under the CANONICAL spelling: two spellings of one directory
/// race to fill `visited`, so letting the canonical path win makes the
/// winner's output identical to the loser's, and the whole take
/// deterministic whichever branch walks the tree. `emit` is the spelling
/// every entry under this subtree takes; a dir whose canonical differs
/// from `emit` is a link — recorded — and its subtree re-roots at the
/// canonical path.
fn walk(
    dir: &Path,
    emit: &Path,
    visited: &Mutex<FxHashSet<PathBuf>>,
    links: &Mutex<Vec<(PathBuf, PathBuf)>>,
    out: &mut Vec<Entry>,
) {
    let mut emit = emit.to_path_buf();
    if let Ok(canonical) = fs::canonicalize(dir) {
        if canonical != emit {
            links
                .lock()
                .push((canonical.clone(), dir.to_path_buf()));
            emit = canonical.clone();
        }
        if !visited.lock().insert(canonical) {
            return;
        }
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
                stat_into(
                    &entry.path().join("package_config.json"),
                    &emit.join(".dart_tool").join("package_config.json"),
                    out,
                );
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
            dirs.push((path, emit.join(&name)));
        } else if recorded(&path) {
            // Only analyzer inputs are recorded — the same predicate
            // the journal merge's insert arm applies.
            out.push(entry_of(emit.join(&name), &metadata));
        }
    }
    let nested: Vec<Vec<Entry>> = dirs
        .par_iter()
        .map(|(dir, emit)| {
            let mut nested = Vec::new();
            walk(dir, emit, visited, links, &mut nested);
            nested
        })
        .collect();
    out.extend(nested.into_iter().flatten());
}

/// Stats `path` and emits the entry under `spelling` — the walk's
/// canonical-spelling rule keeps link-reached configs consistent with
/// their root tree's own emission.
fn stat_into(path: &Path, spelling: &Path, out: &mut Vec<Entry>) {
    if let Some(stat) = stat(path) {
        out.push(Entry {
            path: spelling.into(),
            stat,
        });
    }
}

fn entry_of(path: PathBuf, metadata: &fs::Metadata) -> Entry {
    Entry {
        path: path.into_boxed_path(),
        stat: Stat::of(metadata),
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
