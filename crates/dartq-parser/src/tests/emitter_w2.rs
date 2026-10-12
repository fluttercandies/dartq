//! Emitter-wave W2 pins (#885): the W4 face-alignment wave, each change
//! locked to its adjudicated probe row (docs/audits/w4-blindspot-probes.md;
//! oracle dart analyze 3.13.1, machine format).
//!
//! * `invalid_literal_in_configuration` — the bundle's one missing emitter:
//!   `endConditionalUri` (ast_builder.dart:1509-1524) checks only the `==`
//!   VALUE and only when it is a single `StringInterpolationImpl`; the row
//!   anchors at the first interpolation expression. Adjacent-strings values
//!   stay silent (re-probed live at 3.13.1: `== 'a' '$x'` emits no row).
//! * `non_sync_abstract_method` — dartq-only rows retired: the oracle
//!   converter suppresses the code (error_converter.dart:338-340,
//!   "Not reported but followed by a MISSING_FUNCTION_BODY error").
//! * `required_parameter_with_default` — pseudo-shared to
//!   `named_parameter_outside_group` (fe91 codes_generated.dart:2742-2746);
//!   both separators on a required positional report the same code.
//! * `empty_*_parameter_list` — pseudo-shared to `missing_identifier`
//!   (fe91 codes :577-590).
//! * `wrong_number_of_parameters_for_setter` — the top-level setter arm
//!   (ast_builder.dart:6257/:6289) mirrors the class-member check; the row
//!   anchors on the setter name.
//! * not-sync anchors — setter/factory rows span the `async`/`sync` keyword
//!   (`asyncToken`, parser_impl.dart:4644/:6060/:6230), never the star.

use super::collector::{latest_features, scan_ok, setup};
use crate::Parser;

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
    let mut expected: Vec<String> = expected.iter().map(ToString::to_string).collect();
    expected.sort();
    assert_eq!(got, expected, "{label}: diagnostic multiset diverged");
}

/// Asserts `code` never fires.
fn assert_code_absent(source: &'static str, code: &str, label: &str) {
    let mut p = setup(source);
    p.parse_unit();
    let got = p.listener.error_codes();
    assert!(
        !got.iter().any(|c| c == code),
        "{label}: {code} fired: {got:?}"
    );
}

/// Parses and returns the first diagnostic's `(code, start, end)` span
/// (UTF-16 units; ASCII sources make them byte-exact with the probes).
fn first_span(source: &'static str) -> (String, u32, u32) {
    let (table, text) = scan_ok(source);
    let mut p = Parser::new(
        text,
        table,
        latest_features(),
        super::collector::Collector::default(),
    );
    p.parse_unit();
    let (code, start, end) = &p.listener.errors[0];
    (code.clone(), *start, *end)
}

// —— invalid_literal_in_configuration (probe f08/b04/b03) ——————————————————

/// Probed row: 2:41 len 2 — the first interpolation expression `$x` of the
/// `==` value (`$` start, identifier end).
#[test]
fn conditional_uri_value_interpolation_reports() {
    let (code, start, end) = first_span("import 'a.dart' if (dart.library.io == '$x') 'b.dart';\n");
    assert_eq!(code, "invalid_literal_in_configuration");
    assert_eq!((start, end), (40, 42));
}

/// Probed b04: two interpolations, one row only — at the FIRST (`$x`).
#[test]
fn conditional_uri_reports_only_first_interpolation() {
    let (code, start, end) =
        first_span("import 'a.dart' if (dart.library.io == '$x$y') 'b.dart';\n");
    assert_eq!(code, "invalid_literal_in_configuration");
    assert_eq!((start, end), (40, 42));
}

/// Probed b03: a plain value literal reports nothing (the URI_DOES_NOT_EXIST
/// family is resolution-tier).
#[test]
fn conditional_uri_plain_value_stays_silent() {
    assert_code_absent(
        "import 'a.dart' if (dart.library.io == 't') 'b.dart';\n",
        "invalid_literal_in_configuration",
        "plain conditional-uri value",
    );
}

/// The URI itself is never checked (probe f09 value-vs-uri split): the
/// `uri_with_interpolation` face belongs to resolution, not the parse tier.
#[test]
fn uri_position_interpolation_does_not_report() {
    assert_code_absent(
        "import '${'a'}.dart';\n",
        "invalid_literal_in_configuration",
        "interpolated import URI",
    );
}

/// Adjacent-strings values are not `StringInterpolationImpl` — re-probed
/// live at 3.13.1 (`== 'a' '$x'` emits no such row), mirroring the official
/// `is StringInterpolationImpl` test.
#[test]
fn conditional_uri_adjacent_strings_value_stays_silent() {
    assert_code_absent(
        "import 'a.dart' if (dart.library.io == 'a' '$x') 'b.dart';\n",
        "invalid_literal_in_configuration",
        "adjacent-strings conditional-uri value",
    );
}

// —— non_sync_abstract_method retirement (probe f04/c03) ———————————————————

/// Probed f04: the oracle emits `missing_function_body` twice and NOTHING
/// else at the parse tier — the dartq-only `non_sync_abstract_method` rows
/// are suppressed by the oracle converter (error_converter.dart:338-340).
#[test]
fn abstract_member_async_modifier_suppression() {
    assert_codes(
        "class A { void m() async; void g() sync*; }\n",
        &["missing_function_body", "missing_function_body"],
        "async/sync* abstract members",
    );
    assert_code_absent(
        "class A { void m() async; void g() sync*; }\n",
        "non_sync_abstract_method",
        "async/sync* abstract members",
    );
}

/// Probed c03: the suppression holds in an abstract class too.
#[test]
fn abstract_class_async_member_suppression() {
    assert_codes(
        "abstract class A { void m() async; }\n",
        &["missing_function_body"],
        "abstract-class async member",
    );
    assert_code_absent(
        "abstract class A { void m() async; }\n",
        "non_sync_abstract_method",
        "abstract-class async member",
    );
}

// —— required-positional default value (probe f15/b05/b06) —————————————————

/// Probed f15: `=` on a required positional reports the pseudo-shared
/// `named_parameter_outside_group` at the `=` (2:14 len 1 on the probe's
/// line 2; offset 13 on this line-1 source).
#[test]
fn required_positional_default_repoints_to_named_parameter_outside_group() {
    let (code, start, end) = first_span("void f(int x = 1) {}\n");
    assert_eq!(code, "named_parameter_outside_group");
    assert_eq!((start, end), (13, 14));
}

/// Probed b05: the `:` separator keeps the same code — both separators on a
/// required positional route identically.
#[test]
fn required_positional_colon_matches_equals_code() {
    assert_codes(
        "void f(int x: 1) {}\n",
        &["named_parameter_outside_group"],
        "colon on required positional",
    );
}

/// Probed b06: defaults inside the optional groups and plain formals are the
/// negative boundary — nothing fires.
#[test]
fn grouped_defaults_stay_silent() {
    assert_codes(
        "void f([int x = 1]) {}\n",
        &[],
        "optional-positional default",
    );
    assert_codes("void f({int y = 2}) {}\n", &[], "optional-named default");
    assert_codes("void f(int z) {}\n", &[], "plain positional");
}

// —— empty optional/named groups (probe f14) ———————————————————————————————

/// Probed f14: `void named({}) {}` / `void optional([]) {}` report the
/// pseudo-shared `missing_identifier` at the closing delimiter (2:13:1 on
/// the probe's layout = offset 12 here; the `[]` Index token splits with the
/// `]` at offset+1 = 15).
#[test]
fn empty_parameter_groups_report_missing_identifier() {
    let (code, start, end) = first_span("void named({}) {}\n");
    assert_eq!(code, "missing_identifier");
    assert_eq!((start, end), (12, 13));
    let (code, start, end) = first_span("void optional([]) {}\n");
    assert_eq!(code, "missing_identifier");
    assert_eq!((start, end), (15, 16));
}

// —— setter arity (probe f18/b10) ——————————————————————————————————————————

/// Probed f18 class arm: one row anchored on the setter name.
#[test]
fn class_setter_arity_anchors_on_name() {
    let (code, start, end) = first_span("class C { set s(a, b) {} }\n");
    assert_eq!(code, "wrong_number_of_parameters_for_setter");
    assert_eq!((start, end), (14, 15));
}

/// Probed f18 top-level arm (the W4 fix): `set topSetter() => 1;` fires at
/// 6:5:9 on the probe's layout — the identifier `topSetter`, offset 4 len 9
/// on this line-1 source.
#[test]
fn top_level_setter_arity_anchors_on_name() {
    let (code, start, end) = first_span("set topSetter() => 1;\n");
    assert_eq!(code, "wrong_number_of_parameters_for_setter");
    assert_eq!((start, end), (4, 13));
}

/// Probed b10: an optional parameter also violates the exactly-one-required
/// rule.
#[test]
fn optional_param_setter_still_reports_arity() {
    assert_codes(
        "class C { set s([a]) {} }\n",
        &["wrong_number_of_parameters_for_setter"],
        "optional-param setter",
    );
}

/// A conforming setter stays silent.
#[test]
fn conforming_setter_stays_silent() {
    assert_codes("class C { set s(int x) {} }\n", &[], "conforming setter");
}

// —— not-sync anchors (probe b02/c01/f02/f03) ——————————————————————————————

/// Probed b02: `set foo(int x) sync* {}` — the row spans the `sync` keyword
/// (2:16 len 4; offset 15 on this line-1 source), not the star.
#[test]
fn starred_sync_setter_anchors_on_keyword() {
    let (code, start, end) = first_span("set foo(int x) sync* {}\n");
    assert_eq!(code, "invalid_modifier_on_setter");
    assert_eq!((start, end), (15, 19));
}

/// Probed f02: the plain `async` setter keeps the keyword anchor (2:16 len 5).
#[test]
fn async_setter_anchors_on_keyword() {
    let (code, start, end) = first_span("set foo(int x) async {}\n");
    assert_eq!(code, "invalid_modifier_on_setter");
    assert_eq!((start, end), (15, 20));
}

/// Probed c01: the starred factory reports `non_sync_factory` on the `sync`
/// keyword (probe layout 4:17 len 4 = `  factory … sync*` line 4, so the
/// shape is a CLASS MEMBER; here line 2 with `async` at offset 26, span
/// (26, 30) — col 17 as measured).
#[test]
fn starred_sync_factory_anchors_on_keyword() {
    let (code, start, end) = first_span("class C {\n  factory F.x() sync* => F._();\n}\n");
    assert_eq!(code, "non_sync_factory");
    assert_eq!((start, end), (26, 30));
}

/// Probed f03: the plain `async` factory keeps the keyword anchor (probe
/// layout 4:17 len 5; here (26, 31)).
#[test]
fn async_factory_anchors_on_keyword() {
    let (code, start, end) = first_span("class C {\n  factory F.x() async => F._();\n}\n");
    assert_eq!(code, "non_sync_factory");
    assert_eq!((start, end), (26, 31));
}

/// Top-level `factory` shape (re-probed live at 3.13.1): the oracle reports
/// `factory_top_level_declaration` and NEVER `non_sync_factory` there — the
/// not-sync check is a class-member face only. Guards the dispatch order:
/// the top-level recovery must not fall through into the factory body parse.
#[test]
fn top_level_factory_reports_no_not_sync() {
    let (code, start, end) = first_span("factory F.x() async => F._();\n");
    assert_eq!(code, "factory_top_level_declaration");
    assert_eq!((start, end), (0, 7));
    assert_code_absent(
        "factory F.x() async => F._();\n",
        "non_sync_factory",
        "top-level factory",
    );
}

/// Probed f01 (aligned, re-pinned): a bare `sync` keeps
/// `missing_star_after_sync` on the keyword.
#[test]
fn bare_sync_still_reports_missing_star() {
    let (code, start, end) = first_span("void f() sync {}\n");
    assert_eq!(code, "missing_star_after_sync");
    assert_eq!((start, end), (9, 13));
}

/// Probed c02 (negative boundary): clean async/generator forms are silent.
#[test]
fn clean_async_forms_stay_silent() {
    assert_codes("Future<int> g() async => 1;\n", &[], "clean async");
    assert_codes("void f() {}\n", &[], "plain sync");
}
