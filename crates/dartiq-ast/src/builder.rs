//! The arena writer. The parser drives it through begin/end events for
//! containers, leaf events for handles, `patch` for back-filling an open
//! node's payload slot, and `mark_error` for the error flag; speculation is
//! handled with `mark`/`rollback` (layout spec §3).
//!
//! Invariants locked here (milestone-independent, layout spec §6): post-order
//! push on every `end`, so child ranges are contiguous; begin/end pairs
//! checked against the collapse-map pairing table; payload slots reserved at
//! `begin` and back-filled by member events (the official class-like builder
//! pattern); the compilation unit root sealed last.
//!
//! This crate never depends on the parser: `dartiq-parser` implements its
//! `ParserListener` trait over these event families (layout spec §1.3 end).

#![allow(clippy::cast_possible_truncation)] // ids/offsets are u32 by design

use crate::kind::{NodeKind, accepts_begin, flags, payload_len};
use crate::table::{DATA_NONE, NodeHeader, SyntaxTable};

/// One open (begun, not yet sealed) container. Its payload slot is already
/// reserved at the tail of the pool; later member events back-fill it in
/// place, which is what lets a class-like declaration absorb events spread
/// across its whole header (collapse map §2.2 pattern).
#[derive(Debug)]
struct OpenNode {
    kind: NodeKind,
    first_token: u32,
    /// First child = arena length when this node was begun; every node
    /// completed while it stays open is its child, contiguously.
    first_child: u32,
    payload_start: u32,
    errored: bool,
}

/// Restoration point for parser speculation: `rollback` truncates the open
/// stack, the arena, the pool, and every error-flag bit set since the mark.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Marker {
    open: usize,
    nodes: usize,
    pool: usize,
    error_log: usize,
}

/// Builds one [`SyntaxTable`] from parser events.
#[derive(Debug, Default)]
pub struct AstBuilder {
    nodes: Vec<NodeHeader>,
    pool: Vec<u32>,
    open: Vec<OpenNode>,
    /// Open-stack indices whose `errored` bit was flipped false→true, in
    /// order — the undo log that makes error propagation rollback-safe.
    error_log: Vec<usize>,
}

impl AstBuilder {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Opens a container node anchored at `first_token` and reserves its
    /// payload slot.
    ///
    /// For shared-begin pairs (collapse map §2) the parser opens the kind it
    /// can name up front; the payload width of that kind must be at least the
    /// width of every end kind it may seal as (true by construction for the
    /// five groups — the wider kind is the canonical open). `end` enforces
    /// this with a debug assertion.
    pub fn begin(&mut self, kind: NodeKind, first_token: u32) {
        let payload_start = self.pool.len() as u32;
        self.pool
            .resize(self.pool.len() + payload_len(kind) as usize, 0);
        self.open.push(OpenNode {
            kind,
            first_token,
            first_child: self.nodes.len() as u32,
            payload_start,
            errored: false,
        });
    }

    /// Seals the innermost open node as `end_kind`, spanning
    /// `[first_token, end_token)` — `end_token` is the *exclusive* bound
    /// (one past the node's last token; equal to `first_token` for
    /// all-synthetic nodes).
    ///
    /// # Panics
    ///
    /// Panics when no node is open (parser bug, surfaces in any build).
    /// Debug builds additionally assert that `end_kind` accepts the open
    /// kind (pairing table) and that the sealed kind's payload fits the slot
    /// reserved at `begin`; release builds trust the parser.
    pub fn end(&mut self, end_kind: NodeKind, end_token: u32) {
        let node = self.open.pop().expect("end without a matching begin");
        debug_assert!(
            accepts_begin(end_kind, node.kind),
            "end {end_kind:?} does not pair with open {:?}",
            node.kind
        );
        let width = payload_len(end_kind);
        debug_assert!(
            node.payload_start as usize + width as usize <= self.pool.len(),
            "payload slot narrower than the sealed kind's width"
        );
        let errored = node.errored;
        if errored {
            self.propagate_error();
        }
        self.nodes.push(NodeHeader {
            kind: end_kind,
            flags: if errored { flags::ERROR } else { 0 },
            first_token: node.first_token,
            end_token,
            first_child: node.first_child,
            end_child: self.nodes.len() as u32,
            data: if width == 0 {
                DATA_NONE
            } else {
                node.payload_start
            },
        });
    }

    /// Emits a single-token leaf (a collapsed Handle with substantive
    /// content, e.g. `LiteralInt`). When the kind carries payload, word 0 is
    /// the token itself (all 1-width leaves store exactly their token) and
    /// the rest zero-fill for later `patch` calls.
    pub fn leaf(&mut self, kind: NodeKind, token: u32) {
        self.leaf_span(kind, token, token + 1);
    }

    /// Emits a leaf over an explicit half-open token range (leaves that
    /// carry a multi-token span, e.g. `Send`, `SkippedBody`).
    pub fn leaf_span(&mut self, kind: NodeKind, first_token: u32, end_token: u32) {
        let width = payload_len(kind);
        let payload_start = self.pool.len() as u32;
        if width == 0 {
            self.pool_done(DATA_NONE, kind, first_token, end_token);
        } else {
            self.pool.push(first_token);
            self.pool.resize(self.pool.len() + width as usize - 1, 0);
            self.pool_done(payload_start, kind, first_token, end_token);
        }
    }

    fn pool_done(&mut self, data: u32, kind: NodeKind, first_token: u32, end_token: u32) {
        self.nodes.push(NodeHeader {
            kind,
            flags: 0,
            first_token,
            end_token,
            first_child: self.nodes.len() as u32,
            end_child: self.nodes.len() as u32,
            data,
        });
    }

    /// Back-fills field `field` (0-based word index into the innermost open
    /// node's payload slot) with `value` — the member-event side of the
    /// class-like slot pattern.
    ///
    /// # Panics
    ///
    /// Panics when no node is open. Debug builds additionally assert that
    /// the field lies inside the open node's slot.
    pub fn patch(&mut self, field: usize, value: u32) {
        let node = self.open.last().expect("patch without an open node");
        let slot = node.payload_start as usize;
        debug_assert!(
            field < payload_len(node.kind) as usize,
            "patch field {field} outside the slot of {:?}",
            node.kind
        );
        self.pool[slot + field] = value;
    }

    /// Marks an error inside the innermost open node; the bit propagates to
    /// every ancestor as nodes seal (layout spec §3). Diagnostics themselves
    /// never enter the AST — they flow through the listener to the
    /// diagnostics channel.
    pub fn mark_error(&mut self) {
        let top = self.open.len() - 1;
        self.set_errored(top);
    }

    fn set_errored(&mut self, index: usize) {
        let node = &mut self.open[index];
        if !node.errored {
            node.errored = true;
            self.error_log.push(index);
        }
    }

    fn propagate_error(&mut self) {
        // Called after the errored node was popped: the new top is its parent.
        if let Some(parent) = self.open.len().checked_sub(1) {
            self.set_errored(parent);
        }
    }

    /// Marks parser speculation: everything built after this point can be
    /// discarded with [`Self::rollback`], mirroring
    /// `TokenCursor::mark`/`reset` (layout spec §3, risk R5).
    #[must_use]
    pub fn mark(&self) -> Marker {
        Marker {
            open: self.open.len(),
            nodes: self.nodes.len(),
            pool: self.pool.len(),
            error_log: self.error_log.len(),
        }
    }

    /// Discards every event since `marker`, including error-flag bits set on
    /// still-open nodes.
    pub fn rollback(&mut self, marker: Marker) {
        for &index in &self.error_log[marker.error_log..] {
            if index < self.open.len() {
                self.open[index].errored = false;
            }
        }
        self.error_log.truncate(marker.error_log);
        self.open.truncate(marker.open);
        self.nodes.truncate(marker.nodes);
        self.pool.truncate(marker.pool);
    }

    /// Consumes the builder into the immutable table.
    ///
    /// # Panics
    ///
    /// Panics when a begun node was never sealed — the compilation unit root
    /// is allocated last, so a non-empty open stack means a truncated unit.
    #[must_use]
    pub fn finish(self) -> SyntaxTable {
        debug_assert!(
            self.open.is_empty(),
            "finish with {} node(s) still open",
            self.open.len()
        );
        SyntaxTable::from_parts(self.nodes, self.pool)
    }
}
