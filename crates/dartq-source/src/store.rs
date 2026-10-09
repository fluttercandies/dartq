//! Content-addressed storage of source bytes (PRD §5.2).

use rustc_hash::FxHashMap;

use crate::content_id::{ContentId, content_hash};

/// Content-addressed storage: bytes are named by their [`ContentId`], so
/// identical content occupies exactly one slot no matter how many files or
/// artifacts reference it.
///
/// The surface is synchronous and object-safe by design. Every intended
/// backend answers from storage that is already local — RAM now, mmap or a
/// disk cache later — so a call is a cheap lookup, not fallible remote IO. A
/// future file-backed store implements this same surface with blocking reads
/// on engine worker threads (the rust-analyzer model); if an async backend is
/// ever genuinely needed, it arrives as a separate extension trait and
/// existing callers are unaffected.
pub trait SourceStore {
    /// Stores `bytes` under its [`ContentId`] and returns that id.
    ///
    /// Storing bytes whose id is already present keeps the existing slot, so
    /// the store never holds two copies of the same content.
    fn put(&mut self, bytes: &[u8]) -> ContentId;

    /// Stores owned `bytes` under their [`ContentId`] without an intermediate
    /// copy, and returns that id.
    ///
    /// This is the zero-copy counterpart of [`SourceStore::put`] for callers
    /// that already own a `Vec<u8>` (the engine's overlay-to-apply path):
    /// the buffer is moved into the store instead of copied. As with `put`,
    /// an id that is already present keeps the existing slot and the incoming
    /// buffer is dropped.
    ///
    /// The default forwards to `put`, which copies once; backends that can
    /// take ownership override this.
    fn put_owned(&mut self, bytes: Vec<u8>) -> ContentId {
        self.put(&bytes)
    }

    /// Returns the bytes named by `id`, or `None` when unknown to this store.
    fn get(&self, id: ContentId) -> Option<&[u8]>;

    /// Drops the content named by `id`, returning whether it was present.
    ///
    /// Callers use this for garbage collection: once no live artifact
    /// references `id`, its bytes can be reclaimed. Removing an id that is
    /// later re-`put` is fine — content addressing makes the bytes identical.
    ///
    /// The default returns `false` and keeps everything: a backend without a
    /// reclamation story is honestly reported as "nothing removed" rather
    /// than pretending to evict.
    fn remove(&mut self, id: ContentId) -> bool {
        let _ = id;
        false
    }

    /// Reclaims every content that `is_live` does not vouch for, returning
    /// how many entries were removed.
    ///
    /// This is the batch form of [`SourceStore::remove`]: the caller owns the
    /// liveness policy — which ids its residents and caches still reference —
    /// and the store owns only the sweep. Content addressing keeps the sweep
    /// safe by construction: an id evicted here that is later re-`put`
    /// returns byte-identical under the same name.
    ///
    /// The default returns `0` and keeps everything, the same honest
    /// no-reclamation posture as the [`SourceStore::remove`] default.
    fn collect_garbage(&mut self, is_live: &dyn Fn(ContentId) -> bool) -> usize {
        let _ = is_live;
        0
    }

    /// Number of distinct contents held.
    fn len(&self) -> usize;

    /// Total bytes held across all distinct contents.
    ///
    /// This is the engine's observable memory budget: `len` alone cannot
    /// bound residency because contents differ wildly in size. The default
    /// `0` means the backend does not track its footprint.
    fn len_bytes(&self) -> u64 {
        0
    }

    /// Whether the store holds no content.
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// In-memory [`SourceStore`]: a map from [`ContentId`] to the stored bytes.
///
/// Tracks the running byte total on insertion, removal, and reclamation so
/// [`len_bytes`](SourceStore::len_bytes) is O(1) instead of a scan over every
/// entry.
#[derive(Debug, Default)]
pub struct MemorySourceStore {
    entries: FxHashMap<ContentId, Vec<u8>>,
    total_bytes: u64,
}

impl MemorySourceStore {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

impl SourceStore for MemorySourceStore {
    fn put(&mut self, bytes: &[u8]) -> ContentId {
        let id = content_hash(bytes);
        if let std::collections::hash_map::Entry::Vacant(entry) = self.entries.entry(id) {
            self.total_bytes += bytes.len() as u64;
            entry.insert(bytes.to_vec());
        }
        id
    }

    fn put_owned(&mut self, bytes: Vec<u8>) -> ContentId {
        let id = content_hash(&bytes);
        if let std::collections::hash_map::Entry::Vacant(entry) = self.entries.entry(id) {
            self.total_bytes += bytes.len() as u64;
            entry.insert(bytes);
        }
        id
    }

    fn get(&self, id: ContentId) -> Option<&[u8]> {
        self.entries.get(&id).map(Vec::as_slice)
    }

    fn remove(&mut self, id: ContentId) -> bool {
        match self.entries.remove(&id) {
            Some(bytes) => {
                self.total_bytes -= bytes.len() as u64;
                true
            }
            None => false,
        }
    }

    fn collect_garbage(&mut self, is_live: &dyn Fn(ContentId) -> bool) -> usize {
        let before = self.entries.len();
        let mut reclaimed = 0u64;
        self.entries.retain(|id, bytes| {
            if is_live(*id) {
                true
            } else {
                reclaimed += bytes.len() as u64;
                false
            }
        });
        self.total_bytes -= reclaimed;
        before - self.entries.len()
    }

    fn len(&self) -> usize {
        self.entries.len()
    }

    fn len_bytes(&self) -> u64 {
        self.total_bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content_id::content_hash;

    #[test]
    fn put_dedups_identical_content() {
        let mut store = MemorySourceStore::new();
        assert!(store.is_empty());

        let first = store.put(b"void main() {}");
        let second = store.put(b"void main() {}");
        assert_eq!(first, second);
        assert_eq!(store.len(), 1);
        assert!(!store.is_empty());
    }

    #[test]
    fn get_returns_exact_bytes() {
        let mut store = MemorySourceStore::new();
        let id = store.put(b"hello dartq");
        assert_eq!(store.get(id), Some(&b"hello dartq"[..]));
        assert_eq!(store.get(content_hash(b"never stored")), None);
        assert_eq!(store.get(content_hash(b"")), None);
    }

    #[test]
    fn empty_content_roundtrip() {
        let mut store = MemorySourceStore::new();
        let id = store.put(b"");
        assert_eq!(store.get(id), Some(&b""[..]));
        assert_eq!(store.len(), 1);
        assert_eq!(store.len_bytes(), 0);
    }

    #[test]
    fn remove_drops_content_and_updates_counts() {
        let mut store = MemorySourceStore::new();
        let a = store.put(b"aaaa");
        let b = store.put(b"bbbbbb");
        assert_eq!(store.len(), 2);
        assert_eq!(store.len_bytes(), 10);

        assert!(store.remove(a));
        assert_eq!(store.get(a), None);
        assert_eq!(store.len(), 1);
        assert_eq!(store.len_bytes(), 6);

        assert!(!store.remove(a), "removing twice reports not-present");
        assert!(!store.remove(content_hash(b"never stored")));
        assert_eq!(store.get(b), Some(&b"bbbbbb"[..]));

        assert!(store.remove(b));
        assert!(store.is_empty());
        assert_eq!(store.len_bytes(), 0);
    }

    #[test]
    fn put_owned_moves_bytes_in_and_dedups() {
        let mut store = MemorySourceStore::new();
        let first = store.put_owned(b"void main() {}".to_vec());
        let second = store.put_owned(b"void main() {}".to_vec());
        assert_eq!(first, second);
        assert_eq!(store.len(), 1, "owned double-put occupies one slot");
        assert_eq!(store.len_bytes(), 14, "no growth for identical content");

        // Mixed put/put_owned of the same content also stays one slot.
        assert_eq!(store.put(b"void main() {}"), first);
        assert_eq!(store.len(), 1);
        assert_eq!(store.len_bytes(), 14);
        assert_eq!(store.get(first), Some(&b"void main() {}"[..]));
    }

    /// T4 pin: the eviction round trip drives `len` and the `total_bytes`
    /// ledger back to exactly zero with no residue, and content addressing
    /// survives eviction — re-putting evicted bytes yields the same id with
    /// rebuilt accounting.
    #[test]
    fn eviction_round_trip_returns_total_to_zero() {
        let mut store = MemorySourceStore::new();

        let a = store.put(b"aaaa");
        assert_eq!(store.len(), 1);
        assert_eq!(store.len_bytes(), 4);

        assert!(store.remove(a));
        assert_eq!(store.len(), 0);
        assert_eq!(store.len_bytes(), 0);

        let b = store.put_owned(b"bbbbbb".to_vec());
        assert_ne!(a, b);
        assert_eq!(store.len(), 1);
        assert_eq!(store.len_bytes(), 6);

        assert!(store.remove(b));
        assert!(!store.remove(b), "already evicted");
        assert_eq!(store.len(), 0);
        assert_eq!(store.len_bytes(), 0);
        assert!(store.is_empty());

        assert_eq!(store.put(b"aaaa"), a, "same bytes, same content address");
        assert_eq!(store.len(), 1);
        assert_eq!(store.len_bytes(), 4);
        assert_eq!(store.get(a), Some(&b"aaaa"[..]));
    }

    /// Pin: `collect_garbage` keeps exactly the ids the liveness predicate
    /// vouches for (resident contents and cache-referenced ids), drops the
    /// rest with an exact byte-ledger adjustment, and re-putting a swept id
    /// rebuilds the slot under the same content address.
    #[test]
    fn collect_garbage_keeps_live_drops_dead() {
        let mut store = MemorySourceStore::new();
        let live = store.put(b"live");
        let cache_ref = store.put(b"cache-referenced");
        let dead_a = store.put(b"dead-a");
        let dead_b = store.put_owned(b"dead-b".to_vec());

        let vouched: Vec<ContentId> = vec![live, cache_ref];
        let is_live = |id: ContentId| vouched.contains(&id);
        assert_eq!(store.collect_garbage(&is_live), 2);

        assert_eq!(store.len(), 2);
        assert_eq!(store.len_bytes(), 4 + 16);
        assert_eq!(store.get(live), Some(&b"live"[..]));
        assert_eq!(store.get(cache_ref), Some(&b"cache-referenced"[..]));
        assert_eq!(store.get(dead_a), None);
        assert_eq!(store.get(dead_b), None);

        // A sweep over an all-live store removes nothing.
        assert_eq!(store.collect_garbage(&|_: ContentId| true), 0);

        // Content addressing: the swept id returns byte-identical.
        assert_eq!(store.put(b"dead-b"), dead_b);
        assert_eq!(store.get(dead_b), Some(&b"dead-b"[..]));

        // Sweeping everything empties the store exactly.
        assert_eq!(store.collect_garbage(&|_: ContentId| false), 3);
        assert!(store.is_empty());
        assert_eq!(store.len_bytes(), 0);
    }

    /// The trait defaults pin the honest semantics for backends that do not
    /// implement reclamation or byte tracking: `remove` reports nothing
    /// removed, `collect_garbage` reclaims nothing, `len_bytes` reports
    /// zero, and `put_owned` still routes the content through `put`.
    #[test]
    fn trait_defaults_cover_minimal_backends() {
        struct Minimal(Vec<(ContentId, Vec<u8>)>);

        impl SourceStore for Minimal {
            fn put(&mut self, bytes: &[u8]) -> ContentId {
                let id = content_hash(bytes);
                self.0.push((id, bytes.to_vec()));
                id
            }

            fn get(&self, id: ContentId) -> Option<&[u8]> {
                self.0
                    .iter()
                    .find(|(known, _)| *known == id)
                    .map(|(_, bytes)| bytes.as_slice())
            }

            fn len(&self) -> usize {
                self.0.len()
            }
        }

        let mut store = Minimal(Vec::new());
        let id = store.put_owned(b"owned by default".to_vec());
        assert_eq!(store.get(id), Some(&b"owned by default"[..]));
        assert!(!store.remove(id));
        assert_eq!(store.collect_garbage(&|_: ContentId| false), 0);
        assert_eq!(store.len_bytes(), 0);
    }
}
