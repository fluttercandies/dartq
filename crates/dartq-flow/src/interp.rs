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
use dartq_source::{SourceRange, saturate_slot_u32};
use rustc_hash::{FxHashMap, FxHashSet};

use crate::ev::Ev;
use crate::facts::FlowFact;
use crate::model::{Lists, NIL, ReachArena, Reasons, State};
use crate::ty::{Base, Classification, Ty, TypeOps};

/// Metadata of one local variable (or parameter).
pub(crate) struct VarMeta {
    /// Declared type (name-level).
    pub ty: Ty,
    /// The declared type's written argument list (arena index;
    /// [`crate::ty::EMPTY_ARGS`] when argument-less) — the promotion
    /// gate's declared face (#611 step 4).
    pub args: u32,
    pub late: bool,
    pub is_final: bool,
    /// Span of the declaration's name token, when declared locally.
    pub decl_span: Option<SourceRange>,
}

/// One lexical scope frame: name id to variable id.
pub(crate) struct Scope {
    bindings: FxHashMap<u32, u32>,
    /// Monotonic generation, unique per pushed scope instance: the binding
    /// journal's undo entries address their scope by (depth, gen), so a
    /// scope popped during a collect replay and re-pushed at the same depth
    /// is never the target of a stale binding restore.
    scope_id: u32,
}

impl Scope {
    pub(crate) fn new(scope_id: u32) -> Self {
        Self {
            bindings: FxHashMap::default(),
            scope_id,
        }
    }
}

/// One undo entry of the name-binding journal: the scope a binding landed
/// in (depth at bind time plus its generation), the bound name, and the
/// binding it replaced. `collect_ranges`' rollback replays entries past
/// the snapshot point backwards, restoring each scope's pre-collect
/// binding — a collect walk's declarations into a scope that existed
/// before the run must not survive into the real walk (`_produceErrors`'
/// scrutinee once resolved to a like-named variable declared kilobytes
/// later: the if-case pattern then narrowed the wrong face, and every
/// promoted read under it answered `Unknown`).
pub(crate) struct BindingUndo {
    depth: usize,
    scope_id: u32,
    name: u32,
    prev: Option<u32>,
}

/// One open collection if-element: the arm anchors and the saved
/// then-arm state (see [`Interp::coll_if`]).
pub(crate) struct CollIfFrame {
    /// The then-element's first token offset.
    pub(crate) then_start: u32,
    /// The else-element's first token offset, set at `ElseControlFlow`.
    pub(crate) else_start: Option<u32>,
    /// The then-arm's exit state, saved at `ElseControlFlow` (official
    /// `_IfContext._afterThen`).
    pub(crate) after_then: Option<State>,
}

/// Effect sets of one construct extent (official `AssignedVariablesNodeInfo`).
#[derive(Default, Debug)]
pub(crate) struct Effects {
    pub declared: FxHashSet<u32>,
    pub written: FxHashSet<u32>,
    /// The writes recorded at this frame's own depth — closure-nested
    /// writes land only in the closure's frame, so the outermost frame's
    /// set is the direct-write view the write-site rows need (a variable
    /// written both ways carries both facts).
    pub written_direct: FxHashSet<u32>,
    pub read: FxHashSet<u32>,
    /// Reads inside nested closures (official `readCaptured`).
    pub read_captured: FxHashSet<u32>,
    /// Writes inside nested closures (official `captured`).
    pub captured: FxHashSet<u32>,
}

/// Labeled case-group close joins, projected to the variables live when
/// the switch block's effect set was computed (see
/// `maybe_close_case_group`): writes and closure captures only — the
/// labeled close reads no other member of [`Effects`].
pub(crate) struct LabeledEffects {
    pub written: FxHashSet<u32>,
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
    /// The state at `GuardBegin`: the guard-false arm for the close's
    /// unmatched join (the guard-true arm is `current` itself — the
    /// guard's own events ran on it).
    pub pre_guard: Option<State>,
    /// Pattern-declared variables awaiting scope flush (name id, final).
    pub pending_decls: Vec<(u32, bool)>,
    /// The bindings the pattern's variables shadowed in the innermost
    /// scope (name id, previously bound variable): an if-case restores
    /// them when its then branch ends — the pattern variables are in scope
    /// in the guard and the then statement only (`if (p case Sub p) ...
    /// else p.other` reads the OUTER `p` in the else branch).
    pub shadowed: Vec<(u32, Option<u32>)>,
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
        /// The enclosing began-dead snapshot, restored at the executable's
        /// end: a closure body's own statement boundaries must not rewrite
        /// what the enclosing statement began in.
        began_dead: bool,
    },
    Block {
        scopes_base: usize,
    },
    If {
        branch: State,
        after_then: Option<State>,
        /// An if-case's shadowed bindings (see [`PatternState::shadowed`]),
        /// restored when the then branch ends; empty for a plain `if`.
        shadowed: Vec<(u32, Option<u32>)>,
    },
    Cond {
        branch: State,
        then_state: Option<State>,
        /// The then arm's entry state was unreachable (a literal-folded
        /// condition): the dead-arm row fires at the arm's close. An arm
        /// that only ENDS unreachable (a `throw`/Never subexpression) is
        /// live — the official rows only when a node inside was checked
        /// unreachable, which a literal-folded entry guarantees.
        then_dead_entry: bool,
        /// Same latch for the else arm, taken at the colon.
        else_dead_entry: bool,
    },
    Branch {
        branch: State,
        /// `??`'s result type — the LHS's non-null form (official `??`
        /// typing); `None` for the boolean-operator branches.
        ifnull_ty: Option<Ty>,
        /// The operator token (`&&`/`||`/`??`): the dead-RHS row's anchor
        /// at the construct's close.
        op_span: TokenSpan,
        /// The right operand's entry state was unreachable (a literal-folded
        /// left operand, or a guaranteed-non-null `??` LHS): the dead-RHS
        /// row fires at the construct's close. An operand that only ENDS
        /// unreachable (a `throw`/Never subexpression) is live.
        rhs_dead_entry: bool,
    },
    Cascade {
        exprs_base: usize,
        /// `null_shorts` depth at cascade begin, INCLUDING the head `?..`
        /// entry when the cascade opened one: section-end flushes stop
        /// above it (the head's skip path joins at `CascadeEnd`, official
        /// `nullAwareAccess_end` after the last section).
        null_shorts_base: usize,
        /// The head `?..` opened a short (it sits at `null_shorts_base - 1`
        /// and joins at `CascadeEnd`).
        own_short: bool,
    },
    /// A loop: break/continue target (`collection` selects a
    /// collection-`for` element).
    While {
        collection: bool,
        checkpoint: u32,
        cond_false: Option<State>,
        break_model: Option<State>,
        continue_model: Option<State>,
        /// A for-in loop closes with the `forEach_end` formula — the
        /// body's fall-through joins the exit beside the head state —
        /// unlike `whileStatement_end`/`for_end`, which join only the
        /// condition-false and break models.
        for_in: bool,
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
        /// The condition's true state, latched at `AssertAfterCondition`
        /// (official `_AssertContext._conditionTrue`,
        /// `flow_analysis.dart:6481`): `assert_end` joins `previous` with
        /// it. `None` when no message follows (the end reads the
        /// condition's own true arm).
        cond_true: Option<State>,
    },
    /// A switch statement or switch expression (the case-alternatives
    /// machinery).
    Switch {
        is_expr: bool,
        checkpoint: u32,
        previous: State,
        unmatched: State,
        break_model: Option<State>,
        /// The scrutinee's variable when it is a bare read: the promotion
        /// key the matched-type rows answer against (#1349). Any other
        /// scrutinee shape carries no flow-visible promotion.
        scrutinee: Option<u32>,
        /// The scrutinee's flow type at the block's open (the promoted type
        /// of a bare-read scrutinee — upstream's `scrutineeType` IS the
        /// promoted static type) beside its written-argument tree: the
        /// `switch_end` `isExhaustive` answer ([`Interp::is_always_exhaustive`]).
        scrutinee_ty: Ty,
        scrutinee_args: u32,
        /// The switch-expression latch: the match-time matched-type row a
        /// `SwitchExprCasePattern` computed, emitted at the alternative's
        /// `SwitchExprCaseEnd` where the clause anchor token arrives (the
        /// chain state updates at the pattern close, so the row cannot be
        /// recomputed there).
        pending_matched: Option<(Ty, u32)>,
        /// A switch expression's latched case-value types: one per
        /// alternative's body value, taken at its `SwitchExprCaseEnd` and
        /// joined at `SwitchExprBlockEnd` into the expression's type
        /// (upstream Dart SDK source, pkg/analyzer/lib/src/dart/ast/
        /// ast.dart `SwitchExpressionImpl.staticType`'s arm fold — the
        /// name-level join here is [`Interp::name_lub`], the same join
        /// `??` carries; the real hierarchy's LUB stays the resolved
        /// back-end's question). Empty for statement switches and
        /// recovered case-less blocks.
        expr_case_tys: Vec<Ty>,
        /// A `CaseExprEnd` was seen: the group awaits its first body event
        /// (one-event lookahead closes the alternatives there).
        case_open: bool,
        /// The open member's pattern binders' shadowed bindings (the arm
        /// counterpart of [`Frame::If::shadowed`]): a `case Sub x:` binder
        /// shadows only inside its own member — the member's close
        /// restores the outer binding so later members' reads of the
        /// spelling resolve the scrutinee again.
        case_shadowed: Vec<(u32, Option<u32>)>,
        group_labels: bool,
        group_default: bool,
        group_combined: Option<State>,
        /// Set once the group closed and its body started.
        body_depth: Option<usize>,
        /// The just-closed group's `default` keyword with whether its own
        /// keyword row already fired (probe table G rule 2): the next
        /// group's opening consumes it — the chain marks unreachable, and a
        /// case-headed opener reports the default's keyword row.
        dead_default: Option<(TokenSpan, bool)>,
        /// The offset of the member keyword whose head-time row last fired
        /// in this switch (a member-anchored report): the next `CaseEnd`
        /// latch reads it so an already-reported default is never
        /// re-reported by its group's close.
        reported_kw: Option<u32>,
        block: Range<usize>,
        /// The block's effect set for labeled closes, computed once at the
        /// first labeled close (see `maybe_close_case_group`); `None` until
        /// then — a switch with no labeled group never pays for it.
        labeled_effects: Option<LabeledEffects>,
    },
    /// A labeled statement: a labeled break's target. `label` is the
    /// first label's interned id (`None` only when a recovered label
    /// never recorded a name); `extra_labels` holds the remaining labels
    /// of the statement (`a: b: s` — either name targets it).
    Labeled {
        label: Option<u32>,
        extra_labels: Vec<u32>,
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
    /// The right separator's end: the header/body containment boundary
    /// for the loop's body close (an anchor past it is a body descendant,
    /// one at or before it belongs to the header).
    pub header_end: u32,
}

impl ForBounds {
    /// Partitions the header. The offset scan keys on the parts' spanned
    /// wrapper events, but an expression's events PRECEDE its `ExprStmt`
    /// wrapper (the wrapper fires at the statement's end), so a literal
    /// condition's value event sits before anything spanned in its region.
    /// The `ForInit` marker — recorded when the initializer finishes — is
    /// therefore the init/cond index boundary and overrides the scan; the
    /// cond/upd boundary stays offset-based (the right separator is the
    /// first updater's token, and updater events carry its offset or a
    /// postfix end at or after it).
    pub(crate) fn build(
        events: &[Ev],
        begin: usize,
        parts: usize,
        left: u32,
        right: u32,
        header_end: u32,
        end: usize,
    ) -> Self {
        let mut cond_start = parts;
        let mut upd_start = parts;
        for (j, event) in events.iter().enumerate().take(parts).skip(begin + 1) {
            if let Some(span) = event.span() {
                if span.range.offset >= right {
                    upd_start = upd_start.min(j);
                } else if span.range.offset >= left {
                    cond_start = cond_start.min(j);
                }
            }
        }
        for (j, event) in events.iter().enumerate().take(parts).skip(begin + 1) {
            if matches!(event, Ev::ForInit) {
                cond_start = j + 1;
                break;
            }
        }
        Self {
            init: begin + 1..cond_start,
            cond: cond_start..upd_start,
            upd: upd_start..parts,
            body: parts + 1..end,
            header_end,
        }
    }
}

/// The current variable-declaration context.
pub(crate) struct DeclCtx {
    pub late: bool,
    pub is_final: bool,
    pub ty: Ty,
    /// The declared type's written arguments (arena index), beside
    /// [`DeclCtx::ty`].
    pub args: u32,
}

impl Default for DeclCtx {
    fn default() -> Self {
        Self {
            late: false,
            is_final: false,
            ty: Ty::UNKNOWN,
            args: crate::ty::EMPTY_ARGS,
        }
    }
}

/// One expression value on the expression stack.
/// One captured condition-variable binding (language#1274): the split
/// states saved at `final b = v != null;` (or `==`, or a type test),
/// replayed when `b` reads as a condition operand.
pub(crate) struct CondVarEntry {
    /// The tested variable.
    pub tested: u32,
    /// Its write-version at capture — a mismatch at replay invalidates.
    pub version: u32,
    /// The split's true-arm state (the tested variable's model read).
    pub if_true: State,
    /// The false-arm state.
    pub if_false: State,
}

pub(crate) struct Expr {
    /// Type of the expression (name-level).
    pub ty: Ty,
    /// The written-argument tree of [`Expr::ty`] (arena index;
    /// [`crate::ty::EMPTY_ARGS`] when argument-less or unknown) — the
    /// promotion sites that refine this value's type cons the rows with
    /// it (#1010).
    pub args: u32,
    /// The expression reads this variable (promotable reference).
    pub reference: Option<u32>,
    /// The expression reads a property whose receiver is itself a
    /// promotable reference (a bare local read or `this`): the property
    /// slot ([`Interp::prop_slot`]) the promotion state lives in (#1409).
    /// The read's flow type at this layer is the marker pair
    /// ([`Ty::UNKNOWN_NULLABLE`] unpromoted, [`Ty::UNKNOWN`] promoted).
    pub prop: Option<u32>,
    /// The expression is exactly a `this` read: the promotable receiver
    /// shape of property promotion (official `thisExpression()`'s
    /// `_thisVersion` reference, `flow_analysis.dart:8149-8153`).
    pub this_recv: bool,
    /// Boolean literal truth.
    pub bool_value: Option<bool>,
    /// Non-trivial branch states (`ifTrue`/`ifFalse`); `None` = trivial.
    pub branches: Option<(State, State)>,
    /// The branches are a type-test/equality split whose fold also
    /// narrowed `self.current` to the true arm: a value consumer that
    /// drops this expression rejoins both arms
    /// ([`Interp::rejoin_value_split`]). Null-short and control-branch
    /// pairs resolve through their own machinery and never rejoin here.
    pub test_split: bool,
    /// The token of a bare variable read, when this expression is exactly
    /// one: a plain assignment target retracts the read's fact (the
    /// identifier is the write, not a read).
    pub read_span: Option<TokenSpan>,
    /// The callee identifier's token when this access row is a following
    /// send's callee (`x.m(...)`'s `m`): the key the staged
    /// invocation-result lookup reads (#2567 R18). No other consumer —
    /// the read-span faces stay untouched by the callee role.
    pub callee_span: Option<TokenSpan>,
    /// The interned name when this expression came from an identifier
    /// event: the member face of a property read (`c._b`'s `_b` — the
    /// property key's field half).
    pub name: Option<u32>,
    /// The `exprs` index where this value's null-shorted chain began — the
    /// receiver row's own index (or its propagated `chain_base` for a
    /// chained `x?.y?.z`), set on the result row a null-aware
    /// DotAccess/IndexAccess pushes. A consumer closing the operand region
    /// above this index owns the chain's join (official
    /// `finishNullShorting`'s `targetDepth`, type_analyzer.dart:804-827 —
    /// the shorts opened by the consumed expression, all the way down its
    /// chain).
    pub chain_base: Option<usize>,
    /// The value is a collection literal's collapsed head: the type the
    /// name level answers is the unknown face, but the written value is a
    /// real `List`/`Set`/`Map` instance — an assignment target's write
    /// type is a proper subtype of any named declared non-null form, the
    /// face the promotion's written-type gate admits (#2401).
    pub literal: bool,
    /// The expression's invalid face arrived through a NON-reference
    /// shape — a member read over an invalid receiver, an unresolved
    /// call's result. The official's is-check folds are strict on such
    /// an operand (only a trackable reference — a local, or the bare
    /// implicit-`this` read the resolver treats as one — carries the
    /// lenient vacuous verdict; oracle-probed: `x.foo is String` stays
    /// silent while the extension's bare `foo is String` deads its
    /// else).
    pub non_reference_invalid: bool,
}

impl Expr {
    pub(crate) fn trivial(ty: Ty) -> Self {
        Self {
            ty,
            args: crate::ty::EMPTY_ARGS,
            reference: None,
            prop: None,
            this_recv: false,
            bool_value: None,
            branches: None,
            test_split: false,
            read_span: None,
            callee_span: None,
            name: None,
            chain_base: None,
            literal: false,
            non_reference_invalid: false,
        }
    }

    pub(crate) fn of_reference(var: u32, ty: Ty, args: u32) -> Self {
        Self {
            ty,
            args,
            reference: Some(var),
            prop: None,
            this_recv: false,
            bool_value: None,
            branches: None,
            test_split: false,
            read_span: None,
            callee_span: None,
            name: None,
            chain_base: None,
            literal: false,
            non_reference_invalid: false,
        }
    }

    /// The promotable receiver identity of this expression, when property
    /// promotion can track through it: a bare local read (its variable id)
    /// or a `this` read ([`THIS_RECEIVER`]).
    pub(crate) fn prop_receiver(&self) -> Option<u32> {
        if self.this_recv {
            Some(THIS_RECEIVER)
        } else {
            self.reference
        }
    }
}

/// The property-key receiver sentinel for `this` (explicit or implicit):
/// `this` never rebinds within one executable body (the official
/// `_thisVersions` stack exists for extension-member dispatch, which this
/// layer does not analyze), so one fixed identity is sound (#1409;
/// eligibility per upstream `ThisPropertyTarget`,
/// `flow_analysis.dart:5977-5990`).
pub(crate) const THIS_RECEIVER: u32 = u32::MAX - 1;

/// Recursion budget for the always-exhaustive bound walk
/// ([`Interp::is_always_exhaustive`]). A well-formed `extends` chain is
/// far shorter than this; the budget only bounds the spelling-level cycle
/// an ill-formed `T extends U, U extends T` would otherwise spin on.
const BOUND_RECURSION_LIMIT: u8 = 16;

/// One open null-shorting context (official `_NullAwareAccessContext`,
/// flow_analysis.dart:9664): the skip path captured at the `?.`/`?[`/`?..`
/// and the access-path state the condition face can still read back.
pub(crate) struct NullShort {
    /// The receiver-was-null path (upstream `shortcutControlPath` —
    /// `flow_analysis.dart:9206`): the pre-split state, made unreachable
    /// when the receiver type can never be null (:9217-9221).
    pub shortcut: State,
    /// The receiver-was-non-null path (upstream's promoted `_current` at
    /// the access): the consumer-visible branch pair's non-null half, kept
    /// so the expression's `branches` still splits a condition on it.
    pub matched: Option<State>,
    /// `exprs` depth when the context opened: the `?` belongs to the
    /// expression whose operand region began at this depth, so a consumer
    /// closing a deeper operand (`throw e`, `f(e)`, `x[e]`) leaves it open
    /// — the official `nullShortingDepth` bound (`finishNullShorting`'s
    /// `targetDepth`, type_analyzer.dart:804-827).
    pub exprs_depth: usize,
    /// Whether the access path was already dead when the context opened —
    /// the pre-split state was unreachable, or a never-null/Null-typed
    /// receiver classified it dead (:9214-9221). The member-level dead-node
    /// poll (`checkUnreachableNode`, type_analyzer.dart:2082) reads the
    /// state as the member STARTED — a `throw`/Never inside the member's own
    /// tail (`x?.m(throw _)`) dies after the poll, so it must not anchor the
    /// access row; a dead-at-open path is the genuine dead member.
    pub dead_at_open: bool,
}

/// The receiver-invalidation sweep over one state's property slots: the
/// state-level half of [`Interp::join_state_with_sweep`], also driven
/// directly by [`Interp::write_var`] with empty finals (a true write has
/// no finality exception — see [`Interp::join_state_with_sweep`] for the
/// upstream anchors). A cleared promoted chain is not destroyed: each
/// entry lands in the slot's [`VarModel::history`] as
/// [`crate::facts::NonPromotionCause::PropertyEvidence`] — the flat-slot form of upstream's
/// `_nonPromotableProperties` version chain (`flow_analysis.dart:5707-5719`),
/// so a later failed read can still name the promotion that WOULD have
/// stood when the consumer resolves the member's non-promotability.
pub(crate) fn sweep_state_props(
    state: &mut State,
    props: &[(u32, u32)],
    written: &FxHashSet<u32>,
    captured: &FxHashSet<u32>,
    finals: &FxHashSet<u32>,
    lists: &Lists,
    reasons: &mut Reasons,
) {
    // Slots interned after this state forked (property reads inside the
    // joined region) sit past the state's extent — `State::props` only
    // grows through the resizing promotion writes. Fresh models fill the
    // gap (the same fill [`State::with_prop`] uses) so the indexed writes
    // below stay in bounds; for a never-touched slot the fresh model IS
    // the entry ([`State::prop`]'s fallback, now materialized).
    state
        .props
        .resize(props.len(), crate::model::VarModel::fresh());
    for slot in 0..props.len() {
        let (receiver, _) = props[slot];
        if captured.contains(&receiver) {
            let model = state.prop(slot as u32);
            if model.captured() && model.promoted == NIL {
                continue;
            }
            let history = stash_prop_evidence(model.promoted, model.history, lists, reasons);
            state.props[slot] = crate::model::VarModel {
                promoted: NIL,
                tested: NIL,
                version: crate::model::CAPTURED,
                history,
                ..model
            };
        } else if written.contains(&receiver) && !finals.contains(&receiver) {
            let model = state.prop(slot as u32);
            if model.promoted == NIL {
                continue;
            }
            let history = stash_prop_evidence(model.promoted, model.history, lists, reasons);
            state.props[slot] = crate::model::VarModel {
                promoted: NIL,
                history,
                ..model
            };
        }
    }
}

/// Moves a property slot's cleared promoted chain into its history as
/// would-be evidence, most specific first (the official's
/// `allPreviouslyPromotedTypes` gathers `promotedTypes` in version order —
/// upstream Dart SDK source, flow_analysis.dart:8773-8788).
fn stash_prop_evidence(promoted: u32, history: u32, lists: &Lists, reasons: &mut Reasons) -> u32 {
    let mut history = history;
    for (ty, args) in lists.iter_entries(promoted) {
        history = reasons.cons(
            ty,
            args,
            crate::facts::NonPromotionCause::PropertyEvidence,
            history,
        );
    }
    history
}

/// One open dead-code interval (official `_DeadCodeStart` plus the
/// emission state its covering `flowEnd` needs).
#[derive(Clone, Copy)]
pub(crate) struct DeadWindow {
    /// Replay-frame depth when the interval opened: a construct-end close
    /// covers the anchor exactly when the anchor's frames are all still on
    /// the stack (official `_containsFirstDeadNode`).
    depth: usize,
    /// The first dead node's first token: the row's anchor.
    anchor: TokenSpan,
    /// End of the interval so far: the furthest statement end seen while
    /// the interval is open — the covering list's last statement's
    /// `node.end`, its trailing `;` included (`StmtEnd` events; official
    /// dead_code_verifier.dart:253-279 trim, :326-332 length).
    end: u32,
    /// Do-while split (official `parent is DoStatement`): the interval's
    /// anchor is a do-while condition whose back edge is dead, so the
    /// close emits the `while (c);` part first and the tail second.
    split: Option<SourceRange>,
    /// The interval opened on a do-while back edge and still waits for
    /// the statement's `while` part (`DoEnd` attaches it).
    awaiting: bool,
    /// The split's second row starts at the first dead statement after
    /// the do-while (official `semicolon.next`).
    split_start: Option<u32>,
    /// The split's second row is suppressed: a break exits the do-while,
    /// so the code after it is reachable (official `hasBreakStatement`).
    split_broken: bool,
    /// The interval's row is reported by the semantic layer's as-cast
    /// tail face instead: the cast-type poll opened it directly inside a
    /// variable initializer, where that face stages the official row at
    /// the cast type's head token (probed: dart analyze 3.13.1 —
    /// `var a = f() as int;` with a Never-typed `f()` anchors `DEAD_CODE`
    /// there). The interval itself still follows the official one-row
    /// rule (upstream Dart SDK source,
    /// pkg/analyzer/lib/src/error/dead_code_verifier.dart:183-189 — later
    /// unreachable nodes continue it, a covering `flowEnd` retires it),
    /// so following dead statements extend it; only the emission skips.
    suppressed: bool,
}

impl DeadWindow {
    /// Opens the interval at the first dead node.
    fn at(depth: usize, anchor: TokenSpan) -> Self {
        Self {
            depth,
            anchor,
            end: anchor.range.end(),
            split: None,
            awaiting: false,
            split_start: None,
            split_broken: false,
            suppressed: false,
        }
    }

    /// Opens the interval for a do-while condition (the anchor arrives
    /// with the statement's `while` part at `DoEnd`).
    fn awaiting_do(depth: usize) -> Self {
        Self {
            depth,
            anchor: TokenSpan {
                id: 0,
                range: SourceRange {
                    offset: 0,
                    length: 0,
                },
            },
            end: 0,
            split: None,
            awaiting: true,
            split_start: None,
            split_broken: false,
            suppressed: false,
        }
    }

    /// Whether the interval waits for its do-while's `while` part.
    pub(crate) fn awaiting_split(&self) -> bool {
        self.awaiting
    }

    /// Whether the interval's anchor is the node whose frame stack depth is
    /// `depth` — the wholly-dead block face: the anchor IS the covering
    /// node, so the close spans the block's full extent including `}`
    /// (official `flowEnd`'s `node != firstDeadNode` trim applies only to
    /// inner anchors, dead_code_verifier.dart:253-279).
    pub(crate) fn anchored_at_depth(&self, depth: usize) -> bool {
        self.depth == depth
    }

    /// Whether the interval's anchor starts at `offset` (the switch
    /// member's own keyword — the member anchored the interval itself).
    pub(crate) fn anchor_at(&self, offset: u32) -> bool {
        self.anchor.range.offset == offset
    }

    /// Grows the interval's end to a later dead statement's end (the
    /// official trim to the covering block's last statement).
    pub(crate) fn extend(&mut self, end: u32) {
        if end > self.end {
            self.end = end;
        }
    }

    /// Re-anchors the interval to a statement wrapper whose span begins
    /// at or before the current anchor (the wrapper's statement IS the
    /// official first dead node — the verifier walks top-down, so the
    /// statement precedes every child whose inner poll opened this
    /// interval). A pending do-while split tail that the same statement
    /// interior anchored moves with it.
    pub(crate) fn reanchor_to(&mut self, span: TokenSpan) {
        self.anchor = span;
        if self.split_start.is_some_and(|start| start >= span.range.offset) {
            self.split_start = Some(span.range.offset);
        }
    }

    /// Whether the interval's anchor starts at or after `offset` (the
    /// wrapper's own statement begin — the anchor sits inside it).
    pub(crate) fn anchor_from(&self, offset: u32) -> bool {
        self.anchor.range.offset >= offset
    }

    /// Attaches the do-while's `while (c);` part and records whether a
    /// break makes the post-do tail reachable (official
    /// `hasBreakStatement`).
    pub(crate) fn attach_split(&mut self, while_part: SourceRange, tail_reachable: bool) {
        self.split = Some(while_part);
        self.awaiting = false;
        self.split_broken = tail_reachable;
    }

    /// Marks the interval's row as the semantic as-cast tail face's: the
    /// cast-type poll opened it directly inside a variable initializer.
    pub(crate) fn suppress(&mut self) {
        self.suppressed = true;
    }
}

/// How a construct end covers an open dead-code interval
/// (official `_containsFirstDeadNode`).
#[derive(Clone, Copy)]
pub(crate) enum Covered {
    /// The anchor must sit strictly inside the construct that ends here
    /// (branch, loop body, try/catch body).
    Strict,
    /// An anchor at the closing depth is covered too (a switch case body
    /// keeps no frame of its own).
    Body,
    /// The declaration exit: covers everything.
    Exec,
}

/// The interpreter over one executable's event buffer.
#[expect(
    clippy::struct_excessive_bools,
    reason = "flow state machine: the declaration/type-test/cast contexts and \
              the cast-pattern/as-base phases are independent phases of one \
              walk, not a state encoding"
)]
pub(crate) struct Interp<'o> {
    pub(crate) ops: &'o dyn TypeOps,
    /// The recorder's written-argument arena (index 0 reserved empty):
    /// `Ev::Type` rows carry ids into it and the promotion gate compares
    /// the trees structurally (#611 step 4).
    pub(crate) args: &'o [Vec<crate::ty::ArgTy>],
    /// Declared-bound nullability faces by interned type-parameter name
    /// ([`crate::ty::Boundary`]) — the enclosing declaration's table the
    /// recorder captured at parse time. Absent rows degrade to the
    /// syntactic classification.
    pub(crate) boundaries: &'o [Option<crate::ty::Boundary>],
    /// The unit's closed-declaration names (sealed classes, enums, `bool`,
    /// `FutureOr`) the switch-end exhaustiveness verdict reads — the
    /// recorder's capture, mounted at replay time.
    pub(crate) closed: &'o crate::ty::ClosedNames,
    /// The unit's top-level variable declarations (the recorder's capture,
    /// mounted at replay time): the lexical chain's outermost scope, the
    /// read fallback once every scope frame misses. The empty table is the
    /// direct-construction default.
    pub(crate) top_level: &'o FxHashMap<u32, crate::record::TopVar>,
    /// The inferred locals' declared types the semantic layer staged for
    /// this unit (#2140), keyed on the declarator name token's offset —
    /// adopted at `InitializerEnd` when the written header left the slot
    /// unknown. Read-only for the replay.
    pub(crate) inferred_locals: &'o FxHashMap<u32, (crate::ty::Ty, u32)>,
    /// The staged assignment RHS types the semantic layer resolved for this
    /// unit (#2174), beside their written-argument arena index (#2364),
    /// keyed on the write target token's offset — adopted at `Ev::Assign`
    /// when the fold's own RHS read is the unknown face. Read-only for the
    /// replay.
    pub(crate) staged_rhs: &'o FxHashMap<u32, (crate::ty::Ty, u32)>,
    /// `this`-read types keyed on the `this` token's offset: the semantic
    /// layer resolves a `this` read its event stream leaves unknown — an
    /// extension or extension-type instance member's extended type — and
    /// stages it here before the executable's replay. The `Ev::This` fold
    /// adopts it when its own answer is the unknown face; the `switch
    /// (this)` scrutinee and every `this`-chained read then answer on the
    /// real type. Entries whose `this` never folds are inert (bounded by
    /// the unit's `this`-read count).
    pub(crate) staged_this: &'o FxHashMap<u32, crate::ty::Ty>,
    /// The staged invocation-result types the semantic layer resolved for
    /// this unit (#2567 R18), beside their written-argument arena index,
    /// keyed on the callee identifier token's offset — adopted at
    /// `Ev::Send`/`Ev::SendNoArgs` when the fold's own callee read is an
    /// unknown face, so a switch whose scrutinee is a resolved call
    /// (`readEnum(Tag.values)`'s generic return) answers the
    /// always-exhaustive verdict on the real type. Read-only for the
    /// replay.
    pub(crate) staged_calls: &'o FxHashMap<u32, (crate::ty::Ty, u32)>,
    /// The recorder's interned name lexemes, mounted at replay time: the
    /// class-head face (`x = C()`) reads the callee's spelling — a
    /// lowercase head is a function/getter call, not an implicit creation.
    /// The empty slice is the direct-construction default (every head
    /// reads as a non-class call).
    pub(crate) names: &'o [Box<str>],
    pub(crate) reach: ReachArena,
    pub(crate) lists: Lists,
    /// The non-promotion history pool beside [`Interp::lists`] — the same
    /// per-executable lifetime (fresh per `run`, folded into the
    /// recorder's unit arena at `exec_end`).
    pub(crate) reasons: Reasons,
    /// The interned `WhyNotPromoted`/`NonPromotedPropertyRead`/
    /// `ThisNotPromoted` entry rows this executable produced — index 0 is
    /// the reserved empty row (upstream's empty-map shape); the recorder
    /// appends it to the unit arena at `exec_end` and rewrites the facts'
    /// `entries` ids by the base offset.
    pub(crate) non_promotions: Vec<Vec<crate::facts::NonPromotionEntry>>,
    pub(crate) current: State,
    pub(crate) stack: Vec<Frame>,
    pub(crate) exprs: Vec<Expr>,
    pub(crate) scopes: Vec<Scope>,
    pub(crate) vars: Vec<VarMeta>,
    pub(crate) facts: Vec<FlowFact>,
    /// The property-key table (#1409): slot → `(receiver, field)` — the
    /// receiver is a variable id or [`THIS_RECEIVER`], the field an
    /// interned member name (the flow-level stand-in for upstream's
    /// `propertyName` keys, `flow_analysis.dart:5694-5716`). Slot order is
    /// allocation order and matches every [`State::props`] index; slots are
    /// never freed (stable across collect rollbacks, which truncate back to
    /// the snapshot's length).
    pub(crate) props: Vec<(u32, u32)>,
    /// The tested reference of the most recent type-test/equality-null
    /// split fold (the condition-variable capture's feed, language#1274):
    /// an initializer whose expression IS that fold binds the split to
    /// the declared local.
    pub(crate) cond_split_var: Option<u32>,
    /// The condition variables (language#1274, official
    /// `restoreConditionVariableState`): a bool local bound from a
    /// null-comparison or type-test split replays the split's
    /// tested-variable models when the local itself is later read as a
    /// condition operand — invalidated by an intervening write (the
    /// captured write-version check stands in the official's
    /// `ValueVersion` rebasing).
    pub(crate) cond_vars: std::collections::HashMap<u32, CondVarEntry>,
    /// Local-function names declared in the unit ([`crate::ev::Ev::
    /// LocalFnName`] feed): a bare-head invocation of one resolves in scope,
    /// so the unresolved-call `InvalidType` face must skip it — the
    /// library-level back-end cannot see function-local names.
    pub(crate) local_fns: FxHashSet<u32>,
    /// Property key → slot, the resolve-or-insert side of
    /// [`Interp::prop_slot`].
    prop_slots: FxHashMap<(u32, u32), u32>,

    /// Monotonic scope generation: every pushed scope takes the next
    /// value, so a re-pushed scope at a recycled depth never aliases a
    /// journal entry's target.
    pub(crate) scope_gen: u32,
    /// Open collection if-elements, innermost last. Each entry carries the
    /// arm anchors and the saved then-arm state (official
    /// `ifStatement_elseBegin`'s `context._afterThen = _current`,
    /// flow_analysis.dart:7111-7115) — the dead-arm rows anchor each arm's
    /// own extent (official `handle_ifElement_thenEnd` / `elseEnd` →
    /// `flowEnd(ifTrue)` / `flowEnd(ifFalse)`, type_analyzer.dart:1163-1173)
    /// and the close joins `afterThen` with `afterElse`, not the branch
    /// model.
    pub(crate) coll_if: Vec<CollIfFrame>,
    /// The name-binding undo journal ([`BindingUndo`]): one entry per
    /// `bind_name`, replayed backwards by `collect_ranges`' rollback for
    /// entries past the snapshot point.
    binding_journal: Vec<BindingUndo>,

    pub(crate) decl: DeclCtx,
    /// Enclosing declaration contexts: `VarDeclBegin` swaps the single
    /// active slot out to here, `VarDeclEnd` restores it (#1051 — a nested
    /// list's end must not reset the enclosing list's header type),
    /// mirroring [`Interp::pending_decl_bases`] for the pending-init list.
    pub(crate) decl_stack: Vec<DeclCtx>,
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
    /// The open logical-or alternation's latch (depth-1 refutable
    /// context, bare-read scrutinee): the scrutinee's promoted face
    /// after the LEFT branch parsed — the close compares the right
    /// branch's face against it (same → the shared promotion stands,
    /// differing → a union the name level cannot spell restores the
    /// pre-pattern face).
    pub(crate) or_pattern_latch: Option<(u32, Ty, u32)>,
    /// The pattern depth at which an open top-level OR alternation's
    /// right operand parses (its object pattern's promotion gate).
    pub(crate) or_operand_depth: Option<usize>,
    /// The open OR alternation's right-branch promotion face: `None`
    /// until the right operand's object pattern runs, then `Some(None)`
    /// when it promoted nothing and `Some(Some(face))` when it did —
    /// the close's join decision (`or_pattern_end`).
    pub(crate) or_right_face: Option<Option<(Ty, u32)>>,
    pub(crate) type_test_pending: Option<(bool, Ty, u32)>,
    pub(crate) as_cast_pending: Option<Ty>,
    /// The type currently being parsed (test/cast/formal consumers).
    pub(crate) tested_ty: Option<Ty>,
    /// The open `is`-test's written arguments, latched with the type into
    /// the pending tuple at `IsOperator`.
    pub(crate) tested_args: u32,
    /// The open `as`-cast's written arguments, beside
    /// [`Interp::as_cast_pending`].
    pub(crate) as_cast_args: u32,
    /// The pending declared type's written arguments (formal, pattern
    /// variable consumers), beside [`Interp::pending_ty`].
    pub(crate) pending_ty_args: u32,
    pub(crate) in_decl: bool,
    pub(crate) in_type_test: bool,
    pub(crate) in_as_cast: bool,
    pub(crate) for_header_depth: usize,
    /// The depth contributed by condition slices of C-style `for` headers
    /// (raised by `run_header_expr` only): condition wrappers dead-anchor
    /// like body-level statements (`header_dead_check`), so the counter
    /// separates them from init/updater slices.
    pub(crate) header_cond_depth: usize,
    pub(crate) anywhere: Effects,
    /// The open dead-code interval (official
    /// `NullSafetyDeadCodeVerifier._firstDead`): the first unreachable
    /// node anchors one row; every later unreachable node continues the
    /// same interval until a covering construct ends (`flowEnd`), and the
    /// row then spans the anchor through the last dead statement.
    /// Blocks, labels, and do-while conditions end without a covering
    /// close, so their intervals run out to the enclosing construct.
    pub(crate) dead: Option<DeadWindow>,

    /// Statement-boundary reachability snapshot: whether the statement now
    /// beginning entered dead. Post-order statement wrappers
    /// (`ExprStmt`/`ReturnBegin`/`YieldBegin`/`PatternVarDeclStmt` fire at
    /// the statement's end) read it instead of the live state, so a
    /// statement that began live but terminated flow itself (`throw 0;`)
    /// does not self-anchor the dead row — the official's first dead node
    /// is the next visited node (`_visitNode`).
    pub(crate) stmt_began_dead: bool,

    /// The start offset of the [`Ev::Exit`] that killed the flow, while the
    /// flow is dead from it. The statement wrapper's anchor decision
    /// ([`Interp::wrapper_dead_check`]) reads it: the recorder can drain
    /// the exit past its own statement's `StmtEnd`, whose snapshot refresh
    /// still reads a live state, so a following statement compares the
    /// exit position with its own begin.
    pub(crate) exit_at: Option<u32>,
    /// Open `??=` skip-path snapshots (paired with [`crate::ev::Ev::
    /// IfNullAssignBegin`]): the state at the operator — the live path
    /// that skips the RHS. An RHS that exits (`x ??= throw e`) restores
    /// its snapshot at the write, the official's post-state.
    pub(crate) if_null_skips: Vec<(u32, State)>,
    /// A deferred object-pattern vacuous cover (see
    /// [`crate::patterns`]' `resolve_vacuous_object_cover`): the owner
    /// name id and the head-time state — the may-fail arm should the
    /// close find an invalid-typed property sub-pattern, else the cover's
    /// kill. Cleared at every pattern-context open and decided at every
    /// close.
    pub(crate) vacuous_object: Option<(u32, State)>,
    /// The deferred cover's collected property sub-pattern names (the
    /// shorthand binds' names — the member's own spelling).
    pub(crate) vacuous_fields: Vec<u32>,
    /// The open pattern context's bind buffer (post-order: a property's
    /// bind precedes its object pattern's own event) — the defer's
    /// snapshot source. Cleared at every pattern-context open.
    pub(crate) pattern_binds: Vec<u32>,

    /// Open null-shorting contexts (official `_NullAwareAccessContext`
    /// stack, flow_analysis.dart:9664): each entry is the skip-path state
    /// captured when a `?.`/`?[`/`?..` was reached — the pre-promotion
    /// state of `_nullAwareAccess_rightBegin` (:9206), or unreachable when
    /// the receiver type can never be null (:9217-9221). The chain's
    /// shorted run continues in `self.current` (the receiver promoted
    /// non-null); the pending states rejoin at the enclosing expression's
    /// boundary — upstream's `finishNullShorting`/`nullAwareAccess_end`
    /// (`_join(_current, context._previous).unsplit()`, :7403) — which the
    /// event stream marks at every value-consumer event.
    pub(crate) null_shorts: Vec<NullShort>,

    /// The expression-stack depth each OPEN argument list began at — the
    /// per-argument flush bound ([`Ev::ArgumentEnd`]): a `?.` short opened
    /// inside one argument closes at that argument's own end, while a
    /// caller-side short (`f()?.m(g()?.n)`'s outer `?`) stays open to the
    /// chain's end. Pushed at [`Ev::ArgumentsBegin`], popped at
    /// [`Ev::ArgumentsEnd`], truncated with a collect run's rollback.
    pub(crate) args_bases: Vec<usize>,

    /// The executable-frame depth when the current variable initializer
    /// began; `None` outside one. The cast-type dead poll ([`Ev::AsBegin`])
    /// reads it: a poll that opens the interval directly inside the
    /// initializer (no nested executable between) marks the interval
    /// suppressed — the semantic layer's as-cast tail face reports that
    /// row, and a flow row of its own would duplicate it.
    pub(crate) init_exec_depth: Option<usize>,

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
    /// The binding journal's length at freeze: rollback replays the
    /// entries beyond it, restoring every scope binding the collect walk
    /// touched (truncating `scopes` alone leaves a walked-in binding in a
    /// pre-existing scope alive — the next real-walk read resolves the
    /// collect-only variable).
    journal_len: usize,
    /// The interned non-promotion rows roll back with the collect run —
    /// their fact rows do, so unreferenced rows must not linger.
    non_promotions_len: usize,
    props_len: usize,
    dead: Option<DeadWindow>,
    init_exec_depth: Option<usize>,
    /// Open null-shorting contexts: pushed and joined inside the collect
    /// run's replay, so they roll back with it.
    null_shorts_len: usize,
    /// Open argument-list bases roll back with the collect run too.
    args_bases_len: usize,
}

/// The direct-construction default for [`Interp::top_level`]: no unit face.
static NO_TOP_LEVEL: std::sync::LazyLock<FxHashMap<u32, crate::record::TopVar>> =
    std::sync::LazyLock::new(FxHashMap::default);

/// The direct-construction default for [`Interp::inferred_locals`]: no
/// staged inferred-local types — every `var`/`final` declarator keeps its
/// unknown-declared slot (the pre-#2140 behavior).
static NO_INFERRED_LOCALS: std::sync::LazyLock<FxHashMap<u32, (crate::ty::Ty, u32)>> =
    std::sync::LazyLock::new(FxHashMap::default);

/// The direct-construction default for [`Interp::staged_rhs`]: no staged
/// assignment RHS types — every plain `=` write keeps the fold's own RHS
/// read (the pre-#2174 behavior).
static NO_STAGED_RHS: std::sync::LazyLock<FxHashMap<u32, (crate::ty::Ty, u32)>> =
    std::sync::LazyLock::new(FxHashMap::default);

/// The direct-construction default for [`Interp::staged_this`]: no staged
/// `this` types — every `this` read keeps the unknown face (the
/// pre-#2292 behavior).
static NO_STAGED_THIS: std::sync::LazyLock<FxHashMap<u32, crate::ty::Ty>> =
    std::sync::LazyLock::new(FxHashMap::default);

/// The direct-construction default for [`Interp::staged_calls`]: no staged
/// invocation results — every send keeps the fold's own callee face.
static NO_STAGED_CALLS: std::sync::LazyLock<FxHashMap<u32, (crate::ty::Ty, u32)>> =
    std::sync::LazyLock::new(FxHashMap::default);

/// The direct-construction default for [`Interp::names`]: no lexemes —
/// every invocation head reads as a non-class call.
static NO_NAMES: std::sync::LazyLock<Vec<Box<str>>> = std::sync::LazyLock::new(Vec::new);

/// The direct-construction default for [`Interp::closed`]: no closed
/// declarations — every always-exhaustive verdict answers `false` (the
/// pre-mount behavior).
static NO_CLOSED: std::sync::LazyLock<crate::ty::ClosedNames> =
    std::sync::LazyLock::new(crate::ty::ClosedNames::default);

/// The resolution of one bare-name reference: a local variable id (the
/// scope chain's answer — promotion and definite-assignment state track
/// it) or a top-level declaration face (no flow state; the read row
/// carries the written type).
pub(crate) enum Resolved {
    Local(u32),
    Top(crate::record::TopVar),
}

impl<'o> Interp<'o> {
    pub(crate) fn new(
        ops: &'o dyn TypeOps,
        args: &'o [Vec<crate::ty::ArgTy>],
        boundaries: &'o [Option<crate::ty::Boundary>],
    ) -> Self {
        Self {
            ops,
            args,
            boundaries,
            closed: &NO_CLOSED,
            top_level: &NO_TOP_LEVEL,
            inferred_locals: &NO_INFERRED_LOCALS,
            staged_rhs: &NO_STAGED_RHS,
            staged_this: &NO_STAGED_THIS,
            staged_calls: &NO_STAGED_CALLS,
            names: &NO_NAMES,
            reach: ReachArena::new(),
            lists: Lists::default(),
            reasons: Reasons::default(),
            non_promotions: vec![Vec::new()],
            current: State::new(0),
            stack: Vec::new(),
            exprs: Vec::new(),
            scopes: vec![Scope {
                bindings: FxHashMap::default(),
                scope_id: 0,
            }],
            vars: Vec::new(),
            facts: Vec::new(),
            props: Vec::new(),
            scope_gen: 1,
            coll_if: Vec::new(),
            binding_journal: Vec::new(),
            cond_split_var: None,
            cond_vars: std::collections::HashMap::new(),
            local_fns: FxHashSet::default(),
            prop_slots: FxHashMap::default(),
            decl: DeclCtx::default(),
            decl_stack: Vec::new(),
            pending_ty: None,
            pending_writes: Vec::new(),
            pending_inits: Vec::new(),
            pending_decl_bases: Vec::new(),
            pattern: None,
            pattern_depth: 0,
            or_pattern_latch: None,
            or_operand_depth: None,
            or_right_face: None,
            type_test_pending: None,
            as_cast_pending: None,
            tested_ty: None,
            tested_args: crate::ty::EMPTY_ARGS,
            as_cast_args: crate::ty::EMPTY_ARGS,
            pending_ty_args: crate::ty::EMPTY_ARGS,
            in_decl: false,
            in_type_test: false,
            in_as_cast: false,
            for_header_depth: 0,
            header_cond_depth: 0,
            anywhere: Effects::default(),
            dead: None,
            stmt_began_dead: false,
            exit_at: None,
            if_null_skips: Vec::new(),
            vacuous_object: None,
            vacuous_fields: Vec::new(),
            pattern_binds: Vec::new(),
            init_exec_depth: None,
            null_shorts: Vec::new(),
            args_bases: Vec::new(),
            collect: None,
            snapshot: None,
        }
    }

    /// Runs one executable's buffer: the pre-pass collect, then the real
    /// replay.
    pub(crate) fn run(&mut self, events: &[Ev]) {
        // A resolved back-end builds its `Base::Name` bindings from the
        // recorder's lexeme table before the first question arrives — the
        // incremental scan costs one bounds check for a name-level impl
        // (the default no-op) and one forward slice for a mounted one.
        self.ops.observe_names(self.names);
        let match_of = build_match_map(events);
        self.anywhere = self.collect_ranges(events, 0..events.len(), &match_of);
        let mut i = 0;
        while i < events.len() {
            let control = self.step(events, i, &match_of);
            match control {
                Control::Continue => i += 1,
                Control::Jump(to) => i = to,
            }
        }
        // The write-site rows (the semantic query face's family-c):
        // aggregated from the AssignedVariables anywhere sets —
        // `written_direct` holds the writes outside nested closures,
        // `captured` the writes inside them — one row per (declaration,
        // in-closure) pair, so a variable written both ways emits both
        // rows (`written` itself carries the closure-merged shape the
        // demotion faces need). Declaration initializers and for-in loop
        // variables never reach the sets (they write through
        // [`Self::apply_write`], the official sets do not count them —
        // the `prefer_final_locals` premise).
        // Emitted after the replay: the ids the pre-pass recorded are the
        // replay's own (the declaration sequence is deterministic), so the
        // metadata lookup cannot name the wrong variable. Stale ids
        // (collect-rollback residue, the `cjoin` live filter's concern)
        // and declarations without a name span emit nothing. Ids sort
        // first because the take_facts sort is stable and equal
        // decl-offset keys would otherwise order by hash-set iteration —
        // nondeterministic.
        let mut ids: Vec<u32> = self.anywhere.written_direct.iter().copied().collect();
        ids.sort_unstable();
        for &var in &ids {
            if let Some(span) = self.vars.get(var as usize).and_then(|meta| meta.decl_span) {
                self.emit(FlowFact::WriteSite {
                    decl_name_span: span,
                    in_closure: false,
                });
            }
        }
        ids.clear();
        ids.extend(self.anywhere.captured.iter().copied());
        ids.sort_unstable();
        for var in ids {
            if let Some(span) = self.vars.get(var as usize).and_then(|meta| meta.decl_span) {
                self.emit(FlowFact::WriteSite {
                    decl_name_span: span,
                    in_closure: true,
                });
            }
        }
        // The declaration exit for the OUTERMOST executable: the recorder
        // records no trailing ExecEnd for it (the pipeline runs from its
        // own `exec_end`), so the close covering every interval still open
        // fires at the end of the buffer. Nested executables closed their
        // windows at their recorded ExecEnd.
        self.flow_end(Covered::Exec);
    }

    // —— variables ——————————————————————————————————————————————————————

    /// Declares a variable, records the effect, binds the name, and
    /// initializes the flow model. `initialized` mirrors the official
    /// `declare(initialized:)`: locals with an initializer (or `late`
    /// locals, or parameters) start assigned; plain locals without one
    /// start unassigned.
    pub(crate) fn declare_var(&mut self, name: u32, meta: VarMeta, initialized: bool) -> u32 {
        let var = saturate_slot_u32(self.vars.len());
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

    /// Pushes a fresh scope with the next generation id — the single
    /// allocation point so every pushed scope's `scope_id` stays unique
    /// over the whole interp lifetime.
    pub(crate) fn push_scope(&mut self) {
        let scope_id = self.scope_gen;
        self.scope_gen = self.scope_gen.wrapping_add(1);
        self.scopes.push(Scope::new(scope_id));
    }

    pub(crate) fn bind_name(&mut self, name_id: u32, var: u32) {
        let Some(depth) = self.scopes.len().checked_sub(1) else {
            return;
        };
        if let Some(scope) = self.scopes.last_mut() {
            let scope_id = scope.scope_id;
            let prev = scope.bindings.insert(name_id, var);
            self.binding_journal.push(BindingUndo {
                depth,
                scope_id,
                name: name_id,
                prev,
            });
        }
    }

    /// The innermost scope's current binding of a name — what a fresh
    /// declaration of the same spelling is about to shadow.
    pub(crate) fn bound_var(&self, name_id: u32) -> Option<u32> {
        self.scopes
            .last()
            .and_then(|scope| scope.bindings.get(&name_id).copied())
    }

    /// Restores the bindings a construct's pattern variables shadowed
    /// (newest first): the construct's scope ended, and the spellings read
    /// their outer declarations again.
    pub(crate) fn restore_bindings(&mut self, shadowed: &[(u32, Option<u32>)]) {
        let Some(scope) = self.scopes.last_mut() else {
            return;
        };
        for (name_id, prev) in shadowed.iter().rev() {
            match prev {
                Some(var) => {
                    scope.bindings.insert(*name_id, *var);
                }
                None => {
                    scope.bindings.remove(name_id);
                }
            }
        }
    }

    /// The official lexical lookup's order (upstream Dart SDK source,
    /// pkg/analyzer/lib/src/dart/resolver/property_element_resolver.dart:2137-2139):
    /// the scope chain innermost-first, then the unit's own top-level
    /// declarations as the outermost scope frame. Only a full miss falls to
    /// the caller's implicit-`this` face.
    pub(crate) fn resolve(&self, name_id: u32) -> Option<Resolved> {
        for scope in self.scopes.iter().rev() {
            if let Some(&var) = scope.bindings.get(&name_id) {
                return Some(Resolved::Local(var));
            }
        }
        self.top_level.get(&name_id).copied().map(Resolved::Top)
    }

    /// Records a write (real or collect mode) and updates the flow state.
    /// `promote_to_interest` is false for compound and unknown-type
    /// writes. `write_span` is the write expression's own range — the
    /// official `DemoteViaExplicitWrite.node` anchor
    /// (`assignment_expression_resolver.dart:324,533,698`,
    /// `increment_or_decrement_resolver.dart:339`,
    /// `for_resolver.dart:236`); `None` for the write shapes the official
    /// passes no node to (pattern assignments, the synthetic-degraded
    /// faces) — their demotions record no reason link.
    pub(crate) fn write_var(
        &mut self,
        var: u32,
        written: Ty,
        written_args: u32,
        promote_to_interest: bool,
        write_span: Option<SourceRange>,
        literal_rhs: bool,
    ) {
        self.record_write(var);
        self.apply_write(
            var,
            written,
            written_args,
            promote_to_interest,
            write_span,
            literal_rhs,
        );
    }

    /// The state half of [`Self::write_var`], without the
    /// AssignedVariables record. Per-iteration initializations — a
    /// declaration's own initializer and a for-in loop variable — move
    /// the flow state but are not writes in the official sets
    /// (`prefer_final_locals` treats the variables as never assigned), so
    /// they pass `None`: the official `declare`/`_initialize` carry no
    /// `nonPromotionReason` (`flow_analysis.dart:4914-4916`).
    /// `written_args` is the written type's arena index — the demotion's
    /// instantiated-spelling compares read it (#2204 phase D).
    pub(crate) fn apply_write(
        &mut self,
        var: u32,
        written: Ty,
        written_args: u32,
        promote_to_interest: bool,
        write_span: Option<SourceRange>,
        literal_rhs: bool,
    ) {
        // A stale id (a collect rollback can strand effect-set entries and
        // scope bindings past their variable) degrades to the unknown
        // declared type; the state write itself is model-bounds-safe.
        let (declared, declared_args) = match self.vars.get(var as usize) {
            Some(meta) => (meta.ty, meta.args),
            None => (Ty::UNKNOWN, crate::ty::EMPTY_ARGS),
        };
        self.current = self.current.clone().write(
            self.ops,
            &mut self.lists,
            &mut self.reasons,
            var,
            declared,
            declared_args,
            written,
            written_args,
            promote_to_interest,
            literal_rhs,
            write_span,
            self.args,
            self.names,
        );
        // The receiver's value version changed (official `write` allocates
        // a fresh one), so its property promotions do not carry over
        // (#1409): every property slot keyed on this receiver resets. No
        // finality exception — an actual write bumps the version even for
        // a `late final` (official `PromotionModel.write` checks no
        // finality, flow_analysis.dart:4802-4866), unlike the conservative
        // join's finals rule (upstream Dart SDK source,
        // pkg/_fe_analyzer_shared/lib/src/flow_analysis/flow_analysis.dart:3799-3813).
        let mut written_one = FxHashSet::default();
        written_one.insert(var);
        let no_finals = FxHashSet::default();
        sweep_state_props(
            &mut self.current,
            &self.props,
            &written_one,
            &FxHashSet::default(),
            &no_finals,
            &self.lists,
            &mut self.reasons,
        );
    }

    /// The property-key slot of `(receiver, field)`, allocated on first
    /// use (#1409). Slot order is allocation order — every [`State::props`]
    /// index is one of these slots.
    pub(crate) fn prop_slot(&mut self, receiver: u32, field: u32) -> u32 {
        if let Some(&slot) = self.prop_slots.get(&(receiver, field)) {
            return slot;
        }
        let slot = saturate_slot_u32(self.props.len());
        self.props.push((receiver, field));
        self.prop_slots.insert((receiver, field), slot);
        slot
    }

    /// Whether property lookups on `receiver` are dead because the
    /// receiver is write-captured (official `_handleProperty`'s
    /// target-version gate via `target._getVersion() == null`,
    /// `flow_analysis.dart:9021-9026` — a captured receiver's value version
    /// association is permanently gone). `this` is never captured.
    pub(crate) fn prop_target_captured(&self, receiver: u32) -> bool {
        receiver != THIS_RECEIVER && self.current.model(receiver).captured()
    }

    /// The property slot's standing promotion: `Some` when a non-null
    /// promotion chain stands and neither the receiver nor the slot is
    /// write-captured.
    pub(crate) fn prop_promoted(&self, receiver: u32, slot: u32) -> bool {
        !self.prop_target_captured(receiver)
            && !self.current.prop(slot).captured()
            && self.current.prop(slot).promoted != crate::model::NIL
    }

    // —— effect recording (collect mode) —————————————————————————————————

    pub(crate) fn record_write(&mut self, var: u32) {
        if let Some(frames) = self.collect.as_mut() {
            for frame in frames.iter_mut() {
                frame.written.insert(var);
            }
            // The direct-write view records at the innermost frame only: a
            // closure-body write is a capture of the enclosing scopes, not
            // one of their direct writes, and dies with the closure's own
            // frame at the pop.
            if let Some(frame) = frames.last_mut() {
                frame.written_direct.insert(var);
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

    /// Whether any control path reaches the current state (official
    /// `isReachable` = `overallReachable`, `flow_analysis.dart:6396`) — the
    /// diagnostic face: dead-code anchors, unassigned-read rows, promotion
    /// rows, and body-completion rows all query it. Splits resurrect the
    /// LOCAL flag (official `Reachability.split`), never this one; the
    /// local flag's readers (`switch_afterCase`'s break join,
    /// `attachFinally`, the do-while break approximation) call
    /// [`ReachArena::locally_reachable`] directly.
    pub(crate) fn reachable(&self) -> bool {
        self.reach.overall_reachable(self.current.reach)
    }

    /// Splits the current state on a condition expression (official
    /// `FlowModel` branch machinery): explicit branch states win, then
    /// boolean literals, otherwise both branches inherit the state.
    pub(crate) fn cond_split(&mut self, expr: &Expr) -> (State, State) {
        if let Some((if_true, if_false)) = &expr.branches {
            return (if_true.clone(), if_false.clone());
        }
        // The condition-variable replay (language#1274, official
        // `restoreConditionVariableState`): a bare read of a bool local
        // bound from a null-comparison/type-test split re-derives its
        // arms — the tested variable's saved models rebased onto the
        // CURRENT state (an intervening write invalidates via the
        // version check).
        if let Some(var) = expr.reference
            && let Some(entry) = self.cond_vars.get(&var)
            && self.current.model(entry.tested).version == entry.version
        {
            let t = self
                .current
                .clone()
                .with_model(entry.tested, entry.if_true.model(entry.tested));
            let f = self
                .current
                .clone()
                .with_model(entry.tested, entry.if_false.model(entry.tested));
            return (t, f);
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
        let state = State::join(
            first,
            second,
            &mut self.reach,
            &mut self.lists,
            self.vars.len(),
        );
        // A join that revives the flow ends a latched Never-exit's anchoring
        // duty (official `handleExit` kills only its own region; the branch
        // rejoin's live state owns everything after it). Without the
        // retirement the next flow-killing statement self-anchors on the
        // stale exit (`if (c) { never(); } throw 'x';` anchored its own
        // throw — oracle-clean).
        if self.reach.overall_reachable(state.reach) {
            self.exit_at = None;
        }
        state
    }

    /// Splits the current state in place (official `FlowModel.split`).
    pub(crate) fn split_cur(&mut self) {
        self.current = self.current.clone().split(&mut self.reach);
    }

    /// Marks the current state unreachable in place.
    pub(crate) fn unreachable_cur(&mut self) {
        self.current = self.current.clone().set_unreachable(&mut self.reach);
    }

    /// Opens a null-shorting context over `receiver` (official
    /// `_nullAwareAccess_rightBegin`, flow_analysis.dart:9199-9257):
    /// splits the current state, keeps the pre-promotion state as the skip
    /// path, and promotes the receiver to non-null on the access path.
    /// A never-null receiver makes the skip path unreachable (official
    /// `_isGuaranteedNonNullWithSoundNullSafety`, :9217-9221); a
    /// null-typed receiver makes the access path unreachable (:9214-9216).
    /// The receiver's matched state is kept beside the shortcut so the
    /// access's own event can still feed the condition face
    /// (`Expr::branches`) the pair it reported before this machinery.
    ///
    /// The caller hands over the receiver's `(reference, ty, args)`
    /// triple — read off the expression stack — rather than the `Expr`
    /// itself, so the call holds no borrow into `self`.
    pub(crate) fn null_short_begin(&mut self, receiver: (Option<u32>, Ty, u32)) {
        self.split_cur();
        let mut shortcut = self.current.clone();
        let mut matched = None;
        let (reference, ty, args) = receiver;
        if let Some(var) = reference {
            let (promoted, promoted_args) = self.promote_to_non_null(ty, args);
            let next = self.current.clone().finish_type_test(
                self.ops,
                &mut self.lists,
                var,
                None,
                Some((promoted, promoted_args)),
                self.args,
                self.names,
            );
            self.current = next;
            matched = Some(self.current.clone());
        }
        match self.classify(ty) {
            Classification::NullOrEquivalent => self.unreachable_cur(),
            Classification::NonNullable => {
                shortcut = shortcut.set_unreachable(&mut self.reach);
            }
            _ => {}
        }
        self.null_shorts.push(NullShort {
            shortcut,
            matched,
            exprs_depth: self.exprs.len(),
            dead_at_open: !self.reachable(),
        });
    }

    /// Closes one open null-shorting context (official
    /// `nullAwareAccess_end`, flow_analysis.dart:7400-7404): joins the
    /// access path into the skip path and drops the checkpoint.
    pub(crate) fn null_short_end(&mut self) -> Option<NullShort> {
        let short = self.null_shorts.pop()?;
        self.current = self
            .join(Some(self.current.clone()), Some(short.shortcut.clone()))
            .unsplit(&mut self.reach);
        Some(short)
    }

    /// Ends the null-shorting contexts opened inside the operand region the
    /// caller is closing — `exprs_base` is the value-stack depth at that
    /// operand's start. Only shorts opened strictly inside (`exprs_depth >
    /// base`) close: the chain's own `?` (`x?.m(g()?.n)`'s outer short
    /// beside an argument's) ends at its access event, not here — the
    /// official `nullShortingTargetDepth` bound (type_analyzer.dart:804-827)
    /// mapped onto the event stream.
    pub(crate) fn flush_null_shorts_above(&mut self, exprs_base: usize) {
        while self
            .null_shorts
            .last()
            .is_some_and(|short| short.exprs_depth > exprs_base)
        {
            self.null_short_end();
        }
    }

    /// The operand-region bound for a consumer about to eat the top `arity`
    /// rows, chain-aware: a null-shorted result row's [`Expr::chain_base`]
    /// drags the bound down to the chain's receiver index, so
    /// `x?.m(throw _)`'s own `?.` short joins at this consumer (official
    /// `finishNullShorting` runs at the consumed expression's boundary) while
    /// `x?[throw _]`'s — whose `throw` sits strictly inside the operand —
    /// survives for the access's own close. Compute BEFORE the pops the
    /// consumer performs: the rows it reads are the ones being consumed.
    pub(crate) fn consumer_base(&self, arity: usize) -> usize {
        let top = self.exprs.len().saturating_sub(arity);
        self.exprs[top.min(self.exprs.len())..]
            .iter()
            .enumerate()
            .filter_map(|(i, e)| e.chain_base.map(|b| b.min(top + i)))
            .min()
            .unwrap_or(top)
    }

    /// The branch pair a null-aware access contributes to a condition
    /// split: the innermost open context's (non-null, skip) halves, or
    /// `None` when no `?` opened for this receiver.
    pub(crate) fn null_short_branches(&mut self) -> Option<(State, State)> {
        let short = self.null_shorts.last()?;
        Some((
            short
                .matched
                .clone()
                .unwrap_or_else(|| self.current.clone()),
            short.shortcut.clone(),
        ))
    }

    /// Boundary-aware nullability classification — the name-level stand-in
    /// for the official `classifyType` subtype question (upstream Dart SDK
    /// source, pkg/analyzer/lib/src/dart/resolver/flow_analysis_visitor.dart:615-632).
    /// A `?`-less type-parameter reference resolves through its declared
    /// bound ([`crate::ty::Boundary`]): `T extends Object?` classifies
    /// potentially-nullable where the written spelling alone would say
    /// non-nullable. A nullable-written reference (`T?`) and every
    /// non-parameter name fall through to the syntactic
    /// [`TypeOps::classify`].
    pub(crate) fn classify(&self, t: Ty) -> Classification {
        if !t.nullable
            && let Base::Name(id) = t.base
            && let Some(boundary) = self.boundaries.get(id as usize).copied().flatten()
        {
            return boundary.class;
        }
        self.ops.classify(t)
    }

    /// Boundary-aware promote-to-non-null: the promoted type beside its
    /// written-argument tree (official `tryMarkNonNullable` +
    /// `promoteToNonNull`). A boundary-potentially-nullable parameter
    /// promotes to its bound's non-null face; a boundary whose non-null
    /// face is the bound itself (`dynamic`, `FutureOr<void>` — the
    /// identical-bound collapse, pkg/analyzer/lib/src/dart/element/type_system.dart:1467-1495)
    /// returns the input unchanged, like the syntactic path.
    pub(crate) fn promote_to_non_null(&self, t: Ty, args: u32) -> (Ty, u32) {
        if !t.nullable
            && let Base::Name(id) = t.base
            && let Some(boundary) = self.boundaries.get(id as usize).copied().flatten()
        {
            return boundary
                .target
                .map_or((t, args), |(ty, target_args)| (ty, target_args));
        }
        let promoted = self.ops.promote_to_non_null(t);
        // The opaque-declared face's marker step (#2243, centralized from
        // the equality path — every refinement site shares it): the
        // non-null face of the unresolved-nullable marker is the plain
        // unknown, so a `!`/`?.`-receiver/`??`-LHS refinement on a local
        // whose declared type the fold adopted as the marker refines
        // exactly like a null check does (#2241).
        if promoted == t && t == Ty::UNKNOWN_NULLABLE {
            return (Ty::UNKNOWN, crate::ty::EMPTY_ARGS);
        }
        (promoted, args)
    }

    /// Name-level least upper bound (official `typeOperations.join`): the
    /// subtype-ordered join — `a <: b` gives `b`, `b <: a` gives `a`, and a
    /// name-unordered pair joins to the unknown face (the real hierarchy's
    /// LUB is the resolved back-end's question, not the name layer's). An
    /// unknown operand is the official's InvalidType — the subtype check
    /// answers `InvalidType <: X` true for every `X`
    /// (subtype.dart:274), so the join lands on the spelled side
    /// (`LUB(InvalidType, String)` = `String`) instead of the unknown
    /// face the plain `is_subtype` order would keep.
    pub(crate) fn name_lub(&self, a: Ty, b: Ty) -> Ty {
        // The oracle's LUB ignores an invalid operand wholesale
        // (upstream least_upper_bound: the invalid side is dropped, never
        // joined — `LUB(InvalidType, X) = X`): a spelling the resolved
        // back-end maps invalid loses to any other face, so the join
        // lands on the spelled side, or falls to the unknown face for the
        // semantic layer's staged type to fill. Keeping the invalid name
        // instead typed every `?? `-joined inferred local with the
        // invalid face and deaded switch arms the oracle keeps live.
        let a_invalid =
            self.ops
                .is_invalid_typed(a, crate::ty::EMPTY_ARGS, self.args, self.names);
        let b_invalid =
            self.ops
                .is_invalid_typed(b, crate::ty::EMPTY_ARGS, self.args, self.names);
        if a_invalid && !b_invalid {
            return b;
        }
        if b_invalid && !a_invalid {
            return a;
        }
        if a_invalid && b_invalid {
            return Ty::UNKNOWN;
        }
        if a.base == Base::Unknown {
            return b;
        }
        if b.base == Base::Unknown {
            return a;
        }
        // The official LUB joins the nullability suffix (`least_upper_bound`
        // over the nullability-aware lattice): `LUB(Null, T) = T?` — a
        // switch-expression arm folding `_ => null` with instance arms must
        // keep the expression nullable, or a `?? ` over the assigned value
        // folds its right side dead.
        let either_null = a.base == Base::Null || b.base == Base::Null;
        let nullable_join = a.nullable || b.nullable || either_null;
        let with_join = |mut ty: Ty| -> Ty {
            if ty.base != Base::Null {
                ty.nullable = ty.nullable || nullable_join;
            }
            ty
        };
        if a.base == Base::Null && b.base == Base::Null {
            return a;
        }
        if self.ops.is_subtype(a, b) {
            return with_join(b);
        }
        if self.ops.is_subtype(b, a) {
            return with_join(a);
        }
        Ty::UNKNOWN
    }

    /// Whether the interned name spells a class head — a leading underscore
    /// skipped, an uppercase first letter (`C`, `_Private`): the
    /// implicit-creation face (`x = C()`) keys on it, since a lowercase
    /// head is a function/getter call whose return this layer cannot spell.
    pub(crate) fn is_class_head(&self, name: u32) -> bool {
        self.names
            .get(name as usize)
            .is_some_and(|spelling| {
                spelling
                    .trim_start_matches('_')
                    .starts_with(|c: char| c.is_ascii_uppercase())
            })
    }

    /// Whether the send about to consume its two arguments is a bare
    /// `identical(a, b)` call: the callee three slots below the argument
    /// pair is an unresolved bare head spelling `identical` — not a
    /// variable read (a local callable named `identical` carries its
    /// reference) and not a member/prefixed send (those carry no bare
    /// head name). The [`Interp::equality_check`] face applies
    /// (upstream Dart SDK source,
    /// pkg/analyzer/lib/src/dart/resolver/invocation_inferrer.dart
    /// :773-785: the resolved dart:core top-level `identical` with
    /// exactly two arguments).
    pub(crate) fn send_callee_is_core_identical(&self) -> bool {
        let Some(callee) = self
            .exprs
            .len()
            .checked_sub(3)
            .and_then(|index| self.exprs.get(index))
        else {
            return false;
        };
        callee.reference.is_none()
            && callee.prop.is_some()
            && callee.name.is_some_and(|name| {
                self.names
                    .get(name as usize)
                    .is_some_and(|s| &**s == "identical")
            })
    }

    /// Official `_typesAreDisjointDueToNullability`: one side is
    /// null-or-equivalent (`classifyType` → nullOrEquivalent) while the
    /// other is non-nullable — every value of either lies outside the
    /// other, so a type check between them always fails.
    pub(crate) fn disjoint_by_nullability(&self, a: Ty, b: Ty) -> bool {
        let (a_class, b_class) = (self.classify(a), self.classify(b));
        (a_class == Classification::NullOrEquivalent && b_class == Classification::NonNullable)
            || (b_class == Classification::NullOrEquivalent
                && a_class == Classification::NonNullable)
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

    /// The number of executable frames on the stack — the face the
    /// initializer bracket snapshots ([`Interp::init_exec_depth`]) to
    /// detect a cast directly inside the initializer versus inside a
    /// nested executable.
    pub(crate) fn exec_frame_depth(&self) -> usize {
        self.stack
            .iter()
            .filter(|frame| matches!(frame, Frame::Exec { .. }))
            .count()
    }

    /// The first unreachable node opens the dead-code interval (official
    /// `_visitNode`): every later unreachable node continues the same
    /// interval — no row of its own — and the interval's end tracks the
    /// furthest statement end so the close can trim the row to the
    /// covering list's last statement, its trailing `;` included
    /// (official `Block.statements.last` + `node.end`). Reachable nodes
    /// keep the interval open: only a covering close retires it.
    pub(crate) fn dead_code(&mut self, span: TokenSpan) {
        if self.reachable() {
            return;
        }
        let end = span.range.end();
        if let Some(window) = &mut self.dead {
            if end > window.end {
                window.end = end;
            }
            if window.split.is_some() && window.split_start.is_none() {
                window.split_start = Some(span.range.offset);
            }
            return;
        }
        self.dead = Some(DeadWindow::at(self.stack.len(), span));
    }

    /// Opens the interval for a dead do-while back edge (official: the
    /// do-while's condition is the first dead node and its parent is the
    /// `DoStatement`, so the close splits into the `while` part and the
    /// tail). Only when no interval is open: an earlier anchor continues
    /// to own the reporting.
    pub(crate) fn dead_do_condition(&mut self) {
        if self.dead.is_none() && !self.reachable() {
            self.dead = Some(DeadWindow::awaiting_do(self.stack.len()));
        }
    }

    /// The collection-if arm's own `flowEnd` (official
    /// `flowEnd(ifTrue)` / `flowEnd(ifFalse)`, dead_code_verifier.dart:217-260):
    /// the arm node contains the first dead node — the walk entered the
    /// arm unreachable, so the first dead node IS the arm element — and
    /// the row runs from the arm's first token to the arm's end, then
    /// resets. This re-anchors past an interval whose poll opened inside
    /// the arm (the event stream is post-order, so an inner poll can sit
    /// later in the arm than the official's top-down first-dead node); an
    /// interval opened BEFORE the arm keeps ownership — the arm only
    /// extends its end, exactly `dead_expr_arm`'s fold.
    pub(crate) fn dead_arm_row(&mut self, arm_start: u32, arm_end: u32) {
        if self.reachable() {
            return;
        }
        if let Some(window) = &mut self.dead {
            // An empty arm range is a recovery artifact (no arm span was
            // recorded): it can neither re-anchor nor report, so any open
            // interval keeps ownership with the arm as a no-op.
            if arm_end <= arm_start || window.anchor.range.offset < arm_start {
                window.extend(arm_end);
                return;
            }
        }
        self.dead = None;
        if arm_end > arm_start {
            self.emit(FlowFact::DeadCode {
                span: TokenSpan {
                    id: 0,
                    range: SourceRange {
                        offset: arm_start,
                        length: arm_end - arm_start,
                    },
                },
            });
        }
    }

    /// Closes the dead-code interval at a construct end (official
    /// `flowEnd`). `Covered::Strict` closes only when the anchor sits
    /// strictly inside the construct that ends here; `Covered::Body` also
    /// accepts an anchor at the closing depth (a switch case body has no
    /// frame of its own); `Covered::Exec` closes unconditionally (the
    /// declaration exit covers everything, trimmed to the body's last
    /// statement). A do-while split emits the `while (c);` part first and
    /// the tail from the first dead statement after the do-while, the
    /// tail dropped when a break makes it reachable.
    pub(crate) fn flow_end(&mut self, covered: Covered) {
        let Some(window) = self.dead else {
            return;
        };
        let covers = match covered {
            Covered::Strict => window.depth > self.stack.len(),
            Covered::Body => window.depth >= self.stack.len(),
            Covered::Exec => true,
        };
        self.close_covered(covers);
    }

    /// The C-style `for`'s body close (official `visitForStatement`'s
    /// `flowEnd(node.body)`): `Covered::Strict` containment plus an anchor
    /// AT the loop's own depth when it sits past the header end — the body
    /// block's `{` and a non-block body statement carry the loop's frame
    /// depth (the body keeps no frame of its own), while the condition's
    /// anchor (same depth, before the separator) is not a body descendant
    /// (dead_code_verifier.dart:453-459).
    fn flow_end_body(&mut self, header_end: u32) {
        let Some(window) = self.dead else {
            return;
        };
        let covers = window.depth > self.stack.len()
            || (window.depth == self.stack.len() && window.anchor.range.offset > header_end);
        self.close_covered(covers);
    }

    /// Emits and clears the open dead-code interval when `covers` holds
    /// (the shared tail of the close gates): a do-while split emits the
    /// `while (c);` part first and the tail second, anything else one row
    /// from the anchor to the interval's end. A suppressed interval
    /// (whose row the semantic as-cast tail face reports) retires
    /// without emitting.
    fn close_covered(&mut self, covers: bool) {
        if !covers {
            return;
        }
        let Some(window) = self.dead.take() else {
            return;
        };
        if window.suppressed {
            return;
        }
        if let Some(while_part) = window.split {
            self.emit(FlowFact::DeadCode {
                span: TokenSpan {
                    id: window.anchor.id,
                    range: while_part,
                },
            });
            if !window.split_broken
                && let Some(start) = window.split_start
                && window.end > start
            {
                self.emit(FlowFact::DeadCode {
                    span: TokenSpan {
                        id: window.anchor.id,
                        range: SourceRange {
                            offset: start,
                            length: window.end - start,
                        },
                    },
                });
            }
        } else if window.end > window.anchor.range.offset {
            self.emit(FlowFact::DeadCode {
                span: TokenSpan {
                    id: window.anchor.id,
                    range: SourceRange {
                        offset: window.anchor.range.offset,
                        length: window.end - window.anchor.range.offset,
                    },
                },
            });
        }
    }

    /// Reads of definitely-unassigned locals (official `isUnassigned`). A
    /// stale id (a scope binding stranded past its variable by a collect
    /// rollback) emits no fact: the fact's metadata would name the wrong
    /// variable. The row emits under `!assigned` — the union of the two
    /// consumer gates: upstream's `isDefinitelyUnassigned` (the
    /// `unassigned` bit — the late-local and cast faces) is narrower than
    /// the non-late reads' `!isAssigned` (the potentially-unassigned join
    /// face); `definitely_unassigned` carries the bit so the consumer
    /// picks its own gate.
    pub(crate) fn check_unassigned_read(&mut self, span: TokenSpan, var: u32) {
        let Some(meta) = self.vars.get(var as usize) else {
            return;
        };
        let model = self.current.model(var);
        if self.reachable() && !model.assigned {
            self.emit(FlowFact::UnassignedRead {
                span,
                is_late: meta.late,
                is_final: meta.is_final,
                definitely_unassigned: model.unassigned,
                declared_type: meta.decl_span,
            });
        }
    }

    /// The condition-operand query row (official `checkForNonBoolCondition`'s
    /// flow half — upstream Dart SDK source,
    /// pkg/analyzer/lib/src/error/bool_expression_verifier.dart:45-73): the
    /// flow type at the operand's evaluation point, when the operand value
    /// is a promotable reference read (its read anchor travels with the
    /// value). Other operand shapes — calls, property accesses, `is`-tests,
    /// assignments — carry no flow-visible type and emit no row; their
    /// static type is the type layer's question. No reachability gate: the
    /// official verdict is a type question and fires in dead code too.
    pub(crate) fn condition_operand(&mut self, cond: &Expr) {
        if let (Some(span), Some(_)) = (cond.read_span, cond.reference) {
            self.emit(FlowFact::ConditionType {
                span,
                ty: cond.ty,
                args: cond.args,
            });
        }
    }

    /// The scrutinee's flow type under a switch frame's match-time chain
    /// state (official `getMatchedValueType`,
    /// flow_analysis.dart:8753-8760: the current promotion model's head
    /// for the matched value's promotion key, falling back to the matched
    /// value's declared type). `None` when the scrutinee is not a
    /// promotable reference read — its static type is then the type
    /// layer's answer.
    pub(crate) fn matched_type_at(&self, frame_index: usize) -> Option<(Ty, u32)> {
        let Frame::Switch {
            scrutinee: Some(var),
            unmatched,
            ..
        } = &self.stack[frame_index]
        else {
            return None;
        };
        let model = unmatched.model(*var);
        Some((
            self.promoted_type_of(*var, &model),
            self.promoted_args_of(*var, &model),
        ))
    }

    /// `TypeSystem.isAlwaysExhaustive` (type_system.dart:846-888), delegated
    /// to the mounted ops first: a resolved back-end answers through the
    /// declared-type hierarchy (sealed classes and enums close cross-file,
    /// `bool`/`FutureOr` by their core identity). The name-level fallback
    /// mirrors the official decision order over the recorder's
    /// declaration capture ([`ClosedNames`]): `Null` and `bool` split
    /// completely, enums and sealed classes are closed by their element
    /// faces, `FutureOr<T>` recurses into its argument, and a type
    /// parameter recurses into its bound (the official
    /// `TypeParameterTypeImpl` arm, :868-877) — `T extends St` over the
    /// enum `St` splits completely, so a full-coverage switch over `T`
    /// leaves the post-switch state dead. The bound is the *declared* one
    /// ([`crate::ty::Boundary::bound`]): the promoted-bound half of that
    /// arm is not modelled, which keeps the verdict a conservative subset
    /// (a promotion the level cannot see answers `false`). Extension types
    /// (erasure recursion), records, and function types stay undecidable
    /// at this level — `false`, the conservative arm: a missed verdict
    /// keeps the post-switch state live (the pre-mount behavior), never a
    /// false dead report. Nullability is agnostic exactly like the
    /// official's `InterfaceType` arm — `S?` of a sealed `S` is still
    /// always-exhaustive (the null half is the space model's split, not
    /// this gate's).
    pub(crate) fn is_always_exhaustive(&self, ty: Ty, args: u32) -> bool {
        self.exhaustive_within(ty, args, BOUND_RECURSION_LIMIT)
    }

    /// [`Interp::is_always_exhaustive`] with a recursion budget. The
    /// official recurses unguarded over resolved elements, whose bounds
    /// the type system keeps well-founded; the recorder's table is
    /// spelling-level, so an ill-formed `T extends U, U extends T` (which
    /// the oracle rejects as `type_parameter_supertype_of_its_bound`)
    /// would cycle. The budget makes the walk structurally terminating —
    /// an exhausted chain answers `false`, the conservative arm.
    fn exhaustive_within(&self, ty: Ty, args: u32, budget: u8) -> bool {
        if budget == 0 {
            return false;
        }
        if self
            .ops
            .is_always_exhaustive(ty, args, self.args, self.names)
        {
            return true;
        }
        let Base::Name(id) = ty.base else {
            return matches!(ty.base, Base::Null);
        };
        if self.closed.names.contains(&id) || self.closed.bool_name == Some(id) {
            return true;
        }
        if self.closed.future_or == Some(id)
            && let Some(row) = self.args.get(args as usize)
            && let [arg] = row.as_slice()
        {
            return self.exhaustive_within(
                Ty {
                    base: arg.base,
                    nullable: arg.nullable,
                },
                arg.args,
                budget - 1,
            );
        }
        // The `TypeParameterTypeImpl` arm (:868-877): a name that captured
        // a declared bound at parse time is a type parameter, and the
        // verdict is its bound's.
        self.boundaries
            .get(id as usize)
            .copied()
            .flatten()
            .is_some_and(|boundary| {
                let (bound, bound_args) = boundary.bound;
                self.exhaustive_within(bound, bound_args, budget - 1)
            })
    }

    // —— collect runs ——————————————————————————————————————————————————

    /// Runs the event ranges once in collect mode and returns the bottom
    /// effect frame. All flow-state mutations are rolled back and fact
    /// emission is suppressed. The real match table is passed through so
    /// extent handlers behave identically in collect runs (they read
    /// `match_of` before their collect guards only by convention — the
    /// empty table removed that fragile invariant).
    pub(crate) fn collect_ranges(
        &mut self,
        events: &[Ev],
        range: Range<usize>,
        match_of: &[u32],
    ) -> Effects {
        let snapshot = Snapshot {
            current: self.current.clone(),
            exprs_len: self.exprs.len(),
            frames_len: self.stack.len(),
            scopes_len: self.scopes.len(),
            vars_len: self.vars.len(),
            facts_len: self.facts.len(),
            journal_len: self.binding_journal.len(),
            non_promotions_len: self.non_promotions.len(),
            props_len: self.props.len(),
            dead: self.dead,
            init_exec_depth: self.init_exec_depth,
            null_shorts_len: self.null_shorts.len(),
            args_bases_len: self.args_bases.len(),
        };
        let outer = self.collect.replace(vec![Effects::default()]);
        let _ = self.snapshot.replace(snapshot);
        let mut i = range.start;
        while i < range.end {
            match self.step(events, i, match_of) {
                Control::Continue => i += 1,
                // Extent handlers consume wholesale in collect runs too.
                Control::Jump(to) => i = to.max(range.start),
            }
        }
        let frames = self.collect.take().unwrap_or_default();
        // Roll back interpreter state. The snapshot is the rollback's
        // source of truth; a missing one (unbalanced nesting) still
        // restores the enclosing collect before returning.
        let Some(s) = self.snapshot.take() else {
            self.collect = outer;
            return Effects::default();
        };
        self.exprs.truncate(s.exprs_len);
        // Undo the collect walk's name bindings BEFORE the scope
        // truncation: entries whose scope survives (`depth < scopes_len`,
        // generation match) restore the pre-collect binding or remove it
        // entirely; a generation mismatch means the binding's scope was
        // popped mid-walk (a newer scope now sits at that depth — leave
        // it), and a depth past the truncation point dies with its scope.
        while self.binding_journal.len() > s.journal_len {
            let undo = self.binding_journal.pop().expect("journal len checked");
            if let Some(scope) = self.scopes.get_mut(undo.depth) {
                if scope.scope_id == undo.scope_id {
                    match undo.prev {
                        Some(prev) => {
                            scope.bindings.insert(undo.name, prev);
                        }
                        None => {
                            scope.bindings.remove(&undo.name);
                        }
                    }
                }
            }
        }
        self.stack.truncate(s.frames_len);
        self.scopes.truncate(s.scopes_len);
        self.vars.truncate(s.vars_len);
        self.facts.truncate(s.facts_len);
        self.non_promotions.truncate(s.non_promotions_len);
        self.props.truncate(s.props_len);
        self.prop_slots
            .retain(|_, slot| (*slot as usize) < s.props_len);
        self.current = s.current;
        self.dead = s.dead;
        self.init_exec_depth = s.init_exec_depth;
        self.null_shorts.truncate(s.null_shorts_len);
        self.args_bases.truncate(s.args_bases_len);
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
            parent
                .written_direct
                .extend(inner.written_direct.iter().copied());
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

impl Interp<'_> {
    // —— conservative-join plumbing —————————————————————————————————————

    /// The final variables among `written` (promotions on finals survive
    /// conservative joins). Stale ids filter out with the rest.
    pub(crate) fn finals_of(&self, written: &FxHashSet<u32>) -> FxHashSet<u32> {
        written
            .iter()
            .copied()
            .filter(|&var| self.vars.get(var as usize).is_some_and(|m| m.is_final))
            .collect()
    }

    /// The complete conservative join over a detached state: the model
    /// half ([`State::conservative_join`]) plus its property-slot half
    /// ([`sweep_state_props`]) — two halves of one event, paired here so
    /// a call site cannot run the models alone and carry stale property
    /// promotions into the joined region. The sweep's receiver rule: a
    /// receiver in `written` loses its property promotions (its value
    /// version changed — final receivers keep them, the official
    /// conservative-join finals rule, `flow_analysis.dart:3803-3813`); a
    /// receiver in `captured` is permanently write-captured and every
    /// property lookup on it dies (official `conservativeJoin`'s captured
    /// arm plus the `_handleProperty` lookup gate, :3826-3831 — the
    /// official model-level join does not sweep dependent properties
    /// because the lookup re-checks capture; the flat slot form records
    /// the capture on the slots so the join's captured arm propagates
    /// it). The two halves touch disjoint state fields (`models` vs
    /// `props`), so either order yields the same state. `reason` rides to
    /// the model half: `Some` only at suspension points, where each
    /// dropped promotion appends a `DemoteViaSuspension` link (the
    /// official's sole reason-bearing join); every other join passes
    /// `None`, which drops the history outright.
    pub(crate) fn join_state_with_sweep(
        &mut self,
        state: State,
        written: &FxHashSet<u32>,
        captured: &FxHashSet<u32>,
        reason: Option<crate::facts::NonPromotionCause>,
    ) -> State {
        let finals = self.finals_of(written);
        let mut state = state.conservative_join(
            written,
            captured,
            &finals,
            &self.lists,
            &mut self.reasons,
            reason,
        );
        sweep_state_props(
            &mut state,
            &self.props,
            written,
            captured,
            &finals,
            &self.lists,
            &mut self.reasons,
        );
        state
    }

    /// `current.conservativeJoin(written, captured)` with finality applied
    /// and no demotion reason: loop heads and closure entries erase the
    /// history (`flow_analysis.dart:4732-4741`).
    pub(crate) fn cjoin(&mut self, written: &FxHashSet<u32>, captured: &FxHashSet<u32>) {
        self.cjoin_reason(written, captured, None);
    }

    /// [`Self::cjoin`] carrying the official `getNonPromotionReason`
    /// payload — `Some` only at suspension points.
    pub(crate) fn cjoin_reason(
        &mut self,
        written: &FxHashSet<u32>,
        captured: &FxHashSet<u32>,
        reason: Option<crate::facts::NonPromotionCause>,
    ) {
        // Effect sets recorded by a collect run may reference variables
        // declared inside the collected range: those ids are gone after the
        // state rollback (they are re-declared per iteration) and must not
        // reach the join.
        let live = |v: &u32| (*v as usize) < self.vars.len();
        let written: FxHashSet<u32> = written.iter().copied().filter(live).collect();
        let captured: FxHashSet<u32> = captured.iter().copied().filter(live).collect();
        let current = std::mem::take(&mut self.current);
        self.current = self.join_state_with_sweep(current, &written, &captured, reason);
    }

    /// The official `suspension` (`flow_analysis.dart:7909`): every
    /// variable read in the current local function and written anywhere
    /// loses its promotions, except the function's own locals. The demotion
    /// applies only when the suspension point sits inside a local function
    /// (official `_enclosingFunctionExpressionInfoStack` non-empty): an
    /// await/yield directly in an async/sync* method body cannot let an
    /// enclosing function resume, so nothing demotes there. `span` is the
    /// suspension node's own range — the `DemoteViaSuspension.node` anchor
    /// each dropped promotion's reason link carries
    /// (`flow_analysis.dart:7935-7947`).
    pub(crate) fn suspend(&mut self, span: SourceRange) {
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
        self.cjoin_reason(
            &demote,
            &FxHashSet::default(),
            Some(crate::facts::NonPromotionCause::Suspension { span }),
        );
    }

    /// Interns a non-promotion entry row: the history chain walked front to
    /// back (most recent demotion first), subtype-filtered against `current`
    /// when given (the official variable arm's
    /// `!isSubtypeOf(currentType, nonPromotedType)` rule,
    /// `flow_analysis.dart:8846-8848` — a dropped type the flow type is
    /// still below is irrelevant), deduplicated first-wins per type (the
    /// official map's `??=`). The property arm passes `None`: every cleared
    /// promoted type stands as evidence. Equal rows share one arena slot,
    /// so repeated reads of the same demoted state cost no growth.
    pub(crate) fn intern_non_promotion(&mut self, current: Option<(Ty, u32)>, history: u32) -> u32 {
        if history == crate::model::NIL {
            return 0;
        }
        let mut entries: Vec<crate::facts::NonPromotionEntry> = Vec::new();
        for (ty, args, cause) in self.reasons.iter(history) {
            if current.is_some_and(|(cur, _)| self.ops.is_subtype(cur, ty)) {
                continue;
            }
            if entries.iter().any(|e| e.ty == ty && e.args == args) {
                continue;
            }
            entries.push(crate::facts::NonPromotionEntry { ty, args, cause });
        }
        if entries.is_empty() {
            return 0;
        }
        if let Some(index) = self.non_promotions.iter().position(|row| *row == entries) {
            #[expect(
                clippy::cast_possible_truncation,
                reason = "arena index; a body with 2^32 distinct demotion \
                          contexts cannot parse"
            )]
            return index as u32;
        }
        #[expect(
            clippy::cast_possible_truncation,
            reason = "arena index; a body with 2^32 distinct demotion \
                      contexts cannot parse"
        )]
        let id = self.non_promotions.len() as u32;
        self.non_promotions.push(entries);
        id
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

    /// Finds the innermost break target frame (top-down): with no label
    /// the nearest loop or switch statement; with a label the frame the
    /// label resolves to (official `getLabelTarget` walk, see
    /// [`Interp::find_labeled_target`]).
    pub(crate) fn find_break_target(&self, label: Option<u32>) -> Option<usize> {
        if let Some(id) = label {
            self.find_labeled_target(id, true)
        } else {
            for (index, frame) in self.stack.iter().enumerate().rev() {
                match frame {
                    Frame::While { .. } => return Some(index),
                    Frame::Switch { is_expr, .. } if !*is_expr => return Some(index),
                    _ => {}
                }
            }
            None
        }
    }

    /// Finds the innermost continue target: with no label the nearest
    /// loop; with a label the frame the label resolves to. `continue`
    /// never targets a switch (the official joins a continue model no
    /// switch end ever reads — a no-op), so it resolves to nothing there.
    pub(crate) fn find_continue_target(&self, label: Option<u32>) -> Option<usize> {
        if let Some(id) = label {
            self.find_labeled_target(id, false)
        } else {
            for (index, frame) in self.stack.iter().enumerate().rev() {
                if matches!(frame, Frame::While { .. }) {
                    return Some(index);
                }
            }
            None
        }
    }

    /// Resolves a labeled `break`/`continue` to its target frame: the
    /// official `getLabelTarget` walk (`flow_analysis_visitor.dart:487`)
    /// ported to frames. The nearest (top-down) `Labeled` frame carrying
    /// the label id is found; when its labeled statement's inner
    /// statement is a loop — or a switch, for `break` — that inner frame
    /// is the target, so the join lands in the context that owns the
    /// jump's exit (official lines 511-520); otherwise the labeled frame
    /// itself. No match means no context: official `handleBreak`/
    /// `handleContinue` join nothing and just mark the path unreachable.
    fn find_labeled_target(&self, label: u32, is_break: bool) -> Option<usize> {
        for (index, frame) in self.stack.iter().enumerate().rev() {
            let matches = match frame {
                Frame::Labeled {
                    label: Some(id),
                    extra_labels,
                    ..
                } => *id == label || extra_labels.contains(&label),
                _ => false,
            };
            if !matches {
                continue;
            }
            // The labeled statement's inner statement, when a loop or
            // switch, owns the frame directly above.
            return match self.stack.get(index + 1) {
                Some(Frame::While { .. }) => Some(index + 1),
                Some(Frame::Switch { is_expr: false, .. }) if is_break => Some(index + 1),
                Some(Frame::Switch { .. }) => None,
                _ => Some(index),
            };
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
            Frame::While { checkpoint, .. } => *checkpoint,
            // A labeled continue onto a non-loop labeled statement: the
            // official joins its context's continue model, which nothing
            // ever reads — the only observable is the dead path.
            Frame::Labeled { .. } => {
                self.unreachable_cur();
                return;
            }
            _ => return,
        };
        let joined = self.current.clone().unsplit_to(&mut self.reach, checkpoint);
        let prior = match &mut self.stack[index] {
            Frame::While { continue_model, .. } => continue_model.take(),
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
        self.push_scope();
        let scopes_base = self.scopes.len() - 1;
        if self.collect.is_some() {
            self.collect_push();
            self.stack.push(Frame::Exec {
                is_closure,
                scopes_base,
                saved: self.current.clone(),
                reads: FxHashSet::default(),
                declared: FxHashSet::default(),
                began_dead: self.stmt_began_dead,
            });
            return;
        }
        // A nested executable re-roots statement-boundary reachability
        // (official flow analysis walks each function body reachable): the
        // enclosing statement-boundary snapshot must not leak in, and the
        // body's own boundaries must not leak out (restored at
        // [`Interp::exec_end`]).
        let began_dead = self.stmt_began_dead;
        self.stmt_began_dead = false;
        if !is_closure {
            self.stack.push(Frame::Exec {
                is_closure: false,
                scopes_base,
                saved: self.current.clone(),
                reads: FxHashSet::default(),
                declared: FxHashSet::default(),
                began_dead,
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
                began_dead,
            });
            return;
        }
        let end = end as usize;
        let effects = self.collect_ranges(events, i + 1..end, match_of);
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
            began_dead,
        });
    }

    fn exec_end(&mut self, is_expr: bool, end: TokenSpan) {
        // The executable's end covers only the intervals anchored inside
        // it (official `flowEnd` on the declaration:
        // `_containsFirstDeadNode` fails for an anchor in the enclosing
        // scope, which keeps running to its own close).
        self.flow_end(Covered::Body);
        let collecting = self.collect.is_some();
        let Some(frame) = self.stack.pop() else {
            // A frame-less close (unbalanced recovery buffer): nothing to
            // close, no value to net.
            return;
        };
        let Frame::Exec {
            is_closure,
            scopes_base,
            saved,
            began_dead,
            ..
        } = frame
        else {
            self.stack.push(frame);
            return;
        };
        self.scopes.truncate(scopes_base);
        // The enclosing statement boundaries resume: the body's own
        // boundaries never rewrite them.
        self.stmt_began_dead = began_dead;
        // Net one value for the consumer (crate contract, see
        // `SwitchExprBlockEnd`/`ListLitEnd`): a function expression leaves
        // its value where the initializer/argument/assignment pops it. The
        // push belongs to the stack contract, not the diagnostic face —
        // collect runs need it too, or a downstream `ArgumentsEnd` pops the
        // slot the consumer meant for the enclosing expression and every
        // later pairing (an assignment target's `record_write`, an
        // initializer's `pending_inits`) reads the wrong slot.
        if is_expr {
            self.exprs.push(Expr::trivial(Ty::UNKNOWN));
        }
        if collecting {
            self.collect_pop(is_closure);
            return;
        }
        // The body's fall-through completion (official
        // `checkForBodyMayCompleteNormally`'s first decision,
        // type_analyzer.dart:647-652): the current state IS the body's
        // exit state here — read before the closure restore below puts
        // the enclosing state back.
        if self.reachable() {
            self.emit(FlowFact::BodyMayCompleteNormally { span: end });
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
            for_in,
            ..
        } = frame
        else {
            return;
        };
        // A for-in closes with the `forEach_end` formula
        // (`flow_analysis.dart:6954-6959`): the body's fall-through
        // joins the exit beside the head state — unlike
        // `whileStatement_end`/`for_end`, which join only the
        // condition-false and break models. (The official context there
        // never reads its own break model; the head's conservative join
        // already widened every body-written local, so keeping our
        // accumulated break states is a harmless reachability superset.)
        let exit = if for_in {
            let fall = self.current.clone();
            let breaks = self.join(Some(fall), break_model);
            self.join(cond_false.clone(), Some(breaks))
        } else {
            self.join(cond_false.clone(), break_model)
        };
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
        // The header's own first marker decides the kind: a search for
        // `ForParts` alone HIJACKS a for-in's extent when a C-for nests
        // inside its body (the inner `ForParts` matches, and the replay
        // slices the inner header over the outer tail — the for-in's post
        // code runs as loop body and every break/continue folds wrong).
        let Some(parts) = (i + 1..end).find(|&j| {
            matches!(events[j], Ev::ForParts { .. } | Ev::ForInParts)
        }) else {
            return Control::Continue;
        };
        let Ev::ForParts {
            left_sep,
            right_sep,
        } = events[parts]
        else {
            // A for-in: the linear walk delivers its `ForInParts` event.
            return Control::Continue;
        };
        let bounds = ForBounds::build(
            events,
            i,
            parts,
            left_sep.range.offset,
            right_sep.range.offset,
            right_sep.range.end(),
            end,
        );
        self.replay_c_for(&bounds, events, match_of, false);
        Control::Jump(end + 1)
    }

    /// The shared C-style `for` body: head split + join, initializers,
    /// condition, body, back-edge continue join, updaters, exit join.
    /// With `collection`, the frame stays until `ForControlFlowEnd`.
    fn replay_c_for(
        &mut self,
        bounds: &ForBounds,
        events: &[Ev],
        match_of: &[u32],
        collection: bool,
    ) {
        let base = self.exprs.len();
        let effects = self.collect_ranges(events, bounds.init.start..bounds.body.end, match_of);
        self.split_cur();
        // The init runs before the head join: the official loop-head join
        // sits at `for_conditionBegin` — after the initializer's one-time
        // execution — and the join's live-variable filter needs the
        // for-init declaration present (`vars.len()`), or the updater's
        // write to a for-init variable never re-widens its promotion and
        // the condition folds on the narrowed state
        // (`for (N? n = v; n != null; n = n.parent)` read its condition as
        // always-true and deaded the loop tail).
        self.for_header_depth += 1;
        self.run_range(events, bounds.init.clone(), match_of);
        self.for_header_depth = self.for_header_depth.saturating_sub(1);
        self.exprs.truncate(base);
        self.cjoin(&effects.written, &effects.captured);
        let checkpoint = self.reach.parent(self.current.reach);
        self.stack.push(Frame::While {
            collection,
            checkpoint,
            cond_false: None,
            break_model: None,
            continue_model: None,
            for_in: false,
        });
        let frame = self.stack.len() - 1;
        // The init→condition part boundary: header parts carry no statement
        // wrapper (the header parses a bare `parse_expression` list), so
        // this refresh is what lets the condition's wrapper anchor when the
        // init exited (`for (throw 0; c;)` — the official's first dead node
        // there is the condition).
        self.stmt_began_dead = !self.reachable();
        // Condition (empty condition = `true`).
        let cond = self.run_header_expr(events, bounds.cond.clone(), match_of, base);
        if let Some(expr) = &cond {
            self.condition_operand(expr);
        }
        let (if_true, if_false) = if let Some(expr) = cond {
            self.cond_split(&expr)
        } else {
            let cur = self.current.clone();
            (cur.clone(), cur.set_unreachable(&mut self.reach))
        };
        if let Some(Frame::While { cond_false, .. }) = self.stack.get_mut(frame) {
            *cond_false = Some(if_false);
        }
        self.current = if_true;
        // The official `for_conditionEnd` latch (dead_code_verifier.dart
        // :344-354): whether a dead interval was already open when the
        // condition ended. An interval anchored at the `for` itself (or at
        // the header) keeps owning the reporting — the updaters never fire
        // their own row (probe u5).
        let had_dead = self.dead.is_some();
        // Body.
        self.run_range(events, bounds.body.clone(), match_of);
        if !collection {
            // The loop statement's close covers body intervals (official
            // `visitForStatement`'s `flowEnd(node.body)`,
            // type_analyzer.dart:3192, containment per
            // dead_code_verifier.dart:453-459): an anchor inside the
            // body's extent — the body block's `{` or a statement within —
            // reports here, a wholly dead body block spanning its full
            // extent `}` included; an anchor in the header or at the `for`
            // itself is not a body descendant and the interval stays open
            // for the outer covering close. The close runs BEFORE the
            // updaters: the official row ends at the body's own end, so an
            // updater's statement end never folds into a body row and a
            // live tail after the loop stays row-free (probes u1/u6).
            self.flow_end_body(bounds.header_end);
        }
        // Updaters run on the back-edge state: join pending continues
        // first (official `forStatement_updaterBegin`). The frame read is
        // guarded: a recovery-shaped body can leave fewer frames on the
        // stack than the loop pushed (the analyzer corpus reproducible —
        // `stack[frame]` indexed past a truncated stack), and a missing
        // frame degrades to no pending continues exactly like every other
        // unbalanced-recovery read here.
        let continue_model = match self.stack.get(frame) {
            Some(Frame::While { continue_model, .. }) => continue_model.clone(),
            _ => None,
        };
        self.current = self.join(Some(self.current.clone()), continue_model);
        // Official `for_updaterBegin` (dead_code_verifier.dart:357-378):
        // updaters entered unreachable fire their own immediate row — from
        // the first updater's begin token to the last one's end. The part
        // carries no statement wrapper (the header parses a bare
        // `parse_expression` list), so the begin comes from the first
        // spanned event (an updater identifier's token), pulled back to a
        // prefix assignment's operator when one precedes it, and the end
        // from the postfix operators' and assignment expressions' ends
        // (the operand events' own ends for a bare prefix `++i`). The row
        // is independent of the body's: the official reports it without
        // opening an interval, so the loop's exit path is unaffected (a
        // live condition keeps the tail row-free, probe u3) and a wholly
        // dead loop folds the updaters into its own row instead (probe
        // u5).
        if !self.reachable() && !had_dead {
            let mut begin: Option<TokenSpan> = None;
            let mut last: Option<u32> = None;
            for event in &events[bounds.upd.clone()] {
                if let Some(span) = event.span() {
                    if begin.is_none() {
                        begin = Some(span);
                    }
                    last = Some(
                        last.map_or(span.range.end(), |current| current.max(span.range.end())),
                    );
                }
                match *event {
                    // `++i`: the operator's span follows the operand's events
                    // (the parser reports the prefix assignment after the
                    // operand parse) and precedes it in the source — the row
                    // begins at the operator (the updaters' beginToken,
                    // dead_code_verifier.dart:367-375).
                    Ev::UnaryPrefixAssign { span } => {
                        begin = Some(match begin {
                            Some(b) if b.range.offset <= span.range.offset => b,
                            _ => span,
                        });
                    }
                    Ev::Assign { end, .. }
                    | Ev::StmtEnd { end }
                    | Ev::UnaryPostfixAssign { end } => {
                        last = Some(last.map_or(end, |current| current.max(end)));
                    }
                    _ => {}
                }
            }
            if let (Some(begin), Some(last)) = (begin, last)
                && last > begin.range.offset
            {
                self.emit(FlowFact::DeadCode {
                    span: TokenSpan {
                        id: begin.id,
                        range: SourceRange {
                            offset: begin.range.offset,
                            length: last - begin.range.offset,
                        },
                    },
                });
            }
        }
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
        self.header_cond_depth += 1;
        self.run_range(events, range, match_of);
        self.for_header_depth = self.for_header_depth.saturating_sub(1);
        self.header_cond_depth = self.header_cond_depth.saturating_sub(1);
        // The slice is one expression: null-shorting opened inside it ends
        // here (official `finishNullShorting` at the dispatch boundary); an
        // enclosing `?` chain stays open.
        self.flush_null_shorts_above(base);
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
        // The header's own first marker decides the kind (the statement
        // form's `for_begin` documents the nested-marker hijack).
        if let Some(parts) = (i + 1..end).find(|&j| {
            matches!(events[j], Ev::ForParts { .. } | Ev::ForInParts)
        }) && matches!(events[parts], Ev::ForParts { .. })
        {
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
                right_sep.range.end(),
                end,
            );
            self.replay_c_for(&bounds, events, match_of, true);
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
            self.for_in_parts_at(events, i + 1, parts, end, true, match_of);
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
    fn for_in_parts(&mut self, events: &[Ev], i: usize, match_of: &[u32]) {
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
        self.for_in_parts_at(events, head_start, i, end, false, match_of);
    }

    fn for_in_parts_at(
        &mut self,
        events: &[Ev],
        head_start: usize,
        i: usize,
        end: usize,
        collection: bool,
        match_of: &[u32],
    ) {
        // The iterable expression ends: rejoin null-shorting opened inside
        // it before its value is consumed; an enclosing `?` chain stays
        // open.
        self.flush_null_shorts_above(self.exprs.len().saturating_sub(1));
        // The iterable expression value is on the stack; it contributes no
        // promotion state. Its written-argument tree is the element-type
        // source below: `for (e in xs)` on `List<int>` iterates `int`
        // (upstream `_inferForEachVariable` reads the resolved element
        // type; the name-level face of `Iterable<E>`'s `E` is the
        // iterable's first written argument).
        let mut element_ty = self
            .exprs
            .pop()
            .and_then(|iterable| {
                self.args
                    .get(iterable.args as usize)
                    .and_then(|args| args.first())
                    .map(|arg| (Ty { base: arg.base, nullable: arg.nullable }, arg.args))
            })
            .unwrap_or((Ty::UNKNOWN, crate::ty::EMPTY_ARGS));
        let body_start = (i..end).find(|&j| matches!(events[j], Ev::ForInBodyBegin));
        let body = match body_start {
            Some(start) => start..end,
            None => i + 1..end,
        };
        let effects = self.collect_ranges(events, body, match_of);
        // The loop head is official `forEach_bodyBegin`
        // (`flow_analysis.dart:6941-6952`): the state splits and
        // conservatively joins the body's writes in ONE step, and the
        // context saves THAT joined state as its `previous` — the
        // zero-iteration arm the exit joins carries the body's writes as
        // maybe, so a local the body assigns reads possibly- (not
        // definitely-) unassigned after the loop.
        self.split_cur();
        self.cjoin(&effects.written, &effects.captured);
        let previous = self.current.clone();
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
        if let Some(Resolved::Local(var)) = events[head_start..i].iter().find_map(|ev| match ev {
            Ev::InitializedIdent { name, .. } => self.resolve(*name),
            _ => None,
        }) {
            // The declared slot the `final e`/`var e` head left unknown
            // adopts the iterable's element type — the same inferred-type
            // fill `InitializerEnd` performs for `var v = e` (#2140's
            // staged face aside; the fold's own recovery here is the
            // iterable's written first argument). An explicitly spelled
            // head (`for (int e in xs)`) keeps its annotation — only the
            // unknown face fills.
            // The element face the fold writes: its own written-args
            // inference first; the semantic layer's staged element type
            // (#2506) fills when the fold could not — a property-read
            // iterable (`unit.directives`) carries no written argument
            // tree, and the Unknown slot downstream mis-fires the
            // InvalidType-scrutinee posture over the loop variable.
            // An explicitly spelled head (`for (int e in xs)`) keeps its
            // annotation — the unknown face fills below.
            let staged = if element_ty.0.base == crate::ty::Base::Unknown {
                self.vars
                    .get(var as usize)
                    .and_then(|meta| meta.decl_span)
                    .and_then(|span| self.inferred_locals.get(&span.offset))
                    .copied()
            } else {
                None
            };
            if let Some((staged_ty, staged_args)) = staged {
                element_ty = (staged_ty, staged_args);
                // The declared slot the `final e`/`var e` head left
                // unknown adopts the element type — the same inferred-type
                // fill `InitializerEnd` performs for `var v = e`.
                if let Some(meta) = self.vars.get_mut(var as usize)
                    && meta.ty.base == crate::ty::Base::Unknown
                {
                    meta.ty = staged_ty;
                    meta.args = staged_args;
                }
            } else if let Some(meta) = self.vars.get_mut(var as usize)
                && meta.ty.base == crate::ty::Base::Unknown
            {
                meta.ty = element_ty.0;
                meta.args = element_ty.1;
            }
            // Not a write in the official sets (the loop variable can be
            // `final`) and carries no demotion reason (the official
            // `forEach_assigns_to_declared_var` declares — no
            // `nonPromotionReason`, `flow_analysis.dart:4914-4916`): the
            // state moves without the write-site record.
            // (The searched names declared in this buffer always resolve
            // local — the top-level face never reaches this write.)
            self.apply_write(var, element_ty.0, element_ty.1, false, None, false);
        }
        let checkpoint = self.reach.parent(self.current.reach);
        self.stack.push(Frame::While {
            collection,
            checkpoint,
            cond_false: Some(previous),
            break_model: None,
            continue_model: None,
            for_in: true,
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
            Ev::CaseBegin { .. }
                | Ev::CaseExprBegin { .. }
                | Ev::CaseNoWhen
                | Ev::CaseExprEnd { .. }
        ) {
            self.maybe_close_case_group(events, match_of);
        }
        match ev {
            Ev::ExecBegin { is_closure } => {
                self.exec_begin(*is_closure, events, i, match_of);
                Control::Continue
            }
            Ev::ExecEnd { is_expr, end } => {
                self.exec_end(*is_expr, *end);
                Control::Continue
            }
            Ev::ForBegin { span } => self.for_begin(*span, events, i, match_of),
            Ev::ForControlFlowBegin => self.collection_for_begin(events, i, match_of),
            Ev::ForInParts => {
                // The `in` keyword ends the head pattern (official
                // `patternForIn_end` fires before the iterable runs): close
                // the head's standalone context so the zero-iteration state
                // and the loop exit see the popped state, and no later `if`
                // adopts the context (#2288: the leaked `Decl` deaded the
                // else branch after a for-in object pattern).
                self.close_standalone_pattern();
                self.for_in_parts(events, i, match_of);
                Control::Continue
            }
            Ev::ForInEnd => {
                // The loop statement's end covers body intervals (official
                // `forEach_end`'s covering close).
                self.flow_end(Covered::Strict);
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
    use crate::ev::Ev::{
        DoBegin, DoEnd, ExecBegin, ExecEnd, ForBegin, ForControlFlowBegin, ForControlFlowEnd,
        ForEnd, ForInEnd, SwitchBlockBegin, SwitchBlockEnd, TryBegin, TryEnd, WhileBegin, WhileEnd,
    };
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
            ForEnd | ForInEnd => {
                if let Some(begin) = for_stack.pop() {
                    link_pair(&mut match_of, begin, i);
                }
            }
            ForControlFlowBegin => coll_for_stack.push(i),
            ForControlFlowEnd { .. } => {
                if let Some(begin) = coll_for_stack.pop() {
                    link_pair(&mut match_of, begin, i);
                }
            }
            WhileBegin { .. } => while_stack.push(i),
            WhileEnd => {
                if let Some(begin) = while_stack.pop() {
                    link_pair(&mut match_of, begin, i);
                }
            }
            DoBegin { .. } => do_stack.push(i),
            DoEnd { .. } => {
                if let Some(begin) = do_stack.pop() {
                    link_pair(&mut match_of, begin, i);
                }
            }
            ExecBegin { .. } => exec_stack.push(i),
            ExecEnd { .. } => {
                if let Some(begin) = exec_stack.pop() {
                    link_pair(&mut match_of, begin, i);
                }
            }
            TryBegin { .. } => try_stack.push(i),
            TryEnd { .. } => {
                if let Some(begin) = try_stack.pop() {
                    link_pair(&mut match_of, begin, i);
                }
            }
            SwitchBlockBegin => switch_block_stack.push(i),
            SwitchBlockEnd => {
                if let Some(begin) = switch_block_stack.pop() {
                    link_pair(&mut match_of, begin, i);
                }
            }
            _ => {}
        }
    }
    match_of
}

/// Records the matching begin/end indices of one construct extent.
fn link_pair(match_of: &mut [u32], begin: usize, end: usize) {
    match_of[begin] = saturate_slot_u32(end);
    match_of[end] = saturate_slot_u32(begin);
}
