//! #2807 exact-row pins: the extension-type no-representation pair
//! (`expected_representation_field` + `missing_primary_constructor`).
//!
//! Every expected row is the 3.13.7 oracle face from the conformance report
//! (`target/conformance/report.tsv`), with the decision points mirrored from
//! the pinned SDK (upstream Dart SDK source, pkg/analyzer +
//! pkg/_fe_analyzer_shared):
//!
//! * `missing_primary_constructor` is the shared parser's, anchored on the
//!   last consumed token of the name/type-parameter chain and gated on
//!   `allowExtensionTypeRepresentation` — augmentations never carry it
//!   (parser_impl.dart:3831-3834).
//! * `expected_representation_field` is the error verifier's
//!   `_checkForExtensionTypeRepresentationErrorCodes` face. Its anchors:
//!   the empty parsed list takes the real `)`; the *synthesized* empty list
//!   (no constructor head, or a `.name` head with no formals) takes
//!   `insertParens`'s zero-length synthetic `()` — at the token after the
//!   type name for the headless shape (ast_builder.dart:1590-1597,
//!   token_stream_rewriter.dart:38-59), at the `.` for the named head
//!   (ast_builder.dart:2709-2712); initializing formals take the
//!   `this`/`super` keyword (error_verifier.dart:5384-5393); the
//!   function-typed suffix (feature off) takes the formal's begin token
//!   (:5433-5439).
//! * The verifier runs whenever the constructor head parsed — augmentations
//!   included (ast_builder.dart:6484-6496) — but never on a headless
//!   augmentation (its namePart stays a NameWithTypeParameters, so the
//!   check returns before the empty-list arm).

use super::collector::{latest_features, scan_ok};
use crate::ParserFeatures;
use dartq_language::{FeatureSet, LanguageVersion};

fn rows_with(source: &'static str, features: ParserFeatures) -> Vec<(String, u32, u32)> {
    let (table, text) = scan_ok(source);
    let mut collector = super::collector::Collector::default();
    let mut parser = crate::Parser::new(text, table, features, &mut collector);
    parser.parse_unit();
    parser.listener.errors.clone()
}

fn pre_feature_features() -> ParserFeatures {
    ParserFeatures::from_feature_set(FeatureSet::for_language_version(LanguageVersion::new(
        3, 10,
    )))
}

/// `extension type E {}` (oracle ExtensionTypeDeclarationParserTest
/// test_primaryConstructor_missing 2|14 + 2|18|0): the pair anchors on the
/// name and on the zero-length synthetic `(` slot at the body token.
#[test]
fn no_head_pair_anchors_on_name_and_synthetic_slot() {
    let rows = rows_with("extension type E {}\n", latest_features());
    assert!(rows.contains(&("missing_primary_constructor".into(), 15, 16)));
    assert!(rows.contains(&("expected_representation_field".into(), 17, 17)));
}

/// Behind type parameters the synthesized slot sits at the token after the
/// NAME — the `<` — not after the parameter list (oracle
/// parser-fe no_primary_constructor 4|19|0 vs 4|23), and
/// `missing_primary_constructor` anchors on the closing `>` (the parser
/// pointer at the head check, oracle 4|18|1).
#[test]
fn no_head_behind_type_parameters_anchors_before_them() {
    let rows = rows_with("extension type E<T> {}\n", latest_features());
    assert!(
        rows.contains(&("missing_primary_constructor".into(), 18, 19)),
        "mpc on the `>`: {rows:?}"
    );
    assert!(
        rows.contains(&("expected_representation_field".into(), 16, 16)),
        "erf on the `<`: {rows:?}"
    );
}

/// The `implements` follower is the synthesized slot's host token (oracle
/// no_primary_constructor 2|20|0): same column as the old token-width face,
/// zero length.
#[test]
fn no_head_before_implements_anchors_on_the_clause_keyword() {
    let rows = rows_with("extension type E implements Foo {}\n", latest_features());
    assert!(rows.contains(&("expected_representation_field".into(), 17, 17)));
}

/// A missing declaration name leaves the synthetic name's follower as the
/// slot: the `<` of the type parameters (oracle Extension-methods
/// syntax_t06 20|15|0), with `missing_primary_constructor` on the `>`
/// (oracle 20|17|1).
#[test]
fn no_head_with_missing_name_anchors_on_the_type_parameter_opener() {
    let rows = rows_with("extension type<T> on C<T> {}\n", latest_features());
    assert!(rows.contains(&("missing_primary_constructor".into(), 16, 17)));
    assert!(rows.contains(&("expected_representation_field".into(), 14, 14)));
}

/// `extension type E.n {}`: the `.name` head synthesized its empty list at
/// the `.` (oracle extension_type_primary_constructor_error 13|20|0 for
/// `E<T>.named`), and the parser's `missing_primary_constructor_parameters`
/// stays on the constructor name (oracle 13|21|5).
#[test]
fn named_head_without_formals_anchors_on_the_period() {
    let rows = rows_with("extension type E.n {}\n", latest_features());
    assert!(rows.contains(&("missing_primary_constructor_parameters".into(), 17, 18)));
    assert!(rows.contains(&("expected_representation_field".into(), 16, 16)));
    assert!(
        !rows
            .iter()
            .any(|(c, ..)| c == "missing_primary_constructor")
    );
}

/// `extension type E.() {}`: a PARSED empty list anchors on the real `)`
/// with the token's width (oracle extension_type_primary_constructor_error
/// 4|19|1).
#[test]
fn named_head_with_empty_formals_anchors_on_the_real_close_paren() {
    let rows = rows_with("extension type E.() {}\n", latest_features());
    assert!(rows.contains(&("expected_representation_field".into(), 18, 19)));
}

/// Initializing formals anchor on the keyword itself, typed or bare
/// (oracle regress_53625 53|24|4 `this` behind `int`, 57|20|4 bare;
/// 62|24|5 `super`).
#[test]
fn initializing_formals_anchor_on_the_keyword() {
    let rows = rows_with("extension type E(int this.x) {}\n", latest_features());
    assert!(
        rows.contains(&("expected_representation_field".into(), 21, 25)),
        "erf on `this` behind the type: {rows:?}"
    );
    let rows = rows_with("extension type E(this.x) {}\n", latest_features());
    assert!(
        rows.contains(&("expected_representation_field".into(), 17, 21)),
        "erf on bare `this`: {rows:?}"
    );
    let rows = rows_with("extension type E(super.x) {}\n", latest_features());
    assert!(
        rows.contains(&("expected_representation_field".into(), 17, 22)),
        "erf on bare `super`: {rows:?}"
    );
}

/// The function-typed suffix (feature off) anchors on the formal's FIRST
/// token — the modifier — never the list's `(` (oracle
/// declaring_parameter_pre_feature 41|19|3 `var`, 42|19|5 `final`).
#[test]
fn function_typed_formal_anchors_on_the_formal_begin() {
    let rows = rows_with("extension type E(var f()) {}\n", pre_feature_features());
    assert!(
        rows.contains(&("expected_representation_field".into(), 17, 20)),
        "erf on `var`: {rows:?}"
    );
    let rows = rows_with("extension type E(final f()) {}\n", pre_feature_features());
    assert!(
        rows.contains(&("expected_representation_field".into(), 17, 22)),
        "erf on `final`: {rows:?}"
    );
}

/// A headless augmentation carries NEITHER face: the parser's report is
/// `allowExtensionTypeRepresentation`-gated and the verifier's synthesized
/// empty list is intro-only (oracle: every `augment extension type E {}`
/// row set across the co19 Augmentations suite is silent on both codes).
#[test]
fn headless_augmentation_reports_neither_face() {
    let rows = rows_with("augment extension type E {}\n", latest_features());
    assert!(
        !rows
            .iter()
            .any(|(c, ..)| c == "missing_primary_constructor"
                || c == "expected_representation_field")
    );
}

/// A parsed `.name` head on an augmentation keeps the verifier face — the
/// constructor parsed, so the shape checks run (oracle Augmentations
/// extension_types_A01_t02 19|27|0) — while the parser's
/// `missing_primary_constructor_parameters` stays intro-only.
#[test]
fn augmentation_named_head_keeps_the_verifier_face() {
    let rows = rows_with("augment extension type E.n {}\n", latest_features());
    assert!(rows.contains(&("expected_representation_field".into(), 24, 24)));
    assert!(
        !rows
            .iter()
            .any(|(c, ..)| c == "missing_primary_constructor_parameters")
    );
}

/// A parsed formal list on an augmentation keeps the verifier face too: the
/// initializing-formal anchor fires ungated (the AST builder keeps the
/// PrimaryConstructorDeclaration, ast_builder.dart:6484-6496).
#[test]
fn augmentation_formals_keep_the_verifier_face() {
    let rows = rows_with("augment extension type E(this.x) {}\n", latest_features());
    assert!(rows.contains(&("expected_representation_field".into(), 25, 29)));
}
