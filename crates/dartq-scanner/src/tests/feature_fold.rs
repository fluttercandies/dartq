//! The lexical-stance fold pins (`ScanConfig::from_feature_set`).

use crate::ScanConfig;
use dartq_language::{Feature, FeatureSet, LanguageVersion, NEWEST_KNOWN_VERSION};

/// The fold must equal the registry's own truth, bit for bit, across the
/// version matrix bracketing both stances' shipping boundaries
/// (`triple-shift` at 2.14, `augmentations` unshipped) and the horizon
/// clamp: each bit is exactly the `enabledIn <= version` predicate over
/// its yaml entry. The explicit opt-in face keeps an unshipped
/// experiment foldable on — the construction faces stay distinct.
#[test]
fn scan_fold_matches_the_registry_truth_across_versions() {
    let shipped = |feature: Feature, version: LanguageVersion| {
        let version = if version > NEWEST_KNOWN_VERSION {
            NEWEST_KNOWN_VERSION
        } else {
            version
        };
        feature
            .meta()
            .enabled_in
            .is_some_and(|enabled_in| version >= enabled_in)
    };
    for version in [
        LanguageVersion::new(2, 12),
        LanguageVersion::new(2, 13),
        LanguageVersion::new(2, 14),
        LanguageVersion::new(3, 13),
        LanguageVersion::new(3, 15),
    ] {
        let folded = ScanConfig::from_feature_set(FeatureSet::for_language_version(version));
        let expected = ScanConfig {
            enable_triple_shift: shipped(Feature::TripleShift, version),
            enable_augmentations: shipped(Feature::Augmentations, version),
        };
        assert_eq!(folded, expected, "scan fold drifts at {version}");
    }
    let opted_in = FeatureSet::latest().with_experiment_for_version(
        dartq_language::DEFAULT_LANGUAGE_VERSION,
        Feature::Augmentations,
    );
    assert!(ScanConfig::from_feature_set(opted_in).enable_augmentations);
    assert!(!ScanConfig::from_feature_set(FeatureSet::latest()).enable_augmentations);
}
