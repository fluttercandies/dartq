//! The runner: one case through the production lint pipeline, asserted
//! against its folded expectations.
//!
//! The pipeline is the real one — [`dartq_lints::testsupport::Fixture`]
//! (scanner → parser → arena through [`dartq_lints::parse_unit`]), the
//! registry's enable set for the case's lint, and the engine run. When the
//! enabled rule consumes the semantic fold
//! ([`dartq_lints::mounts_semantics`]) the case runs through the
//! production semantic mount ([`Fixture::run_semantic`]: the source folded
//! as the sole analyzed file of `package:t`, dart:core seeded), the same
//! shape the driver mounts per file; every other case runs the plain
//! [`Fixture::run`].
//!
//! Only the case's first file (the analyzed unit) is lint-run; supporting
//! file blocks are parsed and retained for the future cross-file faces.
//!
//! Assertion scope: (code, UTF-16 range) multiset equality — the upstream
//! `assertDiagnosticsFromMarkup` contract — plus the rendered message for
//! expectations that pin one (the line-anchored marker's `: message`).

use std::path::PathBuf;

use dartq_lints::{
    Lint, mounts_semantics, rules,
    testsupport::{Fixture, one_rule},
};
use dartq_source::LineIndex;

use crate::fixture::{Case, Expectation, FixtureDoc};

/// Runs every case of `doc`, asserting each. Quarantined cases (the
/// `//- diverge:` header) are skipped — their divergence reasons stay
/// pinned in the data file for the rule lane to unpick. Every
/// non-quarantined case runs before the first failure panics, so one
/// report names them all.
///
/// # Panics
///
/// See [`assert_case`].
pub fn assert_doc(doc: &FixtureDoc) {
    let mut failures = Vec::new();
    let mut run = 0usize;
    for case in &doc.cases {
        if case.diverge.is_some() {
            continue;
        }
        run += 1;
        let lint = case.lint(&doc.lint);
        let expected = case.analyzed(&doc.lint).expect("parse re-validated").expectations;
        let emitted = run_case(case, &doc.lint);
        if let Err((missing, extra)) = match_rows(&expected, &emitted) {
            failures.push(render_failure(case, lint, missing, extra));
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {run} asserted cases diverged:\n\n{}",
        failures.len(),
        failures.join("\n\n")
    );
}

/// Runs `case` through the production pipeline and asserts its emitted
/// lint rows equal the annotated expectations.
///
/// # Panics
///
/// On any mismatch — with the case name, both row sets rendered
/// `line:col` over the analyzed source, and the source itself. Also
/// panics when the case names a rule the registry does not carry (via
/// [`one_rule`]).
pub fn assert_case(case: &Case, doc_lint: &str) {
    let lint = case.lint(doc_lint);
    let expected = case.analyzed(doc_lint).expect("parse re-validated").expectations;
    let emitted = run_case(case, doc_lint);
    match match_rows(&expected, &emitted) {
        Ok(()) => {}
        Err((missing, extra)) => panic!("{}", render_failure(case, lint, missing, extra)),
    }
}

/// Runs `case` through the production pipeline: the parse bridge, the
/// case's declared path, and the semantic mount when the enabled rule
/// consumes the fold.
#[must_use]
pub fn run_case(case: &Case, doc_lint: &str) -> Vec<Lint> {
    let lint = case.lint(doc_lint);
    let target = &case.files[0];
    let mut fixture = Fixture::parse(&target.code);
    fixture.path = PathBuf::from(target.path.trim_start_matches('/'));
    let enabled = one_rule(lint);
    if mounts_semantics(&enabled) {
        fixture.run_semantic(&enabled)
    } else {
        fixture.run(&enabled)
    }
}

/// The analyzed file's code — the coordinate space expectations and lints
/// both render against.
fn expected_source(case: &Case) -> String {
    case.files[0].code.clone()
}

/// The failure report: case, lint, both row sets (the missing expectations
/// and the unexpected lints), and the analyzed source.
fn render_failure(
    case: &Case,
    lint: &str,
    missing: Vec<Expectation>,
    extra: Vec<Lint>,
) -> String {
    format!(
        "case `{}` (lint `{lint}`) diverged\nmissing expectations:\n{}\nunexpected lints:\n{}\nanalyzed file {}:\n{}",
        case.name,
        rows(&missing, &expected_source(case)),
        rows_lints(&extra),
        case.files[0].path,
        case.files[0].code,
    )
}

/// Matches expected rows against emitted lints (greedy, by code + range,
/// message compared when the expectation pins one). `Ok(())` on equality;
/// otherwise the unmatched expectations and lints.
fn match_rows(
    expected: &[Expectation],
    emitted: &[Lint],
) -> Result<(), (Vec<Expectation>, Vec<Lint>)> {
    let mut taken = vec![false; emitted.len()];
    let mut missing = Vec::new();
    for expectation in expected {
        let hit = emitted.iter().enumerate().find(|(i, lint)| {
            !taken[*i]
                && rules()[lint.rule.0 as usize].name == expectation.code
                && lint.range.start == expectation.range.start
                && lint.range.end == expectation.range.end
                && (expectation.message.is_none() || Some(lint.message.as_str()) == expectation.message.as_deref())
        });
        match hit {
            Some((i, _)) => taken[i] = true,
            None => missing.push(expectation.clone()),
        }
    }
    let extra: Vec<Lint> = emitted
        .iter()
        .enumerate()
        .filter(|(i, _)| !taken[*i])
        .map(|(_, lint)| lint.clone())
        .collect();
    if missing.is_empty() && extra.is_empty() {
        Ok(())
    } else {
        Err((missing, extra))
    }
}

/// Renders expectation rows with `line:col` (UTF-16 positions) over the
/// analyzed source.
fn rows(items: &[Expectation], source: &str) -> String {
    if items.is_empty() {
        return "  (none)".into();
    }
    let lines = LineIndex::new(source);
    items
        .iter()
        .map(|e| {
            let (line, col) = lines.line_col_utf16(source, e.range.start);
            let message = e.message.as_deref().map_or(String::new(), |m| format!(": {m}"));
            format!("  {line}:{col} {} [{}..{}]{message}", e.code, e.range.start, e.range.end)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Renders unmatched lints the same way (the code's own message always
/// shown).
fn rows_lints(items: &[Lint]) -> String {
    if items.is_empty() {
        return "  (none)".into();
    }
    items
        .iter()
        .map(|l| {
            format!(
                "  {} [{}..{}]: {}",
                rules()[l.rule.0 as usize].name,
                l.range.start,
                l.range.end,
                l.message
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}
