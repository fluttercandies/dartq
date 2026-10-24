use super::*;

/// The `dart:async` class face the member tables lack: `Stream`'s own
/// declared instance surface (upstream Dart SDK source,
/// sdk/lib/async/stream.dart — `map`/`where`/`listen`/`first`/…; the
/// `Object` members ride the shared `Object` exemption). The extension
/// `on`-face walk chain-breaks on the unindexed declaration, so a bare
/// miss inside an extension body would report where the oracle resolves
/// through the SDK class (the xml_events `each_event` tap shape). Consulted
/// beside [`CORE_INTERFACE_MEMBERS`] / [`CORE_TYPE_MEMBERS`] by the
/// frozen-face classification.
pub(crate) const SDK_FACES_EXTRA: &[(&str, &[&str])] = &[(
    "Stream",
    &[
        "any",
        "asBroadcastStream",
        "asyncExpand",
        "asyncMap",
        "cast",
        "contains",
        "distinct",
        "drain",
        "elementAt",
        "every",
        "expand",
        "first",
        "firstWhere",
        "fold",
        "forEach",
        "handleError",
        "isEmpty",
        "isBroadcast",
        "join",
        "last",
        "lastWhere",
        "length",
        "listen",
        "map",
        "pipe",
        "reduce",
        "single",
        "singleWhere",
        "skip",
        "skipWhile",
        "take",
        "takeWhile",
        "timeout",
        "toList",
        "toSet",
        "transform",
        "where",
    ],
),
    // dart:isolate's own faces (sdk/lib/isolate/isolate.dart:849/:931) —
    // `ReceivePort implements Stream<dynamic>`, so its frozen row is the
    // whole instance face: own declarations beside the Stream's the
    // unindexed edge cannot traverse (the same rule CORE_INTERFACE_
    // MEMBERS rows follow; `SDK_SUPER_FACES` joins the Stream row).
    ("ReceivePort", &["close", "listen", "sendPort"]),
    ("SendPort", &["send"]),
];

/// The unindexed `implements` edges between SDK faces the frozen verdict
/// joins: `ReceivePort implements Stream<dynamic>` makes the Stream row's
/// members answer for a ReceivePort-anchored climb.
const SDK_SUPER_FACES: &[(&str, &[&str])] = &[("ReceivePort", &["Stream"])];

/// The frozen-face member verdict (`Engine::frozen_member_verdict`): the
/// three answers a [`Walk::ChainBroken`] member walk can still decide
/// through [`CORE_INTERFACE_MEMBERS`] / [`CORE_TYPE_MEMBERS`] —
/// `Defined` (a reachable frozen face carries the member), `Absent`
/// (every hop was decidable and none carried it — the member family
/// reports), `Unknown` (a genuinely unindexed, non-frozen declaration
/// broke the climb — the under-report direction, same as `ChainBroken`).
pub(crate) enum FrozenFace {
    Defined,
    Absent,
    Unknown,
}

/// The static type a cascade target receiver classifies to (#2459②③): the
/// named faces answer the `..[` section's operator question directly, a
/// prefix or unresolvable receiver has no type the model can walk.
fn cascade_target_ty_of(recv: &Receiver) -> Option<Ty> {
    match recv {
        Receiver::Typed { ty, .. } => Some(*ty),
        Receiver::Created { name, .. }
        | Receiver::Container { name, .. }
        | Receiver::PrefixedContainer { name, .. }
        | Receiver::This { container: name, .. }
        | Receiver::Super { container: name, .. } => Some(Ty::named(*name)),
        Receiver::Prefix { .. } | Receiver::Unknown => None,
    }
}

impl<'a> Engine<'a> {
    /// The named argument membership test against the active callee
    /// (`type_analyzer.dart:5820`): a resolved callee whose formal table
    /// lacks the label reports `undefined_named_parameter` at the label; an
    /// unresolved callee stays silent (its own invocation code reports) and
    /// only feeds the unresolved-name table.
    pub(crate) fn check_named_argument(&mut self, name: StringId, range: SourceRange) {
        // A super-constructor invocation's labels bind against the resolved
        // superclass constructor's named formals
        // (`required_parameters_verifier.dart`'s super-invocation face —
        // the callee element resolves like `check_super_constructor`, and
        // `super(name: v)` reads the unnamed constructor's table while
        // `super.named(name: v)` reads the selector's).
        if self.initializer_depth > 0 {
            if let Some((container, selector)) = self.init_super_callee() {
                // An unresolvable super-constructor (missing superclass, an
                // external container the index does not model) stays silent
                // at the labels — the unresolved-callee posture, same as the
                // invokes-stack path below; the initializer's own
                // constructor faces own that report. The names table still
                // carries the miss only when the callee resolved (the main
                // path's `!defined` shape: an unresolved callee's label is
                // not a miss).
                let (resolved, defined) = match self.super_ctor_named_formals(container, selector) {
                    // The membership test is the call-site face's own
                    // (`named_formal_for`): a `super(x: v)` label binds a
                    // `this._x` formal's public name like every other
                    // invocation (upstream keys `NamedExpression.name`
                    // against `ParameterElement.name`, which is the public
                    // spelling — `correspondingPublicName`,
                    // token_impl.dart:364).
                    Some(formals) => (true, self.named_formal_for(&formals, name).is_some()),
                    None => (false, false),
                };
                if resolved && !defined {
                    let text = self.strings.resolve(name).to_string();
                    self.emit(SemanticCode::UndefinedNamedParameter, range, &text, "");
                }
                if !defined {
                    self.record_miss(name, range, false, NameUse::NamedArgument);
                }
                return;
            }
        }
        let Some(callee) = self.invokes.last().and_then(|slot| *slot) else {
            return;
        };
        let (shape, callee_name, qualified) = match self.rows.get(callee) {
            Some(row) => (row.shape.clone(), row.name, row.qualified),
            None => return,
        };
        let (resolved, defined) = match &shape {
            // The membership test exists only where the oracle's does: a real
            // top-level function's formal table. A class callee is a
            // constructor invocation (a constructor accepting the label is
            // silent — constructors sit outside the M1 model) and a getter
            // callee is a property access; both stay unresolved, mirroring
            // the arity path's filter.
            RowShape::Bare(_) => {
                // An invoked qualified selector (`c.m(b: 1)`) reaches its
                // labels before the receiver merge: the parser fires the dot
                // event after the send, so the latched row is still the
                // selector's placeholder — the `qualified` plain row sitting
                // directly on its receiver. Classify that pair now; the
                // deferred merge needs the stack untouched.
                if matches!(shape, RowShape::Bare(BareInfo::Plain { .. })) && qualified {
                    let recv = match callee
                        .checked_sub(1)
                        .and_then(|i| self.rows.get(i))
                        .map(|below| below.shape.clone())
                    {
                        Some(RowShape::Bare(info)) => Engine::receiver_of_bare(info),
                        Some(RowShape::Receiver(recv)) => recv,
                        _ => Receiver::Unknown,
                    };
                    let formals = self.member_formals_full(&recv, callee_name);
                    (
                        formals.is_some(),
                        formals.is_some_and(|f| self.named_formal_for(&f, name).is_some()),
                    )
                } else {
                    let decl = self
                        .top_level_decl(callee_name)
                        .filter(|d| d.kind == MicroKind::TopLevelFn && d.formals.is_some());
                    let defined = decl.as_ref().is_some_and(|d| {
                        d.formals
                            .as_deref()
                            .is_some_and(|f| self.named_formal_for(f, name).is_some())
                    });
                    (decl.is_some(), defined)
                }
            }
            RowShape::Member { recv } => {
                let formals = self.member_formals_full(recv, callee_name);
                (
                    formals.is_some(),
                    formals.is_some_and(|f| self.named_formal_for(&f, name).is_some()),
                )
            }
            _ => return,
        };
        if resolved && !defined {
            let text = self.strings.resolve(name).to_string();
            self.emit(SemanticCode::UndefinedNamedParameter, range, &text, "");
        }
        if !defined {
            // The names table carries every real miss (the membership test
            // itself is live).
            self.record_miss(name, range, false, NameUse::NamedArgument);
        }
    }

    /// The super-constructor callee an open argument group inside an
    /// initializer list binds to: `(enclosing container, selector)` —
    /// `None` = the unnamed face (`super(...)`, `super.new(...)`). The
    /// group's callee row reads back through the arg level's mark: a
    /// bare `super(` keeps its [`Receiver::Super`] row (the `invokes` link
    /// drops it — the keyword never resolves as a member), a folded
    /// `super.name(` selector row matches its [`PendingInitSuper`] entry.
    /// `None` when the open group's callee is neither shape (an ordinary
    /// member call nested in the list's expression, `x = super.m(a: 1)`).
    pub(crate) fn init_super_callee(&self) -> Option<(StringId, Option<StringId>)> {
        let level = self.arg_levels.last()?;
        // The group's callee row was latched at open (`ArgLevel.callee_row`
        // — `mark` alone cannot serve: `flush_argument` advances it past
        // each argument's own row).
        let head = self.rows.get(level.callee_row?)?;
        if let RowShape::Receiver(Receiver::Super { container, .. }) = &head.shape {
            return Some((*container, None));
        }
        // `super.name(`: the folded Value row's offset matches its
        // `PendingInitSuper.selector_offset` — only the invoked head shape
        // redirects (an `x = super.m(...)` expression callee folds the same
        // Value row but its candidate never armed `invoked`).
        self.pending_init_super
            .iter()
            .rev()
            .find(|c| c.invoked && c.head && c.selector_offset == head.range.offset)
            .map(|c| (c.container, Some(c.name)))
    }

    /// The resolved super-constructor's formal table (`ctor_formals`) for
    /// `check_named_argument`: `selector` picks the named table, `None` the
    /// unnamed face's key (the enclosing class's `superclass` spelling —
    /// `ctor_formals` keys the unnamed entry under the declared name).
    /// `None` = unresolvable superclass or a constructor the index does not
    /// model — the caller stays silent (the under-report posture; the
    /// oracle's own `undefined_constructor_in_initializer` owns that
    /// report). The table comes back owned so the caller can hand it to
    /// [`Engine::named_formal_for`] and still `emit` under `&mut self`.
    pub(crate) fn super_ctor_named_formals(
        &self,
        container: StringId,
        selector: Option<StringId>,
    ) -> Option<Formals> {
        let superclass = self
            .index
            .get(self.file)
            .and_then(|f| f.container(container))
            .and_then(|c| c.superclass)?;
        let (super_container, _) = resolve_container(
            self.graph,
            self.namespaces,
            self.index,
            self.lib,
            self.file,
            superclass,
        )?;
        super_container
            .ctor_formals
            .get(&selector.unwrap_or(superclass))
            .map(|formals| (**formals).clone())
    }

    // —— pass 2 ———————————————————————————————————————————————————————

    /// Resolves the recorded rows in source order and emits. The flow
    /// promotions (#2241) ride beside the read types: the member walk's
    /// receiver verdict re-decides against them, exactly like the staged
    /// families' teardown join.
    /// The deferred invocation face's pass-2 half (#2547): the callee's
    /// flow read type re-decides callability — a promotion that narrowed
    /// the local to a callable face (a `call` class, a function shape)
    /// drops the staged row; any other flow face (or none — the
    /// read-type fact missing) keeps the parse-time verdict.
    fn apply_deferred_invocation_faces(&mut self) {
        let deferred = std::mem::take(&mut self.deferred_invocation_faces);
        for (span, name) in deferred {
            let flow_ty = self.read_type_at(span);
            let callable = match flow_ty {
                Some(ty) => match ty {
                    Ty::Function { .. } => true,
                    Ty::Named { name: head, .. } => {
                        self.strings.resolve(head) == "Function"
                            || self.fn_var_sigs.contains_key(&name)
                            || self.alias_fn_sig(head).is_some()
                            || matches!(
                                self.resolved_member(head, self.intern("call")),
                                Some(entry) if entry.kind == MemberKind::Method
                            )
                    }
                    _ => false,
                },
                None => false,
            };
            if callable {
                continue;
            }
            let code = SemanticCode::InvocationOfNonFunctionExpression;
            self.report.usage.push(Diagnostic::new(
                code.severity(),
                code.code_info(),
                code.message("", ""),
                self.file,
                span,
                code.correction_message(""),
            ));
        }
    }

    /// The pass-2 re-bind of a promotion-latched initializer (#2539):
    /// each deferred local re-derives its initializer member's static
    /// type over the promoted receiver (the same re-decision
    /// [`Engine::rederived_receiver`] makes for the member row itself)
    /// and re-binds when the answer names a different type than the
    /// parse-time bind wrote. Runs before any row resolves, so every
    /// later face — the re-bound local's own member reads, the argument
    /// and return checks — reads the promoted accessor face. The answer
    /// rides [`Engine::receiver_static_ty`] on a synthetic row (the same
    /// single source the chained faces consult): an accessor read
    /// instantiates the declared return against the re-derived receiver,
    /// and an invoked member (`var l = c.fragments.toList()`) answers
    /// the instantiated SEND result, never the raw schema a bare
    /// `return_ty` read would leak.
    /// The shared re-bind direction gate: only a NARROWING face is a
    /// promotion (see the member arm's inline twin for the full
    /// rationale — the `!`/`as` widening binds keep their written face).
    fn promotion_narrows(&self, corrected: Ty, parse_ty: Ty) -> bool {
        match (corrected, parse_ty) {
            (
                Ty::Named {
                    name: c_head,
                    nullable: c_null,
                    ..
                },
                Ty::Named {
                    name: p_head,
                    nullable: p_null,
                    ..
                },
            ) => {
                let nullability_ok = !c_null || p_null;
                if p_head == self.intern("Object") {
                    nullability_ok && !c_null
                } else if c_head == p_head {
                    nullability_ok
                } else {
                    let non_null = |name: StringId| Ty::Named {
                        name,
                        nullable: false,
                        args: ArgListId::EMPTY,
                    };
                    !matches!(
                        self.declared_subtype_negative(&non_null(c_head), &non_null(p_head)),
                        Some(false)
                    )
                }
            }
            _ => false,
        }
    }

    fn apply_deferred_rebinds(&mut self) {
        let deferred = std::mem::take(&mut self.deferred_var_rebinds);
        for (decl_offset, member_offset, parse_ty) in deferred {
            let Some(index) = self
                .member_reads
                .binary_search_by(|read| read.offset.cmp(&member_offset))
                .ok()
            else {
                continue;
            };
            let read = self.member_reads[index];
            let recv = self.rederived_receiver(member_offset, read.recv);
            let Receiver::Typed { ty: recv_ty, .. } = recv else {
                continue;
            };
            if !matches!(recv_ty, Ty::Named { .. }) {
                continue;
            }
            // The invoked-member probe (the same byte test the chained
            // faces take): a member token directly followed by `(` is a
            // send, so the initializer's static type is the callee's
            // result face, not the accessor's declared return.
            let name = self.strings.resolve(read.name);
            let units: u32 = if name.is_ascii() {
                name.len() as u32
            } else {
                name.chars().map(char::len_utf16).sum::<usize>() as u32
            };
            let invoked = self.byte_at(read.offset + units) == Some(b'(');
            let row = Row {
                name: read.name,
                range: SourceRange {
                    offset: read.offset,
                    length: 0,
                },
                qualified: true,
                shape: RowShape::Member { recv },
                intent: if invoked {
                    Intent::Invoke
                } else {
                    Intent::Read
                },
                write_op: None,
                ty: Ty::UNKNOWN,
            };
            let mut corrected = self.receiver_static_ty(&row);
            // The await flatten the parse-time bind applies (#2506): an
            // initializer inside an await extent binds the FUTURE'S
            // argument (`final db = await open();`), and the re-derived
            // accessor face owes the same flatten — a raw `Future<T>`
            // re-bind would un-flatten the local.
            if self.await_operand_extents.iter().any(|&(start, end)| {
                member_offset >= start && member_offset < end
            }) {
                corrected = self.flatten(corrected);
            }
            // The promotion direction gate: only a NARROWING re-bind is a
            // promotion. The re-derived accessor's declared return can
            // WIDEN the parse-time face — a `!`-asserted initializer's
            // non-null bind, an `as`-cast bind — and the oracle keeps the
            // written face there (the assert/cast decided it), so the
            // re-bind must not replace it. `corrected` narrows `parse_ty`
            // exactly when its head's supertype chain names `parse_ty`'s
            // head and its nullability does not loosen.
            let narrows = match (corrected, parse_ty) {
                (
                    Ty::Named {
                        name: c_head,
                        nullable: c_null,
                        ..
                    },
                    Ty::Named {
                        name: p_head,
                        nullable: p_null,
                        ..
                    },
                ) => {
                    let nullability_ok = !c_null || p_null;
                    // `Object` is the universal root: every named face
                    // narrows it (the core heads' incomplete chains would
                    // answer `Missing` on the walk the resolved world
                    // cannot complete).
                    if p_head == self.intern("Object") {
                        nullability_ok && !c_null
                    } else if c_head == p_head {
                        nullability_ok
                    } else {
                        // The cross-head promotion question is subtype-ness
                        // — whether `c_head`'s supertype closure names
                        // `p_head` — never a member lookup: a subclass
                        // field's covariant override (`Impl.element`'s
                        // `Sub2` against the base getter's `Sub?`) has no
                        // member SPELLED `Sub`, so the member walk answered
                        // `Missing` and dropped every override-precision
                        // re-bind. `declared_subtype_negative`'s `None`
                        // covers subtype-or-undecidable (the conservative
                        // accept the old `ChainBroken` arm took); only a
                        // proven non-subtype rejects.
                        let non_null = |name: StringId| Ty::Named {
                            name,
                            nullable: false,
                            args: ArgListId::EMPTY,
                        };
                        !matches!(
                            self.declared_subtype_negative(
                                &non_null(c_head),
                                &non_null(p_head)
                            ),
                            Some(false)
                        )
                    }
                }
                _ => false,
            };
            if corrected != Ty::UNKNOWN && corrected != parse_ty && narrows {
                self.rebound_locals.insert(decl_offset, (parse_ty, corrected));
            }
        }
        // The bare-identifier initializer twin (#2567): `var x = p;` where
        // `p`'s flow state carries a promotion infers the PROMOTED type
        // (the official inference reads the initializer's static type —
        // the promoted face). The parse-time bind cannot see the
        // promotion (the read-type table fills after the parse pass), so
        // the declaration stages here and the pass-2 read type re-binds
        // under the same narrowing gate.
        let deferred_bare = std::mem::take(&mut self.deferred_bare_rebinds);
        for (decl_offset, span, parse_ty) in deferred_bare {
            let Some(corrected) = self.read_type_at(span) else {
                continue;
            };
            let narrows = self.promotion_narrows(corrected, parse_ty);
            if corrected != Ty::UNKNOWN && corrected != parse_ty && narrows {
                self.rebound_locals.insert(decl_offset, (parse_ty, corrected));
            }
        }
        // The deferred closure formals: the context schema recomputes over
        // the re-derived receiver face (a promotion-latched link below the
        // callee — `rebound.fragments.where((f) => f.isComplete)` — freezes
        // the stale schema entry at pass 1), and the corrected entry joins
        // the same `rebound_locals` map the var re-binds ride, so every
        // read of the formal re-derives through the re-bound face.
        let deferred_formals = std::mem::take(&mut self.deferred_closure_formals);
        for entry in deferred_formals {
            let Some(face) = self.recv_expression_ty(entry.below) else {
                continue;
            };
            let Ty::Named { name: head, .. } = face else {
                continue;
            };
            let recv = Receiver::Typed {
                type_name: head,
                ty: face,
            };
            // The row keeps its member-token offset across the merge (the
            // fold tiles the length, never the start), so the offset pairs
            // the captured callee identity.
            let corrected =
                self.closure_context_schema(recv, entry.callee_name, None, entry.slot, entry.formal);
            let Some(corrected) = corrected.filter(|ty| *ty != Ty::UNKNOWN && *ty != entry.parse_ty)
            else {
                continue;
            };
            // The same narrowing gate the var re-binds apply: the schema
            // entry must narrow the frozen pass-1 face, never widen it.
            let narrows = match (corrected, entry.parse_ty) {
                (
                    Ty::Named {
                        name: c_head,
                        nullable: c_null,
                        ..
                    },
                    Ty::Named {
                        name: p_head,
                        nullable: p_null,
                        ..
                    },
                ) => {
                    let nullability_ok = !c_null || p_null;
                    if p_head == self.intern("Object") {
                        nullability_ok && !c_null
                    } else if c_head == p_head {
                        nullability_ok
                    } else {
                        let non_null = |name: StringId| Ty::Named {
                            name,
                            nullable: false,
                            args: ArgListId::EMPTY,
                        };
                        !matches!(
                            self.declared_subtype_negative(&non_null(c_head), &non_null(p_head)),
                            Some(false)
                        )
                    }
                }
                _ => false,
            };
            if narrows {
                self.rebound_locals
                    .insert(entry.decl_offset, (entry.parse_ty, corrected));
            }
        }
    }

    pub(crate) fn resolve_rows(&mut self, promotions: &PromotionStaging) {
        self.apply_deferred_rebinds();
        self.apply_deferred_invocation_faces();
        self.build_ur_sites();
        let rows = std::mem::take(&mut self.rows);
        // The #2459 cascade-index target latches: a first-section `..[` fold
        // binds the cascade target row itself, and its push-time type is the
        // target's static type — the oracle resolves EVERY section's index
        // face against it (`x..[0] = 1..[1] = 2` reports both brackets on
        // the target's type). Continuation sections bind the same `..[` fold
        // to the previous section's RHS leftover row (the walk's newest row
        // before the bracket), so their target question reads this table
        // instead.
        let cascade_targets: Vec<(u32, Ty)> = rows
            .iter()
            .filter_map(|row| {
                let Intent::Operator(op, bracket) = row.intent else {
                    return None;
                };
                if op != self.intern("[]") {
                    return None;
                }
                if self.cascade_index_continuation(bracket.offset) {
                    return None;
                }
                let ty = row.ty;
                (ty != Ty::UNKNOWN).then_some((row.range.offset, ty))
            })
            .collect();
        let mut deferred = std::mem::take(&mut self.deferred_miss_rows);
        deferred.sort_by_key(|row| row.range.offset);
        // The fold-deferred rows interleave by offset: their emissions keep
        // the report's source order (later merges assume it).
        let mut index = 0usize;
        for row in &rows {
            // The conditional-URI face: rows sourced inside an `if (...)`
            // configuration resolve to nothing (the configuration is never
            // an expression; the parser's invalid_literal_in_configuration
            // span carries them).
            if self.conditional_uri_rows.contains(&row.range.offset) {
                continue;
            }
            while index < deferred.len() && deferred[index].range.offset <= row.range.offset {
                // The conditional-URI face gates the fold-deferred rows the
                // same way (their offsets are inside the window too).
                if !self
                    .conditional_uri_rows
                    .contains(&deferred[index].range.offset)
                {
                    self.resolve_row(promotions, &deferred[index], &cascade_targets);
                }
                index += 1;
            }
            self.resolve_row(promotions, row, &cascade_targets);
        }
        for deferred_row in &deferred[index..] {
            if !self
                .conditional_uri_rows
                .contains(&deferred[index].range.offset)
            {
                self.resolve_row(promotions, deferred_row, &cascade_targets);
            }
        }
    }

    /// One row's pass-2 dispatch (shared by the main stack and the
    /// fold-deferred rows).
    pub(crate) fn resolve_row(
        &mut self,
        promotions: &PromotionStaging,
        row: &Row,
        cascade_targets: &[(u32, Ty)],
    ) {
        // The #2459②③ cascade-index misfold: a CONTINUATION section's
        // `..[` fold binds the previous section's RHS leftover row
        // (`x..[0] = s..[1] = 2` folds the second bracket onto `s`, a
        // `JSObject()..['k'] = v.toJS` chain folds the next bracket onto
        // the `toJS` getter row) — the official walk binds every section's
        // index face to the CASCADE TARGET, never to that row. Answer the
        // target's own `[]`/`[]=` question at the bracket, then resolve the
        // row's pre-fold read face (the oracle still reads every leftover:
        // `s` stays a use, `toJS` resolves through its extension).
        if let Intent::Operator(op, bracket) = row.intent
            && op == self.intern("[]")
            && self.cascade_index_continuation(bracket.offset)
        {
            if let Some(target) = self.cascade_index_target_ty(bracket.offset, cascade_targets) {
                self.check_operator_expr(target, row);
            }
            let plain = Row {
                intent: Intent::Read,
                write_op: None,
                ..row.clone()
            };
            self.resolve_row(promotions, &plain, cascade_targets);
            return;
        }
        match &row.shape {
            RowShape::Bare(info) => self.resolve_bare_row(promotions, row, *info),
            RowShape::Member { recv } => {
                let recv = self.cascade_recv_receiver(row, *recv);
                let recv = self.promoted_receiver(row.range.offset, recv);
                let rebound_face = if matches!(recv, Receiver::Typed { .. })
                    && self
                        .member_reads
                        .binary_search_by(|read| read.offset.cmp(&row.range.offset))
                        .is_ok()
                {
                    self.rebound_local_receiver(row.range.offset, &recv)
                } else {
                    None
                };
                let recv =
                    if let Some(face) = rebound_face
                        && let Ty::Named { name, .. } = face
                    {
                        Receiver::Typed {
                            type_name: name,
                            ty: face,
                        }
                    } else {
                        recv
                    };
                let recv = self.tolerant_receiver(promotions, row, recv);
                self.resolve_member_row(promotions, row, recv);
            }
            RowShape::Receiver(recv) => self.resolve_receiver_row(row, *recv),
            RowShape::TypeUse(..) => self.resolve_type_row(row),
            RowShape::Record(fields) => self.resolve_record_row(row, fields),
            RowShape::Value => {}
        }
    }

    /// Whether a `;` separates two UTF-16 offsets — the same separator
    /// probe `merge_member`'s cascade window runs: a statement boundary
    /// proves a fresh cascade, while a `;` inside an expression's own spans
    /// (a `"a;b"` literal) is part of it.
    pub(crate) fn semicolon_between(&self, from: u32, to: u32) -> bool {
        let (start, end) = (self.byte_of(from), self.byte_of(to));
        let (lo, hi) = if start <= end { (start, end) } else { (end, start) };
        self.source.as_bytes()[lo..hi].contains(&b';')
    }

    /// Whether the `..[`-preceded bracket at `bracket_offset` belongs to a
    /// CONTINUATION index section. The FIRST section's `..[` fold binds the
    /// cascade target row itself (the walk's newest row before its bracket
    /// is the target), while every LATER section's fold binds the previous
    /// section's RHS leftover row — so the fold's legitimacy is the
    /// section's own position: walking back from the section operator at
    /// bracket depth 0, a preceding `..`/`?..` before any chain boundary
    /// (`;`, `,`, or the target's own opening group) marks a continuation.
    /// `false` also answers for every bracket a cascade section does not
    /// own (`..b[0]` reads the selector spelling).
    fn cascade_index_continuation(&self, bracket_offset: u32) -> bool {
        let mut b = self.byte_of(bracket_offset);
        let src = self.source.as_bytes();
        while b > 0 && src[b - 1].is_ascii_whitespace() {
            b -= 1;
        }
        if b < 2 || src[b - 1] != b'.' || src[b - 2] != b'.' {
            return false;
        }
        b -= 2; // the section's own `..`
        let mut depth = 0i32;
        while b > 0 {
            let c = src[b - 1];
            b -= 1;
            match c {
                b']' | b')' => depth += 1,
                b'[' | b'(' => {
                    if depth == 0 {
                        return false;
                    }
                    depth -= 1;
                }
                b';' | b',' | b'{' | b'}' if depth == 0 => return false,
                b'.' if depth == 0 => {
                    if b > 0 && src[b - 1] == b'.' {
                        return true;
                    }
                }
                _ => {}
            }
        }
        false
    }

    /// The static type of the cascade target an index section resolves
    /// against (#2459②③): the newest selector-section read inside the
    /// `;`-free window carries the latched target receiver
    /// (`C()..foo = 1..[0] = 2` answers on `C`), else the nearest
    /// first-section target fold's push-time type (`x..[0] = 1..[1] = 2`
    /// answers on the target). Every miss keeps the section silent — the
    /// under-report posture for the shapes no pass-2 record describes.
    fn cascade_index_target_ty(
        &self,
        bracket_offset: u32,
        cascade_targets: &[(u32, Ty)],
    ) -> Option<Ty> {
        if let Some(read) = self.member_reads.iter().rev().find(|read| {
            read.offset < bracket_offset
                && self.cascade_sections.contains(&read.offset)
                && !self.semicolon_between(read.offset, bracket_offset)
        }) {
            return cascade_target_ty_of(&read.recv);
        }
        cascade_targets
            .iter()
            .rev()
            .find(|(offset, _)| {
                *offset < bracket_offset && !self.semicolon_between(*offset, bracket_offset)
            })
            .map(|(_, ty)| *ty)
    }

    /// The #2459① cascade-RHS receiver: a member merge inside a cascade
    /// section's RHS or arguments that is NOT a section selector itself
    /// (`..sourceSpan = token.span`'s `span`, `..p = original.p`'s inner
    /// read) resolves through its own receiver upstream, but the merge-time
    /// cascade latch (S4-B) substitutes the latched cascade target verbatim
    /// whenever the popped row is a leftover at section level. The
    /// substitution is detectable here: the recorded receiver equals the
    /// newest section read's receiver exactly, and the row's recorded
    /// receiver span still points at the true receiver expression — the
    /// flow read type at that span re-derives the receiver the same way the
    /// inferred-slot face (#2346①) does. Section selectors keep the latched
    /// target (the S4-B face itself), and every other shape keeps the
    /// merge-time record byte for byte.
    fn cascade_recv_receiver(&self, row: &Row, recv: Receiver) -> Receiver {
        if self.cascade_sections.contains(&row.range.offset) {
            return recv;
        }
        let Some(src) = self.member_reads.iter().rev().find(|read| {
            read.offset < row.range.offset && self.cascade_sections.contains(&read.offset)
        }) else {
            return recv;
        };
        if self.semicolon_between(src.offset, row.range.offset) || src.recv != recv {
            return recv;
        }
        let Ok(index) = self
            .member_reads
            .binary_search_by(|read| read.offset.cmp(&row.range.offset))
        else {
            return recv;
        };
        let read_span = self.member_reads[index].recv_span;
        // The receiver expression's own face — the flow read type (with
        // its trailing `!` applied) or the chain link below.
        match self.recv_expression_ty(read_span) {
            Some(face @ Ty::Named { name: flow_head, .. }) => Receiver::Typed {
                type_name: flow_head,
                ty: face,
            },
            _ => recv,
        }
    }

    /// A surviving typed-keyword receiver (`this + b`, `super[i]`): only
    /// operator intents carry checks — member merges consumed every other
    /// shape at the dot event.
    pub(crate) fn resolve_receiver_row(&mut self, row: &Row, recv: Receiver) {
        if !matches!(row.intent, Intent::Operator(..)) {
            return;
        }
        let (read, write) = self.operator_spellings(row);
        match recv {
            Receiver::This { container, .. } => {
                // Inside `extension E on T`/`extension type E(T)`, `this`
                // carries the `on` type's static face — the operator
                // resolves through `T`'s member table, not `E`'s
                // (upstream `extension_member_resolver.dart`: `this[0]`
                // inside `extension on List` resolves `List.operator []`,
                // never an extension member). A class/mixin/enum's `this`
                // reads its own container unchanged.
                let resolved = match self.container_kind_of(container) {
                    Some(ContainerKind::Extension) => self
                        .extension_on_type_args(container)
                        .map_or(container, |(head, _)| head),
                    _ => container,
                };
                let mut missing: Vec<String> = Vec::new();
                for spelling in [read, write].into_iter().flatten() {
                    if matches!(self.walk_members(resolved, spelling), Walk::Missing) {
                        missing.push(self.strings.resolve(spelling).to_string());
                    }
                }
                if missing.is_empty() {
                    return;
                }
                let type_text = self.strings.resolve(resolved).to_string();
                for text in missing {
                    self.emit(
                        SemanticCode::UndefinedOperator,
                        row.range,
                        &text,
                        &type_text,
                    );
                }
            }
            Receiver::Super { container, .. } => {
                let mut missing: Vec<String> = Vec::new();
                for spelling in [read, write].into_iter().flatten() {
                    if matches!(self.walk_super(container, spelling), Walk::Missing) {
                        missing.push(self.strings.resolve(spelling).to_string());
                    }
                }
                if missing.is_empty() {
                    return;
                }
                let type_text = self.strings.resolve(container).to_string();
                for text in missing {
                    self.emit(
                        SemanticCode::UndefinedSuperMember(SuperShape::Operator),
                        row.range,
                        &text,
                        &type_text,
                    );
                }
            }
            // A folded creation/extension-override receiver (`C() + 1`,
            // `E(1) * 2`): the operator resolves against the container's
            // member face — extensions report their dedicated operator row.
            Receiver::Created { name, kind } => {
                let Intent::Operator(_, op_range) = row.intent else {
                    return;
                };
                let mut missing: Vec<String> = Vec::new();
                for spelling in [read, write].into_iter().flatten() {
                    if matches!(self.walk_members(name, spelling), Walk::Missing) {
                        missing.push(self.strings.resolve(spelling).to_string());
                    }
                }
                if missing.is_empty() {
                    return;
                }
                let host_text = self.strings.resolve(name).to_string();
                for text in missing {
                    let code = if kind == ContainerKind::Extension {
                        SemanticCode::UndefinedExtensionOperator
                    } else {
                        SemanticCode::UndefinedOperator
                    };
                    self.emit(code, op_range, &text, &host_text);
                }
                self.record_miss(name, op_range, true, NameUse::Read);
            }
            _ => {}
        }
    }

    /// The receiver re-decide a member row resolves against when the keyed
    /// [`Engine::promoted_receiver`] kept the push-time face. The flow's
    /// promoted read-type fact for an assignment-window receiver stages at
    /// the window's own shifted key (`target = target.julianday` — the
    /// staged row sits one byte left of the receiver read, while the
    /// unpromoted declared face holds the read's exact span), so the
    /// re-decide consults both spans under the same rules the keyed path
    /// applies — a named face that differs from the static type, minus the
    /// `?`-widened same-head pre-state — then falls back to the #2241
    /// promotion staging at the receiver read. The shifted window can only
    /// match rows staged at it (reads never overlap, so no other variable's
    /// row answers there); every miss keeps the keyed verdict.
    fn tolerant_receiver(
        &self,
        promotions: &PromotionStaging,
        row: &Row,
        recv: Receiver,
    ) -> Receiver {
        let Receiver::Typed {
            ty: static_ty,
            type_name,
            ..
        } = recv
        else {
            return recv;
        };
        let Ok(index) = self
            .promoted_recv_reads
            .binary_search_by(|(member, _)| member.cmp(&row.range.offset))
        else {
            return recv;
        };
        let read_span = self.promoted_recv_reads[index].1;
        // A re-bound local's face is authoritative (#2539): the flow read
        // face still spells the parse-time bind — the staging predates the
        // pass-2 promotion re-decision — so the re-decision below would
        // clobber the promoted accessor face with the stale declared one.
        if let Some(face) = self.rebound_local_ty(row.range.offset) {
            if face == static_ty {
                return recv;
            }
            if let Ty::Named { name, .. } = face {
                return Receiver::Typed {
                    type_name: name,
                    ty: face,
                };
            }
        }
        if let Some(redecided) = self.redecided_face(static_ty, self.read_type_at(read_span)) {
            return redecided;
        }
        // The assignment window's shifted staging key (`target` reads at
        // `offset`, its promoted face stages at `offset - 1`).
        if read_span.offset > 0 {
            let shifted = SourceRange {
                offset: read_span.offset - 1,
                length: read_span.length,
            };
            if let Some(redecided) = self.redecided_face(static_ty, self.read_type_at(shifted)) {
                return redecided;
            }
        }
        // The #2241 staging: an opaque marker carries only the nullability
        // decision (the parse-time type's non-null form stands), a spelled
        // face re-decides under the same widening guard.
        let Some(promoted) = promotions.promoted(read_span) else {
            return recv;
        };
        let redecided = match promoted {
            Ty::Unknown => {
                let non_null = Self::non_null_form(static_ty);
                (non_null != static_ty).then(|| Receiver::Typed {
                    type_name,
                    ty: non_null,
                })
            }
            Ty::Named { name, .. }
                if promoted != static_ty && !self.widened_face(&promoted, &static_ty) =>
            {
                Some(Receiver::Typed {
                    type_name: name,
                    ty: promoted,
                })
            }
            _ => None,
        };
        redecided.unwrap_or(recv)
    }

    /// The re-decide verdict of a flow read-type face: a named face that
    /// differs from the push-time static type re-decides the receiver,
    /// unless it merely `?`-widens the same head (the postfix-`!` pre-state
    /// — promotion never widens nullability).
    fn redecided_face(&self, static_ty: Ty, flow_ty: Option<Ty>) -> Option<Receiver> {
        let flow_ty = flow_ty?;
        let Ty::Named { name, .. } = flow_ty else {
            return None;
        };
        if flow_ty == static_ty || self.widened_face(&flow_ty, &static_ty) {
            return None;
        }
        Some(Receiver::Typed {
            type_name: name,
            ty: flow_ty,
        })
    }

    /// Whether `flow` is exactly `static`'s own head widened to nullable —
    /// the postfix-`!` pre-state signature, never a promotion.
    fn widened_face(&self, flow: &Ty, static_ty: &Ty) -> bool {
        matches!(
            (flow, static_ty),
            (
                Ty::Named {
                    name: flow_head,
                    nullable: true,
                    ..
                },
                Ty::Named {
                    name: static_head,
                    nullable: false,
                    ..
                },
            ) if flow_head == static_head
        )
    }

    /// One row's pass-2 dispatch (shared by the main stack and the
    /// fold-deferred rows). Bare rows carry the promotions too: their
    /// operator face re-decides against the flow-promoted type exactly
    /// like the member walk's receiver verdict.
    pub(crate) fn resolve_bare_row(
        &mut self,
        promotions: &PromotionStaging,
        row: &Row,
        info: BareInfo,
    ) {
        let name_text = self.strings.resolve(row.name);
        match info {
            // A prefix used bare: invoking a prefix is an error the oracle
            // names with an unfrozen code — record only. The bare prefix
            // reference still counts as a use of the prefix (`unused_import`
            // hasPrefixUsedWithoutName face).
            BareInfo::Container { .. } | BareInfo::Prefix { .. } => {
                if !self.self_scope_rows.contains(&row.range.offset) {
                    self.used_names.insert(row.name);
                }
                // The type-literal operator face: the operator applies to
                // the literal's static type `Type`, which declares none of
                // the user operators (`List[null]` reports
                // `undefined_operator '[]'` on `Type` — issue_45251_list;
                // upstream Dart SDK source, pkg/analyzer/lib/src/dart/
                // ast/ast.dart `TypeLiteral.staticType`). Anchored on the
                // operator's own span (the bracket pair for index shapes).
                if matches!(info, BareInfo::Container { .. })
                    && let Intent::Operator(_, op_range) = row.intent
                {
                    let (read, write) = self.operator_spellings(row);
                    for spelling in [read, write].into_iter().flatten() {
                        if spelling == self.intern("==")
                            || self
                                .index
                                .get(self.file)
                                .is_some_and(|f| f.has_extension_member(spelling))
                        {
                            continue;
                        }
                        let op_text = self.strings.resolve(spelling).to_string();
                        self.emit(SemanticCode::UndefinedOperator, op_range, &op_text, "Type");
                    }
                    self.record_miss(row.name, row.range, true, NameUse::Read);
                }
            }
            BareInfo::Plain { ty } => {
                // Any resolved reference uses the name it resolved to (the
                // official `_useIdentifierElement` / `members` faces; writes
                // included — only the local-variable rows distinguish). The
                // parse-time self-reference exclusions skip their rows.
                if !self.self_scope_rows.contains(&row.range.offset) {
                    self.used_names.insert(row.name);
                }
                // The implicit-this face (`_checkForInvalidInstanceMember
                // Access2`, upstream Dart SDK source, pkg/analyzer/lib/src/
                // generated/error_verifier.dart — `ThisContext.
                // instanceFieldDeclaration` disallows `this`): a bare READ
                // in an instance field initializer resolving to an instance
                // member of the enclosing container reports at the name.
                // Both facts snapshot at push time (the field group and the
                // container stack tear down before pass 2).
                if !matches!(row.intent, Intent::Write | Intent::ReadWrite)
                    && self.field_init_this_rows.contains(&row.range.offset)
                {
                    self.emit(
                        SemanticCode::ImplicitThisReferenceInInitializer,
                        row.range,
                        name_text,
                        "",
                    );
                }
                // The discarded-value face (`_check` + `checkNameExpression`,
                // use_result_verifier.dart): an invocation or bare read of a
                // UseResult-carrying top-level declaration, not sealed used.
                // A local shadowing the name resolves to the local here —
                // the model carries no local annotations, so the face
                // lookup answers from the outer declaration and may
                // over-report that shadow (documented divergence).
                if matches!(row.intent, Intent::Invoke | Intent::Read)
                    && !self.self_scope_rows.contains(&row.range.offset)
                {
                    self.check_top_level_unused_result(row);
                }
                // A non-write resolution of a field name is the implicit
                // getter's read (writes never spare a field) — unless the
                // statement window discarded the read (`_isReadIdentifier`'s
                // false arm, upstream Dart SDK source,
                // pkg/analyzer/lib/src/error/unused_local_elements_verifier
                // .dart:418-440: a bare compound-assignment LHS, prefix, or
                // postfix operand whose parent is the ExpressionStatement
                // joins no read set, so `_x += 1;`/`_x++;`/`-_x;` leave the
                // field unused while `??=` and member targets keep theirs).
                // Inside an extension body `this` reads the `on` type's
                // member — the owner credits the extended container, not
                // the extension (the same face `note_member_read`'s `This`
                // arm carries).
                if !matches!(row.intent, Intent::Write)
                    && !self.statement_discarded_rows.contains(&row.range.offset)
                {
                    if let Some(&pushed) = self.field_read_rows.get(&row.range.offset) {
                        self.read_names.insert(row.name);
                        let owner = self
                            .container_context()
                            .and_then(|(container, kind)| {
                                (kind == ContainerKind::Extension)
                                    .then(|| self.enclosing_extension_on_type(container))
                                    .flatten()
                            })
                            .or_else(|| self.read_owner_container(row.range.offset))
                            .or(Some(pushed));
                        if let Some(container) = owner {
                            self.member_read_owners
                                .entry(row.name)
                                .or_default()
                                .insert((self.file, container));
                        }
                    } else if let Some(on) = self
                        .container_context()
                        .filter(|&(_, kind)| kind == ContainerKind::Extension)
                        .map(|(container, _)| container)
                        .and_then(|container| self.enclosing_extension_on_type(container))
                    {
                        // The extension body's bare read of an `on`-type
                        // field never lands in `field_read_rows` (the
                        // resolution join keys only the extension's own
                        // fields), yet it is the implicit getter's read the
                        // unused-field face credits — join the extended
                        // container here (`this` inside an extension IS the
                        // `on` type).
                        if let Walk::Found(entry) = self.walk_members(on, row.name)
                            && entry.kind == MemberKind::Field
                            && !entry.is_static
                        {
                            self.read_names.insert(row.name);
                            self.member_read_owners
                                .entry(row.name)
                                .or_default()
                                .insert((self.file, on));
                        }
                    }
                }
                // A direct operator application on a declared-typed receiver
                // (`c + d`, `c[i] = v`). The flow promotion's face re-decides
                // first (#2241's join, the same face the member walk's
                // receiver verdict reads): `if (sub is List) sub[i]` resolves
                // `[]` against the promoted `List`, not the declared
                // `Object?`. An opaque staged marker carries only the
                // nullability decision (#2174) — the parse-time type's
                // non-null form stands. A type whose static type the model
                // leaves unknown is the SDK projection's names-only stub —
                // for the dart:core collection heads that is the generic
                // class type literal (`List < 1`), and the operator applies
                // to the literal's static type `Type`, which declares none
                // of the user operators (the `A < 1` container face's
                // decision, one resolution grade down).
                if matches!(row.intent, Intent::Operator(..)) {
                    let ty = match promotions.promoted(row.range) {
                        Some(Ty::Unknown) => Self::non_null_form(ty),
                        Some(promoted) => promoted,
                        None => ty,
                    };
                    if ty == Ty::UNKNOWN && self.sdk_collection_type_literal(row.name) {
                        let Intent::Operator(_, op_range) = row.intent else {
                            return;
                        };
                        let (read, write) = self.operator_spellings(row);
                        for spelling in [read, write].into_iter().flatten() {
                            if spelling == self.intern("==")
                                || self
                                    .index
                                    .get(self.file)
                                    .is_some_and(|f| f.has_extension_member(spelling))
                            {
                                continue;
                            }
                            let op_text = self.strings.resolve(spelling).to_string();
                            self.emit(SemanticCode::UndefinedOperator, op_range, &op_text, "Type");
                        }
                    } else {
                        self.check_operator_expr(ty, row);
                    }
                }
            }
            BareInfo::Missing {
                this_type,
                in_exec,
                on_opaque,
                ext_invoke,
            } => {
                // Mixin `on` constraints and extension `on` types are
                // invisible to the model: inside their instance members a
                // bare miss may be a constraint member — record only.
                if on_opaque {
                    self.record_miss(row.name, row.range, false, use_kind_of(&row.intent));
                    return;
                }
                match row.intent {
                    Intent::Invoke => {
                        // The extension face decides first (its slot is
                        // `Some` only inside an `extension` body): a
                        // resolved `on` type's `this` hosts the miss
                        // (`undefined_method` for 'C' of `on C`, '<unknown>'
                        // of `on (int, int)`), an `InvalidType`/nullable
                        // `this` reports nothing (upstream
                        // `unpromotedThisType` — the oracle's lookup dies on
                        // the invalid extended type).
                        if let Some(host) = ext_invoke {
                            if let Some(container) = host {
                                let type_text = self.strings.resolve(container).to_string();
                                if !Self::synthetic_reference(row.range)
                                    && !self.keyword_bare_reference(row.range)
                                {
                                    self.emit(
                                        SemanticCode::UndefinedMethod,
                                        row.range,
                                        name_text,
                                        &type_text,
                                    );
                                }
                            }
                            self.record_miss(row.name, row.range, false, NameUse::Invoke);
                            return;
                        }
                        // UnqualifiedLookupDomain: inside a class or enum
                        // instance member the invocation miss names `this`'s
                        // type (`undefined_method`); everywhere else it is
                        // `undefined_function`.
                        if let Some(container) = this_type {
                            let type_text = self.strings.resolve(container).to_string();
                            // `_reportUndefinedMethodOrNew` is guard-free in
                            // the observed oracle (analyzer 8.1.1
                            // method_invocation_resolver.dart:424-431): a
                            // `show`-listed name of a broken import does NOT
                            // suppress `undefined_method` — only
                            // `undefined_function`/`undefined_identifier`
                            // consult `shouldIgnoreUndefined`.
                            if !Self::synthetic_reference(row.range)
                                && !self.keyword_bare_reference(row.range)
                            {
                                self.emit(
                                    SemanticCode::UndefinedMethod,
                                    row.range,
                                    name_text,
                                    &type_text,
                                );
                            }
                        } else if !Self::synthetic_reference(row.range)
                            && !self.keyword_bare_reference(row.range)
                            && !self.guard.ignore(None, row.name)
                        {
                            self.emit(SemanticCode::UndefinedFunction, row.range, name_text, "");
                        }
                        self.record_miss(row.name, row.range, false, NameUse::Invoke);
                    }
                    Intent::Read if row.name == self.intern("await") && in_exec => {
                        // `await` in a sync body is never a lookup miss: the
                        // frozen code names the missing async marker. The
                        // oracle's arm fires before the guard is consulted
                        // (simple_identifier_resolver.dart 8.1.1:217-221 —
                        // `undefined_identifier_await` reports unconditionally,
                        // the `shouldIgnoreUndefinedIdentifier` check rides
                        // the `else if` below it).
                        self.emit(SemanticCode::UndefinedIdentifierAwait, row.range, "", "");
                        self.record_miss(row.name, row.range, false, NameUse::Read);
                    }
                    Intent::Read | Intent::Write | Intent::ReadWrite => {
                        // A write to a defined getter-only name is under-reported
                        // (the oracle's setter code needs a receiver type); a
                        // fully missing name reports. A write pushed inside a
                        // constructor-initializer list is the field-initializer
                        // LHS — the oracle codes it through the dedicated
                        // initializer face (`_checkForInvalidField`,
                        // error_verifier.dart: `initializer_for_non_existent_
                        // field` / `field_initialized_outside_declaring_
                        // class`), never `undefined_identifier`.
                        let is_write = matches!(row.intent, Intent::Write | Intent::ReadWrite);
                        let initializer_container = is_write
                            .then(|| self.init_rows.get(&row.range.offset))
                            .flatten()
                            .copied();
                        if let Some(container) = initializer_container {
                            // `_checkForInvalidField` reads the ENCLOSING
                            // class's own field map (`enclosingInstanceElement
                            // .getField` — superclass fields never initialize
                            // here): a miss or a non-field member (method,
                            // getter/setter) codes the whole initializer. The
                            // static-field arm (`initializer_for_static_
                            // field`) has no code-table variant yet — that hit
                            // stays silent rather than mis-code.
                            let own_field = resolve_container(
                                self.graph,
                                self.namespaces,
                                self.index,
                                self.lib,
                                self.file,
                                container,
                            )
                            .and_then(|(decl, _)| decl.members.get(&row.name))
                            .is_some_and(|entry| {
                                entry.kind == MemberKind::Field && !entry.is_static
                            });
                            if !own_field {
                                self.emit(
                                    SemanticCode::InitializerForNonExistentField,
                                    row.range,
                                    name_text,
                                    "",
                                );
                            }
                        } else if !Self::synthetic_reference(row.range)
                            && !self.keyword_bare_reference(row.range)
                            && !self.guard.ignore(None, row.name)
                            && (initializer_container.is_some()
                                || !self.bare_write_target_exists(row.name, this_type))
                        {
                            self.emit(SemanticCode::UndefinedIdentifier, row.range, name_text, "");
                        }
                        let use_kind = match row.intent {
                            Intent::Write | Intent::ReadWrite => NameUse::Write,
                            _ => NameUse::Read,
                        };
                        self.record_miss(row.name, row.range, false, use_kind);
                    }
                    Intent::Operator(..) => {
                        // The undefined operand reports itself (`missing + 1`'s
                        // undefined_identifier at the head — the
                        // simple-identifier face the operator's own resolution
                        // rides): the same guarded emission the Read arm
                        // carries (the write-initializer clause is a
                        // write-only posture no operator head takes). The
                        // operator face never cascades a second row — the
                        // invalid operand's dynamic face swallows the
                        // operator lookup (probed: `missing + 1` reports the
                        // identifier alone, `missing[0]` likewise).
                        if !Self::synthetic_reference(row.range)
                            && !self.keyword_bare_reference(row.range)
                            && !self.guard.ignore(None, row.name)
                            && !self.bare_write_target_exists(row.name, this_type)
                        {
                            self.emit(SemanticCode::UndefinedIdentifier, row.range, name_text, "");
                        }
                        self.record_miss(row.name, row.range, false, NameUse::Read);
                    }
                }
            }
        }
    }

    /// Emits a type-position row from its push-time verdict snapshot.
    pub(crate) fn resolve_type_row(&mut self, row: &Row) {
        let RowShape::TypeUse(verdict, suppressed) = row.shape else {
            return;
        };
        let name_text = self.strings.resolve(row.name);
        match verdict {
            TypeVerdict::PrefixHead => {
                // The declaration-head keyword skip rides this face too: a
                // prefix head can never spell `dynamic`/`var`/`final`/`const`/
                // `void` (no legal parse writes one — `dynamic` answers the
                // top type before any prefix question upstream), so a row
                // carrying that spelling is parser-recovery noise and the
                // oracle reports nothing (probe: dio dio_mixin.dart:419
                // `if (T != dynamic &&` — the recovered `dynamic` reported
                // `undefined_identifier` here while `dart analyze` stays
                // silent; the value-position gate does not reach a TypeUse
                // row).
                if !Self::synthetic_reference(row.range)
                    && !self.keyword_bare_reference(row.range)
                    && !self.guard.ignore(None, row.name)
                {
                    self.emit(SemanticCode::UndefinedIdentifier, row.range, name_text, "");
                }
                self.record_miss(row.name, row.range, false, NameUse::Read);
            }
            TypeVerdict::Prefixed { prefix, local } => {
                // The local-prefix face: a prefix head that names a local
                // or parameter element is not an import prefix — the
                // oracle's prefixed lookup fails on the head and asks no
                // question of the continuation, so the miss stays silent
                // (upstream Dart SDK source, pkg/analyzer/lib/src/dart/
                // resolver/named_type_resolver.dart — the importPrefix
                // gate leaves the NamedType unresolved with no second
                // row). #1444: an extension-type clause position stays
                // silent too — the official clause-parent guard defers
                // the face.
                if !local
                    && !self.guard.ignore(Some(prefix), row.name)
                    && !Self::take_extension_clause_miss(
                        &mut self.extension_clause_miss,
                        row.range.offset,
                    )
                {
                    self.emit(SemanticCode::UndefinedClass, row.range, name_text, "");
                }
                self.record_miss(row.name, row.range, true, NameUse::Type);
            }
            // The plain path's non-type face: the name resolved to a
            // non-type element (`reportNullOrNonTypeElement`'s tail,
            // named_type_resolver.dart:794-806). A type-argument position
            // replaces it with `non_type_as_type_argument` (the same
            // context chain the miss rows ride); the `is`/`as` suppression
            // does not apply — the oracle reports the resolver face there
            // too (the pinned `x isn't a type` on a parameter inside an
            // `is` operand).
            TypeVerdict::NonType => {
                if Self::take_context_miss(&mut self.type_arg_miss_rows, row.range.offset) {
                    self.emit_corrected(SemanticCode::NonTypeAsTypeArgument, row.range, name_text);
                // The clause-parent suppression covers resolved non-types
                // too (named_type_resolver.dart:772-778 returns before the
                // `element != null` notAType arm): a clause entry that names
                // a variable or function reports the clause code instead
                // (scope_analyzer.dart:1195-1228 — the element exists but is
                // no ClassElement/MixinElement, so the clause switch
                // decides).
                } else if let Some(tag) =
                    Self::take_clause_miss(&mut self.clause_miss, row.range.offset)
                {
                    self.emit_clause_miss(tag, row.range, name_text);
                } else if let Some((anchor_file, anchor_range)) =
                    Self::take_decl_anchor(&mut self.non_type_anchors, row.range.offset)
                {
                    // The declaration row rides when the resolved element's
                    // fragment carries a location
                    // (named_type_resolver.dart:794-806).
                    self.emit_with_context(
                        SemanticCode::NotAType,
                        row.range,
                        name_text,
                        "",
                        vec![ContextMessage {
                            message: format!("The declaration of '{name_text}' is here.").into(),
                            file: anchor_file,
                            range: anchor_range,
                        }],
                    );
                } else {
                    self.emit(SemanticCode::NotAType, row.range, name_text, "");
                }
            }
            TypeVerdict::RedirectTarget => {
                // `redirect_to_non_class` is the oracle's answer here and it
                // is outside the frozen gate — record the miss, emit nothing.
                self.record_miss(row.name, row.range, false, NameUse::Type);
            }
            TypeVerdict::Missing => {
                if !suppressed && !self.guard.ignore(None, row.name) {
                    // A declaration parse keeps `await` in the type position:
                    // the unresolved simple name carries the await code, not
                    // `undefined_class` (named_type_resolver.dart:814-818 —
                    // `importPrefix == null && lexeme == 'await'` fires
                    // `undefinedIdentifierAwait` at the NamedType and
                    // returns before the `undefinedClass` fallthrough; the
                    // message names `await` itself, no arguments).
                    if row.name == self.intern("await") {
                        self.emit(SemanticCode::UndefinedIdentifierAwait, row.range, "", "");
                    }
                    // #1314 FAM13: the context chain replaces the
                    // `undefined_class` fallthrough — the type-argument
                    // face first, then the creation head
                    // (named_type_resolver.dart:760-770; both replacements
                    // pinned by the oracle: only the context row lands at
                    // the name).
                    else if Self::take_context_miss(
                        &mut self.type_arg_miss_rows,
                        row.range.offset,
                    ) {
                        self.emit_corrected(
                            SemanticCode::NonTypeAsTypeArgument,
                            row.range,
                            name_text,
                        );
                    } else if let Some(explicit) =
                        Self::take_keyed_miss(&mut self.creation_miss_rows, row.range.offset)
                    {
                        // An explicit `new`/`const` creation head reports
                        // `creation_with_non_type` on every miss — the
                        // `ConstructorInvocation` parent routes both the
                        // unresolved name and the resolved-non-class shape
                        // (upstream Dart SDK source, named_type_resolver.dart
                        // :285-289/:313-330). An implicit `C(...)` head keeps
                        // the split: a resolved non-class reports
                        // `creation_with_non_type`, an unresolved name
                        // `undefined_class`.
                        if explicit
                            || matches!(self.resolve_bare(row.name).0, BareInfo::Plain { .. })
                        {
                            self.emit_corrected(
                                SemanticCode::CreationWithNonType,
                                row.range,
                                name_text,
                            );
                        } else {
                            self.emit_corrected(SemanticCode::UndefinedClass, row.range, name_text);
                        }
                    }
                    // The catch clause's `on`-type row: the F-P recovered
                    // shapes code `non_type_in_catch_clause`, never the
                    // resolver's `undefined_class` (the parser's gate
                    // tripwires pin the routing; nonTypeInCatchClause,
                    // messages.yaml:13748-13753).
                    else if Self::take_context_miss(
                        &mut self.catch_on_miss_rows,
                        row.range.offset,
                    ) {
                        self.emit_corrected(
                            SemanticCode::NonTypeInCatchClause,
                            row.range,
                            name_text,
                        );
                    }
                    // #1314 FAM14: the clause faces replace the fallthrough
                    // (scope_analyzer.dart `_resolveType`'s clause switch);
                    // the `on` constraint is additive — the oracle keeps
                    // the resolver's `undefined_class` beside it.
                    else if let Some(tag) =
                        Self::take_clause_miss(&mut self.clause_miss, row.range.offset)
                    {
                        self.emit_clause_miss(tag, row.range, name_text);
                    }
                    // #1444: an extension-type header clause never reports
                    // `undefined_class` from name resolution — the official
                    // clause-parent guard defers (see the ledger note), so
                    // the hit consumes silently.
                    else if Self::take_extension_clause_miss(
                        &mut self.extension_clause_miss,
                        row.range.offset,
                    ) {
                    } else {
                        self.emit(SemanticCode::UndefinedClass, row.range, name_text, "");
                    }
                }
                self.record_miss(row.name, row.range, false, NameUse::Type);
            }
        }
    }

    /// Consumes one recorded context-miss offset (#1314 FAM13): the rows
    /// ascend with parse order, so a binary hit removes exactly the matching
    /// reference.
    pub(crate) fn take_context_miss(rows: &mut Vec<u32>, offset: u32) -> bool {
        match rows.binary_search(&offset) {
            Ok(index) => {
                rows.remove(index);
                true
            }
            Err(_) => false,
        }
    }

    /// Consumes one recorded offset out of a keyed miss table (the
    /// creation-head explicit flag — the same ascending binary hit as
    /// [`Self::take_context_miss`], returning the carried tag).
    pub(crate) fn take_keyed_miss(rows: &mut Vec<(u32, bool)>, offset: u32) -> Option<bool> {
        match rows.binary_search_by_key(&offset, |entry| entry.0) {
            Ok(index) => Some(rows.remove(index).1),
            Err(_) => None,
        }
    }

    /// Consumes one recorded non-type declaration anchor (the `not_a_type`
    /// context face): the same ascending binary hit as
    /// [`Self::take_keyed_miss`], returning the carried (file, span) pair.
    pub(crate) fn take_decl_anchor(
        rows: &mut Vec<(u32, (FileId, SourceRange))>,
        offset: u32,
    ) -> Option<(FileId, SourceRange)> {
        match rows.binary_search_by_key(&offset, |entry| entry.0) {
            Ok(index) => Some(rows.remove(index).1),
            Err(_) => None,
        }
    }

    /// Consumes one recorded clause-entry offset (#1314 FAM14): the same
    /// ascending binary hit, returning the clause face.
    pub(crate) fn take_clause_miss(
        rows: &mut Vec<(u32, ClauseTag)>,
        offset: u32,
    ) -> Option<ClauseTag> {
        match rows.binary_search_by_key(&offset, |entry| entry.0) {
            Ok(index) => Some(rows.remove(index).1),
            Err(_) => None,
        }
    }

    /// Inserts one clause-entry miss keeping the ledger's ascending order
    /// (the binary consumption above requires it): out-of-order clause
    /// recovery pushes an extends head whose offset precedes already-fed
    /// `implements`/`with` entries.
    pub(crate) fn push_clause_miss(&mut self, offset: u32, tag: ClauseTag) {
        let at = self
            .clause_miss
            .partition_point(|(existing, _)| *existing < offset);
        self.clause_miss.insert(at, (offset, tag));
    }

    /// Emits a clause-entry miss through its ledger tag
    /// (scope_analyzer.dart `_resolveType`'s clause switch): the `on`
    /// constraint is additive — the oracle keeps the resolver's
    /// `undefined_class` beside `mixin_super_class_constraint_non_interface`.
    pub(crate) fn emit_clause_miss(&mut self, tag: ClauseTag, range: SourceRange, name_text: &str) {
        match tag {
            ClauseTag::With => {
                self.emit(SemanticCode::MixinOfNonClass, range, "", "");
            }
            // The two clause faces carrying oracle correctionMessages
            // (codes.rs `correction_message`): the emit helper pushes no
            // correction, so these route through the correction-carrying
            // emit.
            ClauseTag::Implements => {
                let code = SemanticCode::ImplementsNonClass;
                let message = code.message("", "");
                self.emit_message_with_correction(code, range, message);
            }
            ClauseTag::Extends => {
                let code = SemanticCode::ExtendsNonClass;
                let message = code.message("", "");
                self.emit_message_with_correction(code, range, message);
            }
            ClauseTag::AliasSuper => {
                self.emit(SemanticCode::MixinWithNonClassSuperclass, range, "", "");
            }
            ClauseTag::OnConstraint => {
                self.emit(
                    SemanticCode::MixinSuperClassConstraintNonInterface,
                    range,
                    "",
                    "",
                );
                self.emit(SemanticCode::UndefinedClass, range, name_text, "");
            }
        }
    }

    /// Consumes one recorded extension-type clause entry (#1444): the same
    /// ascending binary hit; a hit is silent.
    pub(crate) fn take_extension_clause_miss(rows: &mut Vec<u32>, offset: u32) -> bool {
        match rows.binary_search(&offset) {
            Ok(index) => {
                rows.remove(index);
                true
            }
            Err(_) => false,
        }
    }

    /// The synthetic-token reference skip: a row whose token span is empty
    /// is a recovery-inserted identifier — the oracle's resolver walks real
    /// identifier tokens only and reports nothing for one (upstream Dart
    /// SDK source, pkg/analyzer/lib/src/dart/element/element.dart — a
    /// synthetic identifier token carries no element to resolve against).
    pub(crate) fn synthetic_reference(range: SourceRange) -> bool {
        range.length == 0
    }

    /// The keyword-token bare-reference skip: the statement lane leaves a
    /// The declaration-start keyword gate: a bare row whose source text is
    /// one of `var`/`final`/`const`/`dynamic`/`void` stands where the oracle
    /// parsed a declaration head, not an expression identifier — its
    /// recovery reports no `undefined_identifier` for the standing token
    /// (probe: `var;`/`final;`/`const;`/`dynamic;`/`void;` emit nothing
    /// upstream). Every other keyword consumed as a name DOES resolve —
    /// `ensureIdentifier` returns `isIdentifier` spellings directly
    /// (upstream Dart SDK source, pkg/_fe_analyzer_shared/lib/src/parser/
    /// identifier_context_impl.dart:397) and `isKeywordOrIdentifier`
    /// fallbacks are consumed with `expectedIdentifierButGotKeyword`
    /// (:435-440), so `assert`, `class`, `in`, `as`, `when`, `late`,
    /// `static`, `covariant` all report `undefined_identifier` upstream.
    /// Member faces keep this check off: a built-in identifier is a legal
    /// member name (`obj.late`).
    pub(crate) fn keyword_bare_reference(&self, range: SourceRange) -> bool {
        let start = self.byte_of(range.offset);
        let end = self.byte_of(range.offset + range.length);
        self.source
            .get(start..end)
            .is_some_and(|text| matches!(text, "var" | "final" | "const" | "dynamic" | "void"))
    }

    /// The static type of one receiver EXPRESSION: the flow read-type face
    /// first (the #2346① leg — a promoted local's read answers at its own
    /// span), then the chained-member face — a receiver that is itself a
    /// merged member read (`e.supertype.m`'s `e.supertype`, `a.b.c`'s
    /// `a.b`) owns the accessor's declared return even when the merge froze
    /// the leaf's receiver `Unknown` because the predecessor hop resolved
    /// on the declared (unpromoted) receiver type. The merge records the
    /// receiver's span as the receiver ROW's own range — for a chained
    /// member node that is the node's member token — so the chain leg
    /// matches the node's staged read record by its token's end and
    /// re-derives its receiver through this same face
    /// ([`Self::receiver_static_ty`] then answers getters, fields, and
    /// invocation returns alike). Each backward hop matches a strictly
    /// earlier token, so the walk descends the chain monotonically and
    /// terminates.
    pub(crate) fn recv_expression_ty(&self, read_span: SourceRange) -> Option<Ty> {
        self.recv_expression_face(read_span)
            .map(|ty| self.null_asserted_after(read_span, ty))
    }

    /// [`Self::recv_expression_ty`] before the trailing null-assert: the
    /// flow read face, then the chained-member face.
    fn recv_expression_face(&self, read_span: SourceRange) -> Option<Ty> {
        // The re-bound local's corrected face outranks the raw flow read:
        // the flow layer's declared slot staged the parse-time bind
        // (#2140) and pass 2's re-bind (#2539) is the only holder of the
        // promoted accessor face — without this layer a `var` chain
        // (`var a = f.element; var b = a.m`) re-derives `b`'s initializer
        // against `a`'s stale parse face and the whole chain rides the
        // base declaration. The helper's own gate keeps a genuinely
        // promoted read (a flow face past the parse bind) on its flow
        // face.
        if let Some(ty) = self.rebound_read_face(read_span) {
            return Some(ty);
        }
        if let Some(ty) = self.read_type_at(read_span) {
            return Some(ty);
        }
        // The chain leg reads the staged member-read records only: a chain
        // node's own record sits at its member token, whose span END equals
        // the receiver span the child merge recorded (`e.supertype.m`'s
        // leaf names `supertype`'s end) — so the walk matches the node by
        // its member token's end and descends through each record's
        // `recv_span`. Offsets are UTF-16 units, so the token end is the
        // name's UTF-16 length past its offset.
        let end = read_span.offset + read_span.length;
        let token_end = |read: &MemberRead| {
            let name = self.strings.resolve(read.name);
            let units: usize = if name.is_ascii() {
                name.len()
            } else {
                name.chars().map(char::len_utf16).sum()
            };
            read.offset as usize + units == end as usize
        };
        let prev = self
            .member_reads
            .iter()
            .rev()
            .find(|read| read.offset >= read_span.offset && token_end(read))?;
        self.chained_read_static_ty(prev)
    }

    /// The re-derived static type of one chained member node, keyed by its
    /// staged read record: the receiver re-reads through
    /// [`Self::recv_expression_ty`] — the flow face wins because the
    /// record's merge-time receiver carries the declared, unpromoted type
    /// — with the record's classified receiver as the flow-blind
    /// fallback, and the accessor face answers over the re-typed
    /// receiver — [`Self::receiver_static_ty`] on a synthetic row whose
    /// receiver carries the re-derived type, so the
    /// getter/field/tear-off/invocation distinctions stay in one place.
    fn chained_read_static_ty(&self, read: &MemberRead) -> Option<Ty> {
        // The re-derived face wins: the record's merge-time receiver carries
        // the UNPROMOTED static type (the promotion re-decision #1465 keeps
        // the declared face when the flow answers only in pass 2), so the
        // flow read at the receiver's own span — the same face the node's
        // own member verdict runs on — re-types it first. The record's
        // classified receiver remains the fallback for the shapes the flow
        // layer cannot spell (`this`, a created receiver).
        let recv_ty = self.recv_expression_ty(read.recv_span).or(match &read.recv {
            Receiver::Typed { ty, .. } => Some(*ty),
            Receiver::This {
                container, kind, ..
            } => Some(self.this_static_ty(*container, *kind)),
            Receiver::Created {
                name,
                kind: ContainerKind::Class,
            } => Some(Ty::named(*name)),
            _ => None,
        })?;
        let Ty::Named { name, .. } = recv_ty else {
            return None;
        };
        let row = Row {
            name: read.name,
            range: SourceRange {
                offset: read.offset,
                length: 0,
            },
            qualified: true,
            shape: RowShape::Member {
                recv: Receiver::Typed {
                    type_name: name,
                    ty: recv_ty,
                },
            },
            intent: Intent::Read,
            write_op: None,
            ty: Ty::UNKNOWN,
        };
        let ty = self.receiver_static_ty(&row);
        // An invoked callee's chain node is the SEND, not the read: a
        // member token followed by `(` (the same byte probe the
        // redirect-edge face takes) makes the node's value the
        // implicit-`call` dispatch, so the send's static type is the
        // `call` method's return — never the accessor's own declared
        // type (`entry.key(context)` reads `XPathSequence` through
        // `XPathExpression.call`, not the `key` field's own class).
        let name = self.strings.resolve(read.name);
        let units: u32 = if name.is_ascii() {
            name.len() as u32
        } else {
            name.chars().map(char::len_utf16).sum::<usize>() as u32
        };
        let invoked = self.byte_at(read.offset + units) == Some(b'(');
        let ty = if invoked && !self.is_never_ty(ty) {
            match self.implicit_call_face(ty) {
                Some((_, call_ret, _)) => call_ret,
                None => ty,
            }
        } else {
            ty
        };
        (ty != Ty::UNKNOWN).then_some(ty)
    }

    /// Marks a read-shaped member access under its resolved receiver
    /// handle (the `unused_field` element-identity face). An unresolved
    /// receiver marks nothing — the name-level `read_names` still feeds
    /// the use face.
    pub(crate) fn note_member_read(&mut self, name: StringId, recv: &Receiver) {
        let handle = match *recv {
            // `this`/`super` inside an extension body resolves against the
            // `on` type's instance face — `this`'s static type IS the
            // extended type, so the read credits the `on` container's
            // member, not the extension's. A `super` access is a plain
            // container (extensions declare no `super` member face). A
            // named-container `this` keeps its own container.
            Receiver::This { container, kind } if kind == ContainerKind::Extension => {
                match self.enclosing_extension_on_type(container) {
                    Some(on) => (self.file, on),
                    // The on-name never resolved — no honest owner.
                    None => return,
                }
            }
            Receiver::This { container, .. } | Receiver::Super { container, .. } => {
                (self.file, container)
            }
            // `C.f` — a static-member read through the container's own name
            // (a `static const` field of a private class rides this face):
            // the receiver IS the owning container, no instance to type.
            Receiver::Container {
                name: container, ..
            } => (self.file, container),
            Receiver::Typed { type_name, .. } => {
                match resolve_container(
                    self.graph,
                    self.namespaces,
                    self.index,
                    self.lib,
                    self.file,
                    type_name,
                ) {
                    Some((_, owner)) => (owner, type_name),
                    None => return,
                }
            }
            _ => return,
        };
        self.member_read_owners
            .entry(name)
            .or_default()
            .insert(handle);
    }

    /// The read's owning container: the published push-time bare
    /// resolution (`RefTarget::Member` names the enclosing container — the
    /// frame stack is gone by pass 2), else the live container context.
    pub(crate) fn read_owner_container(&self, offset: u32) -> Option<StringId> {
        if let Some(found) = self.refs.iter().rev().find(|r| r.range.offset == offset)
            && let RefTarget::Member { container } = found.target
        {
            return Some(container);
        }
        self.container_context().map(|(container, _)| container)
    }

    /// Whether a read resolved against the container or any of its
    /// supertypes (`Sub`'s read of an inherited `_f` spares the base
    /// field; a sibling class's same-spelled field never does). The join
    /// walks each recorded receiver's supertype closure toward the
    /// declaring container — a `Sub`-typed receiver reading `Base._f`
    /// reaches `Base` through `Sub`'s own ancestry. Walking the
    /// declaration's supertypes instead would credit a base's read of its
    /// own member to a same-spelled subtype field (the inverse relation).
    pub(crate) fn member_was_read(&self, container: StringId, name: &StringId) -> bool {
        let Some(receivers) = self.member_read_owners.get(name) else {
            return false;
        };
        let decl = (self.file, container);
        if receivers.contains(&decl) {
            return true;
        }
        let hierarchy = FoldHierarchy {
            graph: self.graph,
            namespaces: self.namespaces,
            index: self.index,
            file: self.file,
            lib: self.lib,
            memo: RefCell::default(),
            in_progress: RefCell::default(),
        };
        receivers
            .iter()
            .any(|receiver| *receiver == decl || hierarchy.closure(*receiver).contains(&decl))
    }

    pub(crate) fn resolve_member_row(
        &mut self,
        promotions: &PromotionStaging,
        row: &Row,
        recv: Receiver,
    ) {
        // Any member access uses the accessed name (the official `members`
        // set; unresolved reads land in `unresolvedReadMembers`, which the
        // field rule credits the same). Read-shaped accesses also join the
        // field-read index — writes never spare a field. A row that named
        // the enclosing executable itself never marks (the official
        // `_enclosingExec` exclusion).
        let self_named = self.self_scope_rows.contains(&row.range.offset);
        if !self_named {
            self.used_names.insert(row.name);
        }
        // A read intent credits the field's implicit getter (the official
        // `members` face); an `Intent::Operator` is the dot/index-merged
        // read the `[]` fold rewrites from `Read` (`x._m[k]`, `C.f[i]`) —
        // the operand still reads the member, so it credits the same.
        // `ReadWrite` is the compound `??=`/`+=` face (a read before the
        // write). A bare `Intent::Write` never reads the getter.
        let reads_getter = matches!(row.intent, Intent::Read | Intent::ReadWrite)
            || matches!(row.intent, Intent::Operator(..));
        if !self_named && reads_getter {
            self.read_names.insert(row.name);
            self.note_member_read(row.name, &recv);
        }
        let name_text = self.strings.resolve(row.name);
        // The synthetic selector skip: a zero-length member name is a
        // recovery-inserted identifier (`x.;`) — the oracle's resolver
        // skips the synthetic token, so no member face reports for it and
        // the miss only lands in the byproduct table.
        if Self::synthetic_reference(row.range) {
            self.record_miss(row.name, row.range, true, use_kind_of(&row.intent));
            return;
        }
        match recv {
            Receiver::Unknown => {
                // The `new` selector face (`prefix.C.new` after the prefix
                // failed, cascade `..new()` over an unknown target): the
                // official property lookup answers `needsGetterError`
                // unconditionally for the name 'new' — before the
                // dynamic/invalid-receiver silence
                // (type_property_resolver.dart:78-83) — so the read reports
                // `undefined_getter`, the invocation `undefined_method`,
                // against the InvalidType the failed receiver left.
                if row.name == self.intern("new") {
                    let code = match row.intent {
                        Intent::Invoke => SemanticCode::UndefinedMethod,
                        Intent::Write | Intent::ReadWrite => SemanticCode::UndefinedSetter,
                        _ => SemanticCode::UndefinedGetter,
                    };
                    // The member family on an invalid receiver is
                    // guard-free like every member face (the oracle's
                    // `_isIgnored` consults `shouldIgnoreUndefined` only on
                    // the unqualified/prefixed *name* domains — an
                    // instance-member miss never reaches it).
                    self.emit(code, row.range, name_text, "InvalidType");
                    self.record_miss(row.name, row.range, true, use_kind_of(&row.intent));
                    return;
                }
                // The cascade-section face (`f()..g()`): the official walk
                // answers the section send itself, so the discarded value
                // reports at the selector when the name carries the face —
                // with the target type unknown to the model, the section
                // resolves by scope (the top-level site). Other unknown
                // receivers keep the under-report (`dyn.foo()` dispatches
                // dynamically, no element to check).
                if !self_named
                    && matches!(row.intent, Intent::Invoke | Intent::Read)
                    && self.cascade_sections.contains(&row.range.offset)
                {
                    self.check_top_level_unused_result(row);
                }
                // The inferred-slot receiver face (#2346①): a receiver
                // whose declared slot the name-level model never bound —
                // the for-in loop variable (`for (var e in xs)`'s `e`
                // carries the iterable's element type, upstream's
                // `_inferForEachVariable`), a pattern destructure the
                // flat-record bind declined — still owns a static type:
                // the flow layer's read-type row at the receiver's own
                // span (the same face the promoted-receiver re-decision
                // #1465 reads). A named flow face runs the member verdict
                // on its container's instance face, the flow type's own
                // nullability riding the receiver verdict — a nullable
                // face emits the shared unchecked-access code at the
                // found/miss gates (`type_property_resolver`'s
                // `isNullable` tail precedes every member resolution), a
                // non-null face the `undefined_*` trio. Every other face
                // — a dynamic dispatch, an unresolvable receiver, a
                // diverging repeat read — keeps the under-report.
                if !self_named
                    && let Some(read_span) = self
                        .member_reads
                        .binary_search_by(|read| read.offset.cmp(&row.range.offset))
                        .ok()
                        .map(|index| self.member_reads[index].recv_span)
                    && let Some(
                        face @ Ty::Named {
                            name: flow_head, ..
                        },
                    ) = self
                        .rebound_local_receiver(row.range.offset, &recv)
                        .or_else(|| self.recv_expression_ty(read_span))
                {
                    self.check_instance_member(
                        promotions,
                        row,
                        Receiver::Typed {
                            type_name: flow_head,
                            ty: face,
                        },
                    );
                    return;
                }
            }
            Receiver::Prefix { name: prefix } => {
                // `lazy.loadLibrary()` is the deferred-load runtime face
                // the oracle synthesizes on a `deferred as` prefix — it
                // joins no declared table, so the prefixed hit probe
                // misses it by construction. Skip it here (upstream
                // `DeferredLoad` synthesis: every deferred prefix carries
                // a `loadLibrary` function returning `Future<void>`).
                if row.name == self.intern("loadLibrary")
                    && row.intent == Intent::Invoke
                    && self
                        .graph
                        .import_edges(self.lib)
                        .iter()
                        .any(|edge| edge.deferred && edge.prefix == Some(prefix))
                {
                    return;
                }
                if self.prefixed_hit(prefix, row.name) || self.guard.ignore(Some(prefix), row.name)
                {
                    return;
                }
                match row.intent {
                    // Verified: a prefixed *invocation* miss is
                    // `undefined_function`; the property path reports
                    // `undefined_prefixed_name` (PrefixedLookupDomain).
                    Intent::Invoke => {
                        self.emit(SemanticCode::UndefinedFunction, row.range, name_text, "");
                        self.record_miss(row.name, row.range, true, NameUse::Invoke);
                    }
                    Intent::Read | Intent::Write | Intent::ReadWrite => {
                        let type_text = self.strings.resolve(prefix).to_string();
                        self.emit(
                            SemanticCode::UndefinedPrefixedName,
                            row.range,
                            name_text,
                            &type_text,
                        );
                        let use_kind = match row.intent {
                            Intent::Write | Intent::ReadWrite => NameUse::Write,
                            _ => NameUse::Read,
                        };
                        self.record_miss(row.name, row.range, true, use_kind);
                    }
                    Intent::Operator(..) => {
                        self.record_miss(row.name, row.range, true, NameUse::Read);
                    }
                }
            }
            Receiver::Container { name, kind } => {
                self.check_container_member(row, name, kind);
            }
            // The prefix-resolved container (`io.Directory.gone`): the
            // member verdict runs on the foreign container's static face —
            // the bare-scope `resolve_container` cannot reach it (the name
            // sits behind the prefix table), so the check re-resolves
            // through [`prefixed_container`] directly.
            Receiver::PrefixedContainer { prefix, name, kind } => {
                if let Some((resolved, _)) = prefixed_container(
                    self.graph,
                    self.namespaces,
                    self.index,
                    self.lib,
                    prefix,
                    name,
                ) {
                    self.container_member_verdict(row, resolved, name, kind);
                }
            }
            // A folded creation: class-shaped results resolve on the
            // instance face (superclass walk, `Object` members); an
            // extension override resolves against the extension's members —
            // its `on` type's `Object` face is the under-report.
            Receiver::Created { name, kind } => {
                if kind == ContainerKind::Extension {
                    if self.members.object_members.contains(&row.name)
                        || row.name == self.intern("==")
                    {
                        return;
                    }
                    self.check_container_member(row, name, kind);
                } else {
                    self.check_instance_member(
                        promotions,
                        row,
                        Receiver::Typed {
                            type_name: name,
                            ty: Ty::UNKNOWN,
                        },
                    );
                }
            }
            Receiver::This { .. } | Receiver::Typed { .. } => {
                self.check_instance_member(promotions, row, recv);
            }
            Receiver::Super { container, .. } => {
                self.check_super_member(row, container);
            }
        }
    }

    pub(crate) fn prefixed_hit(&self, prefix: StringId, name: StringId) -> bool {
        !matches!(
            self.namespaces
                .lookup_prefixed(self.graph, self.lib, prefix, name),
            dartq_resolution::Lookup::NotFound
        )
    }

    /// Static/enum/extension access on a container name (`C.m`, `E.x`,
    /// `Ext.g`) — the oracle's `StaticLookupDomain`
    /// (`lookup_failure_reporter.dart`): the interface face reports
    /// `undefined_method` (the type-literal shape shares the registry name
    /// and message), `undefined_getter`, `undefined_enum_constant` on enum
    /// reads, and `undefined_setter` on writes; the extension face reports
    /// its dedicated trio. Operator intents have no static-face shape the
    /// M1 model can name — record only. External containers suppress.
    pub(crate) fn check_container_member(
        &mut self,
        row: &Row,
        container: StringId,
        kind: ContainerKind,
    ) {
        let Some((resolved, _)) = resolve_container(
            self.graph,
            self.namespaces,
            self.index,
            self.lib,
            self.file,
            container,
        ) else {
            return;
        };
        self.container_member_verdict(row, resolved, container, kind);
    }

    /// [`Engine::check_container_member`] over an already-resolved
    /// container — the resolved-prefix arm (`io.Directory.gone`) holds its
    /// `prefixed_container` hit, which the bare-scope `resolve_container`
    /// above cannot reproduce (the name lives behind the prefix, not in
    /// the diagnosing library's scope).
    pub(crate) fn container_member_verdict(
        &mut self,
        row: &Row,
        resolved: &Container,
        container: StringId,
        kind: ContainerKind,
    ) {
        let write = matches!(row.intent, Intent::Write | Intent::ReadWrite);
        // Write intent resolves through the SETTER spelling plus a
        // non-final field's implicit setter — a getter under the base
        // name never satisfies a write (`Ext(item).g = x` on a
        // getter-only `g` reports `undefined_extension_setter`, the
        // same verdict the class-face `undefined_setter` shares).
        let defined = if write {
            resolved
                .members
                .contains_key(&self.setter_spelling(row.name))
                || resolved
                    .members
                    .get(&row.name)
                    .is_some_and(|entry| matches!(entry.kind, MemberKind::Field) && !entry.is_final)
        } else {
            resolved.members.contains_key(&row.name)
        };
        // The row's `kind` is the receiver's push-time spelling — the
        // micro tag conflates `extension` and `extension type` into
        // `ContainerKind::Extension`. The verdict keys on the RESOLVED
        // container's kind instead (upstream `StaticLookupDomain`'s element
        // switch, analyzer source `lookup_failure_reporter.dart`:437-463 —
        // `ExtensionElementImpl` alone takes the extension trio; an
        // `ExtensionTypeElement` is an `InterfaceElement` and reports the
        // class-family `undefined_method`/`_getter`/`_setter` "for the
        // type").
        let resolved_kind = resolved.kind;
        // An enum's synthesized STATIC `values` getter joins the
        // interface face without an entry in `members` — a static read
        // through the container's own name resolves the accessor
        // upstream (`EnumElementImpl`'s synthesized static members; the
        // `index`/`name` synth twins are instance members and keep the
        // miss at this face).
        if !defined
            && resolved_kind == ContainerKind::Enum
            && row.name == self.members.enum_synth_members[0]
        {
            return;
        }
        if !defined
            && (resolved.constructors.contains(&row.name)
                || (row.name == self.intern("new")
                    && resolved_kind != ContainerKind::Extension
                    && resolved.has_unnamed_constructor))
        {
            // A named-constructor selector (`C.named`) resolves through the
            // creation face — the constructor is the class's own
            // declaration, never an undefined member. The `.new` spelling
            // answers the unnamed constructor the same way
            // (ast_rewrite.dart's `name.lexeme == 'new'` →
            // `unnamedConstructor`); extensions have no constructors — an
            // extension TYPE does (its primary and named forms), so the
            // `Extension` exclusion is the resolved kind's, not the row's.
            self.check_member_unused_result(row, container, &Receiver::Unknown, true);
            return;
        }
        if defined {
            self.check_member_unused_result(
                row,
                container,
                &Receiver::Container {
                    name: container,
                    kind,
                },
                false,
            );
            return;
        }
        let name_text = self.strings.resolve(row.name);
        let type_text = self.strings.resolve(container).to_string();
        // A compound write checks both faces (the oracle resolves the read
        // first): the read-face text, then the setter text.
        let read_face_missing = !resolved.members.contains_key(&row.name);
        let (invoke_code, read_code, write_code) = match resolved_kind {
            ContainerKind::Extension => (
                SemanticCode::UndefinedExtensionMethod,
                SemanticCode::UndefinedExtensionGetter,
                SemanticCode::UndefinedExtensionSetter,
            ),
            _ => (
                SemanticCode::UndefinedMethod,
                if resolved_kind == ContainerKind::Enum {
                    SemanticCode::UndefinedEnumConstant
                } else {
                    SemanticCode::UndefinedGetter
                },
                SemanticCode::UndefinedSetter,
            ),
        };
        match row.intent {
            Intent::Invoke => self.emit(invoke_code, row.range, name_text, &type_text),
            Intent::Read => self.emit(read_code, row.range, name_text, &type_text),
            Intent::Write => self.emit(write_code, row.range, name_text, &type_text),
            Intent::ReadWrite => {
                if read_face_missing {
                    self.emit(read_code, row.range, name_text, &type_text);
                }
                self.emit(write_code, row.range, name_text, &type_text);
            }
            Intent::Operator(..) => {}
        }
        self.record_miss(row.name, row.range, true, use_kind_of(&row.intent));
    }

    /// Instance access through a declaration-typed receiver (`this.m`, `c.m`
    /// on a `C c`). A clean walk that misses reports the member family
    /// (`undefined_method`/`_getter`/`_setter` by access intent); a broken
    /// chain, the extension pool, enum synthesized members, and `Object`
    /// members suppress. Operator intents are the member-result shape
    /// (`c.j + 1`, `c.j[i]`): the member itself is the operator's receiver —
    /// a found member hands the operator check to its result type, a clean
    /// miss reports the getter read (the operator's invalid type never
    /// cascades). Mixin `on` constraints and extension `on` types are
    /// invisible to the model, so bodies of those containers record only.
    pub(crate) fn check_instance_member(
        &mut self,
        promotions: &PromotionStaging,
        row: &Row,
        recv: Receiver,
    ) {
        // The never-receiver junction (`receiverType is NeverTypeImpl`,
        // method_invocation_resolver.dart:644-667, the property resolver's
        // twin guards): the member lookup never runs on a `Never` receiver —
        // the staged receiver row (`receiver_of_type_never`) and the send's
        // dead tail own the face, and `_resolveReceiverWithoutTarget`
        // answers nothing (`f().foo()` reports the receiver pair alone).
        if matches!(recv, Receiver::Typed { ty, .. } if self.is_never_ty(ty)) {
            return;
        }
        let (container, this_kind) = match recv {
            Receiver::This { container, kind } => (container, Some(kind)),
            Receiver::Typed { type_name, .. } => (type_name, None),
            _ => return,
        };
        // A receiver whose class came from the closure-formal inference
        // (the solved callee-own parameter — the type is OUR approximation
        // of the oracle's): the member walk on it under-resolves the
        // extension/import members the oracle resolves through, so every
        // face against it stays silent — the undefined rows the walk would
        // emit (and the operator faces beside them) are under-report, not
        // evidence.
        if self.inferred_formal_containers.contains(&container) {
            // The oracle resolves this access (the posture's premise), so
            // the spelling stays a resolved use — the declaration it names
            // must not flip to `unused_element` because OUR walk sat out.
            // Instance-member sites read the spelling credit; extension
            // members read the per-face triple the application credit
            // keys — every same-file face declaring the spelling takes it
            // (the self-use arm's granularity; a suppressed access never
            // disambiguates siblings).
            self.inferred_recv_used_members.insert(row.name);
            if let Some(elements) = self.index.get(self.file) {
                let face_credits: Vec<u32> = elements
                    .extension_faces()
                    .iter()
                    .enumerate()
                    .filter(|(_, face)| face.members.contains_key(&row.name))
                    .filter_map(|(index, _)| u32::try_from(index).ok())
                    .collect();
                for index in face_credits {
                    self.extension_member_used
                        .insert((self.file, index, row.name));
                }
            }
            return;
        }
        // The extension-application faces read the receiver's nullability
        // (applicability and the unchecked-access companion both split on
        // it). A `?.`-merged access unwraps the receiver first — `x?.m`
        // accesses `m` on the non-null `C`, so a non-nullable `on C` face
        // still applies (the oracle's `applicableTo` runs against the
        // promoted non-null static type, not `C?`).
        let mut receiver_nullable = match &recv {
            Receiver::Typed { ty, .. } => {
                Self::is_nullable_ty(*ty) && !self.null_aware_members.contains(&row.range.offset)
            }
            _ => false,
        };
        // `Object`'s own members read on a nullable receiver need no null
        // check: `Null` declares them too, so the oracle resolves
        // `x.hashCode` / `x.toString()` / `x.runtimeType` /
        // `x.noSuchMethod(...)` on an `Object?` receiver through the
        // nullable type's interface and reports nothing
        // (`type_property_resolver.dart`'s nullable-receiver branch looks
        // the name up on `Null` before reporting).
        if receiver_nullable && self.members.object_members.contains(&row.name) {
            receiver_nullable = false;
        }
        // The promoted-property face (#1409, pass-2 leg): a member access
        // whose receiver read stood promoted (`_r != null` in the guard)
        // reads a non-null receiver — the promotable-field substitution
        // decides eligibility exactly like the teardown join's staged
        // checks (`isPropertyPromotable`, field_promotability.dart:70-76).
        // The receiver read's span is the `promoted_recv_reads` entry keyed
        // on this member's offset.
        if receiver_nullable
            && let Some(read_span) = self
                .promoted_recv_reads
                .binary_search_by(|(member, _)| member.cmp(&row.range.offset))
                .ok()
                .map(|index| self.promoted_recv_reads[index].1)
            && let Some(ty) = match &recv {
                Receiver::Typed { ty, .. } => Some(*ty),
                _ => None,
            }
            && self
                .promotable_field_substitution(&self.prop_promotion_rows, read_span, ty)
                .is_some()
        {
            receiver_nullable = false;
        }
        // The promoted-variable face (#2241): a bare-read receiver whose
        // flow promotion stands — `if (files != null) files.length` on the
        // `var`-inferred local whose promoted head is the flow lattice's
        // opaque marker (a signature-less function head shares the shape,
        // #2174). `filter_staged`'s general arm rewrites the staged
        // `Ty::Unknown` to the parse-time type's non-null form; the
        // receiver verdict reads the same staging: an opaque head, or a
        // promotion to a strictly non-null face, reads a non-null receiver
        // — a `Null` or nullable promoted head (the `== null` branch)
        // keeps the verdict.
        if receiver_nullable
            && let Some(read_span) = self
                .promoted_recv_reads
                .binary_search_by(|(member, _)| member.cmp(&row.range.offset))
                .ok()
                .map(|index| self.promoted_recv_reads[index].1)
            && let Some(promoted) = promotions.promoted(read_span)
            && (promoted == Ty::Unknown || promoted.is_strictly_non_nullable())
        {
            receiver_nullable = false;
        }
        // F-C: a selector pushed inside a constructor-initializer list
        // resolves through the dedicated initializer world — the field name
        // against the field set (`_checkForInvalidField`), the invocation
        // against the class's constructor table
        // (`RedirectingConstructorInvocation`; a missing target reports
        // nothing) — and the member trio never fires there. A write miss
        // (the name absent, or resolving to an accessor rather than a
        // field) codes `initializer_for_non_existent_field`; read and
        // invocation intents stay silent (the under-report posture the
        // redirect face shares).
        if self.init_rows.contains_key(&row.range.offset) {
            if matches!(row.intent, Intent::Write | Intent::ReadWrite) {
                // `_checkForInvalidField`: the name must be a real field of
                // the enclosing class — a miss or an accessor spelling
                // codes the dedicated face; chain-broken receivers defer.
                let reports = match self.walk_members(container, row.name) {
                    Walk::Missing => true,
                    Walk::Found(entry) => entry.kind != MemberKind::Field,
                    Walk::ChainBroken => false,
                };
                if reports {
                    let name_text = self.strings.resolve(row.name);
                    self.emit(
                        SemanticCode::InitializerForNonExistentField,
                        row.range,
                        name_text,
                        "",
                    );
                }
            }
            return;
        }
        if let Intent::Operator(..) = row.intent {
            // The `Object` face the walk cannot reach (its chain ends at an
            // unindexed root): `hashCode`/`toString`/`==`/`noSuchMethod`/
            // `runtimeType` are defined on every class-shaped receiver, so
            // the plain member arm's walk below never fires on them
            // (`member_defined`'s exemption). The operator-merged row owes
            // the same guard — `inner.hashCode * 5` is a defined getter read
            // followed by a check of `*` on the getter's result, never an
            // `undefined_getter`.
            if row.name == self.intern("==") || self.members.object_members.contains(&row.name) {
                return;
            }
            // The `on`-face consult the plain tail runs (`this`'s static
            // type inside a mixin or extension body is the constraint /
            // extended type): the operator-merged row owes it too —
            // `this.offset + 1` inside `extension E on C` resolves `offset`
            // through C's face, never through the extension's member table.
            // A resolvable member suppresses; an undecidable face (a frozen
            // `Unknown` — the on-name the diagnosing scope cannot name is
            // the oracle's `InvalidType` extendedType) silences the miss
            // the same; only a decidable `Absent` reports below, naming the
            // `on` type.
            let mut report_on = container;
            if let Some(kind) = self.container_kind_of(container)
                && matches!(kind, ContainerKind::Mixin | ContainerKind::Extension)
            {
                if !matches!(self.on_face_walk(container, kind, row.name), Walk::Missing) {
                    self.record_miss(row.name, row.range, true, NameUse::Read);
                    return;
                }
                if kind == ContainerKind::Extension && matches!(recv, Receiver::This { .. }) {
                    match self.enclosing_extension_on_type(container) {
                        Some(on) => {
                            if !matches!(
                                self.frozen_member_verdict(on, row.name),
                                FrozenFace::Absent
                            ) {
                                self.record_miss(row.name, row.range, true, NameUse::Read);
                                return;
                            }
                            report_on = on;
                        }
                        None => {
                            self.record_miss(row.name, row.range, true, NameUse::Read);
                            return;
                        }
                    }
                }
            }
            match self.walk_members(container, row.name) {
                Walk::Found(entry) => {
                    // The nullable-dereference face the plain member arm
                    // fires (`member_defined`'s found arm): the oracle's
                    // `isNullable` tail precedes every member resolution, so
                    // a found member read through a nullable receiver is
                    // `unchecked_use_of_nullable_value` at the selector even
                    // as the operator's own face resolves on the getter's
                    // result (`a.offset - b.offset` over `a: Node?` reports
                    // the unchecked row, never an undefined getter).
                    if receiver_nullable {
                        let name_text = self.strings.resolve(row.name);
                        self.emit_corrected(
                            SemanticCode::UncheckedUseOfNullableValue,
                            row.range,
                            name_text,
                        );
                    }
                    let ty = Self::member_result_ty(&entry);
                    self.check_operator_expr(ty, row);
                    return;
                }
                // The frozen interface faces decide a break at the root or
                // a deeper hop the same as the plain member arm below: a
                // carried member suppresses (its result type is beyond the
                // name-level model — `check_operator_expr` reads `Unknown`),
                // a decidable miss reports the getter read — or the shared
                // unchecked-access code on a merely-nullable receiver — and
                // a genuinely unindexed declaration keeps the under-report.
                Walk::ChainBroken => {
                    match self.frozen_member_verdict(report_on, row.name) {
                        FrozenFace::Absent => {
                            let name_text = self.strings.resolve(row.name);
                            if receiver_nullable {
                                self.emit_corrected(
                                    SemanticCode::UncheckedUseOfNullableValue,
                                    row.range,
                                    name_text,
                                );
                            } else {
                                let type_text = self.strings.resolve(report_on).to_string();
                                self.emit(
                                    SemanticCode::UndefinedGetter,
                                    row.range,
                                    name_text,
                                    &type_text,
                                );
                            }
                            self.record_miss(row.name, row.range, true, NameUse::Read);
                        }
                        FrozenFace::Defined | FrozenFace::Unknown => {}
                    }
                    return;
                }
                Walk::Missing => {
                    let name_text = self.strings.resolve(row.name);
                    if receiver_nullable {
                        self.emit_corrected(
                            SemanticCode::UncheckedUseOfNullableValue,
                            row.range,
                            name_text,
                        );
                    } else {
                        let type_text = self.strings.resolve(report_on).to_string();
                        self.emit(
                            SemanticCode::UndefinedGetter,
                            row.range,
                            name_text,
                            &type_text,
                        );
                    }
                    self.record_miss(row.name, row.range, true, NameUse::Read);
                    return;
                }
            }
        }
        // The extension-application face reads the receiver's full static
        // type: `a.b().c`'s chained `Typed.ty` carries the declared return
        // type's argument list (`Map<K, FieldItem>`), which the `on`
        // clause's argument gate compares positionally. `this` reads its
        // container's instance type (an extension body's `this` is the
        // extended type). A bare head skips the gate either way.
        let receiver_ty = match recv {
            Receiver::Typed { ty, .. } => ty,
            Receiver::This { container, kind } => self.this_static_ty(container, kind),
            _ => Ty::UNKNOWN,
        };
        // An extension `this` whose `on` type is a record or function type
        // has a fully DECIDABLE member face — the record's field getters
        // (or the function type's `call`), then nothing — so a miss reports
        // exactly like the oracle: `undefined_method`/`-getter`/`-setter`
        // against the extended type's display ('<unknown>' for a record,
        // 'Function' for `on void Function()`). `member_defined` cannot see
        // this (a non-named receiver reads as the suppress-everything
        // face), so the record/function `this` is answered here. A nullable
        // `this` keeps the under-report (the oracle's
        // `unchecked_use_of_nullable_value` face).
        if this_kind == Some(ContainerKind::Extension) {
            let record_hit = matches!(receiver_ty, Ty::Record { .. })
                && self
                    .record_field_ty(receiver_ty, row.name, receiver_nullable)
                    .is_some();
            let function_hit = matches!(
                receiver_ty,
                Ty::Function {
                    nullable: false,
                    ..
                }
            ) && row.name == self.intern("call");
            if record_hit || function_hit {
                return;
            }
            let (decidable, display) = match receiver_ty {
                Ty::Record {
                    nullable: false, ..
                } => (true, "<unknown>"),
                Ty::Function {
                    nullable: false, ..
                } => (true, "Function"),
                _ => (false, ""),
            };
            if decidable {
                let name_text = self.strings.resolve(row.name);
                match row.intent {
                    Intent::Invoke => {
                        self.emit(SemanticCode::UndefinedMethod, row.range, name_text, display);
                    }
                    Intent::Read => {
                        self.emit(SemanticCode::UndefinedGetter, row.range, name_text, display);
                    }
                    Intent::Write | Intent::ReadWrite => {
                        self.emit(SemanticCode::UndefinedSetter, row.range, name_text, display);
                    }
                    Intent::Operator(..) => {}
                }
                self.record_miss(row.name, row.range, true, use_kind_of(&row.intent));
                return;
            }
        }
        if self.member_defined(container, receiver_ty, row, false, receiver_nullable) {
            // No implicit-this face here: every caller hands a QUALIFIED
            // member (`x.m`, `C().m`, `this.m`), and the oracle runs
            // `_checkForInvalidInstanceMemberAccess2` from the unqualified
            // nodes only (`visitUnqualifiedNameExpression`,
            // `UnqualifiedFunctionInvocation`, the unqualified assignment
            // targets) — the bare-row face owns it; an explicit `this` in
            // an initializer is `invalid_reference_to_this` at the keyword.
            self.check_member_unused_result(
                row,
                container,
                &Receiver::Typed {
                    type_name: container,
                    ty: Ty::UNKNOWN,
                },
                false,
            );
            return;
        }
        if matches!(self.walk_members(container, row.name), Walk::ChainBroken) {
            // A chain broken at the root type (`int x; x.noSuchGetter`) is
            // the external-primitive face: the core member table decides.
            // A broken superclass hop is decidable through the frozen
            // interface faces (`Comparable`, `Match`, `Stream`, …) the
            // element index cannot carry — `face_member_frozen`'s same
            // consult, three-valued so a genuinely unindexed declaration
            // keeps the under-report. A write resolves its own pair of
            // lookups exactly like `member_defined`'s write arm: the plain
            // spelling satisfies the write through a field/getter hit (the
            // assignment-to-* family owns that face), and only a miss on
            // BOTH spellings reports `undefined_setter` — so the getter
            // verdict consults first and the `name=` spelling breaks the
            // tie (`Match`'s face carries no setters; `IOSink`'s and
            // `StreamController`'s do). A compound write's read face is the
            // getter verdict's own answer.
            let write = matches!(row.intent, Intent::Write | Intent::ReadWrite);
            let getter_verdict = self.frozen_member_verdict(container, row.name);
            let setter_verdict = write.then(|| {
                let setter = self.setter_spelling(row.name);
                self.frozen_member_verdict(container, setter)
            });
            // The write-side defined answer joins the getter face's: a
            // `name=` member satisfies the write like a field hit does.
            let defined = matches!(getter_verdict, FrozenFace::Defined)
                || matches!(setter_verdict, Some(FrozenFace::Defined));
            let getter_absent = matches!(getter_verdict, FrozenFace::Absent);
            let setter_absent =
                matches!(setter_verdict, Some(FrozenFace::Absent)) || !write && getter_absent;
            let undecidable = matches!(getter_verdict, FrozenFace::Unknown)
                || matches!(setter_verdict, Some(FrozenFace::Unknown));
            if defined {
                // The member resolved on the frozen face — the found-arm's
                // faces (`member_defined`'s nullable-deref gate plus the
                // unused-result companion) apply exactly as a walkable
                // hit's. A compound write judges its two faces separately:
                // `m` absent under a defined `m=` still reports the getter
                // read (the oracle resolves it first), and `m` defined
                // under an absent `m=` reports the setter write.
                if row.intent == Intent::ReadWrite {
                    let name_text = self.strings.resolve(row.name);
                    let type_text = self.strings.resolve(container).to_string();
                    if getter_absent {
                        self.emit(
                            SemanticCode::UndefinedGetter,
                            row.range,
                            name_text,
                            &type_text,
                        );
                        self.record_miss(row.name, row.range, true, use_kind_of(&row.intent));
                        return;
                    }
                    if matches!(setter_verdict, Some(FrozenFace::Absent)) {
                        self.emit(
                            SemanticCode::UndefinedSetter,
                            row.range,
                            name_text,
                            &type_text,
                        );
                        self.record_miss(row.name, row.range, true, use_kind_of(&row.intent));
                        return;
                    }
                }
                if receiver_nullable {
                    let name_text = self.strings.resolve(row.name);
                    self.emit_corrected(
                        SemanticCode::UncheckedUseOfNullableValue,
                        row.range,
                        name_text,
                    );
                }
                self.check_member_unused_result(
                    row,
                    container,
                    &Receiver::Typed {
                        type_name: container,
                        ty: Ty::UNKNOWN,
                    },
                    false,
                );
                return;
            }
            if undecidable {
                self.check_core_type_member(container, row, receiver_nullable);
                return;
            }
            // `member_defined` already ran the extension pool and the
            // `Object`-member/`==` arms. A miss through a merely-nullable
            // receiver is the oracle's unchecked-access row (`type_property_
            // resolver`'s `isNullable` tail never reaches `undefined_*` —
            // the shared `unchecked_use_of_nullable_value` code stands for
            // every variant); a non-nullable miss reports the access-intent
            // trio like a walkable clean miss, per face: the read face
            // answers the getter verdict, the write face the pair.
            let name_text = self.strings.resolve(row.name);
            if receiver_nullable {
                self.emit_corrected(
                    SemanticCode::UncheckedUseOfNullableValue,
                    row.range,
                    name_text,
                );
                self.record_miss(row.name, row.range, true, use_kind_of(&row.intent));
                return;
            }
            let type_text = self.strings.resolve(container).to_string();
            match row.intent {
                Intent::Invoke => {
                    self.emit(
                        SemanticCode::UndefinedMethod,
                        row.range,
                        name_text,
                        &type_text,
                    );
                }
                Intent::Read => {
                    self.emit(
                        SemanticCode::UndefinedGetter,
                        row.range,
                        name_text,
                        &type_text,
                    );
                }
                Intent::Write | Intent::ReadWrite => {
                    // The oracle resolves the getter before the setter — a
                    // compound write reports its absent read face first.
                    if row.intent == Intent::ReadWrite && getter_absent {
                        self.emit(
                            SemanticCode::UndefinedGetter,
                            row.range,
                            name_text,
                            &type_text,
                        );
                    }
                    if setter_absent {
                        self.emit(
                            SemanticCode::UndefinedSetter,
                            row.range,
                            name_text,
                            &type_text,
                        );
                    }
                }
                Intent::Operator(..) => {}
            }
            self.record_miss(row.name, row.range, true, use_kind_of(&row.intent));
            return;
        }

        // An SDK-stub anchor's clean MISS suppresses when the frozen rows
        // define the member — dart:isolate's names-only `ReceivePort` (an
        // indexed empty container with an unindexed `implements Stream`
        // clause) walks to `Missing`, and `first`/`sendPort` are defined
        // by the stub faces (`SDK_FACES_EXTRA`/`SDK_SUPER_FACES`). Every
        // other verdict keeps the miss's own reporting path — the
        // stub-face ABSENT answer must flow through the unchanged arm
        // (the message's type face and the rig pins depend on it).
        else if matches!(self.walk_members(container, row.name), Walk::Missing)
            && matches!(
                self.frozen_member_verdict(container, row.name),
                FrozenFace::Defined
            )
        {
            self.check_member_unused_result(
                row,
                container,
                &Receiver::Typed {
                    type_name: container,
                    ty: Ty::UNKNOWN,
                },
                false,
            );
            return;
        }
        let Some(kind) = self.container_kind_of(container) else {
            // The external-root face: an unindexable receiver type (the
            // core primitives in the CLI world, an undeclared name in the
            // bare unit) decides through the core member table — the
            // dead-edge walk lands here as a clean `Missing` now, never as
            // the broken-chain arm above.
            self.check_core_type_member(container, row, receiver_nullable);
            return;
        };
        if kind == ContainerKind::Enum && self.members.enum_synth_members.contains(&row.name) {
            // The synthesized `index`/`values`/`names` getters are defined
            // instance members; a qualified read of one never reports the
            // initializer face (the bare `index` read is the bare row's).
            return;
        }
        // The `on`-face union (#2186 S2): `walk_members` never climbs a
        // mixin's `on` constraints or an extension's extended type, but
        // those members ride the receiver's interface (`this`'s static
        // type inside an extension IS the `on` type). A carried member
        // resolves the access; a face the model cannot walk keeps the
        // record-only path (the constraint may carry the member). A
        // decidable miss reports like the oracle's `InstanceLookupDomain` —
        // the miss names `this`'s static type: the mixin itself for a mixin
        // body, the `on` type for an extension body. An unresolved on-name
        // is the oracle's `InvalidType` extendedType — its member misses
        // stay silent. A declared-type receiver's decidable miss keeps
        // naming the written static type (the class-family tail below owns
        // the shared emission).
        let mut report_on = container;
        if matches!(kind, ContainerKind::Mixin | ContainerKind::Extension) {
            if !matches!(self.on_face_walk(container, kind, row.name), Walk::Missing) {
                self.record_miss(row.name, row.range, true, use_kind_of(&row.intent));
                return;
            }
            if kind == ContainerKind::Extension && matches!(recv, Receiver::This { .. }) {
                match self.enclosing_extension_on_type(container) {
                    Some(on) => {
                        // The `on` face the model cannot walk (a bare SDK
                        // type like `Stream<T>`) still answers by its
                        // frozen member table: a hit resolves the access —
                        // the extension applies to a receiver whose
                        // interface carries the member. A miss names the
                        // `on` type in the report below (the climb anchors
                        // at the on-name itself). A face the verdict
                        // cannot decide keeps the record-only posture: the
                        // on-name itself is the oracle's unresolvable
                        // extended type (`InvalidType`), whose member
                        // misses stay silent.
                        match self.frozen_member_verdict(on, row.name) {
                            FrozenFace::Defined | FrozenFace::Unknown => {
                                self.record_miss(
                                    row.name,
                                    row.range,
                                    true,
                                    use_kind_of(&row.intent),
                                );
                                return;
                            }
                            FrozenFace::Absent => report_on = on,
                        }
                    }
                    // The on-name never resolved — nothing honest names the
                    // report, so the record-only posture holds here too.
                    None => {
                        self.record_miss(row.name, row.range, true, use_kind_of(&row.intent));
                        return;
                    }
                }
            }
        }
        let name_text = self.strings.resolve(row.name);
        // A decidable miss through a merely-nullable receiver is the
        // oracle's unchecked-access row — `type_property_resolver`'s
        // `isNullable` tail reports the shared
        // `unchecked_use_of_nullable_value` code at the selector and never
        // reaches the `undefined_*` family, whatever the member face the
        // model decided the miss on.
        if receiver_nullable {
            self.emit_corrected(
                SemanticCode::UncheckedUseOfNullableValue,
                row.range,
                name_text,
            );
            self.record_miss(row.name, row.range, true, use_kind_of(&row.intent));
            return;
        }
        let type_text = self.strings.resolve(report_on).to_string();
        match row.intent {
            Intent::Invoke => {
                self.emit(
                    SemanticCode::UndefinedMethod,
                    row.range,
                    name_text,
                    &type_text,
                )
            }
            Intent::Write | Intent::ReadWrite => {
                // A compound write checks the read face first (the oracle
                // resolves the getter before the setter).
                if row.intent == Intent::ReadWrite
                    && matches!(self.walk_members(container, row.name), Walk::Missing)
                {
                    self.emit(
                        SemanticCode::UndefinedGetter,
                        row.range,
                        name_text,
                        &type_text,
                    );
                }
                self.emit(
                    SemanticCode::UndefinedSetter,
                    row.range,
                    name_text,
                    &type_text,
                );
            }
            Intent::Read => {
                self.emit(
                    SemanticCode::UndefinedGetter,
                    row.range,
                    name_text,
                    &type_text,
                );
            }
            Intent::Operator(..) => unreachable!("operator intents returned above"),
        }
        self.record_miss(row.name, row.range, true, use_kind_of(&row.intent));
    }

    /// `super.member` — the oracle's `SuperLookupDomain`: one analyzerCode
    /// (`undefined_super_member`) with a per-access-shape message, reported
    /// against the enclosing container's name. Operator intents follow the
    /// member-result shape (the direct `super + b` form lives on the
    /// receiver rows in [`Engine::resolve_receiver_row`], which reports the
    /// operator shape). A broken super chain (mixins: `on` constraints are
    /// not edges) suppresses.
    pub(crate) fn check_super_member(&mut self, row: &Row, container: StringId) {
        if let Intent::Operator(..) = row.intent {
            match self.walk_super(container, row.name) {
                Walk::Found(entry) => {
                    let ty = Self::member_result_ty(&entry);
                    self.check_operator_expr(ty, row);
                }
                // The frozen faces decide a break on an unindexed SDK
                // supertype (`class C extends Match { f() { super.m + 1 } }`)
                // — carried member suppresses, decidable miss reports the
                // getter shape, genuinely unknown hops stay silent.
                Walk::ChainBroken => {
                    if matches!(
                        self.frozen_super_verdict(container, row.name),
                        FrozenFace::Absent
                    ) {
                        let name_text = self.strings.resolve(row.name);
                        let type_text = self.strings.resolve(container).to_string();
                        self.emit(
                            SemanticCode::UndefinedSuperMember(SuperShape::Getter),
                            row.range,
                            name_text,
                            &type_text,
                        );
                        self.record_miss(row.name, row.range, true, NameUse::Read);
                    }
                }
                Walk::Missing => {
                    let name_text = self.strings.resolve(row.name);
                    let type_text = self.strings.resolve(container).to_string();
                    self.emit(
                        SemanticCode::UndefinedSuperMember(SuperShape::Getter),
                        row.range,
                        name_text,
                        &type_text,
                    );
                    self.record_miss(row.name, row.range, true, NameUse::Read);
                }
            }
            return;
        }
        // Extensions never apply to `super` accesses; the nullability
        // flag is inert on this path.
        if self.member_defined(container, Ty::UNKNOWN, row, true, false) {
            self.check_member_unused_result(
                row,
                container,
                &Receiver::Super {
                    container,
                    kind: ContainerKind::Class,
                },
                false,
            );
            return;
        }
        if matches!(self.walk_super(container, row.name), Walk::ChainBroken) {
            // The broken-hop consult: the frozen SDK interface faces answer
            // a break that the element index cannot carry (`class C extends
            // Match` reaching `super.m`). A write resolves its pair of
            // lookups like `member_defined`'s `name=` arm — the plain
            // spelling satisfies the write through a field/getter hit, and
            // only a miss on BOTH spellings reports the setter shape; a
            // compound write's read face is the getter verdict's own
            // answer. A genuinely unindexed face stays silent.
            let write = matches!(row.intent, Intent::Write | Intent::ReadWrite);
            let getter_verdict = self.frozen_super_verdict(container, row.name);
            let setter_verdict = write.then(|| {
                let setter = self.setter_spelling(row.name);
                self.frozen_super_verdict(container, setter)
            });
            let defined = matches!(getter_verdict, FrozenFace::Defined)
                || matches!(setter_verdict, Some(FrozenFace::Defined));
            let getter_absent = matches!(getter_verdict, FrozenFace::Absent);
            let setter_absent =
                matches!(setter_verdict, Some(FrozenFace::Absent)) || !write && getter_absent;
            let undecidable = matches!(getter_verdict, FrozenFace::Unknown)
                || matches!(setter_verdict, Some(FrozenFace::Unknown));
            let name_text = self.strings.resolve(row.name);
            let type_text = self.strings.resolve(container).to_string();
            if defined {
                // The compound write judges its faces separately: `m`
                // absent under a defined `m=` still reports the getter
                // shape, `m` defined under an absent `m=` the setter's.
                if row.intent == Intent::ReadWrite {
                    if getter_absent {
                        self.emit(
                            SemanticCode::UndefinedSuperMember(SuperShape::Getter),
                            row.range,
                            name_text,
                            &type_text,
                        );
                        self.record_miss(row.name, row.range, true, use_kind_of(&row.intent));
                        return;
                    }
                    if matches!(setter_verdict, Some(FrozenFace::Absent)) {
                        self.emit(
                            SemanticCode::UndefinedSuperMember(SuperShape::Setter),
                            row.range,
                            name_text,
                            &type_text,
                        );
                        self.record_miss(row.name, row.range, true, use_kind_of(&row.intent));
                        return;
                    }
                }
                self.check_member_unused_result(
                    row,
                    container,
                    &Receiver::Super {
                        container,
                        kind: ContainerKind::Class,
                    },
                    false,
                );
                return;
            }
            if undecidable {
                return;
            }
            let shape = match row.intent {
                Intent::Invoke => SuperShape::Method,
                Intent::Write | Intent::ReadWrite => SuperShape::Setter,
                Intent::Read => SuperShape::Getter,
                Intent::Operator(..) => unreachable!("operator intents returned above"),
            };
            // A compound write checks the read face first (the oracle
            // resolves the getter before the setter); its absent faces
            // report in that order.
            if row.intent == Intent::ReadWrite && getter_absent {
                self.emit(
                    SemanticCode::UndefinedSuperMember(SuperShape::Getter),
                    row.range,
                    name_text,
                    &type_text,
                );
            }
            if !write || setter_absent {
                self.emit(
                    SemanticCode::UndefinedSuperMember(shape),
                    row.range,
                    name_text,
                    &type_text,
                );
            }
            self.record_miss(row.name, row.range, true, use_kind_of(&row.intent));
            return;
        }
        let shape = match row.intent {
            Intent::Invoke => SuperShape::Method,
            Intent::Write | Intent::ReadWrite => SuperShape::Setter,
            Intent::Read => SuperShape::Getter,
            Intent::Operator(..) => unreachable!("operator intents returned above"),
        };
        let name_text = self.strings.resolve(row.name);
        let type_text = self.strings.resolve(container).to_string();
        // A compound write checks the read face first (the oracle resolves
        // the getter before the setter).
        if row.intent == Intent::ReadWrite
            && matches!(self.walk_super(container, row.name), Walk::Missing)
        {
            self.emit(
                SemanticCode::UndefinedSuperMember(SuperShape::Getter),
                row.range,
                name_text,
                &type_text,
            );
        }
        self.emit(
            SemanticCode::UndefinedSuperMember(shape),
            row.range,
            name_text,
            &type_text,
        );
        self.record_miss(row.name, row.range, true, use_kind_of(&row.intent));
    }

    /// The frozen-face member verdict behind a [`Walk::ChainBroken`]
    /// receiver: climbs the receiver type's supertype graph exactly like
    /// [`Engine::face_member_frozen`] (the `@override` face's consult —
    /// `extends` linear, `with` mixins in reverse application order,
    /// `implements`/`on` faces through the pending queue), but three-valued.
    /// Each `resolve_container` failure classifies by the same split the
    /// member walk applies: a frozen-face spelling
    /// ([`CORE_INTERFACE_MEMBERS`] / [`CORE_TYPE_MEMBERS`], aliased through
    /// [`COLLECTION_INTERFACE_ALIASES`]) answers its member set outright; a
    /// dead edge ([`Engine::supertype_hop_dead`]'s `true` arm — an
    /// undefined, ambiguous, or non-class supertype, the oracle's
    /// `InvalidType` that contributes nothing) keeps the climb decidable;
    /// a resolved-but-unindexed declaration the tables do not know leaves
    /// the chain genuinely broken (`Unknown`, the under-report direction —
    /// its members could carry the name). `Defined` needs the write
    /// spelling joined by its setter form upstream: call sites pass the
    /// single spelling this access resolves through.
    /// Whether `name` is a frozen SDK face the member tables carry (the
    /// `dart:core`/`dart:async`/`dart:io` interfaces plus the
    /// `dart:collection` bases that re-export them, and the `Stream` row
    /// the tables lack) — its members are known exactly, so a spelling it
    /// lacks contributes nothing.
    pub(crate) fn frozen_face_text(name: &str) -> bool {
        CORE_INTERFACE_MEMBERS
            .iter()
            .chain(CORE_TYPE_MEMBERS.iter())
            .chain(SDK_FACES_EXTRA.iter())
            .any(|(container, _)| name == *container)
            || COLLECTION_INTERFACE_ALIASES
                .iter()
                .any(|(alias, _)| name == *alias)
    }

    pub(crate) fn frozen_member_verdict(&self, face: StringId, spelling: StringId) -> FrozenFace {
        self.frozen_faces_verdict([(face, self.file)].into_iter(), spelling, true)
    }

    /// The super-face twin of [`Self::frozen_member_verdict`]: `super.m`'s
    /// lookup starts above the enclosing container's own members, so the
    /// climb seeds with the declaration's supertype edges — its superclass,
    /// `with` mixins, `implements` faces and `on` constraints (the same
    /// seed [`Engine::face_member_any`] reads). An unindexed container
    /// itself breaks the chain: `Unknown`.
    pub(crate) fn frozen_super_verdict(
        &self,
        container: StringId,
        spelling: StringId,
    ) -> FrozenFace {
        let Some(decl) = self
            .index
            .get(self.file)
            .and_then(|f| f.container(container))
        else {
            return FrozenFace::Unknown;
        };
        let seeds = decl
            .superclass
            .into_iter()
            .chain(decl.with_clause.iter().copied())
            .chain(decl.implements_clause.iter().copied())
            .chain(decl.on_clause.iter().copied())
            .map(|face| (face, self.file));
        self.frozen_faces_verdict(seeds, spelling, false)
    }

    /// The one graph-wide declaration of container `name` outside
    /// `scope_lib`, when exactly one library declares it — the anchor face
    /// for a receiver type the diagnosing library's scope does not carry
    /// (`XmlNodeList` reached unimported through a member's declared type).
    /// Any non-unique answer stays `None`: the climb must not guess.
    fn unique_declared_elsewhere(
        &self,
        name: StringId,
        scope_lib: LibraryId,
    ) -> Option<(FileId, LibraryId)> {
        // The oracle's `InvalidType` posture first: a type the diagnosing
        // file itself spells and cannot resolve is `undefined_class` there
        // and the receiver's static type is Invalid — its member misses
        // stay silent even when another library declares the same name
        // (the corpus's `Context context` from a broken package import).
        // Only a receiver type that arrived through a resolved member
        // signature — never spelled in this file — re-anchors the climb.
        if self
            .report
            .names
            .iter()
            .any(|r| r.name == name && r.use_kind == NameUse::Type && !r.qualified)
        {
            return None;
        }
        let libs = self.graph.declaring_libraries(name);
        if libs.len() != 1 || libs[0] == scope_lib {
            return None;
        }
        let lib = libs[0];
        let file = self.graph.unit_files(lib).iter().copied().find(|&f| {
            self.index
                .get(f)
                .is_some_and(|unit| unit.container(name).is_some())
        })?;
        Some((file, lib))
    }

    /// The shared climb behind [`Self::frozen_member_verdict`] and
    /// [`Self::frozen_super_verdict`] (see there for the three-valued
    /// contract and the hop classification). `anchor` holds when the first
    /// seed IS the receiver type's own spelling rather than a supertype
    /// edge: an unresolvable anchor that no frozen face knows leaves the
    /// receiver statically `InvalidType`, which answers every member face
    /// silently — `Unknown`, never `Absent` (a genuinely undeclared
    /// receiver type must not emit `undefined_*`). An anchor the scope
    /// cannot resolve but exactly one graph library declares re-anchors
    /// the climb there ([`Self::unique_declared_elsewhere`]).
    fn frozen_faces_verdict(
        &self,
        seeds: impl Iterator<Item = (StringId, FileId)>,
        spelling: StringId,
        anchor: bool,
    ) -> FrozenFace {
        let spelling_text = self.strings.resolve(spelling);
        let frozen_face = Self::frozen_face_text;
        let frozen_hit = |name: &str| {
            let aliases: &[&str] = COLLECTION_INTERFACE_ALIASES
                .iter()
                .chain(SDK_SUPER_FACES.iter())
                .find(|(alias, _)| name == *alias)
                .map_or(&[][..], |(_, aliases)| aliases);
            CORE_INTERFACE_MEMBERS
                .iter()
                .chain(CORE_TYPE_MEMBERS.iter())
                .chain(SDK_FACES_EXTRA.iter())
                .any(|(container, members)| {
                    (name == *container || aliases.contains(container))
                        && members.contains(&spelling_text)
                })
        };
        let mut seen = FxHashSet::default();
        let mut undecidable = false;
        // Set when the current `name` hop is a fallback re-attempt (the
        // seen-check must not divert it back to the pending queue).
        let mut retried = false;
        // The `implements`/`on` faces a resolved hop leaves unclimbed — the
        // linear superclass chain handles `extends`+`with`; when it
        // dead-ends, the interface faces keep the climb alive in
        // declaration order (mirrors `face_member_frozen`'s pending queue).
        let mut pending: VecDeque<(StringId, FileId)> = seeds.collect();
        let Some((first, first_owner)) = pending.pop_front() else {
            return FrozenFace::Unknown;
        };
        let mut name = first;
        let mut owner = first_owner;
        let mut scope_lib = self.lib;
        let mut at_anchor = anchor;
        'climb: loop {
            if !retried && !seen.insert(name) {
                let Some((next, next_owner)) = pending.pop_front() else {
                    break;
                };
                name = next;
                owner = next_owner;
                // A queued face is never the receiver type itself.
                at_anchor = false;
                continue;
            }
            retried = false;
            let Some((found, found_owner)) = resolve_container(
                self.graph,
                self.namespaces,
                self.index,
                scope_lib,
                owner,
                name,
            ) else {
                // The unindexed hop answers by its frozen member face: a
                // hit defines the member, a known face without it
                // contributes nothing (the frozen rows fold the whole
                // instance face, inherited members included), a dead edge
                // contributes nothing, and anything else breaks the climb's
                // decidability — its members could carry the name. The
                // anchor itself is the receiver's whole static type: not
                // frozen-known means `InvalidType`, which never reports.
                let text = self.strings.resolve(name);
                if frozen_hit(text) {
                    return FrozenFace::Defined;
                }
                if !frozen_face(text) {
                    if at_anchor
                        && let Some((declared_file, declared_lib)) =
                            self.unique_declared_elsewhere(name, scope_lib)
                    {
                        // Re-anchor at the unique declaring library: its
                        // namespace resolves the supertype edges the
                        // diagnosing scope never saw. The same spelling
                        // cannot resolve here again (`seen` holds it), so
                        // the retry bypasses the queue check once.
                        scope_lib = declared_lib;
                        owner = declared_file;
                        at_anchor = false;
                        retried = true;
                        continue 'climb;
                    }
                    if at_anchor || !self.supertype_hop_dead(name, scope_lib, owner) {
                        undecidable = true;
                    }
                }
                at_anchor = false;
                let Some((next, next_owner)) = pending.pop_front() else {
                    break;
                };
                name = next;
                owner = next_owner;
                continue;
            };
            at_anchor = false;
            if let Some(entry) = found.members.get(&spelling)
                && !entry.is_static
            {
                return FrozenFace::Defined;
            }
            // A RESOLVED dart: stub container (the SDK projection's
            // names-only units — an empty member map, no modelled
            // clauses) is the frozen rows' face too: the unindexed arm
            // above answers the unresolvable spelling, this one the
            // indexed-but-empty stub (dart:isolate's `ReceivePort`,
            // whose `first` rides `implements Stream`). Membership in a
            // `dart:` library is the whole test: a stub file registers
            // as its node's unit (`library_of_file` answers it), and a
            // unit-less file (the bare rig's single file, a local
            // declaration the diagnosing file itself carries) is never
            // an SDK face — its member face is exactly what the index
            // resolved above, so a local `class Iterable {}` keeps
            // reporting a missing `first`.
            if self
                .graph
                .library_of_file(found_owner)
                .is_some_and(|lib| self.graph.uri(lib).starts_with("dart:"))
                && frozen_hit(self.strings.resolve(name))
            {
                return FrozenFace::Defined;
            }
            let owner_lib = self
                .owner_library_of(found_owner, name)
                .unwrap_or(scope_lib);
            for mixin in found.with_clause.iter().rev() {
                if !seen.insert(*mixin) {
                    continue;
                }
                match resolve_container(
                    self.graph,
                    self.namespaces,
                    self.index,
                    owner_lib,
                    owner,
                    *mixin,
                ) {
                    Some((mixin_container, _))
                        if mixin_container
                            .members
                            .get(&spelling)
                            .is_some_and(|entry| !entry.is_static) =>
                    {
                        return FrozenFace::Defined;
                    }
                    None => {
                        let text = self.strings.resolve(*mixin);
                        if frozen_hit(text) {
                            return FrozenFace::Defined;
                        }
                        if !frozen_face(text) && !self.supertype_hop_dead(*mixin, owner_lib, owner)
                        {
                            undecidable = true;
                        }
                    }
                    _ => {}
                }
            }
            for super_face in found.implements_clause.iter().chain(found.on_clause.iter()) {
                pending.push_back((*super_face, found_owner));
            }
            match found.superclass {
                Some(superclass) => {
                    scope_lib = owner_lib;
                    name = superclass;
                    owner = found_owner;
                }
                None => {
                    let Some((next, next_owner)) = pending.pop_front() else {
                        break;
                    };
                    name = next;
                    owner = next_owner;
                    scope_lib = owner_lib;
                }
            }
        }
        if undecidable {
            FrozenFace::Unknown
        } else {
            FrozenFace::Absent
        }
    }

    /// The external-primitive member face (`int x; x.noSuchGetter`): the
    /// receiver type is a dart:core primitive when the name is one of the
    /// [`CORE_TYPE_MEMBERS`] keys and resolves to no indexed container.
    /// Extension members still apply implicitly, so the pool gates first; a
    /// table hit is defined, a miss reports the access-intent trio.
    pub(crate) fn check_core_type_member(
        &mut self,
        container: StringId,
        row: &Row,
        receiver_nullable: bool,
    ) {
        let container_text = self.strings.resolve(container);
        // `Symbol` rides the same face: every symbol literal is
        // `Symbol`-typed (`#void.foo;` resolves `.foo` against it —
        // symbols.dart) and the class declares none of the user selectors.
        const SYMBOL_MEMBERS: [&str; 0] = [];
        let symbol_face = container_text == "Symbol";
        let core = CORE_TYPE_MEMBERS
            .iter()
            .find(|(name, _)| container_text == *name)
            .map(|(name, members)| (*name, *members));
        let Some((_, members)) = core.or_else(|| symbol_face.then_some(("", &SYMBOL_MEMBERS[..])))
        else {
            return; // not a known primitive: suppress as before
        };
        if resolve_container(
            self.graph,
            self.namespaces,
            self.index,
            self.lib,
            self.file,
            container,
        )
        .is_some()
        {
            return; // locally declared: the regular walk already decided
        }
        let write = matches!(row.intent, Intent::Write | Intent::ReadWrite);
        // The core-type face knows the receiver's spelling only — the
        // nullability split stays under-report (`false`).
        match self.extension_application(container, Ty::UNKNOWN, row.name, write, false) {
            ExtensionApp::None => {}
            // `UnsafeSingle` never surfaces here — the `false` receiver
            // nullability keeps every resolved face safe.
            ExtensionApp::Single | ExtensionApp::UnsafeSingle => return,
            ExtensionApp::Ambiguous(displays) => {
                self.emit_extension_ambiguity(row.name, row.range, displays);
                return;
            }
        }
        let name_text = self.strings.resolve(row.name);
        // A recovery-spliced synthetic selector never reports on the Symbol
        // face (the oracle's `#foo.void` keeps the keyword error only).
        if symbol_face && name_text.is_empty() {
            return;
        }
        if members.contains(&name_text)
            || (write && members.contains(&self.strings.resolve(self.setter_spelling(row.name))))
        {
            // The nullable-dereference face (nullable_dereference_verifier
            // .dart report — the same emission the walkable-model found-arm
            // makes): a member defined on the core type read through a
            // merely-nullable receiver is the unchecked-access row at the
            // selector. The oracle's Object-member reads (`s.hashCode`,
            // `s.toString()`, `s == other`) stay silent — the member face
            // answers without a dereference question — so the
            // `==`/Object arm below keeps its under-report.
            if receiver_nullable && !self.members.object_members.contains(&row.name) {
                self.emit_corrected(
                    SemanticCode::UncheckedUseOfNullableValue,
                    row.range,
                    name_text,
                );
            }
            return;
        }
        if row.name == self.intern("==") || self.members.object_members.contains(&row.name) {
            return;
        }
        // A table miss through a merely-nullable receiver is the oracle's
        // unchecked-access row — the `isNullable` tail reports the shared
        // `unchecked_use_of_nullable_value` code and never reaches the
        // `undefined_*` family.
        if receiver_nullable {
            self.emit_corrected(
                SemanticCode::UncheckedUseOfNullableValue,
                row.range,
                name_text,
            );
            self.record_miss(row.name, row.range, true, use_kind_of(&row.intent));
            return;
        }
        let type_text = container_text.to_string();
        match row.intent {
            Intent::Invoke => self.emit(
                SemanticCode::UndefinedMethod,
                row.range,
                name_text,
                &type_text,
            ),
            Intent::Write | Intent::ReadWrite => {
                if row.intent == Intent::ReadWrite {
                    self.emit(
                        SemanticCode::UndefinedGetter,
                        row.range,
                        name_text,
                        &type_text,
                    );
                }
                self.emit(
                    SemanticCode::UndefinedSetter,
                    row.range,
                    name_text,
                    &type_text,
                );
            }
            Intent::Read => self.emit(
                SemanticCode::UndefinedGetter,
                row.range,
                name_text,
                &type_text,
            ),
            Intent::Operator(..) => {}
        }
        self.record_miss(row.name, row.range, true, use_kind_of(&row.intent));
    }

    /// The read and write operator spellings an access needs (`[]` / `[]=`
    /// for index rows by their write form; the plain spelling for binary
    /// operators); `(None, None)` for non-operator rows.
    pub(crate) fn operator_spellings(&self, row: &Row) -> (Option<StringId>, Option<StringId>) {
        let Intent::Operator(op, _) = &row.intent else {
            return (None, None);
        };
        if *op == self.intern("[]") {
            match row.write_op {
                Some(OpWrite::Assign) => (None, Some(self.intern("[]="))),
                Some(OpWrite::Compound) => (Some(*op), Some(self.intern("[]="))),
                None => (Some(*op), None),
            }
        } else {
            (Some(*op), None)
        }
    }

    /// The `name=` lookup spelling of a write target. Cached: the `format!`
    /// plus intern allocation repeats for the same write targets across a
    /// unit, and pass-time member checks ask on every write.
    pub(crate) fn setter_spelling(&mut self, name: StringId) -> StringId {
        self.members.setter_spelling(self.strings, name)
    }

    /// A bare write's second-chance setter resolution (`m = v` where `m`
    /// exists only as a setter): the push-time tentative-read lookup sees
    /// plain-name declarations — getters, fields, top-levels — and never
    /// the `name=` spelling, because the assignment upgrades the row's
    /// intent after the push. The oracle's write resolution consults the
    /// setter namespace (`preferSetter`), so a top-level or implicit-this
    /// setter defines the write silently. A getter-only name stays
    /// under-reported (the oracle's `undefined_setter` needs a receiver
    /// type this face cannot supply).
    pub(crate) fn bare_write_target_exists(
        &mut self,
        name: StringId,
        this_type: Option<StringId>,
    ) -> bool {
        let setter = self.setter_spelling(name);
        // The scoped lookup runs live (top-level setters resolve here); the
        // implicit-this arm rides the push-time snapshot (`this_type`) —
        // the frame stack is gone by pass 2.
        if !matches!(self.resolve_bare(setter).0, BareInfo::Missing { .. }) {
            return true;
        }
        this_type.is_some_and(|container| {
            resolve_container(
                self.graph,
                self.namespaces,
                self.index,
                self.lib,
                self.file,
                container,
            )
            .is_some_and(|(decl, _)| decl.members.contains_key(&setter))
        })
    }

    /// A member's result type as far as M1 knows it: the declared field type
    /// or getter return type; methods (function-typed tear-offs) and setters
    /// stay [`Ty::Unknown`] — every check on them suppresses.
    pub(crate) fn member_result_ty(entry: &MemberEntry) -> Ty {
        match entry.kind {
            MemberKind::Field => entry.var_ty,
            MemberKind::Getter => entry.return_ty,
            MemberKind::Method | MemberKind::Setter => Ty::UNKNOWN,
        }
    }

    /// The row's type read as the enclosing EXPRESSION's static type: a
    /// `?.` short-circuit makes the whole chain node nullable — `a?.b`
    /// types `B?` and `a?.b.c` types `C?` (upstream Dart SDK source,
    /// pkg/analyzer's `ShortCircuiting` resolution: the `?` joins the
    /// result of every node the operator guards). The row keeps the
    /// member's own declared face — the continuation's receiver check reads
    /// that face (`a?.b.c` resolves `c` on `b`'s written `B`/`B?`, never
    /// the short-circuit's `B?`) — so the `?` re-arms only where the row is
    /// consumed as a result (an assignment's right side, a `return`, an
    /// argument, a condition operand). The member-offset latch
    /// ([`Self::null_aware_members`]) marks every chain node the `?.`
    /// guards: the `?.` member itself plus the plain continuations its
    /// merge short-circuits (`merge_member` propagates the latch while the
    /// receiver it folded reads non-null — a nullable member result keeps
    /// the continuation's own unchecked-dereference verdict). An
    /// operator-merged row keeps its operator result — and a row inside a
    /// statically-`bool` composite keeps the composite's `bool`: `x?.m ==
    /// y` types `bool`, never `bool?` — so the upgrade leaves both alone.
    pub(crate) fn expression_row_ty(&self, row: &Row) -> Ty {
        // An invocation row stores its callee's declared type; the
        // expression's type is the send's result — a function-typed
        // callee's signature return ([`Self::invoked_callee_result`]),
        // else the resolved callee's declared return
        // ([`Self::callee_return_ty`], the same face the write staging
        // reads).
        // A stored send type stands (a folded creation's instance, the
        // null-assert rewrite `f()!` left on the row); only a send whose
        // row carries nothing resolves its callee here.
        let ty = match self.invoked_callee_result(row) {
            Some(ty) => ty,
            None if row.intent == Intent::Invoke && row.ty == Ty::UNKNOWN => {
                self.callee_return_ty(row)
            }
            None => row.ty,
        };
        if self.null_aware_members.contains(&row.range.offset)
            && !matches!(row.intent, Intent::Operator(..))
            && !self.bool_decided_covers(row)
        {
            Self::nullable_form(ty)
        } else {
            ty
        }
    }

    /// The member-read row's static type for the `var`-inference window —
    /// the follow-up the window's own faces answer `Unknown` on: the
    /// official `staticType` of a property access is the resolved member's
    /// declared type, so a getter read merged over a typed receiver
    /// answers its declared return (the accessor face
    /// [`Self::receiver_static_ty`] already carries), and an instance miss
    /// consults the applied extension face ([`Self::extension_read_ty`]).
    /// The extension getter's return is the one shape the merge-time
    /// field face can never answer (the merged row's `ty` keeps `Unknown`
    /// there), and every `var` declarator the read initializes stays
    /// untyped without it, so an extension use reached through the local
    /// never credits (`element.ffiClass` over `var element =
    /// elementAnnotation?.element` — the ffi_verifier census shape). The
    /// `?.` short-circuit's result nullability re-arms on the answer
    /// (`var a = c?.id` keeps `a` nullable). Every shape neither arm
    /// answers stays [`Ty::UNKNOWN`] — the under-report direction; a
    /// method tear-off is the window's own face, not this one.
    pub(crate) fn initializer_read_ty(&mut self, row: &Row) -> Ty {
        if row.intent != Intent::Read {
            return Ty::UNKNOWN;
        }
        let declared = match self.receiver_static_ty(row) {
            Ty::UNKNOWN => None,
            ty => Some(ty),
        };
        let Some(ty) = declared.or_else(|| self.extension_read_ty(row)) else {
            return Ty::UNKNOWN;
        };
        if self.null_aware_members.contains(&row.range.offset)
            && !self.bool_decided_covers(row)
        {
            return Self::nullable_form(ty);
        }
        ty
    }

    /// The extension arm of [`Self::initializer_read_ty`]: a member read
    /// whose instance walk missed but whose accessor resolves through an
    /// applied extension face answers the face's declared return. The
    /// application runs the resolve-side pipeline
    /// ([`Self::extension_application`]) — applicability, specificity, and
    /// the element credit the access owes — so the verdict must be a
    /// resolved application (`Single`/`UnsafeSingle`), and the attributed
    /// face is the pool's UNIQUE declarer of the spelling: a second
    /// declarer is the ambiguous or specificity-ordered application this
    /// face cannot attribute (the oracle's ambiguous access resolves no
    /// member either). `None` keeps the under-report: an unclassified
    /// receiver, a nullable-`on`-less verdict, and non-accessor members.
    fn extension_read_ty(&mut self, row: &Row) -> Option<Ty> {
        let RowShape::Member { recv } = &row.shape else {
            return None;
        };
        let (head, receiver_ty, receiver_nullable) = match recv {
            Receiver::Typed { type_name, ty, .. } => (*type_name, *ty, Self::is_nullable_ty(*ty)),
            Receiver::This { container, kind } => {
                let ty = self.this_static_ty(*container, *kind);
                let Ty::Named { name, .. } = ty else {
                    return None;
                };
                (name, ty, Self::is_nullable_ty(ty))
            }
            Receiver::Created {
                name,
                kind: ContainerKind::Class,
            } => (*name, Ty::named(*name), false),
            _ => return None,
        };
        // A `?.` access unwraps the receiver before the member read (the
        // same verdict `check_instance_member` feeds the application).
        let receiver_nullable =
            receiver_nullable && !self.null_aware_members.contains(&row.range.offset);
        self.accessible_extension_faces();
        let faces = self.accessible_extensions.as_deref().unwrap_or(&[]);
        let declarers: Vec<(&ExtensionFace, FileId)> = faces
            .iter()
            .filter(|(face, _)| face.members.contains_key(&row.name))
            .copied()
            .collect();
        if declarers.is_empty() {
            return None;
        }
        match self.extension_application(head, receiver_ty, row.name, false, receiver_nullable) {
            ExtensionApp::Single | ExtensionApp::UnsafeSingle => {}
            _ => return None,
        }
        // Several accessible extensions can declare the spelling over one
        // head (the analyzer corpus's paired `withOriginDeclaration`
        // getters: a concrete `on List<ConstructorFragmentImpl>` beside a
        // generic `on List<T extends …>`); the oracle resolves by
        // applicability, and for the read's static type every declarer
        // that CAN apply to this receiver instantiates to the same answer
        // — the concrete face names the receiver's own argument, the
        // generic one binds its parameter to it. A face whose `on`
        // arguments cannot match (a different concrete element) never
        // applies; disagreement among the viable faces stays the
        // under-report (`None`).
        let mut answer: Option<Ty> = None;
        for (face, face_file) in declarers {
            let Some(entry) = face.members.get(&row.name) else {
                continue;
            };
            let declared = match entry.kind {
                MemberKind::Getter => entry.return_ty,
                MemberKind::Field => entry.var_ty,
                _ => continue,
            };
            // The face's types are foreign-file payloads (argument lists
            // interned in the extension's own pool) — project through the
            // boundary before any argument read, the same cross-pool
            // discipline everywhere else.
            let on_ty = self.boundary.ty(face_file, face.on_ty);
            if !self.extension_on_applies(on_ty, receiver_ty, &face.type_param_names) {
                continue;
            }
            let ty = self.boundary.ty(face_file, declared);
            // A generic extension's declared return still spells its own
            // type parameters (`Iterable<T>` under `extension E<T> on
            // List<T>`); the oracle instantiates the extended type against
            // the receiver before answering, so the `on` type's
            // bare-parameter arguments pair with the receiver's arguments
            // positionally (`List<X>` binds `T := X`) and the declared
            // return substitutes through.
            let map: Vec<(StringId, Ty)> = match (on_ty, receiver_ty) {
                (
                    Ty::Named {
                        args: on_args, ..
                    },
                    Ty::Named {
                        args: recv_args, ..
                    },
                ) if on_args != ArgListId::EMPTY
                    && recv_args != ArgListId::EMPTY
                    && !face.type_param_names.is_empty() =>
                {
                    let on_list = self.boundary.pool().get(on_args).to_vec();
                    let recv_list = self.boundary.pool().get(recv_args).to_vec();
                    on_list
                        .into_iter()
                        .zip(recv_list)
                        .filter_map(|(on_arg, recv_arg)| {
                            let Ty::Named {
                                name,
                                args: ArgListId::EMPTY,
                                ..
                            } = on_arg
                            else {
                                return None;
                            };
                            face.type_param_names
                                .contains(&name)
                                .then_some((name, recv_arg))
                        })
                        .collect()
                }
                _ => Vec::new(),
            };
            let ty = if map.is_empty() {
                ty
            } else {
                self.subst_params(ty, &map).unwrap_or(ty)
            };
            let ty = self.fold_alias_ty(ty);
            match answer {
                None => answer = Some(ty),
                Some(kept) if kept == ty => {}
                Some(_) => return None,
            }
        }
        answer
    }

    /// Whether an extension's `on` type can apply to `receiver`: heads
    /// equal, and each `on` argument either equals the receiver's argument
    /// or is one of the extension's own bare type parameters (instantiable
    /// against whatever the receiver carries). The read face's per-face
    /// viability filter — a face whose concrete arguments differ from the
    /// receiver's can never be the oracle's resolution, whatever the
    /// specificity ordering among the survivors.
    fn extension_on_applies(
        &self,
        on_ty: Ty,
        receiver: Ty,
        type_params: &[StringId],
    ) -> bool {
        let (
            Ty::Named {
                name: on_head,
                args: on_args,
                ..
            },
            Ty::Named {
                name: recv_head,
                args: recv_args,
                ..
            },
        ) = (on_ty, receiver)
        else {
            return false;
        };
        if on_head != recv_head
            || (on_args == ArgListId::EMPTY) != (recv_args == ArgListId::EMPTY)
        {
            return false;
        }
        if on_args == ArgListId::EMPTY {
            return true;
        }
        let on_list = self.boundary.pool().get(on_args).to_vec();
        let recv_list = self.boundary.pool().get(recv_args).to_vec();
        on_list.len() == recv_list.len()
            && on_list.iter().zip(recv_list.iter()).all(|(on_arg, recv_arg)| {
                on_arg == recv_arg
                    || matches!(on_arg, Ty::Named {
                        name,
                        args: ArgListId::EMPTY,
                        ..
                    } if type_params.contains(&name))
            })
    }

    /// The operator application on a known receiver type (`c + d`, `c[i]`,
    /// `c.j + 1` against the member result): each read/write spelling that
    /// the type's container genuinely lacks reports `undefined_operator` at
    /// the operator's anchor span — unless an applicable extension declares
    /// the spelling (the implicit-application face resolves user operators
    /// too: `x[0]` over an extension `operator []`, upstream Dart SDK
    /// source, `pkg/analyzer/lib/src/dart/resolver/extension_member_resolver.dart`,
    /// `getExtension` + the `[]`-operator lookup). Each index spelling is its
    /// own lookup key — a read never satisfies through `[]=`, a compound
    /// write needs both. Unresolved or external types (`Unknown`, broken
    /// chains) suppress — an invalid receiver type never cascades.
    pub(crate) fn check_operator_expr(&mut self, ty: Ty, row: &Row) {
        let Intent::Operator(_, op_range) = row.intent else {
            return;
        };
        let (read, write) = self.operator_spellings(row);
        let Ty::Named { name, .. } = ty else {
            return;
        };
        // An unindexable receiver still answers through the frozen faces:
        // `Match`/`Stream`-typed operands chain-break the walk at the root,
        // but the interface table decides a user operator's absence exactly
        // (`x + 1` on `Match` — `undefined_operator`); a genuinely
        // unindexed declaration keeps the under-report.
        let indexed = resolve_container(
            self.graph,
            self.namespaces,
            self.index,
            self.lib,
            self.file,
            name,
        )
        .is_some();
        let mut missing: Vec<String> = Vec::new();
        for spelling in [read, write].into_iter().flatten() {
            let walk_missing = if indexed {
                matches!(self.walk_members(name, spelling), Walk::Missing)
            } else {
                matches!(
                    self.frozen_member_verdict(name, spelling),
                    FrozenFace::Absent
                )
            };
            if walk_missing {
                match self.extension_application(name, ty, spelling, false, false) {
                    ExtensionApp::None => {
                        missing.push(self.strings.resolve(spelling).to_string());
                    }
                    ExtensionApp::Single | ExtensionApp::UnsafeSingle => {}
                    ExtensionApp::Ambiguous(displays) => {
                        // The ambiguity verdict replaces the member family
                        // (the oracle reports the ambiguous application, not
                        // the operator miss) — anchored at the operator.
                        self.emit_extension_ambiguity(spelling, op_range, displays);
                    }
                }
            }
        }
        if missing.is_empty() {
            return;
        }
        let type_text = self.strings.resolve(name).to_string();
        for text in missing {
            self.emit(SemanticCode::UndefinedOperator, op_range, &text, &type_text);
        }
    }

    /// Whether an instance field's declaration initializer is open: the
    /// variable-initializer bracket (`in_initializer`) inside a member
    /// field group at the group's own frame depth (a closure body inside
    /// the initializer is not the initializer), with the group non-static
    /// — the instance context `_checkForImplicitThisReferenceInInitializer`
    /// arms on.
    pub(crate) fn in_instance_field_initializer(&self) -> bool {
        // A `late` instance field's initializer is lazy — `this` is
        // in scope there (upstream `SuperContext.fieldDeclaration`'s
        // late arm falls through to the enclosing class), so the
        // `_checkForImplicitThisReferenceInInitializer` face arms only
        // on a non-late instance group.
        self.in_initializer
            && self.field_group_open
            && self.init_frame_depth == self.field_group_depth
            && !self.open_member_static
            && !self.group_late
    }

    /// Runs the walk(s) an access needs and applies the shared suppressions
    /// (broken chain, extension application, `Object` members). A write also
    /// accepts the setter spelling (`name=`): a same-named getter, field, or
    /// method suppresses with the oracle's assignment-to-* codes, a setter
    /// defines the write.
    pub(crate) fn member_defined(
        &mut self,
        container: StringId,
        receiver_ty: Ty,
        row: &Row,
        super_walk: bool,
        receiver_nullable: bool,
    ) -> bool {
        let write = matches!(row.intent, Intent::Write | Intent::ReadWrite);
        // The write target resolves the SETTER side
        // (`assignment_expression_resolver.dart` `_resolveSimpleAssignment`:
        // `writeElement` is the `name=` lookup): a setter or a field
        // satisfies the write cleanly, while a same-named getter or method
        // hit still leaves `undefined_setter` silent — the oracle routes it
        // to the assignment-to-* family (`assignment_to_final_no_setter` on
        // a getter, `assignment_to_method` on a method), decided elsewhere.
        // `undefined_setter` fires only when neither spelling declares
        // anything.
        let found = if write {
            let named = if super_walk {
                self.walk_super(container, row.name)
            } else {
                self.walk_members(container, row.name)
            };
            matches!(named, Walk::Found(_)) || {
                let spelling = self.setter_spelling(row.name);
                self.member_found(container, spelling, super_walk)
            }
        } else {
            self.member_found(container, row.name, super_walk)
        };
        if found {
            // The nullable-dereference face (nullable_dereference_verifier
            // .dart report): an interface member reached through a
            // merely-nullable receiver reads a possibly-null receiver —
            // `unchecked_use_of_nullable_value` at the selector. A member
            // applied through a nullable `on` extension face never
            // dereferences (the applicability test already accepted the
            // nullable type), and a `?.` access unwrapped the receiver —
            // both stay silent here.
            if receiver_nullable {
                let name_text = self.strings.resolve(row.name);
                self.emit_corrected(
                    SemanticCode::UncheckedUseOfNullableValue,
                    row.range,
                    name_text,
                );
            }
            return true;
        }
        if row.name == self.intern("==") || self.members.object_members.contains(&row.name) {
            return true;
        }
        if super_walk {
            // Extensions do not apply to `super` accesses; the conservative
            // any-face gate keeps the under-report direction for the
            // unresolvable-receiver shapes.
            return self
                .index
                .get(self.file)
                .is_some_and(|f| f.has_extension_member(row.name));
        }
        match self.extension_application(container, receiver_ty, row.name, write, receiver_nullable)
        {
            ExtensionApp::None => false,
            ExtensionApp::Single => true,
            // The member resolved through a non-nullable `on` face against a
            // nullable receiver (`x.m` on `C?` where `extension E on C`
            // declares `m`): the access resolves the member but reads a
            // possibly-null receiver — the oracle's unsafe-access row
            // (`unchecked_use_of_nullable_value` at the selector), never
            // the `undefined_*` face. A `?.` access already read the
            // receiver non-null, so `receiver_nullable` is false there.
            ExtensionApp::UnsafeSingle if receiver_nullable => {
                let name_text = self.strings.resolve(row.name);
                self.emit_corrected(
                    SemanticCode::UncheckedUseOfNullableValue,
                    row.range,
                    name_text,
                );
                true
            }
            ExtensionApp::UnsafeSingle => true,
            ExtensionApp::Ambiguous(displays) => {
                self.emit_extension_ambiguity(row.name, row.range, displays);
                // The verdict leaves the property unresolved for the
                // null-safety walk: a nullable receiver reports the
                // unchecked-access face beside it (the oracle's paired rows
                // — both codes at the same access, the property variant of
                // the shared code).
                if receiver_nullable {
                    let name_text = self.strings.resolve(row.name);
                    self.emit_corrected(
                        SemanticCode::UncheckedUseOfNullableValue,
                        row.range,
                        name_text,
                    );
                }
                true
            }
        }
    }

    /// The implicit-application outcome for `member` against a receiver
    /// whose resolved container is `receiver`
    /// (`extension_member_resolver.dart:85-128` `findExtension` +
    /// `applicable_extensions.dart:157-171` `applicableTo`): the same-file
    /// faces whose `on` type spells the receiver's name and that declare the
    /// member under its base spelling — or, when `accept_setter` holds, the
    /// member under its base or setter spelling (write targets only; a read
    /// never resolves through a setter, so `x.foo` with a face declaring
    /// only `foo=` still reports). A face with an absent or unresolvable
    /// `on` type never matches a resolved receiver — its members are
    /// suppressed from application (the oracle's `extendedType` fails to
    /// resolve; an inapplicable extension never gates an `undefined_*`).
    ///
    /// Two or more applicable candidates are never separable at the M1 name
    /// level: `_isMoreSpecific` (`extension_member_resolver.dart:469-513`)
    /// decides on SDK-ness and subtype relations between the instantiated
    /// `on` types, and same-named `on` types answer equal — so the access is
    /// ambiguous by the oracle's definition. The `Ambiguous` payload carries
    /// the candidates' display pairs (the error path only; `None`/`Single`
    /// allocate nothing).
    pub(crate) fn extension_application(
        &mut self,
        receiver: StringId,
        receiver_ty: Ty,
        member: StringId,
        accept_setter: bool,
        receiver_nullable: bool,
    ) -> ExtensionApp {
        let setter = self.setter_spelling(member);
        // Applicable faces in declaration order, bucketed by specificity:
        // against a non-nullable receiver a non-nullable `on` type
        // dominates its nullable-`on` twins (`_isMoreSpecific`). A nullable
        // receiver does NOT remove the member — it resolves through the
        // non-nullable `on` face too, but the access is unsafe
        // (`unchecked_use_of_nullable_value`): the `UnsafeSingle` verdict
        // carries that distinction. Equal on-types answer equal — two
        // non-nullable faces stay ambiguous.
        // The face cache borrows `self` mutably (the lazy fill); every
        // immutable lookup it needs is hoisted ahead so the borrow region
        // only spans the walk below.
        let world = self.name_world();
        let object = self.intern("Object");
        let iterable = self.intern("Iterable");
        let strings = self.strings;
        // The lazy fill needs `&mut self`; its result slice then lives as a
        // shared borrow so the argument gate can reach the boundary pool.
        self.accessible_extension_faces();
        let boundary = &self.boundary;
        let faces = self.accessible_extensions.as_deref().unwrap_or(&[]);
        // Applicability is the receiver's subtype relation to the `on`
        // type (`extension_member_resolver.dart` `_getApplicable`: the
        // receiver is assignable to the instantiated extended type), so an
        // extension on `I<T>` applies to `J extends I<T>` — the declared
        // supertype lattice, not name equality.
        // The name-level subtype walk plus the faces it cannot see: every
        // type reaches `Object`, and the unindexed SDK collection roots.
        // `List`/`Set` reach `Iterable` by fiat; the `dart:collection` bases
        // reach every interface their alias entry declares (`HashMap <: Map`,
        // `HashSet <: Set`, `ListQueue <: Queue` — the stub declarations the
        // declared-lattice walk cannot see).
        let reaches = |sub: StringId, sup: StringId| {
            sub == sup
                || sup == object
                || world.named_sub(sub, sup)
                || (sup == iterable && matches!(strings.resolve(sub), "List" | "Set"))
                || crate::member_face::COLLECTION_INTERFACE_ALIASES
                    .iter()
                    .any(|(alias, roots)| {
                        *alias == strings.resolve(sub) && roots.contains(&strings.resolve(sup))
                    })
        };
        // A head the index resolves to a user-declared container carries a
        // complete written-clause model, so the name-level walk over it is
        // exact and a miss is a real miss — an `extension type LookupName`
        // with no `implements` reaches nothing. An SDK builtin head
        // (`String`, `int` — no container anywhere) or an excerpt-declared
        // one answers nothing here: its real supertypes (`String implements
        // Comparable<String>`) are not modeled, so the gates below keep
        // the face (the under-report direction). An owner the graph never
        // registered (the single-file rigs) is in-file, hence user.
        let user_container = |head: StringId| -> bool {
            resolve_container(
                self.graph,
                self.namespaces,
                self.index,
                self.lib,
                self.file,
                head,
            )
            .is_some_and(|(_, owner)| {
                self.graph
                    .library_of_file(owner)
                    .is_none_or(|lib| !self.graph.uri(lib).starts_with("dart:"))
            })
        };
        // The instantiated argument's verdict against a face parameter's
        // declared bound (`applicableTo`'s `tryChooseFinalTypes` bound
        // check): `arg <: bound`. An in-scope type parameter answers
        // through its own declared bound — the oracle's
        // `TypeParameterType` subtypes a bound exactly when its own bound
        // does (`subtype.dart:306-317`), so `T extends Enum` satisfies
        // `Enum` while the unbounded `T` reads `Object?` and a concrete
        // bound rejects it (the #2497 `values.byName` row over
        // `List<T>`/`Iterable<T>`). A bound written in terms of a
        // parameter (`T extends S`) chains through `S`'s bound; the hop
        // count caps the recovered cycle (`T extends T`). A shape the
        // name level cannot order keeps the face (the under-report
        // direction).
        let satisfies_bound = |receiver_arg: Ty,
                               bound: Ty,
                               bindings: &[(dartq_types::TypeParamId, dartq_types::TypeId)]|
         -> bool {
            let mut arg = receiver_arg;
            let mut hops = 0u8;
            while let Ty::Named {
                name,
                nullable,
                args: ArgListId::EMPTY,
            } = arg
            {
                // The scope walk has finished by the time the row runs
                // (`frames`/`type_params` are already torn down), so the
                // in-scope test can't see `T`. Its bound survives only in
                // the per-executable/container captures — `param_bound_of`
                // answers the written bound (or the implicit `Object?`)
                // for a name a capture actually declares, `None` for a
                // concrete head (`String`/`Color`), which stays put.
                let Some(declared) = self.param_bound_of(name) else {
                    break;
                };
                // The argument's own `?` survives the substitution: a
                // `T?` instantiation never satisfies a non-nullable
                // bound even when `T`'s declared bound does.
                arg = if nullable {
                    match declared {
                        Ty::Named { name, args, .. } => Ty::Named {
                            name,
                            nullable: true,
                            args,
                        },
                        other => other,
                    }
                } else {
                    declared
                };
                hops += 1;
                if hops >= 8 {
                    break;
                }
            }
            // #2508: both sides concrete — the snapshot's structural
            // subtype answers the bound check (`arg <: bound[T := arg]`,
            // `tryChooseFinalTypes`' instantiation before the subtype
            // test) where the name-level match below can only
            // pattern-match heads. The face-parameter bindings substitute
            // first, so a recursive bound (`T extends Comparable<T>`)
            // reads `Comparable<arg>` exactly like the oracle's inference
            // result. A `None` — an unmodelled arg or bound, or a bound
            // that still mentions a parameter after the substitution —
            // keeps the name-level arms verbatim (the under-report
            // direction). The parameter-arg chain above already resolved
            // every in-scope parameter through `param_bound_of`; a name
            // that still maps to a declared parameter (`type_params` is
            // name-keyed — a container-level parameter no exec window
            // declares) is NOT a concrete pair, so the strict arm
            // declines it the same way.
            if let Some(ops) = self.ops {
                let arg_id = ops.map_ty_strict(arg, &self.boundary.args.borrow());
                let bound_id = ops
                    .map_ty_strict(bound, &self.boundary.args.borrow())
                    .map(|bound_id| ops.substitute(bound_id, bindings));
                if let (Some(arg_id), Some(bound_id)) = (arg_id, bound_id)
                    && !ops.mentions_param(arg_id)
                    && !ops.mentions_param(bound_id)
                    && ops.subtype_ids(arg_id, bound_id)
                {
                    // A structural subtype path is a proof; a structural
                    // `false` is not — the snapshot's hierarchy carries only
                    // the edges the captures fed (an SDK `num` without its
                    // `Comparable<num>` face), so the negative falls to
                    // the name-level arms below.
                    return true;
                }
            }
            match (arg, bound) {
                // `dynamic <: Object` and `dynamic <: Object?` — the only
                // tops a `dynamic` instantiation satisfies; any concrete
                // bound rejects it.
                (Ty::Dynamic, Ty::Named { name, args, .. }) => {
                    args == ArgListId::EMPTY && name == object
                }
                (
                    Ty::Named {
                        name: arg_head,
                        nullable: arg_nullable,
                        args: arg_args,
                    },
                    Ty::Named {
                        name: bound_head,
                        nullable: bound_nullable,
                        args: bound_args,
                    },
                ) => {
                    // A bound carrying its own arguments (the recursive
                    // `T extends Comparable<T>`) decides at the head level
                    // when the bare argument head is a user container —
                    // `extension type LookupName` never satisfies
                    // `Comparable<T>` (the oracle's `applicableTo` fails
                    // the inference and the face never applies), while a
                    // user `implements Comparable<Self>` passes. A nested
                    // argument or an unmodeled argument head keeps the
                    // face (under-report).
                    if arg_args == ArgListId::EMPTY && bound_args != ArgListId::EMPTY {
                        return (!arg_nullable || bound_nullable)
                            && (!user_container(arg_head)
                                || arg_head == bound_head
                                || reaches(arg_head, bound_head));
                    }
                    if arg_args != ArgListId::EMPTY || bound_args != ArgListId::EMPTY {
                        return true;
                    }
                    (!arg_nullable || bound_nullable)
                        && (arg_head == bound_head || reaches(arg_head, bound_head))
                }
                // A shape the name level cannot order (an unknown
                // instantiation, a record or function bound) keeps the
                // face.
                _ => true,
            }
        };
        // A nullable `on` argument position (`on Iterable<T?>`) binds the
        // parameter to the receiver argument's non-nullable core — the
        // inferrer matches `Iterable<L?>` against `Iterable<T?>` as
        // `T := L` — so the bound check reads the core, never the
        // receiver's own `?` (`NullableIterableExtensions<T extends
        // Object>.nonNulls` applies to every `Iterable<X?>`, the face's
        // whole purpose).
        let bind_through_nullable = |on_nullable: bool, receiver_arg: Ty| -> Ty {
            match receiver_arg {
                Ty::Named {
                    name,
                    nullable: true,
                    args,
                } if on_nullable => Ty::Named {
                    name,
                    nullable: false,
                    args,
                },
                other => other,
            }
        };
        // The `on` clause's argument list gates beside the head
        // (`applicable_extensions.dart:205-281` `applicableTo`: the oracle
        // infers the extended type's fresh parameters against the receiver
        // — `on Map<K, FieldItem>` never applies to a
        // `Map<K, MethodItem>` receiver, while an argument spelling the
        // extension's own type parameter instantiates and always passes).
        // The name-level read compares concrete arguments only when the
        // two sides share the head — a subtype hop's argument projection
        // stays unmodeled (the under-report direction), and a side that
        // never interned its list (`Unknown`, a bare head) skips the gate.
        let arg_applicable = |face: &ExtensionFace, face_file: FileId| {
            // The face's `on` type interns in its declaring file's pool —
            // a foreign face's argument list projects across the boundary
            // before the diagnosing pool reads it (same-file faces pass
            // through untouched).
            let on_ty = boundary.ty(face_file, face.on_ty);
            let (
                Ty::Named {
                    name: receiver_head,
                    args: receiver_args,
                    ..
                },
                Ty::Named {
                    name: on_head,
                    args: on_args,
                    ..
                },
            ) = (receiver_ty, on_ty)
            else {
                return true;
            };
            let receiver_args = boundary.pool().get(receiver_args).to_vec();
            let on_args = boundary.pool().get(on_args).to_vec();
            // A raw-head receiver (`Map x`, `List xs`, `JSArray a`)
            // instantiates each of ITS OWN declared parameters to that
            // parameter's bound (`instantiateToBounds`: `List<dynamic>`,
            // `JSArray<JSAny?>`) — the receiver DECLARATION's bounds
            // (`Container::type_param_bounds`, projected across its file
            // boundary), never the face's: the face's parameter bound is
            // the question's right-hand side. An unbounded or unmodeled
            // parameter reads `dynamic`, so a bare `Map` against `on Map<K,
            // FieldItem>` fails `dynamic <: FieldItem` and a bare `List`
            // against `T extends AccessorFragment` fails its bound, exactly
            // as the oracle's `applicableTo` rejects both (probed 3.13.1:
            // `List xs; xs.withOrigin` over two bounded `on List<T>` faces
            // is `undefined_getter`, never ambiguous), while a bare
            // `JSArray` reads `JSArray<JSAny?>` and `JSArrayToList<T extends
            // JSAny?>` applies.
            let receiver_args = if receiver_args.is_empty() && !on_args.is_empty() {
                let declared: Vec<Ty> = resolve_container(
                    self.graph,
                    self.namespaces,
                    self.index,
                    self.lib,
                    self.file,
                    receiver_head,
                )
                .map(|(decl, owner)| {
                    decl.type_param_bounds
                        .iter()
                        .map(|&bound| boundary.ty(owner, bound))
                        .collect()
                })
                .unwrap_or_default();
                (0..on_args.len())
                    .map(|index| declared.get(index).copied().unwrap_or(Ty::Dynamic))
                    .collect()
            } else {
                receiver_args
            };
            // #2508: the face-parameter binding table the strict bound
            // check substitutes — `applicableTo` binds each `on`
            // parameter to the receiver's argument at the same position
            // (the identity projection this whole `arg_applicable` arm
            // already assumes), then asks `arg <: bound[T:=arg]`. A
            // receiver argument that maps nothing structural leaves its
            // slot out, and a bound leaf over that parameter keeps the
            // `TypeParameter` shape — the strict arm declines it (the
            // under-report direction).
            let strict_bindings: Vec<(dartq_types::TypeParamId, dartq_types::TypeId)> = match self
                .ops
            {
                Some(ops) if receiver_args.len() == on_args.len() => {
                    let pool = boundary.args.borrow();
                    receiver_args
                        .iter()
                        .zip(on_args.iter())
                        .filter_map(|(&receiver_arg, &on_arg)| {
                            let Ty::Named {
                                name: param_name,
                                nullable: on_nullable,
                                args: param_args,
                            } = on_arg
                            else {
                                return None;
                            };
                            if param_args != ArgListId::EMPTY {
                                return None;
                            }
                            let param = ops.param_id(param_name)?;
                            let bound_arg =
                                bind_through_nullable(on_nullable, receiver_arg);
                            Some((param, ops.map_ty_strict(bound_arg, &pool)?))
                        })
                        .collect()
                }
                _ => Vec::new(),
            };
            if receiver_head != on_head {
                // The subtype hop's argument projection is unmodeled; the
                // one decision still decidable across it is the applied
                // face's parameter bounds. `applicableTo` instantiates
                // each `on` parameter to the receiver's corresponding
                // argument — the identity mapping every SDK collection
                // edge writes (`List<E> implements Iterable<E>`) — then
                // fails the inference when the instantiation violates the
                // parameter's declared bound (`Iterable<String>` never
                // applies `EnumByName<T extends Enum>`; an unbounded `T`
                // receiver fails the same way, #2497). A concrete `on`
                // argument gates the receiver's argument covariantly under
                // the same identity projection, decidable only when the
                // receiver argument head is a user container
                // (`on Iterable<BaseName>` never applies to
                // `Set<LookupName>`). Positions whose receiver argument is
                // unmodeled, nested, or raw-defaulted, or whose `on`
                // argument is parameterized or arity-mismatched, stay
                // unmodeled (the under-report direction).
                if receiver_args.len() != on_args.len() {
                    return true;
                }
                return receiver_args.iter().zip(on_args.iter()).all(
                    |(&receiver_arg, &on_arg)| {
                        let Ty::Named {
                            name: arg_name,
                            nullable: on_nullable,
                            args,
                        } = on_arg
                        else {
                            return true;
                        };
                        if args != ArgListId::EMPTY {
                            return true;
                        }
                        let Some(param_index) =
                            face.type_param_names.iter().position(|&n| n == arg_name)
                        else {
                            let Ty::Named {
                                name: receiver_arg_head,
                                args: receiver_arg_args,
                                ..
                            } = receiver_arg
                            else {
                                return true;
                            };
                            return receiver_arg_args != ArgListId::EMPTY
                                || !user_container(receiver_arg_head)
                                || reaches(receiver_arg_head, arg_name);
                        };
                        let bound = face
                            .type_param_bounds
                            .get(param_index)
                            .copied()
                            .unwrap_or(Ty::Dynamic);
                        let bound = boundary.ty(face_file, bound);
                        if matches!(bound, Ty::Dynamic) {
                            return true;
                        }
                        satisfies_bound(
                            bind_through_nullable(on_nullable, receiver_arg),
                            bound,
                            &strict_bindings,
                        )
                    },
                );
            }
            if receiver_args.len() != on_args.len() {
                return true;
            }
            receiver_args
                .iter()
                .zip(on_args.iter())
                .all(|(&receiver_arg, &on_arg)| {
                    // An `on` argument spelling one of the extension's own
                    // type parameters instantiates against the receiver —
                    // the inferrer binds it, then checks the instantiated
                    // bound: an argument the bound rejects fails the
                    // inference and the face never applies
                    // (`applicable_extensions.dart` `applicableTo`'s
                    // `tryChooseFinalTypes`), the
                    // `_ListOfPropertyAccessorFragment<T extends
                    // PropertyAccessorFragmentImpl> on List<T>` shape. A
                    // raw receiver instantiates to the `dynamic` row
                    // below, which every concrete bound rejects — the
                    // oracle answers `undefined_getter` on `List<dynamic>`
                    // there (probed, /tmp/probe2431/lib/b.dart:38).
                    if let Ty::Named {
                        name: arg_name,
                        nullable: on_nullable,
                        args,
                    } = on_arg
                        && args == ArgListId::EMPTY
                        && let Some(param_index) =
                            face.type_param_names.iter().position(|&n| n == arg_name)
                    {
                        let bound = face
                            .type_param_bounds
                            .get(param_index)
                            .copied()
                            .unwrap_or(Ty::Dynamic);
                        let bound = boundary.ty(face_file, bound);
                        if matches!(bound, Ty::Dynamic) {
                            return true;
                        }
                        return satisfies_bound(
                            bind_through_nullable(on_nullable, receiver_arg),
                            bound,
                            &strict_bindings,
                        );
                    }
                    // Covariant positions: the receiver's argument must be
                    // a subtype of the `on` argument. Only a bare concrete
                    // pair can fail that test at the name level — a nested
                    // argument list on either side stays unmodeled and
                    // compatible (under-report).
                    let (
                        Ty::Named {
                            name: arg_head,
                            args: arg_args,
                            ..
                        },
                        Ty::Named {
                            name: receiver_head,
                            args: receiver_args,
                            ..
                        },
                    ) = (on_arg, receiver_arg)
                    else {
                        // A `dynamic` receiver argument subtypes every
                        // top-shaped `on` argument (`dynamic <: Object` and
                        // `dynamic <: Object?` — the oracle's `applicableTo`
                        // runs the real subtype test, and both tops accept
                        // it); a concrete `on` argument still rejects it.
                        // Every other non-named pair stays compatible.
                        if matches!(receiver_arg, Ty::Dynamic) {
                            return match on_arg {
                                Ty::Dynamic => true,
                                Ty::Named { name, .. } => name == object,
                                _ => false,
                            };
                        }
                        return true;
                    };
                    if arg_args != ArgListId::EMPTY || receiver_args != ArgListId::EMPTY {
                        return true;
                    }
                    reaches(receiver_head, arg_head)
                })
        };
        // The receiver-container provenance gate (#2504): the oracle's
        // `applicableTo` runs the extended type against the receiver's
        // RESOLVED type, so a face applies only when the receiver's
        // declaration — or a written supertype of it, each resolved in its
        // own declaring scope — IS the declaration the face's `on` head
        // names. The name-level `reaches` walk cannot tell a local
        // `class Iterable<T>` from `dart:core`'s, and its fiat collection
        // edges (`List`/`Set` → `Iterable`, the alias table) are facts
        // about the SDK's own containers alone — a local `List` with no
        // written clause never reaches the SDK's `Iterable`, and a local
        // `Iterable` shadow must not attract the SDK's `EnumByName`. The
        // gate engages only when the face's `on` head is declared in the
        // face's own file (the bundled SDK excerpts and every
        // self-contained user face); a face whose `on` head imports from
        // elsewhere keeps the ungated posture (nothing in-file to compare
        // against). The walk accepts only a step that lands ON the `on`
        // head's own declaration (same name, same file) — the receiver
        // itself or a WRITTEN clause hop (`extends`/`with`/`implements`, a
        // mixin's `on` constraints), each resolved in its owner's own
        // scope so a shadow in the diagnosing unit never reaches a
        // foreign declaration's clause. An unresolvable receiver head or
        // clause hop keeps the face (the under-report direction); the
        // walk stays bounded by its visited set with a visit cap.
        let provenance_ok =
            |face_file: FileId, receiver_head: StringId, on_head: StringId| -> bool {
                if self
                    .index
                    .get(face_file)
                    .and_then(|elements| elements.container(on_head))
                    .is_none()
                {
                    return true;
                }
                let mut queue: Vec<(StringId, LibraryId, FileId)> =
                    vec![(receiver_head, self.lib, self.file)];
                let mut seen: Vec<(FileId, StringId)> = Vec::new();
                let mut unresolved = false;
                let mut visits = 0usize;
                while let Some((name, scope_lib, scope_file)) = queue.pop() {
                    visits += 1;
                    if visits > 256 {
                        return true;
                    }
                    let Some((decl, owner)) = resolve_container(
                        self.graph,
                        self.namespaces,
                        self.index,
                        scope_lib,
                        scope_file,
                        name,
                    )
                    else {
                        unresolved = true;
                        continue;
                    };
                    if !seen.contains(&(owner, name)) {
                        seen.push((owner, name));
                    } else {
                        continue;
                    }
                    if name == on_head && owner == face_file {
                        return true;
                    }
                    // The clause names resolve in the owner's own library;
                    // an owner the graph never registered as a unit (the
                    // single-file rigs) falls back to the scope that
                    // resolved this hop — for an in-file owner that IS its
                    // library (the under-report direction for a foreign
                    // one).
                    let owner_lib = self.graph.library_of_file(owner).unwrap_or(scope_lib);
                    for sup in decl
                        .superclass
                        .iter()
                        .chain(decl.with_clause.iter())
                        .chain(decl.implements_clause.iter())
                        .chain(decl.on_clause.iter())
                    {
                        queue.push((*sup, owner_lib, owner));
                    }
                }
                // The walk exhausted without landing on the on-head's own
                // declaration: inapplicable, unless a hop it could not
                // resolve may have hidden the path (keep the face).
                unresolved
            };
        // The oracle's applicability test instantiates the extended type
        // against the receiver's RESOLVED type — an `InvalidType` receiver
        // never applies an extension (`applicableTo` returns no
        // `ExtensionResolution`), so the member stays uncredited and the
        // access keeps its own `undefined_*` face. The name-level
        // `InvalidType` read is [`Self::extension_receiver_resolves`]; a
        // `dynamic` receiver dispatches dynamically — no extension binds it
        // either.
        // An extension body's `this` receiver carries the container as its
        // head (the extension's own name resolves NonType — a getter-scope
        // element, not a type): the `on` clause's interned `Ty` is the
        // resolvable receiver shape, so the head check defers to it there.
        let receiver_valid = !matches!(receiver_ty, Ty::Dynamic)
            && if receiver_ty == Ty::UNKNOWN {
                self.type_resolution(receiver).0 == TypeResolution::Type
            } else {
                self.extension_receiver_resolves(receiver_ty)
            };
        // #2508: the mounted `LibraryTypes` snapshot answers the
        // applicability test structurally — `applicableTo` instantiates
        // the extended type's parameters against the receiver's RESOLVED
        // type (`constrainArgument` over the supertype projection) and
        // asks `isSubtypeOf(receiver, extendedType)`, so a generic hop's
        // argument projection (`Box<int>` reaching `Iterable<Wrapper<int>>`
        // against `on Iterable<T>`) decides by the real lattice instead
        // of the name-level argument gate. The `on` type's own parameter
        // leaves bind through the face's declared parameter names
        // (`snapshot.type_params` is name-keyed — a name no declaration
        // registers simply binds nothing). A `None` verdict — an
        // unmodeled receiver/on shape, an unregistered head, a bound the
        // capture never fed — keeps the whole name-level gate chain
        // verbatim (the under-report direction).
        let ops = self.ops;
        let applies_strict = |face: &ExtensionFace, face_file: FileId, on_ty: Ty| -> Option<bool> {
            let ops = ops?;
            // The face's own bounds ride the question explicitly
            // ([`Engine::strict_face_params`]); a bound the snapshot cannot
            // map declines the whole strict verdict.
            let face_params = self.strict_face_params(face, face_file)?;
            ops.extension_applies_strict(receiver_ty, on_ty, &face_params, &boundary.args.borrow())
        };
        let mut applicable: Vec<(&ExtensionFace, Ty, FileId)> = Vec::new();
        if receiver_valid {
            for &(face, face_file) in faces {
                let Some(on) = face.on_type else { continue };
                if !(face.members.contains_key(&member)
                    || (accept_setter && face.members.contains_key(&setter)))
                {
                    continue;
                }
                // The full `on` type projects into the diagnosing pool
                // ahead of both gates — the structural arm needs the
                // projected shape, and the name-level arm reads it
                // verbatim below.
                let on_ty = boundary.ty(face_file, face.on_ty);
                // A structural `true` is a proof the face applies; a
                // structural `false` (or no fact) hands the question to the
                // name-level gate chain — the snapshot's hierarchy is only
                // as complete as the captures that fed it.
                let applicable_face = matches!(applies_strict(face, face_file, on_ty), Some(true))
                    || (reaches(receiver, on)
                        && provenance_ok(face_file, receiver, on)
                        && arg_applicable(face, face_file));
                if !applicable_face {
                    continue;
                }
                applicable.push((face, on_ty, face_file));
            }
        }
        // `_isMoreSpecific` steps 3-4 at the name level: a face whose
        // instantiated `on` type is a strict subtype of another applicable
        // face's dominates it (`extension_member_resolver.dart:467-491`).
        // The name-level subtype reads the two `on` types' shape: equal
        // heads compare covariant argument-wise — a non-nullable argument
        // subtypes its nullable twin (`List<ManifestNode>` vs
        // `List<ManifestNode?>`), a subtype argument subtypes its
        // supertype (`Map<K, FieldItem>` vs `Map<K, ManifestItem>`) — and
        // unequal heads ride the declared supertype walk. A shape the
        // name level cannot order (an own type parameter, nested argument
        // lists, unknowns) answers compatible; those pairs fall to step
        // 5's instantiate-to-bounds tail below.
        let on_subtype = |sub: Ty, sup: Ty| -> bool {
            // #2508: the snapshot orders the instantiated `on` types
            // structurally where it can — a generic face's
            // `Iterable<Wrapper<int>>` subtyping its `Iterable<Base>`
            // twin decides through the real lattice, not the covariant
            // head-argument read below. `None` keeps the name-level arms.
            if self
                .ops
                .and_then(|ops| ops.try_subtype_strict(sub, sup, &self.boundary.args.borrow()))
                == Some(true)
            {
                return true;
            }
            let (
                Ty::Named {
                    name: sub_head,
                    nullable: sub_nullable,
                    args: sub_args,
                },
                Ty::Named {
                    name: sup_head,
                    nullable: sup_nullable,
                    args: sup_args,
                },
            ) = (sub, sup)
            else {
                return true;
            };
            if sub_nullable && !sup_nullable {
                return false;
            }
            if sub_head != sup_head {
                return reaches(sub_head, sup_head);
            }
            if sub_args == ArgListId::EMPTY || sup_args == ArgListId::EMPTY {
                return true;
            }
            // The pools are cold-path reads (ambiguity resolution only);
            // the argument gate's same to_vec copy sidesteps the temporary
            // borrow.
            let sub_args = boundary.pool().get(sub_args).to_vec();
            let sup_args = boundary.pool().get(sup_args).to_vec();
            if sub_args.len() != sup_args.len() {
                return true;
            }
            sub_args.iter().zip(sup_args.iter()).all(|(&a, &b)| {
                let (
                    Ty::Named {
                        name: a_head,
                        nullable: a_nullable,
                        args: a_args,
                    },
                    Ty::Named {
                        name: b_head,
                        nullable: b_nullable,
                        args: b_args,
                    },
                ) = (a, b)
                else {
                    return true;
                };
                if a_nullable && !b_nullable {
                    return false;
                }
                if a_args != ArgListId::EMPTY || b_args != ArgListId::EMPTY {
                    return true;
                }
                a_head == b_head || reaches(a_head, b_head)
            })
        };
        // Step 5's instantiate-to-bounds projection
        // (`extension_member_resolver.dart:459-466` `_instantiateToBounds`):
        // every bare own-parameter position in the `on` type's arguments is
        // replaced by its declared bound — a bound-less parameter by
        // `dynamic`, the Dart reading of unbounded. A bound written in
        // terms of its own parameter (the recursive
        // `T extends Comparable<T>`) substitutes as written; its residual
        // parameter rides the comparison as the unmodeled shape. A
        // non-parameter argument, an argument-less `on` type, or a face
        // without type parameters projects to itself.
        let bound_projected = |face: &ExtensionFace, face_file: FileId, on_ty: Ty| -> Ty {
            let Ty::Named {
                name,
                nullable,
                args,
            } = on_ty
            else {
                return on_ty;
            };
            if args == ArgListId::EMPTY || face.type_param_names.is_empty() {
                return on_ty;
            }
            let on_args = boundary.pool().get(args).to_vec();
            let projected: Vec<Ty> = on_args
                .iter()
                .map(|&arg| {
                    let Ty::Named {
                        name: param,
                        args: param_args,
                        ..
                    } = arg
                    else {
                        return arg;
                    };
                    if param_args != ArgListId::EMPTY {
                        return arg;
                    }
                    let Some(index) = face.type_param_names.iter().position(|&n| n == param) else {
                        return arg;
                    };
                    let bound = face
                        .type_param_bounds
                        .get(index)
                        .copied()
                        .unwrap_or(Ty::Dynamic);
                    boundary.ty(face_file, bound)
                })
                .collect();
            let projected_args = boundary.args.borrow_mut().named(name, nullable, &projected);
            match projected_args {
                Ty::Named { args, .. } => Ty::Named {
                    name,
                    nullable,
                    args,
                },
                _ => on_ty,
            }
        };
        // Step 5's pair test at the projections' name level. Unlike the
        // steps 3-4 read, the projections' arguments carry the bounds, so
        // the unbounded parameter's `dynamic` instantiation decides
        // strictly: it subtypes only the tops (`dynamic` itself and the
        // argument-less `Object`/`Object?`) and every concrete bound
        // rejects it — the `IterableComparableExtension<T extends
        // Comparable<T>>`-over-`IterableExtension<T>` ordering (probed,
        // /tmp/probe2431/lib/a.dart and d.dart). A shape the name level
        // cannot model (a residual parameter inside a bound, nested
        // argument lists) answers unordered — the pair stays mutually
        // compatible and step 5 decides nothing (the under-report
        // direction).
        let bounds_subtype = |sub: Ty, sup: Ty| -> bool {
            let (
                Ty::Named {
                    name: sub_head,
                    nullable: sub_nullable,
                    args: sub_args,
                },
                Ty::Named {
                    name: sup_head,
                    nullable: sup_nullable,
                    args: sup_args,
                },
            ) = (sub, sup)
            else {
                return false;
            };
            if sub_nullable && !sup_nullable {
                return false;
            }
            if sub_head != sup_head {
                return reaches(sub_head, sup_head);
            }
            if sub_args == ArgListId::EMPTY || sup_args == ArgListId::EMPTY {
                return true;
            }
            let sub_args = boundary.pool().get(sub_args).to_vec();
            let sup_args = boundary.pool().get(sup_args).to_vec();
            if sub_args.len() != sup_args.len() {
                return true;
            }
            sub_args.iter().zip(sup_args.iter()).all(|(&a, &b)| {
                // `dynamic <: dynamic`, and `dynamic <: Object`/`Object?` —
                // nothing else accepts it.
                if matches!(b, Ty::Dynamic) {
                    return true;
                }
                if matches!(a, Ty::Dynamic) {
                    return match b {
                        Ty::Named {
                            name,
                            args: ArgListId::EMPTY,
                            ..
                        } => name == object,
                        _ => false,
                    };
                }
                let (
                    Ty::Named {
                        name: a_head,
                        nullable: a_nullable,
                        ..
                    },
                    Ty::Named {
                        name: b_head,
                        nullable: b_nullable,
                        ..
                    },
                ) = (a, b)
                else {
                    return false;
                };
                if a_nullable && !b_nullable {
                    return false;
                }
                if a_head != b_head {
                    return reaches(a_head, b_head);
                }
                // Equal heads with argument lists on both sides: only a
                // concrete bare pair could still order, and steps 3-4
                // would have ordered it — what reaches here carries a
                // residual parameter (the recursive bound) and stays
                // unordered.
                false
            })
        };
        // The oracle's specificity order opens with provenance, not types
        // (`extension_member_resolver.dart:470-479`): a non-SDK extension is
        // unconditionally more specific than an SDK one, so a user-space
        // twin silently wins over the platform's own face (package:
        // collection's `IterableExtension` over dart:core's
        // `IterableExtensions`) before any subtype comparison runs — the
        // oracle never reports the pair ambiguous. A face file the graph
        // never registered answers non-SDK (the under-report direction:
        // the twin pair stays unordered). Only the synthetic face files
        // carry `dart:` library URIs — real source libraries are
        // `package:`/path URIs.
        let sdk_origin: Vec<bool> = applicable
            .iter()
            .map(|&(_, _, face_file)| {
                self.graph
                    .library_of_file(face_file)
                    .is_some_and(|lib| self.graph.uri(lib).starts_with("dart:"))
            })
            .collect();
        let mut specific: Vec<(&ExtensionFace, FileId, (String, String))> = Vec::new();
        let mut nullable: Vec<(&ExtensionFace, FileId, (String, String))> = Vec::new();
        for (index, &(face, on_ty, face_file)) in applicable.iter().enumerate() {
            let dominated = applicable.iter().enumerate().any(|(other, _)| {
                if other == index {
                    return false;
                }
                let (other_face, other_on_ty, other_file) = applicable[other];
                // Steps 1-2: the SDK-precedence order decides before the
                // subtype steps; same-origin pairs fall through to them.
                if sdk_origin[other] != sdk_origin[index] {
                    return !sdk_origin[other];
                }
                if on_subtype(other_on_ty, on_ty) {
                    if !on_subtype(on_ty, other_on_ty) {
                        // Steps 3-4: the instantiated `on` types order.
                        return true;
                    }
                    // Step 5 (`extension_member_resolver.dart:500-506`):
                    // mutually compatible `on` types order on the
                    // instantiate-to-bounds projections — the bounded face
                    // dominates its unbounded twin when the projections
                    // order one way and not the other. #2508: the
                    // snapshot's own `ParamFacts` bounds drive the
                    // projection where both `on` types map; a `None`
                    // verdict keeps the name-level projection and pair
                    // test verbatim.
                    // Each face's OWN bounds (`strict_face_params`) feed
                    // the projection — the snapshot's name-keyed registry
                    // would fold two faces' same-spelled parameters into
                    // one bound.
                    let strict = match (
                        self.ops,
                        self.strict_face_params(other_face, other_file),
                        self.strict_face_params(face, face_file),
                    ) {
                        (Some(ops), Some(sub_params), Some(sup_params)) => ops.try_bounds_subtype(
                            other_on_ty,
                            &sub_params,
                            on_ty,
                            &sup_params,
                            &self.boundary.args.borrow(),
                        ),
                        _ => None,
                    };
                    // Either world's `true` proves a direction; the
                    // negative needs both worlds (a structural `false` over
                    // an incompletely fed hierarchy is no fact).
                    let sub = bound_projected(other_face, other_file, other_on_ty);
                    let sup = bound_projected(face, face_file, on_ty);
                    let (strict_sub_le_sup, strict_sup_le_sub) =
                        strict.unwrap_or((false, false));
                    (strict_sub_le_sup || bounds_subtype(sub, sup))
                        && !(strict_sup_le_sub || bounds_subtype(sup, sub))
                } else {
                    false
                }
            });
            if dominated {
                continue;
            }
            let display = self.extension_display(face);
            if face.on_type_nullable {
                nullable.push((face, face_file, display));
            } else {
                specific.push((face, face_file, display));
            }
        }
        // On a nullable receiver a nullable `on` face is the safe match;
        // only when none applies does the unsafe non-nullable set answer.
        let (candidates, unsafe_match) = if receiver_nullable {
            if nullable.is_empty() {
                (specific, true)
            } else {
                (nullable, false)
            }
        } else if specific.is_empty() {
            (nullable, false)
        } else {
            (specific, false)
        };
        // A uniquely applied member is used (the official `usedElements.members`
        // face): the application resolves the access, so the member's
        // declaration is spared. Both the read and — when the write
        // accepted it — the setter spelling join; a spelling no face
        // declares never matches a finalize key, so over-inserting is
        // inert. The applied face also credits its contributing import
        // (`ImportsTracking.notifyExtensionUsed`, scope.dart:308-321): the
        // import that provided the extension is used, an own-library face
        // contributes nothing.
        // An unsafe match credits nothing: `applicableTo` rejects the
        // non-nullable `on` type against the nullable receiver (the access
        // reads a possibly-null receiver — the oracle's
        // `unchecked_use_of_nullable_value` row, emitted by the caller's
        // `UnsafeSingle` arm), so the member never enters `usedElements`
        // and the import stays uncredited — the oracle's `unused_element`/
        // `unused_import` still fire on both.
        // An ambiguous access resolves no member (the oracle leaves every
        // candidate unreferenced — `unused_element` still fires on each).
        if candidates.len() == 1 && !unsafe_match {
            let (face, face_file, _) = candidates[0];
            // The element key: the face's position in its declaring file's
            // `extension_faces()` pool (pointer identity IS element identity
            // within an analysis — the pool is frozen before the walk). A
            // face the index cannot see credits nothing rather than guessing
            // a slot.
            if let Some(face_index) = self
                .index
                .get(face_file)
                .and_then(|elements| {
                    elements
                        .extension_faces()
                        .iter()
                        .position(|entry| std::ptr::eq(entry, face))
                })
                .and_then(|index| u32::try_from(index).ok())
            {
                self.extension_member_used
                    .insert((face_file, face_index, member));
                if accept_setter {
                    self.extension_member_used
                        .insert((face_file, face_index, setter));
                }
            }
            self.credit_extension_import(face, face_file);
        }
        match candidates.len() {
            0 => ExtensionApp::None,
            1 if unsafe_match => ExtensionApp::UnsafeSingle,
            1 => ExtensionApp::Single,
            _ => ExtensionApp::Ambiguous(
                candidates
                    .into_iter()
                    .map(|(_, _, display)| display)
                    .collect(),
            ),
        }
    }

    /// Whether the receiver's interned type resolves at its head — the
    /// oracle's `InvalidType` test `extension_application` gates on. Only
    /// the top-level `Named` head decides: `Missing x` leaves the receiver
    /// an `InvalidType`, so `applicableTo` returns no `ExtensionResolution`
    /// and the access resolves nothing. A resolved head carrying an
    /// unresolvable name deeper inside is still a valid interface/record/
    /// function type, not an `InvalidType`: `List<InvalidType>` stays an
    /// `InterfaceType`, a `(Missing,)` record stays a `RecordType`, and a
    /// callee's unbound type-parameter spelling (the `A` in `seq2`'s
    /// declared `Parser<(A, B)>`) instantiates at the send — so the member
    /// face runs and the `on`-clause gate decides applicability. Probed
    /// against the oracle: `MyList<Missing>` applies `on MyList<C>` (the
    /// member resolves, no `undefined_*`), while a `(Missing,)` receiver
    /// reports `undefined_getter` through the argument gate's own verdict.
    pub(crate) fn extension_receiver_resolves(&self, receiver_ty: Ty) -> bool {
        match receiver_ty {
            Ty::Named { name, .. } => {
                self.type_resolution(name).0 == TypeResolution::Type
                    // A head the diagnosing scope cannot name but exactly
                    // one graph library declares is still a valid static
                    // type (it arrived through a resolved member
                    // signature) — extensions imported here apply to it
                    // (`node.parentElement` on the unimported `XmlNode`).
                    || self.unique_declared_elsewhere(name, self.lib).is_some()
            }
            _ => true,
        }
    }

    /// The `accessibleExtensions` list `extension_application` probes
    /// (`LibraryFragmentScope.accessibleExtensions`, scope.dart:473-488):
    /// every extension face this library declares (all unit files) plus the
    /// named extension faces each import edge's target exports under its
    /// combinators — prefixed imports' faces included (a prefix scope still
    /// contributes its extensions to implicit application), deferred edges
    /// too (the prefix scope adds a deferred library's elements the same).
    /// An unnamed extension never reaches the import scope — export entries
    /// key on names — but an own-library unnamed face applies all the same.
    /// Built once per pass; the probe list is small and the member-miss
    /// path re-reads it per access.
    pub(crate) fn accessible_extension_faces(&mut self) -> &[(&'a ExtensionFace, FileId)] {
        if self.accessible_extensions.is_none() {
            let mut faces: Vec<(&'a ExtensionFace, FileId)> = Vec::new();
            // The oracle's list is a SET of elements
            // (`LibraryFragmentScope.accessibleExtensions`,
            // scope.dart:482-487): a face reachable through several routes
            // — the own unit files AND one or more import edges — enters
            // the application pool once. Pointer identity is element
            // identity here: each face lives once in its declaring file's
            // `extension_faces` pool.
            let mut seen: FxHashSet<*const ExtensionFace> = FxHashSet::default();
            let mut push = |faces: &mut Vec<(&'a ExtensionFace, FileId)>,
                            face: &'a ExtensionFace,
                            file: FileId| {
                if seen.insert(face as *const ExtensionFace) {
                    faces.push((face, file));
                }
            };
            // The diagnosed unit always contributes its own faces first —
            // `unit_files` is the graph's registration face, which a caller
            // that never folds a part tree (the test rigs' `declare`-only
            // shape) leaves empty; the file under diagnosis is a unit of
            // `self.lib` by construction.
            if let Some(elements) = self.index.get(self.file) {
                for face in elements.extension_faces() {
                    push(&mut faces, face, self.file);
                }
            }
            for &unit in self.graph.unit_files(self.lib) {
                if unit == self.file {
                    continue;
                }
                if let Some(elements) = self.index.get(unit) {
                    for face in elements.extension_faces() {
                        push(&mut faces, face, unit);
                    }
                }
            }
            for edge in self.graph.import_edges(self.lib) {
                let Some(target) = edge.target else { continue };
                // The pool's face order reaches the ambiguity message's
                // candidate enumeration (`emit_extension_ambiguity` lists
                // the members in pool order), so an import target's
                // export-scope iteration must not leak container order
                // into the output. The sort key is the name's BYTES, not
                // its intern id: ids are allocation-ordered and a parallel
                // intern race can drift them across runs, while the byte
                // order is content-addressed and run-stable (the pool is a
                // set — order carries no resolution semantics).
                let strings = self.strings;
                let mut scope: Vec<_> = self.graph.export_scope(target).collect();
                scope.sort_by_key(|&(name, _)| strings.resolve(name));
                for (name, entry) in scope {
                    if !edge.combinators.allows(name) {
                        continue;
                    }
                    let Some(elements) = self.index.get(entry.file) else {
                        continue;
                    };
                    for face in elements
                        .extension_faces()
                        .iter()
                        .filter(|face| face.name == Some(name))
                    {
                        push(&mut faces, face, entry.file);
                    }
                }
            }
            self.accessible_extensions = Some(faces);
        }
        self.accessible_extensions.as_deref().unwrap_or(&[])
    }

    /// The `notifyExtensionUsed` credit of one applied face: a face
    /// declared in another library marks the spelling under which it
    /// exports — the import-site `unused_import` face spares every import
    /// that provides the name (the oracle credits the element to each
    /// import whose export scope carries it; the name-level model credits
    /// the exported spelling). An own-library face contributes nothing —
    /// its name resolved locally, never through an import.
    pub(crate) fn credit_extension_import(&mut self, face: &ExtensionFace, face_file: FileId) {
        if face_file == self.file || self.graph.library_of_file(face_file) == Some(self.lib) {
            return;
        }
        if let Some(name) = face.name {
            self.import_used.insert(name);
        }
    }

    /// The display pair of one applicable face: the element shape
    /// (`extension E on C`, the `ExtensionElement` toString — the official
    /// two-variant argument, `ambiguous_extension_member_access_test.dart:54`)
    /// and the resolver's inline shape (`extension 'E'` / `unnamed extension
    /// on 'C'`, `extension_member_resolver.dart:119-124`).
    pub(crate) fn extension_display(&self, face: &ExtensionFace) -> (String, String) {
        let name = face.name.map(|n| self.strings.resolve(n).to_string());
        let on_type = self
            .strings
            .resolve(face.on_type.expect("an applicable face has an on type"))
            .to_string();
        let element = match &name {
            Some(name) => format!("extension {name} on {on_type}"),
            None => format!("extension on {on_type}"),
        };
        let inline = match name {
            Some(name) => format!("extension '{name}'"),
            None => format!("unnamed extension on '{on_type}'"),
        };
        (element, inline)
    }

    /// Emits the `ambiguous_extension_member_access` verdict for the
    /// applicable candidates: the two-variant message for exactly two, the
    /// joined-list variant for more (`diagnostic.g.dart:338-348` for
    /// three-or-more, :363-374 for two), anchored at the access. The list
    /// renders per `commaSeparatedWithAnd`
    /// (`utilities/extensions/string.dart:41-104`): `a and b`, then
    /// `a, b, and c` from three on.
    pub(crate) fn emit_extension_ambiguity(
        &mut self,
        member: StringId,
        range: SourceRange,
        candidates: Vec<(String, String)>,
    ) {
        let name_text = self.strings.resolve(member).to_string();
        let (code, a1, a2) = if let [first, second] = candidates.as_slice() {
            (
                SemanticCode::AmbiguousExtensionMemberTwo,
                first.0.clone(),
                second.0.clone(),
            )
        } else {
            let inlines: Vec<String> = candidates.into_iter().map(|(_, inline)| inline).collect();
            let list = match inlines.len() {
                0 | 1 => inlines.into_iter().next().unwrap_or_default(),
                2 => {
                    let mut it = inlines.into_iter();
                    format!(
                        "{} and {}",
                        it.next().unwrap_or_default(),
                        it.next().unwrap_or_default()
                    )
                }
                _ => {
                    let last = inlines.last().expect("non-empty above").clone();
                    let head = &inlines[..inlines.len() - 1];
                    format!("{}, and {}", head.join(", "), last)
                }
            };
            (
                SemanticCode::AmbiguousExtensionMemberThreeOrMore,
                list,
                String::new(),
            )
        };
        self.emit3(code, range, &name_text, &a1, &a2);
    }

    /// Whether the plain member walk finds `member` on `container`.
    pub(crate) fn member_found(
        &self,
        container: StringId,
        member: StringId,
        super_walk: bool,
    ) -> bool {
        let walk = if super_walk {
            self.walk_super(container, member)
        } else {
            self.walk_members(container, member)
        };
        matches!(walk, Walk::Found(_))
    }

    pub(crate) fn container_kind_of(&self, container: StringId) -> Option<ContainerKind> {
        resolve_container(
            self.graph,
            self.namespaces,
            self.index,
            self.lib,
            self.file,
            container,
        )
        .map(|(c, _)| c.kind)
    }
}

/// Pins for the `UndefinedGuard` face scoping (#2186 S1): the observed
/// oracle (analyzer 8.1.1, the 3.13.1 SDK of record) consults
/// `shouldIgnoreUndefined` only on the unqualified/prefixed *name* faces —
/// `undefined_function` (`_reportUndefinedFunction`,
/// method_invocation_resolver.dart:403) and `undefined_identifier`
/// (`shouldIgnoreUndefinedIdentifier`, simple_identifier_resolver.dart:222)
/// plus the named-type family (`shouldIgnoreUndefinedNamedType`,
/// named_type_resolver.dart:326) and `undefined_prefixed_name`
/// (property_element_resolver.dart:891). The member faces — the
/// `undefined_method`/`_getter`/`_setter` trio — never consult it
/// (`_reportUndefinedMethodOrNew` is guard-free, same file :424-431), and
/// `undefined_identifier_await` fires before the guard in its else-if
/// chain (simple_identifier_resolver.dart:217-221).
#[cfg(test)]
mod tests {
    use super::*;
    use crate::collector::collect;
    use dartq_language::{FeatureSet, LanguageVersion};
    use dartq_resolution::{CombinatorClause, Combinators, LibraryGraphBuilder};
    use dartq_scanner::ScanConfig;
    use dartq_source::FileTable;
    use std::path::Path;

    /// Diagnoses `source` as the sole file of a library carrying one
    /// broken import edge (`target: None`) built by `make_edge`.
    fn report_with_edge(
        source: &str,
        make_edge: impl FnOnce(&StringTable) -> ImportEdge,
    ) -> FileReport {
        let strings = StringTable::new();
        let mut files = FileTable::new();
        let file = files.intern(Path::new("main.dart"));
        let scanned = dartq_scanner::scan(
            source,
            ScanConfig {
                enable_triple_shift: true,
                enable_augmentations: true,
            },
            file,
        );
        assert!(
            scanned.diagnostics.is_empty(),
            "scanner diagnostics: {source:?}"
        );
        let set = FeatureSet::for_language_version(LanguageVersion::new(3, 15));
        let features = ParserFeatures::from_feature_set(set);
        let elements = collect(source, &scanned.tokens, features, &strings);
        let mut builder = LibraryGraphBuilder::new(&strings);
        let lib = builder.intern("package:t/main.dart");
        let mut index = ElementIndex::default();
        index.insert(file, elements.clone());
        for (name, decl) in elements.declarations() {
            if decl.kind == MicroKind::TypeParam {
                continue;
            }
            builder.declare(lib, name, file);
        }
        builder.import(lib, make_edge(&strings));
        let graph = builder.finish();
        let namespaces = Namespaces::build(&graph);
        let fold = fold_unit(
            source,
            &scanned.tokens,
            features,
            &strings,
            file,
            lib,
            &graph,
            &namespaces,
            &index,
            StrictModes::default(),
            &LibraryClassModifiers::default(),
            set.is_enabled(dartq_language::Feature::InferenceUpdate2),
        );
        let mut report = fold.report;
        report.diagnostics = merge_source_ordered(fold.parse_diagnostics, report.diagnostics);
        report
    }

    /// A broken import showing `name` (the S1 trigger shape).
    fn broken_show(name: &'static str) -> impl FnOnce(&StringTable) -> ImportEdge {
        move |strings| ImportEdge {
            target: None,
            prefix: None,
            deferred: false,
            combinators: Combinators::new(
                [CombinatorClause::Show(
                    vec![strings.intern(name)].into_boxed_slice(),
                )],
                strings,
            ),
        }
    }

    /// An unfiltered broken import (the guard's no-show arm never fires on
    /// bare names — only the prefixed arm can).
    fn broken_bare() -> impl FnOnce(&StringTable) -> ImportEdge {
        |_| ImportEdge {
            target: None,
            prefix: None,
            deferred: false,
            combinators: Combinators::unfiltered(),
        }
    }

    /// Diagnoses `source` as the sole file of a library importing one SDK
    /// face library: `sdk_lib` is the import URI (`dart:async`) and
    /// `sdk_source`'s declarations are owned by a registered-but-empty unit
    /// — the namespace resolves the names to a declaration whose file
    /// carries no container model, the exact posture of the real `dart:`
    /// faces (the frozen member tables own their members, and the
    /// supertype hops into them classify dead).
    fn report_sdk_face(source: &str, sdk_lib: &str, sdk_source: &str) -> FileReport {
        let strings = StringTable::new();
        let mut files = FileTable::new();
        let file = files.intern(Path::new("main.dart"));
        let sdk_file = files.intern(Path::new("sdk_face.dart"));
        let scan_cfg = ScanConfig {
            enable_triple_shift: true,
            enable_augmentations: true,
        };
        let scanned = dartq_scanner::scan(source, scan_cfg, file);
        assert!(
            scanned.diagnostics.is_empty(),
            "scanner diagnostics: {source:?}"
        );
        let set = FeatureSet::for_language_version(LanguageVersion::new(3, 15));
        let features = ParserFeatures::from_feature_set(set);
        let elements = collect(source, &scanned.tokens, features, &strings);
        let sdk_scanned = dartq_scanner::scan(sdk_source, scan_cfg, sdk_file);
        let sdk_elements = collect(sdk_source, &sdk_scanned.tokens, features, &strings);
        let mut builder = LibraryGraphBuilder::new(&strings);
        let lib = builder.intern("package:t/main.dart");
        let mut index = ElementIndex::default();
        index.insert(file, elements.clone());
        for (name, decl) in elements.declarations() {
            if decl.kind == MicroKind::TypeParam {
                continue;
            }
            builder.declare(lib, name, file);
        }
        let lib_sdk = builder.intern(sdk_lib);
        builder.unit(lib_sdk, sdk_file);
        for (name, decl) in sdk_elements.declarations() {
            if decl.kind == MicroKind::TypeParam {
                continue;
            }
            builder.declare(lib_sdk, name, sdk_file);
        }
        // The SDK unit registers but carries no container model: the
        // names resolve, the member faces answer from the frozen tables.
        let empty_scanned = dartq_scanner::scan("", scan_cfg, sdk_file);
        index.insert(
            sdk_file,
            collect("", &empty_scanned.tokens, features, &strings),
        );
        builder.import(
            lib,
            ImportEdge {
                target: Some(lib_sdk),
                prefix: None,
                deferred: false,
                combinators: Combinators::unfiltered(),
            },
        );
        let graph = builder.finish();
        let namespaces = Namespaces::build(&graph);
        let fold = fold_unit(
            source,
            &scanned.tokens,
            features,
            &strings,
            file,
            lib,
            &graph,
            &namespaces,
            &index,
            StrictModes::default(),
            &LibraryClassModifiers::default(),
            set.is_enabled(dartq_language::Feature::InferenceUpdate2),
        );
        let mut report = fold.report;
        report.diagnostics = merge_source_ordered(fold.parse_diagnostics, report.diagnostics);
        report
    }

    fn code_names(report: &FileReport) -> Vec<&'static str> {
        report
            .diagnostics
            .iter()
            .map(|d| d.code.name)
            .collect::<Vec<_>>()
    }

    /// S1 fixture (a): a `show`-listed name of a broken import invoked as a
    /// constructor inside a class body is `undefined_method` — the oracle's
    /// `_reportUndefinedMethodOrNew` consults no suppression, so the guard
    /// must not silence it (the ~430 oracle-only rows' root cause).
    #[test]
    fn shown_name_invoked_in_class_reports_undefined_method() {
        let report = report_with_edge("class C { void m() { Foo(); } }", broken_show("Foo"));
        assert_eq!(code_names(&report), ["undefined_method"]);
        let row = &report.diagnostics[0];
        assert_eq!(
            &"class C { void m() { Foo(); } }"[row.range.offset as usize..row.range.end() as usize],
            "Foo"
        );
    }

    /// S1 fixture (b): the same `show`-listed name invoked at top level
    /// stays silent — `undefined_function` is a guarded face
    /// (`_reportUndefinedFunction` consults `shouldIgnoreUndefined`). The
    /// miss still lands in the names table.
    #[test]
    fn shown_name_invoked_at_top_level_stays_suppressed() {
        let report = report_with_edge("void f() { Foo(); }", broken_show("Foo"));
        assert!(report.diagnostics.is_empty(), "{:?}", code_names(&report));
        assert_eq!(report.names.len(), 1);
        assert_eq!(report.names[0].use_kind, NameUse::Invoke);
    }

    /// The guarded read face keeps its suppression: a `show`-listed name
    /// read as a bare identifier stays silent (`undefined_identifier`
    /// consults `shouldIgnoreUndefinedIdentifier`).
    #[test]
    fn shown_name_bare_read_stays_suppressed() {
        let report = report_with_edge("void f() { gone; }", broken_show("gone"));
        assert!(report.diagnostics.is_empty(), "{:?}", code_names(&report));
        assert_eq!(report.names.len(), 1);
        assert_eq!(report.names[0].use_kind, NameUse::Read);
    }

    /// S1 fixture (c): an unfiltered broken import never suppresses a bare
    /// miss — `undefined_method` reports for the in-class invocation, the
    /// same verdict the show-listed shape now reaches.
    #[test]
    fn unfiltered_broken_import_invoke_in_class_reports() {
        let report = report_with_edge("class C { void m() { Foo(); } }", broken_bare());
        assert_eq!(code_names(&report), ["undefined_method"]);
    }

    /// S1 fixture (d): a `show`-listed name read in a const context (the
    /// `const Foo.bar()`-class prefixed-ctor head shape) reports
    /// `undefined_identifier` through the guarded read face — the guard
    /// still applies, so this stays silent.
    #[test]
    fn shown_name_prefixed_ctor_head_stays_suppressed() {
        let report = report_with_edge("const x = Foo.bar();", broken_show("Foo"));
        assert!(
            !code_names(&report).contains(&"undefined_method"),
            "{:?}",
            code_names(&report)
        );
        // The `Foo` head of `Foo.bar` resolves through the guarded
        // identifier face: show-listed on a broken import stays silent.
        assert!(
            !code_names(&report).contains(&"undefined_identifier"),
            "{:?}",
            code_names(&report)
        );
    }

    /// The `.new` selector on an unresolved receiver: the member family on
    /// `InvalidType` is guard-free like every member face — a `show`-listed
    /// receiver name suppresses its own `undefined_identifier`, but the
    /// `.new` member miss still reports.
    #[test]
    fn new_selector_on_shown_broken_receiver_reports() {
        let report = report_with_edge("void f() { gone.new(); }", broken_show("gone"));
        assert_eq!(code_names(&report), ["undefined_method"]);
    }

    /// Diagnoses `source` as a single-file library with a clean unfiltered
    /// import edge to a second unit `a.dart` (the two-unit counterpart of
    /// [`report_with_edge`] — the drift repro's cross-file member faces).
    fn report_two(source_a: &str, source: &str) -> FileReport {
        report_units(&[("a.dart", source_a)], "main.dart", source, false)
    }

    /// [`report_two`] with the import edge `show`-filtered to one name: the
    /// unit's other declarations stay out of the diagnosing library's
    /// namespace while their types still flow through member signatures
    /// (the xml corpus's `XmlNodeList` receiver shape).
    fn report_show(source_a: &str, shown: &str, source: &str) -> FileReport {
        report_units_edge(
            &[("a.dart", source_a)],
            "main.dart",
            source,
            false,
            Some(shown),
        )
    }

    /// Diagnoses `source` as the defining unit of a library whose
    /// declarations live partly in `part` files (the drift `part of`
    /// receiver shape): `parts` is `(path, source)` pairs joined to the
    /// main unit's library through `part of`.
    fn report_with_parts(parts: &[(&str, &str)], source: &str) -> FileReport {
        report_units(parts, "main.dart", source, true)
    }

    /// The shared two-shape rig behind [`report_two`] and
    /// [`report_with_parts`]: `units` are foreign units (imported) when
    /// `as_parts` is false, or same-library parts when true — the part
    /// files declare into the main library's namespace either way, so the
    /// member walk reaches them identically (upstream `part of` units
    /// share the enclosing library's scope).
    fn report_units(
        units: &[(&str, &str)],
        main_path: &str,
        source: &str,
        as_parts: bool,
    ) -> FileReport {
        report_units_edge(units, main_path, source, as_parts, None)
    }

    /// The rig core behind [`report_units`]: `show` filters every import
    /// edge to one visible name (the xml corpus's `XmlNodeList` receiver
    /// shape — the unit's other declarations stay out of the diagnosing
    /// library's namespace while their types still flow through member
    /// signatures).
    fn report_units_edge(
        units: &[(&str, &str)],
        main_path: &str,
        source: &str,
        as_parts: bool,
        show: Option<&str>,
    ) -> FileReport {
        let strings = StringTable::new();
        let mut files = FileTable::new();
        let file_m = files.intern(Path::new(main_path));
        let unit_files: Vec<FileId> = units
            .iter()
            .map(|(path, _)| files.intern(Path::new(path)))
            .collect();
        let set = FeatureSet::for_language_version(LanguageVersion::new(3, 15));
        let features = ParserFeatures::from_feature_set(set);
        let scan_cfg = ScanConfig {
            enable_triple_shift: true,
            enable_augmentations: true,
        };
        let scanned_m = dartq_scanner::scan(source, scan_cfg, file_m);
        assert!(
            scanned_m.diagnostics.is_empty(),
            "scanner diagnostics: {source:?}"
        );
        let scanned_units: Vec<_> = units
            .iter()
            .zip(&unit_files)
            .map(|((_, unit_source), unit_file)| {
                let scanned = dartq_scanner::scan(unit_source, scan_cfg, *unit_file);
                assert!(
                    scanned.diagnostics.is_empty(),
                    "scanner diagnostics: {unit_source:?}"
                );
                scanned
            })
            .collect();
        let elements_m = collect(source, &scanned_m.tokens, features, &strings);
        let elements_units: Vec<_> = units
            .iter()
            .zip(&scanned_units)
            .map(|((_, unit_source), scanned)| {
                collect(unit_source, &scanned.tokens, features, &strings)
            })
            .collect();
        let mut builder = LibraryGraphBuilder::new(&strings);
        let lib_m = builder.intern("package:t/main.dart");
        let mut index = ElementIndex::default();
        index.insert(file_m, elements_m.clone());
        for (name, decl) in elements_m.declarations() {
            if decl.kind == MicroKind::TypeParam {
                continue;
            }
            builder.declare(lib_m, name, file_m);
        }
        for (i, elements_u) in elements_units.iter().enumerate() {
            let unit_file = unit_files[i];
            index.insert(unit_file, elements_u.clone());
            if as_parts {
                // A part file folds into the main library's own namespace
                // (upstream `part of` shares the enclosing library scope).
                builder.unit(lib_m, unit_file);
                for (name, decl) in elements_u.declarations() {
                    if decl.kind == MicroKind::TypeParam {
                        continue;
                    }
                    builder.declare(lib_m, name, unit_file);
                }
            } else {
                let lib_u = builder.intern(&format!("package:t/unit{i}.dart"));
                builder.unit(lib_u, unit_file);
                for (name, decl) in elements_u.declarations() {
                    if decl.kind == MicroKind::TypeParam {
                        continue;
                    }
                    builder.declare(lib_u, name, unit_file);
                }
                let combinators = show.map(|shown| {
                    Combinators::new(
                        [CombinatorClause::Show(
                            vec![strings.intern(shown)].into_boxed_slice(),
                        )],
                        &strings,
                    )
                });
                builder.import(
                    lib_m,
                    ImportEdge {
                        target: Some(lib_u),
                        prefix: None,
                        deferred: false,
                        combinators: combinators.unwrap_or_else(Combinators::unfiltered),
                    },
                );
            }
        }
        let graph = builder.finish();
        let namespaces = Namespaces::build(&graph);
        let mut fold = fold_unit(
            source,
            &scanned_m.tokens,
            features,
            &strings,
            file_m,
            lib_m,
            &graph,
            &namespaces,
            &index,
            StrictModes::default(),
            &LibraryClassModifiers::default(),
            set.is_enabled(dartq_language::Feature::InferenceUpdate2),
        );
        fold.report.diagnostics =
            merge_source_ordered(fold.parse_diagnostics, fold.report.diagnostics);
        fold.report
    }

    /// #2190 shape 1 — the drift3_preview `abstract interface class
    /// DriftSession` face: its abstract getter and method members are own
    /// members of a same-library declaration, resolved through a declared
    /// parameter type. The oracle's interface walk resolves both silently.
    #[test]
    fn drift_wrapper_interface_members_resolve() {
        let report = report_two(
            "abstract interface class DriftSession {\n\
             \x20 Future<DriftSession> exec(int i);\n\
             \x20 int? get transactionParent;\n\
             }\n\
             class Future<T> {}",
            "void f(DriftSession s) { s.transactionParent; s.exec(1); }",
        );
        assert_eq!(
            code_names(&report),
            Vec::<&'static str>::new(),
            "{:?}",
            code_names(&report)
        );
    }

    /// #2190 shape 2 — the `_CollectByDialect` record-type field whose
    /// annotation runs onto the next line (`final List<({…})>\n
    /// collectedStatements = [];`): the field must land in the member map
    /// under its written name regardless of the annotation split.
    #[test]
    fn drift_wrapper_record_field_multiline_resolves() {
        let report = report_two(
            "class B {}\n\
             final class C extends B {\n\
             \x20 final List<({int x, String y})>\n\
             \x20 collectedStatements = [];\n\
             }\n\
             class List<T> {}",
            "void f(C c) { c.collectedStatements; }",
        );
        assert_eq!(
            code_names(&report),
            Vec::<&'static str>::new(),
            "{:?}",
            code_names(&report)
        );
    }

    /// #2190 shape 3 — the `part of` plain-class field
    /// (`DatabaseConnection.executor`): a trivial field declared in a part
    /// file of the diagnosing library resolves through the member walk.
    /// Its constructor initializer carries a `switch` expression (the
    /// corpus shape) — included to keep the population honest.
    #[test]
    fn drift_wrapper_part_file_field_resolves() {
        let report = report_with_parts(
            &[(
                "conn.dart",
                "part of 'main.dart';\n\
                 class QueryExecutor {}\n\
                 class StreamQueryStore {}\n\
                 class DatabaseConnection implements QueryExecutor {\n\
                 \x20 final QueryExecutor executor;\n\
                 \x20 final StreamQueryStore streamQueries;\n\
                 \x20 DatabaseConnection(this.executor, {StreamQueryStore? streamQueries})\n\
                 \x20   : streamQueries = streamQueries ?? switch (executor) {\n\
                 \x20       DatabaseConnection() => executor.streamQueries,\n\
                 \x20       _ => StreamQueryStore(),\n\
                 \x20     };\n\
                 }",
            )],
            "void f(DatabaseConnection db) { db.executor; }",
        );
        assert_eq!(
            code_names(&report),
            Vec::<&'static str>::new(),
            "{:?}",
            code_names(&report)
        );
    }

    /// #2190 discriminator — every corpus receiver is bound from
    /// `await expr` (`final s = await currentSession()`). The var-inferred
    /// declarator binds the initializer's expression type; the oracle's
    /// `await` flattens `Future<T>` to `T` before the member lookup runs
    /// (upstream `typeSystem.flatten`, type_system.dart:425-471 —
    /// `Future<S>`/`FutureOr<S>` → `S`, nullability preserved).
    ///
    /// Measured on HEAD: dartq emits `undefined_getter`/`undefined_method`
    /// **naming `Future`** — the receiver binds `Future<DriftSession>`
    /// because `end_await_expression` never folds the operand row's `ty`
    /// and `initializer_static_ty`/`bind_var_type` record the callee's raw
    /// return type. This is the await-fold loss (evidence-2186 §S4), NOT a
    /// member-face miss — all three member shapes verify clean below. The
    /// fix site is the inference/flow side (`listener.rs
    /// end_await_expression` folding `rows.last().ty`, or
    /// `engine_core.rs::initializer_static_ty` unwrapping `Future<T>` when
    /// the row sits under an await span) — outside this lane's file scope.
    /// `#[ignore]`d so the oracle-truth pin rides in the tree and flips to
    /// live the moment the flatten lands.
    #[test]
    fn drift_wrapper_await_receiver_discriminator() {
        let report = report_two(
            "abstract interface class DriftSession {\n\
             \x20 int? get transactionParent;\n\
             \x20 Future<int> exec(int i);\n\
             }\n\
             class Future<T> {}",
            "Future<DriftSession> g() => g();\n\
             void f() async {\n\
             \x20 final s = await g();\n\
             \x20 s.transactionParent;\n\
             \x20 s.exec(1);\n\
             }",
        );
        assert_eq!(
            code_names(&report),
            Vec::<&'static str>::new(),
            "{:?}",
            report
                .diagnostics
                .iter()
                .map(|d| d.message.to_string())
                .collect::<Vec<_>>()
        );
    }

    /// #2190 control — a genuinely missing member on the same wrapper face
    /// still reports `undefined_getter` (the under-report bias guard: the
    /// fix must not silence true misses).
    #[test]
    fn drift_wrapper_missing_member_still_reports() {
        let report = report_two(
            "abstract interface class DriftSession {\n\
             \x20 int? get transactionParent;\n\
             }",
            "void f(DriftSession s) { s.onMissing; }",
        );
        assert_eq!(code_names(&report), ["undefined_getter"]);
        assert_eq!(
            report.diagnostics[0].message,
            "The getter 'onMissing' isn't defined for the type 'DriftSession'."
        );
    }

    /// The receiver anchor the diagnosing scope cannot name (the unit is
    /// `show`-filtered to the wrapper, so `XmlNodeList` itself is not in
    /// the namespace — the xml corpus's `w.list.whereType()` shape) still
    /// climbs: exactly one graph library declares the type, so the frozen
    /// verdict re-anchors there, the dead `extends` hop (`Missing` is
    /// undeclared anywhere) contributes nothing, and the miss reports
    /// naming the resolved class. Without the re-anchor the climb answers
    /// `Unknown` and stays silent while the oracle reports.
    ///
    /// Verified 1:1 against the oracle on the real engine
    /// (/tmp/probe2293/showprobe — `import 'a.dart' show Wrapper` +
    /// `w.list.whereType()` → both sides `undefined_method` naming
    /// `XmlNodeList`). `#[ignore]`d here: the two-unit rig resolves the
    /// foreign getter's return type one grade short of the CLI's graph
    /// wiring, so the receiver anchor never reaches the re-anchor arm —
    /// the same rig gap the await-fold discriminator documents. Flips live
    /// when the rig wires the foreign-unit boundary pool the CLI builds.
    #[test]
    #[ignore = "rig gap: foreign-unit receiver ty projection (see doc)"]
    fn unimported_declared_receiver_member_reports() {
        let report = report_show(
            "class XmlNodeList extends Missing {}\n\
             class Wrapper {\n\
             \x20 XmlNodeList get list => XmlNodeList();\n\
             }",
            "Wrapper",
            "import 'a.dart' show Wrapper;\n\
             void f(Wrapper w) {\n\
             \x20 w.list.whereType();\n\
             }",
        );
        assert_eq!(code_names(&report), ["undefined_method"]);
        assert_eq!(
            report.diagnostics[0].message,
            "The method 'whereType' isn't defined for the type 'XmlNodeList'."
        );
    }

    /// An extension on a bare SDK type (`Stream<T>`) reads the frozen
    /// member table the element index cannot carry: `this` inside the body
    /// IS the `on` type, `map` is one of `Stream`'s members
    /// (`SDK_FACES_EXTRA`, upstream Dart SDK source, sdk/lib/async/
    /// stream.dart), so the implicit application resolves and stays
    /// silent.
    #[test]
    fn stream_extension_member_resolves_through_frozen_face() {
        let report = report_sdk_face(
            "import 'dart:async';\n\
             extension Tap<T> on Stream<T> {\n\
             \x20 Stream<T> tap(void Function(T) f) {\n\
             \x20   return this.map(f);\n\
             \x20 }\n\
             }",
            "dart:async",
            "class Stream<T> {}",
        );
        assert_eq!(
            code_names(&report),
            Vec::<&'static str>::new(),
            "{:?}",
            report
                .diagnostics
                .iter()
                .map(|d| d.message.to_string())
                .collect::<Vec<_>>()
        );
    }

    /// The miss control: the same extension shape accessing a member
    /// `Stream` genuinely lacks reports `undefined_method` naming the
    /// `on` type — the frozen verdict's `Absent` answer, never the
    /// silent `Unknown`.
    #[test]
    fn stream_extension_member_miss_reports_on_type() {
        let report = report_sdk_face(
            "import 'dart:async';\n\
             extension Tap<T> on Stream<T> {\n\
             \x20 Stream<T> tap() {\n\
             \x20   return this.nope();\n\
             \x20 }\n\
             }",
            "dart:async",
            "class Stream<T> {}",
        );
        assert_eq!(code_names(&report), ["undefined_method"]);
        assert_eq!(
            report.diagnostics[0].message,
            "The method 'nope' isn't defined for the type 'Stream'."
        );
    }

    /// The dead-anchored sibling `if`'s promotion consumption (#2293
    /// rider): inside a region the flow marked dead (the always-true
    /// `if (s == null) return null` — `s` narrowed to `Null` by the
    /// earlier `s is Src` exit), the `if (file != null)` promotion still
    /// holds for the guarded read — the oracle analyzes dead code with
    /// the same flow state (probe: dead-region `g(z)` with `z` promoted
    /// reports no argument row) and the staged argument check consumes
    /// the promotion, never reporting `FS?` into the `FS` formal.
    ///
    /// Ignored pending a flow-side fix: the promotion fact for a
    /// dead-region read is suppressed by the flow layer's reachability
    /// gate (`dartq-flow` `interp_expr.rs` — the `PromotedRead`/`ReadType`
    /// emits both require `self.reachable()`), so the staged check finds
    /// no row and the parse-time verdict stands. The consumption side is
    /// proven correct on the live path (the same fixture's final
    /// `UR.f(file)` read stages `FS` and cleans). Upstream runs the same
    /// flow state in dead regions (dead-region promotions materialize;
    /// only the `dead_code` row marks the region), so the fix is dropping
    /// the reachability gate from those two fact emits — `dartq-flow` is
    /// #2288/#2292's lane; revive this pin when that lands.
    #[ignore = "flow gate: dead-region PromotedRead suppressed (dartq-flow interp_expr.rs reachable gate, #2288/#2292)"]
    #[test]
    fn dead_region_null_check_promotion_cleans_argument() {
        let report = report_units(
            &[],
            "/p.dart",
            "class FS {}\n\
             class UR { UR.f(FS f); UR.e(dynamic s); }\n\
             class Src {}\n\
             class SrcFactory { Src? forUri2(dynamic u) => null; }\n\
             class Map<K, V> { V? operator [](K key) => null; }\n\
             Map<dynamic, FS?> m1 = {};\n\
             Map<dynamic, FS?> m2 = {};\n\
             UR? f(SrcFactory fac, dynamic uri) {\n\
             \x20 var s = fac.forUri2(uri);\n\
             \x20 if (s is Src) {\n\
             \x20   return UR.e(s);\n\
             \x20 }\n\
             \x20 FS? file = m1[uri];\n\
             \x20 if (file == null) {\n\
             \x20   if (s == null) {\n\
             \x20     return null;\n\
             \x20   }\n\
             \x20   file = m2[uri];\n\
             \x20   if (file != null) {\n\
             \x20     return UR.f(file);\n\
             \x20   }\n\
             \x20 }\n\
             \x20 return UR.f(file);\n\
             }",
            false,
        );
        // The `dead_code` row matches the oracle (the always-true sibling
        // if's first dead statement); the argument row must clean.
        assert_eq!(
            code_names(&report),
            ["dead_code"],
            "{:?}",
            report
                .diagnostics
                .iter()
                .map(|d| d.message.to_string())
                .collect::<Vec<_>>()
        );
    }

    /// The prefixed-head unresolved-reference code (#2289 rider): the
    /// head of a value-position `missing.name` selector is a plain bare
    /// miss — the oracle reports `undefined_identifier` at the head's own
    /// span beside the constant-eval row (`undefined_class` is the
    /// TYPE-position code and never fires here), and the row must survive
    /// to the report, not drop at the merge. The `+`-operand head
    /// (`missing + 1`) rides the same face (#2306): the operator's
    /// operand reports the identifier alone, the operator lookup itself
    /// never cascades over the invalid operand.
    #[test]
    fn prefixed_head_value_miss_reports_identifier() {
        let report = report_units(
            &[],
            "/p.dart",
            "class Object {}\n\
             class C {\n\
             \x20 const C(Object? a);\n\
             }\n\
             const a = C(missing);\n\
             const b = C(missing.name);\n\
             const c = C(missing + 1);\n\
             const d = C(!missing);\n\
             const e = C((missing));",
            false,
        );
        let mut heads: Vec<u32> = report
            .diagnostics
            .iter()
            .filter(|d| d.code.name == "undefined_identifier")
            .map(|d| d.range.offset)
            .collect();
        heads.sort_unstable();
        assert_eq!(heads, [62, 84, 111, 138, 161]);
    }

    /// #2306: the operator's undefined operand reports itself at the head
    /// — `missing + 1` and `missing[0]` carry the same
    /// `undefined_identifier` face a bare read carries (upstream Dart SDK
    /// source, `pkg/analyzer/lib/src/dart/resolver/simple_identifier_resolver.dart`,
    /// the operand's own `simpleIdentifier` visit), and the operator
    /// lookup never cascades a second row over the invalid operand (the
    /// operand's dynamic face swallows it — probed against the oracle,
    /// both shapes report the identifier alone). `final` heads keep the
    /// const-initializer face out of the pin's scope.
    #[test]
    fn operator_head_miss_reports_identifier_without_cascade() {
        let report = report_units(
            &[],
            "/p.dart",
            "class Object {}\n\
             final a = missing + 1;\n\
             final b = missing[0];",
            false,
        );
        assert_eq!(
            code_names(&report),
            ["undefined_identifier", "undefined_identifier"],
            "{:?}",
            report
                .diagnostics
                .iter()
                .map(|d| d.message.to_string())
                .collect::<Vec<_>>()
        );
        let mut heads: Vec<u32> = report
            .diagnostics
            .iter()
            .filter(|d| d.code.name == "undefined_identifier")
            .map(|d| d.range.offset)
            .collect();
        heads.sort_unstable();
        assert_eq!(heads, [26, 49]);
    }

    /// The guarded read face keeps its suppression in operator position: a
    /// `show`-listed name of a broken import as an operator operand stays
    /// silent — the operator head takes the Read arm's guard verbatim
    /// (`undefined_identifier` consults `shouldIgnoreUndefined`), and the
    /// miss still lands in the names table.
    #[test]
    fn shown_name_operator_head_stays_suppressed() {
        let report = report_with_edge("void f() { Missing + 1; }", broken_show("Missing"));
        assert!(report.diagnostics.is_empty(), "{:?}", code_names(&report));
        assert_eq!(report.names.len(), 1);
        assert_eq!(report.names[0].use_kind, NameUse::Read);
    }

    /// #2190 M1a residue — the extends-walk face: `escapedNameFor` is
    /// declared on `Column<T>` (`dsl/columns.dart:53`), the direct
    /// superclass of `GeneratedColumn` (`column_impl.dart:9` —
    /// `GeneratedColumn<T extends Object> extends Column<T>`; `Column`
    /// itself extends `Expression<T>`). The member walk must climb the
    /// generic `extends` edge and find it: both the implicit-this call
    /// inside `GeneratedColumn` (`column_impl.dart:123,187` —
    /// `escapedNameFor(into.dialect)`) and the declared-parameter access
    /// (`migration.dart:532` — `column.escapedNameFor(context.dialect)`)
    /// resolve silently in the oracle.
    #[test]
    fn drift_wrapper_extends_walk_member_resolves() {
        let report = report_two(
            "class Object {}\n\
             class String {}\n\
             class int {}\n\
             abstract class Expression<D extends Object> {}\n\
             abstract class Column<T extends Object> extends Expression<T> {\n\
             \x20 String get name;\n\
             \x20 String escapedNameFor(int dialect) => name;\n\
             }\n\
             class GenerationContext { int get dialect => 0; }",
            "class GeneratedColumn<T extends Object> extends Column<T> {\n\
             \x20 @override\n\
             \x20 String get name => throw '';\n\
             \x20 void writeColumnDefinition(GenerationContext into) {\n\
             \x20   final escapedName = escapedNameFor(into.dialect);\n\
             \x20   into.dialect;\n\
             \x20   escapedName;\n\
             \x20 }\n\
             }\n\
             void rename(GeneratedColumn column, GenerationContext context) {\n\
             \x20 column.escapedNameFor(context.dialect);\n\
             }",
        );
        assert_eq!(
            code_names(&report),
            Vec::<&'static str>::new(),
            "{:?}",
            report
                .diagnostics
                .iter()
                .map(|d| d.message.to_string())
                .collect::<Vec<_>>()
        );
    }

    /// #2190 M1a residue — the deeper generic hop: `GeneratedColumnWith
    /// TypeConverter<D, S extends Object> extends GeneratedColumn<S>`
    /// (`column_impl.dart:296-297`) reads `escapedNameFor` two generic
    /// instantiations up (`GeneratedColumn<S>` → `Column<S>`), and the
    /// `implements` face contributes members too (`Expression implements
    /// FunctionParameter` — `expression.dart:32`).
    #[test]
    fn drift_wrapper_deep_generic_and_interface_walk_resolves() {
        let report = report_two(
            "class Object {}\n\
             class String {}\n\
             class int {}\n\
             abstract class FunctionParameter { int get arity; }\n\
             abstract class Expression<D extends Object> implements FunctionParameter {}\n\
             abstract class Column<T extends Object> extends Expression<T> {\n\
             \x20 String get name;\n\
             \x20 String escapedNameFor(int dialect) => name;\n\
             }\n\
             class GenerationContext { int get dialect => 0; }",
            "abstract class GeneratedColumn<T extends Object> extends Column<T> {}\n\
             class GeneratedColumnWithTypeConverter<D, S extends Object>\n\
             \x20   extends GeneratedColumn<S> {\n\
             \x20 @override\n\
             \x20 String get name => throw '';\n\
             \x20 @override\n\
             \x20 int get arity => 0;\n\
             \x20 String write(GenerationContext into) => escapedNameFor(into.dialect);\n\
             }\n\
             void f(GeneratedColumnWithTypeConverter<int, String> c, Expression<String> e) {\n\
             \x20 c.escapedNameFor(0);\n\
             \x20 c.arity;\n\
             \x20 e.arity;\n\
             }",
        );
        assert_eq!(
            code_names(&report),
            Vec::<&'static str>::new(),
            "{:?}",
            report
                .diagnostics
                .iter()
                .map(|d| d.message.to_string())
                .collect::<Vec<_>>()
        );
    }

    /// #2190 M1a residue — the `Object`-member face: `inner.hashCode` where
    /// `inner` is a declared `Expression` (the corpus's
    /// `expression.dart:484/521/607` overrides read `hashCode` through the
    /// `Object` interface — upstream `objectMembers` merge, never an
    /// `undefined_getter`).
    #[test]
    fn drift_wrapper_object_member_on_expression_resolves() {
        let report = report_two(
            "class Object {}\n\
             class int {}\n\
             abstract class Expression<D extends Object> {}",
            "class Eq extends Expression<Object> {\n\
             \x20 final Expression<Object> inner;\n\
             \x20 Eq(this.inner);\n\
             \x20 @override\n\
             \x20 int get hashCode => inner.hashCode;\n\
             }\n\
             void f(Expression<Object> e) { e.hashCode; }",
        );
        assert_eq!(
            code_names(&report),
            Vec::<&'static str>::new(),
            "{:?}",
            report
                .diagnostics
                .iter()
                .map(|d| d.message.to_string())
                .collect::<Vec<_>>()
        );
    }

    /// #2190 M1a residue — the ctor-reference face: `QueryRow(row, db)` is
    /// a bare constructor call whose head resolves as the class
    /// (`custom_select.dart:55` — `rows.map((row) => QueryRow(row, _db))`).
    /// The wrapper receivers around it are user classes reached through the
    /// library namespace; the bare-ctor classification must not leak an
    /// `undefined_*` row naming the wrapper type.
    #[test]
    fn drift_wrapper_ctor_reference_resolves() {
        let report = report_two(
            "class Object {}\n\
             class QueryRow {\n\
             \x20 final Object data;\n\
             \x20 final Object db;\n\
             \x20 QueryRow(this.data, this.db);\n\
             }\n\
             class DatabaseConnection {}\n\
             class List<T> {\n\
             \x20 Iterable<R> map<R>(R Function(T) f) => throw '';\n\
             }\n\
             class Iterable<T> {\n\
             \x20 List<T> toList() => throw '';\n\
             }",
            "void f(List<Object> rows, Object db) {\n\
             \x20 rows.map((row) => QueryRow(row, db)).toList();\n\
             \x20 final conn = DatabaseConnection();\n\
             \x20 conn;\n\
             }",
        );
        assert_eq!(
            code_names(&report),
            Vec::<&'static str>::new(),
            "{:?}",
            report
                .diagnostics
                .iter()
                .map(|d| d.message.to_string())
                .collect::<Vec<_>>()
        );
    }

    // —— #2225 S2 residue pins ——————————————————————————————————————————

    /// [`report_two`] variant whose foreign unit declares `name` into the
    /// library graph but is withheld from the element index — the
    /// synthetic-SDK-face shape (`dart:core`/`dart:async`/`dart:io`
    /// declarations resolve through the namespace but carry no indexed
    /// members), which chain-breaks `walk_members` where an undeclared or
    /// indexed name would not. The unit's source declares nothing else —
    /// the frozen member tables answer for the face.
    fn report_unindexed_unit(source_a: &str, source: &str) -> FileReport {
        let strings = StringTable::new();
        let mut files = FileTable::new();
        let file_m = files.intern(Path::new("main.dart"));
        let file_a = files.intern(Path::new("a.dart"));
        let set = FeatureSet::for_language_version(LanguageVersion::new(3, 15));
        let features = ParserFeatures::from_feature_set(set);
        let scan_cfg = ScanConfig {
            enable_triple_shift: true,
            enable_augmentations: true,
        };
        let scanned_m = dartq_scanner::scan(source, scan_cfg, file_m);
        assert!(
            scanned_m.diagnostics.is_empty(),
            "scanner diagnostics: {source:?}"
        );
        let scanned_a = dartq_scanner::scan(source_a, scan_cfg, file_a);
        assert!(
            scanned_a.diagnostics.is_empty(),
            "scanner diagnostics: {source_a:?}"
        );
        let elements_m = collect(source, &scanned_m.tokens, features, &strings);
        let elements_a = collect(source_a, &scanned_a.tokens, features, &strings);
        let mut builder = LibraryGraphBuilder::new(&strings);
        let lib_m = builder.intern("package:t/main.dart");
        let lib_a = builder.intern("package:t/a.dart");
        builder.unit(lib_a, file_a);
        let mut index = ElementIndex::default();
        index.insert(file_m, elements_m.clone());
        for (name, decl) in elements_m.declarations() {
            if decl.kind == MicroKind::TypeParam {
                continue;
            }
            builder.declare(lib_m, name, file_m);
        }
        for (name, decl) in elements_a.declarations() {
            if decl.kind == MicroKind::TypeParam {
                continue;
            }
            builder.declare(lib_a, name, file_a);
        }
        builder.import(
            lib_m,
            ImportEdge {
                target: Some(lib_a),
                prefix: None,
                deferred: false,
                combinators: Combinators::unfiltered(),
            },
        );
        // `file_a` is deliberately absent from `index` — its declarations
        // resolve through the namespace but model no members.
        let graph = builder.finish();
        let namespaces = Namespaces::build(&graph);
        let mut fold = fold_unit(
            source,
            &scanned_m.tokens,
            features,
            &strings,
            file_m,
            lib_m,
            &graph,
            &namespaces,
            &index,
            StrictModes::default(),
            &LibraryClassModifiers::default(),
            set.is_enabled(dartq_language::Feature::InferenceUpdate2),
        );
        fold.report.diagnostics =
            merge_source_ordered(fold.parse_diagnostics, fold.report.diagnostics);
        fold.report
    }

    /// `(x as dynamic)` keeps the operand's own static type — `dynamic` —
    /// not the boolish latch's `bool` (the `AsExpression.staticType` face):
    /// an `int` return position accepts a `dynamic` expression silently,
    /// where a `bool`-collapsed row would report `return_of_invalid_type`.
    /// The `bool`-target control: `bool` is undeclared in the harness, so
    /// the cast reports `cast_to_non_type` and the expression's static type
    /// is the oracle's `InvalidType` — assignable into the `int` return
    /// position, so no `return_of_invalid_type` stacks on top. The
    /// trailing bare-statement shape guards the #2225 face-4 false
    /// positive: a discarded `(x as dynamic);` asks no type question at
    /// all.
    #[test]
    fn dynamic_cast_paren_keeps_the_dynamic_type() {
        let report = report_units(
            &[],
            "main.dart",
            "class Object {}\n\
             class int {}\n\
             int f(Object x) => (x as dynamic);\n\
             int g(Object x) => (x as bool);\n\
             void h(Object x) {\n\
             \x20 (x as dynamic);\n\
             \x20 ((x as dynamic));\n\
             }",
            false,
        );
        assert_eq!(
            code_names(&report),
            ["cast_to_non_type"],
            "{:?}",
            code_names(&report)
        );
    }

    /// The condition-sparing latch survives the `dynamic` fix: `x as bool`
    /// in a condition context still spares `non_bool_condition` (a `bool`
    /// cast is statically `bool`), and `x as dynamic` stays spared through
    /// the `dynamic`-assignable arm rather than the latch — same oracle
    /// verdict, no collapsed row. `bool` is undeclared in the harness, so
    /// each `as bool` site carries its `cast_to_non_type` row — the
    /// condition faces must not join them.
    #[test]
    fn bool_cast_condition_sparing_holds() {
        let report = report_units(
            &[],
            "main.dart",
            "class Object {}\n\
             void f(Object x) {\n\
             \x20 if (x as bool) {}\n\
             \x20 if ((x as bool)) {}\n\
             \x20 if (x as dynamic) {}\n\
             \x20 if ((x as dynamic)) {}\n\
             }",
            false,
        );
        assert_eq!(
            code_names(&report),
            ["cast_to_non_type", "cast_to_non_type"],
            "{:?}",
            code_names(&report)
        );
    }

    /// The interface-typed receiver face: a `Match`-typed local whose
    /// declaration resolves to an unindexed unit (the synthetic-SDK shape)
    /// chain-breaks the member walk — the frozen `Match` face
    /// (`sdk/lib/core/pattern.dart:85`) answers: `absent` reports the
    /// access-intent trio while `group`/`start`/`[]` stay silent.
    #[test]
    fn unindexed_interface_receiver_reports_absent_members() {
        let report = report_unindexed_unit(
            "abstract class Match {\n\
             \x20 int get start;\n\
             \x20 int get end;\n\
             \x20 String? group(int group);\n\
             }",
            "import 'a.dart';\n\
             void f(Match m) {\n\
             \x20 m.absent;\n\
             \x20 m.group(0);\n\
             \x20 m.start;\n\
             \x20 m.missing();\n\
             \x20 m.gone = 1;\n\
             }",
        );
        assert_eq!(
            code_names(&report),
            ["undefined_getter", "undefined_method", "undefined_setter"],
            "{:?}",
            code_names(&report)
        );
    }

    /// The deep-chain shape: `class C implements Match` breaks the member
    /// walk on the interface hop (`Match` resolves to the unindexed unit) —
    /// the frozen face still answers, so `c.absent` reports and `c.group`
    /// stays silent. The rig has no dart:core, so `C`'s own member type
    /// spellings (`int`, `String`, `int`) carry their `undefined_class`
    /// rows beside the access miss.
    #[test]
    fn unindexed_interface_hop_reports_absent_members() {
        let report = report_unindexed_unit(
            "abstract class Match {\n\
             \x20 int get start;\n\
             \x20 String? group(int group);\n\
             }",
            "import 'a.dart';\n\
             class C implements Match {\n\
             \x20 int get start => 0;\n\
             \x20 String? group(int group) => '';\n\
             }\n\
             void f(C c) { c.absent; c.group(0); c.start; }",
        );
        assert_eq!(
            code_names(&report),
            [
                "undefined_class",
                "undefined_class",
                "undefined_class",
                "undefined_getter"
            ],
            "{:?}",
            code_names(&report)
        );
    }

    /// The core-type operator face (#2235): an `int`/`double`/`num`/
    /// `String`/`bool` receiver whose declaration resolves to an unindexed
    /// unit chain-breaks the member walk, and the frozen member table now
    /// carries each face's SDK-declared operator spellings — the declared
    /// operators (`<`/`>=`/`+`/`&`/`<<`, `[]`/`+`/`*` on `String`, `&` on
    /// `bool`) stay silent exactly as the oracle answers, while a genuinely
    /// absent spelling (`[]` on `int`, `+` on the frozen `Match` face)
    /// still reports `undefined_operator`.
    #[test]
    fn unindexed_core_type_operators_decide_through_frozen_face() {
        let report = report_unindexed_unit(
            "abstract class Match {\n\
             \x20 int get start;\n\
             }\n\
             class int {}\n\
             class double {}\n\
             class num {}\n\
             class String {}\n\
             class bool {}\n",
            "import 'a.dart';\n\
             void f(int a, double d, num n, String s, bool b, Match m) {\n\
             \x20 if (a < 2 && a >= 0) {}\n\
             \x20 if (a + 1 - 2 * 3 % 4 / 5 > 0) {}\n\
             \x20 if ((a & 1 | 2 ^ 3) > 0 && (a << 1 >> 2 >>> 3) > 0) {}\n\
             \x20 if (d < 2 && n < 2) {}\n\
             \x20 s[0];\n\
             \x20 s * 2;\n\
             \x20 s + s;\n\
             \x20 if (b & true | false ^ true) {}\n\
             \x20 a[0];\n\
             \x20 m + 1;\n\
             }",
        );
        assert_eq!(
            code_names(&report),
            ["undefined_operator", "undefined_operator"],
            "{:?}",
            code_names(&report)
        );
        // The two misses are `a[0]`'s `[]` on `int` and `m + 1`'s `+` on
        // `Match` — in source order, each message carrying its spelling.
        let messages: Vec<&str> = report
            .diagnostics
            .iter()
            .map(|d| d.message.as_str())
            .collect();
        assert!(
            messages[0].contains("'[]'") && messages[0].contains("'int'"),
            "{messages:?}"
        );
        assert!(
            messages[1].contains("'+'") && messages[1].contains("'Match'"),
            "{messages:?}"
        );
    }

    /// The anchor discipline: a receiver whose type is genuinely
    /// undeclared (`Gone` resolves to nothing — the oracle's `InvalidType`)
    /// answers every member face silently; only the frozen-face spellings
    /// decide through a broken chain.
    #[test]
    fn undeclared_receiver_stays_silent() {
        let report = report_units(
            &[],
            "main.dart",
            "void f(Gone g) { g.absent; g.missing(); g.gone = 1; }",
            false,
        );
        assert_eq!(
            code_names(&report),
            ["undefined_class"],
            "{:?}",
            code_names(&report)
        );
    }

    /// The `super(name:)` public-name join (#2214's face, routed through
    /// `named_formal_for`): a `this._x` field formal's element carries the
    /// public `x` (`correspondingPublicName`, token_impl.dart:364), so
    /// `super(x: 0)` binds silently while `super(y: 0)` keeps its
    /// `undefined_named_parameter` miss. The fixture's `_x` is written by
    /// the field formals and never read — the private field carries its
    /// `unused_field` row beside the named miss.
    #[test]
    fn super_named_argument_binds_the_public_formal_name() {
        let report = report_units(
            &[],
            "main.dart",
            "class int {}\n\
             class B {\n\
             \x20 B({required this._x});\n\
             \x20 final int _x;\n\
             }\n\
             class C extends B {\n\
             \x20 C() : super(x: 0);\n\
             }\n\
             class D extends B {\n\
             \x20 D() : super(y: 0);\n\
             }",
            false,
        );
        assert_eq!(
            code_names(&report),
            ["unused_field", "undefined_named_parameter"],
            "{:?}",
            code_names(&report)
        );
    }

    /// The `on`-face union in `check_instance_member` (#2186 S2's
    /// `on_face_walk`, routed through the Mixin/Extension arm): a
    /// declared-typed receiver's miss decidable through the constraint
    /// climb reports the access intent naming the static type — the
    /// oracle's interface lookup (the constraint members ride `M`'s
    /// interface; `lookup_failure_reporter.dart`'s instance domain).
    #[test]
    fn mixin_typed_receiver_miss_names_the_mixin() {
        let report = report_units(
            &[],
            "main.dart",
            "class C { void hit() {} } \
             mixin M on C {} \
             void f(M m) { m.miss(); }",
            false,
        );
        let names = code_names(&report);
        assert_eq!(names, ["undefined_method"], "{names:?}");
        assert!(
            report.diagnostics[0].message.contains("'M'"),
            "{:?}",
            report.diagnostics[0].message
        );
    }

    /// The union's resolved side: a constraint member reached through the
    /// `on` climb silences the access exactly like an own member.
    #[test]
    fn mixin_typed_receiver_hit_on_the_constraint_stays_silent() {
        let report = report_units(
            &[],
            "main.dart",
            "class C { void hit() {} } \
             mixin M on C {} \
             void f(M m) { m.hit(); }",
            false,
        );
        assert_eq!(
            code_names(&report),
            Vec::<&'static str>::new(),
            "{:?}",
            code_names(&report)
        );
    }

    /// A `this` receiver's decidable `on`-face miss inside an extension
    /// body reports naming the `on` type (#2230's emit-side flip — the
    /// oracle's `InstanceLookupDomain` names `this`'s static type: the
    /// extended `C`, never the extension). An unresolved `on` name keeps
    /// the record-only arm — nothing honest names the report.
    #[test]
    fn this_miss_inside_extension_body_reports_on_the_on_type() {
        let report = report_units(
            &[],
            "main.dart",
            "class C { void hit() {} } \
             extension X on C { void g() { this.miss(); } }",
            false,
        );
        let names = code_names(&report);
        assert_eq!(names, ["undefined_method"], "{names:?}");
        assert!(
            report.diagnostics[0].message.contains("'C'"),
            "{:?}",
            report.diagnostics[0].message
        );
    }
}
