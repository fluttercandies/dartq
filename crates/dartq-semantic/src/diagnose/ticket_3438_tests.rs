//! The #3438 pins: `end_if_null`'s missing `dead_null_aware_expression`
//! arms. The oracle reads `leftOperand.staticType` for the `??` verdict —
//! the row model must answer the same face for three LHS shapes the
//! scan missed: a collection-literal right operand (`x ?? []`), a
//! leading-dot shorthand (`.blue ?? Color.red`), and an `is`-test or
//! generic-instantiation left operand (`{a is bool ?? true: 3}`,
//! `anyArgs(C<int, int> ?? 1)`).
//!
//! Oracle provenance: `dart analyze` 3.13.1 — the conformance corpus's
//! `missing` column names these exact rows
//! (`InferredTypeTest/test_nullCoalescingOperator{,2}`,
//! `simple_identifier_if_null_error_test`,
//! `constructor_if_null_error_test`,
//! `nullable_type_ambiguous_test`, `ambiguities_A14_t01`,
//! `explicit_instantiation_syntax_test_30`).

use crate::diagnose::test_rig::{CORE_PRELUDE, RigImport, report_ops, report_ops_imports};

/// `(offset, length)` of `needle`'s first occurrence inside the FIXTURE
/// text, shifted past the prelude the rig mounts ahead of it.
fn at(source: &str, needle: &str) -> (u32, u32) {
    let found = source
        .find(needle)
        .unwrap_or_else(|| panic!("`{needle}` missing"));
    (
        found as u32 + CORE_PRELUDE.len() as u32,
        needle.len() as u32,
    )
}

/// The `dead_null_aware_expression` rows as `(offset, length)`,
/// source-ordered.
fn dead_rows(source: &str) -> Vec<(u32, u32)> {
    let report = report_ops(source);
    let mut rows: Vec<(u32, u32)> = report
        .diagnostics
        .iter()
        .filter(|row| row.code.name == "dead_null_aware_expression")
        .map(|row| (row.range.offset, row.range.length))
        .collect();
    rows.sort_unstable();
    rows.dedup();
    rows
}

/// `x ?? []` over a non-nullable `x` reports the dead `[]` — the
/// collection literal's `]` end feeds the report range, never a
/// degenerate zero-length span (`test_nullCoalescingOperator`).
#[test]
fn if_null_over_an_empty_list_rhs_reports_the_dead_literal() {
    let source = "\
void f(List<int> x) {\n\
\x20 var y = x ?? [];\n\
}\n";
    assert_eq!(dead_rows(source), vec![at(source, "[]")]);
}

/// `x ?? [1]` — a non-empty literal's right operand anchors the same
/// dead face.
#[test]
fn if_null_over_a_non_empty_list_rhs_reports_the_dead_literal() {
    let source = "\
void f(List<int> x) {\n\
\x20 var y = x ?? [1];\n\
}\n";
    assert_eq!(dead_rows(source), vec![at(source, "[1]")]);
}

/// A nullable `x` keeps `x ?? []` live — the left operand can be null,
/// so the literal may run.
#[test]
fn if_null_over_a_nullable_left_operand_stays_silent() {
    let source = "\
void f(List<int>? x) {\n\
\x20 var y = x ?? [];\n\
}\n";
    assert_eq!(dead_rows(source), Vec::new());
}

/// The dot-shorthand LHS reads the context type's member face: a
/// non-nullable enum getter (`.blue`) makes the `??` right operand dead
/// — `simple_identifier_if_null_error_test`.
#[test]
fn if_null_over_a_dot_shorthand_member_reports_the_dead_rhs() {
    let source = "\
enum Color { red, green, blue }\n\
void f(Color fallback) {\n\
\x20 Color color = .blue ?? fallback;\n\
}\n";
    // The report anchors `node.rightOperand` — `fallback` alone, never the
    // statement's `;` terminator. `rfind` skips the `f(Color fallback)`
    // parameter to reach the use-site right operand.
    let found = source.rfind("fallback").unwrap() as u32 + CORE_PRELUDE.len() as u32;
    assert_eq!(dead_rows(source), vec![(found, "fallback".len() as u32)]);
}

/// The imported-member arm (`constructor_if_null_error_test`): the
/// shorthand resolves against the imported class's context, and the
/// non-nullable constructor's `??` right operand reports.
#[test]
fn if_null_over_an_imported_dot_shorthand_reports_the_dead_rhs() {
    let helper = "class ConstructorClass {\n\
\x20 final int? x;\n\
\x20 ConstructorClass(this.x);\n\
\x20 ConstructorClass.regular(this.x);\n\
}\n";
    let source = "\
import 'package:helper/helper.dart';\n\
void f(ConstructorClass fallback) {\n\
\x20 ConstructorClass first = .new(1) ?? fallback;\n\
}\n";
    let report = report_ops_imports(
        source,
        &[RigImport {
            uri: "package:helper/helper.dart",
            prefix: None,
            source: helper,
        }],
    );
    assert!(
        report
            .diagnostics
            .iter()
            .any(|d| d.code.name == "dead_null_aware_expression"),
        "{:?}",
        report.diagnostics.iter().map(|d| d.code.name).collect::<Vec<_>>()
    );
}

/// `(a is bool) ?? true` — the `is`-test's `bool` result is
/// non-nullable, so the `??` right operand is dead
/// (`nullable_type_ambiguous_test`).
#[test]
fn if_null_over_an_is_test_reports_the_dead_rhs() {
    let source = "\
void f(Object? a) {\n\
\x20 var m = {a is bool ?? true: 3};\n\
}\n";
    assert_eq!(dead_rows(source), vec![at(source, "true")]);
}

/// A generic instantiation (`C<int, int>` — a TypeLiteral) is
/// non-nullable, so `?? 1` is dead
/// (`explicit_instantiation_syntax_test_30` arm).
#[test]
fn if_null_over_a_generic_instantiation_reports_the_dead_rhs() {
    let source = "\
class C<T, S> {\n\
\x20 C();\n\
}\n\
void f() {\n\
\x20 var v = C<int, int> ?? 1;\n\
}\n";
    assert_eq!(dead_rows(source), vec![at(source, "1")]);
}

/// `main() {` with no written return type reads the oracle's
/// `dynamic`/unknown face — `body_might_complete_normally` stays silent
/// (the `test_async_flattened` discriminating pin: an omitted return is
/// `returnType == null` upstream, never a potentially non-nullable one).
#[test]
fn omitted_return_type_stages_the_silent_dynamic_face() {
    let source = "\
typedef Future<int> CreatesFutureInt();\n\
main() {\n\
\x20 CreatesFutureInt createFutureInt = () async => f();\n\
\x20 Future<int> futureInt = createFutureInt();\n\
\x20 futureInt.then((int i) => print(i));\n\
}\n\
Future<int> f() => Future.value(0);\n";
    let report = report_ops(source);
    assert!(
        !report
            .diagnostics
            .iter()
            .any(|d| d.code.name == "body_might_complete_normally"),
        "{:?}",
        report.diagnostics.iter().map(|d| d.code.name).collect::<Vec<_>>()
    );
}
