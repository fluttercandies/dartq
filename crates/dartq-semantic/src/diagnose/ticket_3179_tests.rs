//! The #3179 pins: the `on`-head shapes that resolve to a null/record
//! `exceptionType` upstream and therefore start the catch-all face of the
//! `_CatchClausesVerifier` (upstream Dart SDK source,
//! pkg/analyzer/lib/src/error/dead_code_verifier.dart:552-608 —
//! `nextCatchClause`'s `currentType == null || ....isDartCoreObject` arm
//! reports every FOLLOWING clause from `catchClauses[index + 1]` through
//! `catchClauses.last`).
//!
//! The clause fact is built in `finish_catch_clause`
//! (`diagnose/scopes_bodies.rs`): a record-typed (or paren-headed,
//! recovered) `on` head joins the try's clause list with `on_type: None`
//! even though the head's field types fire their own `TypeReference`
//! events, and a clause whose body block never materialized ends at its
//! head's recovery boundary. The two faces pinned here are the ones the
//! corpus exercises — `on (int, int)` (`on_clauses_A01_t02`),
//! `on (int i, {String? n})` (`on_clauses_A02_t01`), and the bare
//! `on (42, …)` recovery (`parser-fe/record/on`) — plus the plain
//! `catch (e)` control and the `on Object?` face (a nullable Object is
//! NOT `isDartCoreObject`, so it takes the subtype arm instead).

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

/// A valid record-type head is the catch-all face: the following `on int`
/// clause is dead, anchored at its own `on` (not at the record head).
#[test]
fn ticket_3179_record_head_makes_the_tail_dead() {
    let fixture = "void f() {\n  try {\n  } on (int, String) {\n  } on int {\n  }\n}";
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

/// The named-record recovery the co19 cases probe (`on (int i, {String? n})`)
/// is the same catch-all face.
#[test]
fn ticket_3179_named_record_head_makes_the_tail_dead() {
    let fixture = "void f() {\n  try {\n  } on (int i, {String? n}) {\n  } on String {\n  }\n}";
    assert_eq!(
        rows_of(fixture),
        vec![(
            "dead_code_catch_following_catch",
            fixture.find("on String").unwrap()
        )],
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

/// `on Object?` is NOT the catch-all face (`isDartCoreObject` needs the none
/// nullability suffix): the tail reports the subtype arm instead, anchored at
/// the dead clause, with `int <: Object?` as the right-top rule.
#[test]
fn ticket_3179_nullable_object_head_takes_the_subtype_arm() {
    let fixture = "void f() {\n  try {\n  } on Object? {\n  } on int {\n  }\n}";
    let rows = rows_of(fixture);
    assert_eq!(
        rows,
        vec![(
            "dead_code_on_catch_subtype",
            fixture.find("on int").unwrap()
        )],
        "{:#?}",
        rows
    );
    let report = report_ops(fixture);
    assert!(
        code_names(&report)
            .iter()
            .all(|code| *code == "dead_code_on_catch_subtype"),
        "{:#?}",
        rows
    );
}
