//! #1600 primary-constructor declaring-parameter pins (burn case 4ebca937)
//! plus the pre-feature `const` gate rider (burn case 891ff7af). Every
//! expected row is the probed 3.13.1 oracle face at the corpus default
//! language version (`^3.13.0`, no experiment flag) on
//! `primary_constructors/initializing_declaring.dart` and, for the rider,
//! `primary_constructors/invalid_primary_constructors_pre_feature.dart`
//! (`// @dart=3.10`). Decision points mirror the upstream parser (upstream
//! Dart SDK source, pkg/_fe_analyzer_shared, parser_impl.dart:2212-2226 for
//! the declaring faces, :3798-3823 and :3884-3905 for the `const` gate;
//! message templates pkg/front_end messages.yaml
//! `InitializingDeclaringParameter` / `SuperInitializingDeclaringParameter`).
//!
//! The parser owns exactly one row per declaring leg: the declaring code at
//! the `this`/`super` keyword of a `var`/`final` formal, or the modifier
//! walk's `extraneous_modifier` at `const`. The oracle's other rows on the
//! same lines (`initializing_formal_for_non_existent_field`,
//! `super_formal_parameter_without_associated_positional`,
//! `duplicate_definition`) are resolver-side and never reach this crate.

use super::collector::{Collector, latest_features, scan_ok};
use crate::ParserFeatures;
use dartq_language::{FeatureSet, LanguageVersion};

/// Recoverable-error rows `(code, start, end)` in emission order plus their
/// parallel problem texts, for `source` under `features`.
fn rows_with(
    source: &'static str,
    features: ParserFeatures,
) -> (Vec<(String, u32, u32)>, Vec<String>) {
    let (table, text) = scan_ok(source);
    let mut collector = Collector::default();
    let mut parser = crate::Parser::new(text, table, features, &mut collector);
    parser.parse_unit();
    (
        parser.listener.errors.clone(),
        parser.listener.texts.clone(),
    )
}

fn rows(source: &'static str) -> Vec<(String, u32, u32)> {
    rows_with(source, latest_features()).0
}

fn row(code: &str, start: u32, end: u32) -> (String, u32, u32) {
    (code.to_owned(), start, end)
}

/// 3.10 feature set: primary constructors off (`enabledIn: 3.13.0`), the
/// `// @dart=3.10` corpus posture.
fn pre_feature_features() -> ParserFeatures {
    ParserFeatures::from_feature_set(FeatureSet::for_language_version(LanguageVersion::new(
        3, 10,
    )))
}

// —— L1/L2: `var`|`final` this.x → initializing_declaring_parameter ————————

/// Oracle initializing_declaring.dart 1|13 (`var`) and 2|15 (`final`): one
/// syntactic row each, anchored on the `this` keyword (length 4).
#[test]
fn var_final_this_formal_reports_declaring_code_at_this() {
    assert_eq!(
        rows("class C(var this.str);\n"),
        [row("initializing_declaring_parameter", 12, 16)],
    );
    assert_eq!(
        rows("class C(final this.str);\n"),
        [row("initializing_declaring_parameter", 14, 18)],
    );
}

// —— L3: `const` this.x → extraneous_modifier only ———————————————————————————

/// Oracle 3|9: `extraneous_modifier` at `const` (length 5) and nothing
/// else parser-side — the declaring gate is `varFinalOrConst != null &&
/// !isA(CONST)` (parser_impl.dart:2213), so a const formal never carries a
/// declaring code.
#[test]
fn const_this_formal_keeps_extraneous_modifier_without_declaring_code() {
    assert_eq!(
        rows("class C(const this.str);\n"),
        [row("extraneous_modifier", 8, 13)],
    );
}

// —— L4/L5: `var`|`final` super.x → super_initializing_declaring_parameter —

/// Oracle 4|13 (`var`) and 5|15 (`final`): anchored on the `super` keyword
/// (length 5).
#[test]
fn var_final_super_formal_reports_declaring_code_at_super() {
    assert_eq!(
        rows("class C(var super.str);\n"),
        [row("super_initializing_declaring_parameter", 12, 17)],
    );
    assert_eq!(
        rows("class C(final super.str);\n"),
        [row("super_initializing_declaring_parameter", 14, 19)],
    );
}

// —— L6: `const` super.x → extraneous_modifier only ——————————————————————————

/// Oracle 6|9: the same const posture as L3 on a super formal.
#[test]
fn const_super_formal_keeps_extraneous_modifier_without_declaring_code() {
    assert_eq!(
        rows("class C(const super.str);\n"),
        [row("extraneous_modifier", 8, 13)],
    );
}

/// The whole probed fixture in one unit: six rows in source order, one per
/// line on that line's keyword — the parser half of the oracle's row set,
/// with the upstream message templates verbatim.
#[test]
fn initializing_declaring_fixture_parser_rows() {
    let (rows, texts) = rows_with(
        concat!(
            "class C(var this.str);\n",
            "class C(final this.str);\n",
            "class C(const this.str);\n",
            "class C(var super.str);\n",
            "class C(final super.str);\n",
            "class C(const super.str);\n",
        ),
        latest_features(),
    );
    assert_eq!(
        rows,
        [
            row("initializing_declaring_parameter", 12, 16),
            row("initializing_declaring_parameter", 37, 41),
            row("extraneous_modifier", 56, 61),
            row("super_initializing_declaring_parameter", 85, 90),
            row("super_initializing_declaring_parameter", 111, 116),
            row("extraneous_modifier", 131, 136),
        ],
    );
    assert_eq!(texts[0], "Declaring parameters can't be initializing.");
    assert_eq!(texts[2], "Can't have modifier 'const' here.");
    assert_eq!(texts[3], "Declaring parameters can't be super parameters.");
}

/// The declaring faces are a primary-constructor posture only: the same
/// modifier + receiver shapes in an ordinary constructor's formals stay
/// silent on the declaring codes (parser_impl.dart:2212 keys on
/// `MemberKind.PrimaryConstructor`).
#[test]
fn ordinary_constructor_formals_never_report_declaring_codes() {
    for source in [
        "class C {\n  C(var this.x);\n}\n",
        "class C {\n  C(final this.x);\n}\n",
        "class C {\n  C(var super.x);\n}\n",
        "class C {\n  C(final super.x);\n}\n",
    ] {
        let got = rows(source);
        assert!(
            !got.iter()
                .any(|(code, _, _)| code.contains("declaring_parameter")),
            "{source:?}: declaring code outside a primary constructor: {got:?}"
        );
    }
}

// —— Rider: `const` without a primary constructor at a pre-feature pin ———

/// Oracle invalid_primary_constructors_pre_feature.dart (`// @dart=3.10`)
/// carries zero `const_without_primary_constructor` rows: with the feature
/// off every per-kind `const` face collapses into `unexpected_token`
/// (parser_impl.dart:3884-3905), and the parenthesized mixin/extension
/// heads name the `const` token itself in that report
/// (:3798-3823, `lexeme: constKeyword ?? beginPrimaryConstructor` — oracle
/// 13|7 "Unexpected text 'const'.", 29|11 same; 5|8 / 21|12 "Unexpected
/// text '('." when no `const` precedes the head).
#[test]
fn pre_feature_const_heads_take_the_unexpected_token_face() {
    for (source, start, end, text, label) in [
        (
            "mixin const M() {}\n",
            6,
            11,
            "Unexpected text 'const'.",
            "mixin const head",
        ),
        (
            "mixin M() {}\n",
            7,
            8,
            "Unexpected text '('.",
            "mixin paren head",
        ),
        (
            "extension const E() on int {}\n",
            10,
            15,
            "Unexpected text 'const'.",
            "extension const head",
        ),
        (
            "extension E() on int {}\n",
            11,
            12,
            "Unexpected text '('.",
            "extension paren head",
        ),
    ] {
        let (got, texts) = rows_with(source, pre_feature_features());
        assert_eq!(
            got,
            [row("unexpected_token", start, end)],
            "{label} at 3.10"
        );
        assert_eq!(texts[0], text, "{label} at 3.10 message text");
    }
    // `extension type const E {}` reports the missing primary constructor
    // on the name and the representation-field gap at the body at every
    // language version; never the `const` face (oracle 4|22, 4|24).
    for features in [pre_feature_features(), latest_features()] {
        let (got, _) = rows_with("extension type const E {}\n", features);
        assert_eq!(
            got,
            [
                row("missing_primary_constructor", 21, 22),
                row("expected_representation_field", 23, 24),
            ],
            "extension type const head: {got:?}"
        );
    }
}

/// With the feature on, the parenthesized mixin/extension heads keep their
/// per-kind faces on the same anchor (`const` when present, else the `(`).
#[test]
fn post_feature_const_heads_keep_the_per_kind_face() {
    assert_eq!(
        rows("mixin const M() {}\n"),
        [row("mixin_primary_constructor", 6, 11)],
    );
    assert_eq!(
        rows("mixin M() {}\n"),
        [row("mixin_primary_constructor", 7, 8)],
    );
    assert_eq!(
        rows("extension const E() on int {}\n"),
        [row("extension_primary_constructor", 10, 15)],
    );
}

/// The full pre-feature fixture at its `// @dart=3.10` pin: zero
/// `const_without_primary_constructor` rows anywhere (the burn row's whole
/// claim).
#[test]
fn pre_feature_fixture_never_reports_const_without_primary_constructor() {
    let (got, _) = rows_with(
        concat!(
            "class const C {}\n",
            "enum const E {}\n",
            "extension type const E {}\n",
            "mixin M() {}\n",
            "mixin M(int i) {}\n",
            "mixin M.named() {}\n",
            "mixin M.named(int i) {}\n",
            "mixin M<T>() {}\n",
            "mixin M<T>(int i) {}\n",
            "mixin M<T>.named() {}\n",
            "mixin M<T>.named(int i) {}\n",
            "mixin const M() {}\n",
            "mixin const M(int i) {}\n",
            "mixin const M.named() {}\n",
            "mixin const M.named(int i) {}\n",
            "mixin const M<T>() {}\n",
            "mixin const M<T>(int i) {}\n",
            "mixin const M<T>.named() {}\n",
            "mixin const M<T>.named(int i) {}\n",
            "extension E() on int {}\n",
            "extension E(int i) on int {}\n",
            "extension E.named() on int {}\n",
            "extension E.named(int i) on int {}\n",
            "extension E<T>() on int {}\n",
            "extension E<T>(int i) on int {}\n",
            "extension E<T>.named() on int {}\n",
            "extension E<T>.named(int i) on int {}\n",
            "extension const E() on int {}\n",
            "extension const E(int i) on int {}\n",
            "extension const E.named() on int {}\n",
            "extension const E.named(int i) on int {}\n",
            "extension const E<T>() on int {}\n",
            "extension const E<T>(int i) on int {}\n",
            "extension const E<T>.named() on int {}\n",
            "extension const E<T>.named(int i) on int {}\n",
        ),
        pre_feature_features(),
    );
    let const_faces: Vec<_> = got
        .iter()
        .filter(|(code, _, _)| code == "const_without_primary_constructor")
        .collect();
    assert!(
        const_faces.is_empty(),
        "const_without_primary_constructor at 3.10: {const_faces:?}"
    );
    // Every `const`-headed or `(`/`.`-headed declaration of the fixture
    // collapses into the plain unexpected-token row: 35 declarations minus
    // `extension type const E {}` (its own two-row face) = 34 rows.
    let unexpected = got
        .iter()
        .filter(|(code, _, _)| code == "unexpected_token")
        .count();
    assert_eq!(unexpected, 34, "unexpected_token rows at 3.10: {got:?}");
}
