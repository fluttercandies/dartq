//! Agent-facing logic: query, impact, explain, compatibility, and fix.
//!
//! Landed faces:
//!
//! * [`compatibility`] — the PRD §15.5 compatibility query: the same base
//!   code under several SDK contexts, aggregated into a per-axis ✓/✗
//!   matrix with the first failing diagnostic per axis.
//! * [`impact`] — the PRD §15.4 impact query: the changed symbols with
//!   their signature verdicts, the reverse-dependency closure tagged
//!   content-only vs semantic, the test-graph join, and the environment
//!   matrix (v1: one always-compatible context).

pub mod compatibility;
pub mod impact;
