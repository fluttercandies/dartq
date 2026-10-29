//! #2862 exact-row pins: annotation junk (`@meta`) before a declaration
//! name. The official class/mixin/extension-type and enum name templates
//! consume such junk in doubt and splice the zero-width synthetic name at
//! the FOLLOWER's offset (upstream Dart SDK source,
//! pkg/_fe_analyzer_shared/lib/src/parser/identifier_context_impl.dart,
//! branch 3 — `rewriter.insertSyntheticIdentifier(identifier)`), so every
//! downstream header face anchors on the junk identifier through the
//! zero-width-name `findNonZeroLengthToken` walk (util.dart:56) and the
//! `ensureBlock` follower fallback. Every expected row below is the 3.13.7
//! oracle face from the conformance corpus (Primary-constructors
//! grammar_A06_t01: `class @meta C(var int v);` 29|7|1 + 29|8|4,
//! `mixin class @meta M();` 33|13|1 + 33|14|4,
//! `enum @meta E(final int v) {` 37|6|1 + 37|7|4,
//! `extension type @meta ET(int _) {}` 45|16|1 + 45|17|4/4|0).

use super::collector::{latest_features, scan_ok};
use crate::ParserFeatures;

fn rows_with(source: &'static str, features: ParserFeatures) -> Vec<(String, u32, u32)> {
    let (table, text) = scan_ok(source);
    let mut collector = super::collector::Collector::default();
    let mut parser = crate::Parser::new(text, table, features, &mut collector);
    parser.parse_unit();
    parser.listener.errors.clone()
}

/// `extension type @meta ET(int _) {}` (oracle 45|16|1, 45|17|4, 45|17|4,
/// 45|17|0): the missing-name report stays on the `@`, while the
/// primary-constructor, representation-field and body faces ride the
/// zero-width name synthetic onto `meta`.
#[test]
fn extension_type_header_anchors_on_the_junk_identifier() {
    let rows = rows_with("extension type @meta ET(int _) {}\n", latest_features());
    assert!(
        rows.contains(&("missing_identifier".into(), 15, 16)),
        "missing_identifier on the `@`: {rows:?}"
    );
    assert!(
        rows.contains(&("missing_primary_constructor".into(), 16, 20)),
        "missing_primary_constructor on `meta`: {rows:?}"
    );
    assert!(
        rows.contains(&("expected_representation_field".into(), 16, 16)),
        "expected_representation_field zero-length at `meta`: {rows:?}"
    );
    assert!(
        rows.contains(&("expected_body".into(), 16, 20)),
        "expected_body on `meta`: {rows:?}"
    );
}

/// `class @meta C(var int v);` (oracle 29|7|1 + 29|8|4): the class body
/// face anchors on the junk identifier, not the `@`.
#[test]
fn class_header_anchors_on_the_junk_identifier() {
    let rows = rows_with("class @meta C(var int v);\n", latest_features());
    assert!(
        rows.contains(&("missing_identifier".into(), 6, 7)),
        "missing_identifier on the `@`: {rows:?}"
    );
    assert!(
        rows.contains(&("expected_body".into(), 7, 11)),
        "expected_body on `meta`: {rows:?}"
    );
}

/// `mixin class @meta M();` (oracle 33|13|1 + 33|14|4): same face through
/// the `mixin class` modifier walk.
#[test]
fn mixin_class_header_anchors_on_the_junk_identifier() {
    let rows = rows_with("mixin class @meta M();\n", latest_features());
    assert!(
        rows.contains(&("missing_identifier".into(), 12, 13)),
        "missing_identifier on the `@`: {rows:?}"
    );
    assert!(
        rows.contains(&("expected_body".into(), 13, 17)),
        "expected_body on `meta`: {rows:?}"
    );
}

/// `enum @meta E(final int v) { e0(0); }` (oracle 37|6|1 + 37|7|4): the
/// enumDeclaration template takes the same consume-in-doubt branch, so the
/// missing-body face anchors on the junk identifier.
#[test]
fn enum_header_anchors_on_the_junk_identifier() {
    let rows = rows_with(
        "enum @meta E(final int v) {\n  e0(0);\n}\n",
        latest_features(),
    );
    assert!(
        rows.contains(&("missing_identifier".into(), 5, 6)),
        "missing_identifier on the `@`: {rows:?}"
    );
    assert!(
        rows.contains(&("missing_enum_body".into(), 6, 10)),
        "missing_enum_body on `meta`: {rows:?}"
    );
}

/// Junk behind type parameters keeps the pinned face: `extension type<T> on
/// C<T> {}` (oracle Extension-methods syntax_t06 20|17|1 + 20|15|0) — the
/// `<` takes the insert-before keep-set branch, so `missing_primary_constructor`
/// stays on the closing `>` and the representation slot on the `<`.
#[test]
fn type_parameter_junk_keeps_the_insert_before_face() {
    let rows = rows_with("extension type<T> on C<T> {}\n", latest_features());
    assert!(
        rows.contains(&("missing_primary_constructor".into(), 16, 17)),
        "missing_primary_constructor on the `>`: {rows:?}"
    );
    assert!(
        rows.contains(&("expected_representation_field".into(), 14, 14)),
        "expected_representation_field on the `<`: {rows:?}"
    );
}
