use super::*;

use dartq_scanner::{Keyword, TokenKind};

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
pub(crate) const PATTERN_ARM_SCOPE_BASE: u32 = u32::MAX - u16::MAX as u32;

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
    /// the face. The two ghost placements tell the shapes apart: the
    /// statement-start route plants the ghost AT the junk the re-parse
    /// then names, while the consume-in-doubt route plants it at the
    /// consumed junk's END (`insertSyntheticIdentifier(identifier)`,
    /// token_stream_rewriter.dart:238-248) — the recovered name's own
    /// offset when the two are glued (`{st}`: oracle 3.13.7 anchors
    /// `unused_catch_stack` on `st`). The flip never firing (a synthetic
    /// comma) routes to the typed-catch handoff below.
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
            // arms no written type) never confirms the pair this way;
            // there both ensure routes planted nameless synthetics, which
            // the arm below reads directly.
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
            // Both ensure routes synthetic (`catch (var e)`: the reserved
            // `var` starts a statement, so the exception ensure and the
            // trace ensure behind the rewriter's synthetic comma each plant
            // a nameless identifier): the official list reads `[<syn>,
            // <syn>]`, so `catchParameters[1]` is a nameless stack trace —
            // oracle 3.13.7 reports `unused_catch_stack` '<unnamed>' (and
            // `duplicate_definition` '') for `catch (var e) {}` and for
            // `on int catch (var e) {}` alike, the exception staying
            // auto-used through its trace sibling.
            if let Some((first, second)) = slots
                && self.is_nameless(self.locals[first].name)
                && self.is_nameless(self.locals[second].name)
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
        if ghost_offset.is_some_and(|offset| offset == range.offset && !self.junk_ends_at(offset)) {
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

    /// Whether a scanned token other than the slot's comma ends exactly at
    /// `offset` (UTF-16) — the consumed junk a consume-in-doubt catch ghost
    /// was spliced behind ([`Self::reconcile_catch_slots`]). A statement-
    /// start ghost sits at its junk's own start, after whitespace or the
    /// written comma, so no such token ends there.
    fn junk_ends_at(&self, offset: u32) -> bool {
        let tokens = self.tokens.as_slice();
        let at = tokens.partition_point(|token| token.utf16_offset < offset);
        at.checked_sub(1)
            .and_then(|index| tokens.get(index))
            .is_some_and(|token| {
                token.kind != TokenKind::Comma
                    && self.utf16_of((token.byte_offset + token.byte_len) as usize) == offset
            })
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
    ///
    /// An `_`-named local function binds nothing at all
    /// ([`Self::declares_nothing`]): the official `LocalScope.add` skips
    /// the element like every other wildcard, so neither a sibling nor an
    /// in-body `_()` resolves — both report `undefined_function`, while
    /// the tracked row still carries the `dead_code` face.
    pub(crate) fn note_local_function(&mut self, name: StringId, range: SourceRange) {
        // The name joins the enclosing statement list's duplicate scope
        // (`checkStatements`' `FunctionDeclarationStatement` arm): the name
        // event fires before the body block opens the function's own scope,
        // so the innermost scope is the enclosing list's.
        self.dup_scope_add(name, range);
        if !self.declares_nothing(name)
            && self.frames.len() >= 2
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

    /// `begin_import`: one site per written `import` directive — the
    /// `unused_import` pass aligns `imports` positionally against the
    /// library's import edges, and an import element exists upstream for
    /// every written directive whatever its URI shape (the interpolation
    /// arm still contributes a `libraryImports[index]` and an edge with no
    /// target; `imports_verifier.dart:232+` iterates the directive list).
    /// The engine registers exactly one `ImportEdge` per directive
    /// (`DirectiveKind::Import` over `unit.facts.directives`), so a
    /// dropped site here does not merely skip that directive: the count
    /// gate (`edges.len() != imports.len()`) silences the whole file's
    /// `unused_import`/`unused_shown_name` faces, whatever the other
    /// directives look like.
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
    ///
    /// A non-string URI head (`import 5;`, an unquoted experimental URI as
    /// the pinned parser recovers it) joins with an empty `uri_text` and a
    /// zero-length `uri_range` at that head. The parser hands the AST a
    /// synthetic `String` token for it, but a synthetic token's id sits
    /// past the scanned table (`Cursor::insert_synthetic`), so the engine's
    /// head read lands on the real token: the recovered URI's position is
    /// where upstream's empty `relativeUriString` anchors, and the site
    /// must exist for the count gate to hold.
    #[expect(clippy::cast_possible_truncation)] // span arithmetic narrows under the scanner u32 offset cap
    pub(crate) fn begin_import_site(&mut self, import_kw: Token) {
        // `import` with no following token (end of file): the directive is
        // still written, so the site must still exist — anchored on the
        // keyword itself.
        let Some(uri) = self.tokens.get(import_kw.id as usize + 1) else {
            self.imports.push(ImportSite {
                uri_text: String::new(),
                uri_range: SourceRange {
                    offset: import_kw.range.offset,
                    length: 0,
                },
                prefix: None,
                shown: Vec::new(),
            });
            return;
        };
        if uri.kind != dartq_scanner::TokenKind::String {
            self.imports.push(ImportSite {
                uri_text: String::new(),
                uri_range: SourceRange {
                    offset: uri.utf16_offset,
                    length: 0,
                },
                prefix: None,
                shown: Vec::new(),
            });
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
        // A record-typed (or otherwise paren-headed) `on` head carries no
        // named `on` type: the clause joins the list with `on_type: None`,
        // and [`Self::finish_try_statement`]'s `catch_head_is_on` keeps it
        // out of the catch-all arm — upstream resolves the head to a
        // `RecordType` (never null, never `isDartCoreObject`), so the
        // clause takes the subtype arm, silent against named heads
        // (oracle 3.13.7: `on (int, String) {} on int {}` reports
        // nothing). The head's own name references (a valid record
        // type's field types) already ran their rows; they must not turn
        // the clause into a named on-clause.
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
    /// catch-all clause (no `on` type, or an `on` type resolving to dart:core
    /// `Object`) followed by more clauses makes the tail dead; and an
    /// on-clause whose type is a subtype of an earlier on-clause type in the
    /// same try is dead too (`dead_code_verifier.dart`, `nextCatchClause`;
    /// the subtype arm, :589-604). Either way the span covers the first dead
    /// clause through the last one, and one dead-catch row per try at most
    /// (`_done`, dead_code_verifier.dart:414).
    ///
    /// Faces the upstream text pins exactly:
    ///
    /// - The catch-all test reads the RESOLVED type:
    ///   `exceptionType?.type == null || ....isDartCoreObject`, and
    ///   `isDartCoreObject` is element identity — core `Object`, ANY
    ///   written nullability (`type.dart:767-769` checks only
    ///   `element.name`/`element.library`). So `on Object?` IS catch-all,
    ///   exactly like `on Object`.
    /// - An `on` head that starts with `(` still resolves: `computeType`
    ///   accepts the record type annotation (parser_impl.dart:10723 — the
    ///   records-feature ambiguity resolution), so `exceptionType.type` is
    ///   a `RecordType` — never null, never `isDartCoreObject`. The clause
    ///   takes the subtype arm, where a record shape proves nothing against
    ///   a named head and vice versa — silent (corpus `on_clauses_A01_*` /
    ///   `A03_*`, all `-`). An `on` head NO type parse accepts
    ///   (`on<T>(…)`) resolves to the `InvalidType`: the 0c rule answers
    ///   `true` from either side of `isSubtypeOf`, so the corpus's
    ///   `'InvalidType' is a subtype of 'String'` row (`on_clauses_A02_t05`)
    ///   fires through [`Self::resolved_subtype`].
    /// - The subtype arm asks `TypeSystem.isSubtypeOf(current, earlier)` —
    ///   the resolved types WITH their type-argument lists (covariance:
    ///   `on num {} on int {}` fires, `on int {} on num {}` is silent,
    ///   `dead_code_on_catch_subtype/doc_0`). [`Self::resolved_subtype`] is
    ///   that walk over the written shapes; an unresolved head answers
    ///   `true` from either side (0c) and keeps the oracle's
    ///   `'InvalidType'` argument spelling ([`Ty::Unknown`] renders
    ///   `InvalidType`).
    pub(crate) fn finish_try_statement(&mut self) {
        let Some(clauses) = self.try_clauses.pop() else {
            return;
        };
        let object = self.intern("Object");
        let mut visited: Vec<Ty> = Vec::new();
        for (index, clause) in clauses.iter().enumerate() {
            // A bare `catch` carries no `exceptionType` (null — catch-all);
            // an `on` clause always resolves to a type (named, record, or
            // `InvalidType`) and reaches the subtype arm, with core
            // `Object` in any nullability the one catch-all exception.
            let catches_all = match clause.on_type {
                None => !self.catch_head_is_on(clause),
                Some(name) => name == object,
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
            // resolved on-types (`on int?` is not below `on int`; the
            // argument lists decide same-head pairs covariantly).
            let current = self.catch_on_ty(clause);
            if let Some(earlier) = visited
                .iter()
                .find(|earlier| self.resolved_subtype(current, **earlier))
                .copied()
            {
                let start = clause.start.offset;
                let end = clauses[clauses.len() - 1].end.end();
                // The message names the RESOLVED faces: an invalid head
                // spells `InvalidType` (the oracle's `DartType
                // .getDisplayString` on the substituted `InvalidType`), a
                // written argument list renders its arguments.
                let a0 = self.catch_ty_display(current);
                let a1 = self.catch_ty_display(earlier);
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

    /// Whether the clause's head token is the `on` keyword — the split the
    /// `on_type == None` fold at [`Self::finish_catch_clause`] hides:
    /// `None` is either a bare `catch` (no exception type — catch-all) or a
    /// record/paren head (`on (...)`, which still resolves upstream — never
    /// catch-all).
    fn catch_head_is_on(&self, clause: &CatchClauseFact) -> bool {
        self.tokens
            .as_slice()
            .get(self.token_at_offset(clause.start.offset))
            .is_some_and(|t| t.kind == TokenKind::Keyword(Keyword::On))
    }

    /// The diagnostic-message face of a verdict type: an invalid head (the
    /// unresolved/paren-junk face) spells `InvalidType` — the oracle's
    /// `DartType.getDisplayString` on the substituted `InvalidType` — while
    /// everything else renders through [`Self::ty_display`]. `Ty::render`'s
    /// `Unknown` arm spells `""` for the diagnoser's unknown slots
    /// (model.rs `render`), so the invalid face carries the literal —
    /// `render_slot`'s own `InvalidType` splice is the same spelling.
    fn catch_ty_display(&self, ty: Ty) -> String {
        if self.is_invalid_ty(ty) {
            return "InvalidType".to_string();
        }
        self.ty_display(ty)
    }

    /// The token index at/after a UTF-16 offset (the table is
    /// offset-ordered; EOF closes every walk).
    pub(crate) fn token_at_offset(&self, offset: u32) -> usize {
        self.tokens
            .as_slice()
            .partition_point(|t| t.utf16_offset < offset)
    }

    /// The written on-clause type as a [`Ty`] (`dynamic`/`Null` keep their
    /// dedicated shapes; a named head carries its written type-argument
    /// list — upstream compares the RESOLVED types, so `on List<String>`
    /// and `on List<dynamic>` must not collide).
    pub(crate) fn catch_on_ty(&self, clause: &CatchClauseFact) -> Ty {
        let Some(name) = clause.on_type else {
            return self.catch_record_face(clause);
        };
        match self.strings.resolve(name) {
            "dynamic" => Ty::Dynamic,
            "Null" => Ty::Null,
            _ => Ty::Named {
                name,
                nullable: clause.on_nullable,
                args: self.catch_head_args(clause),
            },
        }
    }

    /// The record/paren head's lattice face (the folded
    /// `on_type == None` clause whose head token IS the `on` keyword):
    /// a `(`-led head resolves to a `RecordType` — it proves nothing
    /// against a named head and no named head proves against it — while a
    /// head no type parse accepts (`on<T>(…)`, a junk token after `on`)
    /// resolves to `InvalidType` (the 0c rule: subtypes everything, and
    /// everything subtypes it — corpus `on_clauses_A02_t05`).
    fn catch_record_face(&self, clause: &CatchClauseFact) -> Ty {
        let toks = self.tokens.as_slice();
        let head = self.token_at_offset(clause.start.offset);
        if !toks
            .get(head)
            .is_some_and(|t| t.kind == TokenKind::Keyword(Keyword::On))
        {
            // A bare `catch` fact never reaches this arm (it answers the
            // catch-all test first); an unexpected face keeps the old
            // whole-fact invalid answer.
            return Ty::UNKNOWN;
        }
        match toks.get(head + 1).map(|t| t.kind) {
            Some(TokenKind::OpenParen) => {
                // The record shape rides the clause's own arity: the
                // depth-0 commas inside the paren group + 1 fields, the
                // recovered holes `Unknown` (the analyzer's InvalidType
                // splice keeps the record's arity honest — `(int, ,)`
                // binds `(int, InvalidType)`). Identity decides the rare
                // record-vs-record pair; named heads prove nothing.
                let Some(close) = self.tokens.matching_end(head + 1) else {
                    return Ty::UNKNOWN;
                };
                let mut commas = 0usize;
                let mut depth = 0usize;
                for t in toks.iter().take(close).skip(head + 2) {
                    match t.kind {
                        TokenKind::OpenParen
                        | TokenKind::OpenSquare
                        | TokenKind::OpenCurly
                        | TokenKind::Lt => depth += 1,
                        TokenKind::CloseParen
                        | TokenKind::CloseSquare
                        | TokenKind::CloseCurly
                        | TokenKind::Gt => depth = depth.saturating_sub(1),
                        TokenKind::Comma if depth == 0 => commas += 1,
                        _ => {}
                    }
                }
                let arity = if close == head + 2 { 0 } else { commas + 1 };
                self.boundary
                    .args
                    .borrow_mut()
                    .record(&vec![Ty::Unknown; arity], &[], false)
            }
            _ => Ty::Unknown,
        }
    }

    /// The written type-argument group of the clause's on-head (the `<…>`
    /// between the head name and the body block) — [`ArgListId::EMPTY`]
    /// when the head is raw or any argument spelling walks past the
    /// simple-shape grammar (an exotic argument keeps the whole list raw:
    /// the pre-fix face, never a fabricated one).
    fn catch_head_args(&self, clause: &CatchClauseFact) -> ArgListId {
        let toks = self.tokens.as_slice();
        let mut cursor = self.token_at_offset(clause.start.offset);
        let mut group = None;
        while let Some(t) = toks.get(cursor) {
            match t.kind {
                // The body block (or a body-less `on X;`) closes the head.
                TokenKind::OpenCurly | TokenKind::Semicolon | TokenKind::Eof => break,
                TokenKind::Lt => {
                    group = Some(cursor);
                    break;
                }
                _ => cursor += 1,
            }
        }
        let Some(open) = group else {
            return ArgListId::EMPTY;
        };
        let Some(close) = self.tokens.matching_end(open) else {
            return ArgListId::EMPTY;
        };
        let mut args = Vec::new();
        let mut cursor = open + 1;
        while cursor < close {
            if toks[cursor].kind == TokenKind::Comma {
                // An empty argument slot (`A<, B>`) — recovery shape.
                return ArgListId::EMPTY;
            }
            let Some((ty, next)) = self.catch_arg_ty(toks, cursor, close) else {
                return ArgListId::EMPTY;
            };
            args.push(ty);
            cursor = match toks.get(next) {
                Some(t) if t.kind == TokenKind::Comma => next + 1,
                Some(_) => next,
                None => return ArgListId::EMPTY,
            };
            if cursor == next && cursor <= close {
                // `next` landed on the closer: the list ended.
                break;
            }
        }
        if args.is_empty() || cursor != close {
            return ArgListId::EMPTY;
        }
        self.boundary.intern_list(&args)
    }

    /// One written type argument in `[start, end)`: an identifier head
    /// (`dynamic`/`Null` keep their dedicated shapes) with an optional
    /// nested argument group and trailing `?`. Returns the type and the
    /// index just past it, or `None` on any other spelling (prefixed
    /// heads, function types, junk).
    fn catch_arg_ty(
        &self,
        toks: &[dartq_scanner::Token],
        start: usize,
        end: usize,
    ) -> Option<(Ty, usize)> {
        let head = toks.get(start)?;
        if head.kind != TokenKind::Identifier {
            return None;
        }
        let name = &self.source[head.byte_range()];
        let (mut ty, mut next) = match name {
            "dynamic" => (Ty::Dynamic, start + 1),
            "Null" => (Ty::Null, start + 1),
            _ => (
                Ty::Named {
                    name: self.intern(name),
                    nullable: false,
                    args: ArgListId::EMPTY,
                },
                start + 1,
            ),
        };
        if toks.get(next)?.kind == TokenKind::Lt {
            let close = self.tokens.matching_end(next)?;
            if close >= end {
                return None;
            }
            let Ty::Named {
                name: inner_name,
                nullable: false,
                ..
            } = ty
            else {
                return None;
            };
            let mut inner = Vec::new();
            let mut cursor = next + 1;
            while cursor < close {
                if toks[cursor].kind == TokenKind::Comma {
                    return None;
                }
                let (arg, after) = self.catch_arg_ty(toks, cursor, close)?;
                inner.push(arg);
                cursor = match toks.get(after) {
                    Some(t) if t.kind == TokenKind::Comma => after + 1,
                    Some(_) => after,
                    None => return None,
                };
            }
            if inner.is_empty() || cursor != close {
                return None;
            }
            ty = Ty::Named {
                name: inner_name,
                nullable: false,
                args: self.boundary.intern_list(&inner),
            };
            next = close + 1;
        }
        if toks.get(next)?.kind == TokenKind::Question {
            let Ty::Named { nullable, .. } = &mut ty else {
                return None;
            };
            *nullable = true;
            next += 1;
        }
        Some((ty, next))
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

    /// The shorthand resolution's context type: the declaration group's
    /// armed annotation folded through the alias chain and the `FutureOr<S>`
    /// base (the oracle's `_resolveDotShorthand` unwraps both —
    /// `typedef E2 = E; E2 e = .a` resolves `.a` against `E`, and the static
    /// namespace denoted by `S` is the one `FutureOr<S>` denotes). `None`
    /// when the group carries no decidable context: unwritten (`var x =
    /// .a`), unresolvable (the annotation is the oracle's `InvalidType` —
    /// `pending_decl_ty_unresolved`, which swallows the member verdict
    /// upstream), or a non-interface shape the shorthand's lookup never
    /// climbs (records and `void`; a function-type context still reports
    /// through the invocation face, see [`Self::resolve_decl_shorthands`]).
    fn shorthand_context_ty(&self) -> Option<Ty> {
        if self.pending_decl_ty_unresolved {
            return None;
        }
        let ty = self.future_or_base(self.fold_alias_ty(self.pending_decl_ty?));
        (ty != Ty::UNKNOWN).then_some(ty)
    }

    /// The shorthand's resolved static face for `name` against the resolved
    /// context head `context` — upstream's `_resolveElement` union: a
    /// constructor spelling (`.new`, `.named`) answers the context type
    /// itself; a static member spelling answers the member's declared face
    /// (field/getter `var_ty`, method `return_ty`); an instance member and
    /// every miss answer `None` — the `dot_shorthand_undefined_member`
    /// verdict. The constructor probe unions the in-file decl sites
    /// ([`Self::ctor_decl_site`]) with the container's model face
    /// ([`Self::ctor_face_exists`]): a context head declared in a sibling
    /// unit owns no decl site here, so the local-only probe would
    /// misreport the shorthand `.named()` that resolves upstream.
    ///
    /// The answered face never carries the context's `?`. Upstream returns
    /// the ELEMENT's type, not the context's: a constructor spelling hands
    /// `PropertyElementResolverResult(getType: element.returnType)` —
    /// `C<int>`, the substituted interface type — and an enum constant the
    /// static field's own declared face (`Color`, never `Color?`)
    /// (property_element_resolver.dart:92-124, `_resolveDotShorthand`'s
    /// constructor arm and its `_resolveTargetInterfaceElement`
    /// fall-through). Propagating the context's `?` here invented a
    /// nullable result for `C? c = .new()` and the downstream member read
    /// then reported `unchecked_use_of_nullable_value` on a receiver the
    /// oracle promotes to non-null (the co19 `type_inference_A01_t01/t02/
    /// t08/t14`, `type_inference_A02_t01/t02`, `semantics_A05_t01` rows).
    fn shorthand_member_ty(
        &self,
        context: StringId,
        _nullable: bool,
        args: ArgListId,
        name: StringId,
    ) -> Option<Ty> {
        let ctor_hit = self.ctor_decl_site(context, name).is_some()
            || resolve_container(
                self.graph,
                self.namespaces,
                self.index,
                self.lib,
                self.file,
                context,
            )
            .is_some_and(|(decl, _)| self.ctor_face_exists(decl, context, name));
        if ctor_hit {
            return Some(Ty::Named {
                name: context,
                nullable: false,
                args,
            });
        }
        // An enum constant resolves as the enum's own type — the same
        // arm `member()` runs in `const_eval_faces.rs` for the const
        // face. `walk_members` finds enum constants only where the
        // container model exposes them as `members` entries; an
        // imported or augmented enum's `enum_constants` list never
        // reaches that map, so the walk misses where this arm answers.
        if resolve_container(
            self.graph,
            self.namespaces,
            self.index,
            self.lib,
            self.file,
            context,
        )
        .is_some_and(|(decl, _)| {
            decl.kind == ContainerKind::Enum && decl.enum_constants.contains(&name)
        }) {
            return Some(Ty::Named {
                name: context,
                nullable: false,
                args,
            });
        }
        let Walk::Found(entry) = self.walk_members(context, name) else {
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
        } = self.shorthand_context_ty()?
        else {
            return None;
        };
        self.shorthand_member_ty(context, nullable, args, row.name)
    }

    /// The declaration-initializer shorthand pass (#3433): every leading-dot
    /// selector row the window folded resolves against the declaration
    /// group's context type — upstream `DotShorthandPropertyAccess` /
    /// `DotShorthandInvocation` (`property_element_resolver.dart`'s
    /// `_resolveTargetInterfaceElement` with `resolvingDotShorthand`,
    /// `method_invocation_resolver.dart`'s `_resolveReceiverTypeLiteralFor
    /// DotShorthand`), never the bare scope the selector row's `Bare` shape
    /// pretended to answer. Runs at the variable-initializer close
    /// ([`Self::finish_const_initializer`] calls it for every group,
    /// const or not), while `pending_decl_ty` still carries the group's
    /// written annotation and `init_mark` still names the window's first
    /// row.
    ///
    /// The gate triple — `qualified` (the selector fold), `Bare(_)` (the
    /// dot access never merged a receiver), `dot_shorthand_context` (a `.`
    /// opens the name with an expression-opener ahead) — picks exactly the
    /// un-merged leading-dot selectors: `x.foo`, `x?.foo`, `x..foo`, and
    /// `super.new` all fail one arm. `row.ty` is the once-only marker:
    /// `push_member` writes `Unknown` for a non-record receiver shape, the
    /// pass upgrades a hit to the member's declared face, so a stale
    /// `init_mark` resweeping an older window never re-verdicts a resolved
    /// row (a miss row re-verdicts harmlessly — the emit dedups on the
    /// range).
    ///
    /// The verdict mirrors the oracle's two faces:
    ///
    /// - Context head resolves (`Ty::Named`): a member/constructor hit
    ///   types the row with the member's declared face and stays silent; a
    ///   miss reports `dot_shorthand_undefined_member` at the selector —
    ///   read and invocation arms alike (the invocation's
    ///   `dotShorthandUndefinedInvocation` and the read's
    ///   `dotShorthandUndefinedGetter` freeze to the same code).
    /// - No decidable context: the invocation arm still reports
    ///   `dot_shorthand_undefined_member` (its `contextType` renders `'_'`
    ///   upstream — `var c = .m()`); the read arm reports
    ///   `dot_shorthand_missing_context` at the whole shorthand span
    ///   (#3612).
    /// - An unresolvable annotation (`pending_decl_ty_unresolved`) or a
    ///   non-`Named` head: the read arm's verdict is `missing_context`;
    ///   the invocation arm reports with the written context's display
    ///   (`C Function() c = .m()`).
    ///
    /// Selector rows pushed with `Intent::Write`/`ReadWrite`/`Operator`
    /// resolve through their OWN position's arm (#3612 widened the head
    /// gate): an expression-statement target (``.a = v;``) arms the
    /// unknown schema; a mid-expression target keeps the under-report.
    ///
    /// Each row resolves against the context ITS position computes
    /// ([`Self::shorthand_ctx_at`]): a shorthand directly under `=` reads
    /// the declaration's armed annotation, while an operand/pattern/
    /// argument shorthand inside the same initializer reads its own
    /// parent's face (#3568 — `E e = (m == .x)` resolves `.x` against the
    /// left operand's type, not `E`).
    pub(crate) fn resolve_decl_shorthands(&mut self) {
        let Some(mark) = self.init_mark else {
            return;
        };
        for index in mark..self.rows.len() {
            self.verdict_shorthand_at(index, true);
        }
    }

    /// The shared shorthand verdict (#3433's two-arm rule, widened to every
    /// computable position by #3568, both arms reporting since #3612): the
    /// row resolves against its position's context — a hit upgrades the row
    /// to the member's declared face, a miss against a NAMED ACCESSIBLE
    /// context head reports the head's NAME (`dotShorthandUndefinedGetter`
    /// /`...UndefinedInvocation` spell `typeReference.name`/
    /// `receiver.displayName` — `List<int>` reports `'List'`), and every
    /// weaker context keeps each arm's own face: the invocation arm reports
    /// the schema it was visited under (`'_'`, `dynamic`, `bool`, a
    /// written schema), the read arm reports `dot_shorthand_missing_context`
    /// at the WHOLE shorthand span (upstream
    /// `dotShorthandMissingContext.at(node)`,
    /// property_element_resolver.dart:129).
    ///
    /// `frame_live` marks rows whose innermost executable still sits on the
    /// frame stack (`self.rows`): `return`/`=>` positions then read the
    /// live frame's return face. A deferred row's executable already
    /// closed — its verdict ran at the closer's own sweep, so a re-sweep
    /// keeps the context-free answer instead of borrowing the enclosing
    /// frame's return.
    fn verdict_shorthand_at(&mut self, index: usize, live: bool) {
        let row = self.rows[index].clone();
        if !self.is_shorthand_head(&row) {
            return;
        }
        let Some((resolve, schema)) = self.shorthand_ctx_at(&row, live) else {
            return;
        };
        if let Some(ty) = self.verdict_shorthand_miss(&row, resolve, schema) {
            self.rows[index].shape = RowShape::Bare(BareInfo::Plain { ty });
            self.rows[index].ty = ty;
        }
    }

    /// The shared miss verdict once a position's context resolves: a
    /// member/constructor hit answers the member's declared face (the
    /// caller upgrades its own row slot); a miss emits the position's row.
    /// Both arms report their own face on every weaker context — a
    /// non-interface shape (`dynamic`, `Never`, `void`, a function or
    /// record type), a decidable-but-empty schema (`'_'`), or a private
    /// context head this unit cannot see (an imported alias's `_C` right
    /// — the element lookup stops at the import's privacy boundary, so
    /// the head never enters the member walk and never spells itself).
    fn verdict_shorthand_miss(
        &mut self,
        row: &Row,
        resolve: Option<Ty>,
        schema: String,
    ) -> Option<Ty> {
        let resolve = resolve.filter(|ty| match ty {
            Ty::Named { name, .. } => self.shorthand_head_accessible(*name),
            _ => true,
        });
        // The arm split is SYNTACTIC upstream (DotShorthandInvocation vs
        // DotShorthandPropertyAccess node kinds), not an intent fact: a
        // `++`/`--`-demoted invocation chain (`.new()++`) keeps the
        // invocation arm's `'_'` verdict.
        let invoke = row.intent == Intent::Invoke || self.selector_invokes_at(row.range.offset);
        // An INVOKED `.id<typeArgs>` whose `id` names a constructor is a
        // compile-time error before the member verdict runs: type
        // parameters can never apply to a dot-shorthand constructor
        // invocation (instance_creation_expression_resolver.dart:105-114
        // — `wrongNumberOfTypeArgumentsDotShorthandConstructor` under the
        // shared `_constructor` code, `.at(typeArguments)`). The gate is
        // the INVOKED spelling only — the rewritten node kind is the
        // invocation; a tear-off `.id<typeArgs>` resolves as a
        // constructor tear-off whose group validates against the torn
        // function type (`wrong_number_of_type_arguments_function`),
        // never this code. The report does NOT suppress the expression's
        // own resolution, so the member verdict below still types the
        // row.
        if invoke && let Some(Ty::Named { name: head, .. }) = resolve {
            self.emit_shorthand_ctor_type_args(row, head);
        }
        let resolved = resolve.and_then(|ctx| {
            let Ty::Named {
                name: head,
                nullable,
                args,
            } = ctx
            else {
                return None;
            };
            self.shorthand_member_ty(head, nullable, args, row.name)
        });
        match (resolved, invoke) {
            (Some(ty), _) => Some(ty),
            (None, true) => {
                // The invocation arm reports unconditionally once its
                // context is not an accessible interface type
                // (method_invocation_resolver.dart:290-304): a named head
                // spells its own name, every weaker context the schema it
                // was visited under.
                let text = match resolve {
                    Some(Ty::Named { name, .. }) => self.strings.resolve(name).to_string(),
                    _ => schema,
                };
                self.emit_shorthand_undefined_member(row.range, &text);
                None
            }
            (None, false) => {
                // The property-access arm reports a named accessible head
                // at the selector (`dotShorthandUndefinedGetter`); every
                // weaker context reports the whole-span
                // `dot_shorthand_missing_context`.
                match resolve {
                    Some(Ty::Named { name, .. }) => {
                        let text = self.strings.resolve(name).to_string();
                        self.emit_shorthand_undefined_member(row.range, &text);
                    }
                    _ => self.emit_shorthand_missing_context(row.range),
                }
                None
            }
        }
    }

    /// `wrong_number_of_type_arguments_constructor` for a dot-shorthand
    /// constructor invocation that wrote a `<...>` group — the shared
    /// `_constructor` code under the dot-shorthand face's own text
    /// (messages.yaml's `wrongNumberOfTypeArgumentsDotShorthandConstructor`
    /// `sharedName`, :19930; `instance_creation_expression_resolver.dart`
    /// :105-114 reports it `.at(typeArguments)` whenever the shorthand's
    /// context denotes an accessible class, unconditionally — the group's
    /// own arity never enters).
    ///
    /// The gate is the RESOLVED selector, not the written spelling: `id`
    /// must name a constructor of the context head (`.new`/`.id`), a plain
    /// static member keeps the member face (`.staticMethod<int>()` is a
    /// valid generic-method instantiation — `type_inference_A09_t01`'s
    /// `cContextProvider(.staticMethod<int>())` reports nothing). The
    /// group is the balanced `<...>` the selector's own span is followed
    /// by; a selector with no group is the silent face.
    ///
    /// Upstream's abstract arm REPLACES this row for an abstract class
    /// whose resolved constructor is generative
    /// (`instance_creation_expression_resolver.dart:96-101`:
    /// `contextElement.isAbstract && constructorElement != null &&
    /// !constructorElement.isFactory` reports `instantiateAbstractClass`
    /// instead, the else-if never reaching the type-arguments report) —
    /// the gate suppresses so that face's owner owns the report alone.
    fn emit_shorthand_ctor_type_args(&mut self, row: &Row, head: StringId) {
        let Some(group) = self.shorthand_type_args_span(row.range) else {
            return;
        };
        let site = self.ctor_decl_site(head, row.name);
        let container = resolve_container(
            self.graph,
            self.namespaces,
            self.index,
            self.lib,
            self.file,
            head,
        );
        let ctor_hit = site.is_some()
            || container
                .as_ref()
                .is_some_and(|(decl, _)| self.ctor_face_exists(decl, head, row.name));
        if !ctor_hit {
            return;
        }
        // The unnamed factory keys the class's own name; a named factory
        // its selector.
        let unnamed = row.name == head || row.name == self.intern("new");
        let factory = site.is_some_and(|s| s.is_factory)
            || container.as_ref().is_some_and(|(decl, _)| {
                decl.factory_formals
                    .contains_key(&if unnamed { head } else { row.name })
            });
        if !factory
            && container
                .is_some_and(|(decl, _)| decl.kind == ContainerKind::Class && decl.is_abstract)
        {
            return;
        }
        // The oracle's `.at(typeArguments)` anchor is the `<...>` group.
        let class = self.strings.resolve(head).to_string();
        let ctor = self.strings.resolve(row.name).to_string();
        self.emit_shorthand_ctor_type_args_at(group, &class, &ctor);
    }

    /// The `<...>` group following the selector at `selector` — the
    /// balanced run starting at the first non-trivia `<`. `None` when the
    /// selector carries no group (or the group never closes: the parser's
    /// own recovery owns that shape).
    fn shorthand_type_args_span(&self, selector: SourceRange) -> Option<SourceRange> {
        use dartq_scanner::TokenKind as TK;
        let toks = self.tokens.as_slice();
        let idx = toks.partition_point(|t| t.utf16_offset < selector.end());
        let lt = toks.get(idx).filter(|t| t.kind == TK::Lt)?;
        let close = self.tokens.matching_end(idx)?;
        let close = toks.get(close)?;
        Some(SourceRange {
            offset: lt.utf16_offset,
            length: (close.utf16_offset + close.byte_len).saturating_sub(lt.utf16_offset),
        })
    }

    /// The `wrong_number_of_type_arguments_constructor` row under the
    /// dot-shorthand text (`sharedName` reuse: the code name and machine
    /// form stay the shared `_constructor` face, the message is the
    /// dot-shorthand variant's).
    fn emit_shorthand_ctor_type_args_at(&mut self, range: SourceRange, class: &str, ctor: &str) {
        if self.report.diagnostics.iter().any(|d| {
            d.range == range && d.code.name == "wrong_number_of_type_arguments_constructor"
        }) {
            return;
        }
        let message = format!(
            "The dot shorthand resolves to the constructor '{class}.{ctor}', and type \
             parameters can't be applied to dot shorthand constructor invocations."
        );
        self.report.diagnostics.push(Diagnostic::new(
            SemanticCode::WrongNumberOfTypeArgumentsConstructor.severity(),
            SemanticCode::WrongNumberOfTypeArgumentsConstructor.code_info(),
            message,
            self.file,
            range,
            None,
        ));
    }

    /// Whether a `Named` context head names a type THIS unit can see: a
    /// private spelling (`_C`) that resolves to no type here belongs to a
    /// sibling library, and the analyzer's target-element lookup stops at
    /// the import's privacy boundary — the context then is not an
    /// accessible interface type, so both arms report through their weak
    /// faces (the invocation spells the schema `'_C'`, the read arm
    /// reports `missing_context`; neither spells the head as an element).
    fn shorthand_head_accessible(&self, name: StringId) -> bool {
        let spelling = self.strings.resolve(name);
        if !spelling.starts_with('_') {
            return true;
        }
        matches!(self.type_resolution(name).0, TypeResolution::Type)
    }

    /// `dot_shorthand_missing_context` at the WHOLE shorthand span — the
    /// dot through the selector's end (upstream reports the node, not the
    /// name: `dotShorthandMissingContext.at(node)`,
    /// property_element_resolver.dart:129). The const face emits the same
    /// span for the const roots it reaches, so the emit dedups on it; a
    /// stale-window resweep likewise stays idempotent.
    fn emit_shorthand_missing_context(&mut self, selector: SourceRange) {
        let toks = self.tokens.as_slice();
        let idx = toks.partition_point(|t| t.utf16_offset < selector.offset);
        let Some(dot) = idx.checked_sub(1).and_then(|i| toks.get(i)) else {
            return;
        };
        let span = SourceRange {
            offset: dot.utf16_offset,
            length: selector.end() - dot.utf16_offset,
        };
        if self
            .report
            .diagnostics
            .iter()
            .any(|d| d.range == span && d.code.name == "dot_shorthand_missing_context")
        {
            return;
        }
        self.emit(SemanticCode::DotShorthandMissingContext, span, "", "");
    }

    /// Whether the selector at `offset` is written as an invocation — the
    /// name, one optional balanced type-argument group, then the call's
    /// `(` (the token-level twin of the const face's byte probe): the
    /// verdict's arm split follows the WRITTEN node kind, so a selector
    /// whose invocation shape an operator event demoted
    /// (`.new()++`'s `ReadWrite`) still verdicts as the invocation arm.
    fn selector_invokes_at(&self, offset: u32) -> bool {
        use dartq_scanner::TokenKind as TK;
        let toks = self.tokens.as_slice();
        let mut idx = toks.partition_point(|t| t.utf16_offset < offset) + 1;
        if toks.get(idx).is_some_and(|t| t.kind == TK::Lt) {
            let mut depth = 0i32;
            while let Some(t) = toks.get(idx) {
                match t.kind {
                    TK::Lt => depth += 1,
                    TK::LtLt => depth += 2,
                    TK::Gt => depth -= 1,
                    TK::GtGt => depth -= 2,
                    TK::GtGtGt => depth -= 3,
                    _ => {}
                }
                idx += 1;
                if depth <= 0 {
                    break;
                }
            }
        }
        toks.get(idx).is_some_and(|t| t.kind == TK::OpenParen)
    }

    /// The un-merged leading-dot selector gate shared by every shorthand
    /// sweep: `qualified` (the selector fold), `Bare(_)` (no receiver merged),
    /// `ty == Unknown` (never verdicted — the once-only marker), and the
    /// byte-level `.`-opener probe — or a `...`/`...?` spread head, whose
    /// `...` byte the opener probe reads as a receiver end, or a `!`-
    /// negation head (`!.a`), whose `!` byte reads as a postfix assert of
    /// a receiver. Write/compound/operator intents (``.red = 1;``,
    /// ``.red++;``) verdict with the others since #3612 — their statement
    /// positions arm the unknown schema, whose read arm reports
    /// `missing_context`.
    fn is_shorthand_head(&self, row: &Row) -> bool {
        row.qualified
            && matches!(row.shape, RowShape::Bare(_))
            && row.ty == Ty::UNKNOWN
            && matches!(
                row.intent,
                Intent::Read
                    | Intent::Invoke
                    | Intent::Write
                    | Intent::ReadWrite
                    | Intent::Operator(..)
            )
            && (self.dot_shorthand_context(row.range.offset)
                || self.is_spread_head(row.range.offset)
                || self.is_negation_shorthand_head(row.range.offset))
    }

    /// Whether the `.` before the selector opens a spread element
    /// (`... .x` / `...? .x`): the token before the dot is a spread
    /// operator. The byte-level opener probe rejects that shape (a `.`
    /// before the dot reads as a receiver end), so the sweep admits it
    /// through this arm and [`Self::spread_ctx_at`] supplies the context.
    fn is_spread_head(&self, offset: u32) -> bool {
        use dartq_scanner::TokenKind as TK;
        let toks = self.tokens.as_slice();
        let name_idx = toks.partition_point(|t| t.utf16_offset < offset);
        let Some(dot_idx) = name_idx.checked_sub(1) else {
            return false;
        };
        let Some(prev_idx) = dot_idx.checked_sub(1) else {
            return false;
        };
        matches!(
            toks[prev_idx].kind,
            TK::PeriodPeriodPeriod | TK::PeriodPeriodPeriodQuestion
        )
    }

    /// Whether the `.` before the selector opens a NEGATED shorthand
    /// (`!.a`, `!!.a`): the `!`-run before the dot roots at a position
    /// token (an operator, `=`, `(`, `,`, `;`), not at the postfix `!` of
    /// a written receiver — `x!.a` and `x!!.a` end their `!`-runs at an
    /// expression token, and the byte-level opener probe's rejection
    /// stands for them. A genuine negation's context is the language's
    /// `bool` operand schema (`o = !.getterTrue` reports `'bool'`).
    fn is_negation_shorthand_head(&self, offset: u32) -> bool {
        use dartq_scanner::{Keyword as Kw, TokenKind as TK};
        let toks = self.tokens.as_slice();
        let name_idx = toks.partition_point(|t| t.utf16_offset < offset);
        let Some(dot_idx) = name_idx.checked_sub(1) else {
            return false;
        };
        let mut idx = dot_idx;
        while idx > 0 && toks[idx - 1].kind == TK::Bang {
            idx -= 1;
        }
        if idx == 0 || idx == dot_idx {
            return false;
        }
        !matches!(
            toks[idx - 1].kind,
            TK::Identifier
                | TK::Int
                | TK::Double
                | TK::Hexadecimal
                | TK::String
                | TK::StringInterpolationIdentifier
                | TK::CloseParen
                | TK::CloseSquare
                | TK::CloseCurly
                | TK::Gt
                | TK::Bang
                | TK::Keyword(Kw::This | Kw::Super | Kw::True | Kw::False | Kw::Null)
        )
    }

    /// The body-end shorthand sweep (#3568): runs at every executable body's
    /// close ([`Self::stage_body_completion`], before the frame pops and —
    /// for literals — before the interior rows drain), verdicting every
    /// unresolved shorthand head the initializer pass never reached: operand
    /// (`x == .a`), pattern (`case .a`, `case == .a`, switch-expression
    /// arms), argument (`take(.a)`), return (`return .a`), condition
    /// (`if (.a)`), `??`-right, collection-element, and assignment-target
    /// positions. `self.rows` rows carry live frames; deferred rows
    /// (closed literal interiors) re-run only the frame-free positions —
    /// their frame-bound verdicts were decided at their own close.
    fn sweep_body_shorthands(&mut self) {
        for index in 0..self.rows.len() {
            let owner_live = self
                .frames
                .iter()
                .rev()
                .find(|frame| frame.row_mark <= index)
                .is_some_and(|frame| matches!(frame.kind, FrameKind::Executable { .. }));
            self.verdict_shorthand_at(index, owner_live);
        }
        for index in 0..self.deferred_miss_rows.len() {
            let row = self.deferred_miss_rows[index].clone();
            if !self.is_shorthand_head(&row) {
                continue;
            }
            let Some((resolve, schema)) = self.shorthand_ctx_at(&row, false) else {
                continue;
            };
            self.verdict_deferred_shorthand(index, &row, resolve, schema);
        }
    }

    /// The shorthand's per-position context (#3568): the type the shorthand
    /// resolves against plus the schema text the weaker verdicts render.
    /// Upstream arms the context while walking into the shorthand
    /// (`pushDotShorthandContext` on the `==` right operand
    /// — binary_expression_resolver.dart:112 — the parent's `contextType`
    /// on every other node); this row-level pass recomputes it from the
    /// token before the shorthand's `.`.
    ///
    /// The verdict answer is three-way: `Some(Some(ty), schema)` — a
    /// decidable context (a `Ty::Named` head the member walk reads, or a
    /// weaker shape — `dynamic`, `bool`, a written non-interface schema);
    /// `Some(None, "_")` — the position provably arms no context (the
    /// invoke arm's `'_'` upstream; the read arm's
    /// `dot_shorthand_missing_context`); `None` — the position cannot be
    /// decided here (an operand the row walk cannot type, a type-parameter
    /// formal, a shape no arm pins) — the row keeps its silence rather
    /// than fabricate an `undefined` row upstream never emits (the
    /// under-report direction every shorthand face outside the declaration
    /// window keeps).
    fn shorthand_ctx_at(&self, row: &Row, live: bool) -> Option<(Option<Ty>, String)> {
        use dartq_scanner::{Keyword as Kw, TokenKind as TK};
        let toks = self.tokens.as_slice();
        // The `.` before the selector name: the selector token sits at
        // `row.range.offset`; the token immediately before it is the dot.
        let name_idx = toks.partition_point(|t| t.utf16_offset < row.range.offset);
        let dot_idx = name_idx.checked_sub(1)?;
        let prev_idx = dot_idx.checked_sub(1)?;
        let prev = toks[prev_idx];
        // The equality-operand rule (`binary_expression_resolver.dart`
        // `_resolveEqual`): both operands are analyzed with the unknown
        // schema, so only a DIRECT dot shorthand on the right reads the left
        // operand's static type. Every shorthand nested in either operand's
        // subtree — the left root, a parenthesized or conditional right —
        // verdicts against `'_'`: the read arm reports the whole-span
        // `missing_context` and the invocation arm the empty schema text,
        // exactly the declaration-window answers.
        if self.shorthand_roots_equality_lhs(name_idx, prev_idx) {
            return Some((None, "_".to_string()));
        }
        if self.shorthand_nested_in_equality_rhs(dot_idx) {
            return Some((None, "_".to_string()));
        }
        // A shorthand that roots the LEFT operand of `is`/`as` resolves
        // against NO context: upstream analyzes a type test's and a cast's
        // operands under the unknown schema (`binary_expression_resolver.dart`
        // — neither operator pushes a dot-shorthand context), so the read
        // arm reports the whole-span `missing_context` and the invocation
        // arm the empty schema. The enclosing bool position never leaks into
        // that operand (`if (.one is C)` reports `missing_context`, not a
        // `'bool'` member miss).
        if matches!(
            self.shorthand_chain_end(name_idx),
            Some(end) if matches!(toks[end].kind, TK::Keyword(Kw::Is | Kw::As))
        ) {
            return Some((None, "_".to_string()));
        }
        // A shorthand whose selector chain an increment/decrement
        // terminates (`.new()++`, `.red++`) — the operand schema is the
        // unknown one upstream (test_postfixOperator's `C c = .new()++`
        // reports `'_'`).
        if let Some(end) = self.shorthand_chain_end(name_idx)
            && matches!(toks[end].kind, TK::PlusPlus | TK::MinusMinus)
        {
            return Some((None, "_".to_string()));
        }
        // The row whose written expression ends just before `token`'s
        // offset — the operand/context expression the position reads.
        // Searched from the shorthand's own index backward in `self.rows`,
        // then the deferred tail. The `EqEq`/`BangEq` arm below prepends a
        // `deferred_miss_rows`-first search to dodge the `==` composite's
        // `Value` row; every other caller keeps this order.
        let operand_row_before = |engine: &Self, end: u32, before: usize| -> Option<Row> {
            engine
                .rows
                .iter()
                .take(before)
                .rev()
                .find(|c| engine.trivia_between(engine.staged_expression_span(c.range).end(), end))
                .or_else(|| {
                    engine.deferred_miss_rows.iter().rev().find(|c| {
                        engine.trivia_between(engine.staged_expression_span(c.range).end(), end)
                    })
                })
                .cloned()
        };
        // The row index of the shorthand itself within `self.rows` (the
        // deferred rows fall back to the deferred-window search only).
        let self_index = self
            .rows
            .iter()
            .position(|c| c.range == row.range)
            .unwrap_or(self.rows.len());
        let ty_pair = |engine: &Self, ty: Ty| -> Option<(Option<Ty>, String)> {
            Some((Some(ty), engine.ty_display(ty)))
        };
        match prev.kind {
            // `x == .a` / `x != .a` — the right operand's context is the
            // LEFT operand's static type
            // (binary_expression_resolver.dart:112-116). An untypedable
            // left operand keeps the under-report rather than a `'_'` the
            // oracle's `left.typeOrThrow` would still fill.
            TK::EqEq | TK::BangEq => {
                // The `==`/`!=` fold drains the operand rows to
                // `deferred_miss_rows` and pushes a `Value` composite
                // (typed `bool`, spanning the whole binary) onto
                // `self.rows` — searching `self.rows` first picks that
                // composite and fabricates a `bool` context for the right
                // operand's shorthand (`C() == .g` reads `'bool'` where
                // the oracle resolves `C`). The real operand is the last
                // `deferred_miss_rows` entry; `self.rows` is the fallback
                // for shapes that leave no composite (unfolded literals
                // in a `??` tail, a shorthand's own right operand). Every
                // candidate must END before the operator: inside an
                // argument group the composite itself drains to
                // `deferred_miss_rows` too, and its staged span (keyed at
                // the shared start) aliases the left operand's
                // (`take(C(1) == .missing)` read `'bool'`).
                let eq_operand = |engine: &Self, end: u32, before: usize| -> Option<Row> {
                    let left_of = |c: &&Row| {
                        c.range.end() <= end
                            && engine
                                .trivia_between(engine.staged_expression_span(c.range).end(), end)
                    };
                    engine
                        .deferred_miss_rows
                        .iter()
                        .rev()
                        .find(left_of)
                        .or_else(|| engine.rows.iter().take(before).rev().find(left_of))
                        .cloned()
                };
                match eq_operand(self, prev.utf16_offset, self_index) {
                    // A shorthand as the LEFT operand arms no context for
                    // the right — upstream's special-case covers only the
                    // shorthand's own site; the right operand reads `'_'`.
                    Some(c) if c.qualified && matches!(c.shape, RowShape::Bare(_)) => {
                        Some((None, "_".to_string()))
                    }
                    Some(c) => {
                        let ty = self.operand_static_ty(&c);
                        if ty == Ty::UNKNOWN {
                            // `case == .a` / `case != .a`: the relational
                            // pattern's `==` has no left operand — the
                            // context is the scrutinee's matched type.
                            self.scrutinee_ctx_at(prev_idx)
                        } else {
                            ty_pair(self, ty)
                        }
                    }
                    None => self.scrutinee_ctx_at(prev_idx),
                }
            }
            // `case .a:` / `case .a =>` — the constant pattern's expression
            // resolves against the matched type (the scrutinee's static
            // type — analyzeConstantPattern's `matchedValueType`,
            // type_analyzer.dart:522).
            TK::Keyword(Kw::Case) => self.case_scrutinee_ctx(prev_idx),
            // `return .a` / `=> .a` — the innermost executable's declared
            // return face; a deferred row's executable already closed
            // (`live == false`), so it keeps the context-free answer.
            TK::Keyword(Kw::Return | Kw::Yield) | TK::Function => {
                if !live {
                    None
                } else {
                    self.return_ctx()
                }
            }
            // `&&`/`||` — the language-defined `bool` operand context
            // (`bool_context_error_test`), EXCEPT inside a `case` pattern,
            // where the logical operands resolve against the scrutinee's
            // matched type (`if (e case .one || .two)` — patterns_A02).
            // `!` always arms `bool` (a negation operand, never a pattern
            // position).
            TK::AmpersandAmpersand | TK::BarBar => {
                self.pattern_case_ctx(prev_idx).or_else(|| self.bool_ctx())
            }
            TK::Bang => self.bool_ctx(),
            // `... .x` / `...? .x` — a spread element resolves against the
            // collection the literal builds: `Iterable` for a list/set,
            // `Map` for a map (`non_ambiguity_A02_*`).
            TK::PeriodPeriodPeriod | TK::PeriodPeriodPeriodQuestion => self.spread_ctx_at(dot_idx),
            // `(`, `,` — argument-list or collection/record positions.
            TK::OpenParen | TK::Comma => self.group_ctx_at(row, prev_idx, live, operand_row_before),
            // `:` — a named-argument label's value (`f(name: .a)`), else a
            // conditional else-branch or a map entry's value.
            TK::Colon => self
                .colon_ctx_at(row, prev_idx, operand_row_before)
                .or_else(|| self.conditional_branch_ctx(prev_idx, live)),
            // `=` — the declaration initializer's own shorthand
            // (`E e = .a`): the armed annotation's face, decidable only
            // while the declaration's own window is live (`pending_decl_ty`
            // pops at the declarator's close — a body-sweep re-verdict
            // reads a stale or absent face). Any other `=` shape — an
            // assignment target — keeps the under-report: the target's own
            // face owns the shorthand.
            TK::Eq if self.init_mark.is_some_and(|mark| self_index >= mark) => {
                self.eq_ctx_at(prev_idx)
            }
            TK::Eq => None,
            // A conditional's then-branch (`cond ? .a : b`): the branch
            // inherits the conditional's own visiting schema —
            // `if (c == x ? .new() : y)` reports `'bool'`, an `==`-nested
            // conditional keeps `'_'` (equality_A02_t01/t02).
            TK::Question => self.conditional_branch_ctx(prev_idx, live),
            // Prefix `-`/`~`/`++`/`--` operands — no language-defined
            // operand schema: the unknown schema (`o = -.two()` reports
            // `'_'`; an infix right operand `- .a` shares the answer).
            TK::Minus | TK::Tilde | TK::PlusPlus | TK::MinusMinus => Some((None, "_".to_string())),
            // A list/set literal's first element: the element schema
            // (equality_A02_t01's `if ([C(0)] == [.id(1)])` reports `'_'`).
            TK::OpenSquare => self.literal_elem_ctx(prev_idx),
            // `{` — a block statement's first expression arms the unknown
            // schema (`) {` of a body, `else {`; no_context's statement
            // shapes); a set/map literal's first element answers through
            // the element face.
            TK::OpenCurly => match prev_idx
                .checked_sub(1)
                .and_then(|i| toks.get(i))
                .map(|t| t.kind)
            {
                Some(TK::CloseParen | TK::Semicolon | TK::CloseCurly)
                | Some(TK::Keyword(Kw::Else | Kw::Do)) => Some((None, "_".to_string())),
                _ => self.literal_elem_ctx(prev_idx),
            },
            // An expression statement's own shorthand — `.red = 1;`,
            // `.red++;`, `.one!;` — arms no context anywhere: the unknown
            // schema (no_context's read shapes report `missing_context`).
            TK::Semicolon | TK::CloseCurly => Some((None, "_".to_string())),
            // `await .red;` as the statement's whole expression — the
            // await's own context is the unknown schema; an `await` nested
            // in a larger expression keeps the under-report.
            TK::Keyword(Kw::Await)
                if matches!(
                    prev_idx
                        .checked_sub(1)
                        .and_then(|i| toks.get(i))
                        .map(|t| t.kind),
                    Some(TK::Semicolon | TK::OpenCurly | TK::CloseCurly) | None
                ) =>
            {
                Some((None, "_".to_string()))
            }
            _ => None,
        }
    }

    /// Whether the shorthand at `name_idx` roots the LEFT operand of an
    /// `==`/`!=` comparison (`binary_expression_resolver.dart` analyzes that
    /// operand under the unknown schema): the selector chain that starts at
    /// the shorthand's name runs through its invocations, index reads, and
    /// member selects — plus a grouping paren around the whole operand —
    /// and stops exactly at the equality operator. A shorthand that is
    /// already the operator's right operand is excluded (its own arm above
    /// reads the left operand's type); a chain that ends on any other token
    /// (an argument, a condition, a declaration initializer) keeps the
    /// position's own context.
    fn shorthand_roots_equality_lhs(&self, name_idx: usize, prev_idx: usize) -> bool {
        use dartq_scanner::TokenKind as TK;
        if matches!(self.tokens.as_slice()[prev_idx].kind, TK::EqEq | TK::BangEq) {
            return false;
        }
        matches!(
            self.shorthand_chain_end(name_idx),
            Some(end) if matches!(self.tokens.as_slice()[end].kind, TK::EqEq | TK::BangEq)
        )
    }

    /// The token that terminates the shorthand-rooted selector chain at
    /// `name_idx`: the chain runs through its invocations, index reads,
    /// and member selects — plus a grouping paren around the whole
    /// operand — and stops at the first token that is none of those.
    /// [`Self::shorthand_roots_equality_lhs`] reads it for the `==`/`!=`
    /// that makes the shorthand a LEFT operand; the `++`/`--` terminator
    /// (`.new()++`) answers the unknown schema the same way. `None` when
    /// the chain never completes (an unmatched bracket, a non-grouping
    /// close).
    fn shorthand_chain_end(&self, name_idx: usize) -> Option<usize> {
        use dartq_scanner::TokenKind as TK;
        let toks = self.tokens.as_slice();
        let mut idx = name_idx + 1;
        while let Some(token) = toks.get(idx) {
            match token.kind {
                TK::OpenParen | TK::OpenSquare => match self.tokens.matching_end(idx) {
                    Some(end) => idx = end + 1,
                    None => return None,
                },
                TK::Period | TK::QuestionPeriod => idx += 2,
                // A close that ends the operand: only a GROUPING paren wraps
                // the operand (`(...) == x`); an invocation's `)` belongs to
                // the chain's callee and is already consumed above, so a
                // close reached here either wraps the operand (step past it
                // and keep scanning) or ends the enclosing construct.
                TK::CloseParen => match self.grouping_open(idx) {
                    Some(_) => idx += 1,
                    None => return None,
                },
                _ => return Some(idx),
            }
        }
        None
    }

    /// The opener of the group a `)` closes, when that group is a grouping
    /// paren (the token before the opener is not a callee end — an
    /// identifier, a `)`/`]`, or a type argument's `>`). `None` for a call
    /// group: its contents are the callee's arguments, never the operand's
    /// own extent.
    fn grouping_open(&self, close_idx: usize) -> Option<usize> {
        use dartq_scanner::TokenKind as TK;
        let toks = self.tokens.as_slice();
        let mut depth = 0i32;
        let mut open = None;
        for idx in (0..close_idx).rev() {
            match toks[idx].kind {
                TK::CloseParen | TK::CloseSquare | TK::CloseCurly => depth += 1,
                TK::OpenParen => {
                    if depth == 0 {
                        open = Some(idx);
                        break;
                    }
                    depth -= 1;
                }
                TK::OpenSquare | TK::OpenCurly => {
                    if depth == 0 {
                        return None;
                    }
                    depth -= 1;
                }
                _ => {}
            }
        }
        let open = open?;
        if open == 0 {
            return Some(open);
        }
        (!matches!(
            toks[open - 1].kind,
            TK::Identifier | TK::CloseParen | TK::CloseSquare | TK::Gt | TK::GtGt | TK::GtGtGt
        ))
        .then_some(open)
    }

    /// Whether the shorthand at `dot_idx` sits inside an expression that is
    /// the DIRECT right operand of `==`/`!=` — a parenthesized or conditional
    /// subtree. Upstream pushes no shorthand context onto such an operand
    /// (only a direct dot shorthand reads the left operand's type), so the
    /// nested shorthand verdicts against `'_'`.
    fn shorthand_nested_in_equality_rhs(&self, dot_idx: usize) -> bool {
        use dartq_scanner::TokenKind as TK;
        let Some(open) = self.enclosing_group_open(dot_idx) else {
            return false;
        };
        let Some(before) = open.checked_sub(1) else {
            return false;
        };
        matches!(self.tokens.as_slice()[before].kind, TK::EqEq | TK::BangEq)
    }

    /// The index of the opener matching the closer at `close_idx` — the
    /// token store carries only the forward `matching_end`, so the
    /// backward answer counts bracket depth (the conditional-branch walk
    /// needs it to step over completed groups).
    fn matching_open_idx(&self, close_idx: usize) -> Option<usize> {
        use dartq_scanner::TokenKind as TK;
        let toks = self.tokens.as_slice();
        let open = match toks.get(close_idx)?.kind {
            TK::CloseParen => TK::OpenParen,
            TK::CloseSquare => TK::OpenSquare,
            TK::CloseCurly => TK::OpenCurly,
            _ => return None,
        };
        let mut depth = 0i32;
        for idx in (0..close_idx).rev() {
            match toks[idx].kind {
                TK::CloseParen | TK::CloseSquare | TK::CloseCurly => depth += 1,
                kind if kind == open && depth == 0 => return Some(idx),
                TK::OpenParen | TK::OpenSquare | TK::OpenCurly => depth -= 1,
                _ => {}
            }
        }
        None
    }

    /// The context of a `&&`/`||` token that sits INSIDE a `case` pattern's
    /// logical-operand chain (`if (e case .one || .two)`: the pattern's
    /// operands resolve against the scrutinee's matched type, not `bool`).
    /// `None` when the scan leftward reaches a statement/group boundary
    /// first — ordinary boolean logic, the `bool` face answers.
    fn pattern_case_ctx(&self, op_idx: usize) -> Option<(Option<Ty>, String)> {
        use dartq_scanner::{Keyword as Kw, TokenKind as TK};
        let toks = self.tokens.as_slice();
        let mut depth = 0i32;
        for idx in (0..op_idx).rev() {
            match toks[idx].kind {
                TK::CloseParen | TK::CloseSquare | TK::CloseCurly => depth += 1,
                TK::OpenParen | TK::OpenSquare | TK::OpenCurly => {
                    if depth == 0 {
                        return None;
                    }
                    depth -= 1;
                }
                TK::Semicolon => return None,
                TK::Keyword(Kw::Case) if depth == 0 => return self.case_scrutinee_ctx(idx),
                _ => {}
            }
        }
        None
    }

    /// A conditional-expression branch's context (`cond ? .a : b` /
    /// `cond ? b : .a`): both branches inherit the conditional's own
    /// visiting schema, so the answer classifies the boundary that opens
    /// the conditional's TEST expression — `if (… == c ? .new() : …)`
    /// arms `bool`, a declaration initializer's conditional the written
    /// annotation, an `==`-nested conditional the unknown schema. A `:`
    /// with no `?` behind it is a map entry's value — the enclosing
    /// literal's element face answers.
    fn conditional_branch_ctx(
        &self,
        branch_idx: usize,
        live: bool,
    ) -> Option<(Option<Ty>, String)> {
        use dartq_scanner::{Keyword as Kw, TokenKind as TK};
        let toks = self.tokens.as_slice();
        let is_colon = toks.get(branch_idx).is_some_and(|t| t.kind == TK::Colon);
        let cursor = if is_colon {
            match self.conditional_question_before(branch_idx) {
                Some(q) => q,
                // A map entry's value: the element face.
                None => return self.literal_elem_ctx(self.enclosing_group_open(branch_idx)?),
            }
        } else {
            branch_idx
        };
        // (The `?`-pairing scan is `conditional_question_before`'s,
        // invocation.rs's shared helper — nested conditionals pair their
        // own `?`/`:` at depth.)
        // Walk left over the test expression — operands, literals,
        // operators, and completed groups all continue it — to the
        // boundary token that opens the position; that boundary's own
        // face is the branch's context.
        let mut idx = cursor;
        while idx > 0 {
            idx -= 1;
            match toks[idx].kind {
                TK::CloseParen | TK::CloseSquare | TK::CloseCurly => {
                    idx = self.matching_open_idx(idx)?;
                }
                TK::OpenParen
                | TK::OpenSquare
                | TK::OpenCurly
                | TK::Comma
                | TK::Semicolon
                | TK::Question
                | TK::Colon
                | TK::Eq
                | TK::Function
                | TK::Keyword(
                    Kw::If | Kw::While | Kw::Assert | Kw::Return | Kw::Yield | Kw::Else | Kw::Do,
                ) => return self.expr_boundary_ctx(idx, live),
                _ => {}
            }
        }
        Some((None, "_".to_string()))
    }

    /// The context of the expression whose first token sits right AFTER
    /// `stop_idx` — the boundary that opens the position: statement
    /// separators arm the unknown schema, `if (`/`while (`/`assert (`
    /// the `bool` schema, a declarator `=` the written annotation, `=>`
    /// and `return`/`yield` the return face, an open bracket whatever
    /// schema the bracket's own position arms (`==` operand → `'_'`), a
    /// nested conditional's branch token recurses one level up.
    fn expr_boundary_ctx(&self, stop_idx: usize, live: bool) -> Option<(Option<Ty>, String)> {
        use dartq_scanner::{Keyword as Kw, TokenKind as TK};
        let toks = self.tokens.as_slice();
        match toks[stop_idx].kind {
            TK::Semicolon | TK::OpenCurly | TK::CloseCurly => Some((None, "_".to_string())),
            // A declarator `=` arms the annotation (`E e = c ? .a : b`);
            // an assignment target keeps the under-report.
            TK::Eq => self.eq_ctx_at(stop_idx),
            TK::Function | TK::Keyword(Kw::Return | Kw::Yield) => {
                if live {
                    self.return_ctx()
                } else {
                    Some((None, "_".to_string()))
                }
            }
            // A nested conditional's branch token: classify one level up.
            TK::Question | TK::Colon => self.conditional_branch_ctx(stop_idx, live),
            TK::OpenParen | TK::OpenSquare => {
                match stop_idx
                    .checked_sub(1)
                    .and_then(|i| toks.get(i))
                    .map(|t| t.kind)
                {
                    // `if (` / `while (` / `assert (` — the `bool` schema.
                    Some(TK::Keyword(Kw::If | Kw::While | Kw::Assert)) => self.bool_ctx(),
                    // An `==`/`!=` operand — the unknown schema.
                    Some(TK::EqEq | TK::BangEq) => Some((None, "_".to_string())),
                    // A grouping `(` inherits the schema the group
                    // expression itself was visited under — classify the
                    // boundary that opens it (`; (.a);` at statement level
                    // answers `'_'`; `E e = (.a)` the annotation). A call's
                    // open lands `None` through the same recursion (its
                    // predecessor is a callee end); a `[` literal keeps
                    // the under-report — `literal_elem_ctx` owns its
                    // element face.
                    Some(_) if toks[stop_idx].kind == TK::OpenParen => {
                        self.expr_boundary_ctx(stop_idx - 1, live)
                    }
                    _ => None,
                }
            }
            TK::Keyword(Kw::Else | Kw::Do) => Some((None, "_".to_string())),
            _ => None,
        }
    }

    /// A collection literal element's context (`[.a]`, `{.a}`, a later
    /// element after a `,`, a map entry's value): the element schema the
    /// LITERAL's own position arms — an `==`/`!=` operand and a condition
    /// position arm the unknown schema (`if ([C(0)] == [.id(1)])` reports
    /// `'_'`; `if ({"key": C(0)} != {"key": .intOne})` reports
    /// `missing_context`); a statement-level collection arms the unknown
    /// element schema; and the declaration initializer's own literal arms
    /// the group's inference face — an UNANNOTATED group (`var l = [.a]`)
    /// arms the unknown schema (the oracle reports the whole-span
    /// `dot_shorthand_missing_context`; an annotated group's element type
    /// lives in the annotation's type arguments, under-reported). An
    /// index access `a[...]` (an identifier/`)`/`]` before the bracket)
    /// keeps the under-report.
    fn literal_elem_ctx(&self, open_idx: usize) -> Option<(Option<Ty>, String)> {
        use dartq_scanner::{Keyword as Kw, TokenKind as TK};
        let toks = self.tokens.as_slice();
        let before = open_idx
            .checked_sub(1)
            .and_then(|i| toks.get(i))
            .map(|t| t.kind)?;
        match before {
            TK::EqEq | TK::BangEq => Some((None, "_".to_string())),
            // A collection directly inside the condition's own paren
            // (`if ([.a])`): the literal itself was visited under `bool`,
            // and its elements arm no context.
            TK::OpenParen => match open_idx
                .checked_sub(2)
                .and_then(|i| toks.get(i))
                .map(|t| t.kind)
            {
                Some(TK::Keyword(Kw::If | Kw::While | Kw::Assert)) => Some((None, "_".to_string())),
                _ => None,
            },
            // A statement-level collection (`[.a];`, a block's first
            // expression) arms the unknown element schema — the same
            // statement face [`Self::shorthand_ctx_at`]'s own `;`/`}`
            // arm runs (no_context's read shapes report
            // `missing_context`).
            TK::Semicolon | TK::OpenCurly | TK::CloseCurly => Some((None, "_".to_string())),
            // A nested collection's element inherits the enclosing
            // literal's own element face (`[[.a]]` classifies the outer
            // literal; a map value `{k: [.a]}` classifies the map).
            TK::OpenSquare => self.literal_elem_ctx(open_idx - 1),
            TK::Colon => self
                .enclosing_group_open(open_idx)
                .and_then(|open| self.literal_elem_ctx(open)),
            // The declaration initializer's own literal: an inference
            // group (`var`/`final`, no written type) arms the unknown
            // element schema — see [`Self::inference_group_literal_ctx`].
            TK::Eq => self.inference_group_literal_ctx(open_idx),
            _ => None,
        }
    }

    /// Whether the literal at `open_idx` is the initializer of an
    /// inference group's declarator (`var`/`final`, no written type): the
    /// declarator head classifies positionally (stable across the
    /// deferred re-verdicts, which run after the declaration window
    /// closed) — `name =` directly, or — for a later declarator after
    /// `,` — the balanced back-scan over the sibling's initializer to
    /// the previous `=`'s name and the group head beyond it. Such groups
    /// arm the unknown element schema (the oracle's whole-span
    /// `dot_shorthand_missing_context`); an annotated group
    /// (`List<A> l = [.a]`), a `const` group (the const face's own
    /// family), and a non-declarator `=` (an assignment target — the
    /// target's own face owns the context) answer `None`.
    fn inference_group_literal_ctx(&self, open_idx: usize) -> Option<(Option<Ty>, String)> {
        use dartq_scanner::{Keyword as Kw, TokenKind as TK};
        let toks = self.tokens.as_slice();
        // `var|final name =` directly before the literal.
        if matches!(
            open_idx
                .checked_sub(2)
                .and_then(|i| toks.get(i))
                .map(|t| t.kind),
            Some(TK::Identifier)
        ) && matches!(
            open_idx
                .checked_sub(3)
                .and_then(|i| toks.get(i))
                .map(|t| t.kind),
            Some(TK::Keyword(Kw::Var | Kw::Final))
        ) {
            return Some((None, "_".to_string()));
        }
        // A later declarator: scan left over the previous declarator's
        // initializer (interior groups ride at depth) to the `=` that
        // opens it, its name, and the head beyond.
        let mut idx = open_idx.checked_sub(2)?;
        let mut depth = 0i32;
        while idx > 0 {
            idx -= 1;
            match toks[idx].kind {
                TK::CloseParen | TK::CloseSquare | TK::CloseCurly => depth += 1,
                TK::OpenParen | TK::OpenSquare | TK::OpenCurly => depth -= 1,
                TK::Eq if depth == 0 => {
                    if !matches!(
                        idx.checked_sub(1).and_then(|i| toks.get(i)).map(|t| t.kind),
                        Some(TK::Identifier)
                    ) {
                        return None;
                    }
                    match idx.checked_sub(2).and_then(|i| toks.get(i)).map(|t| t.kind) {
                        Some(TK::Keyword(Kw::Var | Kw::Final)) => {
                            return Some((None, "_".to_string()));
                        }
                        // An intermediate declarator keeps the run going;
                        // a written type's end token (an annotated sibling
                        // annotates the whole group) ends it.
                        Some(TK::Comma) => {}
                        _ => return None,
                    }
                }
                // A statement boundary ends the declarator run — an
                // assignment or a shape this walk does not own.
                TK::Semicolon | TK::OpenCurly | TK::CloseCurly if depth == 0 => return None,
                _ => {}
            }
        }
        None
    }

    /// The declaration-initializer context for a shorthand directly under
    /// `=`/`const`/`new`: the armed group annotation — `Some` for a
    /// written type, the `'_'` verdict for `var`/unresolved, matching the
    /// original declaration-window rule. The LOOKUP context is the
    /// futureOr-base of the folded annotation (`FutureOr<C>` resolves on
    /// `C`); the weak schema display keeps the written `FutureOr<...>`
    /// spelling (`FutureOr<void> v = .id()` reports `'FutureOr<void>'` —
    /// the visiting schema, semantics_A02).
    fn shorthand_decl_ctx(&self) -> Option<(Option<Ty>, String)> {
        if self.pending_decl_ty_unresolved {
            return Some((None, "_".to_string()));
        }
        let Some(raw) = self.pending_decl_ty else {
            return Some((None, "_".to_string()));
        };
        if raw == Ty::UNKNOWN {
            return Some((None, "_".to_string()));
        }
        let folded = self.fold_alias_ty(raw);
        let base = self.future_or_base(folded);
        Some((Some(base), self.ty_display(folded)))
    }

    /// The language-defined `bool` shorthand context: an `if`/`while`/
    /// `assert` condition, an `&&`/`||` operand and a `!` operand resolve
    /// their shorthand against `bool` (the 3.13.7 oracle reports
    /// `The static getter 'x' isn't defined for the context type 'bool'`
    /// there — `bool_context_error_test`, `grammar_A05`). `bool` carries no
    /// static members, so every miss reports at the selector and every hit
    /// (there is none) stays silent — the same shape a real `Ty::Named`
    /// head takes.
    fn bool_ctx(&self) -> Option<(Option<Ty>, String)> {
        Some((
            Some(Ty::Named {
                name: self.intern("bool"),
                nullable: false,
                args: ArgListId::EMPTY,
            }),
            "bool".to_string(),
        ))
    }

    /// The spread-element shorthand context (`... .x`, `...? .x`): a spread
    /// argument resolves against the collection the enclosing literal
    /// builds — `Iterable` for a list (`[`) or set (`{` with one type
    /// argument), `Map` for a map literal (`{` with two type arguments).
    /// The oracle reports the raw head (`'Iterable'`, `'Map'` —
    /// `non_ambiguity_A02_*`), so a shorthand there is an undefined member
    /// unless the head carries that static name. `None` when the enclosing
    /// collection kind is undecidable (an untyped brace literal, a nested
    /// `>>` the scanner did not split) — the under-report.
    fn spread_ctx_at(&self, dot_idx: usize) -> Option<(Option<Ty>, String)> {
        use dartq_scanner::TokenKind as TK;
        let toks = self.tokens.as_slice();
        let open_idx = self.enclosing_group_open(dot_idx)?;
        let head = match toks[open_idx].kind {
            TK::OpenSquare => "Iterable",
            TK::OpenCurly => {
                // The explicit type-argument arity decides set vs map: one
                // argument is a set, two a map. A nested `>>`/`>>>` the
                // scanner left unsplit makes the arity unreadable — bail.
                let closed = open_idx.checked_sub(1)?;
                if toks[closed].kind != TK::Gt {
                    return None;
                }
                let mut depth = 0i32;
                let mut lt = None;
                for i in (0..closed).rev() {
                    match toks[i].kind {
                        TK::Gt => depth += 1,
                        TK::Lt => {
                            if depth == 0 {
                                lt = Some(i);
                                break;
                            }
                            depth -= 1;
                        }
                        TK::GtGt | TK::GtGtGt | TK::GtGtEq | TK::GtGtGtEq => return None,
                        _ => {}
                    }
                }
                let lt = lt?;
                let mut commas = 0usize;
                let mut depth = 0i32;
                for t in &toks[lt + 1..closed] {
                    match t.kind {
                        TK::Lt => depth += 1,
                        TK::Gt => depth -= 1,
                        TK::GtGt | TK::GtGtGt | TK::GtGtEq | TK::GtGtGtEq => return None,
                        TK::Comma if depth == 0 => commas += 1,
                        _ => {}
                    }
                }
                if commas >= 1 { "Map" } else { "Iterable" }
            }
            _ => return None,
        };
        let name = self.intern(head);
        Some((
            Some(Ty::Named {
                name,
                nullable: false,
                args: ArgListId::EMPTY,
            }),
            head.to_string(),
        ))
    }

    /// The scrutinee's matched type for a `case == .a` relational pattern:
    /// the `case` token before the `==` selects the same scrutinee face a
    /// `case .a` constant pattern reads. `None` when the `==` is not a
    /// pattern arm (a plain `==` whose left operand stayed untyped).
    fn scrutinee_ctx_at(&self, op_idx: usize) -> Option<(Option<Ty>, String)> {
        let toks = self.tokens.as_slice();
        let case_idx = op_idx.checked_sub(1)?;
        if toks[case_idx].kind != TokenKind::Keyword(Keyword::Case) {
            return None;
        }
        self.case_scrutinee_ctx(case_idx)
    }

    /// The matched type behind a `case` token: `if (v case .a)` reads `v`'s
    /// static type (the expression directly before `case`); a switch-case
    /// head's `case .a:` reads the enclosing `switch (…)`'s scrutinee —
    /// scanned back over the case group to the `switch` keyword's own
    /// parenthesized expression. `None` when the scrutinee's type cannot
    /// be re-derived (the under-report).
    fn case_scrutinee_ctx(&self, case_idx: usize) -> Option<(Option<Ty>, String)> {
        use dartq_scanner::{Keyword as Kw, TokenKind as TK};
        let toks = self.tokens.as_slice();
        let before_idx = case_idx.checked_sub(1)?;
        // `if (v case .a)` / a nested pattern operand: the expression
        // directly before `case` is the matched value.
        let operand_ty = |engine: &Self, end: u32| -> Option<Ty> {
            engine
                .rows
                .iter()
                .chain(engine.deferred_miss_rows.iter())
                .rev()
                .find(|c| {
                    let span = engine.staged_expression_span(c.range);
                    engine.trivia_between(span.end(), end)
                })
                .map(|c| engine.operand_static_ty(c))
                .filter(|ty| *ty != Ty::UNKNOWN)
        };
        match toks[before_idx].kind {
            // `{`/`:`/`=>`/`&&`/`||`/`(` or `,` — a switch-case head (or a
            // nested pattern inside one): the scrutinee is the enclosing
            // `switch`/`if-case` group's own value. Scan back for the
            // `switch`/`if` keyword whose `(` encloses the value.
            TK::OpenCurly
            | TK::Colon
            | TK::Function
            | TK::AmpersandAmpersand
            | TK::BarBar
            | TK::OpenParen
            | TK::Comma => {
                let mut depth = 0i32;
                for idx in (0..=before_idx).rev() {
                    match toks[idx].kind {
                        TK::CloseCurly | TK::CloseParen | TK::CloseSquare => depth += 1,
                        TK::OpenCurly | TK::OpenParen | TK::OpenSquare => {
                            if depth == 0 {
                                break;
                            }
                            depth -= 1;
                        }
                        TK::Keyword(Kw::Switch | Kw::If) if depth == 0 => {
                            // The `(` after `switch`/`if` holds the
                            // scrutinee expression; its row's static type
                            // is the matched type.
                            let open = toks
                                .get(idx + 1)
                                .filter(|t| t.kind == TK::OpenParen)
                                .map(|t| t.utf16_offset)?;
                            let close = self.tokens.matching_end(idx + 1)?;
                            let end = toks[close].utf16_offset;
                            let ty = self
                                .rows
                                .iter()
                                .chain(self.deferred_miss_rows.iter())
                                .filter(|c| c.range.offset > open && c.range.end() <= end)
                                .last()
                                .map(|c| self.operand_static_ty(c))
                                .filter(|ty| *ty != Ty::UNKNOWN)?;
                            return Some((Some(ty), self.ty_display(ty)));
                        }
                        _ => {}
                    }
                }
                None
            }
            // `if (v case .a)` — the value is the expression right before
            // `case`.
            _ => {
                let ty = operand_ty(self, toks[case_idx].utf16_offset)?;
                Some((Some(ty), self.ty_display(ty)))
            }
        }
    }

    /// The innermost live executable's return context (`return .a`,
    /// `=> .a`, `yield .a`): the declared return latched on the frame, a
    /// closure literal's armed context return. An executable that arms
    /// NONE — an unannotated, still-inferring body (`f() { return .a; }`)
    /// — visits its returns under the unknown schema: the read arm
    /// reports `missing_context` and the invocation arm `'_'`
    /// (type_inference_A03_t01: `return .new()` reports `'_'`); a
    /// DECLARED `dynamic`/`void` return still spells itself
    /// (semantics_A02: `'void'`).
    fn return_ctx(&self) -> Option<(Option<Ty>, String)> {
        let ty = self
            .frames
            .iter()
            .rev()
            .find_map(|frame| match &frame.kind {
                FrameKind::Executable { ret, ctx_ret, .. } => {
                    ret.as_ref().map(|exec| exec.ty).or(*ctx_ret)
                }
                _ => None,
            });
        match ty {
            Some(ty) if ty != Ty::UNKNOWN => {
                let folded = self.fold_alias_ty(ty);
                let base = self.future_or_base(folded);
                Some((Some(base), self.ty_display(folded)))
            }
            _ => Some((None, "_".to_string())),
        }
    }

    /// `(`, `,` context: a call/`new` argument reads the paired parameter's
    /// type; inside `if (`/`while (`/`for (` it is the condition's own
    /// face; a grouping paren inherits the schema its own position arms
    /// (the statement-level `(.a);` arms the unknown schema); a collection
    /// literal's `,`-separated element answers through the element face;
    /// a record element keeps the under-report unless a schema supplies
    /// one.
    fn group_ctx_at(
        &self,
        _row: &Row,
        prev_idx: usize,
        live: bool,
        operand_row_before: impl Fn(&Self, u32, usize) -> Option<Row>,
    ) -> Option<(Option<Ty>, String)> {
        use dartq_scanner::{Keyword as Kw, TokenKind as TK};
        let toks = self.tokens.as_slice();
        let prev = toks[prev_idx];
        // Find the argument group's open `(`: for a `,` the scan walks back
        // past balanced groups counting top-level commas; for `(` the
        // shorthand is the group's first element. A `,` whose enclosing
        // open bracket is a `[`/`{` LITERAL (no `(` ahead of it) is a
        // collection element — the element face answers.
        let open_idx = match prev.kind {
            TK::OpenParen => Some(prev_idx),
            TK::Comma => {
                let mut depth = 0i32;
                let mut found = None;
                let mut literal = None;
                for idx in (0..prev_idx).rev() {
                    match toks[idx].kind {
                        TK::CloseParen | TK::CloseSquare | TK::CloseCurly => depth += 1,
                        TK::OpenParen => {
                            if depth == 0 {
                                found = Some(idx);
                                break;
                            }
                            depth -= 1;
                        }
                        TK::OpenSquare | TK::OpenCurly => {
                            if depth == 0 {
                                literal = Some(idx);
                                break;
                            }
                            depth -= 1;
                        }
                        _ => {}
                    }
                }
                if found.is_none()
                    && let Some(literal_open) = literal
                {
                    return self.literal_elem_ctx(literal_open);
                }
                found
            }
            _ => None,
        };
        let open_idx = open_idx?;
        // `if (`/`while (`/`assert (` — the language-defined `bool`
        // condition context (the 3.13.7 oracle reports `'bool'` for a
        // shorthand there). Only the group's FIRST element inherits it: a
        // comma-separated `assert` message is a `String`, not `bool`.
        // `for (`/`switch (`/`return (` arm no context the walk can
        // re-derive — every one keeps the under-report rather than
        // fabricate a verdict.
        let before_open = open_idx.checked_sub(1);
        if let Some(TK::Keyword(Kw::If | Kw::While | Kw::Assert)) =
            before_open.and_then(|i| toks.get(i)).map(|t| t.kind)
        {
            return if prev.kind == TK::OpenParen {
                self.bool_ctx()
            } else {
                None
            };
        }
        if let Some(TK::Keyword(Kw::For | Kw::Switch | Kw::Return)) =
            before_open.and_then(|i| toks.get(i)).map(|t| t.kind)
        {
            return None;
        }
        // A GROUPING paren (no callee ahead — the callee test is
        // `grouping_open`'s): the parenthesized expression inherits the
        // schema the paren's own position arms (`(.red);` at statement
        // level arms the unknown schema — no_context; `E e = (.a);` the
        // annotation; `x == (.a)` answered earlier, in the equality
        // guard).
        if !matches!(
            before_open.and_then(|i| toks.get(i)).map(|t| t.kind),
            Some(
                TK::Identifier | TK::CloseParen | TK::CloseSquare | TK::Gt | TK::GtGt | TK::GtGtGt
            )
        ) {
            return self.expr_boundary_ctx(open_idx, live);
        }
        // The argument position's parameter type: the callee is the row
        // whose NAME abuts `(` — the staged span widens through the call
        // itself, so the raw range end answers here (`f(.a)`'s `f` ends
        // at `(`; `staged_expression_span` would swallow `f(.a)` whole).
        let callee = self
            .rows
            .iter()
            .chain(self.deferred_miss_rows.iter())
            .rev()
            .find(|c| self.trivia_between(c.range.end(), toks[open_idx].utf16_offset))
            .cloned()
            .or_else(|| operand_row_before(self, toks[open_idx].utf16_offset, self.rows.len()));
        let callee = callee?;
        let formals = self.callee_formals_for(&callee)?;
        // Positional index: top-level commas between `(` and the
        // shorthand's `.`.
        let mut depth = 0i32;
        let mut positional = 0usize;
        let dot_offset = toks[prev_idx + 1].utf16_offset;
        for t in &toks[open_idx + 1..] {
            if t.utf16_offset >= dot_offset {
                break;
            }
            match t.kind {
                TK::OpenParen | TK::OpenSquare | TK::OpenCurly => depth += 1,
                TK::CloseParen | TK::CloseSquare | TK::CloseCurly => depth -= 1,
                TK::Comma if depth == 0 => positional += 1,
                _ => {}
            }
        }
        // A named argument's label (`name: .a`): the identifier right
        // before the shorthand's `.` is the name, the `:` before it is the
        // label mark.
        if prev.kind == TK::Identifier
            && let Some(colon_idx) = prev_idx.checked_sub(1)
            && toks[colon_idx].kind == TK::Colon
        {
            let name = self.intern(&self.source[toks[prev_idx].byte_range()]);
            return formals
                .named_formal(name)
                .map(|f| f.ty)
                .filter(|ty| *ty != Ty::UNKNOWN)
                .and_then(|ty| self.shorthand_ctx_formal(ty));
        }
        return formals
            .positional
            .get(positional)
            .map(|f| f.ty)
            .filter(|ty| *ty != Ty::UNKNOWN)
            .and_then(|ty| self.shorthand_ctx_formal(ty));
    }

    /// The argument-formal context face: a concrete declared type supplies
    /// the shorthand's context (`take(E)`'s `E`); a bare type parameter or
    /// an unsubstituted head (`C<T>`'s `T` — the call-site substitution
    /// `T=C` lives in the invocation's own inference face, not the written
    /// formal) keeps the under-report rather than verdict a `T` the
    /// oracle's instantiated context never sees. The detection is the
    /// head's type answer — a frame type parameter answers `Type` through
    /// the type-param stack, an unsubstituted container parameter (or any
    /// spelling that never resolves a type element) answers `Missing`/
    /// `NonType` and is undecidable here. The context armed is the
    /// formal's INSTANTIATED type — alias spellings fold to their element
    /// before the lookup and the schema display (`contextAlias(C)`'s
    /// `.m()` reports `'_C'`, `contextAlias(C<int>)`'s reports
    /// `'_C<num>'`; private_test), and a `FutureOr<S>` formal resolves
    /// against its base (`take(FutureOr<C>)`'s `.instance` miss spells
    /// `'C'` — semantics_A02's futureOrBase).
    fn shorthand_ctx_formal(&self, ty: Ty) -> Option<(Option<Ty>, String)> {
        if let Ty::Named { name, .. } = ty {
            let is_param =
                self.type_params.contains(&name) || self.pending_type_params.contains(&name);
            if is_param || !matches!(self.type_resolution(name).0, TypeResolution::Type) {
                return None;
            }
        }
        let folded = self.fold_alias_ty(ty);
        let base = self.future_or_base(folded);
        Some((Some(base), self.ty_display(folded)))
    }

    /// `:` context — a named-argument label's value (`f(name: .a)` — the
    /// paired named formal); a conditional-expression branch or a map
    /// entry's value keeps the under-report.
    fn colon_ctx_at(
        &self,
        _row: &Row,
        prev_idx: usize,
        operand_row_before: impl Fn(&Self, u32, usize) -> Option<Row>,
    ) -> Option<(Option<Ty>, String)> {
        use dartq_scanner::TokenKind as TK;
        let toks = self.tokens.as_slice();
        // `name: .a` — an identifier directly before the `:` is a named
        // argument label; the callee's named formal supplies the context.
        let name_idx = prev_idx.checked_sub(1)?;
        if toks[name_idx].kind != TK::Identifier {
            return None;
        }
        let open = self.enclosing_group_open(name_idx)?;
        let callee = self
            .rows
            .iter()
            .chain(self.deferred_miss_rows.iter())
            .rev()
            .find(|c| self.trivia_between(c.range.end(), toks[open].utf16_offset))
            .cloned()
            .or_else(|| operand_row_before(self, toks[open].utf16_offset, self.rows.len()))?;
        let formals = self.callee_formals_for(&callee)?;
        let name = self.intern(&self.source[toks[name_idx].byte_range()]);
        formals
            .named_formal(name)
            .map(|f| f.ty)
            .filter(|ty| *ty != Ty::UNKNOWN)
            .and_then(|ty| self.shorthand_ctx_formal(ty))
    }

    /// `=` context: a shorthand directly under `=` — the declaration
    /// initializer's own (`E e = .a` → the armed annotation via
    /// [`Self::shorthand_decl_ctx`]; `var e = .a` → the `'_'` verdict the
    /// declaration window has always carried). An assignment target's
    /// `x = .a` keeps the under-report — the target's type faces the
    /// shorthand's own read arm elsewhere.
    fn eq_ctx_at(&self, eq_idx: usize) -> Option<(Option<Ty>, String)> {
        use dartq_scanner::{Keyword as Kw, TokenKind as TK};
        let toks = self.tokens.as_slice();
        // The declarator-name arm: `T name =`/`var|final|const|late name =`
        // — an identifier directly before `=` whose own predecessor is the
        // declared type (an identifier — `E e`, a function/record type's
        // closing `)`) or a declaration keyword. Anything else (`x =`,
        // `o.f =`, `this.x =`, `o[i] =`) is an assignment whose target's
        // own face owns the shorthand — the under-report.
        let name_idx = eq_idx.checked_sub(1)?;
        if toks[name_idx].kind != TK::Identifier {
            return None;
        }
        match name_idx
            .checked_sub(1)
            .and_then(|i| toks.get(i))
            .map(|t| t.kind)
        {
            // A generic annotation's closing `>` (`FutureOr<void> v =`,
            // `C<int> c =`) ends the declarator's type the same way an
            // identifier does; `dynamic`/`void`/bare `Function` spell
            // their annotation through a keyword token.
            Some(TK::Identifier | TK::CloseParen | TK::Gt | TK::GtGt | TK::GtGtGt)
            | Some(TK::Keyword(
                Kw::Var
                | Kw::Final
                | Kw::Const
                | Kw::Late
                | Kw::Static
                | Kw::Covariant
                | Kw::Dynamic
                | Kw::Void
                | Kw::Function,
            )) => self.shorthand_decl_ctx(),
            _ => None,
        }
    }

    /// The innermost open group token index enclosing `idx` (`(` or `[`
    /// or `{`, depth-0 backward scan).
    fn enclosing_group_open(&self, idx: usize) -> Option<usize> {
        use dartq_scanner::TokenKind as TK;
        let toks = self.tokens.as_slice();
        let mut depth = 0i32;
        for i in (0..idx).rev() {
            match toks[i].kind {
                TK::CloseParen | TK::CloseSquare | TK::CloseCurly => depth += 1,
                TK::OpenParen | TK::OpenSquare | TK::OpenCurly => {
                    if depth == 0 {
                        return Some(i);
                    }
                    depth -= 1;
                }
                _ => {}
            }
        }
        None
    }

    /// The callee row's formal table — a bare callee resolves the
    /// enclosing container's member (an implicit `this.m` call) then the
    /// top-level declaration; a member row walks its receiver's face.
    fn callee_formals_for(&self, callee: &Row) -> Option<Formals> {
        match &callee.shape {
            RowShape::Member { recv } => self
                .member_formals_full(recv, callee.name)
                .map(|f| f.into_owned()),
            RowShape::Bare(_) => {
                // An unqualified call inside a container resolves the
                // member through `this`.
                let this_recv = self.frames.iter().rev().find_map(|f| match &f.kind {
                    FrameKind::Container { name, kind, .. } => Some(Receiver::This {
                        container: *name,
                        kind: *kind,
                    }),
                    _ => None,
                });
                if let Some(recv) = this_recv
                    && let Some(f) = self.member_formals_full(&recv, callee.name)
                {
                    return Some(f.into_owned());
                }
                self.top_level_decl(callee.name)
                    .and_then(|d| d.formals.as_deref().cloned())
            }
            RowShape::Receiver(recv) => self
                .member_formals_full(recv, callee.name)
                .map(|f| f.into_owned()),
            _ => None,
        }
    }

    /// The deferred row's verdict — the shared rule applied to a row that
    /// already left the live stack (the upgrade writes the deferred slot,
    /// not `self.rows`).
    fn verdict_deferred_shorthand(
        &mut self,
        index: usize,
        row: &Row,
        resolve: Option<Ty>,
        schema: String,
    ) {
        if let Some(ty) = self.verdict_shorthand_miss(row, resolve, schema) {
            self.deferred_miss_rows[index].shape = RowShape::Bare(BareInfo::Plain { ty });
            self.deferred_miss_rows[index].ty = ty;
        }
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
        // the row directly below the `??` group mark. The anchor is the
        // operand's use-site span, never the raw row: a member read's row
        // holds the selector (`a.x`'s `x`) where the oracle spans `a.x`,
        // and a parenthesized operand's row sits on the `)` where the
        // oracle covers the whole group (`(x)` reports `(x)`).
        if let Some(left) = mark
            .checked_sub(1)
            .and_then(|index| self.rows.get(index))
            .filter(|lhs| !matches!(lhs.intent, Intent::Operator(..)))
            && self.operand_static_ty(left) == Ty::Void
        {
            let left = left.clone();
            self.emit_use_of_void_result(Ty::Void, &left);
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
        // The left operand's own row: an operator-marked row is the
        // index/`+` geometry, never the read's own type (`m[k] ?? d`
        // reads `Map.[]`'s `V?` — #2172's `groupSetsBy`/`groupBy` rows).
        // An invoked row reads its send result through the shared
        // verdict, whose bound-aware posture keeps a generic key send
        // (`m[keyOf(e)] ?? d`) from anchoring. An unfolded index
        // (`f()[k] ?? d`, a call-result receiver keeps its operand rows)
        // leaves the KEY as the top row: the left operand is the `[]`
        // read, never the key. And the operand node is the whole
        // expression: a row that stops short of the operator with real
        // tokens between its expression span and the operator is an
        // inner operand of a larger one (`(true ? x : 499)`'s branch, a
        // parenthesized conditional), never the operand the oracle types.
        // The span (not the bare row) answers because an invocation's row
        // is its callee head (`a.b()`, a dot shorthand's `.new(1)`).
        let lhs = mark
            .checked_sub(1)
            .and_then(|index| self.rows.get(index))
            .filter(|lhs| !matches!(lhs.intent, Intent::Operator(..)))
            .filter(|lhs| !self.is_index_key_row(lhs))
            .filter(|lhs| {
                let span = self.staged_expression_span(lhs.range);
                // The operand reaches `??` in two shapes: a bare/member row
                // ABUTS it (only trivia between the span end and `??`), and a
                // folded result row (`a is bool`, `C<int,int>`) CONTAINS the
                // operator — the span walk greedily absorbs the `??` right
                // operand into the operand's extent, so `op` lands strictly
                // inside the span.
                self.trivia_between(span.end(), op_range.offset)
                    || (span.offset < op_range.offset && op_range.offset < span.end())
            });
        // An explicit generic instantiation (`C<int,int>` — a `TypeLiteral`,
        // upstream static type `Type`) folds into an `Intent::Operator`
        // value row the walk above drops by intent; its `>`-close is the
        // type-arguments group, never a binary `>` (the `a > b` comparison
        // arm carries `eq_spans` and already set `lhs_is_bool_comparison`).
        // `Type` is unconditionally non-nullable, so the right operand is
        // dead the moment the operand resolved.
        let instantiation = !lhs_is_bool_comparison
            && mark
                .checked_sub(1)
                .and_then(|index| self.rows.get(index))
                .is_some_and(|row| {
                    // The instantiation's `>`-close op carries its own
                    // extent end (`C<int,int>` ends at `>`); the row's
                    // `staged_expression_span` stops at the folded head, so
                    // the reach check reads the bracket's own range end. The
                    // `>` byte distinguishes the type-arguments group from
                    // an index `[]`/`?[]` face (a nullable `V?` read, never
                    // a `Type` literal) and from arithmetic operators.
                    let (Intent::Operator(_, bracket), RowShape::Value) = (&row.intent, &row.shape)
                    else {
                        return false;
                    };
                    self.byte_at(bracket.offset) == Some(b'>')
                        && !self.is_index_key_row(row)
                        && self.trivia_between(bracket.end(), op_range.offset)
                });
        let non_null = lhs_is_bool_comparison
            || instantiation
            || lhs.is_some_and(|lhs| self.coalesce_operand_never_null(lhs, true));

        // The report range (`node.rightOperand`'s own node range): first
        // token after the operator through the rhs-end latch — a
        // degenerate value row (a collection literal's closing-token face)
        // cannot spell it, and an open latch beats a stale row geometry.
        // Computed before the verdict so the promotion re-decision's
        // staging can carry it (the row stack is gone by teardown).
        let range = match rhs_end {
            Some(end) if end > rhs_offset => SourceRange {
                offset: rhs_offset,
                length: end - rhs_offset,
            },
            // No rhs-end latch (`handle_binary_rhs_end` never saw the `??`
            // arm's last token — a collection literal's `]` close rides the
            // literal fold, not the binary right-operand edge): the value
            // row's expression span spells the operand's own extent. An
            // EMPTY literal (`x ?? []`) leaves its zero-length `[` row on
            // top — the `]` is the next token, and the expression span's
            // close-row re-anchor expands `[..]` to the group's extent.
            _ => self
                .rows
                .last()
                .map(|rhs| self.degenerate_rhs_span(rhs.range, rhs_offset))
                .map(|span| {
                    let offset = span.offset.min(rhs_offset);
                    SourceRange {
                        offset,
                        length: span.end().saturating_sub(offset),
                    }
                })
                .unwrap_or(SourceRange {
                    offset: rhs_offset,
                    length: 0,
                }),
        };

        if !non_null {
            // The walk's declared-type verdict missed — the flow's
            // promoted type may still anchor the dead face
            // (`_checkForDeadNullCoalesce` reads `leftOperand.staticType`,
            // flow-augmented: `x` promoted non-null by `if (x != null)`
            // is dead). The read types only join at teardown, so the
            // re-decision stages the operand's own local-read span
            // against this report range.
            if let Some(lhs_range) = lhs.map(|lhs| lhs.range)
                && self.local_hits.contains_key(&lhs_range.offset)
                && range.length > 0
            {
                self.coalesce_staged.push((lhs_range, range));
            }
            return;
        }
        if range.length > 0 {
            self.stage_usage(SemanticCode::DeadNullAwareExpression, range, "");
        }
    }

    /// The `??` right operand's expression span when the fold's top row is a
    /// degenerate zero-length face. An empty collection literal leaves its
    /// opener row on top — a fused `[]` is ONE [`TokenKind::Index`] token
    /// whose own range already spans both brackets, and an empty `{..}`/`(..)`
    /// group's closer is the next token (its `]`/`}`/`)` re-anchors the
    /// group's extent through [`Self::staged_expression_span`]). A real
    /// (non-empty) operand's row already spells its own span and falls
    /// through unchanged.
    fn degenerate_rhs_span(&self, range: SourceRange, rhs_offset: u32) -> SourceRange {
        use dartq_scanner::TokenKind as TK;
        if range.length != 0 || range.offset != rhs_offset {
            return self.staged_expression_span(range);
        }
        let toks = self.tokens.as_slice();
        let index = toks.partition_point(|t| t.utf16_offset < range.offset);
        // The fused `[]`/`[]=` token: its own extent IS the operand's
        // (`x ?? []` — the `]` never scans as a separate token; the fused
        // spellings are pure ASCII, so `byte_len` is the utf16 length).
        if let Some(tok) = toks.get(index)
            && tok.utf16_offset == range.offset
            && matches!(tok.kind, TK::Index | TK::IndexEq)
        {
            return SourceRange {
                offset: tok.utf16_offset,
                length: tok.byte_len,
            };
        }
        // A real group's closer: the next token re-anchors `[..]`/`{..}`/`(..)`
        // to the operand's whole extent.
        if let Some(close) = toks.get(index + 1)
            && matches!(
                close.kind,
                TK::CloseSquare | TK::CloseCurly | TK::CloseParen
            )
        {
            return self.staged_expression_span(SourceRange {
                offset: close.utf16_offset,
                length: 1,
            });
        }
        self.staged_expression_span(range)
    }

    /// The closed `??`'s static type (`resolveIfNull`, upstream Dart SDK
    /// source, pkg/analyzer/lib/src/dart/resolver/
    /// binary_expression_resolver.dart: `LUB(NonNull(T1), T2)`), staged
    /// for the parenthesized close. Only the plain two-row shape (one left
    /// operand row, one right operand row) is read; the join answers when
    /// the heads are equal or one names a declared supertype of the other
    /// (the LUB is then that supertype); every other pair stays untyped.
    fn stage_if_null_result(&mut self, mark: usize, op_range: SourceRange) {
        // `mark` is the row count at the `??` group's open: the left
        // operand's rows occupy `..mark` (its result is the last row below
        // `mark`), the right operand's rows are `mark..` and its result is
        // the last row on the stack — a compound right side (`x ?? f(y)`,
        // `x ?? a.b`, `x ?? -1`) leaves more than one, the two-row shape
        // only the parenthesized minimal pair.
        if mark == 0 || self.rows.len() <= mark {
            return;
        }
        let lhs = &self.rows[mark - 1];
        let rhs = self.rows.last().expect("rows.len() > mark");
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

    /// The `++`/`--` compound-write twin of
    /// [`Self::cancel_dead_chains_on_write`]: the verifier's dead faces are
    /// read-only call sites (`verifyIndexExpression` fires once, from the
    /// index READ resolution — upstream Dart SDK source,
    /// pkg/analyzer/lib/src/generated/resolver.dart:3404; the
    /// property/method pair likewise runs only where the access resolves
    /// for read), so a null-aware access that IS the compound write's own
    /// target retires its latched chain — `l?[0]++`, `++l?[0]`, `l?[0]--`,
    /// `c?.x++`, `c?.x--` all stay silent upstream.
    ///
    /// `slot` is the written access's own key token: the `[` of a `?[` (one
    /// token past the `?`) or the member selector of a `?.` (two past).
    /// A chain that already climbed past its root keeps its row —
    /// `l?[0][1]++` writes the outer `[1]` while the inner `?[` read still
    /// reports (`_verifyUnassignedSimpleIdentifier`'s parent climb,
    /// dead_code_verifier.dart:491-497) — and a cascade chain keeps its row
    /// on `++` too: the `++` attaches OUTSIDE the cascade section, so the
    /// section still answers `verifyCascadeExpression`'s first-section
    /// face (`l?..x++` reports `?..x`; `l?..[0]` is a parser error anyway).
    pub(crate) fn cancel_dead_chain_on_compound_write(&mut self, slot: u32) {
        self.dead_null_chains
            .retain(|&(op, _, _, kind, _)| match kind {
                crate::diagnose::types::DeadChainKind::Index => op + 1 != slot,
                crate::diagnose::types::DeadChainKind::Member => op + 2 != slot,
                crate::diagnose::types::DeadChainKind::CascadeMember
                | crate::diagnose::types::DeadChainKind::CascadeIndex => true,
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
        // The named-target walk covers a bare/member/keyword receiver
        // (`x ??=`, `c.f ??=`, `this.x ??=`); an index target (`a[i] ??=`,
        // `x?[i] ??=`) folds into a `RowShape::Value` stand-in at the `]`
        // whose offset rides `index_result_recvs` — the official
        // `setReadElement`'s `IndexExpression` arm answers `operator[]`'s
        // return type, so the stand-in IS the assignment target upstream
        // writes.
        let target = self
            .rows
            .iter()
            .rev()
            .find(|row| self.is_assignment_target(row, limit))
            .or_else(|| self.index_assignment_target(limit));
        let Some(target) = target else {
            return;
        };
        let target_range = target.range;
        // The index-result stand-in's `?[` nullability is the access's own
        // widening — the `??=` read face upstream strips it (the desugared
        // `t[i] ??= v` reads `t[i]`'s `[]` return on the non-null temp), so
        // the verdict asks the `[]` return's own nullability.
        let never_null = if self
            .index_result_recvs
            .get(&target_range.offset)
            .is_some_and(|(_, was_question)| *was_question)
        {
            let ty = Engine::non_null_form(self.operand_static_ty(target));
            !self.written_ty_invalid(ty) && self.strictly_non_nullable(ty)
        } else {
            self.coalesce_operand_never_null(target, false)
        };
        // The report anchors at `node.value`'s own expression span, never an
        // inner operand row (`bad()`'s head, `-42`'s digits, `[]`'s closing
        // bracket): from the operand's first token through the value row's
        // own expression end. An invalid value (`x ??= ...`) stages no row
        // at all — the official report then anchors the SYNTHETIC empty
        // node at the value's position, a zero-length span the row model
        // spells as the operator's first-token offset.
        let range = match self
            .rows
            .last()
            .filter(|row| row.range.offset >= rhs_offset)
        {
            Some(value) => {
                let span = self.staged_expression_span(value.range);
                let offset = span.offset.min(rhs_offset);
                SourceRange {
                    offset,
                    length: span.end().saturating_sub(offset),
                }
            }
            None => SourceRange {
                offset: rhs_offset,
                length: 0,
            },
        };
        if !never_null {
            // The declared-type verdict missed — the `??=` write's own
            // promotion may anchor the dead face (`x ??= 1` re-promotes a
            // nullable `x` to non-null, so a following `x ??= 2` reports;
            // `visitIfNullAssignment` reads the target's flow-aware
            // `readResolution.type`). The promotion joins only at
            // teardown, so the re-decision stages the target's own
            // local-read span against this report range.
            if self.local_hits.contains_key(&target_range.offset) && range.length > 0 {
                self.coalesce_staged.push((target_range, range));
            }
            return;
        }
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
        if row.range.end() > limit || matches!(row.intent, Intent::Operator(..) | Intent::Invoke) {
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

    /// The index-expression assignment target the named-row walk cannot
    /// reach (`a[i] ??= v`, `x?[i] ??= v`): the fold leaves a
    /// `RowShape::Value` stand-in at the `]`, its offset latched in
    /// [`Engine::index_result_recvs`] — the official
    /// `visitIfNullAssignment` resolves the `IndexExpression` node through
    /// `setReadElement`'s `[]` arm, so the stand-in is the row that
    /// answers the target's read face.
    fn index_assignment_target(&self, limit: u32) -> Option<&Row> {
        self.rows.iter().rev().find(|row| {
            matches!(row.shape, RowShape::Value)
                && self.index_result_recvs.contains_key(&row.range.offset)
                && row.range.end() <= limit
                && self.trivia_between(row.range.end(), limit)
        })
    }

    /// Whether the source at a UTF-16 offset starts a Dart identifier.
    fn starts_a_name(&self, offset: u32) -> bool {
        self.byte_at(offset)
            .is_some_and(|byte| byte.is_ascii_alphabetic() || byte == b'_' || byte == b'$')
    }

    /// Whether the primary constructor's formal `name` must not answer from
    /// the current position. The formals are scoped by the constructor
    /// alone upstream — `ConstructorInitializerScope` over the non-late
    /// INSTANCE field initializers and `PrimaryParameterScope` over the
    /// `this : ...` body
    /// (pkg/analyzer/lib/src/dart/resolver/scope_context.dart:406-461) —
    /// while the container frame carries them as locals for the body's
    /// sake. Every other member plane is outside both scopes and reads
    /// `undefined_identifier` (oracle 3.13.7: a method body
    /// `void m() { print(foo); }`, a getter `int get g => foo;`, a
    /// `static` or `late` field initializer, and a secondary constructor's
    /// initializer list all report `Undefined name 'foo'.`), which is what
    /// the default-on [`Engine::primary_formals_muted`] window encodes.
    pub(crate) fn primary_formal_hidden(&self, name: StringId) -> bool {
        self.primary_formals_muted && self.primary_formals.contains(&name)
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
        if self.primary_formal_hidden(name) {
            return None;
        }
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
                let visible = |d: &LocalDecl| {
                    d.name == name
                        && !d.retired
                        && match d.group {
                            Some(g) if g >= PATTERN_ARM_SCOPE_BASE => {
                                let slot = (u32::MAX - g) as usize;
                                switches.len() >= slot
                                    && switches
                                        .get(slot - 1)
                                        .is_some_and(|ctx| ctx.switch_kw.offset < d.range.offset)
                            }
                            _ => true,
                        }
                };
                // Upstream's `LocalScope` binds a redeclared name to the
                // FIRST declaration the scope saw (`LocalScope.add`'s
                // `??=` — the same first-wins posture
                // [`Engine::dup_scope_add`] records). The flat `locals`
                // window can hold the redeclaring sibling beside it
                // (`var i; var i;` sit in one frame), so the innermost
                // open statement-list scope's stored range names the
                // bound row; a name no open scope holds (a shadowing
                // declaration the frame walk already narrowed to, or the
                // for-head's own `dup_for_parts` map) keeps the window's
                // last entry — the historical last-wins read. A visible
                // case binder (an if-case or switch-case head's pattern
                // variable: no group, an odd logical-or site group, or the
                // arm-scope sentinel band — a pattern-declaration
                // statement's binders carry an even group and join the
                // statement-list scope) never registers in a duplicate
                // scope: it lives in the case's own nested scope, retires
                // when that scope closes, and is newer than every
                // statement-list binding it could meet, so it shadows them
                // (`var e = switch (r) { Box(:var e) => e, … }` and
                // `if (parent case PA parent) { parent… }` read the binder).
                let newest = self.locals[lo..hi].iter().rposition(|d| visible(d));
                if let Some(p) = newest
                    && matches!(self.locals[lo + p].kind, LocalKind::PatternVariable)
                    && self.locals[lo + p]
                        .group
                        .is_none_or(|g| g % 2 == 1 || g >= PATTERN_ARM_SCOPE_BASE)
                {
                    return Some((lo + p) as u32);
                }
                if let Some(bound) = self
                    .dup_scopes
                    .iter()
                    .rev()
                    .find_map(|scope| scope.get(&name))
                    && let Some(p) = self.locals[lo..hi]
                        .iter()
                        .position(|d| d.range == *bound && visible(d))
                {
                    return Some((lo + p) as u32);
                }
                return newest.map(|p| (lo + p) as u32);
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
        self.bind_inferred_ty(name, ty, VarInferSlot::Local);
    }

    /// Binds a `var`/`final`-inferred declarator's resolved initializer
    /// type into the slot its declaration owns: a local's frame slot, or —
    /// for a top-level variable or a field — the overlay the model read
    /// surfaces answer through, since `TopDecl::var_ty` and
    /// `MemberEntry::var_ty` carry `Unknown` for every unwritten annotation
    /// (the collector runs before any inference).
    pub(crate) fn bind_inferred_ty(&mut self, name: StringId, ty: Ty, slot: VarInferSlot) {
        let ty = self.instantiated_head_ty(ty);
        match slot {
            VarInferSlot::Local => {
                if !self.declares_nothing(name)
                    && let Some(frame) = self.frames.last_mut()
                {
                    frame.var_types.insert(name, ty);
                }
            }
            VarInferSlot::TopLevel => {
                self.top_var_inferred_tys.insert(name, ty);
            }
            VarInferSlot::Field(container) => {
                self.field_inferred_tys.insert((container, name), ty);
            }
        }
    }

    /// Pairs the initializer starting at `at` (UTF-16) with its declarator
    /// in the open field/top-level variable group — the LAST name the group
    /// folded ahead of it, since a group keeps its names in one block
    /// (`var a, b = 2;`: `b` owns the initializer) while the initializers
    /// follow one per initialized declarator. The skipped names are
    /// initializerless declarators: they carry no inferred type and drop.
    pub(crate) fn take_model_declarator(
        &mut self,
        at: u32,
    ) -> Option<(StringId, SourceRange, VarInferSlot)> {
        let index = self
            .pending_model_declarators
            .iter()
            .rposition(|(_, range, _)| range.end() <= at)?;
        self.pending_model_declarators.drain(..=index).last()
    }

    /// The innermost frame's declared slot for a local — the typedness ask
    /// the for-in element staging makes before overwriting an inferred
    /// (`Unknown`) slot.
    pub(crate) fn local_declared_ty(&self, name: StringId) -> Option<Ty> {
        self.frames
            .last()
            .and_then(|frame| frame.var_types.get(&name).copied())
    }

    /// `patternTypeMismatchInIrrefutableContext` over one
    /// `PatternVariableDeclarationStatement` (`var (int a) = 's';`): every
    /// node of the irrefutable pattern asks `matched <: required` —
    /// the initializer's folded static type descends through record
    /// slots / list elements / map values / object getter columns / the
    /// cast target — and a provable non-answer reports the node's own
    /// span (upstream `dispatchPattern` →
    /// `patternTypeMismatchInIrrefutableContext` at `.at(pattern)`,
    /// shared_type_analyzer.dart:176-190). The oracle-verified required
    /// faces: a typed leaf `T x` requires `T`; a record node requires
    /// the `Object?`-slot record of its field shape; a list requires
    /// `List<written ?? matched-element ?? Object?>`; a map requires
    /// `Map<written K, V ?? matched entries ?? Object?, Object?>`; an
    /// object node requires its written head. Parenthesized nodes are
    /// transparent; refutable nodes (constants, relationals, `||`,
    /// `&&`, `?`) never check here — upstream marks them refutable and
    /// their whole subtree skips the face.
    ///
    /// The walk is token-level: the statement's pattern window is the
    /// tokens between the declaration keyword (`var`/`final`, found by a
    /// depth-0 backward scan from `=`) and the `=` itself — `init_start`
    /// (the `=`'s end) and `semi` only frame the initializer's row
    /// window, which the same single-row rule as the destructure bind
    /// reads for the matched type.
    fn check_irrefutable_destructure(&mut self, init_start: u32, semi: u32) {
        let toks = self.tokens.as_slice();
        let eq = match self.token_at_offset(init_start).checked_sub(1) {
            Some(eq) if toks.get(eq).is_some_and(|t| t.kind == TokenKind::Eq) => eq,
            _ => return,
        };
        // The declaration keyword: the nearest `var`/`final` at bracket
        // depth 0 before `=` — a `;`, `{`, or `}` closes the statement
        // first (recovery keeps the whole ask silent).
        let mut depth = 0i32;
        let mut kw = None;
        let mut i = eq;
        while i > 0 {
            i -= 1;
            match toks[i].kind {
                TokenKind::CloseParen | TokenKind::CloseSquare | TokenKind::CloseCurly => {
                    depth += 1;
                }
                TokenKind::OpenParen | TokenKind::OpenSquare | TokenKind::OpenCurly => {
                    if depth == 0 {
                        break;
                    }
                    depth -= 1;
                }
                TokenKind::Keyword(Keyword::Var | Keyword::Final) if depth == 0 => {
                    kw = Some(i);
                    break;
                }
                TokenKind::Semicolon if depth == 0 => break,
                _ => {}
            }
        }
        let Some(kw) = kw else { return };
        // The matched type: the initializer's one non-TypeUse row inside
        // `[init_start, semi)` — [`Self::destructure_record`]'s window.
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
        let Some(matched) = self.initializer_static_ty(&row) else {
            return;
        };
        if matched == Ty::UNKNOWN || self.is_invalid_ty(matched) {
            return;
        }
        let (Some(lo), Some(hi)) = (self.token_source_range(kw + 1), self.token_source_range(eq))
        else {
            return;
        };
        self.irrefutable_walk_between(lo, hi, matched);
    }

    /// One token's UTF-16 span by index (`None` past the table).
    fn token_source_range(&self, index: usize) -> Option<SourceRange> {
        self.tokens.as_slice().get(index).map(|t| SourceRange {
            offset: t.utf16_offset,
            length: t.byte_len,
        })
    }

    /// The `[lo, hi)` token window's UTF-16 span (the node report's
    /// `.at(pattern)` anchor: offset = first token's, end = last's).
    fn token_span(&self, lo: usize, hi: usize) -> Option<SourceRange> {
        let toks = self.tokens.as_slice();
        let first = toks.get(lo)?;
        let last = toks.get(hi.checked_sub(1)?)?;
        Some(SourceRange {
            offset: first.utf16_offset,
            length: (last.utf16_offset + last.byte_len).saturating_sub(first.utf16_offset),
        })
    }

    /// One irrefutable-pattern node over the token window `[lo, hi)`
    /// carrying `matched` as its matched value type: the
    /// `pattern_type_mismatch_in_irrefutable_context` emitter behind
    /// [`Self::irrefutable_walk_between`]. Structural recursion —
    /// container children recurse with their column's matched type;
    /// everything else stays silent.
    ///
    /// `assign` marks a pattern-assignment context: bare identifiers are
    /// then assigned-variable patterns whose required type is the
    /// variable's declared one (`analyzeAssignedVariablePattern`), never
    /// fresh binders that infer from the matched type.
    fn irrefutable_node(&mut self, lo: usize, hi: usize, matched: Ty, assign: bool) {
        if lo >= hi {
            return;
        }
        let toks = self.tokens.as_slice();
        // A cast tail `p as T` at depth 0 keeps the node check silent and
        // re-dispatches the inner pattern with `T` as its matched type
        // (`analyzeCastPattern`: no irrefutable check on the node; the
        // operand sees `requiredType` — `var (int h as num) = 's'`
        // reports the leaf's `'num'` vs `'int'`). The container column
        // rides through unchanged, so a `List r as List<num>` rest keeps
        // the rest's own `List<E>` matched face.
        {
            let mut depth = 0i32;
            let mut cast = None;
            for i in lo..hi {
                match toks[i].kind {
                    TokenKind::OpenParen | TokenKind::OpenSquare | TokenKind::OpenCurly => {
                        depth += 1;
                    }
                    TokenKind::CloseParen | TokenKind::CloseSquare | TokenKind::CloseCurly => {
                        depth -= 1
                    }
                    TokenKind::Keyword(Keyword::As) if depth == 0 => cast = Some(i),
                    _ => {}
                }
            }
            if let Some(as_at) = cast {
                let mut carry = 0u32;
                let Some((cast_ty, _)) = self.reconstruct_ty_at(as_at + 1, &mut carry) else {
                    return;
                };
                self.irrefutable_node(lo, as_at, cast_ty, assign);
                return;
            }
        }
        // The `?` null-check postfix is refutable — the node reports and
        // its whole subtree leaves this face (`analyzeNullCheckOrAssertPattern`
        // reports first, then `makeRefutable()`); the `!` null-assert
        // postfix re-dispatches transparently.
        let mut hi = hi;
        loop {
            match toks.get(hi - 1).map(|t| t.kind) {
                Some(TokenKind::Bang) => hi -= 1,
                Some(TokenKind::Question) => {
                    self.emit_refutable_pattern(lo, hi);
                    return;
                }
                _ => break,
            }
        }
        if lo >= hi {
            return;
        }
        // The refutable shapes, per node, BEFORE the kind faces
        // (`dispatchPattern`'s per-node gate): `||` reports at the node
        // and silences its subtree (`analyzeLogicalOrPattern`), a
        // depth-0 relational/equality head likewise
        // (`analyzeRelationalPattern`). Constant patterns are the
        // parser's row (patterns.rs `report_refutable_constant_pattern`)
        // and fall through to the silent arms below. A leading `<` that
        // opens a typed collection's argument group is NOT a relational
        // head — `<T>[...]`/`<K,V>{...}` spell the list and map nodes.
        {
            let mut depth = 0i32;
            let mut i = lo;
            let mut logical_or = false;
            let mut logical_and = None;
            while i < hi {
                match toks[i].kind {
                    TokenKind::OpenParen | TokenKind::OpenSquare | TokenKind::OpenCurly => {
                        depth += 1;
                    }
                    TokenKind::CloseParen | TokenKind::CloseSquare | TokenKind::CloseCurly => {
                        depth -= 1
                    }
                    TokenKind::Lt if depth == 0 => {
                        // Typed-collection head (`<T>[...]`/`<K,V>{...}`)
                        // or a typed object head (`Square<C>(…)`)? Scan
                        // forward for the group's `>` and the group opener
                        // behind it; anything else is the relational
                        // operator `<`.
                        let mut inner = 0i32;
                        let mut j = i + 1;
                        let mut is_container = false;
                        while j < hi {
                            match toks[j].kind {
                                TokenKind::Lt | TokenKind::LtLt => inner += 1,
                                TokenKind::Gt | TokenKind::GtGt => {
                                    if inner == 0 {
                                        is_container = matches!(
                                            toks.get(j + 1).map(|t| t.kind),
                                            Some(
                                                TokenKind::OpenSquare
                                                    | TokenKind::OpenCurly
                                                    | TokenKind::OpenParen
                                            )
                                        );
                                        break;
                                    }
                                    inner -= 1;
                                }
                                _ => {}
                            }
                            j += 1;
                        }
                        if is_container {
                            i = j;
                        } else {
                            self.emit_refutable_pattern(lo, hi);
                            return;
                        }
                    }
                    TokenKind::BarBar if depth == 0 => logical_or = true,
                    TokenKind::AmpersandAmpersand if depth == 0 => logical_and = Some(i),
                    TokenKind::EqEq
                    | TokenKind::BangEq
                    | TokenKind::Gt
                    | TokenKind::GtEq
                    | TokenKind::LtEq
                        if depth == 0 =>
                    {
                        self.emit_refutable_pattern(lo, hi);
                        return;
                    }
                    _ => {}
                }
                i += 1;
            }
            if logical_or {
                self.emit_refutable_pattern(lo, hi);
                return;
            }
            // `&&` forwards the context verbatim to BOTH operands
            // (`analyzeLogicalAndPattern`), each seeing the same matched
            // value type — the left-associative split takes the LAST
            // depth-0 operator.
            if let Some(op) = logical_and {
                self.irrefutable_node(lo, op, matched, assign);
                self.irrefutable_node(op + 1, hi, matched, assign);
                return;
            }
        }
        match toks[lo].kind {
            TokenKind::OpenParen => {
                let Some(close) = self.tokens.matching_end(lo) else {
                    return;
                };
                if close + 1 != hi {
                    return;
                }
                // Record vs parenthesized: a depth-0 comma or named-field
                // colon inside makes the record node; a lone inner node
                // is transparent (`(int a)` reports at `int a`'s span).
                let mut depth = 0i32;
                let mut is_record = false;
                for i in (lo + 1)..close {
                    match toks[i].kind {
                        TokenKind::OpenParen | TokenKind::OpenSquare | TokenKind::OpenCurly => {
                            depth += 1;
                        }
                        TokenKind::CloseParen | TokenKind::CloseSquare | TokenKind::CloseCurly => {
                            depth -= 1
                        }
                        TokenKind::Comma | TokenKind::Colon if depth == 0 => is_record = true,
                        _ => {}
                    }
                }
                if !is_record {
                    self.irrefutable_node(lo + 1, close, matched, assign);
                    return;
                }
                let fields = self.pattern_fields(lo + 1, close);
                let object_q = Self::nullable_form(Ty::named(self.intern("Object")));
                // The required record face (`analyzeRecordPattern`'s
                // `requiredTypeNamedTypes`): every positional field adds
                // one `Object?` slot; every named field adds one too, but
                // a REPEATED name is a `duplicate_record_pattern_field`
                // row upstream and contributes no second slot.
                let mut positional = Vec::new();
                let mut named: Vec<(StringId, Ty)> = Vec::new();
                for (name, _, _, _) in &fields {
                    match name {
                        Some(name) => {
                            if !named.iter().any(|(seen, _)| seen == name) {
                                named.push((*name, object_q));
                            }
                        }
                        None => positional.push(object_q),
                    }
                }
                let required = self
                    .boundary
                    .args
                    .borrow_mut()
                    .record(&positional, &named, false);
                if let Some(at) = self.token_span(lo, hi) {
                    self.emit_irrefutable(at, matched, required);
                }
                // Children read their slot's type only when the matched
                // record carries the same shape — the official
                // `_matchRecordTypeShape` gate: every named field must
                // exist on the matched record, the positional fields must
                // pair one-for-one, and the matched record must carry no
                // field the pattern leaves out. Every other matched face
                // hands them `Object?` (`dispatchFields(objectQuestionType)`)
                // — except `dynamic`, which dispatches `dynamic`.
                let slots: Vec<Ty> = match matched {
                    Ty::Record { shape, .. } => {
                        let pool = self.boundary.pool();
                        let shape = pool.record_shape(shape);
                        let mut slots = Vec::with_capacity(fields.len());
                        let mut positional_cursor = 0usize;
                        let mut named_seen = 0usize;
                        let mut same_shape = true;
                        for (name, _, _, _) in &fields {
                            match name {
                                Some(name) => {
                                    named_seen += 1;
                                    match shape.named.iter().find(|(n, _)| n == name) {
                                        Some((_, ty)) => slots.push(*ty),
                                        None => {
                                            same_shape = false;
                                            break;
                                        }
                                    }
                                }
                                None => match shape.positional.get(positional_cursor) {
                                    Some(ty) => {
                                        slots.push(*ty);
                                        positional_cursor += 1;
                                    }
                                    None => {
                                        same_shape = false;
                                        break;
                                    }
                                },
                            }
                        }
                        if same_shape
                            && positional_cursor == shape.positional.len()
                            && named_seen == named.len()
                        {
                            slots
                        } else {
                            vec![object_q; fields.len()]
                        }
                    }
                    Ty::Dynamic => vec![Ty::Dynamic; fields.len()],
                    _ => vec![object_q; fields.len()],
                };
                for ((_, fld_lo, fld_hi, _), slot) in fields.iter().zip(slots) {
                    self.irrefutable_node(*fld_lo, *fld_hi, slot, assign);
                }
            }
            TokenKind::OpenSquare | TokenKind::Lt => {
                // List `[...]` / `<T>[...]` or map `{...}` / `<K,V>{...}`.
                let (mut cursor, written) = if toks[lo].kind == TokenKind::Lt {
                    let mut carry = 0u32;
                    let Some((first, mut next)) = self.reconstruct_ty_at(lo + 1, &mut carry) else {
                        return;
                    };
                    let mut args = vec![first];
                    while toks.get(next).is_some_and(|t| t.kind == TokenKind::Comma) {
                        let Some((arg, after)) = self.reconstruct_ty_at(next + 1, &mut carry)
                        else {
                            return;
                        };
                        args.push(arg);
                        next = after;
                    }
                    match toks.get(next).map(|t| t.kind) {
                        Some(TokenKind::Gt) => (next + 1, Some(args)),
                        Some(TokenKind::GtEq) => (next + 1, Some(args)),
                        _ => return,
                    }
                } else {
                    (lo, None)
                };
                let (open, close_kind) = match toks.get(cursor).map(|t| t.kind) {
                    Some(TokenKind::OpenSquare) => (cursor, TokenKind::CloseSquare),
                    Some(TokenKind::OpenCurly) => (cursor, TokenKind::CloseCurly),
                    _ => return,
                };
                cursor += 1;
                let Some(close) = self.tokens.matching_end(open) else {
                    return;
                };
                if toks.get(close).map(|t| t.kind) != Some(close_kind) || close + 1 != hi {
                    return;
                }
                let fields = self.pattern_fields(cursor, close);
                match close_kind {
                    TokenKind::CloseSquare => {
                        // `List<elementType>` — the written arg, else the
                        // matched list's element, else `dynamic` for a
                        // `dynamic` matched face and `Object?` otherwise
                        // (`analyzeListPattern`, type_analyzer.dart:979-1009).
                        let object_q = Self::nullable_form(Ty::named(self.intern("Object")));
                        let element = written
                            .as_ref()
                            .and_then(|args| args.first().copied())
                            .unwrap_or_else(|| {
                                if matched == Ty::Dynamic {
                                    Ty::Dynamic
                                } else {
                                    self.list_element_of(matched).unwrap_or(object_q)
                                }
                            });
                        let required = {
                            let list = self.intern("List");
                            self.boundary
                                .args
                                .borrow_mut()
                                .named(list, false, &[element])
                        };
                        if let Some(at) = self.token_span(lo, hi) {
                            self.emit_irrefutable(at, matched, required);
                        }
                        // A rest element's sub-pattern sees the LIST, not
                        // the element (`analyzeListPattern`:
                        // `subPatternMatchedType = requiredType`).
                        for (_, fld_lo, fld_hi, rest) in &fields {
                            let column = if *rest { required } else { element };
                            self.irrefutable_node(*fld_lo, *fld_hi, column, assign);
                        }
                    }
                    _ => {
                        // `Map<keyType, valueType>` — written args, else
                        // the matched map's entries, else `Object?` both
                        // (`analyzeMapPattern`). Only the VALUE patterns
                        // recurse (a key is a constant face).
                        let object_q = Self::nullable_form(Ty::named(self.intern("Object")));
                        let (key_ty, val_ty) = match written.as_ref() {
                            Some(args) if args.len() >= 2 => (args[0], args[1]),
                            _ => self.map_entries_of(matched).unwrap_or((object_q, object_q)),
                        };
                        let required = {
                            let map = self.intern("Map");
                            self.boundary
                                .args
                                .borrow_mut()
                                .named(map, false, &[key_ty, val_ty])
                        };
                        if let Some(at) = self.token_span(lo, hi) {
                            self.emit_irrefutable(at, matched, required);
                        }
                        for (_, fld_lo, fld_hi, rest) in &fields {
                            // `fld_lo` here is the value sub-pattern —
                            // [`Self::pattern_fields`] already split the
                            // `key: value` pair for `{}`-led groups. A
                            // map rest is an error face
                            // (`rest_element_in_map_pattern`) whose
                            // sub-pattern sees `dynamic` upstream — no
                            // mismatch row can land there.
                            if *rest {
                                continue;
                            }
                            self.irrefutable_node(*fld_lo, *fld_hi, val_ty, assign);
                        }
                    }
                }
            }
            TokenKind::OpenCurly => {
                let Some(close) = self.tokens.matching_end(lo) else {
                    return;
                };
                if close + 1 != hi {
                    return;
                }
                let fields = self.pattern_fields(lo + 1, close);
                let object_q = Self::nullable_form(Ty::named(self.intern("Object")));
                let (key_ty, val_ty) = self.map_entries_of(matched).unwrap_or((object_q, object_q));
                let required = {
                    let map = self.intern("Map");
                    self.boundary
                        .args
                        .borrow_mut()
                        .named(map, false, &[key_ty, val_ty])
                };
                if let Some(at) = self.token_span(lo, hi) {
                    self.emit_irrefutable(at, matched, required);
                }
                for (_, fld_lo, fld_hi, rest) in &fields {
                    if *rest {
                        continue;
                    }
                    self.irrefutable_node(*fld_lo, *fld_hi, val_ty, assign);
                }
            }
            TokenKind::Identifier | TokenKind::Keyword(_) => {
                // A bare identifier in an assignment context is an
                // assigned-variable pattern: its required type is the
                // variable's own declared type, and no object head can
                // follow (`analyzeAssignedVariablePattern`).
                if assign && hi == lo + 1 && toks[lo].kind == TokenKind::Identifier {
                    let name = self.intern(&self.source[toks[lo].byte_range()]);
                    let Some(required) = self.typed_local(name) else {
                        return;
                    };
                    if let Some(at) = self.token_span(lo, hi) {
                        self.emit_irrefutable(at, matched, required);
                    }
                    return;
                }
                // A leading `var`/`final` keyword is the variable pattern's
                // own modifier, never a type head: strip it and re-dispatch
                // the remainder (`final int _` is a declared-variable
                // pattern whose required type is the written `int`; a bare
                // `var x` infers from the matched value and stays silent).
                // A leading `const` opens a constant pattern — the parser
                // owns that refutable row, so the subtree stays silent.
                let mut lo = lo;
                while lo < hi
                    && matches!(
                        toks[lo].kind,
                        TokenKind::Keyword(Keyword::Var | Keyword::Final | Keyword::Const)
                    )
                {
                    if toks[lo].kind == TokenKind::Keyword(Keyword::Const) {
                        return;
                    }
                    lo += 1;
                }
                if lo >= hi {
                    return;
                }
                // Typed leaf `T x` / `T _` vs object head `Name(...)`.
                let mut carry = 0u32;
                let Some((required, next)) = self.reconstruct_ty_at(lo, &mut carry) else {
                    return;
                };
                let toks = self.tokens.as_slice();
                match toks.get(next).map(|t| t.kind) {
                    Some(TokenKind::OpenParen) => {
                        let Some(close) = self.tokens.matching_end(next) else {
                            return;
                        };
                        if close + 1 != hi {
                            return;
                        }
                        if let Some(at) = self.token_span(lo, hi) {
                            self.emit_irrefutable(at, matched, required);
                        }
                        // Getter-column dispatch: `Name(f: p)` reads
                        // `f`'s column through the written head
                        // (`resolveObjectPatternPropertyGet` —
                        // [`Self::object_pattern_binder_ty`]). An
                        // UNRESOLVED getter hands the child `dynamic`
                        // upstream (`resolveObjectPatternPropertyGet`
                        // answers `typeProvider.dynamicType` when the
                        // getter is missing), never `Object?` — the
                        // column is undecidable here, so the under-report
                        // direction applies.
                        let head = match required {
                            Ty::Named { name, .. } => Some(name),
                            _ => None,
                        };
                        for (getter, fld_lo, fld_hi, _) in self.pattern_fields(next + 1, close) {
                            let column = head
                                .and_then(|head| {
                                    getter.and_then(|g| self.object_pattern_binder_ty(head, g))
                                })
                                .unwrap_or(Ty::Dynamic);
                            self.irrefutable_node(fld_lo, fld_hi, column, assign);
                        }
                    }
                    Some(TokenKind::Identifier | TokenKind::Keyword(_)) => {
                        // The declared-variable leaf: the node spans the
                        // written type through the variable name.
                        if next + 1 != hi {
                            return;
                        }
                        let Some(at) = self.token_span(lo, hi) else {
                            return;
                        };
                        self.emit_irrefutable(at, matched, required);
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }

    /// The shared irrefutable walk over a token window: `lo` is the
    /// pattern's first token (inclusive; a leading `var`/`final` keyword
    /// is stripped), `hi` the first token AFTER the window, `matched` the
    /// matched value type. This is the entry point every irrefutable
    /// context funnels through — the pattern variable declaration's
    /// destructure window ([`Self::destructure_record`]), a pattern
    /// assignment's left side ([`Self::irrefutable_walk_pattern_assignment`])
    /// and a for-in head ([`Self::irrefutable_walk_for_in_head`]).
    pub(crate) fn irrefutable_walk_between(
        &mut self,
        lo: SourceRange,
        hi: SourceRange,
        matched: Ty,
    ) {
        if matched == Ty::UNKNOWN || self.is_invalid_ty(matched) {
            return;
        }
        let lo = self.token_at_offset(lo.offset);
        let hi = self.token_at_offset(hi.offset);
        self.irrefutable_node(lo, hi, matched, false);
    }

    /// A pattern assignment's left side (`(a, b) = expr`, `Square(:f) =
    /// expr`): the parser fires the event after the RHS parsed, so the
    /// matched type is the RHS expression's folded static type and the
    /// pattern window is the token run between the statement boundary and
    /// the `=`. Assignment contexts bind no fresh variables — a bare
    /// identifier is an assigned-variable pattern whose required type is
    /// the variable's declared one.
    pub(crate) fn irrefutable_walk_pattern_assignment(&mut self, equals: Token) {
        let Some(eq) = self.eq_index(equals) else {
            return;
        };
        let Some(lo) = self.pattern_start_before(eq) else {
            return;
        };
        let Some(semi) = self.statement_end_after(eq) else {
            return;
        };
        let Some(matched) = self.assignment_rhs_ty(equals, semi) else {
            return;
        };
        if matched == Ty::UNKNOWN || self.is_invalid_ty(matched) {
            return;
        }
        self.irrefutable_node(lo, eq, matched, true);
    }

    /// A for-in pattern head (`for (var (int v) in xs)`): the head's
    /// pattern window runs from the declaration keyword to the `in`, and
    /// the matched type is the iterable's element type the caller already
    /// computed (`for_in_element_staging`).
    pub(crate) fn irrefutable_walk_for_in_head(
        &mut self,
        keyword: Token,
        in_kw: Token,
        matched: Ty,
    ) {
        if matched == Ty::UNKNOWN || self.is_invalid_ty(matched) {
            return;
        }
        let lo = self.token_at_offset(keyword.range.offset) + 1;
        let hi = self.token_at_offset(in_kw.range.offset);
        self.irrefutable_node(lo, hi, matched, false);
    }

    /// The index of the `=` token `equals` names, `None` when the token
    /// table disagrees (a synthetic recovery token).
    #[allow(dead_code)] // rider helper: the listener.rs call sites are a later lane
    fn eq_index(&self, equals: Token) -> Option<usize> {
        let toks = self.tokens.as_slice();
        let eq = self.token_at_offset(equals.range.offset);
        (toks.get(eq).map(|t| t.kind) == Some(TokenKind::Eq)).then_some(eq)
    }

    /// The pattern's first token before the `=` at index `eq`. The
    /// pattern's own outermost group closes right before the `=`, so the
    /// backward depth walk lands on its opener; an object pattern's
    /// written head (`Square<C>`, `p.C`) is absorbed onto that opener.
    /// The token before the result must be a statement boundary — a
    /// pattern assignment in expression position has no reliable window
    /// and keeps the under-report.
    #[allow(dead_code)] // rider helper: the listener.rs call sites are a later lane
    fn pattern_start_before(&self, eq: usize) -> Option<usize> {
        let toks = self.tokens.as_slice();
        let mut depth = 0i32;
        let mut i = eq;
        let start = loop {
            if i == 0 {
                return None;
            }
            i -= 1;
            match toks[i].kind {
                TokenKind::CloseParen | TokenKind::CloseSquare | TokenKind::CloseCurly => {
                    depth += 1;
                }
                TokenKind::OpenParen | TokenKind::OpenSquare | TokenKind::OpenCurly => {
                    if depth == 0 {
                        break self.pattern_head_start(i);
                    }
                    depth -= 1;
                }
                TokenKind::Semicolon if depth == 0 => break i + 1,
                _ => {}
            }
        };
        let boundary = match start.checked_sub(1).map(|i| toks[i].kind) {
            None => true,
            Some(TokenKind::Semicolon | TokenKind::OpenCurly | TokenKind::CloseCurly) => true,
            Some(_) => false,
        };
        boundary.then_some(start)
    }

    /// Absorbs an object pattern's written head onto its `(`: `Name`,
    /// `p.Name`, and a `<…>` argument group each extend the window left.
    #[allow(dead_code)] // rider helper: the listener.rs call sites are a later lane
    fn pattern_head_start(&self, open: usize) -> usize {
        let toks = self.tokens.as_slice();
        let mut j = open;
        if j > 0 && toks[j - 1].kind == TokenKind::Gt {
            let mut inner = 1i32;
            let mut k = j - 1;
            let mut found = None;
            while k > 0 {
                k -= 1;
                match toks[k].kind {
                    TokenKind::Gt => inner += 1,
                    TokenKind::Lt => {
                        inner -= 1;
                        if inner == 0 {
                            found = Some(k);
                            break;
                        }
                    }
                    _ => {}
                }
            }
            match found {
                Some(lt) => j = lt,
                None => return open,
            }
        }
        if j == 0
            || !matches!(
                toks[j - 1].kind,
                TokenKind::Identifier | TokenKind::Keyword(_)
            )
        {
            return open;
        }
        j -= 1;
        if j >= 2
            && toks[j - 1].kind == TokenKind::Period
            && matches!(
                toks[j - 2].kind,
                TokenKind::Identifier | TokenKind::Keyword(_)
            )
        {
            j -= 2;
        }
        j
    }

    /// The `;` offset closing the statement that owns the `=` at index
    /// `eq` (the RHS window's end), `None` when recovery left none.
    #[allow(dead_code)] // rider helper: the listener.rs call sites are a later lane
    fn statement_end_after(&self, eq: usize) -> Option<u32> {
        let toks = self.tokens.as_slice();
        let mut depth = 0i32;
        for token in &toks[eq + 1..] {
            match token.kind {
                TokenKind::OpenParen | TokenKind::OpenSquare | TokenKind::OpenCurly => depth += 1,
                TokenKind::CloseParen | TokenKind::CloseSquare | TokenKind::CloseCurly => {
                    if depth == 0 {
                        return Some(token.utf16_offset);
                    }
                    depth -= 1;
                }
                TokenKind::Semicolon if depth == 0 => return Some(token.utf16_offset),
                _ => {}
            }
        }
        None
    }

    /// The assignment RHS's static type: the tail non-`TypeUse` row inside
    /// `[equals.end, semi)` — the fold's outermost expression row
    /// ([`Self::single_row_since`]'s discipline).
    #[allow(dead_code)] // rider helper: the listener.rs call sites are a later lane
    fn assignment_rhs_ty(&self, equals: Token, semi: u32) -> Option<Ty> {
        let row = self
            .rows
            .iter()
            .filter(|row| !matches!(row.shape, RowShape::TypeUse(..)))
            .filter(|row| row.range.offset >= equals.range.end() && row.range.end() <= semi)
            .next_back()?;
        self.initializer_static_ty(row)
    }

    /// Splits a container's inner token window into its field
    /// sub-patterns: `(name?, lo, hi, rest)` per field — the name of a
    /// `name: p` record/object field (and of a `key: p` map entry, where
    /// `lo`/`hi` bound the VALUE), `None` for a positional/bare field;
    /// `rest` marks a `...` rest element (the marker itself is stripped
    /// from `lo`, the flag carries the column switch the callers apply).
    /// Depth-0 commas delimit.
    fn pattern_fields(&self, lo: usize, hi: usize) -> Vec<(Option<StringId>, usize, usize, bool)> {
        let toks = self.tokens.as_slice();
        let mut fields = Vec::new();
        let mut depth = 0i32;
        let mut start = lo;
        let mut i = lo;
        while i <= hi {
            let at_break = i == hi || {
                match toks[i].kind {
                    TokenKind::OpenParen | TokenKind::OpenSquare | TokenKind::OpenCurly => {
                        depth += 1;
                        false
                    }
                    TokenKind::CloseParen | TokenKind::CloseSquare | TokenKind::CloseCurly => {
                        depth -= 1;
                        false
                    }
                    TokenKind::Comma => depth == 0,
                    _ => false,
                }
            };
            if at_break {
                let mut lo = start;
                let hi = i;
                let mut rest = false;
                while lo < hi
                    && matches!(
                        toks[lo].kind,
                        TokenKind::PeriodPeriodPeriod | TokenKind::PeriodPeriodPeriodQuestion
                    )
                {
                    lo += 1;
                    rest = true;
                }
                // A `name: p` / `key: p` pair splits at the FIRST depth-0
                // colon inside the field slice.
                let mut inner_depth = 0i32;
                let mut colon = None;
                for j in lo..hi {
                    match toks[j].kind {
                        TokenKind::OpenParen | TokenKind::OpenSquare | TokenKind::OpenCurly => {
                            inner_depth += 1;
                        }
                        TokenKind::CloseParen | TokenKind::CloseSquare | TokenKind::CloseCurly => {
                            inner_depth -= 1
                        }
                        TokenKind::Colon if inner_depth == 0 => {
                            colon = Some(j);
                            break;
                        }
                        _ => {}
                    }
                }
                let (name, sub_lo) = match colon {
                    // `: pattern` — the field name is omitted and inferred
                    // from the sub-pattern's variable name
                    // (`PatternFieldNameImpl.name`: `nameNode.name ??
                    // pattern.variablePattern?.name`), the same rule the
                    // object-pattern getter column reads. A sub-pattern
                    // that is not a variable pattern infers no name.
                    Some(at) if at == lo => (self.pattern_field_name(at + 1, hi), at + 1),
                    Some(at) => (
                        if matches!(toks[lo].kind, TokenKind::Identifier | TokenKind::Keyword(_))
                            && lo + 1 == at
                        {
                            Some(self.intern(&self.source[toks[lo].byte_range()]))
                        } else {
                            None
                        },
                        at + 1,
                    ),
                    None => (None, lo),
                };
                if sub_lo < hi {
                    fields.push((name, sub_lo, hi, rest));
                }
                start = i + 1;
            }
            i += 1;
        }
        fields
    }

    /// The name a `: pattern` field infers from its sub-pattern
    /// (`PatternFieldNameImpl.name`: `nameNode.name ??
    /// pattern.variablePattern?.name`). Only a variable pattern carries
    /// one, and the official `variablePattern` getters reach it through
    /// the transparent wrapper chain — cast (`p as T`), null-assert
    /// (`p!`), null-check (`p?`) and parenthesized (`(p)`) all forward to
    /// their operand. A leaf is `name` or `T name`, optionally behind a
    /// `var`/`final` modifier. Every other shape (container, record,
    /// constant, relational, logical) answers `None`.
    fn pattern_field_name(&self, lo: usize, hi: usize) -> Option<StringId> {
        let toks = self.tokens.as_slice();
        let mut lo = lo;
        let mut hi = hi;
        loop {
            if lo >= hi {
                return None;
            }
            if toks[lo].kind == TokenKind::OpenParen {
                let close = self.tokens.matching_end(lo)?;
                if close + 1 != hi {
                    return None;
                }
                lo += 1;
                hi = close;
                continue;
            }
            // A depth-0 `as` casts, and the variable pattern is its
            // operand (`CastPatternImpl.variablePattern`).
            let mut depth = 0i32;
            let mut cast = None;
            for i in lo..hi {
                match toks[i].kind {
                    TokenKind::OpenParen | TokenKind::OpenSquare | TokenKind::OpenCurly => {
                        depth += 1;
                    }
                    TokenKind::CloseParen | TokenKind::CloseSquare | TokenKind::CloseCurly => {
                        depth -= 1;
                    }
                    TokenKind::Keyword(Keyword::As) if depth == 0 => cast = Some(i),
                    _ => {}
                }
            }
            if let Some(as_at) = cast {
                hi = as_at;
                continue;
            }
            // `p!` / `p?` unwrap to `p`.
            match toks.get(hi - 1).map(|t| t.kind) {
                Some(TokenKind::Bang | TokenKind::Question) => {
                    hi -= 1;
                    continue;
                }
                _ => break,
            }
        }
        // The `var`/`final` modifier is the leaf's own keyword.
        while lo < hi
            && matches!(
                toks[lo].kind,
                TokenKind::Keyword(Keyword::Var | Keyword::Final)
            )
        {
            lo += 1;
        }
        if lo >= hi {
            return None;
        }
        // A bare `name`, or a declared `T name` leaf. A `_` is a WILDCARD
        // pattern, not a variable pattern (`WildcardPatternImpl` inherits
        // the base `variablePattern => null`), so it infers no name and
        // the field stays positional.
        let name_at = if hi == lo + 1 {
            matches!(toks[lo].kind, TokenKind::Identifier).then_some(lo)
        } else {
            let mut carry = 0u32;
            let (_, next) = self.reconstruct_ty_at(lo, &mut carry)?;
            (next + 1 == hi && matches!(toks[next].kind, TokenKind::Identifier)).then_some(next)
        }?;
        let text = &self.source[toks[name_at].byte_range()];
        (text != "_").then(|| self.intern(text))
    }

    /// The `List<X>` element face of a matched named type, `None` when
    /// the matched head is not `List`.
    fn list_element_of(&self, ty: Ty) -> Option<Ty> {
        let Ty::Named { name, args, .. } = ty else {
            return None;
        };
        if self.strings.resolve(name) != "List" {
            return None;
        }
        self.boundary.pool().get(args).first().copied()
    }

    /// The `Map<K, V>` entry faces of a matched named type, `None` when
    /// the matched head is not `Map`.
    fn map_entries_of(&self, ty: Ty) -> Option<(Ty, Ty)> {
        let Ty::Named { name, args, .. } = ty else {
            return None;
        };
        if self.strings.resolve(name) != "Map" {
            return None;
        }
        let pool = self.boundary.pool();
        let args = pool.get(args);
        Some((*args.first()?, *args.get(1)?))
    }

    /// The refutable-shape row: `refutable_pattern_in_irrefutable_context`
    /// at the node's own extent (`.at(pattern)` — `shared_type_analyzer`
    /// `refutablePatternInIrrefutableContext`), then `makeRefutable()`
    /// silences the whole subtree (the caller returns).
    ///
    /// The row renders straight from the official registry descriptor:
    /// this crate's [`SemanticCode`] table carries no variant for it yet
    /// (the parser owns the constant-pattern face and emits its own row),
    /// so the semantic walk takes the generated `CodeInfo` and the
    /// registry-verbatim text rather than inventing a table entry outside
    /// its lane. A `SemanticCode::RefutablePatternInIrrefutableContext`
    /// variant would let this site use the shared emit path — rider on
    /// the code-table lane.
    fn emit_refutable_pattern(&mut self, lo: usize, hi: usize) {
        let Some(at) = self.token_span(lo, hi) else {
            return;
        };
        self.report.diagnostics.push(Diagnostic::new(
            Severity::Error,
            dartq_diagnostics::REFUTABLE_PATTERN_IN_IRREFUTABLE_CONTEXT,
            "Refutable patterns can't be used in an irrefutable context.",
            self.file,
            at,
            Some(
                "Try using an if-case, a 'switch' statement, or a 'switch' expression instead."
                    .to_string(),
            ),
        ));
    }

    /// The one verdict line: `matched` provably not assignable to
    /// `required` reports the node with both display faces (the
    /// `assignable` `None` stays silent — an undecidable name-level ask
    /// under-reports, never fabricates).
    fn emit_irrefutable(&mut self, at: SourceRange, matched: Ty, required: Ty) {
        // The undecidable faces answer NOTHING, never a fabricated row: a
        // `dynamic` matched type (every upstream check site spells
        // `matchedValueType is! SharedDynamicType`, so the sentinel never
        // reaches the lattice whatever `strict-casts` says), a
        // scope-parameter matched head (the name level cannot read its
        // bound, so upstream's `isSubtypeOf` question stays open — the
        // `T` vs `int` shape) and an unresolvable/invalid head all leave
        // the verdict `None`.
        if matches!(matched, Ty::Dynamic | Ty::Unknown) || self.is_invalid_ty(matched) {
            return;
        }
        // The 0c rule's other leg: `isAssignableTo` answers `true` when
        // EITHER side is `InvalidType` (subtype.dart:274-276), so an
        // unresolvable required head — `var Rectangle(:x) = o` with the
        // class undefined — is silent here rather than a mismatch row.
        if self.is_invalid_ty(required) {
            return;
        }
        if let Ty::Named { name, .. } = matched
            && self.is_scope_param(name)
        {
            return;
        }
        // The verdict: `assignable`'s nominal lattice answers first —
        // `Some` is the assignability answer itself, so the report fires
        // on `false`. When it abstains the declared-edge lattice answers
        // only a NAMED source whose head the model can decompose — an
        // unindexed `Object` head carries no declared edges at the `Ty`
        // level, so `assignable` defers while `Object <: List`/`String`/
        // record is false at every grade and the oracle's row still
        // lands. Two abstention faces stay SILENT: a `Record`/`Function`
        // matched face ([`NameTy::from_semantic`] maps it to a
        // shape-keyed sentinel the fallback cannot express).
        let verdict = match self.assignable(matched, required, false) {
            Some(assignable) => !assignable,
            None => matches!(matched, Ty::Named { .. }) && !self.name_subtype(matched, required),
        };
        if verdict {
            let a0 = self.ty_display(matched);
            let a1 = self.ty_display(required);
            self.emit(
                SemanticCode::PatternTypeMismatchInIrrefutableContext,
                at,
                &a0,
                &a1,
            );
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
    pub(crate) fn destructure_record(
        &mut self,
        init_start: u32,
        semi: u32,
    ) -> Vec<(StringId, SourceRange, Ty)> {
        self.check_irrefutable_destructure(init_start, semi);
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
    pub(crate) fn class_static_const_tail_ty(&mut self) -> Option<Ty> {
        // The member push replaces the qualifier row, so `A` survives only
        // as the member row's container receiver (the same shape
        // [`Self::classify_constant_tail`] reads for the enum tail).
        if let Some(last) = self.rows.last()
            && let RowShape::Member {
                recv:
                    Receiver::Container {
                        name,
                        kind: ContainerKind::Class,
                    },
            } = &last.shape
            && let Some(value) = self.const_field_values.get(&(*name, last.name))
        {
            // `Created` folds to the container's own type like an enum value
            // does, but its primitive equality is undecidable here — excluded
            // before the fold.
            if !matches!(value, ConstantValue::Created(_)) {
                let ty = self.folded_ty(value);
                if ty != Ty::UNKNOWN {
                    return Some(ty);
                }
            }
        }
        // The recovered-expression tail (`visitConstantPattern` evaluates
        // the pattern's expression unconditionally and reads `value.type`,
        // constant_verifier.dart:140-165): the parser-flagged shapes —
        // `const 0`, `~0`, `!false`, `const C()`, `const <int>[]`, `const
        // {}`, `const #a`, `const int`, `const bool.fromEnvironment` —
        // never arm the literal fold and never leave a class-static tail,
        // so without this probe their never-matches verdict drops
        // wholesale. The window is still open at this point (the caller
        // closes it after the type is answered); the peek applies the
        // latched prefix operators exactly as the close would and is a
        // no-op for the later [`ConstCapture::close_constant_pattern`].
        let node = self.const_capture.peek_constant_pattern_root()?;
        // A symbol literal's capture node spans only the `#` token — the
        // listener's `end_literal_symbol` hands the hash range because the
        // name's identifier parts live on the token stream. The upstream
        // never-matches row anchors at the ConstantPattern node (`const #a`
        // whole), so the root's span widens to the literal's real end: the
        // byte scan walks the source past `#` over `(ident .)* ident` and
        // `utf16_of` re-bases it.
        if matches!(node.expr, Expr::Value(ConstantValue::Symbol(_))) {
            let bytes = self.source.as_bytes();
            // `node.span` is UTF-16; `#` is ASCII, so the byte walk starts
            // at its end. The last identifier part's end is the literal end
            // — a trailing `.` does not count.
            let mut pos = self.byte_of(node.span.end());
            let mut last_ident_end = pos;
            let mut expect_ident = true;
            while pos < bytes.len() {
                let byte = bytes[pos];
                if byte == b'.' && !expect_ident {
                    expect_ident = true;
                    pos += 1;
                } else if byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'$' {
                    expect_ident = false;
                    pos += 1;
                    last_ident_end = pos;
                } else {
                    break;
                }
            }
            self.const_capture
                .respan_constant_pattern_root(self.utf16_of(last_ident_end));
        }
        let static_ty = self
            .rows
            .last()
            .map(|row| self.operand_static_ty(row))
            .filter(|ty| *ty != Ty::UNKNOWN);
        match eval(&node, self) {
            // A produced value is the oracle's DartObject: the gate reads
            // `value.type` once `hasPrimitiveEquality` accepts it — the
            // VALUE's own type first (a `Created`'s container, never the
            // `Type`-literal row the creation head left on top), then the
            // expression's static row for the faces `folded_ty` cannot
            // spell (List/Map/Type/Symbol/Record/Instance).
            Verdict::Value(value) => {
                if !self.const_value_primitive_equality(&value) {
                    return None;
                }
                let ty = self.folded_ty(&value);
                if ty != Ty::UNKNOWN {
                    Some(ty)
                } else {
                    static_ty
                }
            }
            // `Verdict::Constant` — a const expression whose value the
            // subset does not carry (the `const C()` creation faces the
            // eval leaves opaque): the oracle still has a DartObject whose
            // type is the expression's static type; its primitive equality
            // is the created head's `==`/`hashCode` walk.
            Verdict::Constant => {
                let ty = static_ty?;
                let Ty::Named { name, .. } = ty else {
                    return Some(ty);
                };
                self.created_primitive_equality(name).then_some(ty)
            }
            // A miss (`NotConstant`/`Unresolved`/`Fault`) is the oracle's
            // `value is! DartObjectImpl` path — never-matches stays silent
            // there even while the const-ness face reports its own row.
            _ => None,
        }
    }

    /// `hasPrimitiveEquality` for a folded value (upstream `value.dart`'s
    /// per-state answers): the primitive states accept unconditionally;
    /// collections and records accept when every element does; the enum
    /// and symbol/type literal states carry the official's own equality;
    /// a `Created` instance is decided by its container's `==`/`hashCode`
    /// walk ([`Self::created_primitive_equality`]); the opaque
    /// `Instance` accepts only on the function tear-off shape (the
    /// upstream `FunctionState` answer).
    fn const_value_primitive_equality(&self, value: &ConstantValue) -> bool {
        match value {
            ConstantValue::Null
            | ConstantValue::Bool(_)
            | ConstantValue::Int(_)
            | ConstantValue::Double(_)
            | ConstantValue::Str(_)
            | ConstantValue::Enum { .. }
            | ConstantValue::Type(_)
            | ConstantValue::Symbol(_) => true,
            ConstantValue::List(elems) | ConstantValue::Set(elems) => elems
                .iter()
                .all(|elem| self.const_value_primitive_equality(elem)),
            ConstantValue::Map(pairs) => pairs.iter().all(|(key, val)| {
                self.const_value_primitive_equality(key) && self.const_value_primitive_equality(val)
            }),
            ConstantValue::Record { positional, named } => positional
                .iter()
                .chain(named.iter().map(|(_, val)| val))
                .all(|elem| self.const_value_primitive_equality(elem)),
            ConstantValue::Created(name) => self.created_primitive_equality(*name),
            ConstantValue::Instance(kind) => matches!(kind, InstanceKind::Function),
        }
    }

    /// `GenericState.hasPrimitiveEquality` at the name level: the
    /// container's `==` and `hashCode` resolve concretely to `Object`'s —
    /// at this layer "no override anywhere" ([`Walk::Missing`], the
    /// stubbed-core walk cannot spell `Object` itself but a real override
    /// is always in-file) or "the found face is `Object`'s". A found face
    /// on any other container fails the gate; a broken chain stays
    /// undecidable and keeps the documented under-report.
    fn created_primitive_equality(&self, container: StringId) -> bool {
        let object = self.intern("Object");
        [self.intern("=="), self.intern("hashCode")]
            .iter()
            .all(|member| match self.walk_members(container, *member) {
                Walk::Missing => true,
                Walk::Found(_) => self.member_faces.last_origin() == Some(object),
                Walk::ChainBroken => false,
            })
    }

    /// Whether a local-scope declaration of `name` binds nothing: under the
    /// wildcard-variables feature a lone `_` declares no name in any frame
    /// scope, so neither the frame's resolution set ([`Self::add_local`],
    /// [`Self::note_local_function`]) nor its declared-type map
    /// ([`Self::bind_declared_type`], [`Self::bind_parameter_type`],
    /// [`Self::bind_inferred_ty`]) receives it — the official skips the
    /// element at every local-scope population site (upstream Dart SDK
    /// source, pkg/analyzer/lib/src/dart/element/scope.dart:537-557
    /// `LocalScope.add`, :106-124 `FormalParameterScope`, :15-35
    /// `ConstructorInitializerScope`), so a read falls through the lexical
    /// chain and reports undefined-name. The formal keeps its positional
    /// slot in the context schema ([`Self::formal_ordinal`]) and the
    /// tracked `locals` row stays for unused-warning bookkeeping; only the
    /// binding is skipped.
    ///
    /// Top-level declarations are outside every frame scope and never
    /// consults this gate — a top-level `_` is an ordinary library-scope
    /// name and keeps resolving.
    fn declares_nothing(&self, name: StringId) -> bool {
        self.is_wildcard(name)
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
        // pkg/analyzer/lib/src/dart/element/scope.dart:537-557), and the
        // formal scopes on the same feature gate (scope.dart:106-124
        // FormalParameterScope; scope.dart:15-35 ConstructorInitializerScope)
        // — so a later read falls through the lexical chain to
        // undefined-name. The tracked row is deliberately kept:
        // [`Self::add_tracked_local`] still pushes the usage entry (the
        // unused-local wildcard exemption walks the tracked vec, not this
        // resolution set), only the binding is skipped. The no-frame
        // branch (the top-level initializer world) stays ordinary — a
        // top-level `_` is a library-scope name, not a LocalScope row, and
        // keeps resolving.
        if self.declares_nothing(name) && self.frames.last().is_some() {
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
                // The window's first binder latches the join floor at its
                // own index (#3691): a same-name binder of an EARLIER
                // head sits below it and must stay unreachable — upstream
                // mints each `DeclaredVariablePattern` its own element.
                self.switch
                    .if_case_binder_floor
                    .get_or_insert(self.locals.len());
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
        // The if-case floor (#3691): inside an open window the sibling
        // scan starts at the head's own first binder, so a completed
        // head's same-named row below it can no longer be joined — its
        // element is spent. Outside a window (the for-in conversion) the
        // floor is `None` and the frame mark bounds as before.
        let lo = lo.max(self.switch.if_case_binder_floor.unwrap_or(0));
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
        // The statement-list scope binds FIRST-wins — upstream
        // `LocalScope.add` inserts through `_addGetter`'s
        // `_getters[element.name()] ??= element` (upstream Dart SDK
        // source, pkg/analyzer/lib/src/dart/element/scope.dart's
        // `EnclosedScope`), so a same-scope redeclaration leaves the
        // original name bound and the later read lands on the FIRST
        // declaration. [`Engine::resolve_local`] consults this stored
        // range to bind the same row; the context row the verifier
        // attaches ("The first definition of this name.") keys on the
        // same first declaration.
        let first = self.dup_scopes.last_mut().and_then(|scope| {
            scope
                .get(&name)
                .copied()
                .or_else(|| scope.insert(name, range))
        });
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
    /// bound type keeps a resolved alias head's use-site spelling (`A<int>`
    /// stays `A<int>`, never the expanded `int Function()`) — the oracle's
    /// `getDisplayString(preferTypeAlias)` reads `TypeImpl.alias`, a
    /// decoration on the instantiated type, so the written spelling is a
    /// presentation property every downstream message face renders back;
    /// the verdict layer still unfolds the kept spelling at the ask
    /// (`fold_alias_verdict`). Every other shape folds through the alias
    /// first — the frame's declared types are the resolved interface types
    /// every verdict reads. A bare parameterized head surviving the fold
    /// (`G source` over `typedef G<X extends A<X>> = X Function()`, `G?`
    /// alike) binds its instantiate-to-bounds face — the oracle's resolved
    /// `staticType` for a written `G` is `G<A<dynamic>>` (`type.dart`
    /// `instantiateTypeFormalsToBounds`), so the declared slot carries the
    /// defaults, never the raw head.
    pub(crate) fn bind_declared_type(&mut self, name: StringId) {
        let ty = self.pending_decl_ty.unwrap_or(Ty::UNKNOWN);
        let ty = match ty {
            Ty::Named { name, args, .. }
                if (args == ArgListId::EMPTY && self.alias_rhs(name).is_some())
                    || (args != ArgListId::EMPTY && self.alias_rhs_any(name).is_some()) =>
            {
                ty
            }
            _ => self.fold_alias_ty(ty),
        };
        let ty = self.instantiated_head_ty(ty);
        if self.declares_nothing(name) {
            return;
        }
        if let Some(frame) = self.frames.last_mut() {
            frame.var_types.insert(name, ty);
        }
    }

    /// Binds a formal or catch parameter's own declared type (one arm per
    /// parameter, not shared with a group). The bound type keeps a resolved
    /// alias head's use-site spelling for the message faces (the same
    /// `preferTypeAlias` posture [`Self::bind_declared_type`] holds; the
    /// verdict layer unfolds it at the ask) and folds every other shape
    /// through the alias first — the frame's declared types are the
    /// resolved interface types every verdict reads. An implicitly typed formal of an
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
    ///
    /// A wildcard `_` formal binds no slot at all
    /// ([`Self::declares_nothing`]); the written or inferred type is still
    /// consumed (`armed_ty` never leaks into the next parameter) and the
    /// formal's positional slot in the context schema keeps counting
    /// ([`Self::formal_ordinal`]).
    pub(crate) fn bind_parameter_type(&mut self, name: StringId) {
        let ty = self
            .armed_ty
            .take()
            .or_else(|| self.closure_param_context_ty(name))
            .unwrap_or(Ty::UNKNOWN);
        let ty = match ty {
            Ty::Named { name, args, .. }
                if (args == ArgListId::EMPTY && self.alias_rhs(name).is_some())
                    || (args != ArgListId::EMPTY && self.alias_rhs_any(name).is_some()) =>
            {
                ty
            }
            _ => self.fold_alias_ty(ty),
        };
        let ty = self.instantiated_head_ty(ty);
        if self.declares_nothing(name) {
            return;
        }
        if let Some(frame) = self.frames.last_mut() {
            frame.var_types.insert(name, ty);
        }
    }

    /// The open formal's positional ordinal inside the innermost frame's
    /// formal list: the parameter rows the frame already carries, less the
    /// formal being bound (its own row precedes the type bind — every call
    /// site runs `note_parameter`/`note_catch_parameter` first).
    ///
    /// The tracked rows decide the ordinal, never `var_types.len()`: a
    /// wildcard `_` formal binds no declared-type slot
    /// ([`Self::declares_nothing`]) yet keeps its positional place in the
    /// context schema — the official element list keeps the formal and
    /// drops only its name, so `void Function(int, String)` pairs `(_, s)`'s
    /// `s` with `String` (`_` claims `int`). Dropping the slot from the
    /// count would shift every following formal by one.
    fn formal_ordinal(&self) -> Option<usize> {
        let lo = *self.frame_local_marks.last()?;
        Some(
            self.locals
                .get(lo..)?
                .iter()
                .filter(|decl| matches!(decl.kind, LocalKind::Parameter { .. }))
                .count()
                .saturating_sub(1),
        )
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
            SchemaSlot::Positional(self.formal_ordinal()?)
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
        let (slot, callee_row, unnamed_head, named_head) = {
            let level = self.arg_levels.last()?;
            (
                match level.pending_name {
                    Some(label) => SchemaSlot::Named(label),
                    None => SchemaSlot::Positional(
                        level
                            .args
                            .iter()
                            .filter(|(label, ..)| label.is_none())
                            .count(),
                    ),
                },
                level.callee_row,
                level.unnamed_head,
                level.named_head,
            )
        };
        // The creation-head lane mirrors [`Self::arg_slot_ty_ctx`]'s: the
        // callee transport drops the row for a keyword creation
        // (`new F0((a) { … })`) and a written-group head, but the level's
        // own latches still spell it — `unnamed_head` is the class whose
        // constructor table supplies the literal's formal, `named_head`
        // is the `.named` selector whose class segment sits before the
        // dot (`new F0.n((x) { … })` keys `n` against `F0`'s named table)
        // — both already carrying the written-group/to-bounds
        // substitution, so `F3<int>((x) { … })` binds `x := int` where the
        // callee-formals lane would leave `T` raw. A prefix-qualified
        // `const p.C(` resolves through the prefix's table the same
        // helper consults. A named-selector creation head resolves here
        // even when the callee transport kept a row: `new C<int>.n(`
        // latches BOTH `callee_row` (the dropped fold's stale index — its
        // row is an empty-named placeholder, not the `.n` selector) and
        // `named_head` (the authoritative `n` head whose class segment
        // `slot_creation_class` byte-walks), so the named head lane must
        // win over the callee-row path that would read the wrong row.
        if callee_row.is_none() || named_head.is_some() {
            let (head, range, named) = match (unnamed_head, named_head) {
                (Some((head, range)), _) => (head, range, false),
                (None, Some((selector, range))) => (selector, range, true),
                (None, None) => return None,
            };
            let formals = self.slot_creation_formals(head, range, named);
            return formals
                .and_then(|formals| match slot {
                    SchemaSlot::Positional(index) => formals.positional.get(index).map(|f| f.ty),
                    SchemaSlot::Named(label) => formals.named_formal(label).map(|f| f.ty),
                })
                .and_then(|ty| self.closure_schema_sig(ty))
                .and_then(|sig| Self::schema_entry(&sig, formal));
        }
        let callee_row = callee_row?;
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
        // The pass-2 re-bind abstention: the upward face does not re-run
        // when the argument list is gone (`callee_row=None` — the deferred
        // re-derive), so a schema entry still spelling an unsolved
        // inference parameter is the horizontal-inference placeholder,
        // never a decision — the frozen pass-1 bind (a solved face like
        // the disagreeing-siblings `Object` LUB, or an honest Unknown)
        // must not be overwritten by it. `b.reduce(C(), D(), (a) { … })`
        // solved `a := Object` at bind time; the pass-2 re-derive, blind
        // to the already-typed arguments, answered the raw `S` and the
        // narrowing gate let the placeholder replace the decided face —
        // the member-miss then read the parameter's bound arm
        // (`'<unknown>'`) where the oracle's solved face reports
        // `'Object'`. The `dynamic` under-report for a param face whose
        // pass-1 bind stayed `Unknown` is preserved downstream: this
        // abstention only fires when a callee window declares the name.
        if callee_row.is_none()
            && self
                .closure_callee_params(&recv, callee_name)
                .is_some_and(|(params, ..)| {
                    let names: Vec<StringId> = params.iter().map(|(name, _)| *name).collect();
                    !names.is_empty() && self.ty_names_any(schema, &names)
                })
        {
            return None;
        }
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
                // posture — the oracle resolves them). A class the local
                // walk CAN see — the diagnosing file's own declaration, or
                // a sibling graph unit's indexed members — answers real
                // `undefined_*` rows the oracle reports (`b.reduce(Ann(),
                // (a) => a.noSuchEver())` reports `undefined_method 'A'`
                // upstream), so only a container whose member table is out
                // of the walk's reach joins the suppression.
                if callee_own_head && let Some(solved) = instantiated {
                    if let Ty::Named { name, .. } = solved {
                        // `Object` is the one unindexed head whose member
                        // universe is frozen-complete (`members.object_members`
                        // enumerates it): a solved-`Object` formal's miss
                        // (`a.noSuchEver()` under `LUB(C, D)`) is a KNOWN
                        // `undefined_method 'Object'` row, never the
                        // under-resolved face the suppression exists for —
                        // the walk's silence on a table-complete head is
                        // evidence, not posture.
                        let object = name == self.intern("Object");
                        let walkable = object
                            || resolve_container(
                                self.graph,
                                self.namespaces,
                                self.index,
                                self.lib,
                                self.file,
                                name,
                            )
                            .is_some();
                        if !walkable {
                            self.inferred_formal_containers.insert(name);
                        }
                    }
                    return Some(solved);
                }
                // An unsolved type-PARAMETER spelling no enclosing scope
                // declares — the receiver container's or the callee's own
                // parameter the instantiation above could not seed
                // (`Bag<L>().map((e) => …)`'s `E` off a creation receiver,
                // `extension<T> on T?`'s `T`, `Bag<Und>`'s rejected `T`) — is
                // OUR placeholder, not a type the caller's scope can name:
                // the unit-wide declaration table answers the spelling as a
                // type, but the oracle solved it. Binding it hands the
                // bound-aware receiver faces a parameter whose implicit
                // `Object?` bound fabricates `undefined_*` and
                // `unchecked_use_of_nullable_value` rows the oracle never
                // reports, so the formal keeps `dynamic` (the under-report).
                // Only the pass-1 bind sees the scope; the pass-2 re-bind
                // keeps its own narrowing gates.
                if callee_row.is_some()
                    && !self.is_scope_param(head)
                    && self
                        .index
                        .get(self.file)
                        .and_then(|elements| elements.decl(head))
                        .is_some_and(|decl| decl.kind == MicroKind::TypeParam)
                {
                    return None;
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
                // Conflicting lower constraints merge to their LUB, the
                // same `MergedTypeConstraint`/`chooseTypes` rule the
                // send-close solver applies below — `C <: S` + `D <: S`
                // binds `S := LUB(C, D)` (the oracle's `Object` for
                // disagreeing sibling arguments). An undecidable join
                // still poisons.
                Some(kept) => {
                    let joined = self.closure_lub(kept, ty);
                    if self.schema_type_is_decidable(joined) {
                        solutions[index] = Some(joined);
                    } else {
                        poisoned.insert(index);
                        solutions[index] = None;
                    }
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
        for (name, ty, _, range) in args {
            let formal = match name {
                Some(label) => formals.named_formal(*label),
                None => {
                    let formal = formals.positional.get(positional_index);
                    positional_index += 1;
                    formal
                }
            };
            // The solve-only argument type: an implicit creation argument
            // closes untyped ([`Self::creation_arg_ty`]); `level.args`
            // keeps its `Unknown`, so the argument verdict reads the same
            // posture as before.
            let ty = if *ty == Ty::Unknown {
                self.creation_arg_ty(*range).unwrap_or(Ty::Unknown)
            } else {
                *ty
            };
            if let (Some(formal), true) = (formal, ty != Ty::Unknown) {
                self.closure_unify_upward(formal.ty, ty, &params, &mut candidates);
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
                // Conflicting lower constraints merge to their LUB
                // (upstream `MergedTypeConstraint`/`chooseTypes`:
                // `C <: S` + `D <: S` binds `S := LUB(C, D)` — the oracle's
                // `Object` for disagreeing sibling arguments). An
                // undecidable join still poisons.
                Some(kept) => {
                    let joined = self.closure_lub(kept, ty);
                    if self.schema_type_is_decidable(joined) {
                        solutions[index] = Some(joined);
                    } else {
                        poisoned.insert(index);
                        solutions[index] = None;
                    }
                }
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
            && let Some(Some((name, _, VarInferSlot::Local))) = self.init_declarators.last()
        {
            self.invalid_typed_locals.insert(*name);
        }
        if !map.is_empty() {
            self.solved_call_params.insert(row.range.offset, map);
        }
    }

    /// The static type of an implicit unnamed creation argument (`f(A())`)
    /// for the send-close solve: the parser reports the creation as a send
    /// of the class name, so its value row reaches the enclosing send's
    /// close untyped, while upstream's argument is the
    /// `InstanceCreationExpression` — statically the created class
    /// (`identity(A())` binds `T := A`). Only an exact `Name(...)` argument
    /// (the creation's `)` ends the argument) over a non-generic class
    /// answers: a generic class's instantiation is the creation's own
    /// inference, so it keeps the free-parameter under-report, as does a
    /// name a local, a type parameter, or an alias owns.
    fn creation_arg_ty(&self, range: SourceRange) -> Option<Ty> {
        let tokens = self.tokens.as_slice();
        let at = tokens.partition_point(|t| t.utf16_offset < range.offset);
        tokens
            .get(at)
            .filter(|t| t.utf16_offset == range.offset && t.kind == TokenKind::Identifier)?;
        tokens
            .get(at + 1)
            .filter(|t| t.kind == TokenKind::OpenParen)?;
        let close = self.tokens.matching_end(at + 1)?;
        if !matches!(
            tokens.get(close + 1).map(|t| t.kind),
            Some(TokenKind::Comma | TokenKind::CloseParen)
        ) {
            return None;
        }
        let name = self.intern(self.tokens.lexeme(self.source, at));
        if self.type_params.contains(&name)
            || self.pending_type_params.contains(&name)
            || self.type_resolution(name).0 != TypeResolution::Type
        {
            return None;
        }
        let (container, _) = resolve_container(
            self.graph,
            self.namespaces,
            self.index,
            self.lib,
            self.file,
            name,
        )?;
        (container.kind == ContainerKind::Class && container.type_params == 0).then_some(
            Ty::Named {
                name,
                nullable: false,
                args: ArgListId::EMPTY,
            },
        )
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
            // A named-constructor selector (`new C<T>.n((x) { … })`,
            // `p.C<T>.n(`, `C<T>.n(`) is no member — the member walk never
            // reaches `ctor_formals`, so when the callee name IS a
            // registered constructor the container's own parameters pair
            // against the call's written group: `C<int>.n((x) { … })`
            // binds `x := int` through the constructor's `C<T>`
            // environment. A static member on the same receiver answers
            // through the member arm below instead — `C.m(` keeps its
            // method parameters.
            if let Some(params) = self.ctor_selector_params(*recv, callee_name) {
                return Some(params);
            }
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
        if let Some(window) = self.own_exec_window(callee_name, false)
            && !window.params.is_empty()
        {
            let own = window.params.len();
            return Some((window.params.to_vec(), Vec::new(), Some(own)));
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

    /// The named-constructor selector's inference environment
    /// ([`Self::closure_callee_params`]'s creation arm): `callee_name` is
    /// a registered constructor of the receiver's container — `C<T>.n(`,
    /// `new C<T>.n(`, `p.C<T>.n(` — and the container's own parameters
    /// pair against the call's written type arguments exactly like an own
    /// member's would, the receiver's written instantiation seeding them.
    /// `None` when the callee is no constructor (a static member takes
    /// the member arm) or the container declares no parameters.
    fn ctor_selector_params(
        &self,
        recv: Receiver,
        callee_name: StringId,
    ) -> Option<(
        Vec<(StringId, Option<Ty>)>,
        Vec<(StringId, Ty)>,
        Option<usize>,
    )> {
        let (class, recv_ty) = match recv {
            Receiver::Container { name, .. } => (name, None),
            Receiver::Created { name, ty, .. } => (name, Some(ty)),
            Receiver::PrefixedContainer { name, .. } => (name, None),
            _ => (StringId::NONE, None),
        };
        let (container, _owner) = if let Receiver::PrefixedContainer { prefix, name, .. } = recv {
            prefixed_container(
                self.graph,
                self.namespaces,
                self.index,
                self.lib,
                prefix,
                name,
            )?
        } else if class != StringId::NONE {
            resolve_container(
                self.graph,
                self.namespaces,
                self.index,
                self.lib,
                self.file,
                class,
            )?
        } else {
            return None;
        };
        // The selector must name a declared constructor — a static member
        // answers through the member arm below, never here. The membership
        // face is `constructors` (the same declared-selector table the
        // tear-off face reads); `ctor_formals` keys on the unnamed face's
        // class spelling and is a formals table, not a membership test.
        if !container.constructors.contains(&callee_name) {
            return None;
        }
        // The container's own parameters pair positionally against the
        // receiver's written instantiation — `bounds.container_params`
        // carries the same-file declared names (the member arm's source of
        // truth); a prefix-resolved container answers through its own
        // `type_param_names`/`type_param_bounds` tables (the bounds index
        // keys on the diagnosing file's containers only). The bound trees
        // translate through `tree_ty` so the `T extends num` spellings keep
        // their bounds in the schema.
        let params = self.bounds.container_params(class).or_else(|| {
            (!container.type_param_names.is_empty()).then(|| {
                container
                    .type_param_names
                    .iter()
                    .map(|name| (*name, None, false))
                    .collect()
            })
        })?;
        if params.is_empty() {
            return None;
        }
        let params: Vec<(StringId, Option<Ty>)> = params
            .iter()
            .map(|(name, bound, _)| (*name, bound.as_ref().map(|tree| self.tree_ty(tree))))
            .collect();
        // The seed source splits on the receiver face: a folded `Created`
        // receiver's `ty` already carries the solved `C<int>` arguments; a
        // `Container`/`PrefixedContainer` receiver spells them on the class
        // segment before the selector (`C<int>.n(`, `p.C<int>.n(`), which
        // `slot_creation_class`+`slot_head_group_args` recover from the
        // callee row's byte range — the same read the head lane's
        // `slot_creation_formals` runs. The callee row is the innermost
        // open send's `.n` row — the most recent qualified row spelling
        // `callee_name` still on the stack while its arguments bind.
        let mut seeds: Vec<(StringId, Ty)> = Vec::new();
        let written_args: Option<Vec<Ty>> = if let Some(Some(Ty::Named { args, .. })) = recv_ty
            && args != ArgListId::EMPTY
        {
            Some(self.boundary.pool().get(args).to_vec())
        } else {
            self.rows
                .iter()
                .rev()
                .find(|row| row.name == callee_name && row.qualified)
                .and_then(|row| self.slot_creation_class(row.range))
                .and_then(|(seg, seg_offset)| self.slot_head_group_args(seg, seg_offset))
        };
        if let Some(arg_list) = written_args
            && arg_list.len() == params.len()
        {
            for ((param, _), ty) in params.iter().zip(arg_list.iter()) {
                seeds.push((*param, *ty));
            }
        }
        Some((params, seeds, None))
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

    /// The send-close upward solve's merge for two conflicting lower
    /// constraints on the same inference parameter — upstream
    /// `MergedTypeConstraint`/`chooseTypes`: `C <: S` + `D <: S` binds
    /// `S := LUB(C, D)` (the oracle's `Object` for disagreeing sibling
    /// arguments, its `num` for an `int`/`double` pair). The candidates
    /// this merge sees are always VALUE types (`closure_unify_upward`
    /// pushes an argument's static type, never a schema parameter), so a
    /// bare-headed or argument-carrying `Named` pair decides through the
    /// supertype lattice; a `Null`/`Never`/top leg and every
    /// non-`Named` shape keep the name-level under-report (the merge's
    /// `schema_type_is_decidable` gate still poisons the slot).
    fn closure_lub(&self, left: Ty, right: Ty) -> Ty {
        if left == right {
            return left;
        }
        // TOP absorbs; BOTTOM yields — the same lattice edges the
        // composite join runs (least_upper_bound.dart:405-451).
        if matches!(left, Ty::Dynamic | Ty::Void) {
            return left;
        }
        if matches!(right, Ty::Dynamic | Ty::Void) {
            return right;
        }
        if left == Ty::Never {
            return right;
        }
        if right == Ty::Never {
            return left;
        }
        // `UP(Null, T)` is `T?`; `Null` against `Null` already matched.
        if left == Ty::Null {
            return Self::nullable_form(right);
        }
        if right == Ty::Null {
            return Self::nullable_form(left);
        }
        // The nullability merge: `UP(T1?, T2) = UP(T1, T2)?`.
        if Self::closure_lub_nullable_leg(left) || Self::closure_lub_nullable_leg(right) {
            return Self::nullable_form(
                self.closure_lub(Self::non_null_form(left), Self::non_null_form(right)),
            );
        }
        let (
            Ty::Named {
                name: l, args: la, ..
            },
            Ty::Named {
                name: r, args: ra, ..
            },
        ) = (left, right)
        else {
            // A non-`Named` leg (a function/record/invalid face) has no
            // name-level common supertype — the under-report posture.
            return Ty::Unknown;
        };
        let world = self.name_world();
        if l == r {
            // Same head over covariant arguments: the pairwise LUB
            // (`Iterable<int>`/`Iterable<double>` → `Iterable<num>`); a
            // bare or arity-mismatched pair keeps the shared head.
            let (left_args, right_args) = {
                let pool = self.boundary.pool();
                (pool.get(la).to_vec(), pool.get(ra).to_vec())
            };
            if la != ra && left_args.len() == right_args.len() && !left_args.is_empty() {
                let joined: Vec<Ty> = left_args
                    .iter()
                    .zip(right_args.iter())
                    .map(|(&a, &b)| self.closure_lub(a, b))
                    .collect();
                if joined.iter().all(|ty| *ty != Ty::Unknown) {
                    return self.boundary.args.borrow_mut().named(l, false, &joined);
                }
            }
            return Ty::Named {
                name: l,
                nullable: false,
                args: if la == ra { la } else { ra },
            };
        }
        // A subtype pair answers the supertype directly.
        if world.named_sub(l, r) && (la == ArgListId::EMPTY || la == ra) {
            return Ty::Named {
                name: r,
                nullable: false,
                args: ra,
            };
        }
        if world.named_sub(r, l) && (ra == ArgListId::EMPTY || ra == la) {
            return Ty::Named {
                name: l,
                nullable: false,
                args: la,
            };
        }
        // Distinct heads: the least common supertype over the named
        // lattice — the same candidate set and domination walk the
        // composite join's common-supertype arm runs. Bare operands walk
        // name-level (`int`/`double` answers `num`, unrelated classes
        // `Object`). An arg-carrying pair needs the shared head certified
        // under ONE instantiation on both sides — the official interface
        // LUB's supertype-set intersection (`_computeLeastUpperBound`,
        // least_upper_bound.dart:162-177): `List<int>`/`Set<int>` both view
        // `Iterable<E>` with `E := int`, so `Iterable<int>` stands; views
        // that differ (`C<int>`/`D<String>` over a shared `I`) are not in
        // the intersection and the `Object` terminal keeps the oracle's
        // answer. Without the strict snapshot the certification is
        // unavailable and the `Object` under-report stands.
        let mut heads: Vec<StringId> = vec![l, r, self.intern("Object")];
        for name in [
            "num",
            "Comparable",
            "Iterable",
            "List",
            "Set",
            "Map",
            "String",
            "Pattern",
            "bool",
            "Enum",
            "Function",
            "Future",
            "Record",
        ] {
            heads.push(self.intern(name));
        }
        if let Some(elements) = self.index.get(self.file) {
            for (name, decl) in elements.declarations() {
                if matches!(
                    decl.kind,
                    MicroKind::Class | MicroKind::Mixin | MicroKind::Enum | MicroKind::Extension
                ) {
                    heads.push(name);
                }
            }
        }
        heads.sort_unstable();
        heads.dedup();
        if la != ArgListId::EMPTY || ra != ArgListId::EMPTY {
            let Some(ops) = self.ops else {
                return Ty::named(self.intern("Object"));
            };
            let mut level: Vec<Ty> = Vec::new();
            for head in heads {
                let (Some(left_env), Some(right_env)) = (
                    ops.instance_env(left, head, &self.boundary.args),
                    ops.instance_env(right, head, &self.boundary.args),
                ) else {
                    continue;
                };
                if left_env != right_env {
                    continue;
                }
                let args: Vec<Ty> = left_env.into_iter().map(|(_, ty)| ty).collect();
                level.push(self.boundary.args.borrow_mut().named(head, false, &args));
            }
            if level.is_empty() {
                return Ty::named(self.intern("Object"));
            }
            // The same domination descent the bare walk runs, the
            // comparability ask at type level (`List<int>` dominates its
            // `Iterable<int>` view even where the name lattice has no
            // builtin edge): a candidate nothing strictly subtypes is the
            // join when unique; a tied top level drops wholesale and the
            // descent continues among what it dominated.
            loop {
                let dominated: Vec<usize> = (0..level.len())
                    .filter(|&i| {
                        (0..level.len()).any(|j| {
                            j != i
                                && self.slot_subtype(level[j], level[i]) == Some(true)
                                && self.slot_subtype(level[i], level[j]) != Some(true)
                        })
                    })
                    .collect();
                let top: Vec<Ty> = level
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| !dominated.contains(i))
                    .map(|(_, &ty)| ty)
                    .collect();
                if top.len() == 1 {
                    return top[0];
                }
                if top.len() == level.len() {
                    return Ty::named(self.intern("Object"));
                }
                level = top;
            }
        }
        let mut level: Vec<StringId> = heads
            .into_iter()
            .filter(|&head| head != l && head != r)
            .filter(|&head| world.named_sub(l, head) && world.named_sub(r, head))
            .collect();
        if level.is_empty() {
            return Ty::named(self.intern("Object"));
        }
        loop {
            let dominated: Vec<usize> = (0..level.len())
                .filter(|&i| {
                    (0..level.len()).any(|j| {
                        j != i
                            && world.named_sub(level[j], level[i])
                            && !world.named_sub(level[i], level[j])
                    })
                })
                .collect();
            let top: Vec<StringId> = level
                .iter()
                .enumerate()
                .filter(|(i, _)| !dominated.contains(i))
                .map(|(_, &head)| head)
                .collect();
            if top.len() == 1 {
                return Ty::named(top[0]);
            }
            if top.len() == level.len() {
                return Ty::named(self.intern("Object"));
            }
            level = top;
        }
    }

    /// [`Self::closure_lub`]'s nullability-merge leg predicate (named apart
    /// from the composite join's own — a `Named`/`Function`/`Record` leg
    /// carrying the trailing `?` rewrites through `non_null_form`).
    fn closure_lub_nullable_leg(ty: Ty) -> bool {
        matches!(
            ty,
            Ty::Named { nullable: true, .. }
                | Ty::Function { nullable: true, .. }
                | Ty::Record { nullable: true, .. }
        )
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
            // The bare-creation callee (`F0([...])`) reaches here too:
            // its class declaration answers no `formals`, so the
            // unnamed-constructor table (keyed on the class's own name)
            // supplies the slot — `F0(List<int> a)` types the literal's
            // context `List<int>` exactly like the keyword spelling.
            Receiver::Unknown => self
                .top_level_decl(name)
                .and_then(|d| d.formals.as_deref().cloned())
                .or_else(|| self.ctor_schema_formals(name, name))
                .or_else(|| self.factory_ctor_formals(name, name)),
            // A prefixed callee resolves through the prefix's import table,
            // never this unit's unprefixed constructor tables — a local
            // class `C` must not supply `p.C(`'s formals. The prefix
            // segment rides the receiver's `name`; the callee row spells
            // the class/selector behind it, so `p.C(` reads `C`'s unnamed
            // constructor table from the prefix-resolved container
            // (`p.f(...)`'s function-declaration arm keeps its own lane —
            // the element model answers the prefixed function's formals
            // the same walk serves).
            Receiver::Prefix { name: prefix } => self
                .top_level_decl(name)
                .and_then(|d| d.formals.as_deref().cloned())
                .or_else(|| {
                    let (container, owner) = prefixed_container(
                        self.graph,
                        self.namespaces,
                        self.index,
                        self.lib,
                        *prefix,
                        name,
                    )?;
                    let mut formals = (**container
                        .ctor_formals
                        .get(&name)
                        .or_else(|| container.factory_formals.get(&name))?)
                    .clone();
                    self.project_formals(owner, &mut formals);
                    self.patch_field_formals(name, &mut formals);
                    Some(formals)
                }),
            // A constructor invocation (`Eb._(cb: (n) => …)`, the below
            // row a bare class name): constructors are not
            // [`MemberEntry`] rows, so the member walk never reaches
            // them — the container's own table answers as the fallback.
            Receiver::Container { name: class, .. } => self
                .member_formals_full(recv, name)
                .map(|f| f.into_owned())
                .or_else(|| self.ctor_schema_formals(*class, name))
                .or_else(|| self.factory_ctor_formals(*class, name)),
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

    /// The argument-slot context's creation-head formal table: the callee
    /// transport drops the row for a keyword creation (`new F0([...])`) and
    /// a written-group head (`F0<int>(`, `C<int>.named(`), so
    /// [`Self::arg_slot_ty_ctx`] re-derives the resolved container and the
    /// selected constructor from the level's own head latches — the same
    /// source-spelled walk `invocation.rs`'s `creation_class_before` runs
    /// for the fold, kept local because the byte walk answers the pair
    /// (class segment AND optional prefix) the formals need.
    ///
    /// `named` selects the table key: the unnamed face keys on the class's
    /// own name (`F0(` → `ctor_formals[F0]`), the named face on the
    /// written selector (`F0.n(` → `ctor_formals[n]`). A written
    /// `<...>` group on the class segment instantiates the container's
    /// own parameters into the formals (`F0<int>([...])` reads
    /// `List<int>`, never `List<T>`); the to-bounds face of a raw head
    /// stays with the caller's unsolved-parameter gate (the literal
    /// element check adjudicates the raw `List<T>` no worse than the
    /// bind-time schema did).
    fn slot_creation_formals(
        &mut self,
        name: StringId,
        range: SourceRange,
        named: bool,
    ) -> Option<Formals> {
        // `name`/`range` spell the head the latch carries — the class for
        // an unnamed head, the `.named` selector for a named one. The
        // class segment sits before it, over one `.` and a balanced
        // `<...>` group when written; for the unnamed face it IS `name`.
        let (class, selector, class_offset) = if named {
            let (class, class_offset) = self.slot_creation_class(range)?;
            (class, name, class_offset)
        } else {
            (name, name, range.offset)
        };
        // A `p.` before the class segment resolves through the prefix's
        // import table, never this unit's own scope.
        let (container, owner) = match self.slot_head_prefix(class_offset) {
            Some(prefix) => prefixed_container(
                self.graph,
                self.namespaces,
                self.index,
                self.lib,
                prefix,
                class,
            )?,
            None => resolve_container(
                self.graph,
                self.namespaces,
                self.index,
                self.lib,
                self.file,
                class,
            )?,
        };
        let mut formals = (**container
            .ctor_formals
            .get(&selector)
            .or_else(|| container.factory_formals.get(&selector))?)
        .clone();
        self.project_formals(owner, &mut formals);
        self.patch_field_formals(class, &mut formals);
        // The class segment's own written group (`F0<int>`) instantiates
        // the container's parameters into every formal — the send's
        // solved `List<int>` face. A raw head instantiates to the
        // declared bounds (`instantiateToBounds`: `F3<dynamic>` — an
        // unbounded parameter's `dynamic` accepts every literal element
        // the way the oracle's inferred instantiation does); an arity
        // mismatch leaves the formals unsubstituted (the wrong-arity
        // verdict is another layer's).
        let args = self.slot_head_group_args(class, class_offset);
        let keys: FxHashMap<StringId, Ty> = match args {
            Some(args) if args.len() == container.type_param_names.len() => container
                .type_param_names
                .iter()
                .copied()
                .zip(args.iter().copied())
                .collect(),
            _ => container
                .type_param_names
                .iter()
                .copied()
                .zip(container.type_param_bounds.iter().copied())
                .map(|(param, bound)| {
                    let bound = self.boundary.ty(owner, bound);
                    (
                        param,
                        if bound == Ty::UNKNOWN {
                            Ty::Dynamic
                        } else {
                            bound
                        },
                    )
                })
                .collect(),
        };
        if !keys.is_empty() {
            for formal in formals
                .positional
                .iter_mut()
                .map(|f| &mut f.ty)
                .chain(formals.named.iter_mut().map(|f| &mut f.ty))
            {
                *formal = self.substitute_ty(*formal, &keys);
            }
        }
        Some(formals)
    }

    /// The class segment a named-selector creation head spells
    /// (`C.named(`, `C<int>.named(`): the identifier before the selector's
    /// `.`, across a balanced `<...>` type-argument group when one sits
    /// between — the same ASCII byte walk `invocation.rs`'s
    /// `creation_class_before` runs for the fold. Returns the interned
    /// class name and the segment's UTF-16 start offset.
    fn slot_creation_class(&self, selector: SourceRange) -> Option<(StringId, u32)> {
        let bytes = self.source.as_bytes();
        let mut cursor = self.byte_of(selector.offset);
        while cursor > 0 && bytes[cursor - 1].is_ascii_whitespace() {
            cursor -= 1;
        }
        if cursor == 0 || bytes[cursor - 1] != b'.' {
            return None;
        }
        cursor -= 1;
        while cursor > 0 && bytes[cursor - 1].is_ascii_whitespace() {
            cursor -= 1;
        }
        if cursor > 0 && bytes[cursor - 1] == b'>' {
            let mut depth = 0usize;
            while cursor > 0 {
                let byte = bytes[cursor - 1];
                cursor -= 1;
                if byte == b'>' {
                    depth += 1;
                } else if byte == b'<' {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
            }
            if cursor == 0 {
                return None;
            }
            while cursor > 0 && bytes[cursor - 1].is_ascii_whitespace() {
                cursor -= 1;
            }
        }
        let end = cursor;
        while cursor > 0
            && (bytes[cursor - 1].is_ascii_alphanumeric()
                || bytes[cursor - 1] == b'_'
                || bytes[cursor - 1] == b'$')
        {
            cursor -= 1;
        }
        if cursor == end {
            return None;
        }
        Some((
            self.intern(&self.source[cursor..end]),
            self.utf16_of(cursor),
        ))
    }

    /// The import prefix a class segment's head is qualified by
    /// (`p.C(`/`p.C.n(` → `p`): the identifier before the segment's `.`,
    /// ASCII-walked like [`Self::slot_creation_class`]. `None` for a bare
    /// head — the `.` the walk finds could also be `..`/`?.`, neither of
    /// which prefixes a type, so the left-guard rejects those spellings.
    fn slot_head_prefix(&self, offset: u32) -> Option<StringId> {
        let bytes = self.source.as_bytes();
        let mut cursor = self.byte_of(offset);
        while cursor > 0 && bytes[cursor - 1].is_ascii_whitespace() {
            cursor -= 1;
        }
        if cursor < 2 || bytes[cursor - 1] != b'.' {
            return None;
        }
        // A cascade (`..`) or null-aware (`?.`) dot is no prefix edge.
        if bytes[cursor - 2] == b'.' || bytes[cursor - 2] == b'?' {
            return None;
        }
        cursor -= 1;
        while cursor > 0 && bytes[cursor - 1].is_ascii_whitespace() {
            cursor -= 1;
        }
        let end = cursor;
        while cursor > 0
            && (bytes[cursor - 1].is_ascii_alphanumeric()
                || bytes[cursor - 1] == b'_'
                || bytes[cursor - 1] == b'$')
        {
            cursor -= 1;
        }
        if cursor == end {
            return None;
        }
        Some(self.intern(&self.source[cursor..end]))
    }

    /// The written `<...>` group after a class segment's head
    /// (`F0<int>(` → `[int]`): the group's balanced bytes sit between the
    /// head's end and the next non-trivia byte — `.`/`(` either way the
    /// caller's latch already proved. `None` when no group was written or
    /// the type-argument parse cannot spell it (an unregistered head, a
    /// function-typed argument — the under-report posture).
    fn slot_head_group_args(&self, head: StringId, offset: u32) -> Option<Vec<Ty>> {
        // `call_type_args`/`completed_group` carry the headed group's
        // parsed arguments keyed at the segment's own offset — the same
        // read `receiver_group_args` runs for a tear-off operand; only
        // the byte-walk below covers a group neither latch saw.
        if let Some((_, entry)) = self.call_type_args.iter().find(|(at, _)| *at == offset) {
            return Some(entry.tys.to_vec());
        }
        if let Some((_, group_offset, args)) = &self.completed_group
            && *group_offset == offset
        {
            return Some(args.clone());
        }
        let bytes = self.source.as_bytes();
        let head_len = self.strings.resolve(head).encode_utf16().count() as u32;
        let mut cursor = self.byte_of(offset + head_len);
        while cursor < bytes.len() && bytes[cursor].is_ascii_whitespace() {
            cursor += 1;
        }
        if cursor >= bytes.len() || bytes[cursor] != b'<' {
            return None;
        }
        let mut depth = 0usize;
        while cursor < bytes.len() {
            match bytes[cursor] {
                b'<' => depth += 1,
                b'>' => {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                _ => {}
            }
            cursor += 1;
        }
        if depth != 0 || cursor >= bytes.len() {
            return None;
        }
        let left = Token {
            id: 0,
            range: SourceRange {
                offset: self.utf16_of(cursor + 1),
                length: 1,
            },
        };
        self.literal_group_args(left)
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
        // (iv) The enclosing executable's return position runs first: a
        // literal nested in an outer literal's `=>` body opens at the SAME
        // row mark as the initializer that owns the outer literal (the
        // outer's deferred rows never advanced the count), so the
        // declaration channel below would steal it and re-bind the
        // declaration face — `F<int> g = () => (x) => …` would hand the
        // inner literal `F<int>`'s whole instantiated signature instead of
        // the outer context's return. Position beats provenance upstream
        // (`inferLocalFunction` reads the literal's AST position, never
        // the enclosing declaration's): whenever a return or arrow-body
        // mark matches, the enclosing executable is the literal's real
        // host.
        // The mark holds for arrow AND block bodies (the body's own
        // returns re-arm `return_mark` inside the literal, where the
        // capture already latched the context — the false direction the
        // old mark-equality read could not separate). The `=>`-body arm
        // (`body_mark` latches where the body's expression begins) is the
        // same position spelled without `return`: `f() => (x) => …` —
        // gated on an enclosing frame's brace being absent (any executable
        // — declared OR a literal — the literal's own frame is skipped:
        // its `=>` body never opens a brace but its context is what this
        // call is still deciding), so a block body's first-statement
        // literal (`f() { (x) => …; }`, `() { (x) => …; }`) never takes
        // the channel.
        let at_return = self.return_mark == Some(mark);
        let at_arrow_body = self.body_mark == Some(mark)
            && self.frames.iter().rev().skip(1).any(|f| match &f.kind {
                FrameKind::Executable { body_brace, .. } => body_brace.is_none(),
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
        // (i) The declaration initializer: the literal opened exactly at
        // the initializer's mark (a deeper literal inside it opened after
        // a callee's row and misses). `Function2<int, String>` folds to
        // `String Function(int)` — the alias the raw `pending_decl_ty`
        // misses.
        if self.init_mark == Some(mark)
            && let Some(ty) = self.pending_decl_ty
        {
            // A bare generic alias head (`Foo f = <T>(T x) {}` over
            // `typedef Foo<S> = S Function<T>(T x)`): the fold below hands
            // the literal the raw right-hand side, the alias's own
            // parameters riding unsubstituted (`S Function(T)` — the face
            // every downstream diagnostic then displays). The oracle opens
            // the literal under the instantiate-to-bounds instantiation
            // (`Foo<dynamic>` = `dynamic Function<T>(T)`,
            // `instantiateToBounds`, type_system.dart:719-731), so the
            // typedef face answers it whole first. A `?`-spelled use keeps
            // the same instantiation — the declared nullability joins onto
            // the substituted signature (`Foo?` reads `(dynamic
            // Function<T>(T))?`) exactly like the unchanged fold's own
            // `nullable |=` joins the unfolded right-hand side. A head the
            // face cannot instantiate (a non-alias, a poisoned capture)
            // falls through to that fold.
            if let Ty::Named {
                name,
                nullable,
                args: ArgListId::EMPTY,
            } = ty
                && let Some(ctx) = self.closure_typedef_function_ty(name, ArgListId::EMPTY)
            {
                let ctx = if nullable {
                    match ctx {
                        Ty::Function { shape, .. } => Ty::Function {
                            shape,
                            nullable: true,
                        },
                        other => other,
                    }
                } else {
                    ctx
                };
                return (Some(ctx), false);
            }
            let folded = self.fold_alias_ty(ty);
            if let Some(ctx) = self.function_ty_of(folded) {
                return (Some(ctx), false);
            }
            // The old-style typedef's element-model face: `typedef T
            // Function2<S, T>(S x)` spells its `Ty::Function` RHS through
            // `decl.var_ty`, with the written arguments pairing through
            // the shape's own `type_params_named` — `instantiate_alias`'s
            // `type_param_names` gate misses this spelling entirely. The
            // use-site `?` joins the substituted signature the same way
            // the bare-use arm above carries it.
            if let Ty::Named {
                name,
                nullable,
                args,
            } = folded
                && args != ArgListId::EMPTY
                && let Some(ctx) = self.closure_typedef_function_ty(name, args)
            {
                let ctx = if nullable {
                    match ctx {
                        Ty::Function { shape, .. } => Ty::Function {
                            shape,
                            nullable: true,
                        },
                        other => other,
                    }
                } else {
                    ctx
                };
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
        self.arg_slot_ty_ctx()
            .and_then(|ty| self.function_ty_of(ty))
    }

    /// The open argument slot's own context type — the pending argument's
    /// declared formal folded and inference-substituted exactly like
    /// [`Self::argument_ctx_at`] resolves it, but returned raw (the
    /// collection literal's channel reads `List<int>` itself, never a
    /// function shape). `None` when no argument is open or the callee
    /// answers no formal for the slot — including the unsolved-parameter
    /// gate (a bare `T` is no context).
    pub(crate) fn arg_slot_ty_ctx(&mut self) -> Option<Ty> {
        let (slot, callee_row, enum_const, unnamed_head, named_head) = {
            let level = self.arg_levels.last()?;
            (
                match level.pending_name {
                    Some(label) => SchemaSlot::Named(label),
                    None => SchemaSlot::Positional(
                        level
                            .args
                            .iter()
                            .filter(|(label, ..)| label.is_none())
                            .count(),
                    ),
                },
                level.callee_row,
                level.enum_const,
                level.unnamed_head,
                level.named_head,
            )
        };
        // The creation-head lane: the callee transport drops the row for
        // a keyword creation (`new F0([...])`) and a written-group head
        // (`F0<int>(`, `F0<int>.named(`), but the level's own latches
        // still spell it — `unnamed_head` is the class name whose
        // constructor table keys on itself, `named_head` is the `.named`
        // selector whose class segment sits before the dot.
        let (recv, callee_name, callee_row, head_formals) = match callee_row {
            Some(callee_row) => {
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
                (recv, callee.name, Some(callee_row), None)
            }
            None if enum_const => return None,
            None => match (unnamed_head, named_head) {
                (Some((head, range)), _) => (
                    Receiver::Unknown,
                    head,
                    None,
                    self.slot_creation_formals(head, range, false),
                ),
                (None, Some((selector, range))) => (
                    Receiver::Unknown,
                    selector,
                    None,
                    self.slot_creation_formals(selector, range, true),
                ),
                (None, None) => return None,
            },
        };
        // The slot's own declared type — the schema function the literal
        // matches. The element formals answer first; a function-typed
        // variable callee's captured signature closes (the same two
        // lanes [`Self::closure_context_schema`] opens). A creation head
        // reads the selected constructor's table (`new F0([...])` types
        // the literal's context from `F0(List<int> a)`); the written
        // group's type arguments instantiate the class's own parameters
        // there (`F0<int>([...])` reads `List<int>`, never `List<T>`).
        let slot_formals = head_formals.or_else(|| self.callee_formals(&recv, callee_name));
        let slot_ty = match slot {
            SchemaSlot::Positional(index) => slot_formals
                .as_ref()
                .and_then(|f| f.positional.get(index).map(|f| f.ty)),
            SchemaSlot::Named(label) => slot_formals
                .as_ref()
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
            return Some(folded);
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
        let inst = self.closure_schema_instantiate(slot_ty, &recv, callee_name, callee_row);
        match inst {
            Some(ty) => Some(ty),
            None => {
                let has_open_param = self
                    .closure_callee_params(&recv, callee_name)
                    .map(|(params, ..)| {
                        let names: Vec<StringId> = params.iter().map(|(name, _)| *name).collect();
                        !names.is_empty() && self.ty_names_any(slot_ty, &names)
                    })
                    .unwrap_or(false);
                if has_open_param { None } else { Some(slot_ty) }
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

    /// The instantiated function type a typedef's use spells as a closure
    /// context — `Function2<int, String>` for the old-style
    /// `typedef T Function2<S, T>(S x)` written with arguments, and
    /// `dynamic Function<T>(T)` for the modern `typedef Foo<S> = S
    /// Function<T>(T x)` used bare — the channel-(i) faces
    /// [`Self::fold_alias_ty`] answers raw. The written face pairs the
    /// arguments against the shape's own `type_params_named` (the element
    /// model's only whole record for the old spelling: the collector
    /// synthesizes the RHS `Ty::Function` into `decl.var_ty`, while
    /// `decl.type_param_names` stays empty — the count-only payload
    /// `instantiate_alias` refuses). The bare face substitutes the ALIAS's
    /// parameters (`decl.type_param_names`) to their instantiate-to-bounds
    /// defaults and keeps the right-hand side's own `<T>` group.
    /// `None` for a non-alias head, a non-function `var_ty`, a poisoned
    /// bounds capture, or a partially written argument list.
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
        // A written MODERN-alias use (`Foo<int>` over `typedef Foo<S> = S
        // Function<T>(T x)`): the arguments instantiate the ALIAS's
        // parameters — the declaration's own spellings — so `S := int`
        // substitutes into the right-hand shape while the function's own
        // `<T>` group survives verbatim (`Foo<int>` = `int Function<T>(T)`
        // — the element-model type the oracle hands `inferLocalFunction`).
        // Zipping the group's parameters instead would bind `T := int`,
        // drop the group, and leave `S` raw (the pre-fix `'S'` row). The
        // old-style spelling (`typedef List<X> Function(int) F<X>()`)
        // records no `type_param_names` — its alias parameters ARE the
        // right-hand side's `type_params_named` group, so the written
        // list pairs against the group directly and consumes it.
        //
        // A bare generic use (`Foo f`): the oracle resolves the raw head to
        // the instantiate-to-bounds instantiation (`Foo<dynamic>` —
        // `instantiateToBounds`, type_system.dart:719-731), so each alias
        // parameter takes its bound default under the same preserved
        // group. A poisoned capture (the snapshot answers `None`) and a
        // partially written list (the wrong-arity face another layer
        // reports) keep the bail's silence.
        let (map, own_group) = if !written.is_empty() {
            if !decl.type_param_names.is_empty() && written.len() == decl.type_param_names.len() {
                let map: FxHashMap<StringId, Ty> =
                    decl.type_param_names.iter().copied().zip(written).collect();
                (map, Some(type_params_named))
            } else if decl.type_param_names.is_empty() && written.len() == type_params_named.len() {
                let map: FxHashMap<StringId, Ty> = type_params_named
                    .iter()
                    .map(|(param, _)| *param)
                    .zip(written)
                    .collect();
                (map, None)
            } else {
                return None;
            }
        } else if written.is_empty() {
            let defaults = self.bounds.itb_default_args(self.strings, name)?;
            let spellings = &decl.type_param_names;
            if defaults.len() != spellings.len() {
                return None;
            }
            let map: FxHashMap<StringId, Ty> = spellings
                .iter()
                .copied()
                .zip(defaults.iter().map(|tree| self.tree_ty(tree)))
                .collect();
            (map, Some(type_params_named))
        } else {
            return None;
        };
        let ret = self.closure_subst_param_ty(return_ty, &map);
        let positional: Vec<Ty> = positional
            .iter()
            .map(|ty| self.closure_subst_param_ty(*ty, &map))
            .collect();
        let named: Vec<(StringId, Ty)> = named
            .iter()
            .map(|(n, ty)| (*n, self.closure_subst_param_ty(*ty, &map)))
            .collect();
        // The written path's zip consumed the group — the answer is a
        // concrete signature with no `type_params` of its own. The bare
        // path substituted the ALIAS's parameters instead, so the
        // right-hand side's own `<T>` group survives verbatim
        // (`Foo<dynamic>` = `dynamic Function<T>(T)` — the element-model
        // type the oracle hands `inferLocalFunction`).
        let mut substituted = self.boundary.args.borrow_mut();
        let ty = match own_group {
            Some(group) => substituted.function_generic(&group, ret, &positional, &named, nullable),
            None => substituted.function(0, ret, &positional, &named, nullable),
        };
        drop(substituted);
        Some(ty)
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
                // The signature's own binder shadows the use-site map
                // (`typedef Foo<T> = T Function<T>(T)` — every `T` leaf
                // inside the right-hand signature is bound by the `<T>`
                // group, never by the alias's own `T`, so `Foo<int>` reads
                // `T Function<T>(T)` verbatim — the oracle's
                // `element.dart` `instantiate` substitutes only the free
                // spellings). A count-only group names nothing: opaque —
                // the under-report direction, matching `ty_fold`'s walk. A
                // bound subtree (`<T extends List<T>>`) sits inside the
                // same scope — its `T` leaves are the binder's, so the
                // shadowed map covers the bounds too.
                let shadowed: Vec<StringId> = names.iter().map(|(n, _)| *n).collect();
                if type_params > 0 && shadowed.is_empty() {
                    return ty;
                }
                let inner = if shadowed.is_empty() {
                    None
                } else {
                    let mut inner = map.clone();
                    inner.retain(|name, _| !shadowed.contains(name));
                    Some(inner)
                };
                let map = inner.as_ref().unwrap_or(map);
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
                // Own the return face before the re-intern guard:
                // `closure_subst_param_ty` re-enters the pool in every
                // compound arm (`pool()`'s borrow), which a live `RefMut`
                // here panics against (#3202's frame pair — the same
                // discipline as `subst_ty`'s function leg and #2992's
                // alias-fold rebuild).
                let return_ty = self.closure_subst_param_ty(return_ty, map);
                if names.is_empty() && type_params > 0 {
                    self.boundary.args.borrow_mut().function(
                        type_params,
                        return_ty,
                        &positional,
                        &named,
                        nullable,
                    )
                } else {
                    self.boundary.args.borrow_mut().function_generic(
                        &names,
                        return_ty,
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
            // the same column the diagnose-side fn capture carries. A
            // count-only capture (a projection that kept the arity but not
            // the spellings) still interns the arity — the render drops
            // only the unspellable group, never the quantifier itself.
            let type_params: Vec<(StringId, Option<Ty>)> = fn_tree
                .type_params_named
                .iter()
                .map(|(name, bound)| (*name, bound.as_ref().map(|b| self.tree_ty(b))))
                .collect();
            return if type_params.is_empty() && fn_tree.type_params > 0 {
                self.boundary.args.borrow_mut().function_required(
                    fn_tree.type_params,
                    fn_tree.required_positional,
                    ret,
                    &positional,
                    &named,
                    tree.nullable,
                )
            } else {
                self.boundary.args.borrow_mut().function_generic_required(
                    &type_params,
                    fn_tree.required_positional,
                    ret,
                    &positional,
                    &named,
                    tree.nullable,
                )
            };
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
    ///
    /// The armed type slot IS cleared, and that is no contradiction: it is
    /// the one-shot latch a declaration's own return annotation arms and
    /// its name event consumes ([`Engine::latch_return_type`]), so whatever
    /// a previous declaration left on it belongs to that declaration. An
    /// untyped head (`main() { … }`) must read the element's own absent
    /// annotation — upstream's untyped return is `dynamic`, never a
    /// non-nullable completion target — and not the last type some earlier
    /// declaration completed: `typedef int Foo(bool a, [String b]);` arms a
    /// whole `int Function(bool, String)` face on the slot, which the
    /// next untyped declaration would otherwise latch as its own return and
    /// report `body_might_complete_normally` against (#3514). The
    /// unarmed latch reads [`Ty::UNKNOWN`], the completion walk's silent
    /// stand-in for a declaration that spells no return type.
    pub(crate) fn push_executable_frame(&mut self, instance: bool) {
        self.armed_ty = None;
        self.armed_ty_unresolved = false;
        self.armed_ty_span = None;
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
    /// the async body's *inferred* return — `Future<flatten(clamp)>` over
    /// the body's own returns, never the imposed face verbatim
    /// (body_inference_context.dart:132-153 `computeInferredReturnType`,
    /// :259-261 `_contextTypeForImposed` supplying the
    /// `FutureOr<futureValueType(imposed)>` context, :206-222
    /// `_clampToContextType`). The clamp answers `void` first, then the
    /// body's returned type when it fits the context, and only then the
    /// context itself — so the two arms whose destination is independent
    /// of the context are the ones this name-level walk can ground:
    ///
    /// - `futureValueType(R)` is `void` (`R` spells `void`/`Future<void>`/
    ///   `FutureOr<void>`): the clamp short-circuits to `void`, and the
    ///   destination is `Future<void>` — `void Function() test = () async
    ///   { return v; }` reports `'Future<void>'`, never the raw `void`.
    /// - `futureValueType(R)` is `Object?` (`R` names no `Future`/
    ///   `FutureOr` head at all, or names one whose argument is
    ///   `Object?`/a type parameter): every flattened return type is a
    ///   subtype of `FutureOr<Object?>`, so the clamp answers the body's
    ///   own type and the per-return ask degenerates to `S <: S`. No
    ///   return row can fire; `Future<void>` is that silent face's
    ///   spelling here (`Object Function() f = () async { return v; }`
    ///   is clean upstream).
    ///
    /// A concrete `Future<X>`/`FutureOr<X>` context keeps `Future<X>`: the
    /// clamp either answers `X` (identical destination) or the body's own
    /// type, on which the `X` ask agrees (`Future<int> Function()` with a
    /// `void` return reports `'Future<int>'`, `int Function()` with the
    /// same body stays silent — `Object?` is the fallback that absorbs
    /// it). Formals bind before this event fires, so the rewrite never
    /// disturbs the per-formal inference arm.
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
        // The clamp's grounded arm (`_clampToContextType`): an implied
        // `Object?` value type is the fallback face of a context that is
        // not a `Future`/`FutureOr` at all — the body's own return type
        // wins there, so no return row survives and the wrap carries the
        // silent `void` face instead of the fallback head.
        let flat = if self.future_value_type(return_ty) == self.object_question() {
            Ty::Void
        } else {
            engine_core::flatten_ty(self.strings, &self.boundary.pool(), return_ty)
        };
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
        // The body's statements are complete at the executable's end
        // event — verdict the shorthand heads the initializer pass never
        // reached while the frame's return face is still live (#3568).
        self.sweep_body_shorthands();
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
        // The SYNC completion question folds a written-but-unresolved
        // head to the silent face: upstream's `returnType` is
        // `InvalidType` there (`isNullable` admits it, so the verdict's
        // `FutureOr`-base arm swallows it without a row) — the same
        // stand-in an absent annotation already latches. The async
        // question keeps the written head instead: `InvalidType` is a
        // top face upstream (legal through `Future<Never> <: T`), and
        // the `FutureOr<futureValueType(S)>` context reads it through
        // `futureValueType`'s `Object?` arm — folding to `Unknown` here
        // would skip a row the oracle still reports.
        let declared = if *is_async {
            declared
        } else {
            self.invalid_return_fold(declared)
        };
        self.insert_body_completion(end.range, declared, *body_brace, *is_async, anchor);
    }

    /// The sync completion question's `InvalidType` stand-in
    /// (`checkForBodyMayCompleteNormally`'s `isNullable` admission,
    /// resolver.dart:584-603): a `Named` head that resolves to no type in
    /// scope — an undefined name, a non-type element, a member of the
    /// enclosing container — is upstream `InvalidType` and asks no
    /// nullability question. Folding it to `Ty::UNKNOWN` (the same
    /// stand-in an omitted return annotation latches) silences both
    /// twins uniformly; the nullable arm asks no scope question of its
    /// own, so an unresolved `D?` otherwise reported the warning the
    /// oracle never produces. A head that resolves — primitives, frame
    /// type parameters, real declarations — keeps its written face.
    fn invalid_return_fold(&self, ty: Ty) -> Ty {
        match ty {
            Ty::Named { name, .. } if !self.named_type_is_type(name) => Ty::UNKNOWN,
            _ => ty,
        }
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
        // A local function's frame releases at `pop_frame` without a
        // `stage_body_completion` call — the shorthand sweep rides the
        // same close so `case`/argument/`==` heads inside the body
        // verdict (#3568). The popped frame's own `ret` is already
        // gone; `return`-positioned heads keep the context-free arm.
        self.sweep_body_shorthands();
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
                t.utf16_offset == brace.offset && t.kind == dartq_scanner::TokenKind::OpenCurly
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
        // The same `InvalidType` fold [`Engine::stage_body_completion`]
        // applies: only the sync question (the async arm reads the
        // written head through `futureValueType`'s `Object?` face).
        let declared = if is_async {
            declared
        } else {
            self.invalid_return_fold(declared)
        };
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
            } if name == self.intern("FutureOr") => {
                if !self.named_type_is_type(name) && !*is_async {
                    return None;
                }
                // `isPotentiallyNonNullable` splits the twins — the union's
                // `isNullable` answer delegates to the argument's own
                // nullability (type_system.dart:1227-1230), so
                // `FutureOr<FutureOr<int>?>` reads `FutureOr<int>?`'s `?`
                // (the nullable twin) while `FutureOr<FutureOr<int>>`
                // stays potentially non-nullable (the ERROR). A base that
                // escaped the first arm's silent faces makes the nullable
                // answer the WARNING twin — never the silent face.
                if self.completion_nullable(ret) {
                    SemanticCode::BodyMightCompleteNormallyNullable
                } else {
                    SemanticCode::BodyMightCompleteNormally
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
        // An argument-bearing head answers through resolved arguments:
        // `returnType?.type` upstream is the element-typed face, where an
        // argument that names no type is `InvalidType` and an alias use
        // keeps its spelled head while answering through the right-hand
        // side. The fold's `InvalidType` arm writes [`Ty::Unknown`] — the
        // verdict's silent-base face — because the staged row outlives
        // this declaration's scope: the teardown walk can no longer tell
        // `FutureOr<NonExistent>` from `FutureOr<R>` once the type
        // parameters pop, so the unresolved spelling must be decided here.
        if let Ty::Named {
            name,
            nullable,
            args,
        } = folded
            && args != ArgListId::EMPTY
        {
            // The element list must copy out before the fold — each
            // argument's rewrite can intern into the same pool, and a live
            // `Ref` across `borrow_mut` is a runtime panic, not a borrow
            // error the compiler sees.
            let source_args = {
                let pool = self.boundary.pool();
                pool.get(args).to_vec()
            };
            let rewritten: Vec<Ty> = source_args
                .iter()
                .map(|arg| self.completion_arg_fold(*arg))
                .collect();
            return Ty::Named {
                name,
                nullable,
                args: self.boundary.intern_list(&rewritten),
            };
        }
        self.completion_alias_face(folded)
    }

    /// The terminal face of the shared alias fold — the right-hand sides
    /// [`Engine::fold_alias_ty`] deliberately keeps written (`Null`/`void`/
    /// `Never`/`Unknown` are "not the modeled face" there) but the
    /// completion verdict reads resolved (`typedef Nothing = void; Nothing
    /// f() {}` answers `void`, never the alias head). A bare `Named` whose
    /// captured right-hand side is one of those terminals answers the
    /// terminal; every other folded shape keeps its own face. Runs at the
    /// verdict join too — `alias_rhs` reads declaration tables that
    /// outlive the declaring frame's scope (a type parameter's `Local`
    /// decl is no `TypeAlias`, so `R` keeps its written head either way).
    fn completion_alias_face(&self, ty: Ty) -> Ty {
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

    /// One resolved type argument of the completion walk's staged declared
    /// return: the alias fold answers the argument's own right-hand side
    /// (`FutureOr<Nothing>`'s argument reads `void`, the verdict's silent
    /// face; `FutureOr<NF>`'s reads `FutureOr<void>`), and a `Named` head
    /// that resolves to no type in scope is the oracle's `InvalidType` —
    /// [`Ty::Unknown`]. A type parameter, a primitive, and every resolvable
    /// declaration keep the written head so the diagnostic's display text
    /// stays spelled (`'FutureOr<NInt>'`, never `'FutureOr<int>'`). Runs at
    /// staging while the declaring frame's type parameters still bind.
    fn completion_arg_fold(&self, ty: Ty) -> Ty {
        let Ty::Named {
            name,
            nullable,
            args,
        } = ty
        else {
            return ty;
        };
        if !self.named_type_is_type(name) {
            return Ty::UNKNOWN;
        }
        if args == ArgListId::EMPTY {
            return ty;
        }
        // The element list copies out before the fold for the same
        // `Ref`-across-`borrow_mut` reason [`Self::completion_declared_ty`]
        // notes.
        let source_args = {
            let pool = self.boundary.pool();
            pool.get(args).to_vec()
        };
        let rewritten: Vec<Ty> = source_args
            .iter()
            .map(|arg| self.completion_arg_fold(*arg))
            .collect();
        Ty::Named {
            name,
            nullable,
            args: self.boundary.intern_list(&rewritten),
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

    /// `TypeSystem::futureOrBase` at name level (type_system.dart:473-482):
    /// a `FutureOr<T>` head with exactly one type argument unwraps to `T`
    /// and the question asks again — the base recurses through nested
    /// `FutureOr` heads, so `FutureOr<FutureOr<void>>`'s base is `void` and
    /// `FutureOr<FutureOr<int>>`'s is `int`. The written spelling resolves
    /// before the official asks: each hop folds the argument's own alias
    /// (`FutureOr<Nothing>`'s `void` right-hand side IS the base upstream,
    /// and `FutureOr<NF>` over `typedef NF = FutureOr<void>` bases on the
    /// unfolded union), while an argument that names no type arrives
    /// already spelled [`Ty::Unknown`] — the staged `InvalidType` stand-in
    /// [`Engine::completion_declared_ty`] writes. Every other type is its
    /// own base (the written `?` never strips — `FutureOr<int>?`'s base is
    /// `int`). A bare `FutureOr` spells `FutureOr<dynamic>` — its base is
    /// `dynamic`, one of the silent faces.
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
        let inner = {
            let pool = self.boundary.pool();
            let list = pool.get(args);
            if list.len() != 1 {
                return ret;
            }
            list[0]
        };
        self.future_or_base(self.completion_alias_face(inner))
    }

    /// `TypeSystem::isNullable` (type_system.dart:1216-1233) over the
    /// completion walk's resolved face — the nullability question the
    /// `body_might_complete_normally` verdict splits on. The top faces
    /// (`dynamic`, `void`, `Null`, and the [`Ty::Unknown`] `InvalidType`
    /// stand-in the staging fold writes) are nullable; a `?` suffix is
    /// nullable; a `FutureOr<T>` head delegates to its argument — through
    /// the argument's own resolution, so `FutureOr<Nothing>`'s `void`
    /// right-hand side and `FutureOr<NF>`'s unfolded union answer instead
    /// of the spelled name. Every other resolved head is non-nullable — a
    /// `Named` here is a real type or a type parameter, never an
    /// unresolved spelling (the staging fold already turned those into
    /// [`Ty::Unknown`], which the top-face arm answers before scope is
    /// consulted).
    fn completion_nullable(&self, ty: Ty) -> bool {
        match self.completion_alias_face(ty) {
            Ty::Dynamic | Ty::Void | Ty::Null | Ty::Unknown => true,
            Ty::Named {
                name,
                nullable,
                args,
            } => {
                if nullable {
                    return true;
                }
                if name != self.intern("FutureOr") {
                    return false;
                }
                // `FutureOr<T>` delegates to `T` (`isDartAsyncFutureOr`
                // arm); a bare `FutureOr` spells `FutureOr<dynamic>` —
                // nullable. Any other arity is a malformed list the
                // conservative non-nullable face answers.
                if args == ArgListId::EMPTY {
                    return true;
                }
                let arg = {
                    let pool = self.boundary.pool();
                    let list = pool.get(args);
                    (list.len() == 1).then_some(list[0])
                };
                arg.is_some_and(|arg| self.completion_nullable(arg))
            }
            Ty::Record { nullable, .. } | Ty::Function { nullable, .. } => nullable,
            Ty::Never => false,
        }
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
        // An operator head latches `ret` at `begin_method`, BEFORE its
        // return annotation parses (the parser commits to the method at the
        // `operator` keyword; the latch drops the armed slot, an earlier
        // construct's type): every completion that starts before the
        // declaring name is that annotation's, and the outermost head
        // completes last (`List<int> operator +` completes `int`, then the
        // re-fired `List`), so each one overwrites. Every other executable
        // latches at its name event, after its annotation completed — no
        // later completion (a type-parameter bound, a formal, a body local
        // past a nested closure's frame) starts before the name, so an
        // unannotated `f<T extends num>(T a) {}` keeps its `dynamic` face.
        // A record field's or a function type's inner completion is no
        // annotation head: their closes arm the slot without reaching this
        // event, so such an operator return keeps the `dynamic` face.
        let Some(ty) = self.armed_ty else {
            return;
        };
        if !self.record_ty_stack.is_empty() || !self.fn_type_capture.is_empty() {
            return;
        }
        let name_offset = match self.frames.last().map(|frame| &frame.kind) {
            Some(FrameKind::Executable { ret: Some(ret), .. }) => ret.name_span.offset,
            _ => return,
        };
        if span.offset >= name_offset {
            return;
        }
        // A re-fired parameterized head's written arguments join the armed
        // slot only after this event (`handle_type`'s #611 join): the
        // group latch keyed at this head's offset carries them.
        let ty = match (ty, &self.completed_group) {
            (
                Ty::Named {
                    name,
                    nullable,
                    args: ArgListId::EMPTY,
                },
                Some((_, offset, args)),
            ) if *offset == span.offset => Ty::Named {
                name,
                nullable,
                args: self.boundary.intern_list(args),
            },
            (ty, _) => ty,
        };
        if let Some(frame) = self.frames.last_mut()
            && let FrameKind::Executable { ret: Some(ret), .. } = &mut frame.kind
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
    ///
    /// A wildcard type parameter (`f<_>`, `class C<_>`) enters neither
    /// list: `TypeParameterScope` skips the `_` element under the feature
    /// exactly like `LocalScope.add` does (upstream Dart SDK source,
    /// pkg/analyzer/lib/src/dart/element/scope.dart:944-960), so the
    /// parameter is an undefined name in every reading plane — its body
    /// reads fall through the lexical chain ([`Self::declares_nothing`]'s
    /// gate, the same one `add_local` applies). Below 3.7 the gate is off
    /// and `_` is an ordinary type parameter.
    pub(crate) fn push_type_parameter(&mut self, name: StringId) {
        if self.declares_nothing(name) {
            return;
        }
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

/// The #3116 `??`/`??=` dead-coalesce promotion pins: the staged
/// re-decision that anchors the dead face on the operand's flow-promoted
/// type — `local_hits`-keyed spans ride `coalesce_staged` to the
/// `flush_null_aware_staged` teardown join. `#[path]` resolves against
/// this file's directory — `diagnose/`.
#[cfg(test)]
#[path = "ticket_3116_tests.rs"]
mod ticket_3116_tests;

#[cfg(test)]
mod tests_3514 {
    //! The `body_might_complete_normally[_nullable]` return face (#3514,
    //! C5): the completion walk reads the element's OWN declared return
    //! type (upstream `checkForBodyMayCompleteNormally` reads
    //! `element.returnType`, type_analyzer.dart:639-704), never a face an
    //! earlier declaration left on the shared armed slot.
    //!
    //! The defect: `typedef int Foo(bool a, [String b]);` arms a whole
    //! `int Function(bool, String)` face on `Engine::armed_ty` at its
    //! right-hand side's close, and the next *untyped* head latched that
    //! face as its own return — an untyped function's return is `dynamic`
    //! upstream, no potentially-non-nullable completion target, so the
    //! oracle never reports it. The corpus fixtures this reproduces are
    //! the `function_subtype` family's `local0_test` (`main` reported as
    //! `int Function(bool, String)`), `local1_test` (`bool`) and
    //! `cast2_test` (a method reported as `void Function(int)`).
    //!
    //! Oracle provenance: `dart analyze` 3.13.7
    //! (`~/.dartq-oracle/3.13.7/dart-sdk/bin/dart analyze`) over the same
    //! fixture text; each expectation below is a live verdict, quoted at
    //! its test.

    use crate::diagnose::test_rig::{CORE_PRELUDE, report_ops};

    /// Every `body_might_complete_normally*` row as `(code, offset)`,
    /// source-ordered — the two completion codes are the only rows these
    /// pins decide, so the fixture's other rows (unused local functions,
    /// unresolved `print`) never enter the answer.
    fn completion_rows(source: &str) -> Vec<(&'static str, u32)> {
        let report = report_ops(source);
        let mut rows: Vec<(&'static str, u32)> = report
            .diagnostics
            .iter()
            .filter(|row| row.code.name.starts_with("body_might_complete_normally"))
            .map(|row| (row.code.name, row.range.offset))
            .collect();
        rows.sort_unstable();
        rows
    }

    /// The offset of the `nth` (0-based) occurrence of `needle` in the
    /// FIXTURE text, shifted past the prelude the rig mounts ahead of it.
    /// Every fixture here is ASCII, so byte and UTF-16 offsets coincide.
    fn at(source: &str, needle: &str, nth: usize) -> u32 {
        let found = source
            .match_indices(needle)
            .nth(nth)
            .unwrap_or_else(|| panic!("`{needle}` occurrence #{nth} missing"));
        found.0 as u32 + CORE_PRELUDE.len() as u32
    }

    /// A typedef's function-typed right-hand side arms the shared slot; the
    /// following untyped `main` must still read its own absent return
    /// (`dynamic`, silent), while the declared `int g()` keeps its row.
    ///
    /// Probed: exactly one row, `body_might_complete_normally` at `int g`'s
    /// name (`lib/a.dart:7:5`), none at `main`.
    #[test]
    fn untyped_main_latches_its_own_absent_return() {
        let source = "\
typedef int Foo(bool a, [String b]);\n\
\n\
main() {\n\
\x20 print(Foo);\n\
}\n\
\n\
int g() {}\n";
        assert_eq!(
            completion_rows(source),
            [("body_might_complete_normally", at(source, "g()", 0))],
            "{:#?}",
            completion_rows(source)
        );
    }

    /// The `cast2_test` shape: a top-level function-typed variable arms a
    /// `void Function(int)` face, and the class method that follows is
    /// untyped — its body's completion question reads `dynamic` and stays
    /// silent.
    ///
    /// Probed: no completion row at all ("No issues found" for the same
    /// text).
    #[test]
    fn untyped_method_after_a_function_typed_field_stays_silent() {
        let source = "\
void Function(int) face = (x) => x;\n\
\n\
class C {\n\
\x20 m() {}\n\
}\n";
        assert_eq!(completion_rows(source), Vec::new());
    }

    /// Control: a declared function-typed return is potentially
    /// non-nullable (`isPotentiallyNonNullable` admits no null), so its
    /// body keeps the ERROR twin — the fix must not silence the code.
    ///
    /// Probed: one row, `body_might_complete_normally` at `g`'s name
    /// (`lib/a.dart:1:20`): "The body might complete normally, causing
    /// 'null' to be returned, but the return type, 'void Function(int)',
    /// is a potentially non-nullable type."
    #[test]
    fn declared_function_typed_return_keeps_its_row() {
        let source = "void Function(int) g() {}\n";
        assert_eq!(
            completion_rows(source),
            [("body_might_complete_normally", at(source, "g()", 0))],
            "{:#?}",
            completion_rows(source)
        );
    }

    /// Control: a nullable declared return keeps the WARNING twin
    /// (`body_might_complete_normally_nullable`), whose span is the same
    /// declaring name.
    ///
    /// Probed: one row, `body_might_complete_normally_nullable` at `h`'s
    /// name (`lib/a.dart:1:6`): "This function has a nullable return type
    /// of 'int?', but ends without returning a value."
    #[test]
    fn nullable_declared_return_keeps_its_twin() {
        let source = "int? h() {}\n";
        assert_eq!(
            completion_rows(source),
            [(
                "body_might_complete_normally_nullable",
                at(source, "h()", 0)
            )],
            "{:#?}",
            completion_rows(source)
        );
    }
}

#[cfg(test)]
#[path = "ticket_3640_tests.rs"]
mod ticket_3640_tests;
