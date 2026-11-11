//! The #3179 pins: the `on`-head shapes that resolve to a null/record
//! `exceptionType` upstream and therefore start the catch-all face of the
//! `_CatchClausesVerifier` (upstream Dart SDK source,
//! pkg/analyzer/lib/src/error/dead_code_verifier.dart:552-608 —
//! `nextCatchClause`'s `currentType == null || ....isDartCoreObject` arm
//! reports every FOLLOWING clause from `catchClauses[index + 1]` through
//! `catchClauses.last`).
//!
//! The clause fact is built in `finish_catch_clause`
//! (`diagnose/scopes_bodies.rs`): a record-typed `on` head is a
//! `RecordType` upstream — the subtype arm, which a following non-record
//! clause never satisfies (silent) — while the paren-headed recovery with
//! no type joins the try's clause list with `on_type: None`, and a clause
//! whose body block never materialized ends at its head's recovery
//! boundary. The faces pinned here are the ones the corpus exercises —
//! `on (int, int)` (`on_clauses_A01_t02`), `on (int i, {String? n})`
//! (`on_clauses_A02_t01`), and the bare `on (42, …)` recovery
//! (`parser-fe/record/on`) — plus the plain `catch (e)` control and the
//! `on Object?` face.

use crate::diagnose::test_rig::{CORE_PRELUDE, code_names, report_ops};

/// The diagnostic rows' `(code, offset)` pairs, offsets relative to the
/// fixture (the prelude is stripped).
fn rows_of(source: &str) -> Vec<(&'static str, usize)> {
    let report = report_ops(source);
    report
        .diagnostics
        .iter()
        .map(|d| (d.code.name, d.range.offset as usize - CORE_PRELUDE.len()))
        .collect()
}

/// A valid record-type head is NOT the catch-all face: upstream's
/// `exceptionType` is the `RecordType` itself (never null, never
/// `isDartCoreObject`), so the following `on int` takes the subtype arm —
/// `int` is no subtype of `(int, String)` — and the oracle (3.13.7) is
/// silent.
#[test]
fn ticket_3179_record_head_makes_the_tail_dead() {
    let fixture = "void f() {\n  try {\n  } on (int, String) {\n  } on int {\n  }\n}";
    assert_eq!(
        rows_of(fixture),
        Vec::<(&str, usize)>::new(),
        "{:#?}",
        rows_of(fixture)
    );
}

/// The named-record head (`on (int i, {String? n})`, the co19 shape) is
/// the same subtype arm: `String` is no subtype of the record, so the
/// oracle (3.13.7) is silent.
#[test]
fn ticket_3179_named_record_head_makes_the_tail_dead() {
    let fixture = "void f() {\n  try {\n  } on (int i, {String? n}) {\n  } on String {\n  }\n}";
    assert_eq!(
        rows_of(fixture),
        Vec::<(&str, usize)>::new(),
        "{:#?}",
        rows_of(fixture)
    );
}

/// The bare `on (…)` recovery with no type at all (`parser-fe/record/on`,
/// `on_clauses_A02_t01`): still one clause on the list, still catch-all.
#[test]
fn ticket_3179_paren_garbage_head_makes_the_tail_dead() {
    let fixture = "void f() {\n  try {\n  } on int {\n  }\n  on(42, n: \"x\");\n}";
    assert_eq!(
        rows_of(fixture),
        vec![(
            "dead_code_catch_following_catch",
            fixture.find("on(42").unwrap()
        )],
        "{:#?}",
        rows_of(fixture)
    );
}

/// The control: a plain `catch (e)` clause is catch-all on the name path
/// (no record head involved).
#[test]
fn ticket_3179_plain_catch_is_catch_all() {
    let fixture = "void f() {\n  try {\n  } catch (e) {\n  } on int {\n  }\n}";
    assert_eq!(
        rows_of(fixture),
        vec![(
            "dead_code_catch_following_catch",
            fixture.find("on int").unwrap()
        )],
        "{:#?}",
        rows_of(fixture)
    );
}

/// `on Object?` reads as the catch-all family for the dead-code face
/// AND fires its own nullable-head row: probe 3.13.7 over the same
/// fixture reports `NULLABLE_TYPE_IN_CATCH_CLAUSE` at the `Object?`
/// head ("A potentially nullable type can't be used in an 'on' clause
/// because it isn't valid to throw a nullable expression.") then
/// `DEAD_CODE_CATCH_FOLLOWING_CATCH` at the tail ("Dead code: Catch
/// clauses after a 'catch (e)' or an 'on Object catch (e)' are never
/// reached." — the nullable-Object head joins the catch-all family).
#[test]
fn ticket_3179_nullable_object_head_takes_the_catch_all_arm() {
    let fixture = "void f() {\n  try {\n  } on Object? {\n  } on int {\n  }\n}";
    let rows = rows_of(fixture);
    assert_eq!(
        rows,
        vec![
            (
                "nullable_type_in_catch_clause",
                fixture.find("Object?").unwrap()
            ),
            (
                "dead_code_catch_following_catch",
                fixture.find("on int").unwrap()
            ),
        ],
        "{:#?}",
        rows
    );
    let report = report_ops(fixture);
    assert!(
        code_names(&report).iter().all(|code| {
            *code == "nullable_type_in_catch_clause" || *code == "dead_code_catch_following_catch"
        }),
        "{:#?}",
        rows
    );
}
