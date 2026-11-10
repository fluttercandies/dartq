//! The #3061 pins — the argument-type over-emission family: the await
//! flatten on argument rows, the closure literal's downward return
//! inference, the built-in `Map<K, V> <: Iterable<E>` argument relation,
//! and the if-null assignment's own write face.
//!
//! Four inference gaps upstream of the argument verdict made dartq reject
//! assignments the oracle accepts (`resolver.dart:519-536`
//! `checkForArgumentTypesNotAssignableInList` reads each argument's
//! resolved `staticType`, which the oracle computes with the await
//! flatten, the downward closure context, the Map-as-Iterable built-in,
//! and the if-null `LUB(NonNull(T1), T2)` nodeType):
//!
//! - an `await` operand's argument row kept the raw `Future<T>` face —
//!   the on-demand send/member read (`expression_row_ty`) bypassed the
//!   close-time flatten (`analyzeAwaitExpression`, type_analyzer.dart
//!   :542-548 — the static type of `await e` IS `flatten(e)`);
//! - a closure literal's captured signature carries a `void` return stub
//!   (`closure_capture_end` hardcodes it), so `g(() => 5)` against
//!   `int Function()` reported the stub — the formal's return is the
//!   downward context the literal re-infers under (`inferLocalFunction`'s
//!   cap `S = T if T <: R else R`);
//! - `Map` declares no supertype clause in the SDK source, so the
//!   declared-edge walk saw no `Iterable` hop and reported
//!   `Map<dynamic,dynamic> => Iterable<Object?>` where the language's
//!   built-in relation (element-wise `MapEntry<K, V> <: E`) accepts;
//! - `x ??= e` staged the bare RHS type as the flow write face — the
//!   oracle records `LUB(NonNull(T1), T2)`
//!   (assignment_expression_resolver.dart `_resolveTypes`).

use super::test_rig::{RigImport, report_ops, report_ops_with};
use super::*;

fn codes(folded: &FileReport) -> Vec<&str> {
    folded
        .diagnostics
        .iter()
        .map(|diag| diag.code.name)
        .collect()
}

/// `f(await c.m())` offers the flattened operand type: the argument ask
/// reads `int` (`flatten(Future<int>)`), which assigns into the `int`
/// formal. The fixture declares its own `Future` head so the unflattened
/// `Future<int>` face is a decidable rejection (the declaration lives
/// outside the `dart:` libraries, `declared_outside_core`).
#[test]
fn await_operand_argument_row_flattens() {
    let folded = report_ops(
        "class Future<T> {}\n\
         class C { Future<int> m(); }\n\
         void f(int x) {}\n\
         void g(C c) async {\n\
           f(await c.m());\n\
         }\n",
    );
    assert_eq!(
        codes(&folded),
        Vec::<&str>::new(),
        "`f(await c.m())` asks the flattened `int` — silent"
    );
}

/// The negative control: the same send WITHOUT the await keeps the raw
/// `Future<int>` face and reports — the flatten must ride the await
/// extent, never the send itself.
#[test]
fn unawaited_future_argument_reports() {
    let folded = report_ops(
        "class Future<T> {}\n\
         class C { Future<int> m(); }\n\
         void f(int x) {}\n\
         void g(C c) {\n\
           f(c.m());\n\
         }\n",
    );
    assert_eq!(
        codes(&folded),
        vec!["argument_type_not_assignable"],
        "`f(c.m())` offers `Future<int>` against `int`"
    );
}

/// `f(await fut)` on a bare read — the parse-time flatten already types
/// the row; the argument ask stays silent either way (the stored row
/// carries the flattened face idempotently).
#[test]
fn await_bare_read_argument_stays_silent() {
    let folded = report_ops(
        "class Future<T> {}\n\
         void f(int x) {}\n\
         void g(Future<int> fut) async {\n\
           f(await fut);\n\
         }\n",
    );
    assert_eq!(
        codes(&folded),
        Vec::<&str>::new(),
        "`f(await fut)` was silent pre-fix and must stay silent"
    );
}

/// `h(() => 5)` against `int Function()` — the closure literal re-infers
/// its return from the formal's (`inferLocalFunction`'s cap), so the
/// `void`-stub signature never reports.
#[test]
fn closure_context_return_stays_silent() {
    let folded = report_ops(
        "void h(int Function() cb) {}\n\
         void k() {\n\
           h(() => 5);\n\
         }\n",
    );
    assert_eq!(
        codes(&folded),
        Vec::<&str>::new(),
        "`h(() => 5)` infers the context return `int` — silent"
    );
}

/// The still-reports path: a written parameter the context cannot retype
/// (`(String x) => 5` against `int Function()`) keeps the rejection, and
/// the message renders the context-typed literal (`int Function(String)`,
/// the cap's shape) — the oracle's resolved FunctionExpression type.
#[test]
fn closure_written_parameter_mismatch_reports() {
    let folded = report_ops(
        "void h(int Function() cb) {}\n\
         void k() {\n\
           h((String x) => 5);\n\
         }\n",
    );
    assert_eq!(
        codes(&folded),
        vec!["argument_type_not_assignable"],
        "the written `String` formal keeps the literal's rejection"
    );
    assert!(
        folded.diagnostics[0]
            .message
            .as_str()
            .contains("int Function(String)"),
        "the message renders the context-typed literal shape, got: {}",
        folded.diagnostics[0].message
    );
}

/// The core Map argument assigns into an `Iterable` formal — the
/// language's built-in `Map<K, V> <: Iterable<E>` relation. The fixture
/// keeps `Map`/`Iterable`/`MapEntry` out of the analyzed unit (an
/// imported stand-in library) so the core face is not shadowed.
#[test]
fn map_argument_assigns_to_iterable() {
    let folded = report_ops_with(
        "void f(Iterable<Object?> items) {}\n\
         void g(Map m) {\n\
           f(m);\n\
         }\n",
        &[RigImport {
            uri: "package:t/core.dart",
            prefix: None,
            source: "class Iterable<E> {}\n class MapEntry<K, V> {}\n class Map<K, V> {}\n",
        }],
        None,
    );
    assert_eq!(
        codes(&folded),
        Vec::<&str>::new(),
        "`Map<dynamic, dynamic> <: Iterable<Object?>` via `MapEntry<dynamic, dynamic> <: Object?`"
    );
}

/// The element relation decides against: `MapEntry<dynamic, dynamic>` is
/// no `String`, so the tight `Iterable<String>` formal keeps its
/// rejection — the face never blankets the pair.
#[test]
fn map_argument_against_tight_element_reports() {
    let folded = report_ops_with(
        "void f(Iterable<String> items) {}\n\
         void g(Map m) {\n\
           f(m);\n\
         }\n",
        &[RigImport {
            uri: "package:t/core.dart",
            prefix: None,
            source: "class String {}\n class Iterable<E> {}\n class MapEntry<K, V> {}\n class Map<K, V> {}\n",
        }],
        None,
    );
    assert_eq!(
        codes(&folded),
        vec!["argument_type_not_assignable"],
        "`MapEntry<dynamic, dynamic> <: String` is false — the oracle reports"
    );
}

/// A same-file `Map` declaration shadows the core face: the fixture's own
/// edges decide, and a clause-less local `Map` stays a rejection.
#[test]
fn local_map_declaration_keeps_declared_edges() {
    let folded = report_ops_with(
        "void f(Iterable<Object?> items) {}\n\
         class Map<K, V> {}\n\
         void g(Map m) {\n\
           f(m);\n\
         }\n",
        &[RigImport {
            uri: "package:t/core.dart",
            prefix: None,
            source: "class Iterable<E> {}\n",
        }],
        None,
    );
    assert_eq!(
        codes(&folded),
        vec!["argument_type_not_assignable"],
        "the file's own `Map` has no `Iterable` edge — the built-in must not fire"
    );
}

/// `y ??= null` writes the declared face (`LUB(NonNull(num?), Null)` =
/// `num?`): the later `f(y)` asks `num?` against `num` and reports the
/// nullable face — the oracle's `num? => num`, never a `Null => num`
/// residue from the bare RHS type.
#[test]
fn ifnull_null_rhs_write_face_is_declared_form() {
    let folded = report_ops(
        "void f(num x) {}\n\
         void g(num? y) {\n\
           y ??= null;\n\
           f(y);\n\
         }\n",
    );
    assert_eq!(
        codes(&folded),
        vec!["argument_type_not_assignable"],
        "`f(y)` after the if-null write offers `num?` — the assignment's own static type"
    );
    assert!(
        folded.diagnostics[0].message.as_str().contains("'num?'"),
        "the message renders the declared `num?` write face, got: {}",
        folded.diagnostics[0].message
    );
}

/// The non-nullifying twin: `y ??= 0` joins `NonNull(num?)` with `int`
/// into `num`, and `f(y)` stays silent — the write promoted, exactly the
/// oracle's nodeType face.
#[test]
fn ifnull_non_null_rhs_write_promotes() {
    let folded = report_ops(
        "void f(num x) {}\n\
         void g(num? y) {\n\
           y ??= 0;\n\
           f(y);\n\
         }\n",
    );
    assert_eq!(
        codes(&folded),
        Vec::<&str>::new(),
        "`y ??= 0` writes `num` — `f(y)` accepts"
    );
}
