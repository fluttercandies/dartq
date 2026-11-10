//! #3642 pins — the variance pair's gap faces:
//! * `wrong_type_parameter_variance_in_superinterface`: a generic typedef's
//!   parameter variances are BODY-computed (variance_builder.dart:173-181),
//!   not keyword-declared — `typedef F<X> = void Function(X)` contributes
//!   `X` contravariantly wherever `F<X>` heads a superinterface, and the
//!   function-shaped right-hand side walks through the `Ty` → `VarTree`
//!   conversion's `Ty::Function` arm.
//! * `wrong_type_parameter_variance_position`: a member type spelled as a
//!   function type (`final T Function() b`) resolves through the same
//!   conversion (the model's `Ty::Function` interned shape), and the
//!   `covariant` marker suppresses exactly the induced-setter/formal
//!   contravariant face (error_verifier.dart:7583-7588/:7633-7635).

use super::super::test_rig::{report_ops, report_ops_experiments_with, rows};
use dartq_language::Feature;

/// The `wrong_type_parameter_variance_in_superinterface` rows of `source`
/// (code, offset, message), diagnosed under the variance experiment.
fn super_rows(source: &str) -> Vec<String> {
    report_ops_experiments_with(source, &[], None, &[Feature::Variance])
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "wrong_type_parameter_variance_in_superinterface")
        .map(|d| format!("@{}: {}", d.range.offset, d.message))
        .collect()
}

/// The `wrong_type_parameter_variance_position` rows of `source` under the
/// variance experiment.
fn position_rows(source: &str) -> Vec<String> {
    report_ops_experiments_with(source, &[], None, &[Feature::Variance])
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "wrong_type_parameter_variance_position")
        .map(|d| format!("@{}: {}", d.range.offset, d.message))
        .collect()
}

/// `test_class_extends_function_parameterType` (the canonical miss): the
/// alias's `X` is contravariant through `Function(X)`, so `A<F<X>>` reads
/// `X` contravariantly — a legacy-covariant parameter cannot satisfy it.
/// The row anchors at the parameter's name token (upstream
/// `typeParameter.diagnosticRange`), col 9 = `X` on line 4.
#[test]
fn typedef_contravariant_body_fires_the_superinterface_row() {
    let source = concat!(
        "typedef F<X> = void Function(X);\n",
        "class A<X> {}\n",
        "class B<X> extends A<F<X>> {}\n",
    );
    let rows = super_rows(source);
    assert_eq!(rows.len(), 1, "{rows:?}");
    let offset = source.find("B<X>").unwrap() + 2; // the `X` token
    assert!(
        rows[0].starts_with(&format!(
            "@{offset}: 'X' can't be used contravariantly or invariantly in 'A<F<X>>'"
        )),
        "{rows:?}"
    );
}

/// `test_class_extends_function_returnType`: `typedef F<X> = X Function()`
/// computes `X` covariant — `A<F<X>>` stays within the legacy parameter's
/// covariant face and the row must NOT fire.
#[test]
fn typedef_covariant_body_stays_silent() {
    let source = concat!(
        "typedef F<X> = X Function();\n",
        "class A<X> {}\n",
        "class B<X> extends A<F<X>> {}\n",
    );
    let rows = super_rows(source);
    assert!(rows.is_empty(), "{rows:?}");
}

/// `typedef F<X> = X Function(X)` computes `X` invariant (return is
/// covariant, parameter is contravariant, the meet is invariant) —
/// `A<F<X>>` still breaks the legacy-covariant parameter.
#[test]
fn typedef_invariant_body_fires_the_superinterface_row() {
    let source = concat!(
        "typedef F<X> = X Function(X);\n",
        "class A<X> {}\n",
        "class B<X> extends A<F<X>> {}\n",
    );
    let rows = super_rows(source);
    assert_eq!(rows.len(), 1, "{rows:?}");
}

/// A bare function-typed superinterface walks the same tree shape as the
/// typedef body (`extends A<void Function(X)>` — the written-type
/// capture's `VarTreeKind::Fn` arm): `X` occurs contravariantly.
#[test]
fn bare_function_superinterface_still_fires() {
    let source = concat!(
        "class A<X> {}\n",
        "class B<X> extends A<void Function(X)> {}\n"
    );
    let rows = super_rows(source);
    assert_eq!(rows.len(), 1, "{rows:?}");
}

/// `variance_in_field_error_test`'s `final T Function() b` face: the
/// field's function-typed `var_ty` carries `T` covariantly (return
/// position), so `in T` rejects it — one row at the field's name token
/// (the getter-side check; `final` suppresses the induced-setter face).
#[test]
fn function_typed_field_fires_at_its_name() {
    let source = concat!(
        "class A<in T> {\n",
        "  final T Function() b = () => throw \"\";\n",
        "}\n",
    );
    let rows = position_rows(source);
    assert_eq!(rows.len(), 1, "{rows:?}");
    let offset = source.find("b =").unwrap();
    assert!(
        rows[0].starts_with(&format!(
            "@{offset}: The 'in' type parameter 'T' can't be used in an 'out' position."
        )),
        "{rows:?}"
    );
}

/// A `covariant` non-final field emits only the getter-side row: the
/// induced setter's parameter rides the field's `isExplicitlyCovariant`
/// mark (element_builder.dart:504), so upstream's
/// `node.covariantKeyword == null` clause suppresses the setter check
/// (error_verifier.dart:7583-7588). Without the exemption the face
/// over-reports a second row at the same anchor.
#[test]
fn covariant_field_suppresses_only_the_setter_face() {
    let source = concat!("class A<in T> {\n", "  covariant late T e;\n", "}\n",);
    let rows = position_rows(source);
    assert_eq!(rows.len(), 1, "{rows:?}");
}

/// A `covariant` method formal is exempt from the contravariant-position
/// check (error_verifier.dart:7633-7635 `parameter.isCovariant`): the
/// formal's declared variance IS the contravariant face, so `out T` over
/// `covariant T x` must not fire the `in` row a plain `T x` fires.
#[test]
fn covariant_formal_is_exempt_but_plain_formal_fires() {
    let exempt = concat!("class A<out T> {\n", "  void m(covariant T x) {}\n", "}\n",);
    assert!(
        position_rows(exempt).is_empty(),
        "covariant formal must not fire"
    );
    let plain = concat!("class A<out T> {\n", "  void m(T x) {}\n", "}\n");
    // The plain formal's check anchors at the formal's own span upstream
    // (rider: the member-name anchor stands in until the model carries
    // type spans) — one row is the pin.
    assert_eq!(position_rows(plain).len(), 1, "plain formal must fire");
}

/// `Ty::Record` member types convert through the same arm: `(T,)` carries
/// `T` covariantly, so `in T` rejects the field. The shape is the
/// records-face twin of `final T Function() b`.
#[test]
fn record_typed_field_fires_at_its_name() {
    let source = concat!(
        "class A<in T> {\n",
        "  final (T,) r = (throw \"\",);\n",
        "}\n"
    );
    let rows = position_rows(source);
    assert_eq!(rows.len(), 1, "{rows:?}");
}

/// A chained typedef resolves through the seeded table's fixed-point:
/// `typedef G<X> = F<X>` after `typedef F<X> = void Function(X)` keeps
/// `X` contravariant, so `A<G<X>>` fires exactly like `A<F<X>>`.
#[test]
fn chained_typedef_preserves_the_computed_variance() {
    let source = concat!(
        "typedef F<X> = void Function(X);\n",
        "typedef G<X> = F<X>;\n",
        "class A<X> {}\n",
        "class B<X> extends A<G<X>> {}\n",
    );
    let rows = super_rows(source);
    assert_eq!(rows.len(), 1, "{rows:?}");
}

/// The non-experiment face: without `Feature::Variance` the legacy
/// superinterface check still fires (the capture is unconditional — #742)
/// but the position check's keyword-gated member faces stay silent.
#[test]
fn superinterface_face_is_not_experiment_gated() {
    let source = concat!(
        "typedef F<X> = void Function(X);\n",
        "class A<X> {}\n",
        "class B<X> extends A<F<X>> {}\n",
    );
    let report = report_ops(source);
    let supers = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "wrong_type_parameter_variance_in_superinterface")
        .count();
    assert_eq!(supers, 1, "{:#?}", rows(&report));
}
