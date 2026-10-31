//! #3200 pins: the parser-side half of the body-less factory hand-up.
//!
//! Upstream splits this face in two, and only the first half is parseable:
//!
//! - `parseFactoryMethod` (parser_impl.dart:6141) passes `allowAbstract:
//!   isAugmentationsFeatureEnabled` into `parseFunctionBody` (:6232-6235), so
//!   with the augmentations feature on a `;` body is parse-legal and the
//!   parser reports nothing at all;
//! - `_checkForFactoryBodyCompleteness` (error_verifier.dart:5078-5107) then
//!   reports `factory_without_body` on the introductory declaration's
//!   `errorRange` (ast.dart:7065-7070: `typeName` — else the `factory`
//!   keyword — through `name`) when no fragment of the constructor is
//!   complete, and `factory_not_complete_after_augmentations` (:5103) when
//!   the augmentation chain has more than one fragment.
//!
//! The verifier half needs the chain, not the unit: the same body-less
//! declaration is silent when a sibling adds `augment factory named() =>
//! throw 0;` and takes the other code when it adds `augment factory named();`
//! — and augmentations may live in another part file. A per-unit parse cannot
//! decide that, so the parser lane owns only the suppression pinned here; the
//! emit stays with the verifier lane (`SemanticCode::FactoryWithoutBody`).
//! Emitting from the parser would add a `factory_without_body` row to every
//! augmentation-completed and every two-fragment constructor.

use super::collector::{latest_features, scan_ok};
use dartq_language::{Feature, FeatureSet};

struct Rows {
    rows: Vec<(String, u32, u32)>,
}

fn parse_with(source: &'static str, features: crate::ParserFeatures) -> Rows {
    let (table, text) = scan_ok(source);
    let mut collector = super::collector::Collector::default();
    let mut parser = crate::Parser::new(text, table, features, &mut collector);
    parser.parse_unit();
    let mut rows = parser.listener.errors.clone();
    rows.sort();
    Rows { rows }
}

fn parse(source: &'static str) -> Rows {
    parse_with(source, latest_features())
}

/// The augmentations feature on at the current language version: the shipped
/// fold never enables it (no `enabledIn`), only the experiment opt-in does —
/// exactly the pipeline's `with_experiment_for_version` face.
fn augmentations_features() -> crate::ParserFeatures {
    crate::ParserFeatures::from_feature_set(
        FeatureSet::latest().with_experiment(Feature::Augmentations),
    )
}

fn parse_augmentations(source: &'static str) -> Rows {
    parse_with(source, augmentations_features())
}

/// Spans of every `missing_function_body` row (`msg::expected_body` — the
/// upstream `missingFunctionBody` template), in source order.
fn missing_body_spans(parsed: &Rows) -> Vec<(u32, u32)> {
    parsed
        .rows
        .iter()
        .filter(|(code, _, _)| code == "missing_function_body")
        .map(|(_, start, end)| (*start, *end))
        .collect()
}

fn has_code(parsed: &Rows, code: &str) -> bool {
    parsed.rows.iter().any(|(c, _, _)| c == code)
}

/// The four body-less factory shapes of the corpus
/// (`constructor_body/ConstructorBodyTest/*_noBody`): with the feature on the
/// parser is silent — no `missing_function_body` (`;` arm) and no
/// `factory_without_body` (not a parser code).
#[test]
fn ticket_3200_augmentations_suppress_the_row() {
    for source in [
        "class C { factory C.named(); }\n",
        "class C { factory named(); }\n",
        "class C { factory (); }\n",
        "class C { const factory C(); }\n",
    ] {
        let parsed = parse_augmentations(source);
        assert!(
            missing_body_spans(&parsed).is_empty(),
            "the `;` arm must stay silent under augmentations: {source:?} -> {:?}",
            parsed.rows
        );
        assert!(
            !has_code(&parsed, "factory_without_body"),
            "the emit is verifier-tier, never a parser row: {source:?} -> {:?}",
            parsed.rows
        );
    }
}

/// With the feature off the same shapes keep the `;`-anchored
/// `missing_function_body` (upstream `allowAbstract: false`), one row at the
/// semicolon — never at the constructor head.
#[test]
fn ticket_3200_default_flag_keeps_the_semicolon_row() {
    let cases = [
        ("class C { factory C.named(); }\n", (27, 28)),
        ("class C { factory named(); }\n", (25, 26)),
        ("class C { factory (); }\n", (20, 21)),
        ("class C { const factory C(); }\n", (27, 28)),
    ];
    for (source, semi) in cases {
        let parsed = parse(source);
        assert_eq!(
            missing_body_spans(&parsed),
            vec![semi],
            "the `;` arm keeps its anchor with the feature off: {source:?} -> {:?}",
            parsed.rows
        );
        assert!(
            !has_code(&parsed, "factory_without_body"),
            "the feature-off lane never reports the augmentations code: {source:?} -> {:?}",
            parsed.rows
        );
    }
}

/// `external` and redirecting factories are complete declarations upstream
/// (`error_verifier.dart`'s fragment rule: external/no-redirection both count
/// as a body), so neither flag reports a body row.
#[test]
fn ticket_3200_external_and_redirecting_stay_silent() {
    for source in [
        "class C { external factory C(); }\n",
        "class C { factory C() = D; }\n",
    ] {
        for features in [latest_features(), augmentations_features()] {
            let parsed = parse_with(source, features);
            assert!(
                missing_body_spans(&parsed).is_empty(),
                "complete factories never report a body: {source:?} -> {:?}",
                parsed.rows
            );
        }
    }
}

/// The augmentation-chain split the parser deliberately cannot take: both
/// sources are parse-silent, while the oracle reports nothing for the first
/// and `factory_not_complete_after_augmentations` for the second. Pinned so a
/// later lane cannot "fix" the missing emit here without breaking them.
#[test]
fn ticket_3200_parse_cannot_decide_the_chain() {
    for source in [
        "class A { factory named(); augment factory named() => throw 0; }\n",
        "class A { factory named(); augment factory named(); }\n",
    ] {
        let parsed = parse_augmentations(source);
        assert!(
            missing_body_spans(&parsed).is_empty(),
            "both chain shapes are parse-silent: {source:?} -> {:?}",
            parsed.rows
        );
    }
}

/// The static-member arm of the class-body dispatch is a different gate
/// (`static_kw.is_none()` at the body dispatch, members.rs): the augmentations
/// feature must not silence it.
#[test]
fn ticket_3200_static_member_arm_is_not_gated() {
    let parsed = parse_augmentations("class C { static void m(); }\n");
    assert_eq!(
        missing_body_spans(&parsed),
        vec![(25, 26)],
        "the static `;` arm keeps reporting: {:?}",
        parsed.rows
    );
}

/// A body-less generative constructor is abstract-legal at this tier (the
/// verifier owns `concrete_class_with_abstract_member`), so the new gate must
/// not leak a body row onto it under either flag.
#[test]
fn ticket_3200_bodyless_generative_constructor_is_not_leaked() {
    for features in [latest_features(), augmentations_features()] {
        let parsed = parse_with("class C { C(); }\n", features);
        assert!(
            missing_body_spans(&parsed).is_empty(),
            "generative `;` bodies stay abstract-legal: {:?}",
            parsed.rows
        );
        assert!(
            !has_code(&parsed, "factory_without_body"),
            "the factory code never fires on a generative constructor: {:?}",
            parsed.rows
        );
    }
}
