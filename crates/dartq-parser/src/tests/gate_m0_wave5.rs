//! M0-gate wave-5 regression pins (#605): constant patterns with type
//! arguments and `.new` constructor names in pattern context (the
//! `const_patterns.dart` m0-syntax fixture class). The official decision
//! order (`parser_impl.dart`, Dart 3.15 main):
//!
//! * `case const ...` enters the expression engine with
//!   `ConstantPatternContext.explicit` (`parsePrimaryPattern` :11781-11797).
//! * The name's `<...>` group is kept only when it opens an invocation —
//!   directly `(` (`parseSend` :9396-9412) or `.` + (`new`|identifier) + `(`
//!   (the `parseUnaryExpression` implicit-creation probe :7995-8013 →
//!   `parseImplicitCreationExpression` :9009, where the parser-level
//!   `ensureIdentifier` rewrite :4076-4083 lets `new` name the unnamed
//!   constructor with no report).
//! * A group that opens nothing is consumed by the precedence level itself
//!   (:7129-7147): a non-`(` follower is a generic tear-off, which a
//!   constant-pattern context reports exactly once
//!   (`invalidConstantPatternGeneric`) — it never reaches the binary loop,
//!   so no comparison chain is minted.
//! * `parseSend`'s explicit-const gate (:9413-9423): only `.`/`(`/`<` can
//!   still complete a const constructor invocation; any other follower
//!   reports `invalidConstantPatternConstPrefix` once.

use super::collector::setup;

/// Sorted diagnostic-code multiset of one whole-unit parse of `pattern`
/// inside a switch case (`void f(o) { switch (o) { case <pattern>: break; } }`).
fn case_codes(pattern: &'static str) -> Vec<String> {
    let source: &'static str = Box::leak(
        format!("void f(o) {{ switch (o) {{ case {pattern}: break; }} }}\n").into_boxed_str(),
    );
    let mut p = setup(source);
    p.parse_unit();
    let mut codes = p.listener.error_codes();
    codes.sort();
    codes
}

/// [`case_codes`] with the expectation sorted the same way.
fn assert_case_codes(pattern: &'static str, expected: &[&str], label: &str) {
    let got = case_codes(pattern);
    let mut want: Vec<String> = expected.iter().map(ToString::to_string).collect();
    want.sort();
    assert_eq!(got, want, "{label}: diagnostic multiset diverged");
}

/// Event-sequence variant of [`case_codes`]: parses the unit and hands the
/// listener to `check` after asserting the (code-multiset, event-subsequence)
/// pair.
fn assert_case_stream(pattern: &'static str, expected: &[&str], label: &str) {
    let source: &'static str = Box::leak(
        format!("void f(o) {{ switch (o) {{ case {pattern}: break; }} }}\n").into_boxed_str(),
    );
    let mut p = setup(source);
    p.parse_unit();
    let mut got = p.listener.error_codes();
    got.sort();
    let want: Vec<String> = expected.iter().map(ToString::to_string).collect();
    assert_eq!(got, want, "{label}: diagnostic multiset diverged");
    p.listener
        .assert_sequence(&[("begin", "constant_pattern"), ("end", "constant_pattern")]);
}

// —— clean const constructor invocations (fixture :74-81) —————————————————

/// `case const GenericClass<int>():` — the group opens the invocation and is
/// kept (`parseSend` :9403-9412); no comparison chain, no const-prefix noise.
#[test]
fn const_generic_invocation_is_a_constant_pattern() {
    assert_case_stream(
        "const GenericClass<int>()",
        &[],
        "const GenericClass<int>()",
    );
}

/// `case const GenericClass<int>(a: 0):` — named arguments ride along.
#[test]
fn const_generic_named_argument_invocation_is_clean() {
    assert_case_stream(
        "const GenericClass<int>(a: 0)",
        &[],
        "const GenericClass<int>(a: 0)",
    );
}

/// `case const GenericClass<int>.new():` — the implicit-creation probe
/// (:7995-8013) routes to the constructor-reference form; `.new` is the
/// unnamed constructor, not a keyword recovery (:4076-4083).
#[test]
fn const_generic_dot_new_invocation_is_clean() {
    assert_case_stream(
        "const GenericClass<int>.new()",
        &[],
        "const GenericClass<int>.new()",
    );
}

/// `case const GenericClass<int>.new(a: 1):` — named arguments after `.new`.
#[test]
fn const_generic_dot_new_named_arguments_are_clean() {
    assert_case_stream(
        "const GenericClass<int>.new(a: 1)",
        &[],
        "const GenericClass<int>.new(a: 1)",
    );
}

// —— generic tear-offs report exactly once (fixture :105-110) —————————————

/// `case const GenericClass<int>:` — the group opens nothing; the precedence
/// level consumes it and reports `invalidConstantPatternGeneric` (:7129-7147)
/// instead of minting a `<`/`>` comparison chain.
#[test]
fn const_generic_tear_off_reports_generic_once() {
    assert_case_stream(
        "const GenericClass<int>",
        &["invalid_constant_pattern_generic"],
        "const GenericClass<int>",
    );
}

/// `case const prefix.GenericClass<int>:` — same, through the selector chain.
#[test]
fn const_prefixed_generic_tear_off_reports_generic_once() {
    assert_case_codes(
        "const prefix.GenericClass<int>",
        &["invalid_constant_pattern_generic"],
        "const prefix.GenericClass<int>",
    );
}

/// `case GenericClass<int>.new:` — implicit context; the tear-off reports
/// once and the `.new` selector stays silent (:7825-7841).
#[test]
fn generic_dot_new_tear_off_reports_generic_once() {
    assert_case_stream(
        "GenericClass<int>.new",
        &["invalid_constant_pattern_generic"],
        "GenericClass<int>.new",
    );
}

/// `case GenericClass<int>:` — implicit constant pattern, same single report.
#[test]
fn implicit_generic_tear_off_reports_generic_once() {
    assert_case_codes(
        "GenericClass<int>",
        &["invalid_constant_pattern_generic"],
        "GenericClass<int>",
    );
}

// —— the explicit-const gate (parseSend :9413-9423) ———————————————————————

/// `case const value:` — a bare identifier under an explicit `const` reports
/// the const-prefix error once (`:` completes no constructor invocation).
#[test]
fn const_bare_identifier_reports_const_prefix() {
    assert_case_codes(
        "const value",
        &["invalid_constant_const_prefix"],
        "const value",
    );
}

/// `case const Foo.new():` — the `.` follower is exempt from the gate and the
/// selector rewrite names the unnamed constructor with no report.
#[test]
fn const_dot_new_without_type_arguments_is_clean() {
    assert_case_stream("const Foo.new()", &[], "const Foo.new()");
}

// —— boundary pins: neighboring faces stay on their official shapes ————————
//
// Each expectation below is decided by the cited official line:
// * `case const Foo()` — the group-free invocation keep (`parseSend`
//   :9420-9428); green.
// * `case const Foo.named<int>()` — the selector chain's method type
//   arguments followed by `(` (:7847-7860); green.
// * `case GenericClass<int>():` — the non-const object-pattern keep
//   (`parsePrimaryPattern` :11828-11856, follower `(` after the group);
//   green with an `object_pattern` event.
// * `case const const GenericClass<int>():` — the duplicate-`const` report
//   (`parsePrimary` :8168-8176) with the rest riding the const-expression
//   constructor reference (:6414); exactly one diagnostic.

#[test]
fn plain_const_invocation_stays_green() {
    assert_case_stream("const Foo()", &[], "const Foo()");
}

#[test]
fn named_constructor_type_arguments_stay_green() {
    assert_case_codes("const Foo.named<int>()", &[], "const Foo.named<int>()");
}

#[test]
fn nonconst_object_pattern_with_type_arguments_is_an_object_pattern() {
    let source: &'static str = "void f(o) { switch (o) { case GenericClass<int>(): break; } }\n";
    let mut p = setup(source);
    p.parse_unit();
    assert!(
        p.listener.error_codes().is_empty(),
        "{:?}",
        p.listener.error_codes()
    );
    p.listener.assert_sequence(&[("handle", "object_pattern")]);
}

#[test]
fn duplicate_const_generic_invocation_reports_duplicate_only() {
    assert_case_codes(
        "const const GenericClass<int>()",
        &["invalid_constant_pattern_duplicate_const"],
        "const const GenericClass<int>()",
    );
}
