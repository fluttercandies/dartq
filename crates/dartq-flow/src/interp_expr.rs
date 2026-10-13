//! Expression events: references, tests, boolean/null-aware branching,
//! assignments, and collection elements.

use dartq_parser::TokenSpan;

use crate::ev::{BinOp, Ev};
use crate::facts::FlowFact;
pub(crate) use crate::interp::Expr;
use crate::interp::{Frame, Interp};
use crate::model::State;
use crate::model::is_non_nullable;
use crate::ty::{Base, Ty};

impl Interp<'_> {
    /// Handles one expression-layer event; `false` means the event belongs
    /// to another layer.
    #[expect(
        clippy::too_many_lines,
        reason = "single port of the official expression decision order; splitting the \
                  match would scatter the per-event documentation"
    )]
    pub(crate) fn expr_step(
        &mut self,
        ev: &Ev,
        events: &[Ev],
        i: usize,
        _match_of: &[u32],
    ) -> bool {
        match ev {
            Ev::Ident {
                name,
                token,
                context,
            } => {
                use dartq_parser::IdentifierContext as Ctx;
                match context {
                    Ctx::Expression | Ctx::ExpressionContinuation
                        if matches!(events.get(i + 1), Some(Ev::ForInExprBegin)) =>
                    {
                        // A for-in loop target (`for (i in xs)`) is the
                        // write, not a read (analyzer for_resolver.dart
                        // :237-241 `flow?.write` right before
                        // `forEach_bodyBegin`); the write consumes the
                        // reference, leaving the iterable as the head's
                        // only value.
                        if let Some(var) = self.resolve(*name) {
                            self.write_var(var, Ty::UNKNOWN, false);
                        }
                    }
                    Ctx::Expression | Ctx::ExpressionContinuation => match self.resolve(*name) {
                        Some(var) => {
                            self.check_unassigned_read(*token, var);
                            self.record_read(var);
                            let model = self.current.model(var);
                            let ty = self.promoted_type_of(var, &model);
                            let args = self.promoted_args_of(var, &model);
                            let declared = self
                                .vars
                                .get(var as usize)
                                .map_or(Ty::UNKNOWN, |meta| meta.ty);
                            // Promotion fact (bounded to real promotions:
                            // `promoted_type_of` returns the declared type
                            // when unpromoted, so only a chain head emits).
                            // The row carries the chain head's written
                            // arguments beside the bare spelling (#1010),
                            // and dead reads carry no row (their
                            // unreachable region is already reported as
                            // dead code). Invariant with the plain-assign
                            // retraction below: an assignment TARGET read
                            // under an active promotion emits here, so the
                            // retraction removes this fact too — the
                            // target identifier is a write, not a read.
                            if self.reachable() && ty != declared {
                                self.emit(FlowFact::PromotedRead {
                                    span: *token,
                                    ty,
                                    args,
                                });
                            }
                            let mut expr = Expr::of_reference(var, ty, args);
                            expr.read_span = Some(*token);
                            self.exprs.push(expr);
                        }
                        None => self.exprs.push(Expr::trivial(Ty::UNKNOWN)),
                    },
                    // Named-argument labels and declaration names carry no
                    // value; catch/declaration names never reach the
                    // expression stack.
                    _ => {}
                }
            }
            Ev::Type {
                base,
                nullable,
                args,
            } => {
                let ty = Ty {
                    base: *base,
                    nullable: *nullable,
                };
                self.sink_type(ty, *args);
            }
            Ev::NoType | Ev::FunctionType => self.sink_type(Ty::UNKNOWN, crate::ty::EMPTY_ARGS),
            Ev::VoidType => self.sink_type(
                Ty {
                    base: crate::ty::Base::Void,
                    nullable: false,
                },
                crate::ty::EMPTY_ARGS,
            ),
            Ev::Bool { value } => {
                let mut expr = Expr::trivial(Ty::UNKNOWN);
                expr.bool_value = Some(*value);
                self.exprs.push(expr);
            }
            Ev::Null => self.exprs.push(Expr::trivial(Ty::NULL)),
            Ev::OtherExpr { .. } => self.exprs.push(Expr::trivial(Ty::UNKNOWN)),
            Ev::UnaryPrefix { is_not } => {
                let operand = self.pop_value();
                if *is_not {
                    let mut expr = Expr::trivial(Ty::UNKNOWN);
                    expr.bool_value = operand.bool_value.map(|b| !b);
                    expr.branches = operand.branches.map(|(t, f)| (f, t));
                    self.exprs.push(expr);
                } else {
                    self.exprs.push(operand);
                }
            }
            Ev::NonNullAssert => {
                let operand = self.pop_value();
                if let Some(var) = operand.reference {
                    let promoted = self.ops.promote_to_non_null(operand.ty);
                    if promoted != operand.ty {
                        self.current = self.current.clone().finish_type_test(
                            self.ops,
                            &mut self.lists,
                            var,
                            None,
                            Some((promoted, operand.args)),
                        );
                    }
                    self.exprs
                        .push(Expr::trivial(self.ops.promote_to_non_null(operand.ty)));
                } else {
                    self.exprs
                        .push(Expr::trivial(self.ops.promote_to_non_null(operand.ty)));
                }
            }
            Ev::UnaryPrefixAssign | Ev::UnaryPostfixAssign { .. } => {
                // `++x` / `x++`: the target reference was pushed; the write
                // stores a non-null value of the declared type.
                let target = self.pop_value();
                if let Some(var) = target.reference {
                    let declared = self
                        .vars
                        .get(var as usize)
                        .map_or(Ty::UNKNOWN, |meta| meta.ty);
                    self.write_var(var, self.ops.promote_to_non_null(declared), false);
                }
                self.exprs.push(Expr::trivial(Ty::UNKNOWN));
            }
            Ev::BinaryBegin { op, span } => match op {
                BinOp::And | BinOp::Or => {
                    // The left operand is on the stack; split on it and run
                    // the right operand under the taken branch (official
                    // `logicalBinaryOp_rightBegin`).
                    let left = self.pop_value();
                    let (left_if_true, left_if_false) = self.cond_split(&left);
                    let (taken, other) = if matches!(op, BinOp::And) {
                        (left_if_true, left_if_false)
                    } else {
                        (left_if_false, left_if_true)
                    };
                    let rhs_dead_entry = !self.reach.overall_reachable(taken.reach);
                    self.stack.push(Frame::Branch {
                        branch: other,
                        ifnull_ty: None,
                        op_span: *span,
                        rhs_dead_entry,
                    });
                    self.current = taken;
                }
                BinOp::IfNull => {
                    // Official `ifNullExpression_rightBegin`
                    // (flow_analysis.dart:7086-7104): split; the SKIP path
                    // (LHS was non-null — the path that skips the RHS,
                    // official `shortcutState`) carries the LHS's non-null
                    // promotion (`tryMarkNonNullable(...).ifTrue`), while
                    // the RHS itself evaluates on the LHS-null path (the
                    // split state, unchanged). A guaranteed-non-null LHS
                    // type (official `_isGuaranteedNonNullWithSoundNullSafety`)
                    // makes the RHS path unreachable — `int x; x ?? 1`
                    // anchors its dead row at `??` (probe table C; a
                    // resolved-type LHS like `x != null` needs the type
                    // layer — semantic queue).
                    let left = self.pop_value();
                    self.split_cur();
                    let shortcut = match left.reference {
                        Some(var) => {
                            let promoted = self.ops.promote_to_non_null(left.ty);
                            if promoted == left.ty {
                                self.current.clone()
                            } else {
                                self.current.clone().finish_type_test(
                                    self.ops,
                                    &mut self.lists,
                                    var,
                                    None,
                                    Some((promoted, left.args)),
                                )
                            }
                        }
                        None => self.current.clone(),
                    };
                    let mut rhs_dead_entry = false;
                    if is_non_nullable(self.ops, left.ty) {
                        self.unreachable_cur();
                        rhs_dead_entry = true;
                    }
                    // `??` types as the LHS's non-null form.
                    let result_ty = self.ops.promote_to_non_null(left.ty);
                    self.stack.push(Frame::Branch {
                        branch: shortcut,
                        ifnull_ty: Some(result_ty),
                        op_span: *span,
                        rhs_dead_entry,
                    });
                }
                _ => {}
            },
            Ev::BinaryEnd { op, end } => match op {
                BinOp::And | BinOp::Or => {
                    // The official reports the dead right operand at the
                    // construct's own `flowEnd`, re-anchored at the operator
                    // (dead_code_verifier.dart:311-322) — before the branch
                    // join restores reachability, and only when the operand
                    // RAN dead (a literal-folded entry), never when it
                    // merely ends unreachable (`c && throw 1` is live).
                    if let Some(Frame::Branch {
                        op_span,
                        rhs_dead_entry,
                        ..
                    }) = self.stack.last()
                    {
                        let (op_span, rhs_dead_entry) = (*op_span, *rhs_dead_entry);
                        if rhs_dead_entry {
                            self.dead_expr_arm(op_span, *end);
                        }
                    }
                    let right = self.pop_value();
                    let (right_if_true, right_if_false) = self.cond_split(&right);
                    let Some(Frame::Branch {
                        branch: other,
                        ifnull_ty: _,
                        ..
                    }) = self.stack.pop()
                    else {
                        // A frame-less close (unbalanced recovery buffer):
                        // the binary still nets one value for its consumer.
                        self.exprs.push(Expr::trivial(Ty::UNKNOWN));
                        return true;
                    };
                    let (if_true, if_false) = if matches!(op, BinOp::And) {
                        (right_if_true, self.join(Some(other), Some(right_if_false)))
                    } else {
                        (self.join(Some(other), Some(right_if_true)), right_if_false)
                    };
                    self.current = if_true.clone();
                    let mut expr = Expr::trivial(Ty::UNKNOWN);
                    expr.branches = Some((if_true, if_false));
                    self.exprs.push(expr);
                }
                BinOp::IfNull => {
                    // Official `ifNullExpression_end`
                    // (flow_analysis.dart:7071-7080): join the RHS-exit
                    // state with the non-null skip path. The conditional
                    // split of the whole expression contributes the skip
                    // path to both branches (its truthiness is the LHS
                    // value's — unknowable at the name level), the same
                    // convention as the And/Or arms above.
                    if let Some(Frame::Branch {
                        op_span,
                        rhs_dead_entry,
                        ..
                    }) = self.stack.last()
                    {
                        let (op_span, rhs_dead_entry) = (*op_span, *rhs_dead_entry);
                        if rhs_dead_entry {
                            self.dead_expr_arm(op_span, *end);
                        }
                    }
                    let right = self.pop_value();
                    let (right_if_true, right_if_false) = self.cond_split(&right);
                    let Some(Frame::Branch {
                        branch: shortcut,
                        ifnull_ty,
                        ..
                    }) = self.stack.pop()
                    else {
                        // A frame-less close (unbalanced recovery buffer):
                        // the binary still nets one value for its consumer.
                        self.exprs.push(Expr::trivial(Ty::UNKNOWN));
                        return true;
                    };
                    let if_true = self.join(Some(shortcut.clone()), Some(right_if_true));
                    let if_false = self.join(Some(shortcut), Some(right_if_false));
                    self.current = if_true.clone();
                    let mut expr = Expr::trivial(ifnull_ty.unwrap_or(Ty::UNKNOWN));
                    expr.branches = Some((if_true, if_false));
                    self.exprs.push(expr);
                }
                BinOp::Eq | BinOp::NotEq => {
                    let right = self.pop_value();
                    let left = self.pop_value();
                    let inverted = matches!(op, BinOp::NotEq);
                    self.equality_check(&left, &right, inverted);
                }
                BinOp::Other => {
                    self.exprs.pop();
                    self.exprs.pop();
                    self.exprs.push(Expr::trivial(Ty::UNKNOWN));
                }
            },
            Ev::IsBegin => self.in_type_test = true,
            Ev::IsOperator { is_not } => {
                // The tested type arrived through Type events (before the
                // operator event); its written arguments latch beside it.
                let tested = self.tested_ty.take().unwrap_or(Ty::UNKNOWN);
                let tested_args = self.tested_args;
                self.type_test_pending = Some((*is_not, tested, tested_args));
            }
            Ev::IsEnd => {
                self.in_type_test = false;
                let Some((is_not, tested, tested_args)) = self.type_test_pending.take() else {
                    return true;
                };
                let operand = self.pop_value();
                let mut expr = Expr::trivial(Ty::UNKNOWN);
                if let Some(var) = operand.reference {
                    let promoted = self.promotion_candidate(var, tested, tested_args);
                    let (matched, unmatched) =
                        self.branch_type_test(var, tested, tested_args, promoted);
                    let (if_true, if_false) = if is_not {
                        (unmatched, matched)
                    } else {
                        (matched, unmatched)
                    };
                    self.current = if_true.clone();
                    expr.branches = Some((if_true, if_false));
                } else {
                    let cur = self.current.clone();
                    expr.branches = Some((cur.clone(), cur));
                }
                self.exprs.push(expr);
            }
            Ev::AsBegin => {
                self.in_as_cast = true;
                self.as_base = self.exprs.len();
            }
            Ev::AsOperator => {
                let cast_ty = self.tested_ty.take().unwrap_or(Ty::UNKNOWN);
                self.as_cast_pending = Some(cast_ty);
            }
            Ev::AsEnd => {
                self.in_as_cast = false;
                let Some(cast_ty) = self.as_cast_pending.take() else {
                    return true;
                };
                if self.exprs.len() == self.as_base {
                    // A cast pattern (`p as T`; parser patterns.rs:176-187
                    // reuses the as-operator events): the primary pattern is
                    // not an expression, so nothing landed on the expr
                    // stack. The cast is a type assertion on the matched
                    // path: the declared variable refines to the cast type
                    // directly (shared_type_analyzer.dart:145-152) — it is
                    // not a promotion candidate and must not run through
                    // the promotion gate.
                    if let Some((name, _)) = self
                        .pattern
                        .as_ref()
                        .and_then(|p| p.pending_decls.last().copied())
                        && let Some(var) = self.resolve(name)
                    {
                        self.current = self.current.clone().finish_type_test(
                            self.ops,
                            &mut self.lists,
                            var,
                            None,
                            Some((cast_ty, self.as_cast_args)),
                        );
                    }
                    return true;
                }
                let operand = self.pop_value();
                // The guaranteed-fail face (official `asExpression_end`'s
                // `_isTypeCheckGuaranteedToFailWithSoundNullSafety` →
                // `_typesAreDisjointDueToNullability`): a null-or-equivalent
                // operand under a non-nullable cast target — or the reverse —
                // always throws, so the flow state goes unreachable and the
                // block tail is dead (`Null n; n as int; print(x);`).
                if self.disjoint_by_nullability(operand.ty, cast_ty) {
                    self.unreachable_cur();
                }
                if let Some(var) = operand.reference {
                    if let Some(promoted) = self.promotion_candidate(var, cast_ty, self.as_cast_args)
                    {
                        self.current = self.current.clone().finish_type_test(
                            self.ops,
                            &mut self.lists,
                            var,
                            None,
                            Some((promoted, self.as_cast_args)),
                        );
                    }
                }
                self.exprs.push(Expr::trivial(cast_ty));
            }
            Ev::Assign { plain } => {
                let rhs = self.pop_value();
                let target = self.pop_value();
                // The read-back value's written arguments are the right-hand
                // side's when the write carried its type (a plain `=`), and
                // argument-less for the compound operators' unknown face.
                let rhs_args = if *plain { rhs.args } else { crate::ty::EMPTY_ARGS };
                if let Some(var) = target.reference {
                    let written = if *plain { rhs.ty } else { Ty::UNKNOWN };
                    if let Some(span) = target.read_span {
                        // A plain assignment's target identifier is the
                        // write, not a read: retract its facts. Both read
                        // facts can coexist at this span — the target read
                        // may run under an active promotion (writing
                        // inside a promoted branch) and may be definitely
                        // unassigned.
                        if *plain {
                            self.facts.retain(|fact| {
                                !matches!(
                                    fact,
                                    FlowFact::UnassignedRead { span: s, .. }
                                        | FlowFact::PromotedRead { span: s, .. }
                                        if *s == span
                                )
                            });
                        }
                    }
                    self.write_var(var, written, *plain);
                    // The assignment expression reads back the variable.
                    self.exprs
                        .push(Expr::of_reference(var, written, rhs_args));
                } else {
                    self.exprs
                        .push(Expr::trivial(if *plain { rhs.ty } else { Ty::UNKNOWN }));
                }
            }
            Ev::PatternAssign => {
                let rhs = self.pop_value();
                for var in std::mem::take(&mut self.pending_writes) {
                    self.write_var(var, rhs.ty, true);
                }
            }
            Ev::CondBegin => {
                let cond = self.pop_value();
                let (if_true, if_false) = self.cond_split(&cond);
                let then_dead_entry = !self.reach.overall_reachable(if_true.reach);
                self.stack.push(Frame::Cond {
                    branch: if_false,
                    then_state: None,
                    then_dead_entry,
                    else_dead_entry: false,
                });
                self.current = if_true;
            }
            Ev::CondColon {
                then_begin,
                then_end,
            } => {
                // The official fires `flowEnd(thenExpression)` at the arm's
                // end (type_analyzer.dart:2588): a dead then arm reports its
                // own range before the else arm's state swap — and only an
                // entry-dead arm rows (`cond ? throw 1 : 2` is live).
                let then_dead_entry = match self.stack.last() {
                    Some(Frame::Cond {
                        then_dead_entry, ..
                    }) => *then_dead_entry,
                    _ => false,
                };
                if then_dead_entry {
                    self.dead_expr_arm(*then_begin, *then_end);
                }
                let then_expr = self.pop_value();
                if let Some(Frame::Cond {
                    branch,
                    then_state,
                    else_dead_entry,
                    ..
                }) = self.stack.last_mut()
                {
                    *then_state = Some(self.current.clone());
                    *else_dead_entry = !self.reach.overall_reachable(branch.reach);
                    self.current = branch.clone();
                }
                let _ = then_expr;
                // The then value is replaced by the whole conditional's
                // value at CondEnd.
            }
            Ev::CondEnd {
                else_begin,
                end: arm_end,
            } => {
                // `flowEnd(elseExpression)` (type_analyzer.dart:2616): the
                // dead else arm reports before the branch join.
                let else_dead_entry = match self.stack.last() {
                    Some(Frame::Cond {
                        else_dead_entry, ..
                    }) => *else_dead_entry,
                    _ => false,
                };
                if else_dead_entry {
                    self.dead_expr_arm(*else_begin, *arm_end);
                }
                let else_expr = self.pop_value();
                let Some(Frame::Cond { then_state, .. }) = self.stack.pop() else {
                    // A frame-less close (unbalanced recovery buffer): the
                    // conditional still nets one value for its consumer.
                    self.exprs.push(Expr::trivial(Ty::UNKNOWN));
                    return true;
                };
                let joined = self.join(then_state, Some(self.current.clone()));
                self.current = joined;
                let ty = if else_expr.ty == Ty::UNKNOWN {
                    Ty::UNKNOWN
                } else {
                    else_expr.ty
                };
                self.exprs.push(Expr::trivial(ty));
            }
            Ev::DotAccess { null_aware } | Ev::IndexAccess { null_aware } => {
                let target = self.pop_value();
                let mut expr = Expr::trivial(Ty::UNKNOWN);
                if *null_aware {
                    // Official nullAwareAccess_end: the access path sees
                    // the target promoted non-null; the skip path does not.
                    if let (Some(var), false) =
                        (target.reference, is_non_nullable(self.ops, target.ty))
                    {
                        let promoted = self.ops.promote_to_non_null(target.ty);
                        let matched = self.current.clone().finish_type_test(
                            self.ops,
                            &mut self.lists,
                            var,
                            None,
                            Some((promoted, target.args)),
                        );
                        expr.branches = Some((matched, self.current.clone()));
                    }
                }
                self.exprs.push(expr);
            }
            Ev::Send | Ev::SendNoArgs => {
                // The callee flows through as the send's result: a bare
                // identifier keeps its variable reference and promoted type
                // (a plain assignment's target is exactly such a reference).
                if let Some(callee) = self.exprs.pop() {
                    self.exprs.push(callee);
                }
            }
            Ev::ArgumentsBegin
            | Ev::ParenBegin
            | Ev::ParenEnd
            | Ev::ThenControlFlow
            | Ev::AwaitBegin => {}
            Ev::ArgumentsEnd { count } => {
                for _ in 0..*count {
                    self.exprs.pop();
                }
            }
            Ev::Throw => {
                let _ = self.exprs.pop();
                self.unreachable_cur();
            }
            Ev::Exit { .. } => {
                // Never-typed subexpression (official `handleExit`): the
                // flow state goes unreachable at the expression's end. No
                // pop: the official call site fires with the subexpression
                // still on the stack ("Stack: (Expression)",
                // type_analyzer.dart:815-819) and the enclosing construct's
                // own close event consumes it — popping here would
                // underflow e.g. `ArgumentsEnd` for `f(neverFn())`. The
                // recorded `end` offset mirrors the official trace payload;
                // only `self.current` mutates, so a collect run rolls the
                // effect back through its snapshot exactly like Throw.
                self.unreachable_cur();
            }
            Ev::Rethrow => {
                self.unreachable_cur();
            }
            Ev::RecordLitEnd { count } | Ev::ListLitEnd { count } => self.literal_value(*count),
            Ev::MapEntryEnd => {
                // The entry's value is the value's own stack entry; the
                // key's entry drops.
                let value = self.exprs.pop();
                self.exprs.pop();
                if let Some(value) = value {
                    self.exprs.push(value);
                }
            }
            Ev::CascadeBegin => {
                self.stack.push(Frame::Cascade {
                    exprs_base: self.exprs.len(),
                });
            }
            Ev::CascadeAccess { .. } => {
                // The section's value is consumed by the cascade.
                self.exprs.pop();
            }
            Ev::CascadeEnd => {
                match self.stack.pop() {
                    Some(Frame::Cascade { exprs_base }) => {
                        // Keep the cascade target as the value.
                        self.exprs.truncate(exprs_base);
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
            Ev::EndLiteralString {
                interpolation_count,
            } => {
                for _ in 0..*interpolation_count {
                    self.exprs.pop();
                }
                self.exprs.push(Expr::trivial(Ty::UNKNOWN));
            }
            Ev::Spread | Ev::NullAwareElement => {
                // The operand's value is consumed; the element leaves its
                // own (trivial) value — every literal element nets exactly
                // one.
                self.exprs.pop();
                self.exprs.push(Expr::trivial(Ty::UNKNOWN));
            }
            Ev::IfControlFlowBegin => {
                let cond = self.pop_value();
                let (if_true, if_false) = self.cond_split(&cond);
                self.stack.push(Frame::Branch {
                    branch: if_false,
                    ifnull_ty: None,
                    // Collection if-elements have no operator row face; the
                    // field only feeds the `&&`/`||`/`??` close.
                    op_span: TokenSpan {
                        id: 0,
                        range: dartq_source::SourceRange {
                            offset: 0,
                            length: 0,
                        },
                    },
                    rhs_dead_entry: false,
                });
                self.current = if_true;
            }
            Ev::ElseControlFlow => {
                // The then-element is consumed; the else-element runs under
                // the condition-false branch.
                self.exprs.pop();
                if let Some(Frame::Branch { branch, .. }) = self.stack.last() {
                    self.current = branch.clone();
                }
            }
            Ev::EndIfControlFlow { has_else } => {
                let _ = self.exprs.pop();
                if *has_else {
                    // The then-element was popped at ElseControlFlow.
                }
                let Some(Frame::Branch { branch, .. }) = self.stack.pop() else {
                    // A frame-less close (unbalanced recovery buffer): the
                    // element still nets one value for its consumer.
                    self.exprs.push(Expr::trivial(Ty::UNKNOWN));
                    return true;
                };
                let joined = self.join(Some(branch), Some(self.current.clone()));
                self.current = joined;
                // The element leaves its own value (every literal element
                // nets exactly one).
                self.exprs.push(Expr::trivial(Ty::UNKNOWN));
            }
            Ev::ExprBody => {
                // The arrow body's value is consumed by the function; the
                // function's own value for the consumer is netted at
                // `ExecEnd` (crate contract, see `SwitchExprBlockEnd`).
                self.exprs.pop();
            }
            _ => return false,
        }
        true
    }

    // —— shared expression helpers ———————————————————————————————————————

    /// Pops one operand value. Recovery-shaped buffers (metadata contexts,
    /// cross-frame event bleed) can record a consumer without its
    /// producers; the missing operand degrades to a trivial value — no
    /// reference, no branches, so it contributes no fact — and the
    /// handler's stack arithmetic stays intact.
    pub(crate) fn pop_value(&mut self) -> Expr {
        self.exprs
            .pop()
            .unwrap_or_else(|| Expr::trivial(Ty::UNKNOWN))
    }

    /// Collapses one literal's element values to a single value: each
    /// element netted exactly one, so the last is the literal's own value
    /// for the enclosing consumer's LIFO pop and the earlier ones drop; an
    /// empty literal carries a trivial value.
    fn literal_value(&mut self, count: usize) {
        if count > 0 {
            self.exprs
                .truncate(self.exprs.len().saturating_sub(count - 1));
        } else {
            self.exprs.push(Expr::trivial(Ty::UNKNOWN));
        }
    }

    /// The promoted type of a variable read (chain head or declared). A
    /// stale id (recovery-shaped buffers can resolve a binding past a
    /// collect rollback) degrades to the unknown type.
    pub(crate) fn promoted_type_of(&self, var: u32, model: &crate::model::VarModel) -> Ty {
        if model.promoted == crate::model::NIL {
            self.vars
                .get(var as usize)
                .map_or(Ty::UNKNOWN, |meta| meta.ty)
        } else {
            self.lists.head(model.promoted)
        }
    }

    /// The written-argument tree of a variable read's flow type (chain
    /// head's rows or the declared type's) — the arena-read pairing of
    /// [`Interp::promoted_type_of`] that the [`FlowFact::PromotedRead`]
    /// rows carry (#1010). Same stale-id degradation.
    pub(crate) fn promoted_args_of(&self, var: u32, model: &crate::model::VarModel) -> u32 {
        if model.promoted == crate::model::NIL {
            self.vars
                .get(var as usize)
                .map_or(crate::ty::EMPTY_ARGS, |meta| meta.args)
        } else {
            self.lists.head_args(model.promoted)
        }
    }

    /// The promotion target for a type check (official
    /// `tryPromoteForTypeCheck`'s true branch,
    /// flow_analysis.dart:4137-4145): the tested type factored against
    /// every existing promotion. A bottom candidate (`x is int` twice
    /// factors to `Never`) and a candidate the current type already
    /// subsumes (`x is int?` on `int x`) are not steps (official
    /// `isValidPromotionStep`); anything else is the step. The official
    /// `tryPromoteToType` further gates on `tested <: current`
    /// (flow_analysis.dart:7229-7246) — a question the name-level seam
    /// cannot decide (`int <: num` and `String <: num` are both false
    /// without a class hierarchy) and the oracle pins require the
    /// promotion on (`num n`, `is int`), so the undecided case promotes
    /// optimistically; a resolved back-end re-tightens the gate through
    /// [`crate::ty::TypeOps::try_promote_to`]. The written-argument face
    /// of the same question IS decidable here (#611 step 4): the test is
    /// refused when the tested arguments cannot be a subtype of the
    /// declared ones — the official lattice refuses it too.
    fn promotion_candidate(&mut self, var: u32, tested: Ty, tested_args: u32) -> Option<Ty> {
        match tested.base {
            Base::Unknown | Base::Dynamic | Base::Void => None,
            _ => {
                let declared_args = self
                    .vars
                    .get(var as usize)
                    .map_or(crate::ty::EMPTY_ARGS, |meta| meta.args);
                if !self.args_admit(tested, tested_args, declared_args) {
                    return None;
                }
                let model = self.current.model(var);
                let previous = self.promoted_type_of(var, &model);
                let mut candidate = tested;
                for ty in self.lists.iter(model.promoted) {
                    candidate = self.ops.factor(candidate, ty);
                }
                if self.ops.is_bottom(candidate) || self.ops.is_subtype(previous, candidate) {
                    None
                } else {
                    Some(candidate)
                }
            }
        }
    }

    /// The argument face of the promotion gate: whether a test spelled
    /// with `tested_args` can promote a variable declared with
    /// `declared_args`. Both argument-less is the name-level fast path
    /// (the ops seam decides, zero arena traffic); a written tested list
    /// over an argument-less declaration admits (raw `List` spells
    /// `List<dynamic>`, whose entries admit everything); the reverse
    /// refuses (`List<dynamic>` is no subtype of `List<int>`) — except
    /// `Never`, a subtype of every instantiation. Two written lists
    /// compare element-wise below.
    fn args_admit(&self, tested: Ty, tested_args: u32, declared_args: u32) -> bool {
        match (tested_args, declared_args) {
            (0, 0) | (_, 0) => true,
            (0, _) => tested.base == Base::Never,
            (t, d) => self.args_subtype(t, d),
        }
    }

    /// Element-wise covariance (`List<int>` is a subtype of
    /// `List<int?>`): every entry pair must be a subtype. Distinct entry
    /// names are undecidable at this seam and refuse — the official
    /// lattice would refuse too unless a real subclass relation holds,
    /// which the name level cannot see. Equal arena ids short-circuit
    /// (canonical interning makes id equality tree equality).
    fn args_subtype(&self, t: u32, d: u32) -> bool {
        let (Some(ts), Some(ds)) = (self.args.get(t as usize), self.args.get(d as usize)) else {
            return false;
        };
        ts.len() == ds.len()
            && ts
                .iter()
                .zip(ds)
                .all(|(a, b)| a.args == b.args || self.entry_subtype(a, b))
    }

    /// The name-level subtype question for one argument-tree entry
    /// (mirrors [`crate::ty::NameTypeOps::is_subtype`]'s decidable arms,
    /// recursing into written arguments).
    fn entry_subtype(&self, a: &crate::ty::ArgTy, b: &crate::ty::ArgTy) -> bool {
        match (a.base, b.base) {
            (Base::Dynamic | Base::Unknown | Base::Void, _)
            | (_, Base::Dynamic | Base::Unknown | Base::Void) => true,
            (Base::Null, _) => b.nullable,
            (_, Base::Null) => false,
            _ => {
                a.base == b.base
                    && (!a.nullable || b.nullable)
                    && (a.args == b.args || self.args_subtype(a.args, b.args))
            }
        }
    }

    /// Routes a declared type to its consumer: the open variable
    /// declaration, the pending formal, the open `is`-test, or the open
    /// `as`-cast. The written-argument id rides beside the type (#611
    /// step 4) — every type row passes through here, so each sink sees
    /// the pair as one fact.
    fn sink_type(&mut self, ty: Ty, args: u32) {
        if self.in_decl {
            self.decl.ty = ty;
            self.decl.args = args;
        } else if self.in_type_test {
            self.tested_ty = Some(ty);
            self.tested_args = args;
        } else if self.in_as_cast {
            self.as_cast_pending = Some(ty);
            self.as_cast_args = args;
        } else {
            self.pending_ty = Some(ty);
            self.pending_ty_args = args;
        }
    }

    /// A successful type test on `var`: returns (matched, unmatched)
    /// states (official `tryPromoteForTypeCheck`,
    /// flow_analysis.dart:4118-4179). The matched branch promotes to the
    /// gated candidate (its written arguments are the tested spelling's,
    /// `tested_args`); the unmatched branch promotes to the factored
    /// remainder `factor(previous, tested)` — shape-preserving at this
    /// seam, so the remainder keeps the previous chain head's rows — and
    /// a bottom remainder marks it unreachable (official
    /// `ifFalseIsUnreachable`, the sound arm —
    /// `sound-flow-analysis` is enabled-by-default and expired,
    /// `experiments/flags.dart:309-315`).
    fn branch_type_test(
        &mut self,
        var: u32,
        tested: Ty,
        tested_args: u32,
        promoted: Option<Ty>,
    ) -> (State, State) {
        let (previous, previous_args) = {
            let model = self.current.model(var);
            (
                self.promoted_type_of(var, &model),
                self.promoted_args_of(var, &model),
            )
        };
        let matched = self.current.clone().finish_type_test(
            self.ops,
            &mut self.lists,
            var,
            Some((tested, tested_args)),
            promoted.map(|ty| (ty, tested_args)),
        );
        let factored = self.ops.factor(previous, tested);
        let unmatched = if self.ops.is_bottom(factored) {
            self.current.clone().set_unreachable(&mut self.reach)
        } else {
            // Official `isValidPromotionStep`: the factor output is a
            // subtype of `previous` by construction, so the step is valid
            // exactly when `previous` is not a subtype of it (no mutual
            // subtypes, no repeats).
            let factored = if self.ops.is_subtype(previous, factored) {
                None
            } else {
                Some(factored)
            };
            self.current.clone().finish_type_test(
                self.ops,
                &mut self.lists,
                var,
                Some((tested, tested_args)),
                factored.map(|ty| (ty, previous_args)),
            )
        };
        (matched, unmatched)
    }

    /// The official `_equalityCheck` at the name level: a null literal on
    /// one side promotes (or un-promotes) the other side's reference in
    /// the branch states.
    fn equality_check(&mut self, left: &Expr, right: &Expr, inverted: bool) {
        let mut expr = Expr::trivial(Ty::UNKNOWN);
        let null_side = |e: &Expr| e.ty.base == crate::ty::Base::Null;
        let reference = if null_side(right) {
            left.reference
        } else if null_side(left) {
            right.reference
        } else {
            None
        };
        if let Some(var) = reference {
            let operand = if null_side(right) { left } else { right };
            if is_non_nullable(self.ops, operand.ty) {
                // Never null: the equality result is a known constant.
                expr.bool_value = Some(inverted);
                expr.branches = None;
            } else {
                let promoted = self.ops.promote_to_non_null(operand.ty);
                let matched = self.current.clone().finish_type_test(
                    self.ops,
                    &mut self.lists,
                    var,
                    None,
                    Some((promoted, operand.args)),
                );
                // `x == null`: the true-branch saw null; the
                // false-branch saw non-null. `!=` inverts.
                let (eq_true, eq_false) = (self.current.clone(), matched);
                let (if_true, if_false) = if inverted {
                    (eq_false, eq_true)
                } else {
                    (eq_true, eq_false)
                };
                self.current = if_true.clone();
                expr.branches = Some((if_true, if_false));
            }
        } else {
            let cur = self.current.clone();
            expr.branches = Some((cur.clone(), cur));
        }
        self.exprs.push(expr);
    }
}
