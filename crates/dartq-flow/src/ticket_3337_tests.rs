//! The #3337 pins: the declared-bound leg of the `TypeParameterTypeImpl`
//! exhaustiveness arm over a promoted type-variable scrutinee.
//!
//! Upstream's promoted type of a type variable keeps the variable identity
//! (`T & S` — `tryPromoteToType`'s `TypeParameterType` arm,
//! type_system.dart:1749-1758), so `isAlwaysExhaustive`'s `TypeParameter`
//! arm (:836-847) answers the disjunction `exhaustive(promotedBound) ||
//! exhaustive(declared bound)`. This level's promotion chain spells the
//! promoted head alone (the head replaces the variable's type), which
//! carries the promotedBound leg; the `switch_end` verdict re-adds the
//! declared-bound leg from the declaration capture
//! ([`crate::ty::Boundary`]) — exhaustiveness does not descend subtypes
//! (a subclass of a sealed type need not be sealed), so the leg changes
//! verdicts the head alone cannot: `T extends St` (sealed) promoted to
//! `Mid` closes a switch the bare `Mid` head leaves open. Oracle rows
//! (dart analyze 3.13.7): the post-switch statement reports dead_code in
//! P1/C2 and stays live in C1/C3; P2 pins the named SDK-bound residual
//! (`num`'s core sealedness is outside this lattice, oracle-dead). The
//! `non_exhaustive_switch_statement` error beside the oracle's rows is
//! the semantic face, not a flow fact.

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
            FlowFact::DeadCode { span } => Some(
                source
                    [span.range.offset as usize..(span.range.offset + span.range.length) as usize]
                    .to_string(),
            ),
            _ => None,
        })
        .collect()
}

const SEALED_MID: &str = concat!(
    "sealed class St {}\n",
    "abstract class Mid extends St {}\n",
    "class A extends Mid {}\n",
    "class B extends Mid {}\n",
    "void i() {}\n",
);

/// [`SEALED_MID`] plus the test body — `concat!` only takes literals, so
/// the shared header composes at runtime.
fn with_sealed_mid(body: &str) -> String {
    format!("{SEALED_MID}{body}")
}

// —— the declared-bound leg over a promoted variable (P1, P2) ————————————

#[test]
fn promoted_scrutinee_closes_through_the_sealed_declared_bound() {
    // P1: the promoted head `Mid` is not exhaustive; the declared bound
    // `St` is sealed. All case bodies terminate and the gap arm leaves the
    // unmatched chain live, so the verdict alone decides: upstream reads
    // `T & Mid` -> bound `St` -> TRUE -> post-switch dead (oracle 1 row).
    let src = with_sealed_mid(concat!(
        "void p1<T extends St>(T t) {\n",
        "  if (t is Mid) {\n",
        "    switch (t) {\n",
        "      case A _:\n",
        "        return;\n",
        "    }\n",
        "    i();\n",
        "  }\n",
        "}\n",
    ));
    assert_eq!(
        dead_of(&src),
        vec!["i();".to_string()],
        "{:?}",
        dead_of(&src)
    );
}

#[test]
fn promoted_scrutinee_bound_outside_the_unit_stays_conservative() {
    // P2 (the ticket pin's shape, kept as the NAMED residual): `t is int`
    // over `T extends num` — upstream answers TRUE through the declared
    // bound (`sealed class num`, sdk/lib/core/num.dart) and deads the
    // post-switch statement (oracle: dead `i();` + the
    // non_exhaustive_switch_statement error). This lattice cannot see a
    // core head's sealedness: `ClosedNames` captures the unit's own
    // sealed/enum declarations and the resolved builtin table marks no
    // sealed core head, so the declared-bound leg recurses into `num` and
    // answers `false` — post-switch live. The under-report direction (a
    // missed verdict keeps the state live); capturing core sealedness is
    // the separate SDK-bound residual, not the promotedBound wiring.
    let src = concat!(
        "void i() {}\n",
        "void p2<T extends num>(T t) {\n",
        "  if (t is int) {\n",
        "    switch (t) {\n",
        "      case 1:\n",
        "        return;\n",
        "    }\n",
        "    i();\n",
        "  }\n",
        "}\n",
    );
    assert!(dead_of(src).is_empty(), "{:?}", dead_of(src));
}

// —— the conservative faces (C1, C3) ——————————————————————————————————————

#[test]
fn promoted_non_parameter_keeps_no_declared_bound_leg() {
    // C1: a promoted non-parameter read (`num n` -> `int`) has no type
    // variable to re-read: the head's own verdict stands — `int` is not
    // exhaustive, post-switch live (oracle 0 rows).
    let src = concat!(
        "void i() {}\n",
        "void c1(num n) {\n",
        "  if (n is int) {\n",
        "    switch (n) {\n",
        "      case 1:\n",
        "        return;\n",
        "    }\n",
        "    i();\n",
        "  }\n",
        "}\n",
    );
    assert!(dead_of(src).is_empty(), "{:?}", dead_of(src));
}

#[test]
fn fall_through_bodies_keep_the_break_model_live() {
    // C3: a completing case body falls through into the break model, so
    // the post-switch state stays live even on a TRUE verdict (oracle
    // 0 rows) — the leg must not over-dead past the break join. The
    // unit seal-bound makes the leg fire, so this exercises the guard on
    // the fixed path, not the conservative one.
    let src = with_sealed_mid(concat!(
        "void c3<T extends St>(T t) {\n",
        "  if (t is Mid) {\n",
        "    switch (t) {\n",
        "      case A _:\n",
        "        i();\n",
        "    }\n",
        "    i();\n",
        "  }\n",
        "}\n",
    ));
    assert!(dead_of(&src).is_empty(), "{:?}", dead_of(&src));
}

// —— the unpromoted regression surface (C2) ——————————————————————————————————

#[test]
fn unpromoted_parameter_boundary_walk_is_unchanged() {
    // C2: a bare `T` scrutinee never latched a promotion, so the boundary
    // arm inside the base ask already recursed into the declared bound —
    // the verdict was TRUE before this change and must stay TRUE (unit
    // seal-bound so the name-level capture sees it; oracle 1 row).
    let src = with_sealed_mid(concat!(
        "void c2<T extends St>(T t) {\n",
        "  switch (t) {\n",
        "    case A _:\n",
        "      return;\n",
        "  }\n",
        "  i();\n",
        "}\n",
    ));
    assert_eq!(
        dead_of(&src),
        vec!["i();".to_string()],
        "{:?}",
        dead_of(&src)
    );
}
