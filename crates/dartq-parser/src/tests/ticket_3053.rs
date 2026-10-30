//! #3053 pins: the type-reference family's keyword faces in
//! `ensure_identifier` — a built-in in a type position reports
//! `built_in_identifier_as_type` and is CONSUMED as the type name (never
//! `missing_identifier`, never a silent swallow); a reserved word reports
//! `expected_type_name` and is consumed.
//!
//! Root (upstream pin `.third/dart-sdk-3.13.7`,
//! `pkg/_fe_analyzer_shared/lib/src/parser/identifier_context_impl.dart`,
//! `TypeReferenceIdentifierContext.ensureIdentifier` :1507-1560):
//!
//! * built-in, plain/continuation faces (`isBuiltInIdentifierAllowed:
//!   false`, :1512/:1542-1549) → report `builtInIdentifierAsType`, return
//!   the token (consumed; `handleIdentifier` rides the parser-level wrapper,
//!   parser_impl.dart:4031);
//! * built-in, prefixed face (`isBuiltInIdentifierAllowed: true`, :1523)
//!   → silent;
//! * reserved word → the `expectedType` else-branch (:1553-1556), consumed;
//! * `void`/`dynamic`/`Function` never reach the keyword branch —
//!   `isValidNonRecordTypeReference` early-returns them as valid types
//!   (`pkg/_fe_analyzer_shared/lib/src/parser/type_info.dart:139-151`).
//!
//! Oracle A/B (dart analyze 3.13.x, parser rows only): `mixin M on
//! implements B {}` carries `built_in_identifier_as_type` 1:12 +
//! `unexpected_token` 1:23; `class C extends implements {}` and
//! `class D extends a.implements {}` carry the single
//! `built_in_identifier_as_type`; `class C extends a.class {}` carries the
//! single `expected_type_name`; `class D extends dynamic {}` and
//! `int Function()` are silent.

use super::collector::{Collector, latest_features, scan_ok};

fn rows_with(source: &'static str) -> Vec<(String, u32, u32)> {
    let (table, text) = scan_ok(source);
    let mut collector = Collector::default();
    let mut parser = crate::Parser::new(text, table, latest_features(), &mut collector);
    parser.parse_unit();
    parser.listener.errors.clone()
}

/// `mixin M on implements B {}` (oracle 1:12 `built_in_identifier_as_type`
/// on `implements` + 1:23 `unexpected_token` on `B`): the #3050 residual —
/// the built-in consumed as the `on` constraint takes the type-reference
/// face instead of a silent pass or a `missing_identifier` slot.
#[test]
fn on_clause_builtin_type_reports_built_in_identifier_as_type() {
    let rows = rows_with("mixin M on implements B {}\n");
    assert!(
        rows.contains(&("built_in_identifier_as_type".into(), 11, 21)),
        "built_in_identifier_as_type on the consumed `implements`, got {rows:?}"
    );
    assert!(
        rows.contains(&("unexpected_token".into(), 22, 23)),
        "the #3050 unexpected_token on the junk `B` must survive, got {rows:?}"
    );
    assert!(
        !rows.iter().any(|(code, _, _)| code == "missing_identifier"),
        "the keyword is consumed as the type, no missing_identifier, got {rows:?}"
    );
}

/// `class C extends implements {}` (oracle: single parser row
/// `built_in_identifier_as_type` 1:17): the extends clause's recovered
/// built-in type is reported on the keyword and consumed, leaving `{` for
/// the body.
#[test]
fn extends_builtin_type_reports_and_consumes() {
    let rows = rows_with("class C extends implements {}\n");
    assert_eq!(
        rows,
        vec![("built_in_identifier_as_type".to_owned(), 16, 26)],
        "got {rows:?}"
    );
}

/// `class D extends a.implements {}` (oracle: single parser row
/// `built_in_identifier_as_type` 1:19): the continuation face after the
/// dot rejects built-ins (`isBuiltInIdentifierAllowed: false`) where the
/// prefixed head face would allow them.
#[test]
fn continuation_builtin_type_reports_and_consumes() {
    let rows = rows_with("class D extends a.implements {}\n");
    assert_eq!(
        rows,
        vec![("built_in_identifier_as_type".to_owned(), 18, 28)],
        "got {rows:?}"
    );
}

/// `class C extends a.class {}` (oracle: single parser row
/// `expected_type_name` 1:19): a reserved word in the continuation takes
/// the `expectedType` else-branch and is consumed as the type name.
#[test]
fn continuation_reserved_type_reports_expected_type_name() {
    let rows = rows_with("class C extends a.class {}\n");
    assert_eq!(
        rows,
        vec![("expected_type_name".to_owned(), 18, 23)],
        "got {rows:?}"
    );
}

/// `class C extends implements.Foo {}` (oracle: zero parser rows — the
/// built-in + `.` head passes `isValidNonRecordTypeReference`,
/// type_info.dart:145, and the prefixed face allows built-ins): the
/// prefixed head is consumed silently.
#[test]
fn prefixed_builtin_head_is_silent() {
    let rows = rows_with("class C extends implements.Foo {}\n");
    assert!(
        rows.is_empty(),
        "built-in prefixed head must be silent, got {rows:?}"
    );
}

/// Valid-type built-ins stay silent (`class D extends dynamic {}` and
/// `int Function()` — oracle: zero parser rows): `dynamic`/`Function` are
/// valid type references (type_info.dart:146-147) and never reach the
/// keyword branch.
#[test]
fn valid_type_builtins_are_silent() {
    let rows = rows_with("class D extends dynamic {}\n");
    assert!(rows.is_empty(), "`extends dynamic` is a valid type, got {rows:?}");
    let rows = rows_with("int Function() f = () => throw 0;\n");
    assert!(rows.is_empty(), "`int Function()` is silent, got {rows:?}");
}

