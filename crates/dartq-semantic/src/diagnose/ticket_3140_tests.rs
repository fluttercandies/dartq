//! The #3140 pins: `invalid_override` — the covariance and top-type faces.
//!
//! Two oracle verifiers decide an override, and both are pinned here:
//!
//! - `CorrectOverrideHelper` (upstream Dart SDK source,
//!   pkg/analyzer/lib/src/error/correct_override.dart:36-41 and :70-93
//!   `_computeThisTypeForSubtype`): the plain function-subtype question
//!   after every `covariant` parameter of the DECLARED member is rewritten
//!   to `Object?`. The flag is propagated along the override chain
//!   (`_inheritCovariance`, inheritance_manager3.dart:1063-1130), so an
//!   inherited `covariant` formal marks the overriding parameter too and the
//!   parameter face never breaks on a covariant pair.
//! - `CovariantParametersVerifier` (correct_override.dart:96-138, wired from
//!   inheritance_override.dart:548-555): for every covariant parameter, the
//!   declared type must be RELATED to the corresponding parameter of every
//!   transitive superinterface's own member — `thisType <: superType ||
//!   superType <: thisType` — reporting one plain `invalid_override` row per
//!   unrelated pair (never the setter code), naming the interface that
//!   DECLARES that member (an ancestor's member contributes even when an
//!   intermediate class redeclares the name).
//!
//! The top-type arms of the subtype matrix (`subtype.dart:52-66`) ride the
//! same helper: `void`/`dynamic`/`Object?` accept every result (`Right
//! Top`), while a `void`/`dynamic` source is a subtype only of a top target
//! (`Left Top` — `void m()` over `int m()` is broken, the reverse is not).
//!
//! Oracle provenance: `dart analyze` 3.13.7 over the /tmp/io3140 override
//! battery (covariant two-level and three-level chains, propagation through
//! an uncovariant intermediate, covariant setters, void returns, named-slot
//! order).

use super::test_rig::{CORE_PRELUDE, assert_silent, report_ops, rows};

/// The absolute fixture offset of `needle` (the prelude the rig prepends
/// included; the fixtures are ASCII, so byte offset == UTF-16 offset).
fn at(fixture: &str, needle: &str) -> u32 {
    (CORE_PRELUDE.len() + fixture.find(needle).unwrap()) as u32
}

/// The `invalid_override` rows as `(offset, message)` sorted by
/// `(offset, message)` — the oracle's machine-row face (the setter variant
/// shares the `invalid_override` machine name).
fn override_rows(report: &crate::diagnose::FileReport) -> Vec<(u32, String)> {
    let mut v: Vec<(u32, String)> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "invalid_override")
        .map(|d| (d.range.offset, d.message.to_string()))
        .collect();
    v.sort();
    v
}

// —— covariance: the subtype face skips every covariant pair ————————————

/// `void m(dynamic)` over `void m(covariant num)` is silent: the inherited
/// covariant formal propagates onto the overriding parameter
/// (`_inheritCovariance`), `_computeThisTypeForSubtype` rewrites it to
/// `Object?`, and the verifier finds `dynamic` and `num` related
/// (`num <: dynamic`).
#[test]
fn inherited_covariant_param_widening_is_silent() {
    let report = report_ops(
        "class A { void m(covariant num v) {} }\n\
         class B extends A { void m(dynamic v) {} }\n",
    );
    assert_silent(&report);
}

/// A narrowing onto a RELATED type stays silent too: `void m(num)` over
/// `void m(covariant num)` is related in both directions (equal types).
#[test]
fn inherited_covariant_param_related_narrowing_is_silent() {
    let report = report_ops(
        "class A { void m(covariant num v) {} }\n\
         class B extends A { void m(num v) {}\n}\n",
    );
    assert_silent(&report);
}

/// A declared `covariant` parameter related to the super parameter is
/// silent (`covariant int` over `num`: `int <: num`).
#[test]
fn declared_covariant_related_parameter_is_silent() {
    let report = report_ops(
        "class A { void m(num v) {} }\n\
         class B extends A { void m(covariant int v) {} }\n",
    );
    assert_silent(&report);
}

/// A declared `covariant` parameter unrelated to the super one reports the
/// plain `invalid_override` against that super member (the verifier's
/// relatedness question, never the setter code).
#[test]
fn declared_covariant_unrelated_parameter_reports_the_super_face() {
    let fixture = "class A { void m(int v) {} }\n\
                   class B extends A { void m(covariant String v) {} }\n";
    let report = report_ops(fixture);
    assert_eq!(
        override_rows(&report),
        vec![(
            at(fixture, "m(covariant String v)"),
            "'B.m' ('void Function(String)') isn't a valid override of \
             'A.m' ('void Function(int)')."
                .to_string()
        )],
        "rows: {:?}",
        rows(&report)
    );
}

/// An INHERITED covariant parameter unrelated to the overriding one reports
/// against the interface that declares it — the subtype face itself stays
/// silent (`Object?` rewrite).
#[test]
fn inherited_covariant_unrelated_parameter_reports_the_declaring_face() {
    let fixture = "class A { void m(covariant num v) {} }\n\
                   class B extends A { void m(covariant String v) {} }\n";
    let report = report_ops(fixture);
    assert_eq!(
        override_rows(&report),
        vec![(
            at(fixture, "m(covariant String v)"),
            "'B.m' ('void Function(String)') isn't a valid override of \
             'A.m' ('void Function(num)')."
                .to_string()
        )],
        "rows: {:?}",
        rows(&report)
    );
}

/// The verifier walks EVERY transitive superinterface's own member: a
/// three-level chain whose declared type is unrelated to both ancestors
/// lands two rows naming `A` and `B` (upstream
/// `classHierarchy.implementedInterfaces`).
#[test]
fn covariant_verifier_covers_every_transitive_interface() {
    let fixture = "class A { void m(covariant num v) {} }\n\
                   class B extends A { void m(covariant String v) {} }\n\
                   class C extends B { void m(covariant List<int> v) {} }\n";
    let report = report_ops(fixture);
    let target = "'C.m' ('void Function(List<int>)') isn't a valid override of ";
    assert_eq!(
        override_rows(&report),
        vec![
            (
                at(fixture, "m(covariant String v)"),
                "'B.m' ('void Function(String)') isn't a valid override of \
                 'A.m' ('void Function(num)')."
                    .to_string(),
            ),
            (
                at(fixture, "m(covariant List<int> v)"),
                format!("{target}'A.m' ('void Function(num)')."),
            ),
            (
                at(fixture, "m(covariant List<int> v)"),
                format!("{target}'B.m' ('void Function(String)')."),
            ),
        ],
        "rows: {:?}",
        rows(&report)
    );
}

/// The covariant mark propagates through a non-covariant intermediate: the
/// intermediate widening is silent, and only the leaf's unrelated type
/// reports — against the ancestor that declares `covariant`, not the
/// intermediate.
#[test]
fn covariant_propagates_through_a_plain_intermediate() {
    let fixture = "class A { void m(covariant num v) {} }\n\
                   class B extends A { void m(dynamic v) {} }\n\
                   class C extends B { void m(String v) {} }\n";
    let report = report_ops(fixture);
    assert_eq!(
        override_rows(&report),
        vec![(
            at(fixture, "m(String v)"),
            "'C.m' ('void Function(String)') isn't a valid override of \
             'A.m' ('void Function(num)')."
                .to_string()
        )],
        "rows: {:?}",
        rows(&report)
    );
}

/// A covariant setter pair reports the plain `invalid_override` (the
/// verifier never uses the setter code) with the setter's display name.
#[test]
fn covariant_setter_pair_reports_one_plain_row() {
    let fixture = "class A { set x(covariant num v) {} }\n\
                   class B extends A { set x(String v) {} }\n";
    let report = report_ops(fixture);
    assert_eq!(
        override_rows(&report),
        vec![(
            at(fixture, "x(String v)"),
            "'B.x' ('void Function(String)') isn't a valid override of \
             'A.x' ('void Function(num)')."
                .to_string()
        )],
        "rows: {:?}",
        rows(&report)
    );
}

/// A covariant parameter inherited through an `implements` interface keeps
/// the legal override silent (`covariant num` over `int` is related).
#[test]
fn covariant_parameter_from_an_implemented_interface_is_related() {
    let report = report_ops(
        "abstract class A { void m(covariant num v); }\n\
         class B implements A { void m(int v) {} }\n",
    );
    assert_silent(&report);
}

// —— the top-type arms of the subtype matrix ——————————————————————————

/// `void m()` over `int m()` breaks (`Left Top`: a `void` source is a
/// subtype only of a top target).
#[test]
fn void_result_over_a_concrete_result_reports() {
    let fixture = "class A { int m() => 0; }\n\
                   class B extends A { void m() {} }\n";
    let report = report_ops(fixture);
    assert_eq!(
        override_rows(&report),
        vec![(
            at(fixture, "m() {}"),
            "'B.m' ('void Function()') isn't a valid override of \
             'A.m' ('int Function()')."
                .to_string()
        )],
        "rows: {:?}",
        rows(&report)
    );
}

/// A concrete result over a `void` inherited one is legal (`Right Top`:
/// every type satisfies a `void` face).
#[test]
fn concrete_result_over_a_void_result_is_silent() {
    let report = report_ops(
        "class A { void m() {} }\n\
         class B extends A { int m() => 0; }\n",
    );
    assert_silent(&report);
}

/// `dynamic m()` over `void m()` is legal: a `dynamic` source is a subtype
/// of every top target.
#[test]
fn dynamic_result_over_a_void_result_is_silent() {
    let report = report_ops(
        "class A { void m() {} }\n\
         class B extends A { dynamic m() => 0; }\n",
    );
    assert_silent(&report);
}

// —— the message's named-parameter slots ——————————————————————————————

/// The signature slots render a function type's named parameters in NAME
/// order, not declaration order (the oracle formats the named set sorted).
#[test]
fn named_parameter_slots_render_in_name_order() {
    let fixture = "class A { void m({int a = 0, int b = 0, int c = 0, int d = 0}) {} }\n\
                   class B extends A { void m({int a = 0, int c = 0, int d = 0, int e = 0}) {} }\n";
    let report = report_ops(fixture);
    assert_eq!(
        override_rows(&report),
        vec![(
            at(fixture, "m({int a = 0, int c = 0"),
            "'B.m' ('void Function({int a, int c, int d, int e})') isn't a \
             valid override of 'A.m' ('void Function({int a, int b, int c, int d})')."
                .to_string()
        )],
        "rows: {:?}",
        rows(&report)
    );
}
