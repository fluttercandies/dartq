//! Ticket #4335 pins: the pre-generic-metadata list-literal element face
//! of the `could_not_infer` cluster (#4149 residue T4).
//!
//! A contextless untyped list literal whose upward element candidate is a
//! generic function type is not a legal element type while the diagnosing
//! file's language version predates `generic-metadata` (enabledIn 2.14 in
//! the vendored registry; the 3.13.7 oracle reports at `@dart=2.13` and
//! stays clean at 2.14 — #4337's boundary probes): the element inferrer
//! reports `could_not_infer` at the whole literal with the "Inferred
//! candidate type … has type parameters …" detail (upstream Dart SDK
//! source, pkg/analyzer/lib/src/dart/element/generic_inferrer.dart:379-407,
//! the `_genericMetadataIsEnabled` gate fed by
//! invocation_inference_helper.dart:74-75 — the engine's settled feature
//! flag, the same channel a pre-2.14 package default opts out through).
//!
//! The cluster's remaining twelve rows root in the invocation-inference
//! seam (`invocation.rs` — the tear-off argument instantiation feed, the
//! creation solves, the mutual-bounds instantiate-to-bounds), which a
//! sibling lane owns this round; the handoff map lives in the lane
//! report, not here. The bare rig (no `CORE_PRELUDE`) keeps the fixture's
//! `@dart` override the file's leading comment — the position the
//! override must occupy.

#![allow(clippy::semicolon_if_nothing_returned)]
use super::test_rig::report_ops_bare;
use super::*;

/// `(code, anchored text, message)` rows in source order.
fn rows(report: &FileReport, full: &str) -> Vec<(&'static str, String, String)> {
    report
        .diagnostics
        .iter()
        .map(|d| {
            (
                d.code.name,
                full[d.range.offset as usize..(d.range.offset + d.range.length) as usize]
                    .to_string(),
                d.message.to_string(),
            )
        })
        .collect()
}

/// `[f]` over `void f<X>() {}` under `// @dart=2.12` — the corpus row
/// (`CouldNotInferTest/test_functionType_optOutOfGenericMetadata#0`),
/// oracle-word-for-word: `could_not_infer` anchored at the whole literal.
#[test]
fn opt_out_generic_metadata_list_element_reports() {
    let fixture = "// @dart=2.12\n\
                   void f<X>() {}\n\
                   main() {\n\
                   \x20 [f];\n\
                   }\n";
    let report = report_ops_bare(fixture);
    assert_eq!(
        rows(&report, fixture),
        vec![(
            "could_not_infer",
            "[f]".to_string(),
            "Couldn't infer type parameter 'E'. Inferred candidate type \
             void Function<X>() has type parameters [X], but a function with \
             type parameters cannot be used as a type argument."
                .to_string(),
        )],
        "rows: {:?}",
        rows(&report, fixture)
    );
}

/// The same literal without the override comment — the modern default
/// keeps generic metadata on, so the candidate is a legal type argument
/// and the file stays silent.
#[test]
fn modern_default_stays_silent() {
    let fixture = "void f<X>() {}\n\
                   main() {\n\
                   \x20 [f];\n\
                   }\n";
    let report = report_ops_bare(fixture);
    assert!(
        report.diagnostics.is_empty(),
        "rows: {:?}",
        rows(&report, fixture)
    );
}

/// `// @dart=2.13` still predates the feature's 2.14 enable — the same
/// literal reports (3.13.7-oracle-probed; the gate is the version, not
/// the comment's presence). #4337's envelope flip: the registry threshold
/// is 2.14, and this pin originally expected 2.13 to stay clean.
#[test]
fn dart_2_13_reports() {
    let fixture = "// @dart=2.13\n\
                   void f<X>() {}\n\
                   main() {\n\
                   \x20 [f];\n\
                   }\n";
    let report = report_ops_bare(fixture);
    assert_eq!(
        rows(&report, fixture),
        vec![(
            "could_not_infer",
            "[f]".to_string(),
            "Couldn't infer type parameter 'E'. Inferred candidate type \
             void Function<X>() has type parameters [X], but a function with \
             type parameters cannot be used as a type argument."
                .to_string(),
        )],
        "rows: {:?}",
        rows(&report, fixture)
    );
}

/// A NON-generic function reference is a legal element candidate even
/// under the opt-out — the face keys on the candidate's own type
/// parameters, never on the tear-off shape itself.
#[test]
fn non_generic_candidate_stays_silent() {
    let fixture = "// @dart=2.12\n\
                   void g() {}\n\
                   main() {\n\
                   \x20 [g];\n\
                   }\n";
    let report = report_ops_bare(fixture);
    assert!(
        report.diagnostics.is_empty(),
        "rows: {:?}",
        rows(&report, fixture)
    );
}

/// A generic candidate beside a non-identifier element: the upward LUB
/// leaves the function family (`UP(fn, int)` resolves past `Object`), so
/// the candidate is never a generic function type — 3.13.7-oracle-probed
/// silence, parity (#4337).
#[test]
fn mixed_elements_stay_silent() {
    let fixture = "// @dart=2.12\n\
                   void f<X>() {}\n\
                   main() {\n\
                   \x20 [f, 0];\n\
                   }\n";
    let report = report_ops_bare(fixture);
    assert!(
        report.diagnostics.is_empty(),
        "rows: {:?}",
        rows(&report, fixture)
    );
}

/// The override comment after a line of code is not a leading override —
/// the scan stops at the first code line and the modern default stands.
#[test]
fn late_override_comment_stays_silent() {
    let fixture = "void f<X>() {}\n\
                   // @dart=2.12\n\
                   main() {\n\
                   \x20 [f];\n\
                   }\n";
    let report = report_ops_bare(fixture);
    assert!(
        report.diagnostics.is_empty(),
        "rows: {:?}",
        rows(&report, fixture)
    );
}
