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
/// the introduction's claims seed the scope before any fragment's — the
/// second-in-library-order fragment's repeat is the report.
#[test]
fn the_merge_is_part_order_independent() {
    // Two `augment` heads of one name in ONE part file: the first head's
    // `x = 2` survives `named_containers`'s keep-first into the unit's
    // container row, the second head's `x = 3` does not (the #3378
    // keep-first residual — a `model.rs` change, out of scope). The merged
    // claim eval sees one fragment `x` → one `duplicate_definition`.
    let report = report_ops_augment(
        "class C { int x = 1; }\n",
        "augment class C { int x = 2; }\naugment class C { int x = 3; }\n",
    );
    let names = code_names(&report);
    assert_eq!(
        names
            .iter()
            .filter(|name| **name == "duplicate_definition")
            .count(),
        1,
        "{:#?}",
        rows(&report)
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
