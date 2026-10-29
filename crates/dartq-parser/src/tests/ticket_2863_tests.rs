//! #2863 exact-row pins: the anonymous-methods experiment grammar —
//! `receiver.{ ... }`, `receiver.=> e`, `receiver.(p) { ... }` and their
//! `?.`/`..`/`?..` faces, behind
//! [`crate::ParserFeatures::anonymous_methods_enabled`] (upstream Dart SDK
//! source, pkg/_fe_analyzer_shared/lib/src/parser/parser_impl.dart,
//! `:7292-7295` selector dispatch, `:7524-7547` `_beginsAnonymousMethod`
//! lookahead, `:7550-7590` `_parseAnonymousMethod` body, `:7770-7801`
//! cascade dispatch). With the experiment off the member dispatch falls
//! through to the ordinary selector path — the upstream
//! `reportExperimentNotEnabled` call is an unimplemented stub that never
//! fires, so the off posture is the plain noise rows pinned below
//! (oracle-pinned by the DeadCode experiment corpus rows). The
//! missing-body recovery arm
//! (`:7581-7589`) is unreachable through the public gate — the lookahead
//! and the body check read the same follower unless a recovery synthetic is
//! spliced between them — so every clean-parse pin doubles as its
//! does-not-fire-spuriously proof.

use super::collector::{Collector, latest_features, scan_ok};
use crate::ParserFeatures;
use dartq_language::{Feature, FeatureSet};

/// The latest fold with the anonymous-methods experiment opted in — the
/// corpus `//- options: analyzer: enable-experiment: [anonymous-methods]`
/// channel folds through the same set.
fn anon_features() -> ParserFeatures {
    ParserFeatures::from_feature_set(
        FeatureSet::latest().with_experiment(Feature::AnonymousMethods),
    )
}

fn rows_with(source: &'static str, features: ParserFeatures) -> Vec<(String, u32, u32)> {
    let (table, text) = scan_ok(source);
    let mut collector = Collector::default();
    let mut parser = crate::Parser::new(text, table, features, &mut collector);
    parser.parse_unit();
    parser.listener.errors.clone()
}

/// `receiver.=> e` (upstream expression/plain_test): the implicit-params
/// arrow member parses clean with the experiment on.
#[test]
fn selector_arrow_member_parses_clean() {
    let rows = rows_with("void main() {\n  buffer.=> foo;\n}\n", anon_features());
    assert!(rows.is_empty(), "flag-on `.=>` must parse clean: {rows:?}");
}

/// `receiver.{ ... }` (upstream block/plain_test): the implicit-params
/// block member parses clean with the experiment on.
#[test]
fn selector_block_member_parses_clean() {
    let rows = rows_with(
        "void main() {\n  buffer.{ return foo; };\n}\n",
        anon_features(),
    );
    assert!(
        rows.is_empty(),
        "flag-on dot-block member must parse clean: {rows:?}"
    );
}

/// `receiver.(int x) => e` — an explicitly parameterized arrow member; the
/// wrong-parameter-type face is semantic (ast_builder), never parser rows.
#[test]
fn selector_explicit_params_arrow_parses_clean() {
    let rows = rows_with(
        "void main() {\n  buffer.(int x) => x;\n}\n",
        anon_features(),
    );
    assert!(
        rows.is_empty(),
        "flag-on `.(x) =>` must parse clean: {rows:?}"
    );
}

/// `receiver.(String s) { ... }` — explicitly parameterized block member.
#[test]
fn selector_explicit_params_block_parses_clean() {
    let rows = rows_with(
        "void main() {\n  buffer.(String s) { return s; };\n}\n",
        anon_features(),
    );
    assert!(
        rows.is_empty(),
        "flag-on explicit-params block must parse clean: {rows:?}"
    );
}

/// `receiver.() => 1` — an EMPTY explicit list parses clean at the parser
/// tier: the wrong-parameter-list code is ast_builder-side (second slice).
#[test]
fn selector_empty_params_arrow_keeps_parser_rows_empty() {
    let rows = rows_with("void main() {\n  1.() => 1;\n}\n", anon_features());
    assert!(
        rows.is_empty(),
        "flag-on `.() =>` must parse clean: {rows:?}"
    );
}

/// `receiver?.=> e` / `receiver?.{ ... }`: the null-aware selector faces.
/// The anonymous arm fires no null-aware split (the upstream anonymous arm
/// carries no such event) and still parses clean.
#[test]
fn null_aware_selector_members_parse_clean() {
    let rows = rows_with(
        "void main() {\n  buffer?.=> foo;\n  buffer?.{ return foo; };\n}\n",
        anon_features(),
    );
    assert!(
        rows.is_empty(),
        "flag-on `?.` faces must parse clean: {rows:?}"
    );
}

/// `receiver..{ ... }` / `receiver?..{ ... }` / `receiver..(p) { ... }`:
/// the cascade initial-section faces (upstream block/cascaded_test,
/// null_aware_cascaded_test).
#[test]
fn cascade_initial_section_members_parse_clean() {
    let rows = rows_with(
        "void main() {\n  buffer..{ return foo; };\n  buffer?..{ return foo; };\n  buffer..(p) { return p; };\n}\n",
        anon_features(),
    );
    assert!(
        rows.is_empty(),
        "flag-on cascade faces must parse clean: {rows:?}"
    );
}

/// `receiver..=> e` and `receiver?..=> e`: the cascade arrow faces
/// (upstream expression/cascaded_test).
#[test]
fn cascade_arrow_members_parse_clean() {
    let rows = rows_with(
        "void main() {\n  buffer..=> foo;\n  buffer?..=> foo;\n}\n",
        anon_features(),
    );
    assert!(
        rows.is_empty(),
        "flag-on cascade arrows must parse clean: {rows:?}"
    );
}

/// `(receiver.=> e).=> f` (upstream plain_test v2): an anonymous member
/// nested in a parenthesized receiver, then another anonymous member.
#[test]
fn parenthesized_receiver_then_selector_member_parses_clean() {
    let rows = rows_with(
        "void main() {\n  final v = (buffer.=> foo).=> bar;\n}\n",
        anon_features(),
    );
    assert!(
        rows.is_empty(),
        "nested `.=>` faces must parse clean: {rows:?}"
    );
}

/// `receiver.=> foo.=> bar` (upstream cascaded_test v6 shape): a chained
/// selector-level anonymous member — the second gate reads the member
/// after the first arrow body's expression.
#[test]
fn chained_selector_members_parse_clean() {
    let rows = rows_with(
        "void main() {\n  final v = buffer.=> foo.=> bar;\n}\n",
        anon_features(),
    );
    assert!(
        rows.is_empty(),
        "chained `.=>` faces must parse clean: {rows:?}"
    );
}

/// `receiver.=> foo..=> bar` (upstream cascaded_test v7): a selector-level
/// member followed by a cascade-level member in the same expression.
#[test]
fn selector_then_cascade_members_parse_clean() {
    let rows = rows_with(
        "void main() {\n  final v = buffer.=> foo..=> bar;\n}\n",
        anon_features(),
    );
    assert!(
        rows.is_empty(),
        "selector+cascade mix must parse clean: {rows:?}"
    );
}

/// Flag off: `.` + `=>` falls through to the ordinary selector path — the
/// operand takes parse_primary's identifier recovery, the single
/// `missing_identifier` row the oracle emits for the same input.
#[test]
fn flag_off_selector_arrow_is_ordinary_noise() {
    let rows = rows_with("void main() {\n  1.=> 1;\n}\n", latest_features());
    assert!(
        rows.contains(&("missing_identifier".into(), 18, 20)),
        "missing_identifier on the `=>`: {rows:?}"
    );
}

/// Flag off: `.` + `{` — the `{` operand takes the arm's own
/// missing_identifier anchor (OpenCurly is a complete-expression operand),
/// then parses as a set/map literal (the oracle's ordinary path too).
#[test]
fn flag_off_selector_block_is_ordinary_noise() {
    let rows = rows_with("void main() {\n  1.{ return 1; }\n}\n", latest_features());
    assert!(
        rows.contains(&("missing_identifier".into(), 18, 19)),
        "missing_identifier on the open-brace operand: {rows:?}"
    );
}

/// Flag off: `.` + `(p) => p` — the `(` operand anchors the arm's
/// missing_identifier row, then the group reads as a function literal (the
/// oracle's ordinary path too).
#[test]
fn flag_off_selector_params_is_ordinary_noise() {
    let rows = rows_with("void main() {\n  1.(p) => p;\n}\n", latest_features());
    assert!(
        rows.contains(&("missing_identifier".into(), 18, 19)),
        "missing_identifier on the `(`: {rows:?}"
    );
}
