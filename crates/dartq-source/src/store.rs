//! Content-addressed storage of source bytes.

use std::path::Path;

use parking_lot::Mutex;
use rustc_hash::FxHashMap;

use crate::content_id::{ContentId, content_hash};
use crate::text::{MappedRange, OpenError, SourceText, TextInner};

/// Content-addressed storage: bytes are named by their [`ContentId`], so
/// identical content occupies exactly one slot no matter how many files or
/// artifacts reference it.
///
/// The surface is synchronous and object-safe by design. Every intended
/// backend answers from storage that is already local — [`MemorySourceStore`]
/// and [`PooledSourceStore`] hold RAM, [`MmapSourceStore`] holds read-only
/// file mappings — so a call is a cheap lookup, not fallible remote IO (the
/// engine-worker blocking-read posture of the upstream rust-analyzer VFS).
/// A future async backend, if ever genuinely needed, arrives as a separate
/// extension trait and existing callers are unaffected.
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

    /// Stores canonical [`SourceText`] under its precomputed id and returns
    /// that id — no second hash pass, and for backends holding file
    /// mappings no copy at all: the text's mapping lease is shared into the
    /// slot by refcount.
    ///
    /// The default forwards to [`SourceStore::put`] (one hash + one copy);
    /// backends that can honor the carried id override this.
    fn put_text(&mut self, text: &SourceText) -> ContentId {
        self.put(text.as_bytes())
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

    /// Bytes served out of read-only file mappings — the evictable
    /// page-cache share of [`len_bytes`](SourceStore::len_bytes). A
    /// heap-only backend answers `0` (the default); a mapping-backed one
    /// reports the mapped share so observers can split resident-anonymous
    /// bytes from evictable ones.
    fn mapped_bytes(&self) -> u64 {
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

/// Byte-level content dedup across stores: every store constructed over one
/// pool holds each distinct content exactly once between them — the
/// multi-context store face (one base, N context views). The pool is the
/// shared instance (`Arc` inside, [`Clone`] is a handle); each store keeps
/// its own `ContentId` → slot map, so reads stay lock-free zero-copy and
/// each store's reclamation path stays its own.
///
/// Pool reclamation is reference-count-driven: a slot held by no store is
/// dead (content addressing reproduces it byte-identically if the content
/// is ever stored again), and the pool amortizes its sweep the way the
/// engine's own stores amortize theirs — after `POOL_GC_THRESHOLD` fresh
/// insertions. A store's `Drop` alone never reclaims: the next pool sweep
/// does, so a closed context's bytes keep serving its live siblings.
#[derive(Clone, Debug, Default)]
pub struct ContentPool {
    inner: std::sync::Arc<Mutex<PoolInner>>,
}

#[derive(Debug, Default)]
struct PoolInner {
    entries: FxHashMap<ContentId, std::sync::Arc<[u8]>>,
    /// Fresh insertions since the last sweep — the amortization tally.
    new_since_sweep: usize,
    total_bytes: u64,
}

/// Fresh pool insertions that trigger one reference-count sweep.
const POOL_GC_THRESHOLD: usize = 1024;

impl ContentPool {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Distinct contents the pool holds — the deduplicated entry count.
    #[must_use]
    pub fn len(&self) -> usize {
        self.inner.lock().entries.len()
    }

    /// Whether the pool holds no content.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Total bytes the pool holds — the deduplicated footprint: each
    /// distinct content counted once, unlike a member store's view total.
    #[must_use]
    pub fn len_bytes(&self) -> u64 {
        self.inner.lock().total_bytes
    }

    /// The slot for `id`, inserting `bytes` under it when the pool has not
    /// seen the content yet. The caller guarantees `id == content_hash(bytes)`.
    fn get_or_insert(&self, id: ContentId, bytes: &[u8]) -> std::sync::Arc<[u8]> {
        let mut inner = self.inner.lock();
        if inner.new_since_sweep >= POOL_GC_THRESHOLD {
            // A slot held by no store is dead. Sweeping before the new
            // insertion lands keeps the just-stored content out of this
            // sweep's way entirely.
            let mut reclaimed = 0u64;
            inner.entries.retain(|_, slot| {
                if std::sync::Arc::strong_count(slot) > 1 {
                    true
                } else {
                    reclaimed += slot.len() as u64;
                    false
                }
            });
            inner.total_bytes -= reclaimed;
            inner.new_since_sweep = 0;
        }
        if !inner.entries.contains_key(&id) {
            let slot: std::sync::Arc<[u8]> = std::sync::Arc::from(bytes);
            inner.total_bytes += bytes.len() as u64;
            inner.new_since_sweep += 1;
            inner.entries.insert(id, std::sync::Arc::clone(&slot));
        }
        std::sync::Arc::clone(&inner.entries[&id])
    }
}

/// A [`SourceStore`] wired to a [`ContentPool`]: fresh contents dedup
/// through the pool, everything else behaves exactly like
/// [`MemorySourceStore`] — the per-store map is exclusively owned, so `get`
/// stays zero-copy and [`collect_garbage`](SourceStore::collect_garbage)
/// only ever drops this store's own handles.
#[derive(Debug)]
pub struct PooledSourceStore {
    pool: ContentPool,
    entries: FxHashMap<ContentId, std::sync::Arc<[u8]>>,
    total_bytes: u64,
}

impl PooledSourceStore {
    /// A store whose fresh contents dedup through `pool`.
    #[must_use]
    pub fn new(pool: &ContentPool) -> Self {
        Self {
            pool: pool.clone(),
            entries: FxHashMap::default(),
            total_bytes: 0,
        }
    }
}

impl SourceStore for PooledSourceStore {
    fn put(&mut self, bytes: &[u8]) -> ContentId {
        let id = content_hash(bytes);
        if let std::collections::hash_map::Entry::Vacant(entry) = self.entries.entry(id) {
            self.total_bytes += bytes.len() as u64;
            entry.insert(self.pool.get_or_insert(id, bytes));
        }
        id
    }

    fn put_owned(&mut self, bytes: Vec<u8>) -> ContentId {
        let id = content_hash(&bytes);
        if let std::collections::hash_map::Entry::Vacant(entry) = self.entries.entry(id) {
            self.total_bytes += bytes.len() as u64;
            let slot: std::sync::Arc<[u8]> = std::sync::Arc::from(bytes);
            let pooled = self.pool.get_or_insert(id, &slot);
            entry.insert(pooled);
        }
        id
    }

    fn get(&self, id: ContentId) -> Option<&[u8]> {
        self.entries.get(&id).map(std::sync::Arc::as_ref)
    }

    fn remove(&mut self, id: ContentId) -> bool {
        match self.entries.remove(&id) {
            Some(slot) => {
                self.total_bytes -= slot.len() as u64;
                true
            }
            None => false,
        }
    }

    fn collect_garbage(&mut self, is_live: &dyn Fn(ContentId) -> bool) -> usize {
        let before = self.entries.len();
        let mut reclaimed = 0u64;
        self.entries.retain(|id, slot| {
            if is_live(*id) {
                true
            } else {
                reclaimed += slot.len() as u64;
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

/// The mapped-pool companion of [`ContentPool`]: same cross-store dedup
/// contract (stores wired to one pool hold each distinct content exactly
/// once between them, one refcounted slot per [`ContentId`]), but the slot
/// is a [`Slot`] — a [`SourceText`] lease or raw bytes — so a
/// [`PooledMappedSourceStore`]'s [`put_text`](SourceStore::put_text) shares
/// the incoming mapping lease into the pool by refcount. Canonical file
/// text stays on evictable page-cache pages instead of being copied onto
/// the anonymous heap (the pool face of [`MmapSourceStore`]'s posture),
/// while `put`/`put_owned` byte inserts keep the owned-slot path.
///
/// Reclamation is the pool's standing contract: a slot held by no store is
/// dead, swept amortized after `MAPPED_POOL_GC_THRESHOLD` fresh insertions;
/// a store's `Drop` never reclaims, so a closed context's bytes keep
/// serving its live siblings until the next sweep.
#[derive(Clone, Debug, Default)]
pub struct MappedContentPool {
    inner: std::sync::Arc<Mutex<MappedPoolInner>>,
}

#[derive(Debug, Default)]
struct MappedPoolInner {
    entries: FxHashMap<ContentId, Slot>,
    /// Fresh insertions since the last sweep — the amortization tally.
    new_since_sweep: usize,
    total_bytes: u64,
    /// The mapped share of `total_bytes` — the PSS-split probe face.
    mapped_bytes: u64,
}

/// Fresh pool insertions that trigger one reference-count sweep (the same
/// amortization cadence as [`POOL_GC_THRESHOLD`]).
const MAPPED_POOL_GC_THRESHOLD: usize = POOL_GC_THRESHOLD;

impl MappedContentPool {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Distinct contents the pool holds — the deduplicated entry count.
    #[must_use]
    pub fn len(&self) -> usize {
        self.inner.lock().entries.len()
    }

    /// Whether the pool holds no content.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Total bytes the pool holds — the deduplicated footprint.
    #[must_use]
    pub fn len_bytes(&self) -> u64 {
        self.inner.lock().total_bytes
    }

    /// Bytes the pool holds in file-mapping slots — the evictable
    /// page-cache share of [`len_bytes`](MappedContentPool::len_bytes)
    /// (the resident-anonymous vs page-cache split the serial-phase PSS
    /// probe reads).
    #[must_use]
    pub fn mapped_bytes(&self) -> u64 {
        self.inner.lock().mapped_bytes
    }

    /// The slot for `id`, inserting `bytes` under it when the pool has not
    /// seen the content yet. The caller guarantees `id == content_hash(bytes)`.
    /// A dedup hit costs no copy — the existing slot's handle is returned.
    fn get_or_insert(&self, id: ContentId, bytes: &[u8]) -> Slot {
        let mut inner = self.inner.lock();
        sweep_mapped_pool(&mut inner);
        if !inner.entries.contains_key(&id) {
            inner.total_bytes += bytes.len() as u64;
            inner.new_since_sweep += 1;
            inner
                .entries
                .insert(id, Slot::Bytes(std::sync::Arc::from(bytes)));
        }
        inner.entries[&id].clone()
    }

    /// The owned-bytes counterpart of [`get_or_insert`](Self::get_or_insert):
    /// `bytes` moves into the slot without a second copy (`put_owned`'s
    /// path). The caller guarantees `id == content_hash(&bytes)`.
    fn get_or_insert_owned(&self, id: ContentId, bytes: std::sync::Arc<[u8]>) -> Slot {
        let mut inner = self.inner.lock();
        sweep_mapped_pool(&mut inner);
        if !inner.entries.contains_key(&id) {
            inner.total_bytes += bytes.len() as u64;
            inner.new_since_sweep += 1;
            inner.entries.insert(id, Slot::Bytes(bytes));
        }
        inner.entries[&id].clone()
    }

    /// The slot for `id`, sharing `text`'s lease into the pool when the
    /// content is new — a mapping lease stays a mapping (zero-copy), an
    /// owned lease stays owned. The caller guarantees
    /// `id == text.content_id()`.
    fn get_or_insert_text(&self, id: ContentId, text: &SourceText) -> Slot {
        let mut inner = self.inner.lock();
        sweep_mapped_pool(&mut inner);
        if !inner.entries.contains_key(&id) {
            let slot = Slot::Text(text.inner().clone());
            inner.total_bytes += slot.len() as u64;
            if slot.is_mapped() {
                inner.mapped_bytes += slot.len() as u64;
            }
            inner.new_since_sweep += 1;
            inner.entries.insert(id, slot);
        }
        inner.entries[&id].clone()
    }
}

/// The pool sweep: a slot held by no store is dead (content addressing
/// reproduces it byte-identically if re-stored). Runs once per
/// `MAPPED_POOL_GC_THRESHOLD` fresh insertions, ahead of the new insertion
/// so the just-stored content is never swept by its own trigger.
fn sweep_mapped_pool(inner: &mut MappedPoolInner) {
    if inner.new_since_sweep < MAPPED_POOL_GC_THRESHOLD {
        return;
    }
    let mut reclaimed = 0u64;
    let mut reclaimed_mapped = 0u64;
    inner.entries.retain(|_, slot| {
        if slot.strong_count() > 1 {
            true
        } else {
            reclaimed += slot.len() as u64;
            if slot.is_mapped() {
                reclaimed_mapped += slot.len() as u64;
            }
            false
        }
    });
    inner.total_bytes -= reclaimed;
    inner.mapped_bytes -= reclaimed_mapped;
    inner.new_since_sweep = 0;
}

/// A [`SourceStore`] wired to a [`MappedContentPool`]: the pooled-store
/// contract over mapping-capable slots. Fresh contents dedup through the
/// pool — `put`/`put_owned` land as owned byte slots,
/// [`put_text`](SourceStore::put_text) shares the text's lease (a mapping
/// lease keeps the content on page-cache pages, shared across every pooled
/// store by one `Arc`). The per-store map is exclusively owned, so `get`
/// stays zero-copy and
/// [`collect_garbage`](SourceStore::collect_garbage) only ever drops this
/// store's own handles — identical to [`PooledSourceStore`]'s discipline.
#[derive(Debug)]
pub struct PooledMappedSourceStore {
    pool: MappedContentPool,
    entries: FxHashMap<ContentId, Slot>,
    total_bytes: u64,
    /// The mapped share of `total_bytes` — this store's view of the
    /// page-cache-backed content it holds.
    mapped_bytes: u64,
}

impl PooledMappedSourceStore {
    /// A store whose fresh contents dedup through `pool`, keeping mapping
    /// leases mapped.
    #[must_use]
    pub fn new(pool: &MappedContentPool) -> Self {
        Self {
            pool: pool.clone(),
            entries: FxHashMap::default(),
            total_bytes: 0,
            mapped_bytes: 0,
        }
    }
}

impl PooledMappedSourceStore {
    /// Books `slot` into this store's view under `id`: the byte ledgers
    /// account what the pool actually handed back — a `put` over bytes
    /// already pooled as a mapping holds the mapped slot, so the mapped
    /// tally reads the slot, not the call shape.
    fn insert_slot(&mut self, id: ContentId, slot: Slot) {
        self.total_bytes += slot.len() as u64;
        if slot.is_mapped() {
            self.mapped_bytes += slot.len() as u64;
        }
        self.entries.insert(id, slot);
    }
}

impl SourceStore for PooledMappedSourceStore {
    fn put(&mut self, bytes: &[u8]) -> ContentId {
        let id = content_hash(bytes);
        if !self.entries.contains_key(&id) {
            let slot = self.pool.get_or_insert(id, bytes);
            self.insert_slot(id, slot);
        }
        id
    }

    fn put_owned(&mut self, bytes: Vec<u8>) -> ContentId {
        let id = content_hash(&bytes);
        if !self.entries.contains_key(&id) {
            // `Arc::from(Vec)` reuses the buffer's allocation; the pool sees
            // the bytes through it (the same shape `PooledSourceStore` uses).
            let slot = self
                .pool
                .get_or_insert_owned(id, std::sync::Arc::from(bytes));
            self.insert_slot(id, slot);
        }
        id
    }

    fn put_text(&mut self, text: &SourceText) -> ContentId {
        let id = text.content_id();
        if !self.entries.contains_key(&id) {
            let slot = self.pool.get_or_insert_text(id, text);
            self.insert_slot(id, slot);
        }
        id
    }

    fn get(&self, id: ContentId) -> Option<&[u8]> {
        self.entries.get(&id).map(Slot::as_bytes)
    }

    fn remove(&mut self, id: ContentId) -> bool {
        match self.entries.remove(&id) {
            Some(slot) => {
                self.total_bytes -= slot.len() as u64;
                if slot.is_mapped() {
                    self.mapped_bytes -= slot.len() as u64;
                }
                true
            }
            None => false,
        }
    }

    fn collect_garbage(&mut self, is_live: &dyn Fn(ContentId) -> bool) -> usize {
        let before = self.entries.len();
        let mut reclaimed = 0u64;
        let mut reclaimed_mapped = 0u64;
        self.entries.retain(|id, slot| {
            if is_live(*id) {
                true
            } else {
                reclaimed += slot.len() as u64;
                if slot.is_mapped() {
                    reclaimed_mapped += slot.len() as u64;
                }
                false
            }
        });
        self.total_bytes -= reclaimed;
        self.mapped_bytes -= reclaimed_mapped;
        before - self.entries.len()
    }

    fn len(&self) -> usize {
        self.entries.len()
    }

    fn len_bytes(&self) -> u64 {
        self.total_bytes
    }

    fn mapped_bytes(&self) -> u64 {
        self.mapped_bytes
    }
}

/// One CAS slot: a shared text lease (owned `Arc<str>` or a file-mapping
/// lease handed in by [`SourceText`]) or the raw byte face of the legacy
/// `put`/`put_owned` methods.
#[derive(Clone, Debug)]
enum Slot {
    Text(TextInner),
    Bytes(std::sync::Arc<[u8]>),
}

impl Slot {
    fn as_bytes(&self) -> &[u8] {
        match self {
            Slot::Text(TextInner::Owned(text)) => text.as_bytes(),
            Slot::Text(TextInner::Mapped(mapped)) => mapped.as_str().as_bytes(),
            Slot::Bytes(bytes) => bytes,
        }
    }

    fn len(&self) -> usize {
        match self {
            Slot::Text(TextInner::Owned(text)) => text.len(),
            Slot::Text(TextInner::Mapped(mapped)) => mapped.as_str().len(),
            Slot::Bytes(bytes) => bytes.len(),
        }
    }

    /// The slot's refcount — every variant is a single `Arc` underneath, so
    /// the pool sweep's liveness test is uniform across slot kinds.
    fn strong_count(&self) -> usize {
        match self {
            Slot::Text(TextInner::Owned(text)) => std::sync::Arc::strong_count(text),
            Slot::Text(TextInner::Mapped(mapped)) => std::sync::Arc::strong_count(mapped),
            Slot::Bytes(bytes) => std::sync::Arc::strong_count(bytes),
        }
    }

    /// Whether the slot's bytes live in a read-only file mapping — the
    /// evictable page-cache share of a store's footprint.
    fn is_mapped(&self) -> bool {
        matches!(self, Slot::Text(TextInner::Mapped(_)))
    }
}

/// The file-backed [`SourceStore`]: canonical text lives in read-only file
/// mappings and owned slots, keyed by [`ContentId`] — the CAS layout the
/// engine's source layer is built around (the upstream rust-analyzer VFS's
/// shape, content-addressed: N identical files cost one mapping).
///
/// # Zero-copy faces
///
/// - [`load_file`](MmapSourceStore::load_file) maps the file, strips one
///   BOM into the mapped range, validates UTF-8 once (SIMD via
///   `simdutf8`), hashes once, and registers the mapping lease — no byte
///   is copied off the page cache. Invalid-UTF-8 files fall back to an
///   owned canonical slot (rare, and identical in behavior to the owned
///   store).
/// - [`put_text`](SourceStore::put_text) shares the text's lease into the
///   slot by refcount under its carried id — no re-hash, no copy.
/// - [`get`](SourceStore::get) borrows the slot's bytes — no per-access
///   validation or conversion.
///
/// # Boundedness
///
/// The table holds at most one slot per distinct content of the files put
/// into it; the engine's liveness face ([`remove`](SourceStore::remove) /
/// [`collect_garbage`](SourceStore::collect_garbage), driven by residency)
/// is the bound — the same contract as [`MemorySourceStore`], strictly
/// better paged: mapped slots are clean file-backed pages the kernel
/// evicts under memory pressure without process OOM, degrading to page
/// re-faults on the next access. An unbounded corpus is the caller's
/// choice, as with the owned store.
///
/// # Mutation contract
///
/// Mappings alias the file for the slot's lifetime; the file must not be
/// truncated in place while mapped (see the mapping lifetime contract on
/// [`SourceText`]). Mutations reach the engine through the overlay, which
/// replaces text wholesale and retires mappings by refcount.
#[derive(Debug, Default)]
pub struct MmapSourceStore {
    entries: FxHashMap<ContentId, Slot>,
    total_bytes: u64,
    /// The mapped share of `total_bytes` (slots holding file mappings —
    /// [`SourceStore::mapped_bytes`] is O(1) off this ledger instead of a
    /// per-entry scan).
    mapped_bytes: u64,
}

impl MmapSourceStore {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Maps the file and registers its canonical text under the CAS table
    /// (the [`open_mapped`](crate::open_mapped) loader plus
    /// [`SourceStore::put_text`]). The workspace walk uses the two pieces
    /// separately so mappings are taken in parallel and inserted in load
    /// order; single loads take this shortcut.
    ///
    /// # Errors
    ///
    /// The [`open_mapped`](crate::open_mapped) failures: file open/map
    /// I/O errors and the span-capacity rejection.
    pub fn load_file(&mut self, path: &Path) -> Result<SourceText, OpenError> {
        let text = MappedRange::open(path)?;
        self.put_text(&text);
        Ok(text)
    }
}

impl SourceStore for MmapSourceStore {
    fn put(&mut self, bytes: &[u8]) -> ContentId {
        let id = content_hash(bytes);
        if let std::collections::hash_map::Entry::Vacant(entry) = self.entries.entry(id) {
            self.total_bytes += bytes.len() as u64;
            entry.insert(Slot::Bytes(std::sync::Arc::from(bytes)));
        }
        id
    }

    fn put_owned(&mut self, bytes: Vec<u8>) -> ContentId {
        let id = content_hash(&bytes);
        if let std::collections::hash_map::Entry::Vacant(entry) = self.entries.entry(id) {
            self.total_bytes += bytes.len() as u64;
            entry.insert(Slot::Bytes(std::sync::Arc::from(bytes)));
        }
        id
    }

    fn put_text(&mut self, text: &SourceText) -> ContentId {
        let id = text.content_id();
        if let std::collections::hash_map::Entry::Vacant(entry) = self.entries.entry(id) {
            let slot = Slot::Text(text.inner().clone());
            self.total_bytes += slot.len() as u64;
            if slot.is_mapped() {
                self.mapped_bytes += slot.len() as u64;
            }
            entry.insert(slot);
        }
        id
    }

    fn get(&self, id: ContentId) -> Option<&[u8]> {
        self.entries.get(&id).map(Slot::as_bytes)
    }

    fn remove(&mut self, id: ContentId) -> bool {
        match self.entries.remove(&id) {
            Some(slot) => {
                self.total_bytes -= slot.len() as u64;
                if slot.is_mapped() {
                    self.mapped_bytes -= slot.len() as u64;
                }
                true
            }
            None => false,
        }
    }

    fn collect_garbage(&mut self, is_live: &dyn Fn(ContentId) -> bool) -> usize {
        let before = self.entries.len();
        let mut reclaimed = 0u64;
        let mut reclaimed_mapped = 0u64;
        self.entries.retain(|id, slot| {
            if is_live(*id) {
                true
            } else {
                reclaimed += slot.len() as u64;
                if slot.is_mapped() {
                    reclaimed_mapped += slot.len() as u64;
                }
                false
            }
        });
        self.total_bytes -= reclaimed;
        self.mapped_bytes -= reclaimed_mapped;
        before - self.entries.len()
    }

    fn len(&self) -> usize {
        self.entries.len()
    }

    fn len_bytes(&self) -> u64 {
        self.total_bytes
    }

    fn mapped_bytes(&self) -> u64 {
        self.mapped_bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content_id::content_hash;
    use crate::open_mapped;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// Two stores over one pool hold each distinct content exactly once:
    /// identical puts land on the same slot, the pool's footprint counts it
    /// once, and each store's own view totals behave like a standalone
    /// store's.
    #[test]
    fn pooled_stores_share_byte_storage() {
        let pool = ContentPool::new();
        let mut a = PooledSourceStore::new(&pool);
        let mut b = PooledSourceStore::new(&pool);
        let id = a.put(b"void main() {}");
        assert_eq!(b.put(b"void main() {}"), id, "content addressing is global");
        assert_eq!(pool.len(), 1);
        assert_eq!(pool.len_bytes(), 14);
        assert_eq!(a.len(), 1);
        assert_eq!(b.len(), 1);
        assert_eq!(
            a.len_bytes() + b.len_bytes(),
            28,
            "view totals are per-store"
        );
        assert_eq!(a.get(id), Some(&b"void main() {}"[..]));
        assert_eq!(b.get(id), Some(&b"void main() {}"[..]));

        // Fresh content inserts through the pool; `put_owned` moves. Views
        // stay per-store: only a store that put content itself reads it.
        let owned = b"void main() {}".to_vec();
        assert_eq!(a.put_owned(owned), id, "owned insert keeps the slot");
        let fresh = b"// fresh";
        let fresh_id = a.put_owned(fresh.to_vec());
        assert_eq!(pool.len(), 2);
        assert_eq!(a.get(fresh_id), Some(&fresh[..]));
        assert_eq!(b.get(fresh_id), None, "views are per-store");
    }

    /// The mapped-pool dedup contract, byte path: two stores over one
    /// `MappedContentPool` hold each distinct content once (the same
    /// identity pin as `pooled_stores_share_byte_storage`), and `put`/
    /// `put_owned` land as owned byte slots — `mapped_bytes` stays zero.
    #[test]
    fn mapped_pooled_stores_share_byte_storage() {
        let pool = MappedContentPool::new();
        let mut a = PooledMappedSourceStore::new(&pool);
        let mut b = PooledMappedSourceStore::new(&pool);
        let id = a.put(b"void main() {}");
        assert_eq!(b.put(b"void main() {}"), id, "content addressing is global");
        assert_eq!(pool.len(), 1);
        assert_eq!(pool.len_bytes(), 14);
        assert_eq!(
            a.len_bytes() + b.len_bytes(),
            28,
            "view totals are per-store"
        );
        assert_eq!(a.get(id), Some(&b"void main() {}"[..]));
        assert_eq!(b.get(id), Some(&b"void main() {}"[..]));
        assert_eq!(pool.mapped_bytes(), 0, "byte inserts are owned slots");
        assert_eq!(a.mapped_bytes(), 0);

        let owned = b"void main() {}".to_vec();
        assert_eq!(a.put_owned(owned), id, "owned insert keeps the slot");
        assert_eq!(pool.len(), 1);
    }

    /// The mmap-pool lever: `put_text` of a file-mapped text shares the
    /// mapping lease into the pool — the slot stays file-backed (evictable
    /// page cache, not anonymous heap), both pooled stores read the same
    /// `Arc` lease, and `mapped_bytes` reports the page-cache share. An
    /// owned text (the overlay face) lands owned and unmetered as mapped.
    #[test]
    fn mapped_pool_keeps_text_leases_mapped() {
        let file = TempFile::write("pool-mapped", b"var x = 1;\n");
        let text = open_mapped(file.path()).expect("load");
        assert!(text.is_mapped());

        let pool = MappedContentPool::new();
        let mut a = PooledMappedSourceStore::new(&pool);
        let mut b = PooledMappedSourceStore::new(&pool);
        let id = a.put_text(&text);
        assert_eq!(b.put_text(&text), id, "the lease dedups by content id");
        assert_eq!(pool.len(), 1);
        assert_eq!(pool.mapped_bytes(), 11);
        assert_eq!(a.mapped_bytes(), 11);
        assert_eq!(b.mapped_bytes(), 11);
        assert_eq!(a.get(id), Some(&b"var x = 1;\n"[..]));
        assert_eq!(b.get(id), Some(&b"var x = 1;\n"[..]));

        // Mixed spellings dedup by id: a `put` of identical bytes keeps the
        // mapped slot (first writer wins the lease, identity never forks).
        assert_eq!(a.put(b"var x = 1;\n"), id);
        assert_eq!(pool.len(), 1);
        assert_eq!(pool.mapped_bytes(), 11);

        // An owned text is content too — stored unmetered.
        let owned = SourceText::owned("var y = 2;\n".to_owned());
        let owned_id = a.put_text(&owned);
        assert_eq!(pool.len(), 2);
        assert_eq!(pool.mapped_bytes(), 11, "owned leases are not mapped");
        assert_eq!(a.get(owned_id), Some(&b"var y = 2;\n"[..]));
    }

    /// Mapped-pool GC is per-store and the byte/mapped ledgers drain exactly:
    /// a sibling's handle keeps the slot alive, `remove` and the sweep report
    /// honest counts, and re-putting swept content reproduces the id.
    #[test]
    fn mapped_pooled_gc_is_per_store() {
        let file = TempFile::write("pool-gc", b"payload\n");
        let text = open_mapped(file.path()).expect("load");
        let pool = MappedContentPool::new();
        let mut a = PooledMappedSourceStore::new(&pool);
        let mut b = PooledMappedSourceStore::new(&pool);
        let id = a.put_text(&text);
        assert_eq!(b.put(b"payload\n"), id, "same content, same id");

        assert_eq!(a.collect_garbage(&|_: ContentId| false), 1);
        assert_eq!(a.get(id), None);
        assert_eq!(b.get(id), Some(&b"payload\n"[..]));
        assert_eq!(pool.len(), 1, "the sibling's handle keeps the slot");
        assert_eq!(pool.mapped_bytes(), 8);

        assert!(b.remove(id));
        assert_eq!(b.mapped_bytes(), 0);
        assert_eq!(b.len_bytes(), 0);
        // The `text` lease itself pins the slot's refcount — drop it so the
        // pool's entry is dead (a held lease keeps mapped bytes alive by
        // design; the slot outliving its store entries is the contract).
        drop(text);
        // The dead slot lingers in the pool until the threshold-crossing
        // fresh insert sweeps it — the pool's amortized contract.
        let mut churn = PooledMappedSourceStore::new(&pool);
        for i in 0..MAPPED_POOL_GC_THRESHOLD {
            churn.put(format!("churn {i}").as_bytes());
        }
        assert_eq!(pool.len(), churn.len(), "the dead slot was swept");
        assert_eq!(pool.mapped_bytes(), 0);
        assert_eq!(
            churn.put(b"payload\n"),
            id,
            "content addressing restores it"
        );
    }

    /// A store's reclamation drops only its own handles: a sibling keeps
    /// reading shared bytes after this store collected them as dead, and
    /// the pool entry survives for exactly as long as some store holds it.
    #[test]
    fn pooled_gc_is_per_store_and_bytes_outlive_dropped_handles() {
        let pool = ContentPool::new();
        let mut a = PooledSourceStore::new(&pool);
        let mut b = PooledSourceStore::new(&pool);
        let id = a.put(b"shared bytes");
        assert_eq!(b.put(b"shared bytes"), id);

        assert_eq!(
            a.collect_garbage(&|_: ContentId| false),
            1,
            "this store's handle is dropped"
        );
        assert_eq!(a.get(id), None);
        assert_eq!(
            b.get(id),
            Some(&b"shared bytes"[..]),
            "the sibling's view is untouched"
        );
        assert_eq!(pool.len(), 1, "the pool entry serves the live sibling");

        // Once no store holds the slot, the entry is dead by refcount; the
        // threshold-crossing fresh insert sweeps it, and a re-put
        // reproduces the id byte-identically.
        drop(b);
        let mut churn = PooledSourceStore::new(&pool);
        for i in 0..(POOL_GC_THRESHOLD as u64) {
            churn.put(format!("churn {i}").as_bytes());
        }
        assert_eq!(
            churn.get(id),
            None,
            "no store put it, so this store's view is empty"
        );
        assert_eq!(
            churn.put(b"shared bytes"),
            id,
            "content addressing restores it"
        );
        assert_eq!(
            pool.len(),
            churn.len(),
            "the dead entry was swept; the pool holds churn's live view"
        );
        assert_eq!(pool.len_bytes(), churn.len_bytes());
    }

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

    /// A temp file that removes itself on drop.
    struct TempFile(PathBuf);

    impl TempFile {
        fn write(tag: &str, bytes: &[u8]) -> Self {
            static COUNTER: AtomicU32 = AtomicU32::new(0);
            let path = std::env::temp_dir().join(format!(
                "dartq-source-file-store-{tag}-{}-{}",
                std::process::id(),
                COUNTER.fetch_add(1, Ordering::Relaxed)
            ));
            fs::write(&path, bytes).expect("write temp file");
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempFile {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.0);
        }
    }

    /// `load_file` maps the file and registers it under the canonical
    /// text's content id — the same id the owned store assigns to the
    /// canonicalized bytes. This is the cross-backend identity pin: the
    /// mmap store and the memory store must never fork a file's identity.
    #[test]
    fn load_file_identity_matches_the_owned_store() {
        let cases: [(&str, &[u8]); 4] = [
            ("plain", b"void main {}"),
            ("bom", "\u{FEFF}void main {}".as_bytes()),
            ("empty", b""),
            ("invalid", b"a\xffb"),
        ];
        for (tag, bytes) in cases {
            let file = TempFile::write(tag, bytes);
            let mut mapped = MmapSourceStore::new();
            let mut owned = MemorySourceStore::new();
            let text = mapped.load_file(file.path()).expect("load");
            let reference = owned.put(SourceText::canonical(bytes).as_bytes());
            assert_eq!(text.content_id(), reference, "`{tag}` identity forks");
            assert_eq!(
                mapped.get(text.content_id()),
                owned.get(reference),
                "`{tag}` bytes fork across backends"
            );
            assert_eq!(mapped.len(), 1);
            assert_eq!(mapped.len_bytes(), u64::try_from(text.len()).unwrap());
        }
    }

    /// N identical files cost one mapping: the second `load_file` of the
    /// same bytes dedups into the existing slot (the CAS table never grows)
    /// and both leases serve identical text.
    #[test]
    fn identical_files_collapse_to_one_slot() {
        let a = TempFile::write("dup-a", b"shared body\n");
        let b = TempFile::write("dup-b", b"shared body\n");
        let mut store = MmapSourceStore::new();
        let first = store.load_file(a.path()).expect("load a");
        let second = store.load_file(b.path()).expect("load b");
        assert_eq!(first.content_id(), second.content_id());
        assert_eq!(store.len(), 1, "CAS dedup");
        assert_eq!(store.len_bytes(), 12);
        assert_eq!(store.get(first.content_id()), Some(&b"shared body\n"[..]));

        // BOM'd vs stripped spellings of the same text collapse too: the
        // canonical text is the identity, not the file bytes.
        let bom = TempFile::write("dup-bom", "\u{FEFF}shared body\n".as_bytes());
        let bom_text = store.load_file(bom.path()).expect("load bom");
        assert_eq!(bom_text.content_id(), first.content_id());
        assert_eq!(store.len(), 1);
    }

    /// Mapped loads alias the file (zero-copy), not a heap copy.
    #[test]
    fn load_file_aliases_the_mapping_for_valid_utf8() {
        let file = TempFile::write("mapped", b"var x = 1;\n");
        let mut store = MmapSourceStore::new();
        let text = store.load_file(file.path()).expect("load");
        assert!(text.is_mapped(), "valid UTF-8 without BOM stays mapped");
        assert_eq!(text.as_str(), "var x = 1;\n");
        assert_eq!(store.get(text.content_id()), Some(&b"var x = 1;\n"[..]));
    }

    /// A missing file's I/O error surfaces as-is; nothing enters the table.
    #[test]
    fn load_file_missing_path_is_an_io_error() {
        let mut store = MmapSourceStore::new();
        let missing = std::env::temp_dir().join("dartq-source-never-here.dart");
        assert!(store.load_file(&missing).is_err());
        assert!(store.is_empty());
    }

    /// The reclamation faces work on mapped slots exactly like owned ones:
    /// a removed id's lease survives in the caller's hand, the slot is gone,
    /// and re-putting the file restores it byte-identically under the same
    /// content address.
    #[test]
    fn removal_and_reput_round_trip_mapped_slots() {
        let file = TempFile::write("evict", b"payload");
        let mut store = MmapSourceStore::new();
        let text = store.load_file(file.path()).expect("load");
        let id = text.content_id();

        assert!(store.remove(id));
        assert_eq!(store.get(id), None);
        assert_eq!(store.len_bytes(), 0);
        assert_eq!(text.as_str(), "payload", "the lease outlives the slot");

        let reloaded = store.load_file(file.path()).expect("reload");
        assert_eq!(reloaded.content_id(), id, "content addressing restores it");
        assert_eq!(store.len_bytes(), 7);
    }

    /// `put_text` honors the carried id: inserting the same text twice (and
    /// a `put` of the same bytes) lands one slot, and mapped + owned
    /// spellings of identical text dedup against each other.
    #[test]
    fn put_text_dedups_by_the_carried_id() {
        let mut store = MmapSourceStore::new();
        let owned = SourceText::owned("void f {}".to_string());
        assert_eq!(store.put_text(&owned), owned.content_id());
        assert_eq!(store.put_text(&owned), owned.content_id());
        assert_eq!(store.put(b"void f {}"), owned.content_id());
        assert_eq!(store.len(), 1);
        assert_eq!(store.len_bytes(), 9);
        assert_eq!(store.get(owned.content_id()), Some(&b"void f {}"[..]));
    }

    /// The corrupt-file path (the file-backed store's trust story): content
    /// is addressed by the blake3 hash of the bytes actually read, so a
    /// corrupted or truncated file can never answer a stale id — the id is
    /// re-derived from the bytes at load, the stale id misses with `None`,
    /// and the lossy-decode fallback for bytes that no longer validate as
    /// UTF-8 keeps the load panic-free. Repairing the file re-derives the
    /// canonical text under the canonical content address.
    #[test]
    fn corrupt_file_misses_and_rederives_without_panicking() {
        let mut store = MmapSourceStore::new();
        let healthy_id = content_hash(b"var s = 'e';\n");
        let file = TempFile::write("corrupt", b"var s = 'e';\n");

        // Truncate mid-UTF-8-sequence (the 0xC3 tail of a two-byte
        // character): the mapped validation falls back to the owned lossy
        // form — no mapping aliases the corrupt bytes, and nothing panics.
        fs::write(file.path(), b"var s = '\xC3").expect("corrupt file");
        let corrupt = store.load_file(file.path()).expect("load corrupt");
        assert_eq!(corrupt.as_str(), "var s = '\u{FFFD}");
        assert!(!corrupt.is_mapped(), "invalid UTF-8 takes the owned form");

        // The healthy bytes are gone from disk, so their id is a miss —
        // never a wrong answer served from a stale slot.
        assert_eq!(store.get(healthy_id), None, "corruption is a miss");
        assert_eq!(store.len(), 1);
        assert_eq!(store.len_bytes(), 12);

        // Repairing the file re-derives the canonical text under the
        // canonical address; both spellings keep their own CAS slots.
        fs::write(file.path(), b"var s = 'e';\n").expect("repair file");
        let repaired = store.load_file(file.path()).expect("load repaired");
        assert_eq!(repaired.content_id(), healthy_id);
        assert_eq!(store.get(healthy_id), Some(&b"var s = 'e';\n"[..]));
        assert_eq!(store.len(), 2);
        assert_eq!(store.len_bytes(), 25);
    }
}
