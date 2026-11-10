//! The #3563 pins: the `ambiguous_extension_member_access` slots render the
//! candidate faces as full extension ELEMENTS — the `ExtensionElement`
//! toString (upstream Dart SDK source, pkg/analyzer/lib/src/dart/element/
//! display_string_builder.dart:117-125 `writeExtensionElement`): the name,
//! its type-parameter group (`X extends bound`), and the extended type with
//! its arguments spelled verbatim — `extension E1<X extends void Function(X)>
//! on A<X>`, never the bare head `extension E1 on A`.
//!
//! The expected texts are the 3.13.7 oracle's byte-for-byte (`dart analyze
//! --format=machine` over the mixed, both-generic, non-generic, and unnamed
//! three-candidate fixtures). The 3+-candidate list keeps the resolver's
//! inline shapes (upstream Dart SDK source, pkg/analyzer/lib/src/dart/
//! resolver/extension_member_resolver.dart:119-124): a named extension
//! contributes `extension 'E'` alone, an unnamed one the full extended type.

use super::FileReport;
use super::test_rig::{code_names, report_ops, rows};

const AMBIGUOUS: &str = "ambiguous_extension_member_access";

/// The single ambiguity row's message text, byte-exact against the oracle.
#[track_caller]
fn assert_ambiguity_message(report: &FileReport, expected: &str) {
    let matches: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == AMBIGUOUS)
        .collect();
    assert!(
        matches.len() == 1,
        "exactly one ambiguity row expected, got {:?}",
        rows(report)
    );
    assert_eq!(
        matches[0].message, expected,
        "the ambiguity slots must render the oracle's element text"
    );
}

/// Both faces generic: each candidate spells its own `<X extends void
/// Function(X)>` group — oracle 3.13.7:
///
/// `A member named 'foo' is defined in 'extension E1<X extends void
/// Function(X)> on C<X>' and 'extension E2<X extends void Function(X)> on
/// C<X>', and neither is more specific.`
#[test]
fn both_generic_pair_renders_parameter_groups() {
    let report = report_ops(
        "class C<T> {} \
         extension E1<X extends void Function(X)> on C<X> { int get foo => 1; } \
         extension E2<X extends void Function(X)> on C<X> { int get foo => 2; } \
         void use(C<Never> a) { a.foo; }",
    );
    assert!(
        code_names(&report).contains(&AMBIGUOUS),
        "{:?}",
        rows(&report)
    );
    assert_ambiguity_message(
        &report,
        "A member named 'foo' is defined in \
         'extension E1<X extends void Function(X)> on C<X>' and \
         'extension E2<X extends void Function(X)> on C<X>', and neither is \
         more specific.",
    );
}

/// The non-generic control keeps the bare shape — oracle 3.13.7:
///
/// `A member named 'bar' is defined in 'extension E1 on B' and 'extension
/// E2 on B', and neither is more specific.`
#[test]
fn non_generic_pair_keeps_bare_element_shape() {
    let report = report_ops(
        "class B {} \
         extension E1 on B { int get bar => 1; } \
         extension E2 on B { int get bar => 2; } \
         void use(B b) { b.bar; }",
    );
    assert!(
        code_names(&report).contains(&AMBIGUOUS),
        "{:?}",
        rows(&report)
    );
    assert_ambiguity_message(
        &report,
        "A member named 'bar' is defined in \
         'extension E1 on B' and \
         'extension E2 on B', and neither is more specific.",
    );
}

/// The three-or-more list renders the unnamed inline shape with the full
/// extended type (`unnamed extension on 'D'`), comma-and-joined — oracle
/// 3.13.7 (upstream Dart SDK source, pkg/analyzer/lib/src/dart/resolver/
/// extension_member_resolver.dart:119-124 and the
/// `commaSeparatedWithAnd` join).
#[test]
fn three_candidate_list_renders_unnamed_inline_shape() {
    let report = report_ops(
        "class D {} \
         extension on D { int get zap => 1; } \
         extension on D { int get zap => 2; } \
         extension on D { int get zap => 3; } \
         void use(D d) { d.zap; }",
    );
    assert!(
        code_names(&report).contains(&AMBIGUOUS),
        "{:?}",
        rows(&report)
    );
    assert_ambiguity_message(
        &report,
        "A member named 'zap' is defined in \
         unnamed extension on 'D', unnamed extension on 'D', and unnamed \
         extension on 'D', and none are more specific.",
    );
}
