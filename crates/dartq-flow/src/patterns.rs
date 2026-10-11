//! Pattern events: alternative patterns for cases, if-case, and pattern
//! variable declarations.

use crate::ev::Ev;
use crate::interp::{Expr, Frame, Interp, PatternKind, PatternState, VarMeta};
use crate::ty::{Base, Ty};

impl<'o> Interp<'o> {
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
                Ev::PatternBegin | Ev::PatternEnd | Ev::GuardBegin | Ev::GuardEnd
            )
        {
            return;
        }
        match ev {
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
                        // the unmatched sibling is the pre-split state; the
                        // matched path continues in `current`.
                        let unmatched = self.current.clone();
                        self.split_cur();
                        // An if-case's scrutinee expression is on the stack.
                        let scrutinee = self.exprs.pop().and_then(|e| e.reference);
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
            Ev::ConstantPatternBegin => {}
            Ev::ConstantPatternEnd => {
                // A `null` constant test promotes the scrutinee on the
                // matched path (official tryMarkNonNullable).
                let constant = self.exprs.pop();
                if let Some(pattern) = self.pattern.as_ref() {
                    if constant.is_some_and(|e| e.ty.base == Base::Null) {
                        if let Some(var) = pattern.scrutinee {
                            let declared = self.vars[var as usize].ty;
                            let promoted = self.ops.promote_to_non_null(declared);
                            if promoted != declared {
                                self.current = self.current.clone().finish_type_test(
                                    self.ops,
                                    &mut self.lists,
                                    var,
                                    None,
                                    Some(promoted),
                                );
                            }
                        }
                    }
                }
            }
            Ev::DeclaredVarPattern { name, is_final } => {
                // Pattern variables are assigned exactly when the pattern
                // matched — declared initialized on the matched path
                // (official `declare(initialized: true)` on the pattern's
                // component scope).
                let meta = VarMeta {
                    ty: Ty::UNKNOWN,
                    late: false,
                    is_final: *is_final,
                    decl_span: None,
                };
                self.declare_var(*name, meta, true);
                if let Some(pattern) = self.pattern.as_mut() {
                    pattern.pending_decls.push((*name, *is_final));
                }
            }
            Ev::AssignedVarPattern { name } => {
                if let Some(var) = self.resolve(*name) {
                    self.pending_writes.push(var);
                }
            }
            Ev::Wildcard { .. } => {}
            Ev::GuardBegin => {
                // Snapshot the pre-guard state: the guard's own events run
                // on `current` next and leave it on the guard-true arm, so
                // this snapshot is the guard-false arm for the close's
                // unmatched join.
                if self.pattern.is_some() {
                    if let Some(pattern) = self.pattern.as_mut() {
                        pattern.pre_guard = Some(self.current.clone());
                    }
                }
            }
            Ev::GuardEnd => {
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

    /// Opens the pattern context of a case expression (official
    /// `switch_beginAlternative`, flow_analysis.dart:7965-7971): the
    /// clause's pattern runs under the switch frame's chained unmatched
    /// state — earlier clauses' guard-false outcomes chain there — and one
    /// pattern context opens for the clause. `is_expr` selects the
    /// switch-expression variant (no body lookahead, no case_open).
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
        if let Some(frame_index) = self.switch_frame() {
            if let Frame::Switch {
                unmatched: slot, ..
            } = &mut self.stack[frame_index]
            {
                *slot = unmatched;
            }
        }
    }

    /// Applies a pattern's guard to split matched/unmatched, returning the
    /// chained unmatched state (official `_popPattern`, fed by the clause's
    /// `guardInfo` — flow_analysis.dart:8004-8042: the guard's true state
    /// becomes the clause's matched state, the false state joins the
    /// unmatched chain). The guard's own events ran on `current` between
    /// `GuardBegin` and `GuardEnd` — its last branching event (e.g. the
    /// type-test's `IsEnd`, interp_expr.rs) left `current` on the true arm
    /// — so `current` IS the guard's ifTrue state and the `GuardBegin`
    /// snapshot is its ifFalse.
    fn apply_pattern_exit(
        &mut self,
        mut pattern: PatternState,
        has_guard: bool,
    ) -> crate::model::State {
        let mut unmatched = pattern.unmatched;
        if has_guard || matches!(pattern.kind, PatternKind::Switch | PatternKind::SwitchExpr) {
            if pattern.guard.take().is_some() {
                let if_false = pattern
                    .pre_guard
                    .take()
                    .unwrap_or_else(|| self.current.clone());
                unmatched = self.join(Some(unmatched), Some(if_false));
            }
        }
        unmatched
    }
}
