//! #3252 pins — C1+C7 covariant-parameter chains: `_inheritCovariance` /
//! `_inferParameterCovariance` read the oracle's `namedCandidates[name]` —
//! each DIRECT superinterface's merged interface-map member — while the
//! `CovariantParametersVerifier` rows still walk every transitive
//! superinterface's OWN declaration (`implementedInterfaces`). The marks
//! ride the override spine (each candidate's own propagation included,
//! recursively): a covariant mark on an overridden-out ancestor never
//! reaches the declared member.
//!
//! Upstream correspondence (`pkg/analyzer`):
//! - `inheritance_manager3.dart:1063-1189` `_inheritCovariance` — reads
//!   `namedCandidates[name]` (the `overridden` map), pairing
//!   `_ParameterDesc` positional-by-index / named-by-name;
//! - `instance_member_inferrer.dart` `_inferParameterCovariance` /
//!   `_isCovariantSetter` — the declared member's marks mutate during
//!   inference, so a mid-chain `B.m(dynamic)` over `A.m(covariant dynamic)`
//!   still answers covariant to ITS overrider;
//! - `correct_override.dart:96-251` `CovariantParametersVerifier` — the
//!   transitive own-declaration walk, kept on `covariant_own_faces`.
//!
//! Registered through `diagnose/tests.rs`'s `#[path]` block: the brief's
//! named files are live lanes out of this ticket's write scope, and
//! `#[path]` keeps the module inside the `diagnose` subtree so
//! `crate::diagnose::test_rig` resolves.

use crate::diagnose::FileReport;
use crate::diagnose::test_rig::{report_ops, rows};

/// The `invalid_override` rows of one report, verbatim.
fn override_rows(report: &FileReport) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "invalid_override")
        .map(|d| d.message.to_string())
        .collect()
}

/// The mixin-replace spine: `with M` substitutes M's member for the
/// superclass one in the interface map, so `namedCandidates[m]` carries
/// `[M.m]` alone — `A.m`'s covariant mark is overridden OUT. `C.m(String)`
/// is NOT marked: the subtype pass compares `String` against M's `num`
/// directly (unrelated either way) and lands the plain row against `M` —
/// not a covariant-verifier row against `A`. The pre-fix transitive
/// own-declaration read found `A.m(covariant)` anyway, marked `C.m`, and
/// reported the verifier row against `A` (right count, wrong face).
#[test]
fn mixin_replaced_covariant_mark_stays_off_the_declared_member() {
    let report = report_ops(
        "\
class A { void m(covariant num a) {} }
mixin M { void m(num a) {} }
class C extends A with M { void m(String a) {} }
",
    );
    let found = override_rows(&report);
    assert_eq!(
        found.len(),
        1,
        "exactly one invalid_override row: {}",
        rows(&report).join("\n")
    );
    assert!(
        found[0].contains("of 'M.m'"),
        "the subtype-pass row anchors at the mixin face, not A: {}",
        rows(&report).join("\n")
    );
}

/// The mark's own propagation through a mid-chain declaration: upstream
/// mutates `B.m`'s parameter flag during instance-member inference (it
/// overrode `A.m(covariant dynamic)`), so `C.m` inherits the mark from
/// `namedCandidates[m] = [B.map.m]` — the transitive raw-declaration read
/// and the spine read agree here, and the pin guards the recursion that
/// keeps them agreeing (without it the spine sees B's bare `dynamic`
/// declaration and the `String` parameter breaks the subtype pass).
#[test]
fn covariant_mark_propagates_through_a_non_covariant_redeclaration() {
    let report = report_ops(
        "\
class A { void m(covariant dynamic a) {} }
class B extends A { void m(dynamic a) {} }
class C extends B { void m(String a) {} }
",
    );
    assert!(
        override_rows(&report).is_empty(),
        "the propagated mark lets String narrow a covariant parameter: {}",
        rows(&report).join("\n")
    );
}

/// The `inheritance_method2` lattice spine, three hops: A declares the
/// covariant parameter, B redeclares without the keyword (marked by
/// propagation), C narrows to `int` — related to every transitive
/// superinterface parameter (`num`/`dynamic`), so the verifier stays
/// silent and the subtype pass skips the pair entirely.
#[test]
fn three_hop_chain_marks_through_each_redeclaration() {
    let report = report_ops(
        "\
class A { void m(covariant num a) {} }
class B extends A { void m(num a) {} }
class C extends B { void m(int a) {} }
",
    );
    assert!(
        override_rows(&report).is_empty(),
        "int-vs-num related pairs carry no row: {}",
        rows(&report).join("\n")
    );
}

/// The verifier's own row still lands: a declared `covariant` parameter on
/// `C.m` must be RELATED to every transitive superinterface's own
/// parameter — `String` vs B's `num` is unrelated either direction, so
/// `CovariantParametersVerifier` emits the row against `B` (the
/// transitive own-declaration walk `covariant_own_faces` keeps feeding
/// it), never against `A`'s `dynamic` (related).
#[test]
fn declared_covariant_parameter_verifies_against_the_transitive_faces() {
    let report = report_ops(
        "\
class A { void m(covariant dynamic a) {} }
class B extends A { void m(num a) {} }
class C extends B { void m(covariant String a) {} }
",
    );
    let found = override_rows(&report);
    assert_eq!(
        found.len(),
        1,
        "the unrelated B pair lands one verifier row: {}",
        rows(&report).join("\n")
    );
    assert!(
        found[0].contains("of 'B.m'"),
        "the verifier row anchors at B's own declaration: {}",
        rows(&report).join("\n")
    );
}

/// The `implements` face contributes its own candidate: `namedCandidates`
/// collects `B.map.m` and `I.map.m` — I's covariant mark reaches `C.m`
/// through the interface face even though the superclass spine is
/// covariant-free, the subtype pass skips the pair, and the verifier
/// finds every pair related (`int` vs `num`).
#[test]
fn implements_face_candidate_carries_the_mark() {
    let report = report_ops(
        "\
class I { void m(covariant num a) {} }
class B { void m(num a) {} }
class C extends B implements I { void m(int a) {} }
",
    );
    assert!(
        override_rows(&report).is_empty(),
        "the implements face's covariant candidate marks C.m: {}",
        rows(&report).join("\n")
    );
}

/// The named-parameter correspondence (`_ParameterDesc.name`): a covariant
/// named parameter propagates by name, not by position — B redeclares it
/// plain, C narrows it to a related type.
#[test]
fn named_parameter_mark_propagates_by_name() {
    let report = report_ops(
        "\
class A { void m({covariant num a = 0}) {} }
class B extends A { void m({num a = 0}) {} }
class C extends B { void m({int a = 0}) {} }
",
    );
    assert!(
        override_rows(&report).is_empty(),
        "the named covariant parameter propagates its mark: {}",
        rows(&report).join("\n")
    );
}

/// A genuinely covariant-free spine stays unmarked: no face declares the
/// keyword, so `C.m(String)` over `B.m(num)` is a plain subtype break —
/// the parameter is not covariant, `num` is no subtype of `String`, and
/// the row anchors at `B`.
#[test]
fn unmarked_parameter_reports_the_subtype_break() {
    let report = report_ops(
        "\
class A { void m(num a) {} }
class B extends A { void m(num a) {} }
class C extends B { void m(String a) {} }
",
    );
    let found = override_rows(&report);
    assert_eq!(
        found.len(),
        1,
        "the unmarked narrowing reports once: {}",
        rows(&report).join("\n")
    );
    assert!(
        found[0].contains("of 'B.m'"),
        "the row anchors at the direct super face: {}",
        rows(&report).join("\n")
    );
}
