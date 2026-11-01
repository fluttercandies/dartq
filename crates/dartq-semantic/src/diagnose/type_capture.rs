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
            if check.container.is_none()
                && self.decl_head_exempts_init(check.range.offset)
            {
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
        if tokens.get(at).is_some_and(|token| token.utf16_offset == offset) {
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
            Ty::Named {
                name, nullable, ..
            } if nullable || self.strings.resolve(name) != "FutureOr" => nullable,
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
                                    .filter(|formal| {
                                        formal.default_value == Some(super_forward)
                                    })
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
                        if required_positional > supplied_positional
                            || !required_named.is_empty()
                        {
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
                    "class" | "mixin" | "enum" | "extension" | "type" | "abstract"
                    | "base" | "final" | "interface" | "sealed",
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
                let names: Vec<StringId> =
                    capture.params.iter().map(|(name, _)| *name).collect();
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
        // The written `<T, U extends B>` group rides the shape (#2878):
        // the message slots spell it back (`void Function<T>(T)`), the
        // official `FunctionType` display form. #3384: the group is the
        // window-rebuilt slice (declaration order, bounds with their
        // written arguments) when the latch matched, else the captured
        // slice as before.
        let fn_ty = self.boundary.args.borrow_mut().function_generic(
            &type_params,
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
            // #3384: the same rebuilt slice rides the stash —
            // `fn_sig_display` spells the group from it, so the
            // signature-display face inherits the declaration-order
            // fix with the armed-slot face.
            params: type_params.into_boxed_slice(),
            required_positional: capture.required_positional.min(u16::MAX as usize) as u16,
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
    pub(crate) fn closure_capture_end(&mut self, end: u32) -> Option<(u32, Ty)> {
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
            let return_ty = if arrow_body {
                let row = self
                    .rows
                    .iter()
                    .rev()
                    .find(|row| {
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
                    .cloned();
                match row {
                    // A member READ row's `ty` is lazily empty (the merge
                    // leaves the declared-type write to the read faces) —
                    // `initializer_read_ty` re-derives the accessor answer
                    // through the classified receiver; sends/operators read
                    // the folded face `expression_row_ty` carries.
                    Some(row) if row.intent == Intent::Read => {
                        match self.initializer_read_ty(&row) {
                            Ty::UNKNOWN => self.expression_row_ty(&row),
                            ty => ty,
                        }
                    }
                    Some(row) => self.expression_row_ty(&row),
                    None => Ty::Void,
                }
            } else {
                Ty::Void
            };
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
            Some((capture.offset, ty))
        })
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

// The #3464 pins (the `duplicate_constructor` head-shape pass). Registered
// here, in the face's own file, via `#[path]`: the module stays inside the
// `diagnose` subtree so the test rig's `pub(super)` items remain visible.
#[cfg(test)]
#[path = "ticket_3464_tests.rs"]
mod ticket_3464_tests;

#[cfg(test)]
mod exec_bounds_pairing_tests {
    use super::super::test_rig::report_ops;

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
        let written =
            bound_driven_rows("T f<T extends num>(T t) => t;\nvoid g() { f(null); }\n");
        assert!(
            bare.iter().any(|m| m.contains("'num'")),
            "bare fn's bound must drive the clamp row: {bare:?}"
        );
        assert_eq!(
            bare, written,
            "a written return type must pair the same bound as a bare header"
        );
    }

}
