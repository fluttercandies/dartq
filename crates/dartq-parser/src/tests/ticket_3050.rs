//! #3050 pins: mixin-header recovery — out-of-order / junk tokens between
//! the `on` and `implements` clauses surface on the junk token itself and
//! never collapse into `expected_body` / `missing_function_parameters`.
//!
//! Root (upstream pin the pinned Dart SDK 3.13.7,
//! `pkg/_fe_analyzer_shared/lib/src/parser/parser_impl.dart`):
//!
//! * `parseMixinHeaderRecovery` (:3445) opens every loop iteration with
//!   `skipUnexpectedTokenOpt(token, ['on', 'implements', '{'])` (:3475):
//!   a non-keyword token whose follower carries the `on`/`implements`
//!   string value (or is the `{` token) is reported `unexpectedToken` on
//!   itself and skipped, so the clause walk resumes on the clause keyword.
//! * The misspelled `extend`/`extends` gate (:3519) feeds the same
//!   `parseMixinOn` (:3555) whose `onKeyword` anchor is the misspelled
//!   token itself (the `token.next!` read at :3558), and the duplicate /
//!   order checks (:3490-3503) run for both `on` arms alike.
//!
//! Oracle A/B (dart analyze, parser rows only): clean `mixin O on A {}`
//! and the valid `mixin M on A implements B {}` are silent; the junk
//! cases below carry exactly the rows pinned here.

use super::collector::{Collector, latest_features, scan_ok};

fn rows_with(source: &'static str) -> Vec<(String, u32, u32)> {
    let (table, text) = scan_ok(source);
    let mut collector = Collector::default();
    let mut parser = crate::Parser::new(text, table, latest_features(), &mut collector);
    parser.parse_unit();
    parser.listener.errors.clone()
}

/// Clean `mixin O on A {}` (oracle: zero parser rows).
#[test]
fn clean_mixin_on_is_silent() {
    let rows = rows_with("mixin O on A {}\n");
    assert!(rows.is_empty(), "clean `mixin O on A {{}}` must be silent, got {rows:?}");
}

/// The valid two-clause header `mixin M on A implements B {}` (oracle: zero
/// parser rows) — the recovery loop must not be entered at all.
#[test]
fn clean_mixin_on_implements_is_silent() {
    let rows = rows_with("mixin M on A implements B {}\n");
    assert!(
        rows.is_empty(),
        "valid `mixin M on A implements B {{}}` must be silent, got {rows:?}"
    );
}

/// `mixin M on implements B {}` (the recon core shape, oracle 1:23
/// `unexpectedToken` on `B`): the built-in `implements` parses as the `on`
/// constraint (ComplexType recovered face), leaving `B` as junk before the
/// body — the recovery's unexpected-token skip reports `B` and clears it so
/// the body `{` is found. The collapse faces (`expected_body`,
/// `missing_function_parameters`) must never appear. The type-reference
/// face ON `implements` itself is the TypeReference context's business
/// (oracle `built_in_identifier_as_type`, recovery.rs lane) and is
/// deliberately not pinned here.
#[test]
fn on_clause_builtin_type_junk_clears_to_body() {
    let rows = rows_with("mixin M on implements B {}\n");
    assert!(
        rows.contains(&("unexpected_token".into(), 22, 23)),
        "one unexpected_token on the junk `B` before the body, got {rows:?}"
    );
    assert!(
        !rows.iter().any(|(code, _, _)| code == "expected_body"),
        "the body must be found, no expected_body collapse, got {rows:?}"
    );
    assert!(
        !rows.iter()
            .any(|(code, _, _)| code == "missing_function_parameters"),
        "no member reparse collapse, got {rows:?}"
    );
}

/// `mixin N on class {}` (oracle: `expectedType` on `class` 1:12 +
/// `expectedBody` 1:12 + `missingIdentifier` on the `{` 1:18 — the reserved
/// word is NOT a skipUnexpectedTokenOpt candidate, keywords are exempt): the
/// reserved word survives the recovery loop for the body-side faces and the
/// recovery adds no unexpected_token of its own.
#[test]
fn on_clause_reserved_word_survives_recovery() {
    let rows = rows_with("mixin N on class {}\n");
    assert!(
        rows.contains(&("expected_type_name".into(), 11, 16)),
        "expected_type_name on the reserved `class`, got {rows:?}"
    );
    assert!(
        rows.contains(&("expected_body".into(), 11, 16)),
        "expected_body on the reserved `class`, got {rows:?}"
    );
    assert!(
        rows.contains(&("missing_identifier".into(), 17, 18)),
        "missing_identifier on the `{{`, got {rows:?}"
    );
    assert!(
        !rows.iter().any(|(code, _, _)| code == "unexpected_token"),
        "recovery must not skip a keyword junk token, got {rows:?}"
    );
    assert!(
        !rows.iter()
            .any(|(code, _, _)| code == "missing_function_parameters"),
        "no member reparse collapse, got {rows:?}"
    );
}

/// `mixin X on A B {}` (oracle 1:14 `unexpectedToken` on `B`, single row):
/// identifier junk whose follower is the body `{` is reported and skipped.
#[test]
fn identifier_junk_before_body_is_skipped() {
    let rows = rows_with("mixin X on A B {}\n");
    assert_eq!(
        rows,
        vec![("unexpected_token".to_owned(), 13, 14)],
        "exactly one unexpected_token on the junk `B`, got {rows:?}"
    );
}

/// `mixin W on A junk on B {}` (oracle: `unexpectedToken` on `junk` 1:14 +
/// `multiple_on_clauses` on the second `on` 1:19): the skipped junk lets the
/// second `on` arm parse and take the duplicate-clause check.
#[test]
fn skipped_junk_feeds_second_on_duplicate_check() {
    let rows = rows_with("mixin W on A junk on B {}\n");
    assert!(rows.contains(&("unexpected_token".into(), 13, 17)), "got {rows:?}");
    assert!(
        rows.contains(&("multiple_on_clauses".into(), 18, 20)),
        "got {rows:?}"
    );
    assert_eq!(rows.len(), 2, "no extra rows, got {rows:?}");
}

/// `mixin S extends A {}` (oracle 1:9 `expectedInstead('on')`, single row):
/// the misspelled-arm report anchors on the `extends` token.
#[test]
fn misspelled_extends_reports_expected_instead_on() {
    let rows = rows_with("mixin S extends A {}\n");
    assert_eq!(
        rows,
        vec![("expected_instead".to_owned(), 8, 15)],
        "got {rows:?}"
    );
}

/// `mixin Y extend A {}` (oracle 1:9 `expectedInstead('on')`, single row):
/// the identifier-shaped `extend` takes the same gate.
#[test]
fn misspelled_extend_reports_expected_instead_on() {
    let rows = rows_with("mixin Y extend A {}\n");
    assert_eq!(
        rows,
        vec![("expected_instead".to_owned(), 8, 14)],
        "got {rows:?}"
    );
}

/// `mixin T on A on B {}` (oracle 1:14 `multiple_on_clauses` on the second
/// `on`, single row).
#[test]
fn duplicate_on_clause_reports_once() {
    let rows = rows_with("mixin T on A on B {}\n");
    assert_eq!(
        rows,
        vec![("multiple_on_clauses".to_owned(), 13, 15)],
        "got {rows:?}"
    );
}

/// `mixin U implements A on B {}` (oracle 1:22 `implements_before_on` on the
/// late `on`, single row).
#[test]
fn late_on_clause_reports_implements_before_on() {
    let rows = rows_with("mixin U implements A on B {}\n");
    assert_eq!(
        rows,
        vec![("implements_before_on".to_owned(), 21, 23)],
        "got {rows:?}"
    );
}

/// `mixin V on A implements B implements C {}` (oracle 1:27
/// `multiple_implements_clauses` on the second `implements`, single row).
#[test]
fn duplicate_implements_clause_reports_once() {
    let rows = rows_with("mixin V on A implements B implements C {}\n");
    assert_eq!(
        rows,
        vec![("multiple_implements_clauses".to_owned(), 26, 36)],
        "got {rows:?}"
    );
}
