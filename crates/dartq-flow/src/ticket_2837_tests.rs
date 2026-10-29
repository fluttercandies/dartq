//! The #2837 pins: every sound-only fold answers the unit's
//! sound-flow-analysis posture (`Feature.sound_flow_analysis`,
//! `enabledIn: 3.9.0` — a `// @dart = 3.8` unit analyzes with the folds
//! stood down, `type_analyzer_options.dart:20` wiring the upstream flag).
//!
//! The census face (report.tsv, post-75756bec): the six upstream
//! `*_disabled_test` cases of `tests/language/sound_flow_analysis/`
//! carried 22 extra `dead_code` rows plus a 16-row
//! `unchecked_use_of_nullable_value` demotion cascade — our folds fired
//! at language 3.8 where the oracle's are gated off. Each disabled pin
//! below mirrors one upstream case body; each enabled twin pins the same
//! shape at 3.15 so the gate cannot over-close. The leg split is pinned
//! both ways too: the bottom-tested (`is Never`) and `null == null`
//! equality arms are mode-blind upstream (flow_analysis.dart checks
//! `isBottomType` before `_isTypeCheckGuaranteedToFail…` :5937-5941 and
//! leaves `_GuaranteedEqual` ungated :7229-7240), so they must keep
//! firing at 3.8.

use crate::facts::FlowFact;
use crate::forward::SharedFlow;
use crate::record::Flow;
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_parser::ParserFeatures;
use dartq_scanner::{ScanConfig, TokenTable};

/// Dead-code facts of `source`, parsed at `major.minor` with the flow
/// posture the same feature set folds (the production channel:
/// `fold_unit_with_ops` reads `features.sound_flow_analysis` into
/// `Flow::set_sound_flow`).
fn dead_of_at(source: &str, major: u32, minor: u32) -> Vec<String> {
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
        LanguageVersion::new(major, minor),
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
                source
                    [span.range.offset as usize..(span.range.offset + span.range.length) as usize]
                    .to_string(),
            ),
            _ => None,
        })
        .collect()
}

fn disabled(source: &str) -> Vec<String> {
    dead_of_at(source, 3, 8)
}

fn enabled(source: &str) -> Vec<String> {
    dead_of_at(source, 3, 15)
}

// —— disabled pins: the six census cases' shapes stay live at 3.8 ————

/// `trivially_satisfied_type_check_disabled_test`: `x is T` on a `T`-typed
/// operand is not known to succeed — the reference form (the
/// bottom-factor dead arm, flow_analysis.dart:3211-3219) and the
/// non-reference form (`_isTypeCheckGuaranteedToSucceed…`, :7645) both
/// keep the else live.
#[test]
fn disabled_trivially_satisfied_check_stays_live() {
    let source = r#"
void test({required int intValue, required int Function() intFunction, required int other}) {
  if (intValue is int) {
    shouldBeDemoted = null;
  } else {
    [intValue].expectStaticType<Exactly<List<int>>>();
  }
  if (intFunction() is int) {
    shouldBeDemoted = null;
  } else {
    shouldBeDemoted = 0;
  }
}
Object? shouldBeDemoted;
"#;
    assert_eq!(disabled(source), Vec::<String>::new());
}

/// `type_tests_disabled_test`'s `is Null`/`is! Null` faces: a
/// nullability-disjoint pair is `_isTypeCheckGuaranteedToFail…`'s arm
/// (:7611 `if (!…Enabled) return false`) — both arms stay live at 3.8.
#[test]
fn disabled_is_null_disjoint_stays_live() {
    let source = r#"
void test({required int intValue, required int Function() intFunction}) {
  if (intValue is Null) {
    shouldBeDemoted1 = null;
  } else {
    shouldBeDemoted2 = null;
  }
  if (intFunction() is Null) {
    shouldBeDemoted1 = null;
  } else {
    shouldBeDemoted2 = null;
  }
  if (intValue is! Null) {
    shouldBeDemoted1 = null;
  } else {
    shouldBeDemoted2 = null;
  }
}
Object? shouldBeDemoted1;
Object? shouldBeDemoted2;
"#;
    assert_eq!(disabled(source), Vec::<String>::new());
}

/// `type_tests_disabled_test`'s `as Null` faces: a cast the oracle knows
/// to throw only under sound flow (`asExpression_end` rides
/// `_isTypeCheckGuaranteedToFail…`, flow_analysis.dart:5303-5310) leaves
/// the continuation live — both directions of the disjointness.
#[test]
fn disabled_as_cast_disjoint_stays_live() {
    let source = r#"
void test({required int intValue, required Null nullValue}) {
  intValue as Null;
  shouldBeDemoted = null;
  nullValue as int;
  shouldBeDemoted = null;
  null as int;
  shouldBeDemoted = null;
}
Object? shouldBeDemoted;
"#;
    assert_eq!(disabled(source), Vec::<String>::new());
}

/// `if_null_disabled_test`: `<nonNullable> ?? <expr>` is not known to
/// skip `<expr>` (`ifNullExpression_rightBegin`'s nonNullable kill rides
/// `soundFlowAnalysisEnabled`, flow_analysis.dart:5857-5864).
#[test]
fn disabled_if_null_rhs_stays_live() {
    let source = r#"
void test({required int intValue, required int Function() intFunction, required int other}) {
  intValue ?? other;
  shouldBeDemoted = null;
  intFunction() ?? other;
  shouldBeDemoted = null;
}
Object? shouldBeDemoted;
"#;
    assert_eq!(disabled(source), Vec::<String>::new());
}

/// `pattern_not_matching_{null,non_nullable}_disabled_test`'s object
/// faces: `<nonNullable>()` cannot-match over a Null scrutinee and
/// `Null()` over a non-nullable scrutinee are `promoteForPattern`'s
/// sound-only arms (flow_analysis.dart:6360-6383) — both case bodies
/// stay live at 3.8.
#[test]
fn disabled_pattern_cannot_match_stays_live() {
    let source = r#"
void test({required Null nullValue, required int intValue}) {
  if (nullValue case int()) {
    shouldBeDemoted1 = null;
  } else {
    shouldBeDemoted2 = null;
  }
  if (intValue case Null()) {
    shouldBeDemoted1 = null;
  } else {
    shouldBeDemoted2 = null;
  }
}
Object? shouldBeDemoted1;
Object? shouldBeDemoted2;
"#;
    assert_eq!(disabled(source), Vec::<String>::new());
}

/// The constant-pattern disjoint face (`_equalityCheck`'s gated arm,
/// flow_analysis.dart:7241-7258): `case null` over a non-nullable
/// matched value answers `_NoEqualityInformation` at 3.8 — the case
/// body stays live and the null-check promotion face is not entered
/// (the disjoint arm sits before it upstream).
#[test]
fn disabled_constant_null_disjoint_stays_live() {
    let source = r#"
void test({required int intValue}) {
  if (intValue case null) {
    shouldBeDemoted1 = null;
  } else {
    shouldBeDemoted2 = null;
  }
}
Object? shouldBeDemoted1;
Object? shouldBeDemoted2;
"#;
    assert_eq!(disabled(source), Vec::<String>::new());
}

/// The expression `==` twin of the same gated arm: `nonNull == null` is
/// not known not-equal at 3.8, so neither side of the condition dies.
#[test]
fn disabled_eq_null_disjoint_stays_live() {
    let source = r#"
void test({required int intValue}) {
  if (intValue == null) {
    shouldBeDemoted1 = null;
  } else {
    shouldBeDemoted2 = null;
  }
}
Object? shouldBeDemoted1;
Object? shouldBeDemoted2;
"#;
    assert_eq!(disabled(source), Vec::<String>::new());
}

// —— mode-blind legs: the over-gating guard (must still kill at 3.8) ——

/// `isBottomType` is checked before the sound predicate
/// (flow_analysis.dart:5937-5941): `x is Never` folds to `false` at
/// every language version — the then branch is dead at 3.8 too
/// (`unreachable_via_never_test` rides this leg).
#[test]
fn disabled_bottom_tested_still_kills() {
    let source = r#"
void test({required int intValue}) {
  if (intValue is Never) {
    intValue = 1;
  }
}
"#;
    let dead = disabled(source);
    assert_eq!(dead.len(), 1, "{dead:?}");
    assert!(dead[0].contains("intValue = 1"), "{dead:?}");
}

/// `_GuaranteedEqual` (both operands null-or-equivalent) is ungated
/// (flow_analysis.dart:7229-7240): `null == null` keeps its known-equal
/// fold at 3.8 (`unreachable_via_this_property_test`'s documented
/// exception rides this arm).
#[test]
fn disabled_null_null_equal_still_kills() {
    let source = r#"
void test() {
  if (null == null) {
    ok = 1;
  } else {
    dead = 1;
  }
}
Object? ok;
Object? dead;
"#;
    let dead_rows = disabled(source);
    assert_eq!(dead_rows.len(), 1, "{dead_rows:?}");
    assert!(dead_rows[0].contains("dead = 1"), "{dead_rows:?}");
}

// —— enabled twins: the same shapes keep their kills at 3.15 ————

/// The `trivially_satisfied_type_check_test` twin: a same-type `is` on a
/// reference operand factors to bottom and deads the else. (The
/// non-reference form `intFunction() is int` under-reports at the name
/// level in every version — the operand's fold type is Unknown until the
/// semantic layer stages the resolved return — so it pins nothing here.)
#[test]
fn enabled_trivially_satisfied_check_kills_else() {
    let source = r#"
void test({required int intValue}) {
  if (intValue is int) {
    shouldBeDemoted = null;
  } else {
    shouldBeDemoted = 0;
  }
}
Object? shouldBeDemoted;
"#;
    let dead = enabled(source);
    assert_eq!(dead.len(), 1, "{dead:?}");
    assert!(dead[0].contains("shouldBeDemoted = 0"), "{dead:?}");
}

/// The `type_tests_test` twin: `is Null` folds to `false` — the then
/// branch dies; `as Null` and `null as int` kill the continuation. The
/// two casts share one dead interval (the official's first-dead-node
/// window runs to the block's covering close), so the second row spans
/// both anchors.
#[test]
fn enabled_is_and_as_null_kill() {
    let source = r#"
void test({required int intValue, required Null nullValue}) {
  if (intValue is Null) {
    thenDead = null;
  } else {
    elseLive = null;
  }
  intValue as Null;
  afterCast = null;
  null as int;
  afterNullCast = null;
}
Object? thenDead;
Object? elseLive;
Object? afterCast;
Object? afterNullCast;
"#;
    let dead = enabled(source);
    assert_eq!(dead.len(), 2, "{dead:?}");
    assert!(dead[0].contains("thenDead"), "{dead:?}");
    assert!(
        dead[1].contains("afterCast") && dead[1].contains("afterNullCast"),
        "{dead:?}"
    );
}

/// The `if_null_test` twin: a guaranteed-non-null LHS still kills its
/// RHS at 3.15.
#[test]
fn enabled_if_null_kills_rhs() {
    let source = r#"
void test({required int intValue, required int Function() intFunction, required int other}) {
  intValue ?? other;
  intFunction() ?? other;
}
"#;
    let dead = enabled(source);
    assert_eq!(dead.len(), 2, "{dead:?}");
    assert!(dead.iter().all(|row| row.contains("other")), "{dead:?}");
}

/// The `pattern_not_matching_null_test` twin: `case int()` over a
/// Null-typed scrutinee kills the case body at 3.15. (The mirrored
/// `Null()`-over-non-nullable direction under-reports at the name level
/// in every version — the object-pattern head spells the NAME `Null`,
/// which classifies non-nullable, so its kill is the semantic layer's
/// `pattern_never_matches_value_type` face, not a flow row.)
#[test]
fn enabled_pattern_cannot_match_kills_body() {
    let source = r#"
void test({required Null nullValue}) {
  if (nullValue case int()) {
    thenDead = null;
  } else {
    elseLive = null;
  }
}
Object? thenDead;
Object? elseLive;
"#;
    let dead = enabled(source);
    assert_eq!(dead.len(), 1, "{dead:?}");
    assert!(dead[0].contains("thenDead"), "{dead:?}");
}

/// The constant-pattern twin over a PROMOTED scrutinee: after `o != null`
/// the matched value is non-nullable, so `case null` is nullability-
/// disjoint and the case body dies at 3.15 (the declared-disjoint dedup
/// keeps the row flow-owned, since `Object?` itself is not disjoint from
/// `Null`).
#[test]
fn enabled_constant_null_over_promoted_scrutinee_kills_body() {
    let source = r#"
void test(Object? objectValue) {
  if (objectValue != null) {
    if (objectValue case null) {
      thenDead = null;
    } else {
      elseLive = null;
    }
  }
}
Object? thenDead;
Object? elseLive;
"#;
    let dead = enabled(source);
    assert_eq!(dead.len(), 1, "{dead:?}");
    assert!(dead[0].contains("thenDead"), "{dead:?}");
}

/// The `==` twin of the gated arm: `nonNull == null` folds to
/// known-not-equal and deads the then branch at 3.15.
#[test]
fn enabled_eq_null_disjoint_kills_then() {
    let source = r#"
void test({required int intValue}) {
  if (intValue == null) {
    thenDead = null;
  } else {
    elseLive = null;
  }
}
Object? thenDead;
Object? elseLive;
"#;
    let dead = enabled(source);
    assert_eq!(dead.len(), 1, "{dead:?}");
    assert!(dead[0].contains("thenDead"), "{dead:?}");
}
