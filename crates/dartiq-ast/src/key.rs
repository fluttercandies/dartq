//! The AST cache identity: identical keys share one parsed unit across
//! workspaces and contexts — the AST is shareable, semantics is not
//! (layout spec §4).

use dartiq_language::FeatureSet;
use dartiq_source::ContentId;

/// Fingerprint of a [`FeatureSet`] (the `dartiq-language` 64-bit FNV-1a
/// fingerprint; collision stance documented there — lookup key only, never a
/// correctness check).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FeatureSetId(u64);

impl FeatureSetId {
    /// Fingerprints `features`.
    #[must_use]
    pub fn of(features: &FeatureSet) -> Self {
        Self(features.fingerprint())
    }

    /// The underlying fingerprint.
    #[must_use]
    pub const fn raw(self) -> u64 {
        self.0
    }
}

/// Parser behavior version: starts at 1 and bumps on any parser behavior or
/// diagnostic change; feeds `EngineSemanticVersion` (layout spec §4).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ParserVersion(u32);

impl ParserVersion {
    /// The current parser version. 2: #151 payload-layout semantics changed
    /// (token-leaf words eliminated into `first_token`, explicit
    /// `Option`-word encoding, ParenthesizedCondition/Send/
    /// ExpressionStatement widths) — v1 tables must not be reused as v2.
    pub const CURRENT: Self = Self(2);

    /// The underlying number.
    #[must_use]
    pub const fn raw(self) -> u32 {
        self.0
    }
}

/// Cache key of a parsed unit: content identity + feature fingerprint +
/// parser version. A unit is a pure function of its key; there is no AST
/// serialization in M1 — L1 LRU evicts whole units and they are recomputed
/// (layout spec §4).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct AstKey {
    /// Blake3-128 content digest of the source bytes.
    pub content: ContentId,
    /// Feature set the unit was parsed under.
    pub feature_set: FeatureSetId,
    /// Parser behavior version.
    pub parser_version: ParserVersion,
}

impl AstKey {
    /// Keys a unit parsed by [`ParserVersion::CURRENT`].
    #[must_use]
    pub fn new(content: ContentId, feature_set: FeatureSetId) -> Self {
        Self {
            content,
            feature_set,
            parser_version: ParserVersion::CURRENT,
        }
    }
}
