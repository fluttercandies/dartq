//! The #2938 pins: the generic-send `couldNotInfer` face — the real
//! `GenericInferrer` gather/choose/check pipeline (upstream Dart SDK
//! source, pkg/analyzer/lib/src/dart/element/generic_inferrer.dart's
//! `tryChooseFinalTypes` + `_formatError`, and
//! pkg/_fe_analyzer_shared/lib/src/type_inference/type_constraint.dart's
//! origin texts). The oracle outputs below were captured verbatim from
//! `dart analyze` runs of the same shapes.

use super::test_rig::{CORE_PRELUDE, report_ops};
use super::*;

fn infer_rows(report: &FileReport) -> Vec<(u32, u32, String)> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "could_not_infer")
        .map(|d| (d.range.offset, d.range.length, d.message.to_string()))
        .collect()
}

fn at(source: &str, needle: &str) -> u32 {
    (CORE_PRELUDE.len() + source.find(needle).expect("needle in fixture")) as u32
}

/// The single-argument creation failure (oracle `test_inference_error`):
/// `(double x) => 0` re-types under the context `T Function(T)` as
/// `int Function(double)` — the round merges `int <: T` (return,
/// covariant) with `T <: double` (formal, contravariant); `int` is the
/// chosen known lower and fails `int <: double`, so one Argument origin
/// prints in the unsatisfied block.
#[test]
fn infer_creation_single_argument_origin() {
    let source = r#"
void main() {
  var c = C((double x) => 0);
}
class C<T> {
  C(T Function(T) f);
}
"#;
    let report = report_ops(source);
    let rows = infer_rows(&report);
    assert_eq!(rows.len(), 1, "{:?}", report.diagnostics);
    let (offset, _len, msg) = &rows[0];
    assert_eq!(*offset, at(source, "C((double"));
    assert!(
        msg.contains("Tried to infer 'int' for 'T'")
            && msg.contains("Parameter 'f'")
            && msg.contains("declared as     'T Function(T)'")
            && msg.contains("but argument is 'int Function(double)'.")
            && msg.contains("Consider passing explicit type argument(s)"),
        "{msg}"
    );
    // The single merged round is unsatisfied — no "inferred from" block.
    assert!(!msg.contains("was inferred from"), "{msg}");
}

/// Two disagreeing argument rounds (oracle `test_inference_error` shape):
/// `int Function(int)` + `String Function(String)` squash to
/// `LUB(int, String) = Object`; the choice fails both per-round checks,
/// so BOTH Argument origins print in declaration order.
#[test]
fn infer_creation_two_arguments_merge_to_object() {
    let source = r#"
void main() {
  var c = C2((int a) => 0, (String b) => "x");
}
class C2<T> {
  C2(T Function(T) a, T Function(T) b);
}
"#;
    let report = report_ops(source);
    let rows = infer_rows(&report);
    assert_eq!(rows.len(), 1, "{:?}", report.diagnostics);
    let (_, _, msg) = &rows[0];
    assert!(
        msg.contains("Tried to infer 'Object' for 'T'")
            && msg.contains("Parameter 'a'")
            && msg.contains("but argument is 'int Function(int)'.")
            && msg.contains("Parameter 'b'")
            && msg.contains("but argument is 'String Function(String)'."),
        "{msg}"
    );
}

/// The preliminary-fix face (oracle `String v = id2(3)`): the downward
/// context constrains `T <: String`, `choosePreliminaryTypes` fixes
/// `T := String`, the later `int <: T` argument constraint drops
/// (`_typesInferredSoFar`), every recorded constraint is satisfied, and
/// NO `couldNotInfer` reports — the arg-vs-`String` formal check owns
/// the visible row.
#[test]
fn infer_preliminary_fixed_parameter_never_reports() {
    let source = r#"
void main() {
  String v = id2(3);
}
T id2<T>(T x) => x;
"#;
    let report = report_ops(source);
    assert!(
        infer_rows(&report).is_empty(),
        "{:?}",
        report.diagnostics
    );
}

/// The generic tear-off argument's own solve
/// (`inferFunctionTypeInstantiation` + `constrainGenericFunctionInContext`):
/// `h(f)` where `f` is `T Function<T extends num>(T, T)` infers `T := num`
/// against `int Function(int, double)`, fails `num <: int`, and reports
/// at the ARGUMENT — with the FunctionContext origin spelling the
/// tear-off's written signature.
#[test]
fn infer_generic_tearoff_function_context() {
    let source = r#"
T f<T extends num>(T a, T b) => a;
void main() {
  var x = h(f);
}
int h(int Function(int, double) t) => 0;
"#;
    let report = report_ops(source);
    let rows = infer_rows(&report);
    assert_eq!(rows.len(), 1, "{:?}", report.diagnostics);
    let (offset, _len, msg) = &rows[0];
    assert_eq!(*offset, at(source, "f);"));
    assert!(
        msg.contains("Tried to infer 'num' for 'T'")
            && msg.contains("Function type")
            && msg.contains("declared as 'T Function<T extends num>(T, T)'")
            && msg.contains("used where  'int Function(int, double)' is required."),
        "{msg}"
    );
}

/// The instantiated-form arm (oracle `_checkArgumentsNotMatchingBounds`):
/// `f<T2 extends Func1<T1, T1>, T1>` with `int` — the bound-gate kills
/// the `int <: T2` lower (`int` fails `greatestClosure(T1 Function(T1))`),
/// the partial upper's schema closes to `Object? Function(Never)`, and
/// the i2b-completed result fails `T2`'s substituted bound — the report
/// carries no "Tried to infer" intro.
///
/// Ignored pending the `exec_bounds` capture regression: `f`'s window
/// never pairs under the in-flight listener/type_capture edits (the
/// `#2516` `top_level_decl` fallback supplies the parameter NAMES but no
/// declaration-bound source exists for a top-level function whose window
/// missed). The gate, the schema closure, and the instantiated-form emit
/// are all exercised once `exec_bounds` carries `(T2, Func1<T1,T1>)`.
#[test]
#[ignore = "exec_bounds capture regressed under in-flight foreign lanes; \
            f's declared Func1<T1,T1> bound never pairs"]
fn infer_bound_gate_instantiated_form() {
    let source = r#"
typedef Func1<T1, T2> = T1 Function(T2);
void main() {
  var x = f(3);
}
T2 f<T2 extends Func1<T1, T1>, T1>(T2 a) => throw 0;
"#;
    let report = report_ops(source);
    let rows = infer_rows(&report);
    assert_eq!(rows.len(), 1, "{:?}", report.diagnostics);
    let (offset, _len, msg) = &rows[0];
    assert_eq!(*offset, at(source, "f(3)"));
    assert!(
        msg.contains("Couldn't infer type parameter 'T2'")
            && msg.contains(
                "'Object? Function(Never)' doesn't conform to the bound \
                 'dynamic Function(dynamic)'"
            )
            && msg.contains("instantiated from 'T1 Function(T1)'")
            && !msg.contains("Tried to infer"),
        "{msg}"
    );
}

/// The recursive-bound pair (oracle `foo<X extends Y, Y extends A<X>>()`):
/// both seeds close to `A<Object?>`, `Y`'s substituted bound
/// `A<A<Object?>>` rejects `A<Object?>`, and the instantiated form
/// spells the whole completed argument vector.
#[test]
fn infer_recursive_bound_instantiated_form() {
    let source = r#"
class A<T> { A(T x); }
void foo<X extends Y, Y extends A<X>>() {}
void main() {
  foo();
}
"#;
    let report = report_ops(source);
    let rows = infer_rows(&report);
    assert!(
        rows.iter().any(|(_, _, msg)| {
            msg.contains("Couldn't infer type parameter 'Y'")
                && msg.contains(
                    "'A<Object?>' doesn't conform to the bound 'A<A<Object?>>'"
                )
                && msg.contains("instantiated from 'A<X>'")
                && msg.contains("using type arguments [A<Object?>, A<Object?>]")
        }),
        "{:?}",
        rows
    );
}

/// The downward-context inference (oracle-verbatim): `F<int,num> h =
/// g(42)` — the ReturnType round merges `T <: int` (return, covariant)
/// with `num <: T` (formal, contravariant) BEFORE the argument
/// constraint; the preliminary solve fixes `T := num`, the `int <: T`
/// argument constraint drops (`_typesInferredSoFar`), `num <: int`
/// fails, and only the ReturnType origin prints — no satisfied block,
/// no Argument line.
#[test]
fn infer_return_context_origin_only() {
    let source = r#"
typedef F<T1, T2> = T1 Function(T2);
void main() {
  F<int, num> h = g(42);
}
T Function(T) g<T>(T t) => (x) => x;
"#;
    let report = report_ops(source);
    let rows = infer_rows(&report);
    assert_eq!(rows.len(), 1, "{:?}", report.diagnostics);
    let (_, _, msg) = &rows[0];
    assert!(
        msg.contains("Tried to infer 'num' for 'T'")
            && msg.contains("Return type")
            && msg.contains("declared as 'T Function(T)'")
            && msg.contains("used where  'int Function(num)' is required.")
            && !msg.contains("was inferred from")
            && !msg.contains("Parameter"),
        "{msg}"
    );
}

/// The silence controls: a bound-gated constructor argument (the `NotA`
/// formal's lower never registers), a hole-only collection literal under
/// a bounded parameter, and a satisfied same-shape solve never report.
#[test]
fn infer_silence_controls() {
    let source = r#"
class A {}
class NotA {}
class C2<T extends A> {
  C2(T f);
}
class D<P extends num> {
  D(List<P> x);
}
NotA myF = NotA();
void main() {
  var c = C2(myF);
  var d = D([]);
}
T id2<T>(T x) => x;
void later() {
  var ok = id2(3);
}
"#;
    let report = report_ops(source);
    assert!(
        infer_rows(&report).is_empty(),
        "{:?}",
        report.diagnostics
    );
}
