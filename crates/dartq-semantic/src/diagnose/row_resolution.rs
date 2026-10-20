use super::*;

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
                        formals
                            .is_some_and(|f| self.named_formal_for(&f, name).is_some()),
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
    pub(crate) fn resolve_rows(&mut self, promotions: &PromotionStaging) {
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
                    self.resolve_row(promotions, &deferred[index]);
                }
                index += 1;
            }
            self.resolve_row(promotions, row);
        }
        for deferred_row in &deferred[index..] {
            if !self
                .conditional_uri_rows
                .contains(&deferred_row.range.offset)
            {
                self.resolve_row(promotions, deferred_row);
            }
        }
    }

    /// One row's pass-2 dispatch (shared by the main stack and the
    /// fold-deferred rows).
    pub(crate) fn resolve_row(&mut self, promotions: &PromotionStaging, row: &Row) {
        match &row.shape {
            RowShape::Bare(info) => self.resolve_bare_row(row, *info),
            RowShape::Member { recv } => {
                let recv = self.promoted_receiver(row.range.offset, *recv);
                self.resolve_member_row(promotions, row, recv);
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
                                let type_text =
                                    self.strings.resolve(container).to_string();
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
        let start = self.byte_of(range.offset);
        let end = self.byte_of(range.offset + range.length);
        self.source
            .get(start..end)
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
        let (container, this_kind) = match recv {
            Receiver::This { container, kind } => (container, Some(kind)),
            Receiver::Typed { type_name, .. } => (type_name, None),
            _ => return,
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
            match self.walk_members(container, row.name) {
                Walk::Found(entry) => {
                    let ty = Self::member_result_ty(&entry);
                    self.check_operator_expr(ty, row);
                    return;
                }
                // The frozen interface faces decide a break at the root or
                // a deeper hop the same as the plain member arm below: a
                // carried member suppresses (its result type is beyond the
                // name-level model — `check_operator_expr` reads `Unknown`),
                // a decidable miss reports the getter read, a genuinely
                // unindexed declaration keeps the under-report.
                Walk::ChainBroken => {
                    match self.frozen_member_verdict(container, row.name) {
                        FrozenFace::Absent => {
                            let name_text = self.strings.resolve(row.name);
                            let type_text = self.strings.resolve(container).to_string();
                            self.emit(
                                SemanticCode::UndefinedGetter,
                                row.range,
                                name_text,
                                &type_text,
                            );
                            self.record_miss(row.name, row.range, true, NameUse::Read);
                        }
                        FrozenFace::Defined | FrozenFace::Unknown => {}
                    }
                    return;
                }
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
            let function_hit = matches!(receiver_ty, Ty::Function { nullable: false, .. })
                && row.name == self.intern("call");
            if record_hit || function_hit {
                return;
            }
            let (decidable, display) = match receiver_ty {
                Ty::Record { nullable: false, .. } => (true, "<unknown>"),
                Ty::Function { nullable: false, .. } => (true, "Function"),
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
                if receiver_nullable && !matches!(row.intent, Intent::Invoke) {
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
            // `Object`-member/`==` arms — a miss decidable through the
            // frozen faces reports the access-intent trio like a walkable
            // clean miss, per face: the read face answers the getter
            // verdict, the write face the pair.
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
            if !matches!(
                self.on_face_walk(container, kind, row.name),
                Walk::Missing
            ) {
                self.record_miss(row.name, row.range, true, use_kind_of(&row.intent));
                return;
            }
            if kind == ContainerKind::Extension && matches!(recv, Receiver::This { .. }) {
                match self.enclosing_extension_on_type(container) {
                    Some(on) => report_on = on,
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
        let type_text = self.strings.resolve(report_on).to_string();
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
    pub(crate) fn frozen_member_verdict(&self, face: StringId, spelling: StringId) -> FrozenFace {
        self.frozen_faces_verdict([(face, self.file)].into_iter(), spelling, true)
    }

    /// The super-face twin of [`Self::frozen_member_verdict`]: `super.m`'s
    /// lookup starts above the enclosing container's own members, so the
    /// climb seeds with the declaration's supertype edges — its superclass,
    /// `with` mixins, `implements` faces and `on` constraints (the same
    /// seed [`Engine::face_member_any`] reads). An unindexed container
    /// itself breaks the chain: `Unknown`.
    pub(crate) fn frozen_super_verdict(&self, container: StringId, spelling: StringId) -> FrozenFace {
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

    /// The shared climb behind [`Self::frozen_member_verdict`] and
    /// [`Self::frozen_super_verdict`] (see there for the three-valued
    /// contract and the hop classification). `anchor` holds when the first
    /// seed IS the receiver type's own spelling rather than a supertype
    /// edge: an unresolvable anchor that no frozen face knows leaves the
    /// receiver statically `InvalidType`, which answers every member face
    /// silently — `Unknown`, never `Absent` (a genuinely undeclared
    /// receiver type must not emit `undefined_*`).
    fn frozen_faces_verdict(
        &self,
        seeds: impl Iterator<Item = (StringId, FileId)>,
        spelling: StringId,
        anchor: bool,
    ) -> FrozenFace {
        let spelling_text = self.strings.resolve(spelling);
        // Whether `name` is a frozen SDK face the member tables carry (the
        // `dart:core`/`dart:async`/`dart:io` interfaces plus the
        // `dart:collection` bases that re-export them) — its members are
        // known exactly, so a spelling it lacks contributes nothing.
        let frozen_face = |name: &str| {
            CORE_INTERFACE_MEMBERS
                .iter()
                .chain(CORE_TYPE_MEMBERS.iter())
                .any(|(container, _)| name == *container)
                || COLLECTION_INTERFACE_ALIASES
                    .iter()
                    .any(|(alias, _)| name == *alias)
        };
        let frozen_hit = |name: &str| {
            let aliases: &[&str] = COLLECTION_INTERFACE_ALIASES
                .iter()
                .find(|(alias, _)| name == *alias)
                .map_or(&[][..], |(_, aliases)| aliases);
            CORE_INTERFACE_MEMBERS
                .iter()
                .chain(CORE_TYPE_MEMBERS.iter())
                .any(|(container, members)| {
                    (name == *container || aliases.contains(container))
                        && members.contains(&spelling_text)
                })
        };
        let mut seen = FxHashSet::default();
        let mut undecidable = false;
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
        loop {
            if !seen.insert(name) {
                let Some((next, next_owner)) = pending.pop_front() else {
                    break;
                };
                name = next;
                owner = next_owner;
                // A queued face is never the receiver type itself.
                at_anchor = false;
                continue;
            }
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
            for super_face in found
                .implements_clause
                .iter()
                .chain(found.on_clause.iter())
            {
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
                matches!(self.frozen_member_verdict(name, spelling), FrozenFace::Absent)
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
        let mut applicable: Vec<(&ExtensionFace, StringId, FileId)> = Vec::new();
        if receiver_valid {
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

    /// Whether the receiver's interned type resolves everywhere — the
    /// oracle's `InvalidType` test `extension_application` gates on. A
    /// written type carrying an unresolvable name anywhere inside
    /// (`List<UnresolvedName>` — the head `List` still resolves, so the
    /// caller's `Ty::UNKNOWN` probe arm runs the head check itself;
    /// `(Missing,)`, `Missing Function()`) is an `InvalidType`: the
    /// resolver reports the spelling's own row and the access resolves
    /// nothing, so no extension member may credit or suppress it. The walk
    /// unfolds the `Ty` recursively through the boundary pool —
    /// `Named.args`, a record's field types, a function type's return and
    /// formal types — every `Named` head re-running the access's own
    /// `type_resolution` verdict; non-named leaves (`dynamic`, `void`, the
    /// bottom shapes) resolve by construction.
    fn extension_receiver_resolves(&self, receiver_ty: Ty) -> bool {
        let mut stack = vec![receiver_ty];
        while let Some(ty) = stack.pop() {
            match ty {
                Ty::Named { name, args, .. } => {
                    if self.type_resolution(name).0 != TypeResolution::Type {
                        return false;
                    }
                    stack.extend_from_slice(self.boundary.pool().get(args));
                }
                Ty::Record { shape, .. } => {
                    let pool = self.boundary.pool();
                    let shape = pool.record_shape(shape);
                    stack.extend_from_slice(&shape.positional);
                    stack.extend(shape.named.iter().map(|&(_, ty)| ty));
                }
                Ty::Function { shape, .. } => {
                    let pool = self.boundary.pool();
                    let shape = pool.function_shape(shape);
                    stack.push(shape.return_ty);
                    stack.extend_from_slice(&shape.positional);
                    stack.extend(shape.named.iter().map(|&(_, ty)| ty));
                }
                _ => {}
            }
        }
        true
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
        assert!(scanned.diagnostics.is_empty(), "scanner diagnostics: {source:?}");
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
        let report = report_with_edge(
            "class C { void m() { Foo(); } }",
            broken_show("Foo"),
        );
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
        let report = report_with_edge(
            "class C { void m() { Foo(); } }",
            broken_bare(),
        );
        assert_eq!(code_names(&report), ["undefined_method"]);
    }

    /// S1 fixture (d): a `show`-listed name read in a const context (the
    /// `const Foo.bar()`-class prefixed-ctor head shape) reports
    /// `undefined_identifier` through the guarded read face — the guard
    /// still applies, so this stays silent.
    #[test]
    fn shown_name_prefixed_ctor_head_stays_suppressed() {
        let report = report_with_edge(
            "const x = Foo.bar();",
            broken_show("Foo"),
        );
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
                builder.import(
                    lib_m,
                    ImportEdge {
                        target: Some(lib_u),
                        prefix: None,
                        deferred: false,
                        combinators: Combinators::unfiltered(),
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
        assert_eq!(code_names(&report), Vec::<&'static str>::new(), "{:?}", code_names(&report));
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
        assert_eq!(code_names(&report), Vec::<&'static str>::new(), "{:?}", code_names(&report));
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
        assert_eq!(code_names(&report), Vec::<&'static str>::new(), "{:?}", code_names(&report));
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
            [
                "undefined_getter",
                "undefined_method",
                "undefined_setter"
            ],
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
