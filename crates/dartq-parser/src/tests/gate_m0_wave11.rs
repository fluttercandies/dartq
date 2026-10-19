//! M0-gate wave-11 regression pins (the w4 trend long tail): the legacy
//! switch case expression, the `var/final/const`-vs-`late` junk-name split,
//! the primary-constructor `covariant` formal gate, and the missing-class-
//! body junk re-expansion.
//!
//! Every face was re-derived against the 3.13.1 oracle
//! (`dart analyze --format=machine` probes) before pinning; the w4 trend
//! rows for several of these files predate intermediate parser work, so the
//! pins lock the CURRENT traced behavior, not the stale captures.

use super::collector::{scan_ok, setup};
use crate::ParserFeatures;
use dartq_language::{FeatureSet, LanguageVersion};

/// Parses `source` with the latest feature set and returns the sorted
/// diagnostic code multiset.
fn sorted_codes(source: &'static str) -> Vec<String> {
    let mut parser = setup(source);
    parser.parse_unit();
    let mut codes = parser.listener.error_codes();
    codes.sort();
    codes
}

/// 2.19 feature set (patterns off — the legacy statement grammar).
fn legacy_features() -> ParserFeatures {
    ParserFeatures::from_feature_set(FeatureSet::for_language_version(LanguageVersion::new(
        2, 19,
    )))
}

fn assert_codes(source: &'static str, expected: &[&str], label: &str) {
    let got = sorted_codes(source);
    let expected: Vec<String> = expected.iter().map(ToString::to_string).collect();
    assert_eq!(got, expected, "{label}: diagnostic multiset diverged");
}

// —— legacy switch case (patterns off) ————————————————————————————————————

/// Under a 2.19 language version the case head parses as a plain
/// expression — the branch sits in `parseSwitchBlock`
/// (parser_impl.dart:10945-10949), not inside `parsePattern`, so the
/// disabled-patterns face never fires and `case 1:` closes silently.
/// The whole file is diagnostic-free (the oracle agrees, zero rows on
/// the `assigned_variables/data/switch.dart` fixture).
#[test]
fn legacy_switch_case_head_parses_as_expression() {
    let source = "f(int a) {\n  switch (a = 0) {\n    case 1:\n      break;\n    default:\n      break;\n  }\n}\n";
    let (table, text) = scan_ok(source);
    let mut collector = super::collector::Collector::default();
    let mut parser = crate::Parser::new(text, table, legacy_features(), &mut collector);
    parser.parse_unit();
    assert!(
        parser.listener.errors.is_empty(),
        "legacy switch case must parse clean: {:?}",
        parser.listener.errors
    );
}

// —— junk member names: the var/final/const split —————————————————————————

/// `late ;` — the built-in `late` is not a modifier before `;` (official
/// `isModifier`, modifier_context.dart:13-53), so it stays in the name slot
/// and the field is declared NAMED `late` (no synthetic insert; the oracle
/// traces `late;` through the `!next.isIdentifier` fall-through,
/// parser_impl.dart:4356). `parseFields`'s type face is the only diagnostic
/// (probed 3.13.1: one row, `missingConstFinalVarOrType` anchored on
/// `late`).
#[test]
fn late_semicolon_reports_only_missing_type() {
    assert_codes(
        "late;\n",
        &["missing_const_final_var_or_type"],
        "top-level `late ;`",
    );
    assert_codes(
        "class C {\n  late;\n}\n",
        &["missing_const_final_var_or_type"],
        "instance `late ;`",
    );
}

/// `var ;` — with a `var`/`final`/`const` modifier the inserted-name face
/// IS reported (probed 3.13.1: `missing_identifier` on the `;`), and the
/// modifier suppresses `parseFields`'s type face.
#[test]
fn var_semicolon_reports_missing_identifier() {
    assert_codes("var;\n", &["missing_identifier"], "top-level `var ;`");
    assert_codes(
        "class C {\n  var;\n}\n",
        &["missing_identifier"],
        "instance `var ;`",
    );
}

/// `int ;` — a consumed type with a junk name: silent name insert, and the
/// `NoType` re-derive surfaces `missingConstFinalVarOrType` on the type
/// (probed 3.13.1: one row anchored on `int`).
#[test]
fn type_semicolon_reports_only_missing_type() {
    assert_codes(
        "int;\n",
        &["missing_const_final_var_or_type"],
        "top-level `int ;`",
    );
    assert_codes(
        "class C {\n  int;\n}\n",
        &["missing_const_final_var_or_type"],
        "instance `int ;`",
    );
}

// —— primary-constructor `covariant` formal —————————————————————————————————

/// `class P(covariant int x) {}` — with the feature on, `covariant` in a
/// primary-constructor formal is only legal alongside `var`
/// (modifier_context.dart:238-246): one
/// `invalid_covariant_modifier_in_primary_constructor` row on the
/// `covariant` token.
#[test]
fn covariant_primary_constructor_formal_reports() {
    assert_codes(
        "class P(covariant int x) {}\n",
        &["invalid_covariant_modifier_in_primary_constructor"],
        "covariant primary formal",
    );
}

// —— missing class body: junk re-expansion from headerStart ————————————————

/// `class C({var int a})` at EOF — the params parse succeeds, but the
/// missing-body recovery restarts from `headerStart`
/// (`parser_impl.dart:3098)`: the `(` ahead of the body `{` reports as
/// skipped junk (`unexpected_token`), `{var int a}` re-forms the class
/// body (the member re-reports `var_and_type` and walks the `;` back to
/// `a`), and the leftover `)` lands as top-level junk
/// (`expected_executable`). The probed oracle face (3.13.1, feature on,
/// re-probed 2026-10-12) is exactly these four codes — `var_and_type`
/// ONCE, from the junk member: the declaring-parameter parse itself stays
/// silent under the `PrimaryConstructor` exemption (`parser_impl.dart:2283`-
/// 2298; the earlier twice-face probe predates the feature-on oracle).
#[test]
fn missing_class_body_reexpands_primary_params_as_junk() {
    assert_codes(
        "class C({var int a})\n",
        &[
            "expected_executable",
            "expected_token",
            "unexpected_token",
            "var_and_type",
        ],
        "primary params at EOF re-expand as junk",
    );
}

// —— #2181 union residual: `late;` declares a field named `late` ——————————

/// The recovered top-level field carries the `late` token as its NAME event
/// (official `!next.isIdentifier` fall-through, parser_impl.dart:4356): the
/// declared variable is `late`, so a later bare `late` read resolves (the
/// m0 union-gate dartq_only `undefined_identifier` at
/// use_late_in_non_nnbd.dart 19:3 was a synthetic-named recovery — the read
/// had nothing to bind to). Offsets: `late` 0..4, `x` 10..11.
#[test]
fn late_semicolon_top_level_field_named_late() {
    let mut p = setup("late;\nint x;\n");
    p.parse_unit();
    let c = &mut p.listener;
    assert_eq!(
        std::mem::take(&mut c.errors),
        vec![("missing_const_final_var_or_type".to_owned(), 0, 4)],
        "diagnostic stream must stay the single type-less rider: {:?}",
        c.errors
    );
    let named = |ctx: &str, start: u32, end: u32| {
        c.idents
            .iter()
            .any(|(k, s, e, _)| k == ctx && *s == start && *e == end)
    };
    assert!(
        named("TopLevelVariableDeclaration", 0, 4),
        "top-level field name event must carry the `late` token: {:?}",
        c.idents
    );
    assert!(
        named("TopLevelVariableDeclaration", 10, 11),
        "the following `int x;` must still declare `x`: {:?}",
        c.idents
    );
    // No synthetic name was spliced at the `;` (offset 4, zero-width).
    assert!(
        c.idents.iter().all(|(_, s, e, _)| !(*s == 4 && *e == 4)),
        "no synthetic identifier at the `;`: {:?}",
        c.idents
    );
}

/// The class-member flavor: the field inside `class C` is likewise named
/// `late` (FieldDeclaration name event on the keyword, 12..16).
#[test]
fn late_semicolon_class_member_field_named_late() {
    let mut p = setup("class C {\n  late;\n}\n");
    p.parse_unit();
    let c = &mut p.listener;
    assert_eq!(
        std::mem::take(&mut c.errors),
        vec![("missing_const_final_var_or_type".to_owned(), 12, 16)],
        "class-member stream must stay the single type-less rider: {:?}",
        c.errors
    );
    assert!(
        c.idents
            .iter()
            .any(|(k, s, e, _)| k == "FieldDeclaration" && *s == 12 && *e == 16),
        "member field name event must carry the `late` token: {:?}",
        c.idents
    );
}

/// The statement flavor stays an expression statement reading the built-in
/// `late` as a plain identifier (official trace: `isModifier` false before
/// `;` → `token == start` → `parseExpressionStatement`,
/// parser_impl.dart:10019-10026) — with the top-level field named `late`
/// declared above, this read resolves and the union-gate dartq_only row
/// disappears. The only row is the top-level field's type-less rider (the
/// oracle emits the same row on its `late;` line); the statement `late` is
/// an `Expression` ident on the keyword (22..26).
#[test]
fn late_semicolon_statement_reads_identifier_late() {
    let mut p = setup("late;\nvoid main() {\n  late;\n}\n");
    p.parse_unit();
    let c = &mut p.listener;
    assert_eq!(
        std::mem::take(&mut c.errors),
        vec![("missing_const_final_var_or_type".to_owned(), 0, 4)],
        "only the top-level `late;` rider may fire: {:?}",
        c.errors
    );
    assert!(
        c.idents
            .iter()
            .any(|(k, s, e, _)| k == "Expression" && *s == 22 && *e == 26),
        "the statement `late` must be an identifier read: {:?}",
        c.idents
    );
}

// —— #2181 union residual: method named `operator` —————————————————————————

/// `int operator(int x)` with a body calling it (the
/// keyword_named_class_methods_2.dart 227/229 shape): `(` is not a
/// user-definable operator, so the official `operator` branch falls through
/// and `operator` is a PLAIN method name (parser_impl.dart:5490-5558). The
/// parser must declare the method under that name (MethodDeclaration ident)
/// and emit the call-site `operator(x-1)` as an identically named send —
/// the m0 union-gate dartq_only `undefined_method` row is a consumer-side
/// lookup defect iff this stream holds.
#[test]
fn method_named_operator_declares_and_calls_by_name() {
    let src: &'static str = "class WrapperClass {\n  int operator(int x) {\n    if (x == 0) return 42;\n    return operator(x-1) + 1;\n  }\n}\n";
    let decl = src.find("operator").unwrap() as u32;
    let call = src[decl as usize + 8..].find("operator").unwrap() as u32 + decl + 8;
    let mut p = setup(src);
    p.parse_unit();
    let c = &mut p.listener;
    assert!(
        c.errors.is_empty(),
        "oracle stream for the operator-named member is empty: {:?}",
        c.errors
    );
    assert!(
        c.idents
            .iter()
            .any(|(k, s, e, _)| k == "MethodDeclaration" && *s == decl && *e == decl + 8),
        "method name event must carry the `operator` token: {:?}",
        c.idents
    );
    assert!(
        c.idents
            .iter()
            .any(|(k, s, e, _)| k == "Expression" && *s == call && *e == call + 8),
        "the call site must read `operator` by name: {:?}",
        c.idents
    );
    assert!(
        c.log.contains(&("begin", "method")) && c.log.contains(&("end", "method")),
        "the member must close as a method: {:?}",
        c.log
    );
}

// —— #2181 torture residual: type args stay anchored at their receiver —————

/// `Report(List<String>.of(rows))` (the torture builders.dart 24 shape): the
/// `<String>` group belongs to the argument's `List` head, never to the
/// outer `Report(` call. Canonical parser event order — the type-arguments
/// group opens AFTER the `List` identifier and closes BEFORE the `.of`
/// identifier — pins the anchoring at the parser tier; the torture-gate
/// dartq_only `wrong_number_of_type_arguments` on `Report` is a
/// consumer-side attribution defect iff this order holds.
#[test]
fn generic_argument_type_args_stay_anchored_at_list_head() {
    let src = "void f() { Report(List<String>.of(rows)); }\n";
    let list_at = src.find("List").unwrap() as u32;
    let of_at = src.find(".of(").unwrap() as u32 + 1;
    let mut p = setup(src);
    p.parse_unit();
    let c = &mut p.listener;
    assert!(
        c.errors.is_empty(),
        "the builders.dart call shape parses clean: {:?}",
        c.errors
    );
    let ident_at = |off: u32| {
        c.idents
            .iter()
            .find(|(_, s, _, _)| *s == off)
            .map(|(_, _, _, idx)| *idx)
    };
    let list_idx = ident_at(list_at);
    let of_idx = ident_at(of_at);
    let (Some(list_idx), Some(of_idx)) = (list_idx, of_idx) else {
        panic!("argument identifiers must fire: {:?}", c.idents);
    };
    let bta = c.log.iter().position(|l| *l == ("begin", "type_arguments"));
    let eta = c.log.iter().position(|l| *l == ("end", "type_arguments"));
    let (Some(bta), Some(eta)) = (bta, eta) else {
        panic!("the `<String>` group must fire type-argument events: {:?}", c.log);
    };
    // The ident's log index is the position the NEXT log entry takes, so
    // the group opening right after the `List` ident lands AT list_idx.
    assert!(
        bta >= list_idx && eta < of_idx,
        "type-args group must nest between the `List` and `of` idents \
         (List idx {list_idx}, begin {bta}, end {eta}, of idx {of_idx}): {:?}",
        c.log
    );
}
