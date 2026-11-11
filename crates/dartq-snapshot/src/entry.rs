//! Snapshot entries: keys and the checksummed, tables-stamped binary
//! payload frame. The payload codec is `bincode-next`'s serde bridge
//! (#4207, positional): payload shapes carry every field on the wire (no
//! `skip_serializing_if` — a positional decoder would misalign), and the
//! generation header's `format_version` gates codec eras (a change of
//! codec bumps [`crate::SNAPSHOT_FORMAT_VERSION`], so an old generation
//! is a whole-generation miss before any entry decodes).

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::blake3_128;

/// Cross-process identity of one entry: the content it was
/// computed from, the caller-defined feature fingerprint that shaped the
/// analysis, and the engine semantic version. The on-disk filename is the
/// blake3-128 hash of this triple.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EntryKey {
    /// blake3-128 content hash of the source bytes.
    pub content_hash: [u8; 16],
    /// Opaque caller-defined fingerprint (e.g. language features in play).
    pub feature_fingerprint: u64,
    /// Engine semantic version supplied by the caller.
    pub semantic_version: u32,
}

impl EntryKey {
    /// The entry id: blake3-128 over the fixed-width encoding of the
    /// triple.
    #[must_use]
    pub fn id(&self) -> [u8; 16] {
        let mut packed = [0u8; 16 + 8 + 4];
        packed[..16].copy_from_slice(&self.content_hash);
        packed[16..24].copy_from_slice(&self.feature_fingerprint.to_le_bytes());
        packed[24..].copy_from_slice(&self.semantic_version.to_le_bytes());
        blake3_128(&packed)
    }
}

/// One stored entry. On disk: the blake3-128 hex checksum of the payload
/// bytes, a space, the blake3-128 hex stamp of the generation's
/// remap-slices serialization, a newline, then those bytes — so truncation
/// and corruption are both caught by the checksum, and an entry is only
/// ever decoded under the save group (the same remap slices) that wrote it
/// (#338). The entry-id manifest sharing `tables.json`'s atomic write is
/// outside the stamp (#422): an entry-set delta leaves the frame — and the
/// file — byte-identical.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry<T> {
    pub key: EntryKey,
    pub payload: T,
}

/// Frames an entry for writing. Infallible except for payload
/// serialization. The payload is positional binary (#4207): payload
/// shapes carry every field explicitly, and the frame's checksum covers
/// exactly the bytes a decode reads.
pub(super) fn encode<T: Serialize>(
    entry: &Entry<T>,
    tables_stamp: [u8; 16],
) -> Result<Vec<u8>, bincode_next::error::EncodeError> {
    let payload = bincode_next::serde::encode_to_vec(entry, bincode_next::config::standard())?;
    let mut framed = Vec::with_capacity(66 + payload.len());
    framed.extend_from_slice(hex::encode(blake3_128(&payload)).as_bytes());
    framed.push(b' ');
    framed.extend_from_slice(hex::encode(tables_stamp).as_bytes());
    framed.push(b'\n');
    framed.extend_from_slice(&payload);
    Ok(framed)
}

/// Decodes a framed entry; `None` on any framing, checksum, or payload
/// failure (the accelerator invariant: corruption is a miss). The embedded
/// tables stamp returns with the entry — the store compares it against the
/// generation's current remap-slices stamp and treats a mismatch as
/// *absent* (an entry from another save group), never as corrupt.
pub(super) fn decode<T: DeserializeOwned>(framed: &[u8]) -> Option<(Entry<T>, [u8; 16])> {
    let newline = framed.iter().position(|&b| b == b'\n')?;
    let (tags, payload) = (&framed[..newline], &framed[newline + 1..]);
    // Two 32-character hex tags separated by one space.
    if tags.len() != 65 || tags[32] != b' ' {
        return None;
    }
    let checksum: [u8; 16] = hex::decode(&tags[..32]).ok()?.try_into().ok()?;
    let stamp: [u8; 16] = hex::decode(&tags[33..]).ok()?.try_into().ok()?;
    if blake3_128(payload) != checksum {
        return None;
    }
    bincode_next::serde::decode_from_slice(payload, bincode_next::config::standard())
        .ok()
        .map(|(entry, _)| (entry, stamp))
}

#[cfg(test)]
mod tests {
    use super::{decode, encode};
    use crate::EntryKey;

    #[test]
    fn frame_round_trips_and_rejects_corruption() {
        let entry = crate::Entry {
            key: EntryKey {
                content_hash: [1; 16],
                feature_fingerprint: 2,
                semantic_version: 3,
            },
            payload: vec!["alpha".to_string(), "beta".to_string()],
        };
        let stamp = [7u8; 16];
        let framed = encode(&entry, stamp).expect("vec serializes");
        assert_eq!(decode::<Vec<String>>(&framed), Some((entry.clone(), stamp)));

        // Any single-byte payload flip trips the checksum.
        let mut corrupt = framed.clone();
        let last = corrupt.len() - 1;
        corrupt[last] ^= 0xFF;
        assert_eq!(decode::<Vec<String>>(&corrupt), None);

        // Truncation before the newline and into the payload both miss.
        assert_eq!(decode::<Vec<String>>(&framed[..31]), None);
        assert_eq!(decode::<Vec<String>>(&framed[..framed.len() / 2]), None);

        // The stamp round-trips distinctly.
        assert_ne!(
            decode::<Vec<String>>(&encode(&entry, [8u8; 16]).expect("encode")).expect("decoded"),
            (entry.clone(), stamp)
        );
    }
}
