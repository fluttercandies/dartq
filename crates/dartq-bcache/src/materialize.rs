//! CAS entry → project-tree materialization: turn a store hit into a
//! native flutter_tools skip.
//!
//! A cached flutter build entry is replayed into the target worktree's
//! `.dart_tool/flutter_build/<hash-dir>/` so that flutter's
//! content-hash validation finds every target up-to-date and never
//! invokes frontend_server. The `flutter test` face replays its kernel
//! cache into the fixed-path `<buildDir>/test_cache/` instead —
//! frontend_server's initialize-from-dill validation is the equivalent
//! correctness net there, and because that validation resolves the
//! dill's embedded `file://` URIs, a replay into a DIFFERENT tree
//! rewrites them through [`dill_rewrite`] (a same-tree hit keeps the
//! verbatim clone). Payload blobs are placed with
//! copy-on-write clones (measured on APFS: 4.4 GB in 0.379 s; plain
//! copies are orders slower), never hardlinks — flutter_tools rewrites
//! some outputs in place and a hardlink would corrupt the shared
//! store. The skip only survives if the bookkeeping files
//! (stamp/`.filecache`/depfile/outputs) carry the TARGET tree's
//! absolute paths, so every byte occurrence of the ingest tree's
//! WORKTREE root is replaced with the target tree's worktree root —
//! the anchor is wider than the project root so workspace-sibling
//! paths rewrite too — and every `flutter_build/<old-name>` segment
//! recorded by the ingest tree is rewritten to the recomputed name of
//! this tree before the file lands.
//!
//! Placement is FILL-ONLY-MISSING: a hit never replaces or renames an
//! existing build dir. Files the tree already holds stay exactly as
//! flutter left them (their bookkeeping already names this tree, and
//! wholesale replacement would make two alternating trees rebuild on
//! every hit where native flutter skips); only missing files are
//! placed. Every referenced blob is verified present BEFORE the first
//! byte touches the tree, so a store gap is a clean miss rather than a
//! destroyed dir. Foreign-path dirs (bookkeeping naming paths outside
//! the ingest anchors) never reach this module cross-tree — the
//! orchestrator refuses them at plan time.
//!
//! Performance targets (measured on APFS): a 4.4 GB entry materializes
//! in under 1.5 s; the rewrite pass stays under 50 ms for typical
//! stamp/`.filecache` sets; peak memory stays under 100 MB for any
//! entry size because payloads are streamed clone/copy operations and
//! only rewrite files are loaded, one at a time, under a 16 MiB bound
//! — the one whole-file exception is the test face's kernel dill
//! (~113 MB), which [`dill_rewrite`] streams from a single full read.
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

use memchr::memmem;

use crate::dill_rewrite;
use crate::error::{BcacheError, Result};
use crate::fsutil::{clone_file, stamp_of, CloneStatus};
use crate::store::CasStore;
use crate::types::{PlacedFile, RewriteKind, blob_id_hex, ManifestFile};

/// Hard per-file bound for rewrite (`PathText`) files: 16 MiB.
/// Bookkeeping files are KBs; payload files (dills, kernels) are
/// `RewriteKind::None` and stay streamed, so the bound never bites in
/// practice — it exists so a mislabeled manifest cannot turn the
/// rewriter into a whole-file loader.
const REWRITE_LIMIT: usize = 16 * 1024 * 1024;

/// One directory of a materialization: its target-tree path (derived by
/// the caller per the entry's [`crate::NameRule`]) and the entry files
/// that land inside it.
pub struct DirPlan {
    /// Target-tree dir path relative to the project root: hash dirs land
    /// under `.dart_tool/flutter_build/<name>`, the test kernel cache at
    /// the build dir's `test_cache`. A plain relative path, validated.
    pub path: PathBuf,
    /// The `flutter_build/<old-name>` → `flutter_build/<new-name>`
    /// rewrite this dir's bookkeeping needs (`None` when the recorded
    /// name already replays verbatim — a tree-relative name, or a
    /// same-tree replay). Hash-dir names embed the ingest tree's
    /// absolute output dir, so a cross-tree replay lands under a
    /// recomputed name and every recorded mention of the old segment
    /// must follow.
    pub name_rewrite: Option<(String, String)>,
    /// Files of the dir, relative to it.
    pub files: Vec<ManifestFile>,
}

/// Everything needed to replay one cached entry into a target tree.
pub struct MaterializePlan {
    /// Every dir of the entry, in the caller's order (the orchestrator
    /// keeps the manifest's name-sorted order).
    pub dirs: Vec<DirPlan>,
    /// Ingest-tree absolute WORKTREE root: the byte string whose
    /// occurrences in bookkeeping files are rewritten away.
    pub old_root: String,
    /// Target-tree absolute worktree root: the replacement byte string.
    pub new_root: PathBuf,
}

impl MaterializePlan {
    /// Whether the replay crosses trees: the ingest root and the
    /// target root differ as byte strings. Both are canonicalized by
    /// the orchestrator — a symlink-spelled prefix (say `/tmp` for
    /// `/private/tmp` on macOS) would silently miss the dill's
    /// embedded URIs and degrade the hit to a partial recompile.
    fn roots_differ(&self) -> bool {
        self.old_root.as_bytes() != self.new_root.as_os_str().as_encoded_bytes()
    }
}

/// Counts and sizes of one materialization run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MaterializeReport {
    /// Files placed as copy-on-write clones.
    pub cloned: u64,
    /// Files placed by the streaming-copy fallback.
    pub copied: u64,
    /// Recorded files the tree already held and the run left untouched
    /// (fill-only-missing).
    pub skipped_existing: u64,
    /// Bookkeeping files in which at least one path was rewritten.
    pub rewritten_files: u64,
    /// Total non-overlapping root- and name-segment replacements
    /// performed.
    pub rewritten_occurrences: u64,
    /// Kernel-cache dills rewritten to this tree's URI prefix (a
    /// cross-tree hit; [`crate::types::RewriteKind::DillUris`]).
    pub dill_rewrites: u64,
    /// Kernel-cache dills placed verbatim because the URI rewriter
    /// refused them (version gate, non-ASCII match, unknown layout):
    /// today's same-tree behavior, at the cost of a frontend_server
    /// recompile.
    pub dill_rewrite_refused: u64,
    /// Final on-disk bytes of the materialized entry.
    pub total_bytes: u64,
}

/// The result of one materialization run: the counts above plus, per
/// plan dir in plan order, every placed file's fresh identity (see
/// [`PlacedFile`]) — the handoff the orchestrator's post-run scan uses
/// to recognize the files it placed without re-reading them.
#[derive(Debug, Clone)]
pub struct MaterializeOutcome {
    pub report: MaterializeReport,
    pub placed: Vec<Vec<PlacedFile>>,
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

/// Materialize `plan` into `project_root`: one dir per [`DirPlan`], each
/// at its recorded target-tree relative path. Returns the run's counts
/// and, per dir, the placed files' fresh identities for the post-run
/// scan (see [`PlacedFile`]).
///
/// Placement is fill-only-missing: a file the tree already holds is
/// left untouched, and no dir is ever replaced or renamed aside — a
/// dir flutter itself wrote already carries this tree's paths, and
/// replacing it with the cached copy (whose bytes embed the tree that
/// built last) would force a rebuild on every hit where native flutter
/// skips. Every referenced blob is verified PRESENT before the first
/// byte touches the tree; the blobs of every file the tree is missing
/// are then identity-verified (streamed blake3, fanned out over four
/// scoped readers — audit A15: the check was ~96% of a restore's wall
/// time as one serialized read chain), and a mismatch aborts with
/// [`BcacheError::Corrupt`] naming the blob — a wrong artifact is worse
/// than a cache miss. Because verification now completes before the
/// first placement, a corrupt store blob costs a fully clean miss, not
/// a half-filled dir. Bookkeeping files get their embedded ingest-tree
/// paths rewritten to target-tree paths (root segments, then
/// `flutter_build/<name>` segments) before the run reports success;
/// without that rewrite the skip would not survive the tree move.
pub fn materialize(
    store: &CasStore,
    plan: &MaterializePlan,
    project_root: &Path,
) -> Result<MaterializeOutcome> {
    validate_plan(plan)?;
    // The whole plan's blobs must exist before anything is touched: a
    // gc racing this hit must degrade it to a clean miss, never
    // replace a valid dir's files with a restore that fails halfway.
    for dir in &plan.dirs {
        for file in &dir.files {
            let blob = store.blob_path(&file.blob);
            if !blob.is_file() {
                return Err(BcacheError::NotFound(format!(
                    "blob {} for {} is missing from the store",
                    blob_id_hex(&file.blob),
                    file.path.display()
                )));
            }
        }
    }

    // The blobs of every file the tree is missing, identity-verified up
    // front and in parallel (see the doc comment above).
    let missing = missing_files(plan, project_root);
    let sources: Vec<PathBuf> = missing
        .iter()
        .map(|(dir_at, file_at)| store.blob_path(&plan.dirs[*dir_at].files[*file_at].blob))
        .collect();
    verify_missing_blobs(store, plan, &missing, &sources)?;

    let mut report = MaterializeReport {
        cloned: 0,
        copied: 0,
        skipped_existing: 0,
        rewritten_files: 0,
        rewritten_occurrences: 0,
        dill_rewrites: 0,
        dill_rewrite_refused: 0,
        total_bytes: 0,
    };
    let mut placed: Vec<Vec<PlacedFile>> = Vec::with_capacity(plan.dirs.len());

    for dir in &plan.dirs {
        let target = project_root.join(&dir.path);
        fs::create_dir_all(&target)?;
        let mut dir_placed = Vec::with_capacity(dir.files.len());
        for file in &dir.files {
            if target.join(&file.path).exists() {
                // Fill-only-missing: the tree's own copy stays. Its
                // identity is unknown to this run, so nothing may
                // vouch for it in the post-run scan.
                report.skipped_existing += 1;
                continue;
            }
            dir_placed.push(materialize_one(store, plan, dir, file, &target, &mut report)?);
        }
        placed.push(dir_placed);
    }
    Ok(MaterializeOutcome { report, placed })
}

/// The `(dir, file)` positions of a plan whose target files the tree
/// does not hold yet — exactly the blobs the integrity pass must hash.
fn missing_files(plan: &MaterializePlan, project_root: &Path) -> Vec<(usize, usize)> {
    let mut missing = Vec::new();
    for (dir_at, dir) in plan.dirs.iter().enumerate() {
        let target = project_root.join(&dir.path);
        for (file_at, file) in dir.files.iter().enumerate() {
            if !target.join(&file.path).exists() {
                missing.push((dir_at, file_at));
            }
        }
    }
    missing
}

/// Hashes every missing file's blob with
/// [`crate::fsutil::hash_files_parallel`] and checks each digest against
/// the recorded one. A mismatch quarantines the blob by deletion
/// (ingest leaves a same-size stored blob alone — removing it is what
/// lets the next ingest re-store the true bytes) and aborts the
/// materialization with [`BcacheError::Corrupt`].
fn verify_missing_blobs(
    store: &CasStore,
    plan: &MaterializePlan,
    missing: &[(usize, usize)],
    sources: &[PathBuf],
) -> Result<()> {
    #[cfg(not(unix))]
    let digests: Vec<std::io::Result<crate::types::BlobId>> = {
        use crate::fsutil::file_blake3;
        sources.iter().map(|src| file_blake3(src)).collect()
    };
    #[cfg(unix)]
    let digests = crate::fsutil::hash_files_parallel(sources);
    for ((dir_at, file_at), result) in missing.iter().zip(digests) {
        let file = &plan.dirs[*dir_at].files[*file_at];
        let digest = result.map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => BcacheError::NotFound(format!(
                "blob {} for {} is missing from the store",
                blob_id_hex(&file.blob),
                file.path.display()
            )),
            _ => BcacheError::Io(e),
        })?;
        if digest != file.blob {
            let _ = fs::remove_file(store.blob_path(&file.blob));
            return Err(BcacheError::Corrupt(format!(
                "blob {} for {} failed its content check",
                blob_id_hex(&file.blob),
                file.path.display()
            )));
        }
    }
    Ok(())
}

/// Places one manifest file — presence and integrity already verified
/// by [`materialize`]'s upfront pass — into its materialized dir and
/// reports it with the identity it landed under.
fn materialize_one(
    store: &CasStore,
    plan: &MaterializePlan,
    dir: &DirPlan,
    file: &ManifestFile,
    dir_path: &Path,
    report: &mut MaterializeReport,
) -> Result<PlacedFile> {
    let src = store.blob_path(&file.blob);

    let dst = dir_path.join(&file.path);
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent)?;
    }

    // Rename-atomic placement: write into a temp sibling, then rename,
    // so no reader sees a half-written file. A leftover temp from a
    // crashed run is cleared by clone_file's EEXIST handling.
    let tmp = tmp_sibling(&dst);

    // Whether the bytes that land are the blob's bytes. Every path
    // below starts verbatim (the clone) and only the rewrites that
    // actually change bytes clear it — the post-run scan may reference
    // the blob directly for a verbatim placement, never for a
    // rewritten one.
    let mut verbatim = true;

    // A kernel-cache dill replayed into a different tree carries the
    // ingest tree's `file://` URIs, which frontend_server cannot
    // resolve here — rewrite the prefixes while the bytes stream into
    // the temp. A refused rewrite (or a same-root plan) falls through
    // to the verbatim clone: today's behavior, never a blocked build.
    let mut placed = false;
    if matches!(file.rewrite, RewriteKind::DillUris) && plan.roots_differ() {
        let mut from = b"file://".to_vec();
        from.extend_from_slice(plan.old_root.as_bytes());
        from.push(b'/');
        let mut to = b"file://".to_vec();
        to.extend_from_slice(plan.new_root.as_os_str().as_encoded_bytes());
        to.push(b'/');
        match dill_rewrite::rewrite_file(&src, &tmp, &from, &to) {
            Ok(_) => {
                report.dill_rewrites += 1;
                placed = true;
                verbatim = false;
            }
            Err(_) => {
                report.dill_rewrite_refused += 1;
                let _ = fs::remove_file(&tmp);
            }
        }
    }

    if !placed {
        match clone_file(&src, &tmp)? {
            CloneStatus::Cloned => report.cloned += 1,
            CloneStatus::Copied => report.copied += 1,
        }
    }
    fs::rename(&tmp, &dst)?;

    // Rewrite before the exec bit: the rewrite writes a fresh file via
    // tmp+rename, which would discard permissions set earlier. A
    // same-root plan rewrites the root onto itself — bytes unchanged —
    // so only a cross-root replacement clears the verbatim flag.
    if matches!(file.rewrite, RewriteKind::PathText) {
        let old_root = plan.old_root.as_bytes();
        let new_root = plan.new_root.as_os_str().as_encoded_bytes();
        let mut occurrences = rewrite_bookkeeping(&dst, &[(old_root, new_root)])?;
        if let Some((old_name, new_name)) = &dir.name_rewrite {
            occurrences += rewrite_name_segments(&dst, old_name, new_name)?;
        }
        if occurrences > 0 {
            report.rewritten_files += 1;
            report.rewritten_occurrences += occurrences as u64;
            if plan.roots_differ() {
                verbatim = false;
            }
        }
    }

    if file.executable {
        set_executable(&dst)?;
    }

    let metadata = fs::metadata(&dst)?;
    report.total_bytes += metadata.len();
    Ok(PlacedFile {
        path: file.path.clone(),
        stamp: stamp_of(&metadata),
        blob: file.blob,
        rewrite: file.rewrite,
        foreign_paths: file.foreign_paths,
        verbatim,
    })
}

/// Applies `(old, new)` byte replacements to one bookkeeping file. The
/// file is fully loaded under [`REWRITE_LIMIT`], patched in memory, and
/// written back via tmp+rename in the same directory. Returns the
/// number of replacements; zero means the file already carried the
/// target spelling and is left untouched on disk.
fn rewrite_bookkeeping(path: &Path, pairs: &[(&[u8], &[u8])]) -> Result<usize> {
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
    let mut occurrences = 0;
    for (old, new) in pairs {
        occurrences += rewrite_bytes(&mut data, old, new);
    }
    if occurrences > 0 {
        let tmp = tmp_sibling(path);
        fs::write(&tmp, &data)?;
        fs::rename(&tmp, path)?;
    }
    Ok(occurrences)
}

/// Rewrites `flutter_build/<old_name>` segments to
/// `flutter_build/<new_name>` in one bookkeeping file. A match only
/// counts when the name ends at a non-hex byte — config-hash names are
/// exactly 32 lowercase hex digits, so a longer hex run is not this
/// dir's name and must not be touched. The file is loaded under
/// [`REWRITE_LIMIT`] and written back via tmp+rename. Returns the
/// number of segment replacements.
fn rewrite_name_segments(path: &Path, old_name: &str, new_name: &str) -> Result<usize> {
    let mut file = fs::File::open(path)?;
    let mut data = Vec::new();
    (&mut file)
        .take(REWRITE_LIMIT as u64 + 1)
        .read_to_end(&mut data)?;
    if data.len() > REWRITE_LIMIT {
        return Err(BcacheError::Unsupported(format!(
            "bookkeeping file {} is {} bytes; the name rewriter supports at most {}",
            path.display(),
            data.len(),
            REWRITE_LIMIT
        )));
    }
    let needle: Vec<u8> = format!("flutter_build/{old_name}").into_bytes();
    let replacement: Vec<u8> = format!("flutter_build/{new_name}").into_bytes();
    let finder = memmem::Finder::new(&needle);
    let mut cursor = 0usize;
    let mut count = 0usize;
    let mut out = Vec::with_capacity(data.len());
    while let Some(pos) = finder.find(&data[cursor..]) {
        let at = cursor + pos;
        let end = at + needle.len();
        // The byte after the match must not extend the hex name.
        let boundary = data
            .get(end)
            .is_none_or(|&byte| !byte.is_ascii_hexdigit());
        out.extend_from_slice(&data[cursor..at]);
        if boundary {
            out.extend_from_slice(&replacement);
            count += 1;
        } else {
            out.extend_from_slice(&needle);
        }
        cursor = end;
    }
    out.extend_from_slice(&data[cursor..]);
    if count > 0 {
        let tmp = tmp_sibling(path);
        fs::write(&tmp, &out)?;
        fs::rename(&tmp, path)?;
    }
    Ok(count)
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
        // A dir path must be a plain relative path: not absolute, no
        // `.`, `..`, or root component, and no backslash (a Windows-path
        // leak that would not name a dir on the target's separator).
        let plain = dir.path.is_relative()
            && !dir.path.as_os_str().is_empty()
            && dir.path.components().all(|component| {
                matches!(component, std::path::Component::Normal(name)
                    if !name.as_encoded_bytes().contains(&b'\\'))
            });
        if !plain {
            return Err(BcacheError::Invalid(format!(
                "materialize dir path {:?} is not a plain relative path",
                dir.path
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
                    "manifest path {} escapes its dir",
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
/// crash leftovers out of flutter's glob-based scans, the embedded file
/// name keeps temp names unique per target file, and the embedded pid
/// keeps two shims materializing into one tree (a terminal and an IDE —
/// audit A14) from clobbering each other's temp before the rename.
fn tmp_sibling(path: &Path) -> PathBuf {
    let name = path
        .file_name()
        .map(|n| {
            format!(
                ".{}.{}.dartq-tmp",
                n.to_string_lossy(),
                std::process::id()
            )
        })
        .unwrap_or_else(|| format!(".{}.dartq-tmp", std::process::id()));
    path.with_file_name(name)
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
            foreign_paths: false,
            stamp: None,
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
            worktree_root_at_ingest: "/old/root".to_string(),
            pub_cache_root: String::new(),
            flutter_root_at_ingest: String::new(),
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
            foreign_paths: false,
            stamp: None,
        }
    }

    /// A hash-dir plan under `.dart_tool/flutter_build`, the build faces'
    /// shape. `name_rewrite` defaults to verbatim (no recorded-name
    /// rewrite); tests that need one pass it explicitly.
    fn hash_dir(name: &str, files: Vec<ManifestFile>) -> DirPlan {
        DirPlan {
            path: Path::new(".dart_tool").join("flutter_build").join(name),
            name_rewrite: None,
            files,
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
            dirs: vec![hash_dir(
                "abc123",
                vec![
                    manifest("app.dill", payload_id, false, RewriteKind::None),
                    manifest("flutter_build.dill.stamp", stamp_id, false, RewriteKind::PathText),
                    manifest("sub/dir/tool.out", payload_id, true, RewriteKind::None),
                ],
            )],
            old_root: old_root.to_string(),
            new_root: new_root.clone(),
        };

        let project = tempfile::tempdir().expect("project dir");
        let MaterializeOutcome { report, .. } =
        materialize(&store, &plan, project.path()).expect("materialize");

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
        assert_eq!(report.skipped_existing, 0);
        assert_eq!(report.rewritten_files, 1);
        assert_eq!(report.rewritten_occurrences, 1);
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
                hash_dir(
                    "dir0000000000000000000000000001",
                    vec![
                        manifest("app.dill", kernel_id, false, RewriteKind::None),
                        manifest("k.stamp", kernel_stamp_id, false, RewriteKind::PathText),
                    ],
                ),
                hash_dir(
                    "dir0000000000000000000000000002",
                    vec![
                        manifest("assets.bin", assets_id, false, RewriteKind::None),
                        manifest("a.stamp", assets_stamp_id, false, RewriteKind::PathText),
                    ],
                ),
            ],
            old_root: old_root.to_string(),
            new_root: new_root.clone(),
        };

        let project = tempfile::tempdir().expect("project dir");
        let build_root = project.path().join(".dart_tool").join("flutter_build");
        // A pre-existing dir under the SECOND name only: its recorded
        // stamp already exists (fill-only-missing leaves it — it carries
        // this tree's own paths), its payload does not (placed).
        let existing = build_root.join("dir0000000000000000000000000002");
        fs::create_dir_all(&existing).expect("mkdir existing");
        fs::write(existing.join("a.stamp"), b"this tree's own stamp").expect("stamp");

        let MaterializeOutcome { report, .. } =
            materialize(&store, &plan, project.path()).expect("materialize");

        assert_eq!(report.cloned + report.copied, 3);
        assert_eq!(report.skipped_existing, 1);
        assert_eq!(report.rewritten_files, 1);
        assert_eq!(report.rewritten_occurrences, 1);
        assert_eq!(
            report.total_bytes,
            (kernel_stamp.len() + b"kernel payload".len() + b"asset payload".len()) as u64
        );

        let one = build_root.join("dir0000000000000000000000000001");
        let two = build_root.join("dir0000000000000000000000000002");
        assert_eq!(
            fs::read_to_string(one.join("k.stamp")).expect("kernel stamp"),
            format!(r#"["{}/lib/main.dart"]"#, new_root.display())
        );
        assert_eq!(
            fs::read_to_string(two.join("a.stamp")).expect("existing stamp untouched"),
            "this tree's own stamp"
        );
        assert_eq!(
            fs::read(one.join("app.dill")).expect("kernel"),
            b"kernel payload".to_vec()
        );
        assert_eq!(
            fs::read(two.join("assets.bin")).expect("assets"),
            b"asset payload".to_vec()
        );
        // Nothing is renamed aside, ever.
        let aside: Vec<_> = fs::read_dir(&build_root)
            .expect("read build root")
            .filter_map(|entry| entry.ok())
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .contains("bcache-stale")
            })
            .collect();
        assert!(aside.is_empty(), "fill-only-missing renames nothing aside");
    }

    /// A3's core: a hit over a dir that already exists fills ONLY the
    /// missing files — existing bytes keep their content and inode, no
    /// stale sibling appears — and a missing blob aborts as a clean
    /// miss BEFORE the tree is touched.
    #[test]
    fn materialize_fills_only_missing_files_and_verifies_blobs_first() {
        let store_dir = tempfile::tempdir().expect("store dir");
        let store = CasStore::open(store_dir.path()).expect("open store");

        let payload_id = place_blob(&store, store_dir.path(), "payload", b"payload bytes");
        let stamp_id = place_blob(&store, store_dir.path(), "stamp", b"stamp bytes");
        let missing_id = BlobId(*blake3::hash(b"never stored").as_bytes());
        let plan = |files: Vec<ManifestFile>| MaterializePlan {
            dirs: vec![hash_dir("abc123", files)],
            old_root: "/old/root".to_string(),
            new_root: PathBuf::from("/new/root"),
        };

        let project = tempfile::tempdir().expect("project dir");
        let hash_dir = project.path().join(".dart_tool/flutter_build/abc123");
        fs::create_dir_all(&hash_dir).expect("create existing dir");
        fs::write(hash_dir.join("app.dill"), b"this tree's payload").expect("payload");
        let before = fs::metadata(hash_dir.join("app.dill")).expect("stat");

        // The complete-entry case: only the missing stamp lands.
        let MaterializeOutcome { report, placed } = materialize(
            &store,
            &plan(vec![
                manifest("app.dill", payload_id, false, RewriteKind::None),
                manifest("flutter_build.dill.stamp", stamp_id, false, RewriteKind::None),
            ]),
            project.path(),
        )
        .expect("materialize");
        assert_eq!(report.skipped_existing, 1);
        assert_eq!(report.cloned + report.copied, 1);
        assert_eq!(
            fs::read(hash_dir.join("app.dill")).expect("existing untouched"),
            b"this tree's payload"
        );
        let after = fs::metadata(hash_dir.join("app.dill")).expect("stat after");
        assert_eq!(
            crate::fsutil::stamp_of(&after),
            crate::fsutil::stamp_of(&before),
            "the existing file is not rewritten"
        );
        assert_eq!(
            fs::read(hash_dir.join("flutter_build.dill.stamp")).expect("filled stamp"),
            b"stamp bytes"
        );
        // Only the placed file is reported for the post-run scan.
        assert_eq!(placed[0].len(), 1);
        assert_eq!(placed[0][0].path, Path::new("flutter_build.dill.stamp"));
        // No stale siblings anywhere.
        let build_root = project.path().join(".dart_tool/flutter_build");
        let aside: Vec<_> = fs::read_dir(&build_root)
            .expect("read build root")
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains("bcache-stale"))
            .collect();
        assert!(aside.is_empty(), "the dir is never moved aside");

        // The S6 case: a blob gc removed mid-plan aborts as a clean
        // miss with the tree untouched — a valid dir survives.
        fs::write(
            hash_dir.join("flutter_build.dill.stamp"),
            b"this tree's stamp",
        )
        .expect("stamp");
        let err = materialize(
            &store,
            &plan(vec![manifest("gone.bin", missing_id, false, RewriteKind::None)]),
            project.path(),
        )
        .expect_err("a missing blob aborts");
        assert!(
            matches!(&err, BcacheError::NotFound(msg) if msg.contains(&blob_id_hex(&missing_id))),
            "expected NotFound naming the blob, got {err:?}"
        );
        assert_eq!(
            fs::read(hash_dir.join("flutter_build.dill.stamp")).expect("stamp intact"),
            b"this tree's stamp"
        );
    }

    /// A4 in isolation: recorded `flutter_build/<old-name>` segments in
    /// bookkeeping bytes follow the dir to its recomputed name, with a
    /// hex-boundary guard so a longer hex run is never mis-rewritten.
    #[test]
    fn materialize_rewrites_recorded_dir_name_segments() {
        let store_dir = tempfile::tempdir().expect("store dir");
        let store = CasStore::open(store_dir.path()).expect("open store");

        let old_name = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let new_name = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
        let stamp = format!(
            r#"{{"inputs":["/old/root/.dart_tool/flutter_build/{old_name}/app.dill"],"outputs":[".dart_tool/flutter_build/{old_name}/out.d","ref":"flutter_build/{old_name}deadbeefdeadbeefdeadbeef00"]}}"#
        );
        let stamp_id = place_blob(&store, store_dir.path(), "stamp", stamp.as_bytes());

        let mut dir = hash_dir(
            new_name,
            vec![manifest("flutter_build.dill.stamp", stamp_id, false, RewriteKind::PathText)],
        );
        dir.name_rewrite = Some((old_name.to_owned(), new_name.to_owned()));
        let plan = MaterializePlan {
            dirs: vec![dir],
            old_root: "/old/root".to_string(),
            new_root: PathBuf::from("/new/root"),
        };

        let project = tempfile::tempdir().expect("project dir");
        let MaterializeOutcome { report, .. } =
            materialize(&store, &plan, project.path()).expect("materialize");

        let placed = fs::read_to_string(
            project
                .path()
                .join(".dart_tool/flutter_build")
                .join(new_name)
                .join("flutter_build.dill.stamp"),
        )
        .expect("placed stamp");
        assert!(
            placed.contains(&format!("/new/root/.dart_tool/flutter_build/{new_name}/app.dill")),
            "absolute mention re-rooted and renamed: {placed}"
        );
        assert!(
            placed.contains(&format!("flutter_build/{new_name}/out.d")),
            "relative mention renamed: {placed}"
        );
        assert!(
            !placed.contains(&format!("flutter_build/{old_name}/")),
            "no old-name segment survives: {placed}"
        );
        assert!(
            placed.contains("flutter_build/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaadeadbeefdeadbeefdeadbeef00"),
            "the longer hex run keeps the old bytes: {placed}"
        );
        // Two segment hits (absolute + relative) plus the root hit.
        assert_eq!(report.rewritten_occurrences, 3);
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
            dirs: vec![hash_dir(
                "abc123",
                vec![manifest("app.dill", payload_id, false, RewriteKind::None)],
            )],
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
        // With verification completing before the first placement, a
        // corrupt blob costs a fully clean miss: the hash dir is never
        // created (the pre-parallel behavior left an empty dir behind).
        let hash_dir = project.path().join(".dart_tool/flutter_build/abc123");
        assert!(!hash_dir.exists(), "the tree is never touched");
    }

    #[test]
    fn plan_validation_rejects_before_any_io() {
        let store_dir = tempfile::tempdir().expect("store dir");
        let store = CasStore::open(store_dir.path()).expect("open store");

        let project = tempfile::tempdir().expect("project dir");

        // Validation fails before any store or tree access, so the blob
        // id never needs to exist.
        let mut plan = MaterializePlan {
            dirs: vec![hash_dir(
                "abc123",
                vec![manifest("app.dill", BlobId([9; 32]), false, RewriteKind::None)],
            )],
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
        plan.dirs[0].path = PathBuf::from("../escape");
        let err = materialize(&store, &plan, project.path()).expect_err("traversal must be Invalid");
        assert!(matches!(err, BcacheError::Invalid(_)), "got {err:?}");

        plan.dirs[0].path = PathBuf::from("/absolute/dir");
        let err =
            materialize(&store, &plan, project.path()).expect_err("absolute must be Invalid");
        assert!(matches!(err, BcacheError::Invalid(_)), "got {err:?}");

        assert!(
            !project.path().join(".dart_tool").exists(),
            "a rejected plan must not touch the project tree"
        );
    }

    /// The fixed-path face: the plan lands at its project-root relative
    /// path, over a pre-existing dir, filling only what is missing —
    /// the other configs' files stay where flutter looks.
    #[test]
    fn materialize_in_place_plan_lands_at_fixed_path() {
        let store_dir = tempfile::tempdir().expect("store dir");
        let store = CasStore::open(store_dir.path()).expect("open store");
        let dill_id = place_blob(&store, store_dir.path(), "src-dill", b"kernel bytes");
        let other_id = place_blob(&store, store_dir.path(), "src-other", b"other config bytes");

        let plan = MaterializePlan {
            dirs: vec![DirPlan {
                path: PathBuf::from("build/test_cache"),
                name_rewrite: None,
                files: vec![
                    manifest("build/ab.cache.dill", dill_id, false, RewriteKind::None),
                    manifest("build/cd.cache.dill", other_id, false, RewriteKind::None),
                ],
            }],
            old_root: "/old/root".to_string(),
            new_root: PathBuf::from("/new/root"),
        };

        let project = tempfile::tempdir().expect("project dir");
        let cache = project.path().join("build").join("test_cache");
        // A foreign-config dill and an unrelated file already live there.
        fs::create_dir_all(cache.join("build")).expect("mkdir");
        fs::write(cache.join("build").join("ef.cache.dill"), b"foreign config").expect("foreign");
        fs::write(cache.join("build").join("lock"), b"transient").expect("lock");

        let MaterializeOutcome { report, .. } =
        materialize(&store, &plan, project.path()).expect("materialize");

        assert_eq!(report.skipped_existing, 0, "the pre-existing foreign files are not recorded files");
        assert_eq!(
            fs::read(cache.join("build").join("ab.cache.dill")).expect("dill"),
            b"kernel bytes".to_vec()
        );
        assert_eq!(
            fs::read(cache.join("build").join("cd.cache.dill")).expect("other"),
            b"other config bytes".to_vec()
        );
        assert_eq!(
            fs::read(cache.join("build").join("ef.cache.dill")).expect("foreign stays"),
            b"foreign config".to_vec()
        );
        assert_eq!(
            fs::read(cache.join("build").join("lock")).expect("lock stays"),
            b"transient".to_vec()
        );
        // No stale siblings accumulate next to the dir.
        let aside: Vec<_> = fs::read_dir(project.path().join("build"))
            .expect("read build")
            .filter_map(|entry| entry.ok())
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .contains("bcache-stale")
            })
            .collect();
        assert!(aside.is_empty(), "in-place placement renames nothing aside");
    }

    /// The cross-tree kernel-dill paths: a `DillUris` file whose blob is
    /// a valid kernel component is placed with its `file://` URIs
    /// rewritten to the target root; a blob the rewriter refuses lands
    /// verbatim (counted, never fatal); a same-root plan keeps the
    /// byte-identical clone.
    #[test]
    fn materialize_cross_tree_dill_rewrites_and_refusal_falls_back() {
        use crate::dill_rewrite::synthetic_component;

        let store_dir = tempfile::tempdir().expect("store dir");
        let store = CasStore::open(store_dir.path()).expect("open store");

        let old_root = "/old/canonical/root";
        let project = tempfile::tempdir().expect("project dir");
        let new_root = fs::canonicalize(project.path()).expect("canonical root");
        let old_uri = format!("file://{old_root}/lib/main.dart");
        let dill = synthetic_component(
            &[(old_uri.as_str(), "package:app/main.dart")],
            &[old_uri.as_str(), "dart:core"],
            1,
        );
        let dill_id = place_blob(&store, store_dir.path(), "src-dill", &dill);
        let garbage_id = place_blob(&store, store_dir.path(), "src-garbage", b"not a dill");

        let cache_dir = DirPlan {
            path: PathBuf::from("build/test_cache"),
            name_rewrite: None,
            files: vec![
                manifest("build/ab.cache.dill", dill_id, false, RewriteKind::DillUris),
                manifest("build/cd.cache.dill", garbage_id, false, RewriteKind::DillUris),
            ],
        };

        // Cross-tree: the valid dill is rewritten, the garbage one
        // degrades to the verbatim clone.
        let plan = MaterializePlan {
            dirs: vec![cache_dir],
            old_root: old_root.to_string(),
            new_root: new_root.clone(),
        };
        let MaterializeOutcome { report, .. } =
        materialize(&store, &plan, project.path()).expect("materialize");
        assert_eq!(report.dill_rewrites, 1);
        assert_eq!(report.dill_rewrite_refused, 1);
        assert_eq!(report.cloned + report.copied, 1, "only the refused dill clones");

        let placed = fs::read(project.path().join("build/test_cache/build/ab.cache.dill"))
            .expect("placed dill");
        let new_uri = format!("file://{}/lib/main.dart", new_root.display());
        assert!(memmem::find(&placed, new_uri.as_bytes()).is_some(), "URI rewritten");
        // The old URI survives exactly once: the embedded source TEXT
        // (diagnostics-only, left verbatim by design); its source-table
        // and string-table occurrences are rewritten away.
        assert_eq!(
            memmem::find_iter(&placed, old_uri.as_bytes()).count(),
            1,
            "only the embedded source text keeps the old-tree URI"
        );
        assert_eq!(
            fs::read(project.path().join("build/test_cache/build/cd.cache.dill"))
                .expect("refused dill"),
            b"not a dill".to_vec(),
            "a refused rewrite places the blob verbatim"
        );

        // Same-root: the verbatim clone, no rewrite attempt. A fresh
        // tree, so fill-only-missing actually places both files.
        let same_project = tempfile::tempdir().expect("same-root project dir");
        let same = MaterializePlan {
            dirs: vec![DirPlan {
                path: PathBuf::from("build/test_cache"),
                name_rewrite: None,
                files: vec![
                    manifest("build/ab.cache.dill", dill_id, false, RewriteKind::DillUris),
                    manifest("build/cd.cache.dill", garbage_id, false, RewriteKind::DillUris),
                ],
            }],
            old_root: old_root.to_string(),
            new_root: PathBuf::from(old_root),
        };
        let MaterializeOutcome { report, .. } =
            materialize(&store, &same, same_project.path()).expect("materialize");
        assert_eq!(report.dill_rewrites, 0);
        assert_eq!(report.dill_rewrite_refused, 0);
        assert_eq!(report.cloned + report.copied, 2, "same-tree clones both");
        assert_eq!(
            fs::read(same_project.path().join("build/test_cache/build/ab.cache.dill"))
                .expect("verbatim dill"),
            dill,
            "same-root replay is byte-identical"
        );
    }

    /// The placed-identity handoff: every placed file reports its fresh
    /// on-disk stamp, and `verbatim` is exactly "the placed bytes are
    /// the blob's bytes" — true for payload clones and same-root
    /// rewrites, false for a cross-tree path rewrite that replaced
    /// bytes.
    #[test]
    fn materialize_reports_placed_identities_for_post_scan() {
        let store_dir = tempfile::tempdir().expect("store dir");
        let store = CasStore::open(store_dir.path()).expect("open store");

        let payload_id = place_blob(&store, store_dir.path(), "payload", b"payload bytes");
        let old_root = "/old/root";
        let stamp = format!("[\"{old_root}/lib/main.dart\"]");
        let stamp_id = place_blob(&store, store_dir.path(), "stamp", stamp.as_bytes());
        let files = vec![
            manifest("app.dill", payload_id, false, RewriteKind::None),
            manifest("flutter_build.dill.stamp", stamp_id, false, RewriteKind::PathText),
        ];

        // Cross-tree: the PathText stamp lands with the new root inside,
        // so it is NOT its blob's bytes; the payload clone is.
        let project = tempfile::tempdir().expect("project dir");
        let plan = MaterializePlan {
            dirs: vec![hash_dir("abc123", files.clone())],
            old_root: old_root.to_string(),
            new_root: PathBuf::from("/new/root"),
        };
        let outcome = materialize(&store, &plan, project.path()).expect("materialize");
        assert_eq!(outcome.placed.len(), 1, "one PlacedFile vec per plan dir");
        let placed = &outcome.placed[0];
        assert_eq!(placed.len(), 2);

        let dill = placed
            .iter()
            .find(|f| f.path == Path::new("app.dill"))
            .expect("placed dill");
        assert!(dill.verbatim, "a payload clone is the blob's bytes");
        assert_eq!(dill.blob, payload_id);
        assert_eq!(
            dill.stamp,
            crate::fsutil::stamp_of(
                &fs::metadata(project.path().join(".dart_tool/flutter_build/abc123/app.dill"))
                    .expect("metadata")
            ),
            "the fresh identity of the placed file"
        );

        let stamp_file = placed
            .iter()
            .find(|f| f.path == Path::new("flutter_build.dill.stamp"))
            .expect("placed stamp");
        assert!(!stamp_file.verbatim, "a cross-tree rewrite changed the bytes");
        assert_eq!(stamp_file.rewrite, RewriteKind::PathText);

        // Same-root: the rewrite replaces the root onto itself — bytes
        // unchanged — so every placement stays verbatim.
        let same = MaterializePlan {
            dirs: vec![hash_dir("abc123", files)],
            old_root: old_root.to_string(),
            new_root: PathBuf::from(old_root),
        };
        let outcome = materialize(&store, &same, project.path()).expect("materialize");
        assert!(
            outcome.placed[0].iter().all(|file| file.verbatim),
            "same-root placements keep the blob's bytes"
        );
    }

    /// A14's temp-name half: rename-atomic temps carry the writing
    /// process's pid, so two shims materializing into one tree never
    /// target the same temp path.
    #[test]
    fn tmp_sibling_carries_the_writer_pid() {
        let a = tmp_sibling(Path::new("/tree/.dart_tool/flutter_build/x/app.dill"));
        let b = tmp_sibling(Path::new("/tree/.dart_tool/flutter_build/x/kernel_snapshot.d"));
        let expected = format!(".app.dill.{}.dartq-tmp", std::process::id());
        assert_eq!(
            a.file_name().expect("name"),
            expected.as_str(),
            "the temp name embeds this process's pid"
        );
        assert_ne!(
            a.file_name(),
            b.file_name(),
            "distinct targets keep distinct temps"
        );
    }
}
