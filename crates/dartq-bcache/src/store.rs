//! The content-addressed store: entries addressed by key, payload blobs
//! deduplicated by content.
//!
//! # Layout
//!
//! Under the root the caller injects (default
//! `~/.cache/dartq/cas/{STORE_LAYOUT_VERSION}`):
//!
//! * `blobs/<hex[0:2]>/<hex>` — immutable payload named by the blake3 of
//!   its bytes, 256-way sharded. Entries that differ only in their key
//!   (say, four define combinations over the same sources) share one set
//!   of blobs.
//! * `entries/<key[0:2]>/<key>/manifest.json` — one [`EntryManifest`] per
//!   key, compact JSON.
//! * `entries/<key[0:2]>/<key>/hit` — the hit sidecar: its mtime is the
//!   last-hit clock (see [`CasStore::record_hit`]). Nothing ever parses
//!   its content; it exists so a hit never rewrites the manifest.
//! * `tmp/` — scratch space: every file is written here and renamed into
//!   place, never written in place.
//!
//! # Write protocol
//!
//! A publish is write scratch → sync → rename, so a reader of a store path
//! sees the previous file or the new one, never a partial write; the sync
//! orders the data ahead of the rename, so a crash can lose a fresh
//! publish but never leave a renamed file with torn content. Ingest
//! verifies each blob's bytes against its id over the bytes that landed
//! (a copy-on-write clone of the source, with a streaming copy as the
//! fallback), re-touches every blob it reuses without rewriting, and
//! publishes every blob before
//! the manifest that references them: a stored blob holds exactly the
//! content its name claims, and a published manifest never names a blob
//! its own gc could reclaim underneath it (see the concurrency section).
//!
//! # Concurrency
//!
//! Several dartq processes (one per worktree) may share a store; v2 takes
//! no locks. Content addressing makes blob publication idempotent — two
//! writers of one blob rename identical bytes onto the same name.
//! Manifests are last-writer-wins per key, and every writer of a key wrote
//! a valid entry for it. `tmp/` lives under the root, so every rename stays
//! on one volume and is atomic. A manifest that fails to parse surfaces as
//! [`BcacheError::Corrupt`] on lookup; callers degrade to a miss and
//! rebuild. The walkers behind `stats` and `gc` go further and refuse to
//! fail on one: an unreadable or undecodable manifest is skipped and
//! counted (`corrupt_manifests`), so no single bad file can disable the
//! disk budget.
//!
//! Last-hit bookkeeping is a sidecar file beside the manifest, never a
//! manifest rewrite: a `record_hit` racing an ingest of the same key can
//! no longer revert the manifest to an older build — the sidecar only
//! ever makes the entry look more recently used, and gc evicts by the
//! newer of the manifest's `last_hit_unix` and the sidecar's mtime.
//!
//! Reusing a stored blob re-touches its mtime before the referencing
//! manifest is published, so a concurrent gc's in-flight window spares
//! it; gc additionally re-stats a blob immediately before unlinking and
//! spares any file whose mtime moved since its scan. Between the two
//! guards, every reuse is visible to a gc that started before, during,
//! or after it.
//!
//! `gc` is the only operation that deletes. It spares files young enough
//! to belong to an ingest in flight, and tolerates the remaining races by
//! design: when a reader resolves an entry while gc evicts it, the worst
//! case is a materialize that finds a blob gone and reports
//! [`BcacheError::NotFound`]; the caller rebuilds, and the post-build
//! ingest restores the blob. Should a demonstrated failure ever call for
//! mutual exclusion, the seam is a store-wide lock held by `gc` alone —
//! readers and ingest stay lock-free.
//!
//! # Cost
//!
//! * `ingest`: per source, one clone attempt plus one read of the clone
//!   (hashing the bytes that landed), falling back to a bounded copy —
//!   on a copy-on-write filesystem that halves the I/O of the old
//!   read-and-copy loop; parallel across files; target: 100 MB of new
//!   blobs in under 2 s.
//! * `lookup`: one small file read and parse; target: under 5 ms warm.
//! * `record_hit`: metadata-only — one existence check and one sidecar
//!   timestamp write; no manifest I/O.
//! * `used_bytes`: one small counter read — the O(1) budget probe.
//! * `gc` / `stats`: the only full passes — one pass over the manifests
//!   plus one walk over the blobs, O(entries + blobs), holding one small
//!   record per blob. `gc` rebooks the usage counter to what it saw.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use rayon::prelude::*;
use rustc_hash::{FxHashMap, FxHashSet};
use serde::{Deserialize, Serialize};

use crate::error::{BcacheError, Result};
use crate::fsutil::{clone_file_hashed, mtime_ns, touch};
use crate::types::{BlobId, EntryManifest, StoreStats, blob_id_hex, is_hex_digest};

/// The store layout version: the final path component of the default
/// root (`~/.cache/dartq/cas/v2`), appended by the config glue. Cut
/// from v1 with no migration and no cross-version reads: 0.1.x is an
/// alpha under an explicit no-compatibility ruling, and schema drift
/// inside one version (new manifest enum variants filed under the same
/// directory) made released binaries treat newer manifests as corrupt —
/// and their gc delete them. A clean cut removes that class outright:
/// an old binary never sees a v2 path. v1 is left in place, never read
/// and never written again; an old binary keeps working on it untouched.
pub const STORE_LAYOUT_VERSION: &str = "v2";

const BLOBS: &str = "blobs";
const ENTRIES: &str = "entries";
const TMP: &str = "tmp";
const MANIFEST: &str = "manifest.json";
/// The hit sidecar beside a manifest: its mtime is the last-hit clock,
/// its content a diagnostic timestamp no code parses.
const HIT: &str = "hit";

const NANOS_PER_SEC: u64 = 1_000_000_000;

/// Streaming buffer per worker: hashing and copying never hold more than
/// this much of a file in memory, whatever its size.
const CHUNK: usize = 1 << 20;

/// Files younger than this may belong to an ingest still in flight in
/// another process (blobs written, manifest not yet published); gc never
/// deletes them, be they scratch files or unreferenced blobs.
const IN_FLIGHT_SECS: u64 = 60 * 60;

/// A store rooted at a caller-chosen directory.
#[derive(Debug)]
pub struct CasStore {
    root: PathBuf,
}

/// What one [`CasStore::gc`] pass did.
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug, Serialize, Deserialize)]
pub struct GcReport {
    /// Entries present when the pass started.
    pub entries_before: u64,
    /// Entries removed: LRU evictions plus unreadable manifests.
    pub entries_evicted: u64,
    /// Blobs removed for having no remaining reference.
    pub blobs_removed: u64,
    /// Bytes reclaimed: removed blobs plus stale scratch files.
    pub bytes_freed: u64,
    /// Manifests that failed to read or decode during the pass — evicted
    /// with the entries they belonged to (no lookup can serve them) and
    /// counted so a damaged or drifting store stays visible in every
    /// report instead of failing the pass.
    pub corrupt_manifests: u64,
}

impl CasStore {
    /// Opens the store at `root`, creating its directories if missing.
    /// Idempotent, and safe against concurrent opens of the same root.
    pub fn open(root: &Path) -> Result<Self> {
        for dir in [BLOBS, ENTRIES, TMP] {
            let dir = root.join(dir);
            fs::create_dir_all(&dir).map_err(io_at(&dir))?;
        }
        Ok(Self {
            root: root.to_path_buf(),
        })
    }

    /// Where blob `id` lives: `blobs/<hex[0:2]>/<hex>`.
    #[must_use]
    pub fn blob_path(&self, id: &BlobId) -> PathBuf {
        let hex = blob_id_hex(id);
        self.root.join(BLOBS).join(&hex[..2]).join(&hex)
    }

    /// The directory of entry `key`: `entries/<key[0:2]>/<key>`, holding
    /// the manifest and the hit sidecar.
    fn entry_dir(&self, key: &str) -> PathBuf {
        let shard = key.get(..2).unwrap_or(key);
        self.root.join(ENTRIES).join(shard).join(key)
    }

    /// Where the manifest of entry `key` lives:
    /// `entries/<key[0:2]>/<key>/manifest.json`. Keys are the key engine's
    /// 64 lowercase hex digits; every fallible operation rejects any other
    /// spelling before it becomes a path.
    #[must_use]
    pub fn entry_path(&self, key: &str) -> PathBuf {
        self.entry_dir(key).join(MANIFEST)
    }

    /// The manifest stored for `key`, `None` on a miss. An unreadable
    /// manifest is [`BcacheError::Corrupt`]: callers treat it as a miss,
    /// and the next ingest of the key replaces it.
    pub fn lookup(&self, key: &str) -> Result<Option<EntryManifest>> {
        check_key(key)?;
        let path = self.entry_path(key);
        let Some(bytes) = read_if_present(&path)? else {
            return Ok(None);
        };
        parse_manifest(&path, key, &bytes).map(Some)
    }

    /// Stamps entry `key` as hit now — the clock gc evicts by. The stamp
    /// is the mtime of a sidecar file beside the manifest
    /// (`entries/<shard>/<key>/hit`): `record_hit` never reads, parses,
    /// or rewrites the manifest itself, so a hit racing an ingest of the
    /// same key cannot revert the manifest to an older build — the
    /// sidecar only ever makes the entry look more recently used (gc
    /// evicts by the newer of the manifest's `last_hit_unix` and the
    /// sidecar's mtime). Metadata-only: an existence check and one tiny
    /// sidecar write, no manifest I/O. The sidecar's content is
    /// diagnostic; nothing parses it, so a torn write is harmless. An
    /// entry gc removed meanwhile stays removed: the call fails
    /// [`BcacheError::NotFound`] instead of resurrecting anything.
    pub fn record_hit(&self, key: &str) -> Result<()> {
        check_key(key)?;
        let dir = self.entry_dir(key);
        if !dir.join(MANIFEST).is_file() {
            return Err(BcacheError::NotFound(format!("entry {key}")));
        }
        let path = dir.join(HIT);
        let mut file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&path)
            .map_err(io_at(&path))?;
        file.write_all(unix_secs(SystemTime::now()).to_string().as_bytes())
            .map_err(io_at(&path))
    }

    /// Publishes `draft` as the entry for `draft.key`, storing every source
    /// as a blob first.
    ///
    /// Each `(path, blob)` source is cloned into the store and the bytes
    /// that landed must hash to `blob`; otherwise the ingest fails with
    /// [`BcacheError::Corrupt`] naming the path (the build tree changed
    /// under the ingest) and no manifest is published. A blob the store
    /// already holds is not rewritten, but its source is still verified
    /// and its mtime re-touched: reuse must look recent to a concurrent
    /// gc, or the gc could reclaim the blob between this validation and
    /// the manifest publish. The same touch covers blobs referenced
    /// without a source (the warm `flutter test` reuse path). The draft
    /// is rejected up front ([`BcacheError::Invalid`]) when its key is
    /// malformed, a file path is not a plain relative path, or a
    /// referenced blob is neither sourced nor stored — the checks run
    /// over the files of every [`crate::DirRecord`] of the draft. A
    /// previous manifest for the key is replaced whole. The draft is
    /// persisted as given, timestamps included.
    pub fn ingest(&self, draft: EntryManifest, sources: &[(PathBuf, BlobId)]) -> Result<()> {
        check_key(&draft.key)?;
        let manifest = to_json(&draft)?;
        let sourced: FxHashSet<BlobId> = sources.iter().map(|(_, id)| *id).collect();
        // Blobs referenced without a source: already stored, reused
        // as-is — and re-touched below, before the manifest lands.
        let mut reused: Vec<BlobId> = Vec::new();
        for dir in &draft.dirs {
            for file in &dir.files {
                if !is_plain_relative(&file.path) {
                    return Err(BcacheError::Invalid(format!(
                        "manifest path {:?}: expected a plain relative path of plain names",
                        file.path
                    )));
                }
                if !sourced.contains(&file.blob) {
                    if self.blob_path(&file.blob).is_file() {
                        reused.push(file.blob);
                    } else {
                        return Err(BcacheError::Invalid(format!(
                            "{}: blob {} is neither sourced nor stored",
                            file.path.display(),
                            blob_id_hex(&file.blob)
                        )));
                    }
                }
            }
        }
        let added = AtomicU64::new(0);
        sources.par_iter().try_for_each_init(
            || vec![0u8; CHUNK],
            |buf, (src, id)| {
                let delta = self.put_blob(src, id, buf)?;
                added.fetch_add(delta, Ordering::Relaxed);
                Ok::<(), BcacheError>(())
            },
        )?;
        // Book the new bytes before the manifest exists: the blobs are
        // on disk whatever happens to the publish.
        self.bump_used(added.load(Ordering::Relaxed))?;
        for id in &reused {
            let path = self.blob_path(id);
            touch(&path).map_err(io_at(&path))?;
        }
        self.publish(&self.entry_path(&draft.key), &manifest)
    }

    /// Brings the blobs under `budget_bytes` if it can, then deletes every
    /// blob no remaining manifest references.
    ///
    /// Entries are evicted least recently hit first (by the effective hit
    /// time: the newer of the manifest's `last_hit_unix` and the hit
    /// sidecar's mtime) until the blobs that remain fit the budget. An
    /// entry hit within `grace_secs` of `now_unix` is never evicted, even
    /// if the store stays over budget. A manifest that cannot be read or
    /// decoded is evicted and counted in
    /// [`GcReport::corrupt_manifests`], never fatal — no single bad file
    /// may disable the budget. Scratch files and unreferenced blobs
    /// younger than one hour may belong to an ingest in flight and are
    /// spared. Files that vanish mid-pass (a concurrent gc) are skipped,
    /// and the running usage counter is rewritten to the truth this pass
    /// observed (see [`Self::used_bytes`]).
    pub fn gc(&self, budget_bytes: u64, grace_secs: u64, now_unix: u64) -> Result<GcReport> {
        let entries = self.scan_entries()?;
        let blobs = self.scan_blobs()?;
        let mut report = GcReport {
            entries_before: entries.len() as u64,
            ..GcReport::default()
        };
        let in_flight = |mtime_secs: u64| now_unix.saturating_sub(mtime_secs) < IN_FLIGHT_SECS;

        // References per blob over every readable manifest. A blob one
        // manifest lists twice counts twice; eviction decrements alike.
        let mut refs: FxHashMap<BlobId, u64> = FxHashMap::default();
        let mut live = Vec::with_capacity(entries.len());
        let mut evict = Vec::new();
        for entry in entries {
            match entry.facts {
                Ok(facts) => {
                    for blob in &facts.blobs {
                        *refs.entry(*blob).or_default() += 1;
                    }
                    live.push((facts.last_hit_unix, entry.dir, facts.blobs));
                }
                Err(_) => {
                    report.corrupt_manifests += 1;
                    evict.push(entry.dir);
                }
            }
        }

        // Bytes that survive the pass as things stand, and the sizes of the
        // blobs the pass may delete once nothing references them.
        let mut kept: u64 = blobs
            .iter()
            .filter(|blob| is_referenced(&refs, &blob.id) || in_flight(blob.secs()))
            .map(|blob| blob.len)
            .sum();
        let deletable: FxHashMap<BlobId, u64> = blobs
            .iter()
            .filter(|blob| !in_flight(blob.secs()))
            .map(|blob| (blob.id, blob.len))
            .collect();

        // Least recently hit first; the directory breaks ties, keeping the
        // order deterministic. Hit times only grow along this order, so the
        // first grace-protected entry ends the eviction run.
        live.sort_unstable_by(|a, b| (a.0, &a.1).cmp(&(b.0, &b.1)));
        let mut evicted = 0;
        for (last_hit, _, ids) in &live {
            if kept <= budget_bytes || now_unix.saturating_sub(*last_hit) < grace_secs {
                break;
            }
            for id in ids {
                if let Some(count) = refs.get_mut(id) {
                    *count -= 1;
                    if *count == 0
                        && let Some(len) = deletable.get(id)
                    {
                        kept = kept.saturating_sub(*len);
                    }
                }
            }
            evicted += 1;
        }
        evict.extend(live.into_iter().take(evicted).map(|(_, dir, _)| dir));

        // Manifests go before blobs: at every step of the pass, no
        // surviving manifest names a deleted blob.
        for dir in &evict {
            if removed(dir, fs::remove_dir_all(dir))? {
                report.entries_evicted += 1;
            }
        }
        let scanned_bytes: u64 = blobs.iter().map(|blob| blob.len).sum();
        let mut blob_bytes_freed = 0u64;
        for blob in &blobs {
            if is_referenced(&refs, &blob.id) || in_flight(blob.secs()) {
                continue;
            }
            let path = self.blob_path(&blob.id);
            // Second reuse guard, for the ones the first (the ingest-side
            // touch) cannot cover: a touch that landed after this pass's
            // scan but before its unlink. An unchanged nanosecond mtime
            // means the scan's verdict still describes the file; a moved
            // one means an ingest just reused the blob, and it stays.
            match fs::metadata(&path) {
                Ok(meta) if mtime_ns(&meta) == blob.mtime_ns => {}
                Ok(_) => continue,
                Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
                Err(e) => return Err(io_at(&path)(e)),
            }
            if removed(&path, fs::remove_file(&path))? {
                report.blobs_removed += 1;
                report.bytes_freed += blob.len;
                blob_bytes_freed += blob.len;
            }
        }
        for (path, meta) in list(&self.root.join(TMP))? {
            if meta.is_file()
                && !in_flight(mtime_unix(&path, &meta)?)
                && removed(&path, fs::remove_file(&path))?
            {
                report.bytes_freed += meta.len();
            }
        }
        // This pass just walked every blob: rewrite the running usage
        // counter to what it observed, healing any drift from lost
        // read-modify-write races between concurrent writers.
        self.set_used(scanned_bytes - blob_bytes_freed)?;
        Ok(report)
    }

    /// The bytes currently held by blobs under this root, from the
    /// running counter — one small file read, no walk. This is the
    /// O(1) "is the store over budget?" probe for the post-ingest
    /// opportunistic gc; the number is advisory (concurrent writers can
    /// lose an update) and every gc pass rewrites it to the truth it
    /// observed. A missing counter reads as 0; an unparsable one (a
    /// foreign file; atomic rewrites cannot tear) also reads as 0 and
    /// heals at the next gc — the probe never fails the budget it
    /// serves.
    pub fn used_bytes(&self) -> Result<u64> {
        let path = self.used_path();
        match fs::read_to_string(&path) {
            Ok(text) => Ok(text.trim().parse().unwrap_or(0)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(0),
            Err(e) => Err(io_at(&path)(e)),
        }
    }

    /// Adds `delta` to the running usage counter, atomically. A
    /// read-modify-write racing another writer's can lose an update;
    /// that drift is the accepted price of lock-free booking and is
    /// healed by the next gc pass (see [`Self::gc`]).
    fn bump_used(&self, delta: u64) -> Result<()> {
        if delta == 0 {
            return Ok(());
        }
        let total = self.used_bytes()?.saturating_add(delta);
        self.write_used(total)
    }

    /// Rewrites the counter to `total` (the gc heal path).
    fn set_used(&self, total: u64) -> Result<()> {
        self.write_used(total)
    }

    fn write_used(&self, total: u64) -> Result<()> {
        let path = self.used_path();
        self.publish(&path, format!("{total}\n").as_bytes())
    }

    fn used_path(&self) -> PathBuf {
        self.root.join("meta").join("used")
    }

    /// Entry and blob counts, total blob bytes, the oldest effective hit,
    /// and how many manifests failed to read or decode. A corrupt
    /// manifest is skipped and counted — never fatal — so one bad file
    /// cannot disable the disk budget that `gc` (sharing this walker)
    /// enforces; `gc` purges the counted entries. This is the explicit,
    /// full-walk face: callers polling the budget should use the O(1)
    /// [`Self::used_bytes`] instead.
    pub fn stats(&self) -> Result<StoreStats> {
        let mut stats = StoreStats::default();
        let mut oldest = u64::MAX;
        for entry in self.scan_entries()? {
            match entry.facts {
                Ok(facts) => {
                    oldest = oldest.min(facts.last_hit_unix);
                    stats.entries += 1;
                }
                Err(_) => stats.corrupt_manifests += 1,
            }
        }
        if stats.entries > 0 {
            stats.oldest_hit_unix = oldest;
        }
        for blob in self.scan_blobs()? {
            stats.blobs += 1;
            stats.blob_bytes += blob.len;
        }
        Ok(stats)
    }

    /// Stores `src` as blob `id`. The fresh path clones `src` into the
    /// store and verifies that the bytes that landed hash to `id`; a
    /// stored blob is left alone once `src` re-verifies — and its mtime
    /// re-touched, marking the reuse for a concurrent gc — unless its
    /// size disagrees with the source (a copy truncated by a crash or
    /// damaged outside the store), which is then replaced. Returns the
    /// bytes added to the store (0 on reuse) for the running usage
    /// counter; the rare damaged-blob replacement may overcount by the
    /// old size, which the next gc pass heals.
    fn put_blob(&self, src: &Path, id: &BlobId, buf: &mut [u8]) -> Result<u64> {
        let dest = self.blob_path(id);
        if let Ok(stored) = fs::metadata(&dest) {
            let (actual, len) = hash_copy(src, buf)?;
            verify(src, id, &actual)?;
            if stored.len() == len {
                touch(&dest).map_err(io_at(&dest))?;
                return Ok(0);
            }
        }
        self.put_blob_fresh(src, id, &dest)
    }

    /// The fresh-blob path of [`Self::put_blob`]: clone `src` onto a
    /// not-yet-existing scratch name and hash the bytes that landed
    /// (audit A16: on a copy-on-write filesystem the clone plus one
    /// read of it halves the I/O of the read-and-copy loop; the
    /// streaming copy remains the fallback). Cloning onto a name
    /// nothing created sidesteps `clone_file`'s EEXIST dance, and the
    /// digest vouches for exactly what gets published — never for a
    /// source that changed after the snapshot. The adopted descriptor
    /// is the clone itself, so the publish sync still orders its data
    /// ahead of the rename.
    fn put_blob_fresh(&self, src: &Path, id: &BlobId, dest: &Path) -> Result<u64> {
        let path = self.scratch_path()?;
        let (_, len, actual) = match clone_file_hashed(src, &path) {
            Ok(cloned) => cloned,
            Err(e) => {
                // A refused clone leaves nothing behind; a failed
                // fallback copy may leave a partial file. Neither may
                // survive the ingest.
                let _ = fs::remove_file(&path);
                return Err(io_at(&path)(e));
            }
        };
        let scratch = Scratch::adopt(&path)?;
        verify(src, id, &actual)?;
        create_parent(dest)?;
        scratch.publish(dest)?;
        Ok(len)
    }

    /// Write–sync–rename of `bytes` onto `dest`, retrying exactly once
    /// when a concurrent gc removed the destination's directory between
    /// the mkdir and the rename (an evicted entry whose key an ingest is
    /// re-publishing right now). The retry window is two syscalls wide,
    /// and a gc pass evicts each directory at most once.
    fn publish(&self, dest: &Path, bytes: &[u8]) -> Result<()> {
        let mut retried = false;
        loop {
            create_parent(dest)?;
            let mut scratch = self.scratch()?;
            scratch.write(bytes)?;
            match scratch.publish(dest) {
                Ok(()) => return Ok(()),
                Err(err) if !retried && is_not_found(&err) => retried = true,
                Err(err) => return Err(err),
            }
        }
    }

    /// A unique scratch path under `tmp/` — nothing created there yet.
    /// The pid plus an in-process sequence keeps live writers apart; the
    /// timestamp keeps a recycled pid from colliding with the leftover
    /// of a crashed process.
    fn scratch_path(&self) -> Result<PathBuf> {
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since| since.as_nanos());
        let name = format!(
            "{}-{}-{nanos}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        );
        Ok(self.root.join(TMP).join(name))
    }

    /// A fresh, empty scratch file at a unique path.
    fn scratch(&self) -> Result<Scratch> {
        let path = self.scratch_path()?;
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(io_at(&path))?;
        Ok(Scratch {
            path,
            file,
            published: false,
        })
    }

    /// Every entry that has a manifest, judged by the same parse as
    /// [`Self::lookup`]: an entry gc keeps is one lookup can serve. A
    /// manifest that cannot be read or decoded — hand-corrupted bytes, a
    /// schema drift from a newer binary — is carried as `Err` facts for
    /// the caller to skip and count, never a scan failure: no per-file
    /// condition may disable stats or the budget pass. An entry
    /// directory without a manifest belongs to an ingest in flight (or
    /// is the empty leftover of a crashed one) and is not an entry.
    fn scan_entries(&self) -> Result<Vec<ScannedEntry>> {
        let mut entries = Vec::new();
        for (shard, meta) in list(&self.root.join(ENTRIES))? {
            if !meta.is_dir() {
                continue;
            }
            for (dir, meta) in list(&shard)? {
                if !meta.is_dir() {
                    continue;
                }
                let path = dir.join(MANIFEST);
                let bytes = match fs::read(&path) {
                    Ok(bytes) => bytes,
                    Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
                    Err(e) => {
                        entries.push(ScannedEntry {
                            dir,
                            facts: Err(io_at(&path)(e)),
                        });
                        continue;
                    }
                };
                let key = dir
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let facts = parse_manifest(&path, &key, &bytes).map(|manifest| EntryFacts {
                    last_hit_unix: hit_unix(&dir, manifest.last_hit_unix),
                    blobs: manifest
                        .dirs
                        .into_iter()
                        .flat_map(|dir| dir.files)
                        .map(|file| file.blob)
                        .collect(),
                });
                entries.push(ScannedEntry { dir, facts });
            }
        }
        Ok(entries)
    }

    /// Every blob on disk. A file counts as a blob only where
    /// [`Self::blob_path`] puts its name — checked as "the name parses
    /// as a digest and sits under the shard its first two digits
    /// prescribe", the allocation-free spelling of `blob_path(id) ==
    /// path` (this runs once per blob of a 60k-blob store); anything
    /// else under `blobs/` is foreign and left alone.
    fn scan_blobs(&self) -> Result<Vec<ScannedBlob>> {
        let mut blobs = Vec::new();
        for (shard, meta) in list(&self.root.join(BLOBS))? {
            if !meta.is_dir() {
                continue;
            }
            let Some(shard_name) = shard.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            for (path, meta) in list(&shard)? {
                let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
                    continue;
                };
                if meta.is_file()
                    && name.get(..2) == Some(shard_name)
                    && let Ok(id) = name.parse::<BlobId>()
                {
                    blobs.push(ScannedBlob {
                        id,
                        len: meta.len(),
                        mtime_ns: mtime_ns(&meta),
                    });
                }
            }
        }
        Ok(blobs)
    }
}

/// What gc and stats keep of a manifest: its effective hit time (the
/// newer of the manifest's `last_hit_unix` and the hit sidecar's mtime)
/// and the blobs it references (one per file, repeats included).
struct EntryFacts {
    last_hit_unix: u64,
    blobs: Vec<BlobId>,
}

/// One entry found on disk: its directory, and its facts or why lookup
/// would reject its manifest.
struct ScannedEntry {
    dir: PathBuf,
    facts: Result<EntryFacts>,
}

/// One blob found on disk. The mtime is kept at nanosecond resolution:
/// the in-flight window compares whole seconds (derived), while the
/// pre-unlink re-verify must tell a reuse touch apart from the scan even
/// within the same second.
struct ScannedBlob {
    id: BlobId,
    len: u64,
    mtime_ns: u64,
}

impl ScannedBlob {
    fn secs(&self) -> u64 {
        self.mtime_ns / NANOS_PER_SEC
    }
}

/// The entry's effective last-hit time: the manifest's own field, or the
/// hit sidecar's mtime when a hit stamped it later. A missing (or
/// unreadable) sidecar leaves the manifest's value — the sidecar can
/// only ever make an entry look more recently used, never colder.
fn hit_unix(dir: &Path, manifest_last_hit: u64) -> u64 {
    let sidecar = fs::metadata(dir.join(HIT))
        .and_then(|meta| meta.modified())
        .map_or(0, unix_secs);
    manifest_last_hit.max(sidecar)
}

/// A file under `tmp/` on its way into the store. Removed on drop unless
/// published, so a failed write leaves no residue.
struct Scratch {
    path: PathBuf,
    file: File,
    published: bool,
}

impl Scratch {
    /// Adopts the file already sitting at `path` — the clone the fresh
    /// blob path landed there — under the same publish-or-remove
    /// discipline as a created scratch file. Opened read-write so the
    /// publish sync (a barrier, or `fdatasync`) is valid on the adopted
    /// descriptor everywhere.
    fn adopt(path: &Path) -> Result<Self> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
            .map_err(io_at(path))?;
        Ok(Self {
            path: path.to_path_buf(),
            file,
            published: false,
        })
    }

    fn write(&mut self, bytes: &[u8]) -> Result<()> {
        self.file.write_all(bytes).map_err(io_at(&self.path))
    }

    /// Syncs the content, then renames it onto `dest`: readers of `dest`
    /// see the previous file or this one, never a partial write, and a
    /// crash cannot leave `dest` naming torn content.
    fn publish(mut self, dest: &Path) -> Result<()> {
        sync_before_rename(&self.file).map_err(io_at(&self.path))?;
        fs::rename(&self.path, dest).map_err(io_at(dest))?;
        self.published = true;
        Ok(())
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        if !self.published {
            // A destructor has no one to report to; a scratch file that
            // survives this (or a crash) is reclaimed by gc's sweep.
            let _ = fs::remove_file(&self.path);
        }
    }
}

/// Orders the file's data ahead of the rename that publishes it: a crash
/// may lose a fresh publish, never expose a renamed file with torn content.
///
/// Apple platforms issue `F_BARRIERFSYNC`, a write barrier, instead of the
/// drive-cache flush `File::sync_data` maps to there (`F_FULLFSYNC`,
/// milliseconds per call on Apple NVMe — it would dominate an ingest of
/// many small files and the `record_hit` of every cache hit). A volume that
/// rejects the barrier gets the full flush. Elsewhere: `fdatasync`.
fn sync_before_rename(file: &File) -> io::Result<()> {
    #[cfg(target_vendor = "apple")]
    {
        use std::os::fd::AsRawFd;
        loop {
            // SAFETY: `fcntl` on a descriptor `file` keeps open for the
            // whole call; F_BARRIERFSYNC takes no argument and touches no
            // memory.
            if unsafe { libc::fcntl(file.as_raw_fd(), libc::F_BARRIERFSYNC) } != -1 {
                return Ok(());
            }
            if io::Error::last_os_error().kind() != io::ErrorKind::Interrupted {
                break;
            }
        }
    }
    file.sync_data()
}

/// One streaming pass over `src`, hashing every byte: the reuse check
/// of an already-stored blob, and the final fallback copy's verifier
/// when a filesystem refuses to clone (which hashes the copy it wrote).
/// Returns the digest and the byte count.
fn hash_copy(src: &Path, buf: &mut [u8]) -> Result<(BlobId, u64)> {
    let mut file = File::open(src).map_err(io_at(src))?;
    let mut hasher = blake3::Hasher::new();
    let mut len = 0u64;
    loop {
        let n = match file.read(buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(io_at(src)(e)),
        };
        hasher.update(&buf[..n]);
        len += n as u64;
    }
    Ok((BlobId(*hasher.finalize().as_bytes()), len))
}

/// Parses the manifest stored at `path` for `key`. Unparsable bytes and a
/// manifest naming another key are both [`BcacheError::Corrupt`].
fn parse_manifest(path: &Path, key: &str, bytes: &[u8]) -> Result<EntryManifest> {
    let manifest: EntryManifest = serde_json::from_slice(bytes).map_err(corrupt_at(path))?;
    if manifest.key != key {
        return Err(BcacheError::Corrupt(format!(
            "{}: manifest names key {:?}",
            path.display(),
            manifest.key
        )));
    }
    Ok(manifest)
}

fn verify(src: &Path, expected: &BlobId, actual: &BlobId) -> Result<()> {
    if actual == expected {
        return Ok(());
    }
    Err(BcacheError::Corrupt(format!(
        "{}: content hashes to {}, the draft names blob {}",
        src.display(),
        blob_id_hex(actual),
        blob_id_hex(expected)
    )))
}

fn check_key(key: &str) -> Result<()> {
    if is_hex_digest(key) {
        return Ok(());
    }
    Err(BcacheError::Invalid(format!(
        "entry key {key:?}: expected 64 lowercase hex digits"
    )))
}

/// Whether `path` is non-empty, relative, and made of plain names only —
/// no root, `.`, or `..` — so it can never resolve outside the build
/// directory it is joined to.
fn is_plain_relative(path: &Path) -> bool {
    path.components().next().is_some()
        && path
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
}

fn is_referenced(refs: &FxHashMap<BlobId, u64>, id: &BlobId) -> bool {
    refs.get(id).is_some_and(|&count| count > 0)
}

fn to_json(value: &impl Serialize) -> Result<Vec<u8>> {
    serde_json::to_vec(value)
        .map_err(|e| BcacheError::Invalid(format!("manifest serialization: {e}")))
}

/// The bytes at `path`; `None` if there is no such file.
fn read_if_present(path: &Path) -> Result<Option<Vec<u8>>> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(io_at(path)(e)),
    }
}

/// The items of `dir` with their metadata (symlinks not followed), in no
/// particular order. A missing `dir` lists as empty, and items that vanish
/// mid-walk (a concurrent gc) are skipped.
fn list(dir: &Path) -> Result<Vec<(PathBuf, fs::Metadata)>> {
    let items = match fs::read_dir(dir) {
        Ok(items) => items,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(io_at(dir)(e)),
    };
    let mut out = Vec::new();
    for item in items {
        let item = item.map_err(io_at(dir))?;
        let path = item.path();
        match item.metadata() {
            Ok(meta) => out.push((path, meta)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(io_at(&path)(e)),
        }
    }
    Ok(out)
}

fn create_parent(path: &Path) -> Result<()> {
    match path.parent() {
        Some(dir) => fs::create_dir_all(dir).map_err(io_at(dir)),
        None => Ok(()),
    }
}

/// Whether a removal removed something: `Ok(false)` when the target was
/// already gone (a concurrent gc got there first).
fn removed(path: &Path, result: io::Result<()>) -> Result<bool> {
    match result {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(io_at(path)(e)),
    }
}

/// Whether the error is the filesystem's "no such file" — the publish
/// retry's signal that a concurrent gc removed the destination's
/// directory between the mkdir and the rename.
fn is_not_found(err: &BcacheError) -> bool {
    matches!(err, BcacheError::Io(e) if e.kind() == io::ErrorKind::NotFound)
}

fn mtime_unix(path: &Path, meta: &fs::Metadata) -> Result<u64> {
    Ok(unix_secs(meta.modified().map_err(io_at(path))?))
}

/// Seconds since the Unix epoch; 0 for earlier times.
fn unix_secs(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}

/// Attaches `path` to an I/O error, keeping its kind for callers that
/// branch on it.
fn io_at(path: &Path) -> impl FnOnce(io::Error) -> BcacheError {
    move |e| BcacheError::Io(io::Error::new(e.kind(), format!("{}: {e}", path.display())))
}

fn corrupt_at(path: &Path) -> impl FnOnce(serde_json::Error) -> BcacheError {
    move |e| BcacheError::Corrupt(format!("{}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::Arc;
    use std::thread;
    use std::time::{Duration, Instant};

    use super::*;
    use crate::types::{DirRecord, ManifestFile, NameRule, RewriteKind, TargetSpec, ToolchainFingerprint};

    struct Fixture {
        _dir: tempfile::TempDir,
        root: PathBuf,
        build: PathBuf,
        store: CasStore,
    }

    fn fixture() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("cas");
        let build = dir.path().join("build");
        fs::create_dir(&build).unwrap();
        let store = CasStore::open(&root).unwrap();
        Fixture {
            _dir: dir,
            root,
            build,
            store,
        }
    }

    /// A well-formed entry key, distinct per `n`.
    fn key(n: u8) -> String {
        blob_id_hex(&BlobId([n; 32]))
    }

    /// Writes `bytes` as `name` under `dir`; returns the ingest source.
    fn source(dir: &Path, name: &str, bytes: &[u8]) -> (PathBuf, BlobId) {
        let path = dir.join(name);
        fs::write(&path, bytes).unwrap();
        (path, BlobId(*blake3::hash(bytes).as_bytes()))
    }

    /// One DirRecord over `sources`, named `name`, replayable per `rule`.
    fn record(
        name: &str,
        rule: NameRule,
        sources: &[(PathBuf, BlobId)],
    ) -> DirRecord {
        DirRecord {
            name_observed: name.to_owned(),
            name_rule: rule,
            name_defines: match rule {
                NameRule::RecomputeFromTemplate => BTreeMap::from([(
                    "BuildMode".to_owned(),
                    "debug".to_owned(),
                )]),
                _ => BTreeMap::new(),
            },
            name_output_dir: match rule {
                NameRule::RecomputeFromTemplate => "/work/app/build/out".to_owned(),
                _ => String::new(),
            },
            files: sources
                .iter()
                .map(|(path, blob)| ManifestFile {
                    path: PathBuf::from(path.file_name().unwrap()),
                    blob: *blob,
                    executable: false,
                    rewrite: RewriteKind::None,
                    foreign_paths: false,
                    stamp: None,
                })
                .collect(),
        }
    }

    fn draft(key: &str, sources: &[(PathBuf, BlobId)], last_hit_unix: u64) -> EntryManifest {
        EntryManifest {
            key: key.to_owned(),
            project_root_at_ingest: "/work/app".to_owned(),
            worktree_root_at_ingest: "/work".to_owned(),
            pub_cache_root: "/work/.pub-cache".to_owned(),
            flutter_root_at_ingest: "/opt/flutter".to_owned(),
            dirs: vec![record(
                "0f1e2d3c4b5a69788796a5b4c3d2e1f0",
                NameRule::RecomputeFromTemplate,
                sources,
            )],
            defines: BTreeMap::from([("BuildMode".to_owned(), "debug".to_owned())]),
            target: TargetSpec {
                command: "build".to_owned(),
                platform: "ios".to_owned(),
                mode: "debug".to_owned(),
                entry: None,
                flavor: None,
                extra_key_args: Vec::new(),
            },
            toolchain: ToolchainFingerprint {
                engine_revision: "engine".to_owned(),
                flutter_version: "3.47.1".to_owned(),
                platform_strong_dill: BlobId([1; 32]),
                frontend_server_snapshot: BlobId([2; 32]),
            },
            created_at_unix: last_hit_unix,
            last_hit_unix,
        }
    }

    /// Ingests an entry `key(n)` made of exactly `sources`.
    fn put(f: &Fixture, n: u8, sources: &[(PathBuf, BlobId)], last_hit_unix: u64) {
        f.store
            .ingest(draft(&key(n), sources, last_hit_unix), sources)
            .unwrap();
    }

    /// Removes entry `key(n)`'s directory, leaving its blobs unreferenced.
    fn drop_entry(f: &Fixture, n: u8) {
        fs::remove_dir_all(f.store.entry_path(&key(n)).parent().unwrap()).unwrap();
    }

    /// Every file under `blobs/`, sorted.
    fn blob_files(root: &Path) -> Vec<PathBuf> {
        let mut files = Vec::new();
        for shard in fs::read_dir(root.join(BLOBS)).unwrap() {
            for file in fs::read_dir(shard.unwrap().path()).unwrap() {
                files.push(file.unwrap().path());
            }
        }
        files.sort();
        files
    }

    fn scratch_is_empty(root: &Path) -> bool {
        fs::read_dir(root.join(TMP)).unwrap().next().is_none()
    }

    /// A clock far enough ahead that every file a test wrote is past the
    /// in-flight window.
    fn later() -> u64 {
        unix_secs(SystemTime::now()) + 2 * IN_FLIGHT_SECS
    }

    #[test]
    fn store_roundtrip_ingest_lookup() {
        let f = fixture();
        let sources = [
            source(&f.build, "app.dill", b"kernel bytes"),
            source(&f.build, "kernel_snapshot_program.d", b"app.dill: lib/main.dart"),
        ];
        let entry = draft(&key(1), &sources, 100);
        f.store.ingest(entry.clone(), &sources).unwrap();

        assert_eq!(f.store.lookup(&key(1)).unwrap(), Some(entry));
        assert_eq!(f.store.lookup(&key(2)).unwrap(), None);
        for (path, blob) in &sources {
            let hex = blob_id_hex(blob);
            let stored = f.store.blob_path(blob);
            assert_eq!(stored, f.root.join(BLOBS).join(&hex[..2]).join(&hex));
            assert_eq!(fs::read(&stored).unwrap(), fs::read(path).unwrap());
        }
        assert_eq!(blob_files(&f.root).len(), 2);
    }

    #[test]
    fn store_multi_dir_roundtrip_and_gc_span_all_dirs() {
        let f = fixture();
        let kernel = [
            source(&f.build, "app.dill", b"kernel bytes"),
            source(&f.build, "kernel_snapshot.d", b"app.dill: lib/main.dart"),
        ];
        let assets = [
            source(&f.build, "AssetManifest.bin", b"manifest bytes"),
            source(&f.build, "app.dill", b"kernel bytes"),
        ];
        let entry = EntryManifest {
            dirs: vec![
                record(
                    "0000000000000000000000000000000a",
                    NameRule::RecomputeFromTemplate,
                    &kernel,
                ),
                record(
                    "0000000000000000000000000000000b",
                    NameRule::VerbatimRelative,
                    &assets,
                ),
            ],
            ..draft(&key(1), &kernel, 100)
        };
        f.store
            .ingest(entry.clone(), &kernel.iter().chain(&assets).cloned().collect::<Vec<_>>())
            .unwrap();

        assert_eq!(f.store.lookup(&key(1)).unwrap(), Some(entry));
        // The blob shared by both dirs is stored once; refcounting spans
        // both dirs' files.
        assert_eq!(blob_files(&f.root).len(), 3);

        // Evicting the entry frees every blob both dirs referenced.
        let report = f.store.gc(0, 0, later()).unwrap();
        assert_eq!(report.entries_evicted, 1);
        assert_eq!(report.blobs_removed, 3);
        assert_eq!(blob_files(&f.root).len(), 0);
        assert_eq!(f.store.lookup(&key(1)).unwrap(), None);
    }

    #[test]
    fn store_ingest_is_idempotent() {
        let f = fixture();
        // Two identical stamps share one blob even within one ingest.
        let sources = [
            source(&f.build, "app.dill", b"kernel bytes"),
            source(&f.build, "a.stamp", b"{}"),
            source(&f.build, "b.stamp", b"{}"),
        ];
        put(&f, 1, &sources, 100);
        let blobs = blob_files(&f.root);
        assert_eq!(blobs.len(), 2);

        let second = EntryManifest {
            dirs: vec![record(
                "ffeeddccbbaa99887766554433221100",
                NameRule::RecomputeFromTemplate,
                &sources,
            )],
            ..draft(&key(1), &sources, 200)
        };
        f.store.ingest(second.clone(), &sources).unwrap();
        assert_eq!(blob_files(&f.root), blobs);
        assert_eq!(f.store.lookup(&key(1)).unwrap(), Some(second));
        assert!(scratch_is_empty(&f.root));
    }

    #[test]
    fn store_ingest_detects_mutated_source() {
        let f = fixture();
        let sources = [
            source(&f.build, "app.dill", b"kernel bytes"),
            source(&f.build, "app.dill.d", b"deps"),
        ];
        let mutated = sources[0].0.display().to_string();

        // Changed before its blob is stored: the hash over the bytes
        // that landed (the clone) catches it.
        let entry = draft(&key(1), &sources, 100);
        fs::write(&sources[0].0, b"rebuilt under the ingest").unwrap();
        match f.store.ingest(entry, &sources) {
            Err(BcacheError::Corrupt(msg)) => assert!(msg.contains(&mutated), "{msg}"),
            other => panic!("expected Corrupt, got {other:?}"),
        }
        assert_eq!(f.store.lookup(&key(1)).unwrap(), None);
        assert!(!f.store.blob_path(&sources[0].1).exists());

        // Changed after its blob is stored: the stored blob does not vouch
        // for the source.
        fs::write(&sources[0].0, b"kernel bytes").unwrap();
        put(&f, 1, &sources, 100);
        fs::write(&sources[0].0, b"rebuilt again").unwrap();
        match f.store.ingest(draft(&key(2), &sources, 100), &sources) {
            Err(BcacheError::Corrupt(msg)) => assert!(msg.contains(&mutated), "{msg}"),
            other => panic!("expected Corrupt, got {other:?}"),
        }
        assert_eq!(f.store.lookup(&key(2)).unwrap(), None);
    }

    /// The fresh-blob path lands the source's exact bytes under a new
    /// inode (audit A16: clone-then-hash). A new inode holds on both the
    /// clone and the fallback copy, so this also pins that the store
    /// never hardlinks the build tree — a shared inode would let
    /// flutter's in-place rewrites corrupt a stored blob.
    #[test]
    fn store_ingest_fresh_blob_is_the_sources_bytes_under_a_new_inode() {
        let f = fixture();
        let payload: Vec<u8> = (0..100_000u32).map(|i| (i % 251) as u8).collect();
        let sources = [
            source(&f.build, "app.dill", &payload),
            source(&f.build, "app.dill.d", b"deps"),
        ];
        put(&f, 1, &sources, 100);

        let stored = f.store.blob_path(&sources[0].1);
        assert_eq!(fs::read(&stored).unwrap(), payload);
        assert!(scratch_is_empty(&f.root));
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let blob_ino = fs::metadata(&stored).unwrap().ino();
            let src_ino = fs::metadata(&sources[0].0).unwrap().ino();
            assert_ne!(blob_ino, src_ino);
        }
    }

    #[test]
    fn store_ingest_is_atomic() {
        let f = fixture();
        put(&f, 1, &[source(&f.build, "app.dill", b"kernel bytes")], 100);
        assert!(scratch_is_empty(&f.root));

        // A multi-chunk source that changes under the ingest: the partial
        // scratch copy is discarded and no manifest appears.
        let sources = [
            source(&f.build, "big.bin", &vec![7u8; CHUNK + 3]),
            source(&f.build, "small.bin", b"small"),
        ];
        let entry = draft(&key(2), &sources, 100);
        fs::write(&sources[0].0, vec![8u8; CHUNK + 3]).unwrap();
        assert!(matches!(
            f.store.ingest(entry, &sources),
            Err(BcacheError::Corrupt(_))
        ));
        assert!(!f.store.entry_path(&key(2)).exists());
        assert_eq!(f.store.lookup(&key(2)).unwrap(), None);
        assert!(scratch_is_empty(&f.root));
        assert!(f.store.lookup(&key(1)).unwrap().is_some());
    }

    #[test]
    fn store_gc_evicts_lru_and_spares_grace() {
        let f = fixture();
        let now = later();
        let [a, b, c, d, orphan] = [
            ("a", 1_000),
            ("b", 2_000),
            ("c", 4_000),
            ("d", 8_000),
            ("o", 16_000),
        ]
        .map(|(name, len)| source(&f.build, name, &vec![name.as_bytes()[0]; len]));
        put(&f, 1, &[a.clone(), b.clone()], 100);
        put(&f, 2, &[b.clone(), c.clone()], 200);
        put(&f, 3, &[d.clone()], now - 10);
        put(&f, 4, &[orphan.clone()], 50);
        drop_entry(&f, 4);

        // 15_000 bytes stay referenced once the orphan goes; evicting the
        // oldest entry frees `a`, which brings the store under budget. `b`
        // survives: entry 2 still references it.
        let report = f.store.gc(14_500, 3_600, now).unwrap();
        assert_eq!(
            report,
            GcReport {
                entries_before: 3,
                entries_evicted: 1,
                blobs_removed: 2,
                bytes_freed: 17_000,
                corrupt_manifests: 0,
            }
        );
        assert_eq!(f.store.lookup(&key(1)).unwrap(), None);
        assert!(f.store.lookup(&key(2)).unwrap().is_some());
        assert!(!f.store.blob_path(&a.1).exists());
        assert!(!f.store.blob_path(&orphan.1).exists());
        for (_, id) in [&b, &c, &d] {
            assert!(f.store.blob_path(id).exists());
        }

        // A zero budget evicts entry 2, then stops at entry 3, hit inside
        // the grace window: the store stays over budget.
        let report = f.store.gc(0, 3_600, now).unwrap();
        assert_eq!(
            report,
            GcReport {
                entries_before: 2,
                entries_evicted: 1,
                blobs_removed: 2,
                bytes_freed: 6_000,
                corrupt_manifests: 0,
            }
        );
        assert_eq!(f.store.lookup(&key(2)).unwrap(), None);
        assert!(f.store.lookup(&key(3)).unwrap().is_some());
        assert_eq!(blob_files(&f.root), vec![f.store.blob_path(&d.1)]);
    }

    /// The blob-versioning contract of a re-ingested key (the warm
    /// `flutter test` face): a replacement manifest swaps the file's
    /// blob reference — nothing accumulates per dir — and the
    /// superseded blob is reclaimed by gc once past the in-flight
    /// window, while the entry and its current blob survive.
    #[test]
    fn store_gc_frees_superseded_blob_after_manifest_replacement() {
        let f = fixture();
        // Two runs rewrote the same dill path with different bytes.
        let dill = "ab.cache.dill.track.dill";
        let v1 = source(&f.build, dill, b"kernel v1");
        put(&f, 1, &[v1.clone()], 100);
        let v2 = source(&f.build, dill, b"kernel v2");
        put(&f, 1, &[v2.clone()], 200);

        let manifest = f.store.lookup(&key(1)).unwrap().unwrap();
        assert_eq!(manifest.dirs[0].files.len(), 1, "no append-only blob list");
        assert_eq!(manifest.dirs[0].files[0].blob, v2.1, "the reference is replaced");
        assert!(f.store.blob_path(&v1.1).exists(), "unreferenced until gc");

        // Fresh blobs may belong to an ingest in flight: v1 is spared.
        let now = unix_secs(SystemTime::now());
        assert_eq!(f.store.gc(u64::MAX, 0, now).unwrap().blobs_removed, 0);

        let report = f.store.gc(u64::MAX, 0, later()).unwrap();
        assert_eq!(report.entries_evicted, 0, "a huge budget keeps the entry");
        assert_eq!(report.blobs_removed, 1);
        assert_eq!(report.bytes_freed, (b"kernel v1".len() as u64));
        assert!(!f.store.blob_path(&v1.1).exists());
        assert!(f.store.blob_path(&v2.1).exists());
        assert!(f.store.lookup(&key(1)).unwrap().is_some());
    }

    #[test]
    fn store_stats_on_mixed_store() {
        let f = fixture();
        assert_eq!(f.store.stats().unwrap(), StoreStats::default());

        let a = source(&f.build, "a", &[b'a'; 10]);
        let b = source(&f.build, "b", &[b'b'; 20]);
        let c = source(&f.build, "c", &[b'c'; 40]);
        let orphan = source(&f.build, "o", &[b'o'; 80]);
        put(&f, 1, &[a.clone(), b.clone()], 300);
        put(&f, 2, &[b, c], 100);
        put(&f, 3, &[orphan], 50);
        drop_entry(&f, 3);
        // Neither foreign files nor an entry still being ingested count.
        fs::write(f.root.join(BLOBS).join(".DS_Store"), b"finder").unwrap();
        fs::write(f.store.blob_path(&a.1).with_file_name(".DS_Store"), b"finder").unwrap();
        fs::create_dir_all(f.store.entry_path(&key(9)).parent().unwrap()).unwrap();

        // The shared blob counts once; the unreferenced one occupies the
        // store until gc runs.
        assert_eq!(
            f.store.stats().unwrap(),
            StoreStats {
                entries: 2,
                blobs: 4,
                blob_bytes: 150,
                oldest_hit_unix: 100,
                corrupt_manifests: 0,
            }
        );
    }

    #[test]
    fn store_lookup_reports_corrupt_manifest() {
        let f = fixture();
        let garbage = f.store.entry_path(&key(1));
        fs::create_dir_all(garbage.parent().unwrap()).unwrap();
        fs::write(&garbage, b"\x00\xffnot json").unwrap();
        assert!(matches!(f.store.lookup(&key(1)), Err(BcacheError::Corrupt(_))));
        // record_hit only checks that a manifest file is there: the hit
        // sidecar is meaningless for an entry lookup already rejected,
        // and gc evicts it below either way.
        f.store.record_hit(&key(1)).unwrap();

        // A valid manifest cut short.
        let sources = [source(&f.build, "app.dill", b"kernel bytes")];
        put(&f, 2, &sources, 100);
        let truncated = f.store.entry_path(&key(2));
        let bytes = fs::read(&truncated).unwrap();
        fs::write(&truncated, &bytes[..bytes.len() / 2]).unwrap();
        assert!(matches!(f.store.lookup(&key(2)), Err(BcacheError::Corrupt(_))));

        // A well-formed manifest filed under another key.
        put(&f, 3, &sources, 100);
        let misfiled = f.store.entry_path(&key(4));
        fs::create_dir_all(misfiled.parent().unwrap()).unwrap();
        fs::copy(f.store.entry_path(&key(3)), &misfiled).unwrap();
        assert!(matches!(f.store.lookup(&key(4)), Err(BcacheError::Corrupt(_))));

        // stats summarizes around them and counts them; gc purges exactly
        // them. Neither face lets one bad manifest fail the store.
        assert_eq!(
            f.store.stats().unwrap(),
            StoreStats {
                entries: 1,
                blobs: 1,
                blob_bytes: b"kernel bytes".len() as u64,
                oldest_hit_unix: 100,
                corrupt_manifests: 3,
            }
        );
        let report = f.store.gc(u64::MAX, 0, later()).unwrap();
        assert_eq!(
            report,
            GcReport {
                entries_before: 4,
                entries_evicted: 3,
                blobs_removed: 0,
                bytes_freed: 0,
                corrupt_manifests: 3,
            }
        );
        for n in [1, 2, 4] {
            assert_eq!(f.store.lookup(&key(n)).unwrap(), None);
        }
        assert!(f.store.lookup(&key(3)).unwrap().is_some());
        assert_eq!(f.store.stats().unwrap().entries, 1);
    }

    #[test]
    fn store_record_hit_touches_the_sidecar_not_the_manifest() {
        let f = fixture();
        let sources = [source(&f.build, "app.dill", b"kernel bytes")];
        put(&f, 1, &sources, 100);
        let manifest = f.store.entry_path(&key(1));
        let bytes = fs::read(&manifest).unwrap();

        f.store.record_hit(&key(1)).unwrap();

        // The manifest is untouched byte for byte; the hit lives in the
        // sidecar's mtime, and the manifest's own field keeps the
        // ingest-time value.
        assert_eq!(fs::read(&manifest).unwrap(), bytes);
        let hit = f.store.lookup(&key(1)).unwrap().unwrap();
        assert_eq!(hit, draft(&key(1), &sources, 100));
        assert!(manifest.parent().unwrap().join(HIT).is_file());
        assert!(scratch_is_empty(&f.root));

        // The sidecar is the clock gc evicts by: entry 2 — same blob
        // bytes, same manifest hit field, no sidecar — is colder and
        // goes first under a budget that keeps one entry's worth.
        let cold = source(&f.build, "cold", b"cold entry bytes");
        put(&f, 2, &[cold.clone()], 100);
        let report = f.store.gc(b"cold entry bytes".len() as u64, 0, later()).unwrap();
        assert_eq!(report.entries_evicted, 1);
        assert!(f.store.lookup(&key(1)).unwrap().is_some());
        assert_eq!(f.store.lookup(&key(2)).unwrap(), None);

        assert!(matches!(
            f.store.record_hit(&key(2)),
            Err(BcacheError::NotFound(_))
        ));
        assert!(matches!(
            f.store.record_hit("../escape"),
            Err(BcacheError::Invalid(_))
        ));
        assert!(!f.store.entry_path(&key(3)).exists());
    }

    #[test]
    fn store_gc_spares_in_flight_files() {
        let f = fixture();
        let orphan = source(&f.build, "o", &[b'o'; 64]);
        put(&f, 1, &[orphan.clone()], 100);
        drop_entry(&f, 1);
        // The scratch file of an ingest that may still be writing.
        fs::write(f.root.join(TMP).join("4242-0-0"), [0u8; 16]).unwrap();

        let now = unix_secs(SystemTime::now());
        assert_eq!(f.store.gc(0, 0, now).unwrap(), GcReport::default());
        assert!(f.store.blob_path(&orphan.1).exists());
        assert!(!scratch_is_empty(&f.root));

        let report = f.store.gc(0, 0, later()).unwrap();
        assert_eq!(
            report,
            GcReport {
                entries_before: 0,
                entries_evicted: 0,
                blobs_removed: 1,
                bytes_freed: 64 + 16,
                corrupt_manifests: 0,
            }
        );
        assert!(blob_files(&f.root).is_empty());
        assert!(scratch_is_empty(&f.root));
    }

    #[test]
    fn store_ingest_rejects_invalid_drafts() {
        let f = fixture();
        let sources = [source(&f.build, "app.dill", b"kernel bytes")];
        let good = draft(&key(1), &sources, 100);

        let bad_key = EntryManifest {
            key: "../../escape".to_owned(),
            ..good.clone()
        };
        assert!(matches!(
            f.store.ingest(bad_key, &sources),
            Err(BcacheError::Invalid(_))
        ));
        for path in ["../outside", "/abs/app.dill", "", "./app.dill", "a/../b"] {
            let mut bad = good.clone();
            bad.dirs[0].files[0].path = PathBuf::from(path);
            assert!(
                matches!(f.store.ingest(bad, &sources), Err(BcacheError::Invalid(_))),
                "{path:?}"
            );
        }
        // A referenced blob that is neither sourced nor stored.
        assert!(matches!(
            f.store.ingest(good.clone(), &[]),
            Err(BcacheError::Invalid(_))
        ));
        // Rejection happens before any write.
        assert!(blob_files(&f.root).is_empty());
        assert_eq!(f.store.lookup(&key(1)).unwrap(), None);

        f.store.ingest(good.clone(), &sources).unwrap();
        // Once stored, a blob may be referenced without a source.
        let reuse = draft(&key(2), &sources, 100);
        f.store.ingest(reuse, &[]).unwrap();
        assert_eq!(f.store.lookup(&key(1)).unwrap(), Some(good));
    }

    /// Every file under `root` as (relative path, bytes), sorted — the
    /// byte-level snapshot the v1-isolation test compares.
    fn tree(root: &Path) -> Vec<(PathBuf, Vec<u8>)> {
        fn walk(dir: &Path, root: &Path, out: &mut Vec<(PathBuf, Vec<u8>)>) {
            for item in fs::read_dir(dir).unwrap() {
                let path = item.unwrap().path();
                if path.is_dir() {
                    walk(&path, root, out);
                } else {
                    out.push((
                        path.strip_prefix(root).unwrap().to_path_buf(),
                        fs::read(&path).unwrap(),
                    ));
                }
            }
        }
        let mut out = Vec::new();
        walk(root, root, &mut out);
        out.sort();
        out
    }

    /// Backdates `path`'s mtime past the in-flight window — the state of
    /// a store that sat between builds — without waiting an hour.
    #[cfg(unix)]
    fn age_past_in_flight(path: &Path) {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;
        let since = (SystemTime::now() - Duration::from_secs(2 * IN_FLIGHT_SECS))
            .duration_since(UNIX_EPOCH)
            .unwrap();
        let times = [
            libc::timespec {
                tv_sec: 0,
                tv_nsec: libc::UTIME_OMIT,
            },
            libc::timespec {
                tv_sec: since.as_secs() as i64,
                tv_nsec: i64::from(since.subsec_nanos()),
            },
        ];
        let cpath = CString::new(path.as_os_str().as_bytes()).unwrap();
        // SAFETY: utimensat reads the NUL-terminated path and the two
        // timespecs by pointer and returns; the return value is checked.
        let rc = unsafe { libc::utimensat(libc::AT_FDCWD, cpath.as_ptr(), times.as_ptr(), 0) };
        assert_eq!(rc, 0, "age {}", path.display());
    }

    /// One corrupt manifest among valid ones: stats succeeds and counts
    /// it, and gc still enforces a tiny budget on the rest — no single
    /// bad file can disable the disk budget.
    #[test]
    fn store_stats_and_gc_count_corrupt_manifests_and_keep_going() {
        let f = fixture();
        let a = source(&f.build, "a", &[b'a'; 40]);
        let b = source(&f.build, "b", &[b'b'; 40]);
        put(&f, 1, &[a.clone()], 100);
        put(&f, 2, &[b.clone()], 200);

        let garbage = f.store.entry_path(&key(3));
        fs::create_dir_all(garbage.parent().unwrap()).unwrap();
        fs::write(&garbage, b"\x00\xffnot json").unwrap();

        let stats = f.store.stats().unwrap();
        assert_eq!(stats.entries, 2);
        assert_eq!(stats.corrupt_manifests, 1);
        assert_eq!(stats.oldest_hit_unix, 100);

        // Budget for one blob: the coldest valid entry and the corrupt
        // one go; the newer valid entry stays with its blob.
        let report = f.store.gc(40, 0, later()).unwrap();
        assert_eq!(
            report,
            GcReport {
                entries_before: 3,
                entries_evicted: 2,
                blobs_removed: 1,
                bytes_freed: 40,
                corrupt_manifests: 1,
            }
        );
        assert_eq!(f.store.lookup(&key(1)).unwrap(), None);
        assert!(f.store.lookup(&key(2)).unwrap().is_some());
        assert!(f.store.blob_path(&b.1).exists());
        assert_eq!(
            f.store.stats().unwrap(),
            StoreStats {
                entries: 1,
                blobs: 1,
                blob_bytes: 40,
                oldest_hit_unix: 200,
                corrupt_manifests: 0,
            }
        );
    }

    /// A manifest with an enum variant this build does not know — the
    /// schema-drift shape the v2 cut made impossible across versions,
    /// still reachable by hand corruption: skipped and counted, never an
    /// error, and gc reclaims the entry.
    #[test]
    fn store_unknown_variants_are_counted_not_fatal() {
        let f = fixture();
        let a = source(&f.build, "a", &[b'a'; 10]);
        put(&f, 1, &[a.clone()], 100);

        let future = draft(&key(2), std::slice::from_ref(&a), 150);
        let mut json = String::from_utf8(to_json(&future).unwrap()).unwrap();
        assert!(json.contains("\"recompute_from_template\""));
        json = json.replace("\"recompute_from_template\"", "\"from_the_future\"");
        let path = f.store.entry_path(&key(2));
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, json).unwrap();

        // lookup degrades to Corrupt (a miss, a rebuild, a fresh
        // ingest); the walkers skip and count it instead.
        assert!(matches!(
            f.store.lookup(&key(2)),
            Err(BcacheError::Corrupt(_))
        ));
        let stats = f.store.stats().unwrap();
        assert_eq!(stats.entries, 1);
        assert_eq!(stats.oldest_hit_unix, 100);
        assert_eq!(stats.corrupt_manifests, 1);
        assert_eq!(stats.blobs, 1);

        let report = f.store.gc(u64::MAX, 0, later()).unwrap();
        assert_eq!(report.corrupt_manifests, 1);
        assert_eq!(report.entries_evicted, 1);
        assert_eq!(report.blobs_removed, 0, "the valid entry keeps its blob");
        assert!(f.store.lookup(&key(1)).unwrap().is_some());
        assert!(f.store.blob_path(&a.1).exists());
        assert_eq!(f.store.stats().unwrap().corrupt_manifests, 0);
    }

    /// The v2 cut in action: an old v1 root sits beside the v2 root,
    /// never read and never written again, while v2 works — an old
    /// binary keeps its own store and can neither judge nor delete v2
    /// entries.
    #[test]
    fn store_v2_open_leaves_a_v1_root_untouched() {
        assert_eq!(STORE_LAYOUT_VERSION, "v2");
        let dir = tempfile::tempdir().unwrap();
        let cas = dir.path().join("cas");

        let v1 = CasStore::open(&cas.join("v1")).unwrap();
        let old = source(dir.path(), "old.dill", b"v1 kernel bytes");
        v1.ingest(draft(&key(1), &[old.clone()], 100), &[old])
            .unwrap();
        let garbage = v1.entry_path(&key(2));
        fs::create_dir_all(garbage.parent().unwrap()).unwrap();
        fs::write(&garbage, b"\x00\xffnot json").unwrap();
        let before = tree(&cas.join("v1"));

        let v2 = CasStore::open(&cas.join(STORE_LAYOUT_VERSION)).unwrap();
        let fresh = source(dir.path(), "fresh.dill", b"v2 kernel bytes");
        v2.ingest(draft(&key(1), &[fresh.clone()], 100), &[fresh.clone()])
            .unwrap();
        v2.record_hit(&key(1)).unwrap();
        v2.gc(u64::MAX, 0, later()).unwrap();

        assert_eq!(tree(&cas.join("v1")), before, "v1 is never touched again");
        assert_eq!(
            v2.lookup(&key(1)).unwrap().unwrap().dirs[0].files[0].blob,
            fresh.1
        );
        assert_eq!(v2.lookup(&key(2)).unwrap(), None);
    }

    /// The A12 race as a live drill: ingesters re-referencing blobs on
    /// both reuse paths while gc passes run under a zero budget; after
    /// quiescence every published manifest's blobs exist — zero
    /// dangling. Pool blobs are aged past the in-flight window, so an
    /// unreferenced-and-untouched blob is genuinely deletable at any
    /// moment.
    #[test]
    #[cfg(unix)]
    fn store_gc_never_dangles_a_published_manifest() {
        let f = fixture();
        let store = Arc::new(CasStore::open(&f.root).unwrap());
        let pool: Vec<(PathBuf, BlobId)> = (0..12)
            .map(|i| source(&f.build, &format!("pool{i:02}"), &[(i % 251) as u8; 256]))
            .collect();
        // Seed the pool into the store, then detach it from every
        // manifest and age it past the in-flight window: an orphaned,
        // old blob a gc pass may reclaim the moment nothing re-touches
        // it.
        let seed = key(250);
        store.ingest(draft(&seed, &pool, 100), &pool).unwrap();
        fs::remove_dir_all(store.entry_path(&seed).parent().unwrap()).unwrap();
        for (_, id) in &pool {
            age_past_in_flight(&store.blob_path(id));
        }

        let ingesters: Vec<_> = (0..3)
            .map(|t| {
                let store = Arc::clone(&store);
                let pool = pool.clone();
                thread::spawn(move || {
                    for i in 0..300 {
                        let key = key(((t * 5 + i) % 8) as u8);
                        let blobs: Vec<(PathBuf, BlobId)> = (0..3)
                            .map(|j| pool[(t + i + j) as usize % pool.len()].clone())
                            .collect();
                        let entry = draft(&key, &blobs, 100);
                        // Both ingest shapes: with sources (put_blob's
                        // verify-and-touch path) and reuse-only (the
                        // validation-and-touch path).
                        let ingested = if i % 2 == 0 {
                            store.ingest(entry, &blobs)
                        } else {
                            store.ingest(entry, &[])
                        };
                        if ingested.is_ok() {
                            store.record_hit(&key).ok();
                        }
                    }
                })
            })
            .collect();
        let collectors: Vec<_> = (0..2)
            .map(|_| {
                let store = Arc::clone(&store);
                thread::spawn(move || {
                    for _ in 0..40 {
                        store.gc(0, 0, unix_secs(SystemTime::now())).ok();
                    }
                })
            })
            .collect();
        for handle in ingesters {
            handle.join().unwrap();
        }
        for handle in collectors {
            handle.join().unwrap();
        }
        // A closing pass under a realistic budget and grace window: the
        // hot entries survive it, and it must not dangle them either.
        store.gc(768, 3_600, unix_secs(SystemTime::now())).unwrap();

        let mut manifests = 0;
        for (shard, meta) in list(&f.root.join(ENTRIES)).unwrap() {
            if !meta.is_dir() {
                continue;
            }
            for (dir, meta) in list(&shard).unwrap() {
                if !meta.is_dir() {
                    continue;
                }
                let path = dir.join(MANIFEST);
                if !path.is_file() {
                    continue;
                }
                let bytes = fs::read(&path).unwrap();
                let key = dir.file_name().unwrap().to_string_lossy().into_owned();
                let manifest = parse_manifest(&path, &key, &bytes).unwrap();
                manifests += 1;
                for dir in &manifest.dirs {
                    for file in &dir.files {
                        assert!(
                            store.blob_path(&file.blob).is_file(),
                            "manifest {} names a missing blob",
                            path.display()
                        );
                    }
                }
            }
        }
        assert!(manifests > 0, "the race must leave live entries to check");
    }

    /// The A13 stress shape: record_hit hammering a key while an ingest
    /// replaces the same key's manifest. The manifest on disk is exactly
    /// the last ingested generation — a hit never reverts it — and the
    /// hits landed in the sidecar instead.
    #[test]
    fn store_record_hit_never_reverts_a_concurrent_ingest() {
        let f = fixture();
        let store = Arc::new(CasStore::open(&f.root).unwrap());
        let sources = [source(&f.build, "app.dill", b"kernel bytes")];
        let done = Arc::new(std::sync::atomic::AtomicBool::new(false));

        let hitter = {
            let store = Arc::clone(&store);
            let done = Arc::clone(&done);
            thread::spawn(move || {
                // Hammer for as long as the ingester runs (a fixed count
                // alone could finish before the first ingest lands, when
                // every hit is a NotFound); the cap is a hang guard only.
                let mut hits = 0u32;
                while !done.load(Ordering::Relaxed) && hits < 500_000 {
                    store.record_hit(&key(1)).ok();
                    hits += 1;
                }
            })
        };
        let ingester = {
            let store = Arc::clone(&store);
            let sources = sources.clone();
            thread::spawn(move || {
                let mut last = None;
                for generation in 0..300 {
                    let mut entry = draft(&key(1), &sources, 100);
                    entry.target.extra_key_args = vec![format!("gen{generation}")];
                    store.ingest(entry.clone(), &sources).unwrap();
                    last = Some(entry);
                }
                last
            })
        };
        let last = ingester.join().unwrap().unwrap();
        done.store(true, Ordering::Relaxed);
        hitter.join().unwrap();
        // One hit after quiescence: the sidecar must exist no matter how
        // the threads interleaved.
        store.record_hit(&key(1)).unwrap();

        assert_eq!(store.lookup(&key(1)).unwrap().unwrap(), last);
        assert!(
            store
                .entry_path(&key(1))
                .parent()
                .unwrap()
                .join(HIT)
                .is_file(),
            "the hits landed in the sidecar"
        );
    }

    /// The running usage counter: after a mixed sequence of ingests,
    /// reuses, replacements, and a healing gc pass, the counter equals a
    /// fresh full-walk total — the budget probe never needs the walk.
    #[test]
    #[cfg(unix)]
    fn store_running_usage_counter_matches_full_walks() {
        let f = fixture();
        let check = |f: &Fixture| {
            assert_eq!(
                f.store.used_bytes().unwrap(),
                f.store.stats().unwrap().blob_bytes,
                "counter equals the full walk"
            );
        };

        // Distinct fresh sources.
        let mut originals = Vec::new();
        for i in 0..20u8 {
            let src = source(
                &f.build,
                &format!("s{i:02}"),
                &vec![(i % 7) as u8; i as usize + 100],
            );
            put(&f, i, std::slice::from_ref(&src), 100 + u64::from(i));
            originals.push(src);
        }
        check(&f);

        // Reuse-only ingests book no new bytes.
        for i in 0..10u8 {
            let reuse = draft(&key(200 + i), std::slice::from_ref(&originals[i as usize]), 100);
            f.store.ingest(reuse, &[]).unwrap();
        }
        check(&f);

        // Replacements add the new generation's bytes; the superseded
        // blobs stay on disk until gc.
        for i in 0..5u8 {
            let src = source(&f.build, &format!("r{i:02}"), &[(i % 11) as u8 | 0x80; 300]);
            put(&f, i, std::slice::from_ref(&src), 200);
        }
        check(&f);

        // Age every blob past the in-flight window and gc under a real
        // budget: the evictions and removals rewrite the counter to the
        // truth the pass observed.
        for shard in fs::read_dir(f.root.join(BLOBS)).unwrap() {
            for blob in fs::read_dir(shard.unwrap().path()).unwrap() {
                age_past_in_flight(&blob.unwrap().path());
            }
        }
        f.store.gc(1_000, 0, unix_secs(SystemTime::now())).unwrap();
        check(&f);
        assert!(f.store.stats().unwrap().entries > 0);

        // Incremental booking continues on top of the healed value.
        let late = source(&f.build, "late", &[7u8; 250]);
        put(&f, 100, std::slice::from_ref(&late), 300);
        check(&f);
    }

    /// The scale guard: 60k blobs / 200 manifests — stats stays within a
    /// small multiple of a bare directory walk of the same tree (the
    /// audit baseline was 248 ms end to end; the walk dominates, the
    /// per-blob name validation is the rest, and nothing per-ingest ever
    /// rescans blobs).
    #[test]
    fn store_stats_walks_a_60k_blob_store_quickly() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("cas");
        let store = CasStore::open(&root).unwrap();
        for shard in 0..256 {
            fs::create_dir_all(root.join(BLOBS).join(format!("{shard:02x}"))).unwrap();
        }
        for i in 0..60_000u64 {
            let hex = format!("{i:064x}");
            fs::File::create(root.join(BLOBS).join(&hex[..2]).join(&hex)).unwrap();
        }
        for n in 0..200u8 {
            let sources: Vec<(PathBuf, BlobId)> = (0..3u64)
                .map(|j| {
                    let hex = format!("{:064x}", u64::from(n) * 3 + j);
                    (PathBuf::from(format!("f{j}")), hex.parse::<BlobId>().unwrap())
                })
                .collect();
            store.ingest(draft(&key(n), &sources, 100), &[]).unwrap();
        }

        store.stats().unwrap(); // warm the directory caches
        // The bar is relative, not absolute: the box runs concurrent
        // builds, so wall-clock flakes with its load. A bare walk of the
        // same tree (read_dir + metadata per blob, nothing else) is the
        // unavoidable cost of counting 60k blobs; stats() must stay a
        // small multiple of it — an extra stat or read per blob would
        // not. Best of five each isolates the walk's own cost.
        let bare_walk = |root: &Path| {
            let start = Instant::now();
            for shard in fs::read_dir(root.join(BLOBS)).expect("blobs root") {
                let shard = shard.expect("shard dir").path();
                for blob in fs::read_dir(&shard).expect("shard") {
                    // The metadata fetch is the walk's whole per-blob
                    // cost; the value itself goes unused.
                    blob.expect("blob").metadata().expect("blob metadata");
                }
            }
            start.elapsed()
        };
        let mut best_bare = Duration::from_secs(3600);
        let mut best_stats = Duration::from_secs(3600);
        let mut stats = StoreStats::default();
        for _ in 0..5 {
            best_bare = best_bare.min(bare_walk(&root));
            let start = Instant::now();
            stats = store.stats().unwrap();
            best_stats = best_stats.min(start.elapsed());
        }
        assert_eq!(stats.blobs, 60_000);
        assert_eq!(stats.entries, 200);
        assert_eq!(stats.corrupt_manifests, 0);
        assert!(
            best_stats < best_bare * 2,
            "stats {best_stats:?} vs a bare walk of {best_bare:?} — per-blob work crept into the walk"
        );
    }
}
