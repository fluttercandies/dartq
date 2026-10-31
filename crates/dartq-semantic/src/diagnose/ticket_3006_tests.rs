//! The #3006 pins — the `missing_default_value_for_parameter` residuals:
//! the `;`-bodied constructor verdict and the type-parameter /
//! `FutureOr`-argument faces of `isPotentiallyNonNullable`.
//!
//! * `defaultValuesAreExpected` answers true for a `ConstructorDeclaration`
//!   regardless of the body shape — a `;`-bodied generative constructor
//!   still expects defaults on its optional parameters
//!   (error_verifier.dart:8330-8343); `isFactory && isRedirecting` is the
//!   only constructor exclusion, and an `= G;` redirect never reaches the
//!   empty-body event. A `;`-bodied `MethodDeclaration` stays silent —
//!   `isAbstract` answers false (:8357-8367).
//! * A bare type parameter `T` is potentially non-nullable — the declared
//!   element carries no `promotedBound` and `isNullable` never consults
//!   `bound`, so `T` answers non-nullable and `T?` answers nullable
//!   (type_system.dart:1216-1233, `isPotentiallyNonNullable` :1257).
//! * `FutureOr<T>` recurses its argument: `FutureOr<int>` is potentially
//!   non-nullable, `FutureOr<int?>` is nullable, a bare `FutureOr` reads
//!   `FutureOr<dynamic>` (type_system.dart:1227-1230).
//!
//! The #3015 pins — the `implicit_super_initializer_missing_arguments`
//! residual: `this`/`super` FORMALS never join the explicit-super set
//! (error_verifier.dart:7182-7189 early-returns on an initializer entry
//! alone); the formals' supplies subtract granularly instead
//! (`verifySuperFormalParameters`, :7244-7258).

use super::test_rig::{CORE_PRELUDE, report_ops};
use super::*;

/// The `missing_default_value_for_parameter` rows as the exact anchored
/// source text, source order.
fn mdv_spans(report: &FileReport, full: &str) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "missing_default_value_for_parameter")
        .map(|d| {
            full[d.range.offset as usize..(d.range.offset + d.range.length) as usize].to_string()
        })
        .collect()
}

/// The `implicit_super_initializer_missing_arguments` rows as the exact
/// anchored source text, source order.
fn super_init_spans(report: &FileReport, full: &str) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "implicit_super_initializer_missing_arguments")
        .map(|d| {
            full[d.range.offset as usize..(d.range.offset + d.range.length) as usize].to_string()
        })
        .collect()
}

/// Every diagnostic code name, source order.
fn code_names(report: &FileReport) -> Vec<&'static str> {
    report.diagnostics.iter().map(|d| d.code.name).collect()
}

/// A `;`-bodied generative constructor still expects defaults — the
/// `ConstructorDeclaration` arm of `defaultValuesAreExpected` answers true
/// unconditionally, where a `;`-bodied method answers `isAbstract` false.
#[test]
fn semicolon_constructor_still_expects_defaults() {
    let fixture = concat!(
        "class A { A([int x]); }\n",
        "class B { B({int y}); }\n",
        "class C { C([int? z]); }\n",
        "class D { void m([int w]); }\n",
        "class E { static void s([int v]); }\n",
    );
    let full = format!("{CORE_PRELUDE}{fixture}");
    let report = report_ops(fixture);
    assert_eq!(
        mdv_spans(&report, &full),
        vec!["x".to_string(), "y".to_string()],
        "the `;` ctors report; the nullable formal and the abstract methods stay silent"
    );
}

/// A block-bodied constructor reports the same rows — the body verdict
/// joins the `;` verdict on the `ConstructorDeclaration` arm.
#[test]
fn block_constructor_reports_same_rows() {
    let fixture = concat!(
        "class A { A([int x]) {} }\n",
        "class B { B.named({int y}) {} }\n",
        "class C { C([int x]) : assert(true); }\n",
    );
    let full = format!("{CORE_PRELUDE}{fixture}");
    let report = report_ops(fixture);
    assert_eq!(
        mdv_spans(&report, &full),
        vec!["x".to_string(), "y".to_string(), "x".to_string()]
    );
}

/// A stale constructor site cannot carry the default expectation onto a
/// sibling `;`-bodied method — `ctor_pending` closes with its own
/// constructor, so the abstract method after a `;` ctor stays silent.
#[test]
fn stale_ctor_site_does_not_leak_onto_methods() {
    let fixture = "class A { A([int x]); void m([int y]); }\n";
    let full = format!("{CORE_PRELUDE}{fixture}");
    let report = report_ops(fixture);
    assert_eq!(
        mdv_spans(&report, &full),
        vec!["x".to_string()],
        "only the ctor's formal reports — `m` is abstract"
    );
}

/// A non-redirecting factory's `;` body expects defaults — the
/// `ConstructorDeclaration` arm excludes only `isFactory && isRedirecting`
/// (and external/augmentation heads), so `factory F.make([int x]);`
/// reports while `factory G.make([int x]) = G;` never reaches the
/// empty-body event.
#[test]
fn semicolon_factory_expects_defaults() {
    let fixture = concat!(
        "class F { F(int x); factory F.make([int x]); }\n",
        "class G { G(int x); factory G.make([int x]) = G; }\n",
    );
    let full = format!("{CORE_PRELUDE}{fixture}");
    let report = report_ops(fixture);
    assert_eq!(
        mdv_spans(&report, &full),
        vec!["x".to_string()],
        "the `;` factory reports; the required-positionals and the redirecting factory stay silent"
    );
}

/// `external` heads stay silent on every arm — `external C([int x]);`
/// takes its defaults in the embedding, never from Dart
/// (`defaultValuesAreExpected`'s `isExternal` exclusion,
/// error_verifier.dart:8337).
#[test]
fn external_constructor_stays_silent() {
    let fixture = concat!(
        "class A { external A([int x]); }\n",
        "class B { external factory B.make([int y]); }\n",
    );
    let report = report_ops(fixture);
    assert!(
        mdv_spans(&report, &format!("{CORE_PRELUDE}{fixture}")).is_empty(),
        "external heads never expect defaults: {:?}",
        report
            .diagnostics
            .iter()
            .map(|d| d.code.name)
            .collect::<Vec<_>>()
    );
}

/// A bare type parameter `T` is potentially non-nullable — `isNullable(T)`
/// answers false without a `promotedBound` (`bound` is never consulted,
/// type_system.dart:1216-1233), so `T`'s optional formals report while
/// `T?`'s stay silent.
#[test]
fn bare_type_parameter_is_potentially_non_nullable() {
    let fixture = concat!(
        "class A<T> { void m([T x]) {} }\n",
        "class B<T> { void m([T? x]) {} }\n",
        "class C<T> { C([T x]); }\n",
        "void f<T>([T x]) {}\n",
        "void g<T>([T? x]) {}\n",
    );
    let full = format!("{CORE_PRELUDE}{fixture}");
    let report = report_ops(fixture);
    assert_eq!(
        mdv_spans(&report, &full),
        vec!["x".to_string(), "x".to_string(), "x".to_string()],
        "bare `T` reports on method, `;` ctor, and top-level arms; `T?` stays silent"
    );
}

/// `FutureOr<T>` recurses its argument — `FutureOr<int>` is potentially
/// non-nullable, `FutureOr<int?>` and a bare `FutureOr`
/// (`FutureOr<dynamic>`) are nullable, and `FutureOr<T>` over a bare type
/// parameter keeps the parameter's non-nullable answer
/// (type_system.dart:1227-1230).
#[test]
fn future_or_recurses_its_argument() {
    let fixture = concat!(
        "void a([FutureOr<int> x]) {}\n",
        "void b([FutureOr<int?> x]) {}\n",
        "void c([FutureOr x]) {}\n",
        "void d<T>([FutureOr<T> x]) {}\n",
        "void e<T>([FutureOr<T?> x]) {}\n",
        "void f([FutureOr<FutureOr<int>> x]) {}\n",
    );
    let full = format!("{CORE_PRELUDE}{fixture}");
    let report = report_ops(fixture);
    assert_eq!(
        mdv_spans(&report, &full),
        vec!["x".to_string(), "x".to_string(), "x".to_string()],
        "FutureOr<int>, FutureOr<T>, and nested FutureOr<int> report; the nullable arms stay silent"
    );
}

/// Record and function types decide on their own `?` — bare shapes are
/// potentially non-nullable, `?`-suffixed shapes are nullable
/// (type_system.dart:1216-1233).
#[test]
fn record_and_function_shapes_decide_on_their_suffix() {
    let fixture = concat!(
        "void a([(int, int) x]) {}\n",
        "void b([(int, int)? x]) {}\n",
        "void c([int Function() x]) {}\n",
        "void d([int Function()? x]) {}\n",
    );
    let full = format!("{CORE_PRELUDE}{fixture}");
    let report = report_ops(fixture);
    assert_eq!(
        mdv_spans(&report, &full),
        vec!["x".to_string(), "x".to_string()],
        "the bare record and function shapes report; the `?` faces stay silent"
    );
}

// —— #3015: super-formal supplies vs explicit-super satisfaction ————

/// A PARTIALLY supplied superclass still reports: `super.a` supplies only
/// the named `a`, the required `b` goes unsupplied — a super formal is not
/// an initializer entry, so the early return never fires
/// (error_verifier.dart:7182-7189, :7252-7258). The positional arm
/// mirrors it: one super formal against two required positionals.
#[test]
fn super_formal_partial_supply_still_reports() {
    let fixture = concat!(
        "class C { C({required int a, required int b}); }\n",
        "class D extends C { D({super.a}); }\n",
        "class S { S(int x, int y); }\n",
        "class T extends S { T(int super.x); }\n",
    );
    let full = format!("{CORE_PRELUDE}{fixture}");
    let report = report_ops(fixture);
    assert_eq!(
        super_init_spans(&report, &full),
        vec!["D".to_string(), "T".to_string()],
        "the under-supplied formals report at their constructor heads"
    );
}

/// A FULLY supplied superclass stays silent: every required named spelling
/// is covered by a same-spelling named super formal and the required
/// positionals are outnumbered by the positional super formals
/// (error_verifier.dart:7244-7258 — `removeAll(namedArgumentNames)` then
/// the count comparison).
#[test]
fn super_formal_full_supply_stays_silent() {
    let fixture = concat!(
        "class C { C({required int a, required int b}); }\n",
        "class D extends C { D({super.a, super.b}); }\n",
        "class S { S(int x, int y); }\n",
        "class T extends S { T(int super.x, int super.y); }\n",
        "class U { U({required int only}); }\n",
        "class V extends U { V({required super.only}); }\n",
    );
    let full = format!("{CORE_PRELUDE}{fixture}");
    let report = report_ops(fixture);
    assert_eq!(
        super_init_spans(&report, &full),
        Vec::<String>::new(),
        "fully-supplied forwarding constructors stay silent: {:?}",
        code_names(&report)
    );
}

/// An initializing `this.f` formal supplies nothing to the superclass —
/// it is neither a super formal nor an initializer entry, so the
/// implicit-super face fires beside it (error_verifier.dart:7182-7189;
/// `verifySuperFormalParameters` counts `SuperFormalParameterImpl` only).
#[test]
fn this_formal_never_satisfies_implicit_super() {
    let fixture = concat!(
        "class S { S(int a); }\n",
        "class D extends S { int f; D(this.f); }\n",
    );
    let full = format!("{CORE_PRELUDE}{fixture}");
    let report = report_ops(fixture);
    assert_eq!(
        super_init_spans(&report, &full),
        vec!["D".to_string()],
        "the this-formal neither satisfies nor supplies: {:?}",
        code_names(&report)
    );
}

/// An explicit `super(...)` initializer satisfies regardless of the
/// formals beside it — the early return precedes the counting
/// (error_verifier.dart:7182-7189).
#[test]
fn explicit_super_still_satisfies_beside_super_formals() {
    let fixture = concat!(
        "class C { C({required int a, required int b}); }\n",
        "class D extends C { D({super.a}) : super(a, 2); }\n",
    );
    let full = format!("{CORE_PRELUDE}{fixture}");
    let report = report_ops(fixture);
    assert_eq!(
        super_init_spans(&report, &full),
        Vec::<String>::new(),
        "the explicit super invocation satisfies: {:?}",
        code_names(&report)
    );
}

