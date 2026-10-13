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
    /// Promotions whose type derives from an arg-bearing spelling stay
    /// out: the row carries only the bare named type, so the consumer
    /// stage — which re-interns promoted spellings argument-less — could
    /// clean rows against written-argument targets on a fiction (#611
    /// step 4; the row gains the arguments once the consumer face reads
    /// them).
    PromotedRead {
        /// Span of the read's identifier token.
        span: TokenSpan,
        /// The promoted type on this control-flow path (chain head).
        ty: Ty,
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
}
