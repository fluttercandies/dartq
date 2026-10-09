//! Tests for the generated feature registry, `LanguageVersion`, and
//! `FeatureSet`. Expected values are pinned to the vendored
//! `data/experimental_features.yaml` snapshot.

use std::collections::HashSet;

use dartq_language::{
    DEFAULT_LANGUAGE_VERSION, FEATURE_COUNT, FEATURES, Feature, FeatureSet, LanguageVersion,
    UnknownFeature, triple_shift_enabled,
};

fn lv(major: u32, minor: u32) -> LanguageVersion {
    LanguageVersion::new(major, minor)
}

#[test]
fn default_language_version_matches_sdk() {
    // Vendored yaml `current-version: '3.15.0'`; the SDK-generated
    // `defaultLanguageVersion` in _fe_analyzer_shared is `Version(3, 15)`.
    assert_eq!(DEFAULT_LANGUAGE_VERSION, lv(3, 15));
}

#[test]
fn language_version_display() {
    assert_eq!(lv(3, 15).to_string(), "3.15");
    assert_eq!(lv(2, 12).to_string(), "2.12");
}

#[test]
fn language_version_parses_two_and_three_part() {
    assert_eq!("3.15".parse::<LanguageVersion>(), Ok(lv(3, 15)));
    assert_eq!("3.15.0".parse(), Ok(lv(3, 15)));
    assert_eq!("2.19".parse(), Ok(lv(2, 19)));
}

#[test]
fn language_version_rejects_malformed() {
    for bad in ["", "3", "a.b", "3.x", "-1.2", "3.15.0.1", " 3.15", "3.15 "] {
        let parsed = bad.parse::<LanguageVersion>();
        assert!(parsed.is_err(), "{bad:?} should not parse");
    }
}

/// Audit F3: the official grammar for version numbers is bare ASCII digits
/// (`abstract_scanner.dart` digit accumulation — no sign), while
/// `u32::from_str` accepts a leading `+`. A `+`-prefixed version must be
/// rejected, not silently parsed.
#[test]
fn language_version_rejects_leading_plus() {
    for bad in ["+3.15", "+3", "3.15.+", "++3"] {
        let parsed = bad.parse::<LanguageVersion>();
        assert!(parsed.is_err(), "{bad:?} should not parse");
    }
}

#[test]
fn language_version_parse_error_message() {
    let err = "nope".parse::<LanguageVersion>().unwrap_err();
    assert_eq!(
        err.to_string(),
        "invalid Dart language version `nope`: expected `<major>.<minor>` or `<major>.<minor>.<patch>`"
    );
}

#[test]
fn language_version_accepts_leading_zeros_like_official_scanner() {
    // The official `// @dart` scanner accumulates digits, so a leading zero
    // is just part of the number (`abstract_scanner.dart` digit scan):
    // "03.15" is 3.15. Pinned so this is not "fixed" into a divergence.
    assert_eq!("03.15".parse::<LanguageVersion>(), Ok(lv(3, 15)));
    assert_eq!("003.015".parse(), Ok(lv(3, 15)));
}

#[test]
fn language_version_orders_numerically() {
    // 3.9 < 3.10 catches a lexicographic-order bug.
    let mut versions = [
        lv(3, 15),
        lv(2, 12),
        lv(3, 9),
        lv(2, 19),
        lv(3, 0),
        lv(3, 10),
    ];
    versions.sort_unstable();
    assert_eq!(
        versions,
        [
            lv(2, 12),
            lv(2, 19),
            lv(3, 0),
            lv(3, 9),
            lv(3, 10),
            lv(3, 15)
        ]
    );
}

#[test]
fn registry_counts_are_consistent() {
    assert_eq!(FEATURE_COUNT, 49);
    assert_eq!(Feature::ALL.len(), FEATURE_COUNT);
    assert_eq!(FEATURES.len(), FEATURE_COUNT);
}

#[test]
fn registry_follows_yaml_order() {
    // First and last entries of the vendored yaml.
    assert_eq!(Feature::ALL[0], Feature::Variance);
    assert_eq!(Feature::ALL[FEATURE_COUNT - 1], Feature::SpreadCollections);
}

#[test]
fn registry_names_round_trip() {
    let mut names = HashSet::with_capacity(FEATURE_COUNT);
    for feature in Feature::ALL {
        assert_eq!(
            Feature::try_from(feature.meta().name),
            Ok(feature),
            "round trip for {:?}",
            feature.meta().name
        );
        assert_eq!(feature.meta().flag, feature.meta().name);
        names.insert(feature.meta().name);
    }
    assert_eq!(names.len(), FEATURE_COUNT, "names must be unique");
}

#[test]
fn try_from_matches_flag_strings() {
    assert_eq!(
        Feature::try_from("dot-shorthands"),
        Ok(Feature::DotShorthands)
    );
    assert_eq!(Feature::try_from("triple-shift"), Ok(Feature::TripleShift));
    assert_eq!(
        Feature::try_from("inference-update-4"),
        Ok(Feature::InferenceUpdate4)
    );
}

#[test]
fn try_from_rejects_unknown() {
    assert_eq!(
        Feature::try_from("no-such-thing"),
        Err(UnknownFeature("no-such-thing".to_owned()))
    );
    // The Dart-side camelCase name is not a flag string.
    assert_eq!(
        Feature::try_from("dotShorthands"),
        Err(UnknownFeature("dotShorthands".to_owned()))
    );
    assert_eq!(
        UnknownFeature("x".to_owned()).to_string(),
        "unknown experiment flag `x`"
    );
}

#[test]
fn shipped_features_gate_on_enabled_in() {
    let fs = FeatureSet::for_language_version(lv(2, 19));
    assert!(!fs.is_enabled(Feature::Records));
    assert!(!fs.is_enabled(Feature::ClassModifiers));
    let fs = FeatureSet::for_language_version(lv(3, 0));
    assert!(fs.is_enabled(Feature::Records));
    assert!(fs.is_enabled(Feature::ClassModifiers));
}

#[test]
fn dot_shorthands_boundary() {
    assert!(!FeatureSet::for_language_version(lv(3, 9)).is_enabled(Feature::DotShorthands));
    assert!(FeatureSet::for_language_version(lv(3, 10)).is_enabled(Feature::DotShorthands));
    assert!(FeatureSet::latest().is_enabled(Feature::DotShorthands));
}

#[test]
fn non_nullable_boundary() {
    assert!(!FeatureSet::for_language_version(lv(2, 11)).is_enabled(Feature::NonNullable));
    assert!(FeatureSet::for_language_version(lv(2, 12)).is_enabled(Feature::NonNullable));
}

/// Audit A7: shipped-feature boundary probes for the earlier eras — 2.0
/// (constant-update-2018, control-flow-collections, set-literals,
/// spread-collections), 2.6 (extension-methods), 2.13
/// (nonfunction-type-aliases), 2.15 (constructor-tearoffs), 2.17
/// (enhanced-enums), 2.19 (unnamed-libraries). Only 2.12/2.14/3.0/3.10 were
/// pinned before; the generation path is uniform, so these are cheap
/// insurance against a future vendored-yaml or generator regression.
#[test]
fn shipped_feature_boundaries_across_eras() {
    let cases = [
        (Feature::ConstantUpdate2018, (1, 0), (2, 0)),
        (Feature::ControlFlowCollections, (1, 0), (2, 0)),
        (Feature::SetLiterals, (1, 0), (2, 0)),
        (Feature::SpreadCollections, (1, 0), (2, 0)),
        (Feature::ExtensionMethods, (2, 5), (2, 6)),
        (Feature::NonfunctionTypeAliases, (2, 12), (2, 13)),
        (Feature::ConstructorTearoffs, (2, 14), (2, 15)),
        (Feature::EnhancedEnums, (2, 16), (2, 17)),
        (Feature::UnnamedLibraries, (2, 18), (2, 19)),
    ];
    for (feature, before, at) in cases {
        let (bm, bn) = before;
        let (am, an) = at;
        assert_eq!(
            feature.meta().enabled_in,
            Some(lv(am, an)),
            "{}: yaml enabledIn drifted",
            feature.meta().name
        );
        assert!(
            !FeatureSet::for_language_version(lv(bm, bn)).is_enabled(feature),
            "{} must be off at {bm}.{bn}",
            feature.meta().name
        );
        assert!(
            FeatureSet::for_language_version(lv(am, an)).is_enabled(feature),
            "{} must be on at {am}.{an}",
            feature.meta().name
        );
    }
}

#[test]
fn unshipped_features_default_off() {
    // augmentations and variance have no enabledIn: off even at the current
    // version, opt-in only.
    assert!(!FeatureSet::latest().is_enabled(Feature::Augmentations));
    assert!(!FeatureSet::latest().is_enabled(Feature::Variance));
}

#[test]
fn latest_matches_default_version() {
    assert_eq!(
        FeatureSet::latest(),
        FeatureSet::for_language_version(DEFAULT_LANGUAGE_VERSION)
    );
    assert!(FeatureSet::latest().is_enabled(Feature::Records));
    assert!(!FeatureSet::latest().is_enabled(Feature::Variance));
}

/// Audit A2: `is_above_latest` is the crate-level hook for the
/// `INVALID_LANGUAGE_VERSION_OVERRIDE` warning + override drop (the official
/// scanner emits the warning and nulls the override; the version then
/// re-resolves via the package-config/default chain). Measured oracle shape
/// (facts version-skew §1.3 T2, oracle latest = 3.13): `@dart = 3.14` is
/// above-latest (WARNING, override dropped), `@dart = 3.13` is not. Against
/// the vendored latest (3.15) the predicate fires at 3.16+.
#[test]
fn is_above_latest_pins_the_override_rejection_boundary() {
    assert_eq!(DEFAULT_LANGUAGE_VERSION, lv(3, 15));
    assert!(lv(3, 16).is_above_latest());
    assert!(lv(4, 0).is_above_latest());
    assert!(!lv(3, 15).is_above_latest());
    assert!(!lv(3, 13).is_above_latest());
}

/// Exercises the gate-bypassing constructor; flag decoding must use
/// `with_experiment_for_version` (see the A1 tests below).
#[test]
fn with_experiment_enables_and_preserves() {
    let base = FeatureSet::latest();
    assert!(!base.is_enabled(Feature::Variance));
    let opt_in = base.with_experiment(Feature::Variance);
    assert!(opt_in.is_enabled(Feature::Variance));
    // Existing version-derived bits survive; the base set is unchanged.
    assert!(opt_in.is_enabled(Feature::Records));
    assert!(!base.is_enabled(Feature::Variance));
}

/// Audit A1: the official decode layer gates an explicit
/// `--enable-experiment` flag on the feature's experimental release version
/// (`experiments_impl.dart::restrictEnableFlagsToVersion`: "experimental
/// features should be available since `min(sdk, experimentalRelease)`").
/// `augmentations` released in 3.6 (vendored yaml
/// `experimentalReleaseVersion: '3.6.0'`, SDK-generated
/// `ExperimentalFlag.augmentations.experimentReleasedVersion = Version(3, 6)`),
/// so the flag must be a no-op for a library at 3.5 and take effect from 3.6
/// on.
#[test]
fn explicit_flag_below_release_version_is_ignored() {
    assert_eq!(
        Feature::Augmentations.meta().experimental_release_version,
        Some(lv(3, 6))
    );
    let flagged_3_5 = FeatureSet::for_language_version(lv(3, 5))
        .with_experiment_for_version(lv(3, 5), Feature::Augmentations);
    assert!(!flagged_3_5.is_enabled(Feature::Augmentations));
    // From the release version on (and at the current version) the flag
    // opts the library in.
    for version in [lv(3, 6), DEFAULT_LANGUAGE_VERSION] {
        let flagged = FeatureSet::for_language_version(version)
            .with_experiment_for_version(version, Feature::Augmentations);
        assert!(flagged.is_enabled(Feature::Augmentations), "at {version}");
    }
}

/// A flag without an assigned experimental release version (yaml omits
/// `experimentalReleaseVersion`) opts in only at the exact current version:
/// `experiments_impl.dart` gates on `version == sdkLanguageVersion`, so
/// versions above the current one must not opt in either (audit F2 — a
/// `>=` gate would wrongly enable the flag at 3.16+).
#[test]
fn unreleased_flag_opts_in_only_at_current_version() {
    assert_eq!(
        Feature::InferenceUpdate4
            .meta()
            .experimental_release_version,
        None
    );
    let below_current = FeatureSet::for_language_version(lv(3, 14))
        .with_experiment_for_version(lv(3, 14), Feature::InferenceUpdate4);
    assert!(!below_current.is_enabled(Feature::InferenceUpdate4));
    let above_current = FeatureSet::for_language_version(lv(3, 16))
        .with_experiment_for_version(lv(3, 16), Feature::InferenceUpdate4);
    assert!(!above_current.is_enabled(Feature::InferenceUpdate4));
    let at_current = FeatureSet::for_language_version(DEFAULT_LANGUAGE_VERSION)
        .with_experiment_for_version(DEFAULT_LANGUAGE_VERSION, Feature::InferenceUpdate4);
    assert!(at_current.is_enabled(Feature::InferenceUpdate4));
}

/// Expired flags are dropped by the decode layer before any version check
/// (`experiments_impl.dart::_flagStringsToMap` skips `isExpired` entries), so
/// the flag must never enable the feature — `enabledIn`-derived enablement is
/// the only path.
#[test]
fn expired_flag_is_ignored_at_any_version() {
    // non-nullable: expired, `enabledIn: '2.12.0'`. Below the release the
    // flag must not force it on; at the release version the feature is on via
    // version-derived enablement, flag or no flag.
    assert!(Feature::NonNullable.is_expired());
    let below = FeatureSet::for_language_version(lv(2, 11))
        .with_experiment_for_version(lv(2, 11), Feature::NonNullable);
    assert!(!below.is_enabled(Feature::NonNullable));
    let at = FeatureSet::for_language_version(lv(2, 12))
        .with_experiment_for_version(lv(2, 12), Feature::NonNullable);
    assert!(at.is_enabled(Feature::NonNullable));
}

#[test]
fn triple_shift_gate_is_two_fourteen() {
    assert!(!triple_shift_enabled(lv(2, 13)));
    assert!(triple_shift_enabled(lv(2, 14)));
    assert!(triple_shift_enabled(lv(3, 0)));
    for minor in 0..=15 {
        let version = lv(3, minor);
        assert_eq!(
            triple_shift_enabled(version),
            FeatureSet::for_language_version(version).is_enabled(Feature::TripleShift),
            "mismatch at 3.{minor}"
        );
    }
}

#[test]
fn fingerprint_is_deterministic() {
    assert_eq!(
        FeatureSet::latest().fingerprint(),
        FeatureSet::latest().fingerprint()
    );
    assert_eq!(
        FeatureSet::for_language_version(lv(2, 12)).fingerprint(),
        FeatureSet::for_language_version(lv(2, 12)).fingerprint()
    );
}

#[test]
fn fingerprint_distinguishes_sets() {
    let latest = FeatureSet::latest();
    assert_ne!(
        latest.fingerprint(),
        latest.with_experiment(Feature::Variance).fingerprint()
    );
    assert_ne!(
        FeatureSet::for_language_version(lv(2, 12)).fingerprint(),
        latest.fingerprint()
    );
    // A pure function of the bit set: equal sets produce equal prints.
    assert_eq!(
        latest.fingerprint(),
        FeatureSet::for_language_version(lv(3, 15)).fingerprint()
    );
}

/// Golden fingerprints pinned to the current vendored yaml snapshot
/// (dart-lang/sdk `e6f4207adef69106d7519a79144efdb5c1acc4d0`,
/// `current-version: '3.15.0'`, vendored 2026-10-08).
///
/// Fingerprints derive from registry bit order (= yaml appearance order), so
/// re-vendoring the yaml can silently shift bits and change every
/// fingerprint — reserved as the future `FeatureSetId` key, that drift must
/// not be swallowed. These constants make it loud: when this test fails
/// after a re-vendor, the fingerprint values moved — first decide whether
/// the key needs an explicit versioning scheme (e.g. a `DATA_VERSION` baked
/// into the registry), only then update the constants with the new values.
#[test]
fn fingerprint_golden_values() {
    let latest = FeatureSet::latest().fingerprint();
    assert_eq!(latest, 0x8ee5_3921_de0a_b360);
    // Same bit set as `latest`, same print: DEFAULT_LANGUAGE_VERSION is 3.15.
    assert_eq!(
        FeatureSet::for_language_version(lv(3, 15)).fingerprint(),
        latest
    );
    assert_eq!(
        FeatureSet::for_language_version(lv(2, 12)).fingerprint(),
        0x3aca_cb83_56da_1694
    );
}

#[test]
fn expired_metadata_from_yaml() {
    assert!(Feature::DotShorthands.meta().expired);
    assert!(Feature::TripleShift.meta().expired);
    assert!(!Feature::Variance.meta().expired);
    assert!(!Feature::PrimaryConstructors.meta().expired);
    // The read-only query mirrors `meta().expired` for flag decoding.
    assert!(Feature::TripleShift.is_expired());
    assert!(!Feature::Variance.is_expired());
}

#[test]
fn enabled_in_metadata_from_yaml() {
    assert_eq!(Feature::Records.meta().enabled_in, Some(lv(3, 0)));
    assert_eq!(Feature::DotShorthands.meta().enabled_in, Some(lv(3, 10)));
    assert_eq!(Feature::TripleShift.meta().enabled_in, Some(lv(2, 14)));
    assert_eq!(Feature::Augmentations.meta().enabled_in, None);
}

#[test]
fn experimental_release_version_metadata() {
    assert_eq!(
        Feature::Macros.meta().experimental_release_version,
        Some(lv(3, 3))
    );
    assert_eq!(
        Feature::Augmentations.meta().experimental_release_version,
        Some(lv(3, 6))
    );
    assert_eq!(Feature::Variance.meta().experimental_release_version, None);
}

#[test]
fn help_is_first_sentence() {
    assert_eq!(
        Feature::DotShorthands.meta().help,
        "Shorter dot syntax for static accesses."
    );
    assert_eq!(Feature::TestExperiment.meta().help, "Has no effect.");
    assert_eq!(Feature::Variance.meta().help, "Sound variance");
}
