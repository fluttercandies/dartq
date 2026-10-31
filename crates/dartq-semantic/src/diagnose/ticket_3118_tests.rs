//! The #3118 pins: a generic LOCAL function declaration latches
//! `pending_exec_name` exactly like a top-level function's arm (the
//! `exec_bounds` route — a local function owns no member send, so the
//! `method_exec_bounds` table's member-inference guard has nothing to
//! race). Before the latch, `pair_exec_window` had no name to key the
//! closed `<T extends num>` window on, and a bound-violating written
//! type-argument group (`f<String>`) under-reported where the oracle
//! checks the argument against the declared bound (upstream Dart SDK
//! source, pkg/analyzer/lib/src/dart/resolver/
//! element_binding_visitor.dart:385-401 registers a real
//! `LocalFunctionFragmentImpl`, and scope_context.dart:260-279 wraps its
//! resolution in `withTypeParameterScope(element.typeParameters)` — the
//! same shape a top-level declaration owns).

use crate::diagnose::test_rig::{assert_silent, code_names, report_ops, rows};

/// The bound-violating WRITTEN type-argument group checks against the
/// local function's declared bound: `f<String>` over `f<T extends num>`
/// reports `type_argument_not_matching_bounds` — the `exec_bounds`
/// window is the bounds-verdict's parameter source at the invocation
/// site (`decl_bounds` in `check_arguments`).
#[test]
fn written_type_arguments_check_the_local_bound() {
    let report = report_ops(
        "void g() {\n\
         \x20 T f<T extends num>(T x) => x;\n\
         \x20 f<String>(\"s\");\n\
         }",
    );
    let names = code_names(&report);
    assert!(
        names
            .iter()
            .any(|name| *name == "type_argument_not_matching_bounds"),
        "expected the bounds face, got {:?}",
        rows(&report)
    );
    assert!(
        report
            .diagnostics
            .iter()
            .any(|d| d.message.contains("'num'")),
        "the bound violation must name 'num': {:?}",
        rows(&report)
    );
}

/// The bare (return-less) header pairs the same bound: `f`'s own `<T>`
/// window lands in `exec_bounds` under the same name the written-return
/// shape pairs on — the return type precedes the name, never disturbing
/// the token-order latch.
#[test]
fn bare_local_generic_fn_pairs_its_bound() {
    let report = report_ops(
        "void g() {\n\
         \x20 f<T extends num>(T x) => x;\n\
         \x20 f<String>(\"s\");\n\
         }",
    );
    let names = code_names(&report);
    assert!(
        names
            .iter()
            .any(|name| *name == "type_argument_not_matching_bounds"),
        "expected the bounds face, got {:?}",
        rows(&report)
    );
}

/// A local generic nested inside a METHOD pairs the function-shaped
/// window, not the method table: the bound still drives the written
/// type-argument verdict.
#[test]
fn a_local_generic_inside_a_method_pairs() {
    let report = report_ops(
        "class C {\n\
         \x20 void m() {\n\
         \x20   T f<T extends num>(T x) => x;\n\
         \x20   f<String>(\"s\");\n\
         \x20 }\n\
         }",
    );
    let names = code_names(&report);
    assert!(
        names
            .iter()
            .any(|name| *name == "type_argument_not_matching_bounds"),
        "expected the bounds face, got {:?}",
        rows(&report)
    );
}

/// An in-bounds call stays silent: `f(1)` over `f<T extends num>` admits
/// `int` against `num` — the paired bound reports nothing.
#[test]
fn an_in_bounds_call_stays_silent() {
    let report = report_ops(
        "void g() {\n\
         \x20 T f<T extends num>(T x) => x;\n\
         \x20 f(1);\n\
         }",
    );
    assert_silent(&report);
}

/// A non-generic local function is untouched by the latch: its
/// `pending_exec_name` dies at the formal list's open (the
/// `formal_list_depth == 0` clear), and the argument check still reads
/// its own formals.
#[test]
fn a_non_generic_local_fn_is_unaffected() {
    let report = report_ops(
        "void g() {\n\
         \x20 int f(int x) => x;\n\
         \x20 f(1);\n\
         }",
    );
    assert_silent(&report);
}

/// The inference-driven `argument_type_not_assignable` face (`f("s")`
/// solves `T := num` off the bound and rejects the `String` argument):
/// the `exec_bounds` window now carries `(T, Some(num))`, but the
/// argument walk pairs the argument against the callee's FORMALS — a
/// generic local function captures no `local_fn_sig` (the tear-off
/// capture's `type_params.len() > frame.type_mark` early return keeps
/// the bound-instantiated signature unspelled), so `formals` resolves
/// `None` and the constraint gather never binds `T`. Residual pending
/// the generic-local-fn signature channel.
#[test]
#[ignore = "generic local fn formals channel missing: local_fn_sig captures \
            nothing under type_params (engine_core's capture early-return), \
            and the call-formals arm never consults it"]
fn local_generic_fn_reports_the_bound_clamp() {
    let report = report_ops(
        "void g() {\n\
         \x20 T f<T extends num>(T x) => x;\n\
         \x20 f(\"s\");\n\
         }",
    );
    let names = code_names(&report);
    assert_eq!(
        names,
        vec!["argument_type_not_assignable"],
        "{:?}",
        rows(&report)
    );
    assert!(
        report
            .diagnostics
            .iter()
            .any(|d| d.message.contains("'num'")),
        "the clamp must name 'num': {:?}",
        rows(&report)
    );
}
