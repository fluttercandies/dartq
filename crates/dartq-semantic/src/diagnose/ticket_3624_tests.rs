//! The #3624 pins — the `const_initialized_with_non_constant_value` census
//! residual (191 missing / 92 extra, census-11
//! `target/conformance/census-20261102-122042.report.tsv`): the collection
//! control-flow elements (`if`/`...`/`for`) were invisible to the const
//! faces — the capture dropped them at the literal close, so a `for`
//! element folded into the element list produced no `const_eval_for_element`
//! leaf, a bare spread operand vanished from a set-resolved `{...list}`,
//! and a dead `if` branch reported no element-code row.
//!
//! The fixes mirror upstream `_ConstLiteralVerifier.verify`
//! (constant_verifier.dart) and `_buildListConstant`/`_buildMapConstant`/
//! `_buildSetConstant` (evaluation.dart):
//!
//! - `const_eval.rs` stages the control elements as structured `Expr`
//!   nodes — `IfElement { cond, then, els }`, `Spread { operand }`,
//!   `Faulted(ConstFault::ForElement)` — through `ctl_marks`
//!   (`begin_ctl_element`/`close_ctl_element`), so the element faces read
//!   the same shapes the oracle's `CollectionElement` walk reads.
//! - `const_eval_faces.rs`'s `check_const_elements` runs the pure
//!   verifier (`verify_const_element`): an `if` element evaluates its
//!   condition (a non-bool constant faults `non_bool_condition` at
//!   `element.expression`), verifies the live branch, and scans the dead
//!   branch's not-potentially-constant shapes for element-code rows;
//!   a `...` element evaluates its operand against the element code;
//!   a plain element keeps the site default.
//! - The var-level verdict still anchors the site default:
//!   `finish_const_initializer` emits `const_init` at the whole literal
//!   for a top-level/field `for` element (the oracle's
//!   `genericError(node.initializer)` path — a local const reports the
//!   element row alone), while `push_literal_value` pushes the fault leaf
//!   at the literal span so the declaration's own verdict wraps it.
//!
//! Every pin is the oracle's 3.13.7 machine row set for the fixture, at
//! the corpus span; anchors locate with `source.find` — no hand-counted
//! offsets. Registered through `diagnose/tests.rs`'s `#[path]` block.

use super::test_rig::{CORE_PRELUDE, report_ops_bare};
use super::*;

/// `(code, offset, written text at the row)` — the span-exact projection:
/// code + offset + text pin a row's code, anchor, and length together.
/// The vector is sorted by `(code, offset, text)`, so expected rows are
/// listed in code-name order.
fn oracle_rows<'a>(report: &FileReport, source: &'a str) -> Vec<(&'static str, u32, &'a str)> {
    let mut rows: Vec<(&'static str, u32, &'a str)> = report
        .diagnostics
        .iter()
        .map(|d| {
            (
                d.code.name,
                d.range.offset,
                &source[d.range.offset as usize..d.range.end() as usize],
            )
        })
        .collect();
    rows.sort();
    rows
}

/// A top-level `const x = [for ...]` reports `const_eval_for_element` at
/// the whole `for` element AND the site default
/// `const_initialized_with_non_constant_value` at the literal — the
/// oracle's `visitVariableDeclaration` `_reportError` wraps the failed
/// initializer evaluation at `node.initializer` (the literal), the
/// element's own `InvalidConstant` carries the allow-listed
/// `constEvalForElement` code at the element.
#[test]
fn for_element_reports_the_element_and_the_literal() {
    // `report_ops_bare` + `CORE_PRELUDE`: `i < 3`/`i++` need `int`'s
    // `operator <`/`+` — the bare `report` rig declares no members, so the
    // parts would fault `undefined_operator` (a fixture artifact, not the
    // oracle's rows).
    let fixture = "const x = [for (var i = 0; i < 3; i++) i];\n";
    let source = format!("{CORE_PRELUDE}{fixture}");
    let literal = source.find("[for").expect("fixture has the list") as u32;
    let for_element = source.find("for (").expect("fixture has the for") as u32;
    let rep = report_ops_bare(&source);
    assert_eq!(
        oracle_rows(&rep, &source),
        vec![
            (
                "const_eval_for_element",
                for_element,
                "for (var i = 0; i < 3; i++) i"
            ),
            (
                "const_initialized_with_non_constant_value",
                literal,
                "[for (var i = 0; i < 3; i++) i]",
            ),
        ],
        "{:#?}",
        oracle_rows(&rep, &source)
    );
}

/// A local `const z = [for ...]` reports the element row ALONE — the
/// oracle's local `_reportError` carries the element's `InvalidConstant`
/// verbatim (the allow-listed `constEvalForElement` overrides the site
/// default at the element), and no `const_init` twin fires at the
/// literal. The declared `z` is never read, so the oracle's
/// `unused_local_variable` joins it (the `for_const_error_test_*` corpus
/// rows pair the same two codes).
#[test]
fn local_for_element_reports_the_element_alone() {
    let fixture = "void f() {\n  const z = [for (var i = 0; i < 3; i++) i];\n}\n";
    let source = format!("{CORE_PRELUDE}{fixture}");
    let for_element = source.find("for (").expect("fixture has the for") as u32;
    let z_decl = source.find("z =").expect("fixture declares z") as u32;
    let rep = report_ops_bare(&source);
    assert_eq!(
        oracle_rows(&rep, &source),
        vec![
            (
                "const_eval_for_element",
                for_element,
                "for (var i = 0; i < 3; i++) i"
            ),
            ("unused_local_variable", z_decl, "z"),
        ],
        "{:#?}",
        oracle_rows(&rep, &source)
    );
}

/// `const s = {...list}` where the literal resolves set (the spread
/// operand is a `List`): the bare spread operand is a set element the
/// parser's `has_set_entry` could not see — a `List` operand is a clean
/// spread, so the literal folds silent.
#[test]
fn spread_decided_set_literal_with_a_clean_operand_is_silent() {
    // `CORE_PRELUDE`: `<int>` needs the resolved `int` class — bare
    // `report` leaves it a non-type (`non_type_as_type_argument`).
    let source = format!("{CORE_PRELUDE}const list = <int>[1, 2];\nconst s = {{...list}};\n");
    let rep = report_ops_bare(&source);
    assert_eq!(
        oracle_rows(&rep, &source),
        vec![],
        "{:#?}",
        oracle_rows(&rep, &source)
    );
}

/// `const s = {...x}` where the operand is a non-constant `List` (an
/// `Iterable`, so the spread still decides the literal set): the oracle's
/// `test_const_spread_final` rows are `non_constant_set_element` at the
/// operand plus the site default `const_init` at the operand leaf — the
/// element's own `InvalidConstant` is not allow-listed, so the literal's
/// `_reportError` wraps it as the site default at the operand. The
/// spread's `Expr::Spread` node keeps the operand on the staged list (the
/// pre-wrap `split_set_or_map` dropped bare operands, so this literal
/// element-checked nothing). An `int`-typed operand would not exercise
/// the same oracle path — a non-`Iterable` spread leaves `{...nc}`
/// ambiguous (`ambiguous_set_or_map_literal_either`, no element check).
#[test]
fn spread_decided_set_literal_with_a_nonconstant_operand_reports() {
    let fixture = "final List x = [];\nconst s = {...x};\n";
    let source = format!("{CORE_PRELUDE}{fixture}");
    let operand = source.find("x}").expect("fixture has the operand") as u32;
    let rep = report_ops_bare(&source);
    assert_eq!(
        oracle_rows(&rep, &source),
        vec![
            ("const_initialized_with_non_constant_value", operand, "x",),
            ("non_constant_set_element", operand, "x"),
        ],
        "{:#?}",
        oracle_rows(&rep, &source)
    );
}

/// `const l = [if (false) nonConst]`: the `if` condition evaluates to a
/// known `false`, so the oracle's verifier scans the dead then-branch's
/// not-potentially-constant shapes — `nonConst` reports the element code
/// (`non_constant_list_element`) at its own leaf, and the literal's own
/// verdict stays silent (the live branch contributes nothing). The dead
/// branch itself also reads `dead_code` at its expression (upstream
/// `test_deadBlock_ifElement`: the flow layer marks the skipped element's
/// whole subtree dead — `dead_code` at `nonConst`, not the element-code
/// alone).
#[test]
fn dead_if_branch_reports_the_element_code_only() {
    let fixture = "var nonConst = 1;\nconst l = [if (false) nonConst];\n";
    let source = format!("{CORE_PRELUDE}{fixture}");
    let operand = source.find("nonConst]").expect("fixture has the leaf") as u32;
    let rep = report_ops_bare(&source);
    assert_eq!(
        oracle_rows(&rep, &source),
        vec![
            ("dead_code", operand, "nonConst"),
            ("non_constant_list_element", operand, "nonConst"),
        ],
        "{:#?}",
        oracle_rows(&rep, &source)
    );
}

/// `const l = [if (true) nonConst]`: the live then-branch's leaf fails —
/// the element code reports at `nonConst` AND the site default
/// `const_init` anchors at the operand leaf (the literal's own verdict
/// fails on the live branch's expression, not the element wrapper).
#[test]
fn live_if_branch_reports_the_element_code_and_const_init() {
    let fixture = "var nonConst = 1;\nconst l = [if (true) nonConst];\n";
    let source = format!("{CORE_PRELUDE}{fixture}");
    let operand = source.find("nonConst]").expect("fixture has the leaf") as u32;
    let rep = report_ops_bare(&source);
    assert_eq!(
        oracle_rows(&rep, &source),
        vec![
            (
                "const_initialized_with_non_constant_value",
                operand,
                "nonConst",
            ),
            ("non_constant_list_element", operand, "nonConst"),
        ],
        "{:#?}",
        oracle_rows(&rep, &source)
    );
}

/// `const l = [if (1) x]`: a non-bool constant condition faults
/// `non_bool_condition` at the condition expression (upstream
/// `conditionValue == null` → `InvalidConstant.forEntity(entity:
/// element.expression)` — the entity is the expression, not the whole
/// element). The literal's own verdict carries the same allow-listed
/// fault (pushed at the literal span), so the declaration's
/// `_reportError` swaps the site default for the fault's code — the one
/// `non_bool_condition` row at the condition is the whole report.
#[test]
fn non_bool_if_condition_faults_at_the_condition() {
    let source = "const l = [if (1) 2];\n";
    let cond = source.find('1').expect("fixture has the condition") as u32;
    let rep = report(source);
    assert_eq!(
        oracle_rows(&rep, source),
        vec![("non_bool_condition", cond, "1")],
        "{:#?}",
        oracle_rows(&rep, source)
    );
}

/// `const x = throw 'e'`: the oracle anchors the failure at the whole
/// `throw expr` node — the const_init row spans the `throw` keyword
/// through the operand's end, not the keyword alone.
#[test]
fn throw_anchors_the_whole_throw_expression() {
    let source = "const x = throw 'e';\n";
    let throw_start = source.find("throw").expect("fixture has the throw") as u32;
    let rep = report(source);
    assert_eq!(
        oracle_rows(&rep, source),
        vec![(
            "const_initialized_with_non_constant_value",
            throw_start,
            "throw 'e'",
        ),],
        "{:#?}",
        oracle_rows(&rep, source)
    );
}

/// `const d = dynamic` is silent: a bare `dynamic` read is a valid
/// `Type` literal constant (the resolver's `dynamic` pseudo-element is a
/// `TypeLiteral`), so no const-init row fires — the pre-fix bare-name
/// path classified it `Unresolved`.
#[test]
fn dynamic_type_literal_is_a_valid_constant() {
    let source = "const d = dynamic;\n";
    let rep = report(source);
    assert_eq!(
        oracle_rows(&rep, source),
        vec![],
        "{:#?}",
        oracle_rows(&rep, source)
    );
}

/// `const v = .undef` under a declaration shorthand context that cannot
/// answer the member: the resolution face emits
/// `dot_shorthand_undefined_member` at the bare selector AND the const
/// root still owes the oracle's `invalidConstant` at the WHOLE shorthand
/// (the dot included) — the pre-fix `dot_shorthand_context` suppression
/// swallowed the const-init twin entirely.
#[test]
fn failed_shorthand_reports_const_init_at_the_whole_shorthand() {
    let source = "const v = .undef;\n";
    let shorthand = source.find(".undef").expect("fixture has the shorthand") as u32;
    let rep = report(source);
    assert_eq!(
        oracle_rows(&rep, source),
        vec![
            (
                "const_initialized_with_non_constant_value",
                shorthand,
                ".undef",
            ),
            ("dot_shorthand_missing_context", shorthand, ".undef"),
        ],
        "{:#?}",
        oracle_rows(&rep, source)
    );
}
