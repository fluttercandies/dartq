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
//!
//! The content hashes flutter validates live in each hash dir's
//! FileStore record, `.filecache` (flutter_tools 3.47.1
//! `build_system/file_store.dart`: `{"version":2,"files":[{"path",
//! "hash"}]}`, the md5 of every tracked input and output by absolute
//! path). A cross-tree replay rewrites bytes flutter tracks there — a
//! payload dill's URIs, a re-rooted output — so a `.filecache` this run
//! places carries the md5 of each rewritten file's NEW bytes; with the
//! ingest tree's md5 left in place, `Node.computeChanges` would see the
//! rewritten dill as changed output and recompile the kernel on every
//! fresh-tree hit (see [`patch_file_store`]).

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use memchr::memmem;
use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};

use crate::dill_rewrite;
use crate::error::{BcacheError, Result};
use crate::fsutil::{CloneStatus, clone_file, file_blake3, stamp_of};
use crate::store::CasStore;
use crate::types::{BlobId, ManifestFile, PlacedFile, RewriteKind, blob_id_hex};

/// Hard per-file bound for rewrite (`PathText`) files: 16 MiB.
/// Bookkeeping files are KBs; payload files (dills, kernels) are
/// `RewriteKind::None` and stay streamed, so the bound never bites in
/// practice — it exists so a mislabeled manifest cannot turn the
/// rewriter into a whole-file loader. The ingest refuses bookkeeping
/// files past it up front (see [`crate::ingest_scan::ScanSkips`]), so
/// no entry carries a file every hit would refuse to rewrite.
pub(crate) const REWRITE_LIMIT: usize = 16 * 1024 * 1024;

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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MaterializeReport {
    /// The warm no-op: the tree already held every recorded file, so
    /// fill-only-missing had nothing to fill and the run touched neither
    /// the store nor the tree (see [`materialize`]).
    pub intact: bool,
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
    /// Kernel dills placed with this tree's URI prefix (a cross-tree
    /// hit; [`crate::types::RewriteKind::DillUris`]) — rewritten, or
    /// cloned from a placement this run already rewrote from the same
    /// blob (`app.dill` and the bundle's `kernel_blob.bin`).
    pub dill_rewrites: u64,
    /// Test kernel-cache dills placed verbatim because the URI rewriter
    /// refused them (version gate, non-ASCII match, unknown layout):
    /// frontend_server's initialize-from-dill validation re-resolves
    /// their URIs, at the cost of a recompile.
    pub dill_rewrite_refused: u64,
    /// Payload dills (`app.dill`, `kernel_blob.bin`) the URI rewriter
    /// refused, left out of the tree. Verbatim, such a dill would carry
    /// the ingest tree's URIs past flutter's FileStore validation (its
    /// md5 is the recorded one) into this tree's artifacts; missing, it
    /// makes flutter rerun the target that produces it.
    pub dill_omitted: u64,
    /// `.filecache` md5 records re-aimed at the bytes this run
    /// rewrote (see [`patch_file_store`]).
    pub filecache_patched: u64,
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
/// The warm no-op comes first: placement only ever fills missing files,
/// so when the tree already holds every recorded file (a same-tree
/// rerun) there is nothing to do — no store access, no blob check, no
/// placement — and the run reports [`MaterializeReport::intact`]. The
/// check is one stat per recorded file. Files the tree holds are never
/// replaced on any path, so skipping is exactly the full path's effect;
/// whether their bytes still match is flutter's own content validation
/// to decide, as it always was.
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
/// than a cache miss — after removing it from the store, so the
/// rebuild's ingest re-stores the true bytes. Because verification
/// completes before the first placement, a corrupt store blob costs a
/// fully clean miss, not a half-filled dir. Bookkeeping files get their
/// embedded ingest-tree paths rewritten to target-tree paths (root
/// segments, then `flutter_build/<name>` segments) before the run
/// reports success; without that rewrite the skip would not survive the
/// tree move. A `.filecache` the run places lands last, carrying the
/// md5 of every file the run rewrote (see [`patch_file_store`]).
pub fn materialize(
    store: &CasStore,
    plan: &MaterializePlan,
    project_root: &Path,
) -> Result<MaterializeOutcome> {
    // The replacement root must be usable as one — non-empty, absolute,
    // and canonicalizable — before any rewrite may serve it. A root
    // that is not (nothing in the orchestrator's flow should produce
    // one) degrades the plan to verbatim placement: a rewrite onto it
    // could not be right, while verbatim files merely cost flutter its
    // own content validation and a rebuild of what it cannot recognize.
    let degraded;
    let plan = if usable_root(&plan.new_root) {
        plan
    } else {
        degraded = verbatim_plan(plan);
        &degraded
    };
    validate_plan(plan)?;
    // The warm no-op: fill-only-missing has nothing to fill when the
    // tree already holds every recorded file — the common same-tree
    // rerun — so the run ends before the store is touched.
    let missing = missing_files(plan, project_root);
    if missing.is_empty() {
        return Ok(MaterializeOutcome {
            report: MaterializeReport {
                intact: true,
                skipped_existing: plan.dirs.iter().map(|dir| dir.files.len() as u64).sum(),
                ..MaterializeReport::default()
            },
            placed: vec![Vec::new(); plan.dirs.len()],
        });
    }
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
    let sources: Vec<PathBuf> = missing
        .iter()
        .map(|(dir_at, file_at)| store.blob_path(&plan.dirs[*dir_at].files[*file_at].blob))
        .collect();
    verify_missing_blobs(store, plan, &missing, &sources)?;

    let mut run = Run {
        report: MaterializeReport::default(),
        // Only a FileStore record this run places needs the md5s of the
        // byte-changing placements; a tree that keeps its own record
        // pays for none.
        want_md5: missing
            .iter()
            .any(|&(dir_at, file_at)| is_file_store(&plan.dirs[dir_at].files[file_at].path)),
        dills: FxHashMap::default(),
    };
    let mut placed: Vec<Vec<PlacedFile>> = vec![Vec::new(); plan.dirs.len()];
    // The md5 of every byte-changing placement under both spellings a
    // FileStore record keys files by: absolute (build-dir outputs) and
    // project-relative (asset outputs).
    let mut rewritten: FxHashMap<PathBuf, String> = FxHashMap::default();
    // FileStore records land last, once every md5 they must carry is
    // known — across dirs, too: an asset dir's `kernel_blob.bin` is
    // tracked by its hash dir's record.
    let mut deferred: Vec<(usize, usize)> = Vec::new();

    for (dir_at, dir) in plan.dirs.iter().enumerate() {
        let target = project_root.join(&dir.path);
        fs::create_dir_all(&target)?;
        for (file_at, file) in dir.files.iter().enumerate() {
            if target.join(&file.path).exists() {
                // Fill-only-missing: the tree's own copy stays. Its
                // identity is unknown to this run, so nothing may
                // vouch for it in the post-run scan.
                run.report.skipped_existing += 1;
                continue;
            }
            if is_file_store(&file.path) {
                deferred.push((dir_at, file_at));
                continue;
            }
            let Some((placed_file, md5)) =
                materialize_one(store, plan, dir, file, &target, &mut run)?
            else {
                continue;
            };
            if let Some(md5) = md5 {
                rewritten.insert(target.join(&file.path), md5.clone());
                rewritten.insert(dir.path.join(&file.path), md5);
            }
            placed[dir_at].push(placed_file);
        }
    }
    run.want_md5 = false;
    for (dir_at, file_at) in deferred {
        let dir = &plan.dirs[dir_at];
        let file = &dir.files[file_at];
        let target = project_root.join(&dir.path);
        let Some((mut placed_file, _)) =
            materialize_one(store, plan, dir, file, &target, &mut run)?
        else {
            continue;
        };
        let path = target.join(&file.path);
        let patched = patch_file_store(&path, &rewritten)?;
        if patched > 0 {
            let metadata = fs::metadata(&path)?;
            run.report.filecache_patched += patched;
            run.report.total_bytes =
                run.report.total_bytes - placed_file.stamp.size + metadata.len();
            placed_file.stamp = stamp_of(&metadata);
            // The patch changed the bytes again: the record carries the
            // hash of what finally landed (KBs).
            placed_file.on_disk = Some(file_blake3(&path)?);
        }
        placed[dir_at].push(placed_file);
    }
    Ok(MaterializeOutcome {
        report: run.report,
        placed,
    })
}

/// The mutable state of one materialization run.
struct Run {
    report: MaterializeReport,
    /// Whether byte-changing placements must report their md5: set
    /// while a FileStore record this run places is still pending.
    want_md5: bool,
    /// Kernel dills this run rewrote, by source blob: the placed path
    /// and its md5 (when wanted). A payload replayed twice — `app.dill`
    /// and the bundle's `kernel_blob.bin` are the same bytes — is
    /// rewritten once and cloned after.
    dills: FxHashMap<BlobId, (PathBuf, Option<String>)>,
}

/// Whether `root` can serve as a rewrite target: non-empty, absolute,
/// and canonicalizable (it names the target tree's worktree root,
/// which exists by construction — anything else is a malformed or
/// foreign plan best placed verbatim).
fn usable_root(root: &Path) -> bool {
    root.as_os_str().len() > 0 && root.is_absolute() && fs::canonicalize(root).is_ok()
}

/// A copy of `plan` with every rewrite stripped: all files verbatim,
/// no name recomputation. The verbatim-fallback shape of a plan whose
/// replacement root is not usable (see [`materialize`]).
fn verbatim_plan(plan: &MaterializePlan) -> MaterializePlan {
    MaterializePlan {
        dirs: plan
            .dirs
            .iter()
            .map(|dir| DirPlan {
                path: dir.path.clone(),
                name_rewrite: None,
                files: dir
                    .files
                    .iter()
                    .map(|file| ManifestFile {
                        rewrite: RewriteKind::None,
                        ..file.clone()
                    })
                    .collect(),
            })
            .collect(),
        old_root: plan.old_root.clone(),
        new_root: plan.new_root.clone(),
    }
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
/// materialization with [`BcacheError::Corrupt`], whose message says
/// whether the removal took: one that fails leaves every hit of the
/// entry a miss until the blob is deleted. The running usage counter
/// keeps the removed bytes until the next gc pass rewrites it.
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
            let what = format!(
                "blob {} for {} failed its content check",
                blob_id_hex(&file.blob),
                file.path.display()
            );
            // Already gone (a concurrent heal or gc) is as good as removed.
            return Err(BcacheError::Corrupt(
                match fs::remove_file(store.blob_path(&file.blob)) {
                    Err(e) if e.kind() != std::io::ErrorKind::NotFound => format!(
                        "{what}, and removing it from the store failed ({e}): every hit of \
                         this entry misses until it is deleted"
                    ),
                    _ => format!(
                        "{what} and was removed from the store; the next build re-stores it"
                    ),
                },
            ));
        }
    }
    Ok(())
}

/// Places one manifest file — presence and integrity already verified
/// by [`materialize`]'s upfront pass — into its materialized dir and
/// reports it with the identity it landed under, plus the md5 of the
/// landed bytes when the run wants it ([`Run::want_md5`]) and they are
/// not the blob's: the hash a FileStore record must carry for them.
/// `None` when the file is deliberately left out (a refused payload
/// dill, see [`MaterializeReport::dill_omitted`]).
fn materialize_one(
    store: &CasStore,
    plan: &MaterializePlan,
    dir: &DirPlan,
    file: &ManifestFile,
    dir_path: &Path,
    run: &mut Run,
) -> Result<Option<(PlacedFile, Option<String>)>> {
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
    // The md5 of the landed bytes, when a reused rewrite already knows
    // it.
    let mut md5: Option<String> = None;
    // Whether this placement ran the dill rewriter (and so seeds the
    // reuse memo for the same blob placed again).
    let mut fresh_dill = false;

    // A kernel dill replayed into a different tree carries the ingest
    // tree's `file://` URIs, which frontend_server cannot resolve
    // here — rewrite the prefixes while the bytes stream into the
    // temp. A refused rewrite of a test kernel-cache dill falls through
    // to the verbatim clone (frontend_server's initialize-from-dill
    // validation re-resolves its URIs); a refused payload dill is left
    // out instead (see [`MaterializeReport::dill_omitted`]). A same-root
    // plan keeps the verbatim clone.
    let mut placed = false;
    if matches!(file.rewrite, RewriteKind::DillUris) && plan.roots_differ() {
        if let Some((earlier, earlier_md5)) = run.dills.get(&file.blob) {
            // The same payload this run already rewrote for this tree:
            // the rewrite is deterministic, so its placement is cloned.
            clone_file(earlier, &tmp)?;
            md5.clone_from(earlier_md5);
            run.report.dill_rewrites += 1;
            placed = true;
            verbatim = false;
        } else {
            let mut from = b"file://".to_vec();
            from.extend_from_slice(plan.old_root.as_bytes());
            from.push(b'/');
            let mut to = b"file://".to_vec();
            to.extend_from_slice(plan.new_root.as_os_str().as_encoded_bytes());
            to.push(b'/');
            match dill_rewrite::rewrite_file(&src, &tmp, &from, &to) {
                Ok(_) => {
                    run.report.dill_rewrites += 1;
                    placed = true;
                    verbatim = false;
                    fresh_dill = true;
                }
                Err(_) => {
                    let _ = fs::remove_file(&tmp);
                    let payload = file
                        .path
                        .file_name()
                        .and_then(|name| name.to_str())
                        .is_some_and(crate::ingest_scan::is_payload_dill);
                    if payload {
                        run.report.dill_omitted += 1;
                        return Ok(None);
                    }
                    run.report.dill_rewrite_refused += 1;
                }
            }
        }
    }

    if !placed {
        match clone_file(&src, &tmp)? {
            CloneStatus::Cloned => run.report.cloned += 1,
            CloneStatus::Copied => run.report.copied += 1,
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
        // Every escaped spelling of the ingest root (depfile `\ `,
        // JSON `\\`, both) is rewritten to the same spelling of the
        // target root; a spelling that escapes nothing is the plain
        // pair itself and is not repeated.
        let mut pairs: Vec<(Vec<u8>, Vec<u8>)> = vec![(old_root.to_vec(), new_root.to_vec())];
        for (escaped_old, escaped_new) in std::iter::zip(
            crate::ingest_scan::escaped_forms(old_root),
            crate::ingest_scan::escaped_forms(new_root),
        ) {
            if escaped_old != old_root {
                pairs.push((escaped_old, escaped_new));
            }
        }
        let pairs: Vec<(&[u8], &[u8])> = pairs
            .iter()
            .map(|(old, new)| (old.as_slice(), new.as_slice()))
            .collect();
        let mut occurrences = rewrite_bookkeeping(&dst, &pairs)?;
        if let Some((old_name, new_name)) = &dir.name_rewrite {
            occurrences += rewrite_name_segments(&dst, old_name, new_name)?;
        }
        if occurrences > 0 {
            run.report.rewritten_files += 1;
            run.report.rewritten_occurrences += occurrences as u64;
            if plan.roots_differ() {
                verbatim = false;
            }
        }
    }

    if file.executable {
        set_executable(&dst)?;
    }

    if run.want_md5 && !verbatim && md5.is_none() {
        md5 = Some(file_md5_hex(&dst)?);
    }
    if fresh_dill {
        run.dills.insert(file.blob, (dst.clone(), md5.clone()));
    }

    let metadata = fs::metadata(&dst)?;
    run.report.total_bytes += metadata.len();
    // What the record can prove about the landed bytes later: a
    // verbatim clone is the blob (identity-verified up front); a
    // re-rooted bookkeeping file is hashed here, at KB cost, so a
    // byte-identical flutter rewrite of it is recognizable; a re-rooted
    // kernel dill (tens of MB) is not — flutter rewriting one is a real
    // change that is hashed regardless (see [`PlacedFile::on_disk`]).
    let on_disk = if verbatim {
        Some(file.blob)
    } else if matches!(file.rewrite, RewriteKind::PathText) {
        Some(file_blake3(&dst)?)
    } else {
        None
    };
    let placed_file = PlacedFile {
        path: file.path.clone(),
        stamp: stamp_of(&metadata),
        blob: file.blob,
        rewrite: file.rewrite,
        foreign_paths: file.foreign_paths,
        on_disk,
    };
    Ok(Some((placed_file, md5)))
}

/// flutter_tools' FileStore record — `.filecache`, format version 2
/// (flutter_tools 3.47.1 `build_system/file_store.dart`, `FileStorage`
/// / `FileHash`): every tracked input and output by path, with the md5
/// of its bytes as last hashed. Field order is flutter's own.
#[derive(Serialize, Deserialize)]
struct FileStorage {
    version: u32,
    files: Vec<FileHash>,
}

#[derive(Serialize, Deserialize)]
struct FileHash {
    path: String,
    hash: String,
}

/// The FileStore record's name in a build dir (`FileStore.kFileCache`).
const FILE_STORE: &str = ".filecache";

/// The one FileStore format this module patches (`FileStore._kVersion`).
const FILE_STORE_VERSION: u32 = 2;

fn is_file_store(path: &Path) -> bool {
    path == Path::new(FILE_STORE)
}

/// Streamed md5 of a placed file as lowercase hex — the digest a
/// FileStore record holds (`Digest.toString()` of package:crypto's
/// md5). A 1 MiB buffer keeps a ~100 MB dill from ever loading whole.
fn file_md5_hex(path: &Path) -> std::io::Result<String> {
    use md5::{Digest, Md5};

    let mut file = fs::File::open(path)?;
    let mut hasher = Md5::new();
    let mut buf = vec![0u8; 1024 * 1024];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

/// Re-aims the md5 records of a FileStore record this run placed at
/// the bytes this run rewrote, keyed by the paths `rewritten` names.
///
/// flutter_tools 3.47.1 `Node.computeChanges` (build_system.dart)
/// diffs every tracked input and output through
/// `FileStore.diffFileList` — `_hashFile` md5s the file and marks it
/// dirty on any mismatch with the record (file_store.dart) — and reruns
/// the target. The record replays the INGEST tree's md5s, so without
/// this patch every byte-changing placement reads as changed output:
/// the cross-tree dill rewrite alone reran `kernel_snapshot_program`
/// (and with it `copy_flutter_bundle`) on every fresh-tree hit. The
/// patch is sound because the record and the patched files are one
/// recorded build: it only ever runs on a record this run placed, for
/// files this run placed, so every tracked INPUT keeps its recorded
/// md5 and flutter still rebuilds whatever this tree's sources change.
///
/// A record that does not parse as format version 2 is left as placed:
/// flutter rebuilds what it cannot validate — the cost of a miss, never
/// a wrong skip. Returns the number of records patched.
fn patch_file_store(path: &Path, rewritten: &FxHashMap<PathBuf, String>) -> Result<u64> {
    if rewritten.is_empty() {
        return Ok(0);
    }
    // Bounded by construction: the ingest refuses bookkeeping files
    // past REWRITE_LIMIT.
    let data = fs::read(path)?;
    let Ok(mut storage) = serde_json::from_slice::<FileStorage>(&data) else {
        return Ok(0);
    };
    if storage.version != FILE_STORE_VERSION {
        return Ok(0);
    }
    let mut patched = 0u64;
    for file in &mut storage.files {
        if let Some(md5) = rewritten.get(Path::new(&file.path))
            && file.hash != *md5
        {
            file.hash.clone_from(md5);
            patched += 1;
        }
    }
    if patched > 0 {
        let bytes = serde_json::to_vec(&storage)
            .map_err(|err| BcacheError::Invalid(format!("{}: {err}", path.display())))?;
        let tmp = tmp_sibling(path);
        fs::write(&tmp, &bytes)?;
        fs::rename(&tmp, path)?;
    }
    Ok(patched)
}

/// Applies `(old, new)` byte replacements to one bookkeeping file.
/// Every replacement sits at a path boundary (see
/// [`replace_at_path_boundary`]). The file is fully loaded under
/// [`REWRITE_LIMIT`], patched in memory, and written back via
/// tmp+rename in the same directory. Returns the number of
/// replacements; zero means the file already carried the target
/// spelling and is left untouched on disk.
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
        occurrences += replace_at_path_boundary(&mut data, old, new);
    }
    if occurrences > 0 {
        let tmp = tmp_sibling(path);
        fs::write(&tmp, &data)?;
        fs::rename(&tmp, path)?;
    }
    Ok(occurrences)
}

/// Replaces every occurrence of `old` in `data` with `new` that sits
/// at a path boundary: the data ends at the match, or a `/` follows —
/// the spellings where the matched bytes ARE the root (a descendant
/// path, or the root itself at the data's end). A following byte that
/// could extend the last path component (`/old` inside `/old-x`, a
/// sibling directory) is left alone: rewriting it would corrupt a path
/// this plan has no authority over, while missing it only costs
/// flutter a rebuild of the one file that mentioned it.
fn replace_at_path_boundary(data: &mut Vec<u8>, old: &[u8], new: &[u8]) -> usize {
    if old.is_empty() {
        return 0;
    }
    let finder = memmem::Finder::new(old);
    let mut cursor = 0usize;
    let mut count = 0usize;
    let mut out = Vec::with_capacity(data.len());
    while let Some(pos) = finder.find(&data[cursor..]) {
        let at = cursor + pos;
        let end = at + old.len();
        out.extend_from_slice(&data[cursor..at]);
        if data.get(end).is_none_or(|&byte| byte == b'/') {
            out.extend_from_slice(new);
            count += 1;
        } else {
            out.extend_from_slice(old);
        }
        cursor = end;
    }
    out.extend_from_slice(&data[cursor..]);
    if count > 0 {
        *data = out;
    }
    count
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
        let boundary = data.get(end).is_none_or(|&byte| !byte.is_ascii_hexdigit());
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
        .map(|n| format!(".{}.{}.dartq-tmp", n.to_string_lossy(), std::process::id()))
        .unwrap_or_else(|| format!(".{}.dartq-tmp", std::process::id()));
    path.with_file_name(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{
        BlobId, DirRecord, EntryManifest, NameRule, TargetSpec, ToolchainFingerprint,
    };
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
        assert_eq!(
            rewrite_bytes(&mut data, b"/old/root", b"/much/longer/root"),
            1
        );
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
        let new_root_dir = tempfile::tempdir().expect("new root dir");
        let new_root = new_root_dir.path().to_path_buf();
        let stamp = format!("[\"{old_root}/lib/main.dart\"]");
        let stamp_id = place_blob(&store, store_dir.path(), "src-stamp", stamp.as_bytes());

        let plan = MaterializePlan {
            dirs: vec![hash_dir(
                "abc123",
                vec![
                    manifest("app.dill", payload_id, false, RewriteKind::None),
                    manifest(
                        "flutter_build.dill.stamp",
                        stamp_id,
                        false,
                        RewriteKind::PathText,
                    ),
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
        assert_eq!(
            fs::read(hash_dir.join("app.dill")).expect("read dill"),
            payload
        );

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
        let new_root_dir = tempfile::tempdir().expect("new root dir");
        let new_root = new_root_dir.path().to_path_buf();
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
        let expected_stamp = format!(r#"["{}/lib/main.dart"]"#, new_root.display());
        assert_eq!(
            report.total_bytes,
            (expected_stamp.len() + b"kernel payload".len() + b"asset payload".len()) as u64
        );

        let one = build_root.join("dir0000000000000000000000000001");
        let two = build_root.join("dir0000000000000000000000000002");
        assert_eq!(
            fs::read_to_string(one.join("k.stamp")).expect("kernel stamp"),
            expected_stamp
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
            .filter(|entry| entry.file_name().to_string_lossy().contains("bcache-stale"))
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
                manifest(
                    "flutter_build.dill.stamp",
                    stamp_id,
                    false,
                    RewriteKind::None,
                ),
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
            &plan(vec![manifest(
                "gone.bin",
                missing_id,
                false,
                RewriteKind::None,
            )]),
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
            vec![manifest(
                "flutter_build.dill.stamp",
                stamp_id,
                false,
                RewriteKind::PathText,
            )],
        );
        dir.name_rewrite = Some((old_name.to_owned(), new_name.to_owned()));
        let new_root_dir = tempfile::tempdir().expect("new root dir");
        let plan = MaterializePlan {
            dirs: vec![dir],
            old_root: "/old/root".to_string(),
            new_root: new_root_dir.path().to_path_buf(),
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
            placed.contains(&format!(
                "{}/.dart_tool/flutter_build/{new_name}/app.dill",
                new_root_dir.path().display()
            )),
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
            placed.contains(
                "flutter_build/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaadeadbeefdeadbeefdeadbeef00"
            ),
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

    /// The self-heal loop: a stored blob damaged in place at its own size
    /// (the case ingest's reuse rule never rereads) fails the content
    /// check and is removed, so the hit is a clean miss and a retry before
    /// the rebuild misses exactly like a missing blob; the rebuild's
    /// re-ingest of the same source then takes the fresh path and lands
    /// the true bytes, and the next hit materializes them.
    #[test]
    fn materialize_quarantines_a_corrupt_blob_and_reingest_heals_it() {
        let store_dir = tempfile::tempdir().expect("store dir");
        let store = CasStore::open(store_dir.path()).expect("open store");
        let original: &[u8] = b"original payload bytes";
        let payload_id = place_blob(&store, store_dir.path(), "src-payload.bin", original);
        let plan = MaterializePlan {
            dirs: vec![hash_dir(
                "abc123",
                vec![manifest("app.dill", payload_id, false, RewriteKind::None)],
            )],
            old_root: "/old/root".to_string(),
            new_root: PathBuf::from("/new/root"),
        };
        fs::write(store.blob_path(&payload_id), b"corrupted payload byte").expect("tamper blob");

        let project = tempfile::tempdir().expect("project dir");
        let err = materialize(&store, &plan, project.path()).expect_err("corrupt blob must abort");
        assert!(
            matches!(&err, BcacheError::Corrupt(message) if message.contains("removed from the store")),
            "the shim's note reports the removal: {err}"
        );
        assert!(
            !store.blob_path(&payload_id).exists(),
            "the corrupt blob is quarantined"
        );
        assert!(
            !project.path().join(".dart_tool").exists(),
            "the miss never touches the tree"
        );
        assert!(
            matches!(
                materialize(&store, &plan, project.path()),
                Err(BcacheError::NotFound(_))
            ),
            "until the rebuild, the hit misses like any missing blob"
        );

        // The rebuild's ingest re-stores the source: with the blob absent
        // the fresh path runs, and the true bytes land under the same id.
        assert_eq!(
            place_blob(&store, store_dir.path(), "src-payload.bin", original),
            payload_id
        );
        assert_eq!(
            crate::fsutil::file_blake3(&store.blob_path(&payload_id)).expect("hash healed blob"),
            payload_id
        );

        let outcome = materialize(&store, &plan, project.path()).expect("the healed entry hits");
        assert_eq!(outcome.report.cloned + outcome.report.copied, 1);
        let placed = project
            .path()
            .join(".dart_tool/flutter_build/abc123/app.dill");
        assert_eq!(fs::read(&placed).expect("read placed file"), original);
    }

    /// Intact blobs pass the content check and stay in the store: only a
    /// failed check removes anything.
    #[test]
    fn materialize_keeps_intact_blobs_in_the_store() {
        let store_dir = tempfile::tempdir().expect("store dir");
        let store = CasStore::open(store_dir.path()).expect("open store");
        let payload = place_blob(&store, store_dir.path(), "payload.bin", b"payload bytes");
        let stamp = place_blob(&store, store_dir.path(), "stamp.json", b"{\"stamp\":1}");
        let plan = MaterializePlan {
            dirs: vec![hash_dir(
                "abc123",
                vec![
                    manifest("app.dill", payload, false, RewriteKind::None),
                    manifest("stamp.json", stamp, false, RewriteKind::None),
                ],
            )],
            old_root: "/old/root".to_string(),
            new_root: PathBuf::from("/new/root"),
        };

        let project = tempfile::tempdir().expect("project dir");
        let outcome = materialize(&store, &plan, project.path()).expect("intact blobs hit");
        assert_eq!(outcome.report.cloned + outcome.report.copied, 2);
        for id in [payload, stamp] {
            assert!(
                store.blob_path(&id).is_file(),
                "blob {} stays stored",
                blob_id_hex(&id)
            );
        }
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
                vec![manifest(
                    "app.dill",
                    BlobId([9; 32]),
                    false,
                    RewriteKind::None,
                )],
            )],
            old_root: "/old/root".to_string(),
            new_root: PathBuf::from("/new/root"),
        };

        plan.old_root = "/old/back\\slash".to_string();
        let err =
            materialize(&store, &plan, project.path()).expect_err("backslash must be Invalid");
        assert!(matches!(err, BcacheError::Invalid(_)), "got {err:?}");

        plan.old_root = String::new();
        let err =
            materialize(&store, &plan, project.path()).expect_err("empty root must be Invalid");
        assert!(matches!(err, BcacheError::Invalid(_)), "got {err:?}");

        plan.old_root = "/old/root".to_string();
        plan.dirs[0].path = PathBuf::from("../escape");
        let err =
            materialize(&store, &plan, project.path()).expect_err("traversal must be Invalid");
        assert!(matches!(err, BcacheError::Invalid(_)), "got {err:?}");

        plan.dirs[0].path = PathBuf::from("/absolute/dir");
        let err = materialize(&store, &plan, project.path()).expect_err("absolute must be Invalid");
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

        assert_eq!(
            report.skipped_existing, 0,
            "the pre-existing foreign files are not recorded files"
        );
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
            .filter(|entry| entry.file_name().to_string_lossy().contains("bcache-stale"))
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
                manifest(
                    "build/cd.cache.dill",
                    garbage_id,
                    false,
                    RewriteKind::DillUris,
                ),
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
        assert_eq!(
            report.cloned + report.copied,
            1,
            "only the refused dill clones"
        );

        let placed = fs::read(project.path().join("build/test_cache/build/ab.cache.dill"))
            .expect("placed dill");
        let new_uri = format!("file://{}/lib/main.dart", new_root.display());
        assert!(
            memmem::find(&placed, new_uri.as_bytes()).is_some(),
            "URI rewritten"
        );
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
                    manifest(
                        "build/cd.cache.dill",
                        garbage_id,
                        false,
                        RewriteKind::DillUris,
                    ),
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
            fs::read(
                same_project
                    .path()
                    .join("build/test_cache/build/ab.cache.dill")
            )
            .expect("verbatim dill"),
            dill,
            "same-root replay is byte-identical"
        );
    }

    /// A payload dill the rewriter refuses never lands cross-tree: its
    /// recorded md5 would pass flutter's FileStore validation with the
    /// ingest tree's URIs inside, so it is left out (counted) and
    /// flutter reruns the target that produces it. The dir's other
    /// files still land; a same-root replay places it verbatim.
    #[test]
    fn refused_payload_dill_is_left_out_cross_tree() {
        let store_dir = tempfile::tempdir().expect("store dir");
        let store = CasStore::open(store_dir.path()).expect("open store");
        let garbage_id = place_blob(&store, store_dir.path(), "src-garbage", b"not a dill");
        let stamp_id = place_blob(&store, store_dir.path(), "src-stamp", b"{}");
        let name = "dir000000000000000000000000000000a";
        let files = || {
            vec![
                manifest("app.dill", garbage_id, false, RewriteKind::DillUris),
                manifest(
                    "kernel_snapshot_program.stamp",
                    stamp_id,
                    false,
                    RewriteKind::None,
                ),
            ]
        };

        let project = tempfile::tempdir().expect("project dir");
        let plan = MaterializePlan {
            dirs: vec![hash_dir(name, files())],
            old_root: "/old/canonical/root".to_string(),
            new_root: fs::canonicalize(project.path()).expect("canonical root"),
        };
        let MaterializeOutcome { report, placed } =
            materialize(&store, &plan, project.path()).expect("materialize");
        assert_eq!(report.dill_omitted, 1);
        assert_eq!(report.dill_rewrite_refused, 0);
        assert_eq!(report.dill_rewrites, 0);
        let dir = project.path().join(".dart_tool/flutter_build").join(name);
        assert!(
            !dir.join("app.dill").exists(),
            "the refused payload dill is left out"
        );
        assert!(dir.join("kernel_snapshot_program.stamp").is_file());
        assert_eq!(placed[0].len(), 1, "only the stamp is a placement");

        let same_project = tempfile::tempdir().expect("same-root project dir");
        let same_root = fs::canonicalize(same_project.path()).expect("canonical root");
        let same = MaterializePlan {
            dirs: vec![hash_dir(name, files())],
            old_root: same_root.to_string_lossy().into_owned(),
            new_root: same_root,
        };
        let MaterializeOutcome { report, .. } =
            materialize(&store, &same, same_project.path()).expect("materialize");
        assert_eq!(report.dill_omitted, 0);
        assert_eq!(
            fs::read(
                same_project
                    .path()
                    .join(".dart_tool/flutter_build")
                    .join(name)
                    .join("app.dill")
            )
            .expect("same-root payload"),
            b"not a dill".to_vec(),
            "a same-root replay places the payload verbatim"
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
        let old_root_dir = tempfile::tempdir().expect("old root dir");
        let old_root = old_root_dir.path().to_string_lossy().into_owned();
        let stamp = format!("[\"{old_root}/lib/main.dart\"]");
        let stamp_id = place_blob(&store, store_dir.path(), "stamp", stamp.as_bytes());
        let files = vec![
            manifest("app.dill", payload_id, false, RewriteKind::None),
            manifest(
                "flutter_build.dill.stamp",
                stamp_id,
                false,
                RewriteKind::PathText,
            ),
        ];

        // Cross-tree: the PathText stamp lands with the new root inside,
        // so it is NOT its blob's bytes; the payload clone is.
        let project = tempfile::tempdir().expect("project dir");
        let new_root_dir = tempfile::tempdir().expect("new root dir");
        let plan = MaterializePlan {
            dirs: vec![hash_dir("abc123", files.clone())],
            old_root: old_root.clone(),
            new_root: new_root_dir.path().to_path_buf(),
        };
        let outcome = materialize(&store, &plan, project.path()).expect("materialize");
        assert_eq!(outcome.placed.len(), 1, "one PlacedFile vec per plan dir");
        let placed = &outcome.placed[0];
        assert_eq!(placed.len(), 2);

        let dill = placed
            .iter()
            .find(|f| f.path == Path::new("app.dill"))
            .expect("placed dill");
        assert!(dill.verbatim(), "a payload clone is the blob's bytes");
        assert_eq!(dill.blob, payload_id);
        assert_eq!(
            dill.stamp,
            crate::fsutil::stamp_of(
                &fs::metadata(
                    project
                        .path()
                        .join(".dart_tool/flutter_build/abc123/app.dill")
                )
                .expect("metadata")
            ),
            "the fresh identity of the placed file"
        );

        let stamp_file = placed
            .iter()
            .find(|f| f.path == Path::new("flutter_build.dill.stamp"))
            .expect("placed stamp");
        assert!(
            !stamp_file.verbatim(),
            "a cross-tree rewrite changed the bytes"
        );
        assert_eq!(stamp_file.rewrite, RewriteKind::PathText);
        assert_eq!(
            stamp_file.on_disk,
            Some(
                file_blake3(
                    &project
                        .path()
                        .join(".dart_tool/flutter_build/abc123/flutter_build.dill.stamp")
                )
                .expect("hash placed stamp")
            ),
            "the record carries the hash of the re-rooted bytes that landed"
        );

        // Same-root: the rewrite replaces the root onto itself — bytes
        // unchanged — so every placement stays verbatim. The root must
        // be a real directory: an unusable one would degrade to verbatim
        // placement for a different reason entirely.
        let same = MaterializePlan {
            dirs: vec![hash_dir("abc123", files)],
            old_root: old_root.clone(),
            new_root: PathBuf::from(&old_root),
        };
        let outcome = materialize(&store, &same, project.path()).expect("materialize");
        assert!(
            outcome.placed[0].iter().all(PlacedFile::verbatim),
            "same-root placements keep the blob's bytes"
        );
    }

    /// The #4069 bundle-face repro, synthetic end to end: payload dills
    /// (`app.dill`, `kernel_blob.bin`) whose kernel tables carry
    /// worktree URIs, machine-cache URIs, and synthetic `file://`
    /// string literals replay cross-tree with exactly the worktree
    /// prefixes rewritten — the cache URIs and the literals are data
    /// the prefix-exact rewrite must pass through untouched.
    #[test]
    fn payload_dills_replay_cross_tree_with_only_worktree_uris_rewritten() {
        use crate::dill_rewrite::synthetic_component;

        let store_dir = tempfile::tempdir().expect("store dir");
        let store = CasStore::open(store_dir.path()).expect("open store");
        let root_a = tempfile::tempdir().expect("worktree-a dir");
        let a = root_a.path().to_string_lossy().into_owned();
        let pub_uri = "file:///pubs/hosted/pub.dev/widgets-1.0.0/lib/widgets.dart";
        let sdk_uri = "file:///sdks/flutter/bin/cache/artifacts/engine/skia/markup.dart";
        let literal_a = "file:///path/to/foo";
        let literal_b = "file:///root/path/a/b.dart";

        let app = synthetic_component(
            &[
                (
                    &format!("file://{a}/lib/main.dart"),
                    "package:app/main.dart",
                ),
                (
                    &format!("file://{a}/examples/demo_app/lib/src/panel.dart"),
                    "package:demo/src/panel.dart",
                ),
                (pub_uri, "package:widgets/widgets.dart"),
                (sdk_uri, "package:skia/markup.dart"),
            ],
            &[literal_a, literal_b],
            4,
        );
        let app_id = place_blob(&store, store_dir.path(), "app-dill", &app);
        let blob = synthetic_component(
            &[(
                &format!("file://{a}/lib/main.dart"),
                "package:app/main.dart",
            )],
            &[literal_a],
            1,
        );
        let blob_id = place_blob(&store, store_dir.path(), "kernel-blob", &blob);

        let root_b = tempfile::tempdir().expect("worktree-b dir");
        let plan = MaterializePlan {
            dirs: vec![hash_dir(
                "dir0000000000000000000000000000009",
                vec![
                    manifest("app.dill", app_id, false, RewriteKind::DillUris),
                    manifest("kernel_blob.bin", blob_id, false, RewriteKind::DillUris),
                ],
            )],
            old_root: a.clone(),
            new_root: root_b.path().to_path_buf(),
        };
        let project = tempfile::tempdir().expect("project dir");
        let MaterializeOutcome { report, .. } =
            materialize(&store, &plan, project.path()).expect("materialize");
        assert_eq!(report.dill_rewrites, 2, "both payload dills rewrite");
        assert_eq!(report.dill_rewrite_refused, 0);

        let b = root_b.path().to_string_lossy().into_owned();
        let dir = project
            .path()
            .join(".dart_tool/flutter_build/dir0000000000000000000000000000009");
        let placed_app = fs::read(dir.join("app.dill")).expect("placed app.dill");
        assert!(
            memmem::find(
                placed_app.as_slice(),
                format!("file://{b}/lib/main.dart").as_bytes()
            )
            .is_some()
        );
        assert!(
            memmem::find(
                placed_app.as_slice(),
                format!("file://{b}/examples/demo_app/lib/src/panel.dart").as_bytes()
            )
            .is_some(),
            "every worktree URI is rewritten to the target root"
        );
        // The old root survives exactly as the two embedded source
        // TEXTS (diagnostics-only, verbatim by design).
        assert_eq!(
            memmem::find_iter(&placed_app, format!("file://{a}/").as_bytes()).count(),
            2
        );
        assert!(memmem::find(&placed_app, pub_uri.as_bytes()).is_some());
        assert!(memmem::find(&placed_app, sdk_uri.as_bytes()).is_some());
        assert!(memmem::find(&placed_app, literal_a.as_bytes()).is_some());
        assert!(
            memmem::find(&placed_app, literal_b.as_bytes()).is_some(),
            "synthetic literals pass through untouched"
        );

        let placed_blob = fs::read(dir.join("kernel_blob.bin")).expect("placed kernel_blob");
        assert!(
            memmem::find(&placed_blob, format!("file://{b}/lib/main.dart").as_bytes()).is_some()
        );
        assert!(memmem::find(&placed_blob, literal_a.as_bytes()).is_some());
    }

    /// A14's temp-name half: rename-atomic temps carry the writing
    /// process's pid, so two shims materializing into one tree never
    /// target the same temp path.
    #[test]
    fn tmp_sibling_carries_the_writer_pid() {
        let a = tmp_sibling(Path::new("/tree/.dart_tool/flutter_build/x/app.dill"));
        let b = tmp_sibling(Path::new(
            "/tree/.dart_tool/flutter_build/x/kernel_snapshot.d",
        ));
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

    /// The root replacement honors path boundaries: a descendant path
    /// and the root at the data's end are rewritten, a sibling whose
    /// name merely starts with the root (`/old/root-lookalike`) keeps
    /// its bytes — rewriting it would corrupt a path this plan has no
    /// authority over.
    #[test]
    fn root_replacement_respects_path_boundaries() {
        let mut data = b"/old/root/lib/a.dart /old/root-lookalike/b.dart /old/root".to_vec();
        let count = replace_at_path_boundary(&mut data, b"/old/root", b"/new/root");
        assert_eq!(count, 2, "descendant + data end, never the sibling");
        assert_eq!(
            data,
            b"/new/root/lib/a.dart /old/root-lookalike/b.dart /new/root".to_vec()
        );

        // No boundary case left behind by an empty needle.
        let mut data = b"stable".to_vec();
        assert_eq!(replace_at_path_boundary(&mut data, b"", b"/x"), 0);
        assert_eq!(data, b"stable".to_vec());
    }

    /// The escaped spellings of the ingest root are rewritten to the
    /// same spelling of the target root: a depfile's `\ ` form (the
    /// audit's escaped-root finding) survives a cross-tree replay, and
    /// a replacement root that itself needs escaping is written
    /// escaped. Both roots are real directories — a replacement root
    /// that does not exist degrades to verbatim placement.
    #[test]
    fn materialize_rewrites_the_depfiles_escaped_root_spelling() {
        let store_dir = tempfile::tempdir().expect("store dir");
        let store = CasStore::open(store_dir.path()).expect("open store");
        let old_root = tempfile::Builder::new()
            .prefix("old ro ot")
            .tempdir()
            .expect("old root dir");
        let old_root = old_root.path().to_string_lossy().into_owned();
        let escaped_old = old_root.replace(' ', "\\ ");

        // The depfile form against a plain target root.
        let depfile = format!("{escaped_old}/build/app.dill: {escaped_old}/lib/main.dart\n");
        let depfile_id = place_blob(&store, store_dir.path(), "depfile", depfile.as_bytes());
        // The same form against a target root that needs escaping too.
        let json = format!(r#"["{escaped_old}/lib/main.dart"]"#);
        let json_id = place_blob(&store, store_dir.path(), "json", json.as_bytes());

        let new_root = tempfile::Builder::new()
            .prefix("new tr ee")
            .tempdir()
            .expect("new root dir");
        let new_root = new_root.path().to_path_buf();
        let escaped_new = new_root.to_string_lossy().replace(' ', "\\ ");
        let plan = MaterializePlan {
            dirs: vec![hash_dir(
                "abc123",
                vec![
                    manifest(
                        "kernel_snapshot.d",
                        depfile_id,
                        false,
                        RewriteKind::PathText,
                    ),
                    manifest("outputs.json", json_id, false, RewriteKind::PathText),
                ],
            )],
            old_root,
            new_root: new_root.clone(),
        };
        let project = tempfile::tempdir().expect("project dir");
        let MaterializeOutcome { report, .. } =
            materialize(&store, &plan, project.path()).expect("materialize");

        let dir = project.path().join(".dart_tool/flutter_build/abc123");
        let rewritten_depfile = fs::read_to_string(dir.join("kernel_snapshot.d")).expect("depfile");
        assert_eq!(
            rewritten_depfile,
            format!("{escaped_new}/build/app.dill: {escaped_new}/lib/main.dart\n"),
            "the escaped spelling is rewritten to the escaped target"
        );
        let rewritten_json = fs::read_to_string(dir.join("outputs.json")).expect("json");
        assert_eq!(
            rewritten_json,
            format!(r#"["{escaped_new}/lib/main.dart"]"#),
            "the escaped spelling rewrites inside JSON framing too"
        );
        assert_eq!(report.rewritten_occurrences, 3);
    }

    /// An unusable replacement root (non-empty, absolute, and
    /// canonicalizable are the bars) degrades the whole plan to
    /// verbatim placement instead of serving rewrites that cannot be
    /// right: every file lands as the blob's bytes, nothing is
    /// rewritten, and the placement reports verbatim.
    #[test]
    fn unusable_new_root_degrades_to_verbatim_placement() {
        assert!(!usable_root(Path::new("")), "empty");
        assert!(!usable_root(Path::new("relative/root")), "not absolute");
        assert!(
            !usable_root(Path::new("/no/such/root/anywhere")),
            "not canonicalizable"
        );
        assert!(usable_root(&std::env::current_dir().expect("cwd")), "valid");

        let store_dir = tempfile::tempdir().expect("store dir");
        let store = CasStore::open(store_dir.path()).expect("open store");
        let stamp = "[\"/old/root/lib/main.dart\"]";
        let stamp_id = place_blob(&store, store_dir.path(), "stamp", stamp.as_bytes());
        let old_name = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let new_name = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

        let project = tempfile::tempdir().expect("project dir");
        for unusable in ["", "relative/root", "/no/such/root/anywhere"] {
            let mut dir = hash_dir(
                new_name,
                vec![manifest("k.stamp", stamp_id, false, RewriteKind::PathText)],
            );
            dir.name_rewrite = Some((old_name.to_owned(), new_name.to_owned()));
            let plan = MaterializePlan {
                dirs: vec![dir],
                old_root: "/old/root".to_string(),
                new_root: PathBuf::from(unusable),
            };
            let outcome = materialize(&store, &plan, project.path()).expect("materialize");
            assert_eq!(
                outcome.report.rewritten_files, 0,
                "root {unusable:?}: nothing rewrites"
            );
            assert_eq!(
                outcome.report.rewritten_occurrences, 0,
                "root {unusable:?}: the name rewrite is stripped too"
            );
            assert!(
                outcome.placed[0]
                    .iter()
                    .all(|file| file.verbatim() && file.rewrite == RewriteKind::None),
                "root {unusable:?}: verbatim placements all the way down"
            );
            let placed_stamp = fs::read_to_string(
                project
                    .path()
                    .join(".dart_tool/flutter_build")
                    .join(new_name)
                    .join("k.stamp"),
            )
            .expect("stamp");
            assert_eq!(placed_stamp, stamp, "the blob's bytes, unchanged");
            // Clear the dir so the next unusable root places afresh.
            fs::remove_dir_all(project.path().join(".dart_tool")).expect("clean");
        }
    }

    /// The two-file plan of the warm no-op tests, and its target dir.
    fn warm_plan(store: &CasStore, store_dir: &Path) -> (MaterializePlan, BlobId) {
        let payload_id = place_blob(store, store_dir, "payload", b"payload bytes");
        let stamp_id = place_blob(store, store_dir, "stamp", b"stamp bytes");
        let plan = MaterializePlan {
            dirs: vec![hash_dir(
                "abc123",
                vec![
                    manifest("app.dill", payload_id, false, RewriteKind::None),
                    manifest(
                        "flutter_build.dill.stamp",
                        stamp_id,
                        false,
                        RewriteKind::None,
                    ),
                ],
            )],
            old_root: "/old/root".to_string(),
            new_root: PathBuf::from("/new/root"),
        };
        (plan, payload_id)
    }

    /// #4070 (a): a rerun over a tree that already holds every recorded
    /// file is a no-op — the store is never consulted (a blob gc removed
    /// would fail any run that reached the presence check), nothing is
    /// placed or rewritten, and the report says so.
    #[test]
    fn warm_rerun_over_a_complete_tree_never_touches_store_or_tree() {
        let store_dir = tempfile::tempdir().expect("store dir");
        let store = CasStore::open(store_dir.path()).expect("open store");
        let (plan, payload_id) = warm_plan(&store, store_dir.path());
        let project = tempfile::tempdir().expect("project dir");

        let cold = materialize(&store, &plan, project.path()).expect("cold placement");
        assert!(!cold.report.intact, "a fresh tree is filled");
        assert_eq!(cold.report.cloned + cold.report.copied, 2);

        let dir = project.path().join(".dart_tool/flutter_build/abc123");
        let stamps = || {
            ["app.dill", "flutter_build.dill.stamp"]
                .map(|name| stamp_of(&fs::metadata(dir.join(name)).expect("stat placed file")))
        };
        let before = stamps();
        fs::remove_file(store.blob_path(&payload_id)).expect("gc drops a blob");

        let warm = materialize(&store, &plan, project.path()).expect("the no-op needs no blob");
        assert!(warm.report.intact);
        assert_eq!(warm.report.skipped_existing, 2);
        assert_eq!(warm.report.cloned + warm.report.copied, 0);
        assert_eq!(warm.report.total_bytes, 0);
        assert_eq!(warm.placed.len(), 1, "one (empty) placed list per plan dir");
        assert!(warm.placed[0].is_empty());
        assert_eq!(stamps(), before, "no tree file was rewritten");
    }

    /// #4070 (b): a missing recorded file sends the run down the full
    /// path — the store is checked first (a gc'd blob is the clean
    /// miss) and the gap is filled. A file the tree holds under moved
    /// size and mtime (flutter rewrote it) is no reason to run: no path
    /// ever replaces a file the tree holds, so it stays the tree's.
    #[test]
    fn any_missing_file_takes_the_full_path_and_held_files_stay_the_trees() {
        let store_dir = tempfile::tempdir().expect("store dir");
        let store = CasStore::open(store_dir.path()).expect("open store");
        let (plan, payload_id) = warm_plan(&store, store_dir.path());
        let project = tempfile::tempdir().expect("project dir");
        materialize(&store, &plan, project.path()).expect("cold placement");
        let dir = project.path().join(".dart_tool/flutter_build/abc123");

        let rewritten = b"flutter's own rewrite, longer than the blob";
        fs::write(dir.join("flutter_build.dill.stamp"), rewritten).expect("flutter rewrite");
        let held = materialize(&store, &plan, project.path()).expect("no-op");
        assert!(
            held.report.intact,
            "a held file with a moved stamp is still held"
        );
        assert_eq!(
            fs::read(dir.join("flutter_build.dill.stamp")).expect("read"),
            rewritten
        );

        fs::remove_file(dir.join("app.dill")).expect("delete one output");
        let filled = materialize(&store, &plan, project.path()).expect("full path");
        assert!(!filled.report.intact);
        assert_eq!(
            filled.report.cloned + filled.report.copied,
            1,
            "only the gap is filled"
        );
        assert_eq!(filled.report.skipped_existing, 1);
        assert_eq!(filled.placed[0].len(), 1);
        assert_eq!(
            fs::read(dir.join("app.dill")).expect("filled"),
            b"payload bytes"
        );
        assert_eq!(
            fs::read(dir.join("flutter_build.dill.stamp")).expect("read"),
            rewritten
        );

        fs::remove_file(dir.join("app.dill")).expect("delete again");
        fs::remove_file(store.blob_path(&payload_id)).expect("gc drops the blob");
        let err = materialize(&store, &plan, project.path()).expect_err("store gap");
        assert!(
            matches!(&err, BcacheError::NotFound(msg) if msg.contains(&blob_id_hex(&payload_id))),
            "the full path checks the store before the tree: {err:?}"
        );
        assert_eq!(
            fs::read(dir.join("flutter_build.dill.stamp")).expect("read"),
            rewritten
        );
    }

    /// flutter_tools' FileStore check, as `Node.computeChanges` runs it
    /// (build_system.dart → file_store.dart `_hashFile`): every record's
    /// file is md5-hashed — a relative path against the project dir, as
    /// flutter resolves it against its cwd — and any digest that differs
    /// from the record (or a missing file) is a dirty target. Returns the
    /// records that would rerun their target.
    fn flutter_dirty_records(project: &Path) -> Vec<String> {
        use md5::{Digest, Md5};

        let hash_dir = project.join(".dart_tool/flutter_build/c04e6105834f6d5f4ffd798f1b39830f");
        let record: FileStorage =
            serde_json::from_slice(&fs::read(hash_dir.join(FILE_STORE)).expect("read record"))
                .expect("a valid version-2 record");
        assert_eq!(record.version, FILE_STORE_VERSION);
        record
            .files
            .iter()
            .filter(|file| {
                let path = project.join(&file.path);
                fs::read(&path).map_or(true, |bytes| {
                    format!("{:x}", Md5::digest(&bytes)) != file.hash
                })
            })
            .map(|file| file.path.clone())
            .collect()
    }

    /// #4071: the fresh-worktree replay lands outputs flutter's own
    /// content validation accepts. The ingest tree's `.filecache` holds
    /// the md5 of tree A's `app.dill`; the cross-tree placement rewrites
    /// the dill's URIs (#4069), and without re-aiming the record at the
    /// new bytes `kernel_snapshot_program` read its own output as
    /// changed and recompiled on every fresh-tree hit (16 of 19 files
    /// rewritten, live). The bundle's `kernel_blob.bin` — the same bytes
    /// in the asset dir — is cloned from the one rewrite and re-aimed
    /// too; the input record keeps its md5, and verbatim outputs keep
    /// theirs. A tree that keeps its own `.filecache` is never patched.
    #[test]
    fn fresh_tree_replay_lands_a_filecache_flutter_validation_accepts() {
        use crate::dill_rewrite::synthetic_component;
        use md5::{Digest, Md5};

        let md5_hex = |bytes: &[u8]| format!("{:x}", Md5::digest(bytes));
        let store_dir = tempfile::tempdir().expect("store dir");
        let store = CasStore::open(store_dir.path()).expect("open store");
        let tree_a = tempfile::tempdir().expect("tree a");
        let tree_b = tempfile::tempdir().expect("tree b");
        let root_a = fs::canonicalize(tree_a.path()).expect("canonical a");
        let root_b = fs::canonicalize(tree_b.path()).expect("canonical b");
        let a = root_a.to_string_lossy().into_owned();
        let hash = "c04e6105834f6d5f4ffd798f1b39830f";

        let source = b"import 'package:core/core.dart';\nvoid main() {}\n";
        let dill_a = synthetic_component(
            &[(
                &format!("file://{a}/app/lib/main.dart"),
                "package:app/main.dart",
            )],
            &[&format!("file://{a}/packages/core/lib/core.dart")],
            1,
        );
        let native_assets = br#"{"format-version":[1,0,0],"native-assets":{}}"#;
        let font = b"font bytes";
        let kernel_stamp = format!(
            r#"{{"inputs":["{a}/app/lib/main.dart"],"outputs":["{a}/app/.dart_tool/flutter_build/{hash}/app.dill"]}}"#
        );
        let file_store = format!(
            concat!(
                r#"{{"version":2,"files":["#,
                r#"{{"path":"{a}/app/lib/main.dart","hash":"{src}"}},"#,
                r#"{{"path":"{a}/app/.dart_tool/flutter_build/{hash}/app.dill","hash":"{dill}"}},"#,
                r#"{{"path":"{a}/app/.dart_tool/flutter_build/{hash}/native_assets.json","hash":"{na}"}},"#,
                r#"{{"path":"{a}/app/build/flutter_assets/kernel_blob.bin","hash":"{dill}"}},"#,
                r#"{{"path":"build/flutter_assets/fonts/MaterialIcons-Regular.otf","hash":"{font}"}}"#,
                r#"]}}"#
            ),
            a = a,
            hash = hash,
            src = md5_hex(source),
            dill = md5_hex(&dill_a),
            na = md5_hex(native_assets),
            font = md5_hex(font),
        );
        let dill_id = place_blob(&store, store_dir.path(), "dill", &dill_a);
        let stamp_id = place_blob(&store, store_dir.path(), "kstamp", kernel_stamp.as_bytes());
        let native_id = place_blob(&store, store_dir.path(), "native", native_assets);
        let font_id = place_blob(&store, store_dir.path(), "font", font);
        let store_id = place_blob(&store, store_dir.path(), "filecache", file_store.as_bytes());
        let plan = MaterializePlan {
            dirs: vec![
                hash_dir(
                    hash,
                    vec![
                        manifest(".filecache", store_id, false, RewriteKind::PathText),
                        manifest("app.dill", dill_id, false, RewriteKind::DillUris),
                        manifest(
                            "kernel_snapshot_program.stamp",
                            stamp_id,
                            false,
                            RewriteKind::PathText,
                        ),
                        manifest("native_assets.json", native_id, false, RewriteKind::None),
                    ],
                ),
                DirPlan {
                    path: PathBuf::from("build/flutter_assets"),
                    name_rewrite: None,
                    files: vec![
                        manifest("kernel_blob.bin", dill_id, false, RewriteKind::DillUris),
                        manifest(
                            "fonts/MaterialIcons-Regular.otf",
                            font_id,
                            false,
                            RewriteKind::None,
                        ),
                    ],
                },
            ],
            old_root: a.clone(),
            new_root: root_b.clone(),
        };

        // Tree B: a fresh checkout of the same sources, no build state.
        let project_b = root_b.join("app");
        fs::create_dir_all(project_b.join("lib")).expect("mkdir lib");
        fs::write(project_b.join("lib/main.dart"), source).expect("source");
        let outcome = materialize(&store, &plan, &project_b).expect("materialize");
        assert_eq!(
            outcome.report.dill_rewrites, 2,
            "one rewrite, one clone of it"
        );
        assert_eq!(outcome.report.dill_rewrite_refused, 0);
        assert_eq!(
            outcome.report.filecache_patched, 2,
            "app.dill and kernel_blob.bin"
        );

        let placed_dir = project_b.join(".dart_tool/flutter_build").join(hash);
        let dill_b = fs::read(placed_dir.join("app.dill")).expect("placed dill");
        assert_ne!(dill_b, dill_a, "the URIs were rewritten");
        assert_eq!(
            fs::read(project_b.join("build/flutter_assets/kernel_blob.bin")).expect("kernel blob"),
            dill_b,
            "the second placement of the blob is the same rewrite"
        );
        assert_eq!(
            flutter_dirty_records(&project_b),
            Vec::<String>::new(),
            "every target skips"
        );
        let record = fs::read_to_string(placed_dir.join(FILE_STORE)).expect("record");
        assert!(
            !record.contains(&a),
            "no record names the ingest tree: {record}"
        );
        assert!(
            record.contains(&md5_hex(source)),
            "the input keeps its recorded md5"
        );

        // The placed identity of the patched record is its final one,
        // and so is the hash the tree record carries for it.
        let filecache = outcome.placed[0]
            .iter()
            .find(|file| file.path == Path::new(FILE_STORE))
            .expect("placed record");
        assert!(!filecache.verbatim());
        assert_eq!(
            filecache.stamp,
            stamp_of(&fs::metadata(placed_dir.join(FILE_STORE)).expect("stat record"))
        );
        assert_eq!(
            filecache.on_disk,
            Some(file_blake3(&placed_dir.join(FILE_STORE)).expect("hash record")),
            "the on-disk hash is of the patched bytes"
        );
        let payload = outcome.placed[0]
            .iter()
            .find(|file| file.path == Path::new("app.dill"))
            .expect("placed dill");
        assert_eq!(
            payload.on_disk, None,
            "a rewritten kernel dill is not hashed at placement"
        );

        // A tree that keeps its own record: the dill is filled and
        // rewritten, the tree's record is never patched — its md5s
        // describe this tree's own last build, not the entry's.
        let tree_c = tempfile::tempdir().expect("tree c");
        let project_c = fs::canonicalize(tree_c.path())
            .expect("canonical c")
            .join("app");
        let own_dir = project_c.join(".dart_tool/flutter_build").join(hash);
        fs::create_dir_all(&own_dir).expect("mkdir own dir");
        fs::write(own_dir.join(FILE_STORE), b"this tree's own record").expect("own record");
        let plan_c = MaterializePlan {
            new_root: fs::canonicalize(tree_c.path()).expect("canonical c"),
            ..plan
        };
        let outcome = materialize(&store, &plan_c, &project_c).expect("materialize c");
        assert_eq!(outcome.report.filecache_patched, 0);
        assert_eq!(outcome.report.dill_rewrites, 2);
        assert_eq!(
            fs::read(own_dir.join(FILE_STORE)).expect("own record"),
            b"this tree's own record"
        );
    }
}
