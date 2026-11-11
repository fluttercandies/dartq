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
    /// order alone. A nested group's close (`<X extends void Function<U>()>`
    /// — the bound's own `<U>` re-opens `tv_window`) consumes the inner
    /// window first, so the enclosing close finds `None` and must leave the
    /// latched `pending_tv` alone: clearing it here would erase the inner
    /// group's latch before the container head adopts it.
    pub(crate) fn close_type_variables(&mut self, window_end: Token) {
        if let Some(window) = self.tv_window.take() {
            self.pending_tv = Some((window.unbounded, window_end.range.offset));
        }
    }

    /// #1314 FAM10: pairs the closed type-variable window with the latched
    /// declaring executable's name (`exec_bounds`). Each parameter's
    /// written bound maps through [`Engine::tree_ty`] — a poisoned bound
    /// keeps the name-only shape, recovery residue is never a fact. The
    /// token-order guard mirrors the container adoption: a declaring name
    /// precedes its own window, a foreign window's close sits after a
    /// later declaration's name and drops (the latch clears either way —
    /// keep-first, one window adopts once).
    ///
    /// A nested group's close never pairs (#2897): a bound's function-type
    /// variables (`void f<T extends void Function<X extends num>>()`)
    /// close INSIDE the header's group — adopting there would overwrite
    /// the executable's parameters with the bound's own variable
    /// (`X extends num`) and every bounds face would check the wrong
    /// variable. The gate reads the still-open enclosing group after the
    /// window latch, before the executable latch is consumed.
    pub(crate) fn pair_exec_window(&mut self, window_end: u32) {
        if self.bounds.in_type_variables() {
            return;
        }
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
        // (their `exec_params.is_none()` guard would starve). The window
        // also carries its declaring name's offset and declaration order
        // is the walk's own order: the post-walk name-only scans iterate
        // this list, never a hash map (#4202).
        self.exec_windows.push(ExecWindow {
            owner: name,
            at: name_offset,
            is_method: self.pending_exec_is_method,
            params: window,
        });
    }

    /// #1314 FAM18: the group's close — the constructor-independent faces
    /// decide (`not_initialized_non_nullable_variable` for top-level
    /// variables and static fields; `final_not_initialized` for static
    /// finals — the top-level final spelling stays parser-owned, the CFE's
    /// shared `finalNotInitialized` face fires there); instance-field
    /// entries stay latched for the container's end.
    ///
    /// The declaration heads exempt both faces: `abstract`/`external`
    /// (`visitTopLevelVariableDeclaration`'s group gate,
    /// error_verifier.dart:2088-2092) and `late` (the staged latch never
    /// arms) — an `external int x;` is implemented outside the library and
    /// owns no initializer obligation.
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
            // The top-level group gate (`visitTopLevelVariableDeclaration`:
            // `node.abstractKeyword == null && node.externalKeyword == null
            // && !variableList.isLate`, error_verifier.dart:2088-2092): the
            // face covers both the `final_not_initialized` and the
            // `not_initialized_non_nullable_variable` rows, so it decides
            // before the split. A static field's bits reach this point
            // through the member model above (the `exempt` check); a
            // top-level variable has no model row, so its head is read off
            // the token stream.
            if check.container.is_none() && self.decl_head_exempts_init(check.range.offset) {
                continue;
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
                // The staged entry's written-head gate is re-decided on the
                // collected declared type — the oracle reads
                // `element.type`, not the annotation (`dynamic`/`void`/
                // `Null`/`FutureOr<T?>` never report; `isNullable`
                // recurses the `FutureOr` argument, type_system.dart:
                // 1220-1233). An uncaptured (`Unknown`) type keeps the
                // written-head verdict: the staged gate already cleared
                // the explicit `?` spellings.
                let var_ty = match check.container {
                    Some(container) => self
                        .index
                        .get(self.file)
                        .and_then(|elements| elements.container(container))
                        .and_then(|decl| decl.members.get(&check.name))
                        .map(|entry| entry.var_ty),
                    None => self
                        .index
                        .get(self.file)
                        .and_then(|elements| elements.decl(check.name))
                        .map(|decl| decl.var_ty),
                };
                if var_ty.is_some_and(|ty| ty != Ty::Unknown && self.written_nullable(ty)) {
                    continue;
                }
                self.emit(
                    SemanticCode::NotInitializedNonNullableVariable,
                    check.range,
                    &name,
                    "",
                );
            }
        }
    }

    /// The lexeme of the token starting exactly at `offset` (UTF-16), or
    /// `None` when no token begins there — a recovered/synthetic anchor
    /// never lands on a real token boundary.
    pub(crate) fn lexeme_at(&self, offset: u32) -> Option<&'a str> {
        let tokens = self.tokens.as_slice();
        let at = tokens.partition_point(|token| token.utf16_offset < offset);
        if tokens
            .get(at)
            .is_some_and(|token| token.utf16_offset == offset)
        {
            Some(self.tokens.lexeme(self.source, at))
        } else {
            None
        }
    }

    /// The token immediately AFTER the one starting at `offset` — the
    /// `.name` selector of a named primary-constructor head reads it.
    pub(crate) fn token_after(&self, offset: u32) -> Option<SourceRange> {
        let tokens = self.tokens.as_slice();
        let at = tokens.partition_point(|token| token.utf16_offset <= offset);
        tokens.get(at).map(|token| SourceRange {
            offset: token.utf16_offset,
            length: self.utf16_of((token.byte_offset + token.byte_len) as usize)
                - token.utf16_offset,
        })
    }

    /// The name-level `isNullable` (type_system.dart:1208-1233): the
    /// `?`-suffixed and `dynamic`/`Null`/`void`/`Unknown` faces are
    /// nullable, a `FutureOr<T>` recurses its argument (`FutureOr` bare
    /// reads `FutureOr<dynamic>`), everything else is not. `FutureOr` is
    /// name-matched: a user type shadowing the dart:async spelling takes
    /// the generic arm, which is the under-report direction.
    fn written_nullable(&self, ty: Ty) -> bool {
        match ty {
            Ty::Dynamic | Ty::Void | Ty::Null | Ty::Unknown => true,
            Ty::Named { name, nullable, .. }
                if nullable || self.strings.resolve(name) != "FutureOr" =>
            {
                nullable
            }
            Ty::Named {
                args: crate::model::ArgListId::EMPTY,
                ..
            } => true,
            Ty::Named { args, .. } => {
                let arg = {
                    let pool = self.boundary.args.borrow();
                    pool.get(args).first().copied()
                };
                arg.is_none_or(|arg| self.written_nullable(arg))
            }
            Ty::Record { nullable, .. } | Ty::Function { nullable, .. } => nullable,
            Ty::Never => false,
        }
    }

    /// Whether the top-level declaration head of the variable named at
    /// `name_offset` carries `abstract` or `external` — upstream's group
    /// gate for both uninitialized faces (`visitTopLevelVariableDeclaration`,
    /// error_verifier.dart:2088-2092). The model carries no modifier row
    /// for a top-level variable, so the head is read off the token stream:
    /// the scan opens after the previous declaration boundary and stops at
    /// the group's first `=` — an initializer can hold a bare `external`
    /// identifier (`var y = external;` is legal), a head cannot.
    fn decl_head_exempts_init(&self, name_offset: u32) -> bool {
        let tokens = self.tokens.as_slice();
        let at = tokens.partition_point(|token| token.utf16_offset < name_offset);
        let mut start = at as isize - 1;
        while start >= 0 {
            match self.tokens.lexeme(self.source, start as usize) {
                ";" | "{" | "}" => break,
                _ => start -= 1,
            }
        }
        for index in (start + 1).max(0) as usize..at {
            match self.tokens.lexeme(self.source, index) {
                "abstract" | "external" => return true,
                "=" | ";" | "}" => return false,
                _ => {}
            }
        }
        false
    }

    /// Whether the constructor head starting at `head` (its class-name
    /// token) was declared `external` — the upstream `_addConstructor`
    /// `externalKeyword` exclusion (constructor_fields_verifier.dart:57).
    /// `external` precedes `const` (`external const C()`); any other
    /// modifier or a member boundary stops the walk.
    fn ctor_head_is_external(&self, head: SourceRange) -> bool {
        let tokens = self.tokens.as_slice();
        let at = tokens.partition_point(|token| token.utf16_offset < head.offset);
        let mut index = at as isize - 1;
        while index >= 0 {
            match self.tokens.lexeme(self.source, index as usize) {
                "const" => index -= 1,
                "external" => return true,
                _ => return false,
            }
        }
        false
    }

    /// #1314 FAM18: the container's end — its constructor set is complete,
    /// so the instance-field half decides. With no explicit generative
    /// constructor the declaration loop owns the face (per field in
    /// declaration order, the `abstract`/`external` bits exempt); with one,
    /// the `ConstructorFieldsVerifier` owns it: per generative constructor,
    /// the still-uninitialized finals' aggregated row first, then the
    /// non-final non-nullable rows — all anchored on the constructor's
    /// type-name-through-selector errorRange, the unnamed spelling's
    /// class-name token. A constructor-initializer assignment (`b = e` /
    /// `this.b = e`) releases the field's latch (`updateWithInitializers`).
    /// External and named-factory sites stay out of the drive (upstream
    /// `_addConstructor` skips external heads and never sees a factory),
    /// and duplicate-named fields join no constructor row (upstream
    /// `checkElement`'s already-reported guard).
    pub(crate) fn flush_init_instance_fields(&mut self, container: StringId) {
        // The const-constructor declaration face (#3293) reads the complete
        // field set and constructor set of the class, both closed by this
        // point — every early return below belongs to the
        // initialization rows alone.
        self.flush_const_ctor_non_final_face(container);
        // The generative constructor set: every declared constructor rides a
        // `begin_constructor` site; factories register no head anchor (their
        // provisional is `None`) and stay out of the generative walks. The
        // row span is the oracle's `errorRange` (constructor_fields_
        // verifier.dart:66/:90 — the ConstructorDeclaration's
        // type-name-through-selector for a secondary head, the
        // PrimaryConstructorDeclaration's own errorRange — the class name
        // through its selector — for a primary): the site's own range is
        // rewritten to the selector token for a named secondary and stays
        // the `(`/`.` `begin` token for a primary, so the class-name extent
        // recomposes from the head anchor + container-name token instead.
        let container_name = self
            .decl_sites
            .iter()
            .rev()
            .find(|site| matches!(site.kind, DeclKind::Container) && site.name == container)
            .map(|site| site.range);
        // The declaration currently closing: its container site is the last
        // one of the name (an `augment` declaration is a separate site). The
        // flush runs once per declaration, so a constructor or field belongs
        // to THIS declaration only when it sits at or after this start — a
        // merged element's earlier declaration was already decided by its
        // own flush.
        let decl_start = container_name.map(|range| range.offset);
        // Upstream keys `_forInterface` per ELEMENT: an `augment` constructor
        // merges into the introductory element, whose `_Constructor` state
        // (and `errorRange`) is created by the FIRST declaration and reused
        // by every later fragment. Reporting the merged element again at an
        // augment head would double the row, so a key already decided by an
        // earlier declaration stays out of this pass.
        let ctor_key = |site: &DeclSite| {
            if site.name == container || self.strings.resolve(site.name) == "new" {
                container
            } else {
                site.name
            }
        };
        let mut decided_ctors: FxHashSet<StringId> = FxHashSet::default();
        if let Some(start) = decl_start {
            for (index, site) in self.decl_sites.iter().enumerate() {
                if site.is_factory
                    || !matches!(site.kind, DeclKind::Constructor { container: c } if c == container)
                {
                    continue;
                }
                let Some(head) = self.ctor_head_anchors.get(&index) else {
                    continue;
                };
                if head.offset < start {
                    decided_ctors.insert(ctor_key(site));
                }
            }
        }
        let ctor_spans: Vec<(usize, SourceRange)> = self
            .decl_sites
            .iter()
            .enumerate()
            .filter(|(_, site)| {
                matches!(site.kind, DeclKind::Constructor { container: c } if c == container)
            })
            .filter_map(|(index, site)| {
                let head = *self.ctor_head_anchors.get(&index)?;
                // Only this declaration's constructors (see `decl_start`).
                if decl_start.is_some_and(|start| head.offset < start) {
                    return None;
                }
                if decided_ctors.contains(&ctor_key(site)) {
                    return None;
                }
                // Upstream `_addConstructor` skips `externalKeyword` heads
                // outright (constructor_fields_verifier.dart:56-59): an
                // external generative constructor still counts for
                // `hasGenerativeConstructor` (the declaration face stays
                // off) but never joins the per-constructor report.
                if self.ctor_head_is_external(head) {
                    return None;
                }
                let span = match self.lexeme_at(head.offset) {
                    // A primary constructor's anchor is its `(` token — the
                    // oracle's errorRange is the class name (unnamed) or
                    // the class name through the `.name` selector.
                    Some("(") => container_name.unwrap_or(head),
                    Some(".") => {
                        let start = container_name.map_or(head.offset, |n| n.offset);
                        let end = self
                            .token_after(head.offset)
                            .map_or(head.end(), |token| token.end());
                        SourceRange {
                            offset: start,
                            // `end` is the head's next-token end; `start` is the
                            // `.Name` container prefix when one is written, which
                            // can sit AFTER that token on a recovery head
                            // (`.name(` with the selector past the `(`). Clamp so
                            // the inverted span reports a zero-length row instead
                            // of underflowing.
                            length: end.saturating_sub(start),
                        }
                    }
                    // A named secondary's site range was rewritten to its
                    // selector (`MethodDeclarationContinuation`) — merge
                    // head through selector; an unnamed head's site range
                    // IS the head.
                    _ if site.range.offset > head.offset => SourceRange {
                        offset: head.offset,
                        // A head anchor past its site range end (recovery ctor)
                        // inverts the length; clamp to zero rather than
                        // underflow.
                        length: site.range.end().saturating_sub(head.offset),
                    },
                    _ => site.range,
                };
                Some((index, span))
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
        // counts EVERY declared generative constructor, redirecting and
        // external ones included: their presence routes instance fields to
        // the `ConstructorFieldsVerifier` face (which then silences each
        // redirecting ctor per-ctor via its own
        // `hasRedirectingConstructorInvocation` early return, and skips
        // external ctors in `_addConstructor`). A factory is not
        // generative — a factory-only class keeps the declaration face
        // (`isGenerative` on a factory element is false, so
        // `factory C.named() => throw 0;` reports `final_not_initialized`
        // at the field). `DeclKind::Constructor` sites with `is_factory`
        // are the named factories (unnamed ones register no site).
        let has_generative_ctor = self.decl_sites.iter().any(|site| {
            !site.is_augment
                && !site.is_factory
                && matches!(site.kind, DeclKind::Constructor { container: c } if c == container)
        });
        // `member_duplicate_definition_verifier.dart`'s constructor pass:
        // per container ELEMENT, constructor names dedupe by selector —
        // `member.name?.lexeme ?? 'new'` — and every second-and-later
        // declaration of a key reports `duplicate_constructor` at its
        // `errorRange` (`ConstructorDeclarationImpl`: the type name through
        // the selector, or `newKeyword`/`factoryKeyword` through the name
        // for a `new`-head/bare-factory spelling). The element's context
        // spans every `augment` fragment — the constructor set seeds across
        // fragments in declaration order — while a same-named plain
        // redeclaration is a fresh element (its `duplicate_definition` row
        // is another face's business, so earlier declarations' sites stay
        // out).
        {
            let new_name = self.intern("new");
            // The element chain ends at the declaration currently closing:
            // the last container site of the name, extended backwards —
            // a fragment joins the previous element's context only while
            // it is itself an `augment` declaration.
            let container_sites: Vec<usize> = self
                .decl_sites
                .iter()
                .enumerate()
                .filter(|(_, site)| {
                    matches!(site.kind, DeclKind::Container) && site.name == container
                })
                .map(|(index, _)| index)
                .collect();
            let mut first = container_sites.len().saturating_sub(1);
            while first > 0
                && self.decl_head_is_augment(self.decl_sites[container_sites[first]].range)
            {
                first -= 1;
            }
            let chain_start = container_sites
                .get(first)
                .map(|&index| self.decl_sites[index].range.offset);
            let in_chain = |offset: u32| chain_start.is_none_or(|start| offset >= start);
            // Seeds — claims that never report: upstream adds the primary
            // constructor's element name before the member walk. A class or
            // enum primary already rides its own Constructor decl site (the
            // member walk's first arm claims it in offset order); an
            // extension type's representation primary registers no site, so
            // its key reads off the header's post-name tokens — `(` keys
            // `new`, `.name` the selector (`.new` the unnamed face again).
            let mut seen_ctors: FxHashSet<StringId> = FxHashSet::default();
            let extension_type = self
                .index
                .get(self.file)
                .and_then(|elements| elements.container(container))
                .is_some_and(|decl| decl.kind == ContainerKind::ExtensionType);
            if extension_type {
                for &index in &container_sites[first..] {
                    if let Some(key) = self.header_primary_key(self.decl_sites[index].range) {
                        seen_ctors.insert(key);
                        break;
                    }
                }
            }
            // The member entries in declaration order: every constructor
            // site and bare-factory head of the chain, keyed and spanned by
            // the upstream head-shape rules (see `dup_ctor_entry` /
            // `dup_factory_entry`). An `augment` member merges into the
            // introductory element — never a second declaration.
            let mut ctor_keys: Vec<(StringId, SourceRange)> = Vec::new();
            for site in &self.decl_sites {
                if site.is_augment
                    || !matches!(
                        site.kind,
                        DeclKind::Constructor { container: c } if c == container
                    )
                    || !in_chain(site.range.offset)
                {
                    continue;
                }
                if let Some(entry) = self.dup_ctor_entry(site, container, new_name) {
                    ctor_keys.push(entry);
                }
            }
            ctor_keys.extend(
                self.unnamed_factory_heads
                    .iter()
                    .filter(|(c, _, range)| *c == container && in_chain(range.offset))
                    .filter_map(|entry| self.dup_factory_entry(*entry)),
            );
            // The bare `factory (` head (primary-constructor syntax) fires no
            // identifier event — it registers neither a decl site nor an
            // `unnamed_factory_heads` tuple — so the pass finds it on the
            // token stream: `factory`+`(` at member level whose modifier run
            // reaches a member boundary claims the unnamed face at the
            // `factory` keyword (the `errorRange` for a nameless factory
            // head — `typeName ?? (newKeyword ?? factoryKeyword)` alone).
            if let Some(name_range) = container_name
                && in_chain(name_range.offset)
            {
                ctor_keys.extend(
                    self.bare_factory_heads(name_range)
                        .into_iter()
                        .map(|range| (new_name, range)),
                );
            }
            ctor_keys.sort_by_key(|(_, range)| range.offset);
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
        // Upstream `_forInterface` keeps the enum's own `index` out of the
        // required-initialization set (`element is EnumElement && field.name
        // == 'index'`, constructor_fields_verifier.dart:120-121): the
        // implicit enum index owns no initialization obligation, and a
        // same-named declaration collides with it (its own row is the
        // override/duplicate face).
        let enum_index = self
            .index
            .get(self.file)
            .and_then(|elements| elements.container(container))
            .is_some_and(|decl| decl.kind == ContainerKind::Enum);
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
            if enum_index && self.strings.resolve(check.name) == "index" {
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
                // The staged entry's written-head gate is re-decided on the
                // collected declared type: `FutureOr<T>` is nullable when T
                // is (`isNullable`, type_system.dart:1220-1233 —
                // `FutureOr<int?> v` is no required-initialization face), a
                // verdict the name-level written-type arm cannot reach. An
                // uncaptured type keeps that arm's verdict.
                let var_ty = self
                    .index
                    .get(self.file)
                    .and_then(|elements| elements.container(container))
                    .and_then(|decl| decl.members.get(&check.name))
                    .map(|entry| entry.var_ty);
                if var_ty.is_some_and(|ty| ty != Ty::Unknown && self.written_nullable(ty)) {
                    continue;
                }
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
            // explicit super satisfaction — an entry in `ctor_super_init`,
            // fed exclusively by an actual `super(...)`/`super.n(...)`
            // initializer item (upstream early-returns on a
            // `SuperConstructorInvocation` /
            // `RedirectingConstructorInvocation` initializer alone,
            // error_verifier.dart:7182-7189; a redirecting constructor's
            // own site never reaches this loop, the `ctor_redirects`
            // filter) — reports at the head when the superclass's unnamed
            // constructor still misses arguments. Upstream counts the
            // required positionals and the required named spellings
            // (:7225-7232), subtracts what the constructor's own super
            // formals supply (`verifySuperFormalParameters` — each
            // positional super formal supplies its positional argument,
            // each named one its spelling, :7244-7250) and reports when
            // required positionals outnumber the supplied count or a
            // required named spelling survives (:7252-7258). The supplies
            // read off the captured formal table's super-formal markers: a
            // positional super forward's `super.forward` default marker
            // (positional formals cannot declare defaults, so the slot is
            // free) and an unannotated named super formal's `Ty::Never`
            // type sentinel — an annotated named super formal carries its
            // written type and reads as unsupplied (a known capture gap;
            // the supply under-reports, the face over-reports there).
            if !self.ctor_super_init.contains(site) {
                let superclass = self
                    .index
                    .get(self.file)
                    .and_then(|elements| elements.container(container))
                    .and_then(|decl| decl.superclass);
                let super_formals = superclass.and_then(|superclass| {
                    self.index
                        .get(self.file)
                        .and_then(|elements| elements.container(superclass))
                        .and_then(|decl| decl.ctor_formals.get(&superclass))
                        .map(|formals| &**formals)
                });
                if let Some((superclass, super_formals)) = superclass.zip(super_formals) {
                    let mut required_named: FxHashSet<StringId> = super_formals
                        .named
                        .iter()
                        .filter(|formal| formal.required)
                        .map(|formal| formal.name)
                        .collect();
                    let required_positional = super_formals
                        .positional
                        .iter()
                        .filter(|formal| formal.required)
                        .count();
                    if required_positional > 0 || !required_named.is_empty() {
                        // The subclass constructor's own super-formal
                        // supplies, keyed under its own lookup spelling (the
                        // class name for the unnamed/primary face, the
                        // selector for a named one — the same key
                        // `check_super_formal` reads the target's table
                        // under). A table the capture cannot model supplies
                        // nothing (the same under-capture posture
                        // `check_super_formal` takes).
                        let supplies = self
                            .index
                            .get(self.file)
                            .and_then(|elements| elements.container(container))
                            .and_then(|decl| decl.ctor_formals.get(&self.decl_sites[*site].name))
                            .map(|formals| &**formals);
                        let super_forward = self.intern("super.forward");
                        let supplied_positional = supplies
                            .map(|formals| {
                                formals
                                    .positional
                                    .iter()
                                    .filter(|formal| formal.default_value == Some(super_forward))
                                    .count()
                            })
                            .unwrap_or(0);
                        if let Some(formals) = supplies {
                            for formal in
                                formals.named.iter().filter(|formal| formal.ty == Ty::Never)
                            {
                                required_named.remove(&formal.name);
                                required_named.remove(&self.formal_public_name(formal.name));
                            }
                        }
                        if required_positional > supplied_positional || !required_named.is_empty() {
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

    /// The token immediately BEFORE the one starting at `offset` (UTF-16) —
    /// the `duplicate_constructor` head-shape walk's step.
    fn dup_token_before(&self, offset: u32) -> Option<SourceRange> {
        let tokens = self.tokens.as_slice();
        let at = tokens.partition_point(|token| token.utf16_offset < offset);
        let token = tokens.get(at.checked_sub(1)?)?;
        Some(SourceRange {
            offset: token.utf16_offset,
            length: self.utf16_of((token.byte_offset + token.byte_len) as usize)
                - token.utf16_offset,
        })
    }

    /// The first token starting at or after `offset` (UTF-16).
    fn dup_token_at(&self, offset: u32) -> Option<SourceRange> {
        let tokens = self.tokens.as_slice();
        let at = tokens.partition_point(|token| token.utf16_offset < offset);
        let token = tokens.get(at)?;
        Some(SourceRange {
            offset: token.utf16_offset,
            length: self.utf16_of((token.byte_offset + token.byte_len) as usize)
                - token.utf16_offset,
        })
    }

    /// Whether the container declaration whose name sits at `name_range` is
    /// an `augment` fragment: the header keywords scan backwards to the
    /// declaration boundary — `augment` is the only latch the
    /// `duplicate_constructor` element-chain needs (upstream keys the
    /// context on `element.firstFragment`, shared by every fragment).
    fn decl_head_is_augment(&self, name_range: SourceRange) -> bool {
        let mut offset = name_range.offset;
        loop {
            let Some(prev) = self.dup_token_before(offset) else {
                return false;
            };
            match self.lexeme_at(prev.offset) {
                Some("augment") => return true,
                Some(
                    "class" | "mixin" | "enum" | "extension" | "type" | "abstract" | "base"
                    | "final" | "interface" | "sealed",
                ) => offset = prev.offset,
                _ => return false,
            }
        }
    }

    /// The primary constructor's `duplicate_constructor` key for the
    /// container header whose name sits at `name_range`: `new` for an
    /// unnamed or `.new` head, the selector's interned name for a `.named`
    /// one — `None` when the header carries no primary (an `augment`
    /// fragment or a formals-less plain header). Upstream reads
    /// `primaryConstructor.declaredFragment!.element.name`, which is
    /// exactly the header's post-name `(`/`.selector` shape.
    fn header_primary_key(&self, name_range: SourceRange) -> Option<StringId> {
        let new_name = self.intern("new");
        let mut head = self.dup_token_at(name_range.end())?;
        if self.lexeme_at(head.offset) == Some("<") {
            // Skip the written type-parameter group to its matching `>`.
            let mut depth = 0usize;
            let mut offset = head.offset;
            loop {
                let token = self.dup_token_at(offset)?;
                for ch in self.lexeme_at(token.offset).unwrap_or("").chars() {
                    match ch {
                        '<' => depth += 1,
                        '>' => depth = depth.saturating_sub(1),
                        _ => {}
                    }
                }
                offset = token.end();
                if depth == 0 {
                    break;
                }
            }
            head = self.dup_token_at(offset)?;
        }
        match self.lexeme_at(head.offset) {
            Some("(") => Some(new_name),
            Some(".") => {
                let selector = self.token_after(head.offset)?;
                match self.lexeme_at(selector.offset) {
                    Some("new") | None => Some(new_name),
                    Some(text) => Some(self.intern(text)),
                }
            }
            _ => None,
        }
    }

    /// One constructor site's `duplicate_constructor` entry — `(key,
    /// errorRange)` — or `None` when upstream's pass skips the member
    /// outright (`typeName` is present and is not the enclosing class's
    /// name: `A.foo` inside `B`, `new.named`, `factory.someName`).
    ///
    /// The site slot only records the written name token, so the head's
    /// shape is re-read off the token stream: the token before it decides
    /// the arm — `.` a type-name selector head, `new` a `new`-keyword head,
    /// anything else a plain head (which must then spell the container
    /// itself, else the upstream typeName gate skips it).
    fn dup_ctor_entry(
        &self,
        site: &DeclSite,
        container: StringId,
        new_name: StringId,
    ) -> Option<(StringId, SourceRange)> {
        let at = site.range;
        let lexeme = self.lexeme_at(at.offset);
        // The header primary (`class C(...)`, `enum E.named(...)`): the
        // site's own range is the `(`/`.` token — it claims the element
        // name (upstream's `constructorNames.add(element.name)`), never a
        // row of its own.
        if site.name == container {
            match lexeme {
                Some("(") => return Some((new_name, at)),
                Some(".") => {
                    let key = self
                        .token_after(at.offset)
                        .and_then(|sel| self.lexeme_at(sel.offset))
                        .map_or(new_name, |text| {
                            if text == "new" {
                                new_name
                            } else {
                                self.intern(text)
                            }
                        });
                    return Some((key, at));
                }
                _ => {}
            }
        }
        // Recovery debris (`this.new = e`, `this.x = e`, `new = e`): the
        // recovered member upstream names the identifier before `=` — the
        // `new` spelling keys the unnamed face, any other identifier its
        // own name; the report range is that identifier alone.
        if lexeme == Some("=") {
            let prev = self.dup_token_before(at.offset)?;
            let text = self.lexeme_at(prev.offset)?;
            let key = if text == "new" {
                new_name
            } else {
                self.intern(text)
            };
            return Some((key, prev));
        }
        let prev = self.dup_token_before(at.offset)?;
        match self.lexeme_at(prev.offset) {
            // `T.name` — the type name must be the enclosing class (the
            // upstream `typeName != firstFragment.name` skip); the range
            // is the type name through the selector.
            Some(".") => {
                let tname = self.dup_token_before(prev.offset)?;
                if self.lexeme_at(tname.offset) != Some(self.strings.resolve(container)) {
                    return None;
                }
                let key = if lexeme == Some("new") {
                    new_name
                } else {
                    site.name
                };
                Some((
                    key,
                    SourceRange {
                        offset: tname.offset,
                        length: at.end().saturating_sub(tname.offset),
                    },
                ))
            }
            // The `new`-keyword head (`new()`, `new named()`, `new new()`):
            // `errorRange` is `newKeyword` through the name — the name
            // slot's `(` spelling keys the unnamed face exactly like a
            // null name upstream; a `new` name still spells `new new`.
            // The `new.name` recovery's site sits on the `.` slot when the
            // selector is a plain identifier (the `.` arm above drops it as
            // a `new`-spelled typeName); a `new` selector lands here as a
            // `(`-slot site whose `prev` is the NAME token — a `.` before
            // it means the head is `new.new` (newKeyword through name).
            Some("new") => {
                if lexeme == Some(".") {
                    return None;
                }
                let key = if lexeme == Some("(") {
                    new_name
                } else {
                    site.name
                };
                let range = if lexeme == Some("(") {
                    match self
                        .dup_token_before(prev.offset)
                        .and_then(|dot| (self.lexeme_at(dot.offset) == Some(".")).then_some(dot))
                        .and_then(|dot| self.dup_token_before(dot.offset))
                    {
                        // `new.new()` — the keyword is the `new` ahead of
                        // the dot, the name is the `new` ahead of `(`.
                        Some(kw) if self.lexeme_at(kw.offset) == Some("new") => SourceRange {
                            offset: kw.offset,
                            length: prev.end().saturating_sub(kw.offset),
                        },
                        // `new()` — a null name upstream; the keyword alone.
                        _ => prev,
                    }
                } else {
                    SourceRange {
                        offset: prev.offset,
                        length: at.end().saturating_sub(prev.offset),
                    }
                };
                Some((key, range))
            }
            // A plain head: `C()` spells the enclosing class and keys the
            // unnamed face; any other identifier is a wrong typeName
            // (upstream skips) or a non-constructor member the recovery
            // captured (`get`/`set` heads).
            _ => {
                if site.name == container {
                    Some((new_name, site.range))
                } else {
                    None
                }
            }
        }
    }

    /// One bare-factory head's `duplicate_constructor` entry — the same
    /// `(key, errorRange)` as [`Self::dup_ctor_entry`]. `None` when the
    /// declaration is an `augment factory` (the member merges into the
    /// introductory element) or the head anchor cannot be attributed.
    ///
    /// Upstream reads the head off the AST: `factory C()` spells a
    /// typeName so the range is `C` alone; `factory new()` names the
    /// constructor `new` so the range is `factory` through it;
    /// `factory()` has neither, the `factory` keyword alone;
    /// `factory name()` spans `factory name`.
    fn dup_factory_entry(
        &self,
        (container, key, head): (StringId, StringId, SourceRange),
    ) -> Option<(StringId, SourceRange)> {
        let new_name = self.intern("new");
        let prev = self.dup_token_before(head.offset)?;
        if self.lexeme_at(prev.offset) != Some("factory") {
            // A head anchor the `factory`-adjacent shape cannot explain —
            // keep the captured verdict rather than invent a span.
            return Some((key, head));
        }
        // `augment` before `factory` (through `external`/`const`): the
        // member merges into the introductory element — never a
        // declaration of the selector.
        let mut at = prev.offset;
        while let Some(before) = self.dup_token_before(at) {
            match self.lexeme_at(before.offset) {
                Some("const") | Some("external") => at = before.offset,
                Some("augment") => return None,
                _ => break,
            }
        }
        let head_text = self.lexeme_at(head.offset)?;
        let joined = |end: SourceRange| SourceRange {
            offset: prev.offset,
            length: end.end().saturating_sub(prev.offset),
        };
        if head_text == self.strings.resolve(container) {
            Some((new_name, head))
        } else if head_text == "(" {
            Some((new_name, prev))
        } else if head_text == "new" {
            Some((new_name, joined(head)))
        } else {
            Some((self.intern(head_text), joined(head)))
        }
    }

    /// The `duplicate_constructor` ranges of the declaration's bare
    /// `factory (` heads — the primary-constructor-syntax unnamed factory.
    /// The parser's unnamed-factory arm fires `handle_no_identifier` (no
    /// name event reaches the engine), so no decl site and no
    /// `unnamed_factory_heads` tuple ever records the shape; the pass
    /// re-derives it from the token stream: a `factory` whose follower is
    /// `(` and whose modifier run walks back to a member boundary
    /// (`{`, `}`, `;` through `external`/`const`) at member depth. The
    /// return is the `factory` keyword's own range — the upstream
    /// `errorRange` for a nameless factory head.
    fn bare_factory_heads(&self, name_range: SourceRange) -> Vec<SourceRange> {
        let tokens = self.tokens.as_slice();
        // The declaration body's `{`: `(...)`/`<...>` groups in the header
        // never fake it, and a `;` header (no body) ends the scan.
        let mut index = tokens.partition_point(|token| token.utf16_offset < name_range.end());
        let body_open = loop {
            let Some(_) = tokens.get(index) else {
                return Vec::new();
            };
            match self.tokens.lexeme(self.source, index) {
                "{" => break index,
                "(" | "[" | "<" => {
                    index = self
                        .tokens
                        .matching_end(index)
                        .map_or(index + 1, |end| end + 1);
                }
                ";" => return Vec::new(),
                _ => index += 1,
            }
        };
        let body_close = self
            .tokens
            .matching_end(body_open)
            .unwrap_or(tokens.len().saturating_sub(1));
        let mut heads = Vec::new();
        let mut depth = 0usize;
        let mut i = body_open + 1;
        while i < body_close {
            match self.tokens.lexeme(self.source, i) {
                // Literal/expression braces at member level only raise the
                // bar for the `factory (` head — expression positions never
                // declare constructors.
                "{" => depth += 1,
                "}" => depth = depth.saturating_sub(1),
                "factory" if depth == 0 => {
                    let follower_is_formals = tokens
                        .get(i + 1)
                        .is_some_and(|_| self.tokens.lexeme(self.source, i + 1) == "(");
                    if follower_is_formals {
                        // The member-start verdict: `external`/`const` ride
                        // along (`external const factory ()`), an `augment`
                        // or any non-modifier head answers NO (a typed
                        // `T factory()` is a method, `static factory()` the
                        // identifier shape the parser's gate rejects).
                        let mut at = tokens[i].utf16_offset;
                        let declares = loop {
                            let Some(prev) = self.dup_token_before(at) else {
                                break false;
                            };
                            match self.lexeme_at(prev.offset) {
                                Some("external") | Some("const") => at = prev.offset,
                                Some("augment") => break false,
                                Some("{") | Some("}") | Some(";") => break true,
                                _ => break false,
                            }
                        };
                        if declares {
                            heads.push(SourceRange {
                                offset: tokens[i].utf16_offset,
                                length: 7,
                            });
                        }
                    }
                }
                _ => {}
            }
            i += 1;
        }
        heads
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
        // of an executable's interior stays out EXCEPT the head region:
        // the return annotation (and the formal list of a method/
        // getter/setter) parses after `begin_top_level_method`/`begin_method`
        // pushed the Executable frame but before the body opens —
        // `T Function<T>(T, T) get foo` must still capture or the latched
        // return degrades to `FnShapeId::EMPTY` = `dynamic Function()`
        // (the `direct_function_typed_return_spelling_stays_silent` pin
        // covers the BODY-position face only). The head is the window where
        // an Executable frame is open but no body event has run: no
        // `body_brace` (the block `{` latch) and no `body_mark` (the
        // async-modifier latch that opens BOTH the `=>` and `{` body
        // windows — `handle_async_modifier` fires unconditionally).
        let head_region = self.frames.iter().rev().find_map(|f| match &f.kind {
            FrameKind::Executable { body_brace, .. } => Some(body_brace.is_none()),
            _ => None,
        }) == Some(true)
            && self.body_mark.is_none();
        if self.formal_list_depth == 0
            && !self.in_block_body()
            && !head_region
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
        // The written `<T, U extends B>` group rebuilds from the bounds
        // window latch when it is this capture's own (#3384): the parser
        // completes the variables in REVERSE declaration order, so the
        // captured slice spells the group backwards, and the shared
        // armed slot loses each bound's explicit arguments (while the
        // window is open `Bounds::end_group` routes the group to the
        // bound window, so the completed-group side channel never arms
        // and the head re-fires bare). The latch carries DECLARATION
        // order (`Bounds::end_type_variables` reverses it back) with the
        // full written bound trees, mapped through `tree_ty` exactly
        // like `pair_exec_window`. The name guard's invariant: a NESTED
        // group's close takes the `frames` branch in
        // `Bounds::end_type_variables` and never touches `pending`, so
        // the latch is either this capture's own window or an older
        // declaration's stale one — names decide, and the fallback keeps
        // today's completion-order slice behaviour-identical.
        let type_params: Vec<(StringId, Option<Ty>)> = match self.fn_type_capture.last() {
            Some(capture) => {
                let names: Vec<StringId> = capture.params.iter().map(|(name, _)| *name).collect();
                self.window_type_params(&names)
                    .unwrap_or_else(|| capture.params.clone())
            }
            None => Vec::new(),
        };
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
        // The written required-positional split rides the shape: the
        // canonical `FnShape` identity carries the `[...]` boundary
        // (`int Function(int)` and `int Function([int])` are distinct
        // types upstream — `_equalParameters` compares each formal's
        // optional flag), so the signature's own count is part of the
        // intern key rather than a side table's guess.
        let required_positional = capture.required_positional.min(u16::MAX as usize) as u16;
        // The written `<T, U extends B>` group rides the shape (#2878):
        // the message slots spell it back (`void Function<T>(T)`), the
        // official `FunctionType` display form. #3384: the group is the
        // window-rebuilt slice (declaration order, bounds with their
        // written arguments) when the latch matched, else the captured
        // slice as before.
        let fn_ty = self.boundary.args.borrow_mut().function_generic_required(
            &type_params,
            required_positional,
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
        // A written function type standing as a call-site type argument
        // (`f2<void Function(), String>()`) joins the open group as ONE
        // member here. A function type fires no `handle_type` leaf, so
        // without this hand-over the group holds one member fewer and every
        // LATER argument shifts one parameter left: `f2<String,
        // void Function()>()` judged `String` against the FIRST parameter
        // and never checked the second at all, while `f2<List<int>
        // Function(), String>()` seated the return type's own `int` as the
        // first member. The member is complete at this close (a function
        // type never re-fires a parameterized head), so the hand-over
        // clears the merge latch and appends once. The extent is the
        // written annotation `_typeArgumentErrorNode` anchors — the return
        // type's own head through the formals' `)` (#3571). A return the
        // grammar leaves unwritten (`f2<Function(), String>()`) or that is
        // itself a function type — the inner close seats it through
        // `fn_capture_type` with no head offset (`f2<int Function()
        // Function()>()`) — has no anchor of its own: the written start
        // comes from the backward chain scan, and a chain the grammar
        // would not admit keeps the under-report. The trailing `?`
        // belongs to the type and joins the extent
        // (`f2<void Function()?>()` anchors its `?`).
        if self.fn_type_capture.is_empty()
            && let Some(closer_end) = self.fn_formal_closer_end
            && self
                .call_args_capture
                .as_ref()
                .is_some_and(|args| args.depth == self.type_arg_depth)
        {
            let anchor = match capture.ret {
                Some((_, leaf, start)) if leaf != StringId::NONE || start != 0 => Some(start),
                // A return-less spelling: the chain head is the start. A
                // function-typed return: the chain head extends to the
                // return's own head (`int Function() Function()` starts at
                // `int`).
                Some(_) => self.written_function_chain_start(closer_end, true),
                None => self.written_function_chain_start(closer_end, false),
            };
            if let Some(start) = anchor
                && closer_end > start
            {
                let end = if nullable {
                    self.nullable_suffix_end(closer_end)
                } else {
                    closer_end
                };
                if end > start {
                    self.call_args_push_completed(
                        fn_ty,
                        SourceRange {
                            offset: start,
                            length: end - start,
                        },
                    );
                }
            }
        }
        // A body-position capture (#2359) arms its shape for the
        // annotation/test faces but claims no variable-signature stash: the
        // claim sites are formals and container-level groups, and a stash
        // surviving into the body would be stolen by the next closure
        // formal's claim (the stale-attribution direction).
        if capture.poisoned || self.in_block_body() {
            return;
        }
        self.fn_type_stash = Some(Box::new(FnVarSig {
            // #3384: the same rebuilt slice rides the stash —
            // `fn_sig_display` spells the group from it, so the
            // signature-display face inherits the declaration-order
            // fix with the armed-slot face.
            params: type_params.into_boxed_slice(),
            required_positional,
            positional: capture.positional.into_boxed_slice(),
            named: capture.named.into_boxed_slice(),
            return_ty,
        }));
    }

    /// The capture's own type-variable group rebuilt from the bounds
    /// window latch: written order, each bound mapped through
    /// [`Engine::tree_ty`] (`pair_exec_window`'s exact mapping — the
    /// same recovery posture for a recovered bound tree). `None` = no
    /// latch, or the latch names another group — the caller keeps its
    /// captured slice.
    fn window_type_params(&self, expected: &[StringId]) -> Option<Vec<(StringId, Option<Ty>)>> {
        let window = self.bounds.pending_window()?;
        // The latch carries written order; the capture pushed in parser
        // completion order — the reverse.
        let completion: Vec<StringId> = window.iter().rev().map(|(name, _, _)| *name).collect();
        if completion != expected {
            return None;
        }
        Some(
            window
                .iter()
                .map(|(name, bound, _)| (*name, bound.as_ref().map(|tree| self.tree_ty(tree))))
                .collect(),
        )
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
    /// formal list's own group. The no-capture arm also latches the
    /// frame's own list's section split for the local-function tear-off
    /// capture (#2842): `[` records the required-formal count so far (the
    /// optional-positional boundary), `{` poisons the capture (no
    /// named-formal face). A nested list — a function-typed formal's own
    /// parameters — is not the frame's.
    pub(crate) fn fn_capture_optional_group(&mut self, opener: &str) {
        if let Some(capture) = self.fn_type_capture.last_mut() {
            capture.section = if opener == "[" {
                FnFormalSection::OptionalPositional
            } else {
                FnFormalSection::Named
            };
        } else {
            self.fn_formal_named_section = opener == "{";
            // The frame's own list (#2842): the section split latches on
            // the executable frame the formals bind into.
            if self.formal_list_depth == 1 {
                let named = opener == "{";
                let required = (!named).then(|| {
                    let mark = *self.frame_local_marks.last().unwrap_or(&0);
                    let count = self.locals[mark..]
                        .iter()
                        .filter(|decl| matches!(decl.kind, LocalKind::Parameter { .. }))
                        .count();
                    u16::try_from(count).unwrap_or(u16::MAX)
                });
                if let Some(frame) = self.frames.last_mut() {
                    if named {
                        frame.formal_named = true;
                    } else {
                        frame.formal_optional_from = required;
                    }
                }
            }
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
        // An UNWRITTEN formal type (`void f(x)`, `f([x])`, `f({x})`, a
        // nested `f(void g(x))` inner formal) arms nothing — the upstream
        // default is `dynamic` (`visitRegularFormalParameter`:
        // `node.type == null` → `element.type = _typeProvider.dynamicType`,
        // resolution_visitor.dart:808-813), NOT a poisoned capture. The
        // capture keeps the slot at `dynamic` so the function-type shape
        // renders `Function(dynamic)` like the oracle.
        let ty = capture.pending_formal.take().unwrap_or(Ty::Dynamic);
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
    /// starts the literal's signature capture. `ctx` is the matched
    /// context function type the enclosing site supplied
    /// ([`Engine::closure_context_ty`]) — `None` on every non-function
    /// context (the oracle's no-inference arm).
    pub(crate) fn closure_capture_begin(
        &mut self,
        offset: u32,
        ctx: Option<Ty>,
        ctx_from_arg: bool,
    ) {
        self.closure_caps.push(ClosureCapture {
            offset,
            frame_depth: self.frames.len(),
            list_depth: 0,
            list_open: false,
            positional: Vec::new(),
            optional_from: None,
            poisoned: false,
            ctx,
            ctx_from_arg,
            type_params: 0,
        });
    }

    /// The literal's own `<T…>` variable closed (the group sits between
    /// `begin_function_expression` and the literal's formal list): the
    /// upstream match gate pairs this count against the context's
    /// type-parameter count at close time. A nested type-annotation's
    /// variable reports against its own capture, not the literal's —
    /// those live inside the literal's formal list (`list_depth > 0`)
    /// or inside a deeper frame, and the two guards exclude both.
    pub(crate) fn closure_type_variable(&mut self) {
        let frames = self.frames.len();
        if let Some(capture) = self.closure_caps.last_mut()
            && capture.frame_depth == frames
            && capture.list_depth == 0
            && self.fn_type_capture.is_empty()
        {
            capture.type_params = capture.type_params.saturating_add(1);
        }
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

    /// A formal of the literal's own list: its resolved type joins the
    /// capture — the written type when present, else the context schema's
    /// downward answer (`greatestClosure(Qi)`, `Object?` for `Qi <: Null`,
    /// `dynamic` when the context supplies nothing —
    /// `inferLocalFunction`'s `Ri`). An untyped formal under no context
    /// keeps `dynamic` (`inferLocalFunction` with no formal-inference
    /// source — the parameter's downward answer IS `dynamic`): the
    /// literal's own signature still lands instead of poisoning.
    pub(crate) fn closure_formal(&mut self) {
        let depth = self.formal_list_depth;
        let ty = self.armed_ty;
        // The formal's ordinal and the context answer resolve before the
        // mutable borrow of the capture stack.
        let (index, ctx) = match self.closure_caps.last() {
            Some(capture)
                if capture.list_open
                    && capture.frame_depth == self.frames.len()
                    && capture.list_depth == depth =>
            {
                (capture.positional.len(), capture.ctx)
            }
            _ => return,
        };
        let resolved = match ty {
            Some(ty) if ty != Ty::Unknown => Some(ty),
            _ => match ctx {
                Some(ctx) => Some(
                    self.closure_ctx_formal_ty(ctx, index)
                        .unwrap_or(Ty::Dynamic),
                ),
                // No context: the untyped formal keeps `dynamic` — the
                // literal's capture stays un-poisoned.
                None => Some(Ty::Dynamic),
            },
        };
        let Some(capture) = self.closure_caps.last_mut() else {
            return;
        };
        match resolved {
            Some(ty) => capture.positional.push(ty),
            None => capture.poisoned = true,
        }
    }

    /// `end_function_expression` (the literal's frame still open): closes
    /// the capture and hands an exact signature to the enclosing argument
    /// list. A suspends body (`async`/`sync*`/`async*`) changes the
    /// literal's static return — `Future<…>` through the capture-ctx
    /// rewrite (`note_async_modifier`), `Iterable`/`Stream` for the
    /// generators — so the literal keeps only the typed row and drops the
    /// `void` stub (whose re-type-to-formal-return mechanism is the sync
    /// cap and would silently undo the wrap). Returns the literal's span
    /// start plus the final inferred type when
    /// a matched context `K` supplied one (`inferLocalFunction`'s
    /// `(R0, …, Rn) -> M'`: written-and-inferred formals, the context
    /// return under the cap `S = T if T <: R else S = R` — the body's
    /// returns already re-checked against `R`, so `S = R` is the face the
    /// cap produces on this name-level walk, keeping the failure the
    /// closure-return row's and never the enclosing assignability row's).
    /// With no context, a plain `=>` literal still answers its own static
    /// type — flagged `false` (not context-supplied), so the listener
    /// types only the slots that consume it.
    pub(crate) fn closure_capture_end(&mut self, end: u32) -> Option<(u32, Ty, bool)> {
        let capture = self.closure_caps.pop()?;
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
        // The literal's own body modifier reads off its still-open frame.
        // A suspends body changes the literal's static return: `async`
        // wraps the downward context's return through `Future<flatten(R)>`
        // (the rewrite already sits on the capture's ctx —
        // `note_async_modifier`), and a generator body returns
        // `Iterable`/`Stream`. The `void` stub's whole mechanism is the
        // sync cap (`S = R` — the argument face re-types the stub's return
        // to the formal's before asking), which is FALSE for a suspends
        // literal: the re-typed stub would silently undo the wrap and
        // erase the row the oracle reports (`g(() async {})` over
        // `int Function()` answers `Future<Null> Function()`, never the
        // re-typed `int Function()`). A suspends literal skips the stub —
        // the argument channel then decides through the ordinary ask on
        // the typed row's wrapped face.
        let (suspends, is_async, generator, arrow_body) =
            match self.frames.last().map(|frame| &frame.kind) {
                Some(FrameKind::Executable {
                    is_async,
                    generator,
                    body_brace,
                    ..
                }) => (
                    *is_async || *generator,
                    *is_async,
                    *generator,
                    body_brace.is_none(),
                ),
                _ => (false, false, false, false),
            };
        // The upstream match gate closes at the literal's own group count
        // (`typeContext.typeParameters.length == functionTypeParameters
        // .length`): a counted literal under a mismatched-count context
        // acquires nothing — the context face stays the under-report
        // posture every channel carried before.
        let ctx = capture
            .ctx
            .filter(|ctx| self.closure_ctx_matches(*ctx, capture.type_params));
        if capture.poisoned || capture.list_depth == 0 {
            return None;
        }
        let mut bare = None;
        if !suspends {
            // The `=>` body's inferred return: the body's own expression row
            // still sits on the stack (the listener drains it only after
            // this close — `end_function_expression`'s `deferred_miss_rows`
            // hand-off), so its resolved static type answers the captured
            // signature's return where the `void` STUB stood. Upstream's
            // `staticType` is that same answer: `xs.map((t) => t.element)`
            // feeds `int Function(Element)` to the enclosing send's
            // upward constraint, solving `map`'s own `R` — the downward
            // formal inference's fixpoint return leg (`generic_inferrer`'s
            // `constrainArgument` on the literal's inferred type). The
            // `=>` body's top-level row is its LAST completed expression
            // row (merges fold receiver rows into the member/send row); a
            // block body's `return` tally keeps the `void` stub (the
            // `S = R` re-type mechanism its consumers already run). A row
            // that resolves `Unknown` — a deferred-formal re-bind the
            // pass-2 schema will only correct later — keeps the stub too,
            // the same under-report the face carried before.
            let body_row = arrow_body
                .then(|| {
                    self.rows.iter().rev().find(|row| {
                        row.range.offset >= capture.offset
                            && matches!(
                                row.intent,
                                Intent::Read | Intent::Invoke | Intent::Operator(..)
                            )
                            && !matches!(row.shape, RowShape::TypeUse(..))
                            && !self
                                .interpolation_child_rows
                                .contains(&row.range.offset)
                            // A cascade body's static type is the TARGET
                            // (`x..add(1)` reads `x`) — the section rows
                            // sit last but answer only their own selector,
                            // so they never speak for the body.
                            && !self.cascade_sections.contains(&row.range.offset)
                    })
                })
                .flatten()
                .cloned();
            let return_ty = match &body_row {
                // A member READ row's `ty` is lazily empty (the merge
                // leaves the declared-type write to the read faces) —
                // `initializer_read_ty` re-derives the accessor answer
                // through the classified receiver; sends/operators read
                // the folded face `expression_row_ty` carries.
                Some(row) if row.intent == Intent::Read => match self.initializer_read_ty(row) {
                    Ty::UNKNOWN => self.expression_row_ty(row),
                    ty => ty,
                },
                Some(row) => self.expression_row_ty(row),
                None => Ty::Void,
            };
            // The context-free literal's own static type
            // (`FunctionExpressionImpl.staticType` under no imposed context:
            // the written-or-`dynamic` formals and the `=>` body's type —
            // `var g = () => 0` is `int Function()`). Answered only when the
            // body's row IS the whole body (it starts at the body's first
            // token): a composite body whose newest row is a trailing operand
            // (`() => c ? 1 : 'a'`, `() => a ?? b`) keeps the untyped face, as
            // do the generic and optional-group literals whose shapes this
            // capture does not spell.
            if ctx.is_none()
                && capture.type_params == 0
                && capture.optional_from.is_none()
                && return_ty != Ty::UNKNOWN
                && !self.is_invalid_ty(return_ty)
                && let Some(row) = &body_row
            {
                let bytes = self.source.as_bytes();
                let mut body = self.byte_of(end);
                while bytes.get(body).is_some_and(u8::is_ascii_whitespace) {
                    body += 1;
                }
                if self.byte_of(row.range.offset) == body {
                    bare = Some(self.boundary.args.borrow_mut().function(
                        0,
                        return_ty,
                        &capture.positional,
                        &[],
                        false,
                    ));
                }
            }
            let required = capture.optional_from.unwrap_or(capture.positional.len());
            let sig = Box::new(FnVarSig {
                params: Box::default(),
                required_positional: u16::try_from(required).unwrap_or(u16::MAX),
                positional: capture.positional.clone().into_boxed_slice(),
                named: Box::default(),
                return_ty,
            });
            if let Some(level) = self.arg_levels.last_mut() {
                level.closures.push((capture.offset, end, sig));
            }
        }
        // The final literal type rides the pushed Value row: the context's
        // return under the cap over the literal's own resolved formals.
        // A `void`-returning consumer admits the `void` stub above; the
        // typed row carries the context's real return so the assignability
        // and return channels read the same face the oracle infers.
        ctx.and_then(|ctx| {
            let ret = self.function_shape_return(ctx)?;
            // A generator body's static return is `Iterable<T>`/`Stream<T>`
            // (`sync*` spells `Iterable`, `async*` `Stream`). The element
            // type infers from the body's yields; the imposed context's
            // own `Iterable`/`Stream`-headed return supplies it ahead of
            // that (`Stream<int> Function() f = () async* {}` infers the
            // `int` element — the official's downward element inference),
            // and an element-less context answers the empty-body
            // `dynamic` (`() sync* {}` alone answers `Iterable<dynamic>
            // Function()`).
            let ret = if generator {
                let element = match ret {
                    Ty::Named { name, args, .. }
                        if args != ArgListId::EMPTY
                            && (name == self.intern("Iterable")
                                || name == self.intern("Stream")) =>
                    {
                        self.boundary
                            .pool()
                            .get(args)
                            .first()
                            .copied()
                            .unwrap_or(Ty::Dynamic)
                    }
                    _ => Ty::Dynamic,
                };
                let head = self.intern(if is_async { "Stream" } else { "Iterable" });
                let args = self.boundary.intern_list(&[element]);
                Ty::Named {
                    name: head,
                    nullable: false,
                    args,
                }
            } else {
                ret
            };
            let ty =
                self.boundary
                    .args
                    .borrow_mut()
                    .function(0, ret, &capture.positional, &[], false);
            Some((capture.offset, ty, true))
        })
        .or_else(|| bare.map(|ty| (capture.offset, ty, false)))
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
    /// are annotations and ride the #610 sink). `depth` is the group's
    /// own `type_arg_depth` (the engine's counter after this group's
    /// increment).
    pub(crate) fn call_args_begin(&mut self, headed: bool, depth: u32) {
        if headed {
            return;
        }
        self.call_args_capture = Some(CallArgsCapture {
            depth,
            ..CallArgsCapture::default()
        });
    }

    /// A type completion inside the open call-argument group (the merge
    /// latch mirrors the sink's: the re-fired head replaces its own
    /// leaf). A completion at a deeper depth is a member's own nested
    /// annotation (`List<int>` inside `f<List<int>>(...)`) — never a
    /// member of this group.
    pub(crate) fn call_args_feed(
        &mut self,
        ty: Ty,
        head: StringId,
        offset: u32,
        span: SourceRange,
    ) {
        // A completion inside an open written function type is that
        // signature's own return/formal/parameter-bound part, never a
        // member of the enclosing call-site group: `f2<List<int>
        // Function(), String>()` completes `List`, `int` and the re-fired
        // `List<int>` at depths that collide with the group's own, so the
        // depth gate alone would seat `int` as the FIRST member and shift
        // every later argument one parameter left. The signature close
        // ([`Self::fn_capture_end`]) hands the whole type over as one
        // member instead.
        if !self.fn_type_capture.is_empty() {
            return;
        }
        let Some(capture) = self.call_args_capture.as_mut() else {
            return;
        };
        if capture.depth != self.type_arg_depth {
            return;
        }
        if capture
            .pending
            .is_some_and(|(prev_head, prev_offset)| prev_head != head || prev_offset != offset)
        {
            // A stale leaf whose re-fire never came stays as written.
        } else if capture.pending.take().is_some() {
            capture.members.pop();
        }
        capture.pending = Some((head, offset));
        capture.members.push((ty, span));
    }

    /// A member that completes WITHOUT a `handle_type` leaf of its own — a
    /// written function type (`f2<void Function(), String>()`): the
    /// signature close owns the whole member, so it appends as written and
    /// clears the merge latch (no re-fire can follow a closed signature).
    /// The return type parses BEFORE `begin_function_type` fires, so its
    /// completions may already have seated themselves; every one of them
    /// lies inside the closed signature's own written extent and is swept
    /// here — otherwise `List<int> Function()` would consume two parameter
    /// slots and shift the arguments after it.
    fn call_args_push_completed(&mut self, ty: Ty, span: SourceRange) {
        let Some(capture) = self.call_args_capture.as_mut() else {
            return;
        };
        if capture.depth != self.type_arg_depth {
            return;
        }
        let end = span.offset + span.length;
        capture
            .members
            .retain(|(_, member)| member.offset >= end || member.offset < span.offset);
        capture.pending = None;
        capture.members.push((ty, span));
    }

    /// The written start of a function type sealed at `closer_end` (its
    /// formals' `)` end, exclusive) whose return type carries no head
    /// anchor: the return is absent (`A<Function()>()`) or itself a
    /// function type (`A<int Function() Function()>()`). The start is the
    /// FIRST `Function` keyword of the backward written chain, each unit
    /// spelled `Function [<...>] (...)`; a chain the grammar would not
    /// admit — any residue before a unit that is not another unit's
    /// formals — keeps the under-report (`None`). With
    /// `extend_to_return_head` (a function-TYPED return: the type's start
    /// is its return's own head), the walk then takes the head that
    /// precedes the chain — its own `<...>` group (`List<int> …`) and
    /// identifier run (`int`, `p.C`) — falling back to the chain head
    /// when no head is spelled.
    fn written_function_chain_start(
        &self,
        closer_end: u32,
        extend_to_return_head: bool,
    ) -> Option<u32> {
        let bytes = self.source.as_bytes();
        let ident = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
        let mut i = self.byte_of(closer_end);
        let mut units = 0usize;
        loop {
            while i > 0 && bytes[i - 1].is_ascii_whitespace() {
                i -= 1;
            }
            if i == 0 || bytes[i - 1] != b')' {
                return None;
            }
            i -= 1;
            let mut depth = 0usize;
            loop {
                if i == 0 {
                    return None;
                }
                i -= 1;
                match bytes[i] {
                    b')' => depth += 1,
                    b'(' if depth == 0 => break,
                    b'(' => depth -= 1,
                    _ => {}
                }
            }
            while i > 0 && bytes[i - 1].is_ascii_whitespace() {
                i -= 1;
            }
            // The unit's own type-variable group (`Function<T>(...)`).
            if i > 0 && bytes[i - 1] == b'>' {
                let mut angles = 0usize;
                loop {
                    if i == 0 {
                        return None;
                    }
                    i -= 1;
                    match bytes[i] {
                        b'>' => angles += 1,
                        b'<' => {
                            angles -= 1;
                            if angles == 0 {
                                break;
                            }
                        }
                        _ => {}
                    }
                }
                while i > 0 && bytes[i - 1].is_ascii_whitespace() {
                    i -= 1;
                }
            }
            if i < 8 || &bytes[i - 8..i] != b"Function" || (i >= 9 && ident(bytes[i - 9])) {
                return None;
            }
            i -= 8;
            units += 1;
            // The chain continues when another unit's formals close right
            // before this keyword (`Function() Function()`); anything else
            // ends it — the group's `<`, a `,`, or the declaration ahead.
            let mut j = i;
            while j > 0 && bytes[j - 1].is_ascii_whitespace() {
                j -= 1;
            }
            if !(j > 0 && bytes[j - 1] == b')' && units < 8) {
                break;
            }
            i = j;
        }
        if !extend_to_return_head {
            return Some(self.utf16_of(i));
        }
        // The function-typed return's own head precedes the chain: its
        // `<...>` group first (`List<int> Function() Function()`), then the
        // identifier run (`int`, `p.C`). No head spelled — the chain head
        // is the type's own start.
        let mut j = i;
        while j > 0 && bytes[j - 1].is_ascii_whitespace() {
            j -= 1;
        }
        if j > 0 && bytes[j - 1] == b'>' {
            let mut angles = 0usize;
            loop {
                if j == 0 {
                    return Some(self.utf16_of(i));
                }
                j -= 1;
                match bytes[j] {
                    b'>' => angles += 1,
                    b'<' => {
                        angles -= 1;
                        if angles == 0 {
                            break;
                        }
                    }
                    _ => {}
                }
            }
            while j > 0 && bytes[j - 1].is_ascii_whitespace() {
                j -= 1;
            }
        }
        while j > 0 && (ident(bytes[j - 1]) || bytes[j - 1] == b'.') {
            j -= 1;
        }
        Some(self.utf16_of(j))
    }

    /// The written end when the sealed function type carries its own
    /// nullable suffix: the position after the `?` that follows the
    /// formals' `)` (whitespace between belongs to the type), else
    /// `closer_end` unchanged.
    fn nullable_suffix_end(&self, closer_end: u32) -> u32 {
        let bytes = self.source.as_bytes();
        let mut i = self.byte_of(closer_end);
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i < bytes.len() && bytes[i] == b'?' {
            self.utf16_of(i) + 1
        } else {
            closer_end
        }
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
        let (tys, spans): (Vec<Ty>, Vec<SourceRange>) = capture.members.into_iter().unzip();
        self.call_type_args
            .retain(|(offset, _)| *offset < head_offset);
        self.call_type_args.push((
            head_offset,
            CallTypeArgs {
                tys: tys.into_boxed_slice(),
                spans: spans.into_boxed_slice(),
            },
        ));
    }

    /// The explicit type arguments of the callee row, if a call-site
    /// group latched any.
    ///
    /// The named-selector creation (`c<T>.m(...)`, `C<T>.named(...)`): the
    /// headless group's entry keys on the RECEIVER row — `rows[callee - 1]`
    /// — never on the selector's own offset, so the first lookup misses and
    /// the second arm re-keys through the container segment. A bare or
    /// member callee with no container row below keeps the miss (a plain
    /// `f<T>(x)`'s entry sits at the callee's own offset already; a stale
    /// receiver row's group grounds nothing).
    pub(crate) fn call_args_for(&mut self, callee: usize) -> Option<CallTypeArgs> {
        let offset = self.rows.get(callee)?.range.offset;
        let mut pos = self
            .call_type_args
            .iter()
            .rposition(|(at, _)| *at == offset);
        if pos.is_none()
            && callee >= 1
            && let Some(container_offset) =
                self.rows.get(callee - 1).and_then(|row| match &row.shape {
                    RowShape::Bare(BareInfo::Container { .. }) => Some(row.range.offset),
                    _ => None,
                })
        {
            pos = self
                .call_type_args
                .iter()
                .rposition(|(at, _)| *at == container_offset);
        }
        let pos = pos?;
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
        // HELD-WIRING SEAM (#2910): the member-use deprecation/experimental
        // pass runs here because every natural call slot — the fold tail
        // beside `finalize_usage` (unused_finalization.rs) and the
        // `member_defined` funnel (row_resolution.rs) — sits in a sibling
        // lane's file this round. The pass is self-contained (it reads
        // only post-pass-2 resolution products); when a natural slot
        // frees, this call moves there verbatim.
        self.report_member_use_warnings();
        // HELD-WIRING SEAM (#3744): the fabricated extension-member
        // `type_argument_not_matching_bounds` prune runs here because every
        // natural slot — the pass-1 emit seam (`check_call_type_arg_bounds`,
        // invocation.rs) and the bounds table (`extension_member_bounds`,
        // invocation.rs) — sits in a sibling lane's file this round. The
        // pass is self-contained (it re-decides emitted rows against the
        // post-pass-2 receiver re-derivation); when a natural slot frees,
        // this call moves there verbatim.
        self.prune_fabricated_bounds_rows();
        if self.var_containers.is_empty() {
            return;
        }
        let strings = self.strings;
        // The declared-variance table: this file's containers by name. A
        // head outside it reads all-covariant — every generic of the
        // default SDK carries legacy-covariant parameters
        // (element.dart:11560-11562).
        let mut table: FxHashMap<StringId, Vec<Variance>> = self
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
        // The typedef half of the table: upstream computes each generic
        // alias's parameter variances from its aliased body
        // (variance_builder.dart:173-181, `_computeDeclared` — the element's
        // stored variance is the BODY-computed value, not a keyword), so a
        // `typedef F<X> = void Function(X)` head contributes `X`
        // contravariantly wherever `F<X>` appears. Without the seed the
        // head reads all-covariant and every `A<F<X>>` superinterface
        // suppresses. The alias's own parameters carry no `in`/`out`/
        // `inout` keyword — the computed value IS the declared face.
        //
        // Chained aliases (`typedef B<X> = A<X>`) resolve inside the same
        // fixed-point loop the official worklist runs (`_computeVariances`:
        // a typedef whose body references an uncomputed alias waits for
        // its pass). `variance_in_tree` reads an unseeded alias head as
        // all-covariant — the wrong direction for a pending chain — so the
        // loop re-seeds until a pass inserts nothing: an unresolvable
        // cycle or a non-alias head's stall keeps the under-report face
        // rather than inventing a variance.
        if let Some(elements) = self.index.get(self.file) {
            // One conversion per alias; the tree is stable across passes.
            let aliases: Vec<(StringId, VarTree, Box<[StringId]>)> = elements
                .decls
                .iter()
                .filter(|(_, decl)| decl.kind == MicroKind::TypeAlias)
                .filter(|(_, decl)| !decl.type_param_names.is_empty())
                .filter_map(|(name, decl)| {
                    self.ty_to_tree(decl.var_ty)
                        .map(|tree| (*name, tree, decl.type_param_names.clone()))
                })
                .collect();
            let mut pending: Vec<&(StringId, VarTree, Box<[StringId]>)> = aliases.iter().collect();
            loop {
                let mut stalled = Vec::with_capacity(pending.len());
                let mut progressed = false;
                for entry in pending {
                    let (name, tree, params) = entry;
                    let variances: Vec<Variance> = params
                        .iter()
                        .map(|param| variance_in_tree(tree, *param, &table, VARIANCE_WALK_DEPTH))
                        .collect();
                    // A still-unseeded alias inside the body leaves the
                    // row all-Unrelated exactly when the param never
                    // occurs elsewhere — the same row a ready alias with
                    // a phantom param produces, so the seed cannot wait
                    // on a signal the walk itself does not carry. The
                    // insert-then-repass loop resolves it: the pending
                    // head's row lands on the NEXT pass, after the
                    // referent seeds. Detect a pending referent by
                    // walking the body's heads — only a same-file alias
                    // name not yet in `table` stalls.
                    let mut waits = false;
                    let mut frontier = vec![tree];
                    while let Some(node) = frontier.pop() {
                        match &node.kind {
                            VarTreeKind::Head { name, args } => {
                                if !table.contains_key(name)
                                    && aliases.iter().any(|(alias, _, _)| alias == name)
                                {
                                    waits = true;
                                    break;
                                }
                                frontier.extend(args.iter());
                            }
                            VarTreeKind::Fn {
                                ret,
                                params,
                                bounds,
                            } => {
                                frontier.extend(ret.iter().map(|ret| ret.as_ref()));
                                frontier.extend(params.iter().chain(bounds.iter()));
                            }
                            VarTreeKind::Record { fields } => {
                                frontier.extend(fields.iter());
                            }
                            VarTreeKind::Unit => {}
                        }
                    }
                    if waits {
                        stalled.push(entry);
                        continue;
                    }
                    table.insert(*name, variances);
                    progressed = true;
                }
                if stalled.is_empty() || !progressed {
                    break;
                }
                pending = stalled;
            }
        }
        // The member name anchors (the model carries no spans; the
        // declaration sites do) — keyed on the site index so the member
        // face's per-node spans join by site.
        let member_ranges: FxHashMap<(StringId, StringId), usize> = self
            .decl_sites
            .iter()
            .enumerate()
            .filter_map(|(index, site)| match site.kind {
                DeclKind::Member { container, .. } => Some((container, site.name, index)),
                _ => None,
            })
            .map(|(container, name, index)| ((container, name), index))
            .collect();
        // Owned clones: the face rows outlive the `&mut self` emissions
        // below — one small struct per member method.
        let member_faces: FxHashMap<usize, MemberFaceSpans> = self
            .member_face_spans
            .iter()
            .map(|face| (face.site, face.clone()))
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
                let Some(&site) = member_ranges.get(&(container.name, *member)) else {
                    continue;
                };
                let anchor = self.decl_sites[site].range;
                // The `_checkForWrongVariancePosition` node anchors the
                // member face captured during the parse — each arm's own
                // node (the return TypeAnnotation, each FormalParameter,
                // each `X extends B` TypeParameter) or the member name
                // when the face's slot is absent.
                let face = member_faces.get(&site);
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
                            // The induced setter's parameter is covariant
                            // exactly when the field wrote `covariant`
                            // (element_builder.dart:504
                            // `isExplicitlyCovariant` — the mark makes the
                            // INDUCED setter parameter covariant, the
                            // upstream `node.covariantKeyword == null`
                            // clause at error_verifier.dart:7583-7588): a
                            // `covariant` field fires only the getter-side
                            // check.
                            if !entry.is_final && !entry.covariant {
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
                            // The return-type arm anchors at the return
                            // TypeAnnotation node (`T` in `T method1()`),
                            // not the member name.
                            let return_anchor =
                                face.and_then(|face| face.return_span).unwrap_or(anchor);
                            self.check_variance_position(
                                variance,
                                declared_variance,
                                keyword,
                                parameter,
                                return_anchor,
                            );
                            // The own type-parameter bound arm
                            // (error_verifier.dart:7943-7956): the bound
                            // reads in the `inout` position —
                            // `Variance.invariant.combine` — anchored at
                            // the `X extends B` TypeParameter node; an
                            // absent bound (`Ty::Dynamic`) is upstream's
                            // `bound == null → continue`.
                            for (index, bound) in entry.type_param_bounds.iter().enumerate() {
                                if matches!(*bound, Ty::Dynamic) {
                                    continue;
                                }
                                let variance = Variance::Invariant
                                    .combine(self.variance_in_ty(*bound, parameter, &table));
                                let bound_anchor = face
                                    .and_then(|face| face.bounds.get(index))
                                    .copied()
                                    .unwrap_or(anchor);
                                self.check_variance_position(
                                    variance,
                                    declared_variance,
                                    keyword,
                                    parameter,
                                    bound_anchor,
                                );
                            }
                            if let Some(formals) = &entry.formals {
                                for (index, formal) in
                                    formals.positional.iter().chain(&formals.named).enumerate()
                                {
                                    // A `covariant` formal skips the
                                    // contravariant-position check
                                    // (error_verifier.dart:7633-7635
                                    // `parameter.isCovariant` — the
                                    // formal's declared variance IS the
                                    // contravariant face).
                                    if formal.covariant {
                                        continue;
                                    }
                                    let variance = Variance::Contravariant
                                        .combine(self.variance_in_ty(formal.ty, parameter, &table));
                                    // Each formal's contravariant row
                                    // anchors at its own FormalParameter
                                    // node (`Contra<T> x`,
                                    // `required Contra<T> x`).
                                    let formal_anchor = face
                                        .and_then(|face| face.formals.get(index))
                                        .copied()
                                        .unwrap_or(anchor);
                                    self.check_variance_position(
                                        variance,
                                        declared_variance,
                                        keyword,
                                        parameter,
                                        formal_anchor,
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
    /// converted shape (named, function, and record faces convert; the
    /// remaining leaves — `dynamic`, `void`, `Unknown`, `Null`, `Never` —
    /// carry no occurrence and read `unrelated`, the under-report
    /// direction).
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

    /// The `Ty` → `VarTree` conversion the member/typedef faces walk: the
    /// interned shapes resolve through the pool (`Named` arguments,
    /// `Function` return + formals + own-parameter bounds, `Record`
    /// fields); the primitive leaves (`Unknown`, `Null`, `Dynamic`,
    /// `Never`, `Void`) have no `VarTreeKind` and answer `None`, matching
    /// the written-type capture's `void`/recovery `ret: None` face. A
    /// `None` leaf inside a structure answers `None` for the whole tree —
    /// the under-report direction, never an invented variance.
    pub(crate) fn ty_to_tree(&self, ty: Ty) -> Option<VarTree> {
        let pool = self.boundary.pool();
        match ty {
            Ty::Named {
                name,
                nullable,
                args,
            } => {
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
            Ty::Function { shape, nullable } => {
                let shape = pool.function_shape(shape);
                let ret = match shape.return_ty {
                    // `void` has no `VarTreeKind`: the written-type capture
                    // leaves `ret: None` for a `void` return (the unrelated
                    // contribution), so the model face mirrors it.
                    Ty::Void => None,
                    return_ty => Some(Box::new(self.ty_to_tree(return_ty)?)),
                };
                let params = shape
                    .positional
                    .iter()
                    .chain(shape.named.iter().map(|(_, ty)| ty))
                    .map(|&ty| self.ty_to_tree(ty))
                    .collect::<Option<Vec<_>>>()?;
                let bounds = shape
                    .type_params_named
                    .iter()
                    .filter_map(|(_, bound)| *bound)
                    .map(|bound| self.ty_to_tree(bound))
                    .collect::<Option<Vec<_>>>()?;
                Some(VarTree {
                    nullable,
                    poisoned: false,
                    kind: VarTreeKind::Fn {
                        ret,
                        params,
                        bounds,
                    },
                })
            }
            Ty::Record { shape, nullable } => {
                let shape = pool.record_shape(shape);
                let fields = shape
                    .positional
                    .iter()
                    .chain(shape.named.iter().map(|(_, ty)| ty))
                    .map(|&ty| self.ty_to_tree(ty))
                    .collect::<Option<Vec<_>>>()?;
                Some(VarTree {
                    nullable,
                    poisoned: false,
                    kind: VarTreeKind::Record { fields },
                })
            }
            Ty::Unknown | Ty::Null | Ty::Dynamic | Ty::Never | Ty::Void => None,
        }
    }

    /// #3744 — the fabricated `type_argument_not_matching_bounds` prune:
    /// every emitted row anchored at a member send's written type argument
    /// re-decides against the receiver's RE-DERIVED static face
    /// ([`Self::promoted_receiver`]), the same question pass 1 answered on
    /// the frozen push-time face. The oracle's extension-member bounds
    /// face reads the member's instantiated type over the resolved
    /// receiver (`type_arguments_verifier.dart`: the `node.function
    /// .staticType` the resolver stamped — promoted where a promotion
    /// stands), so a `n is int` local read's `expectStaticType` send
    /// checks `Exactly<int>` where the frozen `num` face checked
    /// `Exactly<num>` — a fabricated extra per emit.
    ///
    /// The re-verdict only DROPS: a row whose seam cannot be identified
    /// (no member-read record, no written type-argument group, a
    /// container-declared member, an ambiguous extension pool), whose
    /// receiver re-decides to the same face, or whose substituted
    /// verdict still fails keeps byte for byte — the under-report
    /// direction, never a new fabrication.
    fn prune_fabricated_bounds_rows(&mut self) {
        use dartq_scanner::TokenKind as K;
        if !self
            .report
            .diagnostics
            .iter()
            .any(|row| row.code.name == "type_argument_not_matching_bounds")
        {
            return;
        }
        self.accessible_extension_faces();
        // Each member read whose token is followed by a `<...>` type
        // argument group: `(group extent, member offset, member spelling,
        // recv, recv_span)`. Rows anchored inside the extent are the
        // candidates; `recv_span` is the receiver EXPRESSION's extent the
        // context-shaped arms (`?:`/`??`/`??=`/`await`) re-decide against.
        let mut sends: Vec<(u32, u32, u32, StringId, Receiver, SourceRange)> = Vec::new();
        for read in &self.member_reads {
            let at = self
                .tokens
                .as_slice()
                .partition_point(|t| t.utf16_offset <= read.offset);
            let Some(member_token) = at.checked_sub(1) else {
                continue;
            };
            let tokens = self.tokens.as_slice();
            if tokens[member_token].utf16_offset != read.offset {
                continue;
            }
            let Some(lt) = tokens
                .get(member_token + 1)
                .filter(|t| t.kind == K::Lt)
                .map(|_| member_token + 1)
            else {
                continue;
            };
            let Some(gt) = self.tokens.matching_end(lt) else {
                continue;
            };
            // The group's own `>` is the matched token's LAST angle: the
            // scanner folds adjacent closers into one `GtGt`/`GtGtGt`
            // (the parser's closer-split may or may not have separated
            // them), so the inner `>` closes a nested group and the last
            // byte closes this one. `byte_len` equals the UTF-16 length
            // for an all-ASCII closer run.
            sends.push((
                tokens[lt].utf16_offset,
                tokens[gt].utf16_offset + tokens[gt].byte_len - 1,
                read.offset,
                read.name,
                read.recv,
                read.recv_span,
            ));
        }
        if sends.is_empty() {
            return;
        }
        // Per send, the group's argument spellings parsed once — the
        // `(arg utf16 start, Ty)` column the row's anchor keys on.
        let mut parsed: Vec<Option<Vec<(u32, Ty)>>> = Vec::with_capacity(sends.len());
        for &(lt, gt, _, _, _, _) in &sends {
            parsed.push(self.written_group_args(lt, gt));
        }
        // The candidates: `(row offset, send index)` pairs whose anchor
        // sits inside a member send's written group. Collected before the
        // re-verdict pass — the verdict borrows `self` mutably (the
        // extension-face lazy fill, the application latch).
        let mut candidates: Vec<(u32, usize)> = Vec::new();
        for row in &self.report.diagnostics {
            if row.code.name != "type_argument_not_matching_bounds" {
                continue;
            }
            for (index, &(lt, gt, _, _, _, _)) in sends.iter().enumerate() {
                if lt < row.range.offset && row.range.offset < gt && parsed[index].is_some() {
                    candidates.push((row.range.offset, index));
                }
            }
        }
        let mut drop_offsets: FxHashSet<u32> = FxHashSet::default();
        for (offset, index) in candidates {
            let (_, _, member_at, member, recv, recv_span) = sends[index];
            let args = parsed[index].as_deref().expect("gated above");
            let Some(arg_at) = args
                .iter()
                .position(|&(start, _)| start == offset)
                .or_else(|| {
                    // `written_function_literal_span` grows a bare
                    // `Function` head through its formals group — match
                    // by containment inside the segment instead.
                    args.iter().position(|&(start, _)| {
                        start <= offset && args.iter().all(|&(s, _)| !(start < s && s <= offset))
                    })
                })
            else {
                continue;
            };
            let arg = args[arg_at].1;
            if self.bounds_row_conforms(member_at, member, recv, recv_span, args, arg, arg_at) {
                drop_offsets.insert(offset);
            }
        }
        if drop_offsets.is_empty() {
            return;
        }
        self.report.diagnostics.retain(|row| {
            row.code.name != "type_argument_not_matching_bounds"
                || !drop_offsets.contains(&row.range.offset)
        });
    }

    /// The re-decided verdict of one emitted bounds row (#3744): `true`
    /// when the member send re-resolves to a single extension member
    /// whose bound — instantiated over the receiver's RE-DERIVED face and
    /// the written group's arguments — the offending argument satisfies.
    /// `false` keeps the row (the under-report direction).
    /// #3797 widens the receiver face the verdict re-decides against: the
    /// promoted-read re-derivation first (the #3744 question), then the
    /// context-shaped faces the frozen push-time face could not spell —
    /// the `await` operand's flattened face and the `?:`/`??`/`??=`
    /// composite's context-pushed result (the `inference_update_3`
    /// family's `S = greatestClosure(K)` arm). Every retry runs the same
    /// instantiated-bound question the original verdict asks — a
    /// candidate that cannot re-decide to a conforming bound keeps the
    /// row.
    fn bounds_row_conforms(
        &mut self,
        member_at: u32,
        member: StringId,
        recv: Receiver,
        recv_span: SourceRange,
        args: &[(u32, Ty)],
        arg: Ty,
        arg_at: usize,
    ) -> bool {
        // The receiver face pass 2 re-derives: a promotion, a chained
        // link's settled result, or a narrowing reads here; the frozen
        // face unchanged answers the same verdict pass 1 gave.
        let recv = self.promoted_receiver(member_at, recv);
        let Some((head, receiver_ty, receiver_nullable)) = self.bounds_recv_face(recv) else {
            return false;
        };
        if self.bounds_verdict_at(
            head,
            receiver_ty,
            receiver_nullable,
            member,
            args,
            arg,
            arg_at,
        ) {
            return true;
        }
        // The context-shaped alternates (#3797): each receiver face the
        // oracle's resolver could have stamped over the frozen one. Every
        // candidate re-decides the WHOLE verdict — the walk gate, the
        // extension application, the bound substitution — since an
        // `await`-flattened or context-pushed face can name a different
        // head entirely (`SubFuture` → `int`).
        for ty in self.context_shaped_recv_tys(recv_span, receiver_ty) {
            let Some((alt_head, alt_ty, alt_nullable)) = self.bounds_alt_face(ty) else {
                continue;
            };
            if self.bounds_verdict_at(alt_head, alt_ty, alt_nullable, member, args, arg, arg_at) {
                return true;
            }
        }
        false
    }

    /// A candidate receiver face's `(head, ty, nullable)` triple, or
    /// `None` when the face is not a resolvable named head — the same
    /// deconstruction the promoted-receiver arm runs.
    fn bounds_alt_face(&mut self, ty: Ty) -> Option<(StringId, Ty, bool)> {
        let Ty::Named { name, .. } = ty else {
            return None;
        };
        Some((name, ty, self.potentially_nullable(ty)))
    }

    /// The `(head, ty, nullable)` of a promoted receiver record — the
    /// `Receiver::Typed`/`Receiver::This` deconstruction
    /// [`Self::bounds_row_conforms`]'s primary arm performs; any other
    /// receiver shape answers `None` (the keep direction).
    fn bounds_recv_face(&mut self, recv: Receiver) -> Option<(StringId, Ty, bool)> {
        match recv {
            Receiver::Typed { type_name, ty } => {
                Some((type_name, ty, self.potentially_nullable(ty)))
            }
            Receiver::This { container, kind } => {
                let ty = self.this_static_ty(container, kind);
                self.bounds_alt_face(ty)
            }
            _ => None,
        }
    }

    /// The instantiated-bound verdict over one concrete receiver face
    /// (`head`/`receiver_ty`/`receiver_nullable`): the extension
    /// resolution, the `on`-parameter binding, the bound projection, and
    /// the written-argument conformance check. `true` drops the row;
    /// `false` keeps it.
    fn bounds_verdict_at(
        &mut self,
        head: StringId,
        receiver_ty: Ty,
        receiver_nullable: bool,
        member: StringId,
        args: &[(u32, Ty)],
        arg: Ty,
        arg_at: usize,
    ) -> bool {
        // A member the receiver's own container declares never rode the
        // extension seam — its param table is the container's, and its
        // row keeps the frozen verdict.
        if let Walk::Found(entry) = self.walk_members(head, member)
            && entry.kind == MemberKind::Method
        {
            return false;
        }
        let faces = self.accessible_extensions.as_deref().unwrap_or(&[]);
        let declarers: Vec<(&crate::model::ExtensionFace, FileId)> = faces
            .iter()
            .filter(|(face, _)| {
                face.members
                    .get(&member)
                    .is_some_and(|entry| entry.kind == MemberKind::Method)
            })
            .copied()
            .collect();
        if declarers.len() != 1 {
            return false;
        }
        let (face, face_file) = declarers[0];
        match self.extension_application(head, receiver_ty, member, false, receiver_nullable) {
            ExtensionApp::Single | ExtensionApp::UnsafeSingle => {}
            ExtensionApp::None | ExtensionApp::Ambiguous(_) => return false,
        }
        let Some(entry) = face.members.get(&member) else {
            return false;
        };
        if entry.type_param_names.is_empty() || arg_at >= entry.type_param_names.len() {
            return false;
        }
        // The extension's own parameters instantiate from the re-decided
        // receiver — the same binding `extension_member_bounds` builds:
        // a bare own-parameter `on` head binds `T := receiver` wholesale,
        // a generic `on` head binds its argument positions through the
        // receiver's written arguments.
        let mut bindings: Vec<(StringId, Ty)> = Vec::new();
        if let Some(on_head) = face.on_type
            && face.type_param_names.contains(&on_head)
        {
            bindings.push((on_head, receiver_ty));
        } else if let (
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
        ) = (self.boundary.ty(face_file, face.on_ty), receiver_ty)
            && on_head == recv_head
        {
            let on_args = self.boundary.pool().get(on_args).to_vec();
            let recv_args = self.boundary.pool().get(recv_args).to_vec();
            if on_args.len() == recv_args.len() {
                for (param, arg) in on_args.iter().zip(recv_args) {
                    if let Ty::Named {
                        name,
                        args: ArgListId::EMPTY,
                        ..
                    } = param
                        && face.type_param_names.contains(name)
                    {
                        bindings.push((*name, arg));
                    }
                }
            }
        }
        let Some(bound) = entry.type_param_bounds.get(arg_at).copied() else {
            return false;
        };
        if matches!(bound, Ty::Dynamic | Ty::Unknown) {
            return false;
        }
        let bound = self.boundary.ty(face_file, bound);
        let Some(bound) = (if bindings.is_empty() {
            Some(bound)
        } else {
            self.subst_params(bound, &bindings)
        }) else {
            return false;
        };
        // The member's own parameters substitute the written group's
        // arguments (the `<TFrom, TTo extends Iterable<TFrom>>` shapes),
        // truncated to the shared prefix exactly like the emit's map.
        let len = entry.type_param_names.len().min(args.len());
        let map: Vec<(StringId, Ty)> = entry.type_param_names[..len]
            .iter()
            .zip(args[..len].iter())
            .map(|(name, (_, ty))| (*name, *ty))
            .collect();
        let Some(bound) = self.subst_params(bound, &map) else {
            return false;
        };
        if !self.call_type_arg_decidable_at(arg) || !self.call_type_arg_decidable_at(bound) {
            return false;
        }
        self.assignable(arg, bound, false) == Some(true)
    }

    /// The context-shaped receiver faces the frozen push-time face could
    /// not spell (#3797), as [`Ty`] candidates for
    /// [`Self::bounds_row_conforms`]'s retry loop. Every arm is
    /// conservative: each candidate must still re-decide the whole
    /// extension-member bounds verdict, so a wrong guess keeps the row
    /// exactly as the frozen face would.
    ///
    /// * **await** — `(await x).m<T>()`: the frozen receiver face is the
    ///   operand's own type where the oracle stamps
    ///   `flatten(operand)` — a user Future-like (`SubFuture`, an enum
    ///   `implements Future<E>`, a bound-chained type parameter)
    ///   flattens to the derived `E` upstream, while the push-time face
    ///   stayed the operand. `x.expectStaticType<Exactly<int>>()` over an
    ///   `await x` whose operand is `SubFuture` fires a fabricated row
    ///   unless the verdict retries under `flatten(SubFuture) = int`.
    /// * **context-pushed composite** — `f((c ? a : b)..m<T>())`,
    ///   `f((x ??= y)..m<T>())`: upstream's `?:`/`??`/`??=` static type
    ///   is `T` when `UP <: S` else `S = greatestClosure(K)` where `K` is
    ///   the composite's downward context (inference_update_3's
    ///   documented rule). The frozen face is the context-free UP, so a
    ///   send inside `contextB1(...)`/`contextIterable(...)`/`context<..>`
    ///   checks `Exactly<A>` where the oracle checks `Exactly<B1<Object?>>`.
    ///
    /// `frozen` is the re-derived (post-promotion) frozen face — the
    /// composite arm's `T` for the `T <: S` branch decision.
    fn context_shaped_recv_tys(&mut self, recv_span: SourceRange, frozen: Ty) -> Vec<Ty> {
        let mut out = Vec::new();
        // The pass-2 read face (#3797): `node.function.staticType`'s direct
        // analogue — the receiver expression re-typed after the read-type
        // join. This carries the promotions the frozen push-time face
        // lacks (a `this`-promoted property read under `if (this is B)`
        // reads the override's `int` where pass 1 froze `num`); the
        // promotion latch excludes invoked callees, so a member CALL
        // receiver keeps its declared face here and only a non-invoked
        // read (`x.expectStaticType<Exactly<int>>` tear-off) moves.
        if let Some(ty) = self.recv_expression_ty(recv_span) {
            if ty != Ty::UNKNOWN && ty != frozen {
                out.push(ty);
            }
        }
        if let Some(ty) = self.await_flattened_recv_ty(recv_span) {
            out.push(ty);
        }
        if let Some(ty) = self.context_pushed_composite_ty(recv_span, frozen) {
            out.push(ty);
        }
        // The composite's OWN static type (#3820): the oracle stamps a
        // `??`/`??=`/`?:` composite with `UP(NonNull(T1), T2')` / `LUB`
        // and an `op=`/`++`/`--` composite with the operand's read face —
        // the context-free half of the typing rules the context-pushed
        // arm above only covers on its `S` branch. Every face the
        // promotion latch or the join pass never staged (`(a.x++)` over a
        // promoted override, the `(local ??= value)` join over a promoted
        // left) reaches the verdict here; an unjoinable pair answers no
        // candidate (the keep direction).
        if let Some(ty) = self.composite_self_ty(recv_span) {
            if ty != Ty::UNKNOWN && ty != frozen {
                out.push(ty);
            }
        }
        out
    }

    /// The `await`-flattened receiver face when the receiver expression
    /// is literally `(await <operand>)`/`await <operand>` — the token
    /// stream's first `await` inside the span's outer parens, the
    /// operand's re-derived face flattened through
    /// [`Self::prune_await_flatten`]. `None` on every other shape (the
    /// keep direction).
    fn await_flattened_recv_ty(&mut self, recv_span: SourceRange) -> Option<Ty> {
        use dartq_scanner::TokenKind as K;
        let tokens = self.tokens.as_slice();
        let first = tokens.partition_point(|t| t.utf16_offset < recv_span.offset);
        let last = tokens.partition_point(|t| t.utf16_offset < recv_span.end());
        if last <= first {
            return None;
        }
        // Strip the span's own outer parens, then require the `await`
        // keyword as the first token — a `!await`-shaped receiver (the
        // keyword nested deeper, e.g. `f(await x)`) never qualifies.
        let mut lo = first;
        let mut hi = last;
        while hi > lo
            && tokens[lo].kind == K::OpenParen
            && self.tokens.matching_end(lo) == Some(hi - 1)
        {
            lo += 1;
            hi -= 1;
        }
        if lo >= hi || self.tokens.lexeme(self.source, lo) != "await" {
            return None;
        }
        // The operand is everything after `await`: `lo + 1 .. hi` in
        // token indices — an empty operand keeps the row.
        if lo + 1 >= hi {
            return None;
        }
        let operand_start = tokens[lo + 1].utf16_offset;
        let operand_end = {
            let last = &tokens[hi - 1];
            self.utf16_of((last.byte_offset + last.byte_len) as usize)
        };
        if operand_end <= operand_start {
            return None;
        }
        let operand = SourceRange {
            offset: operand_start,
            length: operand_end - operand_start,
        };
        let ty = self.recv_expression_ty(operand).unwrap_or(Ty::UNKNOWN);
        if ty == Ty::UNKNOWN {
            return None;
        }
        let flat = self.prune_await_flatten(ty);
        (flat != ty).then_some(flat)
    }

    /// `await`'s flatten over the receiver-expression face: upstream's
    /// `flatten` — the written `Future`/`FutureOr` head peels one level,
    /// a USER Future-like (`implements Future<E>` — `SubFuture`,
    /// `MixFuture`, `ExtensionSubFuture` in the corpus) answers the
    /// strict snapshot's instantiated `Future<E>` view, and a type
    /// parameter derives its written bound's future (`X extends
    /// SubFuture` → `int`). Mirrors `await_flatten`/`derived_future_walk`
    /// (assignability.rs) — kept private here so the prune stays
    /// self-contained in this file's lane.
    fn prune_await_flatten(&self, ty: Ty) -> Ty {
        self.prune_derived_future(self.flatten(ty), 0)
    }

    /// [`Self::prune_await_flatten`]'s depth-capped walk.
    fn prune_derived_future(&self, ty: Ty, depth: u8) -> Ty {
        if depth > 8 {
            return ty;
        }
        let Ty::Named {
            name,
            nullable,
            args,
        } = ty
        else {
            return ty;
        };
        // A written `Future`/`FutureOr` head already peeled through
        // `flatten` — re-asking the instantiated view would double-
        // flatten. User Future-like heads answer the `Future<E>` env's
        // argument verbatim (the snapshot already walked the chain).
        if name != self.intern("Future")
            && name != self.intern("FutureOr")
            && let Some(arg) = self.prune_future_type_arg(name, args)
        {
            return if nullable {
                Self::nullable_form(arg)
            } else {
                arg
            };
        }
        // The `X & B` walk: the parameter's written bound derives the
        // future. A bound that derives nothing keeps the operand's own
        // face.
        if let Some(bound) = self.param_bound_of(name) {
            let walked = self.prune_derived_future(self.flatten(bound), depth + 1);
            if walked != bound {
                return if nullable {
                    Self::nullable_form(walked)
                } else {
                    walked
                };
            }
        }
        ty
    }

    /// The `Future<E>` argument a user Future-like type carries —
    /// `ops.instance_env(ty, Future)`'s first binding, `None` when the
    /// snapshot cannot answer (the keep direction).
    fn prune_future_type_arg(&self, name: StringId, args: ArgListId) -> Option<Ty> {
        let ops = self.ops?;
        let env = ops.instance_env(
            Ty::Named {
                name,
                nullable: false,
                args,
            },
            self.intern("Future"),
            &self.boundary.args,
        );
        env.and_then(|env| env.first().map(|(_, ty)| *ty))
    }

    /// The context-pushed receiver face of a `?:`/`??`/`??=` composite
    /// receiver (#3797): upstream's inference-update-3 typing gives the
    /// composite `T` when `UP <: S`, else `S = greatestClosure(K)` where
    /// `K` is the composite's downward context (upstream
    /// `conditional_expression`/`if_null` resolution, mirrored by the
    /// corpus's own rule comments). The frozen push-time face is the
    /// context-free UP — a `contextB1((b ? c1Int : c2Double)..m<T>())`
    /// send checks `Exactly<A>` there where the oracle checks
    /// `Exactly<B1<Object?>>` and stays silent.
    ///
    /// Decision, all under-report: `Some(S)` only when `S` resolves AND
    /// `T <: S` is PROVEN false (`assignable == Some(false)`) — the
    /// oracle's stamp is unconditional once `UP` cannot land inside the
    /// context; any other verdict keeps the `T`-branch face the emit
    /// already judged. `frozen` is the re-derived push-time face
    /// standing in for the oracle's `T` — the same composite join
    /// pass 1 computed.
    fn context_pushed_composite_ty(&mut self, recv_span: SourceRange, frozen: Ty) -> Option<Ty> {
        use dartq_scanner::TokenKind as K;
        let tokens = self.tokens.as_slice();
        let mut first = tokens.partition_point(|t| t.utf16_offset < recv_span.offset);
        let mut last = tokens.partition_point(|t| t.utf16_offset < recv_span.end());
        // A tail-anchored receiver row (`(x ??= A())` — the if-null join
        // stages nothing, so the cascade target's read anchors at the
        // paren close) spans one closer token; expand it to the matched
        // group so the composite window is the whole parenthesized
        // expression rather than its final `)`.
        if last == first + 1 && tokens[first].kind == K::CloseParen {
            if let Some(opener) = (0..first).rev().find(|&i| {
                tokens[i].kind == K::OpenParen && self.tokens.matching_end(i) == Some(first)
            }) {
                first = opener;
            }
        }
        if last <= first {
            return None;
        }
        // The composite's own token window, outer parens stripped — the
        // cascade target `(b ? a : c)` spans its parens, the bare
        // `x ??= y` spans the operands.
        let mut lo = first;
        let mut hi = last;
        while hi > lo
            && tokens[lo].kind == K::OpenParen
            && self.tokens.matching_end(lo) == Some(hi - 1)
        {
            lo += 1;
            hi -= 1;
        }
        if hi - lo < 3 {
            return None;
        }
        // Depth-0 separator scan: a top-level `?`/`??`/`??=` marks the
        // receiver as a composite whose static type the context shapes.
        // The operator KIND matters — a conditional stamps `S` on
        // `T <! S` alone, while the if-null forms carry a third arm.
        let mut conditional_at = None;
        let mut if_null_at = None;
        let mut at = lo;
        while at < hi && (conditional_at.is_none() || if_null_at.is_none()) {
            match tokens[at].kind {
                K::OpenCurly
                | K::StringInterpolationExpression
                | K::OpenParen
                | K::OpenSquare
                | K::Lt => {
                    if let Some(close) = self.tokens.matching_end(at) {
                        if close < hi {
                            at = close + 1;
                            continue;
                        }
                    }
                    at += 1;
                }
                K::QuestionQuestion | K::QuestionQuestionEq => {
                    if_null_at.get_or_insert(at);
                    at += 1;
                }
                K::Question => {
                    conditional_at.get_or_insert(at);
                    at += 1;
                }
                _ => at += 1,
            }
        }
        if conditional_at.is_none() && if_null_at.is_none() {
            return None;
        }
        // `?:` is the lowest-precedence operator: a paired top-level `?`
        // wins over an earlier `??` (`a ?? b ? c : d` groups as
        // `(a ?? b) ? c : d`).
        let if_null_at = conditional_at.is_none().then_some(if_null_at).flatten();
        let context = self.prune_composite_context(recv_span, lo)?;
        // `T <: S` proven false gates BOTH kinds — any other verdict
        // (`Some(true)`, `None`) keeps the `T`-branch face the emit
        // already judged. A mis-resolved `S` still cannot drop the row by
        // itself: the candidate re-decides the whole bound verdict.
        if self.assignable(frozen, context, false) != Some(false) {
            return None;
        }
        let Some(op_at) = if_null_at else {
            // Conditional (`c ? a : b`): oracle 3.13.7 stamps `S` on
            // `T <! S` alone — verified silent for
            // `contextB1((b ? C1<int>() : C2<double>())..m<…>())`.
            return Some(context);
        };
        // If-null (`??`/`??=`) carries a THIRD arm
        // (inference-update-3): `T` when `T <: S`; else `S` when
        // `T2' <: S`; else `T`. Releasing `S` on `T <! S` alone drops
        // rows the oracle reports — verified: `contextB1((x ??= A())..m)`
        // reports `Exactly<A>` and `contextB1((x ?? B1<Object?>())..m)`
        // reports `Exactly<Object>` on 3.13.7. `T2'` is only spellable
        // when `T2 <: J'` (then `T2' = T2`); an opaque coercion keeps
        // the row.
        let lhs = self.prune_operand_ty(lo, op_at)?;
        let rhs = self.prune_operand_ty(op_at + 1, hi)?;
        let j_prime = if tokens[op_at].kind == K::QuestionQuestionEq {
            // A bare local write is exactly `IDENT ??=` — the local's
            // declared (unpromoted) face is `J'`. Any other lvalue's
            // setter write type is outside this face's reach.
            if op_at != lo + 1 || tokens[lo].kind != K::Identifier {
                return None;
            }
            let name = self.strings.intern(self.tokens.lexeme(self.source, lo));
            self.local_declared_ty(name)?
        } else {
            Self::non_null_form(lhs)
        };
        if self.assignable(rhs, j_prime, false) != Some(true) {
            return None;
        }
        (self.assignable(rhs, context, false) == Some(true)).then_some(context)
    }

    /// One composite operand's re-derived face: `recv_expression_ty`
    /// over the token slice `lo_tok..hi_tok`, trimming `?`/`:`/`??`
    /// separators at both ends and anchoring at the tail token the way
    /// the chain walk's keys do.
    fn prune_operand_ty(&mut self, lo_tok: usize, hi_tok: usize) -> Option<Ty> {
        use dartq_scanner::TokenKind as K;
        let tokens = self.tokens.as_slice();
        let (mut lo, mut hi) = (lo_tok, hi_tok);
        while hi > lo && matches!(tokens[lo].kind, K::Question | K::Colon) {
            lo += 1;
        }
        while hi > lo && matches!(tokens[hi - 1].kind, K::Question | K::Colon) {
            hi -= 1;
        }
        if hi <= lo {
            return None;
        }
        let first = &tokens[lo];
        let last = &tokens[hi - 1];
        let offset = if hi - lo > 1
            && first.kind == K::OpenParen
            && self.tokens.matching_end(lo) == Some(hi - 1)
        {
            first.utf16_offset
        } else if hi - lo > 1 {
            last.utf16_offset
        } else {
            first.utf16_offset
        };
        let end = self.utf16_of((last.byte_offset + last.byte_len) as usize);
        self.recv_expression_ty(SourceRange {
            offset,
            length: end.saturating_sub(offset),
        })
        .filter(|ty| *ty != Ty::UNKNOWN)
    }

    /// The composite receiver's own static type (#3820), as
    /// [`Ty`] for the re-decided verdict's candidate list. Shapes
    /// (depth-0 operators over the receiver's token window, outer parens
    /// stripped):
    ///
    /// * **if-null** (`??`/`??=`) — `UP(NonNull(T1), T2')` where `T1` is
    ///   the left operand's re-derived read face and `T2'` the right
    ///   operand's face, call-coerced against a function-shaped left
    ///   (`resolveIfNull`, upstream Dart SDK source,
    ///   `binary_expression_resolver.dart:214-216` — the context-free
    ///   `T` branch). A bare-local `??=` additionally requires the right
    ///   face provable inside the local's declared face (`J'`), the same
    ///   gate the context-pushed arm runs;
    /// * **conditional** (`?:`) — `LUB(then, else)`, the operands'
    ///   re-derived faces joined;
    /// * **inc-dec** (prefix/postfix `++`/`--`) and **compound
    ///   assignment** (`op=`) — the operand's re-derived read face (the
    ///   result type is the read type: upstream Dart SDK source,
    ///   `postfix_expression_resolver.dart:62` `node.readType`,
    ///   `assignment_expression_resolver.dart:84` the local-provider
    ///   read).
    ///
    /// Numeric binary operators stay out (the operator-result table is
    /// the operand-face layer's, not this prune's). `None` on every
    /// unjoinable or unrecognized shape — the keep direction.
    fn composite_self_ty(&mut self, recv_span: SourceRange) -> Option<Ty> {
        use dartq_scanner::TokenKind as K;
        let tokens = self.tokens.as_slice();
        let (lo, hi) = self.composite_window(recv_span)?;
        if hi - lo < 2 {
            return None;
        }
        // The depth-0 operator inventory: leftmost `?` (with its matching
        // depth-0 `:`), leftmost `??`/`??=`, leftmost inc-dec and
        // compound-assignment marks. Group tokens jump past their
        // matched closer.
        let mut conditional_at = None;
        let mut colon_at = None;
        let mut if_null_at = None;
        let mut inc_dec_at = None;
        let mut compound_at = None;
        let mut at = lo;
        while at < hi {
            match tokens[at].kind {
                K::OpenCurly
                | K::StringInterpolationExpression
                | K::OpenParen
                | K::OpenSquare
                | K::Lt => {
                    if let Some(close) = self.tokens.matching_end(at) {
                        if close < hi {
                            at = close + 1;
                            continue;
                        }
                    }
                    at += 1;
                }
                K::QuestionQuestion | K::QuestionQuestionEq => {
                    if_null_at.get_or_insert(at);
                    at += 1;
                }
                K::PlusPlus | K::MinusMinus => {
                    inc_dec_at.get_or_insert(at);
                    at += 1;
                }
                K::PlusEq
                | K::MinusEq
                | K::StarEq
                | K::SlashEq
                | K::PercentEq
                | K::TildeSlashEq
                | K::AmpersandEq
                | K::BarEq
                | K::CaretEq
                | K::LtLtEq
                | K::GtGtEq
                | K::GtGtGtEq => {
                    compound_at.get_or_insert(at);
                    at += 1;
                }
                K::Question => {
                    // A conditional's `?` needs its DEPTH-0 `:` to the
                    // right (a colon nested in a group is another
                    // expression's), and a nullable-type suffix (`is
                    // int?`) has none — the pair requirement keeps both
                    // out of the conditional face.
                    let mut probe = at + 1;
                    let mut colon = None;
                    while probe < hi {
                        match tokens[probe].kind {
                            K::Colon => {
                                colon = Some(probe);
                                break;
                            }
                            K::OpenCurly
                            | K::StringInterpolationExpression
                            | K::OpenParen
                            | K::OpenSquare
                            | K::Lt => {
                                if let Some(close) = self.tokens.matching_end(probe) {
                                    if close < hi {
                                        probe = close + 1;
                                        continue;
                                    }
                                }
                                probe += 1;
                            }
                            _ => probe += 1,
                        }
                    }
                    if let Some(colon) = colon {
                        conditional_at.get_or_insert(at);
                        colon_at.get_or_insert(colon);
                    }
                    at += 1;
                }
                _ => at += 1,
            }
        }
        // `?:` is the lowest-precedence operator: a paired top-level `?`
        // wins over an earlier `??` (`a ?? b ? c : d` groups as
        // `(a ?? b) ? c : d`), the same precedence the context-pushed
        // arm applies.
        if let (Some(q), Some(c)) = (conditional_at, colon_at) {
            let then_face = self.prune_operand_ty(q + 1, c)?;
            let else_face = self.prune_operand_ty(c + 1, hi)?;
            return self.prune_join(then_face, else_face);
        }
        if let Some(op_at) = if_null_at {
            let lhs = self.prune_operand_ty(lo, op_at)?;
            let rhs = self.prune_operand_ty(op_at + 1, hi)?;
            // A bare local write gates the right face on `J'` (the
            // local's declared face); any other lvalue's setter write
            // type is outside this face's reach, so the right face
            // stands as inferred.
            if tokens[op_at].kind == K::QuestionQuestionEq {
                if op_at == lo + 1 && tokens[lo].kind == K::Identifier {
                    let name = self.strings.intern(self.tokens.lexeme(self.source, lo));
                    let j_prime = self.local_declared_ty(name)?;
                    if self.assignable(rhs, j_prime, false) != Some(true) {
                        return None;
                    }
                }
            }
            let (left, right) = self.coerce_if_null_callable(Self::non_null_form(lhs), rhs);
            return self.prune_join(left, right);
        }
        if let Some(op_at) = inc_dec_at {
            // Prefix marks the operand to the right, postfix to the
            // left; the result type is the operand's read face either
            // way.
            let operand = if op_at == lo {
                self.prune_operand_ty(op_at + 1, hi)?
            } else {
                self.prune_operand_ty(lo, op_at)?
            };
            return Some(operand);
        }
        if let Some(op_at) = compound_at {
            if op_at > lo {
                return self.prune_operand_ty(lo, op_at);
            }
        }
        None
    }

    /// The receiver span's composite token window: the span's token
    /// range with a tail-anchored single closer expanded to its opener
    /// (a cascade target's `(x ??= A())` row anchors at the `)`) and the
    /// outermost paren pairs stripped. `(lo, hi)` in token indices.
    fn composite_window(&self, recv_span: SourceRange) -> Option<(usize, usize)> {
        use dartq_scanner::TokenKind as K;
        let tokens = self.tokens.as_slice();
        let mut first = tokens.partition_point(|t| t.utf16_offset < recv_span.offset);
        let mut last = tokens.partition_point(|t| t.utf16_offset < recv_span.end());
        if last == first + 1 && tokens[first].kind == K::CloseParen {
            if let Some(opener) = (0..first).rev().find(|&i| {
                tokens[i].kind == K::OpenParen && self.tokens.matching_end(i) == Some(first)
            }) {
                first = opener;
            }
        }
        if last <= first {
            return None;
        }
        while last > first
            && tokens[first].kind == K::OpenParen
            && self.tokens.matching_end(first) == Some(last - 1)
        {
            first += 1;
            last -= 1;
        }
        Some((first, last))
    }

    /// [`Self::composite_self_ty`]'s conservative `UP`: the exact rules
    /// only — identity, provable subsumption (the join answers the
    /// supertype), `Null` absorption, the nullability merge
    /// (`UP(T1?, T2) = UP(T1, T2)?`), `dynamic`/`void` top absorption,
    /// the `Never` bottom discharge, the numeric pair `int`/`double` →
    /// `num`, the bare `Function` pivot over function shapes, and the
    /// same-head covariant argument join (a parameter with declared
    /// non-covariant variance refuses the join). Every other pair
    /// answers `None` — the real `least_upper_bound.dart` walks shared
    /// supertypes this face does not model, and a missing candidate
    /// keeps the row.
    fn prune_join(&mut self, left: Ty, right: Ty) -> Option<Ty> {
        if left == right {
            return Some(left);
        }
        // `UP(Null, T)` is `T`/`T?` (`least_upper_bound.dart:487-524`).
        if let (Ty::Null, other) | (other, Ty::Null) = (&left, &right) {
            return Some(Self::nullable_form(*other));
        }
        // TOP and BOTTOM discharge before the subsumption asks
        // (`least_upper_bound.dart:405-427` TOP, `:429-451` BOTTOM).
        if matches!(left, Ty::Dynamic | Ty::Void) {
            return Some(left);
        }
        if matches!(right, Ty::Dynamic | Ty::Void) {
            return Some(right);
        }
        if matches!(left, Ty::Never) {
            return Some(right);
        }
        if matches!(right, Ty::Never) {
            return Some(left);
        }
        if self.assignable(left, right, false) == Some(true) {
            return Some(right);
        }
        if self.assignable(right, left, false) == Some(true) {
            return Some(left);
        }
        // Nullability merge (`:562-571`): join the stripped cores, then
        // re-suffix. The stripped cores also put the numeric pair on
        // its non-null spelling.
        if self.potentially_nullable(left) || self.potentially_nullable(right) {
            let inner = self.prune_join(Self::non_null_form(left), Self::non_null_form(right))?;
            return Some(Self::nullable_form(inner));
        }
        if self.is_named_head(left, "int") && self.is_named_head(right, "double")
            || self.is_named_head(left, "double") && self.is_named_head(right, "int")
        {
            return Some(Ty::Named {
                name: self.intern("num"),
                nullable: false,
                args: ArgListId::EMPTY,
            });
        }
        // The bare `Function` head is the supertype of every function
        // shape (`subtype.dart`'s function block); two function shapes
        // with no subsumption answer it only when one side IS the bare
        // head — distinct signatures join above this face's reach.
        if (self.prune_bare_function_head(left)
            && matches!(
                right,
                Ty::Function {
                    nullable: false,
                    ..
                }
            ))
            || (self.prune_bare_function_head(right)
                && matches!(
                    left,
                    Ty::Function {
                        nullable: false,
                        ..
                    }
                ))
        {
            return Some(if self.prune_bare_function_head(left) {
                left
            } else {
                right
            });
        }
        // Same-head generic join: covariant argument positions join
        // pairwise; a declared non-covariant position refuses (the join
        // of an invariant/contravariant argument is not the covariant
        // lift).
        if let (
            Ty::Named {
                name: lname,
                args: largs,
                nullable: lnull,
            },
            Ty::Named {
                name: rname,
                args: rargs,
                nullable: rnull,
            },
        ) = (&left, &right)
            && lname == rname
            && lnull == rnull
        {
            let lhs_args = self.boundary.pool().get(*largs).to_vec();
            let rhs_args = self.boundary.pool().get(*rargs).to_vec();
            if lhs_args.len() == rhs_args.len() && self.head_args_covariant(*lname, lhs_args.len())
            {
                let mut joined = Vec::with_capacity(lhs_args.len());
                for (a, b) in lhs_args.iter().zip(rhs_args.iter()) {
                    if a == b {
                        joined.push(*a);
                    } else {
                        joined.push(self.prune_join(*a, *b)?);
                    }
                }
                return Some(
                    self.boundary
                        .args
                        .borrow_mut()
                        .named(*lname, *lnull, &joined),
                );
            }
        }
        None
    }

    /// Whether `ty` is a non-nullable argument-less named head spelled
    /// `name` (`int`, `double`, bare `Function`).
    fn is_named_head(&self, ty: Ty, name: &str) -> bool {
        matches!(
            ty,
            Ty::Named {
                name: n,
                args: ArgListId::EMPTY,
                nullable: false,
            } if n == self.intern(name)
        )
    }

    /// Whether `ty` is the bare `Function` head (no arguments, not
    /// nullable) — the function family's shared supertype. Local spelling
    /// of the join pivot: the assignability module's same-named inherent
    /// method is module-private and cannot be reached from here.
    fn prune_bare_function_head(&self, ty: Ty) -> bool {
        self.is_named_head(ty, "Function")
    }

    /// Whether every parameter of `head` (as declared in this file's
    /// variance containers) is covariant or unmarked — the guard the
    /// same-head join runs. Heads outside the containers read
    /// all-covariant (the SDK's generics are legacy-covariant;
    /// `element.dart:11560-11562`), a head with fewer declared params
    /// than `arity` refuses.
    fn head_args_covariant(&self, head: StringId, arity: usize) -> bool {
        match self.var_containers.iter().find(|c| c.name == head) {
            None => true,
            Some(container) => {
                container.params.len() == arity
                    && container
                        .params
                        .iter()
                        .all(|(_, declared, _)| declared.is_none_or(|v| v == Variance::Covariant))
            }
        }
    }

    /// The composite's downward context `K` as `S = greatestClosure(K)`
    /// (#3797): the enclosing ARGUMENT's instantiated formal schema —
    /// `contextB1(<expr>)` reads `B1<T>`'s greatest closure
    /// `B1<Object?>`, `context<Iterable<num>>(<expr>)` reads the
    /// written-argument substitution `Iterable<num>`. Resolves a bare
    /// top-level function callee's declared formals and substitutes:
    /// the call's WRITTEN type arguments pair against the declaration's
    /// parameter order; an unwritten group greatest-closes every
    /// unsolved parameter occurrence (`Object?` covariant, `Never`
    /// contravariant — default generic covariance). `None` on every
    /// shape outside the argument-slot channel (an assignment target, a
    /// nested composite's own context, a member/prefixed callee) — the
    /// keep direction.
    fn prune_composite_context(
        &mut self,
        recv_span: SourceRange,
        composite_lo: usize,
    ) -> Option<Ty> {
        use dartq_scanner::TokenKind as K;
        let tokens = self.tokens.as_slice();
        // The argument-list `(` enclosing the composite: the nearest
        // `(` before `composite_lo` whose matched `)` extends past the
        // composite's end AND that sits inside its own argument span —
        // walking outward over nested groups, skipping the composite's
        // own parens (they start inside `composite_lo`'s window).
        let mut arglist = None;
        for i in (0..composite_lo).rev() {
            match tokens[i].kind {
                K::CloseParen | K::CloseSquare | K::CloseCurly => break,
                K::OpenParen => {
                    // Is this `(` an argument list (callee-shaped tail)
                    // rather than a grouping? The token before it must
                    // read `IDENT`, `>`, `)`, `]`, or a literal — a
                    // grouping's own opener follows an operator/`,`/`(`.
                    let Some(close) = self.tokens.matching_end(i) else {
                        continue;
                    };
                    if tokens[close].utf16_offset < recv_span.end() {
                        // The `(` closes before the composite ends — it
                        // cannot contain it.
                        continue;
                    }
                    arglist = Some(i);
                    break;
                }
                // A `,`/operator at depth 0 relative to our backward
                // walk means no enclosing `(` before the argument —
                // actually ANY non-opener token keeps walking (`,` of a
                // sibling arg would sit INSIDE the arglist's own `(`).
                _ => {}
            }
        }
        let arglist = arglist?;
        // The callee tail: `IDENT` or `IDENT<args>` directly before
        // the `(`. Member sends (`o.m(`), prefixed callees (`p.f(`), and
        // callable-expression tails (`f()()`— a `)`/`]` tail) stay out
        // — only the bare top-level function resolves cleanly here.
        let mut callee_at = arglist;
        while callee_at > 0 && matches!(tokens[callee_at - 1].kind, K::Identifier) == false {
            // Written type-argument group `f<W0..>(` — walk back over
            // the matched `<...>`: the closer before `(` is `>`; its
            // opener is a `<` at angle-depth 0.
            if matches!(tokens[callee_at - 1].kind, K::Gt | K::GtGt | K::GtGtGt) {
                let mut depth = 0usize;
                let mut j = callee_at - 1;
                let mut found = None;
                while j > 0 {
                    j -= 1;
                    match tokens[j].kind {
                        K::Gt | K::GtGt | K::GtGtGt => depth += 1,
                        K::Lt => {
                            if depth == 0 {
                                found = Some(j);
                                break;
                            }
                            depth -= 1;
                        }
                        _ => {}
                    }
                }
                let Some(lt) = found else {
                    return None;
                };
                if lt == 0 || tokens[lt - 1].kind != K::Identifier {
                    return None;
                }
                callee_at = lt;
                break;
            }
            return None;
        }
        let name_token = callee_at.checked_sub(1)?;
        if tokens[name_token].kind != K::Identifier {
            return None;
        }
        // A member/cascade tail (`o.m(`, `o..m(`, `o?.m(`) is not a
        // top-level callee — the identifier rides a receiver the context
        // channel does not carry here.
        if name_token > 0
            && matches!(
                tokens[name_token - 1].kind,
                K::Period
                    | K::QuestionPeriod
                    | K::PeriodPeriod
                    | K::QuestionPeriodPeriod
                    | K::PeriodPeriodPeriod
                    | K::PeriodPeriodPeriodQuestion
            )
        {
            return None;
        }
        let name = self
            .strings
            .intern(self.tokens.lexeme(self.source, name_token));
        let decl = self.top_level_decl(name)?;
        let formals = decl.formals.as_ref()?;
        // Which argument? Count depth-0 `,` inside the arglist before
        // the composite's argument start (the composite's outer paren
        // IS the argument's first token, or the token right after the
        // arglist `(`/`,`).
        let mut positional = 0usize;
        let mut named: Option<StringId> = None;
        {
            let mut depth = 0usize;
            let mut arg_start = arglist + 1;
            let mut i = arglist + 1;
            while i < composite_lo {
                match tokens[i].kind {
                    K::OpenParen | K::OpenSquare | K::OpenCurly | K::Lt => depth += 1,
                    K::CloseParen
                    | K::CloseSquare
                    | K::CloseCurly
                    | K::Gt
                    | K::GtGt
                    | K::GtGtGt => depth = depth.saturating_sub(1),
                    K::Comma if depth == 0 => {
                        positional += 1;
                        arg_start = i + 1;
                        named = None;
                    }
                    K::Identifier if depth == 0 && arg_start == i => {
                        // `name:` — a named-argument label: the next
                        // token is `:`.
                        if tokens.get(i + 1).is_some_and(|t| t.kind == K::Colon) {
                            named = Some(self.strings.intern(self.tokens.lexeme(self.source, i)));
                        }
                    }
                    _ => {}
                }
                i += 1;
            }
        }
        // The formal at the slot: `decl`'s Tys are projected through the
        // declaring file's pool already (`Projected` carries them), so
        // `formal.ty` is the boundary face.
        let formal_ty = match named {
            Some(label) => formals.named.iter().find(|f| f.name == label).map(|f| f.ty),
            None => formals.positional.get(positional).map(|f| f.ty),
        }?;
        // The instantiation: written type arguments pair positionally
        // against `type_param_names`; an unwritten group leaves every
        // parameter unsolved (greatestClosure's `_`).
        let params: &[StringId] = &decl.type_param_names;
        if params.is_empty() {
            return Some(formal_ty);
        }
        let mut subst: Vec<(StringId, Ty)> = Vec::new();
        if callee_at != arglist {
            // `f<W0, …>(`: the group's written args, `lt` is the `<`.
            let lt = callee_at;
            let gt = self.tokens.matching_end(lt)?;
            let written = self.written_group_args(
                tokens[lt].utf16_offset,
                tokens[gt].utf16_offset + tokens[gt].byte_len - 1,
            )?;
            for (param, (_, ty)) in params.iter().zip(written.iter()) {
                subst.push((*param, *ty));
            }
        }
        if subst.len() == params.len() {
            return self.subst_params(formal_ty, &subst);
        }
        // greatestClosure: each unsolved parameter binds `Object?` in
        // covariant position, `Never` contravariant — the corpus's
        // helpers (`contextB1`/`contextIterable`/`context`) are plain
        // legacy-covariant generics, so the polarity walk covers them
        // exactly. A partially solved map still greatest-closes the
        // REST (the map substitutes what's known).
        self.prune_greatest_closure(formal_ty, params, &subst, true)
    }

    /// `greatestClosure(K)` over the callee's unsolved inference
    /// parameters (#3797): every `param` leaf not already bound in
    /// `subst` binds `Object?` covariantly or `Never` contravariantly
    /// (upstream `computeGreatestClosure` — `_` in covariant position
    /// is `Object?`, in contravariant `Never`). `covariant` flips
    /// through function-type parameter positions and stays through
    /// named/record arguments and returns. A `Ty::Function` carrying
    /// its own type-parameter group bails (a generic context cannot
    /// spell here) — `None` keeps the row.
    fn prune_greatest_closure(
        &self,
        ty: Ty,
        params: &[StringId],
        subst: &[(StringId, Ty)],
        covariant: bool,
    ) -> Option<Ty> {
        match ty {
            Ty::Named {
                name,
                nullable,
                args,
            } => {
                if args == ArgListId::EMPTY {
                    if let Some(bound) = subst.iter().find(|(p, _)| *p == name) {
                        return Some(bound.1);
                    }
                    if params.contains(&name) {
                        return Some(if covariant {
                            self.object_question()
                        } else {
                            Ty::Never
                        });
                    }
                    return Some(ty);
                }
                let inner: Vec<Ty> = self.boundary.pool().get(args).to_vec();
                let mut out = Vec::with_capacity(inner.len());
                for arg in inner {
                    out.push(self.prune_greatest_closure(arg, params, subst, covariant)?);
                }
                Some(self.boundary.args.borrow_mut().named(name, nullable, &out))
            }
            Ty::Function { shape, nullable } => {
                let shape = self.boundary.pool().function_shape(shape).clone();
                if shape.type_params != 0 || !shape.type_params_named.is_empty() {
                    return None;
                }
                let ret = self.prune_greatest_closure(shape.return_ty, params, subst, covariant)?;
                let mut positional = Vec::with_capacity(shape.positional.len());
                for param in shape.positional.iter() {
                    positional
                        .push(self.prune_greatest_closure(*param, params, subst, !covariant)?);
                }
                let mut named = Vec::with_capacity(shape.named.len());
                for (name, formal) in shape.named.iter() {
                    named.push((
                        *name,
                        self.prune_greatest_closure(*formal, params, subst, !covariant)?,
                    ));
                }
                Some(self.boundary.args.borrow_mut().function_required(
                    0,
                    shape.required_positional,
                    ret,
                    &positional,
                    &named,
                    nullable,
                ))
            }
            Ty::Record { shape, nullable } => {
                let shape = self.boundary.pool().record_shape(shape).clone();
                let mut positional = Vec::with_capacity(shape.positional.len());
                for field in shape.positional.iter() {
                    positional.push(self.prune_greatest_closure(*field, params, subst, covariant)?);
                }
                let mut named = Vec::with_capacity(shape.named.len());
                for (name, field) in shape.named.iter() {
                    named.push((
                        *name,
                        self.prune_greatest_closure(*field, params, subst, covariant)?,
                    ));
                }
                Some(
                    self.boundary
                        .args
                        .borrow_mut()
                        .record(&positional, &named, nullable),
                )
            }
            other => Some(other),
        }
    }

    /// The decidability gate of the bounds emit (`call_type_arg_decidable`
    /// in invocation.rs): a bare named head must resolve as a type; every
    /// compound shape decides. Kept beside the prune so the port stays
    /// self-contained.
    fn call_type_arg_decidable_at(&self, ty: Ty) -> bool {
        match ty {
            Ty::Named {
                name,
                args: ArgListId::EMPTY,
                ..
            } => self.named_type_is_type(name),
            _ => true,
        }
    }

    /// The written type-argument group's members as `(utf16 start, Ty)`
    /// pairs, parsed from the source bytes between `lt` and `gt` (the
    /// angle offsets). The head of each segment resolves through the
    /// recorded `type_uses` (textual order); a segment that fails to
    /// parse — a function-type spelling, an unresolved head, trailing
    /// residue — drops the whole group to `None` (the keep direction).
    fn written_group_args(&self, lt: u32, gt: u32) -> Option<Vec<(u32, Ty)>> {
        let bytes = self.source.as_bytes();
        let start = self.byte_of(lt) + 1;
        let end = self.byte_of(gt);
        if start > end {
            return None;
        }
        let mut segments = Vec::new();
        let mut seg_start = start;
        let mut i = start;
        let mut depth = 0usize;
        while i < end {
            match bytes[i] {
                b'<' | b'(' | b'[' | b'{' => depth += 1,
                b'>' | b')' | b']' | b'}' => depth = depth.saturating_sub(1),
                b',' if depth == 0 => {
                    segments.push((seg_start, i));
                    seg_start = i + 1;
                }
                b'\'' | b'"' => {
                    let quote = bytes[i];
                    i += 1;
                    while i < end && bytes[i] != quote {
                        if bytes[i] == b'\\' {
                            i += 1;
                        }
                        i += 1;
                    }
                }
                _ => {}
            }
            i += 1;
        }
        segments.push((seg_start, end));
        let mut args = Vec::with_capacity(segments.len());
        for (s, e) in segments {
            let s = {
                let mut s = s;
                while s < e && bytes[s].is_ascii_whitespace() {
                    s += 1;
                }
                s
            };
            if s >= e {
                return None;
            }
            let ty = self.written_arg_ty(s, e)?;
            args.push((self.utf16_of(s), ty));
        }
        Some(args)
    }

    /// One written argument's [`Ty`] over its byte segment: the head
    /// identifier tokenized from the bytes (invocation type-argument
    /// spellings ride no `type_uses` publication — the head is simply the
    /// first identifier, qualified spellings `p.C` keeping the leaf), its
    /// optional nested `<...>` group, and a trailing `?`. `None` on any
    /// residue (a function-form spelling, a non-type head, trailing
    /// bytes the grammar would not admit) — the keep direction.
    fn written_arg_ty(&self, start: usize, end: usize) -> Option<Ty> {
        let bytes = self.source.as_bytes();
        // The head identifier, qualified spellings `p.C` keeping the leaf
        // `C`: ident runs joined by single dots; a head that is not an
        // identifier fails the parse.
        let ident = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
        let mut leaf = start;
        let head_end = loop {
            let mut e = leaf;
            while e < end && ident(bytes[e]) {
                e += 1;
            }
            if e == leaf || bytes[leaf].is_ascii_digit() {
                return None;
            }
            if e + 1 < end && bytes[e] == b'.' && bytes[e + 1] != b'.' {
                leaf = e + 1;
            } else {
                break e;
            }
        };
        let name = self
            .strings
            .intern(std::str::from_utf8(&bytes[leaf..head_end]).ok()?);
        if !matches!(self.type_resolution(name), (TypeResolution::Type, _)) {
            return None;
        }
        match self.strings.resolve(name) {
            "dynamic" => return Some(Ty::Dynamic),
            "void" => return Some(Ty::Void),
            "Null" => return Some(Ty::Null),
            _ => {}
        }
        let nullable = self.written_nullable_suffix(self.utf16_of(head_end));
        let mut cursor = head_end;
        if nullable {
            let mut c = cursor;
            while c < end && bytes[c].is_ascii_whitespace() {
                c += 1;
            }
            if c < end && bytes[c] == b'?' {
                cursor = c + 1;
            }
        }
        let mut c = cursor;
        while c < end && bytes[c].is_ascii_whitespace() {
            c += 1;
        }
        let args = if c < end && bytes[c] == b'<' {
            let (close, after) = {
                let mut depth = 0usize;
                let mut i = c;
                loop {
                    if i >= end {
                        return None;
                    }
                    match bytes[i] {
                        b'<' => depth += 1,
                        b'>' => {
                            depth -= 1;
                            if depth == 0 {
                                break (i, i + 1);
                            }
                        }
                        _ => {}
                    }
                    i += 1;
                }
            };
            if bytes[after..end].iter().any(|b| !b.is_ascii_whitespace()) {
                return None;
            }
            let mut nested = Vec::new();
            let mut seg_start = c + 1;
            let mut i = seg_start;
            let mut depth = 0usize;
            while i < close {
                match bytes[i] {
                    b'<' | b'(' | b'[' | b'{' => depth += 1,
                    b'>' | b')' | b']' | b'}' => depth = depth.saturating_sub(1),
                    b',' if depth == 0 => {
                        let s = {
                            let mut s = seg_start;
                            while s < i && bytes[s].is_ascii_whitespace() {
                                s += 1;
                            }
                            s
                        };
                        if s >= i {
                            return None;
                        }
                        nested.push(self.written_arg_ty(s, i)?);
                        seg_start = i + 1;
                    }
                    _ => {}
                }
                i += 1;
            }
            let s = {
                let mut s = seg_start;
                while s < close && bytes[s].is_ascii_whitespace() {
                    s += 1;
                }
                s
            };
            if s >= close {
                return None;
            }
            nested.push(self.written_arg_ty(s, close)?);
            self.boundary.intern_list(&nested)
        } else {
            if c != end {
                return None;
            }
            ArgListId::EMPTY
        };
        Some(Ty::Named {
            name,
            nullable,
            args,
        })
    }
}

// The #3464 pins (the `duplicate_constructor` head-shape pass). Registered
// here, in the face's own file, via `#[path]`: the module stays inside the
// `diagnose` subtree so the test rig's `pub(super)` items remain visible.
#[cfg(test)]
#[path = "ticket_3464_tests.rs"]
mod ticket_3464_tests;

#[cfg(test)]
mod exec_bounds_pairing_tests {
    use super::super::test_rig::{report_ops, report_ops_bare};

    /// The bound-driven row a generic callee's captured `exec_bounds`
    /// window produces for a bound-violating argument: `T extends num`
    /// clamps a `null` solution to `num`, and the argument check names
    /// `num`. Empty when the window never paired (name-only params carry
    /// no bound), so the row's presence and its `num` spelling are the
    /// observable proof that `exec_bounds` carries the declaration's
    /// bound for that executable.
    fn bound_driven_rows(source: &str) -> Vec<String> {
        report_ops(source)
            .diagnostics
            .iter()
            .filter(|d| d.code.name == "argument_type_not_assignable")
            .map(|d| d.message.to_string())
            .collect()
    }

    /// #3099: a function with a WRITTEN return type pairs its
    /// type-variable bound exactly like a bare (return-less) header. The
    /// capture keys the closed `<...>` window on the declaring name in
    /// both shapes (the return type precedes the name, so it never
    /// disturbs the name-vs-window token-order latch), so the bound rides
    /// `exec_bounds` and the same clamp row fires.
    #[test]
    fn written_return_fn_pairs_its_bound_like_a_bare_fn() {
        let bare = bound_driven_rows("f<T extends num>(T t) => t;\nvoid g() { f(null); }\n");
        let written = bound_driven_rows("T f<T extends num>(T t) => t;\nvoid g() { f(null); }\n");
        assert!(
            bare.iter().any(|m| m.contains("'num'")),
            "bare fn's bound must drive the clamp row: {bare:?}"
        );
        assert_eq!(
            bare, written,
            "a written return type must pair the same bound as a bare header"
        );
    }

    /// The `type_argument_not_matching_bounds` rows of `report_ops`, as
    /// `(offset, message)` pairs.
    fn bounds_rows(source: &str) -> Vec<(u32, String)> {
        report_ops(source)
            .diagnostics
            .iter()
            .filter(|d| d.code.name == "type_argument_not_matching_bounds")
            .map(|d| (d.range.offset, d.message.to_string()))
            .collect()
    }

    /// #3744: a written FUNCTION TYPE standing as an explicit call-site
    /// type argument occupies exactly one parameter slot. Before the
    /// `fn_capture_end` hand-over the group held one member fewer — a
    /// function type fires no `handle_type` leaf — so every LATER argument
    /// shifted one parameter left and the last one went unchecked:
    /// `f2<String, void Function()>()` judged `String` against the FIRST
    /// parameter (`A extends int`) and never checked `B` at all. Oracle
    /// 3.13.7 reports both arguments at their own parameter.
    #[test]
    fn function_type_argument_seats_its_own_parameter_slot() {
        let source = "void f2<A extends int, B extends int>() {}\n\
                      void main() { f2<String, void Function()>(); }\n";
        let messages: Vec<String> = bounds_rows(source).into_iter().map(|(_, m)| m).collect();
        assert_eq!(
            messages.len(),
            2,
            "both arguments check, each at its own parameter: {messages:?}"
        );
        assert!(
            messages[0].starts_with("'String'") && messages[0].ends_with("'A'."),
            "the first argument keeps the first parameter: {messages:?}"
        );
        assert!(
            messages[1].starts_with("'void Function()'") && messages[1].ends_with("'B'."),
            "the function type seats the second parameter: {messages:?}"
        );
    }

    /// #3744: a function type whose RETURN TYPE carries a generic group
    /// (`List<int> Function()`) hands its whole written extent over as one
    /// member — the return type's own completions never stand as members of
    /// their own, so the argument after it keeps its parameter slot.
    #[test]
    fn nested_group_in_function_return_keeps_the_next_slot() {
        let source = "void f2<A extends int, B extends int>() {}\n\
                      void main() { f2<List<int> Function(), String>(); }\n";
        let messages: Vec<String> = bounds_rows(source).into_iter().map(|(_, m)| m).collect();
        assert_eq!(
            messages.len(),
            2,
            "one row per argument, no shifted duplicate: {messages:?}"
        );
        assert!(
            messages[0].starts_with("'List<int> Function()'") && messages[0].ends_with("'A'."),
            "the whole function type is the first argument: {messages:?}"
        );
        assert!(
            messages[1].starts_with("'String'") && messages[1].ends_with("'B'."),
            "the following argument keeps the second slot: {messages:?}"
        );
    }

    /// #3744: the leading-position row anchors at the written annotation's
    /// start — the return type's head, the face
    /// `_typeArgumentErrorNode` returns — never at the `Function` keyword.
    #[test]
    fn leading_function_type_argument_anchors_at_its_return_head() {
        let source = "void f2<A extends int, B extends int>() {}\n\
                      void main() { f2<void Function(), String>(); }\n";
        // The bare rig keeps the fixture's own offsets (`report_ops`
        // prepends a prelude, which would shift every anchor).
        let rows: Vec<(u32, String)> = report_ops_bare(source)
            .diagnostics
            .iter()
            .filter(|d| d.code.name == "type_argument_not_matching_bounds")
            .map(|d| (d.range.offset, d.message.to_string()))
            .collect();
        assert_eq!(rows.len(), 2, "both arguments check: {rows:?}");
        let expected = source.find("void Function()").expect("fixture") as u32;
        assert_eq!(
            rows[0].0, expected,
            "the row anchors at the written annotation's head: {rows:?}"
        );
    }
}

// The #3642 pins (the `wrong_type_parameter_variance_position` /
// `wrong_type_parameter_variance_in_superinterface` pair): the `Ty` →
// `VarTree` conversion's function/record faces, the typedef variance seed,
// and the `covariant`-formal/field exemption faces. Registered here via
// `#[path]` under the same module contract as the #3464 pins above.
#[cfg(test)]
#[path = "ticket_3642_tests.rs"]
mod ticket_3642_tests;
