//! The async-gap context face: a second, linear fold over the same
//! per-executable event buffer the promotion interpreter replays, tracking
//! await crossings, BuildContext-typed usages, and mounted-guard validity
//! (the analysis core of `use_build_context_synchronously`).
//!
//! The row contract lives on [`crate::FlowFact::AsyncContextUse`]; this
//! module owns the walk that produces it. The decision order mirrors the
//! upstream visitor (upstream Dart SDK source,
//! `pkg/linter/lib/src/rules/use_build_context_synchronously.dart`): a
//! usage's verdict is the first interesting state a backward walk would
//! find, expressed forward — awaits open gap segments, guard-forming
//! constructs close them per branch polarity, and transparent constructs
//! pass the pending state through. An awaited value carries its crossing
//! into the enclosing guard folds: an await inside a guard expression
//! folds that guard to asynchronous (the upstream tables' `Gap`
//! operand), re-opening the gap on the construct's branches. Branch
//! merge rules, short-circuit
//! polarity tables, bool-equality folds, and the loop/try/switch
//! compositions follow the upstream tables; branch termination (the
//! `terminatesControl` input) is recognized syntactically
//! (return/throw/rethrow/break/continue), which is exact for the shapes
//! the official corpus exercises.

use dartq_parser::{IdentifierContext, TokenSpan};
use rustc_hash::FxHashMap;

use crate::ev::{BinOp, Ev};
use crate::facts::{AsyncOrigin, FlowFact};
use crate::ty::Base;

/// Which context origin a value or check belongs to (the row's
/// [`AsyncOrigin`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Side {
    Context,
    State,
}

const SIDES: [Side; 2] = [Side::Context, Side::State];

impl Side {
    fn index(self) -> usize {
        match self {
            Side::Context => 0,
            Side::State => 1,
        }
    }
}

/// The async state of one guard expression, per side (upstream
/// `AsyncState`, cut to what a usage's verdict needs): `None` is the
/// upstream `null` (uninteresting), `Gap` is `asynchronous`, `Pos`/`Neg`
/// are `mountedCheck`/`notMountedCheck`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum G {
    None,
    Gap,
    Pos(Side),
    Neg(Side),
}

impl G {
    fn flip(self) -> G {
        match self {
            G::Pos(side) => G::Neg(side),
            G::Neg(side) => G::Pos(side),
            other => other,
        }
    }
}

type Guard = [G; 2];

const NO_GUARD: Guard = [G::None, G::None];

/// The upstream `isAnd` table: the combined state of `a && b` for one
/// side (`null` = [`G::None`]).
fn combine_and(a: G, b: G) -> G {
    match (a, b) {
        (G::None, _) => b,
        (_, G::None) => a,
        (_, G::Gap) => G::Gap,
        (G::Gap, _) => b,
        (G::Pos(_) | G::Neg(_), _) => a,
    }
}

/// The upstream `isOr` table.
fn combine_or(a: G, b: G) -> G {
    match (a, b) {
        (G::Gap, _) | (_, G::Gap) => G::Gap,
        (_, G::Neg(_)) => b,
        (G::Pos(_), G::Pos(_)) | (G::Neg(_), _) => a,
        (_, _) => G::None,
    }
}

/// The upstream if-like condition combine (expression state, case-clause
/// state), used to fold a condition position's markers.
fn combine_cond(a: G, b: G) -> G {
    match (a, b) {
        (G::None, _) => b,
        (_, G::None) => a,
        (_, G::Gap) => G::Gap,
        (G::Gap, _) => b,
        _ => a,
    }
}

/// One side's pending verdict state: what a usage of that origin sees
/// here. `unrel` is the upstream `hasUnrelatedMountedCheck` accumulator;
/// `gap`/`gap_at` identify the crossing a `Gap` state came from.
#[derive(Clone, Copy)]
struct Slot {
    st: St,
    unrel: bool,
    gap: u32,
    gap_at: TokenSpan,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum St {
    Clear,
    Gap,
    Guarded,
    GuardedNot,
}

/// A zero span (the synthetic anchor of degenerate rows).
const ZERO_SPAN: TokenSpan = TokenSpan {
    id: 0,
    range: dartq_source::SourceRange {
        offset: 0,
        length: 0,
    },
};

impl Slot {
    const CLEAR: Slot = Slot {
        st: St::Clear,
        unrel: false,
        gap: 0,
        gap_at: ZERO_SPAN,
    };

    fn gap(gap: u32, gap_at: TokenSpan) -> Slot {
        Slot {
            st: St::Gap,
            unrel: false,
            gap,
            gap_at,
        }
    }
}

/// The expression-stack marker kinds the face distinguishes.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// Not context-shaped.
    Other,
    /// A bool literal (the `== true` fold's operand).
    Bool(bool),
    /// A property name after `.` awaiting its [`Ev::DotAccess`].
    Member(u32),
    /// `this` (the State receiver shape for `.mounted`/`.context`).
    This,
    /// A BuildContext-typed local/parameter; `nullable` is the declared
    /// form (arguments require the non-nullable one).
    Ctx { nullable: bool },
    /// A context value of State origin (bare unresolved `context`,
    /// `this.context`): a use, and a State-side check receiver.
    StVal,
    /// A State-typed reference: a check receiver, never a use.
    StRef,
}

/// One expression-stack marker.
#[derive(Clone, Copy)]
struct Mk {
    kind: Kind,
    span: TokenSpan,
    /// The guard summary carried for enclosing guard positions.
    guard: Guard,
    /// The verdict snapshot at the value's evaluation point (an access
    /// use reads this, not the later access-time state).
    snap: [Slot; 2],
    /// The marker is an argument list's result ([`Ev::ArgumentsEnd`]):
    /// the call's own send nets it — folding the bare callee head beneath
    /// it — so a call contributes exactly one marker.
    net: bool,
}

impl Mk {
    const OTHER: Mk = Mk {
        kind: Kind::Other,
        span: ZERO_SPAN,
        guard: NO_GUARD,
        snap: [Slot::CLEAR; 2],
        net: false,
    };
}

/// Construct frames of the statement walk.
enum Frame {
    /// A nested executable (closure/local function): opaque — its rows
    /// are suppressed and its awaits never touch the enclosing body.
    Exec {
        base: usize,
        saved: [Slot; 2],
        saved_loops: u32,
        saved_scope: usize,
    },
    /// An assert: awaits inside affect only uses inside (the upstream
    /// walk never crosses the statement).
    Assert {
        base: usize,
        saved: [Slot; 2],
    },
    If {
        pre: [Slot; 2],
        base: usize,
        /// The condition's folded guard and its unrel accumulations,
        /// latched at `ThenBegin` for the `IfEnd` merge.
        cond: Guard,
        cond_unrel: [bool; 2],
        cond_gap: (u32, TokenSpan),
        /// The verdict slots at the split (latched at `ThenBegin`, before
        /// the branch map): a mounted check in the condition sets its
        /// Guarded verdict before the branches stream, and the branch its
        /// polarity does not select keeps that verdict — only the merge at
        /// `IfEnd` re-derives from the pre-state.
        check_entry: [Slot; 2],
        after_then: Option<[Slot; 2]>,
        then_term: bool,
        else_term: bool,
    },
    Loop {
        pre: [Slot; 2],
        base: usize,
        /// Await count when the construct began.
        entry: u64,
        /// Await count when the body began (the loop-back boundary).
        body_entry: u64,
        /// The header's folded guard, latched at `WhileBodyBegin`/`ForParts`/
        /// `DoEnd` for the after-loop merge.
        cond: Guard,
        /// C-style header async, latched at `ForParts`.
        header_gap: bool,
        /// An exit (return/throw) was seen inside the body: the exit
        /// path skips the remaining iterations, so the after-loop merge
        /// drops the body-gap upgrade (under-approximates the
        /// reachability join, never over-flags).
        exits: bool,
    },
    Try {
        pre: [Slot; 2],
        base: usize,
        entry: u64,
        /// Await count at the first catch clause (the try body's exit).
        body_awaits: Option<u64>,
        /// Await count at the last closed clause.
        prev_clause: u64,
        /// Body-or-catch async, latched when the finally begins.
        finally_gap: bool,
    },
    Switch {
        is_expr: bool,
        pre: [Slot; 2],
        base: usize,
        entry: u64,
        /// The verdict slots when the block began (post-scrutinee).
        block_entry: [Slot; 2],
        /// Await count when the member list began.
        member_start: u64,
    },
    /// A `?:` expression (arms never terminate control, upstream
    /// `terminatesControl`'s statement-only gate).
    Cond {
        pre: [Slot; 2],
        base: usize,
        cond: Guard,
        after_then: Option<[Slot; 2]>,
    },
    CollIf {
        pre: [Slot; 2],
        base: usize,
        /// The condition's folded guard (latched at `ThenControlFlow`).
        cond: Guard,
        after_then: Option<[Slot; 2]>,
    },
    CollFor {
        pre: [Slot; 2],
        base: usize,
        entry: u64,
    },
    Cascade {
        base: usize,
    },
}

/// A buffered Clear verdict: materialized as a row when a later await
/// upgrades it (loop-back: the next iteration runs the code after the use
/// first; switch members: any member's await flags the whole switch).
struct PendingUse {
    span: TokenSpan,
    origin: AsyncOrigin,
    /// The owning loop frame's stack index, when loop-nested.
    loop_at: Option<u32>,
    /// The owning switch-statement frame's stack index, when inside a
    /// switch member.
    switch_at: Option<u32>,
    awaits_at: u64,
}

/// The per-executable async-gap walk.
pub(crate) struct GapPass {
    // Interned spelling ids (resolved once per unit by the recorder).
    context_ty: u32,
    state_ty: u32,
    mounted: u32,
    context_name: u32,
    pub(crate) facts: Vec<FlowFact>,
    /// Verdict slots per origin: the state a usage of that origin sees.
    slots: [Slot; 2],
    /// Await crossing ordinal.
    gap: u32,
    last_await: TokenSpan,
    awaits: u64,
    /// Open await keywords (nested awaits pair innermost-last).
    await_stack: Vec<TokenSpan>,
    stack: Vec<Frame>,
    markers: Vec<Mk>,
    /// Declared-type face per local binding.
    vars: Vec<Kind>,
    scopes: Vec<FxHashMap<u32, u32>>,
    loops: u32,
    closures: u32,
    /// Type rows are gated off inside `is`/`as` spellings.
    type_gated: u32,
    pending_ty: Kind,
    in_decl: bool,
    /// The open declaration list's type face and the enclosing lists'.
    decl: (Kind, Vec<Kind>),
    pending_uses: Vec<PendingUse>,
}

impl GapPass {
    pub(crate) fn new(context_ty: u32, state_ty: u32, mounted: u32, context_name: u32) -> Self {
        Self {
            context_ty,
            state_ty,
            mounted,
            context_name,
            facts: Vec::new(),
            slots: [Slot::CLEAR; 2],
            gap: 0,
            last_await: ZERO_SPAN,
            awaits: 0,
            await_stack: Vec::new(),
            stack: Vec::new(),
            markers: Vec::new(),
            vars: Vec::new(),
            scopes: vec![FxHashMap::default()],
            loops: 0,
            closures: 0,
            type_gated: 0,
            pending_ty: Kind::Other,
            in_decl: false,
            decl: (Kind::Other, Vec::new()),
            pending_uses: Vec::new(),
        }
    }

    /// Runs the walk over one executable's buffer. Linear: the cursor
    /// advances one event per step, no jumps, every recovery-shaped pop
    /// degrades to an inert marker — the engine termination invariant
    /// holds by construction.
    pub(crate) fn run(mut self, events: &[Ev]) -> Self {
        for ev in events {
            self.step(ev);
        }
        // The body-final upgrade: a still-Clear use nested in a loop sees
        // the whole enclosing block's backward scan — a Gap in the
        // function's fall-through state flags it (the upstream ascent
        // full-scans each enclosing level).
        let final_gap = [self.slots[0].st == St::Gap, self.slots[1].st == St::Gap];
        for u in std::mem::take(&mut self.pending_uses) {
            if final_gap[match u.origin {
                AsyncOrigin::Context => 0,
                AsyncOrigin::State => 1,
            }] {
                self.emit_use(u.span, u.origin, true);
            }
        }
        self
    }

    // —— slots and rows ————————————————————————————————————————————————

    /// A mounted-name check on `side` (or on an unresolved receiver):
    /// every mismatched origin accumulates the wrong-mounted flag.
    fn observe_check(&mut self, side: Option<Side>) {
        for s in SIDES {
            if side != Some(s) {
                self.slots[s.index()].unrel = true;
            }
        }
    }

    /// Opens the gap: every origin's verdict becomes Gap (the unrel
    /// flags reset — the upstream walk stops here, dropping older flags).
    fn open_gap(&mut self) {
        let slot = Slot::gap(self.gap, self.last_await);
        self.slots = [slot, slot];
    }

    /// Records a context usage. Clear verdicts buffer while a loop or a
    /// switch member owns the point (their verdicts depend on awaits that
    /// have not streamed yet); everything else emits unless a closure
    /// owns it (the protected-callback face is not modeled, and the
    /// plain walk never reports closure-internal uses).
    fn use_at(&mut self, span: TokenSpan, origin: AsyncOrigin) {
        if self.closures > 0 {
            return;
        }
        let slot = self.slots[match origin {
            AsyncOrigin::Context => 0,
            AsyncOrigin::State => 1,
        }];
        if slot.st == St::Clear {
            self.buffer(span, origin);
        } else {
            self.emit_at(span, origin, slot, false);
        }
    }

    /// Emits one row from `slot`. `upgraded` rows read the nearest
    /// crossing (their own verdict slot was Clear — the await that flags
    /// them is the one the loop-back scan found).
    fn emit_use(&mut self, span: TokenSpan, origin: AsyncOrigin, upgraded: bool) {
        let slot = self.slots[match origin {
            AsyncOrigin::Context => 0,
            AsyncOrigin::State => 1,
        }];
        self.emit_at(span, origin, slot, upgraded);
    }

    /// The loop-back upgrade at a loop's end: awaits after a buffered
    /// Clear use run before that use's next iteration.
    fn upgrade_loop(&mut self, frame_index: usize, body_entry: u64) {
        if self.awaits <= body_entry {
            return;
        }
        let index = u32::try_from(frame_index).ok();
        let mut i = 0;
        while i < self.pending_uses.len() {
            if self.pending_uses[i].loop_at == index && self.pending_uses[i].awaits_at <= body_entry
            {
                let u = self.pending_uses.remove(i);
                self.emit_use(u.span, u.origin, true);
            } else {
                i += 1;
            }
        }
    }

    /// The switch-member resolution at the statement's end: any member's
    /// await flags every buffered member use (upstream scans the whole
    /// member list); a gap-free switch drops them.
    fn resolve_switch(&mut self, frame_index: usize, member_gap: bool) {
        let index = u32::try_from(frame_index).ok();
        let mut i = 0;
        while i < self.pending_uses.len() {
            if self.pending_uses[i].switch_at == index {
                let u = self.pending_uses.remove(i);
                if member_gap {
                    self.emit_use(u.span, u.origin, true);
                }
            } else {
                i += 1;
            }
        }
    }

    // —— marker helpers ————————————————————————————————————————————————

    fn push_val(&mut self, kind: Kind, span: TokenSpan, guard: Guard) {
        self.markers.push(Mk {
            kind,
            span,
            guard,
            snap: self.slots,
            net: false,
        });
    }

    fn pop_marker(&mut self) -> Mk {
        self.markers.pop().unwrap_or(Mk::OTHER)
    }

    fn pop_val(&mut self) -> Mk {
        let m = self.pop_marker();
        if matches!(m.kind, Kind::Member(_)) {
            Mk::OTHER
        } else {
            m
        }
    }

    /// Pops every marker above `base`, folding their guards in push order
    /// with the upstream condition combine (the if-case's
    /// (expression, caseClause) fold generalizes to N markers).
    fn fold_to(&mut self, base: usize) -> Guard {
        let mut acc = NO_GUARD;
        let mut popped = Vec::new();
        while self.markers.len() > base {
            popped.push(self.markers.pop().unwrap_or(Mk::OTHER));
        }
        for m in popped.into_iter().rev() {
            for (a, g) in acc.iter_mut().zip(m.guard) {
                *a = combine_cond(*a, g);
            }
        }
        acc
    }

    /// The branch-entry map of a condition guard (upstream
    /// `_visitIfLike`'s branch switches), read for the slot of `side`.
    /// `positive` selects the then-branch mapping (positive checks guard
    /// the then; negative checks guard the else). `check_fallback` is the
    /// split-time verdict a mounted check left on this side: when the
    /// condition carries a check, the branch its polarity does not select
    /// keeps that verdict instead of the pre-state (the `IfEnd` merge
    /// re-derives from the pre-state); conditions without a check fall
    /// through to the pre-state plus the condition's unrel accumulations.
    fn branch_entry(
        &self,
        side: Side,
        pre: Slot,
        c: G,
        cond_unrel: bool,
        positive: bool,
        check_fallback: Option<Slot>,
    ) -> Slot {
        let mine = match c {
            G::Pos(s) => positive && s == side,
            G::Neg(s) => !positive && s == side,
            _ => false,
        };
        if mine {
            return Slot {
                st: St::Guarded,
                unrel: false,
                gap: pre.gap,
                gap_at: pre.gap_at,
            };
        }
        match c {
            G::Gap => Slot::gap(self.gap, self.last_await),
            _ => check_fallback.unwrap_or(Slot {
                st: pre.st,
                unrel: pre.unrel || cond_unrel,
                gap: pre.gap,
                gap_at: pre.gap_at,
            }),
        }
    }

    /// The upstream if-merge for one side, in the upstream rule order
    /// (`_visitIfLike`'s reference-follows block). The `unrel`
    /// accumulations follow the walk-visibility rule: a stop state takes
    /// the branch-tail flags, a fall-through takes the pre-state's plus
    /// the condition's.
    #[expect(clippy::too_many_arguments)] // mirrors `_visitIfLike`'s 8-value merge signature; bundling would rename without clarifying
    fn merge_if(
        pre: Slot,
        c: G,
        cond_unrel: bool,
        cond_gap: (u32, TokenSpan),
        t: Slot,
        e: Option<Slot>,
        then_term: bool,
        else_term: bool,
    ) -> Slot {
        let guarded_not = |unrel: bool| Slot {
            st: St::GuardedNot,
            unrel,
            gap: pre.gap,
            gap_at: pre.gap_at,
        };
        if t.st == St::GuardedNot && (e.is_some_and(|e| e.st == St::GuardedNot) || else_term) {
            return guarded_not(t.unrel);
        }
        if let Some(e) = e
            && e.st == St::GuardedNot
            && then_term
        {
            return guarded_not(e.unrel);
        }
        if t.st == St::Gap && !then_term {
            return t;
        }
        if let Some(e) = e
            && e.st == St::Gap
            && !else_term
        {
            return e;
        }
        if c == G::Gap {
            return Slot::gap(cond_gap.0, cond_gap.1);
        }
        if (matches!(c, G::Pos(_)) && else_term) || (matches!(c, G::Neg(_)) && then_term) {
            return guarded_not(cond_unrel);
        }
        Slot {
            st: pre.st,
            unrel: pre.unrel || cond_unrel,
            gap: pre.gap,
            gap_at: pre.gap_at,
        }
    }
}

impl GapPass {
    // —— row and verdict helpers ————————————————————————————————————————

    /// Emits one row from an explicit verdict slot (a target access reads
    /// its receiver's latched snapshot; an argument reads the current
    /// slots). `upgraded` rows read the nearest crossing — their own
    /// verdict was Clear and the flagging await is the one the loop-back
    /// scan found.
    fn emit_at(&mut self, span: TokenSpan, origin: AsyncOrigin, slot: Slot, upgraded: bool) {
        let (gap, gap_at, guarded) = if upgraded || slot.st == St::Clear {
            (self.gap, self.last_await, false)
        } else {
            (slot.gap, slot.gap_at, slot.st != St::Gap)
        };
        self.facts.push(FlowFact::AsyncContextUse {
            span,
            origin,
            gap,
            gap_at,
            guarded,
            unrelated_mounted: !guarded && slot.unrel,
        });
    }

    /// Records a usage with a latched verdict (invocation targets and
    /// getter accesses evaluate before the call's arguments).
    fn use_latched(&mut self, span: TokenSpan, origin: AsyncOrigin, snap: Slot) {
        if self.closures > 0 {
            return;
        }
        if snap.st == St::Clear {
            self.buffer(span, origin);
        } else {
            self.emit_at(span, origin, snap, false);
        }
    }

    /// Parks a Clear verdict until the owning loop or switch statement
    /// resolves it.
    fn buffer(&mut self, span: TokenSpan, origin: AsyncOrigin) {
        let loop_at = if self.loops > 0 {
            self.stack
                .iter()
                .rposition(|f| matches!(f, Frame::Loop { .. }))
                .and_then(|i| u32::try_from(i).ok())
        } else {
            None
        };
        let switch_at = if loop_at.is_none() {
            self.stack
                .iter()
                .rposition(|f| matches!(f, Frame::Switch { is_expr: false, .. }))
                .and_then(|i| u32::try_from(i).ok())
        } else {
            None
        };
        if loop_at.is_some() || switch_at.is_some() {
            self.pending_uses.push(PendingUse {
                span,
                origin,
                loop_at,
                switch_at,
                awaits_at: self.awaits,
            });
        }
    }

    /// Declares a binding in the innermost scope.
    fn declare(&mut self, name: u32, kind: Kind) {
        if let Some(scope) = self.scopes.last_mut()
            && let Some(index) = u32::try_from(self.vars.len()).ok()
        {
            self.vars.push(kind);
            scope.insert(name, index);
        }
    }

    /// The declared-type face of a name (innermost scope wins); `None`
    /// when undeclared (the bare Flutter names resolve by spelling).
    fn resolve(&self, name: u32) -> Option<Kind> {
        for scope in self.scopes.iter().rev() {
            if let Some(&index) = scope.get(&name)
                && let Some(kind) = self.vars.get(index as usize)
            {
                return Some(*kind);
            }
        }
        None
    }

    fn push_loop(&mut self) {
        self.loops += 1;
        self.stack.push(Frame::Loop {
            pre: self.slots,
            base: self.markers.len(),
            entry: self.awaits,
            body_entry: self.awaits,
            cond: NO_GUARD,
            header_gap: false,
            exits: false,
        });
    }

    /// Closes the top loop frame: resolves its buffered uses against the
    /// body's awaits (the loop-back upgrade), then merges the after-loop
    /// state — the condition's or header's gap, or any body await, puts
    /// the continuation behind a crossing.
    fn close_loop(&mut self, header_cond: Option<Guard>) {
        let Some(index) = self
            .stack
            .iter()
            .rposition(|f| matches!(f, Frame::Loop { .. }))
        else {
            return;
        };
        let Frame::Loop {
            pre,
            base,
            body_entry,
            cond,
            header_gap,
            exits,
            ..
        } = self.stack.remove(index)
        else {
            return;
        };
        self.markers.truncate(base);
        let c = header_cond.unwrap_or(cond);
        self.upgrade_loop(index, body_entry);
        self.reindex(index);
        let body_gap = !exits && self.awaits > body_entry;
        for i in 0..2 {
            self.slots[i] = if matches!(c[i], G::Gap) || header_gap || body_gap {
                Slot::gap(self.gap, self.last_await)
            } else {
                pre[i]
            };
        }
        self.loops = self.loops.saturating_sub(1);
    }

    /// The await gap since a switch member list began (any member's await
    /// flags the whole construct — the upstream member-list scan).
    fn member_gap(&self, is_expr: bool) -> bool {
        self.stack.iter().rev().any(|f| match f {
            Frame::Switch {
                is_expr: e,
                member_start,
                ..
            } if *e == is_expr => self.awaits > *member_start,
            _ => false,
        })
    }

    /// A return/throw (`exit`) or a break/continue (`jump`) terminates:
    /// every enclosing if arm left behind folds as terminated. Exits
    /// cross loops and switches (they leave the whole construct); jumps
    /// stop at the first loop, statement switch, or nested executable.
    fn terminate(&mut self, exit: bool) {
        for frame in self.stack.iter_mut().rev() {
            match frame {
                Frame::If {
                    after_then,
                    then_term,
                    else_term,
                    ..
                } => {
                    if after_then.is_none() {
                        *then_term = true;
                    } else {
                        *else_term = true;
                    }
                }
                Frame::Loop { exits, .. } if exit => *exits = true,
                Frame::Loop { .. } | Frame::Switch { is_expr: false, .. } | Frame::Exec { .. } => {
                    break;
                }
                _ => {}
            }
        }
    }

    /// Stack indices above a removed frame shift down by one; buffered
    /// uses referencing outer owners must follow.
    fn reindex(&mut self, removed: usize) {
        for u in &mut self.pending_uses {
            if let Some(at) = u.loop_at.as_mut()
                && usize::try_from(*at).is_ok_and(|a| a > removed)
            {
                *at -= 1;
            }
            if let Some(at) = u.switch_at.as_mut()
                && usize::try_from(*at).is_ok_and(|a| a > removed)
            {
                *at -= 1;
            }
        }
    }

    // —— the walk ————————————————————————————————————————————————————————

    #[expect(clippy::too_many_lines)] // the per-event dispatch mirrors the upstream event walk; splitting would scatter the table
    fn step(&mut self, ev: &Ev) {
        match ev {
            // —— executable framing ——
            // The buffer's own executable frames eventlessly (the recorder
            // starts the buffer at its body), so every ExecBegin/ExecEnd
            // EVENT in the buffer belongs to a nested executable.
            Ev::ExecBegin { .. } => {
                self.stack.push(Frame::Exec {
                    base: self.markers.len(),
                    saved: self.slots,
                    saved_loops: self.loops,
                    saved_scope: self.scopes.len(),
                });
                self.closures += 1;
            }
            Ev::ExecEnd { is_expr, .. } => {
                if let Some(Frame::Exec {
                    base,
                    saved,
                    saved_loops,
                    saved_scope,
                }) = self.stack.pop()
                {
                    self.closures -= 1;
                    self.markers.truncate(base);
                    self.scopes.truncate(saved_scope);
                    self.loops = saved_loops;
                    self.slots = saved;
                }
                if *is_expr {
                    self.markers.push(Mk::OTHER);
                }
            }
            // Events the pass deliberately ignores: framing and type shape
            // it does not track, pattern and guard scaffolding, loop and
            // jump sub-event markers whose state moves at their End events,
            // and expression wrappers whose operands fold at their End
            // events.
            Ev::BlockBodyBegin
            | Ev::BlockBodyEnd
            | Ev::AsyncModifier { .. }
            | Ev::EmptyBody
            | Ev::StmtEnd { .. }
            | Ev::IsOperator { .. }
            | Ev::AsOperator
            | Ev::NoInitializer
            | Ev::InitializerBegin
            | Ev::PatternVarDeclStmt { .. }
            | Ev::IfCaseCondition
            | Ev::Wildcard { .. }
            | Ev::WhileBodyEnd
            | Ev::DoBodyEnd
            | Ev::ForInit
            | Ev::ForBodyEnd
            | Ev::ForInParts
            | Ev::ForInExprBegin
            | Ev::ForInBodyEnd
            | Ev::ReturnBegin { .. }
            | Ev::YieldBegin { .. }
            | Ev::LabeledBegin { .. }
            | Ev::LabelDecl { .. }
            | Ev::LabeledEnd
            | Ev::SwitchBlockEnd
            | Ev::CaseExprBegin { .. }
            | Ev::CaseNoWhen
            | Ev::CaseEnd { .. }
            | Ev::SwitchExprCasePattern
            | Ev::PatternBegin
            | Ev::PatternEnd
            | Ev::ObjectPattern { .. }
            | Ev::ShapePattern
            | Ev::NullCheckPattern
            | Ev::ConstantPatternBegin
            | Ev::ConstantPatternEnd
            | Ev::GuardBegin
            | Ev::NonNullAssert
            | Ev::BinaryBegin { .. }
            | Ev::ArgumentsBegin
            | Ev::ParenBegin
            | Ev::ParenEnd => {}
            Ev::ExprStmt { .. }
            | Ev::InitializerEnd
            | Ev::ForInExprEnd
            | Ev::YieldEnd { .. }
            | Ev::SwitchExprCaseEnd { .. } => {
                self.pop_val();
            }
            Ev::ExprBody | Ev::CaseExprEnd { .. } => {
                self.pop_marker();
            }

            // —— the async gap ——
            Ev::AwaitBegin { token } => self.await_stack.push(*token),
            Ev::AwaitEnd { .. } => {
                if let Some(token) = self.await_stack.pop()
                    && self.await_stack.is_empty()
                {
                    self.awaits += 1;
                    self.gap += 1;
                    self.last_await = token;
                    self.open_gap();
                    // The awaited value carries the crossing into every
                    // enclosing guard fold: its marker's guard becomes
                    // asynchronous, so the `combine_and`/`combine_or`/
                    // `combine_cond` tables propagate the gap (upstream
                    // `isAnd`/`isOr`'s asynchronous operand, order kept:
                    // `asynchronous && mountedCheck` still folds to the
                    // check) and the branch maps re-open it — an await
                    // inside a mounted-check condition leaves the branch
                    // asynchronous, not guarded.
                    if let Some(m) = self.markers.last_mut() {
                        m.guard = [G::Gap, G::Gap];
                    }
                }
            }

            // —— types ——
            Ev::Type { base, nullable, .. } => {
                if self.type_gated == 0 && !self.in_decl {
                    self.pending_ty = match base {
                        Base::Name(id) if *id == self.context_ty => Kind::Ctx {
                            nullable: *nullable,
                        },
                        Base::Name(id) if *id == self.state_ty => Kind::StRef,
                        _ => Kind::Other,
                    };
                }
            }
            Ev::NoType | Ev::FunctionType { .. } | Ev::VoidType => {
                if self.type_gated == 0 && !self.in_decl {
                    self.pending_ty = Kind::Other;
                }
            }
            Ev::IsBegin | Ev::AsBegin { .. } => self.type_gated += 1,
            Ev::IsEnd | Ev::AsEnd => self.type_gated = self.type_gated.saturating_sub(1),

            // —— declarations ——
            Ev::Formal { name, .. } => {
                let kind = std::mem::replace(&mut self.pending_ty, Kind::Other);
                self.declare(*name, kind);
            }
            Ev::VarDeclBegin { .. } => {
                let ty = std::mem::replace(&mut self.pending_ty, Kind::Other);
                let outer = std::mem::replace(&mut self.decl.0, ty);
                self.decl.1.push(outer);
                self.in_decl = true;
            }
            Ev::InitializedIdent { name, .. } => {
                let kind = self.decl.0;
                self.declare(*name, kind);
                self.in_decl = false;
            }
            Ev::VarDeclEnd => {
                if let Some(outer) = self.decl.1.pop() {
                    self.decl.0 = outer;
                }
                self.in_decl = false;
            }
            Ev::DeclaredVarPattern { name, .. } | Ev::AssignedVarPattern { name } => {
                self.declare(*name, Kind::Other);
            }

            // —— blocks ——
            Ev::Block { .. } => self.scopes.push(FxHashMap::default()),
            Ev::EndBlock { .. } => {
                if self.scopes.len() > 1 {
                    self.scopes.pop();
                }
            }

            // —— if ——
            Ev::IfBegin { .. } => self.stack.push(Frame::If {
                pre: self.slots,
                base: self.markers.len(),
                cond: NO_GUARD,
                cond_unrel: [false; 2],
                cond_gap: (self.gap, self.last_await),
                check_entry: self.slots,
                after_then: None,
                then_term: false,
                else_term: false,
            }),
            Ev::ThenBegin => {
                let (pre, base) = match self.stack.last() {
                    Some(Frame::If { pre, base, .. }) => (*pre, *base),
                    _ => return,
                };
                // The condition's markers fold in source order — the value
                // marker itself carries the folded guard (`==`/`!`/`&&`
                // shapes), so nothing is popped ahead of the fold.
                let c = self.fold_to(base);
                let cond_unrel = [
                    self.slots[0].unrel && !pre[0].unrel,
                    self.slots[1].unrel && !pre[1].unrel,
                ];
                let cond_gap = (self.gap, self.last_await);
                // The split-time verdict: a check in the condition set its
                // Guarded slots before this point; the branch its polarity
                // does not select keeps them (see `branch_entry`).
                let check_entry = self.slots;
                let checked =
                    matches!(c[0], G::Pos(_) | G::Neg(_)) || matches!(c[1], G::Pos(_) | G::Neg(_));
                if let Some(Frame::If {
                    cond,
                    cond_unrel: u,
                    cond_gap: g,
                    check_entry: entry,
                    ..
                }) = self.stack.last_mut()
                {
                    *cond = c;
                    *u = cond_unrel;
                    *g = cond_gap;
                    *entry = check_entry;
                }
                let fallback = |i| checked.then_some(check_entry[i]);
                for i in 0..2 {
                    self.slots[i] =
                        self.branch_entry(SIDES[i], pre[i], c[i], cond_unrel[i], true, fallback(i));
                }
            }
            Ev::ElseBegin => {
                self.pop_val();
                let (pre, c, cond_unrel, check_entry, checked) = match self.stack.last() {
                    Some(Frame::If {
                        pre,
                        cond,
                        cond_unrel,
                        check_entry,
                        ..
                    }) => {
                        let checked = matches!(cond[0], G::Pos(_) | G::Neg(_))
                            || matches!(cond[1], G::Pos(_) | G::Neg(_));
                        (*pre, *cond, *cond_unrel, *check_entry, checked)
                    }
                    _ => return,
                };
                if let Some(Frame::If { after_then, .. }) = self.stack.last_mut() {
                    *after_then = Some(self.slots);
                }
                let fallback = |i| checked.then_some(check_entry[i]);
                for i in 0..2 {
                    self.slots[i] = self.branch_entry(
                        SIDES[i],
                        pre[i],
                        c[i],
                        cond_unrel[i],
                        false,
                        fallback(i),
                    );
                }
            }
            Ev::IfEnd { has_else } => {
                if *has_else {
                    self.pop_val();
                }
                let Some(index) = self
                    .stack
                    .iter()
                    .rposition(|f| matches!(f, Frame::If { .. }))
                else {
                    return;
                };
                let Frame::If {
                    pre,
                    base,
                    cond,
                    cond_unrel,
                    cond_gap,
                    after_then,
                    then_term,
                    else_term,
                    ..
                } = self.stack.remove(index)
                else {
                    return;
                };
                self.markers.truncate(base);
                // With an else: `after_then` is the then tail, the current
                // slots the else tail. Without: the current slots ARE the
                // then tail.
                let (t, e) = if *has_else {
                    (after_then.unwrap_or(self.slots), Some(self.slots))
                } else {
                    (self.slots, None)
                };
                for i in 0..2 {
                    self.slots[i] = Self::merge_if(
                        pre[i],
                        cond[i],
                        cond_unrel[i],
                        cond_gap,
                        t[i],
                        e.map(|s| s[i]),
                        then_term,
                        else_term,
                    );
                }
            }

            // —— loops ——
            Ev::WhileBegin { .. } | Ev::DoBegin { .. } | Ev::ForBegin { .. } => self.push_loop(),
            Ev::WhileBodyBegin => {
                let top = self.pop_val();
                let base = match self.stack.last() {
                    Some(Frame::Loop { base, .. }) => *base,
                    _ => return,
                };
                let rest = self.fold_to(base);
                let c = [
                    combine_cond(top.guard[0], rest[0]),
                    combine_cond(top.guard[1], rest[1]),
                ];
                if let Some(Frame::Loop {
                    body_entry, cond, ..
                }) = self.stack.last_mut()
                {
                    *body_entry = self.awaits;
                    *cond = c;
                }
            }
            Ev::WhileEnd | Ev::ForEnd | Ev::ForInEnd => self.close_loop(None),
            Ev::DoBodyBegin => {
                if let Some(Frame::Loop { body_entry, .. }) = self.stack.last_mut() {
                    *body_entry = self.awaits;
                }
            }
            Ev::DoEnd { .. } => {
                let base = match self.stack.last() {
                    Some(Frame::Loop { base, .. }) => *base,
                    _ => return,
                };
                let c = self.fold_to(base);
                self.close_loop(Some(c));
            }
            Ev::ForParts {
                left_sep,
                right_sep,
            } => {
                let (left, right) = (left_sep.range.offset, right_sep.range.offset);
                let (base, pre) = match self.stack.last() {
                    Some(Frame::Loop { base, pre, .. }) => (*base, *pre),
                    _ => return,
                };
                let mut popped = Vec::new();
                while self.markers.len() > base {
                    popped.push(self.markers.pop().unwrap_or(Mk::OTHER));
                }
                let mut c = NO_GUARD;
                for m in popped.into_iter().rev() {
                    let at = m.span.range.offset;
                    if at >= left && at < right {
                        for (cg, g) in c.iter_mut().zip(m.guard) {
                            *cg = combine_cond(*cg, g);
                        }
                    }
                }
                let header_gap = matches!(c[0], G::Gap) || matches!(c[1], G::Gap);
                if let Some(Frame::Loop {
                    body_entry,
                    cond,
                    header_gap: h,
                    ..
                }) = self.stack.last_mut()
                {
                    *body_entry = self.awaits;
                    *cond = c;
                    *h = header_gap;
                }
                // The C-style condition's guard maps onto the body entry
                // in both polarities (upstream guards the body here; the
                // while condition does not — its Gap only).
                for ((i, slot), pre_s) in self.slots.iter_mut().enumerate().zip(pre) {
                    *slot = match c[i] {
                        G::Gap => Slot::gap(self.gap, self.last_await),
                        G::Pos(s) if s == SIDES[i] => Slot {
                            st: St::Guarded,
                            unrel: false,
                            gap: pre_s.gap,
                            gap_at: pre_s.gap_at,
                        },
                        G::Neg(s) if s == SIDES[i] => Slot {
                            st: St::GuardedNot,
                            unrel: false,
                            gap: pre_s.gap,
                            gap_at: pre_s.gap_at,
                        },
                        _ => *slot,
                    };
                }
            }
            Ev::ForBodyBegin => {
                if let Some(Frame::Loop { body_entry, .. }) = self.stack.last_mut() {
                    *body_entry = self.awaits;
                }
            }
            Ev::ForInBodyBegin => {
                if let Some(Frame::Loop {
                    entry,
                    body_entry,
                    header_gap,
                    ..
                }) = self.stack.last_mut()
                {
                    *body_entry = self.awaits;
                    *header_gap = self.awaits > *entry;
                }
            }

            // —— jumps and exits ——
            Ev::ReturnEnd { has_expression } => {
                if *has_expression {
                    self.pop_val();
                }
                self.terminate(true);
            }
            Ev::Break { .. } | Ev::Continue { .. } => self.terminate(false),

            // —— try ——
            Ev::TryBegin { .. } => self.stack.push(Frame::Try {
                pre: self.slots,
                base: self.markers.len(),
                entry: self.awaits,
                body_awaits: None,
                prev_clause: self.awaits,
                finally_gap: false,
            }),
            Ev::CatchClauseBegin => {
                let (pre, entry, stored) = match self.stack.last() {
                    Some(Frame::Try {
                        pre,
                        entry,
                        body_awaits,
                        ..
                    }) => (*pre, *entry, *body_awaits),
                    _ => return,
                };
                // The try body's exit IS the first catch clause's entry: on
                // the first clause the stored count is latched here, before
                // the gap questions read it.
                let body_awaits = stored.unwrap_or(self.awaits);
                if stored.is_none()
                    && let Some(Frame::Try { body_awaits: b, .. }) = self.stack.last_mut()
                {
                    *b = Some(body_awaits);
                }
                let body_gap = body_awaits > entry;
                let catch_gap = self.awaits > body_awaits;
                for (slot, pre_s) in self.slots.iter_mut().zip(pre) {
                    *slot = if body_gap || catch_gap {
                        Slot::gap(self.gap, self.last_await)
                    } else {
                        pre_s
                    };
                }
            }
            Ev::CatchClauseEnd => {
                if let Some(Frame::Try { prev_clause, .. }) = self.stack.last_mut() {
                    *prev_clause = self.awaits;
                }
            }
            Ev::FinallyBlock => {
                let (pre, entry, stored, prev_clause) = match self.stack.last() {
                    Some(Frame::Try {
                        pre,
                        entry,
                        body_awaits,
                        prev_clause,
                        ..
                    }) => (*pre, *entry, *body_awaits, *prev_clause),
                    _ => return,
                };
                // No catch clauses ran: the body's exit is the finally's
                // entry, latched here before the gap questions read it.
                let body_awaits = stored.unwrap_or(self.awaits);
                if stored.is_none()
                    && let Some(Frame::Try { body_awaits: b, .. }) = self.stack.last_mut()
                {
                    *b = Some(body_awaits);
                }
                let body_gap = body_awaits > entry;
                let catch_gap = prev_clause > body_awaits;
                if let Some(Frame::Try { finally_gap, .. }) = self.stack.last_mut() {
                    *finally_gap = body_gap || catch_gap;
                }
                for (slot, pre_s) in self.slots.iter_mut().zip(pre) {
                    *slot = if body_gap || catch_gap {
                        Slot::gap(self.gap, self.last_await)
                    } else {
                        pre_s
                    };
                }
            }
            Ev::TryEnd { has_finally } => {
                let Some(index) = self
                    .stack
                    .iter()
                    .rposition(|f| matches!(f, Frame::Try { .. }))
                else {
                    return;
                };
                let Frame::Try {
                    pre,
                    base,
                    entry,
                    body_awaits,
                    prev_clause,
                    finally_gap,
                } = self.stack.remove(index)
                else {
                    return;
                };
                self.markers.truncate(base);
                let body_gap = body_awaits.is_some_and(|b| b > entry);
                let catch_gap = prev_clause > body_awaits.unwrap_or(entry);
                for (slot, pre_s) in self.slots.iter_mut().zip(pre) {
                    *slot = if *has_finally && slot.st != St::Clear {
                        // The finally tail's own state survives it (the
                        // finally runs on every path; its tail carries).
                        *slot
                    } else if body_gap || catch_gap || finally_gap {
                        Slot::gap(self.gap, self.last_await)
                    } else {
                        pre_s
                    };
                }
            }

            // —— switch statement ——
            Ev::SwitchBegin { .. } => self.stack.push(Frame::Switch {
                is_expr: false,
                pre: self.slots,
                base: self.markers.len(),
                entry: self.awaits,
                block_entry: self.slots,
                member_start: self.awaits,
            }),
            Ev::SwitchBlockBegin => {
                self.pop_val();
                if let Some(Frame::Switch {
                    block_entry,
                    member_start,
                    ..
                }) = self.stack.last_mut()
                {
                    *block_entry = self.slots;
                    *member_start = self.awaits;
                }
            }
            Ev::CaseBegin { .. } => {
                let member_gap = self.member_gap(false);
                let block_entry = match self
                    .stack
                    .iter()
                    .rev()
                    .find(|f| matches!(f, Frame::Switch { is_expr: false, .. }))
                {
                    Some(Frame::Switch { block_entry, .. }) => *block_entry,
                    _ => return,
                };
                for (slot, block_s) in self.slots.iter_mut().zip(block_entry) {
                    *slot = if member_gap {
                        Slot::gap(self.gap, self.last_await)
                    } else {
                        block_s
                    };
                }
            }
            Ev::SwitchEnd => {
                let Some(index) = self
                    .stack
                    .iter()
                    .rposition(|f| matches!(f, Frame::Switch { is_expr: false, .. }))
                else {
                    return;
                };
                let Frame::Switch {
                    pre,
                    base,
                    entry,
                    member_start,
                    ..
                } = self.stack.remove(index)
                else {
                    return;
                };
                self.markers.truncate(base);
                let member_gap = self.awaits > member_start;
                let head_gap = member_start > entry;
                self.resolve_switch(index, member_gap);
                self.reindex(index);
                for (slot, pre_s) in self.slots.iter_mut().zip(pre) {
                    *slot = if member_gap || head_gap {
                        Slot::gap(self.gap, self.last_await)
                    } else {
                        pre_s
                    };
                }
            }

            // —— switch expression ——
            Ev::SwitchExprBegin { .. } => self.stack.push(Frame::Switch {
                is_expr: true,
                pre: self.slots,
                base: self.markers.len(),
                entry: self.awaits,
                block_entry: self.slots,
                member_start: self.awaits,
            }),
            Ev::SwitchExprBlockBegin => {
                self.pop_val();
                if let Some(Frame::Switch {
                    block_entry,
                    member_start,
                    ..
                }) = self.stack.last_mut()
                {
                    *block_entry = self.slots;
                    *member_start = self.awaits;
                }
            }
            Ev::SwitchExprCaseBegin => {
                let member_gap = self.member_gap(true);
                let block_entry = match self
                    .stack
                    .iter()
                    .rev()
                    .find(|f| matches!(f, Frame::Switch { is_expr: true, .. }))
                {
                    Some(Frame::Switch { block_entry, .. }) => *block_entry,
                    _ => return,
                };
                for (slot, block_s) in self.slots.iter_mut().zip(block_entry) {
                    *slot = if member_gap {
                        Slot::gap(self.gap, self.last_await)
                    } else {
                        block_s
                    };
                }
            }
            Ev::SwitchExprBlockEnd => {
                let Some(index) = self
                    .stack
                    .iter()
                    .rposition(|f| matches!(f, Frame::Switch { is_expr: true, .. }))
                else {
                    return;
                };
                let Frame::Switch {
                    pre,
                    base,
                    entry,
                    member_start,
                    ..
                } = self.stack.remove(index)
                else {
                    return;
                };
                self.markers.truncate(base);
                let member_gap = self.awaits > member_start;
                let head_gap = member_start > entry;
                self.reindex(index);
                for (slot, pre_s) in self.slots.iter_mut().zip(pre) {
                    *slot = if member_gap || head_gap {
                        Slot::gap(self.gap, self.last_await)
                    } else {
                        pre_s
                    };
                }
                // The expression nets its value for the consumer.
                self.markers.push(Mk::OTHER);
            }

            // —— assert ——
            Ev::AssertBegin { .. } => self.stack.push(Frame::Assert {
                base: self.markers.len(),
                saved: self.slots,
            }),
            // The condition/message boundary is transparent to the gap
            // walk — the message is just another expression to it.
            Ev::AssertAfterCondition => {}
            Ev::AssertEnd => {
                if let Some(Frame::Assert { base, saved }) = self.stack.pop() {
                    self.markers.truncate(base);
                    self.slots = saved;
                }
            }

            // —— patterns ——
            Ev::GuardEnd => {
                let guard = self.pop_val();
                if let Some(Frame::Switch { is_expr: true, .. }) = self.stack.last() {
                    // A switch-expression arm reads its when-clause: a gap
                    // in the guard puts the arm behind the crossing, a
                    // positive check guards it.
                    for (i, slot) in self.slots.iter_mut().enumerate() {
                        let (gap, gap_at) = (slot.gap, slot.gap_at);
                        *slot = match guard.guard[i] {
                            G::Gap => Slot::gap(self.gap, self.last_await),
                            G::Pos(s) if s == SIDES[i] => Slot {
                                st: St::Guarded,
                                unrel: false,
                                gap,
                                gap_at,
                            },
                            _ => *slot,
                        };
                    }
                }
            }

            // —— expressions ——
            Ev::Ident {
                name,
                token,
                context,
            } => match context {
                IdentifierContext::ExpressionContinuation => {
                    self.markers.push(Mk {
                        kind: Kind::Member(*name),
                        span: *token,
                        guard: NO_GUARD,
                        snap: self.slots,
                        net: false,
                    });
                }
                IdentifierContext::Expression => match self.resolve(*name) {
                    Some(kind) => self.push_val(kind, *token, NO_GUARD),
                    None if *name == self.mounted => {
                        // The bare Flutter `mounted` (implicit
                        // `this.mounted`): a State-side check whose guard
                        // value carries the positive polarity for the
                        // enclosing branch maps.
                        self.slots[1] = Slot {
                            st: St::Guarded,
                            unrel: false,
                            gap: self.slots[1].gap,
                            gap_at: self.slots[1].gap_at,
                        };
                        self.observe_check(Some(Side::State));
                        self.markers.push(Mk {
                            kind: Kind::Other,
                            span: *token,
                            guard: [G::None, G::Pos(Side::State)],
                            snap: self.slots,
                            net: false,
                        });
                    }
                    None if *name == self.context_name => {
                        // The bare unresolved `context`: the
                        // `State.context` getter shape.
                        self.push_val(Kind::StVal, *token, NO_GUARD);
                    }
                    None => self.push_val(Kind::Other, *token, NO_GUARD),
                },
                _ => {}
            },
            Ev::This { span } => self.push_val(Kind::This, *span, NO_GUARD),
            Ev::OtherExpr { span } => self.push_val(Kind::Other, *span, NO_GUARD),
            Ev::Bool { value } => {
                // A constant condition checks nothing: both polarities
                // carry side-tagged guards only so the branch maps apply.
                let g = if *value {
                    [G::Pos(Side::Context), G::Pos(Side::State)]
                } else {
                    [G::Neg(Side::Context), G::Neg(Side::State)]
                };
                self.markers.push(Mk {
                    kind: Kind::Bool(*value),
                    span: ZERO_SPAN,
                    guard: g,
                    snap: self.slots,
                    net: false,
                });
            }
            Ev::Null => self.markers.push(Mk::OTHER),
            Ev::UnaryPrefix { is_not } => {
                let mut m = self.pop_marker();
                if *is_not {
                    m.guard = [m.guard[0].flip(), m.guard[1].flip()];
                }
                self.markers.push(m);
            }
            Ev::UnaryPrefixAssign { .. } | Ev::UnaryPostfixAssign { .. } | Ev::PatternAssign => {
                self.pop_marker();
                self.markers.push(Mk::OTHER);
            }
            Ev::BinaryEnd { op, .. } => {
                let rhs = self.pop_val();
                let lhs = self.pop_val();
                let guard = match op {
                    BinOp::And => [
                        combine_and(lhs.guard[0], rhs.guard[0]),
                        combine_and(lhs.guard[1], rhs.guard[1]),
                    ],
                    BinOp::Or => [
                        combine_or(lhs.guard[0], rhs.guard[0]),
                        combine_or(lhs.guard[1], rhs.guard[1]),
                    ],
                    BinOp::Eq | BinOp::NotEq => {
                        // `c == b` / `c != b` bool folds: the effective
                        // polarity decides whether the guard or its flip
                        // carries.
                        let (l, r) = (lhs, rhs);
                        let mut g = NO_GUARD;
                        if let Kind::Bool(v) = r.kind {
                            for (g_i, l_i) in g.iter_mut().zip(l.guard) {
                                let eff = if *op == BinOp::Eq { v } else { !v };
                                *g_i = if eff { l_i } else { l_i.flip() };
                            }
                        } else if let Kind::Bool(v) = l.kind {
                            for (g_i, r_i) in g.iter_mut().zip(r.guard) {
                                let eff = if *op == BinOp::Eq { v } else { !v };
                                *g_i = if eff { r_i } else { r_i.flip() };
                            }
                        }
                        g
                    }
                    BinOp::IfNull | BinOp::Other => NO_GUARD,
                };
                self.markers.push(Mk {
                    kind: Kind::Other,
                    span: ZERO_SPAN,
                    guard,
                    snap: self.slots,
                    net: false,
                });
            }
            Ev::Assign { .. } => {
                let rhs = self.pop_val();
                self.pop_marker();
                self.markers.push(Mk {
                    kind: Kind::Other,
                    span: rhs.span,
                    guard: rhs.guard,
                    snap: self.slots,
                    net: false,
                });
            }
            Ev::CondBegin => {
                let cond = self.pop_val();
                let pre = self.slots;
                self.stack.push(Frame::Cond {
                    pre,
                    base: self.markers.len(),
                    cond: cond.guard,
                    after_then: None,
                });
                for i in 0..2 {
                    self.slots[i] =
                        self.branch_entry(SIDES[i], pre[i], cond.guard[i], false, true, None);
                }
            }
            Ev::CondColon { .. } => {
                self.pop_val();
                let (pre, c) = match self.stack.last() {
                    Some(Frame::Cond { pre, cond, .. }) => (*pre, *cond),
                    _ => return,
                };
                if let Some(Frame::Cond { after_then, .. }) = self.stack.last_mut() {
                    *after_then = Some(self.slots);
                }
                for i in 0..2 {
                    self.slots[i] = self.branch_entry(SIDES[i], pre[i], c[i], false, false, None);
                }
            }
            Ev::CondEnd { .. } => {
                self.pop_val();
                let Some(index) = self
                    .stack
                    .iter()
                    .rposition(|f| matches!(f, Frame::Cond { .. }))
                else {
                    return;
                };
                let Frame::Cond {
                    pre,
                    base,
                    cond,
                    after_then,
                } = self.stack.remove(index)
                else {
                    return;
                };
                self.markers.truncate(base);
                for i in 0..2 {
                    self.slots[i] = Self::merge_if(
                        pre[i],
                        cond[i],
                        false,
                        (self.gap, self.last_await),
                        after_then.map_or(self.slots[i], |t| t[i]),
                        Some(self.slots[i]),
                        false,
                        false,
                    );
                }
                self.markers.push(Mk::OTHER);
            }
            Ev::DotAccess { .. } => {
                let name = self.pop_marker();
                // An argument-bearing member leaf nets its arguments'
                // marker above the name marker (`x.y(..)`'s Send fires
                // after ArgumentsEnd, before this event): one net sits
                // between the access and its name, never more.
                let name = if matches!(name.kind, Kind::Member(_)) {
                    name
                } else {
                    self.pop_marker()
                };
                let target = self.pop_marker();
                let member = match name.kind {
                    Kind::Member(id) => id,
                    _ => u32::MAX,
                };
                if member == self.mounted {
                    // `x.mounted`: a check on the resolved receiver; a
                    // nullable context's check is not a guard (upstream
                    // skipNullable), an unresolved receiver checks both
                    // origins. The result marker carries the check's
                    // polarity for the enclosing branch maps.
                    let guard = match target.kind {
                        Kind::Ctx { nullable: false } => {
                            self.slots[0] = Slot {
                                st: St::Guarded,
                                unrel: false,
                                gap: target.snap[0].gap,
                                gap_at: target.snap[0].gap_at,
                            };
                            self.observe_check(Some(Side::Context));
                            [G::Pos(Side::Context), G::None]
                        }
                        Kind::Ctx { nullable: true } => NO_GUARD,
                        Kind::StVal | Kind::StRef | Kind::This => {
                            self.slots[1] = Slot {
                                st: St::Guarded,
                                unrel: false,
                                gap: target.snap[1].gap,
                                gap_at: target.snap[1].gap_at,
                            };
                            self.observe_check(Some(Side::State));
                            [G::None, G::Pos(Side::State)]
                        }
                        _ => {
                            let g0 = (target.snap[0].gap, target.snap[0].gap_at);
                            let g1 = (target.snap[1].gap, target.snap[1].gap_at);
                            self.slots = [
                                Slot {
                                    st: St::Guarded,
                                    unrel: false,
                                    gap: g0.0,
                                    gap_at: g0.1,
                                },
                                Slot {
                                    st: St::Guarded,
                                    unrel: false,
                                    gap: g1.0,
                                    gap_at: g1.1,
                                },
                            ];
                            self.observe_check(None);
                            [G::Pos(Side::Context), G::Pos(Side::State)]
                        }
                    };
                    self.markers.push(Mk {
                        kind: Kind::Other,
                        span: name.span,
                        guard,
                        snap: self.slots,
                        net: false,
                    });
                    return;
                }
                if member == self.context_name && matches!(target.kind, Kind::This | Kind::StRef) {
                    // `this.context` / `state.context`: a State-origin
                    // context value (getter shape).
                    self.markers.push(Mk {
                        kind: Kind::StVal,
                        span: name.span,
                        guard: NO_GUARD,
                        snap: self.slots,
                        net: false,
                    });
                    return;
                }
                // Any other member access reads its receiver with the
                // verdict latched at receiver evaluation.
                match target.kind {
                    Kind::Ctx { .. } => {
                        self.use_latched(target.span, AsyncOrigin::Context, target.snap[0]);
                    }
                    Kind::StVal => {
                        self.use_latched(target.span, AsyncOrigin::State, target.snap[1]);
                    }
                    _ => {}
                }
                self.markers.push(Mk::OTHER);
            }
            Ev::IndexAccess { .. } | Ev::MapEntryEnd => {
                self.pop_val();
                self.pop_val();
                self.markers.push(Mk::OTHER);
            }
            Ev::Send { .. } | Ev::SendNoArgs => {
                // The bare head's own send fires mid-parse — before the
                // selector's member name streams — so it must not consume
                // the value marker: the member access (`x.y`), the mounted
                // check (`x.mounted`), and the argument list all read the
                // receiver's kind after this event (`DotAccess` /
                // `ArgumentsEnd`). A call's leaf send nets the argument
                // result instead — folding the bare callee head beneath it
                // (member names stay for `DotAccess`), so every call
                // contributes exactly one marker and no head marker leaks
                // between a member access and its receiver. Inert bare
                // heads collapse to Other.
                let top = self.pop_marker();
                if top.net {
                    if !matches!(self.markers.last().map(|m| &m.kind), Some(Kind::Member(_))) {
                        self.pop_marker();
                    }
                    self.markers.push(Mk::OTHER);
                } else if matches!(top.kind, Kind::Other | Kind::Bool(_)) {
                    self.markers.push(Mk::OTHER);
                } else {
                    self.markers.push(top);
                }
            }
            Ev::ArgumentsEnd { count } => {
                let n = (*count).min(self.markers.len());
                let args: Vec<Mk> = self.markers.split_off(self.markers.len() - n);
                for arg in &args {
                    match arg.kind {
                        // Argument reads use the post-argument state
                        // (upstream scans the finished argument list).
                        Kind::Ctx { nullable: false } => {
                            self.use_at(arg.span, AsyncOrigin::Context);
                        }
                        Kind::StVal => self.use_at(arg.span, AsyncOrigin::State),
                        _ => {}
                    }
                }
                self.markers.push(Mk {
                    net: true,
                    ..Mk::OTHER
                });
            }
            Ev::Throw => {
                self.pop_val();
                self.markers.push(Mk::OTHER);
                self.terminate(true);
            }
            Ev::Rethrow | Ev::Exit { .. } => self.terminate(true),
            Ev::RecordLitEnd { count }
            | Ev::EndLiteralString {
                interpolation_count: count,
            }
            | Ev::ListLitEnd { count } => {
                let n = (*count).min(self.markers.len());
                for _ in 0..n {
                    self.pop_val();
                }
                self.markers.push(Mk::OTHER);
            }
            Ev::CascadeBegin { .. } => {
                self.stack.push(Frame::Cascade {
                    base: self.markers.len(),
                });
            }
            Ev::CascadeEnd => {
                self.pop_val();
                if let Some(Frame::Cascade { base }) = self.stack.pop() {
                    self.markers.truncate(base);
                }
                self.markers.push(Mk::OTHER);
            }
            Ev::Spread | Ev::NullAwareElement | Ev::CascadeAccess { .. } => {
                self.pop_val();
                self.markers.push(Mk::OTHER);
            }
            Ev::IfControlFlowBegin => self.stack.push(Frame::CollIf {
                pre: self.slots,
                base: self.markers.len(),
                cond: NO_GUARD,
                after_then: None,
            }),
            Ev::ThenControlFlow { .. } => {
                let c = self.pop_val().guard;
                let pre = match self.stack.last() {
                    Some(Frame::CollIf { pre, .. }) => *pre,
                    _ => return,
                };
                if let Some(Frame::CollIf { cond, .. }) = self.stack.last_mut() {
                    *cond = c;
                }
                for i in 0..2 {
                    self.slots[i] = self.branch_entry(SIDES[i], pre[i], c[i], false, true, None);
                }
            }
            Ev::ElseControlFlow { .. } => {
                self.pop_val();
                let (pre, c) = match self.stack.last() {
                    Some(Frame::CollIf { pre, cond, .. }) => (*pre, *cond),
                    _ => return,
                };
                if let Some(Frame::CollIf { after_then, .. }) = self.stack.last_mut() {
                    *after_then = Some(self.slots);
                }
                for i in 0..2 {
                    self.slots[i] = self.branch_entry(SIDES[i], pre[i], c[i], false, false, None);
                }
            }
            Ev::EndIfControlFlow { .. } => {
                self.pop_val();
                let Some(index) = self
                    .stack
                    .iter()
                    .rposition(|f| matches!(f, Frame::CollIf { .. }))
                else {
                    return;
                };
                let Frame::CollIf {
                    pre,
                    base,
                    cond,
                    after_then,
                } = self.stack.remove(index)
                else {
                    return;
                };
                self.markers.truncate(base);
                for i in 0..2 {
                    self.slots[i] = Self::merge_if(
                        pre[i],
                        cond[i],
                        false,
                        (self.gap, self.last_await),
                        after_then.map_or(self.slots[i], |t| t[i]),
                        Some(self.slots[i]),
                        false,
                        false,
                    );
                }
                self.markers.push(Mk::OTHER);
            }
            Ev::ForControlFlowBegin => self.stack.push(Frame::CollFor {
                pre: self.slots,
                base: self.markers.len(),
                entry: self.awaits,
            }),
            Ev::ForControlFlowEnd { .. } => {
                self.pop_val();
                let Some(Frame::CollFor { pre, base, entry }) = self.stack.pop() else {
                    return;
                };
                self.markers.truncate(base);
                for (slot, pre_s) in self.slots.iter_mut().zip(pre) {
                    *slot = if self.awaits > entry {
                        Slot::gap(self.gap, self.last_await)
                    } else {
                        pre_s
                    };
                }
                self.markers.push(Mk::OTHER);
            }
            // The `?[`/`?.` operand-begin markers carry no value and mutate
            // no marker: this pass tracks await/gap segments, not
            // reachability.
            Ev::NullAwareIndexOperandBegin | Ev::NullAwareMemberBegin => {}
        }
    }
}
