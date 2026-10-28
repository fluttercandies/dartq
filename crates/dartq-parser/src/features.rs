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
//! crate docs). The parser needs no field for `triple_shift`: when the
//! feature is off, the scanner hands it plain `>>`+`>` sequences and the
//! ordinary grammar paths do the right thing. `patterns` and `variance`
//! are **syntactic** gates owned here.
//!
//! The fold is keyed by the generated registry: one pass over
//! [`Feature::ALL`], each enabled feature applied through
//! [`ParserFeatures::enable`] — the parser's per-feature stance table,
//! exhaustive over the generated enum with no wildcard arm, so a
//! re-vendored yaml entry without a parser stance fails compilation until
//! it gets one (an explicit inert arm or a new fold bit). No field here
//! hand-mirrors the yaml's `enabledIn`; version comparison stays in
//! `FeatureSet::for_language_version`.

use crate::listener::ExperimentalFlag;
use dartq_language::{Feature, FeatureSet};

/// Grammar variants decided once at parser construction (mirrors the
/// official `FeatureSet` record, which is likewise a flat flag bundle).
#[allow(clippy::struct_excessive_bools)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ParserFeatures {
    /// `isPatternsFeatureEnabled` (`parser_impl.dart:325)`: the whole pattern
    /// production family. When off, `Parser::parse_pattern`
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
    /// The dot-shorthand gate (`enabledIn: 3.10`, shipped — the shared
    /// parser has no flag for it; the analyzer's `AstBuilder` reports from
    /// its own `enabledDotShorthands`, `ast_builder.dart:135`). The parser
    /// always parses the shorthand shape; when off, the head anchors report
    /// `experiment_not_enabled` (`Parser::report_dot_shorthands_gate`).
    pub dot_shorthands_enabled: bool,
}

impl ParserFeatures {
    /// The all-off fold every feature application starts from.
    const OFF: Self = Self {
        patterns_enabled: false,
        variance_enabled: false,
        primary_constructors_enabled: false,
        augmentations_enabled: false,
        enhanced_parts_enabled: false,
        dot_shorthands_enabled: false,
    };

    /// Folds a [`FeatureSet`] into the parser's plain flags.
    ///
    /// One pass over the generated registry (`Feature::ALL`), each
    /// enabled feature applied through [`ParserFeatures::enable`] — the
    /// single place a feature's parser stance lives. Version-derived
    /// enablement comes from the registry's `enabledIn`
    /// (`FeatureSet::for_language_version`); the caller can additionally
    /// layer `--enable-experiment` opt-ins onto the set beforehand.
    #[must_use]
    pub fn from_feature_set(features: FeatureSet) -> Self {
        let mut folded = Self::OFF;
        for feature in Feature::ALL {
            if features.is_enabled(feature) {
                folded.enable(feature);
            }
        }
        folded
    }

    /// Applies one enabled feature to the fold — the parser's stance
    /// table, exhaustive over the generated registry enum on purpose (no
    /// wildcard arm): a re-vendored yaml entry stops compiling here until
    /// its parser stance is decided.
    fn enable(&mut self, feature: Feature) {
        match feature {
            Feature::Patterns => self.patterns_enabled = true,
            Feature::Variance => self.variance_enabled = true,
            Feature::PrimaryConstructors => self.primary_constructors_enabled = true,
            Feature::Augmentations => self.augmentations_enabled = true,
            Feature::EnhancedParts => self.enhanced_parts_enabled = true,
            Feature::DotShorthands => self.dot_shorthands_enabled = true,
            // Lexical only: `triple-shift` decides in the scanner
            // (`>>>` as one token); the parser sees its effect through
            // the token stream (the module docs' division of labor).
            Feature::TripleShift => {}
            // No parser stance: the grammar surface is version-blind for
            // these entries (the fixed 3.15 surface accepts the shipped
            // shapes; the experimental ones have no production here), and
            // their lint/semantic faces live in their own projections.
            Feature::ConstFunctions
            | Feature::Macros
            | Feature::RecordUse
            | Feature::DataAssets
            | Feature::UnquotedImports
            | Feature::InferenceUpdate4
            | Feature::ThisPromotion
            | Feature::StaticExtensions
            | Feature::AnonymousMethods
            | Feature::TestExperiment
            | Feature::PrivateNamedParameters
            | Feature::NativeAssets
            | Feature::GetterSetterError
            | Feature::SoundFlowAnalysis
            | Feature::NullAwareElements
            | Feature::InferenceUsingBounds
            | Feature::WildcardVariables
            | Feature::DigitSeparators
            | Feature::InferenceUpdate3
            | Feature::InlineClass
            | Feature::InferenceUpdate2
            | Feature::SealedClass
            | Feature::ClassModifiers
            | Feature::Records
            | Feature::UnnamedLibraries
            | Feature::AlternativeInvalidationStrategy
            | Feature::InferenceUpdate1
            | Feature::EnhancedEnums
            | Feature::NamedArgumentsAnywhere
            | Feature::SuperParameters
            | Feature::ConstructorTearoffs
            | Feature::GenericMetadata
            | Feature::NonfunctionTypeAliases
            | Feature::NonNullable
            | Feature::ExtensionMethods
            | Feature::ConstantUpdate2018
            | Feature::ControlFlowCollections
            | Feature::SetLiterals
            | Feature::SpreadCollections => {}
        }
    }
}

impl ExperimentalFlag {
    /// The yaml-registry entry this parser flag mirrors — the same
    /// [`dartq_language::Feature`] the gate fields above fold. The
    /// experiment-message selection reads the entry's metadata (`enabledIn`
    /// separates the shipped wording from the off-by-default one,
    /// `errors.dart:12`).
    #[must_use]
    pub(crate) fn feature(self) -> dartq_language::Feature {
        match self {
            Self::TripleShift => dartq_language::Feature::TripleShift,
            Self::Patterns => dartq_language::Feature::Patterns,
            Self::Variance => dartq_language::Feature::Variance,
            Self::Augmentations => dartq_language::Feature::Augmentations,
            Self::PrimaryConstructors => dartq_language::Feature::PrimaryConstructors,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The fold must equal the registry's own truth, field for field, at
    /// every version that decides differently: each bit is exactly the
    /// `enabledIn <= version` predicate over its yaml entry (clamped at
    /// the data horizon like every version-derived face). The matrix
    /// brackets every shipped boundary the six stances cross; a new fold
    /// bit without its registry truth — or a stance flip — fails here.
    #[test]
    fn fold_matches_the_registry_truth_across_versions() {
        const MATRIX: [dartq_language::LanguageVersion; 9] = [
            dartq_language::LanguageVersion::new(2, 12),
            dartq_language::LanguageVersion::new(2, 16),
            dartq_language::LanguageVersion::new(2, 19),
            dartq_language::LanguageVersion::new(2, 99),
            dartq_language::LanguageVersion::new(3, 0),
            dartq_language::LanguageVersion::new(3, 9),
            dartq_language::LanguageVersion::new(3, 10),
            dartq_language::LanguageVersion::new(3, 12),
            dartq_language::LanguageVersion::new(3, 15),
        ];
        let shipped = |feature: Feature, version: dartq_language::LanguageVersion| {
            let horizon = dartq_language::NEWEST_KNOWN_VERSION;
            let version = if version > horizon { horizon } else { version };
            feature
                .meta()
                .enabled_in
                .is_some_and(|enabled_in| version >= enabled_in)
        };
        for version in MATRIX {
            let folded = ParserFeatures::from_feature_set(
                dartq_language::FeatureSet::for_language_version(version),
            );
            let expected = ParserFeatures {
                patterns_enabled: shipped(Feature::Patterns, version),
                variance_enabled: shipped(Feature::Variance, version),
                primary_constructors_enabled: shipped(Feature::PrimaryConstructors, version),
                augmentations_enabled: shipped(Feature::Augmentations, version),
                enhanced_parts_enabled: shipped(Feature::EnhancedParts, version),
                dot_shorthands_enabled: shipped(Feature::DotShorthands, version),
            };
            assert_eq!(folded, expected, "parser fold drifts at {version}");
        }
    }

    /// The explicit opt-in face: a set carrying an unshipped experiment
    /// folds its bit on while the version-derived set keeps it off — the
    /// two construction faces `from_feature_set` must keep distinct.
    #[test]
    fn experiment_opt_in_folds_only_through_the_set() {
        let latest = dartq_language::FeatureSet::latest();
        assert!(!ParserFeatures::from_feature_set(latest).variance_enabled);
        let opted_in = latest.with_experiment(Feature::Variance);
        assert!(ParserFeatures::from_feature_set(opted_in).variance_enabled);
    }
}
