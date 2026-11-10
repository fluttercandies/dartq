//! #3694 — an unannotated declaration whose initializer's static type is
//! `Null` declares `dynamic`, never `Null`. Upstream Dart SDK source:
//! `pkg/analyzer/lib/src/generated/resolver.dart`
//! `variableTypeFromInitializerType` (locals, applied by
//! `variable_declaration_resolver.dart`) and
//! `pkg/analyzer/lib/src/summary2/top_level_inference.dart` `_refineType`
//! (top-level variables and fields).
//!
//! Oracle shapes: `InferredTypeTest/test_dontInferTypeWhenInitializerIsNull#0`
//! (a local), `InferredTypeTest/test_dontInferFieldTypeWhenInitializerIsNull#0`
//! (a top-level variable, a static and an instance field) and
//! `nnbd/inference/variables_initialized_to_null_test` (`null as Null`).
//! The rule refines the DECLARATION's type: a written `Null` annotation keeps
//! `Null`, and a non-`Null` initializer keeps its own type.

use super::test_rig::{assert_silent, code_names, report_ops, rows};

/// The `invalid_assignment` rows of `source`, `code @offset: message` each.
fn invalid_assignments(source: &str) -> Vec<String> {
    rows(&report_ops(source))
        .into_iter()
        .filter(|row| row.starts_with("invalid_assignment "))
        .collect()
}

/// The main shape: a `null`-initialized local is `dynamic`, so a `String`
/// write into it is silent.
#[test]
fn null_initialized_local_accepts_any_write() {
    assert_silent(&report_ops(
        "Object? f() {\n\
           var x = null;\n\
           x = \"hi\";\n\
           return x;\n\
         }\n",
    ));
}

/// `test_dontInferTypeWhenInitializerIsNull#0` verbatim: the oracle's only
/// row is the write-only local's `unused_local_variable`.
#[test]
fn null_initialized_local_oracle_shape() {
    let report = report_ops(
        "test() {\n\
           var x = null;\n\
           x = \"hi\";\n\
           x = 3;\n\
         }\n",
    );
    assert_eq!(
        code_names(&report),
        vec!["unused_local_variable"],
        "{:?}",
        rows(&report)
    );
}

/// The refinement is the declaration's type, not a write tolerance: a read
/// of the `dynamic` local into a `String` slot is silent too.
#[test]
fn null_initialized_local_reads_as_dynamic() {
    assert_silent(&report_ops(
        "String f() {\n\
           var x = null;\n\
           String y = x;\n\
           return y;\n\
         }\n",
    ));
}

/// The field form: an instance field initialized to `null` is `dynamic`.
#[test]
fn null_initialized_field_accepts_any_write() {
    assert_silent(&report_ops(
        "class C {\n\
           var f = null;\n\
         }\n\
         void g(C c) {\n\
           c.f = \"hi\";\n\
         }\n",
    ));
}

/// The top-level form: a top-level variable initialized to `null` is
/// `dynamic`.
#[test]
fn null_initialized_top_level_accepts_any_write() {
    assert_silent(&report_ops(
        "var x = null;\n\
         void g() {\n\
           x = \"hi\";\n\
         }\n",
    ));
}

/// `test_dontInferFieldTypeWhenInitializerIsNull#0` verbatim: only the
/// three `int`-inferred twins report; the `null`-initialized top-level
/// variable, static field and instance field accept the `String` writes.
#[test]
fn null_initialized_model_slots_oracle_shape() {
    let found = invalid_assignments(
        "var x = null;\n\
         var y = 3;\n\
         class A {\n\
           static var x = null;\n\
           static var y = 3;\n\
           var x2 = null;\n\
           var y2 = 3;\n\
         }\n\
         test() {\n\
           x = \"hi\";\n\
           y = \"hi\";\n\
           A.x = \"hi\";\n\
           A.y = \"hi\";\n\
           new A().x2 = \"hi\";\n\
           new A().y2 = \"hi\";\n\
         }\n",
    );
    assert_eq!(found.len(), 3, "{found:?}");
    assert!(
        found.iter().all(|row| row
            .ends_with("A value of type 'String' can't be assigned to a variable of type 'int'.")),
        "{found:?}"
    );
}

/// `variables_initialized_to_null_test`: a `null as Null` initializer's
/// static type is `Null` as well, so the local and the top-level variable
/// both infer `dynamic`.
#[test]
fn null_cast_initializer_infers_dynamic() {
    let found = invalid_assignments(
        "var global1 = null as Null;\n\
         Object? f(Object three) {\n\
           var local1 = null as Null;\n\
           local1 = three;\n\
           global1 = three;\n\
           return local1;\n\
         }\n",
    );
    assert!(found.is_empty(), "{found:?}");
}

/// Negative control: a WRITTEN `Null` local keeps `Null` — the write still
/// reports.
#[test]
fn written_null_local_write_still_reports() {
    let found = invalid_assignments(
        "Object? f() {\n\
           Null x = null;\n\
           x = \"hi\";\n\
           return x;\n\
         }\n",
    );
    assert!(
        found.iter().any(|row| row
            .ends_with("A value of type 'String' can't be assigned to a variable of type 'Null'.")),
        "{found:?}"
    );
}

/// Negative control: a WRITTEN `Null` local's read keeps `Null` — the
/// `String` initializer still reports.
#[test]
fn written_null_local_read_still_reports() {
    let found = invalid_assignments(
        "String f() {\n\
           Null x = null;\n\
           String y = x;\n\
           return y;\n\
         }\n",
    );
    assert!(
        found.iter().any(|row| row
            .ends_with("A value of type 'Null' can't be assigned to a variable of type 'String'.")),
        "{found:?}"
    );
}

/// Negative control: a WRITTEN `Null` field keeps `Null`.
#[test]
fn written_null_field_write_still_reports() {
    let found = invalid_assignments(
        "class C {\n\
           Null f = null;\n\
         }\n\
         void g(C c) {\n\
           c.f = \"hi\";\n\
         }\n",
    );
    assert!(
        found.iter().any(|row| row
            .ends_with("A value of type 'String' can't be assigned to a variable of type 'Null'.")),
        "{found:?}"
    );
}

/// A non-`Null` initializer infers its own type: the `String` write into
/// the `int` local still reports.
#[test]
fn non_null_initializer_keeps_its_type() {
    let found = invalid_assignments(
        "Object? f() {\n\
           var x = 1;\n\
           x = \"hi\";\n\
           return x;\n\
         }\n",
    );
    assert!(
        found.iter().any(|row| row
            .ends_with("A value of type 'String' can't be assigned to a variable of type 'int'.")),
        "{found:?}"
    );
}
