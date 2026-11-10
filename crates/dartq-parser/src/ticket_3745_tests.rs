//! Ticket #3745 — `X extension`/pseudo-modifier top-level dispatch: sealed,
//! base and interface are NOT declaration modifiers (upstream `isModifier`,
//! modifier_context.dart:13 — the modifier keyword set is abstract/augment/
//! const/covariant/external/final/late/required/static/var). In
//! `parseTopLevelDeclarationImpl` (parser_impl.dart:591-626) the pseudo
//! capture only applies when the keyword is followed by class/mixin/enum
//! (sealed also `abstract class`); before any other token — including
//! `extension` and `typedef` — the pseudo keyword stays in the token
//! stream and `parseTopLevelMemberImpl` parses the construct as a member:
//!
//! * `sealed extension E on C {}` — `sealed` commits as a type
//!   (`looksLikeName(extension)` is true), the `extension` name slot's
//!   keep-set rejects the declaration head and splices a synthetic name:
//!   oracle `expected_token ';'` at `sealed` + `missing_identifier` at
//!   `extension` (+ the semantic `undefined_class` on the `sealed` type),
//!   then `extension E on C {}` reparses as a clean extension declaration.
//!   `base` decomposes identically.
//! * `interface extension E on C {}` — `interface` is a built-in and
//!   never commits as a type (`isValidNonRecordTypeReference` requires a
//!   `.` follower), so it becomes the field NAME:
//!   `missing_const_final_var_or_type` + `expected_token ';'` at
//!   `interface`, `extension E ...` reparsed.
//! * `sealed`/`base`/`interface typedef T = C` — `looksLikeName(typedef)`
//!   is false (upstream typedef exclusion: `typedef` followed by an
//!   identifier reads as a separate declaration), so the pseudo keyword
//!   becomes the NAME: `missing_const_final_var_or_type` +
//!   `expected_token ';'` at the keyword, `typedef ...` reparsed.
//!
//! The companion fix gates `parse_mixin`'s verifier-face
//! `built_in_identifier_in_declaration` report on the keyword actually
//! becoming the mixin name (`at.id == name.id`, the class-lane pattern):
//! `mixin extension/enum/typedef/mixin <name>` splices a synthetic name
//! at the declaration-head keyword — the keyword stays for the reparse
//! and no built-in-name row exists upstream. `mixin abstract class X {}`
//! keeps the row because `abstract` is not a declaration-head keyword and
//! does become the mixin name.

use std::collections::BTreeSet;

use crate::parser::Parser;
use crate::tests::collector::Collector;
use dartq_language::{Feature, FeatureSet};
use dartq_scanner::ScanConfig;

/// Parses `source` at `set` and returns the scanner+parser rows as
/// `(code, start, end)` — UTF-16 offsets, matching the ticket_3740 harness.
fn rows_at(source: &'static str, set: FeatureSet) -> BTreeSet<(String, u32, u32)> {
    let config = ScanConfig::from_feature_set(set);
    let features = crate::ParserFeatures::from_feature_set(set);
    let mut files = dartq_source::FileTable::new();
    let file = files.intern(std::path::Path::new("test.dart"));
    let result = dartq_scanner::scan(source, config, file);
    let mut rows: BTreeSet<(String, u32, u32)> = result
        .diagnostics
        .iter()
        .map(|d| {
            (
                d.code.name.to_owned(),
                d.range.offset,
                d.range.offset + d.range.length,
            )
        })
        .collect();
    let mut parser = Parser::new(
        source,
        Box::leak(Box::new(result.tokens)),
        features,
        Box::leak(Box::new(Collector::default())),
    );
    parser.parse_unit();
    rows.extend(parser.listener.errors.iter().cloned());
    rows
}

fn augment_set() -> FeatureSet {
    FeatureSet::latest().with_experiment(Feature::Augmentations)
}

// ------------------------------------------------------------------
// pseudo modifier before `extension` — member/type-name lane
// ------------------------------------------------------------------

#[test]
fn sealed_extension_reparses_as_extension() {
    // Oracle on `sealed extension E3 on C {}`: `expected_token ';'` at
    // `sealed` (0,6) + `missing_identifier` at `extension` (7,16); the
    // extension declaration itself reparses cleanly.
    let rows = rows_at("sealed extension E3 on C {}\nclass C {}\n", augment_set());
    assert!(
        rows.contains(&("expected_token".to_owned(), 0, 6)),
        "expected expected_token at (0,6), got {rows:?}"
    );
    assert!(
        rows.contains(&("missing_identifier".to_owned(), 7, 16)),
        "expected missing_identifier at (7,16), got {rows:?}"
    );
    // No row may be anchored on the `on`/`{}` extension tail — it parsed
    // as a real extension declaration.
    for code in [
        "expected_body",
        "expected_type_name",
        "extraneous_modifier",
        "missing_const_final_var_or_type",
    ] {
        assert!(
            !rows.iter().any(|(c, _, _)| c == code),
            "unexpected {code} row: {rows:?}"
        );
    }
}

#[test]
fn base_extension_reparses_as_extension() {
    // `base extension E6 on C {}` decomposes like `sealed`: `base` is the
    // field type, `extension` keep-set rejects (5,14 is the `extension`
    // span: `base`=0-3, ` `=4, `extension`=5-13).
    let rows = rows_at("base extension E6 on C {}\nclass C {}\n", augment_set());
    assert!(
        rows.contains(&("expected_token".to_owned(), 0, 4)),
        "expected expected_token at (0,4), got {rows:?}"
    );
    assert!(
        rows.contains(&("missing_identifier".to_owned(), 5, 14)),
        "expected missing_identifier at (5,14), got {rows:?}"
    );
    assert!(
        !rows
            .iter()
            .any(|(c, _, _)| c == "missing_const_final_var_or_type"),
        "unexpected missing_const_final_var_or_type row: {rows:?}"
    );
}

#[test]
fn interface_extension_reports_field_rows() {
    // `interface extension E4 on C {}`: `interface` is a built-in and does
    // not commit as a type, so it is the field NAME —
    // `missing_const_final_var_or_type` at `interface` (0,9) +
    // `expected_token ';'` at `interface` (0,9); `extension E4 ...` is
    // reparsed as an extension declaration.
    let rows = rows_at(
        "interface extension E4 on C {}\nclass C {}\n",
        augment_set(),
    );
    assert!(
        rows.contains(&("missing_const_final_var_or_type".to_owned(), 0, 9)),
        "expected missing_const_final_var_or_type at (0,9), got {rows:?}"
    );
    assert!(
        rows.contains(&("expected_token".to_owned(), 0, 9)),
        "expected expected_token at (0,9), got {rows:?}"
    );
    assert!(
        !rows.iter().any(|(c, _, _)| c == "missing_identifier"),
        "unexpected missing_identifier row: {rows:?}"
    );
}

// ------------------------------------------------------------------
// pseudo modifier before `typedef` — the name lane
// ------------------------------------------------------------------

#[test]
fn sealed_typedef_reports_field_rows() {
    // `sealed typedef T3 = C;`: `looksLikeName(typedef)` is false
    // (typedef+identifier reads as a separate declaration), so `sealed`
    // is the field NAME — `missing_const_final_var_or_type` (0,6) +
    // `expected_token ';'` (0,6); `typedef T3 = C;` reparses as a typedef.
    let rows = rows_at("sealed typedef T3 = C;\n", augment_set());
    assert!(
        rows.contains(&("missing_const_final_var_or_type".to_owned(), 0, 6)),
        "expected missing_const_final_var_or_type at (0,6), got {rows:?}"
    );
    assert!(
        rows.contains(&("expected_token".to_owned(), 0, 6)),
        "expected expected_token at (0,6), got {rows:?}"
    );
    assert!(
        !rows.iter().any(|(c, _, _)| c == "missing_identifier"),
        "unexpected missing_identifier row: {rows:?}"
    );
}

#[test]
fn interface_typedef_reports_field_rows() {
    // Same decomposition for `interface typedef T4 = C;`: `interface` is
    // the field name (`interface`=0-8).
    let rows = rows_at("interface typedef T4 = C;\n", augment_set());
    assert!(
        rows.contains(&("missing_const_final_var_or_type".to_owned(), 0, 9)),
        "expected missing_const_final_var_or_type at (0,9), got {rows:?}"
    );
    assert!(
        rows.contains(&("expected_token".to_owned(), 0, 9)),
        "expected expected_token at (0,9), got {rows:?}"
    );
}

#[test]
fn base_typedef_reports_field_rows() {
    // `base typedef T6 = C;`: `base` is the field name (0,4).
    let rows = rows_at("base typedef T6 = C;\n", augment_set());
    assert!(
        rows.contains(&("missing_const_final_var_or_type".to_owned(), 0, 4)),
        "expected missing_const_final_var_or_type at (0,4), got {rows:?}"
    );
    assert!(
        rows.contains(&("expected_token".to_owned(), 0, 4)),
        "expected expected_token at (0,4), got {rows:?}"
    );
}

// ------------------------------------------------------------------
// `mixin` head before a declaration-head keyword — no built-in-name row
// ------------------------------------------------------------------

#[test]
fn mixin_extension_splices_without_builtin_row() {
    // Oracle on `mixin extension E5 on C {}`: `missing_identifier` at
    // `extension` (6,15) + `expected_body` at `extension` (6,15) — the
    // keyword splices a synthetic mixin name and the extension reparses;
    // NO `built_in_identifier_in_declaration` (the keyword never became
    // the mixin name).
    let rows = rows_at("mixin extension E5 on C {}\nclass C {}\n", augment_set());
    assert!(
        rows.contains(&("missing_identifier".to_owned(), 6, 15)),
        "expected missing_identifier at (6,15), got {rows:?}"
    );
    assert!(
        rows.contains(&("expected_body".to_owned(), 6, 15)),
        "expected expected_body at (6,15), got {rows:?}"
    );
    assert!(
        !rows
            .iter()
            .any(|(c, _, _)| c == "built_in_identifier_in_declaration"),
        "extra built_in_identifier_in_declaration row: {rows:?}"
    );
}

#[test]
fn mixin_typedef_splices_without_builtin_row() {
    // Oracle on `mixin typedef T5 = C;`: `missing_identifier` (6,13) +
    // `expected_body` (6,13) only — no built-in-name row.
    let rows = rows_at("mixin typedef T5 = C;\nclass C {}\n", augment_set());
    assert!(
        rows.contains(&("missing_identifier".to_owned(), 6, 13)),
        "expected missing_identifier at (6,13), got {rows:?}"
    );
    assert!(
        rows.contains(&("expected_body".to_owned(), 6, 13)),
        "expected expected_body at (6,13), got {rows:?}"
    );
    assert!(
        !rows
            .iter()
            .any(|(c, _, _)| c == "built_in_identifier_in_declaration"),
        "extra built_in_identifier_in_declaration row: {rows:?}"
    );
}

#[test]
fn mixin_abstract_keeps_builtin_row() {
    // Negative control: `mixin abstract class X {}` — `abstract` is NOT a
    // declaration-head keyword so the keep-set does not fire; `abstract`
    // becomes the mixin name and the verifier row survives:
    // `expected_body` (6,14) + `built_in_identifier_in_declaration`
    // (6,14).
    let rows = rows_at("mixin abstract class X {}\n", augment_set());
    assert!(
        rows.contains(&("built_in_identifier_in_declaration".to_owned(), 6, 14)),
        "expected built_in_identifier_in_declaration at (6,14), got {rows:?}"
    );
    assert!(
        rows.contains(&("expected_body".to_owned(), 6, 14)),
        "expected expected_body at (6,14), got {rows:?}"
    );
}

// ------------------------------------------------------------------
// declaration-head captures keep working
// ------------------------------------------------------------------

#[test]
fn sealed_class_still_dispatches_as_class() {
    // `sealed class C {}` stays a class declaration — the conditional
    // capture must not break the class/mixin/enum follower path.
    let rows = rows_at("sealed class C {}\n", augment_set());
    assert!(
        !rows
            .iter()
            .any(|(c, _, _)| c == "missing_identifier" || c == "expected_token"),
        "sealed class produced recovery rows: {rows:?}"
    );
}

#[test]
fn base_mixin_still_dispatches_as_mixin() {
    // `base mixin M {}` stays a mixin declaration.
    let rows = rows_at("base mixin M {}\n", augment_set());
    assert!(
        !rows
            .iter()
            .any(|(c, _, _)| c == "missing_identifier" || c == "expected_token"),
        "base mixin produced recovery rows: {rows:?}"
    );
}
