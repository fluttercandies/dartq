//! Type annotation grammar tests.

use super::collector::setup;
use crate::types::TypeInfo;

#[test]
fn simple_type() {
    let mut p = setup("int x");
    let end = p.ensure_type_not_void_entry();
    assert_eq!(p.cursor.lexeme(end.id), "int");
    assert_eq!(p.listener.log, [("handle", "type")]);
}

#[test]
fn nullable_type_consumes_question() {
    let mut p = setup("int? x");
    let end = p.ensure_type_not_void_entry();
    assert_eq!(p.cursor.lexeme(end.id), "?");
}

#[test]
fn nested_generics_split_gt_gt() {
    let mut p = setup("List<List<int>> x");
    let end = p.ensure_type_not_void_entry();
    // The split leading `>` is a parser-inserted token with a fixed lexeme.
    assert_eq!(p.cursor.lexeme(end.id), ">");
    assert!(p.listener.errors.is_empty(), "{:?}", p.listener.errors);
}

#[test]
fn void_type() {
    let mut p = setup("void");
    let end = p.parse_type_or_void_entry();
    assert_eq!(p.cursor.lexeme(end.id), "void");
}

#[test]
fn no_type_reports_expected_type_name() {
    let mut p = setup("+");
    let _ = p.ensure_type_not_void_entry();
    assert_eq!(
        p.listener.error_codes(),
        vec!["expected_type_name".to_string()]
    );
}

// --- #135: compound-closer fast path (official split-remainder rule) ----

/// Runs `compute_type` at the cursor and asserts it consumed nothing and
/// emitted no diagnostics (compute is a read-only analysis).
fn compute_shape(source: &'static str, required: bool) -> TypeInfo {
    let mut p = setup(source);
    let before = p.cursor.progress();
    let info = p.compute_type(required, false);
    assert!(
        p.listener.errors.is_empty(),
        "{source:?}: unexpected diagnostics: {:?}",
        p.listener.errors
    );
    assert_eq!(
        p.cursor.progress(),
        before,
        "{source:?}: compute_type must not consume"
    );
    info
}

#[test]
fn compound_closer_required_keeps_fast_shape() {
    // `A<B>>` (scanner-coalesced). Oracle: `simpleTypeWith1ArgumentGtGt` —
    // required position returns the fast shape regardless of what follows,
    // because the split remainder `>` blocks neither the shape nor
    // recovery (type_info.dart:283-286, type_info_impl.dart:1293).
    let info = compute_shape("A<B>> x", true);
    assert!(matches!(info, TypeInfo::SimpleWith1Arg { nullable: false }));
}

#[test]
fn compound_closer_optional_is_no_type() {
    // Oracle: noType. The decision token is the split remainder `=` of
    // `>=` (splitGtEq, type_info_impl.dart:1263), which is never a name —
    // the optional-position `looksLikeName` check cannot pass
    // (type_info.dart:270-292).
    let info = compute_shape("A<B>=c d", false);
    assert!(matches!(info, TypeInfo::NoType));
}

#[test]
fn compound_closer_question_suffix_not_nullable() {
    // Oracle: non-nullable fast shape. `simpleNullableTypeWith1Argument`
    // requires a plain `>` closer (type_info.dart:283); the remainder `>`
    // of `>>` is never a `?`, so the `?` after the compound token stays
    // outside the type.
    let info = compute_shape("A<B>>? x", true);
    assert!(matches!(info, TypeInfo::SimpleWith1Arg { nullable: false }));
}

#[test]
fn plain_closer_question_suffix_nullable() {
    // Regression pin: `A<B>?` keeps the nullable fast shape when a name
    // follows (type_info.dart:277-281, simpleNullableTypeWith1Argument).
    let info = compute_shape("A<B>? c", false);
    assert!(matches!(info, TypeInfo::SimpleWith1Arg { nullable: true }));
}

#[test]
fn builtin_recovery_compound_closer_optional_is_no_type() {
    // Built-in identifier (`required`) + `id<id>=`. Oracle: noType — the
    // recovery `looksLikeName(skip(next).next!)` reads the split remainder
    // `=`, which is never a name (type_info.dart:173 +
    // type_info_impl.dart:1263), so the recovered complex shape is not
    // entered in optional position.
    let info = compute_shape("required<B>=c d", false);
    assert!(matches!(info, TypeInfo::NoType));
}
