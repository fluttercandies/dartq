//! #3461 — the `unnecessary_type_check` verdict legs that read the
//! oracle's silent faces as constant-true (or vice-versa): the
//! parameterized function-RHS typedef fold and the prefixed `p.T`
//! checked-head recovery.
//!
//! Oracle contract (upstream Dart SDK source,
//! pkg/analyzer/lib/src/error/best_practices_verifier.dart:987-1041 —
//! `_checkAllTypeChecks` → `TypeSystem.isSubtypeOf(leftType,
//! rightType)`): the row fires only when the operand's resolved static
//! type is a subtype of the checked type. Three faces this file pins:
//!
//! 1. `m2 is F2<T>` over `typedef F2<T> = core.List<core.int>
//!    Function(int y, {List<Function> x})`: the `var_ty` fold snapshots
//!    the armed return head (`core.List`), never the anonymous
//!    signature, so an unfolded `Named{F2,[T]}` checked side stranded
//!    the pair as (Function, Named) and the strict ask answered false —
//!    silent where the oracle reports. The fold instantiates the
//!    file-side `index.aliases` capture at the written arguments
//!    (`instantiated_alias_sig`), the same face `function_shape_sig`
//!    reads for the declared-type asks.
//!
//! 2. `x is F2` bare: `instantiateTypeFormalsToBounds` — the bare use
//!    reads the bounds-substituted signature, not the raw head.
//!
//! 3. `e is p.T`: the listener's leaf-substitute arms `Named{T}` — the
//!    leaf resolves the element but the subtype ask against the
//!    operand's own prefixed declared face (`p.T`) needs the joined
//!    spelling back; `armed_ty_span` still anchors the prefix token,
//!    and `resolve_bare(prefix) == RefTarget::Prefix` rebuilds it.
//!
//! Probed against `dart analyze` 3.13.1 (`--sdk-path` FVM 3.47.1
//! cache): every expectation below is a live verdict, not a
//! derivation.

use super::test_rig::{RigImport, assert_silent, code_names, report_ops, report_ops_imports, rows};
use super::*;

/// The report's code names in source order (`(severity, code, file)` is
/// set-shaped; this fixture's rows land in source order anyway).
fn names(report: &FileReport) -> Vec<&'static str> {
    code_names(report)
}

/// The `p.` spelling the corpus `core.` fixtures ride: a sibling
/// library mounted BOTH unprefixed (so `canonical_head`'s same-element
/// guard collapses `p.C`→`C` — the face the corpus's `core.`=dart:core
/// carries, where `core.List` and `List` name one element) and under
/// the `p` prefix. Declaring `List`/`int` in the mounted lib cannot
/// stand: the rig's own prelude declares them in the main unit, so a
/// foreign `List` is a genuinely different element and the honest
/// answer is silence. `C`/`I` collide with no prelude head.
const P_LIB: RigImport<'static> = RigImport {
    uri: "package:p/p.dart",
    prefix: Some("p"),
    source: "class C<E> {} class I {}",
};
const P_LIB_BARE: RigImport<'static> = RigImport {
    uri: "package:p/p.dart",
    prefix: None,
    source: "class C<E> {} class I {}",
};

/// Shape (1), the dominant corpus miss: `m2 is F2<T>` inside `U2<T>` —
/// the alias-written target folds to the instantiated signature, the
/// member tear-off's own `p.`-spelled return leaf recovers through the
/// declaration's name token (the member table's armed head keeps only
/// the prefix), and the honest required count (1) decides the arity
/// gate. Probed: reports exactly the `unnecessary_type_check` row.
#[test]
fn parameterized_alias_target_over_member_tear_off() {
    let report = report_ops_imports(
        "typedef F2<T> = p.C<p.I> Function(I y, {C Function()? x});\n\
         class U2<T> {\n\
           p.C<p.I> m2(I y, {C Function()? x}) => throw 'x';\n\
           void test() { m2 is F2<T>; }\n\
         }\n",
        &[P_LIB, P_LIB_BARE],
    );
    assert_eq!(
        names(&report),
        vec!["unnecessary_type_check"],
        "{:?}",
        rows(&report)
    );
}

/// Shape (1b), the twin the fold must also answer: `m2 is F2<int>` —
/// the substituted `C<int>` return reads through the same capture.
#[test]
fn parameterized_alias_target_substitutes_written_args() {
    let report = report_ops_imports(
        "typedef F2<T> = p.C<p.I> Function(I y, {C Function()? x});\n\
         class U2<T> {\n\
           p.C<p.I> m2(I y, {C Function()? x}) => throw 'x';\n\
           void test() { m2 is F2<int>; }\n\
         }\n",
        &[P_LIB, P_LIB_BARE],
    );
    assert_eq!(
        names(&report),
        vec!["unnecessary_type_check"],
        "{:?}",
        rows(&report)
    );
}

/// Shape (1c), the written-type twin already on the books: the alias
/// fold must agree with the literal spelling, not diverge.
#[test]
fn written_signature_twin_reports_identically() {
    let report = report_ops_imports(
        "class U2<T> {\n\
           p.C<p.I> m2(I y, {C Function()? x}) => throw 'x';\n\
           void test() {\n\
             m2 is p.C<p.I> Function(I y, {C Function()? x});\n\
           }\n\
         }\n",
        &[P_LIB, P_LIB_BARE],
    );
    assert_eq!(
        names(&report),
        vec!["unnecessary_type_check"],
        "{:?}",
        rows(&report)
    );
}

/// Shape (2), the bare parameterized alias: `m2 is F2` instantiates to
/// bounds — `T`'s unbounded `Object?` default substitutes into the
/// return `p.C<p.I>`'s argument slot unchanged (the alias's own
/// parameter never occurs in the RHS), so the pair still answers
/// constant-true.
#[test]
fn bare_parameterized_alias_instantiates_to_bounds() {
    let report = report_ops_imports(
        "typedef F2<T> = p.C<p.I> Function(I y, {C Function()? x});\n\
         class U2<T> {\n\
           p.C<p.I> m2(I y, {C Function()? x}) => throw 'x';\n\
           void test() { m2 is F2; }\n\
         }\n",
        &[P_LIB, P_LIB_BARE],
    );
    assert_eq!(
        names(&report),
        vec!["unnecessary_type_check"],
        "{:?}",
        rows(&report)
    );
}

/// Shape (3), the prefixed checked head: `x is async.A<int>` over an
/// `async.A<int>`-declared operand — the joined `async.A` head reads
/// the same element the operand's declared face spells, where the
/// leaf-only `Named{A}` answered a different head and stayed silent.
/// The `async` library mounts unprefixed too (the same-element face).
/// Probed: reports the row.
#[test]
fn prefixed_checked_head_matches_prefixed_operand() {
    let report = report_ops_imports(
        "void f(async.A<int> x) { x is async.A<int>; }\n",
        &[
            RigImport {
                uri: "package:async/async.dart",
                prefix: Some("async"),
                source: "class int {} class A<T> {}",
            },
            RigImport {
                uri: "package:async/async.dart",
                prefix: None,
                source: "class int {} class A<T> {}",
            },
        ],
    );
    assert_eq!(
        names(&report),
        vec!["unnecessary_type_check"],
        "{:?}",
        rows(&report)
    );
}

/// Shape (3b), the wrong-prefix silence the joined head preserves:
/// `await.B` and `async.B` name different declarations — the oracle's
/// subtype ask answers false and stays silent, which the leaf-collapse
/// (`B` vs `B`) would have over-reported.
#[test]
fn mismatched_prefix_heads_stay_silent() {
    let report = report_ops_imports(
        "void f(async.B<int> x) { x is await.B; }\n",
        &[
            RigImport {
                uri: "package:async/async.dart",
                prefix: Some("async"),
                source: "class int {} class B<T> {}",
            },
            RigImport {
                uri: "package:await/await.dart",
                prefix: Some("await"),
                source: "class B<T> {}",
            },
        ],
    );
    assert_silent(&report);
}

/// The genuinely unnecessary check: a provable subtype pair on plain
/// class heads — `int <: num` over the prelude.
#[test]
fn subtype_pair_reports_constant_true() {
    let report = report_ops("void f(int x) { x is num; }\n");
    assert_eq!(
        names(&report),
        vec!["unnecessary_type_check"],
        "{:?}",
        rows(&report)
    );
}

/// A check the oracle keeps silent on: `num is int` — `num` is not a
/// subtype of `int`, the pair is informative.
#[test]
fn non_subtype_pair_stays_silent() {
    let report = report_ops("void f(num x) { x is int; }\n");
    assert_silent(&report);
}

/// The check inside a conditional rides the same verdict: the `if`
/// arm's flow promotion is upstream's business; the constant-true row
/// still fires at the check site.
#[test]
fn conditional_check_reports() {
    let report = report_ops("void f(int x) { if (x is num) { x + 1; } }\n");
    assert_eq!(
        names(&report),
        vec!["unnecessary_type_check"],
        "{:?}",
        rows(&report)
    );
}

/// A promoted receiver: `x` narrows to `int` before the check — the
/// verdict reads the promoted face (`int <: num` still constant-true).
#[test]
fn promoted_receiver_reports() {
    let report = report_ops("void f(num x) { if (x is int) { x is num; } }\n");
    assert_eq!(
        names(&report),
        vec!["unnecessary_type_check"],
        "{:?}",
        rows(&report)
    );
}

/// The `dynamic` operand: `dynamic <: T` is false for every concrete
/// `T` — the oracle stays silent, never the constant-true face.
#[test]
fn dynamic_operand_stays_silent() {
    let report = report_ops("void f(dynamic x) { x is num; }\n");
    assert_silent(&report);
}

/// A type parameter with a bound: `T extends num` over `x is num` —
/// the bound's `num <: num` proves the pair constant-true.
#[test]
fn bounded_type_parameter_reports() {
    let report = report_ops("void f<T extends num>(T x) { x is num; }\n");
    assert_eq!(
        names(&report),
        vec!["unnecessary_type_check"],
        "{:?}",
        rows(&report)
    );
}

/// A nullable-union case: `int? <: num?` — the nullable projections
/// still answer the strict subtype question constant-true.
#[test]
fn nullable_union_reports() {
    let report = report_ops("void f(int? x) { x is num?; }\n");
    assert_eq!(
        names(&report),
        vec!["unnecessary_type_check"],
        "{:?}",
        rows(&report)
    );
}
