//! #3110 — `invalid_assignment` + `argument_type_not_assignable` pins.
//!
//! Two arms land in `assignability.rs`; the other three are pinned
//! `#[ignore]`d against their upstream roots, which live in lanes this
//! ticket does not own:
//!
//! * **Landed — the instantiated member write face** (the AT2+IA4 arm):
//!   `assignment_target_ty`'s `Receiver::Typed`/`Receiver::Created` arms
//!   answer the member's declared face with the receiver's written
//!   arguments substituted — `C<String>`'s `T? field` writes against
//!   `String?`, the oracle's `correspondingParameter.type` (upstream Dart
//!   SDK source, pkg/analyzer/lib/src/generated/
//!   error_detection_helpers.dart:72-75).
//! * **Blocked — AT1 (untyped formal is `dynamic`)**: the capture's
//!   `fn_capture_formal` poisons an untyped formal instead of pushing
//!   `dynamicType` (upstream Dart SDK source,
//!   pkg/analyzer/lib/src/dart/resolver/resolution_visitor.dart:808-813) —
//!   `diagnose/type_capture.rs`, an in-flight lane (#3099).
//! * **Blocked — IA1 (instantiate-to-bound seeds the declared bound)**:
//!   the `toF(source)` send's instantiated return `F<G>` is clobbered to
//!   `Void` by the function-typed-callee arm of the send-close
//!   (`invocation.rs`, in-flight), and `dartq-types`' `replace_top_bottom`
//!   is missing the alias-param-variance arm that turns the bound seed
//!   into `Object?` (upstream
//!   replace_top_bottom_visitor.dart:78). `fsource` binds `Void`, so the
//!   `F<…>` pair is never asked — `assignable(Void, _)` answers `None`,
//!   not the written-type gate and not a pair-equal decision.
//! * **Blocked — AT2 (the verdict's formal is the instantiated one)**:
//!   the send solve keeps the declared `T` where the oracle substitutes
//!   the call's type arguments (`enum E<T> { a<int>("1") }` — `T = int`),
//!   and the explicit-argument bound clamp fires where the oracle clamps
//!   only the inferred solution — `invocation.rs`, in-flight.

use super::test_rig::{CORE_PRELUDE, report_ops};
use super::*;

fn rows_named<'d>(report: &'d FileReport, name: &str) -> Vec<&'d Diagnostic> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == name)
        .collect()
}

// —— Landed: the member write face instantiates the receiver's arguments ——

/// A generic container's member write instantiates the receiver's
/// arguments: `C<String>` writes `field`/`v` against `String?`/`String`,
/// and the message renders the instantiated face — never `T?`/`T`
/// (upstream Dart SDK source, pkg/analyzer/lib/src/generated/
/// error_detection_helpers.dart:72-75 — the verdict asks
/// `correspondingParameter.type`, the already-instantiated formal).
#[test]
fn ia4_member_write_uses_instantiated_formal() {
    let fixture = "\
class C<T> {
  T? field;
  void set v(T t) {}
}
void main() {
  C<String> c = C<String>();
  c.field = 3;
  c.v = 3;
}
";
    let report = report_ops(fixture);
    let rows = rows_named(&report, "invalid_assignment");
    assert_eq!(
        rows.len(),
        2,
        "both member writes report: {:?}",
        report
            .diagnostics
            .iter()
            .map(|d| (d.code.name, d.range))
            .collect::<Vec<_>>()
    );
    let rendered: Vec<String> = rows.iter().map(|d| d.message.to_string()).collect();
    assert!(
        rendered.iter().all(|m| m.contains("String")),
        "the message renders the instantiated formal (String), not T: {rendered:?}"
    );
}

/// The generic-alias field write: `IntBox` is `Box<int>`, so `b.item`
/// writes against `int?` — the alias's own substitution composes with
/// the receiver env.
#[test]
fn ia4_alias_field_write_uses_instantiated_formal() {
    let fixture = "\
class Box<T> { T? item; }
typedef IntBox = Box<int>;
void main() {
  IntBox b = IntBox();
  b.item = \"s\";
}
";
    let report = report_ops(fixture);
    let rows = rows_named(&report, "invalid_assignment");
    assert_eq!(
        rows.len(),
        1,
        "the alias field write reports: {:?}",
        report
            .diagnostics
            .iter()
            .map(|d| (d.code.name, d.range))
            .collect::<Vec<_>>()
    );
    assert!(
        rows[0].message.contains("int?"),
        "the message renders the alias-instantiated formal (int?), not T?: {:?}",
        rows[0].message
    );
}

/// A folded creation receiver answers the setter's value formal
/// (`C().x = "s"` — the write resolves against `x=`'s `int`, upstream
/// `getWriteType`): the `Receiver::Created` arm of
/// `assignment_target_ty` keeps the declared write face the fold used to
/// starve.
#[test]
fn ia6_creation_receiver_setter_write_reports() {
    let fixture = "\
class C { set x(int v) {} }
void main() {
  C().x = \"s\";
}
";
    let report = report_ops(fixture);
    let rows = rows_named(&report, "invalid_assignment");
    assert_eq!(
        rows.len(),
        1,
        "the setter write on a creation receiver reports: {:?}",
        report
            .diagnostics
            .iter()
            .map(|d| (d.code.name, d.range))
            .collect::<Vec<_>>()
    );
    assert!(
        rows[0].message.contains("int"),
        "the write face is the setter's declared int: {:?}",
        rows[0].message
    );
}

/// The typed-receiver arm stays exact on an unparameterized write: no
/// substitution composes, so the declared face stands.
#[test]
fn ia4_unparameterized_member_write_unchanged() {
    let fixture = "\
class C { int field = 0; }
void main() {
  C c = C();
  c.field = \"s\";
}
";
    let report = report_ops(fixture);
    let rows = rows_named(&report, "invalid_assignment");
    assert_eq!(
        rows.len(),
        1,
        "the plain field write still reports once: {:?}",
        report
            .diagnostics
            .iter()
            .map(|d| (d.code.name, d.range))
            .collect::<Vec<_>>()
    );
    assert!(
        rows[0].message.contains("int"),
        "the declared face renders: {:?}",
        rows[0].message
    );
}

// —— Blocked arms: the pins below cite the upstream rule and the owning ——
// —— lane; they stay `#[ignore]`d until the root lands.                ——

/// AT1 — an untyped formal is `dynamicType` upstream
/// (pkg/analyzer/lib/src/dart/resolver/resolution_visitor.dart:808-813):
/// `void f(x,)` resolves `void Function(dynamic)`, so the `topx`
/// tear-off assigns cleanly. dartq's `fn_capture_formal`
/// (diagnose/type_capture.rs, in-flight #3099) poisons the untyped
/// formal instead — the captured shape drops it and the arity compare
/// sees a phantom extra required positional.
#[test]
// un-ignored 2026-11-07: passing per stale-ignore triage (ticket #3973)
fn at1_untyped_required_formal_stays_silent() {
    let fixture = "\
void topx(x,) {}
void argfx(void f(x,)) {}
void main() { argfx(topx); }
";
    let report = report_ops(fixture);
    assert!(
        rows_named(&report, "argument_type_not_assignable").is_empty(),
        "dartq must not report ATNA for an untyped-formal pair: {:?}",
        report
            .diagnostics
            .iter()
            .map(|d| d.code.name)
            .collect::<Vec<_>>()
    );
}

/// The optional-positional twin: `void f([y,])` resolves
/// `void Function([dynamic])`.
#[test]
// un-ignored 2026-11-07: passing per stale-ignore triage (ticket #3973)
fn at1_untyped_optional_formal_stays_silent() {
    let fixture = "\
void topy([y,]) {}
void argfy(void f([y,])) {}
void main() { argfy(topy); }
";
    let report = report_ops(fixture);
    assert!(
        rows_named(&report, "argument_type_not_assignable").is_empty(),
        "dartq must not report ATNA for an untyped-optional pair: {:?}",
        report
            .diagnostics
            .iter()
            .map(|d| d.code.name)
            .collect::<Vec<_>>()
    );
}

/// A genuine arity mismatch still reports — a `void f(int)` tear-off
/// never satisfies `void Function()`.
#[test]
fn at1_real_arity_mismatch_still_reports() {
    let fixture = "\
void topy(int y) {}
void argfn(void f()) {}
void main() { argfn(topy); }
";
    let report = report_ops(fixture);
    let rows = rows_named(&report, "argument_type_not_assignable");
    assert_eq!(
        rows.len(),
        1,
        "a real arity refusal still reports: {:?}",
        report
            .diagnostics
            .iter()
            .map(|d| d.code.name)
            .collect::<Vec<_>>()
    );
}

/// IA1 — instantiate-to-bound seeds the declared bound else `Object?`
/// (pkg/analyzer/lib/src/dart/element/generic_inferrer.dart:449), never
/// `Never`: the mutual-bound typedef pair resolves `fsource` to a real
/// `F<…>` face and both `invalid_assignment` rows fire at the RHS reads.
/// dartq clobbers the `toF(source)` send's instantiated return to `Void`
/// (invocation.rs's function-typed-callee arm — fixed by #3122, which
/// now preserves `F<G>`), and the surviving `F<G<ITB>>` vs `F<G<…>>`
/// pair verdict lives in assignability.rs's typedef-pair arm — a lane
/// this ticket does not own.
#[test]
// un-ignored 2026-11-07: passing per stale-ignore triage (ticket #3973)
fn ia1_mutual_bound_typedef_reports_both_assignments() {
    let fixture = "\
typedef F<X> = void Function<Y extends X>();
typedef G<X extends Y, Y extends A<Y>> = void Function(Y);
class A<X> {}
F<X> toF<X>(X x) => <Y extends X>() {};
void test(G source) {
  var fsource = toF(source);
  F<G<A<dynamic>, Never>> target1 = fsource;
  F<G<A<Never>, Never>> target2 = fsource;
}
void main() {}
";
    let report = report_ops(fixture);
    let rows = rows_named(&report, "invalid_assignment");
    assert_eq!(
        rows.len(),
        2,
        "oracle reports two invalid_assignment rows: {:?}",
        report
            .diagnostics
            .iter()
            .map(|d| (d.code.name, d.range))
            .collect::<Vec<_>>()
    );
}

/// AT2 — the argument verdict reads `correspondingParameter.type`, the
/// already-instantiated formal
/// (pkg/analyzer/lib/src/generated/error_detection_helpers.dart:72-75):
/// `enum E<T> { a<int>("1") }` instantiates `T = int`, so `b<int>(2)` is
/// clean and `a<int>("1")` reports `String → int` — never `→ T`. dartq's
/// send solve keeps the declared `T` (invocation.rs, in-flight).
#[test]
fn at2_enum_ctor_arg_uses_instantiated_formal() {
    let fixture = "\
enum E<T> {
  a<int>(\"1\"),
  b<int>(2);
  const E(T t);
}
void main() {}
";
    let report = report_ops(fixture);
    let rows = rows_named(&report, "argument_type_not_assignable");
    assert_eq!(
        rows.len(),
        1,
        "only the String argument reports: {:?}",
        report
            .diagnostics
            .iter()
            .map(|d| (d.code.name, d.range))
            .collect::<Vec<_>>()
    );
    assert!(
        rows[0].message.contains("int"),
        "the message renders the instantiated int, not T: {:?}",
        rows[0].message
    );
}

/// The explicit-argument twin: `g<String>("s")` violates the bound, so
/// the oracle keeps the written `String` as the formal — `String →
/// String` asks no question — while the inferred solution would clamp.
/// dartq's `effective` clamps both (invocation.rs, in-flight).
#[test]
fn at2_explicit_bound_violation_reports_no_atna() {
    let fixture = "\
void g<T extends num>(T t) {}
void main() {
  g<String>(\"s\");
}
";
    let report = report_ops(fixture);
    assert_eq!(
        rows_named(&report, "type_argument_not_matching_bounds").len(),
        1,
        "the TANMB row fires: {:?}",
        report
            .diagnostics
            .iter()
            .map(|d| d.code.name)
            .collect::<Vec<_>>()
    );
    assert!(
        rows_named(&report, "argument_type_not_assignable").is_empty(),
        "explicit-arg formal stays the written String — no ATNA: {:?}",
        report
            .diagnostics
            .iter()
            .map(|d| d.code.name)
            .collect::<Vec<_>>()
    );
}
