//! The #2919 member-send resolution pins: the sends whose receiver the
//! pass-2 row model now decides is potentially null, so the existing
//! `unchecked_use_of_nullable_value` faces fire where they used to fall
//! through.
//!
//! 1. The nullable implicit `this` inside `extension on A?`: the oracle's
//!    `type_property_resolver` runs its `isNullable` tail BEFORE the member
//!    lookup, so every member send on the extended `this` reports the
//!    shared unchecked code at the selector — a method invocation
//!    (`this.foo()`), a getter read (`this.foo`), a setter write
//!    (`this.foo = v`), the index operator (`this[i]`, `this[i] = v`), and
//!    the unary operator (`-this`). The `?.`/`?[...]` accesses stand down
//!    (the null-aware latch), and the extension's own `on`-type members
//!    resolve through the applicable extension (silent).
//! 2. The accessor-result callee function check (`x.first()` over
//!    `List<T?>` where `T extends Function`): a method/send whose resolved
//!    member is a field/getter and whose RESULT is a nullable function
//!    shape reports the function variant — the callee expression is a
//!    `FunctionExpressionInvocation`, never the member-send arm.
//! 3. The three-segment creation head (`unresolved.Class<int>.new()`): the
//!    `undefined_identifier` reports the `unresolved.Class` span (through
//!    the class-name token), never the `?` generic tail — the latched
//!    `creation_ref` selector's end, not the period's offset.
//!
//! Oracle provenance: the #2919 census rows —
//! `UncheckedUseOfNullableValueInsideExtensionTest/test_methodInvocation_
//! nullable#0`, `test_propertyAccess_getter_nullable#0`,
//! `test_propertyAccess_setter_nullable#0`, `test_indexExpression_
//! nullable#0`, `test_prefixExpression_minus_nullable#0`, and
//! `UncheckedUseOfNullableValueTest/test_member_potentiallyNullable_
//! called#0` (`dart analyze`, Dart 3.13).

use super::test_rig::report_ops;
use crate::diagnose::FileReport;

/// The `unchecked_use_of_nullable_value` rows of `report`, sorted by offset
/// — `(offset, length, message)`.
fn unchecked_rows(report: &FileReport) -> Vec<(u32, u32, String)> {
    let mut rows: Vec<(u32, u32, String)> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "unchecked_use_of_nullable_value")
        .map(|d| (d.range.offset, d.range.length, d.message.to_string()))
        .collect();
    rows.sort();
    rows
}

/// `extension E on A?` with `foo` declared on `A`: `this.foo()` reports the
/// method variant at the selector; `this?.foo()` and the extension's own
/// `this.bar()` stand down. Census
/// `test_methodInvocation_nullable#0`.
#[test]
fn pin_method_invocation_nullable_this() {
    let report = report_ops(
        "class A {\n\
         \x20 void foo() {}\n\
         }\n\
         extension E on A? {\n\
         \x20 void bar() {}\n\
         \x20 void baz() {\n\
         \x20   this.foo();\n\
         \x20   this?.foo();\n\
         \x20   this.bar();\n\
         \x20 }\n\
         }\n",
    );
    let rows = unchecked_rows(&report);
    assert_eq!(rows.len(), 1, "only this.foo() reports: {rows:?}");
    let (_, _, message) = &rows[0];
    assert!(
        message.contains("The method 'foo' can't be unconditionally invoked"),
        "the method variant: {rows:?}"
    );
}

/// `extension E on A?` with `int get foo` on `A`: `this.foo` reports the
/// property variant; `this?.foo` stands down. Census
/// `test_propertyAccess_getter_nullable#0`.
#[test]
fn pin_property_access_getter_nullable_this() {
    let report = report_ops(
        "class A {\n\
         \x20 int get foo => 0;\n\
         }\n\
         extension E on A? {\n\
         \x20 void baz() {\n\
         \x20   this.foo;\n\
         \x20   this?.foo;\n\
         \x20 }\n\
         }\n",
    );
    let rows = unchecked_rows(&report);
    assert_eq!(rows.len(), 1, "only this.foo reports: {rows:?}");
    let (_, _, message) = &rows[0];
    assert!(
        message.contains("The property 'foo' can't be unconditionally accessed"),
        "the property variant: {rows:?}"
    );
}

/// `extension E on A?` with `set foo` on `A`: the qualified write
/// `this.foo = v` reports the PROPERTY variant (the oracle's setter
/// spelling — the bare `foo = v` write reads as the method send, the
/// `this.`-qualified write reads as the property access). Census
/// `test_propertyAccess_setter_nullable#0`.
#[test]
fn pin_property_access_setter_nullable_this() {
    let report = report_ops(
        "class A {\n\
         \x20 set foo(int v) {}\n\
         }\n\
         extension E on A? {\n\
         \x20 void baz() {\n\
         \x20   this.foo = 0;\n\
         \x20   this?.foo = 0;\n\
         \x20 }\n\
         }\n",
    );
    let rows = unchecked_rows(&report);
    assert_eq!(rows.len(), 1, "only this.foo = 0 reports: {rows:?}");
    let (_, _, message) = &rows[0];
    assert!(
        message.contains("The property 'foo' can't be unconditionally accessed"),
        "the setter property variant: {rows:?}"
    );
}

/// `extension E on A?` with `operator []`/`[]=` on `A`: `this[0]` and
/// `this[0] = v` each report ONE `[]` method row at the bracket; the
/// `?[...]` accesses stand down. Census `test_indexExpression_nullable#0`.
#[test]
fn pin_index_expression_nullable_this() {
    let report = report_ops(
        "class A {\n\
         \x20 int operator[](int i) => 0;\n\
         \x20 operator[]=(int i, int v) {}\n\
         }\n\
         extension E on A? {\n\
         \x20 void bar() {\n\
         \x20   this[0];\n\
         \x20   this?[0];\n\
         \x20   this[0] = 0;\n\
         \x20   this?[0] = 0;\n\
         \x20 }\n\
         }\n",
    );
    let rows = unchecked_rows(&report);
    assert_eq!(
        rows.len(),
        2,
        "this[0] and this[0] = 0 each report: {rows:?}"
    );
    for (_, _, message) in &rows {
        assert!(
            message.contains("The method '[]' can't be unconditionally invoked"),
            "the index method variant: {rows:?}"
        );
    }
}

/// `extension E on A?` with `operator-` on `A`: `-this` reports `unary-` at
/// the operator token. Census `test_prefixExpression_minus_nullable#0`.
#[test]
fn pin_prefix_expression_minus_nullable_this() {
    let report = report_ops(
        "class A {\n\
         \x20 A operator-() => this;\n\
         }\n\
         extension E on A? {\n\
         \x20 void bar() {\n\
         \x20   -this;\n\
         \x20 }\n\
         }\n",
    );
    let rows = unchecked_rows(&report);
    assert_eq!(rows.len(), 1, "only -this reports: {rows:?}");
    let (_, _, message) = &rows[0];
    assert!(
        message.contains("The method 'unary-' can't be unconditionally invoked"),
        "the unary- method variant: {rows:?}"
    );
}

/// `x.first()` over `List<T?>` where `T extends Function`: the resolved
/// member is `first` (a getter whose result is `T?`), and the invocation
/// applies that nullable function — the oracle spells the FUNCTION variant
/// (`FunctionExpressionInvocation`), never the member-send arm. Census
/// `test_member_potentiallyNullable_called#0`.
#[test]
fn pin_member_callee_function_nullable() {
    let report = report_ops(
        "m<T extends Function>(List<T?> x) {\n\
         \x20 x.first();\n\
         }\n",
    );
    let rows = unchecked_rows(&report);
    assert_eq!(rows.len(), 1, "x.first() reports: {rows:?}");
    let (_, _, message) = &rows[0];
    assert!(
        message.contains("The function can't be unconditionally invoked"),
        "the function variant: {rows:?}"
    );
}

/// The three-segment creation head `unresolved.Class<int>.new()`: the
/// `undefined_identifier` reports the `unresolved.Class` span — the head
/// starts at `unresolved` and ends at the class-name token, never through
/// the `<int>` tail. The `?` generic tail is not part of the resolved type
/// name (the official importPrefix arm's `diagnostic.at(node.name)`).
#[test]
fn pin_creation_head_span_tail() {
    let report = report_ops("var x = unresolved.Class<int>.new();\n");
    let rows: Vec<(u32, u32, String)> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "undefined_identifier")
        .map(|d| (d.range.offset, d.range.length, d.message.to_string()))
        .collect();
    assert_eq!(rows.len(), 1, "one undefined_identifier: {rows:?}");
    let (offset, length, _) = rows[0];
    // `unresolved.Class` spans 16 columns starting 8 columns into the
    // fixture (`var x = `) — the rig prepends `CORE_PRELUDE`, so the
    // absolute anchor is the prelude length plus the fixture-relative 8.
    // The head must NOT extend through `<int>` or `.new`.
    let source_len = "unresolved.Class".len() as u32;
    let fixture_base = super::test_rig::CORE_PRELUDE.len() as u32;
    assert_eq!(length, source_len, "the class-name span: {rows:?}");
    assert_eq!(offset, fixture_base + 8, "the head offset: {rows:?}");
}
