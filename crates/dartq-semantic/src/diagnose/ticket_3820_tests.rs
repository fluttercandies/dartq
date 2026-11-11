//! The #3820 pins (census C2 lane C): the composite receiver's OWN static
//! type as a re-decided `type_argument_not_matching_bounds` candidate —
//! the context-free half of the oracle's composite typing
//! (`binary_expression_resolver.dart:214-216` the if-null `T` branch,
//! `assignment_expression_resolver.dart:84` the local-provider read,
//! `postfix_expression_resolver.dart:62` `node.readType`, upstream Dart
//! SDK source, pkg/analyzer/lib/src/dart/resolver/).
//!
//! Each pin mirrors one census shape from the gated
//! `census-20261104-092425` extras (the `Exactly<…>` bound-lattice
//! concentration): the `package:expect` static-type helper's exact
//! contract (`typedef Exactly<T> = T Function(T)` + the
//! `expectStaticType<R extends Exactly<T>>` extension) is inlined so the
//! extension application's bound substitution runs the corpus path.
//!
//! Fail-before evidence: the census itself — every shape pinned here is a
//! recorded extra row (dartq-only) at the cited census anchor; the pins
//! pass once the composite's own face reaches the verdict.

use super::test_rig::{report_ops, rows};

/// The `type_argument_not_matching_bounds` rows of `report` as
/// `offset|length|message`.
fn bounds_rows(report: &super::FileReport) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "type_argument_not_matching_bounds")
        .map(|d| format!("{}|{}|{}", d.range.offset, d.range.length, d.message))
        .collect()
}

/// The `package:expect/static_type_helper.dart` contract the corpus pins
/// against, verbatim shapes.
const HELPERS: &str = "typedef Exactly<T> = T Function(T);\n\
     extension StaticType<T> on T {\n\
       T expectStaticType<R extends Exactly<T>>() => this;\n\
     }\n";

/// The shared implement hierarchy of the if-null local-join corpus pins
/// (`if_null_assignment_local_test.dart`'s declarations).
const HIERARCHY: &str = "class A {}\n\
     class B1<T> implements A {}\n\
     class B2<T> implements A {}\n\
     class C1<T> implements B1<T>, B2<T> {}\n\
     class C2<T> implements B1<T>, B2<T> {}\n";

/// A promoted-left `??=` join reports the composite's own `UP` face —
/// `if_null_assignment_local_test.dart:104-110` (`Iterable<int>?` local
/// promoted under `is`, joined with `Iterable<double>`): the oracle
/// stamps the composite `Iterable<num>` (`T = UP(NonNull(T1), T2')`, the
/// `T <: S` branch — `binary_expression_resolver.dart:214-216`), the
/// written `Exactly<Iterable<num>>` then satisfies the bound derived
/// from that face, and the census row (`108:55`, `'Exactly<Iterable<num>>'
/// doesn't conform to the bound 'Exactly<Object>'`) disappears.
#[test]
fn if_null_local_join_reports_composite_up_face() {
    let report = report_ops(&format!(
        "{HELPERS}{HIERARCHY}\
         Object? contextIterable<T>(Iterable<T> x) => x;\n\
         f() {{\n\
         \x20 Iterable<int>? local1;\n\
         \x20 var iterableDouble = <double>[] as Iterable<double>;\n\
         \x20 if (local1 is Iterable<int>?) {{\n\
         \x20   contextIterable(\n\
         \x20     (local1 ??= iterableDouble)..expectStaticType<Exactly<Iterable<num>>>(),\n\
         \x20   );\n\
         \x20 }}\n\
         }}\n"
    ));
    assert!(bounds_rows(&report).is_empty(), "{:#?}", rows(&report));
}

/// The S-branch companion (`if_null_assignment_local_test.dart:166-174`):
/// the composite's context-free `UP` is `A`, but the oracle stamps `S =
/// B1<Object?>` (`T <!: S`, both parts `<: S`) — the context-pushed arm
/// answers the row, and the composite's own face must not displace it.
/// The census row (`170:49`, `'Exactly<B1<Object?>>' doesn't conform to
/// the bound 'Exactly<Object>'`) disappears through the context face.
#[test]
fn if_null_local_s_branch_still_reports_context_face() {
    let report = report_ops(&format!(
        "{HELPERS}{HIERARCHY}\
         Object? contextB1<T>(B1<T> x) => x;\n\
         f() {{\n\
         \x20 Object? local1;\n\
         \x20 var c2Double = C2<double>();\n\
         \x20 if (local1 is C1<int>?) {{\n\
         \x20   contextB1(\n\
         \x20     (local1 ??= c2Double)..expectStaticType<Exactly<B1<Object?>>>(),\n\
         \x20   );\n\
         \x20 }}\n\
         }}\n"
    ));
    assert!(bounds_rows(&report).is_empty(), "{:#?}", rows(&report));
}

/// A compound assignment's composite type is the operand's read face —
/// `number_operator_typing_test.dart` `(d += i1)` over `double d, int i1`
/// reports `double` (`assignment_expression_resolver.dart:84` the
/// local-provider read; the census row anchors `237:30`).
/// The composite's own face then satisfies `Exactly<double>`.
#[test]
fn compound_assignment_read_face_drops_the_row() {
    let report = report_ops(&format!(
        "{HELPERS}\
         f() {{\n\
         \x20 double d = 1.0;\n\
         \x20 int i1 = 1;\n\
         \x20 (d += i1).expectStaticType<Exactly<double>>();\n\
         }}\n"
    ));
    assert!(bounds_rows(&report).is_empty(), "{:#?}", rows(&report));
}

/// The same composite shape with a genuinely violating argument still
/// reports — the drop is verdict-based, not emission absence: the read
/// face `double` derives the bound `Exactly<double>`, and
/// `Exactly<String>` fails it.
#[test]
fn compound_assignment_real_violation_still_reports() {
    let report = report_ops(&format!(
        "{HELPERS}\
         f() {{\n\
         \x20 double d = 1.0;\n\
         \x20 int i1 = 1;\n\
         \x20 (d += i1).expectStaticType<Exactly<String>>();\n\
         }}\n"
    ));
    let hits = bounds_rows(&report);
    assert_eq!(hits.len(), 1, "{:#?}", rows(&report));
    assert!(
        hits[0].ends_with(
            "'Exactly<String>' doesn't conform to the bound 'Exactly<double>' \
             of the type parameter 'R'."
        ),
        "{hits:#?}"
    );
}

/// Prefix and postfix `++` report the operand's read face —
/// `number_operator_typing_test.dart` `(i++)`/`(++i)` over `int i`
/// (`postfix_expression_resolver.dart:62` `node.readType`; census rows
/// anchor the same block).
#[test]
fn inc_dec_read_face_drops_the_row() {
    let report = report_ops(&format!(
        "{HELPERS}\
         f() {{\n\
         \x20 int i = 0;\n\
         \x20 (i++).expectStaticType<Exactly<int>>();\n\
         \x20 (++i).expectStaticType<Exactly<int>>();\n\
         }}\n"
    ));
    assert!(bounds_rows(&report).is_empty(), "{:#?}", rows(&report));
}

/// A conditional expression reports `LUB(then, else)` —
/// `null_shorting_in_conditional_test.dart:18` `(b ? c?.d : d)` over
/// `C? c, D d` reports `D?` (the null-shorted then-face joined with the
/// else-face; census row anchors `18:35`, `'Exactly<D?>' doesn't
/// conform to the bound 'Exactly<D>'`).
#[test]
fn conditional_lub_face_drops_the_row() {
    let report = report_ops(&format!(
        "{HELPERS}\
         class D {{}}\n\
         class C {{ D? d; }}\n\
         f(bool b, C? c, D d) {{\n\
         \x20 (b ? c?.d : d).expectStaticType<Exactly<D?>>();\n\
         }}\n"
    ));
    assert!(bounds_rows(&report).is_empty(), "{:#?}", rows(&report));
}

/// A bare `??=` whose join face disagrees with the written argument
/// still reports — the composite arm answers `UP(NonNull(int), double) =
/// num`, and `Exactly<String>` fails the derived bound. The keep
/// direction the whole arm hangs on.
#[test]
fn if_null_composite_real_violation_still_reports() {
    let report = report_ops(&format!(
        "{HELPERS}\
         f() {{\n\
         \x20 int? x;\n\
         \x20 double d = 1.0;\n\
         \x20 (x ??= d).expectStaticType<Exactly<String>>();\n\
         }}\n"
    ));
    let hits = bounds_rows(&report);
    assert_eq!(hits.len(), 1, "{:#?}", rows(&report));
    assert!(
        hits[0].ends_with(
            "'Exactly<String>' doesn't conform to the bound 'Exactly<num>' \
             of the type parameter 'R'."
        ),
        "{hits:#?}"
    );
}

/// A plain (non-composite) real violation still reports — the control
/// every composite candidate hangs off: an unjoinable receiver face
/// keeps the emit-side row untouched.
#[test]
fn plain_local_real_violation_still_reports() {
    let report = report_ops(&format!(
        "{HELPERS}\
         f() {{\n\
         \x20 int i = 0;\n\
         \x20 i.expectStaticType<Exactly<String>>();\n\
         }}\n"
    ));
    let hits = bounds_rows(&report);
    assert_eq!(hits.len(), 1, "{:#?}", rows(&report));
    assert!(
        hits[0].ends_with(
            "'Exactly<String>' doesn't conform to the bound 'Exactly<int>' \
             of the type parameter 'R'."
        ),
        "{hits:#?}"
    );
}
