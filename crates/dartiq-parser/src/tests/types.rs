//! Type annotation grammar tests.

use super::collector::setup;

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
