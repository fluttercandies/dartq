//! The #2869 pins: the invocation-site type-argument bounds face
//! (`_checkInvocationTypeArguments`, upstream Dart SDK source,
//! pkg/analyzer/lib/src/error/type_arguments_verifier.dart:570-633).
//! Every explicit type argument of a generic send checks against its
//! parameter's bound substituted with the written arguments, the report
//! anchors at the offending argument's own node (`typeArgumentList[i]`),
//! a `dynamic` argument satisfies every bound (the top type), and an
//! unresolvable argument or bound reads `DynamicType`/null upstream —
//! the erroneous-type suppression, never a mismatch.

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

/// The generic top-level function send
/// (TypeArgumentNotMatchingBoundsTest/test_methodInvocation_topLevelFunction):
/// `f<String>()` violates `T extends num` and reports at the written
/// argument.
#[test]
fn generic_function_send_bad_type_argument_reports_at_the_argument() {
    let source = r#"
class Point<T extends num> {
  Point(T x, T y);
}
Point<T> f<T extends num>(T x, T y) {
  return new Point<T>(x, y);
}
main() {
  f<String>('hello', 'world');
}
"#;
    let report = report_ops(source);
    assert_eq!(
        bounds_rows(&report),
        [(
            at(source, "String>('hello'"),
            6,
            "'String' doesn't conform to the bound 'num' of the type parameter 'T'.".to_string()
        )],
        "{:?}",
        report.diagnostics
    );
}

/// The unnamed creation
/// (TypeArgumentNotMatchingBoundsTest/test_new_superTypeOfUpperBound
/// family): `G<A>()` violates `E extends B` — the creation's explicit
/// arguments check exactly like a send's.
#[test]
fn unnamed_creation_bad_type_argument_reports() {
    let source = r#"
class A {}
class B {}
class G<E extends B> {}
main() {
  G<A>();
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

/// The type-alias creation (ConstructorReferenceResolutionTest_TypeArgs/
/// test_alias_genericWithBound_unnamed_badBound family): the ALIAS's own
/// parameter bounds decide (`checkNamedType` reads
/// `alias.element.typeParameters` on the alias application).
#[test]
fn type_alias_creation_checks_the_alias_bounds() {
    let source = r#"
class A<T> {
  A();
}
typedef TA<T extends num> = A<T>;
main() {
  TA<String>();
}
"#;
    let report = report_ops(source);
    assert_eq!(
        bounds_rows(&report),
        [(
            at(source, "String>();"),
            6,
            "'String' doesn't conform to the bound 'num' of the type parameter 'T'.".to_string()
        )],
        "{:?}",
        report.diagnostics
    );
}

/// The substituted compound bound (`<T, U extends List<T>>`): the bound
/// spells the EARLIER written argument (`Substitution.fromPairs2` over
/// the written pairs), so `List<String>` violates `List<int>`.
#[test]
fn compound_bound_substitutes_the_written_arguments() {
    let source = r#"
void f<T, U extends List<T>>(T t, U u) {}
main() {
  f<int, List<String>>(0, []);
}
"#;
    let report = report_ops(source);
    assert_eq!(
        bounds_rows(&report),
        [(
            at(source, "List<String>>(0, [])"),
            12,
            "'List<String>' doesn't conform to the bound 'List<int>' of the type parameter 'U'."
                .to_string()
        )],
        "{:?}",
        report.diagnostics
    );
}

/// The erroneous-bound suppression (upstream reads a null bound when the
/// `extends` clause never resolves and skips the check): an unresolvable
/// argument or bound decides nothing — the oracle reports
/// `undefined_class` here, never a bounds mismatch.
#[test]
fn unresolvable_argument_or_bound_suppresses() {
    let source = r#"
class C<T extends Missing> {}
void f<T extends num>(T x) {}
main() {
  C<String>();
  f<AlsoMissing>(0);
}
"#;
    let report = report_ops(source);
    assert!(bounds_rows(&report).is_empty(), "{:?}", report.diagnostics);
}

/// The `dynamic` argument pin: the written `dynamic` IS bound-checked —
/// the strict subtype ask `dynamic <: num` is false, so the bound
/// rejects it (probe 3.13.7: `TYPE_ARGUMENT_NOT_MATCHING_BOUNDS` "'dynamic'
/// doesn't conform to the bound 'num' of the type parameter 'T'." at the
/// written `dynamic` argument's own span).
#[test]
fn dynamic_argument_satisfies_any_bound() {
    let source = r#"
void f<T extends num>(T x) {}
main() {
  f<dynamic>(0);
}
"#;
    let report = report_ops(source);
    let rows = bounds_rows(&report);
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert!(
        rows[0]
            .2
            .contains("'dynamic' doesn't conform to the bound 'num' of the type parameter 'T'"),
        "{rows:?}"
    );
    assert_eq!(rows[0].1, 7, "{rows:?}");
}

/// The satisfying generic-function-type argument
/// (TypeArgumentNotMatchingBoundsTest/
/// test_methodInvocation_genericFunctionTypeArgument_match): `F` unfolds
/// to `void Function<T extends num>()`, whose parameter bound equals the
/// target's per `relateTypeParameters` (`fn_sig_subtype`'s positional
/// unification) — the invocation face stays silent. The declaring
/// header's OWN bound window decides: a bound's function-type variables
/// (`<X extends num>` inside `T extends void Function<X extends num>()`)
/// close a nested group and never pair as the executable's parameters
/// (#2897 — the stolen `[(X, num)]` window made this fire against `num`).
#[test]
fn generic_function_type_argument_matching_bound_stays_silent() {
    let source = r#"
typedef F = void Function<T extends num>();
void f<T extends void Function<X extends num>()>() {}
void g() {
  f<F>();
}
"#;
    let report = report_ops(source);
    assert!(bounds_rows(&report).is_empty(), "{:?}", report.diagnostics);
}

/// The explicit extension override with a violating type argument
/// (TypeArgumentNotMatchingBoundsTest/
/// test_extensionOverride_hasTypeArguments_call): the written arguments
/// check against the extension's declared bounds exactly like a send's,
/// the report anchored at the offending argument.
#[test]
fn extension_override_bad_type_argument_reports_at_the_argument() {
    let source = r#"
extension E<T extends num> on int {
  void call() {}
}
void f() {
  E<String>(0)();
}
"#;
    let report = report_ops(source);
    assert_eq!(
        bounds_rows(&report),
        [(
            at(source, "String>(0)()"),
            6,
            "'String' doesn't conform to the bound 'num' of the type parameter 'T'.".to_string()
        )],
        "{:?}",
        report.diagnostics
    );
}
