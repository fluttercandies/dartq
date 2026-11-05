//! CAS entry → project-tree materialization: turn a store hit into a
//! native flutter_tools skip.
//!
//! A cached flutter build entry is replayed into the target worktree's
//! `.dart_tool/flutter_build/<hash-dir>/` so that flutter's
//! content-hash validation finds every target up-to-date and never
//! invokes frontend_server. Payload blobs are placed with
//! copy-on-write clones (measured on APFS: 4.4 GB in 0.379 s; plain
//! copies are orders slower), never hardlinks — flutter_tools rewrites
//! some outputs in place and a hardlink would corrupt the shared
//! store. The skip only survives if the bookkeeping files
//! (stamp/`.filecache`/depfile/outputs) carry the TARGET tree's
//! absolute paths, so every byte occurrence of the ingest-tree root is
//! replaced with the target-tree root before the file lands.
//!
//! Performance targets (measured on APFS): a 4.4 GB entry materializes
//! in under 1.5 s; the rewrite pass stays under 50 ms for typical
//! stamp/`.filecache` sets; peak memory stays under 100 MB for any
//! entry size because payloads are streamed clone/copy operations and
//! only rewrite files are loaded, one at a time, under a 16 MiB bound.
//!
//! Crash atomicity: individual file writes are rename-atomic (write to
//! a temp sibling, rename into place), so no reader ever sees a
//! half-written file. A crash mid-materialize leaves a partial hash
//! dir, which is safe by construction — flutter_tools validates
//! content hashes, never mtimes, and treats wrong-content output as a
//! miss and rebuilds. No staging dance is built on top.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use memchr::memmem;

use crate::error::{BcacheError, Result};
use crate::fsutil::{clone_file, file_blake3, CloneStatus};
use crate::store::CasStore;
use crate::types::{blob_id_hex, ManifestFile, RewriteKind};

/// Hard per-file bound for rewrite (`PathText`) files: 16 MiB.
/// Bookkeeping files are KBs; payload files (dills, kernels) are
/// `RewriteKind::None` and stay streamed, so the bound never bites in
/// practice — it exists so a mislabeled manifest cannot turn the
/// rewriter into a whole-file loader.
const REWRITE_LIMIT: usize = 16 * 1024 * 1024;

/// One directory of a materialization: its target-tree name (recomputed
/// by the caller per the entry's [`crate::NameRule`]) and the entry files
/// that land inside it.
pub struct DirPlan {
    /// Target-tree hash dir NAME. Single path component, validated.
    pub name: String,
    /// Files of the dir, relative to it.
    pub files: Vec<ManifestFile>,
}

/// Everything needed to replay one cached entry into a target tree.
pub struct MaterializePlan {
    /// Every dir of the entry, in the caller's order (the orchestrator
    /// keeps the manifest's name-sorted order).
    pub dirs: Vec<DirPlan>,
    /// Ingest-tree absolute root: the byte string whose occurrences in
    /// bookkeeping files are rewritten away.
    pub old_root: String,
    /// Target-tree absolute root: the replacement byte string.
    pub new_root: PathBuf,
}

/// Counts and sizes of one materialization run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MaterializeReport {
    /// Files placed as copy-on-write clones.
    pub cloned: u64,
    /// Files placed by the streaming-copy fallback.
    pub copied: u64,
    /// Bookkeeping files in which at least one path was rewritten.
    pub rewritten_files: u64,
    /// Total non-overlapping root-path replacements performed.
    pub rewritten_occurrences: u64,
    /// True when a pre-existing hash dir was renamed aside instead of
    /// deleted.
    pub replaced_stale_dir: bool,
    /// Final on-disk bytes of the materialized entry.
    pub total_bytes: u64,
}

/// Replace every non-overlapping left-to-right occurrence of `old` in
/// `data` with `new`. When the lengths match the patch is applied
/// in-place (no reallocation); otherwise the output buffer is rebuilt.
/// Returns the number of replacements. An empty needle matches nothing.
pub fn rewrite_bytes(data: &mut Vec<u8>, old: &[u8], new: &[u8]) -> usize {
    if old.is_empty() {
        return 0;
    }
    let finder = memmem::Finder::new(old);
    if new.len() == old.len() {
        let mut count = 0usize;
        let mut cursor = 0usize;
        while let Some(pos) = finder.find(&data[cursor..]) {
            let at = cursor + pos;
            data[at..at + new.len()].copy_from_slice(new);
            cursor = at + old.len();
            count += 1;
        }
        count
    } else {
        // Count first so the output buffer is allocated at its exact
        // final size, then walk the matches once more to build it.
        let mut count = 0usize;
        let mut cursor = 0usize;
        while let Some(pos) = finder.find(&data[cursor..]) {
            cursor += pos + old.len();
            count += 1;
        }
        if count == 0 {
            return 0;
        }
        let cap = data.len() + count * new.len() - count * old.len();
        let mut out = Vec::with_capacity(cap);
        let mut cursor = 0usize;
        while let Some(pos) = finder.find(&data[cursor..]) {
            let at = cursor + pos;
            out.extend_from_slice(&data[cursor..at]);
            out.extend_from_slice(new);
            cursor = at + old.len();
        }
        out.extend_from_slice(&data[cursor..]);
        debug_assert_eq!(out.len(), cap);
        *data = out;
        count
    }
}

/// Materialize `plan` into `<project_root>/.dart_tool/flutter_build/`:
/// one dir per [`DirPlan`], each at its recorded target-tree name.
///
/// A pre-existing target dir is renamed aside to
/// `<name>.bcache-stale-<unix_millis>` (user data is never deleted),
/// independently per dir. Every blob is identity-verified (streamed
/// blake3) before placement; a mismatch aborts with
/// [`BcacheError::Corrupt`] naming the blob — a wrong artifact is worse
/// than a cache miss, and the partial hash dir left behind is a safe
/// miss for flutter_tools. Bookkeeping files get their embedded
/// ingest-tree paths rewritten to target-tree paths before the run
/// reports success; without that rewrite the skip would not survive the
/// tree move.
pub fn materialize(
    store: &CasStore,
    plan: &MaterializePlan,
    project_root: &Path,
) -> Result<MaterializeReport> {
    validate_plan(plan)?;

    let build_root = project_root.join(".dart_tool").join("flutter_build");

    let mut report = MaterializeReport {
        cloned: 0,
        copied: 0,
        rewritten_files: 0,
        rewritten_occurrences: 0,
        replaced_stale_dir: false,
        total_bytes: 0,
    };

    for dir in &plan.dirs {
        let hash_dir = build_root.join(&dir.name);
        if hash_dir.exists() {
            let stale = build_root.join(format!("{}.bcache-stale-{}", dir.name, unix_millis()));
            fs::rename(&hash_dir, &stale)?;
            report.replaced_stale_dir = true;
        }
        fs::create_dir_all(&hash_dir)?;
        for file in &dir.files {
            materialize_one(store, plan, file, &hash_dir, &mut report)?;
        }
    }
    Ok(report)
}

/// Places one manifest file into the hash dir.
fn materialize_one(
    store: &CasStore,
    plan: &MaterializePlan,
    file: &ManifestFile,
    hash_dir: &Path,
    report: &mut MaterializeReport,
) -> Result<()> {
    let src = store.blob_path(&file.blob);

    // Identity check before any placement: a corrupted store blob must
    // abort the run rather than land in a project tree.
    let digest = file_blake3(&src).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => BcacheError::NotFound(format!(
            "blob {} for {} is missing from the store",
            blob_id_hex(&file.blob),
            file.path.display()
        )),
        _ => BcacheError::Io(e),
    })?;
    if digest != file.blob {
        // Quarantine by deletion: a blob that failed its identity check
        // is corrupt on disk, and ingest leaves a same-size stored blob
        // alone — removing it is what lets the next ingest re-store the
        // true bytes.
        let _ = fs::remove_file(&src);
        return Err(BcacheError::Corrupt(format!(
            "blob {} for {} failed its content check",
            blob_id_hex(&file.blob),
            file.path.display()
        )));
    }

    let dst = hash_dir.join(&file.path);
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent)?;
    }

    // Rename-atomic placement: clone into a temp sibling, then rename,
    // so no reader sees a half-written file. A leftover temp from a
    // crashed run is cleared by clone_file's EEXIST handling.
    let tmp = tmp_sibling(&dst);
    match clone_file(&src, &tmp)? {
        CloneStatus::Cloned => report.cloned += 1,
        CloneStatus::Copied => report.copied += 1,
    }
    fs::rename(&tmp, &dst)?;

    // Rewrite before the exec bit: the rewrite writes a fresh file via
    // tmp+rename, which would discard permissions set earlier.
    if matches!(file.rewrite, RewriteKind::PathText) {
        let new_bytes = plan.new_root.as_os_str().as_encoded_bytes();
        let occurrences = rewrite_bookkeeping(&dst, plan.old_root.as_bytes(), new_bytes)?;
        if occurrences > 0 {
            report.rewritten_files += 1;
            report.rewritten_occurrences += occurrences as u64;
        }
    }

    if file.executable {
        set_executable(&dst)?;
    }

    report.total_bytes += fs::metadata(&dst)?.len();
    Ok(())
}

/// Rewrites old-root bytes to new-root bytes in one bookkeeping file.
/// The file is fully loaded under [`REWRITE_LIMIT`], patched in memory,
/// and written back via tmp+rename in the same directory. Returns the
/// number of replacements; zero means the file already carried
/// target-tree paths and is left untouched on disk.
fn rewrite_bookkeeping(path: &Path, old: &[u8], new: &[u8]) -> Result<usize> {
    let mut file = fs::File::open(path)?;
    let mut data = Vec::new();
    // take() bounds the load even if the file changes size mid-run: a
    // file larger than the limit reads as limit+1 bytes and is refused.
    (&mut file)
        .take(REWRITE_LIMIT as u64 + 1)
        .read_to_end(&mut data)?;
    if data.len() > REWRITE_LIMIT {
        return Err(BcacheError::Unsupported(format!(
            "bookkeeping file {} is {} bytes; the path rewriter supports at most {}",
            path.display(),
            data.len(),
            REWRITE_LIMIT
        )));
    }
    let occurrences = rewrite_bytes(&mut data, old, new);
    if occurrences > 0 {
        let tmp = tmp_sibling(path);
        fs::write(&tmp, &data)?;
        fs::rename(&tmp, path)?;
    }
    Ok(occurrences)
}

/// Plan validation runs before any filesystem effect: a rejected plan
/// leaves the project tree untouched.
///
/// v1 limitation, deliberate: roots are rewritten as plain path bytes.
/// If `old_root` contains a backslash, a quote, or any control byte —
/// i.e. anything that would also appear JSON-escaped inside bookkeeping
/// files — the plan is rejected as [`BcacheError::Invalid`]. Matching
/// escaped variants needs a decoding rewriter, which is out of scope
/// for v1.
fn validate_plan(plan: &MaterializePlan) -> Result<()> {
    for dir in &plan.dirs {
        let name = &dir.name;
        if name.is_empty() || name == "." || name == ".." || name.contains('/') || name.contains('\\')
        {
            return Err(BcacheError::Invalid(format!(
                "flutter hash dir name {name:?} is not a single path component"
            )));
        }
        for file in &dir.files {
            if file.path.is_absolute()
                || file
                    .path
                    .components()
                    .any(|c| matches!(c, std::path::Component::ParentDir))
            {
                return Err(BcacheError::Invalid(format!(
                    "manifest path {} escapes the hash dir",
                    file.path.display()
                )));
            }
        }
    }
    if plan.old_root.is_empty() {
        return Err(BcacheError::Invalid("old_root must not be empty".into()));
    }
    if plan
        .old_root
        .as_bytes()
        .iter()
        .any(|&b| b == b'\\' || b == b'"' || b < 0x20)
    {
        return Err(BcacheError::Invalid(format!(
            "old_root {:?} needs JSON string escaping; v1 rewrites plain paths only",
            plan.old_root
        )));
    }
    Ok(())
}

/// Forces mode 0o755 for executable-flagged outputs. Blobs carry no
/// permission bits, so there is nothing to preserve: the flag is the
/// entire mode policy, and non-flagged files keep the clone/copy
/// default. Non-unix targets have no mode bit to set and ignore the
/// flag.
fn set_executable(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o755))?;
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
    Ok(())
}

/// Temporary sibling name for rename-atomic writes: a leading dot keeps
/// crash leftovers out of flutter's glob-based scans, and the embedded
/// file name keeps temp names unique per target file.
fn tmp_sibling(path: &Path) -> PathBuf {
    let name = path
        .file_name()
        .map(|n| format!(".{}.dartq-tmp", n.to_string_lossy()))
        .unwrap_or_else(|| ".dartq-tmp".to_string());
    path.with_file_name(name)
}

/// Millisecond timestamp for stale-dir names. A clock before the epoch
/// (impossible on a sane host) degrades to suffix 0.
fn unix_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{BlobId, DirRecord, EntryManifest, NameRule, TargetSpec, ToolchainFingerprint};
    use std::collections::BTreeMap;

    /// A valid entry key for draft manifests: 64 lowercase hex digits.
    const KEY: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    /// Places raw bytes into the store through the public ingest API:
    /// the bytes land in a source file, a one-file draft references
    /// them, and ingest verifies and stores them under their content
    /// id. Sequential calls share one key; each ingest replaces the
    /// previous manifest but leaves earlier blobs stored.
    fn place_blob(store: &CasStore, dir: &Path, name: &str, bytes: &[u8]) -> BlobId {
        let id = BlobId(*blake3::hash(bytes).as_bytes());
        let src = dir.join(name);
        fs::write(&src, bytes).expect("write source file");
        let file = ManifestFile {
            path: PathBuf::from(name),
            blob: id,
            executable: false,
            rewrite: RewriteKind::None,
        };
        store
            .ingest(draft(vec![file]), &[(src, id)])
            .expect("ingest blob");
        id
    }

    /// A minimal valid draft manifest around one dir of the given files.
    fn draft(files: Vec<ManifestFile>) -> EntryManifest {
        EntryManifest {
            key: KEY.to_string(),
            project_root_at_ingest: "/old/root".to_string(),
            dirs: vec![DirRecord {
                name_observed: "ingest-tree-hash".to_string(),
                name_rule: NameRule::NotReplayable,
                name_defines: BTreeMap::new(),
                name_output_dir: String::new(),
                files,
            }],
            defines: BTreeMap::new(),
            target: TargetSpec {
                command: "build".to_string(),
                platform: "macos".to_string(),
                mode: "debug".to_string(),
                entry: None,
                flavor: None,
                extra_key_args: Vec::new(),
            },
            toolchain: ToolchainFingerprint {
                engine_revision: "test-engine".to_string(),
                flutter_version: "0.0.0".to_string(),
                platform_strong_dill: BlobId([0; 32]),
                frontend_server_snapshot: BlobId([0; 32]),
            },
            created_at_unix: 0,
            last_hit_unix: 0,
        }
    }

    fn manifest(path: &str, blob: BlobId, executable: bool, rewrite: RewriteKind) -> ManifestFile {
        ManifestFile {
            path: PathBuf::from(path),
            blob,
            executable,
            rewrite,
        }
    }

    #[test]
    fn rewrite_bytes_no_occurrence_returns_zero_unchanged() {
        let original = b"nothing to see here".to_vec();
        let mut data = original.clone();
        assert_eq!(rewrite_bytes(&mut data, b"/old/root", b"/new/root"), 0);
        assert_eq!(data, original);
    }

    #[test]
    fn rewrite_bytes_single_and_multiple_occurrences_in_place() {
        // Same-length replacement: patched in place, no reallocation.
        let mut data = Vec::with_capacity(48);
        data.extend_from_slice(b"/old/root/a /old/root/b untouched");
        let cap_before = data.capacity();
        assert_eq!(rewrite_bytes(&mut data, b"/old/root", b"/new/root"), 2);
        assert_eq!(data, b"/new/root/a /new/root/b untouched".to_vec());
        assert_eq!(
            data.capacity(),
            cap_before,
            "same-length rewrite must not reallocate"
        );
    }

    #[test]
    fn rewrite_bytes_overlapping_matches_leftmost_non_overlapping() {
        let mut data = b"aaa".to_vec();
        assert_eq!(rewrite_bytes(&mut data, b"aa", b"bb"), 1);
        assert_eq!(data, b"bba".to_vec());
    }

    #[test]
    fn rewrite_bytes_length_changes_shorter_and_longer() {
        let mut data = b"root=/old/root end".to_vec();
        assert_eq!(rewrite_bytes(&mut data, b"/old/root", b"/much/longer/root"), 1);
        assert_eq!(data, b"root=/much/longer/root end".to_vec());

        let mut data = b"root=/old/root end".to_vec();
        assert_eq!(rewrite_bytes(&mut data, b"/old/root", b"/n"), 1);
        assert_eq!(data, b"root=/n end".to_vec());
    }

    #[test]
    fn rewrite_bytes_empty_needle_returns_zero() {
        let original = b"stable".to_vec();
        let mut data = original.clone();
        assert_eq!(rewrite_bytes(&mut data, b"", b"anything"), 0);
        assert_eq!(data, original);
    }

    #[test]
    fn materialize_replays_entry_into_target_tree() {
        let store_dir = tempfile::tempdir().expect("store dir");
        let store = CasStore::open(store_dir.path()).expect("open store");

        let payload: Vec<u8> = (0..4096u32).map(|i| (i % 256) as u8).collect();
        let payload_id = place_blob(&store, store_dir.path(), "src-payload.bin", &payload);

        let old_root = "/old/root";
        let new_root = PathBuf::from("/brand/new/root");
        let stamp = format!("[\"{old_root}/lib/main.dart\"]");
        let stamp_id = place_blob(&store, store_dir.path(), "src-stamp", stamp.as_bytes());

        let plan = MaterializePlan {
            dirs: vec![DirPlan {
                name: "abc123".to_string(),
                files: vec![
                    manifest("app.dill", payload_id, false, RewriteKind::None),
                    manifest("flutter_build.dill.stamp", stamp_id, false, RewriteKind::PathText),
                    manifest("sub/dir/tool.out", payload_id, true, RewriteKind::None),
                ],
            }],
            old_root: old_root.to_string(),
            new_root: new_root.clone(),
        };

        let project = tempfile::tempdir().expect("project dir");
        let report = materialize(&store, &plan, project.path()).expect("materialize");

        let hash_dir = project.path().join(".dart_tool/flutter_build/abc123");
        assert_eq!(fs::read(hash_dir.join("app.dill")).expect("read dill"), payload);

        let rewritten = fs::read(hash_dir.join("flutter_build.dill.stamp")).expect("read stamp");
        let expected_stamp = format!("[\"{}/lib/main.dart\"]", new_root.display());
        assert_eq!(rewritten, expected_stamp.as_bytes());
        assert!(memmem::find(&rewritten, old_root.as_bytes()).is_none());

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(hash_dir.join("sub/dir/tool.out"))
                .expect("stat tool.out")
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o755);
        }

        assert_eq!(report.cloned + report.copied, 3);
        assert_eq!(report.rewritten_files, 1);
        assert_eq!(report.rewritten_occurrences, 1);
        assert!(!report.replaced_stale_dir);
        assert_eq!(
            report.total_bytes,
            (payload.len() * 2 + expected_stamp.len()) as u64
        );
    }

    #[test]
    fn materialize_multi_dir_places_renames_and_rewrites_per_dir() {
        let store_dir = tempfile::tempdir().expect("store dir");
        let store = CasStore::open(store_dir.path()).expect("open store");

        let kernel_id = place_blob(&store, store_dir.path(), "kernel", b"kernel payload");
        let old_root = "/old/root";
        let new_root = PathBuf::from("/new/root");
        let kernel_stamp = format!(r#"["{old_root}/lib/main.dart"]"#);
        let kernel_stamp_id = place_blob(&store, store_dir.path(), "ks", kernel_stamp.as_bytes());
        let assets_stamp = format!(r#"{{"{old_root}/build/a.bin"}}"#);
        let assets_stamp_id = place_blob(&store, store_dir.path(), "as", assets_stamp.as_bytes());
        let assets_id = place_blob(&store, store_dir.path(), "assets", b"asset payload");

        let plan = MaterializePlan {
            dirs: vec![
                DirPlan {
                    name: "dir0000000000000000000000000001".to_string(),
                    files: vec![
                        manifest("app.dill", kernel_id, false, RewriteKind::None),
                        manifest("k.stamp", kernel_stamp_id, false, RewriteKind::PathText),
                    ],
                },
                DirPlan {
                    name: "dir0000000000000000000000000002".to_string(),
                    files: vec![
                        manifest("assets.bin", assets_id, false, RewriteKind::None),
                        manifest("a.stamp", assets_stamp_id, false, RewriteKind::PathText),
                    ],
                },
            ],
            old_root: old_root.to_string(),
            new_root: new_root.clone(),
        };

        let project = tempfile::tempdir().expect("project dir");
        let build_root = project.path().join(".dart_tool").join("flutter_build");
        // A stale dir under the SECOND name only: the per-dir rename must
        // spare the first dir's absent target and replace the second's.
        let stale = build_root.join("dir0000000000000000000000000002");
        fs::create_dir_all(&stale).expect("mkdir stale");
        fs::write(stale.join("sentinel"), b"user data").expect("sentinel");

        let report = materialize(&store, &plan, project.path()).expect("materialize");

        assert!(report.replaced_stale_dir);
        assert_eq!(report.cloned + report.copied, 4);
        assert_eq!(report.rewritten_files, 2);
        assert_eq!(report.rewritten_occurrences, 2);
        assert_eq!(
            report.total_bytes,
            (kernel_stamp.len() + assets_stamp.len() + b"kernel payload".len()
                + b"asset payload".len()) as u64
        );

        let one = build_root.join("dir0000000000000000000000000001");
        let two = build_root.join("dir0000000000000000000000000002");
        assert_eq!(
            fs::read_to_string(one.join("k.stamp")).expect("kernel stamp"),
            format!(r#"["{}/lib/main.dart"]"#, new_root.display())
        );
        assert_eq!(
            fs::read_to_string(two.join("a.stamp")).expect("assets stamp"),
            format!(r#"{{"{}/build/a.bin"}}"#, new_root.display())
        );
        assert_eq!(
            fs::read(one.join("app.dill")).expect("kernel"),
            b"kernel payload".to_vec()
        );
        assert_eq!(
            fs::read(two.join("assets.bin")).expect("assets"),
            b"asset payload".to_vec()
        );
        // The stale dir's user data survives aside.
        let stale_dirs: Vec<_> = fs::read_dir(&build_root)
            .expect("read build root")
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name()
                    .is_some_and(|name| name.to_string_lossy().starts_with("dir0000000000000000000000000002.bcache-stale-"))
            })
            .collect();
        assert_eq!(stale_dirs.len(), 1);
        assert_eq!(
            fs::read(stale_dirs[0].join("sentinel")).expect("sentinel"),
            b"user data"
        );
    }

    #[test]
    fn materialize_renames_existing_hash_dir_aside() {
        let store_dir = tempfile::tempdir().expect("store dir");
        let store = CasStore::open(store_dir.path()).expect("open store");

        let payload_id = place_blob(&store, store_dir.path(), "src-payload.bin", b"payload bytes");
        let plan = MaterializePlan {
            dirs: vec![DirPlan {
                name: "abc123".to_string(),
                files: vec![manifest("app.dill", payload_id, false, RewriteKind::None)],
            }],
            old_root: "/old/root".to_string(),
            new_root: PathBuf::from("/new/root"),
        };

        let project = tempfile::tempdir().expect("project dir");
        let hash_dir = project.path().join(".dart_tool/flutter_build/abc123");
        fs::create_dir_all(&hash_dir).expect("create stale dir");
        fs::write(hash_dir.join("sentinel"), b"user data").expect("write sentinel");

        let report = materialize(&store, &plan, project.path()).expect("materialize");

        assert!(report.replaced_stale_dir);
        let build_root = project.path().join(".dart_tool/flutter_build");
        let stale_dirs: Vec<_> = fs::read_dir(&build_root)
            .expect("read build root")
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| {
                p.file_name()
                    .is_some_and(|n| n.to_string_lossy().starts_with("abc123.bcache-stale-"))
            })
            .collect();
        assert_eq!(stale_dirs.len(), 1, "exactly one stale dir renamed aside");
        assert_eq!(
            fs::read(stale_dirs[0].join("sentinel")).expect("sentinel survives"),
            b"user data"
        );
        assert_eq!(
            fs::read(hash_dir.join("app.dill")).expect("new content in place"),
            b"payload bytes"
        );
    }

    #[test]
    fn materialize_aborts_on_corrupted_blob() {
        let store_dir = tempfile::tempdir().expect("store dir");
        let store = CasStore::open(store_dir.path()).expect("open store");

        let payload_id = place_blob(
            &store,
            store_dir.path(),
            "src-payload.bin",
            b"original payload bytes",
        );
        let plan = MaterializePlan {
            dirs: vec![DirPlan {
                name: "abc123".to_string(),
                files: vec![manifest("app.dill", payload_id, false, RewriteKind::None)],
            }],
            old_root: "/old/root".to_string(),
            new_root: PathBuf::from("/new/root"),
        };

        // Tamper with the stored blob after ingest, keeping its size:
        // the case ingest would never repair on its own.
        fs::write(store.blob_path(&payload_id), b"corrupted payload byte").expect("tamper blob");
        assert_eq!(
            b"corrupted payload byte".len(),
            b"original payload bytes".len(),
            "the tamper keeps the blob's size"
        );

        let project = tempfile::tempdir().expect("project dir");
        let err = materialize(&store, &plan, project.path()).expect_err("corrupt blob must abort");
        assert!(
            matches!(err, BcacheError::Corrupt(_)),
            "expected Corrupt, got {err:?}"
        );
        assert!(
            err.to_string().contains(&blob_id_hex(&payload_id)),
            "error must name the blob: {err}"
        );
        // The corrupted blob is deleted, so a later ingest re-stores it.
        assert!(
            !store.blob_path(&payload_id).exists(),
            "the corrupted blob must be gone from the store"
        );
        // Documented behavior: the partial hash dir is left in place
        // (an empty dir here) — a safe miss for flutter_tools.
        let hash_dir = project.path().join(".dart_tool/flutter_build/abc123");
        assert!(hash_dir.is_dir());
        assert_eq!(fs::read_dir(&hash_dir).expect("read dir").count(), 0);
    }

    #[test]
    fn plan_validation_rejects_before_any_io() {
        let store_dir = tempfile::tempdir().expect("store dir");
        let store = CasStore::open(store_dir.path()).expect("open store");

        let project = tempfile::tempdir().expect("project dir");

        // Validation fails before any store or tree access, so the blob
        // id never needs to exist.
        let mut plan = MaterializePlan {
            dirs: vec![DirPlan {
                name: "abc123".to_string(),
                files: vec![manifest("app.dill", BlobId([9; 32]), false, RewriteKind::None)],
            }],
            old_root: "/old/root".to_string(),
            new_root: PathBuf::from("/new/root"),
        };

        plan.old_root = "/old/back\\slash".to_string();
        let err =
            materialize(&store, &plan, project.path()).expect_err("backslash must be Invalid");
        assert!(matches!(err, BcacheError::Invalid(_)), "got {err:?}");

        plan.old_root = String::new();
        let err = materialize(&store, &plan, project.path()).expect_err("empty root must be Invalid");
        assert!(matches!(err, BcacheError::Invalid(_)), "got {err:?}");

        plan.old_root = "/old/root".to_string();
        plan.dirs[0].name = "../escape".to_string();
        let err = materialize(&store, &plan, project.path()).expect_err("traversal must be Invalid");
        assert!(matches!(err, BcacheError::Invalid(_)), "got {err:?}");

        assert!(
            !project.path().join(".dart_tool").exists(),
            "a rejected plan must not touch the project tree"
        );
    }
}
