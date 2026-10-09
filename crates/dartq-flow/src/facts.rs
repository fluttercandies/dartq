//! The observable output of flow analysis: per-executable facts.

use dartq_parser::TokenSpan;

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
}
