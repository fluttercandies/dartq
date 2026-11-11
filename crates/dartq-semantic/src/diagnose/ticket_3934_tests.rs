//! Ticket #3934 — the two semantic-tier rows that block the four
//! `*_37733` case verdicts.
//!
//! # Item 1 — `invalid_inline_function_type` rides the parser tier
//!
//! The oracle's row is the fasta parser's own report
//! (`parseFormalParameter`'s generalized-function-type arm, parser_impl.dart
//! `:2323-2328`), which #3935 landed. The engine already merges parser rows
//! into the merged report, so both `typeAlias_37733` cases produce the exact
//! oracle row set with no semantic emitter at all — these pins hold that
//! end-to-end shape so a later tier split cannot silently drop it.
//!
//! # Item 2 — `concrete_class_with_abstract_member`
//!
//! The class arm of `_reportConcreteClassWithAbstractMember`
//! (inheritance_override.dart `:909-951`, reached from the `:417-437` loop)
//! reports a bodyless member of a NON-abstract class when its interface has no
//! concrete implementation of that name. `enum_verifier.rs` gains
//! `verify_class_abstract_members`, mounted from the same container walk as
//! the enum arms.
//!
//! Every expected row below is measured against
//! `dart analyze --format=machine` (oracle SDK 3.13.7) on the same fixture.

use super::test_rig::{code_names, report_ops_bare, rows};

/// `class C {\n  f(<T>());\n}\n` — the `test_method_name_notNull_37733`
/// fixture. Oracle rows: `concrete_class_with_abstract_member` at 3:4 len 9
/// (the whole `f(<T>());` declaration, offset 12 here) plus the parser-tier
/// `missing_identifier` at 3:6 (the `<`, offset 14).
#[test]
fn concrete_class_with_abstract_member_reports_the_bodyless_member() {
    let report = report_ops_bare("class C {\n  f(<T>());\n}\n");
    assert!(
        code_names(&report).contains(&"concrete_class_with_abstract_member"),
        "expected the class-arm row, got {:?}",
        rows(&report)
    );
    let row = report
        .diagnostics
        .iter()
        .find(|d| d.code.name == "concrete_class_with_abstract_member")
        .expect("the class-arm row");
    assert_eq!(
        (row.range.offset, row.range.length),
        (12, 9),
        "the row anchors at the whole member declaration (`f(<T>());`), got {:?}",
        rows(&report)
    );
    // The message interpolates the member then the class.
    assert!(
        rows(&report)
            .iter()
            .any(|row| row.contains("'f' must have a method body because 'C' isn't abstract.")),
        "message shape: {:?}",
        rows(&report)
    );
}

/// Control: an `abstract class` declaring the same bodyless member stays
/// silent (the dispatch gate is `!container.is_abstract`, upstream
/// inheritance_override.dart:417).
#[test]
fn abstract_class_bodyless_member_is_silent() {
    let report = report_ops_bare("abstract class C {\n  void f();\n}\n");
    assert!(
        !code_names(&report).contains(&"concrete_class_with_abstract_member"),
        "an abstract class must not report the row, got {:?}",
        rows(&report)
    );
}

/// Control: a concrete class whose bodyless member IS implemented by a mixin
/// stays silent (`interface_has_concrete` over the `with` face).
#[test]
fn mixin_implementation_suppresses_the_row() {
    let report = report_ops_bare("mixin M {\n  void f() {}\n}\nclass C with M {\n  void f();\n}\n");
    assert!(
        !code_names(&report).contains(&"concrete_class_with_abstract_member"),
        "a mixin-provided implementation must suppress the row, got {:?}",
        rows(&report)
    );
}

/// The two `typeAlias_37733` fixtures' `invalid_inline_function_type` rows are
/// parser-tier (see the module docs). This pin holds the combined shape for
/// `typedef T=Function(<S>());` so the row cannot vanish if a later change
/// splits the tiers.
#[test]
fn invalid_inline_function_type_row_survives_the_merge() {
    let report = report_ops_bare("typedef T=Function(<S>());\n");
    let names = code_names(&report);
    assert!(
        names.contains(&"invalid_inline_function_type"),
        "the parser-tier row must reach the merged report, got {:?}",
        rows(&report)
    );
    assert!(
        names.contains(&"missing_identifier"),
        "the companion name-slot row must survive too, got {:?}",
        rows(&report)
    );
}
