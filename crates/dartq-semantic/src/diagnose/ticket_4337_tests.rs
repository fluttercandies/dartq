//! Ticket #4337 pins: the `could_not_infer` generic-metadata under-report
//! envelope around #4335's list-literal face — every expectation below is
//! 3.13.7-oracle-probed (`dart analyze --format=machine`, probe package
//! with a `2.12` package default; raw probe rows in the ticket report).
//!
//! * **Multi-name elements**: `[f, g]` REPORTS — the upward candidate is
//!   the function-type LUB of the elements, and two same-shape generic
//!   tear-offs join to the first element's spelling (`[g, f]` reports
//!   `void Function<Y>()`, upstream `relateTypeParameters` fresh-copies
//!   the FIRST list's parameter names). Shape-gate rejections stay
//!   silent exactly as the oracle does: type-parameter count mismatch,
//!   bounds mismatch, a non-generic function beside a generic one.
//! * **Set/map literals stay SILENT** — parity, not a gap: upstream's
//!   set/map element inferers are constructed WITHOUT the diagnostic
//!   reporter (`typed_literal_resolver.dart` `_inferSetTypeDownwards`
//!   :640-661 / `_inferMapTypeDownwards` :525-549 pass no
//!   `diagnosticReporter`; only `_inferListTypeDownwards` :463-484
//!   does), so the generic-metadata report structurally cannot fire —
//!   probed: `{f}`, `{f, f}`, `{1: f}`, `{f: 1}` all stay clean.
//! * **Version gate boundary**: `generic-metadata` ships at 2.14 in the
//!   vendored registry, so `@dart=2.13` REPORTS and `@dart=2.14` stays
//!   clean — the gate reads the engine's settled feature flag, the same
//!   channel a pre-2.14 PACKAGE default (no `// @dart` comment; the
//!   package config's `languageVersion`, engine `language_version_for`
//!   → `scan_unit`'s settled set) opts out through. Probed: a `2.12`
//!   package default with no comment reports.
//! * **Call-inference gate — HANDOFF, not pinned**: the same inferrer
//!   gate fires in CALL argument inference upstream
//!   (`invocation_inference_helper.dart` wires the reporter): `g(f)`
//!   over `void g<T>(T x) {}` reports `Couldn't infer type parameter
//!   'T'.` at the CALLEE NAME (`g`, len 1 — probed; `mk(f)` at `mk`).
//!   The root is the argument-inference seam in `invocation.rs` (#4332's
//!   live lane; the same seam as #4336's handed-off rows), so this file
//!   deliberately pins no call-shaped silence — the face belongs to that
//!   lane's closure, not to the list-literal gate here.
//!
//! Residual under-report (documented, oracle-reporting shapes this gate
//! keeps silent): joined FORMALS the GLB cannot decide (`[f(int), g(double)]`
//! — the oracle spells the GLB `Never`), and F-bounds that spell
//! differently across the two declarations (upstream alpha-substitutes
//! before comparing bounds).

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

/// The `could_not_infer` rows alone — the set/map fixtures ride `var x =`
/// shapes whose unused-local face is not this ticket's question.
fn infer_rows(report: &FileReport, full: &str) -> Vec<(&'static str, String, String)> {
    rows(report, full)
        .into_iter()
        .filter(|(code, _, _)| *code == "could_not_infer")
        .collect()
}

/// `[f, g]` over two same-shape generics with different parameter names —
/// the oracle reports the FIRST element's spelling (probe
/// `e1_two_names`: 5|3|6, `void Function<X>() has type parameters [X]`).
#[test]
fn multi_name_elements_report() {
    let fixture = "// @dart=2.12\n\
                   void f<X>() {}\n\
                   void g<Y>() {}\n\
                   main() {\n\
                   \x20 [f, g];\n\
                   }\n";
    let report = report_ops_bare(fixture);
    assert_eq!(
        rows(&report, fixture),
        vec![(
            "could_not_infer",
            "[f, g]".to_string(),
            "Couldn't infer type parameter 'E'. Inferred candidate type \
             void Function<X>() has type parameters [X], but a function with \
             type parameters cannot be used as a type argument."
                .to_string(),
        )],
        "rows: {:?}",
        rows(&report, fixture)
    );
}

/// `[g, f]` — the join keeps the first-seen element's parameter names
/// (probe `e1_order`: `void Function<Y>() has type parameters [Y]`).
#[test]
fn multi_name_order_keeps_first_names() {
    let fixture = "// @dart=2.12\n\
                   void f<X>() {}\n\
                   void g<Y>() {}\n\
                   main() {\n\
                   \x20 [g, f];\n\
                   }\n";
    let report = report_ops_bare(fixture);
    assert_eq!(
        rows(&report, fixture),
        vec![(
            "could_not_infer",
            "[g, f]".to_string(),
            "Couldn't infer type parameter 'E'. Inferred candidate type \
             void Function<Y>() has type parameters [Y], but a function with \
             type parameters cannot be used as a type argument."
                .to_string(),
        )],
        "rows: {:?}",
        rows(&report, fixture)
    );
}

/// `[f, f]` — the identical pair joins to itself (probe `e1_same_twice`).
#[test]
fn same_name_twice_reports() {
    let fixture = "// @dart=2.12\n\
                   void f<X>() {}\n\
                   main() {\n\
                   \x20 [f, f];\n\
                   }\n";
    let report = report_ops_bare(fixture);
    assert_eq!(
        rows(&report, fixture),
        vec![(
            "could_not_infer",
            "[f, f]".to_string(),
            "Couldn't infer type parameter 'E'. Inferred candidate type \
             void Function<X>() has type parameters [X], but a function with \
             type parameters cannot be used as a type argument."
                .to_string(),
        )],
        "rows: {:?}",
        rows(&report, fixture)
    );
}

/// `[f, g, h]` — three same-shape names, one report at the whole literal
/// (probe `e1_three_names`: 6|11|9).
#[test]
fn three_names_report() {
    let fixture = "// @dart=2.12\n\
                   void f<X>() {}\n\
                   void g<Y>() {}\n\
                   void h<Z>() {}\n\
                   main() {\n\
                   \x20 [f, g, h];\n\
                   }\n";
    let report = report_ops_bare(fixture);
    assert_eq!(
        rows(&report, fixture),
        vec![(
            "could_not_infer",
            "[f, g, h]".to_string(),
            "Couldn't infer type parameter 'E'. Inferred candidate type \
             void Function<X>() has type parameters [X], but a function with \
             type parameters cannot be used as a type argument."
                .to_string(),
        )],
        "rows: {:?}",
        rows(&report, fixture)
    );
}

/// Returns join through the LUB: `int`/`double` joins `num` (probe
/// `e1_num_merge`: `num Function<X>() has type parameters [X]`). The
/// bare rig carries no `dart:core` prelude, so the fixture's own
/// `int`/`double` heads also read as `undefined_class` rows — the pin
/// filters to the inference face those heads feed.
#[test]
fn numeric_return_join_reports_num() {
    let fixture = "// @dart=2.12\n\
                   int f<X>() => 1;\n\
                   double g<Y>() => 1.5;\n\
                   main() {\n\
                   \x20 [f, g];\n\
                   }\n";
    let report = report_ops_bare(fixture);
    assert_eq!(
        infer_rows(&report, fixture),
        vec![(
            "could_not_infer",
            "[f, g]".to_string(),
            "Couldn't infer type parameter 'E'. Inferred candidate type \
             num Function<X>() has type parameters [X], but a function with \
             type parameters cannot be used as a type argument."
                .to_string(),
        )],
        "rows: {:?}",
        rows(&report, fixture)
    );
}

/// Typed formals and returns carry verbatim when the shapes agree modulo
/// parameter naming (probe `e1_typed_params`: `T Function<T>(T)`).
#[test]
fn typed_params_same_shape_report() {
    let fixture = "// @dart=2.12\n\
                   T f<T>(T x) => x;\n\
                   U g<U>(U y) => y;\n\
                   main() {\n\
                   \x20 [f, g];\n\
                   }\n";
    let report = report_ops_bare(fixture);
    assert_eq!(
        rows(&report, fixture),
        vec![(
            "could_not_infer",
            "[f, g]".to_string(),
            "Couldn't infer type parameter 'E'. Inferred candidate type \
             T Function<T>(T) has type parameters [T], but a function with \
             type parameters cannot be used as a type argument."
                .to_string(),
        )],
        "rows: {:?}",
        rows(&report, fixture)
    );
}

/// Type-parameter COUNT mismatch joins to the bare `Function` interface —
/// the generic-metadata report cannot fire (probe `e1_arity_mismatch`:
/// clean).
#[test]
fn arity_mismatch_stays_silent() {
    let fixture = "// @dart=2.12\n\
                   void f<X, Y>() {}\n\
                   void g<Z>() {}\n\
                   main() {\n\
                   \x20 [f, g];\n\
                   }\n";
    let report = report_ops_bare(fixture);
    assert!(
        infer_rows(&report, fixture).is_empty(),
        "rows: {:?}",
        rows(&report, fixture)
    );
}

/// Bounds mismatch fails `relateTypeParameters` — the join answers
/// `Function` and the report cannot fire (probe `e1_bounds_mismatch`:
/// clean).
#[test]
fn bounds_mismatch_stays_silent() {
    let fixture = "// @dart=2.12\n\
                   void f<X extends num>() {}\n\
                   void g<Y>() {}\n\
                   main() {\n\
                   \x20 [f, g];\n\
                   }\n";
    let report = report_ops_bare(fixture);
    assert!(
        infer_rows(&report, fixture).is_empty(),
        "rows: {:?}",
        rows(&report, fixture)
    );
}

/// A non-generic function beside a generic one: the count mismatch again
/// — silence, not a first-element report (probe `e1_generic_plus_plain`:
/// only the unused-local warning).
#[test]
fn generic_plus_plain_function_stays_silent() {
    let fixture = "// @dart=2.12\n\
                   void f<X>() {}\n\
                   void g() {}\n\
                   main() {\n\
                   \x20 [f, g];\n\
                   }\n";
    let report = report_ops_bare(fixture);
    assert!(
        infer_rows(&report, fixture).is_empty(),
        "rows: {:?}",
        rows(&report, fixture)
    );
}

/// `{f}` — a contextless SET literal stays silent: the set inferrer
/// carries no diagnostic reporter upstream (probe `e2_set`: only the
/// unused-local warning).
#[test]
fn set_literal_stays_silent() {
    let fixture = "// @dart=2.12\n\
                   void f<X>() {}\n\
                   main() {\n\
                   \x20 var x = {f};\n\
                   }\n";
    let report = report_ops_bare(fixture);
    assert!(
        infer_rows(&report, fixture).is_empty(),
        "rows: {:?}",
        rows(&report, fixture)
    );
}

/// `{f, f}` — the repeated-element set face (`equal_elements_in_set`)
/// is this fixture's only row family; the generic-metadata report stays
/// off (probe `e2_set_twice`).
#[test]
fn set_literal_twice_stays_silent() {
    let fixture = "// @dart=2.12\n\
                   void f<X>() {}\n\
                   main() {\n\
                   \x20 var x = {f, f};\n\
                   }\n";
    let report = report_ops_bare(fixture);
    assert!(
        infer_rows(&report, fixture).is_empty(),
        "rows: {:?}",
        rows(&report, fixture)
    );
}

/// `{1: f}` — a map VALUE candidate stays silent (probe `e2_map_value`).
#[test]
fn map_value_stays_silent() {
    let fixture = "// @dart=2.12\n\
                   void f<X>() {}\n\
                   main() {\n\
                   \x20 var x = {1: f};\n\
                   }\n";
    let report = report_ops_bare(fixture);
    assert!(
        infer_rows(&report, fixture).is_empty(),
        "rows: {:?}",
        rows(&report, fixture)
    );
}

/// `{f: 1}` — a map KEY candidate stays silent (probe `e2_map_key`).
#[test]
fn map_key_stays_silent() {
    let fixture = "// @dart=2.12\n\
                   void f<X>() {}\n\
                   main() {\n\
                   \x20 var x = {f: 1};\n\
                   }\n";
    let report = report_ops_bare(fixture);
    assert!(
        infer_rows(&report, fixture).is_empty(),
        "rows: {:?}",
        rows(&report, fixture)
    );
}

/// `// @dart=2.14` is the feature's own enable — the boundary's clean
/// side (probe `e4_214`: clean; the reporting side, `@dart=2.13`, is
/// pinned in #4335's flipped `dart_2_13_reports`).
#[test]
fn dart_2_14_stays_silent() {
    let fixture = "// @dart=2.14\n\
                   void f<X>() {}\n\
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
