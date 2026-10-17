//! The immutable arena: one flat `Vec` of 24-byte node headers plus the
//! shared `u32` payload pool.
//!
//! Parents are not stored: the parent of node `i` is the first `j > i` whose
//! child range covers `i` — a forward walk that is short in practice because
//! post-order completion puts a node's parent right after its subtree
//!.

#![allow(clippy::cast_possible_truncation)] // ids are u32 by design

use core::mem::size_of;

use crate::kind::{NodeKind, flags, payload_len};

/// Pool index sentinel: the node has no payload words.
pub(crate) const DATA_NONE: u32 = u32::MAX;

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
}

impl SyntaxTable {
    /// Assembles a table from its parts, compacting both vectors — the
    /// table is frozen from here on, so the len-based
    /// [`Self::memory_footprint`] reports the exact allocation. Only the
    /// builder calls this.
    pub(crate) fn from_parts(mut nodes: Vec<NodeHeader>, mut pool: Vec<u32>) -> Self {
        nodes.shrink_to_fit();
        pool.shrink_to_fit();
        Self { nodes, pool }
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

    /// The parent of `i`: the first node completed after `i` whose child
    /// window covers `i` (window and adoption conventions:
    /// [`Self::children`]). "First coverer in arena order" is what makes an
    /// adopted row resolve to the node that semantically owns it — the
    /// borrower's window starts later than the rows it adopts but is the
    /// first range laid over them. `None` for the root. Worst-case cost is
    /// a forward walk to the root's end, but the parent is typically the
    /// next node completed after `i`'s subtree.
    #[must_use]
    pub fn parent(&self, id: u32) -> Option<u32> {
        let i = id as usize;
        self.nodes[i + 1..]
            .iter()
            .position(|h| h.first_child <= id && id < h.end_child)
            .map(|off| (i + 1 + off) as u32)
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

    /// Bytes held by the arena itself: headers plus pool words. The engine's
    /// AST-cache accounting uses this (the density targets are
    /// asserted against it). Both vectors are `shrink_to_fit` at freeze
    /// (`Self::from_parts`), so len-based accounting is exact.
    #[must_use]
    pub fn memory_footprint(&self) -> usize {
        self.nodes.len() * size_of::<NodeHeader>() + self.pool.len() * size_of::<u32>()
    }
}
