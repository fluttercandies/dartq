//! The #3111 pins: the context-free closure literal's `=>` body feeds its
//! resolved return into the captured signature, closing the
//! downward-inference fixpoint — the formal binds from the callee's
//! corresponding parameter (the receiver-seeded schema entry,
//! `_fe_analyzer_shared`'s downward constraint on the closure's formals)
//! BEFORE the body resolves, and the body's inferred return rides the
//! signature back into the enclosing send's upward constraint
//! (`generic_inferrer`'s `constrainArgument` on the literal's inferred
//! `staticType`). `xs.map((t) => t.element)` therefore solves `map`'s own
//! `R` — `Iterable<int>`, never the `Iterable<void>` the stub forced.

use crate::diagnose::test_rig::{assert_silent, code_names, report_ops, rows};

/// The headline shape: a member send whose OWN inference parameter only
// the closure body's return can constrain. `map<R>(R Function(E) f)` —
// the formal `t` binds `Element` downward off the receiver's written
// instantiation, the body `t.element` resolves `int`, and the send's
// upward gather unifies `R := int` off the captured signature's baked
// return. Silent.
#[test]
fn the_arrow_body_return_solves_the_callee_parameter() {
    let report = report_ops(
        "class Element { int get element => 0; }\n\
         class Lst<E> { Iterable<R> map<R>(R Function(E) f) => throw 0; }\n\
         void f(Lst<Element> xs) {\n\
         \x20 Iterable<int> ys = xs.map((t) => t.element);\n\
         \x20 ys;\n\
         }",
    );
    assert_silent(&report);
}

/// The wrong-type arm proves the solve really ran: a `String` body
// produces `Iterable<String>`, which `Iterable<int>` rejects — the
// `invalid_assignment` names the instantiated face, never the
// `Iterable<void>` under-infer.
#[test]
fn the_solved_return_reports_a_real_mismatch() {
    let report = report_ops(
        "class Element { String get name => ''; }\n\
         class Lst<E> { Iterable<R> map<R>(R Function(E) f) => throw 0; }\n\
         void f(Lst<Element> xs) {\n\
         \x20 Iterable<int> ys = xs.map((t) => t.name);\n\
         \x20 ys;\n\
         }",
    );
    let names = code_names(&report);
    assert_eq!(names, vec!["invalid_assignment"], "{:?}", rows(&report));
}

/// A bare formal read — `(t) => t` — feeds the formal's own bound face
// back: `R := Element` answers `Iterable<Element>` silently.
#[test]
fn a_bare_formal_body_feeds_its_own_bound() {
    let report = report_ops(
        "class Element {}\n\
         class Lst<E> { Iterable<R> map<R>(R Function(E) f) => throw 0; }\n\
         void f(Lst<Element> xs) {\n\
         \x20 Iterable<Element> ys = xs.map((t) => t);\n\
         \x20 ys;\n\
         }",
    );
    assert_silent(&report);
}

/// The block body keeps the `void`-stub posture (the `S = R` re-type its
// consumers run): a block-bodied literal under the same generic send
// still under-infers rather than over-claiming a return the `return`
// tally never recorded — the under-report arm stays byte-identical.
#[test]
fn a_block_body_keeps_the_stub_posture() {
    let report = report_ops(
        "class Element { int get element => 0; }\n\
         class Lst<E> { Iterable<R> map<R>(R Function(E) f) => throw 0; }\n\
         void f(Lst<Element> xs) {\n\
         \x20 xs.map((t) {\n\
         \x20   return t.element;\n\
         \x20 });\n\
         }",
    );
    assert_silent(&report);
}

/// A non-generic callee is untouched: the written `bool Function(E)`
// formal pairs the literal's formals exactly as before — a genuine
// return-type mismatch is the closure-return row's, never the
// argument's.
#[test]
fn a_typed_formal_body_still_reports_its_own_row() {
    let report = report_ops(
        "class Element {}\n\
         class Lst<E> { Iterable<E> where(bool Function(E) t) => throw 0; }\n\
         void f(Lst<Element> xs) {\n\
         \x20 xs.where((t) => t);\n\
         }",
    );
    assert!(
        code_names(&report)
            .iter()
            .any(|name| *name == "return_of_invalid_type_from_closure"),
        "{:?}",
        rows(&report)
    );
}

/// The primitive-body path is unchanged in kind: `=> 0` baked `int` before
// (the lexeme fallback) and bakes `int` now — `Iterable<int>` silent.
#[test]
fn a_primitive_body_keeps_its_answer() {
    let report = report_ops(
        "class Element {}\n\
         class Lst<E> { Iterable<R> map<R>(R Function(E) f) => throw 0; }\n\
         void f(Lst<Element> xs) {\n\
         \x20 Iterable<int> ys = xs.map((t) => 0);\n\
         \x20 ys;\n\
         }",
    );
    assert_silent(&report);
}
