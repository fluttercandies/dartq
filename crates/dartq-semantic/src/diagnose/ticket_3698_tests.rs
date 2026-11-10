//! #3698 pins — the `wrong_type_parameter_variance_position` member-face
//! anchors (`_checkForWrongVariancePosition`, upstream Dart SDK source
//! `pkg/analyzer/lib/src/generated/error_verifier.dart:7565-7658`):
//! * the return-type check anchors at the return TypeAnnotation node
//!   (`T` in `T method1()`, `Cov<T>` in `Cov<T> method3()`), not the
//!   member name;
//! * each formal's contravariant-position check anchors at its own
//!   FormalParameter node (`Contra<T> x`, `required Contra<T> x`), not
//!   the member name;
//! * a method's own type-parameter bound check anchors at the
//!   `X extends B` TypeParameter node and fires in the `inout` position
//!   (`Invariant.combine`, error_verifier.dart:7608-7624) — the arm was
//!   missing entirely;
//! * the field arms keep the variable-name anchor (upstream
//!   `fields.variables.first.name`), now including the getter's return
//!   type (`T get c` anchors at `T`).

use super::super::test_rig::report_ops_experiments_with;
use dartq_language::Feature;

/// The `wrong_type_parameter_variance_position` rows of `source` under the
/// variance experiment: `code @offset+length: message`.
fn position_rows(source: &str) -> Vec<String> {
    report_ops_experiments_with(source, &[], None, &[Feature::Variance])
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "wrong_type_parameter_variance_position")
        .map(|d| format!("@{}+{}: {}", d.range.offset, d.range.length, d.message))
        .collect()
}

/// `test_method_returnType` (variance_in_method_error_test): the row
/// anchors at the `T` return annotation — before the fix it sat at the
/// `method1` name token.
#[test]
fn method_return_row_anchors_at_the_return_type() {
    let source = "class A<in T> {\n  T method1() => throw 0;\n}\n";
    let rows = position_rows(source);
    assert_eq!(rows.len(), 1, "{rows:?}");
    let offset = source.find("T method1").unwrap();
    assert_eq!(
        rows[0],
        format!("@{offset}+1: The 'in' type parameter 'T' can't be used in an 'out' position."),
    );
}

/// `test_method_parameterType`: each formal anchors at its own
/// FormalParameter node — `Contra<T> x` is 11 units starting at `C`.
#[test]
fn method_formal_row_anchors_at_the_formal() {
    let source = "typedef Contra<T> = void Function(T);\n\
         class A<in T> {\n  void method2(Contra<T> x) {}\n}\n";
    let rows = position_rows(source);
    assert_eq!(rows.len(), 1, "{rows:?}");
    let offset = source.find("Contra<T> x").unwrap();
    assert_eq!(
        rows[0],
        format!("@{offset}+11: The 'in' type parameter 'T' can't be used in an 'out' position."),
    );
}

/// `test_method_typeParameterBound`: the `X extends B` arm (missing
/// outright before the fix) anchors at the TypeParameter node — `X`
/// through the bound's end — and reports the `inout` position
/// (`Variance.invariant.combine`, error_verifier.dart:7614-7624).
#[test]
fn method_bound_row_anchors_at_the_type_parameter() {
    let source = "class A<in T> {\n  void method18<X extends T>() {}\n}\n";
    let rows = position_rows(source);
    assert_eq!(rows.len(), 1, "{rows:?}");
    let offset = source.find("X extends T").unwrap();
    assert_eq!(
        rows[0],
        format!("@{offset}+11: The 'in' type parameter 'T' can't be used in an 'inout' position."),
    );
}

/// The bound arm's `out` face (`variance_out_method_error_test`
/// `test_method_typeParameterBound`-family): an `out` parameter in an
/// `inout` bound position fires the same node anchor.
#[test]
fn method_bound_row_fires_for_out_parameter() {
    let source = "class A<out T> {\n  void m<X extends T>() {}\n}\n";
    let rows = position_rows(source);
    assert_eq!(rows.len(), 1, "{rows:?}");
    let offset = source.find("X extends T").unwrap();
    assert_eq!(
        rows[0],
        format!("@{offset}+11: The 'out' type parameter 'T' can't be used in an 'inout' position."),
    );
}

/// `test_method_namedParameterType` (`{required Contra<T> x}`): the
/// FormalParameter node includes the `required` keyword — the row spans
/// `required Contra<T> x` (20 units), not just `Contra<T> x`.
#[test]
fn named_formal_row_includes_the_required_keyword() {
    let source = "typedef Contra<T> = void Function(T);\n\
         class A<in T> {\n  void m({required Contra<T> x}) {}\n}\n";
    let rows = position_rows(source);
    assert_eq!(rows.len(), 1, "{rows:?}");
    let offset = source.find("required Contra<T> x").unwrap();
    assert_eq!(
        rows[0],
        format!("@{offset}+20: The 'in' type parameter 'T' can't be used in an 'out' position."),
    );
}

/// Two named formals in one `{...}` section anchor independently —
/// `test_method23`-shape: only the offending formal's span rows; a
/// non-firing sibling emits nothing (the named-section split must not
/// collapse or misalign the two).
#[test]
fn two_named_formals_anchor_independently() {
    let source = "typedef Contra<T> = void Function(T);\n\
         typedef Cov<T> = T Function();\n\
         class A<in T> {\n  void m({required Cov<T> x, required Contra<T> y}) {}\n}\n";
    let rows = position_rows(source);
    assert_eq!(rows.len(), 1, "{rows:?}");
    let offset = source.find("required Contra<T> y").unwrap();
    assert_eq!(
        rows[0],
        format!("@{offset}+20: The 'in' type parameter 'T' can't be used in an 'out' position."),
    );
}

/// `test_setter_parameterType` (variance_out_field_error_test): a
/// setter's formal anchors at its FormalParameter node — `T value` is 7
/// units starting at `T`; the `void` return reads unrelated and rows
/// nothing.
#[test]
fn setter_formal_row_anchors_at_the_formal() {
    let source = "class A<out T> {\n  void set a(T value) => value;\n}\n";
    let rows = position_rows(source);
    assert_eq!(rows.len(), 1, "{rows:?}");
    let offset = source.find("T value").unwrap();
    assert_eq!(
        rows[0],
        format!("@{offset}+7: The 'out' type parameter 'T' can't be used in an 'in' position."),
    );
}

/// `test_getter_returnType` (variance_in_field_error_test): a getter's
/// return-type row anchors at the `T` annotation before `get`, not the
/// `c` name token.
#[test]
fn getter_return_row_anchors_at_the_return_type() {
    let source = "class A<in T> {\n  T get c => throw 0;\n}\n";
    let rows = position_rows(source);
    assert_eq!(rows.len(), 1, "{rows:?}");
    let offset = source.find("T get c").unwrap();
    assert_eq!(
        rows[0],
        format!("@{offset}+1: The 'in' type parameter 'T' can't be used in an 'out' position."),
    );
}

/// `test_field`/induced-setter arm keeps the variable-name anchor:
/// `late T d` fires the induced setter's contravariant row at `d`.
#[test]
fn field_induced_setter_row_anchors_at_the_name() {
    let source = "class A<out T> {\n  late T d;\n}\n";
    let rows = position_rows(source);
    assert_eq!(rows.len(), 1, "{rows:?}");
    let offset = source.find("d;").unwrap();
    assert_eq!(
        rows[0],
        format!("@{offset}+1: The 'out' type parameter 'T' can't be used in an 'in' position."),
    );
}

/// Negative control: a method whose bound, formals and return all stay
/// within the declared keyword rows nothing — an `in T` parameter whose
/// only occurrence is the contravariant formal `T x`, with a bound
/// mentioning no `T` (`Unrelated` satisfies every keyword) and a `void`
/// return.
#[test]
fn method_within_the_keyword_rows_nothing() {
    let source = "class A<in T> {\n  void m<X extends num>(T x) {}\n}\n";
    let rows = position_rows(source);
    assert!(rows.is_empty(), "{rows:?}");
}

/// Negative control: the experiment gate — a keyword parameter without
/// the variance grant emits no `_position` rows at all (the keyword
/// parses but the face stays gated, error_verifier.dart:7567
/// `isLegacyCovariant` posture).
#[test]
fn keyword_parameter_without_the_experiment_stays_silent() {
    let source = "class A<in T> {\n  T method1() => throw 0;\n}\n";
    let report = report_ops_experiments_with(source, &[], None, &[]);
    let rows: Vec<String> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "wrong_type_parameter_variance_position")
        .map(|d| format!("{} @{}: {}", d.code.name, d.range.offset, d.message))
        .collect();
    assert!(rows.is_empty(), "{rows:?}");
}

/// The `covariant` formal exemption survives the re-anchor
/// (error_verifier.dart:7633-7635): `covariant Contra<T> x` declares the
/// contravariant face itself and rows nothing.
#[test]
fn covariant_formal_stays_silent() {
    let source = "typedef Contra<T> = void Function(T);\n\
         class A<out T> {\n  void m(covariant Contra<T> x) {}\n}\n";
    let rows = position_rows(source);
    assert!(rows.is_empty(), "{rows:?}");
}

/// The compound arm ordering and anchoring of one class — the fixture's
/// `method1`/`method2`/`method18`/`method21` shapes in one container:
/// each row lands on its own arm's node (return annotation, formal,
/// `X extends B` bound, `required`-leading named formal) and the count
/// matches the oracle's one-row-per-arm shape.
#[test]
fn compound_member_anchors_each_arm_at_its_own_node() {
    let source = concat!(
        "typedef Contra<T> = void Function(T);\n",
        "class A<in T> {\n",
        "  T method1() => throw 0;\n",
        "  void method2(Contra<T> x) {}\n",
        "  void method18<X extends T>() {}\n",
        "  void method21({required Contra<T> x}) {}\n",
        "}\n",
    );
    let rows = position_rows(source);
    assert_eq!(rows.len(), 4, "{rows:?}");
    let ret = source.find("T method1").unwrap();
    let formal = source.find("Contra<T> x)").unwrap();
    let bound = source.find("X extends T").unwrap();
    let named = source.find("required Contra<T> x").unwrap();
    assert_eq!(
        rows,
        vec![
            format!("@{ret}+1: The 'in' type parameter 'T' can't be used in an 'out' position."),
            format!(
                "@{formal}+11: The 'in' type parameter 'T' can't be used in an 'out' position."
            ),
            format!(
                "@{bound}+11: The 'in' type parameter 'T' can't be used in an 'inout' position."
            ),
            format!("@{named}+20: The 'in' type parameter 'T' can't be used in an 'out' position."),
        ],
    );
}

/// An unbounded own type parameter (`X`, no `extends`) carries no bound
/// check at all (upstream `bound == null → continue`,
/// error_verifier.dart:7613-7615) — the method rows nothing.
#[test]
fn unbounded_type_parameter_rows_nothing() {
    let source = "class A<in T> {\n  void m<X>(T x) {}\n}\n";
    let rows = position_rows(source);
    // `T x` IS a contravariant-position violation for `in T`? No — `in`
    // allows contravariant: the formal's `T` at `in` position satisfies
    // the `in` keyword, so only the absent bound matters here.
    assert!(rows.is_empty(), "{rows:?}");
}
