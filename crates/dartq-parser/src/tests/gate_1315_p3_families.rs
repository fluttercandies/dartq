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

/// A reserved word heading a top-level declaration in the typedef-name slot
/// is NOT the recovered-name shape (upstream `isIdentifierRecovered` requires
/// the group successor to be `(`/`=`, parser_impl.dart:1590-1594): the normal
/// `ensureIdentifier` template splices a synthetic name and keeps `var` for
/// the declaration loop (`TypedefDeclarationIdentifierContext` branch 2,
/// identifier_context_impl.dart:1417-1424). Oracle rows for `typedef var a;`
/// (offsets are utf16):
///
/// * `expected_token ';'` @0|7 — `ensureSemicolon`'s
///   `findPreviousNonZeroLengthToken` walks back over the synthetic `()` and
///   the synthetic name to `typedef`;
/// * `missing_identifier` @8|11 — the `var` token;
/// * `missing_typedef_parameters` @8|11 — the same token.
///
/// The `name (`/`name <T>(` and `name =` shapes keep the recovered face
/// (`expectedIdentifierButGotKeyword`, oracle `typedef var();` /
/// `typedef void assert();`), and junk heads consume in doubt
/// (`typedef void 5();` — a single `missing_identifier` at `5`,
/// `insertSyntheticIdentifier(identifier)` branch 3, :1433-1439).
#[test]
fn typedef_name_reserved_word_heading_declaration_inserts_synthetic() {
    assert_eq!(
        rows_with("typedef var a;\n", super::collector::latest_features()),
        vec![
            ("missing_identifier".to_string(), 8, 11),
            ("missing_typedef_parameters".to_string(), 8, 11),
            ("expected_token".to_string(), 0, 7),
        ],
        "typedef var a; must take the synthetic-name reparse face"
    );
    // Recovered routes unchanged: `(` after the head forces keyword-as-name.
    assert_codes(
        "typedef var();\n",
        &[
            "built_in_identifier_in_declaration",
            "expected_identifier_but_got_keyword",
        ],
        "typedef var(); stays keyword-as-name",
    );
    assert_codes(
        "typedef void assert();\n",
        &[
            "built_in_identifier_in_declaration",
            "expected_identifier_but_got_keyword",
        ],
        "typedef void assert(); stays keyword-as-name",
    );
    assert_codes(
        "typedef var = int;\n",
        &[
            "built_in_identifier_in_declaration",
            "expected_identifier_but_got_keyword",
        ],
        "typedef var = int; stays keyword-as-name",
    );
    // Non-keyword junk head consumes in doubt: the single missing_identifier
    // row, no expected_executable and no cascade (oracle `typedef void 5();`).
    assert_eq!(
        rows_with("typedef void 5();\n", super::collector::latest_features()),
        vec![("missing_identifier".to_string(), 13, 14)],
        "typedef void 5(); consumes the junk in doubt"
    );
    // The `(`/`<`/`=`/`;`/EOF heads stay leave-posture (branch 2 keep set).
    assert_eq!(
        rows_with("typedef void ();\n", super::collector::latest_features()),
        vec![("missing_identifier".to_string(), 13, 14)],
        "typedef void (); keeps ( for the parameter list"
    );
    // Well-formed typedefs stay row-free.
    assert_codes("typedef void F(int x);\n", &[], "clean old-style typedef");
    assert_codes(
        "typedef F = void Function();\n",
        &[],
        "clean new-style typedef",
    );
}

// —— C6c: record-type typedef return faces —————————————————————————————

/// `typedef <recordType> <name>(<formals>);` — the `(` right after `typedef`
/// is the RETURN TYPE's record literal, not a function's parameter list. The
/// dispatch gate (`typedef_looks_like_record`, declarations.rs) must probe
/// `compute_type` AT the `(` — probing from `typedef` itself yields `NoType`
/// (the keyword is not a type reference), which fell through to the
/// top-level-member path and minted `missing_function_body` + a
/// `duplicate_definition 'typedef'` cascade per record typedef. Oracle:
/// `record_type_annotations_A01_t04` lines 25-29, parser-fe `record_typedef`
/// — all row-free (upstream `parseTypedef` → `computeType(typedefKeyword)`,
/// `parser_impl.dart:1457`, reads `token.next` = the `(`).
#[test]
fn typedef_record_return_type_parses_as_typedef() {
    // Exact corpus shape: record-typed return + record-typed parameter.
    assert!(
        rows_with(
            "typedef (int, String name, bool) R1((int, String name, bool) r);\n",
            super::collector::latest_features()
        )
        .is_empty(),
        "record-typedef head must parse clean"
    );
    // Positional record return + empty formals.
    assert!(
        rows_with("typedef (int, String) R2();\n", super::collector::latest_features())
            .is_empty()
    );
    // Named-field record return.
    assert!(
        rows_with(
            "typedef (int, {String s}) baz();\n",
            super::collector::latest_features()
        )
        .is_empty()
    );
    // Empty record `()` return type stays a typedef too.
    assert!(
        rows_with("typedef () foo();\n", super::collector::latest_features()).is_empty()
    );
    // Sibling: single-field record without trailing comma keeps its own row
    // (parseRecordType's `recordTypeOnePositionalNoTrailingComma`,
    // parser_impl.dart:1849-1855) — the typedef head is not the owner. (dartq
    // anchors on the `)`; the oracle anchors on the `(` — a documented
    // anchor-drift residual outside this face.)
    assert_eq!(
        rows_with(
            "typedef (int) bar(int x);\n",
            super::collector::latest_features()
        ),
        vec![("record_type_one_positional_no_trailing_comma".to_string(), 12, 13)],
    );
    // Sibling: `typedef` followed by a non-record `(` shape still takes the
    // member path unchanged (`typedef` usable as a plain identifier).
    // (assert via a clean non-record function pair)
    assert!(
        rows_with("typedef void R4();\n", super::collector::latest_features()).is_empty()
    );
}

// —— C6b: class-member tail recovery + adopted-closer faces —————————————

/// `recoverFromInvalidMember`'s tail (upstream `parser_impl.dart:11350-11368`):
/// ANY progress since the member head routes to `parseFields` — a `}`/`EOF`
/// follower included — so `class C { var }` surfaces the field's own
/// `missing_identifier` + `expected_token ';'`, never `expected_class_member`
/// (the Oracle face for `MethodTest` `*_noName` shapes). Zero progress still
/// reports `expected_class_member` on the junk token (`5`, `;`).
#[test]
fn class_member_tail_routes_progress_to_parse_fields() {
    // `var` consumed the member-head walk: `class C { var }` parses as an
    // unnamed `var` field — `;` reported on `var` (10..13, the walk-back
    // anchor over the synthetic name) and `missing_identifier` on `}`
    // (14..15). Oracle `partial_code/class.dart` MethodTest shapes.
    assert_eq!(
        rows_with("class C { var }\n", super::collector::latest_features()),
        vec![
            ("missing_identifier".to_string(), 14, 15),
            ("expected_token".to_string(), 10, 13),
        ],
    );
    // Zero progress on a literal/`;` head keeps `expected_class_member`.
    for src in ["class C { 5 }\n", "class C { ; }\n"] {
        assert_eq!(
            rows_with(src, super::collector::latest_features()),
            vec![("expected_class_member".to_string(), 10, 11)],
            "{src:?}",
        );
    }
    // Clean field stays row-free.
    assert!(rows_with("class C { var x; }\n", super::collector::latest_features()).is_empty());
}

/// The `(`-unclosed member shape: `ensureCloseParen`'s `moveSynthetic` arm
/// (upstream `parser_impl.dart:5035-5037`) relocates the scanner-parked
/// synthetic `)` out of the stream, so the enclosing member loop never sees
/// it — `class C { m( void a(b) {} }` emits only `expected_token ')'`
/// (@the `{` where the group should have closed, length 1 via
/// `translateErrorToken`'s unmatched arm, `translate_error_token.dart:89-104`),
/// no trailing `expected_class_member` on a resurrected 0-width `)`.
#[test]
fn adopted_paren_closer_never_resurfaces_as_member() {
    assert_eq!(
        rows_with(
            "class C { m( void a(b) {} }\n",
            super::collector::latest_features()
        ),
        vec![("expected_token".to_string(), 23, 24)],
    );
    // A well-formed twin stays silent.
    assert!(
        rows_with("class C { m() {} }\n", super::collector::latest_features()).is_empty()
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

// —— C6d: pattern-assignment object-pattern gate ————————————————————————

/// `skipOuterPattern`'s object-pattern arm (upstream `parser_impl.dart:12345-
/// 12352` + `skipObjectPatternRest` :12354): `pos` tracks the LAST type-name
/// token, so the pattern group's `(` is the next offset — the prior port
/// checked `kind_at(pos)` (the type name itself) and always returned `None`,
/// so `TypeName(...) = expr` fell through to expression parsing and emitted
/// `illegal_assignment_to_non_assignable` + `missing_identifier` on the
/// `var`/`final`/type/`:`-shorthand field heads. The `<...>` skip already
/// lands `pos` on the `(`; the identifier-only/`id.id` walks advance `pos+1`.
#[test]
fn object_pattern_assignment_enters_pattern_path() {
    // `Square(areaAsInt: var a) = x` parses as a pattern assignment: the
    // parser consumes `var a` silently; `pattern_assignment_declares_variable`
    // is the analyzer-side face dartq's parser mirrors on the name slot.
    let rows = rows_with(
        "class Square { final int areaAsInt; Square(this.areaAsInt); }\nmain() {\n  var x = Square(1);\n  Square(areaAsInt: var a) = x;\n}\n",
        super::collector::latest_features(),
    );
    // No `illegal_assignment_to_non_assignable` / `missing_identifier` may
    // survive — the `(...) =` tail routed to `parse_pattern_assignment`.
    assert!(
        !rows
            .iter()
            .any(|(c, _, _)| c == "missing_identifier" || c == "illegal_assignment_to_non_assignable"),
        "object pattern must route to parse_pattern_assignment, got {rows:?}"
    );
    assert!(
        rows.iter()
            .any(|(c, _, _)| c == "pattern_assignment_declares_variable"),
        "the var-field name slot must surface pattern_assignment_declares_variable: {rows:?}"
    );
    // A `final`/typed/`const` field value all stay inside the pattern path
    // (`name: <declaring pattern>` — the field label names the slot).
    for tail in ["final b", "int c", "const e"] {
        let src = format!(
            "class Square {{ final int areaAsInt; Square(this.areaAsInt); }}\nmain() {{\n  var x = Square(1);\n  Square(areaAsInt: {tail}) = x;\n}}\n"
        );
        let rows = rows_with(Box::leak(src.into_boxed_str()), super::collector::latest_features());
        assert!(
            !rows
                .iter()
                .any(|(c, _, _)| c == "missing_identifier" || c == "illegal_assignment_to_non_assignable"),
            "{tail:?} must stay in the pattern path, got {rows:?}"
        );
    }
    // Shorthand `:var d` (no explicit field name — the name is inferred from
    // the sub-pattern's variable) is the `kind == Colon` arm of the field
    // loop; `var d` still routes through `parse_variable_pattern`.
    let rows = rows_with(
        "class Square { final int d; Square(this.d); }\nmain() {\n  var x = Square(1);\n  Square(:var d) = x;\n}\n",
        super::collector::latest_features(),
    );
    assert!(
        !rows
            .iter()
            .any(|(c, _, _)| c == "missing_identifier" || c == "illegal_assignment_to_non_assignable"),
        "shorthand `:var d` must stay in the pattern path, got {rows:?}"
    );
    assert!(
        rows.iter()
            .any(|(c, _, _)| c == "pattern_assignment_declares_variable"),
        "the shorthand var name slot must surface pattern_assignment_declares_variable: {rows:?}"
    );
    // A clean object-pattern assignment keeps the pattern path row-free.
    assert!(
        rows_with(
            "class Square { final int a; Square(this.a); }\nmain() {\n  var x = Square(1);\n  var a = 0;\n  Square(a: a) = x;\n}\n",
            super::collector::latest_features(),
        )
        .iter()
        .all(|(c, _, _)| c != "missing_identifier" && c != "illegal_assignment_to_non_assignable")
    );
}

// —— #2965: record-typedef `...Eq`-tail (shared compound closer) ——————————————

/// The `typedef <name><<typeVars>>=<RHS>` shape whose type-variable bound
/// ends on a `>=`/`>>=`/`>>>=`/`>>>>=`/`>>>>>=` compound closer shared with
/// enclosing `<` groups. The bound's group closes on the compound's *last*
/// `>` unit (`end_shared`), leaving `=` as the successor — the official
/// stream's `splitGtFromGtGt*`/`splitCloser` remainder
/// (`util.dart:140-234`), which the new-style-typedef discriminator reads as
/// `=` (`parser_impl.dart:1497`). The prior port treated the whole `...Eq`
/// tail as a `>`-closer unit count and forced `end_raw_compound`/`backed_up`
/// wrong, so the head scanned as a generic `id<...>` type (NoType never
/// fired) and the follower read `end+1` (the real `T` RHS head) instead of
/// the `=` remainder — falling to old-style parse and emitting
/// `missing_typedef_parameters`/`expected_token(';')`/`missing_function_body`
/// EXTRA rows. Oracle (`dart analyze` 3.47.1) reports zero rows for all
/// shapes; the `>`-only `closer_units` + `end_shared`/`shared_remainder`
/// encoding restores parity.
#[test]
fn record_typedef_shared_eq_closer_parses_row_free() {
    for src in [
        // Baseline record-typedef faces — always row-free.
        "typedef (int, int) T1();\n",
        "typedef ({int j}) T2();\n",
        "typedef (int, int)? T3();\n",
        "typedef ({int j})? T4();\n",
        // `>=`-tail single bound (raw compound, first `>` is the group's).
        "typedef F1<T extends (int, int)>= T Function();\n",
        // Shared `...Eq` tails: the bound's `>` is the compound's last unit,
        // `=` is the successor. F2/F3/F5/F6 were the EXTRA cluster; F4 is
        // the pure-`>` (`>>>>`) sibling that already parsed clean.
        "typedef F2<T extends List<(int, int)>>= T Function();\n",
        "typedef F3<T extends List<List<(int, int)>>>= T Function();\n",
        "typedef F4<T extends List<List<List<(int, int)>>>>= T Function();\n",
        "typedef F5<T extends List<List<List<List<(int, int)>>>>>= T Function();\n",
        "typedef F6<T extends List<List<List<List<List<(int, int)>>>>>>= T Function();\n",
    ] {
        let rows = rows_with(src, super::collector::latest_features());
        assert!(
            rows.is_empty(),
            "{src:?} must be row-free like the oracle, got {rows:?}"
        );
    }
    // The pure-`>` shared tail (`>>>`/`>>>>`/`>>>>>`/`>>>>>>`) on a *class*
    // type-parameter bound exercises the same `step_over_type`/`backed_up`
    // walk with no `=` remainder — the bound's `>` is still the compound's
    // last unit, but the successor is a real `>`-closer read by the
    // shared-continuation check (no `end_eq_shared`). Oracle reports zero
    // rows (unsigned_right_shift_test class `E3`..`E6` shapes).
    for src in [
        "class E3<T extends List<List<int>>> {}\n",
        "class E4<T extends List<List<List<int>>>> {}\n",
        "class E5<T extends List<List<List<List<int>>>>> {}\n",
        "class E6<T extends List<List<List<List<List<int>>>>>> {}\n",
    ] {
        let rows = rows_with(src, super::collector::latest_features());
        assert!(
            rows.is_empty(),
            "{src:?} must be row-free like the oracle, got {rows:?}"
        );
    }
}
