//! The #3003 pins: the `p.C` prefix-head faces — the oracle's
//! `resolve()` importPrefix arm (upstream Dart SDK source,
//! pkg/analyzer/lib/src/dart/resolver/named_type_resolver.dart:85-128)
//! splits every `p.C` on the head's resolved element: a type-bearing
//! element rewrites to `C.name`, a real `PrefixElement` consults the
//! prefix scope, `null` falls into `reportNullOrNonTypeElement`'s
//! context chain (:596-728) over the whole written `p.C` span naming
//! `C`, and ANY other element — a local, parameter, type parameter,
//! own-declared member, top-level function or variable, `dynamic`, an
//! extension name — reports `prefix_shadowed_by_local_declaration` at
//! the prefix token and marks the whole node `InvalidType`.
//!
//! Oracle provenance: `dart analyze` 3.13.5 (the semantic faces were
//! probed against the real binary; patch-level parity with the pinned
//! 3.13.7 oracle holds on this family).
//!
//! Fixture shapes avoid the `unused_local_variable` face: `is`/`as` operand
//! probes ride expression statements, annotation probes ride return types,
//! and shadowing locals are parameters (an unreferenced parameter is
//! never a diagnostic).

use super::test_rig::{RigImport, assert_silent, code_names, report_ops, report_ops_imports, rows};

/// `new p.C()` where `p` is a top-level function: the importPrefix arm's
/// fall-through reports the shadowed face at the head token.
#[test]
fn new_fn_head_prefixed_creation_reports_prefix_shadowed() {
    let report = report_ops("void nonPrefix() {}\nvoid f() { new nonPrefix.Class(); }\n");
    assert_eq!(
        code_names(&report),
        vec!["prefix_shadowed_by_local_declaration"],
        "{:?}",
        rows(&report)
    );
}

/// `const p.C.named()` (three segments) reports the same face — the
/// shadowed head answers before the constructor selector exists.
#[test]
fn const_fn_head_three_segment_reports_prefix_shadowed() {
    let report = report_ops("void nonPrefix() {}\nvoid f() { const nonPrefix.Class.named(); }\n");
    assert_eq!(
        code_names(&report),
        vec!["prefix_shadowed_by_local_declaration"],
        "{:?}",
        rows(&report)
    );
}

/// A top-level variable head shadows identically.
#[test]
fn new_var_head_prefixed_creation_reports_prefix_shadowed() {
    let report = report_ops("var v = 1;\nvoid f() { new v.Class(); }\n");
    assert_eq!(
        code_names(&report),
        vec!["prefix_shadowed_by_local_declaration"],
        "{:?}",
        rows(&report)
    );
}

/// A bare keyworded head (`new nonPrefix()`) keeps the creation face —
/// no `.` means the head is the type itself, not a prefix.
#[test]
fn new_fn_head_bare_reports_creation_with_non_type() {
    let report = report_ops("void nonPrefix() {}\nvoid f() { new nonPrefix(); }\n");
    assert_eq!(
        code_names(&report),
        vec!["creation_with_non_type"],
        "{:?}",
        rows(&report)
    );
}

/// A missing head followed by `.` reports `undefined_identifier` over
/// the written `missing.Class` span (the `importPrefix.element == null`
/// arm of `reportNewWithNonType`, named_type_resolver.dart:566-590).
#[test]
fn new_missing_head_prefixed_reports_undefined_identifier() {
    let report = report_ops("void f() { new missing.Class(); }\n");
    assert_eq!(
        code_names(&report),
        vec!["undefined_identifier"],
        "{:?}",
        rows(&report)
    );
}

/// A bare missing head keeps `creation_with_non_type`.
#[test]
fn new_missing_head_bare_reports_creation_with_non_type() {
    let report = report_ops("void f() { new missing(); }\n");
    assert_eq!(
        code_names(&report),
        vec!["creation_with_non_type"],
        "{:?}",
        rows(&report)
    );
}

/// `p.C` in a type position where `p` is a top-level function: the same
/// importPrefix arm fires — the use never counts as a use of the
/// shadowing element.
#[test]
fn annotation_fn_head_reports_prefix_shadowed() {
    let report = report_ops("void topFn() {}\ntopFn.Class? f() => null;\n");
    assert_eq!(
        code_names(&report),
        vec!["prefix_shadowed_by_local_declaration"],
        "{:?}",
        rows(&report)
    );
}

/// `localVar.Class` in a type position shadows through a parameter.
#[test]
fn annotation_local_head_reports_prefix_shadowed() {
    let report = report_ops("void f(int localVar) { localVar.Class? g() => null; g(); }\n");
    assert_eq!(
        code_names(&report),
        vec!["prefix_shadowed_by_local_declaration"],
        "{:?}",
        rows(&report)
    );
}

/// `p.C` in a type position where `p` misses every scope: the whole
/// `p.C` resolves to `InvalidType` — `undefined_class` names the
/// continuation over the written `p.C` span
/// (`reportNullOrNonTypeElement`'s fall-through,
/// named_type_resolver.dart:722-727).
#[test]
fn annotation_missing_head_reports_undefined_class() {
    let report = report_ops("missing.Class? f() => null;\n");
    assert_eq!(
        code_names(&report),
        vec!["undefined_class"],
        "{:?}",
        rows(&report)
    );
}

/// `1 is p.C` with a missing head reports `type_test_with_undefined_name`
/// over the `p.C` span — the `is` operand context arm.
#[test]
fn is_operand_missing_head_reports_type_test_with_undefined_name() {
    let report = report_ops("void f() { if (1 is missing.Class) {} }\n");
    assert_eq!(
        code_names(&report),
        vec!["type_test_with_undefined_name"],
        "{:?}",
        rows(&report)
    );
}

/// `1 as p.C` with a missing head reports `cast_to_non_type` over the
/// `p.C` span — the `as` operand context arm.
#[test]
fn as_operand_missing_head_reports_cast_to_non_type() {
    let report = report_ops("void f() { 1 as missing.Class; }\n");
    assert_eq!(
        code_names(&report),
        vec!["cast_to_non_type"],
        "{:?}",
        rows(&report)
    );
}

/// `List<missing.Class>` in a return type reports
/// `non_type_as_type_argument` over the `p.C` span.
#[test]
fn type_arg_missing_head_reports_non_type_as_type_argument() {
    let report = report_ops("List<missing.Class>? f() => null;\n");
    assert_eq!(
        code_names(&report),
        vec!["non_type_as_type_argument"],
        "{:?}",
        rows(&report)
    );
}

/// `on missing.C` in a catch clause reports `non_type_in_catch_clause`.
#[test]
fn catch_on_missing_head_reports_non_type_in_catch_clause() {
    let report = report_ops("void f() { try {} on missing.Class {} }\n");
    assert_eq!(
        code_names(&report),
        vec!["non_type_in_catch_clause"],
        "{:?}",
        rows(&report)
    );
}

/// `1 as topVar` — a resolved non-type element inside the `as` operand
/// reports `cast_to_non_type`, never `not_a_type` (the
/// `_isTypeInAsExpression` arm outranks the `element != null` tail,
/// named_type_resolver.dart:621-629).
#[test]
fn as_operand_nontype_head_reports_cast_to_non_type() {
    let report = report_ops("var topVar = 1;\nvoid f() { 1 as topVar; }\n");
    assert_eq!(
        code_names(&report),
        vec!["cast_to_non_type"],
        "{:?}",
        rows(&report)
    );
}

/// `1 as localVar` — a parameter inside the `as` operand reports the
/// same (the context chain fires before the `LocalVariableElement` arm).
#[test]
fn as_operand_local_head_reports_cast_to_non_type() {
    let report = report_ops("void f(int localVar) { 1 as localVar; }\n");
    assert_eq!(
        code_names(&report),
        vec!["cast_to_non_type"],
        "{:?}",
        rows(&report)
    );
}

/// `1 as List<topVar>` — a non-type element in the operand's own
/// type-argument list reports `non_type_as_type_argument`.
#[test]
fn as_operand_type_arg_nontype_reports() {
    let report = report_ops("var topVar = 1;\nvoid f() { 1 as List<topVar>; }\n");
    assert_eq!(
        code_names(&report),
        vec!["non_type_as_type_argument"],
        "{:?}",
        rows(&report)
    );
}

/// `C.Class` in a type position — the head resolves to a class — is the
/// `_rewriteToConstructorName` non-creation arm: `not_a_type 'C.Class'`
/// over the written span (named_type_resolver.dart:105-113/:373-457).
#[test]
fn annotation_type_head_reports_not_a_type() {
    let report = report_ops("class C {}\nC.Class? f() => null;\n");
    assert_eq!(
        code_names(&report),
        vec!["not_a_type"],
        "{:?}",
        rows(&report)
    );
}

/// An enum head answers the same rewrite face.
#[test]
fn annotation_enum_head_reports_not_a_type() {
    let report = report_ops("enum E { a }\nE.Class? f() => null;\n");
    assert_eq!(
        code_names(&report),
        vec!["not_a_type"],
        "{:?}",
        rows(&report)
    );
}

/// `1 is En.Class` — the rewrite arm outranks the `is` operand context:
/// `not_a_type`, never `type_test_with_undefined_name`.
#[test]
fn is_operand_type_head_reports_not_a_type() {
    let report = report_ops("enum E { a }\nvoid f() { if (1 is E.Class) {} }\n");
    assert_eq!(
        code_names(&report),
        vec!["not_a_type"],
        "{:?}",
        rows(&report)
    );
}

/// `dynamic.Class` — `dynamic` is a `DynamicElementImpl` upstream, never
/// a prefix: the shadowed face fires.
#[test]
fn annotation_dynamic_head_reports_prefix_shadowed() {
    let report = report_ops("dynamic.Class? f() => null;\n");
    assert_eq!(
        code_names(&report),
        vec!["prefix_shadowed_by_local_declaration"],
        "{:?}",
        rows(&report)
    );
}

/// An inherited member's `x.C` reads `undefined_class` — the upstream
/// `enclosingClass` member fallback never climbs (declared members
/// only), so `field` answers `null` and the context chain reports the
/// continuation name over the written span.
#[test]
fn annotation_inherited_member_head_reports_undefined_class() {
    let report =
        report_ops("class O { int field = 0; }\nclass I extends O { field.Class? m() => null; }\n");
    assert_eq!(
        code_names(&report),
        vec!["undefined_class"],
        "{:?}",
        rows(&report)
    );
}

/// An own-declared member's `x.C` is the shadowed face.
#[test]
fn annotation_own_member_head_reports_prefix_shadowed() {
    let report = report_ops("class I { int field = 0; field.Class? m() => null; }\n");
    assert_eq!(
        code_names(&report),
        vec!["prefix_shadowed_by_local_declaration"],
        "{:?}",
        rows(&report)
    );
}

/// `new p.C()` where `p` is an inherited member: the declared-only
/// fallback answers `null`, so `p.C` resolves to `InvalidType` —
/// `undefined_identifier` over the written `p.C` (the creation
/// importPrefix-miss arm, named_type_resolver.dart:566-590).
#[test]
fn new_inherited_member_head_reports_undefined_identifier() {
    let report = report_ops(
        "class O { int field = 0; }\nclass I extends O { void m() { new field.Class(); } }\n",
    );
    assert_eq!(
        code_names(&report),
        vec!["undefined_identifier"],
        "{:?}",
        rows(&report)
    );
}

/// A type parameter's `T.C` shadows identically.
#[test]
fn annotation_type_param_head_reports_prefix_shadowed() {
    let report = report_ops("class B<T> { T.Class? f() => null; }\n");
    assert_eq!(
        code_names(&report),
        vec!["prefix_shadowed_by_local_declaration"],
        "{:?}",
        rows(&report)
    );
}

/// A real prefix head + a missing class keeps the prefixed
/// `undefined_class` (the prefix-scope lookup misses —
/// named_type_resolver.dart:140-147 + `reportNullOrNonTypeElement`'s
/// null-element arm).
#[test]
fn annotation_real_prefix_missing_class_reports_undefined_class() {
    let report = report_ops_imports(
        "p.Class? f() => null;\n",
        &[RigImport {
            uri: "package:t/a.dart",
            prefix: Some("p"),
            source: "class Real {}\n",
        }],
    );
    assert_eq!(
        code_names(&report),
        vec!["undefined_class"],
        "{:?}",
        rows(&report)
    );
}

/// `1 is missing` (bare, no `.`) keeps the operand's undefined-name face.
#[test]
fn is_operand_bare_missing_reports_type_test_with_undefined_name() {
    let report = report_ops("void f() { if (1 is missing) {} }\n");
    assert_eq!(
        code_names(&report),
        vec!["type_test_with_undefined_name"],
        "{:?}",
        rows(&report)
    );
}

/// `1 as missing` (bare) keeps `cast_to_non_type`.
#[test]
fn as_operand_bare_missing_reports_cast_to_non_type() {
    let report = report_ops("void f() { 1 as missing; }\n");
    assert_eq!(
        code_names(&report),
        vec!["cast_to_non_type"],
        "{:?}",
        rows(&report)
    );
}

/// A resolved `p.C` use still credits the prefix — the unused-import
/// face's head must not die under the shadowed/miss rewrites.
#[test]
fn annotation_resolved_prefix_stays_silent() {
    let report = report_ops_imports(
        "p.Real? f() => null;\n",
        &[RigImport {
            uri: "package:t/a.dart",
            prefix: Some("p"),
            source: "class Real {}\n",
        }],
    );
    assert_silent(&report);
}
