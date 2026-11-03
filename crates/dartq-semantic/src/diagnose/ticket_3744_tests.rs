//! #3744 — the fabricated `type_argument_not_matching_bounds` prune.
//!
//! Pass 1 checks an extension-member send's written type arguments against
//! the member's bound instantiated over the receiver's FROZEN push-time
//! face; the oracle checks against the receiver expression's resolved
//! static type — promoted where a promotion stands
//! (`type_arguments_verifier.dart` reads `node.function.staticType`). A
//! promoted local receiver (`n is int; n.expectStaticType<Exactly<int>>()`)
//! therefore carried a fabricated extra: the frozen `num` face checked
//! `Exactly<num>` where the oracle's promoted `int` face checks
//! `Exactly<int>` and stays silent. The teardown prune re-decides every
//! emitted row anchored inside a member send's written group against the
//! pass-2 receiver re-derivation and drops the rows whose re-verdict
//! conforms.
//!
//! Oracle provenance: `~/.dartq-oracle/3.13.7/dart-sdk/bin/dart analyze`
//! over a package fixture — each pin names the oracle verdict it encodes.

use super::test_rig::{assert_silent, code_names, report_ops, report_ops_experiments_with, rows};

/// The co19 `StaticType`/`Exactly<T>` extension the census cases use.
const EXPECT_HELPER: &str = "typedef Exactly<T> = T Function(T);\n\
     extension StaticType<T> on T {\n\
     \x20 T expectStaticType<R extends Exactly<T>>() => this;\n\
     }\n";

/// The `type_argument_not_matching_bounds` rows of `report` as
/// `code @offset: message` strings.
fn bounds_rows(report: &super::FileReport) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "type_argument_not_matching_bounds")
        .map(|d| format!("{} @{}: {}", d.code.name, d.range.offset, d.message))
        .collect()
}

/// A local `n is int` promotion: the promoted receiver reads `int`, the
/// bound instantiates `T := int`, and `Exactly<int>` conforms — the oracle
/// stays silent (the fabricated-row shape this ticket removes).
#[test]
fn promoted_local_receiver_stays_silent() {
    let report = report_ops(&format!(
        "{EXPECT_HELPER}void f(num n) {{\n\
         \x20 if (n is int) {{\n\
         \x20   n.expectStaticType<Exactly<int>>();\n\
         \x20 }}\n\
         }}\n"
    ));
    assert_silent(&report);
}

/// The promoted local's covariant-override read: `a is B` re-types `a.x`
/// to `int` (B's getter overrides A's `num`), `T := int` conforms — the
/// oracle stays silent.
#[test]
fn promoted_local_member_read_stays_silent() {
    let report = report_ops(&format!(
        "{EXPECT_HELPER}class A {{\n\
         \x20 num get x => 1;\n\
         }}\n\
         class B extends A {{\n\
         \x20 @override\n\
         \x20 int get x => 1;\n\
         }}\n\
         void f(A a) {{\n\
         \x20 if (a is B) {{\n\
         \x20   a.x.expectStaticType<Exactly<int>>();\n\
         \x20 }}\n\
         }}\n"
    ));
    assert_silent(&report);
}

/// A WRONG expectation over the promoted receiver still reports in the
/// oracle: `T := int` makes the bound `Exactly<int>`, and `Exactly<num>`
/// does not conform — one row. Pass 1 never emitted here at all (the
/// frozen `num` face saw `Exactly<num>` conform), so this is the ticket's
/// MISSING-side twin: fabricating it needs the promoted face at the emit
/// seam (`check_extension_member_type_arg_bounds`, invocation.rs), which
/// a sibling lane owns — pinned `#[ignore]` as the documented residual.
#[test]
#[ignore = "missing-side residual: pass-1 never emitted; the emit seam lives in invocation.rs"]
fn promoted_local_wrong_expectation_reports() {
    let report = report_ops(&format!(
        "{EXPECT_HELPER}void f(num n) {{\n\
         \x20 if (n is int) {{\n\
         \x20   n.expectStaticType<Exactly<num>>();\n\
         \x20 }}\n\
         }}\n"
    ));
    assert_eq!(bounds_rows(&report).len(), 1, "{:#?}", rows(&report));
}

/// The unpromoted control: the same send outside the `is` guard keeps the
/// frozen face — `T := num`, `Exactly<int>` fails — the oracle fires.
#[test]
fn unpromoted_receiver_reports() {
    let report = report_ops(&format!(
        "{EXPECT_HELPER}void f(num n) {{\n\
         \x20 n.expectStaticType<Exactly<int>>();\n\
         }}\n"
    ));
    assert_eq!(bounds_rows(&report).len(), 1, "{:#?}", rows(&report));
}

/// The `this is B` guard does NOT promote the implicit-`this` property
/// read's bounds face: the oracle checks `x.expectStaticType<Exactly<int>>()`
/// under `this is B` against `T := num` — the declared getter return —
/// and FIRES (verified on 3.13.7 with `this-promotion` enabled). The
/// prune must not over-drop: a re-derived face that differs only where
/// the oracle's own verdict keeps the declared face keeps the row.
#[test]
fn this_is_guard_keeps_the_declared_bound() {
    let report = report_ops_experiments_with(
        &format!(
            "{EXPECT_HELPER}class A {{\n\
             \x20 num get x => 1;\n\
             \x20 void t() {{\n\
             \x20   if (this is B) {{\n\
             \x20     x.expectStaticType<Exactly<int>>();\n\
             \x20   }}\n\
             \x20 }}\n\
             }}\n\
             class B extends A {{\n\
             \x20 @override\n\
             \x20 int get x => 1;\n\
             }}\n"
        ),
        &[],
        None,
        &[dartq_language::Feature::ThisPromotion],
    );
    assert_eq!(bounds_rows(&report).len(), 1, "{:#?}", rows(&report));
}

/// A non-extension member send never reaches the prune: a container's own
/// generic method checks its own parameter table — the frozen face's row
/// is genuine. `C().m<num>()` over `void m<T extends int>()` fires (the
/// oracle reports the violation; the send never was an extension
/// application).
#[test]
fn container_member_violation_keeps() {
    let report = report_ops(
        "class C {\n\
         \x20 void m<T extends num>() {}\n\
         }\n\
         void f() {\n\
         \x20 C().m<String>();\n\
         }\n",
    );
    assert!(
        code_names(&report).contains(&"type_argument_not_matching_bounds"),
        "{:#?}",
        rows(&report)
    );
}
