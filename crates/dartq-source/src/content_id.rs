//! Content identities: the 128-bit blake3 digest that names source bytes
//! everywhere in dartq (PRD §5.2).

use std::fmt;
use std::str::FromStr;

/// Content identity: the first 128 bits of the blake3 digest of some bytes.
///
/// This is the universal name for source content in dartq — two byte
/// sequences are the same content if and only if their [`ContentId`]s are
/// equal, regardless of file path, package, or machine (PRD §5.2). The
/// 128-bit truncation is the PRD-confirmed choice (`blake3-128`); it keeps
/// ids small enough to embed in caches and diagnostics while collision
/// risk stays negligible for source-sized inputs.
///
/// Ordering and hashing are the plain byte order of the digest, so ids can
/// be used directly as map keys and for deterministic sortings.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ContentId([u8; 16]);

/// Computes the [`ContentId`] of `bytes`: blake3 truncated to 128 bits.
#[must_use]
pub fn content_hash(bytes: &[u8]) -> ContentId {
    let digest = blake3::hash(bytes);
    let mut id = [0u8; 16];
    id.copy_from_slice(&digest.as_bytes()[..16]);
    ContentId(id)
}

impl ContentId {
    /// Parses the lowercase or uppercase 32-character hex form produced by
    /// [`Display`](fmt::Display).
    ///
    /// # Errors
    ///
    /// Returns [`ParseContentIdError`] when the input is not exactly 32 hex
    /// characters.
    pub fn parse_hex(s: &str) -> Result<Self, ParseContentIdError> {
        Self::from_str(s)
    }
}

impl fmt::Display for ContentId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&hex::encode(self.0))
    }
}

impl fmt::Debug for ContentId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ContentId({self})")
    }
}

impl FromStr for ContentId {
    type Err = ParseContentIdError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let bytes = s.as_bytes();
        if bytes.len() != 32 {
            return Err(ParseContentIdError::InvalidLength(bytes.len()));
        }
        let mut id = [0u8; 16];
        // Table lookup, not `from_str_radix`: a leading `+`/`-` sign is not a
        // hex digit and must not parse as a ContentId. With the length
        // pre-checked above, the only reachable `hex` error is a non-hex
        // character (odd length / wrong slice size are excluded).
        hex::decode_to_slice(bytes, &mut id).map_err(|_| ParseContentIdError::InvalidHex)?;
        Ok(ContentId(id))
    }
}

/// Fails when `parse_hex`/`FromStr` rejects a [`ContentId`] spelling.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ParseContentIdError {
    /// The input is not exactly 32 characters; carries the actual length.
    #[error("invalid ContentId length {0}, expected 32 hex characters")]
    InvalidLength(usize),
    /// The input contains a non-hex character.
    #[error("invalid ContentId: expected only hex characters")]
    InvalidHex,
}

#[cfg(feature = "serde")]
mod serde_impl {
    use super::ContentId;
    use serde::de::{self, Deserialize, Deserializer, Visitor};
    use serde::ser::{Serialize, Serializer};
    use std::fmt;

    impl Serialize for ContentId {
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            serializer.serialize_str(&self.to_string())
        }
    }

    impl<'de> Deserialize<'de> for ContentId {
        fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            struct ContentIdVisitor;

            impl Visitor<'_> for ContentIdVisitor {
                type Value = ContentId;

                fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                    f.write_str("a 32-character blake3-128 hex string")
                }

                fn visit_str<E: de::Error>(self, v: &str) -> Result<Self::Value, E> {
                    ContentId::parse_hex(v).map_err(E::custom)
                }
            }

            deserializer.deserialize_str(ContentIdVisitor)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Official BLAKE3 test vectors (hash mode), truncated to the first 16
    /// bytes; guard against wiring the wrong hash or truncation. The empty
    /// input is the canonical published vector; the non-empty one is pinned
    /// by the blake3 crate's own test suite (`test_hex_encoding_decoding` in
    /// blake3 1.8.7, the locked version): blake3 of `b"foo"`.
    #[test]
    fn known_blake3_vectors() {
        assert_eq!(
            content_hash(b"").to_string(),
            "af1349b9f5f9a1a6a0404dea36dcc949"
        );
        assert_eq!(
            content_hash(b"foo").to_string(),
            "04e0bb39f30b1a3feb89f536c93be150"
        );
    }

    #[test]
    fn same_content_same_id_different_content_different_id() {
        assert_eq!(content_hash(b"let x = 1;"), content_hash(b"let x = 1;"));
        assert_ne!(content_hash(b"let x = 1;"), content_hash(b"let x = 2;"));
        assert_ne!(content_hash(b""), content_hash(b"\0"));
    }

    #[test]
    fn hex_roundtrip() {
        let id = content_hash(b"dartq source");
        let parsed: ContentId = id.to_string().parse().expect("own hex output parses");
        assert_eq!(parsed, id);
        // Case-insensitive input, canonical lowercase output.
        let upper = id.to_string().to_uppercase();
        assert_eq!(upper.parse::<ContentId>().expect("uppercase parses"), id);
    }

    #[test]
    fn parse_rejects_bad_input() {
        assert_eq!(
            "abc".parse::<ContentId>(),
            Err(ParseContentIdError::InvalidLength(3))
        );
        assert_eq!(
            "g".repeat(32).parse::<ContentId>(),
            Err(ParseContentIdError::InvalidHex)
        );
    }

    /// Radix parsers accept a `+`/`-` sign prefix (`from_str_radix("+a", 16)`
    /// used to parse as `0x0a`); a [`ContentId`] spelling is pure hex only.
    #[test]
    fn parse_rejects_sign_prefixes_and_boundaries() {
        assert_eq!(
            "a".repeat(30).parse::<ContentId>(),
            Err(ParseContentIdError::InvalidLength(30))
        );
        let plus = "a".repeat(30) + "+a";
        assert_eq!(plus.len(), 32);
        assert_eq!(
            plus.parse::<ContentId>(),
            Err(ParseContentIdError::InvalidHex)
        );
        let minus = "a".repeat(30) + "-a";
        assert_eq!(
            minus.parse::<ContentId>(),
            Err(ParseContentIdError::InvalidHex)
        );
        assert_eq!(
            "".parse::<ContentId>(),
            Err(ParseContentIdError::InvalidLength(0))
        );
        assert_eq!(
            "a".repeat(33).parse::<ContentId>(),
            Err(ParseContentIdError::InvalidLength(33))
        );
    }

    #[cfg(feature = "serde")]
    #[test]
    fn serde_roundtrip() {
        let id = content_hash(b"serde bytes");
        let json = serde_json::to_string(&id).expect("serialize");
        assert_eq!(json, format!("\"{id}\""));
        let back: ContentId = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, id);
    }
}
