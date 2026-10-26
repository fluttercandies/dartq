//! Statement events: conditionals, loops, try/catch/finally, switch
//! statements, jumps, declarations, and asserts — in the official
//! decision order.

use dartq_parser::TokenSpan;
use dartq_source::saturate_slot_u32;

use crate::ev::{BlockKind, Ev};
use crate::facts::FlowFact;
use crate::interp::{
    DeclCtx, Effects, Expr, Frame, Interp, LabeledEffects, PatternKind, VarMeta,
};
use crate::model::{NIL, State, attach_finally_model};
use crate::ty::Ty;

impl Interp<'_> {
    /// Handles one statement-layer event; `false` means the event belongs
    /// to another layer. Statement-boundary bookkeeping wraps the arm:
    /// every non-wrapper arm refreshes [`Interp::stmt_began_dead`] after it
    /// runs (the reachability then is the state the NEXT statement begins
    /// in), while the post-order wrapper arms read it instead.
    pub(crate) fn stmt_step(&mut self, ev: &Ev, events: &[Ev], i: usize, match_of: &[u32]) -> bool {
        // The wrapper arms fire at the statement's END (post-order): they
        // must observe the state the statement BEGAN in, so only the other
        // arms refresh the snapshot.
        let wrapper = matches!(
            ev,
            Ev::ExprStmt { .. }
                | Ev::ReturnBegin { .. }
                | Ev::YieldBegin { .. }
                | Ev::PatternVarDeclStmt { .. }
        );
        let handled = self.stmt_step_arm(ev, events, i, match_of);
        if handled && !wrapper && !self.collecting() {
            self.stmt_began_dead = !self.reachable();
        }
        handled
    }

    /// The statement decision order proper (see [`Interp::stmt_step`]).
    #[expect(
        clippy::too_many_lines,
        reason = "single port of the official statement decision order; splitting the \
                  match would scatter the per-event documentation"
    )]
    fn stmt_step_arm(&mut self, ev: &Ev, events: &[Ev], i: usize, match_of: &[u32]) -> bool {
        match ev {
            Ev::ExprStmt { span } => {
                self.header_dead_check(*span);
                // The statement-level expression ends: the null-shorting
                // its own `?` opened rejoins here (official
                // `finishNullShorting` at the expression boundary) — a
                // `throw`/Never inside the shorted tail must not kill the
                // receiver-was-null path.
                self.flush_null_shorts_above(self.consumer_base(1));
                // Inside a `for` header the statement wrapper is an
                // artifact of the part's parsing: its value feeds the
                // header replay, so it stays on the stack.
                if self.for_header_depth == 0 {
                    let value = self.exprs.pop();
                    // A dropped condition-valued statement expression
                    // (`a != null;`) rejoins its split.
                    if let Some(value) = &value {
                        self.rejoin_value_split(value);
                    }
                }
            }
            Ev::StmtEnd { end } => {
                // A statement completed: an open dead-code interval runs
                // to the statement's end, its trailing `;` included (the
                // official close trims the covering list to its last
                // statement and reports `node.end - offset` —
                // dead_code_verifier.dart:253-279, :326-332).
                if let Some(window) = &mut self.dead {
                    window.extend(*end);
                }
            }
            Ev::IfBegin { span } | Ev::SwitchBegin { span } | Ev::SwitchExprBegin { span } => {
                self.dead_code(*span);
            }
            // `end_return_statement`/`end_yield_statement` record these at
            // the statement's END: gate on the began-dead snapshot, never
            // the live state (`return throw 1;` must not self-anchor).
            Ev::ReturnBegin { span } | Ev::YieldBegin { span } => {
                self.wrapper_dead_check(*span);
            }
            Ev::IfCaseCondition => self.if_case_condition(),
            Ev::ThenBegin => {
                // If-case: the condition's own marker converted the open
                // pattern context, and the pattern's matched state becomes
                // the branch state.
                if self.pattern.as_ref().map(|p| p.kind) == Some(PatternKind::IfCase) {
                    self.if_case_then();
                    return true;
                }
                // A `Decl` context still open at this point carrying a
                // scrutinee is this if's own case clause: every non-clause
                // owner (pattern variable declaration `PatternVarDeclStmt`,
                // for-in head `ForInParts`, pattern assignment) seals its
                // context at its own end event, and a bare `if (cond)`
                // cannot open one — the condition is a plain expression.
                // The recorded path's `IfCaseCondition` marker flips the
                // kind a step early; a replay stream that never forwards
                // `handle_parenthesized_condition` reaches here with the
                // kind still `Decl` (#2401: the semantic listener drops
                // `case_kw`), so the clause face must not hinge on the
                // marker's arrival. Adopt it the same way — the unmatched
                // chain carries the fall-through promotion the `case null`
                // arm wrote there.
                if self.pattern.as_ref().map(|p| p.kind) == Some(PatternKind::Decl)
                    && self.pattern.as_ref().and_then(|p| p.scrutinee).is_some()
                {
                    if let Some(pattern) = self.pattern.as_mut() {
                        pattern.kind = PatternKind::IfCase;
                    }
                    self.if_case_then();
                    return true;
                }
                // The condition ends: the null-shorting its own `?`
                // opened rejoins before the branch split (official
                // `finishNullShorting`).
                self.flush_null_shorts_above(self.consumer_base(1));
                // A standalone pattern context still open here belongs to a
                // construct that missed its close (a declaration statement
                // or assignment whose end event never arrived). It must not
                // leak into this if: the official pushes one context per
                // construct, and adopting the stale one deaded the else
                // branch after a for-in pattern head (#2288).
                self.close_standalone_pattern();
                let cond = self.pop_value();
                // The condition's own query (official
                // `handle_ifStatement_conditionEnd`, type_analyzer.dart:1191).
                self.condition_operand(&cond);
                let (if_true, if_false) = self.cond_split(&cond);
                self.stack.push(Frame::If {
                    branch: if_false,
                    after_then: None,
                    shadowed: Vec::new(),
                });
                self.current = if_true;
            }
            Ev::ElseBegin => {
                // The then branch's end covers its intervals (official
                // `handle_ifStatement_thenEnd`'s `flowEnd(ifTrue)`,
                // type_analyzer.dart:1199-1206). The containment gate
                // accepts the close node itself (`_containsFirstDeadNode`,
                // dead_code_verifier.dart:453-459 — the walk starts at the
                // first dead node and matches on equality), so a wholly
                // dead branch anchored at its own block is covered:
                // `Covered::Body`'s `>=`, not `Strict`.
                self.flow_end(crate::interp::Covered::Body);
                let after_then = self.current.clone();
                // The then branch's scope ends: an if-case's pattern
                // variables leave the lexical scope here.
                let shadowed = match self.stack.last_mut() {
                    Some(Frame::If {
                        branch,
                        after_then: slot,
                        shadowed,
                    }) => {
                        *slot = Some(after_then);
                        self.current = branch.clone();
                        std::mem::take(shadowed)
                    }
                    _ => Vec::new(),
                };
                self.restore_bindings(&shadowed);
                // Entering the else branch adopts the pre-then state: when
                // it is alive over a dead-from-exit current, the exit's
                // anchoring duty ends with the then region ([`Interp::join`]'s
                // retirement rule, without a join call here).
                if self.reachable() {
                    self.exit_at = None;
                }
            }
            Ev::IfEnd { has_else } => {
                // The last branch's end covers its intervals (official
                // `handle_ifStatement_thenEnd` without an else, else
                // `handle_ifStatement_elseEnd`). As at `ElseBegin`, the
                // official containment accepts the close node itself
                // (dead_code_verifier.dart:453-459), covering a wholly
                // dead branch anchored at its own block (`Covered::Body`).
                self.flow_end(crate::interp::Covered::Body);
                let Some(Frame::If {
                    branch,
                    after_then,
                    shadowed,
                }) = self.stack.pop()
                else {
                    return true;
                };
                self.restore_bindings(&shadowed);
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
                    self.collect_ranges(events, i..end as usize, match_of)
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
                    for_in: false,
                });
            }
            Ev::WhileBodyBegin => {
                if self.collecting() {
                    // The condition's value leaves the stack here in every
                    // mode (the leak note at `SwitchBlockBegin`).
                    self.flush_null_shorts_above(self.consumer_base(1));
                    self.exprs.pop();
                    return true;
                }
                // The condition ends: rejoin the null-shorting its own `?`
                // opened before the branch split.
                self.flush_null_shorts_above(self.consumer_base(1));
                let cond = self.exprs.pop();
                // The condition's own query (official `visitWhileStatement`,
                // type_analyzer.dart:4889).
                if let Some(cond) = &cond {
                    self.condition_operand(cond);
                }
                let cond = cond.unwrap_or_else(|| Expr::trivial(Ty::UNKNOWN));
                let (if_true, if_false) = self.cond_split(&cond);
                if let Some(Frame::While { cond_false, .. }) = self.stack.last_mut() {
                    *cond_false = Some(if_false);
                }
                self.current = if_true;
            }
            Ev::WhileEnd => {
                // The loop statement's end covers body intervals (official
                // `visitWhileStatement`'s `flowEnd(node.body)`,
                // type_analyzer.dart:4901). The containment gate accepts the
                // close node itself (`_containsFirstDeadNode`,
                // dead_code_verifier.dart:453-459 — the walk starts at the
                // first dead node and matches on equality), so the at-depth
                // body band is covered: the body block's `{` and a non-block
                // body statement carry the loop's frame depth (the body keeps
                // no frame of its own), while the `while` keyword's anchor
                // sits one depth below and stays open for the outer covering
                // close. No offset gate is needed: reachability is unchanged
                // between `WhileBegin` and `WhileBodyBegin`, so the condition
                // can never be the first dead node — an entry-dead loop
                // anchors at the keyword first.
                self.flow_end(crate::interp::Covered::Body);
                self.loop_exit(false);
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
                    self.collect_ranges(events, i..end as usize, match_of)
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
                    for_in: false,
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
                    // The do-while condition's value leaves the stack here
                    // in every mode (the leak note at `SwitchBlockBegin`).
                    self.flush_null_shorts_above(self.consumer_base(1));
                    self.exprs.pop();
                    return true;
                }
                // The condition ends: rejoin the null-shorting its own `?`
                // opened before the branch split.
                self.flush_null_shorts_above(self.consumer_base(1));
                let cond = self.exprs.pop();
                // The do-while condition's own query (official
                // `visitDoStatement`, type_analyzer.dart:2817).
                if let Some(cond) = &cond {
                    self.condition_operand(cond);
                }
                let cond = cond.unwrap_or_else(|| Expr::trivial(Ty::UNKNOWN));
                let (_, if_false) = self.cond_split(&cond);
                if let Some(Frame::While { cond_false, .. }) = self.stack.last_mut() {
                    *cond_false = Some(if_false);
                }
                self.loop_exit(false);
                // Attach the `while (c);` part to the interval the do's
                // back edge opened, and drop the tail row when a break
                // makes the code after the do-while reachable (official
                // `hasBreakStatement`). The LOCAL flag: a break path is a
                // real control path even when the statement is entry-dead
                // (the official test is syntactic, not `isReachable`).
                let tail_reachable = self.reach.locally_reachable(self.current.reach);
                if let Some(window) = &mut self.dead
                    && window.awaiting_split()
                {
                    window.attach_split(*while_range, tail_reachable);
                }
            }
            Ev::ReturnEnd { has_expression } => {
                if *has_expression {
                    // The returned expression ends: rejoin the
                    // null-shorting its own `?` opened before the value is
                    // consumed.
                    self.flush_null_shorts_above(self.consumer_base(1));
                    self.exprs.pop();
                }
                // Official handleReturn: the current path becomes
                // unreachable (inside closures too).
                self.unreachable_cur();
            }
            Ev::YieldEnd { span } => {
                self.flush_null_shorts_above(self.consumer_base(1));
                self.exprs.pop();
                self.suspend(*span);
            }
            Ev::AwaitEnd { span } => {
                // The awaited operand ends: rejoin the null-shorting its
                // own `?` opened before the suspension (official
                // `finishNullShorting` at the operand boundary).
                self.flush_null_shorts_above(self.consumer_base(1));
                self.exprs.pop();
                self.exprs.push(Expr::trivial(Ty::UNKNOWN));
                self.suspend(*span);
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
                let target = self.find_continue_target(*label);
                match target {
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
                    self.collect_ranges(events, i + 1..body_end, match_of)
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
                self.push_scope();
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
                    // Models AND property slots sweep together: a receiver
                    // written in the try body kills the promotions tested
                    // before the try on the catch path too (official
                    // `FlowModel.conservativeJoin`,
                    // flow_analysis.dart:3786-3830, invoked from
                    // `tryCatchStatement_bodyEnd`, :8163-8181).
                    let state = self.join_state_with_sweep(previous, &written, &captured, None);
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
                // A live exception-entry state over a dead-from-exit current
                // ends the exit's anchoring duty (same retirement rule as
                // [`Interp::join`] / `ElseBegin`).
                if self.reachable() {
                    self.exit_at = None;
                }
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
                // state conservatively over the try body's writes. The
                // receivers written in the try body invalidate their
                // property promotions on the exception path too (the
                // write may have happened before the exception; #1409).
                let entry = self.join_state_with_sweep(pre, &wrote, &captured, None);
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
                                &after_try,
                                &before_finally,
                                &self.current.clone(),
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
            Ev::SwitchBlockBegin => {
                if self.collecting() {
                    // The scrutinee's value leaves the stack at this event
                    // in every mode: a collect run that skips the pop leaks
                    // the slot, and every later pop — an assignment target
                    // reading the wrong expression — silently stops
                    // recording its write, so the walk's AssignedVariables
                    // sets come back empty (a closure containing a switch
                    // then demotes nothing at later closure entries).
                    self.flush_null_shorts_above(self.consumer_base(1));
                    self.exprs.pop();
                    return true;
                }
                // The scrutinee ends: rejoin null-shorting opened inside it
                // before its value is read (`flush_null_shorts` keeps any
                // enclosing `?` chain open).
                self.flush_null_shorts_above(self.consumer_base(1));
                // The scrutinee expression value is on the stack; its
                // variable (when the scrutinee is a bare read) is the
                // promotion key the matched-type rows answer against
                // (#1349). Its flow type and written arguments feed the
                // `switch_end` always-exhaustive verdict
                // ([`Interp::is_always_exhaustive`]) — upstream's
                // `scrutineeType` is the promoted static type, which the
                // value's own row already carries.
                let (scrutinee, scrutinee_ty, scrutinee_args) = self
                    .exprs
                    .pop()
                    .map_or((None, Ty::UNKNOWN, crate::ty::EMPTY_ARGS), |e| {
                        (e.reference, e.ty, e.args)
                    });
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
                    case_shadowed: Vec::new(),
                    scrutinee,
                    scrutinee_ty,
                    scrutinee_args,
                    pending_matched: None,
                    expr_case_tys: Vec::new(),
                    case_open: false,
                    group_labels: false,
                    group_default: false,
                    group_combined: None,
                    body_depth: None,
                    dead_default: None,
                    reported_kw: None,
                    block,
                    labeled_effects: None,
                });
            }
            Ev::CaseBegin {
                label_count,
                expression_count,
                kw,
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
                // The previous group's default feeds this group's opening:
                // the chained unmatched state goes unreachable (the
                // default's guard-true, no-pattern alternative end). No
                // keyword row here — a default-headed opener stays silent
                // (probe table G rule 2: s7's first default stays live).
                self.consume_dead_default(frame_index, false);
                if let Some(anchor) = kw.filter(|_| is_default) {
                    // The member check of a default head (official
                    // `switch_beginAlternative` + `checkUnreachableNode`):
                    // run the head under the chained unmatched state.
                    let chain = match &self.stack[frame_index] {
                        Frame::Switch { unmatched, .. } => unmatched.clone(),
                        _ => self.current.clone(),
                    };
                    self.current = chain;
                    self.dead_code(anchor);
                    // `handleDefault`'s `flowEnd(member)`: a member anchored
                    // at its own keyword reports there and resets (official
                    // dead_code_verifier.dart:232-235) — the body then opens
                    // a fresh interval.
                    self.report_member_head(anchor);
                }
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
            Ev::CaseExprBegin { span } => {
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
                    // The previous group's default: the chain goes
                    // unreachable and — a case-headed opener — the default's
                    // own keyword row fires (probe table G rule 2).
                    self.consume_dead_default(frame_index, true);
                }
                self.case_pattern_begin(false);
                // The member check (official `checkUnreachableNode(member)`
                // right after `switch_beginAlternative`): a dead member
                // anchors at its `case` keyword.
                self.dead_code(*span);
            }
            Ev::CaseExprEnd { has_guard, kw } => {
                if self.collecting() {
                    return true;
                }
                // The matched-value query fires at the pattern match,
                // before the clause's own pattern and guard touch anything
                // (official `getMatchedValueType` inside the pattern walk;
                // the frame's chain below is the match-time one — only
                // `case_pattern_end` writes it).
                if let Some(frame_index) = self.switch_frame()
                    && let Some((ty, args)) = self.matched_type_at(frame_index)
                {
                    self.emit(FlowFact::MatchedType {
                        span: *kw,
                        ty,
                        args,
                    });
                }
                // `handleCaseHead`'s `flowEnd(member)` runs before
                // `switch_endAlternative`: a member anchored at its own
                // keyword reports there and resets.
                self.report_member_head(*kw);
                self.case_pattern_end(*has_guard);
            }
            Ev::SwitchEnd
            | Ev::CaseNoWhen
            | Ev::NoInitializer
            | Ev::BlockBodyBegin
            | Ev::BlockBodyEnd
            | Ev::EmptyBody => {}
            Ev::CaseEnd {
                default_kw,
                statement_count: _,
            } => {
                if self.collecting() {
                    return true;
                }
                // Latch the group's default (if any) for the next group's
                // opening (probe table G rules 1-2); a default whose
                // head-time member row already fired is latched reported so
                // the group close never re-reports it.
                if let Some(frame_index) = self.switch_frame() {
                    let reported = match &self.stack[frame_index] {
                        Frame::Switch { reported_kw, .. } => reported_kw.is_some_and(|offset| {
                            default_kw.is_some_and(|kw| kw.range.offset == offset)
                        }),
                        _ => false,
                    };
                    if let Frame::Switch { dead_default, .. } = &mut self.stack[frame_index] {
                        *dead_default = default_kw.map(|kw| (kw, reported));
                    }
                    // A member carrying a `default` head exhausts the
                    // unmatched chain the moment its clause ends: the
                    // default's alternative matches everything the earlier
                    // heads left (official `switch_endAlternative` of the
                    // merged member's default head). A `default` stacked
                    // after case heads (`case 3: default:`) folds into that
                    // case-headed member (official
                    // `getSwitchStatementMemberInfo`), so its `CaseBegin`
                    // carries `expression_count > 0` — the group close ran
                    // without the default's chain kill, and the clause end
                    // is the latest observable point. For a default-headed
                    // member the close already killed the chain; the kill
                    // is idempotent there.
                    if default_kw.is_some()
                        && let Frame::Switch { unmatched, .. } = &mut self.stack[frame_index]
                    {
                        *unmatched = unmatched.clone().set_unreachable(&mut self.reach);
                    }
                }
            }
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
            Ev::SwitchExprBlockBegin => {
                if self.collecting() {
                    // The scrutinee's value leaves the stack here in every
                    // mode (the leak note at `SwitchBlockBegin`).
                    self.flush_null_shorts_above(self.consumer_base(1));
                    self.exprs.pop();
                    return true;
                }
                // The scrutinee ends: rejoin null-shorting opened inside it
                // (`flush_null_shorts` keeps any enclosing `?` chain open).
                self.flush_null_shorts_above(self.consumer_base(1));
                // The scrutinee's variable is the matched-type rows'
                // promotion key (#1349).
                let scrutinee = self.exprs.pop().and_then(|e| e.reference);
                let checkpoint = self.current.reach;
                self.stack.push(Frame::Switch {
                    is_expr: true,
                    checkpoint,
                    previous: self.current.clone(),
                    unmatched: self.current.clone(),
                    break_model: None,
                    case_shadowed: Vec::new(),
                    scrutinee,
                    scrutinee_ty: Ty::UNKNOWN,
                    scrutinee_args: crate::ty::EMPTY_ARGS,
                    pending_matched: None,
                    expr_case_tys: Vec::new(),
                    case_open: false,
                    group_labels: false,
                    group_default: false,
                    group_combined: None,
                    body_depth: None,
                    dead_default: None,
                    reported_kw: None,
                    block: i..i,
                    labeled_effects: None,
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
                // The matched-value query fires at the pattern match: the
                // chain below is the match-time one (`case_pattern_end_expr`
                // updates it right after), and the clause's anchor token
                // only arrives at `SwitchExprCaseEnd` — latch the row there
                // (#1349).
                if let Some(frame_index) = self.switch_frame()
                    && let Some(row) = self.matched_type_at(frame_index)
                    && let Frame::Switch {
                        pending_matched, ..
                    } = &mut self.stack[frame_index]
                {
                    *pending_matched = Some(row);
                }
                // End of the alternative's pattern (official
                // switch_endAlternative): chain the unmatched state and
                // join the combined group state.
                self.case_pattern_end_expr();
            }
            Ev::SwitchExprCaseEnd { has_guard, begin } => {
                if self.collecting() {
                    // The case value — and the guard's value behind it,
                    // when present — leaves the stack here in every mode
                    // (the leak note at `SwitchBlockBegin`).
                    self.exprs.pop();
                    if *has_guard {
                        self.exprs.pop();
                    }
                    return true;
                }
                // The latched match-time row (see `SwitchExprCasePattern`).
                if let Some(frame_index) = self.switch_frame() {
                    let pending = match &mut self.stack[frame_index] {
                        Frame::Switch {
                            pending_matched, ..
                        } => pending_matched.take(),
                        _ => None,
                    };
                    if let Some((ty, args)) = pending {
                        self.emit(FlowFact::MatchedType {
                            span: *begin,
                            ty,
                            args,
                        });
                    }
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
                // branch from. Null-shorting opened inside the case value
                // (and guard) rejoins here; an enclosing `?` chain stays
                // open.
                self.flush_null_shorts_above(self.consumer_base(usize::from(*has_guard) + 1));
                let mut guard_false = None;
                // The body value is the stack's top (the guard's value sits
                // below it): its type feeds the block-end arm fold.
                let case_value_ty;
                if *has_guard {
                    case_value_ty = self.exprs.pop().map(|e| e.ty);
                    let guard = self.exprs.pop();
                    if let Some(guard) = guard {
                        let (if_true, if_false) = self.cond_split(&guard);
                        self.current = if_true;
                        guard_false = Some(if_false);
                    }
                } else {
                    case_value_ty = self.exprs.pop().map(|e| e.ty);
                }
                if let Some(frame_index) = self.switch_frame() {
                    if let Some(ty) = case_value_ty
                        && let Frame::Switch { expr_case_tys, .. } = &mut self.stack[frame_index]
                    {
                        expr_case_tys.push(ty);
                    }
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
                    if let Frame::Switch {
                        group_combined,
                        case_shadowed,
                        ..
                    } = &mut self.stack[frame_index]
                    {
                        *group_combined = Some(joined);
                        // The alternative's binders leave their scope with
                        // the arm (official `switch_endAlternative`'s
                        // member close).
                        let shadowed = std::mem::take(case_shadowed);
                        self.restore_bindings(&shadowed);
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
                let (combined, previous, case_tys) = match &self.stack[frame_index] {
                    Frame::Switch {
                        group_combined,
                        previous,
                        expr_case_tys,
                        ..
                    } => (
                        group_combined.clone(),
                        previous.clone(),
                        expr_case_tys.clone(),
                    ),
                    _ => return true,
                };
                self.switch_scope_close(frame_index);
                self.stack.remove(frame_index);
                // Exhaustiveness of switch expressions is unmounted (needs
                // the resolver); an unmatched-hole joins nothing. A
                // recovered case-less switch has no alternatives: carry the
                // entry state instead of joining two absent states.
                self.current = self
                    .join(combined.or(Some(previous.clone())), None)
                    .inherit_tested(&previous, &mut self.lists);
                // The expression's type: the case-value LUB fold (upstream
                // Dart SDK source, pkg/analyzer/lib/src/dart/ast/ast.dart
                // `SwitchExpressionImpl.staticType`'s arm fold; the
                // name-level join is `??`'s). The plain UNKNOWN this push
                // used to carry left every value consumer untyped — an
                // initializer's inferred local adopted no face and a
                // receiver's flow read lifted nothing.
                let ty = case_tys
                    .into_iter()
                    .reduce(|joined, ty| self.name_lub(joined, ty))
                    .unwrap_or(Ty::UNKNOWN);
                // Net one value for the consumer (crate contract, see
                // `ListLitEnd`/`ExprBody`): without it `var y = switch
                // (x) {...}` finds no rhs at `InitializerEnd` and `x =
                // switch (x) {...}` pops the assignment target as rhs.
                self.exprs.push(Expr::trivial(ty));
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
                    cond_true: None,
                });
            }
            Ev::AssertAfterCondition => {
                if self.collecting() {
                    return true;
                }
                // Official `assert_afterCondition`
                // (flow_analysis.dart:6478-6485): latch the condition's
                // true state for the end's join, then run the message on
                // the FALSE state — `assert(x == null, '${x.entries}')`
                // reads `x` promoted non-null inside the message (the
                // null-check's false arm), and `assert(t != null, ...)`'s
                // message reads `t` null.
                let Some(Frame::Assert { cond_true, .. }) = self.stack.last_mut()
                else {
                    return true;
                };
                let cond = self.exprs.last();
                let (if_true, if_false) = cond
                    .and_then(|c| c.branches.as_ref().map(|(t, f)| (t.clone(), f.clone())))
                    .unwrap_or_else(|| {
                        // A non-branching condition (official
                        // `_makeTrivialExpressionInfo`): both states are the
                        // merged current.
                        let cur = self.current.clone();
                        (cur.clone(), cur)
                    });
                *cond_true = Some(if_true);
                self.current = if_false;
            }
            Ev::AssertEnd => {
                if self.collecting() {
                    return true;
                }
                let Some(Frame::Assert {
                    previous,
                    exprs_base,
                    cond_true,
                }) = self.stack.pop()
                else {
                    return true;
                };
                // The first expression since begin is the condition; the
                // message (if any) ran after it and its effects are
                // dropped, matching the official join of previous &
                // condTrue only. The condition's own query fires before the
                // truncate (official `visitAssertStatement`,
                // type_analyzer.dart:2167; the initializer twin at :2139).
                let cond = self
                    .exprs
                    .get(exprs_base)
                    .map(|c| (c.read_span, c.reference, c.ty, c.args));
                if let Some((span, reference, ty, args)) = cond
                    && let (Some(span), Some(_)) = (span, reference)
                {
                    self.emit(FlowFact::ConditionType { span, ty, args });
                }
                // The latched condition-true state wins when the message
                // boundary ran (the message's own reads left `current` on
                // the false arm — the end's join reads only the latched
                // true arm); a message-less assert reads the condition's
                // true arm off the merged state as before.
                let cond_true = cond_true.or_else(|| {
                    self.exprs.get(exprs_base).and_then(|cond| {
                        cond.branches
                            .as_ref()
                            .map(|(if_true, _)| if_true.clone())
                            .or(cond.bool_value.filter(|&b| b).map(|_| self.current.clone()))
                    })
                });
                // The assert's expressions end: rejoin null-shorting opened
                // inside them before the previous-state join.
                self.flush_null_shorts_above(exprs_base);
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
                // Nested lists swap-and-save the enclosing context (#1051,
                // paired at `VarDeclEnd`) — an inner list's header must not
                // discard the outer list's type for its remaining items.
                let ty = self.pending_ty.take().unwrap_or(Ty::UNKNOWN);
                let args = std::mem::take(&mut self.pending_ty_args);
                let outer = std::mem::replace(
                    &mut self.decl,
                    DeclCtx {
                        late: *late,
                        is_final: *is_final,
                        // The header's annotation arrives as `Type`/`NoType`
                        // events immediately ahead of this event (statements
                        // and fields both parse the type before
                        // `begin_variables_declaration`), parked in
                        // `pending_ty` exactly as the `Formal` and pattern
                        // consumers read it — leaving it there declared
                        // every typed local as unknown, which silenced
                        // null-check promotions over it (#1011). The
                        // `in_decl` sink path below stays as the fallback
                        // for an annotation that follows the begin but
                        // precedes the first identifier; that identifier
                        // seals the slot (`InitializedIdent`), so
                        // initializer-internal `Type` events cannot
                        // redeclare later list items (#1037).
                        ty,
                        args,
                    },
                );
                self.decl_stack.push(outer);
                self.in_decl = true;
                // Paired with `VarDeclEnd`: declarations nested inside an
                // initializer stack their pending inits on top of the
                // enclosing one (a flat list let the inner head's end
                // clobber the enclosing pending write).
                self.pending_decl_bases.push(self.pending_inits.len());
            }
            Ev::InitializedIdent { name, token } => {
                let meta = VarMeta {
                    ty: self.decl.ty,
                    args: self.decl.args,
                    late: self.decl.late,
                    is_final: self.decl.is_final,
                    decl_span: Some(token.range),
                };
                // The declaration's type slot closes with the first variable
                // it declares. A later `Type` event inside the list belongs
                // to a nested construct — a closure parameter's annotation,
                // an `is`/`as` in another item's initializer — not to the
                // header; letting it into the sink re-declared the remaining
                // list items with the leaked type (#1037: the closure
                // parameter itself read unknown in the same stroke). The
                // header annotation always precedes this event (`Type`
                // fires before `VarDeclBegin` in the real stream, and the
                // fallback shape parks it in the slot before the first
                // identifier), so sealing here keeps the header type for
                // the list's remaining items.
                self.in_decl = false;
                // Declared before the initializer runs (official
                // `declare(initialized: false)` — a self-reference reads
                // definitely unassigned).
                let var = self.declare_var(*name, meta, false);
                self.pending_inits.push(var);
            }
            Ev::InitializerBegin => {
                // The variable-initializer bracket (paired at
                // `InitializerEnd`): the cast-type dead poll reads the
                // snapshot depth to leave the initializer-face row to the
                // semantic as-cast tail face.
                self.init_exec_depth = Some(self.exec_frame_depth());
                // A late local's initializer is a deferred context (official
                // `lateInitializer_begin` = `_functionExpression_begin`,
                // flow_analysis.dart:7266-7282: `late x = e` acts as
                // `late x = LAZY_MAGIC(() => e)`): it runs at the first
                // read, not at the declaration, so everything written
                // anywhere reads as maybe-assigned inside it — the same
                // conservative join a closure entry applies — and the
                // initializer's interior flow effects restore at its end
                // instead of leaking into the enclosing statement. State
                // only: the value and the declaration's own initialize
                // write flow through `InitializerEnd` unchanged, and no
                // executable frame opens (no scope, no body-completion
                // fact, no dead-window close — those stay the enclosing
                // statement's).
                if self.decl.late {
                    self.late_init_saved.push(Some(self.current.clone()));
                    let anywhere_written = self.anywhere.written.clone();
                    let anywhere_captured = self.anywhere.captured.clone();
                    self.cjoin(&anywhere_written, &anywhere_captured);
                } else {
                    self.late_init_saved.push(None);
                }
            }
            Ev::LocalFnName { name } => {
                self.local_fns.insert(*name);
            }
            Ev::InitializerEnd => {
                self.init_exec_depth = None;
                // The deferred-context pair closes (the late note at
                // `InitializerBegin`): a late initializer's interior state
                // restores — the declaration's own initialize write below
                // applies to the enclosing state, exactly as the official's
                // `lateInitializer_end` (`_functionExpression_end`) returns
                // to the declaration site before the late local's lazy
                // machinery takes over.
                if let Some(Some(saved)) = self.late_init_saved.pop() {
                    self.current = saved;
                }
                // The initializer expression ends: rejoin null-shorting
                // opened inside it (`var v = x?.y` — official
                // `finishNullShorting` at the initializer's boundary); an
                // enclosing `?` chain stays open.
                self.flush_null_shorts_above(self.consumer_base(1));
                let var = self.pending_inits.pop();
                let rhs = self.exprs.pop();
                // A condition-valued initializer (`final b = a != null;`)
                // consumed as a plain value rejoins its split: the
                // narrowed state must not leak into the next statement.
                if let Some(rhs) = &rhs {
                    self.rejoin_value_split(rhs);
                }
                // The condition-variable capture (language#1274): the
                // initializer IS the null-comparison/type-test split —
                // save its arms keyed on the declared local for the
                // condition reads below.
                if let Some(rhs) = rhs.as_ref()
                    && rhs.test_split
                    && let Some((if_true, if_false)) = &rhs.branches
                    && let (Some(var), Some(tested)) = (var, self.cond_split_var.take())
                {
                    self.cond_vars.insert(
                        var,
                        crate::interp::CondVarEntry {
                            tested,
                            version: self.current.model(tested).version,
                            if_true: if_true.clone(),
                            if_false: if_false.clone(),
                        },
                    );
                }
                if let (Some(var), Some(rhs)) = (var, rhs) {
                    // The declared-type slot the written header left unknown
                    // adopts the initializer's inferred face (#2140): the
                    // semantic layer's staged type wins when present.
                    // Otherwise the fold's own initializer read is the
                    // official `_inferLocalVariable` input, so a nullable
                    // spelled face adopts whole — the written-argument tree
                    // included, which the staging seam cannot carry
                    // (#2243). A non-nullable spelled face keeps the
                    // unknown slot: every null-check disjointness and
                    // promotion answer it produced would be new output the
                    // pre-#2243 fold never made, and no promotion hangs on
                    // it. The unknown face adopts as the opaque-nullable
                    // declared face — the null check's marker step (#2243)
                    // keys on it, and every other consumer reads it exactly
                    // like the unknown it was. The PLAIN unknown face adopts
                    // it too (#2241): an initializer the fold can only type
                    // as unknown (a call's return, a property chain whose
                    // receiver is itself a property read — officially not a
                    // promotable property target, so no slot face engages)
                    // still has a definite static type the semantic layer
                    // resolves; declaring it the plain unknown would refuse
                    // every null-check refinement and leave the guarded
                    // reads unpromoted where the official's
                    // `_inferLocalVariable` typed them nullable.
                    if let Some(meta) = self.vars.get_mut(var as usize)
                        && meta.ty.base == crate::ty::Base::Unknown
                    {
                        let staged = meta
                            .decl_span
                            .and_then(|span| self.inferred_locals.get(&span.offset))
                            .copied();
                        if let Some((staged, staged_args)) = staged {
                            meta.ty = staged;
                            meta.args = staged_args;
                        } else if rhs.name.is_some_and(|name| {
                            matches!(rhs.ty.base, crate::ty::Base::Name(id) if id == name)
                                && self.ops.call_head_unresolved(name, self.names)
                        }) {
                            // The unresolved-call initializer face: the
                            // send's result is the official's
                            // `InvalidType` — the resolved back-end
                            // certified the head resolves nowhere, and
                            // `_inferLocalVariable` types the declared
                            // slot with it, so every `is`-test on the
                            // local succeeds vacuously (the 0c arm) and
                            // deads the failing arm. The #2243 guard keeps
                            // every OTHER non-nullable name face out of
                            // the slot; this one is the back-end's
                            // certified answer, not a fold guess.
                            meta.ty = rhs.ty;
                            meta.args = rhs.args;
                        } else if matches!(rhs.ty.base, crate::ty::Base::Name(_))
                            && self.ops.is_invalid_typed(
                                rhs.ty,
                                rhs.args,
                                self.args,
                                self.names,
                            )
                        {
                            // The invalid-initializer face (the member-read
                            // twin of the unresolved-call face above): a
                            // member read on an invalid-typed receiver
                            // carries the official's `InvalidType`, and the
                            // back-end certifies the spelling resolves
                            // nowhere as a type — `_inferLocalVariable`
                            // types the declared slot with it, so every
                            // `is`-test on the local succeeds vacuously
                            // (the 0c arm) and deads the failing arm.
                            meta.ty = rhs.ty;
                            meta.args = rhs.args;
                        } else if rhs.ty.nullable
                            && matches!(
                                rhs.ty.base,
                                crate::ty::Base::Name(_) | crate::ty::Base::Function
                            )
                        {
                            meta.ty = rhs.ty;
                            meta.args = rhs.args;
                        } else if rhs.this_recv
                            && matches!(rhs.ty.base, crate::ty::Base::Name(_))
                        {
                            // The `this`-initializer face adopts whole even
                            // non-nullable: the staged read is the same
                            // answer the type layer computes for `this`
                            // (the enclosing container, an extension's
                            // written `on` type — argument-bearing
                            // spellings drop to the bare head the flow
                            // lattice spells), not a fold guess — the
                            // official `_inferLocalVariable` types
                            // `final that = this;` with `this`'s own
                            // static type, and the switch-end
                            // exhaustiveness verdict reads it.
                            meta.ty = rhs.ty;
                            meta.args = rhs.args;
                        } else if rhs.ty.base == crate::ty::Base::Unknown {
                            meta.ty = Ty::UNKNOWN_NULLABLE;
                        }
                    }
                    // The declaration's own initialization moves the flow
                    // state (assignments, promotions) but is not a write in
                    // the official `AssignedVariables` sets — the
                    // `prefer_final_locals` premise — so the write-site
                    // record is skipped ([`Interp::apply_write`]), and no
                    // demotion reason attaches (the official
                    // `declare`/`_initialize` carry no
                    // `nonPromotionReason`, `flow_analysis.dart:4914-4916`).
                    self.apply_write(var, rhs.ty, rhs.args, true, None, false);
                }
            }
            Ev::VarDeclEnd => {
                // Restore the enclosing list's context (#1051); the
                // outermost list's restore yields the default — the same
                // value the previous unconditional reset wrote.
                self.decl = self.decl_stack.pop().unwrap_or_default();
                // The slot is closed with the list: a nested list can only
                // begin inside an initializer, and every list's first
                // identifier precedes its initializers, so the enclosing
                // list's seal (set at its first `InitializedIdent`) is
                // already `false` here — no per-list stacking needed.
                self.in_decl = false;
                if let Some(base) = self.pending_decl_bases.pop() {
                    self.pending_inits.truncate(base);
                }
            }
            Ev::Formal {
                name,
                this_or_super: _,
                token,
            } => {
                let ty = match self.pending_ty.take() {
                    // The context-inferred formal face (official
                    // `_inferFormalParameters`): an implicitly typed
                    // formal of an open function literal arrives with
                    // `pending_ty` already materialized as `Some(Unknown)`
                    // by the type-completion event — not `None` — so the
                    // `var` face's `.or_else` chain never consults the
                    // staged schema entry. A genuinely written annotation
                    // never lands here (its `pending_ty` resolves non-
                    // unknown); an unknown pending with a staged inferred
                    // type adopts the staged spelling exactly like the
                    // `var`-declarator window's `meta.ty` adoption
                    // (#2140).
                    None | Some(Ty::UNKNOWN) => self
                        .inferred_locals
                        .get(&token.range.offset)
                        .map(|&(ty, _)| ty)
                        .unwrap_or(Ty::UNKNOWN),
                    Some(ty) => ty,
                };
                let args = std::mem::take(&mut self.pending_ty_args);
                let meta = VarMeta {
                    ty,
                    args,
                    late: false,
                    is_final: false,
                    decl_span: Some(token.range),
                };
                self.declare_var(*name, meta, true);
            }
            Ev::PatternVarDeclStmt { span } => {
                self.header_dead_check(*span);
                // The declaration ends (official
                // `patternVariableDeclaration_end`) — `var (a, b) = ...;`
                // has no alternative clause to fall through to.
                self.close_standalone_pattern();
            }
            Ev::Block { kind, span } => {
                // An empty block directly inside an if branch anchors
                // nothing: the branch body's traversal visits NODES
                // (`checkUnreachableNode`), and an empty `{}` has none to
                // visit — `if (x is! int) {}` keeps its dead then branch
                // silent (oracle-probed). A loop-body `{}` is itself
                // polled (probe table J u1/u4 report its row), so only the
                // branch frame suppresses; any other empty block keeps the
                // pre-existing poll.
                if matches!(kind, BlockKind::Statement)
                    && (!matches!(self.stack.last(), Some(Frame::If { .. }))
                        || !matches!(events.get(i + 1), Some(Ev::EndBlock { .. })))
                {
                    self.dead_code(*span);
                }
                self.push_scope();
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
                    // below) is the covering node itself and never extends
                    // — UNLESS the interval's anchor IS the block (a wholly
                    // dead branch/loop body): the official close reports
                    // the anchor node's own extent, `}` included
                    // (dead_code_verifier.dart:253-279's trim only applies
                    // when the anchor sits inside, not at, the covering
                    // node). A block nested in a statement list is the trim
                    // TARGET instead: `visitBlock` fires no `flowEnd` of
                    // its own (type_analyzer.dart:2230), so the covering
                    // close's single-step trim ends the row at the list's
                    // last statement, `}` included (`visitForStatement`'s
                    // `flowEnd(node.body)`, type_analyzer.dart:3194). The
                    // stack can be shallow here — the outermost executable
                    // records no Exec frame — so the parent is looked up
                    // guarded; a missing parent is a statement block.
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
                    let block_anchored = self
                        .dead
                        .as_ref()
                        .is_some_and(|window| window.anchored_at_depth(self.stack.len() - 1));
                    if (!is_body_block || block_anchored)
                        && let Some(window) = &mut self.dead
                    {
                        window.extend(*end);
                    }
                    self.stack.pop();
                    self.scopes.truncate(scopes_base);
                }
            }
            _ => return false,
        }
        true
    }

    /// Dead-code check for header wrapper statements. Init/updater slices
    /// skip statement semantics: their wrappers are parsing artifacts (the
    /// value feeds the header replay), a dead loop anchors at `for`
    /// itself (`for_begin`), and dead updaters get their own official row
    /// (`for_updaterBegin`, dead_code_verifier.dart:357-375) — not a fold.
    /// The condition slice anchors: the condition can be the first dead
    /// node (`test_forLoop_noUpdaters` anchors the row at the condition's
    /// `(` — dead_code_test.dart:212-220), and the loop's body `flowEnd`
    /// does not cover it (the body block is not an ancestor of the
    /// condition, dead_code_verifier.dart:453-459). Equal depths mean no
    /// init/updater slice encloses — body level or inside a condition.
    ///
    /// Body-level wrappers anchor only when the statement BEGAN dead
    /// ([`Interp::stmt_began_dead`]): one that began live and terminated
    /// flow itself stays silent — the next node anchors.
    fn header_dead_check(&mut self, span: TokenSpan) {
        if self.for_header_depth == self.header_cond_depth {
            self.wrapper_dead_check(span);
        } else {
            // A header part boundary (`for (throw 0; c;)`): the wrapper is
            // a parsing artifact that never self-anchors, and the part's
            // end is the next part's statement boundary — the init's exit
            // makes the condition the first dead node.
            self.stmt_began_dead = !self.reachable();
        }
    }

    /// The post-order statement wrapper's dead check: the row opens only
    /// when the statement began dead. A statement that began live but
    /// terminated flow itself (`throw 0;`, `return throw 1;`) must not
    /// self-anchor — the official's first dead node is the next visited
    /// node (`dead_code_verifier` `_visitNode`).
    ///
    /// The begin-dead snapshot misses one shape: the recorder can drain a
    /// Never exit ([`Ev::Exit`]) past its own statement's `StmtEnd`, so the
    /// refresh there still reads a live state. A wrapper whose flow is dead
    /// from an exit that PRECEDES its own begin is a statement that started
    /// after the exit (`boom(); g();` anchors at `g();`); an exit at or
    /// after the begin is the statement's own death and stays silent.
    fn wrapper_dead_check(&mut self, span: TokenSpan) {
        // The began-dead face, or an exit drained past the statement's own
        // `StmtEnd` whose snapshot refresh still read live: either way the
        // statement started after flow died, and a live current state makes
        // the whole check a no-op (the `dead_code` reachability gate).
        let began_dead = self.stmt_began_dead
            || (!self.reachable() && self.exit_at.is_some_and(|at| at < span.range.offset));
        if !began_dead || self.reachable() {
            return;
        }
        // A began-dead statement is itself the official first dead node
        // (the verifier walks top-down, so the statement precedes every
        // child, dead_code_verifier.dart:183-189). Inner expression polls
        // (the dot-access member poll, the cast-type poll) ran EARLIER in
        // this post-order stream and may have opened the interval on a
        // sub-statement node — re-anchor it to the statement. An interval
        // whose anchor sits before the statement belongs to an earlier
        // dead statement (the one-row fold) and keeps ownership, as does a
        // do-while split whose anchor is its `while` part.
        if let Some(window) = &mut self.dead {
            if window.anchor_from(span.range.offset) {
                window.reanchor_to(span);
                return;
            }
        }
        self.dead_code(span);
    }

    /// The switch member's keyword row: the official
    /// `flowEnd(SwitchMember)` with `node == firstDeadNode` reports at
    /// `node.keyword` — the bare `case`/`default` token — and resets the
    /// interval (dead_code_verifier.dart:232-235); a member folded into an
    /// outer interval (anchor elsewhere) keeps it open.
    fn report_member_head(&mut self, kw: TokenSpan) {
        let anchored_here = self
            .dead
            .as_ref()
            .is_some_and(|window| window.anchor_at(kw.range.offset));
        if anchored_here {
            self.emit(FlowFact::DeadCode { span: kw });
            self.dead = None;
            if let Some(frame_index) = self.switch_frame()
                && let Frame::Switch { reported_kw, .. } = &mut self.stack[frame_index]
            {
                *reported_kw = Some(kw.range.offset);
            }
        }
    }

    /// Consumes the previous group's latched `default` at the next group's
    /// opening: the chain's unmatched state goes unreachable (the
    /// default's alternative ends with a true guard and no pattern —
    /// official `switch_endAlternative` via `_popPattern`), and a
    /// case-headed opener also reports the default's own keyword row
    /// unless its head-time check already fired it (probe table G rules
    /// 1-2: every member at/after the first default is dead, while a
    /// first default followed by another default-headed group stays
    /// live).
    fn consume_dead_default(&mut self, frame_index: usize, case_opener: bool) {
        let latched = match &mut self.stack[frame_index] {
            Frame::Switch { dead_default, .. } => dead_default.take(),
            _ => None,
        };
        let Some((kw, reported)) = latched else {
            return;
        };
        if let Frame::Switch { unmatched, .. } = &mut self.stack[frame_index] {
            *unmatched = unmatched.clone().set_unreachable(&mut self.reach);
        }
        if case_opener && !reported {
            self.emit(FlowFact::DeadCode { span: kw });
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
    #[expect(
        clippy::cast_possible_truncation,
        reason = "var ids are arena indices bounded by one executable's \
                  declarations; u32 matches the event payload width"
    )]
    pub(crate) fn maybe_close_case_group(&mut self, events: &[Ev], match_of: &[u32]) {
        // The close runs its own collect over the switch block, and the
        // frame keeps `case_open` until it returns — a collect run's
        // re-stepped events would re-enter here for the same still-open
        // group and recurse without bound.
        if self.collect.is_some() {
            return;
        }
        // A case head's own pattern region (between its `CaseExprBegin`
        // and `CaseExprEnd`) must not close the previous head's still-open
        // group: stacked heads (`case A: case B: <body>`) are ONE member
        // sharing one body — official `getSwitchStatementMemberInfo` merges
        // consecutive empty members' heads, and `switch_endAlternatives`
        // runs only when the merged member's body begins. The
        // `PatternKind::Switch` context opened at the head's
        // `CaseExprBegin` is the region marker; `case_pattern_end` pops it
        // at `CaseExprEnd`, so the body's first event closes the merged
        // group with every head's matched state joined. Closing at the
        // next head's first pattern event instead would treat the empty
        // body between the heads as a completed member:
        // `switch_after_case` would join its fall-through into the break
        // model and an exhaustive switch would complete normally even
        // when the shared body returns.
        if matches!(
            self.pattern.as_ref().map(|p| p.kind),
            Some(PatternKind::Switch)
        ) {
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
            //
            // The block's effect set is computed once at the first close
            // and cached on the frame (#919 release-readiness audit:
            // re-collecting per close costs O(g*n) for g groups over n
            // block events — the official analyzer computes the switch's
            // `AssignedVariables` once per node and reuses it). Byte-level
            // equivalence of the cached join: every close would re-step
            // the same block events from the same snapshot, so membership
            // for ids below the variable count at collect time is
            // identical at every close (ids are monotonic and stable below
            // it); collect-run declarations allocate ids at/above that
            // count, the rollback erases them, and their join effects land
            // only on dead slots — reads past a state's models fall back
            // to fresh models, re-declaration overwrites, and joins
            // truncate to the live count. The cache therefore keeps
            // exactly the live-id projection, and each close joins the
            // same per-variable demotions in the same sequence as a fresh
            // collection would.
            if matches!(
                self.stack[frame_index],
                Frame::Switch {
                    labeled_effects: None,
                    ..
                }
            ) {
                let effects = self.collect_ranges(events, block.start..block.end, match_of);
                // Drop collect-run-declared ids: only ids live at collect
                // time are observable at any close (see above).
                let live = self.vars.len() as u32;
                let written = effects.written.into_iter().filter(|&v| v < live).collect();
                let captured = effects.captured.into_iter().filter(|&v| v < live).collect();
                if let Frame::Switch {
                    labeled_effects, ..
                } = &mut self.stack[frame_index]
                {
                    *labeled_effects = Some(LabeledEffects { written, captured });
                }
            }
            let (written, captured, previous) = match &self.stack[frame_index] {
                Frame::Switch {
                    labeled_effects: Some(effects),
                    previous,
                    ..
                } => (
                    effects.written.clone(),
                    effects.captured.clone(),
                    previous.clone(),
                ),
                _ => return,
            };
            // The join sweeps models and property slots together: a
            // receiver written in any case body kills the promotions
            // carried into the switch on every labeled-entry path.
            let joined = self.join_state_with_sweep(previous, &written, &captured, None);
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
        // Official `switch_afterCase` reads the LOCAL flag
        // (flow_analysis.dart:8033): `switch_endAlternatives`' closing
        // split resurrects it on purpose, so a dead group's body still
        // joins its fall-through into the break model.
        let reachable = self.reach.locally_reachable(self.current.reach);
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
            case_shadowed,
            ..
        } = &mut self.stack[frame_index]
        {
            *body_depth = None;
            *group_combined = None;
            let shadowed = std::mem::take(case_shadowed);
            self.restore_bindings(&shadowed);
        }
    }

    /// Pops the switch frame at `SwitchBlockEnd`: the official `switch_end`
    /// (flow_analysis.dart:7979-8000). `isExhaustive` is the scrutinee's
    /// always-exhaustive verdict ([`Interp::is_always_exhaustive`], the
    /// `hasDefault` arm already covered — a default group's close marked
    /// the unmatched chain unreachable): an exhaustive switch never joins
    /// the unmatched chain into the exit, and with no break/fall-through
    /// state at all the post-switch state is `previous` unreachable — the
    /// unreachable body-completion silence upstream's
    /// `checkForBodyMayCompleteNormally` reads through `isReachable`
    /// (type_analyzer.dart:647-652).
    /// The switch's own scope close: any binder an empty-bodied or
    /// recovered member left open restores here (a well-formed member
    /// already restored at its close).
    fn switch_scope_close(&mut self, frame_index: usize) {
        let shadowed = match &mut self.stack[frame_index] {
            Frame::Switch {
                case_shadowed, ..
            } => std::mem::take(case_shadowed),
            _ => return,
        };
        self.restore_bindings(&shadowed);
    }

    fn switch_statement_end(&mut self, switch_index: usize) {
        let (unmatched, previous, break_model, exhaustive) = match &self.stack[switch_index] {
            Frame::Switch {
                unmatched,
                previous,
                break_model,
                scrutinee_ty,
                scrutinee_args,
                ..
            } => (
                unmatched.clone(),
                previous.clone(),
                break_model.clone(),
                self.is_always_exhaustive(*scrutinee_ty, *scrutinee_args),
            ),
            _ => return,
        };
        self.switch_scope_close(switch_index);
        self.stack.remove(switch_index);
        let exit = if exhaustive {
            // No implicit fall-through default joins the unmatched chain
            // (official `if (!isExhaustive) breakState = _join(...)`);
            // `breakState ??= previous.setUnreachable()` — an exhaustive
            // switch whose bodies all terminate leaves the code after it
            // dead (the scrutinee type is uninhabited past the cases).
            break_model.unwrap_or_else(|| previous.clone().set_unreachable(&mut self.reach))
        } else {
            self.join(break_model, Some(unmatched))
        };
        self.current = exit
            .unsplit(&mut self.reach)
            .inherit_tested(&previous, &mut self.lists);
    }

    /// Statement-position if-case: the pattern's matched state becomes the
    /// then-branch state (official `ifCaseStatement_endAlternative`).
    /// Pattern-declared variables already live in the enclosing scope from
    /// their declaration events.
    fn if_case_then(&mut self) {
        // Parser event-order drift guard: an IfCase-gated ThenBegin always
        // follows its pattern events, so the context is live in a coherent
        // stream. A missing context is a producer contract break — skip the
        // branch instead of panicking inside the interpreter (async_gap's
        // let-else return posture).
        let Some(mut pattern) = self.pattern.take() else {
            return;
        };
        let mut matched = self.current.clone();
        if let Some(guard) = pattern.guard {
            let (if_true, if_false) = self.cond_split(&guard);
            // The guard's false arm is an UNMATCHED path (official
            // `_handleGuard` joins it into the same `_unmatched` chain the
            // pattern's failures feed): dropping it folds the else arm
            // dead whenever the pattern itself cannot fail (`case P p when
            // p.g` over an exact scrutinee leaves `return false` after the
            // if reachable in the oracle).
            pattern.unmatched = self.join(Some(pattern.unmatched.clone()), Some(if_false));
            matched = if_true;
        }
        self.stack.push(Frame::If {
            branch: pattern.unmatched,
            after_then: None,
            shadowed: pattern.shadowed,
        });
        self.current = matched;
    }
}

/// The official `attachFinally` at the state level: reachability follows
/// the finally's exit; per-variable promotion models follow the version
/// compare (identical before/after models keep the try promotions). The
/// property slots join under the same per-key rule — the official
/// `_applyPropertyPromotions` transfers finally-established property
/// promotions onto the post-state (`flow_analysis.dart:5745-5800`), which
/// the flat slot form reproduces by running the identical per-key compare:
/// a receiver written in the try body reset its slots there, so the
/// finally's own tests decide the exit promotions (#1409).
fn attach_finally(
    interp: &mut Interp<'_>,
    after_try: &State,
    before_finally: &State,
    after_finally: &State,
) -> State {
    let vars = saturate_slot_u32(interp.vars.len());
    let slots = after_try
        .props
        .len()
        .max(before_finally.props.len())
        .max(after_finally.props.len());
    let reachable_after = interp.reach.locally_reachable(after_finally.reach);
    let mut models = Vec::with_capacity(vars as usize);
    for var in 0..vars {
        let v1 = after_try.model(var);
        let v2 = before_finally.model(var);
        let v3 = after_finally.model(var);
        // The before-finally model participates only when the variable was
        // already in a distinct state there (the official domain diff).
        let before = if v2 == v1 { None } else { Some(v2) };
        models.push(attach_finally_model(&interp.lists, v1, before, v3));
    }
    let mut props = Vec::with_capacity(slots);
    for slot in 0..slots {
        let slot = saturate_slot_u32(slot);
        let v1 = after_try.prop(slot);
        let v2 = before_finally.prop(slot);
        let v3 = after_finally.prop(slot);
        let before = if v2 == v1 { None } else { Some(v2) };
        props.push(attach_finally_model(&interp.lists, v1, before, v3));
    }
    let reach = if reachable_after {
        after_try.reach
    } else {
        interp.reach.set_unreachable(after_try.reach)
    };
    State {
        reach,
        models,
        props,
    }
}
