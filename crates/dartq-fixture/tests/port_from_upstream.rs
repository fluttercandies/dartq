//! The env-gated port generator: converts the upstream linter rule tests
//! into the checked-in `.fixture` data files and pins the converter
//! against the real upstream corpus.
//!
//! Two environment variables drive it (unset → the test skips, so plain
//! CI runs are a no-op):
//!
//! ```text
//! DARTQ_UPSTREAM_RULE_TESTS=<dir of *_test.dart>   the upstream corpus
//! DARTQ_FIXTURE_OUT=<dir>                          where <rule>.fixture lands
//! cargo test -p dartq-fixture --test port_from_upstream -- --nocapture
//! ```
//!
//! The pin half runs whenever the corpus dir is set: `avoid_empty_else`
//! must port to exactly four cases — the two markup reports and the two
//! clean bodies — with the raw `assertDiagnostics` test rejected, the
//! converter's canonical no-ambiguity proof.

use std::path::{Path, PathBuf};

/// The proof-port rule set (the ten rules `dartq-lints`' upstream_cases
/// driver pins). A rule without a `<rule>_test.dart` in the corpus dir
/// fails the run — the data file set and the driver must stay in lockstep.
const RULES: &[&str] = &[
    "avoid_empty_else",
    "constant_identifier_names",
    "control_flow_in_finally",
    "curly_braces_in_flow_control_structures",
    "empty_catches",
    "hash_and_equals",
    "leading_newlines_in_multiline_strings",
    "prefer_is_not_empty",
    "prefer_is_not_operator",
    "unnecessary_new",
];

#[test]
fn ports_upstream_rule_tests_when_configured() {
    let Some(corpus) = std::env::var_os("DARTQ_UPSTREAM_RULE_TESTS") else {
        eprintln!("port_from_upstream: DARTQ_UPSTREAM_RULE_TESTS unset; skipping");
        return;
    };
    let corpus = PathBuf::from(corpus);
    let out = std::env::var_os("DARTQ_FIXTURE_OUT").map(PathBuf::from);
    for rule in RULES {
        let source = std::fs::read_to_string(corpus.join(format!("{rule}_test.dart")))
            .unwrap_or_else(|e| panic!("{rule}_test.dart: {e}"));
        let outcome = dartq_fixture::port_file(&source)
            .unwrap_or_else(|reason| panic!("{rule}: whole-file reject: {reason}"));
        let rendered = dartq_fixture::render(&outcome);
        let (ported, skipped) = outcome.tally();
        eprintln!(
            "{rule}: rule `{}` — {ported} ported, {skipped} skipped/rejected",
            outcome.rule
        );
        for (test, reason) in &outcome.skipped {
            eprintln!("  skipped {test}: {reason}");
        }
        match &out {
            Some(dir) => {
                std::fs::create_dir_all(dir).expect("create out dir");
                let path = Path::new(dir).join(format!("{rule}.fixture"));
                // A previous generation's `//- diverge:` quarantines ride
                // into the new file — regeneration never unpicks a pinned
                // divergence.
                let previous = std::fs::read_to_string(&path).unwrap_or_default();
                let (rendered, stale) = dartq_fixture::merge_divergences(&rendered, &previous);
                assert!(
                    stale.is_empty(),
                    "{rule}: stale quarantines (cases no longer porting): {stale:?}"
                );
                let doc = dartq_fixture::parse(&rendered)
                    .unwrap_or_else(|e| panic!("{rule}: rendered fixture does not parse: {e:?}"));
                assert_eq!(doc.cases.len(), ported, "{rule}: case count");
                std::fs::write(&path, rendered)
                    .unwrap_or_else(|e| panic!("write {}: {e}", path.display()));
                eprintln!("  wrote {}", path.display());
            }
            None => {
                let doc = dartq_fixture::parse(&rendered)
                    .unwrap_or_else(|e| panic!("{rule}: rendered fixture does not parse: {e:?}"));
                assert_eq!(doc.cases.len(), ported, "{rule}: case count");
                eprintln!("  (DARTQ_FIXTURE_OUT unset; not writing)");
            }
        }
    }
    pin_avoid_empty_else(&corpus);
}

/// The converter self-test against the canonical real file: five test
/// methods — two markup, two clean, one raw `assertDiagnostics` — with the
/// first markup case's marked range pinned to the `;` of the empty else.
fn pin_avoid_empty_else(corpus: &Path) {
    let source = std::fs::read_to_string(corpus.join("avoid_empty_else_test.dart")).expect("read");
    let outcome = dartq_fixture::port_file(&source).expect("ports");
    assert_eq!(outcome.rule, "avoid_empty_else");
    let (ported, skipped) = outcome.tally();
    assert_eq!(
        (ported, skipped),
        (4, 1),
        "2 markup + 2 clean ported, 1 raw rejected"
    );
    assert!(
        outcome.skipped[0]
            .0
            .starts_with("test_else_noStatement_notEnclosed"),
        "the rejected case is the raw assertDiagnostics one: {:?}",
        outcome.skipped
    );
    let names: Vec<&str> = outcome.ported.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(
        names,
        vec![
            "test_else_emptyStatement_hasElseIf",
            "test_else_emptyStatement_noElseIf",
            "test_else_noEmptyStatement_enclosed",
            "test_else_noEmptyStatement_notEnclosed",
        ]
    );
    let rendered = dartq_fixture::render(&outcome);
    let doc = dartq_fixture::parse(&rendered).expect("parses");
    let first = &doc.cases[0];
    // The verbatim upstream body: the raw string's leading newline is the
    // first (empty) line, eleven lines in total.
    assert_eq!(first.files[0].code.lines().count(), 11);
    let analyzed = first.analyzed("avoid_empty_else").expect("analyzed");
    assert_eq!(analyzed.expectations.len(), 1);
    assert_eq!(analyzed.expectations[0].code, "avoid_empty_else");
    let range = analyzed.expectations[0].range.clone();
    assert_eq!(
        &analyzed.code[range.start as usize..range.end as usize],
        ";",
        "the marked range is the empty-else `;`"
    );
    let line_start = analyzed.code[..range.start as usize]
        .rfind('\n')
        .map_or(0, |at| at + 1);
    let line_end = analyzed.code[line_start..]
        .find('\n')
        .map_or(analyzed.code.len(), |at| line_start + at);
    assert_eq!(&analyzed.code[line_start..line_end], "  else ;");
}
