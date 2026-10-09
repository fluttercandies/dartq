//! Cross-process remap slices (PRD §10.5): the engine's internal
//! `StringId`/`FileId` are process-local, so a snapshot carries its own id
//! spaces and the wiring layer remaps them onto the live engine's ids at
//! load time.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// A `StringId` as recorded inside a snapshot generation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct SnapshotStringId(pub u32);

/// A `FileId` as recorded inside a snapshot generation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct SnapshotFileId(pub u32);

/// The generation-wide remap slices: snapshot-internal ids paired with the
/// values they name. The wiring layer interns these values in the fresh
/// process and translates payload references through the pairing.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemapTables {
    pub strings: Vec<(SnapshotStringId, String)>,
    pub files: Vec<(SnapshotFileId, PathBuf)>,
}
