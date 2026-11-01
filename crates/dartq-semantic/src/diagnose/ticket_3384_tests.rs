//! The #3384 display-order residual pins: a written function type's own
//! type-parameter group must display in DECLARATION order with each
//! bound's explicit type arguments spelled — the official `FunctionType`
//! display form (pinned Dart SDK source,
//! `pkg/analyzer/lib/src/dart/element/display_string_builder.dart`
//! `writeFunctionType` → `_writeTypeParameters`).
//!
//! Oracle provenance: pinned `dart analyze` 3.13.7 on the minimized
//! fixtures below, and the conformance board rows they reconstruct
//! (`Instantiate-to-bound/typedef/static/typedef_l2_02_t06`,
//! `typedef_l1_t10`: dartq rendered `void Function<Y extends A, X
//! extends A>()` where the oracle renders
//! `void Function<X extends A<dynamic>, Y extends A<Never>>()`).
//!
//! Two defects, both UPSTREAM of the render:
//! 1. **Order** — the parser completes a `<...>` group in reverse
//!    declaration order (`parse_variables`' completion pass) and the
//!    fn-type capture stores that completion order verbatim (the
//!    `end_type_variable` push); the render then shows the group
//!    reversed.
//! 2. **Bound args** — a bound's parameterized head re-fires while the
//!    bounds window (not the site capture) owns the argument group, so
//!    the completed-group side channel never arms and the stored bound
//!    reads bare (`A<dynamic>` displays `A`).
//!
//! The model-level render is already faithful — the first two pins lock
//! that half (they must stay green through the upstream fix). The
//! end-to-end pins carry the oracle-true strings; they are `#[ignore]`d
//! until the capture fix (#3410) lands and must flip green with no edit.

use super::FileReport;
use super::test_rig::report_ops;
use crate::model::{ArgPool, Ty};
use dartq_source::{StringId, StringTable};

fn rows(report: &FileReport, code: &str) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == code)
        .map(|d| d.message.to_string())
        .collect()
}

// —— the model-level render (already faithful) ——————————————————

/// The render spells the group in the STORED order verbatim: two
/// bounded parameters keep their declaration order — the oracle's
/// display contract the reversed capture violates upstream.
#[test]
fn render_keeps_the_stored_declaration_order() {
    let strings = StringTable::new();
    let mut pool = ArgPool::default();
    let x_int = pool.named(strings.intern("int"), false, &[]);
    let y_string = pool.named(strings.intern("String"), false, &[]);
    let group: Box<[(StringId, Option<Ty>)]> = Box::new([
        (strings.intern("X"), Some(x_int)),
        (strings.intern("Y"), Some(y_string)),
    ]);
    let ty = pool.function_generic(&group, Ty::Void, &[], &[], false);
    assert_eq!(
        ty.render(&strings, &pool),
        "void Function<X extends int, Y extends String>()"
    );
}

/// The render spells a bound's explicit type arguments — the oracle
/// displays the written `A<dynamic>` bound argumented, never bare.
#[test]
fn render_spells_a_bound_type_arguments() {
    let strings = StringTable::new();
    let mut pool = ArgPool::default();
    let a = strings.intern("A");
    let a_dynamic = pool.named(a, false, std::slice::from_ref(&Ty::Dynamic));
    let group: Box<[(StringId, Option<Ty>)]> = Box::new([(strings.intern("Z"), Some(a_dynamic))]);
    let ty = pool.function_generic(&group, Ty::Void, &[], &[], false);
    assert_eq!(
        ty.render(&strings, &pool),
        "void Function<Z extends A<dynamic>>()"
    );
}

// —— the end-to-end capture (oracle-true, blocked on #3410) ——————

/// The written group displays in DECLARATION order. dartq currently
/// displays the completion order (`<Y extends String, X extends int>`).
#[test]
#[ignore = "blocked on listener.rs order+bound-args fix, ticket #3410"]
fn written_group_displays_in_declaration_order() {
    let report = report_ops(
        "void t() {\n\
         \x20 int i = 0;\n\
         \x20 void Function<X extends int, Y extends String>() p = i;\n\
         }",
    );
    let messages = rows(&report, "invalid_assignment");
    assert!(
        messages.iter().any(|m| m
            == "A value of type 'int' can't be assigned to a variable of type 'void Function<X extends int, Y extends String>()'."),
        "invalid_assignment messages: {messages:?}"
    );
}

/// A written bound keeps its explicit type arguments. dartq currently
/// displays the bare head (`<Z extends A>`).
#[test]
#[ignore = "blocked on listener.rs order+bound-args fix, ticket #3410"]
fn written_bound_keeps_its_type_arguments() {
    let report = report_ops(
        "class A<X> {}\n\
         void t() {\n\
         \x20 int i = 0;\n\
         \x20 void Function<Z extends A<dynamic>>() q = i;\n\
         }",
    );
    let messages = rows(&report, "invalid_assignment");
    assert!(
        messages.iter().any(|m| m
            == "A value of type 'int' can't be assigned to a variable of type 'void Function<Z extends A<dynamic>>()'."),
        "invalid_assignment messages: {messages:?}"
    );
}
