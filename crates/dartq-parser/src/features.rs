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
//! crate docs). `triple_shift` is lexical *and* syntactic: with the feature
//! off the scanner hands the parser adjacent `>>`+`>` sequences, and the
//! parser's split-shape recovery faces (`parser_impl.dart:5790/:7262/:7392`)
//! report `experiment_not_enabled` and continue as the merged operator, so
//! the fold carries the bit too. `patterns` and `variance` are **syntactic**
//! gates owned here.
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
    /// `isAnonymousMethodsFeatureEnabled` (`parser_impl.dart:364-365`): the
    /// anonymous-method member family — `receiver.{ ... }`, `receiver.=> e`,
    /// `receiver.(p) { ... }` and their `?.`/`..`/`?..` faces
    /// (`parser_impl.dart:7292-7295` selector, :7770-7801 cascade). Opt-in
    /// with no `enabledIn`. When off, the member dispatch falls through to
    /// the ordinary selector path: the upstream `reportExperimentNotEnabled`
    /// call is an upstream unimplemented stub that never fires, so the off
    /// posture is plain `missing_identifier`/`expected_token` noise
    /// (oracle-pinned).
    pub anonymous_methods_enabled: bool,
    /// The sound-flow-analysis posture (`enabledIn: 3.9.0`, expired — the
    /// analyzer wires it as `soundFlowAnalysisEnabled:
    /// featureSet.isEnabled(Feature.sound_flow_analysis)`,
    /// type_analyzer_options.dart:20). No grammar face: the flow-analysis
    /// replay reads it through the fold's `ParserFeatures` — a pre-3.9
    /// unit (a `// @dart = 3.8` library) analyzes with the sound-only
    /// folds (nullability-disjoint test/cast failures, trivially
    /// satisfied checks, `??` RHS kills, pattern cannot-match kills)
    /// standing down.
    pub sound_flow_analysis: bool,
    /// `records` (`enabledIn: 3.0.0`): record literals and record types.
    /// When off, the analyzer's `AstBuilder` reports
    /// `experiment_not_enabled` at the record's `(` and degrades the node
    /// (`endRecordLiteral` ast_builder.dart:2786, `endRecordType`
    /// :2831) — the parse continues on the parenthesized shape.
    pub records_enabled: bool,
    /// `class-modifiers` (`enabledIn: 3.0.0`): the `base`/`interface`/
    /// `final` class modifiers and the `mixin` of `mixin class`. When off,
    /// each modifier reports `experiment_not_enabled` at its own token
    /// (`beginClassDeclaration` ast_builder.dart:252-282,
    /// `beginNamedMixinApplication` :524-553, `beginMixinDeclaration`
    /// :487-494 for `base`) and is dropped from the declaration.
    pub class_modifiers_enabled: bool,
    /// `sealed-class` (`enabledIn: 3.0.0`): the `sealed` modifier, gated
    /// separately from [`Self::class_modifiers_enabled`] exactly as the
    /// analyzer gates it (`beginClassDeclaration` ast_builder.dart:242-250).
    pub sealed_class_enabled: bool,
    /// `digit-separators` (`enabledIn: 3.6.0`): `_` in int/double literals.
    /// The scanner always lexes the separators; the analyzer's
    /// `handleLiteral{Int,Double}WithSeparators` report
    /// `experiment_not_enabled` at the literal token when off
    /// (ast_builder.dart:4680/:4719).
    pub digit_separators_enabled: bool,
    /// `triple-shift` (`enabledIn: 2.14.0`): the lexical gate decides in
    /// the scanner (`>>>` as one token; the module docs' division of
    /// labor); with it off the scanner hands the parser adjacent `>>`+`>`
    /// sequences and the parser's split-shape faces report
    /// `experiment_not_enabled` and continue as the merged operator
    /// (`parser_impl.dart:5790` operator member, :7262 `>>>=`, :7392
    /// binary `>>>`).
    pub triple_shift_enabled: bool,
    /// `constructor-tearoffs` (`enabledIn: 2.15.0`): `new` rewritten to an
    /// identifier (`A.new`) and a standalone explicit type-argument
    /// application (`f<int>`). When off, `handleNewAsIdentifier`
    /// (ast_builder.dart:4984) and `handleTypeArgumentApplication`
    /// (ast_builder.dart:5602) each report `experiment_not_enabled` — the
    /// first at the `new`, the second over the `<...>` group.
    pub constructor_tearoffs_enabled: bool,
    /// `named-arguments-anywhere` (`enabledIn: 2.17.0`): a positional
    /// argument after a named one. When off, `endArguments`
    /// (ast_builder.dart:970) reports `positional_after_named_argument`
    /// over the positional's expression.
    pub named_arguments_anywhere_enabled: bool,
    /// `super-parameters` (`enabledIn: 2.17.0`): a `super.x` formal
    /// parameter. When off, `endFormalParameter` (ast_builder.dart:1789)
    /// reports `experiment_not_enabled` at the `super` keyword.
    pub super_parameters_enabled: bool,
    /// `enhanced-enums` (`enabledIn: 2.17.0`): enum constants with
    /// arguments or a named/generic constructor selector
    /// (`handleEnumElement`, ast_builder.dart:4035), a `;` closing the
    /// constant list (`handleEnumElements`, :4094) and a header with a
    /// with-clause, implements-clause or type parameters
    /// (`handleEnumHeader`, :4131) each report `experiment_not_enabled`
    /// when off — one row, anchored on the first present construct.
    pub enhanced_enums_enabled: bool,
    /// `generic-metadata` (`enabledIn: 2.14.0`): type arguments on an
    /// annotation. When off, `endMetadata` (ast_builder.dart:2423) reports
    /// `experiment_not_enabled` at the group's `<`.
    pub generic_metadata_enabled: bool,
    /// `nonfunction-type-aliases` (`enabledIn: 2.13.0`): a `typedef X = T;`
    /// whose right-hand type is not a function type. When off,
    /// `endGenericTypeAlias` (ast_builder.dart:3373) reports
    /// `experiment_not_enabled` at the `=`.
    pub nonfunction_type_aliases_enabled: bool,
    /// `inline-class` (`enabledIn: 3.3.0`): the `extension type`
    /// declaration. When off, `endExtensionTypeDeclaration`
    /// (ast_builder.dart:1607) reports `experiment_not_enabled` at the
    /// `type` keyword.
    pub inline_class_enabled: bool,
    /// `unnamed-libraries` (`enabledIn: 2.19.0`): a bare `library;`
    /// directive. When off, `endLibraryName` (ast_builder.dart:2281)
    /// reports `experiment_not_enabled` at the `library` keyword.
    pub unnamed_libraries_enabled: bool,
    /// `null-aware-elements` (`enabledIn: 3.8.0`): `?e` elements,
    /// `?k: v`/`k: ?v` map entries. When off, `handleNullAwareElement`
    /// (ast_builder.dart:5119) and `handleLiteralMapEntry` (:4771) report
    /// `experiment_not_enabled` at the `?` and drop the marker.
    pub null_aware_elements_enabled: bool,
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
        anonymous_methods_enabled: false,
        sound_flow_analysis: false,
        records_enabled: false,
        class_modifiers_enabled: false,
        sealed_class_enabled: false,
        digit_separators_enabled: false,
        triple_shift_enabled: false,
        constructor_tearoffs_enabled: false,
        named_arguments_anywhere_enabled: false,
        super_parameters_enabled: false,
        enhanced_enums_enabled: false,
        generic_metadata_enabled: false,
        nonfunction_type_aliases_enabled: false,
        inline_class_enabled: false,
        unnamed_libraries_enabled: false,
        null_aware_elements_enabled: false,
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
            // Grammar gate: the anonymous-method member family (the field's
            // doc) — dispatched in the selector and cascade arms.
            Feature::AnonymousMethods => self.anonymous_methods_enabled = true,
            // Not a grammar gate: the fold carries the posture for the
            // flow-analysis replay (the field's doc).
            Feature::SoundFlowAnalysis => self.sound_flow_analysis = true,
            // Version-gated grammar faces (the fields' docs): the shipped
            // features whose off-version parse still reports
            // `experiment_not_enabled`.
            Feature::Records => self.records_enabled = true,
            Feature::ClassModifiers => self.class_modifiers_enabled = true,
            Feature::SealedClass => self.sealed_class_enabled = true,
            Feature::DigitSeparators => self.digit_separators_enabled = true,
            // Lexical gate in the scanner (`>>>` as one token) plus the
            // parser's split-shape faces (the field's doc).
            Feature::TripleShift => self.triple_shift_enabled = true,
            Feature::ConstructorTearoffs => self.constructor_tearoffs_enabled = true,
            Feature::NamedArgumentsAnywhere => self.named_arguments_anywhere_enabled = true,
            Feature::SuperParameters => self.super_parameters_enabled = true,
            Feature::EnhancedEnums => self.enhanced_enums_enabled = true,
            Feature::GenericMetadata => self.generic_metadata_enabled = true,
            Feature::NonfunctionTypeAliases => self.nonfunction_type_aliases_enabled = true,
            Feature::InlineClass => self.inline_class_enabled = true,
            Feature::UnnamedLibraries => self.unnamed_libraries_enabled = true,
            Feature::NullAwareElements => self.null_aware_elements_enabled = true,
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
            | Feature::TestExperiment
            | Feature::PrivateNamedParameters
            | Feature::NativeAssets
            | Feature::GetterSetterError
            | Feature::InferenceUsingBounds
            | Feature::WildcardVariables
            | Feature::InferenceUpdate3
            | Feature::InferenceUpdate2
            | Feature::AlternativeInvalidationStrategy
            | Feature::InferenceUpdate1
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
    /// brackets every shipped boundary the stances cross — 2.13
    /// (nonfunction-type-aliases), 2.14 (triple-shift / generic-metadata),
    /// 2.15 (constructor-tearoffs), 2.17 (named-arguments-anywhere /
    /// super-parameters / enhanced-enums), 2.19 (unnamed-libraries), 3.0
    /// (records / class-modifiers / sealed-class), 3.3 (inline-class), 3.6
    /// (digit-separators), 3.8 (null-aware-elements), 3.9
    /// (sound-flow-analysis), 3.10 (dot-shorthands), 3.13
    /// (primary-constructors) — on both sides; a new fold bit without its
    /// registry truth — or a stance flip — fails here.
    #[test]
    fn fold_matches_the_registry_truth_across_versions() {
        const MATRIX: [dartq_language::LanguageVersion; 14] = [
            dartq_language::LanguageVersion::new(2, 12),
            dartq_language::LanguageVersion::new(2, 13),
            dartq_language::LanguageVersion::new(2, 14),
            dartq_language::LanguageVersion::new(2, 16),
            dartq_language::LanguageVersion::new(2, 19),
            dartq_language::LanguageVersion::new(2, 99),
            dartq_language::LanguageVersion::new(3, 0),
            dartq_language::LanguageVersion::new(3, 5),
            dartq_language::LanguageVersion::new(3, 6),
            // The sound-flow-analysis boundary's two sides (the #2837
            // posture: a `// @dart = 3.8` unit folds the bit off).
            dartq_language::LanguageVersion::new(3, 8),
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
                anonymous_methods_enabled: shipped(Feature::AnonymousMethods, version),
                sound_flow_analysis: shipped(Feature::SoundFlowAnalysis, version),
                records_enabled: shipped(Feature::Records, version),
                class_modifiers_enabled: shipped(Feature::ClassModifiers, version),
                sealed_class_enabled: shipped(Feature::SealedClass, version),
                digit_separators_enabled: shipped(Feature::DigitSeparators, version),
                triple_shift_enabled: shipped(Feature::TripleShift, version),
                constructor_tearoffs_enabled: shipped(Feature::ConstructorTearoffs, version),
                named_arguments_anywhere_enabled: shipped(Feature::NamedArgumentsAnywhere, version),
                super_parameters_enabled: shipped(Feature::SuperParameters, version),
                enhanced_enums_enabled: shipped(Feature::EnhancedEnums, version),
                generic_metadata_enabled: shipped(Feature::GenericMetadata, version),
                nonfunction_type_aliases_enabled: shipped(Feature::NonfunctionTypeAliases, version),
                inline_class_enabled: shipped(Feature::InlineClass, version),
                unnamed_libraries_enabled: shipped(Feature::UnnamedLibraries, version),
                null_aware_elements_enabled: shipped(Feature::NullAwareElements, version),
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
        assert!(
            !ParserFeatures::from_feature_set(latest).variance_enabled,
            "no enabledIn: latest never ships the bit"
        );
        assert!(
            !ParserFeatures::from_feature_set(latest).anonymous_methods_enabled,
            "no enabledIn: latest never ships the bit"
        );
        let opted_in = latest
            .with_experiment(Feature::Variance)
            .with_experiment(Feature::AnonymousMethods);
        let folded = ParserFeatures::from_feature_set(opted_in);
        assert!(folded.variance_enabled);
        assert!(folded.anonymous_methods_enabled);
    }
}
