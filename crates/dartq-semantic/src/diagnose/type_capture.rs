use super::*;

impl<'a> Engine<'a> {
    /// Opens a type-variables window (`begin_type_variables`): the
    /// declaration header's `<T, S extends num>` is being parsed, and each
    /// variable's name token lands in it for the unbounded anchoring.
    pub(crate) fn open_type_variables(&mut self) {
        self.tv_window = Some(TvWindow::default());
    }

    /// Closes the window and latches its unbounded count with the group's
    /// closing offset (`end_type_variables`): the owning declaration's name
    /// precedes that offset, so a container head adopts the latch by token
    /// order alone.
    pub(crate) fn close_type_variables(&mut self, window_end: Token) {
        self.pending_tv = self
            .tv_window
            .take()
            .map(|window| (window.unbounded, window_end.range.offset));
    }

    /// #1314 FAM10: pairs the closed type-variable window with the latched
    /// declaring executable's name (`exec_bounds`). Each parameter's
    /// written bound maps through [`Engine::tree_ty`] — a poisoned bound
    /// keeps the name-only shape, recovery residue is never a fact. The
    /// token-order guard mirrors the container adoption: a declaring name
    /// precedes its own window, a foreign window's close sits after a
    /// later declaration's name and drops (the latch clears either way —
    /// keep-first, one window adopts once).
    pub(crate) fn pair_exec_window(&mut self, window_end: u32) {
        let Some((name, name_offset)) = self.pending_exec_name else {
            return;
        };
        self.pending_exec_name = None;
        if name_offset >= window_end {
            return;
        }
        let Some(window) = self.bounds.pending_window() else {
            return;
        };
        let window: Box<[(StringId, Option<Ty>)]> = window
            .iter()
            .map(|(param, bound, _)| (*param, bound.as_ref().map(|tree| self.tree_ty(tree))))
            .collect();
        // The method head routes apart so the callee-side substitution
        // faces — whose member-send inference sources its own parameters
        // from the member walk — never see a method entry race them
        // (their `exec_params.is_none()` guard would starve).
        if self.pending_exec_is_method {
            self.method_exec_bounds.insert(name, window);
        } else {
            self.exec_bounds.insert(name, window);
        }
    }

    /// #1314 FAM18: the group's close — the constructor-independent faces
    /// decide (`not_initialized_non_nullable_variable` for top-level
    /// variables and static fields; `final_not_initialized` for static
    /// finals — the top-level final spelling stays parser-owned, the CFE's
    /// shared `finalNotInitialized` face fires there); instance-field
    /// entries stay latched for the container's end.
    pub(crate) fn flush_init_variables(&mut self) {
        let mut index = 0usize;
        while index < self.pending_init_variables.len() {
            if self.pending_init_variables[index].container.is_some()
                && !self.pending_init_variables[index].is_static
            {
                index += 1;
                continue;
            }
            let check = self.pending_init_variables.remove(index);
            if check.initialized {
                continue;
            }
            // The member-model exemption (`checkElement`'s `abstract` /
            // `external` bits) reaches the static arm too: an
            // `external static` field (`@Native` family) is implemented
            // outside the class body and stays silent.
            if let Some(container) = check.container {
                let exempt = self
                    .index
                    .get(self.file)
                    .and_then(|elements| elements.container(container))
                    .and_then(|decl| decl.members.get(&check.name))
                    .is_some_and(|entry| entry.is_abstract || entry.is_external);
                if exempt {
                    continue;
                }
            }
            // The top-level final face is the parser's (the shared CFE
            // `finalNotInitialized` rider on the field parse).
            if check.is_final && !check.is_static {
                continue;
            }
            let name = self.strings.resolve(check.name).to_string();
            if check.is_final {
                self.emit(SemanticCode::FinalNotInitialized, check.range, &name, "");
            } else {
                self.emit(
                    SemanticCode::NotInitializedNonNullableVariable,
                    check.range,
                    &name,
                    "",
                );
            }
        }
    }

    /// #1314 FAM18: the container's end — its constructor set is complete,
    /// so the instance-field half decides. With no explicit generative
    /// constructor the declaration loop owns the face (per field in
    /// declaration order, the `abstract`/`external` bits exempt); with one,
    /// the `ConstructorFieldsVerifier` owns it: per generative constructor,
    /// the still-uninitialized finals' aggregated row first, then the
    /// non-final non-nullable rows — all anchored on the constructor's
    /// type-name-through-name span, the unnamed spelling's class-name
    /// token. A constructor-initializer assignment (`b = e` / `this.b = e`)
    /// releases the field's latch (`updateWithInitializers`). Named-selector
    /// sites and named factories stay out of the drive (the upstream
    /// visitor walks generative constructors; the site kind carries no
    /// factory bit, so a named generative constructor under-reports rather
    /// than a named factory over-reporting), and duplicate-named fields
    /// join no constructor row (upstream `checkElement`'s already-reported
    /// guard).
    pub(crate) fn flush_init_instance_fields(&mut self, container: StringId) {
        // The generative constructor set: every declared constructor rides a
        // `begin_constructor` site; factories register no head anchor (their
        // provisional is `None`) and stay out of the generative walks. The
        // head anchor is the row span — the class-name token the head starts
        // at — for named and unnamed heads alike (the oracle's
        // type-name-through-name anchor).
        let ctor_spans: Vec<(usize, SourceRange)> = self
            .decl_sites
            .iter()
            .enumerate()
            .filter(|(_, site)| {
                matches!(site.kind, DeclKind::Constructor { container: c } if c == container)
            })
            .filter_map(|(index, _)| {
                self.ctor_head_anchors.get(&index).map(|span| (index, *span))
            })
            // A redirecting generative ctor (`C.named(...) : this(...)`)
            // satisfies its finals through the redirect target — the
            // `final_not_initialized_constructor` verifier
            // (`ConstructorFieldsVerifier`) walks only non-redirecting
            // generative ctors. `redirect_edges` records the `this(...)`
            // shape's source ctor's site-index.
            .filter(|(index, _)| !self.ctor_redirects.contains(index))
            .collect();
        // Upstream `hasGenerativeConstructor` (error_verifier.dart:7114 —
        // `element.constructors.any(isGenerative && isOriginDeclaration)`)
        // counts EVERY declared generative constructor, redirecting ones
        // included: their presence routes instance fields to the
        // `ConstructorFieldsVerifier` face (which then silences each
        // redirecting ctor per-ctor via its own
        // `hasRedirectingConstructorInvocation` early return). A
        // redirecting-only set — the extension-type
        // `extension type S(int v) { S.fwd() : this(0); }` shape — keeps the
        // declaration face OFF: the primary formal binds the representation
        // field for the redirect target. `DeclKind::Constructor` sites are
        // generative ctor declarations only (factories register none).
        let has_generative_ctor = self.decl_sites.iter().any(|site| {
            !site.is_augment
                && matches!(site.kind, DeclKind::Constructor { container: c } if c == container)
        });
        // `member_duplicate_definition_verifier.dart`'s constructor pass:
        // per container, constructor names dedupe by selector — the unnamed
        // spelling keys as `new` (`member.name?.lexeme ?? 'new'`), a named
        // spelling by its selector — and every second-and-later declaration
        // of a key reports `duplicate_constructor` at its own name span.
        // The pass runs per class ELEMENT (one declaration): a duplicate
        // class name declares a fresh constructor set, so same-named
        // earlier declarations' sites stay out (the oracle reports
        // `duplicate_definition` on the class name and never a
        // cross-declaration `duplicate_constructor`). The current
        // declaration is the last container site of the name; its
        // constructors all start at or after its name token.
        {
            let decl_start = self
                .decl_sites
                .iter()
                .rev()
                .find(|site| matches!(site.kind, DeclKind::Container) && site.name == container)
                .map(|site| site.range.offset);
            let new_name = self.intern("new");
            // `errorRange` (`ConstructorDeclaration`): the type name through
            // the selector for a named head, the type name alone otherwise.
            let mut ctor_keys: Vec<(StringId, SourceRange)> = self
                .decl_sites
                .iter()
                .enumerate()
                .filter(|(_, site)| {
                    // An augment constructor merges into the introductory
                    // element's fragment chain (upstream
                    // `member.name?.lexeme ?? 'new'` keys per element) —
                    // never a second declaration of the selector.
                    !site.is_augment
                        && matches!(
                            site.kind,
                            DeclKind::Constructor { container: c } if c == container
                        )
                })
                .filter(|(_, site)| decl_start.is_none_or(|start| site.range.offset >= start))
                .map(|(index, site)| {
                    if site.name == container {
                        return (new_name, site.range);
                    }
                    let head = self
                        .ctor_head_anchors
                        .get(&index)
                        .or_else(|| self.factory_head_anchors.get(&index))
                        .filter(|head| head.offset < site.range.offset);
                    let range = head.map_or(site.range, |head| SourceRange {
                        offset: head.offset,
                        length: site.range.end() - head.offset,
                    });
                    (site.name, range)
                })
                .collect();
            ctor_keys.extend(
                self.unnamed_factory_heads
                    .iter()
                    .filter(|(c, _, range)| {
                        *c == container && decl_start.is_none_or(|start| range.offset >= start)
                    })
                    .map(|&(_, key, range)| (key, range)),
            );
            ctor_keys.sort_by_key(|(_, range)| range.offset);
            let mut seen_ctors: FxHashSet<StringId> = FxHashSet::default();
            for (key, range) in ctor_keys {
                if !seen_ctors.insert(key) {
                    let arg = if key == new_name {
                        ""
                    } else {
                        self.strings.resolve(key)
                    };
                    self.emit(SemanticCode::DuplicateConstructor, range, arg, "");
                }
            }
        }
        let mut duplicates: FxHashSet<StringId> = FxHashSet::default();
        if has_generative_ctor {
            let mut seen: FxHashSet<StringId> = FxHashSet::default();
            for check in &self.pending_init_variables {
                if check.container == Some(container) && !seen.insert(check.name) {
                    duplicates.insert(check.name);
                }
            }
        }
        // The declaration-order walk: finals and non-finals split here (the
        // spelling decision below needs the constructor set, the
        // `ConstructorFieldsVerifier`'s report order needs the split). The
        // per-constructor spare sets (`initInParameter` /
        // `updateWithInitializers`) release a field for their own
        // constructor only.
        let mut finals: Vec<(String, SourceRange)> = Vec::new();
        let mut non_finals: Vec<(String, SourceRange)> = Vec::new();
        let mut index = 0usize;
        while index < self.pending_init_variables.len() {
            if self.pending_init_variables[index].container != Some(container) {
                index += 1;
                continue;
            }
            let check = self.pending_init_variables.remove(index);
            if check.initialized || duplicates.contains(&check.name) {
                continue;
            }
            let exempt = self
                .index
                .get(self.file)
                .and_then(|elements| elements.container(container))
                .and_then(|decl| decl.members.get(&check.name))
                .is_some_and(|entry| entry.is_abstract || entry.is_external);
            if exempt {
                continue;
            }
            let name = self.strings.resolve(check.name).to_string();
            if check.is_final {
                finals.push((name, check.range));
            } else {
                non_finals.push((name, check.range));
            }
        }
        if ctor_spans.is_empty() {
            // No anchored generative constructor: the declaration walk owns
            // the face, per field at its name in declaration order
            // (error_verifier.dart's
            // `_checkForNotInitializedFieldDeclaration` variable loop) —
            // but only when no generative constructor is declared at all
            // (`hasGenerativeConstructor` upstream; redirecting ctors count
            // — `class C { final int f; C.fwd() : this(0); }` and the
            // extension-type `extension type S(int v) { S.fwd() : this(0); }`
            // report nothing, the verifier's redirect early-return silencing
            // each one).
            if !has_generative_ctor {
                // An extension type never reports here: upstream's
                // representation constructor is generative
                // (`element.constructors.any(isGenerative)`,
                // error_verifier.dart:7114), so the
                // `ConstructorFieldsVerifier` owns the container and the
                // primary's formal binds the rep — even when the only other
                // constructor is a factory (the
                // `extension type E._(int i) { external factory E(); }`
                // shape, whose factory registers no generative site). Our
                // site capture keeps the primary out of `decl_sites`, so
                // that shape lands the rep in this arm; the pending rep row
                // carries the primary formal's own name span, so the span
                // match identifies it without a text pass. Extension types
                // declare no other instance fields, so non-rep finals are
                // impossible here and the loop degenerates for them.
                let rep_spans: Vec<SourceRange> = self
                    .index
                    .get(self.file)
                    .and_then(|elements| elements.container(container))
                    .filter(|decl| decl.kind == ContainerKind::ExtensionType)
                    .map(|decl| {
                        decl.primary_formals
                            .iter()
                            .map(|rep| rep.name_range)
                            .collect()
                    })
                    .unwrap_or_default();
                for (name, range) in &finals {
                    if rep_spans.contains(range) {
                        continue;
                    }
                    self.emit(SemanticCode::FinalNotInitialized, *range, name, "");
                }
                for (name, range) in &non_finals {
                    self.emit(
                        SemanticCode::NotInitializedNonNullableInstanceField,
                        *range,
                        name,
                        "",
                    );
                }
            }
            return;
        }
        // The `ConstructorFieldsVerifier`'s report order, per constructor
        // span: the finals' aggregated row first (`reportNotInitializedFinal`,
        // names sorted), then one row per still-uninitialized non-final
        // non-nullable field (`reportNotInitializedNonNullable`, names
        // sorted).
        finals.sort_by(|a, b| a.0.cmp(&b.0));
        non_finals.sort_by(|a, b| a.0.cmp(&b.0));
        for (site, span) in &ctor_spans {
            // The implicit-super face: a generative constructor with no
            // explicit super satisfaction (a `super(...)` initializer or a
            // `super.x` formal) whose superclass's unnamed constructor
            // declares required positional parameters reports at the head
            // (`implicit_super_initializer_missing_arguments`,
            // diagnostic.g.dart:9734 — the official walk resolves the
            // implicit `super()` target's required-ness).
            if !self.ctor_super_init.contains(site) {
                let superclass = self
                    .index
                    .get(self.file)
                    .and_then(|elements| elements.container(container))
                    .and_then(|decl| decl.superclass);
                if let Some(superclass) = superclass {
                    let super_ctor_required = self
                        .index
                        .get(self.file)
                        .and_then(|elements| elements.container(superclass))
                        .and_then(|decl| decl.ctor_formals.get(&superclass))
                        .is_some_and(|formals| {
                            formals.positional.iter().any(|formal| formal.required)
                        });
                    if super_ctor_required {
                        let super_text = self.strings.resolve(superclass).to_string();
                        self.emit(
                            SemanticCode::ImplicitSuperInitializerMissingArguments,
                            *span,
                            &super_text,
                            "",
                        );
                    }
                }
            }
            let spares = |s: &Self, name: &str| {
                let id = s.intern(name);
                s.ctor_formals_init.contains(&(*site, id))
                    || s.ctor_init_writes.contains(&(*site, id))
            };
            let finals: Vec<&(String, SourceRange)> = finals
                .iter()
                .filter(|(name, _)| !spares(self, name))
                .collect();
            let non_finals: Vec<&(String, SourceRange)> = non_finals
                .iter()
                .filter(|(name, _)| !spares(self, name))
                .collect();
            match finals.as_slice() {
                [] => {}
                [(name, _)] => {
                    self.emit(
                        SemanticCode::FinalNotInitializedConstructor,
                        *span,
                        name,
                        "",
                    );
                }
                [(name1, _), (name2, _)] => {
                    self.emit(
                        SemanticCode::FinalNotInitializedConstructor,
                        *span,
                        name1,
                        name2,
                    );
                }
                _ => {
                    let rest = finals.len() - 2;
                    self.emit3(
                        SemanticCode::FinalNotInitializedConstructor,
                        *span,
                        &finals[0].0,
                        &finals[1].0,
                        &rest.to_string(),
                    );
                }
            }
            for (name, _) in &non_finals {
                self.emit(
                    SemanticCode::NotInitializedNonNullableInstanceField,
                    *span,
                    name,
                    "",
                );
            }
        }
    }

    /// #1314 FAM14: attributes the header-clause entries to their clause
    /// face for the resolver's context chain. The clause event fires after
    /// its own types parse, so the entries currently buffered are exactly
    /// that clause's — a snapshot, not a drain (the modifier use-site
    /// checks keep consuming the shared buffer's spans for anchors).
    pub(crate) fn clause_buffer_snapshot(&mut self, tag: ClauseTag) {
        let Some(deferred) = self.deferred_modifier.as_ref() else {
            return;
        };
        let offsets: Vec<u32> = deferred
            .spans
            .buffered
            .iter()
            .map(|span| span.offset)
            .collect();
        for offset in offsets {
            self.push_clause_miss(offset, tag);
        }
    }

    /// The with-clause entries join the deferred mixin-use face after the
    /// clause ledger has had its pass: a resolved entry the ledger never
    /// consumes (the clause faces fire only for unresolvable heads)
    /// re-decides at the teardown ([`Engine::finalize_mixin_uses`]).
    pub(crate) fn drain_with_clause_spans(&mut self) {
        if let Some(deferred) = self.deferred_modifier.as_mut() {
            self.pending_mixin_uses
                .extend(deferred.spans.buffered.drain(..));
        }
    }

    /// The recovered extends-after-implements tail: the parser keeps the
    /// tail clause's types (it reports only the ordering row), so whatever
    /// the header window still buffers at the extends commit is the tail
    /// clause's own entries — their unresolvable heads code
    /// `extends_non_class`, never the bare `undefined_class` (upstream
    /// Dart SDK source, pkg/analyzer/lib/src/dart/resolver/
    /// scope_analyzer.dart `_resolveType`, the `ExtendsClause` arm). A
    /// tail head that took the super slot directly (nothing buffered)
    /// rides the committed extends span instead.
    pub(crate) fn header_extends_tail_feed(&mut self) {
        // The extends head itself already joined the ledger at
        // `header_extends_commit` — only the buffered tail entries feed here.
        let offsets: Vec<u32> = match self.deferred_modifier.as_mut() {
            Some(deferred) if !deferred.spans.buffered.is_empty() => deferred
                .spans
                .buffered
                .drain(..)
                .map(|span| span.offset)
                .collect(),
            _ => return,
        };
        for offset in offsets {
            self.push_clause_miss(offset, ClauseTag::Extends);
        }
    }

    /// #1314 FAM14: the named mixin application's superclass head (the
    /// `class C = S with ...` slot, committed into the extends face at the
    /// `with` boundary) joins the ledger — or upgrades a pending `Extends`
    /// entry at the same offset, the class-with-clause face (the oracle's
    /// `declaration.withClause != null` selects `mixin_with_non_class_
    /// superclass` over `extends_non_class`, scope_analyzer.dart:1216-1221;
    /// a plain `class C extends Missing with M` resolves through the same
    /// mixin-application shape upstream).
    pub(crate) fn clause_alias_super(&mut self) {
        let Some(deferred) = self.deferred_modifier.as_ref() else {
            return;
        };
        let Some(span) = deferred.spans.extends else {
            return;
        };
        if let Some(entry) = self
            .clause_miss
            .iter_mut()
            .rev()
            .find(|(offset, _)| *offset == span.offset)
        {
            entry.1 = ClauseTag::AliasSuper;
        } else {
            self.push_clause_miss(span.offset, ClauseTag::AliasSuper);
        }
    }

    /// #1314 FAM14: feeds the just-claimed trailing entries of a counted
    /// clause (`implements`/`on`) — the claim moved them to the tail of
    /// the clause's own span list.
    pub(crate) fn clause_trailing_feed(&mut self, count: usize, tag: ClauseTag) {
        let Some(deferred) = self.deferred_modifier.as_ref() else {
            return;
        };
        let spans = match tag {
            ClauseTag::Implements => &deferred.spans.implements,
            ClauseTag::OnConstraint => &deferred.spans.on,
            _ => return,
        };
        let at = spans.len().saturating_sub(count);
        let offsets: Vec<u32> = spans[at..].iter().map(|span| span.offset).collect();
        for offset in offsets {
            self.push_clause_miss(offset, tag);
        }
    }

    /// Attaches the latched window to a container head, keyed by name
    /// (keep-first: a same-named duplicate's second header is the
    /// `duplicate_definition` face's business). The latch is scoped to its
    /// owner by token order: a declaring name always precedes its own
    /// `<...>` group, while any later declaration's name sits after a
    /// foreign window's close — a non-container owner (a typedef, a
    /// generic function) can never have its count adopted, so no boundary
    /// drops are needed. An unnamed container (`extension on Foo`) owns no
    /// raw verdict — its window is never consulted by name.
    ///
    /// The same event feeds the #610 bounds capture (the parameter bounds
    /// and the enum/extension-type flags) and opens the container's header
    /// window, so its supertype clauses capture from here to the body.
    pub(crate) fn note_container_type_variables(
        &mut self,
        name: Option<Token>,
        is_enum: bool,
        is_extension_type: bool,
    ) {
        if let Some(id) = name.and_then(|token| self.name_of(token)) {
            self.bounds.open_header(id, is_extension_type, is_enum);
            // The #612 capture opens with the header (params adopt below,
            // clause trees capture from here to the body) — unconditionally:
            // the legacy superinterface face answers in the default posture
            // too (#742; error_verifier.dart:7999-8054 is not
            // experiment-gated).
            self.var_header = Some(VarHeader::new(id));
        }
        let Some((unbounded, window_end)) = self.pending_tv.take() else {
            return;
        };
        let Some(token) = name else {
            return;
        };
        if token.range.offset >= window_end {
            return;
        }
        let Some(id) = self.name_of(token) else {
            return;
        };
        self.container_unbounded.entry(id).or_insert(unbounded);
        self.bounds.note_container(
            Some(id),
            token.range.offset,
            window_end,
            is_extension_type,
            is_enum,
        );
        // The type-variable window just closed is this container's: adopt
        // its captured parameters (name, keyword, anchor).
        if let Some(header) = self.var_header.as_mut()
            && header.name == id
        {
            header.params = std::mem::take(&mut self.var_tv_window);
        }
    }

    // —— #612: the variance-family capture hooks ———————————————————————————
    // Every hook is one gate check: without the `variance` experiment the
    // capture never opens and each answers immediately — default programs
    // stay byte-identical (#612 adjudication).

    /// `end_type_variable`: one declared parameter `(name, keyword, anchor)`
    /// joins the open window. The keyword's text is the official spelling
    /// (`out`/`in`/`inout`, `shared_type.dart:28-47`); anything else reads
    /// as the legacy default. The window collects regardless of the
    /// experiment (#742) — the legacy face reads the same capture.
    pub(crate) fn var_type_variable(
        &mut self,
        name: StringId,
        variance: Option<Token>,
        range: SourceRange,
    ) {
        let declared = variance.and_then(|token| match self.text_of(token) {
            Some("out") => Some(Variance::Covariant),
            Some("in") => Some(Variance::Contravariant),
            Some("inout") => Some(Variance::Invariant),
            _ => None,
        });
        self.var_tv_window.push((name, declared, range));
    }

    /// `begin_type_variables`: a fresh window (a previous declaration's
    /// stale window never met its container — drop it, with any paired
    /// names of an abandoned group).
    pub(crate) fn var_reset_type_variables(&mut self) {
        self.var_tv_window.clear();
        self.var_name_stack.clear();
    }

    /// `handle_type`: a simple type head completed inside the open header.
    /// The re-fired parameterized head merges into its group node (same
    /// name at the same offset — the trailing `?` rides on the re-fire).
    pub(crate) fn var_leaf(&mut self, name: StringId, offset: u32, nullable: bool) {
        let Some(header) = self.var_header.as_mut() else {
            return;
        };
        if header.rep_formals {
            return;
        }
        if header
            .sink
            .pending_head
            .as_ref()
            .is_some_and(|(h, o, _)| *h == name && *o == offset)
        {
            let (_, _, args) = header.sink.pending_head.take().expect("checked above");
            header.sink.pending_head = None;
            header.sink.route(VarTree {
                nullable,
                poisoned: false,
                kind: VarTreeKind::Head { name, args },
            });
            return;
        }
        // A stale pending head whose re-fire never came routes as written.
        if let Some((stale, _, args)) = header.sink.pending_head.take() {
            header.sink.route(VarTree {
                nullable: false,
                poisoned: false,
                kind: VarTreeKind::Head { name: stale, args },
            });
        }
        header.sink.route(VarTree {
            nullable,
            poisoned: false,
            kind: VarTreeKind::Head {
                name,
                args: Vec::new(),
            },
        });
    }

    /// `begin_type_arguments`: a group opened on `head` (name, offset).
    pub(crate) fn var_begin_group(&mut self, head: Option<(StringId, SourceRange)>) {
        if let Some(header) = self.var_header.as_mut()
            && !header.rep_formals
        {
            header.sink.stack.push(VarFrame::Group {
                head: head.map(|(name, range)| (name, range.offset)),
                nodes: Vec::new(),
                poisoned: false,
            });
        }
    }

    /// `end_type_arguments`: the group closed; a parameterized node waits
    /// for its re-fired head, a headless group poisons the parent.
    pub(crate) fn var_end_group(&mut self) {
        let Some(header) = self.var_header.as_mut() else {
            return;
        };
        let Some(VarFrame::Group {
            head,
            nodes,
            poisoned,
        }) = header.sink.stack.pop()
        else {
            return;
        };
        match head {
            Some((name, offset)) => {
                header.sink.pending_head = Some((name, offset, nodes));
            }
            None => {
                if poisoned {
                    header.sink.poison_top();
                }
            }
        }
    }

    /// `begin_function_type`.
    pub(crate) fn var_begin_function_type(&mut self) {
        if let Some(header) = self.var_header.as_mut()
            && !header.rep_formals
        {
            header.sink.stack.push(VarFrame::Fn(VarFnFrame {
                ret: None,
                params: Vec::new(),
                bounds: Vec::new(),
                in_params: false,
                in_bounds: false,
                poisoned: false,
            }));
        }
    }

    /// `end_function_type`: the frame closes into a tree.
    pub(crate) fn var_end_function_type(&mut self, nullable: bool) {
        if let Some(header) = self.var_header.as_mut()
            && let Some(VarFrame::Fn(frame)) = header.sink.stack.pop()
        {
            header.sink.route(frame.into_tree(nullable));
        }
    }

    /// `begin_formal_parameters`: a function-type frame enters its formals
    /// region; at the header level this is an extension type's primary
    /// constructor — its formals' types are not superinterfaces. The latch
    /// arms with a nesting count: a function-type formal's own region
    /// re-enters on the suppressed, frame-less sink, and only the arming
    /// region's close lifts the latch, so the implements clause that
    /// follows the header captures normally.
    pub(crate) fn var_formal_boundary(&mut self) {
        let Some(header) = self.var_header.as_mut() else {
            return;
        };
        if !header.sink.formal_boundary() {
            header.rep_depth += 1;
            header.rep_formals = true;
        }
    }

    /// `end_formal_parameters`: the innermost formals region closes; the
    /// latch lifts when the arming (outermost) region does. A clause event
    /// alone fires too late — the clause's trees parsed while the latch was
    /// up, so they would already be lost.
    pub(crate) fn var_formal_region_end(&mut self) {
        let Some(header) = self.var_header.as_mut() else {
            return;
        };
        if header.rep_depth > 0 {
            header.rep_depth -= 1;
            if header.rep_depth == 0 {
                header.rep_formals = false;
            }
        }
    }

    /// A signature's own type-parameter window opened inside a function
    /// type: trees complete into its bounds until the window closes.
    pub(crate) fn var_open_bounds(&mut self) {
        if let Some(header) = self.var_header.as_mut() {
            header.sink.open_bounds();
        }
    }

    pub(crate) fn var_close_bounds(&mut self) {
        if let Some(header) = self.var_header.as_mut() {
            header.sink.close_bounds();
        }
    }

    // —— written function-type signatures of variables (the invocation
    // face's formals for callees the element model cannot see) ————

    /// `begin_function_type`: a capture opens only where a variable
    /// declaration's own annotation can be — outside container headers
    /// (clauses and primary formals capture there) and outside executable
    /// bodies (parameters and local annotations stay out of scope for the
    /// face). A nested opening inside an unfinished capture is a
    /// function-typed formal's own annotation: poison the outer — the
    /// face cannot attribute the inner to a formal slot. The return
    /// region is NOT adopted from the armed slot here — the parser fires
    /// this event before the return type parses (`ComplexType::parse_type`'s
    /// pass 1), so the armed slot holds nothing of this signature yet; the
    /// return latches through [`Engine::fn_capture_type`] as its
    /// completions arrive.
    pub(crate) fn fn_capture_begin(&mut self) {
        if self.var_header.is_some() {
            return;
        }
        // #2359: inside an executable's block body the written function
        // type is the body-position annotation face (`x is void
        // Function(int)`, a local's annotation) — it captures and composes
        // into a real interned shape, so the shape-keyed sentinels
        // (`NameTy::from_semantic`) decide same-signature checks. The rest
        // of an executable's interior stays out: the return annotation (and
        // an arrow body, an initializer list) parses with the frame open but
        // the body brace unlatched, and its shapeless degradation is the
        // pinned under-report of the assignability context face
        // (`direct_function_typed_return_spelling_stays_silent`).
        if self.formal_list_depth == 0
            && !self.in_block_body()
            && !self
                .frames
                .iter()
                .all(|f| matches!(f.kind, FrameKind::Container { .. }))
        {
            return;
        }
        match self.fn_type_capture.last_mut() {
            // A NESTED function type (`Future<I> Function(void
            // Function(Object), Object)` — the outer signature's formal
            // carries an inner signature) pushes its OWN capture frame:
            // the inner close pops the inner and hands its interned
            // `Ty::Function` to this (still-open) outer capture through
            // `fn_capture_type`, exactly like any other formal type, and
            // the outer's own close then stashes the FULL signature. The
            // old blanket poison made every nested-signature parameter
            // degrade to a 0-formal stash, so the call-site arity read
            // "0 expected". Only an inner close that never landed any
            // formal (recovery) poisons the parent — the unclaimed flag
            // carries that verdict from the inner close.
            Some(open) => {
                let unclaimed = std::mem::take(&mut self.fn_nested_unclaimed);
                open.poisoned |= unclaimed;
                self.fn_type_capture.push(FnTypeCapture::default());
            }
            None => {
                self.fn_type_capture.push(FnTypeCapture::default());
            }
        }
    }

    /// Whether the position sits inside the innermost executable's latched
    /// block body (`begin_block_function_body`'s brace): the
    /// body-position annotation face. Statement blocks, catch clauses, and
    /// for-loops push [`FrameKind::Block`] frames over the executable, so
    /// the walk reads the innermost *executable* frame's latch.
    fn in_block_body(&self) -> bool {
        self.frames.iter().rev().find_map(|f| match &f.kind {
            FrameKind::Executable { body_brace, .. } => Some(body_brace.is_some()),
            _ => None,
        }) == Some(true)
    }

    /// `end_function_type`: the frame closes into a stashed signature —
    /// the next variable group's names claim it, every other declaration
    /// boundary drops it. The completed function type also re-arms
    /// [`Self::armed_ty`]: the innermost written formal's `handle_type`
    /// fires last inside the `(...)` region, so without the re-arm the
    /// slot would carry the *parameter's* type (`void Function(bool)?`
    /// armed `bool` — the `missing_default_value_for_parameter` /
    /// `unnecessary_null_comparison` false positives, #2161). The armed
    /// `Ty::Function` is also fed to the still-open parent capture, so a
    /// function-typed formal (`void Function(int Function())`) records the
    /// nested shape as its parameter type.
    pub(crate) fn fn_capture_end(&mut self, nullable: bool) {
        let Some(capture) = self.fn_type_capture.pop() else {
            // No capture in this context (a function type inside a body or
            // a container header the sig face does not record): the written
            // shape is still the last-completed type — arm a shapeless
            // `dynamic Function()` with the written `?`.
            self.armed_ty = Some(Ty::Function {
                shape: crate::model::FnShapeId::EMPTY,
                nullable,
            });
            self.armed_ty_unresolved = false;
            self.fn_capture_type(
                Ty::Function {
                    shape: crate::model::FnShapeId::EMPTY,
                    nullable,
                },
                StringId::NONE,
                0,
            );
            return;
        };
        let named: Vec<(StringId, Ty)> = capture
            .named
            .iter()
            .map(|(name, ty, _)| (*name, *ty))
            .collect();
        let return_ty = capture.ret.map_or(Ty::Dynamic, |(ty, ..)| ty);
        let fn_ty = self.boundary.args.borrow_mut().function(
            capture.params.len().min(u16::MAX as usize) as u16,
            return_ty,
            &capture.positional,
            &named,
            nullable,
        );
        self.armed_ty = Some(fn_ty);
        self.armed_ty_unresolved = false;
        // An inner capture that never landed a pending formal nor a
        // return (a stray `Function(` the formals never claimed): the
        // enclosing capture treats it as recovery poison; a clean close
        // leaves the outer signature intact.
        self.fn_nested_unclaimed = capture.in_params
            && capture.pending_formal.is_none()
            && capture.positional.is_empty()
            && capture.named.is_empty();
        self.fn_capture_type(fn_ty, StringId::NONE, 0);
        // A body-position capture (#2359) arms its shape for the
        // annotation/test faces but claims no variable-signature stash: the
        // claim sites are formals and container-level groups, and a stash
        // surviving into the body would be stolen by the next closure
        // formal's claim (the stale-attribution direction).
        if capture.poisoned || self.in_block_body() {
            return;
        }
        self.fn_type_stash = Some(Box::new(FnVarSig {
            params: capture.params.into_boxed_slice(),
            required_positional: capture.required_positional.min(u16::MAX as usize) as u16,
            positional: capture.positional.into_boxed_slice(),
            named: capture.named.into_boxed_slice(),
            return_ty,
        }));
    }

    /// A type completion inside the open capture: the formal section arms
    /// the pending formal, the signature's own type-variable bounds are
    /// the variables' business (consumed at each `end_type_variable`), and
    /// the return region takes every other completion — each replaces the
    /// previous, so the parameterized head's re-fire (the region's last
    /// word) wins over its leaf and type-argument completions.
    pub(crate) fn fn_capture_type(&mut self, ty: Ty, head: StringId, offset: u32) {
        let Some(capture) = self.fn_type_capture.last_mut() else {
            return;
        };
        if capture.in_params {
            capture.pending_formal = Some(ty);
        } else if !capture.in_variables {
            capture.ret = Some((ty, head, offset));
        }
    }

    /// `begin_formal_parameters` inside the open capture.
    pub(crate) fn fn_capture_formals_begin(&mut self) {
        if let Some(capture) = self.fn_type_capture.last_mut() {
            capture.in_params = true;
            capture.section = FnFormalSection::Required;
        }
    }

    /// `begin_optional_formal_parameters` inside the open capture: `[`
    /// opens the optional-positional group, `{` the named one. The same
    /// event arms the claim section when no capture is open — the outer
    /// formal list's own group.
    pub(crate) fn fn_capture_optional_group(&mut self, opener: &str) {
        if let Some(capture) = self.fn_type_capture.last_mut() {
            capture.section = if opener == "[" {
                FnFormalSection::OptionalPositional
            } else {
                FnFormalSection::Named
            };
        } else {
            self.fn_formal_named_section = opener == "{";
        }
    }

    /// A formal's name inside the open capture: the pending type joins
    /// its section's slot, with the formal head's `required` keyword
    /// (latched at `begin_formal_parameter`) on the named slots.
    pub(crate) fn fn_capture_formal(&mut self, name: StringId) {
        let required = std::mem::take(&mut self.capture_formal_required);
        let Some(capture) = self.fn_type_capture.last_mut() else {
            return;
        };
        if !capture.in_params {
            return;
        }
        let Some(ty) = capture.pending_formal.take() else {
            capture.poisoned = true;
            return;
        };
        match capture.section {
            FnFormalSection::Named => capture.named.push((name, ty, required)),
            FnFormalSection::Required => {
                capture.required_positional += 1;
                capture.positional.push(ty);
            }
            FnFormalSection::OptionalPositional => capture.positional.push(ty),
        }
    }

    /// `end_formal_parameter` inside an open capture: an UNNAMED formal
    /// (`Future<I> Function(void Function(Object), Object)`'s positional
    /// columns) fires no identifier event, so the pending formal its type
    /// armed claims here — a named formal's identifier event already took
    /// the slot, making this a no-op.
    pub(crate) fn fn_capture_formal_unnamed(&mut self) {
        let Some(capture) = self.fn_type_capture.last_mut() else {
            return;
        };
        if !capture.in_params {
            return;
        }
        let Some(ty) = capture.pending_formal.take() else {
            return;
        };
        match capture.section {
            FnFormalSection::Named => capture.poisoned = true,
            FnFormalSection::Required => {
                capture.required_positional += 1;
                capture.positional.push(ty);
            }
            FnFormalSection::OptionalPositional => capture.positional.push(ty),
        }
    }

    /// `end_formal_parameters` inside the open capture: a trailing
    /// unnamed formal (`void Function(T)`) flushes unclaimed, and the
    /// return region closes.
    pub(crate) fn fn_capture_formals_end(&mut self) {
        let Some(capture) = self.fn_type_capture.last_mut() else {
            return;
        };
        if let Some(ty) = capture.pending_formal.take() {
            match capture.section {
                FnFormalSection::Named => capture.poisoned = true,
                FnFormalSection::Required | FnFormalSection::OptionalPositional => {
                    capture.positional.push(ty)
                }
            }
        }
        capture.in_params = false;
    }

    /// The signature's own type variable arrived: it joins the capture's
    /// scope instead of the frameless park, so it never reaches the
    /// initializer's frames (the official scopes a function type's
    /// variables to the type itself).
    pub(crate) fn fn_capture_type_variable(&mut self, name: StringId) {
        if let Some(capture) = self.fn_type_capture.last_mut() {
            capture.scope.push(name);
        }
    }

    /// Every declaration boundary that is not the variable group owning a
    /// stashed signature drops it (a stale stash must never claim a later
    /// group).
    pub(crate) fn fn_stash_drop(&mut self) {
        self.fn_type_stash = None;
    }

    /// `begin_function_expression` (the literal's frame already open):
    /// starts the literal's signature capture.
    pub(crate) fn closure_capture_begin(&mut self, offset: u32) {
        self.closure_caps.push(ClosureCapture {
            offset,
            frame_depth: self.frames.len(),
            list_depth: 0,
            list_open: false,
            positional: Vec::new(),
            optional_from: None,
            poisoned: false,
        });
    }

    /// `begin_formal_parameters` (depth already bumped): the first list
    /// opened directly under the open literal's frame is the literal's own.
    pub(crate) fn closure_formals_begin(&mut self) {
        let depth = self.formal_list_depth;
        let frames = self.frames.len();
        if let Some(capture) = self.closure_caps.last_mut()
            && capture.frame_depth == frames
            && capture.list_depth == 0
        {
            capture.list_depth = depth;
            capture.list_open = true;
        }
    }

    /// `end_formal_parameters` (depth not yet dropped): the literal's own
    /// list closed.
    pub(crate) fn closure_formals_end(&mut self) {
        let depth = self.formal_list_depth;
        if let Some(capture) = self.closure_caps.last_mut()
            && capture.list_open
            && capture.list_depth == depth
        {
            capture.list_open = false;
        }
    }

    /// The `[`/`{` opener inside the literal's own list: `[` starts the
    /// optional-positional tail; a named section poisons (the consumer
    /// renders positional signatures only).
    pub(crate) fn closure_optional_group(&mut self, opener: &str) {
        let depth = self.formal_list_depth;
        if let Some(capture) = self.closure_caps.last_mut()
            && capture.list_open
            && capture.list_depth == depth
        {
            if opener == "[" {
                capture.optional_from = Some(capture.positional.len());
            } else {
                capture.poisoned = true;
            }
        }
    }

    /// A formal of the literal's own list: its written type (still armed)
    /// joins the capture; an untyped formal poisons — its type is the
    /// solver's downward answer, which this face does not compute.
    pub(crate) fn closure_formal(&mut self) {
        let depth = self.formal_list_depth;
        let ty = self.armed_ty;
        if let Some(capture) = self.closure_caps.last_mut()
            && capture.list_open
            && capture.list_depth == depth
        {
            match ty {
                Some(ty) if ty != Ty::Unknown => capture.positional.push(ty),
                _ => capture.poisoned = true,
            }
        }
    }

    /// `end_function_expression` (the literal's frame still open): closes
    /// the capture and hands an exact signature to the enclosing argument
    /// list. An `async`/generator body changes the literal's return type
    /// (`Future<void>`), which this capture does not render — poisoned.
    pub(crate) fn closure_capture_end(&mut self, end: u32) {
        let Some(capture) = self.closure_caps.pop() else {
            return;
        };
        // The closure-return tally closes with the literal: a counted
        // return set with no valid entry marks the closure all-invalid —
        // its `T Function(...)` consumers infer an `InvalidType` result.
        if self.closure_return_counted.contains(&capture.offset)
            && !self.closure_return_valid.contains(&capture.offset)
        {
            self.closure_invalid_returns.insert(capture.offset);
            // The span joins even when the signature capture poisoned (an
            // untyped `_` formal): the invalid-result probe matches
            // arguments by span, not by the captured signature.
            self.invalid_closure_spans.push((capture.offset, end));
        }
        let suspends = self.frames.last().is_some_and(|frame| {
            matches!(
                frame.kind,
                FrameKind::Executable { is_async: true, .. }
                    | FrameKind::Executable {
                        generator: true,
                        ..
                    }
            )
        });
        if capture.poisoned || capture.list_depth == 0 || suspends {
            return;
        }
        let required = capture.optional_from.unwrap_or(capture.positional.len());
        let sig = Box::new(FnVarSig {
            params: Box::default(),
            required_positional: u16::try_from(required).unwrap_or(u16::MAX),
            positional: capture.positional.into_boxed_slice(),
            named: Box::default(),
            return_ty: Ty::Void,
        });
        if let Some(level) = self.arg_levels.last_mut() {
            level.closures.push((capture.offset, end, sig));
        }
    }

    /// An unattributable shape inside the open capture (an inline
    /// function-typed formal, a receiver formal): the signature suppresses.
    pub(crate) fn fn_capture_poison(&mut self) {
        if let Some(capture) = self.fn_type_capture.last_mut() {
            capture.poisoned = true;
        }
    }

    /// `end_formal_parameter` inside the open capture: a formal whose name
    /// never fired (an unnamed `void Function(T)` slot) flushes unclaimed
    /// here; a receiver formal poisons.
    pub(crate) fn fn_capture_formal_end(&mut self, receiver: bool) {
        let Some(capture) = self.fn_type_capture.last_mut() else {
            return;
        };
        if !capture.in_params {
            return;
        }
        if receiver {
            capture.poisoned = true;
            return;
        }
        if let Some(ty) = capture.pending_formal.take() {
            match capture.section {
                FnFormalSection::Named => capture.poisoned = true,
                FnFormalSection::Required => {
                    capture.required_positional += 1;
                    capture.positional.push(ty);
                }
                FnFormalSection::OptionalPositional => capture.positional.push(ty),
            }
        }
    }

    // —— explicit call-site type arguments ————————————————

    /// `begin_type_arguments`: a headless group at call depth opens the
    /// written-argument capture (an invocation's `<...>`; headed groups
    /// are annotations and ride the #610 sink).
    pub(crate) fn call_args_begin(&mut self, headed: bool) {
        if headed {
            return;
        }
        self.call_args_capture = Some(CallArgsCapture::default());
    }

    /// A type completion inside the open call-argument group (the merge
    /// latch mirrors the sink's: the re-fired head replaces its own leaf).
    pub(crate) fn call_args_feed(&mut self, ty: Ty, head: StringId, offset: u32) {
        let Some(capture) = self.call_args_capture.as_mut() else {
            return;
        };
        if capture
            .pending
            .is_some_and(|(prev_head, prev_offset)| prev_head != head || prev_offset != offset)
        {
            // A stale leaf whose re-fire never came stays as written.
        } else if capture.pending.take().is_some() {
            capture.members.pop();
        }
        capture.pending = Some((head, offset));
        capture.members.push(ty);
    }

    /// `end_type_arguments`: the headless group closed; its written
    /// members latch for the head row the group applies to.
    pub(crate) fn call_args_end(&mut self, head_row: usize) {
        let Some(capture) = self.call_args_capture.take() else {
            return;
        };
        // The entry keys on the head row's OFFSET, never its index: rows
        // pop and merge through every fold, and a reused index would
        // re-ground a dead group on an unrelated later callee (json1's
        // `_col('key')` inheriting a sibling declaration's `<int>` and
        // reporting the wrong return instantiation).
        let Some(head) = self.rows.get(head_row) else {
            return;
        };
        let head_offset = head.range.offset;
        self.call_type_args
            .retain(|(offset, _)| *offset < head_offset);
        self.call_type_args
            .push((head_offset, capture.members.into_boxed_slice()));
    }

    /// The explicit type arguments of the callee row, if a call-site
    /// group latched any.
    pub(crate) fn call_args_for(&mut self, callee: usize) -> Option<Box<[Ty]>> {
        let offset = self.rows.get(callee)?.range.offset;
        let pos = self
            .call_type_args
            .iter()
            .rposition(|(at, _)| *at == offset)?;
        Some(self.call_type_args.remove(pos).1)
    }

    /// `begin_record_type`.
    pub(crate) fn var_begin_record_type(&mut self) {
        if let Some(header) = self.var_header.as_mut()
            && !header.rep_formals
        {
            header.sink.stack.push(VarFrame::Record {
                nodes: Vec::new(),
                poisoned: false,
            });
        }
    }

    /// `end_record_type`: the frame closes into a tree.
    pub(crate) fn var_end_record_type(&mut self, nullable: bool) {
        if let Some(header) = self.var_header.as_mut()
            && let Some(VarFrame::Record { nodes, poisoned }) = header.sink.stack.pop()
        {
            header.sink.route(VarTree {
                nullable,
                poisoned,
                kind: VarTreeKind::Record { fields: nodes },
            });
        }
    }

    /// A clause event: the completed supertype trees join the set.
    pub(crate) fn var_clause(&mut self, slot: VarSlot, expected: Option<usize>) {
        if let Some(header) = self.var_header.as_mut() {
            header.clause(slot, expected);
        }
    }

    /// The body opened: the capture finalizes onto its container
    /// (keep-first by name; a re-entry's clause trees write through).
    pub(crate) fn var_finish_header(&mut self) {
        if self.var_header.is_none() {
            return;
        }
        let header = self.var_header.take().expect("checked above");
        let open = header.open || !header.sink.roots.is_empty() || !header.sink.stack.is_empty();
        if let Some(existing) = self
            .var_containers
            .iter_mut()
            .find(|container| container.name == header.name)
        {
            existing.supers = header.supers;
            existing.open = open;
            return;
        }
        self.var_containers.push(VarContainer {
            name: header.name,
            params: header.params,
            supers: header.supers,
            open,
        });
    }

    /// Declaration end: the safety-net drop (the capture normally closed
    /// at the body).
    pub(crate) fn var_drop_header(&mut self) {
        self.var_header = None;
    }

    /// The end-of-pass variance family (#612): the superinterface face
    /// (`_checkForWrongTypeParameterVarianceInSuperinterfaces`,
    /// error_verifier.dart:7999-8054) and the member faces
    /// (:7910-7996 through `_checkForWrongVariancePosition` :8074-8088),
    /// against the whole-file capture. Runs after the parse, staged into
    /// the usage rows for the teardown's source-ordered merge.
    ///
    /// Gate posture (#742): the legacy superinterface face
    /// (`wrong_type_parameter_variance_in_superinterface`) answers in the
    /// default posture — the oracle's walk runs unconditionally
    /// (error_verifier.dart:7999-8054, dispatching on `isLegacyCovariant`)
    /// and its default-corpus expectations fire the legacy code with no
    /// experiment grant
    /// (`wrong_type_parameter_variance_in_superinterface_test.dart`,
    /// e.g. `test_class_extends_function_parameterType`). The
    /// explicit-keyword faces keep the experiment gate: a parameter
    /// carrying a written keyword answers
    /// `wrong_explicit_type_parameter_variance_in_superinterface` here and
    /// drives the `_position` member faces only under the grant.
    ///
    /// Model edges (documented divergences, all under-report except the
    /// noted face): generic typedefs in superinterfaces answer unrelated
    /// (alias expansion is a resolution fact this face does not model —
    /// the same posture as the #610 map); the formal `covariant` and
    /// field-group `covariant` exemptions (:7926-7929, :7972-7974) are
    /// unmodeled; a method's type-parameter-bound face (:7943-7956) is
    /// unmodeled; the method-formal/return anchors sit at the member name.
    #[expect(clippy::too_many_lines)] // long diagnostic-table dispatch
    pub(crate) fn finalize_variance(&mut self) {
        if self.var_containers.is_empty() {
            return;
        }
        let strings = self.strings;
        // The declared-variance table: this file's containers by name. A
        // head outside it reads all-covariant — every generic of the
        // default SDK carries legacy-covariant parameters
        // (element.dart:11560-11562).
        let table: FxHashMap<StringId, Vec<Variance>> = self
            .var_containers
            .iter()
            .map(|container| {
                (
                    container.name,
                    container
                        .params
                        .iter()
                        .map(|(_, declared, _)| declared.unwrap_or(Variance::Covariant))
                        .collect(),
                )
            })
            .collect();
        // The member name anchors (the model carries no spans; the
        // declaration sites do).
        let member_ranges: FxHashMap<(StringId, StringId), SourceRange> = self
            .decl_sites
            .iter()
            .filter_map(|site| match site.kind {
                DeclKind::Member { container, .. } => Some((container, site.name, site.range)),
                _ => None,
            })
            .map(|(container, name, range)| ((container, name), range))
            .collect();
        for container in std::mem::take(&mut self.var_containers) {
            if container.open {
                continue;
            }
            let Some(model) = self
                .index
                .get(self.file)
                .and_then(|elements| elements.container(container.name))
            else {
                continue;
            };
            // The official verifier has no extension-declaration visit
            // (:615/:659/:1053/:1194/:1865 — class, class-type alias, enum,
            // extension type, mixin).
            if matches!(model.kind, ContainerKind::Extension) {
                continue;
            }
            // —— the superinterface face ———————————————————————————————
            for (parameter, declared, range) in &container.params {
                let declared_variance = declared.unwrap_or(Variance::Covariant);
                for (_, tree) in &container.supers {
                    if tree.poisoned {
                        continue;
                    }
                    let variance = variance_in_tree(tree, *parameter, &table, VARIANCE_WALK_DEPTH);
                    if variance.greater_or_equal(declared_variance) {
                        continue;
                    }
                    let display = self.var_tree_display(tree);
                    let code = match declared {
                        // A keyword without the grant keeps its face gated
                        // (#742): the keyword parses unconditionally (the
                        // header always allows variance), so the capture
                        // sees it and the gate is the only suppression.
                        Some(_) if !self.variance_enabled => continue,
                        Some(_) => SemanticCode::WrongExplicitTypeParameterVarianceInSuperinterface,
                        None => SemanticCode::WrongTypeParameterVarianceInSuperinterface,
                    };
                    let name = strings.resolve(*parameter).to_owned();
                    let message = match declared {
                        Some(keyword) => {
                            let position = variance.keyword().unwrap_or("");
                            code.message4(
                                &name,
                                keyword.keyword().unwrap_or(""),
                                position,
                                &display,
                            )
                        }
                        None => code.message(&name, &display),
                    };
                    self.report.usage.push(Diagnostic::new(
                        code.severity(),
                        code.code_info(),
                        message,
                        self.file,
                        *range,
                        code.correction_message(""),
                    ));
                }
            }
            // —— the member faces ———————————————————————————————————————
            // Only explicit keywords are checked (`!isLegacyCovariant`,
            // :7918/:7945): an all-legacy container can never fire. The
            // faces keep the experiment gate (#742): a parsed keyword
            // without the grant answers nothing.
            if !self.variance_enabled
                || container
                    .params
                    .iter()
                    .all(|(_, declared, _)| declared.is_none())
            {
                continue;
            }
            for (member, entry) in &model.members {
                if entry.is_static {
                    continue;
                }
                let Some(&anchor) = member_ranges.get(&(container.name, *member)) else {
                    continue;
                };
                for (parameter, declared, _) in &container.params {
                    let Some(keyword) = declared.and_then(Variance::keyword) else {
                        continue;
                    };
                    let parameter = *parameter;
                    let declared_variance = declared.unwrap_or(Variance::Covariant);
                    match entry.kind {
                        MemberKind::Field => {
                            let variance = self.variance_in_ty(entry.var_ty, parameter, &table);
                            self.check_variance_position(
                                variance,
                                declared_variance,
                                keyword,
                                parameter,
                                anchor,
                            );
                            if !entry.is_final {
                                let setter = Variance::Contravariant.combine(variance);
                                self.check_variance_position(
                                    setter,
                                    declared_variance,
                                    keyword,
                                    parameter,
                                    anchor,
                                );
                            }
                        }
                        MemberKind::Method | MemberKind::Getter | MemberKind::Setter => {
                            let variance = self.variance_in_ty(entry.return_ty, parameter, &table);
                            self.check_variance_position(
                                variance,
                                declared_variance,
                                keyword,
                                parameter,
                                anchor,
                            );
                            if let Some(formals) = &entry.formals {
                                for formal in formals.positional.iter().chain(&formals.named) {
                                    let variance = Variance::Contravariant
                                        .combine(self.variance_in_ty(formal.ty, parameter, &table));
                                    self.check_variance_position(
                                        variance,
                                        declared_variance,
                                        keyword,
                                        parameter,
                                        anchor,
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    /// One `_position` verdict: `variance` must satisfy the parameter's
    /// declared keyword (`_checkForWrongVariancePosition`,
    /// error_verifier.dart:8074-8088).
    pub(crate) fn check_variance_position(
        &mut self,
        variance: Variance,
        declared: Variance,
        keyword: &str,
        parameter: StringId,
        anchor: SourceRange,
    ) {
        if variance.greater_or_equal(declared) {
            return;
        }
        // `unrelated` satisfies every declared keyword, so the position
        // keyword is always present at an emission.
        let Some(position) = variance.keyword() else {
            return;
        };
        let code = SemanticCode::WrongTypeParameterVariancePosition;
        let name = self.strings.resolve(parameter);
        self.report.usage.push(Diagnostic::new(
            code.severity(),
            code.code_info(),
            code.message3(keyword, name, position),
            self.file,
            anchor,
            code.correction_message(""),
        ));
    }

    /// The written-type display for a superinterface slot: the head, its
    /// arguments, the function/record shapes as written (a function type's
    /// own `<...>` section and record field names are not captured — a
    /// documented display divergence on those edges).
    pub(crate) fn var_tree_display(&self, tree: &VarTree) -> String {
        match &tree.kind {
            VarTreeKind::Head { name, args } => {
                let mut text = self.strings.resolve(*name).to_owned();
                if !args.is_empty() {
                    text.push('<');
                    for (index, arg) in args.iter().enumerate() {
                        if index > 0 {
                            text.push_str(", ");
                        }
                        text.push_str(&self.var_tree_display(arg));
                    }
                    text.push('>');
                }
                if tree.nullable {
                    text.push('?');
                }
                text
            }
            VarTreeKind::Fn { ret, params, .. } => {
                let mut text = match ret {
                    Some(ret) => self.var_tree_display(ret),
                    // `void` never fires a type event; an absent return
                    // displays as `void` (a recovery residue displays so
                    // too).
                    None => "void".to_string(),
                };
                text.push_str(" Function(");
                for (index, param) in params.iter().enumerate() {
                    if index > 0 {
                        text.push_str(", ");
                    }
                    text.push_str(&self.var_tree_display(param));
                }
                text.push(')');
                if tree.nullable {
                    text.push('?');
                }
                text
            }
            VarTreeKind::Record { fields } => {
                let mut text = String::from("(");
                for (index, field) in fields.iter().enumerate() {
                    if index > 0 {
                        text.push_str(", ");
                    }
                    text.push_str(&self.var_tree_display(field));
                }
                text.push(')');
                if tree.nullable {
                    text.push('?');
                }
                text
            }
            VarTreeKind::Unit => String::new(),
        }
    }

    /// The variance of `parameter` in a model type: the tree walk over the
    /// converted shape (`Ty` cannot represent function or record shapes —
    /// an `Unknown` contributes unrelated, the under-report direction).
    pub(crate) fn variance_in_ty(
        &self,
        ty: Ty,
        parameter: StringId,
        table: &FxHashMap<StringId, Vec<Variance>>,
    ) -> Variance {
        match self.ty_to_tree(ty) {
            Some(tree) => variance_in_tree(&tree, parameter, table, VARIANCE_WALK_DEPTH),
            None => Variance::Unrelated,
        }
    }

    pub(crate) fn ty_to_tree(&self, ty: Ty) -> Option<VarTree> {
        let Ty::Named {
            name,
            nullable,
            args,
        } = ty
        else {
            return None;
        };
        let pool = self.boundary.pool();
        let args = pool
            .get(args)
            .iter()
            .map(|&arg| self.ty_to_tree(arg))
            .collect::<Option<Vec<_>>>()?;
        Some(VarTree {
            nullable,
            poisoned: false,
            kind: VarTreeKind::Head { name, args },
        })
    }
}
