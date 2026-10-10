//! The generation header: the four fields whose equality gates a whole
//! snapshot generation.

use serde::{Deserialize, Serialize};

use crate::blake3_128;

/// The on-disk container format written by this crate. Bumped only for a
/// breaking change to the container itself (e.g. a bincode switch); entry
/// payload semantics live in [`GenerationHeader::semantic_version`], which
/// the caller supplies.
///
/// v2 folded the entry-id manifest into `tables.json` (one atomic unit
/// with the remap slices). v3 (#338 save-group atomicity, #341): the entry
/// frame embeds a blake3-128 stamp of the generation's `tables.json`
/// bytes — tables are written first, entries carry the stamp, and `load`
/// discards entries whose stamp mismatches, so an entry can only ever
/// decode under the save group that wrote it. v4 (#422 write
/// amplification): the stamp narrowed to the *remap-slices serialization*
/// — the manifest stays in `tables.json`'s atomic write but outside the
/// hash, so an entry-set delta leaves the stamp (and every unchanged
/// entry frame) byte-identical and `save` skips rewriting such files;
/// `load` also rejects an entry whose payload key disagrees with its
/// filename. v5 (#482): the header gained [`GenerationHeader::context_salt`]
/// — the caller's remaining fork dimensions (target, declared variables,
/// analysis options) hashed in, so forked contexts can never collide on
/// one generation directory. No on-disk back-compat is provided or
/// needed: an earlier generation simply mismatches its header and is
/// rebuilt.
pub const SNAPSHOT_FORMAT_VERSION: u32 = 5;

/// Identity of one snapshot generation (PRD §5.5, §5.6). A loaded
/// generation is used only when every field equals what the caller
/// expects; any inequality is a miss for the whole generation, not an
/// error.
///
/// `semantic_version` is the caller's engine semantic version (the engine's
/// `FACTS_SEMANTIC_VERSION` for facts snapshots); this crate never derives
/// it and has no dependency on `dartq-engine`.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct GenerationHeader {
    /// Must be [`SNAPSHOT_FORMAT_VERSION`]; the store's `save` rejects
    /// foreign versions rather than writing an unreadable generation.
    pub format_version: u32,
    /// Engine semantic version supplied by the caller.
    pub semantic_version: u32,
    /// Resolved SDK identity (version, revision, target).
    pub sdk_identity: String,
    /// blake3-128 fingerprint of the package graph.
    pub package_graph_fingerprint: [u8; 16],
    /// blake3-128 over the caller's remaining fork dimensions — the SDK
    /// target segment, the conditional-import declared-variables map, and
    /// the analysis-options content (#482). Opaque to this crate: the
    /// caller derives it so two contexts differing on any of these
    /// dimensions never share a generation directory, while the header
    /// itself stays compact and value-hashed (dimensions folded in, whole
    /// maps never embedded).
    pub context_salt: [u8; 16],
}

impl GenerationHeader {
    /// The generation directory name: blake3-128 over the canonical JSON
    /// form of the header. Two stores with equal headers therefore share
    /// one directory, and a mismatched header cannot name an existing one.
    pub(crate) fn dir_id(&self) -> [u8; 16] {
        let canonical = serde_json::to_vec(self)
            .expect("header fields are strings and numbers; serialization cannot fail");
        blake3_128(&canonical)
    }
}
