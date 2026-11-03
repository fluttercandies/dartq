//! #3525 pins — a record scrutinee reaches `NameTy` as a record, so the
//! exhaustiveness engine can split it per field and the case rows decide
//! the way the oracle does.
//!
//! Before the fix three sites folded record knowledge away:
//!
//! (a) `handle_record_pattern` pushed a `Restriction::Unknown` marker for
//!     the case pattern — `collect_singles` turned it into a top face, so
//!     no record case ever covered a field.
//! (b) `close_record_pattern` never emitted a coverage leaf — the record
//!     frame's `fields` (`ObjectField` buckets the field handlers filled)
//!     were drained but never folded into the case's space.
//! (c) `handle_wildcard_pattern` only pushed leaves for `Ty::Named` — a
//!     written `Null` (`Null _`) went through `Ty::Null` and collapsed to
//!     `NamedContext`, so the oracle's `unreachable_switch_case` on the
//!     `Null _` row landed on a different case here.
//!
//! The leaf is now built where the oracle builds it
//! (`createRecordSpace`, upstream Dart SDK source
//! `pkg/_fe_analyzer_shared/lib/src/exhaustiveness/shared.dart`): every
//! `ObjectField` the record frame collected becomes a per-field
//! `Key::RecordIndex`/`Key::RecordName` column of the case's
//! `Space<NameTy>`, and the record's own `NameTy::Record{shape}` head
//! keeps the shape the columns key off. The required record type mirrors
//! `_createStaticTypeWithContext` — all-`Object?` fields unless the
//! scrutinee's matched type narrows it.
//!
//! Constant record patterns fold through `_convertConstantValue`'s
//! `RecordState` arm (upstream `pkg/analyzer` `exhaustiveness.dart`): a
//! `const r0 = (E.a, false)` case carries the record static type plus the
//! per-field value spaces.
//!
//! Oracle corpus: `tests/language/patterns/exhaustiveness/record_switch_test.dart`,
//! `record_literal_switch_test.dart`, `record_literal_named_switch_test.dart`.
//! Each pin below names the corpus function it reproduces; a negative
//! control proves the row still reports where the oracle does.

use super::test_rig::{assert_silent, report_ops, rows};

/// The corpus `enum E` + four constant records the constant-pattern
/// fixtures share (`record_switch_test.dart` head).
const CONSTS: &str = "enum E { a, b }\n\
                      const r0 = (E.a, false);\n\
                      const r1 = (E.b, false);\n\
                      const r2 = (E.a, true);\n\
                      const r3 = (E.b, true);\n";

/// Corpus `exhaustiveSwitch` — the four constant record cases cover
/// `(E, bool)` completely: silent.
#[test]
fn const_record_cases_exhaustive() {
    let report = report_ops(&format!(
        "{CONSTS}void f((E, bool) r) {{\n\
         \x20 switch (r) {{\n\
         \x20   case r0: break;\n\
         \x20   case r1: break;\n\
         \x20   case r2: break;\n\
         \x20   case r3: break;\n\
         \x20 }}\n\
         }}\n"
    ));
    assert_silent(&report);
}

/// Corpus `nonExhaustiveSwitch1` — `r1 = (E.b, false)` missing: the
/// oracle reports `non_exhaustive_switch_statement` at the `switch`
/// keyword with witness `(E.b, false)`.
#[test]
fn const_record_cases_missing_one_reports_at_switch() {
    let report = report_ops(&format!(
        "{CONSTS}void f((E, bool) r) {{\n\
         \x20 switch (r) {{\n\
         \x20   case r0: break;\n\
         \x20   case r2: break;\n\
         \x20   case r3: break;\n\
         \x20 }}\n\
         }}\n"
    ));
    let names = super::test_rig::code_names(&report);
    assert_eq!(
        names,
        vec!["non_exhaustive_switch_statement"],
        "expected exactly the non-exhaustive row: {:#?}",
        rows(&report)
    );
    // The anchor sits inside the `switch` keyword's span (corpus: error
    // column 3, len 6 — the `switch` token). The analyzed source is the
    // prelude + the full fixture (consts included); `switch` is the only
    // `switch` token in it.
    let offset = report.diagnostics[0].range.offset as usize;
    let analyzed = format!(
        "{}{}{}",
        super::test_rig::CORE_PRELUDE,
        CONSTS,
        "void f((E, bool) r) {\n switch (r) {\n   case r0: break;\n   case r2: break;\n   case r3: break;\n }\n}\n"
    );
    let kw = analyzed.find("switch").unwrap();
    assert!(
        (kw..kw + "switch".len() + 1).contains(&offset),
        "row must anchor inside the switch keyword @{kw}: {:#?}",
        rows(&report)
    );
}

/// Corpus `unreachableCase1` — `case r0` repeated: the oracle reports
/// `unreachable_switch_case` on the duplicate.
#[test]
fn const_record_case_duplicate_is_unreachable() {
    let report = report_ops(&format!(
        "{CONSTS}void f((E, bool) r) {{\n\
         \x20 switch (r) {{\n\
         \x20   case r0: break;\n\
         \x20   case r1: break;\n\
         \x20   case r2: break;\n\
         \x20   case r3: break;\n\
         \x20   case r0: break;\n\
         \x20 }}\n\
         }}\n"
    ));
    let names = super::test_rig::code_names(&report);
    assert_eq!(
        names,
        vec!["unreachable_switch_case"],
        "expected the duplicate-case row: {:#?}",
        rows(&report)
    );
}

/// Corpus `unreachableDefault` — a `default` behind the four covering
/// record cases is `unreachable_switch_default`.
#[test]
fn const_record_default_after_cover_is_unreachable() {
    let report = report_ops(&format!(
        "{CONSTS}void f((E, bool) r) {{\n\
         \x20 switch (r) {{\n\
         \x20   case r0: break;\n\
         \x20   case r1: break;\n\
         \x20   case r2: break;\n\
         \x20   case r3: break;\n\
         \x20   default: break;\n\
         \x20 }}\n\
         }}\n"
    ));
    let names = super::test_rig::code_names(&report);
    assert_eq!(
        names,
        vec!["unreachable_switch_default"],
        "expected the unreachable-default row: {:#?}",
        rows(&report)
    );
}

/// Corpus `exhaustiveNullableSwitch` — `(E, bool)?` with the four record
/// cases plus `case null`: silent.
#[test]
fn const_record_nullable_with_null_case_is_exhaustive() {
    let report = report_ops(&format!(
        "{CONSTS}void f((E, bool)? r) {{\n\
         \x20 switch (r) {{\n\
         \x20   case r0: break;\n\
         \x20   case r1: break;\n\
         \x20   case r2: break;\n\
         \x20   case r3: break;\n\
         \x20   case null: break;\n\
         \x20 }}\n\
         }}\n"
    ));
    assert_silent(&report);
}

/// Corpus `nonExhaustiveNullableSwitch1` — `(E, bool)?` without the
/// `null` case: `non_exhaustive_switch_statement` at `switch`.
#[test]
fn const_record_nullable_missing_null_reports() {
    let report = report_ops(&format!(
        "{CONSTS}void f((E, bool)? r) {{\n\
         \x20 switch (r) {{\n\
         \x20   case r0: break;\n\
         \x20   case r1: break;\n\
         \x20   case r2: break;\n\
         \x20   case r3: break;\n\
         \x20 }}\n\
         }}\n"
    ));
    let names = super::test_rig::code_names(&report);
    assert_eq!(
        names,
        vec!["non_exhaustive_switch_statement"],
        "expected the non-exhaustive row: {:#?}",
        rows(&report)
    );
}

/// Corpus `nonExhaustiveNullableSwitch2` — `null` present but `(E.b,
/// false)` missing: still non-exhaustive on the record side.
#[test]
fn const_record_nullable_missing_field_reports() {
    let report = report_ops(&format!(
        "{CONSTS}void f((E, bool)? r) {{\n\
         \x20 switch (r) {{\n\
         \x20   case r0: break;\n\
         \x20   case r2: break;\n\
         \x20   case r3: break;\n\
         \x20   case null: break;\n\
         \x20 }}\n\
         }}\n"
    ));
    let names = super::test_rig::code_names(&report);
    assert_eq!(
        names,
        vec!["non_exhaustive_switch_statement"],
        "expected the non-exhaustive row: {:#?}",
        rows(&report)
    );
}

/// Corpus `record_literal_switch_test.dart exhaustiveSwitch` — record
/// patterns (not constants) split the same way: silent.
#[test]
fn record_pattern_cases_exhaustive() {
    let report = report_ops(
        "enum E { a, b }\n\
         void f((E, bool) r) {\n\
         \x20 switch (r) {\n\
         \x20   case (E.a, false): break;\n\
         \x20   case (E.b, false): break;\n\
         \x20   case (E.a, true): break;\n\
         \x20   case (E.b, true): break;\n\
         \x20 }\n\
         }\n",
    );
    assert_silent(&report);
}

/// Corpus `nonExhaustiveSwitch1` of the literal file — `(E.b, false)`
/// pattern missing: non-exhaustive at `switch`.
#[test]
fn record_pattern_cases_missing_one_reports() {
    let report = report_ops(
        "enum E { a, b }\n\
         void f((E, bool) r) {\n\
         \x20 switch (r) {\n\
         \x20   case (E.a, false): break;\n\
         \x20   case (E.a, true): break;\n\
         \x20   case (E.b, true): break;\n\
         \x20 }\n\
         }\n",
    );
    let names = super::test_rig::code_names(&report);
    assert_eq!(
        names,
        vec!["non_exhaustive_switch_statement"],
        "expected the non-exhaustive row: {:#?}",
        rows(&report)
    );
}

/// Corpus `nonExhaustiveSwitchWithDefault` — `default` covers the rest:
/// silent (the default is reachable, so no `unreachable_switch_default`).
#[test]
fn record_pattern_default_only_is_silent() {
    let report = report_ops(
        "enum E { a, b }\n\
         void f((E, bool) r) {\n\
         \x20 switch (r) {\n\
         \x20   case (E.a, false): break;\n\
         \x20   default: break;\n\
         \x20 }\n\
         }\n",
    );
    assert_silent(&report);
}

/// Corpus `record_literal_named_switch_test.dart exhaustiveSwitch` —
/// named fields `({E a, bool b})` split per `Key::RecordName`: silent.
#[test]
fn named_record_pattern_cases_exhaustive() {
    let report = report_ops(
        "enum E { a, b }\n\
         void f(({E a, bool b}) r) {\n\
         \x20 switch (r) {\n\
         \x20   case (a: E.a, b: false): break;\n\
         \x20   case (a: E.b, b: false): break;\n\
         \x20   case (a: E.a, b: true): break;\n\
         \x20   case (a: E.b, b: true): break;\n\
         \x20 }\n\
         }\n",
    );
    assert_silent(&report);
}

/// Corpus named-file `nonExhaustiveSwitch1` — `(a: E.b, b: false)`
/// missing: non-exhaustive.
#[test]
fn named_record_pattern_missing_one_reports() {
    let report = report_ops(
        "enum E { a, b }\n\
         void f(({E a, bool b}) r) {\n\
         \x20 switch (r) {\n\
         \x20   case (a: E.a, b: false): break;\n\
         \x20   case (a: E.a, b: true): break;\n\
         \x20   case (a: E.b, b: true): break;\n\
         \x20 }\n\
         }\n",
    );
    let names = super::test_rig::code_names(&report);
    assert_eq!(
        names,
        vec!["non_exhaustive_switch_statement"],
        "expected the non-exhaustive row: {:#?}",
        rows(&report)
    );
}

/// Defect (c): a written `Null` wildcard `Null _` is the `Null` static
/// type — it covers only the null space, so over `int?` the `int _` case
/// behind it stays live (non-exhaustive is impossible: `int?` splits to
/// `int` + `Null`). The oracle's `unreachable_switch_case` lands on a
/// SECOND `Null _` / `null` row — before the fix the leaf collapsed to
/// `NamedContext` and the row landed on `int _` instead.
#[test]
fn null_wildcard_covers_null_space_over_nullable() {
    let report = report_ops(
        "void f(int? o) {\n\
         \x20 switch (o) {\n\
         \x20   case null: break;\n\
         \x20   case Null _: break;\n\
         \x20   case int _: break;\n\
         \x20 }\n\
         }\n",
    );
    let names = super::test_rig::code_names(&report);
    assert_eq!(
        names,
        vec!["unreachable_switch_case"],
        "the duplicate null-space case must be the unreachable one: {:#?}",
        rows(&report)
    );
    // The unreachable row anchors on the `Null _` case — the second
    // null-space case — not the `int _` behind it (the pre-fix bug
    // placed it there).
    let fixture = "void f(int? o) {\n switch (o) {\n   case null: break;\n   case Null _: break;\n   case int _: break;\n }\n}\n";
    let analyzed = format!("{}{}", super::test_rig::CORE_PRELUDE, fixture);
    let offset = report.diagnostics[0].range.offset as usize;
    let null_case = analyzed.find("case Null").unwrap();
    let int_case = analyzed.find("case int").unwrap();
    assert!(
        (null_case..int_case).contains(&offset),
        "unreachable_switch_case must anchor inside the `Null _` case span \
         [case Null @{null_case}, case int @{int_case}), got @{offset}: {:#?}",
        rows(&report)
    );
}

/// Same defect, other ordering: `Null _` FIRST then `null` — the `null`
/// case is the unreachable one (a written `Null` and the `null` literal
/// cover the same space, upstream `static_type.dart:21`).
#[test]
fn null_wildcard_first_makes_null_case_unreachable() {
    let report = report_ops(
        "void f(int? o) {\n\
         \x20 switch (o) {\n\
         \x20   case Null _: break;\n\
         \x20   case null: break;\n\
         \x20   case int _: break;\n\
         \x20 }\n\
         }\n",
    );
    let names = super::test_rig::code_names(&report);
    assert_eq!(
        names,
        vec!["unreachable_switch_case"],
        "the `null` case behind `Null _` must be unreachable: {:#?}",
        rows(&report)
    );
    let fixture = "void f(int? o) {\n switch (o) {\n   case Null _: break;\n   case null: break;\n   case int _: break;\n }\n}\n";
    let analyzed = format!("{}{}", super::test_rig::CORE_PRELUDE, fixture);
    let offset = report.diagnostics[0].range.offset as usize;
    let null_case = analyzed.find("case null").unwrap();
    let int_case = analyzed.find("case int").unwrap();
    assert!(
        (null_case..int_case).contains(&offset),
        "unreachable_switch_case must anchor inside the `null` case span \
         [case null @{null_case}, case int @{int_case}), got @{offset}: {:#?}",
        rows(&report)
    );
}

/// Negative control: a record scrutinee with an uncovered field still
/// reports — the record path must not blanket-silence switches (the
/// `Restriction::Unknown` pre-fix shape under-reported in the other
/// direction).
#[test]
fn record_scrutinee_still_reports_non_exhaustive() {
    let report = report_ops(
        "enum E { a, b }\n\
         void f((E, bool) r) {\n\
         \x20 switch (r) {\n\
         \x20   case (E.a, false): break;\n\
         \x20 }\n\
         }\n",
    );
    let names = super::test_rig::code_names(&report);
    assert_eq!(
        names,
        vec!["non_exhaustive_switch_statement"],
        "a single record case must not silence the switch: {:#?}",
        rows(&report)
    );
}

/// Negative control (c): `Null _` alone over a NON-nullable scrutinee
/// reports the same face `case null` does —
/// `pattern_never_matches_value_type` plus `dead_code` on the body (the
/// oracle's nullability-disjoint cut, probe2444 shape): the `Null` leaf
/// covers nothing of `int`, so the case can never match and its body is
/// dead.
#[test]
fn null_wildcard_over_non_nullable_reports_never_match() {
    let report = report_ops(
        "void f(int o) {\n\
         \x20 switch (o) {\n\
         \x20   case Null _: break;\n\
         \x20 }\n\
         }\n",
    );
    let names = super::test_rig::code_names(&report);
    assert!(
        names.contains(&"pattern_never_matches_value_type"),
        "Null _ over int must report never-match: {:#?}",
        rows(&report)
    );
    assert!(
        names.contains(&"dead_code"),
        "the never-matching body must be dead: {:#?}",
        rows(&report)
    );
    assert!(
        !names.contains(&"non_exhaustive_switch_statement"),
        "the never-match face reports instead: {:#?}",
        rows(&report)
    );
}
