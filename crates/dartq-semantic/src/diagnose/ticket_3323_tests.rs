//! #3323 pins — the two uninitialized-declaration faces and their gates.
//!
//! Upstream splits the family by declaration site, and every arm reads the
//! same three facts off the declaration (never off a flow state):
//!
//! * **`const_not_initialized`** — a `const` group's declarator with no
//!   initializer, at the top level (`visitTopLevelVariableDeclaration`),
//!   inside a body (`visitVariableDeclarationStatement`) and for fields
//!   (`_checkForNotInitializedFieldDeclaration`). The const branch runs
//!   first and is exclusive: a const group never reaches the
//!   non-nullable face.
//! * **`not_initialized_non_nullable_variable`** — the one-initializer-less
//!   non-final declarator whose *resolved* type is potentially
//!   non-nullable, gated by the group (`abstract`/`external`/`late` heads
//!   opt out) and reported at the name token.
//! * **`not_initialized_non_nullable_instance_field`** — the same decision
//!   on an instance field, owned by the declaration walk only when the
//!   container declares no generative constructor.
//!
//! The pins below hold the gates this file owns (the flush side): the
//! `abstract`/`external` head exemption for top-level variables, the
//! `=`-bounded head scan that keeps an initializer from smuggling a
//! modifier, and the nullability re-decision on the collected type. The
//! `#[ignore]`d pin is the residual this file cannot reach: a *local*
//! const group's rows are staged by no pass at all (the parser emits the
//! field/top-level spelling, the listener latches non-const groups only).

use super::test_rig::{CORE_PRELUDE, report_ops};
use super::*;

/// The rows of `code` as the exact anchored source text, source order.
fn spans_of(report: &FileReport, full: &str, code: &str) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == code)
        .map(|d| {
            full[d.range.offset as usize..(d.range.offset + d.range.length) as usize].to_string()
        })
        .collect()
}

/// Every row code, for the "nothing else fired" assertions.
fn codes(report: &FileReport) -> Vec<&'static str> {
    report.diagnostics.iter().map(|d| d.code.name).collect()
}

/// The dominant shape plus its three exemptions: a written non-nullable
/// type with no initializer reports at the name; `?`-nullable, `late` and
/// the untyped (`dynamic`) spellings never reach the latch.
#[test]
fn top_level_non_nullable_uninitialized_reports_once() {
    let fixture = concat!(
        "int x;\n",
        "int? nullable;\n",
        "late int deferred;\n",
        "var untyped;\n",
        "dynamic dyn;\n",
    );
    let full = format!("{CORE_PRELUDE}{fixture}");
    let report = report_ops(fixture);
    assert_eq!(
        spans_of(&report, &full, "not_initialized_non_nullable_variable"),
        vec!["x".to_string()],
    );
}

/// An initializer anywhere in the group releases its own declarator only —
/// the flush's latch is per declarator, and the head scan stops at the `=`
/// that opens the initializer.
#[test]
fn group_initializer_releases_only_its_declarator() {
    let fixture = "int a = 1, b;\nint c, d = 2;\n";
    let full = format!("{CORE_PRELUDE}{fixture}");
    let report = report_ops(fixture);
    assert_eq!(
        spans_of(&report, &full, "not_initialized_non_nullable_variable"),
        vec!["b".to_string(), "c".to_string()],
        "each uninitialized declarator keeps its own row"
    );
}

/// The top-level group gate (`error_verifier.dart:2088-2092`): an
/// `abstract` or `external` head exempts every declarator of the group,
/// and the scan reads the head off the token stream (a top-level variable
/// carries no modifier row in the model).
#[test]
fn abstract_and_external_top_level_heads_stay_silent() {
    let fixture = concat!(
        "external int a;\n",
        "abstract int b;\n",
        "external int c, d;\n",
        "external Object? e;\n",
        "int f;\n",
    );
    let full = format!("{CORE_PRELUDE}{fixture}");
    let report = report_ops(fixture);
    assert_eq!(
        spans_of(&report, &full, "not_initialized_non_nullable_variable"),
        vec!["f".to_string()],
        "only the plain head reports; the exempt heads include the group siblings"
    );
}

/// An annotation precedes the modifiers in the head; the exemption still
/// reads through it, and the previous declaration's tail bounds the scan.
#[test]
fn exempt_head_survives_metadata_and_declaration_boundary() {
    let fixture = concat!(
        "int before = 0;\n",
        "@deprecated external int a;\n",
        "int after;\n",
    );
    let full = format!("{CORE_PRELUDE}{fixture}");
    let report = report_ops(fixture);
    assert_eq!(
        spans_of(&report, &full, "not_initialized_non_nullable_variable"),
        vec!["after".to_string()],
    );
}

/// The static-field arm keeps its model exemption (the `abstract` /
/// `external` bits ride the member row, not the token stream), so only the
/// plain static field reports.
#[test]
fn static_field_exemptions_hold() {
    let fixture = concat!(
        "class C {\n",
        "  external static int s;\n",
        "  static int t;\n",
        "  static int? u;\n",
        "}\n",
    );
    let full = format!("{CORE_PRELUDE}{fixture}");
    let report = report_ops(fixture);
    assert_eq!(
        spans_of(&report, &full, "not_initialized_non_nullable_variable"),
        vec!["t".to_string()],
    );
}

/// The instance-field counterpart: no generative constructor, so the
/// declaration walk owns the face; `?`-nullable and `late` stay out.
#[test]
fn instance_field_face_reports_only_non_nullable() {
    let fixture = concat!(
        "class A {\n",
        "  int? nullable;\n",
        "  late int deferred;\n",
        "  int c;\n",
        "}\n",
    );
    let full = format!("{CORE_PRELUDE}{fixture}");
    let report = report_ops(fixture);
    assert_eq!(
        spans_of(&report, &full, "not_initialized_non_nullable_instance_field"),
        vec!["c".to_string()],
    );
    assert!(
        spans_of(&report, &full, "not_initialized_non_nullable_variable").is_empty(),
        "the instance spelling owns the row: {:?}",
        codes(&report)
    );
}

/// The const branch is exclusive upstream: a `const` group's rows are the
/// `const_not_initialized` ones, and no non-nullable row joins them. (The
/// field/top-level spelling is parser-owned; this pin holds the exclusion
/// on the flush side.)
#[test]
fn const_group_never_reaches_the_non_nullable_face() {
    let fixture = "const int a;\nconst int? b;\n";
    let full = format!("{CORE_PRELUDE}{fixture}");
    let report = report_ops(fixture);
    assert_eq!(
        spans_of(&report, &full, "const_not_initialized"),
        vec!["a".to_string(), "b".to_string()],
        "both const declarators report the const spelling"
    );
    assert!(
        spans_of(&report, &full, "not_initialized_non_nullable_variable").is_empty(),
        "the const branch is exclusive: {:?}",
        codes(&report)
    );
}

/// A **local** const group (`visitVariableDeclarationStatement`,
/// error_verifier.dart:2204-2224): the const-group machinery's
/// `disarm_const_group` flush emits `const_not_initialized` for every
/// declared name whose initializer slot the cursor never consumed —
/// covering local, field, and top-level const declarations uniformly.
#[test]
fn local_const_group_reports_each_declarator() {
    let fixture = "void f() { const int a, b; }\n";
    let full = format!("{CORE_PRELUDE}{fixture}");
    let report = report_ops(fixture);
    assert_eq!(
        spans_of(&report, &full, "const_not_initialized"),
        vec!["a".to_string(), "b".to_string()],
    );
}

/// An initialized const declarator reports nothing; a mixed group
/// reports only the uninitialized name.
#[test]
fn const_initializer_releases_its_own_declarator() {
    let fixture = "void f() { const int a = 1, b; }\n";
    let full = format!("{CORE_PRELUDE}{fixture}");
    let report = report_ops(fixture);
    assert_eq!(
        spans_of(&report, &full, "const_not_initialized"),
        vec!["b".to_string()],
        "only the uninitialized declarator reports"
    );
}

/// A const field inside a class body: `const_instance_field` fires at
/// `const`, `const_not_initialized` at the name — the semantic engine
/// owns both rows.
#[test]
fn const_field_reports_both_faces() {
    let fixture = "class A {\n  const int v;\n}\n";
    let full = format!("{CORE_PRELUDE}{fixture}");
    let report = report_ops(fixture);
    assert_eq!(
        spans_of(&report, &full, "const_not_initialized"),
        vec!["v".to_string()],
    );
    assert!(
        !spans_of(&report, &full, "const_instance_field").is_empty(),
        "const_instance_field also fires"
    );
}

/// A top-level const declarator without an initializer reports
/// `const_not_initialized` at the name.
#[test]
fn top_level_const_reports() {
    let fixture = "const int v;\n";
    let full = format!("{CORE_PRELUDE}{fixture}");
    let report = report_ops(fixture);
    assert_eq!(
        spans_of(&report, &full, "const_not_initialized"),
        vec!["v".to_string()],
    );
}
