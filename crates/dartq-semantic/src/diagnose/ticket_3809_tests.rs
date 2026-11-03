//! The #3809 pins: the `could_not_infer` detail's `_` display and the
//! named-constructor row's written extent.
//!
//! Two faces of the raw-creation inference row land here, both byte-exact
//! against the pinned oracle (Dart SDK 3.13.7) `dart analyze
//! --format=machine` rows for exactly these fixtures:
//!
//! - **the detail's unsolved-slot display** — `_formatConstraints` prints
//!   an unsolved sibling parameter as `_` while the
//!   `type_argument_not_matching_bounds` twin spells the same slot out
//!   (`type_arguments_verifier.dart:342-540`). The detail builder read
//!   `CreationInferenceRow::bound` (the twin's fully substituted text) and
//!   now reads `infer_bound`; the first pin holds both faces of one row
//!   so neither can drift back onto the other's field.
//! - **the row's anchor** — a named constructor's callee row anchors at
//!   the selector (`P._()` keeps `_` alone, one column past the head)
//!   while the oracle writes at the whole `ConstructorName`, whose extent
//!   starts at the head (`P._` spans 3 at the head's column). The second
//!   pin covers that widening; the third is its control, the unnamed
//!   creation whose anchor already is the head.
//!
//! # Adjacent gaps (reported, not pinned)
//!
//! The `could_not_infer` face of a NAMED constructor reached through a
//! non-function alias (`A.foo()` / `A.bar()` under `typedef A<X extends
//! C<X> Function(C<X>)> = C<X>;`) is dropped today: the alias route keys a
//! bare alias callee only, and a qualified `A.selector` row never reaches
//! the creation-inference face. The oracle reports both at 12:3/13:3 with
//! the same text the bare `A()` row carries
//! (`CouldNotInferTest.test_instanceCreation_viaTypeAlias_notWellBounded`).
//! That is a missing-row defect on the alias route, not an extent defect —
//! the widened name walk above already spells `A.foo` when the row lands.

use super::test_rig::{CORE_PRELUDE, report_ops, rows};
use super::*;

/// The absolute fixture offset of `needle`'s LAST occurrence (the prelude
/// the rig prepends included): the fixtures declare a constructor and
/// invoke it, so the call site is the trailing spelling.
fn at_last(fixture: &str, needle: &str) -> usize {
    CORE_PRELUDE.len() + fixture.rfind(needle).expect("needle present")
}

/// The absolute fixture offset of `needle`'s first occurrence.
fn at(fixture: &str, needle: &str) -> usize {
    CORE_PRELUDE.len() + fixture.find(needle).expect("needle present")
}

/// The messages of every `name` row at `offset:len`.
fn messages_at<'r>(report: &'r FileReport, name: &str, offset: usize, len: usize) -> Vec<&'r str> {
    report
        .diagnostics
        .iter()
        .filter(|d| {
            d.code.name == name
                && d.range.offset as usize == offset
                && d.range.length as usize == len
        })
        .map(|d| d.message.as_str())
        .collect()
}

/// `class A<X1 extends X2, X2 extends A<X1, X2>> {}` with the bare creation
/// `A();` — `X1` stays unsolved, so the reported `X2`'s bound prints it as
/// `_` in the detail and fully substituted in the twin. Oracle
/// `Instantiate-to-bound/class/static/class_l2_t01` 66:3 (the two faces of
/// one row: detail `'A<_, A<Object?, Object?>>'`, twin
/// `'A<A<Object?, Object?>, A<Object?, Object?>>'`).
#[test]
fn detail_prints_an_unsolved_sibling_as_underscore() {
    let src = r#"
class A<X1 extends X2, X2 extends A<X1, X2>> {}
main() {
  A();
}
"#;
    let report = report_ops(src);
    let offset = at(src, "A();");
    assert_eq!(
        messages_at(&report, "could_not_infer", offset, 1),
        [concat!(
            "Couldn't infer type parameter 'X2'.\n\n",
            "Tried to infer 'A<Object?, Object?>' for 'X2' which doesn't work:\n  ",
            "Type parameter 'X2' is declared to extend 'A<X1, X2>' producing ",
            "'A<_, A<Object?, Object?>>'.\n\n",
            "Consider passing explicit type argument(s) to the generic.\n\n",
        )],
        "{:#?}",
        rows(&report)
    );
    // The twin keeps the fully substituted spelling — the same slot the
    // detail prints `_` for.
    assert_eq!(
        messages_at(&report, "type_argument_not_matching_bounds", offset, 1),
        [concat!(
            "'A<Object?, Object?>' doesn't conform to the bound ",
            "'A<A<Object?, Object?>, A<Object?, Object?>>' of the type parameter 'X2'.",
        )],
        "{:#?}",
        rows(&report)
    );
}

/// `class P<T extends C<T>, U extends C<U>> { P._(); }` with the named
/// creation `P._();` — both parameters report at the whole constructor
/// name. Oracle `CouldNotInferTest.test_constructors_inferenceFBounded`
/// 13:3 len 3 (`P._`), the twins at 13:3 len 1 (the `NamedType` head).
#[test]
fn named_constructor_rows_anchor_the_whole_name() {
    let src = r#"
class C<T> {}
class P<T extends C<T>, U extends C<U>> {
  P._();
}
void main() {
  P._();
}
"#;
    let report = report_ops(src);
    let head = at_last(src, "P._();");
    assert_eq!(
        messages_at(&report, "could_not_infer", head, 3),
        [
            concat!(
                "Couldn't infer type parameter 'T'.\n\n",
                "Tried to infer 'C<Object?>' for 'T' which doesn't work:\n  ",
                "Type parameter 'T' is declared to extend 'C<T>' producing 'C<C<Object?>>'.\n\n",
                "Consider passing explicit type argument(s) to the generic.\n\n",
            ),
            concat!(
                "Couldn't infer type parameter 'U'.\n\n",
                "Tried to infer 'C<Object?>' for 'U' which doesn't work:\n  ",
                "Type parameter 'U' is declared to extend 'C<U>' producing 'C<C<Object?>>'.\n\n",
                "Consider passing explicit type argument(s) to the generic.\n\n",
            ),
        ],
        "{:#?}",
        rows(&report)
    );
    assert_eq!(
        messages_at(&report, "type_argument_not_matching_bounds", head, 1),
        [
            "'C<Object?>' doesn't conform to the bound 'C<C<Object?>>' of the type parameter 'T'.",
            "'C<Object?>' doesn't conform to the bound 'C<C<Object?>>' of the type parameter 'U'.",
        ],
        "{:#?}",
        rows(&report)
    );
}

/// The control: an UNNAMED creation's callee row already anchors at the
/// head, so the extents stay 1 in both faces — the backwards step of the
/// name walk must not fire here. Oracle
/// `CouldNotInferTest.test_constructors_inferenceFBounded`'s unnamed
/// sibling shape (`B()` over `class B<T extends Box<T>>`, 13:3 len 1).
#[test]
fn unnamed_creation_keeps_the_head_anchor() {
    let src = r#"
class Box<T> {}
class B<T extends Box<T>> {}
void main() {
  B();
}
"#;
    let report = report_ops(src);
    let head = at(src, "B();");
    assert_eq!(
        messages_at(&report, "could_not_infer", head, 1),
        [concat!(
            "Couldn't infer type parameter 'T'.\n\n",
            "Tried to infer 'Box<Object?>' for 'T' which doesn't work:\n  ",
            "Type parameter 'T' is declared to extend 'Box<T>' producing 'Box<Box<Object?>>'.\n\n",
            "Consider passing explicit type argument(s) to the generic.\n\n",
        )],
        "{:#?}",
        rows(&report)
    );
    assert_eq!(
        messages_at(&report, "type_argument_not_matching_bounds", head, 1),
        [concat!(
            "'Box<Object?>' doesn't conform to the bound 'Box<Box<Object?>>' ",
            "of the type parameter 'T'.",
        )],
        "{:#?}",
        rows(&report)
    );
}

/// The alias face of the same builder: `typedef A<X1 extends B<X1, X2>,
/// X2> = B<X1, X2>;` over `class B<X extends B<X, Y>, Y>` with a bare
/// `A()` — the alias route keys the alias's own parameters, `X1` solves
/// and reports, and the unbounded `X2` prints `_` in the detail while the
/// suppressed twin's slot would read `dynamic`. Oracle
/// `Instantiate-to-bound/nonfunction_typedef/static/nonfunction_l2_t06`
/// 77:3.
#[test]
fn alias_creation_detail_prints_the_unsolved_slot_as_underscore() {
    let src = r#"
class B<X extends B<X, Y>, Y> {}
typedef A<X1 extends B<X1, X2>, X2> = B<X1, X2>;
main() {
  A();
}
"#;
    let report = report_ops(src);
    let offset = at(src, "A();");
    assert_eq!(
        messages_at(&report, "could_not_infer", offset, 1),
        [concat!(
            "Couldn't infer type parameter 'X1'.\n\n",
            "Tried to infer 'B<Object?, Object?>' for 'X1' which doesn't work:\n  ",
            "Type parameter 'X1' is declared to extend 'B<X1, X2>' producing ",
            "'B<B<Object?, Object?>, _>'.\n\n",
            "Consider passing explicit type argument(s) to the generic.\n\n",
        )],
        "{:#?}",
        rows(&report)
    );
}
