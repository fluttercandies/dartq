//! #4293 pins: the deferred promotion-aware literal-element re-decision.
//! The set/list/map element emitters record every potentially-nullable leaf
//! their parse-time verdict cannot decide; the deferred pass re-asks each
//! assignability question against the leaf's flow-aware static type once
//! the `PromotedRead`/`ReadType` rows stand. The three-state matrix —
//! promoted / un-promoted / null literal — crossed with the set, list, and
//! map-key/value element positions:
//!
//! * promoted to the expected face → silent;
//! * un-promoted → the oracle's row on the declared face (`'int?'`);
//! * promotion to a non-assignable subtype → the row on the PROMOTED face
//!   (`if (x is String)` over `Object?` reports `'String'`);
//! * null literal → its own row (`'Null'`);
//! * `?`-marked elements/entries and compound elements keep their
//!   parse-time verdicts (the null-aware face is a different question).
//!
//! Oracle provenance: `dart analyze` 3.13.7 over the exact fixtures (the
//! expected rows below mirror the probe verbatim).

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

/// The promoted states: `is`-promotion, `!= null` promotion, both
/// conditional shapes, and the nullable element type that admits the
/// declared face. Every family stays silent — the false rows the
/// un-promoted declared face would fabricate (the #4289 guard's old
/// under-report direction, now answered exactly).
#[test]
fn promoted_leaves_stay_silent() {
    let report = report_ops(
        "void promoted(int? x) {\n\
         \x20 if (x is int) {\n\
         \x20   final a = <int>{x};\n\
         \x20   final b = <int>[x];\n\
         \x20   final c = <int, int>{x: x};\n\
         \x20 }\n\
         \x20 if (x != null) {\n\
         \x20   final d = <int>{x};\n\
         \x20   final e = <int>[x];\n\
         \x20   final f = <int, int>{x: x};\n\
         \x20 }\n\
         \x20 final g = x != null ? <int>{x} : <int>{};\n\
         \x20 final h = x == null ? <int>[] : <int>[x];\n\
         \x20 final i = <int?>{x};\n\
         \x20 final j = <int?>[x];\n\
         }\n",
    );
    for code in [
        "set_element_type_not_assignable",
        "list_element_type_not_assignable",
        "map_key_type_not_assignable",
        "map_value_type_not_assignable",
    ] {
        assert!(
            texts(&report, code).is_empty(),
            "every promoted leaf reads its promoted face and stays silent \
             ({code})"
        );
    }
}

/// The genuinely un-promoted states: the deferred pass reports the
/// declared nullable face at every element position — set, list, map key,
/// map value — matching the oracle's rows the #4289 guard declined.
#[test]
fn unpromoted_leaves_report_the_declared_face() {
    let report = report_ops(
        "void unpromoted(int? x) {\n\
         \x20 final a = <int>{x};\n\
         \x20 final b = <int>[x];\n\
         \x20 final c = <int, int>{x: x};\n\
         \x20 final d = <int, String>{x: 'a'};\n\
         \x20 final e = <String, int>{'a': x};\n\
         }\n",
    );
    assert_eq!(
        texts(&report, "set_element_type_not_assignable"),
        vec![
            "The element type 'int?' can't be assigned to the set type \
             'int'."
                .to_string()
        ],
        "the un-promoted set element reports"
    );
    assert_eq!(
        texts(&report, "list_element_type_not_assignable"),
        vec![
            "The element type 'int?' can't be assigned to the list type \
             'int'."
                .to_string()
        ],
        "the un-promoted list element reports"
    );
    assert_eq!(
        texts(&report, "map_key_type_not_assignable"),
        vec![
            "The element type 'int?' can't be assigned to the map key type \
             'int'."
                .to_string(),
            "The element type 'int?' can't be assigned to the map key type \
             'int'."
                .to_string(),
        ],
        "both un-promoted map keys report"
    );
    assert_eq!(
        texts(&report, "map_value_type_not_assignable"),
        vec![
            "The element type 'int?' can't be assigned to the map value type \
             'int'."
                .to_string(),
            "The element type 'int?' can't be assigned to the map value type \
             'int'."
                .to_string(),
        ],
        "both un-promoted map values report"
    );
}

/// The null literal keeps its row in every element position: `Ty::Null` is
/// never `nullable: true`, so the parser-time verdict stands unchanged.
#[test]
fn null_literals_keep_their_rows() {
    let report = report_ops(
        "void nullLiterals() {\n\
         \x20 final a = <int>{null};\n\
         \x20 final b = <int>[null];\n\
         \x20 final c = <int, int>{null: null};\n\
         }\n",
    );
    assert_eq!(
        texts(&report, "set_element_type_not_assignable"),
        vec!["The element type 'Null' can't be assigned to the set type 'int'.".to_string()],
        "the null set element reports"
    );
    assert_eq!(
        texts(&report, "list_element_type_not_assignable"),
        vec![
            "The element type 'Null' can't be assigned to the list type \
             'int'."
                .to_string()
        ],
        "the null list element reports"
    );
    assert_eq!(
        texts(&report, "map_key_type_not_assignable"),
        vec![
            "The element type 'Null' can't be assigned to the map key type \
             'int'."
                .to_string()
        ],
        "the null map key reports"
    );
    assert_eq!(
        texts(&report, "map_value_type_not_assignable"),
        vec![
            "The element type 'Null' can't be assigned to the map value type \
             'int'."
                .to_string()
        ],
        "the null map value reports"
    );
}

/// A promotion to a non-assignable subtype flows into the reported face:
/// `if (x is String)` over `Object?` reports `'String'`, never the declared
/// `'Object?'` — the oracle reads the flow-aware static type, and the
/// deferred pass answers the same face at every element position.
#[test]
fn promoted_non_assignable_subtype_flows_into_the_message() {
    let report = report_ops(
        "void promotedSubtype(Object? x) {\n\
         \x20 if (x is String) {\n\
         \x20   final a = <int>{x};\n\
         \x20   final b = <int>[x];\n\
         \x20   final c = <int, int>{x: x};\n\
         \x20 }\n\
         }\n",
    );
    assert_eq!(
        texts(&report, "set_element_type_not_assignable"),
        vec![
            "The element type 'String' can't be assigned to the set type \
             'int'."
                .to_string()
        ]
    );
    assert_eq!(
        texts(&report, "list_element_type_not_assignable"),
        vec![
            "The element type 'String' can't be assigned to the list type \
             'int'."
                .to_string()
        ]
    );
    assert_eq!(
        texts(&report, "map_key_type_not_assignable"),
        vec![
            "The element type 'String' can't be assigned to the map key type \
             'int'."
                .to_string()
        ]
    );
    assert_eq!(
        texts(&report, "map_value_type_not_assignable"),
        vec![
            "The element type 'String' can't be assigned to the map value \
             type 'int'."
                .to_string()
        ]
    );
}

/// The nested and context-donated faces ride the same re-decision: an
/// un-promoted leaf reports through the enclosing literal's donation (map
/// value set/list, declaration-initializer `Set`/`List`/`Map` contexts),
/// while the promoted block stays silent.
#[test]
fn nested_and_context_donated_faces_re_decide() {
    let unpromoted = report_ops(
        "void nestedUnpromoted(int? x) {\n\
         \x20 final a = <String, Set<int>>{'k': {x}};\n\
         \x20 final b = <String, List<int>>{'k': [x]};\n\
         \x20 Set<int> c = {x};\n\
         \x20 List<int> d = [x];\n\
         \x20 Map<int, int> e = {x: x};\n\
         }\n",
    );
    assert_eq!(
        texts(&unpromoted, "set_element_type_not_assignable").len(),
        2,
        "the map-value donation and the declaration context both report"
    );
    assert_eq!(
        texts(&unpromoted, "list_element_type_not_assignable").len(),
        2,
        "the map-value donation and the declaration context both report"
    );
    assert_eq!(texts(&unpromoted, "map_key_type_not_assignable").len(), 1);
    assert_eq!(texts(&unpromoted, "map_value_type_not_assignable").len(), 1);

    let promoted = report_ops(
        "void nestedPromoted(int? x) {\n\
         \x20 if (x != null) {\n\
         \x20   final a = <String, Set<int>>{'k': {x}};\n\
         \x20   final b = <String, List<int>>{'k': [x]};\n\
         \x20   Set<int> c = {x};\n\
         \x20   List<int> d = [x];\n\
         \x20   Map<int, int> e = {x: x};\n\
         \x20  }\n\
         }\n",
    );
    for code in [
        "set_element_type_not_assignable",
        "list_element_type_not_assignable",
        "map_key_type_not_assignable",
        "map_value_type_not_assignable",
    ] {
        assert!(
            texts(&promoted, code).is_empty(),
            "the promoted nested/context faces stay silent ({code})"
        );
    }
}

/// Compound assignment elements (`?x += 1`) and `?`-marked entries keep
/// their parse-time verdicts (this ticket's mandate: null-aware leaves are
/// a different question). Compound elements report no element row in either
/// promotion state (oracle: `invalid_null_aware_operator`/
/// `unchecked_use_of_nullable_value` only). A `?`-marked map value keeps
/// its parse-time answer too — the current face reports the declared
/// nullable type, a pre-existing over-report against the oracle that the
/// null-aware-entry verification owns (residual divergence, out of this
/// ticket's nullable-declared family).
#[test]
fn compound_and_null_aware_elements_keep_their_parse_time_verdicts() {
    let report = report_ops(
        "void compound(int? x) {\n\
         \x20 final a = <int>[?x += 1];\n\
         \x20 final b = <int>{?x += 1};\n\
         \x20 final c = <String, int>{'a': ?x};\n\
         }\n",
    );
    for code in [
        "set_element_type_not_assignable",
        "list_element_type_not_assignable",
        "map_key_type_not_assignable",
    ] {
        assert!(
            texts(&report, code).is_empty(),
            "compound and null-aware elements stay out of the family ({code})"
        );
    }
    assert_eq!(
        texts(&report, "map_value_type_not_assignable"),
        vec![
            "The element type 'int?' can't be assigned to the map value type \
             'int'."
                .to_string()
        ],
        "the `?`-marked map value keeps its parse-time answer (pre-existing \
         over-report the null-aware-entry face owns)"
    );
}
