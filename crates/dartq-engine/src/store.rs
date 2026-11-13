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
//! # Answer-record sidecars (#4273)
//!
//! `<root>/answers/<key>.answer` — the answered-turn records the analyze
//! faces persist (#4243) — fold into the store's byte budget: every
//! [`ContentStore::open`] (the cold path; an ask never pays for it) sweeps
//! the dir, unlinking sidecars whose blob is gone from the store (already
//! the probe's `blob_missing` miss — the record can never hit again),
//! foreign or torn records, and aged write debris, then LRU-capping the
//! survivors' bytes under the same [`StoreConfig::size_limit`] the books
//! enforce — each face, blobs and sidecars, independently at or under the
//! flag. The LRU axis is the sidecar's mtime: the atomic rename each
//! persist lands makes it the last-persist clock, the same field the
//! books' crash rebuild reads, and choosing it costs no record schema
//! change (existing sidecars keep serving). Probe-time misses unlink
//! their dead record too, under the concurrent-writer guard — see
//! [`remove_answer_record_if`].
//!
//! # Debris sweep (#4053)
//!
//! Two crash classes leave files eviction can never see back: a blob whose
//! rename landed but whose journal row never did (the writer died between
//! the flush's blob write and its append — the manifest loads healthy, so
//! the mtime rebuild never runs, and [`ContentStore::evict`] walks only
//! booked rows), and `<id>.tmp` debris from a write torn before its
//! rename. [`ContentStore::sweep_debris`] reclaims both, each file only
//! past [`SWEEP_GRACE`] of mtime age — the timestamp class the books
//! already use (the rebuild reads the same field as `last_touch`). The
//! grace covers the write-completion horizon: a blob renamed but not yet
//! journaled becomes booked within one apply batch (the daemon's flush
//! cadence), so a concurrent writer's in-flight files are orders of
//! magnitude inside the grace and never swept. The sweep is a `dartq
//! cache gc` pass, never an open-time one — the read path pays nothing
//! for the write path's crash hygiene.
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
use std::path::{Path, PathBuf};
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

/// How old a file must be before [`ContentStore::sweep_debris`] removes
/// it: an off-books blob or `<id>.tmp` debris with a younger mtime may be
/// an in-flight write (blob renamed, journal row pending — a window that
/// closes within one apply batch) and stays. One hour is three orders of
/// magnitude past that window and matches the build cache's grace window
/// (`grace_secs`, the cache family's "well past in-flight" constant). A
/// future mtime (a clock wound backwards) reads as young — never swept.
const SWEEP_GRACE: Duration = Duration::from_secs(3600);

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

/// The answered-turn sidecar's path under a store root (#4243):
/// `<root>/answers/<key-hex>.answer` — the small mutable record beside
/// the content-addressed blob the store holds (stamps move on every
/// touch, so the record cannot live under content addressing).
#[must_use]
pub fn answer_record_path(root: &Path, key: &[u8; 16]) -> PathBuf {
    let hex: String = key.iter().map(|byte| format!("{byte:02x}")).collect();
    root.join("answers").join(format!("{hex}.answer"))
}

/// A content blob's on-disk path under a store root: the two-hex shard
/// directory, then the full hex name (#4260's unassembled probe reads
/// blobs by path without opening the books; [`ContentStore::blob_path`]
/// delegates here, one spelling for both).
#[must_use]
pub fn answer_blob_path(root: &Path, id: &ContentId) -> PathBuf {
    let hex = id.to_string();
    root.join("store").join(&hex[..2]).join(hex)
}

/// The temp-file counter for [`write_answer_record`]'s sibling temps.
static ANSWER_TMP_SEQ: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// How many prefix bytes [`answer_prefix_blob`] reads per sidecar: the
/// `version` field's varint (1), the `blob` string's varint length (1),
/// and the 32-hex [`ContentId`] spelling (32). The sweep pays this, not a
/// whole-record read — a monorepo stamp set makes one sidecar megabytes,
/// and a 10 GiB dir of those must not move at open.
const ANSWER_PREFIX_LEN: usize = 34;

/// The blob id peeked from an answer sidecar's first
/// [`ANSWER_PREFIX_LEN`] bytes: the record's leading `version` varint,
/// the `blob` string's varint length, and the hex id itself — the
/// positional-bincode layout under the varint standard config
/// (`AnsweredRecord { version, blob, .. }`), pinned by a codec pin test
/// so a schema change that moves the prefix fails the pin, not
/// production. `None` for anything else (a foreign version, a torn or
/// short prefix, a non-hex blob spelling) — the sweep's verdict that the
/// record is dead weight.
fn answer_prefix_blob(prefix: &[u8]) -> Option<ContentId> {
    let version = crate::state::persist::ANSWER_RECORD_VERSION;
    if prefix.first() != Some(&(version as u8)) || prefix.get(1) != Some(&32) {
        return None;
    }
    let hex = std::str::from_utf8(prefix.get(2..ANSWER_PREFIX_LEN)?).ok()?;
    ContentId::parse_hex(hex).ok()
}

/// Writes one answer-record sidecar atomically (#4243): a unique sibling
/// temp file, then a same-filesystem rename over the destination — the
/// snapshot store's save discipline (a torn write leaves the previous
/// record or none, both misses; durability is deliberately omitted, a
/// young record lost to power loss is an acceptable miss).
///
/// # Errors
///
/// Any [`std::io::Error`] from creating the answers directory or the
/// write-rename pair — the caller treats a failed persist as a future
/// miss, never an error.
pub fn write_answer_record(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("answer");
    let tmp = path.with_file_name(format!(
        "{name}.tmp-{}-{}",
        std::process::id(),
        ANSWER_TMP_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let written = fs::write(&tmp, bytes).and_then(|()| fs::rename(&tmp, path));
    if written.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    written
}

/// Reads one answer-record sidecar; any I/O failure is the caller's miss.
///
/// # Errors
///
/// Any [`std::io::Error`] from the read (absent, unreadable, torn).
pub fn read_answer_record(path: &Path) -> std::io::Result<Vec<u8>> {
    fs::read(path)
}

/// Removes the answer record at `path` only if its bytes still equal
/// `judged` — the miss-path unlink's concurrent-writer guard (#4273): a
/// probe that judged a sidecar dead (decode, stamps, or blob miss — a
/// verdict that can never hit again without a re-persist) re-reads before
/// unlinking, so a writer whose atomic rename landed fresh bytes between
/// the two reads keeps its record: fresh content cannot byte-equal the
/// stale bytes the probe judged (the stamp sets differ by construction).
/// A completed hit is immune by construction — its bytes are already in
/// memory, and unlinking never disturbs an open file. Best-effort, like
/// every sidecar write: a failed read or unlink leaves the record for the
/// next open's sweep.
pub fn remove_answer_record_if(path: &Path, judged: &[u8]) {
    if fs::read(path).is_ok_and(|current| current == judged) {
        let _ = fs::remove_file(path);
    }
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

/// What one [`ContentStore::sweep_debris`] pass reclaimed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SweepReport {
    /// Off-books blobs removed: present on disk, absent from the books,
    /// older than [`SWEEP_GRACE`].
    pub blobs_swept: u64,
    /// Bytes those blobs occupied.
    pub blob_bytes_freed: u64,
    /// `<id>.tmp` write-debris files removed, past the same grace.
    pub tmp_swept: u64,
    /// Bytes that debris occupied.
    pub tmp_bytes_freed: u64,
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
    /// The cache root the config named — the answer-record sidecars'
    /// parent (`<root>/answers`) and the blob directory's parent.
    root: PathBuf,
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
            root: config.root.clone(),
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
        // The answers fold (#4273) runs after the open-time eviction, so
        // records stranded by this very eviction reclaim in the same open;
        // the cold path pays it, never an ask.
        store.sweep_answers();
        // Unconditional compaction at open (the full manifest write open
        // has always paid): it normalizes rebuilt books into a fresh base
        // and absorbs any journal the previous session left.
        store.compact();
        Ok(store)
    }

    /// The cache root this store was opened under — the answer-record
    /// sidecars' parent (`<root>/answers`, [#4243](crate::state::persist)).
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Files one payload under its content digest and books it.
    /// Already-stored content writes nothing (global dedup) — it just
    /// re-touches the LRU clock. A failed blob write books nothing and
    /// reports nothing (the accelerator posture); [`Self::contains`] tells
    /// the truth afterwards.
    pub fn put(&mut self, payload: &[u8]) -> ContentId {
        self.put_at(payload, now_secs())
    }

    /// [`Self::put`] over a whole apply batch whose content ids the caller
    /// already holds (the residency's `facts.content` is the same blake3
    /// face, so nothing re-hashes). The blob writes — one
    /// create + write + rename per new content, the batch's only I/O — fan
    /// out over rayon instead of serializing the cold walk's whole
    /// workspace on the main thread; the books then mutate serially in
    /// batch order, standing as the one-by-one puts leave them: dedup hits
    /// re-touch, landed writes book, failed writes book nothing. An id
    /// repeated inside the batch (same-content files) writes once — the
    /// one-by-one form would retry a failed first write at the repeat, a
    /// difference confined to the accelerator's best-effort face (a failed
    /// write is retried by the next batch carrying the content either way).
    pub(crate) fn put_known_batch(&mut self, batch: &[(ContentId, &[u8])]) {
        use rayon::prelude::*;
        let now = now_secs();
        let mut seen: rustc_hash::FxHashSet<ContentId> = rustc_hash::FxHashSet::default();
        let fresh: Vec<(ContentId, &[u8])> = batch
            .iter()
            .filter(|(id, _)| !self.entries.contains_key(id) && seen.insert(*id))
            .copied()
            .collect();
        let landed: Vec<bool> = fresh
            .par_iter()
            .map(|(id, payload)| self.write_blob(id, payload))
            .collect();
        for (id, _) in batch {
            if let Some(entry) = self.entries.get_mut(id) {
                entry.last_touch = now;
            }
        }
        for ((id, payload), landed) in fresh.into_iter().zip(landed) {
            if !landed {
                continue;
            }
            let size = u64::try_from(payload.len()).unwrap_or(u64::MAX);
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

    /// The gc-only integrity pass: reclaims what eviction can never see
    /// back — blobs on disk the books do not carry (a writer that died
    /// between its blob's rename and the journal append) and stale
    /// `<id>.tmp` write debris. Both classes go only past [`SWEEP_GRACE`]
    /// of mtime age, so a concurrent writer's in-flight files are never
    /// touched; booked blobs are not candidates at any age (eviction owns
    /// those, pin-gated). The books are neither read from nor written to
    /// disk here — everything removed is by definition off them — so no
    /// manifest bytes move and the books lock stays untaken. Never called
    /// on the read path ([`Self::open`] stays free of it); `dartq cache
    /// gc` is the caller.
    pub fn sweep_debris(&self) -> SweepReport {
        let mut report = SweepReport::default();
        let Ok(shards) = fs::read_dir(&self.dir) else {
            return report;
        };
        for shard in shards.flatten() {
            if !shard.file_type().is_ok_and(|kind| kind.is_dir()) {
                continue;
            }
            let Ok(files) = fs::read_dir(shard.path()) else {
                continue;
            };
            for file in files.flatten() {
                let Ok(meta) = file.metadata() else {
                    continue;
                };
                if !meta.is_file() {
                    continue;
                }
                // The grace check first: it skips every young file without
                // a name parse, and `elapsed` failing (a future mtime)
                // reads as young — never swept.
                let aged = meta
                    .modified()
                    .ok()
                    .and_then(|mtime| mtime.elapsed().ok())
                    .is_some_and(|age| age >= SWEEP_GRACE);
                if !aged {
                    continue;
                }
                let file_name = file.file_name();
                let Some(name) = file_name.to_str() else {
                    continue;
                };
                let size = meta.len();
                if let Ok(id) = ContentId::parse_hex(name) {
                    if !self.entries.contains_key(&id) && fs::remove_file(file.path()).is_ok() {
                        report.blobs_swept += 1;
                        report.blob_bytes_freed += size;
                    }
                } else if let Some(stem) = name.strip_suffix(".tmp")
                    && ContentId::parse_hex(stem).is_ok()
                    && fs::remove_file(file.path()).is_ok()
                {
                    report.tmp_swept += 1;
                    report.tmp_bytes_freed += size;
                }
            }
        }
        report
    }

    /// The answers fold (#4273): the open-time sweep of
    /// `<root>/answers` — the answered-turn sidecars' bounded lifecycle,
    /// paid once per open (the cold path) and never inside an ask. Three
    /// cuts, in walk order:
    ///
    /// * dead records unlink outright — a blob gone from the store (the
    ///   probe's `blob_missing` miss, permanent until a re-persist whose
    ///   put-then-sidecar ordering means any sidecar on disk references a
    ///   blob whose rename already landed, so a present sidecar with an
    ///   absent blob was killed by eviction, not by an in-flight writer),
    ///   or a prefix that no longer names a version-and-blob record
    ///   (torn, foreign, or a future codec — dead weight for this
    ///   binary, whose own keys fork away from a foreign generation
    ///   anyway);
    /// * `<key>.answer.tmp-*` write debris unlinks past [`SWEEP_GRACE`]
    ///   — the same in-flight protection [`Self::sweep_debris`] gives
    ///   the store's own temps, while a final-path record is never
    ///   in-flight (the atomic rename discipline) and needs no grace;
    /// * the survivors LRU-cap at [`Self::limit`] — the same byte budget
    ///   and the same posture the books just enforced, evicting
    ///   oldest-mtime first (the last-persist clock) with a
    ///   path-ascending tie-break for determinism.
    ///
    /// Best-effort throughout (the accelerator posture): any I/O failure
    /// skips its file, and a record this pass misses stays for the next
    /// open. A concurrent writer's fresh record is the mtime-newest, so
    /// LRU pressure reaches it last — the same documented degradation as
    /// the books' own eviction under a pathologically small cap.
    fn sweep_answers(&self) {
        let Ok(files) = fs::read_dir(self.root.join("answers")) else {
            return;
        };
        let mut live: Vec<(u64, u64, PathBuf)> = Vec::new();
        for file in files.flatten() {
            let Ok(meta) = file.metadata() else {
                continue;
            };
            if !meta.is_file() {
                continue;
            }
            let file_name = file.file_name();
            let Some(name) = file_name.to_str() else {
                continue;
            };
            let path = file.path();
            if name.contains(".answer.tmp-") {
                // Write debris: reclaimed only past the grace, so a
                // concurrent writer's in-flight temp is never touched —
                // `sweep_debris`'s guard, same constant, same reason.
                let aged = meta
                    .modified()
                    .ok()
                    .and_then(|mtime| mtime.elapsed().ok())
                    .is_some_and(|age| age >= SWEEP_GRACE);
                if aged {
                    let _ = fs::remove_file(&path);
                }
            } else if name.ends_with(".answer") {
                // A final-path record is never in-flight (atomic rename),
                // so the dead-record cuts need no grace.
                let dead = match read_prefix(&path) {
                    None => false, // unreadable right now: not provably dead
                    Some(prefix) => match answer_prefix_blob(&prefix) {
                        None => true,
                        Some(blob) => !answer_blob_path(&self.root, &blob).is_file(),
                    },
                };
                if dead {
                    let _ = fs::remove_file(&path);
                    continue;
                }
                let mtime = meta
                    .modified()
                    .ok()
                    .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                    .map_or(0, |span| span.as_secs());
                live.push((mtime, meta.len(), path));
            }
        }
        let mut total: u64 = live.iter().map(|&(_, size, _)| size).sum();
        if total <= self.limit {
            return;
        }
        live.sort_unstable();
        for (_, size, path) in live {
            if total <= self.limit {
                break;
            }
            if fs::remove_file(&path).is_ok() {
                total = total.saturating_sub(size);
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
        answer_blob_path(&self.root, id)
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

/// The first [`ANSWER_PREFIX_LEN`] bytes of the file at `path` — fewer
/// when the file is shorter, which is itself a verdict (a record too
/// small to carry version-and-blob is dead; [`answer_prefix_blob`] reads
/// a short slice as `None`). `None` only on an I/O failure, where the
/// file is not provably dead and stays for the next open.
fn read_prefix(path: &Path) -> Option<Vec<u8>> {
    use std::io::Read;
    let mut prefix = Vec::with_capacity(ANSWER_PREFIX_LEN);
    fs::File::open(path)
        .ok()?
        .take(ANSWER_PREFIX_LEN as u64)
        .read_to_end(&mut prefix)
        .ok()?;
    Some(prefix)
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
    use filetime::{FileTime, set_file_mtime};
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

    /// The batch put books exactly what the one-by-one puts book: a
    /// standing entry re-touches without a delta, a batch-internal repeat
    /// writes and books once, and every fresh blob reads back byte-equal.
    #[test]
    fn batch_put_books_like_one_by_one_puts() {
        let scratch = Scratch::new("batch-put");
        let mut store = ContentStore::open(&scratch.config(u64::MAX)).expect("open");
        let standing = store.put_at(b"standing", 1);
        store.flush();
        let fresh_a = content_hash(b"fresh-a");
        let fresh_b = content_hash(b"fresh-b");
        let batch: [(ContentId, &[u8]); 4] = [
            (standing, b"standing"),
            (fresh_a, b"fresh-a"),
            (fresh_b, b"fresh-b"),
            (fresh_a, b"fresh-a"),
        ];
        store.put_known_batch(&batch);
        assert_eq!(store.len(), 3, "one row per distinct id");
        assert_eq!(
            store.total_bytes(),
            (b"standing".len() + b"fresh-a".len() + b"fresh-b".len()) as u64,
            "the repeat books its bytes once"
        );
        assert_eq!(store.read(&fresh_a).expect("landed"), b"fresh-a");
        assert_eq!(store.read(&fresh_b).expect("landed"), b"fresh-b");
        assert!(
            !store.delta.contains(&standing),
            "a dedup hit is a re-touch, not a book delta"
        );
        assert!(store.delta.contains(&fresh_a) && store.delta.contains(&fresh_b));
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

    /// The gc-only integrity sweep (#4053): a blob whose rename landed but
    /// whose journal row never did, and a torn write's `.tmp`, are
    /// reclaimed — each only past [`SWEEP_GRACE`]; young debris (a possible
    /// in-flight write) and booked blobs at any age are never candidates,
    /// and the books move not one row.
    #[test]
    fn sweep_reclaims_aged_debris_and_spares_the_rest() {
        let scratch = Scratch::new("sweep");
        let payload = b"booked payload";
        let mut store = ContentStore::open(&scratch.config(u64::MAX)).expect("open");
        let booked = store.put(payload);
        store.flush();
        let store_dir = scratch.0.join("store");
        let epoch = FileTime::from_unix_time(0, 0);

        // A booked blob aged to the epoch: eviction's territory, never
        // the sweep's.
        let booked_hex = booked.to_string();
        let booked_path = store_dir.join(&booked_hex[..2]).join(&booked_hex);
        set_file_mtime(&booked_path, epoch).expect("age booked blob");

        // Aged off-books debris: the blob a dead writer renamed but never
        // journaled.
        let orphan = content_hash(b"orphan payload");
        let orphan_hex = orphan.to_string();
        let orphan_path = store_dir.join(&orphan_hex[..2]).join(&orphan_hex);
        fs::create_dir_all(orphan_path.parent().expect("shard")).expect("orphan shard");
        fs::write(&orphan_path, b"orphan payload").expect("orphan blob");
        set_file_mtime(&orphan_path, epoch).expect("age orphan");
        let tmp_old = store_dir
            .join(&orphan_hex[..2])
            .join(format!("{orphan_hex}.tmp"));
        fs::write(&tmp_old, b"half-written").expect("aged tmp");
        set_file_mtime(&tmp_old, epoch).expect("age tmp");

        // Young debris: a possible in-flight write, untouchable.
        let fresh = content_hash(b"fresh orphan");
        let fresh_hex = fresh.to_string();
        let fresh_path = store_dir.join(&fresh_hex[..2]).join(&fresh_hex);
        fs::create_dir_all(fresh_path.parent().expect("shard")).expect("fresh shard");
        fs::write(&fresh_path, b"fresh orphan").expect("young blob");
        let tmp_young = store_dir
            .join(&fresh_hex[..2])
            .join(format!("{fresh_hex}.tmp"));
        fs::write(&tmp_young, b"half-written").expect("young tmp");

        let report = store.sweep_debris();

        assert_eq!(
            report,
            SweepReport {
                blobs_swept: 1,
                blob_bytes_freed: bytes(b"orphan payload"),
                tmp_swept: 1,
                tmp_bytes_freed: bytes(b"half-written"),
            },
            "one aged blob and one aged tmp reclaimed, both counted with bytes"
        );
        assert!(
            !orphan_path.exists() && !tmp_old.exists(),
            "aged debris gone"
        );
        assert!(
            fresh_path.is_file() && tmp_young.is_file(),
            "young debris survives the grace guard"
        );
        assert!(
            booked_path.is_file() && store.contains(&booked),
            "a booked blob at any age is eviction's, never the sweep's"
        );

        // The books never moved: reopen loads them unchanged, and the
        // young debris still on disk stays off them.
        drop(store);
        let reopened = ContentStore::open(&scratch.config(u64::MAX)).expect("reopen");
        assert!(reopened.contains(&booked));
        assert_eq!(reopened.total_bytes(), bytes(payload));
        assert!(
            !reopened.contains(&fresh),
            "young debris stays off the books"
        );
    }

    /// One sidecar record for the answers fixtures: the persist side's
    /// shape over a caller-chosen blob id (#4273).
    fn answer_record(blob: &ContentId) -> crate::state::persist::AnsweredRecord {
        crate::state::persist::AnsweredRecord {
            version: crate::state::persist::ANSWER_RECORD_VERSION,
            blob: blob.to_string(),
            stamps: vec![(
                PathBuf::from("/fixture/target.dart"),
                crate::state::persist::AnswerStamp {
                    dev: 0,
                    ino: 0,
                    mtime_ns: 0,
                    size: 0,
                },
            )],
            summary: crate::state::persist::AnswerSummary::default(),
        }
    }

    /// Plants a blob file at the store's content-addressed path without
    /// booking it — the sweep reads blob existence off the disk path, the
    /// probe's own truth, so the books never enter these fixtures.
    fn plant_blob(root: &Path, payload: &[u8]) -> ContentId {
        let id = content_hash(payload);
        let path = answer_blob_path(root, &id);
        fs::create_dir_all(path.parent().expect("shard")).expect("blob shard");
        fs::write(&path, payload).expect("plant blob");
        id
    }

    /// Mints one sidecar the way the persist side mints it (#4273
    /// fixtures): encoded record under `key`, beside its (planted) blob.
    fn mint_answer(root: &Path, key: &[u8; 16], blob: &ContentId) -> (PathBuf, Vec<u8>) {
        let bytes =
            crate::state::persist::encode_answer(&answer_record(blob)).expect("encodes");
        let path = answer_record_path(root, key);
        write_answer_record(&path, &bytes).expect("mint sidecar");
        (path, bytes)
    }

    /// The answers fold at open (#4273): a sidecar whose blob is gone —
    /// the probe's permanent `blob_missing` miss — a future-codec record,
    /// and aged write debris all reclaim; a live record, a young
    /// in-flight temp, and a file unreadable right now stay for the next
    /// open.
    #[test]
    fn answers_open_sweep_reclaims_dead_records_and_spares_live() {
        let scratch = Scratch::new("answers-sweep");
        drop(ContentStore::open(&scratch.config(u64::MAX)).expect("open"));
        let live = plant_blob(&scratch.0, b"live answer bytes");
        let (live_path, _) = mint_answer(&scratch.0, &[1; 16], &live);

        // Dead: the blob never landed on disk (evicted between the
        // sidecar writes of two sessions, or a stranded record).
        let ghost = content_hash(b"never stored");
        let (ghost_path, _) = mint_answer(&scratch.0, &[2; 16], &ghost);

        // Dead: a future codec's record — foreign bytes for this binary.
        let mut future = answer_record(&live);
        future.version += 1;
        let future_bytes = crate::state::persist::encode_answer(&future).expect("encodes");
        let future_path = answer_record_path(&scratch.0, &[3; 16]);
        write_answer_record(&future_path, &future_bytes).expect("mint future");

        // Dead: a prefix too short to carry version-and-blob at all.
        let torn_path = answer_record_path(&scratch.0, &[4; 16]);
        fs::write(&torn_path, b"\x01\x20ab").expect("mint torn");

        // Write debris: young survives the grace, aged reclaims.
        let answers = scratch.0.join("answers");
        let young_tmp = answers.join("0000.answer.tmp-1-1");
        fs::write(&young_tmp, b"half-written").expect("young tmp");
        let aged_tmp = answers.join("ffff.answer.tmp-2-1");
        fs::write(&aged_tmp, b"half-written").expect("aged tmp");
        set_file_mtime(&aged_tmp, FileTime::from_unix_time(0, 0)).expect("age tmp");

        drop(ContentStore::open(&scratch.config(u64::MAX)).expect("reopen"));

        assert!(live_path.is_file(), "an in-budget live record survives open");
        assert!(
            !ghost_path.exists() && !future_path.exists() && !torn_path.exists(),
            "stale-blob, foreign-codec, and torn records are dead weight, reclaimed"
        );
        assert!(young_tmp.is_file(), "young debris may be in flight");
        assert!(!aged_tmp.exists(), "aged debris reclaims");
    }

    /// The answers budget at open (#4273): over-limit sidecar bytes evict
    /// oldest-mtime first — the last-persist clock — down to the cap, the
    /// books' own posture on the sidecar face; a record a concurrent
    /// writer just landed is the mtime-newest, so LRU pressure reaches it
    /// last.
    #[test]
    fn answers_open_sweep_lru_caps_to_the_budget() {
        let scratch = Scratch::new("answers-lru");
        drop(ContentStore::open(&scratch.config(u64::MAX)).expect("open"));
        let mut aged_sizes = Vec::new();
        for (index, key) in [[1u8; 16], [2; 16], [3; 16]].into_iter().enumerate() {
            let blob = plant_blob(&scratch.0, format!("aged payload {index}").as_bytes());
            let (path, _) = mint_answer(&scratch.0, &key, &blob);
            set_file_mtime(&path, FileTime::from_unix_time(1000 + index as i64, 0))
                .expect("age record");
            aged_sizes.push(fs::metadata(&path).expect("meta").len());
        }
        let fresh_blob = plant_blob(&scratch.0, b"fresh payload");
        let (fresh_path, _) = mint_answer(&scratch.0, &[9; 16], &fresh_blob);
        let fresh_size = fs::metadata(&fresh_path).expect("meta").len();

        // The budget holds all but the oldest aged record.
        let limit = aged_sizes[1] + aged_sizes[2] + fresh_size;
        drop(ContentStore::open(&scratch.config(limit)).expect("reopen"));

        assert!(
            !answer_record_path(&scratch.0, &[1; 16]).exists(),
            "the oldest last-persist clock evicts first"
        );
        assert!(
            answer_record_path(&scratch.0, &[2; 16]).is_file()
                && answer_record_path(&scratch.0, &[3; 16]).is_file()
                && fresh_path.is_file(),
            "in-budget records and the mtime-newest survive"
        );
        let surviving: u64 = fs::read_dir(scratch.0.join("answers"))
            .expect("answers dir")
            .flatten()
            .filter_map(|entry| entry.metadata().ok())
            .map(|meta| meta.len())
            .sum();
        assert!(
            surviving <= limit,
            "the sidecar face sits at or under the flag's budget ({surviving} <= {limit})"
        );
    }

    /// The miss-path unlink's concurrent-writer guard (#4273): only the
    /// bytes a probe actually judged dead may unlink — a re-persist
    /// landing fresh bytes between the probe's read and its unlink spares
    /// the record — and a missing path is a quiet no-op.
    #[test]
    fn answer_unlink_guard_spares_replaced_bytes() {
        let scratch = Scratch::new("answers-guard");
        drop(ContentStore::open(&scratch.config(u64::MAX)).expect("open"));
        let blob = plant_blob(&scratch.0, b"guard payload");
        let (path, judged) = mint_answer(&scratch.0, &[7; 16], &blob);

        remove_answer_record_if(&path, b"fresh bytes a writer just landed");
        assert!(path.is_file(), "replaced bytes are never unlinked");

        remove_answer_record_if(&path, &judged);
        assert!(!path.exists(), "the judged-stale bytes unlink");

        remove_answer_record_if(&scratch.0.join("answers").join("absent.answer"), b"x");
    }

    /// The sweep's prefix peek against the record codec (#4273): the
    /// varint-standard positional layout puts `version` at byte 0 and the
    /// 32-hex blob spelling at bytes 2..34, whatever the stamps payload
    /// behind them — the pin every record schema change must re-run. A
    /// foreign version and a short prefix read as dead.
    #[test]
    fn answer_prefix_blob_pins_the_codec_layout() {
        let blob = content_hash(b"pin target");
        let bytes =
            crate::state::persist::encode_answer(&answer_record(&blob)).expect("encodes");
        assert!(
            bytes.len() > ANSWER_PREFIX_LEN,
            "the fixture record carries a stamps payload past the prefix"
        );
        assert_eq!(
            answer_prefix_blob(&bytes),
            Some(blob),
            "version varint, string-length varint, then the hex id"
        );

        let mut foreign = answer_record(&blob);
        foreign.version += 1;
        let foreign_bytes =
            crate::state::persist::encode_answer(&foreign).expect("encodes");
        assert_eq!(
            answer_prefix_blob(&foreign_bytes),
            None,
            "a foreign version is dead weight for this binary"
        );
        assert_eq!(
            answer_prefix_blob(&bytes[..ANSWER_PREFIX_LEN - 1]),
            None,
            "a prefix too short for version-and-blob is torn"
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
