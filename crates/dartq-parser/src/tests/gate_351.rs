//! Ticket #351: the six #339 forensic gap families, pinned green against
//! their official anchors.
//!
//! Each family backs one or more dartq-semantic REDs (#339 in-code
//! annotations: `collector.rs::captures_formals` and the `diagnose.rs`
//! await/arity/null block). Families 1 and 3 were already satisfied at
//! HEAD 45196c8 (the #333 formal-gate rewrite and the statement-level
//! `computeType` follower vetoes) — the pins below keep them green. The
//! await family (2) is a non-gap on the parser side: the official parser
//! keeps `await x;` a variable declaration in a sync body
//! (`parser_impl.dart:6797-6809`; `looksLikeAwaitExpression` is false in
//! statement context) and the oracle reports the misuse from the type
//! annotation (`named_type_resolver.dart:814-818`), so the declaration
//! shape pinned here is the correct one.

use super::collector::{Collector, setup};

fn parse(source: &'static str) -> Collector {
    let mut p = setup(source);
    p.parse_unit();
    p.listener
}

// —— family 1: committed-return formal gate (official parser_impl.dart
// :2221-2264: type + tentative name + `(` is an inline function-typed
// formal regardless of the type shape) ————————————————————————————————————

/// `void cb(int inner)?` enters the inline function-type path; the trailing
/// `?` and the following formal `outer` survive. Backs the semantic
/// `captures_formals` RED (named table `[cb, outer]`).
#[test]
fn committed_return_function_typed_formal_keeps_group() {
    let c = parse("void g({void cb(int inner)?, int outer = 1}) {}");
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    c.assert_sequence(&[
        ("begin", "optional_formal_parameters"),
        ("begin", "function_typed_formal"),
        ("begin", "formal_parameters"),
        ("end", "formal_parameters"),
        ("end", "function_typed_formal"),
        ("end", "optional_formal_parameters"),
        ("end", "formal_parameters"),
    ]);
}

// —— family 4: `<A, A>` application scan in statement position (official
// `computeMethodTypeArguments` + `handleTypeArgumentApplication`,
// parser_impl.dart:7104-7125; `_mayFollowTypeArgs`, type_info.dart:511) ———

/// A generic instantiation as a statement head applies its type arguments
/// without an invocation. Backs the semantic `function_reference_arity` RED.
#[test]
fn function_reference_application_event() {
    let c = parse("class A {} void g<T>() {} void f() { g<A, A>; }");
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    c.assert_sequence(&[
        ("begin", "type_arguments"),
        ("end", "type_arguments"),
        ("handle", "type_argument_application"),
        ("handle", "expression_statement"),
    ]);
}

/// An implicit call reference (`c<A, A>;` on a callable class) takes the
/// same application path. Backs the semantic `implicit_call_reference` RED.
#[test]
fn implicit_call_reference_application_event() {
    let c = parse("class C { T call<T>(T x) => x; } void f(C c) { c<A, A>; }");
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    c.assert_sequence(&[
        ("begin", "type_arguments"),
        ("end", "type_arguments"),
        ("handle", "type_argument_application"),
        ("handle", "expression_statement"),
    ]);
}

/// The `_mayFollowTypeArgs` gate: the type-argument reading requires an
/// allowed follower; `f(a<b, c>-d)` stays relational.
#[test]
fn relational_follower_keeps_operators() {
    let c = parse("void f(int a, int b, int c, int d) { f(a<b, c>-d); }");
    assert!(
        !c.log.contains(&("handle", "type_argument_application")),
        "no application event for the `-` follower: {:?}",
        c.log
    );
}

// —— family 5: `c.m<D>()` statement head (official `computePrefixedType`
// optional-shape veto, type_info_impl.dart:1081-1084, plus the post-dot
// method-type-arguments block, parser_impl.dart:7345-7371) —————————————————

/// A member invocation with type arguments as a statement head stays a send.
/// Backs the semantic `member_invocation_arity` RED.
#[test]
fn member_invocation_with_type_args_is_send() {
    let c = parse("class D {} class C { void m<T, U>() {} } void f(C c) { c.m<D>(); }");
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    c.assert_sequence(&[
        ("begin", "type_arguments"),
        ("end", "type_arguments"),
        ("handle", "send"),
        ("handle", "expression_statement"),
    ]);
}

/// A genuine prefixed parameterized declaration is untouched by the veto.
#[test]
fn prefixed_parameterized_declaration_keeps_type() {
    let c = parse("void f() { p.F<int> v; }");
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    c.assert_sequence(&[
        ("begin", "variables_declaration"),
        ("begin", "initialized_identifier"),
        ("end", "initialized_identifier"),
        ("end", "variables_declaration"),
    ]);
}

// —— family 6: `Type? name = init;` follower (official `=`-arm speculation,
// parser_impl.dart:9976-9995) —————————————————————————————————————————————

/// A nullable type with an initializer keeps the declaration. Backs the
/// semantic `declaration_initializer_null` RED (`int? y = null;`).
#[test]
fn nullable_type_with_initializer_is_declaration() {
    let c = parse("void f() { int? y = null; }");
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    c.assert_sequence(&[
        ("begin", "variables_declaration"),
        ("begin", "initialized_identifier"),
        ("end", "initialized_identifier"),
        ("end", "variables_declaration"),
    ]);
}

/// The `a ? b = c : d` conditional still drops the type reading.
#[test]
fn conditional_with_assignment_initializer_drops_type() {
    let c = parse("void f(bool a, int b, int c, int d) { a ? b = c : d; }");
    assert!(
        !c.log.contains(&("begin", "variables_declaration")),
        "no declaration for the conditional: {:?}",
        c.log
    );
    c.assert_sequence(&[("handle", "expression_statement")]);
}

// —— families 2/3: await-as-type + creation/extension statement heads ——————

/// The sync `await x;` probe keeps the official declaration shape (the
/// oracle reports the misuse from the type annotation,
/// `named_type_resolver.dart:814-818`); the async `await f;` stays an
/// expression. Pinned so the semantic await rule sees exactly these shapes.
#[test]
fn await_statement_shapes_follow_official_dispatch() {
    let c = parse("void f() { await x; } void g() async { await f; }");
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    c.assert_sequence(&[
        ("begin", "variables_declaration"),
        ("end", "variables_declaration"),
    ]);
}

/// `const C<A>();` keeps its const-creation expression shape (the
/// statement-level `computeType` `(`-follower veto, fast arm
/// type_info.dart:270-292). Pinned for the semantic `creation_arity` RED.
#[test]
fn const_creation_with_type_args_is_expression() {
    let c = parse("class A {} class C {} void f() { new C(); const C<A>(); }");
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    c.assert_sequence(&[
        ("handle", "expression_statement"),
        ("begin", "type_arguments"),
        ("end", "type_arguments"),
        ("handle", "expression_statement"),
    ]);
}

/// `Ex<A>(a);` keeps its send shape for the semantic
/// `extension_override_arity` RED (the `(`-follower gate in `parseSend`,
/// parser_impl.dart:9404-9409).
#[test]
fn extension_override_with_type_args_is_expression() {
    let c = parse("class A {} extension Ex on A {} void f(A a) { Ex<A>(a); }");
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    c.assert_sequence(&[
        ("begin", "type_arguments"),
        ("end", "type_arguments"),
        ("handle", "send"),
        ("handle", "expression_statement"),
    ]);
}
