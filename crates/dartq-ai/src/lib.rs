//! Agent-facing logic: query, impact, explain, compatibility, and fix.
//!
//! Landed faces:
//!
//! * [`compatibility`] — the compatibility query: the same base
//!   code under several SDK contexts, aggregated into a per-axis ✓/✗
//!   matrix with the first failing diagnostic per axis.
//! * [`explain`] — the explain query: the one-stop symbol
//!   report (declaration, supertypes, members, references, the declaring
//!   library's importers, and its tests) over borrowed index faces.
//! * [`diagnostic_explain`] — the `explain --diagnostic` report: a single
//!   diagnostic's echo, registry facts, fix rows, same-code neighbours,
//!   and docs URL over borrowed faces.
//! * [`impact`] — the impact query: the changed symbols with
//!   their signature verdicts, the reverse-dependency closure tagged
//!   content-only vs semantic, the test-graph join, and the environment
//!   matrix (v1: one always-compatible context).
//! * [`rev_import_graph`] — the reversed import graph as one shared CSR
//!   build (`impact`'s inversion, reused by the amortized batch queries
//!   and the T8 explain stage).

pub mod compatibility;
pub mod diagnostic_explain;
pub mod explain;
pub mod impact;
pub mod rev_import_graph;
