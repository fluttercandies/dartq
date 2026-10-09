//! M0-gate wave-1 regression pins (#332): the six dartq-only diagnostic
//! families were parser recovery decision-order divergences from the
//! official parser, each cascading one divergence into dozens of downstream
//! misdiagnoses. Every pin here mirrors an official recovery template
//! (`identifier_context_impl.dart`, `type_info_impl.dart`,
//! `parser_impl.dart`) or an official `.parser.expect` multiset from
//! `pkg/front_end/parser_testcases/error_recovery/`:
//!
//! * `expected_token` / `missing_identifier` from keyword-named names: the
//!   official context templates report
//!   `expectedIdentifierButGotKeyword` (analyzer code
//!   `expected_identifier_but_got_keyword`) and *consume* the keyword as
//!   the name, so the parse continues cleanly (symbol literals,
//!   formal parameters, fields, top-level members, class names).
//! * `missing_const_final_var_or_type`: the reserved-name member lanes
//!   (`parseTopLevelMemberImpl` :4304, member impl :5651) recompute the
//!   return type as required, so `int assert = 42;` is type + keyword name.
//! * `missing_method/function_parameters` + `missing_function_body` from
//!   operator members: official `parseMethod` keeps the cursor on the
//!   operator name across the return-type re-parse.
//! * `record_type_one_positional_no_trailing_comma` on parenthesized
//!   expressions: official `computeRecordType`'s `!required` follower
//!   validation (type_info_impl.dart:771-860) keeps `(v = 0) ?? 0;` an
//!   expression, and the formal inline-function-type check never fires
//!   when a type was committed (`parser_impl.dart:2221-2264`), so
//!   `f((Enum, Enum) r)` keeps its record-type parameter.

use super::collector::setup;

/// Sorted diagnostic-code multiset of one whole-unit parse.
fn sorted_codes(source: &'static str) -> Vec<String> {
    let mut p = setup(source);
    p.parse_unit();
    let mut codes = p.listener.error_codes();
    codes.sort();
    codes
}

/// Asserts the exact diagnostic multiset (codes only, sorted).
fn assert_codes(source: &'static str, expected: &[&str], label: &str) {
    let got = sorted_codes(source);
    let expected: Vec<String> = {
        let mut e = expected.iter().map(ToString::to_string).collect::<Vec<_>>();
        e.sort();
        e
    };
    assert_eq!(got, expected, "{label}: diagnostic multiset diverged");
}

const KW: &str = "expected_identifier_but_got_keyword";

/// Family 1 (symbols.dart shape): a reserved word after `#` (or after the
/// dot) is reported once and consumed as the identifier —
/// `LiteralSymbolIdentifierContext.ensureIdentifier`
/// (identifier_context_impl.dart:748-776); `#void` is the official
/// `handleSymbolVoid` special case and stays silent.
#[test]
fn keyword_symbol_literals_report_once_and_consume() {
    assert_codes(
        "main() {\n  #void;\n  #void.foo;\n  #foo.void;\n  #break;\n  #foo.assert;\n  #assert.foo;\n}\n",
        &[KW, KW, KW, KW],
        "symbol literals",
    );
}

/// Family 2 (`keyword_named_formal_parameter.dart`): every reserved word as a
/// formal parameter name reports exactly the keyword recovery, in all six
/// parameter forms (official :15-20 report one error each; pseudo keywords
/// stay silent).
#[test]
fn keyword_named_formal_parameters_report_once_and_consume() {
    assert_codes(
        "void test(int assert) {}\nvoid test([int break = 42]) {}\n\
         void test({int class: 3}) {}\nvoid test({int const = 3}) {}\n\
         void test([int do]) {}\nvoid test(int while) {}\n",
        &[KW; 6],
        "reserved formal parameters",
    );
    assert_codes(
        "void test(int abstract) {}\nvoid test({int dynamic: 1}) {}\n",
        &[],
        "built-in formal parameters stay silent",
    );
}

/// Families 3+5 (`keyword_named_class_fields.dart` /
/// `keyword_named_top_level_fields.dart`): `TYPE <reserved word> = ...` is
/// type + keyword name — the member lanes recompute the type as required
/// (official :5651-5668 and :4304-4320) instead of reporting
/// `missing_const_final_var_or_type`.
#[test]
fn keyword_named_fields_report_once_and_consume() {
    assert_codes(
        "class C {\n  int assert = 42;\n  int break = 42;\n  int case = 42;\n}\n",
        &[KW, KW, KW],
        "class fields",
    );
    assert_codes(
        "int assert = 42;\nint while;\n",
        &[KW, KW],
        "top-level fields",
    );
}

/// Family 6 top-level methods (`keyword_named_top_level_methods.dart`
/// head shape): a reserved method name reports the keyword recovery and
/// the body parses.
#[test]
fn keyword_named_top_level_methods_report_once_and_consume() {
    assert_codes(
        "int assert(int x) {\n  if (x == 0) return 42;\n  return assert(x-1) + 1;\n}\n",
        &[KW],
        "reserved top-level method name",
    );
}

/// Family 3b (`issue_46346.dart`): class names — reserved words report the
/// keyword recovery, built-in identifiers report
/// `built_in_identifier_in_declaration`, and both are consumed as the name
/// (`ClassOrMixinOrExtensionIdentifierContext`,
/// identifier_context_impl.dart:50-118).
#[test]
fn keyword_named_classes_report_and_consume() {
    assert_codes(
        "class assert {}\nclass abstract {}\nclass as {}\nclass pseudo {}\n",
        &[
            KW,
            "built_in_identifier_in_declaration",
            "built_in_identifier_in_declaration",
        ],
        "class names",
    );
    assert_codes(
        "class assert = A with B;\nclass abstract = A with B;\n",
        &[KW, "built_in_identifier_in_declaration"],
        "named mixin applications",
    );
}

/// Family 4a: a parenthesized assignment stays an expression —
/// `computeRecordType`'s optional-shape veto (type_info_impl.dart:771-860)
/// rejects the group because no declaration name follows.
#[test]
fn parenthesized_expression_is_not_a_record_type() {
    assert_codes(
        "main() {\n  (v = 0) ?? 0;\n}\n",
        &[],
        "parenthesized assignment",
    );
}

/// Family 4b: `(as) = x` is a pattern assignment whose variable is the
/// built-in `as`; the official parser reports only
/// `illegalPatternAssignmentVariableName` (parser_impl.dart:11971-11979).
#[test]
fn pattern_assignment_named_as_official() {
    assert_codes(
        "main() {\n  (as) = x;\n}\n",
        &["illegal_pattern_assignment_variable_name"],
        "pattern assignment",
    );
}

/// Family 4c: record-type parameters — the formal inline-function-type
/// check only fires with no committed type (`parser_impl.dart:2221-2264`),
/// so `((Enum, Enum) r)` keeps its parameter shape.
#[test]
fn record_type_parameter_stays_a_parameter() {
    assert_codes(
        "f((Enum, Enum) r) => switch (r) {\n  (Enum(), Enum.a) => 0,\n};\nenum Enum { a }\n",
        &[],
        "record-type parameter + switch expression",
    );
}

/// Family 6 (`null_aware_access.dart`): operator members without return-type
/// parentheses — official `parseMethod` keeps the cursor on the operator
/// name across the return-type re-parse, so untyped parameter lists parse
/// clean.
#[test]
fn operator_members_parse_clean() {
    assert_codes(
        "class C {\n  void set setter(value) {}\n  C operator [](index) => this;\n\
         void operator []=(index, value) {}\n  C operator +(other) => this;\n}\n",
        &[],
        "operator members",
    );
}
