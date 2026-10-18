//! Pattern events: alternative patterns for cases, if-case, and pattern
//! variable declarations.

use crate::ev::Ev;
use crate::interp::{Expr, Frame, Interp, PatternKind, PatternState, Resolved, VarMeta};
use crate::ty::{Base, Classification, Ty};

impl Interp<'_> {
    /// Handles one pattern-layer event.
    pub(crate) fn pattern_step(&mut self, ev: &Ev, _events: &[Ev], _i: usize, _match_of: &[u32]) {
        // The collect pre-pass tracks writes/declares only. The pattern
        // state machine (matched/unmatched splits, guard snapshots) is a
        // flow-walk concern, and a collect-run pattern context leaks
        // across `collect_ranges`' rollback (its snapshot does not cover
        // `self.pattern`), double-driving the head events the real walk
        // then re-runs.
        if self.collecting()
            && matches!(
                ev,
                Ev::PatternBegin
                    | Ev::PatternEnd
                    | Ev::ObjectPattern { .. }
                    | Ev::GuardBegin
                    | Ev::GuardEnd
            )
        {
            return;
        }
        match ev {
            Ev::ObjectPattern { name, args } => self.matched_object_pattern(*name, *args),
            Ev::PatternBegin => {
                self.pattern_depth += 1;
                if self.pattern_depth == 1 {
                    if self.pattern.is_some() {
                        // A case head already opened the pattern context
                        // (`case_pattern_begin`): official
                        // `switch_beginAlternative` pushes exactly one
                        // pattern context per clause, so the case-level
                        // state IS the clause's context — keep its kind,
                        // unmatched chain and scrutinee; only version the
                        // matched path for the pattern's component joins.
                        self.split_cur();
                    } else {
                        // Standalone pattern context (if-case, pattern
                        // variable declaration): official `_pushPattern` —
                        // the working unmatched chain starts UNREACHABLE
                        // (`_unmatched = _current.setUnreachable()`), so
                        // the first clause's `_join(_unmatched, promoted)`
                        // short-circuits to the promoted state undiluted —
                        // an if-case's `case null` promotes the scrutinee
                        // on the fall-through (the oracle's
                        // promotion_patterns fixture 70-72: the branch
                        // demotes, the fall-through read is clean). The
                        // pre-split reachable state here would intersect
                        // the promotion away at the chain join.
                        self.split_cur();
                        // An if-case's scrutinee expression is on the stack.
                        let scrutinee = self.exprs.pop().and_then(|e| e.reference);
                        let unmatched = self.current.clone().set_unreachable(&mut self.reach);
                        self.pattern = Some(PatternState {
                            kind: std::mem::replace(&mut self.pattern_kind_hint, PatternKind::Decl),
                            unmatched,
                            scrutinee,
                            guard: None,
                            pre_guard: None,
                            pending_decls: Vec::new(),
                        });
                    }
                }
            }
            Ev::PatternEnd => {
                self.pattern_depth = self.pattern_depth.saturating_sub(1);
            }
            Ev::ConstantPatternEnd => {
                let constant = self.exprs.pop();
                // Only a TOP-LEVEL constant pattern is an equality check on
                // the matched value — a field's constant sub-pattern
                // (`case Point(x: 5)`) constrains that field, not the
                // scrutinee (the same depth gate
                // [`Interp::matched_object_pattern`] uses).
                if self.pattern_depth != 1 {
                    return;
                }
                let Some(constant) = constant else {
                    return;
                };
                let Some(var) = self.pattern_scrutinee() else {
                    return;
                };
                // Official `constantPattern_end` →
                // `_handleEqualityCheckPattern` (flow_analysis.dart:8866-8948):
                // the implicit `== operand` runs `_equalityCheck` on
                // (matched value type, constant type).
                //
                // A nullability-disjoint pair returns
                // `_EqualityCheckHasKnownResult(areEqual: false)` — the match
                // is guaranteed to fail, so the live matched state folds into
                // the unmatched chain and `current` goes unreachable; the
                // case body runs dead and anchors its row at the first body
                // statement.
                let (matched, matched_args) = {
                    let model = self.current.model(var);
                    (
                        self.promoted_type_of(var, &model),
                        self.promoted_args_of(var, &model),
                    )
                };
                if self.disjoint_by_nullability(matched, constant.ty) {
                    let cur = self.current.clone();
                    if let Some(chain) = self.pattern.as_ref().map(|p| p.unmatched.clone()) {
                        let joined = self.join(Some(chain), Some(cur));
                        if let Some(pattern) = self.pattern.as_mut() {
                            pattern.unmatched = joined;
                        }
                    }
                    // Officially `current` goes unreachable and the case
                    // body dies. Emit it only when the disjointness is
                    // promotion-derived — a nullable scrutinee promoted
                    // non-null is invisible to the space world, so the flow
                    // row is the sole source. When the DECLARED type is
                    // already disjoint (`case null` over `int`), the
                    // semantic layer's case-body row owns the report and a
                    // flow row would duplicate it
                    // (`switch_same_type_variable_pattern_chain_dead`'s
                    // `case null` after a total `case A a2` — flow emits
                    // once, semantic once, unless the kill stands down).
                    let declared = self
                        .vars
                        .get(var as usize)
                        .map_or(Ty::UNKNOWN, |meta| meta.ty);
                    if !self.disjoint_by_nullability(declared, constant.ty) {
                        self.unreachable_cur();
                    }
                    return;
                }
                // A null-or-equivalent constant is the null-check face
                // (`case null:` → `_handleNullTestPattern` matchesIfNull).
                // The pattern fails only where the matched value is non-null,
                // so the unmatched chain takes the non-null-promoted state
                // (official `_unmatched = _join(_unmatched!, ifNotNull)`);
                // `current` — the matched path — stays the is-null state.
                // When the matched value is already `Null`/equivalent the
                // official marks that promoted state unreachable (it can never
                // be non-null), which the join drops — an always-matching
                // `case null` leaves the chain's reachability untouched.
                if self.classify(constant.ty) == Classification::NullOrEquivalent {
                    let (non_null, non_null_args) =
                        self.promote_to_non_null(matched, matched_args);
                    let mut if_not_null = if non_null != matched {
                        self.current.clone().finish_type_test(
                            self.ops,
                            &mut self.lists,
                            var,
                            None,
                            Some((non_null, non_null_args)),
                        )
                    } else {
                        self.current.clone()
                    };
                    if self.classify(matched) == Classification::NullOrEquivalent {
                        if_not_null = if_not_null.set_unreachable(&mut self.reach);
                    }
                    if let Some(chain) = self.pattern.as_ref().map(|p| p.unmatched.clone()) {
                        let joined = self.join(Some(chain), Some(if_not_null));
                        if let Some(pattern) = self.pattern.as_mut() {
                            pattern.unmatched = joined;
                        }
                    }
                    return;
                }
                // Every other constant gives no narrowing
                // (`_NoEqualityInformation`): the match may or may not hold
                // on the same scrutinee state, so `current` joins the
                // unmatched chain unchanged.
                let cur = self.current.clone();
                if let Some(chain) = self.pattern.as_ref().map(|p| p.unmatched.clone()) {
                    let joined = self.join(Some(chain), Some(cur));
                    if let Some(pattern) = self.pattern.as_mut() {
                        pattern.unmatched = joined;
                    }
                }
            }
            Ev::DeclaredVarPattern {
                name,
                is_final,
                token,
            } => {
                // Pattern variables are assigned exactly when the pattern
                // matched — declared initialized on the matched path
                // (official `declare(initialized: true)` on the pattern's
                // component scope). The annotation arrives as `Type`/`NoType`
                // events immediately ahead of this event
                // (parse_variable_pattern's `parse_type`); it is the
                // variable's declared type (official `declare` carries it):
                // the unmatched-branch factoring reads it, and a resolved
                // back-end's promotion gate keys on it.
                let meta = VarMeta {
                    ty: self.pending_ty.take().unwrap_or(Ty::UNKNOWN),
                    args: std::mem::take(&mut self.pending_ty_args),
                    late: false,
                    is_final: *is_final,
                    decl_span: Some(token.range),
                };
                self.declare_var(*name, meta, true);
                if let Some(pattern) = self.pattern.as_mut() {
                    pattern.pending_decls.push((*name, *is_final));
                }
            }
            Ev::AssignedVarPattern { name } => {
                // A top-level name in an assigned-var position writes no
                // tracked local — the pending write feeds a state write,
                // which only a local id addresses.
                if let Some(Resolved::Local(var)) = self.resolve(*name) {
                    self.pending_writes.push(var);
                }
            }
            Ev::Wildcard { .. } => {
                // A typed wildcard (`num _`) parses its annotation but
                // declares nothing: drop the pending type so it cannot
                // leak into the next consumer (`Formal`,
                // `DeclaredVarPattern`).
                self.pending_ty = None;
                self.pending_ty_args = crate::ty::EMPTY_ARGS;
            }
            Ev::GuardBegin => {
                // Snapshot the pre-guard state: the guard's own events run
                // on `current` next and leave it on the guard-true arm, so
                // this snapshot is the guard-false arm for the close's
                // unmatched join.
                if let Some(pattern) = self.pattern.as_mut() {
                    pattern.pre_guard = Some(self.current.clone());
                }
            }
            Ev::GuardEnd => {
                // The guard expression ends: rejoin null-shorting opened
                // inside it before its value is read; an enclosing `?`
                // chain stays open.
                self.flush_null_shorts_above(self.consumer_base(1));
                let guard = self.exprs.pop();
                if let Some(pattern) = self.pattern.as_mut() {
                    pattern
                        .guard
                        .get_or_insert(guard.unwrap_or_else(|| Expr::trivial(Ty::UNKNOWN)));
                }
            }
            _ => {}
        }
    }

    /// A matched object pattern's promotion of the switched variable
    /// (official `promoteForPattern`, flow_analysis.dart:7703-7755, fed by
    /// `analyzeObjectPattern` — shared type_analyzer.dart:1749-1772 — with
    /// the pattern's downward-inferred required type). The match splits
    /// matched/unmatched through the same [`Interp::branch_type_test`]
    /// port as an `is` test (`tryPromoteForTypeCheck`): the matched arm is
    /// what the clause's guard and body run on — the official order is
    /// pattern → guard → body, so the guard of the same clause sees the
    /// promotion — and the failed arm joins the unmatched chain when the
    /// match can fail (official `coversMatchedType` gate), so the next
    /// clause and the post-switch state see the unpromoted variable. A
    /// nullability-disjoint pair kills the matched path (official
    /// `cannotMatch` → `setUnreachable`): the case body is dead.
    pub(crate) fn matched_object_pattern(&mut self, name: u32, args: u32) {
        // A top-level pattern constrains the matched value; a field's
        // sub-pattern constrains that field (the official sub-pattern face
        // runs under pushPropertySubpattern) and promotes nothing here.
        if self.pattern_depth != 1 {
            return;
        }
        let Some(var) = self.pattern_scrutinee() else {
            return;
        };
        let known = Ty {
            base: Base::Name(name),
            nullable: false,
        };
        let model = self.current.model(var);
        let matched = self.promoted_type_of(var, &model);
        let candidate = self.pattern_promotion_candidate(var, known, args);
        let (matched_arm, unmatched_arm) = self.branch_type_test(var, known, args, candidate);
        self.current = matched_arm;
        if self.disjoint_by_nullability(matched, known) {
            self.unreachable_cur();
        }
        if !self.ops.is_subtype(matched, known)
            && let Some(chain) = self.pattern.as_ref().map(|p| p.unmatched.clone())
        {
            let joined = self.join(Some(chain), Some(unmatched_arm));
            if let Some(pattern) = self.pattern.as_mut() {
                pattern.unmatched = joined;
            }
        }
    }

    /// The matched value's variable — the key every matched-type question
    /// (`promoted_type_of`, `finish_type_test`) resolves against. The switch
    /// scrutinee (statement or expression alternative) lives on the switch
    /// frame; the standalone contexts (if-case, pattern declaration) carry
    /// their own matched value's reference. The kind gate keeps an if-case
    /// nested in a case body from latching the switch frame's scrutinee.
    fn pattern_scrutinee(&self) -> Option<u32> {
        match self.pattern.as_ref().map(|p| p.kind) {
            Some(PatternKind::Switch | PatternKind::SwitchExpr) => {
                self.switch_frame()
                    .and_then(|index| match &self.stack[index] {
                        Frame::Switch { scrutinee, .. } => *scrutinee,
                        _ => None,
                    })
            }
            Some(PatternKind::IfCase | PatternKind::Decl) => {
                self.pattern.as_ref().and_then(|p| p.scrutinee)
            }
            None => None,
        }
    }

    /// The pattern-face promotion candidate (official
    /// `tryPromoteForTypeCheck`'s true arm reached through
    /// `promoteForPattern`): the pattern type factored against every
    /// standing promotion ([`Interp::factored_candidate`]). The argument
    /// gate cuts to same-base shapes — the written-vs-declared comparison
    /// is the raw-spelling question (`case List():` against `List<int> x`
    /// spells `List<dynamic>`, the match can fail); across bases the
    /// parameter lists are unrelated and the question is the undecidable
    /// cross-type subtyping the is-test face answers optimistically.
    /// The unparameterized head of a cross-base match (`case Some():` on
    /// `Option<T>`) promotes: the official entry infers the required
    /// type's arguments downward from the matched type
    /// (`downwardInferObjectPatternRequiredType`), and the written
    /// spelling stays the honest name-level stand-in.
    fn pattern_promotion_candidate(&self, var: u32, tested: Ty, tested_args: u32) -> Option<Ty> {
        match tested.base {
            Base::Unknown | Base::Dynamic | Base::Void => None,
            _ => {
                if let Some(meta) = self.vars.get(var as usize)
                    && meta.ty.base == tested.base
                    && !self.args_admit(tested, tested_args, meta.args)
                {
                    return None;
                }
                self.factored_candidate(var, tested)
            }
        }
    }

    /// Opens the pattern context of a case expression (official
    /// `switch_beginAlternative`, flow_analysis.dart:7965-7971): the
    /// clause's pattern runs under the switch frame's chained unmatched
    /// state — earlier clauses' guard-false outcomes chain there — and one
    /// pattern context opens for the clause. `is_expr` selects the
    /// switch-expression variant (no body lookahead, no `case_open`).
    pub(crate) fn case_pattern_begin(&mut self, is_expr: bool) {
        let chain = self
            .switch_frame()
            .and_then(|frame_index| match &self.stack[frame_index] {
                Frame::Switch { unmatched, .. } => Some(unmatched.clone()),
                _ => None,
            });
        let unmatched = chain.unwrap_or_else(|| self.current.clone());
        self.current = unmatched.clone();
        self.pattern = Some(PatternState {
            kind: if is_expr {
                PatternKind::SwitchExpr
            } else {
                PatternKind::Switch
            },
            unmatched,
            scrutinee: None,
            guard: None,
            pre_guard: None,
            pending_decls: Vec::new(),
        });
    }

    /// Closes the pattern context of a switch-statement case expression:
    /// the guard's branches split matched/unmatched, and the chain state
    /// plus the combined group state update in the switch frame (official
    /// `switch_endAlternative`).
    pub(crate) fn case_pattern_end(&mut self, has_guard: bool) {
        let Some(pattern) = self.pattern.take() else {
            return;
        };
        let PatternKind::Switch = pattern.kind else {
            self.pattern = Some(pattern);
            return;
        };
        let unmatched = self.apply_pattern_exit(pattern, has_guard);
        if let Some(frame_index) = self.switch_frame() {
            let combined = self.current.clone();
            let prior = match &mut self.stack[frame_index] {
                Frame::Switch {
                    unmatched: slot,
                    group_combined,
                    case_open,
                    ..
                } => {
                    *slot = unmatched;
                    *case_open = true;
                    group_combined.take()
                }
                _ => None,
            };
            let joined = self.join(prior, Some(combined));
            if let Frame::Switch { group_combined, .. } = &mut self.stack[frame_index] {
                *group_combined = Some(joined);
            }
        }
    }

    /// Closes the pattern context of a switch-expression alternative
    /// (same chain semantics, no body lookahead). Only the unmatched
    /// chain updates here: the alternative joins the group once, at
    /// `SwitchExprCaseEnd`, with its final state — the alternative's
    /// contribution is the guard-true state, never the pre-guard one
    /// (official `switch_endAlternative`,
    /// flow_analysis.dart:8008-8039).
    pub(crate) fn case_pattern_end_expr(&mut self) {
        let Some(pattern) = self.pattern.take() else {
            return;
        };
        let PatternKind::SwitchExpr = pattern.kind else {
            self.pattern = Some(pattern);
            return;
        };
        let unmatched = self.apply_pattern_exit(pattern, false);
        if let Some(frame_index) = self.switch_frame()
            && let Frame::Switch {
                unmatched: slot, ..
            } = &mut self.stack[frame_index]
        {
            *slot = unmatched;
        }
    }

    /// Applies a pattern's guard to split matched/unmatched, returning the
    /// chained unmatched state (official `_popPattern`, fed by the clause's
    /// `guardInfo` — flow_analysis.dart:8004-8042: the guard's true state
    /// becomes the clause's matched state, the false state joins the
    /// unmatched chain). The guard's own events ran on `current` between
    /// `GuardBegin` and `GuardEnd` — its last branching event (e.g. the
    /// type-test's `IsEnd`, `interp_expr.rs`) left `current` on the true arm
    /// — so `current` IS the guard's ifTrue state and the `GuardBegin`
    /// snapshot is its ifFalse.
    fn apply_pattern_exit(
        &mut self,
        mut pattern: PatternState,
        has_guard: bool,
    ) -> crate::model::State {
        let mut unmatched = pattern.unmatched;
        if (has_guard || matches!(pattern.kind, PatternKind::Switch | PatternKind::SwitchExpr))
            && pattern.guard.take().is_some()
        {
            let if_false = pattern
                .pre_guard
                .take()
                .unwrap_or_else(|| self.current.clone());
            unmatched = self.join(Some(unmatched), Some(if_false));
        }
        unmatched
    }
}
