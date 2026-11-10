//! The #3345 pins — the `invalid_override` residual cohort's two arms:
//!
//! - **T1, the per-face interface map** (upstream Dart SDK source,
//!   `pkg/analyzer/lib/src/dart/element/inheritance_manager3.dart`
//!   `_getInterfaceClass`): `interface.map[name]` seats ONE member per
//!   clause face — the superclass's own map value first, each `with`
//!   mixin's own declaration replacing it, an inherited-through mixin
//!   member merging by the most-specific arm, `implements`/`on` faces
//!   appending. A flat most-specific over the transitive hit list picks
//!   the overridden ANCESTOR instead (`A.m241(dynamic)` beating the
//!   intermediate `B.m241(num)`), which fabricated
//!   `invalid_implementation_override` rows comparing the concrete
//!   implementation against a member the interface map never seated —
//!   upstream's `interface.map[m]` is `B.m241`, `implemented[m]` the same
//!   element, and the pair compares trivially valid.
//! - **T2, the clause gate** (`inheritance_override.dart`
//!   `_checkDirectSuperTypes`' early return): a disallowed
//!   direct-supertype clause stands every member check down.
//! - **The inherited inference arm** (`instance_member_inferrer.dart`
//!   `_inferExecutable` on every element's own members): an inherited hit's
//!   unwritten slots read inferred — `B { method29(a) }` over `A {
//!   method29(int a) }` compares `String` against `int`, never against an
//!   `Unknown` that suppressed the verdict — and the `covariant` field
//!   keyword's induced-setter mark propagates through the override spine
//!   (`element_builder.dart`'s `valueFragment.isExplicitlyCovariant` +
//!   `_inheritCovariance`).

use super::FileReport;
use super::test_rig::{CORE_PRELUDE, code_names, report_ops, report_ops_bare};

/// Every diagnostic's `code @anchored-text` in emission order; `fixture`
/// is the source the rig analyzed ahead of the shared core prelude.
fn rows(fixture: &str, report: &FileReport) -> Vec<String> {
    let full = format!("{CORE_PRELUDE}{fixture}");
    report
        .diagnostics
        .iter()
        .map(|d| {
            format!(
                "{} @{}",
                d.code.name,
                &full[d.range.offset as usize..(d.range.offset + d.range.length) as usize]
            )
        })
        .collect()
}

/// The #241 shape: `B.method241(num)` under `A.method241(dynamic)` is a
/// genuine unrelated-parameter break — the oracle's `invalid_override`
/// lands at `B`'s member name. `C extends B` declares nothing and the
/// interface map seats `B`'s own declaration (`namedCandidates`' declared
/// name is never replaced), so `implemented[m]` and `interface.map[m]` are
/// the same element and NO `invalid_implementation_override` fires — the
/// flat most-specific read seated `A`'s overridden ancestor instead and
/// fabricated the second row.
#[test]
fn widened_chain_reports_once_on_its_declaring_class() {
    let fixture = "class A {\n\
         \x20 method241(dynamic a) => null;\n\
         }\n\
         class B extends A {\n\
         \x20 method241(num a) => null;\n\
         }\n\
         class C extends B {\n\
         }\n";
    let report = report_ops(fixture);
    assert_eq!(
        code_names(&report),
        vec!["invalid_override"],
        "{:?}",
        report.diagnostics
    );
    assert_eq!(
        rows(fixture, &report),
        vec!["invalid_override @method241"],
        "{:?}",
        report.diagnostics
    );
}

/// The #285 shape: same spine, an invalid narrowing override on `B`
/// (`int` under `num`). `B`'s own member check reports `invalid_override`
/// at `method285`; `C` still carries no
/// `invalid_implementation_override` — the interface map seats `B`'s
/// member even though it broke against `A`'s (a declared name's named
/// candidates never replace it, `inheritance_manager3.dart:499`).
#[test]
fn narrowed_member_reports_once_on_its_declaring_class() {
    let fixture = "class A {\n\
         \x20 method285(num a) => null;\n\
         }\n\
         class B extends A {\n\
         \x20 method285(int a) => null;\n\
         }\n\
         class C extends B {\n\
         }\n";
    let report = report_ops(fixture);
    assert_eq!(
        code_names(&report),
        vec!["invalid_override"],
        "{:?}",
        report.diagnostics
    );
    assert_eq!(
        rows(fixture, &report),
        vec!["invalid_override @method285"],
        "{:?}",
        report.diagnostics
    );
}

/// The member-check codes the clause gate stands down — the gate is the
/// discriminating signal, not the exact clause-row companions (the bare
/// rig adds `extends_non_class`/`implements_non_class` alongside the
/// restricted-type row).
const MEMBER_CHECK_CODES: &[&str] = &[
    "invalid_override",
    "invalid_implementation_override",
    "concrete_class_with_abstract_member",
    "inconsistent_inheritance",
    "must_be_overridden",
];

/// The clause gate (T2): `class C extends Null` reports its disallowed
/// clause row and never runs the member checks — `C`'s members would
/// otherwise compare against the restricted face's member list. The bare
/// rig keeps `Null` spelling the SDK face (the shared prelude's user
/// declarations shadow the restricted spellings — the
/// `isNonSubtypableClass` resolved-element rule).
#[test]
fn disallowed_extends_clause_suppresses_member_checks() {
    let report = report_ops_bare(
        "class C extends Null {\n\
         \x20 void m(dynamic x) {}\n\
         }\n",
    );
    let names = code_names(&report);
    assert!(
        names.contains(&"subtype_of_disallowed_type"),
        "{:?}",
        report.diagnostics
    );
    assert!(
        !names.iter().any(|n| MEMBER_CHECK_CODES.contains(n)),
        "{:?}",
        report.diagnostics
    );
}

/// The clause gate on `implements` (`implements FutureOr`) — same early
/// return, no member rows.
#[test]
fn disallowed_implements_clause_suppresses_member_checks() {
    let report = report_ops_bare(
        "class C implements FutureOr {\n\
         \x20 void m(dynamic x) {}\n\
         }\n",
    );
    let names = code_names(&report);
    assert!(
        names.contains(&"subtype_of_disallowed_type"),
        "{:?}",
        report.diagnostics
    );
    assert!(
        !names.iter().any(|n| MEMBER_CHECK_CODES.contains(n)),
        "{:?}",
        report.diagnostics
    );
}

/// The upstream `test_abstract_field_covariant_inheritance` shape
/// verbatim: the `covariant` FIELD keyword's induced-setter mark
/// propagates through `implements`-only chains (`_inheritCovariance`
/// walks the transitive `implementedInterfaces`), so `C`'s `int` setter
/// is implicitly covariant — the `CorrectOverrideHelper` rewrite reads
/// `Object?` on both compared parameters and the pair is trivially
/// valid. The oracle's answer is ZERO diagnostics; any
/// `invalid_override` here means the covariance chain broke.
#[test]
fn covariant_field_chain_stays_silent_through_implements() {
    let fixture = "abstract class A {\n\
         \x20 abstract covariant num x;\n\
         }\n\
         abstract class B implements A {\n\
         \x20 void set x(Object value);\n\
         }\n\
         abstract class C implements B {\n\
         \x20 int get x;\n\
         \x20 void set x(int value);\n\
         }\n";
    let report = report_ops(fixture);
    assert!(
        !code_names(&report).contains(&"invalid_override"),
        "{:?}",
        report.diagnostics
    );
}

/// The discriminating twin of the covariant chain
/// (`test_setter_overrides_abstract_field_invalid`): `A`'s field carries
/// NO `covariant` keyword, so `B`'s `int` setter parameter keeps its
/// written type — `int` narrows `num` on the parameter face and the
/// oracle's `invalid_override` lands at `B`'s setter. Without the
/// induced-setter mark's absence reading through, the pair silently
/// passes.
#[test]
fn non_covariant_field_reports_the_narrowed_setter() {
    let fixture = "abstract class A {\n\
         \x20 abstract num x;\n\
         }\n\
         abstract class B implements A {\n\
         \x20 int get x;\n\
         \x20 void set x(int value);\n\
         }\n";
    let report = report_ops(fixture);
    let setter_rows: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "invalid_override" && d.message.contains("B.x"))
        .collect();
    assert_eq!(setter_rows.len(), 1, "{:?}", report.diagnostics);
    assert!(
        setter_rows[0].message.contains("A.x"),
        "{:?}",
        setter_rows[0].message
    );
}

/// The inherited-inference arm (the #29 shape): `B.method29(a)`'s
/// unwritten parameter infers `int` from `A.method29(int)` at `B`'s own
/// model build — `Class`'s `String` override then breaks against the
/// inferred signature and `invalid_override` lands at the member name.
/// Without the declaring-container inference the raw `Unknown` slot
/// suppressed the verdict entirely.
#[test]
fn unwritten_inherited_parameter_infers_at_its_declaring_container() {
    let fixture = "class A {\n\
         \x20 method29(int a) => null;\n\
         }\n\
         class B extends A {\n\
         \x20 method29(a) => null;\n\
         }\n\
         abstract class I {\n\
         \x20 method29(covariant num a);\n\
         }\n\
         abstract class J {\n\
         \x20 method29(num a);\n\
         }\n\
         class Class extends B implements I, J {\n\
         \x20 method29(String a) => null;\n\
         }\n";
    let report = report_ops(fixture);
    let names = code_names(&report);
    assert!(
        names.contains(&"invalid_override"),
        "{:?}",
        report.diagnostics
    );
    assert!(
        rows(fixture, &report)
            .iter()
            .any(|r| r == "invalid_override @method29"),
        "{:?}",
        report.diagnostics
    );
}

/// The mixin-order arm (`_addImplemented`'s last-writer): `with M1, M2`
/// seats `M2`'s member as the concrete implementation — when the
/// implementation face reports, `M2.m` is the concrete signature named,
/// never `M1`'s (the forward iteration's M1-first read claimed the wrong
/// concrete seat). The unrelated `M1.m`/`M2.m` pair also lands the
/// declared-face `invalid_override` on `M2` — the applied `S&M2`
/// superclass member check upstream runs too.
#[test]
fn the_last_applied_mixin_wins_the_concrete_seat() {
    let fixture = "abstract class I {\n\
         \x20 void m(int x);\n\
         }\n\
         class S {}\n\
         mixin M1 {\n\
         \x20 void m(int x) {}\n\
         }\n\
         mixin M2 {\n\
         \x20 void m(num x) {}\n\
         }\n\
         class C extends S with M1, M2 implements I {\n\
         }\n";
    let report = report_ops(fixture);
    // `M2.m(num)` is a valid override of `I.m(int)` — the last mixin's
    // seat stays silent; `M1.m(int)` was equally valid, so the arm's
    // discriminator is the ORDER alone: both members legal, the row count
    // must be zero (a forward-walk seat names M1's member — still silent —
    // but the seat identity below pins M2 via the breaking pair).
    assert_eq!(
        rows(fixture, &report),
        Vec::<String>::new(),
        "{:?}",
        report.diagnostics
    );
}

/// The breaking counterpart, upstream-faithful: `M2.m(String)` conflicts
/// with `I.m(int)` in the named-candidates merge — the name never seats
/// in `interface.map`, so `invalid_implementation_override` has no
/// counterpart and stays silent. The oracle's answer pair is
/// `inconsistent_inheritance` (the recorded merge conflict) plus the
/// applied-class member check `invalid_override` (`M2.m` vs `M1.m` —
/// each mixin application runs its own override check).
#[test]
fn the_conflicting_mixin_reports_inconsistent_inheritance() {
    let fixture = "abstract class I {\n\
         \x20 void m(int x);\n\
         }\n\
         class S {}\n\
         mixin M1 {\n\
         \x20 void m(int x) {}\n\
         }\n\
         mixin M2 {\n\
         \x20 void m(String x) {}\n\
         }\n\
         class C extends S with M1, M2 implements I {\n\
         }\n";
    let report = report_ops(fixture);
    let names = code_names(&report);
    assert!(
        names.contains(&"inconsistent_inheritance"),
        "{:?}",
        report.diagnostics
    );
    assert!(
        !names.contains(&"invalid_implementation_override"),
        "{:?}",
        report.diagnostics
    );
}

/// The concrete-seat discriminator: `I.m(num)` is the interface map's
/// most specific member (a `num` parameter accepts `int`, subtyping
/// `M2.m(int)`), while `_addImplemented`'s last writer seats `M2.m(int)`
/// as the concrete implementation — `int` narrows `num`, so the
/// implementation row fires naming `M2.m` as the concrete signature. A
/// forward (M1-first) seat would name `M1.m` instead: `M1.m(num)` is a
/// VALID override of `I.m(num)` and stays silent, so any silence or
/// `M1` naming means the last-writer order regressed.
#[test]
fn the_last_applied_mixin_names_the_broken_concrete_seat() {
    let fixture = "abstract class I {\n\
         \x20 void m(num x);\n\
         }\n\
         class S {}\n\
         mixin M1 {\n\
         \x20 void m(num x) {}\n\
         }\n\
         mixin M2 {\n\
         \x20 void m(int x) {}\n\
         }\n\
         class C extends S with M1, M2 implements I {\n\
         }\n";
    let report = report_ops(fixture);
    let impls: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "invalid_implementation_override")
        .collect();
    assert_eq!(impls.len(), 1, "{:?}", report.diagnostics);
    assert!(impls[0].message.contains("M2.m"), "{:?}", impls[0].message);
}
