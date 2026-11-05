//! The crate-wide data contract: content ids, cache-key inputs, and the
//! entry manifest the store persists.
//!
//! Serialization is deterministic by construction: serde emits struct
//! fields in declaration order and every map is a `BTreeMap`, so equal
//! values always serialize to identical bytes.

use std::collections::BTreeMap;
use std::fmt;
use std::path::PathBuf;
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

/// Whether a materialized file needs the project-root path rewrite.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RewriteKind {
    None,
    /// Rewrite absolute ingest-tree-root path bytes to the target tree's
    /// root.
    PathText,
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
    /// Touched at ingest with no captured template and no verbatim-safe
    /// producer: the name embeds strings this crate cannot recompute, so
    /// materialization skips this dir.
    NotReplayable,
}

/// One `.dart_tool/flutter_build/<name>` directory of a cache entry.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct DirRecord {
    /// Dir name observed at ingest (provenance; also the replay name for
    /// [`NameRule::VerbatimRelative`]).
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
    /// Absolute project root at ingest time; rewrite source for
    /// [`RewriteKind::PathText`] files and the prefix replaced in
    /// [`DirRecord::name_output_dir`] strings.
    pub project_root_at_ingest: String,
    /// Every hash dir the ingested run touched, sorted by
    /// [`DirRecord::name_observed`].
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
    /// Smallest `last_hit_unix` over all entries; 0 for an empty store.
    pub oldest_hit_unix: u64,
}
