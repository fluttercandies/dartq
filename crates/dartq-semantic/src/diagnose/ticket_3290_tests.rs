//! #3290 pins — the `redirect_to_non_const_constructor` target-constness
//! gate on the constructor-declaration face.
//!
//! `_checkForRedirectToNonConstConstructor` (error_verifier.dart:7330-7347)
//! reads the target element's `isConst`. Inside an enum every generative
//! constructor is implicitly const — with the primary-constructors feature a
//! generative constructor in an enum body may drop the `const` keyword, and
//! an enum primary constructor carries it implicitly (upstream
//! `enum_implicit_const_constructor_test`: "generative constructors in enums
//! are implicitly const and don't require the `const` keyword"). The written
//! `const` bit alone therefore reads a redirect inside an enum as
//! non-const-to-non-const and over-reports: the enum primary target holds no
//! declaration site at all, and a `const`-less enum constructor sits at a
//! site whose bit is false. The pins below hold the enum posture silent and
//! re-assert the class faces the gate owns.

use crate::diagnose::test_rig::{CORE_PRELUDE, report_ops};
use crate::diagnose::types::FileReport;

const REDIRECT: &str = "redirect_to_non_const_constructor";

/// The `redirect_to_non_const_constructor` rows of one report as
/// `(offset, length)`, in report order.
fn redirect_rows(report: &FileReport) -> Vec<(u32, u32)> {
    report
        .diagnostics
        .iter()
        .filter(|row| row.code.name == REDIRECT)
        .map(|row| (row.range.offset, row.range.length))
        .collect()
}

/// Asserts the report carries no `redirect_to_non_const_constructor` row
/// (other families — `unused_element`, `undefined_enum_constructor` — are
/// free to fire and are not this face's business).
#[track_caller]
fn assert_no_redirect(source: &str) {
    let report = report_ops(source);
    assert!(
        redirect_rows(&report).is_empty(),
        "expected no {REDIRECT} row, got {:?}",
        redirect_rows(&report)
    );
}

/// The offset of the `nth` occurrence of `needle` in `source`, as
/// [`report_ops`] reports it (the fixture rides behind [`CORE_PRELUDE`]).
#[track_caller]
fn at(source: &str, needle: &str, nth: usize) -> u32 {
    let hit = source
        .match_indices(needle)
        .nth(nth)
        .map_or_else(|| panic!("missing {needle:?}"), |(index, _)| index);
    u32::try_from(CORE_PRELUDE.len() + hit).expect("fixture offsets fit u32")
}

/// The enum primary constructor target: `enum E(int x)` declares the unnamed
/// generative constructor through its header, so the redirect holds no
/// declaration site — and the primary constructor is implicitly const.
#[test]
fn ticket_3290_enum_primary_redirect_stays_silent() {
    assert_no_redirect("enum E(int x) {\n  v(0);\n  const E.named(int x) : this(x);\n}\n");
}

/// The declared const target (the corpus `ConstructorBodyTest` shape): both
/// constructors carry `const`, so the gate reads the target's own bit.
#[test]
fn ticket_3290_enum_declared_const_redirect_stays_silent() {
    assert_no_redirect(
        "enum E {\n  v1, v2.named();\n  const E();\n  const E.named() : this();\n}\n",
    );
}

/// The `const`-less target inside an enum: the primary-constructors feature
/// makes it implicitly const, so the written bit being false must not report.
#[test]
fn ticket_3290_enum_const_less_target_stays_silent() {
    assert_no_redirect(
        "enum E {\n  e;\n  final int x;\n  E.named(this.x);\n  const E.unnamed() : this.named(1);\n}\n",
    );
}

/// The class face the gate owns: a `const` constructor redirecting to a
/// non-const constructor reports at the target selector
/// (`constructorSelector?.name2 ?? thisKeyword`).
#[test]
fn ticket_3290_class_non_const_redirect_reports() {
    let source = "class A {\n  A.a();\n  const A.b() : this.a();\n}\n";
    let report = report_ops(source);
    assert_eq!(
        redirect_rows(&report),
        vec![(at(source, "a()", 1), 1)],
        "the target selector `a` anchors the row"
    );
}

/// A const-to-const redirect stays silent (no printed pin regression).
#[test]
fn ticket_3290_class_const_redirect_stays_silent() {
    assert_no_redirect("class A {\n  const A.a();\n  const A.b() : this.a();\n}\n");
}

/// The unnamed `this()` target: a non-const unnamed constructor is the
/// implicit-default sibling — still never const, still reported at `this`.
#[test]
fn ticket_3290_class_unnamed_redirect_reports_at_this() {
    let source = "class A {\n  A();\n  const A.named() : this();\n}\n";
    let report = report_ops(source);
    assert_eq!(
        redirect_rows(&report),
        vec![(at(source, "this", 0), 4)],
        "the `this` keyword anchors the row"
    );
}

/// A redirecting factory's `= C.name` target anchors the row at the whole
/// `C.name` reference (the `factoryRedirectionTarget` extent, not a selector
/// token).
#[test]
fn ticket_3290_factory_redirect_reports_at_redirect_extent() {
    let source = "class A {\n  A.a();\n  const factory A.b() = A.a;\n}\n";
    let report = report_ops(source);
    assert_eq!(
        redirect_rows(&report),
        vec![(at(source, "A.a", 1), 3)],
        "the `A.a` reference anchors the factory row"
    );
}
