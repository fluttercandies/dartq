use super::*;

/// The arm-scope sentinel range inside [`LocalDecl::group`]: a case-header
/// pattern variable (`case A(:var e)`, a switch-expression arm) encodes its
/// declaring switch-stack depth as `BASE - depth`, so [`Engine::resolve_local`]
/// can drop the binding once the switch closed — the official scopes a case
/// pattern's variables to the arm (`unused_local_elements_verifier` visits
/// them as locals of the arm's scope), and a read past the switch must bind
/// the outer declaration, never the arm's (the search.dart
/// `var element = switch (…) { ExecutableInvocationResolution(:var element)
/// => … }` census shape, #2189-a). Real pattern-declaration group ids count
/// up from 1; reaching the sentinel range would need ~4 billion pattern
/// groups in one unit. Depths are syntactic nesting — a fraction of
/// [`u16::MAX`]; deeper stacks stay untagged (the flat-frame under-report).
const PATTERN_ARM_SCOPE_BASE: u32 = u32::MAX - u16::MAX as u32;

/// The dart:core iterable-producer getters whose declared returns the
/// member walk cannot reach (the declaring units are unindexed like every
/// SDK unit, so `walk_members` chain-breaks on them). Each row is
/// `(container, getter, element type-argument index, result head)` from
/// the reference SDK (`sdk/lib/core/map.dart` — `Iterable<K> get keys`,
/// `Iterable<V> get values`); a chained call's element face reads the
/// receiver's written type argument at the row's index
/// (`m.values.map((n) => …)` binds `n` from `Map`'s second argument).
const CHAIN_PRODUCER_GETTERS: [(&str, &str, usize, &str); 2] = [
    ("Map", "keys", 0, "Iterable"),
    ("Map", "values", 1, "Iterable"),
];

/// One parameter slot of a formal table: a positional ordinal or a named
/// spelling — the pairing the closure-literal inference walks on both sides
/// (`_inferFormalParameters`'s positional iterators and its
/// `namedParameterTypes` map).
#[derive(Clone, Copy)]
enum SchemaSlot {
    Positional(usize),
    Named(StringId),
}

impl<'a> Engine<'a> {
    /// A local variable declaration (`LocalVariableDeclaration`): the
    /// tracked local of the open frame, and the statement-list duplicate
    /// scope's newest name.
    pub(crate) fn note_local(&mut self, name: StringId, range: SourceRange) {
        self.dup_scope_add(name, range);
        self.add_tracked_local(name, range, LocalKind::Variable);
    }

    /// A formal parameter of the executable whose name has fired: the
    /// tracked local joins its declaration site for the
    /// `unused_element_parameter` body-reference rule.
    pub(crate) fn note_parameter(&mut self, name: StringId, range: SourceRange) {
        self.add_tracked_local(
            name,
            range,
            LocalKind::Parameter {
                site: self.open_exec_site,
            },
        );
        if let Some(site) = self.open_exec_site {
            self.site_params
                .entry(site)
                .or_default()
                .push((name, range));
        }
    }

    /// A catch clause parameter: first is the exception variable, second
    /// the stack trace. The exception's auto-use bit patches in at
    /// `handle_catch_block` when both facts are known.
    pub(crate) fn note_catch_parameter(&mut self, name: StringId, range: SourceRange) {
        let kind = if self.catch_params == 0 {
            LocalKind::CatchException { auto: false }
        } else {
            LocalKind::CatchStack
        };
        self.catch_params += 1;
        self.add_tracked_local(name, range, kind);
    }

    /// A named local function (reports as `unused_element`). The name
    /// event fires inside the function's own frame: the name joins the
    /// ENCLOSING frame's resolution set (its official scope — in-body
    /// recursion resolves through it, and outer invocations find it),
    /// while the tracked entry defers to the end event — its use-counting
    /// window is the enclosing frame's, so an outer invocation spares it
    /// (the official `_enclosingExec` keeps only in-body self-references
    /// from counting: the entry is absent from the tracked stack while
    /// the body parses, so a self-reference finds no declaration).
    pub(crate) fn note_local_function(&mut self, name: StringId, range: SourceRange) {
        // The name joins the enclosing statement list's duplicate scope
        // (`checkStatements`' `FunctionDeclarationStatement` arm): the name
        // event fires before the body block opens the function's own scope,
        // so the innermost scope is the enclosing list's.
        self.dup_scope_add(name, range);
        if self.frames.len() >= 2
            && let Some(enclosing) = self.frames.len().checked_sub(2)
            && let Some(frame) = self.frames.get_mut(enclosing)
        {
            frame.locals.insert(name);
        }
        self.pending_local_fn = Some((name, range));
        if let Some(depth) = self.frames.len().checked_sub(1) {
            self.local_fn_stack.push((depth, name));
        }
    }

    /// `end_local_function_declaration` / `end_named_function_expression`:
    /// the tracked local-function entry materializes into the enclosing
    /// frame's window just before the frame pops.
    pub(crate) fn finish_local_fn_site(&mut self) {
        if let Some((name, range)) = self.pending_local_fn.take() {
            self.locals.push(LocalDecl {
                name,
                range,
                kind: LocalKind::LocalFunction,
                uses: 0,
                group: None,
            });
        }
    }

    /// `begin_import`: the URI literal is the token after the keyword.
    /// Only an honestly quoted literal joins (recovery shapes stay
    /// silent — the under-report direction).
    #[expect(clippy::cast_possible_truncation)] // span arithmetic narrows under the scanner u32 offset cap
    pub(crate) fn begin_import_site(&mut self, import_kw: Token) {
        let Some(uri) = self.tokens.get(import_kw.id as usize + 1) else {
            return;
        };
        let raw = &self.source[uri.byte_range()];
        let bytes = raw.as_bytes();
        if bytes.len() < 2 {
            return;
        }
        let quote = bytes[0];
        if (quote != b'\'' && quote != b'"') || bytes[bytes.len() - 1] != quote {
            return;
        }
        self.imports.push(ImportSite {
            uri_text: raw[1..raw.len() - 1].to_string(),
            uri_range: SourceRange {
                offset: uri.utf16_offset,
                length: raw.chars().map(char::len_utf16).sum::<usize>() as u32,
            },
            prefix: None,
            shown: Vec::new(),
        });
    }

    /// The statement-window read rule (the official `_isUsefulRead`,
    /// unused_local_elements_verifier.dart:603-619): a single unqualified
    /// read in an expression statement whose parent shape is a
    /// null-assertion, a logical-not, an increment/decrement, or a
    /// unary-operator invocation never counts (`!v;`, `v!;`, `v++;`,
    /// `++v;`, `-v;`); a compound assignment's read does not count either
    /// (`visitCompoundAssignment`'s `readCountsAsUse: parent is!
    /// ExpressionStatement`). Plain `v;`, `v + 1;`, and `v ??= 1;` are
    /// useful reads.
    ///
    /// A discarded bare row also joins `statement_discarded_rows`, the
    /// pass-2 gate that refuses the field's implicit-getter read credit
    /// (`unused_field`) — one verdict, both faces.
    pub(crate) fn check_statement_use(&mut self, begin: Token, end: Token) {
        // The statement boundary closes the pattern-declaration window: a
        // later group is its own all-or-nothing set.
        self.pattern_group_open = None;
        let w0 = begin.range.offset;
        let w1 = end.range.end();
        // The window's named rows (value rows are literals, groups,
        // closures, record literals — their shapes are never the discarded
        // read): one plain read at most, plus the compound-assignment
        // targets (upstream `visitCompoundAssignment`'s `readCountsAsUse:
        // parent is! ExpressionStatement` — the target's read never counts
        // in a statement window, regardless of the other identifiers in it,
        // so `x += i;` still spares `i` while cancelling `x`). Each row
        // carries its bare-ness: only a bare row IS the local — a member
        // row's hit is the RECEIVER's occurrence credit, and the receiver
        // of `v.m += 1;`/`v.m++;` reads `v` as a plain useful read (the
        // official `_visitNameExpression`/`_isUsefulRead` — `v`'s own
        // parent is the member access, never the discarded operator).
        let mut read_row: Option<(SourceRange, bool)> = None;
        let mut rw_rows: Vec<(SourceRange, bool)> = Vec::new();
        for row in self.rows.iter().rev() {
            if row.range.end() <= w0 {
                break;
            }
            if row.range.offset >= w1 || matches!(row.shape, RowShape::Value | RowShape::Record(_))
            {
                continue;
            }
            let bare = matches!(row.shape, RowShape::Bare(_));
            match row.intent {
                Intent::Read => {
                    if read_row.is_some() {
                        return; // two plain reads: the reads are useful
                    }
                    read_row = Some((row.range, bare));
                }
                Intent::ReadWrite => rw_rows.push((row.range, bare)),
                // invoke/write/operator rows are always uses
                _ => return,
            }
        }
        // The window's token texts (bounded by the statement itself); the
        // offset side is the token's UTF-16 offset, comparable with the
        // rows' ranges.
        let window_ops: Vec<(u32, &str)> = (begin.id..=end.id)
            .filter_map(|id| {
                self.tokens
                    .get(id as usize)
                    .map(|t| (t.utf16_offset, &self.source[t.byte_range()]))
            })
            .collect();
        // A compound assignment discards its targets; `??=` never does
        // (`visitIfNullAssignment` reads count as use unconditionally).
        let compound_write = window_ops.iter().any(|(_, text)| {
            matches!(
                *text,
                "+=" | "-="
                    | "*="
                    | "/="
                    | "%="
                    | "~/="
                    | "&="
                    | "|="
                    | "^="
                    | "<<="
                    | ">>="
                    | ">>>="
            )
        });
        if compound_write {
            for (row_range, bare) in &rw_rows {
                if !*bare {
                    continue;
                }
                // The field-face twin: the pass-2 bare read-credit gate
                // consults the same discard (`_isReadIdentifier` gates the
                // official `readMembers` join and the local `uses` count
                // with one verdict).
                self.statement_discarded_rows.insert(row_range.offset);
                if let Some(hit) = self.local_hits.remove(&row_range.offset) {
                    self.locals[hit as usize].uses -= 1;
                }
            }
        }
        if let Some((row_range, bare)) = read_row {
            let head_op = self
                .text_of(begin)
                .is_some_and(|text| matches!(text, "!" | "-" | "~" | "++" | "--"));
            let tail_op = window_ops.iter().any(|(offset, text)| {
                *offset == row_range.end() && matches!(*text, "!" | "++" | "--")
            });
            if (head_op || tail_op) && bare {
                self.statement_discarded_rows.insert(row_range.offset);
                if let Some(hit) = self.local_hits.remove(&row_range.offset) {
                    self.locals[hit as usize].uses -= 1;
                }
            }
        }
    }

    /// `handle_catch_block`: the clause's facts are complete (on-type,
    /// parameters, body block). The official auto-use decision patches the
    /// exception variable (plain `catch`, or a stack-trace sibling, uses
    /// it), the clause joins the try's clause list, and the catch frame
    /// closes — the parameters leave scope only after the body.
    pub(crate) fn finish_catch_clause(&mut self) {
        let auto = self.catch_on_type.is_none() || self.catch_params > 1;
        if let Some(mark) = self.frame_local_marks.last().copied() {
            for decl in self.locals[mark..].iter_mut().rev() {
                if let LocalKind::CatchException { auto: slot } = &mut decl.kind {
                    *slot = auto;
                    break;
                }
            }
        }
        self.catch_params = 0;
        let Some(start) = self.catch_clause_start.take() else {
            self.catch_on_type = None;
            self.catch_on_nullable = None;
            // The `on`-type miss offsets stay: pass 2 resolves the TypeUse
            // rows after the clauses closed, and each offset is consumed
            // by exact match.
            return;
        };
        let on_type = self.catch_on_type.take();
        let on_nullable = self.catch_on_nullable.take().unwrap_or(false);
        if let Some(clauses) = self.try_clauses.last_mut() {
            clauses.push(CatchClauseFact {
                on_type,
                on_nullable,
                start,
                end: self.last_block_end.unwrap_or(start),
            });
        }
        self.pop_frame();
    }

    /// `end_try_statement`: the official `_CatchClausesVerifier` — a
    /// catch-all clause (no `on` type, or `on Object`) followed by more
    /// clauses makes the tail dead; and an on-clause whose type is a
    /// subtype of an earlier on-clause type in the same try is dead too
    /// (`dead_code_verifier.dart`, `nextCatchClause`; the subtype arm,
    /// :589-604). Either way the span covers the first dead clause through
    /// the last one, and one dead-catch row per try at most (`_done`).
    pub(crate) fn finish_try_statement(&mut self) {
        let Some(clauses) = self.try_clauses.pop() else {
            return;
        };
        let object = self.intern("Object");
        let mut visited: Vec<Ty> = Vec::new();
        for (index, clause) in clauses.iter().enumerate() {
            let catches_all = clause.on_type.is_none_or(|t| t == object);
            if catches_all {
                if index + 1 < clauses.len() {
                    let start = clauses[index + 1].start.offset;
                    let end = clauses[clauses.len() - 1].end.end();
                    self.stage_usage(
                        SemanticCode::DeadCodeCatchFollowingCatch,
                        SourceRange {
                            offset: start,
                            length: end.saturating_sub(start),
                        },
                        "",
                    );
                }
                break;
            }
            // The subtype arm: `isSubtypeOf(current, earlier)` over the
            // written on-types (`on int?` is not below `on int`).
            let current = self.catch_on_ty(clause);
            if let Some(earlier) = visited
                .iter()
                .find(|earlier| self.name_subtype(current, **earlier))
                .copied()
            {
                let start = clause.start.offset;
                let end = clauses[clauses.len() - 1].end.end();
                let a0 = self.ty_display(current);
                let a1 = self.ty_display(earlier);
                self.stage_typed(
                    SemanticCode::DeadCodeOnCatchSubtype,
                    SourceRange {
                        offset: start,
                        length: end.saturating_sub(start),
                    },
                    &a0,
                    &a1,
                );
                break;
            }
            visited.push(current);
        }
    }

    /// The written on-clause type as a [`Ty`] (`dynamic`/`Null` keep their
    /// dedicated shapes; everything else is the written name with its
    /// trailing-`?` face).
    pub(crate) fn catch_on_ty(&self, clause: &CatchClauseFact) -> Ty {
        let Some(name) = clause.on_type else {
            return Ty::UNKNOWN;
        };
        match self.strings.resolve(name) {
            "dynamic" => Ty::Dynamic,
            "Null" => Ty::Null,
            _ => Ty::Named {
                name,
                nullable: clause.on_nullable,
                args: ArgListId::EMPTY,
            },
        }
    }

    /// `handle_label` (the colon fired): the pending label opens its own
    /// scope; `end_labeled_statement` pops one scope per label.
    pub(crate) fn note_label_decl(&mut self) {
        if let Some((name, range)) = self.pending_label.take() {
            self.label_scopes.push(LabelScope {
                labels: vec![(name, range, false)],
            });
        }
    }

    /// A `break`/`continue` target: the innermost scope declaring the name
    /// marks its label used.
    pub(crate) fn note_label_use(&mut self, name: StringId) {
        for scope in self.label_scopes.iter_mut().rev() {
            if let Some(slot) = scope.labels.iter_mut().find(|(n, _, _)| *n == name) {
                slot.2 = true;
                return;
            }
        }
    }

    /// `end_labeled_statement`: one scope per label pops; the statement's
    /// labels merge (nested `a: b: stmt` declares both) and the unused
    /// ones report (the official `_LabelTracker` pop,
    /// `dead_code_verifier.dart`).
    pub(crate) fn finish_labeled_statement(&mut self, label_count: usize) {
        let count = label_count.min(self.label_scopes.len());
        if count == 0 {
            return;
        }
        let split = self.label_scopes.len() - count;
        let mut merged: Vec<(StringId, SourceRange, bool)> = Vec::new();
        for scope in self.label_scopes.drain(split..) {
            merged.extend(scope.labels);
        }
        for (name, range, used) in merged {
            if !used {
                let text = self.strings.resolve(name).to_string();
                self.stage_usage(SemanticCode::UnusedLabel, range, &text);
            }
        }
    }

    /// `begin_binary_expression` for `??`: the left operand's row parks
    /// with the operator span.
    pub(crate) fn begin_if_null(&mut self, op_range: SourceRange) {
        self.if_null_groups.push((self.rows.len(), op_range));
    }

    /// The dead-coalesce operand's never-null verdict
    /// (`_checkForDeadNullCoalesce`'s operand face): the row's own type,
    /// widened to the nullable face when a `?.` merge stands at the row's
    /// offset (`x?.f ?? d` keeps the null union the merge could not join —
    /// the same member-offset latch `null_comparison`'s operand face
    /// re-arms), and the send's RESULT for an invoked operand (`a.b() ?? c`
    /// reads `b`'s declared return — the row keeps the callee's declared
    /// type, never the send's; the same shape `initializer_static_ty`'s
    /// Invoke arm answers). A type-parameter send result suppresses the
    /// verdict — the written name carries no bound (`null_comparison`'s
    /// posture), so `m[key(e)] ?? d`'s generic key send never anchors the
    /// dead face.
    fn coalesce_operand_never_null(&self, row: &Row) -> bool {
        if self.null_aware_members.contains(&row.range.offset) {
            return Self::nullable_form(row.ty).is_strictly_non_nullable();
        }
        if matches!(row.intent, Intent::Invoke) {
            let ty = self.operand_static_ty(row);
            if ty != Ty::UNKNOWN {
                return match ty {
                    Ty::Named {
                        name,
                        nullable: false,
                        ..
                    } if self.type_params.contains(&name)
                        || self.pending_type_params.contains(&name) =>
                    {
                        false
                    }
                    _ => ty.is_strictly_non_nullable(),
                };
            }
        }
        row.ty.is_strictly_non_nullable()
    }

    /// `end_binary_expression` for `??`: the right operand's row anchors
    /// the report when the left operand's type is strictly non-nullable —
    /// the right operand is never executed (the official
    /// `_checkForDeadNullCoalesce`, error_verifier.dart:4733-4739). An
    /// equality comparison as the whole left operand counts too: its
    /// static type is `bool` (`x != null ?? false`).
    pub(crate) fn end_if_null(&mut self) {
        let Some((mark, _op_range)) = self.if_null_groups.pop() else {
            return;
        };
        let mut lhs_is_bool_comparison = false;
        if let Some(eq) = self.eq_spans.last().copied() {
            // The comparison's own extent: its two operand rows — the eq
            // latch spans from the left row's head through the right row's
            // end (the `??` right operand's row sits above them).
            let left = mark
                .checked_sub(2)
                .and_then(|index| self.rows.get(index))
                .map(|row| row.range);
            let eq_right = mark
                .checked_sub(1)
                .and_then(|index| self.rows.get(index))
                .map(|row| row.range);
            if let (Some(left), Some(eq_right)) = (left, eq_right)
                && eq.offset == left.offset
                && eq.end() == eq_right.end()
            {
                lhs_is_bool_comparison = true;
                self.eq_spans.pop();
            }
        }
        let non_null = lhs_is_bool_comparison
            || mark
                .checked_sub(1)
                .and_then(|index| self.rows.get(index))
                // An operator-marked row is the index/`+` geometry, never
                // the read's own type (`m[k] ?? d` reads `Map.[]`'s `V?` —
                // #2172's `groupSetsBy`/`groupBy` rows). An invoked row
                // reads its send result through the shared verdict, whose
                // type-parameter posture keeps a generic key send
                // (`m[keyOf(e)] ?? d`) from anchoring.
                .filter(|lhs| !matches!(lhs.intent, Intent::Operator(..)))
                .is_some_and(|lhs| self.coalesce_operand_never_null(lhs));
        if !non_null {
            return;
        }
        if let Some(rhs) = self.rows.last() {
            let range = rhs.range;
            self.stage_usage(SemanticCode::DeadNullAwareExpression, range, "");
        }
    }

    /// `handle_assignment_expression` for `??=`: the target's read type
    /// strictly non-nullable anchors the report at the value (the official
    /// `_checkForDeadNullCoalesce`'s `??=` arm). An operator-marked row is
    /// never the target: `m[k] ??= v` reads through `Map.[]`, whose `V?`
    /// the name-level model does not type on the receiver row — the
    /// receiver's own non-nullability is not the read's (#2172's
    /// `groupSetsBy`/`groupBy` rows).
    pub(crate) fn check_dead_null_assignment(&mut self, operator: Token) {
        if self.text_of(operator) != Some("??=") {
            return;
        }
        let limit = operator.range.offset;
        // The index-write shape reads through `Map.[]`, whose `V?` no row
        // carries: the compound mark lands on the key expression's row (the
        // nearest non-value before the operator), whose return type is not
        // the read's — the index-read target's type is the unmodeled
        // operator return, and the face stays silent (`groupBy`'s
        // `(map[key(element)] ??= []).add(element)`, #2172).
        if self
            .rows
            .iter()
            .rev()
            .filter(|row| row.range.end() <= limit)
            .any(|row| matches!(row.write_op, Some(OpWrite::Compound)))
        {
            return;
        }
        let non_null = self
            .rows
            .iter()
            .rev()
            .find(|row| {
                row.range.end() <= limit
                    && !matches!(row.shape, RowShape::Value)
                    && !matches!(row.intent, Intent::Operator(..))
            })
            .is_some_and(|target| self.coalesce_operand_never_null(target));
        if non_null && let Some(value) = self.rows.last() {
            let range = value.range;
            self.stage_usage(SemanticCode::DeadNullAwareExpression, range, "");
        }
    }

    /// The local declaration `name` resolves to, innermost frame first
    /// (the same walk [`Engine::resolve_bare`]'s locals arm runs).
    ///
    /// An arm-scoped case-header pattern variable (the sentinel group,
    /// [`PATTERN_ARM_SCOPE_BASE`]) is a candidate only while its
    /// declaring switch context is still on the stack — a read past the
    /// switch falls through to the outer declaration, the official
    /// arm-scope end (`#2189`-a's leak direction).
    #[expect(clippy::cast_possible_truncation)] // span arithmetic narrows under the scanner u32 offset cap
    pub(crate) fn resolve_local(&self, name: StringId) -> Option<u32> {
        for depth in (0..self.frames.len()).rev() {
            if self.frames[depth].locals.contains(&name) {
                let lo = self.frame_local_marks[depth];
                let hi = self
                    .frame_local_marks
                    .get(depth + 1)
                    .copied()
                    .unwrap_or(self.locals.len());
                // Within one frame, a use follows its declaration
                // textually: the last entry under the name is it — unless
                // that entry is an arm-scoped pattern variable whose
                // switch already closed. The tag's stack slot must still
                // be covered AND held by the declaring switch: a context
                // whose keyword starts after the declaration is a later
                // sibling switch at the same depth, never the tag's own.
                let switches = &self.switch.switches;
                return self.locals[lo..hi]
                    .iter()
                    .rposition(|d| {
                        d.name == name
                            && match d.group {
                                Some(g) if g >= PATTERN_ARM_SCOPE_BASE => {
                                    let slot = (u32::MAX - g) as usize;
                                    switches.len() >= slot
                                        && switches.get(slot - 1).is_some_and(|ctx| {
                                            ctx.switch_kw.offset < d.range.offset
                                        })
                                }
                                _ => true,
                            }
                    })
                    .map(|p| (lo + p) as u32);
            }
        }
        None
    }

    pub(crate) fn container_context(&self) -> Option<(StringId, ContainerKind)> {
        self.frames.iter().rev().find_map(|f| match &f.kind {
            FrameKind::Container { name, kind, .. } => Some((*name, *kind)),
            _ => None,
        })
    }

    /// The innermost container frame's application-face member map when
    /// that frame is an unnamed extension (the frame's face-rank latch,
    /// resolved against the pool's nameless faces in declaration order),
    /// `None` for every other frame shape. An unnamed extension has no
    /// container entry (`close_container` moves its members into
    /// `extension_faces` outright), so this map is the frame's only
    /// own-member face; a broken placeholder-name walk must not answer
    /// for it (#1701).
    pub(crate) fn unnamed_extension_members(&self) -> Option<&crate::model::NameMap<MemberEntry>> {
        let innermost = self
            .frames
            .iter()
            .rev()
            .find(|f| matches!(f.kind, FrameKind::Container { .. }))?;
        let FrameKind::Container {
            extension_face: Some(face),
            ..
        } = &innermost.kind
        else {
            return None;
        };
        Some(
            &self
                .index
                .get(self.file)?
                .extension_faces()
                .iter()
                .filter(|f| f.name.is_none())
                .nth(*face)?
                .members,
        )
    }

    /// The unnamed extension's own-member formals for `name` (the
    /// invocation arity face's route, #1881): the frame's application-face
    /// member map answers when `name` is a declared method, exactly as the
    /// named twin's container walk does. `None` outside an
    /// unnamed-extension frame or for every other member kind — the
    /// named-container route owns those.
    pub(crate) fn unnamed_extension_method_formals(&self, name: StringId) -> Option<Formals> {
        let entry = self.unnamed_extension_members()?.get(&name)?;
        (entry.kind == MemberKind::Method)
            .then(|| entry.formals.as_deref().cloned())
            .flatten()
    }

    /// Whether the innermost executable is an instance member.
    pub(crate) fn instance_context(&self) -> bool {
        self.frames
            .iter()
            .rev()
            .find_map(|f| match &f.kind {
                FrameKind::Executable {
                    instance, ret: _, ..
                } => Some(*instance),
                _ => None,
            })
            .unwrap_or(false)
    }

    pub(crate) fn in_executable(&self) -> bool {
        self.frames
            .iter()
            .any(|f| matches!(f.kind, FrameKind::Executable { .. }))
    }

    /// Binds a local's declared type in the innermost frame (the
    /// declared-variable-pattern binding face).
    pub(crate) fn bind_var_type(&mut self, name: StringId, ty: Ty) {
        if let Some(frame) = self.frames.last_mut() {
            frame.var_types.insert(name, ty);
        }
    }

    /// The flat record destructure bind (`final (a, b) = pair()`): the
    /// pattern variables carry no written type — the oracle infers each
    /// from the matched record's positional slots
    /// (`analyzeRecordPattern`, type_analyzer.dart's positional walk). The
    /// initializer's folded row sits between `init_start` (the `=`'s end)
    /// and `semi` (the `;`'s offset); exactly one non-TypeUse row there is
    /// the expression's own. The bind runs only when the whole declaration
    /// is one flat positional record — `pattern_bind_records == 1`, no
    /// named field or other shape left its `pattern_bind_dirty` mark, and
    /// the collected untyped variables line up one-per-positional-slot —
    /// so every unsupported shape (nested records, named fields, object/
    /// list/map/cast members, typed leaves beside bare ones) keeps the
    /// documented under-report instead of binding a wrong type.
    ///
    /// Case-head and assignment-pattern windows share the same append-only
    /// collection — their variables linger into the next declaration's
    /// window, where the count mismatch suppresses the bind (the
    /// under-report direction either way).
    pub(crate) fn destructure_record(&mut self, init_start: u32, semi: u32) {
        let vars = std::mem::take(&mut self.pattern_bind_vars);
        let records = self.pattern_bind_records;
        let dirty = self.pattern_bind_dirty;
        self.pattern_bind_records = 0;
        self.pattern_bind_dirty = false;
        if dirty || records != 1 || vars.is_empty() {
            return;
        }
        let mut inits = self
            .rows
            .iter()
            .filter(|row| {
                !matches!(row.shape, RowShape::TypeUse(..))
                    && row.range.offset >= init_start
                    && row.range.end() <= semi
            })
            .cloned();
        let (Some(row), None) = (inits.next(), inits.next()) else {
            return;
        };
        let Some(ty) = self.initializer_static_ty(&row) else {
            return;
        };
        let Ty::Record { shape, .. } = ty else {
            return;
        };
        let fields = self.boundary.pool().record_shape(shape).positional.to_vec();
        if fields.len() != vars.len() {
            return;
        }
        if let Some(frame) = self.frames.last_mut() {
            for (name, ty) in vars.iter().zip(fields) {
                frame.var_types.insert(*name, ty);
            }
        }
    }

    /// Classifies a constant pattern whose events left no literal: a
    /// qualified `E.c` tail (enum container + member row) becomes the
    /// enum-value leaf; everything else is unrepresentable.
    pub(crate) fn classify_constant_tail(&self) -> Restriction {
        // The member push replaces the qualifier row, so the `E` of `E.c`
        // survives only as the member row's container receiver.
        let Some(last) = self.rows.last() else {
            return Restriction::Unknown;
        };
        let RowShape::Member { recv } = &last.shape else {
            return Restriction::Unknown;
        };
        match recv {
            Receiver::Container {
                name,
                kind: ContainerKind::Enum,
            } => Restriction::EnumValue(*name, last.name),
            _ => Restriction::Unknown,
        }
    }

    /// The class-static-constant tail's own type: `case A.a:` over a class
    /// container names a static const member, and the constant's static
    /// type is the folded value's (upstream `visitConstantPattern`
    /// evaluates the reference and reads `value.type` — upstream Dart SDK
    /// source, pkg/analyzer/lib/src/dart/constant/constant_verifier.dart).
    /// Answered only for in-file statics whose value already folded to a
    /// shape the oracle's `hasPrimitiveEquality` accepts unconditionally —
    /// the primitive literals and enum values, the arming set the literal
    /// and enum-tail faces already own: a user-class value's primitive
    /// equality hinges on `==`/`hashCode` overrides the name level cannot
    /// survey faithfully (an inherited SDK override such as `Duration`'s
    /// stays silent upstream), so the `Created` and collection shapes keep
    /// the documented under-report. Static non-consts (`static final`)
    /// carry no folded value and stay silent — the oracle's
    /// `constant_pattern_with_non_constant_expression` there is the eval
    /// face's row, not this one.
    pub(crate) fn class_static_const_tail_ty(&self) -> Option<Ty> {
        // The member push replaces the qualifier row, so `A` survives only
        // as the member row's container receiver (the same shape
        // [`Self::classify_constant_tail`] reads for the enum tail).
        let last = self.rows.last()?;
        let RowShape::Member {
            recv:
                Receiver::Container {
                    name,
                    kind: ContainerKind::Class,
                },
        } = &last.shape
        else {
            return None;
        };
        let value = self.const_field_values.get(&(*name, last.name))?;
        // `Created` folds to the container's own type like an enum value
        // does, but its primitive equality is undecidable here — excluded
        // before the fold.
        if matches!(value, ConstantValue::Created(_)) {
            return None;
        }
        let ty = self.folded_ty(value);
        (ty != Ty::UNKNOWN).then_some(ty)
    }

    /// A pattern variable declared outside every frame (the top-level
    /// variable-initializer world) still binds for the enclosing clause's
    /// reads: the guard and case body of a switch/if-case expression in
    /// the initializer resolve to it. There is no frame to pop, so the
    /// set clears with the declaration that staged it
    /// ([`Self::end_top_level_fields`]'s event) — a wider binding than the
    /// official clause scope only under-reports undefined-name, the flat
    /// frame's documented direction.
    pub(crate) fn add_local(&mut self, name: StringId) {
        if let Some(frame) = self.frames.last_mut() {
            frame.locals.insert(name);
        } else {
            self.initializer_locals.insert(name);
        }
    }

    /// Records a local declaration of the usage cluster: the frame's name
    /// set (resolution) plus the tracked entry (kind, anchor, group).
    pub(crate) fn add_tracked_local(
        &mut self,
        name: StringId,
        range: SourceRange,
        kind: LocalKind,
    ) {
        self.add_local(name);
        // A pattern-variable-declaration window claims the pattern-declared
        // variables of one declaration: the first opens it, the rest join
        // its group (one used member spares all). The window is the GROUP
        // ID, never a locals index — an index outlived frame pops and read
        // a truncated slot.
        //
        // A case-header pattern variable is arm-scoped instead: it takes
        // the sentinel group ([`PATTERN_ARM_SCOPE_BASE`]) carrying its
        // declaring switch depth, never joins the window, and never opens
        // one — the all-or-nothing group is a `PatternVariableDeclaration`
        // face, while the official visits a case pattern's variables
        // individually (each is its own local element).
        let group = if matches!(kind, LocalKind::PatternVariable) {
            let depth = self.switch.switches.len();
            if self.switch.case_sink_open
                && !self.switch.if_case_window
                && depth > 0
                && depth <= u16::MAX as usize
            {
                Some(u32::MAX - depth as u32)
            } else if let Some(group) = self.pattern_group_open {
                Some(group)
            } else {
                self.pattern_group_seq += 1;
                self.pattern_group_open = Some(self.pattern_group_seq);
                Some(self.pattern_group_seq)
            }
        } else {
            // A non-pattern declaration closes the window: a group spans
            // consecutive pattern-declared variables only.
            self.pattern_group_open = None;
            None
        };
        self.locals.push(LocalDecl {
            name,
            range,
            kind,
            uses: 0,
            group,
        });
    }

    /// A top-level statement declaration joins the open statement-list
    /// duplicate scope (`duplicate_definition_verifier.dart`
    /// `checkStatements` — a block body, a function body, and each switch
    /// member's statement list own one scope; a repeat inside the same
    /// scope is `duplicate_definition` at the second name, the first
    /// declaration riding the context row). A name in an open for head
    /// joins the head's own list scope instead (`checkForVariables` — a
    /// loop's declaration list checks inside itself, never against the
    /// enclosing block); wildcards never collide (the verifier's
    /// `isWildcard` arm).
    pub(crate) fn dup_scope_add(&mut self, name: StringId, range: SourceRange) {
        if self.is_wildcard(name) {
            return;
        }
        if self.in_for_parts.last().is_some() {
            let first = self.dup_for_parts.insert(name, range);
            if let Some(first) = first {
                self.stage_local_duplicate(name, range, first);
            }
            return;
        }
        let first = self
            .dup_scopes
            .last_mut()
            .and_then(|scope| scope.insert(name, range));
        if let Some(first) = first {
            self.stage_local_duplicate(name, range, first);
        }
    }

    /// Stages one local `duplicate_definition` (ERROR at the second
    /// declaration's name token, the oracle's first-declaration context row
    /// attached) — the same shape the capture-time log's
    /// [`duplicate_definition_diagnostics`] renders for the file-level
    /// face. The usage sink is the staging pool the fold merges
    /// source-ordered into the report tail.
    pub(crate) fn stage_local_duplicate(
        &mut self,
        name: StringId,
        second: SourceRange,
        first: SourceRange,
    ) {
        let text = self.strings.resolve(name).to_string();
        let diagnostic = Diagnostic::new(
            Severity::Error,
            SemanticCode::DuplicateDefinition.code_info(),
            SemanticCode::DuplicateDefinition.message(&text, ""),
            self.file,
            second,
            None,
        )
        .with_context(vec![ContextMessage {
            message: SharedStr::from("The first definition of this name."),
            file: self.file,
            range: first,
        }]);
        self.report.usage.push(diagnostic);
    }

    /// Binds the open declaration group's type to `name` in the innermost
    /// frame (one `parse_type` arms the group; every name in it binds). The
    /// bound type folds through a type alias first — the frame's declared
    /// types are the resolved interface types every verdict reads.
    pub(crate) fn bind_declared_type(&mut self, name: StringId) {
        let ty = self.pending_decl_ty.unwrap_or(Ty::UNKNOWN);
        let ty = self.fold_alias_ty(ty);
        if let Some(frame) = self.frames.last_mut() {
            frame.var_types.insert(name, ty);
        }
    }

    /// Binds a formal or catch parameter's own declared type (one arm per
    /// parameter, not shared with a group). The bound type folds through a
    /// type alias first — the frame's declared types are the resolved
    /// interface types every verdict reads. An implicitly typed formal of an
    /// open function literal infers downward from the enclosing invocation's
    /// context type (`function_expression_resolver.dart`
    /// `_inferFormalParameters`: the schema's corresponding parameter type
    /// `K` runs `greatestClosure`; `T <: Null` — an unresolvable head, the
    /// oracle's `InvalidType` — binds `Object?`, a genuinely nullable schema
    /// type binds itself, `dynamic` stays `dynamic` and never infers). Only
    /// a `FunctionType` context supplies any inference; every other slot
    /// keeps `dynamic` — the suppress-everything face `Ty::UNKNOWN` already
    /// carries.
    pub(crate) fn bind_parameter_type(&mut self, name: StringId) {
        let ty = self
            .armed_ty
            .take()
            .or_else(|| self.closure_param_context_ty(name))
            .unwrap_or(Ty::UNKNOWN);
        let ty = self.fold_alias_ty(ty);
        if let Some(frame) = self.frames.last_mut() {
            frame.var_types.insert(name, ty);
        }
    }

    /// The downward-inferred type of an implicitly typed formal inside an
    /// open function literal's own formal list (`(e) => e.foo()` as an
    /// argument): the callee's parameter type at the literal's argument
    /// slot is the context schema; the formal's own schema entry decides
    /// (the positional pair iterates in declaration order, a named formal
    /// pairs by spelling). `None` answers the oracle's "no inference" — a
    /// non-function context, a `dynamic` schema entry, or a callee the
    /// element tables cannot see all leave the formal `dynamic`
    /// (`Ty::UNKNOWN`). The schema-type walk itself is the oracle's
    /// `InvalidType` test inverted: an unresolvable head or type argument
    /// binds `Object?`, like the unindexed SDK receiver whose first type
    /// argument is itself unresolvable (`List<Und>.map((e) => …)` — `E`
    /// *is* that argument).
    fn closure_param_context_ty(&mut self, name: StringId) -> Option<Ty> {
        // The literal's own list: the innermost open capture belongs to
        // this literal's frame and sits inside its own formal list (a
        // function-typed formal's inner parameters report against their own
        // depth and skip).
        let capture = self.closure_caps.last()?;
        if !capture.list_open
            || capture.frame_depth != self.frames.len()
            || capture.list_depth != self.formal_list_depth
        {
            return None;
        }
        // The formal's schema entry: a positional formal pairs by
        // declaration order (the formals already bound in this frame ARE
        // the ordinal — no local can precede a formal), a named one by its
        // spelling.
        let formal = if self.fn_formal_named_section {
            SchemaSlot::Named(name)
        } else {
            SchemaSlot::Positional(self.frames.last()?.var_types.len())
        };
        // The literal's argument slot in the open invocation: completed
        // positional arguments before it, or the pending named label.
        let level = self.arg_levels.last()?;
        let slot = match level.pending_name {
            Some(label) => SchemaSlot::Named(label),
            None => SchemaSlot::Positional(
                level
                    .args
                    .iter()
                    .filter(|(label, ..)| label.is_none())
                    .count(),
            ),
        };
        let callee_row = level.callee_row?;
        let callee = self.rows.get(callee_row)?;
        // An unmerged member placeholder (`xs.map(…)`) classifies its
        // receiver from the row directly below it — the same below-row read
        // the argument check makes; a bare callee's own receiver is the
        // top-level face. A member-access receiver row (`m.values.map(…)`,
        // `holder.items.map(…)`) carries no receiver face of its own — the
        // chained call's receiver is the access *result*, which
        // [`Self::chain_link_receiver`] types.
        let recv = if callee.qualified {
            callee_row
                .checked_sub(1)
                .and_then(|below| self.rows.get(below))
                .map_or(Receiver::Unknown, |below| {
                    let recv = self.argument_receiver(below);
                    if matches!(recv, Receiver::Unknown) {
                        self.chain_link_receiver(below)
                    } else {
                        recv
                    }
                })
        } else {
            Receiver::Unknown
        };
        // The callee's parameter type at the literal's slot is the context
        // type the literal's own schema came from. The element formals
        // answer first; a function-typed variable callee's captured
        // signature closes.
        let callee_name = callee.name;
        let slot_ty = match slot {
            SchemaSlot::Positional(index) => self
                .callee_formals(&recv, callee_name)
                .and_then(|f| f.positional.get(index).map(|f| f.ty)),
            SchemaSlot::Named(label) => self
                .callee_formals(&recv, callee_name)
                .and_then(|f| f.named_formal(label).map(|f| f.ty)),
        }
        .or_else(|| {
            self.fn_var_sigs
                .get(&callee_name)
                .and_then(|sig| match slot {
                    SchemaSlot::Positional(index) => sig.positional.get(index).copied(),
                    SchemaSlot::Named(label) => sig
                        .named
                        .iter()
                        .find(|(n, ..)| *n == label)
                        .map(|(_, ty, _)| *ty),
                })
        });
        // The schema function's own parameter entry: the formal's
        // `greatestClosure` source. The written signature table answers a
        // function-typed formal the element model poisoned (rule 0c); the
        // element slot answers through its alias chain; the unindexed SDK
        // receiver's own first type argument stands in when the callee's
        // formal table is out of reach (`List<Und>.map((e) => …)` — `E`
        // *is* that argument).
        let written = match slot {
            SchemaSlot::Positional(index) => self
                .fn_formal_sigs
                .get(&callee_name)
                .and_then(|table| table.positional.get(index)),
            SchemaSlot::Named(label) => self
                .fn_formal_sigs
                .get(&callee_name)
                .and_then(|table| table.named.get(&label)),
        };
        let schema = match written {
            Some(sig) => Self::schema_entry(sig, formal),
            None => slot_ty
                .and_then(|ty| self.closure_schema_sig(ty))
                .and_then(|sig| Self::schema_entry(&sig, formal))
                .or_else(|| match recv {
                    Receiver::Typed { ty, .. } => {
                        // The stand-in substitutes into a *declared* type's
                        // instantiation. An invalid receiver head — the
                        // oracle's `InvalidType` (an unresolvable
                        // `NodeList<Annotation>`) — has no function type to
                        // read, so no schema entry forms: the formal keeps
                        // `dynamic` and its member reads stay silent. A
                        // provable head keeps the stand-in (`List<Und>`'s
                        // `E` is still that first written argument).
                        let Ty::Named { name: head, .. } = ty else {
                            return None;
                        };
                        if !self.named_type_is_type(head) {
                            return None;
                        }
                        self.receiver_first_type_arg(ty)
                    }
                    _ => None,
                }),
        }?;
        // A schema entry spelling the callee's own inference parameters
        // (`forCorrespondingPairs`'s `T1`/`T2`, `writeList`'s `T`) is not a
        // type the caller's scope can name — the instantiation substitutes
        // the call's solution: explicit type arguments first, then the
        // receiver's written instantiation (a `Box<int>` member's `T`), then
        // the upward unification of the arguments already typed (the
        // send-close solver's inputs, run here at bind time). Only names the
        // callee actually declares as inference parameters can bind — a
        // misspelt head collects no binding and keeps the unresolvable
        // fallback below.
        let schema = self
            .closure_schema_instantiate(schema, &recv, callee_name, callee_row)
            .unwrap_or(schema);
        // `greatestClosure`'s nullable face: `T <: Null` binds `Object?`;
        // an invalid type argument (`List<Und>`'s `E`) collapses to the
        // same; a genuinely nullable schema type binds itself; `dynamic`
        // never infers.
        match schema {
            Ty::Dynamic => None,
            Ty::Unknown => Some(self.object_question()),
            Ty::Null | Ty::Never => Some(self.object_question()),
            Ty::Named {
                name: head,
                nullable,
                args,
            } => {
                if nullable {
                    return Some(self.object_question());
                }
                if self.named_type_is_type(head) && self.args_are_types(args) {
                    return None;
                }
                // The schema entry the formal's inference consumed carries
                // a name the file cannot prove is a type — a declaration's
                // own type parameter (`map`'s `E`, the receiver container's
                // `T`) or a genuinely misspelt head. The oracle substitutes
                // the receiver's instantiated element before answering: a
                // `List<Node>.map((each) => each.copy())` binds `Node`, and
                // a `dynamic` element never infers. The receiver's first
                // written argument stands in for that substitution here —
                // a decidable non-null named argument or a `dynamic` one
                // reads the oracle's silent arm; every other shape keeps
                // the `Object?` recovery (the `Und`-schema pins' verdict —
                // a receiver argument the file cannot name collapses to the
                // same invalid face the unresolved head already carried).
                match recv {
                    Receiver::Typed { ty, .. } => {
                        match self.receiver_first_type_arg(ty) {
                            Some(Ty::Dynamic) => None,
                            Some(Ty::Named {
                                name: arg_head,
                                nullable: false,
                                args: arg_args,
                            }) if self.named_type_is_type(arg_head)
                                && self.args_are_types(arg_args) =>
                            {
                                None
                            }
                            _ => Some(self.object_question()),
                        }
                    }
                    _ => Some(self.object_question()),
                }
            }
            _ => None,
        }
    }

    /// The schema entry with the callee's inference parameters replaced by
    /// this call's solution (`None` = no binding formed — the caller keeps
    /// the unsubstituted schema and its unresolvable-head recovery). The
    /// solver mirrors the send-close inference faces in declaration order:
    /// explicit type arguments fix first, the receiver's written
    /// instantiation seeds the declaring container's own parameters, and the
    /// already-typed positional/named arguments unify upward for the
    /// still-open slots — a parameter solved from exactly one decidable
    /// position (or several agreeing ones) binds; disagreement poisons. The
    /// substituted schema must itself stay decidable in the caller's scope
    /// ([`Self::schema_type_is_decidable`]), else no substitution forms.
    fn closure_schema_instantiate(
        &self,
        ty: Ty,
        recv: &Receiver,
        callee_name: StringId,
        callee_row: usize,
    ) -> Option<Ty> {
        let (params, seeds, ordered) = self.closure_callee_params(recv, callee_name)?;
        if params.is_empty() {
            return None;
        }
        let param_index = |name: StringId| params.iter().position(|(param, _)| *param == name);
        let mut solutions: Vec<Option<Ty>> = vec![None; params.len()];
        // Explicit type arguments fix first — peeked, not consumed (the
        // send-close face owns the slot). A positional list pairs against
        // the declaration's parameter order, which the foreign-declaration
        // recovery cannot vouch for, so those solutions stay open.
        if ordered
            && let Some((_, explicit)) = self
                .call_type_args
                .iter()
                .find(|(row, _)| *row == callee_row)
        {
            for (slot, ty) in solutions.iter_mut().zip(explicit.iter()) {
                *slot = Some(*ty);
            }
        }
        for (name, ty) in &seeds {
            if let Some(index) = param_index(*name)
                && solutions[index].is_none()
            {
                solutions[index] = Some(*ty);
            }
        }
        // Upward face: the arguments already typed at bind time unify
        // against the callee's formals for the still-open slots.
        let formals = self.callee_formals(recv, callee_name)?;
        let level = self.arg_levels.last()?;
        let mut candidates: Vec<(StringId, Ty)> = Vec::new();
        let mut positional_index = 0usize;
        for (name, ty, _literal, _range) in &level.args {
            let formal = match name {
                Some(n) => formals.named_formal(*n),
                None => {
                    let formal = formals.positional.get(positional_index);
                    positional_index += 1;
                    formal
                }
            };
            if let Some(formal) = formal
                && *ty != Ty::Unknown
            {
                self.closure_unify_upward(formal.ty, *ty, &params, &mut candidates);
            }
        }
        let mut poisoned: FxHashSet<usize> = FxHashSet::default();
        for (param, ty) in candidates {
            let Some(index) = param_index(param) else {
                continue;
            };
            if poisoned.contains(&index) {
                continue;
            }
            match solutions[index] {
                None => solutions[index] = Some(ty),
                Some(kept) if kept == ty => {}
                Some(_) => {
                    poisoned.insert(index);
                    solutions[index] = None;
                }
            }
        }
        let map: Vec<(StringId, Ty)> = params
            .iter()
            .zip(solutions.iter())
            .filter_map(|((param, _), solution)| solution.map(|ty| (*param, ty)))
            .collect();
        if map.is_empty() {
            return None;
        }
        let out = self.subst_params(ty, &map)?;
        // A solution that is itself unresolvable (an invalid written type
        // argument, a receiver element the file cannot name) must not
        // become the formal's type: the substituted head would land in the
        // unresolvable-schema recovery and emit the very row the
        // unsubstituted schema never did (`Bag<Und>`'s `T` is a provable
        // same-file type parameter, `Und` is not). Rejection keeps the
        // schema untouched — exactly the pre-existing route.
        (out != ty && self.schema_type_is_decidable(out)).then_some(out)
    }

    /// Whether a substituted schema entry is a type the caller's scope can
    /// name: `dynamic` never infers, and a named type is decidable only
    /// when its head and every written type argument prove as types. Every
    /// other shape (an unresolvable head, an invalid argument, the
    /// bottom/`void` faces) refuses the substitution — the under-report
    /// posture: no invented types.
    fn schema_type_is_decidable(&self, ty: Ty) -> bool {
        match ty {
            Ty::Dynamic => true,
            Ty::Named { name, args, .. } => {
                self.named_type_is_type(name) && self.args_are_types(args)
            }
            _ => false,
        }
    }

    /// The callee's inference parameters and receiver-side seeds at bind
    /// time — the bind-time view of the send-close solver's parameter set.
    /// A member send answers the found method's own type-parameter names
    /// plus the declaring container's captured parameters (seeded by the
    /// receiver's written instantiation for an own member); a same-file
    /// executable reads its `exec_bounds` capture; a function-typed
    /// variable callee reads its written signature's parameters. A foreign
    /// top-level function carries no parameter-name table — its declared
    /// arity gates a leaf-collecting walk over the formal and return types
    /// (bare leaves the caller's scope cannot prove are types, in written
    /// order, capped at the declared count).
    ///
    /// The third element reports whether the returned order is the
    /// declaration's type-parameter order — a positional explicit
    /// type-argument list pairs against it. A foreign top-level
    /// declaration only carries an arity, so its recovered spellings are
    /// written order, not declaration order, and the list pairs against
    /// nothing.
    fn closure_callee_params(
        &self,
        recv: &Receiver,
        callee_name: StringId,
    ) -> Option<(Vec<(StringId, Option<Ty>)>, Vec<(StringId, Ty)>, bool)> {
        // The member send's parameters (mirrors the send-close solver):
        // method type-parameter names are authoritative; the declaring
        // container's own parameters join only for the receiver's own
        // member, seeded by the receiver's written instantiation.
        if !matches!(recv, Receiver::Unknown | Receiver::Prefix { .. }) {
            let (container, recv_ty) = match *recv {
                Receiver::Typed { type_name, ty } => (type_name, Some(ty)),
                Receiver::This { container, .. } => (container, None),
                _ => return None,
            };
            if let Walk::Found(entry) = self.walk_members(container, callee_name)
                && entry.kind == MemberKind::Method
            {
                let own_member = resolve_container(
                    self.graph,
                    self.namespaces,
                    self.index,
                    self.lib,
                    self.file,
                    container,
                )
                .is_some_and(|(c, _)| c.members.get(&callee_name).is_some());
                let mut params: Vec<(StringId, Option<Ty>)> = entry
                    .type_param_names
                    .iter()
                    .map(|name| (*name, None))
                    .collect();
                let container_params = if own_member {
                    self.bounds.container_params(container).unwrap_or_default()
                } else {
                    Vec::new()
                };
                let mut seeds: Vec<(StringId, Ty)> = Vec::new();
                if own_member
                    && let Some(Ty::Named { args, .. }) = recv_ty
                {
                    let pool = self.boundary.pool();
                    let args = pool.get(args);
                    if args.len() == container_params.len() {
                        for ((param, _, _), ty) in container_params.iter().zip(args.iter()) {
                            seeds.push((*param, *ty));
                        }
                    }
                }
                params.extend(
                    container_params
                        .iter()
                        .map(|(param, bound, _)| (*param, bound.as_ref().map(|tree| self.tree_ty(tree)))),
                );
                return Some((params, seeds, true));
            }
            return None;
        }
        // A same-file executable's captured bounds.
        if let Some(params) = self.exec_bounds.get(&callee_name)
            && !params.is_empty()
        {
            return Some((params.to_vec(), Vec::new(), true));
        }
        // A function-typed variable callee's written signature.
        if let Some(sig) = self.fn_var_sigs.get(&callee_name)
            && !sig.params.is_empty()
        {
            return Some((sig.params.to_vec(), Vec::new(), true));
        }
        // A foreign top-level function: `type_params` is a bare count, so
        // the parameter names recover from the declaration's own types —
        // every bare leaf the caller's scope cannot prove is a type, in
        // written order, capped at the declared arity.
        let decl = self.top_level_decl(callee_name)?;
        if decl.kind != MicroKind::TopLevelFn || decl.type_params == 0 {
            return None;
        }
        let arity = usize::from(decl.type_params);
        let mut names: Vec<StringId> = Vec::new();
        if let Some(formals) = decl.formals.as_deref() {
            for formal in formals.positional.iter().chain(formals.named.iter()) {
                self.unprovable_leaves(formal.ty, &mut names, arity);
            }
        }
        self.unprovable_leaves(decl.return_ty, &mut names, arity);
        if names.is_empty() {
            return None;
        }
        Some((
            names.into_iter().map(|name| (name, None)).collect(),
            Vec::new(),
            false,
        ))
    }

    /// Every bare leaf of `ty` the caller's scope cannot prove is a type —
    /// the candidate inference-parameter spellings of a foreign generic
    /// declaration — appended in written order until `cap` names collect.
    /// A misspelt head inside a foreign generic's own types is a real risk
    /// the arity cap contains.
    fn unprovable_leaves(&self, ty: Ty, out: &mut Vec<StringId>, cap: usize) {
        if out.len() >= cap {
            return;
        }
        match ty {
            Ty::Named { name, args, .. } => {
                if args == ArgListId::EMPTY {
                    if !self.named_type_is_type(name) && !out.contains(&name) {
                        out.push(name);
                    }
                    return;
                }
                for arg in self.boundary.pool().get(args) {
                    self.unprovable_leaves(*arg, out, cap);
                }
            }
            Ty::Function { shape, .. } => {
                let pool = self.boundary.pool();
                let shape = pool.function_shape(shape);
                let (positional, named, return_ty) =
                    (shape.positional.clone(), shape.named.clone(), shape.return_ty);
                drop(pool);
                for ty in positional {
                    self.unprovable_leaves(ty, out, cap);
                }
                for (_, ty) in named {
                    self.unprovable_leaves(ty, out, cap);
                }
                self.unprovable_leaves(return_ty, out, cap);
            }
            Ty::Record { shape, .. } => {
                let pool = self.boundary.pool();
                let shape = pool.record_shape(shape);
                let (positional, named) = (shape.positional.clone(), shape.named.clone());
                drop(pool);
                for ty in positional {
                    self.unprovable_leaves(ty, out, cap);
                }
                for (_, ty) in named {
                    self.unprovable_leaves(ty, out, cap);
                }
            }
            _ => {}
        }
    }

    /// The upward-constraint collector for the bind-time solve — the
    /// send-close unifier's structural walk: a bare leaf belonging to the
    /// parameter set binds the whole argument type; a same-head generic
    /// pair binds pairwise through the argument lists; the
    /// `List`/`Set` → `Iterable` builtins edge crosses the one head hop
    /// `Iterable<T>` formals need; `Function` and `Record` pairs bind
    /// positionally. Everything else binds nothing.
    fn closure_unify_upward(
        &self,
        formal: Ty,
        arg: Ty,
        params: &[(StringId, Option<Ty>)],
        out: &mut Vec<(StringId, Ty)>,
    ) {
        match (formal, arg) {
            (
                Ty::Named {
                    name,
                    nullable: false,
                    args,
                },
                _,
            ) if args == ArgListId::EMPTY => {
                if params.iter().any(|(param, _)| *param == name) {
                    out.push((name, arg));
                }
            }
            (
                Ty::Named {
                    name,
                    nullable: false,
                    args: formal_args,
                },
                Ty::Named {
                    name: arg_head,
                    args: arg_args,
                    ..
                },
            ) if formal_args != ArgListId::EMPTY => {
                let pool = self.boundary.args.borrow();
                let (formal_args, arg_args) = (pool.get(formal_args), pool.get(arg_args));
                if name == arg_head && formal_args.len() == arg_args.len() {
                    for (f, a) in formal_args.iter().zip(arg_args.iter()) {
                        self.closure_unify_upward(*f, *a, params, out);
                    }
                    return;
                }
                let iterable = self.intern("Iterable");
                if name == iterable && formal_args.len() == 1 && arg_args.len() == 1 {
                    let head = self.strings.resolve(arg_head);
                    if head == "List" || head == "Set" {
                        self.closure_unify_upward(formal_args[0], arg_args[0], params, out);
                    }
                }
            }
            (
                Ty::Function {
                    shape: formal_shape,
                    nullable: false,
                },
                Ty::Function {
                    shape: arg_shape, ..
                },
            ) => {
                let pool = self.boundary.pool();
                let (formal_shape, arg_shape) = (
                    pool.function_shape(formal_shape),
                    pool.function_shape(arg_shape),
                );
                for (f, a) in formal_shape
                    .positional
                    .iter()
                    .zip(arg_shape.positional.iter())
                {
                    self.closure_unify_upward(*f, *a, params, out);
                }
                self.closure_unify_upward(formal_shape.return_ty, arg_shape.return_ty, params, out);
            }
            (
                Ty::Record {
                    shape: formal_shape,
                    nullable: false,
                },
                Ty::Record {
                    shape: arg_shape, ..
                },
            ) => {
                let pool = self.boundary.pool();
                let (formal_shape, arg_shape) = (
                    pool.record_shape(formal_shape),
                    pool.record_shape(arg_shape),
                );
                for (f, a) in formal_shape
                    .positional
                    .iter()
                    .zip(arg_shape.positional.iter())
                {
                    self.closure_unify_upward(*f, *a, params, out);
                }
            }
            _ => {}
        }
    }

    /// The schema function's parameter entry under `slot` — the type the
    /// oracle's `greatestClosure` consumes for one closure formal.
    fn schema_entry(sig: &FnVarSig, slot: SchemaSlot) -> Option<Ty> {
        match slot {
            SchemaSlot::Positional(index) => sig.positional.get(index).copied(),
            SchemaSlot::Named(label) => sig
                .named
                .iter()
                .find(|(name, ..)| *name == label)
                .map(|(_, ty, _)| *ty),
        }
    }

    /// The `Object?` recovery type (`_typeSystem.objectQuestion`).
    fn object_question(&mut self) -> Ty {
        Ty::Named {
            name: self.intern("Object"),
            nullable: true,
            args: ArgListId::EMPTY,
        }
    }

    /// Every written type argument names a type — the oracle's argument
    /// walk: one unresolvable head makes the whole written type
    /// `InvalidType`.
    fn args_are_types(&self, args: ArgListId) -> bool {
        self.boundary.pool().get(args).iter().all(|arg| match *arg {
            Ty::Named {
                name,
                nullable: _,
                args,
            } => self.named_type_is_type(name) && self.args_are_types(args),
            _ => true,
        })
    }

    /// The full formal table of a callee (an unmerged member placeholder's
    /// receiver resolves the member face; a bare callee reads the
    /// top-level declaration).
    fn callee_formals(&self, recv: &Receiver, name: StringId) -> Option<Formals> {
        match recv {
            Receiver::Unknown | Receiver::Prefix { .. } => self
                .top_level_decl(name)
                .and_then(|d| d.formals.as_deref().cloned()),
            _ => self.member_formals_full(recv, name).map(|f| f.into_owned()),
        }
    }

    /// The written function shape of a type, aliases unfolded — the
    /// argument-slot schema the closure-literal inference reads.
    fn closure_schema_sig(&self, ty: Ty) -> Option<FnVarSig> {
        match self.fold_alias_ty(ty) {
            Ty::Function { shape, .. } => {
                let pool = self.boundary.pool();
                let shape = pool.function_shape(shape);
                Some(FnVarSig {
                    params: Box::default(),
                    required_positional: u16::try_from(shape.positional.len()).unwrap_or(u16::MAX),
                    positional: shape.positional.clone(),
                    named: shape
                        .named
                        .iter()
                        .map(|(name, ty)| (*name, *ty, false))
                        .collect(),
                    return_ty: shape.return_ty,
                })
            }
            Ty::Named { name, .. } => self.alias_fn_sig(name).map(|sig| *sig),
            _ => None,
        }
    }

    /// One written argument subtree as a body-level [`Ty`]: the head
    /// canonicalizes like the annotation head (`dynamic`/`void`/`Null`),
    /// and the subtree interns bottom-up into the boundary pool (#611) so
    /// nested written arguments compare structurally. A poisoned subtree
    /// keeps the name-only shape — its deeper structure is recovery
    /// residue, never a written fact.
    pub(crate) fn tree_ty(&self, tree: &crate::capture::TypeTree) -> Ty {
        match self.strings.resolve(tree.head) {
            "dynamic" => return Ty::Dynamic,
            "void" => return Ty::Void,
            "Null" => return Ty::Null,
            _ => {}
        }
        let args = if tree.poisoned {
            ArgListId::EMPTY
        } else {
            let args: Vec<Ty> = tree.args.iter().map(|arg| self.tree_ty(arg)).collect();
            self.boundary.intern_list(&args)
        };
        Ty::Named {
            name: tree.head,
            nullable: tree.nullable,
            args,
        }
    }

    /// The first written type argument of a declared-typed receiver: the
    /// receiver type's own pooled slot. The retired written-argument side
    /// channel kept this face alive for shapes whose annotation never
    /// interned (poisoned groups, a `dynamic` head) — those shapes now read
    /// argument-less and return `None` here, the same suppression the
    /// channel's own poison rule mandated: recovery residue is never a
    /// written fact.
    pub(crate) fn receiver_first_type_arg(&self, ty: Ty) -> Option<Ty> {
        let Ty::Named { args, .. } = ty else {
            return None;
        };
        self.boundary.pool().get(args).first().copied()
    }

    /// The static face of a chained producer's result (`m.values.map(…)`,
    /// `holder.items.map(…)`): a member-access row carries no receiver face
    /// of its own ([`Self::argument_receiver`] answers `Unknown`), but the
    /// chained call's closure literal binds from the access *result*. The
    /// upstream static type of `e.member` is the getter's declared return
    /// instantiated with the receiver's written type arguments (upstream
    /// Dart SDK source, pkg/analyzer/lib/src/dart/resolver/
    /// property_element_resolver.dart `_executeGetter`); this answers it
    /// only when the result is a provable named type — every other shape
    /// keeps the chain's `Unknown` suppression.
    fn chain_link_receiver(&self, row: &Row) -> Receiver {
        let RowShape::Member {
            recv: Receiver::Typed { type_name, ty },
            ..
        } = &row.shape
        else {
            return Receiver::Unknown;
        };
        // An indexed container's own face walks; the getter's declared
        // return instantiates with the receiver's written arguments (the
        // anchor's own parameters — an inherited entry's environment is the
        // supertype's, and a wrong substitution must not form: leftover
        // unprovable names fail the decidability gate below).
        if let Walk::Found(entry) = self.walk_members(*type_name, row.name)
            && entry.kind == MemberKind::Getter
        {
            let ret = self.fold_alias_ty(entry.return_ty);
            let Ty::Named { name, nullable, .. } = ret else {
                return Receiver::Unknown;
            };
            let ret = match self.bounds.container_params(*type_name) {
                Some(params) if !params.is_empty() => {
                    let Ty::Named { args: recv_args, .. } = *ty else {
                        return Receiver::Unknown;
                    };
                    let recv_args = self.boundary.pool().get(recv_args).to_vec();
                    if recv_args.len() != params.len() {
                        return Receiver::Unknown;
                    }
                    let map: Vec<(StringId, Ty)> = params
                        .iter()
                        .zip(recv_args.iter())
                        .map(|((param, ..), arg)| (*param, *arg))
                        .collect();
                    match self.subst_params(ret, &map) {
                        Some(ret) => ret,
                        None => return Receiver::Unknown,
                    }
                }
                _ => ret,
            };
            // A nullable result head (`Node? get best`) is no iterable face
            // to chain from; a provable non-null named head is the access
            // result's own receiver.
            let Ty::Named { args, .. } = ret else {
                return Receiver::Unknown;
            };
            return if nullable
                || !self.named_type_is_type(name)
                || !self.args_are_types(args)
            {
                Receiver::Unknown
            } else {
                Receiver::Typed {
                    type_name: name,
                    ty: ret,
                }
            };
        }
        // The SDK's unindexed units chain-break, so the frozen producer
        // table stands in: the declared `Iterable<E>` face reads its
        // element from the receiver's written type argument at the row's
        // index (the [`CORE_STATIC_RETURNS`] precedent for an unindexed
        // declared return).
        let head = self.strings.resolve(*type_name);
        let Some((_, _, index, result)) = CHAIN_PRODUCER_GETTERS.iter().find(
            |(container, getter, ..)| *container == head && self.intern(getter) == row.name,
        ) else {
            return Receiver::Unknown;
        };
        let Ty::Named { args, .. } = *ty else {
            return Receiver::Unknown;
        };
        let Some(element) = self.boundary.pool().get(args).get(*index).copied() else {
            return Receiver::Unknown;
        };
        if !self.schema_type_is_decidable(element) {
            return Receiver::Unknown;
        }
        Receiver::Typed {
            type_name: self.intern(result),
            ty: Ty::Named {
                name: self.intern(result),
                nullable: false,
                args: self.boundary.intern_list(&[element]),
            },
        }
    }

    /// A container head: the member face and the `this`/`super` receiver for
    /// its body. An unnamed extension gets a placeholder face name — its
    /// own-member face is its application face's member map, so the frame
    /// latches that face's index: the pool is frozen by walk time (one face
    /// per extension declaration, appended in declaration order at
    /// `collect`'s `close_container`), and the walk opens container heads in
    /// the same order, so the count of unnamed-extension frames opened so
    /// far IS the declaration's rank among the nameless faces. A recovery
    /// shape that drops the container degrades safely: the count desyncs
    /// only on that broken input, an out-of-range rank answers a clean
    /// miss, and the bare name falls through exactly as a `Walk::Missing`
    /// walk would (#1701).
    pub(crate) fn push_container_frame(&mut self, kind: ContainerKind, name: Option<StringId>) {
        let extension_face = match (kind, name) {
            (ContainerKind::Extension, None) => {
                let face = self.unnamed_extension_faces_opened;
                self.unnamed_extension_faces_opened += 1;
                Some(face)
            }
            _ => None,
        };
        self.push_frame(FrameKind::Container {
            name: name.unwrap_or_else(|| self.intern("")),
            kind,
            extension_face,
        });
    }

    /// An executable head: formals and its own type parameters live inside.
    /// The return-type span is deliberately NOT cleared here: it arms at
    /// the head's type events, and a closure nested in a later default
    /// value must not drop it before the declaration's end check.
    pub(crate) fn push_executable_frame(&mut self, instance: bool) {
        self.push_frame(FrameKind::Executable {
            instance,
            ret: None,
            super_invoked: false,
            body_brace: None,
            is_async: false,
            generator: false,
        });
    }

    /// Latches the body's async modifier onto the innermost executable
    /// frame (`handle_async_modifier`): the completion warning's generator
    /// and async-legality gates read these bits (`checkForBodyMayComplete
    /// Normally`'s `body.isGenerator` / `body.isAsynchronous` arms). The
    /// modifier event belongs to the body whose frame is innermost — a
    /// nested function literal latches its own frame.
    pub(crate) fn note_async_modifier(&mut self, is_async: bool, generator: bool) {
        if let Some(frame) = self.frames.last_mut()
            && let FrameKind::Executable {
                is_async: slot_async,
                generator: slot_generator,
                ..
            } = &mut frame.kind
        {
            *slot_async = is_async;
            *slot_generator = generator;
        }
    }

    /// The executable body's opening `{` while the innermost frame is the
    /// executable being parsed: first brace wins (the body brace is the
    /// first block-function-body event a frame sees — a nested closure's
    /// body latches the closure's own frame).
    pub(crate) fn note_body_brace(&mut self, brace: SourceRange) {
        if let Some(frame) = self.frames.last_mut()
            && let FrameKind::Executable { body_brace, .. } = &mut frame.kind
            && body_brace.is_none()
        {
            *body_brace = Some(brace);
        }
    }

    /// The executable declaration's completion staging
    /// (`end_method`/`end_top_level_method`, before the frame pops): the
    /// declared return type, the block-body `{` gate keyed by the end
    /// token — the exact span the flow layer's `BodyMayCompleteNormally`
    /// fact carries — and the declaring name's span (the completion rows'
    /// anchor) stage for the teardown's fold to join
    /// (`body_might_complete_normally_nullable`). Setters stay out (the
    /// official `!node.isSetter` gate), and frames without a latched return
    /// type (constructors, closures) under-report. The
    /// modifier gates front-run the staging: a generator body always
    /// completes normally (`body.isGenerator` early return,
    /// type_analyzer.dart:658-660), and an `async` body under an illegal
    /// return type already reports its own face — no second row here
    /// (:661-681, the `_isLegalReturnType` shape).
    pub(crate) fn stage_body_completion(&mut self, get_or_set: Option<Token>, end: Token) {
        if get_or_set.and_then(|t| self.text_of(t)) == Some("set") {
            return;
        }
        let Some(Frame {
            kind:
                FrameKind::Executable {
                    ret: Some(exec),
                    body_brace,
                    is_async,
                    generator,
                    ..
                },
            ..
        }) = self.frames.last()
        else {
            return;
        };
        if *generator || (*is_async && !self.legal_async_return(exec.ty)) {
            return;
        }
        let row = (end.range, exec.ty, *body_brace, *is_async, exec.name_span);
        match self
            .body_completions
            .binary_search_by(|(r, ..)| r.offset.cmp(&row.0.offset))
        {
            // A diverging repeat (two executables sharing one end token)
            // leaves no single row that stands for the span — poisoned, the
            // join misses (the [`PromotionStaging`] rule).
            Ok(index) => {
                self.body_completions.remove(index);
            }
            Err(index) => self.body_completions.insert(index, row),
        }
    }

    /// The name-level `_isLegalReturnType` for an `async` body: the top
    /// types read true through the official `Future<Never> <: T` lower-bound
    /// check (`dynamic`, `void`, and the inferred stand-in this model uses
    /// for an absent declaration), plus the `Future`/`FutureOr` family; any
    /// other head is the illegal-async-return-type face's own business, and
    /// the completion warning spares it (type_analyzer.dart:661-681).
    pub(crate) fn legal_async_return(&self, ty: Ty) -> bool {
        match ty {
            Ty::Unknown | Ty::Dynamic | Ty::Void => true,
            // `Future<Never> <: Never` does not hold (the bottom type is no
            // `Future`), so a declared `Never` stays illegal. A record or
            // function type is no `Future`/`FutureOr` either.
            Ty::Null | Ty::Never | Ty::Record { .. } | Ty::Function { .. } => false,
            Ty::Named { name, .. } => {
                name == self.intern("Future") || name == self.intern("FutureOr")
            }
        }
    }

    /// The `body_might_complete_normally[_nullable]` verdict for the
    /// executable whose body ends at exactly `end`
    /// (`checkForBodyMayCompleteNormally`, type_analyzer.dart:639-704):
    /// `Some((code, return type, declaring name span))` when a diagnostic
    /// fires — the oracle's `atSourceRange` anchors every staged (named)
    /// shape at its declaring name (probed 3.13.1: top-level function,
    /// method, static member, getter, local function, factory compound
    /// name; the block `{` is the closure literal's anchor, and closures
    /// stage no return) —
    /// `None` when no method/top-level staging stood for the end token, the
    /// body is not a block (the official reports `BlockFunctionBody`s
    /// only), or the branch order stays silent. The order, in the official
    /// shape: (a) a potentially non-nullable return type reports the
    /// COMPILE_TIME_ERROR twin (`body_might_complete_normally`,
    /// `isPotentiallyNonNullable`'s pick at type_analyzer.dart:688); (b) a
    /// nullable return type whose `FutureOr` base is
    /// `dynamic`/invalid/unknown/`void`/`Null` stays silent; (c) else the
    /// nullable warning carries the return type's display text.
    pub(crate) fn body_may_complete_report(
        &self,
        end: SourceRange,
    ) -> Option<(SemanticCode, Ty, SourceRange)> {
        let index = self
            .body_completions
            .binary_search_by(|(r, ..)| r.offset.cmp(&end.offset))
            .ok()?;
        let (staged, declared, brace, is_async, name) = &self.body_completions[index];
        if *staged != end {
            return None;
        }
        // The block-body gate only: the official `BlockFunctionBody`s-only
        // face; the row's anchor is the declaring name, not the `{`.
        (*brace)?;
        // An `async` body's completion type is the declared return's
        // future-flattened form (`TypeSystem.flatten`,
        // type_analyzer.dart:682-704): `Future<void>` flattens to `void`
        // (silent), `Future<T>`/`FutureOr<T>` to `flatten(T)`.
        let ret = if *is_async {
            self.flatten_future(*declared)
        } else {
            *declared
        };
        let code = match ret {
            // (b)'s top faces: each is its own `FutureOr` base.
            Ty::Unknown | Ty::Null | Ty::Dynamic | Ty::Void => return None,
            // (a): a non-nullable name, record, function, or `Never` (the
            // bottom type declares no null) is potentially non-nullable —
            // the ERROR twin reports. A named head that resolves to no
            // type is `InvalidType` upstream (`_resolveToElement`), whose
            // nullability question never reaches this walk — the same
            // gate [`Self::named_type_is_type`] applies at the return
            // matrix.
            Ty::Never => SemanticCode::BodyMightCompleteNormally,
            // A non-nullable `FutureOr<void>`/`FutureOr<dynamic>` head is
            // not potentially non-nullable (the union's void/dynamic base
            // admits a normal completion), and it is not a `?`-spelled
            // nullable either — both twins stay silent. The same
            // `future_or_base` read the nullable arm below applies.
            Ty::Named {
                name,
                nullable: false,
                ..
            } if name == self.intern("FutureOr")
                && matches!(
                    self.future_or_base(ret),
                    Ty::Unknown | Ty::Null | Ty::Dynamic | Ty::Void
                ) =>
            {
                return None;
            }
            Ty::Named {
                name,
                nullable: false,
                ..
            } if self.named_type_is_type(name) => SemanticCode::BodyMightCompleteNormally,
            Ty::Record {
                nullable: false, ..
            }
            | Ty::Function {
                nullable: false, ..
            } => SemanticCode::BodyMightCompleteNormally,
            Ty::Named { nullable: true, .. }
            | Ty::Record { nullable: true, .. }
            | Ty::Function { nullable: true, .. } => {
                if matches!(
                    self.future_or_base(ret),
                    // (b): the base is one of the silent faces.
                    Ty::Unknown | Ty::Null | Ty::Dynamic | Ty::Void // (c): the warning.
                ) {
                    return None;
                }
                SemanticCode::BodyMightCompleteNormallyNullable
            }
            // An unresolved named head (`InvalidType` upstream) asks no
            // nullability question — silent.
            _ => return None,
        };
        Some((code, ret, *name))
    }

    /// `TypeSystem::flatten` at name level for an `async` body's declared
    /// return: `Future<T>`/`FutureOr<T>` unwrap to `flatten(T)`, `dynamic`
    /// stays `dynamic`, every other type is its own flatten. A bare
    /// `Future`/`FutureOr` spells `Future<dynamic>` — the implicit
    /// argument unwraps to `dynamic` (`Future m1() async {}` completes
    /// with null legally, so its body check stays silent).
    pub(crate) fn flatten_future(&self, ty: Ty) -> Ty {
        let Ty::Named { name, args, .. } = ty else {
            return ty;
        };
        if name != self.intern("Future") && name != self.intern("FutureOr") {
            return ty;
        }
        if args == ArgListId::EMPTY {
            return Ty::Dynamic;
        }
        let pool = self.boundary.pool();
        let list = pool.get(args);
        if list.len() != 1 {
            return ty;
        }
        self.flatten_future(list[0])
    }

    /// `TypeSystem::futureOrBase` at name level: a written `FutureOr<T>`
    /// with exactly one type argument unwraps to `T`, every other type is
    /// its own base (the written `?` never strips — `FutureOr<int>?`'s
    /// base is `int`). A bare `FutureOr` spells `FutureOr<dynamic>` — its
    /// base is `dynamic`, one of the silent faces.
    pub(crate) fn future_or_base(&self, ret: Ty) -> Ty {
        let Ty::Named { name, args, .. } = ret else {
            return ret;
        };
        if name != self.intern("FutureOr") {
            return ret;
        }
        if args == ArgListId::EMPTY {
            return Ty::Dynamic;
        }
        let pool = self.boundary.pool();
        let list = pool.get(args);
        if list.len() != 1 {
            return ret;
        }
        list[0]
    }

    /// A `super` expression while the innermost executable frame is open:
    /// latches that frame's `invokesSuperSelf` face (nested function
    /// literals carry their own frame, so their supers never satisfy the
    /// enclosing method's gate).
    pub(crate) fn note_super_expression(&mut self) {
        if let Some(frame) = self.frames.last_mut()
            && let FrameKind::Executable { super_invoked, .. } = &mut frame.kind
        {
            *super_invoked = true;
        }
    }

    /// The innermost executable frame's `invokesSuperSelf` face (`None`
    /// outside any executable).
    pub(crate) fn top_executable_super_invoked(&self) -> Option<bool> {
        self.frames.last().and_then(|f| match &f.kind {
            FrameKind::Executable { super_invoked, .. } => Some(*super_invoked),
            _ => None,
        })
    }

    /// Latches the armed declared type as the innermost executable's return
    /// type (`begin_method`/`begin_top_level_method` fire before the return
    /// type's `parse_type`; the name identifier fires after it).
    pub(crate) fn latch_return_type(
        &mut self,
        host: ExecKind,
        name: StringId,
        name_span: SourceRange,
    ) {
        let ty = self.armed_ty.take().unwrap_or(Ty::UNKNOWN);
        if let Some(frame) = self.frames.last_mut()
            && let FrameKind::Executable { ret, .. } = &mut frame.kind
            && ret.is_none()
        {
            *ret = Some(ExecReturn {
                ty,
                host,
                name,
                name_span,
                ctor: false,
            });
        }
    }

    /// Arms the written return-type span when a type head lands in the
    /// declaration's return-type position: the innermost frame is the
    /// executable being declared and its return type is not latched yet
    /// (the name event latches it — every later type of the declaration is
    /// a formal's). Cleared at the declaration's end
    /// ([`Engine::check_setter_return`]) and by the frame events.
    pub(crate) fn arm_return_span(&mut self, span: SourceRange) {
        let in_return_position = self
            .frames
            .last()
            .is_some_and(|frame| matches!(&frame.kind, FrameKind::Executable { ret: None, .. }));
        // Only the return-position completion writes the slot: once `ret`
        // latches (at the name token) every later completion is a formal's
        // or a nested type's — it must not clobber the annotation span the
        // setter face reads (`int set s(int v)`: the formal `int`
        // completes after `ret` latches, and wiping the slot would lose
        // the `non_void_return_for_setter` row).
        if in_return_position {
            self.open_return_span = Some(span);
        }
    }

    /// The `non_void_return_for_setter` face (error_verifier.dart:7015-7025,
    /// `visitMethodDeclaration`'s setter arm): a written return annotation
    /// that is not `void` reports at the annotation; an absent one reports
    /// nothing (the oracle's `returnType == null` early-out) and an
    /// unresolved annotation's latched type is unknown — still not `void`.
    /// Consumed at the declaration's end, before the frame pops.
    pub(crate) fn check_setter_return(&mut self, get_or_set: Option<Token>) {
        let span = self.open_return_span.take();
        if get_or_set.and_then(|t| self.text_of(t)) != Some("set") {
            return;
        }
        let Some(span) = span else {
            return;
        };
        let ret = self
            .frames
            .last()
            .map_or(Ty::UNKNOWN, |frame| match &frame.kind {
                FrameKind::Executable { ret, .. } => ret.as_ref().map_or(Ty::UNKNOWN, |r| r.ty),
                _ => Ty::UNKNOWN,
            });
        if ret != Ty::Void {
            self.emit(SemanticCode::NonVoidReturnForSetter, span, "", "");
        }
    }

    /// A factory's return type is its container (`error_verifier.dart`'s
    /// declared-return face: the constructor element's return type is the
    /// enclosing class, whatever the head's recovery shape), so the `=>`
    /// body runs the return check even on `factory new() => 0;`. Latched at
    /// the factory's name identifier; never touches the armed annotation
    /// slot (a factory head parses no return type). The host is the
    /// constructor wording (`returnOfInvalidTypeFromConstructor`: "can't
    /// be returned from the constructor 'E'") — a factory is a constructor
    /// element, never a method.
    pub(crate) fn latch_factory_return(&mut self, container: StringId, name_span: SourceRange) {
        if let Some(frame) = self.frames.last_mut()
            && let FrameKind::Executable { ret, .. } = &mut frame.kind
            && ret.is_none()
        {
            *ret = Some(ExecReturn {
                ty: Ty::named(container),
                host: ExecKind::Constructor,
                name: container,
                name_span,
                ctor: true,
            });
        }
    }

    /// A statement block, catch clause, for body, or typedef scope.
    pub(crate) fn push_block_frame(&mut self) {
        self.push_frame(FrameKind::Block);
    }

    /// Enters a type parameter's scope. A declaration whose frame is already
    /// open (a method's own `<T>` parses after its return type; function
    /// expressions open before theirs) joins the frame's `type_params`
    /// directly — the frame's `type_mark` truncates it on pop — and
    /// re-scopes the signature type rows that resolved `Missing` before the
    /// parameter existed. Container headers parse their variables before
    /// `begin_*_declaration`, so those park for the frame that owns them.
    pub(crate) fn push_type_parameter(&mut self, name: StringId) {
        let Some(mark) = self.frames.last().map(|f| f.row_mark) else {
            self.pending_type_params.push(name);
            return;
        };
        self.type_params.push(name);
        // The signature rows pushed since the frame opened (`T m<T>(...)`'s
        // return type) never saw this parameter: a still-missing type use of
        // the same name was the parameter all along, not an undefined class.
        for row in &mut self.rows[mark..] {
            if row.name == name
                && matches!(row.shape, RowShape::TypeUse(TypeVerdict::Missing, false))
            {
                row.shape = RowShape::Value;
            }
        }
    }
}
