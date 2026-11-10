//! The #3780 pins — the `use_of_void_result` MISSING side of the census
//! sweep (~212 rows; the extra side closed under #3767). Upstream
//! `checkForUseOfVoidResult` (error_detection_helpers.dart:245-258) fires on
//! every operand whose `staticType` is `void` where the enclosing construct
//! consumes the value — the clusters pinned here are the seams where the
//! oracle reports and the engine answered nothing.
//!
//! # Clusters (oracle-verified, `dart analyze` 3.13.7)
//!
//! 1. The for-in / await-for ELEMENT face (`_checkForEachParts`'s
//!    `sequenceElementType is VoidType && variableType is! VoidType` arm,
//!    error_verifier.dart:4478-4484): a `void` element iterated into a
//!    declared non-`void` or existing non-`void` loop variable reports at
//!    the ITERABLE (`.at(node.iterable)`). The wildcard declarator `Object? _`
//!    and the existing-variable head `for (object in xs)` report exactly
//!    like the declared `Object? o` head #3349 pinned.
//! 2. The `?`-marked collection operand (literal_element_verifier.dart:117
//!    /:152/:197): the null-aware marker TESTS the operand for `null`, so a
//!    `void` operand is used even when the stored slot type is also `void` —
//!    `<void>[?v]`, `<void, int>{?k: 0}`, `<int, void>{0: ?v}` all report at
//!    the operand expression.
//! 3. The `this` inside `extension on void` — `this`'s static type is
//!    `void`, so `this.toString()` consumes a `void` value at `this`.
//! 4. The composite ARGUMENT operand (`checkForArgumentTypeNotAssignable`,
//!    error_detection_helpers.dart:42-60): `use((v()))`, `use(c ? v() : v())`
//!    report at the whole argument expression — the single-row send path
//!    (`use(v())`, already green) reads the operand's `staticType` through
//!    the group's/conditional's own node, never through a bare leaf.
//! 5. The invoked `void`-typed callee and member (`function_expression_
//!    invocation_resolver.dart:86-96,214-222): `foo()()` invokes `foo()`'s
//!    `void` RESULT and `c.foo()` on `C<void>` invokes the `void` field —
//!    the callee's resolved static type is `void`, reporting at the callee
//!    expression.
//! 6. The `void` getter/function NAME read as a value upstream reports only
//!    when INVOKED (`c.foo()`/`foo()()` — a bare `c.name`/`top` read is no
//!    use; see the negative pins).
//!
//! Registered through `diagnose/tests.rs`'s `#[path]` block; the module
//! stays inside the `diagnose` subtree so `crate::diagnose::test_rig`
//! resolves.

use crate::diagnose::test_rig::{CORE_PRELUDE, report_ops, rows};
use crate::diagnose::types::FileReport;

/// The oracle's `useOfVoidResult` message spelling (messages.yaml:19638).
const VOID_MSG: &str = "This expression has a type of 'void' so its value can't be used.";

/// Asserts the fixture's `use_of_void_result` rows sit at exactly
/// `expected` `(offset, length)` pairs (offsets relative to the fixture's
/// own text, `CORE_PRELUDE` shifted). Other diagnostics a fixture's shape
/// invites are tolerated.
fn assert_void_rows(report: &FileReport, fixture: &str, expected: &[(usize, u32)]) {
    let mut got: Vec<(u32, u32)> = report
        .diagnostics
        .iter()
        .filter(|d| d.message == VOID_MSG)
        .map(|d| (d.range.offset, d.range.length))
        .collect();
    got.sort_unstable();
    let prelude = CORE_PRELUDE.len() as u32;
    let mut want: Vec<(u32, u32)> = expected
        .iter()
        .map(|(offset, length)| (prelude + *offset as u32, *length))
        .collect();
    want.sort_unstable();
    assert_eq!(got, want, "fixture={fixture:?}\nrows={:?}", rows(report));
}

/// The `(offset, length)` of the LAST `needle` in `fixture`.
fn span_of(fixture: &str, needle: &str) -> (usize, u32) {
    let at = fixture.rfind(needle).expect("needle");
    (at, needle.len() as u32)
}

/// Every `(offset, length)` of `needle` in `fixture`, ascending.
fn all_spans(fixture: &str, needle: &str) -> Vec<(usize, u32)> {
    fixture
        .match_indices(needle)
        .map(|(at, _)| (at, needle.len() as u32))
        .collect()
}

// —— Cluster 1: the for-in element face ——
//
// `for (Object? _ in List<void>)`: the wildcard declarator reports at the
// ITERABLE (`values`, len 6) — the oracle's `_checkForEachParts` arm reads
// `variableType is! VoidType` on the declared `Object?`/`dynamic` head, and
// the wildcard's `_` declares no readable name but still carries the
// declared type.
#[test]
#[ignore = "held: for_in_loop_var latch (listener.rs:3756) + for_in_void_element gate (engine_core.rs:12433) — wildcard `_` head never latches; do-not-touch files"]
fn ticket_3780_for_in_void_element_wildcard_head() {
    let fixture = "void f(List<void> values) {\n  for (Object? _ in values) {}\n  for (dynamic _ in values) {}\n}";
    let report = report_ops(fixture);
    // Each `values` iterable use reports at its own span (the `values`
    // inside the head, not the parameter list or a later row).
    let spans: Vec<(usize, u32)> = all_spans(fixture, "values) {}")
        .into_iter()
        .map(|(at, _)| (at, 6))
        .collect();
    assert_eq!(spans.len(), 2, "fixture shape: {fixture:?}");
    assert_void_rows(&report, fixture, &spans);
}

// `for (object in List<void>)` — an EXISTING (non-declared) loop variable
// whose declared type is non-`void` consumes the `void` element the same
// way: the iterable reports for both `Object? object` and `dynamic
// anything` heads.
#[test]
#[ignore = "held: existing-variable head — for_in_loop_var latch (listener.rs:3756, C::LocalVariableDeclaration only) + for_in_void_element (engine_core.rs:12433); do-not-touch files"]
fn ticket_3780_for_in_void_element_existing_variable() {
    let fixture = "void f(List<void> values) {\n  Object? object;\n  dynamic anything;\n  for (object in values) {\n    object;\n  }\n  for (anything in values) {\n    anything;\n  }\n}";
    let report = report_ops(fixture);
    let spans: Vec<(usize, u32)> = all_spans(fixture, "values) {")
        .into_iter()
        .map(|(at, _)| (at, 6))
        .collect();
    assert_eq!(spans.len(), 2, "fixture shape: {fixture:?}");
    assert_void_rows(&report, fixture, &spans);
}

// `await for (Object? _ in Stream<void>)`: the await-for's `Stream` element
// arm — same `variableType is! VoidType` gate against the declared head.
// `Stream` is declared in the fixture (the prelude carries `Iterable` only).
#[test]
#[ignore = "held: await-for Stream element — for_in_loop_var latch (listener.rs:3756) + for_in_void_element (engine_core.rs:12433); do-not-touch files"]
fn ticket_3780_await_for_void_stream_element() {
    let fixture = "class Stream<E> {}\nvoid f(Stream<void> values) async {\n  await for (Object? _ in values) {}\n  await for (dynamic _ in values) {}\n}";
    let report = report_ops(fixture);
    let spans: Vec<(usize, u32)> = all_spans(fixture, "values) {}")
        .into_iter()
        .map(|(at, _)| (at, 6))
        .collect();
    assert_eq!(spans.len(), 2, "fixture shape: {fixture:?}");
    assert_void_rows(&report, fixture, &spans);
}

// Negative control for cluster 1: a `var` head infers the element itself
// (a `void` head — upstream stays silent), and a plain `Iterable<int>` is
// no void use at all.
#[test]
fn ticket_3780_for_in_var_head_and_non_void_iterable_silent() {
    for fixture in [
        "void f(List<void> values) {\n  for (var o in values) {}\n}",
        "void f(List<int> values) {\n  for (Object? o in values) {}\n}",
    ] {
        let report = report_ops(fixture);
        let void_rows: Vec<_> = report
            .diagnostics
            .iter()
            .filter(|d| d.message == VOID_MSG)
            .collect();
        assert!(
            void_rows.is_empty(),
            "silent: {fixture:?}\nrows={:?}",
            rows(&report)
        );
    }
}

// —— Cluster 2: the `?`-marked collection operand ——
//
// The null-aware marker tests the operand for `null`: a `void` operand is
// consumed even into a `void` slot (`<void>[?value]` reports upstream at
// `value`, len 5).
#[test]
#[ignore = "held: ?-element operand — check_collection_void_elements (listener.rs:14258) keeps the `?` shapes out via top_level_question; upstream literal_element_verifier.dart:117/152 makes it unconditional; do-not-touch file"]
fn ticket_3780_null_aware_list_and_set_elements() {
    let list = "void f(void value) {\n  <void>[?value];\n}";
    let report = report_ops(list);
    assert_void_rows(&report, list, &[span_of(list, "value]")]);
    let set = "void f(void value) {\n  <void>{?value};\n}";
    let report = report_ops(set);
    assert_void_rows(&report, set, &[span_of(set, "value}")]);
}

// The map entry's `?key:`/`?value:` arms report the void operand (the `?`
// tests for null where the stored slot type would discard it).
#[test]
#[ignore = "held: ?key:/?value: map operand — check_collection_void_elements (listener.rs:14258); upstream literal_element_verifier.dart:197 unconditional when `?` non-null; do-not-touch file"]
fn ticket_3780_map_entry_question_key_and_value() {
    let key = "void f(void key) {\n  <void, int>{?key: 0};\n}";
    let report = report_ops(key);
    // The operand is `entry.key2` — `key` itself (len 3), `?`-led.
    let (at, _) = span_of(key, "key: 0");
    assert_void_rows(&report, key, &[(at, 3)]);
    let value = "void f(void value) {\n  <int, void>{0: ?value};\n}";
    let report = report_ops(value);
    let (at, _) = span_of(value, "value}");
    assert_void_rows(&report, value, &[(at, 5)]);
}

// —— Cluster 3: `this` inside `extension on void` ——
//
// `this`'s static type is `void` inside `extension on void`: a member
// access on it consumes the `void` value and reports at `this` (len 4).
#[test]
#[ignore = "held: `this` receiver typing inside `extension on void` — receiver type resolution lives in engine_core.rs/scopes_bodies.rs; do-not-touch files"]
fn ticket_3780_extension_on_void_this_use() {
    let fixture = "extension on void {\n  testVoid() {\n    this.toString();\n  }\n}";
    let report = report_ops(fixture);
    let (at, _) = span_of(fixture, "this.toString");
    // The operand is the `this` keyword itself (len 4).
    assert_void_rows(&report, fixture, &[(at, 4)]);
}

// —— Cluster 4: the composite argument operand ——
//
// `use((v()))`: the argument is the ParenthesizedExpression `(v())` — its
// static type is the inner `void`, and the report anchors the whole group.
#[test]
#[ignore = "held: parenthesized argument operand — the `)`-group row's operand_static_ty needs the inner-expression resolve in check_arguments (invocation.rs:~13521); do-not-touch file"]
fn ticket_3780_argument_parenthesized_void() {
    let fixture = "void use(Object? x) {}\nvoid v() {}\nvoid g() {\n  use((v()));\n}";
    let report = report_ops(fixture);
    assert_void_rows(&report, fixture, &[span_of(fixture, "(v())")]);
}

// Nested parens report the OUTERMOST group (`.at(expression)` covers the
// operand node itself): `use(((v())))`'s argument is `((v()))`, len 7.
#[test]
#[ignore = "held: nested parenthesized argument — same `)`-group inner resolve in check_arguments (invocation.rs:~13521); do-not-touch file"]
fn ticket_3780_argument_nested_parenthesized_void() {
    let fixture = "void use(Object? x) {}\nvoid v() {}\nvoid g() {\n  use(((v())));\n}";
    let report = report_ops(fixture);
    assert_void_rows(&report, fixture, &[span_of(fixture, "((v()))")]);
}

// `use(c ? v() : v())`: the conditional argument's LUB is `void` — the
// report anchors the whole conditional (`true ? v() : v()`).
#[test]
#[ignore = "held: conditional argument — the Value row's composite_lub resolves when fold_conditional stored ty=Void; the unmerged posture needs the conditional probe in check_arguments (invocation.rs:~13521); do-not-touch file"]
fn ticket_3780_argument_conditional_void() {
    let fixture = "void use(Object? x) {}\nvoid v() {}\nvoid g() {\n  use(true ? v() : v());\n}";
    let report = report_ops(fixture);
    assert_void_rows(&report, fixture, &[span_of(fixture, "true ? v() : v()")]);
}

// `use((x))` where `x` is a `void` local: the parenthesized operand keeps
// the inner `void` static type and reports the group.
#[test]
#[ignore = "held: parenthesized void-local argument — same `)`-group inner resolve in check_arguments (invocation.rs:~13521); do-not-touch file"]
fn ticket_3780_argument_parenthesized_void_local() {
    let fixture =
        "void use(Object? x) {}\nvoid v() {}\nvoid g() {\n  void x = v();\n  use((x));\n}";
    let report = report_ops(fixture);
    // The argument expression is `(x)` (the `use(` call's own parens are
    // the send's, not the operand's).
    assert_void_rows(&report, fixture, &[span_of(fixture, "(x)")]);
}

// —— Cluster 5: the invoked `void` callee ——
//
// `foo()()` invokes `foo()`'s `void` result — the callee expression `foo`
// reports (len 3), never the outer send.
#[test]
#[ignore = "held: invoked void result `foo()()` — consume_send's callee_is_invocation arm stores the callee's declared face, not function.typeOrThrow; needs operand_static_ty + anchor split (invocation.rs:~15013); do-not-touch file"]
fn ticket_3780_invoked_void_result() {
    let fixture = "void foo() {}\nmain() {\n  foo()();\n}";
    let report = report_ops(fixture);
    let (at, _) = span_of(fixture, "foo");
    assert_void_rows(&report, fixture, &[(at, 3)]);
}

// `c.foo()` on `C<void>` — `foo` is the `T`-typed field `void`: the callee
// expression `c.foo` reports (receiver through selector, len 5).
#[test]
#[ignore = "held: `c.foo()` on `C<void>` — function-typed field invoked through property access anchors receiver-through-selector; consume_send void arm + anchor split (invocation.rs:~15013); do-not-touch file"]
fn ticket_3780_invoked_void_generic_member() {
    let fixture = "class C<T> { T foo; C(this.foo); }\nvoid f(C<void> c) {\n  c.foo();\n}";
    let report = report_ops(fixture);
    assert_void_rows(&report, fixture, &[span_of(fixture, "c.foo")]);
}

// —— Negative pins ——
//
// A bare `void`-typed name read is no use: `c.name` (void getter) and the
// bare `top` function reference stay silent upstream — only INVOKING the
// void result consumes it.
#[test]
fn ticket_3780_bare_void_name_read_silent() {
    let fixture = "class C {\n  void get name => null;\n}\nvoid top() {}\nvoid g(C c) {\n  c.name;\n  top;\n}";
    let report = report_ops(fixture);
    let void_rows: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| d.message == VOID_MSG)
        .collect();
    assert!(
        void_rows.is_empty(),
        "bare void reads stay silent: {:?}",
        rows(&report)
    );
}

// `use(void x)` accepts the `void` actual — the formal's `void` face keeps
// the `checkForArgumentTypeNotAssignable` guard silent (the `use(v())`
// single-row path's negative control; #3762 pinned the same guard for
// binary operands).
#[test]
fn ticket_3780_void_formal_accepts_void_argument() {
    let fixture = "void use(void x) {}\nvoid v() {}\nvoid g() {\n  use(v());\n  use((v()));\n}";
    let report = report_ops(fixture);
    let void_rows: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| d.message == VOID_MSG)
        .collect();
    assert!(
        void_rows.is_empty(),
        "void formal accepts void actual: {:?}",
        rows(&report)
    );
}
