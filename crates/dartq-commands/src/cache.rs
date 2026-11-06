//! The cache management face: `dartq cache stats|gc`, one aggregate over
//! every on-disk cache dartq keeps.
//!
//! Three stores answer it, each through its own machinery:
//!
//! * `bcache` — the Flutter build cache ([`dartq_bcache`]);
//!   `dartq bcache stats|gc` stays its per-face spelling.
//! * `snapshot` — the L2 facts snapshots ([`dartq_snapshot`]): one
//!   directory per generation, evicted whole.
//! * `content` — the engine's L3 content store ([`dartq_engine::store`]):
//!   source blobs sharded by digest.
//!
//! The session registry shares the cache root but holds runtime state, so
//! it is an informational row only: outside every total, never touched.
//!
//! `stats` is read-only on every face, which rules out the obvious calls:
//! `CasStore::open` creates its store, `ContentStore::open` evicts and
//! rewrites its books, and `Registry::list` sweeps dead records. `gc` runs
//! each store's own eviction at its resolved cap — the governing
//! `dartq.yaml`'s `cache.size_limit_gib` for the two analysis stores, each
//! store's default without one — so a store grown under a larger
//! configured cap is no longer trimmed by a defaults-assuming pass. The
//! content face then sweeps the debris its eviction can never see back
//! (#4053): blobs on disk no manifest row references, and torn `<id>.tmp`
//! writes, each only past the store's age guard (one hour of mtime age —
//! a concurrent writer's in-flight files are never touched). The
//! engine's `--cache-size-limit` flag governs only while the engine owns
//! the store (the analyze and daemon faces arm it per run); this
//! maintenance face cannot observe a run's flag and enforces the
//! configured number. The build cache sits outside `cache.size_limit_gib`:
//! its `build_cache.budget_gb` is its own knob. A store nothing ever wrote
//! stays unwritten under both.
//!
//! A face that fails answers with an `error` row and the others still
//! report: one torn manifest must not hide the rest of the cache.

use std::ffi::OsStr;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use dartq_bcache::orchestrator::ShimConfig;
use dartq_bcache::{GcReport, StoreStats};
use dartq_engine::store::{
    ContentStore, DEFAULT_SIZE_LIMIT, StoreConfig, SweepReport, default_root,
};
use dartq_snapshot::{
    DEFAULT_CAPACITY_BYTES, EvictionReport, SnapshotConfig, SnapshotStats, SnapshotStore,
};
use dartq_source::ContentId;
use serde::Serialize;

use crate::bcache::{bcache_gc, bcache_stats, default_shim_config, shim_config};
use crate::config::ConfigError;

/// The content store's directory under the cache root.
const CONTENT_DIR: &str = "store";
/// The session registry's directory under the cache root.
const SESSIONS_DIR: &str = "sessions";
/// Why a face under the cache root cannot exist.
const NO_CACHE_ROOT: &str = "no cache root: neither XDG_CACHE_HOME nor HOME is set";

/// Where each store lives and the cap its `gc` enforces.
#[derive(Debug, Clone)]
pub struct CacheConfig {
    /// The build cache.
    pub bcache: ShimConfig,
    /// The facts snapshot store.
    pub snapshot: SnapshotConfig,
    /// The content store; `None` with no home directory to root it in.
    pub content: Option<StoreConfig>,
}

impl CacheConfig {
    /// The stores this machine runs: the build cache as the `dartq.yaml`
    /// governing `dir` configures it, the snapshot and content stores at
    /// the default roots with their caps resolved from that same governing
    /// config — its `cache.size_limit_gib` when set, else each store's own
    /// default (the zero-config behavior, byte-identical). The build cache
    /// keeps its `build_cache.budget_gb`, outside the shared cap. Each
    /// store resolves its own root, so a row names where that store really
    /// is.
    ///
    /// # Errors
    ///
    /// [`ConfigError`] as [`crate::config::discover`] reports it.
    pub fn discover(dir: &Path) -> Result<Self, ConfigError> {
        let governing = crate::config::discover(dir)?;
        let limit = governing
            .as_ref()
            .and_then(|config| config.cache)
            .map(|cache| cache.size_limit_gib.saturating_mul(1024 * 1024 * 1024));
        Ok(Self {
            bcache: governing
                .as_ref()
                .and_then(|config| config.build_cache.as_ref())
                .map(shim_config)
                .unwrap_or_else(default_shim_config),
            snapshot: SnapshotConfig {
                root: None,
                capacity_bytes: limit.unwrap_or(DEFAULT_CAPACITY_BYTES),
            },
            content: default_root().map(|root| StoreConfig {
                root,
                size_limit: limit.unwrap_or(DEFAULT_SIZE_LIMIT),
            }),
        })
    }
}

/// One aggregate answer: a row per face, the total last.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CacheReport<R> {
    /// The rows, in print order.
    pub rows: Vec<R>,
    /// How many faces answered with an `error` row.
    pub failed: u64,
}

impl<R: Serialize> CacheReport<R> {
    /// Writes one JSON object per row, one per line.
    ///
    /// # Errors
    ///
    /// The sink's write failure, or a root that is not valid UTF-8.
    pub fn write_lines(&self, out: &mut impl Write) -> io::Result<()> {
        for row in &self.rows {
            serde_json::to_writer(&mut *out, row)?;
            out.write_all(b"\n")?;
        }
        Ok(())
    }
}

/// One `cache stats` line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "face", rename_all = "lowercase")]
pub enum StatsRow {
    /// The build cache: `entries` are cache keys, the bytes are blobs. Its
    /// cap is the `build_cache.budget_gb` knob's budget, not the shared
    /// `cache.size_limit_gib`.
    Bcache {
        root: PathBuf,
        cap_bytes: u64,
        #[serde(flatten)]
        stats: StoreStats,
    },
    /// The snapshot store: `entries` are per-file facts.
    Snapshot {
        root: PathBuf,
        /// The capacity `gc` enforces: `cache.size_limit_gib` when the
        /// governing config sets it, else the store's own default.
        cap_bytes: u64,
        #[serde(flatten)]
        stats: SnapshotStats,
    },
    /// The content store: `entries` are blobs, counted off the disk.
    Content {
        root: PathBuf,
        /// The byte cap `gc` enforces: `cache.size_limit_gib` when the
        /// governing config sets it, else the store's own default.
        cap_bytes: u64,
        entries: u64,
        bytes: u64,
    },
    /// The session registry, for the record: `cache` is always `false`.
    Sessions {
        root: PathBuf,
        cache: bool,
        records: u64,
    },
    /// The sums over the cache faces that answered.
    Total {
        entries: u64,
        bytes: u64,
        failed: u64,
    },
    /// A face that could not answer.
    #[serde(untagged)]
    Failed { face: &'static str, error: String },
}

impl StatsRow {
    /// What this row adds to the total: `(entries, bytes)`.
    fn usage(&self) -> (u64, u64) {
        match self {
            Self::Bcache { stats, .. } => (stats.entries, stats.blob_bytes),
            Self::Snapshot { stats, .. } => (stats.entries, stats.bytes),
            Self::Content { entries, bytes, .. } => (*entries, *bytes),
            Self::Sessions { .. } | Self::Total { .. } | Self::Failed { .. } => (0, 0),
        }
    }
}

/// One `cache gc` line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "face", rename_all = "lowercase")]
pub enum GcRow {
    /// The build cache's budget/LRU pass.
    Bcache {
        root: PathBuf,
        #[serde(flatten)]
        report: GcReport,
    },
    /// The snapshot store's capacity pass.
    Snapshot {
        root: PathBuf,
        #[serde(flatten)]
        report: EvictionReport,
    },
    /// The content store's cap pass plus its debris sweep, as the net
    /// change on disk.
    Content {
        root: PathBuf,
        entries_before: u64,
        /// Cap-pass evictions; the sweep's off-books removals count in
        /// `off_books_swept`, not here.
        entries_evicted: u64,
        /// Total disk bytes reclaimed: the blob walk's net delta (cap
        /// evictions plus swept off-books blobs) and the `.tmp` debris
        /// bytes alike.
        bytes_freed: u64,
        /// Blobs removed that no manifest row referenced (crash debris),
        /// past the store's sweep grace.
        off_books_swept: u64,
        /// `<id>.tmp` torn-write debris removed, past the same grace.
        tmp_swept: u64,
        /// Bytes that `.tmp` debris occupied — invisible to the blob walk,
        /// already part of `bytes_freed`.
        tmp_bytes_freed: u64,
    },
    /// The sum over the faces that answered.
    Total { bytes_freed: u64, failed: u64 },
    /// A face that could not answer.
    #[serde(untagged)]
    Failed { face: &'static str, error: String },
}

impl GcRow {
    /// What this row adds to the total.
    fn bytes_freed(&self) -> u64 {
        match self {
            Self::Bcache { report, .. } => report.bytes_freed,
            Self::Snapshot { report, .. } => report.bytes_freed,
            Self::Content { bytes_freed, .. } => *bytes_freed,
            Self::Total { .. } | Self::Failed { .. } => 0,
        }
    }
}

/// `dartq cache stats`: every face's usage, then the total. Read-only.
#[must_use]
pub fn stats(config: &CacheConfig) -> CacheReport<StatsRow> {
    let content = config.content.as_ref();
    let mut failed = 0;
    let mut rows: Vec<StatsRow> = [
        ("bcache", bcache_row(&config.bcache)),
        ("snapshot", snapshot_row(&config.snapshot)),
        ("content", content_row(content)),
    ]
    .into_iter()
    .map(|(face, row)| {
        row.unwrap_or_else(|error| {
            failed += 1;
            StatsRow::Failed { face, error }
        })
    })
    .collect();
    let (entries, bytes) = rows
        .iter()
        .map(StatsRow::usage)
        .fold((0, 0), |sum, usage| (sum.0 + usage.0, sum.1 + usage.1));
    if let Some(content) = content {
        rows.push(sessions_row(&content.root));
    }
    rows.push(StatsRow::Total {
        entries,
        bytes,
        failed,
    });
    CacheReport { rows, failed }
}

/// `dartq cache gc`: every store's own eviction pass, then the total.
pub fn gc(config: &CacheConfig) -> CacheReport<GcRow> {
    let mut failed = 0;
    let mut rows: Vec<GcRow> = [
        ("bcache", bcache_gc_row(&config.bcache)),
        ("snapshot", snapshot_gc_row(&config.snapshot)),
        ("content", content_gc_row(config.content.as_ref())),
    ]
    .into_iter()
    .map(|(face, row)| {
        row.unwrap_or_else(|error| {
            failed += 1;
            GcRow::Failed { face, error }
        })
    })
    .collect();
    let bytes_freed: u64 = rows.iter().map(GcRow::bytes_freed).sum();
    rows.push(GcRow::Total {
        bytes_freed,
        failed,
    });
    CacheReport { rows, failed }
}

fn bcache_row(config: &ShimConfig) -> Result<StatsRow, String> {
    let stats = if config.store_root.exists() {
        bcache_stats(config).map_err(|error| error.to_string())?
    } else {
        StoreStats::default()
    };
    Ok(StatsRow::Bcache {
        root: config.store_root.clone(),
        cap_bytes: config.budget_bytes,
        stats,
    })
}

fn bcache_gc_row(config: &ShimConfig) -> Result<GcRow, String> {
    let report = if config.store_root.exists() {
        bcache_gc(config).map_err(|error| error.to_string())?
    } else {
        GcReport::default()
    };
    Ok(GcRow::Bcache {
        root: config.store_root.clone(),
        report,
    })
}

fn snapshot_row(config: &SnapshotConfig) -> Result<StatsRow, String> {
    let store = SnapshotStore::open(config.clone()).map_err(|error| error.to_string())?;
    Ok(StatsRow::Snapshot {
        root: store.generations_dir(),
        cap_bytes: config.capacity_bytes,
        stats: store.stats(),
    })
}

fn snapshot_gc_row(config: &SnapshotConfig) -> Result<GcRow, String> {
    let store = SnapshotStore::open(config.clone()).map_err(|error| error.to_string())?;
    Ok(GcRow::Snapshot {
        root: store.generations_dir(),
        report: store.enforce_capacity(),
    })
}

fn content_row(config: Option<&StoreConfig>) -> Result<StatsRow, String> {
    let config = config.ok_or(NO_CACHE_ROOT)?;
    let root = config.root.join(CONTENT_DIR);
    let (entries, bytes) = content_usage(&root);
    Ok(StatsRow::Content {
        root,
        cap_bytes: config.size_limit,
        entries,
        bytes,
    })
}

/// The content store has no eviction call of its own: `open` is the cap
/// pass — load the books, evict unpinned entries oldest-touch first down
/// to the cap, compact — the one every analyze run pays at startup. `gc`
/// then sweeps the debris eviction can never see back (off-books blobs
/// and torn `.tmp` writes past the store's grace). The row is the net
/// change on disk, so a writer racing the pass can only shrink it.
fn content_gc_row(config: Option<&StoreConfig>) -> Result<GcRow, String> {
    let config = config.ok_or(NO_CACHE_ROOT)?;
    let root = config.root.join(CONTENT_DIR);
    let (entries_before, bytes_before) = content_usage(&root);
    let mut swept = SweepReport::default();
    if root.exists() {
        let store = ContentStore::open(config).map_err(|error| error.to_string())?;
        swept = store.sweep_debris();
    }
    let (entries_after, bytes_after) = content_usage(&root);
    let removed = entries_before.saturating_sub(entries_after);
    Ok(GcRow::Content {
        root,
        entries_before,
        entries_evicted: removed.saturating_sub(swept.blobs_swept),
        bytes_freed: bytes_before.saturating_sub(bytes_after) + swept.tmp_bytes_freed,
        off_books_swept: swept.blobs_swept,
        tmp_swept: swept.tmp_swept,
        tmp_bytes_freed: swept.tmp_bytes_freed,
    })
}

/// The blobs under the content store directory `dir` as `(count, bytes)`:
/// the walk the store's books rebuild runs, without the store — every
/// well-named regular file one shard deep, so the books and `.tmp` debris
/// never count. What cannot be read counts as absent.
fn content_usage(dir: &Path) -> (u64, u64) {
    let (mut entries, mut bytes) = (0, 0);
    let shards = fs::read_dir(dir).into_iter().flatten().flatten();
    for shard in shards.filter(|shard| shard.file_type().is_ok_and(|kind| kind.is_dir())) {
        for blob in fs::read_dir(shard.path()).into_iter().flatten().flatten() {
            let named = blob
                .file_name()
                .to_str()
                .is_some_and(|name| ContentId::parse_hex(name).is_ok());
            if named
                && let Ok(meta) = blob.metadata()
                && meta.is_file()
            {
                entries += 1;
                bytes += meta.len();
            }
        }
    }
    (entries, bytes)
}

/// The registry's records on disk, live or dead: a count of file names, so
/// nothing is opened and nothing swept.
fn sessions_row(cache_root: &Path) -> StatsRow {
    let root = cache_root.join(SESSIONS_DIR);
    let records = fs::read_dir(&root).map_or(0, |files| {
        files
            .flatten()
            .filter(|file| file.path().extension() == Some(OsStr::new("json")))
            .count() as u64
    });
    StatsRow::Sessions {
        root,
        cache: false,
        records,
    }
}

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use dartq_bcache::{BlobId, CasStore, blob_id_hex};
    use dartq_snapshot::{
        DEFAULT_CAPACITY_BYTES, Entry, EntryKey, GenerationHeader, RemapTables,
        SNAPSHOT_FORMAT_VERSION,
    };
    use dartq_source::content_hash;
    use filetime::{FileTime, set_file_atime, set_file_mtime};

    use super::*;
    use crate::bcache::shim_config;
    use crate::config::BuildCacheConfig;

    /// A throwaway cache root, removed even when the test fails.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(tag: &str) -> Self {
            Self::under(&std::env::temp_dir(), &format!("dq_cmd_cache_{tag}"))
        }

        /// The ticket's fixture base — a literal test-fixture scratch path,
        /// created and consumed by these tests alone.
        fn ticket(tag: &str) -> Self {
            Self::under(Path::new("/tmp/dartq-cache-4052"), tag)
        }

        fn under(base: &Path, tag: &str) -> Self {
            let root = base.join(format!(
                "{tag}_{}_{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .expect("clock")
                    .as_nanos()
            ));
            fs::create_dir_all(&root).expect("scratch dir");
            Self(root)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// Every store under `root`, at the default caps.
    fn config_at(root: &Path) -> CacheConfig {
        CacheConfig {
            bcache: shim_config(&BuildCacheConfig {
                root: Some(root.join("cas")),
                ..BuildCacheConfig::default()
            }),
            snapshot: SnapshotConfig {
                root: Some(root.to_path_buf()),
                capacity_bytes: DEFAULT_CAPACITY_BYTES,
            },
            content: Some(StoreConfig {
                root: root.to_path_buf(),
                size_limit: DEFAULT_SIZE_LIMIT,
            }),
        }
    }

    /// One blob no manifest references, aged past the store's in-flight
    /// window: the shape a bcache gc pass always reclaims.
    fn plant_orphan_blob(config: &CacheConfig, bytes: &[u8]) {
        let store = CasStore::open(&config.bcache.store_root).expect("open cas");
        let path = store.blob_path(&BlobId([1; 32]));
        fs::create_dir_all(path.parent().expect("shard")).expect("shard dir");
        fs::write(&path, bytes).expect("blob");
        set_file_mtime(&path, FileTime::from_unix_time(1, 0)).expect("age blob");
    }

    /// One generation of `entries` entries, distinct per `salt`.
    fn save_generation(config: &CacheConfig, salt: u8, entries: u8) {
        let store = SnapshotStore::open(config.snapshot.clone()).expect("open snapshots");
        let header = GenerationHeader {
            format_version: SNAPSHOT_FORMAT_VERSION,
            semantic_version: 1,
            sdk_identity: "sdk".to_owned(),
            package_graph_fingerprint: [salt; 16],
            context_salt: [0; 16],
        };
        let rows: Vec<Entry<String>> = (0..entries)
            .map(|n| Entry {
                key: EntryKey {
                    content_hash: [n; 16],
                    feature_fingerprint: 0,
                    semantic_version: 1,
                },
                payload: "facts".to_owned(),
            })
            .collect();
        store
            .save(&header, &RemapTables::default(), &rows)
            .expect("save generation");
    }

    fn put_contents(config: &CacheConfig, payloads: &[&[u8]]) {
        let config = config.content.as_ref().expect("content config");
        let mut store = ContentStore::open(config).expect("open content store");
        for payload in payloads {
            store.put(payload);
        }
        store.flush();
    }

    /// Every file under `dir` with its size and mtime, sorted.
    fn tree(dir: &Path) -> Vec<(PathBuf, u64, SystemTime)> {
        let mut files = Vec::new();
        let mut pending = vec![dir.to_path_buf()];
        while let Some(current) = pending.pop() {
            for entry in fs::read_dir(&current).expect("read dir").flatten() {
                let meta = entry.metadata().expect("metadata");
                if meta.is_dir() {
                    pending.push(entry.path());
                } else {
                    let mtime = meta.modified().expect("mtime");
                    files.push((entry.path(), meta.len(), mtime));
                }
            }
        }
        files.sort();
        files
    }

    /// One row per face in print order, the session line outside the sums.
    #[test]
    fn stats_reports_every_face_and_sums_the_total() {
        let scratch = Scratch::new("stats");
        let config = config_at(&scratch.0);
        plant_orphan_blob(&config, b"12345");
        save_generation(&config, 1, 2);
        put_contents(&config, &[b"aaa", b"bbbb"]);

        let report = stats(&config);

        assert_eq!(report.failed, 0);
        let [bcache, snapshot, content, sessions, total] = report.rows.as_slice() else {
            panic!("five rows: {:?}", report.rows);
        };
        assert_eq!(
            bcache,
            &StatsRow::Bcache {
                root: config.bcache.store_root.clone(),
                cap_bytes: config.bcache.budget_bytes,
                stats: StoreStats {
                    blobs: 1,
                    blob_bytes: 5,
                    ..StoreStats::default()
                },
            }
        );
        let StatsRow::Snapshot {
            root,
            cap_bytes,
            stats,
        } = snapshot
        else {
            panic!("snapshot row: {snapshot:?}");
        };
        assert_eq!(root, &scratch.0.join("snapshots"));
        assert_eq!(*cap_bytes, DEFAULT_CAPACITY_BYTES);
        assert_eq!((stats.generations, stats.entries), (1, 2));
        assert_eq!(
            content,
            &StatsRow::Content {
                root: scratch.0.join("store"),
                cap_bytes: DEFAULT_SIZE_LIMIT,
                entries: 2,
                bytes: 7,
            },
            "the books beside the shards are not blobs"
        );
        assert_eq!(
            sessions,
            &StatsRow::Sessions {
                root: scratch.0.join("sessions"),
                cache: false,
                records: 0,
            }
        );
        assert_eq!(
            total,
            &StatsRow::Total {
                entries: 4,
                bytes: 5 + stats.bytes + 7,
                failed: 0,
            }
        );
    }

    /// The build cache's key count joins the total, not its blob count.
    #[test]
    fn bcache_adds_its_keys_and_blob_bytes_to_the_total() {
        let row = StatsRow::Bcache {
            root: PathBuf::new(),
            cap_bytes: 20,
            stats: StoreStats {
                entries: 2,
                blobs: 9,
                blob_bytes: 3,
                oldest_hit_unix: 4,
                corrupt_manifests: 0,
            },
        };
        assert_eq!(row.usage(), (2, 3));
    }

    /// Stats is the read-only face even over stores past their caps: the
    /// content store's `open`, the obvious call, would evict right here.
    #[test]
    fn stats_changes_nothing_on_disk() {
        let scratch = Scratch::new("read-only");
        let mut config = config_at(&scratch.0);
        plant_orphan_blob(&config, b"12345");
        save_generation(&config, 1, 1);
        save_generation(&config, 2, 1);
        put_contents(&config, &[b"aaa", b"bbbb"]);
        config.snapshot.capacity_bytes = 1;
        config.content.as_mut().expect("content").size_limit = 1;
        let before = tree(&scratch.0);

        let report = stats(&config);

        assert_eq!(report.failed, 0);
        assert_eq!(tree(&scratch.0), before);
    }

    /// Neither face conjures a store on a machine that never had one:
    /// `CasStore::open` and `ContentStore::open` both would.
    #[test]
    fn a_fresh_machine_stays_empty() {
        let scratch = Scratch::new("fresh");
        let config = config_at(&scratch.0);

        let usage = stats(&config);
        let freed = gc(&config);

        assert_eq!(
            usage.rows.last(),
            Some(&StatsRow::Total {
                entries: 0,
                bytes: 0,
                failed: 0,
            })
        );
        assert_eq!(
            freed.rows.last(),
            Some(&GcRow::Total {
                bytes_freed: 0,
                failed: 0,
            })
        );
        assert_eq!(fs::read_dir(&scratch.0).expect("read").count(), 0);
    }

    /// Session records are counted by name and survive both faces — a
    /// malformed one is exactly what the registry's own listing deletes.
    #[test]
    fn session_records_are_counted_and_never_touched() {
        let scratch = Scratch::new("sessions");
        let sessions = scratch.0.join("sessions");
        fs::create_dir_all(sessions.join("sockets")).expect("sessions dir");
        fs::write(sessions.join("dead.json"), b"not a record").expect("record");
        let config = config_at(&scratch.0);

        let report = stats(&config);
        let _ = gc(&config);

        assert!(report.rows.contains(&StatsRow::Sessions {
            root: sessions.clone(),
            cache: false,
            records: 1,
        }));
        assert_eq!(
            fs::read(sessions.join("dead.json")).expect("record survives"),
            b"not a record"
        );
    }

    /// Each store evicts by its own rule and the total sums what they
    /// freed: the bcache orphan goes, the snapshot store keeps its newest
    /// generation, the content store comes down to its cap.
    #[test]
    fn gc_runs_each_store_own_eviction() {
        let scratch = Scratch::new("gc");
        let mut config = config_at(&scratch.0);
        plant_orphan_blob(&config, b"12345");
        save_generation(&config, 1, 1);
        save_generation(&config, 2, 1);
        put_contents(&config, &[&[b'a'; 100], &[b'b'; 100], &[b'c'; 100]]);
        config.snapshot.capacity_bytes = 1;
        config.content.as_mut().expect("content").size_limit = 150;

        let report = gc(&config);

        assert_eq!(report.failed, 0);
        let [bcache, snapshot, content, total] = report.rows.as_slice() else {
            panic!("four rows: {:?}", report.rows);
        };
        assert_eq!(
            bcache,
            &GcRow::Bcache {
                root: config.bcache.store_root.clone(),
                report: GcReport {
                    blobs_removed: 1,
                    bytes_freed: 5,
                    ..GcReport::default()
                },
            }
        );
        let GcRow::Snapshot {
            report: evicted, ..
        } = snapshot
        else {
            panic!("snapshot row: {snapshot:?}");
        };
        assert_eq!(
            (evicted.generations_before, evicted.generations_evicted),
            (2, 1)
        );
        assert!(evicted.bytes_freed > 0);
        assert_eq!(
            content,
            &GcRow::Content {
                root: scratch.0.join("store"),
                entries_before: 3,
                entries_evicted: 2,
                bytes_freed: 200,
                off_books_swept: 0,
                tmp_swept: 0,
                tmp_bytes_freed: 0,
            }
        );
        assert_eq!(
            total,
            &GcRow::Total {
                bytes_freed: 5 + evicted.bytes_freed + 200,
                failed: 0,
            }
        );
    }

    /// Stores within their caps keep every entry.
    #[test]
    fn gc_within_the_caps_frees_nothing() {
        let scratch = Scratch::new("gc-noop");
        let config = config_at(&scratch.0);
        save_generation(&config, 1, 1);
        put_contents(&config, &[b"aaa", b"bbbb"]);

        let report = gc(&config);

        assert_eq!(report.failed, 0);
        assert!(report.rows.contains(&GcRow::Snapshot {
            root: scratch.0.join("snapshots"),
            report: EvictionReport {
                generations_before: 1,
                generations_evicted: 0,
                bytes_freed: 0,
            },
        }));
        assert!(report.rows.contains(&GcRow::Content {
            root: scratch.0.join("store"),
            entries_before: 2,
            entries_evicted: 0,
            bytes_freed: 0,
            off_books_swept: 0,
            tmp_swept: 0,
            tmp_bytes_freed: 0,
        }));
        assert_eq!(
            report.rows.last(),
            Some(&GcRow::Total {
                bytes_freed: 0,
                failed: 0,
            })
        );
    }

    /// The content face's gc sweeps debris the books can never see back
    /// (#4053): an aged off-books blob and aged `.tmp` debris are reclaimed
    /// and reported in the row's additive fields — never as evictions —
    /// while young debris (a possible in-flight write) stays. The fixture
    /// base is the ticket's literal scratch path.
    #[test]
    fn gc_sweeps_content_debris_off_the_books() {
        let scratch = Scratch::under(Path::new("/tmp/dartq-cache-4053"), "debris");
        let config = config_at(&scratch.0);
        put_contents(&config, &[b"booked"]);
        let store_dir = scratch.0.join("store");
        let tmp_bytes = b"half-written".len() as u64;

        let orphan = content_hash(b"off the books");
        let orphan_hex = orphan.to_string();
        let orphan_path = store_dir.join(&orphan_hex[..2]).join(&orphan_hex);
        plant_blob(&store_dir, &orphan, 4096, true);
        let in_flight = content_hash(b"in flight");
        let fresh_hex = in_flight.to_string();
        let fresh_path = store_dir.join(&fresh_hex[..2]).join(&fresh_hex);
        plant_blob(&store_dir, &in_flight, 4096, false);
        let torn_hex = content_hash(b"torn write").to_string();
        let torn = store_dir.join(&torn_hex[..2]).join(format!("{torn_hex}.tmp"));
        fs::create_dir_all(torn.parent().expect("shard")).expect("torn shard");
        fs::write(&torn, b"half-written").expect("torn tmp");
        set_file_mtime(&torn, FileTime::from_unix_time(0, 0)).expect("age tmp");

        let report = gc(&config);

        assert_eq!(report.failed, 0);
        assert_eq!(
            report
                .rows
                .iter()
                .find(|row| matches!(row, GcRow::Content { .. })),
            Some(&GcRow::Content {
                root: store_dir.clone(),
                entries_before: 3,
                entries_evicted: 0,
                bytes_freed: 4096 + tmp_bytes,
                off_books_swept: 1,
                tmp_swept: 1,
                tmp_bytes_freed: tmp_bytes,
            }),
            "the sweep's removals report as themselves, not as evictions"
        );
        assert!(!orphan_path.exists() && !torn.exists());
        assert!(
            fresh_path.is_file(),
            "young debris (a possible in-flight write) survives the sweep"
        );
        assert_eq!(
            report.rows.last(),
            Some(&GcRow::Total {
                bytes_freed: 4096 + tmp_bytes,
                failed: 0,
            }),
            "the tmp bytes join the total's freed sum"
        );
    }

    /// A torn manifest no longer fails the build cache's stats — it is
    /// counted in the row's `corrupt_manifests` and the face keeps
    /// reporting (one bad file must not blind the budget); `gc` purges
    /// the damage.
    #[test]
    fn a_failed_stats_face_leaves_the_others_reporting() {
        let scratch = Scratch::new("failed-stats");
        let config = config_at(&scratch.0);
        save_generation(&config, 1, 1);
        let store = CasStore::open(&config.bcache.store_root).expect("open cas");
        let manifest = store.entry_path(&blob_id_hex(&BlobId([9; 32])));
        fs::create_dir_all(manifest.parent().expect("entry dir")).expect("entry dir");
        fs::write(&manifest, b"{torn").expect("manifest");

        let report = stats(&config);

        assert_eq!(report.failed, 0);
        assert!(matches!(
            &report.rows[0],
            StatsRow::Bcache { stats, .. }
                if stats.corrupt_manifests == 1 && stats.entries == 0
        ));
        assert!(matches!(&report.rows[1], StatsRow::Snapshot { .. }));
        assert!(matches!(
            report.rows.last(),
            Some(StatsRow::Total { failed: 0, .. })
        ));

        assert_eq!(gc(&config).failed, 0);
        assert_eq!(stats(&config).failed, 0);
        assert!(matches!(
            &stats(&config).rows[0],
            StatsRow::Bcache { stats, .. } if stats.corrupt_manifests == 0
        ));
    }

    /// A store that cannot open fails its own gc row alone.
    #[test]
    fn a_failed_gc_face_leaves_the_others_reporting() {
        let scratch = Scratch::new("failed-gc");
        let config = config_at(&scratch.0);
        save_generation(&config, 1, 1);
        fs::write(scratch.0.join("store"), b"squatting").expect("plant file");

        let report = gc(&config);

        assert_eq!(report.failed, 1);
        assert!(matches!(&report.rows[1], GcRow::Snapshot { .. }));
        assert!(matches!(
            &report.rows[2],
            GcRow::Failed {
                face: "content",
                ..
            }
        ));
        assert!(matches!(
            report.rows.last(),
            Some(GcRow::Total { failed: 1, .. })
        ));
    }

    /// With no root to live under, the content face fails on both faces
    /// and the registry, which shares that root, has no row.
    #[test]
    fn no_cache_root_fails_the_content_face() {
        let scratch = Scratch::new("no-root");
        let mut config = config_at(&scratch.0);
        config.content = None;

        let usage = stats(&config);
        let freed = gc(&config);

        assert_eq!((usage.failed, freed.failed), (1, 1));
        assert!(usage.rows.contains(&StatsRow::Failed {
            face: "content",
            error: NO_CACHE_ROOT.to_owned(),
        }));
        assert!(
            !usage
                .rows
                .iter()
                .any(|row| matches!(row, StatsRow::Sessions { .. }))
        );
        assert!(freed.rows.contains(&GcRow::Failed {
            face: "content",
            error: NO_CACHE_ROOT.to_owned(),
        }));
    }

    /// The build cache follows the governing `dartq.yaml`, a malformed one
    /// stays the usage failure it is everywhere, and the snapshot root
    /// stays the store's own to resolve. The root is a literal fixture:
    /// discovery never touches it.
    #[test]
    fn discover_reads_the_build_cache_from_the_governing_config() {
        let scratch = Scratch::new("discover");
        let yaml = scratch.0.join("dartq.yaml");
        fs::write(&yaml, "build_cache:\n  root: /fixture/cas\n").expect("config");

        let config = CacheConfig::discover(&scratch.0).expect("discover");

        assert_eq!(config.bcache.store_root, PathBuf::from("/fixture/cas/v2"));
        assert_eq!(config.snapshot.root, None);

        fs::write(&yaml, "build_cache:\n  typo: 1\n").expect("config");
        assert!(matches!(
            CacheConfig::discover(&scratch.0),
            Err(ConfigError::Invalid(_))
        ));
    }

    /// The shared cap resolves through the governing config — found by
    /// walking up from a nested directory — and overrides both analysis
    /// stores' defaults, while the build cache keeps its own
    /// `build_cache.budget_gb` outside it. The roots are literal fixtures.
    #[test]
    fn discover_resolves_the_shared_cap_over_the_defaults() {
        let scratch = Scratch::ticket("discover-cap");
        fs::create_dir_all(scratch.0.join("nested/deep")).expect("dirs");
        fs::write(
            scratch.0.join("dartq.yaml"),
            "cache:\n  size_limit_gib: 2\nbuild_cache:\n  root: /fixture/cas\n  budget_gb: 3\n",
        )
        .expect("config");

        let config = CacheConfig::discover(&scratch.0.join("nested/deep")).expect("discover");

        assert_eq!(config.snapshot.capacity_bytes, 2 << 30);
        assert_eq!(
            config.content.as_ref().expect("content").size_limit,
            2 << 30
        );
        assert_eq!(
            config.bcache.budget_bytes,
            3 << 30,
            "the build cache keeps its own knob, outside the shared cap"
        );
        assert_eq!(config.snapshot.root, None);
    }

    /// No `cache:` section, no config at all: every store keeps its own
    /// default — the zero-config contract, byte-identical to before the
    /// knob existed.
    #[test]
    fn discover_without_the_cache_section_keeps_store_defaults() {
        let scratch = Scratch::ticket("discover-defaults");
        let config = CacheConfig::discover(&scratch.0).expect("no config file");
        assert_eq!(config.snapshot.capacity_bytes, DEFAULT_CAPACITY_BYTES);
        assert_eq!(
            config.content.as_ref().expect("content").size_limit,
            DEFAULT_SIZE_LIMIT
        );

        fs::write(
            scratch.0.join("dartq.yaml"),
            "build_cache:\n  root: /fixture/cas\n",
        )
        .expect("config");
        let config = CacheConfig::discover(&scratch.0).expect("config without the section");
        assert_eq!(config.snapshot.capacity_bytes, DEFAULT_CAPACITY_BYTES);
        assert_eq!(
            config.content.as_ref().expect("content").size_limit,
            DEFAULT_SIZE_LIMIT
        );
    }

    /// The configured cap governs `gc`, not the store defaults: fixture
    /// stores grown past `cache.size_limit_gib` — but far under the 10 GiB
    /// defaults, where neither pass would evict a thing — come down to the
    /// configured number on both analysis faces, and `stats` names the
    /// governing cap on each row. Sizes ride sparse files (`set_len`): the
    /// stores account by file size, so the cap math is the real path
    /// without materializing gigabytes.
    #[test]
    fn gc_enforces_the_configured_cap_on_fixture_stores() {
        let scratch = Scratch::ticket("gc-configured");
        fs::write(&scratch.0.join("dartq.yaml"), "cache:\n  size_limit_gib: 1\n")
            .expect("config");
        let discovered = CacheConfig::discover(&scratch.0).expect("discover");
        let cap = 1 << 30;
        assert_eq!(discovered.snapshot.capacity_bytes, cap);
        assert_eq!(
            discovered.content.as_ref().expect("content").size_limit,
            cap
        );

        // Fixture stores at the ticket base, carrying the discovered caps.
        let mut config = config_at(&scratch.0);
        config.snapshot.capacity_bytes = discovered.snapshot.capacity_bytes;
        let content_cap = discovered.content.as_ref().expect("content").size_limit;
        config.content.as_mut().expect("content").size_limit = content_cap;

        // The snapshot store grown past the cap: an aged generation
        // inflated past 1 GiB (the LRU victim) beside a fresh small one.
        save_generation(&config, 1, 1);
        let snapshots = scratch.0.join("snapshots");
        let victim = fs::read_dir(&snapshots)
            .expect("generations")
            .flatten()
            .map(|entry| entry.path())
            .find(|path| path.is_dir())
            .expect("the first generation on disk");
        inflate_and_age(&victim, 2 << 30);
        save_generation(&config, 2, 1);

        // The content store ditto: two sparse blobs, 1.2 GiB together, the
        // aged one the eviction victim.
        let store_dir = scratch.0.join("store");
        plant_blob(&store_dir, &content_hash(b"ticket-a"), 600 << 20, true);
        plant_blob(&store_dir, &content_hash(b"ticket-b"), 600 << 20, false);

        let before = stats(&config);
        assert_eq!(before.failed, 0);
        let snapshot = before
            .rows
            .iter()
            .find_map(|row| match row {
                StatsRow::Snapshot { cap_bytes, stats, .. } if *cap_bytes == cap => Some(*stats),
                _ => None,
            })
            .expect("snapshot row at the configured cap");
        assert_eq!((snapshot.generations, snapshot.entries), (2, 2));
        assert!(snapshot.bytes > cap, "grown past the configured cap");
        assert_eq!(
            before.rows.contains(&StatsRow::Content {
                root: store_dir.clone(),
                cap_bytes: cap,
                entries: 2,
                bytes: 2 * (600 << 20),
            }),
            true,
            "the stats row names the governing cap: {:?}",
            before.rows
        );

        let report = gc(&config);
        assert_eq!(report.failed, 0);
        let evicted = report
            .rows
            .iter()
            .find_map(|row| match row {
                GcRow::Snapshot { report, .. } => Some(*report),
                _ => None,
            })
            .expect("snapshot gc row");
        assert_eq!(
            (evicted.generations_before, evicted.generations_evicted),
            (2, 1),
            "the aged generation is the victim"
        );
        assert!(evicted.bytes_freed > cap);
        assert_eq!(
            report.rows.contains(&GcRow::Content {
                root: store_dir.clone(),
                entries_before: 2,
                entries_evicted: 1,
                bytes_freed: 600 << 20,
                off_books_swept: 0,
                tmp_swept: 0,
                tmp_bytes_freed: 0,
            }),
            true,
            "the content pass evicted per the configured cap: {:?}",
            report.rows
        );

        let after = stats(&config);
        assert_eq!(after.failed, 0);
        let snapshot = after
            .rows
            .iter()
            .find_map(|row| match row {
                StatsRow::Snapshot { stats, .. } => Some(*stats),
                _ => None,
            })
            .expect("snapshot row after gc");
        assert_eq!(snapshot.generations, 1);
        assert!(snapshot.bytes <= cap, "down to the configured cap");
        assert_eq!(
            after.rows.contains(&StatsRow::Content {
                root: store_dir.clone(),
                cap_bytes: cap,
                entries: 1,
                bytes: 600 << 20,
            }),
            true,
            "the surviving blob is under the configured cap: {:?}",
            after.rows
        );
    }

    /// Inflates one file under `dir` to `bytes` (a sparse extent — the
    /// stores account by file size) and ages every file to the epoch,
    /// making `dir` the oldest thing an LRU pass sees.
    fn inflate_and_age(dir: &Path, bytes: u64) {
        fn walk(dir: &Path, files: &mut Vec<PathBuf>) {
            for entry in fs::read_dir(dir).expect("walk").flatten() {
                if entry.file_type().expect("type").is_dir() {
                    walk(&entry.path(), files);
                } else {
                    files.push(entry.path());
                }
            }
        }
        let mut files = Vec::new();
        walk(dir, &mut files);
        fs::OpenOptions::new()
            .write(true)
            .open(&files[0])
            .expect("open for inflate")
            .set_len(bytes)
            .expect("sparse size");
        let epoch = FileTime::from_unix_time(0, 0);
        for file in &files {
            set_file_atime(file, epoch).expect("age atime");
            set_file_mtime(file, epoch).expect("age mtime");
        }
    }

    /// One sparse blob under the content store's shard layout:
    /// digest-named, `bytes` long, aged to the epoch when `aged` — planted
    /// without the store, so its open rebuilds the books from disk and the
    /// eviction rides the rebuilt touches.
    fn plant_blob(store_dir: &Path, id: &ContentId, bytes: u64, aged: bool) {
        let hex = id.to_string();
        let path = store_dir.join(&hex[..2]).join(&hex);
        fs::create_dir_all(path.parent().expect("shard")).expect("shard dir");
        fs::OpenOptions::new()
            .write(true)
            .create(true)
            .open(&path)
            .expect("blob file")
            .set_len(bytes)
            .expect("sparse size");
        if aged {
            let epoch = FileTime::from_unix_time(0, 0);
            set_file_atime(&path, epoch).expect("age atime");
            set_file_mtime(&path, epoch).expect("age mtime");
        }
    }

    /// The machine shape, byte for byte: `face` leads every row, a store's
    /// own fields follow flat, and a failed face is just its name and
    /// error. The roots are literal fixtures.
    #[test]
    fn rows_print_one_json_object_per_line() {
        fn lines<R: Serialize>(rows: Vec<R>) -> String {
            let mut out = Vec::new();
            CacheReport { rows, failed: 0 }
                .write_lines(&mut out)
                .expect("write");
            String::from_utf8(out).expect("utf-8")
        }

        assert_eq!(
            lines(vec![
                StatsRow::Bcache {
                    root: PathBuf::from("/fixture/cas/v2"),
                    cap_bytes: 7,
                    stats: StoreStats {
                        entries: 1,
                        blobs: 2,
                        blob_bytes: 3,
                        oldest_hit_unix: 4,
                        corrupt_manifests: 0,
                    },
                },
                StatsRow::Snapshot {
                    root: PathBuf::from("/fixture/snapshots"),
                    cap_bytes: 8,
                    stats: SnapshotStats {
                        generations: 1,
                        entries: 2,
                        bytes: 3,
                    },
                },
                StatsRow::Content {
                    root: PathBuf::from("/fixture/store"),
                    cap_bytes: 9,
                    entries: 1,
                    bytes: 2,
                },
                StatsRow::Sessions {
                    root: PathBuf::from("/fixture/sessions"),
                    cache: false,
                    records: 1,
                },
                StatsRow::Failed {
                    face: "bcache",
                    error: "torn".to_owned(),
                },
                StatsRow::Total {
                    entries: 3,
                    bytes: 5,
                    failed: 1,
                },
            ]),
            concat!(
                r#"{"face":"bcache","root":"/fixture/cas/v2","cap_bytes":7,"entries":1,"blobs":2,"blob_bytes":3,"oldest_hit_unix":4,"corrupt_manifests":0}"#,
                "\n",
                r#"{"face":"snapshot","root":"/fixture/snapshots","cap_bytes":8,"generations":1,"entries":2,"bytes":3}"#,
                "\n",
                r#"{"face":"content","root":"/fixture/store","cap_bytes":9,"entries":1,"bytes":2}"#,
                "\n",
                r#"{"face":"sessions","root":"/fixture/sessions","cache":false,"records":1}"#,
                "\n",
                r#"{"face":"bcache","error":"torn"}"#,
                "\n",
                r#"{"face":"total","entries":3,"bytes":5,"failed":1}"#,
                "\n",
            )
        );
        assert_eq!(
            lines(vec![
                GcRow::Bcache {
                    root: PathBuf::from("/fixture/cas/v2"),
                    report: GcReport {
                        entries_before: 1,
                        entries_evicted: 2,
                        blobs_removed: 3,
                        bytes_freed: 4,
                        corrupt_manifests: 0,
                    },
                },
                GcRow::Snapshot {
                    root: PathBuf::from("/fixture/snapshots"),
                    report: EvictionReport {
                        generations_before: 1,
                        generations_evicted: 2,
                        bytes_freed: 3,
                    },
                },
                GcRow::Content {
                    root: PathBuf::from("/fixture/store"),
                    entries_before: 1,
                    entries_evicted: 2,
                    bytes_freed: 3,
                    off_books_swept: 4,
                    tmp_swept: 5,
                    tmp_bytes_freed: 6,
                },
                GcRow::Failed {
                    face: "content",
                    error: "squatted".to_owned(),
                },
                GcRow::Total {
                    bytes_freed: 7,
                    failed: 1,
                },
            ]),
            concat!(
                r#"{"face":"bcache","root":"/fixture/cas/v2","entries_before":1,"entries_evicted":2,"blobs_removed":3,"bytes_freed":4,"corrupt_manifests":0}"#,
                "\n",
                r#"{"face":"snapshot","root":"/fixture/snapshots","generations_before":1,"generations_evicted":2,"bytes_freed":3}"#,
                "\n",
                r#"{"face":"content","root":"/fixture/store","entries_before":1,"entries_evicted":2,"bytes_freed":3,"off_books_swept":4,"tmp_swept":5,"tmp_bytes_freed":6}"#,
                "\n",
                r#"{"face":"content","error":"squatted"}"#,
                "\n",
                r#"{"face":"total","bytes_freed":7,"failed":1}"#,
                "\n",
            )
        );
    }
}
