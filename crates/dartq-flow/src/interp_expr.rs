//! Expression events: references, tests, boolean/null-aware branching,
//! assignments, and collection elements.

use dartq_parser::TokenSpan;
use dartq_source::SourceRange;

use crate::ev::{BinOp, Ev};
use crate::facts::{FlowFact, PropReceiver};
pub(crate) use crate::interp::Expr;
use crate::interp::{Frame, Interp, Resolved, THIS_RECEIVER};
use crate::model::State;
use crate::ty::{Base, Classification, Ty};

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
                    // A member selector (`x.name`, `..name`, `p.name`): the
                    // lexical scope never answers it — a like-named local
                    // (`var manifest = element.manifest?.instance`) is not
                    // read, written, or typed by the access (the official
                    // selector resolution never consults the scope). The
                    // DotAccess fold types the access from the receiver's
                    // own slot.
                    Ctx::ExpressionContinuation => {
                        let mut expr = Expr::trivial(Ty::UNKNOWN_NULLABLE);
                        expr.name = Some(*name);
                        expr.read_span = Some(*token);
                        self.exprs.push(expr);
                    }
                    Ctx::Expression if matches!(events.get(i + 1), Some(Ev::ForInExprBegin)) => {
                        // A for-in loop target (`for (i in xs)`) is the
                        // write, not a read (analyzer for_resolver.dart
                        // :237-241 `flow?.write` right before
                        // `forEach_bodyBegin`); the write consumes the
                        // reference, leaving the iterable as the head's
                        // only value. Its anchor is the target identifier
                        // itself (the official's write node) — the
                        // `DemoteViaExplicitWrite` context row's span. A
                        // top-level target carries no definite-assignment
                        // face.
                        if let Some(Resolved::Local(var)) = self.resolve(*name) {
                            self.write_var(
                                var,
                                Ty::UNKNOWN,
                                crate::ty::EMPTY_ARGS,
                                false,
                                Some(token.range),
                                false,
                            );
                        }
                    }
                    Ctx::Expression => match self.resolve(*name) {
                        Some(Resolved::Local(var)) => {
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
                            // Dead-region reads still carry the row: the
                            // official materializes the promotion under an
                            // unreachable state too (its `PromotedRead`
                            // evidence keys the consumer's verdict, not the
                            // reachability), and the region's own dead row
                            // stays the only reachability face.
                            if ty != declared {
                                self.emit(FlowFact::PromotedRead {
                                    span: *token,
                                    ty,
                                    args,
                                });
                            }
                            // The read-type row (the semantic query face's
                            // family-a): every read carries its flow type —
                            // promoted or declared, reachable or dead — so
                            // a consumer can ask for the static type at the
                            // span without re-running the fold. Same
                            // retraction invariant as the promotion fact
                            // above: a plain assignment's target read
                            // retracts with it (the target identifier is a
                            // write, not a read).
                            self.emit(FlowFact::ReadType {
                                span: *token,
                                ty,
                                args,
                            });
                            // The why-not-promoted row (upstream
                            // `whyNotPromoted`'s variable arm,
                            // flow_analysis.dart:8832-8857): a reachable
                            // read of a variable whose history is non-empty
                            // — at least one recorded demotion stands
                            // between the variable's past promotion and
                            // this read. The row's entries carry the
                            // `(type, reason)` pairs a consumer renders
                            // into the context messages (empty after the
                            // official filters — all links below the flow
                            // type — render nothing, the empty-map path).
                            if self.reachable() && model.history != crate::model::NIL {
                                let entries =
                                    self.intern_non_promotion(Some((ty, args)), model.history);
                                self.emit(FlowFact::WhyNotPromoted {
                                    span: *token,
                                    current: ty,
                                    current_args: args,
                                    entries,
                                });
                            }
                            let mut expr = Expr::of_reference(var, ty, args);
                            expr.read_span = Some(*token);
                            expr.name = Some(*name);
                            self.exprs.push(expr);
                        }
                        Some(Resolved::Top(top)) => {
                            // The unit's own top-level declaration (the
                            // lexical fallback's answer): the read row
                            // carries the written type. No
                            // definite-assignment or promotion state — the
                            // official promotable-element face covers
                            // locals and parameters; a dead read carries no
                            // row like any other.
                            if self.reachable() {
                                self.emit(FlowFact::ReadType {
                                    span: *token,
                                    ty: top.ty,
                                    args: top.args,
                                });
                            }
                            let mut expr = Expr::trivial(top.ty);
                            expr.args = top.args;
                            expr.read_span = Some(*token);
                            expr.name = Some(*name);
                            self.exprs.push(expr);
                        }
                        None => {
                            // The unresolved bare read: the implicit-`this`
                            // member face's flow shape (#1409). This layer
                            // cannot tell an instance member from a
                            // top-level name or a typo — every unresolved
                            // read tracks the `(this, name)` slot and the
                            // semantic eligibility gate decides whether
                            // the member is a promotable field (official
                            // `_resolveUnqualifiedNameRead`'s
                            // `thisLookupGetter2` dispatch,
                            // property_element_resolver.dart:2137/:2183).
                            let mut expr = Expr::trivial(Ty::UNKNOWN_NULLABLE);
                            let slot = self.prop_slot(THIS_RECEIVER, *name);
                            let promoted = self.prop_promoted(THIS_RECEIVER, slot);
                            if promoted && self.reachable() {
                                self.emit(FlowFact::PromotedPropertyRead { span: *token });
                            } else if self.reachable() {
                                // The non-promoted mirror (upstream
                                // `_getNonPromotionReasons`'s property
                                // arm): the slot's cleared promoted
                                // types stand as evidence for the
                                // consumer's element-layer verdict.
                                let history = self.current.prop(slot).history;
                                if history != crate::model::NIL {
                                    let entries = self.intern_non_promotion(None, history);
                                    self.emit(FlowFact::NonPromotedPropertyRead {
                                        span: *token,
                                        name: *name,
                                        receiver: PropReceiver::This,
                                        entries,
                                    });
                                }
                            }
                            expr.ty = if promoted {
                                Ty::UNKNOWN
                            } else {
                                Ty::UNKNOWN_NULLABLE
                            };
                            expr.prop = Some(slot);
                            expr.name = Some(*name);
                            expr.read_span = Some(*token);
                            self.exprs.push(expr);
                        }
                    },
                    // A constructor-reference head (`new C()`, `const C()`,
                    // `C<T>.named(...)`) is the creation's value — its
                    // static type is the head's class, so the expression
                    // carries the named non-null face and a following
                    // `x = C()` write can promote `x` to it. The selector
                    // continuations (`.named`) push nothing: the head's
                    // entry already is the invocation's result.
                    Ctx::ConstructorReference => {
                        self.exprs.push(Expr::trivial(Ty {
                            base: Base::Name(*name),
                            nullable: false,
                        }));
                    }
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
            Ev::NoType => self.sink_type(Ty::UNKNOWN, crate::ty::EMPTY_ARGS),
            // The function-type head sinks its written nullability over the
            // signature-less `Function` base: a nullable function-typed
            // declaration promotes exactly like a named one, and the
            // signature questions stay downstream (the staged promotion
            // re-decides from the real declared type).
            Ev::FunctionType { nullable } => self.sink_type(
                Ty {
                    base: crate::ty::Base::Function,
                    nullable: *nullable,
                },
                crate::ty::EMPTY_ARGS,
            ),
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
            Ev::This { span } => {
                // The `this` read: a promotable property receiver (official
                // `thisExpression()`'s `_thisVersion` reference,
                // `flow_analysis.dart:8149-8153`). The token anchors the
                // property rows keyed at `this._b`'s expression start. A
                // staged type (#2292 — the extension/extension-type
                // extended type the semantic layer resolved) replaces the
                // unknown, so the `switch (this)` scrutinee and
                // `this`-chained reads answer on the real type.
                let ty = self
                    .staged_this
                    .get(&span.range.offset)
                    .copied()
                    .unwrap_or(Ty::UNKNOWN);
                let mut expr = Expr::trivial(ty);
                expr.this_recv = true;
                expr.read_span = Some(*span);
                self.exprs.push(expr);
            }
            Ev::UnaryPrefix { is_not } => {
                // The operand ends: rejoin the null-shorting its own `?`
                // opened (`!x?.y`).
                self.flush_null_shorts_above(self.consumer_base(1));
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
                let (promoted, promoted_args) = self.promote_to_non_null(operand.ty, operand.args);
                if let Some(var) = operand.reference
                    && promoted != operand.ty
                {
                    self.current = self.current.clone().finish_type_test(
                        self.ops,
                        &mut self.lists,
                        var,
                        None,
                        Some((promoted, promoted_args)),
                        self.args,
                        self.names,
                    );
                } else if let Some(slot) = operand.prop {
                    // Official `nonNullAssert_end` marks whatever reference
                    // the operand read carries non-nullable
                    // (`tryMarkNonNullable`, flow_analysis.dart:7384-7404)
                    // — the property face included: `sharedWorker!` promotes
                    // the `(receiver, member)` slot, so the reads that
                    // follow in the block are non-null. The eligibility
                    // faces are the same ones the property null check uses:
                    // a captured receiver or slot leaves the slot alone.
                    let receiver = self.props[slot as usize].0;
                    if !self.prop_target_captured(receiver)
                        && !self.current.prop(slot).captured()
                    {
                        self.current = self.current.clone().promote_prop_non_null(
                            self.ops,
                            &mut self.lists,
                            slot,
                            Ty::UNKNOWN,
                            self.args,
                            self.names,
                        );
                    }
                }
                self.exprs.push(Expr::trivial(promoted));
            }
            Ev::UnaryPrefixAssign { .. } | Ev::UnaryPostfixAssign { .. } => {
                // `++x` / `x++`: the target reference was pushed; the write
                // stores a non-null value of the declared type. The write's
                // own range is the increment expression (the official
                // `flow.write`/`postIncDec` node,
                // increment_or_decrement_resolver.dart:332-348): a prefix
                // form spans its operator through the operand, a postfix
                // form the operand through its operator.
                let (prefix_op, postfix_end) = match ev {
                    Ev::UnaryPrefixAssign { span } => (Some(span.range), None),
                    Ev::UnaryPostfixAssign { end } => (None, Some(*end)),
                    _ => unreachable!("non-increment event in increment arm"),
                };
                let target = self.pop_value();
                if let Some(var) = target.reference {
                    let (declared, declared_args) = self
                        .vars
                        .get(var as usize)
                        .map_or((Ty::UNKNOWN, crate::ty::EMPTY_ARGS), |meta| {
                            (meta.ty, meta.args)
                        });
                    let write_span = match (
                        prefix_op,
                        target.read_span.map(|token| token.range),
                        postfix_end,
                    ) {
                        (Some(op), Some(operand), _) => Some(SourceRange {
                            offset: op.offset,
                            length: operand.end().saturating_sub(op.offset),
                        }),
                        (_, Some(operand), Some(end)) => Some(SourceRange {
                            offset: operand.offset,
                            length: end.saturating_sub(operand.offset),
                        }),
                        // A local write's target read always carries its
                        // token; a synthetic shape degrades to no anchor
                        // (the demotion then records no reason link, the
                        // official's unreasoned-write shape).
                        _ => None,
                    };
                    self.write_var(
                        var,
                        self.ops.promote_to_non_null(declared),
                        declared_args,
                        false,
                        write_span,
                        false,
                    );
                }
                self.exprs.push(Expr::trivial(Ty::UNKNOWN));
                // The whole read-modify-write is one expression — an open
                // null-shorting context (`x?.y++`) rejoins after its write
                // lands on the access path; an enclosing `?` chain stays
                // open.
                self.flush_null_shorts_above(self.consumer_base(1));
            }
            Ev::BinaryBegin { op, span } => {
                // The left operand ended: its own null-shorting rejoins
                // before the operator's split (`x?.y && z` — official
                // `finishNullShorting` at the operand boundary).
                self.flush_null_shorts_above(self.consumer_base(1));
                match op {
                    BinOp::And | BinOp::Or => {
                        // The left operand is on the stack; split on it and run
                        // the right operand under the taken branch (official
                        // `logicalBinaryOp_rightBegin`).
                        let left = self.pop_value();
                        // The left operand's own query (official
                        // `checkForNonBoolExpression` at each `&&`/`||` node —
                        // binary_expression_resolver.dart:168), at its
                        // evaluation point.
                        self.condition_operand(&left);
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
                        let (promoted, promoted_args) =
                            self.promote_to_non_null(left.ty, left.args);
                        let shortcut = match left.reference {
                            Some(var) if promoted != left.ty => {
                                self.current.clone().finish_type_test(
                                    self.ops,
                                    &mut self.lists,
                                    var,
                                    None,
                                    Some((promoted, promoted_args)),
                                    self.args,
                                    self.names,
                                )
                            }
                            _ => self.current.clone(),
                        };
                        let mut rhs_dead_entry = false;
                        if self.classify(left.ty) == Classification::NonNullable {
                            self.unreachable_cur();
                            rhs_dead_entry = true;
                        }
                        // `??` types as the LHS's non-null form.
                        let result_ty = promoted;
                        self.stack.push(Frame::Branch {
                            branch: shortcut,
                            ifnull_ty: Some(result_ty),
                            op_span: *span,
                            rhs_dead_entry,
                        });
                    }
                    _ => {}
                }
            }
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
                    // The right operand evaluated under the left's true
                    // branch (for `||`: the left's false branch) — its row
                    // carries the promotion standing there (official query
                    // point per operand, bool_expression_verifier.dart:45-73).
                    self.condition_operand(&right);
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
                    // The `&&`/`||` arms carry the left operand's
                    // type-test split (the narrowed `current` stands in the
                    // taken arm): a value consumer drops the result and
                    // rejoins both arms, exactly like a bare test's value
                    // context.
                    expr.test_split = true;
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
                    let (op_span, rhs_dead_entry) = match self.stack.last() {
                        Some(Frame::Branch {
                            op_span,
                            rhs_dead_entry,
                            ..
                        }) => (Some(*op_span), *rhs_dead_entry),
                        _ => (None, false),
                    };
                    if rhs_dead_entry
                        && let Some(op_span) = op_span
                    {
                        self.dead_expr_arm(op_span, *end);
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
                    // The expression's type is `LUB(NonNull(lhs), rhs)` —
                    // nullable exactly when the right side can run and is
                    // itself nullable (the official types `int? ?? int?`
                    // as `int?`). A guaranteed-non-null left side never
                    // evaluates the right, so its result stays the left's
                    // non-null form; a nullable right side makes the
                    // nullable face observable down-chain (`(a ?? b) ?? c`
                    // keeps `c` live), where stamping the left's non-null
                    // form alone deads every later `??` arm.
                    let mut result_ty = ifnull_ty.unwrap_or(Ty::UNKNOWN);
                    // The join's other operand: the official's
                    // `LUB(NonNull(lhs), rhs)` sees the RIGHT side too —
                    // an unresolved LHS (`c.x` reading the opaque marker)
                    // still types the result as the RHS's face
                    // (`LUB(InvalidType, String)` = `String`,
                    // least_upper_bound.dart:55), and a nullable RHS widens
                    // a spelled LHS the same way `int? ?? int?` does.
                    result_ty = self.name_lub(result_ty, right.ty);
                    // A literal right operand (collection, record, string)
                    // is never null: the official analyzes it against the
                    // if-null context and `LUB(NonNull(T1), T2)` keeps the
                    // non-null form (`resolveIfNull`,
                    // binary_expression_resolver.dart:69-109). The value
                    // this layer carries for such a literal is the opaque
                    // UNKNOWN marker, whose classification is
                    // potentially-nullable — without the gate
                    // `List<Object>? args; args = args ?? const []` would
                    // widen back to nullable and demote `args` on the write.
                    // The operand's last event is its own closer: any
                    // wrapper (`(…)`, a send, a selector) records its own
                    // event after it, so only a bare literal matches.
                    let rhs_literal = matches!(
                        events.get(i.wrapping_sub(1)),
                        Some(
                            Ev::ListLitEnd { .. }
                                | Ev::RecordLitEnd { .. }
                                | Ev::EndLiteralString { .. }
                        )
                    );
                    // A `null` LHS promotes to Never, and the literal's own
                    // face is unknowable here: keep the widened form rather
                    // than stamping the LHS's bottom onto the result.
                    if !rhs_dead_entry
                        && !(rhs_literal && result_ty != Ty::NEVER)
                        && self.classify(result_ty) == Classification::NonNullable
                        && self.classify(right.ty) != Classification::NonNullable
                    {
                        result_ty = result_ty.make_nullable();
                    }
                    // The result's written arguments are the winning side's
                    // (`#2204`-style pairing): the RHS's own tree when the
                    // join lands on it, the LHS's non-null form's otherwise.
                    // The left operand is long folded, but its read staged
                    // its `(ty, args)` pair as the last read fact ahead of
                    // the `??` operator — the read the non-null form
                    // (`promote_to_non_null`) refines, so its arg tree is the
                    // promoted result's. A composite LHS (`f() ?? d`, `a.b ??
                    // d`, `(a ?? b) ?? d`) mismatches the head and falls back
                    // to the empty tree — the same under-instantiated face an
                    // unresolvable RHS already carries. Without it
                    // `List<Object>? x; x = x ?? const []` wrote `List` with
                    // no arguments and the type-of-interest promotion dropped
                    // (`List<dynamic>` is not a subtype of `List<Object>`).
                    let result_args = if result_ty == right.ty {
                        right.args
                    } else {
                        op_span
                            .and_then(|op| {
                                self.facts.iter().rev().find_map(|fact| match fact {
                                    FlowFact::ReadType { span, ty, args }
                                    | FlowFact::PromotedRead { span, ty, args }
                                        if span.range.end() <= op.range.offset
                                            && self.promote_to_non_null(*ty, *args).0
                                                == result_ty =>
                                    {
                                        Some(*args)
                                    }
                                    _ => None,
                                })
                            })
                            .unwrap_or(crate::ty::EMPTY_ARGS)
                    };
                    let mut expr = Expr::trivial(result_ty);
                    expr.args = result_args;
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
            Ev::IsBegin => {
                self.in_type_test = true;
                // The test operand ended (`x?.y is T`): rejoin the
                // operand's own null-shorting before the type spelling's
                // events.
                self.flush_null_shorts_above(self.consumer_base(1));
            }
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
                if let Some(var) = operand.reference
                    && !self.current.model(var).captured()
                {
                    // The official fail arm decides before the reference
                    // branch and covers promotable references too — a
                    // reference operand never short-circuits it (upstream
                    // Dart SDK source, pkg/_fe_analyzer_shared/lib/src/
                    // flow_analysis/flow_analysis.dart:7184-7189). It reads
                    // the operand's *declared* static type:
                    // `isExpression_end`'s `subExpressionType` is the
                    // sub-expression's AST static type
                    // (flow_analysis_visitor.dart:362-369,
                    // `expression.typeOrThrow`), promotion-free — a
                    // promoted face would over-fire (`Object? x` promoted
                    // to `String` then `x is Null` stays live officially,
                    // while `String` is disjoint from `Null`). The fold is
                    // `booleanLiteral(isNot)` (:6544-6559): no promotion
                    // split rides it, so `branch_type_test` is skipped and
                    // `test_split` stays off; `current` keeps the pre-test
                    // state like the non-reference folds — the deadness
                    // rides the branch pair.
                    //
                    // The guaranteed-succeed arm has no reference face
                    // here: upstream reaches it only when
                    // `_getExpressionReference` returned null
                    // (:7190-7205), so a reference test never folds to
                    // `true` on a mere subtype fact (`int x; x is num`
                    // keeps the else live). The reference else-deadness
                    // comes from the promotion split itself — a bottom
                    // `factor(previous, tested)` marks the unmatched arm
                    // unreachable (`tryPromoteForTypeCheck`'s
                    // `ifFalseIsUnreachable`, :4156-4160), already
                    // [`Interp::branch_type_test`]'s face.
                    let declared =
                        self.vars.get(var as usize).map_or(Ty::UNKNOWN, |meta| meta.ty);
                    if self.is_test_guaranteed_fail(declared, tested) {
                        let cur = self.current.clone();
                        let dead = cur.clone().set_unreachable(&mut self.reach);
                        expr.branches = Some(if is_not { (cur, dead) } else { (dead, cur) });
                    } else {
                        let promoted = self.promotion_candidate(var, tested, tested_args);
                        let (matched, unmatched) =
                            self.branch_type_test(var, tested, tested_args, promoted);
                        let (if_true, if_false) = if is_not {
                            (unmatched, matched)
                        } else {
                            (matched, unmatched)
                        };
                        // The split rides `Expr::branches` for the condition
                        // consumers (`cond_split`) and `current` narrows to the
                        // true arm for the guard machinery's contract
                        // (`GuardBegin` snapshots the pre-guard state and the
                        // guard-true arm IS this drift). A value consumer that
                        // drops the expression rejoins both arms
                        // ([`Interp::rejoin_value_split`]) — an `is` result
                        // read as a plain value narrows nothing officially.
                        self.current = if_true.clone();
                        expr.branches = Some((if_true, if_false));
                        expr.test_split = true;
                    }
                } else {
                    let cur = self.current.clone();
                    // The non-reference guaranteed-fail face (official
                    // `isExpression_end`'s first arm — upstream Dart SDK
                    // source, pkg/_fe_analyzer_shared/lib/src/flow_analysis/
                    // flow_analysis.dart:7184-7189 — folded to
                    // `booleanLiteral(isNot)`, decided before the reference
                    // promotion and the guaranteed-succeed arm): a bottom
                    // tested type (`Never`) or a nullability-disjoint pair
                    // makes the test a constant `false` — `is` deads the
                    // then branch, `is!` the else. `current` stays live
                    // like the succeed face: the deadness rides the branch
                    // pair, and a value consumer that drops the expression
                    // never sees it.
                    if self.is_test_guaranteed_fail(operand.ty, tested) {
                        let dead = cur.clone().set_unreachable(&mut self.reach);
                        expr.branches = Some(if is_not { (cur, dead) } else { (dead, cur) });
                    }
                    // The non-reference guaranteed-succeed face (official
                    // `isExpression_end`'s
                    // `_isTypeCheckGuaranteedToSucceedWithSoundNullSafety`
                    // arm — upstream Dart SDK source,
                    // pkg/_fe_analyzer_shared/lib/src/flow_analysis/
                    // flow_analysis.dart:7201-7205 — folded to
                    // `booleanLiteral(!isNot)`, :6544-6559): the operand's
                    // static type is already the tested type's subtype, so
                    // the test is a boolean literal — `is` deads the else
                    // branch, `is!` the then branch. `current` stays live
                    // (the official's `_current` is untouched): the
                    // deadness rides the branch pair, and a value consumer
                    // that drops the expression never sees it.
                    else if self.is_test_guaranteed(operand.ty, operand.args, tested, tested_args)
                    {
                        let dead = cur.clone().set_unreachable(&mut self.reach);
                        expr.branches = Some(if is_not { (dead, cur) } else { (cur, dead) });
                    } else {
                        expr.branches = Some((cur.clone(), cur));
                    }
                }
                self.exprs.push(expr);
            }
            Ev::AsBegin { span } => {
                self.in_as_cast = true;
                // The cast operand ended (`x?.y as T`): rejoin the
                // operand's own null-shorting before the type spelling's
                // events.
                self.flush_null_shorts_above(self.consumer_base(1));
                // The cast-type node poll (official `checkUnreachableNode`
                // of the type before it is visited, type_analyzer.dart:2090):
                // a Never-typed operand's exit lands right before the type
                // spelling, so the cast anchors the statement's dead row —
                // `var a = f() as int;` reports at the cast. A reachable
                // state makes the poll a no-op.
                let opened = self.dead.is_none();
                self.dead_code(*span);
                // When the poll opens the interval directly inside a
                // variable initializer (no nested executable between), the
                // row belongs to the semantic layer's as-cast tail face —
                // it stages the official row at the cast type's head token
                // (probed: dart analyze 3.13.1, `DEAD_CODE` anchored there)
                // — so the interval retires without emitting rather than
                // duplicating it. A poll at a deeper executable frame (a
                // cast inside a closure body within the initializer) keeps
                // its row: that face's own frame gate stays silent there.
                if opened
                    && self.init_exec_depth == Some(self.exec_frame_depth())
                    && let Some(window) = &mut self.dead
                {
                    window.suppress();
                }
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
                // The operator's operand parsed before `AsBegin` — the
                // begin/end pair wraps only the type spelling — so the
                // stack depth never changes across the cast on any face.
                // The pattern face (`p as T`; parser patterns.rs:176-187
                // reuses the as-operator events) is the walk inside a
                // pattern: the primary refines in place, pushes no value,
                // and is not a promotion candidate. An expression cast's
                // operand tops the expr stack.
                if self.pattern_depth == 0 {
                    let operand = self.pop_value();
                    // Official `asExpression_end`
                    // (flow_analysis.dart:6451-6477): a cast whose operand
                    // type and cast type are disjoint by nullability is
                    // guaranteed to throw
                    // (`_isTypeCheckGuaranteedToFailWithSoundNullSafety`)
                    // — the continuation goes unreachable here and the next
                    // executable node anchors the dead row (probed: `null
                    // as int`, `n as int` with `n: Null`, and `x as Null`
                    // all kill; a cast that ends the block reports nothing —
                    // no following node to anchor). A cast operand that
                    // never completes kills through its own `Ev::Exit`
                    // before this point; the promotion below is the only
                    // other reachability effect.
                    if self.disjoint_by_nullability(operand.ty, cast_ty) {
                        self.unreachable_cur();
                    }
                    if let Some(var) = operand.reference
                        && let Some(promoted) =
                            self.promotion_candidate(var, cast_ty, self.as_cast_args)
                    {
                        self.current = self.current.clone().finish_type_test(
                            self.ops,
                            &mut self.lists,
                            var,
                            None,
                            Some((promoted, self.as_cast_args)),
                            self.args,
                            self.names,
                        );
                    }
                    self.exprs.push(Expr::trivial(cast_ty));
                } else if let Some((name, _)) = self
                    .pattern
                    .as_ref()
                    .and_then(|p| p.pending_decls.last().copied())
                    && let Some(Resolved::Local(var)) = self.resolve(name)
                {
                    self.current = self.current.clone().finish_type_test(
                        self.ops,
                        &mut self.lists,
                        var,
                        None,
                        Some((cast_ty, self.as_cast_args)),
                        self.args,
                        self.names,
                    );
                }
            }
            Ev::Assign {
                plain,
                if_null,
                end,
                field_init,
            } => {
                let base = self.consumer_base(2);
                let rhs = self.pop_value();
                let rhs_literal = rhs.literal;
                let mut target = self.pop_value();
                // A condition-valued plain `=` RHS (`v = a != null;`) is a
                // plain value: its split rejoins (see
                // [`Interp::rejoin_value_split`]). Compound operators and
                // `??=` recombine inside their own folds.
                if *plain {
                    self.rejoin_value_split(&rhs);
                }
                // A constructor field initializer's write targets `this.f`
                // (upstream `ConstructorFieldInitializer`): the LHS name is
                // a field name, so a like-named local must take no write —
                // its read rows still retract below (the `f` of `f = e`
                // reads no local either).
                if *field_init {
                    target.reference = None;
                }
                // The read-back value's written arguments are the right-hand
                // side's when the write carried its type (a plain `=` or the
                // `??=` if-null combination), and argument-less for the
                // compound operators' unknown face.
                let rhs_args = if *plain || *if_null {
                    rhs.args
                } else {
                    crate::ty::EMPTY_ARGS
                };
                // The semantic layer's staged RHS type serves both spellable
                // writes (the plain `=` write, #2174, and the `??=` if-null
                // combination, #2360 — the oracle discriminates the null-aware
                // write's downstream verdict on the resolved RHS type alone).
                // The staged type is the resolution-level static type of the
                // RHS beside its written-argument tree (#2364); a bare local
                // read's fold type is its flow type — the read's own
                // promotion included. When the staged face is exactly the
                // read's declared type while the fold already narrowed it
                // (`a = b` with `b` promoted by an earlier
                // `if (b == null) return;`), the promotion is the oracle's
                // write type: keep it (the official read's static type IS the
                // promoted one).
                let staged = if *plain || *if_null {
                    target.read_span.and_then(|span| {
                        let (staged_ty, staged_args) = *self.staged_rhs.get(&span.range.offset)?;
                        match rhs.reference.and_then(|var| self.vars.get(var as usize)) {
                            Some(meta) if staged_ty == meta.ty && rhs.ty != meta.ty => {
                                Some((rhs.ty, rhs_args))
                            }
                            _ => Some((staged_ty, staged_args)),
                        }
                    })
                } else {
                    None
                };
                let (rhs_ty, rhs_ty_args) = staged.unwrap_or((rhs.ty, rhs_args));
                // A plain assignment's target identifier is the write, not
                // a read: retract its facts. Both read facts can coexist at
                // this span — the target read may run under an active
                // promotion (writing inside a promoted branch) and may be
                // definitely unassigned. The gate is the read anchor, not
                // the reference: a top-level target tracks no reference but
                // carries the read-type row, and it is the write's target
                // all the same (a compound operator keeps its rows — the
                // read-modify-write target is also a read).
                if *plain && let Some(span) = target.read_span {
                    self.facts.retain(|fact| {
                        !matches!(
                            fact,
                            FlowFact::UnassignedRead { span: s, .. }
                                | FlowFact::PromotedRead { span: s, .. }
                                | FlowFact::ReadType { span: s, .. }
                                | FlowFact::WhyNotPromoted { span: s, .. }
                                if *s == span
                        )
                    });
                }
                if let Some(var) = target.reference {
                    // `x ??= e`: the write's type is `LUB(NonNull(readType),
                    // rhsType)` (official `_computeIfNullType`,
                    // assignment_expression_resolver.dart:872-907) and the
                    // write promotes through types-of-interest — `x ??= 0`
                    // on `int? x` leaves it promoted to `int`, exactly like
                    // the desugared `x = x ?? 0`. Other compound operators
                    // write the unresolved operator face (UNKNOWN) and never
                    // promote. The RHS evaluated on the unsplit path — the
                    // official null-path split (`ifNullExpression_rightBegin`)
                    // only changes flow inside `e`, which a flattened replay
                    // cannot recreate; the post-state (the write + the
                    // joined result) is the oracle-visible part.
                    // The write's type resolution order: the semantic
                    // layer's staged RHS type wins when present (#2174 —
                    // the resolved send result beats this layer's
                    // name-level guesses, whose unresolved-head faces
                    // cannot spell a return type); otherwise the fold's
                    // own RHS read. A compound operator's result is the
                    // promoted read type the read-modify-write leaves
                    // (the oracle's operator result at name level —
                    // `int + 1` keeps the promoted `int`, and the write
                    // therefore never demotes what the read had
                    // promoted).
                    let (written, written_args) = if *plain {
                        (rhs_ty, rhs_ty_args)
                    } else if *if_null {
                        // `LUB(NonNull(readType), rhsType)` — the boundary-
                        // aware non-null form first, then the name-level
                        // least upper bound (a subtype-ordered join; a real
                        // hierarchy LUB is the semantic layer's question).
                        // An untyped RHS still leaves the lvalue non-null —
                        // `x ??= e` can never widen x back to nullable.
                        // The winning side's written arguments ride beside
                        // the joined type (#2204 phase D, #2364 the staged
                        // tree). The staged face
                        // rides the same join (#2360): a resolved nullable
                        // RHS (`Node?`) joins back to nullable — the
                        // receiver read after the write stays unpromoted,
                        // the oracle's `unchecked_use_of_nullable_value`
                        // face — and a resolved non-null RHS joins to the
                        // non-null face the oracle promotes through (the
                        // official write type is the assignment
                        // expression's own `nodeType`,
                        // assignment_expression_resolver.dart:698-701). An
                        // invalid RHS face (the official's InvalidType)
                        // assumes no promotion either way
                        // (flow_analysis.dart:4966-4969).
                        let (lhs_nn, lhs_nn_args) =
                            self.promote_to_non_null(target.ty, target.args);
                        // A bare unresolved identifier reads the nullable
                        // unknown face (the `None` arm of the read
                        // dispatch): a function tear-off or a name this
                        // layer carries no row for. On a function-typed
                        // lvalue the oracle's RHS is the real signature
                        // type, so `LUB(NonNull(T Function()), f)` is the
                        // non-null declared form itself — the write leaves
                        // the lvalue non-null instead of the invalid face
                        // (#2401: `compare ??= defaultCompare` on
                        // `int Function()? compare` promotes). Any other
                        // lvalue keeps the invalid-unknown arm: the RHS
                        // could be anything, and `LUB(T1, InvalidType)`
                        // promotes nothing.
                        if rhs_ty == Ty::UNKNOWN_NULLABLE && lhs_nn.base == Base::Function {
                            (lhs_nn, lhs_nn_args)
                        } else if rhs_ty == Ty::UNKNOWN || rhs_ty == Ty::UNKNOWN_NULLABLE {
                            // An information-less RHS (an SDK static's return
                            // this layer cannot see; a tear-off of a
                            // top-level function the flow walk does not
                            // resolve) types as the official's InvalidType:
                            // `LUB(NonNull(T1), InvalidType)` is InvalidType —
                            // never a promotion step, and `InvalidType <:
                            // everything` keeps any standing promotion intact
                            // (the demotion arm's `covers` recovery, #2320).
                            (Ty::UNKNOWN, crate::ty::EMPTY_ARGS)
                        } else {
                            let joined = self.name_lub(lhs_nn, rhs_ty);
                            let joined_args = if joined == rhs_ty {
                                rhs_ty_args
                            } else {
                                lhs_nn_args
                            };
                            (joined, joined_args)
                        }
                    } else {
                        // The read-modify-write face: the operator result is
                        // the promoted read type at name level (`int + 1`
                        // → `int`; a user operator returning a different
                        // type is the semantic layer's re-decision). Writing
                        // UNKNOWN here would demote what the read promoted —
                        // `initialCapacity += 1` after a null-check early
                        // return must stay promoted.
                        self.promote_to_non_null(target.ty, target.args)
                    };
                    // The write's own range: the assignment expression (the
                    // official `flow.write` node,
                    // assignment_expression_resolver.dart:322-329 — target
                    // through expression end), the
                    // `DemoteViaExplicitWrite` context row's anchor.
                    let write_span = target.read_span.map(|token| SourceRange {
                        offset: token.range.offset,
                        length: end.saturating_sub(token.range.offset),
                    });
                    self.write_var(
                        var,
                        written,
                        written_args,
                        *plain || *if_null,
                        write_span,
                        *plain && rhs_literal,
                    );
                    // The assignment expression carries no promotable
                    // reference: official `_write` hands one back only under
                    // the experimental `inferenceUpdate4` feature
                    // (flow_analysis.dart:9552), so a stable-language flow
                    // never promotes through the assignment's own value —
                    // `(x = e) != null`, `(x = e)!` and `(x = e).f` test the
                    // value, not `x`. The write above still promotes the
                    // target through its assigned type.
                    self.exprs.push(Expr {
                        args: rhs_args,
                        ..Expr::trivial(written)
                    });
                } else {
                    self.exprs
                        .push(Expr::trivial(if *plain { rhs.ty } else { Ty::UNKNOWN }));
                }
                // The assignment expression ends (`x?.y = v` shorts the
                // RHS too — official `analyzeExpression` finishes the
                // chain's shorting at the assignment's own boundary):
                // rejoin open null-shorting after the write lands on the
                // access path.
                self.flush_null_shorts_above(base);
            }
            Ev::PatternAssign => {
                // The pattern assignment's RHS ends: rejoin its open
                // null-shorting.
                self.flush_null_shorts_above(self.consumer_base(1));
                let rhs = self.pop_value();
                // No write node exists on the official pattern-assignment
                // path (no `flow.write` call in the pattern assignment's
                // resolve — the writes ride the pattern machinery), so the
                // demotions carry no reason link and the history drops: the
                // official's unreasoned-write shape.
                for var in std::mem::take(&mut self.pending_writes) {
                    self.write_var(var, rhs.ty, rhs.args, true, None, false);
                }
                // The assignment ends (official `patternAssignment_end`):
                // the head pattern's context closes — no alternative clause
                // falls through.
                self.close_standalone_pattern();
            }
            Ev::CondBegin => {
                // The condition ended: rejoin its open null-shorting
                // before the arm split.
                self.flush_null_shorts_above(self.consumer_base(1));
                let cond = self.pop_value();
                self.condition_operand(&cond);
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
                // The then arm ended: rejoin its open null-shorting before
                // the state swap.
                self.flush_null_shorts_above(self.consumer_base(1));
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
                // The else arm ended: rejoin its open null-shorting before
                // the arm join.
                self.flush_null_shorts_above(self.consumer_base(1));
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
            Ev::NullAwareIndexOperandBegin | Ev::NullAwareMemberBegin => {
                // The `?[`/`?.` selector's split (official
                // `_nullAwareAccess_rightBegin`): the receiver sits on top
                // of the expression stack, the operand's/member's events
                // have not run yet — capture the skip path now so an
                // operand-side `throw`/Never cannot kill it.
                let receiver = self
                    .exprs
                    .last()
                    .map(|e| (e.reference, e.ty, e.args))
                    .unwrap_or((None, Ty::UNKNOWN, crate::ty::EMPTY_ARGS));
                self.null_short_begin(receiver);
            }
            Ev::IndexAccess { null_aware } => {
                let _operand = self.pop_value();
                let mut expr = Expr::trivial(Ty::UNKNOWN);
                if *null_aware {
                    // The condition face keeps the access's own split
                    // (opened at `NullAwareIndexOperandBegin`) — the JOIN
                    // waits for the whole null-shorted expression's
                    // consumer (official `finishNullShorting` at
                    // `analyzeExpression`'s end): `x?[i] = e` shorts the
                    // assignment RHS. The result row carries the chain's
                    // base like `DotAccess`'s.
                    let recv = self.exprs.last();
                    let recv_base = self
                        .null_shorts
                        .last()
                        .map(|short| short.exprs_depth.saturating_sub(1));
                    expr.chain_base = recv_base.map(|b| {
                        recv.and_then(|e| e.chain_base.map(|inner| inner.min(b)))
                            .unwrap_or(b)
                    });
                    expr.branches = self.null_short_branches();
                } else if let Some(inner) = self.exprs.last().and_then(|e| e.chain_base) {
                    expr.chain_base = Some(inner);
                }
                // The receiver pops unless it is the innermost cascade's
                // target — the one receiver a selector must leave in place
                // (the sections chain on it, and `CascadeEnd` truncates to
                // keep it as the value). Every other receiver's facts merged
                // into the access's value above; leaving it stranded would
                // shift every multi-operand consumer's blind pops
                // (`ArgumentsEnd`, `Send`, `CascadeAccess`).
                if !self.receiver_is_cascade_target() {
                    self.exprs.pop();
                }
                self.exprs.push(expr);
            }
            Ev::DotAccess { null_aware, member } => {
                // The member-access node poll (official `checkUnreachableNode`
                // of the invocation node before its target is visited,
                // type_analyzer.dart:2082): a Never receiver's junction exit
                // lands right before the member spelling, so the access is
                // the first dead node — `f().foo();` anchors its row at the
                // member. A reachable state makes this a no-op. A null-aware
                // access whose `?.` split was live at open (NullShort
                // `dead_at_open` false) died strictly inside the member's own
                // tail (`x?.m(throw _)`) — post-poll timing the official
                // never reports, so the row would mis-anchor. The same gate
                // covers a NON-aware selector inside an open shorted tail
                // (`x?.a.b(throw _)`'s `.b`) — the innermost open short owns
                // this region and its skip path rejoins the continuation.
                if self
                    .null_shorts
                    .last()
                    .is_some_and(|short| !short.dead_at_open)
                {
                    // Interior death — the skip path rejoins at the chain's
                    // boundary and carries the continuation's reachability.
                } else {
                    self.dead_code(*member);
                }
                // The member identifier's own entry sits on top (every
                // member shape pushes through its primary parse); the
                // receiver stays BENEATH it — consumed by the enclosing
                // statement's pop, or kept for the cascade's
                // `CascadeEnd` truncate (the cascade sections route
                // through this event with the target under the section
                // frame's base). The entry is peeked, never popped.
                let member_value = self.pop_value();
                let mut expr = Expr::trivial(Ty::UNKNOWN);
                // The property-read face (#1409): a plain-identifier
                // member on a promotable receiver (a bare local read or
                // `this` — official `ExpressionPropertyTarget`'s reference
                // version and `ThisPropertyTarget`'s `_thisVersion`,
                // `flow_analysis.dart:182-198`, :5977-5990) resolves to a
                // property slot. Eligibility (a private final field, the
                // official `isPropertyPromotable` verdict) is the
                // semantic layer's question — the flow layer tracks every
                // promotable-shaped read and the consumer gates the rows.
                if let (Some(field), Some(receiver)) = (
                    member_value.name,
                    self.exprs.last().and_then(|e| e.prop_receiver()),
                ) {
                    let slot = self.prop_slot(receiver, field);
                    let promoted = self.prop_promoted(receiver, slot);
                    // The promoted-read row at the member name's own
                    // token — the anchor every staged check on the
                    // member read keys on. The event's `member` field
                    // cannot serve: the parser hands the send's tiled
                    // end there (the bare leaf's follower token, the
                    // #585 contract), not the member identifier. Dead
                    // reads carry no row (their region is already the
                    // dead-code face).
                    let member_span = member_value.read_span.unwrap_or(*member);
                    if promoted && self.reachable() {
                        self.emit(FlowFact::PromotedPropertyRead { span: member_span });
                    } else if self.reachable() {
                        // The non-promoted mirror (upstream
                        // `_getNonPromotionReasons`'s property arm): the
                        // slot's cleared promoted types stand as evidence;
                        // a member never tested or promoted carries no
                        // history and emits no row (the official
                        // `allPreviouslyPromotedTypes != null` gate).
                        let history = self.current.prop(slot).history;
                        if history != crate::model::NIL {
                            let entries = self.intern_non_promotion(None, history);
                            // The receiver identity: `this` per member
                            // name, or the local's declaration anchor —
                            // the consumer's identity key. A non-`this`
                            // receiver is always a body-declared local,
                            // and every declaration carries its anchor.
                            let receiver = if receiver == THIS_RECEIVER {
                                PropReceiver::This
                            } else {
                                PropReceiver::Local(
                                    self.vars[receiver as usize]
                                        .decl_span
                                        .expect("receiver local carries its declaration span"),
                                )
                            };
                            self.emit(FlowFact::NonPromotedPropertyRead {
                                span: member_span,
                                name: field,
                                receiver,
                                entries,
                            });
                        }
                    }
                    // The read's flow type at this layer: the marker pair
                    // (unpromoted = potentially-nullable unknown,
                    // promoted = its non-null face). Decision-equivalent
                    // to `UNKNOWN` in every classification question, and
                    // never emitted into a fact row.
                    expr.ty = if promoted {
                        Ty::UNKNOWN
                    } else {
                        Ty::UNKNOWN_NULLABLE
                    };
                    expr.prop = Some(slot);
                }
                if *null_aware {
                    // The `?.`'s split ran at `Ev::NullAwareMemberBegin` (the
                    // member's own send — `x?.m(args)` — completes before this
                    // event, so the `rightBegin` capture must live on the
                    // earlier marker). The branches read the pair here — the
                    // JOIN waits for the whole null-shorted expression's
                    // consumer (official `finishNullShorting` at
                    // `analyzeExpression`'s end, null_shorting.dart:36-56):
                    // `x?.f = e` shorts the assignment RHS and `x?.a.b(c)`
                    // shorts the trailing sends, so an operand-side `throw`
                    // must not revive-then-kill the merged state. The result
                    // row remembers the chain's base so that consumer can
                    // find every short the chain opened (`x?.y?.z`).
                    let recv = self.exprs.last();
                    let recv_base = self
                        .null_shorts
                        .last()
                        .map(|short| short.exprs_depth.saturating_sub(1));
                    expr.chain_base = recv_base.map(|b| {
                        recv.and_then(|e| e.chain_base.map(|inner| inner.min(b)))
                            .unwrap_or(b)
                    });
                    expr.branches = self.null_short_branches();
                } else if let Some(inner) = self.exprs.last().and_then(|e| e.chain_base) {
                    // A non-aware selector chained onto a null-shorted
                    // receiver (`x?.y.z`) is inside the same shorted region
                    // — propagate the base so the consumer flushes the whole
                    // chain.
                    expr.chain_base = Some(inner);
                }
                // The receiver pops unless it is the innermost cascade's
                // target (same discipline as IndexAccess's).
                if !self.receiver_is_cascade_target() {
                    self.exprs.pop();
                }
                self.exprs.push(expr);
            }
            ev @ (Ev::Send { .. } | Ev::SendNoArgs) => {
                // The callee flows through as the send's result: a bare
                // identifier keeps its variable reference and promoted type
                // (a plain assignment's target is exactly such a reference).
                if let Some(mut callee) = self.exprs.pop() {
                    // An invoked bare head that resolved to nothing: the
                    // head's spelling decides the face. A class-head
                    // spelling (`C()`/`C<T>()` without `new`/`const`) is
                    // the implicit-creation face — the call's static type
                    // is the named class, so `x = C()` can promote `x` to
                    // `C`. A lowercase head is a function/getter call
                    // (`foo(x)`, a tear-off's neighbor): the call's static
                    // type is the callee's return, unknown at this layer —
                    // typing it as the head's name (the pre-#2174 face)
                    // wrote garbage into every `x = foo()` and demoted
                    // what the target had promoted. The gate: the send
                    // actually carried an argument list (the preceding
                    // `ArgumentsEnd`; a bare leaf fires `Send` too, and a
                    // tear-off names the callable, not the class), and
                    // the callee is the unresolved bare-head expression
                    // (a `prop` slot marks it — member callees carry
                    // none, resolved callees keep their own type). The
                    // send's written argument tree rides beside the
                    // class face (#2243): `x ??= C<int>()` promotes
                    // through the instantiated spelling.
                    if matches!(ev, Ev::Send { .. })
                        && matches!(events.get(i.wrapping_sub(1)), Some(Ev::ArgumentsEnd { .. }))
                        && callee.reference.is_none()
                        && callee.prop.is_some()
                        && let Some(name) = callee.name
                    {
                        let Ev::Send { args } = ev else {
                            unreachable!("gated on Ev::Send above")
                        };
                        if self.is_class_head(name) {
                            callee.ty = Ty {
                                base: Base::Name(name),
                                nullable: false,
                            };
                            callee.args = *args;
                        } else {
                            callee.ty = Ty::UNKNOWN;
                        }
                        callee.prop = None;
                    }
                    self.exprs.push(callee);
                }
            }
            Ev::ArgumentsBegin | Ev::ParenBegin | Ev::AwaitBegin { .. } => {}
            Ev::ParenEnd => {
                // The parenthesized operand ended: rejoin the null-shorting
                // it opened (`(x?.y)` — the chain's boundary is the `)`,
                // not the enclosing selector list).
                self.flush_null_shorts_above(self.consumer_base(1));
            }
            Ev::ArgumentsEnd { count } => {
                // The arguments are complete expressions: rejoin the
                // null-shorting they opened — but not a caller-side `?`
                // context (`f()?.m(g()?.n)`'s outer short opened before
                // `ArgumentsBegin` and stays open to the chain's end;
                // official `nullShortingTargetDepth` bound).
                self.flush_null_shorts_above(self.consumer_base(*count));
                // The dart:core `identical(a, b)` face: the official
                // resolver routes the invocation through
                // `equalityOperation_end` exactly like a binary `==`
                // (upstream Dart SDK source,
                // pkg/analyzer/lib/src/dart/resolver/
                // invocation_inferrer.dart:556-577 — the `_isIdentical`
                // gate is the resolved dart:core top-level function with
                // exactly two arguments; the shared fold lives in
                // `equalityOperation_end`, flow_analysis.dart:6806-6852),
                // so the same verdicts apply: the disjoint-by-nullability
                // check folds the call constant-false and deads the true
                // branch (`identical(v, null)` on a non-nullable `v` —
                // html `dom.dart:446`), and the null-literal face
                // promotes. The bare-head spelling match is exact for
                // bare calls (a same-library declaration lexically
                // shadows the import and keeps both branches — probed
                // 3.13.1); a prefixed or member send never matches the
                // bare-head shape and stays under-report live.
                if *count == 2 && self.send_callee_is_core_identical() {
                    let right = self.pop_value();
                    let left = self.pop_value();
                    self.equality_check(&left, &right, false);
                    // The fold's result is the send's value: drop the
                    // callee head so the following `Send` finds the
                    // equality expression on top and re-pushes it as the
                    // call's own result.
                    let folded = self.pop_value();
                    self.exprs.pop();
                    self.exprs.push(folded);
                    return true;
                }
                for _ in 0..*count {
                    self.exprs.pop();
                }
            }
            Ev::Throw => {
                // The thrown operand ends: rejoin the null-shorting opened
                // inside it (`throw x?.y`) — never the chain that CONTAINS
                // the throw (`x?[throw '']`'s own `?` stays open to the
                // index access's end).
                self.flush_null_shorts_above(self.consumer_base(1));
                let _ = self.exprs.pop();
                // The throw expression itself still produces a value slot
                // (static type `Never`, resolver_visitor.dart
                // `visitThrowExpression`): the consumer's operand pop must
                // eat this slot, not the producer below it.
                self.exprs.push(Expr::trivial(Ty::NEVER));
                self.unreachable_cur();
            }
            Ev::Exit { end } => {
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
                // The exit's position feeds the statement wrapper's anchor
                // decision ([`Interp::wrapper_dead_check`]): the recorder
                // can drain the exit past its own statement's `StmtEnd`
                // (whose snapshot refresh still reads a live state), so the
                // NEXT statement's wrapper compares the exit position with
                // its own begin instead. Gated off collect runs like the
                // snapshot refresh.
                if !self.collecting() {
                    self.exit_at = Some(end.range.offset);
                }
            }
            Ev::Rethrow => {
                self.unreachable_cur();
            }
            Ev::RecordLitEnd { count } | Ev::ListLitEnd { count } => {
                // The literal's elements are complete expressions: rejoin
                // the null-shorting they opened (the receiver-chain `?`
                // outside the literal stays open to the enclosing
                // expression's end).
                self.flush_null_shorts_above(self.consumer_base(*count));
                self.literal_value(*count)
            }
            Ev::MapEntryEnd => {
                // The entry's key and value are complete expressions:
                // rejoin the null-shorting they opened.
                self.flush_null_shorts_above(self.consumer_base(2));
                // The entry's value is the value's own stack entry; the
                // key's entry drops.
                let value = self.exprs.pop();
                self.exprs.pop();
                if let Some(value) = value {
                    self.exprs.push(value);
                }
            }
            Ev::CascadeBegin { null_aware } => {
                if *null_aware {
                    // `?..`: official `cascadeExpression_afterTarget` runs
                    // `_nullAwareAccess_rightBegin` before the first
                    // section — the target sits on top of the expression
                    // stack and every section runs on its not-null path.
                    let receiver = self
                        .exprs
                        .last()
                        .map(|e| (e.reference, e.ty, e.args))
                        .unwrap_or((None, Ty::UNKNOWN, crate::ty::EMPTY_ARGS));
                    self.null_short_begin(receiver);
                }
                self.stack.push(Frame::Cascade {
                    exprs_base: self.exprs.len(),
                    // Count AFTER the `?..` push: section-end flushes keep
                    // the head's short open (`null_shorts.len() > base`
                    // pops only what a SECTION opened); `CascadeEnd` joins
                    // it via `own_short`.
                    null_shorts_base: self.null_shorts.len(),
                    own_short: *null_aware,
                });
            }
            Ev::CascadeAccess { .. } => {
                // The section's value is consumed by the cascade; any
                // null-shorting the SECTION opened (`x..a?.b`) ends with
                // it — the cascade head's own `?..` context (below
                // `null_shorts_base`) stays open to `CascadeEnd`.
                let base = match self.stack.last() {
                    Some(Frame::Cascade {
                        null_shorts_base, ..
                    }) => *null_shorts_base,
                    _ => 0,
                };
                while self.null_shorts.len() > base {
                    self.null_short_end();
                }
                self.exprs.pop();
            }
            Ev::CascadeEnd => {
                let (base, owns_short) = match self.stack.last() {
                    Some(Frame::Cascade {
                        null_shorts_base,
                        own_short,
                        ..
                    }) => (*null_shorts_base, *own_short),
                    _ => (self.null_shorts.len(), false),
                };
                match self.stack.pop() {
                    Some(Frame::Cascade { exprs_base, .. }) => {
                        // The head `?..` context's skip path rejoins here —
                        // official `nullAwareAccess_end` after the last
                        // section (type_analyzer.dart:2358-2360), so a
                        // section-side `throw`/Never (`x?..[throw '']`)
                        // cannot kill the receiver-was-null path.
                        let end_base = base - usize::from(owns_short);
                        while self.null_shorts.len() > end_base {
                            self.null_short_end();
                        }
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
                // Each interpolation was a complete expression: rejoin the
                // null-shorting they opened.
                self.flush_null_shorts_above(self.consumer_base(*interpolation_count));
                for _ in 0..*interpolation_count {
                    self.exprs.pop();
                }
                self.exprs.push(Expr::trivial(Ty::UNKNOWN));
            }
            Ev::Spread | Ev::NullAwareElement => {
                // The operand's value is consumed; the element leaves its
                // own (trivial) value — every literal element nets exactly
                // one.
                self.flush_null_shorts_above(self.consumer_base(1));
                self.exprs.pop();
                self.exprs.push(Expr::trivial(Ty::UNKNOWN));
            }
            Ev::IfControlFlowBegin => {
                // Collection if-element begin: the parser fires it before
                // the condition streams (the element step opens the `if`
                // at the keyword), so the split waits for the condition's
                // own `ThenControlFlow`.
            }
            Ev::ThenControlFlow => {
                // The collection-if condition ended: rejoin its open
                // null-shorting before the split.
                self.flush_null_shorts_above(self.consumer_base(1));
                let cond = self.pop_value();
                // Collection if-element condition (official
                // `handle_ifElement_conditionEnd`, type_analyzer.dart:1161).
                self.condition_operand(&cond);
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
                self.flush_null_shorts_above(self.consumer_base(1));
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

    /// Rejoins a dropped condition-valued expression's split: the
    /// is-check/equality folds narrow `self.current` to the true arm for
    /// the condition machinery (guards read the drift — `GuardBegin`'s
    /// contract), but a value context that consumes the result as a plain
    /// value (`final b = a != null;`, `x is T;`, `v = a is int;`) leaves
    /// the official state recombined — both arms reachable. Without the
    /// rejoin the narrowed state poisoned later reads (a following
    /// `a ?? g()` saw a promoted `a` and deaded `g()` — dio
    /// `options.dart:629`).
    pub(crate) fn rejoin_value_split(&mut self, expr: &Expr) {
        if expr.test_split
            && let Some((if_true, if_false)) = &expr.branches
        {
            self.current = self.join(Some(if_true.clone()), Some(if_false.clone()));
        }
    }

    /// Whether the expression stack's top entry is the innermost active
    /// cascade's target — the one receiver a selector must leave in place
    /// (the sections chain on it, and `CascadeEnd` truncates to keep it as
    /// the value). Positional, not identity-based: the target is the entry
    /// the cascade frame was opened over, so every other receiver — however
    /// deeply nested in the section's arguments — sits strictly above the
    /// base and pops.
    pub(crate) fn receiver_is_cascade_target(&self) -> bool {
        match self.stack.last() {
            Some(Frame::Cascade { exprs_base, .. }) => *exprs_base == self.exprs.len(),
            _ => false,
        }
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
        if let Some(head) = self.exprs.last_mut() {
            head.literal = true;
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
    /// flow_analysis.dart:4137-4151): the tested type itself when it is a
    /// step over the current type — the official's `tryPromoteToType`
    /// gate (`tested <: current`, type_system.dart:1739-1741) answered
    /// over the instantiated spellings, previous chain head's written
    /// arguments included. A tested type the current type already
    /// subsumes (`x is int?` on `int x`) is no step; a same-base test
    /// whose written arguments cannot sit under the declared ones
    /// (`x is List<num>` on `List<int> x`) refuses at the name level
    /// already. The undecidable cross-base question keeps the
    /// name-level optimistic answer (`num n`, `is int` — the oracle
    /// promotes; [`crate::ty::TypeOps::try_promote_to_typed`]'s default)
    /// and a resolved back-end re-tightens to the real hierarchy.
    fn promotion_candidate(&mut self, var: u32, tested: Ty, tested_args: u32) -> Option<Ty> {
        match tested.base {
            Base::Unknown | Base::Dynamic | Base::Void => None,
            _ => {
                let (previous, previous_args) = {
                    let model = self.current.model(var);
                    (
                        self.promoted_type_of(var, &model),
                        self.promoted_args_of(var, &model),
                    )
                };
                self.ops.try_promote_to_typed(
                    tested,
                    tested_args,
                    previous,
                    previous_args,
                    self.args,
                    self.names,
                )
            }
        }
    }

    /// Routes a declared type to its consumer: the open variable
    /// declaration's type slot (from `VarDeclBegin` until its first
    /// identifier seals it — #1037), the pending formal, the open
    /// `is`-test, or the open `as`-cast. The written-argument id rides
    /// beside the type (#611 step 4) — every type row passes through
    /// here, so each sink sees the pair as one fact.
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

    /// Whether an operand's type test is guaranteed to
    /// fail (official `isExpression_end`'s first arm — upstream Dart
    /// SDK source, pkg/_fe_analyzer_shared/lib/src/flow_analysis/
    /// flow_analysis.dart:7184-7189 — `isBottomType(checkedType)` or
    /// `_isTypeCheckGuaranteedToFailWithSoundNullSafety`
    /// (:9133-9140) over `_typesAreDisjointDueToNullability`
    /// (:9496-9501), folded to `booleanLiteral(isNot)`, decided before
    /// the reference branch and the guaranteed-succeed arm — it covers
    /// promotable references too, fed the declared static type): a
    /// bottom tested type (`Never`) or a nullability-disjoint pair
    /// — one side `Null`-or-equivalent (`Null`, `Never?`), the other
    /// non-nullable — makes the test a constant `false`. The same
    /// refusal trio as
    /// [`Interp::is_test_guaranteed`] applies: an unknown/dynamic/void
    /// face on either side is never a dead-branch producer (the
    /// name-level lattice's optimism must not certify the mark).
    fn is_test_guaranteed_fail(&self, operand_ty: Ty, tested: Ty) -> bool {
        if matches!(
            operand_ty.base,
            Base::Unknown | Base::Dynamic | Base::Void
        ) || matches!(tested.base, Base::Unknown | Base::Dynamic | Base::Void)
        {
            return false;
        }
        self.ops.is_bottom(tested) || self.disjoint_by_nullability(operand_ty, tested)
    }

    /// Whether a non-reference operand's type test is guaranteed to
    /// succeed (official `_isTypeCheckGuaranteedToSucceedWithSoundNullSafety`
    /// — upstream Dart SDK source,
    /// pkg/_fe_analyzer_shared/lib/src/flow_analysis/flow_analysis.dart:9149-9155
    /// — `isSubtypeOf(staticType, checkedType)` under sound flow). The
    /// answer deads a branch, so the boundary refuses: a dynamic tested
    /// spelling or an unknown/void operand face is never a dead-branch
    /// producer (the name-level lattice's top-type optimism would
    /// otherwise certify the mark — the same refusal trio
    /// [`Interp::promotion_candidate`] applies). The named faces go
    /// through [`TypeOps::is_subtype_typed`]; a resolved back-end
    /// re-decides them through the real hierarchy.
    fn is_test_guaranteed(
        &self,
        operand_ty: Ty,
        operand_args: u32,
        tested: Ty,
        tested_args: u32,
    ) -> bool {
        if matches!(
            operand_ty.base,
            Base::Unknown | Base::Dynamic | Base::Void
        ) || matches!(tested.base, Base::Unknown | Base::Dynamic | Base::Void)
        {
            return false;
        }
        self.ops.is_subtype_typed(
            operand_ty,
            operand_args,
            tested,
            tested_args,
            self.args,
            self.names,
        )
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
    /// `experiments/flags.dart:309-315`; oracle-probed: a bare-declared
    /// `int x` failing `x is int` deads the else branch too — the
    /// subtype-always-true test fires the mark on its own, no promotion
    /// chain required).
    pub(crate) fn branch_type_test(
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
            self.args,
            self.names,
        );
        // The unmatched remainder carries the operand spellings' written
        // arguments: `factor(Response, Response<T>)` on bare `Ty`s compares
        // equal and deads the else branch; the instantiated forms keep it
        // live (factor_type.dart's instantiated-type arm, #2186 S4-C).
        let factored = self.ops.factor_typed(
            previous,
            previous_args,
            tested,
            tested_args,
            self.args,
            self.names,
        );
        let unmatched = if self.ops.is_bottom(factored) {
            // The bottom-remainder arm is dead — `response is! T` on a
            // `response` already proved `T` leaves nothing below. The
            // official `_finishTypeTest` keeps `promotedTypes` untouched
            // (typeIfFalse is null, flow_analysis.dart:4160-4163), so the
            // chain still reads the pre-test promotion. The name-level
            // deliberate-divergence boundary (#2204 C) wants the dead
            // remainder's member reads to face the *declared* type — the
            // receiver row's `read_type_at` override would otherwise
            // resolve the promoted head's member and silence the miss
            // (the pin `is_not_promotion_keeps_the_remainder_under_generic_
            // tested`: `response.data` in the dead else reports
            // `undefined_getter` on `Object`, not `Response`). Clearing
            // just this var's `promoted` chain on the dead arm is enough —
            // `promoted_type_of` falls back to `declared` and the
            // downstream member walk sees the un-promoted receiver.
            let mut dead = self.current.clone().set_unreachable(&mut self.reach);
            let model = dead.model(var);
            if model.promoted != crate::model::NIL {
                dead = dead.with_model(
                    var,
                    crate::model::VarModel {
                        promoted: crate::model::NIL,
                        ..model
                    },
                );
            }
            dead
        } else {
            // Official `isValidPromotionStep`: the factor output is a
            // subtype of `previous` by construction, so the step is valid
            // exactly when `previous` is not a subtype of it (no mutual
            // subtypes, no repeats). The instantiated face decides —
            // `factor` keeps `previous`'s written-argument tree, which the
            // typed compare reads.
            let factored = if self.ops.is_subtype_typed(
                previous,
                previous_args,
                factored,
                previous_args,
                self.args,
                self.names,
            ) {
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
                self.args,
                self.names,
            )
        };
        (matched, unmatched)
    }

    /// The official `_equalityCheck` at the name level, in its decision
    /// order (upstream Dart SDK `flow_analysis`): (1) both operand types
    /// classify null-or-equivalent — a known equal result; (2)
    /// null-or-equivalent vs non-nullable — a known not-equal result;
    /// (3) one operand is a null *literal* — a null check promoting the
    /// other side's reference. A `Null`-typed variable read is a
    /// reference, not a literal, so it carries no null info and promotes
    /// nothing; any other shape leaves both branch sides reachable.
    /// Classification runs through the boundary-aware
    /// [`Interp::classify`]: a `?`-less type-parameter reference whose
    /// declared bound is nullable stays potentially nullable, so its
    /// null checks reach the promotion arm instead of the disjoint arm
    /// (official `classifyType`, flow_analysis_visitor.dart:615-632).
    fn equality_check(&mut self, left: &Expr, right: &Expr, inverted: bool) {
        let mut expr = Expr::trivial(Ty::UNKNOWN);
        // Official `_isNullType`: `classifyType` → nullOrEquivalent —
        // `Null` and equivalents such as `Never?`. A type question, not a
        // literal question.
        let null_ty = |t: Ty| self.classify(t) == Classification::NullOrEquivalent;
        // Official `ExpressionInfo.isNull` fires for the null literal
        // only; in this pipeline the literal is the sole null-typed
        // expression without a promotable reference.
        let null_literal = |e: &Expr| e.ty.base == Base::Null && e.reference.is_none();
        let (left_null, right_null) = (null_ty(left.ty), null_ty(right.ty));
        if left_null && right_null {
            // Known equal: `null == null` kills the else-branch, `null
            // != null` the then-branch.
            expr.bool_value = Some(!inverted);
            expr.branches = None;
        } else if (left_null && self.classify(right.ty) == Classification::NonNullable)
            || (right_null && self.classify(left.ty) == Classification::NonNullable)
        {
            // Disjoint by nullability: known not-equal — `==` kills the
            // then-branch, `!=` the else-branch.
            expr.bool_value = Some(inverted);
            expr.branches = None;
        } else {
            let operand = if null_literal(right) {
                Some(left)
            } else if null_literal(left) {
                Some(right)
            } else {
                None
            };
            // The variable-check face (a promotable reference operand).
            if let Some(var) = operand.and_then(|e| e.reference) {
                let operand = operand.expect("reference implies operand");
                // The marker step lives in [`Interp::promote_to_non_null`]
                // (the opaque-declared face's non-null form is the plain
                // unknown, #2243/#2241).
                let (promoted, promoted_args) =
                    self.promote_to_non_null(operand.ty, operand.args);
                if promoted == operand.ty {
                    // Official `tryMarkNonNullable` bails on
                    // `isSameType` (flow_analysis.dart:4059-4068): the
                    // non-null face is the type itself, so no promotion
                    // stands and both sides stay reachable.
                    let cur = self.current.clone();
                    expr.branches = Some((cur.clone(), cur));
                } else {
                    let matched = self.current.clone().finish_type_test(
                        self.ops,
                        &mut self.lists,
                        var,
                        None,
                        Some((promoted, promoted_args)),
                        self.args,
                        self.names,
                    );
                    // `x == null`: the true-branch saw null; the
                    // false-branch saw non-null. `!=` inverts. `current`
                    // narrows for the guard machinery's contract (see the
                    // `IsEnd` arm); a value consumer rejoins both arms
                    // ([`Interp::rejoin_value_split`]).
                    let (eq_true, eq_false) = (self.current.clone(), matched);
                    let (if_true, if_false) = if inverted {
                        (eq_false, eq_true)
                    } else {
                        (eq_true, eq_false)
                    };
                    self.current = if_true.clone();
                    expr.branches = Some((if_true, if_false));
                    expr.test_split = true;
                }
            }
            // The property-check face (#1409): a null check on a
            // property read whose receiver is promotable (official
            // `_equalityCheck`'s `_EqualityCheckIsNullCheck` →
            // `tryMarkNonNullable`, `flow_analysis.dart:4046-4076` — the
            // property's value version is the promotion target; the
            // receiver identity rides in the slot table). The bail
            // faces: a captured receiver or slot (the official
            // `info.writeCaptured` trivial arm), or a promotion already
            // standing (`isValidPromotionStep`'s isSameType arm) — both
            // leave the two branches reachable.
            else if let Some(slot) = operand.and_then(|e| e.prop) {
                let receiver = self.props[slot as usize].0;
                let model = self.current.prop(slot);
                if self.prop_target_captured(receiver)
                    || model.captured()
                    || model.promoted != crate::model::NIL
                {
                    let cur = self.current.clone();
                    expr.branches = Some((cur.clone(), cur));
                } else {
                    // The marker chain: the non-null unknown face. The
                    // unpromoted marker is potentially-nullable unknown,
                    // so the step is always a refinement here.
                    let matched = self.current.clone().promote_prop_non_null(
                        self.ops,
                        &mut self.lists,
                        slot,
                        Ty::UNKNOWN,
                        self.args,
                        self.names,
                    );
                    let (eq_true, eq_false) = (self.current.clone(), matched);
                    let (if_true, if_false) = if inverted {
                        (eq_false, eq_true)
                    } else {
                        (eq_true, eq_false)
                    };
                    self.current = if_true.clone();
                    expr.branches = Some((if_true, if_false));
                    expr.test_split = true;
                }
            } else {
                // No null information: both sides reachable, no promotion.
                let cur = self.current.clone();
                expr.branches = Some((cur.clone(), cur));
            }
        }
        self.exprs.push(expr);
    }
}
