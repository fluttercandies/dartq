//! The #2936 pins: `checkNamedType` runs at every `NamedType`
//! (type_arguments_verifier.dart:243 -> :312), so clause and
//! bound-annotation positions check their arguments like any other site —
//! including the nested `NamedType`s inside their argument lists (the
//! `A<X?>` of `class A<X extends FutureOr<A<X?>>>`), a bare clause head's
//! instantiate-to-bounds form (`extends A` resolving to `A<B<dynamic>>`),
//! and a type alias's right-hand side (`GenericTypeAlias` parents deny the
//! super pass exactly like the clause contexts,
//! `_shouldAllowSuperBoundedTypes` :737-748). The could_not_infer rows pin
//! the message shape the raw-constructor inference face already emits
//! (generic_inferrer.dart:535-538 through
//! type_constraint.dart:263-300's "declared to extend ... producing").

use super::test_rig::{CORE_PRELUDE, report_ops};
use super::*;

fn bounds_rows(report: &FileReport) -> Vec<(u32, u32, String)> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "type_argument_not_matching_bounds")
        .map(|d| (d.range.offset, d.range.length, d.message.to_string()))
        .collect()
}

fn infer_rows(report: &FileReport) -> Vec<(u32, u32, String)> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "could_not_infer")
        .map(|d| (d.range.offset, d.range.length, d.message.to_string()))
        .collect()
}

/// The context-message texts of the first bounds row (the super-bounded
/// and raw-type context rows ride `contextMessages`).
fn bounds_contexts(report: &FileReport) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .find(|d| d.code.name == "type_argument_not_matching_bounds")
        .map(|d| {
            d.context
                .as_deref()
                .unwrap_or_default()
                .iter()
                .map(|c| c.message.to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// Byte offset of `needle` inside the fixture source, shifted by the
/// rig's core prelude (the fixture rides behind it on one line).
fn at(source: &str, needle: &str) -> u32 {
    (CORE_PRELUDE.len() + source.find(needle).expect("needle in fixture")) as u32
}

/// The extends-clause argument face
/// (TypeArgumentNotMatchingBoundsTest/test_extends_regressionInIssue18468Fix):
/// `U` inside `extends X<U>` checks `X`'s `T extends Type` — the clause
/// parent denies the super pass, so the violation reports plain.
#[test]
fn tab_clause_extends_argument() {
    let source = r#"
class X<T extends Type> {}
class Y<U> extends X<U> {}
"#;
    let report = report_ops(source);
    assert_eq!(
        bounds_rows(&report),
        [(
            at(source, "U> {}"),
            1,
            "'U' doesn't conform to the bound 'Type' of the type parameter 'T'.".to_string()
        )],
        "{:?}",
        report.diagnostics
    );
}

/// The with/implements/on clause faces share the denial arm
/// (`WithClause`/`ImplementsClause`/`MixinOnClause`,
/// type_arguments_verifier.dart:745-748): `compatible_t06`'s
/// `with M<Y, X>` shape checks each argument against the mixin head's
/// own parameters.
#[test]
fn tab_implements_with_on_clauses() {
    let source = r#"
class S {}
class T {}
class C<X extends S, Y extends T> {}
mixin M<X extends S, Y extends T> {}
class MA<X, Y> extends C<Y, X> with M<Y, X> {}
class MB<X, Y> implements C<Y, X> {}
"#;
    let report = report_ops(source);
    let rows = bounds_rows(&report);
    // `C<Y, X>` at extends: `Y` vs `S`, `X` vs `T` — both fail.
    // `M<Y, X>` at with: same pair.
    // `C<Y, X>` at implements: same pair.
    assert_eq!(rows.len(), 6, "{:?}", report.diagnostics);
    assert_eq!(
        rows[0],
        (
            at(source, "Y, X> with"),
            1,
            "'Y' doesn't conform to the bound 'S' of the type parameter 'X'.".to_string()
        )
    );
    assert_eq!(
        rows[1],
        (
            at(source, "X> with"),
            1,
            "'X' doesn't conform to the bound 'T' of the type parameter 'Y'.".to_string()
        )
    );
    // The with-clause pair sits inside `M<...>`; the implements pair is
    // the last two rows.
    assert_eq!(
        rows[2],
        (
            at(source, "Y, X> {") + 0,
            1,
            "'Y' doesn't conform to the bound 'S' of the type parameter 'X'.".to_string()
        )
    );
    assert_eq!(
        rows[4],
        (
            at(source, "implements C<Y, X>") + "implements C<".len() as u32,
            1,
            "'Y' doesn't conform to the bound 'S' of the type parameter 'X'.".to_string()
        )
    );
}

/// The bound-internal argument (co19 `class_FutureOr_l1_t05` /
/// `class_l1_t08`): the `X?` inside `X extends FutureOr<A<X?>>` (or
/// `A<X?>`) is a `NamedType` of its own — `X? <: B` conjuncts `Null <: B`,
/// which the `FutureOr`/`A` bound rejects, so the regular pass's failure
/// is decidable and the inverted pass fails identically (the bound
/// annotation's `TypeParameter` parent keeps the super pass allowed, so
/// the report carries the inverted-type context row).
#[test]
fn tab_bound_internal_argument() {
    let source = r#"
class A<X extends A<X?>> {}
"#;
    let report = report_ops(source);
    let rows = bounds_rows(&report);
    assert_eq!(rows.len(), 1, "{:?}", report.diagnostics);
    assert_eq!(
        rows[0],
        (
            at(source, "X?>>"),
            2,
            "'X?' doesn't conform to the bound 'A<X?>' of the type parameter 'X'.".to_string()
        )
    );
    let contexts = bounds_contexts(&report);
    assert!(
        contexts
            .iter()
            .any(|c| c.contains("The inverted type 'A<X?>'")),
        "the super pass's context row: {contexts:?}"
    );
}

/// A bare clause head resolves to its instantiate-to-bounds form
/// (co19 `instantiate_to_bounds_super_bounded_test_14/15/16/17/20/21`):
/// `extends A` resolves `A<B<dynamic>>` — `B<dynamic>` fails `B<T>`'s
/// substituted `B<B<dynamic>>`, the clause parent denies the super pass,
/// and the report anchors at the `NamedType` itself
/// (`_typeArgumentErrorNode`, type_arguments_verifier.dart:757-768) with
/// the raw-type context row (`:441-446`).
#[test]
fn tab_bare_clause_head_instantiated_to_bounds() {
    let source = r#"
class B<T> {}
class A<T extends B<T>> {}
class Test extends A {}
"#;
    let report = report_ops(source);
    let rows = bounds_rows(&report);
    assert_eq!(rows.len(), 1, "{:?}", report.diagnostics);
    assert_eq!(
        rows[0],
        (
            at(source, "A {}"),
            1,
            "'B<dynamic>' doesn't conform to the bound 'B<B<dynamic>>' of the type parameter 'T'."
                .to_string()
        )
    );
    let contexts = bounds_contexts(&report);
    assert!(
        contexts
            .iter()
            .any(|c| c.contains("The raw type was instantiated as 'A<B<dynamic>>'")),
        "the raw-type context row: {contexts:?}"
    );
}

/// A bare clause head whose instantiate-to-bounds form is well-bounded
/// stays silent (`class A<T>` reads `A<dynamic>` — unbounded).
#[test]
fn tab_bare_clause_head_well_bounded_stays_silent() {
    let source = r#"
class A<T> {}
class Test extends A {}
"#;
    let report = report_ops(source);
    assert!(bounds_rows(&report).is_empty(), "{:?}", report.diagnostics);
}

/// The nested-`NamedType` walk: `extends X<G<B>>` checks `G<B>`'s `B`
/// against `G`'s bound as its own `NamedType` — the parent
/// `TypeArgumentList` is not in the deny list, so the super pass runs and
/// the report carries the inverted-type context row.
#[test]
fn tab_clause_nested_argument_checks_its_own_head() {
    let source = r#"
class A {}
class B {}
class G<E extends A> {}
class X<T> {}
class Y extends X<G<B>> {}
"#;
    let report = report_ops(source);
    let rows = bounds_rows(&report);
    assert_eq!(rows.len(), 1, "{:?}", report.diagnostics);
    assert_eq!(
        rows[0],
        (
            at(source, "B>> {}"),
            1,
            "'B' doesn't conform to the bound 'A' of the type parameter 'E'.".to_string()
        )
    );
    let contexts = bounds_contexts(&report);
    assert!(
        contexts
            .iter()
            .any(|c| c.contains("The inverted type 'G<B>'")),
        "the nested report's context row: {contexts:?}"
    );
}

/// The alias-RHS face (co19 `nonfunction_l1_t04`): a named right-hand
/// side's `GenericTypeAlias` parent denies the super pass like the clause
/// contexts — `typedef B<X extends A<A<X>>> = A<X>;` reports the bound's
/// own argument `A<X>` (which fails `A`'s substituted `A<A<X>>`), the
/// nested `X` inside it, and the RHS's `X` — both `X` arguments decide
/// through X's declared bound `A<A<X>>` (`A<A<X>>` is not a subtype of
/// `A<X>`), the decl-scoped `bound(X) <: B` rule.
#[test]
fn tab_alias_rhs_named_side() {
    let source = r#"
class A<X extends A<X>> {}
typedef B<X extends A<A<X>>> = A<X>;
"#;
    let report = report_ops(source);
    let rows = bounds_rows(&report);
    assert!(
        rows.iter().any(|(offset, len, msg)| {
            *len == 4
                && *offset == at(source, "A<X>>> =")
                && msg.contains("'A<X>' doesn't conform to the bound 'A<A<X>>'")
        }),
        "the bound's own argument reports: {:?}",
        rows
    );
    let x_args: Vec<_> = rows
        .iter()
        .filter(|(_, len, msg)| {
            *len == 1 && msg.contains("'X' doesn't conform to the bound 'A<X>'")
        })
        .collect();
    assert_eq!(x_args.len(), 2, "both `X` arguments report: {rows:?}");
    assert_eq!(x_args[0].0, at(source, "X>>> ="));
    assert_eq!(x_args[1].0, at(source, "X>;"));
}

/// The satisfied clause family stays silent — well-bounded arguments at
/// every clause kind, a bare head whose i2b form conforms, and a bound
/// annotation whose nullable argument's `Null <: bound` conjunct keeps
/// the verdict on the unmodelable half.
#[test]
fn tab_clause_family_stays_silent() {
    let source = r#"
class A {}
class G<E extends A> {}
class X<T> {}
class Y extends X<G<A>> {}
class Z<U extends A> extends X<G<U>> {}
"#;
    let report = report_ops(source);
    assert!(bounds_rows(&report).is_empty(), "{:?}", report.diagnostics);
}

/// The legacy-corpus silence control: an unbounded in-scope parameter
/// argument and a bound that satisfies through the hierarchy never
/// report — the recursive walk adds no new rows on well-bounded sources.
#[test]
fn tab_satisfied_nested_and_bounded_args_silent() {
    let source = r#"
class num2 {}
class A<X extends num> {}
class B<Y extends num> extends A<Y> {}
class C<Z> extends A<Z> {}
"#;
    // `A<Y>`: `Y extends num` — `Y <: num` through its own bound
    // (suppressed as unmodelable, the landed posture). `A<Z>`: `Z`
    // unbounded — `Z <: num` is false in the oracle too... but `Z` is an
    // in-scope UNBOUNDED parameter, never suppressed: it reports.
    let report = report_ops(source);
    let rows = bounds_rows(&report);
    assert_eq!(rows.len(), 1, "{:?}", report.diagnostics);
    assert_eq!(
        rows[0],
        (
            at(source, "Z> {}"),
            1,
            "'Z' doesn't conform to the bound 'num' of the type parameter 'X'.".to_string()
        )
    );
}

/// The could_not_infer message shape the raw-constructor inference face
/// already emits (generic_inferrer.dart:535-538 +
/// type_constraint.dart:263-300): `class B<X extends B<X>>` then a bare
/// `B()` seeds `B<Object?>` against `B<B<Object?>>` and reports the
/// "declared to extend ... producing" detail.
#[test]
fn i2b_could_not_infer_extends_clause_origin() {
    let source = r#"
class B<X extends B<X>> {}
main() {
  B();
}
"#;
    let report = report_ops(source);
    let rows = infer_rows(&report);
    assert!(
        rows.iter().any(|(_, _, msg)| {
            msg.contains("Couldn't infer type parameter 'X'")
                && msg.contains("Tried to infer 'B<Object?>' for 'X'")
                && msg.contains(
                    "Type parameter 'X' is declared to extend 'B<X>' producing 'B<B<Object?>>'",
                )
        }),
        "the declared-to-extend origin text: {:?}",
        rows
    );
}

/// The alias-callee route (co19 `nonfunction_l2_t09` shape): `typedef A<X1
/// extends B<X2>, X2 extends B<X2>> = C<X1, X2>;` then a bare `A()`
/// infers the ALIAS's parameters — `B<Object?>` seeds against the
/// `B<X2>`-substituted `B<B<Object?>>` — and reports per failing
/// parameter, spelling the alias's own type variables (#2841 posture).
#[test]
fn i2b_could_not_infer_alias_rhs_creation() {
    let source = r#"
class B<X extends B<X>> {}
class C<X, Y> {}
typedef A<X1 extends B<X2>, X2 extends B<X2>> = C<X1, X2>;
main() {
  A();
}
"#;
    let report = report_ops(source);
    let rows = infer_rows(&report);
    assert!(
        rows.iter().any(|(_, _, msg)| {
            msg.contains("Couldn't infer type parameter 'X1'")
                && msg.contains("Tried to infer 'B<Object?>' for 'X1'")
                && msg.contains(
                    "Type parameter 'X1' is declared to extend 'B<X2>' producing 'B<B<Object?>>'",
                )
        }),
        "the alias X1 origin text: {:?}",
        rows
    );
    assert!(
        rows.iter().any(|(_, _, msg)| {
            msg.contains("Couldn't infer type parameter 'X2'")
                && msg.contains("Tried to infer 'B<Object?>' for 'X2'")
        }),
        "the alias X2 origin text: {:?}",
        rows
    );
}
