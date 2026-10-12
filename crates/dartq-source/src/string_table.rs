//! Strings interned to small stable integers (ast-layout-spec §5).

use std::sync::RwLock;

use rustc_hash::FxHashMap;

/// A small stable integer naming one interned string.
///
/// Issued by a [`StringTable`]; the integer is only meaningful together with
/// the table that created it. Like [`FileId`](crate::FileId), ids are
/// immortal: they are handed out from 0 in first-intern order and are never
/// reused or retracted, so a holder may keep one for the process lifetime.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct StringId(u32);

impl StringId {
    /// The underlying integer, stable for the lifetime of the issuing
    /// [`StringTable`].
    #[must_use]
    pub const fn index(self) -> u32 {
        self.0
    }
}

/// Interning table for strings that must outlive a single file: element
/// names surfaced in results, diagnostic message fragments
/// (ast-layout-spec §5).
///
/// `intern` dedups by content: the same text always yields the same
/// [`StringId`], and ids are handed out from 0 in first-intern order. The
/// table is append-only with immortal ids — ids already recorded in
/// results, facts, and snapshots must keep resolving for the whole
/// session. No reclamation can soundly retire them: a live structure's
/// names cannot be attributed to one content whose death would prove the
/// names dead, because the same text arrives from many files and from
/// synthesized spellings (setter names, prefixed imports), so the holder
/// of a dedup hit is not the recorded first interner. The deferred §17
/// mark-sweep story is thereby closed in favor of a session ceiling:
/// memory grows with distinct interned strings at the measured ~93 B/entry
/// (ast-layout-spec §5, #128), [`len`](Self::len) exposes the count
/// against that bound, and overshoot is accepted — interning past any
/// nominal bound costs memory, never truth.
///
/// Locking: a read/write split for the worker parallelism at 10⁵-file
/// scale. [`resolve`](Self::resolve) and intern hits take the read lock —
/// parallel and allocation-free, a hit is one hash lookup; a miss releases
/// it, takes the write lock, and re-checks, so racing workers still mint
/// one id per text. Sharding stays deferred until a measured contention
/// signal.
///
/// Storage: text is copied once into the table and leaked as `&'static str`
/// (the rust-analyzer `symbol.rs` shape minus the tagged pointer, with u32
/// ids). The leak is the price of [`resolve`](Self::resolve) returning
/// slices without unsafe under interior mutability; the measured per-entry
/// footprint is recorded in ast-layout-spec §5.
#[derive(Debug, Default)]
pub struct StringTable {
    inner: RwLock<Inner>,
}

#[derive(Debug, Default)]
struct Inner {
    ids_by_str: FxHashMap<&'static str, StringId>,
    strings: Vec<&'static str>,
}

impl StringTable {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the [`StringId`] for `s`, allocating the next id on first
    /// sight. Repeated interning of the same text is a read-locked hash
    /// lookup; only the first sight takes the write lock.
    ///
    /// # Panics
    ///
    /// Panics when interning more than [`u32::MAX`] distinct strings, which
    /// would exhaust the [`StringId`] space, or when the table lock is
    /// poisoned by a panic in an earlier call (both are program bugs).
    #[must_use]
    pub fn intern(&self, s: &str) -> StringId {
        {
            let inner = self.inner.read().expect("StringTable lock poisoned");
            if let Some(&id) = inner.ids_by_str.get(s) {
                return id;
            }
        }
        let mut inner = self.inner.write().expect("StringTable lock poisoned");
        if let Some(&id) = inner.ids_by_str.get(s) {
            return id; // A concurrent worker minted it while we upgraded.
        }
        let id = StringId(
            u32::try_from(inner.strings.len()).expect("more than u32::MAX strings interned"),
        );
        let text: &'static str = Box::leak(Box::from(s));
        inner.ids_by_str.insert(text, id);
        inner.strings.push(text);
        id
    }

    /// How many distinct strings the table holds — the count the session
    /// ceiling's memory bound scales with (~93 B/entry, ast-layout-spec §5).
    ///
    /// # Panics
    ///
    /// Panics when the table lock is poisoned by a panic in an earlier
    /// call (a program bug).
    #[must_use]
    pub fn len(&self) -> usize {
        self.inner
            .read()
            .expect("StringTable lock poisoned")
            .strings
            .len()
    }

    /// Whether the table holds no strings.
    ///
    /// # Panics
    ///
    /// Panics when the table lock is poisoned by a panic in an earlier
    /// call (a program bug).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.inner
            .read()
            .expect("StringTable lock poisoned")
            .strings
            .is_empty()
    }

    /// The interned text behind `id`.
    ///
    /// # Panics
    ///
    /// Panics when `id` was not issued by this table. Ids are immortal and
    /// only minted here, so an unknown id is a program bug and is exposed as
    /// one rather than reported as a lookup miss.
    #[must_use]
    pub fn resolve(&self, id: StringId) -> &str {
        let inner = self.inner.read().expect("StringTable lock poisoned");
        inner.strings[id.index() as usize]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn duplicate_strings_share_one_id() {
        let table = StringTable::new();
        let hello = table.intern("hello");
        assert_eq!(table.intern("hello"), hello);
        assert_eq!(table.resolve(hello), "hello");
    }

    #[test]
    fn distinct_strings_get_distinct_ids() {
        let table = StringTable::new();
        let a = table.intern("a");
        let b = table.intern("b");
        assert_ne!(a, b);
        assert_eq!(table.resolve(a), "a");
        assert_eq!(table.resolve(b), "b");
    }

    #[test]
    fn resolve_round_trips_byte_exact() {
        let cases = [
            "hello world",
            "héllo",
            "世界 🦁",
            "\0\0",
            "line\nbreak\ttab",
            "emoji 🎯 tail",
        ];
        let table = StringTable::new();
        let ids: Vec<StringId> = cases.iter().map(|s| table.intern(s)).collect();
        for (s, id) in cases.iter().zip(&ids) {
            assert_eq!(table.resolve(*id), *s);
            assert_eq!(table.resolve(*id).as_bytes(), s.as_bytes());
        }
    }

    #[test]
    fn empty_string_is_a_valid_distinct_entry() {
        let table = StringTable::new();
        let empty = table.intern("");
        assert_eq!(empty.index(), 0);
        assert_eq!(table.resolve(empty), "");
        assert_eq!(table.intern(""), empty, "empty text dedups like any other");
        assert_ne!(table.intern("a"), empty);
    }

    #[test]
    fn ids_follow_first_intern_order() {
        let table = StringTable::new();
        let b = table.intern("b");
        let a = table.intern("a");
        let c = table.intern("c");
        assert_eq!((b.index(), a.index(), c.index()), (0, 1, 2));
        assert_eq!(table.intern("a"), a, "dedup keeps the first id");
        assert_eq!(table.intern("b"), b);
        assert_eq!(table.intern("c"), c);
    }

    /// Property sweep over a fixed generated set: interning in order yields
    /// dense ascending ids, re-interning is a pure hit, and `resolve`
    /// round-trips every entry.
    #[test]
    fn fixed_set_round_trips_and_dedups() {
        const N: u32 = 1000;
        let table = StringTable::new();
        let ids: Vec<StringId> = (0..N).map(|i| table.intern(&i.to_string())).collect();
        for i in 0..N {
            assert_eq!(ids[i as usize].index(), i, "ids are dense and ordered");
            let id = table.intern(&i.to_string());
            assert_eq!(id, ids[i as usize]);
            assert_eq!(table.resolve(id), i.to_string());
        }
    }

    /// The session ceiling's observability face: `len` counts distinct
    /// strings only — re-interning and dedup leave it untouched.
    #[test]
    fn len_counts_distinct_strings_only() {
        let table = StringTable::new();
        assert!(table.is_empty());
        let a = table.intern("a");
        assert_eq!(table.len(), 1);
        assert_eq!(table.intern("a"), a, "re-interning does not grow the table");
        assert_eq!(table.intern("b").index(), 1);
        assert_eq!(table.len(), 2);
        assert!(!table.is_empty());
    }

    /// The read/write lock split under contention: resolver threads reading
    /// pre-interned ids run concurrently with interners growing the table,
    /// every resolve stays byte-exact (ids are immortal however much the
    /// table grows around them), and the write path still mints exactly one
    /// slot per distinct text.
    #[test]
    fn resolves_run_concurrently_with_interning() {
        let table = Arc::new(StringTable::new());
        let seeded: Vec<StringId> = (0..64_u32)
            .map(|i| table.intern(&format!("seed_{i}")))
            .collect();

        std::thread::scope(|s| {
            for _ in 0..4 {
                let table = Arc::clone(&table);
                let seeded = seeded.clone();
                s.spawn(move || {
                    for _ in 0..200 {
                        for (i, &id) in seeded.iter().enumerate() {
                            assert_eq!(table.resolve(id), format!("seed_{i}"));
                        }
                    }
                });
            }
            for t in 0..2_u32 {
                let table = Arc::clone(&table);
                s.spawn(move || {
                    for i in 0..1000_u32 {
                        let _ = table.intern(&format!("t{t}_s{i}"));
                    }
                });
            }
        });

        assert_eq!(table.len(), 64 + 2 * 1000, "one slot per distinct text");
        for (i, &id) in seeded.iter().enumerate() {
            assert_eq!(table.resolve(id), format!("seed_{i}"));
        }
    }

    /// The RwLock-guarded sharing contract: threads racing to intern the same
    /// texts all observe one id per text, and the table holds exactly one
    /// slot per distinct text (no double-insert between lookup and insert).
    #[test]
    fn shared_table_dedups_across_threads() {
        let table = Arc::new(StringTable::new());
        let evens: Vec<StringId> = (0..250_u32)
            .map(|i| table.intern(&(i * 2).to_string()))
            .collect();

        let results: Vec<Vec<StringId>> = std::thread::scope(|s| {
            let handles: Vec<_> = (0..4)
                .map(|_| {
                    let table = Arc::clone(&table);
                    s.spawn(move || {
                        (0..500_u32)
                            .map(|i| table.intern(&i.to_string()))
                            .collect::<Vec<_>>()
                    })
                })
                .collect();
            handles
                .into_iter()
                .map(|h| h.join().expect("test threads join"))
                .collect()
        });

        for r in &results {
            assert_eq!(r, &results[0], "every thread sees identical ids");
        }
        for (i, id) in results[0].iter().enumerate() {
            if i % 2 == 0 {
                assert_eq!(*id, evens[i / 2], "pre-interned texts keep their ids");
            }
        }
        let inner = table.inner.read().expect("unpoisoned in test");
        assert_eq!(inner.strings.len(), 500, "one slot per distinct text");
    }
}
