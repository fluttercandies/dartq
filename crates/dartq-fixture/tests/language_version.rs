//! The language-version pins: the fixture-level `//- language:` marker and
//! the per-file `// @dart=` override both reach
//! `FeatureSet::for_language_version`, so a version-gated rule registers
//! and parses exactly as it does under the production pipeline.
//!
//! `invalid_case_patterns` is the probe rule: it registers only with
//! `patterns` off (the upstream registration bail) and only then sees the
//! legacy `case` head the rule reports.

use dartq_fixture::{FixtureDoc, parse, run_case, version_of};
use dartq_language::{DEFAULT_LANGUAGE_VERSION, FeatureSet, LanguageVersion};
use dartq_lints::{Features, rules};

/// The legacy case head the rule reports (`case 1 + 2:` is a plain
/// expression before 3.0, an invalid constant pattern after).
const LEGACY_HEAD: &str = "void f(Object o) {\n  switch (o) {\n    case 1 + 2:\n  }\n}\n";

fn doc(text: &str) -> FixtureDoc {
    parse(text).expect("fixture parses")
}

/// The emitted rows' rule names for the doc's first case.
fn fired(doc: &FixtureDoc) -> Vec<&'static str> {
    run_case(&doc.cases[0], &doc.lint, doc.language)
        .into_iter()
        .map(|lint| rules()[lint.rule.0 as usize].name)
        .collect()
}

fn count(doc: &FixtureDoc, rule: &str) -> usize {
    fired(doc).iter().filter(|name| **name == rule).count()
}

#[test]
fn fixture_language_219_keeps_the_legacy_case_head() {
    let fixture = doc(&format!(
        "//- lint: invalid_case_patterns\n//- language: 2.19\n//- case: legacy\n{LEGACY_HEAD}"
    ));
    assert_eq!(fixture.language, Some(LanguageVersion::new(2, 19)));
    let version = version_of(&fixture.cases[0], fixture.language);
    assert_eq!(version, LanguageVersion::new(2, 19));
    let features = Features::from_feature_set(FeatureSet::for_language_version(version));
    assert!(!features.patterns, "2.19 predates patterns");
    assert_eq!(count(&fixture, "invalid_case_patterns"), 1);
}

#[test]
fn fixture_language_30_parses_patterns_and_silences_the_rule() {
    let fixture = doc(&format!(
        "//- lint: invalid_case_patterns\n//- language: 3.0\n//- case: patterns\n{LEGACY_HEAD}"
    ));
    let version = version_of(&fixture.cases[0], fixture.language);
    assert_eq!(version, LanguageVersion::new(3, 0));
    let features = Features::from_feature_set(FeatureSet::for_language_version(version));
    assert!(features.patterns, "3.0 ships patterns");
    assert_eq!(count(&fixture, "invalid_case_patterns"), 0);
}

#[test]
fn per_file_override_wins_over_the_fixture_language() {
    let fixture = doc(&format!(
        "//- lint: invalid_case_patterns\n//- language: 3.0\n//- case: overridden\n// @dart=2.19\n{LEGACY_HEAD}"
    ));
    assert_eq!(
        fixture.language,
        Some(LanguageVersion::new(3, 0)),
        "the fixture-level pin stays in the data"
    );
    assert_eq!(
        version_of(&fixture.cases[0], fixture.language),
        LanguageVersion::new(2, 19),
        "the file's own `@dart` override wins"
    );
    assert_eq!(count(&fixture, "invalid_case_patterns"), 1);
}

#[test]
fn an_unpinned_fixture_keeps_the_default_version() {
    let fixture = doc(&format!(
        "//- lint: invalid_case_patterns\n//- case: default\n{LEGACY_HEAD}"
    ));
    assert_eq!(fixture.language, None);
    assert_eq!(
        version_of(&fixture.cases[0], fixture.language),
        DEFAULT_LANGUAGE_VERSION
    );
    assert_eq!(count(&fixture, "invalid_case_patterns"), 0);
    // The one version the runner now always folds must reproduce the bridge
    // default the unpinned fixtures always ran under: a registry bump that
    // breaks this identity fails here, not silently across the corpus.
    assert_eq!(
        Features::from_feature_set(FeatureSet::for_language_version(DEFAULT_LANGUAGE_VERSION)),
        Features::default()
    );
}

#[test]
fn the_language_header_is_a_whole_file_pin() {
    assert_eq!(
        parse("//- lint: a\n//- case: c\n//- language: 2.19\nint x;\n")
            .unwrap_err()
            .line,
        3,
        "a language header after the first case is a parse error"
    );
    assert_eq!(
        parse("//- lint: a\n//- language: 2.19\n//- language: 3.0\n//- case: c\nint x;\n")
            .unwrap_err()
            .line,
        3
    );
    assert_eq!(
        parse("//- lint: a\n//- language: two\n//- case: c\nint x;\n")
            .unwrap_err()
            .line,
        2
    );
}
