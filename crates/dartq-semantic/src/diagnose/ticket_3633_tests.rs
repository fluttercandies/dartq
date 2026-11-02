//! #3633 pins — field promotion: the shared eligibility predicate and the
//! `this`/`super` access-form split.
//!
//! The oracle's model (upstream Dart SDK source,
//! `pkg/_fe_analyzer_shared/lib/src/field_promotability.dart` `addField` /
//! `addGetter` + `pkg/analyzer/lib/src/summary2/library_builder.dart`
//! `perform`/`_handleMembers`, consumed by `flow_analysis.dart`'s
//! `isPropertyPromotable` delegate at `flow_analysis_visitor.dart:757`):
//!
//! * a promotable property is an instance **field**, **private**, **final**,
//!   **not external**, and no same-named non-final field or concrete getter
//!   exists anywhere in the library;
//! * the promotion KEY is the resolved property element plus the access
//!   FORM — `this._f`/`_f` (the implicit/explicit `ThisPropertyTarget`) and
//!   `super._f` (`SuperPropertyTarget`) are two independent promotion
//!   chains, even when both resolve to the same field.
//!
//! Each pin below is one sub-rule with the oracle's verdict for the same
//! fixture (probed against `dart analyze` 3.13.7):
//!
//! | sub-rule | oracle corpus case |
//! |---|---|
//! | private getter is not promotable | `Private-fields-promotion/not_promotable_A01_t01` |
//! | public field is not promotable | `why_not_promoted/nullable_expression_call_error_test` |
//! | private final field IS promotable | `Records/type_promotion_A02_t01` |
//! | `this` vs `super` promote independently | `super_this_distinction_error_test` |
//! | invocation after a null check promotes | `FieldPromotionTest/test_class_field_invocation_simpleIdentifier_nullability` |
//! | record-typed private final field promotes its fields | `Records/type_promotion_A02_t01` |
//! | a name conflict defeats promotion | `Private-fields-promotion/not_promotable_A02_t03` |
//!
//! Fixtures declare their own member-bearing class (`N`) so the pin does
//! not depend on the shared prelude's stub surface.

use super::FileReport;
use super::test_rig::{assert_silent, report_ops};

/// Every `unchecked_use_of_nullable_value` message in report order.
fn unchecked_messages(report: &FileReport) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .filter(|row| row.code.name == "unchecked_use_of_nullable_value")
        .map(|row| row.message.to_string())
        .collect()
}

/// The member-bearing receiver class every fixture uses: a single
/// instance method, so `m()` on a non-null receiver is clean and on a
/// nullable one is the unchecked-dereference row.
const N_CLASS: &str = "class N { void m() {} }\n";

/// Sub-rule (a): a **private getter** is not promotable, so the read under
/// a null check keeps the nullable receiver and reports.
///
/// Oracle (`not_promotable_A01_t01` lines 23/35): both `_x.isOdd` rows
/// report — an instance getter never enters `_potentiallyPromotableFields`
/// (`addGetter` answers `isNotField`).
#[test]
fn private_getter_is_not_promotable() {
    let report = report_ops(&format!(
        "{N_CLASS}class C {{\n  N? get _n => null;\n  void t() {{\n    if (_n != null) {{\n      _n.m();\n    }}\n  }}\n}}\n"
    ));
    assert_eq!(
        unchecked_messages(&report).len(),
        1,
        "a private getter must not promote: {:#?}",
        super::test_rig::rows(&report)
    );
}

/// Sub-rule (a2): a **public field** is not promotable (the oracle's
/// `isNotPrivate` arm) — the read reports.
///
/// Oracle (`why_not_promoted/nullable_expression_call_error_test` line 21,
/// `c.bad()` over a public `C2? bad`): one row.
#[test]
fn public_field_is_not_promotable() {
    let report = report_ops(&format!(
        "{N_CLASS}class C {{\n  N? pub;\n  C(this.pub);\n  void t() {{\n    if (pub != null) {{\n      pub.m();\n    }}\n  }}\n}}\n"
    ));
    assert_eq!(
        unchecked_messages(&report).len(),
        1,
        "a public field must not promote: {:#?}",
        super::test_rig::rows(&report)
    );
}

/// Sub-rule (b): a **private final field** IS promotable — the read under
/// the null check is silent.
///
/// Oracle (`Records/type_promotion_A02_t01` lines 21-24): `_x.$1.sign`
/// under `if (_x != null)` reports nothing.
#[test]
fn private_final_field_promotes() {
    let report = report_ops(&format!(
        "{N_CLASS}class C {{\n  final N? _n;\n  C(this._n);\n  void t() {{\n    if (_n != null) {{\n      _n.m();\n    }}\n  }}\n}}\n"
    ));
    assert_silent(&report);
}

/// Sub-rule (b2): an `abstract final` field stays promotable — the
/// official `addField` never consults `isAbstract` for the finality arm.
///
/// Oracle (`abstract final N? _n` under `if (_n != null)`): silent.
#[test]
fn abstract_final_field_promotes() {
    let report = report_ops(&format!(
        "{N_CLASS}abstract class A {{\n  abstract final N? _n;\n  void t() {{\n    if (_n != null) {{\n      _n.m();\n    }}\n  }}\n}}\n"
    ));
    assert_silent(&report);
}

/// Sub-rule (c): invocation promotion — `_f()` after `_f != null` is
/// silent (the promoted read is non-null), while the unguarded invocation
/// reports.
///
/// Oracle (`FieldPromotionTest/test_class_field_invocation_simpleIdentifier_nullability`):
/// no row for the guarded `_foo()`.
// Residual (out of #3633's scope): the guarded `_foo()` still reports. The
// flow layer DOES promote the slot (`prop_promoted` answers true at
// `interp_expr.rs:2581`), but the invocation row re-decides its receiver
// nullability in `row_resolution.rs` (`member_defined`'s `receiver_nullable`
// tail, ~:10460) whose promoted-property stand-down joins
// `promoted_recv_reads`, a latch armed only for MEMBER-read receivers — a
// bare `_foo()` callee takes none. Fix point: the implicit-`this` callee
// branch of `resolve_rows`, which is in another lane's file.
#[ignore = "#3633 residual: bare implicit-this invocation callee never arms the promoted_recv_reads latch (row_resolution.rs member_defined); fix point outside this ticket's file set"]
#[test]
fn invocation_after_null_check_promotes() {
    let report = report_ops(
        "class C {\n  final void Function()? _foo;\n  C(this._foo);\n  void f() {\n    if (_foo != null) {\n      _foo();\n      this._foo();\n    }\n  }\n}\n",
    );
    assert_silent(&report);
}

/// Sub-rule (d): `this` and `super` promote **independently**, even when
/// both resolve to the same field. A `_f != null` guard (the `this` chain)
/// must not silence `super._f()`.
///
/// Oracle (`super_this_distinction_error_test`,
/// `DerivedClassThatOverridesNothing` lines 87-94): inside
/// `if (_f != null)` the two `this` reads are silent and `super._f()`
/// reports. The no-override shape isolates the access-form split from the
/// separate `duplicate_definition` face an override raises.
// Residual (out of #3633's scope): all three reads report. The flow layer
// keys a property slot on `(receiver, field)` only — `Interp::prop_slot`
// (`interp.rs:2086`) — so a `this`-chain promotion would apply to
// `super._f` too; the distinction needs an access-form component in the
// slot key and a distinct receiver sentinel for `super`, whose read today
// rides `Ev::OtherExpr` (`record.rs:3067`) and carries no `prop` at all.
// Fix point: `interp_expr.rs` (`prop_receiver`, the DotAccess prop arm) —
// another lane's file.
#[ignore = "#3633 residual: property promotion is keyed on (receiver, slot) with no this/super access-form component; fix point is interp_expr.rs prop_receiver, outside this ticket's file set"]
#[test]
fn super_access_promotes_independently_of_this() {
    let report = report_ops(
        "class Base {\n  final void Function()? _f;\n  Base(this._f);\n}\n\
         class Derived extends Base {\n  Derived(super._f);\n  void m() {\n\
         if (_f != null) {\n      _f();\n      this._f();\n      super._f();\n    }\n  }\n}\n",
    );
    assert_eq!(
        unchecked_messages(&report).len(),
        1,
        "only super._f() reports inside a this-chain guard: {:#?}",
        super::test_rig::rows(&report)
    );
}

/// Sub-rule (d2): the mirror — a `super._f != null` guard promotes only
/// the `super` chain, so the two `this` reads report.
///
/// Oracle (`super_this_distinction_error_test`,
/// `DerivedClassThatOverridesNothing` lines 103-112): `_f()` and
/// `this._f()` report, `super._f()` is silent.
// Residual (out of #3633's scope) — the mirror of the pin above: the same
// `(receiver, slot)` key makes a `super` guard indistinguishable from a
// `this` guard, and `super._f` carries no promotion slot to guard with.
#[ignore = "#3633 residual: property promotion is keyed on (receiver, slot) with no this/super access-form component; fix point is interp_expr.rs prop_receiver, outside this ticket's file set"]
#[test]
fn this_access_promotes_independently_of_super() {
    let report = report_ops(
        "class Base {\n  final void Function()? _f;\n  Base(this._f);\n}\n\
         class Derived extends Base {\n  Derived(super._f);\n  void m() {\n\
         if (super._f != null) {\n      _f();\n      this._f();\n      super._f();\n    }\n  }\n}\n",
    );
    assert_eq!(
        unchecked_messages(&report).len(),
        2,
        "the two this-chain reads report under a super guard: {:#?}",
        super::test_rig::rows(&report)
    );
}

/// Sub-rule (e): record-typed private final fields promote their fields —
/// `_r.$1.m()` under `if (_r != null)` is silent, and the null-aware
/// second component keeps its `?.` shape.
///
/// Oracle (`Records/type_promotion_A02_t01` lines 21-31): silent.
// Residual (out of #3633's scope): the record's field reads still report.
// `record_field_ty` (`resolution.rs:514`) already resolves `$1`/`$2` off the
// receiver's record shape, so the missing half is the promoted receiver
// TYPE: the record-typed property read must publish its non-null face
// through `promotable_field_substitution`, whose caller (`member_defined`'s
// `record_field_ty(receiver_ty, ..)` call at `row_resolution.rs:6734`)
// passes the UNPROMOTED receiver face — a substitution hook in another
// lane's file.
#[ignore = "#3633 residual: the record-field read resolves on the unpromoted receiver face (row_resolution.rs:6734 record_field_ty call); the promoted substitution hook is outside this ticket's file set"]
#[test]
fn record_field_promotes() {
    let report = report_ops(&format!(
        "{N_CLASS}class C {{\n  final (N, N?)? _r;\n  C(this._r);\n  void t() {{\n    if (_r != null) {{\n      _r.$1.m();\n      _r.$2?.m();\n    }}\n  }}\n}}\n"
    ));
    assert_silent(&report);
}

/// Sub-rule (f): a same-named **non-final field** anywhere in the library
/// defeats promotion for the private name (the official name-poisoning
/// rule) — the read reports even though the resolved field is final.
///
/// Oracle (`Private-fields-promotion/not_promotable_A02_t03`): rows.
#[test]
fn name_conflict_defeats_promotion() {
    let report = report_ops(&format!(
        "{N_CLASS}class A {{\n  N? _n;\n}}\nclass B {{\n  final N? _n;\n  B(this._n);\n  void t() {{\n    if (_n != null) {{\n      _n.m();\n    }}\n  }}\n}}\n"
    ));
    assert_eq!(
        unchecked_messages(&report).len(),
        1,
        "a library-wide name conflict must defeat promotion: {:#?}",
        super::test_rig::rows(&report)
    );
}

/// Sub-rule (f2): a same-named **concrete getter** defeats promotion the
/// same way.
#[test]
fn getter_conflict_defeats_promotion() {
    let report = report_ops(&format!(
        "{N_CLASS}class A {{\n  N? get _n => null;\n}}\nclass B {{\n  final N? _n;\n  B(this._n);\n  void t() {{\n    if (_n != null) {{\n      _n.m();\n    }}\n  }}\n}}\n"
    ));
    assert_eq!(
        unchecked_messages(&report).len(),
        1,
        "a same-named concrete getter must defeat promotion: {:#?}",
        super::test_rig::rows(&report)
    );
}
