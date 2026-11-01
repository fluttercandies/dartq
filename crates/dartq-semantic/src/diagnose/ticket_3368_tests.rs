//! The #3368 pins: the foreign function-typedef fold inside
//! [`Engine::assignable`] and the callable-class → `Function` arm.
//!
//! Oracle semantics (dart 3.13.7, the pinned conformance oracle): a
//! `TypeAliasElement` use resolves through its aliased type at annotation
//! resolution (`named_type_resolver.dart` `_instantiateElement` →
//! `TypeAliasElementImpl.instantiateImpl`), so `subtype.dart` decides the
//! assignability ask over the EXPANDED pair — `F<int>` over `typedef
//! F<X> = void Function<Y extends X>();` reads `void Function<Y extends
//! int>()`. dartq's written-pair function-shape probe
//! (`function_shape_sig`) only reaches a same-file `BoundsIndex` capture,
//! so a foreign typedef folded AFTER the probe landed its `Ty::Function`
//! face too late — the `(Named, Named)` tail then suppressed every verdict
//! (`None`), and the conformance board's `Instantiate-to-bound/*` +
//! `Simple-bounds/*` buckets went silent on the `F<…>` pairs.
//!
//! The companion arm is upstream `isAssignableTo`'s callable-class
//! acceptance (type_system.dart:876-891): `getCallMethodType` substitutes
//! the receiver's own type arguments and `acceptsFunctionType` carries no
//! `args == EMPTY` gate, so an instantiated `C<int>` with a `call` member
//! decides by the method's substituted signature exactly like the bare
//! `C` does.
//!
//! Matrix pinned here: the foreign-alias verdict on both sides (source
//! and target), the callable-class `Function`/`Function?` target, and the
//! non-alias negatives.

use super::test_rig::{RigImport, report_ops, report_ops_imports};
use crate::diagnose::FileReport;

/// The `invalid_assignment` rows of `report`, as `(code-name, message)`
/// pairs in source order.
fn invalid_assignments(report: &FileReport) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "invalid_assignment")
        .map(|d| d.message.to_string())
        .collect()
}

/// The `argument_type_not_assignable` rows of `report`.
fn argument_rows(report: &FileReport) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "argument_type_not_assignable")
        .map(|d| d.message.to_string())
        .collect()
}

/// The co19 fixture's foreign alias face: `typedef F<X> = void
/// Function<Y extends X>();` lives in `other.dart`, the case file only
/// imports it — the file-local capture [`Engine::function_shape_sig`]
/// reads never sees it.
const FOREIGN_F: &str = "typedef F<X> = void Function<Y extends X>();\n";

/// A `void Function()` source against the foreign `F<int>` target — the
/// folded `F<int>` IS a function shape, so the verdict runs the function
/// rule: `void Function()` is NOT a `void Function<Y extends int>()`
/// (the generic target's parameter count arm refuses a non-generic
/// source — `relateTypeParameters` never relates a generic target to a
/// non-generic source). One row.
#[test]
fn foreign_fn_typedef_target_decides_the_assignment() {
    let report = report_ops_imports(
        "import 'package:t/other.dart';\n\
         void g() {}\n\
         void main() {\n\
         \x20 F<int> f = g;\n\
         }\n",
        &[RigImport {
            uri: "package:t/other.dart",
            prefix: None,
            source: FOREIGN_F,
        }],
    );
    let rows = invalid_assignments(&report);
    assert_eq!(
        rows.len(),
        1,
        "void Function() -> F<int> reports once: {rows:?}"
    );
    assert!(
        rows[0].contains("Function"),
        "the row renders the function face: {rows:?}"
    );
}

/// The reverse direction — an `F<int>`-typed VALUE against a `void
/// Function()` target: the folded source is a generic function shape,
/// the target a non-generic one. The same count arm refuses (a generic
/// source never subtypes a non-generic function target — the
/// `relateTypeParameters` gate), so one row reports. Before the reorder
/// this pair read `Named` on the source side and stayed silent.
#[test]
fn foreign_fn_typedef_source_decides_the_assignment() {
    let report = report_ops_imports(
        "import 'package:t/other.dart';\n\
         void main(F<int> g) {\n\
         \x20 void Function() f = g;\n\
         }\n",
        &[RigImport {
            uri: "package:t/other.dart",
            prefix: None,
            source: FOREIGN_F,
        }],
    );
    let rows = invalid_assignments(&report);
    assert_eq!(
        rows.len(),
        1,
        "F<int> -> void Function() reports once: {rows:?}; all: {:?}",
        report
            .diagnostics
            .iter()
            .map(|d| (d.code.name, d.message.to_string()))
            .collect::<Vec<_>>()
    );
}

/// The argument face of the same fold — `use(g)` where the callee's
/// formal is `F<int>` and the argument a `void Function()`: the folded
/// formal is a function shape and the pair reports
/// `argument_type_not_assignable`, the code the conformance rows carry
/// for the argument spelling.
#[test]
fn foreign_fn_typedef_formal_reports_argument_type() {
    let report = report_ops_imports(
        "import 'package:t/other.dart';\n\
         void take(F<int> f) {}\n\
         void g() {}\n\
         void main() {\n\
         \x20 take(g);\n\
         }\n",
        &[RigImport {
            uri: "package:t/other.dart",
            prefix: None,
            source: FOREIGN_F,
        }],
    );
    let rows = argument_rows(&report);
    assert_eq!(
        rows.len(),
        1,
        "use(void Function()) -> F<int> reports once: {rows:?}"
    );
}

/// The compatible twin — `F<int>` to `F<int>` assigns clean (the folded
/// pair is identical), and a `void Function<Y extends int>()` source
/// fits the same-shape target. Both directions silent: the reorder must
/// not newly report the pairs the oracle accepts.
#[test]
fn foreign_fn_typedef_identical_pair_stays_silent() {
    let report = report_ops_imports(
        "import 'package:t/other.dart';\n\
         F<int> mk() => throw 0;\n\
         void main() {\n\
         \x20 F<int> f = mk();\n\
         }\n",
        &[RigImport {
            uri: "package:t/other.dart",
            prefix: None,
            source: FOREIGN_F,
        }],
    );
    let rows = invalid_assignments(&report);
    assert_eq!(
        rows.len(),
        0,
        "F<int> -> F<int> stays silent: {rows:?}"
    );
}

/// The callable-class → `Function` arm: `void Function() g = C();` over
/// `class C { void call() {} }` — upstream's `callMethodType` accepts the
/// pair unconditionally once `call` resolves (`call` is a function-typed
/// member and `Function` its supertype). Silent.
#[test]
fn callable_class_into_function_target_stays_silent() {
    let report = report_ops("class C {\n\
         \x20 void call() {}\n\
         }\n\
         void main() {\n\
         \x20 void Function() g = C();\n\
         }\n");
    let rows = invalid_assignments(&report);
    assert_eq!(
        rows.len(),
        0,
        "C() -> void Function() stays silent: {rows:?}"
    );
}

/// The instantiated callable — `C<int>()` over `class C<T> { T call() {}
/// }` into `int Function()`: `getCallMethodType` substitutes `T := int`
/// and the method's own signature decides — `int call()` fits `int
/// Function()`, so silent. Before the arm learned the substitution the
/// instantiated source bailed `None` — an under-report upstream accepts
/// explicitly.
#[test]
fn instantiated_callable_matching_signature_stays_silent() {
    let report = report_ops("class C<T> {\n\
         \x20 T call() => throw 0;\n\
         }\n\
         void main() {\n\
         \x20 int Function() g = C<int>();\n\
         }\n");
    let rows = invalid_assignments(&report);
    assert_eq!(
        rows.len(),
        0,
        "C<int>() -> int Function() stays silent: {rows:?}"
    );
}

/// The instantiated callable that fails — `C<int>()` into `String
/// Function()` over `class C<T> { T call() }`: the substituted `int
/// call()` does not fit `String Function()` and the verdict reports,
/// exactly as the bare-head twin does.
#[test]
fn instantiated_callable_mismatched_signature_reports() {
    let report = report_ops("class C<T> {\n\
         \x20 T call() => throw 0;\n\
         }\n\
         void main() {\n\
         \x20 String Function() g = C<int>();\n\
         }\n");
    let rows = invalid_assignments(&report);
    assert_eq!(
        rows.len(),
        1,
        "C<int>() -> String Function() reports: {rows:?}"
    );
}

/// The negative guard: a genuinely-incompatible assignment of a
/// non-function pair still reports through the ordinary named lattice —
/// `int x = "s";` keeps its row, untouched by the fold reorder or the
/// callable arm.
#[test]
fn non_function_pair_still_reports() {
    let report = report_ops("void main() {\n\
         \x20 int x = \"s\";\n\
         }\n");
    let rows = invalid_assignments(&report);
    assert_eq!(rows.len(), 1, "int = String reports: {rows:?}");
}
