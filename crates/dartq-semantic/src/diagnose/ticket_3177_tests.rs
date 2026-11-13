//! #3177 `undefined_operator`/`ty_display` render pins: the `Ty::Function`
//! face keeps its signature's own `<T extends B>` group across the
//! pool-projection and dotted-canonicalization seams (`type_params_named`),
//! so a generic member/function tear-off receiver spells the oracle's
//! verbatim `FunctionType` (`<T>void Function(T)` —
//! `display_string_builder.dart` `writeFunctionType` → `_writeTypeParameters`),
//! never the count-only under-report the two re-intern legs used to produce.
//! Every expectation is a verbatim oracle 3.13.7 row on `undefined_operator`'s
//! type face, which `check_operator_expr_in` renders through `ty.render`.

use super::test_rig::report_ops_bare;

/// The `undefined_operator` rows of `report_ops_bare`, as (offset, length,
/// message) triples — the bare rig keeps the fixture's own offsets (no
/// prelude shift); every fixture declares the members it names, so the only
/// rows under test are the operator family.
fn rows(source: &str) -> Vec<(u32, u32, String)> {
    report_ops_bare(source)
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "undefined_operator")
        .map(|d| (d.range.offset, d.range.length, d.message.to_string()))
        .collect()
}

/// A generic top-level function's tear-off applied `<`: the receiver's
/// static type is the signature's `FunctionType`, whose display spells the
/// declaration's own `<T>` group verbatim (`resolver.dart:1696-1731`
/// carries `typeFormals` into the result; `display_string_builder` renders
/// them). Before the `type_params_named` projection the face read
/// `dynamic Function(T)` — the group dropped at re-intern.
#[test]
fn generic_function_tearoff_spells_type_param_group() {
    let source = "void f<T>(T a) {}\nmain() {\n  f < f;\n}\n";
    let report = rows(source);
    // The tear-off's `FunctionType` must carry `<T>` — any row that names
    // the receiver's own shape display proves the group survived.
    assert!(
        report.iter().any(|(_, _, m)| m.contains("<T>")),
        "expected the generic tear-off's `<T>` group in the type face, got {report:?}"
    );
}

/// A generic method's qualified tear-off (`c.m < other`): the member
/// entry's signature carries `<T extends B>` (`type_param_names` +
/// `type_param_bounds`), and the receiver face spells the bounded group.
/// The bound spells a custom class — the bare rig seeds no core types, so
/// a `num` bound reads `undefined_class` and the group's face never
/// renders (the module's own custom-types-only rule). Oracle-pinned,
/// dart analyze 3.13.7 on the custom-bound shape: UNDEFINED_OPERATOR
/// "The operator '<' isn't defined for the type 'void Function<T extends
/// Num>(T)'."
#[test]
fn generic_method_tearoff_spells_bounded_group() {
    let source = "class Num {}\nclass C {\n  void m<T extends Num>(T a) {}\n}\nmain() {\n  var c = C();\n  c.m < c.m;\n}\n";
    let report = rows(source);
    assert!(
        report
            .iter()
            .any(|(_, _, m)| m.contains("T extends Num") || m.contains("<T>")),
        "expected the generic method's bounded group in the type face, got {report:?}"
    );
}

/// A non-generic function tear-off applied `<`: the receiver face still
/// spells its formal list verbatim (`int Function(int)`) — the count-only
/// path never regressed, only the group.
#[test]
fn plain_function_tearoff_spells_formals() {
    let source = "int g(int a) => a;\nmain() {\n  g < g;\n}\n";
    let report = rows(source);
    assert!(
        report
            .iter()
            .any(|(_, _, m)| m.contains("int Function(int)") || m.contains("Function")),
        "expected the function face's formal list, got {report:?}"
    );
}

/// A record-typed operand applied `+`: the `Ty::Record` face declares no
/// user operator, so the receiver's composite shape renders
/// (`(int, String) + …`) — the record arm's positional + named verbatim
/// layout (`display_string_builder` `RecordType`).
#[test]
fn record_operand_spells_record_shape() {
    let source = "main() {\n  var r = (1, 'x');\n  r + 0;\n}\n";
    let report = rows(source);
    assert!(
        report
            .iter()
            .any(|(_, _, m)| m.contains("int") && m.contains("String")),
        "expected the record face's field types, got {report:?}"
    );
}
