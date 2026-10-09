//! Flow analysis for dartq: type promotion/demotion, definite assignment
//! (including `late`), and reachability, over the parser's listener event
//! stream.
//!
//! Behavior mirrors the official flow analysis engine
//! (`.third/dart-sdk/pkg/_fe_analyzer_shared/lib/src/flow_analysis/flow_analysis.dart`,
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
//!   [`ParserListener`]. Inside an executable's extent it records the
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
//! entered while unreachable) and reads of definitely-unassigned locals.
//! Promotion state is internal; it is consumed here for the facts above
//! and exposed to later milestones through the query face.
//!
//! # Known boundaries (each under-reports, never false-positives)
//!
//! * Static types are name-level ([`ty::NameTypeOps`]): a `Ty` is a
//!   type name plus nullability (or `dynamic`/`Never`/unknown). Promotions
//!   that need real subtyping (`x is T` where `T` crosses a class
//!   hierarchy) are decided optimistically on name compatibility; no
//!   subtype-based *else* promotions are inferred. A full `dartq-types`
//!   back-end plugs in through [`ty::TypeOps`].
//! * Field promotion (private final instance fields) and promoted
//!   properties are not mounted — property reads are opaque values.
//! * Sub-pattern promotions (list/map/record/object pattern fields),
//!   pattern-variable promotion sharing across `||` alternatives and
//!   shared case bodies, and `is`-check guaranteed-success/failure
//!   rewriting need resolved types or matched-value temporaries and are
//!   not mounted.
//! * Switch exhaustiveness without `default` (sealed/enum scrutinee
//!   types) is treated as non-exhaustive; the type system decides it.
//! * Condition-variable state (`bool b = x is int; if (b) …`) is not
//!   restored on read; the official feature needs shared persistent
//!   states.

pub mod ev;
pub mod facts;
pub mod model;
pub mod record;
pub mod ty;

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
