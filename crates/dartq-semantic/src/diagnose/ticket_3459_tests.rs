//! The #3459 pins (the `assignment_to_final` EXTRA cluster): a bare
//! `name = e` inside a constructor-initializer list is a
//! `ConstructorFieldInitializer` upstream — `element_resolver.dart` visits
//! it through `_checkForInvalidField` (`error_verifier.dart`), never
//! `AssignmentExpressionResolver`/`AssignmentVerifier.verify`, so the
//! `assignment_to_*` family never fires there (upstream Dart SDK source,
//! pkg/analyzer/lib/src/error/assignment_verifier.dart:34-95 — the
//! verifier's whole arm list, which the initializer world never calls).
//! The push-time stamp already folds those rows into `init_rows`; the
//! pass-2 member arm in `resolve_bare_row` re-read them as implicit-`this`
//! setter-less writes and mis-coded `assignment_to_final` on every `final`
//! field they initialized — the corpus's dominant extra shape (2,824
//! rows, ~all ctor-initializer writes: `syntax_test_*`'s `fn`,
//! `dot_shorthand_helper.dart`'s `x`, `const_evaluation_lib.dart`'s `x`).
//!
//! The surviving arm keeps the upstream split: the name walks the
//! enclosing class's field set — a `Field` resolves the initializer
//! silently (its `final` is the point of the declaration), a miss or an
//! accessor spelling codes `initializer_for_non_existent_field` at the
//! whole node (`_checkForInvalidField`), never the bare name.
//!
//! Oracle provenance: `dart analyze` 3.13.7 — upstream
//! `AssignmentExpressionResolutionTest` /
//! `assignment_verifier.dart`'s `GetterElement` arm (`variable.isConst`
//! → `assignment_to_const`; `isOriginGetterSetter` →
//! `assignment_to_final_no_setter`; else `assignment_to_final`) and the
//! `definesSetter` induction (`!isFinal || (isLate && isFinal &&
//! !hasInitializer)`), which the same-file carve-out
//! `field_declares_no_setter` mirrors.

use super::test_rig::{RigImport, assert_silent, code_names, report_ops, report_ops_imports, rows};

/// The cluster's dominant shape: `: fn = e` initializes a same-file
/// `final` field — silent upstream, the corpus's 1,490-row head.
#[test]
fn ctor_initializer_final_field_same_file_silent() {
    let report = report_ops(
        "class C {\n\
           final fn;\n\
           C() : fn = 42 {}\n\
           C.named() : fn = wrap(1) {}\n\
           static int wrap(int v) => v;\n\
         }\n",
    );
    assert_silent(&report);
}

/// The shorthand-helper shape: `x = 1` initializers inside a class whose
/// `final` field they set — `dot_shorthand_helper.dart`'s 168-row arm was
/// this same cause, not a shorthand selector.
#[test]
fn ctor_initializer_many_ctors_silent() {
    let report = report_ops(
        "class ConstructorClass {\n\
           final int? x;\n\
           ConstructorClass(this.x);\n\
           ConstructorClass.regular(this.x);\n\
           ConstructorClass.named({this.x});\n\
           ConstructorClass.optional([this.x]);\n\
           ConstructorClass.integer(int i) : x = 1;\n\
           ConstructorClass.ctor(ConstructorClass c) : x = 1;\n\
           const ConstructorClass.constRegular(this.x);\n\
         }\n",
    );
    assert_silent(&report);
}

/// An extension-type secondary constructor's `: id = a` initializes the
/// representation field — `dynamic_semantics_extension_type_*`'s `id`
/// extras.
#[test]
fn extension_type_ctor_initializer_silent() {
    let report = report_ops(
        "extension type A.num0(num it) {\n\
           A.named(int a) : it = a;\n\
         }\n",
    );
    assert_silent(&report);
}

/// An enhanced-enum constant's `: log = 'called'` — the `semantics_A0*`
/// enum extras.
#[test]
fn enum_ctor_initializer_silent() {
    let report = report_ops(
        "enum E {\n\
           e1(),\n\
           e2();\n\
           final String? log;\n\
           const E() : log = 'called';\n\
         }\n",
    );
    assert_silent(&report);
}

/// The miss arm keeps its own face: `: missing = e` over a class with no
/// such field codes `initializer_for_non_existent_field` at the whole
/// node — never `assignment_to_final`, never `undefined_identifier`.
#[test]
fn ctor_initializer_missing_field_reports_own_face() {
    let report = report_ops(
        "class C {\n\
           final int real;\n\
           C() : missing = 1, real = 0 {}\n\
         }\n",
    );
    assert_eq!(
        code_names(&report),
        vec!["initializer_for_non_existent_field"],
        "{:?}",
        rows(&report)
    );
}

/// A plain `final` field write inside a method body still reports
/// `assignment_to_final` — the family is gated, not deleted.
#[test]
fn method_body_final_field_write_reports() {
    let report = report_ops(
        "class C {\n\
           final int x = 0;\n\
           void m() { x = 1; }\n\
         }\n",
    );
    assert_eq!(
        code_names(&report),
        vec!["assignment_to_final"],
        "{:?}",
        rows(&report)
    );
}

/// The same write through a CROSS-FILE field stays a row — `landing` is
/// the foreign declaration, `assignment_to_final` keeps the model
/// convention there.
#[test]
fn cross_file_final_field_write_reports() {
    let report = report_ops_imports(
        "import 'package:t/dep.dart';\n\
         void f(C c) { c.x = 1; }\n",
        &[RigImport {
            uri: "package:t/dep.dart",
            prefix: None,
            source: "class C { final int x = 0; }\n",
        }],
    );
    assert!(
        code_names(&report).contains(&"assignment_to_final"),
        "{:?}",
        rows(&report)
    );
}

/// The static-face twin across files (`C.x = 1`) — same row, the
/// container verdict's own arm.
#[test]
fn cross_file_static_final_write_reports() {
    let report = report_ops_imports(
        "import 'package:t/dep.dart';\n\
         void f() { C.x = 1; }\n",
        &[RigImport {
            uri: "package:t/dep.dart",
            prefix: None,
            source: "class C { static final int x = 0; }\n",
        }],
    );
    assert!(
        code_names(&report).contains(&"assignment_to_final"),
        "{:?}",
        rows(&report)
    );
}

/// A same-file `late final` field without an initializer declares the
/// induced `x=` setter upstream (`definesSetter`) — its first write is
/// the legal initializing write, silent.
#[test]
fn late_final_uninit_write_silent() {
    let report = report_ops(
        "class C {\n\
           late final int x;\n\
           void m() { x = 1; }\n\
         }\n",
    );
    assert_silent(&report);
}

/// A `late final` field WITH an initializer declares no setter — a write
/// reports `assignment_to_final`.
#[test]
fn late_final_initialized_write_reports() {
    let report = report_ops(
        "class C {\n\
           late final int x = 0;\n\
           void m() { x = 1; }\n\
         }\n",
    );
    assert_eq!(
        code_names(&report),
        vec!["assignment_to_final"],
        "{:?}",
        rows(&report)
    );
}

/// A `const` field's write is `assignment_to_const`'s row upstream — this
/// family claims it silently (the honest under-report the #3282 family
/// comment carries) rather than mis-coding `assignment_to_final`.
#[test]
fn const_field_write_not_assignment_to_final() {
    let report = report_ops(
        "class C {\n\
           static const int x = 0;\n\
           void m() { C.x = 1; }\n\
         }\n",
    );
    assert!(
        !code_names(&report).contains(&"assignment_to_final"),
        "{:?}",
        rows(&report)
    );
}

/// An augmented `final` field's write still reports — the augmentation
/// projection carries `is_final` through the member face.
#[test]
fn augmented_field_write_reports() {
    let report = report_ops(
        "class C {\n\
           final int x = 0;\n\
           void m() { this.x = 1; }\n\
         }\n",
    );
    assert!(
        code_names(&report).contains(&"assignment_to_final"),
        "{:?}",
        rows(&report)
    );
}

/// A record-pattern write `case (x: var y)` never assigns `x` — the
/// field-pattern name is a read; the family's write arms stay out.
#[test]
fn record_pattern_field_silent() {
    let report = report_ops(
        "class C {\n\
           final int x = 0;\n\
           void m(Object? o) { if (o case C(x: var y)) {} }\n\
         }\n",
    );
    assert!(
        !code_names(&report).contains(&"assignment_to_final"),
        "{:?}",
        rows(&report)
    );
}

/// A leading-dot shorthand write to an unresolved selector: the oracle
/// codes `dot_shorthand_undefined_member` (the shorthand face owns the
/// miss), never `assignment_to_final` — the family's claim refuses a
/// selector that did not resolve a declared getter.
#[test]
fn dot_shorthand_write_no_assignment_to_final() {
    let report = report_ops(
        "class C {\n\
           static const int x = 0;\n\
           C m(C c) => c..x = .new(1);\n\
         }\n",
    );
    assert!(
        !code_names(&report).contains(&"assignment_to_final"),
        "{:?}",
        rows(&report)
    );
}
