//! #3595 pins: an inference-bound closure formal whose receiver face is
//! the schema's recovered `Object?` must not fabricate
//! `unchecked_use_of_nullable_value` — horizontal inference decides `T`
//! from the later arguments, and a `dynamic`/non-null solution owes no
//! unchecked verdict (the oracle's ground truth below).

use super::test_rig::{report_ops, rows};

fn unchecked_present(report: &super::FileReport) -> bool {
    report
        .diagnostics
        .iter()
        .any(|d| d.code.name == "unchecked_use_of_nullable_value")
}

// ——— stop-fabricating pins ———

/// `f<T>(void Function(T), List<T>)` with `b = []`: `T` solves to
/// `dynamic`, `t.anyMethod()` dispatches dynamically — the oracle
/// reports nothing. The pass-1 `Object?` recovery bind must abstain.
#[test]
fn horizontal_dynamic_solution_stays_silent() {
    let report = report_ops(
        "void f<T>(void Function(T) a, List<T> b) {}\n\
         void m() {\n\
         \x20 f((t) {\n\
         \x20   t.anyMethod();\n\
         \x20   t.anyProp;\n\
         \x20 }, []);\n\
         }\n",
    );
    assert!(
        !unchecked_present(&report),
        "fabricated unchecked row: {:#?}",
        rows(&report)
    );
}

/// `b = [1]` solves `T = int`: `t.anyMethod()` misses `int`'s members
/// upstream — the oracle reports `undefined_method`, never
/// `unchecked_use_of_nullable_value`. The recovered `Object?` face must
/// stand down here too.
#[test]
fn horizontal_int_solution_reports_miss_not_unchecked() {
    let report = report_ops(
        "void f<T>(void Function(T) a, List<T> b) {}\n\
         void m() {\n\
         \x20 f((t) {\n\
         \x20   t.anyMethod();\n\
         \x20 }, [1]);\n\
         }\n",
    );
    assert!(
        !unchecked_present(&report),
        "fabricated unchecked row: {:#?}",
        rows(&report)
    );
}

/// The multi-formal twin: two recovered formals in one send, each
/// constrained by a later argument (`t` → `int`, `u` → `dynamic` —
/// neither owes an unchecked verdict upstream).
#[test]
fn horizontal_multi_formal_stays_silent() {
    let report = report_ops(
        "void f<T, U>(void Function(T) a, void Function(U) b, List<T> c) {}\n\
         void m() {\n\
         \x20 f((t) {\n\
         \x20   t.anyMethod();\n\
         \x20 }, (u) {\n\
         \x20   u.anyProp;\n\
         \x20 }, [1]);\n\
         }\n",
    );
    assert!(
        !unchecked_present(&report),
        "fabricated unchecked row: {:#?}",
        rows(&report)
    );
}

// ——— still-fires controls ———

/// A genuinely nullable local receiver keeps the honest unchecked
/// verdict (`int?` — the schema-recovery gate never sees a declared
/// face).
#[test]
fn declared_nullable_local_still_reports() {
    let report = report_ops(
        "void m(int? x) {\n\
         \x20 x.isOdd;\n\
         }\n",
    );
    assert!(
        unchecked_present(&report),
        "lost the honest unchecked row: {:#?}",
        rows(&report)
    );
}

/// A non-closure receiver bound to a decided `Object?` — the written
/// `Object?` spelling is the oracle's decided face, not the schema
/// recovery — keeps the verdict.
#[test]
fn decided_object_question_local_still_reports() {
    let report = report_ops(
        "void m(Object? o) {\n\
         \x20 o.anyMethod();\n\
         }\n",
    );
    assert!(
        unchecked_present(&report),
        "lost the decided-Object? row: {:#?}",
        rows(&report)
    );
}

/// A class-level `T extends Object?` formal receiver: the type
/// parameter's own bound spells nullable, the unchecked verdict stands.
#[test]
fn class_type_param_nullable_bound_still_reports() {
    let report = report_ops(
        "class C<T extends Object?> {\n\
         \x20 void m(T t) { t.anyMethod(); }\n\
         }\n",
    );
    assert!(
        unchecked_present(&report),
        "lost the nullable-bound row: {:#?}",
        rows(&report)
    );
}
