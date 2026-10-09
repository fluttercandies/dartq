//! dartiq-ast — the compact arena AST. Nodes hold token-range references,
//! never stored offsets: spans derive from the token table at access time
//! ([`byte_span`]/[`utf16_span`]), so byte and UTF-16 coordinates are both
//! available for free and zero-copy (PRD §6.3, layout spec §2).
//!
//! # Layout
//!
//! One flat `Vec<NodeHeader>` of uniform 24-byte rows (kind, error flag,
//! token pair, contiguous child range, payload index) plus one shared
//! append-only `u32` payload pool addressed by that index. Every node pays
//! only for the payload width its kind declares; parents are not stored but
//! derived by a short forward walk. Targets pinned by the spec and asserted
//! at compile time and by the corpus metrics test: 24 B header, ≤30 B average
//! per node, ≤1.5 MB of AST per MB of source.
//!
//! # Faces
//!
//! - [`AstBuilder`] — the event-family writer the parser's listener drives
//!   (`begin`/`end`/`leaf`/`patch`/`mark_error`/`mark`/`rollback`/`finish`).
//!   This crate never depends on the parser; nothing depends back on it.
//! - [`SyntaxTable`] — the immutable result: `root`, `kind`, `is_error`,
//!   `token_range`, `children`, `parent`, `payload`.
//! - [`AstKey`] — content-addressed cache identity; identical keys share one
//!   unit across workspaces/contexts.
//! - Traversal is `match node.kind` + an explicit stack — deliberately no
//!   visitor trait (ast-design-spec §7).
//!
//! Fact sources: `docs/facts/ast-layout-spec.md` (#92, the adjudicated
//! layout), `docs/facts/ast-collapse-map.md` (#31, official 377 node types →
//! [`NodeKind`] variants), `docs/facts/ast-design-spec.md` (#28, evidence).
//! Diagnostics never enter the AST: error *presence* is one flag
//! ([`SyntaxTable::is_error`]), messages flow through the parser's listener
//! to the diagnostics channel.

mod builder;
mod key;
mod kind;
mod span;
mod table;
#[cfg(test)]
mod tests;

pub use builder::{AstBuilder, Marker};
pub use key::{AstKey, FeatureSetId, ParserVersion};
pub use kind::NodeKind;
pub use span::{byte_span, utf16_span};
pub use table::SyntaxTable;

/// Index into [`SyntaxTable`]. The root is the highest id (post-order).
/// Arena-local and unstable across processes — snapshots persist semantic
/// ids only (layout spec §4).
pub type NodeId = u32;

/// Index into the unified token table: scanned tokens keep their scanner
/// ids; parser-inserted tokens are materialized and appended at parse end,
/// so the AST sees one contiguous table (layout spec §2). Same space as the
/// parser's `TokenId`.
pub type TokenRef = u32;

const _: () = assert!(
    core::mem::size_of::<table::NodeHeader>() == 24,
    "NodeHeader must stay exactly 24 bytes (layout spec §1.3)"
);
const _: () = assert!(
    core::mem::align_of::<table::NodeHeader>() == 4,
    "NodeHeader must be 4-byte aligned with no padding"
);
const _: () = assert!(
    core::mem::size_of::<NodeKind>() == 2,
    "NodeKind must stay u16-sized (it lives inside the 24-byte header)"
);
