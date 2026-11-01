//! #3317 — the `missingFunctionBody` parse-tier stance.
//!
//! Upstream `allowAbstract` for a `;`-terminated member body is the
//! augmentations *feature flag*, not the presence of an `augment`/`external`
//! modifier on the same declaration:
//!
//! * class member — `parser_impl.dart:6029-6035`
//!   `(staticToken == null || externalToken != null ||
//!   isAugmentationsFeatureEnabled) && inPlainSync`
//! * top-level method — `parser_impl.dart:4606-4610`
//!   `isExternal || isAugmentationsFeatureEnabled`
//!
//! With the feature off, the parser reports `missing_function_body` on the
//! `;` (oracle `ExecutableBodyTest/test_class_getter_static_noBody` and its
//! `_language305` variant: one row at the semicolon). With the feature on the
//! parse tier is silent and the verdict is the analyzer's post-parse
//! augmentation walk, which picks `missingFunctionBody` or
//! `functionNotCompleteAfterAugmentations` — not decidable per unit, because
//! the `augment` may live in another part file. Emitting from the parse tier
//! there fires on every declaration a later `augment` completes (oracle
//! `ExecutableBodyTest/test_class_getter_static_augmentation_hasBody`: no
//! rows at all).

use super::collector::{Collector, scan_ok};

/// Parses `source` with the latest language version and the augmentations
/// feature flag forced to `augmentations`.
fn parse(source: &'static str, augmentations: bool) -> Collector {
    let (table, text) = scan_ok(source);
    let mut features = crate::ParserFeatures::from_feature_set(
        dartq_language::FeatureSet::for_language_version(
            dartq_language::LanguageVersion::new(3, 15),
        ),
    );
    features.augmentations_enabled = augmentations;
    let mut parser = crate::parser::Parser::new(
        text,
        table,
        features,
        Box::leak(Box::new(Collector::default())),
    );
    parser.parse_unit();
    std::mem::take(parser.listener)
}

fn rows(source: &'static str, augmentations: bool) -> Vec<(String, u32, u32)> {
    parse(source, augmentations)
        .errors
        .into_iter()
        .map(|(code, start, end)| (code, start, end - start))
        .collect()
}

/// `class A { static int get foo; }` — feature off reports on the `;`.
#[test]
fn static_getter_semicolon_body_reports_without_augmentations() {
    // Oracle ExecutableBodyTest/test_class_getter_static_noBody_language305:
    // one `missing_function_body` row at the semicolon (`4:21|1`).
    let source = "class A {\n  static int get foo;\n}\n";
    assert_eq!(
        rows(source, false),
        vec![("missing_function_body".to_string(), 30, 1)],
        "the `;` arm must report at parse time when the feature is off"
    );
}

/// The same member under the augmentations feature: the parse tier is silent.
#[test]
fn static_getter_semicolon_body_silent_with_augmentations() {
    let source = "class A {\n  static int get foo;\n}\n";
    assert_eq!(
        rows(source, true),
        Vec::new(),
        "the completeness verdict belongs to the post-parse augmentation walk"
    );
}

/// The corpus shape the fix is measured on: a static member the following
/// `augment` completes. Neither stance may report at the parse tier under the
/// feature (oracle `test_class_getter_static_augmentation_hasBody`: silent).
#[test]
fn augmentation_completed_static_member_silent() {
    let source = "class A {\n  static int get foo;\n  augment static int get foo => 0;\n}\n";
    assert_eq!(rows(source, true), Vec::new());
}

/// Instance members stay abstract-legal at the parse tier in both stances
/// (their misuse is the verifier's `concrete_class_with_abstract_member`).
#[test]
fn instance_semicolon_body_silent_in_both_stances() {
    let source = "class A {\n  int get foo;\n}\n";
    assert_eq!(rows(source, false), Vec::new());
    assert_eq!(rows(source, true), Vec::new());
}

/// `external` is the one modifier that keeps a `;` body legal on its own
/// (upstream's `externalToken != null` disjunct), with the feature off.
#[test]
fn external_semicolon_body_silent_without_augmentations() {
    let source = "class A {\n  external static int get foo;\n}\n";
    assert_eq!(rows(source, false), Vec::new());
    assert_eq!(rows(source, true), Vec::new());
}

/// Top-level methods follow the same fold
/// (`isExternal || isAugmentationsFeatureEnabled`): report with the feature
/// off, silent with it on.
#[test]
fn top_level_semicolon_body_follows_the_feature() {
    let source = "void f();\n";
    assert_eq!(
        rows(source, false),
        vec![("missing_function_body".to_string(), 8, 1)]
    );
    assert_eq!(rows(source, true), Vec::new());
}

/// `async`/`sync*` bodies never take the abstract arm
/// (`allowAbstract && inPlainSync`): the `;` still reports with the feature
/// off.
#[test]
fn async_semicolon_body_keeps_the_in_plain_sync_gate() {
    let source = "void f() async;\n";
    assert_eq!(
        rows(source, false),
        vec![("missing_function_body".to_string(), 14, 1)]
    );
}
