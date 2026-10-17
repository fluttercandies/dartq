//! Face F-P parser-half pins (recon `trend-w5B` residual bucket): the
//! dartq-only over-fire rows in the F-P micro-shape fixtures are
//! resolution-side (diagnose.rs) on the current tree — every shape's
//! parser event sequence already matches the official recovery traces
//! (static-traced against `pkg/front_end/parser_testcases` in the upstream
//! Dart SDK source). These pins freeze that parse-parity boundary so the
//! semantic fixes retire the rows without a parser regression silently
//! re-firing them: a pin failure here means the shape is parser-home after
//! all and the fix belongs in the recovery code.

use dartq_scanner::{ScanConfig, TokenTable};
use dartq_source::FileTable;

use super::collector::{setup, setup_from_parts};

/// Sorted code multiset of one whole-unit parse (scanner-clean inputs,
/// mirroring the corpus gate's oracle version 3.13 faces; none of these
/// shapes moves between 3.13 and the test default 3.15).
fn sorted_unit_codes(source: &'static str) -> Vec<String> {
    let mut p = setup(source);
    p.parse_unit();
    let mut codes = p.listener.error_codes();
    codes.sort();
    codes
}

/// Sorted parser code multiset for an input whose scanner legitimately
/// reports: asserts the scanner row list first, then parses the scanned
/// table and returns the parser-side codes only.
fn sorted_unit_codes_with_scanner(source: &'static str, scanner_codes: &[&str]) -> Vec<String> {
    let config = ScanConfig {
        enable_triple_shift: true,
        enable_augmentations: true,
    };
    let mut files = FileTable::new();
    let file = files.intern(std::path::Path::new("test.dart"));
    let result = dartq_scanner::scan(source, config, file);
    let mut got: Vec<&str> = result.diagnostics.iter().map(|d| d.code.name).collect();
    got.sort();
    assert_eq!(got, scanner_codes, "scanner rows in {source:?}");
    let table: &'static TokenTable = Box::leak(Box::new(result.tokens));
    let text: &'static str = Box::leak(source.to_string().into_boxed_str());
    let mut p = setup_from_parts(text, table);
    p.parse_unit();
    let mut codes = p.listener.error_codes();
    codes.sort();
    codes
}

// —— `late` without var/type (nnbd/error_recovery/late_without_var_etc) ——

/// Whole-unit pin for `late x;` shapes at every declaration depth (upstream
/// intertwined expect: five `MissingConstFinalVarOrType` reports and nothing
/// else — the recovered `x`/`y`/`z` become declared no-type variables, so
/// the resolver can never see them as references; the old run's
/// `undefined_identifier` rows were resolution-side over-fire on top of
/// this exact face).
#[test]
fn late_without_var_declares_the_name_at_every_depth() {
    let source = "// https://github.com/dart-lang/sdk/issues/43811
// https://github.com/dart-lang/sdk/issues/43812
// https://github.com/dart-lang/sdk/issues/43813

void main() {
  late x;
}

late y;

class Foo {
  late z;

  void foo() {
    late x;
  }

  static void bar() {
    late x;
  }
}
";
    assert_eq!(
        sorted_unit_codes(source),
        ["missing_const_final_var_or_type"; 5],
        "late x; at local/top-level/member depths"
    );
}

/// Statement-lane single shape (the trace's `looksLikeLocalFunction(x)` →
/// `MissingConstFinalVarOrType` at `x` → `handleNoType(late)` →
/// `beginVariablesDeclaration(x, late, null)` path).
#[test]
fn statement_late_name_reports_missing_type_once() {
    assert_eq!(
        sorted_unit_codes("void f() {\n  late x;\n}\n"),
        ["missing_const_final_var_or_type"],
        "`late x;` in a block"
    );
}

// —— also-nnbd/use_late_in_non_nnbd ——————————————————————————————————————

/// Whole-unit pin: five `MissingConstFinalVarOrType` (x5, late, y5, z5,
/// late), two stray-`;` `UnexpectedToken`, two member-lane
/// `ExpectedClassMember` — the official nine-row parser face. The
/// statement-lane `late y5;` declares `y5` (the trace runs through the same
/// after-modifiers path as the local `late x;`), so the resolver-side
/// `undefined_identifier 'y5'/'late'` over-fires have no parser row to
/// stand on.
#[test]
fn use_late_whole_file_nine_row_parser_face() {
    let source = "// https://github.com/dart-lang/sdk/issues/44379

int x1;
late int x2;
late List<int> x3;
late final int x4;
late x5;
; // Meant to confuse so `.next`-ing makes it look like the end of a field.
late;
; // Meant to confuse so `.next`-ing makes it look like the end of a field.

main(List<String> args) {
  int y1;
  late int y2;
  late List<int> y3;
  late final int y4;
  late y5;
  ; // Meant to confuse so `.next`-ing makes it look like the end of a field.
  late;
  ; // Meant to confuse so `.next`-ing makes it look like the end of a field.
}

class Foo {
  int z1;
  late int z2;
  late List<int> x3;
  late final int z4;
  late z5;
  ; // Meant to confuse so `.next`-ing makes it look like the end of a field.
  late;
  ; // Meant to confuse so `.next`-ing makes it look like the end of a field.
}
";
    assert_eq!(
        sorted_unit_codes(source),
        [
            "expected_class_member",
            "expected_class_member",
            "missing_const_final_var_or_type",
            "missing_const_final_var_or_type",
            "missing_const_final_var_or_type",
            "missing_const_final_var_or_type",
            "missing_const_final_var_or_type",
            "unexpected_token",
            "unexpected_token",
        ],
        "use_late_in_non_nnbd whole file"
    );
}

/// Statement `late;` is an expression statement (upstream trace:
/// `parseExpressionStatement` with `handleIdentifier(late, expression)` —
/// `late` is a built-in keyword, so a lone `late` before `;` is not a
/// modifier and not a type). No parser row fires; the resolver's silence on
/// the keyword-token identifier is the semantic recipe, not a parser face.
#[test]
fn statement_late_semicolon_is_a_silent_expression_statement() {
    assert!(
        sorted_unit_codes("void main() {\n  late;\n}\n").is_empty(),
        "`late;` statement parses without rows"
    );
}

// —— error_recovery/extension_member_contributor_test_completion —————————

/// `l.a` dangling at EOF and `l.a print(l.b);` mid-body: both read through
/// the upstream trace — the first as an expression statement with the
/// missing-`;` report anchored on `a`, the second as a local variable
/// declaration `print` of prefixed type `l.a` with the missing-`;` report
/// anchored on `print` (intertwined expect :246/:315). Two `expected_token`
/// rows, nothing else; the `undefined_class 'a'` over-fire is the resolver
/// treating a prefix-that-is-a-local as a namespace.
#[test]
fn dangling_member_access_typing_shapes_two_expected_tokens() {
    let source = "extension E<T extends num> on List<T> {
  bool a(int b, int c) {}
  int get b => 0;
  set c(int d) {}
}
void f(List<int> l) {
  l.a /* the user is typing here */
}
void g(List<int> l) {
  l.a /* the user is typing here */
  print(l.b);
}
";
    assert_eq!(
        sorted_unit_codes(source),
        ["expected_token", "expected_token"],
        "typing member access at EOF and mid-body"
    );
}

// —— targeted/implements_before_extends ———————————————————————————————————

/// `class A implements B extends C {}`: the parser face is the single
/// `implements_before_extends` ordering error (upstream trace:
/// `handleImplements` walks the clause, hits `extends`, reports
/// `implementsBeforeExtends` and re-routes the tail through `parseTypeList`
/// — no type-level row on the parser side). `implements_non_class` is a
/// resolver row (analyzer's resolver-side diagnostic list), and the
/// `extends_non_class` vs `undefined_class 'C'` split is resolver clause
/// routing.
#[test]
fn implements_before_extends_reports_the_ordering_face() {
    assert_eq!(
        sorted_unit_codes("class A implements B extends C {}\n"),
        ["implements_before_extends"],
        "clause-ordering recovery keeps the ordering row"
    );
}

// —— targeted/missing_star_after_sync —————————————————————————————————————

/// `Iterable<int> f() sync {}`: upstream `parseAsyncModifierOpt` reports
/// `invalidSyncModifier` and continues with no star and no re-parse (the
/// declaration and its return type survive intact), so the parser face is
/// the single `missing_star_after_sync` row. The `undefined_class
/// 'Iterable'` over-fire on the untouched return type is resolver-side.
#[test]
fn sync_without_star_reports_once_and_keeps_the_declaration() {
    assert_eq!(
        sorted_unit_codes("Iterable<int> f() sync {}\n"),
        ["missing_star_after_sync"],
        "missing star after sync"
    );
}

// —— `$` + digit interpolation (string1 / blindspot_string_dollar) ————————

/// `'$5'`: `$` starts an interpolation, the digit cannot be its identifier,
/// and the scanner reports one `missing_identifier` at the digit before the
/// synthetic name token lets the string body continue (official
/// `unexpectedDollarInString` → analyzer `missing_identifier`, one row
/// total). The parser fires nothing — the synthetic empty identifier parses
/// as a plain expression operand, and the resolver must skip the
/// nameless reference.
#[test]
fn dollar_digit_interpolation_reports_missing_identifier_once() {
    assert_eq!(
        sorted_unit_codes_with_scanner(
            "var dollarThenDigit = 'a $5 b';\n",
            &["missing_identifier"]
        ),
        Vec::<String>::new(),
        "`$5` interpolation content: scanner-home row, parser silent"
    );
}

/// `return "$1"` without a semicolon (string1.dart's `baz`): the
/// interpolation reports `missing_identifier` at the digit (scanner-home)
/// and the statement tail reports the expected `;` parser-side — one row
/// per side, matching the oracle pair.
#[test]
fn dollar_digit_without_semicolon_reports_two_rows() {
    assert_eq!(
        sorted_unit_codes_with_scanner(
            "String baz() {\n  return \"$1\"\n}\n",
            &["missing_identifier"]
        ),
        ["expected_token"],
        "`\"$1\"` with a missing `;`"
    );
}

// —— targeted/blindspot_library_directives (configured URI value) ————————

/// `if (dart.library.io == '$x')`: the configuration value parses the
/// interpolation in full (upstream `ensureLiteralString` →
/// `parseLiteralString`, so the content identifier event exists on both
/// sides) and reports exactly `invalid_literal_in_configuration`. The
/// resolver must not resolve the invalidated literal's content.
#[test]
fn configured_uri_interpolation_reports_only_invalid_literal() {
    assert_eq!(
        sorted_unit_codes("import 'a.dart' if (dart.library.io == '$x') 'b.dart';\n"),
        ["invalid_literal_in_configuration"],
        "interpolated configuration value"
    );
}

// —— general/call_on_after_try_block (on-clause exception type) ———————————

/// `on Foo {` after a `catch` clause: the exception type parses silently —
/// the parser has no row on the type, and the oracle's
/// `non_type_in_catch_clause` routing (vs the resolver's `undefined_class`)
/// is the semantic recipe. Guard: the recovery face must not grow parser
/// rows here.
#[test]
fn on_clause_exception_type_parses_silently() {
    assert!(
        sorted_unit_codes(
            "void main() {\n  try {\n    ;\n  } catch (e) {\n    ;\n  } on Foo {\n    ;\n  }\n}\n"
        )
        .is_empty(),
        "`on Foo` after catch parses without rows"
    );
}

// —— targeted/blindspot_accessors (getter inside a function body) ————————

/// `int get g => 1;` in a statement lane: the accessor-shaped head recovers
/// with `expected_token` (the `;` before `get`) and
/// `missing_function_parameters` — the oracle's exact parser pair. The
/// `return_of_invalid_type` leak onto the enclosing function's flow check
/// is resolver-side attribution.
#[test]
fn getter_in_function_body_recovers_with_the_oracle_pair() {
    assert_eq!(
        sorted_unit_codes("void getterInFunction() {\n  int get g => 1;\n}\n"),
        ["expected_token", "missing_function_parameters"],
        "local accessor recovery face"
    );
}
