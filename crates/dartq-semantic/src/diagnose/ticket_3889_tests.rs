//! #3889 pins — three `scopes_bodies.rs` faces.
//!
//! (1) The argument-slot context for an untyped `[...]` literal inside a
//! creation (`new F0([...])`, `F0<int>([...])`, `f([...])`): the callee
//! transport drops the row for a keyword or written-group head, so
//! `arg_slot_ty_ctx` reads the level's own `unnamed_head`/`named_head`
//! latches and the selected constructor's formal table — `F0(List<int>
//! a)` types the literal's context `List<int>` and the `String` element
//! reports `list_element_type_not_assignable 'int'`
//! (`literal_element_verifier.dart:51-79`). A bare-creation callee
//! (`F0([...])`) resolves through `callee_formals`'s new
//! `Receiver::Unknown` constructor fallback.
//!
//! (2) A modern alias's written args instantiate the ALIAS's own
//! parameters, not the right-hand side's function group: `typedef Foo<S>
//! = S Function<T>(T x); Foo<int> f = <T>(T x) {};` binds `S := int`
//! while the RHS's `<T>` group survives verbatim — `body_might` prints
//! the solved `'int'`, never the raw `'S'`.
//!
//! Oracle provenance: `~/.dartq-oracle/3.13.7/dart-sdk/bin/dart analyze
//! --format=machine` — `LIST_ELEMENT_TYPE_NOT_ASSIGNABLE 'String' can't
//! be assigned to 'int'` on the `new F0(["hello"])` /
//! `new F0<int>(["hello"])` / `f(["hello"])` fixtures; `BODY_MIGHT …
//! 'int'` on the `Foo<int>` fixture, silent on the raw-`T` control.

use super::test_rig::{report_ops, rows};
use crate::diagnose::FileReport;

fn list_element_rows(report: &FileReport) -> Vec<&str> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "list_element_type_not_assignable")
        .map(|d| d.message.as_str())
        .collect()
}

fn body_mights(report: &FileReport) -> Vec<&str> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name.starts_with("body_might"))
        .map(|d| d.message.as_str())
        .collect()
}

/// (1a) Keyword unnamed creation: `new F0(["hello"])` reads the selected
/// constructor's `List<int>` formal and reports the `String` element;
/// `[3]` stays silent (the keep-side control).
#[test]
fn keyword_creation_arg_list_context() {
    let report = report_ops(
        "class F0 {\n\
         F0(List<int> a) {}\n\
         }\n\
         void main() {\n\
         new F0([]);\n\
         new F0([3]);\n\
         new F0([\"hello\"]);\n\
         new F0([\"hello\", 3]);\n\
         }\n",
    );
    let hits = list_element_rows(&report);
    assert_eq!(hits.len(), 2, "{:#?}", rows(&report));
    assert!(
        hits.iter()
            .all(|m| *m == "The element type 'String' can't be assigned to the list type 'int'."),
        "{:#?}",
        rows(&report)
    );
}

/// (1b) Keyword creation, named-argument slot: `new F1(a: ["hello"])`
/// pairs the literal against the named formal `List<int> a`.
#[test]
fn keyword_creation_named_arg_list_context() {
    let report = report_ops(
        "class F1 {\n\
         F1({List<int>? a}) {}\n\
         }\n\
         void main() {\n\
         new F1(a: [3]);\n\
         new F1(a: [\"hello\"]);\n\
         }\n",
    );
    let hits = list_element_rows(&report);
    assert_eq!(hits.len(), 1, "{:#?}", rows(&report));
    assert_eq!(
        hits[0],
        "The element type 'String' can't be assigned to the list type 'int'.",
        "{:#?}",
        rows(&report)
    );
}

/// (1c) Written-group creation: `new F0<int>(["hello"])` instantiates
/// `T := int` into the constructor's `List<T>` formal — the element
/// context is `List<int>`, never the raw `List<T>` (which would report
/// `int`-vs-`T` rows of its own or none at all).
#[test]
fn written_group_creation_arg_list_context() {
    let report = report_ops(
        "class F0<T> {\n\
         F0(List<T> a) {}\n\
         }\n\
         void main() {\n\
         new F0<int>([]);\n\
         new F0<int>([3]);\n\
         new F0<int>([\"hello\"]);\n\
         }\n",
    );
    let hits = list_element_rows(&report);
    assert_eq!(hits.len(), 1, "{:#?}", rows(&report));
    assert_eq!(
        hits[0],
        "The element type 'String' can't be assigned to the list type 'int'.",
        "{:#?}",
        rows(&report)
    );
}

/// (1d) Raw-head generic creation instantiates to the declared bounds
/// (`instantiateToBounds`): `new F3([["hello"], [3]])` over
/// `F3<T>(Iterable<Iterable<T>> a)` with an unbounded `T` reads
/// `Iterable<Iterable<dynamic>>` — both elements assign to `dynamic`
/// and the case stays silent (the oracle reports no LEM rows on the
/// unwritten-arg lines of the census fixture).
#[test]
fn raw_head_creation_arg_list_context_to_bounds() {
    let report = report_ops(
        "class F3<T> {\n\
         F3(Iterable<Iterable<T>> a) {}\n\
         }\n\
         void main() {\n\
         var f33 = new F3([[\"hello\"], [3]]);\n\
         f33.toString();\n\
         }\n",
    );
    assert!(
        list_element_rows(&report).is_empty(),
        "{:#?}",
        rows(&report)
    );
}

/// (1e) Bare unnamed creation callee: `F0(["hello"])` — no keyword, no
/// written group — resolves through `callee_formals`'s
/// `Receiver::Unknown` constructor fallback (the class's declaration
/// answers no `formals`, so `ctor_schema_formals` keyed on the class's
/// own name supplies the slot).
#[test]
fn bare_creation_arg_list_context() {
    let report = report_ops(
        "class F0 {\n\
         F0(List<int> a) {}\n\
         }\n\
         void main() {\n\
         F0([3]);\n\
         F0([\"hello\"]);\n\
         }\n",
    );
    let hits = list_element_rows(&report);
    assert_eq!(hits.len(), 1, "{:#?}", rows(&report));
    assert_eq!(
        hits[0],
        "The element type 'String' can't be assigned to the list type 'int'.",
        "{:#?}",
        rows(&report)
    );
}

/// (1f) Plain function callee control: `f(["hello"])` over
/// `void f(List<int> a)` — the top-level-declaration formals lane
/// (pre-existing) keeps answering; this pin guards against the new
/// constructor fallback shadowing it.
#[test]
fn function_arg_list_context_control() {
    let report = report_ops(
        "void f(List<int> a) {}\n\
         void main() {\n\
         f([3]);\n\
         f([\"hello\"]);\n\
         }\n",
    );
    let hits = list_element_rows(&report);
    assert_eq!(hits.len(), 1, "{:#?}", rows(&report));
    assert_eq!(
        hits[0],
        "The element type 'String' can't be assigned to the list type 'int'.",
        "{:#?}",
        rows(&report)
    );
}

/// (2a) Written alias args instantiate the alias's own parameters:
/// `Foo<int> f = <T>(T x) {}` opens the literal under
/// `int Function<T>(T)` — the body falls off with context return `int`,
/// so `body_might` prints `'int'` (the oracle's row), never the raw
/// `'S'` the pre-fix written path left standing.
#[test]
fn written_alias_args_instantiate_alias_params() {
    let report = report_ops(
        "typedef Foo<S> = S Function<T>(T x);\n\
         void main() {\n\
         Foo<int> f = <T>(T x) {};\n\
         f.toString();\n\
         }\n",
    );
    let hits = body_mights(&report);
    assert!(
        hits.iter().any(|m| m.contains("'int'")),
        "expected 'int' body_might, got {:#?}",
        rows(&report)
    );
    assert!(
        !hits.iter().any(|m| m.contains("'S'")),
        "raw 'S' leaked into body_might: {:#?}",
        rows(&report)
    );
}

/// (2b) Keep-side control: `Foo<String>` binds `S := String` — a
/// `String` return satisfies the context and no `body_might`/`invalid_
/// assignment` fires (the substituted face is real, not a blanket
/// `int`).
#[test]
fn written_alias_args_other_arg_stays_silent() {
    let report = report_ops(
        "typedef Foo<S> = S Function<T>(T x);\n\
         void main() {\n\
         Foo<String> f = <T>(T x) { return \"ok\"; };\n\
         f.toString();\n\
         }\n",
    );
    assert!(body_mights(&report).is_empty(), "{:#?}", rows(&report));
    assert!(
        report
            .diagnostics
            .iter()
            .all(|d| d.code.name != "invalid_assignment"),
        "{:#?}",
        rows(&report)
    );
}

/// (3) The `?`-spelled bare alias keeps the same instantiate-to-bounds
/// context — `Foo? f = <T>(T x) {}` opens the literal under `(dynamic
/// Function<T>(T))?`: the declared nullability joins the substituted
/// signature instead of falling through to the raw-`S` fold (no
/// `body_might` — the context return is `dynamic`; no `invalid_
/// assignment` against the declared `Foo?` face).
#[test]
fn nullable_bare_alias_keeps_instantiated_context() {
    let report = report_ops(
        "typedef Foo<S> = S Function<T>(T x);\n\
         void main() {\n\
         Foo? f = <T>(T x) {};\n\
         f.toString();\n\
         }\n",
    );
    assert!(body_mights(&report).is_empty(), "{:#?}", rows(&report));
    assert!(
        report
            .diagnostics
            .iter()
            .all(|d| d.code.name != "invalid_assignment"),
        "{:#?}",
        rows(&report)
    );
}
