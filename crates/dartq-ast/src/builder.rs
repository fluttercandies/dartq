//! The arena writer. The parser drives it through begin/end events for
//! containers, leaf events for handles, `patch` for back-filling an open
//! node's payload slot, and `mark_error` for the error flag; speculation is
//! handled with `mark`/`rollback` (layout spec §3).
//!
//! Invariants locked here (milestone-independent, layout spec §6): post-order
//! push on every `end`, so child ranges are contiguous; begin/end pairs
//! checked against the collapse-map pairing table; a Fielded node's payload
//! slot materializes lazily at the pool tail on its first `patch` and is
//! back-filled in place by member events (the official class-like builder
//! pattern; debug builds poison-fill the slot on materialization and sweep
//! it at `end` so a missed write fails at seal time — a node that seals
//! without ever materializing allocates zero pool words); the compilation
//! unit root sealed last (events after the root seal are refused in debug
//! builds). Speculation (`mark`/`rollback`) also undoes `patch` writes into
//! slots that predate the mark. Leaf construction follows the [`LeafShape`]
//! contract: `leaf`/`leaf_span` carry no payload words — Fielded kinds build
//! via `begin`/`patch`/`end`.
//!
//! This crate never depends on the parser: `dartq-parser` implements its
//! `ParserListener` trait over these event families (layout spec §1.3 end).

#![allow(clippy::cast_possible_truncation)] // ids/offsets are u32 by design

use crate::kind::{LeafShape, NodeKind, accepts_begin, flags, leaf_shape, payload_len};
use crate::table::{DATA_NONE, NodeHeader, SyntaxTable};

/// One open (begun, not yet sealed) container. Its payload slot materializes
/// lazily at the tail of the pool on the first [`AstBuilder::patch`] — a node
/// sealed without one allocates nothing — and later member events back-fill
/// it in place, which is what lets a class-like declaration absorb events
/// spread across its whole header (collapse map §2.2 pattern).
#[derive(Debug)]
struct OpenNode {
    kind: NodeKind,
    first_token: u32,
    /// First child = arena length when this node was begun; every node
    /// completed while it stays open is its child, contiguously.
    first_child: u32,
    /// Pool index of the payload slot once materialized; `None` until the
    /// first `patch`. Materialization always appends at the pool tail, so
    /// `rollback` can tell post-mark slots from pre-mark ones by the pool
    /// mark alone.
    payload_start: Option<u32>,
    /// Modifier-set bits (`ModSet`, collapse map §1) collected before sealing
    /// — stored into the header by `end` (`set_modifiers`, #174).
    mod_flags: u16,
    errored: bool,
}

/// Debug-only payload poison: materialization fills the slot with it and
/// `end` sweeps, so a Fielded word the parser forgot to `patch` fails loudly
/// at seal time instead of silently reading as token 0 (which is a valid
/// index — zero-fill is never a value). Release keeps zero-fill and trusts
/// the parser; a payload value that legitimately equals this bit pattern is
/// indistinguishable in debug builds — accepted (the pool is u32 token refs
/// and small counters, none of which reach 3.7 billion).
const POISON: u32 = 0xDEAD_BEEF;

/// One `patch` write recorded for [`AstBuilder::rollback`]: the pool word
/// and the value it held before the write. Logged on every `patch` and
/// retained until `finish` — O(patches) by construction (speculation
/// depth × payload width, both parser-bounded).
#[derive(Clone, Copy, Debug)]
struct UndoWrite {
    word: u32,
    old: u32,
}

/// Restoration point for parser speculation: `rollback` truncates the open
/// stack, the arena, the pool, and every error-flag bit set since the mark,
/// and undoes every `patch` write made since it — including writes into
/// slots whose node predates the mark, which the pool truncation alone
/// cannot reach.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Marker {
    open: usize,
    nodes: usize,
    pool: usize,
    error_log: usize,
    undo_log: usize,
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
    /// `patch` writes as (word, previous value), in order — the undo log
    /// that makes payload back-fill rollback-safe even for slots
    /// materialized before the mark (a pool truncation cannot reach those
    /// words).
    undo_log: Vec<UndoWrite>,
    /// Whether the root was already sealed (the open stack drained on an
    /// `end`). Anything begun or leaf-emitted afterwards would land *behind*
    /// the root and corrupt [`SyntaxTable::root`], so every event entry
    /// point debug-refuses it; `rollback` recomputes it.
    root_sealed: bool,
}

impl AstBuilder {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Opens a container node anchored at `first_token`. No pool words are
    /// reserved: the payload slot materializes at the pool tail on the first
    /// [`Self::patch`] (poison-filled in debug builds — see [`POISON`]), so
    /// a node that seals without patches allocates nothing.
    ///
    /// For shared-begin pairs (collapse map §2) the parser opens the kind it
    /// can name up front; the payload width of that kind must be at least the
    /// width of every end kind it may seal as (true by construction for the
    /// five groups — the wider kind is the canonical open). `end` enforces
    /// this with a debug assertion.
    pub fn begin(&mut self, kind: NodeKind, first_token: u32) {
        debug_assert!(!self.root_sealed, "begin after the root was sealed");
        self.open.push(OpenNode {
            kind,
            first_token,
            first_child: self.nodes.len() as u32,
            payload_start: None,
            mod_flags: 0,
            errored: false,
        });
    }

    /// Sets the innermost open node's modifier-set bits (`ModSet`, collapse
    /// map §1) — the header-flag home of the payload words eliminated in
    /// #174. `bits` must already be positioned inside the `MOD_MASK` mask
    /// (bits 1..=7); repeated calls OR into the set. The derived read side
    /// is the crate-root `mod_flags`/`first_modifier` accessors.
    ///
    /// # Panics
    ///
    /// Panics when no node is open. Debug builds additionally assert that
    /// `bits` stays inside the modifier mask.
    pub fn set_modifiers(&mut self, bits: u16) {
        let node = self
            .open
            .last_mut()
            .expect("set_modifiers without an open node");
        debug_assert!(
            bits & !flags::MOD_MASK == 0,
            "modifier bits outside the mask"
        );
        node.mod_flags |= bits;
    }

    /// Seals the innermost open node as `end_kind`, spanning
    /// `[first_token, end_token)` — `end_token` is the *exclusive* bound
    /// (one past the node's last token; equal to `first_token` for
    /// all-synthetic nodes).
    ///
    /// # Panics
    ///
    /// Panics when no node is open (parser bug, surfaces in any build).
    /// Debug builds additionally assert that `end_token` is not before the
    /// node's `first_token`, that `end_kind` accepts the open kind (pairing
    /// table), that the sealed kind's payload fits the materialized slot,
    /// and that no payload word is still [`POISON`] (a missed `patch`);
    /// release builds trust the parser.
    pub fn end(&mut self, end_kind: NodeKind, end_token: u32) {
        let node = self.open.pop().expect("end without a matching begin");
        debug_assert!(
            accepts_begin(end_kind, node.kind),
            "end {end_kind:?} does not pair with open {:?}",
            node.kind
        );
        debug_assert!(
            end_token >= node.first_token,
            "end {end_kind:?}: end_token {end_token} precedes first_token {}",
            node.first_token
        );
        let width = payload_len(end_kind) as usize;
        let data = match node.payload_start {
            Some(start) => {
                debug_assert!(
                    start as usize + width <= self.pool.len(),
                    "payload slot narrower than the sealed kind's width"
                );
                if width == 0 { DATA_NONE } else { start }
            }
            // Nothing ever patched the node. A width-0 seal allocates zero
            // pool words (the parenthesized-expression narrow-seal case); a
            // Fielded seal materializes its slot at the tail so `payload`
            // stays width-faithful — and the poison fill trips the sweep
            // below at seal time, exactly as if the slot had been reserved
            // at `begin`.
            None if width == 0 => DATA_NONE,
            None => self.reserve_payload(width),
        };
        #[cfg(debug_assertions)]
        if width > 0 {
            for word in &self.pool[data as usize..data as usize + width] {
                debug_assert!(*word != POISON, "unpatched payload word in {end_kind:?}");
            }
        }
        let errored = node.errored;
        if errored {
            self.propagate_error();
        }
        self.root_sealed = self.open.is_empty();
        self.nodes.push(NodeHeader {
            kind: end_kind,
            flags: node.mod_flags | if errored { flags::ERROR } else { 0 },
            first_token: node.first_token,
            end_token,
            first_child: node.first_child,
            end_child: self.nodes.len() as u32,
            data,
        });
    }

    /// Emits a childless single-token leaf: the [`LeafShape::TokenLeaf`]
    /// kinds (a collapsed Handle whose whole content is one token, e.g.
    /// `LiteralInt`) and single-token [`LeafShape::SpanOnly`] kinds. The node
    /// carries **no** payload word — a token leaf's token is
    /// `header.first_token` (the former `token: T` word was eliminated as
    /// derivable, #151) and the span is exactly `[token, token + 1)`.
    ///
    /// [`LeafShape::Fielded`] kinds must not come through here: their slot is
    /// reserved by [`Self::begin`] and back-filled with [`Self::patch`]
    /// before [`Self::end`] (see [`LeafShape::Fielded`] for the pattern).
    /// Debug builds assert the shape.
    pub fn leaf(&mut self, kind: NodeKind, token: u32) {
        debug_assert!(
            matches!(leaf_shape(kind), LeafShape::TokenLeaf | LeafShape::SpanOnly),
            "leaf {kind:?}: Fielded kinds build via begin/patch/end"
        );
        debug_assert!(!self.root_sealed, "leaf after the root was sealed");
        self.pool_done(kind, token, token + 1);
    }

    /// Emits a childless leaf over an explicit half-open token range — the
    /// multi-token [`LeafShape::SpanOnly`] kinds (e.g. `StringLiteral`,
    /// `Send`, `ExpressionStatement`). Token-leaf kinds use [`Self::leaf`]
    /// (their span is forced to one token); Fielded kinds use
    /// `begin`/`patch`/`end`.
    pub fn leaf_span(&mut self, kind: NodeKind, first_token: u32, end_token: u32) {
        debug_assert!(
            leaf_shape(kind) == LeafShape::SpanOnly,
            "leaf_span {kind:?}: TokenLeaf kinds use leaf(), Fielded kinds use begin/patch/end"
        );
        debug_assert!(
            end_token >= first_token,
            "leaf_span {kind:?}: end_token {end_token} precedes first_token {first_token}"
        );
        debug_assert!(!self.root_sealed, "leaf_span after the root was sealed");
        self.pool_done(kind, first_token, end_token);
    }

    fn pool_done(&mut self, kind: NodeKind, first_token: u32, end_token: u32) {
        self.nodes.push(NodeHeader {
            kind,
            flags: 0,
            first_token,
            end_token,
            first_child: self.nodes.len() as u32,
            end_child: self.nodes.len() as u32,
            data: DATA_NONE,
        });
    }

    /// Appends `width` payload words at the pool tail — poison-filled in
    /// debug builds so the `end` sweep catches unwritten ones, zero in
    /// release — and returns the slot's start index.
    fn reserve_payload(&mut self, width: usize) -> u32 {
        let fill = if cfg!(debug_assertions) { POISON } else { 0 };
        let start = self.pool.len() as u32;
        self.pool.resize(self.pool.len() + width, fill);
        start
    }

    /// Back-fills field `field` (0-based word index into the innermost open
    /// node's payload slot) with `value` — the member-event side of the
    /// class-like slot pattern. The slot materializes at the pool tail on
    /// the node's first `patch`; the write is logged so [`Self::rollback`]
    /// can undo it.
    ///
    /// # Panics
    ///
    /// Panics when no node is open. Debug builds additionally assert that
    /// the field lies inside the open node's slot; release builds record
    /// the error through the error log and skip the write — a field
    /// outside the slot would overwrite a neighboring node's payload word.
    pub fn patch(&mut self, field: usize, value: u32) {
        let (kind, payload_start) = match self.open.last() {
            Some(node) => (node.kind, node.payload_start),
            None => panic!("patch without an open node"),
        };
        if field >= payload_len(kind) as usize {
            // Release guard for the debug assertion: a field outside the
            // open node's slot addresses a NEIGHBORING node's pool word —
            // silent corruption. Impossible input corrupts nothing instead:
            // the error is recorded through the builder's error log (it
            // propagates to ancestors at seal and stays rollback-safe) and
            // the write is skipped.
            debug_assert!(false, "patch field {field} outside the slot of {kind:?}");
            self.mark_error();
            return;
        }
        let start = match payload_start {
            Some(start) => start,
            None => {
                let start = self.reserve_payload(payload_len(kind) as usize);
                self.open
                    .last_mut()
                    .expect("patch without an open node")
                    .payload_start = Some(start);
                start
            }
        };
        let slot = start as usize + field;
        let old = self.pool[slot];
        self.undo_log.push(UndoWrite {
            word: slot as u32,
            old,
        });
        self.pool[slot] = value;
    }

    /// Marks an error inside the innermost open node; the bit propagates to
    /// every ancestor as nodes seal (layout spec §3). Diagnostics themselves
    /// never enter the AST — they flow through the listener to the
    /// diagnostics channel.
    ///
    /// # Panics
    ///
    /// Panics when no node is open (the stack index underflows) — parser
    /// bug, surfaces in any build.
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
            undo_log: self.undo_log.len(),
        }
    }

    /// Discards every event since `marker`: the `patch` writes made since
    /// it (each word restored to its previous value — including writes into
    /// slots whose node predates the mark, which the pool truncation alone
    /// cannot reach), the error-flag bits set on still-open nodes, the open
    /// stack, the arena, and the pool. Open nodes that survive the
    /// truncation but first materialized after the mark are dematerialized:
    /// their slot lived in the truncated pool tail, and the next `patch`
    /// re-materializes it.
    pub fn rollback(&mut self, marker: Marker) {
        for entry in self.undo_log.drain(marker.undo_log..).rev() {
            self.pool[entry.word as usize] = entry.old;
        }
        for &index in &self.error_log[marker.error_log..] {
            if index < self.open.len() {
                self.open[index].errored = false;
            }
        }
        self.error_log.truncate(marker.error_log);
        self.open.truncate(marker.open);
        self.nodes.truncate(marker.nodes);
        self.pool.truncate(marker.pool);
        for node in &mut self.open[..marker.open] {
            if node.payload_start.is_some_and(|start| start as usize >= marker.pool) {
                node.payload_start = None;
            }
        }
        // Restore the flag to its state at mark time: the root was already
        // sealed then iff nothing was open and the arena was non-empty.
        self.root_sealed = marker.open == 0 && marker.nodes > 0;
    }

    /// Consumes the builder into the immutable table.
    ///
    /// # Panics
    ///
    /// Debug builds panic when a begun node was never sealed — the
    /// compilation unit root is allocated last, so a non-empty open stack
    /// means a truncated unit. Release builds yield a mis-rooted table
    /// instead (the truncated unit's last arena row would read as the root).
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
