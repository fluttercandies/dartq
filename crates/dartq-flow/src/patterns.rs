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
                    | Ev::ShapePattern
                    | Ev::NullCheckPattern
                    | Ev::BinaryOrPatternBegin
                    | Ev::BinaryOrPatternEnd
                    | Ev::GuardBegin
                    | Ev::GuardEnd
            )
        {
            return;
        }
        match ev {
            Ev::ObjectPattern { name, args } => self.matched_object_pattern(*name, *args),
            Ev::BinaryOrPatternBegin => self.or_pattern_begin(),
            Ev::BinaryOrPatternEnd => self.or_pattern_end(),
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
                        self.vacuous_object = None;
                        self.vacuous_fields.clear();
                        self.pattern_binds.clear();
                        self.pattern = Some(PatternState {
                            kind: PatternKind::Decl,
                            unmatched,
                            scrutinee,
                            guard: None,
                            pre_guard: None,
                            pending_decls: Vec::new(),
                            shadowed: Vec::new(),
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
                            self.args,
                            self.names,
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
            Ev::ShapePattern => {
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
                    let cur = self.current.clone();
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
                if self.pattern_depth != 1 {
                    return;
                }
                if let Some(var) = self.pattern_scrutinee() {
                    let model = self.current.model(var);
                    let (matched, matched_args) = (
                        self.promoted_type_of(var, &model),
                        self.promoted_args_of(var, &model),
                    );
                    // A matched value already known non-null is guaranteed to
                    // match: the official promotes nothing and leaves
                    // `_unmatched` untouched (the arm cannot fall through).
                    let guaranteed = self.classify(matched) == Classification::NonNullable;
                    let (non_null, non_null_args) =
                        self.promote_to_non_null(matched, matched_args);
                    if !guaranteed {
                        // Official order matters here: `_unmatched` joins the
                        // state BEFORE the promotion (the matched value is
                        // still nullable on the failure arm), and only then
                        // does `_setCurrent(ifNotNull)` replace `current`.
                        // Joining the promoted state instead would carry the
                        // non-null face into the next clause's entry state and
                        // kill a later `case null:` arm
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
                    // The declared variable beside the null check (`case var
                    // x?`) is declared untyped (`NoType` leaves
                    // [`Ty::UNKNOWN`]) by the event that precedes this one;
                    // official `_handleNullCheckPattern` hands the
                    // sub-pattern the non-null matched type, and
                    // `DeclaredVariablePattern` declares the variable with
                    // it. An explicit annotation keeps its own type
                    // (`declare` prefers the written one). A guaranteed match
                    // stores the matched type as it stands, already non-null.
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
                } else {
                    // A non-variable scrutinee promotes no reference, but the
                    // pattern can still fail: the live state joins the
                    // failure arm unchanged.
                    let cur = self.current.clone();
                    self.join_unmatched(cur);
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
                let (declared, declared_args) = if declared == Ty::UNKNOWN {
                    self.inferred_locals
                        .get(&token.range.offset)
                        .copied()
                        .map_or((declared, declared_args), |(ty, args)| (ty, args))
                } else {
                    (declared, declared_args)
                };
                let meta = VarMeta {
                    ty: declared,
                    args: declared_args,
                    late: false,
                    is_final: *is_final,
                    decl_span: Some(token.range),
                };
                let shadowed = self.bound_var(*name);
                self.declare_var(*name, meta, true);
                if let Some(pattern) = self.pattern.as_mut() {
                    pattern.pending_decls.push((*name, *is_final));
                    pattern.shadowed.push((*name, shadowed));
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
                if !self.collecting()
                    && self.pattern_depth == 1
                    && declared != Ty::UNKNOWN
                {
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
                        self.non_var_invalid_scrutinee_covers();
                    }
                } else if !self.collecting()
                    && self.pattern_depth > 1
                    && declared != Ty::UNKNOWN
                    && let Some(chain) = self.pattern.as_ref().map(|p| p.unmatched.clone())
                {
                    // A typed sub-pattern nested in an object pattern tests
                    // its own PROPERTY (`Obj(:NamedType f)` over a nullable
                    // field) and can fail on that value alone — official
                    // `analyzeObjectPattern` runs sub-patterns against the
                    // shared `_unmatched` chain. When the enclosing head's
                    // test is vacuous (an exact scrutinee type joins
                    // nothing), this join is the ONLY unmatched path: skip
                    // it and the else arm after `if (x case Obj(:T f))`
                    // folds dead. The pre-test state joins (reachability —
                    // the property variable is scoped to the matched arm,
                    // so the else arm keeps the scrutinee's unpromoted
                    // face, exactly the nested-failure posture).
                    let cur = self.current.clone();
                    let joined = self.join(Some(chain), Some(cur));
                    if let Some(pattern) = self.pattern.as_mut() {
                        pattern.unmatched = joined;
                    }
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
                if !self.collecting()
                    && self.pattern_depth == 1
                    && declared != Ty::UNKNOWN
                {
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
                        self.non_var_invalid_scrutinee_covers();
                    }
                }
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

    pub(crate) fn matched_object_pattern(&mut self, name: u32, args: u32) {
        // A top-level pattern constrains the matched value; a field's
        // sub-pattern constrains that field (the official sub-pattern face
        // runs under pushPropertySubpattern) and promotes nothing here.
        // A direct operand of an open top-level OR alternation runs one
        // level deep (the operand is its own pattern node) but still
        // constrains the SCRUTINEE — its promotion joins the alternation
        // at the close (`or_pattern_end`).
        if self.pattern_depth != 1 && Some(self.pattern_depth) != self.or_operand_depth {
            return;
        }
        let known = Ty {
            base: Base::Name(name),
            nullable: false,
        };
        if self.pattern_known_invalid(known, args) {
            return;
        }
        let Some(var) = self.pattern_scrutinee() else {
            // The non-reference scrutinee's vacuous cover defers exactly
            // like the reference one below: the property sub-patterns
            // decide at the pattern's close.
            if let Some((ty, frame_args)) = self.frame_scrutinee_ty()
                && self
                    .ops
                    .is_invalid_typed(ty, frame_args, self.args, self.names)
            {
                self.defer_vacuous_object_cover(name);
                return;
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
            // verdict the reference path runs below, over the frame's
            // latched scrutinee type; the raw pattern head already spells
            // the required type, and no latched type at all may always
            // fail, so both directions join.
            let covered = self
                .frame_scrutinee_ty()
                .is_some_and(|(ty, frame_args)| {
                    self.ops
                        .is_subtype_typed(ty, frame_args, known, args, self.args, self.names)
                });
            if !covered {
                let cur = self.current.clone();
                self.join_unmatched(cur);
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
        if self.disjoint_by_nullability(matched, known) {
            self.unreachable_cur();
        }
        if !self.ops.is_subtype_typed(
            matched,
            matched_args,
            known,
            args,
            self.args,
            self.names,
        ) {
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
        if fields.iter().any(|&field| {
            self.ops.object_property_invalid(owner, field, self.names)
        }) {
            self.join_unmatched(state);
        } else {
            self.vacuous_cover_kills_chain();
        }
    }

    /// Rejoins one state into the open pattern's unmatched chain (official
    /// `_unmatched = _join(_unmatched!, state)` at every may-fail arm).
    fn join_unmatched(&mut self, state: crate::model::State) {
        if let Some(chain) = self.pattern.as_ref().map(|p| p.unmatched.clone()) {
            let joined = self.join(Some(chain), Some(state));
            if let Some(pattern) = self.pattern.as_mut() {
                pattern.unmatched = joined;
            }
        }
    }

    /// The official `promoteForPattern` invalid-required-type arm
    /// (`flow_analysis.dart:7720-7723`): a pattern whose written type
    /// resolves nowhere neither tests nor promotes — the live state
    /// rejoins the unmatched chain and the body keeps running on it.
    /// Without the arm the type test's `factor(any, Invalid)` answers
    /// `Never` (subtype.dart's 0c arm) and every such pattern body deads.
    fn pattern_known_invalid(&mut self, known: Ty, args: u32) -> bool {
        if !self.ops.is_invalid_typed(known, args, self.args, self.names) {
            return false;
        }
        let cur = self.current.clone();
        self.join_unmatched(cur);
        true
    }

    /// The statement-switch frame's latched scrutinee type (the scrutinee
    /// value's own ty at `SwitchBlockBegin`), with its written arguments.
    fn frame_scrutinee_ty(&self) -> Option<(Ty, u32)> {
        self.switch_frame().and_then(|index| match &self.stack[index] {
            Frame::Switch {
                scrutinee_ty,
                scrutinee_args,
                ..
            } => Some((*scrutinee_ty, *scrutinee_args)),
            _ => None,
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
    fn non_var_invalid_scrutinee_covers(&mut self) {
        if let Some((ty, args)) = self.frame_scrutinee_ty()
            && self.ops.is_invalid_typed(ty, args, self.args, self.names)
        {
            self.vacuous_cover_kills_chain();
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
            shadowed: Vec::new(),
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
        let unmatched = self.apply_pattern_exit(pattern, has_guard);
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
        let unmatched = self.apply_pattern_exit(pattern, false);
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
    /// if never sees an `IfCase` context and never adopts one.
    pub(crate) fn if_case_condition(&mut self) {
        if let Some(pattern) = self.pattern.as_mut()
            && pattern.kind == PatternKind::Decl
        {
            pattern.kind = PatternKind::IfCase;
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
