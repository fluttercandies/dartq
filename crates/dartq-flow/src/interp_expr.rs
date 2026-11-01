//! Expression events: references, tests, boolean/null-aware branching,
//! assignments, and collection elements.

use dartq_parser::TokenSpan;
use dartq_source::SourceRange;

use crate::ev::{BinOp, Ev};
use crate::facts::{FlowFact, PropReceiver};
pub(crate) use crate::interp::Expr;
use crate::interp::{CollIfFrame, Frame, Interp, Resolved, THIS_RECEIVER};
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
                            // The header-part leaf's dead poll, twin to
                            // `Ev::Int`'s (`for (;; 0, throw 1, i++)`'s
                            // `i` — the updater leaf dying mid-list is the
                            // official's first dead node).
                            if self.for_header_depth > self.header_cond_depth {
                                self.dead_code(*token);
                            }
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
                                // A nullability-only promotion standing past a
                                // write our tables could not type (`v = q.m()`
                                // over a receiver only the flow promotes) may
                                // be the oracle's demotion: the opaque row
                                // keeps the non-null consumers silent without
                                // certifying non-null to the null-aware faces.
                                let opaque = self.unresolved_writes.contains(&var)
                                    && declared.nullable
                                    && ty == declared.non_null();
                                self.emit(FlowFact::PromotedRead {
                                    span: *token,
                                    ty: if opaque { Ty::UNKNOWN } else { ty },
                                    args: if opaque { crate::ty::EMPTY_ARGS } else { args },
                                });
                            } else if self.unresolved_writes.contains(&var)
                                && declared.nullable
                                && matches!(declared.base, crate::ty::Base::Name(_))
                            {
                                // A write our tables could not type: the
                                // oracle's write-promotion stands beyond our
                                // sight — the opaque promotion row lets the
                                // staged consumers take the non-null form
                                // (the #2174 channel).
                                self.emit(FlowFact::PromotedRead {
                                    span: *token,
                                    ty: Ty::UNKNOWN,
                                    args: crate::ty::EMPTY_ARGS,
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
                            //
                            // An unpromoted read of an implicitly typed
                            // FORMAL publishes `dynamic`, not the lattice's
                            // suppress face: the formal's element type is
                            // `dynamicType` upstream (upstream Dart SDK
                            // source, pkg/analyzer/lib/src/dart/resolver/
                            // resolution_visitor.dart:799-814), so the
                            // static type at such a read is a KNOWN
                            // `dynamic` — a dynamic receiver the dynamic-call
                            // faces report — never "we don't know". The
                            // lattice slot keeps `Unknown` (no promotion,
                            // reachability or nullability verdict moves).
                            let query_ty = if ty == Ty::UNKNOWN
                                && self
                                    .vars
                                    .get(var as usize)
                                    .is_some_and(|meta| meta.implicit_dynamic)
                            {
                                Ty::DYNAMIC
                            } else {
                                ty
                            };
                            self.emit(FlowFact::ReadType {
                                span: *token,
                                ty: query_ty,
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
                            // The proven-constant face (official
                            // `_getConstantValue` on a `final`/`const`
                            // local's read): `final b = true;` makes `b`'s
                            // later condition reads fold exactly like the
                            // literal — `if (b) { x; } else { y; }` rows
                            // `y;`. Set at `InitializerEnd` when the
                            // declaration was final and the fold carried a
                            // `bool_value`; cleared by any write.
                            expr.bool_value = self
                                .vars
                                .get(var as usize)
                                .and_then(|meta| meta.const_bool);
                            // The by-construction non-null face: an
                            // implicitly typed local whose initializer can
                            // never be null (`final x = 0;`) reads as
                            // non-null like a written non-nullable
                            // annotation, so `x ?? 1` kills the right
                            // operand and `x?.m` kills the skip path. The
                            // slot itself stays the opaque unknown face
                            // (the #2243 guard), which is why the verdict
                            // has to ride the expression.
                            expr.non_null_literal = self
                                .vars
                                .get(var as usize)
                                .is_some_and(|meta| meta.non_null);
                            self.exprs.push(expr);
                            // The bottom-typed read's own exit (official
                            // `recordStaticType`'s `handleExit` on
                            // `type.isBottom`, ast.dart:11146-11153): a
                            // `Never a` parameter read ends the block's
                            // flow at the identifier, and the enclosing
                            // operand's close reports the trailing
                            // dead region — `f(Never a, int b) { a + b; }`
                            // rows `+ b`. The continuation sits on the
                            // stack for the consumer like `Ev::Exit`'s.
                            if self.ops.is_bottom(ty) {
                                self.unreachable_cur();
                                if !self.collecting() {
                                    self.exit_at = Some(token.range.end());
                                }
                            }
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
                            expr.ret = top.ret;
                            expr.read_span = Some(*token);
                            expr.name = Some(*name);
                            // No constant feed: the official never folds a
                            // named `const`/`final` element's value into
                            // flow analysis — `booleanLiteral` fires on the
                            // literal node alone, so `if (DEBUG)` over
                            // `const bool DEBUG = false` keeps both arms
                            // live (dead_code_test.dart's debugConst family
                            // expects silence).
                            self.exprs.push(expr);
                            // The bottom-typed read's exit — the
                            // `Resolved::Local` arm's face applied to the
                            // unit's own top-level `Never` declaration.
                            if self.ops.is_bottom(top.ty) {
                                self.unreachable_cur();
                                if !self.collecting() {
                                    self.exit_at = Some(token.range.end());
                                }
                            }
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
                                self.emit_prop_read_type(slot, *token);
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
                            // The implicit-this member's declared face
                            // (the #2292 sibling): the semantic layer
                            // staged the field/getter's type at this
                            // offset — the bare read's own answer when no
                            // property promotion stands (a promotion keeps
                            // its non-null unknown face; the staged row is
                            // the declared one).
                            if !promoted
                                && let Some(&staged) =
                                    self.staged_this_members.get(&token.range.offset)
                            {
                                expr.ty = staged;
                            }
                            // The implicit-`this` invalid face: inside an
                            // extension whose written `on` type resolves
                            // nowhere (the recorder staged it at this
                            // token), the member read's static type is the
                            // official's `InvalidType` — spelled as the
                            // on-type's own name so the resolved back-end's
                            // existing unresolved-spelling map answers it
                            // (subtype.dart's 0c arm makes every relation
                            // with it vacuous: a switch over the read
                            // covers every typed pattern, and an `is`-test
                            // on the local it feeds always succeeds).
                            if !promoted
                                && let Some(this_ty) = self.staged_this.get(&token.range.offset)
                                && self.ops.is_invalid_typed(
                                    *this_ty,
                                    crate::ty::EMPTY_ARGS,
                                    self.args,
                                    self.names,
                                )
                            {
                                expr.ty = *this_ty;
                            }
                            // The undefined bare read (outside any `on`
                            // context): a spelling the resolved back-end
                            // certifies resolves nowhere — no container,
                            // top-level declaration, or member anywhere —
                            // is the official's `InvalidType` (upstream
                            // `_resolveUnqualifiedNameRead` records it for
                            // an undefined identifier), spelled as the
                            // name itself like the unresolved-call face. A
                            // class-head or type-parameter spelling is a
                            // `Type` literal read, never this face.
                            else if !promoted
                                && expr.ty == Ty::UNKNOWN_NULLABLE
                                && !self.local_fns.contains(name)
                                && !self.is_class_head(*name)
                                && self
                                    .boundaries
                                    .get(*name as usize)
                                    .is_none_or(Option::is_none)
                                && self.ops.call_head_unresolved(*name, self.names)
                            {
                                expr.ty = Ty {
                                    base: Base::Name(*name),
                                    nullable: false,
                                };
                                expr.non_reference_invalid = true;
                            }
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
                self.sink_type(ty, *args, true);
            }
            Ev::NoType => self.sink_type(Ty::UNKNOWN, crate::ty::EMPTY_ARGS, false),
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
                true,
            ),
            Ev::VoidType => self.sink_type(
                Ty {
                    base: crate::ty::Base::Void,
                    nullable: false,
                },
                crate::ty::EMPTY_ARGS,
                true,
            ),
            Ev::Bool { value, span } => {
                // The header-part leaf's dead poll — the same
                // `checkUnreachableNode` face [`Ev::Int`] carries for the
                // init/updater slices (`for (;; true, throw 1, false)`'s
                // trailing `false`); see that arm for the official basis.
                // Outside a header the poll also fires to OPEN the interval
                // (`dead.is_none()`): a literal element reached dead inside
                // a collection is the official's first dead node —
                // `[1, 2, f(), 4]` on `Never f()` anchors `4`. An open
                // interval keeps ownership (later nodes extend), and the
                // wrapper's re-anchor lifts statement-level anchors.
                if self.for_header_depth > self.header_cond_depth || self.dead.is_none() {
                    self.dead_code(*span);
                }
                // The literal's static type is `bool` (official
                // `literalBool`'s recorded type — the `matchedType` an
                // initializer or assignment write promotes through:
                // `Object? o = true` promotes `o` to `Object` because
                // `bool <: Object`, flow_analysis.dart:3982-4098's
                // `_tryPromoteToTypeOfInterest`). Spelling it lets the
                // resolved seam answer the subtype question the opaque
                // unknown face never could; without the interned id (a
                // direct-constructed interp) the face stays unknown — the
                // under-report posture, unchanged.
                let ty = self
                    .closed
                    .bool_name
                    .map(|id| Ty {
                        base: crate::ty::Base::Name(id),
                        nullable: false,
                    })
                    .unwrap_or(Ty::UNKNOWN);
                let mut expr = Expr::trivial(ty);
                expr.bool_value = Some(*value);
                // A boolean literal can never evaluate to null — the same
                // guaranteed-non-null face `type.isNonNullable` gives the
                // `??`/`?.` kills.
                expr.non_null_literal = true;
                self.exprs.push(expr);
            }
            Ev::Int { span } => {
                // The header-part leaf's dead poll (official
                // `checkUnreachableNode` fires through `analyzeExpression`
                // — the init/updater slices are exactly the header's own
                // `analyzeExpression` calls, so a leaf that DIES inside a
                // live-entered header (`for (;; 0, throw 1, 2)`'s `2`) is
                // the official's first dead node). Dead-at-entry headers
                // fold through [`Interp::arm_dead_depth`]; an open interval
                // keeps ownership — the poll folds into the enclosing dead
                // range exactly like the official's `_firstDeadNode !=
                // null` ignore. Outside a header the poll also fires to
                // OPEN the interval (`dead.is_none()`): a literal element
                // reached dead inside a collection is the official's
                // first dead node — `[1, 2, f(), 4]` on `Never f()`
                // anchors `4`, the covering close runs to `]`.
                if self.for_header_depth > self.header_cond_depth || self.dead.is_none() {
                    self.dead_code(*span);
                }
                // The literal's static type is `int` — the same
                // `matchedType` face `Ev::Bool` spells `bool` for
                // (`num? n = 1` promotes `n` to `num`: `int <: num` makes
                // the declared non-null form a type of interest, official
                // `_tryPromoteToTypeOfInterest`,
                // flow_analysis.dart:3982-4098). Without the interned id
                // (a direct-constructed interp) the face stays unknown —
                // the under-report posture, unchanged.
                let ty = self
                    .closed
                    .int_name
                    .map(|id| Ty {
                        base: crate::ty::Base::Name(id),
                        nullable: false,
                    })
                    .unwrap_or(Ty::UNKNOWN);
                let mut expr = Expr::trivial(ty);
                // An integer literal can never evaluate to null — the
                // same guaranteed-non-null face `type.isNonNullable`
                // gives the `??`/`?.` kills.
                expr.non_null_literal = true;
                self.exprs.push(expr);
            }
            Ev::Double { span } => {
                // The same header-part leaf poll `Ev::Int` carries —
                // outside a header it also fires to OPEN the interval
                // (`dead.is_none()`, the dead collection-element face).
                if self.for_header_depth > self.header_cond_depth || self.dead.is_none() {
                    self.dead_code(*span);
                }
                // `Ev::Int`'s `double` twin (`num? d = 1.5` promotes `d`
                // to `num`).
                let ty = self
                    .closed
                    .double_name
                    .map(|id| Ty {
                        base: crate::ty::Base::Name(id),
                        nullable: false,
                    })
                    .unwrap_or(Ty::UNKNOWN);
                let mut expr = Expr::trivial(ty);
                expr.non_null_literal = true;
                self.exprs.push(expr);
            }
            Ev::Null => self.exprs.push(Expr::trivial(Ty::NULL)),
            Ev::OtherExpr { .. } => {
                // `Ev::OtherExpr`'s only remaining producer is the `super`
                // read — it evaluates to a non-null value, so the
                // guaranteed-non-null kills (`super.field?.m`) apply even
                // though the type face stays the opaque unknown.
                let mut expr = Expr::trivial(Ty::UNKNOWN);
                expr.non_null_literal = true;
                self.exprs.push(expr);
            }
            Ev::This { span } => {
                // The `this` read: a promotable property receiver (official
                // `thisExpression()`'s `_thisVersion` reference,
                // flow_analysis.dart:8149-8153). The token anchors the
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
                // `this` can never be null (official `ThisExpression`'s
                // non-nullable static type).
                expr.non_null_literal = true;
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
                    // The `!` result carries the operand's own split
                    // (inverted): `current` still sits in the operand's
                    // true arm, so a value consumer that drops `!(x is T)`
                    // rejoins both arms exactly like the bare test's value
                    // context ([`Interp::rejoin_value_split`]).
                    expr.test_split = operand.test_split;
                    // `!x` is a boolean — non-null by construction (the
                    // `??`/`?.` kills' literal face).
                    expr.non_null_literal = true;
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
                    if !self.prop_target_captured(receiver) && !self.current.prop(slot).captured() {
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
                        // Owned vs inherited dead entry (upstream
                        // `checkUnreachableNode(rightOperand)`): the right
                        // operand's dead mark stands only when it is the
                        // FIRST dead node — a walk that already sits inside
                        // a dead context ignores it (dead_code_verifier
                        // .dart:440-443) and this binary's `flowEnd` stays
                        // silent. Post-LHS `self.current` is `taken`'s
                        // parent state: `!reachable(taken)` alone cannot
                        // tell "the LHS killed the RHS" (`never() && y` —
                        // owned, `&& y` row) from "the whole binary ran
                        // dead" (`return; x && y;` — inherited, folds into
                        // the enclosing interval). The discriminant is the
                        // enclosing dead context: an open interval or a
                        // claimed dead arm means this binary began dead —
                        // the LHS's own poll would have opened the window
                        // first (`x` in the dead statement); a clean ledger
                        // with an unreachable RHS means the LHS itself
                        // killed it.
                        let rhs_dead_entry = !self.reach.overall_reachable(taken.reach)
                            && self.dead.is_none()
                            && self.arm_dead_depth == 0
                            && !self.stmt_began_dead;
                        self.stack.push(Frame::Branch {
                            branch: other,
                            ifnull_ty: None,
                            op_span: *span,
                            rhs_dead_entry,
                            // The left operand's proven value, whether it
                            // folded as a literal/constant or as a nested
                            // short-circuit chain (`false && x && y`).
                            lhs_bool: left.bool_value.or(left.proven_bool),
                        });
                        self.current = taken.clone();
                        // The official `checkUnreachableNode(rightOperand)`
                        // poll at `rightBegin`
                        // (binary_expression_resolver.dart:287): a dead
                        // entry marks the first-dead node at the right
                        // operand itself, so the region is anchored
                        // OUTSIDE every construct the operand contains —
                        // inner `flowEnd`s then find the first dead node
                        // not contained in them and stay silent, while
                        // this binary's own close reports
                        // `[operator, rightOperand.end]` and resets
                        // (dead_code_verifier.dart:238-260, :311-326). The
                        // mark rides the operator's end offset — every
                        // emitted row re-anchors to the operator start, so
                        // the anchor only feeds the inside/outside
                        // comparison. The arm-dead count rides the entry
                        // too: nested polls inside the operand fold into
                        // this arm's close row.
                        if rhs_dead_entry {
                            self.arm_dead_depth += 1;
                            self.dead_code(TokenSpan {
                                id: span.id,
                                range: dartq_source::SourceRange {
                                    offset: span.range.end(),
                                    length: 0,
                                },
                            });
                        }
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
                        // The guaranteed-non-null kill is a sound-only
                        // step (official `ifNullExpression_rightBegin`,
                        // flow_analysis.dart:5857-5864: the nonNullable
                        // arm's `setUnreachable` rides
                        // `soundFlowAnalysisEnabled`; the nullOrEquivalent
                        // skip kill above it is ungated). The verdict reads
                        // [`Interp::guaranteed_non_null`]: a literal or
                        // `this` LHS is non-null by construction even
                        // though its flow face stays the opaque unknown —
                        // `1 ?? x`, `'s' ?? x`, `this ?? x` all dead
                        // their right side officially. The row is owned
                        // only when no enclosing dead context claimed the
                        // interval first — inside an open window or a dead
                        // arm the whole `??` ran dead and the official
                        // ignores the operand's mark (the same owned-vs-
                        // inherited gate the `&&`/`||` arm above reads).
                        if self.sound_flow
                            && (self.classify(left.ty) == Classification::NonNullable
                                || left.non_null_literal)
                        {
                            self.unreachable_cur();
                            rhs_dead_entry = self.dead.is_none()
                                && self.arm_dead_depth == 0
                                && !self.stmt_began_dead;
                        }
                        // `??` types as the LHS's non-null form.
                        let result_ty = promoted;
                        self.stack.push(Frame::Branch {
                            branch: shortcut,
                            ifnull_ty: Some(result_ty),
                            op_span: *span,
                            rhs_dead_entry,
                            lhs_bool: None,
                        });
                        // The `??` face of the dead-entry mark (the
                        // official `checkUnreachableNode(rightOperand)`
                        // poll runs for `??` operands too — the verifier's
                        // `parent is IfNull` re-anchor presumes the first
                        // dead node is the operand itself).
                        if rhs_dead_entry {
                            self.arm_dead_depth += 1;
                            self.dead_code(TokenSpan {
                                id: span.id,
                                range: dartq_source::SourceRange {
                                    offset: span.range.end(),
                                    length: 0,
                                },
                            });
                        }
                    }
                    BinOp::Other | BinOp::Eq | BinOp::NotEq => {
                        // A plain or equality binary (`a + b`, `a == b`):
                        // the right operand still resolves under the left's
                        // exit state — a Never-typed left (a `throw`, a
                        // bottom-returning call) leaves the operand dead
                        // and the official's `flowEnd(rightOperand)`
                        // re-anchors to the operator (`parent is
                        // BinaryExpression`, dead_code_verifier.dart:311-313).
                        // Park the operator span for the close; the
                        // entry-dead mark rides the same `rhs_dead_entry`
                        // latch the logical arms carry. The left operand's
                        // value stays on the stack — `BinaryEnd` pops both.
                        // The latch is owned-only: `!reachable()` at this
                        // point reads the LHS's exit state, which is dead
                        // both when the LHS itself killed it (`never() + b`
                        // — owned `+ b` row) and when the whole binary ran
                        // inside a dead context (`return; a + b;` — the
                        // LHS's own poll already opened the interval, and
                        // the official's `visitNode` ignore-list folds the
                        // operand mark, dead_code_verifier.dart:440-448).
                        // An open window, a claimed arm, or a statement
                        // that itself began dead separates the two — the
                        // began-dead latch covers the leading operand that
                        // never polled a window (`return; a + b;`).
                        let rhs_dead_entry = !self.reachable()
                            && self.dead.is_none()
                            && self.arm_dead_depth == 0
                            && !self.stmt_began_dead;
                        if rhs_dead_entry {
                            self.arm_dead_depth += 1;
                            self.dead_code(TokenSpan {
                                id: span.id,
                                range: dartq_source::SourceRange {
                                    offset: span.range.end(),
                                    length: 0,
                                },
                            });
                        }
                        self.stack.push(Frame::Branch {
                            branch: self.current.clone(),
                            ifnull_ty: None,
                            op_span: *span,
                            rhs_dead_entry,
                            lhs_bool: None,
                        });
                    }
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
                            // The official `flowEnd(rightOperand)`
                            // (binary_expression_resolver.dart:313): the
                            // first dead node (marked at the dead entry)
                            // is the right operand itself — its parent is
                            // this binary, so the row re-anchors to the
                            // operator and spans to the operand's end,
                            // then resets. A region opened before the
                            // operand keeps ownership (the fold). This
                            // arm's dead-arm contribution drops BEFORE
                            // the row: any count still standing is a
                            // strictly outer arm's, which is exactly the
                            // fold condition `dead_arm_row` reads.
                            self.arm_dead_depth = self.arm_dead_depth.saturating_sub(1);
                            self.dead_arm_row(op_span.range.offset, *end, true);
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
                        lhs_bool,
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
                    // The short-circuit dominance: `false && x` and
                    // `x && false` are `false` whichever way `x` runs,
                    // `true || x` and `x || true` are `true`, and a chain of
                    // two folded operands gives the operator's own value.
                    // The verdict is a certainty about the whole condition —
                    // an `if` over it has one provably dead arm — but the
                    // official carries no `booleanLiteral` info for a logical
                    // operator, so this face feeds the branch-fold marker
                    // only (see [`Expr::proven_bool`]).
                    expr.proven_bool = match (
                        op,
                        lhs_bool,
                        right.bool_value.or(right.proven_bool),
                    ) {
                        (BinOp::And, Some(false), _) | (BinOp::And, _, Some(false)) => Some(false),
                        (BinOp::Or, Some(true), _) | (BinOp::Or, _, Some(true)) => Some(true),
                        (BinOp::And, Some(left), Some(right)) => Some(left && right),
                        (BinOp::Or, Some(left), Some(right)) => Some(left || right),
                        _ => None,
                    };
                    // The `&&`/`||` arms carry the left operand's
                    // type-test split (the narrowed `current` stands in the
                    // taken arm): a value consumer drops the result and
                    // rejoins both arms, exactly like a bare test's value
                    // context.
                    expr.test_split = true;
                    // A `&&`/`||` result is a boolean — non-null by
                    // construction.
                    expr.non_null_literal = true;
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
                    if rhs_dead_entry && let Some(op_span) = op_span {
                        // The `??` face of the same official re-anchor
                        // (`parent is IfNull` → offset = operator,
                        // dead_code_verifier.dart:316-318). The arm's own
                        // dead-arm contribution drops before the row —
                        // `dead_arm_row` reads the OUTER arms only.
                        self.arm_dead_depth = self.arm_dead_depth.saturating_sub(1);
                        self.dead_arm_row(op_span.range.offset, *end, true);
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
                                | Ev::SetOrMapLitEnd { .. }
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
                    // The dead-RHS row precedes the operand pops (official
                    // `flowEnd(rightOperand)` at the construct's own end).
                    let (op_span, rhs_dead_entry) = match self.stack.pop() {
                        Some(Frame::Branch {
                            op_span,
                            rhs_dead_entry,
                            ..
                        }) => (Some(op_span), rhs_dead_entry),
                        other => {
                            if let Some(frame) = other {
                                self.stack.push(frame);
                            }
                            (None, false)
                        }
                    };
                    if rhs_dead_entry && let Some(op_span) = op_span {
                        self.arm_dead_depth = self.arm_dead_depth.saturating_sub(1);
                        self.dead_arm_row(op_span.range.offset, *end, true);
                    }
                    let right = self.pop_value();
                    let left = self.pop_value();
                    let inverted = matches!(op, BinOp::NotEq);
                    self.equality_check(&left, &right, inverted);
                }
                BinOp::Other => {
                    // Same dead-RHS row face as the equality arms (`parent
                    // is BinaryExpression` covers every user-definable
                    // operator the spelling carries).
                    let (op_span, rhs_dead_entry) = match self.stack.pop() {
                        Some(Frame::Branch {
                            op_span,
                            rhs_dead_entry,
                            ..
                        }) => (Some(op_span), rhs_dead_entry),
                        other => {
                            if let Some(frame) = other {
                                self.stack.push(frame);
                            }
                            (None, false)
                        }
                    };
                    if rhs_dead_entry && let Some(op_span) = op_span {
                        self.arm_dead_depth = self.arm_dead_depth.saturating_sub(1);
                        self.dead_arm_row(op_span.range.offset, *end, true);
                    }
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
            Ev::IsOperator { is_not, type_head } => {
                // The is-continuation dead poll (official
                // `analyzeExpression`'s bottom-type `handleExit`,
                // type_analyzer.dart:817-819, then the dead-code walk's
                // first unreachable node — the is-expression's TYPE
                // annotation, `dead_code_verifier.dart`'s `_visitNode` at
                // its `beginToken`): a Never-typed operand's exit lands
                // between the operand's events and the type spelling, so
                // `neverFn() is int` anchors its `DEAD_CODE` row at the
                // tested type and runs to the covering body's end —
                // `test() => neverFn() is int;` reports `int;`, exactly
                // the as-cast tail's own posture ([`Ev::AsBegin`]'s poll).
                // A reachable state makes this a no-op.
                self.dead_code(*type_head);
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
                    // `true` as a boolean literal on a mere subtype fact.
                    // The reference else-deadness comes from the promotion
                    // split instead — a bottom `factor(previous, tested)`
                    // marks the unmatched arm unreachable
                    // (`tryPromoteForTypeCheck`'s `ifFalseIsUnreachable`,
                    // :4156-4160), already
                    // [`Interp::branch_type_test`]'s face; `int x; x is num`
                    // therefore deads its else through the FACTOR, not the
                    // fold (oracle-probed on Dart 3.13.1).
                    let declared = self
                        .vars
                        .get(var as usize)
                        .map_or(Ty::UNKNOWN, |meta| meta.ty);
                    // The fail fold's nullability disjointness is a
                    // name-level question — the official's 0c arm
                    // suppresses it the moment either spelling maps to the
                    // invalid face (`Token? target is CommentToken` under a
                    // broken import: both are `InvalidType`, the relation
                    // is vacuously true, and the else — not the then — is
                    // the dead arm). The guard asks the back-end before
                    // the fold decides; the name-level default keeps
                    // today's posture.
                    let invalid_face = |ty: Ty, args: u32| {
                        self.ops.is_invalid_typed(ty, args, self.args, self.names)
                    };
                    if self.is_test_guaranteed_fail(declared, tested)
                        && !invalid_face(
                            declared,
                            self.vars
                                .get(var as usize)
                                .map_or(crate::ty::EMPTY_ARGS, |meta| meta.args),
                        )
                        && !invalid_face(tested, tested_args)
                    {
                        let cur = self.current.clone();
                        let dead = cur.clone().set_unreachable(&mut self.reach);
                        expr.branches = Some(if is_not { (cur, dead) } else { (dead, cur) });
                    } else if self.current.model(var).captured() {
                        // The write-capture bail of `tryPromoteForTypeCheck`
                        // (upstream Dart SDK source, pkg/_fe_analyzer_shared/
                        // lib/src/flow_analysis/flow_analysis.dart:4136-4138):
                        // a write-captured variable is never promoted, and
                        // the returned `ExpressionInfo` is TRIVIAL — both
                        // arms carry the untouched model. `isExpression_end`
                        // reaches this through the reference branch
                        // (:7190-7200) and never through the
                        // `_isTypeCheckGuaranteedToSucceed` fold
                        // (:7201-7205), so a test the declared type would
                        // satisfy outright (`int x` write-captured:
                        // `x is! int`, `x is num`) deads NEITHER arm —
                        // oracle-probed on Dart 3.13.1: no `dead_code` row,
                        // only the static `unnecessary_type_check`. The fail
                        // arm above still applies (it decides before the
                        // reference branch): a write-captured `int x` keeps
                        // its `x is Null` dead then-arm.
                        let cur = self.current.clone();
                        expr.branches = Some((cur.clone(), cur));
                    } else {
                        // The invalid-tested split: the official reference
                        // path still takes `tryPromoteForTypeCheck`, where
                        // `factor(previous, InvalidType)` is `Never` —
                        // `subtype.dart:274` makes any relation with an
                        // invalid type a subtype, so the factored type is
                        // bottom and `ifFalseIsUnreachable` fires
                        // (flow_analysis.dart:4156-4160, :4188-4190). No
                        // promotion step exists to an invalid type (the
                        // mutual-subtype guard rejects it), so the
                        // test-true arm keeps the unpromoted state while
                        // the test-false arm is unreachable — `is` deads
                        // its else, `is!` its then (oracle-probed on
                        // Dart 3.13.1: int/dynamic/invalid operands all
                        // carry the dead-arm row alongside the
                        // TYPE_TEST_WITH_UNDEFINED_NAME spelling row).
                        let invalid_tested =
                            self.ops
                                .is_invalid_typed(tested, tested_args, self.args, self.names);
                        // The UNSPELLABLE-tested face (a record type
                        // spelling — `r is (int,)`): the name-level
                        // lattice maps it to `Base::Unknown`, and
                        // `factor` treats the unknown head as top — its
                        // remainder bottoms vacuously (`is_subtype(X,
                        // Unknown) == true`), deading the unmatched arm
                        // the oracle keeps live. An unspellable tested
                        // type can neither factor nor promote at this
                        // level, so the test is a plain split with no
                        // dead arm. The gate is the NAME-level seam only:
                        // under the resolved back-end an `Unknown` face
                        // is the layer's own resolution gap (a dropped
                        // unresolvable head — the InvalidType stand-in
                        // `is_invalid_typed` certifies, or the gap the
                        // factor path must still see), never a spelling
                        // the lattice chose, so the split keeps the
                        // reference behavior there.
                        let unspellable_tested = tested.base == Base::Unknown
                            && !self.ops.is_resolved_seam();
                        let promoted = if invalid_tested || unspellable_tested {
                            None
                        } else {
                            self.promotion_candidate(var, tested, tested_args)
                        };
                        let (matched, unmatched) = if invalid_tested {
                            (
                                self.current.clone(),
                                self.current.clone().set_unreachable(&mut self.reach),
                            )
                        } else if unspellable_tested {
                            (
                                self.current.clone(),
                                self.current.clone(),
                            )
                        } else {
                            self.branch_type_test(var, tested, tested_args, promoted)
                        };
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
                        expr.branches = Some((if_true.clone(), if_false.clone()));
                        expr.test_split = true;
                        self.cond_split_var = Some(var);
                    }
                } else if let Some(slot) = operand.prop {
                    // The property-reference arm (official
                    // `tryPromoteForTypeCheck` reached through
                    // `isExpression_end`'s `_getExpressionReference` — the
                    // operand's `_PropertyReference` carries the property's
                    // promotion key exactly like a variable's): `x.f is B`
                    // promotes the `(receiver, f)` slot to the gated
                    // candidate, the unmatched arm carries the factored
                    // remainder. The flow layer cannot spell the member's
                    // declared type, so the slot's standing chain head —
                    // the non-null marker when never promoted — is the
                    // `previous` the candidate gate and the factor read;
                    // the semantic layer's eligibility verdict
                    // (`isPropertyPromotable`) stays the consumer's gate,
                    // exactly like the null-check face.
                    let receiver = self.props[slot as usize].0;
                    let model = self.current.prop(slot);
                    if self.prop_target_captured(receiver) || model.captured() {
                        // The write-capture bail (official
                        // `info.writeCaptured`, flow_analysis.dart:4136-4138):
                        // no promotion stands and the returned info is
                        // trivial — both arms keep the pre-test state.
                        let cur = self.current.clone();
                        expr.branches = Some((cur.clone(), cur));
                    } else {
                        let invalid_tested = self
                            .ops
                            .is_invalid_typed(tested, tested_args, self.args, self.names);
                        let unspellable_tested = tested.base == Base::Unknown
                            && !self.ops.is_resolved_seam();
                        let promoted = if invalid_tested || unspellable_tested {
                            None
                        } else {
                            self.prop_promotion_candidate(slot, tested, tested_args)
                        };
                        let (matched, unmatched) = if invalid_tested {
                            (
                                self.current.clone(),
                                self.current.clone().set_unreachable(&mut self.reach),
                            )
                        } else if unspellable_tested {
                            (
                                self.current.clone(),
                                self.current.clone(),
                            )
                        } else {
                            self.branch_prop_type_test(slot, tested, tested_args, promoted)
                        };
                        let (if_true, if_false) = if is_not {
                            (unmatched, matched)
                        } else {
                            (matched, unmatched)
                        };
                        self.current = if_true.clone();
                        expr.branches = Some((if_true.clone(), if_false.clone()));
                        expr.test_split = true;
                    }
                } else {
                    let cur = self.current.clone();
                    // The operand-side verdict folds are strict on the
                    // invalid face: the official's two folds
                    // (`_isTypeCheckGuaranteedToFail...` /
                    // `...ToSucceed...`, flow_analysis.dart:7184-7205)
                    // answer the subtype question with the invalid operand
                    // excluded — a member read over an invalid receiver
                    // keeps both arms (oracle-probed: `x.foo is String`
                    // and `x.foo.bar is String` stay silent), while a
                    // REFERENCE operand's promotion split carries the
                    // lenient 0c verdict above. The invalid-TESTED face
                    // below stays ungated: error recovery makes the test
                    // itself the succeed literal whatever the operand.
                    let operand_invalid = operand.non_reference_invalid;
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
                    if !operand_invalid && self.is_test_guaranteed_fail(operand.ty, tested) {
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
                    //
                    // An invalid operand with NO property slot (an
                    // unresolved call, a member read off a non-reference
                    // target — `x.foo()`, `nowhere.foo`, `f().bar`) has no
                    // flow reference, so the succeed fold reads its
                    // `InvalidType` static type and the 0c arm answers true
                    // (oracle-probed: every such `e is C` deads its else).
                    // Only the unresolved property read on a reference
                    // chain (`x.foo`, `x.foo.bar`, `this.g.foo`) stays open: the
                    // official `propertyGet` hands flow a property
                    // reference over the resolver's `dynamic` unpromoted
                    // type, which neither folds nor factors to bottom.
                    else if (operand_invalid && !operand.ref_chain)
                        || !operand_invalid
                            && (self.is_test_guaranteed(operand.ty, operand.args, tested, tested_args)
                            // The invalid-tested face: an unresolved spelling
                            // is the official's `InvalidType` on the checked
                            // side, and `subtype.dart:274` makes any relation
                            // with it a subtype — the test is the succeed
                            // literal. `is` keeps its then arm and deads the
                            // else, `is!` deads its then (oracle-probed on
                            // the analyzer corpus's unresolved-yaml file: the
                            // positive `is` block stays live, the `is!` block
                            // carries the dead-code row).
                            || self.ops.is_invalid_typed(
                                tested,
                                tested_args,
                                self.args,
                                self.names,
                            ))
                    {
                        let dead = cur.clone().set_unreachable(&mut self.reach);
                        expr.branches = Some(if is_not { (dead, cur) } else { (cur, dead) });
                    } else {
                        expr.branches = Some((cur.clone(), cur));
                    }
                }
                // An `is`/`is!` result is a boolean — non-null by
                // construction.
                expr.non_null_literal = true;
                self.exprs.push(expr);
            }
            Ev::AsBegin { .. } => {
                self.in_as_cast = true;
                // The cast operand ended (`x?.y as T`): rejoin the
                // operand's own null-shorting before the type spelling's
                // events.
                self.flush_null_shorts_above(self.consumer_base(1));
                // The cast-type dead poll runs at `Ev::AsOperator`, which
                // carries the type's head token: the official anchors the
                // row at the TYPE (`checkUnreachableNode` of the type
                // before it is visited, type_analyzer.dart:2090), so
                // `x as int` reports `int..covering close`, never `as..`
                // (the `as` token is no node at all). The latch records
                // whether the operand already killed the flow — see
                // [`Interp::as_began_dead`].
                self.as_began_dead = !self.reachable();
            }
            Ev::AsOperator { type_head } => {
                // The cast-type node poll (the `is` twin's face at
                // [`Ev::IsOperator`]): a Never-typed operand's exit lands
                // right before the type spelling, so the cast TYPE is the
                // official's first dead node and the row runs from its
                // head to the covering close (`doNotReturn() as int;`
                // reports `int;`). A reachable state makes the poll a
                // no-op.
                let began_dead = std::mem::take(&mut self.as_began_dead);
                let opened = self.dead.is_none();
                if began_dead {
                    self.dead_code(*type_head);
                }
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
                let cast_ty = self.tested_ty.take().unwrap_or(Ty::UNKNOWN);
                self.as_cast_pending = Some(cast_ty);
                // The Never-operand cast's type head parks on the enclosing
                // arm frame (the `AsEnd` latch above): first cast wins — a
                // later cast in the same arm already sits inside the dead
                // interval.
                if let Some(Frame::Cond {
                    never_cast,
                    cast_tail,
                    ..
                }) = self.stack.last_mut()
                {
                    if *never_cast && cast_tail.is_none() {
                        *cast_tail = Some(*type_head);
                    }
                    *never_cast = false;
                }
            }
            Ev::AsEnd => {
                self.in_as_cast = false;
                // A cast PATTERN whose sub-pattern is a GUARANTEED-EQUAL
                // constant (`case null as int`): the constant left the
                // cannot-fail posture standing (a total pattern), but the
                // cast wrapper can THROW — official `analyzeCastPattern`
                // runs the sub-pattern first and applies the cast to its
                // result — so the clause may fail and the close's chain
                // kill must not fire (the bare harness's
                // `switch (null) { case null as int: …; default: … }`
                // rowed the `default` dead). A resolved cast pattern adds
                // no may-fail ARM — "a cast pattern cannot fail to match"
                // (`cast_pattern_error_test` pins the dead else of
                // `if (expr<Object?>() case _ as int)`) — hence a posture
                // stand-down without a chain join.
                if self.guard_depth == 0
                    && let Some(pattern) = self.pattern.as_mut()
                    && std::mem::take(&mut pattern.guaranteed_equal)
                {
                    pattern.cannot_fail = false;
                }
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
                    // A cast whose operand never completed inside a
                    // conditional arm (`(throw 0) as T`): the arm's own
                    // `flowEnd` owns the dead row (the cast type is the
                    // first dead node, the row runs to the arm's end) —
                    // latch for the `AsOperator` that follows with the
                    // type's head token.
                    if operand.ty == Ty::NEVER
                        && matches!(self.stack.last(), Some(Frame::Cond { .. }))
                    {
                        if let Some(Frame::Cond { never_cast, .. }) = self.stack.last_mut() {
                            *never_cast = true;
                        }
                    }
                    // Official `asExpression_end`
                    // (flow_analysis.dart:6451-6477): a cast whose operand
                    // type and cast type are disjoint by nullability is
                    // guaranteed to throw
                    // (`_isTypeCheckGuaranteedToFailWithSoundNullSafety`)
                    // — the continuation goes unreachable here and the next
                    // executable node anchors the dead row (probed: `null
                    // as int`, `n as int` with `n: Null`, and `x as Null`
                    // all kill; a cast that ends the block reports nothing —
                    // no following node to anchor). The disjointness verdict
                    // is sound-only (`:7611`: `if (!…Enabled) return
                    // false`) — a pre-3.9 unit's cast never kills. A cast
                    // operand that never completes kills through its own
                    // `Ev::Exit` before this point; the promotion below is
                    // the only other reachability effect.
                    if self.sound_flow && self.disjoint_by_nullability(operand.ty, cast_ty) {
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
                    } else if let Some(slot) = operand.prop
                        && let Some(promoted) =
                            self.prop_promotion_candidate(slot, cast_ty, self.as_cast_args)
                    {
                        // The property cast arm (official
                        // `tryPromoteForTypeCast` on the operand's
                        // `_PropertyReference`, flow_analysis.dart:4088-4102):
                        // `x.f as B` promotes the slot to the gated
                        // candidate — the write-capture bail lives inside
                        // `finish_prop_type_test`'s model check.
                        self.current = self.current.clone().finish_prop_type_test(
                            self.ops,
                            &mut self.lists,
                            slot,
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
            Ev::IfNullAssignBegin { op } => {
                // The RHS-null-path kill (official
                // `ifNullExpression_rightBegin`'s nonNullable arm,
                // flow_analysis.dart:5857-5864): a guaranteed-non-null LHS
                // read type makes the null path the RHS runs on
                // unreachable — `int x = 0; x ??= 0` rows `0` (the operand
                // is the first dead node; `AssignmentExpression` carries
                // no operator re-anchor in dead_code_verifier, so the
                // window anchored by the operand's own poll IS the
                // official extent). A member/index LHS stays the opaque
                // unknown face — the resolved `readType` is the semantic
                // layer's answer, and a `PotentiallyNullable` classify is
                // the under-report direction. The `rhs_dead_entry` latch
                // tells the write's restore to keep the window — an
                // entry-dead arm reports, an exit-dead arm drops. Owned
                // only: a `??=` inside an already-dead context folds its
                // operand into the enclosing interval (the enclosing
                // statement's poll anchored first — re-anchoring it to
                // this operand would shrink the official row).
                let rhs_dead_entry = self.sound_flow
                    && self.dead.is_none()
                    && self.arm_dead_depth == 0
                    && !self.stmt_began_dead
                    && self
                        .exprs
                        .last()
                        .is_some_and(|left| {
                            self.classify(left.ty) == Classification::NonNullable
                                || left.non_null_literal
                        });
                // The `??=` skip-path snapshot (official
                // `ifNullExpression_rightBegin`'s split, flattened): the
                // RHS runs inline below, so the live pre-RHS state is
                // parked here for the exiting-RHS restore at the write.
                self.if_null_skips
                    .push((op.range.end(), self.current.clone(), rhs_dead_entry, None));
                if rhs_dead_entry {
                    self.unreachable_cur();
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
                // The `??=` RHS runs on the null path alone (official
                // `ifNullExpression_rightBegin`'s split); the flattened
                // replay runs it inline, so an RHS that EXITS (`x ??=
                // throw e`) must not leak its unreachability — the
                // post-state is the skip path (live; the write's own
                // non-null LUB promotion carries the null-aware face). A
                // dead window the exit's polls opened inside the RHS drops
                // with the restore: the skip path was never dead.
                // The `??=` RHS runs on the null path alone (official
                // `ifNullExpression_rightBegin`'s split); the flattened
                // replay runs it inline, so an RHS that EXITS (`x ??=
                // throw e`) must not leak its unreachability — the
                // post-state is the skip path (live; the write's own
                // non-null LUB promotion carries the null-aware face).
                // Restoring unconditionally-under-unreachable is sound:
                // a skip path that was already dead restores dead, and
                // a dead window the exit's polls opened inside the RHS
                // drops with a live restore — that path was never dead.
                if *if_null
                    && let Some((_, skip, rhs_dead_entry, operand_start)) =
                        self.if_null_skips.pop()
                    && !self.reachable()
                {
                    if rhs_dead_entry {
                        // The entry-dead `??=` RHS is the official's first
                        // dead node itself — anchor the interval at the
                        // operand's begin token (`AssignmentExpression`
                        // carries no operator re-anchor in
                        // dead_code_verifier). An inner poll's window folds
                        // into the operand anchor (the interval's owner is
                        // always the topmost dead node); no window means a
                        // bare literal operand polled nothing — the poll
                        // here opens it while the state is still the dead
                        // null path (the restore below is live).
                        let anchor = TokenSpan {
                            id: 0,
                            range: dartq_source::SourceRange {
                                offset: operand_start.unwrap_or(0),
                                length: 0,
                            },
                        };
                        match &mut self.dead {
                            Some(window) if window.anchor_from(anchor.range.offset) => {
                                window.reanchor_to(anchor);
                            }
                            Some(_) => {}
                            None => self.dead_code(anchor),
                        }
                    }
                    self.current = skip;
                    if self.reachable() {
                        self.exit_at = None;
                        if !rhs_dead_entry {
                            // An exit-dead `??=` RHS's window drops with
                            // the live restore — the skip path was never
                            // dead (`x ??= throw e`).
                            self.dead = None;
                        }
                    }
                }
                // A condition-valued plain `=` RHS (`v = a != null;`) is a
                // plain value: its split rejoins (see
                // [`Interp::rejoin_value_split`]). Compound operators and
                // `??=` recombine inside their own folds.
                if *plain {
                    self.rejoin_value_split(&rhs);
                }
                // A dead null-shorted WRITE target's interval ends at the
                // assignment's own end (`flowEnd` over the assignment node,
                // type_analyzer.dart:2616 via `handleNullShortingFinished`,
                // resolver.dart:1195-1200): `n?.p = i` reports `p = i`,
                // never `p = i;` and never the bare member.
                self.close_dead_null_write(*end);
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
                            // A promoted PROPERTY read's fold has no variable
                            // meta to compare against: its flow face is the
                            // property-slot marker (`UNKNOWN_NULLABLE`, its
                            // non-null form the null check records), while
                            // the staged face is the semantic layer's
                            // resolved declared head. When the fold carries
                            // the non-null promotion marker beside a staged
                            // nullable named head, the read the write copies
                            // is promoted — the staged head minus its
                            // nullability is the write's type (the oracle's
                            // static type of a promoted field read IS the
                            // promoted face), so `file = _defaultFile` under
                            // `_defaultFile != null` promotes the target the
                            // same way the variable-meta arm promotes
                            // `b = a`. The `prop` slot discriminates the
                            // property read from every other unknown-typed
                            // fold (an unresolved call's staged nullable
                            // return keeps the write unpromoted). A plain
                            // `=` alone: the `??=` write
                            // joins the staged face into its own
                            // non-null-always LUB below, and overriding the
                            // staged face here would promote what the
                            // oracle's `_computeIfNullType` leaves nullable
                            // (a staged nullable call RHS keeps the receiver
                            // unpromoted).
                            _ if *plain
                                && rhs.prop.is_some()
                                && rhs.ty == crate::ty::Ty::UNKNOWN
                                && staged_ty.nullable
                                && matches!(staged_ty.base, crate::ty::Base::Name(_)) =>
                            {
                                Some((staged_ty.non_null(), staged_args))
                            }
                            _ => Some((staged_ty, staged_args)),
                        }
                    })
                } else {
                    None
                };
                let (rhs_ty, rhs_ty_args) = staged.unwrap_or((rhs.ty, rhs_args));
                // The opaque write mark: under a MOUNTED resolved seam
                // (any staged entry proves the stager ran), a nullable-
                // declared local whose right-hand face the lattice still
                // spells `Unknown` — the stager drops a head it cannot
                // resolve, so the absence is the attempted-and-unresolved
                // gap, never a literal — keeps its reads' promotion
                // under-report-safe (see [`Interp::unresolved_writes`]).
                if (*plain || *if_null)
                    && self.ops.is_resolved_seam()
                    && !rhs_literal
                    && (rhs_ty == crate::ty::Ty::UNKNOWN
                        || (rhs_ty == crate::ty::Ty::UNKNOWN_NULLABLE
                            && rhs.prop.is_some()
                            && staged.is_none()))
                    && let Some(var) = target.reference
                    && self
                        .vars
                        .get(var as usize)
                        .is_some_and(|meta| meta.ty.nullable)
                {
                    self.unresolved_writes.insert(var);
                }
                // A plain assignment's target identifier is the write, not
                // a read: retract its facts. Both read facts can coexist at
                // this span — the target read may run under an active
                // promotion (writing inside a promoted branch) and may be
                // definitely unassigned. The gate is the read anchor, not
                // the reference: a top-level target tracks no reference but
                // carries the read-type row, and it is the write's target
                // all the same (a compound operator keeps its rows — the
                // read-modify-write target is also a read).
                //
                // A cascade SECTION's write is the exception: its popped
                // target is the cascade TARGET's own read (`builder..
                // leftBracket = lb` writes the property, and the read at
                // the frame's base is the RECEIVER use — the official keeps
                // the target read promoted across a section write, and the
                // pass-2 re-decision reads the promotion). The frame
                // arithmetic answers which write this is: section values
                // live at or above the cascade frame's base, so a stack
                // BELOW the base means the popped target was the cascade
                // target itself — a nested plain assign inside a section's
                // arguments (`x..a = (y = 1)`) keeps its rows above the
                // base and retracts normally.
                let cascade_target_write = match self.stack.last() {
                    Some(Frame::Cascade { exprs_base, .. }) => self.exprs.len() < *exprs_base,
                    _ => false,
                };
                if *plain
                    && !cascade_target_write
                    && let Some(span) = target.read_span
                {
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
                        // Downward inference: a raw creation of the target's
                        // own declared head (`o = Completer()` into
                        // `Completer<bool>? o`) infers its arguments from
                        // the write's context type — the instantiated
                        // `Completer<bool>`, never the raw face whose
                        // arguments the promotion step would fail.
                        let declared = self.vars.get(var as usize).map(|meta| (meta.ty, meta.args));
                        match declared {
                            Some((declared_ty, declared_args))
                                if rhs_ty_args == crate::ty::EMPTY_ARGS
                                    && declared_args != crate::ty::EMPTY_ARGS
                                    && !rhs_ty.nullable
                                    && matches!(rhs_ty.base, crate::ty::Base::Name(_))
                                    && rhs_ty.base == declared_ty.base
                                    && rhs.reference.is_none() =>
                            {
                                (rhs_ty, declared_args)
                            }
                            _ => (rhs_ty, rhs_ty_args),
                        }
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
                        // An unspellable RHS face: the unknown read, or the
                        // bare-read spelling faces (#2835's tear-off stand-in,
                        // the undefined-read's invalid name — both spell the
                        // read's own name on the implicit-this slot). The
                        // official RHS is the tear-off's real signature there,
                        // whose LUB with the declared form is the declared
                        // form itself (#2401).
                        let rhs_unspellable = rhs_ty == Ty::UNKNOWN_NULLABLE
                            || (rhs.prop.is_some()
                                && matches!(rhs.ty.base, Base::Name(n) if rhs.name == Some(n)));
                        if rhs_unspellable && lhs_nn.base == Base::Function {
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
                    // The condition-variable state rides ANY write
                    // (language#1274: the state lives on the written
                    // variable's new version — official
                    // `conditionVariableState`'s `_write`). A plain write
                    // whose RHS is the null-comparison/type-test split
                    // binds its arms to the written local (the initializer
                    // face's capture, [`Interp::cond_split`]'s replay); a
                    // bare read of another condition variable chain-copies
                    // its entry (`b = c`); every other write leaves the
                    // new version stateless — the stale entry dies instead
                    // of replaying through the write.
                    if *plain {
                        if rhs.test_split
                            && let Some((if_true, if_false)) = &rhs.branches
                            && let Some(tested) = self.cond_split_var.take()
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
                        } else if let Some(entry) = rhs
                            .reference
                            .and_then(|src| self.cond_vars.get(&src).cloned())
                        {
                            self.cond_split_var.take();
                            self.cond_vars.insert(var, entry);
                        } else {
                            self.cond_split_var.take();
                            self.cond_vars.remove(&var);
                        }
                    } else {
                        self.cond_split_var.take();
                        self.cond_vars.remove(&var);
                    }
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
                // The conditional's own dead entry (official
                // `checkUnreachableNode(conditional)` at the node's
                // pre-order visit — dead_code_verifier.dart:431-457): the
                // state BEFORE the condition evaluated decides whether the
                // conditional is the first dead node. A began-dead context
                // (dead statement, claimed arm, or an interval a
                // pre-condition poll opened) means the conditional
                // INHERITS its deadness; otherwise — the condition's own
                // exit killed the walk mid-node (`f(n, c ? x : y)` on a
                // Never `n`) — the conditional IS the official's first
                // dead node, anchored at its head token, and the arms fold
                // into that interval (`flowEnd(arm)` early-returns while
                // `_firstDeadNode` sits outside it).
                let cond_began_dead = self.stmt_began_dead
                    || self.arm_dead_depth > 0
                    || self.dead.is_some();
                let parent_live = !cond_began_dead;
                if parent_live && !self.reachable() {
                    let head = cond.read_span.or(cond.callee_span);
                    if let Some(head) = head {
                        self.dead_code(head);
                    }
                }
                // An arm owns its dead row only when the parent was live
                // (the arm itself is then the official first dead node); a
                // dead-parent arm inherits and folds into the enclosing
                // interval — no count, no row.
                let then_dead_entry =
                    parent_live && !self.reach.overall_reachable(if_true.reach);
                self.stack.push(Frame::Cond {
                    branch: if_false,
                    then_state: None,
                    parent_live,
                    then_dead_entry,
                    else_dead_entry: false,
                    never_cast: false,
                    cast_tail: None,
                });
                self.current = if_true;
                // The then arm entered dead: interior polls fold into its
                // own close row (the arm is the official's first dead
                // node). The count drops at `CondColon`.
                if then_dead_entry {
                    self.arm_dead_depth += 1;
                }
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
                    // The arm row re-anchors past any interval a poll
                    // opened inside the arm (an as-cast's type poll, a
                    // nested dead expression): official `flowEnd` reports
                    // the ARM node's range, the first dead node being the
                    // arm expression itself — the same re-anchor
                    // `dead_arm_row` gives the collection-if arms. The
                    // arm's own dead-arm contribution drops BEFORE the
                    // row so the gate reads the outer arms only.
                    self.arm_dead_depth = self.arm_dead_depth.saturating_sub(1);
                    self.dead_arm_row(then_begin.range.offset, *then_end, then_dead_entry);
                } else {
                    // A mid-arm death — a Never-operand cast whose type is
                    // the first dead node (`(throw 0) as T`): official
                    // `flowEnd(thenExpression)` reports from the type's
                    // head to the arm's own end (the initializer-context
                    // semantic face stages only the bare-initializer row;
                    // the arm close owns this one).
                    self.cast_tail_row(then_begin.range.offset, *then_end);
                }
                // The then arm ended: rejoin its open null-shorting before
                // the state swap. The then value STAYS on the expression
                // stack under the else arm's own pushes — `CondEnd` reads
                // its `(ty, args)` pair for the official
                // `leastUpperBound(thenType, elseType)` result
                // (static_type_analyzer.dart:87); popping it here left only
                // the else arm's face to type the whole conditional.
                self.flush_null_shorts_above(self.consumer_base(1));
                if let Some(Frame::Cond {
                    branch,
                    then_state,
                    parent_live,
                    else_dead_entry,
                    ..
                }) = self.stack.last_mut()
                {
                    *then_state = Some(self.current.clone());
                    *else_dead_entry =
                        *parent_live && !self.reach.overall_reachable(branch.reach);
                    self.current = branch.clone();
                    // The else arm entered dead too: its own close row
                    // reports at `CondEnd` and interior polls fold the
                    // same way the then arm's did.
                    if *else_dead_entry {
                        self.arm_dead_depth += 1;
                    }
                }
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
                    // `flowEnd(elseExpression)` (type_analyzer.dart:2616)
                    // with the same re-anchor as the then arm above: an
                    // interval a poll opened inside the else operand (the
                    // as-cast's type poll — `c ? a : x as T` on an
                    // always-true test) is the arm's own subspan, and the
                    // official's first dead node is the whole arm
                    // expression, so the row spans the operand whole. The
                    // arm's own dead-arm contribution drops BEFORE the
                    // row so the gate reads the outer arms only.
                    self.arm_dead_depth = self.arm_dead_depth.saturating_sub(1);
                    self.dead_arm_row(else_begin.range.offset, *arm_end, else_dead_entry);
                } else {
                    // The else arm's mid-arm cast-tail death (the then arm's
                    // twin above).
                    self.cast_tail_row(else_begin.range.offset, *arm_end);
                }
                // The else arm ended: rejoin its open null-shorting before
                // the arm join.
                self.flush_null_shorts_above(self.consumer_base(1));
                let else_expr = self.pop_value();
                let then_expr = self.pop_value();
                let Some(Frame::Cond { then_state, .. }) = self.stack.pop() else {
                    // A frame-less close (unbalanced recovery buffer): the
                    // conditional still nets one value for its consumer.
                    self.exprs.push(Expr::trivial(Ty::UNKNOWN));
                    return true;
                };
                let joined = self.join(then_state, Some(self.current.clone()));
                self.current = joined;
                // The conditional's static type is the official
                // `leastUpperBound(thenType, elseType)`
                // (static_type_analyzer.dart:87), through the same
                // [`Interp::name_lub`] join the `??` arm and the literal
                // element fold use: a subtype-ordered pair lands on its
                // spelled supertype, an unordered name pair on the unknown
                // face the resolved back-end's real hierarchy answers, and
                // an unspellable arm (the opaque unknown face) joins to the
                // spelled side exactly like `LUB(InvalidType, X) = X`.
                let ty = self.name_lub(then_expr.ty, else_expr.ty);
                // The written-argument tree rides the winning arm's row —
                // `LUB(List<num>, List<int>)` lands on a `Name(List)` head
                // whose message text still needs its `<num>` arguments;
                // an unordered join to the unknown face carries none.
                let args = if ty == else_expr.ty {
                    else_expr.args
                } else if ty == then_expr.ty {
                    then_expr.args
                } else {
                    crate::ty::EMPTY_ARGS
                };
                let mut expr = Expr::trivial(ty);
                expr.args = args;
                self.exprs.push(expr);
            }
            Ev::NullAwareIndexOperandBegin { operand } => {
                // The `?[` selector's split (official
                // `_nullAwareAccess_rightBegin`): the receiver sits on top
                // of the expression stack, the operand's events have not
                // run yet — capture the skip path now so an operand-side
                // `throw`/Never cannot kill it.
                self.null_aware_access_open();
                // The operand is the official's first dead node when the
                // access path died at this open (a `Null`/`Never`-typed
                // receiver: flow_analysis.dart:9214-9216, the bottom-type
                // exit :919-925). Nothing else polls it — reads poll
                // inside `for` headers only and literals never — so the
                // row opens here, at the operand's first token, and the
                // `]` face bounds it (`n?[i]` reports `i]`, `n?[0]` `0]`;
                // dead_code_verifier.dart:183-205, :253-279). A live
                // access path makes the poll a no-op, and so does a
                // statement that BEGAN dead — the top-down official walk
                // latched that statement first, so its own row owns the
                // whole extent (`return; a?[0];` is one row).
                if !self.stmt_began_dead {
                    self.dead_code(*operand);
                }
            }
            Ev::NullAwareMemberBegin => self.null_aware_access_open(),
            Ev::AnonMethodBegin { punct, null_aware } => {
                // The anonymous-method member's null-aware face: a bare
                // `?.` member fires no `Ev::NullAwareMemberBegin` for this
                // shape (the parser's anonymous-method arm skips the
                // selector events), so the short opens here — official
                // `_startNullAwareAccess(node.target)` inside
                // `visitAnonymousMethodInvocation` (resolver.dart:1977). A
                // `Null`/`Never` receiver kills the body path at open; a
                // `?..` head's short is `CascadeBegin`'s, already open.
                if *null_aware {
                    self.null_aware_access_open();
                }
                // The body poll (official `checkUnreachableNode(node.body)`
                // / `visitNode(parameters ?? node.body)`,
                // resolver.dart:1963-1980): a dead access path makes the
                // member body the first dead node, and the covering close
                // re-anchors the row at the operator (`parent is
                // AnonymousMethodInvocation`,
                // dead_code_verifier.dart:317-318) — `punct` IS that
                // operator, so `x.=> 1` rows `.=> 1;`, `x..=> 1` rows
                // `..=> 1;`, `null?.=> 1` rows `?.=> 1;`. A live access
                // path makes the poll a no-op.
                self.dead_code(*punct);
            }
            Ev::AnonMethodEnd { end } => {
                // The member's own end: the `?.` member's chain-end face
                // (upstream `flowEnd(expression)` at `finishNullShorting`
                // ends the row at the member's last token — `null?.=> 1;`
                // reports `?.=> 1`, expected 3:7|6, the `;` never joins).
                // A `.`/`..`/`?..` member closes no `flowEnd` upstream —
                // the face is inert without an open selector short (the
                // plain `?..` head's interval keeps running to `;`,
                // 3:7|8).
                self.chain_face_end(*end);
            }
            Ev::IndexAccess {
                null_aware,
                close_end,
            } => {
                // The `]` closer is the null-shorted chain's row end face
                // (`c1?[0]` ends after `]`) — same contract as the send's
                // `)` face below.
                self.chain_face_end(*close_end);
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
                // An index read on an invalid receiver is invalid itself
                // (upstream `_resolveIndexRead`'s `isInvalid` arm,
                // property_element_resolver.dart:660-674).
                if let Some(recv) = self.exprs.last()
                    && self
                        .ops
                        .is_invalid_typed(recv.ty, recv.args, self.args, self.names)
                {
                    expr.ty = recv.ty;
                    expr.args = recv.args;
                    expr.non_reference_invalid = true;
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
            Ev::DotAccess {
                null_aware,
                member,
                ident,
            } => {
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
                // An exit the access ITSELF produced (the selector's
                // invocation returned Never — `Error.throwWithStackTrace(..)`
                // in the post-order stream fires this poll after its own
                // `Ev::Exit`) is the statement's own death at or after the
                // selector's span: the access was live when the official's
                // pre-order poll ran, so the next node anchors — the same
                // `exit_at` boundary the statement wrapper applies.
                if self
                    .null_shorts
                    .last()
                    .is_some_and(|short| !short.dead_at_open)
                {
                    // Interior death — the skip path rejoins at the chain's
                    // boundary and carries the continuation's reachability.
                } else if !null_aware
                    && matches!(events.get(i + 1), Some(Ev::ArgumentsBegin { .. }))
                {
                    // A non-null-aware invocation's method name is never a
                    // dead-node candidate upstream (resolver.dart:3619
                    // polls `node.methodName` only under `isNullAware`):
                    // `a.m(1, 2)` on a `Never a` reports its first dead
                    // node at the argument list — `Ev::ArgumentsBegin`
                    // anchors at `(` below.
                } else if !self.stmt_began_dead
                    && !self.exit_at.is_some_and(|at| at >= member.range.offset)
                {
                    // A `?.` whose access path was dead at open (a
                    // null-or-equivalent receiver) anchors at the member
                    // name — the official polls the property/method name
                    // node. `member` is the send's tiled end there (the
                    // #585 follower), so the anchor reads the member's own
                    // identifier token.
                    let anchor = self
                        .exprs
                        .last()
                        .and_then(|e| e.read_span)
                        .unwrap_or(*ident);
                    self.dead_code(anchor);
                }
                // The member identifier's own entry sits on top (every
                // member shape pushes through its primary parse); the
                // receiver stays BENEATH it — consumed by the enclosing
                // statement's pop, or kept for the cascade's
                // `CascadeEnd` truncate (the cascade sections route
                // through this event with the target under the section
                // frame's base). The entry is peeked, never popped.
                let member_value = self.pop_value();
                // The post-send tile: a completed dotted send's stream
                // re-accesses at the send's tiled end (`x.m()` fires
                // DotAccess again immediately AFTER the Send event, #585's
                // tiled-end contract) — the popped top is then the send's
                // OWN result, never a selector entry (a real selector's
                // access always follows the selector's identifier event,
                // never a send). The tile keeps the access build intact
                // (the chain/branch arms below, the receiver pop) and only
                // restores the send's result type at the end, so a send
                // whose callee adopted a staged return keeps it as the
                // chain's value.
                let post_send_tile = match events.get(i.wrapping_sub(1)) {
                    Some(Ev::Send { .. } | Ev::SendNoArgs { .. }) => !matches!(
                        // A bare leaf's own send (`c` of `c._b`) fires
                        // between the leaf and the chain's next selector —
                        // an Ident directly under the send marks the
                        // leaf-send, and the access above it is a real
                        // chain selector, never the tile.
                        events.get(i.wrapping_sub(2)),
                        Some(Ev::Ident { .. })
                    ),
                    _ => false,
                };
                // The member identifier is the null-shorted chain's row end
                // face for a plain (send-less) selector (`x?.y.z` ends
                // after `z`). The `member` param tiles one token past the
                // access (#585 follower) even for a bare leaf, so the face
                // reads the recorded `ident`; a completed send's tile is
                // skipped — the send's own `)` face already covered it.
                if !post_send_tile {
                    self.chain_face_end(ident.range.end());
                }
                let mut expr = Expr::trivial(Ty::UNKNOWN);
                // The member's own token rides the access row as its
                // callee role (#2567 R18): a following send keys the
                // staged invocation-result lookup on it. The read-span
                // faces (write retraction, condition queries) stay
                // untouched — the callee role is a dedicated field no
                // other consumer reads.
                expr.callee_span = member_value.read_span;
                // The property-read face (#1409): a plain-identifier
                // member on a promotable receiver (a bare local read or
                // `this` — official `ExpressionPropertyTarget`'s reference
                // version and `ThisPropertyTarget`'s `_thisVersion`,
                // `flow_analysis.dart:182-198`, :5977-5990) resolves to a
                // property slot. Eligibility (a private final field, the
                // official `isPropertyPromotable` verdict) is the
                // semantic layer's question — the flow layer tracks every
                // promotable-shaped read and the consumer gates the rows.
                if !post_send_tile
                    && let (Some(field), Some(receiver)) = (
                        member_value.name,
                        self.exprs.last().and_then(|e| e.prop_receiver()),
                    )
                {
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
                        self.emit_prop_read_type(slot, member_span);
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
                    // receiver (`x?.y.z`) is inside the same shorted
                    // region — propagate the base so the consumer flushes
                    // the whole chain.
                    expr.chain_base = Some(inner);
                }
                // The invalid-receiver member read (the member twin of the
                // unresolved-call head): a receiver whose type IS the
                // invalid face — an unresolved written annotation, an
                // unresolved `on` type, another invalid read — reads every
                // member as the official's `InvalidType`, spelled as the
                // receiver's own ty so the resolved back-end's existing
                // unresolved-spelling map answers it (subtype.dart's 0c
                // arm makes every relation with it vacuous: the local
                // this read feeds keeps `x is T` always-true with the
                // failing arm dead, and a switch over it vacuously covers
                // every typed pattern).
                if !post_send_tile
                    && let Some(recv) = self.exprs.last()
                    && self
                        .ops
                        .is_invalid_typed(recv.ty, recv.args, self.args, self.names)
                {
                    expr.ty = recv.ty;
                    expr.args = recv.args;
                    expr.non_reference_invalid = true;
                }
                // The staged member face: the semantic layer resolved the
                // accessor's declared type at the member identifier
                // (`switch (d.t)` over a sealed `T`). A promoted property
                // keeps its non-null unknown face, a null-shorted access
                // its own `?` (a face the staged row does not spell).
                let promoted_prop = expr.prop.is_some() && expr.ty == Ty::UNKNOWN;
                if !post_send_tile
                    && !promoted_prop
                    && !expr.non_reference_invalid
                    && matches!(expr.ty, Ty::UNKNOWN | Ty::UNKNOWN_NULLABLE)
                    && expr.chain_base.is_none()
                    && let Some(span) = member_value.read_span
                    && let Some(&staged) = self.staged_members.get(&span.range.offset)
                {
                    expr.ty = staged;
                }
                // The tile's one payload: the send's own result type
                // stands (the adopted staged return included) — the fresh
                // access build around it (chain bases, branches, the
                // receiver pop) is exactly what the post-send position
                // owes the chain.
                if post_send_tile && !matches!(member_value.ty, Ty::UNKNOWN | Ty::UNKNOWN_NULLABLE)
                {
                    expr.ty = member_value.ty;
                    expr.args = member_value.args;
                }
                // The invalid-receiver invocation (the send twin of the
                // member read above): a method call on an invalid receiver
                // resolves without a target and its result is the
                // official's `InvalidType` whatever the name — `Object`'s
                // own methods included (upstream Dart SDK source,
                // pkg/analyzer/lib/src/dart/resolver/
                // method_invocation_resolver.dart:616-640).
                else if post_send_tile
                    && let Some(recv) = self.exprs.last()
                    && self
                        .ops
                        .is_invalid_typed(recv.ty, recv.args, self.args, self.names)
                {
                    expr.ty = recv.ty;
                    expr.args = recv.args;
                    expr.non_reference_invalid = true;
                }
                if !post_send_tile && let Some(recv) = self.exprs.last() {
                    expr.ref_chain = recv.ref_chain
                        || recv.prop_receiver().is_some()
                        || (recv.prop.is_some() && !recv.non_reference_invalid);
                }
                // The receiver pops unless it is the innermost cascade's
                // target (same discipline as IndexAccess's).
                if !self.receiver_is_cascade_target() {
                    self.exprs.pop();
                }
                self.exprs.push(expr);
            }
            ev @ (Ev::Send { .. } | Ev::SendNoArgs { .. }) => {
                // The invocation's own closer is the null-shorted chain's
                // row end face (official `handleNullShortingFinished`'s
                // `flowEnd(expression)` extent): a dead chain's window
                // grows to the `)`/head end while the chain runs, and the
                // short's pop below reports `anchor .. chain end` — the
                // statement's `;` never folds in.
                if let Ev::Send { end, .. } | Ev::SendNoArgs { end } = ev {
                    self.chain_face_end(*end);
                }
                // The callee flows through as the send's result: a bare
                // identifier keeps its variable reference and promoted type
                // (a plain assignment's target is exactly such a reference).
                if let Some(mut callee) = self.exprs.pop() {
                    // The captured top-level callable's declared return
                    // (see [`Expr::ret`]): `f()` over `Never f()` adopts
                    // `Never` as its result — the official's
                    // `expressionOrFunctionInvocation` records the
                    // invoked element's return type, so the send's own
                    // `flowEnd` exits on the bottom face
                    // (dead_code_verifier.dart:227-230 + the
                    // `firstDeadNode` poll at the trailing nodes). Only a
                    // real invocation (`Ev::Send` after `ArgumentsEnd`)
                    // adopts: a bare `Send` on a leaf is the send-less
                    // selector face the read already typed.
                    if matches!(ev, Ev::Send { .. })
                        && matches!(events.get(i.wrapping_sub(1)), Some(Ev::ArgumentsEnd { .. }))
                        && let Some((ret_ty, ret_args)) = callee.ret
                    {
                        callee.ty = ret_ty;
                        callee.args = ret_args;
                        callee.ret = None;
                    }
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
                        let Ev::Send { args, .. } = ev else {
                            unreachable!("gated on Ev::Send above")
                        };
                        if self.is_class_head(name) {
                            callee.ty = Ty {
                                base: Base::Name(name),
                                nullable: false,
                            };
                            callee.args = *args;
                        } else if !self.local_fns.contains(&name)
                            && self.ops.call_head_unresolved(name, self.names)
                        {
                            // A nowhere-resolving head's result is the
                            // official's `InvalidType` — spelled as the
                            // head's own name so the RESOLVED back-end's
                            // existing unresolved-spelling map answers it
                            // (a name that resolves as no type reads
                            // `Invalid`, and `subtype.dart`'s 0c arm makes
                            // every relation with it vacuous: a var this
                            // send feeds keeps `x is T` always-true and
                            // the failing arm dead, exactly the oracle's
                            // dead-else row). A local-function name
                            // resolves in scope, so it never takes this
                            // face — its call keeps the unknown return the
                            // `??` widening gate reads (`int? x = a ??
                            // localFn()` stays nullable). At the name
                            // level the same spelling answers
                            // conservatively — not top, not promotable,
                            // its factor keeps it — so both arms stay
                            // live there.
                            callee.ty = Ty {
                                base: Base::Name(name),
                                nullable: false,
                            };
                            // A call is never a reference operand — its
                            // invalid face blocks the is-check folds
                            // (`nowheresfn() is C` keeps both arms).
                            callee.non_reference_invalid = true;
                        } else {
                            callee.ty = Ty::UNKNOWN;
                        }
                        callee.prop = None;
                    }
                    // The staged invocation result (#2567 R18): the
                    // semantic layer resolved the send's return at the
                    // close (the send-close inference's parameter
                    // solution included — `readEnum(Tag.values)` answers
                    // the enum), where this layer's own callee read is an
                    // unknown face. Adopt it so a switch whose scrutinee
                    // is the call answers the always-exhaustive verdict
                    // on the real return. The signature-unknown function
                    // face joins the unknown reads: a callable-typed local
                    // or parameter reads as [`Base::Function`], which by
                    // contract spells the head only — its return type is
                    // the semantic layer's answer, so the staged row is
                    // the sole source of the invocation's static type; with
                    // no staged row the call keeps the function head, whose
                    // nullability the equality fold never reads — the head
                    // spells the call, not the value, so a comparison over
                    // it folds nothing ([`Interp::equality_check`]). A
                    // null-shorted chain stays
                    // out: the official `?.`-call result carries the
                    // short's own `?` on top of the declared return, a
                    // face the staged row does not spell — keeping the
                    // unknown under-reports exactly as before.
                    let callee_key = callee.callee_span.or(callee.read_span);
                    if callee.chain_base.is_none()
                        && matches!(
                            callee.ty,
                            Ty::UNKNOWN
                                | Ty::UNKNOWN_NULLABLE
                                | Ty {
                                    base: Base::Function,
                                    nullable: false,
                                }
                        )
                        && let Some(span) = callee_key
                        && let Some(&(ty, args)) = self.staged_calls.get(&span.range.offset)
                    {
                        // An open null-short whose depth covers the callee
                        // (`x?.m()`, the receiver's split still unclosed)
                        // makes the call's static type nullable — the
                        // short's own `?` rides the declared return
                        // (upstream's short-circuit result face), so the
                        // adopted return joins it rather than the raw
                        // non-null declaration.
                        let shorted = self
                            .null_shorts
                            .last()
                            .is_some_and(|short| short.exprs_depth <= self.exprs.len() + 1);
                        callee.ty = Ty {
                            nullable: ty.nullable || shorted,
                            ..ty
                        };
                        callee.args = args;
                        callee.prop = None;
                    }
                    // A real invocation's result is never a reference
                    // operand: the call's static type is the invoked
                    // element's RETURN type, so `_getExpressionReference`
                    // answers null upstream and an `is` test on the call
                    // takes the non-reference arm. The callee flows through
                    // as the result row, so its own variable slot must be
                    // dropped here — `nf() is int` reads the invocation's
                    // type, never `nf`'s declared callable type (the same
                    // leak fed `condition_operand`, the staged-RHS write's
                    // promoted-read arm, and `x == nf()`'s reference split).
                    // The property slot goes with it: the invoked row is no
                    // property read either (the staged-adoption arm already
                    // clears it for the same reason), so `x = m()` cannot
                    // take the promoted-property write face. The
                    // by-construction non-null face drops too — the call
                    // evaluates to whatever the return declares, not to the
                    // callee variable's own proven-non-null value
                    // (`final f = fn; f() ?? y` keeps `y` when the return
                    // is nullable).
                    // The gate is the argument list like the typing faces
                    // above: a bare `Send` with no `ArgumentsEnd` before it
                    // types no call and keeps the read's identity (a
                    // `f<T>` tear-off never even fires `Send` — it is a
                    // type-argument application — so the gate's real edge
                    // is every invocation, and the check stays as the
                    // belt-and-suspenders the sibling faces already keep).
                    // `super(...)`/`this(...)` constructor sends carry no
                    // variable reference to begin with (a `this`/`super`
                    // read tracks the receiver slot, the unresolved member
                    // read the property one), so clearing hits exactly the
                    // callable-value head shape.
                    if matches!(ev, Ev::Send { .. })
                        && matches!(events.get(i.wrapping_sub(1)), Some(Ev::ArgumentsEnd { .. }))
                    {
                        callee.reference = None;
                        callee.prop = None;
                        callee.non_null_literal = false;
                        // The chain flag dies with the slots (#3452): the
                        // member callee arrived carrying the
                        // reference-rooted `ref_chain` its own DotAccess set
                        // (`x.f`), and a surviving flag re-roots every member
                        // read over the result (`x.f().g`) at the callee's
                        // variable — an invalid face on such a read then took
                        // the reference chain's lenient arm where the oracle
                        // deads the else (`f().bar`'s group, oracle-probed).
                        // The plain-read chains keep it: no argument list, no
                        // clear (`x.f`, `x.f.g` stay open).
                        callee.ref_chain = false;
                    }
                    self.exprs.push(callee);
                    // The bottom-typed result's exit (official
                    // `recordStaticType`'s `handleExit` on
                    // `type.isBottom`, ast.dart:11146-11153): a
                    // `Never`-returning send ends the enclosing flow at
                    // the call's own end — `f(Never a) { g(a); x; }` rows
                    // `x;`. The semantic layer stages the send-level
                    // `Ev::Exit` for the resolved sends it tracks; this
                    // face covers the ones only the flow layer re-typed
                    // (a staged `Never` return adopted above).
                    if self.ops.is_bottom(self.exprs.last().map(|e| e.ty).unwrap_or(Ty::UNKNOWN))
                        && let Ev::Send { end, .. } | Ev::SendNoArgs { end } = ev
                    {
                        self.unreachable_cur();
                        if !self.collecting() {
                            self.exit_at = Some(*end);
                        }
                    }
                }
            }
            Ev::ArgumentsBegin { open } => {
                // The argument-list node poll (official
                // `checkUnreachableNode` on the argument list, the first
                // candidate a dead receiver's member call reaches — the
                // non-null-aware method name is never polled,
                // resolver.dart:3617-3621): `a.m(1, 2)` on a `Never a`
                // anchors at `(`, and an anonymous invocation on a dead
                // callee (`f()(x)`) takes the same face (official
                // `AnonymousMethodInvocation` re-anchor,
                // dead_code_verifier.dart:317-318). An exit produced at or
                // after `(` follows the member poll's boundary rule;
                // `dead_code` itself folds the poll into an open interval
                // and skips arm-dead contexts. A `?.` whose access path
                // died at OPEN keeps its own anchoring: the official
                // polls the member name before the argument list
                // (resolver.dart:3619), and this event stream's member
                // poll lands at the post-send `DotAccess` — letting `(`
                // anchor first would steal `null?.abs()`'s `abs()` row.
                // The poll itself is gated on the callee's own shape: a
                // member send (its selector walked as an
                // `ExpressionContinuation` identifier just before the
                // list) or an anonymous invocation (a `Send` just closed —
                // `f()(x)`) reports its argument list, while a BARE call's
                // own invocation node anchors the interval (`print('2')`
                // rows at `print`, not at `(` — its callee walked as a
                // plain `Expression` read).
                //
                // The callee that EXITED mid-expression is the other face:
                // `x<int>(args)`'s instantiation completes with no event of
                // its own (the type-argument group only sets the pending
                // tree), so the event before `(` is the head identifier —
                // `x` live-read, the exit landing at its end (`exit_at`),
                // every later node dead. The official's argument-list poll
                // re-anchors to `(` whatever the first dead child was
                // (`parent is AnonymousMethodInvocation`,
                // dead_code_verifier.dart:317-318), so `(` owns the row for
                // a bare `x(...)`, a generic `x<int>(...)`, a parenthesized
                // `(f)(...)`, or an index-call `a[i](...)` callee alike —
                // and for a send callee whose `Ev::Exit` the engine
                // injected between the send's close and `(` (`f()(x)` on a
                // `Never f()` result: the exit IS the event before the
                // list). A callee that BEGAN dead in an already-dead region
                // folds the same way the member-send face does — the
                // statement wrapper's re-anchor lifts the anchor to the
                // statement's first node (`print('2')` still rows at
                // `print`).
                if self
                    .null_shorts
                    .last()
                    .is_some_and(|short| short.dead_at_open)
                {
                    // The `?.` face owns the anchor — the member name is
                    // the first dead node upstream.
                } else if matches!(
                    events.get(i.wrapping_sub(1)),
                    Some(
                        Ev::Ident { .. }
                            | Ev::Send { .. }
                            | Ev::SendNoArgs { .. }
                            | Ev::ParenEnd
                            | Ev::IndexAccess { .. }
                            | Ev::Exit { .. }
                    )
                ) && !self.exit_at.is_some_and(|at| at >= open.range.offset)
                {
                    self.dead_code(*open);
                }
                self.args_bases.push(self.exprs.len());
            }
            Ev::ArgumentEnd => {
                // The argument-node poll (official
                // `checkUnreachableNode` on the argument expression): a
                // sibling's exit before it makes the argument the first
                // dead node — `a.clamp(b, c)` on a `Never b` rows `c);`
                // (dead_code_verifier.dart's plain `node` arm; the
                // `AssignmentExpression`/`BinaryExpression` re-anchors do
                // not apply, so the operand's own span IS the official
                // extent). An exit INSIDE the argument (`f(throw x)`, or a
                // `Never`-typed read the argument itself is — `f(b)` on a
                // `Never b`) is the argument's own death at or after its
                // first token: the argument was live when the official's
                // pre-order poll ran, so the next node anchors — the same
                // `exit_at` boundary the member poll applies. Only a bare
                // operand's own span is known here (`read_span` for a leaf
                // read, `callee_span` for a completed send's head); a
                // compound argument without either falls back to its
                // first-token anchor — the region's first spanned event
                // since the enclosing list's opening (a literal argument
                // `g(throw 0, 1)` carries no read/send span, yet the
                // official's top-down poll still visits the ARGUMENT node:
                // dead_code_verifier.dart:183-189) — and a compound one
                // without either stays to its inner polls.
                let arg_span = self
                    .exprs
                    .last()
                    .and_then(|arg| arg.read_span.or(arg.callee_span))
                    .or_else(|| {
                        let start = events[..i]
                            .iter()
                            .rposition(|ev| {
                                matches!(ev, Ev::ArgumentEnd | Ev::ArgumentsBegin { .. })
                            })?;
                        let region = &events[start + 1..i];
                        // A `throw` argument is its own death (official
                        // `parseThrowExpression` — the argument was live
                        // when the pre-order poll ran, so the NEXT node
                        // anchors); its interior literal's span must not
                        // stand in for the argument node.
                        if region.iter().any(|ev| matches!(ev, Ev::Throw)) {
                            return None;
                        }
                        region.iter().find_map(|ev| ev.span())
                    });
                if let Some(arg_span) = arg_span
                    && !self.reachable()
                    && !self
                        .exit_at
                        .is_some_and(|at| at >= arg_span.range.offset)
                {
                    self.dead_code(arg_span);
                }
                // A condition-valued argument (`f(x is T)`, `f(!(x is T))`)
                // consumed as a plain value rejoins its split — the official
                // leaves `_current` recombined at the argument's end, so the
                // narrowed arm must not leak into the sibling arguments or
                // the continuation after the call.
                if self.exprs.last().is_some_and(|arg| arg.test_split)
                    && let Some(arg) = self.exprs.last().cloned()
                {
                    self.rejoin_value_split(&arg);
                }
                // This argument's expression is complete: rejoin the
                // null-shorting IT opened, at its own boundary — official
                // finishes each argument's shorting before the next one
                // starts (`finishNullShorting` at the expression's end;
                // the `?.` in `f(a?.b, c!)` must not join away the later
                // sibling's `!` promotion at the list's close). A
                // caller-side `?` (`f()?.m(g()?.n)`'s outer short) opened
                // at or below the base and survives to its chain's end.
                if let Some(&base) = self.args_bases.last() {
                    self.flush_null_shorts_above(base);
                }
            }
            Ev::ParenBegin { .. } | Ev::AwaitBegin { .. } => {}
            Ev::ParenEnd => {
                // The parenthesized operand ended: rejoin the null-shorting
                // it opened (`(x?.y)` — the chain's boundary is the `)`,
                // not the enclosing selector list).
                self.flush_null_shorts_above(self.consumer_base(1));
            }
            Ev::ArgumentsEnd { count } => {
                // The list closes: its base leaves the per-argument flush
                // stack (before the `identical` fast path's early return).
                self.args_bases.pop();
                // The arguments are complete expressions: rejoin the
                // null-shorting they opened — but not a caller-side `?`
                // context (`f()?.m(g()?.n)`'s outer short opened before
                // `ArgumentsBegin` and stays open to the chain's end;
                // official `nullShortingTargetDepth` bound). Shorts the
                // per-argument boundary already closed leave nothing here.
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
                    // An argument whose `ArgumentEnd` never fired (recovery
                    // buffers) still rejoins before the call's value flows
                    // on — a condition-valued argument consumed as a plain
                    // value must not leak its narrowed arm past the list.
                    if let Some(arg) = self.exprs.pop() {
                        self.rejoin_value_split(&arg);
                    }
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
            Ev::RecordLitEnd { count } => {
                // The record's fields are complete expressions: rejoin
                // the null-shorting they opened (the receiver-chain `?`
                // outside the literal stays open to the enclosing
                // expression's end). The record keeps the collapsed
                // tail's face — a record scrutinee's exhaustiveness
                // (`(previous, next)` deciding a switch) reads it, and
                // the collection-element strip below must not erase it.
                self.flush_null_shorts_above(self.consumer_base(*count));
                self.literal_value_keep_ty(*count)
            }
            Ev::ListLitEnd { count, args } => {
                // The literal's elements are complete expressions: rejoin
                // the null-shorting they opened (the receiver-chain `?`
                // outside the literal stays open to the enclosing
                // expression's end).
                self.flush_null_shorts_above(self.consumer_base(*count));
                // `List`'s interned id spells the literal's static type —
                // `List<E>` over the written `<E>` arguments (upstream
                // `resolveListLiteral`'s written-arguments arm,
                // typed_literal_resolver.dart:68-107): `var l = <int>[]`'s
                // static type is `List<int>`, the `matchedType` an
                // initializer or assignment write promotes through.
                self.literal_value(*count, self.closed.list_name, *args)
            }
            Ev::SetOrMapLitEnd {
                count,
                is_set,
                args,
            } => {
                self.flush_null_shorts_above(self.consumer_base(*count));
                // `<E>{...}` spells `Set<E>` and `<K,V>{...}` spells
                // `Map<K,V>` (upstream `resolveSetOrMapLiteral`'s
                // written-arguments arm,
                // typed_literal_resolver.dart:732-758); the recorder
                // resolved the head (see [`Ev::SetOrMapLitEnd`]).
                let name = if *is_set {
                    self.closed.set_name
                } else {
                    self.closed.map_name
                };
                self.literal_value(*count, name, *args)
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
                // A section that follows another section of the SAME
                // cascade inherits the head's open `?..` context: the
                // official `cascadeExpression_afterTarget` opens ONE
                // `_nullAwareAccess_rightBegin` for the whole cascade and
                // `nullAwareAccess_end` closes it after the LAST section
                // (flow_analysis.dart:6561-6603), so the continuation
                // sections ride the same head short (an out-of-order `?..`
                // the parser reports still belongs to the one context).
                // [`Interp::cascade_head_open`] carries the fact — the
                // open-short count alone cannot tell the head short from
                // an enclosing chain's short (`y?.m(x..a())`).
                let inherited_head = self.cascade_head_open;
                if *null_aware && !inherited_head {
                    // `?..`: official `cascadeExpression_afterTarget` runs
                    // `_nullAwareAccess_rightBegin` before the first
                    // section — the target sits on top of the expression
                    // stack and every section runs on its not-null path.
                    let receiver = self
                        .exprs
                        .last()
                        .map(|e| (e.reference, e.ty, e.args, e.non_null_literal))
                        .unwrap_or((None, Ty::UNKNOWN, crate::ty::EMPTY_ARGS, false));
                    let recv_offset = self
                        .exprs
                        .last()
                        .and_then(|e| e.read_span)
                        .map(|span| span.range.offset)
                        .unwrap_or(0);
                    self.null_short_begin(receiver, recv_offset);
                    // The head's `?..` context ends at the cascade's own
                    // `CascadeEnd` — upstream's direct
                    // `nullAwareAccess_end()` join (resolver.dart:2330), a
                    // non-`flowEnd` close: a dead interval inside a section
                    // keeps running to the statement's `;`, never trimmed
                    // to the section face the selector shorts take.
                    if let Some(short) = self.null_shorts.last_mut() {
                        short.cascade_head = true;
                    }
                    // The target's `ReadType` row re-spells the promoted
                    // face: upstream `cascadeReceiver` reads the target's
                    // promoted static type (`type_analyzer.dart`'s
                    // `cascadeExpression` family), so every section's
                    // receiver verdict re-derives through that row — a
                    // `?..` head over a nullable local leaves `A`, never
                    // `A?`, for the whole cascade extent. Only the flow-type
                    // row moves: a `PromotedRead` here would misreport the
                    // `?..` itself as unnecessary (the official's
                    // `_checkForUnnecessaryNullAware` reads the target's
                    // declared face, which stays `A?` outside the short).
                    if let Some(head) = self.exprs.last()
                        && let Some(var) = head.reference
                        && let Some(span) = head.read_span
                    {
                        let model = self.current.model(var);
                        let promoted = self.promoted_type_of(var, &model);
                        let promoted_args = self.promoted_args_of(var, &model);
                        if promoted != head.ty {
                            self.facts.retain(|fact| {
                                !matches!(
                                    fact,
                                    FlowFact::ReadType { span: s, .. }
                                        | FlowFact::PromotedRead { span: s, .. }
                                        | FlowFact::WhyNotPromoted { span: s, .. }
                                        if *s == span
                                )
                            });
                            self.emit(FlowFact::ReadType {
                                span,
                                ty: promoted,
                                args: promoted_args,
                            });
                        }
                    }
                }
                self.cascade_head_open = *null_aware || inherited_head;
                let target = self
                    .exprs
                    .last()
                    .filter(|_| !self.exprs.is_empty())
                    .cloned()
                    .map(Box::new);
                self.stack.push(Frame::Cascade {
                    exprs_base: self.exprs.len(),
                    // Count AFTER the `?..` push: section-end flushes keep
                    // the head's short open (`null_shorts.len() > base`
                    // pops only what a SECTION opened); `CascadeEnd` joins
                    // it via `own_short` — inherited heads included.
                    null_shorts_base: self.null_shorts.len(),
                    own_short: self.cascade_head_open,
                    target,
                });
            }
            Ev::CascadeAccess { .. } => {
                // The section's value is consumed by the cascade; any
                // null-shorting the SECTION opened (`x..a?.b`) ends with
                // it — the cascade head's own `?..` context (below
                // null_shorts_base) stays open to `CascadeEnd`.
                let (base, exprs_base) = match self.stack.last() {
                    Some(Frame::Cascade {
                        null_shorts_base,
                        exprs_base,
                        ..
                    }) => (*null_shorts_base, *exprs_base),
                    _ => (0, self.exprs.len()),
                };
                while self.null_shorts.len() > base {
                    self.null_short_end();
                }
                // The section head's own property read (official
                // `propertyGet` over the cascade target's temporary
                // reference — the first selector of every section chains
                // on the target, and its promotion slot must exist exactly
                // like a plain `x._field` read's): the head selector rides
                // no DotAccess event (the `..` carries it), so the slot
                // over the target builds here from the head value's own
                // name and read anchor — a later section's same-named head
                // then reads the slot a `!` or null test promoted.
                let head_face = self.exprs.last().and_then(|e| e.name.zip(e.read_span));
                let target_recv = exprs_base
                    .checked_sub(1)
                    .and_then(|idx| self.exprs.get(idx))
                    .and_then(|e| e.prop_receiver());
                if let (Some((field, span)), Some(receiver)) = (head_face, target_recv) {
                    let slot = self.prop_slot(receiver, field);
                    let promoted = self.prop_promoted(receiver, slot);
                    if self.reachable() {
                        if promoted {
                            self.emit(FlowFact::PromotedPropertyRead { span });
                            self.emit_prop_read_type(slot, span);
                        } else {
                            // The non-promoted mirror ([`Ev::DotAccess`]'s
                            // own arm): the slot's cleared promoted types
                            // stand as evidence; a slot never tested or
                            // promoted carries no history and no row.
                            let history = self.current.prop(slot).history;
                            if history != crate::model::NIL {
                                let entries = self.intern_non_promotion(None, history);
                                let prop_receiver = if receiver == THIS_RECEIVER {
                                    PropReceiver::This
                                } else {
                                    PropReceiver::Local(
                                        self.vars[receiver as usize]
                                            .decl_span
                                            .expect("receiver local carries its declaration span"),
                                    )
                                };
                                self.emit(FlowFact::NonPromotedPropertyRead {
                                    span,
                                    name: field,
                                    receiver: prop_receiver,
                                    entries,
                                });
                            }
                        }
                    }
                    // The read's flow type at this layer: the marker pair
                    // (unpromoted = potentially-nullable unknown, promoted
                    // = its non-null face) — the same faces DotAccess's
                    // prop arm writes.
                    if let Some(head) = self.exprs.last_mut() {
                        head.ty = if promoted {
                            Ty::UNKNOWN
                        } else {
                            Ty::UNKNOWN_NULLABLE
                        };
                        head.prop = Some(slot);
                    }
                }
                // The head selector's value stays on the stack when the
                // section continues (`..d?.f()`, `..d[0]`, `..d!`): the
                // continuation selectors chain over the HEAD'S RESULT
                // (the official section is one selector chain —
                // `cascadeExpression`'s sections each start at the target
                // and link through every following selector), so the
                // `?.`/`.`/index receiver is that result, never the
                // cascade target. A section that ends here (the next
                // event is the next section's begin, the cascade's end,
                // or the statement's close) consumes the value as before.
                let section_continues = matches!(
                    events.get(i + 1),
                    Some(
                        Ev::NullAwareMemberBegin
                            | Ev::NullAwareIndexOperandBegin { .. }
                            | Ev::NonNullAssert
                            | Ev::IndexAccess { .. }
                            | Ev::Ident { .. }
                    )
                );
                if !section_continues {
                    self.exprs.pop();
                }
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
                // Another section of the same cascade follows: the head's
                // `?..` context spans it too — only the section-local
                // shorts close here, the head short joins after the LAST
                // section.
                let next_is_section = matches!(events.get(i + 1), Some(Ev::CascadeBegin { .. }));
                if owns_short && !next_is_section {
                    self.cascade_head_open = false;
                }
                match self.stack.pop() {
                    Some(Frame::Cascade {
                        exprs_base, target, ..
                    }) => {
                        // The head `?..` context's skip path rejoins here —
                        // official `nullAwareAccess_end` after the last
                        // section (type_analyzer.dart:2358-2360), so a
                        // section-side `throw`/Never (`x?..[throw '']`)
                        // cannot kill the receiver-was-null path. A
                        // following section keeps the head short open
                        // (the one head context spans the whole cascade).
                        let end_base =
                            base.saturating_sub(usize::from(owns_short && !next_is_section));
                        while self.null_shorts.len() > end_base {
                            self.null_short_end();
                        }
                        // Keep the cascade target as the value.
                        self.exprs.truncate(exprs_base);
                        // A section's write consumed the target entry and
                        // left its assignment value at the base — the
                        // captured target restores the true value face.
                        if let Some(target) = target
                            && self.exprs.len() == exprs_base
                            && exprs_base > 0
                        {
                            self.exprs[exprs_base - 1] = *target;
                        }
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
                // The literal's static type is `String` — the same
                // `matchedType` face `Ev::Bool` spells `bool` for
                // (`Object? s = 'x'` promotes `s` to `Object`: `String <:
                // Object` makes the declared non-null form a type of
                // interest). Without the interned id the face stays
                // unknown — the under-report posture.
                let ty = self
                    .closed
                    .string_name
                    .map(|id| Ty {
                        base: crate::ty::Base::Name(id),
                        nullable: false,
                    })
                    .unwrap_or(Ty::UNKNOWN);
                // A string literal can never evaluate to null — the same
                // guaranteed-non-null face the leaf literals carry.
                let mut expr = Expr::trivial(ty);
                expr.non_null_literal = true;
                self.exprs.push(expr);
            }
            Ev::Spread { token } => {
                // The spread element's own first token (`...`/`...?`): an
                // element reached dead (a dead collection-`for` body, a dead
                // statement) makes the spread element the official's first
                // dead node, so the row anchors at the operator —
                // `{for (; false;) ...dyn}` reports `...dyn};`
                // (dead_code_verifier.dart:183-205). The event is POST-order
                // (the parser fires it after the operand), so the poll is
                // gated on a context that BEGAN dead: an operand that kills
                // the flow itself (`for (;; 1, 2) ...[throw '']`) leaves the
                // deadness to the header's own row (official visit order
                // reaches the updaters before the body), and a live element
                // makes the poll a no-op.
                if self.stmt_began_dead {
                    self.dead_code(*token);
                }
                // The operand's value is consumed; the element leaves its
                // own (trivial) value — every literal element nets exactly
                // one.
                self.flush_null_shorts_above(self.consumer_base(1));
                self.exprs.pop();
                self.exprs.push(Expr::trivial(Ty::UNKNOWN));
            }
            Ev::NullAwareElement => {
                self.flush_null_shorts_above(self.consumer_base(1));
                self.exprs.pop();
                self.exprs.push(Expr::trivial(Ty::UNKNOWN));
            }
            Ev::IfControlFlowBegin { if_kw } => {
                // Collection if-element begin: the parser fires it before
                // the condition streams (the element step opens the `if`
                // at the keyword), so the split waits for the condition's
                // own `ThenControlFlow`. The keyword parks for that arm's
                // dead-entry check — the element's own row anchor.
                self.pending_if_kw = Some(*if_kw);
            }
            Ev::ThenControlFlow { arm_start } => {
                // The collection-if condition ended: rejoin its open
                // null-shorting before the split.
                self.flush_null_shorts_above(self.consumer_base(1));
                let cond = self.pop_value();
                // Collection if-element condition (official
                // `handle_ifElement_conditionEnd`, type_analyzer.dart:1161).
                self.condition_operand(&cond);
                let (if_true, if_false) = self.cond_split(&cond);
                // The if-element's own dead entry — the same
                // pre-condition gate the `?:` face reads above: a
                // began-dead context means the element inherited its
                // deadness (the enclosing statement or arm owns the
                // interval), while a condition that killed the walk itself
                // (`[if (never()) y]` — the condition's own exit) leaves a
                // LIVE parent and the then-element is the official's
                // first dead node.
                let cond_began_dead = self.stmt_began_dead
                    || self.arm_dead_depth > 0
                    || self.dead.is_some();
                let parent_live = !cond_began_dead;
                // The element itself is the official's first dead node
                // when its own condition killed the walk (`[f(n, if (c)
                // y)]` on a Never `n` — `checkUnreachableNode(ifElement)`
                // marks the element, anchored at `if`): the interval opens
                // at the keyword and the arms fold into it exactly like
                // the `?:` conditional's own dead entry.
                if parent_live && !self.reachable() {
                    if let Some(if_kw) = self.pending_if_kw.take() {
                        self.dead_code(if_kw);
                    } else if let Some(head) = cond.read_span.or(cond.callee_span) {
                        self.dead_code(head);
                    }
                } else {
                    self.pending_if_kw = None;
                }
                // The then-element entered dead FROM A LIVE PARENT:
                // interior polls fold into the arm's own close row (the
                // element is the official's first dead node); the count
                // drops at `ElseControlFlow`. A dead-parent element's
                // deadness is inherited — it folds into the enclosing
                // interval and reports nothing of its own.
                let then_dead_entry =
                    parent_live && !self.reach.overall_reachable(if_true.reach);
                if then_dead_entry {
                    self.arm_dead_depth += 1;
                }
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
                    lhs_bool: None,
                });
                self.current = if_true;
                // The element's arm bookkeeping: the then-element runs
                // next, from `arm_start`.
                self.coll_if.push(CollIfFrame {
                    then_start: *arm_start,
                    else_start: None,
                    after_then: None,
                    parent_live,
                    then_dead_entry,
                    else_dead_entry: false,
                });
            }
            Ev::ElseControlFlow {
                then_end,
                else_start,
            } => {
                // The then-element is consumed: its exit state is saved
                // (official `ifStatement_elseBegin`'s
                // `context._afterThen = _current`, flow_analysis.dart:7111-
                // 7115) and a dead then-arm reports its own extent
                // (official `handle_ifElement_thenEnd` → `flowEnd(ifTrue)`,
                // type_analyzer.dart:1163-1166) before the else-element
                // runs under the condition-false branch.
                self.exprs.pop();
                let mut then_dead_entry = false;
                if let Some(frame) = self.coll_if.last_mut() {
                    frame.else_start = Some(*else_start);
                    frame.after_then = Some(self.current.clone());
                    then_dead_entry = frame.then_dead_entry;
                    // The then-element's dead-arm contribution drops at
                    // its close so the row gate reads the outer arms.
                    if then_dead_entry {
                        self.arm_dead_depth = self.arm_dead_depth.saturating_sub(1);
                    }
                }
                if then_dead_entry
                    && let Some(then_start) = self.coll_if.last().map(|f| f.then_start)
                {
                    self.dead_arm_row(then_start, *then_end, then_dead_entry);
                }
                if let Some(Frame::Branch { branch, .. }) = self.stack.last() {
                    // The else-element begins under the condition-false
                    // branch — its own entry deadness latches the same
                    // way the then-element's did (only a live parent makes
                    // it an owned dead arm).
                    let else_dead_entry = !self.reach.overall_reachable(branch.reach)
                        && self
                            .coll_if
                            .last()
                            .is_some_and(|frame| frame.parent_live);
                    if else_dead_entry {
                        self.arm_dead_depth += 1;
                    }
                    if let Some(frame) = self.coll_if.last_mut() {
                        frame.else_dead_entry = else_dead_entry;
                    }
                    self.current = branch.clone();
                }
            }
            Ev::EndIfControlFlow { has_else, arm_end } => {
                let _ = self.exprs.pop();
                // The last arm's `flowEnd` (official
                // `handle_ifElement_thenEnd` at the elseless close /
                // `handle_ifElement_elseEnd`, type_analyzer.dart:1163-1173):
                // a dead arm reports its own extent, one row per arm.
                let coll = self.coll_if.pop();
                if let Some(frame) = &coll {
                    // The closing arm's own dead-arm contribution drops
                    // before the row: the gate reads the outer arms. Only
                    // an arm entered dead FROM A LIVE PARENT reports — a
                    // dead-parent element folds into the enclosing
                    // interval (its deadness is inherited, so
                    // `flowEnd(element)` finds `_firstDeadNode` outside).
                    let arm_dead_entry = if *has_else {
                        frame.else_dead_entry
                    } else {
                        frame.then_dead_entry
                    };
                    if arm_dead_entry {
                        self.arm_dead_depth = self.arm_dead_depth.saturating_sub(1);
                        let anchor = if *has_else {
                            frame.else_start.unwrap_or(frame.then_start)
                        } else {
                            frame.then_start
                        };
                        self.dead_arm_row(anchor, *arm_end, arm_dead_entry);
                    }
                }
                let Some(Frame::Branch { branch, .. }) = self.stack.pop() else {
                    // A frame-less close (unbalanced recovery buffer): the
                    // element still nets one value for its consumer.
                    self.exprs.push(Expr::trivial(Ty::UNKNOWN));
                    return true;
                };
                // The close joins the two arms' exit states (official
                // `ifStatement_end`: `afterThen` saved at elseBegin,
                // `afterElse` = current; the elseless arm joins the then
                // exit with the branch model, flow_analysis.dart:7118-7127).
                let after_then = coll.and_then(|f| f.after_then);
                let joined = if *has_else && let Some(after_then) = after_then {
                    self.join(Some(after_then), Some(self.current.clone()))
                } else {
                    self.join(Some(branch), Some(self.current.clone()))
                };
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
    fn literal_value_keep_ty(&mut self, count: usize) {
        if count > 0 {
            self.exprs
                .truncate(self.exprs.len().saturating_sub(count - 1));
        } else {
            self.exprs.push(Expr::trivial(Ty::UNKNOWN));
        }
        if let Some(head) = self.exprs.last_mut() {
            head.literal = true;
            head.non_null_literal = true;
        }
    }

    /// [`Self::literal_value_keep_ty`]'s collection-literal variant: the
    /// collapsed tail's face is the container's static type — `List`,
    /// `Set`, or `Map` over the literal's written type-argument tree —
    /// never the last element's own (possibly nullable) type
    /// (`[stream.char()]`'s `String?` element must not leak into the
    /// inferred local — UNCHECKED on every later member access). `name`
    /// is the container head's interned id (`None` under a
    /// direct-constructed interp — the literal keeps the opaque unknown
    /// face, the consistent under-report).
    fn literal_value(&mut self, count: usize, name: Option<u32>, args: u32) {
        if count > 0 {
            self.exprs
                .truncate(self.exprs.len().saturating_sub(count - 1));
        } else {
            self.exprs.push(Expr::trivial(Ty::UNKNOWN));
        }
        if let Some(head) = self.exprs.last_mut() {
            head.literal = true;
            head.non_null_literal = true;
            head.ty = name
                .map(|id| Ty {
                    base: crate::ty::Base::Name(id),
                    nullable: false,
                })
                .unwrap_or(Ty::UNKNOWN);
            head.args = args;
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
                self.ops
                    .try_promote_to_typed(
                        tested,
                        tested_args,
                        previous,
                        previous_args,
                        self.args,
                        self.names,
                    )
                    .or_else(|| {
                        self.type_parameter_promotion(previous, tested, tested_args)
                    })
            }
        }
    }

    /// The official `tryPromoteToType`'s type-parameter arm (upstream Dart
    /// SDK source,
    /// pkg/analyzer/lib/src/dart/element/type_system.dart:1754-1768): for a
    /// type variable `T extends U`, a tested type `S` with `S <: U`
    /// promotes the variable to the intersection `T & S` — the analyzer
    /// builds `TypeParameterTypeImpl(promotedBound: S)`, so the read's
    /// static type carries `S`'s members and `S`'s nullability.
    ///
    /// The resolved seam's
    /// [`crate::ty::TypeOps::try_promote_to_typed`] refuses that
    /// step: `S <: T` is false for every proper `S` (a type variable
    /// subtypes nothing below its bound), so the strict-subtype gate sees
    /// no refinement. The declared bound is element data the seam does not
    /// carry — the recorder's [`crate::ty::Boundary`] table does, which is
    /// why the arm lives here beside the boundary-aware nullability faces
    /// ([`Interp::classify`], [`Interp::promote_to_non_null`]).
    ///
    /// This lattice spells no promoted bound, and its chain head replaces
    /// the variable's type outright ([`Interp::promoted_type_of`]), so the
    /// intersection stands as its `S` head — the same spelling the chain
    /// applies to every promoted read. Once the head is a spelled class the
    /// variable is no longer recognisable as a parameter (no boundary row),
    /// and the plain gate owns the question. Shared with the pattern face's
    /// promotion candidate ([`crate::patterns`]).
    pub(crate) fn type_parameter_promotion(
        &self,
        from: Ty,
        tested: Ty,
        tested_args: u32,
    ) -> Option<Ty> {
        let Base::Name(id) = from.base else {
            return None;
        };
        let boundary = self.boundaries.get(id as usize).copied().flatten()?;
        let (bound, bound_args) = boundary.bound;
        // `S <: U` — the official promotion gate (:1756-1757).
        if !self
            .ops
            .is_subtype_typed(tested, tested_args, bound, bound_args, self.args, self.names)
        {
            return None;
        }
        // `isValidPromotionStep` (`flow_analysis.dart:5989-6007`): the
        // intersection `T & S` subtypes `T` by construction, so the step
        // stands unless the variable already satisfies the tested type
        // (`T <: Object` over an `extends Object` bound).
        if self.ops.is_subtype_typed(
            from,
            crate::ty::EMPTY_ARGS,
            tested,
            tested_args,
            self.args,
            self.names,
        ) {
            return None;
        }
        // The intersection's nullability (`type_system.dart:2100-2120`):
        // nullable only when both the variable and the tested type are —
        // a non-nullable variable's `is int?` still narrows to the
        // non-null face.
        Some(if from.nullable && tested.nullable {
            tested
        } else {
            tested.non_null()
        })
    }

    /// Routes a declared type to its consumer: the open variable
    /// declaration's type slot (from `VarDeclBegin` until its first
    /// identifier seals it — #1037), the pending formal, the open
    /// `is`-test, or the open `as`-cast. The written-argument id rides
    /// beside the type (#611 step 4) — every type row passes through
    /// here, so each sink sees the pair as one fact. `written` is the
    /// declared/written marker (official `isImplicitlyTyped` inverted):
    /// `true` from `Ev::Type`/`FunctionType`/`VoidType`, `false` from
    /// `Ev::NoType` — the flag that decides, for the declaration sink,
    /// whether the inferred-face adoption may fill the slot later.
    fn sink_type(&mut self, ty: Ty, args: u32, written: bool) {
        if self.in_decl {
            self.decl.ty = ty;
            self.decl.args = args;
            self.decl.written = written;
        } else if self.in_type_test {
            self.tested_ty = Some(ty);
            self.tested_args = args;
        } else if self.in_as_cast {
            self.as_cast_pending = Some(ty);
            self.as_cast_args = args;
        } else {
            self.pending_ty = Some(ty);
            self.pending_ty_args = args;
            self.pending_ty_written = written;
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
    /// non-nullable — makes the test a constant `false`. The legs split
    /// exactly as upstream: the bottom leg is mode-blind (`isBottomType`
    /// is checked before the sound predicate), the nullability-disjoint
    /// leg is sound-only (`:7611`: `if (!…Enabled) return false` — a
    /// pre-3.9 unit keeps the arm live). The same refusal trio as
    /// [`Interp::is_test_guaranteed`] applies: an unknown/dynamic/void
    /// face on either side is never a dead-branch producer (the
    /// name-level lattice's optimism must not certify the mark).
    fn is_test_guaranteed_fail(&self, operand_ty: Ty, tested: Ty) -> bool {
        if matches!(operand_ty.base, Base::Unknown | Base::Dynamic | Base::Void)
            || matches!(tested.base, Base::Unknown | Base::Dynamic | Base::Void)
        {
            return false;
        }
        self.ops.is_bottom(tested)
            || (self.sound_flow && self.disjoint_by_nullability(operand_ty, tested))
    }

    /// Whether a non-reference operand's type test is guaranteed to
    /// succeed (official `_isTypeCheckGuaranteedToSucceedWithSoundNullSafety`
    /// — upstream Dart SDK source,
    /// pkg/_fe_analyzer_shared/lib/src/flow_analysis/flow_analysis.dart:9149-9155
    /// — `soundFlowAnalysisEnabled && isSubtypeOf(staticType, checkedType)`,
    /// :7645: the whole predicate is sound-only, so a pre-3.9 unit never
    /// folds the test to `true`). The answer deads a branch, so the
    /// boundary refuses: a dynamic tested
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
        if matches!(operand_ty.base, Base::Unknown | Base::Dynamic | Base::Void)
            || matches!(tested.base, Base::Unknown | Base::Dynamic | Base::Void)
        {
            return false;
        }
        self.sound_flow
            && self.ops.is_subtype_typed(
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
        // An is-test whose tested head resolves NOWHERE (a failed-import
        // name — `uriSource is InSummarySource` over the missing
        // package:front_end) is the official's InvalidType operand:
        // subtype.dart's vacuous arm makes the test always-succeed, the
        // failing branch dies, and the operand promotes TO THE TESTED
        // SPELLING (an invalid-typed read whose downstream assignability
        // asks stay vacuous — the oracle's silent argument row). The
        // semantic side's staged filter recognizes the invalid head and
        // skips; here the promotion carries the tested name non-null.
        if let crate::ty::Ty {
            base: crate::ty::Base::Name(name),
            nullable: false,
        } = tested
        {
            let unresolved = self.ops.call_head_unresolved(name, self.names);
            if unresolved {
                // Promotion only: the oracle's flow keeps BOTH arms live
                // over an invalid tested type (no dead_code rows — the
                // R53 regressions over `e is InvalidName` elses), while
                // the static type of the matched read still carries the
                // tested spelling (the argument rows' vacuous checks).
                let matched = self.current.clone().finish_type_test(
                    self.ops,
                    &mut self.lists,
                    var,
                    Some((tested, tested_args)),
                    Some((tested, tested_args)),
                    self.args,
                    self.names,
                );
                let unmatched = self.current.clone();
                return (matched, unmatched);
            }
        }
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
            // official arms it through `ifFalseIsUnreachable`
            // (flow_analysis.dart:3211-3219: `typeIfFalse = null`, then
            // "If not sound, it might still be reachable" — the mark rides
            // `soundFlowAnalysisEnabled`), so a pre-3.9 unit keeps the
            // else live with the promotion chain untouched (the
            // `typeIfFalse = null` posture `current.clone()` carries). The name-level
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
            if self.sound_flow {
                dead
            } else {
                // "If not sound, it might still be reachable"
                // (flow_analysis.dart:3214): the else keeps the pre-test
                // state (`typeIfFalse = null`, no unreachable mark).
                self.current.clone()
            }
        } else {
            // A `FutureOr<R>` remainder `R` (factorType rule 3, the
            // `Future<R> <: what` arm) is the argument node itself: its
            // written-argument tree is the node's own, never the FutureOr
            // list (`List<int>` would otherwise re-read `[List<int>]` as
            // its arguments). Every other remainder keeps `previous`'s.
            let factored_args = match previous.base {
                Base::Name(id) if self.closed.future_or == Some(id) => self
                    .args
                    .get(previous_args as usize)
                    .and_then(|row| row.first())
                    .filter(|arg| arg.base == factored.base && factored.base != previous.base)
                    .map_or(previous_args, |arg| arg.args),
                _ => previous_args,
            };
            // Official `isValidPromotionStep`
            // (flow_analysis.dart:5989-6003): the factor output is a
            // subtype of `previous` by construction, so the sound-mode
            // step is valid exactly when `previous` is not a subtype of
            // it (no mutual subtypes, no repeats); disabled mode refuses
            // only a repeated promotion to the same type
            // (`newType != previousType`). The instantiated face decides —
            // `factor` keeps `previous`'s written-argument tree, which the
            // typed compare reads.
            let factored = if self.sound_flow {
                if self.ops.is_subtype_typed(
                    previous,
                    previous_args,
                    factored,
                    factored_args,
                    self.args,
                    self.names,
                ) {
                    None
                } else {
                    Some(factored)
                }
            } else if (factored, factored_args) == (previous, previous_args) {
                None
            } else {
                Some(factored)
            };
            self.current.clone().finish_type_test(
                self.ops,
                &mut self.lists,
                var,
                Some((tested, tested_args)),
                factored.map(|ty| (ty, factored_args)),
                self.args,
                self.names,
            )
        };
        (matched, unmatched)
    }

    /// The property slot's promotion-chain head with its written-argument
    /// tree — the `previous` of upstream's `_PropertyReference._type`
    /// (the promoted type when one stands, the member's static type
    /// otherwise). The member's declared type is element data this layer
    /// deliberately does not carry, so an unpromoted slot answers the
    /// potentially-nullable unknown marker — the same face the read
    /// itself wears ([`Expr::prop`]'s marker pair).
    fn prop_previous(&self, slot: u32) -> (Ty, u32) {
        let model = self.current.prop(slot);
        if model.promoted == crate::model::NIL {
            (Ty::UNKNOWN_NULLABLE, crate::ty::EMPTY_ARGS)
        } else {
            (
                self.lists.head(model.promoted),
                self.lists.head_args(model.promoted),
            )
        }
    }

    /// The promotion target for a type check on a property slot — the
    /// `_PropertyReference` arm of upstream `tryPromoteForTypeCheck`'s
    /// true branch (`flow_analysis.dart:4137-4151`), the same
    /// [`TypeOps::try_promote_to_typed`] gate the variable candidate runs
    /// (`to <: from` on the instantiated spellings). `from` is the slot's
    /// standing head: the marker pair the read face carries — a spelled
    /// prior promotion re-decides under the real name, and the
    /// undecidable cross-base question over the unknown marker keeps the
    /// lattice's optimistic answer (the official's `dynamic`-previous
    /// promotion).
    fn prop_promotion_candidate(&mut self, slot: u32, tested: Ty, tested_args: u32) -> Option<Ty> {
        match tested.base {
            Base::Unknown | Base::Dynamic | Base::Void => None,
            _ => {
                let (previous, previous_args) = self.prop_previous(slot);
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

    /// The promoted-property read's flow-type row (#3430): when the
    /// slot's standing chain head spells a real type (a type test's
    /// gated candidate — `x.f is B` publishes `B`, never the non-null
    /// marker), the read joins the [`FlowFact::ReadType`] table at its
    /// member token exactly like a promoted variable read — the
    /// receiver-expression face every downstream consumer re-derives
    /// (`recv_expression_ty`/`read_type_at` answer `B` for `x.f`, so a
    /// chained `x.f.m` resolves `m` on the promoted type). A marker
    /// promotion (the `!= null`/`!` face's opaque head) spells nothing
    /// the consumer cannot already see — no row.
    fn emit_prop_read_type(&mut self, slot: u32, span: TokenSpan) {
        let model = self.current.prop(slot);
        if model.promoted == crate::model::NIL {
            return;
        }
        let ty = self.lists.head(model.promoted);
        if !matches!(ty.base, Base::Name(_) | Base::Function) {
            return;
        }
        self.emit(FlowFact::ReadType {
            span,
            ty,
            args: self.lists.head_args(model.promoted),
        });
    }

    /// A successful type test on a property slot: returns (matched,
    /// unmatched) states — the [`Interp::branch_type_test`] shape reached
    /// through [`State::finish_prop_type_test`] on the slot's model. The
    /// matched arm promotes to the gated candidate; the unmatched arm
    /// promotes to the factored remainder — shape-preserving at this
    /// seam, so the remainder keeps the previous chain head's rows — and
    /// a bottom remainder marks it unreachable (the official
    /// `ifFalseIsUnreachable` arm). The invalid-tested shortcut of the
    /// variable face does not repeat: `IsEnd` decides the invalid face
    /// before calling here.
    fn branch_prop_type_test(
        &mut self,
        slot: u32,
        tested: Ty,
        tested_args: u32,
        promoted: Option<Ty>,
    ) -> (State, State) {
        let (previous, previous_args) = self.prop_previous(slot);
        let matched = self.current.clone().finish_prop_type_test(
            self.ops,
            &mut self.lists,
            slot,
            Some((tested, tested_args)),
            promoted.map(|ty| (ty, tested_args)),
            self.args,
            self.names,
        );
        let factored = self.ops.factor_typed(
            previous,
            previous_args,
            tested,
            tested_args,
            self.args,
            self.names,
        );
        let unmatched = if self.ops.is_bottom(factored) {
            // The bottom-remainder arm: `x.f is! T` over a slot already
            // proved `T` leaves nothing below (the official
            // `ifFalseIsUnreachable`, sound-mode only — a pre-3.9 unit
            // keeps the else live with the chain untouched).
            if self.sound_flow {
                self.current.clone().set_unreachable(&mut self.reach)
            } else {
                self.current.clone()
            }
        } else {
            // Official `isValidPromotionStep` on the remainder, the same
            // arm [`Interp::branch_type_test`] runs.
            let factored_args = previous_args;
            let factored = if self.sound_flow {
                if self.ops.is_subtype_typed(
                    previous,
                    previous_args,
                    factored,
                    factored_args,
                    self.args,
                    self.names,
                ) {
                    None
                } else {
                    Some(factored)
                }
            } else if (factored, factored_args) == (previous, previous_args) {
                None
            } else {
                Some(factored)
            };
            self.current.clone().finish_prop_type_test(
                self.ops,
                &mut self.lists,
                slot,
                Some((tested, tested_args)),
                factored.map(|ty| (ty, factored_args)),
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
        // The signature-less callable head is not a VALUE type. A call
        // whose return the lattice cannot spell keeps the callee's
        // [`Base::Function`] face, and that face's non-nullability
        // describes the CALLABLE, never the value the call answers — so no
        // comparison may fold on it. Reading it as a non-nullable value
        // folded `null == nf()` over a `Null Function()` parameter to
        // `false` and reported the dead THEN branch where the oracle
        // reports the dead ELSE one, moving the row's whole extent: our
        // `389:33|48` (the then block's braces) against the oracle's
        // `391:12|55` (the else block's) on the sound-flow equality
        // corpus. Both branches stay reachable, the under-report every
        // unspellable return keeps.
        let value_non_null = |e: &Expr| {
            e.ty.base != Base::Function && self.classify(e.ty) == Classification::NonNullable
        };
        let (left_null, right_null) = (null_ty(left.ty), null_ty(right.ty));
        if left_null && right_null {
            // Known equal: `null == null` kills the else-branch, `null
            // != null` the then-branch.
            expr.bool_value = Some(!inverted);
            expr.branches = None;
        } else if self.sound_flow
            && ((left_null && value_non_null(right))
                || (right_null && value_non_null(left)))
        {
            // Disjoint by nullability: known not-equal — `==` kills the
            // then-branch, `!=` the else-branch. The verdict is
            // `_equalityCheck`'s gated arm
            // (flow_analysis.dart:7241-7258): `_GuaranteedNotEqual` under
            // sound flow, `_NoEqualityInformation` otherwise — the same
            // shared function the constant-pattern face rides.
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
                let (promoted, promoted_args) = self.promote_to_non_null(operand.ty, operand.args);
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
                    expr.branches = Some((if_true.clone(), if_false.clone()));
                    expr.test_split = true;
                    self.cond_split_var = Some(var);
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
        // `==`/`!=` is a boolean — non-null by construction.
        expr.non_null_literal = true;
        self.exprs.push(expr);
    }
}
