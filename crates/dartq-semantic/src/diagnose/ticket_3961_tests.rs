//! #3961 pins — the bounds `ConstructorName` deny posture: the
//! super-bounded recovery (`_shouldAllowSuperBoundedTypes`) is denied at
//! use sites whose written type-argument group is a `ConstructorName`'s
//! child, so a regular violation reports there while the same written
//! arguments stay silent at a plain annotation site (the inverted form
//! passes). The oracle expectations below are verbatim probe results
//! (`dart analyze --format=machine`, 3.13.7).
//!
//! Upstream Dart SDK source,
//! pkg/analyzer/lib/src/error/type_arguments_verifier.dart:699-708: the
//! parent switch denies `ConstructorName` (instance creations, constructor
//! references/tear-offs, factory redirect targets, annotation creations)
//! before its extension-type leg; the parent kinds the in-file captures
//! otherwise carry (`TypeParameter` bounds, `TypeArgumentList` members)
//! keep the pass.

use super::test_rig::report_ops;
use crate::FileReport;

/// The offsets of one code's rows, ascending.
fn offsets(report: &FileReport, code: &str) -> Vec<u32> {
    let mut at = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == code)
        .map(|d| d.range.offset)
        .collect::<Vec<_>>();
    at.sort_unstable();
    at
}

/// The row offsets the harness answers for `fixture` are rig-relative
/// (the shared prelude precedes the fixture), so every expectation below
/// reads its anchor out of the fixture text itself plus the prelude
/// shift — the oracle anchors a bound row at the violating ARGUMENT's own
/// token (`_typeArgumentErrorNode`), the `A` inside each written group.
fn inner_arg_offset(fixture: &str, context: &str) -> u32 {
    let at = fixture
        .find(context)
        .unwrap_or_else(|| panic!("fixture context missing: {context}"));
    let group = fixture[at..]
        .find('<')
        .unwrap_or_else(|| panic!("no group after {context}"));
    (super::test_rig::CORE_PRELUDE.len() + at + group + 1) as u32
}

/// For `A<A> x = A<A>();` the annotation's argument stays silent (the
/// super-bounded pass recovers it) while both creations report — the
/// oracle emits the row at each creation's argument token and nowhere
/// else.
#[test]
fn constructor_name_parent_denies_the_super_bounded_recovery() {
    let fixture = "class A<T extends A<T>> {}\n\
                   void f() {\n\
                   \x20 A<A> x = A<A>();\n\
                   \x20 var y = A<A>();\n\
                   }\n";
    let report = report_ops(fixture);
    let at = offsets(&report, "type_argument_not_matching_bounds");
    let expected = [
        inner_arg_offset(fixture, "x = A<A>()"),
        inner_arg_offset(fixture, "y = A<A>()"),
    ];
    assert_eq!(
        at, expected,
        "only the ConstructorName-parented groups report, got {at:?} \
         ({report:#?})"
    );
    let annotation_arg = inner_arg_offset(fixture, "A<A> x");
    assert!(
        !at.contains(&annotation_arg),
        "the annotation site keeps the super-bounded pass: a row at \
         {annotation_arg} means the deny leaked onto a plain annotation \
         ({report:#?})"
    );
    let msg = &report
        .diagnostics
        .iter()
        .find(|d| d.code.name == "type_argument_not_matching_bounds")
        .unwrap()
        .message;
    assert!(
        msg.contains("'A<A<dynamic>>'")
            && msg.contains("'A<A<A<dynamic>>>'")
            && msg.contains("'T'"),
        "the row spells the argument, the bound, and the parameter like \
         the oracle, got: {msg}"
    );
}

/// A constructor tear-off is the same `ConstructorName` parent
/// (`A<A>.new`): the oracle reports the regular violation the plain
/// annotation spelling would have recovered. The written group reaches
/// the site capture the same way a creation's does — the head token is a
/// type use whose `<…>` group closes into the use-site window.
#[test]
fn constructor_reference_parent_reports_the_regular_violation() {
    let fixture = "class A<T extends A<T>> {}\n\
                   void f() {\n\
                   \x20 void Function() g = A<A>.new;\n\
                   \x20 print(g);\n\
                   }\n";
    let report = report_ops(fixture);
    let at = offsets(&report, "type_argument_not_matching_bounds");
    let expected = [inner_arg_offset(fixture, "= A<A>.new")];
    assert_eq!(
        at, expected,
        "the constructor reference's written group reports the regular \
         violation at its argument token, got {at:?} ({report:#?})"
    );
    let msg = &report
        .diagnostics
        .iter()
        .find(|d| d.code.name == "type_argument_not_matching_bounds")
        .unwrap()
        .message;
    assert!(
        msg.contains("'A<A<dynamic>>'"),
        "the row spells the argument like the oracle, got: {msg}"
    );
}
