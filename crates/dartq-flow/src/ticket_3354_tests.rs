//! #3354 — body-completion reachability pins for the switch join and the
//! terminated tails (the `body_might_complete_normally` over-report face).
//!
//! The official gate is the first decision of
//! `checkForBodyMayCompleteNormally` (upstream Dart SDK source,
//! pkg/analyzer/lib/src/generated/resolver.dart:538-548): a body whose flow
//! state at the body end is not `isReachable` reports nothing at all. The
//! flow layer owns exactly that verdict — the `BodyMayCompleteNormally` fact
//! fires only when the end state is reachable — while the return-type,
//! body-shape and anchor faces belong to the consumer.
//!
//! Two upstream arms produce the unreachable end state:
//!
//! * `switchStatement_end` (upstream `_fe_analyzer_shared`,
//!   lib/src/flow_analysis/flow_analysis.dart:6620-6640):
//!   `if (!isExhaustive) breakState = _join(breakState, _unmatched);` then
//!   `breakState ??= _previous.setUnreachable()`. `isExhaustive` is
//!   `hasDefault || isAlwaysExhaustiveType(scrutineeType)` (type_analyzer.dart
//!   `analyzeSwitchStatement`), so an always-exhaustive scrutinee (enum,
//!   sealed class, `bool`, `Null`, `FutureOr<T>` of one) whose members all
//!   terminate leaves the code after the switch dead.
//! * `handleThrow` / `handleExit` (flow_analysis.dart:919-925) kill the
//!   current path, so a body whose tail is a `throw` or a `Never`-typed
//!   expression ends unreachable.
//!
//! Ticket probing (2026-11-01) over co19 `nnbd`,
//! `Patterns/Exhaustiveness` and `Constructor-tear-offs` against the oracle
//! showed the two arms above already answering correctly for these shapes:
//! every BodyMayCompleteNormally row the corpus over-reports traces to the
//! *consumer* faces instead — the declared return type staged for an
//! unannotated executable (a type-parameter bound or a body-local type is
//! picked up as the return, so `test1<T extends Sealed>(T t) {}` is read as
//! returning `Sealed` where the oracle's `bodyContext.contextType` is
//! absent) and the pattern-space coverage of a cast case (`case C(field: 0)
//! as C` followed by `case C _` covers the matched type upstream, killing
//! the `_unmatched` chain — see `analyzeCastPattern`'s
//! `matchFailsIfWrongType: false`). Both live outside this crate's switch
//! join. The pins below therefore lock the flow boundary in both
//! directions, so that a rework of the join — or a future consumer fix that
//! starts trusting a pattern-space verdict — cannot silently reintroduce the
//! over-report on the reachable side.

use dartq_language::{FeatureSet, LanguageVersion};
use dartq_parser::{ParserFeatures, TokenSpan};
use dartq_scanner::{ScanConfig, TokenTable};
use dartq_source::SourceRange;

use crate::ev::Ev;
use crate::facts::FlowFact;
use crate::forward::SharedFlow;
use crate::interp::Interp;
use crate::record::Flow;
use crate::ty::NameTypeOps;

/// Runs the recorder over `source` (name-level types) and returns the
/// emitted facts.
fn facts_of(source: &str) -> Vec<FlowFact> {
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
    let mut parser = dartq_parser::Parser::new(source, tokens, features, &mut flow);
    parser.parse_unit();
    drop(parser);
    flow.0.borrow_mut().take_facts()
}

/// Offsets of the body-completion anchors (the executables' end tokens).
fn completion_offsets(source: &str) -> Vec<u32> {
    facts_of(source)
        .into_iter()
        .filter_map(|fact| match fact {
            FlowFact::BodyMayCompleteNormally { span } => Some(span.range.offset),
            _ => None,
        })
        .collect()
}

/// Token span for hand-driven event sequences (arbitrary anchor).
fn tspan(offset: u32) -> TokenSpan {
    TokenSpan {
        id: offset,
        range: SourceRange { offset, length: 1 },
    }
}

/// A sealed scrutinee whose every member returns: the switch is
/// always-exhaustive by type (official `TypeSystem.isAlwaysExhaustive`, the
/// sealed-class arm) and no member falls through, so `breakState` stays
/// `null` and `switchStatement_end` answers `_previous.setUnreachable()` —
/// the post-switch point is dead and the body cannot complete normally.
#[test]
fn sealed_exhaustive_switch_all_arms_return_does_not_complete() {
    let source = concat!(
        "sealed class S {} class A extends S {} class B extends S {}",
        " int f(S s) { switch (s) { case A _: return 0; case B _: return 1; } }"
    );
    assert!(completion_offsets(source).is_empty());
}

/// The same join with throwing members: `handleThrow` kills each member
/// path, so no break/fall-through state survives the switch either.
#[test]
fn sealed_exhaustive_switch_all_arms_throw_does_not_complete() {
    let source = concat!(
        "sealed class S {} class A extends S {} class B extends S {}",
        " int f(S s) { switch (s) { case A _: throw 1; case B _: throw 2; } }"
    );
    assert!(completion_offsets(source).is_empty());
}

/// An enum is sealed by construction (`static_type.dart`), so the same join
/// closes a fully covered enum switch.
#[test]
fn enum_exhaustive_switch_all_arms_return_does_not_complete() {
    let source =
        "enum E { a, b } int f(E e) { switch (e) { case E.a: return 0; case E.b: return 1; } }";
    assert!(completion_offsets(source).is_empty());
}

/// A `default` head counts as exhaustive upstream (`hasDefault` wins before
/// the always-exhaustive type question), so a default-bearing switch whose
/// members all return leaves the code after it dead.
#[test]
fn default_bearing_switch_all_arms_return_does_not_complete() {
    let source = "int f(int x) { switch (x) { case 1: return 1; default: return 0; } }";
    assert!(completion_offsets(source).is_empty());
}

/// A `throw` tail ends the path (official `handleThrow`): the executable's
/// end state is unreachable and no completion fact emits.
#[test]
fn throw_tail_does_not_complete() {
    assert!(completion_offsets("int f() { throw 1; }").is_empty());
    // Control: the same body without the throw falls through, anchored at
    // the body's closing brace.
    let source = "int f() { 1; }";
    let end = u32::try_from(source.rfind('}').expect("closing brace")).expect("offset fits");
    assert_eq!(completion_offsets(source), [end]);
}

/// A `Never`-typed expression tail records `Ev::Exit` (official
/// `handleExit`, fired by the engine once a subexpression's static type
/// resolves to `Never`), and the body end is then unreachable.
///
/// Hand-driven: the name-level recorder cannot type a call to `Never` on its
/// own — that is the semantic layer's resolution — so the exit event is fed
/// directly, exactly as the engine appends it
/// (`void f() { neverFn(); }` with `Never neverFn()`).
#[test]
fn never_exit_tail_does_not_complete() {
    let events: Vec<Ev> = vec![
        Ev::ExecBegin { is_closure: false },
        Ev::BlockBodyBegin { span: tspan(9) },
        // `neverFn();` — the call's type resolves to `Never`.
        Ev::OtherExpr { span: tspan(11) },
        Ev::Exit { end: tspan(17) },
        Ev::ExprStmt { span: tspan(11) },
        Ev::StmtEnd { end: 18 },
        Ev::BlockBodyEnd {
            begin: tspan(9),
            end: 20,
        },
        Ev::ExecEnd {
            is_expr: false,
            end: tspan(20),
        },
    ];
    let interp = {
        let mut interp = Interp::new(&NameTypeOps, &[], &[]);
        interp.run(&events);
        interp
    };
    assert!(
        !interp
            .facts
            .iter()
            .any(|fact| matches!(fact, FlowFact::BodyMayCompleteNormally { .. })),
        "{:?}",
        interp.facts
    );
    // Control: without the exit the same buffer completes normally.
    let live: Vec<Ev> = events
        .iter()
        .copied()
        .filter(|ev| !matches!(ev, Ev::Exit { .. }))
        .collect();
    let mut interp = Interp::new(&NameTypeOps, &[], &[]);
    interp.run(&live);
    assert!(
        matches!(
            interp.facts.as_slice(),
            [FlowFact::BodyMayCompleteNormally { .. }]
        ),
        "{:?}",
        interp.facts
    );
}

/// Regression (reachable direction): a switch over an open type is not
/// always-exhaustive, so the unmatched chain joins the exit and the body
/// after it stays live — the completion row must still fire, anchored at the
/// body's closing brace.
#[test]
fn non_exhaustive_switch_over_an_open_type_completes() {
    let source = "int f(Object o) { switch (o) { case int _: return 1; } }";
    let end = u32::try_from(source.rfind('}').expect("closing brace")).expect("offset fits");
    assert_eq!(completion_offsets(source), [end]);
}

/// Regression (reachable direction): an always-exhaustive scrutinee whose
/// member falls through (`break`) seeds `breakState`, so
/// `switchStatement_end` keeps the exit live and the body completes.
#[test]
fn exhaustive_switch_with_a_falling_arm_completes() {
    let source = concat!(
        "sealed class S {} class A extends S {} class B extends S {}",
        " int f(S s) { switch (s) { case A _: return 1; case B _: break; } }"
    );
    let end = u32::try_from(source.rfind('}').expect("closing brace")).expect("offset fits");
    assert_eq!(completion_offsets(source), [end]);
}
