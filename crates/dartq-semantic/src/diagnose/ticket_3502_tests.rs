//! #3502 pins — an extension whose `on` type is nullable applies to the
//! operator and compound-assignment faces of a nullable receiver.
//!
//! # The defect
//!
//! `operator_face_extension_resolved` probed only the row's own
//! [`Intent::Operator`] spellings. The compound/prefix/postfix operand row
//! carries a READ intent (the `+=`/`++` token rides the token-table
//! adjacency, not the intent), so its probe set was empty, the `on int?`
//! face never entered the application, and the nullable tail reported
//! `unchecked_use_of_nullable_value` at the operator token — with the
//! builtin `int.operator+` fallback signature then inventing the
//! `argument_type_not_assignable`/`invalid_assignment` and
//! `invalid_null_aware_operator` rows the oracle never emits
//! (~510 `control_flow_collections` rows, all over-report).
//!
//! # The contract
//!
//! `applicableTo` instantiates the extended type against the receiver
//! (upstream Dart SDK source,
//! pkg/analyzer/lib/src/dart/resolver/extension_member_resolver.dart's
//! `_getApplicable`): an `on int?` face accepts the `int?` receiver as the
//! SAFE match — `ExtensionApp::Single`, the silent arm — while an `on int`
//! face against the same receiver is the `UnsafeSingle` arm and keeps the
//! unchecked report. The adjacent probe added to
//! `operator_face_extension_resolved` asks the compound spelling (`x +=
//! 1`'s `+`, `x++`'s `+`) through the same `extension_application` — the
//! resolver's compound consult runs the identical `findExtension` before
//! the `isNullable` tail fires (binary_expression_resolver.dart /
//! assignment_expression_resolver.dart).
//!
//! Every expectation below was oracle-verified against
//! `~/.dartq-oracle/3.13.7/dart-sdk/bin/dart analyze` (probes under
//! /tmp/3494 and /tmp/3502).

use super::test_rig::{assert_silent, code_names, report_ops, rows};

/// ① The discriminating repro, verbatim: `extension on int?` declares the
/// arithmetic operators and a null-aware element writes `?x op= e` — the
/// oracle reports NOTHING (no `unchecked_use_of_nullable_value`, no
/// `argument_type_not_assignable`, no `invalid_assignment`, no
/// `invalid_null_aware_operator`).
#[test]
fn nullable_on_extension_applies_to_null_aware_compounds() {
    let report = report_ops(
        "extension on int? {\n\
         \x20 int? operator +(int? o) => o;\n\
         \x20 int? operator -(int? o) => o;\n\
         \x20 int? operator *(int? o) => o;\n\
         \x20 int? operator %(int? o) => o;\n\
         \x20 int? operator ~/(int? o) => o;\n\
         }\n\
         void main() {\n\
         \x20 int? x = 2 > 1 ? 1 : null;\n\
         \x20 [?x += 1, ?x -= 1, ?x *= 1, ?x %= 1, ?x ~/= 1, ?x += null];\n\
         }\n",
    );
    assert_silent(&report);
}

/// ② The plain compound form: `x += 1` over an `int?` operand resolves the
/// `on int?` face's `operator +` — silent.
#[test]
fn nullable_on_extension_applies_to_compound_assign() {
    let report = report_ops(
        "extension on int? {\n\
         \x20 int? operator +(int? o) => o;\n\
         }\n\
         void main() {\n\
         \x20 int? x = 2 > 1 ? 1 : null;\n\
         \x20 x += 1;\n\
         \x20 x++;\n\
         \x20 x + 1;\n\
         }\n",
    );
    assert_silent(&report);
}

/// ③ The binary operator face: `x + 1` resolves the same extension —
/// silent (guards the `Intent::Operator` arm the compound fix must not
/// break).
#[test]
fn nullable_on_extension_applies_to_binary_operator() {
    let report = report_ops(
        "extension on int? {\n\
         \x20 int? operator +(int? o) => o;\n\
         }\n\
         void main() {\n\
         \x20 int? x = 2 > 1 ? 1 : null;\n\
         \x20 x + 1;\n\
         }\n",
    );
    assert_silent(&report);
}

/// ④ The unsafe control: an `on int` (non-nullable) face never accepts an
/// `int?` receiver — `applicableTo` rejects it — so `x += 1` and `x++`
/// keep the `unchecked_use_of_nullable_value` report at the operator
/// token. The fix must not collapse the `Single`/`UnsafeSingle` split.
#[test]
fn non_nullable_on_extension_keeps_unchecked_on_nullable_receiver() {
    let report = report_ops(
        "extension on int {\n\
         \x20 int operator +(int o) => o;\n\
         }\n\
         void main() {\n\
         \x20 int? x = 2 > 1 ? 1 : null;\n\
         \x20 x += 1;\n\
         \x20 x++;\n\
         }\n",
    );
    let unchecked: Vec<_> = code_names(&report)
        .into_iter()
        .filter(|name| *name == "unchecked_use_of_nullable_value")
        .collect();
    assert_eq!(
        unchecked.len(),
        2,
        "expected unchecked rows at += and ++, got {:#?}",
        rows(&report)
    );
}

/// ⑤ The non-nullable control: `extension on int` over an `int` receiver
/// resolves the operator silently — the extension still wins over the
/// builtin interface face where the oracle says it does.
#[test]
fn non_nullable_on_extension_resolves_for_non_nullable_receiver() {
    let report = report_ops(
        "extension on int {\n\
         \x20 int frobnicate(int o) => o;\n\
         }\n\
         void main() {\n\
         \x20 int x = 1;\n\
         \x20 x.frobnicate(2);\n\
         }\n",
    );
    assert_silent(&report);
}
