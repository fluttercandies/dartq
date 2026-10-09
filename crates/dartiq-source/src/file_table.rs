//! Workspace paths interned to small stable integers (PRD §10.4).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use rustc_hash::FxHashMap;

/// A small stable integer naming one normalized workspace path.
///
/// Issued by a [`FileTable`]; the integer is only meaningful together with
/// the table that created it. This is the `file` field of every
/// [`SourceRange`](crate::SourceRange) and diagnostic (PRD §10.4).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FileId(u32);

impl FileId {
    /// The underlying integer, stable for the lifetime of the issuing
    /// [`FileTable`].
    #[must_use]
    pub const fn index(self) -> u32 {
        self.0
    }
}

/// Bidirectional map between normalized paths and [`FileId`]s.
///
/// `intern` is idempotent: the same normalized path always yields the same
/// [`FileId`], and ids are handed out from 0 in first-intern order.
///
/// The canonical path string is stored once per resident path: the map key
/// and the id slot share one allocation. The [`PathMemo`] additionally holds
/// the original spelling as its cache key — a second, owned copy per
/// canonicalize-success spelling — which [`forget`] retracts together with
/// the path resolution.
///
/// [`forget`] severs path resolution but never reclaims ids — the
/// immortal-`FileId` contract: ids already recorded in diagnostics, caches,
/// or other structures keep pointing at the same file, so a forgotten id
/// becomes a permanent hole (`path(id)` and `try_file_id` both miss) and is
/// never handed out again. `u32` provides 2^32 ids, far beyond any single
/// session's file churn.
#[derive(Debug, Default)]
pub struct FileTable {
    ids_by_path: FxHashMap<Arc<Path>, FileId>,
    paths: Vec<Option<Arc<Path>>>,
    memo: PathMemo,
}

impl FileTable {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the [`FileId`] for `path`, allocating the next id on first
    /// sight. The path is normalized first — see [`PathMemo`] for the rule
    /// and its caching — so aliased spellings of one file share a single id.
    /// Repeated interning of an already-canonicalized spelling is pure
    /// hashing.
    ///
    /// # Panics
    ///
    /// Panics when interning more than [`u32::MAX`] distinct paths, which
    /// would exhaust the [`FileId`] space.
    pub fn intern(&mut self, path: &Path) -> FileId {
        let normalized = self.memo.normalize(path);
        if let Some(&id) = self.ids_by_path.get(normalized.as_ref()) {
            return id;
        }
        let id =
            FileId(u32::try_from(self.paths.len()).expect("more than u32::MAX paths interned"));
        self.paths.push(Some(Arc::clone(&normalized)));
        self.ids_by_path.insert(normalized, id);
        id
    }

    /// The normalized path behind `id`, or `None` when this table never
    /// issued the id or the id has been [`forget`](Self::forget)ten.
    #[must_use]
    pub fn path(&self, id: FileId) -> Option<&Path> {
        self.paths
            .get(id.index() as usize)
            .and_then(|slot| slot.as_deref())
    }

    /// The id previously interned for `path`, or `None`.
    #[must_use]
    pub fn try_file_id(&self, path: &Path) -> Option<FileId> {
        let normalized = self.memo.get_or_normalize(path);
        self.ids_by_path.get(normalized.as_ref()).copied()
    }

    /// Removes `path` from the table, returning the id it used to hold.
    ///
    /// `path` is normalized once and exactly the key under that one
    /// normalization is severed: later `try_file_id` calls resolving to the
    /// same key miss, and `path(id)` returns `None` from then on — but the
    /// id is never reused; see the immortal-`FileId` contract in the type
    /// docs.
    ///
    /// One key, not one file: under the two-id existence transition (see
    /// [`normalize_path`]), a sibling id for the same file can stay live
    /// under a different key, and spellings that normalize to that key keep
    /// resolving to it — including after the file is deleted, while
    /// spellings normalizing to the severed key stay dead. Pinned by
    /// `forget_after_existence_transition_leaves_sibling_id_live`.
    /// Forgetting a path that is not currently interned returns `None`.
    ///
    /// The memo entry for this exact spelling is retracted (and never
    /// re-filled by the forget itself), so forgotten spellings stop
    /// occupying memo space and churn cycles do not grow the memo. Entries
    /// for other spellings of the same file stay until their own `forget` —
    /// see [`PathMemo::retract`] for the exact boundary. Pinned by
    /// `intern_forget_delete_reintern_cycles_do_not_grow_the_memo`.
    pub fn forget(&mut self, path: &Path) -> Option<FileId> {
        let normalized = self.memo.get_or_normalize(path);
        self.memo.retract(path);
        let id = self.ids_by_path.remove(normalized.as_ref())?;
        self.paths[id.index() as usize] = None;
        Some(id)
    }
}

/// Single normalization rule for path keys, shared by [`FileTable`] and
/// [`Locator`](crate::Locator):
///
/// 1. If the path exists, `std::path::canonicalize` wins: the result is
///    absolute with symlinks and `..` resolved. This is the canonical
///    identity — the same file reached through a symlink, a `.`/`..` spelling,
///    or a relative path interns to one id.
///
///    Platform note: on Windows `canonicalize` returns `\\?\C:\...`
///    verbatim (extended-length) paths, and this crate does not strip the
///    prefix — it makes no cross-platform claim today (the oracle lock is
///    macOS). Any future Windows oracle-parity work must strip the prefix
///    here, or emitted paths diverge byte-wise from `dart analyze`.
/// 2. Otherwise (path does not exist yet or is not accessible) fall back to
///    lexical absolutization: absolute paths are kept as-is, relative paths
///    are joined onto the current working directory; no symlink resolution is
///    attempted.
///
/// Consequence: two spellings of a not-yet-existing file may intern to
/// different ids; once the file exists, both spellings canonicalize to the
/// same path on the next `intern`. The same single spelling can also change
/// identity across the existence transition: interning it before creation
/// stores the lexical spelling under one id, and interning it again after
/// creation stores the canonical spelling under a second id. Both ids stay
/// live with `path(X) == path(Y)`, and `try_file_id` resolves to the newer
/// (canonical) one — a pass that interns early can therefore attribute later
/// diagnostics to a different id than an earlier pass did. Pinned by
/// `intern_across_existence_transition_holds_two_ids`.
///
/// [`FileTable`] and [`Locator`](crate::Locator) consume the rule through
/// [`PathMemo`], which memoizes the canonicalize branch — see its doc for
/// the caching policy.
pub(crate) fn normalize_path(path: &Path) -> PathBuf {
    match canonicalize_existing(path) {
        Some(canonical) => canonical,
        None => lexical_fallback(path),
    }
}

/// Rule half 1 of [`normalize_path`]: the canonical identity of an existing
/// path, or `None` when the path does not exist (or is not accessible).
fn canonicalize_existing(path: &Path) -> Option<PathBuf> {
    path.canonicalize().ok()
}

/// Rule half 2 of [`normalize_path`]: lexical absolutization of a path that
/// has no canonical identity — absolute paths are kept as-is (including any
/// lexical `..`), relative paths are joined onto the current working
/// directory; no symlink resolution is attempted.
fn lexical_fallback(path: &Path) -> PathBuf {
    if path.is_absolute() {
        return path.to_path_buf();
    }
    match std::env::current_dir() {
        Ok(cwd) => cwd.join(path),
        Err(_) => path.to_path_buf(),
    }
}

/// Memoized spelling-to-canonical-path cache backing [`FileTable`] and
/// [`Locator`](crate::Locator).
///
/// Each structure owns one memo; both consume the single rule in
/// [`normalize_path`], and the memo removes the repeated `canonicalize`
/// syscall cost when the same spelling recurs. The engine re-walks the same
/// files every pass, so the memoized hit is the hot path.
///
/// # Caching policy
///
/// Only [`canonicalize`](std::path::Path::canonicalize) successes are
/// cached. The lexical fallback (rule half 2) is recomputed on every call:
/// it depends on the path's existence bit and the current working
/// directory, either of which can flip between calls, and the pinned
/// existence-transition semantics (one spelling interned before its file
/// exists and again after must yield two ids) depend on that existence
/// check being live. Caching fallbacks would fuse those two ids into one.
///
/// Cached entries have no TTL, no watcher, and no time-based eviction; the
/// only retraction is the owning structure's `forget`, which removes the
/// forgotten spelling's own entry (other spellings of the same file keep
/// theirs — the memo is keyed by spelling and keeps no
/// canonical-to-spellings index). Remaining entries live as long as the
/// owning structure and are bounded by the set of distinct spellings seen —
/// the same order of growth as the immortal-`FileId` table
/// itself. A cached mapping may go stale if the file system changes after
/// caching (a symlink is retargeted, or the file is deleted); this is
/// accepted for the session and is consistent with the immortal-`FileId`
/// contract, which likewise freezes identity at first sight and never
/// re-derives it. Content-level freshness is the engine's change-detection
/// job (Locator mtime/content records), not path-spelling resolution.
#[derive(Debug, Default)]
pub(crate) struct PathMemo {
    canonical: FxHashMap<PathBuf, Arc<Path>>,
}

impl PathMemo {
    /// The cached normalization for `path`, if this spelling was
    /// canonicalized here before. Read-only: fills nothing.
    pub(crate) fn get(&self, path: &Path) -> Option<Arc<Path>> {
        self.canonical.get(path).cloned()
    }

    /// Like [`PathMemo::get`], but falls back to a fresh (uncached)
    /// [`normalize_path`] on memo miss — for readers that hold only a shared
    /// borrow and therefore cannot insert.
    pub(crate) fn get_or_normalize(&self, path: &Path) -> Arc<Path> {
        match self.get(path) {
            Some(cached) => cached,
            None => Arc::from(normalize_path(path)),
        }
    }

    /// Normalizes `path` under the single rule, caching canonicalize
    /// successes. Lexical fallbacks stay uncached — see the caching policy
    /// in the type docs.
    pub(crate) fn normalize(&mut self, path: &Path) -> Arc<Path> {
        if let Some(cached) = self.get(path) {
            return cached;
        }
        match canonicalize_existing(path) {
            Some(canonical) => {
                let shared: Arc<Path> = Arc::from(canonical);
                self.canonical
                    .insert(path.to_path_buf(), Arc::clone(&shared));
                shared
            }
            None => Arc::from(lexical_fallback(path)),
        }
    }

    /// Removes the memoized entry for the exact spelling `path`, if any.
    ///
    /// [`FileTable::forget`] and [`Locator::forget`](crate::Locator::forget)
    /// call this so forgotten spellings stop occupying memo space. Only the
    /// given spelling is retracted: memo keys are owned copies of one
    /// spelling each, and the canonical [`Arc`] values are refcounted, so
    /// removal is invisible to every other holder — but there is no
    /// canonical-to-spellings index, so sibling spellings that canonicalize
    /// to the same file keep their entries (each holding its own key while
    /// sharing the canonical value) until their own forget.
    pub(crate) fn retract(&mut self, path: &Path) {
        self.canonical.remove(path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;
    use std::fs;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{Duration, Instant};

    static TEMP_COUNTER: AtomicUsize = AtomicUsize::new(0);

    fn temp_dir(tag: &str) -> PathBuf {
        let n = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("dartiq-source-{tag}-{}-{n}", std::process::id()));
        fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    #[test]
    fn intern_is_idempotent_and_dense() {
        let base = temp_dir("idempotent");
        fs::write(base.join("a.txt"), b"a").expect("write a");
        fs::write(base.join("b.txt"), b"b").expect("write b");

        let mut table = FileTable::new();
        let a = table.intern(&base.join("a.txt"));
        let again = table.intern(&base.join("a.txt"));
        let b = table.intern(&base.join("b.txt"));

        assert_eq!(a, again);
        assert_ne!(a, b);
        assert_eq!(a.index(), 0);
        assert_eq!(b.index(), 1);
        assert_eq!(
            table.path(a).and_then(Path::file_name),
            Some(OsStr::new("a.txt"))
        );
        assert!(table.path(a).expect("interned").is_absolute());

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn aliased_spellings_share_one_id() {
        let base = temp_dir("aliases");
        fs::write(base.join("a.txt"), b"a").expect("write a");

        let mut table = FileTable::new();
        let direct = table.intern(&base.join("a.txt"));
        let via_dot = table.intern(&base.join(".").join("a.txt"));

        assert_eq!(direct, via_dot);
        assert_eq!(table.try_file_id(&base.join("a.txt")), Some(direct));

        let _ = fs::remove_dir_all(&base);
    }

    #[cfg(unix)]
    #[test]
    fn symlink_resolves_to_target_id() {
        let base = temp_dir("symlink");
        fs::write(base.join("target.txt"), b"t").expect("write target");
        std::os::unix::fs::symlink(base.join("target.txt"), base.join("link.txt"))
            .expect("symlink");

        let mut table = FileTable::new();
        let target = table.intern(&base.join("target.txt"));
        let link = table.intern(&base.join("link.txt"));
        assert_eq!(target, link);

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn missing_paths_fall_back_to_lexical_absolutization() {
        let mut table = FileTable::new();
        let missing =
            std::env::temp_dir().join(format!("dartiq-source-missing-{}.txt", std::process::id()));

        let id = table.intern(&missing);
        assert_eq!(table.path(id), Some(missing.as_path()));
        assert_eq!(table.intern(&missing), id, "same missing path stays one id");

        let other = table.intern(&std::env::temp_dir().join("dartiq-source-missing-other.txt"));
        assert_ne!(id, other);
    }

    /// Audit A7: the documented fallback keeps `..` lexically — a nonexistent
    /// absolute path is stored as-is, with no lexical `..` resolution.
    #[test]
    fn missing_path_with_dotdot_stays_lexical() {
        let mut table = FileTable::new();
        let missing = std::env::temp_dir()
            .join("dartiq-source-dotdot-a")
            .join("..")
            .join("dartiq-source-dotdot-b")
            .join("never-created.txt");

        let id = table.intern(&missing);
        assert_eq!(table.path(id), Some(missing.as_path()));
    }

    /// The fallback's relative branch: a missing relative path is joined
    /// onto the current working directory (the absolute branches are pinned
    /// by `missing_paths_fall_back_to_lexical_absolutization` and
    /// `missing_path_with_dotdot_stays_lexical`).
    #[test]
    fn missing_relative_path_joins_onto_cwd() {
        let mut table = FileTable::new();
        let relative = Path::new("dartiq-source-never-created-relative-pin.txt");

        let id = table.intern(relative);
        let cwd = std::env::current_dir().expect("cwd");
        assert_eq!(table.path(id), Some(cwd.join(relative).as_path()));
    }

    /// Audit A3: one path spelling interned before the file exists (lexical
    /// key) and again after creation (canonical key) yields two live ids for
    /// that spelling; lookups resolve to the newer canonical id. This pins
    /// the transition case the doc consequence declares.
    #[test]
    fn intern_across_existence_transition_holds_two_ids() {
        let base = temp_dir("transition");
        let path = base.join("late.txt");

        let mut table = FileTable::new();
        let before = table.intern(&path); // missing: lexical spelling kept
        fs::write(&path, b"x").expect("create the file");
        let after = table.intern(&path); // exists: canonical spelling kept

        assert_ne!(before, after);
        assert_eq!(table.path(before), Some(path.as_path()));
        assert_eq!(
            table.path(after),
            Some(path.canonicalize().expect("file exists now").as_path())
        );
        assert_eq!(table.try_file_id(&path), Some(after));

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn try_file_id_misses_unknown_paths() {
        let table = FileTable::new();
        assert_eq!(
            table.try_file_id(Path::new("/definitely/not/interned.txt")),
            None
        );
    }

    /// Immortal-`FileId` contract: `forget` severs path resolution (both
    /// directions) and the id is never handed out again — re-interning the
    /// same path mints a fresh id, and the forgotten id stays a hole.
    #[test]
    fn forget_severs_resolution_and_never_reuses_ids() {
        let base = temp_dir("forget");
        fs::write(base.join("a.txt"), b"a").expect("write a");
        fs::write(base.join("b.txt"), b"b").expect("write b");

        let mut table = FileTable::new();
        let a = table.intern(&base.join("a.txt"));
        let b = table.intern(&base.join("b.txt"));

        assert_eq!(table.forget(&base.join("a.txt")), Some(a));
        assert_eq!(table.try_file_id(&base.join("a.txt")), None);
        assert_eq!(table.path(a), None, "forgotten ids stop resolving");

        assert_eq!(
            table.try_file_id(&base.join("b.txt")),
            Some(b),
            "sibling ids are untouched"
        );
        assert_eq!(
            table.path(b).and_then(Path::file_name),
            Some(OsStr::new("b.txt"))
        );

        assert_eq!(table.forget(&base.join("a.txt")), None, "already forgotten");

        let again = table.intern(&base.join("a.txt"));
        assert_ne!(again, a, "ids are never reused");
        assert_eq!(again.index(), b.index() + 1, "new ids keep counting up");
        assert_eq!(table.try_file_id(&base.join("a.txt")), Some(again));

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn forget_never_interned_path_returns_none() {
        let mut table = FileTable::new();
        assert_eq!(table.forget(Path::new("/never/interned.txt")), None);
    }

    /// T1 pin: after the two-id existence transition, `forget` severs only
    /// the key under the normalization it is given — the lexical sibling id
    /// stays live, and once the file is deleted it resolves again through a
    /// spelling that normalizes to its key (memo miss + lexical fallback),
    /// while spellings hitting the memo's severed canonical entry stay dead.
    #[test]
    fn forget_after_existence_transition_leaves_sibling_id_live() {
        let base = temp_dir("forget-sibling");
        fs::create_dir(base.join("sub")).expect("create sub dir");
        let path = base.join("late.txt");
        // A second spelling of the same file with different components, so
        // it keys the memo separately from `path` yet canonicalizes to the
        // same file while it exists.
        let aliased = base.join("sub").join("..").join("late.txt");

        let mut table = FileTable::new();
        let before = table.intern(&path); // missing: lexical key
        fs::write(&path, b"x").expect("create the file");
        let after = table.intern(&aliased); // exists: canonical key, second id

        assert_ne!(before, after);
        assert_eq!(table.path(before), Some(path.as_path()));
        assert_eq!(
            table.path(after),
            Some(path.canonicalize().expect("file exists now").as_path())
        );

        assert_eq!(table.forget(&aliased), Some(after));
        assert_eq!(table.path(after), None);
        assert_eq!(
            table.try_file_id(&aliased),
            None,
            "the canonical key is severed"
        );

        let _ = fs::remove_file(&path);

        assert_eq!(
            table.try_file_id(&path),
            Some(before),
            "the lexical sibling id stays live and resolvable"
        );
        assert_eq!(table.path(before), Some(path.as_path()));

        let _ = fs::remove_dir_all(&base);
    }

    /// Memo-retraction pin: re-interning a spelling after `forget` + file
    /// deletion takes the uncached lexical fallback — the fresh id is stored
    /// under the spelling itself, not the deleted file's stale canonical
    /// path (the pre-retraction behavior resurrected the stale canonical key
    /// from the memo; `forget` now retracts it).
    #[test]
    fn reintern_after_forget_and_delete_takes_the_lexical_fallback() {
        let base = temp_dir("stale-canonical");
        let path = base.join("gone.txt");
        fs::write(&path, b"x").expect("create the file");
        let canonical: PathBuf = path.canonicalize().expect("file exists");

        let mut table = FileTable::new();
        let first = table.intern(&path);
        assert_eq!(table.path(first), Some(canonical.as_path()));

        assert_eq!(table.forget(&path), Some(first));
        let _ = fs::remove_file(&path);

        let second = table.intern(&path);
        assert_ne!(second, first, "the severed id is never reused");
        assert_eq!(table.path(first), None, "the first id stays a hole");
        assert_eq!(
            table.path(second),
            Some(path.as_path()),
            "the retracted memo sends the re-intern to the lexical spelling"
        );
        assert_eq!(table.try_file_id(&path), Some(second));

        let _ = fs::remove_dir_all(&base);
    }

    /// Memo-retraction churn pin: intern → forget → delete → re-intern
    /// cycles never grow the memo. The forget retracts the spelling's entry
    /// (and fills nothing itself); the deleted-file re-intern takes the
    /// uncached lexical fallback; a re-created file re-canonicalizes back
    /// into the single entry it held before.
    #[test]
    fn intern_forget_delete_reintern_cycles_do_not_grow_the_memo() {
        let base = temp_dir("memo-retract");
        let path = base.join("churn.txt");
        fs::write(&path, b"x").expect("write");

        let mut table = FileTable::new();
        table.intern(&path);
        assert_eq!(table.memo.canonical.len(), 1, "intern fills the memo");

        assert!(table.forget(&path).is_some());
        assert_eq!(
            table.memo.canonical.len(),
            0,
            "forget retracts the spelling's entry"
        );

        let _ = fs::remove_file(&path);
        table.intern(&path);
        assert_eq!(
            table.memo.canonical.len(),
            0,
            "the deleted-file re-intern takes the uncached fallback"
        );

        for _ in 0..3 {
            fs::write(&path, b"y").expect("re-create");
            table.intern(&path);
            assert_eq!(table.memo.canonical.len(), 1, "one entry per spelling");
            assert!(table.forget(&path).is_some());
            assert_eq!(table.memo.canonical.len(), 0, "retracted again");
            let _ = fs::remove_file(&path);
            table.intern(&path);
            assert_eq!(table.memo.canonical.len(), 0);
        }

        let _ = fs::remove_dir_all(&base);
    }

    /// Audit F2 regression guard: a repeated intern of an already-
    /// canonicalized spelling must be pure hashing — the memoized hit path
    /// issues no `canonicalize` syscalls. The ceiling of 10µs/op (averaged
    /// over N = 10^4 iterations) is placed from measurements: ~2.5µs/op in
    /// unoptimized test builds and ~115ns/op in release ([`FxHashMap`]; was
    /// ~230ns under `SipHash`), so roughly 80x of CI-jitter headroom remains
    /// on the slow side — while a single `canonicalize` costs ~16.6µs/op
    /// even in release (and proportionally more in test builds), so any memo
    /// bypass trips the assertion.
    #[test]
    fn repeated_intern_hit_stays_off_the_filesystem() {
        const N: u32 = 10_000;

        let base = temp_dir("memo-timing");
        let path = base.join("hot.txt");
        fs::write(&path, b"h").expect("write hot file");

        let mut table = FileTable::new();
        let id = table.intern(&path); // cold: canonicalizes once, fills the memo

        let start = Instant::now();
        for _ in 0..N {
            table.intern(&path);
        }
        let per_op = start.elapsed() / N;
        assert!(
            per_op < Duration::from_micros(10),
            "memoized intern hit regressed to filesystem speed: {per_op:?}/op"
        );
        assert_eq!(table.intern(&path), id, "hits keep resolving to one id");

        let _ = fs::remove_dir_all(&base);
    }
}
