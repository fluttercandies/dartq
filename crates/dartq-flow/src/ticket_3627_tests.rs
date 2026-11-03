//! #3627 pins — the condition-variable replay (language#1274, official
//! `restoreConditionVariableState`,
//! `pkg/_fe_analyzer_shared/lib/src/flow_analysis/flow_analysis.dart:6067-6089`
//! via `variableRead` at :8291-8323): a bool local bound from a
//! null-comparison split replays the saved arms when it is read as a
//! condition operand, so the `&&`/`||` right operand sees the tested
//! variable's promotion exactly like the direct comparison.
//!
//! Upstream faces pinned here (each asserted on the `flags` read's
//! [`FlowFact::ReadType`] row — `Flags` is the promoted face, `Flags?`
//! the declared one):
//!
//! 1. `hasFlags && flags.contains(...)` — the `&&` right operand runs in
//!    the condition variable's true arm: `flags` reads non-null.
//! 2. `!hasFlags || flags.contains(...)` — upstream `logicalNot_end`
//!    inverts the operand's `ExpressionInfo` (flow_analysis.dart:7336),
//!    so the `||` right operand runs in the inverted replay's false arm:
//!    `flags` reads non-null. The `!` must carry the condition-variable
//!    linkage — a bare `Expr` rebuild drops it.
//! 3. `hasFlags || flags.contains(...)` — the `||` right operand runs in
//!    the FALSE arm: no positive split inherited, `flags` stays nullable
//!    (the oracle still reports `unchecked_use_of_nullable_value` there).
//! 4. `b && flags.contains` with `b` bound from `flags == null` — the
//!    stored value `true` means `flags` is null: the right operand keeps
//!    `flags` nullable, never promotes it.
//! 5. `hasFlags = c` between the test and the use — the write invalidates
//!    the binding (the official stores the state on the variable's value
//!    version; any write hands a fresh one), so the replay stays out and
//!    `flags` reads nullable.
//! 6. `flags = null` between the test and the use — the tested variable's
//!    own write-version moved, so the replay is unsound and `flags` reads
//!    nullable.
//! 7. `for (hasFlags in ...)` — a non-assignment write shape invalidates
//!    the binding exactly like the plain `=` face (the upstream version
//!    check covers every write, not only `Ev::Assign`).

use crate::facts::FlowFact;
use crate::forward::SharedFlow;
use crate::record::Flow;
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_parser::ParserFeatures;
use dartq_scanner::{ScanConfig, TokenTable};

/// `(offset, ty)` pairs of `ReadType` facts, source-ordered.
fn read_types(source: &str) -> Vec<(u32, crate::ty::Ty)> {
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
            FlowFact::ReadType { span, ty, .. } => Some((span.range.offset, ty)),
            _ => None,
        })
        .collect()
}

/// The `ReadType` at `needle`'s offset (the needle must occur exactly
/// once in `source`).
fn ty_at(source: &str, needle: &str) -> crate::ty::Ty {
    assert_eq!(source.matches(needle).count(), 1, "{needle:?} in {source:?}");
    let offset = u32::try_from(source.find(needle).expect("needle present"))
        .expect("offset fits u32");
    read_types(source)
        .into_iter()
        .find(|(at, _)| *at == offset)
        .map(|(_, ty)| ty)
        .unwrap_or_else(|| panic!("no ReadType at {needle:?} in {source:?}"))
}

/// The replay's true arm promotes the tested variable inside the `&&`
/// right operand — `flags.contains` reads `flags` as the non-null
/// `Flags`, not the declared `Flags?`.
#[test]
fn and_right_operand_replays_the_null_split() {
    let source = concat!(
        "class Flags { bool contains(String s) => false; }\n",
        "void sink(bool b) {}\n",
        "void f(Flags? flags) {\n",
        "  final hasFlags = flags != null;\n",
        "  sink(hasFlags && flags.contains('q'));\n",
        "}\n",
    );
    assert!(
        !ty_at(source, "flags.contains('q')").nullable,
        "the `&&` right operand must see `flags` promoted"
    );
}

/// The inverted replay: `!hasFlags` keeps the condition-variable linkage
/// (upstream `logicalNot_end` returns `operandInfo._invert()`), so the
/// `||` right operand — evaluated when `!hasFlags` is false, i.e. when
/// `hasFlags` held true — sees `flags` promoted.
#[test]
fn negated_condition_variable_replays_through_or() {
    let source = concat!(
        "class Flags { bool contains(String s) => false; }\n",
        "void sink(bool b) {}\n",
        "void f(Flags? flags) {\n",
        "  final hasFlags = flags != null;\n",
        "  sink(!hasFlags || flags.contains('x'));\n",
        "}\n",
    );
    assert!(
        !ty_at(source, "flags.contains('x')").nullable,
        "the `||` right operand under `!hasFlags` must see `flags` promoted"
    );
}

/// Negative control: a bare `hasFlags ||` does NOT hand its right operand
/// the positive null split — the right side runs when `hasFlags` was
/// false, where `flags` is still nullable.
#[test]
fn or_right_operand_keeps_the_false_arm() {
    let source = concat!(
        "class Flags { bool contains(String s) => false; }\n",
        "void sink(bool b) {}\n",
        "void f(Flags? flags) {\n",
        "  final hasFlags = flags != null;\n",
        "  sink(hasFlags || flags.contains('x'));\n",
        "}\n",
    );
    assert!(
        ty_at(source, "flags.contains('x')").nullable,
        "the `||` right operand must keep `flags` nullable"
    );
}

/// A condition variable bound from `flags == null` stores the null arm
/// in its true slot: `b && flags.contains` must not promote `flags` (the
/// stored `true` means `flags` was null).
#[test]
fn false_valued_condition_variable_does_not_promote() {
    let source = concat!(
        "class Flags { bool contains(String s) => false; }\n",
        "void sink(bool b) {}\n",
        "void f(Flags? flags) {\n",
        "  final isNull = flags == null;\n",
        "  sink(isNull && flags.contains('q'));\n",
        "}\n",
    );
    assert!(
        ty_at(source, "flags.contains('q')").nullable,
        "a `== null`-bound condition variable must not promote `flags`"
    );
}

/// A plain rewrite of the condition variable between the test and the
/// use invalidates the binding: the saved split belongs to the old
/// value, so `flags` reads nullable in the `&&` right operand.
#[test]
fn reassigned_condition_variable_does_not_replay() {
    let source = concat!(
        "class Flags { bool contains(String s) => false; }\n",
        "void sink(bool b) {}\n",
        "void f(Flags? flags, bool c) {\n",
        "  var hasFlags = flags != null;\n",
        "  hasFlags = c;\n",
        "  sink(hasFlags && flags.contains('q'));\n",
        "}\n",
    );
    assert!(
        ty_at(source, "flags.contains('q')").nullable,
        "a reassigned condition variable must not replay its stale split"
    );
}

/// A tested-variable write between the test and the use invalidates the
/// replay through the write-version check: `flags = null` moves its
/// version, so `hasFlags` replays nothing and `flags` stays nullable.
#[test]
fn tested_variable_write_invalidates_the_replay() {
    let source = concat!(
        "class Flags { bool contains(String s) => false; }\n",
        "void sink(bool b) {}\n",
        "void f(Flags? flags) {\n",
        "  final hasFlags = flags != null;\n",
        "  flags = null;\n",
        "  sink(hasFlags && flags.contains('q'));\n",
        "}\n",
    );
    assert!(
        ty_at(source, "flags.contains('q')").nullable,
        "a write to the tested variable must invalidate the replay"
    );
}

/// A non-`=` write shape invalidates the binding too: `for (hasFlags in
/// ...)` writes the condition variable through the for-in target face —
/// the upstream version change covers every write, so the stale split
/// must not replay into the `&&` right operand.
#[test]
fn for_in_target_write_invalidates_the_binding() {
    let source = concat!(
        "class Flags { bool contains(String s) => false; }\n",
        "void sink(bool b) {}\n",
        "void f(Flags? flags, List<bool> bs) {\n",
        "  var hasFlags = flags != null;\n",
        "  for (hasFlags in bs) {\n",
        "    sink(hasFlags && flags.contains('q'));\n",
        "  }\n",
        "}\n",
    );
    assert!(
        ty_at(source, "flags.contains('q')").nullable,
        "a for-in write to the condition variable must invalidate the replay"
    );
}
