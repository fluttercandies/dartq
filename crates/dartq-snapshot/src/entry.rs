//! Snapshot entries: keys and the checksummed JSON payload frame.

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::blake3_128;

/// Cross-process identity of one entry (PRD §5.6): the content it was
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

/// One stored entry. On disk: the blake3-128 hex checksum of the JSON
/// bytes, a newline, then those bytes — so truncation and corruption are
/// both caught by the checksum, and the payload is human-readable.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry<T> {
    pub key: EntryKey,
    pub payload: T,
}

/// Frames an entry for writing. Infallible except for payload
/// serialization.
pub(super) fn encode<T: Serialize>(entry: &Entry<T>) -> Result<Vec<u8>, serde_json::Error> {
    let payload = serde_json::to_vec(entry)?;
    let mut framed = Vec::with_capacity(33 + payload.len());
    framed.extend_from_slice(hex::encode(blake3_128(&payload)).as_bytes());
    framed.push(b'\n');
    framed.extend_from_slice(&payload);
    Ok(framed)
}

/// Decodes a framed entry; `None` on any framing, checksum, or JSON
/// failure (the accelerator invariant: corruption is a miss).
pub(super) fn decode<T: DeserializeOwned>(framed: &[u8]) -> Option<Entry<T>> {
    let newline = framed.iter().position(|&b| b == b'\n')?;
    let (tag, payload) = (&framed[..newline], &framed[newline + 1..]);
    if tag.len() != 32 {
        return None;
    }
    let checksum: [u8; 16] = hex::decode(tag).ok()?.try_into().ok()?;
    if blake3_128(payload) != checksum {
        return None;
    }
    serde_json::from_slice(payload).ok()
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
        let framed = encode(&entry).expect("vec serializes");
        assert_eq!(decode::<Vec<String>>(&framed), Some(entry));

        // Any single-byte payload flip trips the checksum.
        let mut corrupt = framed.clone();
        let last = corrupt.len() - 1;
        corrupt[last] ^= 0xFF;
        assert_eq!(decode::<Vec<String>>(&corrupt), None);

        // Truncation before the newline and into the payload both miss.
        assert_eq!(decode::<Vec<String>>(&framed[..31]), None);
        assert_eq!(decode::<Vec<String>>(&framed[..framed.len() / 2]), None);
    }
}
