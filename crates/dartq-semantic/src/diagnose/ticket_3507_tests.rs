//! #3507 pins — the const collection element verdict table: the `for`
//! element face and the element-family codes it shares a verdict table
//! with.
//!
//! # The contract
//!
//! Every `isConst` collection literal (an explicit `const` keyword, or a
//! `const` declaration / default value / annotation context —
//! `constantContext`, pkg/analyzer/lib/src/dart/ast/ast.dart) runs the
//! element verifier (upstream Dart SDK source,
//! pkg/analyzer/lib/src/dart/constant/constant_verifier.dart,
//! `_ConstLiteralVerifier.verify`) over ITS OWN elements:
//!
//! - a `for` element reports `const_eval_for_element` at the whole element
//!   (`for` keyword through the body) and nothing else for that element —
//!   list, set and map alike, top-level or local, even wrapped in an `if`
//!   element;
//! - a map key / set element whose value lacks primitive equality reports
//!   `const_map_key_not_primitive_equality` /
//!   `const_set_element_not_primitive_equality` at the key / element
//!   (`_validateMapLiteralEntry` :1502, `_validateSetExpression` :1158) —
//!   a nested literal is evaluated, never verified, so its keys stay silent;
//! - a bare expression where a map entry is expected reports
//!   `expression_in_map` at the expression, for const AND non-const typed
//!   maps (pkg/analyzer/lib/src/error/literal_element_verifier.dart :100/:135;
//!   an untyped literal reports the ambiguity face instead).
//!
//! `invalid_type_argument_in_const_literal` is the const typed literal's
//! type-argument check (error_verifier.dart): a type parameter anywhere
//! inside the written type arguments reports at the parameter's own
//! identifier; non-const literals are silent.
//!
//! The site default `const_initialized_with_non_constant_value` rides
//! `visitVariableDeclaration` → `_reportError` over the variable's
//! `evaluationResult`: a `for`-element initializer adds it at the literal
//! for a TOP-LEVEL `const` (the literal node spans its type arguments:
//! `<int>[…]`), and never for a LOCAL `const` — a plain non-constant element
//! (`const l = [g]`) adds it for both (that half is green already).
//!
//! # Oracle verification
//!
//! Every expectation was oracle-verified against
//! `~/.dartq-oracle/3.13.7/dart-sdk/bin/dart analyze --format machine`
//! (batch files under /tmp/dq3507: a.dart `for` shapes t1-t8/l1-l10,
//! c.dart type-argument + map-entry shapes, d.dart/e.dart primitive-equality
//! shapes, f.dart the plain-element site default) and cross-checked with
//! `conformance/expected.tsv` (`diagnostics/const_eval_for_element/…`,
//! `…/invalid_type_argument_in_const_list/…`).
//!
//! # Routing
//!
//! The `for`-element face landed with #3399 (`listener.rs`
//! `handle_literal_list` / the set-or-map close + `const_eval.rs`'s
//! ctl-element latch) — the live pins below hold it. The remaining rows
//! are red by MACHINERY GAP in files outside this ticket's `engine_core.rs`
//! scope, so each gap pin is `#[ignore]`d and names its owning seam:
//!
//! - local `const` + `for` element must drop the literal-level site default
//!   (`const_eval_faces.rs` `finish_const_initializer`: the `local` flag
//!   already distinguishes the two sites — the for-element failed literal's
//!   `NotConstant(literal)` verdict must not reach `resolve_const_verdict`
//!   when `local`);
//! - the top-level site default of a TYPED literal spans the type arguments
//!   (`listener.rs` `handle_literal_list`'s for-element branch builds
//!   `literal_span` from the `[` token; the oracle's `ListLiteral` node
//!   starts at `<`);
//! - `expression_in_map` (the literal-element-verifier mirror beside
//!   `assignability.rs`'s `check_list_literal_elements` /
//!   `check_collection_void_elements`, fed from the set-or-map close in
//!   `listener.rs`);
//! - the primitive-equality key / element checks (`const_eval_faces.rs`
//!   `check_const_elements`: a passing `Verdict::Value` whose type declares
//!   `==` or `hashCode`, inherits one, or is a record carrying such a field
//!   — a merely abstract `==` declaration does not count);
//! - `invalid_type_argument_in_const_literal` (the typed-literal face next
//!   to `assignability.rs` `typed_list_literal_ty`: walk the written type
//!   arguments, function types included, for in-scope type parameters).

use super::FileReport;
use super::test_rig::{CORE_PRELUDE, report_ops_bare};

/// The #3507 code family: the rows these pins own. Side rows of the rig
/// (`unused_local_variable`, prelude resolution noise) are other faces.
const FAMILY: [&str; 12] = [
    "const_eval_for_element",
    "const_initialized_with_non_constant_value",
    "non_constant_list_element",
    "non_constant_set_element",
    "non_constant_map_element",
    "non_constant_map_key",
    "non_constant_map_value",
    "expression_in_map",
    "const_map_key_not_primitive_equality",
    "const_set_element_not_primitive_equality",
    "invalid_type_argument_in_const_literal",
    "ambiguous_set_or_map_literal_both",
];

/// `(code, offset, written text at the row)` for the family rows — code +
/// anchor + length pinned together, sorted by `(code, offset)` so expected
/// vectors are listed in code-name order.
fn family_rows<'a>(report: &FileReport, source: &'a str) -> Vec<(&'static str, u32, &'a str)> {
    let mut rows: Vec<(&'static str, u32, &'a str)> = report
        .diagnostics
        .iter()
        .filter(|d| FAMILY.contains(&d.code.name))
        .map(|d| {
            (
                d.code.name,
                d.range.offset,
                &source[d.range.offset as usize..d.range.end() as usize],
            )
        })
        .collect();
    rows.sort();
    rows
}

/// Diagnoses `fixture` under [`CORE_PRELUDE`] and returns the family rows
/// with the prelude-prefixed source the offsets index into.
fn family_of(fixture: &str) -> (Vec<(&'static str, u32, String)>, String) {
    let source = format!("{CORE_PRELUDE}{fixture}");
    let report = report_ops_bare(&source);
    let rows = family_rows(&report, &source)
        .into_iter()
        .map(|(code, offset, text)| (code, offset, text.to_string()))
        .collect();
    (rows, source)
}

/// Offset of `needle` inside the first occurrence of `line` in `source`.
fn at(source: &str, line: &str, needle: &str) -> u32 {
    let line_at = source.find(line).unwrap_or_else(|| panic!("fixture has {line:?}"));
    let needle_at = line.find(needle).unwrap_or_else(|| panic!("{line:?} has {needle:?}"));
    (line_at + needle_at) as u32
}

const FOR: &str = "for (var i = 0; i < 3; i++) i";

/// An explicit `const` list under a non-const variable: the `for` element
/// reports alone — the site default belongs to `const` variables only.
///
/// Oracle a.dart t6 (`var t6 = const [for (var i = 0; i < 3; i++) i];`):
/// exactly `const_eval_for_element` 6:17 len 29.
#[test]
fn explicit_const_list_for_element_reports_the_element_alone() {
    let fixture = "var t = const [for (var i = 0; i < 3; i++) i];\n";
    let (rows, source) = family_of(fixture);
    assert_eq!(
        rows,
        [("const_eval_for_element", at(&source, fixture, FOR), FOR.to_string())],
        "{rows:#?}"
    );
}

/// Top-level `const` set and map: the `for` element at its extent (the map
/// form's body is the `i: i` entry) plus the site default at each literal.
///
/// Oracle a.dart t3/t4: `const_eval_for_element` 3:13 len 29 and 4:13
/// len 32; `const_initialized_with_non_constant_value` 3:12 len 31 and
/// 4:12 len 34.
#[test]
fn top_level_const_set_and_map_for_elements_add_the_site_default() {
    let set = "const s = {for (var i = 0; i < 3; i++) i};\n";
    let map = "const m = {for (var i = 0; i < 3; i++) i: i};\n";
    let (rows, source) = family_of(&format!("{set}{map}"));
    let map_for = "for (var i = 0; i < 3; i++) i: i";
    assert_eq!(
        rows,
        [
            ("const_eval_for_element", at(&source, set, FOR), FOR.to_string()),
            ("const_eval_for_element", at(&source, map, map_for), map_for.to_string()),
            (
                "const_initialized_with_non_constant_value",
                at(&source, set, "{"),
                "{for (var i = 0; i < 3; i++) i}".to_string(),
            ),
            (
                "const_initialized_with_non_constant_value",
                at(&source, map, "{"),
                "{for (var i = 0; i < 3; i++) i: i}".to_string(),
            ),
        ],
        "{rows:#?}"
    );
}

/// A `for` element under an `if` element reports the `for` alone (the `if`
/// wrapper contributes no row), site default at the whole literal.
///
/// Oracle a.dart t8 (`const t8 = [if (true) for (var i = 0; i < 3; i++) i];`):
/// `const_eval_for_element` 8:23 len 29;
/// `const_initialized_with_non_constant_value` 8:12 len 41.
#[test]
fn for_element_under_an_if_element_reports_the_for_alone() {
    let fixture = "const t = [if (true) for (var i = 0; i < 3; i++) i];\n";
    let (rows, source) = family_of(fixture);
    assert_eq!(
        rows,
        [
            ("const_eval_for_element", at(&source, fixture, FOR), FOR.to_string()),
            (
                "const_initialized_with_non_constant_value",
                at(&source, fixture, "["),
                "[if (true) for (var i = 0; i < 3; i++) i]".to_string(),
            ),
        ],
        "{rows:#?}"
    );
}

/// The for-in spelling spans `for` through the body the same way.
///
/// Oracle a.dart t2 (`const List<int> t2 = [for (var i in [1, 2]) i];`):
/// `const_eval_for_element` 2:23 len 23;
/// `const_initialized_with_non_constant_value` 2:22 len 25.
#[test]
fn for_in_element_spans_the_head_through_the_body() {
    let fixture = "const List<int> t = [for (var i in [1, 2]) i];\n";
    let (rows, source) = family_of(fixture);
    let for_in = "for (var i in [1, 2]) i";
    assert_eq!(
        rows,
        [
            ("const_eval_for_element", at(&source, fixture, for_in), for_in.to_string()),
            (
                "const_initialized_with_non_constant_value",
                at(&source, fixture, "[for"),
                "[for (var i in [1, 2]) i]".to_string(),
            ),
        ],
        "{rows:#?}"
    );
}

/// A LOCAL `const` with a `for` element reports the element alone — the
/// local variable's evaluation carries the allow-listed
/// `const_eval_for_element` code, so `_reportError` never substitutes the
/// site default (contrast the plain-element shape `const l = [g]`, which
/// does add it — green already, oracle f.dart l1/l3).
///
/// Oracle a.dart l5 (`const l5 = [for (var i = 0; i < 3; i++) i];` inside
/// `main`): exactly `const_eval_for_element` 14:15 len 29 — no
/// `const_initialized_with_non_constant_value`. dartq adds the site
/// default at the literal (14:14 len 31).
#[test]
#[ignore = "const_eval_faces.rs finish_const_initializer: a local const's for-element failed literal must not reach the site default"]
fn local_const_for_element_has_no_site_default() {
    let fixture = "void f() {\n  const l = [for (var i = 0; i < 3; i++) i];\n}\n";
    let (rows, source) = family_of(fixture);
    assert_eq!(
        rows,
        [("const_eval_for_element", at(&source, fixture, FOR), FOR.to_string())],
        "{rows:#?}"
    );
}

/// The site default of a TYPED top-level literal spans the type arguments:
/// the oracle's `ListLiteral` node starts at `<`.
///
/// Oracle a.dart t5 (`const t5 = <int>[for (var i = 0; i < 3; i++) i];`):
/// `const_eval_for_element` 5:18 len 29;
/// `const_initialized_with_non_constant_value` 5:12 len 36. dartq anchors
/// the site default at `[` (5:17 len 31).
#[test]
#[ignore = "listener.rs handle_literal_list: the for-element branch's literal_span must start at the type arguments"]
fn typed_top_level_literal_site_default_spans_the_type_arguments() {
    let fixture = "const t = <int>[for (var i = 0; i < 3; i++) i];\n";
    let (rows, source) = family_of(fixture);
    assert_eq!(
        rows,
        [
            ("const_eval_for_element", at(&source, fixture, FOR), FOR.to_string()),
            (
                "const_initialized_with_non_constant_value",
                at(&source, fixture, "<int>["),
                "<int>[for (var i = 0; i < 3; i++) i]".to_string(),
            ),
        ],
        "{rows:#?}"
    );
}

/// A bare expression where a map entry is expected reports
/// `expression_in_map` at the expression — const and non-const typed maps
/// alike, every stray element of the literal.
///
/// Oracle c.dart m1-m4 (`<String, int>{'a', 'b' : 2}` as a top-level `var`,
/// a top-level `const`, a local `var`, a local `const`): `expression_in_map`
/// 17:24 / 18:26 / 20:26 / 21:28, len 3 each; e.dart
/// `const <String, int>{'a', 'b': 2, 'c'}`: 12:23 and 12:36, len 3 each.
/// dartq reports nothing.
#[test]
#[ignore = "listener.rs set-or-map close + assignability.rs literal-element faces: the map-entry shape check (literal_element_verifier.dart :100/:135) is unwired"]
fn stray_expression_in_a_typed_map_reports_expression_in_map() {
    let m1 = "var m1 = <String, int>{'a', 'b': 2};\n";
    let m2 = "const m2 = <String, int>{'a', 'b': 2};\n";
    let m3 = "  var m3 = <String, int>{'a', 'b': 2};\n";
    let m4 = "  const m4 = <String, int>{'a', 'b': 2};\n";
    let m5 = "  const m5 = <String, int>{'a', 'b': 2, 'c'};\n";
    let fixture = format!("{m1}{m2}void f() {{\n{m3}{m4}{m5}}}\n");
    let (rows, source) = family_of(&fixture);
    let a = |line: &str| ("expression_in_map", at(&source, line, "'a'"), "'a'".to_string());
    assert_eq!(
        rows,
        [
            a(m1),
            a(m2),
            a(m3),
            a(m4),
            a(m5),
            ("expression_in_map", at(&source, m5, "'c'"), "'c'".to_string()),
        ],
        "{rows:#?}"
    );
}

/// A const map key / set element without primitive equality reports at the
/// key / element: a declared `==` or `hashCode`, an inherited one, a record
/// carrying such a field; a merely abstract `==` declaration keeps
/// primitive equality; nested literals are evaluated, not verified
/// (`const [const {const A(): 0}]` is silent); primitives are silent.
///
/// Oracle d.dart: `const_map_key_not_primitive_equality` 17:10 len 9
/// (`const A()`), 18:10 len 3 (`A()`), 19:10 len 9 (`const B()`), 20:10
/// len 9 (`const C()`), 23:10 len 14 (`(const A(), 1)`), 24:10 len 14
/// (`(a: const A())`), 26:18 len 9 (`const <A, int>{…}`), 27:18 len 9
/// (`var x = const {…}`), 28:26 len 9 (`const Map<A, int> y = {…}`), 31:12
/// len 3 (top-level `const z = {A(): 0}`); `const {const D(): 0}` (abstract
/// `==`) and `const {1: 0, 'a': 1}` silent. e.dart: `const {const A()}` →
/// `const_set_element_not_primitive_equality` 11:10 len 9;
/// `const [const {const A(): 0}]` silent. dartq reports nothing. (The
/// `double` shapes — `1.5`, `double.infinity` — report upstream through
/// `dart:core`'s own `==`; the rig's prelude `double` declares none, so
/// they are not pinned here.)
#[test]
#[ignore = "const_eval_faces.rs check_const_elements: the primitive-equality key/element verdict (constant_verifier.dart _validateMapLiteralEntry :1502 / _validateSetExpression :1158) is unwired"]
fn const_map_key_without_primitive_equality_reports_at_the_key() {
    let z = "const z = {A(): 0};\n";
    let k1 = "  const {const A(): 0};\n";
    let k2 = "  const {A(): 0};\n";
    let k3 = "  const {const B(): 0};\n";
    let k4 = "  const {const C(): 0};\n";
    let k5 = "  const {const D(): 0};\n";
    let k6 = "  const {(const A(), 1): 0};\n";
    let k7 = "  const {(a: const A()): 0};\n";
    let k8 = "  const {1: 0, 'a': 1};\n";
    let k9 = "  const <A, int>{const A(): 0};\n";
    let k10 = "  var x = const {const A(): 0};\n";
    let k11 = "  const Map<A, int> y = {const A(): 0};\n";
    let k12 = "  const [const {const A(): 0}];\n";
    let k13 = "  const {const A()};\n";
    let fixture = format!(
        "class A {{\n  const A();\n  bool operator ==(Object other) => true;\n}}\n\
         class B {{\n  const B();\n  int get hashCode => 0;\n}}\n\
         class C extends A {{\n  const C();\n}}\n\
         class D {{\n  const D();\n  bool operator ==(Object other);\n}}\n\
         {z}void f() {{\n{k1}{k2}{k3}{k4}{k5}{k6}{k7}{k8}{k9}{k10}{k11}{k12}{k13}}}\n"
    );
    let (rows, source) = family_of(&fixture);
    let key = |line: &str, text: &str| {
        (
            "const_map_key_not_primitive_equality",
            at(&source, line, text),
            text.to_string(),
        )
    };
    assert_eq!(
        rows,
        [
            key(z, "A()"),
            key(k1, "const A()"),
            key(k2, "A()"),
            key(k3, "const B()"),
            key(k4, "const C()"),
            key(k6, "(const A(), 1)"),
            key(k7, "(a: const A())"),
            key(k9, "const A()"),
            key(k10, "const A()"),
            key(k11, "const A()"),
            (
                "const_set_element_not_primitive_equality",
                at(&source, k13, "const A()"),
                "const A()".to_string(),
            ),
        ],
        "{rows:#?}"
    );
}

/// A const typed literal whose written type arguments mention an in-scope
/// type parameter reports at the parameter's identifier — anywhere inside
/// the arguments (function-type parameters and return types, nested
/// arguments), for the default-value context too; non-const literals are
/// silent.
///
/// Oracle c.dart: `invalid_type_argument_in_const_literal` 3:41 (the
/// default value's `E Function()`), 6:12, 7:26, 8:12, 10:12, 12:12, 13:20,
/// 14:25 — len 1 each; lines 5, 9, 11 (non-const) silent. Messages:
/// "Constant list literals can't use a type parameter in a type argument,
/// such as 'E'." (set / map spellings for `{}`). dartq reports nothing.
#[test]
#[ignore = "assignability.rs typed-literal face: the const typed literal's type-argument walk (error_verifier.dart) is unwired"]
fn const_typed_literal_type_parameter_argument_reports_at_the_parameter() {
    let d = "  const A([this.x = const <String, List<E Function()>>{}]);\n";
    let l1 = "    <E>[];\n";
    let l2 = "    const <E>[];\n";
    let l3 = "    const <void Function(E)>[];\n";
    let l4 = "    const <E Function()>[];\n";
    let s1 = "    <E>{};\n";
    let s2 = "    const <E>{};\n";
    let m1 = "    <String, E>{};\n";
    let m2 = "    const <E, String>{};\n";
    let m3 = "    const <String, E>{};\n";
    let m4 = "    const <String, List<E>>{};\n";
    let fixture = format!(
        "class A<E> {{\n  final Map<String, List<E Function()>> x;\n{d}  void m() {{\n\
         {l1}{l2}{l3}{l4}{s1}{s2}{m1}{m2}{m3}{m4}  }}\n}}\n"
    );
    let (rows, source) = family_of(&fixture);
    let e = |line: &str, needle: &str| {
        (
            "invalid_type_argument_in_const_literal",
            at(&source, line, needle),
            "E".to_string(),
        )
    };
    assert_eq!(
        rows,
        [
            e(d, "E Function"),
            e(l2, "E"),
            e(l3, "E)"),
            e(l4, "E Function"),
            e(s2, "E"),
            e(m2, "E,"),
            e(m3, "E>"),
            e(m4, "E>>"),
        ],
        "{rows:#?}"
    );
}
