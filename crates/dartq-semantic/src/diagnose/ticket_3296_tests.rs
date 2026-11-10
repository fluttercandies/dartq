//! The #3296 pins: the `type_argument_not_matching_bounds` bare-head face.
//!
//! #3294 measured the family's residual and found the verdict layer correct
//! on every site the engine drove — the gaps were sites that never reached a
//! verdict at all. The dominant one: `PendingBoundedCheck::Site` was only
//! pushed from `Engine::end_group`, so a named type with no written `<...>`
//! group never enqueued, even though upstream runs `checkNamedType` at every
//! `NamedType` and a bare head answers its instantiate-to-bounds form
//! (`type_arguments_verifier.dart:243-247`, the raw-type context row at
//! :441-446). The pins below hold the head-token enqueue in place, in both
//! directions: the group-less heads that must report, and the written-group
//! and silence shapes that must not move.
//!
//! Oracle texts are the corpus spellings, never hand-crafted
//! (`TypeArgumentNotMatchingBoundsTest.test_extensionType_superBounded`,
//! `doc_0`).

use super::test_rig::report_ops;

/// The `type_argument_not_matching_bounds` rows of `report` as
/// `offset|length|message`.
fn bounds_rows(report: &super::FileReport) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "type_argument_not_matching_bounds")
        .map(|d| format!("{}|{}|{}", d.range.offset, d.range.length, d.message))
        .collect()
}

/// The reported length of one `offset|length|message` row.
fn row_len(row: &str) -> &str {
    row.split('|').nth(1).unwrap_or("")
}

/// The reported message of one `offset|length|message` row.
fn row_message(row: &str) -> String {
    row.splitn(3, '|').nth(2).unwrap_or("").to_string()
}

/// A bare-head parameter annotation reports at the head: the corpus shape
/// (`void f(A a)` over `extension type A<T extends A<T>>`) — the raw
/// instantiation `A<A<dynamic>>` is not regular-bounded, and the argument
/// `A<dynamic>` is the reported one. The row anchors at the whole `NamedType`
/// node, which a bare head spells as its own name token.
#[test]
fn bare_extension_type_head_reports() {
    let hits = bounds_rows(&report_ops(
        "extension type A<T extends A<T>>(int it) {}\nvoid f(A a) {}\n",
    ));
    assert_eq!(hits.len(), 1, "{hits:#?}");
    assert_eq!(row_len(&hits[0]), "1", "{hits:#?}");
    assert!(
        row_message(&hits[0]).ends_with(
            "'A<dynamic>' doesn't conform to the bound 'A<A<dynamic>>' of the type parameter 'T'."
        ),
        "{hits:#?}"
    );
}

/// A nullable bare head reports the same way and the row spans the whole type
/// node (`C?`, the oracle's `_typeArgumentErrorNode` fallback anchoring at the
/// `NamedType`): the `?` is part of the reported node.
#[test]
fn bare_nullable_head_reports() {
    let hits = bounds_rows(&report_ops(
        "extension type A<T extends A<T>>(int it) {}\nA? source;\n",
    ));
    assert_eq!(hits.len(), 1, "{hits:#?}");
    assert_eq!(row_len(&hits[0]), "2", "{hits:#?}");
    assert!(
        row_message(&hits[0]).ends_with(
            "'A<dynamic>' doesn't conform to the bound 'A<A<dynamic>>' of the type parameter 'T'."
        ),
        "{hits:#?}"
    );
}

/// An implicit creation's own group reports too (`var a = A<String>()`): the
/// group arrives headless, so the callee head recovered from the constructor
/// reference re-opens the site and the row anchors at the written argument —
/// the corpus `doc_0` shape.
#[test]
fn implicit_creation_group_reports() {
    let hits = bounds_rows(&report_ops(
        "class A<E extends num> {}\nvar a = A<String>();\n",
    ));
    assert_eq!(hits.len(), 1, "{hits:#?}");
    // The row anchors at the written argument (`String`), not the head.
    assert_eq!(row_len(&hits[0]), "6", "{hits:#?}");
    assert!(
        row_message(&hits[0])
            .ends_with("'String' doesn't conform to the bound 'num' of the type parameter 'E'."),
        "{hits:#?}"
    );
}

/// A written group head keeps its single row: the head-token enqueue must not
/// double-count the head that its own site already judged.
#[test]
fn written_group_head_stays_single() {
    let hits = bounds_rows(&report_ops(
        "class C<T extends int> {}\nvoid f(C<String> x) {}\n",
    ));
    assert_eq!(hits.len(), 1, "{hits:#?}");
}

/// A bare clause head keeps its single row: the container header owns it
/// (`check_clause_sites`), and the head-token enqueue must not add a second.
#[test]
fn bare_clause_head_stays_single() {
    let hits = bounds_rows(&report_ops(
        "class A<T extends A<T>> {}\nclass B extends A {}\n",
    ));
    assert_eq!(hits.len(), 1, "{hits:#?}");
}

/// An in-bounds bare head stays silent (`C` over `C<T extends num>`
/// instantiates to `num`, which satisfies its own bound).
#[test]
fn in_bounds_bare_head_stays_silent() {
    let report = report_ops("class C<T extends num> {}\nvoid f(C c) {}\n");
    assert_eq!(bounds_rows(&report), Vec::<String>::new(), "{report:#?}");
}

/// A non-generic bare head stays silent (`int` names no parameterized
/// container, so there is no bound to violate).
#[test]
fn non_generic_bare_head_stays_silent() {
    let report = report_ops("void f(int x) {}\nvar s = 'x';\n");
    assert_eq!(bounds_rows(&report), Vec::<String>::new(), "{report:#?}");
}

/// A generic-function-type argument whose conformance is decidable reports:
/// the bound `F` expands through `map_tree`'s alias arm (#3510 ended the
/// pre-alias map that read every alias-headed bound `dynamic`) into
/// `T Function<T>(T)`, and `int Function(int)` misses it on the signatures'
/// type-parameter count — one `<T>` binder against zero, `function_subtype`'s
/// first refusal (subtype.dart:502-510). The earlier premise — that this
/// conformance was undecidable and the verdict suppressed — was falsified by
/// the oracle 3.13.7 probe on the fixture below:
///
/// ```text
/// ERROR|COMPILE_TIME_ERROR|TYPE_ARGUMENT_NOT_MATCHING_BOUNDS|main.dart|3|11|17|'int Function(int)' doesn't conform to the bound 'F' of the type parameter 'T'.
/// ```
///
/// The row's span diverges in length only: the oracle anchors the whole
/// `GenericFunctionType` node (17 columns), the captured tree's range covers
/// the `int Function` head region (12) — a documented span divergence, the
/// code identical.
#[test]
fn generic_function_type_argument_reports_the_arity_mismatch() {
    let hits = bounds_rows(&report_ops(
        "typedef F = T Function<T>(T);\nclass CB<T extends F> {}\n\
         void f(CB<int Function(int)> a) {}\n",
    ));
    assert_eq!(hits.len(), 1, "{hits:#?}");
    assert_eq!(row_len(&hits[0]), "12", "{hits:#?}");
    assert!(
        row_message(&hits[0]).ends_with(
            "'int Function(int)' doesn't conform to the bound 'F' of the type parameter 'T'."
        ),
        "{hits:#?}"
    );
}

// —— the residual this face does not reach ——————————————————————————————

/// A bare head over a *function-type alias* still reports nothing: the head
/// now reaches a site, but the verdict's instantiate-to-bounds answer has no
/// `decl_params` entry for an alias element
/// (`Bounds::bounds_instantiation` → `None`), so `check_named` falls back to
/// the alias-RHS silence. The written-argument alias face
/// (`check_alias_rhs_sites`) and the container heads above decide normally.
///
/// Ignored until `crates/dartq-semantic/src/bounds.rs` registers the alias
/// declaration among `decl_params` (the `TypeArgumentNotMatchingBounds` corpus
/// `test_functionTypeAlias` / co19 `TypeArgumentNotMatchingBoundsTest`'s
/// `test_notRegularBounded_notSuperBounded_parameter_invariant`, whose bare
/// `foo(G g)` over `typedef G<X extends A<X>> = void Function<Y extends X>()`
/// reports upstream).
#[ignore = "alias heads answer no instantiate-to-bounds form: Bounds::bounds_instantiation has no decl_params entry for an alias element (bounds.rs)"]
#[test]
fn bare_alias_head_reports() {
    let hits = bounds_rows(&report_ops(
        "typedef A<X> = X Function(X);\n\
         typedef G<X extends A<X>> = void Function<Y extends X>();\n\
         foo(G g) {}\n",
    ));
    assert_eq!(hits.len(), 1, "{hits:#?}");
}
