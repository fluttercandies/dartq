//! #2974 — dead-code anchoring pins for the invocation-callee and
//! argument-sibling Never-exit faces.
//!
//! Two sub-shapes the reachability walk under-anchored against the
//! official verifier (`upstream Dart SDK source,
//! pkg/analyzer/lib/src/error/dead_code_verifier.dart`):
//!
//! * A callee that exits mid-expression — `x<int>(args)`, `(f)(args)`,
//!   `a[i](args)`, a bare `f(args)` whose head read landed the exit —
//!   reports its first dead node at the argument list's `(` (official
//!   `AnonymousMethodInvocation` re-anchor,
//!   dead_code_verifier.dart:317-318; the argument list is the first
//!   candidate the dead-receiver walk reaches, resolver.dart:3617-3621).
//!   A callee that BEGAN dead in an already-dead region keeps the
//!   statement wrapper's anchor (`print('2')` rows at `print`).
//! * A positional argument evaluated after a sibling's Never exit —
//!   `a.clamp(b, c)` on a `Never b` — is the official first dead node
//!   (plain `checkUnreachableNode` arm; the operand's own span IS the
//!   extent, extended through the statement's end). An exit INSIDE the
//!   argument (`f(b)` on a `Never b`) skips the poll through the
//!   `exit_at` boundary — the argument was live when the official's
//!   pre-order poll ran, so the next statement anchors.

use crate::ev::Ev;
use crate::facts::FlowFact;
use crate::interp::Interp;
use crate::ty::NameTypeOps;
use dartq_parser::{IdentifierContext, TokenSpan};
use dartq_source::SourceRange;

/// Token span for hand-driven event sequences (arbitrary anchor).
fn tspan(offset: u32) -> TokenSpan {
    TokenSpan {
        id: offset,
        range: SourceRange { offset, length: 1 },
    }
}

/// A bare identifier read for hand-driven sequences.
fn read(name: u32) -> Ev {
    Ev::Ident {
        name,
        token: tspan(name),
        context: IdentifierContext::Expression,
    }
}

/// Drives a hand-built buffer through a fresh interpreter (name-level ops).
fn interp_of(events: &[Ev]) -> Interp<'static> {
    let mut interp = Interp::new(&NameTypeOps, &[], &[]);
    interp.run(events);
    interp
}

/// `(offset, length)` rows of the dead-code facts, in emission order.
fn dead_ranges(events: &[Ev]) -> Vec<(u32, u32)> {
    interp_of(events)
        .facts
        .iter()
        .filter_map(|fact| match fact {
            FlowFact::DeadCode { span } => Some((span.range.offset, span.range.length)),
            _ => None,
        })
        .collect()
}

/// `x<int>(1 + 2);` on a `Never x` — the instantiation completes with no
/// event of its own (the type-argument group only sets the pending tree),
/// so the event before `(` is the head identifier: the exit at `x`'s end
/// leaves `(` as the first dead node (official `AnonymousMethodInvocation`
/// re-anchor, dead_code_verifier.dart:317-318).
///
/// Oracle (FunctionExpressionInvocationTest_test_never): `(1 + 2);` —
/// offset 25, length 8.
#[test]
fn dead_generic_instantiation_callee_anchors_at_paren() {
    let events = vec![
        Ev::ExecBegin { is_closure: false },
        Ev::BlockBodyBegin,
        // `x` at 19 — a `Never` local read: the exit lands at its end
        // (20), every later node dead.
        read(19),
        Ev::Exit { end: tspan(19) },
        // The `x<int>` instantiation's type-argument group records no
        // event: `(` is the next node after the head.
        Ev::ArgumentsBegin { open: tspan(25) },
        read(26),
        Ev::BinaryBegin {
            op: crate::ev::BinOp::Other,
            span: tspan(28),
        },
        read(30),
        Ev::BinaryEnd {
            op: crate::ev::BinOp::Other,
            end: 31,
        },
        Ev::ArgumentEnd,
        Ev::ArgumentsEnd { count: 1 },
        Ev::Send {
            args: crate::ty::EMPTY_ARGS,
            end: 32,
        },
        Ev::ExprStmt { span: tspan(19) },
        Ev::StmtEnd { end: 33 },
        Ev::BlockBodyEnd,
        Ev::ExecEnd {
            is_expr: false,
            end: tspan(35),
        },
    ];
    assert_eq!(dead_ranges(&events), [(25, 8)]);
}

/// `(f)(args);` on a `Never f` — the parenthesized callee's `)` is the
/// event before `(`; the same `AnonymousMethodInvocation` face anchors
/// the row at the argument list.
#[test]
fn dead_parenthesized_callee_anchors_at_paren() {
    let events = vec![
        Ev::ExecBegin { is_closure: false },
        Ev::BlockBodyBegin,
        Ev::ParenBegin,
        read(11),
        Ev::ParenEnd,
        Ev::Exit { end: tspan(11) },
        Ev::ArgumentsBegin { open: tspan(13) },
        read(14),
        Ev::ArgumentEnd,
        Ev::ArgumentsEnd { count: 1 },
        Ev::Send {
            args: crate::ty::EMPTY_ARGS,
            end: 15,
        },
        Ev::ExprStmt { span: tspan(10) },
        Ev::StmtEnd { end: 16 },
        Ev::BlockBodyEnd,
        Ev::ExecEnd {
            is_expr: false,
            end: tspan(18),
        },
    ];
    assert_eq!(dead_ranges(&events), [(13, 3)]);
}

/// `a[i](args);` on a `Never a[i]` — the index access completes just
/// before `(`; the invocation reports at the argument list.
#[test]
fn dead_index_callee_anchors_at_paren() {
    let events = vec![
        Ev::ExecBegin { is_closure: false },
        Ev::BlockBodyBegin,
        read(11),
        read(13),
        Ev::IndexAccess {
            null_aware: false,
            close_end: 14,
        },
        Ev::Exit { end: tspan(13) },
        Ev::ArgumentsBegin { open: tspan(15) },
        read(16),
        Ev::ArgumentEnd,
        Ev::ArgumentsEnd { count: 1 },
        Ev::Send {
            args: crate::ty::EMPTY_ARGS,
            end: 17,
        },
        Ev::ExprStmt { span: tspan(11) },
        Ev::StmtEnd { end: 18 },
        Ev::BlockBodyEnd,
        Ev::ExecEnd {
            is_expr: false,
            end: tspan(20),
        },
    ];
    assert_eq!(dead_ranges(&events), [(15, 3)]);
}

/// `a.clamp(b, c);` on a `Never b` — the sibling's exit ends `b`'s own
/// evaluation, so `c` is the official first dead node (plain
/// `checkUnreachableNode` arm: the operand's own span IS the extent),
/// the statement's close extending the row through `;`.
///
/// Oracle (MethodInvocationResolutionTest_test_clamp_int_never_int):
/// `c);` — offset 40, length 3 (offsets mirror the corpus source
/// `f(int a, Never b, int c) {\n  a.clamp(b, c);\n}`).
#[test]
fn dead_argument_after_sibling_exit() {
    let events = vec![
        Ev::ExecBegin { is_closure: false },
        Ev::BlockBodyBegin,
        // `a` at 29 — live read.
        read(29),
        // `clamp` at 31 — the member continuation read (live state: no
        // poll fires).
        Ev::Ident {
            name: 31,
            token: tspan(31),
            context: IdentifierContext::ExpressionContinuation,
        },
        Ev::ArgumentsBegin { open: tspan(36) },
        // `b` at 37 — the `Never` sibling: exit at its end (38).
        read(37),
        Ev::Exit { end: tspan(37) },
        Ev::ArgumentEnd,
        // `c` at 40 — the first node the walk reaches dead.
        read(40),
        Ev::ArgumentEnd,
        Ev::ArgumentsEnd { count: 2 },
        Ev::Send {
            args: crate::ty::EMPTY_ARGS,
            end: 42,
        },
        Ev::ExprStmt { span: tspan(29) },
        Ev::StmtEnd { end: 43 },
        Ev::BlockBodyEnd,
        Ev::ExecEnd {
            is_expr: false,
            end: tspan(45),
        },
    ];
    assert_eq!(dead_ranges(&events), [(40, 3)]);
}

/// `f(b); g();` on a `Never b` — the exit inside the argument is `b`'s
/// own death at or after its first token: the argument was live when the
/// official's pre-order poll ran, so the enclosing list stays silent and
/// the NEXT statement anchors (the `exit_at` boundary the member poll
/// applies). No `c)`-style sibling row may fire on `b` itself.
#[test]
fn live_argument_with_interior_exit_stays_silent() {
    let events = vec![
        Ev::ExecBegin { is_closure: false },
        Ev::BlockBodyBegin,
        read(11),
        Ev::ArgumentsBegin { open: tspan(12) },
        // `b` at 13 — a `Never` read: the exit lands at ITS end (14),
        // inside the argument the poll covers.
        read(13),
        Ev::Exit { end: tspan(13) },
        Ev::ArgumentEnd,
        Ev::ArgumentsEnd { count: 1 },
        Ev::Send {
            args: crate::ty::EMPTY_ARGS,
            end: 15,
        },
        Ev::ExprStmt { span: tspan(11) },
        Ev::StmtEnd { end: 16 },
        // `g();` at 17 — the first dead node.
        read(17),
        Ev::ArgumentsBegin { open: tspan(18) },
        Ev::ArgumentsEnd { count: 0 },
        Ev::Send {
            args: crate::ty::EMPTY_ARGS,
            end: 19,
        },
        Ev::ExprStmt { span: tspan(17) },
        Ev::StmtEnd { end: 20 },
        Ev::BlockBodyEnd,
        Ev::ExecEnd {
            is_expr: false,
            end: tspan(22),
        },
    ];
    assert_eq!(dead_ranges(&events), [(17, 3)]);
}

/// `print('2');` in an already-dead region — the statement wrapper owns
/// the anchor (the began-dead face, `wrapper_dead_check`): an inner `(`-
/// poll must fold into the open interval, never steal the row to `(`.
#[test]
fn began_dead_statement_keeps_wrapper_anchor() {
    let events = vec![
        Ev::ExecBegin { is_closure: false },
        Ev::BlockBodyBegin,
        // `return;` at 11 — kills the flow live.
        Ev::ReturnBegin { span: tspan(11) },
        Ev::ReturnEnd {
            has_expression: false,
        },
        Ev::StmtEnd { end: 18 },
        // `print('2');` — began dead; the wrapper anchors at `print`.
        read(19),
        Ev::ArgumentsBegin { open: tspan(24) },
        Ev::ArgumentEnd,
        Ev::ArgumentsEnd { count: 1 },
        Ev::Send {
            args: crate::ty::EMPTY_ARGS,
            end: 27,
        },
        Ev::ExprStmt { span: tspan(19) },
        Ev::StmtEnd { end: 28 },
        Ev::BlockBodyEnd,
        Ev::ExecEnd {
            is_expr: false,
            end: tspan(30),
        },
    ];
    assert_eq!(dead_ranges(&events), [(19, 9)]);
}

/// `a.clamp(b, c);` on a `Never a` — the receiver's exit lands before
/// the member spelling; the non-null-aware invocation's method name is
/// never polled upstream (resolver.dart:3619), so the argument list's
/// `(` owns the row.
///
/// Oracle (MethodInvocationResolutionTest_test_clamp_never_int_int):
/// `(b, c);` — offset 32, length 7.
#[test]
fn dead_receiver_member_call_anchors_at_paren() {
    let events = vec![
        Ev::ExecBegin { is_closure: false },
        Ev::BlockBodyBegin,
        // `a` at 25 — the `Never` receiver: exit at its end (26).
        read(25),
        Ev::Exit { end: tspan(25) },
        Ev::Ident {
            name: 27,
            token: tspan(27),
            context: IdentifierContext::ExpressionContinuation,
        },
        Ev::ArgumentsBegin { open: tspan(32) },
        read(33),
        Ev::ArgumentEnd,
        read(36),
        Ev::ArgumentEnd,
        Ev::ArgumentsEnd { count: 2 },
        Ev::Send {
            args: crate::ty::EMPTY_ARGS,
            end: 38,
        },
        Ev::ExprStmt { span: tspan(25) },
        Ev::StmtEnd { end: 39 },
        Ev::BlockBodyEnd,
        Ev::ExecEnd {
            is_expr: false,
            end: tspan(41),
        },
    ];
    assert_eq!(dead_ranges(&events), [(32, 7)]);
}

/// A live invocation — `g(x);` with no exits — must stay silent: the
/// `(`-poll and the per-argument poll are both reachability-gated
/// (over-emit guard).
#[test]
fn live_invocation_stays_silent() {
    let events = vec![
        Ev::ExecBegin { is_closure: false },
        Ev::BlockBodyBegin,
        read(11),
        Ev::ArgumentsBegin { open: tspan(12) },
        read(13),
        Ev::ArgumentEnd,
        Ev::ArgumentsEnd { count: 1 },
        Ev::Send {
            args: crate::ty::EMPTY_ARGS,
            end: 14,
        },
        Ev::ExprStmt { span: tspan(11) },
        Ev::StmtEnd { end: 15 },
        Ev::BlockBodyEnd,
        Ev::ExecEnd {
            is_expr: false,
            end: tspan(17),
        },
    ];
    assert_eq!(dead_ranges(&events), []);
}
