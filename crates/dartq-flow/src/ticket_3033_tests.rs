//! #3033 — dead-code reachability pins for the two dominant `dead_code`
//! residual families, mirroring the official flow model (upstream Dart
//! SDK source, `pkg/_fe_analyzer_shared/lib/src/flow_analysis/
//! flow_analysis.dart`):
//!
//! * Nested constant patterns latch may-fail: upstream
//!   `constantPattern_end` (flow_analysis.dart:5483-5507) runs at EVERY
//!   pattern depth, and its `_NoEqualityInformation` arm joins the
//!   matched state into the enclosing context's unmatched chain
//!   (`_unmatched = _join(_unmatched!, _current)`,
//!   flow_analysis.dart:7470-7475) — a constant can fail on its value,
//!   so the enclosing `||` frame must not read the left operand as
//!   clean. Skipping the join at depth > 1 flagged the right operand of
//!   an alternation dead (`case C(f: E.a || E.b):` over-reported `E.b`)
//!   while the wildcard-left kill (`var _ || E.a` — left guaranteed)
//!   must stay dead.
//! * The `Null(...)` object-pattern head's required type IS the Null
//!   type: `promoteForPattern` classifies the head against
//!   `TypeClassification.nullOrEquivalent`
//!   (flow_analysis.dart:7224-7262 `_equalityCheck`;
//!   flow_analysis.dart:7410-7508 `_handleEqualityCheckPattern`;
//!   flow_analysis.dart / flow_analysis_operations.dart:141-146).
//!   `Null <: Null` is the guaranteed cover (dead else over a
//!   Null-typed matched value); non-nullable vs Null-only is the
//!   disjoint kill (dead then over an `int` matched value).
//!
//! Oracle: `dart analyze` 3.13.7 over the /tmp/dq3033 probe faces (enum
//! alternations, field alternations, wildcard-left kills, `Null()`
//! if-case covers/kills) plus the co19
//! `object_pattern_switch_test`/`pattern_matching_null_test` shapes.

use crate::facts::FlowFact;
use crate::forward::SharedFlow;
use crate::record::Flow;
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_parser::ParserFeatures;
use dartq_scanner::{ScanConfig, TokenTable};

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
            FlowFact::DeadCode { span } => {
                Some(source[span.range.offset as usize..span.range.end() as usize].to_string())
            }
            _ => None,
        })
        .collect()
}

/// Asserts `source` reports exactly one dead row covering `marker` —
/// pins WHICH construct died (the row count and its anchor text)
/// without baking in the interval's exact extent.
fn one_dead_row(source: &str, marker: &str) {
    let dead = dead_of(source);
    assert_eq!(dead.len(), 1, "{dead:?}");
    assert!(dead[0].contains(marker), "{dead:?}");
}

/// Asserts `source` reports no dead rows at all.
fn no_dead(source: &str) {
    assert_eq!(dead_of(source), Vec::<String>::new());
}

// —— alternation reachability: constants latch may-fail ——————————————

#[test]
fn enum_alternation_alternatives_stay_live() {
    // `case E.a || E.b || E.c:` — every constant is value-dependent, no
    // alternative is dead (co19 constant alternations).
    no_dead("enum E { a, b, c }\nvoid f(Object o) {\n  switch (o) {\n    case E.a || E.b || E.c:\n      break;\n  }\n}");
}

#[test]
fn field_alternation_alternatives_stay_live() {
    // `case C(f: E.a || E.b):` — the constants sit at field depth; the
    // may-fail join at depth > 1 keeps the `||` right operand reachable
    // (the over-emit this ticket fixes: `E.b` was flagged dead).
    no_dead("enum E { a, b, c }\nclass C { final E f; const C(this.f); }\nvoid f(C c) {\n  switch (c) {\n    case C(f: E.a || E.b):\n      break;\n  }\n}");
}

#[test]
fn int_literal_alternation_stays_live() {
    no_dead("void f(Object o) {\n  switch (o) {\n    case 1 || 2:\n      break;\n  }\n}");
}

#[test]
fn lone_field_constant_keeps_exit_live() {
    // `case C(f: E.a):` — a single value-dependent field constant: the
    // case may fail, the switch exit stays reachable.
    no_dead("enum E { a, b, c }\nclass C { final E f; const C(this.f); }\nvoid f(C c) {\n  switch (c) {\n    case C(f: E.a):\n      break;\n  }\n}");
}

// —— alternation kills: guaranteed-match left operand —————————————————

#[test]
fn wildcard_left_kills_alternative() {
    // `case var _ || E.a:` — `var _` always matches, so `|| E.a` is dead
    // (oracle row starts at the `||`). The may-fail fix must not stand
    // this kill down.
    let src = "enum E { a, b, c }\nvoid f(Object o) {\n  switch (o) {\n    case var _ || E.a:\n      break;\n  }\n}";
    assert_eq!(dead_of(src), vec!["|| E.a".to_string()]);
}

#[test]
fn field_wildcard_left_kills_alternative() {
    // `case C(f: var _ || E.a):` — same kill one depth down.
    let src = "enum E { a, b, c }\nclass C { final E f; const C(this.f); }\nvoid f(C c) {\n  switch (c) {\n    case C(f: var _ || E.a):\n      break;\n  }\n}";
    assert_eq!(dead_of(src), vec!["|| E.a".to_string()]);
}

// —— `Null()` object-pattern head: Null required type ————————————————

#[test]
fn null_head_covers_null_typed_scrutinee() {
    // `if (nullValue case Null())` over a Null-typed value — `Null <: Null`
    // is the guaranteed cover: the else arm is dead (the miss this
    // ticket fixes).
    one_dead_row(
        "void f() {\n  Null nullValue = null;\n  if (nullValue case Null()) {\n    1;\n  } else {\n    2;\n  }\n}",
        "2;",
    );
}

#[test]
fn null_head_disjoint_from_non_nullable_scrutinee() {
    // `if (i case Null())` over a non-nullable `int` — Null-only vs
    // non-nullable is the disjoint kill: the then arm is dead.
    one_dead_row(
        "void f() {\n  int i = 1;\n  if (i case Null()) {\n    1;\n  } else {\n    2;\n  }\n}",
        "1;",
    );
}

// —— regression locks: null-pattern and equality kills ———————————————

#[test]
fn null_pattern_covers_null_typed_scrutinee() {
    one_dead_row(
        "void f() {\n  Null nullValue = null;\n  if (nullValue case null) {\n    1;\n  } else {\n    2;\n  }\n}",
        "2;",
    );
}

#[test]
fn null_pattern_disjoint_from_non_nullable_scrutinee() {
    one_dead_row(
        "void f() {\n  int i = 1;\n  if (i case null) {\n    1;\n  } else {\n    2;\n  }\n}",
        "1;",
    );
}

#[test]
fn null_equality_kills_else() {
    one_dead_row(
        "void f() {\n  Null nullValue = null;\n  if (nullValue == null) {\n    1;\n  } else {\n    2;\n  }\n}",
        "2;",
    );
    one_dead_row(
        "void f() {\n  if (null == null) {\n    1;\n  } else {\n    2;\n  }\n}",
        "2;",
    );
    one_dead_row(
        "void f() {\n  Null nullValue = null;\n  if (nullValue == nullValue) {\n    1;\n  } else {\n    2;\n  }\n}",
        "2;",
    );
}

#[test]
fn wildcard_case_kills_default() {
    // `case _:` always matches — the `default:` member is dead (both
    // its keyword and body poll dead). Direction lock only: the row
    // split between the clause keyword and its body statement rides
    // the dead-interval machinery and is conformance-adjudicated.
    let src = "enum E { a, b }\nvoid f(E e) {\n  switch (e) {\n    case _:\n      break;\n    default:\n      2;\n  }\n}";
    let dead = dead_of(src);
    assert!(!dead.is_empty(), "{dead:?}");
    assert!(
        dead.iter().any(|row| row.contains("default")),
        "{dead:?}"
    );
    assert!(dead.iter().any(|row| row.contains("2;")), "{dead:?}");
}
