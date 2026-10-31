//! The #3238 pins: the catch-clause verifier's two faces
//! (`_CatchClausesVerifier.nextCatchClause`, upstream Dart SDK source,
//! pkg/analyzer/lib/src/error/dead_code_verifier.dart:543-577) decided on
//! the RESOLVED on-types.
//!
//! - `dead_code_on_catch_subtype` asks `TypeSystem.isSubtypeOf(current,
//!   earlier)` over the resolved types WITH their type-argument lists:
//!   `catch_on_ty` now carries the written arguments (re-derived from the
//!   token window), so same-head pairs decide covariantly through
//!   `resolved_subtype` instead of colliding as one bare head.
//! - `dead_code_catch_following_catch` fires only after a clause whose
//!   `exceptionType?.type` is null (a bare `catch`) or `isDartCoreObject` —
//!   element identity, any written nullability
//!   (pkg/analyzer/lib/src/dart/element/type.dart:767-769). A record/paren
//!   `on` head still resolves (`computeType` accepts the record type,
//!   parser_impl.dart:10723) and therefore takes the subtype arm, where a
//!   record shape proves nothing against a named head; an `on` head no type
//!   parse accepts resolves to `InvalidType`, which the 0c rule proves from
//!   either side (the oracle's `'InvalidType' is a subtype of 'String'`
//!   row, co19 `Records/on_clauses_A02_t05`).
//!
//! Direction of the subtype pins: the oracle reports the LATER clause when
//! its type is below an EARLIER one (`dead_code_on_catch_subtype/doc_0`
//! reads `'int' is a subtype of 'num'` for `on num {} on int {}`), so
//! `on A<num> {} on A<int> {}` fires and `on A<int> {} on A<num> {}` is
//! silent.

use crate::diagnose::test_rig::{CORE_PRELUDE, report_ops};

/// The dead-catch rows of the fixture as `(code, offset, message)`,
/// offsets relative to the fixture (the prelude is stripped); the two
/// dead-catch codes only — a recovery fixture's parser rows ride along
/// and are not under test here.
fn dead_rows(source: &str) -> Vec<(&'static str, usize, String)> {
    report_ops(source)
        .diagnostics
        .iter()
        .filter(|d| {
            matches!(
                d.code.name,
                "dead_code_on_catch_subtype" | "dead_code_catch_following_catch"
            )
        })
        .map(|d| {
            (
                d.code.name,
                d.range.offset as usize - CORE_PRELUDE.len(),
                d.message.to_string(),
            )
        })
        .collect()
}

/// The doc direction over a generic head: `A<int> <: A<num>` through the
/// covariant argument leg (`int extends num`), so the later `on A<int>` is
/// dead and the message spells both written argument lists.
#[test]
fn ticket_3238_same_head_covariant_args_fire() {
    let fixture = "class A<T> {}\nvoid f() {\n  try {\n  } on A<num> {\n  } on A<int> {\n  }\n}";
    assert_eq!(
        dead_rows(fixture),
        vec![(
            "dead_code_on_catch_subtype",
            fixture.find("on A<int>").unwrap(),
            "Dead code: This on-catch block won't be executed because 'A<int>' is a subtype of 'A<num>' and hence will have been caught already.".to_string(),
        )],
        "{:#?}",
        dead_rows(fixture)
    );
}

/// The reverse order is silent: `A<num>` is not below `A<int>` (`num` is
/// `int`'s supertype), the covariant leg refuses.
#[test]
fn ticket_3238_same_head_reversed_args_stay_silent() {
    let fixture = "class A<T> {}\nvoid f() {\n  try {\n  } on A<int> {\n  } on A<num> {\n  }\n}";
    assert_eq!(dead_rows(fixture), Vec::new(), "{:#?}", dead_rows(fixture));
}

/// Unrelated arguments on one head never collide — the pre-fix face read
/// every `on A<…>` as the bare `A` and fired here.
#[test]
fn ticket_3238_same_head_unrelated_args_stay_silent() {
    let fixture =
        "class A<T> {}\nvoid f() {\n  try {\n  } on A<int> {\n  } on A<String> {\n  }\n}";
    assert_eq!(dead_rows(fixture), Vec::new(), "{:#?}", dead_rows(fixture));
}

/// Identical written arguments are the reflexive subtype (the positive
/// control for the argument capture), and the message renders the written
/// list — never the instantiate-to-bounds `List<dynamic>` a raw head would
/// display.
#[test]
fn ticket_3238_identical_args_fire_and_render_written_arguments() {
    let fixture =
        "void f() {\n  try {\n  } on List<String> {\n  } on List<String> {\n  }\n}";
    let rows = dead_rows(fixture);
    assert_eq!(
        rows,
        vec![(
            "dead_code_on_catch_subtype",
            fixture.rfind("on List<String>").unwrap(),
            "Dead code: This on-catch block won't be executed because 'List<String>' is a subtype of 'List<String>' and hence will have been caught already.".to_string(),
        )],
        "{:#?}",
        rows
    );
}

/// A bare `catch` is the catch-all face: the following typed clause is
/// dead, anchored at its own `on` and spanning through the last clause.
#[test]
fn ticket_3238_bare_catch_makes_the_tail_dead() {
    let fixture = "class Foo {}\nvoid f() {\n  try {\n  } catch (e) {\n  } on Foo {\n  }\n}";
    let rows = dead_rows(fixture);
    assert_eq!(
        rows,
        vec![(
            "dead_code_catch_following_catch",
            fixture.find("on Foo").unwrap(),
            "Dead code: Catch clauses after a 'catch (e)' or an 'on Object catch (e)' are never reached.".to_string(),
        )],
        "{:#?}",
        rows
    );
    let report = report_ops(fixture);
    let row = report
        .diagnostics
        .iter()
        .find(|d| d.code.name == "dead_code_catch_following_catch")
        .unwrap();
    // The span law (#3213): first dead clause through the last clause's
    // closing brace.
    let start = fixture.find("on Foo").unwrap();
    let end = fixture.rfind("}\n}").unwrap() + 1;
    assert_eq!(row.range.length as usize, end - start, "{row:#?}");
}

/// `on Object` is `isDartCoreObject`: the same catch-all face.
#[test]
fn ticket_3238_object_head_makes_the_tail_dead() {
    let fixture = "class Foo {}\nvoid f() {\n  try {\n  } on Object {\n  } on Foo {\n  }\n}";
    assert_eq!(
        dead_rows(fixture),
        vec![(
            "dead_code_catch_following_catch",
            fixture.find("on Foo").unwrap(),
            "Dead code: Catch clauses after a 'catch (e)' or an 'on Object catch (e)' are never reached.".to_string(),
        )],
        "{:#?}",
        dead_rows(fixture)
    );
}

/// `isDartCoreObject` reads element identity only (type.dart:767-769 —
/// `element.name == "Object" && element.library.isDartCore`, no nullability
/// test), so `on Object?` is catch-all too: the tail reports
/// `dead_code_catch_following_catch`, never the subtype arm's
/// `'Foo' is a subtype of 'Object?'` row the pre-fix trigger produced.
#[test]
fn ticket_3238_nullable_object_head_is_catch_all() {
    let fixture = "class Foo {}\nvoid f() {\n  try {\n  } on Object? {\n  } on Foo {\n  }\n}";
    assert_eq!(
        dead_rows(fixture),
        vec![(
            "dead_code_catch_following_catch",
            fixture.find("on Foo").unwrap(),
            "Dead code: Catch clauses after a 'catch (e)' or an 'on Object catch (e)' are never reached.".to_string(),
        )],
        "{:#?}",
        dead_rows(fixture)
    );
}

/// A record-type `on` head resolves to a `RecordType` — never null, never
/// `isDartCoreObject` — so it takes the subtype arm, where a record shape
/// proves nothing against the following named clause: silent, exactly the
/// corpus's `on_clauses_A01_t01`/`A01_t02`/`A03_*` rows (all `-`). The
/// pre-fix fold read the head as catch-all and fired the following-catch
/// row — the measured EXTRA face.
#[test]
fn ticket_3238_record_head_takes_the_subtype_arm_silently() {
    let fixture =
        "class Foo {}\nvoid f() {\n  try {\n  } on (int, String) {\n  } on Foo {\n  }\n}";
    assert_eq!(dead_rows(fixture), Vec::new(), "{:#?}", dead_rows(fixture));
}

/// A named clause followed by a record head: the record is not below the
/// named type either (`not_call_on_after_try_block`'s `on (a, b)` clauses
/// join only AFTER a catch-all fired, and `on_clauses_A02_t01`'s
/// `on String {} on (int i, {String? n}) {}` carries no dead-code row).
#[test]
fn ticket_3238_record_head_after_named_clause_stays_silent() {
    let fixture = "void f() {\n  try {\n  } on String {\n  } on (int, String) {\n  }\n}";
    assert_eq!(dead_rows(fixture), Vec::new(), "{:#?}", dead_rows(fixture));
}

/// A record head after a bare `catch` is simply a following clause: the
/// catch-all row spans it (parser-fe `not_call_on_after_try_block`:
/// `catch (e) {} on() {} on(a, b) {} …` reports one row from the first
/// `on()` through the last clause).
#[test]
fn ticket_3238_record_head_after_catch_all_joins_the_dead_span() {
    let fixture = "void f() {\n  try {\n  } catch (e) {\n  } on (int, String) {\n  }\n}";
    let rows = dead_rows(fixture);
    assert_eq!(
        rows,
        vec![(
            "dead_code_catch_following_catch",
            fixture.find("on (int").unwrap(),
            "Dead code: Catch clauses after a 'catch (e)' or an 'on Object catch (e)' are never reached.".to_string(),
        )],
        "{:#?}",
        rows
    );
}

/// An `on` head no type parse accepts (`on<T>(…)`, co19
/// `Records/on_clauses_A02_t05`) resolves to `InvalidType`: the 0c rule
/// proves it below the earlier `on String`, and the message spells the
/// resolved `'InvalidType'` — the oracle's exact row (`35:5`,
/// `'InvalidType' is a subtype of 'String'`). The anchor is the dead
/// clause's `on`; the parser's own recovery rows ride along untested.
#[test]
fn ticket_3238_unparsable_head_is_the_invalid_face() {
    let fixture = "void f() {\n  try {\n  } on String {\n  } on<T>(int i, {String? n}) {\n  }\n}";
    let rows = dead_rows(fixture);
    assert_eq!(
        rows,
        vec![(
            "dead_code_on_catch_subtype",
            fixture.find("on<T>").unwrap(),
            "Dead code: This on-catch block won't be executed because 'InvalidType' is a subtype of 'String' and hence will have been caught already.".to_string(),
        )],
        "{:#?}",
        rows
    );
}

/// One row per try (`_done`): after the subtype arm fired, a third clause
/// that would also be dead stays silent — and the row's span runs from the
/// first dead clause through the LAST clause.
#[test]
fn ticket_3238_one_row_per_try_spans_through_the_last_clause() {
    let fixture = "class A<T> {}\nclass Foo {}\nvoid f() {\n  try {\n  } on A<num> {\n  } on A<int> {\n  } on Foo {\n  }\n}";
    let report = report_ops(fixture);
    let rows: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name.starts_with("dead_code_"))
        .collect();
    assert_eq!(rows.len(), 1, "{rows:#?}");
    let row = rows[0];
    assert_eq!(row.code.name, "dead_code_on_catch_subtype");
    let start = fixture.find("on A<int>").unwrap();
    let end = fixture.rfind("}\n}").unwrap() + 1;
    assert_eq!(row.range.offset as usize - CORE_PRELUDE.len(), start);
    assert_eq!(row.range.length as usize, end - start, "{row:#?}");
}
