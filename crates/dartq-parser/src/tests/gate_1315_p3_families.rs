//! #1315 P3-family regression pins: the still-red burn-down lanes for the
//! augmentation member faces (FAM5), the keyword-named typedef faces (FAM6),
//! and the primary-constructor feature posture (FAM8). Every expected
//! multiset is the traced 3.13.1 oracle face from the still-red evidence
//! replay (`target/evidence/still-red-classification`, m0-merged raw
//! outputs), with the decision points mirrored from the upstream parser
//! (`upstream Dart SDK source, pkg/_fe_analyzer_shared, parser_impl.dart`).

use super::collector::{scan_ok, setup};
use crate::ParserFeatures;
use dartq_language::{FeatureSet, LanguageVersion};

/// Sorted diagnostic-code multiset for `source` under `features`.
fn codes_with(source: &'static str, features: ParserFeatures) -> Vec<String> {
    let (table, text) = scan_ok(source);
    let mut collector = super::collector::Collector::default();
    let mut parser = crate::Parser::new(text, table, features, &mut collector);
    parser.parse_unit();
    let mut codes = parser.listener.error_codes();
    codes.sort();
    codes
}

fn assert_codes(source: &'static str, expected: &[&str], label: &str) {
    let got = codes_with(source, super::collector::latest_features());
    let expected: Vec<String> = expected.iter().map(ToString::to_string).collect();
    assert_eq!(got, expected, "{label}: diagnostic multiset diverged");
}

/// 3.10 feature set: primary constructors off (`enabledIn: 3.13.0`), the
/// `// @dart=3.10` corpus posture.
fn pre_feature_features() -> ParserFeatures {
    ParserFeatures::from_feature_set(FeatureSet::for_language_version(LanguageVersion::new(
        3, 10,
    )))
}

// —— FAM5: uninitialized const/final field faces ————————————————————————

/// The parser's uninitialized-field reports ride the pseudoSharedCode
/// routing: the shared parser's `ConstFieldWithoutInitializer` /
/// `FinalFieldWithoutInitializer` surface under the analyzer registry codes
/// `const_not_initialized` / `final_not_initialized`
/// (`parseFieldInitializerOpt`, parser_impl.dart:4702-4721; analyzer
/// diagnostic.g.dart `constNotInitialized` :3148, `finalNotInitialized`
/// :6855). Oracle: `augmentation/member_errors.dart` 35|26 and
/// `augmentation/top_level_declarations.dart` 12|20 carry those codes, not
/// the shared parser's names.
#[test]
fn uninitialized_field_faces_use_analyzer_codes() {
    // Member flavor: anchored on the name, const arm has no kind filter
    // (parser_impl.dart:4706 — only `name.isSynthetic` suppresses).
    assert_codes(
        "class C {\n  const field;\n}\n",
        &["const_not_initialized"],
        "const member field without initializer",
    );
    // Top-level flavor: the `final` arm requires the TopLevel kind and a
    // clean modifier set (parser_impl.dart:4711-4718).
    assert_codes(
        "final int field;\n",
        &["final_not_initialized"],
        "top-level final field without initializer",
    );
    // Initialized fields stay silent on both arms.
    assert_codes("final int field = 0;\n", &[], "initialized final field");
    // The const template carries the analyzer wording (the shared parser's
    // "The const variable" text loses to the registry template).
    let mut parser = setup("class C {\n  const field;\n}\n");
    parser.parse_unit();
    assert_eq!(
        parser.listener.texts,
        vec!["The constant 'field' must be initialized.".to_string()],
        "const face text must be the analyzer template"
    );
    // Class-member `final` without initializer takes NO parser row (the
    // `final` arm is TopLevel-only; the oracle's member-flavor
    // final_not_initialized rows are resolution-phase).
    assert_codes(
        "class C {\n  final int field;\n}\n",
        &[],
        "member final field stays parser-clean",
    );
}

// —— FAM6: keyword-named typedef faces ———————————————————————————————————

/// Every reserved or built-in keyword taking the typedef-name position
/// surfaces TWO rows at the name token: the identifier-recovery
/// `expected_identifier_but_get_keyword` face (the official
/// `ensureIdentifierPotentiallyRecovered` recovered variant — parseTypedef
/// passes `isIdentifierRecovered = true` for any non-identifier head heading
/// the `(`/`=` group, parser_impl.dart:1494/:1583) plus the analyzer's
/// per-context `built_in_identifier_in_declaration` template (the converter
/// drops the shared parser message in favor of
/// `ErrorVerifier._checkForBuiltInIdentifierAsName`,
/// error_converter.dart:291; oracle keyword_named_typedefs.dart 1|14, 2|9:
/// "'abstract'" fires both). Pseudo keywords are legal names and stay
/// completely silent (oracle lines 7-10: `async`/`await` are row-free).
#[test]
fn keyword_named_typedef_faces() {
    // Built-in keyword, old style.
    assert_codes(
        "typedef void abstract();\n",
        &[
            "built_in_identifier_in_declaration",
            "expected_identifier_but_got_keyword",
        ],
        "built-in old-style typedef name",
    );
    // Built-in keyword, new style (`=` head).
    assert_codes(
        "typedef abstract = void Function();\n",
        &[
            "built_in_identifier_in_declaration",
            "expected_identifier_but_got_keyword",
        ],
        "built-in new-style typedef name",
    );
    // Reserved keyword: the built-in face is the verifier's isKeyword-
    // and-not-pseudo gate, so a reserved word carries BOTH rows exactly
    // like a built-in one (error_verifier.dart:3760-3766; oracle
    // keyword_named_typedefs.dart 5|14 `typedef void assert();` fires the
    // pair — the earlier one-row pin here was an under-report the frozen
    // case data disproves).
    assert_codes(
        "typedef void assert();\n",
        &[
            "built_in_identifier_in_declaration",
            "expected_identifier_but_got_keyword",
        ],
        "reserved old-style typedef name takes both faces",
    );
    // Built-in keyword name takes both faces (same face family as oracle
    // keyword_named_typedefs.dart 55|14, where the built-in is `Function`).
    assert_codes(
        "typedef void get();\n",
        &[
            "built_in_identifier_in_declaration",
            "expected_identifier_but_got_keyword",
        ],
        "built-in accessor name takes both faces",
    );
    // The `Function` face itself (oracle 55|14: `typedef void Function();`
    // fires exactly the pair at the `Function` token).
    assert_codes(
        "typedef void Function();\n",
        &[
            "built_in_identifier_in_declaration",
            "expected_identifier_but_got_keyword",
        ],
        "Function typedef name takes both faces",
    );
    // Pseudo keyword: a legal typedef name, zero rows (oracle lines 7-10).
    assert_codes(
        "typedef void async();\n",
        &[],
        "pseudo keyword name stays legal",
    );
    // The built-in template names the declaration kind.
    let mut parser = setup("typedef void abstract();\n");
    parser.parse_unit();
    assert_eq!(
        parser.listener.texts[1],
        "The built-in identifier 'abstract' can't be used as a typedef name.".to_string(),
        "typedef-name template text"
    );
}

// —— FAM8: primary-constructor feature posture ———————————————————————————

/// `const` ahead of a declaration with no primary constructor reports the
/// per-kind face only with the feature enabled; disabled, every kind
/// collapses into the plain unexpected-token report on the `const` keyword
/// (`parsePrimaryConstructorOpt`, parser_impl.dart:3884-3905). Oracle:
/// `invalid_primary_constructors_pre_feature.dart` 2|7, 3|6
/// (`unexpected_token`) vs `invalid_primary_constructors.dart` 1|7, 2|6
/// (`const_without_primary_constructor`) — same sources, posture split.
#[test]
fn const_without_primary_constructor_follows_the_posture() {
    for (source, expected, label) in [
        (
            "class const C {}\n",
            vec!["const_without_primary_constructor"],
            "class",
        ),
        (
            "enum const E {}\n",
            vec!["const_without_primary_constructor"],
            "enum",
        ),
        (
            "mixin const M {}\n",
            vec!["mixin_primary_constructor"],
            "mixin",
        ),
    ] {
        assert_codes(
            source,
            &expected,
            &format!("{label} at 3.13 keeps the per-kind face"),
        );
        let got = codes_with(source, pre_feature_features());
        assert_eq!(
            got,
            vec!["unexpected_token".to_string()],
            "{label} at 3.10 must take the unexpected-token face: {got:?}"
        );
    }
}

/// The disabled-posture message carries the offending lexeme
/// (`diag.unexpectedToken.withArguments(lexeme: constKeyword)`).
#[test]
fn posture_off_message_names_the_const_keyword() {
    let (table, text) = scan_ok("class const C {}\n");
    let mut collector = super::collector::Collector::default();
    let mut parser = crate::Parser::new(text, table, pre_feature_features(), &mut collector);
    parser.parse_unit();
    assert_eq!(
        parser.listener.texts,
        vec!["Unexpected text 'const'.".to_string()],
        "posture-off template text"
    );
}

// —— rider: missing_method_parameters anchor on the `new` head ———————————

/// (code, start, end) rows for `source` under `features`.
fn rows_with(source: &'static str, features: ParserFeatures) -> Vec<(String, u32, u32)> {
    let (table, text) = scan_ok(source);
    let mut collector = super::collector::Collector::default();
    let mut parser = crate::Parser::new(text, table, features, &mut collector);
    parser.parse_unit();
    parser.listener.errors.clone()
}

/// A `new`-headed member with no name after the keyword keeps the keyword
/// itself as the constructor name (official parseMethod `name = newToken`),
/// so the parseGetterOrFormalParameters recovery anchors
/// `missing_method_parameters` on the rejected `new` token (len 3) — never
/// on its follower. Oracle face: `general/new_as_identifier.dart` 16|44
/// (`C.constructor_field_initializer() : this.new = null;` — the leftover
/// `new = null;` re-enters the member loop as the cascade member).
#[test]
fn new_headed_member_anchors_mmp_on_the_new_keyword() {
    let source = "class C {\n  new = null;\n}\n";
    let rows = rows_with(source, super::collector::latest_features());
    // `new` spans 12..15; the `=` follower spans 16..17.
    let mmp: Vec<_> = rows
        .iter()
        .filter(|(c, _, _)| c == "missing_method_parameters")
        .collect();
    assert_eq!(
        mmp,
        vec![&("missing_method_parameters".to_string(), 12, 15)],
        "mmp must anchor on the `new` token (12..15), not the `=` follower: {rows:?}"
    );
    // The rest of the cascade keeps its shape: the redirect report stays on
    // the `=`.
    assert!(
        rows.iter()
            .any(|(c, s, e)| c == "redirection_in_non_factory_constructor" && (*s, *e) == (16, 17)),
        "redirection face must stay anchored on the `=` follower: {rows:?}"
    );
}

/// The existing `(`/`.`-headed mixin/extension faces already split on the
/// posture (parser_impl.dart:3799/:3813); pin one shape so the gate stays
/// symmetric across the two arms.
#[test]
fn paren_headed_mixin_keeps_the_posture_split() {
    assert_codes(
        "mixin M(int i) {}\n",
        &["mixin_primary_constructor"],
        "paren head at 3.13 reports the primary-constructor face",
    );
    let got = codes_with("mixin M(int i) {}\n", pre_feature_features());
    assert_eq!(
        got,
        vec!["unexpected_token".to_string()],
        "paren head at 3.10 must report the unexpected-token face: {got:?}"
    );
}

// —— rider: missing_primary_constructor_parameters anchor on the name ————
// (see the `new_headed_member_anchors_mmp_on_the_new_keyword` rider above
// for the rows_with helper)

/// `missing_primary_constructor_parameters` anchors on the constructor-name
/// identifier — official `parsePrimaryConstructorOpt` reports on `token`,
/// the `ensureIdentifier` return (:3857-3862), never on the `{` follower.
/// Oracle: `primary_constructors/class_primary_constructor_error.dart` 13
/// and `enum_primary_constructor_error.dart` 15 (`class C<T>.named {}` /
/// `enum const E<T>.named {}`).
#[test]
fn missing_primary_constructor_parameters_anchors_on_the_name() {
    let rows = rows_with("class C<T>.named {}\n", super::collector::latest_features());
    assert!(
        rows.iter()
            .any(|(c, s, e)| c == "missing_primary_constructor_parameters" && (*s, *e) == (11, 16)),
        "anchor must be the `named` keyword (11..16): {rows:?}"
    );
    let rows = rows_with(
        "enum const E<T>.named {}\n",
        super::collector::latest_features(),
    );
    assert!(
        rows.iter()
            .any(|(c, s, e)| c == "missing_primary_constructor_parameters" && (*s, *e) == (16, 21)),
        "enum face anchors on `named` (16..21) too: {rows:?}"
    );
}

// —— rider: extension-type representation field faces ———————————————————

/// The empty representation list anchors `expectedRepresentationField` on
/// the token after the `(` — the `)` itself
/// (error_verifier.dart:5371, `formalParameterList.leftParenthesis.next`),
/// never the body `{`. Oracle:
/// `targeted/blindspot_extension_types.dart` 16
/// (`extension type EmptyRepresentation() {}`).
#[test]
fn empty_representation_field_anchors_on_the_close_paren() {
    let rows = rows_with(
        "extension type E() {}\n",
        super::collector::latest_features(),
    );
    assert!(
        rows.iter()
            .any(|(c, s, e)| c == "expected_representation_field" && (*s, *e) == (17, 18)),
        "the empty list anchor is the `)` (17..18): {rows:?}"
    );
}

/// With primary constructors on (3.13), a delimited representation form
/// (`[int x]`) survives clean — the verifier reads the left delimiter only
/// in the feature-disabled arm (error_verifier.dart:5427-5434).
#[test]
fn delimited_representation_stays_silent_with_the_feature_on() {
    let got = codes_with(
        "extension type E([int x]) {}\n",
        super::collector::latest_features(),
    );
    assert!(
        !got.contains(&"expected_representation_field".to_string()),
        "a delimited form must not fire the representation face at 3.13: {got:?}"
    );
}

// —— rider: `this.new` initializer entry — single identifier event ———————

/// The full corpus cascade (`new_in_initializer.dart` 3): the initializer
/// entry takes the synthetic-name face and ends on the real `new`, which
/// re-enters the member loop as the constructor-name member — the #1315
/// `name = new_kw` arm anchors its `missing_method_parameters` on that
/// `new` keyword (oracle trace `general/new_as_identifier.dart` 96-173:
/// `MissingMethodParameters` on the leftover `new`, then
/// `RedirectionInNonFactory` on the `=`).
#[test]
fn this_new_cascade_anchors_mmp_on_the_leftover_new() {
    let rows = rows_with(
        "class C {\n  new() : this.new = 0;\n}\n",
        super::collector::latest_features(),
    );
    // `this` 20..24, `.` 24, the `new` name 25..28, `=` 30.
    assert!(
        rows.iter()
            .any(|(c, s, e)| c == "missing_method_parameters" && (*s, *e) == (25, 28)),
        "the leftover `new` member must anchor mmp on the keyword (25..28): {rows:?}"
    );
    assert!(
        rows.iter()
            .any(|(c, s, e)| c == "redirection_in_non_factory_constructor" && (*s, *e) == (29, 30)),
        "the redirect face stays on the `=` (29..30): {rows:?}"
    );
}

/// Minimal listener counting `handleIdentifier` events per context (the
/// shared [`super::collector::Collector`] does not log identifier events).
#[derive(Default)]
struct IdentifierProbe {
    errors: Vec<(String, u32, u32)>,
    texts: Vec<String>,
    continuation_ids: Vec<(u32, u32)>,
}

impl crate::listener::ParserListener for IdentifierProbe {
    fn handle_recoverable_error(
        &mut self,
        message: crate::message::Message,
        start: crate::token_id::TokenSpan,
        end: crate::token_id::TokenSpan,
    ) {
        self.texts.push(message.text);
        self.errors.push((
            message.code.name.to_owned(),
            start.range.offset,
            end.range.offset + end.range.length,
        ));
    }

    fn handle_identifier(
        &mut self,
        token: crate::token_id::TokenSpan,
        context: crate::listener::IdentifierContext,
    ) {
        if context == crate::listener::IdentifierContext::ExpressionContinuation {
            self.continuation_ids
                .push((token.range.offset, token.range.offset + token.range.length));
        }
    }
}

/// `this.new = 0;` — the reserved word after `this.` never reaches an
/// identifier gate: the silent rewrite is gated on the `(` follower
/// (`_tryRewriteNewToIdentifier` is only attempted when the token after the
/// name is `(`, parseInitializer :4830-4834), so the entry takes the
/// synthetic-name face — `expectedIdentifier` at `new` (:4841, the
/// fieldInitializer recoveryTemplate) plus `missingAssignmentInInitializer`
/// at `this` (:4902) — and drives exactly ONE identifier event, the official
/// re-read's `ensureIdentifier(., expressionContinuation)`
/// (`handleIdentifier(, expressionContinuation)`; oracle trace
/// `general/new_as_identifier.dart` 96-128). No second fire, no
/// `new`-rewrite, and the real `new` is left for the member loop.
#[test]
fn this_new_non_paren_initializer_fires_one_identifier_event() {
    let (table, text) = scan_ok("class C {\n  new() : this.new = 0;\n}\n");
    let mut identifier_probe = IdentifierProbe::default();
    let mut parser = crate::Parser::new(
        text,
        table,
        super::collector::latest_features(),
        &mut identifier_probe,
    );
    parser.parse_unit();
    let probe = parser.listener;
    // `this` spans 20..24, the `new` name 25..28. The reports anchor there.
    assert!(
        probe
            .errors
            .iter()
            .any(|(c, s, e)| c == "missing_identifier" && (*s, *e) == (25, 28)),
        "missing_identifier must anchor on the `new` name (25..28): {:?}",
        probe.errors
    );
    assert!(
        probe
            .errors
            .iter()
            .any(|(c, s, e)| c == "missing_assignment_in_initializer" && (*s, *e) == (20, 24)),
        "missing_assignment_in_initializer must anchor on `this` (20..24): {:?}",
        probe.errors
    );
    // The slot report surfaces the registry `missing_identifier` text: the
    // fasta `expectedIdentifier` template formats the got-lexeme, but its
    // pseudo-shared-code conversion re-renders the parameterless registry
    // text (frozen oracle ccf7f50c/d1b3378f: every `this.new` slot row reads
    // `Expected an identifier.`).
    assert!(
        probe.texts.iter().any(|t| t == "Expected an identifier."),
        "slot face must be the registry missing_identifier text: {:?}",
        probe.texts
    );
    // Exactly one expressionContinuation identifier: the zero-width synthetic
    // at `new`'s offset (25..25) — the double-fire used to emit two.
    assert_eq!(
        probe.continuation_ids,
        vec![(25, 25)],
        "the synthetic name must fire exactly one expressionContinuation identifier event: {:?}",
        probe.continuation_ids
    );
}
