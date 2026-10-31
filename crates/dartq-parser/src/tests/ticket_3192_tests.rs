//! #3192 pins: a type-argument group after the constructor NAME of a `new`
//! /`const` invocation reports `constructor_with_type_arguments` anchored on
//! the constructor name, then the arguments parse normally.
//!
//! Upstream seam (parser_impl.dart:8853-8866): `parseConstructorReference`
//! returns the constructor name it consumed from the period slot, and
//! `parseConstructorInvocationArguments` — seeing that `token.next` is `<...>`
//! rather than `(` — reports `constructorWithTypeArguments` on the name
//! (`findNonZeroLengthToken` keeps the real-token anchor), consumes the group
//! via `typeArg.parseArguments` + `handleInvalidTypeArguments`, and then
//! parses the `()`. dartq performs the same recovery inside
//! `parse_constructor_reference`'s period arm: a group on the CLASS head
//! (`C<int>(...)`, or the single-dot `C.named<int>()` whose group the
//! pre-period arm consumes as class arguments) is legal and stays clean, and
//! the redirecting-factory body takes no invocation arguments
//! (`parseRedirectingFactoryBody` goes to `ensureSemicolon`).
//!
//! Spans and cleanness are corpus-exact (`conformance/expected.tsv`): the
//! oracle rows are `a.b.c<C>` 2:17|1, `p.Foo.bar<int>` 5:13|3 and
//! `const A<B>.c<C>` 3:14|1 — the anchor is always the constructor NAME, not
//! the `<...>` group. Note `new C<T>()` / `const C<T>()` / `new
//! Class.named<int>()` carry NO parser row (the group rides as class type
//! arguments; the analyzer's `wrong_number_of_type_arguments_constructor` is
//! a semantic face, not this one).

use super::collector::{latest_features, scan_ok};

/// Parser rows of `source` as (code, start, end) UTF-16 triples, sorted.
fn sorted_rows(source: &'static str) -> Vec<(String, u32, u32)> {
    let (table, text) = scan_ok(source);
    let mut collector = super::collector::Collector::default();
    let mut parser = crate::Parser::new(text, table, latest_features(), &mut collector);
    parser.parse_unit();
    let mut rows = parser.listener.errors.clone();
    rows.sort();
    rows
}

fn cwta(start: u32, end: u32) -> (String, u32, u32) {
    ("constructor_with_type_arguments".to_string(), start, end)
}

/// Prefix + class + constructor + group: the corpus row is `2:17|1` — the
/// anchor is the one-character constructor name `c`, and no `expected_token`
/// row accompanies it (the `()` parses as the arguments).
#[test]
fn ticket_3192_new_prefixed_ctor_type_args_report_on_name() {
    assert_eq!(
        sorted_rows("var v = new a.b.c<C>();\n"),
        vec![cwta(16, 17)],
        "oracle 2:17|1: `new a.b.c<C>()` reports on `c` only, then parses `()`"
    );
}

/// The ticket's headline shape: `bar` = 21..24, exactly the `5:13|3` corpus
/// row; recovery continues into the empty argument list.
#[test]
fn ticket_3192_new_prefixed_named_ctor_type_args_report_on_name() {
    assert_eq!(
        sorted_rows("void f() { new p.Foo.bar<int>(); }\n"),
        vec![cwta(21, 24)],
        "oracle 5:13|3: `new p.Foo.bar<int>()` reports on `bar` only"
    );
}

/// The `const` creation path (`parseConstExpression` →
/// `parseConstructorReference(Const)` → the same invocation-arguments
/// recovery): corpus row `3:14|1` anchors on `c`.
#[test]
fn ticket_3192_const_ctor_type_args_report_on_name() {
    assert_eq!(
        sorted_rows("void f() { const A<B>.c<C>(); }\n"),
        vec![cwta(22, 23)],
        "oracle 3:14|1: `const A<B>.c<C>()` reports on `c` only"
    );
}

/// The constant-pattern `new` arm shares the seam (`parsePrimary`'s `new`
/// routes through `parseNewExpression` regardless of pattern context), so the
/// group after the period-slot name `m` reports on `m`, and the `()` parses
/// as the pattern's arguments (no other row).
#[test]
fn ticket_3192_constant_pattern_new_reports_on_ctor_name() {
    assert_eq!(
        sorted_rows("void f(int x) { switch (x) { case new p.C.m<int>(): break; } }\n"),
        vec![cwta(42, 43)],
        "oracle: `case new p.C.m<int>():` reports on `m` (42..43) only"
    );
}

/// Class type arguments on a `new` invocation are legal: no parser row
/// (corpus `new Class<int>().value` carries no annotation).
#[test]
fn ticket_3192_class_type_args_on_new_stay_clean() {
    assert_eq!(
        sorted_rows("void f() { new C<int>(); }\n"),
        Vec::<(String, u32, u32)>::new(),
        "oracle: `new C<int>()` is legal — no parser row"
    );
}

/// The single-dot shape `C.named<int>()`: `.named` rides the qualified rest
/// and the group is consumed as CLASS type arguments by the pre-period arm —
/// exactly where the official `parseConstructorReference` consumes it — so no
/// `constructor_with_type_arguments` (the analyzer's
/// `wrong_number_of_type_arguments_constructor` is a semantic face).
#[test]
fn ticket_3192_single_dot_group_is_class_args() {
    assert_eq!(
        sorted_rows("void f() { new C.named<int>(); }\n"),
        Vec::<(String, u32, u32)>::new(),
        "oracle: `new C.named<int>()` carries no parser row"
    );
}

/// `C<int>.new()` — class type arguments with the explicit unnamed
/// constructor: clean (the implicit-creation path shares the seam, and here
/// no group follows the name).
#[test]
fn ticket_3192_class_args_before_dot_new_stay_clean() {
    assert_eq!(
        sorted_rows("void f() { C<int>.new(); }\n"),
        Vec::<(String, u32, u32)>::new(),
        "oracle: `C<int>.new()` is legal — no parser row"
    );
}
