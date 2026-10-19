//! The L3 content-addressed store: persistent, cross-session,
//! content-keyed byte storage under the user's cache directory.
//!
//! # Layout
//!
//! Blobs live at `<root>/store/<xx>/<digest>` — `xx` is the digest's first
//! two hex characters (the sccache/cgit shard shape: a fan-out that keeps
//! every directory short no matter how many entries a 10 GiB store
//! amasses). The digest is the house [`ContentId`] (blake3-128) of the
//! payload, so source contents keyed here carry the same id
//! the engine's own content store already assigned them. One JSON manifest
//! (`<root>/store/manifest.json`) tracks digest → {size, refcount,
//! last-touch}: **advisory lifecycle input, never correctness** — a blob's
//! identity is its name, so a lost or torn manifest costs bookkeeping,
//! never data.
//!
//! # Delta journal (#1385)
//!
//! The manifest is the *base*: the books as of the last compaction.
//! Between compactions a JSONL journal (`<root>/store/manifest.jsonl`)
//! carries the moves — one absolute-value row per changed id per dirty
//! flush (`set` with the full row, or `del`), appended O(changed) where
//! the pre-journal flush rewrote O(residency) manifest bytes per flush.
//! Every [`FLUSHES_PER_COMPACTION`] dirty flushes (and unconditionally at
//! open, which has always paid one full manifest write) the books compact:
//! the manifest rewrites from the in-memory truth and the journal is
//! removed — manifest rename *first*, so any crash point recovers
//! (replaying a stale journal's absolute rows over a fresher base is a
//! no-op; the reverse order could lose deltas). Recovery = base + journal
//! replay in order, stopping at the first unparsable line: a torn tail
//! degrades to the last complete entry, a foreign line the same way, and
//! rows are idempotent (absolute values, later row wins). A missing,
//! torn, or foreign-version *base* skips the journal entirely and
//! rebuilds from the blobs — the disk is the truth, and a journal is only
//! meaningful over its base.
//!
//! # Bounded lifecycle
//!
//! The store enforces a hard byte cap (`DEFAULT_SIZE_LIMIT`, the
//! `--cache-size-limit` flag value in the CLI; [`parse_size_limit`] is the
//! flag's grammar) and a books entry cap ([`MAX_BOOK_ENTRIES`], the RAM
//! bound the byte cap cannot provide): on [`ContentStore::open`] and after
//! each write batch, unreferenced entries evict oldest-touch first
//! (digest-ascending tie-break) until the total is at or under the byte cap
//! and the books are at or under the entry cap. Every I/O failure on
//! the write path degrades at the store boundary — the cache is an
//! accelerator, a failed put or eviction never fails an analysis (the
//! trace-sink posture) — and the observable surface
//! ([`ContentStore::read`], [`ContentStore::total_bytes`]) stays truthful
//! about what actually landed.
//!
//! # Reference counting (the daemon's surface)
//!
//! [`ContentStore::retain`] / [`ContentStore::release`] move an entry's
//! refcount; eviction skips referenced entries (: content still
//! referenced by any workspace is never evicted). Counts are advisory
//! lifecycle input — nothing correctness-facing reads them — so a crashed
//! holder leaks its pins into the manifest until some later holder
//! releases them.
//!
//! # Concurrency posture
//!
//! The books (base manifest + journal) sit behind an exclusive advisory
//! lock on a sidecar file (`<root>/store/manifest.lock`) held across
//! exactly the read→decide→write sequences that touch them: `open`'s
//! load-evict-compact and `flush`'s evict-append-compact. The lock is an
//! optimization, not a correctness gate: acquisition tries once, then
//! waits a bounded ~200 ms, and a contender that outlasts the wait (or
//! any lock I/O failure) proceeds unlocked, degrading exactly like
//! crashes did before the lock — a lost touch, an entry re-derived by the
//! mtime rebuild, a redundant blob write, last-write-wins books. Blobs
//! are never locked: content addressing makes them immutable, so only the
//! books can race. The sidecar is never unlinked (an unlinked lock file
//! is the classic lost-lock race) and its presence is never correctness.
//! The daemon is the intended heavy
//! writer; single-shot CLI runs put and flush without counting (the
//! pure-LRU mode).

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{self, Write};
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use dartq_source::{ContentId, content_hash};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// The manifest schema version (`manifest.json`). Bump on a layout change;
/// a foreign version rebuilds from disk like a torn file.
const MANIFEST_VERSION: u32 = 1;

/// The default hard cap: 10 GiB.
pub const DEFAULT_SIZE_LIMIT: u64 = 10 * 1024 * 1024 * 1024;

/// The books' entry-count cap — the RAM bound the byte cap cannot provide.
/// The books are advisory lifecycle rows whose memory grows with the *disk*
/// store population, decoupled from any workspace: ~72 B/entry (40 B of
/// `ContentId` + `EntryMeta` row plus BTree node fan-out), so a 10 GiB
/// store of ~1 KB blobs would amass ~10⁷ rows ≈ 0.5–0.8 GB of RAM. 2²⁰
/// rows hold that face to ≈ 72 MB — ten times the ~10⁵ entries a 10⁵-file
/// workspace books — while the byte cap stays the primary bound for
/// realistic blob sizes (a 2²⁰-row store of ≥1 KB blobs is ≤ 1 GiB). Past
/// the cap the same LRU as the byte cap evicts unpinned oldest-touch
/// entries, blob and row together, so a books miss stays a true miss; a
/// fully pinned store keeps its pins (bounded by live workspaces' content
/// sets, never by disk).
const MAX_BOOK_ENTRIES: usize = 1 << 20;

/// Dirty flushes between journal compactions (#1385). The per-flush IO is
/// the O(changed) journal append plus an amortized 1/64 of a full manifest
/// rewrite; the open-time replay is bounded by the deltas of at most 64
/// dirty flushes (at the daemon's 500 ms debounce, ~32 s of active
/// editing — tens of KiB of journal). 64 matches the throttle-interval
/// order of the snapshot store's `ENFORCE_EVERY_SAVES` (32) and keeps the
/// amortized compaction term at ~1.6% of the wholesale rewrite it
/// replaces.
const FLUSHES_PER_COMPACTION: u32 = 64;

/// The delta journal file next to the base manifest (#1385): JSONL, one
/// [`JournalRow`] per line.
const JOURNAL_NAME: &str = "manifest.jsonl";

/// The books-lock sidecar, next to the base manifest and journal
/// (`<root>/store/manifest.lock`).
const LOCK_NAME: &str = "manifest.lock";

/// How long a contended books write waits for the lock before proceeding
/// unlocked: long enough to absorb two daemons opening or flushing
/// against one root, short enough that a stuck holder costs milliseconds.
/// The wait, not the lock, is the bound — past it the documented unlocked
/// degradation applies (the module's concurrency posture).
const LOCK_WAIT: Duration = Duration::from_millis(200);

/// The bounded wait's poll tick.
const LOCK_TICK: Duration = Duration::from_millis(5);

/// One journal line: the absolute book state of one moved id (`set`
/// carries the full row, `del` retires it). Absolute values make rows
/// idempotent under replay — a stale journal over a fresher base is a
/// no-op, a duplicated row (an append retried after a partial write)
/// converges to the same state — and later rows win, so a re-put after an
/// in-flush eviction collapses to the surviving op.
#[derive(Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "lowercase")]
enum JournalRow {
    Set {
        #[serde(with = "hex_id")]
        id: ContentId,
        size: u64,
        refcount: u32,
        last_touch: u64,
    },
    Del {
        #[serde(with = "hex_id")]
        id: ContentId,
    },
}

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
/// set non-empty, else `~/.cache/dartq` — the location.
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
    /// the spelling (`~/.cache/dartq`, XDG-respecting).
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

/// The global L3 store. Plain data in the engine's `&mut`
/// world — no interior mutability, `crate::state::EngineState`'s
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
    /// The ids whose booked row moved since the last landed journal append
    /// or compaction (new puts, evictions, pin moves). A pure LRU re-touch
    /// is not a move: it updates the in-memory clock only, and the row's
    /// stale `last_touch` lands when that id itself next moves (the books
    /// are advisory — a restart evicting by a stale touch is the same
    /// documented degradation as the multi-process lost touch). An empty
    /// set is a clean store: [`Self::flush`] writes nothing. The set
    /// clears when its rows land in the journal; a failed append keeps it
    /// so the next [`Self::flush`] retries (re-appended rows are
    /// idempotent under replay).
    delta: BTreeSet<ContentId>,
    /// Dirty flushes since the last compaction; at
    /// [`FLUSHES_PER_COMPACTION`] the next flush compacts instead of only
    /// appending. Probed via [`Self::set_flushes_per_compaction`].
    appends: u32,
    /// The compaction cadence — [`FLUSHES_PER_COMPACTION`] unless the
    /// probe axis overrides it (`1` reproduces the pre-journal wholesale
    /// rewrite per flush).
    flushes_per_compaction: u32,
    /// The books' entry cap — [`MAX_BOOK_ENTRIES`] unless the probe axis
    /// overrides it ([`Self::set_max_book_entries`]).
    max_book_entries: usize,
}

impl ContentStore {
    /// Opens (creating on first use) the store under `config.root`. Loads
    /// the books — a healthy base manifest plus its journal replay, or the
    /// mtime rebuild when the base is missing, torn, or foreign-versioned
    /// (the disk *is* the truth: every well-named blob re-enters the books
    /// refcount-zero, sized from its file, touched at its mtime) — then
    /// enforces the cap and compacts: the one full manifest write open has
    /// always paid, which doubles as the journal-absorbing normalization.
    ///
    /// # Errors
    ///
    /// [`StoreError::CreateDir`] when the store directory cannot exist;
    /// callers treat that as "no cache today" (warn and continue).
    pub fn open(config: &StoreConfig) -> Result<Self, StoreError> {
        let dir = config.root.join("store");
        fs::create_dir_all(&dir).map_err(|error| StoreError::CreateDir(dir.clone(), error))?;
        let mut store = Self {
            dir,
            limit: config.size_limit,
            entries: BTreeMap::new(),
            total: 0,
            delta: BTreeSet::new(),
            appends: 0,
            flushes_per_compaction: FLUSHES_PER_COMPACTION,
            max_book_entries: MAX_BOOK_ENTRIES,
        };
        // The books read→decide→write under the manifest lock (the
        // module's concurrency posture): a contender past the bounded
        // wait leaves this open unlocked, today's degradation.
        let _books = store.lock_books();
        store.load_books();
        store.evict();
        // Unconditional compaction at open (the full manifest write open
        // has always paid): it normalizes rebuilt books into a fresh base
        // and absorbs any journal the previous session left.
        store.compact();
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

    /// [`Self::put`] for a payload whose content id the caller already
    /// holds (the residency's `facts.content` is the same blake3 face):
    /// skips the re-hash.
    pub(crate) fn put_known(&mut self, id: ContentId, payload: &[u8]) -> ContentId {
        self.put_known_at(id, payload, now_secs())
    }

    fn put_at(&mut self, payload: &[u8], now: u64) -> ContentId {
        self.put_known_at(content_hash(payload), payload, now)
    }

    fn put_known_at(&mut self, id: ContentId, payload: &[u8], now: u64) -> ContentId {
        if let Some(entry) = self.entries.get_mut(&id) {
            // The dedup hit re-touches the in-memory LRU clock only — not
            // a book delta, so no manifest save rides on it (a keystroke
            // batch of re-hits flushes zero bytes regardless of store
            // size; the touched clock persists with the next real delta).
            entry.last_touch = now;
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
            self.delta.insert(id);
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

    /// Persists the book deltas and enforces the cap — the write-batch tail
    /// every caller runs after its puts (the engine flushes once per apply
    /// batch). No-op when nothing moved. The move set lands as journal
    /// rows — O(changed) bytes where the pre-journal flush rewrote the
    /// whole manifest — and every [`FLUSHES_PER_COMPACTION`] dirty flushes
    /// the journal compacts back into a full manifest rewrite.
    pub fn flush(&mut self) {
        if self.delta.is_empty() {
            return;
        }
        // The books write under the manifest lock (the module's
        // concurrency posture); a contender past the bounded wait appends
        // unlocked, today's degradation.
        let _books = self.lock_books();
        self.evict();
        self.append_delta();
        if self.appends >= self.flushes_per_compaction {
            self.compact();
        }
    }

    /// Evicts unreferenced entries oldest-touch first (digest tie-break)
    /// until the total is at or under the byte cap and the books are at or
    /// under the entry cap ([`MAX_BOOK_ENTRIES`]). Both caps evict the same
    /// LRU, blob and row together, so a books miss stays a true miss and
    /// the victims' working set — the eviction transient — is bounded by
    /// the entry cap too. Best-effort: a failed `remove_file` still books
    /// the entry gone — the orphaned blob is re-accounted by a future
    /// rebuild, so the books stay conservative.
    fn evict(&mut self) {
        if self.total <= self.limit && self.entries.len() <= self.max_book_entries {
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
            if self.total <= self.limit && self.entries.len() <= self.max_book_entries {
                break;
            }
            if let Some(entry) = self.entries.remove(&id) {
                let _ = fs::remove_file(self.blob_path(&id));
                self.total = self.total.saturating_sub(entry.size);
                self.delta.insert(id);
            }
        }
    }

    /// Appends one journal row per moved id (ascending id — the `BTreeSet`
    /// order, so the journal bytes are a pure function of the move set and
    /// the books). Rows carry the id's *current* full state, so a
    /// within-flush put→evict or evict→re-put collapses to the surviving
    /// op. The append is a single opened-in-append-mode `write_all`: a
    /// partial write leaves a torn tail that replay drops to the last
    /// complete line, and a failed append keeps the move set so the next
    /// flush re-appends (idempotent rows). Durability (fsync) is
    /// deliberately omitted — a torn tail is a recoverable miss, the same
    /// posture as the atomic manifest rename.
    fn append_delta(&mut self) {
        let mut rows = Vec::new();
        for id in &self.delta {
            let row = match self.entries.get(id) {
                Some(entry) => JournalRow::Set {
                    id: *id,
                    size: entry.size,
                    refcount: entry.refcount,
                    last_touch: entry.last_touch,
                },
                None => JournalRow::Del { id: *id },
            };
            if serde_json::to_writer(&mut rows, &row).is_err() {
                return;
            }
            rows.push(b'\n');
        }
        let Ok(mut journal) = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.dir.join(JOURNAL_NAME))
        else {
            return;
        };
        if journal.write_all(&rows).is_ok() {
            self.delta.clear();
            self.appends += 1;
        }
    }

    /// Rewrites the base manifest from the books (atomically, rows in
    /// ascending-id order — the bytes are a pure function of the books),
    /// then removes the journal. The manifest rename happens *first*: a
    /// crash before the journal removal leaves the fresh base facing stale
    /// rows that replay as no-ops, while the reverse order could lose the
    /// deltas the journal held. On success the move set clears (the base
    /// now holds every id's state) and the append counter resets; a failed
    /// write keeps both so the next compaction retries. A failed journal
    /// removal is harmless — the stale rows ahead of later appends replay
    /// as no-ops over the fresh base, and the next compaction removes the
    /// file.
    fn compact(&mut self) {
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
            let _ = fs::remove_file(self.dir.join(JOURNAL_NAME));
            self.delta.clear();
            self.appends = 0;
        }
    }

    /// Loads the manifest into the books and replays the journal over it,
    /// or rebuilds them from disk when the base is missing, torn, or from
    /// another format version — the journal is only meaningful over its
    /// base, so a rebuilt base ignores it (the blobs are the truth; the
    /// rebuild's documented losses — pins, fresh touches — already cover
    /// what the rows would have restored).
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
                self.replay_journal();
            }
            _ => self.rebuild_books(),
        }
    }

    /// Applies the journal's complete lines in order, stopping at the
    /// first unparsable one — a torn tail (a partial final line from a
    /// killed append) recovers to the last complete entry, and the empty
    /// final chunk a trailing newline leaves is just the stopping point
    /// after every real row.
    fn replay_journal(&mut self) {
        let Ok(bytes) = fs::read(self.dir.join(JOURNAL_NAME)) else {
            return;
        };
        for line in bytes.split(|byte| *byte == b'\n') {
            let Ok(row) = serde_json::from_slice::<JournalRow>(line) else {
                break;
            };
            match row {
                JournalRow::Set {
                    id,
                    size,
                    refcount,
                    last_touch,
                } => {
                    if let Some(old) = self.entries.insert(
                        id,
                        EntryMeta {
                            size,
                            refcount,
                            last_touch,
                        },
                    ) {
                        self.total = self.total.saturating_sub(old.size);
                    }
                    self.total = self.total.saturating_add(size);
                }
                JournalRow::Del { id } => {
                    if let Some(old) = self.entries.remove(&id) {
                        self.total = self.total.saturating_sub(old.size);
                    }
                }
            }
        }
    }

    /// The torn-books path: every well-named blob under the shards
    /// re-enters the books refcount-zero, sized from its file, touched at
    /// its mtime — LRU order survives the crash, pins (advisory) do not.
    /// `.tmp` debris fails the name parse and never enters the books.
    fn rebuild_books(&mut self) {
        self.entries.clear();
        self.total = 0;
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

    /// Pins `id` against eviction (the daemon's surface:
    /// content still referenced by a workspace never evicts). Untracked
    /// ids report `false` — retain the digest [`Self::put`] returned.
    /// Counts saturate; a lost release leaks a pin, never a panic.
    pub fn retain(&mut self, id: &ContentId) -> bool {
        let Some(entry) = self.entries.get_mut(id) else {
            return false;
        };
        entry.refcount = entry.refcount.saturating_add(1);
        entry.last_touch = now_secs();
        self.delta.insert(*id);
        true
    }

    /// Drops one pin. Releasing an unpinned or untracked entry reports
    /// `false`; the count never underflows.
    pub fn release(&mut self, id: &ContentId) -> bool {
        match self.entries.get_mut(id) {
            Some(entry) if entry.refcount > 0 => {
                entry.refcount -= 1;
                self.delta.insert(*id);
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

    /// Booked bytes — the total `Self::evict` caps.
    #[must_use]
    pub fn total_bytes(&self) -> u64 {
        self.total
    }

    /// The hard byte cap.
    #[must_use]
    pub fn limit(&self) -> u64 {
        self.limit
    }

    /// Overrides the compaction cadence — the probe axis's knob
    /// (`DARTQ_BENCH_L3`'s full-rewrite series rides `1` to reproduce the
    /// pre-journal wholesale manifest rewrite per flush, and the
    /// byte-identity pins run the journal cadence against a full-rewrite
    /// cadence over identical op sequences). `0` compacts on every dirty
    /// flush.
    #[doc(hidden)]
    pub fn set_flushes_per_compaction(&mut self, every: u32) {
        self.flushes_per_compaction = every;
    }

    /// Overrides the books' entry cap — the probe axis's knob (the
    /// entry-cap test rides a tiny cap; production runs
    /// [`MAX_BOOK_ENTRIES`]). The reset-to-constant points are `open` and a
    /// fresh `StoreConfig`, same posture as [`Self::set_flushes_per_compaction`].
    #[doc(hidden)]
    pub fn set_max_book_entries(&mut self, max: usize) {
        self.max_book_entries = max;
    }

    /// Takes the exclusive books lock ([`LOCK_NAME`] beside the base
    /// manifest), or `None` when the lock is not held on return:
    /// contention outlasting [`LOCK_WAIT`], or a lock-file I/O failure
    /// (read-only cache dir, a filesystem without advisory locks). `None`
    /// is the documented degradation, never an error — the caller proceeds
    /// unlocked and the races degrade exactly like crashes (the module's
    /// concurrency posture). The lock releases when the returned file
    /// drops; the sidecar itself is never unlinked, since an unlinked
    /// lock file is the classic lost-lock race.
    fn lock_books(&self) -> Option<fs::File> {
        let file = fs::File::create(self.dir.join(LOCK_NAME)).ok()?;
        let deadline = Instant::now() + LOCK_WAIT;
        loop {
            match file.try_lock() {
                Ok(()) => return Some(file),
                Err(fs::TryLockError::WouldBlock) if Instant::now() < deadline => {
                    std::thread::sleep(LOCK_TICK);
                }
                // Contention past the wait, or the lock primitive itself
                // failed — both leave the books unlocked, not failed.
                Err(fs::TryLockError::WouldBlock) | Err(fs::TryLockError::Error(_)) => {
                    return None;
                }
            }
        }
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
        let mut store = ContentStore::open(&scratch.config(u64::MAX)).expect("open");
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
        let mut store = ContentStore::open(&scratch.config(u64::MAX)).expect("open");
        let alpha_id = store.put(alpha);
        let beta_id = store.put(beta);
        assert!(store.retain(&beta_id), "the daemon pin lands");
        store.flush();
        drop(store);

        let reopened = ContentStore::open(&scratch.config(u64::MAX)).expect("reopen");
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
        let mut store = ContentStore::open(&scratch.config(10)).expect("open");
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

        let reopened = ContentStore::open(&scratch.config(5)).expect("reopen");
        assert!(
            reopened.contains(&b) && !reopened.contains(&d),
            "open-time eviction enforces the cap, manifest-carried pin intact"
        );
    }

    /// The books' entry cap ([`MAX_BOOK_ENTRIES`] via the probe knob)
    /// evicts the same LRU as the byte cap — unpinned, oldest-touch first —
    /// blob and row together, so an evicted id's read misses on disk, and a
    /// pinned entry survives regardless of count.
    #[test]
    fn entry_cap_evicts_oldest_unpinned_first() {
        let scratch = Scratch::new("entry-cap");
        let mut store = ContentStore::open(&scratch.config(u64::MAX)).expect("open");
        store.set_max_book_entries(2);
        let a = store.put_at(b"alpha", 100);
        let b = store.put_at(b"beta", 200);
        let c = store.put_at(b"gamma", 300);
        store.flush();
        assert!(
            !store.contains(&a) && store.contains(&b) && store.contains(&c),
            "the oldest unpinned row evicts to the cap"
        );
        assert_eq!(store.len(), 2);
        assert!(store.read(&a).is_err(), "the blob went with its row");

        assert!(store.retain(&b), "pin the middle entry");
        let d = store.put_at(b"delta", 400);
        store.flush();
        assert!(
            !store.contains(&c) && store.contains(&b) && store.contains(&d),
            "the pin survives the entry-cap eviction"
        );
        assert!(store.len() <= 2, "the cap holds after eviction");
    }

    /// The pin surface: unknown ids refuse, counts move one at a time, and
    /// release never underflows.
    #[test]
    fn refcounts_move_one_at_a_time() {
        let scratch = Scratch::new("refcount");
        let mut store = ContentStore::open(&scratch.config(u64::MAX)).expect("open");
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
        let mut store = ContentStore::open(&scratch.config(u64::MAX)).expect("open");
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

        let rebuilt = ContentStore::open(&scratch.config(u64::MAX)).expect("reopen");
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
        let rebuilt = ContentStore::open(&scratch.config(u64::MAX)).expect("reopen");
        assert!(
            rebuilt.contains(&id),
            "a foreign version is torn, not fatal"
        );
    }

    /// The manifest bytes are a pure function of the books (ascending-id
    /// rows): a clean flush writes nothing, and load+save round-trips
    /// byte-stably, so an unchanged library re-saves identically. The
    /// books are driven through a compaction first (#1385) so `first`
    /// captures the full-books manifest the journal cadence converges to.
    #[test]
    fn manifest_bytes_are_deterministic() {
        let scratch = Scratch::new("stable");
        let mut store = ContentStore::open(&scratch.config(u64::MAX)).expect("open");
        store.set_flushes_per_compaction(1);
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

        let mut store = ContentStore::open(&scratch.config(u64::MAX)).expect("reopen");
        store.flush();
        assert_eq!(
            manifest(),
            first,
            "load+save round trips the books byte-stably"
        );
    }

    /// Dirty-on-delta: a pure LRU re-touch (a dedup hit) writes nothing —
    /// the manifest and journal bytes both stand — while a real content
    /// delta appends its journal rows (O(changed) bytes; the base manifest
    /// stands until compaction). Each entry's row lands at its own delta
    /// carrying the then-current clock; a re-hit's refreshed clock waits
    /// for that entry's next move (the books are advisory). #1385 migrated
    /// the delta face from a manifest rewrite to the journal append; the
    /// touch-only half is the original pin, unchanged.
    #[test]
    fn touch_only_update_rewrites_nothing() {
        let scratch = Scratch::new("touch");
        let mut store = ContentStore::open(&scratch.config(u64::MAX)).expect("open");
        let alpha = store.put(b"alpha");
        store.flush();

        let store_dir = scratch.0.join("store");
        let manifest_path = store_dir.join("manifest.json");
        let journal_path = store_dir.join("manifest.jsonl");
        let manifest_bytes = fs::read(&manifest_path).expect("manifest");
        let journal_bytes = fs::read(&journal_path).expect("journal");

        store.put(b"alpha");
        store.flush();
        assert_eq!(
            fs::read(&manifest_path).expect("manifest"),
            manifest_bytes,
            "a touch-only flush rewrites no manifest bytes"
        );
        assert_eq!(
            fs::read(&journal_path).expect("journal"),
            journal_bytes,
            "a touch-only flush appends no journal rows (a byte-equal append is \
             impossible: any append grows the file)"
        );

        let beta = store.put(b"beta");
        store.flush();
        assert_ne!(
            fs::read(&journal_path).expect("journal"),
            journal_bytes,
            "a content-delta flush appends its journal rows"
        );
        assert_eq!(
            fs::read(&manifest_path).expect("manifest"),
            manifest_bytes,
            "the base manifest stands until compaction"
        );
        drop(store);

        let reopened = ContentStore::open(&scratch.config(u64::MAX)).expect("reopen");
        assert!(
            reopened.contains(&alpha) && reopened.contains(&beta),
            "journal replay persisted both entries, each with its own delta's clock"
        );
    }

    /// The byte-identity pin (#1385): an op sequence covering puts, a
    /// cap-forced eviction, a dedup re-hit that moves an entry out of
    /// eviction order, an evicted id rejoining, and a compaction boundary
    /// mid-sequence lands the same books under the journal cadence
    /// (compacts every 2nd flush) as under the wholesale full-rewrite
    /// cadence (compacts every flush — the pre-journal world). Reopen
    /// recovers base+journal and compacts, so the post-reopen manifests
    /// converging byte for byte pins the replay: it produced exactly the
    /// books the wholesale rewrite persisted. Fixed `put_at` stamps keep
    /// every row's `last_touch` deterministic across the paired roots.
    #[test]
    fn journal_replay_matches_full_rewrite_across_compaction_boundary() {
        let journal_root = Scratch::new("journal-pin");
        let rewrite_root = Scratch::new("rewrite-pin");
        run_pinned_sequence(&journal_root, 2);
        run_pinned_sequence(&rewrite_root, 1);

        let store_dir = journal_root.0.join("store");
        let journal_manifest = store_dir.join("manifest.json");
        let rewrite_manifest = rewrite_root.0.join("store").join("manifest.json");
        // Pre-reopen the postures differ exactly where the journal is
        // still carrying un-compacted deltas — the difference the replay
        // must restore.
        assert!(
            store_dir.join("manifest.jsonl").is_file(),
            "the journal root ends mid-cadence with rows un-compacted"
        );
        assert!(
            !rewrite_root.0.join("store").join("manifest.jsonl").exists(),
            "the full-rewrite cadence never leaves a journal"
        );
        assert_ne!(
            fs::read(&journal_manifest).expect("manifest"),
            fs::read(&rewrite_manifest).expect("manifest"),
            "pre-reopen the journal root's base is one cadence stale"
        );

        drop(ContentStore::open(&journal_root.config(u64::MAX)).expect("journal reopen"));
        drop(ContentStore::open(&rewrite_root.config(u64::MAX)).expect("rewrite reopen"));
        assert_eq!(
            fs::read(&journal_manifest).expect("manifest"),
            fs::read(&rewrite_manifest).expect("manifest"),
            "journal replay recovers the exact books a wholesale rewrite persists"
        );
    }

    /// The byte-identity pin's op sequence. The 17-byte cap forces an
    /// eviction at every flush; only `every` differs between the paired
    /// runs. Every move here is a real delta with a fixed stamp — a dedup
    /// re-hit must stay out of the sequence, since its in-memory-only
    /// clock move is exactly the touch divergence the journal documents
    /// (the row lands at the entry's next move, not at another entry's).
    fn run_pinned_sequence(scratch: &Scratch, every: u32) {
        let mut store = ContentStore::open(&scratch.config(17)).expect("open");
        store.set_flushes_per_compaction(every);
        let a = store.put_at(b"alpha payload", 100);
        let b = store.put_at(b"beta", 200);
        let c = store.put_at(b"gamma", 300);
        store.flush(); // evicts the oldest touch (`a`); one del + two sets
        let rejoined = store.put_at(b"alpha payload", 400);
        assert_eq!(rejoined, a, "the content-addressed id is stable");
        store.flush(); // evicts the two smaller rows (`b`, `c`)
        let d = store.put_at(b"delta payload", 500);
        store.flush(); // evicts the rejoined `a`
        assert!(
            store.contains(&d) && !store.contains(&a) && !store.contains(&b) && !store.contains(&c),
            "only the newest payload survives the cap-forced sequence"
        );
        assert_eq!(store.total_bytes(), 13, "the cap held through the sequence");
    }

    /// The daemon's pins survive the journal: retain/release rows replay
    /// to the same refcounts a wholesale rewrite persists. A books-level
    /// pin — `retain`'s wall-clock touch keeps the paired roots' bytes
    /// free to differ.
    #[test]
    fn journal_replay_restores_refcounts_like_full_rewrite() {
        fn pin_sequence(scratch: &Scratch, every: u32) -> (ContentId, ContentId) {
            let mut store = ContentStore::open(&scratch.config(u64::MAX)).expect("open");
            store.set_flushes_per_compaction(every);
            let a = store.put_at(b"alpha", 100);
            let b = store.put_at(b"beta", 200);
            store.flush();
            assert!(store.retain(&a) && store.retain(&a) && store.retain(&b));
            assert!(store.release(&a));
            store.flush();
            (a, b)
        }

        let journal_root = Scratch::new("pin-journal");
        let rewrite_root = Scratch::new("pin-rewrite");
        // Journal cadence: both flushes stay journal-only (no compaction
        // crosses the sequence), so recovery rides the replay alone.
        let (ja, jb) = pin_sequence(&journal_root, FLUSHES_PER_COMPACTION);
        let (ra, rb) = pin_sequence(&rewrite_root, 1);
        assert_eq!(ja, ra, "same content, same ids");
        assert_eq!(jb, rb);

        let journal = ContentStore::open(&journal_root.config(u64::MAX)).expect("reopen");
        let rewrite = ContentStore::open(&rewrite_root.config(u64::MAX)).expect("reopen");
        assert_eq!(
            journal.refcount_of(&ja),
            Some(1),
            "journal replay restores the double-pin-minus-one"
        );
        assert_eq!(rewrite.refcount_of(&ra), Some(1));
        assert_eq!(journal.refcount_of(&jb), Some(1));
        assert_eq!(journal.len(), rewrite.len());
        assert_eq!(journal.total_bytes(), rewrite.total_bytes());
    }

    /// The atomicity contract: a torn journal tail — a partial final line
    /// from a killed append — recovers to the last complete entry, and
    /// garbage ahead of a row stops replay there (a skip-past-garbage
    /// replay would find the row). Recovery never bricks the store.
    #[test]
    fn torn_journal_tail_recovers_to_last_complete_entry() {
        let scratch = Scratch::new("torn-tail");
        let mut store = ContentStore::open(&scratch.config(u64::MAX)).expect("open");
        let a = store.put_at(b"alpha", 100);
        let b = store.put_at(b"beta", 200);
        store.flush();
        let journal = scratch.0.join("store").join("manifest.jsonl");
        let complete = fs::read(&journal).expect("journal");

        // A killed append: half a `set` row, no trailing newline.
        let mut torn = complete.clone();
        torn.extend_from_slice(br#"{"op":"set","id":"af"#);
        fs::write(&journal, &torn).expect("tear the tail");
        drop(store);

        let reopened = ContentStore::open(&scratch.config(u64::MAX)).expect("reopen");
        assert!(
            reopened.contains(&a) && reopened.contains(&b),
            "the complete rows replay; the torn tail drops"
        );
        assert_eq!(reopened.total_bytes(), 9);
        drop(reopened);

        // Stop at the first bad line: a fresh base (open compaction wrote
        // it), one planted garbage line, then the row journaling `g` — a
        // replay that skipped past garbage would find `g`.
        let mut store = ContentStore::open(&scratch.config(u64::MAX)).expect("reopen");
        let g = store.put(b"fresh payload");
        store.flush();
        drop(store);
        let row = fs::read(&journal).expect("journal");
        let mut planted = b"garbage\n".to_vec();
        planted.extend_from_slice(&row);
        fs::write(&journal, &planted).expect("plant garbage ahead of the row");
        let damaged = ContentStore::open(&scratch.config(u64::MAX)).expect("reopen");
        assert!(
            !damaged.contains(&g),
            "replay stops at the first bad line — the row behind it is forfeit"
        );
        drop(damaged);

        // Recovery never bricks the store: the next delta round-trips.
        let mut store = ContentStore::open(&scratch.config(u64::MAX)).expect("reopen");
        let c = store.put(b"gamma");
        store.flush();
        drop(store);
        let reopened = ContentStore::open(&scratch.config(u64::MAX)).expect("reopen");
        assert!(
            reopened.contains(&a) && reopened.contains(&b) && reopened.contains(&c),
            "the store keeps taking deltas after a damaged-journal recovery"
        );
    }

    /// Termination (#1385): the journal is bounded by the compaction
    /// cadence — a dirty flush appends O(changed) rows, the cadence-th
    /// dirty flush compacts the journal away into a current manifest, and
    /// the base manifest stands in between.
    #[test]
    fn journal_compacts_at_the_threshold() {
        let scratch = Scratch::new("threshold");
        let mut store = ContentStore::open(&scratch.config(u64::MAX)).expect("open");
        store.set_flushes_per_compaction(2);
        let a = store.put_at(b"alpha", 100);
        let b = store.put_at(b"beta", 200);
        store.flush();
        let journal = scratch.0.join("store").join("manifest.jsonl");
        let manifest = scratch.0.join("store").join("manifest.json");
        let after_open = fs::read(&manifest).expect("manifest");
        assert!(
            journal.is_file(),
            "the first dirty flush appends the journal"
        );
        assert_eq!(
            fs::read(&manifest).expect("manifest"),
            after_open,
            "the base manifest stands under the journal"
        );

        let c = store.put_at(b"gamma", 300);
        store.flush();
        assert!(
            !journal.exists(),
            "the threshold flush compacts the journal away"
        );
        drop(store);

        let reopened = ContentStore::open(&scratch.config(u64::MAX)).expect("reopen");
        assert!(
            reopened.contains(&a) && reopened.contains(&b) && reopened.contains(&c),
            "the compaction absorbed every appended row"
        );
    }

    /// The books lock is a sidecar beside the manifest and never a
    /// correctness dependency: the uncontended open/flush sequence lands
    /// the manifest bytes the deterministic pin fixes, the sidecar exists
    /// after open, and a foreign holder taking and releasing the lock
    /// perturbs not one byte.
    #[test]
    fn books_lock_is_a_sidecar_and_never_touches_the_bytes() {
        let scratch = Scratch::new("lock-sidecar");
        let mut store = ContentStore::open(&scratch.config(u64::MAX)).expect("open");
        // Cadence 1 compacts at this flush, so the captured bytes are the
        // whole books: at the default cadence the put rides the journal and
        // the reopen's unconditional compaction would rightly materialize
        // the row into the base — that is journal absorption (the
        // contended-open pin spells this out), not lock perturbation.
        store.set_flushes_per_compaction(1);
        let id = store.put(b"sidecar");
        store.flush();
        let store_dir = scratch.0.join("store");
        let manifest_bytes = fs::read(store_dir.join("manifest.json")).expect("manifest");
        assert!(
            store_dir.join("manifest.lock").is_file(),
            "the lock sidecar sits beside the manifest"
        );
        drop(store);

        let holder = fs::File::create(store_dir.join("manifest.lock")).expect("lock file");
        holder.try_lock().expect("foreign hold");
        drop(holder);

        let reopened = ContentStore::open(&scratch.config(u64::MAX)).expect("reopen");
        assert_eq!(
            fs::read(store_dir.join("manifest.json")).expect("manifest"),
            manifest_bytes,
            "the lock round trip never moves a manifest byte"
        );
        assert!(reopened.contains(&id));
    }

    /// A contended open degrades exactly like the pre-lock world and
    /// never corrupts the books: with a foreign holder owning the lock,
    /// `open` outlives the bounded wait, proceeds unlocked, loads the
    /// healthy books from the manifest, and its unconditional compaction
    /// rewrites those same books byte-identically — last-write-wins, no
    /// mixture, no loss. The foreign hold is the second process spelled
    /// with the same primitive: advisory locks contend across separate
    /// handles in one process too.
    #[test]
    fn contended_open_proceeds_unlocked_without_corruption() {
        let scratch = Scratch::new("lock-contended");
        let mut store = ContentStore::open(&scratch.config(u64::MAX)).expect("open");
        // Cadence 1 compacts at this flush, so the captured bytes are the
        // whole books: at the default cadence the put rides the journal
        // and the degraded open's compaction rightly materializes the row
        // into the base — that is journal absorption, not corruption.
        store.set_flushes_per_compaction(1);
        let id = store.put(b"contended payload");
        store.flush();
        let store_dir = scratch.0.join("store");
        let manifest_bytes = fs::read(store_dir.join("manifest.json")).expect("manifest");
        drop(store);

        let holder = fs::File::create(store_dir.join("manifest.lock")).expect("lock file");
        holder.try_lock().expect("foreign hold");

        let reopened = ContentStore::open(&scratch.config(u64::MAX)).expect("open degraded");
        assert!(
            reopened.contains(&id),
            "the unlocked open still loads the healthy books"
        );
        assert_eq!(reopened.total_bytes(), bytes(b"contended payload"));
        assert_eq!(
            fs::read(store_dir.join("manifest.json")).expect("manifest"),
            manifest_bytes,
            "the degraded compaction rewrites exactly the books it loaded"
        );
        drop(holder);

        // With the holder gone the next open takes the lock after its wait.
        drop(ContentStore::open(&scratch.config(u64::MAX)).expect("reopen"));
    }

    /// A contended flush degrades to the unlocked append — the journal
    /// rows still land, and the next uncontended open replays them: the
    /// bounded wait bounds latency, never the write.
    #[test]
    fn contended_flush_still_lands_its_rows() {
        let scratch = Scratch::new("lock-flush");
        let mut store = ContentStore::open(&scratch.config(u64::MAX)).expect("open");
        let a = store.put(b"alpha");
        store.flush();
        let store_dir = scratch.0.join("store");

        let holder = fs::File::create(store_dir.join("manifest.lock")).expect("lock file");
        holder.try_lock().expect("foreign hold");

        let b = store.put(b"beta");
        store.flush(); // contends, outlives the wait, appends unlocked
        assert!(store.contains(&b));
        drop(store);
        drop(holder);

        let reopened = ContentStore::open(&scratch.config(u64::MAX)).expect("reopen");
        assert!(
            reopened.contains(&a) && reopened.contains(&b),
            "the degraded append replayed like any unlocked journal"
        );
    }
}
