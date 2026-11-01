//! The #3319 pins: `invalid_use_of_type_outside_library`'s residual faces
//! after #3161 locked the extends/implements/`with`/`on` matrix.
//!
//! The board's `282 / 94 / 188` row predates the current engine (a stale
//! sweep): measured against oracle 3.13.7 through the conformance harness,
//! every case of the home turf (`diagnostics/*outside_of_library`,
//! `features/class-modifiers`, `features/sealed-class`, co19
//! `Class-Modifiers`, 459 cases) is per-code clean except the library-PREFIX
//! clause faces (`implements p.Base`, tracked as the residual) and the
//! `augment` clause faces. What remains unpinned from that measurement is
//! the part of the family the corpus discriminates sharply and a refactor
//! could silently flatten:
//!
//! * the `base mixin` message variant (mixin display, `BaseMixin` row) —
//!   `error_verifier.dart:3712-3752`'s mixin arm;
//! * the sealed pass on an `implements` entry and on an `on` entry (the
//!   multi-clause drive: `_checkForSealedSupertypeOutsideOfLibrary` rides
//!   the class drive's implements walk and the mixin drive's `on` walk);
//! * the cross-library plain-class negative: a class that carries no
//!   modifier imposes nothing, so the row must not fire — the
//!   over-emission direction the stale board's fails lived in.
//!
//! Oracle provenance: `dart analyze` 3.13.7 over the conformance corpus
//! (`diagnostics/base_mixin_implemented_outside_of_library`,
//! `diagnostics/sealed_class_subtype_outside_of_library`).
use super::test_rig::{CORE_PRELUDE, RigImport, report_ops_imports};

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

/// `class C implements M` over a `base mixin M` in another library reports
/// the MIXIN message variant, anchored on the written entry
/// (`base_mixin_implemented_outside_of_library`; the class wording of the
/// base arm must not leak into it).
#[test]
fn base_mixin_implemented_outside_reports_the_mixin_variant() {
    let fixture = "import 'lib.dart';\nclass C implements M {}\n";
    let report = report_ops_imports(fixture, &[dep("base mixin M {}\n")]);
    assert_eq!(
        outside_rows(&report),
        vec![(
            at(fixture, "M {}"),
            1,
            "The mixin 'M' can't be implemented outside of its library because it's a base mixin."
                .to_string()
        )]
    );
}

/// `class C implements S` over a `sealed class S` reports the sealed pass on
/// the implements entry — the same code and message the `with` face carries,
/// so only the clause drive tells them apart.
#[test]
fn sealed_class_implemented_outside_reports_the_written_entry() {
    let fixture = "import 'lib.dart';\nclass C implements S {}\n";
    let report = report_ops_imports(fixture, &[dep("sealed class S {}\n")]);
    assert_eq!(
        outside_rows(&report),
        vec![(
            at(fixture, "S {}"),
            1,
            "The class 'S' can't be extended, implemented, or mixed in outside of its library \
             because it's a sealed class."
                .to_string()
        )]
    );
}

/// The mixin drive's `on` face over the same sealed class: a `mixin M on S`
/// outside `S`'s library reports at the `on` entry.
#[test]
fn sealed_class_mixin_constraint_outside_reports_the_on_entry() {
    let fixture = "import 'lib.dart';\nmixin M on S {}\n";
    let report = report_ops_imports(fixture, &[dep("sealed class S {}\n")]);
    assert_eq!(
        outside_rows(&report),
        vec![(
            at(fixture, "S {}"),
            1,
            "The class 'S' can't be extended, implemented, or mixed in outside of its library \
             because it's a sealed class."
                .to_string()
        )]
    );
}

/// The over-emission negative: a class with NO class modifier (and none
/// induced — no supertype) is usable from any library, so `implements`,
/// `extends`, `with` and `on` over it report nothing. This is the direction
/// the stale board's 188 fails lived in.
#[test]
fn unmodified_cross_library_super_faces_are_silent() {
    let fixture =
        "import 'lib.dart';\nclass A extends P {}\nclass B implements P {}\nmixin MW on P {}\n";
    let report = report_ops_imports(fixture, &[dep("class P {}\n")]);
    assert!(outside_rows(&report).is_empty(), "{:?}", outside_rows(&report));
}
