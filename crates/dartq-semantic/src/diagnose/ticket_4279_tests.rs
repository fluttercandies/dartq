//! #4279 pins: a record type as a written type argument (`List<(int,
//! int)>`).
//!
//! The use-site written-type capture poisoned a record type inside an
//! annotation's argument group (`bounds.poison()`'s site arm), so
//! `List<(int, int)>` lost its written arguments entirely and every
//! element face degraded: a for-in head over the list bound `Object?`
//! slots, an awaited `Future<(int, Map<String, Object?>)>` lost its
//! record shape, and a `Map<(int, int), String>` key read `int`. The
//! capture now folds the record into one record tree converted to
//! [`Ty::Record`] (#4279), so the element face IS the record and the
//! pattern dispatch arms read the real slot types — the oracle's own
//! answers (dart analyze 3.13.7 receipts on the probe fixtures
//! c2/c2p/r).
//!
//! The dispatch-arm semantics themselves are #4265's adjudication and
//! stay as committed: a genuinely non-record matched value still hands its
//! pattern fields `Object?` (`record_over_int_keeps_object_question_leaves`
//! pins that shape here too, the r.dart receipt).

use super::FileReport;
use super::test_rig::report_ops;

/// The `pattern_type_mismatch_in_irrefutable_context` rows' messages, in
/// report order.
fn pattern_rows(report: &FileReport) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "pattern_type_mismatch_in_irrefutable_context")
        .map(|d| d.message.to_string())
        .collect()
}

/// A generic list's record element matches the record pattern: no row
/// (oracle c2.dart: the element IS `(int, int)`).
#[test]
fn list_record_element_matches_record_pattern() {
    let report = report_ops(
        "void f(List<(int, int)> xs) {\n\
           for (var (int a, int b) in xs) {\n\
             a + b;\n\
           }\n\
         }\n",
    );
    assert_eq!(pattern_rows(&report), Vec::<String>::new());
}

/// A leaf mismatch reports the record's REAL slot type: the matched value
/// at `String a` is `int`, never `Object?` (oracle c2p.dart: one row,
/// 'int' → 'String'). The degraded face used to report three rows
/// (`int` vs `(Object?, Object?)` plus two `Object?` leaves).
#[test]
fn list_record_element_leaf_mismatch_names_real_slot_type() {
    let report = report_ops(
        "void f(List<(int, int)> xs) {\n\
           for (var (String a, int b) in xs) {\n\
             a.length;\n\
             b;\n\
           }\n\
         }\n",
    );
    let rows = pattern_rows(&report);
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert!(
        rows[0].contains("'int' isn't assignable to the required type 'String'"),
        "{rows:?}"
    );
}

/// The #4265 pin: a pattern over a genuinely non-record matched value
/// keeps the oracle's own `Object?` slots — both the record-shape row and
/// the leaf row fire (oracle r.dart: two rows).
#[test]
fn record_over_int_keeps_object_question_leaves() {
    let report = report_ops(
        "void f(int x) {\n\
           var (int a,) = x;\n\
           a;\n\
         }\n",
    );
    let rows = pattern_rows(&report);
    assert_eq!(rows.len(), 2, "{rows:?}");
    assert!(rows.iter().any(|row| row.contains("'(Object?,)'")), "{rows:?}");
    assert!(
        rows.iter()
            .any(|row| row.contains("'Object?' isn't assignable")),
        "{rows:?}"
    );
}

/// A record with named fields as a generic argument keeps its field
/// names and types through the site capture: the `a` slot reads `int`
/// (oracle: `List<({int a, String b})>` matched against `(a: String x,
/// b: int y)` reports both named fields at their real slot types).
#[test]
fn record_argument_with_named_fields() {
    let report = report_ops(
        "void f(List<({int a, String b})> xs) {\n\
           for (var (a: String x, b: int y) in xs) {\n\
             x.length;\n\
             y;\n\
           }\n\
         }\n",
    );
    let rows = pattern_rows(&report);
    assert_eq!(rows.len(), 2, "{rows:?}");
    assert!(
        rows.iter()
            .any(|row| row.contains("'int' isn't assignable to the required type 'String'")),
        "{rows:?}"
    );
    assert!(
        rows.iter()
            .any(|row| row.contains("'String' isn't assignable to the required type 'int'")),
        "{rows:?}"
    );
}

/// A nested record argument (`((int, int), String)`) recurses: the outer
/// shape matches and the inner record's fields dispatch their real types.
#[test]
fn nested_record_argument_matches() {
    let report = report_ops(
        "void f(List<((int, int), String)> xs) {\n\
           for (var ((int a, int b), String c) in xs) {\n\
             a + b;\n\
             c.length;\n\
           }\n\
         }\n",
    );
    assert_eq!(pattern_rows(&report), Vec::<String>::new());

    let mismatched = report_ops(
        "void f(List<((int, int), String)> xs) {\n\
           for (var ((String a, int b), String c) in xs) {\n\
             a.length;\n\
             b;\n\
             c.length;\n\
           }\n\
         }\n",
    );
    let rows = pattern_rows(&mismatched);
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert!(
        rows[0].contains("'int' isn't assignable to the required type 'String'"),
        "{rows:?}"
    );
}

/// The single-positional record `(int,)` keeps its one-field shape.
#[test]
fn single_positional_record_argument() {
    let report = report_ops(
        "void f(List<(int,)> xs) {\n\
           for (var (String a,) in xs) {\n\
             a.length;\n\
           }\n\
         }\n",
    );
    let rows = pattern_rows(&report);
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert!(
        rows[0].contains("'int' isn't assignable to the required type 'String'"),
        "{rows:?}"
    );
}

/// A record used as a map's KEY type reaches the key iterable's element
/// face: `m.keys` yields `(int, int)`, so a `(String, String)` consumer
/// reports the real record (oracle: one row, '(int, int)' → '(String,
/// String)').
#[test]
fn map_record_key_element_face() {
    let report = report_ops(
        "void take((String, String) r) {\n\
           r;\n\
         }\n\
         void f(Map<(int, int), String> m) {\n\
           for (final k in m.keys) {\n\
             take(k);\n\
           }\n\
         }\n",
    );
    let rows: Vec<String> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "argument_type_not_assignable")
        .map(|d| d.message.to_string())
        .collect();
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert!(rows[0].contains("'(int, int)'"), "{rows:?}");
    assert!(rows[0].contains("'(String, String)'"), "{rows:?}");
}

/// The corpus's `runRecord` shape: a record as `Future`'s type argument
/// keeps its shape through `await`, so the record pattern's binders get
/// the real field types (the cockpit_record_commands_test FP family:
/// `final (startExit, started) = await runRecord(...)` and `started[...]`).
#[test]
fn future_record_argument_survives_await() {
    let report = report_ops(
        "Future<(int, Map<String, Object?>)> runRecord(String a) async =>\n\
             (0, <String, Object?>{});\n\
         void g() async {\n\
           final (code, body) = await runRecord('x');\n\
           code + 1;\n\
           body['lines'];\n\
         }\n",
    );
    assert_eq!(pattern_rows(&report), Vec::<String>::new());
    assert!(
        !report
            .diagnostics
            .iter()
            .any(|d| d.code.name == "undefined_operator"),
        "{:?}",
        report
            .diagnostics
            .iter()
            .map(|d| (d.code.name, d.message.to_string()))
            .collect::<Vec<_>>()
    );
}

/// A typed collection literal's written record argument
/// (`<(int, int)>[]`) spells the record on the literal's element face
/// (the `_samplePoints` FP: the literal used to read `List<dynamic>` and
/// the return reported `return_of_invalid_type` the oracle never emits).
#[test]
fn typed_literal_record_element_face() {
    let report = report_ops(
        "List<(int, int)> f() {\n\
           final samples = <(int, int)>[];\n\
           samples.add((1, 2));\n\
           return samples;\n\
         }\n",
    );
    let codes: Vec<&str> = report
        .diagnostics
        .iter()
        .map(|d| d.code.name)
        .collect();
    assert!(
        !codes.contains(&"return_of_invalid_type")
            && !codes.contains(&"invalid_assignment")
            && !codes.contains(&"argument_type_not_assignable"),
        "{:?}",
        report
            .diagnostics
            .iter()
            .map(|d| (d.code.name, d.message.to_string()))
            .collect::<Vec<_>>()
    );
}

/// A typed map literal's record VALUE face answers named-field getters
/// (the `matches[identity].score` FP: the value used to degrade and
/// `.score` reported `undefined_getter` on a non-record face).
#[test]
fn typed_map_literal_record_value_answers_getters() {
    let report = report_ops(
        "class Element {}\n\
         void f(Object key) {\n\
           final matches = <Object, ({Element element, int score})>{};\n\
           final current = matches[key];\n\
           if (current == null || 1 > current.score) return;\n\
           print(current.element);\n\
         }\n",
    );
    assert!(
        !report
            .diagnostics
            .iter()
            .any(|d| d.code.name == "undefined_getter"),
        "{:?}",
        report
            .diagnostics
            .iter()
            .map(|d| (d.code.name, d.message.to_string()))
            .collect::<Vec<_>>()
    );
}

/// A record field spelled by a name nothing resolves keeps the literal
/// silent (the `Map<({Element element, String? routeName}), …>` field
/// initializer FP: the literal declines rather than reporting an
/// assignment against a mis-spelled face — the oracle reports only its
/// own undefined-class row there).
#[test]
fn typed_literal_record_with_unresolved_field_head_stays_silent() {
    let report = report_ops(
        "final m = <({Element element, String? routeName}), int>{};\n",
    );
    assert!(
        !report
            .diagnostics
            .iter()
            .any(|d| d.code.name == "invalid_assignment"
                || d.code.name == "argument_type_not_assignable"),
        "{:?}",
        report
            .diagnostics
            .iter()
            .map(|d| (d.code.name, d.message.to_string()))
            .collect::<Vec<_>>()
    );
}

/// A spread of a call whose FIRST ARGUMENT is nullable but whose declared
/// return is not: the operand ladder's group leg used to read the
/// argument-list parens as an expression group and type the send by the
/// first argument's read row (`_pick(nullableMap, keys)` answering
/// `Map?`), fabricating `unchecked_use_of_nullable_value` the oracle
/// never emits (the cli_output spread-churn rows of the +33 family —
/// `..._pick(schemaMap, const <String>['type'])` inside the null-guarded
/// element). The callee-keyed leg (`call_ends`' completed-send swap)
/// owns the send's answer; the argument list is not an expression.
#[test]
fn spread_of_call_with_nullable_first_argument_keeps_return_face() {
    let report = report_ops(
        "Map<String, Object?> pick(\n\
           Map<Object?, Object?> source,\n\
           Iterable<String> keys,\n\
         ) => <String, Object?>{\n\
           for (final key in keys)\n\
             if (source[key] != null) key: source[key],\n\
         };\n\
         Map<String, Object?> f(\n\
           Map<Object?, Object?> value, {\n\
           required bool more,\n\
         }) {\n\
           final schema = value['schema'];\n\
           final schemaMap = schema is Map<Object?, Object?> ? schema : null;\n\
           return <String, Object?>{\n\
             if (more) ...pick(value, const <String>['schemaRef', 'precision']),\n\
             if (more && schemaMap != null) ...pick(schemaMap, const <String>['type']),\n\
           };\n\
         }\n\
         void main() {}\n",
    );
    assert!(
        !report
            .diagnostics
            .iter()
            .any(|d| d.code.name == "unchecked_use_of_nullable_value"),
        "{:?}",
        report
            .diagnostics
            .iter()
            .map(|d| (d.code.name, d.message.to_string()))
            .collect::<Vec<_>>()
    );
}
