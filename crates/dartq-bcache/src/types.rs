//! The crate-wide data contract: content ids, cache-key inputs, and the
//! entry manifest the store persists.
//!
//! Serialization is deterministic by construction: serde emits struct
//! fields in declaration order and every map is a `BTreeMap`, so equal
//! values always serialize to identical bytes.

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use serde::de::{self, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::error::BcacheError;

/// 32-byte content hash (blake3) identifying one immutable blob.
///
/// Serializes as its 64-digit lowercase hex spelling — the same spelling
/// the store uses for blob file names.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct BlobId(pub [u8; 32]);

/// The canonical spelling of `id`: 64 lowercase hex digits.
#[must_use]
pub fn blob_id_hex(id: &BlobId) -> String {
    blake3::Hash::from_bytes(id.0).to_hex().to_string()
}

/// Whether `s` is a blake3 digest in canonical spelling: exactly 64
/// lowercase hex digits. Blob ids and entry keys share this form.
pub(crate) fn is_hex_digest(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

impl FromStr for BlobId {
    type Err = BcacheError;

    /// Accepts the canonical spelling only, so a blob has exactly one
    /// textual name in manifests and on disk.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if is_hex_digest(s)
            && let Ok(hash) = blake3::Hash::from_hex(s)
        {
            return Ok(Self(*hash.as_bytes()));
        }
        Err(BcacheError::Invalid(format!(
            "blob id {s:?}: expected 64 lowercase hex digits"
        )))
    }
}

impl Serialize for BlobId {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(blake3::Hash::from_bytes(self.0).to_hex().as_str())
    }
}

impl<'de> Deserialize<'de> for BlobId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct HexVisitor;

        impl Visitor<'_> for HexVisitor {
            type Value = BlobId;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a blob id as 64 lowercase hex digits")
            }

            fn visit_str<E: de::Error>(self, v: &str) -> Result<BlobId, E> {
                v.parse().map_err(E::custom)
            }
        }

        deserializer.deserialize_str(HexVisitor)
    }
}

/// Identity of the toolchain that produced (or would produce) an entry.
#[derive(Clone, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
pub struct ToolchainFingerprint {
    /// From flutter's engine stamp: `bin/cache/engine-dart-sdk.stamp`.
    pub engine_revision: String,
    /// `frameworkVersion` of `bin/cache/flutter.version.json`.
    pub flutter_version: String,
    /// blake3 of `bin/cache/dart-sdk/lib/_internal/vm_platform_strong.dill`.
    pub platform_strong_dill: BlobId,
    /// blake3 of the frontend_server snapshot flutter_tools runs:
    /// `bin/cache/dart-sdk/bin/snapshots/frontend_server_aot.dart.snapshot`.
    pub frontend_server_snapshot: BlobId,
}

/// Path-independent identity of one provenance file.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileIdentity {
    /// Relative to the project root.
    TreeRelative(PathBuf),
    /// Stable machine-local absolute path (a file outside the tree, such
    /// as an SDK artifact).
    Absolute(PathBuf),
}

/// One provenance file of the key closure, by identity and content.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
pub struct ClosureFile {
    pub identity: FileIdentity,
    pub content: BlobId,
    pub size: u64,
}

/// The slice of a flutter invocation that feeds the cache key.
#[derive(Clone, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
pub struct TargetSpec {
    /// Subcommand chain, e.g. `build`, `run`, `test`.
    pub command: String,
    /// `ios` | `android` | `macos` | `linux` | `windows` | `web` |
    /// `bundle` | `dart-test` | `host-default`.
    pub platform: String,
    /// `debug` | `profile` | `release` | `jit_release`.
    pub mode: String,
    /// `-t`/`--target`, relative to the project root.
    pub entry: Option<PathBuf>,
    pub flavor: Option<String>,
    /// Any other argv items that can affect build outputs (e.g.
    /// `--obfuscate`, `--split-debug-info`, `--extra-front-end-options`,
    /// `--no-tree-shake-icons`), kept verbatim in original order.
    /// Conservative: when unsure, keep.
    pub extra_key_args: Vec<String>,
}

/// Everything the entry key is computed from.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct KeyInputs {
    pub toolchain: ToolchainFingerprint,
    /// Full define vector (`FLUTTER_GIT_URL` is filtered by callers
    /// before this point).
    pub defines: BTreeMap<String, String>,
    /// The provenance files hashed into the key: tree-relative, or
    /// absolute when outside the tree. Never depfile-sourced (see
    /// [`crate::key::config_key`]).
    pub closure: Vec<ClosureFile>,
    pub target: TargetSpec,
}

/// The absolute-path anchors of one ingest tree: every absolute path a
/// recorded file mentions is classified against them (see
/// [`crate::ingest_scan`]).
///
/// * [`PathAnchors::worktree_root`] — the git toplevel enclosing the
///   project (the project root itself when no repo encloses it). Paths
///   under it are TREE-RELATIVE: a replay into another tree rewrites
///   them to that tree's worktree root. This is the rewrite anchor —
///   wider than the project root on purpose, so a melos/pub-workspace
///   sibling `path:` package (inside the repo, outside the project)
///   rewrites too.
/// * [`PathAnchors::pub_cache`] and [`PathAnchors::flutter_root`] —
///   machine-local roots whose absolute paths are tree-independent and
///   replay verbatim.
/// * Anything else is FOREIGN: a replay into a different tree refuses
///   the dir that mentions it (a bookkeeping file pointing at a path
///   this tree cannot re-derive would let flutter's input validation
///   skip on a file the target tree never sees — a wrong artifact, not
///   a wasted build). Within-tree replay is unaffected: the paths are
///   valid where they were recorded.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct PathAnchors {
    /// Absolute canonical worktree root; never empty at ingest.
    pub worktree_root: PathBuf,
    /// Absolute pub cache root (`$PUB_CACHE`, else pub's default);
    /// empty when unresolvable — an empty root matches nothing, so
    /// pub-cache paths then classify as foreign (a safe, slower
    /// degradation).
    pub pub_cache: PathBuf,
    /// Absolute flutter SDK root of the ingesting invocation.
    pub flutter_root: PathBuf,
}

impl PathAnchors {
    /// Whether an absolute path (raw bytes as recorded) is under `root`
    /// — a true path-component prefix, not a byte-string prefix
    /// (`/wt/x` does not put `/wt/xyz` under `/wt/x`).
    fn is_under(path: &[u8], root: &Path) -> bool {
        let root = root.as_os_str().as_encoded_bytes();
        !root.is_empty()
            && path.len() >= root.len()
            && &path[..root.len()] == root
            && (path.len() == root.len() || path[root.len()] == b'/')
    }

    /// Whether `path` (raw absolute-path bytes) is replayable: under
    /// the worktree root (rewritten on replay) or a machine-local cache
    /// root (verbatim). Foreign otherwise.
    #[must_use]
    pub fn carries_absolute(&self, path: &[u8]) -> bool {
        Self::is_under(path, &self.worktree_root)
            || Self::is_under(path, &self.pub_cache)
            || Self::is_under(path, &self.flutter_root)
    }
}

/// Whether a materialized file needs the absolute-path rewrite.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RewriteKind {
    None,
    /// Rewrite absolute ingest-worktree-root path bytes to the target
    /// tree's worktree root.
    PathText,
    /// A kernel dill — a `flutter test` kernel-cache dill or a build
    /// payload dill (`app.dill`, `kernel_blob.bin`): same-tree replay
    /// stays a byte-identical clone, while a replay into a different
    /// tree rewrites the `file://<ingest-root>/` URI prefixes inside
    /// the kernel tables to the target tree's root (see
    /// [`crate::dill_rewrite`]); a refused rewrite degrades to the
    /// verbatim clone.
    DillUris,
}

/// Filesystem identity of one file at hash time: device, inode, size,
/// and mtime in nanoseconds since the epoch (0 for a pre-epoch clock,
/// `u64::MAX` past the year ~584). On non-unix targets dev and inode
/// are always 0.
///
/// Recorded with every scanned file (see [`ManifestFile::stamp`]) as
/// advisory bookkeeping for the post-run scan — never a correctness
/// input: the tuple is exact, not heuristic. On the filesystems flutter
/// runs on (APFS, ext4) every write moves the nanosecond mtime and a
/// replaced file gets a new inode, so an all-fields match means no
/// write touched that inode since the hash — the content is the bytes
/// the hash saw.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct FileStamp {
    pub dev: u64,
    pub inode: u64,
    pub size: u64,
    pub mtime_ns: u64,
}

/// One file a materialization placed, as the post-run scan finds it
/// again: the placed file's fresh identity, the blob the placement
/// wrote from, and whether the placed bytes are that blob's bytes
/// verbatim (a cross-tree placement rewrites bookkeeping paths and
/// kernel URIs — those files are not their blob's bytes, but their
/// on-disk hash is carried so a later scan can still prove a
/// byte-identical flutter rewrite of them).
///
/// The handoff between the phases of one shim invocation AND the
/// per-tree placement record's persisted entry (see
/// [`crate::placement`]): what one run placed in memory, the record
/// remembers on disk for the runs that place nothing.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct PlacedFile {
    /// Path relative to the placed dir; matches [`ManifestFile::path`].
    pub path: PathBuf,
    pub stamp: FileStamp,
    pub blob: BlobId,
    pub rewrite: RewriteKind,
    /// Carried from the manifest file placed: the root rewrite moves
    /// worktree paths between trees but leaves foreign paths foreign,
    /// so the bit survives both verbatim and rewritten placements.
    pub foreign_paths: bool,
    /// The blake3 of the bytes on disk as placed, when the placement
    /// knows it: the blob's own id for a verbatim clone (the blob was
    /// identity-verified before any byte landed), the rewritten bytes'
    /// id for a re-rooted bookkeeping file (KBs, hashed at rewrite
    /// cost), and `None` for a re-rooted kernel dill (tens of MB — a
    /// placement that flutter did not just rewrite would prove itself
    /// by stamp alone, and one flutter DID rewrite is a real change
    /// that must be hashed whatever a recorded hash said).
    ///
    /// [`Self::verbatim`] is `on_disk == Some(blob)`: the manifest may
    /// reference the blob directly for such a placement anywhere; any
    /// other value means the bytes are this tree's re-rooted rendering
    /// of the blob — a reference only valid while the entry is not
    /// being re-published (see [`crate::ingest_scan`]).
    pub on_disk: Option<BlobId>,
    /// For the bookkeeping flutter rewrites with noise that is not
    /// content (the materializer's `content_identity`): the FileStore
    /// record (`.filecache`) up to its order and its hook-result md5s,
    /// and a hook result (`link_hooks_result.json`,
    /// `dart_build_result.json`) up to its `build_start`/`build_end`
    /// timestamps. flutter_tools rewrites the record on every build in
    /// a Dart `HashMap`'s iteration order, which follows the recorded
    /// paths' hash codes — under another tree's root the same records
    /// land in another order — and a hook result whenever its target
    /// runs, so a rewrite that changed nothing is recognized by this
    /// identity where [`Self::on_disk`] cannot.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<BlobId>,
}

impl PlacedFile {
    /// Whether the bytes on disk are the blob's own bytes — the
    /// placement verified the blob's content and wrote it unchanged, so
    /// a matching stamp proves the file is the blob wherever it sits.
    pub fn verbatim(&self) -> bool {
        self.on_disk == Some(self.blob)
    }
}

/// One file of a cached build directory.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct ManifestFile {
    /// Path relative to the flutter_build hash dir, e.g.
    /// `flutter_assets.d`.
    pub path: PathBuf,
    pub blob: BlobId,
    pub executable: bool,
    pub rewrite: RewriteKind,
    /// Whether the blob's bytes mention an absolute path that is
    /// neither under the ingest worktree root nor a machine-local cache
    /// root (see [`PathAnchors`]). A dir holding such a file is refused
    /// by cross-tree replays; within-tree replay is unaffected.
    pub foreign_paths: bool,
    /// The file's [`FileStamp`] at hash time; `None` in manifests
    /// written before the field existed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stamp: Option<FileStamp>,
}

/// How a recorded dir's name is derived when the entry materializes into
/// another tree.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NameRule {
    /// The producer hashed a tree-RELATIVE output dir (the `build bundle`
    /// face: `build/flutter_assets`), so the observed name is identical in
    /// every tree and replays verbatim.
    VerbatimRelative,
    /// The producer hashed an absolute output dir (the `flutter assemble`
    /// face), so the name is recomputed from the recorded define map and
    /// the root-rewritten output dir.
    RecomputeFromTemplate,
    /// The `flutter test` kernel cache (flutter_tools 3.47.1 source,
    /// TestCompiler): a fixed-name dir (`<buildDir>/test_cache`, default
    /// `build/test_cache`) whose dill file names embed the config md5 —
    /// not a hash dir at all. `name_observed` is the project-relative dir
    /// path at ingest, provenance only: the replay path is re-derived
    /// from the target tree's build dir (which honors
    /// `FLUTTER_BUILD_DIR`), and files land in place, over whatever the
    /// dir already holds — frontend_server's initialize-from-dill
    /// validation is the correctness net for a stale payload.
    TestKernelCache,
    /// The `build bundle` face's asset output dir (flutter_tools 3.47.1
    /// `build bundle --asset-dir`, default `<buildDir>/flutter_assets`):
    /// `copy_flutter_bundle`'s outputs — the kernel blob, assets, engine
    /// snapshots — a fixed project-relative dir, not a hash dir. Its
    /// files are tracked by the hash dir's `.filecache`, so a fresh tree
    /// that lacks them reruns the copy target however faithfully the
    /// hash dir replays. `name_observed` is the project-relative path at
    /// ingest, provenance only: the replay path is this invocation's
    /// asset dir, and files land fill-only-missing like everywhere else.
    AssetDir,
    /// Touched at ingest with no captured template and no verbatim-safe
    /// producer: the name embeds strings this crate cannot recompute, so
    /// materialization skips this dir.
    NotReplayable,
}

/// One recorded directory of a cache entry: a
/// `.dart_tool/flutter_build/<name>` hash dir, or a fixed project-
/// relative dir — the test face's kernel cache (see
/// [`NameRule::TestKernelCache`]), the bundle face's asset dir (see
/// [`NameRule::AssetDir`]).
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct DirRecord {
    /// Dir name (hash dirs) or project-relative path (the fixed dirs)
    /// observed at ingest; provenance, and the replay name for
    /// [`NameRule::VerbatimRelative`] only.
    pub name_observed: String,
    pub name_rule: NameRule,
    /// The define map whose md5 (with `name_output_dir`) produced
    /// `name_observed`; empty unless [`NameRule::RecomputeFromTemplate`].
    pub name_defines: BTreeMap<String, String>,
    /// The output-dir string as hashed at capture time: absolute for the
    /// assemble face, empty otherwise.
    pub name_output_dir: String,
    pub files: Vec<ManifestFile>,
}

/// One cache entry: the persisted `manifest.json` of a key.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct EntryManifest {
    /// Hex blake3 of the canonical [`KeyInputs`] serialization (the key
    /// engine's output).
    pub key: String,
    /// Absolute project root at ingest time (provenance).
    pub project_root_at_ingest: String,
    /// The replay anchor: absolute worktree root (git toplevel) of the
    /// ingest tree — the byte prefix a cross-tree replay rewrites to
    /// the target tree's worktree root in [`RewriteKind::PathText`]
    /// files, dill URI prefixes, and [`DirRecord::name_output_dir`] /
    /// define-value strings used to recompute names.
    pub worktree_root_at_ingest: String,
    /// The machine-local cache roots the ingest classified against (see
    /// [`PathAnchors`]); stored as the classification's provenance.
    /// Deterministic per tree — no timestamps.
    pub pub_cache_root: String,
    pub flutter_root_at_ingest: String,
    /// Every dir the ingested run touched — hash dirs sorted by
    /// [`DirRecord::name_observed`], then the fixed dirs: the asset dir
    /// of a `build bundle` run, the test kernel-cache dir of a `flutter
    /// test` run.
    pub dirs: Vec<DirRecord>,
    pub defines: BTreeMap<String, String>,
    pub target: TargetSpec,
    pub toolchain: ToolchainFingerprint,
    pub created_at_unix: u64,
    pub last_hit_unix: u64,
}

/// Size and age summary of a store.
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug, Serialize, Deserialize)]
pub struct StoreStats {
    pub entries: u64,
    pub blobs: u64,
    pub blob_bytes: u64,
    /// Smallest effective hit time over all entries; 0 for an empty
    /// store.
    pub oldest_hit_unix: u64,
    /// Manifests that failed to read or decode — skipped, not counted in
    /// `entries`. Corruption is never fatal to the summary: one bad
    /// manifest must not disable the disk budget that gc enforces over
    /// the rest of the store.
    pub corrupt_manifests: u64,
}
