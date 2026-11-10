//! #3677 pins — a type name in expression position is a `TypeLiteral`
//! (static type `dart:core Type`), never the aliased function type.
//!
//! The oracle's rule (upstream Dart SDK source,
//! `pkg/analyzer/lib/src/dart/resolver/simple_identifier_resolver.dart`
//! `SimpleIdentifierResolver._resolve2`): a `SimpleIdentifier` that resolves
//! to an `InterfaceElement` or a `TypeAliasElement` records
//! `_typeProvider.typeType` whenever `_isExpressionIdentifier` holds — and
//! for a `TypeAliasElement` also whenever its `aliasedType` is not an
//! `InterfaceType`. The identifier's own static type is therefore `Type`,
//! so `f(F)` against a `Type` formal is silent where our row carried the
//! alias's right-hand `Function` face and reported
//! `argument_type_not_assignable` (census-12 shape 2.1, 147 rows + the
//! family-1 `C<int>`→`Type` tail).
//!
//! The fix rewrites only the value-operand slot (`row.ty`) of a bare
//! type-ALIAS read — and only when the name is NOT a member-receiver head
//! (`Cb.foo`, `Cb.new`): the selector heads keep the declared right-hand
//! face the literal-receiver member-miss pins consume (the
//! `followed_by_period` lookahead in `push_bare` is the discriminator).
//!
//! Each pin below carries the oracle's verdict for the same fixture
//! (`dart analyze` 3.13.7):
//!
//! | sub-rule | oracle corpus case |
//! |---|---|
//! | bare class name in argument position | `NonErrorResolverTest/test_typeType_class#0` |
//! | bare function-RHS typedef in argument position | `NonErrorResolverTest/test_typeType_functionTypeAlias#0` |
//! | prefixed `p.C` in argument position | `NonErrorResolverTest/test_typeType_class_prefixed#0` |
//! | same-name call still types as its return | negative control — `f(g())` over `int g()` |
//! | alias-to-class still constructs/reads members | `typedef L = Real; L()` / `L.named` faces |

use super::test_rig::{RigImport, assert_silent, report_ops, report_ops_imports, rows};

/// The shared `Type`-formal sink every fixture drives the literal through.
const SINK: &str = "void f(Type t) {}\n";

/// Sub-rule (a): a bare **class** name in argument position is a `Type`
/// literal — `f(C)` against `Type` is silent.
///
/// Oracle (`NonErrorResolverTest/test_typeType_class#0`): no rows.
#[test]
fn class_name_in_argument_position_is_type_literal() {
    let report = report_ops(&format!(
        "class C {{}}\n{SINK}void main() {{\n  f(C);\n}}\n"
    ));
    assert_silent(&report);
}

/// Sub-rule (b): a bare **function-RHS typedef** in argument position is a
/// `Type` literal — `f(F)` is silent even though the alias's right-hand
/// side is `void Function()` (upstream's `aliasedType is! InterfaceType`
/// arm). The row must not carry the function face into the assignability
/// check.
///
/// Oracle (`NonErrorResolverTest/test_typeType_functionTypeAlias#0`): no
/// rows.
#[test]
fn function_typedef_in_argument_position_is_type_literal() {
    let report = report_ops(&format!(
        "typedef F = void Function();\n{SINK}void main() {{\n  f(F);\n}}\n"
    ));
    assert_silent(&report);
}

/// Sub-rule (b2): the old-style `typedef F();` form reads identically.
///
/// Oracle: same `Type` literal verdict (`aliasedType` is a function type).
#[test]
fn old_style_typedef_in_argument_position_is_type_literal() {
    let report = report_ops(&format!(
        "typedef F();\n{SINK}void main() {{\n  f(F);\n}}\n"
    ));
    assert_silent(&report);
}

/// Sub-rule (c): a **prefixed** class name `p.C` in argument position is a
/// `Type` literal — silent.
///
/// Oracle (`NonErrorResolverTest/test_typeType_class_prefixed#0`): no
/// rows.
#[test]
fn prefixed_class_name_in_argument_position_is_type_literal() {
    // `RigImport::prefix` binds the `p` edge in the namespace graph, so the
    // fixture needs no `import` directive of its own (one written into the
    // source would land after the prelude's declarations).
    let report = report_ops_imports(
        &format!("{SINK}void main() {{\n  f(p.C);\n}}\n"),
        &[RigImport {
            uri: "package:a/a.dart",
            prefix: Some("p"),
            source: "class C {}\n",
        }],
    );
    assert_silent(&report);
}

/// A sink whose `int` formal the rig can actually check — `Type` lives in
/// the names-only stub so assignability against it under-reports; a
/// concrete `int` formal makes the negative control's row observable.
const INT_SINK: &str = "void g(int t) {}\n";

/// Negative control: a same-shape **call** to a real function still types
/// as its return — `g(h())` where `h()` returns `String` reports
/// `argument_type_not_assignable`, proving only the bare literal is
/// rewritten to `Type`.
#[test]
fn call_of_same_named_function_keeps_its_return_type() {
    let report = report_ops(&format!(
        "String h() => '';\n{INT_SINK}void main() {{\n  g(h());\n}}\n"
    ));
    assert!(
        report
            .diagnostics
            .iter()
            .any(|d| d.code.name == "argument_type_not_assignable"),
        "g(h()) must still see the String return type: {:#?}",
        rows(&report)
    );
}

/// Negative control (b): a **variable** holding a value is NOT a type
/// literal — `g(s)` where `s` is a `String` local reports against the
/// `int` formal (only the *type name* row turns into `Type`).
#[test]
fn function_typed_variable_is_not_a_type_literal() {
    let report = report_ops(&format!(
        "{INT_SINK}void main() {{\n  String s = '';\n  g(s);\n}}\n"
    ));
    assert!(
        report
            .diagnostics
            .iter()
            .any(|d| d.code.name == "argument_type_not_assignable"),
        "g(s) over a String local must still report: {:#?}",
        rows(&report)
    );
}

/// Sub-rule (d): an **alias-to-class** `typedef L = Real` still resolves
/// its member/creation face — `L()` constructs `Real` (the row is reshaped
/// to the class container at invoke) and `L` as a bare argument is a
/// `Type` literal. Both faces survive the `row.ty` rewrite.
///
/// Oracle (`typedef L = Real; L(); f(L)`): `f(L)` silent, `L()` clean.
#[test]
fn alias_to_class_still_constructs_and_reads_as_type() {
    let report = report_ops(&format!(
        "class Real {{}}\ntypedef L = Real;\n{SINK}void main() {{\n  f(L);\n  L();\n}}\n"
    ));
    assert_silent(&report);
}
