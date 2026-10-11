//! The L3 content-addressed store (PRD §17.2): persistent, cross-session,
//! content-keyed byte storage under the user's cache directory.
//!
//! # Layout
//!
//! Blobs live at `<root>/store/<xx>/<digest>` — `xx` is the digest's first
//! two hex characters (the sccache/cgit shard shape: a fan-out that keeps
//! every directory short no matter how many entries a 10 GiB store
//! amasses). The digest is the house [`ContentId`] (blake3-128, PRD
//! §10.5) of the payload, so source contents keyed here carry the same id
//! the engine's own content store already assigned them. One JSON manifest
//! (`<root>/store/manifest.json`) tracks digest → {size, refcount,
//! last-touch}: **advisory lifecycle input, never correctness** — a blob's
//! identity is its name, so a lost or torn manifest costs bookkeeping,
//! never data.
//!
//! # Bounded lifecycle (PRD §17.2)
//!
//! The store enforces a hard byte cap (`DEFAULT_SIZE_LIMIT`, the
//! `--cache-size-limit` flag value in the CLI; [`parse_size_limit`] is the
//! flag's grammar): on [`ContentStore::open`] and after each write batch,
//! unreferenced entries evict oldest-touch first (digest-ascending
//! tie-break) until the total is at or under the cap. Every I/O failure on
//! the write path degrades at the store boundary — the cache is an
//! accelerator, a failed put or eviction never fails an analysis (the
//! trace-sink posture) — and the observable surface
//! ([`ContentStore::read`], [`ContentStore::total_bytes`]) stays truthful
//! about what actually landed.
//!
//! # Reference counting (the daemon's surface)
//!
//! [`ContentStore::retain`] / [`ContentStore::release`] move an entry's
//! refcount; eviction skips referenced entries (PRD §17.2: content still
//! referenced by any workspace is never evicted). Counts are advisory
//! lifecycle input — nothing correctness-facing reads them — so a crashed
//! holder leaks its pins into the manifest until some later holder
//! releases them.
//!
//! # Concurrency posture
//!
//! No file locking. Two processes sharing a root race on the manifest, and
//! the races degrade exactly like crashes: a lost touch, an entry
//! re-derived by the mtime rebuild, a redundant blob write. The daemon
//! (one writer per machine in the PRD's deployment) is the intended heavy
//! writer; single-shot CLI runs put and flush without counting (PRD
//! §17.2's pure-LRU mode).

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use dartq_source::{ContentId, content_hash};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// The manifest schema version (`manifest.json`). Bump on a layout change;
/// a foreign version rebuilds from disk like a torn file.
const MANIFEST_VERSION: u32 = 1;

/// The default hard cap (PRD §17.2): 10 GiB.
pub const DEFAULT_SIZE_LIMIT: u64 = 10 * 1024 * 1024 * 1024;

/// The size grammar's suffix multipliers, matched case-insensitively.
const SUFFIXES: [(&str, u64); 3] = [
    ("kib", 1024),
    ("mib", 1024 * 1024),
    ("gib", 1024 * 1024 * 1024),
];

/// The `--cache-size-limit` grammar rejection; the message carries the
/// accepted spelling.
#[derive(Debug, Error)]
#[error("invalid size `{text}`: expected NUMBER[KiB|MiB|GiB], e.g. 512MiB, 10GiB, 4096")]
pub struct SizeLimitError {
    text: String,
}

impl SizeLimitError {
    fn new(text: &str) -> Self {
        Self {
            text: text.to_owned(),
        }
    }
}

/// Parses the `--cache-size-limit` grammar: `NUMBER[KiB|MiB|GiB]` — the
/// suffix is case-insensitive and a bare number is bytes (`512MiB`,
/// `10GiB`, `4096`). Fractions and unknown units are rejected so a typo
/// can never silently shrink or blow the cap.
///
/// # Errors
///
/// [`SizeLimitError`] for anything outside the grammar, including a value
/// that overflows once its suffix multiplies in.
pub fn parse_size_limit(text: &str) -> Result<u64, SizeLimitError> {
    let reject = || SizeLimitError::new(text);
    let split = text
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(text.len());
    let value: u64 = text[..split].parse().map_err(|_| reject())?;
    let suffix = &text[split..];
    let multiplier = if suffix.is_empty() {
        1
    } else {
        let Some((_, multiplier)) = SUFFIXES
            .iter()
            .find(|(name, _)| suffix.eq_ignore_ascii_case(name))
        else {
            return Err(reject());
        };
        *multiplier
    };
    value.checked_mul(multiplier).ok_or_else(reject)
}

/// The default cache root: `$XDG_CACHE_HOME/dartq` when that variable is
/// set non-empty, else `~/.cache/dartq` — the PRD §17.2 location.
/// `None` on a platform exposing no home directory; callers stay unarmed.
#[must_use]
pub fn default_root() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("XDG_CACHE_HOME").filter(|dir| !dir.is_empty()) {
        return Some(PathBuf::from(dir).join("dartq"));
    }
    std::env::home_dir().map(|home| home.join(".cache").join("dartq"))
}

/// The open-time knobs: where the store lives and how many bytes it may
/// hold.
#[derive(Debug, Clone)]
pub struct StoreConfig {
    /// The cache root; blobs land under `root/store`. [`default_root`] is
    /// the PRD §17.2 spelling (`~/.cache/dartq`, XDG-respecting).
    pub root: PathBuf,
    /// The hard byte cap — [`DEFAULT_SIZE_LIMIT`] unless the caller says
    /// otherwise (the `--cache-size-limit` flag in the CLI).
    pub size_limit: u64,
}

/// The only way [`ContentStore::open`] fails: the store directory cannot
/// exist (a file squatting the path, denied permissions). Everything after
/// open — torn manifests, failed puts, failed evictions — degrades inside
/// the store.
#[derive(Debug, Error)]
pub enum StoreError {
    #[error("creating content-store directory {0}: {1}")]
    CreateDir(PathBuf, #[source] std::io::Error),
}

/// One manifest row: the lifecycle metadata a booked blob carries.
#[derive(Clone, Copy, Debug)]
struct EntryMeta {
    /// The blob's byte size — the cap's accounting unit.
    size: u64,
    /// Live-holder count (the daemon's pins); zero in the CLI's
    /// single-shot, no-counting mode.
    refcount: u32,
    /// Last-touch unix seconds — the LRU clock. `0` on an unusable clock;
    /// ordering then falls to the digest tie-break.
    last_touch: u64,
}

/// The global L3 store (PRD §17.2). Plain data in the engine's `&mut`
/// world — no interior mutability, [`crate::state::EngineState`]'s
/// posture; the daemon drives the pin surface through
/// [`crate::Engine::content_store_mut`].
pub struct ContentStore {
    /// The blob directory: `<root>/store`, shard subdirectories beneath.
    dir: PathBuf,
    limit: u64,
    /// The books: one row per booked blob, ascending-id order (the save
    /// and the eviction tie-break both rely on it).
    entries: BTreeMap<ContentId, EntryMeta>,
    /// Sum of the booked sizes — maintained alongside `entries`, the
    /// number [`Self::evict`] caps.
    total: u64,
    /// Unsaved book deltas (puts, evictions, pin moves). A successful
    /// manifest save clears it; a failed save keeps it so the next
    /// [`Self::flush`] retries.
    dirty: bool,
}

impl ContentStore {
    /// Opens (creating on first use) the store under `config.root`. Loads
    /// the books — a missing, torn, or foreign-version manifest degrades
    /// to the mtime rebuild (the disk *is* the truth: every well-named
    /// blob re-enters the books refcount-zero, sized from its file, touched
    /// at its mtime) — then enforces the cap and saves the books when they
    /// moved.
    ///
    /// # Errors
    ///
    /// [`StoreError::CreateDir`] when the store directory cannot exist;
    /// callers treat that as "no cache today" (warn and continue).
    pub fn open(config: StoreConfig) -> Result<Self, StoreError> {
        let dir = config.root.join("store");
        fs::create_dir_all(&dir).map_err(|error| StoreError::CreateDir(dir.clone(), error))?;
        let mut store = Self {
            dir,
            limit: config.size_limit,
            entries: BTreeMap::new(),
            total: 0,
            dirty: false,
        };
        store.load_books();
        store.evict();
        store.save_books();
        Ok(store)
    }

    /// Files one payload under its content digest and books it.
    /// Already-stored content writes nothing (global dedup) — it just
    /// re-touches the LRU clock. A failed blob write books nothing and
    /// reports nothing (the accelerator posture); [`Self::contains`] tells
    /// the truth afterwards.
    pub fn put(&mut self, payload: &[u8]) -> ContentId {
        self.put_at(payload, now_secs())
    }

    fn put_at(&mut self, payload: &[u8], now: u64) -> ContentId {
        let id = content_hash(payload);
        if let Some(entry) = self.entries.get_mut(&id) {
            entry.last_touch = now;
            self.dirty = true;
            return id;
        }
        let size = u64::try_from(payload.len()).unwrap_or(u64::MAX);
        if self.write_blob(&id, payload) {
            self.total = self.total.saturating_add(size);
            self.entries.insert(
                id,
                EntryMeta {
                    size,
                    refcount: 0,
                    last_touch: now,
                },
            );
            self.dirty = true;
        }
        id
    }

    /// The crash-safe blob write: `.tmp` then rename, so a killed process
    /// never leaves a half-written blob under a real digest name. The
    /// shard directory is created on demand.
    fn write_blob(&self, id: &ContentId, payload: &[u8]) -> bool {
        let path = self.blob_path(id);
        let tmp = path.with_extension("tmp");
        fs::create_dir_all(path.parent().unwrap_or(&self.dir))
            .and_then(|()| fs::write(&tmp, payload))
            .and_then(|()| fs::rename(&tmp, &path))
            .is_ok()
    }

    /// Persists the books and enforces the cap — the write-batch tail
    /// every caller runs after its puts (the engine flushes once per apply
    /// batch). No-op when nothing moved.
    pub fn flush(&mut self) {
        if !self.dirty {
            return;
        }
        self.evict();
        self.save_books();
    }

    /// Evicts unreferenced entries oldest-touch first (digest tie-break)
    /// until the total is at or under the cap. Best-effort: a failed
    /// `remove_file` still books the entry gone — the orphaned blob is
    /// re-accounted by a future rebuild, so the books stay conservative.
    fn evict(&mut self) {
        if self.total <= self.limit {
            return;
        }
        let mut victims: Vec<(u64, ContentId)> = self
            .entries
            .iter()
            .filter(|(_, entry)| entry.refcount == 0)
            .map(|(id, entry)| (entry.last_touch, *id))
            .collect();
        victims.sort_unstable();
        for (_, id) in victims {
            if self.total <= self.limit {
                break;
            }
            if let Some(entry) = self.entries.remove(&id) {
                let _ = fs::remove_file(self.blob_path(&id));
                self.total = self.total.saturating_sub(entry.size);
                self.dirty = true;
            }
        }
    }

    /// Writes the manifest atomically (`.tmp` then rename). Rows save in
    /// ascending-id order, so the bytes are a pure function of the books.
    fn save_books(&mut self) {
        let file = ManifestFile {
            version: MANIFEST_VERSION,
            entries: self
                .entries
                .iter()
                .map(|(id, entry)| ManifestEntry {
                    id: *id,
                    size: entry.size,
                    refcount: entry.refcount,
                    last_touch: entry.last_touch,
                })
                .collect(),
        };
        let Ok(mut json) = serde_json::to_vec(&file) else {
            return;
        };
        json.push(b'\n');
        let path = self.dir.join("manifest.json");
        let mut tmp = path.clone().into_os_string();
        tmp.push(".tmp");
        if fs::write(&tmp, &json)
            .and_then(|()| fs::rename(&tmp, &path))
            .is_ok()
        {
            self.dirty = false;
        }
    }

    /// Loads the manifest into the books, or rebuilds them from disk when
    /// it is missing, torn, or from another format version.
    fn load_books(&mut self) {
        let Ok(bytes) = fs::read(self.dir.join("manifest.json")) else {
            self.rebuild_books();
            return;
        };
        match serde_json::from_slice::<ManifestFile>(&bytes) {
            Ok(file) if file.version == MANIFEST_VERSION => {
                for entry in file.entries {
                    self.total = self.total.saturating_add(entry.size);
                    self.entries.insert(
                        entry.id,
                        EntryMeta {
                            size: entry.size,
                            refcount: entry.refcount,
                            last_touch: entry.last_touch,
                        },
                    );
                }
            }
            _ => self.rebuild_books(),
        }
    }

    /// The torn-books path: every well-named blob under the shards
    /// re-enters the books refcount-zero, sized from its file, touched at
    /// its mtime — LRU order survives the crash, pins (advisory) do not.
    /// `.tmp` debris fails the name parse and never enters the books.
    fn rebuild_books(&mut self) {
        self.entries.clear();
        self.total = 0;
        self.dirty = true;
        let Ok(shards) = fs::read_dir(&self.dir) else {
            return;
        };
        for shard in shards.flatten() {
            if !shard.file_type().is_ok_and(|kind| kind.is_dir()) {
                continue;
            }
            let Ok(blobs) = fs::read_dir(shard.path()) else {
                continue;
            };
            for blob in blobs.flatten() {
                let Ok(meta) = blob.metadata() else {
                    continue;
                };
                if !meta.is_file() {
                    continue;
                }
                let Some(id) = blob
                    .file_name()
                    .to_str()
                    .and_then(|name| ContentId::parse_hex(name).ok())
                else {
                    continue;
                };
                let size = meta.len();
                let last_touch = meta
                    .modified()
                    .ok()
                    .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                    .map_or(0, |age| age.as_secs());
                self.total = self.total.saturating_add(size);
                self.entries.insert(
                    id,
                    EntryMeta {
                        size,
                        refcount: 0,
                        last_touch,
                    },
                );
            }
        }
    }

    /// Pins `id` against eviction (the daemon's surface, PRD §17.2:
    /// content still referenced by a workspace never evicts). Untracked
    /// ids report `false` — retain the digest [`Self::put`] returned.
    /// Counts saturate; a lost release leaks a pin, never a panic.
    pub fn retain(&mut self, id: &ContentId) -> bool {
        let Some(entry) = self.entries.get_mut(id) else {
            return false;
        };
        entry.refcount = entry.refcount.saturating_add(1);
        entry.last_touch = now_secs();
        self.dirty = true;
        true
    }

    /// Drops one pin. Releasing an unpinned or untracked entry reports
    /// `false`; the count never underflows.
    pub fn release(&mut self, id: &ContentId) -> bool {
        match self.entries.get_mut(id) {
            Some(entry) if entry.refcount > 0 => {
                entry.refcount -= 1;
                self.dirty = true;
                true
            }
            _ => false,
        }
    }

    /// The bytes of `id`. Readers needing certainty re-verify content —
    /// the books are advisory (the L2 facts layer already re-verifies at
    /// ingest).
    ///
    /// # Errors
    ///
    /// The blob read's I/O error, surfaced as-is (an unbooked id reads as
    /// `NotFound`).
    pub fn read(&self, id: &ContentId) -> io::Result<Vec<u8>> {
        fs::read(self.blob_path(id))
    }

    /// Whether `id` is booked. Books and disk disagree only through the
    /// documented failure postures (failed put: booked-no; failed
    /// eviction remove: booked-yes, blob gone).
    #[must_use]
    pub fn contains(&self, id: &ContentId) -> bool {
        self.entries.contains_key(id)
    }

    /// Booked-entry count.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether nothing is booked.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Booked bytes — the total [`Self::evict`] caps.
    #[must_use]
    pub fn total_bytes(&self) -> u64 {
        self.total
    }

    /// The hard byte cap.
    #[must_use]
    pub fn limit(&self) -> u64 {
        self.limit
    }

    /// The blob's path: `dir/<first two hex chars>/<full hex>`.
    fn blob_path(&self, id: &ContentId) -> PathBuf {
        let hex = id.to_string();
        self.dir.join(&hex[..2]).join(hex)
    }

    #[cfg(test)]
    fn refcount_of(&self, id: &ContentId) -> Option<u32> {
        self.entries.get(id).map(|entry| entry.refcount)
    }
}

/// Unix seconds — the LRU clock. `0` pre-epoch (a clock wound backwards);
/// such entries evict first, a benign degradation.
fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |age| age.as_secs())
}

/// The manifest's on-disk shape: a version header plus one row per booked
/// blob.
#[derive(Serialize, Deserialize)]
struct ManifestFile {
    version: u32,
    entries: Vec<ManifestEntry>,
}

#[derive(Serialize, Deserialize)]
struct ManifestEntry {
    #[serde(with = "hex_id")]
    id: ContentId,
    size: u64,
    refcount: u32,
    last_touch: u64,
}

/// The digest's JSON spelling: the lowercase 32-hex-character form
/// [`ContentId`]'s `Display` produces.
mod hex_id {
    use std::fmt;

    use dartq_source::ContentId;
    use serde::{Deserializer, Serializer, de};

    pub(super) fn serialize<S: Serializer>(
        id: &ContentId,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&id.to_string())
    }

    pub(super) fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<ContentId, D::Error> {
        struct IdVisitor;

        impl de::Visitor<'_> for IdVisitor {
            type Value = ContentId;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a 32-character hex content digest")
            }

            fn visit_str<E: de::Error>(self, text: &str) -> Result<ContentId, E> {
                ContentId::parse_hex(text).map_err(de::Error::custom)
            }
        }

        deserializer.deserialize_str(IdVisitor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// A throwaway cache root, self-cleaning on drop (the persist tests'
    /// fixture shape, plus drop hygiene).
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(tag: &str) -> Self {
            static COUNTER: AtomicU32 = AtomicU32::new(0);
            let root = std::env::temp_dir().join(format!(
                "dartq-engine-store-{tag}-{}-{}",
                std::process::id(),
                COUNTER.fetch_add(1, Ordering::Relaxed)
            ));
            let _ = fs::remove_dir_all(&root);
            Self(root)
        }

        fn config(&self, limit: u64) -> StoreConfig {
            StoreConfig {
                root: self.0.clone(),
                size_limit: limit,
            }
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn bytes(payload: &[u8]) -> u64 {
        u64::try_from(payload.len()).expect("test payload fits u64")
    }

    /// The digest is the house `ContentId` — blake3-128 of the payload —
    /// pinned to the published BLAKE3 empty-input vector, truncated. The
    /// cross-session key must never drift, or every stored blob orphans
    /// at once. The shard is the digest's first two hex characters: this
    /// payload's blob lands under `store/af/`.
    #[test]
    fn digest_is_the_pinned_house_hash() {
        let id = content_hash(b"");
        assert_eq!(id.to_string(), "af1349b9f5f9a1a6a0404dea36dcc949");

        let scratch = Scratch::new("digest");
        let mut store = ContentStore::open(scratch.config(u64::MAX)).expect("open");
        store.put(b"");
        let hex = id.to_string();
        assert!(
            scratch.0.join("store").join(&hex[..2]).join(&hex).is_file(),
            "the blob lives at the two-hex-char shard path"
        );
    }

    /// The `--cache-size-limit` grammar: suffixed and bare numbers parse,
    /// suffixes are case-insensitive, and typos (unknown units, fractions,
    /// signs, overflow at any stage) never silently shrink or blow the cap.
    #[test]
    fn size_limit_grammar() {
        assert_eq!(
            parse_size_limit("10GiB").ok(),
            Some(10 * 1024 * 1024 * 1024)
        );
        assert_eq!(parse_size_limit("512MiB").ok(), Some(512 * 1024 * 1024));
        assert_eq!(
            parse_size_limit("1kib").ok(),
            Some(1024),
            "the suffix is case-insensitive"
        );
        assert_eq!(
            parse_size_limit("4096").ok(),
            Some(4096),
            "a bare number is bytes"
        );
        assert_eq!(parse_size_limit("0").ok(), Some(0));
        for text in [
            "",
            "MiB",
            "10 TiB",
            "-1",
            "1.5GiB",
            "10x",
            "18446744073709551616",
        ] {
            assert!(parse_size_limit(text).is_err(), "`{text}` must not parse");
        }
        assert!(
            parse_size_limit("18446744073709551615GiB").is_err(),
            "the suffix multiply overflow rejects too"
        );
    }

    /// The cross-session pin: open → put → flush → drop → open sees the
    /// blob, and the manifest round trip is lossless for every booked
    /// column — sizes, the total, and the daemon's pins included.
    #[test]
    fn write_batch_survives_reopen() {
        let scratch = Scratch::new("roundtrip");
        let alpha = b"alpha payload";
        let beta = b"beta";
        let mut store = ContentStore::open(scratch.config(u64::MAX)).expect("open");
        let alpha_id = store.put(alpha);
        let beta_id = store.put(beta);
        assert!(store.retain(&beta_id), "the daemon pin lands");
        store.flush();
        drop(store);

        let reopened = ContentStore::open(scratch.config(u64::MAX)).expect("reopen");
        assert_eq!(
            reopened.read(&alpha_id).expect("blob"),
            alpha,
            "the cross-session read serves the written bytes"
        );
        assert!(reopened.contains(&beta_id));
        assert_eq!(reopened.total_bytes(), bytes(alpha) + bytes(beta));
        assert_eq!(reopened.len(), 2);
        assert_eq!(
            reopened.refcount_of(&beta_id),
            Some(1),
            "the pin survives the manifest round trip"
        );
    }

    /// The cap evicts oldest-touch first, digest-ascending on ties, and a
    /// pinned entry survives regardless of age — after a write batch and
    /// again at open (the manifest-carried pin still gates).
    #[test]
    fn eviction_is_lru_and_respects_pins() {
        let scratch = Scratch::new("evict");
        let mut store = ContentStore::open(scratch.config(10)).expect("open");
        let a = store.put_at(b"aaaaa", 100);
        let b = store.put_at(b"bbbbb", 200);
        store.flush();
        assert_eq!(store.total_bytes(), 10, "at cap: nothing evicts");

        let c = store.put_at(b"ccccc", 300);
        store.flush();
        assert!(
            !store.contains(&a) && store.contains(&b) && store.contains(&c),
            "the oldest touch evicts first"
        );

        assert!(store.retain(&b), "pin the middle entry");
        let d = store.put_at(b"ddddd", 400);
        store.flush();
        assert!(
            !store.contains(&c) && store.contains(&b) && store.contains(&d),
            "the oldest unpinned evicts; the pinned entry survives"
        );
        assert_eq!(store.total_bytes(), 10, "the cap holds after eviction");
        drop(store);

        let reopened = ContentStore::open(scratch.config(5)).expect("reopen");
        assert!(
            reopened.contains(&b) && !reopened.contains(&d),
            "open-time eviction enforces the cap, manifest-carried pin intact"
        );
    }

    /// The pin surface: unknown ids refuse, counts move one at a time, and
    /// release never underflows.
    #[test]
    fn refcounts_move_one_at_a_time() {
        let scratch = Scratch::new("refcount");
        let mut store = ContentStore::open(scratch.config(u64::MAX)).expect("open");
        let ghost = content_hash(b"never stored");
        assert!(!store.retain(&ghost) && !store.release(&ghost));

        let id = store.put(b"payload");
        assert_eq!(store.refcount_of(&id), Some(0));
        assert!(store.retain(&id));
        assert_eq!(store.refcount_of(&id), Some(1));
        assert!(store.release(&id));
        assert_eq!(store.refcount_of(&id), Some(0));
        assert!(!store.release(&id), "the count never underflows");
    }

    /// A torn (or foreign-version, or missing) manifest degrades to the
    /// mtime rebuild — the books re-derive from the blobs themselves,
    /// `.tmp` debris never enters them, and the rebuilt manifest saves
    /// back so the next open takes the healthy path.
    #[test]
    fn torn_manifest_rebuilds_from_disk() {
        let scratch = Scratch::new("torn");
        let payload = b"survives the crash";
        let mut store = ContentStore::open(scratch.config(u64::MAX)).expect("open");
        let id = store.put(payload);
        store.flush();
        let store_dir = scratch.0.join("store");
        let hex = id.to_string();

        fs::write(store_dir.join("manifest.json"), b"{torn").expect("tear the manifest");
        fs::write(
            store_dir.join(&hex[..2]).join(format!("{hex}.tmp")),
            b"half-written",
        )
        .expect("leave blob debris");

        let rebuilt = ContentStore::open(scratch.config(u64::MAX)).expect("reopen");
        assert!(
            rebuilt.contains(&id),
            "the well-named blob re-enters the books"
        );
        assert_eq!(rebuilt.read(&id).expect("blob"), payload);
        assert_eq!(rebuilt.total_bytes(), bytes(payload));
        assert_eq!(
            rebuilt.refcount_of(&id),
            Some(0),
            "pins do not survive the rebuild (advisory input)"
        );
        drop(rebuilt);

        // A manifest from a future format version rebuilds the same way.
        fs::write(
            store_dir.join("manifest.json"),
            br#"{"version":9999,"entries":[]}"#,
        )
        .expect("write a foreign manifest");
        let rebuilt = ContentStore::open(scratch.config(u64::MAX)).expect("reopen");
        assert!(
            rebuilt.contains(&id),
            "a foreign version is torn, not fatal"
        );
    }

    /// The manifest bytes are a pure function of the books (ascending-id
    /// rows): a clean flush writes nothing, and load+save round-trips
    /// byte-stably, so an unchanged library re-saves identically.
    #[test]
    fn manifest_bytes_are_deterministic() {
        let scratch = Scratch::new("stable");
        let mut store = ContentStore::open(scratch.config(u64::MAX)).expect("open");
        store.put(b"one");
        store.put(b"two");
        store.flush();
        let manifest =
            || fs::read(scratch.0.join("store").join("manifest.json")).expect("manifest");
        let first = manifest();
        store.flush();
        assert_eq!(
            manifest(),
            first,
            "a clean flush rewrites nothing (bytes identical)"
        );
        drop(store);

        let mut store = ContentStore::open(scratch.config(u64::MAX)).expect("reopen");
        store.flush();
        assert_eq!(
            manifest(),
            first,
            "load+save round trips the books byte-stably"
        );
    }
}
