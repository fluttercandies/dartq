//! #3626 — the `conflicting_generic_interfaces` face.
//!
//! Oracle 3.13.7 rows, probed and census-pinned (the conformance corpus
//! carries every shape below with the exact operand order):
//!
//! ```text
//! class C extends A implements B {}            // A: I<int>, B: I<String>
//!   The class 'C' can't implement both 'I<int>' and 'I<String>' …
//! mixin M on A implements B {}
//!   The mixin 'M' …                        // on-chain before implements
//! enum E with M1, M2 { v }
//!   The enum 'E' …                          // implements before with
//! extension type C(Never it) implements A, B {}
//!   The extension type 'C' …
//! class C = A with M;                        // typedef A = I<int>
//!   The class 'C' … 'I<int>' and 'I<String>'
//! class D extends B implements C {}          // I<int?> vs I<int>
//!   … 'I<int?>' and 'I<int>' — a one-sided ? never merges
//! class A00 extends Object with M0 implements I<int> {}  // raw M0<T>
//!   … 'I<int>' and 'I<dynamic>' — implements before with,
//!     the raw mixin instantiates to bounds (probed)
//! class C<S, T> implements B<S>, A<T> {}      // B<T> implements A<T>
//!   … 'A<S>' and 'A<T>' — substituted type-parameter leaves conflict
//! class B extends A<FutureOr<Object>> {}; class C extends B implements A<Object> {}
//!   silent — NORM(FutureOr<Object>) = Object merges with Object
//! ```

use crate::FileReport;
use crate::diagnose::test_rig::{
    CORE_PRELUDE, RigImport, code_names, report_ops, report_ops_imports, rows,
};

/// Every `conflicting_generic_interfaces` message, in report order.
fn conflict_rows(report: &FileReport) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "conflicting_generic_interfaces")
        .map(|d| d.message.to_string())
        .collect()
}

/// The single conflict message, verbatim.
fn one_conflict(report: &FileReport) -> String {
    let hits = conflict_rows(report);
    assert_eq!(hits.len(), 1, "expected one row, got {:#?}", rows(report));
    hits.into_iter().next().unwrap()
}

const DIRECT: &str = "The class 'C' can't implement both 'I<int>' and 'I<String>' because the type arguments are different.";

/// `extends` chain against `implements` chain (the rep family:
/// `test_class_extends_implements`, 212-row census face).
#[test]
fn extends_chain_against_implements_chain() {
    let report = report_ops(
        "class I<T> {} \
         class A implements I<int> {} \
         class B implements I<String> {} \
         class C extends A implements B {}\n",
    );
    assert_eq!(one_conflict(&report), DIRECT);
}

/// A `with` application contributes its mixin's interfaces after the
/// `implements` faces (`test_class_extends_with`; the operand order is
/// the probe-confirmed official append order).
#[test]
fn mixin_application_face() {
    let report = report_ops(
        "class I<T> {} \
         class A implements I<int> {} \
         mixin B implements I<String> {} \
         class C extends A with B {}\n",
    );
    assert_eq!(one_conflict(&report), DIRECT);
}

/// A mixin's own `on` constraints come before its `implements` faces
/// (`test_mixin_on_implements`).
#[test]
fn mixin_on_against_implements() {
    let report = report_ops(
        "class I<T> {} \
         class A implements I<int> {} \
         class B implements I<String> {} \
         mixin M on A implements B {}\n",
    );
    assert_eq!(
        one_conflict(&report),
        "The mixin 'M' can't implement both 'I<int>' and 'I<String>' because the type arguments are different."
    );
}

/// An enum's `with` applications conflict through their mixins
/// (`test_enum_with`).
#[test]
fn enum_with_two_mixins() {
    let report = report_ops(
        "class I<T> {} \
         mixin M1 implements I<int> {} \
         mixin M2 implements I<String> {} \
         enum E with M1, M2 { v }\n",
    );
    assert_eq!(
        one_conflict(&report),
        "The enum 'E' can't implement both 'I<int>' and 'I<String>' because the type arguments are different."
    );
}

/// An extension type's `implements` pair conflicts directly
/// (`test_extensionType`).
#[test]
fn extension_type_implements_pair() {
    let report = report_ops(
        "class I<T> {} \
         class A implements I<int> {} \
         class B implements I<num> {} \
         extension type C(Never it) implements A, B {}\n",
    );
    assert_eq!(
        one_conflict(&report),
        "The extension type 'C' can't implement both 'I<int>' and 'I<num>' because the type arguments are different."
    );
}

/// A type-alias superclass expands to its right-hand element — the entry
/// is `I<int>`, not the alias spelling (`test_classTypeAlias_extends_
/// nonFunctionTypedef_with`).
#[test]
fn typedef_superclass_expands() {
    let report = report_ops(
        "class I<T> {} \
         typedef A = I<int>; \
         mixin M implements I<String> {} \
         class C = A with M;\n",
    );
    assert_eq!(one_conflict(&report), DIRECT);
}

/// A one-sided `?` on a type argument never merges
/// (`test_class_extends_implements_nullability`).
#[test]
fn one_sided_nullability_argument_conflicts() {
    let report = report_ops(
        "class I<T> {} \
         class A implements I<int> {} \
         class B implements I<int?> {} \
         class C extends A implements B {}\n",
    );
    assert_eq!(
        one_conflict(&report),
        "The class 'C' can't implement both 'I<int>' and 'I<int?>' because the type arguments are different."
    );
}

/// `Object` and `Object?` arguments are not the Object?-absorbing top
/// pair — as arguments the one-sided `?` conflicts
/// (`test_class_extends_implements_object_objectQuestion`).
#[test]
fn object_question_argument_conflicts() {
    let report = report_ops(
        "class A<T> {} \
         class B implements A<Object> {} \
         class C implements A<Object?> {} \
         class D extends B implements C {}\n",
    );
    assert_eq!(
        one_conflict(&report),
        "The class 'D' can't implement both 'A<Object>' and 'A<Object?>' because the type arguments are different."
    );
}

/// A raw generic mixin instantiates to bounds (`dynamic` for an absent
/// bound), and the `implements` faces collect before the `with` faces —
/// the probe-confirmed operand order
/// (`mixin_declaration_inference_invalid_03`).
#[test]
fn raw_mixin_instantiates_to_bounds_after_implements() {
    let report = report_ops(
        "class I<T> {} \
         mixin M0<T> implements I<T> {} \
         class A00 extends Object with M0 implements I<int> {}\n",
    );
    assert_eq!(
        one_conflict(&report),
        "The class 'A00' can't implement both 'I<int>' and 'I<dynamic>' because the type arguments are different."
    );
}

/// Substituted type-parameter leaves conflict by name — the declaration's
/// own parameters ride the entries as written (`regress22976`).
#[test]
fn substituted_parameter_leaves_conflict() {
    let report = report_ops(
        "class A<T> {} \
         class B<T> implements A<T> {} \
         class C<S, T> implements B<S>, A<T> {}\n",
    );
    assert_eq!(
        one_conflict(&report),
        "The class 'C' can't implement both 'A<S>' and 'A<T>' because the type arguments are different."
    );
}

/// A mixin's `on` constraint chain, instantiated through the mixin's
/// bound when applied raw, conflicts with the superclass instantiation
/// (`regress32353` / `test_class_recursiveSubtypeCheck` shape).
#[test]
fn on_constraint_bound_instantiation_conflicts() {
    let report = report_ops(
        "class F<T, D> {} \
         class Dir {} \
         class ioDir {} \
         mixin Fwd<T extends Dir> on F<T, ioDir> implements Dir {} \
         class Local extends F<Local, ioDir> with Fwd {}\n",
    );
    assert_eq!(
        one_conflict(&report),
        "The class 'Local' can't implement both 'F<Local, ioDir>' and 'F<Dir, ioDir>' because the type arguments are different."
    );
}

/// NORM collapses `FutureOr<Object>` to `Object` before the merge, so the
/// top types merge silently (`test_class_topMerge`).
#[test]
fn future_or_object_norms_to_object_and_merges() {
    let report = report_ops(
        "class A<T> {} \
         class B extends A<FutureOr<Object>> {} \
         class C extends B implements A<Object> {}\n",
    );
    assert!(
        !code_names(&report).contains(&"conflicting_generic_interfaces"),
        "{:#?}",
        rows(&report)
    );
}

/// Equal instantiations never conflict, through any clause pair
/// (`test_noConflict`, `test_class_extends_implements_never`).
#[test]
fn equal_instantiations_stay_silent() {
    let report = report_ops(
        "class I<T> {} \
         class A implements I<int> {} \
         class B implements I<int> {} \
         class C extends A implements B {} \
         class D implements I<Never> {} \
         class E implements I<Never> {} \
         class F extends D implements E {}\n",
    );
    assert!(
        !code_names(&report).contains(&"conflicting_generic_interfaces"),
        "{:#?}",
        rows(&report)
    );
}

/// A same-file augmentation's clause faces merge into the original
/// declaration's collection, reported once at the original's name; the
/// augment's `extends` face collects before the original's `implements`
/// face — clause order, not header order
/// (`augmenting_class_like_declarations_A02_t04`).
#[test]
fn augmentation_clause_faces_merge_into_the_original() {
    let report = report_ops(
        "class I<T> {} \
         abstract class A implements I<int> {} \
         abstract class C implements I<String> {} \
         augment abstract class C extends A {}\n",
    );
    assert_eq!(
        one_conflict(&report),
        "The class 'C' can't implement both 'I<int>' and 'I<String>' because the type arguments are different."
    );
}

/// The memoized merged set of an ALREADY-conflicting supertype feeds the
/// subclass with the conflicted cell's running type, fixing the operand
/// order of the second element's row (`mixin_declaration_inference_
/// invalid_06`'s A02: `I<int>` first, `J<dynamic>` first).
#[test]
fn conflicting_supertype_feeds_its_running_types() {
    let report = report_ops(
        "class I<X> {} \
         class J<X> {} \
         mixin M0<S, T> implements I<S>, J<T> {} \
         mixin M1<S, T> implements I<S>, J<T> {} \
         class A00 extends I<int> with M0 {} \
         class A01 extends J<int> with M1 {} \
         class A02 extends A00 implements A01 {}\n",
    );
    assert_eq!(
        conflict_rows(&report),
        [
            "The class 'A00' can't implement both 'I<int>' and 'I<dynamic>' because the type arguments are different.",
            "The class 'A01' can't implement both 'J<int>' and 'J<dynamic>' because the type arguments are different.",
            "The class 'A02' can't implement both 'I<int>' and 'I<dynamic>' because the type arguments are different.",
            "The class 'A02' can't implement both 'J<dynamic>' and 'J<int>' because the type arguments are different.",
        ]
    );
}

/// Two instantiations of one element through two `on` constraints of the
/// same mixin (`Super-mixins/compatible_t05`'s M2 shape).
#[test]
fn mixin_on_leaves_conflict_across_instantiations() {
    let report = report_ops(
        "class A<T> {} \
         class B<T> implements A<T> {} \
         mixin M2<X, Y> on A<X> implements B<Y> {}\n",
    );
    assert_eq!(
        one_conflict(&report),
        "The mixin 'M2' can't implement both 'A<X>' and 'A<Y>' because the type arguments are different."
    );
}

/// A non-generic interface collected twice never conflicts (the empty
/// argument list is the merge identity), and an unresolvable face is the
/// official `InvalidType` — silent here.
#[test]
fn non_generic_and_unresolved_faces_stay_silent() {
    let report = report_ops(
        "class I {} \
         class A implements I {} \
         class B implements I {} \
         class C extends A implements B, Undefined<int>, Undefined<String> {}\n",
    );
    assert!(
        !code_names(&report).contains(&"conflicting_generic_interfaces"),
        "{:#?}",
        rows(&report)
    );
}

/// The row anchors at the declaration's name token (`.at(nameToken)`),
/// the name's own length.
#[test]
fn direct_conflict_anchors_at_the_name_token() {
    let source = "class I<T> {} \
                  class A implements I<int> {} \
                  class B implements I<String> {} \
                  class Conflicted extends A implements B {}\n";
    let report = report_ops(source);
    let full = format!("{CORE_PRELUDE}{source}");
    let expected = full.find("class Conflicted").unwrap() + "class ".len();
    let hit = report
        .diagnostics
        .iter()
        .find(|d| d.code.name == "conflicting_generic_interfaces")
        .unwrap_or_else(|| panic!("no row: {:#?}", rows(&report)));
    assert_eq!(
        (hit.range.offset as usize, hit.range.length as usize),
        (expected, "Conflicted".len()),
        "{:#?}",
        rows(&report)
    );
}

/// Three distinct instantiations of one element report ONCE — the cell
/// records its first failure and ignores every later update
/// (`_ClassInterfaceType.update`'s `_error != null` early return).
#[test]
fn three_way_conflict_reports_the_first_failure_only() {
    let report = report_ops(
        "class I<T> {} \
         class A implements I<int> {} \
         class B implements I<String> {} \
         class C implements I<bool> {} \
         class D implements A, B, C {}\n",
    );
    assert_eq!(
        conflict_rows(&report),
        ["The class 'D' can't implement both 'I<int>' and 'I<String>' because the type arguments are different."]
    );
}

/// The first operand is the RUNNING merged result, not the first written
/// face: `A<FutureOr<Object>>` and `A<Object>` merge to `A<Object>`, and
/// that merged type heads the row the third face breaks.
#[test]
fn running_merge_feeds_the_first_operand() {
    let report = report_ops(
        "class A<T> {} \
         class B extends A<FutureOr<Object>> {} \
         class C extends B implements A<Object>, A<int> {}\n",
    );
    assert_eq!(
        one_conflict(&report),
        "The class 'C' can't implement both 'A<Object>' and 'A<int>' because the type arguments are different."
    );
}

/// `Never` and `Null` normalize as the spec's bottom and null types even
/// though the clause scanner carries them as named leaves:
/// NORM(FutureOr<Never>) = Future<Never> and NORM(Never?) = Null, so both
/// pairs merge silently rather than fabricating a conflict.
#[test]
fn never_and_null_normalize_before_the_merge() {
    let report = report_ops(
        "class A<T> {} \
         class B extends A<FutureOr<Never>> {} \
         class C extends B implements A<Future<Never>> {} \
         class D extends A<Never?> {} \
         class E extends D implements A<Null> {}\n",
    );
    assert!(
        !code_names(&report).contains(&"conflicting_generic_interfaces"),
        "{:#?}",
        rows(&report)
    );
}

/// An imported library's public alias of a private generic class expands
/// to the element it names: the entries key on `_PrivateGenericClass`
/// (the RHS head resolved in the alias's own library) and render as the
/// element, never as the alias spelling
/// (`private_name_duplicate_interface_error_test`'s A4/A6).
#[test]
fn foreign_public_alias_of_a_private_generic_resolves_to_the_element() {
    let report = report_ops_imports(
        "class A3 extends PublicGenericClassOfInt {} \
         class A4 extends A3 implements PublicGenericClass<String> {} \
         class A5 extends A3 implements PublicGenericClass<int> {} \
         class A6 implements PublicGenericClass<int>, PublicGenericClass<String> {}\n",
        &[RigImport {
            uri: "package:t/private_name_library.dart",
            prefix: None,
            source: "class _PrivateGenericClass<T> {}\n\
                     typedef PublicGenericClass<T> = _PrivateGenericClass<T>;\n\
                     typedef PublicGenericClassOfInt = _PrivateGenericClass<int>;\n",
        }],
    );
    assert_eq!(
        conflict_rows(&report),
        [
            "The class 'A4' can't implement both '_PrivateGenericClass<int>' and '_PrivateGenericClass<String>' because the type arguments are different.",
            "The class 'A6' can't implement both '_PrivateGenericClass<int>' and '_PrivateGenericClass<String>' because the type arguments are different.",
        ]
    );
}
