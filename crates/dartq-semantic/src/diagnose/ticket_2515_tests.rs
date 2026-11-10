//! The #2515 pins: the SDK-units exposure wave's override-signature faces.
//!
//! Three semantics the previous SDK face table masked, each oracle-verified
//! against `dart analyze` 3.13.7:
//!
//! - **Generic members render their own type parameters** in the
//!   `invalid_override` signature slots, bounds included
//!   (`void Function<T extends num>(T)`), upstream Dart SDK source
//!   `pkg/analyzer/lib/src/dart/element/type.dart`'s `FunctionTypeImpl`
//!   textual face.
//! - **A member's type parameter is related only through its bound**
//!   (`subtype.dart`'s Variable arms): `T <: X` holds iff the bound is a
//!   subtype of `X`, `X <: T` only for `Never` or the same variable — so
//!   `void m<T, S>(T, int)` over `void m<T, S>(T, S)` and
//!   `void m<T extends num>(String)` over `void m<T extends num>(T)` both
//!   report, while an identical pair stays silent.
//! - **The completeness report names the interface map's most-specific
//!   member** (`inheritance_override.dart:426-451` reads
//!   `interface.map[name]`, then prints `enclosingElement.displayName`):
//!   `class S extends SetBase<int> with SetMixin<int>` names
//!   `PSetBase.contains`, the superclass's own abstract redeclaration, not
//!   the mixin face's `PSet.contains` interface row.
//!
//! Oracle provenance: the /tmp/ol2515 probe battery (`g1`/`g3` for the
//! signature slots and the type-variable verdicts, `f5`/`f5a` for the
//! `SetBase`/`SetMixin` completeness naming against the real
//! `dart:collection`).

use crate::diagnose::test_rig::{CORE_PRELUDE, assert_silent, report_ops, rows};

/// The absolute fixture offset of `needle` (the prelude the rig prepends
/// included; the fixtures are ASCII, so byte offset == UTF-16 offset).
fn at(fixture: &str, needle: &str) -> u32 {
    (CORE_PRELUDE.len() + fixture.find(needle).unwrap()) as u32
}

/// The override-family rows as `(offset, message)`, sorted.
fn override_rows(report: &crate::diagnose::FileReport) -> Vec<(u32, String)> {
    let mut v: Vec<(u32, String)> = report
        .diagnostics
        .iter()
        .filter(|d| {
            matches!(
                d.code.name,
                "invalid_override" | "non_abstract_class_inherits_abstract_member"
            )
        })
        .map(|d| (d.range.offset, d.message.to_string()))
        .collect();
    v.sort();
    v
}

/// Two type parameters, the second breaking contravariance: the message
/// spells both signatures with the member's own `<T, S>` and `<T extends
/// num>` lists — and the pair is a REAL break (the type variable `S` never
/// accepts `int`).
#[test]
fn generic_member_signature_slots_render_their_type_parameters() {
    let fixture = "class A { void m<T, S>(T a, S b) {} }\n\
                   class B extends A { void m<T, S>(T a, int b) {} }\n";
    let report = report_ops(fixture);
    assert_eq!(
        override_rows(&report),
        vec![(
            at(fixture, "m<T, S>(T a, int b)"),
            "'B.m' ('void Function<T, S>(T, int)') isn't a valid override of \
             'A.m' ('void Function<T, S>(T, S)')."
                .to_string()
        )],
        "rows: {:?}",
        rows(&report)
    );
}

/// A bounded type parameter against a concrete parameter type: the bound
/// (`num`) does not accept `String`, so the pair reports — with both slots
/// rendering `T extends num`.
#[test]
fn bounded_type_parameter_against_a_concrete_parameter_reports() {
    let fixture = "class A { void m<T extends num>(T a) {} }\n\
                   class B extends A { void m<T extends num>(String a) {} }\n";
    let report = report_ops(fixture);
    assert_eq!(
        override_rows(&report),
        vec![(
            at(fixture, "m<T extends num>(String a)"),
            "'B.m' ('void Function<T extends num>(String)') isn't a valid override of \
             'A.m' ('void Function<T extends num>(T)')."
                .to_string()
        )],
        "rows: {:?}",
        rows(&report)
    );
}

/// The control: an identical generic signature stays silent — the type
/// variable relates to itself, and the bound arm must not fabricate a row.
#[test]
fn identical_generic_signature_is_silent() {
    let report = report_ops(
        "class A { void m<T, S>(T a, S b) {} }\n\
         class B extends A { void m<T, S>(T a, S b) {} }\n",
    );
    assert_silent(&report);
}

/// A type parameter is NOT a supertype of its bound: `void m<T extends
/// num>(T)` over `void m<T extends num>(num)` breaks the parameter face
/// (`num <: T` holds only for `Never` or the same variable), while the
/// reverse direction (`num` declared over a `T` inherited) relates through
/// the bound and stays silent — the asymmetry is upstream's.
#[test]
fn type_parameter_does_not_accept_its_own_bound() {
    let fixture = "class A { void m<T extends num>(num a) {} }\n\
                   class B extends A { void m<T extends num>(T a) {} }\n";
    let report = report_ops(fixture);
    assert_eq!(
        override_rows(&report),
        vec![(
            at(fixture, "m<T extends num>(T a)"),
            "'B.m' ('void Function<T extends num>(T)') isn't a valid override of \
             'A.m' ('void Function<T extends num>(num)')."
                .to_string()
        )],
        "rows: {:?}",
        rows(&report)
    );
}

/// The bound arm's silent direction: a `num` parameter over an inherited
/// `T extends num` accepts every `T` (`T <: num` via the bound).
#[test]
fn bound_parameter_accepts_the_inherited_type_variable() {
    let report = report_ops(
        "class A { void m<T extends num>(T a) {} }\n\
         class B extends A { void m<T extends num>(num a) {} }\n",
    );
    assert_silent(&report);
}

/// The completeness description names the interface map's most-specific
/// member: the superclass's own abstract redeclaration
/// (`PSetBase.contains`), not the `with`-mixin face's interface row
/// (`PSet.contains`).
#[test]
fn completeness_names_the_most_specific_declaring_member() {
    let fixture = "abstract class PSet<E> { bool contains(Object? e); }\n\
                   abstract mixin class PSetBase<E> implements PSet<E> {\n\
                     bool contains(Object? e);\n\
                   }\n\
                   mixin PSetMixin<E> on PSet<E> {}\n\
                   class S extends PSetBase<int> with PSetMixin<int> {}\n";
    let report = report_ops(fixture);
    let descriptions: Vec<String> = override_rows(&report)
        .into_iter()
        .map(|(_, message)| message)
        .collect();
    assert_eq!(
        descriptions,
        vec!["Missing concrete implementation of 'PSetBase.contains'.".to_string(),],
        "rows: {:?}",
        rows(&report)
    );
    assert!(
        !rows(&report)
            .iter()
            .any(|row| row.contains("'PSet.contains'")),
        "the interface row must not name the mixin face: {:?}",
        rows(&report)
    );
}
