//! The upstream behavioral port: ten rules' worth of the official linter
//! test corpus, converted mechanically by `port_upstream` (the
//! dartq-fixture crate's converter) into `tests/data/upstream/*.fixture`
//! and run through the production pipeline here — the parse bridge, the
//! registry enable set, and the engine run with the semantic fold mounted
//! for the fold-consuming rules (`prefer_is_not_empty`).
//!
//! Case names are the upstream test method names, verbatim — each carried
//! case names its origin in place. The converter's per-file tally (what
//! ported, what it rejected and why) is pinned in dartq-fixture's
//! `port_from_upstream` generator test against the real corpus.
//!
//! Scope note: these are behavioral cases — real Dart in, emitted lint
//! rows out. Payload-contract pins (the AST shape a rule reads) stay with
//! the hand-built arena fixtures in `src/`; this file does not replace
//! them.
//!
//! Regeneration (converter run over the upstream corpus):
//!
//! ```text
//! DARTQ_UPSTREAM_RULE_TESTS=<upstream rules dir> \
//! DARTQ_FIXTURE_OUT=crates/dartq-lints/tests/data/upstream \
//! cargo test -p dartq-fixture --test port_from_upstream -- --nocapture
//! ```

macro_rules! upstream {
    ($test:ident, $rule:literal) => {
        #[test]
        fn $test() {
            let text = include_str!(concat!("data/upstream/", $rule, ".fixture"));
            let doc = dartq_fixture::parse(text)
                .unwrap_or_else(|e| panic!("{}: fixture does not parse: {e:?}", $rule));
            dartq_fixture::assert_doc(&doc);
        }
    };
}

upstream!(avoid_empty_else, "avoid_empty_else");
upstream!(constant_identifier_names, "constant_identifier_names");
upstream!(control_flow_in_finally, "control_flow_in_finally");
upstream!(
    curly_braces_in_flow_control_structures,
    "curly_braces_in_flow_control_structures"
);
upstream!(empty_catches, "empty_catches");
upstream!(hash_and_equals, "hash_and_equals");
upstream!(
    leading_newlines_in_multiline_strings,
    "leading_newlines_in_multiline_strings"
);
upstream!(prefer_is_not_empty, "prefer_is_not_empty");
upstream!(prefer_is_not_operator, "prefer_is_not_operator");
upstream!(unnecessary_new, "unnecessary_new");
