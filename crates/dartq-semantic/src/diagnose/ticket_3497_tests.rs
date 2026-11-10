//! #3497 pins — a bare function type as the whole written call-site type
//! argument is captured as one member and bounds-checked like any other.
//!
//! # The defect
//!
//! The call-site type-argument capture seats a written function type as
//! one member through `fn_capture_end`'s hand-over, and that hand-over
//! keyed on the return type's own head offset. Two spellings carry no
//! such anchor and dropped out of the group entirely:
//!
//! - a return-less function type — `A<Function()>()` never reached the
//!   bounds face (no member seated), while the oracle reports
//!   `'dynamic Function()' doesn't conform …`;
//! - a function-typed return — the inner close seats the return through
//!   `fn_capture_type` with no head offset, so `A<int Function()
//!   Function()>()` handed the group a span anchored at file offset 0
//!   (a row nearly as long as the file) instead of the written chain.
//!
//! The hand-over now anchors return-less and function-typed returns at
//! the first `Function` keyword of the backward written chain
//! (`Function [<...>] (...)` units), and the trailing `?` — part of the
//! type the oracle's `_typeArgumentErrorNode` anchors — joins the extent
//! (`A<void Function()?>()` anchors 16 units, not 15).
//!
//! # Contract
//!
//! Every expectation below is the pinned oracle's
//! `dart analyze --format machine` answer on the same body
//! (`class A<T extends num>` the bound under test, `dart:core` from the
//! SDK 3.13.7): a bare function-type argument violating the bound fires
//! `type_argument_not_matching_bounds` with the written argument's own
//! extent, later arguments keep their own parameter slots, and shapes the
//! oracle accepts stay silent.

use super::test_rig::{CORE_PRELUDE, report_ops};

/// The bounds rows of one fixture as `(utf16 offset, message)` pairs, in
/// source order — the row anchor is the written argument's own extent
/// (`_typeArgumentErrorNode`), so the offset pins the capture's span.
fn bounds_rows(source: &str) -> Vec<(u32, String)> {
    let report = report_ops(source);
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "type_argument_not_matching_bounds")
        .map(|d| (d.range.offset, d.message.to_string()))
        .collect()
}

/// Oracle-verified preamble: `A<T extends num>` is the one-parameter bound
/// under test, `D<T, U>` the two-parameter shape a later argument's slot
/// shows against, `C<T>`/`G<T extends num>` the named wrappers the
/// display-fill pins nest through, `B` the plain non-conforming class.
const PREAMBLE: &str = "class A<T extends num> { A(); } \
     class D<T extends num, U extends num> { D(); } \
     class C<T> { C(); } \
     class G<T extends num> { G(); } \
     class B { B(); } \
     ";

/// The fixture offset of `needle`, in report coordinates (the rig's core
/// prelude rides ahead of the fixture on one line).
fn at(source: &str, needle: &str) -> u32 {
    (CORE_PRELUDE.len() + source.find(needle).expect("needle in fixture")) as u32
}

/// A spelled-return bare function type violates the bound with its own
/// written extent. `dart analyze` on `A<void Function()>();` reports
/// `'void Function()' doesn't conform to the bound 'num' …` at the 15-unit
/// argument.
#[test]
fn spelled_return_function_argument_is_bounds_checked() {
    let source = format!("{PREAMBLE}void main() {{ A<void Function()>(); }}");
    let start = at(&source, "void Function()");
    assert_eq!(
        bounds_rows(&source),
        [(
            start,
            "'void Function()' doesn't conform to the bound 'num' of the type parameter 'T'."
                .to_string()
        )],
    );
}

/// A nullable-return spelling keeps its own text — the display is neither
/// flattened to `Function` nor re-spelled. `dart analyze` on
/// `A<Object? Function()>();` reports `'Object? Function()' …`.
#[test]
fn nullable_return_spelling_stays_verbatim() {
    let source = format!("{PREAMBLE}void main() {{ A<Object? Function()>(); }}");
    let start = at(&source, "Object? Function()");
    assert_eq!(
        bounds_rows(&source),
        [(
            start,
            "'Object? Function()' doesn't conform to the bound 'num' of the type parameter 'T'."
                .to_string()
        )],
    );
}

/// The same face on a generic function call site (`f<T extends num>`):
/// `dart analyze` on `f<String Function()>();` reports
/// `'String Function()' …` against the callee's own bound.
#[test]
fn generic_function_call_site_checks_the_argument() {
    let source = format!(
        "{PREAMBLE}T f<T extends num>() => throw ''; void main() {{ f<String Function()>(); }}"
    );
    let start = at(&source, "String Function()");
    assert_eq!(
        bounds_rows(&source),
        [(
            start,
            "'String Function()' doesn't conform to the bound 'num' of the type parameter 'T'."
                .to_string()
        )],
    );
}

/// The display fill instantiates through the bare function type's formal
/// column: `dart analyze` on `A<int Function(G)>();` reports
/// `'int Function(G<num>)' …` (the raw head one level down fills).
#[test]
fn display_fills_through_the_bare_function_formals() {
    let source = format!("{PREAMBLE}void main() {{ A<int Function(G)>(); }}");
    let start = at(&source, "int Function(G)");
    assert_eq!(
        bounds_rows(&source),
        [(
            start,
            "'int Function(G<num>)' doesn't conform to the bound 'num' of the type parameter 'T'."
                .to_string()
        )],
    );
}

/// A function type whose return spells a generic instantiation reports
/// with its whole written shape. `dart analyze` on
/// `A<List<C<int> Function(int)>>();` reports
/// `'List<C<int> Function(int)>' …`.
#[test]
fn generic_instantiation_return_reports_whole() {
    let source = format!("{PREAMBLE}void main() {{ A<List<C<int> Function(int)>>(); }}");
    let start = at(&source, "List<C<int> Function(int)>");
    assert_eq!(
        bounds_rows(&source),
        [(
            start,
            "'List<C<int> Function(int)>' doesn't conform to the bound 'num' of the type parameter 'T'."
                .to_string()
        )],
    );
}

/// The ticket's own positive control — the nested shape that already
/// reported — stays intact: `dart analyze` on `A<A<Object?
/// Function(int)>>();` reports the outer argument and the inner
/// element. (The inner row's oracle length is 21 — `Object?
/// Function(int)`; the named-argument capture's 16-unit face is recorded
/// upstream, outside this file's boundary.)
#[test]
fn nested_named_argument_control_stays_paired() {
    let source = format!("{PREAMBLE}void main() {{ A<A<Object? Function(int)>>(); }}");
    let outer = at(&source, "A<Object? Function(int)>");
    let inner = at(&source, "Object? Function(int)");
    assert_eq!(
        bounds_rows(&source),
        [
            (
                outer,
                "'A<Object? Function(int)>' doesn't conform to the bound 'num' of the type parameter 'T'."
                    .to_string()
            ),
            (
                inner,
                "'Never Function(int)' doesn't conform to the bound 'num' of the type parameter 'T'."
                    .to_string()
            ),
        ],
    );
}

/// A return-less function type is captured and checked — the spelling the
/// hand-over used to drop entirely. `dart analyze` on `A<Function()>();`
/// reports `'dynamic Function()' doesn't conform …` at the 10-unit
/// written argument.
#[test]
fn returnless_function_argument_is_captured() {
    let source = format!("{PREAMBLE}void main() {{ A<Function()>(); }}");
    let start = at(&source, "Function()");
    assert_eq!(
        bounds_rows(&source),
        [(
            start,
            "'dynamic Function()' doesn't conform to the bound 'num' of the type parameter 'T'."
                .to_string()
        )],
    );
}

/// A return-less member seats without shifting its neighbours:
/// `dart analyze` on `D<Function(), String>();` reports the first
/// argument against `T` and `String` against `U`.
#[test]
fn returnless_member_keeps_later_slots_aligned() {
    let source = format!("{PREAMBLE}void main() {{ D<Function(), String>(); }}");
    let fn_arg = at(&source, "Function()");
    let str_arg = at(&source, "String");
    assert_eq!(
        bounds_rows(&source),
        [
            (
                fn_arg,
                "'dynamic Function()' doesn't conform to the bound 'num' of the type parameter 'T'."
                    .to_string()
            ),
            (
                str_arg,
                "'String' doesn't conform to the bound 'num' of the type parameter 'U'."
                    .to_string()
            ),
        ],
    );
}

/// A return-less member after a named one keeps both slots:
/// `dart analyze` on `D<String, Function()>();` reports `String` against
/// `T` and the function type against `U`.
#[test]
fn named_then_returnless_keeps_both_slots() {
    let source = format!("{PREAMBLE}void main() {{ D<String, Function()>(); }}");
    let str_arg = at(&source, "String");
    let fn_arg = at(&source, "Function()");
    assert_eq!(
        bounds_rows(&source),
        [
            (
                str_arg,
                "'String' doesn't conform to the bound 'num' of the type parameter 'T'."
                    .to_string()
            ),
            (
                fn_arg,
                "'dynamic Function()' doesn't conform to the bound 'num' of the type parameter 'U'."
                    .to_string()
            ),
        ],
    );
}

/// A function-typed return anchors at the chain's first `Function`
/// keyword — the spelling that used to hand the group a file-start span.
/// `dart analyze` on `A<int Function() Function()>();` reports
/// `'int Function() Function()' …` at the 25-unit written chain.
#[test]
fn function_typed_return_anchors_at_the_chain_head() {
    let source = format!("{PREAMBLE}void main() {{ A<int Function() Function()>(); }}");
    let start = at(&source, "int Function() Function()");
    assert_eq!(
        bounds_rows(&source),
        [(
            start,
            "'int Function() Function()' doesn't conform to the bound 'num' of the type parameter 'T'."
                .to_string()
        )],
    );
}

/// The trailing `?` belongs to the type and joins the row anchor:
/// `dart analyze` on `A<void Function()?>();` reports
/// `'void Function()?' …` at the 16-unit argument.
#[test]
fn nullable_suffix_joins_the_anchor() {
    let source = format!("{PREAMBLE}void main() {{ A<void Function()?>(); }}");
    let start = at(&source, "void Function()");
    let rows = bounds_rows(&source);
    assert_eq!(
        rows,
        [(
            start,
            "'void Function()?' doesn't conform to the bound 'num' of the type parameter 'T'."
                .to_string()
        )],
    );
    let report = report_ops(&source);
    let row = report
        .diagnostics
        .iter()
        .find(|d| d.code.name == "type_argument_not_matching_bounds")
        .expect("bounds row");
    assert_eq!(row.range.length, 16, "the written `?` joins the extent");
}

/// The nullable suffix anchors on the return-typed spelling too:
/// `dart analyze` on `A<int Function()?>();` reports `'int Function()?'
/// …` at the 15-unit argument.
#[test]
fn nullable_suffix_on_spelled_return_joins_too() {
    let source = format!("{PREAMBLE}void main() {{ A<int Function()?>(); }}");
    let start = at(&source, "int Function()");
    let rows = bounds_rows(&source);
    assert_eq!(
        rows,
        [(
            start,
            "'int Function()?' doesn't conform to the bound 'num' of the type parameter 'T'."
                .to_string()
        )],
    );
    let report = report_ops(&source);
    let row = report
        .diagnostics
        .iter()
        .find(|d| d.code.name == "type_argument_not_matching_bounds")
        .expect("bounds row");
    assert_eq!(row.range.length, 15, "the written `?` joins the extent");
}

/// Conforming arguments stay silent: `dart analyze` on `A<int>();`
/// reports no bounds row (the check is live, not blanket-silent).
#[test]
fn conforming_named_argument_stays_silent() {
    let source = format!("{PREAMBLE}void main() {{ A<int>(); }}");
    assert_eq!(bounds_rows(&source), []);
}

/// The declaration spelling the ticket opens with — a bare function type
/// inside an unbounded named type's argument group — parses and diagnoses
/// silently: `dart analyze` on `List<void Function()> xs = [];` reports
/// nothing.
#[test]
fn declaration_argument_group_stays_silent() {
    let source = format!("{PREAMBLE}List<void Function()> xs = [];void main() {{ }}");
    assert_eq!(bounds_rows(&source), []);
}

/// The negative control — a genuinely non-conforming named argument still
/// reports: `dart analyze` on `A<B>();` reports `'B' doesn't conform …`
/// at the head itself.
#[test]
fn nonconforming_named_argument_still_reports() {
    let source = format!("{PREAMBLE}void main() {{ A<B>(); }}");
    let start = at(&source, "B>(");
    assert_eq!(
        bounds_rows(&source),
        [(
            start,
            "'B' doesn't conform to the bound 'num' of the type parameter 'T'.".to_string()
        )],
    );
}
