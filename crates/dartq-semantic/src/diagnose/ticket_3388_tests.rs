//! The #3388 pins: the `creation_args.rs` half of instance-creation
//! downward inference (recon bucket C).
//!
//! Bucket C of the `invalid_assignment` / `argument_type_not_assignable`
//! recon splits the instance-creation faces between two write files.
//! `diagnose/creation_args.rs` owns the created row's TYPE reader
//! ([`Engine::creation_row_ty`], [`Engine::with_explicit_creation_args`],
//! [`Engine::instantiated_head_ty`]); the context discovery and the
//! parameter solve live in `diagnose/invocation.rs`. These pins hold the
//! reader's contract so a solver change cannot silently regress it, and
//! each names the residual face the owning lane still owes.
//!
//! The reader's precedence, all three pinned below — upstream writes the
//! constructor's instantiated return type as the creation node's static
//! type (`visitInstanceCreationExpression`):
//!
//! 1. the written `<…>` group (`C<int>(x)`) — explicit arguments outrank
//!    inference (`creation_row_ty`'s latch and byte-walk arms);
//! 2. the send's solved instantiation, published at the head's offset
//!    (`publish_creation_instantiation`, invocation.rs — the reader peeks
//!    the same slot, peek-not-consume);
//! 3. the instantiate-to-bounds fill for a raw generic head (`C()` over
//!    `C<S extends num>` reads `C<num>`), and nothing at all for a
//!    non-generic head.
//!
//! Residual, recorded not fixed here — all three root in
//! `diagnose/invocation.rs`, not in this file:
//!
//! - the created type's CONTEXT seed: `creation_call_site_ty` poisons a
//!   context-seeded parameter slot when an argument disagrees, so
//!   `A<int, String> x = new A('s', 1)` publishes the to-bounds
//!   `A<dynamic, dynamic>` where upstream pins the context solution
//!   before any constraint is consulted
//!   (pkg/_fe_analyzer_shared/lib/src/type_inference/type_analyzer_operations.dart
//!   `inferTypeParameterFromAll`: a covariant parameter returns its
//!   previously — downward — inferred type, and the argument checks
//!   report the conflict separately);
//! - the argument check's own context seed is the same send close's
//!   declared/write arms, and it only seeds a slot left unsolved — a
//!   resolved seed must instead outrank the upward candidates;
//! - an argument whose own row resolves later than the check stays
//!   under-reported: `Holder<A> h = Holder(B())` reports nothing while
//!   `Holder<int> h = Holder('s')` reports (both measured with this
//!   module's fixtures). Whether the cause is the nested creation's
//!   still-`Unknown` row at the comparison or the seed not reaching a
//!   user-class formal is not yet separated.
//!
//! Offsets are UTF-16; `at` shifts by [`CORE_PRELUDE`] because the rig
//! carries the fixture behind it on one line. All fixtures are ASCII, so
//! the byte and UTF-16 offsets coincide.

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

/// Face 1 — the written group is the created row's static type.
///
/// `var p = new Pair<int, String>(1, 'a')` binds the written
/// instantiation (both arguments agree with it, so no argument row), and
/// the downstream assignment displays that exact face. Residual named
/// above: the same declaration written with a CONTEXT that disagrees
/// (`A<int, String> x = new A('s', 1)`) is the solver's face, not this
/// reader's.
#[test]
fn written_group_publishes_the_created_row_type() {
    let source = r#"
class Pair<K, V> {
  K k = throw 0;
  V v = throw 0;
  Pair(this.k, this.v);
}
void f() {
  var p = new Pair<int, String>(1, 'a');
  Pair<String, String> q = p;
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
    assert_eq!(*offset, at(source, "p;"), "{message}");
    assert_eq!(*length, 1, "{message}");
    assert!(
        message.contains("'Pair<int, String>'") && message.contains("'Pair<String, String>'"),
        "{message}"
    );
}

/// Face 2 — a written group substitutes the constructor formals, so the
/// argument checks against the written parameter while the created type
/// stays the written instantiation. The upstream shape: `Box<B>` over a
/// `Box<A>` declaration reports the assignment only (`B()` satisfies the
/// written `T`), never an argument row.
#[test]
fn written_group_substitutes_the_creation_formals() {
    let source = r#"
class A {}
class B {}
class Box<T> {
  T t = throw 0;
  Box(this.t);
}
void f() {
  Box<A> b = new Box<B>(B());
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
    let message = &rows[0].2;
    assert!(
        message.contains("'Box<B>'") && message.contains("'Box<A>'"),
        "{message}"
    );
}

/// Face 3 — the declared local context seeds a single-parameter
/// creation's formal, so the argument checks against the CONTEXT type and
/// the conflict reports at the argument. This is the working neighbour of
/// the residual multi-parameter face (`A<int, String> x = new A('s', 1)`),
/// whose solver-side pin rule is the owning lane's work.
///
/// The argument is a literal on purpose: a NESTED creation argument
/// (`Holder<A> h = Holder(B())`) resolves no row at the argument check's
/// pass and is suppressed today — measured with this same fixture, see
/// the residual list in the module docs.
#[test]
fn context_seed_reports_the_argument() {
    let source = r#"
class Holder<T> {
  Holder(T t);
}
void f() {
  Holder<int> h = Holder('s');
}
"#;
    let report = report_ops(source);
    let rows = rows_of(&report, "argument_type_not_assignable");
    assert_eq!(rows.len(), 1, "{rows:#?}");
    let (offset, _, message) = &rows[0];
    assert_eq!(*offset, at(source, "'s'"), "{message}");
    assert!(
        message.contains("'String'") && message.contains("'int'"),
        "{message}"
    );
}

/// Face 4 — the agreeing context is silent on both faces: the seeded
/// formal accepts the argument and the created type needs no
/// `invalid_assignment`. The negative control of face 3.
#[test]
fn context_seed_agrees_stays_silent() {
    let source = r#"
class A {}
class Holder<T> {
  Holder(T t);
}
void f() {
  Holder<A> h = Holder(A());
}
"#;
    let report = report_ops(source);
    assert!(
        rows_of(&report, "argument_type_not_assignable").is_empty(),
        "{:#?}",
        report.diagnostics
    );
    assert!(
        rows_of(&report, "invalid_assignment").is_empty(),
        "{:#?}",
        report.diagnostics
    );
}

/// Face 5 — a non-generic head gains no invented arguments: the reader's
/// to-bounds arm must stay inert where no parameter exists (`var x =
/// C()` over `class C` binds `C`, never `C<dynamic>`), so the following
/// `C` assignment is clean.
#[test]
fn non_generic_head_gains_no_arguments() {
    let source = r#"
class C {
  C();
}
void f() {
  var x = C();
  C c = x;
}
"#;
    let report = report_ops(source);
    assert!(
        rows_of(&report, "argument_type_not_assignable").is_empty(),
        "{:#?}",
        report.diagnostics
    );
    assert!(
        rows_of(&report, "invalid_assignment").is_empty(),
        "{:#?}",
        report.diagnostics
    );
}
