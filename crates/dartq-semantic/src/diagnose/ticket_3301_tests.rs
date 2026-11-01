//! The #3301 pins — the `invalid_override` cluster's dominant root: the
//! *inference* read of a super face was the declaring hop's own signature,
//! not the one instantiated at the written clause arguments.
//!
//! # What the cluster pass found
//!
//! The family was re-measured at HEAD over the whole corpus (a full
//! `conformance run`, the two dominant slices plus every case whose oracle
//! or actual side carries either override code): 82 cases still disagree
//! with the pinned SDK, split 50 EXTRA / 27 MISSING / 5 both. The biggest
//! coherent EXTRA cohort is not a compare arm at all — the two passes read
//! DIFFERENT signatures for one face:
//!
//! - the subtype pass reads the interface map's member, i.e. the declaring
//!   hop's signature instantiated at the edge's written arguments
//!   (`inheritance_manager3.dart`'s instantiated interface map,
//!   `Substitution.fromInterfaceType`);
//! - the instance-member inference read (upstream
//!   `instance_member_inferrer.dart`) gets that same entry from
//!   `combined_inherited`, whose single-candidate arm answered the RAW
//!   declaring-hop entry — `class A<E> { E foo() => 0; }` inferred an
//!   implicit `E` into `class B<T> extends A<T>`, while the comparison side
//!   asked about the substituted `T`. Two different types for one member,
//!   and `E <: T` is no more answerable than it looks: the pair reported.
//!
//! The fix instantiates the single-candidate arm too, which also makes the
//! two passes agree whenever the environment is undecidable (both then read
//! the unsubstituted entry, so they can never disagree).
//!
//! Corpus shapes this closes (all oracle-silent today, all measured):
//! `instance_member_inference_class/…/test_method_return_single_generic`,
//! the four `void_type_override_test_*` `var x` field faces, the two
//! `InferredTypeTest/test_inferTypesOnGenericInstantiations_*` fields, and
//! the `features/_core/async/await_*` SDK-edge members.
//!
//! # What it does NOT close (recorded, not pinned)
//!
//! - the `covariant` *field* cohort (~14 cases): `abstract covariant num
//!   x;` loses its covariance at the collector (`begin_fields` drops the
//!   keyword), so the induced setter face cannot carry the mark and
//!   `_inheritCovariance` has nothing to propagate. The model gap lives in
//!   another write scope;
//! - the `augment` cohort and the `declaringFormalParameter` field cohort
//!   (model shapes the semantic layer never sees);
//! - the combined-signature cohort, which needs `no_combined_super_signature`
//!   emitted at all (a registry entry outside this file's scope) before the
//!   remaining rows can move.

use super::FileReport;
use super::test_rig::{code_names, report_ops};

/// Every `invalid_override` message of `report`, in emission order.
fn override_rows(report: &FileReport) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "invalid_override")
        .map(|d| d.message.to_string())
        .collect()
}

/// An implicit method return inferred through a generic supertype edge: the
/// oracle reads `A<T>`'s substituted member (`T`), so `B.foo`'s inferred
/// return is `T` and the override is clean. The unsubstituted read inferred
/// `E` on the declared side while the comparison asked about `T`.
#[test]
fn generic_super_face_method_inference_is_substituted() {
    let report = report_ops(
        "class A<E> {\n\
         \x20 E foo() => throw 0;\n\
         }\n\
         class B<T> extends A<T> {\n\
         \x20 foo() => throw 0;\n\
         }\n",
    );
    assert_eq!(
        override_rows(&report),
        Vec::<String>::new(),
        "{:?}",
        code_names(&report)
    );
}

/// The same leak on the getter face — `E get v` inferred as `E` against the
/// compared `T`.
#[test]
fn generic_super_face_getter_inference_is_substituted() {
    let report = report_ops(
        "class A<E> {\n\
         \x20 E get v => throw 0;\n\
         }\n\
         class B<T> extends A<T> {\n\
         \x20 get v => throw 0;\n\
         }\n",
    );
    assert_eq!(
        override_rows(&report),
        Vec::<String>::new(),
        "{:?}",
        code_names(&report)
    );
}

/// An untyped field inferred through an `implements` edge with an explicit
/// argument: `class B implements A<Object?>` infers `Object?`, never A's own
/// `T` (the corpus face: `void_type_override_test_00b` and its three
/// siblings, whose setter face used to read `void Function(T)` against
/// `void Function(Object?)`).
#[test]
fn implements_edge_argument_reaches_the_inferred_field_type() {
    let report = report_ops(
        "class A<T> {\n\
         \x20 T x;\n\
         \x20 A(this.x);\n\
         }\n\
         class B implements A<Object?> {\n\
         \x20 var x;\n\
         }\n",
    );
    assert_eq!(
        override_rows(&report),
        Vec::<String>::new(),
        "{:?}",
        code_names(&report)
    );
}

/// The inference fix must not blanket the compare arms: a widened result
/// over a concrete supertype is still a break (`num` is no subtype of
/// `int`).
#[test]
fn widened_return_still_reports() {
    let report = report_ops(
        "class A {\n\
         \x20 int f() => 0;\n\
         }\n\
         class B extends A {\n\
         \x20 num f() => 0;\n\
         }\n",
    );
    assert_eq!(
        override_rows(&report),
        vec!["'B.f' ('num Function()') isn't a valid override of 'A.f' ('int Function()')."],
        "{:?}",
        code_names(&report)
    );
}

/// A narrowed parameter is a break too (contravariance: the declaration
/// must accept every value the super accepts).
#[test]
fn narrowed_parameter_still_reports() {
    let report = report_ops(
        "class A {\n\
         \x20 num f(num x) => 0;\n\
         }\n\
         class B extends A {\n\
         \x20 num f(int x) => 0;\n\
         }\n",
    );
    assert_eq!(
        override_rows(&report),
        vec!["'B.f' ('num Function(int)') isn't a valid override of 'A.f' ('num Function(num)')."],
        "{:?}",
        code_names(&report)
    );
}

/// The silent-negative for the substitution path itself: an explicit
/// declaration narrowed against a generic edge's argument is a valid
/// override (the inherited column instantiates to `int`), so nothing
/// reports — an unsubstituted comparison would have read `T` and broken.
#[test]
fn explicit_narrowing_through_a_generic_edge_stays_silent() {
    let report = report_ops(
        "class G<T> {\n\
         \x20 void put(T v) {}\n\
         }\n\
         class H extends G<int> {\n\
         \x20 void put(int v) {}\n\
         }\n",
    );
    assert_eq!(
        override_rows(&report),
        Vec::<String>::new(),
        "{:?}",
        code_names(&report)
    );
}
