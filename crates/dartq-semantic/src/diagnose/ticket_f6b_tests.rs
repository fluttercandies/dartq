//! #F6-B pins — the call-site inference residual inside
//! [`Engine::instantiate_call_return`]. When a callee's type parameter is
//! still free after the send-close upward solve, the explicit type
//! arguments, and the receiver seeds, the oracle's
//! `generic_inferrer.dart` falls back to the parameter's declared bound
//! (`_inferer.typeVariableForSubtype` reads `param.bound`) — the
//! instantiate-to-bounds face. dartq used to emit `Ty::UNKNOWN` for that
//! slot, which downstream rendered as `dynamic` and over-reported
//! `invalid_assignment`/`argument_type_not_assignable` mismatches
//! against generic contexts (`C<dynamic>` vs `C<int>`).
//!
//! The change: bind every still-free callee parameter to its declared
//! bound before the free-parameter check runs. `T f<T extends num>(T x)`
//! with no constraint answers `num` upstream; `T f<T>(T x)` unbounded
//! keeps `dynamic` — the same faces the oracle's instantiate-to-bounds
//! resolves. Explicit type arguments still outrank inference, and a
//! caller-scope `T` (a type parameter in scope at the call site) still
//! names itself legitimately — the `type_params`/`pending_type_params`
//! guard keeps that spelling.
//!
//! Pins below cover (a) the upward-solved slot still wins over the
//! bound fallback, (b) the bound fallback fires for unbound params, (c)
//! the unbounded case still produces `dynamic`, (d) a param bound that
//! itself references a callee param substitutes through the same map
//! (the `T extends List<S>` chained-bound shape), and (e) explicit type
//! arguments keep their spelling over any inference result.

use super::test_rig::report_ops;
use crate::FileReport;

/// The offsets of one code's rows, ascending.
fn offsets(report: &FileReport, code: &str) -> Vec<u32> {
    let mut at = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == code)
        .map(|d| d.range.offset)
        .collect::<Vec<_>>();
    at.sort_unstable();
    at
}

/// The upward-solved parameter still wins over the bound fallback:
/// `int`-typed argument into `T f<T extends num>(T x)` binds `T := int`
/// (the send-close upward solve answers through `solved_call_params`),
/// so the return's instantiated type is `int` — never the `num` bound
/// fallback and never `dynamic`.
#[test]
fn upward_solve_beats_bound_fallback() {
    // `f` returns `T`; an `int` argument should instantiate `T := int`.
    // The `var x = f(0);` declarator then types `x` as `int`. A `String`
    // assignment on the next line produces one `invalid_assignment`.
    let source = "\
T f<T extends num>(T x) => x;
void main() {
  var x = f(0);
  String s = x;
}
";
    let report = report_ops(source);
    // If `x` were `dynamic`/`num`, the `String s = x;` line would not
    // produce a row; if `x` is `int` (the oracle's answer), the
    // `invalid_assignment` fires. The upward solve must win over the
    // `num` bound fallback.
    let rows = offsets(&report, "invalid_assignment");
    assert_eq!(
        rows.len(),
        1,
        "expected one invalid_assignment (int -> String), got {:?}",
        report.diagnostics
    );
}

/// An unbound callee parameter falls to its declared bound — never
/// `Unknown`. `g<T extends num>(int x)` returns `T`; `var y = g(0);`
/// has no constraint on `T`, so the oracle answers `num` (the declared
/// bound). A `String` assign on the next line fires `invalid_assignment`
/// against `num` — the same row an `Unknown`/`dynamic` answer would
/// suppress.
#[test]
fn unbound_param_falls_to_declared_bound() {
    let source = "\
T g<T extends num>(int x) => throw 0;
void main() {
  var y = g(0);
  String s = y;
}
";
    let report = report_ops(source);
    let rows = offsets(&report, "invalid_assignment");
    assert_eq!(
        rows.len(),
        1,
        "expected one invalid_assignment (num -> String), got {:?}",
        report.diagnostics
    );
}

/// A callee param with no declared bound resolves to `dynamic` — the
/// oracle's instantiate-to-bounds face over an unbounded `T`. `h<T>(int
/// x)` returns `T`; `var z = h(0);` carries no constraint, so the oracle
/// types `z` as `dynamic` and `String s = z;` stays silent.
#[test]
fn unbounded_param_resolves_to_dynamic() {
    let source = "\
T h<T>(int x) => throw 0;
void main() {
  var z = h(0);
  String s = z;
}
";
    let report = report_ops(source);
    let rows = offsets(&report, "invalid_assignment");
    assert_eq!(
        rows.len(),
        0,
        "dynamic -> String should not fire, got {:?}",
        report.diagnostics
    );
}

/// Explicit type arguments outrank inference: `f<String>(0)` instantiates
/// `T := String` even when the argument would solve `T := int`. The
/// explicit group's `ordered` slot writes first; the bound fallback
/// never sees the slot.
#[test]
fn explicit_type_argument_wins_over_inference_and_bound() {
    let source = "\
T f<T extends num>(T x) => x;
void main() {
  var x = f<num>(0);
  String s = x;
}
";
    let report = report_ops(source);
    // `x` is `num` (explicit `<num>` wins over the inferred `int` and
    // the bound fallback). `String s = x;` fires `invalid_assignment`
    // for `num` → `String`.
    let rows = offsets(&report, "invalid_assignment");
    assert_eq!(
        rows.len(),
        1,
        "expected one invalid_assignment (num -> String), got {:?}",
        report.diagnostics
    );
}

/// A callee param bound that itself mentions a callee param substitutes
/// through the same `keys` map: `List<T> g<S, T extends List<S>>(S x)`
/// over `g(0)` binds `S := int` upward, then the bound-fallback writes
/// `T := List<int>` (the bound's `S` reads the same map). The result
/// `List<int>` flows into the declarator's static type and the
/// `String s = y;` line fires.
#[test]
fn chained_param_bound_substitutes_through_keys() {
    let source = "\
List<T> g<S, T extends List<S>>(S x) => throw 0;
void main() {
  var y = g(0);
  String s = y;
}
";
    let report = report_ops(source);
    // `y` is `List<int>` — `S := int` (upward solve), `T := List<int>`
    // (bound fallback with `S` substituted). `String s = List<int>`
    // fires `invalid_assignment`.
    let rows = offsets(&report, "invalid_assignment");
    assert_eq!(
        rows.len(),
        1,
        "expected one invalid_assignment (List<int> -> String), got {:?}",
        report.diagnostics
    );
}

/// A param in scope at the call site keeps its own spelling — `E` inside
/// `class Box<E>` calling its own `T get<T>(T x)` with an `E`-typed
/// argument binds `T := E`, not to `T`'s bound. The `type_params`
/// guard keeps the in-scope `E` from being rebound to `dynamic`.
#[test]
fn caller_scope_param_names_itself() {
    let source = "\
class Box<E> {
  T get<T>(T x) => x;
  E pick() {
    var r = get(pickE());
    return r;
  }
  E pickE() => throw 0;
}
";
    let report = report_ops(source);
    // `r` should be `E` — not `dynamic`, not the bound (there is none),
    // not `Unknown`. The `return r;` face then matches `E` return type.
    // The pin's signal: no diagnostic on `return r;` (it would fire
    // `return_of_invalid_type` if `r` were `Unknown`/`dynamic`).
    let rows = offsets(&report, "return_of_invalid_type");
    assert_eq!(
        rows.len(),
        0,
        "expected no return_of_invalid_type, got {:?}",
        report.diagnostics
    );
}
