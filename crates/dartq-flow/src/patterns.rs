//! Pattern events: alternative patterns for cases, if-case, and pattern
//! variable declarations.

use crate::ev::{Ev, PatternShape};
use crate::interp::{Expr, Frame, Interp, PatternKind, PatternState, Resolved, VarMeta};
use crate::ty::{Base, Classification, Ty};
use dartq_parser::TokenSpan;

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
                Ev::PatternBegin { .. }
                    | Ev::PatternEnd { .. }
                    | Ev::ObjectPattern { .. }
                    | Ev::ShapePattern { .. }
                    | Ev::NullCheckPattern
                    | Ev::BinaryOrPatternBegin { .. }
                    | Ev::BinaryOrPatternEnd { .. }
                    | Ev::GuardBegin
                    | Ev::GuardEnd
            )
        {
            return;
        }
        match ev {
            Ev::ObjectPattern { name, args } => {
                self.matched_object_pattern(*name, *args, object_fields_irrefutable(_events, _i));
            }
            Ev::BinaryOrPatternBegin { operator } => {
                self.or_frame_begin(*operator);
                self.or_pattern_begin();
            }
            Ev::BinaryOrPatternEnd { end } => {
                self.or_pattern_end();
                self.or_frame_end(*end);
            }
            Ev::PatternBegin { paren, .. } => {
                // A parenthesized wrapper is grouping-transparent: no
                // depth, no context open, no cannot-fail evidence (the
                // wrapped real node supplies all three).
                if *paren {
                    return;
                }
                // The clause head parsed a real pattern node: the
                // cannot-fail kill's evidence gate (a recovered clause
                // closes its context with zero of these).
                if let Some(pattern) = self.pattern.as_mut() {
                    pattern.saw_node = true;
                }
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
                        // The scrutinee expression ends first: the
                        // null-shorting its own `?.` opened rejoins before
                        // the pattern context is pushed (official
                        // `finishNullShorting` at the scrutinee boundary), so
                        // the receiver's non-null arm never leaks into the
                        // unmatched chain. Only a completed shorted chain
                        // row (`chain_base`) flushes: a declaration or
                        // assignment pattern opens before its value, and
                        // the row beneath it belongs to an enclosing
                        // expression whose short must stay open.
                        if self.exprs.last().is_some_and(|e| e.chain_base.is_some()) {
                            self.flush_null_shorts_above(self.consumer_base(1));
                        }
                        self.split_cur();
                        // The scrutinee face PEEKS, never pops: an if-case's
                        // scrutinee expression sits on the stack, but a
                        // declaration, assignment, or for-in head pattern
                        // opens before its own value parses — the value
                        // beneath belongs to an enclosing construct (a
                        // sibling collection element inside `[0, for (var
                        // (int i) = 1; ...)]`) and consuming it here
                        // corrupts that construct's value accounting. The
                        // pending value is consumed only when an if-case
                        // adopts the context (`if_case_condition` /
                        // `ThenBegin`).
                        let peeked = self.exprs.last().cloned();
                        let scrutinee = peeked.as_ref().and_then(|e| e.reference);
                        let scrutinee_ty = peeked.as_ref().map(|e| (e.ty, e.args));
                        let unmatched = self.current.clone().set_unreachable(&mut self.reach);
                        self.vacuous_object = None;
                        self.vacuous_fields.clear();
                        self.pattern_binds.clear();
                        self.or_join_epoch = 0;
                        self.or_last_clean = None;
                        self.pattern = Some(PatternState {
                            kind: PatternKind::Decl,
                            unmatched,
                            scrutinee,
                            scrutinee_ty,
                            scrutinee_pending: peeked.is_some(),
                            guard: None,
                            pre_guard: None,
                            pending_decls: Vec::new(),
                            shadowed: Vec::new(),
                            cannot_fail: true,
                            saw_node: false,
                        });
                    }
                }
            }
            Ev::PatternEnd { paren } => {
                if *paren {
                    return;
                }
                self.pattern_depth = self.pattern_depth.saturating_sub(1);
            }
            Ev::ConstantPatternEnd => {
                let constant = self.exprs.pop();
                // Only a TOP-LEVEL constant pattern is an equality check on
                // the matched value — a field's constant sub-pattern
                // (`case Point(x: 5)`) constrains that field, not the
                // scrutinee (the same depth gate
                // [`Interp::matched_object_pattern`] uses). The narrowing
                // arms stay top-level-only, but the MAY-FAIL side effect
                // rides at every depth: upstream's `constantPattern_end`
                // runs for a nested constant too and its
                // `_NoEqualityInformation` arm joins the matched state into
                // the enclosing context's unmatched chain
                // (`_unmatched = _join(_unmatched!, _current)`,
                // flow_analysis.dart:7470-7475) — a constant can fail on
                // its own value, so the enclosing pattern's cannot-fail
                // posture stands down and a `||` right operand over it
                // (`King(suit: E.a || E.b)`, `A && 1 || 2`) stays reachable.
                // Skipping the join left the alternation frame reading the
                // left operand as clean and over-reporting the right side
                // dead over enum/constant alternations.
                if self.pattern_depth != 1 {
                    self.join_unmatched(self.current.clone());
                    return;
                }
                let Some(constant) = constant else {
                    self.stand_down_may_fail();
                    return;
                };
                // A NON-reference matched value's equality check joins the
                // chain upstream all the same (`promoteForEquality` keys on
                // the matched value's TYPE, not its shape) — the flow
                // lattice cannot answer it without a promotable reference,
                // so the undecided face keeps the may-fail posture (the
                // kill stands down — `case 1:` over `b ? 1 : 2` keeps the
                // later members live).
                let Some(var) = self.pattern_scrutinee() else {
                    self.stand_down_may_fail();
                    return;
                };
                // Official `constantPattern_end` →
                // `_handleEqualityCheckPattern` (flow_analysis.dart:8866-8948):
                // the implicit `== operand` runs `_equalityCheck` on
                // (matched value type, constant type).
                //
                // A nullability-disjoint pair answers per
                // `_equalityCheck`'s disjoint arm
                // (flow_analysis.dart:7241-7258): `_GuaranteedNotEqual`
                // under sound flow — the match is guaranteed to fail, so
                // the live matched state folds into the unmatched chain
                // and `current` goes unreachable; the case body runs dead
                // and anchors its row at the first body statement.
                // Disabled mode answers `_NoEqualityInformation` ("in
                // weak mode it might produce an equal result") — the
                // disjoint arm sits BEFORE the null-check arm upstream,
                // so even a Null constant over a non-nullable matched
                // value takes the plain may-fail join below, never the
                // null-check face.
                let (matched, matched_args) = {
                    let model = self.current.model(var);
                    (
                        self.promoted_type_of(var, &model),
                        self.promoted_args_of(var, &model),
                    )
                };
                let disjoint = self.disjoint_by_nullability(matched, constant.ty);
                if disjoint && self.sound_flow {
                    let cur = self.current.clone();
                    self.note_may_fail(&cur);
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
                    // semantic layer's case-body row owns the report — a
                    // SWITCH owner only: an if-case or standalone pattern
                    // context has no semantic switch tracker behind it
                    // (`if (x case null) {}` over a non-nullable `x`
                    // folds its then-branch dead here, the oracle's
                    // empty-then-block row), so the kill stands down
                    // exclusively for the two switch kinds.
                    let declared = self
                        .vars
                        .get(var as usize)
                        .map_or(Ty::UNKNOWN, |meta| meta.ty);
                    let semantic_owns = self.disjoint_by_nullability(declared, constant.ty)
                        && matches!(
                            self.pattern.as_ref().map(|p| p.kind),
                            Some(PatternKind::Switch | PatternKind::SwitchExpr)
                        );
                    if !semantic_owns {
                        self.unreachable_cur();
                        if matches!(
                            self.pattern.as_ref().map(|p| p.kind),
                            Some(PatternKind::IfCase | PatternKind::Decl)
                        ) {
                            // The then branch's empty block is itself a
                            // polled node under this fold (the branch marker
                            // the `IfBegin` split reads). The context still
                            // carries the pre-adoption `Decl` kind here — the
                            // `IfCaseCondition` marker flips it a step later
                            // — so `Decl` marks too: a declaration context
                            // never feeds an if branch, and the next `IfBegin`
                            // resets the marker.
                            self.if_branch_folded = true;
                        }
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
                // The face is mode-blind upstream (`_EqualityCheckIsNullCheck`
                // rides the literal's null info, past the gated disjoint
                // arm), and stays reachable only when the pair is NOT
                // nullability-disjoint.
                if !disjoint && self.classify(constant.ty) == Classification::NullOrEquivalent {
                    let (non_null, non_null_args) = self.promote_to_non_null(matched, matched_args);
                    let mut if_not_null = if non_null != matched {
                        self.current.clone().finish_type_test(
                            self.ops,
                            &mut self.lists,
                            var,
                            None,
                            Some((non_null, non_null_args)),
                            self.args,
                            self.names,
                        )
                    } else {
                        self.current.clone()
                    };
                    if self.classify(matched) == Classification::NullOrEquivalent {
                        if_not_null = if_not_null.set_unreachable(&mut self.reach);
                    }
                    // A switch clause's chain starts live (the clause-begin
                    // state) where the official `_pushPattern` starts it
                    // unreachable. The top-level constant IS the whole
                    // clause pattern, so no other arm joined yet: the
                    // official join collapses to `ifNotNull` itself —
                    // joining the live pre-test state would demote the
                    // fall-through (`case null: return; default: x.p`).
                    if matches!(
                        self.pattern.as_ref().map(|p| p.kind),
                        Some(PatternKind::Switch | PatternKind::SwitchExpr)
                    ) {
                        self.note_may_fail(&if_not_null);
                        if let Some(pattern) = self.pattern.as_mut() {
                            pattern.unmatched = if_not_null;
                        }
                        return;
                    }
                    self.note_may_fail(&if_not_null);
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
                self.note_may_fail(&cur);
                if let Some(chain) = self.pattern.as_ref().map(|p| p.unmatched.clone()) {
                    let joined = self.join(Some(chain), Some(cur));
                    if let Some(pattern) = self.pattern.as_mut() {
                        pattern.unmatched = joined;
                    }
                }
            }
            Ev::ShapePattern { shape } => {
                // A top-level record/list/map/relational pattern can fail
                // (the shape or comparison decides): the failure arm joins
                // the pattern context's `unmatched` chain. Without the join
                // the chain stays at its initial unreachable state and the
                // code after `if (v case (a, b))` folds dead — the official
                // `_unmatched` accumulates every refutable shape's failure
                // arm (flow_analysis.dart's pattern dispatch). Nested
                // instances constrain their own field, not the scrutinee —
                // but under a vacuous enclosing head (an exact scrutinee
                // type) the nested failure is the ONLY unmatched path, so
                // every depth joins (the pre-test state is the
                // nested-failure posture for the else arm; when the
                // enclosing head itself is refutable its own join already
                // carried the same state and the LUB is unchanged).
                if !self.collecting()
                    && let Some(chain) = self.pattern.as_ref().map(|p| p.unmatched.clone())
                {
                    // The cannot-fail exemption (official `promoteForPattern`
                    // flags, type_analyzer.dart:1294-1309): an empty map
                    // pattern under sound flow analysis cannot fail once the
                    // type test passes — `matchMayFailEvenIfCorrectType`
                    // latches false — so a covered matched type joins
                    // nothing and the following alternatives of the switch
                    // read dead. Every other shape (non-empty map, list,
                    // record, relational) keeps its may-fail join.
                    if self.pattern_depth == 1
                        && matches!(shape, PatternShape::Map { entries: 0 })
                        && self.sound_flow
                        && self.shape_head_covers()
                    {
                        return;
                    }
                    let cur = self.current.clone();
                    self.note_may_fail(&cur);
                    let joined = self.join(Some(chain), Some(cur));
                    if let Some(pattern) = self.pattern.as_mut() {
                        pattern.unmatched = joined;
                    }
                }
            }
            Ev::NullCheckPattern => {
                // `sub?` at the top level: the matched arm promotes the
                // matched value non-null (official `_handleNullCheckPattern`
                // → `_handleNullTestPattern(matchesIfNull: false)`), the
                // failure arm — the null path — joins `unmatched`.
                //
                // Depth `> 1` rejects, `<= 1` accepts: a bare clause head
                // (`case _?`) holds its own begin open here (depth 1), a
                // paren-led one has the wrapper flagged transparent (depth
                // 0) — both are the clause's spine; a field's sub-pattern
                // (`case Point(x: _?)`) sits deeper.
                if self.pattern_depth > 1 {
                    return;
                }
                let scrutinee_var = self.pattern_scrutinee();
                let scrutinee_expr = self.pattern.as_ref().and_then(|p| p.scrutinee_ty);
                let (non_null, non_null_args) = match scrutinee_var {
                    Some(var) => {
                        let model = self.current.model(var);
                        let (matched, matched_args) = (
                            self.promoted_type_of(var, &model),
                            self.promoted_args_of(var, &model),
                        );
                        // A matched value already known non-null is
                        // guaranteed to match: the official promotes nothing
                        // and leaves `_unmatched` untouched (the arm cannot
                        // fall through). The guarantee is sound-only
                        // (`nullCheckOrAssertPattern_begin`,
                        // flow_analysis.dart:6223-6237: "the pattern might
                        // not match … because sound flow analysis is
                        // disabled") — a pre-3.9 unit keeps the null path.
                        let guaranteed = self.sound_flow
                            && self.classify(matched) == Classification::NonNullable;
                        let (non_null, non_null_args) =
                            self.promote_to_non_null(matched, matched_args);
                        if !guaranteed {
                            // Official order matters here: `_unmatched` joins
                            // the state BEFORE the promotion (the matched
                            // value is still nullable on the failure arm),
                            // and only then does `_setCurrent(ifNotNull)`
                            // replace `current`. Joining the promoted state
                            // instead would carry the non-null face into the
                            // next clause's entry state and kill a later
                            // `case null:` arm
                            // (the riverpod `notifier_provider.dart` shape).
                            let before = self.current.clone();
                            self.current = self.current.clone().finish_type_test(
                                self.ops,
                                &mut self.lists,
                                var,
                                None,
                                Some((non_null, non_null_args)),
                                self.args,
                                self.names,
                            );
                            self.join_unmatched(before);
                        }
                        (non_null, non_null_args)
                    }
                    // A NON-variable matched value (`transformer?.value case
                    // final transformer?`): no promotable reference exists,
                    // so the null path joins `unmatched` with the state as
                    // it stands (official `_handleNullTestPattern`'s failure
                    // arm over an unpromotable value) and the matched state
                    // keeps itself — only the declared binder cares about
                    // the face, which the expression's own type supplies.
                    None => {
                        let Some((ty, args)) = scrutinee_expr else {
                            return;
                        };
                        // The same sound-only guarantee as the reference
                        // arm (`nullCheckOrAssertPattern_begin`,
                        // flow_analysis.dart:6223-6237).
                        let guaranteed =
                            self.sound_flow && self.classify(ty) == Classification::NonNullable;
                        if !guaranteed {
                            let before = self.current.clone();
                            self.join_unmatched(before);
                        }
                        self.promote_to_non_null(ty, args)
                    }
                };
                // The declared variable beside the null check (`case var
                // x?`) is declared untyped (`NoType` leaves
                // [`Ty::UNKNOWN`]) by the event that precedes this one;
                // official `_handleNullCheckPattern` hands the
                // sub-pattern the non-null matched type, and
                // `DeclaredVariablePattern` declares the variable with
                // it. An explicit annotation keeps its own type
                // (`declare` prefers the written one). A guaranteed match
                // stores the matched type as it stands, already non-null.
                // Both scrutinee shapes reach here: the variable path's
                // promoted face and the expression path's own non-null
                // type (`transformer?.value case final t?` binds `t` to
                // the chain's non-null element type).
                if let Some((name, _)) = self
                    .pattern
                    .as_ref()
                    .and_then(|p| p.pending_decls.last().copied())
                    && let Some(Resolved::Local(child)) = self.resolve(name)
                    && let Some(meta) = self.vars.get_mut(child as usize)
                    && meta.ty == Ty::UNKNOWN
                {
                    meta.ty = non_null;
                    meta.args = non_null_args;
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
                let declared = self.pending_ty.take().unwrap_or(Ty::UNKNOWN);
                let declared_args = std::mem::take(&mut self.pending_ty_args);
                // The shorthand binder's staged face (#2567): the semantic
                // layer resolved the property's declared return — a bare
                // `var x` binder in an object pattern adopts it exactly
                // like an inferred initializer's slot.
                let staged = declared == Ty::UNKNOWN
                    && self.inferred_locals.get(&token.range.offset).is_some();
                let (declared, declared_args) = if declared == Ty::UNKNOWN {
                    self.inferred_locals
                        .get(&token.range.offset)
                        .copied()
                        .map_or((declared, declared_args), |(ty, args)| (ty, args))
                } else {
                    (declared, declared_args)
                };
                // The staged binder cannot fail on its own field: the
                // adopted type IS the property's resolved return, so the
                // official field promote answers `coversMatchedType` (the
                // matched value's type sits under the declared one) and
                // joins nothing — the `||`-operand row over the
                // alternation beside it still anchors
                // (`Rectangle(:var areaAsInt || var areaAsInt)` over a
                // `Shape` scrutinee).
                let written = std::mem::take(&mut self.pending_ty_written);
                let meta = VarMeta {
                    ty: declared,
                    args: declared_args,
                    late: false,
                    is_final: *is_final,
                    written,
                    decl_span: Some(token.range),
                    // Pattern binders never carry a proven constant: the
                    // matched value is a runtime extraction, not a fold.
                    const_bool: None,
                    // Nor a by-construction non-null value: the matched
                    // value comes from the scrutinee's runtime rows.
                    non_null: false,
                };
                let shadowed = self.bound_var(*name);
                // The stacked-head alias (official `switchStatement_
                // endAlternative`'s copyPromotionData, flow_analysis.dart
                // :6660-6686): a later head of the same shared-body group
                // that re-declares a name an earlier head bound does not
                // mint a second variable — upstream keys every head's
                // binder of the name to the FIRST head's promotion key and
                // copies the later head's model into it, so the shared
                // body's reads and writes address ONE variable whose model
                // joins assigned across every matched head. Aliasing at
                // the binding keeps the same observable: the name stays
                // bound to the first binder and this head's matched path
                // declares it initialized exactly like the first head's
                // did. Without it, the body's reads would resolve to the
                // LAST head's binder — fresh-unassigned on the earlier
                // heads' matched paths — and a spurious potentially-
                // unassigned row would fire over a body upstream reads as
                // assigned. Gate: a switch-statement case head's own
                // binder (head region open, no guard — a body's pattern
                // declarations see the registry cleared, and a guard's
                // interior never declares), and the current binding is the
                // registry's own variable.
                let aliased = if self.case_head_depth > 0 && self.guard_depth == 0 {
                    self.case_group_binders
                        .get(name)
                        .copied()
                        .filter(|&first| shadowed == Some(first))
                } else {
                    None
                };
                if let Some(first) = aliased {
                    self.current = self.current.clone().declare(first, true);
                } else {
                    let var = self.declare_var(*name, meta, true);
                    if self.case_head_depth > 0 && self.guard_depth == 0 {
                        self.case_group_binders.insert(*name, var);
                    }
                    if let Some(pattern) = self.pattern.as_mut() {
                        pattern.shadowed.push((*name, shadowed));
                    }
                }
                if let Some(pattern) = self.pattern.as_mut() {
                    pattern.pending_decls.push((*name, *is_final));
                }
                // The pattern's bind buffer: the event stream is
                // post-order, so a property sub-pattern's shorthand bind
                // arrives BEFORE its object pattern's own event — the
                // defer snapshots the binds seen so far.
                self.pattern_binds.push(*name);
                // A typed declared-variable pattern at the top level IS the
                // pattern's type test (`case int n` — official
                // `promoteForPattern` over `analyzeDeclaredVariablePattern`):
                // the matched arm promotes the matched value, the failure
                // arm joins `unmatched` when the test can fail. `var x`
                // (no written type) always matches and never joins.
                if !self.collecting() && self.pattern_depth == 1 && declared != Ty::UNKNOWN {
                    if self.pattern_known_invalid(declared, declared_args) {
                        return;
                    }
                    if let Some(var) = self.pattern_scrutinee() {
                        let model = self.current.model(var);
                        let (matched_ty, matched_args) = (
                            self.promoted_type_of(var, &model),
                            self.promoted_args_of(var, &model),
                        );
                        let candidate =
                            self.pattern_promotion_candidate(var, declared, declared_args);
                        let (matched_arm, unmatched_arm) =
                            self.branch_type_test(var, declared, declared_args, candidate);
                        self.current = matched_arm;
                        if !self.ops.is_subtype_typed(
                            matched_ty,
                            matched_args,
                            declared,
                            declared_args,
                            self.args,
                            self.names,
                        ) {
                            self.join_unmatched(unmatched_arm);
                        } else if self.ops.is_invalid_typed(
                            matched_ty,
                            matched_args,
                            self.args,
                            self.names,
                        ) {
                            // The vacuous cover (see
                            // [`Interp::vacuous_cover_kills_chain`]): the
                            // matched type is the invalid face, so the
                            // cannot-fail verdict rides the 0c arm.
                            self.vacuous_cover_kills_chain();
                        }
                    } else {
                        self.non_var_invalid_scrutinee_covers(declared, declared_args);
                    }
                } else if !self.collecting()
                    && !staged
                    && self.pattern_depth > 1
                    && declared != Ty::UNKNOWN
                {
                    // A typed sub-pattern nested in a container field tests
                    // its own property and can fail on that value alone.
                    self.nested_typed_join();
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
                // A typed wildcard (`num _`) declares nothing but still runs
                // the pattern's type test at the top level: the matched arm
                // promotes the matched value, the failure arm joins
                // `unmatched` when the test can fail (official
                // `analyzeWildcardPattern` → the same `promoteForPattern`
                // the declared-variable face shares). An untyped `_` always
                // matches — its annotation-less form drops `pending_ty`
                // untouched. The test is a flow-walk act, skipped under a
                // collect pre-pass (the rollback does not cover `pattern`).
                let declared = self.pending_ty.take().unwrap_or(Ty::UNKNOWN);
                let declared_args = std::mem::take(&mut self.pending_ty_args);
                self.pending_ty_written = false;
                // An untyped `_` at depth 1 leaves the case's unmatched
                // chain to the semantic layer's chain-start emission — the
                // flow kill here would duplicate those rows through the
                // per-statement dead facts.
                if !self.collecting() && self.pattern_depth == 1 && declared != Ty::UNKNOWN {
                    if self.pattern_known_invalid(declared, declared_args) {
                        return;
                    }
                    if let Some(var) = self.pattern_scrutinee() {
                        let model = self.current.model(var);
                        let (matched_ty, matched_args) = (
                            self.promoted_type_of(var, &model),
                            self.promoted_args_of(var, &model),
                        );
                        let candidate =
                            self.pattern_promotion_candidate(var, declared, declared_args);
                        let (matched_arm, unmatched_arm) =
                            self.branch_type_test(var, declared, declared_args, candidate);
                        self.current = matched_arm;
                        if !self.ops.is_subtype_typed(
                            matched_ty,
                            matched_args,
                            declared,
                            declared_args,
                            self.args,
                            self.names,
                        ) {
                            self.join_unmatched(unmatched_arm);
                        } else if self.ops.is_invalid_typed(
                            matched_ty,
                            matched_args,
                            self.args,
                            self.names,
                        ) {
                            // The vacuous cover (see
                            // [`Interp::vacuous_cover_kills_chain`]): the
                            // matched type is the invalid face, so the
                            // cannot-fail verdict rides the 0c arm.
                            self.vacuous_cover_kills_chain();
                        }
                    } else {
                        self.non_var_invalid_scrutinee_covers(declared, declared_args);
                    }
                }
            }
            Ev::GuardBegin => {
                // The guard's interior events are expression events — they
                // must not read as the head region's end (the dispatcher's
                // group-boundary clear checks this depth).
                self.guard_depth += 1;
                // Snapshot the pre-guard state: the guard's own events run
                // on `current` next and leave it on the guard-true arm, so
                // this snapshot is the guard-false arm for the close's
                // unmatched join.
                if let Some(pattern) = self.pattern.as_mut() {
                    pattern.pre_guard = Some(self.current.clone());
                }
            }
            Ev::GuardEnd => {
                self.guard_depth = self.guard_depth.saturating_sub(1);
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
    /// A logical-or alternation opens (`A() || ...` at the top level of
    /// a refutable pattern): latch the scrutinee's promoted face — the
    /// LEFT branch's parse already promoted it, and the close needs the
    /// pre-right face to tell a differing right promotion from an
    /// identical one.
    fn or_pattern_begin(&mut self) {
        self.or_pattern_latch = None;
        self.or_operand_depth = None;
        self.or_right_face = None;
        if self.pattern_depth != 1 {
            return;
        }
        // The right operand's own pattern node parses one level deeper;
        // its object pattern still constrains the scrutinee.
        self.or_operand_depth = Some(self.pattern_depth + 1);
        let Some(pattern) = self.pattern.as_ref() else {
            return;
        };
        let Some(var) = pattern.scrutinee else {
            return;
        };
        let model = self.current.model(var);
        let ty = self.promoted_type_of(var, &model);
        let args = self.promoted_args_of(var, &model);
        self.or_pattern_latch = Some((var, ty, args));
    }

    /// The alternation closes: the right branch's promotion decides. An
    /// unchanged face against the open's latch keeps the shared
    /// promotion (`A() || A()`); a changed face is the union of two
    /// distinct promotions, which the name-level model cannot spell —
    /// the pre-pattern face restores (official
    /// `analyzeLogicalOrPattern` joins the branches' promotions, and a
    /// join of two distinct faces is no single promotion).
    fn or_pattern_end(&mut self) {
        self.or_operand_depth = None;
        let (var, lty, largs) = match self.or_pattern_latch.take() {
            Some(latch) => latch,
            None => {
                self.or_right_face = None;
                return;
            }
        };
        let Some(pattern) = self.pattern.as_ref() else {
            self.or_right_face = None;
            return;
        };
        let entry = pattern.unmatched.model(var);
        let entry_face = (
            self.promoted_type_of(var, &entry),
            self.promoted_args_of(var, &entry),
        );
        // The join: both branches must promote, and to the same face —
        // anything else is a union the name level cannot spell, and the
        // pre-pattern face restores.
        let left = (entry_face != (lty, largs)).then_some((lty, largs));
        let right = self
            .or_right_face
            .flatten()
            .filter(|face| *face != entry_face);
        let joined = left.is_some() && right == left;
        self.or_right_face = None;
        if !joined {
            let mut now_state = self.current.clone();
            now_state = now_state.with_model(var, entry);
            self.current = now_state;
        }
    }

    pub(crate) fn matched_object_pattern(
        &mut self,
        name: u32,
        args: u32,
        fields_irrefutable: bool,
    ) {
        // A top-level pattern constrains the matched value; a field's
        // sub-pattern constrains that field (the official sub-pattern face
        // runs under pushPropertySubpattern) and promotes nothing here.
        // A direct operand of an open top-level OR alternation runs one
        // level deep (the operand is its own pattern node) but still
        // constrains the SCRUTINEE — its promotion joins the alternation
        // at the close (`or_pattern_end`).
        if self.pattern_depth != 1 && Some(self.pattern_depth) != self.or_operand_depth {
            // A nested object pattern's head tests its own PROPERTY and may
            // fail on that value (`Obj(parent2: Cascade())`): under a
            // vacuous enclosing head this join is the only unmatched path —
            // the same deferral the nested typed binder carries. The
            // property's type is beyond the flow lattice, so the head
            // joins as maybe-failing whenever a pattern context is open.
            if !self.collecting()
                && self.pattern.is_some()
                && let Some(chain) = self.pattern.as_ref().map(|p| p.unmatched.clone())
            {
                let cur = self.current.clone();
                self.note_may_fail(&cur);
                let joined = self.join(Some(chain), Some(cur));
                if let Some(pattern) = self.pattern.as_mut() {
                    pattern.unmatched = joined;
                }
            }
            return;
        }
        // A `dynamic(...)` head's required type is `dynamic`: every matched
        // type is its subtype, so the head neither promotes (official
        // `promoteForPattern` never promotes to a top type) nor fails. A
        // switch case's live-init chain dies like any other cover when no
        // property sub-pattern can fail either (`case dynamic(:var t)`);
        // a refutable field keeps the may-fail arm.
        if self
            .names
            .get(name as usize)
            .is_some_and(|n| &**n == "dynamic")
        {
            if fields_irrefutable {
                self.vacuous_cover_kills_chain();
            } else {
                // A refutable field under the dynamic head may fail
                // (official `promoteForPattern`'s sub-pattern walk): the
                // undecided face keeps the may-fail posture.
                self.stand_down_may_fail();
            }
            return;
        }
        // The `Null(...)` head's required type IS the Null type: the same
        // spelling→base normalization the annotation path performs
        // (record.rs's "Null" arm). Without it the head classifies as a
        // plain non-nullable name, so `promoteForPattern`'s covers and
        // disjoint verdicts mis-fire both ways over a Null-typed matched
        // value (`if (nullValue case Null())` missed its dead else —
        // `Null <: Null` is the guaranteed cover; a non-nullable matched
        // value missed the disjoint kill).
        let known = if self
            .names
            .get(name as usize)
            .is_some_and(|n| &**n == "Null")
        {
            Ty {
                base: Base::Null,
                nullable: false,
            }
        } else {
            Ty {
                base: Base::Name(name),
                nullable: false,
            }
        };
        if self.pattern_known_invalid(known, args) {
            return;
        }
        let Some(var) = self.pattern_scrutinee() else {
            // The non-reference scrutinee's vacuous cover defers exactly
            // like the reference one below: the property sub-patterns
            // decide at the pattern's close.
            if let Some((ty, frame_args)) = self.scrutinee_static_ty()
                && self
                    .ops
                    .is_invalid_typed(ty, frame_args, self.args, self.names)
            {
                self.defer_vacuous_object_cover(name);
                return;
            }
            // The `cannotMatch` verdict (`promoteForPattern`'s
            // `classifyType` legs, flow_analysis.dart:6360-6383): a
            // null-or-equivalent matched value can never match a
            // non-nullable head (and the mirror pair) under sound flow.
            // The verdict is a classification question on the matched
            // value's STATIC type — upstream builds a synthetic reference
            // from it — so a standalone context's non-reference scrutinee
            // answers it too. It kills the MATCHED arm only: the
            // `matchFailsIfWrongType` join below still feeds the failure
            // path (upstream's `_current.setUnreachable()` sits beside,
            // never instead of, the `_unmatched` join).
            //
            // The switch kinds are deliberately excluded: their chain and
            // post-switch reachability are joined by the switch tracker
            // (`interp_stmt.rs`'s `switch_exhaustive_verdict` join), which
            // is outside this lane's file partition. Applying the verdict
            // here without that join deads the code AFTER a
            // non-exhaustive switch whose only clause cannot match —
            // residual, tracked with the L-B report.
            let live = self.current.clone();
            if self.sound_flow
                && let Some((ty, _)) = self.standalone_scrutinee_ty()
                && self.disjoint_by_nullability(ty, known)
            {
                self.unreachable_cur();
            }
            // The official `promoteForPattern` runs for a non-reference
            // matched value too (`switch (this.state)` — a property
            // scrutinee, flow_analysis.dart:7746-7776):
            // `tryPromoteForTypeCheck` promotes nothing, so the true and
            // false arms are both the unpromoted current, and
            // `matchFailsIfWrongType && !coversMatchedType` still joins
            // that arm into `_unmatched`. Without the join the chain keeps
            // its `beginAlternative` unreachable seed and every later
            // alternative of the switch reads dead (riverpod's
            // `notifier_provider.dart` `_callEncode` — cases after the
            // first went DEAD_CODE). The covers gate is the same subtype
            // verdict the reference path runs below, over the latched
            // scrutinee type; the raw pattern head already spells
            // the required type, and no latched type at all may always
            // fail, so both directions join.
            let covered = self.scrutinee_static_ty().is_some_and(|(ty, frame_args)| {
                self.ops
                    .is_subtype_typed(ty, frame_args, known, args, self.args, self.names)
            });
            if !covered {
                self.join_unmatched(live);
            }
            // The dotted-read channel: a promotable property scrutinee
            // (`switch (x.y)` — official `promoteForPattern` runs the same
            // type-test arm on the property's promotion key, whose upstream
            // lattice promotes the slot to the required type). The
            // name-level property chain spells only the non-null marker —
            // a non-nullable head writes it on the matched arm exactly
            // like `x.y != null`'s null check (`promote_prop_non_null`),
            // and the receiver/slot capture gates refuse the same cases
            // upstream's `info.writeCaptured` bail does.
            if !known.nullable
                && let Some(slot) = self.pattern_scrutinee_prop()
            {
                let receiver = self.props[slot as usize].0;
                let captured = self.prop_target_captured(receiver)
                    || self.current.prop(slot).captured();
                if !captured {
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
            return;
        };
        let model = self.current.model(var);
        let (matched, matched_args) = (
            self.promoted_type_of(var, &model),
            self.promoted_args_of(var, &model),
        );
        // The downward-inferred required type: a raw pattern spelling
        // solves its own parameters against the matched type along the
        // superclass chain (`case AL():` over `AV<int>` reads `AL<int>`,
        // the spelling the official feeds `promoteForPattern`). A written
        // argument list already spells the required type — inference is
        // the raw face only.
        let (known, args) = if args == crate::ty::EMPTY_ARGS {
            self.ops
                .downcast_infer(known, args, matched, matched_args, self.args, self.names)
                .unwrap_or((known, args))
        } else {
            (known, args)
        };
        let or_operand = Some(self.pattern_depth) == self.or_operand_depth;
        let latch_face = or_operand
            .then(|| self.or_pattern_latch.map(|(_, ty, args)| (ty, args)))
            .flatten();
        let or_before = or_operand.then_some(latch_face);
        let candidate = self.pattern_promotion_candidate(var, known, args);
        let (matched_arm, unmatched_arm) = self.branch_type_test(var, known, args, candidate);
        self.current = matched_arm;
        if let Some((lty, largs)) = or_before.flatten() {
            let m = self.current.model(var);
            let after = (
                self.promoted_type_of(var, &m),
                self.promoted_args_of(var, &m),
            );
            // The candidate's presence is the promotion attempt: an
            // applied test that lands on the face already standing is the
            // vacuous same-face promotion (a valid join arm). A rejected
            // candidate still counts when the pattern's own face IS the
            // face the left branch promoted to (`Top() || Top()`) — the
            // test was vacuous, not absent; anything else promoted
            // nothing.
            let face = if candidate.is_some() {
                Some(after)
            } else {
                ((lty, largs) == (known, args)).then_some((lty, largs))
            };
            self.or_right_face = Some(face);
        }
        // The cannot-match kill (`promoteForPattern`'s verdict,
        // flow_analysis.dart:6360-6383) is sound-only: BOTH classifyType
        // arms gate on `soundFlowAnalysisEnabled` ("a new reasoning step
        // that was added to flow analysis as part of the
        // sound-flow-analysis feature"), so a pre-3.9 unit keeps the case
        // body live however the nullabilities sit.
        if self.sound_flow && self.disjoint_by_nullability(matched, known) {
            self.unreachable_cur();
        }
        if !self
            .ops
            .is_subtype_typed(matched, matched_args, known, args, self.args, self.names)
        {
            self.join_unmatched(unmatched_arm);
        } else if self
            .ops
            .is_invalid_typed(matched, matched_args, self.args, self.names)
        {
            // The vacuous cover: the matched type is the invalid face, so
            // the cannot-fail verdict rides the 0c arm — the case leaves
            // the chain dead for every later alternative. The verdict
            // DEFERS: a property sub-pattern whose own required type is
            // the invalid face may fail (official `promoteForPattern`
            // refuses it), so the close re-decides with the collected
            // fields.
            self.defer_vacuous_object_cover(name);
        }
    }

    /// Latches the object pattern's vacuous cover for the pattern close to
    /// re-decide ([`Interp::resolve_vacuous_object_cover`]): the head-time
    /// state survives as the may-fail arm should any property's required
    /// type be the invalid face, and the property sub-patterns' bound
    /// names collect as the close's query set (the shorthand `:var name`
    /// binds the member's own name; an explicit label's bound name misses
    /// the member table and keeps the cover — the under-report direction).
    fn defer_vacuous_object_cover(&mut self, owner: u32) {
        self.vacuous_object = Some((owner, self.current.clone()));
        self.vacuous_fields = self.pattern_binds.clone();
    }

    /// Decides a deferred object-pattern vacuous cover at the pattern's
    /// close: any property sub-pattern whose required type is the invalid
    /// face may fail (official `promoteForPattern`'s outright refusal,
    /// `flow_analysis.dart:7720-7723`), so its head-time arm rejoins the
    /// unmatched chain instead of the cover's kill.
    fn resolve_vacuous_object_cover(&mut self) {
        let Some((owner, state)) = self.vacuous_object.take() else {
            return;
        };
        let fields = std::mem::take(&mut self.vacuous_fields);
        if fields
            .iter()
            .any(|&field| self.ops.object_property_invalid(owner, field, self.names))
        {
            self.join_unmatched(state);
        } else {
            self.vacuous_cover_kills_chain();
        }
    }

    /// Rejoins one state into the open pattern's unmatched chain (official
    /// `_unmatched = _join(_unmatched!, state)` at every may-fail arm).
    fn join_unmatched(&mut self, state: crate::model::State) {
        self.note_may_fail(&state);
        if let Some(chain) = self.pattern.as_ref().map(|p| p.unmatched.clone()) {
            let joined = self.join(Some(chain), Some(state));
            if let Some(pattern) = self.pattern.as_mut() {
                pattern.unmatched = joined;
            }
        }
    }

    /// Latches the open pattern's cannot-fail posture off (official
    /// `promoteForPattern`'s joins): a join whose joined-in state is
    /// locally reachable means the match can fail, so the clause close's
    /// chain kill stands down. Joins of unreachable states are officially
    /// no-ops on the chain's reachability (`_join` drops the unreachable
    /// arm) — the posture stands. Every latch also bumps the may-fail
    /// epoch ([`Interp::or_join_epoch`]).
    fn note_may_fail(&mut self, state: &crate::model::State) {
        if self.reach.locally_reachable(state.reach)
            && let Some(pattern) = self.pattern.as_mut()
        {
            pattern.cannot_fail = false;
            self.or_join_epoch += 1;
        }
    }

    /// A typed sub-pattern nested in a container field (`Obj(:int f)`,
    /// `case (int _,)`) tests its own property value and can fail on that
    /// value alone — official `analyzeObjectPattern`/record analysis runs
    /// sub-patterns against the shared `_unmatched` chain. When the
    /// enclosing head's test is vacuous (an exact scrutinee type joins
    /// nothing), this join is the ONLY unmatched path: skip it and the
    /// else arm after `if (x case Obj(:T f))` folds dead. The pre-test
    /// state joins (reachability — the property value is scoped to the
    /// matched arm, so the else arm keeps the scrutinee's unpromoted
    /// face, exactly the nested-failure posture).
    fn nested_typed_join(&mut self) {
        let Some(chain) = self.pattern.as_ref().map(|p| p.unmatched.clone()) else {
            return;
        };
        let cur = self.current.clone();
        self.note_may_fail(&cur);
        let joined = self.join(Some(chain), Some(cur));
        if let Some(pattern) = self.pattern.as_mut() {
            pattern.unmatched = joined;
        }
    }

    /// Stands the open pattern's cannot-fail posture down without a join:
    /// a face the lattice cannot answer (a constant over a non-reference
    /// matched value, a dynamic head with refutable fields) may fail
    /// upstream, so the clause close's chain kill must not fire on it.
    fn stand_down_may_fail(&mut self) {
        if let Some(pattern) = self.pattern.as_mut() {
            pattern.cannot_fail = false;
        }
    }

    /// The empty-map pattern's coverage question (official
    /// `coversMatchedType = isSubtypeOf(matchedType, knownType)`,
    /// flow_analysis.dart:6399-6402 — the MATCHED type must sit under the
    /// pattern's required type): the map-friendly matched faces
    /// (`Map`-named, top, dynamic, Never) answer yes over the flow
    /// lattice's record-free shape — a concrete non-map matched type (a
    /// `List`, a named class, `Object`) answers no and keeps the may-fail
    /// join.
    fn shape_head_covers(&self) -> bool {
        let (matched, matched_args) = match self.pattern_scrutinee() {
            Some(var) => {
                let model = self.current.model(var);
                (
                    self.promoted_type_of(var, &model),
                    self.promoted_args_of(var, &model),
                )
            }
            None => match self
                .pattern
                .as_ref()
                .and_then(|p| p.scrutinee_ty)
                .or_else(|| self.frame_scrutinee_ty())
            {
                Some((ty, args)) => (ty, args),
                None => return false,
            },
        };
        match matched.base {
            Base::Unknown | Base::Dynamic | Base::Never => true,
            Base::Name(name) => {
                matched_args == crate::ty::EMPTY_ARGS
                    && self.names.get(name as usize).is_some_and(|n| &**n == "Map")
            }
            _ => false,
        }
    }

    /// Opens a logical-or alternation's dead-walk frame (official
    /// `logicalOrPattern_begin`/`afterRightHandSideBegins`): when the LEFT
    /// operand could not fail (no may-fail join latched its parse), the
    /// right operand parses under the left branch's failure state —
    /// unreachable — and its row anchors at the operator
    /// (`handle_logicalOrPattern_afterLhs` →
    /// `checkUnreachableNode(node.rightOperand)`).
    fn or_frame_begin(&mut self, operator: TokenSpan) {
        if self.collecting() || self.pattern.is_none() {
            return;
        }
        let epoch = self.or_join_epoch;
        // The left branch is clean when nothing latched since the pattern
        // context opened (the first alternation: the left operand's own
        // events are all that ran), or when the previous alternation
        // closed clean at this same epoch (the left-nested `A || B || C`
        // chain — the outer left is the inner or, and the inner or can
        // fail only through its right operand's joins).
        let left_clean = epoch == 0
            || self
                .or_last_clean
                .is_some_and(|(end_epoch, clean)| end_epoch == epoch && clean);
        let entry = self.current.clone();
        self.or_frames.push(crate::interp::OrFrame {
            operator,
            entry,
            epoch,
            left_clean,
        });
        if left_clean {
            self.unreachable_cur();
            self.dead_code(operator);
        }
    }

    /// Closes the frame: the right operand's row (official
    /// `LogicalOrPattern.resolvePattern`'s `flowEnd(rightOperand)` with the
    /// `parent is LogicalOrPattern` span — the row runs from the operator
    /// to `rightOperand.end`, the operand's last token's end), and the
    /// matched state restores to the left branch's
    /// (`logicalOrPattern_end`'s `_join(_lhsMatched, _current)` collapses
    /// to the left branch when the right walked unreachable). A window
    /// anchored elsewhere keeps ownership (an enclosing dead context
    /// latched first — official `visitNode` ignores later unreachable
    /// nodes); the interval then continues through the arm's own close.
    /// The close also publishes the right operand's cleanliness for a
    /// following left-nested alternation.
    fn or_frame_end(&mut self, end: u32) {
        let Some(frame) = self.or_frames.pop() else {
            return;
        };
        let right_clean = frame.epoch == self.or_join_epoch;
        if right_clean {
            self.or_last_clean = Some((self.or_join_epoch, true));
        }
        if frame.left_clean {
            self.close_dead_row_to(frame.operator.range.offset, end);
            self.current = frame.entry;
        }
    }

    /// The official `promoteForPattern` invalid-required-type arm
    /// (`flow_analysis.dart:7720-7723`): a pattern whose written type
    /// resolves nowhere neither tests nor promotes — the live state
    /// rejoins the unmatched chain and the body keeps running on it.
    /// Without the arm the type test's `factor(any, Invalid)` answers
    /// `Never` (subtype.dart's 0c arm) and every such pattern body deads.
    fn pattern_known_invalid(&mut self, known: Ty, args: u32) -> bool {
        if !self
            .ops
            .is_invalid_typed(known, args, self.args, self.names)
        {
            return false;
        }
        let cur = self.current.clone();
        self.join_unmatched(cur);
        true
    }

    /// The statement-switch frame's latched scrutinee type (the scrutinee
    /// value's own ty at `SwitchBlockBegin`), with its written arguments.
    fn frame_scrutinee_ty(&self) -> Option<(Ty, u32)> {
        self.switch_frame()
            .and_then(|index| match &self.stack[index] {
                Frame::Switch {
                    scrutinee_ty,
                    scrutinee_args,
                    ..
                } => Some((*scrutinee_ty, *scrutinee_args)),
                _ => None,
            })
    }

    /// The matched value's STATIC type for a NON-reference scrutinee — the
    /// face the official `promoteForPattern` reads
    /// (`flow_analysis.dart:6350-6394`). Upstream never asks the matched
    /// value for a promotable reference: it builds a *synthetic* reference
    /// from `matchedType` (`context.createReference(matchedType,
    /// _current)`), so the coverage and `cannotMatch` verdicts are pure type
    /// questions and answer for a literal, a select, and a call exactly
    /// like for a variable.
    ///
    /// A statement/expression switch latches the scrutinee's type on its
    /// frame; a standalone context (if-case, pattern declaration) carries
    /// the peeked expression's own `(ty, args)` instead. Both are the same
    /// question, so one accessor answers it — reading only the switch frame
    /// left every standalone non-reference pattern (`if (null case Null v)`)
    /// answering "no matched type", which fell into the may-fail join and
    /// kept the else branch live.
    fn scrutinee_static_ty(&self) -> Option<(Ty, u32)> {
        self.frame_scrutinee_ty().or_else(|| self.standalone_scrutinee_ty())
    }

    /// The standalone contexts' (if-case, pattern declaration) matched-value
    /// type — the peeked scrutinee expression's own `(ty, args)`. A switch
    /// carries the same face on its frame ([`Self::frame_scrutinee_ty`]);
    /// this accessor exists for the verdicts that must stay off the switch
    /// path (see [`Self::non_var_invalid_scrutinee_covers`]'s `cannotMatch`
    /// note).
    fn standalone_scrutinee_ty(&self) -> Option<(Ty, u32)> {
        self.pattern
            .as_ref()
            .filter(|p| matches!(p.kind, PatternKind::IfCase | PatternKind::Decl))
            .and_then(|p| p.scrutinee_ty)
    }

    /// Whether the open standalone context (if-case, declaration) matches a
    /// non-reference value whose static type is the invalid face.
    fn standalone_scrutinee_invalid(&self) -> bool {
        self.pattern.as_ref().is_some_and(|p| {
            matches!(p.kind, PatternKind::IfCase | PatternKind::Decl)
                && p.scrutinee.is_none()
                && p.scrutinee_ty.is_some_and(|(ty, args)| {
                    self.ops.is_invalid_typed(ty, args, self.args, self.names)
                })
        })
    }

    /// The official `_pushPattern` starts every case's working unmatched
    /// chain UNREACHABLE (flow_analysis.dart:9376-9380), and an
    /// alternative whose pattern provably covers leaves it that way —
    /// later alternatives and the post-switch state run unreachable. The
    /// switch-case contexts init the chain from the frame instead (the
    /// live-init keeps every coverage-less pattern face's chain alive),
    /// so a VACUOUS cover — the matched type IS the invalid face, whose
    /// every subtype relation holds (the oracle's invalid-type leniency,
    /// oracle-probed on t/u/v/w shapes: statement and expression
    /// switches, bare and fielded object patterns, declared-variable
    /// patterns) — must kill the chain explicitly. Guards still join
    /// their false arm at the pattern exit, reviving the chain exactly
    /// like the official's guard-may-fail alternative. Statement
    /// switches only: the expression alternatives' dead rows are the
    /// semantic synthesizer's face (a flow window there would anchor on
    /// the arm's member polls).
    fn vacuous_cover_kills_chain(&mut self) {
        if let Some(pattern) = self.pattern.as_mut()
            && matches!(pattern.kind, PatternKind::Switch)
        {
            pattern.unmatched = pattern.unmatched.clone().set_unreachable(&mut self.reach);
        }
    }

    /// A typed pattern over a NON-reference scrutinee (a select, a call):
    /// the official's `promoteForPattern` still answers the coverage
    /// question on the scrutinee's static type. When that type IS the
    /// invalid face, the pattern covers vacuously and the case's
    /// unmatched chain dies; every other scrutinee type keeps the
    /// live-init posture (the may-fail question has no flow-visible
    /// answer without a promotable reference).
    fn non_var_invalid_scrutinee_covers(&mut self, declared: Ty, declared_args: u32) {
        if let Some((ty, args)) = self.scrutinee_static_ty()
            && self.ops.is_invalid_typed(ty, args, self.args, self.names)
        {
            self.vacuous_cover_kills_chain();
            return;
        }
        // A standalone context's (if-case) non-reference matched value —
        // `this` in an extension on an unresolved type, `x.foo` off an
        // invalid receiver: the same vacuous cover, and the context's
        // chain is already seeded unreachable, so skipping the join below
        // keeps the else arm dead.
        if self.standalone_scrutinee_invalid() {
            return;
        }
        // The `cannotMatch` verdict (`promoteForPattern`'s `classifyType`
        // legs, flow_analysis.dart:6360-6383): a null-or-equivalent
        // matched value can never match a non-nullable required type (and
        // the mirror pair) under sound flow. Both legs are classification
        // questions on the matched value's STATIC type — no reference
        // involved — so a standalone if-case's non-reference scrutinee
        // answers them the same. The verdict kills the MATCHED arm only;
        // the failure join below still runs on the pre-verdict state
        // (upstream keeps the join beside `_current.setUnreachable()`,
        // never behind it). Switch kinds are excluded for the reason given
        // in [`Interp::matched_object_pattern`].
        let live = self.current.clone();
        if self.sound_flow
            && let Some((ty, _)) = self.standalone_scrutinee_ty()
            && self.disjoint_by_nullability(ty, declared)
        {
            self.unreachable_cur();
        }
        // A property scrutinee's type test is refutable like a variable's
        // (the non-var face joins the same `_unmatched` —
        // [`Self::matched_object_pattern`]'s covered gate): a frame-latched
        // type that is not the pattern type's subtype may fail, and
        // nothing else feeds the chain when this pattern is the whole
        // case (`member.namePart case PrimaryConstructorDeclarationImpl c`
        // folds its else dead without the join). No latched type at all
        // may always fail, so it joins too.
        if self.pattern.is_some() {
            let covered = self.scrutinee_static_ty().is_some_and(|(ty, frame_args)| {
                self.ops.is_subtype_typed(
                    ty,
                    frame_args,
                    declared,
                    declared_args,
                    self.args,
                    self.names,
                )
            });
            if !covered {
                self.join_unmatched(live);
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

    /// The matched value's property slot — the dotted-read scrutinee's
    /// promotion key (`switch (x.y)`, [`Interp::prop_slot`]): the same
    /// question [`Self::pattern_scrutinee`] answers for a bare-read
    /// scrutinee, keyed on the property slot a promotable member read
    /// latches. Only the switch kinds carry it — a standalone context's
    /// `scrutinee` latch never names a property.
    fn pattern_scrutinee_prop(&self) -> Option<u32> {
        match self.pattern.as_ref().map(|p| p.kind) {
            Some(PatternKind::Switch | PatternKind::SwitchExpr) => {
                self.switch_frame()
                    .and_then(|index| match &self.stack[index] {
                        Frame::Switch { scrutinee_prop, .. } => *scrutinee_prop,
                        _ => None,
                    })
            }
            _ => None,
        }
    }

    /// The pattern-face promotion candidate (official
    /// `tryPromoteForTypeCheck`'s true arm reached through
    /// `promoteForPattern`): the same [`TypeOps::try_promote_to_typed`]
    /// gate the is-test face runs — `to <: from` on the instantiated
    /// spellings, the matched value's promoted type and its written
    /// arguments as `from`. A same-base pattern whose arguments cannot
    /// sit under the matched ones (`case List<num>():` against
    /// `List<int> x`) refuses at the name level already; the unparameterized
    /// head of a cross-base match (`case Some():` on `Option<T>`)
    /// promotes: the official entry infers the required type's arguments
    /// downward from the matched type
    /// (`downwardInferObjectPatternRequiredType`), and the written
    /// spelling stays the honest name-level stand-in the undecided
    /// cross-base question promotes through.
    fn pattern_promotion_candidate(&self, var: u32, tested: Ty, tested_args: u32) -> Option<Ty> {
        match tested.base {
            Base::Unknown | Base::Dynamic | Base::Void => None,
            _ => {
                let model = self.current.model(var);
                let (matched, matched_args) = (
                    self.promoted_type_of(var, &model),
                    self.promoted_args_of(var, &model),
                );
                self.ops.try_promote_to_typed(
                    tested,
                    tested_args,
                    matched,
                    matched_args,
                    self.args,
                    self.names,
                )
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
        self.vacuous_object = None;
        self.vacuous_fields.clear();
        self.pattern_binds.clear();
        self.or_join_epoch = 0;
        self.or_last_clean = None;
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
            scrutinee_ty: None,
            scrutinee_pending: false,
            guard: None,
            pre_guard: None,
            pending_decls: Vec::new(),
            shadowed: Vec::new(),
            cannot_fail: true,
            saw_node: false,
        });
    }

    /// Closes the pattern context of a switch-statement case expression:
    /// the guard's branches split matched/unmatched, and the chain state
    /// plus the combined group state update in the switch frame (official
    /// `switch_endAlternative`).
    pub(crate) fn case_pattern_end(&mut self, has_guard: bool) {
        self.resolve_vacuous_object_cover();
        let Some(mut pattern) = self.pattern.take() else {
            return;
        };
        let PatternKind::Switch = pattern.kind else {
            self.pattern = Some(pattern);
            return;
        };
        let pattern_shadowed = std::mem::take(&mut pattern.shadowed);
        // The cannot-fail kill (official `switchStatement_endAlternative`:
        // `context._unmatched = _popPattern(guardInfo)` — the popped chain
        // IS the pattern context's seed whenever no may-fail arm joined,
        // and the seed started unreachable): a pattern that provably
        // always matches leaves the chain dead, so every later member and
        // the post-switch state read dead. A clause whose head produced
        // no pattern node at all (a recovered expression) knows nothing:
        // the kill stands down.
        let (mut unmatched, kill) = self.apply_pattern_exit(pattern, has_guard);
        if kill {
            unmatched = unmatched.set_unreachable(&mut self.reach);
        }
        if let Some(frame_index) = self.switch_frame() {
            let combined = self.current.clone();
            let prior = match &mut self.stack[frame_index] {
                Frame::Switch {
                    unmatched: slot,
                    group_combined,
                    case_open,
                    case_shadowed,
                    ..
                } => {
                    *slot = unmatched;
                    *case_open = true;
                    // The head's binders shadow only inside this member —
                    // the member's close (`switch_after_case`) restores.
                    case_shadowed.extend(pattern_shadowed);
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
        self.resolve_vacuous_object_cover();
        let Some(mut pattern) = self.pattern.take() else {
            return;
        };
        let PatternKind::SwitchExpr = pattern.kind else {
            self.pattern = Some(pattern);
            return;
        };
        let pattern_shadowed = std::mem::take(&mut pattern.shadowed);
        // The cannot-fail kill (same chain write as the statement face —
        // official `switchStatement_endAlternative` runs for expression
        // alternatives too): the next alternative's entry state is the
        // dead chain, and its whole-arm row anchors there. The seen-node
        // gate keeps a recovered arm (no pattern events) live.
        let (mut unmatched, kill) = self.apply_pattern_exit(pattern, false);
        if kill {
            unmatched = unmatched.set_unreachable(&mut self.reach);
        }
        if let Some(frame_index) = self.switch_frame()
            && let Frame::Switch {
                unmatched: slot,
                case_shadowed,
                ..
            } = &mut self.stack[frame_index]
        {
            *slot = unmatched;
            case_shadowed.extend(pattern_shadowed);
        }
    }

    /// The if-case marker (`handle_parenthesized_condition` with a `case`
    /// keyword): the standalone pattern context opened inside the condition
    /// is this if's clause context. Turning the kind here — at the
    /// condition's end — keeps `ThenBegin`'s adoption exact: an ordinary
    /// if never sees an `IfCase` context and never adopts one. The peeked
    /// scrutinee value is spent here: the clause machinery owns everything
    /// past the condition.
    pub(crate) fn if_case_condition(&mut self) {
        if let Some(pattern) = self.pattern.as_mut()
            && pattern.kind == PatternKind::Decl
        {
            pattern.kind = PatternKind::IfCase;
            let pending = pattern.scrutinee_pending;
            pattern.scrutinee_pending = false;
            if pending {
                drop(self.exprs.pop());
            }
        }
    }

    /// Closes a standalone pattern context at its construct's end — the
    /// official `_popPattern(null)` sites (`patternVariableDeclaration_end`,
    /// `patternAssignment_end`, `patternForIn_end`): the matched state's
    /// split is undone (`_setCurrent(_current.unsplit())`) and the unmatched
    /// chain is dropped (a declaration, assignment, or for-in head has no
    /// alternative clause to fall through to). Only a `Decl` context closes
    /// here: a switch context belongs to its clause machinery, and an
    /// `IfCase` context is consumed by `ThenBegin`.
    pub(crate) fn close_standalone_pattern(&mut self) {
        self.resolve_vacuous_object_cover();
        let Some(pattern) = self.pattern.take() else {
            return;
        };
        if pattern.kind != PatternKind::Decl {
            self.pattern = Some(pattern);
            return;
        }
        self.current = self.current.clone().unsplit(&mut self.reach);
    }

    /// Applies a pattern's guard to split matched/unmatched, returning the
    /// chained unmatched state plus the pattern's final cannot-fail
    /// posture (official `_popPattern`, fed by the clause's `guardInfo` —
    /// flow_analysis.dart:8004-8042: the guard's true state becomes the
    /// clause's matched state, the false state joins the unmatched chain).
    /// The guard's own events ran on `current` between `GuardBegin` and
    /// `GuardEnd` — its last branching event (e.g. the type-test's
    /// `IsEnd`, `interp_expr.rs`) left `current` on the true arm — so
    /// `current` IS the guard's ifTrue state and the `GuardBegin`
    /// snapshot is its ifFalse. The guard can fail, so its join stands
    /// the cannot-fail posture down (the pattern context is already
    /// taken out of `self` here, so the posture latches locally).
    fn apply_pattern_exit(
        &mut self,
        mut pattern: PatternState,
        has_guard: bool,
    ) -> (crate::model::State, bool) {
        let mut unmatched = pattern.unmatched;
        let mut cannot_fail = pattern.cannot_fail && pattern.saw_node;
        if (has_guard || matches!(pattern.kind, PatternKind::Switch | PatternKind::SwitchExpr))
            && pattern.guard.take().is_some()
        {
            let if_false = pattern
                .pre_guard
                .take()
                .unwrap_or_else(|| self.current.clone());
            if self.reach.locally_reachable(if_false.reach) {
                cannot_fail = false;
            }
            unmatched = self.join(Some(unmatched), Some(if_false));
        }
        (unmatched, cannot_fail)
    }
}

/// Whether every property sub-pattern of the object pattern closing at
/// `events[at]` is irrefutable: the field events run between the object's
/// own `PatternBegin` and its `ObjectPattern` close, and only an untyped
/// binder (`:var name`, `name: final x`) or a bare wildcard cannot fail
/// (official `DeclaredVariablePattern`/`WildcardPattern` without a type
/// annotation match every value). Anything else — a constant, a typed
/// binder, a nested destructure, a null-check — answers refutable, the
/// may-fail direction. The walk is bounded by the pattern's own events.
fn object_fields_irrefutable(events: &[Ev], at: usize) -> bool {
    let mut depth = 0usize;
    for ev in events[..at].iter().rev() {
        match ev {
            // Parenthesized wrappers are grouping-transparent: their
            // flagged pair cancels in the backward walk.
            Ev::PatternEnd { paren: true } | Ev::PatternBegin { paren: true, .. } => {}
            Ev::PatternEnd { paren: false } => depth += 1,
            Ev::PatternBegin { paren: false, .. } => {
                if depth == 0 {
                    return true;
                }
                depth -= 1;
            }
            Ev::NoType | Ev::DeclaredVarPattern { .. } | Ev::Wildcard { .. } => {}
            _ => return false,
        }
    }
    false
}
