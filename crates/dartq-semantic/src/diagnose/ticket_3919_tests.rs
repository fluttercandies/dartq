//! #3919 pins: the `mixin_application_not_implemented_interface` verdict
//! reads INSTANTIATED faces, and the clause-error short-circuit suppresses
//! the face on a broken header.
//!
//! Upstream anchors (Dart SDK 3.13.7, the pinned oracle):
//!
//! * `_checkForMixinSuperclassConstraints` (error_verifier.dart:6079-6122):
//!   `typeSystem.isSubtypeOf(enclosingClass.supertype, constraint)` over the
//!   RESOLVED `InterfaceType`s — `I<dynamic>` is not a subtype of `I<int>`,
//!   `A<double>` is not one of `A<int>`, and the message spells the
//!   instantiated faces.
//! * The clause-error short-circuit (:2440-2448): a header that already
//!   carries a clause error (a restricted supertype, a `with` entry that is
//!   no interface type at all) skips the whole inheritance block, so no
//!   constraint row is emitted.
//! * `MixinApplicationNotImplementedInterfaceTest/test_class_hasRecursion`:
//!   `class X with Unresolved, M, CycleWithX` reports only the recursion and
//!   `mixin_of_non_class` rows — the unresolved entry's clause error
//!   short-circuits the constraint face.

use super::test_rig::{CORE_PRELUDE, report_ops};

/// The prelude's UTF-16 length (the `report_ops` fixtures' offset shift).
fn shift() -> u32 {
    u32::try_from(CORE_PRELUDE.encode_utf16().count()).expect("prelude fits u32")
}

const NOT_IMPLEMENTED: &str = "mixin_application_not_implemented_interface";

/// The fixture's own rows of one code, as `(offset, message)`, in row order.
fn rows_of(report: &crate::diagnose::FileReport, code: &str) -> Vec<(u32, String)> {
    let shift = shift();
    report
        .diagnostics
        .iter()
        .filter(|row| row.code.name == code)
        .map(|row| (row.range.offset - shift, row.message.to_string()))
        .collect()
}

/// The UTF-16 span of `needle`'s first occurrence in `source`.
fn span_of(source: &str, needle: &str) -> (u32, u32) {
    let at = source.find(needle).expect("needle present in fixture");
    let offset = u32::try_from(source[..at].encode_utf16().count()).expect("fixture fits u32");
    let length = u32::try_from(needle.encode_utf16().count()).expect("needle fits u32");
    (offset, length)
}

/// The INSTANTIATED verdict: a supertype reached by NAME (`extends I` under
/// `mixin M0<T> on I<T>`) is `I<dynamic>`, and `I<dynamic>` is not a subtype
/// of `I<int>` — the name-level walk reads a match and misses the row.
#[test]
fn constraint_instantiated_arguments_report() {
    let source = "class I<T> {}\n\nmixin M0<T> on I<T> {}\n\nclass C extends I with M0<int> {}\n";
    assert_eq!(
        rows_of(&report_ops(source), NOT_IMPLEMENTED),
        vec![(
            span_of(source, "M0<int>").0,
            "'M0<int>' can't be mixed onto 'I<dynamic>' because 'I<dynamic>' doesn't implement \
             'I<int>'."
                .to_string()
        )]
    );
}

/// The supertype side carries its written arguments too: a named mixin
/// application's head `A<double>` is not a subtype of the constraint
/// `A<int>`, and the message spells the head's arguments.
#[test]
fn constraint_written_superclass_arguments_report() {
    let source = "class A<T> {}\n\nmixin M on A<int> {}\n\nclass C = A<double> with M;\n";
    assert_eq!(
        rows_of(&report_ops(source), NOT_IMPLEMENTED),
        vec![(
            span_of(source, "M;").0,
            "'M' can't be mixed onto 'A<double>' because 'A<double>' doesn't implement 'A<int>'."
                .to_string()
        )]
    );
}

/// A nested written argument list renders through the type display: the
/// applied `M<List<int>>` keeps its argument's own structure.
#[test]
fn constraint_message_renders_nested_written_arguments() {
    let source = "class A<T> {}\n\nmixin M<T> on A<T> {}\n\nclass C with M<List<int>> {}\n";
    let rows = rows_of(&report_ops(source), NOT_IMPLEMENTED);
    assert_eq!(rows.len(), 1, "one row, got {rows:?}");
    assert_eq!(
        rows[0].1,
        "'M<List<int>>' can't be mixed onto 'Object' because 'Object' doesn't implement \
         'A<List<int>>'."
    );
}

/// The satisfied direction stays silent: `A<int>` IS a subtype of the
/// constraint `A<int>` (no row, name-level and instantiated agreement).
#[test]
fn constraint_matching_generic_interface_stays_silent() {
    let source = "class A<T> {}\n\nmixin M<T> on A<T> {}\n\nclass C extends A<int> with M {}\n";
    assert_eq!(
        rows_of(&report_ops(source), NOT_IMPLEMENTED),
        Vec::<(u32, String)>::new()
    );
}

/// A typedef interface names the same element: `C implements I` satisfies
/// `mixin M3 on IAlias` because `IAlias` expands to `I` (upstream compares
/// the resolved `InterfaceType`s, so the name-level miss is an extra).
#[test]
fn constraint_alias_interface_satisfied_stays_silent() {
    let source = "class I {}\n\ntypedef IAlias = I;\n\nmixin M3 on IAlias {}\n\n\
                  class C implements I {}\n\nclass X extends C with M3 {}\n";
    assert_eq!(
        rows_of(&report_ops(source), NOT_IMPLEMENTED),
        Vec::<(u32, String)>::new()
    );
}

/// An unresolved `with` entry is no interface type: its own clause error
/// short-circuits the whole inheritance block, so the later mixin's
/// constraint face never runs (the class still answers its recursion and
/// `mixin_of_non_class` rows — other code families).
#[test]
fn constraint_unresolved_with_entry_stays_silent() {
    let source = "class A {}\n\nmixin M on A {}\n\nmixin CycleWithX on X {}\n\n\
                  abstract class X with Unresolved, M, CycleWithX {}\n";
    assert_eq!(
        rows_of(&report_ops(source), NOT_IMPLEMENTED),
        Vec::<(u32, String)>::new()
    );
}

/// A `with` entry that resolves to a plain class (no mixin face at all)
/// never reaches the constraint check either.
#[test]
fn constraint_non_mixin_with_entry_stays_silent() {
    let source = "class A {}\n\nclass M1 {}\n\nclass C extends Object with M1 {}\n";
    assert_eq!(
        rows_of(&report_ops(source), NOT_IMPLEMENTED),
        Vec::<(u32, String)>::new()
    );
}

/// A restricted supertype is its own clause error (`Classes can't extend
/// 'Null'.`), which short-circuits the inheritance block — the constraint
/// row must not fire even though `Null` implements nothing.
#[test]
fn constraint_restricted_superclass_stays_silent() {
    let source = "mixin M on UnaryNum {}\n\nclass C extends Null with M {}\n";
    assert_eq!(
        rows_of(&report_ops(source), NOT_IMPLEMENTED),
        Vec::<(u32, String)>::new()
    );
}
