//! Ticket #3879 pins: the creation-argument inference packet's phase-1
//! reader contracts, plus the routed-residual record for the four census
//! classes this ticket carried (gated census `census-20261104-092425`).
//!
//! The packet's premise — "the creation-argument inference site lives in
//! `creation_args.rs`" — does not hold: this file's own header and the
//! #3388 pins record that every face that DECIDES a creation's
//! instantiation lives in `invocation.rs` (the context discovery
//! `creation_context_ty`, the parameter solve `creation_call_site_ty`,
//! the publish `publish_creation_instantiation`, and both per-argument
//! checkers `check_explicit_creation_arguments` /
//! `check_named_creation_arguments` / `check_enum_const_arguments`),
//! with the tear-off value in `engine_core.rs` (`tearoff_ty`) and the
//! var-bind read in `listener.rs`. All of those files are sibling
//! lanes' exclusive scope this round, so the four items are ROUTED with
//! the divergence named and evidenced below, and the pins here hold the
//! half the packet owns: the `creation_args.rs` reader contracts the
//! routed fixes must not regress, each verified against the live
//! 3.13.7 oracle (`dart analyze --format=machine`) on the exact
//! fixtures.
//!
//! # The four routed divergences (zero silent drops)
//!
//! **M3, instance-creation half — 54 missing / 14 extra rows recomputed
//! from the census over the three representative families**
//! (`test_downwardsInferenceOnInstanceCreations` 21 missing + 2 extra,
//! `tearing_off_from_typedef_*` 13 missing, `test_futureUnion` +
//! `test_mapLiterals*` 20 missing + 12 extra):
//!
//! * The SWAP seed: `A<int, String> a0 = new B(3, "hello")` over
//!   `class B<S, T> extends A<T, S>` must solve `B`'s parameters by
//!   unifying the CONTEXT through the extends clause (`A<T, S>` against
//!   `A<int, String>` gives `T := int`, `S := String` — the arguments
//!   swap), never positionally. `creation_context_seed`
//!   (invocation.rs) returns `None` whenever the context's head differs
//!   from the created class, and `creation_call_site_ty` then pairs the
//!   upward candidates positionally (`B<int, String>`), so both
//!   argument rows go unreported. Upstream: the downward
//!   `InferenceContext` binds through the instantiated supertype
//!   (upstream Dart SDK source,
//!   pkg/analyzer/lib/src/dart/resolver/invocation_inferrer.dart:180
//!   `FullInvocationInferrer` — the context type feeds the solve ahead
//!   of the argument constraints, `chooseFinalTypes` at :345 and the
//!   `resolveArgumentsToParameters` pairing at :354, defined at
//!   pkg/analyzer/lib/src/generated/resolver.dart:4877). Owner: the
//!   invocation.rs lane.
//! * The same case's eight `invalid_assignment` rows (`new
//!   A<int, dynamic>(3, "hello")` under an `A<int, String>`
//!   declaration) and the var-bind twin (`var w = new A<int,
//!   dynamic>(...)` binds nothing usable — a later `int z = w` reports
//!   nothing while the oracle reports `A<int, dynamic>`): the created
//!   row's static type on the BIND path is lost when the written group
//!   carries a `dynamic` segment, although the same group reads
//!   correctly through the argument-check path (pinned below). Owner:
//!   the listener.rs/engine_core.rs init-read seam, not this reader.
//! * The tear-off family: `var v = List<int>.filled; v(3, "s")` — the
//!   direct spelling also under-reports, so the root is the tear-off
//!   value's signature at the send (`engine_core.rs` `tearoff_ty` and
//!   the send-close formal pairing), not the typedef fold.
//! * `mapLiterals`/`futureUnion`: collection-literal context inference
//!   and `FutureOr` union faces — collection_literal.rs / the send
//!   close, outside this packet's files.
//!
//! **M4, instantiate-to-bounds fallback — the constructor2_test_05
//! row** (`const C.subtyped(a)` over `const C.subtyped(B<U> this.a)`:
//! the argument's `A<int>` constrains nothing, the oracle falls back to
//! `U := dynamic` and still checks, reporting `'A<int>'` against
//! `'B<dynamic>'` — the algorithm is spelled in that test file's own
//! comments). Our `check_named_creation_arguments` instantiation chain
//! (`creation_call_site_ty` → `creation_row_ty` →
//! `written_creation_args`) answers `None` when the solve fails and the
//! chain then checks against the RAW formals, which is undecided —
//! silent. The fallback arm belongs at that chain's end: when the class
//! is generic and no source answered, take
//! [`Engine::instantiated_head_ty`]'s defaults (already built on the
//! #3871 i2b engine's `Bounds::itb_default_args`). Owner: the
//! invocation.rs lane; the engine to reuse is landed and read-only.
//!
//! **E8, enum-formal Never — 8 extra rows** (grammar_A12_t01: an enum
//! constructor's SUPER formals (`const E1(super.x)`) park `Ty::Never`,
//! and `check_enum_const_arguments` plus its const twin then report
//! `argument_type_not_assignable` and
//! `const_constructor_param_type_mismatch` against `'Never'` where the
//! oracle — having already reported the super-formal face itself —
//! stays silent on the arguments). Owner: the invocation.rs lane (the
//! suppression is the argument walk's, keyed on the already-reported
//! super formal; `enum_verifier.rs` owns no argument face).
//!
//! **M11, variance/Super-mixins — 18 missing rows recomputed from the
//! census**: 13 post-failed-inference argument rows (the variance
//! suites' `Contravariant`/`Invariant` families — the same
//! fall-back-and-still-check face as M4, plus a variance-aware
//! candidate the oracle keeps from the covariant parameter), 4
//! variance-aware subtyping rows (`List<Contravariant<Lower>>` must NOT
//! be assignable to `Iterable<Contravariant<Middle>>` — the subtyping
//! engine ignores declared variance, owner: the assignability.rs
//! lane), and 1 super-forwarding row (`super.foo(t1)` over `mixin
//! M<T1, T2> on B<T2>` — the raw `T2` formal is lane #3873's E1
//! substitution face).
//!
//! Offsets are UTF-16; `at` shifts by [`CORE_PRELUDE`] because the rig
//! carries the fixture behind it on one line. All fixtures are ASCII,
//! so byte and UTF-16 offsets coincide.

use super::test_rig::{CORE_PRELUDE, report_ops};
use super::*;

/// Every `code` row of `report` as `(offset, length, message)`.
fn rows_of(report: &FileReport, code: &str) -> Vec<(u32, u32, String)> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == code)
        .map(|d| (d.range.offset, d.range.length, d.message.to_string()))
        .collect()
}

/// Byte offset of `needle` in the fixture, shifted past the core prelude.
fn at(source: &str, needle: &str) -> u32 {
    (CORE_PRELUDE.len() + source.find(needle).expect("needle in fixture")) as u32
}

/// The written group substitutes the constructor formals across every
/// spelling — keyworded unnamed, named selector, and `const` — including
/// a `dynamic` segment (`new A<int, dynamic>.named("hello", 3)` checks
/// the first argument against the WRITTEN `int`, never the raw `S`).
/// Oracle-verified 3.13.7: one `argument_type_not_assignable` row per
/// call site, `'String'` against `'int'`, and nothing else.
#[test]
fn written_group_with_dynamic_segment_substitutes_formals() {
    let source = r#"
class A<S, T> {
  S x = throw 0;
  T y = throw 0;
  A(this.x, this.y);
  A.named(this.x, this.y);
}
void f() {
  var w = new A<int, dynamic>("hello", 3);
  var v = new A<int, dynamic>.named("hello", 3);
  var k = const A<int, dynamic>.named("hello", 3);
}
"#;
    let report = report_ops(source);
    let rows = rows_of(&report, "argument_type_not_assignable");
    assert_eq!(rows.len(), 3, "{:#?}", report.diagnostics);
    for (_offset, length, message) in &rows {
        assert_eq!(*length, 7, "{message}");
        assert!(
            message.contains("argument type 'String'") && message.contains("parameter type 'int'"),
            "{message}"
        );
    }
    // One row per call site, each at its own `"hello"` argument (the
    // rows arrive offset-sorted from the fold).
    assert_eq!(
        rows[0].0,
        at(source, "dynamic>(\"hello\"") + "dynamic>(".len() as u32,
        "{:#?}",
        rows
    );
    assert_eq!(
        rows[1].0,
        at(source, ".named(\"hello\", 3);\n  var k") + ".named(".len() as u32,
        "{:#?}",
        rows
    );
    assert_eq!(
        rows[2].0,
        at(source, ".named(\"hello\", 3);\n}") + ".named(".len() as u32,
        "{:#?}",
        rows
    );
}

/// The written group is the created row's static type on the var-bind
/// path (`var u = new A<String, int>("hello", 3)` binds
/// `A<String, int>`, so the disagreeing declaration reports the written
/// instantiation). Oracle-verified 3.13.7: one `invalid_assignment`
/// row, `'A<String, int>'` against `'A<int, String>'`.
///
/// Routed residual recorded above: the same fixture with a `dynamic`
/// segment in the group (`new A<int, dynamic>`) currently binds nothing
/// usable on this path while the argument-check path (the pin above)
/// reads the same group — the listener.rs/engine_core.rs init-read
/// seam owns that half.
#[test]
fn written_group_binds_the_created_row_type() {
    let source = r#"
class A<S, T> {
  S x = throw 0;
  T y = throw 0;
  A(this.x, this.y);
}
void f() {
  var u = new A<String, int>("hello", 3);
  A<int, String> q = u;
}
"#;
    let report = report_ops(source);
    assert!(
        rows_of(&report, "argument_type_not_assignable").is_empty(),
        "{:#?}",
        report.diagnostics
    );
    let rows = rows_of(&report, "invalid_assignment");
    assert_eq!(rows.len(), 1, "{rows:#?}");
    let (offset, length, message) = &rows[0];
    assert_eq!(*offset, at(source, "u;"), "{message}");
    assert_eq!(*length, 1, "{message}");
    assert!(
        message.contains("'A<String, int>'") && message.contains("'A<int, String>'"),
        "{message}"
    );
}
