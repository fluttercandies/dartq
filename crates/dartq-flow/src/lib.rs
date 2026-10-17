//! Flow analysis for dartq: type promotion/demotion, definite assignment
//! (including `late`), and reachability, over the parser's listener event
//! stream.
//!
//! Behavior mirrors the official flow analysis engine
//! (`upstream Dart SDK source, pkg/_fe_analyzer_shared/lib/src/flow_analysis/flow_analysis.dart`,
//! Dart 3.15 main): the same syntax-directed single-pass walk with
//! branch/merge promotion semantics, the same decision order for
//! short-circuit joins, loop heads, `try`/`finally` composition, and
//! demotion. It is deliberately NOT a basic-block CFG — the official
//! implementation has none, and the join order is
//! observable-output-determining.
//!
//! # Architecture
//!
//! * **Event-driven, decoupled from the AST** — the engine is a
//!   [`dartq_parser::ParserListener`]. Inside an executable's extent it records the
//!   flow-relevant events into a per-executable buffer; when the executable
//!   ends, the buffer is processed in two internal phases: effect
//!   collection (the official `AssignedVariables` pre-pass, producing the
//!   written/captured sets each loop head and closure entry needs) and the
//!   flow replay proper. Nothing outlives the executable (memory law:
//!   state sized by the function, released per function).
//! * **Dense ids, no string keys on the state** — variables are dense
//!   `u32` ids per body; promotion/assignment state is a `Vec` of small
//!   copyable models indexed by variable id; promoted-type and
//!   tested-type chains are persistent cons-cells in a shared pool.
//!   Names are interned once at the declaration/read boundary.
//! * **Sync core, zero async** — pure computation over events.
//!
//! # What the engine reports
//!
//! [`FlowFact`]s per executable, in source order: dead code (statements
//! entered while unreachable), reads of definitely-unassigned locals, and
//! reads whose flow-promoted type differs from the declared one, plus the
//! query rows a consumer reads by offset instead of re-running the fold —
//! the flow type at every identifier read ([`FlowFact::ReadType`]), at
//! every condition operand's evaluation point
//! ([`FlowFact::ConditionType`]), and the switch scrutinee's promoted
//! type at each case clause's pattern match ([`FlowFact::MatchedType`]).
//! The why-not-promoted face ([`FlowFact::WhyNotPromoted`],
//! [`FlowFact::NonPromotedPropertyRead`]) carries, beside the type rows,
//! the ordered reasons a promotion failed — the data behind upstream's
//! `computeWhyNotPromotedMessages` context rows, read through
//! [`Flow::non_promotion_context`] and resolved to the five-variant
//! upstream reason set through [`Flow::non_promotion_reasons`]. Promotion
//! state is internal; it is consumed here for the facts above and exposed
//! to later milestones through the query face.
//!
//! # The promoted-type query face (#1349)
//!
//! The diagnoser consumes the two promoted-type query faces exactly like
//! [`FlowFact::ReadType`]: stage each row's `(span.range, ty)` pair (with
//! `ty`'s written argument tree re-interned through
//! [`Flow::type_args_at`]) into an offset-keyed pool, then answer a
//! position's flow type by lookup. [`FlowFact::ConditionType`] rows are
//! keyed by the condition operand's identifier read — for an operand
//! expression, descend through parentheses to its bare read — and carry
//! the type at that operand's evaluation point (a `&&`/`||` right operand
//! sees the left's true-branch promotions). [`FlowFact::MatchedType`]
//! rows are keyed by the clause anchor (a statement clause's `case`
//! keyword, a switch-expression alternative's first token) and carry the
//! scrutinee's promoted type at the pattern match. A missing row means
//! the position was not a promotable reference read (or, for
//! `MatchedType`, not a bare-read scrutinee): the resolved static type is
//! then the type layer's answer, not a promotion absence.
//!
//! # Known boundaries (each under-reports, never false-positives)
//!
//! * Static types are name-level ([`ty::NameTypeOps`]): a `Ty` is a
//!   type name plus nullability (or `dynamic`/`Never`/unknown). Promotions
//!   that need real subtyping (`x is T` where `T` crosses a class
//!   hierarchy) are decided optimistically on name compatibility. The
//!   else-branch factored promotion follows the official `factorType`
//!   rules cut to the name level (a `T?` remainder promotes to `Never?`,
//!   a bottom remainder marks the branch unreachable). A full
//!   `dartq-types` back-end plugs in through [`ty::TypeOps`].
//! * Field promotion (private final instance fields, #1409) is mounted
//!   for null-check shapes: a property read on a bare local or `this`
//!   receiver tracks its own promotion slot through the same per-key
//!   machinery as the locals (tests, invalidation on receiver
//!   write/capture, joins, `try`/`finally`), and a standing promotion
//!   emits [`FlowFact::PromotedPropertyRead`]. Whether the member is a
//!   promotable field (private, final) under the unit's field-promotion
//!   grant (the settled language version ≥ 3.2,
//!   `inference-update-2`) is the consumer's eligibility
//!   gate — the flow layer tracks every promotable-shaped read. Not
//!   mounted: `is`-test and cast promotions on properties (the promoted
//!   spelling needs the field's declared type, a resolved-type face),
//!   property-of-property chains (`c._a._b`), `super` receivers, and
//!   cascade-section targets — those reads stay opaque (under-report).
//! * Pattern promotions: a matched object pattern's head type promotes
//!   the switched variable through the clause (guard and body — the
//!   official pattern → guard → body order), chained past at the
//!   clause boundary. Sub-pattern promotions (list/map/record/object
//!   pattern fields), record/list/map head promotions (no name-level
//!   type spelling), pattern-variable promotion sharing across `||`
//!   alternatives and shared case bodies, and `is`-check
//!   guaranteed-success/failure rewriting need resolved types or
//!   matched-value temporaries and are not mounted.
//! * Switch exhaustiveness without `default` (sealed/enum scrutinee
//!   types) is treated as non-exhaustive; the type system decides it.
//! * Condition-variable state (`bool b = x is int; if (b) …`) is not
//!   restored on read; the official feature needs shared persistent
//!   states.
//! * The condition query rows cover reference operands only: an operand
//!   whose value is an assignment (`if (b = c)`) or a non-reference
//!   expression carries no read anchor and emits no row.

pub mod ev;
pub mod facts;
pub mod model;
pub mod record;
pub mod ty;

mod async_gap;
mod forward;
mod interp;
mod interp_expr;
mod interp_stmt;
mod patterns;

pub use facts::FlowFact;
pub use forward::SharedFlow;
pub use record::Flow;
pub use ty::{NameTypeOps, TypeOps};

#[cfg(test)]
mod tests;
