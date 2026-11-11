//! #3911 pins — the top-merge consumer chain's discriminated roots.
//!
//! The discriminator cycle (env-gated probes at `merged_member_entry` /
//! `merge_candidates` / `instantiate_inherited` / `scan_one_header`) split
//! the R1 residual into three arms, all in the merge's SUPPORTING faces:
//!
//! 1. `scan_type_args` closed a nested-generic argument group against the
//!    bare `>` lexeme only — the scanner never splits shift tokens
//!    (`>>`/`>>>` are one token), so any clause argument written with a
//!    nested generic (`A<FutureOr<dynamic>>`) marked the whole header
//!    unfaithful: no header facts, the substitution environment
//!    `Undecidable`, the merge never ran, and the consumer kept the walk's
//!    first candidate. The group closers now unwind merged tokens one
//!    level each.
//! 2. `override_decide`'s `Left Top` arm answered an unconditional `false`
//!    for a `dynamic`/`void` source against a non-top-spelled target —
//!    upstream (`subtype.dart:63-68`) spells a recursion: `dynamic <: T1`
//!    iff `Object? <: T1`. `FutureOr<top>` targets are top-adjacent
//!    through that leg (`type_system.dart:1312-1317`
//!    `isTop(FutureOr<T>) = isTop(T)`), so the merge's validity filter
//!    dropped the `dynamic member()` candidate the oracle keeps. The arm
//!    now recurses through `Object?`.
//! 3. Function-tailed clause arguments (`FutureOr<Object?> Function()`)
//!    keep the unmodelable posture for everything but a parameterless
//!    tail — the deep faces' shapes (`DND0`-`DND5`).
//!
//! The consumer (`invocation.rs` `resolved_member` and the send path) is
//! ACQUITTED: with the three arms fixed, the merged answer reaches `x`
//! unaided.

use super::FileReport;
use super::test_rig::{code_names, report_ops, report_ops_experiments_with, rows};
use dartq_language::Feature;

/// The `argument_type_not_assignable` row's message, if the code fired.
fn argument_message(report: &FileReport) -> Option<String> {
    report
        .diagnostics
        .iter()
        .find(|d| d.code.name == "argument_type_not_assignable")
        .map(|d| d.message.to_string())
}

/// `Left Top` through the override face: `dynamic m()` is a VALID override
/// of `FutureOr<Object?> m()` (`dynamic <: FutureOr<Object?>` holds via
/// `Object? <: FutureOr<Object?>`, the FutureOr disjunction's
/// `T0 <: S1` leg against a top target) — and the reverse direction holds
/// through `Right Top` (`FutureOr<Object?> <: dynamic`, `dynamic` is top).
/// Neither pair may report `invalid_override`. Before the fix the forward
/// direction answered an unconditional `false` for every non-top-spelled
/// target.
#[test]
fn dynamic_and_futureor_member_overrides_stay_silent() {
    let report = report_ops(
        r#"class A {
  FutureOr<Object?> m() => throw "unreachable";
}
class B extends A {
  dynamic m() => throw "unreachable";
}
class C extends A {
  FutureOr<Object?> m() => throw "unreachable";
}
"#,
    );
    let names = code_names(&report);
    assert!(
        !names.contains(&"invalid_override"),
        "expected both override directions to stay silent, got {names:?} ({:?})",
        rows(&report)
    );
}

/// The deep face (`DND3` shape, the census's function-tail columns): the
/// candidates' function columns normalize field-wise
/// (`NORM(FutureOr<void> Function()) = void Function()`), the merge lands
/// `Object? Function()` (`void ⊔ Object?`), and the call's result is
/// `Object?` — `x.foo` reports the nullable receiver and the argument row
/// names `Object?`, never `FutureOr`. Before the scan captured
/// parameterless function tails, the header could not spell this clause at
/// all and the merge never ran.
///
/// ROUTED RESIDUAL (#3927, invocation.rs send path — the recon's
/// one-application-short face): the merge answer DOES land — the sibling
/// pin `members_direct_futureor_function_deep_merged_result_reports`
/// fires `unchecked_use_of_nullable_value` on `x.foo`, proving `x` reads
/// the merged function column's return — but the TRAILING call
/// application (`self.member()()`) types the result one application short
/// of the oracle (`x` keeps a nullable face whose `takesNum` argument
/// check suppresses, and the DND3 spelling loses the nullable face to an
/// `undefined_getter`). The application depth of a merged function-typed
/// member result in the send path is the #3927 packet's arm (a), not the
/// merge's.
#[test]
// un-ignored 2026-11-07: passing per stale-ignore triage (ticket #3973)
fn deep_futureor_void_function_tail_crosses_to_object_question() {
    let report = report_ops(
        r#"class A<T> {
  T member() => throw "unreachable";
}
class B<T> {
  T member() => throw "unreachable";
}
void takesNum(num x) {}

class DND3 extends A<FutureOr<void> Function()> implements B<Object? Function()> {
  void test() {
    var self = this;
    var x = self.member()();
    x.foo;
    x.toString;
    takesNum(x);
  }
}
"#,
    );
    let names = code_names(&report);
    assert!(
        names.contains(&"unchecked_use_of_nullable_value"),
        "expected unchecked_use_of_nullable_value on x.foo (x: Object?), got {names:?}"
    );
    let msg = argument_message(&report);
    assert!(
        msg.as_deref().is_some_and(|m| m.contains("'Object?'")),
        "expected argument_type_not_assignable naming 'Object?', got {msg:?} ({names:?})"
    );
    assert!(
        !msg.as_deref().is_some_and(|m| m.contains("FutureOr")),
        "the argument row still names an un-normalized FutureOr candidate, got {msg:?}"
    );
}

/// The M11 declared-variance consumer, INVARIANT head (oracle-verified
/// matrix, pinned 3.13.7): under `inout T` neither direction widens — both
/// return pairs report `return_of_invalid_type` (`h`'s `Inv<C>` over
/// `Inv<C?>` and `k`'s `Inv<C?>` over `Inv<C>`).
///
/// The shape carries no inference parameters, so the return check rides the
/// SUBTYPE face, not the same-head `infer_gather` arm this ticket wired
/// (that arm is the inference-driven face, landed per the #3903 spec). The
/// subtype face's per-parameter relation reads the model's declared-variance
/// column (`Container::type_param_variances`) — #3942 wired it at
/// `covariant_args`, and both directions now decide invariant. Before that
/// wiring `k` fired through the covariant fallback (`C? <: C` fails) while
/// `h` stayed silent (`C <: C?` passes covariantly).
///
/// The parser's variance-parse face additionally mis-spells `int` inside
/// these fixtures' type arguments (`non_type_as_type_argument`) — parser
/// owner (AM).
#[test]
fn invariant_head_rejects_both_widening_directions() {
    let report = report_ops_experiments_with(
        r#"class C {}

class Inv<inout T> {
  T? value;
  Inv(this.value);
}
Inv<C?> h(Inv<C> a) => a;
Inv<C> k(Inv<C?> a) => a;
"#,
        &[],
        None,
        &[Feature::Variance],
    );
    let names = code_names(&report);
    let invalid_rows = names
        .iter()
        .filter(|name| **name == "return_of_invalid_type")
        .count();
    assert!(
        invalid_rows == 2,
        "expected return_of_invalid_type on both invariant directions, got {names:?} ({:?})",
        rows(&report)
    );
}

/// The M11 declared-variance consumer, CONTRAVARIANT head
/// (oracle-verified matrix): under `in T` the head accepts the pair the
/// covariant reading rejects and rejects the pair it accepts — `g`
/// (return `Contra<C?>` over `Contra<C>`: the reversed leg `C <: C?`
/// fails) reports and `f` (return `Contra<C>` over `Contra<C?>`: `C? <:
/// C` fails forward but the reversal makes it the accepted pair) stays
/// silent.
///
/// The static return pairs route through the subtype face, whose
/// per-parameter relation reads the model's declared-variance column —
/// #3942 wired the reversed leg at `covariant_args`, so `g` reports and
/// `f` stays silent exactly as the oracle's matrix says. The REVERSED-pair
/// gather this ticket wired (`infer_gather`'s `Contravariant` arm) is the
/// inference-driven face and is landed per the #3903 spec.
#[test]
fn contravariant_head_reverses_the_accepted_pair() {
    let report = report_ops_experiments_with(
        r#"class C {}

class Contra<in T> {
  T? value;
  Contra(this.value);
}
Contra<C?> g(Contra<C> a) => a;
Contra<C> f(Contra<C?> a) => a;
"#,
        &[],
        None,
        &[Feature::Variance],
    );
    let names = code_names(&report);
    let invalid_rows = names
        .iter()
        .filter(|name| **name == "return_of_invalid_type")
        .count();
    assert!(
        invalid_rows == 1,
        "expected exactly one invalid return under the contravariant head, got {names:?} ({:?})",
        rows(&report)
    );
    // The surviving direction is `f`'s: the declared return widens through
    // the reversed leg (`C? <: C` under `in T` maps the pair the other
    // way), `g`'s narrows (`C <: C?` fails reversed) and is the row the
    // oracle names.
    let stated = rows(&report).join("\n");
    assert!(
        stated.contains("'g'"),
        "expected the invalid return anchored at g, got {stated}"
    );
}
