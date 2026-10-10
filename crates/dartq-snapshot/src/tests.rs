//! Contract pins: round-trip, generation mismatch, corruption tolerance,
//! LRU eviction, atomic writes, and cache-root resolution.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::{Duration, SystemTime};

use filetime::{FileTime, set_file_atime, set_file_mtime};
use serde::{Deserialize, Serialize};

use crate::{
    DEFAULT_CAPACITY_BYTES, Entry, EntryKey, GenerationHeader, RemapTables,
    SNAPSHOT_FORMAT_VERSION, SaveError, SnapshotConfig, SnapshotFileId, SnapshotStore,
    SnapshotStringId,
};

fn test_header() -> GenerationHeader {
    GenerationHeader {
        format_version: SNAPSHOT_FORMAT_VERSION,
        semantic_version: 3,
        sdk_identity: "dart-sdk 3.15.0 (vm)".to_string(),
        package_graph_fingerprint: [7; 16],
    }
}

fn key(content: u8, fingerprint: u64) -> EntryKey {
    EntryKey {
        content_hash: [content; 16],
        feature_fingerprint: fingerprint,
        semantic_version: 3,
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct TestPayload {
    unit: String,
    diagnostics: Vec<u32>,
}

fn payload(unit: &str) -> TestPayload {
    TestPayload {
        unit: unit.to_string(),
        diagnostics: vec![1, 2, 3],
    }
}

fn tables() -> RemapTables {
    RemapTables {
        strings: vec![
            (SnapshotStringId(0), "main".to_string()),
            (SnapshotStringId(1), "dart:core".to_string()),
        ],
        files: vec![(SnapshotFileId(0), "/w/lib/main.dart".into())],
    }
}

/// A throwaway cache root that cleans up after itself, even on failure.
struct TempCache {
    path: PathBuf,
}

static TEMP_SEQ: AtomicU32 = AtomicU32::new(0);

impl TempCache {
    fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "dartq-snapshot-{tag}-{}-{}",
            std::process::id(),
            TEMP_SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).expect("create temp cache root");
        Self { path }
    }

    fn store(&self, capacity_bytes: u64) -> SnapshotStore {
        SnapshotStore::open(SnapshotConfig {
            root: Some(self.path.clone()),
            capacity_bytes,
        })
        .expect("open store")
    }
}

impl Drop for TempCache {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn gen_dir(root: &Path, header: &GenerationHeader) -> PathBuf {
    root.join("snapshots").join(hex::encode(header.dir_id()))
}

fn entries_dir(root: &Path, header: &GenerationHeader) -> PathBuf {
    gen_dir(root, header).join("entries")
}

fn save_one_entry(cache: &TempCache) -> GenerationHeader {
    let header = test_header();
    cache
        .store(DEFAULT_CAPACITY_BYTES)
        .save(
            &header,
            &tables(),
            &[Entry {
                key: key(1, 10),
                payload: payload("main"),
            }],
        )
        .expect("save");
    header
}

/// `save_one_entry` plus a growing re-save: the generation claims both
/// keys and stays coherent — an entry set that only grows across re-saves
/// matches each new manifest exactly.
fn save_two_entries(cache: &TempCache) -> GenerationHeader {
    let header = save_one_entry(cache);
    cache
        .store(DEFAULT_CAPACITY_BYTES)
        .save(
            &header,
            &tables(),
            &[
                Entry {
                    key: key(1, 10),
                    payload: payload("main"),
                },
                Entry {
                    key: key(2, 10),
                    payload: payload("other"),
                },
            ],
        )
        .expect("save sibling");
    header
}

fn assert_generation_miss(mutate: fn(&mut GenerationHeader)) {
    let cache = TempCache::new("mismatch");
    save_one_entry(&cache);
    let mut other = test_header();
    mutate(&mut other);
    assert!(
        cache
            .store(DEFAULT_CAPACITY_BYTES)
            .load::<TestPayload>(&other)
            .is_none()
    );
}

#[test]
fn entry_key_id_mixes_every_component() {
    let base = key(1, 10);
    let fingerprint = EntryKey {
        feature_fingerprint: 11,
        ..base
    };
    let semantic = EntryKey {
        semantic_version: 4,
        ..base
    };
    let content = EntryKey {
        content_hash: [2; 16],
        ..base
    };
    assert_ne!(base.id(), fingerprint.id());
    assert_ne!(base.id(), semantic.id());
    assert_ne!(base.id(), content.id());
}

#[test]
fn round_trip_preserves_entries_and_tables() {
    let cache = TempCache::new("round-trip");
    let header = test_header();
    // Same content hash under two fingerprints: both must persist
    // distinctly — the key is the full triple.
    let saved = vec![
        Entry {
            key: key(1, 10),
            payload: payload("main"),
        },
        Entry {
            key: key(1, 20),
            payload: payload("other"),
        },
    ];
    cache
        .store(DEFAULT_CAPACITY_BYTES)
        .save(&header, &tables(), &saved)
        .expect("save");

    let reloaded = cache
        .store(DEFAULT_CAPACITY_BYTES)
        .load::<TestPayload>(&header)
        .expect("generation");
    assert_eq!(reloaded.header, header);
    assert_eq!(reloaded.tables, tables());

    let mut expected = saved.clone();
    expected.sort_by_key(|entry| entry.key.id());
    assert_eq!(reloaded.entries, expected);

    // Byte-exact round-trip: the stored JSON segment is exactly the
    // re-serialization of the loaded entry (the frame's checksummed body).
    let main = reloaded
        .entries
        .iter()
        .find(|e| e.key == saved[0].key)
        .expect("main entry");
    let framed = fs::read(
        entries_dir(&cache.path, &header).join(format!("{}.entry", hex::encode(main.key.id()))),
    )
    .expect("entry file");
    let body_at = framed.iter().position(|&b| b == b'\n').expect("framed") + 1;
    assert_eq!(
        &framed[body_at..],
        serde_json::to_vec(main).expect("serialize").as_slice()
    );
}

#[test]
fn empty_generation_loads_with_no_entries() {
    let cache = TempCache::new("empty");
    let header = test_header();
    cache
        .store(DEFAULT_CAPACITY_BYTES)
        .save::<TestPayload>(&header, &tables(), &[])
        .expect("save");
    let snapshot = cache
        .store(DEFAULT_CAPACITY_BYTES)
        .load::<TestPayload>(&header)
        .expect("generation");
    assert!(snapshot.entries.is_empty());
    assert_eq!(snapshot.tables, tables());
}

#[test]
fn generation_misses_on_format_version_mismatch() {
    assert_generation_miss(|h| h.format_version += 1);
}

#[test]
fn generation_misses_on_semantic_version_mismatch() {
    assert_generation_miss(|h| h.semantic_version += 1);
}

#[test]
fn generation_misses_on_sdk_identity_mismatch() {
    assert_generation_miss(|h| h.sdk_identity.push_str(" (other)"));
}

#[test]
fn generation_misses_on_package_graph_mismatch() {
    assert_generation_miss(|h| h.package_graph_fingerprint = [9; 16]);
}

/// A single corrupt entry demotes the whole generation: a manifest-named
/// entry that fails its checksum is a mismatch, never a smaller `Some`.
#[test]
fn corrupt_entry_demotes_the_generation() {
    let cache = TempCache::new("corrupt-entry");
    let header = save_two_entries(&cache);
    let victim =
        entries_dir(&cache.path, &header).join(format!("{}.entry", hex::encode(key(1, 10).id())));
    let mut bytes = fs::read(&victim).expect("read entry");
    let last = bytes.len() - 1;
    bytes[last] ^= 0xFF;
    fs::write(&victim, &bytes).expect("corrupt entry");

    assert!(
        cache
            .store(DEFAULT_CAPACITY_BYTES)
            .load::<TestPayload>(&header)
            .is_none()
    );
}

/// Truncation is corruption to the frame checksum: the same whole-generation
/// demotion.
#[test]
fn truncated_entry_demotes_the_generation() {
    let cache = TempCache::new("truncated-entry");
    let header = save_two_entries(&cache);
    let victim =
        entries_dir(&cache.path, &header).join(format!("{}.entry", hex::encode(key(1, 10).id())));
    let bytes = fs::read(&victim).expect("read entry");
    fs::write(&victim, &bytes[..bytes.len() / 2]).expect("truncate entry");

    assert!(
        cache
            .store(DEFAULT_CAPACITY_BYTES)
            .load::<TestPayload>(&header)
            .is_none()
    );
}

#[test]
fn damaged_tables_miss_the_whole_generation() {
    let cache = TempCache::new("corrupt-tables");
    let header = save_one_entry(&cache);
    let generation = gen_dir(&cache.path, &header);

    fs::write(generation.join("tables.json"), b"{oops").expect("corrupt tables");
    assert!(
        cache
            .store(DEFAULT_CAPACITY_BYTES)
            .load::<TestPayload>(&header)
            .is_none()
    );

    // A missing tables file (torn save) is equally a miss.
    fs::remove_file(generation.join("tables.json")).expect("remove tables");
    assert!(
        cache
            .store(DEFAULT_CAPACITY_BYTES)
            .load::<TestPayload>(&header)
            .is_none()
    );
}

#[test]
fn damaged_header_misses_the_whole_generation() {
    let cache = TempCache::new("corrupt-header");
    let header = save_one_entry(&cache);
    let generation = gen_dir(&cache.path, &header);

    fs::write(generation.join("header.json"), b"{").expect("corrupt header");
    assert!(
        cache
            .store(DEFAULT_CAPACITY_BYTES)
            .load::<TestPayload>(&header)
            .is_none()
    );

    // A well-formed header with a wrong field is the same miss.
    fs::write(
        generation.join("header.json"),
        br#"{"format_version":1,"semantic_version":999,"sdk_identity":"x","package_graph_fingerprint":[0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0]}"#,
    )
    .expect("rewrite header");
    assert!(
        cache
            .store(DEFAULT_CAPACITY_BYTES)
            .load::<TestPayload>(&header)
            .is_none()
    );
}

#[test]
fn save_rejects_foreign_format_versions() {
    let cache = TempCache::new("foreign-format");
    let mut foreign = test_header();
    foreign.format_version = SNAPSHOT_FORMAT_VERSION + 1;
    let error = cache
        .store(DEFAULT_CAPACITY_BYTES)
        .save::<TestPayload>(&foreign, &tables(), &[])
        .expect_err("rejected");
    assert!(matches!(error, SaveError::UnsupportedFormatVersion { .. }));
    assert!(!gen_dir(&cache.path, &foreign).exists(), "nothing written");
}

#[test]
fn save_is_atomic_and_temp_files_never_affect_loads() {
    let cache = TempCache::new("atomic");
    let header = test_header();
    let store = cache.store(DEFAULT_CAPACITY_BYTES);
    let main = key(1, 10);
    store
        .save(
            &header,
            &tables(),
            &[Entry {
                key: main,
                payload: payload("v1"),
            }],
        )
        .expect("save");

    for file in fs::read_dir(entries_dir(&cache.path, &header))
        .expect("entries dir")
        .flatten()
    {
        let name = file.file_name().to_string_lossy().into_owned();
        let hex_ok = name.len() == 38
            && Path::new(&name).extension() == Some(std::ffi::OsStr::new("entry"))
            && name[..32].bytes().all(|b| b.is_ascii_hexdigit());
        assert!(hex_ok, "unexpected file: {name}");
    }

    // Overwrite the same key: the rename replacement wins.
    store
        .save(
            &header,
            &tables(),
            &[Entry {
                key: main,
                payload: payload("v2"),
            }],
        )
        .expect("resave");
    let snapshot = store.load::<TestPayload>(&header).expect("generation");
    assert_eq!(snapshot.entries.len(), 1);
    assert_eq!(snapshot.entries[0].payload.unit, "v2");

    // A crashed writer's temp file and frame garbage are ignored, never fatal.
    let dir = entries_dir(&cache.path, &header);
    fs::write(dir.join("deadbeef.entry.tmp-1-1"), b"half-written").expect("plant temp");
    fs::write(dir.join("zz.entry"), b"not a frame").expect("plant junk");
    let snapshot = store.load::<TestPayload>(&header).expect("generation");
    assert_eq!(snapshot.entries.len(), 1);
    assert_eq!(snapshot.entries[0].payload.unit, "v2");
}

#[test]
fn capacity_evicts_the_least_recent_generation() {
    let cache = TempCache::new("lru");
    let header_a = test_header();
    let mut header_b = test_header();
    header_b.package_graph_fingerprint = [8; 16];
    let fat = TestPayload {
        unit: "x".repeat(4096),
        diagnostics: Vec::new(),
    };
    let store = cache.store(DEFAULT_CAPACITY_BYTES);
    store
        .save(
            &header_a,
            &tables(),
            &[Entry {
                key: key(1, 1),
                payload: fat.clone(),
            }],
        )
        .expect("save a");
    store
        .save(
            &header_b,
            &tables(),
            &[Entry {
                key: key(2, 1),
                payload: fat,
            }],
        )
        .expect("save b");
    let dir_a = gen_dir(&cache.path, &header_a);
    let dir_b = gen_dir(&cache.path, &header_b);
    assert!(dir_a.exists() && dir_b.exists());

    // Make A strictly more recent than B's fresh writes.
    let newer = FileTime::from_system_time(SystemTime::now() + Duration::from_mins(10));
    for file in files_under(&dir_a) {
        let _ = set_file_atime(&file, newer);
        let _ = set_file_mtime(&file, newer);
    }

    // One generation fits; the older one must go whole.
    let capacity = dir_size(&dir_a);
    cache.store(capacity).enforce_capacity();
    assert!(!dir_b.exists(), "least recent generation evicted");
    assert!(dir_a.exists(), "most recent generation survives");
    let snapshot = cache
        .store(capacity)
        .load::<TestPayload>(&header_a)
        .expect("generation");
    assert_eq!(snapshot.entries.len(), 1);
}

/// The termination invariant: a symlinked cycle inside a generation's tree
/// must not recurse without bound during the capacity scan — `dir_stats`
/// keys recursion on the entry's own file type and never resolves a link
/// (the pre-fix code followed `path.is_dir()` into the cycle and overflowed
/// the stack). A generation-level symlink is likewise neither counted nor
/// evicted. The test itself is the regression: unbounded recursion aborts
/// the process instead of returning.
#[cfg(unix)]
#[test]
fn symlink_cycle_terminates_the_capacity_scan() {
    let cache = TempCache::new("symlink-cycle");
    let header = save_one_entry(&cache);
    let entries = entries_dir(&cache.path, &header);
    std::os::unix::fs::symlink(&entries, entries.join("cycle")).expect("plant cycle");
    let snapshots = cache.path.join("snapshots");
    std::os::unix::fs::symlink(gen_dir(&cache.path, &header), snapshots.join("linked"))
        .expect("plant generation-level link");

    cache.store(DEFAULT_CAPACITY_BYTES).enforce_capacity();

    assert!(
        gen_dir(&cache.path, &header).exists(),
        "scan completed; the generation is intact"
    );
    assert!(
        fs::symlink_metadata(snapshots.join("linked")).is_ok(),
        "the generation-level link is never followed"
    );
}

/// The union-of-saves semantic is gone: entry files that exist outside the
/// manifest — a torn re-save's new entry, or the residue of a re-save
/// whose set changed — demote the generation to a strict miss, and nothing
/// ever sweeps the residue away. Abandoned files die with the generation
/// under LRU.
#[test]
fn partial_resave_is_a_strict_miss_never_a_union() {
    let cache = TempCache::new("partial-resave");
    let header = save_two_entries(&cache);
    let store = cache.store(DEFAULT_CAPACITY_BYTES);
    let dir = entries_dir(&cache.path, &header);
    let entry_file = |k: u8| dir.join(format!("{}.entry", hex::encode(key(k, 10).id())));

    // Torn re-save: v3's entry file lands, its manifest write never does.
    let v3 = Entry {
        key: key(3, 10),
        payload: payload("v3"),
    };
    fs::write(
        entry_file(3),
        crate::entry::encode(&v3).expect("encode v3"),
    )
    .expect("plant torn entry");
    assert!(
        store.load::<TestPayload>(&header).is_none(),
        "an entry outside the manifest is a miss"
    );

    // Completed re-save with a changed set: the manifest claims {3} while
    // the directory holds {1, 2, 3} — a miss, not a union.
    store
        .save(&header, &tables(), &[v3])
        .expect("subset re-save succeeds");
    assert!(
        store.load::<TestPayload>(&header).is_none(),
        "residue files demote the changed-set re-save"
    );
    for k in [1u8, 2, 3] {
        assert!(entry_file(k).exists(), "residue is never swept: {k}");
    }
}

/// An entry file missing from the claimed set — the eviction-vs-loader
/// race outcome — demotes the whole generation.
#[test]
fn missing_entry_demotes_the_generation() {
    let cache = TempCache::new("missing-entry");
    let header = save_two_entries(&cache);
    let victim =
        entries_dir(&cache.path, &header).join(format!("{}.entry", hex::encode(key(2, 10).id())));
    fs::remove_file(&victim).expect("remove entry");

    assert!(
        cache
            .store(DEFAULT_CAPACITY_BYTES)
            .load::<TestPayload>(&header)
            .is_none()
    );
}

/// The entries directory itself is part of the claim: even an empty
/// manifest requires its (empty) directory, so an eviction that raced a
/// saver cannot leave a valid-looking empty generation behind.
#[test]
fn missing_entries_directory_demotes_even_an_empty_generation() {
    let cache = TempCache::new("no-entries-dir");
    let header = test_header();
    cache
        .store(DEFAULT_CAPACITY_BYTES)
        .save::<TestPayload>(&header, &tables(), &[])
        .expect("save");
    fs::remove_dir_all(entries_dir(&cache.path, &header)).expect("remove entries dir");

    assert!(
        cache
            .store(DEFAULT_CAPACITY_BYTES)
            .load::<TestPayload>(&header)
            .is_none()
    );
}

/// An eviction racing a loader used to yield a partial `Some`; under the
/// manifest gate even a perfectly valid entry file outside the claimed
/// set demotes the whole generation.
#[test]
fn extra_entry_demotes_the_generation() {
    let cache = TempCache::new("extra-entry");
    let header = save_one_entry(&cache);
    let outsider = Entry {
        key: key(9, 10),
        payload: payload("planted"),
    };
    fs::write(
        entries_dir(&cache.path, &header).join(format!("{}.entry", hex::encode(key(9, 10).id()))),
        crate::entry::encode(&outsider).expect("encode outsider"),
    )
    .expect("plant valid entry");

    assert!(
        cache
            .store(DEFAULT_CAPACITY_BYTES)
            .load::<TestPayload>(&header)
            .is_none()
    );
}

/// F1's plain-IO-error window: a re-save that fails at the tables write
/// (its entries already replaced claimed files) leaves the previous
/// coherent generation loadable — same manifest, same file set — never a
/// mixture of unreadable halves. The surgery fails the tables write
/// without touching the previous `tables.json`: the generation directory
/// is made read-only so the tables temp file cannot be created, while the
/// entries directory stays writable and the claimed entry is replaced
/// first — the save order (entries → tables → header) then leaves the old
/// manifest facing exactly the claimed file set.
#[cfg(unix)]
#[test]
fn failed_tables_write_keeps_the_last_coherent_generation() {
    use std::os::unix::fs::PermissionsExt;

    let cache = TempCache::new("failed-tables");
    let header = save_two_entries(&cache);
    let generation = gen_dir(&cache.path, &header);

    // Block the tables write: no new file can be created in a read-only
    // generation directory. The previous tables.json survives untouched.
    let writable = fs::metadata(&generation)
        .expect("generation metadata")
        .permissions();
    fs::set_permissions(&generation, fs::Permissions::from_mode(writable.mode() & !0o200))
        .expect("block tables write");

    let store = cache.store(DEFAULT_CAPACITY_BYTES);
    let error = store
        .save(
            &header,
            &tables(),
            &[Entry {
                key: key(1, 10),
                payload: payload("v2"),
            }],
        )
        .expect_err("tables write fails");
    assert!(matches!(error, SaveError::Io(_)));

    fs::set_permissions(&generation, writable).expect("unblock tables write");
    let snapshot = store.load::<TestPayload>(&header).expect("coherent v1");
    assert_eq!(snapshot.entries.len(), 2);
    let main = snapshot
        .entries
        .iter()
        .find(|entry| entry.key == key(1, 10))
        .expect("claimed entry");
    assert_eq!(main.payload.unit, "v2", "the overwritten entry is the re-save's");
    assert!(snapshot.entries.iter().any(|entry| entry.key == key(2, 10)));
}

/// Recency beats fitting: the newest generation is never evicted, even
/// when the store cannot fit without it.
#[test]
fn enforce_never_evicts_the_newest_generation() {
    let cache = TempCache::new("protect-newest");
    let header_a = test_header();
    let mut header_b = test_header();
    header_b.package_graph_fingerprint = [8; 16];
    let fat = TestPayload {
        unit: "x".repeat(4096),
        diagnostics: Vec::new(),
    };
    let store = cache.store(DEFAULT_CAPACITY_BYTES);
    store
        .save(
            &header_a,
            &tables(),
            &[Entry {
                key: key(1, 1),
                payload: fat.clone(),
            }],
        )
        .expect("save a");
    store
        .save(
            &header_b,
            &tables(),
            &[Entry {
                key: key(2, 1),
                payload: fat,
            }],
        )
        .expect("save b");

    // Make A strictly older than B's fresh writes.
    let older = FileTime::from_system_time(SystemTime::now() - Duration::from_hours(1));
    for file in files_under(&gen_dir(&cache.path, &header_a)) {
        let _ = set_file_atime(&file, older);
        let _ = set_file_mtime(&file, older);
    }

    // Capacity smaller than either generation alone: A goes whole, B stays
    // even though the store remains over capacity. The survivor is intact
    // under the manifest gate — eviction never half-deletes a generation.
    cache.store(1).enforce_capacity();
    assert!(!gen_dir(&cache.path, &header_a).exists(), "older evicted");
    assert!(
        gen_dir(&cache.path, &header_b).exists(),
        "the newest generation survives over capacity"
    );
    let survivor = cache
        .store(1)
        .load::<TestPayload>(&header_b)
        .expect("survivor loads whole");
    assert_eq!(survivor.entries.len(), 1);
}

/// The enforce walk sweeps leaked temp files anywhere under the cache —
/// crash residue never accumulates, and real files are untouched.
#[test]
fn enforce_sweeps_leaked_temp_files() {
    let cache = TempCache::new("tmp-sweep");
    let header = save_one_entry(&cache);
    let generation = gen_dir(&cache.path, &header);
    let entries = entries_dir(&cache.path, &header);
    fs::write(generation.join("header.json.tmp-7-3"), b"torn").expect("plant temp");
    fs::write(entries.join("feedbeef.entry.tmp-7-4"), b"torn").expect("plant temp");

    cache.store(DEFAULT_CAPACITY_BYTES).enforce_capacity();

    assert!(!generation.join("header.json.tmp-7-3").exists());
    assert!(!entries.join("feedbeef.entry.tmp-7-4").exists());
    assert!(generation.join("header.json").exists(), "real files intact");
    assert_eq!(
        fs::read_dir(entries).expect("entries").count(),
        1,
        "one entry left"
    );
}

/// One load touches one marker: the generation gains a `recency` file —
/// the LRU handle the sweep reads — and its entries are never utimed
/// individually.
#[test]
fn load_touches_the_generation_recency_marker() {
    let cache = TempCache::new("recency");
    let header = save_one_entry(&cache);
    let marker = gen_dir(&cache.path, &header).join("recency");
    let entry =
        entries_dir(&cache.path, &header).join(format!("{}.entry", hex::encode(key(1, 10).id())));
    assert!(!marker.exists(), "saving never touches the marker");

    // Park the entry's times in the past: the load must leave them there.
    let past = FileTime::from_system_time(SystemTime::now() - Duration::from_mins(5));
    let _ = set_file_atime(&entry, past);
    let _ = set_file_mtime(&entry, past);

    cache
        .store(DEFAULT_CAPACITY_BYTES)
        .load::<TestPayload>(&header)
        .expect("load");

    assert!(marker.exists(), "the load created the marker");
    let accessed = fs::metadata(&entry)
        .expect("entry metadata")
        .accessed()
        .expect("atime");
    assert_eq!(
        FileTime::from_system_time(accessed),
        past,
        "entries are not utimed individually"
    );
}

fn files_under(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        for entry in fs::read_dir(&current).expect("read dir").flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else {
                files.push(path);
            }
        }
    }
    files
}

fn dir_size(dir: &Path) -> u64 {
    files_under(dir)
        .into_iter()
        .filter_map(|path| fs::metadata(&path).ok())
        .map(|meta| meta.len())
        .sum()
}

fn env_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .expect("env lock")
}

#[test]
fn xdg_cache_home_selects_the_cache_root() {
    let _guard = env_lock();
    let base = TempCache::new("xdg-set");
    // SAFETY: tests run single-threaded; env access serialized by env_lock.
    unsafe { std::env::set_var("XDG_CACHE_HOME", &base.path) };
    let store = SnapshotStore::open(SnapshotConfig::default()).expect("open");
    assert_eq!(store.root(), base.path.join("dartiq"));
}

#[test]
fn unset_xdg_cache_home_falls_back_to_home() {
    let _guard = env_lock();
    let base = TempCache::new("xdg-home");
    // SAFETY: tests run single-threaded; env access serialized by env_lock.
    unsafe { std::env::remove_var("XDG_CACHE_HOME") };
    unsafe { std::env::set_var("HOME", &base.path) };
    let store = SnapshotStore::open(SnapshotConfig::default()).expect("open");
    assert_eq!(store.root(), base.path.join(".cache").join("dartiq"));
}

#[test]
fn relative_xdg_cache_home_is_ignored() {
    let _guard = env_lock();
    let base = TempCache::new("xdg-relative");
    // SAFETY: tests run single-threaded; env access serialized by env_lock.
    unsafe { std::env::set_var("XDG_CACHE_HOME", "relative/cache") };
    unsafe { std::env::set_var("HOME", &base.path) };
    let store = SnapshotStore::open(SnapshotConfig::default()).expect("open");
    assert_eq!(store.root(), base.path.join(".cache").join("dartiq"));
}
