//! Constructor-time feature folding (official `parser_impl.dart:316-373`).
//!
//! The official parser folds its `ExperimentalFeatures` into final bools in
//! the initializer list and never consults the feature set again. We do the
//! same: [`Parser::new`](crate::Parser::new) folds a
//! [`FeatureSet`] into a plain [`ParserFeatures`] struct, and all grammar
//! dispatch reads plain fields.
//!
//! Division of labor with the scanner (its `ScanConfig`): `triple_shift` and
//! `augmentations` are **lexical** gates — the scanner decides whether
//! `>>>` lexes as one token and whether `augment` is a keyword (scanner
//! crate docs). The parser needs no fields for them: when the feature is
//! off, the scanner hands it plain `>>`+`>` sequences and identifiers, and
//! the ordinary grammar paths do the right thing. `patterns` and `variance`
//! are **syntactic** gates owned here.

use dartq_language::FeatureSet;

/// Grammar variants decided once at parser construction (mirrors the
/// official `FeatureSet` record, which is likewise a flat flag bundle).
#[allow(clippy::struct_excessive_bools)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ParserFeatures {
    /// `isPatternsFeatureEnabled` (`parser_impl.dart:325)`: the whole pattern
    /// production family. When off, [`Parser::parse_pattern`](crate::Parser::parse_pattern)
    /// reports `experiment_not_enabled` at the pattern start and consumes
    /// nothing.
    pub patterns_enabled: bool,
    /// Variance modifier parsing in type parameter declarations
    /// (`allowsVariance` in `parseVariables`, `type_info_impl.dart:1492)`:
    /// opt-in experiment with no `enabledIn`, so it is only on via an
    /// explicit `--enable-experiment`.
    pub variance_enabled: bool,
    /// `isPrimaryConstructorsFeatureEnabled` (`parser_impl.dart`): primary
    /// constructor declarations (`class C(...)`, `enum E;`, `this` members),
    /// shipped in 3.13 (`enabledIn: 3.13.0`).
    pub primary_constructors_enabled: bool,
    /// `isAugmentationsFeatureEnabled`: `augment`able declarations
    /// (`abstract` fields/methods outside augmentations). The scanner's
    /// `ScanConfig::enable_augmentations` is the lexical side of the same
    /// gate — when it is off, `augment` never reaches the parser as a
    /// keyword at all.
    pub augmentations_enabled: bool,
    /// `isEnhancedPartsFeatureEnabled` (`parser_impl.dart:328`): the
    /// directive-ordering relaxation in a `part of` file (`DirectiveContext`,
    /// `directive_context.dart:63-67`) — with the feature on, directives
    /// after `part of` are admitted instead of reported.
    pub enhanced_parts_enabled: bool,
}

impl ParserFeatures {
    /// Folds a [`FeatureSet`] into the parser's plain flags.
    ///
    /// Version-derived enablement comes from the registry's `enabledIn`
    /// (`FeatureSet::for_language_version`); the caller can additionally
    /// layer `--enable-experiment` opt-ins onto the set beforehand.
    #[must_use]
    pub fn from_feature_set(features: FeatureSet) -> Self {
        Self {
            patterns_enabled: features.is_enabled(dartq_language::Feature::Patterns),
            variance_enabled: features.is_enabled(dartq_language::Feature::Variance),
            primary_constructors_enabled: features
                .is_enabled(dartq_language::Feature::PrimaryConstructors),
            augmentations_enabled: features.is_enabled(dartq_language::Feature::Augmentations),
            enhanced_parts_enabled: features.is_enabled(dartq_language::Feature::EnhancedParts),
        }
    }
}
