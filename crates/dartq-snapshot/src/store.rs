//! The on-disk store: XDG-rooted generations, atomic writes, LRU capacity.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::SystemTime;

use filetime::{FileTime, set_file_atime, set_file_mtime};
use serde::Serialize;
use serde::de::DeserializeOwned;
use thiserror::Error;

use crate::entry::{self, Entry};
use crate::header::{GenerationHeader, SNAPSHOT_FORMAT_VERSION};
use crate::remap::RemapTables;

/// Default L2 capacity: 10 GiB (PRD §17.2 `--cache-size-limit`).
pub const DEFAULT_CAPACITY_BYTES: u64 = 10 * 1024 * 1024 * 1024;

/// How many saves may pass before the capacity scan runs regardless of the
/// size estimate — the bound on how far the cheap estimate can drift from
/// reality (and on how long stale entries survive between sweeps).
const ENFORCE_EVERY_SAVES: u32 = 32;

/// The per-generation load-recency marker: one file touched once per load
/// stands in for per-entry utimes.
const RECENCY_MARKER: &str = "recency";

/// Uniqueness suffix for temporary files; rename targets never collide
/// across concurrent writers in one process.
static TMP_SEQ: AtomicU32 = AtomicU32::new(0);

/// Store construction parameters. `root: None` resolves the XDG default
/// (`$XDG_CACHE_HOME/dartiq`, falling back to `$HOME/.cache/dartiq`).
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

/// One loaded generation: the matched header, the remap slices, and every
/// entry that survived integrity checks (corrupt entries are dropped, not
/// reported).
#[derive(Clone, Debug)]
pub struct Snapshot<T> {
    pub header: GenerationHeader,
    pub tables: RemapTables,
    pub entries: Vec<Entry<T>>,
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
    /// Saves since the last full scan; [`ENFORCE_EVERY_SAVES`] forces one.
    saves_since_enforce: AtomicU32,
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
        })
    }

    /// The cache root this store reads and writes.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Loads the generation matching `expected`, or `None` on any mismatch
    /// or damage to the header/remap tables. Entries that fail their
    /// checksum or JSON decode are skipped individually.
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
        let tables: RemapTables = parse(&fs::read(generation.join("tables.json")).ok()?)?;

        let mut entries = Vec::new();
        if let Ok(dir) = fs::read_dir(generation.join("entries")) {
            for file in dir.flatten() {
                let path = file.path();
                if path.extension() != Some(std::ffi::OsStr::new("entry")) {
                    continue;
                }
                let Ok(bytes) = fs::read(&path) else { continue };
                let Some(decoded) = entry::decode::<T>(&bytes) else {
                    continue;
                };
                entries.push(decoded);
            }
        }
        entries.sort_by_key(|entry| entry.key.id());
        touch_recency(&generation.join(RECENCY_MARKER));
        Some(Snapshot {
            header: stored,
            tables,
            entries,
        })
    }

    /// Writes a generation: entries first, then remap tables, then the
    /// header — a torn save leaves a headerless directory that loads as a
    /// miss. Re-saving an existing generation overwrites files in place;
    /// every file replacement is atomic.
    ///
    /// Capacity and staleness are throttled, not per-save: the bytes just
    /// written accrue into a cheap estimate, and only when the estimate
    /// crosses `capacity_bytes` or [`ENFORCE_EVERY_SAVES`] saves have
    /// passed does the save pay for the full scan ([`Self::enforce_capacity`])
    /// followed by sweeping this generation's stale entries (files of
    /// content states no longer in the written set).
    ///
    /// # Errors
    ///
    /// A header claiming a foreign `format_version` (nothing is written),
    /// or an IO/serialization failure (earlier files of the generation
    /// remain valid).
    pub fn save<T: Serialize>(
        &self,
        header: &GenerationHeader,
        tables: &RemapTables,
        entries: &[Entry<T>],
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
        let mut written = 0u64;
        let mut ids = Vec::with_capacity(entries.len());
        for entry in entries {
            let id = entry.key.id();
            let framed = entry::encode(entry)?;
            written += framed.len() as u64;
            atomic_write(
                &entries_dir.join(format!("{}.entry", hex::encode(id))),
                &framed,
            )?;
            ids.push(id);
        }
        let encoded_tables = serde_json::to_vec(tables)?;
        written += encoded_tables.len() as u64;
        atomic_write(&generation.join("tables.json"), &encoded_tables)?;
        let encoded_header = serde_json::to_vec(header)?;
        written += encoded_header.len() as u64;
        atomic_write(&generation.join("header.json"), &encoded_header)?;

        let estimate = self.estimated_bytes.fetch_add(written, Ordering::Relaxed) + written;
        let saves = self.saves_since_enforce.fetch_add(1, Ordering::Relaxed) + 1;
        if estimate > self.capacity_bytes || saves >= ENFORCE_EVERY_SAVES {
            self.enforce_capacity();
            ids.sort_unstable();
            sweep_stale_entries(&entries_dir, &ids);
        }
        Ok(())
    }

    /// Evicts whole generations, oldest access first, until the total size
    /// fits `capacity_bytes` — except the newest generation, which is the
    /// one just written or just loaded and is never evicted even when the
    /// store stays over capacity (recency beats fitting). Group eviction
    /// keeps a generation either present or absent — never half-valid
    /// (PRD §17.2). The walk also removes leaked temp files (crash
    /// residue) as a side pass and re-anchors the estimate `save` accrues.
    /// Best-effort: unreadable files are skipped, never reported.
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
            for (path, bytes, _) in &generations[..evictable] {
                if total <= self.capacity_bytes {
                    break;
                }
                if fs::remove_dir_all(path).is_ok() {
                    total -= bytes;
                }
            }
        }
        self.estimated_bytes.store(total, Ordering::Relaxed);
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

/// Removes entry files whose ids are not in the sorted `ids` set — the
/// stale content states earlier saves of this generation left behind. One
/// `read_dir`, no per-name allocation (the hex id decodes into a stack
/// array). Temp and junk names never parse as entries and are left to
/// [`dir_stats`]'s temp sweep or untouched. Best-effort: a failed removal
/// keeps a valid-but-unmatched entry, which loads skip anyway.
fn sweep_stale_entries(entries_dir: &Path, ids: &[[u8; 16]]) {
    let Ok(dir) = fs::read_dir(entries_dir) else {
        return;
    };
    for file in dir.flatten() {
        let file_name = file.file_name();
        let Some(stem) = file_name.to_str() else {
            continue;
        };
        let Some(stem) = stem.strip_suffix(".entry") else {
            continue;
        };
        if stem.len() != 32 {
            continue;
        }
        let mut id = [0u8; 16];
        if hex::decode_to_slice(stem, &mut id).is_err() {
            continue;
        }
        if ids.binary_search(&id).is_err() {
            let _ = fs::remove_file(file.path());
        }
    }
}

/// `$XDG_CACHE_HOME/dartiq`, or `$HOME/.cache/dartiq` when unset, empty, or
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
    Ok(base.join("dartiq"))
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
