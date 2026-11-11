//! #3952 pins — a const variable's WRITTEN annotation survives its
//! initializer's constant fold.
//!
//! The census rows (`double_in_int_op_test_id1..id6`) are EXTRAS of
//! `argument_type_not_assignable` on a const-`dynamic` receiver's `|`
//! operand: `const dynamic i1 = 3; var x = i1 | 2.2;`. The oracle reads
//! the receiver's STATIC type (`dynamic`, so `isAssignableTo` holds and
//! the argument check is silent); dartq read the constant's VALUE type
//! (`int`) and reported the `2.2` argument against `int`'s `int` formal.
//!
//! Root (not the operator gate — Lane AR's 4-way isolation proved the
//! operator face itself decides correctly, `final dynamic` was already
//! silent): the const initializer's close bound the folded VALUE type
//! over the frame's declared slot for EVERY const local, annotated or
//! not (`const_eval_faces.rs`'s `finish_const_initializer` →
//! `bind_var_type(name, folded)`). Upstream re-types a local from its
//! initializer only when the declaration carries NO written type
//! (`variable_declaration_resolver.dart:84`'s `parent.type == null &&
//! element is LocalVariableElementImpl` gate), so every later read of a
//! written-annotation const saw the value's face instead of the declared
//! one.
//!
//! The 4-way isolation below is the probe that pinned it: only the two
//! WRITTEN-type declarations leaked (`const dynamic` and `const Object`
//! both read their value), while `final dynamic` (no const fold) and
//! `const int` (whose value type happens to equal its annotation) read
//! correctly. The `const Object o = 'a'` shape is the sharpest witness:
//! the pre-fix operator message spelled `'String'`, the annotation's
//! exact opposite.

use super::test_rig::report_ops;
use crate::FileReport;

/// The rows of one code, in source order.
fn rows_of(report: &FileReport, code: &str) -> Vec<String> {
    let mut at: Vec<(u32, String)> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == code)
        .map(|d| (d.range.offset, d.message.to_string()))
        .collect();
    at.sort_by_key(|(offset, _)| *offset);
    at.into_iter().map(|(_, text)| text).collect()
}

/// Every diagnostic code, sorted — the whole-verdict face the silence
/// pins read.
fn codes(report: &FileReport) -> Vec<String> {
    let mut out: Vec<String> = report
        .diagnostics
        .iter()
        .map(|d| d.code.name.to_string())
        .collect();
    out.sort();
    out
}

// —— the 4-way isolation (the probe that pinned the root) ————————————————

/// `const dynamic i1 = 3; var x = i1 | 2.2;` — the oracle reads the
/// receiver's STATIC type `dynamic` and stays silent (the census family's
/// canonical shape; `double_in_int_op_test.dart:22`'s `(i1 | d2)` row is
/// this face). Pre-fix the folded `int` value type leaked and the `2.2`
/// argument reported against `int`'s `int` formal.
#[test]
fn const_dynamic_receiver_operand_is_silent() {
    let report = report_ops(
        "void f() {\n\
         \x20 const dynamic i1 = 3;\n\
         \x20 var x = i1 | 2.2;\n\
         }\n",
    );
    assert!(
        !codes(&report)
            .iter()
            .any(|c| c == "argument_type_not_assignable"),
        "{:?}",
        codes(&report)
    );
}

/// The mirror declaration: `final dynamic` never enters the const fold, so
/// it was silent before the fix too — the control that isolates the leak
/// to the const path rather than to the operator gate.
#[test]
fn final_dynamic_receiver_operand_is_silent() {
    let report = report_ops(
        "void f() {\n\
         \x20 final dynamic i1 = 3;\n\
         \x20 var x = i1 | 2.2;\n\
         }\n",
    );
    assert!(
        !codes(&report)
            .iter()
            .any(|c| c == "argument_type_not_assignable"),
        "{:?}",
        codes(&report)
    );
}

/// `const Object o = 'a'; var x = o | 2.2;` — the sharpest witness. The
/// oracle resolves `Object.|` (absent) and reports `undefined_operator`
/// with the DECLARED head; the folded value is `String`, so a leaked
/// value type spells `'String'` in the message instead. Asserting the
/// message text pins both the code and the receiver face.
#[test]
fn const_object_receiver_keeps_the_declared_head() {
    let report = report_ops(
        "void f() {\n\
         \x20 const Object o = 'a';\n\
         \x20 var x = o | 2.2;\n\
         }\n",
    );
    assert_eq!(
        rows_of(&report, "undefined_operator"),
        ["The operator '|' isn't defined for the type 'Object'."]
    );
    assert!(
        !codes(&report)
            .iter()
            .any(|c| c == "argument_type_not_assignable"),
        "{:?}",
        codes(&report)
    );
}

/// `const num n = 3; var x = n | 2.2;` — the third written annotation the
/// probe pinned. `num` declares no `|`, so the oracle reports
/// `undefined_operator('|', num)`; a leaked `int` value type would answer
/// `int.|` and report the `2.2` argument instead.
#[test]
fn const_num_receiver_keeps_the_declared_head() {
    let report = report_ops(
        "void f() {\n\
         \x20 const num n = 3;\n\
         \x20 var x = n | 2.2;\n\
         }\n",
    );
    assert_eq!(
        rows_of(&report, "undefined_operator"),
        ["The operator '|' isn't defined for the type 'num'."]
    );
    assert!(
        !codes(&report)
            .iter()
            .any(|c| c == "argument_type_not_assignable"),
        "{:?}",
        codes(&report)
    );
}

/// `const int i1 = 3; var x = i1 | 2.2;` — the declaration whose value
/// type EQUALS its annotation. It reported correctly before the fix (the
/// leak was invisible), and it must keep reporting: `int.|` takes `int`,
/// so the `2.2` argument is genuinely not assignable.
#[test]
fn const_int_receiver_operand_still_reports() {
    let report = report_ops(
        "void f() {\n\
         \x20 const int i1 = 3;\n\
         \x20 var x = i1 | 2.2;\n\
         }\n",
    );
    assert_eq!(
        rows_of(&report, "argument_type_not_assignable"),
        ["The argument type 'double' can't be assigned to the parameter type 'int'. "]
    );
}

// —— the same leak on the other consumer faces ————————————————————————

/// The assignment face reads the receiver too: `const Object o = 'a';
/// List<int> x = o;` reports the DECLARED `Object` in the message
/// (upstream `checkForAssignableExpressionAtType` renders
/// `element.type`). A leaked `String` value type spelled `'String'`.
#[test]
fn const_object_initializer_reads_the_declared_head() {
    let report = report_ops(
        "void f() {\n\
         \x20 const Object o = 'a';\n\
         \x20 List<int> x = o;\n\
         }\n",
    );
    assert_eq!(
        rows_of(&report, "invalid_assignment"),
        ["A value of type 'Object' can't be assigned to a variable of type 'List<int>'."]
    );
}

/// The member face reads the receiver too: `const Object o = 'a';
/// print(o.length);` misses on `Object` (the oracle's row), where a leaked
/// `String` value type found `String.length` and stayed silent.
#[test]
fn const_object_member_miss_reads_the_declared_head() {
    let report = report_ops(
        "void f() {\n\
         \x20 const Object o = 'a';\n\
         \x20 print(o.length);\n\
         }\n",
    );
    assert_eq!(
        rows_of(&report, "undefined_getter"),
        ["The getter 'length' isn't defined for the type 'Object'."]
    );
}

// —— the unannotated const keeps its inferred face ————————————————

/// `const i1 = 3; var x = i1 | 2.2;` — an UNANNOTATED const's declared
/// type IS its initializer's folded type (`variableTypeFromInitializer
/// Type` under the same `parent.type == null` gate), so the `int` receiver
/// must keep reporting. This is the guard against over-correcting: the
/// fold still owns the slot when no annotation armed one.
#[test]
fn inferred_const_receiver_operand_still_reports() {
    let report = report_ops(
        "void f() {\n\
         \x20 const i1 = 3;\n\
         \x20 var x = i1 | 2.2;\n\
         }\n",
    );
    assert_eq!(
        rows_of(&report, "argument_type_not_assignable"),
        ["The argument type 'double' can't be assigned to the parameter type 'int'. "]
    );
}

/// The unannotated const's folded face still reaches the member walk:
/// `const d = 3; print(d.nosuchmember);` misses on `int` (the oracle's
/// row) rather than suppressing.
#[test]
fn inferred_const_member_miss_keeps_the_folded_face() {
    let report = report_ops(
        "void f() {\n\
         \x20 const d = 3;\n\
         \x20 print(d.nosuchmember);\n\
         }\n",
    );
    assert_eq!(
        rows_of(&report, "undefined_getter"),
        ["The getter 'nosuchmember' isn't defined for the type 'int'."]
    );
}
