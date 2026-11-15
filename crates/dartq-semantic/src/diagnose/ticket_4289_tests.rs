//! #4289 pins: the nested collection literal's descent — an outer
//! map-value / list-element / set-element check that meets an inner
//! `[`/`{` literal hands it the donated context type, and the inner
//! literal's OWN element/entry verifier fires the inner rows through the
//! existing check entry points (no forked checker).
//!
//! `<List<int>, Map<String, int>>{[1]: {'k': 'c'}}` is the mandate shape:
//! the inner `{'k': 'c'}` adopts the donated `Map<String, int>` through
//! [`Engine::literal_ctx_ty`]'s enclosing-`{` walk (the top-level `:`
//! before it selects the value side), resolves map-side, and its own
//! `check_map_literal_entries` reports `map_value_type_not_assignable` at
//! `'c'`. A literal whose head family cannot adopt the donation
//! upward-infers instead and the OUTER check whole-compares the inferred
//! type at the literal's own span (`<Set<List<int>>> {[1], ['x']}` reads
//! `List<int>`/`List<String>` against the set element `Set<List<int>>`).
//!
//! The same descent covers every nesting family — list-in-map-value,
//! map-in-map-value, list-in-list-element, map-in-list-element, the set
//! variants, wrong at different depths, and both-wrong shapes — and the
//! typed `<E>{` set literal's own element face
//! (`_checkForSetElementTypeNotAssignable3`), which had no dartq emitter
//! at all before this ticket.
//!
//! Oracle provenance: `dart analyze` 3.13.7 over the same shapes (the
//! matrix below mirrors the probe file's rows verbatim).

#![allow(clippy::semicolon_if_nothing_returned)]
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

/// The mandate shape: a typed map literal whose value type is itself a
/// generic map — the inner literal adopts `Map<String, int>` and its own
/// entry check reports the failing value. The same descent fires through
/// a list value (`List<Map<String, int>>`), a map key side, and three
/// levels of nesting.
#[test]
fn nested_map_value_descends_into_the_inner_entry_check() {
    let report = report_ops(
        "void main() {\n\
         \x20 final a = <List<int>, Map<String, int>>{[1]: {'k': 'c'}};\n\
         \x20 final b = <String, Map<String, int>>{'k': {'k2': 'c'}};\n\
         \x20 final c = <List<int>, List<Map<String, int>>>{[1]: [{'k': 'c'}]};\n\
         \x20 final d = <List<int>, int>{[1]: 2};\n\
         \x20 final e = <int, Map<String, int>>{'k': {'k2': 'c'}};\n\
         }\n",
    );
    assert_eq!(
        texts(&report, "map_value_type_not_assignable"),
        vec![
            "The element type 'String' can't be assigned to the map value \
             type 'int'."
                .to_string(),
            "The element type 'String' can't be assigned to the map value \
             type 'int'."
                .to_string(),
            "The element type 'String' can't be assigned to the map value \
             type 'int'."
                .to_string(),
            "The element type 'String' can't be assigned to the map value \
             type 'int'."
                .to_string(),
        ],
        "each nested map literal's own entry check fires under the \
         donated value type — value side, list-wrapped, and key side"
    );
    assert_eq!(
        texts(&report, "map_key_type_not_assignable"),
        vec![
            "The element type 'String' can't be assigned to the map key \
             type 'int'."
                .to_string()
        ],
        "the `<int, _>` map's own key check still fires beside the descent"
    );
    assert!(
        texts(&report, "list_element_type_not_assignable").is_empty(),
        "the all-correct `<List<int>, int>` shape stays silent"
    );
}

/// List-in-map-value and map-in-list-element: the inner `[`/`{` literal
/// adopts the donated element/value type when the family matches, and a
/// non-adopting inner literal whole-compares at its own span.
#[test]
fn nested_list_and_map_elements_adopt_or_whole_compare() {
    let report = report_ops(
        "void main() {\n\
         \x20 final f = <String, List<int>>{'k': [1, 'x']};\n\
         \x20 final g = <List<int>> [[1], ['x']];\n\
         \x20 final h = <List<int>> [[1], [2]];\n\
         \x20 final i = <Map<String, int>> [{'k': 1}, {'k': 'c'}];\n\
         \x20 final j = <Map<String, int>> [{'k': 1}, {'k': 2}];\n\
         \x20 final k = <List<List<int>>> [[[1], ['x']]];\n\
         \x20 final l = <List<List<int>>> [[[1], [2]]];\n\
         \x20 final q = <List<int>, Map<String, List<int>>>{[1]: {'k': [1, 'x']}};\n\
         \x20 final r = <List<int>, Map<String, List<int>>>{[1]: {'k': [1, 2]}};\n\
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
            "The element type 'String' can't be assigned to the list type \
             'int'."
                .to_string(),
            "The element type 'String' can't be assigned to the list type \
             'int'."
                .to_string(),
        ],
        "the inner list literals' own element checks fire at every depth"
    );
    assert_eq!(
        texts(&report, "map_value_type_not_assignable"),
        vec![
            "The element type 'String' can't be assigned to the map value \
             type 'int'."
                .to_string()
        ],
        "the map-in-list-element shape reports through the inner entry check"
    );
}

/// The set family: a typed `<E>{` literal's own element face
/// (`set_element_type_not_assignable` — previously no emitter), an untyped
/// set literal under a `Set<E>`/`Iterable<E>` map value, and the
/// non-adopting inner literal's whole-compare at its own span.
#[test]
fn set_elements_check_typed_untyped_and_nested() {
    let report = report_ops(
        "void main() {\n\
         \x20 final n = <Set<List<int>>> {[1], ['x']};\n\
         \x20 final o = <String, Set<int>>{'k': {1, 'x'}};\n\
         \x20 final p = <String, Set<int>>{'k': {1, 2}};\n\
         \x20 final s = <int> {1, 'x'};\n\
         \x20 final u = <String, Set<int>>{'k': {'a'}};\n\
         }\n",
    );
    assert_eq!(
        texts(&report, "set_element_type_not_assignable"),
        vec![
            "The element type 'List<int>' can't be assigned to the set \
             type 'Set<List<int>>'."
                .to_string(),
            "The element type 'List<String>' can't be assigned to the set \
             type 'Set<List<int>>'."
                .to_string(),
            "The element type 'String' can't be assigned to the set type \
             'int'."
                .to_string(),
            "The element type 'String' can't be assigned to the set type \
             'int'."
                .to_string(),
            "The element type 'String' can't be assigned to the set type \
             'int'."
                .to_string(),
        ],
        "typed set elements, untyped set-under-map-value elements, and the \
         non-adopting inner literals all report"
    );
}

/// The set-element face's promotion-aware re-decision (#4293, retiring the
/// #4289 parse-time guard): the emitter records every potentially-nullable
/// leaf its parse-time verdict cannot decide, and
/// [`Engine::deferred_literal_element_checks`] re-asks the assignability
/// question against the leaf's flow-aware static type. A leaf promoted by a
/// surrounding condition reads its promoted non-null face and stays silent;
/// a genuinely un-promoted leaf reports the declared face (`'int?'`). A
/// bare `null` literal reads `Ty::Null` (never `nullable: true`) and keeps
/// its row, a non-nullable face (`'x'`) keeps its verdict, and a `?`-less
/// nullable element type (`<int?>`) admits the declared face silently.
///
/// Oracle provenance (`dart analyze` 3.13.7, the exact fixture rows): five
/// rows — `'Null'` (a), `'int?'` (b), `'int?'` (e), `'String'`+`'Null'` (f)
/// — with both promoted conditionals (c, g) and the `<int?>` literal (d)
/// silent.
#[test]
fn set_element_flow_aware_face_reports_unpromoted_and_keeps_null_and_nonnull() {
    let report = report_ops(
        "void takes(int? maybe, int? unpromoted) {\n\
         \x20 final a = <int>{null};\n\
         \x20 final b = <int>{maybe};\n\
         \x20 final c = maybe == null ? <int>{} : <int>{maybe};\n\
         \x20 final d = <int?>{maybe};\n\
         \x20 final e = <int>{unpromoted};\n\
         \x20 final f = <int>{1, 'x', null};\n\
         \x20 final g = maybe != null ? <int>{maybe} : <int>{};\n\
         }\n",
    );
    assert_eq!(
        texts(&report, "set_element_type_not_assignable"),
        vec![
            "The element type 'Null' can't be assigned to the set type \
             'int'."
                .to_string(),
            "The element type 'int?' can't be assigned to the set type \
             'int'."
                .to_string(),
            "The element type 'int?' can't be assigned to the set type \
             'int'."
                .to_string(),
            "The element type 'String' can't be assigned to the set type \
             'int'."
                .to_string(),
            "The element type 'Null' can't be assigned to the set type \
             'int'."
                .to_string(),
        ],
        "the promoted conditionals (c, g) stay silent, the un-promoted \
         leaves (b, e) report 'int?', and the null/non-nullable faces keep \
         their rows"
    );
}

/// A `[` literal never adopts a `Set` context (`_inferListTypeDownwards`
/// constrains `List<E>` only against `List`/`Iterable` supertypes): the
/// inner list upward-infers and the outer set-element check whole-compares
/// — the `<Set<List<int>>>` rows above — while a `Set` context DOES
/// donate to a `{` literal (`_fromContextType`'s isIterable arm).
#[test]
fn list_literal_never_adopts_a_set_context() {
    let report = report_ops(
        "void takesSet(Set<int> x) {}\n\
         void main() {\n\
         \x20 final a = <Set<int>> {[1], {2}};\n\
         \x20 takesSet(<int>{'a'});\n\
         }\n",
    );
    assert_eq!(
        texts(&report, "set_element_type_not_assignable"),
        vec![
            "The element type 'List<int>' can't be assigned to the set \
             type 'Set<int>'."
                .to_string(),
            "The element type 'String' can't be assigned to the set type \
             'int'."
                .to_string(),
        ],
        "`[1]` whole-compares `List<int>` against the `Set<int>` element \
         while the set literal adopts silently; the typed set argument \
         reports its own element"
    );
}
