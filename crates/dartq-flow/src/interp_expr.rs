//! Expression events: references, tests, boolean/null-aware branching,
//! assignments, and collection elements.

use crate::ev::{BinOp, Ev};
pub(crate) use crate::interp::Expr;
use crate::facts::FlowFact;
use crate::interp::{Frame, Interp};
use crate::model::State;
use crate::model::is_non_nullable;
use crate::ty::{Base, Ty};

impl<'o> Interp<'o> {
    /// Handles one expression-layer event; `false` means the event belongs
    /// to another layer.
    pub(crate) fn expr_step(&mut self, ev: &Ev, _events: &[Ev], _i: usize, _match_of: &[u32]) -> bool {
        match ev {
            Ev::Ident {
                name,
                token,
                context,
            } => {
                use dartq_parser::IdentifierContext as Ctx;
                match context {
                    Ctx::Expression | Ctx::ExpressionContinuation => {
                        match self.resolve(*name) {
                            Some(var) => {
                                self.check_unassigned_read(*token, var);
                                self.record_read(var);
                                let model = self.current.model(var);
                                let ty = self.promoted_type_of(var, &model);
                                let mut expr = Expr::of_reference(var, ty);
                                expr.read_span = Some(*token);
                                self.exprs.push(expr);
                            }
                            None => self.exprs.push(Expr::trivial(Ty::UNKNOWN)),
                        }
                    }
                    // Named-argument labels and declaration names carry no
                    // value; catch/declaration names never reach the
                    // expression stack.
                    _ => {}
                }
            }
            Ev::Type { base, nullable } => {
                let ty = Ty {
                    base: *base,
                    nullable: *nullable,
                };
                self.sink_type(ty);
            }
            Ev::NoType => self.sink_type(Ty::UNKNOWN),
            Ev::VoidType => self.sink_type(Ty {
                base: crate::ty::Base::Void,
                nullable: false,
            }),
            Ev::FunctionType => self.sink_type(Ty::UNKNOWN),
            Ev::Bool { value } => {
                let mut expr = Expr::trivial(Ty::UNKNOWN);
                expr.bool_value = Some(*value);
                self.exprs.push(expr);
            }
            Ev::Null => self.exprs.push(Expr::trivial(Ty::NULL)),
            Ev::OtherExpr { .. } => self.exprs.push(Expr::trivial(Ty::UNKNOWN)),
            Ev::UnaryPrefix { is_not } => {
                let operand = self.exprs.pop().expect("unary operand");
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
                let operand = self.exprs.pop().expect("assert operand");
                if let Some(var) = operand.reference {
                    let promoted = self.ops.promote_to_non_null(operand.ty);
                    if promoted != operand.ty {
                        self.current = self.current.clone().finish_type_test(
                            self.ops,
                            &mut self.lists,
                            var,
                            None,
                            Some(promoted),
                        );
                    }
                    self.exprs
                        .push(Expr::trivial(self.ops.promote_to_non_null(operand.ty)));
                } else {
                    self.exprs
                        .push(Expr::trivial(self.ops.promote_to_non_null(operand.ty)));
                }
            }
            Ev::UnaryPrefixAssign | Ev::UnaryPostfixAssign => {
                // `++x` / `x++`: the target reference was pushed; the write
                // stores a non-null value of the declared type.
                let target = self.exprs.pop().expect("increment target");
                if let Some(var) = target.reference {
                    let declared = self.vars[var as usize].ty;
                    self.write_var(var, self.ops.promote_to_non_null(declared), false);
                }
                self.exprs.push(Expr::trivial(Ty::UNKNOWN));
            }
            Ev::BinaryBegin { op } => match op {
                BinOp::And | BinOp::Or => {
                    // The left operand is on the stack; split on it and run
                    // the right operand under the taken branch (official
                    // `logicalBinaryOp_rightBegin`).
                    let left = self.exprs.pop().expect("binary left");
                    let (left_if_true, left_if_false) = self.cond_split(&left);
                    let (taken, other) = if matches!(op, BinOp::And) {
                        (left_if_true, left_if_false)
                    } else {
                        (left_if_false, left_if_true)
                    };
                    self.stack.push(Frame::Branch { branch: other });
                    self.current = taken;
                }
                _ => {}
            },
            Ev::BinaryEnd { op } => match op {
                BinOp::And | BinOp::Or => {
                    let right = self.exprs.pop().expect("binary right");
                    let (right_if_true, right_if_false) = self.cond_split(&right);
                    let frame = self.stack.pop().expect("branch frame");
                    let Frame::Branch { branch: other } = frame else {
                        self.stack.push(frame);
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
                BinOp::Eq | BinOp::NotEq => {
                    let right = self.exprs.pop().expect("binary right");
                    let left = self.exprs.pop().expect("binary left");
                    let inverted = matches!(op, BinOp::NotEq);
                    self.equality_check(&left, &right, inverted);
                }
                _ => {
                    self.exprs.pop();
                    self.exprs.pop();
                    self.exprs.push(Expr::trivial(Ty::UNKNOWN));
                }
            },
            Ev::IsBegin => self.in_type_test = true,
            Ev::IsOperator { is_not } => {
                // The tested type arrived through Type events (before the
                // operator event).
                let tested = self.tested_ty.take().unwrap_or(Ty::UNKNOWN);
                self.type_test_pending = Some((*is_not, tested));
            }
            Ev::IsEnd => {
                self.in_type_test = false;
                let Some((is_not, tested)) = self.type_test_pending.take() else {
                    return true;
                };
                let operand = self.exprs.pop().expect("is operand");
                let mut expr = Expr::trivial(Ty::UNKNOWN);
                match operand.reference {
                    Some(var) => {
                        let promoted = self.promotion_candidate(var, tested);
                        let (matched, unmatched) = self.branch_type_test(var, tested, promoted);
                        let (if_true, if_false) = if is_not {
                            (unmatched, matched)
                        } else {
                            (matched, unmatched)
                        };
                        self.current = if_true.clone();
                        expr.branches = Some((if_true, if_false));
                    }
                    None => {
                        let cur = self.current.clone();
                        expr.branches = Some((cur.clone(), cur));
                    }
                }
                self.exprs.push(expr);
            }
            Ev::AsBegin => self.in_as_cast = true,
            Ev::AsOperator => {
                let cast_ty = self.tested_ty.take().unwrap_or(Ty::UNKNOWN);
                self.as_cast_pending = Some(cast_ty);
            }
            Ev::AsEnd => {
                self.in_as_cast = false;
                let Some(cast_ty) = self.as_cast_pending.take() else {
                    return true;
                };
                let operand = self.exprs.pop().expect("as operand");
                if let Some(var) = operand.reference {
                    let promoted = self.promotion_candidate(var, cast_ty);
                    if let Some(promoted) = promoted {
                        self.current = self.current.clone().finish_type_test(
                            self.ops,
                            &mut self.lists,
                            var,
                            None,
                            Some(promoted),
                        );
                    }
                }
                self.exprs.push(Expr::trivial(cast_ty));
            }
            Ev::Assign { plain } => {
                let rhs = self.exprs.pop().expect("assign rhs");
                let target = self.exprs.pop().expect("assign target");
                if let Some(var) = target.reference {
                    let written = if *plain { rhs.ty } else { Ty::UNKNOWN };
                    if let Some(span) = target.read_span {
                        // A plain assignment's target identifier is the
                        // write, not a read: retract its fact.
                        if *plain {
                            self.facts.retain(|fact| {
                                !matches!(fact, FlowFact::UnassignedRead { span: s, .. } if *s == span)
                            });
                        }
                    }
                    self.write_var(var, written, *plain);
                    // The assignment expression reads back the variable.
                    self.exprs.push(Expr::of_reference(var, written));
                } else {
                    self.exprs
                        .push(Expr::trivial(if *plain { rhs.ty } else { Ty::UNKNOWN }));
                }
            }
            Ev::PatternAssign => {
                let rhs = self.exprs.pop().expect("pattern assign rhs");
                for var in std::mem::take(&mut self.pending_writes) {
                    self.write_var(var, rhs.ty, true);
                }
            }
            Ev::CondBegin => {
                let cond = self.exprs.pop().expect("conditional condition");
                let (if_true, if_false) = self.cond_split(&cond);
                self.stack.push(Frame::Cond {
                    branch: if_false,
                    then_state: None,
                });
                self.current = if_true;
            }
            Ev::CondColon => {
                let then_expr = self.exprs.pop().expect("then expression");
                if let Some(Frame::Cond {
                    branch,
                    then_state,
                }) = self.stack.last_mut()
                {
                    *then_state = Some(self.current.clone());
                    self.current = branch.clone();
                }
                let _ = then_expr;
                // The then value is replaced by the whole conditional's
                // value at CondEnd.
            }
            Ev::CondEnd => {
                let else_expr = self.exprs.pop().expect("else expression");
                let frame = self.stack.pop().expect("cond frame");
                let Frame::Cond { then_state, .. } = frame else {
                    self.stack.push(frame);
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
                let target = self.exprs.pop().expect("access target");
                let mut expr = Expr::trivial(Ty::UNKNOWN);
                if *null_aware {
                    // Official nullAwareAccess_end: the access path sees
                    // the target promoted non-null; the skip path does not.
                    if let (Some(var), false) = (target.reference, is_non_nullable(self.ops, target.ty))
                    {
                        let promoted = self.ops.promote_to_non_null(target.ty);
                        let matched = self.current.clone().finish_type_test(
                            self.ops,
                            &mut self.lists,
                            var,
                            None,
                            Some(promoted),
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
            Ev::ArgumentsBegin => {}
            Ev::ArgumentsEnd { count } => {
                for _ in 0..*count {
                    self.exprs.pop();
                }
            }
            Ev::Throw => {
                let _ = self.exprs.pop();
                self.unreachable_cur();
            }
            Ev::Rethrow => {
                self.unreachable_cur();
            }
            Ev::ParenBegin | Ev::ParenEnd => {}
            Ev::RecordLitEnd { .. } => {
                // Field values stay consumed by their enclosing literal
                // grammar; nothing net-pushes.
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
                let frame = self.stack.pop().expect("cascade frame");
                if let Frame::Cascade { exprs_base } = frame {
                    // Keep the cascade target as the value.
                    self.exprs.truncate(exprs_base);
                } else {
                    self.stack.push(frame);
                }
            }
            Ev::EndLiteralString { interpolation_count } => {
                for _ in 0..*interpolation_count {
                    self.exprs.pop();
                }
                self.exprs.push(Expr::trivial(Ty::UNKNOWN));
            }
            Ev::Spread | Ev::NullAwareElement => {
                self.exprs.pop();
            }
            Ev::IfControlFlowBegin => {
                let cond = self.exprs.pop().expect("element condition");
                let (if_true, if_false) = self.cond_split(&cond);
                self.stack.push(Frame::Branch { branch: if_false });
                self.current = if_true;
            }
            Ev::ThenControlFlow => {}
            Ev::ElseControlFlow => {
                // The then-element is consumed; the else-element runs under
                // the condition-false branch.
                self.exprs.pop();
                if let Some(Frame::Branch { branch }) = self.stack.last() {
                    self.current = branch.clone();
                }
            }
            Ev::EndIfControlFlow { has_else } => {
                let _ = self.exprs.pop();
                if *has_else {
                    // The then-element was popped at ElseControlFlow.
                }
                let frame = self.stack.pop().expect("if-element frame");
                let Frame::Branch { branch } = frame else {
                    self.stack.push(frame);
                    return true;
                };
                let joined = self.join(Some(branch), Some(self.current.clone()));
                self.current = joined;
            }
            Ev::AwaitBegin => {}
            Ev::ExpressionFunctionBodyEnd | Ev::ExprBody => {
                // The arrow body's value is consumed by the function.
                self.exprs.pop();
            }
            _ => return false,
        }
        true
    }

    // —— shared expression helpers ———————————————————————————————————————

    /// The promoted type of a variable read (chain head or declared).
    pub(crate) fn promoted_type_of(&self, var: u32, model: &crate::model::VarModel) -> Ty {
        if model.promoted != crate::model::NIL {
            self.lists.head(model.promoted)
        } else {
            self.vars[var as usize].ty
        }
    }

    /// The promotion target for a type check: the tested type factored
    /// against every existing promotion (official
    /// `tryPromoteForTypeCheck`). `None` when the check target is
    /// unknowable at the name level.
    fn promotion_candidate(&mut self, var: u32, tested: Ty) -> Option<Ty> {
        match tested.base {
            Base::Unknown | Base::Dynamic | Base::Void => None,
            _ => {
                let model = self.current.model(var);
                let mut candidate = tested;
                for ty in self.lists.iter(model.promoted) {
                    candidate = self.ops.factor(candidate, ty);
                }
                Some(candidate)
            }
        }
    }

    /// Routes a declared type to its consumer: the open variable
    /// declaration, the pending formal, the open `is`-test, or the open
    /// `as`-cast.
    fn sink_type(&mut self, ty: Ty) {
        if self.in_decl {
            self.decl.ty = ty;
        } else if self.in_type_test {
            self.tested_ty = Some(ty);
        } else if self.in_as_cast {
            self.as_cast_pending = Some(ty);
        } else {
            self.pending_ty = Some(ty);
        }
    }

    /// A successful type test on `var`: returns (matched, unmatched)
    /// states (official `_finishTypeTest` splitting).
    fn branch_type_test(&mut self, var: u32, tested: Ty, promoted: Option<Ty>) -> (State, State) {
        let matched = self.current.clone().finish_type_test(
            self.ops,
            &mut self.lists,
            var,
            Some(tested),
            promoted,
        );
        (matched, self.current.clone())
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
        match reference {
            Some(var) => {
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
                        Some(promoted),
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
            }
            None => {
                let cur = self.current.clone();
                expr.branches = Some((cur.clone(), cur));
            }
        }
        self.exprs.push(expr);
    }
}
