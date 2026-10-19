//! Strings interned to small stable integers.

use std::hash::BuildHasher;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{OnceLock, RwLock};

use rustc_hash::{FxBuildHasher, FxHashMap};

/// A small stable integer naming one interned string.
///
/// Issued by a [`StringTable`]; the integer is only meaningful together with
/// the table that created it. Like [`FileId`](crate::FileId), ids are
/// immortal: they are handed out from 0 in first-intern order and are never
/// reused or retracted, so a holder may keep one for the process lifetime.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct StringId(u32);

impl StringId {
    /// The absent-id sentinel for packed rows' optional string columns (a
    /// bare reference row's qualifier). Ids are handed out from 0, and
    /// minting 2³²−1 distinct strings — hundreds of GB of text at the
    /// table's measured per-entry cost — is past every session ceiling, so
    /// equality with this constant is the packed-row "no string" test.
    pub const NONE: Self = Self(u32::MAX);

    /// The underlying integer, stable for the lifetime of the issuing
    /// [`StringTable`].
    #[must_use]
    pub const fn index(self) -> u32 {
        self.0
    }
}

/// Interning table for strings that must outlive a single file: element
/// names surfaced in results, diagnostic message fragments
///.
///
/// `intern` dedups by content: the same text always yields the same
/// [`StringId`], and ids are handed out from 0 in first-intern order. The
/// table is append-only with immortal ids — ids already recorded in
/// results, facts, and snapshots must keep resolving for the whole
/// session. No reclamation can soundly retire them: a live structure's
/// names cannot be attributed to one content whose death would prove the
/// names dead, because the same text arrives from many files and from
/// synthesized spellings (setter names, prefixed imports), so the holder
/// of a dedup hit is not the recorded first interner. The deferred
/// mark-sweep story is thereby closed in favor of a session ceiling:
/// memory grows with distinct interned strings at the measured ~93 B/entry
///, [`len`](Self::len) exposes the count
/// against that bound, and overshoot is accepted — interning past any
/// nominal bound costs memory, never truth.
///
/// Concurrency: every parser listener of the parallel fan-outs interns and
/// resolves per identifier, so the table is the hottest shared structure at
/// 10⁵-file scale (a single global `RwLock` measured at ~50% of all CPU
/// samples on a 10k-file cold analyze — its reader count is one cache line
/// every worker writes). Three layers remove the shared write traffic:
///
/// - [`resolve`](Self::resolve) is lock-free: the texts live in an
///   append-only bucketed slot array (bucket `b` holds `32 << b` slots,
///   allocated once on first use and never moved), so a read is two
///   acquire loads.
/// - intern hits are served first from a per-thread front cache (bounded,
///   cleared past [`FRONT_CAP`] entries, and tagged with the table's
///   identity so one thread never serves another table's ids), so the
///   skewed hot names (`int`, `String`, `this`, …) never touch a lock.
/// - front misses go to one of [`SHARDS`] cache-line-padded `RwLock` shards
///   picked by the text's hash; a shard miss mints under that shard's write
///   lock and re-checks, so racing workers still mint one id per text.
///
/// Ids are dense: one atomic counter issues them, so a single-threaded
/// intern sequence keeps the first-intern order exactly.
///
/// Storage: text is copied once into the table and leaked as `&'static str`
/// (the rust-analyzer `symbol.rs` shape minus the tagged pointer, with u32
/// ids). The leak is the price of [`resolve`](Self::resolve) returning
/// slices without unsafe under interior mutability; the measured per-entry
/// footprint is measured in the string-table memory test.
#[derive(Debug)]
pub struct StringTable {
    /// The process-unique identity the per-thread front cache is tagged
    /// with.
    identity: u64,
    shards: [Shard; SHARDS],
    slots: [OnceLock<Box<[OnceLock<&'static str>]>>; BUCKETS],
    next: AtomicU32,
}

/// Shard count of the text → id map; a power of two.
const SHARDS: usize = 64;

/// Slot buckets: bucket `b` holds `FIRST_BUCKET << b` slots, so 27 buckets
/// cover the whole `u32` id space.
const BUCKETS: usize = 28;
const FIRST_BUCKET_BITS: u32 = 5;

/// Per-thread front-cache bound in entries; past it the cache clears (a
/// bounded ~0.5 MB per worker thread).
const FRONT_CAP: usize = 1 << 14;

/// One map shard, padded to its own cache-line pair so neighboring shards'
/// lock words never share a line.
#[derive(Debug, Default)]
#[repr(align(128))]
struct Shard(RwLock<FxHashMap<&'static str, StringId>>);

static NEXT_IDENTITY: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// The calling thread's front cache: `(table identity, text → id)`.
struct Front {
    identity: u64,
    ids: FxHashMap<&'static str, StringId>,
}

thread_local! {
    static FRONT: std::cell::RefCell<Front> = std::cell::RefCell::new(Front {
        identity: u64::MAX,
        ids: FxHashMap::default(),
    });
}

/// The `(bucket, offset)` of slot `index`.
fn slot_of(index: u32) -> (usize, usize) {
    let biased = u64::from(index) + (1 << FIRST_BUCKET_BITS);
    let bucket = 63 - biased.leading_zeros() - FIRST_BUCKET_BITS;
    let offset = biased - (1 << (bucket + FIRST_BUCKET_BITS));
    (bucket as usize, offset as usize)
}

impl Default for StringTable {
    fn default() -> Self {
        Self {
            identity: NEXT_IDENTITY.fetch_add(1, Ordering::Relaxed),
            shards: std::array::from_fn(|_| Shard::default()),
            slots: std::array::from_fn(|_| OnceLock::new()),
            next: AtomicU32::new(0),
        }
    }
}

impl StringTable {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The front cache's answer for `s`, when this thread has seen it.
    fn front_get(&self, s: &str) -> Option<StringId> {
        FRONT.with(|front| {
            let front = front.borrow();
            (front.identity == self.identity)
                .then(|| front.ids.get(s).copied())
                .flatten()
        })
    }

    /// Records `(text, id)` in this thread's front cache, retagging it to
    /// this table (and clearing it past [`FRONT_CAP`]).
    fn front_put(&self, text: &'static str, id: StringId) {
        FRONT.with(|front| {
            let mut front = front.borrow_mut();
            if front.identity != self.identity || front.ids.len() >= FRONT_CAP {
                front.ids.clear();
                front.identity = self.identity;
            }
            front.ids.insert(text, id);
        });
    }

    /// Writes `text` into slot `index` (allocating its bucket on first use).
    fn store(&self, index: u32, text: &'static str) {
        let (bucket, offset) = slot_of(index);
        let slots = self.slots[bucket].get_or_init(|| {
            (0..(1_usize << (bucket as u32 + FIRST_BUCKET_BITS)))
                .map(|_| OnceLock::new())
                .collect()
        });
        slots[offset]
            .set(text)
            .expect("each id's slot is written exactly once");
    }

    /// Returns the [`StringId`] for `s`, allocating the next id on first
    /// sight. A repeat on the same thread is a lock-free front-cache hit;
    /// otherwise one shard's read lock serves the hit and only the first
    /// sight takes that shard's write lock.
    ///
    /// # Panics
    ///
    /// Panics when interning more than [`u32::MAX`] distinct strings, which
    /// would exhaust the [`StringId`] space, or when a shard lock is
    /// poisoned by a panic in an earlier call (both are program bugs).
    #[must_use]
    pub fn intern(&self, s: &str) -> StringId {
        if let Some(id) = self.front_get(s) {
            return id;
        }
        let hash = FxBuildHasher.hash_one(s);
        // The high bits pick the shard: the map's own probe consumes the
        // low bits, so the two stay independent.
        let shard = &self.shards[(hash >> 58) as usize % SHARDS].0;
        let hit = shard
            .read()
            .expect("StringTable lock poisoned")
            .get_key_value(s)
            .map(|(&text, &id)| (text, id));
        let (text, id) = match hit {
            Some(found) => found,
            None => {
                let mut map = shard.write().expect("StringTable lock poisoned");
                if let Some((&text, &id)) = map.get_key_value(s) {
                    // A concurrent worker minted it while we upgraded.
                    (text, id)
                } else {
                    let index = self.next.fetch_add(1, Ordering::Relaxed);
                    assert!(index != u32::MAX, "more than u32::MAX strings interned");
                    let id = StringId(index);
                    let text: &'static str = Box::leak(Box::from(s));
                    // The slot is written before the id is published
                    // through the map: every holder of the id obtained it
                    // under this lock (or from its own thread's front), so
                    // the slot read in `resolve` observes the write.
                    self.store(index, text);
                    map.insert(text, id);
                    (text, id)
                }
            }
        };
        self.front_put(text, id);
        id
    }

    /// How many distinct strings the table holds — the count the session
    /// ceiling's memory bound scales with.
    #[must_use]
    pub fn len(&self) -> usize {
        self.next.load(Ordering::Relaxed) as usize
    }

    /// Whether the table holds no strings.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The interned text behind `id`. Lock-free: two acquire loads.
    ///
    /// # Panics
    ///
    /// Panics when `id` was not issued by this table. Ids are immortal and
    /// only minted here, so an unknown id is a program bug and is exposed as
    /// one rather than reported as a lookup miss.
    #[must_use]
    pub fn resolve(&self, id: StringId) -> &str {
        let (bucket, offset) = slot_of(id.index());
        self.slots[bucket]
            .get()
            .and_then(|slots| slots[offset].get().copied())
            .expect("StringId not issued by this table")
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

    /// The sharded sharing contract: threads racing to intern the same
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
        assert_eq!(table.len(), 500, "one slot per distinct text");
    }

    /// Ids crossing every slot-bucket boundary (32, 96, 224, …) resolve to
    /// their own text: the bucket arithmetic is exact at each edge.
    #[test]
    fn resolve_is_exact_across_bucket_boundaries() {
        let table = StringTable::new();
        let ids: Vec<StringId> = (0..5000_u32).map(|i| table.intern(&format!("s{i}"))).collect();
        for (i, id) in ids.iter().enumerate() {
            assert_eq!(id.index() as usize, i, "dense first-intern order");
            assert_eq!(table.resolve(*id), format!("s{i}"));
        }
        for (index, (bucket, offset)) in [(0, (0, 0)), (31, (0, 31)), (32, (1, 0)), (95, (1, 63)), (96, (2, 0))] {
            assert_eq!(slot_of(index), (bucket, offset), "slot of {index}");
        }
    }

    /// Two tables on one thread never serve each other's ids through the
    /// per-thread front cache: the identity tag retags the cache.
    #[test]
    fn front_cache_never_crosses_tables() {
        let a = StringTable::new();
        let b = StringTable::new();
        let _ = b.intern("pad");
        let in_a = a.intern("name");
        let in_b = b.intern("name");
        assert_eq!(in_a.index(), 0);
        assert_eq!(in_b.index(), 1, "b minted its own id");
        assert_eq!(a.intern("name"), in_a);
        assert_eq!(b.intern("name"), in_b);
        assert_eq!(a.len(), 1);
        assert_eq!(b.len(), 2);
    }
}
