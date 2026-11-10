//! #3524 pins — the `await` operand's expression-level static type is
//! `flatten(operand)`, and the decl-initializer/assignment faces must see
//! the flattened face.
//!
//! # The defect
//!
//! `end_await_expression` rewrites the operand row's stored `ty` to
//! `flatten(operand_static_ty)` — `Future<int>` becomes `int` — and pushes
//! the operand extent onto `await_operand_extents`. But the assignability
//! faces (`check_decl_initializer`, `check_assignment`,
//! `check_default_value_assignable`) re-derive the right-hand expression's
//! type through `rhs_expression_ty` → `expression_row_ty`, whose invocation
//! arm re-decides the send's static type from the callee's declared return
//! and spells the raw `Future<int>` again — the stored `int` rewrite is
//! discarded (`row.ty != asserted` keeps `asserted`). `Future<int>` against
//! `String` lands in the matrix's undecidable blind spot (an SDK interface
//! head against a final primitive — neither the collection-disjoint arm nor
//! `declared_outside_core` decides it), so `String a = await fi();` staged
//! nothing where the oracle reports `invalid_assignment` at the whole
//! `await fi()` (len 10).
//!
//! # The contract
//!
//! `rhs_expression_ty` re-applies `self.flatten` over the result type
//! whenever the result row sits inside an `await` operand extent — the same
//! on-demand read the argument faces run (`flush_argument` /
//! `flush_unclaimed`, which already flatten over the extent). The flatten
//! belongs to the WHOLE operand expression only: the `??`/`?:` left-operand
//! recursion reads sub-expression types, which upstream does not flatten
//! (`analyzeAwaitExpression` flattens the operand's `staticType` once,
//! type_analyzer.dart:542-548).

use super::test_rig::{CORE_PRELUDE, report_ops};

/// The `(code, offset, len)` rows of `fixture`'s `invalid_assignment`
/// verdicts — the face these pins own. `report_ops` prepends
/// [`CORE_PRELUDE`], so the offsets index into the prefixed source.
fn assignment_rows(fixture: &str) -> Vec<(&'static str, u32, u32)> {
    report_ops(fixture)
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "invalid_assignment")
        .map(|d| (d.code.name, d.range.offset, d.range.length))
        .collect()
}

/// Offset of `needle` inside the first occurrence of `line` within
/// `source` (the prelude-prefixed text the report offsets index into).
fn at(source: &str, line: &str, needle: &str) -> u32 {
    let line_at = source
        .find(line)
        .unwrap_or_else(|| panic!("fixture has {line:?}"));
    let needle_at = line
        .find(needle)
        .unwrap_or_else(|| panic!("{line:?} has {needle:?}"));
    (line_at + needle_at) as u32
}

/// The headline repro: `String a = await fi();` over `Future<int> fi()`
/// reports one `invalid_assignment` anchored at the whole `await fi()`
/// (the `await` keyword through `)` — the oracle's len-10 row), not at the
/// operand's own span.
#[test]
fn await_operand_decl_initializer_reports_the_flattened_type() {
    let l1 = "Future<int> fi() => Future.value(0);\n";
    let l2 = "f() async {\n";
    let l3 = "  String a = await fi();\n";
    let l4 = "}\n";
    let source = format!("{CORE_PRELUDE}{l1}{l2}{l3}{l4}");
    let rows = assignment_rows(&format!("{l1}{l2}{l3}{l4}"));
    assert_eq!(
        rows,
        [("invalid_assignment", at(&source, l3, "await fi()"), 10)],
        "flatten(Future<int>) = int rejects the String slot at the await expression: {rows:?}"
    );
}

/// The silent twin: `String a = await fs();` over `Future<String> fs()`
/// stays silent — the flattened face assigns cleanly, so no verdict may
/// land. The pin guards the verdict reading the flattened type rather
/// than the raw `Future` (a `Future<String>` face would ask
/// `Future<String> <: String` and land the undecidable miss either way,
/// but a flatten that never ran would keep the raw face silent for the
/// wrong reason — the row pair with the pin above is the contract).
#[test]
fn await_operand_decl_initializer_stays_silent_when_the_flattened_type_fits() {
    let fixture = "Future<String> fs() => Future.value(\"x\");\n\
                   f() async {\n\
                   \x20 String a = await fs();\n\
                   }\n";
    assert_eq!(
        assignment_rows(fixture),
        [],
        "flatten(Future<String>) = String assigns into String silently"
    );
}

/// `FutureOr<int>` flattens to `int` (the union's value leg): `String a =
/// await fo();` over `FutureOr<int> fo()` reports the same
/// `invalid_assignment` at `await fo()`.
#[test]
fn await_future_or_operand_flattens_to_the_value_leg() {
    let l1 = "FutureOr<int> fo() => Future.value(0);\n";
    let l2 = "f() async {\n";
    let l3 = "  String a = await fo();\n";
    let l4 = "}\n";
    let source = format!("{CORE_PRELUDE}{l1}{l2}{l3}{l4}");
    let rows = assignment_rows(&format!("{l1}{l2}{l3}{l4}"));
    assert_eq!(
        rows,
        [("invalid_assignment", at(&source, l3, "await fo()"), 10)],
        "flatten(FutureOr<int>) = int rejects the String slot: {rows:?}"
    );
}

/// The same flattened read inside an ARGUMENT position: `g(await fi())`
/// with `g(String s)` and `Future<int> fi()` reports the argument-face
/// verdict, proving the extent read reaches every consumer of the
/// expression's static type, not only the decl-initializer face. (The
/// argument face already flattened — this pin locks the parity so the two
/// faces cannot drift apart.)
#[test]
fn await_operand_inside_an_argument_keeps_its_verdict() {
    let l1 = "Future<int> fi() => Future.value(0);\n";
    let l2 = "void g(String s) {}\n";
    let l3 = "f() async {\n";
    let l4 = "  g(await fi());\n";
    let l5 = "}\n";
    let fixture = format!("{l1}{l2}{l3}{l4}{l5}");
    let rows: Vec<(&'static str, u32, u32)> = report_ops(&fixture)
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "argument_type_not_assignable")
        .map(|d| (d.code.name, d.range.offset, d.range.length))
        .collect();
    let source = format!("{CORE_PRELUDE}{fixture}");
    assert_eq!(
        rows,
        [(
            "argument_type_not_assignable",
            at(&source, l4, "await fi()"),
            10
        )],
        "flatten(Future<int>) = int rejects the String formal: {rows:?}"
    );
}
