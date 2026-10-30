//! The #2908 pins: the instantiation-site bounds faces the capture windows
//! shadowed — a type ALIAS instantiation checks its written arguments
//! against the alias's own parameters at every site kind (`type.alias !=
//! null`, upstream Dart SDK source,
//! pkg/analyzer/lib/src/error/type_arguments_verifier.dart:321-325:
//! annotation, clause, and function-type-alias use alike), and a written
//! type-parameter bound is itself a checked `NamedType` (`checkNamedType`
//! at every annotation; the bound's trees parse inside the declaring
//! `<...>` window, so the site and clause faces never saw them). The
//! clause contexts deny the super pass (`_shouldAllowSuperBoundedTypes`,
//! :737-748); a bound annotation's parent `TypeParameter` is not in the
//! deny list, so the super pass runs there.

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

/// The alias instantiation at an annotation site
/// (TypeArgumentNotMatchingBoundsTest/test_nonFunctionTypeAlias_
/// interfaceType_parameter): `X<String>` checks `String` against the
/// ALIAS's own `T extends A` and reports at the written argument.
#[test]
fn alias_annotation_site_bad_argument_reports_at_the_argument() {
    let source = r#"
class A {}
class B {}
typedef X<T extends A> = Map<int, T>;
void f(X<String> a) {}
"#;
    let report = report_ops(source);
    assert_eq!(
        bounds_rows(&report),
        [(
            at(source, "String> a"),
            6,
            "'String' doesn't conform to the bound 'A' of the type parameter 'T'.".to_string()
        )],
        "{:?}",
        report.diagnostics
    );
}

/// A satisfying alias argument stays silent, and an in-scope type
/// parameter argument declared with a bound suppresses (the official
/// subtype test succeeds through the parameter's own bound — the landed
/// under-report posture).
#[test]
fn alias_satisfied_and_in_scope_bounded_arguments_stay_silent() {
    let satisfied = report_ops(
        r#"
class A {}
typedef X<T extends A> = Map<int, T>;
void f(X<A> a) {}
"#,
    );
    assert!(
        bounds_rows(&satisfied).is_empty(),
        "{:?}",
        satisfied.diagnostics
    );
    let in_scope = report_ops(
        r#"
class A {}
typedef X<T extends A> = Map<int, T>;
void f<T extends A>(X<T> x) {}
"#,
    );
    assert!(
        bounds_rows(&in_scope).is_empty(),
        "{:?}",
        in_scope.diagnostics
    );
}

/// The alias instantiation at a clause site (`class Y extends X<B> {}`):
/// the clause context denies the super pass, so the regular violation
/// reports with no inverted-type context row.
#[test]
fn alias_clause_site_bad_argument_reports() {
    let source = r#"
class A {}
class B {}
typedef X<T extends A> = Map<int, T>;
class Y extends X<B> {}
"#;
    let report = report_ops(source);
    assert_eq!(
        bounds_rows(&report),
        [(
            at(source, "B> {}"),
            1,
            "'B' doesn't conform to the bound 'A' of the type parameter 'T'.".to_string()
        )],
        "{:?}",
        report.diagnostics
    );
}

/// The function-type alias instantiation
/// (TypeArgumentNotMatchingBoundsTest/test_ofFunctionTypeAlias): `F<B>`
/// against `F<T extends A>` reports at the written argument.
#[test]
fn function_type_alias_instantiation_reports() {
    let source = r#"
class A {}
class B {}
typedef F<T extends A> = void Function();
F<B> fff = (throw 0);
"#;
    let report = report_ops(source);
    assert_eq!(
        bounds_rows(&report),
        [(
            at(source, "B> fff"),
            1,
            "'B' doesn't conform to the bound 'A' of the type parameter 'T'.".to_string()
        )],
        "{:?}",
        report.diagnostics
    );
}

/// A written CONTAINER parameter bound checks its own arguments
/// (`class C<T extends G<B>>`): the bound tree parses inside the
/// declaring `<...>` window, which the use-site capture cannot see, so
/// this enumeration is the check's only route.
#[test]
fn container_bound_annotation_bad_argument_reports() {
    let source = r#"
class A {}
class B {}
class G<E extends A> {}
class C<T extends G<B>> {}
"#;
    let report = report_ops(source);
    assert_eq!(
        bounds_rows(&report),
        [(
            at(source, "B>> {}"),
            1,
            "'B' doesn't conform to the bound 'A' of the type parameter 'E'.".to_string()
        )],
        "{:?}",
        report.diagnostics
    );
}

/// The alias twin of the bound face (`typedef H<P extends G<B>> = ...;`):
/// the bound annotation's parent `TypeParameter` keeps the super pass
/// allowed, and the failing regular pass reports identically here (the
/// inverted type `G<B>` fails the same way — `B` is neither top nor
/// bottom, so the inversion moves nothing).
#[test]
fn alias_bound_annotation_bad_argument_reports() {
    let source = r#"
class A {}
class B {}
class G<E extends A> {}
typedef H<P extends G<B>> = void Function();
"#;
    let report = report_ops(source);
    assert_eq!(
        bounds_rows(&report),
        [(
            at(source, "B>> = void"),
            1,
            "'B' doesn't conform to the bound 'A' of the type parameter 'E'.".to_string()
        )],
        "{:?}",
        report.diagnostics
    );
}

/// A satisfied bound annotation stays silent.
#[test]
fn satisfied_bound_annotations_stay_silent() {
    let source = r#"
class A {}
class G<E extends A> {}
typedef H<P extends G<A>> = void Function();
class C<T extends G<A>> {}
"#;
    let report = report_ops(source);
    assert!(bounds_rows(&report).is_empty(), "{:?}", report.diagnostics);
}
