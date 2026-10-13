//! The observable output of flow analysis: per-executable facts.

use dartq_parser::TokenSpan;

use crate::ty::Ty;

/// One flow-analysis fact about an executable body, in source order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FlowFact {
    /// A statement (or statement-position block) is entered while the
    /// current control-flow path is unreachable — the `dead_code` family.
    /// `range` delimits the statement's first token.
    DeadCode {
        /// Span of the dead statement's first token.
        span: TokenSpan,
    },
    /// A read of a local variable (or parameter) that is definitely
    /// unassigned on this control-flow path. The consumer decides the
    /// diagnostic: non-late, non-nullable locals are compile errors
    /// (`not_assigned_potentially_non_nullable_local_variable`); final
    /// locals read before assignment error as well. `late` variables
    /// never surface here as errors but are still reported for parity
    /// tooling.
    UnassignedRead {
        /// Span of the read's identifier token.
        span: TokenSpan,
        /// Whether the variable was declared `late`.
        is_late: bool,
        /// Whether the variable was declared `final` (or `const`).
        is_final: bool,
        /// The variable's declared type, when the declaration carried one.
        declared_type: Option<dartq_source::SourceRange>,
    },
    /// A read of a local variable (or parameter) whose flow-promoted type
    /// differs from its declared type — the promotion facts a type-check
    /// consumer needs to see the promoted type at the read. Emitted only
    /// for real promotions (unpromoted and demoted reads carry no row).
    /// The row carries the promotion-chain head's written argument tree
    /// (`args`, an index into the recorder's arena read through
    /// [`crate::Flow::type_args_at`]; [`crate::ty::EMPTY_ARGS`] when
    /// argument-less) — the same pairing `ty` answers on the type-name
    /// side, so a consumer stages the promoted spelling exactly as
    /// written (#611 step 4; #1010 transport).
    PromotedRead {
        /// Span of the read's identifier token.
        span: TokenSpan,
        /// The promoted type on this control-flow path (chain head).
        ty: Ty,
        /// The chain head's written argument tree (arena index;
        /// [`crate::ty::EMPTY_ARGS`] when argument-less).
        args: u32,
    },
    /// The end of an executable body is reachable: the body can complete
    /// normally by falling off its end. The pure reachability face of the
    /// official `body_might_complete_normally` gate — the official first
    /// decision is `if (!flow.isReachable)` (`type_analyzer.dart:647-652`),
    /// before any return-type face. Every executable kind emits (method,
    /// top-level function, constructor, factory, local function,
    /// closure); the consumer owns the remaining gates (block-body shape,
    /// void/`dynamic`/`null` return faces, nullability) and the
    /// diagnostic's anchor remap (the official row reports at a block
    /// body's left bracket, not the end token).
    BodyMayCompleteNormally {
        /// Span of the executable's end token: the closing `}` of a block
        /// body, the end of an `=>` body's expression, or the `;` of an
        /// empty body.
        span: TokenSpan,
    },
    /// The flow type at one identifier read: the promoted type when a
    /// promotion stands, the declared type otherwise. Unlike
    /// [`FlowFact::PromotedRead`] every reachable read emits a row (a
    /// few hundred to a few thousand per file), so a consumer can ask
    /// for the static type at any read without re-running the fold.
    /// Payloads mirror [`FlowFact::PromotedRead`]; dead reads carry no
    /// row (their region is already the dead-code face), and a plain
    /// assignment's target identifier is a write, not a read — its row
    /// retracts with the target's read facts.
    ReadType {
        /// Span of the read's identifier token.
        span: TokenSpan,
        /// The flow type on this control-flow path (promoted or declared).
        ty: Ty,
        /// The type's written argument tree (arena index;
        /// [`crate::ty::EMPTY_ARGS`] when argument-less).
        args: u32,
    },
    /// One write-classification row of a local variable (or parameter),
    /// aggregated from the official `AssignedVariables` anywhere sets:
    /// at most one row per (declaration, in-closure) pair per body —
    /// `in_closure: false` for a declaration with writes outside nested
    /// closures, `in_closure: true` for one written inside a nested
    /// closure. The write positions themselves are not tracked (the
    /// sets carry variable ids only); a declaration with no row is
    /// never written in its body.
    WriteSite {
        /// Span of the declaration's name token.
        decl_name_span: dartq_source::SourceRange,
        /// The writes sit inside a nested closure.
        in_closure: bool,
    },
}
