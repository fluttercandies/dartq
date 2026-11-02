//! The #3660 pins: `mixin_application_not_implemented_interface` — the
//! `with`-entry `on` constraint satisfaction face.
//!
//! # Root causes fixed
//!
//! * **Implicit supertype.** `enclosingClass.supertype` (upstream
//!   error_verifier.dart:6086) is `Enum` for an enum declaration, `Object`
//!   for a class with no `extends` — not "no superclass at all". The old
//!   arm treated a missing `extends` as unsatisfiable-by-supertype and
//!   reported `Object doesn't implement X` for `M on X` applied to `enum E`
//!   or a plain `class C`, where the oracle reads the constraint against
//!   `Enum <: X` / `Object <: X` and stays silent.
//! * **`X <: Object` axiom.** Every interface type is a subtype of
//!   `Object`; `M on Object` is trivially satisfied by any application.
//! * **Cross-file supertype resolution.** `supertype_reaches` looked the
//!   written superclass name up in `self.file`, so an `extends` clause
//!   naming an imported container (`extends ExtendsSealedClass` where
//!   `ExtendsSealedClass` lives in another library) resolved to nothing
//!   and the constraint check emitted against a phantom `Object`.
//!
//! Oracle anchors (dart 3.13.7):
//! * `enum E with M` where `M on Enum` — silent (`Enum <: Enum`).
//! * `enum E with M` where `M on Object` — silent (`Enum <: Object`).
//! * `enum E with M` where `M on A` — reports `'M' can't be mixed onto
//!   'Enum' because 'Enum' doesn't implement 'A'` at the mixin name.
//! * `class C with M` where `M on Object` — silent (`Object <: Object`).
//! * `class C extends A with M` where `A extends B` imported and
//!   `M on B` — silent (`A <: B` transitively).
//!
//! Upstream: `_checkForMixinSuperclassConstraints`
//! (error_verifier.dart:6076-6116) — `superType` is the resolved
//! `enclosingClass.supertype`; satisfaction is `isSubtypeOf(superType,
//! constraint)` then a probe of each previous mixin.

use crate::diagnose::FileReport;
use crate::diagnose::test_rig::{CORE_PRELUDE, RigImport, report_ops, report_ops_imports};

/// The `code` rows of `report` as `(offset, length)` in the fixture's own
/// coordinates.
fn spans(report: &FileReport, code: &str) -> Vec<(u32, u32)> {
    let shift = u32::try_from(CORE_PRELUDE.len()).expect("prelude fits u32");
    report
        .diagnostics
        .iter()
        .filter(|row| row.code.name == code)
        .map(|row| (row.range.offset - shift, row.range.length))
        .collect()
}

/// The `code` rows of `report`, as messages in row order.
fn messages(report: &FileReport, code: &str) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .filter(|row| row.code.name == code)
        .map(|row| row.message.to_string())
        .collect()
}

/// The UTF-16 offset of `needle`'s first occurrence in `source`.
fn offset_of(source: &str, needle: &str) -> u32 {
    let at = source.find(needle).expect("needle present in fixture");
    u32::try_from(source[..at].encode_utf16().count()).expect("fixture fits u32")
}

/// The span of `needle`'s `nth` occurrence (0-based) in `source`.
fn nth_span_of(source: &str, needle: &str, nth: usize) -> (u32, u32) {
    let mut from = 0usize;
    let mut at = 0usize;
    for _ in 0..=nth {
        at = source[from..].find(needle).expect("needle present") + from;
        from = at + needle.len();
    }
    let offset = u32::try_from(source[..at].encode_utf16().count()).expect("fixture fits u32");
    let length = u32::try_from(needle.encode_utf16().count()).expect("needle fits u32");
    (offset, length)
}

fn count(report: &FileReport, code: &str) -> usize {
    report
        .diagnostics
        .iter()
        .filter(|row| row.code.name == code)
        .count()
}

const CODE: &str = "mixin_application_not_implemented_interface";

// —— enum implicit-Enum supertype ——

/// `enum E with M` where `M on Enum` is satisfied: an enum's implicit
/// supertype IS `Enum`, so `Enum <: Enum` holds.
#[test]
fn enum_with_mixin_on_enum_is_silent() {
    let source = "mixin M on Enum {}\n\nenum E with M {\n  v\n}\n";
    assert_eq!(count(&report_ops(source), CODE), 0);
}

/// `enum E with M` where `M on Object` is satisfied: `Enum <: Object`.
#[test]
fn enum_with_mixin_on_object_is_silent() {
    let source = "mixin M on Object {}\n\nenum E with M {\n  v\n}\n";
    assert_eq!(count(&report_ops(source), CODE), 0);
}

/// `enum E with M` where `M on A` (unrelated) reports at the mixin name
/// with `Enum` as the supertype argument.
#[test]
fn enum_with_mixin_on_unrelated_reports_enum_supertype() {
    let source = "class A {}\n\nmixin M on A {}\n\nenum E with M {\n  v\n}\n";
    let report = report_ops(source);
    assert_eq!(spans(&report, CODE), vec![nth_span_of(source, "M", 1)]);
    assert_eq!(
        messages(&report, CODE),
        vec!["'M' can't be mixed onto 'Enum' because 'Enum' doesn't implement 'A'.".to_string()]
    );
}

// —— `X <: Object` trivial satisfaction ——

/// `class C with M` where `M on Object` is satisfied: a class without
/// `extends` has supertype `Object` and `Object <: Object`.
#[test]
fn class_with_mixin_on_object_is_silent() {
    let source = "mixin M on Object {}\n\nclass C with M {}\n";
    assert_eq!(count(&report_ops(source), CODE), 0);
}

/// `class C extends A with M` where `M on Object` is satisfied:
/// `A <: Object` holds for every interface type.
#[test]
fn class_extends_with_mixin_on_object_is_silent() {
    let source = "class A {}\n\nmixin M on Object {}\n\nclass C extends A with M {}\n";
    assert_eq!(count(&report_ops(source), CODE), 0);
}

// —— cross-file supertype resolution ——

/// `class C extends A with M` where `A` is imported from a library and
/// `A extends B`, `M on B`: the constraint walk must resolve `A` in its
/// own library and find `B` on its supertype chain.
#[test]
fn imported_superclass_satisfies_via_its_own_chain() {
    let source = "import 'package:t/lib.dart';\n\nclass C extends A with M {}\n";
    let lib = RigImport {
        uri: "package:t/lib.dart",
        prefix: None,
        source: "class B {}\nclass A extends B {}\nmixin M on B {}\n",
    };
    let report = report_ops_imports(source, &[lib]);
    assert_eq!(count(&report, CODE), 0);
}

/// The negative control: `class C extends A with M` where `A` is imported
/// and `M on B` is unrelated to `A`'s chain — emits at the mixin name.
#[test]
fn imported_superclass_with_unrelated_constraint_reports() {
    let source = "import 'package:t/lib.dart';\n\nclass C extends A with M {}\n";
    let lib = RigImport {
        uri: "package:t/lib.dart",
        prefix: None,
        source: "class A {}\nclass B {}\nmixin M on B {}\n",
    };
    let report = report_ops_imports(source, &[lib]);
    assert_eq!(
        messages(&report, CODE),
        vec!["'M' can't be mixed onto 'A' because 'A' doesn't implement 'B'.".to_string()]
    );
}

// —— still-emitting arms ——

/// `class C with M` where `M on A` (unrelated) still reports at the mixin
/// name — `Object <: A` does not hold.
#[test]
fn class_with_mixin_on_unrelated_reports_object_supertype() {
    let source = "class A {}\n\nmixin M on A {}\n\nclass C with M {}\n";
    let report = report_ops(source);
    assert_eq!(spans(&report, CODE), vec![nth_span_of(source, "M", 1)]);
    assert_eq!(
        messages(&report, CODE),
        vec!["'M' can't be mixed onto 'Object' because 'Object' doesn't implement 'A'.".to_string()]
    );
}

/// `class C extends B with M` where `M on A` and `B` is unrelated to `A`
/// still reports — `B <: A` does not hold.
#[test]
fn class_extends_with_unrelated_constraint_reports() {
    let source = "class A {}\nclass B {}\n\nmixin M on A {}\n\nclass C extends B with M {}\n";
    let report = report_ops(source);
    assert_eq!(spans(&report, CODE), vec![nth_span_of(source, "M", 1)]);
    assert_eq!(
        messages(&report, CODE),
        vec!["'M' can't be mixed onto 'B' because 'B' doesn't implement 'A'.".to_string()]
    );
}
