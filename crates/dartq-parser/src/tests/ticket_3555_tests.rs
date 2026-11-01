//! #3555 pins: the four `augmentations`-gated member diagnostics.
//!
//! Upstream suppresses all four under the augmentations experiment (the
//! gates are exactly `!isAugmentationsFeatureEnabled`-conditioned); dartq's
//! parser emitted them unconditionally, so every enabled corpus row was a
//! pure EXTRA. The gates, in upstream order:
//!
//! - `extension_declares_abstract_member` — the whole verifier check is
//!   skipped under the feature (upstream Dart SDK source, pkg/analyzer/lib/
//!   src/generated/error_verifier.dart
//!   `_checkForExtensionDeclaresAbstractMember`: `if
//!   (_featureSet.isEnabled(Feature.augmentations)) return;`); the parser
//!   side carries the code.
//! - `abstract_static_field` — `_endClassFields` reports only when
//!   `staticToken != null && !enableAugmentations` (upstream Dart SDK
//!   source, pkg/analyzer/lib/src/fasta/ast_builder.dart); the sibling
//!   `abstractLateField` arm is ungated.
//! - top-level `abstract` extraneous_modifier — the fields path of
//!   `parseTopLevelMemberImpl` reports only when
//!   `!isAugmentationsFeatureEnabled` (upstream Dart SDK source,
//!   pkg/_fe_analyzer_shared/lib/src/parser/parser_impl.dart:4372-4374);
//!   the method path reports unconditionally (:4354-4355), and the
//!   early-return recoveries (pattern variables, invalid heads) never
//!   reach either arm.
//! - `abstract_extension_field` — a parser pseudo-code the analyzer's
//!   error converter drops unconditionally (upstream Dart SDK source,
//!   pkg/analyzer/lib/src/fasta/error_converter.dart); dartq keeps it as
//!   its own code (the gate_local_pins tripwire), so the drop is mirrored
//!   as the same `!augmentations` gate.

use super::collector::{latest_features, scan_ok};
use dartq_language::{Feature, FeatureSet};

fn rows_of(source: &'static str, features: crate::ParserFeatures) -> Vec<(String, u32, u32)> {
    let (table, text) = scan_ok(source);
    let mut collector = super::collector::Collector::default();
    let mut parser = crate::Parser::new(text, table, features, &mut collector);
    parser.parse_unit();
    let mut rows = parser.listener.errors.clone();
    rows.sort();
    rows
}

/// The opt-in face (no `enabledIn`): `latest` + the experiment — the same
/// construction `ticket_3200_tests` pins for the `allowAbstract` arms.
fn augmentations_features() -> crate::ParserFeatures {
    crate::ParserFeatures::from_feature_set(
        FeatureSet::latest().with_experiment(Feature::Augmentations),
    )
}

fn codes(source: &'static str, features: crate::ParserFeatures) -> Vec<String> {
    rows_of(source, features)
        .into_iter()
        .map(|(c, _, _)| c)
        .collect()
}

// —— extension_declares_abstract_member ————————————————————————————————

/// `extension E { int foo(); }`: a `;`-bodied non-static non-external
/// member reports the code with the feature off and stays silent with it
/// on (upstream skips the whole verifier under the experiment).
#[test]
fn extension_declares_abstract_member_gated() {
    const SRC: &str = "extension E on int { int foo(); }\n";
    assert!(
        codes(SRC, latest_features()).contains(&"extension_declares_abstract_member".to_owned()),
        "feature off must report: {:?}",
        codes(SRC, latest_features())
    );
    assert!(
        !codes(SRC, augmentations_features())
            .contains(&"extension_declares_abstract_member".to_owned()),
        "feature on must stay silent: {:?}",
        codes(SRC, augmentations_features())
    );
}

/// `augment` syntax inside the extension is what an enabled corpus looks
/// like — same silence (upstream's gate consults only the feature set).
#[test]
fn extension_declares_abstract_member_augment_member_silent() {
    const SRC: &str = "extension E on int { augment int foo(); }\n";
    assert!(
        !codes(SRC, augmentations_features())
            .contains(&"extension_declares_abstract_member".to_owned()),
        "augment member under the feature must stay silent: {:?}",
        codes(SRC, augmentations_features())
    );
}

/// Static and external extension members never took the code — the gate
/// must not disturb their silence on either flag.
#[test]
fn extension_declares_abstract_member_unaffected_arms() {
    for src in [
        "extension E on int { static int foo(); }\n",
        "extension E on int { external int foo(); }\n",
    ] {
        for features in [latest_features(), augmentations_features()] {
            assert!(
                !codes(src, features).contains(&"extension_declares_abstract_member".to_owned()),
                "{src:?} stays silent either way: {:?}",
                codes(src, features)
            );
        }
    }
}

// —— abstract_static_field —————————————————————————————————————————————

/// `class C { abstract static int x; }`: reports with the feature off
/// (upstream `_endClassFields`), silent with it on.
#[test]
fn abstract_static_field_gated() {
    const SRC: &str = "class C { abstract static int x; }\n";
    assert!(
        codes(SRC, latest_features()).contains(&"abstract_static_field".to_owned()),
        "feature off must report: {:?}",
        codes(SRC, latest_features())
    );
    assert!(
        !codes(SRC, augmentations_features()).contains(&"abstract_static_field".to_owned()),
        "feature on must stay silent: {:?}",
        codes(SRC, augmentations_features())
    );
}

/// The sibling `abstract_late_field` arm is NOT gated upstream
/// (`lateToken != null` fires unconditionally) — it must keep reporting
/// under the experiment.
#[test]
fn abstract_late_field_is_not_gated() {
    const SRC: &str = "class C { abstract late int x; }\n";
    for features in [latest_features(), augmentations_features()] {
        assert!(
            codes(SRC, features).contains(&"abstract_late_field".to_owned()),
            "abstract_late_field stays ungated: {:?}",
            codes(SRC, features)
        );
    }
}

// —— top-level abstract extraneous_modifier ————————————————————————————

/// `abstract int x;` at top level (the fields path): extraneous_modifier
/// with the feature off, silent with it on (upstream
/// `parseTopLevelMemberImpl` :4372).
#[test]
fn top_level_abstract_field_extraneous_gated() {
    const SRC: &str = "abstract int x;\n";
    let off = codes(SRC, latest_features());
    assert!(
        off.iter()
            .any(|c| c == "extraneous_modifier"),
        "feature off must report extraneous_modifier: {off:?}"
    );
    let on = codes(SRC, augmentations_features());
    assert!(
        !on.iter().any(|c| c == "extraneous_modifier"),
        "feature on must stay silent: {on:?}"
    );
}

/// `abstract int f() {}` at top level (the method path): upstream's arm
/// is unconditional (:4354), so the row keeps firing under the
/// experiment.
#[test]
fn top_level_abstract_method_extraneous_ungated() {
    const SRC: &str = "abstract int f() {}\n";
    for features in [latest_features(), augmentations_features()] {
        let got = codes(SRC, features);
        assert!(
            got.iter().any(|c| c == "extraneous_modifier"),
            "the method path reports extraneous_modifier on either flag: {got:?}"
        );
    }
}

/// The early-return recoveries never reach the dispatch arms upstream:
/// `abstract var (a, b) = (1, 2);` routes straight to `parseFields` from
/// the pattern check, so no extraneous_modifier fires on either flag
/// (the pattern row itself is out of scope here).
#[test]
fn top_level_abstract_pattern_recovery_stays_silent() {
    const SRC: &str = "abstract var (a, b) = (1, 2);\n";
    for features in [latest_features(), augmentations_features()] {
        let got = codes(SRC, features);
        assert!(
            !got.iter().any(|c| c == "extraneous_modifier"),
            "the pattern early-return never reports extraneous abstract: {got:?}"
        );
    }
}

// —— abstract_extension_field ——————————————————————————————————————————

/// `extension E { abstract int x; }`: dartq's own pseudo-code fires with
/// the feature off (the gate_local_pins tripwire) and drops under the
/// feature, mirroring the converter's drop on enabled corpora.
#[test]
fn abstract_extension_field_gated() {
    const SRC: &str = "extension E on int { abstract int x; }\n";
    assert!(
        codes(SRC, latest_features()).contains(&"abstract_extension_field".to_owned()),
        "feature off must report: {:?}",
        codes(SRC, latest_features())
    );
    assert!(
        !codes(SRC, augmentations_features()).contains(&"abstract_extension_field".to_owned()),
        "feature on must stay silent: {:?}",
        codes(SRC, augmentations_features())
    );
}

/// `augment abstract int x;` inside the extension (the verified oracle
/// shape — silent upstream under the experiment): only the paired
/// `extension_declares_instance_field` survives, never the abstract code.
#[test]
fn abstract_extension_field_augment_member_silent() {
    const SRC: &str = "extension E on int { augment abstract int x; }\n";
    let got = codes(SRC, augmentations_features());
    assert!(
        !got.iter().any(|c| c == "abstract_extension_field"),
        "the augment member stays silent: {got:?}"
    );
}
