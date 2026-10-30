//! The #2913 pins: `unnecessary_type_check` over the
//! constructor-tear-off operand face (`C.constr`, `C.new`,
//! `C<int>.constr`, `p.C.constr`, `CAlias<int>.id`).
//!
//! The oracle's `_checkAllTypeChecks` reads the operand's
//! `leftNode.typeOrThrow`
//! (upstream Dart SDK source,
//! `pkg/analyzer/lib/src/dart/ast/best_practices_verifier.dart:1011`)
//! — a constructor selector on a container head carries the
//! tear-off's `FunctionType` (`ConstructorElement.type`: the
//! declared formals plus the class's own return head, with the
//! written type arguments substituted for `C<int>.constr` /
//! `CAlias<int>.id`). The merged member row resolves the selector
//! through the declared-constructor table
//! (`ctor_arity_target`'s pair), never the member walk, so the
//! verdict sees the function-typed operand the strict ask
//! (`resolved_subtype`'s Function arm) can decide constant-true.
//!
//! The written target folds the same `FunctionType` — a
//! `Function()` annotation completes through the capture's own
//! arm at `end_function_type`, so both sides spell the same
//! shape.

use super::test_rig::{report_ops, rows};

/// The bare named-constructor tear-off (`C.constr is C Function()`
/// — upstream `ConstructorReference` over a non-generic class):
/// the selector resolves to the declared constructor's signature
/// and the `is` is constant-true.
#[test]
fn ctor_tearoff_named_is_constant_true() {
    let report =
        report_ops("class C { C.constr(); }\nvoid f() {\n  if (C.constr is C Function()) {}\n}\n");
    let utc: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "unnecessary_type_check")
        .collect();
    assert_eq!(utc.len(), 1, "{:#?}", rows(&report));
    assert_eq!(
        utc[0].message,
        "Unnecessary type check; the result is always 'true'."
    );
}

/// The unnamed-constructor tear-off (`C.new is C Function()` —
/// `has_unnamed_constructor`'s pair; `new` names the unnamed
/// constructor in the selector position, upstream
/// `allowsNewAsIdentifier`).
#[test]
fn ctor_tearoff_new_is_constant_true() {
    let report = report_ops("class C { C(); }\nvoid f() {\n  if (C.new is C Function()) {}\n}\n");
    let utc: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "unnecessary_type_check")
        .collect();
    assert_eq!(utc.len(), 1, "{:#?}", rows(&report));
}

/// The instantiated receiver's tear-off (`C<int>.constr is C<int>
/// Function(int)`): the receiver's written type arguments join the
/// return head through the `call_type_args` entry keyed at the
/// head's own offset — the substituted signature the oracle's
/// `ConstructorElement.type` assigns.
#[test]
fn ctor_tearoff_instantiated_receiver_is_constant_true() {
    let report = report_ops(
        "class C<T> { C.constr(T t); }\nvoid f() {\n  if (C<int>.constr is C<int> Function(int)) {}\n}\n",
    );
    let utc: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "unnecessary_type_check")
        .collect();
    assert_eq!(utc.len(), 1, "{:#?}", rows(&report));
}

/// The alias-headed instantiated tear-off (`CAlias<int>.id` —
/// upstream resolves the grounded alias's expansion head, the
/// `alias_head_grounded` receiver fold): the same constructor
/// signature, with the written arguments substituted.
#[test]
fn ctor_tearoff_alias_head_is_constant_true() {
    let report = report_ops(
        "class C<T> { C.id(T t); }\ntypedef CAlias<T> = C<T>;\nvoid f() {\n  if (CAlias<int>.id is C<int> Function(int)) {}\n}\n",
    );
    let utc: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "unnecessary_type_check")
        .collect();
    assert_eq!(utc.len(), 1, "{:#?}", rows(&report));
}

/// The `var`-bound tear-off (`var c1 = C<int>.id; c1 is C<int>
/// Function()`): the initializer's static type is the tear-off's
/// own `FunctionType`, so the bound local answers the strict ask
/// constant-true (`initializer_static_ty`'s tear-off arm).
#[test]
fn ctor_tearoff_var_binding_is_constant_true() {
    let report = report_ops(
        "class C<T> { C.id(T t); }\nvoid f() {\n  var c1 = C<int>.id;\n  if (c1 is C<int> Function(int)) {}\n}\n",
    );
    let utc: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "unnecessary_type_check")
        .collect();
    assert_eq!(utc.len(), 1, "{:#?}", rows(&report));
}

/// An initializing formal without a written type reads its
/// field's declared type (`this.x` — the same face the creation
/// arity checks apply), so `C.constr is C Function(int)` still
/// answers constant-true.
#[test]
fn ctor_tearoff_field_formal_reads_field_ty() {
    let report = report_ops(
        "class C { final int x; C.constr(this.x); }\nvoid f() {\n  if (C.constr is C Function(int)) {}\n}\n",
    );
    let utc: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "unnecessary_type_check")
        .collect();
    assert_eq!(utc.len(), 1, "{:#?}", rows(&report));
}

/// A selector that names no declared constructor keeps the
/// operand's `Unknown` face — the every-check suppresses (the
/// under-report direction; an ordinary member of the same
/// container never routes here either).
#[test]
fn non_ctor_selector_stays_silent() {
    let report = report_ops(
        "class C { C.constr(); static void m() {} }\nvoid f() {\n  if (C.m is C Function()) {}\n}\n",
    );
    assert!(
        report
            .diagnostics
            .iter()
            .all(|d| d.code.name != "unnecessary_type_check"),
        "{:#?}",
        rows(&report)
    );
}

/// A genuinely-wrong target keeps the verdict informative — the
/// constructor's signature is no `C Function(int)` when the
/// declared formal spells `String`.
#[test]
fn ctor_tearoff_wrong_target_stays_silent() {
    let report = report_ops(
        "class C { C.constr(String s); }\nvoid f() {\n  if (C.constr is C Function(int)) {}\n}\n",
    );
    assert!(
        report
            .diagnostics
            .iter()
            .all(|d| d.code.name != "unnecessary_type_check"),
        "{:#?}",
        rows(&report)
    );
}
