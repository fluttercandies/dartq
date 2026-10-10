//! The generation header: the four fields whose equality gates a whole
//! snapshot generation.

use serde::{Deserialize, Serialize};

use crate::blake3_128;

/// The on-disk container format written by this crate. Bumped only for a
/// breaking change to the container itself (e.g. a bincode switch); entry
/// payload semantics live in [`GenerationHeader::semantic_version`], which
/// the caller supplies.
///
/// v2 folds the entry-id manifest into `tables.json` (one atomic unit
/// with the remap slices). No on-disk back-compat is provided or needed:
/// nothing consumes the format yet — the daemon's `save_generation` is
/// unwired and the load-side wiring (#341) designs against this contract.
pub const SNAPSHOT_FORMAT_VERSION: u32 = 2;

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
