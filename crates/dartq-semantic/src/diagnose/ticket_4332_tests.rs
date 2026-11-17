//! Ticket #4332 pins: `strict-inference`'s
//! `inference_failure_on_instance_creation` row — the creation-solver's
//! unconstrained fallback (a constructible generic head invoked with
//! implicit type arguments whose parameters neither a context nor an
//! argument constrained) reports once, at the written constructor-name
//! chain, keyed by the same spelling (upstream Dart SDK source,
//! pkg/analyzer/lib/src/dart/element/generic_inferrer.dart:411-421 and
//! `_reportInferenceFailure`'s ConstructorName arm :597-609).
//!
//! * The bare statement creation, the top-level initializer, the
//!   named-selector chain, the prefixed head, and the extension-type
//!   head all report; the metadata spelling anchors the whole `@…(…)`
//!   node (the Annotation arm :610-624).
//! * Explicit type arguments, a downward context, an upward-binding
//!   formal (`C.of(Map<T, T>)` binds what the raw `Map` of `.named`
//!   cannot), `@optionalTypeArgs`, a pre-generic-metadata library, and
//!   a type-alias head (its parameters fixed at the alias's own
//!   declaration — no call-site inference runs) all stay silent, and
//!   the non-strict posture stays silent everywhere.

#![allow(clippy::semicolon_if_nothing_returned)]
use super::test_rig::{
    CORE_PRELUDE, RigImport, report_ops, report_ops_bare_strict, report_ops_imports_strict,
    report_ops_strict,
};
use super::*;

const STRICT: StrictModes = StrictModes {
    casts: false,
    inference: true,
    raw_types: false,
};

fn at(fixture: &str, needle: &str) -> usize {
    CORE_PRELUDE.len() + fixture.find(needle).unwrap()
}

fn message(constructor: &str) -> String {
    format!("The type argument(s) of the constructor '{constructor}' can't be inferred.")
}

fn rows(report: &FileReport) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .map(|d| {
            format!(
                "{} @ {}:{} {}",
                d.code.name, d.range.offset, d.range.length, d.message
            )
        })
        .collect()
}

/// Exactly one inference_failure row of `len` at the `needle` spelling,
/// carrying the constructor-name message.
#[track_caller]
fn assert_row(report: &FileReport, fixture: &str, needle: &str, len: usize, ctor: &str) {
    let offset = at(fixture, needle);
    let hits: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| {
            d.code.name == "inference_failure_on_instance_creation"
                && d.range.offset as usize == offset
                && d.range.length as usize == len
        })
        .collect();
    assert_eq!(
        hits.len(),
        1,
        "expected one row at {needle} ({offset}:{len}); got {:#?}",
        rows(report)
    );
    assert_eq!(
        hits[0].message,
        message(ctor).as_str(),
        "{:#?}",
        rows(report)
    );
}

/// No inference_failure row anywhere.
#[track_caller]
fn assert_none(report: &FileReport) {
    assert!(
        report
            .diagnostics
            .iter()
            .all(|d| d.code.name != "inference_failure_on_instance_creation"),
        "expected no row; got {:#?}",
        rows(report)
    );
}

/// The bare statement creation reports at the class head.
#[test]
fn bare_statement_creation_reports() {
    let fixture = "class C<T> {}\nvoid f() { C(); }\n";
    let report = report_ops_strict(fixture, STRICT);
    assert_row(&report, fixture, "C();", 1, "C");
}

/// The official default posture (all strictness modes off) stays silent.
#[test]
fn non_strict_stays_silent() {
    let report = report_ops("class C<T> {}\nvoid f() { C(); }\n");
    assert_none(&report);
}

/// A written `<...>` group outranks inference — the explicit face owns
/// the instantiation.
#[test]
fn explicit_type_arguments_stay_silent() {
    let report = report_ops_strict("class C<T> {}\nvoid f() { C<int>(); }\n", STRICT);
    assert_none(&report);
}

/// A named-selector chain whose formal does not mention a parameter
/// (the raw `Map` of the `.named` shape) reports the whole chain
/// spelling — the `HashMap.from` corpus face.
#[test]
fn named_selector_raw_formal_reports_chain() {
    let fixture = "class C<T> { C.named(Map m); }\nvoid f() { C.named({1: 1}); }\n";
    let report = report_ops_strict(fixture, STRICT);
    assert_row(&report, fixture, "C.named({1: 1});", 7, "C.named");
}

/// A named-selector chain whose formal binds the parameter
/// (the `Map<T, T>` of the `.of` shape) stays silent — the upward face.
#[test]
fn named_selector_binding_formal_stays_silent() {
    let report = report_ops_strict(
        "class C<T> { C.of(Map<T, T> m); }\nvoid f() { C.of({1: 1}); }\n",
        STRICT,
    );
    assert_none(&report);
}

/// The same named-selector upward face over a bare-parameter formal —
/// the bind the `.of` shape's structural walk generalizes.
#[test]
fn named_selector_bare_param_formal_stays_silent() {
    let report = report_ops_strict("class C<T> { C.n(T x); }\nvoid f() { C.n(1); }\n", STRICT);
    assert_none(&report);
}

/// The unnamed face's structural bind over a map-literal argument —
/// the literal's own leaf-LUB type feeds the unify exactly like the
/// `.of` factory's `Map<K, V>` formal does.
#[test]
fn unnamed_map_literal_formal_stays_silent() {
    let report = report_ops_strict(
        "class C<T> { C(Map<T, T> m); }\nvoid f() { C({1: 1}); }\n",
        STRICT,
    );
    assert_none(&report);
}

/// The named face of the same bind — the label's colon trims to the
/// value's own window (`m: {1: 1}`).
#[test]
fn named_map_literal_formal_stays_silent() {
    let report = report_ops_strict(
        "class C<T> { C({required Map<T, T> m}); }\nvoid f() { C(m: {1: 1}); }\n",
        STRICT,
    );
    assert_none(&report);
}

/// A list-literal argument binds through its own single-row window
/// (its element rows drain at its close, so the `]`-row is the whole
/// argument face).
#[test]
fn list_literal_formal_stays_silent() {
    let report = report_ops_strict(
        "class C<T> { C(List<T> l); }\nvoid f() { C([1, 2]); }\n",
        STRICT,
    );
    assert_none(&report);
}

/// The prefixed head reports the qualified spelling (`c.HashMap`).
#[test]
fn prefixed_unnamed_reports_qualified_chain() {
    let fixture = "void f() { p.H(); }\n";
    let imports = [RigImport {
        uri: "package:t/h.dart",
        prefix: Some("p"),
        source: "class H<K, V> {}",
    }];
    let report = report_ops_imports_strict(fixture, &imports, STRICT);
    assert_row(&report, fixture, "p.H();", 3, "p.H");
}

/// An extension-type head is a constructible head (`E` of
/// `extension type E<T>(int i)` — the `int` formal binds nothing).
#[test]
fn extension_type_head_reports() {
    let fixture = "extension type E<T>(int i) {}\nvoid f() { E(1); }\n";
    let report = report_ops_strict(fixture, STRICT);
    assert_row(&report, fixture, "E(1);", 1, "E");
}

/// A top-level variable's initializer has no written context — the
/// creation reports (the corpus's `var m = HashMap();` face).
#[test]
fn top_level_initializer_reports() {
    let fixture = "class C<T> {}\nvar m = C();\n";
    let report = report_ops_strict(fixture, STRICT);
    assert_row(&report, fixture, "C();", 1, "C");
}

/// A return context that names the head constrains the parameters —
/// the downward face.
#[test]
fn downward_context_stays_silent() {
    let report = report_ops_strict("class C<T> {}\nC<int> g() { return C(); }\n", STRICT);
    assert_none(&report);
}

/// An argument whose formal mentions the parameter binds it — the
/// upward face over the unnamed constructor.
#[test]
fn upward_argument_stays_silent() {
    let report = report_ops_strict("class C<T> { C(T x); }\nvoid f() { C(1); }\n", STRICT);
    assert_none(&report);
}

/// The metadata spelling anchors the WHOLE annotation node (`@` through
/// the group's `)`) and reports only under generic-metadata — the
/// Annotation arm's own anchor.
#[test]
fn annotation_reports_whole_node() {
    let fixture = "class C<T> { const C(); }\n\n@C()\nvoid f() {}\n";
    let report = report_ops_strict(fixture, STRICT);
    assert_row(&report, fixture, "@C()", 4, "C");
}

/// A pre-generic-metadata library (`// @dart=2.12`, the package
/// language version the feature fold reads) keeps the annotation
/// spelling silent.
#[test]
fn annotation_pre_generic_metadata_stays_silent() {
    let report = report_ops_bare_strict(
        "// @dart=2.12\nclass C<T> { const C(); }\n\n@C()\nvoid f() {}\n",
        STRICT,
    );
    assert_none(&report);
}

/// `@optionalTypeArgs` on the declaring head exempts the creation.
#[test]
fn optional_type_args_head_stays_silent() {
    let report = report_ops_strict(
        "@optionalTypeArgs\nclass C<T> {}\nvoid f() { C(); }\n",
        STRICT,
    );
    assert_none(&report);
}

/// A type-alias head runs no call-site inference — a non-generic alias
/// fixes its parameters at its own declaration (the folded row shape
/// included; the corpus's `interfaceTypeTypedef_noInference` face).
#[test]
fn alias_head_stays_silent() {
    let report = report_ops_strict("class C<T> {}\ntypedef A = C;\nvoid f() { A(); }\n", STRICT);
    assert_none(&report);
}
