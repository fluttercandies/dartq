//! The #2831 pins: the raw-constructor inference face (P1 — the
//! greatest-closure seed and extends-clause re-check of a type-argument-less
//! creation with no downward context, reporting `could_not_infer` plus the
//! creation-site `type_argument_not_matching_bounds` twin) and the nested
//! bounded-head site queueing (P2 — every in-file container argument head
//! checks as its own site, the co19 `F<A<…>>` shape whose outer head an
//! alias answered for nobody). Message texts are the pinned oracle rows of
//! `Instantiate-to-bound/class/static/class_FutureOr_l1_t02` (75:3 and
//! 59:7).

use super::test_rig::{CORE_PRELUDE, report_ops, rows};
use super::*;

/// The absolute fixture offset of `needle` (the prelude the rig prepends
/// included).
fn at(fixture: &str, needle: &str) -> usize {
    CORE_PRELUDE.len() + fixture.find(needle).unwrap()
}

/// Exactly one row of `name` at `offset:len` carrying `message`.
#[track_caller]
fn assert_row(report: &FileReport, name: &str, offset: usize, len: usize, message: &str) {
    let hits: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| {
            d.code.name == name
                && d.range.offset as usize == offset
                && d.range.length as usize == len
        })
        .collect();
    assert_eq!(
        hits.len(),
        1,
        "expected one {name} @ {offset}:{len}; got {:#?}",
        rows(report)
    );
    assert_eq!(hits[0].message, message, "{:#?}", rows(report));
}

/// No row of `name` anywhere.
#[track_caller]
fn assert_none(report: &FileReport, name: &str) {
    assert!(
        report.diagnostics.iter().all(|d| d.code.name != name),
        "expected no {name}; got {:#?}",
        rows(report)
    );
}

/// The raw creation's two rows, byte-exact against the oracle's
/// `class_FutureOr_l1_t02` 75:3 pair.
const INFER_MESSAGE: &str = "Couldn't infer type parameter 'X'.\n\nTried to infer \
    'FutureOr<A<Object?>>' for 'X' which doesn't work:\n  Type parameter 'X' is declared to \
    extend 'FutureOr<A<X>>' producing 'FutureOr<A<FutureOr<A<Object?>>>>'.\n\nConsider passing \
    explicit type argument(s) to the generic.\n\n";
const TWIN_MESSAGE: &str = "'FutureOr<A<Object?>>' doesn't conform to the bound 'FutureOr<A<FutureOr<A<Object?>>>>' \
     of the type parameter 'X'.";

/// P1 failing pin: `class A<X extends FutureOr<A<X>>> {}` with the bare
/// creation `A();` — the greatest-closure seed violates the substituted
/// extends bound and both rows report at the constructor name.
#[test]
fn raw_ctor_reports_inference_and_twin() {
    let src = r#"
class A<X extends FutureOr<A<X>>> {}
main() {
  A();
}
"#;
    let report = report_ops(src);
    let offset = at(src, "A();");
    assert_row(&report, "could_not_infer", offset, 1, INFER_MESSAGE);
    assert_row(
        &report,
        "type_argument_not_matching_bounds",
        offset,
        1,
        TWIN_MESSAGE,
    );
}

/// P1 failing pin over a DECLARED unnamed constructor: the face is the
/// creation's own, with or without a formal table.
#[test]
fn raw_ctor_with_declared_constructor_reports_the_same() {
    let src = r#"
class A<X extends FutureOr<A<X>>> { A(); }
main() {
  A();
}
"#;
    let report = report_ops(src);
    // The indented needle skips the constructor declaration's own `A();`
    // in the class body; +2 steps over the indentation to the name.
    let offset = at(src, "  A();") + 2;
    assert_row(&report, "could_not_infer", offset, 1, INFER_MESSAGE);
    assert_row(
        &report,
        "type_argument_not_matching_bounds",
        offset,
        1,
        TWIN_MESSAGE,
    );
}

/// P1 passing pin: a satisfied seed stays silent — `num`'s closure is
/// itself, and `num <: num`.
#[test]
fn satisfied_seed_stays_silent() {
    let report = report_ops(
        r#"
class B<T extends num> {}
main() {
  B();
}
"#,
    );
    assert_none(&report, "could_not_infer");
    assert_none(&report, "type_argument_not_matching_bounds");
}

/// P1 passing pin: an unbounded parameter seeds `dynamic` (the bound the
/// build maps an absent bound to), which satisfies everything.
#[test]
fn unbounded_creation_stays_silent() {
    let report = report_ops(
        r#"
class C<T> {}
main() {
  C();
}
"#,
    );
    assert_none(&report, "could_not_infer");
}

/// P1 context pin: a creation inside an argument list has a downward
/// context (the outer call's formal) — the seed face never runs.
#[test]
fn argument_position_suppresses_the_seed_face() {
    let report = report_ops(
        r#"
class A<X extends FutureOr<A<X>>> {}
void f(p) {}
main() {
  f(A());
}
"#,
    );
    assert_none(&report, "could_not_infer");
}

/// P1 context pin: a creation initializing an annotated declaration has
/// the written type as its downward context.
#[test]
fn annotated_declaration_suppresses_the_seed_face() {
    let report = report_ops(
        r#"
class A<X extends FutureOr<A<X>>> {}
main() {
  A? x = A();
}
"#,
    );
    assert_none(&report, "could_not_infer");
}

/// P1 multi-parameter pin: only the violating parameter reports, in its
/// own spellings (the arity-correct bound keeps the snapshot's map live —
/// a nested `C<X>` under a two-parameter `C` is its own
/// `wrong_number_of_type_arguments` face and maps nothing here).
#[test]
fn multi_parameter_container_reports_the_violating_slot_only() {
    let src = r#"
class D<T> {}
class C<X extends D<X>, Y extends num> {}
main() {
  C();
}
"#;
    let report = report_ops(src);
    let offset = at(src, "C();");
    assert_row(
        &report,
        "could_not_infer",
        offset,
        1,
        "Couldn't infer type parameter 'X'.\n\nTried to infer 'D<Object?>' for 'X' which \
         doesn't work:\n  Type parameter 'X' is declared to extend 'D<X>' producing \
         'D<D<Object?>>'.\n\nConsider passing explicit type argument(s) to the generic.\n\n",
    );
    assert_row(
        &report,
        "type_argument_not_matching_bounds",
        offset,
        1,
        "'D<Object?>' doesn't conform to the bound 'D<D<Object?>>' of the type parameter 'X'.",
    );
    assert_eq!(
        report
            .diagnostics
            .iter()
            .filter(|d| d.code.name == "could_not_infer")
            .count(),
        1
    );
}

/// P2 failing pin: the co19 59:7 shape — the outer head an alias cannot
/// answer for, the nested `A<…>` head reporting the super-bounded pass's
/// inverted spellings at its argument.
#[test]
fn nested_bounded_head_reports_the_inverted_pass() {
    let src = r#"
class A<X extends FutureOr<A<X>>> {}
class G<T> {}
main() {
  G<A<FutureOr<A<dynamic>?>>>? g;
}
"#;
    let report = report_ops(src);
    let needle = "FutureOr<A<dynamic>?>";
    let offset = at(src, needle);
    assert_row(
        &report,
        "type_argument_not_matching_bounds",
        offset,
        needle.len(),
        "'FutureOr<A<Never>?>' doesn't conform to the bound 'FutureOr<A<FutureOr<A<Never>?>>>' \
         of the type parameter 'X'.",
    );
}

/// P2 suppression pin (byte-stability): the correct instantiation's
/// nested head passes the super-bounded pass — the co19 target line the
/// oracle leaves silent.
#[test]
fn nested_head_correct_instantiation_stays_silent() {
    let report = report_ops(
        r#"
class A<X extends FutureOr<A<X>>> {}
class G<T> {}
main() {
  G<A<FutureOr<A<dynamic>>>>? g;
  G<A<dynamic>>? h;
  G<A?>? i;
}
"#,
    );
    assert_none(&report, "type_argument_not_matching_bounds");
}

/// P2 outermost pin: the previously-live outermost site reports exactly
/// one row — the nested queueing adds no duplicate for the inner head its
/// argument contains.
#[test]
fn outermost_site_reports_once_with_nested_queueing_live() {
    let src = r#"
class A<X extends FutureOr<A<X>>> {}
main() {
  A<FutureOr<A<dynamic>?>>? x;
}
"#;
    let report = report_ops(src);
    let needle = "FutureOr<A<dynamic>?>";
    let offset = at(src, needle);
    assert_row(
        &report,
        "type_argument_not_matching_bounds",
        offset,
        needle.len(),
        "'FutureOr<A<Never>?>' doesn't conform to the bound 'FutureOr<A<FutureOr<A<Never>?>>>' \
         of the type parameter 'X'.",
    );
    assert_eq!(
        report
            .diagnostics
            .iter()
            .filter(|d| d.code.name == "type_argument_not_matching_bounds")
            .count(),
        1,
        "{:#?}",
        rows(&report)
    );
}
