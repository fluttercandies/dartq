//! #3163 `const_initialized_with_non_constant_value` pins: the oracle's
//! left-to-right first-failure anchor on the const-initializer verdict.
//! Four capture faces drove the missing half of the family — `is`/`as`
//! (const-transparent: the operand's own verdict is the result, upstream
//! `visitIsExpression`/`visitAsExpression` carry no constant-verifier arm),
//! `e1[e2]` and `await` (unconditionally non-constant, anchored at the
//! whole expression), and the non-const member read through a type literal
//! (`const x = C.c` — `member()`'s silent deferral, re-decided by the
//! `{container}\u{2}{name}` composite keys `const_decl_name` joins).
//! Every expectation is a verbatim oracle 3.13.7 row.

use super::super::test_rig::report_ops_bare;

/// The `const_initialized_with_non_constant_value` rows of
/// `report_ops_bare`, as `(offset, length)` pairs — the code alone is under
/// test; sibling faces (`non_constant_list_element`, `invalid_constant`,
/// `static_access_to_instance_member`) ride other codes.
fn rows(source: &str) -> Vec<(u32, u32)> {
    report_ops_bare(source)
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "const_initialized_with_non_constant_value")
        .map(|d| (d.range.offset, d.range.length))
        .collect()
}

/// An `is` test is const-transparent: a non-constant operand reports at
/// the operand (`nv`), a constant one stays silent.
#[test]
fn is_test_reports_at_operand() {
    let source = "int nv = 7;\nconst x = nv is int;\nconst y = 1 is int;\n";
    assert_eq!(rows(source), vec![(22, 2)]);
}

/// An `as` cast is const-transparent like `is`: `nv as int` reports at
/// `nv`, `1 as int` stays silent.
#[test]
fn as_cast_reports_at_operand() {
    let source = "int nv = 7;\nconst x = nv as int;\nconst y = 1 as int;\n";
    assert_eq!(rows(source), vec![(22, 2)]);
}

/// `e1[e2]` is never constant — the oracle flags the whole IndexExpression
/// regardless of the operands (`l[0]` over a const list fires the same
/// way): receiver start through the closing bracket.
#[test]
fn index_reports_whole_expression() {
    let source = "const l = [1, 2];\nList<int> nv = const [];\nconst x = nv[0];\nconst y = l[0];\n";
    assert_eq!(rows(source), vec![(53, 5), (70, 4)]);
}

/// `await` is never constant — the oracle anchors the whole
/// AwaitExpression (the `await` keyword through the operand's end).
#[test]
fn await_reports_whole_expression() {
    let source = "Future<int> f() async => 1;\nf() async {\n  const x = await f();\n}\n";
    assert_eq!(rows(source), vec![(52, 9)]);
}

/// A non-const field read through the type literal reports at the access
/// (`C.c`); a const field stays silent.
#[test]
fn nonconst_static_field_reports_at_access() {
    let source = "class C {\n  static int c = 1;\n  static const int k = 2;\n}\nconst x = C.c;\nconst z = C.k;\n";
    assert_eq!(rows(source), vec![(68, 3)]);
}

/// The inner member access anchors a chained read — `C.c.v` reports at
/// `C.c`, not at the whole `C.c.v`.
#[test]
fn nonconst_static_field_chained_reports_inner() {
    let source = "class V {\n  final int v;\n  const V(this.v);\n}\nclass C {\n  static V c = const V(1);\n}\nconst x = C.c.v;\n";
    assert_eq!(rows(source), vec![(95, 3)]);
}

/// First failure wins by evaluation order: `C.c + nv` reports `C.c`
/// (left), `nv + C.c` reports `nv` (left).
#[test]
fn leftmost_failure_wins() {
    let source = "class C {\n  static int c = 1;\n}\nint nv = 7;\nconst x = C.c + nv;\nconst y = nv + C.c;\n";
    assert_eq!(rows(source), vec![(54, 3), (74, 2)]);
}

/// A bare read of the enclosing container's own non-const static field
/// resolves through the class scope — `static const x = c` reports at
/// `c`.
#[test]
fn bare_nonconst_field_in_class_reports() {
    let source = "class C {\n  static int c = 1;\n  static const int x = c;\n}\n";
    assert_eq!(rows(source), vec![(53, 1)]);
}

/// The access inside a const collection anchors the same code (the
/// element faces — `non_constant_list_element` and siblings — are separate
/// codes this pin ignores).
#[test]
fn nonconst_field_inside_collection_reports() {
    let source = "class C {\n  static int c = 1;\n}\nconst l = [C.c];\n";
    assert_eq!(rows(source), vec![(43, 3)]);
}

/// Clean shapes stay silent: const fields, literals, const creations,
/// arithmetic over constants, adjacent strings.
#[test]
fn constant_shapes_stay_silent() {
    let source = "class C {\n  static const int k = 2;\n}\nconst a = C.k;\nconst b = 1 + 2 * 3;\nconst l = [1, C.k];\nconst s = 'a' 'b';\n";
    assert_eq!(rows(source), Vec::<(u32, u32)>::new());
}
