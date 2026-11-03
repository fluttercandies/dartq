//! #3748 — `invalid_assignment` convergence pins.
//!
//! Shape 1.2: a generic function type's quantifier bounds are part of the
//! subtype question, and the declared head of a raw `C` annotation
//! instantiate-to-bounds fills to `C<T Function<T extends num>(T)>` — the
//! face the oracle's message prints. Every written argument below fails
//! the bound's subtype leg (`T` is not `int`, an unbounded `T` cannot prove
//! `extends num`, and a narrower bound is not the bound itself), so the
//! oracle reports `invalid_assignment` for all three.

use super::test_rig::{report_ops, rows};

fn invalid_assignments(source: &str) -> Vec<String> {
    rows(&report_ops(source))
        .into_iter()
        .filter(|row| row.starts_with("invalid_assignment "))
        .collect()
}

fn assert_reports(source: &str, from: &str) {
    let found = invalid_assignments(source);
    let expected = format!(
        "A value of type '{from}' can't be assigned to a variable of type 'C<T Function<T extends num>(T)>'."
    );
    assert!(
        found.iter().any(|row| row.ends_with(&expected)),
        "{found:?}"
    );
}

/// An unbounded `T` cannot prove the `extends num` bound.
#[test]
fn generic_fn_arg_unbounded_src_reports() {
    assert_reports(
        "class C<X extends T Function<T extends num>(T)> {}\n\
         main() {\n\
           C c1 = C<T Function<T>(T)>();\n\
         }\n",
        "C<T Function<T>(T)>",
    );
}

/// A narrower `extends int` bound is not the `extends num` bound itself.
#[test]
fn generic_fn_arg_narrower_bound_reports() {
    assert_reports(
        "class C<X extends T Function<T extends num>(T)> {}\n\
         main() {\n\
           C c2 = C<int Function<T extends int>(int)>();\n\
         }\n",
        "C<int Function<T extends int>(int)>",
    );
}

/// The `on` face's `?` is part of the extended type: `extension E on int?`
/// accepts a `Null` receiver, so `E(null)?.f()` reports no
/// `extension_override_argument_not_assignable` (the oracle emits only the
/// `?.f()` `dead_code`). Pre-fix the bare `int` head rejected `Null` and
/// fabricated the row — and an emitted diagnostic on the receiver suppresses
/// the null-aware `dead_code` walk that owns the `?.` tail.
#[test]
fn extension_override_nullable_on_accepts_null_receiver() {
    let report = super::test_rig::report_ops(
        "extension E on int? {\n  void f() {}\n}\nmain() {\n  E(null)?.f();\n}\n",
    );
    assert!(
        !super::test_rig::code_names(&report)
            .contains(&"extension_override_argument_not_assignable"),
        "{:?}",
        super::test_rig::rows(&report)
    );
    // The rig emits no `dead_code`/`dead_null_aware_expression` for `?.`
    // (the null-aware tail walk is conformance-harness-level), so the pin
    // holds at the arg-not-assignable absence — the verified fix.
}

/// The `??=` callable coercion through an explicit `Extension(s)?.`
/// override: the setter's `Function?` write type accepts a `call`-
/// declaring class via its `call` signature, so the oracle reports only
/// `invalid_null_aware_operator` (the `?.` on the non-null `s`) — never
/// `invalid_assignment` on the value nor `type_argument_not_matching_bounds`
/// on the `expectStaticType` bound. The full inference_update_3 census face
/// (~110 rows of `CallableClass<..> → Function?`) reduces to this arm.
#[test]
fn callable_class_if_null_extension_write_reports_no_assignment() {
    let report = super::test_rig::report_ops(
        "Object? context<T>(T x) => x;\n\
         class CallableClass<T> { T call() => throw ''; }\n\
         extension Extension on String {\n\
           Function? get pFunctionQuestion => null;\n\
           set pFunctionQuestion(Function? value) {}\n\
         }\n\
         typedef Exactly<T> = T Function(T);\n\
         extension StaticType<T> on T {\n\
           T expectStaticType<R extends Exactly<T>>() => this;\n\
         }\n\
         T contextType<T>(Object? result) => result as T;\n\
         main() {\n\
           var s = '' as String?;\n\
           var callableClassInt = CallableClass<int>();\n\
           context<Function?>(\n\
             (Extension(s)?.pFunctionQuestion ??= callableClassInt)\n\
               ..expectStaticType<Exactly<Function?>>(),\n\
           );\n\
         }\n",
    );
    let codes = super::test_rig::code_names(&report);
    assert!(
        !codes.contains(&"invalid_assignment")
            && !codes.contains(&"type_argument_not_matching_bounds"),
        "{:?}",
        super::test_rig::rows(&report)
    );
}

/// The `??=` callable coercion on the bare `Function?` target
/// (`resolveIfNull`'s `T2'`): a `call`-declaring class joins the write as
/// its `call` signature, so `local ??= callableClass` against a
/// `Function?` local writes `Function` and reports NO
/// `invalid_assignment` — and the join types the `expectStaticType`
/// receiver `Function` (binding `T := Function`), so
/// `Exactly<Function>` satisfies the `R extends Exactly<T>` bound.
/// The oracle is fully silent; pre-fix the write's declared-closure
/// negative overturned the nullable Right-Top `true` (a nominal
/// `CallableClass <: Function` refusal that ignores the `.call`
/// acceptance) and the `??=` join dropped the `T2'` coercion, typing the
/// receiver `Object` and fabricating `Exactly<Object>` as the bound.
#[test]
fn if_null_callable_write_and_join_stay_silent() {
    let report = super::test_rig::report_ops(
        "class CallableClass<T> { T call() => throw ''; }\n\
         typedef Exactly<T> = T Function(T);\n\
         extension StaticType<T> on T {\n\
           T expectStaticType<R extends Exactly<T>>() => this;\n\
         }\n\
         main() {\n\
           Function? local;\n\
           var callableClassInt = CallableClass<int>();\n\
           (local ??= callableClassInt).expectStaticType<Exactly<Function>>();\n\
         }\n",
    );
    assert!(
        super::test_rig::code_names(&report).is_empty(),
        "{:?}",
        super::test_rig::rows(&report)
    );
}

/// The `= e` default against an UNRESOLVED formal annotation: upstream's
/// `paramElement.type` for `{A a = null}` is `InvalidType`, which every
/// assignability question short-circuits — the oracle reports only
/// `undefined_class`, never `invalid_assignment`. (The function-typed
/// formal `{A a() = null}` is a different gap: the oracle's
/// `InvalidType Function()` DOES emit `invalid_assignment`, but dartq's
/// `formal_written` never latches for old-style `a()` formals — their
/// name token is swallowed inline — so the check never runs. Out of
/// this shape's scope.)
#[test]
fn default_value_against_unresolved_formal_annotation_stays_silent() {
    // `{A a : null}` — the colon separator is its own lint face; the pin
    // targets only the absent `invalid_assignment`.
    let report = super::test_rig::report_ops("void f({A a : null}) {}\n");
    let codes = super::test_rig::code_names(&report);
    assert!(
        !codes.contains(&"invalid_assignment"),
        "{:?}",
        super::test_rig::rows(&report)
    );

    // `[A a = null]` — same contract under the positional separator.
    let report = super::test_rig::report_ops("void g([A a = null]) {}\n");
    let codes = super::test_rig::code_names(&report);
    assert!(
        !codes.contains(&"invalid_assignment"),
        "{:?}",
        super::test_rig::rows(&report)
    );

    // `covariant final A a : null` — the modifiers are extraneous
    // (`extraneous_modifier` is the parser face the census counts
    // separately); the default-value half keeps the same contract.
    let report = super::test_rig::report_ops("void h({covariant final A a : null}) {}\n");
    let codes = super::test_rig::code_names(&report);
    assert!(
        !codes.contains(&"invalid_assignment"),
        "{:?}",
        super::test_rig::rows(&report)
    );
}

// The residual this shape does not close: `C<int Function<T extends
// num>(int)>` fails the SAME way upstream — the argument's `int` return
// and parameter are not the bound's quantified `T` — but the engine's
// `slot_subtype` still answers `int <: T` through `T`'s `num` bound (a
// concrete type is never a subtype of a bare type parameter upstream;
// `slot_subtype`'s `TypeParameterType` leg is a separate fix, wider than
// this shape's bound-capture surface). Left unpinned until that leg lands.
