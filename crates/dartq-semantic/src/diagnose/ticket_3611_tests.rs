//! #3611 — the `unnecessary_type_check` verdict over operands whose
//! written declaration spells its type with an import prefix.
//!
//! Oracle contract (upstream Dart SDK source,
//! pkg/analyzer/lib/src/error/best_practices_verifier.dart:987-1041 —
//! `_checkAllTypeChecks` → `TypeSystem.isSubtypeOf(operand.staticType,
//! node.type.type)`): the operand's static type is the resolved element,
//! so `_async is async.A` over `async.A _async;` is constant-true
//! whichever prefix the annotation used. This engine's declaration
//! capture drops that prefix — a group-less `p.T` annotation arms nothing
//! the variable can read, a parameterised one arms the leaf — while the
//! checked side's `armed_ty_span` join spells the joined `p.T`; the
//! spelled-head comparison then reads two different types and the row
//! stays silent. The verdict re-reads the written annotation from the
//! declaration's own name token (`Engine::prefixed_operand_ty`) and keeps
//! the recovered `p.T` face only when the prefix resolves.
//!
//! Shapes pinned here, one per declaration kind the corpus family uses
//! (`language/identifier/known_prefix_test.dart` and
//! `known_prefix_error_test.dart`): a top-level variable, a parameterised
//! top-level variable, and a field of a local receiver.
//!
//! Probed against `dart analyze` 3.13.7: every expectation below is the
//! oracle's live verdict.

use super::test_rig::{RigImport, assert_silent, code_names, report_ops_imports, rows};
use super::*;

/// The prefixed library the census cases import (`async.A`, `async.B<T>`
/// — the `built_in_prefix_library_async.dart` face).
const ASYNC_LIB: RigImport<'static> = RigImport {
    uri: "package:async/async.dart",
    prefix: Some("async"),
    source: "class A {} class B<T> {}",
};

/// The code names in source order.
fn names(report: &FileReport) -> Vec<&'static str> {
    code_names(report)
}

/// Shape (1): a top-level variable whose annotation is a group-less
/// prefixed type — the operand face captures nothing, so the recovery is
/// the only way the verdict can see `async.A` on both sides.
#[test]
fn top_level_prefixed_declaration_reports() {
    let report = report_ops_imports(
        "async.A g = new async.A();\nvoid main() { g is async.A; }\n",
        &[ASYNC_LIB],
    );
    assert_eq!(
        names(&report),
        vec!["unnecessary_type_check"],
        "{:?}",
        rows(&report)
    );
}

/// Shape (2): a parameterised prefixed annotation captures the LEAF
/// (`B<dynamic>`), never the joined head — the recovery restores the
/// `async.` segment the checked side keeps.
#[test]
fn generic_prefixed_declaration_reports() {
    let report = report_ops_imports(
        "async.B<dynamic> gb = new async.B();\nvoid main() { gb is async.B; }\n",
        &[ASYNC_LIB],
    );
    assert_eq!(
        names(&report),
        vec!["unnecessary_type_check"],
        "{:?}",
        rows(&report)
    );
}

/// Shape (3): a field of a local receiver — the declaration anchor is the
/// field's own name token inside the container the receiver resolves to.
#[test]
fn field_prefixed_declaration_reports() {
    let report = report_ops_imports(
        "class C {\n  async.A f = new async.A();\n}\n\
         void main() {\n  C c = new C();\n  c.f is async.A;\n}\n",
        &[ASYNC_LIB],
    );
    assert_eq!(
        names(&report),
        vec!["unnecessary_type_check"],
        "{:?}",
        rows(&report)
    );
}

/// A prefixed operand against a different element stays silent: the
/// recovery restores the annotation's own head, it never widens it. (The
/// cross-prefix silence — `async.B<int>` vs `await.B` — is pinned by
/// #3461's `mismatched_prefix_heads_stay_silent`, which this recovery
/// must keep green.)
#[test]
fn different_prefixed_element_stays_silent() {
    let report = report_ops_imports(
        "void f() {\n  async.A a = new async.A();\n  a is async.B;\n}\n",
        &[ASYNC_LIB],
    );
    assert_silent(&report);
}
