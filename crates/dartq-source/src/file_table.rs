//! Workspace paths interned to small stable integers (PRD §10.4).
//!
//! # The path-universe invariant
//!
//! Every path the engine compares — [`FileTable`] keys, [`Locator`] entries,
//! resolver-composed targets, canonicalized package roots — is expressed in
//! one canonical form: the canonicalized longest existing ancestor of the
//! lexically cleaned spelling, plus the remaining components. A path's
//! identity therefore never depends on whether the path itself is on disk:
//! an overlay-only file interns under the same canonical spelling the
//! resolver composes for it, lexical-prefix package attribution sees one
//! spelling, and interning a path before or after its creation yields the
//! same id. Pinned by
//! `intern_across_existence_transition_holds_one_id`.

use std::borrow::Cow;
use std::path::{Component, Path, PathBuf};
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
/// the cleaned spelling as its cache key — a second, owned copy per
/// normalized spelling — which [`forget`] retracts together with the path
/// resolution.
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
    /// One spelling per logical path: since the unified rule derives every
    /// spelling's identity through the same canonical-ancestor walk (see
    /// [`normalize_path`]), all spellings of one path resolve to one key and
    /// one id, and `forget` through any of them severs that id for all —
    /// including after the file is deleted. Pinned by
    /// `forget_through_any_spelling_severs_the_shared_identity`.
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

/// The single normalization rule for path keys, shared by [`FileTable`] and
/// [`Locator`](crate::Locator). It realizes the path-universe invariant (see
/// the module docs): the result is the canonicalized longest existing
/// ancestor of the cleaned spelling, plus the remaining components.
///
/// 1. The spelling is absolutized (relative paths join the current working
///    directory) and cleaned under the lexical dot-segment discipline of
///    [`normalize_lexical`] — `.` dropped, `..` popped or clamped at an
///    absolute root.
/// 2. Walking upward from the cleaned path, the first ancestor that
///    canonicalizes wins. For an existing path the first iteration is plain
///    `std::path::canonicalize` — the physical truth, with symlinks
///    (`/var` → `/private/var` on macOS) and `..` resolved. For a missing
///    path the nearest existing ancestor canonicalizes instead, so the
///    derived spelling lives in the same symlink-resolved universe as the
///    disk files around it; the missing tail itself carries no symlink
///    truth to resolve.
///
///    Platform note: on Windows `canonicalize` returns `\\?\C:\...`
///    verbatim (extended-length) paths, and this crate does not strip the
///    prefix — it makes no cross-platform claim today (the oracle lock is
///    macOS). Any future Windows oracle-parity work must strip the prefix
///    here, or emitted paths diverge byte-wise from `dart analyze`.
/// 3. If nothing up to the root canonicalizes (no reachable file system),
///    the cleaned spelling is kept as-is.
///
/// Consequence: one logical path — one key — one id, independent of its
/// existence bit. The pre-#287 rule kept non-existent spellings lexical, so
/// an overlay-only file interned under the literal workspace spelling while
/// the resolver composed the same file from a library's canonicalized
/// directory: one logical file, two ids, and the part claim missed
/// residency. Lexical `..` in a *dirty* spelling is resolved textually
/// before the walk, matching the analyzer's lexical path discipline
/// (package:path `normalize`); a `..` that crosses a symlink in an existing
/// prefix of a dirty spelling is the one corner where the derived identity
/// can differ from the physical resolution — unobservable for cleaned
/// workspace spellings, which is the only input shape the engine and the
/// oracle produce.
///
/// [`FileTable`] and [`Locator`](crate::Locator) consume the rule through
/// [`PathMemo`], which memoizes both branches — see its doc for the caching
/// policy.
pub(crate) fn normalize_path(path: &Path) -> PathBuf {
    normalize_keyed(&lexical_key(path))
}

/// The walk of [`normalize_path`] over an already-cleaned absolute key.
fn normalize_keyed(cleaned: &Path) -> PathBuf {
    for ancestor in cleaned.ancestors() {
        if let Ok(base) = ancestor.canonicalize() {
            let remainder = cleaned
                .strip_prefix(ancestor)
                .expect("ancestors are prefixes");
            // When `cleaned` itself canonicalizes, the remainder is empty and
            // `PathBuf::join("")` appends a trailing separator (`/a/b.dart/`
            // — a directory spelling for a file), corrupting every
            // `files.path` reader downstream.
            if remainder.as_os_str().is_empty() {
                return base;
            }
            return base.join(remainder);
        }
    }
    cleaned.to_path_buf()
}

/// The lexical dot-segment discipline, shared with dartq-pub (its
/// `normalize` delegates here — one implementation of the RFC 3986
/// `remove_dot_segments` face pub's workspace walk and the oracle's
/// package:path `normalize` agree on): `.` components drop, `..` pops a
/// trailing plain component or clamps at an absolute root; a leading `..`
/// run on a relative path survives.
#[must_use]
pub fn normalize_lexical(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => match normalized.components().next_back() {
                Some(Component::Normal(_)) => {
                    normalized.pop();
                }
                Some(Component::RootDir | Component::Prefix(_)) => {}
                _ => normalized.push(".."),
            },
            other => normalized.push(other),
        }
    }
    normalized
}

/// The memo key of a spelling: absolutized and dot-segment cleaned, so
/// aliased spellings (`a/./b`, `a/../b`, relative vs absolute) of one
/// logical path share one memo entry. The common case — an absolute spelling
/// with no dot segments — borrows instead of allocating.
fn lexical_key(path: &Path) -> Cow<'_, Path> {
    let dotted = path
        .components()
        .any(|c| matches!(c, Component::CurDir | Component::ParentDir));
    if path.is_absolute() && !dotted {
        return Cow::Borrowed(path);
    }
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        match std::env::current_dir() {
            Ok(cwd) => cwd.join(path),
            Err(_) => path.to_path_buf(),
        }
    };
    Cow::Owned(normalize_lexical(&absolute))
}

/// Memoized spelling-to-canonical-path cache backing [`FileTable`] and
/// [`Locator`](crate::Locator).
///
/// Each structure owns one memo; both consume the single rule in
/// [`normalize_path`], and the memo removes the repeated `canonicalize`
/// syscall cost when the same spelling recurs. The engine re-walks the same
/// files every pass, so the memoized hit is the hot path — for overlay-only
/// files (never on disk, re-interned every batch) the walk would otherwise
/// re-probe the ancestor chain each time.
///
/// # Caching policy
///
/// Both rule branches memoize, keyed by the cleaned spelling (see
/// [`lexical_key`]): canonicalize successes and derived missing-path
/// identities alike. Pre-#287 only canonicalize successes were cached,
/// because a missing path's identity flipped with its existence bit and
/// caching would have fused the two ids; the unified rule wants exactly that
/// stability — a spelling's identity is the same before and after its file
/// appears — so the old rationale is gone and both branches cache.
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
    /// The cached normalization for `path`, if this cleaned spelling was
    /// normalized here before. Read-only: fills nothing.
    pub(crate) fn get(&self, path: &Path) -> Option<Arc<Path>> {
        let key = lexical_key(path);
        self.canonical.get(key.as_ref()).cloned()
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

    /// Normalizes `path` under the single rule, caching the result under the
    /// cleaned spelling — both rule branches memoize (see the caching policy
    /// in the type docs).
    pub(crate) fn normalize(&mut self, path: &Path) -> Arc<Path> {
        let key = lexical_key(path);
        if let Some(cached) = self.canonical.get(key.as_ref()) {
            return Arc::clone(cached);
        }
        let shared: Arc<Path> = Arc::from(normalize_keyed(key.as_ref()));
        self.canonical.insert(key.into_owned(), Arc::clone(&shared));
        shared
    }

    /// Removes the memoized entry for the cleaned spelling of `path`, if any.
    ///
    /// [`FileTable::forget`] and [`Locator::forget`](crate::Locator::forget)
    /// call this so forgotten spellings stop occupying memo space. Only the
    /// given spelling is retracted: memo keys are owned copies of one
    /// spelling each, and the canonical [`Arc`] values are refcounted, so
    /// removal is invisible to every other holder — but there is no
    /// canonical-to-spellings index, so sibling spellings that normalize to
    /// the same file keep their entries (each holding its own key while
    /// sharing the canonical value) until their own forget.
    pub(crate) fn retract(&mut self, path: &Path) {
        let key = lexical_key(path);
        self.canonical.remove(key.as_ref());
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
            std::env::temp_dir().join(format!("dartq-source-{tag}-{}-{n}", std::process::id()));
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

    /// The missing-path face of the universe invariant: a path with no disk
    /// identity derives its spelling from the nearest existing ancestor, so
    /// it lands in the same canonical universe (symlinks resolved) as the
    /// disk files around it.
    #[test]
    fn missing_paths_resolve_through_their_existing_ancestor() {
        let base = temp_dir("missing-ancestor");
        let missing = base.join("never-created.txt");

        let mut table = FileTable::new();
        let id = table.intern(&missing);
        let expected = base
            .canonicalize()
            .expect("the ancestor exists")
            .join("never-created.txt");
        assert_eq!(table.path(id), Some(expected.as_path()));
        assert_eq!(table.intern(&missing), id, "same missing path stays one id");

        let other = table.intern(&base.join("also-missing.txt"));
        assert_ne!(id, other);

        let _ = fs::remove_dir_all(&base);
    }

    /// Dot segments resolve lexically before the ancestor walk: `a/..`
    /// cancels, so the walk starts from the cleaned path and the derived
    /// spelling stays in the canonical universe of the existing prefix.
    #[test]
    fn missing_path_dot_segments_resolve_before_the_ancestor_walk() {
        let base = temp_dir("dotdot");
        let missing = base
            .join("a")
            .join("..")
            .join("b")
            .join("never-created.txt");

        let mut table = FileTable::new();
        let id = table.intern(&missing);
        let expected = base
            .canonicalize()
            .expect("the ancestor exists")
            .join("b")
            .join("never-created.txt");
        assert_eq!(table.path(id), Some(expected.as_path()));

        let _ = fs::remove_dir_all(&base);
    }

    /// The fallback's relative branch: a missing relative path is joined
    /// onto the current working directory, then normalized like any other —
    /// the derived spelling carries the cwd's canonical identity.
    #[test]
    fn missing_relative_path_joins_onto_cwd() {
        let mut table = FileTable::new();
        let relative = Path::new("dartq-source-never-created-relative-pin.txt");

        let id = table.intern(relative);
        let cwd = std::env::current_dir().expect("cwd");
        let expected = cwd.canonicalize().expect("cwd exists").join(relative);
        assert_eq!(table.path(id), Some(expected.as_path()));
    }

    /// The path-universe invariant itself: interning a spelling before its
    /// file exists (derived canonical-ancestor spelling) and again after
    /// creation (canonicalize success) yields the SAME id under the SAME
    /// canonical spelling. This is the overlay-only residency contract the
    /// resolver's composed targets depend on (#287).
    #[test]
    fn intern_across_existence_transition_holds_one_id() {
        let base = temp_dir("transition");
        let path = base.join("late.txt");

        let mut table = FileTable::new();
        let before = table.intern(&path); // missing: derived canonical spelling
        let derived = table.path(before).expect("interned").to_path_buf();
        fs::write(&path, b"x").expect("create the file");
        let after = table.intern(&path); // exists: canonicalize success

        assert_eq!(before, after, "identity does not flip with existence");
        let canonical = path.canonicalize().expect("file exists now");
        assert_eq!(table.path(after), Some(canonical.as_path()));
        assert_eq!(
            derived, canonical,
            "the derived spelling was already canonical"
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

    /// Unified-identity pin: all spellings of one path share one id, so
    /// `forget` through any of them — here a dot-segment alias while the
    /// file exists — severs the shared identity for every spelling, and a
    /// re-intern mints a fresh id under the same canonical key.
    #[test]
    fn forget_through_any_spelling_severs_the_shared_identity() {
        let base = temp_dir("forget-shared");
        fs::create_dir(base.join("sub")).expect("create sub dir");
        let path = base.join("late.txt");
        let aliased = base.join("sub").join("..").join("late.txt");

        let mut table = FileTable::new();
        let first = table.intern(&path); // missing: derived canonical spelling
        fs::write(&path, b"x").expect("create the file");
        assert_eq!(table.intern(&aliased), first, "aliased spelling, one id");

        assert_eq!(table.forget(&aliased), Some(first));
        assert_eq!(table.path(first), None);
        assert_eq!(
            table.try_file_id(&path),
            None,
            "every spelling of the severed key is dead"
        );

        let again = table.intern(&path);
        assert_ne!(again, first, "ids are never reused");
        assert_eq!(
            table.path(again),
            Some(path.canonicalize().expect("file exists now").as_path())
        );

        let _ = fs::remove_dir_all(&base);
    }

    /// Memo-retraction pin: re-interning a spelling after `forget` + file
    /// deletion re-derives the canonical spelling from the still-existing
    /// ancestor — the same key the file held while it existed, so
    /// delete/re-create cycles keep one stable identity (pre-#287 this fell
    /// back to the literal spelling and forked the identity).
    #[test]
    fn reintern_after_forget_and_delete_rederives_the_canonical_spelling() {
        let base = temp_dir("stale-canonical");
        let path = base.join("gone.txt");
        fs::write(&path, b"x").expect("write");
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
            Some(canonical.as_path()),
            "the re-intern re-derives the canonical spelling from the live ancestor"
        );
        assert_eq!(table.try_file_id(&path), Some(second));

        let _ = fs::remove_dir_all(&base);
    }

    /// Memo-retraction churn pin: intern → forget → delete → re-intern
    /// cycles never grow the memo beyond one entry per spelling. The forget
    /// retracts the spelling's entry; every re-intern — of an existing or a
    /// deleted file — refills exactly that one entry.
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
            1,
            "the deleted-file re-intern re-derives and refills one entry"
        );

        for _ in 0..3 {
            fs::write(&path, b"y").expect("re-create");
            table.intern(&path);
            assert_eq!(table.memo.canonical.len(), 1, "one entry per spelling");
            assert!(table.forget(&path).is_some());
            assert_eq!(table.memo.canonical.len(), 0, "retracted again");
            let _ = fs::remove_file(&path);
            table.intern(&path);
            assert_eq!(table.memo.canonical.len(), 1);
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
