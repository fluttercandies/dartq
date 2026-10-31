//! The #3161 pins: `invalid_use_of_type_outside_library` — the
//! cross-library class-modifier contract on the written supertype clauses.
//!
//! One machine code carries the whole family: the oracle's
//! `_checkFor*OutsideOfLibrary` passes report `base_class_implemented`,
//! `base_mixin_implemented`, `final_class_extended/implemented`,
//! `final_class_used_as_mixin_constraint`, `interface_class_extended` and
//! `sealed_class_subtype` rows, every one of them anchored on the WRITTEN
//! clause entry (`.at(namedType)` / `.at(superclass)`), naming the class
//! that carries the modifier — never the written alias, never the declaring
//! subtype (upstream Dart SDK source,
//! `pkg/analyzer/lib/src/generated/error_verifier.dart:3317-3352`
//! `_checkForBaseClassOrMixinImplementedOutsideOfLibrary`, `:5157-5240`
//! `_checkForFinalSupertypeOutsideOfLibrary`, `:5507-5530`
//! `_checkForInterfaceClassOrMixinSuperclassOutsideOfLibrary`, `:7019-7034`
//! `_checkForSealedSupertypeOutsideOfLibrary`, and the
//! `base_or_final_type_verifier.dart:239-256` implements-anchored base row).
//!
//! Drive coverage (`_checkClassInheritance` :2327-2370 /
//! `_checkMixinInheritance` :7808-7845): a class drives
//! extends + `with` + implements (the sealed pass) and
//! extends + implements + `on` (the final and interface passes); a mixin
//! drives implements + `on`; an ENUM rides the class drive with a null
//! superclass when it carries a `with`/`implements` clause
//! (`visitEnumDeclaration` → `_checkClassInheritance`, :823-863).
//!
//! A type alias is transparent: the oracle reads `namedType.type` (the
//! resolved interface type), so `implements TypedefBase` over
//! `typedef TypedefBase = BaseClass;` reports the CLASS and anchors on the
//! alias's written span (the co19 Class-modifiers `*_t0x` alias suites).
//!
//! Oracle provenance: `dart analyze` 3.13.7 over the conformance
//! `diagnostics/*_outside_of_library` and co19 Class-modifiers corpora plus
//! the /tmp/ol3161 probe pairs.

use super::test_rig::{CORE_PRELUDE, RigImport, report_ops, report_ops_imports, rows};

/// The absolute fixture offset of `needle` (the prelude the rig prepends
/// included; the fixtures are ASCII, so byte offset == UTF-16 offset).
fn at(fixture: &str, needle: &str) -> u32 {
    (CORE_PRELUDE.len() + fixture.find(needle).unwrap()) as u32
}

/// The `invalid_use_of_type_outside_library` rows as `(offset, len,
/// message)`, sorted — the family's machine-row face.
fn outside_rows(report: &crate::diagnose::FileReport) -> Vec<(u32, u32, String)> {
    let mut v: Vec<(u32, u32, String)> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "invalid_use_of_type_outside_library")
        .map(|d| (d.range.offset, d.range.length, d.message.to_string()))
        .collect();
    v.sort();
    v
}

/// One dependency library holding the modifier-carrying declarations.
fn dep(source: &'static str) -> RigImport<'static> {
    RigImport {
        uri: "lib.dart",
        prefix: None,
        source,
    }
}

// —— extends ————————————————————————————————————————————————————————————

/// `class X extends F` over a `final class F` in another library reports at
/// the written superclass entry, naming the class.
#[test]
fn final_class_extended_outside_reports_the_written_entry() {
    let fixture = "import 'lib.dart';\nclass X extends F {}\n";
    let report = report_ops_imports(fixture, &[dep("final class F {}\n")]);
    assert_eq!(
        outside_rows(&report),
        vec![(
            at(fixture, "F {}"),
            1,
            "The class 'F' can't be extended outside of its library because it's a final class."
                .to_string()
        )],
        "rows: {:?}",
        rows(&report)
    );
}

/// The same shape over an `interface class` reports the interface-extends
/// row at the same anchor.
#[test]
fn interface_class_extended_outside_reports_the_written_entry() {
    let fixture = "import 'lib.dart';\nclass X extends I {}\n";
    let report = report_ops_imports(fixture, &[dep("interface class I {}\n")]);
    assert_eq!(
        outside_rows(&report),
        vec![(
            at(fixture, "I {}"),
            1,
            "The class 'I' can't be extended outside of its library because it's an interface \
             class."
                .to_string()
        )],
        "rows: {:?}",
        rows(&report)
    );
}

// —— implements ————————————————————————————————————————————————————————

/// `class X implements Base` over a `base class Base` in another library.
#[test]
fn base_class_implemented_outside_reports_the_written_entry() {
    let fixture = "import 'lib.dart';\nclass X implements Base {}\n";
    let report = report_ops_imports(fixture, &[dep("base class Base {}\n")]);
    assert_eq!(
        outside_rows(&report),
        vec![(
            at(fixture, "Base {}"),
            4,
            "The class 'Base' can't be implemented outside of its library because it's a base \
             class."
                .to_string()
        )],
        "rows: {:?}",
        rows(&report)
    );
}

/// A `base` ancestor reached transitively is reported ONCE, at the written
/// entry and naming the base ancestor — the declaring subtype's name never
/// appears in the row (the `base_or_final_type_verifier` implements arm is
/// the same diagnostic as the base-implemented walk, and the analyzer's
/// diagnostic set collapses the pair). The sealed supertype's own row rides
/// the same entry beside it (`_checkForSealedSupertypeOutsideOfLibrary`).
#[test]
fn sealed_supertype_through_a_base_ancestor_reports_the_base_once() {
    let fixture = "import 'lib.dart';\nclass X implements Mid {}\n";
    let report = report_ops_imports(
        fixture,
        &[dep(
            "base class Base {}\nsealed class Mid extends Base {}\n",
        )],
    );
    assert_eq!(
        outside_rows(&report),
        vec![
            (
                at(fixture, "Mid {}"),
                3,
                "The class 'Base' can't be implemented outside of its library because it's a base \
                 class."
                    .to_string()
            ),
            (
                at(fixture, "Mid {}"),
                3,
                "The class 'Mid' can't be extended, implemented, or mixed in outside of its library \
                 because it's a sealed class."
                    .to_string()
            ),
        ],
        "rows: {:?}",
        rows(&report)
    );
}

/// `mixin M implements Base` over a `base class`: the class arm of the
/// base-implemented walk answers the mixin's implements clause too.
#[test]
fn mixin_implements_base_class_outside_reports_the_written_entry() {
    let fixture = "import 'lib.dart';\nmixin M implements Base {}\n";
    let report = report_ops_imports(fixture, &[dep("base class Base {}\n")]);
    assert_eq!(
        outside_rows(&report),
        vec![(
            at(fixture, "Base {}"),
            4,
            "The class 'Base' can't be implemented outside of its library because it's a base \
             class."
                .to_string()
        )],
        "rows: {:?}",
        rows(&report)
    );
}

// —— `with` (the sealed drive) ————————————————————————————————————————

/// `class X with S` over a `sealed class S` in another library: the sealed
/// class subtyping vector, reported beside the class-modifier
/// `class_used_as_mixin` row (`_checkForAllMixinErrorCodes` reports that
/// row WITHOUT tripping the gate that guards the sealed pass,
/// error_verifier.dart:2488-2500 vs :2338-2342).
#[test]
fn sealed_class_mixed_in_outside_reports_the_written_entry() {
    let fixture = "import 'lib.dart';\nclass X with S {}\n";
    let report = report_ops_imports(fixture, &[dep("sealed class S {}\n")]);
    assert_eq!(
        outside_rows(&report),
        vec![(
            at(fixture, "S {}"),
            1,
            "The class 'S' can't be extended, implemented, or mixed in outside of its library \
             because it's a sealed class."
                .to_string()
        )],
        "rows: {:?}",
        rows(&report)
    );
}

/// `mixin M with S` is not valid grammar — the mixin drive has no `with`
/// face — but an enum's `with` clause rides the class drive and reports.
#[test]
fn enum_mixed_in_sealed_outside_reports_the_written_entry() {
    let fixture = "import 'lib.dart';\nenum E with S { a }\n";
    let report = report_ops_imports(fixture, &[dep("sealed class S {}\n")]);
    assert_eq!(
        outside_rows(&report),
        vec![(
            at(fixture, "S { a }"),
            1,
            "The class 'S' can't be extended, implemented, or mixed in outside of its library \
             because it's a sealed class."
                .to_string()
        )],
        "rows: {:?}",
        rows(&report)
    );
}

// —— `on` constraints ————————————————————————————————————————————————

/// `mixin M on F` over a `final class F` in another library.
#[test]
fn final_class_as_mixin_constraint_outside_reports_the_written_entry() {
    let fixture = "import 'lib.dart';\nmixin M on F {}\n";
    let report = report_ops_imports(fixture, &[dep("final class F {}\n")]);
    assert_eq!(
        outside_rows(&report),
        vec![(
            at(fixture, "F {}"),
            1,
            "The class 'F' can't be used as a mixin superclass constraint outside of its library \
             because it's a final class."
                .to_string()
        )],
        "rows: {:?}",
        rows(&report)
    );
}

// —— enums (the class drive with a null superclass) ——————————————————————

/// `enum E implements Base` over a `base class Base`: `visitEnumDeclaration`
/// runs the class drive, so the base-implemented row lands exactly like a
/// class's.
#[test]
fn enum_implements_base_outside_reports_the_written_entry() {
    let fixture = "import 'lib.dart';\nenum E implements Base { a }\n";
    let report = report_ops_imports(fixture, &[dep("base class Base {}\n")]);
    assert_eq!(
        outside_rows(&report),
        vec![(
            at(fixture, "Base { a }"),
            4,
            "The class 'Base' can't be implemented outside of its library because it's a base \
             class."
                .to_string()
        )],
        "rows: {:?}",
        rows(&report)
    );
}

// —— type aliases are transparent ————————————————————————————————

/// An alias declared in the USING library (`typedef TBase = Base;` beside the
/// subtype) resolves to the aliased class: the row names `Base` and anchors
/// on the alias's written span.
#[test]
fn local_alias_on_implements_reports_the_aliased_class() {
    let fixture = "import 'lib.dart';\ntypedef TBase = Base;\nclass X implements TBase {}\n";
    let report = report_ops_imports(fixture, &[dep("base class Base {}\n")]);
    assert_eq!(
        outside_rows(&report),
        vec![(
            at(fixture, "TBase {}"),
            5,
            "The class 'Base' can't be implemented outside of its library because it's a base \
             class."
                .to_string()
        )],
        "rows: {:?}",
        rows(&report)
    );
}

/// An alias declared in the DECLARING library (imported through the clause)
/// resolves the same way — and the clause-element-kind pass sees a class, so
/// no `implements_non_class` rides beside the row.
#[test]
fn imported_alias_on_implements_reports_the_aliased_class() {
    let fixture = "import 'lib.dart';\nclass X implements TBase {}\n";
    let report = report_ops_imports(
        fixture,
        &[dep("base class Base {}\ntypedef TBase = Base;\n")],
    );
    assert_eq!(
        outside_rows(&report),
        vec![(
            at(fixture, "TBase {}"),
            5,
            "The class 'Base' can't be implemented outside of its library because it's a base \
             class."
                .to_string()
        )],
        "rows: {:?}",
        rows(&report)
    );
    assert!(
        report
            .diagnostics
            .iter()
            .all(|d| d.code.name != "implements_non_class"),
        "an alias head is a class entry: {:?}",
        rows(&report)
    );
}

/// An imported alias over an `interface class` in the extends slot reports
/// the interface-extends row naming the aliased class.
#[test]
fn imported_alias_on_extends_reports_the_aliased_class() {
    let fixture = "import 'lib.dart';\nclass X extends TI {}\n";
    let report = report_ops_imports(fixture, &[dep("interface class I {}\ntypedef TI = I;\n")]);
    assert_eq!(
        outside_rows(&report),
        vec![(
            at(fixture, "TI {}"),
            2,
            "The class 'I' can't be extended outside of its library because it's an interface \
             class."
                .to_string()
        )],
        "rows: {:?}",
        rows(&report)
    );
}

// —— same-library silence ————————————————————————————————————————————

/// Every clause kind stays silent when the modifier-carrying declaration
/// lives in the SAME library (the contract restricts only other libraries).
#[test]
fn same_library_faces_are_silent() {
    let report = report_ops(
        "base class Base {}\n\
         final class F {}\n\
         sealed class S {}\n\
         interface class I {}\n\
         class A implements Base {}\n\
         class B extends F {}\n\
         class C with S {}\n\
         mixin D on F {}\n\
         class E extends I {}\n",
    );
    assert!(
        outside_rows(&report).is_empty(),
        "rows: {:?}",
        rows(&report)
    );
}
