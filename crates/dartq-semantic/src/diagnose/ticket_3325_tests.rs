//! The #3325 pins — three residual faces of the override / class-modifier
//! walk that `overrides.rs` owns:
//!
//! * a supertype clause written through an import prefix (`implements
//!   p.Base`, `with p.Final`) reached the element model as its BARE leaf,
//!   and `resolve_super` walked the local scope alone — the face resolved
//!   nowhere, so every outside-library arm stayed silent (the #3319
//!   residual, 2 corpus cases) and the `with`-clause teardown could not
//!   resolve a dotted entry at all (`class_used_as_mixin`);
//! * a private member name is scoped to its DECLARING library: the oracle
//!   keys the inheritance map on `(name, library)`, so a same-spelled `_m`
//!   in a foreign library is a different element, never an override face
//!   (the #3301 residual H, 1 case);
//! * an UNMODELED target slot (`Ty::Unknown`) carries no comparable type,
//!   so it can never fail an override — before the guard every arm below
//!   it answered `false` from a blank and fabricated `invalid_override`
//!   (the #3301 residual I, 3 cases).
//!
//! Oracle provenance: `dart analyze` over the conformance corpus — the
//! outside-library family (`features/class-modifiers`), the private-member
//! override shapes, and the `field/override4_test_01` /
//! `mixin{,legacy}/accessor_test` fabricated rows.
use super::test_rig::{CORE_PRELUDE, RigImport, report_ops, report_ops_imports};

/// The absolute fixture offset of `needle` (the prelude the rig prepends
/// included; the fixtures are ASCII, so byte offset == UTF-16 offset).
fn at(fixture: &str, needle: &str) -> u32 {
    (CORE_PRELUDE.len() + fixture.find(needle).unwrap()) as u32
}

/// The `invalid_override` rows — the face under test, filtered from the
/// noise a bare rig fixture necessarily carries (the prelude's declarations
/// make a later `import` a `directive_after_declaration`; an uninitialized
/// non-nullable field reports its own row; a private member never read is
/// `unused_element`).
fn override_rows(report: &crate::diagnose::FileReport) -> usize {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "invalid_override")
        .count()
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

/// One prefixed dependency library.
fn prefixed(source: &'static str) -> RigImport<'static> {
    RigImport {
        uri: "lib.dart",
        prefix: Some("p"),
        source,
    }
}

/// `class C implements p.Base` over a `base class Base` in another library
/// reports the outside-library row anchored on the WHOLE written entry
/// (`p.Base`, prefix head through local end) — the face was silent before
/// the prefix resolution (#3319 residual).
#[test]
fn prefixed_implements_face_reports_outside_library() {
    let fixture = "import 'lib.dart' as p;\nclass C implements p.Base {}\n";
    let report = report_ops_imports(fixture, &[prefixed("base class Base {}\n")]);
    assert_eq!(
        outside_rows(&report),
        vec![(
            at(fixture, "p.Base"),
            6,
            "The class 'Base' can't be implemented outside of its library because it's a base \
             class."
                .to_string()
        )]
    );
}

/// The bare spelling keeps resolving through the same walk (the prefix arm
/// must not shadow it): `class C implements Base` over the same dependency
/// reports the same row.
#[test]
fn bare_implements_face_still_reports_outside_library() {
    let fixture = "import 'lib.dart';\nclass C implements Base {}\n";
    let report = report_ops_imports(
        fixture,
        &[RigImport {
            uri: "lib.dart",
            prefix: None,
            source: "base class Base {}\n",
        }],
    );
    assert_eq!(
        outside_rows(&report),
        vec![(
            at(fixture, "Base {}"),
            4,
            "The class 'Base' can't be implemented outside of its library because it's a base \
             class."
                .to_string()
        )]
    );
}

/// `class C with p.Final` over a plain class in another library reports
/// `class_used_as_mixin` at the written entry: the teardown resolves a
/// dotted spelling through the prefix table, and the entry's span now
/// covers the whole `p.Final` NamedType (the completion fires at the
/// prefix token).
#[test]
fn prefixed_with_face_reports_class_used_as_mixin() {
    let fixture = "import 'lib.dart' as p;\nclass C with p.Final {}\n";
    let report = report_ops_imports(fixture, &[prefixed("class Final {}\n")]);
    let rows: Vec<(u32, u32, String)> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "class_used_as_mixin")
        .map(|d| (d.range.offset, d.range.length, d.message.to_string()))
        .collect();
    assert_eq!(
        rows,
        vec![(
            at(fixture, "p.Final"),
            7,
            "The class 'Final' can't be used as a mixin because it's neither a mixin class nor a \
             mixin."
                .to_string()
        )]
    );
}

/// A foreign library's private `_m` is a DIFFERENT member: the override
/// face must miss, so the signature mismatch between it and this library's
/// `_m` reports nothing (the fabricated `invalid_override` the spelling
/// comparison produced).
#[test]
fn foreign_private_member_is_not_an_override_face() {
    let fixture = "import 'lib.dart';\nclass C extends B { int _m() => 1; }\n";
    let report = report_ops_imports(
        fixture,
        &[RigImport {
            uri: "lib.dart",
            prefix: None,
            source: "class B { String _m() => ''; }\n",
        }],
    );
    assert_eq!(override_rows(&report), 0);
}

/// The same pair inside ONE library keeps the override face: the private
/// name is the same element, so the mismatched signature still breaks.
#[test]
fn same_library_private_member_override_still_breaks() {
    let report = report_ops("class B { String _m() => ''; }\nclass C extends B { int _m() => 1; }");
    assert_eq!(override_rows(&report), 1);
}

/// An inherited field's induced accessor faces carry no modeled return slot
/// (`Ty::Unknown`), so the result face is undecidable — a declared setter
/// with a matching parameter, and a declared getter whose field slot was
/// never written, must both stay silent instead of fabricating a break from
/// the blank (the `field/override4_test_01` / `mixin{,legacy}/accessor_test`
/// rows).
#[test]
fn unknown_target_face_cannot_break_an_override() {
    let setter = report_ops("class A { int x; }\nclass B extends A { set x(int v) {} }");
    assert_eq!(override_rows(&setter), 0);
    let getter = report_ops("class A { var x; }\nclass B extends A { int get x => 1; }");
    assert_eq!(override_rows(&getter), 0);
}

/// The control: the parameter face over the SAME induced setter still
/// decides — a declared `String` parameter cannot accept the field's `int`,
/// so the break reports.
#[test]
fn known_parameter_face_still_breaks_the_override() {
    let report = report_ops("class A { int x; }\nclass B extends A { set x(String v) {} }");
    assert_eq!(override_rows(&report), 1);
}
