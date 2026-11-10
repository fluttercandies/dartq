//! The #3612 pins — the dot-shorthand CONTEXT-INFERENCE faces: the read
//! arm's own `dot_shorthand_missing_context` verdict and the positions
//! whose visiting schema the sweep re-derives.
//!
//! # The wrong fact
//!
//! #3433/#3568 left the read arm silent under every context that is not
//! an accessible `Named` head (upstream's verdict there is
//! `dotShorthandMissingContext.at(node)` — the WHOLE shorthand span, not
//! the selector), and a set of positions — conditional branches, unary
//! operands, `++`/`--` chains, collection elements, map values, statement
//! shapes, unannotated returns, negation heads, alias-folded formals —
//! never armed a context at all.
//!
//! # The contract
//!
//! - Read arm, non-interface context (none/`_`, `dynamic`, `Never`,
//!   `void`, a function type, a private head this unit cannot see):
//!   `dot_shorthand_missing_context` from the dot through the selector's
//!   end (`property_element_resolver.dart:129`).
//! - Invocation arm, same contexts: `dot_shorthand_undefined_member`
//!   with the visiting schema text (`'_'`, `'bool'`, `'void'`,
//!   `'FutureOr<void>'`, a folded alias's `'_C'`).
//! - The LOOKUP context folds aliases and takes the futureOr-base
//!   (`take(FutureOr<C>)` resolves on `C`); the weak schema display
//!   keeps the written spelling (`FutureOr<void> v = .id()` reports
//!   `'FutureOr<void>'`).
//! - A conditional's branches inherit the conditional's own schema:
//!   `if (c == x ? .a : y)` arms `bool`; a declaration initializer's
//!   conditional arms the annotation.
//!
//! Oracle provenance: `dart analyze` 3.13.7 — dot_shorthands
//! `equality_A02_t01/t02` (conditionals, collections, map values),
//! `grammar_A05_t01/t02` (negation, unary), `no_context`
//! `simple_identifier_no_context_error_test` (statement shapes),
//! `type_inference_A03_t01` (unannotated return), `private` (alias
//! fold + cross-library privacy), `semantics_A02_t01` (weak schema
//! displays), `DotShorthandConstructorInvocationResolutionTest/
//! test_postfixOperator` (`++`/`--` chains), `patterns_A02_t01` (a
//! `case` pattern's logical operands read the scrutinee, not `bool`).

use super::test_rig::{CORE_PRELUDE, RigImport, code_names, report_ops, report_ops_imports, rows};
use crate::diagnose::FileReport;

/// The absolute offset of the first `needle` in `fixture` as the rig
/// analyzes it: `report_ops` prefixes [`CORE_PRELUDE`], so every
/// fixture-local offset shifts by the prelude's length.
fn at(fixture: &str, needle: &str) -> u32 {
    let hit = fixture
        .find(needle)
        .unwrap_or_else(|| panic!("{needle:?} not in fixture:\n{fixture}"));
    u32::try_from(CORE_PRELUDE.len() + hit).expect("fixture offsets fit u32")
}

/// The `dot_shorthand_undefined_member` rows as `(offset, length)` pairs,
/// ascending — the selector-span anchors.
fn undefined_rows(report: &FileReport) -> Vec<(u32, u32)> {
    let mut at: Vec<(u32, u32)> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "dot_shorthand_undefined_member")
        .map(|d| (d.range.offset, d.range.end() - d.range.offset))
        .collect();
    at.sort_unstable();
    at
}

/// The `dot_shorthand_missing_context` rows as `(offset, length)` pairs,
/// ascending — the whole-shorthand anchors (the dot through the
/// selector's end).
fn missing_rows(report: &FileReport) -> Vec<(u32, u32)> {
    let mut at: Vec<(u32, u32)> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "dot_shorthand_missing_context")
        .map(|d| (d.range.offset, d.range.end() - d.range.offset))
        .collect();
    at.sort_unstable();
    at
}

/// Whether `report` carries any dot-shorthand row.
fn has_shorthand_row(report: &FileReport) -> bool {
    code_names(report)
        .iter()
        .any(|name| name.starts_with("dot_shorthand_"))
}

// —— read arm, non-interface contexts: the whole-span
// —— `dot_shorthand_missing_context` verdict (test_error_context_invalid
// —— `C Function() c = .member;`, semantics_A02's weak declarations).
#[test]
fn read_arm_reports_missing_context_on_weak_contexts() {
    // A function-type annotation: never an interface head.
    let source = "class C {}\nvoid f() { C Function() c = .member; }\n";
    let r = report_ops(source);
    assert_eq!(
        missing_rows(&r),
        vec![(at(source, ".member"), 7)],
        "a function-type context reports the whole span: {:?}",
        rows(&r)
    );

    // `dynamic`: the schema is known, the lookup is not.
    let source = "void f() { dynamic v = .member; }\n";
    let r = report_ops(source);
    assert_eq!(
        missing_rows(&r),
        vec![(at(source, ".member"), 7)],
        "a dynamic context reports the whole span: {:?}",
        rows(&r)
    );

    // `var`: the initializer's own inferred schema is unknown.
    let source = "void f() { var v = .member; }\n";
    let r = report_ops(source);
    assert_eq!(
        missing_rows(&r),
        vec![(at(source, ".member"), 7)],
        "a `var` context reports the whole span: {:?}",
        rows(&r)
    );

    // The invocation twins keep their schema spellings (semantics_A02:
    // 'void' through a declared void return, 'dynamic' through the
    // written declaration).
    let source = "void f() { dynamic v = .dd(); }\nvoid g() { return .dg(); }\n";
    let r = report_ops(source);
    assert_eq!(
        undefined_rows(&r),
        vec![(at(source, "dd("), 2), (at(source, "dg("), 2)],
        "the weak invoke arms spell their schemas: {:?}",
        rows(&r)
    );
    assert!(missing_rows(&r).is_empty());
}

// —— conditional branches inherit the conditional's own schema
// —— (equality_A02_t01 46:28 `'bool'`, t02 50:25 `'bool'`).
#[test]
fn conditional_branches_read_the_conditionals_schema() {
    let source = "class C { C(int x); }\n\
                  void f(bool condition) { if (C(0) == condition ? .intOne : C(1)) {} }\n\
                  void g(bool condition) { if (condition ? C(1) : .intTwo) {} }\n\
                  void h(bool condition) { if (C(0) == condition ? .id(0) : C(1)) {} }\n";
    let r = report_ops(source);
    assert_eq!(
        undefined_rows(&r),
        vec![
            (at(source, "intOne"), 6),
            (at(source, "intTwo"), 6),
            (at(source, "id("), 2),
        ],
        "both branches arm the condition's `bool` schema: {:?}",
        rows(&r)
    );
    assert!(missing_rows(&r).is_empty());
}

// —— a declaration initializer's conditional arms the ANNOTATION
// —— (the boundary walk classifies the declarator `=`).
#[test]
fn conditional_under_declaration_arms_the_annotation() {
    let source = "enum E { a, b }\n\
                  void f(bool cond) { E e = cond ? .z : E.a; }\n\
                  void g(bool cond) { E e = cond ? .a : E.a; }\n";
    let r = report_ops(source);
    assert_eq!(
        undefined_rows(&r),
        vec![(at(source, "z :"), 1)],
        "the annotation resolves the branch: {:?}",
        rows(&r)
    );
}

// —— negation heads: `!.a` is a shorthand under `bool`, not a postfix
// —— assert of a receiver (grammar_A05_t01 60:11 `'bool'`).
#[test]
fn negation_head_arms_bool() {
    let source = "void f(Object o) { o = !.getterTrue; o = !.methodTrue(); }\n";
    let r = report_ops(source);
    assert_eq!(
        undefined_rows(&r),
        vec![
            (at(source, "getterTrue;"), 10),
            (at(source, "methodTrue("), 10),
        ],
        "a negation's operand arms `bool` for both arms: {:?}",
        rows(&r)
    );
    // `x!.a` stays a receiver access — no shorthand row at all.
    let source = "class C { static int a = 1; }\nvoid f(C? x) { int y = x!.a; }\n";
    let r = report_ops(source);
    assert!(
        !has_shorthand_row(&r),
        "a postfix null-assert is not a shorthand: {:?}",
        rows(&r)
    );
}

// —— unary `-`/`~` and `++`/`--` operands: the unknown schema
// —— (grammar_A05_t02 66:11 `'_'`; test_postfixOperator 5:10/5:12 `'_'`).
#[test]
fn unary_and_incdec_operands_arm_the_unknown_schema() {
    let source = "void f(Object o) { o = -.one; o = -.two(); }\n";
    let r = report_ops(source);
    assert_eq!(missing_rows(&r), vec![(at(source, ".one"), 4)]);
    assert_eq!(undefined_rows(&r), vec![(at(source, "two("), 3)]);

    let source = "class C {}\nvoid f() { C c = .new()++; C d = ++.new(); }\n";
    let r = report_ops(source);
    assert_eq!(
        undefined_rows(&r),
        vec![(at(source, "new()"), 3), (at(source, "new();"), 3)],
        "the `++`/`--` operand schema is unknown: {:?}",
        rows(&r)
    );
}

// —— statement shapes (no_context): the expression statement's own
// —— shorthand arms nothing anywhere — the read arm reports
// —— `missing_context` at the whole span.
#[test]
fn statement_shapes_report_missing_context() {
    let source = "void f() { .red = 1; .one!; (.green); }\n";
    let r = report_ops(source);
    assert_eq!(
        missing_rows(&r),
        vec![
            (at(source, ".red"), 4),
            (at(source, ".one"), 4),
            (at(source, ".green"), 6),
        ],
        "assignment/negated/grouped statement shorthands: {:?}",
        rows(&r)
    );
    assert!(undefined_rows(&r).is_empty());

    let source = "void f() { .red++; }\n";
    let r = report_ops(source);
    assert_eq!(
        missing_rows(&r),
        vec![(at(source, ".red"), 4)],
        "a compound-target statement reports the whole span: {:?}",
        rows(&r)
    );

    let source = "void f() async { await .red; }\n";
    let r = report_ops(source);
    assert_eq!(
        missing_rows(&r),
        vec![(at(source, ".red"), 4)],
        "a statement-level `await` shorthand arms nothing: {:?}",
        rows(&r)
    );
}

// —— collection elements and map values under an equality operand: the
// —— element schema is unknown (equality_A02_t01 50/54/58).
#[test]
fn collection_elements_and_map_values_arm_the_unknown_schema() {
    let source = "class C { C(int x); }\n\
                  void f() { if ([C(0)] == [.id(1)]) {} }\n\
                  void g() { if ({C(0)} != {.f2(0)}) {} }\n\
                  void h() { if ({\"key\": C(0)} != {\"key\": .intOne}) {} }\n";
    let r = report_ops(source);
    assert_eq!(
        undefined_rows(&r),
        vec![(at(source, "id("), 2), (at(source, "f2("), 2)]
    );
    assert_eq!(missing_rows(&r), vec![(at(source, ".intOne"), 7)]);
}

// —— unannotated return: the inferring body's return schema is unknown
// —— (type_inference_A03_t01 18:13 `'_'`); a DECLARED return spells
// —— itself (semantics_A02 57:14 `'void'`).
#[test]
fn return_positions_arm_declared_or_unknown_schema() {
    let source = "f() { return .new(); }\nvoid g() { return .id(); }\n";
    let r = report_ops(source);
    assert_eq!(
        undefined_rows(&r),
        vec![(at(source, "new("), 3), (at(source, "id();"), 2)],
        "unannotated reports '_', declared void reports 'void': {:?}",
        rows(&r)
    );
}

// —— alias-folded formals: the context is the formal's instantiated
// —— type — `typedef Pub = C` resolves on `C` (private_test's
// —— contextAlias family).
#[test]
fn formal_alias_folds_to_its_element() {
    let source = "class C {}\n\
                  typedef Pub = C;\n\
                  void take(Pub c) {}\n\
                  void f() { take(.m()); }\n";
    let r = report_ops(source);
    assert_eq!(
        undefined_rows(&r),
        vec![(at(source, "m("), 1)],
        "the folded head answers the lookup: {:?}",
        rows(&r)
    );
    let text = rows(&r).join("\n");
    assert!(
        text.contains("context type 'C'."),
        "the miss spells the folded element, not the alias: {text}"
    );
}

// —— cross-library privacy: an imported alias's private right-hand side
// —— (`typedef Public_C = _C;` in a sibling unit) is NOT an accessible
// —— interface type here — the read arm reports `missing_context`, the
// —— invocation arm the folded schema `'_C'` (private_class_error's
// —— other-library rows). An in-file private head stays accessible.
#[test]
fn cross_library_private_head_is_not_an_interface_context() {
    let fixture = "// the rig auto-imports 'package:t/priv.dart'\n\
                   class _Local {}\n\
                   void take(Public_C c) {}\n\
                   void takeLocal(_Local c) {}\n\
                   void f() { take(.m); take(.m2()); takeLocal(.m3); }\n";
    let report = report_ops_imports(
        fixture,
        &[RigImport {
            uri: "package:t/priv.dart",
            prefix: None,
            source: "class _C {}\ntypedef Public_C = _C;\n",
        }],
    );
    let dot_m = at(fixture, "take(.m);") + u32::try_from("take(".len()).unwrap();
    assert_eq!(
        missing_rows(&report),
        vec![(dot_m, 2)],
        "a foreign private head reports the whole span: {:?}",
        rows(&report)
    );
    let text = rows(&report).join("\n");
    assert!(
        text.contains("isn't defined for the context type '_C'."),
        "the invoke arm spells the folded foreign schema: {text}"
    );
    assert!(
        text.contains("isn't defined for the context type '_Local'."),
        "an in-file private head stays an interface context: {text}"
    );
}

// —— a `case` pattern's logical operands read the scrutinee's matched
// —— type, never `bool` (patterns_A02_t01: `if (e case .one || .two)`
// —— resolves silently).
#[test]
fn case_pattern_logical_operands_read_the_scrutinee() {
    let source = "enum E { one, two }\nvoid f(E e) { if (e case .one || .two) {} }\n";
    let r = report_ops(source);
    assert!(
        !has_shorthand_row(&r),
        "the scrutinee resolves the pattern's operands: {:?}",
        rows(&r)
    );
}

// —— the lookup folds futureOr but the weak display keeps the written
// —— spelling (semantics_A02_t01 73:24 'FutureOr<void>', 93:21 'C').
#[test]
fn future_or_context_folds_for_lookup_keeps_spelling_for_display() {
    let source = "class C {}\n\
                  void f() { FutureOr<void> v6 = .id(); FutureOr<C> v3 = .instance(); }\n";
    let r = report_ops(source);
    let text = rows(&r).join("\n");
    assert!(
        text.contains("context type 'FutureOr<void>'."),
        "the weak arm spells the written schema: {text}"
    );
    assert!(
        text.contains("context type 'C'."),
        "the lookup takes the futureOr-base: {text}"
    );
}

// —— the const-local enum regression guard (test_const_enum): an in-file
// —— enum's const initializer resolves its shorthand silently.
#[test]
fn const_local_enum_initializer_resolves_silently() {
    let source = "enum Color { red, green, blue }\nvoid f() { const Color c = .blue; }\n";
    let r = report_ops(source);
    assert!(
        !has_shorthand_row(&r),
        "an in-file enum's const initializer resolves: {:?}",
        rows(&r)
    );
}
