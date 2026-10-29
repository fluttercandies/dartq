//! M0-gate wave-8 regression pins (#608): duplicate-emission dedup — the
//! double `missing_identifier` on a member head whose name slot holds junk,
//! and the parser-side abstract+final/interface/sealed modifier trio that
//! the semantic layer solely owns. Official decision order
//! (`parser_impl.dart`, Dart 3.15 main):
//!
//! * A missing member name recovers ONCE. The oracle split (probed
//!   `dart analyze` 3.13.1, machine format): the head insert reports
//!   `missing_identifier` only with a `var`/`final`/`const` modifier
//!   (`var ;` → one row on the `;`); a `late` or type-headed junk name
//!   (`late ;`, `int ;`, `x ;`) inserts silently and `parseFields`'s
//!   type-less rider is the only row, anchored on the member head — the
//!   `late` keyword or the type-position token, never the follower `;`.
//! * The `abstract`+`final`/`interface`/`sealed` class-modifier combination
//!   IS a parser face: `parseClassOrNamedMixinApplication` mints the three
//!   texts after the type variables (`parser_impl.dart:2976-3015`, oracle
//!   3.13.1 `SYNTACTIC_ERROR` rows on the keyword spans). The semantic
//!   declaration-site twin (dartq-resolution `modifiers.rs`) is the L2
//!   resolution face and must stay out of this parser stream.

use super::collector::setup;

/// Exact stream for a member head missing its name before a `;`. Oracle
/// 3.13.1 (`dart analyze --format=machine`, `late;\nint x;\n`): the `late`
/// head carries no diagnostics of its own — built-in `late` before `;` is
/// not a modifier (official `isModifier`), so it IS the field name and no
/// name is inserted; the inserted-name face only fires with a
/// `var`/`final`/`const` modifier (`var ;` probe) — and `parseFields`'s
/// type-less rider anchors on the member head `late` (offset 0..4), not the
/// follower `;`. The earlier two-row pin predated the live probe.
#[test]
fn late_semicolon_top_level_single_missing_identifier() {
    let mut p = setup("late;\nint x;\n");
    p.parse_unit();
    let got = std::mem::take(&mut p.listener.errors);
    let want = vec![("missing_const_final_var_or_type".to_owned(), 0, 4)];
    assert_eq!(got, want, "use_late `late;` head: {got:?}");
}

/// The class-member flavor (`late;` inside a class): same oracle split —
/// built-in `late` before `;` is the member NAME (no insert) and ONE
/// type-less rider anchors on the `late` keyword (probed 3.13.1: line 2
/// col 3, len 4), never `expectedClassMember` and never a name-slot
/// `missing_identifier`.
#[test]
fn late_semicolon_class_member_official_stream() {
    let mut p = setup("class C {\n  late;\n}\n");
    p.parse_unit();
    let got = std::mem::take(&mut p.listener.errors);
    let want = vec![("missing_const_final_var_or_type".to_owned(), 12, 16)];
    assert_eq!(got, want, "class-member `late;` official stream: {got:?}");
}

/// The reserved-word name recovery lane: `late;` recovers with the single
/// type-less rider, the stray `;` is the only other row, and the following
/// declaration stays clean — no inserted-name face anywhere in the unit
/// (oracle 3.13.1, `late;\n;\nlate int x;\n`: exactly these two rows).
#[test]
fn missing_name_head_then_declaration_parses_clean() {
    let mut p = setup("late;\n;\nlate int x;\n");
    p.parse_unit();
    assert_eq!(
        p.listener.error_codes(),
        ["missing_const_final_var_or_type", "unexpected_token"],
        "oracle stream for `late;` + stray `;` + clean follow-up: {:?}",
        p.listener.errors
    );
}

// —— family: abstract+final/interface/sealed trio (parser face) ————————————

/// `abstract sealed class` reports `abstract_sealed_class` anchored on the
/// `sealed` keyword (`parser_impl.dart:2986`; oracle 3.13.1: len-6 span on
/// `sealed`), and nothing else in the unit.
#[test]
fn abstract_sealed_class_parser_stream() {
    let mut p = setup("abstract sealed class C {}\n");
    p.parse_unit();
    assert_eq!(
        p.listener.errors,
        vec![("abstract_sealed_class".to_owned(), 9, 15)],
        "sealed-head pin: {:?}",
        p.listener.errors
    );
}

/// `abstract final interface class` reports `abstract_final_interface_class`
/// spanning `final interface` (parser_impl.dart:2996-2999, the
/// `finalToken`→`interface` coverage; oracle 3.13.1: len-15 span).
#[test]
fn abstract_final_interface_class_parser_stream() {
    let mut p = setup("abstract final interface class C {}\n");
    p.parse_unit();
    assert_eq!(
        p.listener.errors,
        vec![("abstract_final_interface_class".to_owned(), 9, 24)],
        "final+interface-head pin: {:?}",
        p.listener.errors
    );
}

/// `abstract final base class` reports `abstract_final_base_class` spanning
/// `final base` (parser_impl.dart:2991-2993; oracle 3.13.1: len-10 span).
#[test]
fn abstract_final_base_class_parser_stream() {
    let mut p = setup("abstract final base class C {}\n");
    p.parse_unit();
    assert_eq!(
        p.listener.errors,
        vec![("abstract_final_base_class".to_owned(), 9, 19)],
        "final+base-head pin: {:?}",
        p.listener.errors
    );
}

// —— family: keyword-named top-level declarations (recovery-spelling face) —

/// `int class = 42;` names the field with the keyword: the type-less
/// declaration recovery (`parseTopLevelMemberImpl`, `parser_impl.dart:4304` —
/// noType + reserved word + `=`) takes the keyword as the name and
/// `ensureIdentifierPotentiallyRecovered` reports it in place
/// (`expected_identifier_but_got_keyword`, oracle 3.13.1: one row at the
/// keyword), leaving `= 42;` to parse clean — no `missing_identifier` and
/// no class-header reparse cascade.
#[test]
fn int_class_keyword_name_single_row() {
    let mut p = setup("int class = 42;\n");
    p.parse_unit();
    assert_eq!(
        p.listener.errors,
        vec![("expected_identifier_but_got_keyword".to_owned(), 4, 9)],
        "reserved-keyword field name with `=` follower: {:?}",
        p.listener.errors
    );
}

/// The same face for every top-level-keyword-flavored reserved name
/// (`int var/void/const/final/enum = 42;` — oracle 3.13.1: exactly one
/// `expected_identifier_but_got_keyword` per line, keyword consumed).
#[test]
fn reserved_keyword_field_names_single_row_each() {
    for (kw, len) in [
        ("var", 3),
        ("void", 4),
        ("const", 5),
        ("final", 5),
        ("enum", 4),
    ] {
        let src = format!("int {kw} = 42;\n");
        let src: &'static str = Box::leak(src.into_boxed_str());
        let mut p = setup(src);
        p.parse_unit();
        assert_eq!(
            p.listener.errors,
            vec![("expected_identifier_but_got_keyword".to_owned(), 4, 4 + len)],
            "`int {kw} = 42;`: {:?}",
            p.listener.errors
        );
    }
}

/// `library enum;` accepts the keyword as the library name: the library
/// template's branch 2 keep-set is `.` `;` EOF
/// (`identifier_context_impl.dart:953`), and `enum ;`'s follower `;` is in
/// it, so the keyword is consumed with `expected_identifier_but_got_keyword`
/// (oracle 3.13.1, `issue_48371_prime1`: one row, len 4 on `enum`).
#[test]
fn library_enum_keyword_library_name() {
    let mut p = setup("library enum;\n");
    p.parse_unit();
    assert_eq!(
        p.listener.errors,
        vec![("expected_identifier_but_got_keyword".to_owned(), 8, 12)],
        "`library enum;` face: {:?}",
        p.listener.errors
    );
}

/// `var class VarClass {}` never eats the class keyword: the follower is
/// not a name continuation, so the synthetic-name insert reports
/// `missing_identifier` at the keyword, `ensureSemicolon` anchors on the
/// walked-back `var` head, and the class reparses clean (oracle 3.13.1:
/// `expected_token ';'` at the `var` + `missing_identifier` at `class`).
#[test]
fn var_class_missing_name_reparse() {
    let mut p = setup("var class VarClass {}\n");
    p.parse_unit();
    assert_eq!(
        p.listener.errors,
        vec![
            ("missing_identifier".to_owned(), 4, 9),
            ("expected_token".to_owned(), 0, 3),
        ],
        "`var class` reparse stream: {:?}",
        p.listener.errors
    );
}

/// `var typedef VarTypedef = void Function();`: `typedef` heads the next
/// declaration (`isTopLevelKeyword`), so the var-declaration name slot
/// takes the synthetic insert (`missing_identifier` at `typedef`), the
/// semicolon anchors on `var`, and the typedef reparses clean — the
/// type-less rider must NOT fire (the `var` modifier is present) and the
/// rest of the line carries no rows (oracle 3.13.1,
/// `blindspot_var_declarations` line 12).
#[test]
fn var_typedef_missing_name_reparse() {
    let mut p = setup("var typedef VarTypedef = void Function();\n");
    p.parse_unit();
    assert_eq!(
        p.listener.errors,
        vec![
            ("missing_identifier".to_owned(), 4, 11),
            ("expected_token".to_owned(), 0, 3),
        ],
        "`var typedef` reparse stream: {:?}",
        p.listener.errors
    );
}

// —— family: empty index `[]` (adjacent brackets split face) ——————————————

/// `b..a[];` recovers the missing index with ONE `missing_identifier`
/// anchored on the real `]` (the scanner's adjacent-brackets `[]` token
/// splits into `[` `]`, `rewriteSquareBrackets` :4743): no second
/// `expected_token ']'` row (oracle 3.13.1, `issue_42267` line 4).
#[test]
fn cascade_empty_index_single_row() {
    let mut p = setup("void f() { b..a[]; }\n");
    p.parse_unit();
    assert_eq!(
        p.listener.errors,
        vec![("missing_identifier".to_owned(), 16, 17)],
        "cascade `..a[]` face: {:?}",
        p.listener.errors
    );
}

/// The plain (non-cascade) empty index takes the same single-row face
/// (oracle 3.13.1, `issue_42267` lines 6/7: `missing_identifier` at the `]`,
/// never an `expected_token`).
#[test]
fn plain_empty_index_single_row() {
    let mut p = setup("void f() { a[]; }\n");
    p.parse_unit();
    assert_eq!(
        p.listener.errors,
        vec![("missing_identifier".to_owned(), 13, 14)],
        "plain `a[]` face: {:?}",
        p.listener.errors
    );
}

/// The spaced face `g[ ];` — the brackets are two REAL table tokens (no
/// adjacent `[]` to split), so the empty-operand path runs with the closer
/// being the real `]` itself (upstream Dart SDK source, pkg/_fe_analyzer_shared
/// parser_impl.dart `parseArgumentOrIndexStar` + `rewriteSquareBrackets`):
/// exactly ONE `missing_identifier` anchored on the `]`'s own span — which is
/// also the index-expression closer span, the report and the closer peeking
/// the same real token — and no second `expected_token ']'` row. The
/// exact-set pin doubles as the termination proof: the empty operand with a
/// real `]` previously spun forever here, so the synthetic operand must step
/// aside for the real `]` to close the index (a mis-splice would leave it to
/// resurface past the statement as extra rows on the `;`/`}` tail).
#[test]
fn spaced_empty_index_real_closer_single_row() {
    let mut p = setup("void f() { g[ ]; }\n");
    p.parse_unit();
    assert_eq!(
        p.listener.errors,
        vec![("missing_identifier".to_owned(), 14, 15)],
        "spaced `g[ ]` face — the real `]` closes the index: {:?}",
        p.listener.errors
    );
}

// —— family: named-only record type annotation in statement position ———————

/// `({int a}) t;` is a legal local declaration with a named-only record
/// type. The named-fields arm of `parse_record_type` must consume ONLY the
/// group's `)` (`ensureCloseParen` consumes it in every arm and returns its
/// span, cursor already past — the official named arm consumes
/// `token.next!` directly, parser_impl.dart:1747-1907); re-peeking after
/// that consume minted `end` from the FOLLOWING token and the trailing
/// advance swallowed the variable name, so the name slot ran on `;`
/// (`missing_identifier` + a synthetic empty name).
#[test]
fn named_only_record_type_local_parses_clean() {
    let mut p = setup("void main() { ({int a}) t; }\n");
    p.parse_unit();
    assert!(
        p.listener.errors.is_empty(),
        "named-only record type local must parse clean: {:?}",
        p.listener.errors
    );
    p.listener.assert_sequence(&[
        ("begin", "record_type"),
        ("end", "record_type"),
        ("begin", "variables_declaration"),
        ("begin", "initialized_identifier"),
        ("end", "initialized_identifier"),
        ("end", "variables_declaration"),
    ]);
}

/// The same consume shape with an initializer, and as a member return
/// type (the GFT record-return pass reuses the named arm).
#[test]
fn named_only_record_type_shapes_parse_clean() {
    for (source, label) in [
        (
            "void main() { ({int a}) t = (a: 1); }\n",
            "with initializer",
        ),
        ("({int a}) f() { throw ''; }\n", "member return type"),
    ] {
        let mut p = setup(source);
        p.parse_unit();
        assert!(
            p.listener.errors.is_empty(),
            "{label}: {:?}",
            p.listener.errors
        );
    }
}

// —— family: stack_overflow expression-depth fuse tail ————————————————————

/// The expression-depth fuse is a LOCAL recovery (official `parseExpression`
/// `parser_impl.dart:6936-6963`): report once, skip to the innermost region's
/// scanner-paired group end, splice a synthetic identifier, and return to
/// normal parsing. The enclosing paren frames then unwind through their
/// closers by kind (mis-adopting one per the official no-identity check), so
/// for `var x = (((…1…)))` at 600 deep the oracle tail is: `stack_overflow`
/// on the fused `(`, `expected_token ';'` anchored on the last adopted
/// closer (ensureSemicolon's walked-back anchor), `expected_executable` on
/// the leftover `)` (top-level invalid declaration), `unexpected_token` on
/// the final `;` (oracle 3.13.1 `stack_overflow` line 2, cols 510/1208/1209/
/// 1210 — the fuse trips on the entry past the cap, `depth++ > 500`).
#[test]
fn stack_overflow_fuse_tail_normal_unwind() {
    let src = format!("var x = {}1{};\n", "(".repeat(600), ")".repeat(600));
    let src: &'static str = Box::leak(src.into_boxed_str());
    let mut p = setup(src);
    p.parse_unit();
    assert_eq!(
        p.listener.errors,
        vec![
            ("stack_overflow".to_owned(), 509, 510),
            ("expected_token".to_owned(), 1207, 1208),
            ("expected_executable".to_owned(), 1208, 1209),
            ("unexpected_token".to_owned(), 1209, 1210),
        ],
        "fuse tail unwind: {:?}",
        p.listener.errors
    );
}

/// Just under the cap nothing reports: the fuse must not fire early.
#[test]
fn stack_overflow_below_cap_clean() {
    let src = format!("var x = {}1{};\n", "(".repeat(499), ")".repeat(499));
    let src: &'static str = Box::leak(src.into_boxed_str());
    let mut p = setup(src);
    p.parse_unit();
    assert!(
        p.listener.errors.is_empty(),
        "499-deep parens must stay silent: {:?}",
        p.listener.errors
    );
}

// —— family: EOF-face zero-width artifact rows (parked scanner closers) —————

/// Scans + parses `source` (latest features) and returns the MERGED
/// diagnostic rows — scanner channel first, then parser rows as
/// `(code, start, end)` — for fixtures whose unmatched begins legitimately
/// park synthetic closers (`gate_m0_wave3.rs::assert_codes_with_scanner`
/// rig family, with spans so the zero-width faces stay observable).
fn merged_rows_with_scanner(source: &'static str) -> Vec<(String, u32, u32)> {
    let config = dartq_scanner::ScanConfig {
        enable_triple_shift: true,
        enable_augmentations: true,
    };
    let mut files = dartq_source::FileTable::new();
    let file = files.intern(std::path::Path::new("test.dart"));
    let result = dartq_scanner::scan(source, config, file);
    let table: &'static dartq_scanner::TokenTable = Box::leak(Box::new(result.tokens));
    let text: &'static str = Box::leak(source.to_string().into_boxed_str());
    let mut rows: Vec<(String, u32, u32)> = result
        .diagnostics
        .iter()
        .map(|d| {
            (
                d.code.name.to_owned(),
                d.range.offset,
                d.range.offset + d.range.length,
            )
        })
        .collect();
    let mut p = super::collector::setup_from_parts(text, table);
    p.parse_unit();
    rows.extend(p.listener.errors.clone());
    rows
}

/// `new C(;` — the unclosed argument `(` parks a zero-width synthetic `)`
/// in front of the function's `}` (the scanner records the pair without
/// rows). `ensureCloseParen`'s moveSynthetic arm relocates the closer onto
/// the `;` and reports the pair THERE (the official translation's row),
/// behind the argument head's `missing_identifier` on the same token — the
/// oracle's `bracket_mismatch_06` face, no zero-length row anywhere.
#[test]
fn parked_closer_adoption_reports_bracket_mismatch_06() {
    let rows = merged_rows_with_scanner("main() {\n  new C(;\n}\n");
    assert_eq!(
        rows,
        vec![
            ("missing_identifier".to_owned(), 17, 18),
            ("expected_token".to_owned(), 17, 18),
        ],
        "bracket_mismatch_06 merged rows: {rows:?}"
    );
}

/// `case dynamic(foo: int():` — the unclosed object-pattern `(` parks a
/// zero-width synthetic `)` in front of the switch's `}` (the scanner
/// records the pair without rows). The object pattern's missing-`)`
/// recovery adopts the scanner's synthetic (official `ensureCloseParen`
/// moveSynthetic arm) and reports the pair at the relocation anchor — the
/// oracle's single `expected_token` row. The anchor follows the pattern
/// lane's recovery posture (position-posture note in
/// `recovery.rs::pin_object_recovery`), so this pin holds the count.
#[test]
fn parked_closer_adoption_reports_object_recovery() {
    let rows = merged_rows_with_scanner(
        "void f(x) {\n  switch (x) {\n    case dynamic(foo: int():\n      break;\n  }\n}\n",
    );
    assert_eq!(rows.len(), 1, "one adoption row: {rows:?}");
    assert_eq!(
        rows[0].0, "expected_token",
        "object_recovery_missingClosingParen merged rows: {rows:?}"
    );
}

// —— family: issue_39060 recovered type-argument tail (parseUnexpectedEnd) —

/// `{s A<}>` — the var declaration `s A` ends at `<` (`expected_token ';'`
/// anchored on the name, oracle 3.13.1 col 6); the recovered type-argument
/// group `< } >` re-enters as a list-literal head, and the official
/// `parseUnexpectedEnd` tail absorbs the junk `}` silently (`parseCloser`
/// arm — the junk's follower is the real `>`), closing the group there.
/// The list face then anchors `expected_token '['` on the block's `}` at
/// line 3, and the statement's semicolon face anchors back on the `>` —
/// exactly the oracle's four parser rows, no `missing_identifier` cascade.
#[test]
fn issue_39060_type_args_absorb_junk_tail() {
    let mut p = setup("main() {\n  {s A<}>\n}\n");
    p.parse_unit();
    assert_eq!(
        p.listener.errors,
        vec![
            ("expected_token".to_owned(), 14, 15),
            ("expected_type_name".to_owned(), 16, 17),
            ("expected_token".to_owned(), 19, 20),
            ("expected_token".to_owned(), 17, 18),
        ],
        "issue_39060 stream: {:?}",
        p.listener.errors
    );
}

/// A well-formed type-argument group never reaches the recovered tail.
#[test]
fn well_formed_type_arguments_stay_clean() {
    let mut p = setup("void f() { var x = <int>[1]; }\n");
    p.parse_unit();
    assert!(
        p.listener.errors.is_empty(),
        "typed list literal must stay silent: {:?}",
        p.listener.errors
    );
}

/// `case <int>(b) x:` — re-derived to the official walk (the pre-#1487
/// model — one unexpected-end row on the type end via the
/// `parseUnexpectedEnd` OPEN_PAREN arm — encoded the old pattern-head
/// type-args posture and is refuted by the official decision order):
/// `parsePrimaryPattern` parses the `<...>` group as type arguments only
/// when the follower is `[`/`{` (upstream Dart SDK source,
/// parser_impl.dart:11697-11734); a `(` follower backs up, and the head
/// `<` — a relational operator — takes the relational-pattern arm
/// (:11805-11815). The type-args unexpected-end face (and its OPEN_PAREN
/// latch, type_info_impl.dart:1692-1706) never runs for this snippet; the
/// relational face is silent. The three official anchors, all reproduced:
/// the case colon face anchors `expected_token` on the `>` (ensureColon →
/// rewriteAndRecover reports at `token.next`, parser_impl.dart:5053-5060,
/// :5112-5117); the junk statement `> (b)` reports `missing_identifier`
/// at the same `>` (parseSend's ensureIdentifier reached through
/// parsePrimary's recovery tail, parser_impl.dart:8239-8241, :9399) and
/// absorbs `(b)` silently; the missing-`;` face anchors on the `)`
/// (ensureSemicolon's `findPreviousNonZeroLengthToken`,
/// parser_impl.dart:5093-5109). The trailing `x: break;` parses as a
/// labeled statement with no rows.
#[test]
fn unexpected_end_open_paren_arm_reports_once() {
    let mut p = setup("void f(x) {\n  switch (x) {\n    case <int>(b) x:\n      break;\n  }\n}\n");
    p.parse_unit();
    assert_eq!(
        p.listener
            .errors
            .iter()
            .filter(|(_, s, e)| *s == 37 && *e == 40)
            .count(),
        0,
        "no row anchored on the type end `int` (the relational face is silent): {:?}",
        p.listener.errors
    );
    assert_eq!(
        p.listener
            .errors
            .iter()
            .filter(|(_, s, e)| *s == 40 && *e == 41)
            .count(),
        2,
        "exactly the colon face and the primary missing-identifier row on the `>`: {:?}",
        p.listener.errors
    );
    assert_eq!(
        p.listener
            .errors
            .iter()
            .filter(|(_, s, e)| *s == 43 && *e == 44)
            .count(),
        1,
        "exactly the missing-`;` row anchored on the `)`: {:?}",
        p.listener.errors
    );
    assert!(
        !p.listener
            .errors
            .iter()
            .any(|(_, s, e)| *s == 42 && *e == 43),
        "no row on the skipped `b`: {:?}",
        p.listener.errors
    );
}

// —— family: #2165 app-gate recovery singles ———————————————————————————————

/// A list literal whose closer the scanner parked at EOF: the official
/// `parseLiteralListSuffix` recovery (`parser_impl.dart:8536-8541`) moves the
/// synthetic `]` in front of the stray token (`moveSynthetic`), so the
/// unmatched-group row lands at the `;`, not at the end of the source
/// (oracle 3.13.1: `var v = [1, 2, 3;` → `expected_token ']'` 1:17).
#[test]
fn list_literal_adopts_parked_closer_at_stray_token() {
    let mut p = setup("var v = [1, 2, 3;\n");
    p.parse_unit();
    assert_eq!(
        p.listener.errors,
        vec![("expected_token".to_owned(), 16, 17)],
        "{:?}",
        p.listener.errors
    );
}

/// An unclosed type-argument group in a declaration head: the official
/// `ComplexTypeParamOrArgInfo.compute` recovery tail
/// (`type_info_impl.dart:1408-1423`) ends the group on a synthetic `>`
/// before the junk follower, so the name stays out of the type and the
/// declaration keeps its parameters and body — one `expected_token`
/// (`Expected '>' after this`) row on the last argument (oracle 3.13.1, top level and member).
#[test]
fn unclosed_type_args_in_declaration_head_end_before_name() {
    for (source, at) in [
        ("Map<String, int m0() => {};\n", (12, 15)),
        ("class C { Map<String, int m0() => {}; }\n", (22, 25)),
    ] {
        let mut p = setup(source);
        p.parse_unit();
        assert_eq!(
            p.listener.errors,
            vec![("expected_token".to_owned(), at.0, at.1)],
            "{source}: {:?}",
            p.listener.errors
        );
    }
}

/// The same shape in a block is not a type-argument group at all: the
/// statement path computes the type with `inDeclaration` false
/// (`parser_impl.dart:9923`, `looksLikeLocalFunction` :9742), and outside a
/// declaration a member followed by neither `,` nor a closer yields
/// `noTypeParamOrArg` (`type_info_impl.dart:1395-1397`) — `Map < String`
/// then reads as a relational expression (oracle 3.13.1: `expected_token`
/// on `String`, then `missing_identifier` + `unexpected_token` on `,`).
#[test]
fn unclosed_type_args_in_block_read_as_expressions() {
    let mut p = setup("void f() { Map<String, int m0() => {}; }\n");
    p.parse_unit();
    let got = &p.listener.errors;
    for want in [
        ("expected_token".to_owned(), 15, 21),
        ("missing_identifier".to_owned(), 21, 22),
        ("unexpected_token".to_owned(), 21, 22),
    ] {
        assert!(got.contains(&want), "missing {want:?}: {got:?}");
    }
    assert!(
        !got.iter()
            .any(|(code, _, _)| code == "missing_function_body"
                || code == "missing_function_parameters"),
        "no declaration-shaped rows: {got:?}"
    );
}

/// The nesting fuse trips on the entry PAST the cap (`depth++ > 500`,
/// `parser_impl.dart:6939`): 501 nested list literals parse, the 502nd
/// opener reports (oracle 3.13.1 `final x = [[…]]` at 505 deep →
/// `stack_overflow` at col 512, the 502nd `[`, then the normal-unwind tail
/// at cols 1020/1021/1022).
#[test]
fn nesting_fuse_trips_past_the_cap() {
    let src = format!("final x = {}{};\n", "[".repeat(505), "]".repeat(505));
    let src: &'static str = Box::leak(src.into_boxed_str());
    let mut p = setup(src);
    p.parse_unit();
    assert_eq!(
        p.listener.errors,
        vec![
            ("stack_overflow".to_owned(), 511, 512),
            ("expected_token".to_owned(), 1018, 1019),
            ("expected_executable".to_owned(), 1019, 1020),
            ("unexpected_token".to_owned(), 1020, 1021),
        ],
        "{:?}",
        p.listener.errors
    );
}
