//! #3598 pins — a send-close solve's invocation is typed by its declared
//! return instantiated with the stashed solution, never by unwrapping the
//! published face a second time.
//!
//! # The defect
//!
//! `typedef F<X> = void Function<Y extends X>();` plus `F<X> toF<X>(X x)`
//! is the co19 `lang_feature_check.dart` face the `Simple-bounds` /
//! `Instantiate-to-bound` / `InferredTypeTest` families mount, always
//! through `var fsource = toF(source); F<Target>? t = fsource;`. The
//! send-close solve binds `X := A<int>?`, the argument-check walk
//! PUBLISHES the instantiated return onto the callee row (`F<A<int>?>`),
//! and the send-close fold deliberately skips a solved row — so the row's
//! face already IS the call's static type. But
//! [`Engine::operand_static_ty`]'s solved arm re-read that face as the
//! callee's own signature and applied [`Engine::invoked_callee_result`]:
//! the result's own `Y` parameter (`type_params != 0`) defeated the
//! unwrap and answered `Some(Ty::UNKNOWN)` — so `var fsource` bound
//! `Unknown`, every downstream read stayed `Unknown`, and the
//! bound-equality verdict never ran. 1,101 of the census's 1,647 missing
//! `invalid_assignment` rows ride this one lever.
//!
//! # The wiring
//!
//! [`Engine::solved_invoke_result`] (send_result.rs) types a solved
//! invocation by its DECLARED RETURN instantiated with the solution —
//! [`Engine::callee_return_ty`] through
//! [`Engine::instantiate_call_return`], the same publication the
//! argument-check walk wrote — keeping a FUNCTION-typed instantiation
//! intact (`F<G>`, never its `void` return). A face that still DECLARES a
//! solved parameter as its own function-shape type parameter escaped the
//! publication (the callee's own standing signature, a tear-off the
//! declaration tables cannot spell) and keeps the callee unwrap;
//! everything else falls to #3588's face arms. The check-layer fallback a
//! previous lane tried (re-deriving the declared return for a
//! var-inferred local in assignability.rs) fabricated 1,067 rows and
//! recovered none — this is the engine-side root fix: the `var` binding
//! is right in the first place. All oracle verdicts below were probed on
//! 3.13.7 by the #3594/#3588 lanes' fixtures.

use super::FileReport;
use super::test_rig::{RigImport, code_names, report_ops_with, rows};

/// The shared `lang_feature_check.dart` face the co19 corpus mounts:
/// `typedef F<X> = void Function<Y extends X>()` plus `toF`.
const LANG_FEATURE_CHECK: &str = r#"
typedef F<X> = void Function<Y extends X>();
F<X> toF<X>(X x) => <Y extends X>() {};
"#;

/// The corpus fixture's world: `num`/`int`/`String` plus the generic
/// container the bounds families parameterize over.
fn lang_feature_report(body: &str) -> FileReport {
    report_ops_with(
        &format!(
            "class num {{}} class int extends num {{}} class String {{}}\nclass A<X extends num> {{}}\n{body}"
        ),
        &[RigImport {
            uri: "package:x/lang_feature_check.dart",
            prefix: None,
            source: LANG_FEATURE_CHECK,
        }],
        None,
    )
}

/// The declared-type acceptance: `var fsource = toF(source)` infers the
/// instantiated `F<A<int>?>` (the miss names the function face with its
/// substituted `Y` bound — never `Unknown`'s silence, never a second
/// unwrap's `void`).
#[test]
fn t3598_var_binds_instantiated_function_face() {
    let report = lang_feature_report(
        "main() {\n  A<int>? source;\n  var fsource = toF(source);\n  String s = fsource;\n}\n",
    );
    let misses: Vec<&str> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "invalid_assignment")
        .map(|d| d.message.as_str())
        .collect();
    assert_eq!(misses.len(), 1, "expected one row, got {:?}", rows(&report));
    assert!(
        misses[0].contains("'void Function<Y extends A<int>?>()'"),
        "the miss must name the instantiated function face, got {:?}",
        misses[0]
    );
}

/// The fixture's own shape: `F<A<num>?>? t = fsource` reports — the
/// signatures' own `Y` bounds differ (`A<int>?` vs `A<num>?`), and
/// neither is a subtype of the other in both directions (#3594's
/// bound-equality leg, now reached through the honest `var` binding).
#[test]
fn t3598_bounds_differ_reports() {
    let report = lang_feature_report(
        "main() {\n  A<int>? source;\n  var fsource = toF(source);\n  F<A<num>?>? t = fsource;\n}\n",
    );
    assert!(
        code_names(&report).contains(&"invalid_assignment"),
        "expected invalid_assignment, got {:?}",
        rows(&report)
    );
}

/// The control: identical bounds stay silent (the fix must not fabricate
/// the other direction).
#[test]
fn t3598_bounds_equal_stays_silent() {
    let report = lang_feature_report(
        "main() {\n  A<num>? source;\n  var fsource = toF(source);\n  F<A<num>?>? t = fsource;\n}\n",
    );
    assert!(
        !code_names(&report).contains(&"invalid_assignment"),
        "expected no invalid_assignment, got {:?}",
        rows(&report)
    );
}

/// A NON-generic instantiation keeps the plain face: `int counter<T>(T
/// x)` answers `int` whatever the solve bound (the arm must not disturb
/// the param-free-return path).
#[test]
fn t3598_param_free_return_unchanged() {
    let report = lang_feature_report(
        "int counter<T>(T x) => 1;\nmain() {\n  A<int>? source;\n  var n = counter(source);\n  String s = n;\n}\n",
    );
    let misses: Vec<&str> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "invalid_assignment")
        .map(|d| d.message.as_str())
        .collect();
    assert_eq!(misses.len(), 1, "expected one row, got {:?}", rows(&report));
    assert!(
        misses[0].contains("'int'"),
        "the miss must name the plain return, got {:?}",
        misses[0]
    );
}

/// The #3588 residual this fix closes: a solve whose instantiation is a
/// NON-generic function type (`typedef G<T> = T Function(T); G<T> id<T>`)
/// answers the function type itself, never the second unwrap's `int`.
#[test]
fn t3598_plain_function_instantiation_not_unwrapped() {
    let report = report_ops_with(
        &format!(
            "class num {{}} class int extends num {{}} class String {{}}\ntypedef G<T> = T Function(T);\nG<T> id<T>(T x) => (T y) => y;\nmain() {{\n  var r = id(1);\n  String s = r;\n}}\n"
        ),
        &[],
        None,
    );
    let misses: Vec<&str> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "invalid_assignment")
        .map(|d| d.message.as_str())
        .collect();
    assert_eq!(misses.len(), 1, "expected one row, got {:?}", rows(&report));
    assert!(
        misses[0].contains("'int Function(int)'"),
        "the miss must name the instantiated function type, got {:?}",
        misses[0]
    );
}

/// The #3588 sentinels stay green: a solved formal's send stays silent,
/// and a function-typed callee's RESULT (unsolved, the fold's own
/// flatten) still answers the function face — one unwrap, never two.
#[test]
fn t3598_unsolved_function_result_keeps_single_flatten() {
    let report = report_ops_with(
        &format!(
            "class String {{}}\nint Function(int) top() => (int x) => x;\nvoid apply(int Function(int) f) {{ f(1); }}\nmain() {{\n  var r = top();\n  String s = r;\n  apply((int x) => x);\n}}\n"
        ),
        &[],
        None,
    );
    let misses: Vec<&str> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "invalid_assignment")
        .map(|d| d.message.as_str())
        .collect();
    assert_eq!(misses.len(), 1, "expected one row, got {:?}", rows(&report));
    assert!(
        misses[0].contains("'int Function(int)'"),
        "the miss must name the function-typed result, got {:?}",
        misses[0]
    );
}

/// A solved invocation still reports the SEND's result when consumed
/// directly in an assignment (not only through a `var` binding) — the
/// family's non-`var` census rows.
#[test]
fn t3598_direct_assignment_reports() {
    let report = lang_feature_report(
        "main() {\n  A<int>? source;\n  String s = toF(source);\n}\n",
    );
    let misses: Vec<&str> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "invalid_assignment")
        .map(|d| d.message.as_str())
        .collect();
    assert_eq!(misses.len(), 1, "expected one row, got {:?}", rows(&report));
    assert!(
        misses[0].contains("'void Function<Y extends A<int>?>()'"),
        "the miss must name the instantiated face, got {:?}",
        misses[0]
    );
}

/// TEMP bisect probes (removed before finish).
#[test]
fn t3598_bisect_direct() {
    for (name, src, import) in [
        ("plain_fn", "int Function(int) top() => (int x) => x;\nmain() { String s = top(); }\n", false),
        ("alias_fn", "typedef G<T> = T Function(T);\nG<int> idc(int x) => (int y) => y;\nmain() { String s = idc(1); }\n", false),
        ("generic_plain_ret", "int counter<T>(T x) => 1;\nmain() { A<int>? q; String s = counter(q); }\n", true),
        ("generic_fn_ret", "main() { A<int>? q; String s = toF(q); }\n", true),
    ] {
        let body = if import {
            format!("class num {{}} class int extends num {{}} class String {{}}\nclass A<X extends num> {{}}\n{src}")
        } else {
            format!("class String {{}}\n{src}")
        };
        let report = if import {
            report_ops_with(&body, &[RigImport { uri: "package:x/lang_feature_check.dart", prefix: None, source: LANG_FEATURE_CHECK }], None)
        } else {
            report_ops_with(&body, &[], None)
        };
        let misses: Vec<String> = report
            .diagnostics
            .iter()
            .filter(|d| d.code.name == "invalid_assignment")
            .map(|d| d.message.to_string())
            .collect();
        println!("BISECT {name}: {:?}", rows(&report));
    }
}
