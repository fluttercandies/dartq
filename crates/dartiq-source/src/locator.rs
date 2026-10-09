//! Sparse path-to-content mapping backing change detection (PRD §16.7).

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use rustc_hash::FxHashMap;

use crate::content_id::ContentId;
use crate::file_table::PathMemo;

/// Records which [`ContentId`] each path had at which modification time, as
/// the mtime prefilter for change detection (PRD §16.7): on a later pass, a
/// path whose mtime is unchanged can reuse its recorded [`ContentId`] without
/// re-reading or rehashing; a changed mtime means the content must be
/// re-read, rehashed, and re-recorded via [`Locator::record`].
///
/// Pure data structure on purpose — all file system IO (`stat`, read, hash)
/// belongs to the callers. Paths are normalized with the same memoized rule
/// as [`FileTable`](crate::FileTable), so aliased spellings of one file
/// share a single entry. Like the file table, the memo applies only to
/// canonicalize successes: spellings with no canonical identity (missing
/// files) are re-resolved on every call, so this structure never grows for
/// them, and [`Locator::forget`] retracts a forgotten spelling's memo entry.
///
/// Path residency note: the locator stores its own normalized copy of each
/// path, separate from the one copy [`FileTable`](crate::FileTable) keeps.
/// Sharing a single allocation across the two structures would require the
/// engine to hand over the table's interned path (e.g. recording by
/// [`FileId`](crate::FileId)) instead of a bare `&Path`; that seam belongs to
/// the engine API and is deferred until it exists.
#[derive(Debug, Default)]
pub struct Locator {
    entries: FxHashMap<PathBuf, (SystemTime, ContentId)>,
    memo: PathMemo,
}

impl Locator {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Records (or overwrites) the `(mtime, content)` pair for `path`.
    ///
    /// Re-recording an already-known path — the every-pass hot case —
    /// updates the pair in place and allocates nothing; a first-time path
    /// inserts a fresh key.
    pub fn record(&mut self, path: &Path, mtime: SystemTime, content_id: ContentId) {
        let normalized = self.memo.normalize(path);
        match self.entries.get_mut(normalized.as_ref()) {
            Some(entry) => *entry = (mtime, content_id),
            None => {
                self.entries
                    .insert(normalized.as_ref().to_path_buf(), (mtime, content_id));
            }
        }
    }

    /// The recorded `(mtime, content id)` for `path`, or `None` when the
    /// path was never recorded.
    #[must_use]
    pub fn lookup(&self, path: &Path) -> Option<(SystemTime, ContentId)> {
        let normalized = self.memo.get_or_normalize(path);
        self.entries.get(normalized.as_ref()).copied()
    }

    /// Removes the entry for `path`, returning whether one was present.
    ///
    /// The engine calls this when a file is deleted, so its mtime record,
    /// path string, and the memo entry for this exact spelling are
    /// reclaimed instead of leaking for the session. Entries for other
    /// spellings that canonicalize to the same file stay until their own
    /// forget — the memo is keyed by spelling and keeps no
    /// canonical-to-spellings index (see [`PathMemo::retract`]).
    ///
    /// Deletion is a two-structure obligation: the engine must also call
    /// [`FileTable::forget`](crate::FileTable::forget) for the same path.
    /// Forgetting only one side leaves the other holding a stale record —
    /// a resolvable id or an mtime entry — for a file that no longer
    /// exists. Pinned by `forget_same_spelling_on_both_sides`.
    pub fn forget(&mut self, path: &Path) -> bool {
        let normalized = self.memo.get_or_normalize(path);
        self.memo.retract(path);
        self.entries.remove(normalized.as_ref()).is_some()
    }

    /// Keeps only the entries whose path satisfies `keep`, dropping the rest.
    ///
    /// This is the engine's garbage-collection primitive: after a pass, the
    /// engine retains the paths it still tracks and lets every stale entry go
    /// in one sweep instead of `forget`ting them one by one.
    ///
    /// The predicate receives each entry's normalized (canonical) path — the
    /// same key [`Locator::record`] stored — not the spelling the engine's
    /// walk happened to produce. Comparing against non-canonicalized
    /// spellings therefore misses every entry and sweeps the whole table;
    /// build the keep-set through the same normalization.
    ///
    /// The memo is not swept: it keys spellings while the predicate sees
    /// canonical paths, and there is no canonical-to-spellings index.
    /// Spellings are reclaimed through [`Locator::forget`], not `retain`.
    pub fn retain(&mut self, mut keep: impl FnMut(&Path) -> bool) {
        self.entries.retain(|path, _| keep(path));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content_id::content_hash;
    use std::fs;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static TEMP_COUNTER: AtomicUsize = AtomicUsize::new(0);

    #[test]
    fn record_lookup_overwrite() {
        let mut locator = Locator::new();
        let path = Path::new("/work/pkg/main.dart");
        let t0 = SystemTime::UNIX_EPOCH;
        let t1 = t0 + std::time::Duration::from_secs(1);

        assert_eq!(locator.lookup(path), None);

        let id0 = content_hash(b"old");
        locator.record(path, t0, id0);
        assert_eq!(locator.lookup(path), Some((t0, id0)));

        let id1 = content_hash(b"new");
        locator.record(path, t1, id1);
        assert_eq!(
            locator.lookup(path),
            Some((t1, id1)),
            "re-record overwrites"
        );
    }

    #[test]
    fn aliased_existing_paths_share_one_entry() {
        let n = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let base =
            std::env::temp_dir().join(format!("dartiq-source-locator-{}-{n}", std::process::id()));
        fs::create_dir_all(&base).expect("create temp dir");
        let file = base.join("a.dart");
        fs::write(&file, b"void main() {}").expect("write");

        let mut locator = Locator::new();
        let mtime = fs::metadata(&file)
            .expect("stat")
            .modified()
            .expect("mtime");
        let id = content_hash(b"void main() {}");
        locator.record(&file, mtime, id);

        assert_eq!(
            locator.lookup(&base.join(".").join("a.dart")),
            Some((mtime, id))
        );

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn forget_removes_exactly_one_path() {
        let mut locator = Locator::new();
        let p1 = Path::new("/work/pkg/old.dart");
        let p2 = Path::new("/work/pkg/live.dart");
        let t0 = SystemTime::UNIX_EPOCH;
        let id = content_hash(b"bytes");

        locator.record(p1, t0, id);
        locator.record(p2, t0, id);

        assert!(locator.forget(p1));
        assert_eq!(locator.lookup(p1), None);
        assert_eq!(locator.lookup(p2), Some((t0, id)));
        assert!(!locator.forget(p1), "already forgotten");
        assert!(!locator.forget(Path::new("/work/pkg/never.dart")));
    }

    /// Pin for the type-doc claim: missing paths take the uncached lexical
    /// fallback on every call, so record/lookup/forget cycles never grow
    /// either the entries map or the memo.
    #[test]
    fn missing_paths_never_grow_the_locator() {
        let mut locator = Locator::new();
        let missing = Path::new("/work/pkg/never-created.dart");
        let t0 = SystemTime::UNIX_EPOCH;
        let id = content_hash(b"bytes");

        for _ in 0..10 {
            locator.record(missing, t0, id);
            assert_eq!(locator.lookup(missing), Some((t0, id)));
            assert!(
                locator.memo.get(missing).is_none(),
                "the lexical fallback must stay unmemoized"
            );
            assert!(locator.forget(missing));
            assert!(
                locator.memo.get(missing).is_none(),
                "the forget itself must not memoize"
            );
        }
        assert_eq!(locator.lookup(missing), None, "entries reclaimed");
    }

    #[test]
    fn retain_sweeps_all_stale_entries() {
        let mut locator = Locator::new();
        let stale_a = Path::new("/work/pkg/deleted_a.dart");
        let stale_b = Path::new("/work/pkg/deleted_b.dart");
        let live = Path::new("/work/pkg/main.dart");
        let t0 = SystemTime::UNIX_EPOCH;
        let id = content_hash(b"bytes");

        locator.record(stale_a, t0, id);
        locator.record(stale_b, t0, id);
        locator.record(live, t0, id);

        locator.retain(|path| path == live);

        assert_eq!(locator.lookup(stale_a), None);
        assert_eq!(locator.lookup(stale_b), None);
        assert_eq!(locator.lookup(live), Some((t0, id)));

        locator.retain(|_| false);
        assert_eq!(locator.lookup(live), None, "retain to empty clears all");
    }

    /// T3 composite pin: one spelling drives the two-structure deletion
    /// obligation — `Locator::forget` and `FileTable::forget` on the same
    /// path — and both structures stop resolving that spelling.
    #[test]
    fn forget_same_spelling_on_both_sides() {
        let n = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let base =
            std::env::temp_dir().join(format!("dartiq-source-both-{}-{n}", std::process::id()));
        fs::create_dir_all(&base).expect("create temp dir");
        let file = base.join("gone.dart");
        fs::write(&file, b"void main() {}").expect("write");

        let mut table = crate::FileTable::new();
        let mut locator = Locator::new();
        let mtime = fs::metadata(&file)
            .expect("stat")
            .modified()
            .expect("mtime");
        let id = content_hash(b"void main() {}");
        let file_id = table.intern(&file);
        locator.record(&file, mtime, id);

        assert_eq!(locator.lookup(&file), Some((mtime, id)));
        assert_eq!(table.try_file_id(&file), Some(file_id));

        assert!(locator.forget(&file));
        assert_eq!(table.forget(&file), Some(file_id));

        assert_eq!(locator.lookup(&file), None);
        assert_eq!(table.try_file_id(&file), None);
        assert_eq!(table.path(file_id), None);

        let _ = fs::remove_dir_all(&base);
    }
}
