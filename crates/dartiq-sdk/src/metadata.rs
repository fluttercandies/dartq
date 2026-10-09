//! The SDK metadata data model: everything a loaded [`SdkSnapshot`] carries.
//!
//! Serialization of these types is the canonical form behind
//! [`SdkSnapshot::id`](crate::SdkSnapshot::id): maps are `BTreeMap` (sorted
//! keys), vectors keep source order, and `serde_json` emits struct fields in
//! declaration order — so the same parsed metadata always serializes to the
//! same bytes.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::PathBuf;

use serde::Serialize;

use dartiq_language::{Feature, FeatureSet, LanguageVersion};
use dartiq_source::ContentId;

/// Libraries the compiler implicitly imports into every root compilation
/// unit: exactly `dart:core`.
///
/// Both compilers hardcode this single entry. The CFE injects the core
/// library's export namespace into the root unit when there is no explicit
/// import (`pkg/front_end/lib/src/source/source_compilation_unit.dart`,
/// "dart:core should only be implicitly imported into the root compilation
/// unit"); the analyzer pre-discovers `dart:core` before any analysis
/// (`pkg/analyzer/lib/src/dart/analysis/driver.dart`, "`dart:core` is always
/// implicitly imported"). `dart:collection` is *not* implicit — the CFE only
/// pre-reads it to desugar const sets, which is not a scope injection.
pub const IMPLICITLY_IMPORTED_LIBRARIES: [&str; 1] = ["core"];

/// A loaded SDK: parsed metadata, its canonical identity, and the dart-sdk
/// root it was loaded from.
///
/// Constructed only by [`SdkSnapshot::load`](crate::SdkSnapshot::load).
#[derive(Debug)]
pub struct SdkSnapshot {
    /// Canonical content name of the parsed metadata: stable across loads of
    /// the same metadata and across differing JSON layouts of equal content;
    /// distinct whenever any identity-relevant value differs.
    pub id: ContentId,
    /// dart identity: the `version`/`revision` file values and the raw
    /// `libraries.json` content hash (change detection; not part of `id`).
    pub dart: SdkIdentity,
    /// `libraries.json` parse result, per target segment, with `include`
    /// chains already expanded.
    pub sdk_libraries: SdkLibraries,
    /// `allowed_experiments.json` parse result: the static experiment
    /// whitelist (version-interval independent).
    pub platform_metadata: AllowedExperiments,
    /// Language version derived from the `version` file: `major.minor` with
    /// the patch always zero.
    pub language_metadata: LanguageVersion,
    pub(crate) dart_sdk_root: PathBuf,
}

/// dart-side identity: the `version`/`revision` file values plus a hash of
/// the raw `libraries.json` bytes.
///
/// The hash is for change detection only (the same parsed table in a
/// different layout hashes differently); the snapshot id deliberately hashes
/// the parsed form instead, which keeps 3.9.0/3.10.0-style SDKs that ship an
/// identical `libraries.json` deduplicatable while version strings still
/// distinguish the snapshots.
#[derive(Debug, Clone)]
pub struct SdkIdentity {
    /// Trimmed content of `<dart-sdk>/version`, e.g. `"3.10.0"`.
    pub version: String,
    /// Trimmed content of `<dart-sdk>/revision` (the dart git hash), absent
    /// in source checkouts.
    pub revision: Option<String>,
    /// blake3-128 of the raw `libraries.json` bytes.
    pub libraries_json_hash: ContentId,
}

/// The `libraries.json` parse result: every target segment, `include` chains
/// expanded into a flat per-segment library table.
#[derive(Debug, Clone, Serialize)]
pub struct SdkLibraries {
    /// Segment name (internal `_`-prefixed combination segments included)
    /// → expanded segment.
    pub targets: BTreeMap<String, TargetLibraries>,
}

impl SdkLibraries {
    /// The expanded segment named `segment`, or `None` when this SDK's
    /// `libraries.json` has no such segment (segment sets drift across
    /// versions, so they are addressed by name — `vm`, `dart2js`,
    /// `dartdevc`, the wasm family — never by a hardcoded enumeration).
    #[must_use]
    pub fn get(&self, segment: &str) -> Option<&TargetLibraries> {
        self.targets.get(segment)
    }
}

/// One target segment after `include` expansion: the effective library set
/// for that target, own-segment definitions already overriding inherited
/// ones.
#[derive(Debug, Clone, Serialize)]
pub struct TargetLibraries {
    /// Library name (leading `_` internal names preserved) → metadata.
    pub libraries: BTreeMap<String, SdkLibrary>,
}

/// One `dart:` library within one target segment.
///
/// `patches` is kept as path list only — patch files are backend
/// implementation details that do not change the public API surface, so they
/// are stored for later platform-file localization but never read
/// (`docs/facts/sdk-metadata.md` §6).
#[derive(Debug, Clone, Serialize)]
pub struct SdkLibrary {
    /// Implementation file relative to `<dart-sdk>/lib`, e.g.
    /// `"core/core.dart"`.
    pub uri: String,
    /// Backend patch files relative to the same root, source order
    /// preserved (JSON string form normalized to a one-element list).
    pub patches: Vec<String>,
    /// Whether this library produces `dart.library.<name>` = `true` keys.
    /// Defaults to `true`; forced `false` for `_`-prefixed names.
    pub support_conditional_import: bool,
    /// Direct-import gate (`import "dart:x"` without a configuration).
    pub support_direct_import: Importability,
}

/// The `support_direct_import` gate of a [`SdkLibrary`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Importability {
    /// Importable unconditionally.
    Always,
    /// Importable only with an opt-in flag.
    WithFlag,
    /// Not directly importable.
    Never,
}

impl Importability {
    /// Parses the `support_direct_import` JSON value.
    pub(crate) fn from_spec(value: &str) -> Result<Self, String> {
        match value {
            "always" => Ok(Self::Always),
            "with_flag" => Ok(Self::WithFlag),
            "never" => Ok(Self::Never),
            other => Err(format!(
                "invalid support_direct_import value `{other}`, expected \
                 `always`, `with_flag`, or `never`"
            )),
        }
    }
}

/// The `allowed_experiments.json` parse result: the static whitelist of
/// language experiments.
///
/// Sets are resolved at parse time (a reference to an undefined experiment
/// set is an error), so every query here is total. Semantics follow
/// `pkg/_fe_analyzer_shared/lib/src/sdk/allowed_experiments.dart`; the
/// `sdk._example_libraries` key present in real files is deliberately
/// ignored, as the official parser only reads `sdk.default` and
/// `sdk.libraries`.
#[derive(Debug, Clone, Default, Serialize)]
pub struct AllowedExperiments {
    sdk_default: BTreeSet<String>,
    sdk_libraries: BTreeMap<String, BTreeSet<String>>,
    packages: BTreeMap<String, BTreeSet<String>>,
}

impl AllowedExperiments {
    pub(crate) fn new(
        sdk_default: BTreeSet<String>,
        sdk_libraries: BTreeMap<String, BTreeSet<String>>,
        packages: BTreeMap<String, BTreeSet<String>>,
    ) -> Self {
        Self {
            sdk_default,
            sdk_libraries,
            packages,
        }
    }

    /// Experiments enabled for the SDK library `name` (without the `dart:`
    /// prefix), falling back to the SDK default set — the official
    /// `forSdkLibrary` semantics.
    #[must_use]
    pub fn for_sdk_library(&self, name: &str) -> &BTreeSet<String> {
        self.sdk_libraries.get(name).unwrap_or(&self.sdk_default)
    }

    /// Experiments enabled for package `name`, or `None` when the file
    /// grants that package no override — the official `forPackage`
    /// semantics (no default fallback).
    #[must_use]
    pub fn for_package(&self, name: &str) -> Option<&BTreeSet<String>> {
        self.packages.get(name)
    }

    /// The [`FeatureSet`] for SDK library `name` at language `version`: the
    /// version-derived baseline plus every whitelisted experiment the
    /// registry knows.
    ///
    /// # Errors
    ///
    /// [`UnknownExperiments`] when the whitelist names an experiment the
    /// vendored feature registry does not know — a version skew between the
    /// analyzed SDK and dartiq-language that must surface, not silently
    /// disable experiments.
    pub fn feature_set_for_sdk_library(
        &self,
        name: &str,
        version: LanguageVersion,
    ) -> Result<FeatureSet, UnknownExperiments> {
        feature_set_with_experiments(version, self.for_sdk_library(name))
    }

    /// The [`FeatureSet`] for package `name` at language `version`; a
    /// package without a grant gets the version-derived baseline only.
    ///
    /// # Errors
    ///
    /// [`UnknownExperiments`] — see [`Self::feature_set_for_sdk_library`].
    pub fn feature_set_for_package(
        &self,
        name: &str,
        version: LanguageVersion,
    ) -> Result<FeatureSet, UnknownExperiments> {
        match self.for_package(name) {
            Some(experiments) => feature_set_with_experiments(version, experiments),
            None => Ok(FeatureSet::for_language_version(version)),
        }
    }
}

/// Applies `experiments` on top of the version-derived baseline.
fn feature_set_with_experiments(
    version: LanguageVersion,
    experiments: &BTreeSet<String>,
) -> Result<FeatureSet, UnknownExperiments> {
    let mut set = FeatureSet::for_language_version(version);
    let mut unknown = Vec::new();
    for name in experiments {
        match Feature::try_from(name.as_str()) {
            Ok(feature) => set = set.with_experiment(feature),
            Err(_) => unknown.push(name.clone()),
        }
    }
    if unknown.is_empty() {
        Ok(set)
    } else {
        Err(UnknownExperiments(unknown))
    }
}

/// One or more experiment names in an `allowed_experiments.json` whitelist
/// that the vendored feature registry ([`Feature`]) does not know.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnknownExperiments(pub Vec<String>);

impl fmt::Display for UnknownExperiments {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let names = self
            .0
            .iter()
            .map(|name| format!("`{name}`"))
            .collect::<Vec<_>>()
            .join(", ");
        write!(
            f,
            "unknown experiment flag(s) {names}: not in the vendored \
             experimental_features.yaml of dartiq-language; re-vendor a \
             newer snapshot to support this SDK"
        )
    }
}

impl std::error::Error for UnknownExperiments {}
