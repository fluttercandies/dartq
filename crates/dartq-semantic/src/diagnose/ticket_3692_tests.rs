//! #3692 pins — a generic-function-type bound (`f<F extends num>()`'s
//! `T extends void Function<U extends B>()` family) must keep its
//! signature's own variables through the capture: the diagnose-side
//! sinks never receive the collector's per-variable pairing, so the
//! sealed bound node degraded to `void Function()` and every face
//! reading it under-reported. The landed fix (#3692/#3706 merge) records
//! each signature group's window in `Bounds` at its close and stitches
//! it into the sealed node's quantifier columns at `end_function`
//! (`stitch_signature_variables`), and both display faces render a
//! binder's declared bound beside its name (`<U extends B>`) the way
//! the official `_writeTypeParameters` does
//! (display_string_builder.dart:330-343, :498-509).
//!
//! Faces pinned here:
//!
//! * the class-site annotation verdict — `C<F>` over `class C<X extends
//!   void Function<U extends B>()>` reports the oracle-verbatim row
//!   through the `relateTypeParameters` correlate leg once the bound
//!   carries its binder facts;
//! * the matching twin stays silent (mutual bounds agree);
//! * the plain function-type bound (`void Function()`, no quantifier)
//!   keeps reporting — the stitch must not widen the silent face;
//! * the nested signature bound (`U extends G<String>`) still walks and
//!   reports at the declaration (`String` vs `G`'s `num`).
//!
//! The call-site variant (`f<F>()` through the callee's own
//! `exec_bounds` window) stays `#[ignore]`d: the judged bound tree is
//! now complete on this lane's side, but the executable latch itself
//! never pairs — `begin_formal_parameters` at depth 0 inside the GFT
//! bound clears `pending_exec_name` before `end_type_variables` runs
//! `pair_exec_window` (listener.rs's `#1314 FAM10` clear) — a
//! listener.rs change this lane's scope cannot write.
//!
//! The class-site variant shares a second, earlier blocker: a class
//! whose clause nests a signature group (`class C<X extends void
//! Function<U extends B>()>`) never registers its window at all —
//! `Engine::close_type_variables` (type_capture.rs) latches the flat
//! `tv_window` into `pending_tv` unconditionally, so the nested group's
//! close consumes the window and the enclosing group's close clobbers
//! `pending_tv` to `None`; `note_container_type_variables` then returns
//! before `Bounds::note_container` runs and the container's captured
//! bounds stay empty (the judged bound reads as absent and every face
//! on it stays silent). `Bounds::pending` itself holds the correct
//! window (its `frames` stack survives the nesting) — the adoption
//! gate alone is engine-side.
//!
//! Oracle ground truth (upstream Dart SDK source,
//! `pkg/analyzer/test/src/diagnostics/
//! type_argument_not_matching_bounds_test.dart`
//! `test_methodInvocation_genericFunctionTypeArgument_mismatch#0`
//! and this lane's `~/.dartq-oracle/3.13.7` probes on /tmp fixtures):
//! `'F' doesn't conform to the bound 'void Function<U extends B>()' of
//! the type parameter 'T'.` anchored at the written argument.

use super::test_rig::{assert_silent, report_ops, rows};

/// The oracle-verbatim mismatch case: a `void Function<T extends A>()`
/// argument against the `void Function<U extends B>()` bound.
///
/// Oracle (`TypeArgumentNotMatchingBoundsTest/
/// test_methodInvocation_genericFunctionTypeArgument_mismatch#0`): one row,
/// `type_argument_not_matching_bounds` at the `F` argument.
#[test]
fn gft_bound_mismatch_reports() {
    let report = report_ops(
        "class A {}\n\
         class B {}\n\
         typedef F = void Function<T extends A>();\n\
         void f<T extends void Function<U extends B>()>() {}\n\
         void g() {\n\
           f<F>();\n\
         }\n",
    );
    let names: Vec<String> = rows(&report);
    assert!(
        names
            .iter()
            .any(|r| r.contains("type_argument_not_matching_bounds")),
        "expected type_argument_not_matching_bounds, got {names:?}"
    );
}

/// Same-family plain function-type bound: `T extends void Function()`
/// misses identically — the signature's own `()` clears the pairing latch
/// even with no nested `<...>` group (`int` is no function type, so the
/// oracle reports).
#[test]
fn plain_fn_bound_mismatch_reports() {
    let report = report_ops(
        "void f<T extends void Function()>() {}\n\
         void g() {\n\
           f<int>();\n\
         }\n",
    );
    let names: Vec<String> = rows(&report);
    assert!(
        names
            .iter()
            .any(|r| r.contains("type_argument_not_matching_bounds")),
        "expected type_argument_not_matching_bounds, got {names:?}"
    );
}

/// The matching-bound twin stays silent — the same signature with `num`
/// on both sides (`test_methodInvocation_genericFunctionTypeArgument_
/// match#0` spells `num`; the rig's prelude declares it). Silent today for
/// the wrong reason (the judged bound never lands); it must STAY silent
/// through the real `relateTypeParameters` bound-equality leg once the
/// capture lands — this pin guards both directions.
#[test]
fn gft_bound_match_is_silent() {
    let report = report_ops(
        "typedef F = void Function<T extends num>();\n\
         void f<T extends void Function<X extends num>()>() {}\n\
         void g() {\n\
           f<F>();\n\
         }\n",
    );
    assert_silent(&report);
}

/// Negative control: a non-generic bound mismatch through the same
/// verdict still reports — the capture fix must not widen the silent face
/// (`T extends num` bound, `String` argument).
#[test]
fn non_generic_bound_mismatch_still_reports() {
    let report = report_ops(
        "void f<T extends num>() {}\n\
         void g() {\n\
           f<String>();\n\
         }\n",
    );
    let names: Vec<String> = rows(&report);
    assert!(
        names
            .iter()
            .any(|r| r.contains("type_argument_not_matching_bounds")),
        "expected type_argument_not_matching_bounds, got {names:?}"
    );
}

/// The class-site verdict this lane's capture fix unlocks: `C<F>` over
/// `class C<X extends void Function<U extends B>()>` — the bound's sealed
/// node now carries `(U, B)`, the snapshot's binder facts arm the
/// correlate leg, and the row renders the bound with its quantifier's
/// declared bound. Oracle (this lane's /tmp probe of the upstream case's
/// class-site twin): `'F' doesn't conform to the bound 'void Function<U
/// extends B>()' of the type parameter 'X'.` — no context rows.
#[test]
fn class_site_gft_bound_mismatch_reports() {
    let report = report_ops(
        "class A {}\n\
         class B {}\n\
         typedef F = void Function<T extends A>();\n\
         class C<X extends void Function<U extends B>()> {}\n\
         void g() {\n\
           C<F>? x;\n\
           x;\n\
         }\n",
    );
    let names: Vec<String> = rows(&report);
    assert!(
        names.iter().any(|r| r.contains(
            "'F' doesn't conform to the bound 'void Function<U extends B>()' of the type parameter 'X'"
        )),
        "expected the oracle-verbatim GFT-bound row, got {names:?}"
    );
}

/// The matching twin of the class-site pin: `C<G>` over the same bound
/// with `G = void Function<T extends B>()` — the correlate leg's mutual
/// subtype holds and the site stays silent (the pre-stitch silence was
/// the under-report; this pin fences the fix against over-firing).
#[test]
fn class_site_gft_bound_match_is_silent() {
    let report = report_ops(
        "class B {}\n\
         typedef G = void Function<T extends B>();\n\
         class C<X extends void Function<U extends B>()> {}\n\
         void g() {\n\
           C<G>? x;\n\
           x;\n\
         }\n",
    );
    assert_silent(&report);
}

/// The plain function-type bound keeps reporting at the same site
/// (`C<D<int>>` over `class D<X extends void Function()>`): the stitch
/// must not disturb the binder-less shape's verdict. Oracle (this lane's
/// /tmp probe): `'int' doesn't conform to the bound 'void Function()' of
/// the type parameter 'X'.`
#[test]
fn class_site_plain_fn_bound_mismatch_still_reports() {
    let report = report_ops(
        "class D<X extends void Function()> {}\n\
         void g() {\n\
           D<int>? x;\n\
           x;\n\
         }\n",
    );
    let names: Vec<String> = rows(&report);
    assert!(
        names.iter().any(|r| r.contains(
            "'int' doesn't conform to the bound 'void Function()' of the type parameter 'X'"
        )),
        "expected the plain-function-bound row, got {names:?}"
    );
}

/// The stitched bound tree still walks at the declaration: the
/// signature's own binder bound (`U extends G<String>`) reports its
/// nested violation exactly as the binder-less shape did. Oracle (this
/// lane's /tmp probe): `'String' doesn't conform to the bound 'num' of
/// the type parameter 'X'.` at the `G<String>` argument.
#[test]
fn class_decl_nested_signature_bound_reports() {
    let report = report_ops(
        "class G<X extends num> {}\n\
         class C<X extends void Function<U extends G<String>>()>() {}\n",
    );
    let names: Vec<String> = rows(&report);
    assert!(
        names
            .iter()
            .any(|r| r.contains("'String' doesn't conform to the bound 'num'")),
        "expected the nested signature-bound row, got {names:?}"
    );
}
