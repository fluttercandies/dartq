//! The #2972 `subtype_of_disallowed_type` pins: the four clause faces
//! (`extends`/`with`/`implements`/`on`) over the non-subtypable SDK
//! spellings, the shadow test (an in-file same-named declaration is the
//! resolved element and never restricted — upstream
//! `TypeProviderImpl.isNonSubtypableClass`, type_provider.dart:547-555),
//! the core `Enum`-in-`with` exemption (`!mixinType.isDartCoreEnum`,
//! error_verifier.dart:2461-2463), and the bare-alias chase
//! (`typedef T = Null`).
//!
//! The fixtures use the rig's `dart:core` stub spellings (`Null`,
//! `FutureOr`, `Enum`, `Record`) — names the names-only SDK face declares,
//! so nothing unresolved co-fires and each fixture pins exactly the
//! disallowed-subtype row. The prelude rig carries user declarations of
//! `String`/`int`/`bool`/`num`/`double`, which is the shadow face itself.

use super::test_rig::{assert_silent, report_ops, report_ops_bare};

/// The `subtype_of_disallowed_type` rows of `report` as
/// `message @offset` strings — the pins assert these exactly, ignoring
/// any neighbor face a fixture may carry.
fn subtype_rows(report: &super::FileReport) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "subtype_of_disallowed_type")
        .map(|d| format!("{} @{}", d.message, d.range.offset))
        .collect()
}

/// The four clause faces report once each with the clause's own problem
/// text (messages.yaml :17297/:17358/:17374/:17366).
#[test]
fn disallowed_subtype_reports_per_clause_face() {
    let extends = report_ops_bare("class B extends Null {}");
    assert_eq!(
        subtype_rows(&extends),
        ["Classes can't extend 'Null'. @6"],
        "{:?}",
        subtype_rows(&extends)
    );
    let with = report_ops_bare("class C with Record {}");
    assert_eq!(
        subtype_rows(&with),
        ["Classes can't mixin 'Record'. @6"],
        "{:?}",
        subtype_rows(&with)
    );
    let implements = report_ops_bare("class D implements FutureOr {}");
    assert_eq!(
        subtype_rows(&implements),
        ["Classes and mixins can't implement 'FutureOr'. @6"],
        "{:?}",
        subtype_rows(&implements)
    );
    let on = report_ops_bare("mixin M on Null {}");
    assert_eq!(
        subtype_rows(&on),
        ["'Null' can't be used as a superclass constraint. @6"],
        "{:?}",
        subtype_rows(&on)
    );
}

/// One declaration's every clause face reports, in source order of the
/// declaring names: a mixin's `on` constraint beside its `implements`
/// entry, and an enum's `with` entry beside its own `implements` (the
/// per-kind clause split, error_verifier.dart:855-864/:7816-7817).
#[test]
fn disallowed_subtype_covers_every_entry_per_kind() {
    let mixin = report_ops_bare("mixin M on Null implements Record {}");
    assert_eq!(
        subtype_rows(&mixin),
        [
            "'Null' can't be used as a superclass constraint. @6",
            "Classes and mixins can't implement 'Record'. @6",
        ],
        "{:?}",
        subtype_rows(&mixin)
    );
    let enumeration = report_ops_bare("enum E with FutureOr implements Null { a }");
    assert_eq!(
        subtype_rows(&enumeration),
        [
            "Classes can't mixin 'FutureOr'. @5",
            "Classes and mixins can't implement 'Null'. @5",
        ],
        "{:?}",
        subtype_rows(&enumeration)
    );
}

/// A name the analysis set resolves to its own declaration is that
/// element, never the SDK face — the prelude's user `String` shadows the
/// restricted spelling, exactly as a workspace class named `String` does
/// upstream (`isNonSubtypableClass` keys on the resolved element's
/// library).
#[test]
fn disallowed_subtype_shadowed_by_user_declaration() {
    let report = report_ops("class B extends String {}");
    assert!(
        subtype_rows(&report).is_empty(),
        "{:?}",
        subtype_rows(&report)
    );
    assert_silent(&report);
}

/// The core `Enum` in a `with` clause rides the `!mixinType.isDartCoreEnum`
/// exemption (error_verifier.dart:2461-2463): the subtype row never fires
/// there, while `implements Enum` keeps reporting.
#[test]
fn disallowed_subtype_core_enum_exempt_in_with_only() {
    let with = report_ops_bare("class B with Enum {}");
    assert!(subtype_rows(&with).is_empty(), "{:?}", subtype_rows(&with));
    let implements = report_ops_bare("class B implements Enum {}");
    assert_eq!(
        subtype_rows(&implements),
        ["Classes and mixins can't implement 'Enum'. @6"],
        "{:?}",
        subtype_rows(&implements)
    );
}

/// A non-generic bare alias to a restricted spelling chases one level: the
/// classification reads the alias's head, the report keeps the written
/// alias name (`Classes can't mixin 'T'.`, messages.yaml :17358 — the
/// oracle's `{0}` is the written type).
#[test]
fn disallowed_subtype_chases_bare_alias() {
    let report = report_ops_bare("typedef T = Null;\nclass B with T {}");
    assert_eq!(
        subtype_rows(&report),
        ["Classes can't mixin 'T'. @24"],
        "{:?}",
        subtype_rows(&report)
    );
    // An alias to a user-declared element stays silent: the chase resolves
    // the head to the file's own class.
    let user_alias = report_ops_bare("typedef U = C;\nclass C {}\nclass B with U {}");
    assert!(
        subtype_rows(&user_alias).is_empty(),
        "{:?}",
        subtype_rows(&user_alias)
    );
}

/// Clean subtype shapes stay row-free: a plain extends of a file class, a
/// real mixin application, and a satisfied `on` constraint.
#[test]
fn disallowed_subtype_clean_shapes_stay_silent() {
    assert_silent(&report_ops_bare(
        "mixin M {}\nclass A {}\nclass B extends A with M {}",
    ));
    assert_silent(&report_ops_bare("class A {}\nmixin M on A {}"));
}
