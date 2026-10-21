//! dartq-ast — the compact arena AST. Nodes hold token-range references,
//! never stored offsets: spans derive from the token table at access time
//! ([`byte_span`]/[`utf16_span`]), so byte and UTF-16 coordinates are both
//! available for free and zero-copy.
//!
//! # Layout
//!
//! One flat `Vec<NodeHeader>` of uniform 24-byte rows (kind, flags — error
//! bit + modifier bits — token pair, contiguous child range, payload index)
//! plus one shared append-only `u32` payload pool addressed by that index,
//! and one `u32` parent-pointer column (one entry per row, backfilled in one
//! pass at freeze so `parent` is an indexed read — #2467; the O(N) forward
//! walk it replaced dominated lint rule dispatch).
//! Every node pays only for the payload width its kind declares; words
//! derivable from the row's own tokens (span head, closing `;`/bracket,
//! head-when-present keyword, nullable `?`, `ModSet` first token) are not
//! stored at all — the derived-field accessors recompute them (#151 +
//! #174).
//! Targets pinned by the spec and asserted at compile time and by the corpus
//! metrics test: 24 B header, ≤30 B average per node (the parent column
//! rides the budget: model 24 + 12/9 + 4 ≈ 29.33 B/node), ≤1.5 MB of AST
//! per MB of source.
//!
//! # Faces
//!
//! - [`AstBuilder`] — the event-family writer the parser's listener drives
//!   (`begin`/`end`/`leaf`/`patch`/`mark_error`/`mark`/`rollback`/`finish`).
//!   This crate never depends on the parser; nothing depends back on it.
//! - [`SyntaxTable`] — the immutable result: `root`, `kind`, `is_error`,
//!   `token_range`, `children`, `parent`, `payload`.
//! - The derived-field accessors for eliminated payload words (`question`,
//!   `mod_flags`, `last_token`, …) — #174.
//! - [`AstKey`] — content-addressed cache identity; identical keys share one
//!   unit across workspaces/contexts.
//! - Traversal is `match node.kind` + an explicit stack — deliberately no
//!   visitor trait.
//!
//! Provenance: the layout is the adjudicated #92 header/pool split, the
//! variant set applies the #31 collapse rulings to the official 377 node
//! types, and the design rests on the #28 evidence; each contract is
//! stated inline where it applies.
//! Diagnostics never enter the AST: error *presence* is one flag
//! ([`SyntaxTable::is_error`]), messages flow through the parser's listener
//! to the diagnostics channel.

mod builder;
mod derived;
mod key;
mod kind;
mod span;
mod table;
#[cfg(test)]
mod tests;

pub use builder::{AstBuilder, Marker};
pub use derived::{
    first_modifier, for_token, head_is, indexed_open, last_token, mod_flags, question,
};
pub use key::{AstKey, FeatureSetId, ParserVersion};
pub use kind::{LeafShape, NodeKind, leaf_shape};
pub use span::{byte_span, utf16_span};
pub use table::SyntaxTable;

/// Index into [`SyntaxTable`]. The root is the highest id (post-order).
/// Arena-local and unstable across processes — snapshots persist semantic
/// ids only.
pub type NodeId = u32;

/// Index into the unified token table: scanned tokens keep their scanner
/// ids; parser-inserted tokens are appended into the scanner's table by
/// `dartq_scanner::TokenTable::append_inserted` at parse end (Eof re-seated
/// last), so the AST sees one contiguous table. Same space
/// as the parser's `TokenId`.
///
/// Sequencing contract: until the parser runs that append, an inserted id
/// (`table_len + i`) resolves only in the parser's side table — no span read
/// ([`byte_span`]/[`utf16_span`]) may happen before it. (Not to be confused
/// with the builder's lazy payload-slot materialization, which is about pool
/// words, not tokens.)
///
/// The index space is `u32` — a unit's token table is contract-bound below
/// 2³² entries (about 4.3 billion tokens per file; overflow is a
/// scanner/parser contract violation, not a runtime case).
pub type TokenRef = u32;

/// Payload-word sentinel for an absent optional token — the
/// `Option<TokenRef>` encoding. Every
/// optional word is written explicitly (`patch`ed with a real token or
/// [`TOKEN_NONE`]); a missed write is impossible to miss in debug builds
/// (the slot is poison-filled and swept at seal time) and release trusts the
/// parser — zero-fill is never `None` because 0 is a valid token index. Same
/// bit pattern as the pool's internal `DATA_NONE` (different role: absent
/// field vs empty slot).
///
/// The payload pool itself is `u32`-addressed like the token table: a unit's
/// pool is contract-bound below 2³² words (16 GB of payload; overflow is a
/// parser/engine contract violation, not a runtime case).
pub const TOKEN_NONE: TokenRef = u32::MAX;

const _: () = assert!(
    core::mem::size_of::<table::NodeHeader>() == 24,
    "NodeHeader must stay exactly 24 bytes (the ratified layout)"
);
const _: () = assert!(
    core::mem::align_of::<table::NodeHeader>() == 4,
    "NodeHeader must be 4-byte aligned with no padding"
);
const _: () = assert!(
    core::mem::size_of::<NodeKind>() == 2,
    "NodeKind must stay u16-sized (it lives inside the 24-byte header)"
);
