//! The on-disk store: XDG-rooted generations, atomic writes, LRU capacity.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{MutexGuard, PoisonError};
use std::time::SystemTime;

use filetime::{FileTime, set_file_atime, set_file_mtime};
use rustc_hash::{FxHashMap, FxHashSet};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::entry::{self, Entry, EntryKey};
use crate::header::{GenerationHeader, SNAPSHOT_FORMAT_VERSION};
use crate::remap::RemapTables;

/// Default L2 capacity: 10 GiB.
pub const DEFAULT_CAPACITY_BYTES: u64 = 10 * 1024 * 1024 * 1024;

/// How many saves may pass before the capacity scan runs regardless of the
/// size estimate — the bound on how far the cheap estimate can drift from
/// reality.
const ENFORCE_EVERY_SAVES: u32 = 32;

/// Entry ids this store instance knows to be on disk, keyed by
/// `(generation dir id, tables stamp)` — the flush-path bookkeeping
/// (#1386). The daemon flushes every resident entry per debounce tick, and
/// the caller pays a full facts projection plus an encode per entry for
/// each; a recorded id lets the caller skip both, because within a
/// generation a frame is byte-determined by its key: the payload is a pure
/// function of the entry key (same content hash, features, semantics, and
/// header context reanalyze identically — the determinism the
/// deterministic path-smallest payload pick already leans on). The set
/// only ever grows from evidence: a landed write (atomic rename, or the
/// byte-identical read-compare skip), or a completed `load` (which proved
/// the whole manifest is on disk under the loaded stamp). It shrinks on
/// the matching counter-evidence: the save-end sweep unrecords every entry
/// file it deletes, a save or load under one stamp voids sibling-stamp
/// records for the same generation, and capacity eviction prunes whole
/// directories. Under-recording costs redundant work; the only way to a
/// wrong skip is external cache deletion mid-session, which is not a
/// supported scenario — and even then the manifest gate degrades the
/// generation to a miss, never a mixture.
type WrittenKeys = FxHashMap<([u8; 16], [u8; 16]), FxHashSet<[u8; 16]>>;

/// The per-generation load-recency marker: one file touched once per load
/// stands in for per-entry utimes.
const RECENCY_MARKER: &str = "recency";

/// Uniqueness suffix for temporary files; rename targets never collide
/// across concurrent writers in one process.
static TMP_SEQ: AtomicU32 = AtomicU32::new(0);

/// Store construction parameters. `root: None` resolves the XDG default
/// (`$XDG_CACHE_HOME/dartq`, falling back to `$HOME/.cache/dartq`).
#[derive(Clone, Debug)]
pub struct SnapshotConfig {
    pub root: Option<PathBuf>,
    pub capacity_bytes: u64,
}

impl Default for SnapshotConfig {
    fn default() -> Self {
        Self {
            root: None,
            capacity_bytes: DEFAULT_CAPACITY_BYTES,
        }
    }
}

/// Why a save gave up. Load never surfaces these — an unwritten generation
/// is simply a future miss.
#[derive(Debug, Error)]
pub enum SaveError {
    #[error("unsupported snapshot format_version {requested}: this crate writes {supported}")]
    UnsupportedFormatVersion { requested: u32, supported: u32 },
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

/// One loaded generation: the matched header, the remap slices, and the
/// complete manifest-verified entry set. A missing, extra, or undecodable
/// entry demotes the whole load to `None` — a returned `Snapshot` is
/// always internally consistent, never a subset of a save.
#[derive(Clone, Debug)]
pub struct Snapshot<T> {
    pub header: GenerationHeader,
    pub tables: RemapTables,
    pub entries: Vec<Entry<T>>,
}

/// The write half of `tables.json`: the remap slices and the entry-id
/// manifest in one atomic unit, so the pairing between a save's tables and
/// its entry files cannot be torn apart by a crash — a manifest from one
/// save never faces the entries of another.
#[derive(Serialize)]
struct TablesFile<'a> {
    tables: &'a RemapTables,
    entry_ids: &'a [[u8; 16]],
}

/// The read half of `tables.json`.
#[derive(Deserialize)]
struct StoredTables {
    tables: RemapTables,
    entry_ids: Vec<[u8; 16]>,
}

/// The persistent snapshot store under one cache root.
///
/// All reads are best-effort and infallible (accelerator invariant); all
/// writes go through temp-file + [`fs::rename`] into the destination
/// directory, so readers never see partial files.
pub struct SnapshotStore {
    root: PathBuf,
    capacity_bytes: u64,
    /// Cheap high-watermark estimate of the cache total: bytes saved
    /// accrue, evictions and full scans re-anchor it. Only when it crosses
    /// `capacity_bytes` does `save` pay for a real scan.
    estimated_bytes: AtomicU64,
    /// Saves since the last full scan; `ENFORCE_EVERY_SAVES` forces one.
    saves_since_enforce: AtomicU32,
    /// The on-disk entry bookkeeping ([`WrittenKeys`]); mutated under the
    /// lock by `load` (seed), [`Self::save_delta`] (record + unrecord),
    /// and [`Self::enforce_capacity`] (eviction prune), and read by
    /// [`Self::written_keys`] — all cold, once-per-flush paths, never per
    /// entry.
    written: Mutex<WrittenKeys>,
}

impl SnapshotStore {
    /// Opens a store at `config.root` (or the XDG default).
    ///
    /// # Errors
    ///
    /// Only when no root is configured and neither `XDG_CACHE_HOME` nor
    /// `HOME` is set. The directory itself is created lazily by `save`.
    pub fn open(config: SnapshotConfig) -> io::Result<Self> {
        let root = match config.root {
            Some(root) => root,
            None => default_cache_root()?,
        };
        Ok(Self {
            root,
            capacity_bytes: config.capacity_bytes,
            estimated_bytes: AtomicU64::new(0),
            saves_since_enforce: AtomicU32::new(0),
            written: Mutex::new(FxHashMap::default()),
        })
    }

    /// The cache root this store reads and writes.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Loads the generation matching `expected`, or `None` on any
    /// mismatch, damage, or incompleteness: the header must equal
    /// `expected`, `tables.json` must parse, the entries directory must
    /// exist, and the decodable entries whose remap-slices stamp matches
    /// this generation's tables — and whose payload keys agree with their
    /// filenames — must equal the entry-id manifest exactly: a missing,
    /// extra, or undecodable entry demotes the whole generation to a
    /// miss. There is no partial `Some`.
    ///
    /// The stamp check is the save-group pairing gate (#338): an entry
    /// file only decodes under the remap slices it was saved with. A file
    /// whose stamp mismatches — an earlier save's entry facing slices that
    /// changed since, the mixed middle of a torn re-save — is treated as
    /// *absent*, never fatal; the manifest equality then decides. Stale
    /// files from an interrupted sweep are therefore invisible residue,
    /// while a stamped file outside the manifest still demotes the
    /// generation.
    ///
    /// Entry order is deterministic (sorted by key id), and the load
    /// touches the generation's recency marker once, refreshing its access
    /// time for the LRU sweep.
    #[must_use]
    pub fn load<T: DeserializeOwned>(&self, expected: &GenerationHeader) -> Option<Snapshot<T>> {
        let generation = self.generation_dir(expected);
        let stored: GenerationHeader = parse(&fs::read(generation.join("header.json")).ok()?)?;
        if stored != *expected {
            return None;
        }
        let tables_bytes = fs::read(generation.join("tables.json")).ok()?;
        let stored_tables: StoredTables = parse(&tables_bytes)?;
        // Same derivation as `save`: the stamp hashes the remap-slices
        // serialization, so a manifest-only rewrite of `tables.json` pairs
        // with the entries unchanged (#422).
        let tables_stamp = crate::blake3_128(&serde_json::to_vec(&stored_tables.tables).ok()?);
        let mut claimed = stored_tables.entry_ids;
        claimed.sort_unstable();

        // The entries directory must exist (a generation is its entries,
        // even when the manifest is empty) and every decodable, stamped,
        // honestly-keyed entry file must carry a claimed id.
        let mut present: Vec<[u8; 16]> = Vec::with_capacity(claimed.len());
        let mut entries = Vec::with_capacity(claimed.len());
        for file in fs::read_dir(generation.join("entries")).ok()?.flatten() {
            let Some(id) = entry_file_id(&file.path()) else {
                continue;
            };
            let bytes = fs::read(file.path()).ok()?;
            // #422 lazy-decode audit: every decoded entry here is consumed
            // in full — the only callers are this crate's contract tests,
            // and the planned #344 seam materializes each loaded entry
            // (dartq-engine `state/persist.rs`) — so eager JSON decode
            // stays; split into frame-verify + per-key payload parse only
            // if a consumer that reads a subset of keys appears.
            let (decoded, stamp) = entry::decode::<T>(&bytes)?;
            // A foreign stamp is an entry from another save group (#338).
            // A payload key disagreeing with the filename is a relabeled
            // file, served under a key it does not hold. Both are *absent*,
            // never fatal.
            if stamp != tables_stamp {
                continue;
            }
            if decoded.key.id() != id {
                continue;
            }
            entries.push(decoded);
            present.push(id);
        }
        present.sort_unstable();
        if present != claimed {
            return None;
        }
        entries.sort_by_key(|entry| entry.key.id());
        // A completed load is the strongest seeding the bookkeeping can
        // get (#1386): the manifest equality just proved every entry file
        // is on disk under this generation's stamp, so the next flush may
        // skip all of them. The loaded stamp is the directory's truth;
        // sibling-stamp records (a previous save group) are void.
        let dir = stored.dir_id();
        {
            let mut written = self.lock_written();
            written.retain(|(generation, _), _| generation != &dir);
            written.insert((dir, tables_stamp), present.iter().copied().collect());
        }
        touch_recency(&generation.join(RECENCY_MARKER));
        Some(Snapshot {
            header: stored,
            tables: stored_tables.tables,
            entries,
        })
    }

    /// Writes a generation: the remap tables with the entry-id manifest
    /// folded in first (one atomic unit — the tables stamp every entry
    /// frame embeds must exist before the entries do), then the stamped
    /// entries, then the stale-entry sweep, then the header. An entry file
    /// whose new frame is byte-identical to the incumbent is recognized
    /// and left untouched rather than rewritten (#422); `tables.json` is
    /// always written, since its manifest is exactly what an entry-set
    /// delta changes. `load` accepts
    /// the generation only when the directory holds precisely the stamped,
    /// claimed set — a torn save leaves old-stamped entries facing the new
    /// tables and demotes to a miss instead of loading a mixture of two
    /// saves (#338 save-group atomicity: the tables-stamp scheme, chosen
    /// over flock for portability — no platform locking primitives, and a
    /// concurrent-writer race degrades to a miss, never corruption).
    ///
    /// The save-shape contract (see the crate docs): a save claims the
    /// *complete* entry set of the generation. At save end the sweep removes
    /// every on-disk entry outside the claim, so any set shape — growing,
    /// shrinking, or changed — re-saves coherently, and a failed save
    /// degrades to a miss, never a mixture. Entries must carry distinct
    /// keys: the manifest deduplicates and both frames would write one
    /// path, so a duplicate key silently last-wins — a deterministic caller
    /// bug, asserted against in debug builds.
    ///
    /// Capacity is throttled, not per-save: the bytes just written accrue
    /// into a cheap estimate, and only when the estimate crosses
    /// `capacity_bytes` or `ENFORCE_EVERY_SAVES` saves have passed does
    /// the save pay for the full scan ([`Self::enforce_capacity`]).
    ///
    /// # Errors
    ///
    /// A header claiming a foreign `format_version` (nothing is written),
    /// or an IO/serialization failure: the generation then loads as the
    /// last complete state, or as a miss — never as a mixture.
    pub fn save<T: Serialize>(
        &self,
        header: &GenerationHeader,
        tables: &RemapTables,
        entries: &[Entry<T>],
    ) -> Result<(), SaveError> {
        // The authoritative full write: every payload passed is carried as
        // changed, so the bookkeeping never makes `save` skip one — the
        // entries are the truth, even (same key, different payload) when a
        // previous save under the same stamp is already recorded.
        let manifest: Vec<EntryKey> = entries.iter().map(|entry| entry.key).collect();
        self.save_delta(header, tables, &manifest, entries)
    }

    /// The entry ids recorded as on disk under `tables`' stamp in
    /// `header`'s generation, restricted to `keys` — the flush face's skip
    /// set (#1386). Only ids this store instance wrote (or loaded) are
    /// ever returned; an absent record means "project and write", the
    /// conservative answer. The engine filters its facts projections
    /// through this before calling [`Self::save_delta`], turning the
    /// debounced daemon flush from O(residency) into O(changed).
    ///
    /// A serialization failure of `tables` answers empty — nothing is
    /// vouched, so the caller does full work; the accelerator invariant
    /// keeps this query infallible.
    #[must_use]
    pub fn written_keys(
        &self,
        header: &GenerationHeader,
        tables: &RemapTables,
        keys: &[EntryKey],
    ) -> Vec<EntryKey> {
        let Some(stamp) = stamp_of(tables) else {
            return Vec::new();
        };
        let written = self.lock_written();
        let Some(records) = written.get(&(header.dir_id(), stamp)) else {
            return Vec::new();
        };
        keys.iter()
            .filter(|key| records.contains(&key.id()))
            .copied()
            .collect()
    }

    /// The flush variant of [`Self::save`]: `manifest` claims the
    /// generation's *complete* entry set exactly as `save`'s entries would
    /// (the manifest, sweep, and atomicity contract are identical), while
    /// only `changed` carries payloads to encode and write — the subset
    /// the caller found unrecorded via [`Self::written_keys`]. Manifest
    /// keys absent from `changed` must already be on disk under this
    /// stamp, which the written-keys bookkeeping is the record of; a save
    /// records each changed key whose write lands (or whose frame proves
    /// byte-identical to the incumbent), and unrecords every entry the
    /// save-end sweep deletes — so only landed state is ever vouched.
    ///
    /// All of [`Self::save`]'s contract applies unchanged: tables.json is
    /// always written, the sweep removes every on-disk entry outside the
    /// manifest claim, a torn save degrades to a miss, and capacity is
    /// throttled as in `save`.
    ///
    /// # Errors
    ///
    /// Exactly as [`Self::save`].
    pub fn save_delta<T: Serialize>(
        &self,
        header: &GenerationHeader,
        tables: &RemapTables,
        manifest: &[EntryKey],
        changed: &[Entry<T>],
    ) -> Result<(), SaveError> {
        if header.format_version != SNAPSHOT_FORMAT_VERSION {
            return Err(SaveError::UnsupportedFormatVersion {
                requested: header.format_version,
                supported: SNAPSHOT_FORMAT_VERSION,
            });
        }
        let generation = self.generation_dir(header);
        let entries_dir = generation.join("entries");
        fs::create_dir_all(&entries_dir)?;
        let mut ids: Vec<[u8; 16]> = manifest.iter().map(EntryKey::id).collect();
        debug_assert_eq!(
            changed.len(),
            {
                let mut unique: Vec<[u8; 16]> =
                    changed.iter().map(|entry| entry.key.id()).collect();
                unique.sort_unstable();
                unique.dedup();
                unique.len()
            },
            "duplicate EntryKey in `save_delta`'s changed set: the manifest \
             dedups and both frames would write one path — the caller must \
             not carry one key twice"
        );
        ids.sort_unstable();
        ids.dedup();
        debug_assert!(
            changed
                .iter()
                .all(|entry| ids.binary_search(&entry.key.id()).is_ok()),
            "a changed entry outside the manifest claim: `manifest` must name \
             the complete entry set, `changed` the subset to write"
        );
        let encoded_tables = serde_json::to_vec(&TablesFile {
            tables,
            entry_ids: &ids,
        })?;
        // The stamp covers only what entry payloads depend on (#422): the
        // remap-slices serialization. The entry-id manifest shares
        // tables.json's atomic write below (one unit, so a manifest can
        // never face another save's entries) but stays outside the hash —
        // an entry-set delta leaves the stamp, and every unchanged entry
        // frame, byte-identical.
        let tables_stamp = crate::blake3_128(&serde_json::to_vec(tables)?);
        // Tables first: the entry frames below embed the tables stamp,
        // pairing each entry with the save group that wrote it.
        atomic_write(&generation.join("tables.json"), &encoded_tables)?;
        let dir = header.dir_id();
        // This save is the generation's new truth: records from sibling
        // stamps can no longer vouch for on-disk frames (the frames they
        // named were overwritten or swept). Today's saves always use the
        // default tables, so this is a no-op in practice — the #1386
        // audit's note — but the bookkeeping keys on the stamp so a future
        // non-default save group cannot serve stale skips.
        let mut written = self.lock_written();
        written.retain(|(generation_dir, _), _| generation_dir != &dir);
        let records = written.entry((dir, tables_stamp)).or_default();
        let mut written_bytes = encoded_tables.len() as u64;
        for entry in changed {
            let framed = entry::encode(entry, tables_stamp)?;
            let path = entries_dir.join(format!("{}.entry", hex::encode(entry.key.id())));
            // A byte-identical frame is already on disk — the narrowed
            // stamp makes this the common case for an entry-set delta — so
            // the incumbent file is left untouched instead of rewritten;
            // reading a small framed file and comparing is strictly
            // cheaper than the temp+rename write. `tables.json` above is
            // never skipped: its manifest is exactly what an entry-set
            // delta changes. Either way the frame is on disk under this
            // stamp, so the key is recorded.
            if !fs::read(&path).is_ok_and(|incumbent| incumbent == framed) {
                written_bytes += framed.len() as u64;
                atomic_write(&path, &framed)?;
            }
            records.insert(entry.key.id());
        }
        // Generational sweep: the manifest claims exactly `ids`, so entry
        // files left by an earlier save with a different set are removed —
        // the on-disk set equals the claim when this save returns. Each
        // removal unrecords its id: a key that later rejoins the claim
        // (its content edited back) must project and write again, or the
        // manifest would claim a swept file.
        if let Ok(present) = fs::read_dir(&entries_dir) {
            for file in present.flatten() {
                if let Some(id) = entry_file_id(&file.path())
                    && ids.binary_search(&id).is_err()
                {
                    let _ = fs::remove_file(file.path());
                    records.remove(&id);
                }
            }
        }
        drop(written);
        let encoded_header = serde_json::to_vec(header)?;
        written_bytes += encoded_header.len() as u64;
        atomic_write(&generation.join("header.json"), &encoded_header)?;

        let estimate = self
            .estimated_bytes
            .fetch_add(written_bytes, Ordering::Relaxed)
            + written_bytes;
        let saves = self.saves_since_enforce.fetch_add(1, Ordering::Relaxed) + 1;
        if estimate > self.capacity_bytes || saves >= ENFORCE_EVERY_SAVES {
            self.enforce_capacity();
        }
        Ok(())
    }

    /// Evicts whole generations, oldest access first, until the total size
    /// fits `capacity_bytes` — except the newest generation, which is the
    /// one just written or just loaded and is never evicted even when the
    /// store stays over capacity (recency beats fitting). Group eviction
    /// keeps a generation either present or absent — never half-valid
    ///. The walk also removes leaked temp files (crash
    /// residue) as a side pass and re-anchors the estimate `save` accrues.
    /// Best-effort: unreadable files are skipped, never reported. Evicted
    /// directories leave the written-keys bookkeeping with them — a stale
    /// record over a removed generation would make a later re-save skip
    /// files that no longer exist.
    pub fn enforce_capacity(&self) {
        self.saves_since_enforce.store(0, Ordering::Relaxed);
        let mut total = 0u64;
        let mut generations: Vec<(PathBuf, u64, SystemTime)> = Vec::new();
        if let Ok(dirs) = fs::read_dir(self.root.join("snapshots")) {
            for path in dirs.flatten().map(|d| d.path()) {
                // A generation is a real directory; symlinks are never
                // followed (the same rule that keeps the scan terminating).
                if !fs::symlink_metadata(&path).is_ok_and(|meta| meta.is_dir()) {
                    continue;
                }
                let (bytes, recency) = dir_stats(&path);
                total += bytes;
                generations.push((path, bytes, recency));
            }
        }
        if total > self.capacity_bytes {
            generations.sort_by_key(|(_, _, recency)| *recency);
            let evictable = generations.len().saturating_sub(1);
            let mut evicted: Vec<[u8; 16]> = Vec::new();
            for (path, bytes, _) in &generations[..evictable] {
                if total <= self.capacity_bytes {
                    break;
                }
                if fs::remove_dir_all(path).is_ok() {
                    total -= bytes;
                    if let Some(dir) = decode_dir_id(path) {
                        evicted.push(dir);
                    }
                }
            }
            if !evicted.is_empty() {
                self.lock_written()
                    .retain(|(dir, _), _| !evicted.contains(dir));
            }
        }
        self.estimated_bytes.store(total, Ordering::Relaxed);
    }

    /// Locks the bookkeeping map. Poison is recovered rather than
    /// propagated: the records are plain best-effort sets, and a panic
    /// mid-update can only under-record (a not-yet-recorded landed write),
    /// which costs redundant work — never a wrong skip.
    fn lock_written(&self) -> MutexGuard<'_, WrittenKeys> {
        self.written.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn generation_dir(&self, header: &GenerationHeader) -> PathBuf {
        self.root
            .join("snapshots")
            .join(hex::encode(header.dir_id()))
    }
}

/// Parses JSON, mapping failures to the load-side miss convention.
fn parse<T: DeserializeOwned>(bytes: &[u8]) -> Option<T> {
    serde_json::from_slice(bytes).ok()
}

/// The entry id named by `path` — `<32 hex>.entry`, the exact shape `save`
/// writes — or `None` for anything else. Unparseable names (a crashed
/// writer's temp file, planted junk) are never entries and never fatal.
#[must_use]
fn entry_file_id(path: &Path) -> Option<[u8; 16]> {
    let name = path.file_name()?.to_str()?;
    let stem = name.strip_suffix(".entry")?;
    if stem.len() != 32 {
        return None;
    }
    let mut id = [0u8; 16];
    hex::decode_to_slice(stem, &mut id).ok()?;
    Some(id)
}

/// The generation dir id a `snapshots/` directory entry names — the hex
/// filename [`SnapshotStore::generation_dir`] writes — or `None` for
/// anything else.
#[must_use]
fn decode_dir_id(path: &Path) -> Option<[u8; 16]> {
    let stem = path.file_name()?.to_str()?;
    let mut id = [0u8; 16];
    hex::decode_to_slice(stem, &mut id).ok()?;
    Some(id)
}

/// The tables-stamp derivation the entry frames embed (`save_delta`), shared
/// by the written-keys bookkeeping so records key on exactly the value the
/// frames carry. `None` on a serialization failure — the conservative
/// answer that vouches for nothing.
fn stamp_of(tables: &RemapTables) -> Option<[u8; 16]> {
    Some(crate::blake3_128(&serde_json::to_vec(tables).ok()?))
}

/// Creates (or reuses) the generation's recency marker and bumps its
/// atime/mtime, so the LRU sweep sees the hit even on noatime mounts — one
/// touched file per load, not two utimes per entry. Errors are irrelevant:
/// an untouched marker only leaves the generation aging by its file times.
fn touch_recency(path: &Path) {
    if fs::File::create(path).is_ok() {
        let now = FileTime::from_system_time(SystemTime::now());
        let _ = set_file_atime(path, now);
        let _ = set_file_mtime(path, now);
    }
}

/// Writes `bytes` to a unique sibling temporary file, then renames it over
/// `path`. The temp file lives in the destination directory, so the rename
/// is a same-filesystem atomic replace. Durability (fsync) is deliberately
/// omitted: losing a young snapshot to power loss is an acceptable miss.
fn atomic_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("snapshot");
    let tmp = path.with_file_name(format!(
        "{name}.tmp-{}-{}",
        std::process::id(),
        TMP_SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    let written = fs::write(&tmp, bytes).and_then(|()| fs::rename(&tmp, path));
    if written.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    written
}

/// `$XDG_CACHE_HOME/dartq`, or `$HOME/.cache/dartq` when unset, empty, or
/// relative (the XDG spec requires an absolute path).
fn default_cache_root() -> io::Result<PathBuf> {
    let base = if let Some(dir) = std::env::var_os("XDG_CACHE_HOME")
        && !dir.is_empty()
        && Path::new(&dir).is_absolute()
    {
        PathBuf::from(dir)
    } else {
        let home = std::env::var_os("HOME")
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "HOME not set"))?;
        PathBuf::from(home).join(".cache")
    };
    Ok(base.join("dartq"))
}

/// Total file size and newest file timestamp under `dir`, recursively,
/// removing leaked temp files (crash residue) as a side pass. Directory
/// recursion keys on the entry's own file type — symlinks are never
/// resolved — so a symlinked cycle terminates instead of recursing to a
/// stack overflow. Unreadable entries are skipped; an empty or unreadable
/// directory stats as epoch-old, making it the first eviction candidate.
fn dir_stats(dir: &Path) -> (u64, SystemTime) {
    let mut total = 0u64;
    let mut newest = SystemTime::UNIX_EPOCH;
    let Ok(entries) = fs::read_dir(dir) else {
        return (total, newest);
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_dir() {
            let (bytes, recency) = dir_stats(&path);
            total += bytes;
            newest = newest.max(recency);
            continue;
        }
        if file_type.is_file() && entry.file_name().to_string_lossy().contains(".tmp-") {
            let _ = fs::remove_file(&path);
            continue;
        }
        // Symlinks (to files or directories) stat as themselves.
        if let Ok(meta) = entry.metadata() {
            total += meta.len();
            let atime = meta.accessed().unwrap_or(SystemTime::UNIX_EPOCH);
            let mtime = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
            newest = newest.max(atime).max(mtime);
        }
    }
    (total, newest)
}
