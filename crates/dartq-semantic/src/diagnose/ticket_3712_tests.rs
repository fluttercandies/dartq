//! Ticket #3712 pins: the `==`/`!=` left operand's created-value static
//! type arming the dot-shorthand context.
//!
//! # The wrong fact
//!
//! `untyped_operand_ty` had no `Receiver::Created` arm, so a folded
//! creation (`C()`, `ET(1)`, `ET<int>(2)`) as a `==` left operand answered
//! `Ty::UNKNOWN` — the shorthand's context fell through to
//! `scrutinee_ctx_at` (`None` → `'_'`, or a wrong `bool` row winning the
//! context). The upstream answer is the `InstanceCreationExpression`'s
//! `staticType` (`expression.typeOrThrow`): the created instance type.
//!
//! # The contract
//!
//! - `C() == .staticMember` — the context is the created head `C`; a
//!   static member spelling on it resolves silently.
//! - `ET(1) == .member` / `ET<int>(2) == .member` — an extension-type
//!   creation arms its own head (`ContainerKind::Extension` conflates the
//!   plain-extension spelling upstream types `InvalidType`; the resolved
//!   kind decides).
//! - `C() == .missing` — the miss reports `dot_shorthand_undefined_member`
//!   with the context type `C`, never `'_'` and never `bool`.
//! - A genuinely unknown left operand still reports `'_'` — the fallback
//!   is unchanged.
//!
//! Oracle provenance: `dart analyze` — `dot_shorthand_undefined_member`'s
//! `binary_expression_resolver.dart:112-116` left-operand arm; the
//! conformance `equalityRhs` family pins the left-operand static type as
//! the context.

use super::test_rig::{CORE_PRELUDE, code_names, report_ops, rows};
use super::FileReport;

/// The absolute offset of the first `needle` in `fixture` as the rig
/// analyzes it: `report_ops` prefixes [`CORE_PRELUDE`], so every
/// fixture-local offset shifts by the prelude's length.
fn at(fixture: &str, needle: &str) -> u32 {
    let hit = fixture
        .find(needle)
        .unwrap_or_else(|| panic!("{needle:?} not in fixture:\n{fixture}"));
    u32::try_from(CORE_PRELUDE.len() + hit).expect("fixture offsets fit u32")
}

/// The `dot_shorthand_undefined_member` rows as `(offset, length, message)`,
/// ascending.
fn undefined_rows(report: &FileReport) -> Vec<(u32, u32, String)> {
    let mut at: Vec<(u32, u32, String)> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "dot_shorthand_undefined_member")
        .map(|d| {
            (
                d.range.offset,
                d.range.end() - d.range.offset,
                d.message.to_string(),
            )
        })
        .collect();
    at.sort_unstable();
    at
}

/// Whether `report` carries any dot-shorthand row.
fn has_shorthand_row(report: &FileReport) -> bool {
    code_names(report)
        .iter()
        .any(|name| name.starts_with("dot_shorthand_"))
}

// —— a class creation's `==` left operand arms the created head: the
// —— static member spelling resolves silently against `C`.
#[test]
fn class_creation_left_operand_arms_its_head() {
    let source = "class C { static int get g => 0; }\n\
                  void f() { if (C() == .g) {} }\n";
    let r = report_ops(source);
    assert!(
        !has_shorthand_row(&r),
        "the `C` context resolves `.g`: {:?}",
        rows(&r)
    );
}

// —— an extension-type creation arms its head: `ET(1)` folds to a
// —— `Created` row whose `ty` stays unknown (extension types own no
// —— `creation_container` send solve), so the arm itself is what answers.
#[test]
fn extension_type_creation_left_operand_arms_its_head() {
    let source = "extension type ET(int v) { static int get g => 0; }\n\
                  void f() { if (ET(1) == .g) {} }\n";
    let r = report_ops(source);
    assert!(
        !has_shorthand_row(&r),
        "the `ET` context resolves `.g`: {:?}",
        rows(&r)
    );
}

// —— a written type-argument group instantiates the created head through
// —— `creation_row_ty`: `ET<int>(2)` arms `ET<int>` (the member walk keys
// —— on the head either way, but the arm must not lose the written group).
#[test]
fn written_group_creation_left_operand_instantiates() {
    let source = "extension type ET<T>(T v) { static int get g => 0; }\n\
                  void f() { if (ET<int>(2) == .g) {} }\n";
    let r = report_ops(source);
    assert!(
        !has_shorthand_row(&r),
        "the `ET<int>` context resolves `.g`: {:?}",
        rows(&r)
    );
}

// —— the miss spells the created head: `C` has no static `missing`, so
// —— `dot_shorthand_undefined_member` reports with the context type `C`
// —— (upstream's `left.typeOrThrow` — never `'_'`, never `bool`).
#[test]
fn class_creation_miss_reports_the_created_context() {
    let source = "class C {}\nvoid f() { if (C() == .missing) {} }\n";
    let r = report_ops(source);
    assert_eq!(
        undefined_rows(&r),
        vec![(
            at(source, "missing"),
            7,
            "The static getter 'missing' isn't defined for the context type 'C'.".to_string(),
        )],
        "the created head is the context: {:?}",
        rows(&r)
    );
}

// —— the invocation arm reads the same created context (an extension-type
// —— creation, whose `ty` is the arm's only source).
#[test]
fn extension_type_creation_miss_reports_the_created_context() {
    let source = "extension type ET(int v) {}\nvoid f() { if (ET(1) == .m()) {} }\n";
    let r = report_ops(source);
    assert_eq!(
        undefined_rows(&r),
        vec![(
            at(source, "m("),
            1,
            "The static method or constructor 'm' isn't defined for the context type 'ET'."
                .to_string(),
        )],
        "the extension-type head is the context: {:?}",
        rows(&r)
    );
}

// —— negative control: a left operand that is itself a shorthand arms no
// —— context for the right (upstream's special case covers only the
// —— shorthand's own site) — the right operand still spells `'_'`, proving
// —— the fallback did not move.
#[test]
fn shorthand_left_operand_keeps_the_underscore_context() {
    let source = "void f() { if (.a == .m()) {} }\n";
    let r = report_ops(source);
    let text = rows(&r).join("\n");
    assert!(
        text.contains("isn't defined for the context type '_'."),
        "a shorthand left operand still spells '_': {text}"
    );
}
