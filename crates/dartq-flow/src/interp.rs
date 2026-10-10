//! The replay interpreter: processes a recorded event buffer through the
//! flow state machine, in the official decision order.
//!
//! Two phases run per executable: [`Interp::run`] first executes the
//! whole body once in collect mode (the official `AssignedVariables`
//! pre-pass) and then replays for real. Loop heads and closure entries
//! re-collect their own extents on demand (`flow_analysis.dart`
//! `whileStatement_conditionBegin`/`_functionExpression_begin` consult
//! per-node effect sets that only a pre-pass can know).

use std::ops::Range;

use dartq_parser::TokenSpan;
use dartq_source::SourceRange;
use rustc_hash::{FxHashMap, FxHashSet};

use crate::ev::Ev;
use crate::facts::FlowFact;
use crate::model::{Lists, NIL, ReachArena, State};
use crate::ty::{Ty, TypeOps};

/// Metadata of one local variable (or parameter).
pub(crate) struct VarMeta {
    /// Declared type (name-level).
    pub ty: Ty,
    pub late: bool,
    pub is_final: bool,
    /// Span of the declaration's name token, when declared locally.
    pub decl_span: Option<SourceRange>,
}

/// One lexical scope frame: name id to variable id.
pub(crate) struct Scope {
    bindings: FxHashMap<u32, u32>,
}

impl Scope {
    pub(crate) fn new() -> Self {
        Self {
            bindings: FxHashMap::default(),
        }
    }
}

/// Effect sets of one construct extent (official `AssignedVariablesNodeInfo`).
#[derive(Default, Debug)]
pub(crate) struct Effects {
    pub declared: FxHashSet<u32>,
    pub written: FxHashSet<u32>,
    pub read: FxHashSet<u32>,
    /// Reads inside nested closures (official `readCaptured`).
    pub read_captured: FxHashSet<u32>,
    /// Writes inside nested closures (official `captured`).
    pub captured: FxHashSet<u32>,
}

/// What a step tells the driver.
pub(crate) enum Control {
    Continue,
    /// Jump the replay cursor (construct extents consumed wholesale).
    Jump(usize),
}

/// Kind of the active top-level pattern context.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PatternKind {
    /// A switch statement alternative: finalize into the switch frame.
    Switch,
    /// A switch expression alternative.
    SwitchExpr,
    /// `if (e case p)`: finalize at `then` begin.
    IfCase,
    /// A pattern variable declaration statement: finalize at the
    /// statement end.
    Decl,
}

/// The active top-level pattern context (matched/unmatched states).
pub(crate) struct PatternState {
    pub kind: PatternKind,
    /// State if the pattern did not match (official `_unmatched`).
    pub unmatched: State,
    /// The scrutinee/matched value when it is a simple variable read.
    pub scrutinee: Option<u32>,
    /// The pattern's `when`-guard expression, if one fired.
    pub guard: Option<crate::interp_expr::Expr>,
    /// Pattern-declared variables awaiting scope flush (name id, final).
    pub pending_decls: Vec<(u32, bool)>,
}

/// Replay frames (official `_FlowContext` subclasses).
pub(crate) enum Frame {
    Exec {
        is_closure: bool,
        scopes_base: usize,
        /// The state to restore at the executable's end: for closures the
        /// capture-conservative pre-state (official
        /// `_FunctionExpressionContext._previous`).
        saved: State,
        /// Reads and declares of the closure body (for `suspension`
        /// demotion); empty for non-closure executables.
        reads: FxHashSet<u32>,
        declared: FxHashSet<u32>,
    },
    Block {
        scopes_base: usize,
    },
    If {
        branch: State,
        after_then: Option<State>,
    },
    Cond {
        branch: State,
        then_state: Option<State>,
    },
    Branch {
        branch: State,
    },
    Cascade {
        exprs_base: usize,
    },
    /// A loop: break/continue target (`collection` selects a
    /// collection-`for` element).
    While {
        collection: bool,
        checkpoint: u32,
        cond_false: Option<State>,
        break_model: Option<State>,
        continue_model: Option<State>,
    },
    TryCatch {
        /// The pre-split state (the combined try-finally's `beforeTry`).
        pre: State,
        previous: State,
        before_catch: Option<State>,
        after: Option<State>,
        wrote: FxHashSet<u32>,
        captured: FxHashSet<u32>,
    },
    TryFinally {
        after_try: Option<State>,
        before_finally: Option<State>,
    },
    Assert {
        previous: State,
        exprs_base: usize,
    },
    /// A switch statement or switch expression (the case-alternatives
    /// machinery).
    Switch {
        is_expr: bool,
        checkpoint: u32,
        previous: State,
        unmatched: State,
        break_model: Option<State>,
        /// A `CaseExprEnd` was seen: the group awaits its first body event
        /// (one-event lookahead closes the alternatives there).
        case_open: bool,
        group_labels: bool,
        group_default: bool,
        group_combined: Option<State>,
        /// Set once the group closed and its body started.
        body_depth: Option<usize>,
        block: Range<usize>,
    },
    Labeled {
        checkpoint: u32,
        break_model: Option<State>,
    },
}

/// Index bounds of a C-style `for` header's three regions plus its body.
/// Events are recorded in source order, so each region is a contiguous
/// index range; the separators anchor the boundaries and unanchored
/// events (grouping markers) fall out by position.
pub(crate) struct ForBounds {
    pub init: Range<usize>,
    pub cond: Range<usize>,
    pub upd: Range<usize>,
    pub body: Range<usize>,
}

impl ForBounds {
    /// Partitions the header. The buffer records all three header parts
    /// BEFORE the `ForParts` marker (initializers, condition, updaters —
    /// source-ordered), so the two separators' offsets slice them; the
    /// body is everything between `ForParts` and `ForEnd`.
    pub(crate) fn build(
        events: &[Ev],
        begin: usize,
        parts: usize,
        left: u32,
        right: u32,
        end: usize,
    ) -> Self {
        let mut cond_start = parts;
        let mut upd_start = parts;
        for j in begin + 1..parts {
            if let Some(span) = events[j].span() {
                if span.range.offset >= right {
                    upd_start = upd_start.min(j);
                } else if span.range.offset >= left {
                    cond_start = cond_start.min(j);
                }
            }
        }
        Self {
            init: begin + 1..cond_start,
            cond: cond_start..upd_start,
            upd: upd_start..parts,
            body: parts + 1..end,
        }
    }
}

/// The current variable-declaration context.
pub(crate) struct DeclCtx {
    pub late: bool,
    pub is_final: bool,
    pub ty: Ty,
}

impl Default for DeclCtx {
    fn default() -> Self {
        Self {
            late: false,
            is_final: false,
            ty: Ty::UNKNOWN,
        }
    }
}

/// One expression value on the expression stack.
pub(crate) struct Expr {
    /// Type of the expression (name-level).
    pub ty: Ty,
    /// The expression reads this variable (promotable reference).
    pub reference: Option<u32>,
    /// Boolean literal truth.
    pub bool_value: Option<bool>,
    /// Non-trivial branch states (`ifTrue`/`ifFalse`); `None` = trivial.
    pub branches: Option<(State, State)>,
    /// The token of a bare variable read, when this expression is exactly
    /// one: a plain assignment target retracts the read's fact (the
    /// identifier is the write, not a read).
    pub read_span: Option<TokenSpan>,
}

impl Expr {
    pub(crate) fn trivial(ty: Ty) -> Self {
        Self {
            ty,
            reference: None,
            bool_value: None,
            branches: None,
            read_span: None,
        }
    }

    pub(crate) fn of_reference(var: u32, ty: Ty) -> Self {
        Self {
            ty,
            reference: Some(var),
            bool_value: None,
            branches: None,
            read_span: None,
        }
    }
}

/// The interpreter over one executable's event buffer.
pub(crate) struct Interp<'o> {
    pub(crate) ops: &'o dyn TypeOps,
    pub(crate) reach: ReachArena,
    pub(crate) lists: Lists,
    pub(crate) current: State,
    pub(crate) stack: Vec<Frame>,
    pub(crate) exprs: Vec<Expr>,
    pub(crate) scopes: Vec<Scope>,
    pub(crate) vars: Vec<VarMeta>,
    pub(crate) facts: Vec<FlowFact>,

    pub(crate) decl: DeclCtx,
    pub(crate) pending_ty: Option<Ty>,
    /// Pattern-assignment writes awaiting their right-hand side.
    pub(crate) pending_writes: Vec<u32>,
    /// Locals declared since the innermost variable declaration began.
    /// `pending_decl_bases` holds each open declaration's base index into
    /// this list, so nested declarations pop their own inits without
    /// touching the enclosing declaration's.
    pub(crate) pending_inits: Vec<u32>,
    /// Open declarations' base indices into `pending_inits` (one per
    /// `VarDeclBegin`, paired at `VarDeclEnd`).
    pub(crate) pending_decl_bases: Vec<usize>,
    pub(crate) pattern: Option<PatternState>,
    pub(crate) pattern_depth: usize,
    /// Context for the next top-level pattern (case expressions and
    /// switch-expression cases pre-announce; everything else defaults to
    /// a declaration context).
    pub(crate) pattern_kind_hint: PatternKind,
    pub(crate) type_test_pending: Option<(bool, Ty)>,
    pub(crate) as_cast_pending: Option<Ty>,
    /// The type currently being parsed (test/cast/formal consumers).
    pub(crate) tested_ty: Option<Ty>,
    pub(crate) in_decl: bool,
    pub(crate) in_type_test: bool,
    pub(crate) in_as_cast: bool,
    pub(crate) for_header_depth: usize,
    pub(crate) anywhere: Effects,

    pub(crate) collect: Option<Vec<Effects>>,
    snapshot: Option<Snapshot>,
}

/// Interpreter state frozen around a collect run.
struct Snapshot {
    current: State,
    exprs_len: usize,
    frames_len: usize,
    scopes_len: usize,
    vars_len: usize,
    facts_len: usize,
}

impl<'o> Interp<'o> {
    pub(crate) fn new(ops: &'o dyn TypeOps) -> Self {
        Self {
            ops,
            reach: ReachArena::new(),
            lists: Lists::default(),
            current: State::new(0),
            stack: Vec::new(),
            exprs: Vec::new(),
            scopes: vec![Scope {
                bindings: FxHashMap::default(),
            }],
            vars: Vec::new(),
            facts: Vec::new(),
            decl: DeclCtx::default(),
            pending_ty: None,
            pending_writes: Vec::new(),
            pending_inits: Vec::new(),
            pending_decl_bases: Vec::new(),
            pattern: None,
            pattern_depth: 0,
            pattern_kind_hint: PatternKind::Decl,
            type_test_pending: None,
            as_cast_pending: None,
            tested_ty: None,
            in_decl: false,
            in_type_test: false,
            in_as_cast: false,
            for_header_depth: 0,
            anywhere: Effects::default(),
            collect: None,
            snapshot: None,
        }
    }

    /// Runs one executable's buffer: the pre-pass collect, then the real
    /// replay.
    pub(crate) fn run(&mut self, events: Vec<Ev>) {
        let match_of = build_match_map(&events);
        self.anywhere = self.collect_ranges(&events, &[0..events.len()]);
        let mut i = 0;
        while i < events.len() {
            let control = self.step(&events, i, &match_of);
            match control {
                Control::Continue => i += 1,
                Control::Jump(to) => i = to,
            }
        }
    }

    // —— variables ——————————————————————————————————————————————————————

    /// Declares a variable, records the effect, binds the name, and
    /// initializes the flow model. `initialized` mirrors the official
    /// `declare(initialized:)`: locals with an initializer (or `late`
    /// locals, or parameters) start assigned; plain locals without one
    /// start unassigned.
    pub(crate) fn declare_var(&mut self, name: u32, meta: VarMeta, initialized: bool) -> u32 {
        let var = self.vars.len() as u32;
        self.vars.push(meta);
        if let Some(frames) = self.collect.as_mut() {
            for frame in frames.iter_mut() {
                frame.declared.insert(var);
            }
        }
        self.current = self.current.clone().declare(var, initialized);
        self.bind_name(name, var);
        var
    }

    pub(crate) fn bind_name(&mut self, name_id: u32, var: u32) {
        if let Some(scope) = self.scopes.last_mut() {
            scope.bindings.insert(name_id, var);
        }
    }

    pub(crate) fn resolve(&self, name_id: u32) -> Option<u32> {
        for scope in self.scopes.iter().rev() {
            if let Some(&var) = scope.bindings.get(&name_id) {
                return Some(var);
            }
        }
        None
    }

    /// Records a write (real or collect mode) and updates the flow state.
    /// `promote_to_interest` is false for compound and unknown-type
    /// writes.
    pub(crate) fn write_var(&mut self, var: u32, written: Ty, promote_to_interest: bool) {
        self.record_write(var);
        let declared = self.vars[var as usize].ty;
        self.current = self.current.clone().write(
            self.ops,
            &mut self.lists,
            var,
            declared,
            written,
            promote_to_interest,
        );
    }

    // —— effect recording (collect mode) —————————————————————————————————

    pub(crate) fn record_write(&mut self, var: u32) {
        if let Some(frames) = self.collect.as_mut() {
            for frame in frames.iter_mut() {
                frame.written.insert(var);
            }
        }
    }

    pub(crate) fn record_read(&mut self, var: u32) {
        if let Some(frames) = self.collect.as_mut() {
            for frame in frames.iter_mut() {
                frame.read.insert(var);
            }
        }
    }

    // —— state helpers ——————————————————————————————————————————————————

    pub(crate) fn reachable(&self) -> bool {
        self.reach.locally_reachable(self.current.reach)
    }

    /// Splits the current state on a condition expression (official
    /// `FlowModel` branch machinery): explicit branch states win, then
    /// boolean literals, otherwise both branches inherit the state.
    pub(crate) fn cond_split(&mut self, expr: &Expr) -> (State, State) {
        if let Some((if_true, if_false)) = &expr.branches {
            return (if_true.clone(), if_false.clone());
        }
        match expr.bool_value {
            Some(true) => (
                self.current.clone(),
                self.current.clone().set_unreachable(&mut self.reach),
            ),
            Some(false) => (
                self.current.clone().set_unreachable(&mut self.reach),
                self.current.clone(),
            ),
            None => (self.current.clone(), self.current.clone()),
        }
    }

    pub(crate) fn join(&mut self, first: Option<State>, second: Option<State>) -> State {
        State::join(
            first,
            second,
            &mut self.reach,
            &mut self.lists,
            self.vars.len(),
        )
    }

    /// Splits the current state in place (official `FlowModel.split`).
    pub(crate) fn split_cur(&mut self) {
        self.current = self.current.clone().split(&mut self.reach);
    }

    /// Marks the current state unreachable in place.
    pub(crate) fn unreachable_cur(&mut self) {
        self.current = self.current.clone().set_unreachable(&mut self.reach);
    }

    /// Whether a collect run (effect pre-pass) is in progress.
    pub(crate) fn collecting(&self) -> bool {
        self.collect.is_some()
    }

    /// Emits a fact unless a collect run is in progress.
    pub(crate) fn emit(&mut self, fact: FlowFact) {
        if self.collect.is_none() {
            self.facts.push(fact);
        }
    }

    pub(crate) fn dead_code(&mut self, span: TokenSpan) {
        if !self.reachable() {
            self.emit(FlowFact::DeadCode { span });
        }
    }

    /// Reads of definitely-unassigned locals (official `isUnassigned`).
    pub(crate) fn check_unassigned_read(&mut self, span: TokenSpan, var: u32) {
        if self.reachable() && !self.current.model(var).assigned {
            let meta = &self.vars[var as usize];
            self.emit(FlowFact::UnassignedRead {
                span,
                is_late: meta.late,
                is_final: meta.is_final,
                declared_type: meta.decl_span,
            });
        }
    }

    // —— collect runs ——————————————————————————————————————————————————

    /// Runs the event ranges once in collect mode and returns the bottom
    /// effect frame. All flow-state mutations are rolled back and fact
    /// emission is suppressed.
    pub(crate) fn collect_ranges(&mut self, events: &[Ev], ranges: &[Range<usize>]) -> Effects {
        let snapshot = Snapshot {
            current: self.current.clone(),
            exprs_len: self.exprs.len(),
            frames_len: self.stack.len(),
            scopes_len: self.scopes.len(),
            vars_len: self.vars.len(),
            facts_len: self.facts.len(),
        };
        let outer = self.collect.replace(vec![Effects::default()]);
        let _ = self.snapshot.replace(snapshot);
        for range in ranges {
            let mut i = range.start;
            while i < range.end {
                match self.step(events, i, &MATCH_NONE) {
                    Control::Continue => i += 1,
                    // Extent handlers consume wholesale in collect runs too.
                    Control::Jump(to) => i = to.max(range.start),
                }
            }
        }
        let frames = self.collect.take().unwrap_or_default();
        // Roll back interpreter state.
        let s = self.snapshot.take().expect("collect snapshot");
        self.exprs.truncate(s.exprs_len);
        self.stack.truncate(s.frames_len);
        self.scopes.truncate(s.scopes_len);
        self.vars.truncate(s.vars_len);
        self.facts.truncate(s.facts_len);
        self.current = s.current;
        self.collect = outer;
        frames.into_iter().next().unwrap_or_default()
    }

    /// Enters a construct's effect frame inside a collect run.
    pub(crate) fn collect_push(&mut self) {
        if let Some(frames) = self.collect.as_mut() {
            frames.push(Effects::default());
        }
    }

    /// Pops the innermost collect effect frame, propagating per the
    /// closure/late-initializer rules when `is_closure` (official
    /// `AssignedVariables.deferNode`).
    pub(crate) fn collect_pop(&mut self, is_closure: bool) {
        let Some(frames) = self.collect.as_mut() else {
            return;
        };
        if frames.len() <= 1 {
            return;
        }
        let mut inner = frames.pop().expect("inner frame");
        if is_closure {
            // Declared-in-closure variables drop out of the closure's own
            // sets and out of every enclosing capture set.
            let declared: Vec<u32> = inner.declared.iter().copied().collect();
            for var in &declared {
                inner.written.remove(var);
                inner.read.remove(var);
                for frame in frames.iter_mut() {
                    frame.captured.remove(var);
                    frame.read_captured.remove(var);
                }
            }
            let written: Vec<u32> = inner.written.iter().copied().collect();
            let read: Vec<u32> = inner.read.iter().copied().collect();
            for frame in frames.iter_mut() {
                frame.captured.extend(written.iter().copied());
                frame.read_captured.extend(read.iter().copied());
            }
        } else if let Some(parent) = frames.last_mut() {
            parent.written.extend(inner.written.iter().copied());
            parent.read.extend(inner.read.iter().copied());
            parent.captured.extend(inner.captured.iter().copied());
            parent
                .read_captured
                .extend(inner.read_captured.iter().copied());
        }
    }
}

impl Default for State {
    fn default() -> Self {
        Self::new(0)
    }
}

impl<'o> Interp<'o> {
    // —— conservative-join plumbing —————————————————————————————————————

    /// The final variables among `written` (promotions on finals survive
    /// conservative joins).
    pub(crate) fn finals_of(&self, written: &FxHashSet<u32>) -> FxHashSet<u32> {
        written
            .iter()
            .copied()
            .filter(|&var| self.vars[var as usize].is_final)
            .collect()
    }

    /// `current.conservativeJoin(written, captured)` with finality applied.
    pub(crate) fn cjoin(&mut self, written: &FxHashSet<u32>, captured: &FxHashSet<u32>) {
        // Effect sets recorded by a collect run may reference variables
        // declared inside the collected range: those ids are gone after the
        // state rollback (they are re-declared per iteration) and must not
        // reach the join.
        let live = |v: &u32| (*v as usize) < self.vars.len();
        let written: FxHashSet<u32> = written.iter().copied().filter(live).collect();
        let captured: FxHashSet<u32> = captured.iter().copied().filter(live).collect();
        let finals = self.finals_of(&written);
        let current = std::mem::take(&mut self.current);
        self.current = current.conservative_join(&written, &captured, &finals);
    }

    /// The official `suspension` (`flow_analysis.dart:7909`): every
    /// variable read in the current local function and written anywhere
    /// loses its promotions, except the function's own locals. The demotion
    /// applies only when the suspension point sits inside a local function
    /// (official `_enclosingFunctionExpressionInfoStack` non-empty): an
    /// await/yield directly in an async/sync* method body cannot let an
    /// enclosing function resume, so nothing demotes there.
    pub(crate) fn suspend(&mut self) {
        let demote: FxHashSet<u32> = match self.stack.iter().rev().find(|f| {
            matches!(
                f,
                Frame::Exec {
                    is_closure: true,
                    ..
                }
            )
        }) {
            Some(Frame::Exec {
                reads, declared, ..
            }) => reads
                .intersection(&self.anywhere.written)
                .copied()
                .filter(|var| !declared.contains(var))
                .collect(),
            _ => return,
        };
        self.cjoin(&demote, &FxHashSet::default());
    }

    /// Runs a range of the buffer through the driver, honoring jumps.
    pub(crate) fn run_range(&mut self, events: &[Ev], range: Range<usize>, match_of: &[u32]) {
        let mut i = range.start;
        while i < range.end {
            match self.step(events, i, match_of) {
                Control::Continue => i += 1,
                Control::Jump(to) => i = to.max(range.start),
            }
        }
    }

    // —— jump targets ————————————————————————————————————————————————————

    /// Finds the innermost break target frame (top-down): the nearest loop
    /// or switch statement; labeled statements only for labeled breaks.
    pub(crate) fn find_break_target(&self, labeled: bool) -> Option<usize> {
        for (index, frame) in self.stack.iter().enumerate().rev() {
            match frame {
                Frame::While { .. } => return Some(index),
                Frame::Switch { is_expr, .. } if !*is_expr => return Some(index),
                Frame::Labeled { .. } if labeled => return Some(index),
                _ => {}
            }
        }
        None
    }

    /// Finds the innermost continue target: the nearest loop; labeled
    /// statements only for labeled continues.
    pub(crate) fn find_continue_target(&self, labeled: bool) -> Option<usize> {
        for (index, frame) in self.stack.iter().enumerate().rev() {
            match frame {
                Frame::While { .. } => return Some(index),
                Frame::Labeled { .. } if labeled => return Some(index),
                _ => {}
            }
        }
        None
    }

    /// Joins the current state into a target's break model and marks the
    /// current path unreachable (official `handleBreak`).
    pub(crate) fn do_break(&mut self, index: usize) {
        let checkpoint = match &self.stack[index] {
            Frame::While { checkpoint, .. }
            | Frame::Switch { checkpoint, .. }
            | Frame::Labeled { checkpoint, .. } => *checkpoint,
            _ => return,
        };
        let joined = self.current.clone().unsplit_to(&mut self.reach, checkpoint);
        let prior = match &mut self.stack[index] {
            Frame::While { break_model, .. }
            | Frame::Switch { break_model, .. }
            | Frame::Labeled { break_model, .. } => break_model.take(),
            _ => None,
        };
        let joined = self.join(prior, Some(joined));
        if let Frame::While { break_model, .. }
        | Frame::Switch { break_model, .. }
        | Frame::Labeled { break_model, .. } = &mut self.stack[index]
        {
            *break_model = Some(joined);
        }
        self.unreachable_cur();
    }

    /// Joins the current state into a target's continue model (official
    /// `handleContinue`).
    pub(crate) fn do_continue(&mut self, index: usize) {
        let checkpoint = match &self.stack[index] {
            Frame::While { checkpoint, .. } | Frame::Labeled { checkpoint, .. } => *checkpoint,
            _ => return,
        };
        let joined = self.current.clone().unsplit_to(&mut self.reach, checkpoint);
        let prior = match &mut self.stack[index] {
            Frame::While { continue_model, .. } => continue_model.take(),
            Frame::Labeled { .. } => {
                // A labeled continue targets the labeled LOOP: routed to
                // the loop frame by `find_continue_target` in practice.
                None
            }
            _ => None,
        };
        let joined = self.join(prior, Some(joined));
        if let Frame::While { continue_model, .. } = &mut self.stack[index] {
            *continue_model = Some(joined);
        }
        self.unreachable_cur();
    }

    // —— executable framing ————————————————————————————————————————————

    fn exec_begin(&mut self, is_closure: bool, events: &[Ev], i: usize, match_of: &[u32]) {
        self.scopes.push(Scope {
            bindings: FxHashMap::default(),
        });
        let scopes_base = self.scopes.len() - 1;
        if self.collect.is_some() {
            self.collect_push();
            self.stack.push(Frame::Exec {
                is_closure,
                scopes_base,
                saved: self.current.clone(),
                reads: FxHashSet::default(),
                declared: FxHashSet::default(),
            });
            return;
        }
        if !is_closure {
            self.stack.push(Frame::Exec {
                is_closure: false,
                scopes_base,
                saved: self.current.clone(),
                reads: FxHashSet::default(),
                declared: FxHashSet::default(),
            });
            return;
        }
        // Closure entry (official `_functionExpression_begin`): the
        // closure's own writes demote as captures; everything written
        // anywhere demotes as a possible pre-write.
        // An unmatched begin (parser recovery) has no extent to collect:
        // enter with no captured-write knowledge.
        let end = match_of[i];
        if end == NIL {
            self.stack.push(Frame::Exec {
                is_closure: true,
                scopes_base,
                saved: self.current.clone(),
                reads: FxHashSet::default(),
                declared: FxHashSet::default(),
            });
            return;
        }
        let end = end as usize;
        let effects = self.collect_ranges(events, &[i + 1..end]);
        self.cjoin(&FxHashSet::default(), &effects.written);
        let saved = self.current.clone();
        let anywhere_written = self.anywhere.written.clone();
        let anywhere_captured = self.anywhere.captured.clone();
        self.cjoin(&anywhere_written, &anywhere_captured);
        self.stack.push(Frame::Exec {
            is_closure: true,
            scopes_base,
            saved,
            reads: effects.read.clone(),
            declared: effects.declared.clone(),
        });
    }

    fn exec_end(&mut self) {
        let collecting = self.collect.is_some();
        let frame = self.stack.pop().expect("exec frame");
        let Frame::Exec {
            is_closure,
            scopes_base,
            saved,
            ..
        } = frame
        else {
            self.stack.push(frame);
            return;
        };
        self.scopes.truncate(scopes_base);
        if collecting {
            self.collect_pop(is_closure);
            return;
        }
        if is_closure {
            self.current = saved;
        }
    }

    /// Ends the innermost matching loop frame: joins its exit states
    /// (official `whileStatement_end`/`forEach_end`).
    pub(crate) fn loop_exit(&mut self, collection: bool) {
        // A collect run pushes no loop frames of its own: a loop-end event
        // under collect must not touch the stack, or it removes a REAL
        // frame of the enclosing replay — and the rollback (tail truncate)
        // cannot restore a mid-stack removal.
        if self.collecting() {
            return;
        }
        let Some(index) = self
            .stack
            .iter()
            .rposition(|f| matches!(f, Frame::While { collection: c, .. } if *c == collection))
        else {
            return;
        };
        let frame = self.stack.remove(index);
        let Frame::While {
            cond_false,
            break_model,
            ..
        } = frame
        else {
            return;
        };
        let exit = self.join(cond_false.clone(), break_model);
        self.current = exit
            .unsplit(&mut self.reach)
            .inherit_tested(&self.current, &mut self.lists);
    }

    // —— loops ——————————————————————————————————————————————————————————

    /// C-style `for`: consumes the whole extent, replaying the header in
    /// the official order (initializers, condition, body, updaters —
    /// `handle_for_loop_parts` delivered the updaters out of source
    /// order). The effect collect covers the whole construct; the
    /// initializers' writes demote again through plain assignment before
    /// the condition runs, so the overlap is redundant, not unsound.
    fn for_begin(&mut self, span: TokenSpan, events: &[Ev], i: usize, match_of: &[u32]) -> Control {
        self.dead_code(span);
        if self.collect.is_some() {
            return Control::Continue;
        }
        // An unmatched begin (parser recovery) has no extent to replay:
        // fall through to a linear walk.
        let end = match_of[i];
        if end == NIL {
            return Control::Continue;
        }
        let end = end as usize;
        let Some(parts) = (i + 1..end).find(|&j| matches!(events[j], Ev::ForParts { .. })) else {
            return Control::Continue;
        };
        let Ev::ForParts {
            left_sep,
            right_sep,
        } = events[parts]
        else {
            return Control::Jump(end + 1);
        };
        let bounds = ForBounds::build(
            events,
            i,
            parts,
            left_sep.range.offset,
            right_sep.range.offset,
            end,
        );
        self.replay_c_for(bounds, events, match_of, false);
        Control::Jump(end + 1)
    }

    /// The shared C-style `for` body: head split + join, initializers,
    /// condition, body, back-edge continue join, updaters, exit join.
    /// With `collection`, the frame stays until `ForControlFlowEnd`.
    fn replay_c_for(
        &mut self,
        bounds: ForBounds,
        events: &[Ev],
        match_of: &[u32],
        collection: bool,
    ) {
        let base = self.exprs.len();
        let effects = self.collect_ranges(events, &[bounds.init.start..bounds.body.end]);
        self.split_cur();
        self.cjoin(&effects.written, &effects.captured);
        let checkpoint = self.reach.parent(self.current.reach);
        self.stack.push(Frame::While {
            collection,
            checkpoint,
            cond_false: None,
            break_model: None,
            continue_model: None,
        });
        let frame = self.stack.len() - 1;
        self.for_header_depth += 1;
        self.run_range(events, bounds.init.clone(), match_of);
        self.for_header_depth = self.for_header_depth.saturating_sub(1);
        self.exprs.truncate(base);
        // Condition (empty condition = `true`).
        let cond = self.run_header_expr(events, bounds.cond.clone(), match_of, base);
        let (if_true, if_false) = match cond {
            Some(expr) => self.cond_split(&expr),
            None => {
                let cur = self.current.clone();
                (cur.clone(), cur.set_unreachable(&mut self.reach))
            }
        };
        if let Some(Frame::While { cond_false, .. }) = self.stack.get_mut(frame) {
            *cond_false = Some(if_false);
        }
        self.current = if_true;
        // Body.
        self.run_range(events, bounds.body.clone(), match_of);
        // Updaters run on the back-edge state: join pending continues
        // first (official `forStatement_updaterBegin`).
        let continue_model = match &self.stack[frame] {
            Frame::While { continue_model, .. } => continue_model.clone(),
            _ => None,
        };
        self.current = self.join(Some(self.current.clone()), continue_model);
        self.for_header_depth += 1;
        self.run_range(events, bounds.upd.clone(), match_of);
        self.for_header_depth = self.for_header_depth.saturating_sub(1);
        self.exprs.truncate(base);
        if !collection {
            // Exit: condition-false joined with breaks (no back-edge join).
            self.pop_loop_and_exit(frame);
        }
    }

    fn pop_loop_and_exit(&mut self, frame: usize) {
        let frame_state = match self.stack.get(frame) {
            Some(Frame::While {
                cond_false,
                break_model,
                ..
            }) => (cond_false.clone(), break_model.clone()),
            _ => (None, None),
        };
        self.stack.truncate(frame);
        let (cond_false, break_model) = frame_state;
        let exit = self.join(cond_false, break_model);
        self.current = exit
            .unsplit(&mut self.reach)
            .inherit_tested(&self.current, &mut self.lists);
    }

    /// Runs a header expression slice and returns its value (the slice
    /// nets exactly one expression when non-empty).
    fn run_header_expr(
        &mut self,
        events: &[Ev],
        range: Range<usize>,
        match_of: &[u32],
        base: usize,
    ) -> Option<Expr> {
        self.for_header_depth += 1;
        self.run_range(events, range, match_of);
        self.for_header_depth = self.for_header_depth.saturating_sub(1);
        let expr = if self.exprs.len() > base {
            self.exprs.pop()
        } else {
            None
        };
        self.exprs.truncate(base);
        expr
    }

    /// Collection `for` (`[for (var i = 0; i < 3; i++) e]`): consumes the
    /// whole extent. The literal's own end event nets one value for the
    /// enclosing consumer and each element nets exactly one, so the
    /// element's own value is truncated and replaced by a trivial
    /// placeholder carrying no effects.
    fn collection_for_begin(&mut self, events: &[Ev], i: usize, match_of: &[u32]) -> Control {
        if self.collect.is_some() {
            // Collect runs walk the extent linearly: effect recording
            // reaches the enclosing frames directly and the value-stack
            // churn rolls back with the run. A nested collect here would
            // clobber the enclosing run's rollback snapshot.
            return Control::Continue;
        }
        // An unmatched begin (parser recovery) has no extent to consume:
        // fall through to a linear walk.
        let end = match_of[i];
        if end == NIL {
            return Control::Continue;
        }
        let end = end as usize;
        let base = self.exprs.len();
        if let Some(parts) = (i + 1..end).find(|&j| matches!(events[j], Ev::ForParts { .. })) {
            let Ev::ForParts {
                left_sep,
                right_sep,
            } = events[parts]
            else {
                return Control::Jump(end + 1);
            };
            let bounds = ForBounds::build(
                events,
                i,
                parts,
                left_sep.range.offset,
                right_sep.range.offset,
                end,
            );
            self.replay_c_for(bounds, events, match_of, true);
            // The wholesale jump skips the `ForControlFlowEnd` event:
            // close the loop frame here (it would otherwise leak to the
            // end of the executable).
            self.loop_exit(true);
        } else if let Some(parts) = (i + 1..end).find(|&j| matches!(events[j], Ev::ForInParts)) {
            // For-in element: the head (loop variable + iterable) ends
            // BEFORE the parts marker, and `for_in_parts_at` below is the
            // loop head. Replaying the marker would fire the statement-form
            // handler too: a second pop plus a stray non-collection frame.
            self.run_range(events, i + 1..parts, match_of);
            self.for_in_parts_at(events, i + 1, parts, end, true);
            self.run_range(events, parts + 1..end, match_of);
            self.loop_exit(true);
        }
        self.exprs.truncate(base);
        self.exprs.push(Expr::trivial(Ty::UNKNOWN));
        Control::Jump(end + 1)
    }

    /// The loop head of a statement-form for-in (official
    /// `forEach_bodyBegin`): pops the iterable, splits, and conservatively
    /// joins the body's writes.
    fn for_in_parts(&mut self, events: &[Ev], i: usize, _match_of: &[u32]) {
        if self.collect.is_some() {
            return;
        }
        let end = (i..events.len())
            .find(|&j| matches!(events[j], Ev::ForInEnd))
            .unwrap_or(events.len());
        // The head starts right after the statement's `for` (no nested
        // statement-for can sit inside the iterable expression). Without a
        // `ForBegin` (synthetic fragments) there is no head to attribute.
        let head_start = (0..i)
            .rev()
            .find(|&j| matches!(events[j], Ev::ForBegin { .. }))
            .map_or(i, |j| j + 1);
        self.for_in_parts_at(events, head_start, i, end, false);
    }

    fn for_in_parts_at(
        &mut self,
        events: &[Ev],
        head_start: usize,
        i: usize,
        end: usize,
        collection: bool,
    ) {
        // The iterable expression value is on the stack; it contributes no
        // promotion state.
        self.exprs.pop();
        // A declared head's loop variable is initialized before the body
        // (analyzer for_resolver.dart:222-226 `declare(initialized: true)`
        // right before `forEach_bodyBegin`; for_each.dart
        // `forEach_assigns_to_declared_var` expects no diagnostic). Its
        // `VarDeclEnd` has already run (the declaration ends at the `in`),
        // so the variable is resolved by name from the head's
        // `InitializedIdent` — the first one, since the iterable's own
        // declarations follow it. The identifier-head form's target was
        // written at its Ident event; a pattern head declares its
        // variables assigned on match.
        if let Some(var) = events[head_start..i].iter().find_map(|ev| match ev {
            Ev::InitializedIdent { name } => self.resolve(*name),
            _ => None,
        }) {
            self.write_var(var, Ty::UNKNOWN, false);
        }
        let body_start = (i..end).find(|&j| matches!(events[j], Ev::ForInBodyBegin));
        let body = match body_start {
            Some(start) => start..end,
            None => i + 1..end,
        };
        let effects = self.collect_ranges(events, &[body]);
        let previous = self.current.clone();
        self.split_cur();
        self.cjoin(&effects.written, &effects.captured);
        let checkpoint = self.reach.parent(self.current.reach);
        self.stack.push(Frame::While {
            collection,
            checkpoint,
            cond_false: Some(previous),
            break_model: None,
            continue_model: None,
        });
    }

    // —— dispatcher —————————————————————————————————————————————————————

    /// Processes one event. Construct events that own an extent (loops,
    /// collection-`for`, closures) consume the whole extent and return a
    /// jump.
    pub(crate) fn step(&mut self, events: &[Ev], i: usize, match_of: &[u32]) -> Control {
        let ev = &events[i];
        // One-event lookahead: a case group awaiting its body closes its
        // alternatives before any non-case event processes.
        if !matches!(
            ev,
            Ev::CaseBegin { .. } | Ev::CaseExprBegin | Ev::CaseNoWhen | Ev::CaseExprEnd { .. }
        ) {
            self.maybe_close_case_group(events);
        }
        match ev {
            Ev::ExecBegin { is_closure } => {
                self.exec_begin(*is_closure, events, i, match_of);
                Control::Continue
            }
            Ev::ExecEnd => {
                self.exec_end();
                Control::Continue
            }
            Ev::ForBegin { span } => self.for_begin(*span, events, i, match_of),
            Ev::ForControlFlowBegin => self.collection_for_begin(events, i, match_of),
            Ev::ForInParts => {
                self.for_in_parts(events, i, match_of);
                Control::Continue
            }
            Ev::ForInEnd => {
                self.loop_exit(false);
                Control::Continue
            }
            Ev::ForControlFlowEnd { .. } => {
                self.loop_exit(true);
                Control::Continue
            }
            _ => {
                if self.stmt_step(ev, events, i, match_of) {
                    return Control::Continue;
                }
                if self.expr_step(ev, events, i, match_of) {
                    return Control::Continue;
                }
                self.pattern_step(ev, events, i, match_of);
                Control::Continue
            }
        }
    }
}

/// Builds the construct-extent match table: `match_of[i]` is the matching
/// begin/end event index for construct framing events, `NIL` otherwise.
fn build_match_map(events: &[Ev]) -> Vec<u32> {
    use crate::ev::Ev::*;
    let mut match_of = vec![NIL; events.len()];
    let mut for_stack: Vec<usize> = Vec::new();
    let mut while_stack: Vec<usize> = Vec::new();
    let mut do_stack: Vec<usize> = Vec::new();
    let mut exec_stack: Vec<usize> = Vec::new();
    let mut try_stack: Vec<usize> = Vec::new();
    let mut switch_block_stack: Vec<usize> = Vec::new();
    let mut coll_for_stack: Vec<usize> = Vec::new();
    for (i, ev) in events.iter().enumerate() {
        match ev {
            ForBegin { .. } => for_stack.push(i),
            ForEnd { .. } | ForInEnd { .. } => {
                if let Some(begin) = for_stack.pop() {
                    match_of[begin] = i as u32;
                    match_of[i] = begin as u32;
                }
            }
            ForControlFlowBegin => coll_for_stack.push(i),
            ForControlFlowEnd { .. } => {
                if let Some(begin) = coll_for_stack.pop() {
                    match_of[begin] = i as u32;
                    match_of[i] = begin as u32;
                }
            }
            WhileBegin { .. } => while_stack.push(i),
            WhileEnd => {
                if let Some(begin) = while_stack.pop() {
                    match_of[begin] = i as u32;
                    match_of[i] = begin as u32;
                }
            }
            DoBegin { .. } => do_stack.push(i),
            DoEnd => {
                if let Some(begin) = do_stack.pop() {
                    match_of[begin] = i as u32;
                    match_of[i] = begin as u32;
                }
            }
            ExecBegin { .. } => exec_stack.push(i),
            ExecEnd => {
                if let Some(begin) = exec_stack.pop() {
                    match_of[begin] = i as u32;
                    match_of[i] = begin as u32;
                }
            }
            TryBegin { .. } => try_stack.push(i),
            TryEnd { .. } => {
                if let Some(begin) = try_stack.pop() {
                    match_of[begin] = i as u32;
                    match_of[i] = begin as u32;
                }
            }
            SwitchBlockBegin => switch_block_stack.push(i),
            SwitchBlockEnd => {
                if let Some(begin) = switch_block_stack.pop() {
                    match_of[begin] = i as u32;
                    match_of[i] = begin as u32;
                }
            }
            _ => {}
        }
    }
    match_of
}

/// Empty match table for collect runs.
static MATCH_NONE: [u32; 0] = [];
