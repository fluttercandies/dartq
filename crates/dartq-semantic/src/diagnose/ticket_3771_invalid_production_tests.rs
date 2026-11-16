//! The #3771 `Ty::Invalid` wave-1b pins: the three evidence-gated
//! production points and the two side gates, pinned against the oracle
//! fixture forms the ticket's `/tmp/r3544/` corpus carried (probed
//! 3.13.7, oracle text now encoded as in-repo fixtures — the originals
//! are purged, and the expected row sets below ARE the oracle's
//! `(code)` surfaces for each form).
//!
//! The contract: a written annotation that fails type-position
//! resolution materializes as the oracle's `InvalidType` — assignable
//! to and from everything — so the ONLY row a broken annotation ever
//! produces is its own resolution face (`undefined_class`,
//! `not_a_type`, `undefined_identifier`), never a secondary
//! assignability/not-initialized/arity avalanche. Each fixture asserts
//! the oracle's exact code set; the negative controls (f6, f7) prove
//! the widen did not swallow live reports.

#![allow(clippy::semicolon_if_nothing_returned)]
use crate::diagnose::test_rig::{
    RigImport, code_names, report_ops, report_ops_imports, report_ops_strict,
};
use crate::diagnose::{FileReport, StrictModes};

/// The sorted code-name set of `report` — the oracle's diagnostic
/// surface compared as a set (source order is the engine's own).
fn codes(report: &FileReport) -> Vec<&'static str> {
    let mut names = code_names(report);
    names.sort_unstable();
    names
}

/// f1 — `Unresolved x = null;`: the broken annotation produces ONLY its
/// own `undefined_class`. Pre-wave the declared type stayed
/// `Ty::Unknown`, which suppressed `invalid_assignment` anyway — the
/// production half's win is the InvalidType verdict itself, and this
/// pin keeps the avalanche silent through BOTH paths.
#[test]
fn f1_unresolved_declared_type_reports_only_undefined_class() {
    let report = report_ops("Unresolved x = null;\n");
    assert_eq!(
        codes(&report),
        ["undefined_class"],
        "{:?}",
        crate::diagnose::test_rig::rows(&report)
    );
}

/// f2b — `Unresolved x; int y;`: the invalid annotation suppresses
/// `not_initialized_non_nullable_variable` on `x` (upstream's
/// `isPotentiallyNullable` reads `InvalidType` nullable); the resolved
/// `int y` still reports.
#[test]
fn f2b_unresolved_field_suppresses_only_its_own_not_initialized() {
    let report = report_ops("Unresolved x; int y;\n");
    assert_eq!(
        codes(&report),
        ["not_initialized_non_nullable_variable", "undefined_class"],
        "{:?}",
        crate::diagnose::test_rig::rows(&report)
    );
    let not_init: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "not_initialized_non_nullable_variable")
        .collect();
    assert_eq!(
        not_init.len(),
        1,
        "{:?}",
        crate::diagnose::test_rig::rows(&report)
    );
}

/// f2c — `class C { Unresolved f; int g; }`: the same suppression twin
/// at instance-field scope — `f`'s invalid declared type suppresses
/// `not_initialized_non_nullable_instance_field`; `g` still reports.
#[test]
fn f2c_unresolved_instance_field_suppresses_only_its_own_not_initialized() {
    let report = report_ops("class C { Unresolved f; int g; }\n");
    assert_eq!(
        codes(&report),
        [
            "not_initialized_non_nullable_instance_field",
            "undefined_class"
        ],
        "{:?}",
        crate::diagnose::test_rig::rows(&report)
    );
}

/// f3 — `int f() => 0; f v = null;`: a FUNCTION name in a type
/// position is `not_a_type` upstream — its own report — and the
/// variable's invalid declared type suppresses `invalid_assignment`.
#[test]
fn f3_function_name_as_type_reports_only_not_a_type() {
    let report = report_ops("int f() => 0;\nf v = null;\n");
    assert_eq!(
        codes(&report),
        ["not_a_type"],
        "{:?}",
        crate::diagnose::test_rig::rows(&report)
    );
}

/// f4b — `class D { final Unresolved f; D() : f = "s"; }`: the field's
/// invalid declared type accepts the `"s"` initializer — no
/// `field_initializer_not_assignable` avalanche beside the head's own
/// `undefined_class`.
#[test]
fn f4b_invalid_field_type_accepts_any_initializer() {
    let report = report_ops("class D { final Unresolved f; D() : f = \"s\"; }\n");
    assert_eq!(
        codes(&report),
        ["undefined_class"],
        "{:?}",
        crate::diagnose::test_rig::rows(&report)
    );
}

/// f5 — `List<int, String> x = []; Map<int> y = {};`: resolved
/// generic heads keep their arity verdicts — the `InvalidType` wave
/// touches neither.
#[test]
fn f5_resolved_generic_heads_keep_arity_verdicts() {
    let report = report_ops("List<int, String> x = [];\nMap<int> y = {};\n");
    assert_eq!(
        codes(&report),
        [
            "wrong_number_of_type_arguments",
            "wrong_number_of_type_arguments"
        ],
        "{:?}",
        crate::diagnose::test_rig::rows(&report)
    );
}

/// f8 — `Unresolved a = 1; int b = a;`: `a`'s declared type is
/// `InvalidType`; `int b = a` reads `a` invalid → `Invalid <: int`
/// accepts — no `invalid_assignment` beside `undefined_class`.
#[test]
fn f8_invalid_read_assigns_into_int() {
    let report = report_ops("Unresolved a = 1;\nint b = a;\n");
    assert_eq!(
        codes(&report),
        ["undefined_class"],
        "{:?}",
        crate::diagnose::test_rig::rows(&report)
    );
}

/// f9 — `Unresolved x = null; x.foo;`: the member read on an
/// invalid-typed variable stays silent (upstream's invalid-bounded
/// short-circuit types the read `InvalidType` — `undefined_getter` is
/// the wave-2 question, pinned here as the regression posture).
#[test]
fn f9_member_read_on_invalid_receiver_stays_silent() {
    let report = report_ops("Unresolved x = null;\nvoid f() { x.foo; }\n");
    assert_eq!(
        codes(&report),
        ["undefined_class"],
        "{:?}",
        crate::diagnose::test_rig::rows(&report)
    );
}

/// f10 — `List<int, String> x = [1]; void h(List<int> l) {} … h([1])`:
/// the arity verdict on the resolved `List` head reports
/// `wrong_number_of_type_arguments`; the argument's own assignability
/// never doubles-report beside it.
#[test]
fn f10_arity_verdict_stands_alone_on_resolved_head() {
    let report =
        report_ops("List<int, String> x = [1];\nvoid h(List<int> l) {}\nvoid g() { h([1]); }\n");
    let names = code_names(&report);
    assert!(
        names.contains(&"wrong_number_of_type_arguments"),
        "{:?}",
        crate::diagnose::test_rig::rows(&report)
    );
    assert!(
        !names.contains(&"invalid_assignment"),
        "{:?}",
        crate::diagnose::test_rig::rows(&report)
    );
    assert!(
        !names.contains(&"argument_type_not_assignable"),
        "{:?}",
        crate::diagnose::test_rig::rows(&report)
    );
}

/// f11 (strict-casts: true) — `Unresolved u = null; int i1 = u;
/// dynamic d = null; int i2 = d;`: `u`'s invalid declared type accepts
/// `i1` (the `InvalidType` arm runs ahead of the strict-casts overlay —
/// it is NOT a `dynamic` downcast), while `d`'s `dynamic` source still
/// reports `invalid_assignment` under the mode.
#[test]
fn f11_invalid_accept_survives_strict_casts() {
    let report = report_ops_strict(
        "Unresolved u = null;\nint i1 = u;\ndynamic d = null;\nint i2 = d;\n",
        StrictModes {
            casts: true,
            ..StrictModes::default()
        },
    );
    let names = code_names(&report);
    assert!(
        names.contains(&"undefined_class"),
        "{:?}",
        crate::diagnose::test_rig::rows(&report)
    );
    let assignments: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "invalid_assignment")
        .collect();
    assert_eq!(
        assignments.len(),
        1,
        "exactly the `int i2 = d` downcast reports: {:?}",
        crate::diagnose::test_rig::rows(&report)
    );
}

/// f12 — `void h(Unresolved p) {} h(42);`: the parameter's invalid
/// declared type accepts every argument — no
/// `argument_type_not_assignable` beside `undefined_class`.
#[test]
fn f12_invalid_parameter_type_accepts_every_argument() {
    let report = report_ops("void h(Unresolved p) {}\nvoid g() { h(42); }\n");
    assert_eq!(
        codes(&report),
        ["undefined_class"],
        "{:?}",
        crate::diagnose::test_rig::rows(&report)
    );
}

/// f13 — `var v = missing; int i = v;` inside a body: `missing` is
/// `undefined_identifier`; `v`'s inferred `InvalidType` propagates so
/// `int i = v` accepts (the P2+P7 propagation pin); `i` unused reports
/// `unused_local_variable`.
#[test]
fn f13_invalid_propagates_through_inferred_local() {
    let report = report_ops("void f() { var v = missing; int i = v; }\n");
    let names = code_names(&report);
    assert!(
        names.contains(&"undefined_identifier"),
        "{:?}",
        crate::diagnose::test_rig::rows(&report)
    );
    assert!(
        names.contains(&"unused_local_variable"),
        "{:?}",
        crate::diagnose::test_rig::rows(&report)
    );
    assert!(
        !names.contains(&"invalid_assignment"),
        "{:?}",
        crate::diagnose::test_rig::rows(&report)
    );
}

/// f14 — `Unresolved<int> x = []; Unresolved<int, int> y = {};`: a
/// generic head that resolves to nothing is `InvalidType` WHOLE — no
/// arity verdict ever runs on it (the arity check's resolved-head
/// requirement). Each broken annotation produces only its own
/// `undefined_class`.
#[test]
fn f14_unresolved_generic_heads_report_only_undefined_class() {
    let report = report_ops("Unresolved<int> x = [];\nUnresolved<int, int> y = {};\n");
    assert_eq!(
        codes(&report),
        ["undefined_class", "undefined_class"],
        "{:?}",
        crate::diagnose::test_rig::rows(&report)
    );
}

/// f15 — `int f() => 0; f<int> v = null; typedef A = int; A<int> w = 0;`:
/// a non-type head with arguments is `not_a_type` (never the arity
/// face); the resolved typedef `A` keeps `wrong_number_of_type_arguments`.
#[test]
fn f15_non_type_head_with_args_skips_arity_but_resolved_alias_reports() {
    let report = report_ops("int f() => 0;\nf<int> v = null;\ntypedef A = int;\nA<int> w = 0;\n");
    let names = code_names(&report);
    assert!(
        names.contains(&"not_a_type"),
        "{:?}",
        crate::diagnose::test_rig::rows(&report)
    );
    let arity: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "wrong_number_of_type_arguments")
        .collect();
    assert_eq!(
        arity.len(),
        1,
        "exactly the resolved `A` head reports arity: {:?}",
        crate::diagnose::test_rig::rows(&report)
    );
}

/// f6 negative control — `dynamic d = null; int i = d;` under
/// `strict-casts: true`: the `dynamic` downcast still reports
/// `invalid_assignment` — the InvalidType widen must not reach live
/// verdicts. (The non-strict posture is silent upstream too — `dynamic`
/// downcasts are legal without the mode — so the strict mode is the
/// discriminating arm.)
#[test]
fn f6_dynamic_downcast_still_reports() {
    let report = report_ops_strict(
        "dynamic d = null;\nint i = d;\n",
        StrictModes {
            casts: true,
            ..StrictModes::default()
        },
    );
    assert_eq!(
        codes(&report),
        ["invalid_assignment"],
        "{:?}",
        crate::diagnose::test_rig::rows(&report)
    );
}

/// f7 negative control — a real Gap receiver (`f.missing` over a
/// resolved `f` whose member `missing` does not exist) keeps its own
/// report face: the widen only covers the annotation classes, never
/// the member-miss verdicts.
#[test]
fn f7_member_miss_on_typed_receiver_still_reports() {
    let report =
        report_ops("class C { int m() => 0; }\nC f() => C();\nvoid g() { f().missing; }\n");
    assert!(
        !code_names(&report).is_empty(),
        "the member miss must keep reporting: {:?}",
        crate::diagnose::test_rig::rows(&report)
    );
}

/// The `p.C` prefixed miss (`import … as p; p.Undefined x = null;`):
/// the real import prefix's member miss is `InvalidType` upstream —
/// only `undefined_class` at `C`, no avalanche. This is the PrefixHead/
/// Prefixed production class (the `armed_ty_invalid` latch's widened
/// face).
#[test]
fn prefixed_member_miss_produces_invalid_declared_type() {
    let report = report_ops_imports(
        "p.Undefined x = null;\n",
        &[RigImport {
            uri: "package:t/lib.dart",
            prefix: Some("p"),
            source: "class Real {}\n",
        }],
    );
    assert_eq!(
        codes(&report),
        ["undefined_class"],
        "{:?}",
        crate::diagnose::test_rig::rows(&report)
    );
}

/// A resolved `p.C` keeps its own verdict — the `is_invalid_ty`
/// predicate's distinguishing test: a REAL `p.C` that resolves must NOT
/// fold into `Invalid` (the `named_type_is_type` trap — its bare-scope
/// walk cannot see `p.C`, so the wrong predicate would fold a resolved
/// `p.C` into `Invalid` and under-report). The discriminating surface
/// is the value-side fold: `p.Real x` reads `x`'s declared type
/// `Ty::Named{"p.Real"}` into `h(x)`, whose `assignable` entry calls
/// `fold_invalid_ty` → `is_invalid_ty` → `resolve_joined_container` —
/// the walk that answers `p.Real` resolved. A wrongly-folded `p.Real`
/// would make `Invalid <: int` accept and the
/// `argument_type_not_assignable` row would vanish.
///
/// #4320 closed the pre-existing `p.C` verdict gaps this pin once
/// documented: `h(x)` now reports `argument_type_not_assignable` (the
/// `p.Real` head resolves to its real class shape inside `assignable`'s
/// primitive-vs-declared arm), exactly as the same-file `Real` baseline
/// above does. The pin's contract is unchanged — a resolved `p.C` never
/// reports `undefined_class`/`not_a_type` — and the new verdict is the
/// oracle's own answer for the pair.
#[test]
fn resolved_prefixed_type_keeps_its_verdict() {
    // Baseline: a `Real`-typed variable passed where `int` is required
    // reports `argument_type_not_assignable` — the live verdict a
    // wrongly-folded `p.Real` would suppress.
    let same_file =
        report_ops("class Real {}\nReal x = Real();\nvoid h(int r) {}\nvoid g() { h(x); }\n");
    assert_eq!(
        codes(&same_file),
        ["argument_type_not_assignable"],
        "{:?}",
        crate::diagnose::test_rig::rows(&same_file)
    );
    let report = report_ops_imports(
        "p.Real x = p.Real();\nvoid h(int r) {}\nvoid g() { h(x); }\n",
        &[RigImport {
            uri: "package:t/lib.dart",
            prefix: Some("p"),
            source: "class Real {}\n",
        }],
    );
    // The observable contract: `p.Real` resolves (no `undefined_class`
    // row — the `prefixed_hit` answered), and no other face fires. The
    // `h(x)` assignable ask folds `from = Ty::Named{"p.Real"}` through
    // `fold_invalid_ty` → `is_invalid_ty` → `resolve_joined_container`,
    // which probe-verified `resolved_container=true` → `false` (NO
    // fold). #4320's `head_container` wiring lets `assignable`'s
    // primitive-vs-declared arm read `p.Real` as the resolved class it
    // is, so `h(x)` reports `argument_type_not_assignable` — the oracle's
    // own verdict for `Real`-into-`int`, asserted by the baseline above.
    assert!(
        !codes(&report).contains(&"undefined_class"),
        "resolved `p.Real` must not report a resolution miss: {:?}",
        crate::diagnose::test_rig::rows(&report)
    );
    assert!(
        !codes(&report).contains(&"not_a_type"),
        "resolved `p.Real` must not read non-type: {:?}",
        crate::diagnose::test_rig::rows(&report)
    );
}

/// The argument-level fold: `List<Unresolved>` produces
/// `List<InvalidType>` — the whole-vs-argument split
/// (`named_type_resolver`): the head's arity stays decided, and the
/// argument face is the oracle's splice, not a whole-type `Invalid`. The
/// declared variable accepts the `[]` initializer — no assignability
/// avalanche beside the argument's own resolution row.
#[test]
fn invalid_argument_splices_into_parameterized_head() {
    let report = report_ops("List<Unresolved> x = [];\n");
    let names = code_names(&report);
    assert_eq!(
        names.len(),
        1,
        "only the argument's own resolution row stands: {:?}",
        crate::diagnose::test_rig::rows(&report)
    );
    assert!(
        !names.contains(&"invalid_assignment"),
        "{:?}",
        crate::diagnose::test_rig::rows(&report)
    );
}
