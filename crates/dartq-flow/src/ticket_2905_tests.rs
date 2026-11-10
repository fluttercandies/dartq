//! The #2905 pins: the dead_code const-condition residual's four faces.
//!
//! P1 — literal non-nullability (`Expr::non_null_literal`): by-construction
//! non-null values (literals, `this`, bool-producing operators) carry the
//! opaque `Ty::UNKNOWN`, so the `??`/`?.` guaranteed-non-null kills
//! (official `_isGuaranteedNonNullWithSoundNullSafety`,
//! flow_analysis.dart:9217-9221) never fired on them. The flag feeds the
//! same faces the nullable-type classification drives.
//!
//! P2 — the constant-condition feed (`VarMeta::const_bool`): a `final` or
//! `const` local initialized from a `bool_value`-carrying fold replays its
//! proven truth value on every read, so `if (b)` sees a constant condition.
//! Writes clear the provenance (`apply_write`).
//!
//! P4 — the Never-receiver trailing-operand kill (official
//! `recordStaticType`'s `handleExit` on a bottom static type,
//! ast.dart:11146) plus the nested dead-arm window (`arm_dead_depth`):
//! an arm entered dead counts down its own contribution before the
//! `[operator, operand end)` row is weighed, so only strictly-outer arms
//! gate the row — matching upstream's per-arm `flowEnd(rightOperand)`
//! rows while inner polls fold into the enclosing dead window.

use crate::facts::FlowFact;
use crate::forward::SharedFlow;
use crate::record::Flow;
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_parser::ParserFeatures;
use dartq_scanner::{ScanConfig, TokenTable};

/// Dead-code facts of `source` as source text (the production channel:
/// `fold_unit_with_ops` reads `features.sound_flow_analysis` into
/// `Flow::set_sound_flow`).
fn dead_of(source: &str) -> Vec<String> {
    dead_offsets(source)
        .into_iter()
        .map(|(offset, length)| source[offset as usize..(offset + length) as usize].to_string())
        .collect()
}

/// Dead-code facts of `source` as `(offset, length)` pairs — the oracle's
/// `(offset, length)` span is the assertion's ground truth.
fn dead_offsets(source: &str) -> Vec<(u32, u32)> {
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
            FlowFact::DeadCode { span } => Some((span.range.offset, span.range.length)),
            _ => None,
        })
        .collect()
}

// —— P1: literal non-nullability ————————————————————————————————————

#[test]
fn if_null_rhs_dies_after_string_literal() {
    // `'a' ?? 'b'`: the left operand is a by-construction non-null
    // literal, so the right operand is never executed. Oracle 3.13.1
    // reports dead_code `[??, 'b' end)` (the companion
    // dead_null_aware_expression rides the same span through another
    // code namespace).
    let src = "const cc = 'a' ?? 'b';";
    assert_eq!(dead_of(src), vec!["?? 'b'".to_string()]);
}

#[test]
fn null_short_access_dies_after_null_literal() {
    // `null?.abs()`: the null-typed receiver kills the access path —
    // official `_nullAwareAccess_rightBegin`'s NullOrEquivalent arm
    // (flow_analysis.dart:9214-9216). The row spans the access after
    // `?.` (the operator itself is not part of the dead range).
    let src = "void f() {\n  null?.abs();\n}";
    assert_eq!(dead_of(src), vec!["abs()".to_string()]);
}

#[test]
fn if_null_rhs_survives_maybe_null_local() {
    // `n ?? 0` on an `int?` local stays live: the left operand can be
    // null, so the RHS executes on the null path.
    let src = "void f() {\n  int? n = null;\n  var t = n ?? 0;\n  print(t);\n}";
    assert_eq!(dead_of(src), Vec::<String>::new());
}

// —— P2: constant-condition bool feed ———————————————————————————————

#[test]
fn final_bool_local_feeds_else_dead() {
    // `final b = true; if (b) … else …`: the initializer's proven
    // `bool_value` replays on the read, so the else block is dead.
    // Oracle reports the else block's braces-to-braces span.
    let src = "void g(bool c) {\n  final b = true;\n  if (b) { print('live'); } else { print('dead'); }\n}";
    assert_eq!(dead_of(src), vec!["{ print('dead'); }".to_string()]);
}

#[test]
fn reassigned_bool_local_loses_provenance() {
    // A write between the initializer and the read clears `const_bool`:
    // the condition is no longer provable, both arms stay live.
    let src = "void g(bool c) {\n  var b = true;\n  b = c;\n  if (b) { print('x'); } else { print('y'); }\n}";
    assert_eq!(dead_of(src), Vec::<String>::new());
}

#[test]
fn record_tested_is_does_not_leak_dead_arm() {
    // `r is (int,) ? true : true`: the record-type spelling has no
    // name-level `Ty` (`Base::Unknown` — an unspellable tested type).
    // `factor_typed` on Unknown vacuously bottoms the unmatched arm,
    // which used to leak a dead-code row on the conditional's else.
    // The gate keeps both arms reachable: no row.
    let src =
        "void k() {\n  Object r = (1, 2);\n  var v = r is (int,) ? true : true;\n  print(v);\n}";
    assert_eq!(dead_of(src), Vec::<String>::new());
}

// —— P4a: Never-receiver trailing-operand kill ———————————————————————

#[test]
fn binary_rhs_dies_after_never_receiver() {
    // `a + b` where `a: Never`: the receiver's bottom static type kills
    // the flow at the operator; official `BinaryExpression`'s
    // `flowEnd(rightOperand)` re-anchors the dead range to
    // `[operator, rightOperand.end)`.
    let src = "int f(Never a, int b) { a + b; }";
    assert_eq!(dead_of(src), vec!["+ b".to_string()]);
}

#[test]
fn method_args_die_after_never_receiver() {
    // `a.m(1, 2)` where `a: Never`: the same handleExit face — a
    // non-null-aware invocation's method name is never a dead-node
    // candidate upstream (resolver.dart:3619 polls it only under
    // `isNullAware`), so the first dead node is the argument list and
    // the row runs to the statement's end (oracle 3.13.1: `(1, 2);`).
    let src = "int f(Never a) { a.m(1, 2); }";
    assert_eq!(dead_of(src), vec!["(1, 2);".to_string()]);
}

#[test]
fn property_read_dies_at_member_name() {
    // `a.p` where `a: Never`: a property access DOES poll the member
    // name (resolver.dart:4003 `checkUnreachableNode(propertyName)`),
    // so the dead range anchors at `p` and runs to the statement's end
    // (oracle 3.13.1: `p;`).
    let src = "int g(Never a) { a.p; }";
    assert_eq!(dead_of(src), vec!["p;".to_string()]);
}

#[test]
fn empty_args_die_at_open_paren() {
    // `a.m()` where `a: Never`: the empty argument list is still the
    // first polled node (oracle 3.13.1: `();`).
    let src = "int h(Never a) { a.m(); }";
    assert_eq!(dead_of(src), vec!["();".to_string()]);
}

// —— P4b: nested dead-arm window ————————————————————————————————————

#[test]
fn nested_conditional_reports_each_dead_arm() {
    // `true ? (false ? 5 : 6) : x`: the constant-true outer condition
    // kills `x`; inside the taken then-branch, `false` kills `5`.
    // Two independent rows — each arm entered dead reports at its own
    // construct end (oracle-verified 3.13.1).
    let src = "void g(bool c, int x) {\n  var z = true ? (false ? 5 : 6) : x;\n}";
    assert_eq!(dead_of(src), vec!["5".to_string(), "x".to_string()]);
}

#[test]
fn nested_conditional_folds_inside_live_arm() {
    // `c ? (true ? 1 : (false ? 2 : 3)) : x` with `c` unproven: the
    // outer arms stay live; inside the then-branch the inner `true`
    // kills its else — the whole `(false ? 2 : 3)` subexpression is
    // the dead range (the inner-inner conditional sits inside it, and
    // its own arms never open a competing window).
    let src = "void g(bool c, int x) {\n  var q = c ? (true ? 1 : (false ? 2 : 3)) : x;\n}";
    assert_eq!(dead_of(src), vec!["(false ? 2 : 3)".to_string()]);
}

#[test]
fn dead_code_inside_catch_folds_to_single_row() {
    // `if (false) { … }` inside a live catch clause: the constant-false
    // condition kills the then-block — one row covering the block; the
    // enclosing catch body stays live so nothing upstream reports.
    let src = "void g(bool c) {}\nvoid h() {\n  try { g(true, 0); } catch (e) { if (false) { print('nested'); } }\n}";
    assert_eq!(dead_of(src), vec!["{ print('nested'); }".to_string()]);
}
