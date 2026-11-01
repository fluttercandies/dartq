//! The #3456 pins — the `super` face's two-stage lookup: the concrete
//! dispatch walk (`getMember(element, name, forSuper: true)` — the
//! `superImplemented` map) answers `super.m` first; a miss retries through
//! the inherited interface (`getInherited` — `implements` signatures and
//! abstract members included), which reports
//! `abstract_super_member_reference` instead of `undefined_super_member`.
//! Inside `extension`/`extension type` bodies `super` resolves against
//! `InvalidType` upstream, so the member lookup never runs at all
//! (`SuperContext.of != valid`,
//! method_invocation_resolver.dart:947-955).
//!
//! Oracle provenance: `dart analyze` 3.13.7 — upstream
//! `AbstractSuperMemberReferenceTest` (`test_methodInvocation_mixin_implements`,
//! `test_propertyAccess_getter_superImplements`,
//! `test_methodInvocation_superHasAbstract`,
//! `test_methodInvocation_mixinHasNoSuchMethod`,
//! `test_propertyAccess_setter`) and the `SuperInExtensionTest` /
//! `SuperInExtensionTypeTest` silences.

use super::test_rig::{assert_silent, code_names, report_ops};

/// `super.foo(0)` inside `mixin M implements A` (a concrete `A.foo` on an
/// `implements` face — never a dispatch edge) reports
/// `abstract_super_member_reference`, not `undefined_super_member`.
#[test]
fn super_invoke_implements_signature_reports_abstract() {
    let report = report_ops(
        "class A {\n\
           void foo(int _) {}\n\
         }\n\
         mixin M implements A {\n\
           void bar() {\n\
             super.foo(0);\n\
           }\n\
         }\n",
    );
    let names = code_names(&report);
    assert!(
        names.contains(&"abstract_super_member_reference"),
        "expected abstract_super_member_reference, got {names:?}"
    );
    assert!(
        !names.contains(&"undefined_super_member"),
        "unexpected undefined_super_member beside the abstract face, got {names:?}"
    );
}

/// `super.foo` read where an abstract superclass member is the only
/// carrier — upstream's `test_propertyAccess_getter` /
/// `test_methodInvocation_superHasAbstract` shape.
#[test]
fn super_read_abstract_superclass_member_reports_abstract() {
    let report = report_ops(
        "abstract class A {\n\
           int get foo;\n\
         }\n\
         abstract class B extends A {\n\
           int bar() {\n\
             return super.foo;\n\
           }\n\
         }\n",
    );
    let names = code_names(&report);
    assert!(
        names.contains(&"abstract_super_member_reference"),
        "expected abstract_super_member_reference, got {names:?}"
    );
    assert!(
        !names.contains(&"undefined_super_member"),
        "unexpected undefined_super_member beside the abstract face, got {names:?}"
    );
}

/// `class C extends B` where `B implements A` — the concrete `A.foo`
/// getter lives on B's `implements` face (no dispatch), so `super.foo`
/// reports `abstract_super_member_reference` naming the getter.
#[test]
fn super_read_implements_concrete_reports_abstract() {
    let report = report_ops(
        "class A {\n\
           int get foo => 0;\n\
         }\n\
         abstract class B implements A {}\n\
         class C extends B {\n\
           int get foo => super.foo;\n\
         }\n",
    );
    let names = code_names(&report);
    assert!(
        names.contains(&"abstract_super_member_reference"),
        "expected abstract_super_member_reference, got {names:?}"
    );
}

/// `super.foo = v` where the only setter is an abstract one on the
/// superclass — upstream's `test_propertyAccess_setter`: the write leg's
/// `lookUpSetter(inherited)` catches it as `abstract_super_member_reference`
/// with the setter kind word.
#[test]
fn super_write_abstract_setter_reports_abstract() {
    let report = report_ops(
        "abstract class A {\n\
           set foo(int _);\n\
         }\n\
         abstract class B extends A {\n\
           void bar() {\n\
             super.foo = 0;\n\
           }\n\
         }\n",
    );
    let names = code_names(&report);
    assert!(
        names.contains(&"abstract_super_member_reference"),
        "expected abstract_super_member_reference, got {names:?}"
    );
    assert!(
        !names.contains(&"undefined_super_member"),
        "unexpected undefined_super_member beside the abstract face, got {names:?}"
    );
}

/// `super.foo()` where a concrete member exists through a mixin
/// application — upstream's `test_methodInvocation_mixinHasConcrete`
/// (silent): the dispatch walk must still find concrete members applied
/// via `with`.
#[test]
fn super_invoke_mixin_concrete_resolves() {
    let report = report_ops(
        "class S {}\n\
         mixin A {\n\
           void foo(int _) {}\n\
         }\n\
         class B extends S with A {\n\
           void bar() {\n\
             super.foo(0);\n\
           }\n\
         }\n",
    );
    assert_silent(&report);
}

/// `super` inside an extension body never runs the member lookup — the
/// context face (`super_in_extension`, owned elsewhere) reports; no
/// `undefined_super_member` may ride beside it. Upstream's
/// `SuperInExtensionTest.test_methodInvocation_field_instance_late` and
/// `test_getter_inSetter` shapes.
#[test]
fn super_inside_extension_emits_no_member_row() {
    let report = report_ops(
        "extension E on int {\n\
           set setter(int v) {\n\
             super.value = v;\n\
           }\n\
           int get value => super.hashCode;\n\
         }\n",
    );
    let names = code_names(&report);
    assert!(
        !names.contains(&"undefined_super_member"),
        "unexpected undefined_super_member inside an extension, got {names:?}"
    );
}

/// `super` inside an extension-type body: same InvalidType posture —
/// `SuperInExtensionTypeTest.test_methodInvocation`/`test_propertyAccess`
/// keep the member face silent.
#[test]
fn super_inside_extension_type_emits_no_member_row() {
    let report = report_ops(
        "extension type A(int it) {\n\
           void bar() {\n\
             super.foo();\n\
             super.foo;\n\
           }\n\
         }\n",
    );
    let names = code_names(&report);
    assert!(
        !names.contains(&"undefined_super_member"),
        "unexpected undefined_super_member inside an extension type, got {names:?}"
    );
}

/// `super[i]` inside an extension body: the operator receiver arm is
/// silenced the same way — `SuperInExtensionTest.test_indexOperator_inMethod`
/// reports the context face alone.
#[test]
fn super_index_inside_extension_emits_no_member_row() {
    let report = report_ops(
        "extension E on int {\n\
           int bar() {\n\
             return super[0];\n\
           }\n\
         }\n",
    );
    let names = code_names(&report);
    assert!(
        !names.contains(&"undefined_super_member"),
        "unexpected undefined_super_member inside an extension, got {names:?}"
    );
}
