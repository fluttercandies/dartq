//! #3926 pins: the mixin `on` constraint capture under augmentation and
//! under non-named constraint heads.
//!
//! Upstream anchors (Dart SDK 3.13.7, the pinned oracle):
//!
//! * `_mixinDeclaration` (pkg/analyzer/lib/src/summary2/types_builder.dart
//!   :371-384): every fragment's `onClause.superclassConstraints` APPENDS to
//!   the element's `superclassConstraints` — an `augment mixin M on X` adds
//!   `X` to the merged constraint set, it never replaces and is never
//!   ignored.
//! * `_toInterfaceTypeList` (types_builder.dart:47-53): every clause list
//!   passes `whereType<InterfaceTypeImpl>()` — a record-typed `on` head
//!   (`on (int i, String s)`, or a typedef expanding to a record) drops
//!   out of `superclassConstraints` entirely, so no
//!   `mixin_application_not_implemented_interface` constraint row exists
//!   for the application sites.
//! * `_checkForMixinSuperclassConstraints` (error_verifier.dart:6079-6122):
//!   each fragment's own `withClause.mixinTypes` is walked against the
//!   MERGED element's `supertype` and `mixins` — an `augment class C with M`
//!   checks `M` at index 0 of ITS OWN clause (the prior-mixin probe reads
//!   `enclosingClass.mixins[0..0]`, never a sibling fragment's entries).

use super::test_rig::{CORE_PRELUDE, code_names, report_ops, report_ops_augment};

const NOT_IMPLEMENTED: &str = "mixin_application_not_implemented_interface";

/// The fixture's own rows of one code, as `(offset, message)`, in row order.
fn rows_of(report: &crate::diagnose::FileReport, code: &str) -> Vec<(u32, String)> {
    let shift = u32::try_from(CORE_PRELUDE.encode_utf16().count()).expect("prelude fits u32");
    report
        .diagnostics
        .iter()
        .map(|row| (row.code.name, row.range.offset, row.message.to_string()))
        .filter(|(name, _, _)| *name == code)
        .map(|(_, offset, message)| (offset.saturating_sub(shift), message))
        .collect()
}

/// The UTF-16 offset of `needle`'s first occurrence in `source`.
fn offset_of(source: &str, needle: &str) -> u32 {
    let at = source.find(needle).expect("needle present in fixture");
    u32::try_from(source[..at].encode_utf16().count()).expect("fixture fits u32")
}

/// The cross-unit merge: an `augment` head's own `with` clause in a part
/// file is checked against the MERGED element's supertype — `C3 extends A`
/// satisfies `M1`'s `on A`, so the augment fragment's application reports
/// nothing (upstream `_enclosingClass` is the merged element, not the
/// fragment row).
///
/// `#[ignore]`d: the verdict lives in `verify_mixin_application`
/// (enum_verifier.rs:284-470), which iterates `unit.container(name)` — the
/// raw per-file row — instead of the merged element face
/// (`library::resolve_container` / `ElementIndex::element_face`). A
/// fragment-unit row has no `superclass`, so it probes `Object` and mints
/// the extra row. Owning layer: enum_verifier.rs (the check must read the
/// merged face while iterating each fragment's own `with` list).
#[test]
#[ignore = "enum_verifier.rs verify_mixin_application reads the raw unit row, not the merged element face — owning layer outside this ticket's scope"]
fn augment_fragment_with_reads_merged_supertype() {
    let main = "class A {}\n\nmixin M1 on A {}\n\nclass C3 extends A {}\n";
    let augment = "augment class C3 with M1 {}\n";
    let aug = report_ops_augment(main, augment);
    assert_eq!(
        rows_of(&aug, NOT_IMPLEMENTED),
        vec![],
        "augment fragment must not report — merged supertype A satisfies on A"
    );
}

/// The augment-mixin `on` clause joins the element's constraints (upstream
/// appends): `augment mixin M on B` makes a `with M` application on `A`
/// report `A doesn't implement B` — and upstream reports it at EVERY
/// fragment's written `with M`, including the intro's.
#[test]
fn augment_mixin_on_appends_constraint() {
    let source = "class A {}\nclass B {}\nmixin M on A {}\n\nclass C extends A with M {}\n\naugment mixin M on B {}\n";
    let report = report_ops(source);
    assert_eq!(
        rows_of(&report, NOT_IMPLEMENTED),
        vec![(
            offset_of(source, "M {}\n\naugment"),
            "'M' can't be mixed onto 'A' because 'A' doesn't implement 'B'.".to_string()
        )],
        "the augment-declared constraint must land in the merged on_clause"
    );
}

/// A fragment-local `with` entry answers at its own clause index: `class C
/// with M1` followed by `augment class C with M2` reports `M2 on M1`
/// against `Object` — the augment fragment's `with` list is index 0, so no
/// prior mixin is probed (error_verifier.dart:6079-6122's `mixins[0..0]`).
///
/// `#[ignore]`d: `fold_fragment` (collector.rs:2596-2603) unions the
/// fragment's `with` into the sitting row — correct for the element's
/// merged face, but it erases the fragment boundary the check needs:
/// `verify_mixin_application` reads `M2` at merged position 1 and credits
/// prior mixin `M1` (`enclosingClass.mixins[0..1]`), where upstream walks
/// the augment fragment's own `withClause` at index 0 (`mixins[0..0]`).
/// Owning layer: a per-fragment `with` list in the model plus
/// enum_verifier.rs iterating it — the same merged-context fix as the
/// cross-unit arm above.
#[test]
#[ignore = "fragment-local with index needs a per-fragment clause list in the model + enum_verifier iteration — owning layer outside this ticket's scope"]
fn augment_class_with_checks_fragment_local_index() {
    let source =
        "mixin M1 {}\n\nmixin M2 on M1 {}\n\nclass C with M1 {}\n\naugment class C with M2 {}\n";
    let report = report_ops(source);
    assert_eq!(
        rows_of(&report, NOT_IMPLEMENTED),
        vec![(
            offset_of(source, "M2 {}"),
            "'M2' can't be mixed onto 'Object' because 'Object' doesn't implement 'M1'."
                .to_string()
        )],
        "the augment fragment's `with` entry must report at index 0"
    );
}

/// The enum arm: `augment enum E with M2` checks `M2 on M1` against the
/// enum's implicit `Enum` supertype.
///
/// `#[ignore]`d for the same root as the class arm — the fold merges the
/// augment's `with` into `E`'s row, so `M2` probes prior mixin `M1` and
/// stays silent where upstream's fragment-local index reports.
#[test]
#[ignore = "same fragment-local with index root as the class arm — owning layer is enum_verifier.rs + per-fragment model data"]
fn augment_enum_with_checks_against_enum_supertype() {
    let source =
        "mixin M1 {}\n\nmixin M2 on M1 {}\n\nenum E with M1 { e0 }\n\naugment enum E with M2 {;}\n";
    let report = report_ops(source);
    let codes = code_names(&report);
    assert!(
        codes.contains(&NOT_IMPLEMENTED),
        "expected the constraint row on the enum augment, got {codes:?}"
    );
}

/// A record-typed `on` head produces no element constraint: `on (int i,
/// String s)` parses as a recovery (`expected_named_type` owns the row) and
/// upstream's `whereType<InterfaceType>` drops the whole thing — the
/// application `class C with M1` sees an EMPTY constraint list, so no
/// `mixin_application_not_implemented_interface` row exists at all.
#[test]
fn record_typed_on_clause_captures_no_constraint() {
    let source = "mixin M1 on (int i, String s) {}\n\nclass C with M1 {}\n";
    let report = report_ops(source);
    assert_eq!(
        rows_of(&report, NOT_IMPLEMENTED),
        vec![],
        "a record head captures no on-constraint — Object satisfies nothing"
    );
    // The `expected_named_type` recovery row still stands — the parser's
    // own verdict, untouched by the element-level drop.
    let codes = code_names(&report);
    assert!(
        codes.contains(&"expected_named_type"),
        "the recovery row remains, got {codes:?}"
    );
}

/// A typedef that expands to a record is a record-typed head too: `on Rec`
/// resolves to `RecordType` upstream and drops out of
/// `superclassConstraints`, so `with M` reports no constraint row (the
/// `mixin_super_class_constraint_non_interface` clause-error row is the
/// site's own verdict, unaffected).
#[test]
fn alias_to_record_on_clause_captures_no_constraint() {
    let source = "typedef Rec = (int i, String s);\n\nmixin M on Rec {}\n\nclass C with M {}\n";
    let report = report_ops(source);
    assert_eq!(
        rows_of(&report, NOT_IMPLEMENTED),
        vec![],
        "a record-expanding alias head captures no on-constraint"
    );
}

/// The same filter keeps every interface-typed head: a class alias's
/// terminal is a `Ty::Named` container, so `on C2` is a real constraint —
/// `class D with M` on `Object` reports it exactly like a spelled `on C`.
#[test]
fn alias_to_class_on_clause_keeps_constraint() {
    let source = "class C {}\ntypedef C2 = C;\n\nmixin M on C2 {}\n\nclass D with M {}\n";
    let report = report_ops(source);
    assert_eq!(
        rows_of(&report, NOT_IMPLEMENTED),
        vec![(
            offset_of(source, "M {}\n"),
            "'M' can't be mixed onto 'Object' because 'Object' doesn't implement 'C2'.".to_string()
        )],
        "a class-typed alias head stays a constraint"
    );
}

/// A mixed `on` list drops only the record node: `on A, (int, String), B`
/// keeps `A` and `B` as constraints (upstream's `onClause` keeps the
/// `NamedType`s and drops the non-interface one), so `class C with M`
/// reports the first unsatisfied head.
#[test]
fn record_node_inside_on_list_keeps_named_heads() {
    let source = "class A {}\nclass B {}\n\nmixin M on A, (int, String), B {}\n\nclass C extends A with M {}\n";
    let report = report_ops(source);
    assert_eq!(
        rows_of(&report, NOT_IMPLEMENTED),
        vec![(
            offset_of(source, "M {}\n"),
            "'M' can't be mixed onto 'A' because 'A' doesn't implement 'B'.".to_string()
        )],
        "named heads beside a record node stay constraints"
    );
}
