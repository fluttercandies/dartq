//! #3644 pins: the malformed map-pattern entry recovery
//! (`final {"k": int x < 0} = ...` — a relational-pattern junk tail where
//! the entry's value pattern ends).
//!
//! Upstream recovery (upstream Dart SDK source,
//! pkg/_fe_analyzer_shared/lib/src/parser/parser_impl.dart
//! `parseLiteralListSetMapOrFunction` :8740-8745) probes the `<...>` group
//! with `inDeclaration: true`, so `< 0` — no type-shaped content — forms a
//! RECOVERED type-argument group: `parseArguments` reports `expectedType`
//! at the junk operand and its `parseUnexpectedEnd` tail consumes only the
//! `<` plus spliced synthetics (`start.endGroup` is null — the scanner's
//! `discardBeginGroupUntil`/`discardOpenLt` drop the unpaired `<` when the
//! entry's `}` closes, abstract_scanner.dart:739-745/878-882), so the
//! operand stays current. The `[`-face (:8767-8775), the map loop's `:`
//! recovery (`rewriteAndRecover` reports at the current token, :5067-5070)
//! and the entry's value pattern (the refutable constant `0`) all anchor
//! there — the oracle's four 32:27 faces on the `pattern_context_A01_t02`
//! case. dartq's reduced engine probed the group in non-declaration mode,
//! got no group, and the unconsumed `<` fell through to the binary loop,
//! which swallowed the operand and dragged every anchor one token right
//! (the refutable row landed on the closing `}`).

use super::collector::{Collector, latest_features, scan_ok};

fn rows_of(source: &'static str) -> Vec<(String, u32, u32)> {
    let (table, text) = scan_ok(source);
    let mut collector = Collector::default();
    let mut parser = crate::Parser::new(text, table, latest_features(), &mut collector);
    parser.parse_unit();
    let mut rows = parser.listener.errors.clone();
    // Canonical order: position, then code — rows sharing an anchor compare
    // deterministically (gate_2791 harness shape).
    rows.sort_by(|a, b| (a.1, a.2, &a.0).cmp(&(b.1, b.2, &b.0)));
    rows
}

/// The oracle row set for the case's line-32 face
/// (conformance/expected.tsv, `pattern_context_A01_t02`): the `,` recovery
/// anchors on the `<`, and the type-name / `[` / `:` / refutable faces all
/// anchor on the operand `0` — never on the closing `}`.
#[test]
fn malformed_map_entry_anchors_all_faces_on_the_operand() {
    const SRC: &str = "void f() { final {\"key1\": int x4 < 0} = {\"key1\": -1}; }\n";
    // Offsets: the `<` at 33, the operand `0` at 35, the `}` at 36.
    assert_eq!(
        rows_of(SRC),
        vec![
            ("expected_token".to_owned(), 33, 34), // "Expected to find ','."
            ("expected_token".to_owned(), 35, 36), // "Expected to find ':'."
            ("expected_token".to_owned(), 35, 36), // "Expected to find '['."
            ("expected_type_name".to_owned(), 35, 36),
            (
                "refutable_pattern_in_irrefutable_context".to_owned(),
                35,
                36
            ),
        ]
    );
}

/// The recovery leaves the closing `}` silent: no identifier is demanded
/// there (the old face parsed a constant pattern on the `}` and reported
/// `missing_identifier`), and the refutable constant pattern is the
/// operand itself.
#[test]
fn recovery_leaves_the_close_brace_silent() {
    const SRC: &str = "void f() { final {\"k\": int x < 0} = v; }\n";
    let rows = rows_of(SRC);
    assert!(
        !rows.iter().any(|(c, _, _)| c == "missing_identifier"),
        "{rows:?}"
    );
    // Operand `0` at 31; the `}` at 32 carries no row.
    assert!(
        rows.contains(&(
            "refutable_pattern_in_irrefutable_context".to_owned(),
            31,
            32
        )),
        "{rows:?}"
    );
}

/// Clean typed list/map literals reached through the same arm (map-pattern
/// keys, relational-pattern operands) keep their zero-row parse: the
/// declaration-mode probe changes only RECOVERED shapes — valid groups are
/// the Simple1/clean-Complex fast paths under either flag.
#[test]
fn clean_typed_literals_stay_silent() {
    for src in [
        "void f() { if (o case {<int>[1]: 2}) {} }",
        "void f() { if (o case == <int>[1]) {} }",
    ] {
        assert!(rows_of(src).is_empty(), "{src}: {:?}", rows_of(src));
    }
}
