//! M0-gate wave-7 regression pins (#607): class-head faces — the
//! `mixin class` modifier head, missing declaration bodies, and the stray
//! `)` after a class-head face. The official decision order
//! (`parser_impl.dart`, Dart 3.15 main; primary constructors enabled at the
//! corpus language version 3.13):
//!
//! * `mixin class M` dispatch: `_handleModifiersForClassDeclaration`
//!   (:850-876) starts `parseClassOrNamedMixinApplication` AT the `class`
//!   token (`Token token = classKeyword`, :2969); the modifier walk
//!   (`_parseModifiers`, modifier_context.dart:345-384) stops on the
//!   `mixin` head, so only the invalid-combination rows are minted
//!   (`finalMixinClass`/`interfaceMixinClass`/`sealedMixinClass`,
//!   :858-866) and the name step sees `M`. Taking `class` as the name
//!   instead minted `expected_identifier_but_got_keyword` at `class` plus
//!   an `unexpected_token` at `M` (six differential rows across the
//!   `final/interface/sealed_mixin_class.dart` fixtures).
//! * Missing declaration bodies: `ensureBlock` reports message-carrying
//!   kinds on the token BEFORE the missing `{` —
//!   `reportRecoverableError(token, message)` (:4995-5002) anchored per
//!   `findNonZeroLengthToken` (util.dart:56-61, forward over zero-width
//!   synthetics) — while the template flavors (`expectedEnumBody`
//!   block_kind.dart:31-34) report on `next` with the lexeme (:5003-5005).
//! * The header gunk skip (`recoverySmallLookAheadSkipTokens`, :2861-2895)
//!   probes without consuming: it walks `token.next` pointers and on
//!   failure returns null leaving the pointer untouched, and a
//!   next-declaration head is a BAIL condition only (:2870/:2882) —
//!   success is a clause boundary in `lookFor` alone — so the missing
//!   body's `ensureBlock` anchor lands on the real follower (`enum E`
//!   followed by `int f() {}` anchors on `int`) and the follower still
//!   parses as its own declaration.
//! * A stray `)` after a class head is ONE `unexpected_token` row: the
//!   class-late header recovery's `skipUnexpectedTokenOpt` (:5148-5160,
//!   follower `{` in the expected set) or the enum header's gunk skip
//!   (:2744-2752) reports and consumes it once; the body then parses.

use super::collector::setup;

/// Parses `source` and asserts the exact in-order (code, start, end)
/// stream (UTF-16 offsets), then hands the listener to `check` for the
/// structural event/message-template assertions.
fn assert_stream(
    source: &'static str,
    expected: &[(&str, u32, u32)],
    label: &str,
) -> super::collector::Collector {
    let mut p = setup(source);
    p.parse_unit();
    let got: Vec<(String, u32, u32)> = p.listener.errors.clone();
    let want: Vec<(String, u32, u32)> = expected
        .iter()
        .map(|(c, s, e)| (ToString::to_string(c), *s, *e))
        .collect();
    assert_eq!(got, want, "{label}: diagnostic stream diverged");
    p.listener
}

// —— family: `mixin class` modifier-M combination heads —————————————————————

/// `mixin class M {}` is the valid mixin-class shape: the head parses as a
/// plain class declaration with zero diagnostics (official routes
/// `mixin` + `class` through `_handleModifiersForClassDeclaration` with
/// `mixinToken` set, parser_impl.dart:792-801).
#[test]
fn plain_mixin_class_head_is_clean() {
    let listener = assert_stream("mixin class M {}\n", &[], "plain mixin class");
    listener.assert_sequence(&[("begin", "class_declaration"), ("end", "class_declaration")]);
}

/// `final mixin class M {}` (the `final_mixin_class.dart` fixture): the
/// modifier-combination row anchors on the `final` token
/// (parser_impl.dart:858-860) and the head still parses as a class named
/// `M` — no name-row, no `unexpected_token` at `M`. Pre-fix this minted
/// `expected_identifier_but_got_keyword` at `class` (the name step) plus
/// `unexpected_token` at `M` (the header recovery).
#[test]
fn final_mixin_class_reports_only_the_modifier_row() {
    let listener = assert_stream(
        "final mixin class M {}\n",
        &[("final_mixin_class", 0, 5)],
        "final mixin class",
    );
    assert_eq!(
        listener.texts,
        ["A mixin class can't be declared 'final'."],
        "message templates diverged"
    );
    listener.assert_sequence(&[("begin", "class_declaration"), ("end", "class_declaration")]);
}

/// `interface mixin class M {}` (the `interface_mixin_class.dart`
/// fixture): the captured pseudo-keyword reports `interfaceMixinClass`
/// (parser_impl.dart:861-863) and the head parses clean after it.
#[test]
fn interface_mixin_class_reports_only_the_modifier_row() {
    let listener = assert_stream(
        "interface mixin class M {}\n",
        &[("interface_mixin_class", 0, 9)],
        "interface mixin class",
    );
    assert_eq!(
        listener.texts,
        ["A mixin class can't be declared 'interface'."],
        "message templates diverged"
    );
    listener.assert_sequence(&[("begin", "class_declaration"), ("end", "class_declaration")]);
}

/// `sealed mixin class M {}` (the `sealed_mixin_class.dart` fixture):
/// `sealedMixinClass` at the `sealed` token (parser_impl.dart:864-866).
#[test]
fn sealed_mixin_class_reports_only_the_modifier_row() {
    let listener = assert_stream(
        "sealed mixin class M {}\n",
        &[("sealed_mixin_class", 0, 6)],
        "sealed mixin class",
    );
    assert_eq!(
        listener.texts,
        ["A mixin class can't be declared 'sealed'."],
        "message templates diverged"
    );
    listener.assert_sequence(&[("begin", "class_declaration"), ("end", "class_declaration")]);
}

// —— family: expected_body — class-head faces missing their body ————————————

/// `class C(int a)` at EOF (the `class_primary_constructor_error.dart`
/// tail face): the header recovery restarts before the primary-constructor
/// parameters and jams on the `(` (its follower `int` is not a clause
/// keyword), so `ensureBlock` reports the message-flavored
/// `expectedClassBody` ON the `(` (6..7) and the params re-enter the class
/// body as junk members (probed 3.13.1: `expected_body` @`(`,
/// `expected_executable` @`int`, `expected_token(';')` @`a`,
/// `expected_executable` @`)`).
#[test]
fn class_with_primary_constructor_missing_body_anchors_on_open_paren() {
    let listener = assert_stream(
        "class C(int a)",
        &[
            ("expected_body", 6, 7),
            ("expected_executable", 7, 8),
            ("expected_token", 12, 13),
            ("expected_executable", 13, 14),
        ],
        "class head missing body at EOF",
    );
    assert_eq!(
        listener.texts,
        [
            "A class declaration must have a body, even if it is empty.",
            "Expected a method, getter, setter or operator declaration.",
            "Expected to find ';'.",
            "Expected a method, getter, setter or operator declaration.",
        ],
        "message templates diverged"
    );
}

/// Bare `class` at EOF (the `issue_46346_prime_2.dart` fixture): the
/// synthetic name inserts at the EOF position without consuming, so the
/// forward `findNonZeroLengthToken` walk anchors BOTH rows on the
/// zero-width EOF position (oracle: line 1 col 6 length 0) — the
/// name-recovery row (our `missing_identifier`, the shared
/// `MISSING_IDENTIFIER` registry code of the template's
/// `expectedIdentifier`) and the `expectedClassBody` row.
#[test]
fn bare_class_missing_body_anchors_at_eof() {
    let listener = assert_stream(
        "class",
        &[("missing_identifier", 5, 5), ("expected_body", 5, 5)],
        "bare class at EOF",
    );
    assert_eq!(
        listener.texts,
        [
            "Expected an identifier.",
            "A class declaration must have a body, even if it is empty.",
        ],
        "message templates diverged"
    );
}

// —— family: missing_enum_body — enum headers without a body ———————————————— —

/// Bare `enum` followed by the next declaration (the
/// `issue_48371_prime3.dart` head): the reserved `class` is reported as
/// the missing name WITHOUT being consumed (the enumDeclaration template
/// inserts behind a `looksLikeStartOfNextTopLevelDeclaration` head,
/// identifier_context_impl.dart:288-338), the header recovery's probe
/// bails on the `class` head without reporting (official
/// `recoverySmallLookAheadSkipTokens` bails on next-declaration heads,
/// parser_impl.dart:2870/2882 — success is a `lookFor` boundary alone),
/// and `ensureBlock` reports the `expectedEnumBody` template through
/// `findNonZeroLengthToken`, which walks forward over the zero-width
/// synthetic name onto `class` (oracle `issue_48371_prime3`: line 3
/// col 1 length 5). The synthetic name is then consumed by the enum
/// tail's jump to its synthetic block closer, and the follower
/// declaration re-enters declaration parsing directly — the oracle emits
/// no diagnostics on that tail (the post-wave differential carries no
/// cascade rows for the twin fixture, and r5 lists the old port's
/// `unexpected_token` there dartq-only), so the face pins the two family
/// rows as a prefix, guards against the synthetic surfacing as a junk
/// row, and pins the declaration re-entry.
#[test]
fn bare_enum_missing_body_anchors_on_next_declaration() {
    let mut p = setup("enum\n\nclass A {}\n");
    p.parse_unit();
    let got: Vec<(String, u32, u32)> = p.listener.errors.clone();
    assert!(
        got.len() >= 2
            && got[0] == ("missing_identifier".to_string(), 6, 11)
            && got[1] == ("missing_enum_body".to_string(), 6, 11),
        "family rows diverged: {got:?}"
    );
    assert!(
        !p.listener
            .error_codes()
            .iter()
            .any(|c| c == "unexpected_token"),
        "the synthetic name must not surface as a junk row: {:?}",
        p.listener.errors
    );
    assert_eq!(
        &p.listener.texts[..2],
        [
            "Expected an identifier.",
            "An enum definition must have a body with at least one constant name.",
        ],
        "message templates diverged"
    );
    p.listener.assert_sequence(&[
        ("begin", "enum_declaration"),
        ("end", "enum_declaration"),
        ("begin", "class_declaration"),
        ("end", "class_declaration"),
    ]);
}

/// `enum E` followed by a top-level function (the
/// `issue_48371_prime3.dart` tail): the gunk probe (`int`, `f`, `(`)
/// finds no boundary within the 3-token lookahead and rewinds WITHOUT
/// consuming (official `recoverySmallLookAheadSkipTokens` walks `next`
/// pointers only, parser_impl.dart:2861-2895), so the `ensureBlock`
/// template anchors on the real follower `int` and the function parses
/// clean after the enum. Pre-fix the probe consumed the lookahead, the
/// anchor fell on `)` and the residue minted extra top-level junk rows.
#[test]
fn enum_missing_body_keeps_the_follower_intact() {
    let listener = assert_stream(
        "enum E\n\nint f() {}\n",
        &[("missing_enum_body", 8, 11)],
        "enum E followed by a function",
    );
    assert_eq!(
        listener.texts,
        ["An enum definition must have a body with at least one constant name."],
        "message templates diverged"
    );
    listener.assert_sequence(&[
        ("begin", "enum_declaration"),
        ("end", "enum_declaration"),
        ("begin", "top_level_member"),
    ]);
}

// —— family: unexpected_token on the stray `)` after a class-head face ——————

/// `class C) {}`: the late header recovery's `skipUnexpectedTokenOpt`
/// (parser_impl.dart:5148-5160 — the follower `{` is in the expected
/// set) reports the stray once at the `)` and consumes it; the body then
/// parses. Pre-fix this face double-reported through a re-entering skip.
#[test]
fn stray_close_paren_after_class_name_reports_once() {
    let listener = assert_stream(
        "class C) {}",
        &[("unexpected_token", 7, 8)],
        "stray ) after class name",
    );
    assert_eq!(listener.texts, ["Unexpected text ')'."]);
}

/// `class C<T>) {}`: the type-parameter group parses first; the stray `)`
/// face is identical.
#[test]
fn stray_close_paren_after_type_parameters_reports_once() {
    assert_stream(
        "class C<T>) {}",
        &[("unexpected_token", 10, 11)],
        "stray ) after type parameters",
    );
}

/// `enum E) {}`: the enum header's gunk skip finds `{` one token ahead
/// (:2744-2752, single-token flavor) and reports once at the `)` — the
/// spill-drain machinery (wave 6) is not involved: this `)` is consumed
/// by the header recovery, not drained.
#[test]
fn stray_close_paren_after_enum_name_reports_once() {
    let listener = assert_stream(
        "enum E) {}",
        &[("unexpected_token", 6, 7)],
        "stray ) after enum name",
    );
    assert_eq!(listener.texts, ["Unexpected text ')'."]);
}

/// `enum E<T>) {}`: same face behind type parameters.
#[test]
fn stray_close_paren_after_enum_type_parameters_reports_once() {
    assert_stream(
        "enum E<T>) {}",
        &[("unexpected_token", 9, 10)],
        "stray ) after enum type parameters",
    );
}

/// `extension type E) {}`: the extension-type head carries its own
/// missing-primary-constructor rows (the parser's
/// `missingPrimaryConstructor` on the last consumed token plus the
/// verifier's `expectedRepresentationField` on the pending token,
/// production `parse_primary_constructor_opt`), then the header recovery
/// reports the stray `)` once — the same one-row shape as the class face.
#[test]
fn stray_close_paren_after_extension_type_name_reports_once() {
    let listener = assert_stream(
        "extension type E) {}",
        &[
            ("missing_primary_constructor", 15, 16),
            ("expected_representation_field", 16, 17),
            ("unexpected_token", 16, 17),
        ],
        "stray ) after extension type name",
    );
    assert_eq!(
        listener.texts,
        [
            "An extension type declaration must have a primary constructor declaration.",
            "Expected a representation field.",
            "Unexpected text ')'.",
        ],
        "message templates diverged"
    );
}
