//! Ticket #2626: interpreter termination. The step budget
//! ([`crate::interp::STEP_BUDGET_PER_EVENT`]) bounds every executable's
//! replay; this test pins the budget-exhaustion face of the invariant:
//! an exhausted budget deterministically bails with no partial flow
//! facts. (The real-corpus convergence face that calibrated the
//! constant's margin ran against the retired vendored corpus; the margin
//! re-derives from pub-cache packages via conformance/realworld.toml.)

/// An exhausted budget deterministically bails: the drivers unwind, the
/// body contributes no flow facts (a partial replay's state is not
/// evidence), and the same stream under the production budget runs to
/// completion — the bail is a budget artifact, never a silent fact drop.
#[test]
fn step_budget_exhaustion_bails_deterministically() {
    // A legitimate stream: one executable, two declared-and-read locals.
    let events: Vec<crate::ev::Ev> = vec![
        crate::ev::Ev::ExecBegin { is_closure: false },
        crate::ev::Ev::VarDeclBegin {
            late: false,
            is_final: false,
            span: tspan(0),
        },
        crate::ev::Ev::Type {
            base: crate::ty::Base::Name(1),
            nullable: true,
            args: crate::ty::EMPTY_ARGS,
        },
        crate::ev::Ev::InitializedIdent {
            name: 1,
            token: tspan(1),
        },
        crate::ev::Ev::InitializerBegin,
        crate::ev::Ev::OtherExpr { span: tspan(2) },
        crate::ev::Ev::InitializerEnd,
        crate::ev::Ev::VarDeclEnd,
        crate::ev::Ev::VarDeclBegin {
            late: false,
            is_final: false,
            span: tspan(3),
        },
        crate::ev::Ev::Type {
            base: crate::ty::Base::Name(2),
            nullable: true,
            args: crate::ty::EMPTY_ARGS,
        },
        crate::ev::Ev::InitializedIdent {
            name: 2,
            token: tspan(4),
        },
        crate::ev::Ev::InitializerBegin,
        crate::ev::Ev::OtherExpr { span: tspan(5) },
        crate::ev::Ev::InitializerEnd,
        crate::ev::Ev::VarDeclEnd,
        // A read of the first local: a reachable read emits its query row.
        crate::ev::Ev::Ident {
            name: 1,
            token: tspan(6),
            context: dartq_parser::IdentifierContext::Expression,
        },
    ];
    // Production budget: converges, no facts dropped.
    let mut interp = crate::interp::Interp::new(&crate::ty::NameTypeOps, &[], &[]);
    interp.run(&events);
    assert!(!interp.bailed);
    assert!(!interp.facts.is_empty());

    // Same stream past an artificial budget floor: bailed, zero facts.
    let mut interp = crate::interp::Interp::new(&crate::ty::NameTypeOps, &[], &[]);
    interp.override_step_budget_for_test(4);
    interp.run(&events);
    assert!(interp.bailed);
    assert!(interp.facts.is_empty());
    // Determinism: the same shrunk budget bails at the same residue.
    let mut replay = crate::interp::Interp::new(&crate::ty::NameTypeOps, &[], &[]);
    replay.override_step_budget_for_test(4);
    replay.run(&events);
    assert!(replay.bailed);
    assert!(replay.facts.is_empty());
    assert_eq!(replay.budget, interp.budget);
}

/// Token span for hand-driven event sequences (arbitrary anchor).
fn tspan(offset: u32) -> dartq_parser::TokenSpan {
    dartq_parser::TokenSpan {
        id: offset,
        range: dartq_source::SourceRange { offset, length: 1 },
    }
}
