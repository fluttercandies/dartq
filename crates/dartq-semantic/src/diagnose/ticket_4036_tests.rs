//! #4036 — the typed literal's written `<...>` group is read by its own
//! BALANCE, not by the byte ahead of the literal's bracket.
//!
//! `literal_group_args` used to name the group's closing `>` as
//! `byte_of(bracket) - 1`: correct only when the group and the bracket abut.
//! Every whitespace spelling in between — a space, the merged `>` of a
//! nested group, the indent of a wrapped initializer — left that byte short
//! of the group, so the last segment ran past its closer and the whole parse
//! declined. The typed literal then kept the flat head-only face
//! (`List<dynamic>` displays, silent nested element rows) or an opaque row,
//! and every face downstream of the parse answered wrong: the literal's own
//! element checks, its `is` operand, its call-argument and return positions.
//!
//! The ambiguity fold these literals sit next to (`{...t}`'s spread-only
//! braces) is pinned here too: upstream `_inferSetOrMapLiteralType` folds
//! can-be/must-be slots, so a spread whose operand fills NO slot poisons both
//! can-be bits and the literal can resolve neither side.
//!
//! Oracle provenance: `dart analyze` 3.13.7 over the same shapes.

use super::FileReport;
use super::test_rig::report_ops;

/// The messages of one code's rows, in source order.
fn texts(report: &FileReport, code: &str) -> Vec<String> {
    let mut rows = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == code)
        .collect::<Vec<_>>();
    rows.sort_by_key(|d| d.range.offset);
    rows.iter().map(|d| d.message.to_string()).collect()
}

/// The written nested argument is the list literal's element face whatever
/// trivia separates the group from the bracket: the census's space, the
/// space inside a merged closer, and the indent of a wrapped initializer all
/// keep `List<int>` (a `List<dynamic>` display is the flat head-only
/// fallback the bad parse left behind).
#[test]
fn spaced_nested_list_element_face_keeps_the_written_argument() {
    let report = report_ops(
        "void main() {\n\
         \x20 final a = <List<int>> [1];\n\
         \x20 final b = <List<int> > [1, 'x'];\n\
         \x20 final c = <List<int>>\n\
         \x20     [2];\n\
         }\n",
    );
    assert_eq!(
        texts(&report, "list_element_type_not_assignable"),
        vec![
            "The element type 'int' can't be assigned to the list type \
             'List<int>'."
                .to_string(),
            "The element type 'int' can't be assigned to the list type \
             'List<int>'."
                .to_string(),
            "The element type 'String' can't be assigned to the list type \
             'List<int>'."
                .to_string(),
            "The element type 'int' can't be assigned to the list type \
             'List<int>'."
                .to_string(),
        ],
        "each spelling reads the group's own `List<int>` argument"
    );
}

/// A record-shaped argument rides the same capture (`<(int, int)>[...]`):
/// the literal's RESULT face is `List<(int, int)>`, so the return and
/// call-argument positions name the written record type — and the element
/// check's own `list_element_type_not_assignable` row fires against a
/// record element of the wrong shape (#4288 closed the record-vs-record
/// verdict the narrowed assertion used to defer).
#[test]
fn spaced_record_argument_is_the_element_face() {
    let report = report_ops(
        "Set<(int, int)> gives() => <(int, int)> [(1, 2)];\n\
         void takes(Set<(int, int)> x) {}\n\
         void main() {\n\
         \x20 takes(<(int, int)> [(1, 2)]);\n\
         \x20 final bad = <(int, int)> [(3, 'z')];\n\
         }\n",
    );
    assert_eq!(
        texts(&report, "list_element_type_not_assignable"),
        vec![
            "The element type '(int, String)' can't be assigned to the \
             list type '(int, int)'."
                .to_string()
        ],
        "the record element's own check asks the record-vs-record verdict"
    );
    assert_eq!(
        texts(&report, "return_of_invalid_type"),
        vec![
            "A value of type 'List<(int, int)>' can't be returned from the \
             function 'gives' because it has a return type of \
             'Set<(int, int)>'."
                .to_string()
        ],
    );
    assert_eq!(
        texts(&report, "argument_type_not_assignable"),
        vec![
            "The argument type 'List<(int, int)>' can't be assigned to the \
             parameter type 'Set<(int, int)>'. "
                .to_string()
        ],
    );
}

/// The set and map literals read their written arguments the same way: a
/// set's element and a map's value both keep the written nested argument
/// across the whitespace, and the narrower side refuses. (A map whose
/// VALUE type is itself generic — `<K, Map<A, B>>{k: {...}}` — has a
/// separate nested-value-descent gap in the map-entry check, outside this
/// ticket's capture scope.)
#[test]
fn spaced_set_and_map_element_faces_keep_the_written_arguments() {
    let report = report_ops(
        "void main() {\n\
         \x20 final s = <List<int>> {[1], ['a']};\n\
         \x20 final m = <String, List<int>> {'k': ['b']};\n\
         }\n",
    );
    assert_eq!(
        texts(&report, "list_element_type_not_assignable"),
        vec![
            "The element type 'String' can't be assigned to the list type \
             'int'."
                .to_string(),
            "The element type 'String' can't be assigned to the list type \
             'int'."
                .to_string(),
        ],
        "the set's element and the map's value both read their written \
         nested argument"
    );
}

/// The parsed face is what the literal's RESULT positions consume: the `is`
/// operand is statically decided, a call argument names the written face,
/// and a mismatched return names it too.
#[test]
fn literal_result_face_reaches_is_call_and_return_positions() {
    let report = report_ops(
        "void takes(Set<List<int>> s) {}\n\
         Set<List<int>> gives() => <List<int>> [[1]];\n\
         void main() {\n\
         \x20 print(<List<int>> [[1]] is List<List<int>>);\n\
         \x20 takes(<List<int>> [[1]]);\n\
         }\n",
    );
    assert_eq!(
        texts(&report, "unnecessary_type_check"),
        vec![
            "Unnecessary type check; the result is always 'true'.".to_string()
        ],
        "the `is` operand reads the literal's written result face"
    );
    assert_eq!(
        texts(&report, "argument_type_not_assignable"),
        vec![
            "The argument type 'List<List<int>>' can't be assigned to the \
             parameter type 'Set<List<int>>'. "
                .to_string()
        ],
    );
    assert_eq!(
        texts(&report, "return_of_invalid_type"),
        vec![
            "A value of type 'List<List<int>>' can't be returned from the \
             function 'gives' because it has a return type of \
             'Set<List<int>>'."
                .to_string()
        ],
    );
}

/// The spread-only brace fold (`_inferSetOrMapLiteralType`): an operand that
/// fills NO slot poisons both can-be bits, so a literal mixing it with one
/// forced side resolves neither — `_either` — while two forced sides stay
/// `_both`, and a lone decided side stays silent.
#[test]
fn spread_only_braces_fold_into_the_oracle_ambiguity() {
    let report = report_ops(
        "void f<T>(T t, Map<String, int> m, Iterable<int> it) {\n\
         \x20 final a = {...t};\n\
         \x20 final b = {...m, ...t};\n\
         \x20 final c = {...it, ...t};\n\
         \x20 final d = {8, ...m};\n\
         \x20 final e = {...m};\n\
         \x20 final g = {...it};\n\
         }\n",
    );
    assert_eq!(
        texts(&report, "ambiguous_set_or_map_literal_either"),
        vec![
            "This literal must be either a map or a set, but the elements \
             don't have enough information for type inference to work."
                .to_string(),
            "This literal must be either a map or a set, but the elements \
             don't have enough information for type inference to work."
                .to_string(),
            "This literal must be either a map or a set, but the elements \
             don't have enough information for type inference to work."
                .to_string(),
        ],
        "the unbounded `T` spread fills no slot: alone and beside a forced \
         map/set side the literal resolves neither"
    );
    assert_eq!(
        texts(&report, "ambiguous_set_or_map_literal_both").len(),
        1,
        "`{{8, ...m}}` forces both sides"
    );
}
