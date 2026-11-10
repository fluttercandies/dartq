//! #3796 pins — `undefined_operator` cascade + extension-family faces:
//! four oracle-verified contracts landed in `row_resolution.rs`.
//!
//! Oracle contract (upstream Dart SDK source,
//! pkg/analyzer/lib/src/dart/resolver/binary_expression_resolver.dart):
//! a failed binary-operator lookup stamps the composite `InvalidType`,
//! and `InvalidType` suppresses every downstream operator face — so
//! `(a < 5) < 2` reports the inner `<` once, never the outer. Extension
//! operator families (`[]`/`[]=` share one basename in
//! applicable_extensions.dart) and implicit `this`-receiver extension
//! application follow the same oracle text.
//!
//! Probed against `dart analyze` 3.13.7 (the pinned oracle at
//! `~/.dartq-oracle/3.13.7/dart-sdk/bin/dart analyze`).

use crate::diagnose::test_rig::{code_names, report_ops};
use crate::diagnose::types::FileReport;

/// Every `undefined_operator` row's `(offset, length)` in report order.
fn undefined_operator_rows(report: &FileReport) -> Vec<(u32, u32)> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "undefined_operator")
        .map(|d| (d.range.offset, d.range.length))
        .collect()
}

/// `(a < 5) < 2` — the inner `<` on `bool` fails once and stamps the
/// group `InvalidType`; the outer `<` on that face suppresses (oracle:
/// exactly one row at the inner operator).
///
/// Probed: 1 row — `The operator '<' isn't defined for the class 'bool'.`
/// at the inner `<`, the outer silent.
#[test]
fn failed_inner_comparison_suppresses_outer_operator() {
    let fixture = "void f(int a) {\n\
         \x20 (a < 5) < 2;\n\
         }\n";
    let report = report_ops(fixture);
    let rows = undefined_operator_rows(&report);
    assert_eq!(rows.len(), 1, "expected exactly one row, rows={rows:?}");
    // Exactly one row, and the message names `bool` — the outer `<` is
    // suppressed by the group face's InvalidType latch. (The row's span
    // is the inner `<`; offset arithmetic is pinned by the message face.)
    let msg = &report
        .diagnostics
        .iter()
        .find(|d| d.code.name == "undefined_operator")
        .unwrap()
        .message;
    assert!(msg.contains("'<'"), "row must be the `<` face: {msg}");
    assert!(msg.contains("'bool'"), "row must name class bool: {msg}");
}

/// `(confuse(a) < 5) < 2` — a comparison on a `dynamic` operand types the
/// composite `dynamic` (upstream `binary_expression_resolver.dart` forwards
/// the receiver face for dynamic receivers), so the outer `<` dispatches
/// dynamically and reports nothing — even though the latch stamps `bool`
/// on the comparison shape. The mirror contract: `bool < 2` DOES report.
///
/// Probed: `confuse` returning `dynamic` → 0 rows on
/// `(confuse(a) < 5) < 2`; `bool b; b < 2` →
/// `The operator '<' isn't defined for the type 'bool'.`
#[test]
fn dynamic_comparison_result_dispatches_dynamically() {
    // The declared-dynamic operand: `d < 5` types `dynamic`.
    let declared = report_ops(
        "void f() {\n\
         \x20 dynamic d = 1;\n\
         \x20 (d < 5) < 2;\n\
         }\n",
    );
    assert_eq!(
        undefined_operator_rows(&declared),
        Vec::<(u32, u32)>::new(),
        "declared dynamic composite must stay silent, rows={:?}",
        code_names(&declared)
    );
    // The send-result dynamic: `confuse(a)` types `dynamic` from the
    // written return, so the `<` on it forwards `dynamic`.
    let sent = report_ops(
        "dynamic confuse(Object o) => o;\n\
         void f(Object a) {\n\
         \x20 (confuse(a) < 5) < 2;\n\
         }\n",
    );
    assert_eq!(
        undefined_operator_rows(&sent),
        Vec::<(u32, u32)>::new(),
        "send-result dynamic composite must stay silent, rows={:?}",
        code_names(&sent)
    );
    let loud = report_ops(
        "void f() {\n\
         \x20 bool b = true;\n\
         \x20 b < 2;\n\
         }\n",
    );
    assert_eq!(
        undefined_operator_rows(&loud).len(),
        1,
        "the real bool operand must still report, codes={:?}",
        code_names(&loud)
    );
}

/// `this + 2` inside `extension E on C { operator + }` — the implicit
/// `this` receiver resolves the enclosing extension's `+`; oracle silent.
///
/// Probed: 0 rows — `dart analyze` reports nothing on `this + 2` when
/// the extension itself declares `operator +`.
#[test]
fn implicit_this_inside_extension_resolves_operator() {
    let report = report_ops(
        "class C {}\n\
         extension E on C {\n\
         \x20 C operator +(int other) => this;\n\
         \x20 C m() {\n\
         \x20   return this + 2;\n\
         \x20 }\n\
         }\n\
         void g() {\n\
         \x20 C().m();\n\
         }\n",
    );
    assert_eq!(
        undefined_operator_rows(&report),
        Vec::<(u32, u32)>::new(),
        "rows={:?}",
        code_names(&report)
    );
}

/// `a[0] = v` on `class A { operator [](int i) }` + `extension E on A
/// { operator []= }` — `[]`/`[]=` are one extension family: the
/// interface declaring EITHER member blocks ALL implicit extension
/// index resolution (upstream applicable_extensions.dart resolves both
/// spellings under the `[]` basename). Oracle: `[]=` misses on `A`
/// even though only `[]` is declared there.
///
/// Probed: 1 row — `The operator '[]=' isn't defined for the class 'A'.`
#[test]
fn declared_index_read_blocks_extension_index_write() {
    let report = report_ops(
        "class A {\n\
         \x20 int operator [](int i) => i;\n\
         }\n\
         extension E on A {\n\
         \x20 void operator []=(int i, int v) {}\n\
         }\n\
         void f(A a) {\n\
         \x20 a[0] = 1;\n\
         }\n",
    );
    let rows = undefined_operator_rows(&report);
    assert_eq!(
        rows.len(),
        1,
        "expected exactly one `[]=` row, codes={:?}",
        code_names(&report)
    );
    let msg = &report
        .diagnostics
        .iter()
        .find(|d| d.code.name == "undefined_operator")
        .unwrap()
        .message;
    assert!(msg.contains("[]="), "row must be the `[]=` face: {msg}");
    assert!(msg.contains("'A'"), "row must name class A: {msg}");
}

/// `x++` on a `Never` receiver — the postfix face forwards the operand
/// type; `Never` suppresses every operator face (oracle silent).
///
/// Probed: 0 rows on `x++` where `x` is `Never`.
#[test]
fn postfix_increment_on_never_silent() {
    let report = report_ops(
        "void f(Never x) {\n\
         \x20 x++;\n\
         }\n",
    );
    assert_eq!(
        undefined_operator_rows(&report),
        Vec::<(u32, u32)>::new(),
        "rows={:?}",
        code_names(&report)
    );
}

/// The extension-internal basename shadow: inside `extension E on A`
/// declaring only `set topLevelGetter`, the bare READ of `topLevelGetter`
/// never falls through to the top-level getter — the basename claim
/// blocks the enclosing scope (upstream `ExtensionMemberScope`,
/// lexical_lookup.dart).
///
/// Probed: `undefined_identifier` on both bare reads.
#[test]
fn extension_setter_shadows_top_level_getter() {
    let report = report_ops(
        "int get topLevelGetter => -1;\n\
         class A {}\n\
         extension E on A {\n\
         \x20 void set topLevelGetter(int _) {}\n\
         \x20 void test() {\n\
         \x20   topLevelGetter;\n\
         \x20   var y = topLevelGetter + 1;\n\
         \x20 }\n\
         }\n",
    );
    let misses: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "undefined_identifier")
        .collect();
    assert_eq!(
        misses.len(),
        2,
        "expected two shadowed-read rows, codes={:?}",
        code_names(&report)
    );
    assert!(misses.iter().all(|d| d.message.contains("topLevelGetter")));
}

/// The shadow's invocation twin: `topLevelMethod(1)` inside the
/// extension declaring only `set topLevelMethod` falls to `this` — the
/// `on` type `A`'s member lookup — and reports `undefined_method`
/// naming `A`.
///
/// Probed: `undefined_method` 'topLevelMethod' isn't defined for the
/// type 'A'.
#[test]
fn extension_setter_shadowed_invoke_names_on_type() {
    let report = report_ops(
        "int topLevelMethod(int x) => -4;\n\
         class A {}\n\
         extension E on A {\n\
         \x20 void set topLevelMethod(int _) {}\n\
         \x20 void test() {\n\
         \x20   topLevelMethod(1);\n\
         \x20 }\n\
         }\n",
    );
    let misses: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "undefined_method")
        .collect();
    assert_eq!(
        misses.len(),
        1,
        "expected one on-type method-miss row, codes={:?}",
        code_names(&report)
    );
    let msg = &misses[0].message;
    assert!(
        msg.contains("topLevelMethod") && msg.contains("'A'"),
        "row: {msg}"
    );
}

/// The bare write inside an extension body falls to `this` — the `on`
/// type's `name=` member, inherited or through another applicable
/// extension (`setterInInstanceScope`/`setterInOtherExtensionScope`
/// over `extension MyExt on A`, upstream `type_property_resolver`'s
/// member walk + `findExtension`).
///
/// Probed: silent — `x = v` over `A`'s `set x` resolves.
#[test]
fn extension_body_write_resolves_on_type_setter() {
    let report = report_ops(
        "class A {\n\
         \x20 void set x(int v) {}\n\
         }\n\
         extension E on A {\n\
         \x20 void test() {\n\
         \x20   x = 1;\n\
         \x20 }\n\
         }\n",
    );
    assert_eq!(
        code_names(&report),
        Vec::<&'static str>::new(),
        "the on-type setter resolves the write silently"
    );
}

/// The implicit-`this` write climbs the whole hierarchy — `x = 2`
/// inside `B extends A` resolves `A`'s inherited `set x`.
///
/// Probed: silent.
#[test]
fn bare_write_resolves_inherited_setter() {
    let report = report_ops(
        "class A {\n\
         \x20 void set x(int v) {}\n\
         }\n\
         class B extends A {\n\
         \x20 void f() {\n\
         \x20   x = 2;\n\
         \x20 }\n\
         }\n",
    );
    assert_eq!(
        code_names(&report),
        Vec::<&'static str>::new(),
        "the inherited setter resolves the write silently"
    );
}

/// The type-literal resolution: a bare name spelling an in-scope type
/// parameter evaluates to `Type` — `var a = X` inside `C1<X>`'s
/// instance context stays silent (probed 3.13.7).
#[test]
fn type_parameter_read_is_silent() {
    let report = report_ops(
        "class C<X> {\n\
         \x20 void f() {\n\
         \x20   var a = X;\n\
         \x20 }\n\
         }\n",
    );
    assert!(
        !code_names(&report).contains(&"undefined_identifier"),
        "X must resolve the Type literal, codes={:?}",
        code_names(&report)
    );
}

/// A generic tear-off's `undefined_operator` display keeps the
/// signature's OWN type-parameter group — `f<T1, T2> < g` reports on
/// `String Function<T1, T2>(int)` (upstream `writeFunctionType`).
///
/// Probed: `The operator '<' isn't defined for the type
/// 'String Function<T1, T2>(int)'.`
#[test]
fn generic_tearoff_operator_display_keeps_type_params() {
    let report = report_ops(
        "String f<T1, T2>(int x) => '';\n\
         void g() {\n\
         \x20 f < g;\n\
         }\n",
    );
    let row = report
        .diagnostics
        .iter()
        .find(|d| d.code.name == "undefined_operator");
    let Some(row) = row else {
        panic!(
            "expected the `<` undefined_operator row, codes={:?}",
            code_names(&report)
        );
    };
    assert!(
        row.message.contains("Function<T1, T2>"),
        "display must keep the type-parameter group: {}",
        row.message
    );
}

/// A bare read inside `B extends A` losing to two applicable extensions
/// reports the ambiguity verdict AND `undefined_identifier` — the oracle
/// double-reports (upstream `static_extension_internal_resolution_4_error`:
/// `fieldInExtensionScope` inside `class B extends A` where `MyExt` and
/// `ExtraExt` both answer it).
///
/// Probed: `ambiguous_extension_member_access` + `undefined_identifier` at
/// the same span.
#[test]
fn class_context_ambiguous_extension_read_double_reports() {
    let report = report_ops(
        "class A {}\n\
         extension MyExt on A {\n\
         \x20 int get m => 0;\n\
         }\n\
         extension ExtraExt on A {\n\
         \x20 int get m => 0;\n\
         }\n\
         class B extends A {\n\
         \x20 void f() {\n\
         \x20   var t = m;\n\
         \x20 }\n\
         }\n",
    );
    let names = code_names(&report);
    assert!(
        names.contains(&"ambiguous_extension_member_access"),
        "the ambiguity verdict must fire, codes={names:?}"
    );
    assert!(
        names.contains(&"undefined_identifier"),
        "the lexical miss rides beside it, codes={names:?}"
    );
}

/// The class-context extension write rescue: `m = 1` inside `B extends A`
/// resolves `extension on A`'s `set m` through the implicit `this` — the
/// interface lookup missed, the application wins (probed silent, the
/// `static_extension_internal_resolution_4_test` `setterInOtherExtensionScope`
/// family).
#[test]
fn class_context_extension_setter_resolves_bare_write() {
    let report = report_ops(
        "class A {}\n\
         extension MyExt on A {\n\
         \x20 set m(int v) {}\n\
         }\n\
         class B extends A {\n\
         \x20 void f() {\n\
         \x20   m = 1;\n\
         \x20 }\n\
         }\n",
    );
    assert_eq!(
        code_names(&report),
        Vec::<&'static str>::new(),
        "the extension setter resolves the write silently"
    );
}

/// The local-function tear-off rescue: `scheduleMicrotask(next)` reads
/// a nested function through the same visible region the invocation
/// rescue walks (the push-time decl latch drops it).
///
/// Probed: silent — upstream binds the local function before its body.
#[test]
fn local_function_tearoff_silent() {
    let report = report_ops(
        "void scheduleMicrotask(void Function() f) {}\n\
         void outer() {\n\
         \x20 void next() {}\n\
         \x20 scheduleMicrotask(next);\n\
         }\n",
    );
    assert!(
        !code_names(&report).contains(&"undefined_identifier"),
        "the local function tear-off resolves, codes={:?}",
        code_names(&report)
    );
}
