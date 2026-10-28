//! The immutable arena: one flat `Vec` of 24-byte node headers plus the
//! shared `u32` payload pool and one `u32` parent-pointer column.
//!
//! The parent of node `i` is the first `j > i` whose child range covers `i`
//! — "first coverer in arena order". That answer is backfilled into the
//! parent column in one pass at freeze ([`SyntaxTable::from_parts`]), so
//! [`SyntaxTable::parent`] is an indexed read.

#![allow(clippy::cast_possible_truncation)] // ids are u32 by design

use core::mem::size_of;

use crate::PayloadField;
use crate::kind::{NodeKind, flags, payload_len};

/// Pool index sentinel: the node has no payload words.
pub(crate) const DATA_NONE: u32 = u32::MAX;

/// Parent-column sentinel: the row is a root (no later row's child window
/// covers it).
pub(crate) const PARENT_NONE: u32 = u32::MAX;

/// One arena row: 24 bytes, align 4, `repr(C)` with all fields at most 4-byte
/// aligned, so there is no padding.
///
/// `first_token`/`end_token` are the half-open token-index range
/// `[first_token, end_token)` — the span substrate; byte/UTF-16 offsets
/// derive from the token table at access time ([`crate::span`]). Empty
/// ranges (all-synthetic nodes) are allowed: `first_token == end_token`.
/// `first_child`/`end_child` are the half-open child `NodeId` range,
/// contiguous by construction; empty when equal.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub(crate) struct NodeHeader {
    pub(crate) kind: NodeKind,
    pub(crate) flags: u16,
    pub(crate) first_token: u32,
    pub(crate) end_token: u32,
    pub(crate) first_child: u32,
    pub(crate) end_child: u32,
    pub(crate) data: u32,
}

/// The immutable AST of one parsed unit: a post-order arena of nodes over a
/// token table. Shared across the engine by content identity
/// ([`crate::AstKey`]); never mutated after [`crate::AstBuilder::finish`].
#[derive(Debug)]
pub struct SyntaxTable {
    nodes: Vec<NodeHeader>,
    pool: Vec<u32>,
    /// Per-row parent ([`Self::parent`]), backfilled once at freeze. One
    /// `u32` per row; `PARENT_NONE` for the root.
    parents: Vec<u32>,
}

impl SyntaxTable {
    /// Assembles a table from its parts, compacting both vectors — the
    /// table is frozen from here on, so the len-based
    /// [`Self::memory_footprint`] reports the exact allocation. Only the
    /// builder calls this. The parent column is derived here in one pass
    /// ([`build_parents`]).
    pub(crate) fn from_parts(mut nodes: Vec<NodeHeader>, mut pool: Vec<u32>) -> Self {
        nodes.shrink_to_fit();
        pool.shrink_to_fit();
        let parents = build_parents(&nodes);
        Self {
            nodes,
            pool,
            parents,
        }
    }

    /// Number of nodes.
    #[must_use]
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// The root: the last allocated node (post-order; the compilation unit is
    /// sealed last). `None` only for an empty table.
    #[must_use]
    pub fn root(&self) -> Option<u32> {
        self.nodes.len().checked_sub(1).map(|i| i as u32)
    }

    /// The node's kind.
    #[must_use]
    pub fn kind(&self, id: u32) -> NodeKind {
        self.nodes[id as usize].kind
    }

    /// Whether an error was marked inside the node (directly or propagated
    /// from a descendant).
    #[must_use]
    pub fn is_error(&self, id: u32) -> bool {
        self.nodes[id as usize].flags & flags::ERROR != 0
    }

    /// The node's raw flag bits (error bit + modifier-set bits, see
    /// [`crate::kind::flags`]). The derived-field accessors
    /// ([`crate::derived`]) read their field out of here.
    #[must_use]
    pub(crate) fn header_flags(&self, id: u32) -> u16 {
        self.nodes[id as usize].flags
    }

    /// The half-open token-index range `[first, end)` backing the node's
    /// span. Derive offsets via `crate::span`; the compilation unit's span
    /// is a special case handled there.
    #[must_use]
    pub fn token_range(&self, id: u32) -> core::ops::Range<u32> {
        let h = &self.nodes[id as usize];
        h.first_token..h.end_token
    }

    /// The node's child window: every arena row completed while the node
    /// was open — descendants at any depth, not a direct-child list. The
    /// definition-site conventions consumers build on:
    ///
    /// - windows nest: a child's window lies inside its parent's, so
    ///   `children` ranges overlap down the ancestry chain by design;
    /// - post-order ends every subtree at its root's own row, so member
    ///   *n*'s window runs from member *n−1*'s row up to its own — the
    ///   segment walk that recovers per-member slices inside a container's
    ///   window;
    /// - adoption ([`crate::AstBuilder::begin_adopting`] /
    ///   `begin_with_first_child`) lays `first_child` back over rows
    ///   completed before the node began, so a window may cover rows older
    ///   than the node's own begin.
    ///
    /// Direct children derive by peeling subtree extents in window order.
    #[must_use]
    pub fn children(&self, id: u32) -> core::ops::Range<u32> {
        let h = &self.nodes[id as usize];
        h.first_child..h.end_child
    }

    /// The parent of `i`: the node semantically owning `i`'s row — the first
    /// node completed after `i` whose child window covers `i` (window and
    /// adoption conventions: [`Self::children`]). "First coverer in arena
    /// order" is what makes an adopted row resolve to the node that
    /// semantically owns it — the borrower's window starts later than the
    /// rows it adopts but is the first range laid over them. `None` for the
    /// root.
    ///
    /// O(1): the answer is the parent column backfilled at freeze
    /// ([`build_parents`]), not a walk.
    #[must_use]
    pub fn parent(&self, id: u32) -> Option<u32> {
        let parent = self.parents[id as usize];
        (parent != PARENT_NONE).then_some(parent)
    }

    /// The node's payload word named by `field` — the named read face over
    /// [`Self::payload`]: the index resolves through
    /// [`crate::fields::PAYLOAD_FIELDS`] against the node's kind, so a
    /// reader never addresses a word by number. `None` only when the kind's
    /// row has no such field (a kind/field mismatch is a bug the
    /// `debug_assert` names; tests run debug, so a wrong pairing fails
    /// loudly there — the release face stays total and cheap).
    ///
    /// Token-valued words may carry [`crate::TOKEN_NONE`] (absent); filter
    /// at the call site the way [`Self::payload`] readers always have.
    #[must_use]
    pub fn field(&self, id: u32, field: PayloadField) -> Option<u32> {
        let kind = self.kind(id);
        let index = crate::fields::field_index(kind, field);
        debug_assert!(
            index.is_some(),
            "{field:?} is not a payload field of {kind:?} (row {id})"
        );
        Some(self.payload(id)[usize::from(index?)])
    }

    /// The node's shared-pool payload words (empty for span-only and
    /// token-leaf kinds — a token leaf's token is its span head,
    /// [`Self::token_range`] `.start`). The per-field layout of each kind is
    /// pinned by the per-kind payload columns; words eliminated as
    /// derivable (#174) come back through the crate-root derived accessors
    /// (`question`, `mod_flags`, `last_token`, …), not this slice. The run
    /// length is `payload_len` of the kind.
    #[must_use]
    pub fn payload(&self, id: u32) -> &[u32] {
        let h = &self.nodes[id as usize];
        if h.data == DATA_NONE {
            return &[];
        }
        let start = h.data as usize;
        &self.pool[start..start + payload_len(h.kind) as usize]
    }

    /// Bytes held by the arena itself: headers, pool words, and the parent
    /// column. The engine's AST-cache accounting uses this (the density
    /// targets are asserted against it). All vectors are `shrink_to_fit` at
    /// freeze (`Self::from_parts`), so len-based accounting is exact.
    #[must_use]
    pub fn memory_footprint(&self) -> usize {
        self.nodes.len() * size_of::<NodeHeader>()
            + self.pool.len() * size_of::<u32>()
            + self.parents.len() * size_of::<u32>()
    }
}

/// The parent column, backfilled in one pass at freeze: for each row the
/// first later row whose child window covers it — [`SyntaxTable::parent`]'s
/// definition, mirrored exactly. Windows are scanned in row (completion)
/// order and a row is claimed by the first window covering it, which is the
/// same "first coverer in arena order" the per-row forward scan answered.
///
/// A skip-pointer array (path-halving find) jumps the already-claimed rows,
/// so the pass costs one visit per window plus O(α(n)) per claimed row —
/// each row is written exactly once, O(n α(n)) overall, no per-row scans.
fn build_parents(nodes: &[NodeHeader]) -> Vec<u32> {
    let len = nodes.len() as u32;
    let mut parents = vec![PARENT_NONE; nodes.len()];
    // next_free[r]: the first unclaimed row >= r (`len` = none left).
    let mut next_free: Vec<u32> = (0..=len).collect();
    for (row, header) in nodes.iter().enumerate() {
        // The `.min(len)` guards a window tail past the arena (hand-built
        // tables); rows past the tail do not exist, and the definition's
        // scan could not answer them either.
        let window_end = header.end_child.min(len);
        let mut candidate = find_free(&mut next_free, header.first_child.min(window_end));
        while candidate < window_end {
            parents[candidate as usize] = row as u32;
            next_free[candidate as usize] = candidate + 1;
            candidate = find_free(&mut next_free, candidate + 1);
        }
    }
    parents
}

/// Path-halving find over the skip pointers: the first unclaimed row `>=
/// start`.
fn find_free(next_free: &mut [u32], mut start: u32) -> u32 {
    while next_free[start as usize] != start {
        let jump = next_free[start as usize];
        next_free[start as usize] = next_free[jump as usize];
        start = next_free[start as usize];
    }
    start
}
