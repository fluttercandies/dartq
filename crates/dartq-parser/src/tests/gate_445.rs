//! #445 regression pins: the two skip-anchor probe sites — the inline
//! function-typed formal shape follower (`formal_params`) and the
//! literal-with-`new` generic opener (`expressions`) — plus the blind-spot
//! batch: the local-function missing-parameter-list recovery arm, the
//! in-class enum/typedef reports, and end-to-end unterminated string
//! adoption (scanner synthetic closer carries a real offset into the
//! parser's group matching).

use super::collector::{Collector, latest_features, setup};

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

/// Sorted diagnostic-code multiset (scanner + parser) of one whole-unit
/// parse, for inputs the scanner itself flags (an unterminated string pins a
/// zero-width synthetic closer carrying a real offset).
fn sorted_codes_with_scanner(source: &'static str) -> Vec<String> {
    let config = dartq_scanner::ScanConfig {
        enable_triple_shift: true,
        enable_augmentations: true,
    };
    let mut files = dartq_source::FileTable::new();
    let file = files.intern(std::path::Path::new("test.dart"));
    let result = dartq_scanner::scan(source, config, file);
    let table: &'static dartq_scanner::TokenTable = Box::leak(Box::new(result.tokens));
    let text: &'static str = Box::leak(source.to_string().into_boxed_str());
    let mut codes: Vec<String> = result
        .diagnostics
        .iter()
        .map(|d| d.code.name.to_owned())
        .collect();
    let mut p = crate::Parser::new(text, table, latest_features(), Collector::default());
    p.parse_unit();
    codes.extend(p.listener.error_codes());
    codes.sort();
    codes
}

/// [`assert_codes`] over [`sorted_codes_with_scanner`].
fn assert_codes_with_scanner(source: &'static str, expected: &[&str], label: &str) {
    let got = sorted_codes_with_scanner(source);
    let expected: Vec<String> = {
        let mut e = expected.iter().map(ToString::to_string).collect::<Vec<_>>();
        e.sort();
        e
    };
    assert_eq!(got, expected, "{label}: diagnostic multiset diverged");
}

// —— inline function-typed formal shape follower (official :2236-2244) ————

/// `void cb<T>(int x)` keeps the inline function-typed shape: the follower
/// after the `<T>` group is `(`, so `begin_function_typed_formal_parameter`
/// fires. Oracle 3.13.1: silent.
#[test]
fn inline_function_typed_formal_with_type_variables() {
    let mut p = setup("void f(void cb<T>(int x)) {}\n");
    p.parse_unit();
    assert!(
        p.listener.errors.is_empty(),
        "oracle-silent shape: {:?}",
        p.listener.errors
    );
    p.listener.assert_sequence(&[
        ("begin", "function_typed_formal"),
        ("end", "function_typed_formal"),
    ]);
}

/// A raw compound closer splits (official `splitGtGt`): the shape follower
/// is the remainder `>`, never the real successor, so `cb<T>>(int x) y`
/// does NOT take the inline function-typed shape. Oracle 3.13.1: exactly
/// one `expected_token`.
#[test]
fn inline_function_typed_formal_compound_closer_is_not_the_shape() {
    assert_codes(
        "void f(void cb<T>>(int x) y) {}\n",
        &["expected_token"],
        "compound closer reads its remainder",
    );
}

// —— literal-with-`new` generic opener (official :8953-8966) ——————————————

/// `new <int>{};` drops the `new` and parses the map literal
/// (`literalWithNew`); the `<...>` group routes through the real type
/// arguments and the set-or-map literal event, never a new expression.
/// Oracle 3.13.1: exactly one `literal_with_new`.
#[test]
fn new_prefixed_generic_literal_recovers_as_literal() {
    assert_codes(
        "void f() {\n  new <int>{};\n}\n",
        &["literal_with_new"],
        "generic literal with new",
    );
    let mut p = setup("void f() {\n  new <int>{};\n}\n");
    p.parse_unit();
    p.listener
        .assert_sequence(&[("end", "type_arguments"), ("handle", "literal_set_or_map")]);
    assert!(
        !p.listener.log.contains(&("begin", "new_expression")),
        "the `new` must be dropped, not parsed: {:?}",
        p.listener.log
    );
}

/// The non-literal `new` keeps the new-expression lane (control for the
/// generic-opener probe above).
#[test]
fn plain_new_keeps_the_new_expression_lane() {
    let mut p = setup("void f() {\n  new C();\n}\n");
    p.parse_unit();
    assert!(
        p.listener.errors.is_empty(),
        "parser-clean shape: {:?}",
        p.listener.errors
    );
    p.listener
        .assert_sequence(&[("begin", "new_expression"), ("end", "new_expression")]);
}

// —— local function missing its parameter list (official :9756-9759) ———————

/// `h<T> Function() {}` takes `looksLikeLocalFunction`'s recovery arm
/// (identifier, `<...>` group, `Function` follower at the formals
/// position): the statement parses as a local function declaration with
/// type variables instead of a variable declaration. Oracle 3.13.1:
/// syntactically clean.
#[test]
fn local_function_missing_parameter_list_adopts_the_declaration() {
    let mut p = setup("void f() {\n  h<T> Function() {}\n}\n");
    p.parse_unit();
    assert!(
        p.listener.errors.is_empty(),
        "oracle-clean shape: {:?}",
        p.listener.errors
    );
    p.listener.assert_sequence(&[
        ("begin", "local_function_declaration"),
        ("begin", "type_arguments"),
        ("end", "type_arguments"),
        ("handle", "type"),
        ("begin", "function_name"),
        ("begin", "formal_parameters"),
        ("end", "formal_parameters"),
        ("begin", "block_function_body"),
        ("end", "local_function_declaration"),
    ]);
}

/// Without the `()` the `Function` intruder cannot carry the declaration:
/// the statement stays a variable declaration missing its `;`. Oracle
/// 3.13.1: exactly one `expected_token`.
#[test]
fn function_intruder_without_formals_stays_a_variable() {
    let src = "void f() {\n  h<T> Function {}\n}\n";
    assert_codes(src, &["expected_token"], "Function intruder variable path");
    let mut p = setup(src);
    p.parse_unit();
    p.listener
        .assert_sequence(&[("begin", "variables_declaration")]);
    assert!(
        !p.listener
            .log
            .contains(&("begin", "local_function_declaration")),
        "no declaration adoption without `()`: {:?}",
        p.listener.log
    );
}

// —— in-class enum/typedef reports (official `recoverFromInvalidMember`) ——

/// `enum` inside a class reports `enum_in_class` and the shaped declaration
/// is skipped whole. Oracle 3.13.1: exactly one `enum_in_class`.
#[test]
fn enum_in_class_skips_the_declaration() {
    assert_codes(
        "class C {\n  enum E { x }\n}\n",
        &["enum_in_class"],
        "in-class enum",
    );
}

/// `typedef F = void Function();` inside a class reports `typedef_in_class`
/// and — `reportAndSkipTypedefInClass` never skips — reparses from the
/// alias body: `F = void Function();` rides in as a field parse
/// (`missing_const_final_var_or_type`), the `=`-headed member splices the
/// operator recovery (`expected_token`, `missing_identifier`). Oracle
/// 3.13.1: the same four syntactic codes.
#[test]
fn typedef_in_class_function_alias_reparse() {
    assert_codes(
        "class C {\n  typedef F = void Function();\n}\n",
        &[
            "typedef_in_class",
            "missing_const_final_var_or_type",
            "expected_token",
            "missing_identifier",
        ],
        "in-class function-alias typedef",
    );
}

// —— unterminated string end-to-end (scanner synthetic adoption) ——————————

/// An unterminated string pins a zero-width synthetic closer carrying a real
/// offset; the parser adopts it through the string group so the statement
/// recovers with the scanner's `unterminated_string_literal` plus the
/// statement's `expected_token` — and no runaway group swallowing the body.
#[test]
fn unterminated_string_adopts_its_synthetic_closer() {
    assert_codes_with_scanner(
        "void f() {\n  var s = 'abc;\n}\n",
        &["unterminated_string_literal", "expected_token"],
        "synthetic string closer adoption",
    );
}
