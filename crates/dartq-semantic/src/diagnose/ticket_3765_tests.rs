//! The #3765 `class_used_as_mixin` pins: the with-clause entry face after
//! `finalize_mixin_uses` learned the alias fold and the resolved-element
//! non-subtypable gate.
//!
//! Oracle provenance: `dart analyze` 3.13.7 over the /tmp/t3765 battery
//! (bare/generic/chained/nullable/prefixed alias spellings, `dart:core`
//! and `dart:async` restricted heads, mixin and mixin-class targets,
//! self-referencing and function-shaped right-hand sides, and a
//! user-declared shadow of a restricted spelling).
//!
//! Upstream decision order (`_checkForAllMixinErrorCodes`,
//! error_verifier.dart:2863-2946): the entry's TYPE resolves first — a
//! type alias instantiates to its right-hand element before any face
//! reads it. A non-subtypable `dart:*` element routes to the disallowed
//! arm (this engine's `subtype_of_disallowed_type` face) and never
//! reaches `classUsedAsMixin` — except `Enum`, whose `isDartCoreEnum`
//! conjunct falls through to the class arm. A plain `ClassElement`
//! (neither `mixin` nor `mixin class`) whose own library carries class
//! modifiers reports `classUsedAsMixin` naming the ELEMENT, not the
//! written alias.
//!
//! Known sibling-face gap (outside this face's scope): a prefixed alias
//! spelling (`with p.B`) also draws `mixin_of_non_class` from the
//! clause-legality walk in `overrides.rs`, whose prefixed arm does not
//! fold aliases — oracle reports `class_used_as_mixin` alone. The pins
//! assert this face's own rows and leave that row to its owner lane.

use crate::diagnose::test_rig::{
    CORE_PRELUDE, RigImport, assert_silent, report_ops, report_ops_imports, report_ops_with, rows,
};

/// The absolute fixture offset of `needle` (the prelude the rig prepends
/// included; the fixtures are ASCII, so byte offset == UTF-16 offset).
fn at(fixture: &str, needle: &str) -> u32 {
    (CORE_PRELUDE.len() + fixture.find(needle).unwrap()) as u32
}

/// `class_used_as_mixin` rows of `report` as `(offset, length, message)`
/// — the triple the oracle's machine rows compare against.
fn mixin_use_rows(report: &crate::diagnose::FileReport) -> Vec<(u32, u32, String)> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "class_used_as_mixin")
        .map(|d| (d.range.offset, d.range.length, d.message.to_string()))
        .collect()
}

/// One diagnostic's `(code_name, offset, length)` triple over the whole
/// report — the row-set assertions for the suppression faces.
fn code_spans(report: &crate::diagnose::FileReport) -> Vec<(&'static str, u32, u32)> {
    let mut spans: Vec<(&'static str, u32, u32)> = report
        .diagnostics
        .iter()
        .map(|d| (d.code.name, d.range.offset, d.range.length))
        .collect();
    spans.sort_by_key(|(code, offset, _)| (*offset, *code));
    spans
}

fn mixin_msg(name: &str) -> String {
    format!("The class '{name}' can't be used as a mixin because it's neither a mixin class nor a mixin.")
}

// —— F1: an alias to a plain class reports the folded element ————————————

/// `with A` over `typedef A = C` reports at `A` naming `C` (oracle
/// /tmp/t3765/f1.dart: `CLASS_USED_AS_MIXIN |3|29|1| 'C'`).
#[test]
fn bare_alias_to_class_reports_element_name() {
    let fixture = "class C {}\ntypedef A = C;\nclass D with A {}\n";
    let report = report_ops(fixture);
    assert_eq!(
        mixin_use_rows(&report),
        vec![(at(fixture, "with A") + 5, 1, mixin_msg("C"))],
        "rows: {:?}",
        rows(&report)
    );
}

/// A generic alias instantiation folds the same way — `with A<int>` over
/// `typedef A<T> = C<T>` reports the whole `A<int>` span naming `C`
/// (oracle /tmp/t3765/f1c.dart: `|3|29|6| 'C'`).
#[test]
fn generic_alias_to_class_reports_element_name() {
    let fixture = "class C<T> {}\ntypedef A<T> = C<T>;\nclass D with A<int> {}\n";
    let report = report_ops(fixture);
    assert_eq!(
        mixin_use_rows(&report),
        vec![(at(fixture, "with A<int>") + 5, 6, mixin_msg("C"))],
        "rows: {:?}",
        rows(&report)
    );
}

/// An alias whose right-hand side carries arguments folds to the head —
/// `typedef A = C<int>` reports `C` (oracle /tmp/t3765/f1b.dart).
#[test]
fn alias_rhs_arguments_fold_to_head() {
    let fixture = "class C<T> {}\ntypedef A = C<int>;\nclass D with A {}\n";
    let report = report_ops(fixture);
    assert_eq!(
        mixin_use_rows(&report),
        vec![(at(fixture, "with A") + 5, 1, mixin_msg("C"))],
        "rows: {:?}",
        rows(&report)
    );
}

/// A co-alias chain (`A = B`, `B = C`) folds to the terminal container —
/// the row names `C` (oracle /tmp/t3765/f1d.dart: `|4|29|1| 'C'`).
#[test]
fn chained_alias_reports_terminal_element() {
    let fixture = "typedef A = B;\ntypedef B = C;\nclass C {}\nclass D with A {}\n";
    let report = report_ops(fixture);
    assert_eq!(
        mixin_use_rows(&report),
        vec![(at(fixture, "with A") + 5, 1, mixin_msg("C"))],
        "rows: {:?}",
        rows(&report)
    );
}

/// A nullable right-hand side still reports the element —
/// `typedef A = C?` draws `class_used_as_mixin 'C'` beside the
/// nullable-clause face (oracle /tmp/t3765/f_cq.dart).
#[test]
fn nullable_rhs_alias_reports_element() {
    let fixture = "class C {}\ntypedef A = C?;\nclass D with A {}\n";
    let report = report_ops(fixture);
    assert_eq!(
        mixin_use_rows(&report),
        vec![(at(fixture, "with A") + 5, 1, mixin_msg("C"))],
        "rows: {:?}",
        rows(&report)
    );
}

/// An imported alias folds through its own library's scope —
/// `with B` over `other.dart`'s `typedef B = C` names `C`.
#[test]
fn imported_alias_reports_element() {
    let fixture = "import 'package:t/other.dart';\nclass D with B {}\n";
    let report = report_ops_imports(
        fixture,
        &[RigImport {
            uri: "package:t/other.dart",
            prefix: None,
            source: "class C {}\ntypedef B = C;\n",
        }],
    );
    assert_eq!(
        mixin_use_rows(&report),
        vec![(at(fixture, "with B") + 5, 1, mixin_msg("C"))],
        "rows: {:?}",
        rows(&report)
    );
}

/// A prefixed alias folds its leaf — `with p.B` names the folded `C` at
/// the whole `p.B` span (oracle /tmp/t3765/f1p.dart: `|2|29|3| 'C'`).
/// The `mixin_of_non_class` row the same span draws is the sibling-face
/// gap documented in the module header.
#[test]
fn prefixed_alias_reports_folded_element() {
    let fixture = "import 'package:t/other.dart' as p;\nclass D with p.B {}\n";
    let report = report_ops_imports(
        fixture,
        &[RigImport {
            uri: "package:t/other.dart",
            prefix: Some("p"),
            source: "class C {}\ntypedef B = C;\n",
        }],
    );
    assert_eq!(
        mixin_use_rows(&report),
        vec![(at(fixture, "with p.B") + 5, 3, mixin_msg("C"))],
        "rows: {:?}",
        rows(&report)
    );
}

// —— F3: an alias to a mixin or mixin class stays silent —————————————————

/// `typedef A = M` over a `mixin class` — the folded element is legal, no
/// row (oracle /tmp/t3765/f3.dart: silent).
#[test]
fn alias_to_mixin_class_stays_silent() {
    let fixture = "mixin class M {}\ntypedef A = M;\nclass D with A {}\n";
    let report = report_ops(fixture);
    assert_silent(&report);
}

/// `typedef A = M` over a bare `mixin` — silent (oracle /tmp/t3765/f3b).
#[test]
fn alias_to_mixin_stays_silent() {
    let fixture = "mixin M {}\ntypedef A = M;\nclass D with A {}\n";
    let report = report_ops(fixture);
    assert_silent(&report);
}

/// A generic alias to a mixin class — `with A<int>` silent (oracle
/// /tmp/t3765/f3c.dart).
#[test]
fn generic_alias_to_mixin_class_stays_silent() {
    let fixture = "mixin class M<T> {}\ntypedef A<T> = M<T>;\nclass D with A<int> {}\n";
    let report = report_ops(fixture);
    assert_silent(&report);
}

/// A chained alias to a mixin — `A = B`, `B = M` silent (oracle
/// /tmp/t3765/f3e.dart).
#[test]
fn chained_alias_to_mixin_stays_silent() {
    let fixture = "mixin M {}\ntypedef A = M;\ntypedef B = A;\nclass D with B {}\n";
    let report = report_ops(fixture);
    assert_silent(&report);
}

/// A function-shaped right-hand side is not this face's element — the
/// fold stands down (upstream: `mixinType` is not an `InterfaceType`;
/// `mixin_of_non_class` owns the row).
#[test]
fn function_alias_reports_no_class_used_as_mixin() {
    let fixture = "typedef F = void Function();\nclass D with F {}\n";
    let report = report_ops(fixture);
    assert!(
        mixin_use_rows(&report).is_empty(),
        "rows: {:?}",
        rows(&report)
    );
}

/// A self-referencing alias never unfolds — no `class_used_as_mixin`
/// (oracle /tmp/t3765/f_self.dart: the self-reference face plus
/// `mixin_of_non_class`, never this row).
#[test]
fn self_referencing_alias_reports_no_class_used_as_mixin() {
    let fixture = "typedef A = A;\nclass D with A {}\n";
    let report = report_ops(fixture);
    assert!(
        mixin_use_rows(&report).is_empty(),
        "rows: {:?}",
        rows(&report)
    );
}

/// A genuine mixin use — the face's negative control (oracle
/// /tmp/t3765/neg.dart: silent).
#[test]
fn genuine_mixin_stays_silent() {
    let fixture = "mixin M {}\nclass D with M {}\n";
    let report = report_ops(fixture);
    assert_silent(&report);
}

// —— F2: a non-subtypable `dart:*` element routes to the disallowed arm ——

/// A `dart:core`-mounted restricted element: `with int` reports
/// `subtype_of_disallowed_type` and NEVER `class_used_as_mixin` — the
/// upstream `_checkForExtendsOrImplementsDisallowedClass` arm claims the
/// element first (oracle /tmp/t3765/f2.dart).
#[test]
fn dart_core_restricted_element_suppresses_mixin_use() {
    let fixture = "class D with int {}\n";
    let report = report_ops_with(
        fixture,
        &[RigImport {
            uri: "dart:core",
            prefix: None,
            source: "class int {}\n",
        }],
        None,
    );
    assert_eq!(
        code_spans(&report),
        vec![("subtype_of_disallowed_type", 13_u32, 3_u32)],
        "rows: {:?}",
        rows(&report)
    );
}

/// The same gate over a parameterized spelling — `with FutureOr<int>`
/// over a `dart:async`-mounted `FutureOr` reports the disallowed face
/// and never `class_used_as_mixin` (oracle /tmp/t3765/f_fto.dart). The
/// `mixin_of_non_class` row the span also draws is the clause-legality
/// walk's own gap (it does not read the disallowed arm), outside this
/// face's scope.
#[test]
fn dart_async_restricted_element_suppresses_mixin_use() {
    let fixture = "class D with FutureOr<int> {}\n";
    let report = report_ops_with(
        fixture,
        &[RigImport {
            uri: "dart:async",
            prefix: None,
            source: "class FutureOr<T> {}\nclass int {}\n",
        }],
        None,
    );
    assert_eq!(
        code_spans(&report)
            .into_iter()
            .filter(|(code, _, _)| {
                matches!(*code, "subtype_of_disallowed_type" | "class_used_as_mixin")
            })
            .collect::<Vec<_>>(),
        vec![("subtype_of_disallowed_type", 13_u32, 13_u32)],
        "rows: {:?}",
        rows(&report)
    );
}

/// The gate keys on the RESOLVED element's library, not the spelling: a
/// user-declared `String` (the prelude's) used as a mixin is a plain
/// class — `class_used_as_mixin` still reports (oracle
/// /tmp/t3765/f_shadow.dart: `CLASS_USED_AS_MIXIN 'String'`).
#[test]
fn user_shadow_of_restricted_spelling_still_reports() {
    let fixture = "class D with String {}\n";
    let report = report_ops(fixture);
    assert_eq!(
        mixin_use_rows(&report),
        vec![(at(fixture, "with String") + 5, 6, mixin_msg("String"))],
        "rows: {:?}",
        rows(&report)
    );
}
