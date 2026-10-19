use crate::testsupport::{Fixture, one_rule, tok, utf16_at};

/// `test_classBase_classFinal` — a `base` subclass of a `final`
/// superclass reports `final` at the name.
#[test]
fn base_subclass_of_final_reports() {
    let f = Fixture::parse("final class F {}\nbase class B extends F {}\n");
    let lints = f.run(&one_rule("implicit_reopen"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "The class 'B' reopens 'F' because it is not marked 'final'."
    );
    assert_eq!(
        lints[0].correction.as_deref(),
        Some("Try marking 'B' 'final' or annotating it with '@reopen'.")
    );
    let name = tok(&f.source, &f.tokens, "B", 0);
    assert_eq!(lints[0].range.start, utf16_at(&f, name));
}

/// `test_classBase_classInterface` — a `base` subclass of an
/// `interface` superclass reports `interface`.
#[test]
fn base_subclass_of_interface_reports() {
    let f = Fixture::parse("interface class I {}\nbase class B extends I {}\n");
    let lints = f.run(&one_rule("implicit_reopen"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "The class 'B' reopens 'I' because it is not marked 'interface'."
    );
}

/// `test_class_classInterface` — a modifier-less subclass of an
/// `interface` superclass reports `interface`.
#[test]
fn modifierless_subclass_of_interface_reports() {
    let f = Fixture::parse("interface class I {}\nclass C extends I {}\n");
    let lints = f.run(&one_rule("implicit_reopen"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "The class 'C' reopens 'I' because it is not marked 'interface'."
    );
}

/// `test_classFinal_classInterface_ok` — a `final` subclass never
/// reports.
#[test]
fn final_subclass_of_interface_passes() {
    let f = Fixture::parse("interface class I {}\nfinal class C extends I {}\n");
    assert!(f.run(&one_rule("implicit_reopen")).is_empty());
}

/// `test_class_classInterface_reopened_ok` — the `@reopen` annotation
/// silences the report.
#[test]
fn reopen_annotation_passes() {
    let f = Fixture::parse("@reopen\nclass C extends I {}\ninterface class I {}\n");
    assert!(f.run(&one_rule("implicit_reopen")).is_empty());
}

/// A `sealed` subclass never reports (`element.isSealed` :50).
#[test]
fn sealed_subclass_passes() {
    let f = Fixture::parse("interface class I {}\nsealed class S extends I {}\n");
    assert!(f.run(&one_rule("implicit_reopen")).is_empty());
}

/// `test_inducedFinal` — the `sealed` supertype induces `final` from
/// its own `final` superclass (`_setInducedModifier`), so the `base`
/// subclass reports `final`.
#[test]
fn induced_final_reports() {
    let f = Fixture::parse(
        "final class F {}\nsealed class S extends F {}\nbase class C extends S {}\n",
    );
    let lints = f.run(&one_rule("implicit_reopen"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "The class 'C' reopens 'S' because it is not marked 'final'."
    );
}

/// `test_inducedFinal_base_interface` — an `interface` superclass with
/// a `base` superinterface induces `final`.
#[test]
fn induced_final_base_interface_reports() {
    let f = Fixture::parse(
        "base class C {}\ninterface class D {}\nsealed class E extends D implements C {}\nbase class B extends E {}\n",
    );
    let lints = f.run(&one_rule("implicit_reopen"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "The class 'B' reopens 'E' because it is not marked 'final'."
    );
}

/// `test_inducedFinal_mixin_finalClass` — a `final` superclass reached
/// through a `with` mixin induces `final`.
#[test]
fn induced_final_mixin_final_class_reports() {
    let f = Fixture::parse(
        "final class S {}\nmixin M {}\nsealed class A extends S with M {}\nbase class B extends A {}\n",
    );
    let lints = f.run(&one_rule("implicit_reopen"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "The class 'B' reopens 'A' because it is not marked 'final'."
    );
}

/// `test_inducedInterface` — the `sealed` supertype of an `interface`
/// superclass induces `interface`; the modifier-less subclass reports.
#[test]
fn induced_interface_reports() {
    let f = Fixture::parse(
        "interface class I {}\nsealed class S extends I {}\nclass C extends S {}\n",
    );
    let lints = f.run(&one_rule("implicit_reopen"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "The class 'C' reopens 'S' because it is not marked 'interface'."
    );
}

/// `test_inducedInterface_base` — a `base` subclass of an
/// `interface`-induced sealed reports `interface`.
#[test]
fn induced_interface_base_reports() {
    let f = Fixture::parse(
        "interface class I {}\nsealed class S extends I {}\nbase class C extends S {}\n",
    );
    let lints = f.run(&one_rule("implicit_reopen"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "The class 'C' reopens 'S' because it is not marked 'interface'."
    );
}

/// `test_inducedInterface_twoLevels` — the induction chains through
/// nested sealed declarations.
#[test]
fn induced_interface_two_levels_reports() {
    let f = Fixture::parse(
        "interface class I {}\nsealed class S extends I {}\nsealed class S2 extends S {}\nclass C extends S2 {}\n",
    );
    let lints = f.run(&one_rule("implicit_reopen"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "The class 'C' reopens 'S2' because it is not marked 'interface'."
    );
}

/// `test_classTypeAlias_class_classInterface` — the mixin-application
/// face reports at the alias name.
#[test]
fn mixin_application_reports() {
    let f = Fixture::parse("interface class I {}\nmixin M {}\nclass C = I with M;\n");
    let lints = f.run(&one_rule("implicit_reopen"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "The class 'C' reopens 'I' because it is not marked 'interface'."
    );
    let name = tok(&f.source, &f.tokens, "C", 0);
    assert_eq!(lints[0].range.start, utf16_at(&f, name));
}

/// `test_classTypeAlias_classBase_classFinal` — a `base` alias of a
/// `final` class reports `final`.
#[test]
fn mixin_application_base_of_final_reports() {
    let f = Fixture::parse("final class C {}\nmixin M {}\nbase class D = C with M;\n");
    let lints = f.run(&one_rule("implicit_reopen"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "The class 'D' reopens 'C' because it is not marked 'final'."
    );
}

/// `test_classTypeAlias_classBase_classFinal_reopened` — `@reopen`
/// silences the alias face.
#[test]
fn mixin_application_reopen_passes() {
    let f = Fixture::parse("@reopen\nbase class D = C with M;\nfinal class C {}\nmixin M {}\n");
    assert!(f.run(&one_rule("implicit_reopen")).is_empty());
}
