//! The C3b pins: the condition-constant feed's mutable-local half, and
//! the `??=` RHS entry-dead row.
//!
//! # Mutable-local literal fold
//!
//! A non-`late` local declared with a boolean-literal initializer now
//! carries the proven constant on [`VarMeta::const_bool`] — not only a
//! `final` one (#2905 P2) but a `var`/`bool` one too, matching upstream
//! `SsaNode.conditionVariableState` (`flow_analysis.dart:7568-7572`,
//! `_initialize`): a non-trivial `booleanLiteral` `ExpressionInfo` lands
//! on the SSA node for *any* non-`late` local, `final` or mutable. The
//! soundness arm is `restoreConditionVariableState`
//! (`flow_analysis.dart:4984+`, language#1274): it rebases the constant
//! through `rebaseForward` only while the SSA node identity holds, which
//! is exactly the `apply_write` invariant this level already maintains —
//! every write (`=`, `+=`, `++`, `for-in`, pattern binding, closure
//! writes replayed through the same `Interp`) clears `const_bool`, so the
//! constant can never outlive a write. `late` stays excluded per the
//! `_initialize` `isLate` arm.
//!
//! # `??=` RHS entry-dead
//!
//! `x ??= y` on a provably non-nullable LHS (the
//! `ifNullExpression_rightBegin` `nonNullable` arm,
//! `flow_analysis.dart:5842-5864`) kills the RHS path: the operand's
//! first token is the official's first dead node
//! (dead_code_verifier.dart's `parent is AssignmentExpression` arm does
//! not re-anchor to the operator — the interval covers operand start
//! through the containing node's end, `1;` for a statement). The skip
//! state restores at `Ev::Assign { if_null }`; an *exit*-dead RHS
//! (`x ??= throw e`) restores live with no row (the skip path itself was
//! never dead), and an outer dead region keeps its own interval.

use crate::facts::FlowFact;
use crate::forward::SharedFlow;
use crate::record::Flow;
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_parser::ParserFeatures;
use dartq_scanner::{ScanConfig, TokenTable};

/// Dead-code facts of `source` as source text (the production channel:
/// `set_sound_flow` reads the settled feature set).
fn dead_of(source: &str) -> Vec<String> {
    let result = dartq_scanner::scan(
        source,
        ScanConfig {
            enable_triple_shift: true,
            enable_augmentations: true,
        },
        dartq_source::FileTable::new().intern(std::path::Path::new("test.dart")),
    );
    assert!(result.diagnostics.is_empty(), "{source:?}");
    let tokens: &'static TokenTable = Box::leak(Box::new(result.tokens));
    let features = ParserFeatures::from_feature_set(FeatureSet::for_language_version(
        LanguageVersion::new(3, 15),
    ));
    let mut flow = SharedFlow(std::rc::Rc::new(std::cell::RefCell::new(Flow::new(
        tokens, source,
    ))));
    flow.0
        .borrow_mut()
        .set_sound_flow(features.sound_flow_analysis);
    let mut parser = dartq_parser::Parser::new(source, tokens, features, &mut flow);
    parser.parse_unit();
    drop(parser);
    flow.0
        .borrow_mut()
        .take_facts()
        .into_iter()
        .filter_map(|fact| match fact {
            FlowFact::DeadCode { span } => Some(
                source[span.range.offset as usize
                    ..(span.range.offset + span.range.length) as usize]
                    .to_string(),
            ),
            _ => None,
        })
        .collect()
}

/// A typed mutable local's literal folds a dead `if` arm — the census's
/// `bool b = true; [if (!b) 1]` shape.
#[test]
fn mutable_local_literal_folds_collection_if() {
    assert_eq!(
        dead_of("f() { bool b = true; var l = [if (!b) 1]; }"),
        ["1"]
    );
}

/// Same fold through a statement `if` — the losing block dies.
#[test]
fn mutable_local_literal_folds_stmt_if() {
    assert_eq!(dead_of("f() { bool b = true; if (!b) {} else {} }"), ["{}"]);
}

/// An untyped `var` initializer folds identically (the constant rides
/// the literal, not the spelling).
#[test]
fn mutable_var_literal_folds() {
    assert_eq!(
        dead_of("f() { var b = true; var l = [if (!b) 1]; }"),
        ["1"]
    );
}

/// A write clears the fold — the `apply_write` invariant upstream's
/// `restoreConditionVariableState` enforces through the SSA node's
/// identity.
#[test]
fn mutable_local_write_clears_fold() {
    assert_eq!(
        dead_of("f(bool c) { bool b = true; b = c; if (!b) {} else {} }"),
        Vec::<String>::new()
    );
}

/// A closure's write clears too — closures replay through the same
/// `Interp`, so `apply_write` sees them; the constant must not survive.
#[test]
fn mutable_local_closure_write_clears_fold() {
    assert_eq!(
        dead_of(
            "f() { bool b = true; var g = () { b = false; }; var l = [if (!b) 1]; }"
        ),
        Vec::<String>::new()
    );
}

/// `late` never carries the constant (`_initialize`'s `isLate` arm).
#[test]
fn late_local_literal_stays_live() {
    assert_eq!(
        dead_of("f() { late bool b = true; if (!b) {} else {} }"),
        Vec::<String>::new()
    );
}

/// A non-literal initializer (`g()`) is not a proven constant — both
/// arms live.
#[test]
fn mutable_local_nonliteral_stays_live() {
    assert_eq!(
        dead_of("bool g() => true; f() { bool b = g(); if (!b) {} else {} }"),
        Vec::<String>::new()
    );
}

/// The mutable fold feeds `||`'s constant-condition face: `bq || b`
/// kills the RHS (the `booleanLiteral` `ExpressionInfo` is non-trivial on
/// `||` too).
#[test]
fn mutable_local_literal_folds_or_rhs() {
    assert_eq!(
        dead_of("f(bool b) { var bq = true; var x = bq || b; }"),
        ["|| b"]
    );
}

/// The `final` face still folds — the regression pin for #2905 P2's
/// original face.
#[test]
fn final_local_still_folds() {
    assert_eq!(dead_of("f() { final b = true; if (!b) {} else {} }"), ["{}"]);
}

/// `int x; x ??= 1` — non-nullable LHS kills the RHS entry. The interval
/// covers the operand through the containing statement's end
/// (dead_code_verifier's `AssignmentExpression` arm does not re-anchor to
/// `??=`).
#[test]
fn if_null_assign_rhs_dead_on_nonnull_local() {
    assert_eq!(dead_of("f() { int x = 0; x ??= 1; }"), ["1;"]);
}

/// A nullable LHS keeps both paths live.
#[test]
fn if_null_assign_rhs_live_on_nullable_local() {
    assert_eq!(dead_of("f(int? x) { x ??= 1; }"), Vec::<String>::new());
}

/// An exit-dead RHS (`throw`) restores live — the skip path was never
/// dead, so no dead-code row and no leak of the killed state.
#[test]
fn if_null_assign_throw_rhs_no_row() {
    assert_eq!(
        dead_of("f(int? x) { x ??= throw 'e'; } print(1);"),
        Vec::<String>::new()
    );
}

/// Inside an already-dead region the whole statement is the dead node —
/// the `??=` restore must not re-anchor or extend it.
#[test]
fn if_null_assign_inside_dead_region() {
    assert_eq!(dead_of("f(int x) { return; x ??= 1; }"), ["x ??= 1;"]);
}
