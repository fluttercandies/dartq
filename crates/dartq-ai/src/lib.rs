//! Agent-facing logic: query, impact, explain, compatibility, and fix.
//!
//! Landed faces:
//!
//! * [`compatibility`] — the PRD §15.5 compatibility query: the same base
//!   code under several SDK contexts, aggregated into a per-axis ✓/✗
//!   matrix with the first failing diagnostic per axis.
//! * [`explain`] — the PRD §15.3 explain query: the one-stop symbol
//!   report (declaration, supertypes, members, references, the declaring
//!   library's importers, and its tests) over borrowed index faces.
//! * [`impact`] — the PRD §15.4 impact query: the changed symbols with
//!   their signature verdicts, the reverse-dependency closure tagged
//!   content-only vs semantic, the test-graph join, and the environment
//!   matrix (v1: one always-compatible context).
//! * [`rev_import_graph`] — the reversed import graph as one shared CSR
//!   build (`impact`'s inversion, reused by the amortized batch queries
//!   and the T8 explain stage).

pub mod compatibility;
pub mod explain;
pub mod impact;
pub mod rev_import_graph;
