//! The #3377 pins: `promoteForPattern` over a NON-reference matched value.
//!
//! Upstream never asks the matched value for a promotable reference. The
//! official `promoteForPattern` (upstream Dart SDK source,
//! pkg/_fe_analyzer_shared/lib/src/flow_analysis/flow_analysis.dart:6350-6451)
//! builds a **synthetic** reference from the matched value's *static type*
//! (`context.createReference(matchedType, _current)`) and then answers two
//! pure type questions on it:
//!
//! * `cannotMatch` — `classifyType(matchedType)` vs `classifyType(knownType)`
//!   at the two sound-only nullability legs (:6360-6383): a non-nullable
//!   matched value can never match a null-or-equivalent required type, and
//!   the mirror pair. The verdict sets `_current` unreachable;
//! * `coversMatchedType` — `isSubtypeOf(matchedType, knownType)` (:6394).
//!   A covering pattern joins nothing into `_unmatched`, and every
//!   alternative starts that chain unreachable (:9376-9380), so the else
//!   branch (or the later alternatives of a switch) run dead.
//!
//! Both answers exist for a literal, a select, and a call exactly like for a
//! variable — the reference is synthetic. dartq read only the switch frame's
//! latched scrutinee type, so a standalone context's non-reference matched
//! value (`if (null case Null v)`, `if (0 case int v)`) answered "no matched
//! type" and fell into the may-fail join, keeping an else branch the oracle
//! reports dead (`pattern_matching_null_test`,
//! `pattern_matching_non_nullable_test`, `pattern_not_matching_*_test`).
//!
//! A second pin covers the lattice half: `null`'s static type is `Null`, not
//! `Null?` (`type_system.dart:1255`'s `isStrictlyNonNullable` short-circuits
//! on `type.isDartCoreNull` before any suffix check, and `Null?` normalizes
//! to `Null`), so `null <: Null` must hold at the name level.

use dartq_language::{FeatureSet, LanguageVersion};
use dartq_parser::ParserFeatures;
use dartq_scanner::{ScanConfig, TokenTable};

use crate::facts::FlowFact;
use crate::forward::SharedFlow;
use crate::record::Flow;
use crate::ty::{Base, NameTypeOps, Ty, TypeOps};

/// Dead-code facts of `source` as the exact source text they cover.
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
                source[span.range.offset as usize..(span.range.offset + span.range.length) as usize]
                    .to_string(),
            ),
            _ => None,
        })
        .collect()
}

/// Asserts exactly one dead row and that it covers the `else` branch
/// (the marker name is branch-unique, so the row's extent is not
/// over-fitted to the block-vs-body trimming rules).
fn assert_else_block_dead(rows: &[String], what: &str) {
    assert_eq!(rows.len(), 1, "{what}: {rows:?}");
    assert!(rows[0].contains("notReached"), "{what}: {rows:?}");
}

/// Asserts exactly one dead row and that it covers the `then` branch.
fn assert_then_block_dead(rows: &[String], what: &str) {
    assert_eq!(rows.len(), 1, "{what}: {rows:?}");
    assert!(rows[0].contains("notRun"), "{what}: {rows:?}");
}

// —— `coversMatchedType` over a non-reference matched value ————————————————

/// `null case Null v` — a declaration pattern over the null literal. The
/// matched type is `Null`, the required type is `Null`, so the pattern
/// covers and the else branch is dead. This is the oracle's
/// `pattern_matching_null_test` shape (`if (null case Null v)` — the plain
/// `Null` bind, not the `case null` constant face, which the equality-check
/// arm already handled).
#[test]
fn null_scrutinee_declaration_pattern_deads_the_else() {
    let src = concat!(
        "void g() {}\n",
        "void f() {\n",
        "  if (null case Null v) {\n",
        "    g();\n",
        "  } else {\n",
        "    notReached();\n",
        "  }\n",
        "}\n",
    );
    assert_else_block_dead(&dead_of(src), "null case Null v");
}

/// `0 case int v` — an integer literal's static type is `int`, which the
/// required type `int` covers, so the else branch dies. The non-reference
/// half of `pattern_matching_non_nullable_test`.
#[test]
fn int_literal_scrutinee_declaration_pattern_deads_the_else() {
    let src = concat!(
        "void g() {}\n",
        "void f() {\n",
        "  if (0 case int v) {\n",
        "    g();\n",
        "  } else {\n",
        "    notReached();\n",
        "  }\n",
        "}\n",
    );
    assert_else_block_dead(&dead_of(src), "0 case int v");
}

/// `null case Null()` — the object-pattern head over the same non-reference
/// matched value. `matched_object_pattern`'s covered gate had the identical
/// switch-frame-only hole.
#[test]
fn null_scrutinee_object_pattern_deads_the_else() {
    let src = concat!(
        "void g() {}\n",
        "void f() {\n",
        "  if (null case Null()) {\n",
        "    g();\n",
        "  } else {\n",
        "    notReached();\n",
        "  }\n",
        "}\n",
    );
    assert_else_block_dead(&dead_of(src), "null case Null()");
}

// —— `cannotMatch` over a non-reference matched value ——————————————————————

/// `0 case Null v` — a non-nullable matched value with a null-or-equivalent
/// required type. `promoteForPattern`'s first sound-only leg makes the match
/// impossible, so the case body runs dead and the else stays live.
#[test]
fn int_literal_against_null_pattern_deads_the_then() {
    let src = concat!(
        "void g() {}\n",
        "void f() {\n",
        "  if (0 case Null v) {\n",
        "    notRun();\n",
        "  } else {\n",
        "    g();\n",
        "  }\n",
        "}\n",
    );
    assert_then_block_dead(&dead_of(src), "0 case Null v");
}

/// The switch-frame face of the same verdict: `switch (0) { case
/// Null(): … }` — the matched value is `int`, the head's required type
/// `Null`. The case body dies; the clause's failed arm still joins the
/// switch's unmatched chain, so the code after this NON-exhaustive switch
/// stays live (the join that #3377 left to the switch-kind half, #3389 —
/// `ticket_3389_tests.rs` carries the exhaustive mirror).
#[test]
fn switch_case_against_null_pattern_deads_the_match_arm_only() {
    let src = concat!(
        "void g() {}\n",
        "void f() {\n",
        "  switch (0) {\n",
        "    case Null():\n",
        "      notRun();\n",
        "  }\n",
        "  g();\n",
        "}\n",
    );
    let rows = dead_of(src);
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert!(rows[0].contains("notRun"), "{rows:?}");
}

// —— the posture boundaries ————————————————————————————————————————————————

/// A nullable required type still covers the null literal
/// (`Null <: int?`), so the else dies — the covering verdict must not be
/// conflated with the `cannotMatch` pair.
#[test]
fn null_scrutinee_against_nullable_required_type_deads_the_else() {
    let src = concat!(
        "void g() {}\n",
        "void f() {\n",
        "  if (null case int? v) {\n",
        "    g();\n",
        "  } else {\n",
        "    notReached();\n",
        "  }\n",
        "}\n",
    );
    assert_else_block_dead(&dead_of(src), "null case int? v");
}

/// An unresolved non-reference scrutinee (`f()` — the send's value carries no
/// name-level type) decides neither leg, so both branches stay live. The
/// under-report posture: no matched type is not a coverage proof.
#[test]
fn unresolved_call_scrutinee_keeps_both_arms() {
    let src = concat!(
        "int? f() => null;\n",
        "void g() {}\n",
        "void h() {\n",
        "  if (f() case int v) {\n",
        "    g();\n",
        "  } else {\n",
        "    g();\n",
        "  }\n",
        "}\n",
    );
    let rows = dead_of(src);
    assert!(rows.is_empty(), "{rows:?}");
}

// —— the lattice half ——————————————————————————————————————————————————————

/// `null`'s static type is `Null`, not `Null?`: the base carries no `?`
/// suffix, `null <: Null` holds, and `null` still fails a non-nullable
/// required type while satisfying a nullable one.
#[test]
fn null_literal_type_is_the_non_nullable_null_class() {
    let ops = NameTypeOps;
    let null_class = Ty {
        base: Base::Null,
        nullable: false,
    };
    assert_eq!(Ty::NULL, null_class, "the null literal is `Null`, not `Null?`");
    assert!(
        ops.is_subtype(Ty::NULL, null_class),
        "`Null <: Null` must hold"
    );
    assert!(
        ops.is_subtype(Ty::NULL, null_class.make_nullable()),
        "`Null <: Null?` must hold"
    );
    assert!(
        !ops.is_subtype(Ty::NULL, Ty {
            base: Base::Never,
            nullable: false,
        }),
        "the null literal is not the bottom type"
    );
}
