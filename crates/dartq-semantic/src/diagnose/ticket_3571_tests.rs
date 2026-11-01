//! #3571 pins — the bounds-family row extents (offset and length).
//!
//! # The defect
//!
//! 197 corpus rows carried the right code at the right offset with a
//! short length, from the anchor computations the recon
//! (docs/audits/recon-underreport-wave.md T1) mapped to the
//! `type_argument_not_matching_bounds`/`could_not_infer` emission
//! points. The oracle anchors each row over the whole written extent
//! while the engine kept a token-sized face:
//!
//! - the invocation-site argument anchor (`check_call_type_arg_bounds`,
//!   `args.spans[index]`) and the bounds-face argument anchor
//!   (`emit_bounded_issue`, `issue.range`) kept the 8-char `Function`
//!   keyword head of a function-type-literal type argument; the oracle's
//!   `_typeArgumentErrorNode` anchors the whole written TypeAnnotation
//!   (`Function()` spans 10, `Function(int, [String])` spans 23),
//! - the creation-inference `could_not_infer` anchor
//!   (`check_creation_inference_at`) kept the bare class head; the
//!   oracle writes at the whole ConstructorName chain (`A.named()`
//!   spans 7).
//!
//! # The contract
//!
//! Every expectation below was taken from the pinned oracle:
//! `~/.dartq-oracle/3.13.7/dart-sdk/bin/dart analyze --format=machine`
//! on exactly these fixtures. The widening is extent-only: which row
//! reports, its offset, and its message are untouched, and every
//! neighbouring shape whose captured face already spans whole (the bare
//! `Function` class argument, a nullable literal, a parameterized head,
//! the unnamed creation) keeps it — those rows are pinned as controls.
//!
//! # Adjacent capture gaps (reported, not pinned)
//!
//! Three faces drop the whole row before any anchor question arises —
//! missing-row defects outside this ticket's extent-only scope, each
//! with the oracle's shape noted for the owning face:
//!
//! - the creation-ref capture (`new C<Function()>()` — oracle reports
//!   the argument anchor; dartq no row, the #3475 bare-function-type
//!   capture gap);
//! - the alias-RHS face (`typedef F = C<Function()>;` — oracle reports
//!   the argument anchor; dartq no row);
//! - the named and keyword creations (`B.named()`, `new B()` — the
//!   oracle reports the twin at the head and the `could_not_infer` over
//!   the whole ConstructorName chain; dartq no row — the callee row
//!   never reaches the creation-inference face, so its
//!   `written_name_chain_span` widening has no firing shape today; the
//!   unnamed-form pin below carries the face's extent contract).

use super::test_rig::report_ops_bare;

/// The `(offset, length)` of every row of `code`, in report order. The
/// bare rig diagnoses the fixture verbatim, so every asserted offset is
/// the fixture's own (`offset_of` face) — no prelude padding.
fn spans_of(source: &str, code: &str) -> Vec<(u32, u32)> {
    let report = report_ops_bare(source);
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == code)
        .map(|d| (d.range.offset, d.range.length))
        .collect()
}

/// The UTF-16 offset of `needle`'s first occurrence (ASCII fixtures:
/// byte offsets equal the row offsets).
fn offset_of(source: &str, needle: &str) -> u32 {
    source.find(needle).expect("needle present") as u32
}

/// The invocation-site face: every written type argument anchors its own
/// extent. `dart analyze` on this fixture reports, in order — `Function`
/// 8 (the bare class control), `Function()` 10, `Function(int,
/// [String])` 23, `Function()?` 11 (nullable control), and
/// `List<Function()>` 16 (parameterized-head control).
#[test]
fn invocation_site_argument_extents() {
    let source = concat!(
        "class C<T extends num> {\n",
        "  C();\n",
        "  C.named();\n",
        "}\n",
        "void main() {\n",
        "  C<Function> a;\n",
        "  C<Function()> b;\n",
        "  C<Function(int, [String])> c;\n",
        "  C<Function()?> d;\n",
        "  C<List<Function()>> e;\n",
        "}",
    );
    assert_eq!(
        spans_of(source, "type_argument_not_matching_bounds"),
        vec![
            (offset_of(source, "Function> a;"), 8),
            (offset_of(source, "Function()> b;"), 10),
            (offset_of(source, "Function(int, [String])> c;"), 23),
            (offset_of(source, "Function()?> d;"), 11),
            (offset_of(source, "List<Function()>> e;"), 16),
        ],
    );
}

/// The declaration faces (clause site, bound annotation): the same
/// whole-literal argument anchor. The bare `Function` (8) and nullable
/// `Function?` (9) rows are the controls — their captured faces already
/// span whole and must not move.
#[test]
fn declaration_site_argument_extents() {
    let source = concat!(
        "class C<T extends num> {}\n",
        "class D0 extends C<Function> {}\n",
        "class D extends C<Function()> {}\n",
        "class D2 extends C<Function?> {}\n",
        "class E<T extends C<Function()>> {}\n",
    );
    assert_eq!(
        spans_of(source, "type_argument_not_matching_bounds"),
        vec![
            (offset_of(source, "Function> {}"), 8),
            (offset_of(source, "Function()> {}"), 10),
            (offset_of(source, "Function?> {}"), 9),
            (offset_of(source, "Function()>> {}"), 10),
        ],
    );
}

/// The creation-inference face control: the unnamed creation — the one
/// form the engine routes to the face — anchors its `could_not_infer`
/// and twin rows at the bare class head (1), exactly the oracle's
/// extent (the chain is the name itself; `written_name_chain_span`
/// grows a `.selector` continuation and answers `None` here). The
/// keyword and named-creation forms the oracle also reports are the
/// module-header capture gaps, not extent defects.
#[test]
fn creation_inference_extents() {
    let source = concat!(
        "class Box<T> {}\n",
        "class B<T extends Box<T>> {}\n",
        "void main() {\n",
        "  B();\n",
        "}\n",
    );
    let head = offset_of(source, "B();");
    assert_eq!(
        spans_of(source, "type_argument_not_matching_bounds"),
        vec![(head, 1)],
    );
    assert_eq!(spans_of(source, "could_not_infer"), vec![(head, 1)]);
}
