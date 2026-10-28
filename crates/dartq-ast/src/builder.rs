//! The arena writer. The parser drives it through begin/end events for
//! containers, leaf events for handles, `patch` for back-filling an open
//! node's payload slot, and `mark_error` for the error flag; speculation is
//! handled with `mark`/`rollback`.
//!
//! Invariants locked here: post-order
//! push on every `end`, so child ranges are contiguous; begin/end pairs
//! checked against the collapse-map pairing table; a Fielded node's payload
//! slot materializes lazily at the pool tail on its first `patch` and is
//! back-filled in place by member events (the official class-like builder
//! pattern; debug builds poison-fill the slot on materialization and sweep
//! it at `end` so a missed write fails at seal time — a node that seals
//! without ever materializing allocates zero pool words); the compilation
//! unit root sealed last (events after the root seal are refused in debug
//! builds). Speculation (`mark`/`rollback`) also undoes `patch` writes into
//! slots that predate the mark and `set_modifiers` writes into nodes that
//! do. Leaf construction follows the [`LeafShape`]
//! contract: `leaf`/`leaf_span` carry no payload words — Fielded kinds build
//! via `begin`/`patch`/`end`.
//!
//! This crate never depends on the parser: `dartq-parser` implements its
//! `ParserListener` trait over these event families.

#![allow(clippy::cast_possible_truncation)] // ids/offsets are u32 by design

use crate::PayloadField;
use crate::kind::{LeafShape, NodeKind, accepts_begin, flags, leaf_shape, payload_len};
use crate::table::{DATA_NONE, NodeHeader, SyntaxTable};

/// One open (begun, not yet sealed) container. Its payload slot materializes
/// lazily at the tail of the pool on the first [`AstBuilder::patch`] — a node
/// sealed without one allocates nothing — and later member events back-fill
/// it in place, which is what lets a class-like declaration absorb events
/// spread across its whole header.
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
    /// Modifier-set bits collected before sealing
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
/// and the value it held before the write. Logged on every `patch` while a
/// marker is live and retained until `finish` — O(patches under the
/// outermost mark) by construction (speculation depth × payload width,
/// both parser-bounded).
#[derive(Clone, Copy, Debug)]
struct UndoWrite {
    word: u32,
    old: u32,
}

/// One `set_modifiers` write recorded for [`AstBuilder::rollback`]: the
/// open-stack index of the mutated node and the bits it held before the
/// write. Logged only when the OR changes the value and only while a marker
/// is live — the same log-real-transitions-only discipline as the error
/// log.
#[derive(Clone, Copy, Debug)]
struct UndoModifiers {
    node: usize,
    old: u16,
}

/// Restoration point for parser speculation: `rollback` truncates the open
/// stack, the arena, the pool, and every error-flag bit set since the mark,
/// and undoes every `patch` and `set_modifiers` write made since it —
/// including writes into nodes that predate the mark, which the pool and
/// open-stack truncations alone cannot reach.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Marker {
    open: usize,
    nodes: usize,
    pool: usize,
    error_log: usize,
    undo_log: usize,
    mod_log: usize,
    /// The builder's live-marker count when this marker was taken;
    /// `rollback` restores it, so undo logging stops again once the
    /// outermost marker of a speculation nest is consumed.
    marks: usize,
}

/// Builds one [`SyntaxTable`] from parser events.
#[derive(Debug, Default)]
pub struct AstBuilder {
    nodes: Vec<NodeHeader>,
    pool: Vec<u32>,
    open: Vec<OpenNode>,
    /// Open-stack indices whose `errored` bit was flipped false→true, in
    /// order — the undo log that makes error propagation rollback-safe.
    /// Recorded only while a marker is live (`marks`).
    error_log: Vec<usize>,
    /// `patch` writes as (word, previous value), in order — the undo log
    /// that makes payload back-fill rollback-safe even for slots
    /// materialized before the mark (a pool truncation cannot reach those
    /// words). Recorded only while a marker is live (`marks`).
    undo_log: Vec<UndoWrite>,
    /// `set_modifiers` writes as (open-stack index, previous bits), in
    /// order — the undo log that makes modifier collection rollback-safe
    /// even for nodes begun before the mark (an open-stack truncation
    /// cannot reach those). Recorded only while a marker is live (`marks`).
    mod_log: Vec<UndoModifiers>,
    /// Live [`Marker`]s. The three undo logs record only while this is
    /// non-zero: a rollback needs a marker, so with none live there is
    /// nothing to restore — and the production driver never speculates
    /// through the builder, so the steady-state parse allocates no undo
    /// memory.
    marks: usize,
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
    /// [`Self::patch`] (poison-filled in debug builds — see `POISON`), so
    /// a node that seals without patches allocates nothing.
    ///
    /// For shared-begin pairs the parser opens the kind it
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

    /// Sets the innermost open node's modifier-set bits (`ModSet`) — the
    /// header-flag home of the payload words eliminated in
    /// #174. `bits` must already be positioned inside the `MOD_MASK` mask
    /// (bits 1..=7); repeated calls OR into the set. Each state-changing
    /// write is logged so [`Self::rollback`] can undo it. The derived read
    /// side is the crate-root `mod_flags`/`first_modifier` accessors.
    ///
    /// With no node open (a bridge bug) the call is a no-op: a write with
    /// no target has nowhere to land. Debug builds assert the pairing
    /// invariant and the mask; release builds trust the parser.
    pub fn set_modifiers(&mut self, bits: u16) {
        debug_assert!(
            bits & !flags::MOD_MASK == 0,
            "modifier bits outside the mask"
        );
        let Some(index) = self.open.len().checked_sub(1) else {
            debug_assert!(false, "set_modifiers without an open node");
            return;
        };
        let node = &mut self.open[index];
        let next = node.mod_flags | bits;
        if next != node.mod_flags {
            if self.marks > 0 {
                self.mod_log.push(UndoModifiers {
                    node: index,
                    old: node.mod_flags,
                });
            }
            node.mod_flags = next;
        }
    }

    /// Seals the innermost open node as `end_kind`, spanning
    /// `[first_token, end_token)` — `end_token` is the *exclusive* bound
    /// (one past the node's last token; equal to `first_token` for
    /// all-synthetic nodes).
    ///
    /// An `end` with no node open (an unbalanced bridge event) is dropped:
    /// every other degradation would corrupt the arena. An `end_token`
    /// before the node's `first_token` (a bridge seal that carries no
    /// closer — the empty record type `()` seals after its last child and
    /// has none; a recovery shape that began at a parser-inserted token)
    /// seals the node zero-width at its head with the `ERROR` flag — the
    /// inverted range would make every span reader's arithmetic wrap, the
    /// flagged zero-width node is the same degraded face an all-synthetic
    /// node already has. Debug builds additionally assert that `end_kind`
    /// accepts the open kind (pairing table), that the sealed kind's
    /// payload fits the materialized slot, and that no payload word is
    /// still `POISON` (a missed `patch`); release builds trust the parser.
    pub fn end(&mut self, end_kind: NodeKind, end_token: u32) {
        let Some(mut node) = self.open.pop() else {
            debug_assert!(false, "end without a matching begin");
            return;
        };
        debug_assert!(
            accepts_begin(end_kind, node.kind),
            "end {end_kind:?} does not pair with open {:?}",
            node.kind
        );
        let end_token = if end_token < node.first_token {
            node.errored = true;
            node.first_token
        } else {
            end_token
        };
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

    /// Number of nodes built so far (the arena tail index the next node gets).
    #[must_use]
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// The most recently completed node's span head, `None` before any node.
    #[must_use]
    pub fn last_first_token(&self) -> Option<u32> {
        self.nodes.last().map(|h| h.first_token)
    }

    /// The most recently completed node's exclusive end token, `None` before
    /// any node. The read face for end events that carry no end token (the
    /// node seals right after its last child completed).
    #[must_use]
    pub fn last_end_token(&self) -> Option<u32> {
        self.nodes.last().map(|h| h.end_token)
    }

    /// Arena index where the most recently completed node's subtree starts —
    /// the node itself when childless, its `first_child` otherwise. The
    /// bridge reads it to adopt the operand that completed before a
    /// `beginBinaryExpression`-shaped event.
    #[must_use]
    pub fn last_subtree_start(&self) -> Option<u32> {
        let last = self.nodes.last()?;
        Some(if last.first_child == last.end_child {
            self.nodes.len() as u32 - 1
        } else {
            last.first_child
        })
    }

    /// Opens a container anchored at `first_token` that *adopts* the last
    /// `siblings` completed sibling subtrees as its leading children: their
    /// arena rows stay where they are, and the container's child range is
    /// laid back to cover them (`first_child` = the oldest adopted subtree).
    ///
    /// The parser's event order needs this where the official builder pops
    /// already-built operands off its own stack (`beginBinaryExpression`
    /// fires after the left operand parsed — "operands = children×2"; same for `Assignment`, `Qualified`, `DottedName`,
    /// `LabeledStatement`'s labels, `ParenthesizedCondition`'s condition).
    /// A bridge that asks for no siblings, more siblings than the arena
    /// holds, or an adoption before any node completed gets the plain-begin
    /// shape: the node still opens (its paired `end` must pop *it*, not an
    /// ancestor) with whatever adoption the arena actually supports —
    /// debug builds assert the invariants, release builds degrade the
    /// parameter, never the pairing.
    pub fn begin_adopting(&mut self, kind: NodeKind, first_token: u32, siblings: u32) {
        debug_assert!(siblings >= 1, "begin_adopting with no siblings to adopt");
        debug_assert!(!self.root_sealed, "begin after the root was sealed");
        let mut start = self.nodes.len() as u32;
        if siblings >= 1 {
            if let Some(last) = self.last_subtree_start() {
                start = last;
                for _ in 1..siblings {
                    if start == 0 {
                        debug_assert!(false, "begin_adopting walks past the arena head");
                        break;
                    }
                    start = {
                        let prev = start - 1;
                        let h = &self.nodes[prev as usize];
                        if h.first_child == h.end_child {
                            prev
                        } else {
                            h.first_child
                        }
                    };
                }
            } else {
                debug_assert!(false, "begin_adopting before any node completed");
            }
        }
        self.open.push(OpenNode {
            kind,
            first_token,
            first_child: start,
            payload_start: None,
            mod_flags: 0,
            errored: false,
        });
    }

    /// Opens a container anchored at `first_token` with an explicit
    /// `first_child` — the form for adoption segments the caller tracked
    /// itself (the `SwitchCase` case body: labels, case expressions, and
    /// guard clauses complete before `begin_switch_case` fires, in a mix no
    /// sibling count can walk; the listener snapshots the arena tail at the
    /// previous case boundary and hands it here). `first_child` must be a
    /// subtree boundary — `0`, the arena tail, or one past a completed
    /// subtree root; a start strictly inside a completed subtree would split
    /// that subtree's rows between its owner and this node, silently
    /// breaking parent/window consistency (debug builds assert it).
    ///
    /// A `first_child` past the arena tail (a bridge bug) clamps to the
    /// tail in release builds — the node opens with only the rows completed
    /// while it stays open as children — and debug builds assert the
    /// invariant.
    pub fn begin_with_first_child(&mut self, kind: NodeKind, first_token: u32, first_child: u32) {
        debug_assert!(!self.root_sealed, "begin after the root was sealed");
        debug_assert!(
            first_child as usize <= self.nodes.len(),
            "begin_with_first_child past the arena tail"
        );
        let first_child = first_child.min(self.nodes.len() as u32);
        // Well-nestedness: a non-boundary start lies strictly inside some
        // completed subtree, which is exactly "the previous completed row
        // (`first_child - 1`) is not a forest root". That row is covered as
        // a descendant iff some later row has `first_child <= first_child -
        // 1`; rows before the boundary are outside the scanned suffix, and
        // every scanned row's own subtree either starts at or after the
        // boundary (fine) or straddles it (the bug).
        debug_assert!(
            first_child == 0
                || first_child as usize == self.nodes.len()
                || !self.nodes[first_child as usize..]
                    .iter()
                    .any(|h| h.first_child < first_child),
            "begin_with_first_child strictly inside a completed subtree"
        );
        self.open.push(OpenNode {
            kind,
            first_token,
            first_child,
            payload_start: None,
            mod_flags: 0,
            errored: false,
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
    /// With no node open (a bridge bug) the write is skipped — a write with
    /// no target has nowhere to land and nothing to record the error on.
    /// Debug builds assert the pairing invariant and that the field lies
    /// inside the open node's slot; release builds record a field outside
    /// the slot through the error log and skip the write (it would
    /// overwrite a neighboring node's payload word).
    pub fn patch(&mut self, field: usize, value: u32) {
        let (kind, payload_start) = match self.open.last() {
            Some(node) => (node.kind, node.payload_start),
            None => {
                debug_assert!(false, "patch without an open node");
                return;
            }
        };
        if field >= payload_len(kind) as usize {
            // A field outside the open node's slot addresses a NEIGHBORING
            // node's pool word — silent corruption. The shape reaches here
            // from a bridge event that patches a child's field while the
            // child was never begun (a deferred-seal parent such as
            // `AsExpression` is the open node when the type production's
            // end event patches its own words): the error is recorded
            // through the builder's error log (it propagates to ancestors
            // at seal and stays rollback-safe) and the write is skipped,
            // in every build.
            self.mark_error();
            return;
        }
        let start = if let Some(start) = payload_start {
            start
        } else {
            let start = self.reserve_payload(payload_len(kind) as usize);
            self.open
                .last_mut()
                .expect("patch without an open node")
                .payload_start = Some(start);
            start
        };
        let slot = start as usize + field;
        if self.marks > 0 {
            let old = self.pool[slot];
            self.undo_log.push(UndoWrite {
                word: slot as u32,
                old,
            });
        }
        self.pool[slot] = value;
    }

    /// Back-fills the innermost open node's payload word named by `field`
    /// — the named face of [`Self::patch`]: the word index resolves against
    /// the open node's kind through [`crate::fields::PAYLOAD_FIELDS`], so a
    /// field that does not belong to that kind is a named debug failure
    /// instead of a silent wrong-word write (release degrades through the
    /// error log exactly like an out-of-slot index: the write is skipped).
    pub fn patch_field(&mut self, field: PayloadField, value: u32) {
        let open_kind = self.open.last().map(|node| node.kind);
        let Some(kind) = open_kind else {
            debug_assert!(false, "patch without an open node");
            return;
        };
        match crate::fields::field_index(kind, field) {
            Some(index) => self.patch(index as usize, value),
            None => {
                debug_assert!(
                    false,
                    "{field:?} is not a payload field of the open {kind:?}"
                );
                self.mark_error();
            }
        }
    }

    /// Marks an error inside the innermost open node; the bit propagates to
    /// every ancestor as nodes seal. Diagnostics themselves
    /// never enter the AST — they flow through the listener to the
    /// diagnostics channel.
    ///
    /// With no node open (a bridge bug) there is nothing to mark and the
    /// call is a no-op — a flag with no target has nowhere to land. Debug
    /// builds assert the pairing invariant; release builds trust the parser.
    pub fn mark_error(&mut self) {
        let Some(index) = self.open.len().checked_sub(1) else {
            debug_assert!(false, "mark_error without an open node");
            return;
        };
        self.set_errored(index);
    }

    fn set_errored(&mut self, index: usize) {
        let node = &mut self.open[index];
        if !node.errored {
            node.errored = true;
            if self.marks > 0 {
                self.error_log.push(index);
            }
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
    /// `TokenCursor::mark`/`reset`.
    ///
    /// While at least one marker is live, every `patch`, state-changing
    /// `set_modifiers`, and error-flag transition is logged for undo —
    /// with none live, nothing is logged (there is no marker to restore
    /// against), so a driver that never speculates through the builder
    /// allocates no undo memory.
    #[must_use]
    pub fn mark(&mut self) -> Marker {
        self.marks += 1;
        Marker {
            open: self.open.len(),
            nodes: self.nodes.len(),
            pool: self.pool.len(),
            error_log: self.error_log.len(),
            undo_log: self.undo_log.len(),
            mod_log: self.mod_log.len(),
            marks: self.marks,
        }
    }

    /// Discards every event since `marker`: the `patch` writes made since
    /// it (each word restored to its previous value — including writes into
    /// slots whose node predates the mark, which the pool truncation alone
    /// cannot reach), the `set_modifiers` writes made since it (each
    /// surviving node's bits restored likewise, against the open-stack
    /// truncation), the error-flag bits set on still-open nodes, the open
    /// stack, the arena, and the pool. Open nodes that survive the
    /// truncation but first materialized after the mark are dematerialized:
    /// their slot lived in the truncated pool tail, and the next `patch`
    /// re-materializes it.
    pub fn rollback(&mut self, marker: Marker) {
        for entry in self.undo_log.drain(marker.undo_log..).rev() {
            self.pool[entry.word as usize] = entry.old;
        }
        for &entry in self.mod_log[marker.mod_log..].iter().rev() {
            if entry.node < self.open.len() {
                self.open[entry.node].mod_flags = entry.old;
            }
        }
        self.mod_log.truncate(marker.mod_log);
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
            if node
                .payload_start
                .is_some_and(|start| start as usize >= marker.pool)
            {
                node.payload_start = None;
            }
        }
        // Restore the flag to its state at mark time: the root was already
        // sealed then iff nothing was open and the arena was non-empty.
        self.root_sealed = marker.open == 0 && marker.nodes > 0;
        // Markers nested inside this one are dropped with it; undo logging
        // resumes only because this marker's own liveness is restored.
        self.marks = marker.marks;
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
