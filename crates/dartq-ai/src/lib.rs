//! Agent-facing logic: query, impact, explain, compatibility, and fix.
//!
//! Landed faces:
//!
//! * [`impact`] — the PRD §15.4 impact query: the changed symbols with
//!   their signature verdicts, the reverse-dependency closure tagged
//!   content-only vs semantic, the test-graph join, and the environment
//!   matrix (v1: one always-compatible context).

pub mod impact;
