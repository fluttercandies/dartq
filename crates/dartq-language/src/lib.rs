//! Single source of truth for Dart language feature versioning in dartq.
//!
//! This crate defines:
//!
//! * [`LanguageVersion`] — a Dart language version (`major.minor`), with
//!   [`DEFAULT_LANGUAGE_VERSION`] taken from the vendored yaml's
//!   `current-version`.
//! * [`Feature`] — one variant per entry in the Dart SDK's
//!   `tools/experimental_features.yaml`, vendored at
//!   `data/experimental_features.yaml` (dart-lang/sdk tag `3.13.1`,
//!   `current-version: '3.13.0'`, era-pinned to the dart 3.13.1 measurement
//!   oracle — see the yaml header for provenance; vendored 2026-10-09).
//! * [`FeatureSet`] — the features enabled for a library: derived from its
//!   language version, plus explicit `--enable-experiment` opt-ins.
//!
//! The registry ([`Feature`], [`FEATURES`], [`FEATURE_COUNT`]) is generated
//! at compile time by `build.rs` from the vendored yaml; nothing is parsed at
//! runtime. To update, re-vendor the yaml — hand edits will not survive a
//! rebuild.
//!
//! # Semantics
//!
//! Mirrors the SDK's generated tables
//! (`pkg/_fe_analyzer_shared/lib/src/experiments/flags.dart`,
//! `pkg/analyzer/lib/src/dart/analysis/experiments_impl.dart`):
//!
//! * A feature with `enabledIn` is enabled exactly when the library language
//!   version is `>= enabledIn` (a lower version — e.g. via a
//!   `// @dart = 2.12` override — disables it).
//! * A feature without `enabledIn` is disabled by default and only enabled
//!   via an explicit experiment flag, gated on its experimental release
//!   version ([`FeatureSet::with_experiment_for_version`]).
//! * An explicit `--enable-experiment` flag is applied only from
//!   `min(current-version, experimentalReleaseVersion)` on
//!   (`experiments_impl.dart::restrictEnableFlagsToVersion`: "experimental
//!   features should be available since `min(sdk, experimentalRelease)`";
//!   a flag without an assigned release version opts in only at the exact
//!   current version, `version == sdkLanguageVersion`). Expired flags are
//!   dropped before any version check (`_flagStringsToMap`) and never change
//!   enablement.
//! * `expired: true` marks a retired/rejected flag: tools keep running but
//!   only warn about the flag, and expiry never changes version-based
//!   enablement (see `experiments_impl.dart::decodeExplicitFlags`, which
//!   ignores expired flags). Every expired entry in the current snapshot also
//!   has `enabledIn`.
//! * A `// @dart` override above the latest known version
//!   ([`DEFAULT_LANGUAGE_VERSION`]) is officially rejected, not clamped: the
//!   scanner reports `INVALID_LANGUAGE_VERSION_OVERRIDE` (`STATIC_WARNING`,
//!   the `_GREATER` variant, "can't specify a version greater than the
//!   latest known language version") and drops the override
//!   (`scanner.dart`: `_overrideVersion = null`), so the library version
//!   re-resolves through the normal chain — package config, else the
//!   default. When that fallback lands on the default version (== latest)
//!   the drop is observationally a clamp, but the mechanism is drop +
//!   re-resolve. [`LanguageVersion::is_above_latest`] is the detection hook
//!   for that warning/drop emission site (measured in
//!   `docs/facts/version-skew.md` §1.3 T2).

// Help prose inside is copied verbatim from the vendored yaml and may
// contain URLs or camelCase words; do not lint generated docs.
#[allow(clippy::doc_markdown)]
mod generated {
    include!(concat!(env!("OUT_DIR"), "/feature_registry.rs"));
}

use core::fmt;
use core::str::FromStr;

pub use generated::{DEFAULT_LANGUAGE_VERSION, FEATURE_COUNT, FEATURES, Feature, FeatureMeta};

impl Feature {
    /// Whether this flag is expired (retired or rejected in the SDK).
    ///
    /// Official semantics (`experiments_impl.dart::decodeExplicitFlags` in
    /// `pkg/analyzer/lib/src/dart/analysis/`): an expired flag passed to
    /// `--enable-experiment` only produces a warning
    /// (`experimentExpiredEnabled`/`experimentExpiredDisabled`); it never
    /// changes version-based enablement. Flag decoding in the diagnostics
    /// layer must consult this and warn instead of applying the flag.
    #[must_use]
    pub fn is_expired(self) -> bool {
        self.meta().expired
    }
}

/// Whether the `>>>` token lexes as the triple-shift operator for a library
/// at language `version`.
///
/// Official source: the `triple-shift` entry in the vendored yaml
/// (`enabledIn: '2.14.0'`), confirmed by the SDK-generated
/// `ExperimentalFlag.tripleShift`
/// (`pkg/_fe_analyzer_shared/lib/src/experiments/flags.dart`,
/// `experimentEnabledVersion: Version(2, 14)`). The CFE feeds exactly this
/// predicate into `ScannerConfiguration.enableTripleShift`
/// (`pkg/_fe_analyzer_shared/lib/src/parser/experimental_features.dart`).
#[must_use]
pub fn triple_shift_enabled(version: LanguageVersion) -> bool {
    FeatureSet::for_language_version(version).is_enabled(Feature::TripleShift)
}

/// A Dart language version (`major.minor`), e.g. `3.13`.
///
/// The version a library is analyzed under comes from the package config or
/// a `// @dart = <version>` override comment, defaulting to
/// [`DEFAULT_LANGUAGE_VERSION`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LanguageVersion {
    pub major: u32,
    pub minor: u32,
}

impl LanguageVersion {
    #[must_use]
    pub const fn new(major: u32, minor: u32) -> Self {
        Self { major, minor }
    }

    /// Whether this version is greater than the latest known language
    /// version ([`DEFAULT_LANGUAGE_VERSION`]).
    ///
    /// Official behavior for a `// @dart` override above the latest known
    /// version: the scanner emits `INVALID_LANGUAGE_VERSION_OVERRIDE` as a
    /// `STATIC_WARNING` and drops the override (`scanner.dart`:
    /// `_overrideVersion = null`), so the library version re-resolves through
    /// the normal chain — package config, else the default — before any
    /// feature computation (measured in `docs/facts/version-skew.md` §1.3
    /// T2; against the 3.13 oracle, `@dart = 3.14` warned while
    /// `@dart = 3.13` did not — the drop only looks like a clamp when the
    /// fallback resolves to the default, which equals latest).
    /// [`FeatureSet::for_language_version`] does not drop anything — it
    /// already produces the default set for such versions, so the only
    /// missing piece is the warning, which the flag-decode site emits when
    /// this returns `true`.
    #[must_use]
    pub fn is_above_latest(self) -> bool {
        self > DEFAULT_LANGUAGE_VERSION
    }
}

impl fmt::Display for LanguageVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Self { major, minor } = self;
        write!(f, "{major}.{minor}")
    }
}

/// Error returned when a string cannot be parsed as a [`LanguageVersion`].
#[derive(Clone, Debug, PartialEq, Eq, Hash, thiserror::Error)]
#[error(
    "invalid Dart language version `{received}`: expected `<major>.<minor>` or `<major>.<minor>.<patch>`"
)]
pub struct LanguageVersionParseError {
    received: String,
}

impl FromStr for LanguageVersion {
    type Err = LanguageVersionParseError;

    /// Parses `major.minor` (package config and `// @dart` override style) or
    /// `major.minor.patch` (SDK release style, e.g. the yaml's `'3.13.0'`);
    /// a patch part is accepted and ignored. Every part must be bare ASCII
    /// digits — the official grammar has no sign (a leading `+` would be
    /// accepted by `u32::from_str`).
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let err = || LanguageVersionParseError {
            received: s.to_owned(),
        };
        let mut parts = s.split('.');
        let (Some(major), Some(minor)) = (parts.next(), parts.next()) else {
            return Err(err());
        };
        let (Some(major), Some(minor)) = (parse_digits(major), parse_digits(minor)) else {
            return Err(err());
        };
        match parts.next() {
            None => Ok(Self { major, minor }),
            Some(patch) if parse_digits(patch).is_some() && parts.next().is_none() => {
                Ok(Self { major, minor })
            }
            Some(_) => Err(err()),
        }
    }
}

/// Parses a version-number part: a non-empty run of ASCII digits, nothing
/// else. `u32::from_str` alone would also accept a leading `+`.
fn parse_digits(s: &str) -> Option<u32> {
    if !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()) {
        s.parse().ok()
    } else {
        None
    }
}

/// Error from `Feature::try_from(&str)`: the string does not name a known
/// experiment flag in the vendored registry.
#[derive(Clone, Debug, PartialEq, Eq, Hash, thiserror::Error)]
#[error("unknown experiment flag `{0}`")]
pub struct UnknownFeature(pub String);

/// The set of language features enabled for a library.
///
/// Derived from the library's [`LanguageVersion`]
/// ([`FeatureSet::for_language_version`]) plus explicit experiment-flag
/// opt-ins ([`FeatureSet::with_experiment`]). Backed by a single `u128` bit
/// set: bit *i* corresponds to registry entry *i* (vendored yaml order, see
/// [`FEATURES`]).
#[derive(Clone, Copy, Default, PartialEq, Eq, Hash, Debug)]
pub struct FeatureSet(u128);

impl FeatureSet {
    /// Computes the features enabled by default for a library at `version`.
    ///
    /// A feature with `enabledIn` is enabled exactly when
    /// `version >= enabledIn`; a feature without `enabledIn` is disabled here
    /// and requires [`FeatureSet::with_experiment_for_version`]. Expiry never
    /// affects this computation — see the crate docs.
    #[must_use]
    pub fn for_language_version(version: LanguageVersion) -> Self {
        let mut bits = 0u128;
        for (index, meta) in FEATURES.iter().enumerate() {
            if meta
                .enabled_in
                .is_some_and(|enabled_in| version >= enabled_in)
            {
                bits |= 1 << index;
            }
        }
        Self(bits)
    }

    /// Features enabled by default at [`DEFAULT_LANGUAGE_VERSION`].
    #[must_use]
    pub fn latest() -> Self {
        Self::for_language_version(DEFAULT_LANGUAGE_VERSION)
    }

    /// Explicitly enables `feature`, as `--enable-experiment=<flag>` does.
    ///
    /// Bypasses the official release-version gate: the bit is set
    /// unconditionally, whatever language version this set was derived from.
    /// For genuine internal construction and tests only — SDK
    /// allowed-experiment grants must use
    /// [`FeatureSet::with_experiment_for_version`], which re-restricts
    /// grants to the effective language version under the official
    /// expired/release-gate contract.
    #[must_use]
    pub fn with_experiment(mut self, feature: Feature) -> Self {
        self.0 |= 1 << feature as usize;
        self
    }

    /// Explicitly enables `feature` for a library at language `version`, as
    /// `--enable-experiment=<flag>` does, under the official decode contract
    /// (`experiments_impl.dart::restrictEnableFlagsToVersion` in
    /// `pkg/analyzer/lib/src/dart/analysis/`):
    ///
    /// * Expired flags are dropped before any version check
    ///   (`_flagStringsToMap` skips `isExpired` entries) and never change
    ///   enablement. The warning the real toolchain emits at this point
    ///   (`UnnecessaryUseOfExpiredFlag`/`IllegalUseOfExpiredFlag`) is a
    ///   diagnostics concern — see [`Feature::is_expired`].
    /// * A flag without an assigned experimental release version opts the
    ///   library in only at the exact current version
    ///   (`version == sdkLanguageVersion`): "the current sdk version (whatever
    ///   it is) is always used as the language version which opts code into
    ///   the experiment".
    /// * Any other flag opts the library in from
    ///   `min(DEFAULT_LANGUAGE_VERSION, experimentalReleaseVersion)` on
    ///   (`restrictEnableFlagsToVersion`: "experimental features should be
    ///   available since `min(sdk, experimentalRelease)`").
    #[must_use]
    pub fn with_experiment_for_version(
        mut self,
        version: LanguageVersion,
        feature: Feature,
    ) -> Self {
        if feature.is_expired() {
            // Diagnostics emission point for the expired-flag warning.
            return self;
        }
        let enabled = match feature.meta().experimental_release_version {
            // Official exact-equality gate for flags without a release
            // version — not `>=`, or versions above the current one would
            // wrongly opt in too.
            None => version == DEFAULT_LANGUAGE_VERSION,
            Some(release) => version >= release.min(DEFAULT_LANGUAGE_VERSION),
        };
        if enabled {
            self.0 |= 1 << feature as usize;
        }
        self
    }

    /// Whether `feature` is enabled in this set.
    #[must_use]
    pub fn is_enabled(&self, feature: Feature) -> bool {
        self.0 & (1 << feature as usize) != 0
    }

    /// Stable 64-bit fingerprint of the set: FNV-1a over the little-endian
    /// bit bytes. Deterministic across runs and platforms; reserved as the
    /// future `FeatureSetId` key. Fingerprints derive from registry bit
    /// order, so re-vendoring the yaml can shift bits and change
    /// fingerprints.
    ///
    /// Collision stance: at 64 bits the birthday bound sits at ~2^32
    /// distinct sets (contrast `dartq_source::ContentId`, which documents
    /// its 128-bit truncation rationale). Collisions are tolerated as a
    /// cache-key equivalence risk, not treated as a security property: the
    /// real population is the few thousand distinguishable sets over
    /// [`FEATURE_COUNT`] bits, so a colliding pair can only alias a cache
    /// entry. Any correctness check must compare the full bit set and use
    /// the fingerprint only as a lookup/pre-filter key.
    #[must_use]
    pub fn fingerprint(&self) -> u64 {
        const OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
        const PRIME: u64 = 0x0000_0100_0000_01b3;
        let mut hash = OFFSET_BASIS;
        for byte in self.0.to_le_bytes() {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(PRIME);
        }
        hash
    }
}
