//! Statement events: conditionals, loops, try/catch/finally, switch
//! statements, jumps, declarations, and asserts — in the official
//! decision order.

use dartq_parser::TokenSpan;

use crate::ev::{BlockKind, Ev};
use crate::interp::{DeclCtx, Effects, Expr, Frame, Interp, PatternKind, Scope, VarMeta};
use crate::model::{NIL, State, attach_finally_model};
use crate::ty::Ty;

impl<'o> Interp<'o> {
    /// Handles one statement-layer event; `false` means the event belongs
    /// to another layer.
    pub(crate) fn stmt_step(&mut self, ev: &Ev, events: &[Ev], i: usize, match_of: &[u32]) -> bool {
        match ev {
            Ev::ExprStmt { span } => {
                self.header_dead_check(*span);
                // Inside a `for` header the statement wrapper is an
                // artifact of the part's parsing: its value feeds the
                // header replay, so it stays on the stack.
                if self.for_header_depth == 0 {
                    self.exprs.pop();
                }
            }
            Ev::IfBegin { span } => self.dead_code(*span),
            Ev::ThenBegin => {
                // If-case: the pattern context holds the branch states.
                if let Some(pattern) = self.pattern.as_mut() {
                    if pattern.kind == PatternKind::Decl {
                        // No declaration-statement event ended it: this is
                        // an if-case pattern.
                        pattern.kind = PatternKind::IfCase;
                    }
                    if pattern.kind == PatternKind::IfCase {
                        self.if_case_then();
                        return true;
                    }
                }
                let _ = &self.pattern;
                let cond = self.pop_value();
                let (if_true, if_false) = self.cond_split(&cond);
                self.stack.push(Frame::If {
                    branch: if_false,
                    after_then: None,
                });
                self.current = if_true;
            }
            Ev::ElseBegin => {
                // The then branch's end covers its intervals (official
                // `handle_ifStatement_thenEnd`'s `flowEnd`).
                self.flow_end(crate::interp::Covered::Strict);
                let after_then = self.current.clone();
                if let Some(Frame::If {
                    branch,
                    after_then: slot,
                }) = self.stack.last_mut()
                {
                    *slot = Some(after_then);
                    self.current = branch.clone();
                }
            }
            Ev::IfEnd { has_else } => {
                // The last branch's end covers its intervals (official
                // `handle_ifStatement_thenEnd` without an else, else
                // `handle_ifStatement_elseEnd`).
                self.flow_end(crate::interp::Covered::Strict);
                let Some(Frame::If { branch, after_then }) = self.stack.pop() else {
                    return true;
                };
                let then_state = if *has_else { after_then } else { None };
                let joined = self.join(then_state.or(Some(branch)), Some(self.current.clone()));
                self.current = joined.inherit_tested(&self.current, &mut self.lists);
            }
            Ev::WhileBegin { span } => {
                self.dead_code(*span);
                if self.collecting() {
                    return true;
                }
                // Loop head: split + conservative join over the whole loop
                // (official `whileStatement_conditionBegin`).
                // An unmatched begin (parser recovery) collects nothing:
                // the extent is treated as self-contained.
                let end = match_of[i];
                let effects = if end == NIL {
                    Effects::default()
                } else {
                    self.collect_ranges(events, &[i..end as usize], match_of)
                };
                self.split_cur();
                self.cjoin(&effects.written, &effects.captured);
                let checkpoint = self.reach.parent(self.current.reach);
                self.stack.push(Frame::While {
                    collection: false,
                    checkpoint,
                    cond_false: None,
                    break_model: None,
                    continue_model: None,
                });
            }
            Ev::WhileBodyBegin => {
                if self.collecting() {
                    return true;
                }
                let cond = self
                    .exprs
                    .pop()
                    .unwrap_or_else(|| Expr::trivial(Ty::UNKNOWN));
                let (if_true, if_false) = self.cond_split(&cond);
                if let Some(Frame::While { cond_false, .. }) = self.stack.last_mut() {
                    *cond_false = Some(if_false);
                }
                self.current = if_true;
            }
            Ev::WhileEnd => {
                // The loop statement's end covers body intervals (official
                // `visitWhileStatement`'s `flowEnd` on the body).
                self.flow_end(crate::interp::Covered::Strict);
                self.loop_exit(false)
            }
            Ev::DoBegin { span } => {
                self.dead_code(*span);
                if self.collecting() {
                    return true;
                }
                let end = match_of[i];
                let effects = if end == NIL {
                    Effects::default()
                } else {
                    self.collect_ranges(events, &[i..end as usize], match_of)
                };
                self.split_cur();
                self.cjoin(&effects.written, &effects.captured);
                let checkpoint = self.reach.parent(self.current.reach);
                self.stack.push(Frame::While {
                    collection: false,
                    checkpoint,
                    cond_false: None,
                    break_model: None,
                    continue_model: None,
                });
            }
            Ev::DoBodyEnd => {
                if self.collecting() {
                    return true;
                }
                // The continue point (official doStatement_conditionBegin):
                // join pending continues before the condition runs.
                let continue_model = match self.stack.last() {
                    Some(Frame::While { continue_model, .. }) => continue_model.clone(),
                    _ => None,
                };
                self.current = self.join(Some(self.current.clone()), continue_model);
                // A dead back edge makes the condition the first dead node
                // (official: `parent is DoStatement` anchors the two-range
                // split); the `while` part arrives with `DoEnd`.
                self.dead_do_condition();
            }
            Ev::DoEnd { while_range } => {
                if self.collecting() {
                    return true;
                }
                let cond = self
                    .exprs
                    .pop()
                    .unwrap_or_else(|| Expr::trivial(Ty::UNKNOWN));
                let (_, if_false) = self.cond_split(&cond);
                if let Some(Frame::While { cond_false, .. }) = self.stack.last_mut() {
                    *cond_false = Some(if_false);
                }
                self.loop_exit(false);
                // Attach the `while (c);` part to the interval the do's
                // back edge opened, and drop the tail row when a break
                // makes the code after the do-while reachable (official
                // `hasBreakStatement`).
                let tail_reachable = self.reachable();
                if let Some(window) = &mut self.dead {
                    if window.awaiting_split() {
                        window.attach_split(*while_range, tail_reachable);
                    }
                }
            }
            Ev::ReturnBegin { span } => self.dead_code(*span),
            Ev::ReturnEnd { has_expression } => {
                if *has_expression {
                    self.exprs.pop();
                }
                // Official handleReturn: the current path becomes
                // unreachable (inside closures too).
                self.unreachable_cur();
            }
            Ev::YieldBegin { span } => self.dead_code(*span),
            Ev::YieldEnd => {
                self.exprs.pop();
                self.suspend();
            }
            Ev::AwaitEnd => {
                self.exprs.pop();
                self.exprs.push(Expr::trivial(Ty::UNKNOWN));
                self.suspend();
            }
            Ev::Break { label, span } => {
                self.dead_code(*span);
                if self.collecting() {
                    return true;
                }
                match self.find_break_target(*label) {
                    Some(index) => self.do_break(index),
                    None => self.current = self.current.clone().set_unreachable(&mut self.reach),
                }
            }
            Ev::Continue { label, span } => {
                self.dead_code(*span);
                if self.collecting() {
                    return true;
                }
                match self.find_continue_target(*label) {
                    Some(index) => self.do_continue(index),
                    None => self.current = self.current.clone().set_unreachable(&mut self.reach),
                }
            }
            Ev::LabeledBegin { label, span } => {
                self.dead_code(*span);
                if self.collecting() {
                    return true;
                }
                let checkpoint = self.current.reach;
                self.stack.push(Frame::Labeled {
                    label: *label,
                    extra_labels: Vec::new(),
                    checkpoint,
                    break_model: None,
                });
            }
            Ev::LabelDecl { id } => {
                if self.collecting() {
                    return true;
                }
                // An additional label of the innermost labeled statement
                // (`a: b: s`): either name targets the statement.
                if let Some(Frame::Labeled { extra_labels, .. }) = self.stack.last_mut() {
                    extra_labels.push(*id);
                }
            }
            Ev::LabeledEnd => {
                if self.collecting() {
                    return true;
                }
                match self.stack.pop() {
                    Some(Frame::Labeled { break_model, .. }) => {
                        let joined = self.join(break_model, Some(self.current.clone()));
                        self.current = joined;
                    }
                    other => {
                        // A frame-less close (unbalanced recovery buffer) is
                        // a no-op; a mismatched frame goes back.
                        if let Some(frame) = other {
                            self.stack.push(frame);
                        }
                    }
                }
            }
            Ev::TryBegin { span } => {
                self.dead_code(*span);
                if self.collecting() {
                    return true;
                }
                // An unmatched begin (parser recovery) collects nothing:
                // the extent is treated as self-contained.
                let matched = match_of[i];
                let effects = if matched == NIL {
                    Effects::default()
                } else {
                    // The try body extent ends at the first catch/finally
                    // marker or the end of the statement.
                    let end = matched as usize;
                    let body_end = (i + 1..=end)
                        .find(|&j| {
                            matches!(
                                events[j],
                                Ev::CatchClauseBegin | Ev::FinallyBlock | Ev::TryEnd { .. }
                            )
                        })
                        .unwrap_or(end);
                    self.collect_ranges(events, &[i + 1..body_end], match_of)
                };
                let pre = self.current.clone();
                self.split_cur();
                self.stack.push(Frame::TryCatch {
                    pre,
                    previous: self.current.clone(),
                    before_catch: None,
                    after: None,
                    wrote: effects.written,
                    captured: effects.captured,
                });
            }
            Ev::CatchClauseBegin => {
                // The try body's end covers its intervals (official
                // `visitTryStatement`'s `flowEnd` on the body, fired before
                // the catch clauses run).
                self.flow_end(crate::interp::Covered::Strict);
                self.scopes.push(Scope::new());
                if self.collecting() {
                    return true;
                }
                // First catch: compute the conservative entry state
                // (official `tryCatchStatement_bodyEnd` + `catchBegin`).
                let (previous, written, captured, first) = match self.stack.last() {
                    Some(Frame::TryCatch {
                        previous,
                        wrote,
                        captured,
                        before_catch,
                        ..
                    }) => (
                        previous.clone(),
                        wrote.clone(),
                        captured.clone(),
                        before_catch.is_none(),
                    ),
                    _ => return true,
                };
                let entry = if first {
                    let finals = self.finals_of(&written);
                    let state = previous.conservative_join(&written, &captured, &finals);
                    // The try body's normal exit seeds the after-accumulator
                    // (official tryCatchStatement_bodyEnd).
                    let body_exit = self.current.clone();
                    if let Some(Frame::TryCatch {
                        before_catch,
                        after,
                        ..
                    }) = self.stack.last_mut()
                    {
                        *before_catch = Some(state.clone());
                        *after = Some(body_exit);
                    }
                    state
                } else {
                    match self.stack.last() {
                        Some(Frame::TryCatch {
                            before_catch: Some(state),
                            ..
                        }) => state.clone(),
                        _ => return true,
                    }
                };
                self.current = entry;
            }
            Ev::CatchClauseEnd => {
                // The catch body's end covers its intervals (official
                // `flowEnd` on the catch clause body, before the exit join).
                self.flow_end(crate::interp::Covered::Strict);
                if self.collecting() {
                    return true;
                }
                let exit = self.current.clone();
                let prior = match self.stack.last_mut() {
                    Some(Frame::TryCatch { after, .. }) => after.take(),
                    _ => None,
                };
                let joined = self.join(prior, Some(exit));
                if let Some(Frame::TryCatch { after, .. }) = self.stack.last_mut() {
                    *after = Some(joined);
                }
                // The clause body ended: drop the parameter scope.
                self.scopes.pop();
            }
            Ev::FinallyBlock => {
                // The try body's end covers its intervals (official
                // `visitTryStatement`'s `flowEnd` on the body); the finally
                // block itself ends without a covering close.
                self.flow_end(crate::interp::Covered::Strict);
                if self.collecting() {
                    return true;
                }
                // Close the catch context and open the finally context
                // (official `tryFinallyStatement_finallyBegin`).
                let Some(Frame::TryCatch {
                    pre,
                    wrote,
                    captured,
                    after,
                    ..
                }) = self.stack.pop()
                else {
                    return true;
                };
                // The try-catch exit: body normal exit joined with every
                // catch exit.
                let after_try = match after {
                    Some(state) => self.join(Some(state), Some(self.current.clone())),
                    None => self.current.clone(),
                };
                self.current = after_try.clone();
                // Exception-entry path into the finally: join the pre-try
                // state conservatively over the try body's writes.
                let finals = self.finals_of(&wrote);
                let entry = pre.conservative_join(&wrote, &captured, &finals);
                self.current = self.join(Some(self.current.clone()), Some(entry));
                self.stack.push(Frame::TryFinally {
                    after_try: Some(after_try),
                    before_finally: Some(self.current.clone()),
                });
            }
            Ev::TryEnd { has_finally } => {
                if self.collecting() {
                    return true;
                }
                if *has_finally {
                    if let Some(Frame::TryFinally {
                        before_finally,
                        after_try,
                        ..
                    }) = self.stack.pop()
                    {
                        // A frame missing its accumulated states (unbalanced
                        // recovery buffer) leaves the current state alone.
                        if let (Some(after_try), Some(before_finally)) = (after_try, before_finally)
                        {
                            self.current = attach_finally(
                                self,
                                after_try,
                                before_finally,
                                self.current.clone(),
                            );
                        }
                    }
                } else {
                    // No finally: the try body's end covers its intervals
                    // when no catch clause ran (official `flowEnd` on the
                    // body); catch windows closed at their own ends.
                    self.flow_end(crate::interp::Covered::Strict);
                    match self.stack.pop() {
                        Some(Frame::TryCatch { after, .. }) => {
                            let joined = self.join(after, Some(self.current.clone()));
                            self.current = joined.unsplit(&mut self.reach);
                        }
                        other => {
                            // A frame-less close (unbalanced recovery buffer)
                            // is a no-op; a mismatched frame goes back.
                            if let Some(frame) = other {
                                self.stack.push(frame);
                            }
                        }
                    }
                }
            }
            Ev::SwitchBegin { span } => self.dead_code(*span),
            Ev::SwitchBlockBegin => {
                if self.collecting() {
                    return true;
                }
                // The scrutinee expression value is on the stack.
                self.exprs.pop();
                // An unmatched begin (parser recovery) scopes no block
                // events: the block range stays empty.
                let end = match_of[i];
                let block = if end == NIL { i..i } else { i..end as usize };
                let checkpoint = self.current.reach;
                self.stack.push(Frame::Switch {
                    is_expr: false,
                    checkpoint,
                    previous: self.current.clone(),
                    unmatched: self.current.clone(),
                    break_model: None,
                    case_open: false,
                    group_labels: false,
                    group_default: false,
                    group_combined: None,
                    body_depth: None,
                    block,
                });
            }
            Ev::CaseBegin {
                label_count,
                expression_count,
            } => {
                if self.collecting() {
                    return true;
                }
                let Some(frame_index) = self.switch_frame() else {
                    return true;
                };
                // Close the previous case body (official switch_afterCase;
                // no-op when `CaseExprBegin` already closed it).
                self.switch_after_case(frame_index);
                let is_default = *expression_count == 0;
                if let Frame::Switch {
                    group_labels,
                    group_default,
                    case_open,
                    ..
                } = &mut self.stack[frame_index]
                {
                    *group_labels |= *label_count > 0;
                    *group_default |= is_default;
                    // The clause heads opened this group through
                    // `case_pattern_end` (official switch_endAlternative);
                    // a default clause has no head events, so it opens the
                    // group here — official `default` runs its
                    // endAlternative with a true guard
                    // (flow_analysis.dart:1725-1740).
                    *case_open |= is_default;
                }
            }
            Ev::CaseExprBegin => {
                if self.collecting() {
                    return true;
                }
                // Official switch_afterCase closes the previous case body
                // just before the next clause begins (contract at
                // flow_analysis.dart:1656-1684: afterCase follows each
                // body; the next clause's beginAlternative is the first
                // event of that clause).
                if let Some(frame_index) = self.switch_frame() {
                    self.switch_after_case(frame_index);
                }
                self.case_pattern_begin(false);
            }
            Ev::CaseExprEnd { has_guard } => {
                if self.collecting() {
                    return true;
                }
                self.case_pattern_end(*has_guard);
            }
            Ev::SwitchEnd | Ev::CaseEnd { .. } | Ev::CaseNoWhen => {}
            Ev::SwitchBlockEnd => {
                if self.collecting() {
                    return true;
                }
                let Some(frame_index) = self.switch_frame() else {
                    return true;
                };
                self.switch_after_case(frame_index);
                self.switch_statement_end(frame_index);
            }
            Ev::SwitchExprBegin { span } => self.dead_code(*span),
            Ev::SwitchExprBlockBegin => {
                if self.collecting() {
                    return true;
                }
                let _scrutinee = self.exprs.pop();
                let checkpoint = self.current.reach;
                self.stack.push(Frame::Switch {
                    is_expr: true,
                    checkpoint,
                    previous: self.current.clone(),
                    unmatched: self.current.clone(),
                    break_model: None,
                    case_open: false,
                    group_labels: false,
                    group_default: false,
                    group_combined: None,
                    body_depth: None,
                    block: i..i,
                });
            }
            Ev::SwitchExprCaseBegin => {
                if self.collecting() {
                    return true;
                }
                let Some(frame_index) = self.switch_frame() else {
                    return true;
                };
                let unmatched = match &self.stack[frame_index] {
                    Frame::Switch { unmatched, .. } => unmatched.clone(),
                    _ => return true,
                };
                // Each alternative branches from the chained unmatched
                // state.
                self.current = unmatched;
                self.case_pattern_begin(true);
            }
            Ev::SwitchExprCasePattern => {
                if self.collecting() {
                    return true;
                }
                // End of the alternative's pattern (official
                // switch_endAlternative): chain the unmatched state and
                // join the combined group state.
                self.case_pattern_end_expr();
            }
            Ev::SwitchExprCaseEnd { has_guard } => {
                if self.collecting() {
                    return true;
                }
                // Official decision order (flow_analysis.dart:1776-1786):
                // pattern → guard → `switch_endAlternative` → body. The
                // parser delivers the guard after the pattern-close anchor
                // and fires no guard events of its own there (official
                // parser_impl.dart:12457-12466), so the guard's value
                // arrives on the expression stack and `_popPattern`
                // (flow_analysis.dart:9324-9332) runs here: the value's
                // flow state is the guard-true branch and guard-false
                // chains into the unmatched state the later alternatives
                // branch from.
                let mut guard_false = None;
                if *has_guard {
                    let _value = self.exprs.pop();
                    let guard = self.exprs.pop();
                    if let Some(guard) = guard {
                        let (if_true, if_false) = self.cond_split(&guard);
                        self.current = if_true;
                        guard_false = Some(if_false);
                    }
                } else {
                    let _value = self.exprs.pop();
                }
                if let Some(frame_index) = self.switch_frame() {
                    let chained = match (&guard_false, &self.stack[frame_index]) {
                        (Some(if_false), Frame::Switch { unmatched, .. }) => {
                            Some(self.join(Some(unmatched.clone()), Some(if_false.clone())))
                        }
                        _ => None,
                    };
                    let prior = match &mut self.stack[frame_index] {
                        Frame::Switch {
                            unmatched,
                            group_combined,
                            ..
                        } => {
                            if let Some(chained) = chained {
                                *unmatched = chained;
                            }
                            group_combined.take()
                        }
                        _ => None,
                    };
                    let combined = self.current.clone();
                    let joined = self.join(prior, Some(combined));
                    if let Frame::Switch { group_combined, .. } = &mut self.stack[frame_index] {
                        *group_combined = Some(joined);
                    }
                }
            }
            Ev::SwitchExprBlockEnd => {
                if self.collecting() {
                    return true;
                }
                let Some(frame_index) = self.switch_frame() else {
                    return true;
                };
                let (combined, previous) = match &self.stack[frame_index] {
                    Frame::Switch {
                        group_combined,
                        previous,
                        ..
                    } => (group_combined.clone(), previous.clone()),
                    _ => return true,
                };
                self.stack.remove(frame_index);
                // Exhaustiveness of switch expressions is unmounted (needs
                // the resolver); an unmatched-hole joins nothing. A
                // recovered case-less switch has no alternatives: carry the
                // entry state instead of joining two absent states.
                self.current = self
                    .join(combined.or(Some(previous.clone())), None)
                    .inherit_tested(&previous, &mut self.lists);
                // Net one value for the consumer (crate contract, see
                // `ListLitEnd`/`ExprBody`): without it `var y = switch
                // (x) {...}` finds no rhs at `InitializerEnd` and `x =
                // switch (x) {...}` pops the assignment target as rhs.
                self.exprs.push(Expr::trivial(Ty::UNKNOWN));
            }
            Ev::AssertBegin { span } => {
                self.dead_code(*span);
                if self.collecting() {
                    return true;
                }
                let exprs_base = self.exprs.len();
                self.stack.push(Frame::Assert {
                    previous: self.current.clone(),
                    exprs_base,
                });
            }
            Ev::AssertEnd => {
                if self.collecting() {
                    return true;
                }
                let Some(Frame::Assert {
                    previous,
                    exprs_base,
                }) = self.stack.pop()
                else {
                    return true;
                };
                // The first expression since begin is the condition; the
                // message (if any) ran after it and its effects are
                // dropped, matching the official join of previous &
                // condTrue only.
                let cond_true = self.exprs.get(exprs_base).and_then(|cond| {
                    cond.branches
                        .as_ref()
                        .map(|(if_true, _)| if_true.clone())
                        .or(cond.bool_value.filter(|&b| b).map(|_| self.current.clone()))
                });
                self.exprs.truncate(exprs_base);
                let joined = self.join(Some(previous), cond_true);
                self.current = joined.unsplit(&mut self.reach);
            }
            Ev::VarDeclBegin {
                late,
                is_final,
                span,
            } => {
                self.header_dead_check(*span);
                self.decl = DeclCtx {
                    late: *late,
                    is_final: *is_final,
                    // The type events follow the keyword; the sink fills
                    // this in (default for `var`).
                    ty: Ty::UNKNOWN,
                };
                self.in_decl = true;
                // Paired with `VarDeclEnd`: declarations nested inside an
                // initializer stack their pending inits on top of the
                // enclosing one (a flat list let the inner head's end
                // clobber the enclosing pending write).
                self.pending_decl_bases.push(self.pending_inits.len());
            }
            Ev::InitializedIdent { name } => {
                let meta = VarMeta {
                    ty: self.decl.ty,
                    late: self.decl.late,
                    is_final: self.decl.is_final,
                    decl_span: None,
                };
                // Declared before the initializer runs (official
                // `declare(initialized: false)` — a self-reference reads
                // definitely unassigned).
                let var = self.declare_var(*name, meta, false);
                self.pending_inits.push(var);
            }
            Ev::NoInitializer => {}
            Ev::InitializerEnd => {
                let var = self.pending_inits.pop();
                let rhs = self.exprs.pop();
                if let (Some(var), Some(rhs)) = (var, rhs) {
                    self.write_var(var, rhs.ty, true);
                }
            }
            Ev::VarDeclEnd => {
                self.decl = DeclCtx::default();
                self.in_decl = false;
                if let Some(base) = self.pending_decl_bases.pop() {
                    self.pending_inits.truncate(base);
                }
            }
            Ev::Formal { name, .. } => {
                let ty = self.pending_ty.take().unwrap_or(Ty::UNKNOWN);
                let meta = VarMeta {
                    ty,
                    late: false,
                    is_final: false,
                    decl_span: None,
                };
                self.declare_var(*name, meta, true);
            }
            Ev::PatternVarDeclStmt { span } => self.header_dead_check(*span),
            Ev::Block { kind, span } => {
                if matches!(kind, BlockKind::Statement) {
                    self.dead_code(*span);
                }
                self.scopes.push(Scope::new());
                self.stack.push(Frame::Block {
                    scopes_base: self.scopes.len() - 1,
                });
            }
            Ev::EndBlock { end } => {
                if let Some(Frame::Block { scopes_base }) = self.stack.last() {
                    let scopes_base = *scopes_base;
                    // A statement block's own end trims the interval to the
                    // official last-statement end; a construct's body block
                    // (loop/try/catch/branch/switch — the parent frame
                    // below) is the covering node itself and never extends.
                    // The stack can be shallow here — the outermost
                    // executable records no Exec frame — so the parent is
                    // looked up guarded; a missing parent is a statement
                    // block.
                    let parent = self
                        .stack
                        .len()
                        .checked_sub(2)
                        .and_then(|index| self.stack.get(index));
                    let is_body_block = matches!(
                        parent,
                        Some(
                            Frame::While { .. }
                                | Frame::TryCatch { .. }
                                | Frame::TryFinally { .. }
                                | Frame::If { .. }
                                | Frame::Switch { .. }
                        )
                    );
                    if !is_body_block {
                        if let Some(window) = &mut self.dead {
                            window.extend(*end);
                        }
                    }
                    self.stack.pop();
                    self.scopes.truncate(scopes_base);
                }
            }
            Ev::BlockBodyBegin | Ev::BlockBodyEnd | Ev::EmptyBody => {}
            _ => return false,
        }
        true
    }

    /// Dead-code check outside for headers (header slices skip statement
    /// semantics).
    fn header_dead_check(&mut self, span: TokenSpan) {
        if self.for_header_depth == 0 {
            self.dead_code(span);
        }
    }

    /// The innermost switch frame index, if any.
    pub(crate) fn switch_frame(&self) -> Option<usize> {
        self.stack
            .iter()
            .rposition(|f| matches!(f, Frame::Switch { .. }))
    }

    /// One-event lookahead: closes the current case alternative group
    /// before its first body event (official `switch_endAlternatives`).
    pub(crate) fn maybe_close_case_group(&mut self, events: &[Ev], match_of: &[u32]) {
        // The close runs its own collect over the switch block, and the
        // frame keeps `case_open` until it returns — a collect run's
        // re-stepped events would re-enter here for the same still-open
        // group and recurse without bound.
        if self.collect.is_some() {
            return;
        }
        let Some(frame_index) = self.stack.iter().rposition(|f| {
            matches!(
                f,
                Frame::Switch {
                    case_open: true,
                    ..
                }
            )
        }) else {
            return;
        };
        let (labeled, block, combined, default, chain) = match &self.stack[frame_index] {
            Frame::Switch {
                group_labels,
                block,
                group_combined,
                group_default,
                unmatched,
                ..
            } => (
                *group_labels,
                block.clone(),
                group_combined.clone(),
                *group_default,
                unmatched.clone(),
            ),
            _ => return,
        };
        if labeled {
            // Labeled case bodies can be jumped into from anywhere in the
            // switch: conservatively join every write in the block.
            let effects = self.collect_ranges(events, &[block.start..block.end], match_of);
            let previous = match &self.stack[frame_index] {
                Frame::Switch { previous, .. } => previous.clone(),
                _ => return,
            };
            let finals = self.finals_of(&effects.written);
            let joined = previous.conservative_join(&effects.written, &effects.captured, &finals);
            self.current = joined;
        } else {
            // Official `switch_endAlternatives`
            // (flow_analysis.dart:8044-8066): the body runs under the
            // group's combined matched state; with no contributing clause
            // it runs under the chained unmatched state. A default clause
            // contributes the chain as its own alternative state (official
            // `default` runs its endAlternative with a true guard, whose
            // guard-true state is the chain), so a default-bearing group's
            // body joins both.
            let body = match combined {
                Some(state) if default => self.join(Some(state), Some(chain)),
                Some(state) => state,
                None => chain,
            };
            self.current = body;
        }
        if default {
            // The default alternative always matches: no unmatched state
            // survives the group (official isExhaustive=true effect).
            if let Frame::Switch { unmatched, .. } = &mut self.stack[frame_index] {
                *unmatched = unmatched.clone().set_unreachable(&mut self.reach);
            }
        }
        // Control-flow split so the case body's fall-through is observable
        // at afterCase.
        self.split_cur();
        let depth = self.stack.len();
        if let Frame::Switch {
            case_open,
            body_depth,
            ..
        } = &mut self.stack[frame_index]
        {
            *case_open = false;
            *body_depth = Some(depth);
        }
    }

    /// Official `switch_afterCase`: if the case body fell through its end,
    /// join it into the break model (an implicit break).
    fn switch_after_case(&mut self, frame_index: usize) {
        let body_open = match &self.stack[frame_index] {
            Frame::Switch { body_depth, .. } => body_depth.is_some(),
            _ => false,
        };
        if !body_open {
            return;
        }
        // The case group's end covers its body intervals (official
        // `handleMergedStatementCase`'s `flowEnd` on the member): one
        // window per member, never across groups.
        self.flow_end(crate::interp::Covered::Body);
        let reachable = self.reachable();
        if reachable {
            let exit = self.current.clone();
            let prior = match &mut self.stack[frame_index] {
                Frame::Switch { break_model, .. } => break_model.take(),
                _ => None,
            };
            let joined = self.join(prior, Some(exit));
            if let Frame::Switch { break_model, .. } = &mut self.stack[frame_index] {
                *break_model = Some(joined);
            }
        }
        if let Frame::Switch {
            body_depth,
            group_combined,
            ..
        } = &mut self.stack[frame_index]
        {
            *body_depth = None;
            *group_combined = None;
        }
    }

    /// Pops the switch frame at SwitchBlockEnd: joins the break model and
    /// the unmatched chain (official `switch_end`, isExhaustive always
    /// false — exhaustiveness needs the resolver).
    fn switch_statement_end(&mut self, switch_index: usize) {
        let (unmatched, previous, break_model) = match &self.stack[switch_index] {
            Frame::Switch {
                unmatched,
                previous,
                break_model,
                ..
            } => (unmatched.clone(), previous.clone(), break_model.clone()),
            _ => return,
        };
        self.stack.remove(switch_index);
        let exit = self.join(break_model, Some(unmatched));
        self.current = exit.inherit_tested(&previous, &mut self.lists);
    }

    /// Statement-position if-case: the pattern's matched state becomes the
    /// then-branch state (official ifCaseStatement_endAlternative).
    /// Pattern-declared variables already live in the enclosing scope from
    /// their declaration events.
    fn if_case_then(&mut self) {
        let pattern = self.pattern.take().expect("if-case pattern");
        let mut matched = self.current.clone();
        if let Some(guard) = pattern.guard {
            let (if_true, _) = self.cond_split(&guard);
            matched = if_true;
        }
        self.stack.push(Frame::If {
            branch: pattern.unmatched,
            after_then: None,
        });
        self.current = matched;
    }
}

/// The official `attachFinally` at the state level: reachability follows
/// the finally's exit; per-variable promotion models follow the version
/// compare (identical before/after models keep the try promotions).
fn attach_finally(
    interp: &mut Interp<'_>,
    after_try: State,
    before_finally: State,
    after_finally: State,
) -> State {
    let vars = interp.vars.len();
    let reachable_after = interp.reach.locally_reachable(after_finally.reach);
    let mut models = Vec::with_capacity(vars);
    for index in 0..vars {
        let var = index as u32;
        let v1 = after_try.model(var);
        let v2 = before_finally.model(var);
        let v3 = after_finally.model(var);
        // The before-finally model participates only when the variable was
        // already in a distinct state there (the official domain diff).
        let before = if v2 == v1 { None } else { Some(v2) };
        models.push(attach_finally_model(&interp.lists, v1, before, v3));
    }
    let reach = if reachable_after {
        after_try.reach
    } else {
        interp.reach.set_unreachable(after_try.reach)
    };
    State { reach, models }
}
