//! The content-addressed store: entries addressed by key, payload blobs
//! deduplicated by content.
//!
//! # Layout
//!
//! Under the root the caller injects (default `~/.cache/dartq/cas/v1`):
//!
//! * `blobs/<hex[0:2]>/<hex>` — immutable payload named by the blake3 of
//!   its bytes, 256-way sharded. Entries that differ only in their key
//!   (say, four define combinations over the same sources) share one set
//!   of blobs.
//! * `entries/<key[0:2]>/<key>/manifest.json` — one [`EntryManifest`] per
//!   key, compact JSON.
//! * `tmp/` — scratch space: every file is written here and renamed into
//!   place, never written in place.
//!
//! # Write protocol
//!
//! A publish is write scratch → sync → rename, so a reader of a store path
//! sees the previous file or the new one, never a partial write; the sync
//! orders the data ahead of the rename, so a crash can lose a fresh
//! publish but never leave a renamed file with torn content. Ingest
//! verifies each blob's bytes against its id while copying them and
//! publishes every blob before the manifest that references them: a stored
//! blob holds exactly the content its name claims, and a published
//! manifest never names a missing blob.
//!
//! # Concurrency
//!
//! Several dartq processes (one per worktree) may share a store; v1 takes
//! no locks. Content addressing makes blob publication idempotent — two
//! writers of one blob rename identical bytes onto the same name.
//! Manifests are last-writer-wins per key, and every writer of a key wrote
//! a valid entry for it. `tmp/` lives under the root, so every rename stays
//! on one volume and is atomic. A manifest that fails to parse surfaces as
//! [`BcacheError::Corrupt`]; callers degrade to a miss and rebuild.
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
//! * `ingest`: one streaming pass per source (read, hash, and copy fused,
//!   1 MiB buffer per worker), parallel across files; target: 100 MB of
//!   new blobs in under 2 s.
//! * `lookup`: one small file read and parse; target: under 5 ms warm.
//! * `gc` / `stats`: one pass over the manifests plus one walk over the
//!   blobs — O(entries + blobs), holding one small record per blob.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use rayon::prelude::*;
use rustc_hash::{FxHashMap, FxHashSet};
use serde::{Deserialize, Serialize};

use crate::error::{BcacheError, Result};
use crate::types::{BlobId, EntryManifest, StoreStats, blob_id_hex, is_hex_digest};

const BLOBS: &str = "blobs";
const ENTRIES: &str = "entries";
const TMP: &str = "tmp";
const MANIFEST: &str = "manifest.json";

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

    /// Where the manifest of entry `key` lives:
    /// `entries/<key[0:2]>/<key>/manifest.json`. Keys are the key engine's
    /// 64 lowercase hex digits; every fallible operation rejects any other
    /// spelling before it becomes a path.
    #[must_use]
    pub fn entry_path(&self, key: &str) -> PathBuf {
        let shard = key.get(..2).unwrap_or(key);
        self.root.join(ENTRIES).join(shard).join(key).join(MANIFEST)
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

    /// Stamps entry `key` as hit now — the clock gc evicts by. The manifest
    /// is rewritten atomically with `last_hit_unix` changed and every other
    /// field kept as this version's schema spells it. An entry gc removed
    /// meanwhile stays removed: the call fails ([`BcacheError::NotFound`],
    /// or an I/O error when the removal races the rewrite) instead of
    /// resurrecting a manifest whose blobs may be gone.
    pub fn record_hit(&self, key: &str) -> Result<()> {
        check_key(key)?;
        let path = self.entry_path(key);
        let Some(bytes) = read_if_present(&path)? else {
            return Err(BcacheError::NotFound(format!("entry {key}")));
        };
        let mut manifest: EntryManifest =
            serde_json::from_slice(&bytes).map_err(corrupt_at(&path))?;
        manifest.last_hit_unix = unix_secs(SystemTime::now());
        let mut scratch = self.scratch()?;
        scratch.write(&to_json(&manifest)?)?;
        scratch.publish(&path)
    }

    /// Publishes `draft` as the entry for `draft.key`, storing every source
    /// as a blob first.
    ///
    /// Each `(path, blob)` source is hashed while it streams into the store
    /// and must hash to `blob`; otherwise the ingest fails with
    /// [`BcacheError::Corrupt`] naming the path (the build tree changed
    /// under the ingest) and no manifest is published. A blob the store
    /// already holds is not rewritten, but its source is still verified:
    /// the draft's claim about that path must hold either way. The draft is
    /// rejected up front ([`BcacheError::Invalid`]) when its key is
    /// malformed, a file path is not a plain relative path, or a referenced
    /// blob is neither sourced nor stored — the checks run over the files
    /// of every [`crate::DirRecord`] of the draft. A previous manifest for
    /// the key is replaced whole. The draft is persisted as given,
    /// timestamps included.
    pub fn ingest(&self, draft: EntryManifest, sources: &[(PathBuf, BlobId)]) -> Result<()> {
        check_key(&draft.key)?;
        let manifest = to_json(&draft)?;
        let sourced: FxHashSet<BlobId> = sources.iter().map(|(_, id)| *id).collect();
        for dir in &draft.dirs {
            for file in &dir.files {
                if !is_plain_relative(&file.path) {
                    return Err(BcacheError::Invalid(format!(
                        "manifest path {:?}: expected a relative path of plain names",
                        file.path
                    )));
                }
                if !sourced.contains(&file.blob) && !self.blob_path(&file.blob).is_file() {
                    return Err(BcacheError::Invalid(format!(
                        "{}: blob {} is neither sourced nor stored",
                        file.path.display(),
                        blob_id_hex(&file.blob)
                    )));
                }
            }
        }
        sources.par_iter().try_for_each_init(
            || vec![0u8; CHUNK],
            |buf, (src, id)| self.put_blob(src, id, buf),
        )?;
        let path = self.entry_path(&draft.key);
        create_parent(&path)?;
        let mut scratch = self.scratch()?;
        scratch.write(&manifest)?;
        scratch.publish(&path)
    }

    /// Brings the blobs under `budget_bytes` if it can, then deletes every
    /// blob no remaining manifest references.
    ///
    /// Entries are evicted least recently hit first until the blobs that
    /// remain fit the budget. An entry hit within `grace_secs` of
    /// `now_unix` is never evicted, even if the store stays over budget; a
    /// manifest that cannot be read is evicted regardless, as no lookup can
    /// use it. Scratch files and unreferenced blobs younger than one hour
    /// may belong to an ingest in flight and are spared. Files that vanish
    /// mid-pass (a concurrent gc) are skipped.
    pub fn gc(&self, budget_bytes: u64, grace_secs: u64, now_unix: u64) -> Result<GcReport> {
        let entries = self.scan_entries()?;
        let blobs = self.scan_blobs()?;
        let mut report = GcReport {
            entries_before: entries.len() as u64,
            ..GcReport::default()
        };
        let in_flight = |mtime: u64| now_unix.saturating_sub(mtime) < IN_FLIGHT_SECS;

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
                Err(_) => evict.push(entry.dir),
            }
        }

        // Bytes that survive the pass as things stand, and the sizes of the
        // blobs the pass may delete once nothing references them.
        let mut kept: u64 = blobs
            .iter()
            .filter(|blob| is_referenced(&refs, &blob.id) || in_flight(blob.mtime_unix))
            .map(|blob| blob.len)
            .sum();
        let deletable: FxHashMap<BlobId, u64> = blobs
            .iter()
            .filter(|blob| !in_flight(blob.mtime_unix))
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
        for blob in &blobs {
            if is_referenced(&refs, &blob.id) || in_flight(blob.mtime_unix) {
                continue;
            }
            let path = self.blob_path(&blob.id);
            if removed(&path, fs::remove_file(&path))? {
                report.blobs_removed += 1;
                report.bytes_freed += blob.len;
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
        Ok(report)
    }

    /// Entry and blob counts, total blob bytes, and the oldest hit. A
    /// manifest that cannot be read fails the call with
    /// [`BcacheError::Corrupt`]; `gc` purges such entries.
    pub fn stats(&self) -> Result<StoreStats> {
        let mut stats = StoreStats::default();
        let mut oldest = u64::MAX;
        for entry in self.scan_entries()? {
            oldest = oldest.min(entry.facts?.last_hit_unix);
            stats.entries += 1;
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

    /// Stores `src` as blob `id`, verifying while copying that its bytes
    /// hash to `id`. A stored blob is left alone once `src` verifies —
    /// unless its size disagrees with the source (a copy truncated by a
    /// crash or damaged outside the store), which is then replaced.
    fn put_blob(&self, src: &Path, id: &BlobId, buf: &mut [u8]) -> Result<()> {
        let dest = self.blob_path(id);
        if let Ok(stored) = fs::metadata(&dest) {
            let (actual, len) = hash_copy(src, None, buf)?;
            verify(src, id, &actual)?;
            if stored.len() == len {
                return Ok(());
            }
        }
        let mut scratch = self.scratch()?;
        let (actual, _) = hash_copy(src, Some(&mut scratch), buf)?;
        verify(src, id, &actual)?;
        create_parent(&dest)?;
        scratch.publish(&dest)
    }

    /// A fresh scratch file. Its name is unique among live writers (pid
    /// plus an in-process sequence); the timestamp keeps a recycled pid
    /// from colliding with the leftover of a crashed process.
    fn scratch(&self) -> Result<Scratch> {
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since| since.as_nanos());
        let name = format!(
            "{}-{}-{nanos}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        );
        let path = self.root.join(TMP).join(name);
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
    /// [`Self::lookup`]: an entry gc keeps is one lookup can serve. An entry
    /// directory without a manifest belongs to an ingest in flight (or is
    /// the empty leftover of a crashed one) and is not an entry.
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
                let Some(bytes) = read_if_present(&path)? else {
                    continue;
                };
                let key = dir
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let facts = parse_manifest(&path, &key, &bytes).map(|manifest| EntryFacts {
                    last_hit_unix: manifest.last_hit_unix,
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
    /// [`Self::blob_path`] puts its name; anything else under `blobs/` is
    /// foreign and left alone.
    fn scan_blobs(&self) -> Result<Vec<ScannedBlob>> {
        let mut blobs = Vec::new();
        for (shard, meta) in list(&self.root.join(BLOBS))? {
            if !meta.is_dir() {
                continue;
            }
            for (path, meta) in list(&shard)? {
                let Some(id) = path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .and_then(|name| name.parse::<BlobId>().ok())
                else {
                    continue;
                };
                if meta.is_file() && self.blob_path(&id) == path {
                    blobs.push(ScannedBlob {
                        id,
                        len: meta.len(),
                        mtime_unix: mtime_unix(&path, &meta)?,
                    });
                }
            }
        }
        Ok(blobs)
    }
}

/// What gc and stats keep of a manifest: its hit time and the blobs it
/// references (one per file, repeats included).
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

/// One blob found on disk.
struct ScannedBlob {
    id: BlobId,
    len: u64,
    mtime_unix: u64,
}

/// A file under `tmp/` on its way into the store. Removed on drop unless
/// published, so a failed write leaves no residue.
struct Scratch {
    path: PathBuf,
    file: File,
    published: bool,
}

impl Scratch {
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

/// One streaming pass over `src`: hashes every byte and, given a sink,
/// writes the same bytes to it — the stored copy is exactly the hashed
/// content, with no window for `src` to change in between. Returns the
/// digest and the byte count.
fn hash_copy(src: &Path, mut sink: Option<&mut Scratch>, buf: &mut [u8]) -> Result<(BlobId, u64)> {
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
        if let Some(sink) = sink.as_deref_mut() {
            sink.write(&buf[..n])?;
        }
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
                })
                .collect(),
        }
    }

    fn draft(key: &str, sources: &[(PathBuf, BlobId)], last_hit_unix: u64) -> EntryManifest {
        EntryManifest {
            key: key.to_owned(),
            project_root_at_ingest: "/work/app".to_owned(),
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

        // Changed before its blob is stored: the streaming copy catches it.
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
            }
        );
        assert_eq!(f.store.lookup(&key(2)).unwrap(), None);
        assert!(f.store.lookup(&key(3)).unwrap().is_some());
        assert_eq!(blob_files(&f.root), vec![f.store.blob_path(&d.1)]);
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
        assert!(matches!(f.store.record_hit(&key(1)), Err(BcacheError::Corrupt(_))));

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

        // stats refuses to summarize around them; gc purges exactly them.
        assert!(matches!(f.store.stats(), Err(BcacheError::Corrupt(_))));
        let report = f.store.gc(u64::MAX, 0, later()).unwrap();
        assert_eq!(
            report,
            GcReport {
                entries_before: 4,
                entries_evicted: 3,
                blobs_removed: 0,
                bytes_freed: 0,
            }
        );
        for n in [1, 2, 4] {
            assert_eq!(f.store.lookup(&key(n)).unwrap(), None);
        }
        assert!(f.store.lookup(&key(3)).unwrap().is_some());
        assert_eq!(f.store.stats().unwrap().entries, 1);
    }

    #[test]
    fn store_record_hit_updates_last_hit_only() {
        let f = fixture();
        let sources = [source(&f.build, "app.dill", b"kernel bytes")];
        put(&f, 1, &sources, 100);

        let before = unix_secs(SystemTime::now());
        f.store.record_hit(&key(1)).unwrap();
        let hit = f.store.lookup(&key(1)).unwrap().unwrap();
        assert!(hit.last_hit_unix >= before);
        assert_eq!(
            EntryManifest {
                last_hit_unix: 100,
                ..hit
            },
            draft(&key(1), &sources, 100)
        );
        assert!(scratch_is_empty(&f.root));

        assert!(matches!(
            f.store.record_hit(&key(2)),
            Err(BcacheError::NotFound(_))
        ));
        assert!(matches!(
            f.store.record_hit("../escape"),
            Err(BcacheError::Invalid(_))
        ));
        assert!(!f.store.entry_path(&key(2)).exists());
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
}
