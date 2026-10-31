//! #3255 pins — the multi-superinterface candidate merge (recon C4):
//! `_getValidOverrides` + `_topMerge`.
//!
//! Upstream correspondence (`pkg/analyzer`):
//! - `inheritance_manager3.dart`'s `_findMostSpecificFromNamedCandidates`
//!   (`:478-513`) builds one interface-map entry per name from the
//!   candidates every direct super face contributes;
//! - `_getValidOverrides` (`:1360-1378`) keeps the candidates that are a
//!   subtype of every other candidate;
//! - `_topMerge` (`:1193-1334`) answers the first survivor when the
//!   survivors' types agree, the survivor whose type equals the
//!   `_topMergeSignatureTypes` fold result (`:1385-1394`), and otherwise a
//!   synthesized element enclosing the merge target — whose type the
//!   `invalid_override` message prints in the overridden slot
//!   (`diagnostic_factory.dart:369-377`).
//!
//! Reachability: a class's own interface map is what `getMember3` answers
//! for a SUBCLASS's `_checkDeclaredMember` loop
//! (`inheritance_override.dart:522-530`), so a merge is observable exactly
//! when a class inherits a member from several super faces and a subclass
//! declares a non-conforming override of it — every fixture below has that
//! shape. A class that declares the member itself compares against each
//! direct super face separately (`getMember3(I<int>)` / `getMember3(J<String>)`,
//! `test_*_twoInterfaces_conflicting`'s two rows) and never merges.
//!
//! Registered through `diagnose/tests.rs`'s `#[path]` block: `diagnose.rs`
//! is a live-lane file out of this ticket's write scope, and `#[path]`
//! keeps the module inside the `diagnose` subtree so
//! `crate::diagnose::test_rig` resolves.

use crate::diagnose::FileReport;
use crate::diagnose::test_rig::{report_ops_bare, rows};

/// The `invalid_override` rows of one report, verbatim.
fn override_rows(report: &FileReport) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "invalid_override")
        .map(|d| d.message.to_string())
        .collect()
}

/// Both candidates are valid overrides of each other (`dynamic` and
/// `Object?` are both top faces — `subtype.dart`'s Right Top / Left Top),
/// and their types differ, so `_topMerge` merges the parameter column to
/// `Object?`. The merged type equals the SECOND candidate's, so the
/// interface map carries `J.m` — not the first valid candidate `I.m`.
/// A first-candidate pick prints `'I.m' ('String Function(dynamic)')`.
#[test]
fn merged_signature_lands_on_the_candidate_that_spells_it() {
    let report = report_ops_bare(
        "\
abstract class I { String m(dynamic a); }
abstract class J { String m(Object? a); }
abstract class A implements I, J {}
class B extends A { int m(Object? a) => 0; }
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
        found[0].contains("of 'J.m'"),
        "the merged signature is J's: {}",
        found[0]
    );
    assert!(
        found[0].contains("('String Function(Object?)')"),
        "the overridden slot renders the merged parameter column: {}",
        found[0]
    );
}

/// The merged signature belongs to no candidate: `void` and `dynamic`
/// return columns merge to `Object?` (`TopMergeHelper.topMerge`'s
/// `NNBD_TOP_MERGE(void, dynamic) = Object?`), which neither candidate
/// spells. `_topMerge` then synthesizes the member ON the merge target, so
/// the overridden slot names the intermediate class and prints the merged
/// type.
#[test]
fn a_signature_no_candidate_spells_is_synthesized_on_the_merge_target() {
    let report = report_ops_bare(
        "\
abstract class I { void m(); }
abstract class J { dynamic m(); }
abstract class A implements I, J {}
class B extends A { void m(int a) {} }
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
        found[0].contains("of 'A.m'"),
        "the synthesized member encloses the merge target: {}",
        found[0]
    );
    assert!(
        found[0].contains("('Object? Function()')"),
        "the overridden slot renders the merged result column: {}",
        found[0]
    );
}

/// The same synthesis on a parameter column: `void` and `dynamic`
/// parameters merge to `Object?`, and the merged type matches neither
/// candidate.
#[test]
fn a_merged_parameter_column_no_candidate_spells_is_synthesized() {
    let report = report_ops_bare(
        "\
abstract class I { int m(void a); }
abstract class J { int m(dynamic a); }
abstract class A implements I, J {}
class B extends A { int m(int a) => 0; }
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
        found[0].contains("of 'A.m'") && found[0].contains("('int Function(Object?)')"),
        "the synthesized member carries the merged parameter column: {}",
        found[0]
    );
}

/// The upstream `test_method_normalParamType_twoInterfaces` cohort, verbatim:
/// `m(num)` is the valid override of `m(int)` (parameters are
/// contravariant), so the intermediate class's interface map carries `J.m`
/// and the subclass's row anchors there.
#[test]
fn twoInterfaces_cohort_anchors_at_the_widest_parameter_face() {
    let report = report_ops_bare(
        "\
abstract class I { m(int n); }
abstract class J { m(num n); }
abstract class A implements I, J {}
class B extends A { m(String n) {} }
",
    );
    let found = override_rows(&report);
    assert_eq!(
        found.len(),
        1,
        "exactly one invalid_override row: {}",
        rows(&report).join("\n")
    );
    assert_eq!(
        found[0],
        "'B.m' ('dynamic Function(String)') isn't a valid override of 'J.m' ('dynamic Function(num)').",
        "the row is the oracle's, verbatim: {}",
        rows(&report).join("\n")
    );
}

/// The degenerate arm: both candidates spell the same signature, so the
/// merge cannot change the text and the first valid candidate stands (the
/// pre-port pick, kept as a regression guard).
#[test]
fn identical_candidates_keep_the_first_valid_one() {
    let report = report_ops_bare(
        "\
abstract class I { int m(int a); }
abstract class J { int m(int a); }
abstract class A implements I, J {}
class B extends A { String m(int a) => ''; }
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
        found[0].contains("of 'I.m' ('int Function(int)')"),
        "the first valid candidate stands: {}",
        found[0]
    );
}

/// The conflict arm: an arity pair makes neither candidate a valid override
/// of the other (`_getValidOverrides` empty), so the interface map carries
/// no name for it and the subclass's declared member has no face to break
/// against — no merge, no row. The declared member breaks against BOTH
/// candidates, so a first-survivor pick would emit a row: the silence is
/// the conflict arm, not an unrelated fixture. (The arity face decides
/// without the name lattice, which keeps the pin independent of which
/// type-pair the lattice proves unrelated.)
#[test]
fn conflicting_candidates_carry_no_name_and_stay_silent() {
    let report = report_ops_bare(
        "\
abstract class I { void m(int a); }
abstract class J { void m(int a, int b); }
abstract class A implements I, J {}
class B extends A { void m() {} }
",
    );
    let found = override_rows(&report);
    assert!(
        found.is_empty(),
        "a conflicting candidate set reports nothing: {}",
        rows(&report).join("\n")
    );
}

/// The `with`-face variant: the mixin contributes no candidate, the two
/// `implements` faces merge through the same decision, and the widest
/// parameter face wins.
#[test]
fn a_with_face_next_to_two_implemented_faces_still_merges() {
    let report = report_ops_bare(
        "\
mixin M {}
abstract class I { void m(int a); }
abstract class J { void m(num a); }
class A with M implements I, J {}
class B extends A { void m(String a) {} }
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
        found[0].contains("of 'J.m' ('void Function(num)')"),
        "the merged member is the widest parameter face: {}",
        found[0]
    );
}
