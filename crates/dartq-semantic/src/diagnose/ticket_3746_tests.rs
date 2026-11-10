//! #3746 — the census-16:19 emission-loss regression pins.
//!
//! The 2026-11-03 16:19 census reported 96 `augmentation_without_declaration`
//! rows and 48 `undefined_identifier` rows missing against the oracle while
//! the same case ids passed at 13:31. Every missing row reproduces green at
//! this tree — through `dartq analyze` on the exact materialized fixtures,
//! through `conformance run --filter` end to end, and through the unit rig
//! below. The loss was not a code-path defect at HEAD: the census evidence
//! pattern (emitted rows absent yet zero `extra`, contiguous case-id
//! windows, `augment mixin` passing where `augment class` failed) matches a
//! chunk-crash → bisect → dropped-attribution cascade in the harness — a
//! dartq `discovery failed` mid-census (a concurrent lane mutating the
//! shared `chunks/run` dir under a live multi-lane tree) forced bisection,
//! and rows emitted for case roots outside the bisect half's index set were
//! counted-and-dropped rather than attributed. These pins lock the census's
//! own failing shapes in both directions so a semantic-tier regression of
//! the same signature cannot land silently.
//!
//! The failing augmentation cluster was `augment <container>` over the four
//! container kinds (class/enum/extension/extension-type — `augment mixin`
//! passed because it landed outside the bad window); each case carries a
//! member augmentation whose own `augment` keyword anchors a second row.
//! The `undefined_identifier` cluster was `ToSourceVisitorTest` shapes: a
//! bare top-level reference, a set/map literal's collection-element reads,
//! and a legacy switch's scrutinee/case reads.
//!
//! Oracle provenance: `~/.dartq-oracle/3.13.7/dart-sdk/bin/dart analyze` —
//! each pin asserts the oracle's exact (code, offset, length, message).

use super::test_rig::{CORE_PRELUDE, report_ops, report_ops_augment, rows};
use super::*;

/// The absolute fixture offset of `needle` (the rig's prelude included).
fn at(fixture: &str, needle: &str, after: usize) -> usize {
    CORE_PRELUDE.len() + fixture[after..].find(needle).unwrap() + after
}

/// Exactly `count` rows of `name` in `report`.
#[track_caller]
fn assert_code_count(report: &FileReport, name: &str, count: usize) {
    let hits = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == name)
        .count();
    assert_eq!(
        hits,
        count,
        "expected {count} {name} rows; got {:#?}",
        rows(report)
    );
}

/// One row of `name` anchored at `offset:len` carrying `message`.
#[track_caller]
fn assert_row(report: &FileReport, name: &str, offset: usize, len: u32, message: &str) {
    let hits: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| {
            d.code.name == name && d.range.offset as usize == offset && d.range.length == len
        })
        .collect();
    assert_eq!(
        hits.len(),
        1,
        "expected one {name} @ {offset}:{len}; got {:#?}",
        rows(report)
    );
    assert_eq!(hits[0].message, message, "{:#?}", rows(report));
}

/// The class-container census shape: `augment class A { augment <member> }`
/// reports once at the container's `augment` and once at the member's.
#[test]
fn augment_class_member_reports_twice() {
    let src = "\naugment class A {\n  augment int get foo => 0;\n}\n";
    let report = report_ops(src);
    assert_row(
        &report,
        "augmentation_without_declaration",
        at(src, "augment", 0),
        7,
        "The declaration being augmented doesn't exist.",
    );
    assert_row(
        &report,
        "augmentation_without_declaration",
        at(src, "augment int", 0),
        7,
        "The declaration being augmented doesn't exist.",
    );
    assert_code_count(&report, "augmentation_without_declaration", 2);
}

/// The enum-container census shape (the `;` after `{` closes the constant
/// list so the member augmentation parses as a member).
#[test]
fn augment_enum_member_reports_twice() {
    let src = "\naugment enum E {;\n  augment int get foo => 0;\n}\n";
    let report = report_ops(src);
    assert_row(
        &report,
        "augmentation_without_declaration",
        at(src, "augment", 0),
        7,
        "The declaration being augmented doesn't exist.",
    );
    assert_row(
        &report,
        "augmentation_without_declaration",
        at(src, "augment int", 0),
        7,
        "The declaration being augmented doesn't exist.",
    );
    assert_code_count(&report, "augmentation_without_declaration", 2);
}

/// The extension-container census shape.
#[test]
fn augment_extension_member_reports_twice() {
    let src = "\naugment extension E {\n  augment int get foo => 0;\n}\n";
    let report = report_ops(src);
    assert_row(
        &report,
        "augmentation_without_declaration",
        at(src, "augment", 0),
        7,
        "The declaration being augmented doesn't exist.",
    );
    assert_row(
        &report,
        "augmentation_without_declaration",
        at(src, "augment int", 0),
        7,
        "The declaration being augmented doesn't exist.",
    );
    assert_code_count(&report, "augmentation_without_declaration", 2);
}

/// The extension-type-container census shape (`augment extension type E`
/// reports once at the container's `augment` keyword — the `implements`
/// clause's own row rides alongside and does not count here).
#[test]
fn augment_extension_type_reports() {
    let src = "\naugment extension type E implements I {}\n";
    let report = report_ops(src);
    assert_row(
        &report,
        "augmentation_without_declaration",
        at(src, "augment", 0),
        7,
        "The declaration being augmented doesn't exist.",
    );
    assert_code_count(&report, "augmentation_without_declaration", 1);
}

/// The mixin shape that passed the broken window — pinned so a widening
/// regression can't move it either.
#[test]
fn augment_mixin_member_reports_twice() {
    let src = "\naugment mixin M {\n  augment int get foo => 0;\n}\n";
    let report = report_ops(src);
    assert_row(
        &report,
        "augmentation_without_declaration",
        at(src, "augment", 0),
        7,
        "The declaration being augmented doesn't exist.",
    );
    assert_row(
        &report,
        "augmentation_without_declaration",
        at(src, "augment int", 0),
        7,
        "The declaration being augmented doesn't exist.",
    );
    assert_code_count(&report, "augmentation_without_declaration", 2);
}

/// The silent direction: an `augment` declaration that resolves against a
/// preceding same-kind fragment reports nothing (the two-unit library rig —
/// the augmentation merges, the without-declaration family stays off).
#[test]
fn augment_with_base_declaration_stays_silent() {
    let report = report_ops_augment("class A {}\n", "augment class A {}\n");
    let family = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "augmentation_without_declaration")
        .count();
    assert_eq!(family, 0, "{:#?}", rows(&report));
}

/// The bare top-level reference (the ToSourceVisitor `SimpleIdentifier`
/// case): `foo` is undefined — the row anchors at the name.
#[test]
fn undefined_identifier_toplevel_reference() {
    let src = "\nvar x = foo;\n";
    let report = report_ops(src);
    assert_row(
        &report,
        "undefined_identifier",
        at(src, "foo", 0),
        3,
        "Undefined name 'foo'.",
    );
}

/// The set/map literal census shape: the collection-for loop variable and
/// the `if` condition read undefined names — three rows inside one literal.
#[test]
fn undefined_identifier_inside_map_literal() {
    let src = "\nfinal v = <String, String>{'a' : 'b', for (c in d) 'e' : 'f', if (g) 'h' : 'i', ...{'j' : 'k'}};\n";
    let report = report_ops(src);
    assert_row(
        &report,
        "undefined_identifier",
        at(src, "c in d", 0),
        1,
        "Undefined name 'c'.",
    );
    assert_row(
        &report,
        "undefined_identifier",
        at(src, "d)", 0),
        1,
        "Undefined name 'd'.",
    );
    assert_row(
        &report,
        "undefined_identifier",
        at(src, "g)", 0),
        1,
        "Undefined name 'g'.",
    );
    assert_code_count(&report, "undefined_identifier", 3);
}

/// The switch census shape: the scrutinee `x` and the case constant `a`
/// each read undefined names. (The census case carried a `// @dart = 2.19`
/// marker, but under `report_ops` the prelude precedes it — the marker
/// lands mid-file and is inert, so the pin analyzes at the latest version;
/// a legacy-version pin would need the bare rig.)
#[test]
fn undefined_identifier_in_switch_scrutinee_and_case() {
    let src = "\n// @dart = 2.19\nvoid f() {\n  switch (x) {\n    case a: {}\n  }\n}\n";
    let report = report_ops(src);
    assert_row(
        &report,
        "undefined_identifier",
        at(src, "x)", 0),
        1,
        "Undefined name 'x'.",
    );
    assert_row(
        &report,
        "undefined_identifier",
        at(src, "a:", 0),
        1,
        "Undefined name 'a'.",
    );
}
