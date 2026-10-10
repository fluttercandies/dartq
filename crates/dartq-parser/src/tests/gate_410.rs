//! Ticket #410: dartq-only over-report families from the r5 M0 gate
//! (`docs/audits/gate-m0/gate-caliber-r5/report.json`), pinned against the
//! official parser's report/silence contracts.
//!
//! Families:
//! * reserved-word constant patterns (`case true:`, `case null:`,
//!   `true =>`, `case true || false:`): official `parsePrimaryPattern`
//!   (parser_impl.dart:11691-11897) has no reserved-keyword arm — boolean/
//!   null literals fall through to the implicit-constant-pattern expression
//!   parse and stay silent.
//! * object patterns with type arguments (`case Future<int>():`): the
//!   official `(` follower is checked after the type-argument group
//!   (`potentialTypeArg.skip(token).next!`, :11833-11835), not at the
//!   cursor (which still sits on `<`).
//! * constructor references (`= C<int>.new;`): official
//!   `parseConstructorReference` (:6414-6446) reads identifier, qualified
//!   rest, type arguments, THEN the `.name` suffix, with `new` after `.`
//!   rewritten to an identifier (`_tryRewriteNewToIdentifier` :4101).
//! * try/finally: official `parseBlock(finallyToken, ...)` (:1502) consumes
//!   the `finally` token before ensureBlock.
//! * `<T>() {}` generic function literal initializer: the `(` follower of
//!   the literal dispatch is read after the type-argument group
//!   (:8291-8300).

use super::collector::setup;

/// Sorted diagnostic-code multiset of one whole-unit parse.
fn sorted_codes(source: &'static str) -> Vec<String> {
    let mut p = setup(source);
    p.parse_unit();
    p.listener.error_codes()
}

/// Asserts the exact diagnostic multiset (codes only, sorted).
fn assert_codes(source: &'static str, expected: &[&str], label: &str) {
    let got = sorted_codes(source);
    let expected: Vec<String> = expected.iter().map(ToString::to_string).collect();
    assert_eq!(got, expected, "{label}: diagnostic multiset diverged");
}

/// Asserts the parse is diagnostic-free.
fn assert_clean(source: &'static str, label: &str) {
    assert_codes(source, &[], label);
}

// —— reserved-word constant patterns (official: silent) ————————————

#[test]
fn case_true_label_is_silent() {
    assert_clean(
        "void f(bool b) { switch (b) { case true: break; } }",
        "true",
    );
}

#[test]
fn case_false_null_labels_are_silent() {
    assert_clean(
        "void f(Object o) { switch (o) { case false: break; case null: break; } }",
        "false_null",
    );
}

#[test]
fn case_binary_or_pattern_of_literals_is_silent() {
    assert_clean(
        "void f(bool b) { switch (b) { case true || false: break; } }",
        "or_pattern",
    );
}

#[test]
fn switch_expr_literal_arms_are_silent() {
    assert_clean(
        "void f(bool b) { var r = switch (b) { true => 0, false => 1 }; }",
        "switch_expr_arms",
    );
}

#[test]
fn record_pattern_field_literal_is_silent() {
    assert_clean(
        "enum E { a, b } void f((E, bool) r) { switch (r) { case (E.a, true): break; } }",
        "record_field",
    );
}

// —— object patterns with type arguments (official: silent) ————————

#[test]
fn case_object_pattern_with_type_args_is_silent() {
    assert_clean(
        "void f(Object o) { switch (o) { case Future<int>(): break; case int(): break; } }",
        "case",
    );
}

#[test]
fn switch_expr_object_pattern_arms_are_silent() {
    assert_clean(
        "void f(Object o) { var r = switch (o) { int() => 0, Future<String>() => 1, _ => 2 }; }",
        "expr",
    );
}

// —— constructor references (official: silent) ————————————————————

#[test]
fn factory_redirection_new_is_silent() {
    assert_clean("class C { factory D.redir() = C.new; }", "redirect_new");
}

#[test]
fn factory_redirection_generic_new_is_silent() {
    assert_clean(
        "class C { factory D.redir() = C<int>.new; }",
        "redirect_generic",
    );
}

#[test]
fn factory_redirection_prefixed_generic_new_is_silent() {
    assert_clean(
        "class C { factory D.redir() = prefix.C<int>.new; }",
        "redirect_prefixed_generic",
    );
}

#[test]
fn super_new_initializer_is_silent() {
    assert_clean(
        "class B { B(); } class D extends B { D.i() : super.new(); }",
        "super_new",
    );
}

#[test]
fn this_new_initializer_is_silent() {
    assert_clean("class D { D.n(); D.r() : this.n(); }", "this_new");
}

#[test]
fn implicit_creation_with_type_args_is_silent() {
    assert_clean(
        "class C { C.n(); } void f() { var c = C<int>.new; }",
        "implicit_creation",
    );
}

// —— try/finally (official: silent) ————————————————————————————————

#[test]
fn try_finally_no_catch_is_silent() {
    assert_clean(
        "void f(int x) { try { a; } finally { g; } }",
        "finally_only",
    );
}

#[test]
fn try_catch_finally_is_silent() {
    assert_clean(
        "void f() { try { a; } catch (e) { b; } finally { g; } }",
        "catch_finally",
    );
}

#[test]
fn try_finally_with_if_return_is_silent() {
    assert_clean(
        "void f(int? x) { try { if (x == null) return; x; } finally { x; } }",
        "if_return_finally",
    );
}

// —— generic function literal initializer (official: silent) ————————

#[test]
fn generic_function_literal_initializer_is_silent() {
    assert_clean("void Function<T>() g = <T>() {};", "gft_toplevel");
}

#[test]
fn generic_function_literal_typed_formal_is_silent() {
    assert_clean("void h(void Function<T>(T) cb) {}", "gft_formal");
}

// —— list-pattern shapes (official: silent) ————————————————————————

#[test]
fn list_pattern_rest_is_silent() {
    assert_clean(
        "void f(List l) { var a = switch (l) { [...] => 0 }; }",
        "rest",
    );
}

#[test]
fn list_pattern_rest_var_is_silent() {
    assert_clean(
        "void f(List l) { var a = switch (l) { [...var l2] => l2.length }; }",
        "rest_var",
    );
}

#[test]
fn list_pattern_rest_typed_is_silent() {
    assert_clean(
        "void f(List l) { var a = switch (l) { [...List<String> l2] => l2.length }; }",
        "rest_typed",
    );
}

#[test]
fn list_pattern_typed_then_rest_is_silent() {
    assert_clean(
        "class B {} void f(List l) { var a = switch (l) { [B b, ..., _] => 0 }; }",
        "typed_then_rest",
    );
}

#[test]
fn list_pattern_wildcard_arms_are_silent() {
    assert_clean(
        "void f(List l) { var a = switch (l) { [] => 0 }; }",
        "empty",
    );
}

#[test]
fn list_pattern_single_wildcard_is_silent() {
    assert_clean(
        "void f(List l) { var a = switch (l) { [_] => 1 }; }",
        "single",
    );
}

#[test]
fn list_pattern_two_wildcards_are_silent() {
    assert_clean(
        "void f(List l) { var a = switch (l) { [_, _] => 2 }; }",
        "pair",
    );
}

#[test]
fn list_pattern_wildcard_rest_wildcard_is_silent() {
    assert_clean(
        "void f(List l) { var a = switch (l) { [_, ..., _] => 3 }; }",
        "wc_rest_wc",
    );
}

// —— record-type variable patterns (official: silent) ———————————————
#[test]
fn nullable_record_parameter_is_silent() {
    assert_clean(
        "enum E { a, b } void f((E, bool)? r) {}",
        "nullable_record_param",
    );
}

#[test]
fn nullable_record_switch_null_case_is_silent() {
    assert_clean(
        "enum E { a, b } void f((E, bool)? r) { switch (r) { case null: break; } }",
        "nullable_record_null_case",
    );
}

#[test]
fn record_pattern_var_field_is_silent() {
    assert_clean(
        "enum E { a, b } void f((E, bool) r) { switch (r) { case (E.a, var b): break; } }",
        "record_var_field",
    );
}

#[test]
fn record_type_variable_pattern_is_silent() {
    assert_clean(
        "void f((int, int) r) { switch (r) { case (int a, int b): break; } }",
        "record_type_var",
    );
}
