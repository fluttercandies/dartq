//! Ticket #3716 pins: an extension-type creation arming the
//! `equality_operand_ty` `Receiver::Created` face.
//!
//! # The wrong fact
//!
//! `equality_operand_ty` restricted its `Receiver::Created` arm to
//! `ContainerKind::Class`, so a folded extension-type creation (`ET(1)`)
//! answered `Ty::UNKNOWN`. Every consumer the face feeds read the gap as
//! "cannot type" and suppressed: `check_equals_argument` never looked up
//! the `==` formal, `binary_operator_sig` never resolved the declared
//! operator signature, and the binary fold's argument check never ran.
//! The upstream answer is the `InstanceCreationExpression`'s `staticType`
//! — the created extension-type head (`expression.typeOrThrow` in
//! `binary_expression_resolver.dart`).
//!
//! # The contract
//!
//! - `ET(1) == x` — the left operand types `ET`; an extension type's
//!   declared `==(formal)` promotes to `formal?` and the right operand
//!   checks against it (`argument_type_not_assignable` on a miss).
//! - `ET(1) + x` — the declared `operator +` signature resolves: a wrong
//!   right operand reports `argument_type_not_assignable`, a fitting one
//!   stays silent.
//! - `C() + x` — the class arm is unchanged (regression control).
//! - A plain `extension E`, `enum E`, or `mixin M` `(`-spelling stays
//!   `UNKNOWN` — upstream types those operands `InvalidType` (or reports
//!   the non-creation at the site itself), so no operand-driven check may
//!   fire.
//!
//! Oracle provenance: `dart analyze` — `binary_expression_resolver.dart`
//! `_resolveEqual`'s `checkForArgumentTypeNotAssignableForArgument` and the
//! `_resolveBinary` argument check run on the operand's resolved
//! `staticType`, which for an extension-type `ET(1)` is `ET`.

use super::FileReport;
use super::test_rig::{CORE_PRELUDE, code_names, report_ops, rows};

/// The absolute offset of the first `needle` in `fixture` as the rig
/// analyzes it: `report_ops` prefixes [`CORE_PRELUDE`], so every
/// fixture-local offset shifts by the prelude's length.
fn at(fixture: &str, needle: &str) -> u32 {
    let hit = fixture
        .find(needle)
        .unwrap_or_else(|| panic!("{needle:?} not in fixture:\n{fixture}"));
    u32::try_from(CORE_PRELUDE.len() + hit).expect("fixture offsets fit u32")
}

/// The `argument_type_not_assignable` rows as `(offset, length, message)`,
/// ascending.
fn atna_rows(report: &FileReport) -> Vec<(u32, u32, String)> {
    let mut at: Vec<(u32, u32, String)> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "argument_type_not_assignable")
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

// —— the ticket's defect: an extension-type creation's `==` left operand
// —— types as the `ET` head — and the `==`/`!=` fold's own row carries it.
// —— The `==` formal itself is `Object?` upstream (an extension type can
// —— never redeclare an `Object` member — `extension_type_declares_member_
// —— of_object`), so the observable arm lives on the operator signatures
// —— `binary_operator_sig`/`fold_binary` read through the same face.
// —— `ET.+`'s declared formal checks the right operand: `"s"` is no `ET`.
#[test]
fn extension_type_creation_arms_operator_signature() {
    let source = "extension type ET(int v) { ET operator +(ET o) => ET(0); }\n\
                  void f() { var r = ET(1) + \"s\"; }\n";
    let r = report_ops(source);
    assert_eq!(
        atna_rows(&r),
        vec![(
            at(source, "\"s\""),
            3,
            "The argument type 'String' can't be assigned to the parameter type 'ET'. ".to_string(),
        )],
        "the `ET` operand resolves the `+` signature: {:?}",
        rows(&r)
    );
}

// —— the fitting operand stays silent: `ET(2)` is an `ET` (the right
// —— operand's own `Created` row answers the same arm).
#[test]
fn extension_type_creation_fitting_argument_silent() {
    let source = "extension type ET(int v) { ET operator +(ET o) => ET(0); }\n\
                  void f() { var r = ET(1) + ET(2); }\n";
    let r = report_ops(source);
    assert!(
        atna_rows(&r).is_empty(),
        "`ET + ET` fits the formal: {:?}",
        rows(&r)
    );
}

// —— a written type-argument group instantiates the armed head through
// —— `creation_row_ty`: `ET<int>(1)` arms `ET<int>`, and the `+` formal
// —— substitutes through the receiver environment (#3063's arm).
#[test]
fn extension_type_written_group_arms_operator_signature() {
    let source = "extension type ET<T>(T v) { ET operator +(ET<T> o) => ET(v); }\n\
                  void f() { var r = ET<int>(1) + \"s\"; }\n";
    let r = report_ops(source);
    assert_eq!(
        atna_rows(&r),
        vec![(
            at(source, "\"s\""),
            3,
            "The argument type 'String' can't be assigned to the parameter type 'ET<int>'. "
                .to_string(),
        )],
        "the written group instantiates the `+` formal: {:?}",
        rows(&r)
    );
}

// —— regression control: the class arm is unchanged — `C() + "s"` still
// —— gates on `C`'s declared `+` formal (the pre-#3716 path).
#[test]
fn class_creation_operator_signature_unchanged() {
    let source = "class C { C operator +(C o) => this; }\n\
                  void f() { var r = C() + \"s\"; }\n";
    let r = report_ops(source);
    assert_eq!(
        atna_rows(&r),
        vec![(
            at(source, "\"s\""),
            3,
            "The argument type 'String' can't be assigned to the parameter type 'C'. ".to_string(),
        )],
        "the class `+` formal still gates: {:?}",
        rows(&r)
    );
}

// —— negative control: a plain `extension E`'s `E(1)` is `InvalidType`
// —— upstream (an invalid call, not a creation), so the operand stays
// —— `UNKNOWN` and arms no operator signature — an `E` head here would
// —— resolve `E.+` and report `argument_type_not_assignable` on `"s"`.
#[test]
fn plain_extension_call_operand_stays_suppressed() {
    let source = "extension E on int { int operator +(int o) => 0; }\n\
                  void f() { var r = E(1) + \"s\"; }\n";
    let r = report_ops(source);
    assert!(
        atna_rows(&r).is_empty(),
        "the plain-extension `(` spells no creation: {:?}",
        rows(&r)
    );
}

// —— negative control: an enum's `E(` is no creation (enums cannot be
// —— instantiated upstream — `invalid_reference_to_generative_enum_
// —— constructor`), so the `Enum` `Created` kind keeps the suppress.
#[test]
fn enum_call_operand_stays_suppressed() {
    let source = "enum E { a }\nvoid f() { var r = E(1) + \"s\"; }\n";
    let r = report_ops(source);
    assert!(
        atna_rows(&r).is_empty(),
        "the enum `(` spells no creation: {:?}",
        rows(&r)
    );
}

// —— negative control: a mixin's `M(` is no creation upstream either —
// —— the `Mixin` `Created` kind keeps the same suppress even though the
// —— mixin declares the operator the leaked head would resolve.
#[test]
fn mixin_call_operand_stays_suppressed() {
    let source = "mixin M { int operator +(int o) => 0; }\n\
                  void f() { var r = M(1) + \"s\"; }\n";
    let r = report_ops(source);
    assert!(
        atna_rows(&r).is_empty(),
        "the mixin `(` spells no creation: {:?}",
        rows(&r)
    );
}
