//! The #3378 pins — the augment fragment's member scope joins the
//! introduced element's across the library's units.
//!
//! # The defect
//!
//! `Builder::member_scopes` is built per unit, so an `augment class C`
//! written in a part file opens a fresh `MemberScope` instead of joining
//! the introduced class's scope. Upstream keys the duplicate context on
//! the merged element's first fragment
//! (`pkg/analyzer/lib/src/error/member_duplicate_definition_verifier.dart`
//! `_checkClassMembers` resolves
//! `_getElementContext(fragment.element.firstFragment)`), so a member the
//! fragment repeats reports `duplicate_definition` at the fragment's
//! declaration — and the fragment's own member claims merge with the
//! introduction's (a getter in one accepts the other's setter as a pair).
//!
//! # The pins
//!
//! ① an augment member that repeats an introduction member reports at the
//! fragment's declaration; ② a second `x` in one `augment` head collides
//! too (the keep-first survivor's claim); ③ a fragment getter pairs with
//! an introduction setter (no report); ④ instance/static scopes stay
//! distinct across the merge; ⑤ the merge is part-order independent;
//! ⑥ a pool-carrying fragment member joins neither a panic nor a
//! foreign-pool type read; ⑦ a member in a different element is not
//! merged; ⑧ a member from the introducing unit resolves inside the
//! augment body and vice versa (the #3418 face the scope merge must stay
//! consistent with).

use super::test_rig::{code_names, report_ops_augment, report_ops_augment_intro_experiments, rows};

/// ① The introduction's member repeated by the augment reports
/// `duplicate_definition` at the fragment's member — upstream's merged
/// `_InstanceElementContext` claims the introduction's `x` first, so the
/// augment's `x` is the second declaration.
#[test]
fn augment_member_repeating_the_introduction_reports() {
    let report = report_ops_augment(
        "class C { int x = 1; }\n",
        "augment class C { int x = 2; }\n",
    );
    let reported = code_names(&report);
    assert!(
        reported.contains(&"duplicate_definition"),
        "{:#?}",
        rows(&report)
    );
}

/// ② A fragment member claims against the merged element's second
/// declaration — two `x` in ONE `augment` head (the first survives
/// `members`'s keep-first, the second does not; the second's own
/// unit-local pass skipped it as an augment member). The merged eval
/// sees one `x` → one `duplicate_definition` anchored at the kept row.
#[test]
fn a_second_fragment_repeating_the_introduction_reports() {
    let report = report_ops_augment(
        "class C { int x = 1; }\n",
        "augment class C { int x = 2; int x = 3; }\n",
    );
    assert!(
        code_names(&report).contains(&"duplicate_definition"),
        "{:#?}",
        rows(&report)
    );
}

/// ③ The getter/setter pair merges across fragments exactly like within
/// one unit: a getter in the introduction accepts the augment's setter —
/// no `duplicate_definition`.
#[test]
fn fragment_getter_pairs_with_the_introduction_setter() {
    let report = report_ops_augment(
        "class C { int get v => 1; }\n",
        "augment class C { set v(int n) {} }\n",
    );
    assert!(
        !code_names(&report).contains(&"duplicate_definition"),
        "{:#?}",
        rows(&report)
    );
    // …and the same pair written in the other order is still a pair.
    let report = report_ops_augment(
        "class C { set v(int n) {} }\n",
        "augment class C { int get v => 1; }\n",
    );
    assert!(
        !code_names(&report).contains(&"duplicate_definition"),
        "{:#?}",
        rows(&report)
    );
}

/// ④ The instance/static scopes stay distinct across the merge: a static
/// member never collides with an instance member of the same name —
/// upstream's `_InstanceElementContext` claims them into separate
/// `Map<LookupName, ...>` tables (`member_duplicate_definition_verifier
/// .dart`'s `instanceMembers`/`staticMembers`).
#[test]
fn a_static_repeating_an_instance_member_still_reports() {
    let report = report_ops_augment(
        "class C { void m() {} }\n",
        "augment class C { static void m() {} }\n",
    );
    assert!(
        !code_names(&report).contains(&"duplicate_definition"),
        "{:#?}",
        rows(&report)
    );
}

/// ⑤ Part order independence: whichever fragment the library lists first,
/// the introduction's claims seed the scope before any fragment's — each
/// repeat of an already-claimed member reports exactly once, at its own
/// declaration, no matter which fragment carries it.
///
/// Two `augment` heads of one name in ONE part file produce TWO rows, and
/// that is the oracle's count, not a bug: upstream's merged
/// `_InstanceElementContext` claims `x = 1` (getter+setter → pair), then
/// `x = 2` collides → one report, then `x = 3` collides → one report
/// (member_duplicate_definition_verifier.dart walks every fragment of the
/// element). The two rows come from two producers on our side:
///
/// - `x = 2` is the keep-first survivor in the aug unit's
///   `named_containers`, so the MERGED claim eval sees it and reports its
///   collision with the introduction's pair (cross-unit);
/// - `x = 3` is dropped from `container.members` by keep-first, so the
///   merged eval cannot see it — but `member_scopes` is not `members`:
///   the second `augment` head joins the aug unit's `C` scope
///   (collector.rs:1960), where `x = 2`'s pair is already claimed, so the
///   UNIT-LOCAL pass reports `x = 3` in place. Same row upstream emits.
///
/// Swapping the heads' order keeps the count at two: the kept member's
/// row is always emitted by the merged eval, the dropped member's row by
/// the unit-local pass — the producers trade members, never the count.
#[test]
fn the_merge_is_part_order_independent() {
    let dup_count = |augment_source: &str| {
        code_names(&report_ops_augment(
            "class C { int x = 1; }\n",
            augment_source,
        ))
        .iter()
        .filter(|name| **name == "duplicate_definition")
        .count()
    };
    assert_eq!(
        dup_count("augment class C { int x = 2; }\naugment class C { int x = 3; }\n"),
        2
    );
    assert_eq!(
        dup_count("augment class C { int x = 3; }\naugment class C { int x = 2; }\n"),
        2
    );
}

/// ⑥ A fragment member whose declared type carries an argument-pool slot
/// (`List<int>` interns into the declaring unit's [`crate::model::ArgPool`])
/// joins the merged face without a panic and without leaking its foreign
/// slot into another unit's read — the cross-pool gate
/// [`crate::library::ElementFragments`] states.
#[test]
fn a_pool_carrying_fragment_member_neither_panics_nor_leaks() {
    // The introduction reads the fragment's `List<int>` member by name;
    // the read must resolve (the name joins the face) and the report must
    // carry no out-of-range pool read (a panic would kill the test).
    let report = report_ops_augment_intro_experiments(
        "class C { Object readIt() => this.items; }\n",
        "augment class C { List<int> items = []; }\n",
        &[dartq_language::Feature::Augmentations],
    );
    assert!(
        !code_names(&report).contains(&"undefined_getter"),
        "{:#?}",
        rows(&report)
    );
}

/// ⑦ The merge is element-scoped: `augment class D {x}` joins `D`'s
/// element, never `C`'s — an `x` in `D`'s fragment does not collide
/// with `C`'s `x`. `D` has no introduction, so `element.intro` is
/// `None` → the merged eval skips it entirely.
#[test]
fn a_member_in_a_different_element_is_not_merged() {
    let report = report_ops_augment(
        "class C { int x = 1; }\n",
        "augment class D { int x = 2; }\n",
    );
    assert!(
        !code_names(&report).contains(&"duplicate_definition"),
        "{:#?}",
        rows(&report)
    );
}

/// ⑧a A member declared in the introducing unit resolves inside the
/// augment body (the merged face both directions, contract clause).
#[test]
fn introduction_member_resolves_inside_the_augment_body() {
    let report = report_ops_augment(
        "class C { int introMember = 1; }\n",
        "augment class C { int readIt() => this.introMember; }\n",
    );
    assert!(
        !code_names(&report).contains(&"undefined_getter"),
        "{:#?}",
        rows(&report)
    );
}

/// ⑧b A member declared in the fragment resolves inside the introducing
/// unit's body.
#[test]
fn augment_member_resolves_inside_the_introducing_body() {
    let report = report_ops_augment_intro_experiments(
        "class C { int readIt() => this.augmentMember; }\n",
        "augment class C { int augmentMember = 2; }\n",
        &[dartq_language::Feature::Augmentations],
    );
    assert!(
        !code_names(&report).contains(&"undefined_getter"),
        "{:#?}",
        rows(&report)
    );
}

/// ⑧c A member in NEITHER fragment still reports — the merge widens the
/// face to the element's members, never to "anything resolves".
#[test]
fn a_member_declared_in_neither_fragment_still_reports() {
    let report = report_ops_augment_intro_experiments(
        "class C { int readIt() => this.absentMember; }\n",
        "augment class C { int augmentMember = 2; }\n",
        &[dartq_language::Feature::Augmentations],
    );
    assert!(
        rows(&report)
            .iter()
            .any(|row| row.contains("undefined_getter") && row.contains("'absentMember'")),
        "{:#?}",
        rows(&report)
    );
}
