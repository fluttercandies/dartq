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
pub(crate) enum SchemaSlot {
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
    ///
    /// The site prefers `open_ctor_site` over `open_exec_site`: the
    /// constructor's begin event arms the ctor site unconditionally (the
    /// primary-constructor header — `class C(int x)` — never runs a
    /// method-declaration name event, so `open_exec_site` stays unset
    /// there), while the secondary faces arrive at the same site through
    /// either field and a non-constructor context never arms
    /// `open_ctor_site`.
    pub(crate) fn note_parameter(&mut self, name: StringId, range: SourceRange) {
        let site = self.open_ctor_site.or(self.open_exec_site);
        self.add_tracked_local(name, range, LocalKind::Parameter { site });
        if let Some(site) = site {
            self.site_params
                .entry(site)
                .or_default()
                .push((name, range));
        }
    }

    /// A catch clause parameter: the slot it fills decides the kind — the
    /// parser's comma fact ([`Engine::catch_trace_slot`], flipped on the
    /// written comma), never the arrival order of the events: a recovery
    /// shape whose exception name is synthetic (`catch (var e)`) lets the
    /// real `e` reach the trace ensure while the official formals list
    /// still reads it as `catchParameters[0]` — the exception. The
    /// exception's auto-use bit patches in at `handle_catch_block` when
    /// both facts are known.
    pub(crate) fn note_catch_parameter(&mut self, name: StringId, range: SourceRange) {
        let kind = if self.catch_trace_slot {
            LocalKind::CatchStack
        } else {
            LocalKind::CatchException { auto: false }
        };
        self.catch_params += 1;
        self.add_tracked_local(name, range, kind);
    }

    /// `end_formal_parameters` for the catch group's re-parse: the official
    /// AST reads the catch faces off the formals list
    /// (`handleCatchBlock` — exception = `catchParameters[0].name`,
    /// stack trace = `catchParameters[1].name`), so the re-parse's second
    /// real formal name is the trace slot's own binding. The fast pass's
    /// real trace name already carries the stack face; a junk-filled slot
    /// (`{st}`, `[s0, s1]` — only the nameless synthetic row) hands the
    /// face to that second name — the named-section/optional member the
    /// re-parse recovered is the oracle's stack trace variable — and the
    /// nameless ghost row retires into a silent parameter. A junk keyword
    /// the re-parse consumed as its own second name (`catch (e, do …`) is
    /// the slot's junk itself, never a recovered binding: the ghost keeps
    /// the face. The flip never firing (a synthetic comma) routes to the
    /// typed-catch handoff below.
    ///
    /// Every close also zeroes the catch rows' use counts: the formal
    /// re-parse resolves its own tokens against the live scope, and a
    /// fast-pass name re-read in type position (`catch (e, s l:` — the
    /// junk tail parses `s` as the next formal's type) credits a phantom
    /// use to the catch row. The official pipeline resolves uses on the
    /// final AST alone — where the re-parse's faces are the parameter
    /// names, never type annotations — so the group's close drops them;
    /// the body's real reads land only after this point.
    pub(crate) fn reconcile_catch_slots(
        &mut self,
        exc_row: Option<usize>,
        trace_row: Option<usize>,
    ) {
        let Some(&mark) = self.frame_local_marks.last() else {
            return;
        };
        for decl in &mut self.locals[mark..] {
            if matches!(
                decl.kind,
                LocalKind::CatchException { .. } | LocalKind::CatchStack
            ) {
                decl.uses = 0;
            }
        }
        if !self.catch_trace_slot {
            // The typed-catch recovery (`catch (int e)`): the scan's
            // synthetic comma is real for the OFFICIAL rewriter — it stays
            // in the stream when `parseFormalParameters` re-enters, so the
            // official formals list reads `[int, e]` and
            // `handleCatchBlock` names `int` the exception and `e` the
            // stack trace (`catchParameters[0]/[1].name`). Our re-parse
            // walks the raw pair, where `int` armed the twin formal's
            // WRITTEN TYPE — the same fact read the other way: when the
            // re-parse's first formal carries a written type spelled by
            // the scan's first real name, and the scan ensured a second
            // real name, that second name takes the trace face. The
            // type-name row keeps the exception face (a plain catch
            // auto-uses it at `finish_catch_clause`; a sibling presence
            // auto-uses it through the CatchStack check). A first slot the
            // re-parse consumed as a MODIFIER (`catch (var e)` — `var`
            // arms no written type; the official list collapses to `[e]`)
            // leaves the rows untouched: both scan rows keep the exception
            // face and stay silent.
            let twin_ty = exc_row
                .filter(|&row| matches!(self.locals[row].kind, LocalKind::Parameter { .. }))
                .and_then(|row| {
                    let twin = self.locals[row].name;
                    self.frames
                        .last()
                        .and_then(|frame| frame.var_types.get(&twin))
                        .copied()
                });
            let slots = self.catch_scan_slots(mark);
            if let (
                Some(Ty::Named {
                    name: head,
                    nullable: false,
                    args: ArgListId::EMPTY,
                }),
                Some((first, second)),
            ) = (twin_ty, slots)
                && !self.is_nameless(self.locals[first].name)
                && !self.is_nameless(self.locals[second].name)
                && head == self.locals[first].name
            {
                self.locals[second].kind = LocalKind::CatchStack;
            }
            return;
        }
        let mut ghost_offset: Option<u32> = None;
        let mut ghost_rows: Vec<usize> = Vec::new();
        for (i, decl) in self.locals[mark..].iter().enumerate() {
            if matches!(decl.kind, LocalKind::CatchStack) {
                if self.is_nameless(decl.name) {
                    ghost_rows.push(mark + i);
                    ghost_offset = Some(decl.range.offset);
                } else {
                    // The fast pass bound a real trace name — its row
                    // carries the face and the re-parse's names are its
                    // twin and junk tail.
                    return;
                }
            }
        }
        let Some(row) = trace_row else {
            return;
        };
        let range = self.locals[row].range;
        if ghost_offset.is_some_and(|offset| offset == range.offset) {
            return;
        }
        self.locals[row].kind = LocalKind::CatchStack;
        // The silent-parameter kind never emits (`close_frame_locals`'s
        // parameter arm).
        for &i in &ghost_rows {
            self.locals[i].kind = LocalKind::Parameter { site: None };
        }
    }

    /// The catch frame window's scan-slot rows — the two `CatchException`
    /// rows the fast pass ensured — as `(first, second)` when exactly two
    /// exist. A third row (recovery residue) is not a scan shape.
    fn catch_scan_slots(&self, mark: usize) -> Option<(usize, usize)> {
        let mut slots = self.locals[mark..]
            .iter()
            .enumerate()
            .filter(|(_, decl)| matches!(decl.kind, LocalKind::CatchException { .. }))
            .map(|(i, _)| mark + i);
        let first = slots.next()?;
        let second = slots.next()?;
        slots.next().is_none().then_some((first, second))
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
                retired: false,
            });
        }
    }

    /// `begin_import`: one site per `import` directive whose URI starts
    /// with a `String` token — the `unused_import` pass aligns `imports`
    /// positionally against the library's import edges, and an import
    /// element exists upstream for every written directive whatever its
    /// URI shape (the interpolation arm still contributes a
    /// `libraryImports[index]` and an edge with no target;
    /// `imports_verifier.dart:232+` iterates the directive list, so a
    /// dropped site here would misalign every following import against
    /// the engine's per-directive `ImportEdge`, `file_state.dart`'s
    /// `LibraryImport` construction).
    ///
    /// Only an honestly quoted single token carries `uri_text`: a string
    /// interpolation's first segment is a `String` token whose lexeme is
    /// an opening fragment (`'a` of `'a${b}c.dart'`, or `'$` of
    /// `'${x}.dart'`) that fails the closing-quote check, so the site
    /// joins with an empty `uri_text` — the upstream
    /// `stringValue == null` / `DirectiveUriWithoutString` face
    /// (`uri_with_interpolation`, `ast.dart:34082` + `file_state.dart:765`),
    /// which never resolves and never reports on the import side.
    /// `uri_range` still anchors at the first URI token so
    /// `directive_range_around` can walk back to the `import` keyword.
    /// A non-string URI head (`import 5;`) carries no site at all — the
    /// same under-report posture as before, matching the directive walk's
    /// own non-string URI arm.
    #[expect(clippy::cast_possible_truncation)] // span arithmetic narrows under the scanner u32 offset cap
    pub(crate) fn begin_import_site(&mut self, import_kw: Token) {
        let Some(uri) = self.tokens.get(import_kw.id as usize + 1) else {
            return;
        };
        if uri.kind != dartq_scanner::TokenKind::String {
            return;
        }
        let raw = &self.source[uri.byte_range()];
        let mut site = ImportSite {
            uri_text: String::new(),
            uri_range: SourceRange {
                offset: uri.utf16_offset,
                length: raw.chars().map(char::len_utf16).sum::<usize>() as u32,
            },
            prefix: None,
            shown: Vec::new(),
        };
        let bytes = raw.as_bytes();
        if bytes.len() >= 2 {
            let quote = bytes[0];
            if (quote == b'\'' || quote == b'"') && bytes[bytes.len() - 1] == quote {
                site.uri_text = raw[1..raw.len() - 1].to_string();
            }
        }
        self.imports.push(site);
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
    /// it — the sibling's presence reads off the clause's rows: a
    /// `CatchStack` row exists exactly when the trace slot filled, be it
    /// with a real name or the nameless synthetic), the clause joins the
    /// try's clause list, and the catch frame closes — the parameters
    /// leave scope only after the body.
    pub(crate) fn finish_catch_clause(&mut self) {
        let mark = self.frame_local_marks.last().copied();
        let auto = (self.catch_on_type.is_none() && !self.catch_on_record)
            || mark.is_some_and(|mark| {
                self.locals[mark..]
                    .iter()
                    .any(|d| matches!(d.kind, LocalKind::CatchStack))
            });
        if let Some(mark) = mark {
            for decl in &mut self.locals[mark..] {
                if let LocalKind::CatchException { auto: slot } = &mut decl.kind {
                    *slot = auto;
                }
            }
        }
        self.catch_params = 0;
        let record_head = std::mem::take(&mut self.catch_on_record);
        let Some(start) = self.catch_clause_start.take() else {
            self.catch_on_type = None;
            self.catch_on_nullable = None;
            self.catch_head_end = None;
            // The `on`-type miss offsets stay: pass 2 resolves the TypeUse
            // rows after the clauses closed, and each offset is consumed
            // by exact match.
            return;
        };
        // A record-typed (or otherwise paren-headed) `on` head resolves to
        // a null/record `exceptionType` upstream — never `isDartCoreObject`
        // — so the clause joins the list as the catch-all face (bare
        // `catch`), which is what makes the FOLLOWING clauses report
        // `dead_code_catch_following_catch`. The head's own name references
        // (a valid record type's field types) already ran their rows; they
        // must not turn the clause into a named on-clause.
        let on_type = self.catch_on_type.take().filter(|_| !record_head);
        let on_nullable = self.catch_on_nullable.take().unwrap_or(false);
        // A clause whose body block never materialized (`on X;` —
        // `expected_body`) ends at its head's recovery boundary, not at a
        // block end that never came.
        let end = self.last_block_end.unwrap_or(SourceRange {
            offset: start.offset,
            length: self
                .catch_head_end
                .take()
                .unwrap_or(start.offset)
                .saturating_sub(start.offset),
        });
        if let Some(clauses) = self.try_clauses.last_mut() {
            clauses.push(CatchClauseFact {
                on_type,
                on_nullable,
                start,
                end,
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
    ///
    /// Two faces the upstream text pins exactly:
    ///
    /// - The catch-all test reads the RESOLVED type:
    ///   `exceptionType?.type == null || ....isDartCoreObject`, and
    ///   `isDartCoreObject` is `Object` with the none nullability suffix. So
    ///   `on Object?` is NOT catch-all — it takes the subtype arm and every
    ///   later `on T` reports `dead_code_on_catch_subtype` (`T <: Object?`
    ///   is the right-top rule, subtype.dart:52-56), never a
    ///   `catch_following_catch` row anchored at the next clause.
    /// - The subtype arm asks the written types. An unresolved `on` head is
    ///   the official `InvalidType`, which answers `true` from either side
    ///   of `isSubtypeOf` (subtype.dart:274-276, the 0c rule), so the
    ///   oracle's `'InvalidType' is a subtype of 'T'` rows stand; the name
    ///   lattice keeps that face (`Ty::Unknown` maps to the nullable top).
    pub(crate) fn finish_try_statement(&mut self) {
        let Some(clauses) = self.try_clauses.pop() else {
            return;
        };
        let object = self.intern("Object");
        let mut visited: Vec<Ty> = Vec::new();
        for (index, clause) in clauses.iter().enumerate() {
            let catches_all = match clause.on_type {
                None => true,
                Some(name) => name == object && !clause.on_nullable,
            };
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
    /// with the operator span, beside the right operand's first-token
    /// offset (its node range's start — the first token after the
    /// operator).
    pub(crate) fn begin_if_null(&mut self, op_range: SourceRange, rhs_offset: u32) {
        self.if_null_groups
            .push((self.rows.len(), op_range, rhs_offset));
    }

    /// The dead-coalesce operand's never-null verdict
    /// (`_checkForDeadNullCoalesce`'s operand face, error_verifier.dart
    /// :4733-4739): the operand's static type as the oracle reads it —
    /// `node.leftOperand.staticType` for `??`, the target's
    /// `ReadResolutionImpl.type` for `??=`.
    ///
    /// The type is a leading-dot shorthand's context-type member
    /// ([`Self::dot_shorthand_ty`]) or the operand's re-derived static type
    /// (a member's declared accessor type, a type literal's `Type`, an
    /// invoked callee's declared return, a bare read's declared face —
    /// `Self::operand_static_ty`).
    ///
    /// `widen_null_aware` is the `??` face's reading: the operand's
    /// `staticType` carries the null-aware join the member merge could not
    /// spell (`c?.m1 ?? d` reads `double?` and stays silent). The `??=`
    /// face reads the target's read RESOLUTION, which is the access's own
    /// type WITHOUT that join (`c?.m1 ??= 42` reports over a non-nullable
    /// `m1`: the official `visitIfNullAssignment` reads `target.read`).
    ///
    /// An invalid-typed operand suspends the verdict (the official
    /// `isStrictlyNonNullable(InvalidType)` answers nothing — an undefined
    /// class's field never anchors the dead face), a type parameter decides
    /// through its written bound ([`Self::strictly_non_nullable`]), and a
    /// `super` receiver's read is never null (the official AST arm).
    ///
    /// The flow-promoted face (`if (a is Object) a ?? c`, `x ??= 1; x ??= 2`)
    /// is NOT read here: the promotion table only materializes at the
    /// teardown join (`PromotionStaging` — the same feed the deferred
    /// `unnecessary_non_null_assertion` re-decision consumes), never during
    /// the walk, so a promoted operand anchors the dead face only once that
    /// re-decision exists for this family. Every other face answers the
    /// declared/static type, the same under-report the face carried before.
    fn coalesce_operand_never_null(&self, row: &Row, widen_null_aware: bool) -> bool {
        let declared = match self.dot_shorthand_ty(row) {
            Some(ty) => ty,
            None => self.operand_static_ty(row),
        };
        let ty = if self.null_aware_members.contains(&row.range.offset) && widen_null_aware {
            Self::nullable_form(declared)
        } else {
            declared
        };
        if self.written_ty_invalid(ty) {
            return false;
        }
        self.strictly_non_nullable(ty)
            || matches!(
                row.shape,
                RowShape::Receiver(Receiver::Super { .. })
                    | RowShape::Member {
                        recv: Receiver::Super { .. },
                    }
            )
    }

    /// The leading-dot shorthand's static type. A `DotShorthandNameExpression`
    /// / `DotShorthandPropertyAccess` / `DotShorthandConstructorInvocation`
    /// resolves its name against the CONTEXT type — the declaration group's
    /// written annotation (`ConstructorClass v = .new(1)`, `.blue` over an
    /// `enum Color`), never a receiver. A constructor spelling (`.new`,
    /// `.named`) builds the context type itself; a static member spelling
    /// answers the member's declared face. `None` on every other shape: a
    /// real receiver, no armed declaration context, an unresolved or
    /// instance member (the latter reports through
    /// `dot_shorthand_undefined_member`).
    fn dot_shorthand_ty(&self, row: &Row) -> Option<Ty> {
        if !self.dot_shorthand_context(row.range.offset) {
            return None;
        }
        let Ty::Named {
            name: context,
            nullable,
            args,
        } = self.pending_decl_ty?
        else {
            return None;
        };
        if self.ctor_decl_site(context, row.name).is_some() {
            return Some(Ty::Named {
                name: context,
                nullable,
                args,
            });
        }
        let Walk::Found(entry) = self.walk_members(context, row.name) else {
            return None;
        };
        if !entry.is_static {
            return None;
        }
        let ty = if entry.var_ty != Ty::UNKNOWN {
            entry.var_ty
        } else {
            entry.return_ty
        };
        (ty != Ty::UNKNOWN).then_some(ty)
    }

    /// `end_binary_expression` for `??`: the right operand's row anchors
    /// the report when the left operand's type is strictly non-nullable —
    /// the right operand is never executed (the official
    /// `_checkForDeadNullCoalesce`, error_verifier.dart:4733-4739). An
    /// equality comparison as the whole left operand counts too: its
    /// static type is `bool` (`x != null ?? false`).
    pub(crate) fn end_if_null(&mut self) {
        let Some((mark, op_range, rhs_offset)) = self.if_null_groups.pop() else {
            return;
        };
        let rhs_end = self.if_null_rhs_end.take();
        self.stage_if_null_result(mark, op_range);
        // The void-operand use (upstream `visitBinaryExpression` →
        // `checkForUseOfVoidResult(node.leftOperand)` at error_verifier.
        // dart:482): `x ?? d` reads the LEFT operand's value unconditionally
        // — a `void`-typed `x` reports at its own span. The left operand is
        // the row directly below the `??` group mark.
        if let Some(left) = mark
            .checked_sub(1)
            .and_then(|index| self.rows.get(index))
            .filter(|lhs| !matches!(lhs.intent, Intent::Operator(..)))
            && self.operand_static_ty(left) == Ty::Void
        {
            let range = left.range;
            self.emit(SemanticCode::UseOfVoidResult, range, "", "");
        }
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
                // bound-aware posture keeps a generic key send
                // (`m[keyOf(e)] ?? d`) from anchoring.
                .filter(|lhs| !matches!(lhs.intent, Intent::Operator(..)))
                // An unfolded index (`f()[k] ?? d`, a call-result receiver
                // keeps its operand rows) leaves the KEY as the top row:
                // the left operand is the `[]` read, never the key.
                .filter(|lhs| !self.is_index_key_row(lhs))
                // The operand node is the whole expression: a row that
                // stops short of the operator with real tokens between its
                // expression span and the operator is an inner operand of a
                // larger one (`(true ? x : 499)`'s branch, a parenthesized
                // conditional), never the operand the oracle types. The
                // span (not the bare row) answers because an invocation's
                // row is its callee head (`a.b()`, a dot shorthand's
                // `.new(1)`).
                .filter(|lhs| {
                    let span = self.staged_expression_span(lhs.range);
                    self.trivia_between(span.end(), op_range.offset)
                })
                .is_some_and(|lhs| self.coalesce_operand_never_null(lhs, true));

        if !non_null {
            return;
        }
        // The row's range IS the right operand's own node range
        // (`_checkForDeadNullCoalesce` reports at `node.rightOperand`):
        // first token after the operator through the rhs-end latch — a
        // degenerate value row (a collection literal's closing-token face)
        // cannot spell it, and an open latch beats a stale row geometry.
        let range = match rhs_end {
            Some(end) if end > rhs_offset => SourceRange {
                offset: rhs_offset,
                length: end - rhs_offset,
            },
            _ => self
                .rows
                .last()
                .map(|rhs| rhs.range)
                .unwrap_or(SourceRange {
                    offset: rhs_offset,
                    length: 0,
                }),
        };
        if range.length > 0 {
            self.stage_usage(SemanticCode::DeadNullAwareExpression, range, "");
        }
    }

    /// The closed `??`'s static type (`resolveIfNull`, upstream Dart SDK
    /// source, pkg/analyzer/lib/src/dart/resolver/
    /// binary_expression_resolver.dart: `LUB(NonNull(T1), T2)`), staged
    /// for the parenthesized close. Only the plain two-row shape (one left
    /// operand row, one right operand row) is read; the join answers when
    /// the heads are equal or one names a declared supertype of the other
    /// (the LUB is then that supertype); every other pair stays untyped.
    fn stage_if_null_result(&mut self, mark: usize, op_range: SourceRange) {
        if mark == 0 || self.rows.len() != mark + 1 {
            return;
        }
        let lhs = &self.rows[mark - 1];
        let rhs = &self.rows[mark];
        if lhs.range.end() > op_range.offset || rhs.range.offset < op_range.end() {
            return;
        }
        let (
            Ty::Named {
                name: l, args: la, ..
            },
            rt @ Ty::Named {
                name: r,
                nullable: rn,
                ..
            },
        ) = (self.receiver_static_ty(lhs), self.receiver_static_ty(rhs))
        else {
            return;
        };
        let world = self.name_world();
        let joined = if l == r {
            rt
        } else if world.named_sub(r, l) {
            Ty::Named {
                name: l,
                nullable: rn,
                args: la,
            }
        } else if world.named_sub(l, r) {
            rt
        } else {
            return;
        };
        self.if_null_results
            .insert(rhs.range.offset, (lhs.range.offset, joined));
    }

    /// Drops the dead null-aware chains the assignment writes through (the
    /// live verifier reports no dead face for any null-aware WRITE —
    /// `verifyReceiverIndexExpression` fires on the read form only, and the
    /// assignment path carries no `verifyNullAwareAccess` call): the chain
    /// whose extent contains the write target's row is that target's own
    /// chain (`list?..last = 0`, `c1?[0] = 1`), so the write retires it.
    pub(crate) fn cancel_dead_chains_on_write(&mut self, operator: Token) {
        let limit = operator.range.offset;
        // The nearest row ending before `=` is the write target's own tail
        // (receiver, operator, or value row — the `?[` write parses leave
        // only an operator-intent row, so no shape filter here): a chain
        // counts as the write's own only when its extent covers that row.
        let Some(target) = self.rows.iter().rev().find(|row| row.range.end() <= limit) else {
            return;
        };
        let (start, end) = (target.range.offset, target.range.end());
        self.dead_null_chains
            .retain(|&(op, chain_end, _, _, grown)| {
                // A cascade chain a later section already grew covers the write
                // only through that later section — the verifier reports
                // operator..cascade end there (`a?..foo()..x = 1` keeps its row);
                // the root section's own write is the one that retires a chain.
                !(op <= end && start <= chain_end && !grown)
            });
    }

    /// `handle_assignment_expression` for `??=`: the target's read type
    /// strictly non-nullable anchors the report at the value (the official
    /// `visitIfNullAssignment` → `_checkForDeadNullCoalesce(target.read's
    /// type, node.value)`, error_verifier.dart:1519-1527/:4733-4739).
    ///
    /// The target row must be the WHOLE target ([`Self::trivia_between`]):
    /// `x++ ??= y`, `((a, _) && r) ??= x` and `[a, _] ??= x` leave a shorter
    /// inner row before the operator, and the official parser rejects those
    /// shapes as assignment targets outright (`visitIfNullAssignment`'s
    /// `InvalidExpressionAssignmentTargetImpl` arm reports nothing). An
    /// index target (`m[k] ??= v`, `x?[i] ??= v`) leaves its key row
    /// adjacent to the `]`, so the gate keeps the read-through-`[]` shape
    /// silent: the read's type is the operator's return, which the
    /// name-level model does not type (#2172's `groupSetsBy`/`groupBy`
    /// rows).
    pub(crate) fn check_dead_null_assignment(&mut self, operator: Token) {
        if self.text_of(operator) != Some("??=") {
            return;
        }
        let limit = operator.range.offset;
        let rhs_offset = self
            .tokens
            .get(operator.id as usize + 1)
            .map(|token| token.utf16_offset)
            .unwrap_or_else(|| operator.range.end());
        let Some(target) = self
            .rows
            .iter()
            .rev()
            .find(|row| self.is_assignment_target(row, limit))
        else {
            return;
        };
        if !self.coalesce_operand_never_null(target, false) {
            return;
        }
        // The report anchors at `node.value`'s own expression span, never an
        // inner operand row (`bad()`'s head, `-42`'s digits, `[]`'s closing
        // bracket): from the operand's first token through the value row's
        // own expression end. An invalid value stages no row at all — the
        // official report then anchors a synthetic empty node the row model
        // cannot spell, and the face stays silent (under-report).
        let Some(value) = self.rows.last().filter(|row| row.range.offset >= rhs_offset) else {
            return;
        };
        let span = self.staged_expression_span(value.range);
        let offset = span.offset.min(rhs_offset);
        let range = SourceRange {
            offset,
            length: span.end().saturating_sub(offset),
        };
        self.stage_usage(SemanticCode::DeadNullAwareExpression, range, "");
    }

    /// Whether `row` is the whole assignment target of the operator starting
    /// at `limit` — [`Self::check_dead_null_assignment`]'s gate. A target is
    /// a NAME or a member access: the record and collection folds leave a
    /// `)`/`]`/`}` row where a pattern stands, which is no name, and a
    /// literal, a type use, or a send result is no target either — the
    /// official resolver rejects all of them outright
    /// (`visitIfNullAssignment`'s `InvalidExpressionAssignmentTargetImpl`
    /// arm reports nothing). The row must also END at the operator: an
    /// `x++ ??= y` latches the `x` row, an inner operand of a larger
    /// expression, never that expression's own row.
    fn is_assignment_target(&self, row: &Row, limit: u32) -> bool {
        if row.range.end() > limit || matches!(row.intent, Intent::Operator(..) | Intent::Invoke)
        {
            return false;
        }
        let named = match &row.shape {
            RowShape::Member { .. }
            | RowShape::Receiver(Receiver::This { .. } | Receiver::Super { .. }) => true,
            RowShape::Bare(_) => self.starts_a_name(row.range.offset),
            _ => false,
        };
        named && self.trivia_between(row.range.end(), limit)
    }

    /// Whether the source at a UTF-16 offset starts a Dart identifier.
    fn starts_a_name(&self, offset: u32) -> bool {
        self.byte_at(offset)
            .is_some_and(|byte| byte.is_ascii_alphabetic() || byte == b'_' || byte == b'$')
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
                            && !d.retired
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
    /// declared-variable-pattern binding face). A bare parameterized head
    /// (`Named{_, EMPTY}`) binds its instantiate-to-bounds face — the
    /// oracle's resolved `staticType` for a written `G` is `G<A<dynamic>>`
    /// (`type.dart` `instantiateTypeFormalsToBounds`), the same fill
    /// [`Self::bind_declared_type`]/[`Self::bind_parameter_type`] apply.
    /// A pattern binder's head-named write (`Head() && final x`, a
    /// null-check binder's `Circle` scrutinee) reaches the row's
    /// `undefined_operator`/matched-type face with the declaration's own
    /// arguments — never the raw `Circle` head.
    pub(crate) fn bind_var_type(&mut self, name: StringId, ty: Ty) {
        let ty = self.instantiated_head_ty(ty);
        if let Some(frame) = self.frames.last_mut() {
            frame.var_types.insert(name, ty);
        }
    }

    /// The innermost frame's declared slot for a local — the typedness ask
    /// the for-in element staging makes before overwriting an inferred
    /// (`Unknown`) slot.
    pub(crate) fn local_declared_ty(&self, name: StringId) -> Option<Ty> {
        self.frames
            .last()
            .and_then(|frame| frame.var_types.get(&name).copied())
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
    pub(crate) fn destructure_record(
        &mut self,
        init_start: u32,
        semi: u32,
    ) -> Vec<(StringId, SourceRange, Ty)> {
        let vars = std::mem::take(&mut self.pattern_bind_vars);
        let records = self.pattern_bind_records;
        let dirty = self.pattern_bind_dirty;
        self.pattern_bind_records = 0;
        self.pattern_bind_dirty = false;
        if dirty || records != 1 || vars.is_empty() {
            return Vec::new();
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
            return Vec::new();
        };
        let Some(ty) = self.initializer_static_ty(&row) else {
            return Vec::new();
        };
        let Ty::Record { shape, .. } = ty else {
            return Vec::new();
        };
        let fields = self.boundary.pool().record_shape(shape).positional.to_vec();
        if fields.len() != vars.len() {
            return Vec::new();
        }
        if let Some(frame) = self.frames.last_mut() {
            let mut staged: Vec<(StringId, SourceRange, Ty)> = Vec::new();
            for ((name, span), ty) in vars.iter().zip(fields) {
                frame.var_types.insert(*name, ty);
                staged.push((*name, *span, ty));
            }
            return staged;
        }
        Vec::new()
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
            // The prefix-reached enum (`android_webview.Level.debug`): the
            // coverage space keys on the declaration name — the bare leaf,
            // the same spelling the scrutinee's type arms.
            Receiver::PrefixedContainer {
                name,
                kind: ContainerKind::Enum,
                ..
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
        // A frame-local `_` declares nothing when wildcard-variables is on:
        // the official scopes skip the name at every frame level —
        // LocalScope.add (upstream Dart SDK source,
        // pkg/analyzer/lib/src/dart/element/scope.dart:550-554), and the
        // formal scopes on the same feature gate (scope.dart:22,
        // ConstructorInitializerScope; scope.dart:117-118,
        // FormalParameterScope) — so a later read falls through the lexical
        // chain to undefined-name. The tracked row is deliberately kept:
        // [`Self::add_tracked_local`] still pushes the usage entry (the
        // unused-local wildcard exemption walks the tracked vec, not this
        // resolution set), only the binding is skipped. The no-frame
        // branch (the top-level initializer world) stays ordinary — a
        // top-level `_` is a library-scope name, not a LocalScope row, and
        // keeps resolving.
        if self.is_wildcard(name) && self.frames.last().is_some() {
            return;
        }
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
        //
        // An if-case binder takes the same individual posture without the
        // arm scope (#2858): its declaring context pops at the root
        // pattern's completion — long before the then-branch reads the
        // binder — so the sentinel's switch-slot arithmetic would strand
        // every read. The row registers ungrouped and the site join below
        // patches in the same-name spelling pair.
        let mut if_case_site = false;
        let group = if matches!(kind, LocalKind::PatternVariable) {
            let depth = self.switch.switches.len();
            if self.switch.case_sink_open
                && !self.switch.if_case_window
                && depth > 0
                && depth <= u16::MAX as usize
            {
                Some(u32::MAX - depth as u32)
            } else if self.switch.if_case_window {
                if_case_site = true;
                None
            } else {
                // A statement-level pattern-declared variable joins the
                // open statement-list duplicate scope (`checkStatements`
                // `PatternVariableDeclarationStatement` arm —
                // `statement.declaration.elements`' `scope.add`,
                // duplicate_definition_verifier.dart:99-103): `var a` /
                // `final a` / `var (a,b)` collide inside the block,
                // function-body, or for-head scope like any
                // `VariableDeclarationStatement` name. Case-header
                // (arm-scoped) and if-case binders above skip — neither
                // is a `PatternVariableDeclarationStatement` element.
                self.dup_scope_add(name, range);
                if let Some(group) = self.pattern_group_open {
                    Some(group)
                } else {
                    self.pattern_group_seq += 2;
                    self.pattern_group_open = Some(self.pattern_group_seq);
                    Some(self.pattern_group_seq)
                }
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
            retired: false,
        });
        if if_case_site {
            self.join_pattern_site_binder(self.locals.len() - 1);
        }
    }

    /// Joins the pattern-site binder row at `index` with its same-name
    /// sibling — the non-window pattern sites (if-case patterns here, the
    /// for-in pattern heads through [`Self::convert_window_binder_to_site`],
    /// #2858): the official verifier visits each
    /// `visitDeclaredVariablePattern` binder as its own local
    /// (unused_local_elements_verifier.dart:565-579), while one logical-or
    /// element's same-name spellings stay one element upstream
    /// (`UsedLocalElements.addElement`) — a use of any spelling spares the
    /// pair, an unused spelling reports. The scan stops at the most recent
    /// same-name pattern row of the live frame window: an ungrouped
    /// sibling mints a join group over the pair, a joined sibling extends
    /// its group (the `p || p || p` chain), and a window or sentinel row
    /// stays untouched (a declaration's all-or-nothing set and a case
    /// header's arm scope are other sites' contracts).
    fn join_pattern_site_binder(&mut self, index: usize) {
        let Some(&lo) = self.frame_local_marks.last() else {
            return;
        };
        let name = self.locals[index].name;
        let Some(offset) = self.locals[lo..index]
            .iter()
            .rposition(|decl| decl.name == name && matches!(decl.kind, LocalKind::PatternVariable))
        else {
            return;
        };
        let sibling = lo + offset;
        match self.locals[sibling].group {
            None => {
                // The odd id space: window groups mint even
                // (`pattern_group_seq` steps by 2), so a site join never
                // collides with a live window and the parity tells the two
                // apart here.
                self.pattern_group_seq += 2;
                let group = self.pattern_group_seq + 1;
                self.locals[sibling].group = Some(group);
                self.locals[index].group = Some(group);
            }
            Some(group) if group < PATTERN_ARM_SCOPE_BASE && group % 2 == 1 => {
                self.locals[index].group = Some(group);
            }
            _ => {}
        }
    }

    /// Converts one for-in pattern-head row from its window group to the
    /// per-binder site form (#2858): `for (var (a, b) in xs)` parses its
    /// head as a bare pattern with no `PatternVariableDeclaration`
    /// wrapper, so the head's binders register into the all-or-nothing
    /// window before `handle_for_in_loop_parts` names the head a pattern
    /// one — the conversion retags them individually and patches the
    /// same-name spelling join. Rows outside a window group (the
    /// declarator form's plain local, a stray sentinel) stay untouched.
    pub(crate) fn convert_window_binder_to_site(&mut self, index: usize) {
        if !matches!(self.locals[index].kind, LocalKind::PatternVariable) {
            return;
        }
        match self.locals[index].group {
            Some(group) if group < PATTERN_ARM_SCOPE_BASE => {
                self.locals[index].group = None;
                self.join_pattern_site_binder(index);
            }
            _ => {}
        }
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
    /// types are the resolved interface types every verdict reads. A bare
    /// parameterized head surviving the fold (`G source` over `typedef
    /// G<X extends A<X>> = X Function()`, `G?` alike) binds its
    /// instantiate-to-bounds face — the oracle's resolved `staticType` for
    /// a written `G` is `G<A<dynamic>>` (`type.dart`
    /// `instantiateTypeFormalsToBounds`), so the declared slot carries the
    /// defaults, never the raw head.
    pub(crate) fn bind_declared_type(&mut self, name: StringId) {
        let ty = self.pending_decl_ty.unwrap_or(Ty::UNKNOWN);
        let ty = self.fold_alias_ty(ty);
        let ty = self.instantiated_head_ty(ty);
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
    /// carries. A bare parameterized head surviving the fold (`testme(G
    /// source)` — the co19 `Instantiate-to-bound` typedef family's spelled
    /// annotation) binds its instantiate-to-bounds face, the same resolved
    /// `staticType` the declared-type arm above binds.
    pub(crate) fn bind_parameter_type(&mut self, name: StringId) {
        let ty = self
            .armed_ty
            .take()
            .or_else(|| self.closure_param_context_ty(name))
            .unwrap_or(Ty::UNKNOWN);
        let ty = self.fold_alias_ty(ty);
        let ty = self.instantiated_head_ty(ty);
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
        // The formal's ident end joins the closure-formal index: a formal
        // the callee inference never solves (the literal's callee receiver
        // is itself unresolvable — `(x ?? const []).map((sub) => …)`)
        // keeps every member chain rooted at it silent, and the
        // extension-use re-attribution keys the end to re-credit the
        // chain's reads (`sub.source.meta`'s `meta`).
        if let Some(decl) = self.locals.last().filter(|decl| decl.name == name) {
            self.closure_formal_names.insert(decl.name);
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
        // The captured context answers first when it latched through a
        // non-argument channel — the declaration initializer, the
        // assignment target, the enclosing return position folded into
        // `ctx` at open time; a matched count admits its schema entry
        // directly (the same greatest-closure walk the live arg-channel
        // recompute below runs, minus the receiver/classification
        // machinery only the arg channel needs). An argument-channel
        // latch stands aside: this formal's own slot/schema recompute
        // below is the same answer, and the promotion-latched receiver
        // below the callee re-derives at pass 2 — the frozen begin-time
        // face would bind the unpromoted type (`h.items.map((e) => …)`
        // under `h is! Impl` reads `ElImpl`, never `El`).
        if !capture.ctx_from_arg
            && let Some(ctx) = capture.ctx
            && self.closure_ctx_matches(ctx, capture.type_params)
        {
            return match formal {
                SchemaSlot::Positional(index) => self.closure_ctx_formal_ty(ctx, index),
                SchemaSlot::Named(label) => {
                    let Ty::Function { shape, .. } = ctx else {
                        return None;
                    };
                    let schema = {
                        let pool = self.boundary.pool();
                        pool.function_shape(shape)
                            .named
                            .iter()
                            .find(|(n, _)| *n == label)
                            .map(|(_, ty)| *ty)
                    };
                    schema.and_then(|schema| self.closure_ctx_greatest_closure(schema))
                }
            };
        }
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
        let mut below_span = None;
        let recv = if callee.qualified {
            callee_row
                .checked_sub(1)
                .and_then(|below| self.rows.get(below))
                .map_or(Receiver::Unknown, |below| {
                    below_span = Some(below.range);
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
        let callee_name = callee.name;
        let schema = self.closure_context_schema(recv, callee_name, Some(callee_row), slot, formal);
        // The pass-2 deferral: the context face derives from the below
        // row's staged classification, which pass 1 cannot re-derive (a
        // promotion-latched receiver below it — `rebound.fragments.where(
        // (f) => …)` — re-binds only in pass 2), so the formal's slot
        // freezes the stale schema entry. Record the ingredients; the
        // re-bind pass recomputes the schema over the re-derived link
        // face and corrects the formal's reads through the same
        // `rebound_locals` map the var re-binds ride. A formal pass 1 could
        // not solve because the below receiver itself was unclassifiable (a
        // promoted receiver — `if (x is C) x.items.map((e) => …)`) defers
        // with the `Unknown` face: pass 2 re-derives the receiver and the
        // oracle's downward inference stands. A classified receiver whose
        // schema stayed `dynamic` (horizontal inference fixed the callee's
        // own parameter) keeps that verdict.
        if let Some(below) = below_span
            && (schema.is_some() || matches!(recv, Receiver::Unknown))
            && let Some(decl) = self.locals.last().filter(|decl| decl.name == name)
        {
            self.deferred_closure_formals.push(DeferredClosureFormal {
                decl_offset: decl.range.offset,
                parse_ty: schema.unwrap_or(Ty::UNKNOWN),
                callee_name,
                slot,
                formal,
                below,
            });
        }
        return schema;
    }

    /// [`Self::closure_param_context_ty`]'s schema tail: the callee's
    /// parameter type at the literal's slot is the context type the
    /// literal's own schema came from. Shared by the pass-1 bind and the
    /// pass-2 re-bind (the deferral re-runs it over the re-derived
    /// receiver face). The element formals answer first; a
    /// function-typed variable callee's captured signature closes.
    pub(crate) fn closure_context_schema(
        &mut self,
        recv: Receiver,
        callee_name: StringId,
        // `None` on the pass-2 re-bind (see `closure_schema_instantiate`).
        callee_row: Option<usize>,
        slot: SchemaSlot,
        formal: SchemaSlot,
    ) -> Option<Ty> {
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
        let callee_own_head = match (schema, &recv) {
            (
                Ty::Named {
                    name: head,
                    nullable: false,
                    args,
                },
                Receiver::Typed {
                    type_name: container,
                    ..
                },
            ) if args == ArgListId::EMPTY
                && matches!(self.walk_members(*container, callee_name), Walk::Found(ref entry)
                    if entry.kind == MemberKind::Method
                        && entry.type_param_names.contains(&head)) =>
            {
                true
            }
            _ => false,
        };
        let instantiated = self.closure_schema_instantiate(schema, &recv, callee_name, callee_row);
        let schema = instantiated.unwrap_or(schema);
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
                // A callee-own head the call's upward face solved to a
                // type the file CAN name: the oracle binds the class —
                // official `A? <: T?` binds `T` the argument's non-null
                // head (probed: a resolved `Tok2?` argument types the
                // callback formal `Tok2`; manifest_id.dart:150/157's
                // `this` under `extension on ManifestItemId?` stays
                // silent on `it.write`). The class itself binds now —
                // the solved face feeds the call's downstream nesting
                // (bundle_writer.dart's `writeOptionalObject(info, (info)
                // { writeMap(info, … writeValue: (value) => …) })` types
                // the inner map's `K`/`V` from the OUTER formal, which a
                // `dynamic` bind starves) — and the receiver class joins
                // [`Engine::inferred_formal_containers`]: our member walk
                // under-resolves the extension/import members those faces
                // often carry (manifest_item.dart's `items.write`
                // family), so the undefined-member rows against an
                // inference-bound receiver stay silent (the under-report
                // posture — the oracle resolves them).
                if callee_own_head && let Some(solved) = instantiated {
                    if let Ty::Named { name, .. } = solved {
                        self.inferred_formal_containers.insert(name);
                    }
                    return Some(solved);
                }
                // `greatestClosure`'s decidable face (upstream
                // `_inferFormalParameters`): a provable named schema entry
                // binds itself — `(e)` under `void Function(R?)` binds
                // `R?`, nullable suffix and all — not the `Object?`
                // recovery that face exists for (an `InvalidType` or
                // `T <: Null` head). An alias spelling resolving is not
                // enough: a typedef whose right-hand fold terminates in an
                // unprovable head (a `shared.` import the package config
                // cannot resolve) is the oracle's InvalidType at the use —
                // the formal takes the recovery faces below, never this
                // bind.
                if self.named_type_is_type(head)
                    && self.args_are_types(args)
                    && self.folded_alias_arg_provable(head)
                {
                    return Some(Ty::Named {
                        name: head,
                        nullable,
                        args,
                    });
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
                // A schema head the CALLEE ITSELF declares as a method
                // type parameter (the container's own parameters excluded —
                // they are the receiver's to instantiate) is the oracle's
                // fresh-`T` face: the literal's parameters infer before
                // the call's inference completes, so greatestClosure sees
                // the parameter unresolved and answers its bound —
                // `Object?` (the binary writer's `writeOptionalObject<T>(
                // T? object, void Function(T x) write)` callback formal
                // reports the unchecked verdict the oracle reports). Every
                // other head keeps the receiver-stand-in walk below.
                let callee_own_param = match recv {
                    Receiver::Typed {
                        type_name: container,
                        ..
                    } => {
                        matches!(self.walk_members(container, callee_name), Walk::Found(ref entry)
                            if entry.kind == MemberKind::Method
                                && entry.type_param_names.contains(&head))
                    }
                    _ => false,
                };
                if callee_own_param {
                    // Horizontal inference (upstream `_inferFormalParameters`
                    // runs after the non-literal arguments' stage): an
                    // already-passed argument whose formal mentions the
                    // parameter fixed it before the literal's formals were
                    // inferred. When that argument's own type is beyond the
                    // model, the solution is unknown — `dynamic`, never the
                    // fresh-parameter `Object?` bound.
                    if callee_row.is_some() && self.untyped_arg_constrains(&recv, callee_name, head)
                    {
                        return None;
                    }
                    return Some(self.object_question());
                }
                match recv {
                    Receiver::Typed { ty, .. } => {
                        match self.receiver_first_type_arg(ty) {
                            // No written first argument — a receiver the
                            // instantiation never parameterized (the
                            // non-generic `DatabaseConnectionUser` callee
                            // holder, the arg-less statement face). Nothing
                            // invalid was written, so greatestClosure's
                            // `Object?` collapse does not apply wholesale —
                            // the schema head is valid but locally unnamed
                            // (the container's own parameter, an import).
                            // The entry's own nullability still decides:
                            // `greatestClosure(S?)` keeps the nullable face
                            // (an unnamed nullable parameter still owes the
                            // unchecked verdict, the binary writer's
                            // `writeOptionalObject` callback), while a
                            // non-nullable entry keeps `dynamic` (the
                            // under-report `None`).
                            None => nullable.then_some(self.object_question()),
                            Some(Ty::Dynamic) => None,
                            Some(Ty::Named {
                                name: arg_head,
                                nullable: false,
                                args: arg_args,
                            }) if (self.named_type_is_type(arg_head)
                                // A foreign-but-real element type (the
                                // receiver's first argument spelling a class
                                // another graph unit declares — `List<
                                // Reference>.map((e) => …)`'s foreign
                                // `Reference`, never imported into the
                                // diagnosing file): the oracle's element
                                // model holds the resolved class, the
                                // callback formal is that class — not the
                                // `Object?` invalid recovery. Only a head
                                // the whole graph fails to declare is
                                // invalid.
                                || !self.graph.declaring_libraries(arg_head).is_empty()
                                // A prefixed element spelling (`List<
                                // idl.IndexRelationKind>.map`) interns the
                                // head dot-joined; it resolves through the
                                // import prefix, not the bare name tables.
                                || self.prefixed_element_head_resolves(arg_head))
                                && self.args_are_types(arg_args)
                                // An alias spelling resolves, but what it
                                // SPELLS decides: a typedef whose right-hand
                                // fold terminates in an unprovable head
                                // (`MergedTypeConstraint` — a `shared.`
                                // import the package config cannot resolve)
                                // is the oracle's InvalidType at the use,
                                // and greatestClosure binds the formal
                                // `Object?` — the nullable verdict — not
                                // the silent arm.
                                && self.folded_alias_arg_provable(arg_head) =>
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
        &mut self,
        ty: Ty,
        recv: &Receiver,
        callee_name: StringId,
        // `callee_row` is the pass-1 row index; `None` on the pass-2
        // re-bind, where the fold may have retired the row and the
        // argument-list state is gone — the receiver-seeded substitution
        // stands alone there (the oracle's receiver instantiation is the
        // face a re-derived receiver changes; the upward face ran at bind
        // time over arguments the re-bind never retypes).
        callee_row: Option<usize>,
    ) -> Option<Ty> {
        let Some((params, seeds, ordered)) = self.closure_callee_params(recv, callee_name) else {
            return None;
        };
        if params.is_empty() {
            return None;
        }
        let param_index = |name: StringId| params.iter().position(|(param, _)| *param == name);
        let mut solutions: Vec<Option<Ty>> = vec![None; params.len()];
        // The slots the call fixes outright — an explicit type argument,
        // the receiver's written instantiation — are final: the upward
        // face neither re-solves nor poisons them (`fold<Folder?>(null,
        // ...)`'s `Null` argument never overrides the written `Folder?`;
        // the oracle runs no inference over an explicit argument list).
        let mut fixed: Vec<bool> = vec![false; params.len()];
        // Explicit type arguments fix first — peeked, not consumed (the
        // send-close face owns the slot). A positional list pairs against
        // the declaration's parameter order, which the foreign-declaration
        // recovery cannot vouch for, so those solutions stay open.
        if let (Some(own), Some(callee_row)) = (ordered, callee_row)
            && let Some(callee_offset) = self.rows.get(callee_row).map(|r| r.range.offset)
            && let Some((_, entry)) = self
                .call_type_args
                .iter()
                .find(|(row, _)| *row == callee_offset)
            && entry.tys.len() == own
        {
            for ((slot, fixed), ty) in solutions
                .iter_mut()
                .zip(fixed.iter_mut())
                .zip(entry.tys.iter())
            {
                *slot = Some(*ty);
                *fixed = true;
            }
        }
        for (name, ty) in &seeds {
            if let Some(index) = param_index(*name)
                && solutions[index].is_none()
            {
                solutions[index] = Some(*ty);
                fixed[index] = true;
            }
        }
        // Upward face: the arguments already typed at bind time unify
        // against the callee's formals for the still-open slots. The
        // pass-2 re-bind skips it (the argument list closed with pass 1;
        // the seeds below carry the receiver face it re-derived).
        let formals = match callee_row {
            Some(_) => Some(self.callee_formals(recv, callee_name)?),
            None => None,
        };
        let level_args = match callee_row {
            Some(_) => self.arg_levels.last()?.args.clone(),
            None => Vec::new(),
        };
        let mut candidates: Vec<(StringId, Ty)> = Vec::new();
        let mut positional_index = 0usize;
        for (name, ty, _literal, range) in &level_args {
            // An argument the flush face could not type — an
            // extension-getter send keeps `Unknown` there (the merge-time
            // field face never types one, and the multi-row window records
            // the raw unknown) — recovers its static type from its own row
            // on demand, the same resolution the `var`-inference window
            // runs. Without it the schema slot stays unsolved and the
            // closure formal collapses to the `Object?` recovery.
            let ty = if *ty != Ty::Unknown || range.length == 0 {
                *ty
            } else {
                let owned = self
                    .rows
                    .iter()
                    .rev()
                    .find(|row| row.range == *range)
                    .cloned();
                match owned {
                    Some(row) => {
                        let ty = self.expression_row_ty(&row);
                        if ty == Ty::Unknown {
                            // A bare `this` argument's face is the
                            // enclosing container's static type — the
                            // extension's on-type with its nullability
                            // and arguments (`extension on ManifestItemId?`
                            // contributes `ManifestItemId?`, so the `T?`
                            // absorption above solves `T` its non-null
                            // head, manifest_id.dart:150/157).
                            if let Receiver::This { container, kind } = self.argument_receiver(&row)
                            {
                                self.this_static_ty(container, kind)
                            } else {
                                let read = self.initializer_read_ty(&row);
                                if read == Ty::Unknown
                                    && !row.qualified
                                    && self
                                        .top_level_decl(row.name)
                                        .is_some_and(|decl| matches!(decl.kind, MicroKind::Class))
                                {
                                    // A creation argument (`A()`): the
                                    // invocation's static type is the
                                    // class itself — the send's merge
                                    // hasn't typed the row yet (the
                                    // staged entry carries the bare
                                    // callee, `Read`-intent, identifier
                                    // span) at this bind-time read, so
                                    // the class declaration answers
                                    // directly. A bare class-name
                                    // expression (a type literal) rides
                                    // the same face — under-report.
                                    Ty::named(row.name)
                                } else {
                                    read
                                }
                            }
                        } else {
                            ty
                        }
                    }
                    None => Ty::Unknown,
                }
            };
            let formal = match name {
                Some(n) => formals.as_ref().and_then(|f| f.named_formal(*n)),
                None => {
                    let formal = formals
                        .as_ref()
                        .and_then(|f| f.positional.get(positional_index));
                    positional_index += 1;
                    formal
                }
            };
            if let Some(formal) = formal
                && ty != Ty::Unknown
            {
                self.closure_unify_upward(formal.ty, ty, &params, &mut candidates);
            }
        }
        let mut poisoned: FxHashSet<usize> = FxHashSet::default();
        for (param, ty) in candidates {
            let Some(index) = param_index(param) else {
                continue;
            };
            if fixed[index] || poisoned.contains(&index) {
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
        // schema untouched — exactly the pre-existing route. A function
        // schema decides through its parts: the literal's context latch
        // (#2868's argument channel) consumes `K` whole, and a part the
        // solve left as a bare callee parameter (`mapIt`'s unsolved `R`)
        // keeps the schema out of scope's reach — no context.
        (out != ty && self.schema_ty_fully_decidable(out)).then_some(out)
    }

    /// [`Self::schema_type_is_decidable`] lifted over `Ty::Function`: a
    /// substituted function schema is decidable when its return and every
    /// formal decide. The leaf ask is the same except the always-decidable
    /// arms join — a `void`/`dynamic`/`Null`/`Never` part is a solved
    /// answer, never an unresolved spelling.
    fn schema_ty_fully_decidable(&self, ty: Ty) -> bool {
        let Ty::Function { shape, .. } = ty else {
            return self.schema_type_is_decidable(ty);
        };
        let sig = {
            let pool = self.boundary.pool();
            pool.function_shape(shape).clone()
        };
        let decidable = |ty: Ty| {
            matches!(ty, Ty::Dynamic | Ty::Void | Ty::Null | Ty::Never)
                || self.schema_type_is_decidable(ty)
        };
        decidable(sig.return_ty)
            && sig
                .positional
                .iter()
                .chain(sig.named.iter().map(|(_, ty)| ty))
                .all(|ty| decidable(*ty))
    }

    /// The closure-return tally's recording half (the listener's return
    /// handlers call it with the return expression's row index): when the
    /// innermost open function literal owns the current frame, the
    /// expression's resolved type joins the literal's per-offset tally.
    /// Only NAMED heads count — `Null` (absorbed by the LUB's
    /// nullability) and unresolved shapes stay neutral. Validity is asked
    /// WHERE the head was written: locally resolvable heads are real, and
    /// a head spelled through a member's declared return type resolves in
    /// the DECLARING file's scope (the spelling need not be visible here —
    /// `on Hidden` behind a `show` combinator still names a real type,
    /// while the broken-re-export `Source` resolves nowhere).
    pub(crate) fn note_closure_return(&mut self, mark: usize) {
        let Some(capture) = self.closure_caps.last() else {
            return;
        };
        // The literal owns the return when no further EXECUTABLE frame
        // opened inside it (a nested literal or local function takes the
        // return over): block frames of the body sit above the literal's
        // own executable frame without claiming it.
        if self.frames[capture.frame_depth.min(self.frames.len())..]
            .iter()
            .any(|frame| matches!(frame.kind, FrameKind::Executable { .. }))
        {
            return;
        }
        let offset = capture.offset;
        let Some(row) = self.rows.get(mark) else {
            return;
        };
        let row = row.clone();
        let ty = self.expression_row_ty(&row);
        let Ty::Named { name, .. } = ty else {
            return;
        };
        if self.is_scope_param(name)
            || self.name_world().is_known_head(name)
            || !matches!(self.type_resolution(name).0, TypeResolution::Missing)
        {
            self.closure_return_valid.insert(offset);
            return;
        }
        // The origin file: a member send's declared return resolves where
        // the member was declared.
        if let RowShape::Member { recv } = &row.shape {
            let container = match recv {
                Receiver::Typed { type_name, .. } => Some(*type_name),
                Receiver::This { container, .. }
                | Receiver::Container {
                    name: container, ..
                } => Some(*container),
                _ => None,
            };
            let valid = container.is_some_and(|container| {
                matches!(self.walk_members(container, row.name), Walk::Found(_))
                    && self
                        .member_faces
                        .last_found_file()
                        .is_some_and(|file| self.named_type_is_type_in(file, name))
            });
            if valid {
                self.closure_return_valid.insert(offset);
            } else if container.is_some() {
                self.closure_return_counted.insert(offset);
            }
        }
    }

    /// The send-close upward face: unify the call's typed arguments
    /// against the callee's formals, solving the callee's inference
    /// parameters (`math.max(0, n - 1)` binds `T := int`) for the return
    /// read. The bind-time solver ([`Self::closure_schema_instantiate`])
    /// runs the same unification for closure formals while the send is
    /// open; the return's consumers (the assignment staging, the `var`
    /// window) read after the levels close, so the solution stashes keyed
    /// on the callee row's range offset — stable while row indexes churn
    /// under the folds. Conflicting candidates poison the slot and the
    /// solved types must stay decidable in the caller's scope, the same
    /// rules the bind-time solver applies; anything less keeps the
    /// free-parameter under-report stand-in.
    pub(crate) fn solve_call_params(
        &mut self,
        callee: usize,
        row: &Row,
        args: &[(Option<StringId>, Ty, bool, SourceRange)],
        closures: &[(u32, u32)],
    ) {
        let recv = match &row.shape {
            RowShape::Member { recv } => *recv,
            RowShape::Bare(info) => {
                let recv = Engine::receiver_of_bare(*info);
                if row.qualified {
                    // A pre-merge qualified callee (`math.max(...)` at the
                    // group's close — the member merge that retypes the row
                    // to `Member` runs after the send): the receiver row sits
                    // directly below, and its bare face names it.
                    if matches!(recv, Receiver::Unknown)
                        && let Some(below) =
                            callee.checked_sub(1).and_then(|index| self.rows.get(index))
                        && let RowShape::Bare(info) = below.shape
                    {
                        Engine::receiver_of_bare(info)
                    } else {
                        recv
                    }
                } else {
                    // An unqualified implicit-this callee (`readEnum(...)`
                    // inside the declaring class): the bare scope resolves
                    // the name to the enclosing container's member, whose
                    // method type parameters the solve unifies against
                    // (`T readEnum<T extends Enum>(List<T> values)` binding
                    // `T` from the argument's element — the same
                    // implicit-this walk `callee_return_ty`'s bare arm
                    // applies).
                    match recv {
                        Receiver::Unknown => {
                            if let (_, RefTarget::Member { container }) =
                                self.resolve_bare(row.name)
                            {
                                let kind = self
                                    .container_kind_of(container)
                                    .unwrap_or(ContainerKind::Class);
                                Receiver::This { container, kind }
                            } else {
                                Receiver::Unknown
                            }
                        }
                        other => other,
                    }
                }
            }
            _ => return,
        };
        let Some((params, _, _)) = self.closure_callee_params(&recv, row.name) else {
            return;
        };
        if params.is_empty() {
            return;
        }
        // The prefixed callee's formals live in the foreign declaration —
        // projected into the diagnosing pool before any unification reads
        // them (the same cross-pool discipline `closure_callee_params`'
        // prefix arm applies). Every other receiver keeps the shared
        // formal table face.
        let formals = match &recv {
            Receiver::Prefix { name: prefix } => {
                let dartq_resolution::Lookup::Found(dartq_resolution::NamespaceEntry::Decl(file)) =
                    self.namespaces
                        .lookup_prefixed(self.graph, self.lib, *prefix, row.name)
                else {
                    return;
                };
                let Some(decl) = self.index.get(file).and_then(|f| f.decl(row.name)) else {
                    return;
                };
                let decl = self.boundary.top(file, row.name, decl);
                decl.formals.as_deref().cloned()
            }
            _ => self.callee_formals(&recv, row.name),
        };
        let Some(formals) = formals else {
            return;
        };
        let mut candidates: Vec<(StringId, Ty)> = Vec::new();
        let mut positional_index = 0usize;
        for (name, ty, _, _) in args {
            let formal = match name {
                Some(label) => formals.named_formal(*label),
                None => {
                    let formal = formals.positional.get(positional_index);
                    positional_index += 1;
                    formal
                }
            };
            if let (Some(formal), true) = (formal, *ty != Ty::Unknown) {
                self.closure_unify_upward(formal.ty, *ty, &params, &mut candidates);
            }
        }
        let param_index = |name: StringId| params.iter().position(|(param, _)| *param == name);
        let mut solutions: Vec<Option<Ty>> = vec![None; params.len()];
        let mut poisoned: FxHashSet<usize> = FxHashSet::default();
        for (param, ty) in candidates {
            let Some(index) = param_index(param) else {
                continue;
            };
            if poisoned.contains(&index) {
                continue;
            }
            match solutions[index] {
                None if self.schema_type_is_decidable(ty) => solutions[index] = Some(ty),
                Some(kept) if kept == ty => {}
                _ => {
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
        // The invalid-result probe (the resolve_file corpus face): an
        // all-invalid closure argument over a generic send whose solve
        // left every parameter open makes the oracle's own result
        // `InvalidType` (the closure was the only constraint source) —
        // the send's inferred return is invalid, and member accesses over
        // it never apply an extension. When the send is itself a
        // `var`-inferred initializer's outer expression, the declarator's
        // static type is that invalid face: latch it by name here, at the
        // close — the rows window cannot carry it (a closure argument
        // drains the window's rows below the initializer mark before the
        // initializer ends). The closure-caps guard keeps a send inside a
        // closure body from latching an outer pending declarator: that
        // send folds while the enclosing closure's capture is still open,
        // while the argument closure of the send itself has completed its
        // capture before the argument list closes (`in_initializer` and
        // the frame depth are not nesting-safe here — the closure body's
        // own variable initializers clobber the flag and unwind frames
        // below the initializer's arming depth).
        if !self.closure_invalid_returns.is_empty()
            && !params.is_empty()
            && solutions.iter().all(|slot| slot.is_none())
            && closures
                .iter()
                .any(|&(start, _)| self.closure_invalid_returns.contains(&start))
            && self.closure_caps.is_empty()
            && let Some(Some((name, _))) = self.init_declarators.last()
        {
            self.invalid_typed_locals.insert(*name);
        }
        if !map.is_empty() {
            self.solved_call_params.insert(row.range.offset, map);
        }
    }

    /// Whether an already-passed argument of the open send carries no
    /// modelled type while its formal mentions the callee parameter
    /// `param` inside a type argument (`addMapEntryList(entries, (k, v) =>
    /// …)` with `entries` typed by a generic local function's unsolved
    /// return): the argument constrained `param` upstream, so the closure
    /// formal's schema entry is not the fresh parameter. A bare `T`/`T?`
    /// formal stays out — an invalid-typed argument there solves `param`
    /// to the invalid face, whose greatest closure is the `Object?` the
    /// caller answers (`writeOptionalObject<T>(T? object, …)` over an
    /// unresolvable `Token?`).
    fn untyped_arg_constrains(
        &mut self,
        recv: &Receiver,
        callee_name: StringId,
        param: StringId,
    ) -> bool {
        let Some(formals) = self.callee_formals(recv, callee_name) else {
            return false;
        };
        let Some(level) = self.arg_levels.last() else {
            return false;
        };
        let args = self.boundary.args.borrow();
        let nested_mention = |ty: Ty| -> bool {
            let Ty::Named { args: list, .. } = ty else {
                return false;
            };
            let mut stack: Vec<Ty> = args.get(list).to_vec();
            while let Some(ty) = stack.pop() {
                if let Ty::Named {
                    name, args: list, ..
                } = ty
                {
                    if name == param {
                        return true;
                    }
                    stack.extend(args.get(list).iter().copied());
                }
            }
            false
        };
        let mut positional = 0usize;
        level.args.iter().any(|(label, ty, _, _)| {
            let formal = match label {
                Some(label) => formals.named_formal(*label),
                None => {
                    positional += 1;
                    formals.positional.get(positional - 1)
                }
            };
            *ty == Ty::UNKNOWN && formal.is_some_and(|formal| nested_mention(formal.ty))
        })
    }

    /// Whether a substituted schema entry is a type the caller's scope can
    /// name: `dynamic` never infers, and a named type is decidable only
    /// when its head and every written type argument prove as types. Every
    /// other shape (an unresolvable head, an invalid argument, the
    /// bottom/`void` faces) refuses the substitution — the under-report
    /// posture: no invented types.
    pub(crate) fn schema_type_is_decidable(&self, ty: Ty) -> bool {
        match ty {
            Ty::Dynamic => true,
            Ty::Named { name, args, .. } => {
                self.named_type_is_type(name) && self.args_are_types(args)
            }
            _ => false,
        }
    }

    /// A dot-joined element head (`idl.IndexRelationKind`, interned whole
    /// by the prefixed type capture) resolves through its import prefix:
    /// the segment before the last dot names an import prefix of this
    /// library and the tail a declaration it brings. A head with no dot,
    /// an unknown prefix, or a prefixed miss keeps `false` — the bare-name
    /// tables own those.
    fn prefixed_element_head_resolves(&self, head: StringId) -> bool {
        let spelling = self.strings.resolve(head);
        let Some((prefix, tail)) = spelling.rsplit_once('.') else {
            return false;
        };
        if prefix.is_empty() || tail.is_empty() {
            return false;
        }
        let prefix = self.intern(prefix);
        let tail = self.intern(tail);
        matches!(
            self.namespaces
                .lookup_prefixed(self.graph, self.lib, prefix, tail),
            dartq_resolution::Lookup::Found(dartq_resolution::NamespaceEntry::Decl(_))
        )
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
    pub(crate) fn closure_callee_params(
        &self,
        recv: &Receiver,
        callee_name: StringId,
    ) -> Option<(
        Vec<(StringId, Option<Ty>)>,
        Vec<(StringId, Ty)>,
        Option<usize>,
    )> {
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
                let declaring = self.member_faces.last_origin().unwrap_or(container);
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
                // The explicit list pairs against the member's OWN
                // parameters only — and only when every declared one is
                // spelled (a count-only payload would let `fold<P>(...)`'s
                // `P` land on the container's `E`).
                let own = (params.len() == usize::from(entry.type_params)).then_some(params.len());
                let mut seeds: Vec<(StringId, Ty)> = Vec::new();
                if own_member {
                    if let Some(container_params) = self.bounds.container_params(container) {
                        if let Some(Ty::Named { args, .. }) = recv_ty {
                            let pool = self.boundary.pool();
                            let args = pool.get(args);
                            if args.len() == container_params.len() {
                                for ((param, _, _), ty) in container_params.iter().zip(args.iter())
                                {
                                    seeds.push((*param, *ty));
                                }
                            }
                        }
                        params.extend(container_params.iter().map(|(param, bound, _)| {
                            (*param, bound.as_ref().map(|tree| self.tree_ty(tree)))
                        }));
                    } else if let Some(recv_ty) = recv_ty
                        && let Some(env) = self.member_recv_env(recv_ty, container, &entry)
                    {
                        // A foreign container's parameter set spells nowhere
                        // in this unit's capture index; the member's own
                        // signature names it (`Map`'s `forEach(void
                        // action(K,V))`), and the direct receiver spelling
                        // `Map<String,R>` instantiates it positionally — the
                        // same env the send-close solver reads. The declared
                        // names join `params` boundless (the excerpt declares
                        // them unbounded).
                        for (param, ty) in env {
                            seeds.push((param, ty));
                            params.push((param, None));
                        }
                    }
                } else if let Some(recv_ty) = recv_ty
                    && let Some(env) = self.member_recv_env(recv_ty, declaring, &entry)
                {
                    // An inherited member: the receiver viewed as an
                    // instance of the declaring container spells the
                    // environment (`Lst<Item>` calling `Itr<E>.map` binds
                    // `E := Item` through the structural hierarchy).
                    for (param, ty) in env {
                        seeds.push((param, ty));
                        params.push((param, None));
                    }
                }
                return Some((params, seeds, own));
            }
            return None;
        }
        // A prefixed callee reads the prefix's import table — never this
        // unit's unprefixed tables (a local `min` must not bind `math.min`).
        if let Receiver::Prefix { name: prefix } = *recv {
            let dartq_resolution::Lookup::Found(dartq_resolution::NamespaceEntry::Decl(file)) =
                self.namespaces
                    .lookup_prefixed(self.graph, self.lib, prefix, callee_name)
            else {
                return None;
            };
            let decl = self.index.get(file).and_then(|f| f.decl(callee_name))?;
            return self.foreign_decl_params(&self.boundary.top(file, callee_name, decl));
        }
        // A same-file executable's captured bounds.
        if let Some(params) = self.exec_bounds.get(&callee_name)
            && !params.is_empty()
        {
            let own = params.len();
            return Some((params.to_vec(), Vec::new(), Some(own)));
        }
        // A function-typed variable callee's written signature.
        if let Some(sig) = self.fn_var_sigs.get(&callee_name)
            && !sig.params.is_empty()
        {
            let own = sig.params.len();
            return Some((sig.params.to_vec(), Vec::new(), Some(own)));
        }
        // A foreign top-level function's recovered spellings.
        self.top_level_decl(callee_name)
            .and_then(|decl| self.foreign_decl_params(&decl))
    }

    /// A foreign generic top-level function's inference-parameter
    /// spellings, recovered from its projected declaration: the declared
    /// spellings, when the collector carried them, are authoritative —
    /// declaration order, positional pairing, explicit type arguments fix
    /// against them. Otherwise (a bare-count `type_params` payload with no
    /// names): every bare leaf of its written types the caller's scope
    /// cannot prove is a type, in written order, capped at the declared
    /// arity, with no positional pairing (recovered order is written
    /// order, not declaration order).
    fn foreign_decl_params(
        &self,
        decl: &TopDecl,
    ) -> Option<(
        Vec<(StringId, Option<Ty>)>,
        Vec<(StringId, Ty)>,
        Option<usize>,
    )> {
        if decl.type_params == 0 {
            return None;
        }
        let arity = usize::from(decl.type_params);

        // A declaration carrying its parameter spellings — a generic
        // class (`Future<T>`'s `T`), a type alias, a generic top-level
        // function — pairs them positionally against the call's explicit
        // type arguments (the `ordered` arm `closure_schema_instantiate`
        // runs). The kind gate widened past `TopLevelFn` for the
        // constructor-callee face: `Future<Null>(() => 1)`'s callee is
        // `Future` the class, and its `T` is what the written `Null`
        // fixes.
        if decl.type_param_names.len() == arity {
            return Some((
                decl.type_param_names
                    .iter()
                    .map(|name| (*name, None))
                    .collect(),
                Vec::new(),
                Some(arity),
            ));
        }
        if decl.kind != MicroKind::TopLevelFn {
            return None;
        }
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
            None,
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
                    if !self.leaf_is_caller_type(name) && !out.contains(&name) {
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
                let (positional, named, return_ty) = (
                    shape.positional.clone(),
                    shape.named.clone(),
                    shape.return_ty,
                );
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

    /// The recovery's stricter face of [`Engine::named_type_is_type`]: a
    /// type-parameter declaration elsewhere in this unit proves nothing at
    /// the call. The annotation gate reads the file-global declaration
    /// table (a same-file parameter spelling is never an undefined class),
    /// but the foreign-callee parameter recovery asks what the CALL site
    /// can prove: `min`'s `T` is not `acc<T>`'s `T`, and counting the
    /// latter provable empties the recovery — the declared return then
    /// leaks its raw parameter spelling into every sink downstream. Only
    /// the frame latch's in-scope parameters prove; a `TypeParam`
    /// declaration hit through either lookup layer never does.
    fn leaf_is_caller_type(&self, name: StringId) -> bool {
        if self.members.primitive_ids.contains(&name)
            || self.type_params.contains(&name)
            || self.pending_type_params.contains(&name)
        {
            return true;
        }
        if let Some((container, _)) = self.container_context()
            && self
                .index
                .get(self.file)
                .and_then(|f| f.container(container))
                .is_some_and(|c| c.members.get(&name).is_some())
        {
            return false;
        }
        match lookup_scoped(
            self.graph,
            self.namespaces,
            self.index,
            self.lib,
            self.file,
            name,
        ) {
            ScopedHit::Local(decl) => {
                decl.kind != MicroKind::TypeParam && Self::decl_is_type(decl.kind)
            }
            ScopedHit::Found(NamespaceEntry::Decl(file)) => self
                .index
                .get(file)
                .and_then(|f| f.decl(name))
                .map_or(true, |decl| {
                    decl.kind != MicroKind::TypeParam && Self::decl_is_type(decl.kind)
                }),
            ScopedHit::Found(NamespaceEntry::Prefix) | ScopedHit::Ambiguous(_) => false,
            ScopedHit::NotFound => false,
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
                    nullable: true,
                    args,
                },
                arg,
            ) if args == ArgListId::EMPTY => {
                // A nullable-writable bare parameter (`writeOptionalObject<
                // T>(T? object, …)`): the formal's own top-level `?`
                // absorbs the argument's nullability — official `A? <: T?`
                // binds `T` the argument's NON-NULL head (probed: a
                // resolved `Tok2?` argument types the callback formal
                // `Tok2`; `extension on ManifestItemId?`'s `this` stays
                // silent on `it.write`, manifest_id.dart:150/157). The
                // non-null strip only lands on the arg's nullable named
                // face; every other shape (a `Null` literal, `dynamic`)
                // contributes itself, and the caller's decidable gate
                // still rejects heads the file cannot name — those keep
                // the unsolved fall-to-bound (`Object?`) route below
                // (ast_binary_writer.dart:864's unresolvable `Token?`).
                if params.iter().any(|(param, _)| *param == name) {
                    let bound = if let Ty::Named {
                        name: arg_name,
                        args: arg_args,
                        nullable: true,
                    } = arg
                    {
                        Ty::Named {
                            name: arg_name,
                            args: arg_args,
                            nullable: false,
                        }
                    } else {
                        arg
                    };
                    out.push((name, bound));
                }
            }
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
    pub(crate) fn object_question(&self) -> Ty {
        Ty::Named {
            name: self.intern("Object"),
            nullable: true,
            args: ArgListId::EMPTY,
        }
    }

    /// Per-argument invalid recovery for a chain receiver's element face:
    /// a decidable argument keeps itself; the oracle's `InvalidType`
    /// stand-ins (the unresolvable-import `Named` head, the `Unknown`
    /// face) take greatestClosure's `Object?` — the closure formal binds
    /// the nullable face and its member reads report
    /// (`_inferFormalParameters`'s recovery). Every other undecidable
    /// shape drops the whole chain (the pre-existing suppression).
    fn sanitize_invalid_args(&self, args: ArgListId, leak: &[StringId]) -> Option<ArgListId> {
        let list: Vec<Ty> = self.boundary.pool().get(args).to_vec();
        let mut out: Vec<Ty> = Vec::with_capacity(list.len());
        for arg in list {
            if self.schema_type_is_decidable(arg) {
                out.push(arg);
                continue;
            }
            if let Ty::Named { name, .. } = arg
                && leak.contains(&name)
            {
                return None;
            }
            match arg {
                Ty::Unknown => out.push(self.object_question()),
                // A head any graph unit declares is NOT invalid (the
                // getter's declared return spelling a foreign class the
                // diagnosing file never imports — the oracle's element
                // model holds the resolved class and the stand-in walk
                // below answers the silent arm).
                Ty::Named { name, .. }
                    if !self.named_type_is_type(name)
                        && !self.prefixed_element_head_resolves(name)
                        && self.graph.declaring_libraries(name).is_empty() =>
                {
                    out.push(self.object_question())
                }
                _ => return None,
            }
        }
        Some(self.boundary.intern_list(&out))
    }

    /// Every written type argument names a type — the oracle's argument
    /// walk: one unresolvable head makes the whole written type
    /// `InvalidType`.
    pub(crate) fn args_are_types(&self, args: ArgListId) -> bool {
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
    /// top-level declaration; a constructor selector reads the resolved
    /// container's `ctor_formals` table).
    fn callee_formals(&mut self, recv: &Receiver, name: StringId) -> Option<Formals> {
        match recv {
            // A bare class-name callee's constructor schema stays closed:
            // the class's own type parameters bind only through the
            // oracle's downward inference from the assignment context
            // (`SplayTreeMap((a, b) => …)`'s `K`/`V`), a face the
            // name-level walk cannot see — the formal keeps `dynamic`.
            Receiver::Unknown | Receiver::Prefix { .. } => self
                .top_level_decl(name)
                .and_then(|d| d.formals.as_deref().cloned()),
            // A constructor invocation (`Eb._(cb: (n) => …)`, the below
            // row a bare class name): constructors are not
            // [`MemberEntry`] rows, so the member walk never reaches
            // them — the container's own table answers as the fallback.
            Receiver::Container { name: class, .. } => self
                .member_formals_full(recv, name)
                .map(|f| f.into_owned())
                .or_else(|| self.ctor_schema_formals(*class, name)),
            _ => self
                .member_formals_full(recv, name)
                .map(|f| f.into_owned())
                .or_else(|| self.extension_member_formals(recv, name)),
        }
    }

    /// An applied extension member's formal table, the extension's own
    /// type parameters instantiated from the receiver's written arguments
    /// through the `on` clause (`extension M<K, V> on Map<K, V>`'s
    /// `mapValue<R>(R Function(V) f)` types the literal's `v` as the
    /// receiver's `V`): the container walk in
    /// [`Self::member_formals_full`] never reaches these — the member
    /// lives on the extension, not on the `on`-type's container, and the
    /// unindexed SDK receiver (`Map`) answers no member table of its own.
    /// The positional zip of the projected `on` type's arguments with the
    /// receiver's written arguments is the substitution (an `on` head
    /// without written arguments instantiates nothing); a receiver
    /// without written arguments leaves the formals as written.
    fn extension_member_formals(&mut self, recv: &Receiver, name: StringId) -> Option<Formals> {
        let Receiver::Typed { ty, .. } = *recv else {
            return None;
        };
        let Ty::Named {
            name: head, args, ..
        } = ty
        else {
            return None;
        };
        let pairs: Vec<(&'a ExtensionFace, FileId)> = self.accessible_extension_faces().to_vec();
        for (face, file) in pairs {
            if face.on_type != Some(head) {
                continue;
            }
            let Some(entry) = face.members.get(&name) else {
                continue;
            };
            if entry.kind != MemberKind::Method {
                continue;
            }
            let Some(formals) = entry.formals.as_deref() else {
                continue;
            };
            let mut formals = formals.clone();
            self.project_formals(file, &mut formals);
            // The instantiation gate: an extension whose `on` clause spells
            // its own type parameters instantiates ONLY from the
            // receiver's written arguments — a receiver that carries none
            // (an unsolved generic send's result) leaves the formal's
            // parameters spelling the extension's unsolved names, and the
            // schema machinery's `Object?` unresolvable fallback would
            // REFUSE arguments the oracle infers fine (the callback's
            // params are the oracle's solved types, which the name-level
            // model cannot derive). No honest instantiation keeps the
            // whole face off (the under-report direction). A
            // parameter-less extension's formals spell no extension
            // names and answer as written.
            if !face.type_param_names.is_empty() {
                let on_ty = self.boundary.ty(file, face.on_ty);
                let Ty::Named { args: on_args, .. } = on_ty else {
                    continue;
                };
                let written = self.boundary.pool().get(args).to_vec();
                let on_params = self.boundary.pool().get(on_args).to_vec();
                if on_params.len() != written.len() {
                    continue;
                }
                let keys: FxHashMap<StringId, Ty> = on_params
                    .iter()
                    .zip(written.iter().copied())
                    .filter_map(|(param, written)| match param {
                        Ty::Named { name, .. } => Some((*name, written)),
                        _ => None,
                    })
                    .collect();
                if keys.len() != face.type_param_names.len() {
                    continue;
                }
                let all = formals
                    .positional
                    .iter_mut()
                    .map(|f| &mut f.ty)
                    .chain(formals.named.iter_mut().map(|f| &mut f.ty));
                for formal_ty in all {
                    *formal_ty = self.substitute_ty(*formal_ty, &keys);
                }
            }
            return Some(formals);
        }
        None
    }

    /// A constructor invocation's formal table as a closure-context
    /// schema source: the literal at an argument slot
    /// (`Eb._(reportError: (scopeName, node) { … })`) infers its formals
    /// from the selected constructor's parameters exactly like a function
    /// call's — an unannotated field formal (`required this.reportError`)
    /// carries the field's declared type. `check_arguments`'s same faces
    /// apply: the foreign-pool projection and the field-type patch
    /// (same-file only; a foreign class's unannotated field formal stays
    /// `Unknown` and the schema stays silent — the under-report posture).
    pub(crate) fn ctor_schema_formals(
        &self,
        class: StringId,
        selector: StringId,
    ) -> Option<Formals> {
        let (container, owner) = resolve_container(
            self.graph,
            self.namespaces,
            self.index,
            self.lib,
            self.file,
            class,
        )?;
        let mut formals = (**container.ctor_formals.get(&selector)?).clone();
        self.project_formals(owner, &mut formals);
        self.patch_field_formals(class, &mut formals);
        Some(formals)
    }

    /// The factory-declaration formals lane ([`Container::factory_formals`]):
    /// the same resolution and projection as
    /// [`Self::ctor_schema_formals`] over the factory-only table — a
    /// redirecting factory's own written parameter list, served to the
    /// argument-check pairing alone.
    pub(crate) fn factory_ctor_formals(
        &self,
        class: StringId,
        selector: StringId,
    ) -> Option<Formals> {
        let (container, owner) = resolve_container(
            self.graph,
            self.namespaces,
            self.index,
            self.lib,
            self.file,
            class,
        )?;
        let mut formals = (**container.factory_formals.get(&selector)?).clone();
        self.project_formals(owner, &mut formals);
        self.patch_field_formals(class, &mut formals);
        Some(formals)
    }

    /// Publishes one resolved constructor-use edge (#2931): a `member_refs`
    /// row the annotation verifier's `member: Some` arm drains — its
    /// `ctor_annotations` fallback answers the deprecated-constructor face
    /// (constructors carry no [`MemberEntry`] row, so a member-shaped edge
    /// is the only substrate the verifier reads). Upstream fires from
    /// `ElementUsageDetector.constructorName` reading `node.element`'s
    /// metadata (upstream Dart SDK source, pkg/analyzer/lib/src/error/
    /// element_usage_detector.dart:189-191, driven from
    /// `best_practices_verifier.dart` `visitConstructorName`:321-324).
    ///
    /// `selector` is the written selector (`named` in `A.named(`, `new` in
    /// `A.new(`, the class name for the bare `A(`); it normalizes to the
    /// `ctor_formals` lookup spelling the star keys under — `new` reads as
    /// the class name, the same rewrite `note_constructor_continuation`
    /// applies at capture. `range` is the `ConstructorName` span upstream
    /// anchors (the class+selector compound for a named creation, the
    /// head token for the unnamed face). An unresolvable container, a
    /// genuinely absent constructor face, or a constructor whose
    /// declaration carries no annotations mints nothing — the oracle's
    /// `node.element == null` silence, plus #2945's annotation gate.
    pub(crate) fn note_ctor_use(
        &mut self,
        class: StringId,
        selector: StringId,
        range: SourceRange,
    ) {
        let Some((decl, file)) = resolve_container(
            self.graph,
            self.namespaces,
            self.index,
            self.lib,
            self.file,
            class,
        ) else {
            return;
        };
        self.note_ctor_use_in(decl, file, class, selector, range);
    }

    /// [`Self::note_ctor_use`] with the declaring container already
    /// resolved — the prefixed fold's `prefixed_container` hit carries the
    /// owner the bare namespace walk would look up in the wrong scope.
    pub(crate) fn note_ctor_use_in(
        &mut self,
        decl: &Container,
        file: FileId,
        class: StringId,
        selector: StringId,
        range: SourceRange,
    ) {
        if !self.ctor_face_exists(decl, class, selector) {
            return;
        }
        let key = if selector == self.intern("new") {
            class
        } else {
            selector
        };
        // #2945: the edge exists only to carry the declaration's metadata —
        // the verifier's `member: Some` arm drains `ctor_annotations` and a
        // constructor carrying no MemberEntry row has no other annotations a
        // member-shaped edge could name. An unannotated ctor call is still a
        // real use upstream (the class/type-use rows mark it), but a
        // member-shaped index row for it is spurious: #2618's zero-edge
        // contract, and it stops `RefRow`s for plain `Host()` calls feeding
        // the reachability/query surfaces.
        let annotated = self
            .index
            .get(file)
            .and_then(|elements| elements.ctor_annotations(class, key))
            .is_some_and(|annotations| !annotations.is_empty());
        if !annotated {
            return;
        }
        self.report.member_refs.push(MemberRef {
            range,
            kind: MemberRefKind::Invoked,
            target_file: file,
            target_name: key,
            target_container: class,
        });
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

    /// The context type `K` a function literal opens under
    /// (`inferLocalFunction`, inference_visitor_base.dart:2229): the first
    /// available of four channels — (i) the declaration initializer's
    /// written type folded through its alias (`Function2<int, String> f =
    /// (x) => …` reads `String Function(int)`); (ii) the assignment
    /// target's declared type when the literal IS the right-hand side
    /// (`f = (x) => …`); (iii) the argument slot's formal type in the
    /// open invocation (`useNum((x) => …)`); (iv) the enclosing
    /// executable's function-shaped return position (`return (x) => …`).
    /// Every channel answers the folded [`Ty::Function`] itself — the
    /// literal's formals, return checks, and final Value-row type all
    /// read it. A non-function or undecidable context answers `None`
    /// (dynamic, a bare `Function` head, an unresolvable shape) — the
    /// pre-existing under-report posture stays.
    /// Returns the latched context plus `true` when the argument channel
    /// supplied it — the per-formal bind keeps the live re-derive path
    /// there (the receiver below the callee re-binds at pass 2, the
    /// begin-time latch is its stale face).
    pub(crate) fn closure_context_ty(&mut self, literal_offset: u32) -> (Option<Ty>, bool) {
        let mark = self.rows.len();
        // (i) The declaration initializer: the literal opened exactly at
        // the initializer's mark (a deeper literal inside it opened after
        // a callee's row and misses). `Function2<int, String>` folds to
        // `String Function(int)` — the alias the raw `pending_decl_ty`
        // misses.
        if self.init_mark == Some(mark)
            && let Some(ty) = self.pending_decl_ty
        {
            let folded = self.fold_alias_ty(ty);
            if let Some(ctx) = self.function_ty_of(folded) {
                return (Some(ctx), false);
            }
            // The old-style typedef's element-model face: `typedef T
            // Function2<S, T>(S x)` spells its `Ty::Function` RHS through
            // `decl.var_ty`, with the written arguments pairing through
            // the shape's own `type_params_named` — `instantiate_alias`'s
            // `type_param_names` gate misses this spelling entirely.
            if let Ty::Named { name, args, .. } = folded
                && args != ArgListId::EMPTY
                && let Some(ctx) = self.closure_typedef_function_ty(name, args)
            {
                return (Some(ctx), false);
            }
        }
        // (ii) The assignment right-hand side: the token directly before
        // the literal is `=`/`??=` and the closest non-value row before it
        // is the write target. `x = (e) => 3` over a `Function2` field
        // reads the folded target face; a literal deeper in the RHS (a
        // send's argument, a `??` operand) sits behind a `(`/`?`/`,`
        // token and never matches.
        if let Some(ctx) = self.assignment_ctx_at(literal_offset) {
            return (Some(ctx), false);
        }
        // (iii) The argument slot: the literal opened exactly where the
        // pending argument's rows begin — `arg_open` holds between
        // `open_argument` and the flush, `mark` is the row count the
        // argument's own rows start at. The slot's declared formal folds
        // to the schema function; the call's inference parameters already
        // substituted ride the instantiated answer.
        if let Some(ctx) = self.argument_ctx_at(mark) {
            return (Some(ctx), true);
        }
        // (iv) The enclosing executable's return position: the literal
        // opened exactly where the in-flight `return` began and the
        // enclosing declared return is function-shaped — `return (x) => …`
        // inside `Function2<int, String> f() { … }`. The mark holds for
        // arrow AND block bodies (the body's own returns re-arm
        // `return_mark` inside the literal, where the capture already
        // latched the context — the false direction the old mark-equality
        // read could not separate). The `=>`-body arm (`body_mark` latches
        // where the body's expression begins) is the same position spelled
        // without `return`: `f() => (x) => …` — gated on the enclosing
        // body's brace being absent, so a block body's first-statement
        // literal (`f() { (x) => …; }`) never takes the channel.
        let at_return = self.return_mark == Some(mark);
        let at_arrow_body = self.body_mark == Some(mark)
            && self.frames.iter().rev().any(|f| match &f.kind {
                FrameKind::Executable {
                    ret: Some(_),
                    body_brace,
                    ..
                } => body_brace.is_none(),
                _ => false,
            });
        if at_return || at_arrow_body {
            // The enclosing return context: the nearest declared `ret`
            // answers its folded face; an enclosing literal's own matched
            // context supplies ITS return first (a literal nested in a
            // literal's `=>` body infers under the outer's context
            // return, never the outer declaration's — `(x) => (y) => …`
            // under `Function2<int, String>` reads `String`, which is no
            // function type, so the inner literal acquires nothing).
            for (index, frame) in self.frames.iter().enumerate().rev() {
                match &frame.kind {
                    FrameKind::Executable { ret: Some(ret), .. } => {
                        return (self.function_ty_of(self.fold_alias_ty(ret.ty)), false);
                    }
                    FrameKind::Executable { ret: None, .. } => {
                        if let Some(capture) = self
                            .closure_caps
                            .iter()
                            .find(|capture| capture.frame_depth == index + 1)
                            && let Some(ctx) = capture.ctx
                            && self.closure_ctx_matches(ctx, capture.type_params)
                        {
                            return (
                                self.function_shape_return(ctx)
                                    .and_then(|ret| self.function_ty_of(self.fold_alias_ty(ret))),
                                false,
                            );
                        }
                    }
                    _ => {}
                }
            }
        }
        (None, false)
    }

    /// The assignment-channel context at `literal_offset`: a bare `=` or
    /// `??=` directly ahead of the literal (the token table's spelling —
    /// `==`, `=>`, `>=` never spell `=`; the literal's own offset is its
    /// first token, so the token before it is the operator), the closest
    /// non-value row before it the write target, whose declared type
    /// folds to the schema. `None` on every other shape (an operator, a
    /// comma, an open paren — all non-assignment positions).
    fn assignment_ctx_at(&mut self, literal_offset: u32) -> Option<Ty> {
        let tokens = self.tokens.as_slice();
        let index = tokens
            .partition_point(|t| t.utf16_offset < literal_offset)
            .checked_sub(1)?;
        let text = &self.source[tokens[index].byte_range()];
        if text != "=" && text != "??=" {
            return None;
        }
        let limit = tokens[index].utf16_offset;
        let target = self
            .rows
            .iter()
            .rev()
            .find(|row| row.range.end() <= limit && !matches!(row.shape, RowShape::Value))?
            .clone();
        // `c[i] = e` writes through `[]=` — the operator row's declared
        // face is not the assignment's destination (the same gate
        // `check_assignment` runs).
        if matches!(target.intent, Intent::Operator(..)) {
            return None;
        }
        let ty = self.assignment_target_ty(&target);
        self.function_ty_of(self.fold_alias_ty(ty))
    }

    /// The argument-slot channel: the literal IS the pending argument of
    /// the open invocation (its rows begin at `mark`). The slot pairs
    /// like [`Self::closure_param_context_ty`]'s — `pending_name` or the
    /// completed positional count — and the slot's declared formal folds
    /// through the same schema face (an alias spelling, a written
    /// signature table entry, then the inference-parameter instantiation
    /// the bind-time solver runs). `None` when the literal sits deeper
    /// (a nested collection, a named value mid-spelling) or the callee
    /// answers no function-shaped formal.
    fn argument_ctx_at(&mut self, mark: usize) -> Option<Ty> {
        let level = self.arg_levels.last()?;
        if level.mark != mark {
            return None;
        }
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
        let callee = self.rows.get(callee_row)?.clone();
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
        let callee_name = callee.name;
        // The slot's own declared type — the schema function the literal
        // matches. The element formals answer first; a function-typed
        // variable callee's captured signature closes (the same two
        // lanes [`Self::closure_context_schema`] opens).
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
        })?;
        // Fold through the schema face: an alias spelling yields the
        // signature; the call's solved parameters substitute in (the
        // upward face the bind-time solver runs — `map`'s `E` from the
        // receiver's written instantiation).
        let folded = self.fold_alias_ty(slot_ty);
        if folded != slot_ty {
            return self.function_ty_of(folded);
        }
        // The schema's own inference parameters gate the latch: the call
        // solved them (explicit type arguments, the receiver's written
        // instantiation, the arguments already typed) and the whole
        // schema substitutes decidable — `fold<Fo?>(null, (result,
        // folder) …)` reads `Fo? Function(Fo?, Fo?)`. An unsolved or
        // half-solved schema keeps the literal context-free — the
        // bind-time face pairs its own channels per formal, and a raw
        // `T Function(T)` latch would adjudicate the body's returns
        // against a bare callee parameter (the oracle's context is the
        // solved signature; an unsolved `T` is no context at all).
        let inst = self.closure_schema_instantiate(slot_ty, &recv, callee_name, Some(callee_row));
        match inst {
            Some(ty) => self.function_ty_of(ty),
            None => {
                let has_open_param = self
                    .closure_callee_params(&recv, callee_name)
                    .map(|(params, ..)| {
                        let names: Vec<StringId> = params.iter().map(|(name, _)| *name).collect();
                        !names.is_empty() && self.ty_names_any(slot_ty, &names)
                    })
                    .unwrap_or(false);
                if has_open_param {
                    None
                } else {
                    self.function_ty_of(slot_ty)
                }
            }
        }
    }

    /// `ty`'s shape mentions any of `names` as a bare head — the callee
    /// parameter a failed solve left standing (`T Function(T)`'s `T`s).
    fn ty_names_any(&self, ty: Ty, names: &[StringId]) -> bool {
        match ty {
            Ty::Named { name, args, .. } => {
                names.contains(&name)
                    || (args != ArgListId::EMPTY && {
                        let list = self.boundary.args.borrow().get(args).to_vec();
                        list.iter().any(|ty| self.ty_names_any(*ty, names))
                    })
            }
            Ty::Function { shape, .. } => {
                let sig = {
                    let pool = self.boundary.pool();
                    pool.function_shape(shape).clone()
                };
                self.ty_names_any(sig.return_ty, names)
                    || sig
                        .positional
                        .iter()
                        .chain(sig.named.iter().map(|(_, ty)| ty))
                        .any(|ty| self.ty_names_any(*ty, names))
            }
            _ => false,
        }
    }

    /// `ty` when it spells a function type — the context-shape gate
    /// `inferLocalFunction`'s `typeContext is FunctionType` applies. The
    /// bare `Function` head is a `Ty::Named` here (the parser fires a
    /// plain type completion for it), so a non-function spelling, a bare
    /// `Function`, `dynamic`, and the undecidable faces all answer
    /// `None` — the no-context posture.
    fn function_ty_of(&self, ty: Ty) -> Option<Ty> {
        match ty {
            Ty::Function { .. } => Some(ty),
            _ => None,
        }
    }

    /// Whether the matched context `ctx` admits the literal's own
    /// type-parameter count (`typeContext.typeParameters.length ==
    /// functionTypeParameters.length` — the upstream match gate). A
    /// count-miss acquires NOTHING (the literal's own group is a fresh
    /// quantification the context's instantiator cannot read).
    pub(crate) fn closure_ctx_matches(&self, ctx: Ty, literal_type_params: u16) -> bool {
        match ctx {
            Ty::Function { shape, .. } => {
                self.boundary.pool().function_shape(shape).type_params == literal_type_params
            }
            _ => false,
        }
    }

    /// The downward-inferred type of the literal's `index`-th positional
    /// formal under the matched context `ctx` (`_inferFormalParameters`):
    /// `greatestClosure(Qi)` — a decidable named schema entry binds
    /// itself; `T <: Null` (the unresolvable head, `Null`, `Never`) and
    /// the invalid faces bind `Object?`; `dynamic` supplies nothing and
    /// the formal keeps `dynamic`. Positional pairing by declaration
    /// order is the only face this name-level model carries (named
    /// literals poison before here).
    pub(crate) fn closure_ctx_formal_ty(&self, ctx: Ty, index: usize) -> Option<Ty> {
        let Ty::Function { shape, .. } = ctx else {
            return None;
        };
        let schema = {
            let pool = self.boundary.pool();
            *pool.function_shape(shape).positional.get(index)?
        };
        self.closure_ctx_greatest_closure(schema)
    }

    /// `greatestClosure(Qi)` for one context schema entry
    /// (`computeGreatestClosure2`'s face at name level): a decidable named
    /// schema entry binds itself; `T <: Null` — `Null`, `Never`, and the
    /// invalid/unresolvable heads — binds `Object?`; `dynamic` supplies
    /// nothing (the formal keeps `dynamic`, `None` here). Function and
    /// record shapes greatest-close to their own face.
    fn closure_ctx_greatest_closure(&self, schema: Ty) -> Option<Ty> {
        match schema {
            // `dynamic` never infers — the formal keeps `dynamic`.
            Ty::Dynamic => None,
            // `T <: Null` and the invalid faces: `Object?`.
            Ty::Unknown | Ty::Null | Ty::Never => Some(self.object_question()),
            Ty::Named {
                name,
                nullable,
                args,
            } => {
                if self.named_type_is_type(name) && self.args_are_types(args) {
                    Some(Ty::Named {
                        name,
                        nullable,
                        args,
                    })
                } else {
                    Some(self.object_question())
                }
            }
            // Function/record schema entries bind themselves (a
            // function-typed `Qi` greatest-closes to its own shape).
            other => Some(other),
        }
    }

    /// The instantiated function type an old-style (function) typedef's
    /// use spells — `Function2<int, String>` for `typedef T Function2<S,
    /// T>(S x)` — the channel-(i) fallback when [`Self::fold_alias_ty`]
    /// yields no `Ty::Function`. The element model is the only whole
    /// record the old spelling arms: the collector synthesizes the RHS
    /// `Ty::Function` into `decl.var_ty` (its `type_params_named` pairs
    /// the written arguments positionally), while `decl.type_param_names`
    /// stays empty (the count-only payload `instantiate_alias` refuses —
    /// resolution.rs:234). `None` for a non-alias head, a non-function
    /// `var_ty`, or an arity mismatch.
    fn closure_typedef_function_ty(&self, name: StringId, args: ArgListId) -> Option<Ty> {
        let (owner, decl) = match lookup_scoped(
            self.graph,
            self.namespaces,
            self.index,
            self.lib,
            self.file,
            name,
        ) {
            ScopedHit::Local(decl) => (self.file, decl),
            ScopedHit::Found(NamespaceEntry::Decl(file)) => {
                (file, self.index.get(file).and_then(|f| f.decl(name))?)
            }
            _ => return None,
        };
        if decl.kind != MicroKind::TypeAlias || decl.var_ty == Ty::UNKNOWN {
            return None;
        }
        let Ty::Function { shape, nullable } = self.boundary.ty(owner, decl.var_ty) else {
            return None;
        };
        let (type_params_named, return_ty, positional, named) = {
            let pool = self.boundary.pool();
            let shape = pool.function_shape(shape);
            (
                shape.type_params_named.clone(),
                shape.return_ty,
                shape.positional.to_vec(),
                shape.named.to_vec(),
            )
        };
        if type_params_named.is_empty() {
            // A non-generic old-style typedef's `var_ty` IS the answer —
            // `fold_alias_ty` already folded it when it could; reaching
            // here means the RHS carried no names worth substituting.
            return Some(Ty::Function { shape, nullable });
        }
        let written: Vec<Ty> = self.boundary.pool().get(args).to_vec();
        if written.len() != type_params_named.len() {
            return None;
        }
        let map: FxHashMap<StringId, Ty> = type_params_named
            .iter()
            .map(|(param, _)| *param)
            .zip(written)
            .collect();
        let ret = self.closure_subst_param_ty(return_ty, &map);
        let positional: Vec<Ty> = positional
            .iter()
            .map(|ty| self.closure_subst_param_ty(*ty, &map))
            .collect();
        let named: Vec<(StringId, Ty)> = named
            .iter()
            .map(|(n, ty)| (*n, self.closure_subst_param_ty(*ty, &map)))
            .collect();
        // The substituted shape is a concrete signature — the typedef's
        // own parameter group is consumed by the instantiation, so the
        // answer carries no `type_params` of its own.
        Some(
            self.boundary
                .args
                .borrow_mut()
                .function(0, ret, &positional, &named, nullable),
        )
    }

    /// The use-site substitution of one typedef's parameter spellings:
    /// a bare leaf naming a parameter binds the written argument (its
    /// nullability joining the leaf's); a generic head substitutes
    /// pairwise through its argument list; a function subtree substitutes
    /// through return, positionals, named, and written bounds. Every
    /// other face stands — a non-parameter leaf is some other
    /// declaration's spelling, never this alias's.
    fn closure_subst_param_ty(&self, ty: Ty, map: &FxHashMap<StringId, Ty>) -> Ty {
        if let Ty::Named {
            name,
            nullable,
            args: ArgListId::EMPTY,
        } = ty
            && let Some(replacement) = map.get(&name)
        {
            // The written argument's type joins the leaf's own
            // nullability — `T?` under `T := int` spells `int?`.
            if !nullable {
                return *replacement;
            }
            return match *replacement {
                Ty::Named { name, args, .. } => Ty::Named {
                    name,
                    nullable: true,
                    args,
                },
                Ty::Record { shape, .. } => Ty::Record {
                    shape,
                    nullable: true,
                },
                Ty::Function { shape, .. } => Ty::Function {
                    shape,
                    nullable: true,
                },
                other => other,
            };
        }
        match ty {
            Ty::Named {
                name,
                nullable,
                args,
            } if args != ArgListId::EMPTY => {
                let written: Vec<Ty> = {
                    let pool = self.boundary.pool();
                    pool.get(args).to_vec()
                };
                let substituted: Vec<Ty> = written
                    .iter()
                    .map(|arg| self.closure_subst_param_ty(*arg, map))
                    .collect();
                self.boundary
                    .args
                    .borrow_mut()
                    .named(name, nullable, &substituted)
            }
            Ty::Function { shape, nullable } => {
                let (type_params, names, return_ty, positional, named) = {
                    let pool = self.boundary.pool();
                    let shape = pool.function_shape(shape);
                    (
                        shape.type_params,
                        shape.type_params_named.clone(),
                        shape.return_ty,
                        shape.positional.to_vec(),
                        shape.named.to_vec(),
                    )
                };
                let positional: Vec<Ty> = positional
                    .iter()
                    .map(|ty| self.closure_subst_param_ty(*ty, map))
                    .collect();
                let named: Vec<(StringId, Ty)> = named
                    .iter()
                    .map(|(n, ty)| (*n, self.closure_subst_param_ty(*ty, map)))
                    .collect();
                let names: Vec<(StringId, Option<Ty>)> = names
                    .iter()
                    .map(|&(n, bound)| (n, bound.map(|b| self.closure_subst_param_ty(b, map))))
                    .collect();
                let mut args = self.boundary.args.borrow_mut();
                if names.is_empty() && type_params > 0 {
                    args.function(
                        type_params,
                        self.closure_subst_param_ty(return_ty, map),
                        &positional,
                        &named,
                        nullable,
                    )
                } else {
                    args.function_generic(
                        &names,
                        self.closure_subst_param_ty(return_ty, map),
                        &positional,
                        &named,
                        nullable,
                    )
                }
            }
            _ => ty,
        }
    }

    /// One written argument subtree as a body-level [`Ty`]: the head
    /// canonicalizes like the annotation head (`dynamic`/`void`/`Null`),
    /// and the subtree interns bottom-up into the boundary pool (#611) so
    /// nested written arguments compare structurally. A poisoned subtree
    /// keeps the name-only shape — its deeper structure is recovery
    /// residue, never a written fact. A function-form node folds through
    /// its own arm into the interned [`Ty::Function`] (the collector's
    /// annotation arm applies the same discipline); a poisoned one
    /// suppresses to [`Ty::Unknown`], never a fabricated shape.
    pub(crate) fn tree_ty(&self, tree: &crate::capture::TypeTree) -> Ty {
        if let Some(fn_tree) = tree.function.as_ref() {
            if tree.poisoned {
                return Ty::Unknown;
            }
            let ret = fn_tree
                .ret
                .as_deref()
                .map_or(Ty::Dynamic, |ret| self.tree_ty(ret));
            let positional: Vec<Ty> = fn_tree.positional.iter().map(|f| self.tree_ty(f)).collect();
            let named: Vec<(StringId, Ty)> = fn_tree
                .named
                .iter()
                .map(|(name, f)| (*name, self.tree_ty(f)))
                .collect();
            // The signature's own group interns beside the shape (#2878),
            // the same column the diagnose-side fn capture carries.
            let type_params: Vec<(StringId, Option<Ty>)> = fn_tree
                .type_params_named
                .iter()
                .map(|(name, bound)| (*name, bound.as_ref().map(|b| self.tree_ty(b))))
                .collect();
            return self.boundary.args.borrow_mut().function_generic(
                &type_params,
                ret,
                &positional,
                &named,
                tree.nullable,
            );
        }
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

    /// Whether a receiver argument's ALIAS spelling stays provable after
    /// the right-hand fold: an alias resolving to an unprovable head (a
    /// typedef of a prefixed import the package config cannot resolve) is
    /// the oracle's `InvalidType` at the use — the closure formal binds
    /// greatestClosure's `Object?`, never the silent arm. A non-alias
    /// spelling folds to itself and keeps whatever the caller's gate
    /// already decided.
    fn folded_alias_arg_provable(&self, arg_head: StringId) -> bool {
        match self.fold_alias_ty(Ty::named(arg_head)) {
            // `dynamic` never infers — the silent arm.
            Ty::Dynamic => true,
            Ty::Named { name, .. } => {
                self.named_type_is_type(name)
                    || !self.graph.declaring_libraries(name).is_empty()
                    || self.prefixed_element_head_resolves(name)
            }
            _ => false,
        }
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
        // unprovable names fail the decidability gate below). A field's
        // implicit getter answers its declared type the same way.
        if let Walk::Found(entry) = self.walk_members(*type_name, row.name)
            && matches!(entry.kind, MemberKind::Getter | MemberKind::Field)
        {
            let ret = self.fold_alias_ty(if entry.kind == MemberKind::Field {
                entry.var_ty
            } else {
                entry.return_ty
            });
            let Ty::Named { name, nullable, .. } = ret else {
                return Receiver::Unknown;
            };
            let ret = match self
                .bounds
                .container_params(*type_name)
                .filter(|params| !params.is_empty())
                .or_else(|| {
                    // An SDK names-only stub's getter rows carry no
                    // parameter spellings (`Map.values`' raw `Iterable<V>`)
                    // — the dart:core table supplies the head's own, the
                    // same fallback `instantiate_accessor_static` applies;
                    // without it the raw schema leaks and the decidability
                    // gate below suppresses the whole link.
                    crate::member_face::CORE_CONTAINER_PARAMS
                        .iter()
                        .find(|(head, ..)| *head == self.strings.resolve(*type_name))
                        .map(|(_, params)| {
                            params
                                .iter()
                                .map(|p| (self.intern(p), None, false))
                                .collect::<Vec<_>>()
                        })
                })
                .or_else(|| {
                    // A cross-file container (`StreamController<T>.stream`
                    // read in a user unit): its declaring unit's parameter
                    // spellings instantiate the getter's declared return.
                    self.foreign_container_params(*type_name).map(|names| {
                        names
                            .into_iter()
                            .map(|p| (p, None, false))
                            .collect::<Vec<_>>()
                    })
                }) {
                Some(params) if !params.is_empty() => {
                    let Ty::Named {
                        args: recv_args, ..
                    } = *ty
                    else {
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
            // result's own receiver. The result's arguments run the same
            // invalid recovery as the producer table above (`Iterable<
            // Token>` with an unresolvable `Token` chains on `Object?`
            // elements); a deeper undecidable shape keeps the drop.
            let Ty::Named { args, .. } = ret else {
                return Receiver::Unknown;
            };
            if nullable || !self.named_type_is_type(name) {
                return Receiver::Unknown;
            }
            // The declaring container's own type-parameter spellings: an
            // uninstantiated stub getter's declared return leaks them raw
            // (`Iterable<V>` of `HashMap.values` when the substitution
            // found no parameter table) — a spelling, not a written type,
            // and never the invalid-import face this recovery exists for.
            let mut leak: Vec<StringId> = Vec::new();
            if let Some(params) = self.bounds.container_params(*type_name) {
                leak.extend(params.iter().map(|(param, ..)| *param));
            }
            let head = self.strings.resolve(*type_name);
            // The alias roots' parameter spellings stand in for the
            // collection head the bounds and core tables cannot see
            // (`HashMap` reaches `Map`'s `K`/`V`).
            let mut heads = vec![head];
            if let Some((_, roots)) = crate::member_face::COLLECTION_INTERFACE_ALIASES
                .iter()
                .find(|(alias, _)| *alias == head)
            {
                heads.extend(roots.iter().copied());
            }
            for head in heads {
                if let Some((_, params)) = crate::member_face::CORE_CONTAINER_PARAMS
                    .iter()
                    .find(|(core, _)| *core == head)
                {
                    leak.extend(params.iter().map(|param| self.intern(param)));
                }
            }
            let Some(args) = self.sanitize_invalid_args(args, &leak) else {
                return Receiver::Unknown;
            };
            return Receiver::Typed {
                type_name: name,
                ty: Ty::Named {
                    name,
                    nullable,
                    args,
                },
            };
        }
        // The SDK's unindexed units chain-break, so the frozen producer
        // table stands in: the declared `Iterable<E>` face reads its
        // element from the receiver's written type argument at the row's
        // index (the [`CORE_STATIC_RETURNS`] precedent for an unindexed
        // declared return).
        let head = self.strings.resolve(*type_name);
        let Some((_, _, index, result)) = CHAIN_PRODUCER_GETTERS
            .iter()
            .find(|(container, getter, ..)| *container == head && self.intern(getter) == row.name)
        else {
            return Receiver::Unknown;
        };
        let Ty::Named { args, .. } = *ty else {
            return Receiver::Unknown;
        };
        let Some(element) = self.boundary.pool().get(args).get(*index).copied() else {
            return Receiver::Unknown;
        };
        // greatestClosure(InvalidType) = `Object?`: the unresolvable-import
        // element (`Map<String, YamlScalar>` with `package:yaml` absent)
        // still forms the chain — the closure formal binds the nullable
        // face and its member reads report. A deeper undecidable shape
        // keeps the drop.
        let element = if self.schema_type_is_decidable(element) {
            element
        } else {
            match element {
                Ty::Unknown => self.object_question(),
                // A head any graph unit declares is not the oracle's
                // `InvalidType` (a foreign-but-real element class): the
                // element keeps itself and the schema's stand-in walk
                // answers the silent arm.
                Ty::Named { name, .. }
                    if !self.named_type_is_type(name)
                        && !self.prefixed_element_head_resolves(name)
                        && self.graph.declaring_libraries(name).is_empty() =>
                {
                    self.object_question()
                }
                _ => return Receiver::Unknown,
            }
        };
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
            ctx_ret: None,
            super_invoked: false,
            body_brace: None,
            is_async: false,
            generator: false,
        });
    }

    /// Arms a function literal's completion context (the official
    /// `imposedType`): the context type's function return, with the
    /// dynamic/unknown heads dropped — those leave `contextType` null and
    /// the completion walk silent (`function_expression_resolver.dart`
    /// :44-53). A context that answers no function return (an untyped
    /// slot) stays unarmed — the same under-report. Called at the
    /// literal's begin, while its own frame is innermost; the closure
    /// staging reads the arm at the literal's end.
    pub(crate) fn arm_closure_completion(&mut self, ctx: Option<Ty>) {
        let ctx_ret = ctx
            .and_then(|ctx| self.function_shape_return(ctx))
            .filter(|ret| !matches!(ret, Ty::Dynamic | Ty::Unknown));
        if let Some(frame) = self.frames.last_mut()
            && let FrameKind::Executable { ctx_ret: slot, .. } = &mut frame.kind
        {
            *slot = ctx_ret;
        }
    }

    /// Latches the body's async modifier onto the innermost executable
    /// frame (`handle_async_modifier`): the completion warning's generator
    /// and async-legality gates read these bits (`checkForBodyMayComplete
    /// Normally`'s `body.isGenerator` / `body.isAsynchronous` arms). The
    /// modifier event belongs to the body whose frame is innermost — a
    /// nested function literal latches its own frame.
    ///
    /// For a non-generator `async` literal the matched context's return
    /// type additionally rewrites: the oracle's `element.returnType` is
    /// the imposed `R` async-wrapped through `Future<flatten(R)>`
    /// (body_inference_context.dart:132-153, `computeInferredReturnType`
    /// — a body whose returns do not satisfy the context clamps to `R`,
    /// and `FutureOr<futureValueType(imposedType)>` at :259-261 supplies
    /// the async body's context). `void Function() test = () async {
    /// return v; }` reports `'Future<void>'` as the destination, not the
    /// raw `void` — the literal's own `Ty::Function` face carries the
    /// wrapped return for both the verdict and the Value row. Formals
    /// bind before this event fires, so the rewrite never disturbs the
    /// per-formal inference arm.
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
        if !is_async || generator {
            return;
        }
        // The read-only phase: extract the latched context's shape
        // fields and compute the async-wrapped return — the mutable
        // borrow resumes only for the assignment, so `intern`/`pool`/
        // `intern_list` never fight the `last_mut` borrow.
        let Some(Ty::Function { shape, nullable }) =
            self.closure_caps.last().and_then(|capture| capture.ctx)
        else {
            return;
        };
        let (type_params, names, return_ty, positional, named) = {
            let pool = self.boundary.pool();
            let shape = pool.function_shape(shape);
            (
                shape.type_params,
                shape.type_params_named.clone(),
                shape.return_ty,
                shape.positional.to_vec(),
                shape.named.to_vec(),
            )
        };
        let flat = engine_core::flatten_ty(self.strings, &self.boundary.pool(), return_ty);
        let future = Ty::Named {
            name: self.intern("Future"),
            nullable: false,
            args: self.boundary.intern_list(&[flat]),
        };
        let wrapped = if names.is_empty() && type_params > 0 {
            self.boundary.args.borrow_mut().function(
                type_params,
                future,
                &positional,
                &named,
                nullable,
            )
        } else {
            self.boundary.args.borrow_mut().function_generic(
                &names,
                future,
                &positional,
                &named,
                nullable,
            )
        };
        if let Some(capture) = self.closure_caps.last_mut() {
            capture.ctx = Some(wrapped);
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
            // The ctor's OWN body brace ends the initializer window: only
            // the ctor frame latches it (a closure above would swallow the
            // note for its own brace — `frames.len()` equality rejects
            // that, and a closure brace inside an initializer never opens
            // the window early).
            if self.ctor_pending.is_some() && self.frames.len() == self.ctor_frame_len {
                self.ctor_body_open = true;
            }
        }
    }

    /// The executable declaration's completion staging
    /// (`end_method`/`end_top_level_method`, before the frame pops): the
    /// declared return type, the block-body `{` gate keyed by the end
    /// token — the exact span the flow layer's `BodyMayCompleteNormally`
    /// fact carries — and the declaring name's span (the completion rows'
    /// anchor) stage for the teardown's fold to join
    /// (`body_might_complete_normally_nullable`). Setters stay out (the
    /// official `!node.isSetter` gate). A function literal stages its
    /// context return (`arm_closure_completion`'s arm) anchored at the
    /// body brace — the official `BlockFunctionBody` anchor (the
    /// `resolver.dart` switch's `errorNode.block.leftBracket`) — and
    /// frames with neither a latched return nor an armed context
    /// (constructors, untyped closures) under-report. The
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
                    ret,
                    ctx_ret,
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
        // A declared shape stages its latched return (the anchor is the
        // declaring name); a closure literal stages its armed context
        // return, anchored at the body brace — a braceless literal (an
        // arrow body) has no block face to ask about.
        let (staged, anchor) = match (ret.as_ref(), ctx_ret.as_ref(), *body_brace) {
            (Some(exec), ..) => (exec.ty, exec.name_span),
            (None, Some(ctx), Some(brace)) => (*ctx, brace),
            _ => return,
        };
        // The staged type is the RESOLVED declared return (the official
        // walk reads `returnType?.type`, the resolved annotation): an
        // alias head answers its captured right-hand side before any
        // question asks — `typedef Nothing = void; Nothing f() {}` walks
        // as `void` (silent), not as the alias head (a false
        // non-nullable), and the nullable twin reports the aliased display
        // text (`typedef N = int?; N f() {}` → `'int?'`, the nullable
        // warning, not the ERROR twin on the raw head).
        let declared = self.completion_declared_ty(staged);
        if *generator || (*is_async && !self.legal_async_return(declared)) {
            return;
        }
        self.insert_body_completion(end.range, declared, *body_brace, *is_async, anchor);
    }

    /// The `body_completions` row insert shared by the named-executable end
    /// events and the local-function frame pop: keyed by the body end
    /// token's range, the fold's join point with the flow layer's
    /// `BodyMayCompleteNormally` fact.
    fn insert_body_completion(
        &mut self,
        end: SourceRange,
        declared: Ty,
        brace: Option<SourceRange>,
        is_async: bool,
        anchor: SourceRange,
    ) {
        let row = (end, declared, brace, is_async, anchor);
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

    /// The local-function completion staging: the declaration flavor's
    /// `end_local_function_declaration` carries no dedicated staging call,
    /// so its executable frame releases the row at its [`Engine::pop_frame`]
    /// — the same staging `stage_body_completion` runs for named
    /// executables (upstream `visitFunctionDeclaration`'s
    /// `!node.isSetter` arm, resolver.dart:3109-3114; a local function can
    /// never spell `set`, so no get/set gate applies). The staged type is
    /// the declared return latched at the name event; the end token is the
    /// body block's own `}`, resolved through the token table's
    /// `matching_end` (the flow fact's `ExecEnd` span pairs it exactly).
    /// `brace` is the block-body `{`; a `None` brace (arrow or body-less
    /// recovery) stages nothing, mirroring the named path.
    pub(crate) fn stage_local_fn_completion(
        &mut self,
        staged: Ty,
        anchor: SourceRange,
        brace: Option<SourceRange>,
        is_async: bool,
        generator: bool,
    ) {
        let Some(brace) = brace else {
            return;
        };
        // The body block's matching `}`: the `{` token's `end_of` pair — a
        // synthetic close (an unclosed body's recovery token) carries the
        // parser's own span, which is exactly the fact's `ExecEnd` anchor.
        let toks = self.tokens.as_slice();
        let at = toks.partition_point(|t| t.utf16_offset < brace.offset);
        let Some(end) = toks
            .get(at)
            .filter(|t| {
                t.utf16_offset == brace.offset
                    && t.kind == dartq_scanner::TokenKind::OpenCurly
            })
            .and_then(|_| self.tokens.matching_end(at))
            .and_then(|close| self.tokens.get(close))
        else {
            return;
        };
        let end_range = SourceRange {
            offset: end.utf16_offset,
            length: self.utf16_of(end.byte_range().end) - end.utf16_offset,
        };
        let declared = self.completion_declared_ty(staged);
        if generator || (is_async && !self.legal_async_return(declared)) {
            return;
        }
        self.insert_body_completion(end_range, declared, Some(brace), is_async, anchor);
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
        // An `async` body's completion question reads the declared
        // return's completion context — the official
        // `_contextTypeForImposed`'s final arm
        // (body_inference_context.dart): the `FutureOr<V>` union over the
        // future value type, never the flatten — `Future<int> f() async
        // {}` asks through `FutureOr<int>` and the diagnostic renders that
        // union (the corpus `'FutureOr<int>'` row), a nullable element
        // (`Future<int>?`) reads `FutureOr<int?>`. The imposed-null face —
        // no written return type, our `Unknown` stand-in — keeps the
        // oracle's null contextType: the walk never fires.
        let ret = if *is_async {
            if *declared == Ty::Unknown {
                return None;
            }
            self.async_completion_context(*declared)
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
            // `FutureOr<V>` reads its nullability through `V` — upstream
            // `isPotentiallyNonNullable(FutureOr<T>)` delegates to the
            // argument, so a nullable element (`FutureOr<int?>`,
            // `FutureOr<Object?>` — the unimported-`FutureOr`'s
            // `InvalidType` → `Object?` face) reaches the NULLABLE twin,
            // while a non-nullable element's union is potentially
            // non-nullable (`FutureOr<int>` → the ERROR twin). The scope
            // gate is the written-head arm's `named_type_is_type` — the
            // async path's `ret` head is a materialized union, always a
            // real type.
            Ty::Named {
                name,
                nullable: false,
                ..
            } if name == self.intern("FutureOr") =>
            {
                if !self.named_type_is_type(name) && !*is_async {
                    return None;
                }
                match self.future_or_base(ret) {
                    Ty::Named { nullable: true, .. }
                    | Ty::Record { nullable: true, .. }
                    | Ty::Function { nullable: true, .. } => {
                        SemanticCode::BodyMightCompleteNormallyNullable
                    }
                    _ => SemanticCode::BodyMightCompleteNormally,
                }
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

    /// The completion walk's resolved declared return: the shared alias
    /// fold plus the terminal `void`/`Null`/`Never` right-hand sides it
    /// keeps written (the shared fold's other consumers model no
    /// nullability face through them). The official walk reads
    /// `returnType?.type` — the resolved annotation: `typedef Nothing =
    /// void; Nothing f() {}` answers `void` (the named-type resolution's
    /// `type: void`), never the alias head, and the same read decides the
    /// nullable `Nothing?` twin (its `FutureOr` base is `void` — silent).
    pub(crate) fn completion_declared_ty(&self, ty: Ty) -> Ty {
        // Only a head that RESOLVES as a type in scope folds: an
        // unresolved head is the oracle's `InvalidType` — the completion
        // question never reaches it (the silent
        // [`Engine::named_type_is_type`] face), and the alias chase must
        // not resurrect it through an invisible library's declaration. A
        // type parameter or a primitive head is a type through the frame,
        // never an alias use — the chase's not-found face must not see
        // them either.
        let Ty::Named { name, .. } = ty else {
            return ty;
        };
        if !self.named_type_is_type(name)
            || self.type_params.contains(&name)
            || self.pending_type_params.contains(&name)
            || self.members.primitive_ids.contains(&name)
        {
            return ty;
        }
        let folded = self.fold_alias_ty(ty);
        let Ty::Named {
            name,
            nullable,
            args: ArgListId::EMPTY,
        } = folded
        else {
            return folded;
        };
        match self.alias_rhs(name) {
            // `void` swallows the question (a void type carries no `?`).
            Some(Ty::Void) => Ty::Void,
            // `Null` is the model's nullable-without-suffix leaf either way.
            Some(Ty::Null) => Ty::Null,
            // A questioned `Never` has no modeled face (`Ty::Never` carries
            // no nullability) — the written use stays.
            Some(Ty::Never) if !nullable => Ty::Never,
            _ => folded,
        }
    }

    /// The async body's completion context — official
    /// `_contextTypeForImposed`'s final arm (body_inference_context.dart)
    /// as a `Ty`: the `FutureOr<V>` union over the declared return's
    /// future value type. The verdict reads the union's faces through the
    /// same arms as any written `FutureOr`, and the nullable twin renders
    /// the union itself — the oracle's display text (`'FutureOr<int>'` for
    /// `Future<int> f() async {}`). The union's `?` lives on the future
    /// value type (`FutureOr<int?>` spells the argument's `?`, never a
    /// `FutureOr<int?>?` head suffix) — its nullability reaches the
    /// verdict through [`Self::future_or_base`]'s structural read, not a
    /// head flag.
    pub(crate) fn async_completion_context(&self, declared: Ty) -> Ty {
        let value = self.future_value_type(declared);
        Ty::Named {
            name: self.intern("FutureOr"),
            nullable: false,
            args: self.boundary.intern_list(&[value]),
        }
    }

    /// `TypeSystem.futureValueType` at name level (type_system.dart:471):
    /// `S?` strips to `S`, a `Future<S>`/`FutureOr<S>` head unwraps to `S`
    /// (a bare head spells `<dynamic>` — its element is `dynamic`),
    /// `dynamic`/`void` stay themselves, and every other head answers
    /// `Object?`. The recovery leaves (`Never`/`Null`/records/functions)
    /// never reach the answer alive — the async-legality gate silences
    /// them at staging — so the `Object?` arm is the totality face. An
    /// unimported `FutureOr` head is `InvalidType` upstream (its element
    /// resolution fails without the `dart:async` import) — `InvalidType`
    /// falls to the `_`/`Object?` arm, so the unwrap gate is the same
    /// scope question [`Self::named_type_is_type`] asks; `Future` resolves
    /// through dart:core's own re-export and always counts.
    pub(crate) fn future_value_type(&self, ty: Ty) -> Ty {
        match ty {
            Ty::Named {
                name,
                nullable: true,
                args,
            } => self.future_value_type(Ty::Named {
                name,
                nullable: false,
                args,
            }),
            Ty::Named { name, args, .. }
                if (name == self.intern("Future") || name == self.intern("FutureOr"))
                    && self.named_type_is_type(name) =>
            {
                if args == ArgListId::EMPTY {
                    return Ty::Dynamic;
                }
                let pool = self.boundary.pool();
                let list = pool.get(args);
                if list.len() == 1 { list[0] } else { ty }
            }
            Ty::Dynamic | Ty::Void => ty,
            _ => self.object_question(),
        }
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
                display: None,
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
        // An operator head latches `ret` at `begin_method` with the armed
        // slot still empty — the return annotation's `endType` lands AFTER
        // the head event for `int operator >` (the parser commits to the
        // method at the `operator` keyword). The first completion still in
        // the header window — outside any formal list, before the body
        // opens — is that annotation: it backfills the latched UNKNOWN
        // face. An untyped operator's first formal (`operator >(C o)`)
        // completes under `(` and a local variable's annotation completes
        // after `body_mark` — neither reaches this window, so the honest
        // `dynamic` target survives.
        if self.formal_list_depth == 0
            && self.body_mark.is_none()
            && let Some(ty) = self.armed_ty
            && let Some(frame) = self.frames.last_mut()
            && let FrameKind::Executable { ret: Some(ret), .. } = &mut frame.kind
            && ret.ty == Ty::UNKNOWN
        {
            ret.ty = ty;
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
    pub(crate) fn latch_factory_return(
        &mut self,
        container: StringId,
        head: StringId,
        name_span: SourceRange,
    ) {
        // The message's `{2}` is the constructor element's `displayName`: a
        // named factory head renders qualified (`C.named`), the container
        // spelling and the `new` recovery head render bare.
        let container_text = self.strings.resolve(container);
        let head_text = self.strings.resolve(head);
        let (name, display) = if head_text == container_text || head_text == "new" {
            (container, None)
        } else {
            (
                container,
                Some(
                    self.strings
                        .intern(&format!("{container_text}.{head_text}")),
                ),
            )
        };
        // The constructor element's return type is the enclosing class's
        // `thisType` — a generic container spells `A<T>` (the declared
        // parameters, element.dart:1004), not the bare head and not the
        // instantiate-to-bounds `A<dynamic>`: the message's `{1}` and the
        // verdict's target both read it.
        let ty = self
            .bounds
            .container_params(container)
            .filter(|params| !params.is_empty())
            .map(|params| {
                let args: Vec<Ty> = params.iter().map(|(p, _, _)| Ty::named(*p)).collect();
                Ty::Named {
                    name: container,
                    nullable: false,
                    args: self.boundary.intern_list(&args),
                }
            })
            .unwrap_or_else(|| Ty::named(container));
        if let Some(frame) = self.frames.last_mut()
            && let FrameKind::Executable { ret, .. } = &mut frame.kind
            && ret.is_none()
        {
            *ret = Some(ExecReturn {
                ty,
                host: ExecKind::Constructor,
                name,
                display,
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

#[cfg(test)]
mod tests_3098 {
    //! The `dead_null_aware_expression` verdict pins: the operand's static
    //! type (upstream `_checkForDeadNullCoalesce`, error_verifier.dart
    //! :4733-4739) is read the way the oracle reads it, and the report
    //! anchors the value's own expression span.
    //!
    //! Oracle provenance: `dart analyze` 3.13.1 machine format — every
    //! expectation below is a row the conformance corpus's `expected.tsv`
    //! carries for the same shape (`int-to-double/assignment_class_member_*`,
    //! `dot_shorthands/*_if_null_error_test`, `if_null/assignment_behavior_test`,
    //! `super/conditional_operator_test`, `join_return_with_assignment`,
    //! `nnbd/operator_type_test`).
    //!
    //! The FLOW-PROMOTED operand face (`if (a is Object) a ?? c`,
    //! `x ??= 1; x ??= 2`) has no pin: the promotion table only materializes
    //! at the teardown join, so those rows stay missing until the deferred
    //! re-decision lands (see `Engine::coalesce_operand_never_null`).

    use crate::diagnose::test_rig::{CORE_PRELUDE, report_ops};

    /// `(offset, length)` of the `nth` (0-based) occurrence of `needle` in
    /// the FIXTURE text, shifted past the prelude the rig mounts ahead of
    /// it. Every fixture here is ASCII, so byte and UTF-16 offsets coincide.
    fn at(source: &str, needle: &str, nth: usize) -> (u32, u32) {
        let found = source
            .match_indices(needle)
            .nth(nth)
            .unwrap_or_else(|| panic!("`{needle}` occurrence #{nth} missing"));
        (
            found.0 as u32 + CORE_PRELUDE.len() as u32,
            needle.len() as u32,
        )
    }

    /// The `dead_null_aware_expression` rows as `(offset, length)`,
    /// source-ordered.
    fn dead_rows(source: &str) -> Vec<(u32, u32)> {
        let report = report_ops(source);
        let mut rows: Vec<(u32, u32)> = report
            .diagnostics
            .iter()
            .filter(|row| row.code.name == "dead_null_aware_expression")
            .map(|row| (row.range.offset, row.range.length))
            .collect();
        rows.sort_unstable();
        rows.dedup();
        rows
    }

    /// `??` reads the operand's `staticType`; a non-nullable static field's
    /// `??=` is dead, and the null-aware `?.` spelling of the same target is
    /// too (the read resolution carries no null-aware join for `??=`).
    #[test]
    fn if_null_assignment_reads_static_and_null_aware_member_targets() {
        let source = "\
class C {\n\
\x20 static double s = -42;\n\
}\n\
void f() {\n\
\x20 C.s ??= 11;\n\
\x20 C?.s ??= 22;\n\
}\n";
        assert_eq!(
            dead_rows(source),
            vec![at(source, "11", 0), at(source, "22", 0)]
        );
    }

    /// A nullable receiver's `?.` member still reads the accessor's own
    /// type under `??=` (`c?.m1 ??= 11` is dead) while the `??` face keeps
    /// the null-aware join (`c?.m1 ?? 22` is live).
    #[test]
    fn if_null_assignment_keeps_the_accessor_face_that_if_null_widens() {
        let source = "\
class C {\n\
\x20 double m1 = 1;\n\
}\n\
void f(C? c) {\n\
\x20 c?.m1 ??= 11;\n\
\x20 var r = c?.m1 ?? 22;\n\
}\n";
        assert_eq!(dead_rows(source), vec![at(source, "11", 0)]);
    }

    /// A type parameter decides through its written bound: a nullable bound
    /// (or the implicit `Object?`) is potentially null and never anchors,
    /// a non-nullable bound does.
    #[test]
    fn if_null_decides_type_parameters_through_their_bounds() {
        let source = "\
void maybeBound<BQ extends bool?>(BQ cond) {\n\
\x20 cond ?? null;\n\
}\n\
void nonNullBound<T extends Object>(T v) {\n\
\x20 v ?? null;\n\
}\n";
        assert_eq!(dead_rows(source), vec![at(source, "null", 1)]);
    }

    /// An undefined class's field carries an invalid type — the official
    /// `isStrictlyNonNullable(InvalidType)` answers nothing.
    #[test]
    fn if_null_assignment_over_an_undefined_class_stays_silent() {
        let source = "\
class A {\n\
\x20 B _lazyInstance;\n\
\x20 static B get instance {\n\
\x20   _lazyInstance ??= B();\n\
\x20   return _lazyInstance;\n\
\x20 }\n\
}\n";
        assert_eq!(dead_rows(source), Vec::new());
    }

    /// An increment result and a pattern are no assignment targets: the
    /// official `visitIfNullAssignment` returns before the deadness check
    /// (`InvalidExpressionAssignmentTargetImpl`), so no row anchors.
    #[test]
    fn if_null_assignment_over_an_invalid_target_stays_silent() {
        let source = "\
void f(num x, int y) {\n\
\x20 x++ ??= y;\n\
}\n\
void g() {\n\
\x20 int a1 = 42;\n\
\x20 Record r1 = ();\n\
\x20 ((a1, _) && r1) ??= (1, 2);\n\
\x20 int a2 = 3;\n\
\x20 [a2, _] ??= [1, 2];\n\
\x20 var a4 = 3;\n\
\x20 (a4, _) ??= (1, 2);\n\
}\n";
        assert_eq!(dead_rows(source), Vec::new());
    }

    /// The report anchors `node.value`'s own expression span, never the
    /// inner operand row (`sideEffect()`'s callee head, `-42`'s digits).
    #[test]
    fn if_null_assignment_anchors_the_whole_value_expression() {
        let source = "\
void sideEffect() {}\n\
void f() {\n\
\x20 int l = 1;\n\
\x20 l ??= sideEffect();\n\
\x20 int n = 2;\n\
\x20 n ??= -42;\n\
}\n";
        assert_eq!(
            dead_rows(source),
            vec![at(source, "sideEffect()", 1), at(source, "-42", 0)]
        );
    }

    /// A leading-dot shorthand resolves against the declaration's context
    /// type: a constructor spelling builds that type (non-nullable), so the
    /// `??` right operand is dead; a static getter answers its return type.
    #[test]
    fn if_null_reads_a_dot_shorthand_through_its_context_type() {
        let source = "\
class ConstructorClass {\n\
\x20 final int? x;\n\
\x20 ConstructorClass(this.x);\n\
\x20 ConstructorClass.regular(this.x);\n\
}\n\
class Integer {\n\
\x20 final int value;\n\
\x20 Integer(this.value);\n\
\x20 static Integer get one => Integer(1);\n\
}\n\
void f(ConstructorClass firstFallback, ConstructorClass secondFallback, Integer thirdFallback) {\n\
\x20 ConstructorClass first = .new(1) ?? firstFallback;\n\
\x20 ConstructorClass second = .regular(2) ?? secondFallback;\n\
\x20 Integer third = .one ?? thirdFallback;\n\
}\n";
        assert_eq!(
            dead_rows(source),
            vec![
                at(source, "firstFallback", 1),
                at(source, "secondFallback", 1),
                at(source, "thirdFallback", 1),
            ]
        );
    }

    /// A nullable static getter's shorthand keeps the nullable face: the
    /// right operand stays live.
    #[test]
    fn if_null_keeps_a_nullable_dot_shorthand_member_live() {
        let source = "\
class Integer {\n\
\x20 final int value;\n\
\x20 Integer(this.value);\n\
\x20 static Integer? get nullable => null;\n\
}\n\
void f(Integer fallback) {\n\
\x20 Integer pick = .nullable ?? fallback;\n\
}\n";
        assert_eq!(dead_rows(source), Vec::new());
    }

    /// `super` cannot be null, so its read is never null however the
    /// (illegal) `?.` spelling reads.
    #[test]
    fn if_null_assignment_over_a_super_member_is_never_null() {
        let source = "\
class B {\n\
\x20 int field = 1;\n\
}\n\
class C extends B {\n\
\x20 void test() {\n\
\x20   super?.field ??= 11;\n\
\x20 }\n\
}\n";
        assert_eq!(dead_rows(source), vec![at(source, "11", 0)]);
    }

    /// An invalid right operand stages no expression row: the official
    /// report then anchors a synthetic empty node, which the row model
    /// cannot spell — the face stays silent rather than anchoring the
    /// target.
    #[test]
    fn if_null_assignment_without_a_value_row_stays_silent() {
        let source = "\
void f([int? optional]) {\n\
\x20 optional ??= ...;\n\
}\n";
        assert_eq!(dead_rows(source), Vec::new());
    }
}

/// The #3151 pins (`body_might_complete_normally`'s local-function arm —
/// `visitFunctionDeclaration` stages no dedicated end-event call for the
/// declaration flavor, so the frame pop releases the row).
#[cfg(test)]
#[path = "ticket_3151_tests.rs"]
mod ticket_3151_tests;
