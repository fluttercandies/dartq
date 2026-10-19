use super::*;

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
                let (resolved, defined) = match self.super_ctor_named_formals(container, selector)
                {
                    Some(formals) => (true, formals.contains(&name)),
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
                        formals.is_some_and(|f| f.named.iter().any(|entry| entry.name == name)),
                    )
                } else {
                    let decl = self
                        .top_level_decl(callee_name)
                        .filter(|d| d.kind == MicroKind::TopLevelFn && d.formals.is_some());
                    let defined = decl
                        .as_ref()
                        .is_some_and(|d| Self::has_named_formal(d.formals.as_deref(), name));
                    (decl.is_some(), defined)
                }
            }
            RowShape::Member { recv } => {
                let formals = self.member_formals_full(recv, callee_name);
                (
                    formals.is_some(),
                    formals.is_some_and(|f| f.named.iter().any(|entry| entry.name == name)),
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

    /// The resolved super-constructor's named-formal spellings
    /// (`ctor_formals`' named half) for `check_named_argument`: `selector`
    /// picks the named table, `None` the unnamed face's key (the enclosing
    /// class's `superclass` spelling — `ctor_formals` keys the unnamed
    /// entry under the declared name). `None` = unresolvable superclass or
    /// a constructor the index does not model — the caller stays silent
    /// (the under-report posture; the oracle's own
    /// `undefined_constructor_in_initializer` owns that report).
    pub(crate) fn super_ctor_named_formals(
        &self,
        container: StringId,
        selector: Option<StringId>,
    ) -> Option<Vec<StringId>> {
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
        let formals = super_container
            .ctor_formals
            .get(&selector.unwrap_or(superclass))?;
        Some(formals.named.iter().map(|f| f.name).collect())
    }

    // —— pass 2 ———————————————————————————————————————————————————————

    /// Resolves the recorded rows in source order and emits.
    pub(crate) fn resolve_rows(&mut self) {
        self.build_ur_sites();
        let rows = std::mem::take(&mut self.rows);
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
                    self.resolve_row(&deferred[index]);
                }
                index += 1;
            }
            self.resolve_row(row);
        }
        for deferred_row in &deferred[index..] {
            if !self
                .conditional_uri_rows
                .contains(&deferred_row.range.offset)
            {
                self.resolve_row(deferred_row);
            }
        }
    }

    /// One row's pass-2 dispatch (shared by the main stack and the
    /// fold-deferred rows).
    pub(crate) fn resolve_row(&mut self, row: &Row) {
        match &row.shape {
            RowShape::Bare(info) => self.resolve_bare_row(row, *info),
            RowShape::Member { recv } => {
                let recv = self.promoted_receiver(row.range.offset, *recv);
                self.resolve_member_row(row, recv);
            }
            RowShape::Receiver(recv) => self.resolve_receiver_row(row, *recv),
            RowShape::TypeUse(..) => self.resolve_type_row(row),
            RowShape::Record(fields) => self.resolve_record_row(row, fields),
            RowShape::Value => {}
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

    pub(crate) fn resolve_bare_row(&mut self, row: &Row, info: BareInfo) {
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
                    self.emit(SemanticCode::ImplicitThisReferenceInInitializer, row.range, name_text, "");
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
                // getter's read (writes never spare a field).
                if !matches!(row.intent, Intent::Write)
                    && let Some(&pushed) = self.field_read_rows.get(&row.range.offset)
                {
                    self.read_names.insert(row.name);
                    if let Some(container) = self
                        .read_owner_container(row.range.offset)
                        .or(Some(pushed))
                    {
                        self.member_read_owners
                            .entry(row.name)
                            .or_default()
                            .insert((self.file, container));
                    }
                }
                // A direct operator application on a declared-typed receiver
                // (`c + d`, `c[i] = v`). A type whose static type the model
                // leaves unknown is the SDK projection's names-only stub —
                // for the dart:core collection heads that is the generic
                // class type literal (`List < 1`), and the operator applies
                // to the literal's static type `Type`, which declares none
                // of the user operators (the `A < 1` container face's
                // decision, one resolution grade down).
                if matches!(row.intent, Intent::Operator(..)) {
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
                            self.emit(
                                SemanticCode::UndefinedOperator,
                                op_range,
                                &op_text,
                                "Type",
                            );
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
                        // UnqualifiedLookupDomain: inside a class or enum
                        // instance member the invocation miss names `this`'s
                        // type (`undefined_method`); everywhere else it is
                        // `undefined_function`.
                        if let Some(container) = this_type {
                            let type_text = self.strings.resolve(container).to_string();
                            if !Self::synthetic_reference(row.range)
                                && !self.keyword_bare_reference(row.range)
                                && !self.guard.ignore(None, row.name)
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
                        // frozen code names the missing async marker.
                        if !self.guard.ignore(None, row.name) {
                            self.emit(SemanticCode::UndefinedIdentifierAwait, row.range, "", "");
                        }
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
                        // The undefined receiver itself reports above all; the
                        // operator on an invalid type never cascades.
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
                if !self.guard.ignore(None, row.name) {
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
    pub(crate) fn take_clause_miss(rows: &mut Vec<(u32, ClauseTag)>, offset: u32) -> Option<ClauseTag> {
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
        let start = range.offset as usize;
        self.source
            .get(start..start + range.length as usize)
            .is_some_and(|text| matches!(text, "var" | "final" | "const" | "dynamic" | "void"))
    }

    /// Marks a read-shaped member access under its resolved receiver
    /// handle (the `unused_field` element-identity face). An unresolved
    /// receiver marks nothing — the name-level `read_names` still feeds
    /// the use face.
    pub(crate) fn note_member_read(&mut self, name: StringId, recv: &Receiver) {
        let handle = match *recv {
            Receiver::This { container, .. } | Receiver::Super { container, .. } => {
                (self.file, container)
            }
            // `C.f` — a static-member read through the container's own name
            // (a `static const` field of a private class rides this face):
            // the receiver IS the owning container, no instance to type.
            Receiver::Container { name: container, .. } => (self.file, container),
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
        self.member_read_owners.entry(name).or_default().insert(handle);
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
    /// field; a sibling class's same-spelled field never does).
    pub(crate) fn member_was_read(&self, container: StringId, name: &StringId) -> bool {
        let Some(receivers) = self.member_read_owners.get(name) else {
            return false;
        };
        receivers.contains(&(self.file, container)) || {
            let hierarchy = FoldHierarchy {
                graph: self.graph,
                namespaces: self.namespaces,
                index: self.index,
                file: self.file,
                lib: self.lib,
                memo: RefCell::default(),
                in_progress: RefCell::default(),
            };
            hierarchy
                .walk_supertypes((self.file, container))
                .iter()
                .any(|handle| receivers.contains(handle))
        }
    }

    pub(crate) fn resolve_member_row(&mut self, row: &Row, recv: Receiver) {
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
        if !self_named && matches!(row.intent, Intent::Read | Intent::ReadWrite) {
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
                    if !self.guard.ignore(None, row.name) {
                        self.emit(code, row.range, name_text, "InvalidType");
                    }
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
            Receiver::PrefixedContainer {
                prefix,
                name,
                kind,
            } => {
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
                        row,
                        Receiver::Typed {
                            type_name: name,
                            ty: Ty::UNKNOWN,
                        },
                    );
                }
            }
            Receiver::This { .. } | Receiver::Typed { .. } => {
                self.check_instance_member(row, recv);
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
    pub(crate) fn check_container_member(&mut self, row: &Row, container: StringId, kind: ContainerKind) {
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
                || resolved.members.get(&row.name).is_some_and(|entry| {
                    matches!(entry.kind, MemberKind::Field) && !entry.is_final
                })
        } else {
            resolved.members.contains_key(&row.name)
        };
        // An enum's synthesized STATIC `values` getter joins the
        // interface face without an entry in `members` — a static read
        // through the container's own name resolves the accessor
        // upstream (`EnumElementImpl`'s synthesized static members; the
        // `index`/`name` synth twins are instance members and keep the
        // miss at this face).
        if !defined
            && kind == ContainerKind::Enum
            && row.name == self.members.enum_synth_members[0]
        {
            return;
        }
        if !defined
            && (resolved.constructors.contains(&row.name)
                || (row.name == self.intern("new")
                    && kind != ContainerKind::Extension
                    && resolved.has_unnamed_constructor))
        {
            // A named-constructor selector (`C.named`) resolves through the
            // creation face — the constructor is the class's own
            // declaration, never an undefined member. The `.new` spelling
            // answers the unnamed constructor the same way
            // (ast_rewrite.dart's `name.lexeme == 'new'` →
            // `unnamedConstructor`); extensions have no constructors.
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
        let (invoke_code, read_code, write_code) = match kind {
            ContainerKind::Extension => (
                SemanticCode::UndefinedExtensionMethod,
                SemanticCode::UndefinedExtensionGetter,
                SemanticCode::UndefinedExtensionSetter,
            ),
            _ => (
                SemanticCode::UndefinedMethod,
                if kind == ContainerKind::Enum {
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
    pub(crate) fn check_instance_member(&mut self, row: &Row, recv: Receiver) {
        let (Receiver::This { container, .. }
        | Receiver::Typed {
            type_name: container,
            ..
        }) = recv
        else {
            return;
        };
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
            match self.walk_members(container, row.name) {
                Walk::Found(entry) => {
                    let ty = Self::member_result_ty(&entry);
                    self.check_operator_expr(ty, row);
                    return;
                }
                Walk::ChainBroken => return,
                Walk::Missing => {
                    let name_text = self.strings.resolve(row.name);
                    let type_text = self.strings.resolve(container).to_string();
                    self.emit(
                        SemanticCode::UndefinedGetter,
                        row.range,
                        name_text,
                        &type_text,
                    );
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
        if self.member_defined(container, receiver_ty, row, false, receiver_nullable) {
            // The implicit-this face (`_checkForInvalidInstanceMemberAccess2`,
            // error_verifier.dart — `ThisContext.instanceFieldDeclaration`
            // disallows `this`): an instance-member reference — implicit or
            // explicit `this` — inside an instance field's declaration
            // initializer reports at the referencing expression. The
            // push-time snapshot set decides (the field group tears down
            // before pass 2); a static member rides no `this` and stays
            // silent.
            if self.field_init_rows.contains(&row.range.offset)
                && !matches!(
                    self.walk_members(container, row.name),
                    Walk::Found(entry) if entry.is_static
                )
            {
                let name_text = self.strings.resolve(row.name);
                self.emit(
                    SemanticCode::ImplicitThisReferenceInInitializer,
                    row.range,
                    &name_text,
                    "",
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
        if matches!(self.walk_members(container, row.name), Walk::ChainBroken) {
            // A chain broken at the root type (`int x; x.noSuchGetter`) is
            // the external-primitive face: the core member table decides.
            // A broken superclass hop suppresses as before. The receiver's
            // nullability rides along (#2140): a member the core table
            // defines is the dereference face's found-arm, gated exactly
            // like the walkable-model face above.
            self.check_core_type_member(container, row, receiver_nullable);
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
            // The synthesized `index`/`values`/`names` getters are instance
            // members: the initializer-context face fires for them too (the
            // corpus's `final int code = index;` resolves exactly here).
            if self.field_init_rows.contains(&row.range.offset) {
                let name_text = self.strings.resolve(row.name);
                self.emit(
                    SemanticCode::ImplicitThisReferenceInInitializer,
                    row.range,
                    &name_text,
                    "",
                );
            }
            return;
        }
        if matches!(kind, ContainerKind::Mixin | ContainerKind::Extension) {
            // The `on` face is invisible: a miss may be a constraint member.
            self.record_miss(row.name, row.range, true, use_kind_of(&row.intent));
            return;
        }
        let name_text = self.strings.resolve(row.name);
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
                Walk::ChainBroken => {}
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

    /// The external-primitive member face (`int x; x.noSuchGetter`): the
    /// receiver type is a dart:core primitive when the name is one of the
    /// [`CORE_TYPE_MEMBERS`] keys and resolves to no indexed container.
    /// Extension members still apply implicitly, so the pool gates first; a
    /// table hit is defined, a miss reports the access-intent trio.
    pub(crate) fn check_core_type_member(&mut self, container: StringId, row: &Row, receiver_nullable: bool) {
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
            // `==`/Object arm below keeps its under-report, and an
            // invocation intent keeps the send face's ownership (the
            // `member_defined` found-arm's split).
            if receiver_nullable
                && !matches!(row.intent, Intent::Invoke)
                && !self.members.object_members.contains(&row.name)
            {
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
        if resolve_container(
            self.graph,
            self.namespaces,
            self.index,
            self.lib,
            self.file,
            name,
        )
        .is_none()
        {
            return;
        }
        let mut missing: Vec<String> = Vec::new();
        for spelling in [read, write].into_iter().flatten() {
            if matches!(self.walk_members(name, spelling), Walk::Missing) {
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
            if receiver_nullable && !matches!(row.intent, Intent::Invoke) {
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
        match self.extension_application(container, receiver_ty, row.name, write, receiver_nullable) {
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
        // type reaches `Object`, and the unindexed SDK collection roots
        // (`List`/`Set` and the `dart:collection` bases, all `Iterable`).
        let reaches = |sub: StringId, sup: StringId| {
            sub == sup
                || sup == object
                || world.named_sub(sub, sup)
                || (sup == iterable && {
                    let text = strings.resolve(sub);
                    matches!(text, "List" | "Set")
                        || crate::member_face::COLLECTION_INTERFACE_ALIASES
                            .iter()
                            .any(|(alias, roots)| *alias == text && roots.contains(&"Iterable"))
                })
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
            if receiver_head != on_head {
                return true;
            }
            let receiver_args = boundary.pool().get(receiver_args).to_vec();
            let on_args = boundary.pool().get(on_args).to_vec();
            // A raw-head receiver (`Map x`) instantiates to bounds — the
            // unbounded-`dynamic` row — so a bare `Map` against `on
            // Map<K, FieldItem>` fails `dynamic <: FieldItem` exactly as
            // the oracle's `applicableTo` rejects it.
            let receiver_args = if receiver_args.is_empty() && !on_args.is_empty() {
                vec![Ty::Dynamic; on_args.len()]
            } else {
                receiver_args
            };
            if receiver_args.len() != on_args.len() {
                return true;
            }
            receiver_args
                .iter()
                .zip(on_args.iter())
                .all(|(&receiver_arg, &on_arg)| {
                    // An `on` argument spelling one of the extension's own
                    // type parameters instantiates against the receiver —
                    // the inferrer binds it, never constrains it.
                    if let Ty::Named {
                        name: arg_name,
                        args,
                        ..
                    } = on_arg
                        && args == ArgListId::EMPTY
                        && face.type_param_names.contains(&arg_name)
                    {
                        return true;
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
                        // A `dynamic` receiver argument is a subtype of a
                        // top-shaped `on` argument only — a concrete `on`
                        // argument rejects it (`isSubtypeOf(dynamic, X)`
                        // fails in the oracle's `applicableTo`); every other
                        // non-named pair stays compatible.
                        return !matches!(receiver_arg, Ty::Dynamic)
                            || matches!(on_arg, Ty::Dynamic);
                    };
                    if arg_args != ArgListId::EMPTY || receiver_args != ArgListId::EMPTY {
                        return true;
                    }
                    reaches(receiver_head, arg_head)
                })
        };
        let mut applicable: Vec<(&ExtensionFace, StringId, FileId)> = Vec::new();
        for &(face, face_file) in faces {
            let Some(on) = face.on_type else { continue };
            if !(face.members.contains_key(&member)
                || (accept_setter && face.members.contains_key(&setter)))
            {
                continue;
            }
            if !reaches(receiver, on) || !arg_applicable(face, face_file) {
                continue;
            }
            applicable.push((face, on, face_file));
        }
        // `_isMoreSpecific` steps 3-4 at the name level: a face whose `on`
        // type is a strict subtype of another applicable face's dominates
        // it. Equal (or unrelated) `on` types stay mutually unordered —
        // the ambiguity the caller reports.
        let dominated = |on: StringId| {
            applicable
                .iter()
                .any(|&(_, other, _)| other != on && reaches(other, on) && !reaches(on, other))
        };
        let mut specific: Vec<(&ExtensionFace, FileId, (String, String))> = Vec::new();
        let mut nullable: Vec<(&ExtensionFace, FileId, (String, String))> = Vec::new();
        for &(face, on, face_file) in &applicable {
            if dominated(on) {
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
        // An ambiguous access resolves no member (the oracle leaves every
        // candidate unreferenced — `unused_element` still fires on each).
        if candidates.len() == 1 {
            let (face, face_file, _) = candidates[0];
            self.extension_member_used.insert(member);
            if accept_setter {
                self.extension_member_used.insert(setter);
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
                for (name, entry) in self.graph.export_scope(target) {
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
        if face_file == self.file
            || self.graph.library_of_file(face_file) == Some(self.lib)
        {
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
    pub(crate) fn member_found(&self, container: StringId, member: StringId, super_walk: bool) -> bool {
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
