//! The #2890 pins: the #2869 residual bounds faces — the reference forms
//! without a call (`f<A>;` tear-off, `G<A>.named` constructor reference —
//! upstream `checkFunctionReference` / `checkConstructorReference`,
//! type_arguments_verifier.dart:45/:192), the super-send explicit
//! arguments (`super.m<A>(...)`), and the keyword-form creation
//! (`new G<A>()` / `const G<A>()`). Every report anchors at the offending
//! argument's written node; a satisfying argument and the dot-less type
//! literal (`G<A>;` — a `TypeLiteral`, whose visit runs no bounds face)
//! stay silent.

use super::test_rig::{CORE_PRELUDE, report_ops};
use super::*;

fn bounds_rows(report: &FileReport) -> Vec<(u32, u32, String)> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "type_argument_not_matching_bounds")
        .map(|d| (d.range.offset, d.range.length, d.message.to_string()))
        .collect()
}

/// Byte offset of `needle` inside the fixture source, shifted by the
/// rig's core prelude (the fixture rides behind it on one line).
fn at(source: &str, needle: &str) -> u32 {
    (CORE_PRELUDE.len() + source.find(needle).expect("needle in fixture")) as u32
}

/// The function tear-off with explicit type arguments
/// (`checkFunctionReference` → `_checkInvocationTypeArguments` over the
/// reference's own type arguments): `f<A>;` violates `T extends B` and
/// reports at the written argument.
#[test]
fn function_tear_off_bad_type_argument_reports_at_the_argument() {
    let source = r#"
class A {}
class B {}
void f<T extends B>(T x) {}
main() {
  f<A>;
}
"#;
    let report = report_ops(source);
    assert_eq!(
        bounds_rows(&report),
        [(
            at(source, "A>;"),
            1,
            "'A' doesn't conform to the bound 'B' of the type parameter 'T'.".to_string()
        )],
        "{:?}",
        report.diagnostics
    );
}

/// A satisfying tear-off argument stays silent (`B` is its own bound).
#[test]
fn function_tear_off_satisfying_argument_stays_silent() {
    let source = r#"
class A {}
class B {}
void f<T extends B>(T x) {}
main() {
  f<B>;
}
"#;
    let report = report_ops(source);
    assert!(bounds_rows(&report).is_empty(), "{:?}", report.diagnostics);
}

/// The constructor reference `G<A>.named` (any selector after the group
/// rewrites with the type arguments kept,
/// `_toConstructorReference_propertyAccess`): `checkConstructorReference`
/// checks them against the class's substituted bounds and reports at the
/// argument.
#[test]
fn constructor_reference_bad_type_argument_reports() {
    let source = r#"
class A {}
class B {}
class G<E extends B> {
  G.named();
}
main() {
  G<A>.named;
}
"#;
    let report = report_ops(source);
    assert_eq!(
        bounds_rows(&report),
        [(
            at(source, "A>.named"),
            1,
            "'A' doesn't conform to the bound 'B' of the type parameter 'E'.".to_string()
        )],
        "{:?}",
        report.diagnostics
    );
}

/// The dot-less spelling is a TypeLiteral instantiation — the oracle's
/// verifier visit for TypeLiteral runs no bounds face — so `G<A>;` stays
/// silent where `G<A>.named;` reports.
#[test]
fn type_literal_instantiation_stays_silent() {
    let source = r#"
class A {}
class B {}
class G<E extends B> {}
main() {
  G<A>;
}
"#;
    let report = report_ops(source);
    assert!(bounds_rows(&report).is_empty(), "{:?}", report.diagnostics);
}

/// The super-send explicit arguments (`checkMethodInvocation` over the
/// superclass method's own parameters): `super.m<A>(...)` violates the
/// method's `T extends B` and reports at the written argument — the super
/// walk feeds the same latch the plain sends read (#2869 residual).
#[test]
fn super_send_bad_type_argument_reports() {
    let source = r#"
class A {}
class B {}
class Sup {
  void m<T extends B>(T x) {}
}
class C extends Sup {
  void t() {
    super.m<A>(A());
  }
}
"#;
    let report = report_ops(source);
    assert_eq!(
        bounds_rows(&report),
        [(
            at(source, "A>(A())"),
            1,
            "'A' doesn't conform to the bound 'B' of the type parameter 'T'.".to_string()
        )],
        "{:?}",
        report.diagnostics
    );
}

/// The keyword-form creation `new G<A>()`: the constructor name's type is
/// a NamedType, and its written arguments check exactly like an
/// annotation's — the mirror of the landed implicit-creation pin.
#[test]
fn keyword_new_creation_bad_type_argument_reports() {
    let source = r#"
class A {}
class B {}
class G<E extends B> {}
main() {
  new G<A>();
}
"#;
    let report = report_ops(source);
    assert_eq!(
        bounds_rows(&report),
        [(
            at(source, "A>();"),
            1,
            "'A' doesn't conform to the bound 'B' of the type parameter 'E'.".to_string()
        )],
        "{:?}",
        report.diagnostics
    );
}

/// The `const` keyword form rides the same creation-reference seam (a
/// const constructor declared, so the keyword's own face stays silent).
#[test]
fn keyword_const_creation_bad_type_argument_reports() {
    let source = r#"
class A {}
class B {}
class G<E extends B> {
  const G();
}
main() {
  const G<A>();
}
"#;
    let report = report_ops(source);
    assert_eq!(
        bounds_rows(&report),
        [(
            at(source, "A>();"),
            1,
            "'A' doesn't conform to the bound 'B' of the type parameter 'E'.".to_string()
        )],
        "{:?}",
        report.diagnostics
    );
}
