use super::*;

impl ParserListener for DiagnoseListener<'_> {
    // —— frames: containers ——————————————————————————————————————————

    fn begin_class_declaration(
        &mut self,
        begin: Token,
        abstract_kw: Option<Token>,
        sealed_kw: Option<Token>,
        base_kw: Option<Token>,
        interface_kw: Option<Token>,
        final_kw: Option<Token>,
        augment_kw: Option<Token>,
        mixin_kw: Option<Token>,
        name: Token,
    ) {
        self.flow().begin_class_declaration(
            begin,
            abstract_kw,
            sealed_kw,
            base_kw,
            interface_kw,
            final_kw,
            augment_kw,
            mixin_kw,
            name,
        );
        let engine = &mut self.engine;
        // The declaration-site combination rows (`abstract sealed class`,
        // `abstract final base/interface class`, the `mixin class`
        // modifier trio) are the parser's own `SYNTACTIC_ERROR` family —
        // `parser_impl.dart:2984-3002` reports them recoverable at the
        // keyword tokens and the analyzer layer never re-reports — so the
        // engine emits none of them here (upstream Dart SDK source,
        // pkg/analyzer/lib/src/generated/error_verifier.dart: no matching
        // code — the single oracle row is the parser's).
        let name_range = name.range;
        engine.note_container_type_variables(Some(name), false, false);
        let name = engine.name_of(name);
        engine.push_container_frame(ContainerKind::Class, name);
        if let Some(name) = name {
            engine.note_decl_site(name, name_range, DeclKind::Container);
            engine.check_face_inheritance(name, ContainerKind::Class, name_range);
        } else {
            // The nameless recovery container (`name ?? '<unnamed>'`):
            // the face joins under the empty name at the anchor chain.
            let empty = engine.intern("");
            let anchor = engine.nameless_site_range();
            engine.note_decl_site(empty, anchor, DeclKind::Container);
        }
        // The class-modifier restriction faces defer to the written clause
        // spans (#1289 E1): armed here — use sites emit at the body open,
        // the `@sealed` contract at the declaration's end event.
        engine.arm_modifier_checks(begin.range, true, name, name_range);
    }

    fn end_class_declaration(&mut self, begin: Token, end: Token) {
        self.flow().end_class_declaration(begin, end);
        let engine = &mut self.engine;
        // #1314 FAM18: the frame's container name reads before the pop.
        let container = engine.container_context().map(|(c, _)| c);
        engine.bounds.drop_header();
        engine.var_drop_header();
        engine.pop_frame();
        // The deferred modifier checks' last-chance finalize (the body open
        // normally drained the use sites; the `@sealed` contract anchors on
        // the declaring node, complete at this end event).
        engine.run_modifier_use_sites();
        engine.run_sealed_contract(SourceRange {
            offset: end.range.offset,
            length: 0,
        });
        if let Some(container) = container {
            engine.flush_init_instance_fields(container);
        }
        // A primary constructor without a body (`this : ...` member) never
        // resolves a super-invocation target — its `super.` formals
        // associate against the implicit `super()` face at the class's own
        // close.
        let primary_owner = engine.primary_ctor_site;
        // The associated `super.` formals supply the super constructor's
        // parameters — the givenness credit runs while the entries are
        // still buffered (`drain_super_formals` consumes them).
        engine.credit_super_formal_givens(primary_owner, None);
        engine.drain_super_formals(primary_owner, None);
        engine.primary_ctor_site = None;
    }

    fn begin_named_mixin_application(
        &mut self,
        begin: Token,
        _abstract_kw: Option<Token>,
        _sealed_kw: Option<Token>,
        _base_kw: Option<Token>,
        _interface_kw: Option<Token>,
        _final_kw: Option<Token>,
        _augment_kw: Option<Token>,
        _mixin_kw: Option<Token>,
        name: Token,
    ) {
        let engine = &mut self.engine;
        // A named mixin application parses through the same
        // `parseClassOrNamedMixinApplication` production — the modifier
        // combination rows are the parser's `SYNTACTIC_ERROR` family,
        // reported once parse-tier; the engine emits none here (the class
        // header arm documents the upstream citation).
        let name_range = name.range;
        engine.note_container_type_variables(Some(name), false, false);
        let name = engine.name_of(name);
        engine.push_container_frame(ContainerKind::Class, name);
        if let Some(name) = name {
            engine.note_decl_site(name, name_range, DeclKind::Container);
            engine.check_face_inheritance(name, ContainerKind::Class, name_range);
        } else {
            // The nameless recovery container (`name ?? '<unnamed>'`):
            // the face joins under the empty name at the anchor chain.
            let empty = engine.intern("");
            let anchor = engine.nameless_site_range();
            engine.note_decl_site(empty, anchor, DeclKind::Container);
        }
        // The alias has no body-open event: the end event below finalizes
        // both deferred halves (#1289 E1).
        engine.arm_modifier_checks(begin.range, true, name, name_range);
    }

    fn end_named_mixin_application(
        &mut self,
        _begin: Token,
        _class: Token,
        _equals: Token,
        _implements: Option<Token>,
        end: Token,
    ) {
        let engine = &mut self.engine;
        // #1314 FAM18: the frame's container name reads before the pop.
        let container = engine.container_context().map(|(c, _)| c);
        engine.bounds.drop_header();
        // The alias has no body-begin to finalize its header: this event is
        // the finalize (a no-op when a body already closed it).
        engine.var_finish_header();
        engine.pop_frame();
        // Both deferred modifier halves finalize here — the clause spans
        // are complete and the `;` closes the declaring node.
        engine.run_modifier_use_sites();
        engine.run_sealed_contract(end.range);
        if let Some(container) = container {
            engine.flush_init_instance_fields(container);
        }
    }

    fn begin_mixin_declaration(
        &mut self,
        begin: Token,
        augment_kw: Option<Token>,
        base_kw: Option<Token>,
        mixin: Token,
        name: Token,
    ) {
        self.flow()
            .begin_mixin_declaration(begin, augment_kw, base_kw, mixin, name);
        let engine = &mut self.engine;
        // `base` is the only legal mixin modifier; the `final`/`interface`/
        // `sealed` violations are the parser's `SYNTACTIC_ERROR` family,
        // reported once parse-tier — no engine emission here.
        let name_range = name.range;
        engine.note_container_type_variables(Some(name), false, false);
        let name = engine.name_of(name);
        engine.push_container_frame(ContainerKind::Mixin, name);
        if let Some(name) = name {
            engine.note_decl_site(name, name_range, DeclKind::Container);
            engine.check_face_inheritance(name, ContainerKind::Mixin, name_range);
        } else {
            // The nameless recovery container (`name ?? '<unnamed>'`):
            // the face joins under the empty name at the anchor chain.
            let empty = engine.intern("");
            let anchor = engine.nameless_site_range();
            engine.note_decl_site(empty, anchor, DeclKind::Container);
        }
        // The mixin header's clause spans feed the same deferred faces; a
        // mixin has no extends slot (its `on` names buffer straight into
        // the clause capture).
        engine.arm_modifier_checks(begin.range, false, name, name_range);
    }

    fn end_mixin_declaration(&mut self, begin: Token, end: Token) {
        self.flow().end_mixin_declaration(begin, end);
        let engine = &mut self.engine;
        // #1314 FAM18: the frame's container name reads before the pop.
        let container = engine.container_context().map(|(c, _)| c);
        engine.bounds.drop_header();
        engine.var_drop_header();
        engine.pop_frame();
        // The class declaration's finalize shape (the body open normally
        // drained the use sites already).
        engine.run_modifier_use_sites();
        engine.run_sealed_contract(SourceRange {
            offset: end.range.offset,
            length: 0,
        });
        if let Some(container) = container {
            engine.flush_init_instance_fields(container);
        }
    }

    fn begin_enum_declaration(
        &mut self,
        begin: Token,
        augment_kw: Option<Token>,
        enum_keyword: Token,
        name: Token,
    ) {
        self.flow()
            .begin_enum_declaration(begin, augment_kw, enum_keyword, name);
        let engine = &mut self.engine;
        let name_range = name.range;
        engine.enum_decl_site = Some((name_range, augment_kw.is_some()));
        engine.note_container_type_variables(Some(name), true, false);
        let name = engine.name_of(name);
        engine.push_container_frame(ContainerKind::Enum, name);
        // A recovery path that skipped the previous enum's close drops its
        // buffered candidates here (a body never closed never reports).
        engine.pending_unnamed_enum_consts.clear();
        engine.ctor_new_head_kills.clear();
        // #1314 FAM14: the header's clause spans capture arms with a nameless
        // container — the modifier use-site consumers skip on `None` (enums
        // have no official drive), while the clause-ledger snapshots read
        // the same spans.
        engine.arm_modifier_checks(begin.range, false, None, name_range);
        if let Some(name) = name {
            engine.note_decl_site(name, name_range, DeclKind::Container);
            engine.check_face_inheritance(name, ContainerKind::Enum, name_range);
            // The values-field face runs at the finalize pass (the source
            // order here precedes the body's sites).
            engine
                .pending_bounded_checks
                .push(PendingBoundedCheck::Enum {
                    name,
                    range: name_range,
                });
        } else {
            // The nameless recovery container (`name ?? '<unnamed>'`):
            // the face joins under the empty name at the anchor chain.
            let empty = engine.intern("");
            let anchor = engine.nameless_site_range();
            engine.note_decl_site(empty, anchor, DeclKind::Container);
        }
    }

    fn end_enum_declaration(
        &mut self,
        begin: Token,
        enum_keyword: Token,
        left_brace: Token,
        member_count: usize,
        end: Token,
    ) {
        self.flow()
            .end_enum_declaration(begin, enum_keyword, left_brace, member_count, end);
        let engine = &mut self.engine;
        // #1314 FAM18: the frame's container name reads before the pop.
        let container = engine.container_context().map(|(c, _)| c);
        engine.bounds.drop_header();
        engine.var_drop_header();
        engine.pop_frame();
        // #1314 FAM14: the clause-spans capture drains here — a stale
        // capture would buffer file-level types as clause entries.
        engine.deferred_modifier = None;
        if let Some(container) = container {
            engine.flush_init_instance_fields(container);
        }
        // A selector-miss constant's element falls back to the unnamed
        // constructor upstream — the enum's unnamed-ctor site counts as
        // used. The mark lands on the site's own bare name (the container
        // spelling for a written `E()`, the nameless anchor otherwise),
        // which the file-end unused walk's `used_names` read honors.
        for enum_name in std::mem::take(&mut engine.enum_unnamed_ctor_uses) {
            let site_names: Vec<StringId> = engine
                .decl_sites
                .iter()
                .filter(|site| {
                    matches!(
                        site.kind,
                        DeclKind::Constructor { container: c } if c == enum_name
                    )
                })
                .filter(|site| {
                    engine.bare_name(site.name) == enum_name
                        || engine.is_nameless(site.name)
                        || engine.is_bare_new_head(site.name)
                })
                .map(|site| site.name)
                .collect();
            for name in site_names {
                engine.used_names.insert(engine.bare_name(name));
            }
        }
        // The unnamed-constructor verdict: the whole body is parsed, so the
        // recovery kill (`new.named();` naming a constructor) is visible —
        // the official element model is declaration-wide
        // (`visitEnumConstantDeclaration`'s unnamed arm). Buffered constants
        // stay in source order.
        let pending = std::mem::take(&mut engine.pending_unnamed_enum_consts);
        for (enum_name, _, range) in pending {
            if !engine.enum_has_unnamed_constructor(enum_name) {
                engine.emit(SemanticCode::UndefinedEnumConstructor, range, "", "unnamed");
            }
        }
        engine.ctor_new_head_kills.clear();
    }

    fn begin_extension_declaration(
        &mut self,
        augment: Option<Token>,
        extension_keyword: Token,
        name: Option<Token>,
    ) {
        self.flow()
            .begin_extension_declaration(augment, extension_keyword, name);
        let engine = &mut self.engine;
        engine.note_container_type_variables(name, false, false);
        let name = name.and_then(|token| engine.name_of(token));
        engine.push_container_frame(ContainerKind::Extension, name);
    }

    fn end_extension_declaration(
        &mut self,
        begin: Token,
        extension_keyword: Token,
        on: Option<Token>,
        end: Token,
    ) {
        self.flow()
            .end_extension_declaration(begin, extension_keyword, on, end);
        let engine = &mut self.engine;
        engine.bounds.drop_header();
        engine.var_drop_header();
        engine.pop_frame();
    }

    fn begin_extension_type_declaration(
        &mut self,
        augment_kw: Option<Token>,
        extension_keyword: Token,
        name: Token,
    ) {
        self.flow()
            .begin_extension_type_declaration(augment_kw, extension_keyword, name);
        let engine = &mut self.engine;
        let name_range = name.range;
        // #1444: the header's clause-position capture opens here (before
        // the representation) and closes at the body.
        engine.extension_clause_phase = ExtensionClausePhase::Clauses;
        engine.note_container_type_variables(Some(name), false, true);
        let name = engine.name_of(name);
        // The representation checks read the element model (complete since
        // `collect`) and anchor at this name token.
        if let Some(name) = name {
            engine.note_decl_site(name, name_range, DeclKind::Container);
            engine.check_extension_type(name, name_range);
        } else {
            // The nameless recovery container (`name ?? '<unnamed>'`):
            // the face joins under the empty name at the anchor chain.
            let empty = engine.intern("");
            let anchor = engine.nameless_site_range();
            engine.note_decl_site(empty, anchor, DeclKind::Container);
        }
        // The extension-type frame carries its own kind: upstream
        // `visitExtensionTypeDeclaration` opens an `InstanceScope` (the
        // class/mixin scope shape — analyzer source
        // `scope_context.dart:187-222`), not `visitExtensionDeclaration`'s
        // `ExtensionScope`, and `enclosingInstanceElement` is the
        // extension-type element — so in-body bare lookups answer like a
        // class's (own members plus the `implements` interface), and an
        // unqualified miss names `thisType` = `E`
        // (`UnqualifiedLookupDomain`, analyzer source
        // `lookup_failure_reporter.dart:507-553`).
        engine.push_container_frame(ContainerKind::ExtensionType, name);
    }

    fn handle_no_extension_type_body(&mut self, semicolon: Token) {
        // #1444: the `;`-body shape — the clause capture closes with the
        // header.
        let _ = semicolon;
        self.engine.extension_clause_phase = ExtensionClausePhase::Off;
    }

    fn end_extension_type_declaration(
        &mut self,
        begin: Token,
        augment_kw: Option<Token>,
        extension_keyword: Token,
        type_keyword: Token,
        end: Token,
    ) {
        self.flow().end_extension_type_declaration(
            begin,
            augment_kw,
            extension_keyword,
            type_keyword,
            end,
        );
        let engine = &mut self.engine;
        // #1444: the safety-net close (the body open normally dropped the
        // capture).
        engine.extension_clause_phase = ExtensionClausePhase::Off;
        // The representation field's constructor face: `error_verifier.dart`
        // drives `ConstructorFieldsVerifier.addConstructors` for extension
        // types exactly like classes (visitExtensionTypeDeclaration), so
        // every declared constructor that leaves the representation field
        // unbound reports `final_not_initialized_constructor` at its own
        // head. The representation joins the pending set as a final
        // instance field (the primary's formal binds it only for the
        // primary, which is not a member site — secondaries never spare it
        // except through a `this.i` formal or an initializer assignment).
        if let Some(container) = engine.container_context().map(|(c, _)| c) {
            // Only declared SECONDARY constructors join the verifier — the
            // primary formal always binds the representation, so counting it
            // would push the rep into the pending set with no generative
            // ctor site to answer it (the declaration face then fires
            // `final_not_initialized` on `extension type A._(int i) {}`).
            // `constructors` holds the named selectors only: an unnamed
            // primary contributes nothing, a named primary contributes
            // exactly one — the threshold splits at one named row beyond
            // that.
            let declares_ctors = engine
                .index
                .get(engine.file)
                .and_then(|f| f.container(container))
                .is_some_and(|decl| {
                    decl.constructors.len() > usize::from(!decl.has_unnamed_constructor)
                });
            if declares_ctors {
                let reps: Vec<(StringId, SourceRange)> = engine
                    .index
                    .get(engine.file)
                    .and_then(|f| f.container(container))
                    .map(|decl| {
                        decl.primary_formals
                            .iter()
                            .map(|rep| (rep.formal.name, rep.name_range))
                            .collect()
                    })
                    .unwrap_or_default();
                for (name, range) in reps {
                    engine.pending_init_variables.push(PendingInitVariable {
                        name,
                        range,
                        container: Some(container),
                        is_static: false,
                        initialized: false,
                        is_final: true,
                    });
                }
                engine.flush_init_instance_fields(container);
            }
        }
        engine.bounds.drop_header();
        engine.var_drop_header();
        engine.pop_frame();
    }

    // —— the #610 boundedness capture: header clauses, bodies, poison ————
    // A clause event fires after its types parse, so each claims the header
    // sink's roots since the previous claim (`Clause`'s count checks). A
    // body begin finalizes the header; a declaration end is the safety-net
    // drop (handled in the `end_*_declaration` events above).

    fn handle_class_extends(&mut self, extends: Option<Token>, count: usize) {
        // The absent-clause events fire with `None` (count 0 or a recovery
        // count) — no roots to claim.
        if extends.is_some() {
            self.engine.bounds.header_clause(Clause::Extends(count));
            self.engine.var_clause(VarSlot::Extends, Some(count));
        }
        // The clause-entry span capture commits its extends head here
        // whether or not the clause exists (the collector's
        // commit_super_candidate shape — a broken head leaves the slot
        // empty either way).
        self.engine.header_extends_commit();
        // The ordering recovery's tail (implements_before_extends): the
        // tail clause's own entries join the extends face. Gated on the
        // clause existing — the absent-clause event (a plain
        // implements-only header) must leave the ledger alone.
        if extends.is_some() && self.engine.header_implements_seen {
            self.engine.header_extends_tail_feed();
        }
    }

    fn handle_implements(&mut self, implements: Option<Token>, count: usize) {
        if implements.is_some() {
            self.engine.bounds.header_clause(Clause::Implements(count));
            self.engine.var_clause(VarSlot::Implements, Some(count));
            // The tail key: a later extends clause in this header is the
            // ordering recovery's tail clause.
            self.engine.header_implements_seen = true;
        }
        // The trailing-count claim mirrors the collector's unconditional
        // commit (a zero count claims nothing).
        self.engine.header_implements_claim(count);
        // #1314 FAM14: the clause's just-claimed entries join the ledger.
        self.engine
            .clause_trailing_feed(count, ClauseTag::Implements);
    }

    fn handle_class_with_clause(&mut self, _with_keyword: Token) {
        self.engine.bounds.header_clause(Clause::With);
        self.engine.var_clause(VarSlot::With, None);
        // The class-with-clause face: a pending extends head upgrades to
        // the mixin-application superclass tag (`withClause != null`,
        // scope_analyzer.dart:1218-1220). A named mixin application rides
        // the same upgrade below through `clause_alias_super`.
        self.engine.clause_alias_super();
        // #1314 FAM14: the with entries completed before this event.
        self.engine.clause_buffer_snapshot(ClauseTag::With);
        // The mixin-application override check runs against the buffered
        // spans — after the snapshot latches the clause facts, before the
        // drain hands them to `pending_mixin_uses` (upstream Dart SDK
        // source, `pkg/analyzer/lib/src/error/inheritance_override.dart`
        // `InheritanceOverrideVerifier.verify`).
        if let Some((container, _)) = self.engine.container_context() {
            self.engine.check_mixin_application_overrides(container);
        }
        // The resolved entries join the `class_used_as_mixin` teardown.
        self.engine.drain_with_clause_spans();
    }

    fn handle_named_mixin_application_with_clause(&mut self, _with_keyword: Token) {
        self.engine.bounds.header_clause(Clause::With);
        self.engine.var_clause(VarSlot::With, None);
        // The alias's head type latched before the `with` list; the clause
        // boundary closes the extends slot (the collector's shape).
        self.engine.header_extends_commit();
        // #1314 FAM14: the alias's superclass face, then its with entries.
        self.engine.clause_alias_super();
        self.engine.clause_buffer_snapshot(ClauseTag::With);
        if let Some((container, _)) = self.engine.container_context() {
            self.engine.check_mixin_application_overrides(container);
        }
        self.engine.drain_with_clause_spans();
    }

    fn handle_enum_with_clause(&mut self, _with_keyword: Token) {
        self.engine.bounds.header_clause(Clause::With);
        self.engine.var_clause(VarSlot::With, None);
        self.engine.clause_buffer_snapshot(ClauseTag::With);
        if let Some((container, _)) = self.engine.container_context() {
            self.engine.check_mixin_application_overrides(container);
        }
        self.engine.drain_with_clause_spans();
    }

    fn handle_mixin_with_clause(&mut self, _with_keyword: Token) {
        self.engine.bounds.header_clause(Clause::With);
        self.engine.var_clause(VarSlot::With, None);
        self.engine.clause_buffer_snapshot(ClauseTag::With);
    }

    fn handle_mixin_on(&mut self, on: Option<Token>, count: usize) {
        if on.is_some() {
            self.engine.bounds.header_clause(Clause::On(count));
            self.engine.var_clause(VarSlot::On, Some(count));
        }
        self.engine.header_on_claim(count);
        // #1314 FAM14: the constraint entries join the ledger (the
        // additive face).
        self.engine
            .clause_trailing_feed(count, ClauseTag::OnConstraint);
    }

    fn begin_class_or_mixin_or_extension_body(&mut self, kind: DeclarationKind, token: Token) {
        // The flow recorder's extension-header window closes with the
        // body (its `extension_on_head` latch must not race the members'
        // own annotations — `this` staging and the implicit-`this`
        // transport read the on-type long after the header parsed).
        self.flow().begin_class_or_mixin_or_extension_body(kind, token);
        self.engine.bounds.finish_header();
        self.engine.var_finish_header();
        // The header's clause spans are complete: the deferred use-site
        // families emit here, ahead of the body's own rows (#1289 E1).
        self.engine.run_modifier_use_sites();
        // #1444: the header ended — the extension-type clause capture
        // closes with it (the body's types are ordinary written types).
        self.engine.extension_clause_phase = ExtensionClausePhase::Off;
    }

    fn end_class_or_mixin_or_extension_body(
        &mut self,
        _kind: DeclarationKind,
        _count: usize,
        _begin: Token,
        end: Token,
    ) {
        // The container's own static-vs-instance collisions report at its
        // body's close (the member sets are complete).
        self.engine.check_conflicting_static_and_instance();
        // The `@sealed` contract anchors on the whole declaring node; the
        // close brace is its end.
        self.engine.run_sealed_contract(end.range);
    }

    fn handle_no_class_body(&mut self, semicolon: Token) {
        // The body-less declaration shape (`class C;`): the spans are
        // complete and the `;` closes the declaring node.
        self.engine.run_modifier_use_sites();
        self.engine.run_sealed_contract(semicolon.range);
    }

    fn handle_no_mixin_body(&mut self, semicolon: Token) {
        self.engine.run_modifier_use_sites();
        self.engine.run_sealed_contract(semicolon.range);
    }

    fn begin_enum_body(&mut self, _token: Token) {
        self.engine.bounds.finish_header();
        self.engine.var_finish_header();
    }

    fn begin_function_type(&mut self, _begin: Token) {
        let depth = std::mem::take(&mut self.engine.type_arg_depth);
        self.engine.type_arg_depth_saved.push(depth);
        // Function-type shapes are outside the capture's model: poison the
        // open windows (a function-type bound's leaves also fail the
        // one-root claim — this is the belt to those braces).
        self.engine.bounds.poison();
        // The #612 capture models them: the frame routes its return, bounds
        // of its own type parameters, and formals by region.
        self.engine.var_begin_function_type();
        // The written-signature capture (the function-typed-variable
        // invocation face).
        self.engine.fn_capture_begin();
        // The #1351 capture: the RHS's own `Function(` claims the return
        // region; a nested function type (a function-typed formal) hits
        // the non-return region and poisons.
        self.engine.bounds.alias_formal_boundary();
        // The `==` parameter's written type is never a function type (the
        // official's resolved type cannot read `dartCoreObject` here).
        self.engine.equals_type_poison();
    }

    fn begin_record_type(&mut self, left_paren: Token) {
        // #2358: flow latches `tv_in_record` so a record type's own field
        // shapes never capture a type-variable bound (`tv_capturing`).
        self.flow().begin_record_type(left_paren);
        let depth = std::mem::take(&mut self.engine.type_arg_depth);
        self.engine.type_arg_depth_saved.push(depth);
        self.engine.bounds.poison();
        // A record RHS is outside the alias model (#1351).
        self.engine.bounds.alias_poison();
        self.engine.var_begin_record_type();
        self.engine.equals_type_poison();
        // The record's field types are plain type positions, never the
        // enclosing catch clause's `on` head (`on (e) { ... }` — the
        // records-feature ambiguity recovery).
        self.engine.record_type_depth += 1;
        // The written-record capture (F-R): the field types collect on the
        // level until `end_record_type` interns the shape.
        self.engine.record_ty_stack.push(RecordTyLevel::default());
    }

    fn end_record_type(
        &mut self,
        left_paren: Token,
        question: Option<Token>,
        count: usize,
        has_named_fields: bool,
    ) {
        // #2358: flow clears `tv_in_record` — the bound-capture gate closes
        // with the record type's own shape.
        self.flow()
            .end_record_type(left_paren, question, count, has_named_fields);
        if let Some(depth) = self.engine.type_arg_depth_saved.pop() {
            self.engine.type_arg_depth = depth;
        }
        self.engine.var_end_record_type(question.is_some());
        self.engine.record_type_depth -= 1;
        // Fold the captured fields into the interned `Ty::Record` — the
        // trailing `named_count` entries are the named pairs. The result
        // feeds the parent record's pending field type (nested record) or
        // the armed slot (`(int, {String s}) x = …`).
        let engine = &mut self.engine;
        if let Some(level) = engine.record_ty_stack.pop() {
            let named_start = level.fields.len().saturating_sub(level.named_count);
            let mut positional = Vec::with_capacity(named_start);
            let mut named = Vec::with_capacity(level.named_count);
            for (index, ty) in level.fields.iter().copied().enumerate() {
                match level.names.get(index).copied().flatten() {
                    Some(name) if index >= named_start => named.push((name, ty)),
                    _ => positional.push(ty),
                }
            }
            let record_ty =
                engine
                    .boundary
                    .args
                    .borrow_mut()
                    .record(&positional, &named, question.is_some());
            if let Some(parent) = engine.record_ty_stack.last_mut() {
                parent.pending = Some(record_ty);
            } else {
                engine.armed_ty = Some(record_ty);
                engine.armed_ty_unresolved = false;
            }
        }
    }

    fn begin_record_type_entry(&mut self, _first: Token) {
        if let Some(level) = self.engine.record_ty_stack.last_mut() {
            level.pending = None;
        }
    }

    fn end_record_type_entry(&mut self, _last: Token) {
        let engine = &mut self.engine;
        if let Some(level) = engine.record_ty_stack.last_mut() {
            // A recovered hole (`(int, ,)`) never latched a pending type —
            // the field rides the list as `Unknown` (the analyzer's
            // InvalidType splice keeps the record's arity honest).
            level
                .fields
                .push(level.pending.take().unwrap_or(Ty::Unknown));
        }
    }

    fn handle_no_name(&mut self, _token: Token) {
        if let Some(level) = self.engine.record_ty_stack.last_mut() {
            level.names.resize(level.fields.len() + 1, None);
        }
    }

    fn end_record_type_named_fields(&mut self, count: usize, _left_brace: Token) {
        if let Some(level) = self.engine.record_ty_stack.last_mut() {
            level.named_count = count;
        }
    }

    /// Official `beginFormalParameter`: the written `required` keyword
    /// rides the formal-head event — the written-signature capture's named
    /// slots consume it at the formal's name.
    fn begin_formal_parameter(
        &mut self,
        _token: Token,
        _kind: dartq_parser::MemberKind,
        required: Option<Token>,
        _covariant_kw: Option<Token>,
        _var_final_or_const: Option<Token>,
    ) {
        self.engine.capture_formal_required = required.is_some();
    }

    fn begin_formal_parameters(&mut self, _token: Token, kind: dartq_parser::MemberKind) {
        self.engine.var_formal_boundary();
        self.engine.equals_formal_boundary();
        self.engine.fn_capture_formals_begin();
        // The #1351 capture: the open typedef RHS's formals region opens —
        // the trees since the `Function` head become the return type (the
        // head event precedes the return parse).
        if kind == dartq_parser::MemberKind::GeneralizedFunctionType {
            self.engine.bounds.alias_formals_region_open();
        }

        let engine = &mut self.engine;
        // #1444: the representation's formals park the extension-type
        // clause capture — their types are ordinary written types.
        if kind == dartq_parser::MemberKind::PrimaryConstructor
            && engine.extension_clause_phase != ExtensionClausePhase::Off
        {
            engine.extension_clause_phase = ExtensionClausePhase::Representation;
            // The representation's type is no supertype clause: the
            // bounds header capture latches it apart.
            engine.bounds.begin_representation();
        }
        // #1314 FAM10: a formal list without a preceding type-variable
        // window closes the pairing latch — the declaration is non-generic
        // and a later foreign window must not adopt its name.
        if engine.formal_list_depth == 0 {
            engine.pending_exec_name = None;
            engine.pending_exec_is_method = false;
            // The outer list's claims start in the required-positional
            // section; the group events re-arm it.
            engine.fn_formal_named_section = false;
            engine.super_formal_positional_index = 0;
        }
        engine.formal_list_depth += 1;
        engine.closure_formals_begin();
        if engine.formal_list_depth > 1 {
            // A function-typed formal's nested list: park the outer formal's
            // written-type snapshot — the inner formals re-arm the type.
            engine.formal_written_saved = engine.formal_written.take();
        }
    }

    fn end_formal_parameters(
        &mut self,
        count: usize,
        _begin: Token,
        _end: Token,
        kind: dartq_parser::MemberKind,
    ) {
        self.engine.var_formal_region_end();
        self.engine.fn_capture_formals_end();
        self.engine.closure_formals_end();
        // The #1351 capture: the open typedef RHS's formals region closed.
        self.engine.bounds.alias_formals_closed();
        let engine = &mut self.engine;
        // #1444: the representation closed — clause positions resume.
        if kind == dartq_parser::MemberKind::PrimaryConstructor
            && engine.extension_clause_phase == ExtensionClausePhase::Representation
        {
            engine.extension_clause_phase = ExtensionClausePhase::Clauses;
            engine.bounds.end_representation();
        }
        // The body may drive the `noSuchMethod` shape only after the
        // method's own formals close (defaults-window closures predate it).
        engine.arm_no_such_method();
        // The `==` question adjudicates at the outermost list's close.
        engine.equals_parameters_closed(count);
        // #1314 FAM7: the nested list closed — the outer formal's parked
        // snapshot returns (its own `end_formal_parameter` consumes it).
        if engine.formal_list_depth > 0 {
            engine.formal_list_depth -= 1;
        }
        if engine.formal_list_depth == 1 {
            engine.formal_written = engine.formal_written_saved.take();
        }
    }

    fn begin_optional_formal_parameters(&mut self, token: Token) {
        // The #1351 capture: `[` vs `{` selects the positional/named list
        // and latches the required-positional count (a no-op outside the
        // open typedef RHS).
        let opener = self.engine.text_of(token).unwrap_or_default().to_string();
        self.engine.bounds.alias_optional_group(&opener);
        // The written-signature capture: the section split.
        let section = opener.clone();
        self.engine.fn_capture_optional_group(&section);
        self.engine.closure_optional_group(&opener);
    }

    fn begin_formal_parameter_default_value_expression(&mut self, token: Token) {
        self.flow()
            .begin_formal_parameter_default_value_expression(token);
        let engine = &mut self.engine;
        // A default value's expression follows the formal's own written
        // type: snapshot it before closure/type events inside the default
        // can poison (the #748 `==` first-formal read).
        engine.equals_formal_type();
        // Default values are const contexts: a candidate creation inside
        // one is implicitly `const` (#749's guard).
        engine.default_value_depth += 1;
        // #1314 FAM7: the separator's spelling decides the obsolete-colon
        // row (the event's token IS the separator, `=` or `:`).
        engine.formal_has_default = true;
        engine.formal_colon = (engine.text_of(token) == Some(":")).then_some(token.range);
    }

    fn end_formal_parameter_default_value_expression(&mut self, token: Token) {
        self.flow()
            .end_formal_parameter_default_value_expression(token);

        self.engine.default_value_depth -= 1;
        let _ = token;
    }

    fn begin_function_typed_formal_parameter(&mut self, token: Token) {
        self.flow().begin_function_typed_formal_parameter(token);
        let engine = &mut self.engine;
        engine.equals_type_poison();
        // An inline function-typed formal inside a written-signature
        // capture poisons it (the face cannot attribute the nested list).
        engine.fn_capture_poison();
        // The open literal's capture cannot render an inline
        // function-typed formal either.
        if let Some(capture) = engine.closure_caps.last_mut() {
            capture.poisoned = true;
        }
        // #1463: the formal's name token is consumed without an identifier
        // event — latch the shape for the `end_formal_parameter` bind.
        engine.fn_typed_formal_open = true;
        // #1758: open the return-type slot — the parser re-walks the return
        // annotation right after this event (before the nested list), so the
        // next completion AT THIS SAME type-argument depth is the formal's
        // written return type (a `List<int>`s arg completes deeper and never
        // claims it).
        engine
            .fn_formal_ret_pending
            .push((engine.group_heads.len(), None));
    }

    fn handle_no_formal_parameters(&mut self, token: Token, kind: dartq_parser::MemberKind) {
        self.flow().handle_no_formal_parameters(token, kind);
        let engine = &mut self.engine;
        // A getter head (`get noSuchMethod`) has no list: the body may
        // drive the shape. The `==` latch with no list cannot reach its
        // one-parameter question.
        engine.arm_no_such_method();
        engine.members.equals_check = None;
    }

    // —— frames: executables ——————————————————————————————————————————
    // The Executable frame opens at the declaration head so its type
    // parameters and formals live inside it; the body block nests within.

    fn begin_method(
        &mut self,
        kind: DeclarationKind,
        augment_kw: Option<Token>,
        external_kw: Option<Token>,
        static_kw: Option<Token>,
        covariant_kw: Option<Token>,
        var_final_or_const: Option<Token>,
        get_or_set: Option<Token>,
        name: Token,
        enclosing_declaration_name: Option<&str>,
    ) {
        self.flow().begin_method(
            kind,
            augment_kw,
            external_kw,
            static_kw,
            covariant_kw,
            var_final_or_const,
            get_or_set,
            name,
            enclosing_declaration_name,
        );

        let engine = &mut self.engine;
        // The operator-head probe reads the raw token kind up front, before
        // the shadowing intern below (the operator token is not an
        // identifier).
        let name_is_identifier = engine
            .tokens
            .get(name.id as usize)
            .is_some_and(|t| t.kind == dartq_scanner::TokenKind::Identifier);
        let is_setter = get_or_set.and_then(|t| engine.text_of(t)) == Some("set");
        engine.open_member_setter = is_setter;
        engine.open_member_static = static_kw.is_some();
        engine.ctor_pending = None;
        engine.open_toplevel_site = None;
        engine.open_exec_site = None;
        // #1314 FAM7: a fresh executable starts with no staged rows; the
        // `augment` exclusion arms from its own keyword.
        engine.pending_default_checks.clear();
        engine.open_exec_augment = augment_kw.is_some();
        engine.push_executable_frame(static_kw.is_none());
        // The W5 member-invocation latches arm at the head, keyed on the
        // raw name token text (the official checks read the lexeme, so
        // getters/setters spelled the same join the same face).
        engine.begin_equals_parameter(name);
        engine.begin_no_such_method(name);
        // The `invalid_override` check runs at the declaration head: the
        // element model is already complete (`collect` parsed before this
        // pass), and this event carries the name token whose span anchors
        // the emission. Setters live under `name=` in the model.
        let name_range = name.range;
        let Some(name) = engine.name_of(name) else {
            return;
        };
        let is_setter = engine.open_member_setter;
        let spelling = if is_setter {
            engine.setter_spelling(name)
        } else {
            name
        };
        engine.check_override(spelling, name_range);
        // The `@override` presence face and the `@mustCallSuper` probe read
        // the pending annotation latch before any site registration drains
        // it; the probe's verdict adjudicates at the body's end.
        engine.check_override_annotation(spelling, name_range);
        engine.probe_must_call_super(spelling, name_range);
        // The operator head (`operator []=`): the operator token itself is
        // the name and no `MethodDeclaration` identifier event ever follows
        // it, so the named method's site note — which is what drains the
        // annotation latch — never fires. The latch the checks above read
        // would otherwise leak onto the next declaration's head (a
        // member's `@override` reporting on the following static helper).
        // The collector joins the operator spelling as a method member at
        // its own `begin_method`; mirror that site note here.
        if !name_is_identifier {
            let is_static = engine.open_member_static;
            let container = engine.container_context().map_or(spelling, |(c, _)| c);
            let site = engine.note_decl_site(
                spelling,
                name_range,
                DeclKind::Member {
                    kind: MemberKind::Method,
                    is_static,
                    container,
                },
            );
            engine.open_exec_site = Some(site);
        }
    }

    fn end_method(
        &mut self,
        kind: DeclarationKind,
        get_or_set: Option<Token>,
        begin: Token,
        begin_param: Token,
        begin_initializers: Option<Token>,
        end: Token,
    ) {
        self.flow().end_method(
            kind,
            get_or_set,
            begin,
            begin_param,
            begin_initializers,
            end,
        );

        let engine = &mut self.engine;
        // The setter's written return annotation decides before the frame
        // pops (the `non_void_return_for_setter` face).
        engine.check_setter_return(get_or_set);
        // The completion staging reads the executable frame before it pops
        // (the `body_might_complete_normally_nullable` row).
        engine.stage_body_completion(get_or_set, end);
        // The `must_call_super` verdict reads the body's super face before
        // the executable frame closes.
        let super_invoked = engine.top_executable_super_invoked();
        engine.finish_must_call_super(super_invoked);
        // The W5 latches die with the declaration (a body that never
        // matched — `;`/native/recovery — stays silent by exhaustion).
        engine.members.equals_check = None;
        engine.members.nsm_check = None;
        // #1314 FAM7: recovery paths without a body event drop the staged
        // rows (the under-report posture).
        engine.drop_default_checks();
        engine.open_ctor_site = None;
        engine.pop_frame();
    }

    fn begin_top_level_method(
        &mut self,
        last_consumed: Token,
        augment: Option<Token>,
        external: Option<Token>,
    ) {
        self.flow()
            .begin_top_level_method(last_consumed, augment, external);

        let engine = &mut self.engine;
        engine.open_member_setter = false;
        engine.open_member_static = false;
        engine.ctor_pending = None;
        engine.open_exec_site = None;
        // #1314 FAM7: a fresh executable starts with no staged rows; the
        // `augment` exclusion arms from its own keyword.
        engine.pending_default_checks.clear();
        engine.open_exec_augment = augment.is_some();
        engine.push_executable_frame(false);
    }

    fn end_top_level_method(&mut self, begin: Token, get_or_set: Option<Token>, end: Token) {
        self.flow().end_top_level_method(begin, get_or_set, end);

        let engine = &mut self.engine;
        // The setter's written return annotation decides before the frame
        // pops (the `non_void_return_for_setter` face).
        engine.check_setter_return(get_or_set);
        // A top-level setter's site spelling fixes up here (`set` parses
        // before the name event): setters live under `name=`.
        let site = engine.open_toplevel_site.take();
        let is_setter = get_or_set.and_then(|t| engine.text_of(t)) == Some("set");
        if let (Some(site), true) = (site, is_setter) {
            let bare = engine.decl_sites[site].name;
            engine.decl_sites[site].name = engine.setter_spelling(bare);
        }
        // The completion staging reads the executable frame before it pops
        // (the `body_might_complete_normally_nullable` row).
        engine.stage_body_completion(get_or_set, end);
        // #1314 FAM7: recovery paths without a body event drop the rows.
        engine.drop_default_checks();
        engine.pop_frame();
    }

    fn begin_constructor(
        &mut self,
        kind: DeclarationKind,
        augment_kw: Option<Token>,
        external_kw: Option<Token>,
        static_kw: Option<Token>,
        covariant_kw: Option<Token>,
        var_final_or_const: Option<Token>,
        get_or_set: Option<Token>,
        new_kw: Option<Token>,
        name: Token,
        enclosing_declaration_name: Option<&str>,
    ) {
        self.flow().begin_constructor(
            kind,
            augment_kw,
            external_kw,
            static_kw,
            covariant_kw,
            var_final_or_const,
            get_or_set,
            new_kw,
            name,
            enclosing_declaration_name,
        );

        let engine = &mut self.engine;
        // A `const` constructor makes its container a provable const-ctor
        // target (the #279 arg-check gate); its initializer list is a
        // const context too (the #749 candidate guard).
        engine.in_const_ctor = var_final_or_const.and_then(|t| engine.text_of(t)) == Some("const");
        engine.open_member_setter = false;
        engine.open_member_static = false;
        engine.open_toplevel_site = None;
        engine.open_exec_site = None;
        // The constructor's site opens at its head under the class-name
        // spelling (the unnamed face's own name, and the folded creation
        // row's spelling a `_C()` invocation marks given under); the
        // selector continuation rewrites it in place for `C.named`. A
        // recovered head registers the nameless face under the anchor
        // chain (`name ?? '<unnamed>'`), still rewritten by a selector.
        let container = engine.container_context().map(|(c, _)| c);
        engine.ctor_pending = container.map(|c| {
            let site = match engine.name_of(name) {
                Some(resolved) => {
                    let site = engine.note_decl_site(
                        resolved,
                        name.range,
                        DeclKind::Constructor { container: c },
                    );
                    // The head's own span (the class-name token): the
                    // field-initialization and super-call rows anchor there
                    // even after the selector continuation rewrites the
                    // site's range.
                    engine.ctor_head_anchors.insert(site, name.range);
                    site
                }
                None => {
                    let empty = engine.intern("");
                    let anchor = engine.nameless_site_range();
                    engine.note_decl_site(empty, anchor, DeclKind::Constructor { container: c })
                }
            };
            engine.open_ctor_site = Some(site);
            (c, Some(site))
        });
        // #1314 FAM7: a fresh executable starts with no staged rows; the
        // `augment` exclusion arms from its own keyword.
        engine.pending_default_checks.clear();
        engine.open_exec_augment = augment_kw.is_some();
        // The `new.head` recovery (`new.named();`): the official dot-name
        // recovery names the constructor `head`, so the declaration is a
        // NAMED constructor and the container's implicit unnamed face is
        // gone (`newConstructorDotName`, parser_impl.dart:5965-5977). The
        // collector's capture never sees a selector event for the shape,
        // so the diagnoser corrects the face for the enum-constant check
        // (`new()` keeps the unnamed face, `new new()` names it).
        if new_kw.is_some()
            && let Some(container) = container
            && let Some(text) = engine.text_of(name)
            && text != "new"
            && text != "("
            && engine.strings.resolve(container) != text
        {
            engine.ctor_new_head_kills.insert(container);
        }
        engine.push_executable_frame(true);
    }

    fn end_constructor(
        &mut self,
        kind: DeclarationKind,
        begin: Token,
        new_kw: Option<Token>,
        begin_param: Token,
        begin_initializers: Option<Token>,
        end: Token,
    ) {
        self.flow()
            .end_constructor(kind, begin, new_kw, begin_param, begin_initializers, end);

        let engine = &mut self.engine;
        engine.ctor_pending = None;
        engine.in_const_ctor = false;
        // A constructor without an initializer list drains its `super.`
        // formals against the unnamed face (`superConstructor` resolves to
        // the implicit `super()` — `end_initializers` already drained the
        // explicit-redirect case). Only this constructor's entries drain:
        // a primary constructor's `None`-owned buffer outlives it.
        let owner = engine.open_ctor_site;
        // The givenness credit beside the drain (the entries die in it).
        engine.credit_super_formal_givens(owner, None);
        engine.drain_super_formals(owner, None);
        // #1314 FAM7: recovery paths without a body event drop the rows.
        engine.drop_default_checks();
        engine.pop_frame();
    }

    fn begin_factory(
        &mut self,
        kind: DeclarationKind,
        last_consumed: Token,
        augment_kw: Option<Token>,
        external_kw: Option<Token>,
        const_keyword: Option<Token>,
    ) {
        self.flow()
            .begin_factory(kind, last_consumed, augment_kw, external_kw, const_keyword);

        let engine = &mut self.engine;
        // A `const` factory is a const-constructor target; the latch also
        // rides onto its declaration site (the literal-constructor pass's
        // const face; `end_factory` clears it).
        engine.in_const_ctor = const_keyword.is_some();
        // Factories have no `this`.
        engine.open_member_setter = false;
        engine.open_member_static = false;
        engine.open_toplevel_site = None;
        engine.open_exec_site = None;
        // No provisional site: a factory's optional parameters stay out of
        // the `unused_element_parameter` face (its given-matching needs
        // factory-invocation rows).
        engine.ctor_pending = engine.container_context().map(|(c, _)| (c, None));
        // The name event latches the container as the factory's return type
        // (`error_verifier.dart` checks an `=>` body against the
        // constructor's return type — the class itself), including the
        // `new`-recovery heads (`factory new() => 0;`).
        engine.factory_ret_open = true;
        engine.factory_head = None;
        // #1314 FAM7: a fresh executable starts with no staged rows; the
        // `augment` exclusion arms from its own keyword.
        engine.pending_default_checks.clear();
        engine.open_exec_augment = augment_kw.is_some();
        engine.push_executable_frame(false);
    }

    fn end_factory(&mut self, kind: DeclarationKind, begin: Token, factory: Token, end: Token) {
        self.flow().end_factory(kind, begin, factory, end);

        let engine = &mut self.engine;
        engine.ctor_pending = None;
        engine.in_const_ctor = false;
        // An unnamed factory joins the duplicate-constructor pass as `new`;
        // a `factory name(` head (primary-constructor syntax) names its own
        // constructor and keys under that name instead.
        if let Some((head, false)) = engine.factory_head.take()
            && let Some((container, _)) = engine.container_context()
        {
            let text = engine
                .source
                .get(head.offset as usize..head.end() as usize)
                .unwrap_or("");
            let key = if text == "new" || text == engine.strings.resolve(container) {
                engine.intern("new")
            } else {
                engine.intern(text)
            };
            engine.unnamed_factory_heads.push((container, key, head));
        }
        // A recovery path whose name event never fired drops the latch.
        engine.factory_ret_open = false;
        // #1314 FAM7: recovery paths without a body event drop the rows.
        engine.drop_default_checks();
        engine.pop_frame();
    }

    fn begin_local_function_declaration(&mut self, token: Token) {
        self.flow().begin_local_function_declaration(token);

        // The closure/local-function face leaves the enclosing site: its
        // parameters are never `unused_element_parameter` candidates (the
        // official `_isUsedElement` enclosing-element gate), and its locals
        // close with their own frame. The wildcard-dead check latches the
        // declaration start; the name event patches the verdict.
        // `instance` inherits the enclosing accessibility — including a
        // `late` instance field's initializer, whose `withThisAccessibility`
        // span covers nested closures (the local fn has no executable of its
        // own below it to inherit from).
        let instance = self.engine.instance_context() || self.engine.field_initializer_binds_this();
        let engine = &mut self.engine;
        engine.open_exec_site = None;
        engine.pending_local_fn = None;
        engine.local_fn_wildcard.push((token.range.offset, false));
        // #1314 FAM7: a fresh executable starts with no staged rows (a
        // closure nested in a default value leaves the outer rows alone).
        if engine.default_value_depth == 0 {
            engine.pending_default_checks.clear();
            engine.open_exec_augment = false;
        }
        engine.push_executable_frame(instance);
    }

    fn end_local_function_declaration(&mut self, end: Token) {
        self.flow().end_local_function_declaration(end);

        let engine = &mut self.engine;
        // Materialize after the pop: the function's own frame mark covers the
        // decl index otherwise, so `close_frame_locals` would judge the decl
        // at the function's close (before any later use) and truncate it.
        // After the pop it joins the enclosing frame's window — the official
        // verifier judges local functions at their scope's close.
        engine.drop_default_checks();
        engine.pop_frame();
        engine.finish_local_fn_site();
        // An `_`-named local function is dead code over the whole
        // declaration, used or not (DeadCodeVerifier's
        // visitFunctionDeclaration, dead_code_verifier.dart:72-82 — the
        // wildcard-variables posture).
        if let Some((start, wildcard)) = engine.local_fn_wildcard.pop()
            && wildcard
        {
            let range = SourceRange {
                offset: start,
                length: end.range.end().saturating_sub(start),
            };
            engine.emit(SemanticCode::DeadCode, range, "", "");
        }
    }

    /// The function name's completion: the declaration flavor patches the
    /// open wildcard-dead latch with its name's verdict. Named function
    /// expressions share the identifier context but carry
    /// `is_function_expression` — the official face is
    /// `visitFunctionDeclaration` only, so they must not patch.
    fn end_function_name(&mut self, begin: Token, end: Token, is_function_expression: bool) {
        // The flow recorder's local-function name latch rides this hook
        // ([`dartq_flow::ev::Ev::LocalFnName`]) — the call-resolution face
        // must see function-local names before the body's events run.
        self.flow().end_function_name(begin, end, is_function_expression);
        if is_function_expression {
            return;
        }
        let engine = &mut self.engine;
        let Some(name) = engine.name_of(begin) else {
            return;
        };
        let wildcard = engine.is_wildcard(name);
        if let Some(last) = engine.local_fn_wildcard.last_mut() {
            last.1 = wildcard;
        }
    }

    fn begin_function_expression(&mut self, token: Token) {
        self.flow().begin_function_expression(token);

        self.engine.nsm_poison();
        self.engine.const_capture.begin_opaque_group();
        // A closure inside a `late` instance field's initializer binds
        // `this` too — the `bindsThis` accessibility region covers the
        // whole initializer.
        let instance = self.engine.instance_context() || self.engine.field_initializer_binds_this();
        let engine = &mut self.engine;
        engine.open_exec_site = None;
        engine.pending_local_fn = None;
        // #1314 FAM7: a fresh executable starts with no staged rows (a
        // closure nested in a default value leaves the outer rows alone).
        if engine.default_value_depth == 0 {
            engine.pending_default_checks.clear();
            engine.open_exec_augment = false;
        }
        engine.push_executable_frame(instance);
        engine.closure_capture_begin(token.range.offset);
    }

    fn end_function_expression(&mut self, begin: Token, end: Token) {
        self.flow().end_function_expression(begin, end);

        let engine = &mut self.engine;
        engine.closure_capture_end(end.range.end());
        engine.const_capture.end_opaque_group(end.range);
        // #1314 FAM7: recovery paths without a body event drop the rows.
        engine.drop_default_checks();
        engine.pop_frame();
        engine.push_value_row(end);
    }

    fn begin_named_function_expression(&mut self, token: Token) {
        self.flow().begin_named_function_expression(token);

        self.engine.nsm_poison();
        // Same `this`-accessibility inheritance as the anonymous form.
        let instance = self.engine.instance_context() || self.engine.field_initializer_binds_this();
        let engine = &mut self.engine;
        engine.open_exec_site = None;
        engine.pending_local_fn = None;
        // #1314 FAM7: a fresh executable starts with no staged rows (a
        // closure nested in a default value leaves the outer rows alone).
        if engine.default_value_depth == 0 {
            engine.pending_default_checks.clear();
            engine.open_exec_augment = false;
        }
        engine.push_executable_frame(instance);
    }

    fn end_named_function_expression(&mut self, end: Token) {
        self.flow().end_named_function_expression(end);

        let engine = &mut self.engine;
        // Same pop-before-materialize discipline as the declaration form.
        // #1314 FAM7: recovery paths without a body event drop the rows.
        engine.drop_default_checks();
        engine.pop_frame();
        engine.finish_local_fn_site();
        engine.push_value_row(end);
    }

    // —— frames: scopes ———————————————————————————————————————————————

    fn begin_typedef(&mut self, _token: Token) {
        self.engine.push_block_frame();
    }

    fn end_typedef(
        &mut self,
        _augment: Option<Token>,
        _typedef: Token,
        _equals: Option<Token>,
        _end: Token,
    ) {
        let engine = &mut self.engine;
        // The #1351 capture finalizes with the declaration.
        engine.bounds.end_alias();
        // A typedef's function-type RHS is not a variable annotation.
        engine.fn_stash_drop();
        engine.pop_frame();
    }

    fn begin_catch_clause(&mut self, token: Token) {
        self.flow().begin_catch_clause(token);

        // The catch frame owns the catch parameters; the body block's own
        // events are not overridable from outside dartq-parser (BlockKind
        // is crate-private), so block scoping comes from the executable and
        // catch frames only. The frame closes at `handle_catch_block` —
        // after the body — so the parameters stay visible in it and their
        // locals close with the clause.
        let engine = &mut self.engine;
        engine.catch_clause_start = Some(token.range);
        engine.catch_on_type = None;
        engine.catch_on_nullable = None;
        engine.catch_params = 0;
        engine.push_block_frame();
    }

    fn end_catch_clause(&mut self, token: Token) {
        self.flow().end_catch_clause(token);

        // The frame pop moved to `handle_catch_block` (the clause's body
        // block parses between the two events).
    }

    fn begin_for_statement(&mut self, token: Token) {
        self.flow().begin_for_statement(token);

        let engine = &mut self.engine;
        engine.push_block_frame();
        // The head's declaration list is its own duplicate scope
        // (`checkForVariables`) — reset per loop.
        engine.dup_for_parts.clear();
        // Reset the for-in declarator latch (#2506): a classic-for head's
        // declarator latches below and never consumes it (no
        // `handle_for_in_loop_parts`), so the reset belongs at every loop
        // head — the stale latch dies here, never inside a later for-in.
        engine.for_in_loop_var = None;
        // The condition/updater window (classic for) or the iterable window
        // (for-in) opens at the keyword — the statement form (1).
        engine.in_for_parts.push(1);
    }

    fn end_for_statement(&mut self, end: Token) {
        self.flow().end_for_statement(end);

        self.engine.pop_frame();
    }

    fn handle_for_in_loop_parts(
        &mut self,
        await_kw: Option<Token>,
        for_kw: Token,
        left_paren: Token,
        pattern_keyword: Option<Token>,
        in_kw: Token,
    ) {
        self.flow()
            .handle_for_in_loop_parts(await_kw, for_kw, left_paren, pattern_keyword, in_kw);

        // No frame opens here: the loop variable's declaration events fire
        // BEFORE this event (the parser detects the `in` while parsing the
        // head's declaration), so the owning frame must already exist — the
        // statement form's opened at `begin_for_statement` (closed by
        // `end_for_in` alone: `end_for_statement` never fires for a for-in),
        // the collection element form's at `begin_for_control_flow` (closed
        // by `end_for_in_control_flow`). A push here would strand the loop
        // variable in a frame popped before it closed, or strand a mark that
        // shifts every later close by one.

        // The loop variable's element-type staging (#2506): a property-read
        // iterable (`for (var d in unit.directives)`) carries no written-arg
        // inference the flow layer can read, so the loop variable's row
        // would latch `Unknown` — the InvalidType-scrutinee posture the
        // second DEAD_CODE synthesizer keys on. The element face (the
        // sequence head's first written argument, gated by the same
        // `implements` walk as the iterable check below) stages on the flow
        // declared-type seam, keyed on the latched declarator token. The
        // mark is only peeked here — the iterable check pops it below.
        let loop_var = self.engine.for_in_loop_var.take();
        if let (Some(&mark), Some((name, name_tok))) =
            (self.engine.for_in_marks.last(), loop_var.as_ref())
            && let Some((sem_ty, staged)) =
                self.engine.for_in_element_staging(mark, await_kw.is_some())
        {
            // The loop variable's own declared slot takes the element type
            // — the same inferred-type fill the resolver gives the `var`
            // head (`bind_declared_type` latched `Unknown` for it). The
            // fill is what later read rows resolve at fold time: without
            // it a staged variable in a binary operand resolves `Unknown`
            // and the numeric refinement answers `double`
            // (`int + e` over `for (var e in <int list>)` mis-fired
            // INVALID_ASSIGNMENT). A typed head (`for (int e …)`) keeps
            // its spelled annotation — the slot is not `Unknown`.
            if self.engine.local_declared_ty(*name) == Some(Ty::UNKNOWN) {
                self.engine.bind_var_type(*name, sem_ty.clone());
            }
            match staged {
                dartq_flow::ty::StagedArg::Named {
                    lexeme,
                    nullable,
                    args,
                } => {
                    let base = self.flow().intern_name(&lexeme);
                    self.flow().stage_inferred_local(
                        name_tok.offset,
                        FlowTy {
                            base: FlowBase::Name(base),
                            nullable,
                        },
                        args,
                    );
                }
                // The raw-`Iterable` element: the flow's `dynamic` base,
                // never `Unknown`.
                dartq_flow::ty::StagedArg::Dynamic => {
                    self.flow().stage_inferred_local(
                        name_tok.offset,
                        FlowTy {
                            base: FlowBase::Dynamic,
                            nullable: false,
                        },
                        Vec::new(),
                    );
                }
                _ => {}
            }
        }

        // The `_checkForEachParts` iterable face: the iterable's operand
        // rows closed at `end_for_in_expression` and the `await` keyword
        // arrives only now, so the assignability ask runs here.
        if let Some(mark) = self.engine.for_in_marks.pop() {
            self.engine.check_for_in_iterable(mark, await_kw.is_some());
        }
    }

    fn end_for_in(&mut self, end: Token) {
        self.flow().end_for_in(end);

        self.engine.pop_frame();
    }

    // —— scopes: locals and type parameters ———————————————————————————

    fn begin_variables_declaration(
        &mut self,
        token: Token,
        late_kw: Option<Token>,
        var_final_or_const: Option<Token>,
    ) {
        self.flow()
            .begin_variables_declaration(token, late_kw, var_final_or_const);

        let engine = &mut self.engine;
        // The group's declared type arms here (one `parse_type` per group,
        // `handle_no_type` disarmed it for untyped groups); every name in
        // the group binds it.
        engine.pending_decl_ty = engine.armed_ty.take();
        engine.pending_decl_ty_unresolved = engine.armed_ty_unresolved;
        engine.armed_ty_unresolved = false;
        // #1314 FAM18: the group's modifier bits gate the
        // uninitialized-non-nullable latch (`late`/`final` declarations
        // report through the parser-owned rows or stay silent). The `late`
        // bit never re-latches under an open field group: the group head
        // consumed the keyword, so the variable event's `late_kw` is
        // `None` there and a re-latch would clobber the group's bit —
        // `field_initializer_binds_this` / the implicit-this face's
        // `!group_late` leg read it later, inside the initializer (a
        // `late` field's initializer binds `this`, upstream
        // `variable_declaration_resolver.dart`'s `bindsThis`). Local
        // groups (no field group open) own the event's keywords.
        if !engine.field_group_open {
            engine.group_late = late_kw.is_some();
        }
        engine.group_final = var_final_or_const
            .and_then(|t| engine.text_of(t))
            .is_some_and(|text| text == "final");
        engine.group_const = var_final_or_const
            .and_then(|t| engine.text_of(t))
            .is_some_and(|text| text == "const");
        // Field groups arm earlier (`begin_fields`, which precedes their
        // name events — this event follows them there): re-arming here
        // would wipe the already-queued const names, so only a local
        // group (this event precedes its names) arms.
        if var_final_or_const.and_then(|t| engine.text_of(t)) == Some("const")
            && !engine.const_group
        {
            engine.arm_const_group();
        }
    }

    fn end_variables_declaration(&mut self, count: usize, end: Option<Token>) {
        self.flow().end_variables_declaration(count, end);

        let engine = &mut self.engine;
        engine.pending_decl_ty = None;
        engine.pending_decl_ty_unresolved = false;
        engine.armed_ty = None;
        engine.armed_ty_unresolved = false;
        engine.disarm_const_group();
        // The variable group closed: an unclaimed signature stash (a
        // function-type cast inside an initializer) dies here.
        engine.fn_stash_drop();
        // #1314 FAM18: the group closed — the constructor-independent faces
        // (top-level variables, static fields) decide here; instance-field
        // entries stay latched for the container's end (its constructor set
        // is only then complete).
        engine.flush_init_variables();
    }

    fn begin_variable_initializer(&mut self, token: Token) {
        self.flow().begin_variable_initializer(token);

        let engine = &mut self.engine;
        // #1314 FAM18: the initializer belongs to the most recently latched
        // variable (declaration order).
        if let Some(check) = engine.pending_init_variables.last_mut() {
            check.initialized = true;
        }
        // The initializer world arms for every variable initializer — a
        // top-level one included: the faces that arm on the bit carry
        // their own closure exclusion (`init_frame_depth == frames.len()`,
        // so a body nested in the initializer never arms), and the
        // implicit-this face arms only inside a member field group
        // (`begin_fields` never opens the group for a top-level
        // declaration), whose closures have no `this` either — the file
        // scope stays silent (`EvParser(null).build()` / the xml accessor
        // closure — probe2281).
        engine.in_initializer = true;
        engine.init_frame_depth = engine.frames.len();
        engine.init_mark = Some(engine.rows.len());
        // This initializer's own declarator (latched at its name): parked
        // on the window stack so nested declarations inside the
        // initializer cannot re-latch it.
        let declarator = engine.var_infer_pending.take();
        engine.init_declarators.push(declarator);
        engine.open_const_initializer();
    }

    fn end_variable_initializer(&mut self, token: Token) {
        // A `var`-inferred declarator's static type binds from the
        // initializer's resolved expression type — the single-row window
        // (`var self = this` leaves the `this` receiver row) answers it;
        // anything wider or unresolved keeps the `Unknown` slot every
        // verdict reads as suppress. The bind and its flow staging
        // precede the flow forward: a top-level/field initializer folds
        // as its own executable inside that call, and the staged declared
        // type must stand before the replay (#2140).
        let staged = {
            let engine = &mut self.engine;
            engine.ur_seal_used();
            engine.in_initializer = false;
            let mut staged = None;
            // A declarator latched inside this initializer and never
            // consumed (a for-element's loop variable) is dropped with the
            // window: it belongs to no initializer.
            engine.var_infer_pending = None;
            if let Some((name, decl_range)) = engine.init_declarators.pop().flatten()
                && let Some(mark) = engine.init_mark
                && let Some((expr, ifnull_ty)) = {
                    // The `??` pair's own face folds here (this slice): the
                    // two-operand window types the local as
                    // `LUB(NonNull(lhs), rhs)` before the pass-2 flow face
                    // — the flow layer widened the write conservatively (it
                    // cannot resolve the right operand's member face), and
                    // without this bind the promoted-receiver re-derive
                    // fills the widened nullable face below (`final v =
                    // target ?? h.cols; v.isEmpty` reported unchecked where
                    // the oracle stays silent). The `expr` index the tail
                    // reads is the pair's left row (await extents, the
                    // #2539 latch and the explicit-argument rider all key
                    // on it or miss).
                    let ifnull_ty = engine.ifnull_initializer_ty(mark);
                    if ifnull_ty.is_some() {
                        Some((mark, ifnull_ty))
                    } else {
                        match engine.rows.len() {
                        // The single-row window every fold-as-one shape keeps.
                        count if count == mark + 1 => Some((mark, None)),
                        // The `[]` fold leaves the receiver's operator row plus
                        // the result's value row (#2521): the initializer's
                        // expression row is the value row — one past the
                        // operator — so `var symbols = map[key]!` infers the
                        // index result's type exactly like a plain read. Only
                        // the assertion-decided NON-NULL face binds: a nullable
                        // index result (`var h = headers[k]`, `var r =
                        // byIndex[i]`) owes the null-check promotion and the
                        // reassignment writes the staged faces re-decide
                        // downstream, and binding it here fires the return/
                        // unchecked/undefined verdicts before those re-decides
                        // run (the analyzer corpus's map-inferred locals) —
                        // the under-report stands until the write-side staging
                        // carries them. The `!`-asserted face is the one shape
                        // with no such question standing: the assertion itself
                        // decided nullness, the type is the index result's own,
                        // and no promotion re-decide can change it.
                        count
                            if count == mark + 2
                                && matches!(
                                    engine.rows[mark].intent,
                                    Intent::Operator(op, _) if op == engine.intern("[]")
                                )
                                && matches!(engine.rows[mark + 1].shape, RowShape::Value)
                                && engine.rows[mark + 1].ty != Ty::UNKNOWN
                                && !Engine::is_nullable_ty(engine.rows[mark + 1].ty)
                                && engine.byte_at(engine.rows[mark + 1].range.end())
                                    == Some(b'!') =>
                        {
                            Some((mark + 1, None))
                        }
                        _ => None,
                        }
                    }
                }
                && let Some(ty) = ifnull_ty.or_else(|| engine
                    .initializer_static_ty(&engine.rows[expr])
                    .map(|ty| engine.staged_read_ty(&engine.rows[expr]).unwrap_or(ty))
                    .or_else(|| {
                        // The #2481 face: a member read the window's own
                        // faces leave `Unknown` still owns a static type —
                        // the accessor's declared return (instance getter)
                        // or the applied extension face's (the census's
                        // `var element = elementAnnotation?.element`
                        // shape), nullable re-armed for a `?.` merge. The
                        // bind and the flow staging below ride it, so an
                        // extension use through the inferred local
                        // resolves and credits.
                        let row = engine.rows[expr].clone();
                        let derived = engine.initializer_read_ty(&row);
                        (derived != Ty::UNKNOWN).then_some(derived)
                    }))
            {
                // The await operand's extent re-applies the flatten here
                // (#2506): the close-time rewrite reads an `Unknown` row
                // (member rows resolve in the later pass), so the resolved
                // `Future<T>`-shaped type flattens at the bind — the local
                // holds `T`, nullability preserved.
                let ty = if engine.await_operand_extents.iter().any(
                    |&(start, end)| {
                        engine.rows[expr].range.offset >= start
                            && engine.rows[expr].range.end() <= end
                    },
                ) {
                    engine.flatten(ty)
                } else {
                    ty
                };
                // An explicit-argument creation's written arguments ride
                // the head (`creation_args.rs`).
                let ty = engine.with_explicit_creation_args(expr, ty);
                engine.bind_var_type(name, ty);
                // A promotion-latched initializer defers its re-bind
                // (#2539): the member read's accessor return rides the
                // UNPROMOTED declared receiver at this bind, and the
                // promotion re-decision runs only in pass 2. The entry
                // keys the declaring local's NAME-TOKEN offset — the
                // scoped locals-stack index aliases across scopes once
                // the frames close, and the re-bind map must stay
                // per-declaration.
                if let Some(expr_row) = engine.rows.get(expr)
                    && engine
                        .promoted_recv_reads
                        .binary_search_by(|(member, _)| member.cmp(&expr_row.range.offset))
                        .is_ok()
                    && let Some(decl) = engine.locals.last().filter(|decl| decl.name == name)
                {
                    engine
                        .deferred_var_rebinds
                        .push((decl.range.offset, expr_row.range.offset, ty));
                }
                // The bare-identifier twin (#2567): `var x = p;` defers the
                // same re-bind — the initializer read's flow type (a
                // promotion standing at the read) re-binds the inferred
                // face in pass 2. A plain bare read only; every other
                // shape's face decided above.
                if let Some(expr_row) = engine.rows.get(expr)
                    && matches!(
                        expr_row.shape,
                        RowShape::Bare(BareInfo::Plain { .. })
                    )
                    && let Some(decl) = engine.locals.last().filter(|decl| decl.name == name)
                {
                    engine
                        .deferred_bare_rebinds
                        .push((decl.range.offset, expr_row.range, ty));
                }
                // The promotion face rides the flow layer's declared-type
                // slot (#2140): a name-level type with no written
                // arguments stages as the flow declared type, keyed on
                // the declarator's name token — the fold adopts it at the
                // initializer's end, so `case null`/`!= null` promotion
                // and every read's flow type answer on inferred locals
                // exactly like annotated ones. Argument-bearing and
                // non-named shapes stay untyped downstream (the flow
                // lattice cannot spell them; the unknown-declared
                // under-report stands).
                if let Ty::Named {
                    name: ty_name,
                    nullable,
                    args,
                } = ty
                {
                    let lexeme = engine.strings.resolve(ty_name).to_string();
                    // #2506: the written-argument tree rides beside the
                    // head — the `ArgListId::EMPTY` gate dropped every
                    // argument-bearing inference, and the Unknown read row
                    // downstream mis-fires the InvalidType-scrutinee
                    // posture. A tree the flow lattice cannot spell
                    // (`staged_arg_tree`'s `None`) stages the bare head —
                    // the pre-#2506 under-report, never a wrong tree.
                    let tree = staged_arg_tree(
                        &engine.boundary.args.borrow(),
                        &engine.strings,
                        args,
                    )
                    .unwrap_or_default();
                    staged = Some((decl_range.offset, lexeme, nullable, tree));
                }
            }
            staged
        };
        if let Some((offset, lexeme, nullable, tree)) = staged {
            let base = self.flow().intern_name(&lexeme);
            self.flow().stage_inferred_local(
                offset,
                FlowTy {
                    base: FlowBase::Name(base),
                    nullable,
                },
                tree,
            );
        }
        self.flow().end_variable_initializer(token);

        let engine = &mut self.engine;
        // The const close face: a member field group's own initializers
        // (began at the group's frame depth) and file-scope initializers
        // close on the field face — the value joins `const_field_values` /
        // the file scope, and no local var-type binding is written. A
        // function frame opened inside the initializer, or a
        // closure-local's own window (deeper than the group), closes on
        // the local face regardless (#511's misroute guard).
        let in_field_group =
            engine.field_group_open && engine.init_frame_depth == engine.field_group_depth;
        let at_file_scope = !engine.field_group_open && engine.init_frame_depth == 0;
        let no_function_opened = engine.frames.len() == engine.init_frame_depth;
        if (in_field_group || at_file_scope) && no_function_opened {
            engine.close_const_field_initializer();
        } else {
            engine.close_const_initializer();
        }
        engine.check_decl_initializer();
    }

    fn begin_type_variable(&mut self, token: Token) {
        // #2358: flow keys each `boundaries` row on the variable's
        // interned name — the row's registration must see every open
        // variable (flow keeps its own stack; the engine's is separate).
        self.flow().begin_type_variable(token);
        let engine = &mut self.engine;
        // The name token anchors this variable's unbounded declaration: an
        // unbounded variable's `handle_no_type` fires against it.
        if let Some(window) = engine.tv_window.as_mut() {
            window.vars.push(token);
        }
        if let Some(name) = engine.name_of(token) {
            // A window inside a written function-type capture scopes its
            // variables to the capture: they never park for a later
            // frame (the official scopes them to the function type, so
            // the initializer's reads stay undefined).
            if engine.fn_type_capture.last_mut().is_some() {
                engine.fn_capture_type_variable(name);
            } else {
                engine.push_type_parameter(name);
            }
            // The #612 capture pairs the parameter's name here: the Simple1
            // fast path completes the variable at `end_type_variable` with
            // the group's closer token, whose text is not the name.
            engine.var_name_stack.push((name, token.range));
        }
    }

    fn begin_type_variables(&mut self, token: Token) {
        // #2358: forwarded for pairing with flow's `end_type_variables`
        // cleanup (its begin arm is a no-op today; the open half keeps the
        // capture state machine's halves symmetric).
        self.flow().begin_type_variables(token);
        let engine = &mut self.engine;
        // The open signature capture's own `<T, S extends num>` pass: the
        // variables' bound completions belong to the variables (each is
        // consumed at its `end_type_variable`), never to the return
        // region.
        if let Some(capture) = engine.fn_type_capture.last_mut() {
            capture.in_variables = true;
        }
        // A new header started: a previous declaration's parked variables
        // never met their frame (a typedef's, say) — drop them. The header's
        // own row span opens with it (its bounds' rows justify against the
        // parked parameters at frame open).
        engine.pending_rows_mark = engine.rows.len();
        engine.pending_type_params.clear();
        // The header's bound counts open with it.
        engine.open_type_variables();
        engine.bounds.begin_type_variables();
        // The #612 window resets with it (a nested function-type window
        // inside a header resets an already-adopted container's — the
        // offset guard at adoption rejects foreign windows).
        engine.var_reset_type_variables();
        // A window inside a function type carries its signature's own
        // bounds; the container windows close before their header opens.
        engine.var_open_bounds();
    }

    fn end_type_variable(
        &mut self,
        token: Token,
        index: usize,
        extends_or_super: Option<Token>,
        variance: Option<Token>,
    ) {
        // #2358: the `boundaries` registration itself — flow pairs this
        // variable's bound (`extends`-written or the implicit `Object?`)
        // into its table, the exhaustiveness verdict's bound source.
        // Fires before the engine's latched-name early return: flow's own
        // name stack must stay balanced even when the engine latched
        // nothing (recovery), and its end arm tolerates that emptiness.
        self.flow()
            .end_type_variable(token, index, extends_or_super, variance);
        let engine = &mut self.engine;
        // name token, the Simple1 fast path (`<T>`) fires the group's
        // closer (the official event shape) — the name rides the stack
        // pushed at `begin_type_variable`, so the closer's text never keys
        // the window.
        let name = engine
            .var_name_stack
            .last()
            .map(|(name, _)| *name)
            .or_else(|| engine.name_of(token));
        let Some(name) = name else {
            return;
        };
        engine
            .bounds
            .end_type_variable(name, extends_or_super.is_some());
        // The written-signature capture binds the just-armed bound here
        // (an unbounded variable's `handle_no_type` disarmed it first).
        if !engine.fn_type_capture.is_empty() {
            let bound = engine.armed_ty.take();
            let capture = engine
                .fn_type_capture
                .last_mut()
                .expect("capture stack checked non-empty");
            capture.params.push((name, bound));
        }
        // The name rides the stack (the Simple1 end event carries the
        // group closer); the keyword and the anchor come from this event.
        if let Some((var_name, var_range)) = engine.var_name_stack.pop() {
            engine.var_type_variable(var_name, variance, var_range);
        }
    }

    fn end_type_variables(&mut self, begin: Token, end: Token) {
        // #2358: flow drops its capture stacks with the group (a missed
        // cleanup would leak names into the next declaration's rows —
        // the committed boundaries themselves survive).
        self.flow().end_type_variables(begin, end);
        let engine = &mut self.engine;
        engine.close_type_variables(end);
        engine.bounds.end_type_variables();
        engine.var_close_bounds();
        // The signature capture's variables pass closed: later completions
        // are the return region's.
        if let Some(capture) = engine.fn_type_capture.last_mut() {
            capture.in_variables = false;
        }
        // #1314 FAM10: the header's type-variable window closed — pair it
        // with the latched declaring name (token order: a name precedes
        // its own window; a foreign window's close sits after a later
        // declaration's name and drops).
        engine.pair_exec_window(end.range.offset);
    }

    // —— identifiers ——————————————————————————————————————————————————

    #[expect(clippy::too_many_lines)] // long diagnostic-table dispatch
    fn handle_identifier(&mut self, token: Token, context: IdentifierContext) {
        use IdentifierContext as C;
        self.flow().handle_identifier(token, context);

        let engine = &mut self.engine;
        let name = engine.name_of(token);
        match context {
            C::Expression => {
                if let Some(name) = name {
                    engine.push_bare(name, token.range);
                    engine.const_capture.identifier(name, token.range);
                }
            }
            C::ExpressionContinuation => {
                if let Some(name) = name {
                    // The #748 selector name rides this event (the dot's
                    // token is the send's end).
                    engine.nsm_member_name(token);
                    engine.push_member(name, token.range);
                    engine.const_capture.selector_identifier(name, token.range);
                }
            }
            C::TypeReference | C::TypeReferenceContinuation => {
                if let Some(name) = name {
                    // The catch clause's `on` type: the first type reference
                    // after `begin_catch_clause` and before any parameter —
                    // a record-typed on-clause's field types are plain type
                    // positions (`on (e) { ... }` resolves `e` through the
                    // plain path, never the catch face).
                    if engine.record_type_depth == 0
                        && engine.catch_clause_start.is_some()
                        && engine.catch_params == 0
                        && engine.catch_on_type.is_none()
                    {
                        engine.catch_on_type = Some(name);
                        engine.catch_on_miss_rows.push(token.range.offset);
                    }
                    let head = if context == C::TypeReference {
                        TypeHead::Bare
                    } else {
                        TypeHead::Continuation
                    };
                    engine.push_type_use(name, token.range, head);
                    // The head awaits its type-argument group (or its
                    // completing `handle_type`) — unless a DEAD prefix
                    // owns the continuation: the NamedType's element is
                    // null upstream (the prefix bail returned
                    // `InvalidType`), so the continuation's group has no
                    // head to count against and the annotation-arity face
                    // must not arm. A starving miss would let a
                    // resolution-elsewhere homonym own the group — the
                    // analyzer corpus's
                    // `typedef GeneratedTypeConstraint =
                    // shared.GeneratedTypeConstraint<…>` counted its
                    // argument against the typedef being declared
                    // (`wrong_number_of_type_arguments` beside the
                    // prefix's `undefined_identifier`; the oracle counts
                    // against nothing).
                    let dead_prefix = matches!(head, TypeHead::Continuation)
                        && engine.type_prefix.as_ref().is_some_and(|tp| !tp.live);
                    if !dead_prefix {
                        engine.arm_type_head(name, token.range);
                    }
                    // The group-less prefixed shape's leaf (#2523): the
                    // completion's armed type reads this name, never the
                    // prefix token the closing `handle_type` fires at.
                    if matches!(head, TypeHead::Continuation)
                        && let Some((_, leaf)) = engine.pending_prefixed.as_mut()
                        && leaf.is_none()
                    {
                        *leaf = Some((name, token.range));
                    }
                }
            }
            C::PrefixedTypeReference => {
                if let Some(name) = name {
                    engine.arm_type_prefix(name, token.range);
                    engine.pending_prefixed = Some((token.range, None));
                }
            }
            // A constructor reference names a class: the guarded miss is the
            // oracle's `creation_with_non_type` head face (`new Missing()`),
            // never an invocation.
            C::ConstructorReference => {
                if let Some(name) = name {
                    engine.push_type_use(name, token.range, TypeHead::Bare);
                    engine.creation_ref = Some((name, None));
                    // The const-creation window's head: a type head never
                    // joins the capture's operand stack, so the
                    // `const_with_non_const` target face latches it here
                    // (the window opened at the const keyword).
                    engine.const_capture.note_ctor_head(name);
                }
            }
            C::ConstructorReferenceContinuation => {
                if let Some(name) = name
                    && let Some(r) = engine.creation_ref.as_mut()
                {
                    r.1 = Some((name, token.range));
                }
            }
            C::LocalVariableDeclaration => {
                if let Some(name) = name {
                    engine.note_local(name, token.range);
                    engine.const_decl_name(name, false);
                    // The for-in head's declarator latches its name token
                    // (#2506) — `handle_for_in_loop_parts` stages the
                    // element type on it. Latched for both forms (the
                    // statement head's `in_for_parts` is 1, the collection
                    // element's 2); every loop head resets it.
                    if !engine.in_for_parts.is_empty() {
                        engine.for_in_loop_var = Some((name, token.range));
                    }
                    // A `var`/type-inferred declarator parks its name and
                    // name token: the initializer's resolved expression
                    // type binds as the declared type at
                    // `end_variable_initializer` (the `var self = this`
                    // face — the inferred `this` type is the operand's
                    // static type for `self is T` verdicts) and stages as
                    // the flow declared type (#2140).
                    engine.var_infer_pending = if engine.pending_decl_ty.is_none() {
                        Some((name, token.range))
                    } else {
                        None
                    };
                    engine.bind_declared_type(name);
                }
            }
            C::FormalParameterDeclaration => {
                if let Some(name) = name {
                    // The #748 `==` first-formal read runs before the
                    // binding consumes the armed annotation.
                    engine.equals_formal_type();
                    // #1314 FAM7: the formal's written type is still armed
                    // here — `bind_parameter_type` consumes it (the nested
                    // function-typed lists park and restore the snapshot).
                    engine.formal_written = engine.armed_ty;
                    // The open literal's signature capture records the
                    // formal's written type before the bind consumes it.
                    engine.closure_formal();
                    // The written-signature capture binds the formal.
                    engine.fn_capture_formal(name);
                    // The written-signature claim for the formal: a generic
                    // function-type annotation completed since the last
                    // declaration boundary is this formal's written shape.
                    // It joins the declaring executable's table for the
                    // argument check's function-subtype face — the element
                    // model carries `Unknown` for the annotation (rule 0c),
                    // so the table is the only structural record.
                    if let Some(sig) = engine.fn_type_stash.take() {
                        // The callable face: a function-typed parameter is
                        // invocable like a function-typed variable — the
                        // element model poisons the annotation into a
                        // `Named` (rule 0c), so the written signature is
                        // the only callable record.
                        engine.fn_var_sigs.insert(name, sig.clone());
                        if let Some(exec) = engine.frames.last().and_then(|f| match &f.kind {
                            FrameKind::Executable { ret: Some(ret), .. } => Some(ret.name),
                            _ => None,
                        }) {
                            let table = engine.fn_formal_sigs.entry(exec).or_default();
                            if engine.fn_formal_named_section {
                                table.named.insert(name, sig);
                            } else {
                                table.positional.push(sig);
                            }
                        }
                    }
                    engine.note_parameter(name, token.range);
                    engine.bind_parameter_type(name);
                }
            }
            C::CatchParameter => {
                if let Some(name) = name {
                    engine.note_catch_parameter(name, token.range);
                    engine.bind_parameter_type(name);
                }
            }
            C::LocalFunctionDeclaration => {
                if let Some(name) = name {
                    engine.note_local_function(name, token.range);
                    // A local function's name is a boundary: a
                    // function-typed RETURN annotation's stash is not a
                    // variable's (the same face the top-level arm owns — a
                    // stale `int Function()` stash must never reach a later
                    // group past this declaration).
                    engine.fn_stash_drop();
                    // The return host latches here like every executable's:
                    // the written annotation arms before the name (the
                    // frame's own return matrix decides), and the nameless
                    // recovery shape — or a head whose written type the
                    // recovery consumed elsewhere (`issue_49477_prime`'s
                    // dangling member access) — latches `unknown`, whose
                    // matrix stays silent (the oracle's dynamic return type
                    // for a local function written without one).
                    engine.latch_return_type(ExecKind::Function, name, token.range);
                }
            }
            C::LabelDeclaration => {
                if let Some(name) = name {
                    engine.pending_label = Some((name, token.range));
                }
            }
            C::LabelReference => {
                if let Some(name) = name {
                    engine.note_label_use(name);
                }
            }
            C::ImportPrefixDeclaration => {
                if let Some(name) = name
                    && let Some(site) = engine.imports.last_mut()
                {
                    site.prefix = Some(name);
                }
            }
            C::NamedArgumentReference => {
                if let Some(name) = name {
                    engine.check_named_argument(name, token.range);
                    engine.open_argument(Some(name), token.range);
                }
            }
            C::EnumValueDeclaration => {
                // The enum constant's window opens here: its type arguments
                // check against the enclosing enum's declared count (the
                // `_enum` code), and its constructor arguments capture as
                // a const context (`handle_enum_element` closes it). The
                // constant is a static field site — the `unused_field`
                // read rule owns it.
                engine.enter_enum_element();
                engine.open_enum_const_element();
                // The constant's implicit constructor invocation: the
                // reference anchors at the constant's own name (the
                // `.named` selector — when written — re-anchors it). Its
                // argument list checks against the selected constructor's
                // formals at `close_arguments`.
                engine.enum_const_call = Some((token.range, None));
                if let Some(name) = name
                    && let Some((container, _)) = engine.container_context()
                {
                    engine.note_decl_site(
                        name,
                        token.range,
                        DeclKind::Member {
                            kind: MemberKind::Field,
                            is_static: true,
                            container,
                        },
                    );
                    // The unnamed-constructor candidate (`visitEnumConstant
                    // Declaration`'s initializer face): the constant's
                    // implicit invocation needs the unnamed constructor,
                    // and a member can still kill the face after this
                    // point — the verdict joins at the enum's close.
                    engine
                        .pending_unnamed_enum_consts
                        .push((container, name, token.range));
                }
            }
            C::ConstructorReferenceContinuationAfterTypeArguments
                if engine.in_enum_element.is_some() && engine.ctor_pos == 0 =>
            {
                // The constant wrote a constructor selector — the unnamed
                // face no longer rides this constant (a synthetic selector
                // cancels too: the recovery face is the selector's own).
                engine.pending_unnamed_enum_consts.pop();
                // The enum constant's constructor selector (`c.other()`):
                // outside argument expressions (ctor_pos 0), the name must
                // be a constructor the enclosing enum declares
                // (`undefined_enum_constructor`). A parser-inserted
                // synthetic selector keeps the check under the empty name —
                // the oracle resolves the same recovery face.
                let name = name.or_else(|| engine.synthetic_name(token));
                if let Some(name) = name {
                    engine.check_enum_constructor_selector(name, token.range);
                    // The constant's implicit invocation re-anchors at the
                    // selector: its argument checks key `ctor_formals` on
                    // the selector's name and report at its span.
                    if let Some((_, selector)) = &mut engine.enum_const_call {
                        *selector = Some((name, token.range));
                    }
                }
            }
            C::ConstructorReferenceContinuationAfterTypeArguments => {
                // The parameterized creation's selector (`C<int>._x(...)`):
                // the written type-argument group splits the reference, so
                // this event — not `ConstructorReferenceContinuation` —
                // carries the selector, and no member row ever names it.
                // Credit the constructor element against the latched head
                // (the same gate as the end-of-reference credit: the head
                // must resolve to a class declaring the selector; a prefix
                // head resolves to nothing — under-report).
                if let Some(name) = name
                    && let Some((head, _)) = engine.creation_ref
                {
                    engine.credit_constructor_selector(head, name);
                }
                // The `.new` selector of the parameterized creation
                // (`prefix.C<int>.new()`): the head row already reported
                // (the requalified prefix face), and the oracle's property
                // lookup answers `needsGetterError` unconditionally for the
                // name `new` on the InvalidType the dead head left
                // (type_property_resolver.dart:78-83) — the method-shaped
                // invocation reports `undefined_method`. Push the selector
                // as a member row on the unknown receiver — the context
                // only fires with the `(` group following, so the invoke
                // intent rides the push — and pass 2 emits in source
                // order. A live head keeps the
                // constructor-selector question (outside the family's
                // emitters — under-report).
                if let Some(name) = name
                    && engine.implicit_creation_depth > 0
                    && let Some((head, _)) = engine.creation_ref
                    && matches!(engine.resolve_bare(head).0, BareInfo::Missing { .. })
                {
                    engine.push_row(Row {
                        name,
                        range: token.range,
                        qualified: true,
                        shape: RowShape::Member {
                            recv: Receiver::Unknown,
                        },
                        intent: Intent::Invoke,
                        write_op: None,
                        ty: Ty::UNKNOWN,
                    });
                }
            }
            C::MetadataReference => {
                if let Some(name) = name {
                    // The annotated-declaration exemption latch (the
                    // pragma/JS entry-point gates' superset) plus the
                    // annotation-arity check.
                    engine
                        .pending_annotations
                        .push(PendingAnnotation { name, unless: None });
                    engine.check_annotation(name, token.range);
                }
            }
            C::MetadataContinuation | C::MetadataContinuationAfterTypeArguments => {
                // The selector segment of a qualified annotation
                // (`@UseResult.unless`): joins the head segment in the
                // latch (`head.selector`), the shape
                // `use_result_unless` matches.
                if let Some(name) = name {
                    // An annotation's `@C._x` selector names the class's
                    // constructor (the oracle resolves it as the metadata
                    // creation's element); the metadata events carry no
                    // member row, so credit it here against the open
                    // head. The join below rewrites the head spelling, so
                    // the credit reads it first.
                    if let Some(head) = engine.pending_annotations.last().map(|open| open.name) {
                        engine.credit_constructor_selector(head, name);
                    }
                    let selector = engine.strings.resolve(name);
                    let head = engine
                        .pending_annotations
                        .last()
                        .map(|open| engine.strings.resolve(open.name));
                    if let Some(head) = head {
                        let joined = engine.strings.intern(&format!("{head}.{selector}"));
                        if let Some(open) = engine.pending_annotations.last_mut() {
                            open.name = joined;
                        }
                    }
                }
            }
            C::TopLevelFunctionDeclaration => {
                if let Some(name) = name {
                    // A function declaration's name is a boundary: a
                    // function-typed RETURN annotation's stash is not a
                    // variable's.
                    engine.fn_stash_drop();
                    engine.latch_return_type(ExecKind::Function, name, token.range);
                    let site = engine.note_decl_site(name, token.range, DeclKind::TopLevelFn);
                    engine.open_toplevel_site = Some(site);
                    engine.open_exec_site = Some(site);
                    // #1314 FAM10: the header's declaring name — its
                    // type-variable window pairs at the window's close.
                    engine.pending_exec_name = Some((name, token.range.offset));
                    engine.pending_exec_is_method = false;
                } else {
                    // The nameless recovery face (`name ?? '<unnamed>'`):
                    // the declaration joins under the empty name at the
                    // anchor chain; the exec windows stay closed.
                    let empty = engine.intern("");
                    let anchor = engine.nameless_site_range();
                    engine.note_decl_site(empty, anchor, DeclKind::TopLevelFn);
                }
            }
            C::MethodDeclaration => {
                // A factory head's name closes the factory latch: the
                // container is the factory's return type, so the `=>` body
                // runs the official return check (`factory new() => 0;`
                // keeps the check on the recovery head). Fires once — the
                // frame's slot is already taken on the rejected-head
                // re-fire (`new.named`).
                if engine.factory_ret_open {
                    engine.factory_ret_open = false;
                    engine.factory_head = Some((token.range, false));
                    if let Some((container, _)) = engine.container_context() {
                        engine.latch_factory_return(container, token.range);
                    }
                }
                if let Some(name) = name {
                    // A method declaration's name is a boundary: a
                    // function-typed RETURN annotation's stash is not a
                    // variable's.
                    engine.fn_stash_drop();
                    engine.latch_return_type(ExecKind::Method, name, token.range);
                    // #2430: the method header's declaring name joins the
                    // FAM10 pairing — its type-variable window lands in
                    // `method_exec_bounds` at the close, the bound-aware
                    // nullability/never-match gates' read of the
                    // enclosing method's own parameter bounds.
                    engine.pending_exec_name = Some((name, token.range.offset));
                    engine.pending_exec_is_method = true;
                    // A constructor head also rides this event;
                    // `begin_constructor` already opened the head's site
                    // and its provisional range is this same token, so the
                    // member twin is skipped — it would double-report the
                    // private unnamed face (`unused_element` at the class
                    // spelling). The ctor site takes over as the open
                    // executable site; factories ride a `None` provisional
                    // and keep their member capture.
                    let ctor_head = matches!(
                        engine.ctor_pending,
                        Some((_, Some(site))) if engine.decl_sites[site].range == token.range
                    );
                    if ctor_head {
                        if let Some((_, Some(site))) = engine.ctor_pending {
                            engine.open_exec_site = Some(site);
                        }
                    } else {
                        let is_setter = engine.open_member_setter;
                        let spelling = if is_setter {
                            engine.setter_spelling(name)
                        } else {
                            name
                        };
                        let is_static = engine.open_member_static;
                        let container = engine.container_context().map_or(name, |(c, _)| c);
                        let site = engine.note_decl_site(
                            spelling,
                            token.range,
                            DeclKind::Member {
                                kind: MemberKind::Method,
                                is_static,
                                container,
                            },
                        );
                        engine.open_exec_site = Some(site);
                    }
                }
            }
            C::OperatorName => {
                if let Some(name) = name {
                    let container = engine.container_context().map_or(name, |(c, _)| c);
                    engine.note_decl_site(
                        name,
                        token.range,
                        DeclKind::Member {
                            kind: MemberKind::Method,
                            is_static: false,
                            container,
                        },
                    );
                }
            }
            C::MethodDeclarationContinuation => {
                // The named constructor selector (`A.named`): the site joins
                // only under an open constructor/factory (the same context
                // also fires for qualified method heads, which never declare
                // a constructor). A constructor rewrites its provisional
                // head site in place — one site per constructor, now named
                // by the selector (the spelling a `C.named()` creation
                // marks given under); a factory registers its own site here
                // (no provisional was opened).
                if let (Some(name), Some((container, provisional))) = (name, engine.ctor_pending) {
                    if let Some(site) = provisional {
                        let entry = &mut engine.decl_sites[site];
                        entry.name = name;
                        entry.range = token.range;
                        engine.open_exec_site = Some(site);
                    } else {
                        let site = engine.note_decl_site(
                            name,
                            token.range,
                            DeclKind::Constructor { container },
                        );
                        // The `factory` head opened this declaration (a
                        // constructor's provisional is always `Some`) — the
                        // site joins with the factory bit set (the
                        // redirect_generative face's target query).
                        engine.decl_sites[site].is_factory = true;
                        engine.open_exec_site = Some(site);
                        if let Some((head, named)) = &mut engine.factory_head {
                            *named = true;
                            engine.factory_head_anchors.insert(site, *head);
                        }
                    }
                }
            }
            C::TypedefDeclaration => {
                if let Some(name) = name {
                    // A typedef's name is a declaration boundary: a stale
                    // signature stash dies before the RHS parse.
                    engine.fn_stash_drop();
                    // The #1351 capture opens at the declaring name (the
                    // parameters' window and the RHS parse after it).
                    engine.bounds.begin_alias(name);
                    engine.note_decl_site(name, token.range, DeclKind::TypeAlias);
                } else {
                    // The nameless recovery face (see the top-level
                    // function arm); no capture opens.
                    let empty = engine.intern("");
                    let anchor = engine.nameless_site_range();
                    engine.note_decl_site(empty, anchor, DeclKind::TypeAlias);
                }
            }
            // Field groups (class fields, enum body fields, top-level
            // variables): const groups queue the names for their
            // initializer windows; non-const top-level variables join the
            // provable non-const set. Class fields also enter the
            // invalid_override check (their face is the declared type).
            C::FieldDeclaration | C::TopLevelVariableDeclaration => {
                if let Some(name) = name {
                    engine.const_decl_name(name, true);
                    // The written-signature claim: a function-type
                    // annotation completed since the last declaration
                    // boundary is this group's annotation; every name in
                    // the group binds it (the stash drops at the group's
                    // end event).
                    if let Some(sig) = engine.fn_type_stash.clone() {
                        engine.fn_var_sigs.insert(name, sig);
                    }
                    // #1314 FAM18: the group bits and the declared type gate
                    // the latch here; the initializer mark and the flush
                    // decide later. The name event precedes
                    // `begin_variables_declaration` (the parser folds the
                    // names before the group head fires), so the declared
                    // type still sits on the armed slot — `pending_decl_ty`
                    // is only armed afterwards.
                    let ty = engine
                        .pending_decl_ty
                        .or(engine.armed_ty)
                        .unwrap_or(Ty::UNKNOWN);
                    let ty_unresolved = if engine.pending_decl_ty.is_some() {
                        engine.pending_decl_ty_unresolved
                    } else {
                        engine.armed_ty_unresolved
                    };
                    let final_gate = engine.group_final && !engine.group_late;
                    let non_nullable_gate = !engine.group_const
                        && !engine.group_final
                        && !engine.group_late
                        && !ty_unresolved
                        && engine.written_potentially_non_nullable(ty);
                    if final_gate || non_nullable_gate {
                        if context == C::FieldDeclaration {
                            let is_static = engine.open_member_static;
                            if let Some((container, _)) = engine.container_context() {
                                engine.pending_init_variables.push(PendingInitVariable {
                                    name,
                                    range: token.range,
                                    container: Some(container),
                                    is_static,
                                    initialized: false,
                                    is_final: final_gate,
                                });
                            }
                        } else {
                            engine.pending_init_variables.push(PendingInitVariable {
                                name,
                                range: token.range,
                                container: None,
                                is_static: false,
                                initialized: false,
                                is_final: final_gate,
                            });
                        }
                    }
                    if context == C::FieldDeclaration {
                        engine.check_override(name, token.range);
                        // The `@override` presence face reads the pending
                        // annotation latch before the site drains it.
                        engine.check_override_annotation(name, token.range);
                        // `non_final_field_in_enum`: an instance field in an
                        // enum body must be `final` (messages.yaml:13492 —
                        // the anchor is the field's name token; static
                        // fields are exempt, the diagnostic's documentation
                        // scopes the face to instance fields).
                        if let Some((_, ContainerKind::Enum)) = engine.container_context()
                            && !engine.open_member_static
                            && !engine.group_final
                        {
                            let name_text = engine.strings.resolve(name);
                            engine.emit(
                                SemanticCode::NonFinalFieldInEnum,
                                token.range,
                                name_text,
                                "",
                            );
                        }
                        if let Some((container, _)) = engine.container_context() {
                            let is_static = engine.open_member_static;
                            engine.note_decl_site(
                                name,
                                token.range,
                                DeclKind::Member {
                                    kind: MemberKind::Field,
                                    is_static,
                                    container,
                                },
                            );
                        }
                    } else {
                        engine.note_decl_site(name, token.range, DeclKind::TopLevelVar);
                    }
                } else if context == C::TopLevelVariableDeclaration {
                    // The nameless recovery face (see the top-level
                    // function arm); a nameless field keeps the
                    // suppression direction.
                    let empty = engine.intern("");
                    let anchor = engine.nameless_site_range();
                    engine.note_decl_site(empty, anchor, DeclKind::TopLevelVar);
                }
            }
            // A `show`-list name joins the current import's shown set with
            // its token span (`generateUnusedShownNameHints`'s walk); a
            // `hide` name has no hint of its own. An `export`'s show names
            // belong to no import site — they join the use-face's
            // re-export record.
            C::Combinator => {
                if let Some(name) = name
                    && engine.in_show
                {
                    if engine.in_import
                        && let Some(site) = engine.imports.last_mut()
                    {
                        site.shown.push((name, token.range));
                    } else if engine.in_export
                        && let Some(export) = engine.exports.last_mut()
                    {
                        export.shown.push(name);
                    }
                }
            }
            // A record field's written name arrives after its type — the
            // name pairs with the field slot the entry just closed (the
            // named region's trailing entries read them at
            // `end_record_type`).
            C::RecordFieldDeclaration => {
                if let Some(level) = engine.record_ty_stack.last_mut() {
                    level.names.resize(level.fields.len() + 1, None);
                    if let Some(slot) = level.names.last_mut() {
                        *slot = name;
                    }
                }
            }
            _ => {}
        }
    }

    fn handle_qualified(&mut self, _period: Token) {
        self.engine.consume_type_prefix();
    }

    // —— declared types ——————————————————————————————————————————————

    fn handle_type(&mut self, name: Token, question: Option<Token>) {
        self.flow().handle_type(name, question);

        let engine = &mut self.engine;
        // The type completed without a group; its head slot is consumed.
        // A `Some` here is a group-less simple type head (`Box`, `int`) —
        // parameterized heads moved into `group_heads` at `<` and prefixed
        // heads were consumed at `handle_no_type_arguments` — which is
        // exactly the raw-type candidate set.
        let head = engine.pending_type_head.take();
        let Some(text) = engine.text_of(name) else {
            return;
        };
        // The group-less prefixed completion (`p.C`): the official
        // `PrefixedType.parseType` fires `handleType` at the PREFIX's
        // token, so the lexeme above names the prefix — the armed type,
        // the bound declared types, and every downstream face would read
        // a container named after the import prefix (#2523). The leaf
        // segment's name is the type (`p.C` keeps `C`); a completion
        // whose token is not the latched prefix segment consumes the
        // stale stash and completes normally.
        let prefixed_leaf = engine
            .pending_prefixed
            .take()
            .filter(|(prefix_range, _)| *prefix_range == name.range)
            .and_then(|(_, leaf)| leaf);
        let (text, leaf_id, head_id) = match prefixed_leaf {
            // The armed head spells the DOT-JOINED form (`io.File`), the
            // intern table's one-head convention the collector's own
            // prefixed annotations use — a bare leaf would resolve a
            // same-spelled unprefixed import's class (analyzer's own
            // `FileSystemException` behind `io.FileSystemException`), and
            // every container walk on the joined spelling goes through
            // the prefix-aware lookup or misses. The bare leaf still
            // feeds the faces keyed by the written segment (the bounds
            // capture, the header spans).
            Some((leaf_id, _)) => {
                let joined = format!(
                    "{}.{}",
                    text,
                    engine.strings.resolve(leaf_id)
                );
                let head_id = engine.strings.intern(&joined);
                (joined, leaf_id, head_id)
            }
            None => {
                let head_id = engine.strings.intern(text);
                (text.to_string(), head_id, head_id)
            }
        };
        // The #610 capture: every completed type head is a leaf of the open
        // window (the parser re-fires a parameterized head after its group,
        // carrying the trailing `?` — the sink merges that completion).
        engine.bounds.leaf(
            leaf_id,
            question.is_some(),
            match question {
                Some(q) => SourceRange {
                    offset: name.range.offset,
                    length: q.range.end() - name.range.offset,
                },
                None => name.range,
            },
        );
        // The #1351 capture: the same leaf joins the open typedef RHS.
        engine.bounds.alias_leaf(
            leaf_id,
            question.is_some(),
            match question {
                Some(q) => SourceRange {
                    offset: name.range.offset,
                    length: q.range.end() - name.range.offset,
                },
                None => name.range,
            },
        );
        // The #612 capture: the same leaf joins the open header (the
        // parameterized re-fire merges through the sink's head latch).
        engine.var_leaf(leaf_id, name.range.offset, question.is_some());
        // The deferred modifier checks' clause-entry capture (#1289 E1): a
        // depth-0 completion inside the open header is one written entry.
        engine.note_header_type_span(name, question);
        // `dynamic` is a top type and `Null` is the null-type singleton —
        // both have dedicated [`Ty`] shapes.
        let ty = match text.as_str() {
            "dynamic" => Ty::Dynamic,
            "Null" => Ty::Null,
            _ => Ty::Named {
                name: head_id,
                nullable: question.is_some(),
                args: ArgListId::EMPTY,
            },
        };
        // The catch clause's on-type completion (the re-fired
        // parameterized head carries the trailing `?` — it overwrites, so
        // the last completion of the clause's head wins).
        if engine.catch_clause_start.is_some()
            && engine.catch_params == 0
            && engine.catch_on_type == Some(leaf_id)
        {
            engine.catch_on_nullable = Some(question.is_some());
            // `nullable_type_in_catch_clause`
            // (`_checkForNullableTypeInCatchClause`,
            // best_practices_verifier.dart:1377-1391): the on-type is
            // potentially nullable — `T?`, or `dynamic`
            // (`isPotentiallyNullable` = `!isNonNullable`,
            // type_system.dart:1260). An unresolved head reports in the
            // oracle neither (the `InvalidType` guard): the `Missing`
            // type-use row at the head's own offset suppresses.
            if question.is_some() || text == "dynamic" {
                let unresolved = matches!(
                    engine.rows.last().map(|r| &r.shape),
                    Some(RowShape::TypeUse(TypeVerdict::Missing, _))
                ) && engine
                    .rows
                    .last()
                    .is_some_and(|r| r.range.offset == name.range.offset);
                if !unresolved {
                    let span = match question {
                        Some(q) => SourceRange {
                            offset: name.range.offset,
                            length: q.range.end() - name.range.offset,
                        },
                        None => name.range,
                    };
                    engine.stage_usage(SemanticCode::NullableTypeInCatchClause, span, "");
                }
            }
        }
        // `unnecessary_question_mark` (`visitNamedType`,
        // best_practices_verifier.dart:762-779): `Null?` and `dynamic?` —
        // the non-aliased names whose nullability the `?` adds nothing to
        // (`dynamic` checks the lexeme, so a user declaration named
        // `dynamic` is impossible) — anchor at the `?` token.
        if let Some(q) = question
            && (text == "Null" || text == "dynamic")
        {
            let type_name = engine.strings.resolve(leaf_id).to_string();
            engine.stage_usage(SemanticCode::UnnecessaryQuestionMark, q.range, &type_name);
        }
        // The head's resolution rides the armed slot: a `TypeUse Missing`
        // row at the head's own offset means the oracle's element type is
        // `InvalidType` — the uninitialized latch consults this (the FAM18
        // capture) beside the catch clause's own guard.
        engine.armed_ty_unresolved = matches!(
            engine.rows.last().map(|r| &r.shape),
            Some(RowShape::TypeUse(TypeVerdict::Missing, _))
        ) && engine
            .rows
            .last()
            .is_some_and(|r| r.range.offset == name.range.offset);
        engine.armed_ty = Some(ty);
        // The written extent rides the armed slot (the pattern faces'
        // `pattern.type` anchor — the same span the return latch takes).
        let span = match question {
            Some(q) => SourceRange {
                offset: name.range.offset,
                length: q.range.end() - name.range.offset,
            },
            None => name.range,
        };
        engine.armed_ty_span = Some(span);
        // A record-type field's own type (F-R): the completed head is the
        // open level's pending field — `end_record_type_entry` binds it.
        if let Some(level) = engine.record_ty_stack.last_mut() {
            level.pending = Some(ty);
        }
        // The written return-type span of the declaration being headed
        // (the `non_void_return_for_setter` face reads it at the
        // declaration's end).
        engine.arm_return_span(span);
        // The #748 `==` read: an annotation completed inside the method's
        // own parameter list is the first formal's written type candidate.
        engine.equals_mark_type();
        // The written-argument latch: a group whose head this completion
        // re-fires arms its arguments for the binding declaration; any
        // other completion (a group-less head, a headless group's leaf)
        // resets the latch, so a stale annotation never reaches a bind.
        match engine.completed_group.take() {
            Some((group_head, group_offset, args))
                if group_head == leaf_id && group_offset == name.range.offset =>
            {
                // The written arguments join the armed head (#611): the
                // list interns into the boundary pool and the just-armed
                // name-only head carries the canonical slot, so every
                // downstream face (return latches, parameter bindings,
                // cast targets) compares the written shape instead of a
                // bare name.
                let slot = engine.boundary.intern_list(&args);
                if let Some(Ty::Named { args: armed, .. }) = &mut engine.armed_ty {
                    *armed = slot;
                }
            }
            other => {
                engine.completed_group = other;
            }
        }
        if let Some((head_name, head_range)) = head {
            engine.check_raw_type(head_name, head_range);
        }
        // The written-signature and call-argument captures take the final
        // completion (the re-fired parameterized head carries its written
        // arguments on the armed type by now).
        if let Some(ty) = engine.armed_ty {
            let leaf = leaf_id;
            engine.fn_capture_type(ty, leaf, name.range.offset);
            engine.call_args_feed(ty, leaf, name.range.offset);
            // #1758: the re-walked return annotation of an in-flight
            // old-style function-typed formal is the innermost unfilled
            // slot AT THIS type-argument depth (the parser re-parses the
            // return type before the nested formals' own types,
            // formal_params.rs:706-719): `List<int>`s arg completes a
            // level deeper than the slot it would otherwise steal.
            let depth = engine.group_heads.len();
            if let Some((_, slot)) = engine
                .fn_formal_ret_pending
                .iter_mut()
                .rev()
                .find(|(d, slot)| slot.is_none() && *d == depth)
            {
                *slot = Some(ty);
            }
        }
    }

    fn handle_no_type(&mut self, last_consumed: Token) {
        self.flow().handle_no_type(last_consumed);

        let engine = &mut self.engine;
        // An unbounded type variable declares no bound (`class C<T>`); the
        // parser anchors the event at the variable's own name token. A
        // generic-function-type bound's absent return type (`T extends
        // Function()`) anchors elsewhere — that variable is bounded, not
        // unbounded.
        if let Some(window) = engine.tv_window.as_mut()
            && window.vars.contains(&last_consumed)
        {
            window.unbounded += 1;
        }
        // A return-type position that completed without a written type: the
        // setter face's span dies with it (`set setter(value)` reports
        // nothing).
        if engine
            .frames
            .last()
            .is_some_and(|frame| matches!(&frame.kind, FrameKind::Executable { ret: None, .. }))
        {
            engine.open_return_span = None;
        }
        engine.armed_ty = None;
        engine.armed_ty_unresolved = false;
        engine.armed_ty_span = None;
        engine.pending_prefixed = None;
    }

    fn handle_void_keyword(&mut self, token: Token) {
        self.flow().handle_void_keyword(token);

        let engine = &mut self.engine;
        // A `void` type argument (`List<void>`) is a leaf of the open #610
        // capture like any named head.
        let void_id = engine.strings.intern("void");
        engine.bounds.leaf(void_id, false, token.range);
        // The #1351 capture: a `void` return or formal in the open
        // typedef RHS (`typedef F = void Function();`).
        engine.bounds.alias_leaf(void_id, false, token.range);
        // The #612 capture joins it (an unrelated contribution; the display
        // stays written-shape).
        engine.var_leaf(void_id, token.range.offset, false);
        engine.fn_capture_type(Ty::Void, void_id, token.range.offset);
        engine.call_args_feed(Ty::Void, void_id, token.range.offset);
        engine.armed_ty_unresolved = false;
        engine.armed_ty = Some(Ty::Void);
        engine.armed_ty_span = Some(token.range);
        // A `void` record field (`(void, int)`): the pending slot.
        if let Some(level) = engine.record_ty_stack.last_mut() {
            level.pending = Some(Ty::Void);
        }
        let depth = engine.group_heads.len();
        if let Some((_, slot)) = engine
            .fn_formal_ret_pending
            .iter_mut()
            .rev()
            .find(|(d, slot)| slot.is_none() && *d == depth)
        {
            *slot = Some(Ty::Void);
        }
        engine.arm_return_span(token.range);
    }

    fn handle_void_keyword_with_type_arguments(&mut self, token: Token) {
        let engine = &mut self.engine;
        engine.armed_ty_unresolved = false;
        engine.armed_ty = Some(Ty::Void);
        engine.armed_ty_span = Some(token.range);
        let depth = engine.group_heads.len();
        if let Some((_, slot)) = engine
            .fn_formal_ret_pending
            .iter_mut()
            .rev()
            .find(|(d, slot)| slot.is_none() && *d == depth)
        {
            *slot = Some(Ty::Void);
        }
        let void_id = engine.strings.intern("void");
        engine.fn_capture_type(Ty::Void, void_id, token.range.offset);
        engine.call_args_feed(Ty::Void, void_id, token.range.offset);
        engine.arm_return_span(token.range);
    }

    fn begin_initializer(&mut self, token: Token) {
        // The flow layer brackets the entry: its `f = e` assign writes the
        // field, never a like-named formal (`Ev::Assign::field_init`).
        self.flow().begin_initializer(token);
        // F-C: the item's head token — a `this`/`super` event at this offset
        // is the keyword head of a constructor-initializer construct, not a
        // `ThisExpression`/`SuperReference` (upstream `buildInitializer`'s
        // fold, ast_builder.dart:634-756).
        self.engine.init_item_head = Some(token.range.offset);
    }

    fn end_initializer(&mut self, end: Token) {
        self.flow().end_initializer(end);
        self.engine.init_item_head = None;
    }

    fn begin_initializers(&mut self, _token: Token) {
        let engine = &mut self.engine;
        engine.initializer_depth += 1;
        // #1351: a const constructor's initializer list opens its own
        // capture window — the field initializers join the creation-time
        // walk's stash (only a const constructor can serve a const
        // creation; the spelling is the site's current name, the selector
        // rewrite preceding this event).
        if engine.in_const_ctor
            && let Some((container, Some(site))) = engine.ctor_pending
            && let Some(key) = engine.decl_sites.get(site).map(|s| s.name)
        {
            engine.ctor_init_latch = Some((container, key));
            engine.const_capture.open_initializer();
        }
    }

    fn end_initializers(&mut self, _count: usize, _begin: Token, _end: Token) {
        let engine = &mut self.engine;
        // `ConstructorFieldInitializer` is in the official used set.
        engine.ur_seal_used();
        engine.initializer_depth -= 1;
        // F-C: the folded `super.` selectors decide at the list's close —
        // only the argument-group shape is the super-constructor reference;
        // the assignment shape (`super.name = v`) is the field-initializer
        // world — the whole-node row codes `initializer_for_non_existent_
        // field` (`_checkForInvalidField`'s non-field element arms; the
        // name resolving to a real — non-accessor — super field stays
        // silent, the `FieldElement` arm), and the `super().foo` recovery
        // residue drops silently.
        let candidates = std::mem::take(&mut engine.pending_init_super);
        // The resolved super-constructor target decides every buffered
        // `super.` formal's association — the invoked selector's spelling
        // when it heads an initializer item (`super.name(...)` is a
        // SuperConstructorInvocation upstream; `x = super.method()` keeps
        // the expression face and never redirects), `new` folding to the
        // unnamed face (error_verifier.dart's
        // `constructorElement.superConstructor`).
        let super_target = candidates
            .iter()
            .find(|candidate| candidate.invoked && candidate.head)
            .map(|candidate| candidate.name)
            .filter(|name| *name != engine.intern("new"));
        // The list's owner selects its own buffered formals: `open_ctor_site`
        // carries the secondary constructor's site inside its `: ...` list
        // and the primary constructor's inside the `this : ...` body.
        let owner = engine.open_ctor_site;
        // The givenness credit beside the drain (the entries die in it).
        engine.credit_super_formal_givens(owner, super_target);
        engine.drain_super_formals(owner, super_target);
        for candidate in candidates {
            if candidate.invoked {
                engine.check_super_constructor(candidate.container, candidate.name, candidate.span);
            } else if engine
                .source
                .get(candidate.span.end() as usize..)
                .is_some_and(|after| after.trim_start().starts_with('='))
            {
                let reports = match engine.walk_super(candidate.container, candidate.name) {
                    Walk::Found(entry) => entry.kind != MemberKind::Field,
                    Walk::ChainBroken => false,
                    Walk::Missing => true,
                };
                if reports {
                    let name_text = engine.strings.resolve(candidate.name);
                    engine.emit(
                        SemanticCode::InitializerForNonExistentField,
                        candidate.span,
                        &name_text,
                        "",
                    );
                }
            }
        }
        // #1351: the capture closes — the stashed RHS trees join the
        // walk's stash (keep-last: one initializer list per constructor).
        if let Some(key) = engine.ctor_init_latch.take() {
            let inits = std::mem::take(&mut engine.ctor_init_pending);
            engine.ctor_inits.insert(key, inits);
            let _ = engine.const_capture.close_initializer();
        }
    }

    // —— primary constructors ——————————————————————————————————————————

    /// A class/enum primary constructor IS a generative constructor
    /// (upstream `ConstructorFieldsVerifier._addPrimaryConstructor` /
    /// `hasGenerativeConstructor`, error_verifier.dart:7111-7122): register
    /// its decl site and arm `open_ctor_site` so `this.x` formals feed
    /// `ctor_formals_init` and `super.x` formals buffer under the
    /// `None`-owner tag they drain with. The head anchor is the `begin`
    /// token (`(` or `.` — the upstream `errorRange` begins there).
    fn begin_primary_constructor(&mut self, begin: Token) {
        self.flow().begin_primary_constructor(begin);
        let engine = &mut self.engine;
        let Some((container, kind)) = engine.container_context() else {
            return;
        };
        if !matches!(kind, ContainerKind::Class | ContainerKind::Enum) {
            return;
        }
        let site =
            engine.note_decl_site(container, begin.range, DeclKind::Constructor { container });
        engine.ctor_head_anchors.insert(site, begin.range);
        engine.open_ctor_site = Some(site);
        engine.primary_ctor_site = Some(site);
    }

    /// `endPrimaryConstructor` — the header is complete; `open_ctor_site`
    /// releases (the `this : ...` body member re-arms it from
    /// `primary_ctor_site`).
    fn end_primary_constructor(
        &mut self,
        kind: DeclarationKind,
        begin: Token,
        end: Token,
        const_keyword: Option<Token>,
        has_constructor_name: bool,
        name: Option<Token>,
    ) {
        self.flow().end_primary_constructor(
            kind,
            begin,
            end,
            const_keyword,
            has_constructor_name,
            name,
        );
        self.engine.open_ctor_site = None;
    }

    /// The `this : init, ...` body member re-arms the primary
    /// constructor's site so `mark_assignment`'s `ctor_init_writes` and the
    /// initializer list's `super.x` drain see it
    /// (`updateWithInitializers`, constructor_fields_verifier.dart).
    fn begin_primary_constructor_body(&mut self, token: Token, augment: Option<Token>) {
        self.flow().begin_primary_constructor_body(token, augment);
        self.engine.open_ctor_site = self.engine.primary_ctor_site;
    }

    fn end_primary_constructor_body(
        &mut self,
        begin: Token,
        this: Token,
        begin_initializers: Option<Token>,
        end: Token,
    ) {
        self.flow()
            .end_primary_constructor_body(begin, this, begin_initializers, end);
        self.engine.open_ctor_site = None;
    }

    fn begin_metadata(&mut self, token: Token) {
        let engine = &mut self.engine;
        // The arguments are a const context by language rule: the literals
        // inside capture implicitly-const and their element faces fire at
        // the literal closes (`@Target({TargetKind.classType})` over an
        // unresolved `TargetKind` reports `non_constant_set_element`).
        engine.const_capture.open_annotation();
        engine.annotation_at = Some(token.range);
    }

    fn end_metadata(&mut self, begin: Token, _period_before_name: Option<Token>, end: Token) {
        let engine = &mut self.engine;
        engine.note_metadata_args(begin, end);
        engine.const_capture.close_annotation();
        engine.annotation_at = None;
    }

    // —— receivers and selectors ——————————————————————————————————————

    fn handle_this_expression(&mut self, token: Token, context: IdentifierContext) {
        // The static type of `this` rides the flow layer's staged side
        // channel (#2292): the same answer `push_keyword_receiver` reads —
        // `this_static_ty` over the enclosing container, an extension's
        // written `on` type included — keyed on the keyword's offset. The
        // fold adopts it at `Ev::This`, so a `switch (this)` scrutinee
        // carries the real type and the exhaustiveness verdict closes;
        // argument-bearing shapes the flow lattice cannot spell never
        // arrive (the unknown-read face stands).
        if let Some((container, kind)) = self.engine.container_context() {
            let this_ty = self.engine.this_static_ty(container, kind);
            if let Ty::Named {
                name,
                nullable,
                args: ArgListId::EMPTY,
            } = this_ty
            {
                let lexeme = self.engine.strings.resolve(name).to_string();
                let base = self.flow().intern_name(&lexeme);
                self.flow().stage_this_type(
                    token.range.offset,
                    FlowTy {
                        base: FlowBase::Name(base),
                        nullable,
                    },
                );
            }
        }

        self.flow().handle_this_expression(token, context);

        self.engine.push_keyword_receiver(token, false);
        self.engine.const_capture.this_or_super(token.range);
    }

    fn handle_super_expression(&mut self, token: Token, context: IdentifierContext) {
        self.flow().handle_super_expression(token, context);

        {
            let engine = &mut self.engine;
            engine.nsm_super();
            engine.push_keyword_receiver(token, true);
            engine.note_super_expression();
            engine.const_capture.this_or_super(token.range);
        }
    }

    fn handle_dot_access(&mut self, token: Token, end: Token, is_null_aware: bool) {
        self.flow().handle_dot_access(token, end, is_null_aware);

        // The receiver's value flows into the extraction: every pending
        // candidate under it seals used (`_isUsed`'s `PropertyExtraction`
        // parent arm).
        let engine = &mut self.engine;
        engine.nsm_dot(is_null_aware);
        engine.ur_seal_used();
        engine.merge_member(is_null_aware, token.range, end.range.end());
        engine.const_capture.dot_access(is_null_aware, end.range);
        // The member send's bottom face latches here, not at the send
        // event that preceded this one (`never_send.rs`): the merged row
        // is the first shape the callee's declared return reads through.
        if !is_null_aware && engine.merged_send_returns_never() {
            self.pending_exit = Some(end);
        }
    }

    fn handle_cascade_access(&mut self, operator: Token, end: Token, is_null_aware: bool) {
        self.flow()
            .handle_cascade_access(operator, end, is_null_aware);

        let engine = &mut self.engine;
        // The first section carried the `?..` verdict (`is_null_aware`) —
        // the pending op's stand-in duty ends here.
        engine.cascade_null_aware_op = None;
        engine.merge_member(is_null_aware, operator.range, end.range.end());
        // The merged row is the section's selector: its discarded value
        // reports at this token even when the target's type is unknown to
        // the model (the official walk answers on the section node).
        if let Some(row) = engine.rows.last().map(|row| row.range.offset) {
            engine.cascade_sections.insert(row);
        }
    }

    fn handle_send(&mut self, begin: Token, end: Token) {
        self.flow().handle_send(begin, end);

        // The engine resolves the send during the parse (the index is
        // complete), so a bottom face's `handleExit` lands in the open
        // buffer before the executable replays — the parser listener alone
        // cannot know it. Faces: the resolver's bottom-callee check at the
        // name's end (`method_invocation_resolver.dart:129/:351/:489/:547`),
        // then the expression-level bottom check at the expression's end
        // (`type_analyzer.dart:815-819`).
        let exit = self.engine.consume_send(end);
        // The exit records at the next forward ([`Self::flow`]) — never
        // ahead of the send's own statement wrapper.
        match exit {
            Some(SendExit::Callee) => self.pending_exit = Some(begin),
            Some(SendExit::Result) => self.pending_exit = Some(end),
            // A synthetic head (`ensureExpressionIdentifier`'s splice) fired
            // no identifier event, so no callee row exists. Its follower is
            // the recovery face's operator — the `is` of `<synthetic> is T` —
            // and the operator's event folds the expression's `bool` row
            // there. (The head's id indexes the parser's inserted-token side
            // table — the scanned table's `get` misses it.)
            None => {
                self.engine.recovered_send = (self.engine.tokens.get(begin.id as usize).is_none())
                    .then_some(end.range.offset);
            }
        }
    }

    fn begin_if_null_assignment_rhs(&mut self, operator: Token) {
        // The `??=` RHS split's skip-path snapshot rides the flow face
        // ([`dartq_flow::ev::Ev::IfNullAssignBegin`]).
        self.flow().begin_if_null_assignment_rhs(operator);
    }

    fn handle_assignment_expression(&mut self, operator: Token, end: Token) {
        // #2174/#2360: a spellable write's resolved RHS type stages into the
        // flow layer before the assignment folds there — the name-level flow
        // walk reads an SDK static's return as unknown (`RangeError
        // .checkValidRange` is invisible to it), and the write would demote
        // what the target had promoted. The spellable writes are the plain
        // `=` and the `??=` if-null combination (#2360 — the oracle's
        // downstream verdict on a null-aware write discriminates on the
        // resolved RHS type alone); the compound operators' read-modify-write
        // face keeps the unknown written type. Single-row window; a named
        // shape rides its written-argument tree (#2364 — the flow lattice
        // interns it beside the type, `<Issue>[]` spells `List<Issue>`), a
        // tree any non-named entry cannot spell drops (the unknown-written
        // demotion stands). The staging precedes the flow
        // forward: the fold inside the call adopts the staged type.
        let staged = || -> Option<(u32, String, bool, Vec<dartq_flow::ty::StagedArg>)> {
            let engine = &mut self.engine;
            let text = engine.text_of(operator)?;
            // The token text is exact: a `??` binary expression is never
            // this event, and no other assignment spelling contains either
            // lexeme (`??=` is one token; `?=` does not exist).
            if text != "=" && text != "??=" {
                return None;
            }
            let limit = operator.range.offset;
            let target = engine
                .rows
                .iter()
                .enumerate()
                .rev()
                .find(|(_, row)| row.range.end() <= limit && !matches!(row.shape, RowShape::Value))
                .map(|(index, _)| index)?;
            // `c[i] = e` writes through the `[]=` operator: the target's
            // own type is not the assignment's destination (the same gate
            // `check_assignment` applies).
            if matches!(engine.rows[target].intent, Intent::Operator(..)) {
                return None;
            }
            // The RHS's value row: the expression's own result row, not the
            // single-row window `check_assignment` uses (a multi-row shape —
            // an argument-bearing send — is exactly the face this staging
            // exists for). The result row is the outermost *expression* row —
            // `Receiver`/`Member`-shaped, never a `Bare` or `Value` sub-row:
            // an explicit type-argument's own row (`<int>` of `C<int>()`)
            // sits rightmost yet types the argument, not the creation, so the
            // rightmost non-`Bare`/`Value` row is the result; an earlier
            // invoke (`g() ?? R()`'s `g`) is an operand, never the result.
            let rhs_index = engine.rows[target + 1..]
                .iter()
                .enumerate()
                .filter(|(_, row)| row.range.offset > limit)
                .rev()
                .find(|(_, row)| !matches!(row.shape, RowShape::Bare(_) | RowShape::Value))
                .or_else(|| {
                    engine.rows[target + 1..]
                        .iter()
                        .enumerate()
                        .filter(|(_, row)| row.range.offset > limit)
                        .last()
                })
                .map(|(index, _)| target + 1 + index)?;
            let rhs_row = engine.rows[rhs_index].clone();
            // The send face types on demand
            // ([`Engine::callee_return_ty`]): an invocation row's stored
            // type is the callee head's, the resolved return is the send's.
            // A member read resolves through the accessor walk
            // ([`Engine::initializer_read_ty`]); a folded creation's type
            // head (`R` of `R()`) is the instance the send produces. An
            // arg-bearing stored type wins over every on-demand arm — the
            // merged `Created` row's `ty` already carries the solved
            // instantiation (`C<int>()`'s `<int>`), which the declared-head
            // reads would strip to `C`.
            let from_ty = match rhs_row.ty {
                Ty::Named { args, .. } if args != crate::model::ArgListId::EMPTY => rhs_row.ty,
                _ if rhs_row.intent == Intent::Invoke => engine.callee_return_ty_for_write(&rhs_row),
                _ => {
                    let resolved = engine.initializer_read_ty(&rhs_row);
                    if resolved != Ty::UNKNOWN {
                        resolved
                    } else {
                        match &rhs_row.shape {
                            RowShape::Receiver(Receiver::Typed { ty, .. }) => *ty,
                            RowShape::Receiver(
                                Receiver::Container { name, .. }
                                | Receiver::Created { name, .. }
                                | Receiver::PrefixedContainer { name, .. },
                            ) => {
                                // An explicit-argument creation
                                // (`C<int>()`) never lands a solved
                                // `pending_send_ty` — its instantiation
                                // rides in `call_type_args` keyed on this
                                // callee row. The bare head would stage a
                                // `C` write and demote the target's
                                // promoted `C<int>`.
                                let explicit = engine
                                    .call_type_args
                                    .iter()
                                    .find(|(row, _)| *row == rhs_index)
                                    .map(|(_, tys)| tys.clone());
                                match explicit {
                                    Some(tys) => engine
                                        .boundary
                                        .args
                                        .borrow_mut()
                                        .named(*name, false, &tys),
                                    None => Ty::Named {
                                        name: *name,
                                        nullable: false,
                                        args: crate::model::ArgListId::EMPTY,
                                    },
                                }
                            }
                            _ => rhs_row.ty,
                        }
                    }
                }
            };
            // A collection literal right side (`collection_literal.rs`):
            // the opaque value row types as the target's non-null declared
            // type — the oracle's downward-inferred literal, non-null
            // whatever its arguments.
            let from_ty = if from_ty == Ty::UNKNOWN
                && matches!(rhs_row.shape, RowShape::Value)
                && engine.collection_literal_starts_at(operator.range.end())
            {
                Engine::non_null_form(engine.receiver_static_ty(&engine.rows[target]))
            } else {
                from_ty
            };
            // A `?.`-merged right side reads the short-circuit's nullable
            // result into the flow's staged face (`x = a?.b` demotes `x`'s
            // slot to the nullable written type). An operator-merged row
            // keeps its operator result — `x?.m == y` types `bool`.
            let from_ty = if engine.null_aware_members.contains(&rhs_row.range.offset)
                && !matches!(rhs_row.intent, Intent::Operator(..))
            {
                Engine::nullable_form(from_ty)
            } else {
                from_ty
            };
            // A trailing `!` re-decides the write face: the assertion's own
            // static type is the operand's non-null form (`check_null_assert`'s
            // promoteToNonNull row rewrite), so the assignment writes that
            // face — never the nullable return the declared-return recompute
            // or the `?.` merge projected (upstream Dart SDK source,
            // pkg/analyzer/lib/src/dart/resolver/assignment_expression_resolver
            // .dart — the flow write carries the assignment expression's own
            // static type, non-null for `e!` right sides: `command =
            // cmds.at('x')!` promotes `command` to `Cmd`).
            let bang = {
                // The row's expression end: an invoke row anchors at its
                // callee token (`at` of `cmds.at('x')`) — the send span
                // expands it to the closing `)` ([`Engine::invocation_span`]).
                let end = if rhs_row.intent == Intent::Invoke {
                    engine
                        .invocation_span(rhs_row.range)
                        .map(|span| span.end())
                        .unwrap_or_else(|| rhs_row.range.end())
                } else {
                    rhs_row.range.end()
                };
                let mut at = end;
                while engine.byte_at(at).is_some_and(|b| b.is_ascii_whitespace()) {
                    at += 1;
                }
                engine.byte_at(at) == Some(b'!') && engine.byte_at(at + 1) != Some(b'=')
            };
            let from_ty = if bang {
                Engine::non_null_form(from_ty)
            } else {
                from_ty
            };
            if let Ty::Named {
                name,
                nullable,
                args,
            } = from_ty
            {
                // The inference feed: the write target's name keys the
                // resolved RHS type, so a `var`-inferred declarator whose
                // initializer reads that name infers the promoted type (the
                // oracle's flow-aware inference). Plain `=` only: an if-null
                // assignment's expression type is `LUB(NonNull(T1), T2)`,
                // not the bare `T2` this feed carries.
                if text == "=" {
                    // A write inside a nested statement block sits on a
                    // conditional arm: the variable's type at any later join
                    // is the arms' join, never this arm's write, so the feed
                    // must not claim it — the entry drops (poisoning an
                    // earlier unconditional write's claim too) and the
                    // static face decides.
                    if engine.stmt_block_depth == 0 {
                        engine.staged_write_tys.insert(
                            engine.rows[target].name,
                            (
                                engine.rows[target].range.offset,
                                engine.exec_serial(),
                                from_ty,
                            ),
                        );
                    } else {
                        engine
                            .staged_write_tys
                            .remove(&engine.rows[target].name);
                    }
                }
                // The flow face: the head's lexeme plus the written-argument
                // tree. A tree with a non-named entry (an invalid or dynamic
                // spelling) cannot stage faithfully — `None` keeps the
                // unknown-written demotion, the oracle's no-promotion face
                // for an invalid written type.
                let tree = staged_arg_tree(&engine.boundary.args.borrow(), &engine.strings, args)?;
                Some((
                    engine.rows[target].range.offset,
                    engine.strings.resolve(name).to_string(),
                    nullable,
                    tree,
                ))
            } else {
                None
            }
        }();
        if let Some((offset, lexeme, nullable, tree)) = staged {
            let base = self.flow().intern_name(&lexeme);
            self.flow().stage_assignment_rhs(
                offset,
                FlowTy {
                    base: FlowBase::Name(base),
                    nullable,
                },
                tree,
            );
        }
        self.flow().handle_assignment_expression(operator, end);

        let engine = &mut self.engine;
        // `DirectAssignment`/`IfNullAssignment` are in the official used
        // set (the assignment's value flows somewhere).
        engine.ur_seal_used();
        // #1351: the open const-constructor initializer capture keeps the
        // RHS — the assignment merge below folds it away otherwise.
        if engine.ctor_init_latch.is_some() {
            engine.take_ctor_init_rhs();
        }
        engine.const_capture.assignment(operator.range);
        engine.mark_assignment(operator);
        engine.check_assignment(operator);
        engine.check_dead_null_assignment(operator);
        engine.nsm_poison();
    }

    fn begin_binary_expression(&mut self, operator: Token) {
        self.flow().begin_binary_expression(operator);

        let engine = &mut self.engine;
        engine.nsm_poison();
        let Some(text) = engine.text_of(operator) else {
            return;
        };
        // The binary-operator junction (`binary_expression_resolver.dart:
        // 442`): the left operand's row is complete at the operator token.
        // `??` is the if-null node — its operand feeds
        // `dead_null_aware_expression`, not a member resolution.
        if text != "??"
            && let Some(row) = engine.operand_row()
        {
            let at = row.range;
            let receiver_ty = engine.receiver_static_ty(row);
            engine.stage_receiver_of_type_never(at, receiver_ty);
        }
        engine
            .const_capture
            .begin_binary(bin_op_of(text), operator.range);
        if text == "??" {
            engine.begin_if_null(operator.range);
        }
        if text == "==" || text == "!=" {
            // The comparison walk parks the left operand's row index (the
            // `_checkForInvariantNullComparison` mark).
            let mark = engine.rows.len();
            engine.null_cmp_groups.push((mark, operator.range));
        }
        if USER_OPERATORS.contains(&text) {
            let op = engine.intern(text);
            engine.mark_operator(op, operator.range);
        }
    }

    fn handle_binary_rhs_end(&mut self, operator: Token, rhs_last: Token) {
        self.flow().handle_binary_rhs_end(operator, rhs_last);
    }

    fn end_binary_expression(&mut self, operator: Token, end: Token) {
        self.flow().end_binary_expression(operator, end);

        let engine = &mut self.engine;
        engine.nsm_poison();
        // `BinaryOperatorInvocation`/`IfNull` are in the official used set
        // (both operands).
        engine.ur_seal_used();
        engine.const_capture.end_binary(end.range);
        match engine.text_of(operator) {
            Some("??") => engine.end_if_null(),
            // One pop feeds both constant-comparison walks (the official
            // verifiers are independent visitors over the same node).
            op @ Some("==" | "!=") => {
                let negated = op == Some("!=");
                // The comparison is statically `bool` — latch the whole
                // extent before any downstream walk so the
                // `non_bool_condition` checkpoint's top row (the fold keeps
                // the left operand's row on top) sits inside it.
                {
                    let right = engine.rows.last().map(|row| row.range.end());
                    let left = engine
                        .rows
                        .len()
                        .checked_sub(2)
                        .and_then(|index| engine.rows.get(index))
                        .map(|row| row.range.offset);
                    if let (Some(left), Some(right)) = (left, right) {
                        engine.latch_bool_span(left, right);
                    }
                }
                if let Some((mark, op_range)) = engine.null_cmp_groups.pop() {
                    engine.null_comparison(mark, negated);
                    engine.check_nan_comparison(mark, negated, op_range);
                }
                // The comparison's static type is `bool` — latch its extent
                // for the dead-coalesce left-operand check (the immediate
                // `??` reads it; stale spans are harmless, exact-span
                // matched, and drained under a small bound).
                let right = engine.rows.last().map(|row| row.range);
                let left = engine
                    .rows
                    .len()
                    .checked_sub(2)
                    .and_then(|index| engine.rows.get(index))
                    .map(|row| row.range);
                if let (Some(left), Some(right)) = (left, right)
                    && right.end() >= left.offset
                {
                    engine.eq_spans.push(SourceRange {
                        offset: left.offset,
                        length: right.end() - left.offset,
                    });
                    let excess = engine.eq_spans.len().saturating_sub(32);
                    if excess > 0 {
                        engine.eq_spans.drain(..excess + 28);
                    }
                }
                // The `==` invocation's argument face (`_resolveEqual`'s
                // `checkForArgumentTypeNotAssignableForArgument`): the
                // left operand's declared `==` formal decides the right
                // operand, alias-folded for the verdict.
                let left = engine
                    .rows
                    .len()
                    .checked_sub(2)
                    .and_then(|index| engine.rows.get(index).cloned());
                let right = engine.rows.last().cloned();
                if let (Some(left), Some(right)) = (left, right) {
                    engine.check_equals_argument(&left, &right);
                }
            }
            // Both operands of a lazy boolean operator are condition
            // contexts (bool_expression_verifier.dart:45-90 recurses into
            // `&&`/`||`): the rows hold the two operand rows (binary
            // expressions push no row of their own), the left one claimed
            // only when it sits directly against the operator. The
            // composite's own extent is statically `bool` — the
            // `non_bool_condition` checkpoint's top row sits inside it.
            Some("&&" | "||") => {
                if let Some(right) = engine.rows.last().cloned() {
                    engine.classify_condition(&right);
                }
                let left = engine
                    .rows
                    .len()
                    .checked_sub(2)
                    .and_then(|index| engine.rows.get(index).cloned());
                if let Some(left) = left {
                    if engine.ws_between(left.range.end(), operator.range.offset) {
                        engine.classify_condition(&left);
                    }
                    if let Some(right) = engine.rows.last() {
                        let end = right.range.end();
                        engine.latch_bool_span(left.range.offset, end);
                    }
                }
            }
            // Relational comparisons are statically `bool`
            // (`_isStaticallyBool`'s composite faces): latch the whole
            // extent so an enclosing condition checkpoint spares the top
            // row. The latch has no adjacency guard — the checkpoint only
            // ever reads the last row, and a compound left operand's last
            // row sits inside `left..right` regardless.
            Some("<" | ">" | "<=" | ">=") => {
                let right = engine.rows.last().map(|row| row.range.end());
                let left = engine
                    .rows
                    .len()
                    .checked_sub(2)
                    .and_then(|index| engine.rows.get(index))
                    .map(|row| row.range.offset);
                if let (Some(left), Some(right)) = (left, right) {
                    engine.latch_bool_span(left, right);
                }
            }
            _ => {}
        }
        // The binary node's own static type folds the operand rows into
        // one result row (`fold_binary`): every enclosing single-row check
        // — `return_of_invalid_type`, the argument faces, the
        // assignability consumers — reads the node's type, and the right
        // operand's `argument_type_not_assignable` verdict runs against
        // the resolved operator formal. All arms above already read the
        // operand rows; the fold runs last so their two-row window is
        // undisturbed.
        if let Some(op_text) = engine.text_of(operator) {
            let op_text = op_text.to_string();
            engine.fold_binary(&op_text, operator.range);
        }
    }

    fn handle_unary_prefix_expression(&mut self, operator: Token) {
        self.flow().handle_unary_prefix_expression(operator);

        let engine = &mut self.engine;
        engine.nsm_poison();
        let op = match engine.text_of(operator) {
            Some("-") => UnOp::Neg,
            Some("!") => UnOp::Not,
            Some("~") => UnOp::BitNot,
            _ => UnOp::Inc, // `++`/`--` prefixes
        };
        engine.const_capture.unary_prefix(op);
        // The unary-operator junction (`unary_operator_invocation_resolver.
        // dart:49`): the operand's row is complete — the parser fires this
        // event after the operand parses.
        if let Some(row) = engine.operand_row().cloned() {
            let at = row.range;
            let receiver_ty = engine.receiver_static_ty(&row);
            engine.stage_receiver_of_type_never(at, receiver_ty);
            // The `!` operand runs the negation face
            // (`checkForNonBoolNegationExpression`,
            // bool_expression_verifier.dart:76-88): the operand's
            // nullable-bool staging runs, the condition face does not —
            // a known non-`bool` operand reports
            // `non_bool_negation_expression` at the operand's own extent
            // (the official `.at` target is the operand node), and the
            // negation's own `InvalidType`-typed result spares the
            // enclosing condition checkpoint either way (the latch). A
            // list literal's resolved type is a list type — never `bool`
            // — whatever its elements resolve to.
            if op == UnOp::Not {
                engine.classify_negation_operand(&row);
                let ty = engine.operand_static_ty(&row);
                let covered = engine.bool_decided_covers(&row);
                let operand_start = engine.skip_whitespace(operator.range.end());
                let list_shaped = engine.byte_at(operand_start) == Some(b'[')
                    && matches!(row.shape, RowShape::Value);
                // The constant-context gate: a const expression's operand
                // faults belong to the const evaluator (`const c = !1;`
                // reports `const_eval_type_bool`, never this face).
                if !covered
                    && !engine.const_capture.open()
                    && (engine.known_non_bool(ty) || list_shaped)
                {
                    let start = operand_start;
                    let end = row.range.end();
                    if start <= end {
                        engine.emit(
                            SemanticCode::NonBoolNegationExpression,
                            SourceRange {
                                offset: start,
                                length: end - start,
                            },
                            "",
                            "",
                        );
                    }
                }
                engine.latch_bool_span(row.range.offset, row.range.end());
            }
        }
    }

    fn handle_unary_prefix_assignment_expression(&mut self, operator: Token) {
        let engine = &mut self.engine;
        // The prefix `++`/`--` junction (`increment_or_decrement_resolver.
        // dart:273`, the prefix face): the operand's row is complete — the
        // parser fires this event after the operand parses.
        if let Some(row) = engine.operand_row() {
            let at = row.range;
            let receiver_ty = engine.receiver_static_ty(row);
            engine.stage_receiver_of_type_never(at, receiver_ty);
        }
        let _ = operator;
    }

    fn handle_unary_postfix_assignment_expression(&mut self, operator: Token) {
        let engine = &mut self.engine;
        engine.const_capture.unary_postfix(operator.range);
        // The postfix `++`/`--` junction (`increment_or_decrement_resolver.
        // dart:273`, the postfix face): the operand's row precedes the
        // operator token.
        if let Some(row) = engine.operand_row() {
            let at = row.range;
            let receiver_ty = engine.receiver_static_ty(row);
            engine.stage_receiver_of_type_never(at, receiver_ty);
        }
    }

    fn begin_conditional_expression(&mut self, question: Token) {
        self.flow().begin_conditional_expression(question);

        let engine = &mut self.engine;
        engine.nsm_poison();
        // The conditional's condition completed at the `?` — its operand's
        // flow-aware static type is `bool?` — the unchecked-use face
        // (#1351).
        engine.check_condition_operand();
        engine.const_capture.begin_conditional();
    }

    fn handle_conditional_expression_colon(&mut self) {
        self.flow().handle_conditional_expression_colon();

        let engine = &mut self.engine;
        engine.nsm_poison();
        engine.const_capture.conditional_colon();
    }

    /// The then-arm boundary face ([`ParserListener::
    /// end_conditional_expression_then_arm`]): the flow recorder latches
    /// the arm's span here for the colon's dead-arm row — missing this
    /// forward left the latch empty and the (0, 0) fallback resetting any
    /// open dead window at the colon (the unresolved-annotation
    /// `c ? a : b` initializer face).
    fn end_conditional_expression_then_arm(
        &mut self,
        question: Token,
        then_begin: Token,
        colon: Token,
        then_end: Token,
    ) {
        self.flow()
            .end_conditional_expression_then_arm(question, then_begin, colon, then_end);
    }

    fn end_conditional_expression(&mut self, question: Token, colon: Token, end: Token) {
        self.flow().end_conditional_expression(question, colon, end);

        let engine = &mut self.engine;
        engine.nsm_poison();
        engine.const_capture.end_conditional(end.range);
    }

    fn begin_parenthesized_expression_or_record_literal(&mut self, token: Token) {
        self.flow()
            .begin_parenthesized_expression_or_record_literal(token);

        let engine = &mut self.engine;
        engine.nsm_poison();
        engine.const_capture.parenthesized_begin();
        // The literal's field-row window opens here: the record face reads
        // it at the end event (nested literals mark their own window above).
        engine.record_marks.push((engine.rows.len(), token));
        engine.record_literal_names.push(Vec::new());
    }

    fn handle_is_operator(&mut self, is_operator: Token, not: Option<Token>) {
        self.flow().handle_is_operator(is_operator, not);

        // The type just parsed stays armed (`end_is_operator_type` never
        // takes it): the verdict walk decides here, at the official
        // `visitIsExpression` site (`_checkAllTypeChecks`,
        // best_practices_verifier.dart:987-1041).
        let engine = &mut self.engine;
        engine.nsm_poison();
        engine.check_type_check(is_operator.range, not.is_some());
        engine.const_capture.not_constant(is_operator.range);
        // An `is` expression is statically `bool` — the enclosing condition
        // checkpoint spares the operand row.
        if let Some(row) = engine.operand_row() {
            engine.latch_bool_span(row.range.offset, row.range.end());
        }
        // The operand-splice recovery face (`return is(x-1)` parses as
        // `<synthetic> is (x-1)`): the synthetic head fired no identifier
        // event, so no operand row exists, and the suppressed `TypeUse` rows
        // the recovered type left would stand in as the expression's row —
        // an `unknown` that silences the enclosing single-row check. The
        // oracle still types the expression `bool` and runs the return
        // matrix on it (`return_of_invalid_type` bool->int,
        // keyword_named_class_methods_2.dart:184). Fold the residue under
        // the expression's own row.
        if engine.recovered_send.take() == Some(is_operator.range.offset) {
            engine.fold_recovered_is(is_operator);
        } else {
            // The operand's undefined-type face (`visitIsExpression`'s
            // `type_test_with_undefined_name`): the window's suppressed
            // `Missing` row reports the dedicated code — the resolver's
            // plain path never fires for the operand (the non-type-pos
            // suppression holds). The recovery shape keeps its silent
            // record contract (the fold above answers it instead).
            engine.emit_is_type_miss();
        }
        // The `is` expression's own row: statically `bool`. The operand row
        // folds under it (deferred — its resolution still runs in pass 2),
        // so the enclosing single-row checks read the node's type, never
        // the operand's (`bool isValidKey(Object? o) => o is Iterable<E>;`
        // returns the test's `bool`, not the promoted operand — #2172's
        // equality.dart rows). The recovered fold pushes the same shape.
        if let Some(index) = engine
            .rows
            .iter()
            .rposition(|row| !matches!(row.shape, RowShape::TypeUse(..)))
        {
            let start = engine.rows[index].range.offset;
            let end = engine.rows.last().map_or(start, |row| row.range.end());
            for row in engine.rows.drain(index..) {
                engine.deferred_miss_rows.push(row);
            }
            engine.rows.push(Row {
                name: engine.intern(""),
                range: SourceRange {
                    offset: start,
                    length: end.saturating_sub(start),
                },
                qualified: false,
                shape: RowShape::Value,
                intent: Intent::Read,
                write_op: None,
                ty: Ty::named(engine.members.primitive_ids[3]),
            });
        }
    }

    fn handle_as_operator(&mut self, operator: Token) {
        self.flow().handle_as_operator(operator);

        let engine = &mut self.engine;
        engine.nsm_poison();
        // The cast target's undefined-type face (`visitAsExpression`'s
        // `cast_to_non_type`) — the `is` twin over the as-operand window.
        engine.emit_as_type_miss();
        // The cast target folds like the operand side
        // ([`Self::bind_declared_type`]): every target face below reads the
        // resolved interface type — `x as IntAlias` over an `int` operand
        // is dead.
        let cast_ty = engine.armed_ty.map(|ty| engine.fold_alias_ty(ty));
        // A cast whose target is non-nullable `bool` is statically `bool`,
        // and an undecidable target leaves the operand's own row answering
        // the enclosing condition checkpoint — both spare the operand row.
        // A `dynamic` target is NOT latched: `AsExpression.staticType` is
        // the cast type itself (`dynamic`, never `bool`), so its fold pushes
        // a `Value{Dynamic}` result row at the operand's exact span (no
        // `TypeUse` row extends a builtin head) — latching that span let
        // `end_parenthesized_expression` collapse the group's `inner_ty` to
        // `bool` where the oracle answers `dynamic`. The checkpoint spares
        // a `dynamic` result either way (`isAssignableTo(dynamic, bool)`;
        // `known_non_bool` already answers `false` for it), so the latch
        // only ever misled the paren collapse. A `bool?` target stays
        // unlatched for the same reason — its result row must keep the
        // nullable face for the condition's `unchecked_use_of_nullable_value`
        // staging. A known non-`bool` cast target leaves the row unlatched:
        // the checkpoint then reports `non_bool_condition` at the operand
        // row, whose start is the cast expression's own start (the oracle's
        // anchor, `bool_expression_verifier.dart:47`).
        let cast_boolish = match cast_ty {
            Some(Ty::Named {
                name,
                nullable: false,
                ..
            }) => name == engine.members.primitive_ids[3],
            Some(Ty::Unknown) => true,
            _ => false,
        };
        if cast_boolish && let Some(row) = engine.operand_row() {
            engine.latch_bool_span(row.range.offset, row.range.end());
        }
        // Pattern casts report through `handle_cast_pattern`; the
        // expression form is `visitAsExpression`'s `_isUnnecessaryCast`
        // (best_practices_verifier.dart:170-183, 1708-1741).
        if engine.switch.pattern_depth == 0 {
            let operand = engine.operand_row().map_or(Ty::Unknown, |row| row.ty);
            // The simple-local face the nullable-cast join keys on (the
            // row's span and name), snapshotted before the mutable calls.
            let simple_local = engine.operand_row().and_then(|row| {
                matches!(row.shape, RowShape::Bare(BareInfo::Plain { .. }))
                    .then(|| (row.range, row.name))
            });
            // `visitAsExpression`'s expression faces anchor at the whole
            // AsExpression (`.at(node)`, best_practices_verifier.dart:177-186)
            // — the operand's start through the cast type's written end, not
            // the `as` token. A member-chain operand (`x.y as T`) starts at
            // the receiver's span — the merged row anchors at the selector.
            let cast_span = engine
                .operand_row()
                .zip(engine.armed_ty_span)
                .map(|(row, ty_span)| {
                    let start = engine
                        .property_access_span(row.range)
                        .map_or(row.range.offset, |span| span.offset);
                    SourceRange {
                        offset: start,
                        length: ty_span.end().saturating_sub(start),
                    }
                })
                .unwrap_or(operator.range);
            engine.check_unnecessary_cast(cast_span, operand, cast_ty);
            // `visitAsExpression`'s cast face
            // (best_practices_verifier.dart:180-190): the same pair decides
            // `cast_from_null_always_fails`.
            engine.check_cast_from_null(cast_span, operand, cast_ty);
            // `visitAsExpression`'s unassigned-read face
            // (type_analyzer.dart:2107-2126): stages the nullable-cast
            // candidate for the teardown's `UnassignedRead` join.
            engine.check_cast_from_nullable(simple_local, operand, cast_ty);
            // The Never-operand face: the operand's exit leaves the cast
            // type dead inside an initializer (`check_dead_cast_tail`);
            // the send row's raw type is `Unknown`, the junction reads the
            // callee's return type.
            let dead_operand = engine
                .operand_row()
                .map_or(Ty::Unknown, |row| engine.operand_static_ty(row));
            engine.check_dead_cast_tail(operator, dead_operand);
            // The `as` expression's own row: the cast target's resolved
            // type (`AsExpression.staticType`). The operand row folds under
            // it (deferred — its resolution still runs in pass 2), so the
            // chain's next segment resolves on the target, never on the
            // operand's type (`(value as Comparable<Object?>).compareTo(o)`
            // reads Comparable's face, not Object's — #2172's utils.dart
            // row). An unresolved target keeps the operand row (suppress as
            // before).
            if let Some(ty) = cast_ty.filter(|ty| *ty != Ty::UNKNOWN)
                && let Some(index) = engine
                    .rows
                    .iter()
                    .rposition(|row| !matches!(row.shape, RowShape::TypeUse(..)))
            {
                let start = engine.rows[index].range.offset;
                let end = engine.rows.last().map_or(start, |row| row.range.end());
                for row in engine.rows.drain(index..) {
                    engine.deferred_miss_rows.push(row);
                }
                engine.rows.push(Row {
                    name: engine.intern(""),
                    range: SourceRange {
                        offset: start,
                        length: end.saturating_sub(start),
                    },
                    qualified: false,
                    shape: RowShape::Value,
                    intent: Intent::Read,
                    write_op: None,
                    ty,
                });
            }
        }
        engine.const_capture.opaque_leaf(operator.range);
    }

    // —— suppression brackets —————————————————————————————————————————

    fn begin_is_operator_type(&mut self, operator: Token) {
        self.flow().begin_is_operator_type(operator);

        let engine = &mut self.engine;
        engine.enter_non_type_pos();
        // The `is` operand's type never counts as a type use (the official
        // `_insideIsExpression` face).
        engine.is_type_pos += 1;
        // The type's own row window opens here (the recovered-operand fold
        // reads it at the operator's event).
        engine.is_type_mark = Some(engine.rows.len());
    }

    fn end_is_operator_type(&mut self, operator: Token) {
        self.flow().end_is_operator_type(operator);

        let engine = &mut self.engine;
        engine.leave_non_type_pos();
        engine.is_type_pos -= 1;
    }

    fn begin_as_operator_type(&mut self, operator: Token) {
        self.flow().begin_as_operator_type(operator);

        // The non-type-pos suppression is the expression cast's face — its
        // `cast_to_non_type` verdict rides `emit_as_type_miss` at
        // `handle_as_operator`. A cast pattern (`case p as T:`) fires this
        // same bracket but never fires `handle_as_operator`; upstream its
        // NamedType's parent is a CastPattern, not the AsExpression the
        // `castToNonType` arm requires, so the type falls through to the
        // plain `undefined_class`/`not_a_type` arms
        // (named_type_resolver.dart's `reportNullOrNonTypeElement`).
        // Inside a pattern the bracket stays open — symmetric in
        // `end_as_operator_type` — letting the type's TypeUse row resolve
        // through pass 2 like every other position. An expression `as`
        // cannot reach this shape: constant pattern operands admit no
        // `as` grammar.
        if self.engine.switch.pattern_depth == 0 {
            self.engine.enter_non_type_pos();
        }
        self.engine.as_type_mark = Some(self.engine.rows.len());
    }

    fn end_as_operator_type(&mut self, operator: Token) {
        self.flow().end_as_operator_type(operator);

        if self.engine.switch.pattern_depth == 0 {
            self.engine.leave_non_type_pos();
        }
    }

    fn begin_new_expression(&mut self, token: Token) {
        self.engine.enter_ctor_pos();
        self.engine.const_capture.begin_opaque_group();
        let _ = token;
    }

    fn end_new_expression(&mut self, token: Token) {
        self.engine.leave_ctor_pos();
        self.engine.const_capture.end_opaque_group(token.range);
    }

    fn begin_const_expression(&mut self, const_keyword: Token) {
        let engine = &mut self.engine;
        engine.nsm_poison();
        engine.enter_ctor_pos();
        engine.open_const_ctor_invocation();
        engine.const_kw_spans.push(const_keyword.range);
    }

    fn end_const_expression(&mut self, token: Token) {
        let engine = &mut self.engine;
        // The creation's written span (`const` keyword through the closing
        // token): an argument row folding inside belongs to the creation's
        // own evaluation — the resolution-side argument faces read the
        // creation's container type, never an inner literal's (`isExp(const
        // Constant(null))` checks `Constant`, not `null`).
        if let Some(kw) = engine.const_kw_spans.last().copied() {
            let span = SourceRange {
                offset: kw.offset,
                length: token.range.end().saturating_sub(kw.offset),
            };
            engine.const_capture.note_creation_span(span);
        }
        engine.const_kw_spans.pop();
        // The `const_with_non_const` face: a const creation whose target
        // resolves to a container without a const constructor reports at
        // the const keyword — the container may live in any unit of the
        // element model (its const-constructor bit is a model fact).
        // Targets the model cannot resolve (SDK faces, missing imports)
        // defer (the under-report posture the const-argument gate shares).
        if let Some((callee, kw)) = engine.const_creation_pending.take()
            && let Some(name) = callee
            && resolve_container(
                engine.graph,
                engine.namespaces,
                engine.index,
                engine.lib,
                engine.file,
                name,
            )
            .is_some()
        {
            // The verdict stages for the file-tail merge beside the other
            // deferred const faces.
            engine.pending_const_non_const.push((name, kw));
        }
        engine.close_const_ctor_invocation();
        engine.leave_ctor_pos();
    }
    fn begin_implicit_creation_expression(&mut self, _token: Token) {
        self.engine.enter_ctor_pos();
        self.engine.implicit_creation_depth += 1;
    }

    fn end_implicit_creation_expression(&mut self, _begin: Token, _open_angle: Token) {
        let engine = &mut self.engine;
        engine.end_implicit_creation();
        engine.implicit_creation_depth -= 1;
        engine.leave_ctor_pos();
    }

    fn begin_arguments(&mut self, token: Token) {
        self.flow().begin_arguments(token);

        let engine = &mut self.engine;
        engine.nsm_arguments_open();
        // The #749 candidate window: arms on a plain-send class-container
        // head outside every const context (before the capture opens its
        // own level — the head node must stay below the window's base).
        // Every list pushes exactly one slot — `Some` when armed, `None`
        // otherwise — so each close pops its own list's slot and a
        // non-candidate nested list can never steal the candidate's.
        let arm = engine.arm_literal_ctor_args();
        engine.literal_ctor_arms.push(arm);
        // The watermark: only pendings registered inside the list seal used
        // at the close (the official `ArgumentList` parent arm); the
        // callee's own candidate predates it.
        let base = engine.ur_pending.len();
        engine.ur_arg_base.push(base);
        engine.const_capture.begin_arguments();
        engine.open_arguments(token.range.offset);
    }

    fn end_arguments(&mut self, count: usize, begin: Token, end: Token) {
        self.flow().end_arguments(count, begin, end);

        // The armed candidate window closes first: its argument trees
        // leave the capture before the const-context interpretations of
        // the generic close can touch them.
        self.engine.close_literal_ctor_args(end.range);
        // A super/redirect-constructor invocation's argument list is the
        // oracle's `_reportNotPotentialConstantsArguments` site
        // (`_validateConstructorInitializers`, constant_verifier.dart
        // :851-868): each argument must be *potentially* constant — the
        // weaker gate that still fails an unresolved or non-const name
        // expression (`invalid_constant` at the node, not the creation's
        // `const_with_non_constant_argument`). The dedicated close returns
        // the arg nodes and folds the invocation to a non-constant leaf for
        // the enclosing initializer window — the generic `ClosedArgs`
        // dispatch never sees a super-arglist (it folds the args away).
        let init_super =
            self.engine.initializer_depth > 0 && self.engine.init_super_callee().is_some();
        let closed = if init_super {
            let closed = self
                .engine
                .const_capture
                .end_init_invocation_arguments(end.range);
            if let Some(closed) = &closed {
                // The scan is the const constructor's own gate (upstream
                // Dart SDK source, pkg/analyzer/lib/src/dart/constant/
                // constant_verifier.dart — `_validateConstructorInitializers`
                // runs under `node.constKeyword != null` only; probe2484
                // p1d/p1e: a NON-const constructor's `super(f)` stays
                // silent), and its name rule is the potentially-constant
                // one (`potentially_constant.dart` `_nameExpression`): the
                // enclosing const constructor's formal parameters are
                // potentially constant, so the scan runs with the formals
                // bound as opaque names (the [`CtorScope`] overlay) — a
                // const-ctor formal passed through `: super(f)` /
                // `: this.other(f)` stays silent, a genuinely non-constant
                // argument still collects.
                let mut spans = Vec::new();
                if self.engine.in_const_ctor {
                    let formals = self.engine.ctor_init_formals();
                    if formals.is_empty() {
                        for arg in &closed.args {
                            collect_not_potentially_constant(arg, &self.engine, &mut spans);
                        }
                    } else {
                        let mut env: FxHashMap<StringId, CtorBinding> = FxHashMap::default();
                        for name in formals {
                            env.insert(name, CtorBinding::Opaque);
                        }
                        let scope = CtorScope {
                            engine: &self.engine,
                            env: &env,
                        };
                        for arg in &closed.args {
                            collect_not_potentially_constant(arg, &scope, &mut spans);
                        }
                    }
                }
                let engine = &mut self.engine;
                for span in spans {
                    engine.emit(SemanticCode::InvalidConstant, span, "", "");
                }
            }
            // The dedicated close consumed the arg level — nothing left
            // for the generic close to dispatch on.
            None
        } else {
            self.engine.const_capture.end_arguments(end.range)
        };
        let engine = &mut self.engine;
        if let Some(closed) = closed {
            if closed.creation_owned {
                // The creation's own list: each argument checks against
                // `const_with_non_constant_argument`; the gate lives until
                // the window closes (nested calls arm their own gates).
                let gate = engine.arg_gates.last().copied().unwrap_or(false);
                // The initializer walk (#1351) reports at the creation node
                // — the oracle's creation-level fault precedes the
                // per-argument rows in the source-ordered report.
                if gate && let Some(callee) = closed.callee {
                    engine.check_ctor_initializers(callee, &closed.args, closed.span);
                }
                engine.check_const_args(gate, &closed.args);
                if closed.ctor_invocation {
                    // The `const_with_non_const` capture: the target check
                    // decides at the window close against the const-keyword
                    // anchor.
                    if let Some(&kw) = engine.const_kw_spans.last() {
                        engine.const_creation_pending = Some((closed.callee, kw));
                    }
                    // The creation's value joins the enclosing window —
                    // typed by its container when this file declared the
                    // const constructor (type folding), opaque otherwise.
                    let ctor = closed.callee.is_some_and(|n| engine.const_ctor_target(n));
                    let value = if ctor {
                        ConstantValue::Created(closed.callee.expect("checked above"))
                    } else {
                        ConstantValue::Instance(crate::const_eval::InstanceKind::Object)
                    };
                    engine.const_capture.push_value(closed.span, value);
                }
            } else if let Some(name) = closed.callee {
                // `name(...)` in a const context: the implicit const
                // creation form checks per argument; any other resolvable
                // callee is a method-invocation fault; unknown names defer
                // (their own row reports).
                match engine.classify_call(name) {
                    CallClass::ConstCtor => {
                        // The initializer walk (#1351) reports at the
                        // creation node — the oracle's creation-level fault
                        // precedes the per-argument rows, the creation-owned
                        // branch's order.
                        engine.check_ctor_initializers(name, &closed.args, closed.span);
                        engine.check_const_args(true, &closed.args);
                        // The selector face: a const-constructor container
                        // whose written selector never resolves (`const y =
                        // C.named()` over `class C { const C(); }`) fails
                        // the initializer beside the resolution row — the
                        // tail pass stages the
                        // `const_initialized_with_non_constant_value` row
                        // at the creation (the oracle's
                        // const_with_undefined_constructor fixture).
                        if let Some(init) = engine.pending_init_names.last().copied()
                            && !engine.const_creation_valid(name, closed.span)
                        {
                            engine.open_const_creations.push((
                                init,
                                closed.span,
                                ConstCreationEntry::Creation { container: name },
                            ));
                        }
                        engine
                            .const_capture
                            .push_value(closed.span, ConstantValue::Created(name));
                    }
                    CallClass::Invocable => {
                        engine.emit(SemanticCode::ConstEvalMethodInvocation, closed.span, "", "");
                        engine.const_capture.opaque_leaf(closed.span);
                    }
                    CallClass::Unknown => {
                        // The implicit const creation face: a class
                        // container's plain send inside a const-declaration
                        // initializer is the implicit `const C(...)` —
                        // constant only when a const constructor answers
                        // the call (the oracle's constant verifier).
                        // Foreign/unresolved names keep the opaque defer.
                        // A named-ctor selector (`D.named(`) reaches here
                        // as the bare `named` ident — the capture split `D`
                        // off as the creation head, never a `Member`
                        // operand — so the selector row's qualified face and
                        // the class receiver below it name the creation.
                        let named_ctor = engine
                            .invokes
                            .last()
                            .and_then(|slot| *slot)
                            .filter(|&callee| {
                                engine.rows.get(callee).is_some_and(|row| row.qualified)
                            })
                            .and_then(|callee| {
                                let receiver =
                                    callee.checked_sub(1).and_then(|i| engine.rows.get(i))?;
                                match &receiver.shape {
                                    RowShape::Receiver(Receiver::Created {
                                        name,
                                        kind: ContainerKind::Class,
                                    })
                                    | RowShape::Bare(BareInfo::Container {
                                        name,
                                        kind: ContainerKind::Class,
                                    }) => Some(*name),
                                    _ => None,
                                }
                            })
                            .filter(|&container| {
                                let declared = engine
                                    .const_ctor_decl(container)
                                    .is_some_and(|decl| decl.constructors.contains(&name));
                                declared || {
                                    let c = engine.strings.resolve(container);
                                    let s = engine.strings.resolve(name);
                                    CORE_CONST_CTOR
                                        .iter()
                                        .any(|(k, sels)| *k == c && sels.contains(&s))
                                }
                            });
                        // An unindexed `dart:core` const-ctor head (`pragma(`,
                        // `Duration(`, `Object(`, `bool.fromEnvironment(`)
                        // resolves to no indexed container but answers the
                        // implicit `const` all the same — the frozen
                        // [`CORE_CONST_CTOR`] set is the oracle's verdict.
                        let core_const = {
                            let c = engine.strings.resolve(name);
                            CORE_CONST_CTOR
                                .iter()
                                .any(|(k, sels)| *k == c && sels.contains(&""))
                        };
                        if let Some(container) = named_ctor {
                            // `C.named(` is the implicit `const
                            // C.named(...)` — a valid creation folds to its
                            // container's value; no failure row stages.
                            engine
                                .const_capture
                                .push_named_creation(container, closed.span);
                        } else if core_const {
                            engine
                                .const_capture
                                .push_value(closed.span, ConstantValue::Created(name));
                        } else {
                            if let Some(init) = engine.pending_init_names.last().copied() {
                                match resolve_container(
                                    engine.graph,
                                    engine.namespaces,
                                    engine.index,
                                    engine.lib,
                                    engine.file,
                                    name,
                                ) {
                                    Some((decl, _)) if decl.kind == ContainerKind::Class => {
                                        engine.open_const_creations.push((
                                            init,
                                            closed.span,
                                            ConstCreationEntry::Creation { container: name },
                                        ));
                                    }
                                    // An unresolved or non-container callee
                                    // (`const x = Missing()`, `const x =
                                    // F()` over a function typedef): the
                                    // oracle's constant evaluation is
                                    // known-invalid at the initializer —
                                    // the
                                    // `const_initialized_with_non_constant_value`
                                    // row rides beside the callee's own
                                    // resolution report (probed: `dart
                                    // analyze` 3.13.1 pairs the
                                    // `undefined_function` /
                                    // `invocation_of_non_function` row with
                                    // the initializer's). Enum/mixin/
                                    // extension-type heads resolve to their
                                    // own faces (`invalid_reference_to_
                                    // generative_enum_constructor`), so the
                                    // failure entry only stages when no
                                    // container answers.
                                    None => {
                                        engine.open_const_creations.push((
                                            init,
                                            closed.span,
                                            ConstCreationEntry::ArgFailure,
                                        ));
                                    }
                                    _ => {}
                                }
                            }
                            engine.const_capture.opaque_leaf(closed.span);
                        }
                    }
                }
            } else {
                engine.const_capture.opaque_leaf(closed.span);
            }
        }
        // The annotation's own argument group joins the given-at-call index
        // (`_addParametersForArguments` walks every Annotation's
        // argumentList too): a metadata creation's invoke slot carries no
        // callee link, so the generic close's registration never fires for
        // it. The link's absence is the discriminator — a nested call
        // inside an argument keeps its own link and skips; enum constants
        // check theirs separately.
        if let Some(level) = engine.arg_levels.last()
            && engine.invokes.last().and_then(|slot| *slot).is_none()
            && !level.enum_const
            && let Some(annotation) = engine.pending_annotations.last().map(|a| a.name)
        {
            // The lookup spelling of the annotated constructor: the
            // selector segment of a qualified annotation (`@A.named(...)`),
            // the whole name otherwise.
            let text = engine.strings.resolve(annotation);
            let selector = text.rsplit('.').next().unwrap_or(text);
            let key = engine.intern(selector);
            for (given, ..) in &level.args {
                if let Some(given) = given {
                    engine.named_given.insert((key, *given));
                }
            }
            let named = level
                .args
                .iter()
                .filter(|(name, ..)| name.is_some())
                .count();
            if let Ok(positional) = u16::try_from(count.saturating_sub(named)) {
                let given = engine.positional_given.entry(key).or_default();
                *given = (*given).max(positional);
            }
        }
        engine.close_arguments(u16::try_from(count).unwrap_or(u16::MAX), end.range);
        // The constant-pattern object shape (`case const A():`) never fires
        // a send event — its argument list closes without an invocation
        // walk (the reduced parser's `parse_selector_chain` answers the
        // `(` selector in place, expressions.rs:1364/:1381's `handle_send`
        // route only exists on the statement-level `parse_super_expression`
        // path) — so the callee's pending link resolves at the close: a
        // missing head upgrades to the invoked intent and codes
        // `undefined_function` (issue50591_example2 3:11), while a keyword
        // receiver (`case this():`, `case super():`) runs the invocation
        // face here — its `invocation_of_non_function_expression` decision
        // is the resolver's, not the constant verifier's (upstream answers
        // it on the `FunctionExpressionInvocation` before the verifier's
        // `InvalidType` early-return silences the generic face).
        if engine.switch.pattern_depth > 0
            && let Some((callee, _)) = engine.pending_send.take()
        {
            match engine.rows.get(callee).map(|row| &row.shape) {
                Some(RowShape::Bare(BareInfo::Missing { .. })) => {
                    if let Some(row) = engine.rows.get_mut(callee) {
                        row.intent = Intent::Invoke;
                    }
                }
                Some(RowShape::Receiver(Receiver::This { .. } | Receiver::Super { .. })) => {
                    engine.check_invocation_face(callee);
                }
                // Any other callee shape (a resolved head, a value row) has
                // no send event coming — the link is consumed so it cannot
                // leak into the next statement's send.
                _ => {}
            }
        }
        // The list's own expressions were uses (`ArgumentList`).
        engine.ur_seal_used_within();
        engine.nsm_arguments_closed(count);
    }

    fn handle_positional_argument(&mut self, token: Token) {
        self.engine.const_capture.positional_argument();
        self.engine.open_argument(None, token.range);
    }

    fn handle_named_argument(&mut self, colon: Token) {
        self.flow().handle_named_argument(colon);

        // The capture opens the named argument here (fires before the
        // value); the row engine opened it at the label's identifier event.
        self.engine.const_capture.named_argument();
    }

    fn begin_return_statement(&mut self, token: Token) {
        let engine = &mut self.engine;
        engine.nsm_return_open(token);
        engine.return_mark = Some(engine.rows.len());
    }

    fn end_return_statement(&mut self, has_expression: bool, begin: Token, end: Token) {
        self.flow().end_return_statement(has_expression, begin, end);

        let engine = &mut self.engine;
        // `ReturnStatement` is in the official used set.
        engine.ur_seal_used();
        engine.nsm_return_closed(has_expression, end);
        let mark = engine.return_mark.take();
        if !has_expression {
            // Bare `return;` is a `void` expression in the oracle's matrix.
            engine.check_return(Ty::Void, end.range, false);
            return;
        }
        if let Some(mark) = mark
            && let Some((ty, literal, range)) = engine.single_row_since(mark)
        {
            engine.check_return(ty, range, literal);
        }
    }

    fn handle_expression_function_body(&mut self, arrow: Token, end: Option<Token>) {
        self.flow().handle_expression_function_body(arrow, end);

        let engine = &mut self.engine;
        // `ExpressionFunctionBody` is in the official used set.
        engine.ur_seal_used();
        // #1314 FAM7: the `=>` body's verdict is "present" (a closure body
        // inside a default value leaves the outer rows alone).
        if engine.default_value_depth == 0 {
            engine.flush_default_checks(true);
        }
        // The #748 shape adjudicates before the mark is consumed (the
        // body expression was exactly `super.noSuchMethod(<one argument>)`).
        engine.nsm_expression_closed();
        let Some(mark) = engine.body_mark.take() else {
            return;
        };
        // A `void`-returning `=>` body never checks (`return_type_verifier
        // .dart` `verifyExpressionFunctionBody` early-returns when the
        // flattened return type is `VoidType` — an expression-bodied
        // `void f() => e` runs the expression for its value, not its type).
        // The statement body's `return e` keeps checking upstream, so the
        // skip lives only on this event.
        let void_return = engine
            .frames
            .iter()
            .rev()
            .find_map(|f| match &f.kind {
                FrameKind::Executable { ret: Some(ret), .. } => Some(ret.ty == Ty::Void),
                _ => None,
            })
            .unwrap_or(false);
        if !void_return && let Some((ty, literal, range)) = engine.single_row_since(mark) {
            engine.check_return(ty, range, literal);
        }
    }

    fn handle_async_modifier(&mut self, async_kw: Option<Token>, star: Option<Token>) {
        self.flow().handle_async_modifier(async_kw, star);

        let engine = &mut self.engine;
        engine.body_mark = Some(engine.rows.len());
        // The completion gates read the modifier from the frame: `sync*`/
        // `async*` are generator bodies, the `async` spelling alone decides
        // the legality gate (`sync` never reaches it — the generator gate
        // front-runs).
        let is_async = async_kw.and_then(|t| engine.text_of(t)) == Some("async");
        engine.note_async_modifier(is_async, star.is_some());
    }

    fn begin_cascade(&mut self, operator: Token) {
        self.flow().begin_cascade(operator);

        let engine = &mut self.engine;
        engine.nsm_poison();
        // The cascade target completed before the `..`: it is a use (the
        // official `CascadeExpression` target arm); the sections' pendings
        // discard at the close.
        engine.ur_seal_used();
        engine.in_cascade = true;
        // `?..`'s null-aware verdict anchors at this operator — its first
        // section may parse as an index (`?..[i]`), which carries no `?`
        // of its own, so the range waits here for that event (#1758).
        engine.cascade_null_aware_op =
            (engine.text_of(operator) == Some("?..")).then_some(operator.range);
        engine.const_capture.begin_opaque_group();
        let _ = operator;
    }

    fn end_cascade(&mut self) {
        self.flow().end_cascade();

        let engine = &mut self.engine;
        engine.nsm_poison();
        // The sections are not uses (the official `CascadeExpression` arm
        // spares only the target).
        engine.ur_seal_discard();
        engine.in_cascade = false;
        engine.cascade_null_aware_op = None;
        engine.const_capture.end_opaque_group(SourceRange {
            offset: 0,
            length: 0,
        });
    }

    fn handle_null_aware_index_operand_begin(&mut self, question: Token) {
        // Pure flow-face marker (the `?[` split timing): the semantic
        // faces read the access at `handle_indexed_expression`.
        self.flow().handle_null_aware_index_operand_begin(question);
    }

    fn handle_null_aware_member_begin(&mut self, question: Token) {
        // Pure flow-face marker (the `?.` split timing): the semantic
        // faces read the access at `handle_dot_access`.
        self.flow().handle_null_aware_member_begin(question);
    }

    fn handle_indexed_expression(&mut self, question: Option<Token>, open: Token, close: Token) {
        self.flow().handle_indexed_expression(question, open, close);

        let engine = &mut self.engine;
        engine.nsm_poison();
        // `IndexExpression`/`IndexAssignmentTarget` are in the official
        // used set.
        engine.ur_seal_used();
        // The oracle anchors index-access reports on the bracket pair
        // (`property_element_resolver.dart`, `_reportUnresolvedIndex`).
        let bracket = SourceRange {
            offset: open.range.offset,
            length: close.range.end() - open.range.offset,
        };
        // `#279: e1[e2]` is a spec constant the subset cannot value —
        // both operands defer through an opaque leaf.
        engine.const_capture.indexed(bracket);
        // The receiver: the closest row wholly before the `[`.
        let Some(pos) = engine
            .rows
            .iter()
            .rposition(|row| row.range.end() <= open.range.offset)
        else {
            return;
        };
        // `?..[i]`'s index section carries no `?` of its own — the `?`
        // rode the `?..` operator, so the pending cascade-op range stands
        // in as the question token's span (consumed: only the FIRST
        // section is null-aware).
        let question_range = match question {
            Some(q) => Some(q.range),
            None => engine.cascade_null_aware_op.take(),
        };
        if let Some(question_range) = question_range {
            // The same receiver typing `merge_member` runs (`f()?[i]`'s
            // receiver is the `f()` send, its result carried by
            // `callee_return_ty`, #1758).
            let ty = Engine::receiver_row_ty(&engine.rows[pos]);
            let ty = if ty == Ty::UNKNOWN {
                engine.receiver_static_ty(&engine.rows[pos])
            } else {
                ty
            };
            // The `?[`/`?..` operator is the anchor
            // (`atOffset(operator.offset, operator.next.end -
            // operator.offset)`), not the receiver row.
            let op_range = question_range;
            let recv = engine.rows[pos].range;
            let kind = if question.is_some() {
                NullAwareKind::Index
            } else {
                NullAwareKind::Cascade
            };
            // The bound-aware strictly face: an in-scope parameter
            // receiver decides through its written bound (`T extends C?`
            // keeps `?[` legal, `T extends C` reports).
            if engine.strictly_non_nullable(ty) {
                let (op, replacement) = kind.lexemes();
                engine.emit3(
                    SemanticCode::InvalidNullAwareOperator,
                    op_range,
                    op,
                    replacement,
                    "",
                );
            } else if engine.local_hits.contains_key(&recv.offset) {
                engine.null_aware_staged.push((recv, op_range, kind));
                if Engine::is_nullable_ty(ty) {
                    // The `c1?[0]` dead-chain candidate: a null-aware index
                    // over a local's nullable read, resolved at teardown
                    // (`_verifyUnassignedVariable`'s index face).
                    engine
                        .dead_null_chains
                        .push((question_range.offset, close.range.end(), recv));
                }
            }
        }
        // Fold (`a[i]`, `c.j[i] = v`): the index arguments leave the stack —
        // the receiver row carries the `[]` access with the bracket span, and
        // a value row stands for the result so compound-expression detection
        // (argument types, initializer checks) still sees one opaque row. A
        // receiver whose own resolution left no name row (call results,
        // groups) stays unfolded — its operator checks suppress anyway.
        if matches!(
            engine.rows[pos].shape,
            RowShape::Bare(_) | RowShape::Member { .. } | RowShape::Receiver(_)
        ) && engine.rows[pos].intent == Intent::Read
        {
            let index_op = engine.intern("[]");
            // The result row's static type (`index_result.rs`): the
            // receiver's `[]` return under its written arguments, nullable
            // through a `?[` access — the staged write face and the
            // operand folds read it where the opaque row read a gap.
            let receiver_ty = {
                let ty = Engine::receiver_row_ty(&engine.rows[pos]);
                if ty == Ty::UNKNOWN {
                    engine.receiver_static_ty(&engine.rows[pos])
                } else {
                    ty
                }
            };
            let result_ty = match engine.index_result_ty(receiver_ty) {
                ty if question.is_some() => Engine::nullable_form(ty),
                ty => ty,
            };
            
            // The operand rows leave the stack, but every operand still
            // owes its pass-2 resolution — the official resolves every
            // subexpression (`builder..foo[bar]`'s `bar` reports its
            // `undefined_identifier`, issue_42267, and a resolved operand's
            // `used_names`/`read_names` marks stand — `a[_key]` reads the
            // field for `unused_field`). They defer to pass 2 through the
            // same set the send fold drains into; only their mergeable
            // stack position dies here.
            for row in engine.rows.drain(pos + 1..) {
                engine.deferred_miss_rows.push(row);
            }
            engine.rows[pos].intent = Intent::Operator(index_op, bracket);
            engine.push_value_row(close);
            if result_ty != Ty::UNKNOWN
                && let Some(result) = engine.rows.last_mut()
            {
                result.ty = result_ty;
            }
        }
    }

    // —— patterns & switch exhaustiveness (M2 face) ——————————————————————

    fn begin_pattern(&mut self, token: Token) {
        self.flow().begin_pattern(token);

        let engine = &mut self.engine;
        engine.switch.pattern_depth += 1;
        // A pattern opening while a condition window is armed marks an
        // if-case: the checkpoint's plain-condition face is routed away
        // upstream (`analyzeIfCaseStatement`/`analyzeIfCaseElement` never
        // run `checkForNonBoolCondition`). The window is armed by the
        // statement forms (`cond_mark`) and by the collection element's
        // condition (`if_element_cond`).
        if engine.cond_mark.is_some() || engine.if_element_cond > 0 {
            engine.cond_case = true;
            // The if-case root opens its one-case window: the condition
            // value is the pattern's matched type, so the shared
            // root-pattern checks run against it (the official visits the
            // pattern with the scrutinee's type). Exactly the root: a
            // case header's window is already open for its own root
            // (`if (switch (x) { case A _ => 1, _ => 2 } case int y)`) —
            // that root belongs to the switch, not the if-case. The
            // stale clause anchor must not join the if-case's candidates
            // either (the switch-expression case's posture).
            if engine.switch.pattern_stack.is_empty() && !engine.switch.case_sink_open {
                let scrutinee = engine.rows.last().map_or(Ty::Unknown, |row| row.ty);
                engine.switch.open_if_case(scrutinee, token.range);
                engine.clause_anchor = None;
            }
        }
        let binary = std::mem::take(&mut engine.pending_binary_operand);
        engine.switch.pattern_stack.push(PatternFrame {
            leaves: Vec::new(),
            or_ops: 0,
            fields: Vec::new(),
            binary,
            wildcards: Vec::new(),
        });
    }

    fn end_pattern(&mut self, token: Token) {
        self.flow().end_pattern(token);

        let engine = &mut self.engine;
        engine.switch.pattern_depth = engine.switch.pattern_depth.saturating_sub(1);
        let Some(frame) = engine.switch.pattern_stack.pop() else {
            return;
        };
        let merged = SwitchTracker::collapse(frame.leaves, frame.or_ops);
        if engine.switch.pattern_stack.is_empty() {
            // A root pattern: only an open case header's coverage feeds the
            // exhaustiveness walk. An if-case root closes its own window
            // uncaptured (an if-case is no coverage question — the
            // stale-root pins); pattern assignments and other bare roots
            // drop here, so debris can neither leak into a later case nor
            // grow the set.
            if engine.switch.if_case_window {
                engine.switch.close_if_case();
            } else if engine.switch.case_sink_open {
                engine.switch.pattern_roots.push(merged);
            }
        } else if let Some(parent) = engine.switch.pattern_stack.last_mut() {
            parent.leaves.push(merged);
        }
    }

    fn begin_binary_pattern(&mut self, operator: Token) {
        self.flow().begin_binary_pattern(operator);
        let engine = &mut self.engine;
        // The operand pattern parses next; its bracket frame inherits the
        // binary context (the scrutinee stays the matched type).
        let is_or = engine.text_of(operator) == Some("||");
        if !is_or {
            // The left operand already parsed (its wildcard events wait on
            // this operator): the logical-and face decides them now.
            engine.check_pending_wildcards();
        }
        engine.pending_binary_operand = Some(is_or);
        if is_or && let Some(frame) = engine.switch.pattern_stack.last_mut() {
            frame.or_ops += 1;
        }
    }

    fn end_binary_pattern(&mut self, operator: Token) {
        self.flow().end_binary_pattern(operator);
    }

    fn handle_cast_pattern(&mut self, operator: Token) {
        let engine = &mut self.engine;
        // `p as X` covers X's values (official `castPatternToSpace`).
        let leaf = match engine.armed_ty {
            Some(Ty::Named { name, nullable, .. }) => Restriction::Named(name, nullable),
            _ => Restriction::Unknown,
        };
        // The still-armed type is the cast target: the pair at
        // shared_type_analyzer.dart:129-152 (and the Cast kind of
        // `checkPatternNeverMatchesValueType`).
        let cast_ty = engine.armed_ty;
        engine.check_cast_pattern(operator.range, cast_ty);
        // `visitCastPattern`'s face (best_practices_verifier.dart:227-239):
        // the matched-value proxy is the same scrutinee pair.
        engine.check_cast_pattern_null(operator.range, cast_ty);
        engine.pattern_leaf(leaf);
        // The armed type was the cast pattern's — never a declaration's.
        engine.armed_ty = None;
        engine.armed_ty_span = None;
        engine.pattern_bind_dirty = true;
    }

    fn handle_null_assert_pattern(&mut self, bang: Token) {
        // `p!` promotes the matched arm non-null on the matched path.
        self.engine.pattern_bind_dirty = true;
        let engine = &mut self.engine;
        engine.check_null_pattern(bang.range, true);
        // `p!` = the subpattern space ∩ nonNull, unioned with `null`
        // (official `createNullAssertSpace`, upstream Dart SDK source,
        // pkg/_fe_analyzer_shared/lib/src/exhaustiveness/shared.dart:790-797):
        // the null side stays covered, so every case after a `p!` over a
        // nullable scrutinee is unreachable.
        let leaf = match NameTy::from_semantic(
            engine
                .switch
                .switches
                .last()
                .map_or(Ty::Unknown, |c| c.scrutinee),
        ) {
            NameTy::Named(name, _) => {
                Restriction::Union(vec![Restriction::Named(name, false), Restriction::Null])
            }
            _ => Restriction::Unknown,
        };
        engine.pattern_leaf(leaf);
    }

    fn handle_null_check_pattern(&mut self, question: Token) {
        // `p?` can fail on the null path: the flow recorder promotes the
        // matched arm non-null and joins the null arm into the unmatched
        // chain (official `_handleNullCheckPattern`).
        self.flow().handle_null_check_pattern(question);
        // Only a null-check directly wrapping a latched variable (the
        // shorthand `:final x?`) carries the non-null projection to the
        // field boundary; a wrapper on a completed field leaves no live
        // latch, and a stale bit must not leak into the next field.
        self.engine.pending_pattern_null_check =
            self.engine.last_pattern_variable.map(|(name, _)| name);
        // `p?` = the subpattern space ∩ nonNull (official
        // `createNullCheckSpace`, upstream Dart SDK source,
        // pkg/_fe_analyzer_shared/lib/src/exhaustiveness/shared.dart:780-786;
        // `_createStaticTypeWithContext` applies `.nonNullable` to the
        // context type in both nullable and non-nullable contexts): null is
        // NOT covered, so a later `case null:` stays live and this arm
        // alone never exhausts the switch.
        let engine = &mut self.engine;
        engine.pattern_bind_dirty = true;
        engine.check_null_pattern(question.range, false);
        let leaf = match NameTy::from_semantic(
            engine
                .switch
                .switches
                .last()
                .map_or(Ty::Unknown, |c| c.scrutinee),
        ) {
            NameTy::Named(name, _) => Restriction::Named(name, false),
            _ => Restriction::Unknown,
        };
        engine.pattern_leaf(leaf);
    }

    fn handle_relational_pattern(&mut self, operator: Token) {
        // The shape's failure arm joins the flow's unmatched chain — a
        // top-level `case == 5`/`case > 0` can fail, so the code after an
        // if-case stays live (flow_analysis.dart's pattern dispatch).
        self.flow().handle_relational_pattern(operator);
        // Value comparisons are not representable at name level.
        self.engine.pattern_leaf(Restriction::Unknown);
        self.engine.pattern_bind_dirty = true;
    }

    fn handle_list_pattern(&mut self, count: usize, left: Token, right: Token) {
        self.flow().handle_list_pattern(count, left, right);
        // The list face arrives through keysOfInterest/additional properties
        // — not modeled at name level (documented under-report).
        self.engine.pattern_leaf(Restriction::Unknown);
        self.engine.pattern_bind_dirty = true;
    }

    fn handle_map_pattern(&mut self, count: usize, left: Token, right: Token) {
        self.flow().handle_map_pattern(count, left, right);
        self.engine.pattern_leaf(Restriction::Unknown);
        self.engine.pattern_bind_dirty = true;
    }

    fn begin_map_pattern_key(&mut self) {
        // `visitMapPattern`'s per-entry `_evaluateAndReportError`
        // (constant_verifier.dart:342-358): each `key:` expression is a
        // constant context. Open the capture window before the parser reads
        // the key so its nodes accumulate.
        self.engine.const_capture.open_map_pattern_key();
    }

    fn handle_map_pattern_entry(&mut self, colon: Token, _end: Token) {
        // Close the key's window and evaluate it: a non-constant key reports
        // `non_constant_map_pattern_key` at the key's own span. Unlike the
        // constant-pattern face there is no InvalidType early-return — the
        // oracle reports the row even beside a resolution miss (`{id: n}` on
        // an undefined `id` carries both `undefined_identifier` and
        // `non_constant_map_pattern_key`), so no suppression probe here.
        if let Some(node) = self.engine.const_capture.close_map_pattern_key() {
            match eval(&node, &self.engine) {
                Verdict::Value(_) | Verdict::Constant => {}
                Verdict::NotConstant(span)
                | Verdict::Unresolved(span)
                | Verdict::Fault(_, span) => {
                    let span = if span.length > 0 { span } else { colon.range };
                    self.engine
                        .emit_corrected(SemanticCode::NonConstantMapPatternKey, span, "");
                }
            }
        }
    }

    fn handle_record_pattern(&mut self, left_paren: Token, count: usize) {
        // The record shape test can fail — the flow recorder joins the
        // failure arm into the unmatched chain so the statement after an
        // if-case stays live.
        self.flow().handle_record_pattern(left_paren, count);
        // Record shapes live in the interned lattice (`dartq-types`); the
        // name level has no record type, so the row is unknown-shaped and
        // the engine under-reports.
        self.engine.pattern_leaf(Restriction::Unknown);
        // One close per record pattern in the declaration window — the
        // flat `final (a, b) = pair()` binds read exactly one; nested
        // records close more and gate the destructure bind off.
        self.engine.pattern_bind_records = self.engine.pattern_bind_records.saturating_add(1);
    }

    fn begin_constant_pattern(&mut self, const_keyword: Option<Token>) {
        self.flow().begin_constant_pattern(const_keyword);

        let engine = &mut self.engine;
        engine.switch.pending_const = None;
        // Stale literal armings from sibling list/map elements die here.
        engine.const_fold.clear();
        // The const-ness face: `visitConstantPattern` evaluates the
        // pattern's expression (constant_verifier.dart:128-142) — a bare or
        // `const`-prefixed pattern alike, inside or outside a switch
        // (`if (x case e)` runs the same visit). The window's close eval
        // reports `constant_pattern_with_non_constant_expression` on a
        // generic miss, an inner `constEval*` fault on its own span.
        engine.const_capture.open_constant_pattern();
    }

    fn end_constant_pattern(&mut self, const_keyword: Option<Token>) {
        self.flow().end_constant_pattern(const_keyword);

        let engine = &mut self.engine;
        let leaf = engine
            .switch
            .pending_const
            .take()
            .unwrap_or_else(|| engine.classify_constant_tail());
        // The literal-armed type and anchor, else the enum-tail
        // classification's (its constant's type is its enum;
        // `visitConstantPattern`'s `_canBeEqual` inputs,
        // constant_verifier.dart:140-165), else the class-static-const
        // tail's folded value type (#2468 — the same faces, the class
        // container answers from the member's captured constant value).
        let constant = engine.const_fold.take_ty().or_else(|| match leaf {
            Restriction::EnumValue(enum_name, _) => Some(Ty::named(enum_name)),
            _ => engine.class_static_const_tail_ty(),
        });
        // The capture closes first so the never-match anchor is the FULL
        // constant node: upstream anchors `constantPatternNeverMatchesValueType`
        // at the ConstantPattern node itself (upstream Dart SDK source,
        // pkg/analyzer/lib/src/dart/constant/constant_verifier.dart,
        // visitConstantPattern's `.at(node)`) — `A.a` whole, not the inner
        // identifier the resolution rows carry nor the member tail. The
        // capture root spans the parsed expression (a member access spans
        // receiver-through-selector; a literal spans its token), so the
        // literal forms keep their old anchors; the recovery fallbacks
        // below hold the pre-capture chain for windows that never opened.
        let capture = engine.const_capture.close_constant_pattern();
        let mut at = capture
            .as_ref()
            .map(|node| node.span)
            .or_else(|| engine.const_fold.take_at())
            .or_else(|| engine.rows.last().map(|row| row.range));
        // The `const`-prefixed spelling anchors at the keyword: the
        // ConstantPattern node spans `const` through the expression
        // (upstream Dart SDK source, pkg/analyzer/lib/src/dart/constant/
        // constant_verifier.dart, visitConstantPattern's `.at(node)` —
        // `case const A.a:` reports at the keyword column), while the
        // capture root only spans the parsed expression after it.
        if let (Some(kw), Some(span)) = (const_keyword, at.as_mut())
            && kw.range.offset < span.offset
        {
            span.length += span.offset - kw.range.offset;
            span.offset = kw.range.offset;
        }
        engine.check_constant_pattern(constant, at);
        // `visitConstantPattern`'s `_evaluateAndReportError`: the
        // expression's own verdict — a fault overrides the site default,
        // a generic miss reports `constant_pattern_with_non_constant_expression`
        // at the failing node's span. `Value`/`Constant` (and a `null` root
        // — a window that never opened, e.g. a recovery path) stay silent;
        // the oracle's `InvalidType` early-return maps to the same silence.
        if let Some(node) = capture {
            match eval(&node, engine) {
                // An unresolved reference is the oracle's `InvalidType`
                // early-return (constant_verifier.dart:134-136): the
                // resolution row (`undefined_prefixed_name`, ...) answers.
                Verdict::Value(_) | Verdict::Constant | Verdict::Unresolved(_) => {}
                Verdict::NotConstant(span) => {
                    // The oracle's `InvalidType` early-return
                    // (constant_verifier.dart:134-136): when resolution
                    // already answered inside the expression's span — an
                    // `invocation_of_non_function_expression` on `super()`,
                    // an `undefined_*` miss on `a + b` — the evaluator is
                    // never consulted and the generic row does not fire.
                    // A miss covering the whole expression also answers:
                    // `case as:`'s `undefined_identifier 'as'` IS the node's
                    // span. Pending misses count too — the deferred pass-2
                    // rows have not emitted yet, but their resolution already
                    // failed inside the node.
                    let inside = |r: SourceRange| {
                        r.offset >= span.offset && r.end() <= span.end() && r.length > 0
                    };
                    // The suppression's anchor exclusion mirrors the
                    // case-expression face's (`!= span`): a diagnostic that
                    // IS the expression's whole span is the verifier/parser's
                    // own verdict on the node — `invalid_reference_to_this`
                    // on `case this:` (a reference-validity face, not a
                    // resolution `InvalidType`), `invalid_constant_pattern_binary`
                    // — not a failed resolution. The oracle reports the
                    // generic row beside it (`constant_pattern_with_non_constant_expression`
                    // at `this`, constant_verifier.dart's `_evaluateAndReportError`
                    // runs regardless). Resolution misses that already emitted
                    // inside the span still suppress — their rows carry the
                    // `InvalidType` the early-return reads.
                    let miss_row = |row: &Row| {
                        inside(row.range)
                            && matches!(
                                row.shape,
                                RowShape::Bare(BareInfo::Missing { .. }) | RowShape::TypeUse(..)
                            )
                    };
                    let already_answered = engine
                        .report
                        .diagnostics
                        .iter()
                        .any(|d| inside(d.range) && d.range != span)
                        || engine.deferred_miss_rows.iter().any(&miss_row)
                        || engine.rows.iter().any(&miss_row);
                    if !already_answered {
                        engine.emit_corrected(
                            SemanticCode::ConstantPatternWithNonConstantExpression,
                            span,
                            "",
                        );
                    }
                }
                Verdict::Fault(fault, span) => engine.emit_const_fault(fault, span),
            }
        }
        engine.pattern_leaf(leaf);
    }

    /// Closes an object pattern. The parser fires this event *after* the
    /// rest — the fields parsed and their `handle_pattern_field` boundaries
    /// drained this pattern's own bracket frame — so the head name and the
    /// drained buckets meet here: the row is the bare head type exactly
    /// when every field's restriction covers its member's whole column.
    fn handle_object_pattern(
        &mut self,
        first_identifier: Token,
        dot: Option<Token>,
        second_identifier: Option<Token>,
    ) {
        // The flow face (#2122): the recorded `ObjectPattern` event drives
        // the flow-side `matched_object_pattern` promotion (official
        // `promoteForPattern`, flow_analysis.dart:7703-7755, fed by
        // `analyzeObjectPattern`) — the matched arm is the state the
        // clause's guard and body run on, so `case Some(): option.value`
        // resolves on `Some`, not the declared `Option`.
        self.flow()
            .handle_object_pattern(first_identifier, dot, second_identifier);

        let engine = &mut self.engine;
        // `Type(...)` shapes: the first identifier is the type when no dot
        // follows; the `Type.field`/`:field` shorthands stay unmodeled.
        let head = if dot.is_none() {
            engine.name_of(first_identifier)
        } else {
            None
        };
        // The head is a NamedType the parser reports through this event
        // instead of the `handle_identifier(TypeReference)` + `handle_type`
        // pair — feed the plain type-use pipeline so its miss resolves
        // through `resolve_type_row`'s `undefined_class` arm
        // (named_type_resolver.dart `reportNullOrNonTypeElement`'s
        // fallthrough; upstream Dart SDK source, pkg/analyzer/lib/src/dart/
        // resolver/named_type_resolver.dart). No `arm_type_head`: the
        // head's type-argument group already closed inside the pattern's
        // own parse. The `Some(head)` check doubles as the oracle's
        // synthetic-name gate — `first_identifier` is a real stream token
        // (`is_identifier` peek), never a recovery insert.
        if let Some(head) = head {
            engine.push_type_use(head, first_identifier.range, TypeHead::Bare);
        }
        // The pattern head is a `NamedType` reference (the oracle resolves
        // `ObjectPattern.type` through `visitNamedType` →
        // `_useIdentifierElement`): a resolved head counts as a name use —
        // `import_used` unconditionally (the scope lookup itself spares
        // the providing import), `used_names` under the self-scope
        // exclusion the element-use face keeps. `p.T(...)`: the first
        // identifier is the prefix — its live use marks the prefix, and
        // `second_identifier` carries the imported type's own credit.
        if let Some(head) = head {
            engine.import_used.insert(head);
            if !engine.in_own_named_scope(head) {
                engine.used_names.insert(head);
            }
        } else if dot.is_some()
            && let (Some(first), Some(second)) = (
                engine.name_of(first_identifier),
                second_identifier.and_then(|token| engine.name_of(token)),
            )
        {
            let (_, target) = engine.resolve_bare(first);
            if matches!(target, RefTarget::Prefix) {
                engine.used_names.insert(first);
                engine.import_used.insert(second);
                if !engine.in_own_named_scope(second) {
                    engine.used_names.insert(second);
                }
            } else {
                // `Type.selector(...)`: the head type is the first
                // identifier — its bare reference carries the credits.
                engine.import_used.insert(first);
                if !engine.in_own_named_scope(first) {
                    engine.used_names.insert(first);
                }
            }
        }
        // The Object kind of `checkPatternNeverMatchesValueType`
        // (type_analyzer.dart:723-756): the head type is the pattern's
        // required type.
        if let Some(head) = head {
            let required = Ty::named(head);
            engine.check_pattern_never_matches(first_identifier.range, required);
        }
        let mut staged_binders: Vec<(Ty, SourceRange)> = Vec::new();
        let fields = engine
            .switch
            .pattern_stack
            .last_mut()
            .map_or(Vec::new(), |frame| std::mem::take(&mut frame.fields));
        if let Some(head) = head {
            for field in &fields {
                if let Some((getter, at)) = field.getter {
                    engine.pending_pattern_getters.push((head, getter, at));
                }
                // The shorthand binder's declared type: `:var x` reads the
                // getter on the head, and the oracle's
                // `resolveObjectPatternPropertyGet` types the bound
                // variable from the property's declared return
                // (`GetterInvocationResolution(:var element)` binds
                // `GetterElement`, never the raw unknown) — the binder's
                // downstream member reads and case-guard promotions decide
                // on the real face. A bare named return only: a parametric
                // or unresolvable face keeps the under-report (the
                // boundary's argument projection is not engaged here).
                // A typed sub-pattern (`:T x`) keeps its written annotation
                // (only a still-unknown slot takes the fill), and a
                // null-check suffix (`:final x?`) projects the face non-null
                // (the arm's promotion applies to the bound variable).
                if let (Some((binder, binder_span)), Some((getter, _))) =
                    (field.binder, field.getter)
                    && engine
                        .local_declared_ty(binder)
                        .map_or(true, |slot| slot == Ty::UNKNOWN)
                    && let Some(mut ty) = engine.object_pattern_binder_ty(head, getter)
                {
                    if field.binder_non_null && let Ty::Named { nullable, .. } = &mut ty {
                        *nullable = false;
                    }
                    engine.bind_var_type(binder, ty.clone());
                    staged_binders.push((ty, binder_span));
                }
            }
        }
        engine.close_object_fields(head, &fields);
        // An object pattern's fields are member reads, not record slots.
        engine.pattern_bind_dirty = true;
        // The bound variables' flow slots take the same declared face
        // (the #2140 staging, keyed on the binder's name token): the
        // flow's own `DeclaredVarPattern` typed the raw unknown.
        for (ty, span) in staged_binders {
            if let Ty::Named {
                name: ty_name,
                nullable,
                ..
            } = ty
            {
                let lexeme = self.engine.strings.resolve(ty_name).to_string();
                let base = self.flow().intern_name(&lexeme);
                self.flow().stage_inferred_local(
                    span.offset,
                    FlowTy {
                        base: FlowBase::Name(base),
                        nullable,
                    },
                    Vec::new(),
                );
            }
        }
    }

    fn handle_pattern_field(&mut self, colon: Option<Token>) {
        let engine = &mut self.engine;
        // The field's sub-pattern just completed: its bracket merged into
        // this pattern's own frame, so everything buffered since the last
        // boundary is exactly this field's sub-restrictions. A type-free
        // sub-pattern (`var x`, `_`) merges to the context space — it
        // constrains no column, so it pools as no leaf. Record patterns
        // fire the same boundary on their own frames; their buckets have
        // no consumer and die with the bracket.
        let name = colon.and_then(|token| engine.name_of(token));
        // The getter the field reads: the written name, else the
        // shorthand's variable (`:var x` — the colon is the name-less
        // `handle_no_name` shape, so the variable latch answers). The
        // latch is consumed per field so a positional field never reads a
        // stale one.
        let latched = engine.last_pattern_variable.take();
        let mut binder = None;
        let mut binder_non_null = false;
        let getter = match (colon, name) {
            (Some(token), Some(name)) if engine.text_of(token) != Some(":") => {
                Some((name, token.range))
            }
            (Some(_), _) => {
                // The shorthand's variable IS the bound pattern variable —
                // its declared type rides the getter's return below. The
                // pending null-check projects the face non-null only when
                // it wrapped THIS field's variable.
                binder = latched;
                binder_non_null = engine
                    .pending_pattern_null_check
                    .is_some_and(|checked| binder.is_some_and(|(name, _)| name == checked));
                latched
            }
            (None, _) => None,
        };
        // A named field (`:a` / `a: p`) is never a positional record slot —
        // the flat destructure bind requires an all-positional window.
        if colon.is_some() {
            engine.pattern_bind_dirty = true;
        }
        let mut leaves = engine
            .switch
            .pattern_stack
            .last_mut()
            .map_or(Vec::new(), |frame| std::mem::take(&mut frame.leaves));
        leaves.retain(|leaf| *leaf != Restriction::NamedContext);
        if let Some(frame) = engine.switch.pattern_stack.last_mut() {
            frame.fields.push(ObjectField {
                name: getter.map(|(name, _)| name).or(name),
                leaves,
                getter,
                binder,
                binder_non_null: {
                    engine.pending_pattern_null_check.take();
                    binder_non_null
                },
            });
        }
    }

    fn handle_declared_variable_pattern(
        &mut self,
        keyword: Option<Token>,
        variable: Token,
        in_assignment_pattern: bool,
    ) {
        self.flow()
            .handle_declared_variable_pattern(keyword, variable, in_assignment_pattern);

        let engine = &mut self.engine;
        // The bound variable joins the enclosing scope (the flat M1 frame;
        // the oracle's pattern scoping is finer, and a wider binding can
        // only under-report undefined-name, never false-positive). The
        // usage cluster tracks it as a pattern variable: the enclosing
        // `PatternVariableDeclaration` group reports all-or-nothing.
        if let Some(name) = engine.name_of(variable) {
            engine.last_pattern_variable = Some((name, variable.range));
            engine.note_pattern_shadow(name);
            engine.add_tracked_local(name, variable.range, LocalKind::PatternVariable);
            // The DeclaredVariable kind of `checkPatternNeverMatchesValueType`
            // (type_analyzer.dart:723-756) reads the still-armed declared
            // type before the binding consumes it, anchored at
            // `pattern.type` — the written type's span (probed 3.13.1:
            // `IntAlias` at 6:14, not the bound variable).
            let required = engine.armed_ty;
            if let Some(required) = required {
                let at = engine.armed_ty_span.unwrap_or(variable.range);
                engine.check_pattern_never_matches(at, required);
            }
            // The bound declared type folds through a type alias first —
            // the pattern variable's frame type is the resolved interface
            // type ([`Self::bind_declared_type`]'s contract), and the
            // space leaf reads the same resolved head.
            engine.armed_ty_span = None;
            match engine.armed_ty.take().map(|ty| engine.fold_alias_ty(ty)) {
                Some(Ty::Named {
                    name: ty_name,
                    nullable,
                    ..
                }) => {
                    engine.bind_var_type(
                        name,
                        Ty::Named {
                            name: ty_name,
                            nullable,
                            args: ArgListId::EMPTY,
                        },
                    );
                    engine.pattern_leaf(Restriction::Named(ty_name, nullable));
                }
                ty => {
                    if let Some(ty) = ty {
                        engine.bind_var_type(name, ty);
                    } else if !in_assignment_pattern {
                        // A bare pattern variable (`(a, b)`'s `a`): its type
                        // is the scrutinee record's slot — recorded for the
                        // declaration-statement bind ([`Self::
                        // destructure_record`]). Assignment patterns rebind
                        // existing locals and never enter the list.
                        engine.pattern_bind_vars.push(name);
                    }
                    // A type-free variable covers its context: no leaf.
                }
            }
        } else {
            engine.armed_ty = None;
            engine.armed_ty_span = None;
        }
    }

    fn handle_wildcard_pattern(&mut self, keyword: Option<Token>, wildcard: Token) {
        self.flow().handle_wildcard_pattern(keyword, wildcard);

        // A typed wildcard (`M _`) covers the declared type's space (the
        // official wildcard converts through its type); a bare `_` (or
        // `var _`) covers the context: no leaf.
        let engine = &mut self.engine;
        // The Wildcard kind of `checkPatternNeverMatchesValueType`
        // (type_analyzer.dart:723-756) reads the still-armed declared
        // type, anchored at `pattern.type` — the written type's span.
        if let Some(required) = engine.armed_ty {
            let at = engine.armed_ty_span.unwrap_or(wildcard.range);
            engine.check_pattern_never_matches(at, required);
        }
        let declared = engine.armed_ty;
        engine.armed_ty_span = None;
        engine.check_wildcard_pattern(wildcard.range, declared);
        let Some(Ty::Named { name, nullable, .. }) = engine.armed_ty.take() else {
            return;
        };
        engine.pattern_leaf(Restriction::Named(name, nullable));
    }

    // —— switch statements ————————————————————————————————————————————

    fn begin_switch_block(&mut self, token: Token) {
        self.flow().begin_switch_block(token);

        let kw = self
            .engine
            .switch
            .switch_kw_latch
            .take()
            .unwrap_or(token.range);
        self.engine.open_switch(false, kw);
    }

    /// Latches the `switch` keyword: the non-exhaustive anchor (the block
    /// event that follows only carries the `{`).
    fn begin_switch_statement(&mut self, token: Token) {
        self.flow().begin_switch_statement(token);

        self.engine.switch.switch_kw_latch = Some(token.range);
    }

    fn end_switch_block(&mut self, count: usize, begin: Token, end: Token) {
        self.flow().end_switch_block(count, begin, end);

        self.engine.report_switch();
    }

    fn begin_switch_case_when_clause(&mut self, _token: Token) {}

    /// The case header's pattern window opens before its pattern parses;
    /// the root it produces is captured at [`Self::end_case_expression`].
    /// A legacy (`< 3.0`, patterns disabled) toolchain parses the head as a
    /// plain expression instead — its const requirement rides the constant
    /// window, evaluated at the same close.
    fn begin_case_expression(&mut self, case_kw: Token) {
        self.flow().begin_case_expression(case_kw);

        let engine = &mut self.engine;
        engine.switch.case_sink_open = true;
        engine.switch_member_head_begin();
        // The clause anchor latches at the HEAD's opening, before its
        // pattern parses: a constant-pattern candidate inside the head
        // defers to the teardown join keyed on this anchor (#1351), and
        // `begin_switch_case` fires only at the case body — after every
        // head in the group has already been checked (the parser's
        // `parse_statements_in_switch_case` runs past the head loop).
        // The token is the same `case` keyword the flow's `MatchedType`
        // fact anchors at (`end_case_expression`'s `case_kw`).
        engine.clause_anchor = Some(case_kw.range);
        if !engine.patterns_enabled {
            engine.const_capture.open_constant_pattern();
        }
    }

    /// One statement-case header completed: its pattern's coverage latches
    /// here with the guard flag and the `case`-keyword anchor (the oracle's
    /// `caseNode.keyword`, constant_verifier.dart:1076-1085). Shared-body
    /// groups (`case A: case B:`) capture one case per header. The legacy
    /// expression head evaluates here: a generic miss reports
    /// `non_constant_case_expression` at the failing node's span.
    fn end_case_expression(&mut self, case_kw: Token, when: Option<Token>, colon: Token) {
        self.flow().end_case_expression(case_kw, when, colon);

        let engine = &mut self.engine;
        if engine.const_capture.top_is_constant_pattern()
            && let Some(node) = engine.const_capture.close_constant_pattern()
        {
            match eval(&node, engine) {
                Verdict::Value(_) | Verdict::Constant => {}
                Verdict::NotConstant(span) | Verdict::Unresolved(span) => {
                    // The same `InvalidType` early-return shape as the
                    // constant-pattern face: a resolution verdict already
                    // anchored inside the expression's span suppresses the
                    // generic row (constant_verifier.dart:134-136).
                    let already_answered = engine.report.diagnostics.iter().any(|d| {
                        d.range.offset >= span.offset
                            && d.range.end() <= span.end()
                            && d.range.length > 0
                            && d.range != span
                    });
                    if !already_answered {
                        engine.emit(SemanticCode::NonConstantCaseExpression, span, "", "");
                    }
                }
                Verdict::Fault(fault, span) => engine.emit_const_fault(fault, span),
            }
        }
        let root = engine.close_case_sink();
        engine.switch_member_head_done();
        let space = engine.substitute_root(root);
        if let Some(ctx) = engine.switch.switches.last_mut() {
            ctx.cases.push(NameCase {
                space,
                guarded: when.is_some(),
                anchor: case_kw.range,
                flow_anchor: case_kw.range,
                head_colon: Some(colon.range),
                body: None,
            });
        }
    }

    fn end_switch_case(
        &mut self,
        label_count: usize,
        expression_count: usize,
        default_kw: Option<Token>,
        colon_after_default: Option<Token>,
        statement_count: usize,
        begin: Token,
        end: Token,
    ) {
        self.flow().end_switch_case(
            label_count,
            expression_count,
            default_kw,
            colon_after_default,
            statement_count,
            begin,
            end,
        );

        // The member's duplicate scope closes with the member (popped
        // before every early return below).
        self.engine.dup_scopes.pop();

        // The case headers already captured their spaces
        // ([`Self::end_case_expression`]); the body-level event latches
        // `default`, closes any window a broken header left open, and
        // anchors the member body span onto the member's last head (the
        // flow-dead member rows ride it). The span runs from the head's
        // `:` to the member's end: a `default` head reports its colon here
        // (`colon_after_default` — the parser opens no case window for
        // it), a case head's colon latched on the case itself.
        let engine = &mut self.engine;
        engine.switch.case_sink_open = false;
        engine.switch_member_end();
        let body_start = if expression_count > 0 {
            engine
                .switch
                .switches
                .last()
                .and_then(|ctx| ctx.cases.last())
                .and_then(|case| case.head_colon)
        } else {
            colon_after_default.map(|colon| colon.range)
        };
        let body = match (body_start, statement_count) {
            (Some(colon), count) if count > 0 => engine.trimmed_span(colon.end(), end.range.offset),
            _ => None,
        };
        let Some(ctx) = engine.switch.switches.last_mut() else {
            return;
        };
        if let Some(token) = default_kw {
            ctx.default_kw = Some(token.range);
            if ctx.default_pos.is_none() {
                ctx.default_pos = Some(ctx.cases.len());
            }
        }
        // Only a case-headed member owns the last captured case; a
        // `default` member pushes no case, and its body must not ride the
        // previous member's head.
        if expression_count > 0
            && let Some(last) = ctx.cases.last_mut()
        {
            last.body = body;
        }
    }

    // —— switch expressions ————————————————————————————————————————————

    fn begin_switch_expression_block(&mut self, token: Token) {
        self.flow().begin_switch_expression_block(token);

        let kw = self
            .engine
            .switch
            .switch_kw_latch
            .take()
            .unwrap_or(token.range);
        self.engine.open_switch(true, kw);
    }

    /// Latches the `switch` keyword of a switch expression.
    fn begin_switch_expression(&mut self, token: Token) {
        self.flow().begin_switch_expression(token);

        let engine = &mut self.engine;
        engine.nsm_poison();
        engine.switch.switch_kw_latch = Some(token.range);
    }

    /// The expression-case header's pattern window opens before its
    /// pattern parses.
    fn begin_switch_expression_case(&mut self) {
        self.flow().begin_switch_expression_case();

        let engine = &mut self.engine;
        engine.switch.case_sink_open = true;
        engine.switch_member_head_begin();
        // The parser delivers no keyword face for an alternative (the
        // flow fact anchors at its first token); the stale clause anchor
        // must not join a foreign clause — the alternatives' candidates
        // under-report.
        engine.clause_anchor = None;
    }

    /// The expression case's pattern completed: its coverage latches now
    /// (the guard and the `=>` anchor only exist at the case's end).
    fn handle_switch_expression_case_pattern(&mut self, token: Token) {
        self.flow().handle_switch_expression_case_pattern(token);

        let engine = &mut self.engine;
        let root = engine.close_case_sink();
        let space = engine.substitute_root(root);
        if let Some(ctx) = engine.switch.switches.last_mut() {
            ctx.pending_root = Some(space);
        }
    }

    fn end_switch_expression_case(
        &mut self,
        begin: Token,
        when: Option<Token>,
        arrow: Token,
        end: Token,
    ) {
        self.flow()
            .end_switch_expression_case(begin, when, arrow, end);

        let engine = &mut self.engine;
        engine.switch_member_end();
        // `SwitchExpression` is in the official used set.
        engine.ur_seal_used();
        let space = engine
            .switch
            .switches
            .last_mut()
            .and_then(|ctx| ctx.pending_root.take())
            .unwrap_or(Restriction::Unknown);
        if let Some(ctx) = engine.switch.switches.last_mut() {
            ctx.cases.push(NameCase {
                space,
                guarded: when.is_some(),
                anchor: arrow.range,
                flow_anchor: begin.range,
                head_colon: None,
                body: None,
            });
        }
    }

    fn end_switch_expression_block(&mut self, count: usize, begin: Token, end: Token) {
        self.flow().end_switch_expression_block(count, begin, end);

        self.engine.report_switch();
        // The zero-case recovery face: a switch expression recovered with
        // no case bodies folds to the oracle's InvalidType (nothing to
        // fold), so the enclosing `=>` return check asks no question of
        // the expression. The poisoned value row also stops the
        // scrutinee's own row from being the body's single row — the
        // check would otherwise read the scrutinee's type and report
        // against it (upstream Dart SDK source, pkg/analyzer/lib/src/
        // dart/ast/ast.dart `SwitchExpressionImpl.staticType` — the arm
        // fold over zero arms is InvalidType).
        if count == 0 {
            self.engine.push_value_row(begin);
        }
    }

    // —— if-case ————————————————————————————————————————————————————————

    fn handle_parenthesized_condition(
        &mut self,
        _begin: Token,
        case_kw: Option<Token>,
        _when: Option<Token>,
    ) {
        // An if-case's pattern events already ran (the binding face works
        // through `handle_declared_variable_pattern`): `begin_pattern`
        // opened the pattern's one-case window at the root and its
        // `end_pattern` closed it. This event stays passive — the window
        // never spans the guard, and the root is never captured.
        let _ = case_kw;
    }

    fn handle_enum_element(&mut self, _begin: Token, _augment: Option<Token>) {
        // The parser fires this after the constant's type arguments and
        // constructor arguments: the constant's window closed (it opened at
        // the EnumValueDeclaration identifier).
        let engine = &mut self.engine;
        engine.close_enum_const_element();
        engine.leave_enum_element();
    }

    fn handle_enum_elements(&mut self, _elements_end: Token, count: usize) {
        let engine = &mut self.engine;
        engine.close_enum_const_element();
        engine.leave_enum_element();
        // The empty-element face: a non-augmentation enum declaration
        // without constants reports `enum_without_constants` at its name
        // (upstream Dart SDK source, pkg/analyzer/lib/src/error/
        // error_verifier.dart `checkEnumDeclaration` — `!isAugmentation &&
        // constants.isEmpty` at `node.namePart.typeName`).
        if let Some((name_range, is_augment)) = engine.enum_decl_site.take()
            && count == 0
            && !is_augment
        {
            engine.emit_corrected(SemanticCode::EnumWithoutConstants, name_range, "");
        }
    }

    fn begin_type_arguments(&mut self, token: Token) {
        // The flow face (#2125): the recorder opens its argument group
        // here — the trio with `end_type_arguments` /
        // `handle_no_type_arguments` latches the head row's written
        // arguments, and the missing forwards left every written
        // `List<int>` dead as `EMPTY_ARGS` in listener-driven runs.
        self.flow().begin_type_arguments(token);

        let engine = &mut self.engine;
        let head = engine.pending_type_head.take();
        // The written-argument capture: a headless group is an invocation's
        // explicit type arguments (`f<int>(0)`, `C<num>(0)`); its members
        // feed the argument check's grounding.
        engine.call_args_begin(head.is_some());
        // The #610 capture: a type-reference group outside the capture
        // windows opens the site capture (a headless group is never a
        // site — bounds requires the head) — except an enum constant's
        // type-argument group (`v<T, U>()`), whose headless group applies
        // to the enclosing enum's parameters: the official
        // `checkEnumConstantDeclaration` walks
        // `constructorElement.returnType.typeArguments` against the
        // enum's own bounds (upstream Dart SDK source,
        // pkg/analyzer/lib/src/error/type_arguments_verifier.dart
        // :176-202), so the site capture opens at the enum's face. The
        // `group_heads` entry stays headless — the call-argument latch
        // and the count check keep their own paths.
        let site_head =
            if head.is_none() && engine.in_enum_element.is_some() && engine.arg_levels.is_empty() {
                engine
                    .container_context()
                    .filter(|(_, kind)| *kind == ContainerKind::Enum)
                    .map(|(name, _)| (name, token.range))
                    .or(head)
            } else {
                head
            };
        engine.bounds.begin_group(site_head, true);
        // The #1351 capture: the same group nests into the open typedef
        // RHS (a no-op outside one).
        engine.bounds.alias_begin_group(head);
        // The #612 capture: the same group nests into the open header.
        engine.var_begin_group(head);
        // The clause-entry capture: the group's members are argument
        // positions, not entries.
        engine.note_header_type_args_open();
        // #1314 FAM13: the group's member references are type arguments.
        engine.type_arg_depth += 1;
        let base = engine.rows.len();
        engine.group_heads.push((head, base));
    }

    fn end_type_arguments(&mut self, count: usize, begin: Token, end: Token) {
        // The flow face (#2125): the group's interned list latches as the
        // pending arguments of the head row that follows (see
        // `begin_type_arguments`).
        self.flow().end_type_arguments(count, begin, end);

        {
            let engine = &mut self.engine;
            // #1314 FAM13: the group closed.
            engine.type_arg_depth = engine.type_arg_depth.saturating_sub(1);
        }
        self.engine.end_type_arguments(count, begin, end);
    }

    fn handle_no_type_arguments(&mut self, token: Token) {
        // The flow face (#2125): the recorder clears its pending-argument
        // latch (see `begin_type_arguments`).
        self.flow().handle_no_type_arguments(token);

        // The type completed without a group; the head slot is consumed.
        self.engine.pending_type_head = None;
    }

    fn handle_type_argument_application(&mut self, _open_angle: Token) {
        self.engine.consume_type_argument_application();
    }

    // —— value rows: literals and groups ———————————————————————————————
    // Adjacent string literals push no extra row: each constituent string
    // already did.

    fn handle_literal_int(&mut self, token: Token) {
        self.flow().handle_literal_int(token);

        let engine = &mut self.engine;
        engine.push_primitive_row(token, 0);
        engine.const_int_literal(token);
        // The constant-pattern face: an int literal's static type
        // (`hasPrimitiveEquality`, constant_verifier.dart:140-165). Only
        // the pattern's OWN expression arms — a literal inside a `const
        // A(0)` argument list types the evaluated creation's type, never
        // the operand's (`ur_arg_base` holds the open argument lists; a
        // literal nested in one sits below the pattern's expression).
        if engine.const_capture.top_is_constant_pattern() && engine.ur_arg_base.is_empty() {
            let ty = Ty::named(engine.members.primitive_ids[0]);
            engine.const_fold.arm(ty, token.range);
        }
    }

    fn handle_literal_int_with_separators(&mut self, token: Token) {
        self.flow().handle_literal_int_with_separators(token);

        let engine = &mut self.engine;
        engine.push_primitive_row(token, 0);
        engine.const_int_literal(token);
        if engine.const_capture.top_is_constant_pattern() && engine.ur_arg_base.is_empty() {
            let ty = Ty::named(engine.members.primitive_ids[0]);
            engine.const_fold.arm(ty, token.range);
        }
    }

    fn handle_literal_double(&mut self, token: Token) {
        self.flow().handle_literal_double(token);

        let engine = &mut self.engine;
        engine.push_primitive_row(token, 1);
        engine.const_double_literal(token);
        if engine.const_capture.top_is_constant_pattern() && engine.ur_arg_base.is_empty() {
            let ty = Ty::named(engine.members.primitive_ids[1]);
            engine.const_fold.arm(ty, token.range);
        }
    }

    fn handle_literal_double_with_separators(&mut self, token: Token) {
        self.flow().handle_literal_double_with_separators(token);

        let engine = &mut self.engine;
        engine.push_primitive_row(token, 1);
        engine.const_double_literal(token);
        if engine.const_capture.top_is_constant_pattern() && engine.ur_arg_base.is_empty() {
            let ty = Ty::named(engine.members.primitive_ids[1]);
            engine.const_fold.arm(ty, token.range);
        }
    }

    fn handle_literal_bool(&mut self, token: Token) {
        self.flow().handle_literal_bool(token);

        let engine = &mut self.engine;
        if engine.const_capture.top_is_constant_pattern() && engine.ur_arg_base.is_empty() {
            let is_true = engine.text_of(token) == Some("true");
            engine.switch.pending_const = Some(Restriction::BoolValue(is_true));
            let ty = Ty::named(engine.members.primitive_ids[3]);
            engine.const_fold.arm(ty, token.range);
        }
        engine.push_primitive_row(token, 3);
        engine.const_bool_literal(token);
    }

    fn handle_literal_null(&mut self, token: Token) {
        self.flow().handle_literal_null(token);

        let engine = &mut self.engine;
        if engine.const_capture.top_is_constant_pattern() && engine.ur_arg_base.is_empty() {
            engine.switch.pending_const = Some(Restriction::Null);
            engine.const_fold.arm(Ty::Null, token.range);
        }
        engine.push_null_row(token);
        engine.const_null_literal(token);
    }

    fn begin_literal_string(&mut self, begin: Token) {
        let engine = &mut self.engine;
        engine.nsm_poison();
        engine.const_string_begins.push(begin.range);
        engine.const_capture.begin_literal_string();
        // #1823: the literal's row mark — the interpolation reads pushed
        // until the matching end are its children, never the enclosing
        // expression's value row.
        engine.string_row_marks.push(engine.rows.len());
    }

    fn handle_interpolation_expression(&mut self, left: Token, _right: Option<Token>) {
        self.engine
            .const_capture
            .interpolation_expression(left.range);
    }

    // —— conditional-URI configuration window ————————————————————————————
    // The `if (...)` configuration of a configurable import/export is never
    // resolved as an expression (the oracle resolves only the dottedName;
    // the interpolated `==` value is the parser's
    // invalid_literal_in_configuration face), so the rows sourced inside
    // the window drop at the resolution pass.

    fn begin_conditional_uri(&mut self, _if_kw: Token) {
        self.engine.conditional_uri_window = true;
    }

    fn end_conditional_uri(
        &mut self,
        _if_kw: Token,
        _left_paren: Token,
        _equal_sign: Option<Token>,
    ) {
        self.engine.conditional_uri_window = false;
    }

    fn end_literal_string(&mut self, interpolation_count: usize, end: Token) {
        self.flow().end_literal_string(interpolation_count, end);

        let engine = &mut self.engine;
        // `InterpolationExpression` is in the official used set.
        engine.ur_seal_used();
        // The begin span arrives via the capture's own string marks (the
        // engine latched it at `begin_literal_string` — carried on the
        // event's `end` counterpart below).
        let begin = engine.const_string_begins.pop().unwrap_or(end.range);
        if interpolation_count == 0 {
            engine.const_string_leaf(begin, end.range);
        } else {
            engine.const_string_end(interpolation_count, begin, end.range);
        }
        // #1916: the completion row spans the literal's whole node extent
        // (the oracle's StringInterpolation/SimpleStringLiteral) — begin
        // offset through the last consumed String token's end. The parser's
        // end token equals the begin token on an interpolation-free literal,
        // so that face keeps its single-token span.
        let range = SourceRange {
            offset: begin.offset,
            length: end.range.end().saturating_sub(begin.offset),
        };
        engine.push_primitive_row_at(range, 4);
        // #1823: every row pushed since the literal's begin mark is the
        // interpolation's own reads (the completion row is this push) —
        // recorded so the single-expression-window faces skip them. The
        // mark stack pairs with `begin_literal_string`, nesting included.
        // A fold inside the interpolation (`${f(x)}`) can splice the row
        // vec below the mark, so the mark clamps before the slice.
        if let Some(mark) = engine.string_row_marks.pop()
            && engine.rows.len() > 0
        {
            let mark = mark.min(engine.rows.len() - 1);
            engine.interpolation_child_rows.extend(
                engine.rows[mark..engine.rows.len() - 1]
                    .iter()
                    .map(|row| row.range.offset),
            );
        }
        // A plain string literal's constant-pattern static type; an
        // interpolated one is not a `hasPrimitiveEquality` constant
        // (constant_verifier.dart:140-165).
        if engine.const_capture.top_is_constant_pattern()
            && engine.ur_arg_base.is_empty()
            && interpolation_count == 0
        {
            let ty = Ty::named(engine.members.primitive_ids[4]);
            engine.const_fold.arm(ty, end.range);
        }
    }

    fn end_literal_symbol(&mut self, hash: Token, identifier_count: usize) {
        let engine = &mut self.engine;
        // A symbol literal's static type is `Symbol`, so a following selector
        // (`#void.foo;`) resolves against it — the oracle reports the
        // undefined member on the `Symbol` face (`symbols.dart`), which a
        // value row would suppress (`receiver = Unknown`).
        let empty = engine.intern("");
        let symbol = engine.intern("Symbol");
        let ty = Ty::named(symbol);
        engine.push_row(Row {
            name: empty,
            range: hash.range,
            qualified: false,
            shape: RowShape::Receiver(Receiver::Typed {
                type_name: symbol,
                ty,
            }),
            intent: Intent::Read,
            write_op: None,
            ty,
        });
        // The symbol's own name (`parse_symbol_rest`): one
        // operator/`void`/identifier token, or the dotted parts joined —
        // the name decides the duplicate twins' equality (the upstream
        // `SymbolState`).
        let mut name = String::new();
        for index in 0..identifier_count {
            if index > 0 {
                name.push('.');
            }
            if let Some(part) = engine.tokens.get((hash.id + 1 + 2 * index as u32) as usize) {
                name.push_str(&engine.source[part.byte_range()]);
            }
        }
        let name = engine.strings.intern(&name);
        engine.const_capture.symbol(name, hash.range);
    }

    fn handle_adjacent_string_literals(&mut self, start: Token, literal_count: usize) {
        self.engine
            .const_capture
            .adjacent_strings(literal_count, start.range);
    }

    fn handle_literal_list(
        &mut self,
        count: usize,
        left: Token,
        const_keyword: Option<Token>,
        right: Token,
    ) {
        self.flow()
            .handle_literal_list(count, left, const_keyword, right);

        let engine = &mut self.engine;
        // `ListLiteral` is in the official used set.
        engine.ur_seal_used();
        // No begin event brackets a list literal: the close poisons (the
        // body expression was the literal, never the bare shape).
        engine.nsm_poison();
        if let Some(elements) = engine.const_capture.literal_list(count, left.range) {
            // Implicit const literals in a const context check per element
            // with the element code too (the oracle evaluates the literal
            // at the enclosing site).
            let failed =
                engine.check_const_elements(SemanticCode::NonConstantListElement, &elements);
            let payload = engine
                .literal_payload(&elements)
                .map(|values| ConstantValue::List(values.into()));
            engine.push_literal_value(
                failed,
                right.range,
                crate::const_eval::InstanceKind::List,
                payload,
            );
        }
        engine.check_list_literal_elements(left, right);
        // The literal folds to one expression node upstream: its element
        // rows belong to the node, not the stack — drain them into the
        // deferred pass-2 queue exactly like the send fold does, so the
        // enclosing single-row faces (`x = <num>[1]`'s `invalid_assignment`,
        // `init_mark`'s `rows.len() == mark + 1` window) see the literal's
        // own row alone. Element checks above already read their rows; the
        // drained rows still owe their own reports (`undefined_getter`
        // inside an element, `used_names` marks) through pass 2.
        let literal_end = right.range.end();
        let split = engine
            .rows
            .iter()
            .position(|row| row.range.offset > left.range.offset && row.range.end() <= literal_end)
            .unwrap_or(engine.rows.len());
        for row in engine.rows.drain(split..) {
            engine.deferred_miss_rows.push(row);
        }
        // A typed literal (`<num>[...]` / `const <num>[...]`) carries its
        // written `List<E>` static type: the enclosing single-row faces
        // (`invalid_assignment`, `argument_type_not_assignable`,
        // `return_of_invalid_type`) ask their assignability question of it
        // exactly like a typed expression. The row spans the whole literal
        // — `const`/`>`-start through the `]` — matching the oracle's
        // listLiteral anchor.
        if engine.typed_list_literal_ty(left).is_some() {
            let span_start = const_keyword.map_or_else(
                || {
                    // Locate the `<` group's head byte for the anchor's
                    // start: walk back from `[` past the `>` and the group.
                    let bytes = engine.source.as_bytes();
                    let mut cursor = engine.byte_of(left.range.offset);
                    while cursor > 0 && bytes[cursor - 1].is_ascii_whitespace() {
                        cursor -= 1;
                    }
                    // cursor sits just after `>`; walk back over the group.
                    let mut depth = 0usize;
                    while cursor > 0 {
                        cursor -= 1;
                        match bytes[cursor] {
                            b'>' => depth += 1,
                            b'<' => {
                                depth -= 1;
                                if depth == 0 {
                                    break;
                                }
                            }
                            _ => {}
                        }
                    }
                    engine.utf16_of(cursor)
                },
                |token| token.range.offset,
            );
            let range = SourceRange {
                offset: span_start,
                length: right.range.end() - span_start,
            };
            // The written group's nested arguments collapse under the
            // helper's flat segment scan (the segment's LAST type-use
            // wins — `<List<int>>` reads its element as `int`): a nested
            // group rebuilds through the recursive parse (#2317②), an
            // unspellable shape keeps the row opaque.
            let ty = if engine.group_has_nested_args(left) {
                engine
                    .literal_group_args(left)
                    .and_then(|args| match args.as_slice() {
                        [elem] => Some(engine.boundary.args.borrow_mut().named(
                            engine.collection_heads[0],
                            false,
                            std::slice::from_ref(elem),
                        )),
                        _ => None,
                    })
            } else {
                engine.typed_list_literal_ty(left)
            };
            if let Some(ty) = ty {
                engine.push_typed_value_row(range, ty);
                return;
            }
        }
        // The unannotated literal's completed row stays opaque; the
        // return check re-types it against the host's declared return
        // (the context-driven resolution the oracle runs).
        engine
            .untyped_literals
            .insert(right.range.offset, UntypedLiteral::List);
        engine.push_value_row(right);
    }

    fn begin_literal_set_or_map(&mut self, _left: Token, const_keyword: Option<Token>) {
        self.engine.nsm_poison();
        // The non-const duplicate window (#747): only literals without a
        // `const` keyword qualify; the capture skips anything already
        // inside a const window (implicitly const literals are the ERROR
        // const twins' business).
        if const_keyword.is_none() {
            self.engine.const_capture.open_non_const_dup();
        }
    }

    fn handle_literal_set_or_map(
        &mut self,
        count: usize,
        left: Token,
        const_keyword: Option<Token>,
        right: Token,
        has_set_entry: bool,
    ) {
        use crate::const_eval::{InstanceKind, SetOrMap};
        self.flow()
            .handle_literal_set_or_map(count, left, const_keyword, right, has_set_entry);

        let engine = &mut self.engine;
        // `SetOrMapLiteral`/`MapLiteralEntry` are in the official used set.
        engine.ur_seal_used();
        if let Some((shape, has_ctl)) =
            engine
                .const_capture
                .close_non_const_dup(count, left.range, has_set_entry)
        {
            // The `_checkForDuplications` face (WARNING tier).
            engine.check_equal_duplicates(shape, has_ctl);
        } else if let Some(shape) =
            engine
                .const_capture
                .literal_set_or_map(count, left.range, has_set_entry)
        {
            match shape {
                SetOrMap::Map(entries) => {
                    // The oracle names the failing side
                    // (`_validateMapLiteralEntry`): `non_constant_map_key` /
                    // `non_constant_map_value` at each entry's key and value.
                    let mut failed = None;
                    for (key, value) in &entries {
                        for (code, part) in [
                            (SemanticCode::NonConstantMapKey, key),
                            (SemanticCode::NonConstantMapValue, value),
                        ] {
                            let at = engine.check_const_elements(code, std::slice::from_ref(part));
                            failed = failed.or(at);
                        }
                    }
                    let payload = engine.map_payload(&entries);
                    engine.push_literal_value(failed, right.range, InstanceKind::Map, payload);
                }
                SetOrMap::Set(elements) => {
                    let failed =
                        engine.check_const_elements(SemanticCode::NonConstantSetElement, &elements);
                    let payload = engine
                        .literal_payload(&elements)
                        .map(|values| ConstantValue::Set(values.into()));
                    engine.push_literal_value(failed, right.range, InstanceKind::Set, payload);
                }
            }
        }
        // A typed literal (`<int>{}` → `Set<int>`, `<K, V>{}` → `Map<K, V>`)
        // carries its written static type: the same face the typed list
        // literal runs, the row spanning the whole literal —
        // `const`/`<`-start through the `}`. An unannotated literal's
        // completed row stays opaque; the return check re-types it against
        // the host's declared return.
        //
        // The flat group's type comes from the engine helper; a NESTED
        // group rebuilds through the recursive parse — the helper's flat
        // segment scan takes each segment's LAST type-use, so
        // `<String, List<String>>` reads its value as `String`, and its
        // comma split cannot even attribute a nested group (three
        // segments → `None`) (#2317②). A nested shape the name-level
        // parse cannot spell (a function-typed argument) keeps the row
        // opaque and silent — the written group decided the shape
        // upstream either way.
        let nested = engine.group_has_nested_args(left);
        let helper_typed = engine.typed_set_or_map_literal_ty(left);
        let typed_args = if nested {
            engine.literal_group_args(left)
        } else if let Some((_, Ty::Named { args, .. })) = &helper_typed {
            Some(engine.boundary.args.borrow().get(*args).to_vec())
        } else {
            None
        };
        // The ambiguity face (`_inferSetOrMapLiteralType`'s tail): an
        // untyped, non-empty literal whose context decides nothing and
        // whose elements cannot agree on a side reports
        // `ambiguous_set_or_map_literal_either`/`_both` at the literal.
        if helper_typed.is_none() && !nested && engine.literal_context_side(left).is_none() {
            engine.check_set_or_map_ambiguity(left, const_keyword, right, count);
        }
        if let Some(args) = typed_args {
            let head = match args.len() {
                1 => engine.collection_heads[1],
                2 => engine.collection_heads[2],
                _ => {
                    // The written group is not a set/map arity: the
                    // shape stays opaque (a recovery shape the oracle
                    // itself types `dynamic`).
                    engine.push_value_row(right);
                    return;
                }
            };
            let span_start = const_keyword.map_or_else(
                || {
                    engine
                        .typed_group_head_u16(left)
                        .unwrap_or(left.range.offset)
                },
                |token| token.range.offset,
            );
            let range = SourceRange {
                offset: span_start,
                length: right.range.end() - span_start,
            };
            let ty = engine.boundary.args.borrow_mut().named(head, false, &args);
            // The typed map's own entry face
            // (`_verifyMapLiteralEntry`): each key/value against the
            // written type arguments.
            if let [k, v] = args.as_slice() {
                engine.check_map_literal_entries(left, right, *k, *v);
            }
            engine.push_typed_value_row(range, ty);
            return;
        }
        let kind = if count == 0 {
            UntypedLiteral::EmptyBraces
        } else if has_set_entry {
            UntypedLiteral::SetElements
        } else {
            UntypedLiteral::MapEntries
        };
        engine.untyped_literals.insert(right.range.offset, kind);
        // The untyped literal's map-context entry face: a written
        // `Map<K, V>` in the direct initializer / arrow-body / `return`
        // position donates its type arguments to the entry checks
        // (`typed_literal_resolver.dart` `_fromContextType` →
        // literal_element_verifier.dart).
        if let Some((k, v)) = engine.map_entry_context(left) {
            engine.check_map_literal_entries(left, right, k, v);
        }
        engine.push_value_row(right);
    }

    fn handle_literal_map_entry(
        &mut self,
        colon: Token,
        end: Token,
        nk: Option<Token>,
        nv: Option<Token>,
    ) {
        self.flow().handle_literal_map_entry(colon, end, nk, nv);

        self.engine.const_capture.literal_map_entry(colon.range);
    }

    fn end_parenthesized_expression(&mut self, token: Token) {
        self.flow().end_parenthesized_expression(token);

        let engine = &mut self.engine;
        // The parenthesized (non-record) face closes its field window: the
        // group's row carries the inner expression's static type — `(e)` has
        // exactly e's type (the official ParenthesizedExpression.staticType)
        // — so a member access on the group resolves against it (the
        // interface walk and the extension-application face both read the
        // receiver row's type; an opaque row silenced every `(recv).member`
        // shape). Only a row inside this group's window donates its type —
        // a recovery shape with no inner row keeps the opaque face. The
        // inner rows stay below: their own resolutions owe their
        // diagnostics as before.
        // When the inner extent sits inside a statically-`bool` composite
        // span (a comparison's latched `left..right` extent — a binary
        // expression pushes no result row, so the last row's type is the
        // right operand's, not the comparison's `bool`), the parent's
        // `inner_ty` is `bool` — the composite's own static type.
        let inner_row = engine.record_marks.last().and_then(|(_, open)| {
            engine
                .rows
                .last()
                .filter(|row| row.range.offset >= open.range.offset)
        });
        let inner_ty = match inner_row {
            Some(row)
                if engine.bool_decided.iter().any(|span| {
                    span.offset <= row.range.offset && row.range.end() <= span.end()
                }) =>
            {
                Ty::named(engine.members.primitive_ids[3])
            }
            Some(row) => row.ty,
            None => Ty::UNKNOWN,
        };
        engine.record_marks.pop();
        engine.record_literal_names.pop();
        engine.push_row(Row {
            name: engine.intern(""),
            range: token.range,
            qualified: false,
            shape: RowShape::Bare(BareInfo::Plain { ty: inner_ty }),
            intent: Intent::Read,
            write_op: None,
            ty: inner_ty,
        });
        engine.const_capture.parenthesized_end();
    }

    /// A record literal's named field closed (`name: expr`): the label is
    /// the token before the colon — it joins the open literal's name list
    /// in source order (the trailing `names.len()` split fields are the
    /// named pairs).
    fn handle_named_record_field(&mut self, colon: Token) {
        let engine = &mut self.engine;
        let name = colon
            .id
            .checked_sub(1)
            .and_then(|id| engine.tokens.get(id as usize))
            .map(|t| engine.strings.intern(&engine.source[t.byte_range()]))
            .unwrap_or_else(|| engine.intern(""));
        if let Some(names) = engine.record_literal_names.last_mut() {
            names.push(name);
        }
    }

    fn end_record_literal(&mut self, token: Token, count: usize, const_keyword: Option<Token>) {
        self.flow().end_record_literal(token, count, const_keyword);

        let engine = &mut self.engine;
        // `RecordLiteral` is in the official used set.
        engine.ur_seal_used();
        // This close owns the begin-pushed names entry (the parser emits
        // exactly one of `end_record_literal`/`end_parenthesized_expression`
        // per `(`); leaving it would misattribute a nested literal's labels
        // to its parent.
        let names: Vec<StringId> = engine.record_literal_names.pop().unwrap_or_default();
        if let Some(fields) = engine.const_capture.record_literal(count, token.range) {
            let failed = engine.check_const_elements(SemanticCode::NonConstantRecordField, &fields);
            let payload = engine.record_payload(&fields, &names);
            engine.push_literal_value(
                failed,
                token.range,
                crate::const_eval::InstanceKind::Record,
                payload,
            );
        }
        // The literal's row: a single positional field whose type the field
        // window decides carries the record shape (`const(x-1)`'s `(int,)`),
        // so an operator applied to the literal resolves against a type that
        // declares none (`undefined_operator '+'` on `(int,)`,
        // keyword_named_class_methods_2.dart:49). Every other shape keeps
        // the opaque value row (the oracle's own field types are inference
        // faces M1 does not model).
        let fields = engine.record_field_tys(count, names);
        match fields {
            Some((mark, positional, named)) => {
                // The literal's row replaces its field residue: the fields
                // already emitted their own walk-time diagnostics, so the
                // window drains into one typed record row — the same fold
                // the argument rows use. The drain is unfiltered: a dropped
                // row still owes its pass-2 obligations. A resolved bare
                // field marks its usage (`(a: _live())` spares `_live`,
                // `(a: _read)` reads the field) and an unresolved one its
                // report (`(1, _)`'s `_` — the record literal's field
                // resolves as an expression upstream), exactly like the
                // plain-send and creation folds' deferred drain; the pass-2
                // arms keep non-reporting shapes silent.
                engine.deferred_miss_rows.extend(engine.rows.drain(mark..));
                engine.push_record_row(token, positional, named);
            }
            None => engine.push_value_row(token),
        }
    }

    fn end_const_literal(&mut self, end: Token) {
        let engine = &mut self.engine;
        engine.const_capture.close_explicit_literal();
        // The inner literal already pushed its own row at the same end
        // token (list/set/map/record all do): a second opaque row would
        // shadow it — the `const(x-1) + 1` record row must stay the
        // operator's operand.
        if engine.rows.last().map(|row| row.range.end()) != Some(end.range.end()) {
            engine.push_value_row(end);
        }
    }

    // —— #505: directives ———————————————————————————————————————————————

    fn begin_import(&mut self, import_keyword: Token) {
        let engine = &mut self.engine;
        engine.in_import = true;
        engine.begin_import_site(import_keyword);
    }

    fn end_import(&mut self, _import_keyword: Token, _semicolon: Option<Token>) {
        self.engine.in_import = false;
    }

    fn begin_export(&mut self, export_keyword: Token) {
        let engine = &mut self.engine;
        engine.in_export = true;
        let mut face = ExportUseFace {
            directive_range: export_keyword.range,
            ..ExportUseFace::default()
        };
        // The URI literal is the token after the keyword; only an honestly
        // quoted literal joins (the recovery posture matches the import
        // site's — a malformed URI stays silent for the deprecated-SDK
        // face).
        if let Some(uri) = engine.tokens.get(export_keyword.id as usize + 1) {
            let raw = &engine.source[uri.byte_range()];
            let bytes = raw.as_bytes();
            if bytes.len() >= 2 {
                let quote = bytes[0];
                if (quote == b'\'' || quote == b'"') && bytes[bytes.len() - 1] == quote {
                    face.uri_text = raw[1..raw.len() - 1].to_string();
                }
            }
        }
        engine.exports.push(face);
    }

    fn end_export(&mut self, export_keyword: Token, semicolon: Token) {
        let engine = &mut self.engine;
        engine.in_export = false;
        // Latch the whole-directive anchor: `export` keyword start → `;`
        // end (the upstream `usageRange: node.sourceRange` face — the
        // oracle reports the deprecated-SDK row over the ExportDirective,
        // not the URI literal).
        if let Some(export) = engine.exports.last_mut() {
            export.directive_range = SourceRange {
                offset: export_keyword.range.offset,
                length: semicolon.range.end() - export_keyword.range.offset,
            };
        }
    }

    fn begin_show(&mut self, _token: Token) {
        self.engine.in_show = true;
    }

    fn end_show(&mut self, _token: Token) {
        self.engine.in_show = false;
    }

    // —— #279: const contexts ————————————————————————————————————————

    fn begin_fields(
        &mut self,
        kind: DeclarationKind,
        _augment_kw: Option<Token>,
        _abstract_kw: Option<Token>,
        _external_kw: Option<Token>,
        static_kw: Option<Token>,
        _covariant_kw: Option<Token>,
        late_kw: Option<Token>,
        var_final_or_const: Option<Token>,
        _last_consumed: Token,
    ) {
        let engine = &mut self.engine;
        // The member field group is open: its const initializers close on
        // the field face (`const_field_values`), not the local face. The
        // group's frame depth discriminates its own initializers from a
        // closure-local inside one of them. A top-level variable group is
        // not a field group: the instance context the implicit-this face
        // arms on (`ThisContext.instanceFieldDeclaration`) exists only
        // inside a class-family declaration, and `bindsThis`'s
        // `!element.isStatic` answers false at file scope — the group's
        // modifier bits arm regardless (top-level `late`/`final`/`const`
        // latch through them), and the const close reads the file scope
        // through `at_file_scope`.
        engine.field_group_open = !matches!(kind, DeclarationKind::TopLevel);
        engine.field_group_depth = engine.frames.len();
        // The group's modifier bits gate the uninitialized latch at the name
        // events, which precede `begin_variables_declaration` for field
        // groups (the parser folds the names before the group head fires) —
        // the same arms as that event, armed here first.
        engine.group_late = late_kw.is_some();
        engine.group_final = var_final_or_const
            .and_then(|t| engine.text_of(t))
            .is_some_and(|text| text == "final");
        engine.group_const = var_final_or_const
            .and_then(|t| engine.text_of(t))
            .is_some_and(|text| text == "const");
        if var_final_or_const.and_then(|t| engine.text_of(t)) == Some("const") {
            engine.arm_const_group();
            // `const_instance_field` (error_verifier.dart:1260-1268): an
            // instance field group's `const` reports once per group, at
            // the `const` keyword — inside a container (top-level `const`
            // fields are legal).
            if static_kw.is_none() && engine.container_context().is_some() {
                if let Some(const_kw) = var_final_or_const {
                    engine.emit(SemanticCode::ConstInstanceField, const_kw.range, "", "");
                }
            }
        }
        engine.open_member_static = static_kw.is_some();
    }

    fn end_fields(
        &mut self,
        _kind: DeclarationKind,
        _abstract_kw: Option<Token>,
        _augment_kw: Option<Token>,
        _external_kw: Option<Token>,
        _static_kw: Option<Token>,
        _covariant_kw: Option<Token>,
        _late_kw: Option<Token>,
        _var_final_or_const: Option<Token>,
        _count: usize,
        _begin: Token,
        _end: Token,
    ) {
        let engine = &mut self.engine;
        engine.field_group_open = false;
        engine.disarm_const_group();
        // The field group closed: an unclaimed signature stash dies here.
        engine.fn_stash_drop();
    }

    fn end_top_level_fields(
        &mut self,
        _augment_kw: Option<Token>,
        _abstract_kw: Option<Token>,
        _external_kw: Option<Token>,
        _static_kw: Option<Token>,
        _covariant_kw: Option<Token>,
        _late_kw: Option<Token>,
        _var_final_or_const: Option<Token>,
        _count: usize,
        _begin: Token,
        _end: Token,
    ) {
        let engine = &mut self.engine;
        engine.field_group_open = false;
        engine.disarm_const_group();
        // The frameless initializer locals die with their declaration: a
        // later declaration's reads must not resolve to them (the official
        // scopes the variables to their clause).
        engine.initializer_locals.clear();
        // The top-level variable group closed: an unclaimed stash dies.
        engine.fn_stash_drop();
    }

    fn begin_field_initializer(&mut self, _token: Token) {
        self.engine.open_const_initializer();
    }

    fn end_field_initializer(&mut self, _assignment: Token, _end: Token) {
        self.engine.close_const_field_initializer();
    }

    fn begin_const_literal(&mut self, _token: Token) {
        self.engine.const_capture.open_explicit_literal();
    }

    fn end_constructor_reference(
        &mut self,
        start: Token,
        _period_before_name: Option<Token>,
        _end: Token,
        context: ConstructorReferenceContext,
    ) {
        let engine = &mut self.engine;
        if context == ConstructorReferenceContext::Const {
            engine.arm_const_ctor_gate(start);
        }
        // The qualified-head face: a three-segment reference (`prefix.C`,
        // the written period names the constructor) resolves its head as a
        // prefixed NamedType (`named_type_resolver.dart`'s importPrefix
        // face) — a dead prefix reports `undefined_identifier` over the
        // whole type span, never the bare head's face (`new Missing()`
        // reports `creation_with_non_type` — upstream Dart SDK source,
        // pkg/analyzer/lib/src/dart/resolver/named_type_resolver.dart,
        // resolveConstructorTypeReference's tail :313-330). A
        // redirecting-factory target keeps its dedicated silent verdict:
        // the oracle's `redirect_to_non_class` face sits outside the frozen
        // gate, and the requalification would re-skin the head as a scored
        // `undefined_identifier`.
        if let Some(period) = _period_before_name
            && engine.redirect_target_depth == 0
        {
            engine.requalify_creation_head(start, period);
        }
        // The two-segment qualified reference whose head resolves to
        // NOTHING (`fb.C<int>()`, `const fb.C()` — no period before the
        // name, `fb` absent from every scope): the head is the NamedType's
        // import prefix and its element is null, so upstream's
        // `resolveConstructorTypeReference` tail reports
        // `undefined_identifier` over the written `prefix.C` span and
        // returns with `InvalidType` — the creation face and the
        // type-argument face never ask their question. The invocation-parse
        // shape (`fb.C()` keyword-less, which upstream resolves as a target
        // identifier first) reports the same face at the prefix token alone
        // (probed: `fb.C()` → 1:11 length 2, `const fb.C<int>()` → 1:17
        // length 4). The head row converts to the prefix face and its
        // recorded creation miss is consumed — a prefix that RESOLVES keeps
        // the class-segment verdict below (`creation_with_non_type` at the
        // class token), and a resolved non-prefix head (a class spelled as
        // a prefix) keeps the creation-miss arm's faces.
        if _period_before_name.is_none()
            && engine.redirect_target_depth == 0
            && let Some((head, Some((_, selector_range)))) = engine.creation_ref
            && matches!(engine.resolve_bare(head).0, BareInfo::Missing { .. })
            && engine
                .creation_miss_rows
                .binary_search_by_key(&start.range.offset, |entry| entry.0)
                .is_ok()
        {
            let end = match context {
                ConstructorReferenceContext::Const | ConstructorReferenceContext::New => {
                    selector_range.end()
                }
                _ => start.range.end(),
            };
            engine.requalify_creation_head_to(start, end);
        }
        // The two-segment qualified reference (`const Missing.new()`): the
        // `.new` rode the qualified rest, so the reference's type is the
        // two-segment NamedType `Missing.new` — the official importPrefix
        // bail reports `undefined_identifier` over the whole written head
        // (importPrefix.offset through node.name.end) when the first
        // segment is unresolved (upstream Dart SDK source,
        // pkg/analyzer/lib/src/dart/resolver/named_type_resolver.dart,
        // resolveConstructorTypeReference's tail). A resolved head keeps
        // the unnamed-constructor selector face (the rewrite reconstructs
        // the type + selector pair), so only a Missing head row converts.
        if _period_before_name.is_none()
            && engine.redirect_target_depth == 0
            && let Some((_, Some((selector, selector_range)))) = engine.creation_ref
            && selector == engine.intern("new")
        {
            engine.requalify_creation_head_to(start, selector_range.end());
        }
        // A redirecting factory's `= C.name` target: the reference's head
        // keys the target container and its continuation selector keys the
        // target — the `RecursiveConstructorRedirect` cycle walk's edge
        // tuple (`= C` alone latches the head under its own spelling, the
        // `this()` edge's same convention).
        if engine.redirect_target_depth > 0
            && engine.factory_redirect_target.is_none()
            && let Some(name) = engine.name_of(start)
        {
            let (target, span) = engine
                .creation_ref
                .and_then(|(_, selector)| selector)
                .map_or((name, start.range), |(selector, range)| {
                    (
                        selector,
                        SourceRange {
                            offset: start.range.offset,
                            length: range.end() - start.range.offset,
                        },
                    )
                });
            engine.factory_redirect_target = Some((name, target, span));
        }
        // A prefix-headed constructor reference (`fb.RuleState.stable()`)
        // resolves its class segment through the prefix's export scope —
        // the oracle's `resolveConstructorTypeReference` importPrefix arm
        // (named_type_resolver.dart:140-147). A hit on a type stays silent;
        // a miss or a non-type element reports `creation_with_non_type` at
        // the class-name token (the tail's `diagnostic.at(node.name)`, the
        // `errorTarget.element is PrefixElement` branch — :305-330).
        if let Some(index) = engine
            .ctor_prefix_heads
            .iter()
            .position(|offset| *offset == start.range.offset)
        {
            engine.ctor_prefix_heads.remove(index);
            let prefix = engine.name_of(start).unwrap_or_else(|| engine.intern(""));
            let (class_name, class_range) = engine
                .creation_ref
                .and_then(|(_, selector)| selector)
                .map_or((prefix, start.range), |(name, range)| (name, range));
            engine.resolve_ctor_prefix_class(prefix, class_name, class_range);
        }
        // The reference closed — its segments stop matching type-argument
        // groups (a plain invocation's group inside `new C(...)`'s
        // arguments must not).
        //
        // The selector's element credit: this event family carries the
        // creation's `C._x` selector without a member-access row, so the
        // element-use pool needs the explicit credit (the oracle resolves
        // the constructor element of every creation form). Element-accurate
        // through [`Engine::credit_constructor_selector`]'s container
        // check; a prefixed head's segments dead-end there (the
        // under-report direction — a private selector is never reachable
        // through a prefix).
        if let Some((head, Some((selector, _)))) = engine.creation_ref {
            engine.credit_constructor_selector(head, selector);
        }
        engine.creation_ref = None;
    }

    fn begin_await_expression(&mut self, token: Token) {
        self.flow().begin_await_expression(token);

        let engine = &mut self.engine;
        engine.nsm_poison();
        engine.const_capture.begin_opaque_group();
        let _ = token;
    }

    fn end_await_expression(&mut self, begin: Token, end: Token) {
        self.flow().end_await_expression(begin, end);

        let engine = &mut self.engine;
        engine.nsm_poison();
        engine.const_capture.end_opaque_group(begin.range);
        // The await expression's static type is `flatten(operand)`
        // (`analyzeAwaitExpression`, type_analyzer.dart:542-548 —
        // `Future<S>`/`FutureOr<S>` → `S`, nullability preserved, every
        // other type its own flatten). The operand's row is the stack top
        // (await pushes no row of its own), so rewriting its `ty` types
        // the whole expression for every downstream consumer — a
        // `var`-inferred local binds `T`, not `Future<T>`, and a chained
        // `(await f()).g` receiver reads `T` through the parenthesized
        // close's `inner_ty` copy. The gate mirrors the operand window: a
        // row that starts inside the operand extent (after the `await`
        // keyword) is the operand's; a rowless operand leaves an older
        // outer row on top and the rewrite skips it — the same shape the
        // `end_const_literal` guard uses.
        let flat_ty = engine
            .rows
            .last()
            .filter(|row| row.range.offset >= begin.range.end())
            .map(|row| engine.flatten(engine.operand_static_ty(row)));
        if let (Some(row), Some(ty)) = (engine.rows.last_mut(), flat_ty) {
            row.ty = ty;
        }
        // The extent outlives the rewrite: a member operand's row resolves
        // only in the later pass (its close-time `ty` is still `Unknown`),
        // so the `var`-inferred bind re-applies the flatten over the
        // resolved type (#2506 await face — `final d = await t.r;` binds
        // `R`, not `Future<R?>`).
        engine
            .await_operand_extents
            .push((begin.range.end(), end.range.offset));
    }

    fn handle_throw_expression(&mut self, throw: Token, end: Token) {
        self.flow().handle_throw_expression(throw, end);

        // `ThrowExpression` is in the official used set (the event fires
        // after the operand parsed).
        let engine = &mut self.engine;
        engine.nsm_poison();
        engine.ur_seal_used();
        engine.const_capture.throw_expression(throw.range);
        // The throw's static type is `Never` (the resolver records
        // `NeverTypeImpl`, `resolver_visitor.dart` `visitThrowExpression`):
        // the operand's row is the stack top and carries the throw's value
        // downstream, so the assignment/argument checks read the bottom type
        // and never report its operand's own type (`b = throw 'x'` assigns
        // into `bool` — `subtype.dart` Right Bottom).
        if let Some(row) = engine.rows.last_mut() {
            row.ty = Ty::Never;
        }
    }

    fn handle_non_null_assert_expression(&mut self, bang: Token) {
        self.flow().handle_non_null_assert_expression(bang);

        let engine = &mut self.engine;
        engine.nsm_poison();
        // The operand's row is the stack top (the postfix operand parsed
        // just before the `!` event): `visitNullAssertionExpression`'s
        // pair (best_practices_verifier.dart:795-799,
        // error_verifier.dart:7668-7718).
        engine.check_null_assert(bang.range);
        engine.const_capture.non_null_assert(bang.range);
    }

    // —— flow fan-out: events the diagnoser does not consume itself ———
    // Forwarded verbatim to the per-unit flow recorder (see the module
    // docs' flow section).

    fn begin_assert(&mut self, token: Token, kind: AssertKind) {
        self.flow().begin_assert(token, kind);
    }

    // The message analyzes under the negated-condition state
    // (flow_analysis.dart:6478-6485 — `assert_afterCondition` runs the
    // message closure on the condition-false arm), so the forwarded event
    // must reach the recorder or the message's reads analyze under the
    // unconditioned entry state.
    fn assert_after_condition(&mut self) {
        self.flow().assert_after_condition();
    }

    fn begin_block(&mut self, token: Token, kind: BlockKind) {
        self.flow().begin_block(token, kind);
        if !matches!(kind, BlockKind::FunctionBody) {
            self.engine.stmt_block_depth += 1;
        }

        // A statement list opens its duplicate scope (`checkStatements` per
        // `visitBlock` — the try/catch/finally kinds included; the switch
        // body carries no begin_block event, its members scope per case).
        self.engine.dup_scopes.push(FxHashMap::default());
        // The block's own scope frame: its locals close at `end_block`, so
        // a post-block read binds the outer declaration, never a dead inner
        // one (the official visitBlock's scoped element walk). The catch
        // clause's frame (pushed at `begin_catch_clause` for the parameters)
        // nests this body frame inside it, and the parameters stay visible
        // through the normal outer-scope climb.
        self.engine.push_block_frame();
    }

    fn begin_block_function_body(&mut self, token: Token) {
        self.flow().begin_block_function_body(token);
        // A function body's statement list owns a duplicate scope, same as
        // any block (`visitBlock` walks the body's `Block` node).
        self.engine.dup_scopes.push(FxHashMap::default());
        {
            let engine = &mut self.engine;
            // The #748 latch marks the method's own body block (nested
            // blocks and defaults-window closures never match its begin).
            engine.nsm_block_open(token);
            // The block `{` is the executable-completion row's anchor (the
            // official reports at the block's left bracket).
            engine.note_body_brace(token.range);
            // #1314 FAM7: the body verdict is "present" (a closure body
            // inside a default value leaves the outer rows for the owning
            // executable's own body event).
            if engine.default_value_depth == 0 {
                engine.flush_default_checks(true);
            }
        }
    }

    fn begin_do_while_statement(&mut self, token: Token) {
        self.flow().begin_do_while_statement(token);
    }

    fn begin_do_while_statement_body(&mut self, token: Token) {
        self.flow().begin_do_while_statement_body(token);
    }

    fn begin_else_statement(&mut self, token: Token) {
        self.flow().begin_else_statement(token);
        self.engine.if_then_end();
        // Insurance: the condition sealed at `begin_then_statement`.
        self.engine.ur_seal_used();
    }

    fn begin_for_control_flow(&mut self, await_kw: Option<Token>, for_kw: Token) {
        self.flow().begin_for_control_flow(await_kw, for_kw);
        let engine = &mut self.engine;
        // The collection-element form's loop frame: the loop variable's
        // declaration events fire inside the for-parts head (before the
        // for-in split), so the owning frame must open here — and close at
        // `end_for_control_flow` / `end_for_in_control_flow`.
        engine.push_block_frame();
        // The head's declaration list is its own duplicate scope
        // (`checkForVariables`) — reset per loop.
        engine.dup_for_parts.clear();
        // Reset the for-in declarator latch (#2506) — see the statement
        // form's reset for the stale-latch reasoning.
        engine.for_in_loop_var = None;
        // The collection-element form's window (2): its condition is an
        // expression statement like the statement form's; its window closes
        // at the control-flow end.
        engine.in_for_parts.push(2);
        engine.const_capture.begin_ctl_element();
    }

    fn begin_for_in_body(&mut self, token: Token) {
        self.flow().begin_for_in_body(token);
        // The iterable window closes at the body (statement form only —
        // the collection form pops at `end_for_in_control_flow`).
        let engine = &mut self.engine;
        if engine.in_for_parts.last() == Some(&1) {
            engine.in_for_parts.pop();
        }
    }

    fn begin_for_in_expression(&mut self, token: Token) {
        self.flow().begin_for_in_expression(token);
        // The iterable expression's operand window opens here;
        // `handle_for_in_loop_parts` closes it with the `await` keyword and
        // runs the `_checkForEachParts` face over the captured rows.
        self.engine.for_in_marks.push(self.engine.rows.len());
    }

    fn begin_for_statement_body(&mut self, token: Token) {
        self.flow().begin_for_statement_body(token);
    }

    fn begin_if_control_flow(&mut self, token: Token) {
        self.flow().begin_if_control_flow(token);
        self.engine.const_capture.begin_ctl_element();
        // The collection form's condition arms the same window the
        // statement's does when it carries a case clause (the element face
        // of `begin_pattern`'s gate). The parser pairs this event with
        // `handle_then_control_flow` unconditionally, so the depth always
        // tears down at the condition's own end.
        self.engine.if_element_cond += 1;
    }

    fn begin_if_statement(&mut self, token: Token) {
        self.flow().begin_if_statement(token);
        let engine = &mut self.engine;
        engine.if_condition_begin();
        // The condition's operand rows open here (the `non_bool_condition`
        // anchor's first row).
        engine.cond_mark = Some(engine.rows.len());
        engine.cond_case = false;
    }

    fn begin_initialized_identifier(&mut self, token: Token) {
        self.flow().begin_initialized_identifier(token);
    }

    fn begin_labeled_statement(&mut self, token: Token, label_count: usize) {
        self.flow().begin_labeled_statement(token, label_count);
    }

    fn begin_literal_symbol(&mut self, token: Token) {
        self.flow().begin_literal_symbol(token);
    }

    fn begin_pattern_guard(&mut self, when: Token) {
        self.flow().begin_pattern_guard(when);
    }

    fn begin_rethrow_statement(&mut self, token: Token) {
        self.flow().begin_rethrow_statement(token);
    }

    fn begin_switch_case(&mut self, label_count: usize, expression_count: usize, token: Token) {
        self.flow()
            .begin_switch_case(label_count, expression_count, token);

        // A case's pattern variables form their own group window.
        let engine = &mut self.engine;
        engine.pattern_group_open = None;
        // The member's statement list owns a duplicate scope
        // (`checkStatements` per `visitSwitchCase`/`visitSwitchDefault` —
        // one scope per member, never the whole switch body).
        engine.dup_scopes.push(FxHashMap::default());
        // The clause anchor latches (the flow `MatchedType` fact's join
        // key — the same token).
        engine.clause_anchor = Some(token.range);
    }

    fn begin_then_statement(&mut self, token: Token) {
        self.flow().begin_then_statement(token);
        let engine = &mut self.engine;
        engine.if_then_begin();
        // `IfStatement` is in the official used set (the condition).
        engine.ur_seal_used();
        // The if-condition operand's flow-aware static type is `bool?` —
        // the unchecked-use face (#1351).
        engine.check_condition_operand();
    }

    fn begin_try_statement(&mut self, token: Token) {
        self.flow().begin_try_statement(token);

        let engine = &mut self.engine;
        engine.try_clauses.push(Vec::new());
    }

    fn begin_while_statement(&mut self, token: Token) {
        self.flow().begin_while_statement(token);
        let engine = &mut self.engine;
        // The condition's operand rows open here (the `non_bool_condition`
        // anchor's first row).
        engine.cond_mark = Some(engine.rows.len());
        engine.cond_case = false;
    }

    fn begin_while_statement_body(&mut self, token: Token) {
        self.flow().begin_while_statement_body(token);
        let engine = &mut self.engine;
        // `WhileStatement` is in the official used set (the condition).
        engine.ur_seal_used();
        // The while-condition operand's flow-aware static type is `bool?`
        // — the unchecked-use face (#1351).
        engine.check_condition_operand();
    }

    fn end_assert(
        &mut self,
        begin: Token,
        kind: AssertKind,
        left_paren: Token,
        comma: Option<Token>,
        end: Token,
    ) {
        self.flow().end_assert(begin, kind, left_paren, comma, end);
        // `AssertStatement`/`AssertInitializer` are in the official used
        // set (condition and message).
        let engine = &mut self.engine;
        // The assert condition is a condition context
        // (bool_expression_verifier.dart's assert face); the parser
        // delivers no condition-end event, so the operand is the last
        // value row between the paren and the message's comma. The
        // statement face carries the full condition face; the
        // expression/initializer positions stage only — the recovery
        // shapes that borrow those windows (`return assert(x-1) + 1;`)
        // own no oracle condition node.
        let window = (
            left_paren.range.end(),
            comma.map_or(end.range.offset, |c| c.range.offset),
        );
        if kind == AssertKind::Statement {
            engine.check_condition_operand_within(window.0, window.1);
        } else {
            engine.stage_condition_operand_within(window.0, window.1);
        }
        engine.ur_seal_used();
    }

    fn end_block(&mut self, count: usize, begin: Token, end: Token, kind: BlockKind) {
        self.flow().end_block(count, begin, end, kind);
        if !matches!(kind, BlockKind::FunctionBody) {
            self.engine.stmt_block_depth = self.engine.stmt_block_depth.saturating_sub(1);
        }

        // The catch clause fact's body anchor: `handle_catch_block` fires
        // right after the clause body's block ends, so the last block end
        // is its closing brace.
        self.engine.last_block_end = Some(end.range);
        // The block frame's locals decide here (the block-scope unused face
        // — `begin_block`'s pairing pop).
        self.engine.pop_frame();
        self.engine.dup_scopes.pop();
    }

    fn end_block_function_body(&mut self, count: usize, begin: Token, end: Token) {
        self.flow().end_block_function_body(count, begin, end);

        // The #748 block-body face adjudicates at the method's own block's
        // close (one statement, the shape-completing return spanning it).
        self.engine.nsm_block_closed(count, begin, end);
        self.engine.dup_scopes.pop();
    }

    fn end_do_while_statement(&mut self, token: Token, while_kw: Token, end: Token) {
        self.flow().end_do_while_statement(token, while_kw, end);
        let engine = &mut self.engine;
        // `DoStatement` is in the official used set (the while condition).
        engine.ur_seal_used();
        // The do-while condition operand's flow-aware static type is
        // `bool?` — the unchecked-use face (#1351). The bounded variant:
        // the loop body's own rows sit below the condition's, and the
        // condition lives between the `while` keyword and the closing
        // `;`'s statement extent.
        engine.check_condition_operand_within(while_kw.range.end(), end.range.offset);
    }

    fn end_do_while_statement_body(&mut self, end: Token) {
        self.flow().end_do_while_statement_body(end);
    }

    fn end_for_control_flow(&mut self, token: Token) {
        self.flow().end_for_control_flow(token);
        let engine = &mut self.engine;
        if engine.in_for_parts.last() == Some(&2) {
            engine.in_for_parts.pop();
        }
        // The collection-element form's loop frame (opened at
        // `begin_for_control_flow`): the loop variable's local closes with
        // its own scope, mirroring `end_for_in_control_flow`.
        engine.pop_frame();
    }

    fn end_for_in_body(&mut self, end: Token) {
        self.flow().end_for_in_body(end);
    }

    fn end_for_in_control_flow(&mut self, token: Token) {
        self.flow().end_for_in_control_flow(token);
        let engine = &mut self.engine;
        if engine.in_for_parts.last() == Some(&2) {
            engine.in_for_parts.pop();
        }
        // The collection-element form's loop frame: `handle_for_in_loop_parts`
        // pushes it for the statement and collection shapes alike, and the
        // statement form pops at `end_for_in` — the collection twin closes
        // here, so the loop variable's local closes with its own scope (its
        // `unused_local_variable` window is the element's, and a leaked frame
        // would shift every later close by one).
        engine.pop_frame();
    }

    fn end_for_in_expression(&mut self, end: Token) {
        self.flow().end_for_in_expression(end);
        // `ForEachParts` is in the official used set (the iterable).
        self.engine.ur_seal_used();
    }

    fn end_for_statement_body(&mut self, end: Token) {
        self.flow().end_for_statement_body(end);
    }

    #[allow(clippy::too_many_arguments)]
    fn end_formal_parameter(
        &mut self,
        var_or_final: Option<Token>,
        covariant_kw: Option<Token>,
        this_keyword: Option<Token>,
        super_keyword: Option<Token>,
        period_after_this_or_super: Option<Token>,
        name: Token,
        initializer_start: Option<Token>,
        initializer_end: Option<Token>,
        kind: FormalParameterKind,
        member_kind: dartq_parser::MemberKind,
    ) {
        // The inferred-formal face (`_inferFormalParameters`'s flow half):
        // an implicitly typed formal of an open function literal binds its
        // context schema entry in `var_types` at the name event, but the
        // flow's `Ev::Formal` declares `Unknown` without a written
        // annotation — every `e != null` on the inferred formal then finds
        // no nullable step and its reads stay unpromoted (the oracle
        // promotes them). Stage the schema-derived type on the name token
        // before the `Ev::Formal` record consumes it; the same
        // spellability gate as the `var` face holds (a name-level
        // argument-less head — argument-bearing and non-named shapes the
        // flow lattice cannot spell stay untyped, the under-report
        // posture). A formal with a written annotation never arrives: the
        // written `pending_ty` owns the declaration.
        if self.engine.formal_written.is_none()
            && this_keyword.is_none()
            && super_keyword.is_none()
        {
            let staged = self
                .engine
                .name_of(name)
                .and_then(|name_id| {
                    self.engine
                        .frames
                        .last()
                        .and_then(|frame| frame.var_types.get(&name_id).copied())
                })
                .and_then(|ty| match ty {
                    Ty::Named {
                        name: ty_name,
                        nullable,
                        args: ArgListId::EMPTY,
                    } => Some((
                        name.range.offset,
                        self.engine.strings.resolve(ty_name).to_string(),
                        nullable,
                    )),
                    _ => None,
                });
            if let Some((offset, lexeme, nullable)) = staged {
                let base = self.flow().intern_name(&lexeme);
                self.flow().stage_inferred_local(
                    offset,
                    FlowTy {
                        base: FlowBase::Name(base),
                        nullable,
                    },
                    Vec::new(),
                );
            }
        }
        self.flow().end_formal_parameter(
            var_or_final,
            covariant_kw,
            this_keyword,
            super_keyword,
            period_after_this_or_super,
            name,
            initializer_start,
            initializer_end,
            kind,
            member_kind,
        );

        // The #1351 capture: the formal joins the open typedef RHS — its
        // name and requiredness select the named/positional list (a
        // no-op outside the capture; a function-typed formals region
        // already poisoned it at its `Function(`).
        // #1443: two-step bind — the if-let scrutinee's Ref guard would stay
        // live through the body, so the borrow_mut below must run after it drops.
        let name_id = self.engine.name_of(name);
        if let Some(name_id) = name_id {
            let required = !matches!(
                kind,
                FormalParameterKind::OptionalNamed | FormalParameterKind::OptionalPositional
            );
            self.engine
                .bounds
                .alias_formal_end(name_id, name.range.offset, required);
        }

        // The #748 `==` first-formal read: a formal without a default
        // value still holds its written type here (a default's snapshot
        // landed at the default's open).
        self.engine.equals_formal_type();

        // The written-signature capture: a formal whose name event never
        // fired (an unnamed `void Function(T)` slot) flushes unclaimed; a
        // receiver formal poisons the capture.
        self.engine
            .fn_capture_formal_end(this_keyword.is_some() || super_keyword.is_some());

        // FAM18: an initializing formal (`this.name`) initializes the named
        // field for ITS OWN constructor (the
        // `ConstructorFieldsVerifier`'s `initInParameter` state,
        // constructor_fields_verifier.dart) — the per-constructor set the
        // container's flush consults. A `super.x` formal invokes the
        // superclass's constructor the same way (the explicit-super-call
        // satisfaction the implicit-super face reads).
        if let Some(site) = self.engine.open_ctor_site
            && let Some(name_id) = self.engine.name_of(name)
        {
            if this_keyword.is_some() {
                self.engine.ctor_formals_init.insert((site, name_id));
            }
            if this_keyword.is_some() || super_keyword.is_some() {
                self.engine.ctor_super_init.insert(site);
            }
        }

        // An unnamed slot's unconsumed written signature dies here — the
        // slot has no name to claim it, and the next named formal in the
        // group must not inherit a shape that was never its annotation.
        if self.engine.name_of(name).is_none() {
            self.engine.fn_type_stash = None;
        }

        // —— #1314 FAM7: the optional-parameter default-value cluster ————
        // The latches consume unconditionally (a stale separator never
        // reaches the next formal).
        let (colon, had_default, written) = {
            let engine = &mut self.engine;
            (
                engine.formal_colon.take(),
                std::mem::take(&mut engine.formal_has_default),
                engine.formal_written.take(),
            )
        };
        let optional = matches!(
            kind,
            FormalParameterKind::OptionalNamed | FormalParameterKind::OptionalPositional
        );
        let named = kind == FormalParameterKind::OptionalNamed;
        // The colon row's visitor is ungated by the owning executable
        // upstream (`_checkForColonDefaultValue` rides every formal-node
        // visit, best_practices_verifier.dart:497/856/903); the
        // function-type and alias faces carry no default clause there.
        let default_face = matches!(
            member_kind,
            dartq_parser::MemberKind::TopLevelMethod
                | dartq_parser::MemberKind::NonStaticMethod
                | dartq_parser::MemberKind::StaticMethod
                | dartq_parser::MemberKind::ExtensionNonStaticMethod
                | dartq_parser::MemberKind::ExtensionStaticMethod
                | dartq_parser::MemberKind::ExtensionTypeNonStaticMethod
                | dartq_parser::MemberKind::ExtensionTypeStaticMethod
                | dartq_parser::MemberKind::Local
                | dartq_parser::MemberKind::AnonymousMethod
                | dartq_parser::MemberKind::Factory
                | dartq_parser::MemberKind::PrimaryConstructor
        );
        if named
            && default_face
            && let Some(colon) = colon
        {
            self.engine
                .stage_usage(SemanticCode::ObsoleteColonForDefaultValue, colon, "");
        }
        if optional
            && !had_default
            && default_face
            && let Some(written) = written
        {
            // #1443: two-step bind — a `borrow()` in the let-chain condition
            // would keep its Ref guard live through the body, so it must run
            // in its own statement before the borrow_mut calls below.
            let name_id = self.engine.name_of(name);
            if let Some(name_id) = name_id {
                // Primary constructors answer `defaultValuesAreExpected` true
                // unconditionally (error_verifier.dart:8368-8370): decide now.
                if member_kind == dartq_parser::MemberKind::PrimaryConstructor {
                    let engine = &mut self.engine;
                    if engine.written_potentially_non_nullable(written) {
                        let display = engine.strings.resolve(name_id).to_string();
                        let code = if named {
                            SemanticCode::MissingDefaultValueForParameter
                        } else {
                            SemanticCode::MissingDefaultValueForParameterPositional
                        };
                        engine.stage_usage(code, name.range, &display);
                    }
                } else {
                    self.engine.stage_default_check(PendingDefaultCheck {
                        member_kind,
                        name: name_id,
                        name_range: name.range,
                        written,
                        named,
                    });
                }
            }
        }

        // —— #1463: the old-style function-typed formal's scope bind ————
        // The parser's inline-name path swallowed the name token (no
        // identifier event), so the `FormalParameterDeclaration` mux arm
        // never bound it and body uses fell through to the file face
        // (`undefined_function` / `undefined_identifier` /
        // `undefined_method`). Only executable flavors bind: the
        // non-member faces (a nested formal's own parameters, function
        // types, typedefs, primary constructors) declare no body scope.
        // The written type never joins `var_types` — the armed annotation
        // is the formal's RETURN type (`int f()` → `int`), not its own, so
        // `typed_local` keeps answering UNKNOWN; it records in
        // `fn_formal_rets` instead, the face `callee_return_ty` reads for
        // the `f()?.m()` non-null verdict (#1758).
        let fn_typed = std::mem::take(&mut self.engine.fn_typed_formal_open);
        if fn_typed {
            // The formal's own return-type slot was popped by its
            // `end_function_typed_formal_parameter` into
            // `last_fn_formal_ret` — popping here instead would let a
            // nested formal's end steal this formal's slot mid-parse
            // (`List<void Function(dynamic)> f()`'s `dynamic` param ends
            // before the outer `List` head completes, #1758). A `None`
            // means the return annotation never completed (recovery or
            // `var`); the verdict suppresses.
            let ret = self.engine.last_fn_formal_ret.take();
            if !matches!(
                member_kind,
                dartq_parser::MemberKind::FunctionTypedParameter
                    | dartq_parser::MemberKind::GeneralizedFunctionType
                    | dartq_parser::MemberKind::FunctionTypeAlias
                    | dartq_parser::MemberKind::PrimaryConstructor
            ) {
                // #1443: two-step bind — a `borrow()` in the if-chain
                // condition would keep its Ref guard live through the body,
                // so it must run in its own statement before the borrow_mut
                // below.
                let name_id = self.engine.name_of(name);
                if let Some(name_id) = name_id {
                    self.engine.note_parameter(name_id, name.range);
                    if let Some(ret) = ret
                        && let Some(frame) = self.engine.frames.last_mut()
                    {
                        frame.fn_formal_rets.insert(name_id, ret);
                    }
                }
            }
        }

        // The field-formal faces (`_checkForValidField` /
        // `visitSuperFormalParameter`, upstream Dart SDK source,
        // error_verifier.dart): `this.name` initializes a field of the
        // immediately enclosing class — the name must resolve to a declared
        // instance field there (an inherited field does not count, and a
        // getter/setter-only name counts as missing); `super.name` rides
        // the superclass's unnamed constructor's formal table — a positional
        // super formal needs the parameter at its own positional index, a
        // named one the same-spelling named entry. The `this` face gates on
        // a constructor parent (generative or factory — both are
        // `ConstructorDeclaration` upstream, so `ctor_pending` covers both);
        // the `super` face additionally demands a non-redirecting
        // generative constructor (a factory's `superConstructor` is null —
        // `ctor_pending`'s site slot is `None` there — and a non-constructor
        // parent reports `invalid_super_formal_parameter_location` instead,
        // outside this burn set). An extension type's representation formal
        // carries its own face and stays out.
        let in_ctor = self.engine.ctor_pending.is_some()
            || member_kind == dartq_parser::MemberKind::PrimaryConstructor;
        // A `this.`/`super.` formal is a parameter in its own right: its
        // name event arrives under the `FieldInitializer` context (never
        // `FormalParameterDeclaration`), so the parameter bind — the
        // tracked-local entry plus the written type in `var_types` — runs
        // here instead. The written annotation (`String this.p`) types the
        // parameter's reads inside the constructor's own initializer list
        // (`assert(p != null)` reads `String`, not the field's `String?` —
        // upstream binds the initializing formal's element).
        if in_ctor
            && (this_keyword.is_some() || super_keyword.is_some())
            && let Some(name_id) = self.engine.name_of(name)
        {
            self.engine.note_parameter(name_id, name.range);
            // The parameter's declared type: the written annotation when one
            // armed (`String this.p`), else the bound field's own type
            // (`C(this.p)` binds `p` at the field's `String?` — the upstream
            // formal's type reads the field it initializes).
            if self.engine.armed_ty.is_some() {
                self.engine.bind_parameter_type(name_id);
            } else if let Some((container, _)) = self.engine.container_context() {
                let ty = self.engine.member_declared_ty(container, name_id);
                if ty != Ty::UNKNOWN {
                    self.engine.bind_var_type(name_id, ty);
                }
            }
        }
        if self.engine.formal_list_depth == 1
            && in_ctor
            && let Some((container, container_kind)) = self.engine.container_context()
            && matches!(container_kind, ContainerKind::Class | ContainerKind::Enum)
        {
            if let Some(this_kw) = this_keyword {
                self.engine
                    .check_field_formal(container, var_or_final.unwrap_or(this_kw), name);
            } else if super_keyword.is_some()
                && (self
                    .engine
                    .ctor_pending
                    .is_some_and(|(_, site)| site.is_some())
                    || member_kind == dartq_parser::MemberKind::PrimaryConstructor)
            {
                // The association is a resolution product: the initializer
                // list's `super.named(...)` redirect decides the target ctor,
                // so the formal waits for `end_initializers`/`end_constructor`
                // (a secondary constructor) or the primary-constructor body's
                // `this : super.x()` initializer / the class's own close.
                let named = matches!(
                    kind,
                    FormalParameterKind::RequiredNamed | FormalParameterKind::OptionalNamed
                );
                let super_index = if named {
                    u32::MAX
                } else {
                    let index = self.engine.super_formal_positional_index;
                    self.engine.super_formal_positional_index += 1;
                    index
                };
                // The owner tag: the enclosing constructor's decl site —
                // `open_ctor_site` carries the secondary constructor's site
                // during its list and the primary constructor's during the
                // class-header list (`begin_primary_constructor` arms it).
                let owner = self.engine.open_ctor_site;
                if let Some(name_id) = self.engine.name_of(name) {
                    self.engine.pending_super_formals.push((
                        owner,
                        super_index,
                        named,
                        name_id,
                        name.range,
                        container,
                    ));
                }
            }
        }

        // A primary-constructor formal with a `var`/`final` keyword declares
        // a field (`FieldFormalParameterElement.isDeclaring`); `this`/`super`
        // selectors name existing fields instead. The
        // `unused_field_from_primary_constructor` candidates
        // (unused_local_elements_verifier.dart:887-920).
        if member_kind != dartq_parser::MemberKind::PrimaryConstructor {
            return;
        }
        let Some(keyword) = var_or_final else {
            return;
        };
        if this_keyword.is_some() || super_keyword.is_some() {
            return;
        }
        let engine = &mut self.engine;
        // Only class and enum primary constructors report (the official
        // switch's arms); an extension type's representation formal cannot
        // be removed and never reports.
        if !engine
            .container_context()
            .is_some_and(|(_, kind)| matches!(kind, ContainerKind::Class | ContainerKind::Enum))
        {
            return;
        }
        let Some(name_id) = engine.name_of(name) else {
            return;
        };
        let keyword_text = engine.text_of(keyword).unwrap_or_default().to_owned();
        let keyword = engine.intern(&keyword_text);
        // The declaring container scopes the field's read credit (the
        // `unused_field` element-identity face).
        let Some((container, _)) = engine.container_context() else {
            return;
        };
        engine.primary_fields.push(PrimaryFieldSite {
            container,
            name: name_id,
            range: name.range,
            keyword,
        });
    }

    fn end_function_type(&mut self, function: Token, question: Option<Token>) {
        if let Some(depth) = self.engine.type_arg_depth_saved.pop() {
            self.engine.type_arg_depth = depth;
        }
        self.flow().end_function_type(function, question);
        self.engine.var_end_function_type(question.is_some());
        self.engine.fn_capture_end(question.is_some());
        // The #1351 capture: a typedef RHS function type's own close
        // latches its nullability suffix (`typedef W = void Function()?;`
        // expands nullable); a nested function-typed formal's close is
        // not the RHS's own and leaves the latch alone.
        self.engine.bounds.alias_rhs_question(question.is_some());
    }

    fn end_function_typed_formal_parameter(&mut self, token: Token, question: Option<Token>) {
        self.flow()
            .end_function_typed_formal_parameter(token, question);
        // #1463: a nested formal's `end_formal_parameter` consumed the arm
        // this formal's own bind still needs — re-arm. This formal's own
        // take always follows this event (the name and default-value events
        // sit between them; no nested formal list does).
        self.engine.fn_typed_formal_open = true;
        // #1758: the return-type slot closes HERE — this formal's nested
        // list (and any fn-typed formals inside it) closed their own slots
        // before this event. `end_formal_parameter` cannot pop: a param
        // inside the return annotation (`List<void Function(dynamic)>`'s
        // `dynamic`) ends first and would steal the pending slot.
        self.engine.last_fn_formal_ret =
            self.engine.fn_formal_ret_pending.pop().and_then(|(_, t)| t);
    }

    fn end_if_control_flow(&mut self, token: Token) {
        self.flow().end_if_control_flow(token);
    }

    fn end_if_else_control_flow(&mut self, token: Token) {
        self.flow().end_if_else_control_flow(token);
    }

    fn end_if_statement(&mut self, token: Token, else_kw: Option<Token>, end: Token) {
        self.flow().end_if_statement(token, else_kw, end);
        self.engine.if_statement_end();
        self.engine.ur_seal_used();
    }

    fn end_initialized_identifier(&mut self, name: Token) {
        self.flow().end_initialized_identifier(name);
    }

    fn end_invalid_await_expression(&mut self, begin: Token, end: Token) {
        self.flow().end_invalid_await_expression(begin, end);

        // The parser fired `endInvalidAwaitExpression` — `await e` inside a
        // sync (non-async, non-generator) body. The fasta `AwaitInSync`
        // recoverable error converts to `await_in_wrong_context` at the
        // `await` keyword (error_converter.dart). There is no suppression
        // face: the oracle reports it unconditionally.
        self.engine
            .emit_corrected(SemanticCode::AwaitInWrongContext, begin.range, "");
    }

    fn end_invalid_yield_statement(&mut self, begin: Token, star: Option<Token>, end: Token) {
        self.flow().end_invalid_yield_statement(begin, star, end);
    }

    fn end_labeled_statement(&mut self, label_count: usize) {
        self.flow().end_labeled_statement(label_count);

        self.engine.finish_labeled_statement(label_count);
    }

    fn end_pattern_guard(&mut self, token: Token) {
        self.flow().end_pattern_guard(token);
    }

    fn end_rethrow_statement(&mut self, rethrow_kw: Token, end: Token) {
        self.flow().end_rethrow_statement(rethrow_kw, end);
    }

    fn end_switch_statement(&mut self, token: Token, end: Token) {
        self.flow().end_switch_statement(token, end);
    }

    fn end_try_statement(
        &mut self,
        count: usize,
        begin: Token,
        finally_kw: Option<Token>,
        end: Token,
    ) {
        self.flow().end_try_statement(count, begin, finally_kw, end);

        self.engine.finish_try_statement();
    }

    fn end_while_statement(&mut self, token: Token, end: Token) {
        self.flow().end_while_statement(token, end);
    }

    fn end_while_statement_body(&mut self, end: Token) {
        self.flow().end_while_statement_body(end);
    }

    fn end_yield_statement(&mut self, begin: Token, star: Option<Token>, end: Token) {
        self.flow().end_yield_statement(begin, star, end);
        // `YieldStatement` is in the official used set.
        self.engine.ur_seal_used();
        // The generator face (`yield_statement_resolver.dart`
        // `_resolve_generator` → `_checkForYieldOfInvalidType`): the
        // yielded expression (or the `yield*` implied sequence) against
        // the declared sequence's element type. The parser routes the
        // out-of-generator form to `end_invalid_yield_statement` — the
        // `yield_in_non_generator` family owns it, this face never runs
        // there.
        self.engine.check_yield(begin, star, end);
    }

    fn handle_assigned_variable_pattern(&mut self, variable: Token) {
        self.flow().handle_assigned_variable_pattern(variable);

        // An assigned-variable pattern is a reference: the pattern
        // assignment resolves the name through the enclosing scope, and a
        // miss is the oracle's `undefined_identifier` at the name
        // (`[a, var d] = y;` reports `a` — the event is the whole use, so
        // the verdict emits here instead of a deferred row).
        let engine = &mut self.engine;
        let Some(name) = engine.name_of(variable) else {
            return;
        };
        engine.last_pattern_variable = Some((name, variable.range));
        if let Some(hit) = engine.resolve_local(name) {
            engine.locals[hit as usize].uses += 1;
            return;
        }
        let BareInfo::Missing { on_opaque, .. } = engine.resolve_bare(name).0 else {
            return;
        };
        if !on_opaque
            // The conditional-URI face: the configuration's contents never
            // resolve as expressions, so its parse-time writes stay silent
            // (the pass-1 direct emit bypasses the row stack the resolve
            // pass gates).
            && !engine.conditional_uri_window
            && !Engine::synthetic_reference(variable.range)
            && !engine.keyword_bare_reference(variable.range)
            && !engine.guard.ignore(None, name)
        {
            let text = engine.strings.resolve(name).to_string();
            engine.emit(SemanticCode::UndefinedIdentifier, variable.range, &text, "");
        }
        engine.record_miss(name, variable.range, false, NameUse::Write);
    }

    fn handle_break_statement(&mut self, has_target: bool, break_kw: Token, end: Token) {
        self.flow()
            .handle_break_statement(has_target, break_kw, end);
    }

    fn handle_catch_block(
        &mut self,
        on_kw: Option<Token>,
        catch_kw: Option<Token>,
        comma: Option<Token>,
    ) {
        self.flow().handle_catch_block(on_kw, catch_kw, comma);

        self.engine.finish_catch_clause();
    }

    fn handle_continue_statement(&mut self, has_target: bool, continue_kw: Token, end: Token) {
        self.flow()
            .handle_continue_statement(has_target, continue_kw, end);
    }

    fn handle_else_control_flow(&mut self, else_kw: Token, else_start: Token) {
        self.flow().handle_else_control_flow(else_kw, else_start);
    }

    fn handle_empty_function_body(&mut self, semicolon: Token) {
        self.flow().handle_empty_function_body(semicolon);

        // #1314 FAM7: the `;` body's verdict — methods answer
        // `isAbstract` true (drop), top-level functions still expect
        // defaults (flush; only augment/external/native exclude there).
        let engine = &mut self.engine;
        if engine.default_value_depth == 0 {
            engine.flush_default_checks(false);
        }
    }

    fn handle_native_function_body_skipped(&mut self, begin: Token, end: Token) {
        self.flow().handle_native_function_body_skipped(begin, end);
        // #1314 FAM7: native bodies answer `defaultValuesAreExpected`
        // false.
        self.engine.drop_default_checks();
    }

    fn handle_native_function_body_ignored(&mut self, native: Token, next: Token) {
        self.flow()
            .handle_native_function_body_ignored(native, next);
        self.engine.drop_default_checks();
    }

    fn handle_no_function_body(&mut self, token: Token) {
        self.flow().handle_no_function_body(token);
        // Recovery: no body verdict — the rows drop (under-report).
        self.engine.drop_default_checks();
    }

    fn handle_function_body_skipped(&mut self, begin: Token, end: Token, is_expression_body: bool) {
        self.flow()
            .handle_function_body_skipped(begin, end, is_expression_body);
        self.engine.drop_default_checks();
    }

    fn begin_redirecting_factory_body(&mut self, equals: Token) {
        self.flow().begin_redirecting_factory_body(equals);
        {
            let engine = &mut self.engine;
            // #1314 FAM7: a redirecting factory answers
            // `defaultValuesAreExpected` false (error_verifier.dart:8352-8354).
            engine.drop_default_checks();
            // The target reference resolves through the redirect face: its
            // head miss is the oracle's `redirect_to_non_class`
            // (initializer recovery reuses the same body parse — the
            // `this.new = null` shape), never a scored type miss.
            engine.redirect_target_depth += 1;
        }
    }

    fn end_redirecting_factory_body(&mut self, equals: Token, end: Token) {
        self.flow().end_redirecting_factory_body(equals, end);
        let engine = &mut self.engine;
        engine.redirect_target_depth -= 1;
        // The `= C.name` target joins the redirect-edge set keyed on the
        // factory's own constructor spelling — `open_exec_site` carries the
        // site `MethodDeclarationContinuation` (named) or `MethodDeclaration`
        // (unnamed, the head's class spelling — the unnamed-face key).
        // `finalize_redirects`'s cycle walk then reports
        // `recursive_constructor_redirect` at this span for a loop that
        // returns to the factory.
        if let Some((target_container, target, span)) = engine.factory_redirect_target.take()
            && let Some(site) = engine.open_exec_site
            && let Some(container) = engine.container_context().map(|(c, _)| c)
        {
            // The redirect names its target class: a reference for the
            // `unused_element` face (`factory F.a() = _F;` keeps `_F` used).
            engine.used_names.insert(target_container);
            engine
                .factory_redirect_targets
                .insert((target_container, target));
            let ctor = engine.decl_sites[site].name;
            // The redirect-target faces
            // (`_checkForRedirectToInvalidReturnType`,
            // error_verifier.dart:2983-3009, and `_reportRedirectToNonClass`,
            // named_type_resolver.dart:587-594): the head name resolves to a
            // container first — an unresolved or non-container head is the
            // `redirect_to_non_class` row at the target span; a resolved head
            // whose class is not a subtype of the enclosing class is the
            // `redirect_to_invalid_return_type` row (the official `at` is the
            // `factoryRedirectionTarget` = the `C.name` extent). The enclosing
            // face's return type IS the enclosing class itself — the
            // assignability check reduces to a class-vs-class subtype query.
            let target_resolved = resolve_container(
                engine.graph,
                engine.namespaces,
                engine.index,
                engine.lib,
                engine.file,
                target_container,
            );
            match target_resolved {
                None => {
                    let name_text = engine.strings.resolve(target_container).to_string();
                    engine.emit(SemanticCode::RedirectToNonClass, span, &name_text, "");
                }
                Some((target_decl, _)) => {
                    // The faces key on a resolved CONSTRUCTOR element
                    // upstream (`redirectedElement == null` early-returns
                    // into the missing-constructor face — a class with no
                    // matching constructor never reports here): the
                    // selector joins through the container's selector set,
                    // the unnamed spellings (the head itself, `new`)
                    // through the implicit/explicit unnamed face.
                    if engine.ctor_face_exists(target_decl, target_container, target) {
                        // Subtype check: the target's declared type against the
                        // enclosing class's own name (the factory's declared
                        // return type). `declared_subtype_negative` walks the
                        // `from` container's supertype closure — a `Some(false)`
                        // verdict means both resolve and `to` is absent, the
                        // exact under-report shape the official gate picks.
                        let from_ty = Ty::named(target_container);
                        let to_ty = Ty::named(container);
                        if engine.declared_subtype_negative(&from_ty, &to_ty) == Some(false) {
                            let redirected_text = engine.render_ty(from_ty);
                            let redirecting_text = engine.render_ty(to_ty);
                            engine.emit(
                                SemanticCode::RedirectToInvalidReturnType,
                                span,
                                &redirected_text,
                                &redirecting_text,
                            );
                        }
                    }
                }
            }
            engine.redirect_edges.push(RedirectEdge {
                container,
                ctor,
                target_container,
                target,
                span,
                target_span: span,
                factory: true,
                is_const: engine.in_const_ctor,
            });
        }
    }

    fn handle_experiment_not_enabled(&mut self, flag: ExperimentalFlag, begin: Token, end: Token) {
        // The same parse face as `handle_recoverable_error`: the engine
        // l1 collector renders the flag through the parser's message
        // selection — the identical row here.
        let message = dartq_parser::experiment_not_enabled_message(flag);
        self.parse_diagnostics.push(Diagnostic::new(
            Severity::Error,
            message.code,
            message.text,
            self.file,
            begin.range,
            message.correction,
        ));
        self.flow().handle_experiment_not_enabled(flag, begin, end);
    }

    /// The parser fires this after the body expression (the `;` only
    /// exists then), while the flow grammar records the statement wrapper
    /// before the body's events (record.rs `handle_expression_statement`,
    /// the shape its dead-window tests hand-drive). The wrapper therefore
    /// forwards raw — no flush — and the pending exit lands right after
    /// it: a statement whose whole expression is the Never send reports at
    /// the NEXT statement (official `checkUnreachableNode` at the next
    /// statement's entry), never at its own wrapper.
    fn handle_expression_statement(&mut self, begin: Token, end: Token) {
        self.flow.handle_expression_statement(begin, end);
        self.flush_flow_exit();

        // The discarded-value verdict: `ExpressionStatement` is absent from
        // both official `_isUsed` sets (the value is dropped), except the
        // for-condition arm whose statement seals used (the updaters drain
        // at `handle_for_loop_parts`).
        self.engine.ur_expression_statement();
        self.engine.check_statement_use(begin, end);
    }

    fn handle_finally_block(&mut self, finally_kw: Token) {
        self.flow().handle_finally_block(finally_kw);
    }

    fn handle_for_initializer_pattern_variable_assignment(
        &mut self,
        pattern_keyword: Token,
        equals: Token,
    ) {
        self.flow()
            .handle_for_initializer_pattern_variable_assignment(pattern_keyword, equals);
    }

    fn handle_for_loop_parts(
        &mut self,
        for_kw: Token,
        left_paren: Token,
        left_separator: Token,
        right_separator: Token,
        expression_count: usize,
    ) {
        self.flow().handle_for_loop_parts(
            for_kw,
            left_paren,
            left_separator,
            right_separator,
            expression_count,
        );
        // The updaters fired no events: their leftovers are discarded (the
        // official `ForParts` arm spares only the condition, which sealed
        // used at its expression statement), and the window closes — the
        // statement form's own (the collection form pops at its control
        // -flow end).
        let engine = &mut self.engine;
        // The C-`for` condition is a condition context
        // (bool_expression_verifier.dart's forParts face); the parser
        // delivers no condition-end event, so the operand is the last
        // value row between the separators.
        engine.check_condition_operand_within(
            left_separator.range.end(),
            right_separator.range.offset,
        );
        engine.ur_seal_discard();
        if engine.in_for_parts.last() != Some(&2) {
            engine.in_for_parts.pop();
        }
    }

    fn handle_invalid_function_body(&mut self, token: Token) {
        self.flow().handle_invalid_function_body(token);
    }

    fn handle_invalid_statement(&mut self, begin: Token) {
        self.flow().handle_invalid_statement(begin);
    }

    fn handle_label(&mut self, token: Token) {
        self.flow().handle_label(token);

        self.engine.note_label_decl();
    }

    fn handle_no_variable_initializer(&mut self, token: Token) {
        self.flow().handle_no_variable_initializer(token);
    }

    fn handle_null_aware_element(&mut self, token: Token) {
        self.flow().handle_null_aware_element(token);
    }

    fn handle_pattern_assignment(&mut self, equals: Token) {
        self.flow().handle_pattern_assignment(equals);
        // `PatternAssignment` is in the official used set (the event fires
        // after the RHS parsed).
        let engine = &mut self.engine;
        engine.nsm_poison();
        engine.ur_seal_used();
    }

    fn handle_pattern_variable_declaration_statement(
        &mut self,
        keyword: Token,
        equals: Token,
        semicolon: Token,
    ) {
        self.flow()
            .handle_pattern_variable_declaration_statement(keyword, equals, semicolon);

        // `PatternVariableDeclaration` is in the official used set (the
        // event fires after the initializer parsed).
        self.engine.ur_seal_used();
        // The statement's pattern variables join the enclosing statement
        // list's duplicate scope (`checkStatements`'
        // `PatternVariableDeclarationStatement` arm) — the group window
        // still names this statement's set, so the members collect from it
        // before the boundary below closes it. Case-clause pattern
        // variables never fire this event and never join a statement scope.
        if let Some(group) = self.engine.pattern_group_open {
            let vars: Vec<(StringId, SourceRange)> = self
                .engine
                .locals
                .iter()
                .filter(|decl| decl.group == Some(group))
                .map(|decl| (decl.name, decl.range))
                .collect();
            for (name, range) in vars {
                self.engine.dup_scope_add(name, range);
            }
        }
        // The statement boundary closes the group window (the next
        // declaration is its own all-or-nothing set).
        self.engine.pattern_group_open = None;
        // The flat record destructure bind: `final (a, b) = pair()` — the
        // pattern bound no types of its own, but the scrutinee's record
        // carries them; the name-level locals take the positional slots.
        self.engine
            .destructure_record(equals.range.end(), semicolon.range.offset);
    }

    fn handle_recoverable_error(&mut self, p0: Message, p1: Token, p2: Token) {
        // The row joins the fold's parse face (a SYNTACTIC_ERROR, always
        // ERROR-severity, anchored at the reported token) beside the flow
        // forward below.
        self.parse_diagnostics.push(Diagnostic::new(
            Severity::Error,
            p0.code.clone(),
            p0.text.clone(),
            self.file,
            p1.range,
            p0.correction.clone(),
        ));
        self.flow().handle_recoverable_error(p0, p1, p2);
    }

    fn handle_send_without_arguments(&mut self, begin: Token, end: Token, next: Token) {
        self.flow().handle_send_without_arguments(begin, end, next);

        {
            let engine = &mut self.engine;
            engine.nsm_send_poison();
            // A non-invoked identifier expression is a bare-read candidate
            // (the official `checkNameExpression` face): the read of a
            // UseResult-carrying declaration discards here unless its
            // context seals it.
            engine.ur_register_bare_read(begin);
        }
    }

    fn handle_spread_expression(&mut self, operator: Token) {
        self.flow().handle_spread_expression(operator);
        self.engine.const_capture.begin_ctl_element();
    }

    fn handle_switch_case_no_when_clause(&mut self, token: Token) {
        self.flow().handle_switch_case_no_when_clause(token);
        // `SwitchStatement` is in the official used set (the case head's
        // constants); the body statements answer through their own events.
        self.engine.ur_seal_used();
    }

    fn end_switch_case_when_clause(&mut self, _token: Token) {
        // `WhenClause` is in the official used set (the guard expression).
        self.engine.ur_seal_used();
    }

    fn handle_for_initializer_expression_statement(&mut self, _token: Token, _for_in: bool) {
        // The initializer is NOT a use: the official `ForParts` arm
        // intercepts the parent walk before `ForLoopParts` is reached and
        // spares only the condition (`use_result_verifier.dart:168-172`,
        // `return parent.condition2 == node` — upstream Dart SDK source,
        // pkg/analyzer/lib/src/error/use_result_verifier.dart). The
        // `ForLoopParts` arm at :187 catches only the non-`ForParts`
        // parts (for-each's iterable, declared-variable initializers).
        self.engine.ur_seal_discard();
    }

    fn handle_then_control_flow(&mut self, token: Token) {
        self.flow().handle_then_control_flow(token);
        // The element condition's window arms tear down here: the
        // then-element that follows is not a condition.
        self.engine.if_element_cond = self.engine.if_element_cond.saturating_sub(1);
        // The if-element's condition is a condition context too
        // (`handle_ifElement_conditionEnd`, upstream Dart SDK source,
        // `pkg/analyzer/lib/src/dart/resolver/type_analyzer.dart:1153-1166`
        // — the same `checkForNonBoolCondition` the if-statement runs at
        // :1184-1197). The parser fires this event right after the
        // collection form's parenthesized condition
        // (literals.rs `handleThenControlFlow`), so the operand is the
        // last value row — the unchecked-use face (#1351).
        self.engine.check_condition_operand();
    }
}

// —— listener-domain literal & yield faces —————————————————————————————
//
// The typed-literal group parse, the map-entry checks, the brace-literal
// ambiguity decision, and the generator yield check run at the listener
// layer: their decisions read the written source shape (the `<...>` group,
// the entry colons, the direct `=`/`=>`/`return` position) alongside the
// row stack, leaving the engine-side assignability/invocation domains
// untouched.

fn ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

fn trim_ws(bytes: &[u8], mut start: usize, end: usize) -> usize {
    while start < end && bytes[start].is_ascii_whitespace() {
        start += 1;
    }
    start
}

/// The first significant byte at or after `start`: whitespace and both
/// comment forms walk forward. [`preceding_position`]'s forward twin, for
/// the literal element census — upstream reads elements from the AST,
/// where comments do not exist, so a segment that is only trivia (a
/// comment trailing the last element ahead of the `}`) is no element.
fn trim_trivia(bytes: &[u8], mut start: usize, end: usize) -> usize {
    loop {
        while start < end && bytes[start].is_ascii_whitespace() {
            start += 1;
        }
        match comment_end(bytes, start, end) {
            Some(after) => start = after,
            None => return start,
        }
    }
}

/// The source position one past the last significant byte before `cursor`:
/// whitespace and both comment forms walk back over. The direct-position
/// gate's scanner.
fn preceding_position(bytes: &[u8], mut cursor: usize) -> Option<usize> {
    loop {
        while cursor > 0 && bytes[cursor - 1].is_ascii_whitespace() {
            cursor -= 1;
        }
        if cursor >= 2 && bytes[cursor - 1] == b'*' && bytes[cursor - 2] == b'/' {
            let mut i = cursor - 2;
            while i > 0 && !(bytes[i - 1] == b'/' && bytes[i] == b'*') {
                i -= 1;
            }
            if i == 0 {
                return None;
            }
            cursor = i - 1;
            continue;
        }
        if cursor >= 2 && bytes[cursor - 1] == b'/' && bytes[cursor - 2] == b'/' {
            let mut i = cursor - 2;
            while i > 0 && bytes[i - 1] != b'\n' {
                i -= 1;
            }
            cursor = i;
            continue;
        }
        return if cursor == 0 { None } else { Some(cursor) };
    }
}

/// Top-level comma segments between `start` and `end`: bracket depth over
/// `()[]{}`, string contents skipped; `angles` folds `<`/`>` into the
/// depth (type-argument groups carry no comparison operators, element
/// lists do).
fn top_level_comma_segments(
    bytes: &[u8],
    start: usize,
    end: usize,
    angles: bool,
) -> Vec<(usize, usize)> {
    let mut segments = Vec::new();
    let mut seg_start = start;
    let mut i = start;
    let mut depth = 0usize;
    while i < end {
        match bytes[i] {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => depth = depth.saturating_sub(1),
            b'<' if angles => depth += 1,
            b'>' if angles => depth = depth.saturating_sub(1),
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
    segments
}

/// The byte just past the matching `>` of the type-argument group opening
/// at `lt`, when the `<` opens a genuine `<...>` type group rather than a
/// comparison. The scan folds nested `<>` and accepts only type-permissible
/// bytes between the angles; the closing `>` must be followed by a token a
/// typed literal or type-use admits — `{` (typed literal body),
/// `(`/`,`/`)`/`]`/`}`/`:`/`;`/`=`/`>` or end-of-span. Any operator byte,
/// or a `>` followed by an operand (a comparison's right side), returns
/// `None`, leaving the `<`/`>` a plain comparison.
fn angle_group_end(bytes: &[u8], lt: usize, end: usize) -> Option<usize> {
    debug_assert_eq!(bytes[lt], b'<');
    let mut depth = 0usize;
    let mut i = lt;
    while i < end {
        match bytes[i] {
            b'<' => depth += 1,
            b'>' => {
                depth -= 1;
                if depth == 0 {
                    let mut j = i + 1;
                    while j < end && bytes[j].is_ascii_whitespace() {
                        j += 1;
                    }
                    return match bytes.get(j) {
                        None | Some(b',') | Some(b'}') | Some(b')') | Some(b']') | Some(b'{')
                        | Some(b'(') | Some(b':') | Some(b';') | Some(b'=') | Some(b'>') => {
                            Some(i + 1)
                        }
                        _ => None,
                    };
                }
            }
            // Anything outside a type-use's byte set disqualifies the group:
            // strings, arithmetic/comparison/logical operators, brackets.
            b'\'' | b'"' | b'!' | b'&' | b'|' | b'+' | b'-' | b'*' | b'/' | b'%' | b'^' | b'='
            | b'~' | b'(' | b')' | b'[' | b']' | b'{' | b'}' | b':' | b';' => return None,
            _ => {}
        }
        i += 1;
    }
    None
}

/// Top-level comma segments between `start` and `end` for a literal's
/// element list: bracket depth over `()[]{}`, string contents skipped, and
/// a genuine `<...>` type-argument group folded into the depth so a typed
/// literal's value (`key: <int, dynamic>{}`) stays a single leaf. A
/// comparison's `<`/`>` (no valid group per [`angle_group_end`]) does NOT
/// fold — `a < b, c > d` still splits on its element commas.
fn literal_comma_segments(bytes: &[u8], start: usize, end: usize) -> Vec<(usize, usize)> {
    let mut segments = Vec::new();
    let mut seg_start = start;
    let mut i = start;
    let mut depth = 0usize;
    while i < end {
        match bytes[i] {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => depth = depth.saturating_sub(1),
            b'<' => {
                if let Some(close) = angle_group_end(bytes, i, end) {
                    i = close;
                    continue;
                }
            }
            b',' if depth == 0 => {
                segments.push((seg_start, i));
                seg_start = i + 1;
            }
            b'/' => {
                if let Some(after) = comment_end(bytes, i, end) {
                    i = after;
                    continue;
                }
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
    segments
}

/// The byte just past the comment opening at `i` (`//` runs to the end of
/// the line, `/*` to its `*/`), or `None` when `i` is no comment head.
/// Upstream reads elements from the AST, where comments do not exist; a
/// comment's `:`/`,`/`<` bytes must not steer the byte scans.
fn comment_end(bytes: &[u8], i: usize, end: usize) -> Option<usize> {
    if bytes.get(i) != Some(&b'/') {
        return None;
    }
    match bytes.get(i + 1) {
        Some(b'/') => {
            let mut j = i + 2;
            while j < end && bytes[j] != b'\n' {
                j += 1;
            }
            Some(j)
        }
        Some(b'*') => {
            let mut j = i + 2;
            while j + 1 < end {
                if bytes[j] == b'*' && bytes[j + 1] == b'/' {
                    return Some(j + 2);
                }
                j += 1;
            }
            Some(end)
        }
        _ => None,
    }
}

/// The first top-level `:` in `[start, end)`, or `None`.
fn top_level_colon(bytes: &[u8], start: usize, end: usize) -> Option<usize> {
    let mut i = start;
    let mut depth = 0usize;
    while i < end {
        match bytes[i] {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => depth = depth.saturating_sub(1),
            b':' if depth == 0 => return Some(i),
            b'/' => {
                if let Some(after) = comment_end(bytes, i, end) {
                    i = after;
                    continue;
                }
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
    None
}

/// The first top-level `?` in `[start, end)`, or `None`.
fn top_level_question(bytes: &[u8], start: usize, end: usize) -> Option<usize> {
    let mut i = start;
    let mut depth = 0usize;
    while i < end {
        match bytes[i] {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => depth = depth.saturating_sub(1),
            b'?' if depth == 0 => return Some(i),
            b'/' => {
                if let Some(after) = comment_end(bytes, i, end) {
                    i = after;
                    continue;
                }
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
    None
}

/// The byte extent of the balanced `(`...`)` group opening at `open`.
fn balanced_paren(bytes: &[u8], open: usize, end: usize) -> Option<usize> {
    let mut depth = 0usize;
    let mut i = open;
    while i < end {
        match bytes[i] {
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
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
    None
}

/// A word-bounded needle at the top of `[start, end)`.
fn find_word(bytes: &[u8], start: usize, end: usize, needle: &[u8]) -> Option<usize> {
    let mut search = start;
    while let Some(at) = bytes[search..end]
        .windows(needle.len())
        .position(|w| w == needle)
    {
        let at = search + at;
        let before_ok = at == start || !ident_byte(bytes[at - 1]);
        let after = at + needle.len();
        let after_ok = after == end || !ident_byte(bytes[after]);
        if before_ok && after_ok {
            return Some(at);
        }
        search = at + 1;
        if search >= end {
            return None;
        }
    }
    None
}

/// Whether the byte before position `eq` makes the `=` a compound
/// (`==`, `??=`, `+=`, `<=`, ...) rather than the plain assignment.
fn compound_assignment(bytes: &[u8], eq: usize) -> bool {
    eq >= 2
        && matches!(
            bytes[eq - 2],
            b'=' | b'!'
                | b'<'
                | b'>'
                | b'+'
                | b'-'
                | b'*'
                | b'/'
                | b'%'
                | b'&'
                | b'|'
                | b'^'
                | b'?'
        )
}

/// The spread operand's leading bare identifier: its name when the span
/// opens with an identifier token (`l`, `a.b`, `a(1)` — the resolution
/// channels answer the head name), `None` for operands opening with any
/// other token (literals, parens).
fn leading_ident_name(engine: &Engine<'_>, start: usize, end: usize) -> Option<StringId> {
    let bytes = engine.source.as_bytes();
    let mut cursor = start;
    if cursor < end
        && (bytes[cursor].is_ascii_alphabetic() || bytes[cursor] == b'_' || bytes[cursor] == b'$')
    {
        cursor += 1;
    } else {
        return None;
    }
    while cursor < end && (ident_byte(bytes[cursor]) || bytes[cursor] == b'$') {
        cursor += 1;
    }
    let text = std::str::from_utf8(&bytes[start..cursor]).ok()?;
    Some(engine.strings.intern(text))
}

/// The written group's span for the typed literal opening at `left`: the
/// gate (the byte before the bracket closes a written `<>` group, never an
/// expression arrow) plus the balanced walk to the group's `<` head.
fn typed_group_head(engine: &Engine<'_>, left: Token) -> Option<usize> {
    let bytes = engine.source.as_bytes();
    let mut cursor = engine.byte_of(left.range.offset);
    while cursor > 0 && bytes[cursor - 1].is_ascii_whitespace() {
        cursor -= 1;
    }
    if cursor < 2 || bytes[cursor - 1] != b'>' || bytes[cursor - 2] == b'=' {
        return None;
    }
    let mut depth = 0usize;
    loop {
        if cursor == 0 {
            return None;
        }
        cursor -= 1;
        match bytes[cursor] {
            b'>' => depth += 1,
            b'<' => {
                depth -= 1;
                if depth == 0 {
                    return Some(cursor);
                }
            }
            _ => {}
        }
    }
}

/// The written-argument tree a staged assignment RHS type carries (#2364):
/// a named entry spells its lexeme, nullability, and its own nested tree;
/// `dynamic` and the not-a-type face (`Ty::Unknown` — an unresolved
/// spelling, the oracle's `InvalidType` element) spell their special
/// faces. Any other entry (a record or function shape) cannot stage
/// faithfully, so the whole tree drops — the unknown-written demotion
/// stands, the conservative face for an unspellable tree.
pub(super) fn staged_arg_tree(
    pool: &ArgPool,
    strings: &StringTable,
    args: ArgListId,
) -> Option<Vec<dartq_flow::ty::StagedArg>> {
    pool.get(args)
        .iter()
        .map(|ty| match *ty {
            Ty::Named {
                name,
                nullable,
                args,
            } => Some(dartq_flow::ty::StagedArg::Named {
                lexeme: strings.resolve(name).to_string(),
                nullable,
                args: staged_arg_tree(pool, strings, args)?,
            }),
            Ty::Dynamic => Some(dartq_flow::ty::StagedArg::Dynamic),
            Ty::Unknown => Some(dartq_flow::ty::StagedArg::Invalid),
            _ => None,
        })
        .collect()
}

impl Engine<'_> {
    /// Whether the typed literal's written `<...>` group carries nested
    /// type arguments — the collapse repair's trigger (the flat segment
    /// scan attributes nested groups' type-uses to the wrong segment).
    pub(crate) fn group_has_nested_args(&self, left: Token) -> bool {
        let Some(head) = typed_group_head(self, left) else {
            return false;
        };
        let bytes = self.source.as_bytes();
        let end = self.byte_of(left.range.offset);
        let mut depth = 0usize;
        for &b in &bytes[head + 1..end] {
            match b {
                b'<' => {
                    if depth == 0 {
                        return true;
                    }
                    depth += 1;
                }
                b'>' => depth = depth.saturating_sub(1),
                _ => {}
            }
        }
        false
    }

    /// The written `<...>` group's parsed argument types: each top-level
    /// segment's head type-use plus its own nested group, recursing per
    /// argument level. A shape the name-level model cannot spell (a
    /// function-typed or record argument) bails — the literal's row stays
    /// opaque (the under-report posture).
    pub(crate) fn literal_group_args(&self, left: Token) -> Option<Vec<Ty>> {
        let head = typed_group_head(self, left)?;
        // The group's own closing `>` (gate-guaranteed the byte before the
        // literal bracket) stays outside the segment scan.
        let close = self.byte_of(left.range.offset) - 1;
        self.parse_group_args(head + 1, close)
    }

    /// The written group's `<` head as a UTF-16 offset — the typed
    /// literal's row anchor start.
    pub(crate) fn typed_group_head_u16(&self, left: Token) -> Option<u32> {
        typed_group_head(self, left).map(|head| self.utf16_of(head))
    }

    fn parse_group_args(&self, start: usize, end: usize) -> Option<Vec<Ty>> {
        let segments = top_level_comma_segments(self.source.as_bytes(), start, end, true);
        let mut args = Vec::with_capacity(segments.len());
        for (s, e) in segments {
            args.push(self.parse_type_arg(s, e)?);
        }
        Some(args)
    }

    fn parse_type_arg(&self, start: usize, end: usize) -> Option<Ty> {
        let bytes = self.source.as_bytes();
        let start = trim_ws(bytes, start, end);
        // The head is the segment's FIRST type-use (textual order — the
        // head precedes its arguments; the prefix form `p.C` ships one use
        // at `C`).
        let s16 = self.utf16_of(start);
        let e16 = self.utf16_of(end);
        let &(range, name) = self
            .type_uses
            .iter()
            .find(|(r, _)| r.offset >= s16 && r.end() <= e16)?;
        if !matches!(self.type_resolution(name), (TypeResolution::Type, _)) {
            return None;
        }
        let nullable = self.written_nullable_suffix(range.end());
        let mut cursor = self.byte_of(range.end());
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
            // The head's own nested group: balanced to its `>`, and
            // nothing may follow it inside the segment.
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
            let args = self.parse_group_args(c + 1, close)?;
            self.boundary.args.borrow_mut().intern(&args)
        } else {
            if c != end {
                return None;
            }
            crate::model::ArgListId::EMPTY
        };
        Some(Ty::Named {
            name,
            nullable,
            args,
        })
    }

    /// The map literal's entry face
    /// (`literal_element_verifier.dart` `_verifyMapLiteralEntry`): each
    /// entry's key and value rows ask their assignability question of the
    /// literal's resolved type arguments, reporting
    /// `map_key_type_not_assignable` / `map_value_type_not_assignable` at
    /// the failing side. Leaves without a top-level colon (spreads,
    /// expression elements), the conditional shape, and the null-aware
    /// entry (its promotion asks a different question) stay silent.
    pub(crate) fn check_map_literal_entries(&mut self, left: Token, right: Token, k: Ty, v: Ty) {
        for (key, value) in self.entry_spans(left, right) {
            for ((start, end), expected, code) in [
                (key, k, SemanticCode::MapKeyTypeNotAssignable),
                (value, v, SemanticCode::MapValueTypeNotAssignable),
            ] {
                let Some((ty, range)) = self.leaf_completion_row(start, end) else {
                    continue;
                };
                // A `void` key/value short-circuits to the use-of-void
                // face upstream (`_verifyMapLiteralEntryKey`) — no entry
                // row; an unspellable completion row suppresses.
                if ty == Ty::Void || ty == Ty::UNKNOWN {
                    continue;
                }
                if self.assignable(ty, expected, false) == Some(false) {
                    let (a0, a1) = (self.ty_display(ty), self.ty_display(expected));
                    self.emit(code, range, &a0, &a1);
                }
            }
        }
    }

    /// The entry key/value spans (UTF-16) of a map-shaped literal's
    /// leaves.
    fn entry_spans(&self, left: Token, right: Token) -> Vec<((u32, u32), (u32, u32))> {
        let bytes = self.source.as_bytes();
        let mut out = Vec::new();
        for (s, e) in self.literal_leaves(left, right) {
            let s = trim_ws(bytes, s, e);
            let Some(colon) = top_level_colon(bytes, s, e) else {
                continue;
            };
            // The null-aware entry's key promotes to non-null (a
            // different question); a conditional's `?` makes the leaf an
            // expression — both stay silent here.
            if top_level_question(bytes, s, colon).is_some() {
                continue;
            }
            let key = (self.utf16_of(s), self.utf16_of(colon));
            let value = (self.utf16_of(colon + 1), self.utf16_of(e));
            out.push((key, value));
        }
        out
    }

    /// The brace literal's ambiguity decision
    /// (`typed_literal_resolver.dart` `_inferSetOrMapLiteralType`'s tail):
    /// elements that must be map sides (entries, resolved map spreads) and
    /// elements that must be set sides (expressions, resolved iterable
    /// spreads) mixed together report
    /// `ambiguous_set_or_map_literal_both`; no element forcing either side
    /// while some element is positively undecided — a spread whose
    /// expression row reads `dynamic`/non-collection, or an unresolvable
    /// spread operand (the oracle's error-dynamic shape) — reports
    /// `_either` (`_InferredCollectionElementTypeInformation`: a dynamic
    /// spread fills all three slots, forcing neither must-be bit). One
    /// side only decides silently, and the elementless form defaults to a
    /// map. An operand with NO row resolves to a type the row model
    /// cannot spell — its hidden side may already have decided the whole
    /// literal in the oracle, so it suppresses the nothing-forces `_either`
    /// tail (the under-report posture; a resolved collection spread is
    /// this shape's common occupant).
    pub(crate) fn check_set_or_map_ambiguity(
        &mut self,
        left: Token,
        const_keyword: Option<Token>,
        right: Token,
        count: usize,
    ) {
        if count == 0 {
            return;
        }
        let mut has_map = false;
        let mut has_set = false;
        let mut either_evidence = false;
        let mut hidden_side = false;
        for (start, end) in self.literal_leaves(left, right) {
            match self.leaf_side(start, end) {
                LeafSide::Map => has_map = true,
                LeafSide::Set => has_set = true,
                LeafSide::Either => either_evidence = true,
                LeafSide::Blind => hidden_side = true,
            }
        }
        let code = match (has_map, has_set) {
            (true, true) => SemanticCode::AmbiguousSetOrMapLiteralBoth,
            (false, false) if either_evidence && !hidden_side => {
                SemanticCode::AmbiguousSetOrMapLiteralEither
            }
            _ => return,
        };
        let span_start = const_keyword
            .map(|token| token.range.offset)
            .unwrap_or(left.range.offset);
        let range = SourceRange {
            offset: span_start,
            length: right.range.end() - span_start,
        };
        let message = code.message("", "");
        self.emit_message_with_correction(code, range, message);
    }

    /// One leaf's must-be side (`_inferCollectionElementType`'s
    /// `mustBeMap`/`mustBeSet` bits).
    fn leaf_side(&self, start: usize, end: usize) -> LeafSide {
        let bytes = self.source.as_bytes();
        let s = trim_ws(bytes, start, end);
        if bytes[s..].starts_with(b"...") {
            // The spread expression's row decides (`SpreadElementImpl`'s
            // arm): a resolved `Map` spread must be a map side, a resolved
            // iterable-family spread a set side; a `dynamic`/non-collection
            // row fills the can-be slots without forcing either must-be
            // bit — positive `_either` evidence.
            let s16 = self.utf16_of(s);
            let e16 = self.utf16_of(end);
            let after = trim_ws(
                bytes,
                s + 3 + usize::from(bytes.get(s + 3) == Some(&b'?')),
                end,
            );
            if let Some((ty, _)) = self.row_in_span(s16, e16) {
                let Ty::Named { name, .. } = ty else {
                    return LeafSide::Either;
                };
                if name == self.collection_heads[2] {
                    return LeafSide::Map;
                }
                if name == self.collection_heads[0]
                    || name == self.collection_heads[1]
                    || name == self.collection_heads[3]
                {
                    return LeafSide::Set;
                }
                return LeafSide::Either;
            }
            // No row: the operand's type is invisible. An unresolvable
            // operand is the oracle's error-dynamic shape (positive
            // `_either` evidence); a resolved one hides a real type whose
            // side may already have decided the literal — blind.
            if let Some(name) = leading_ident_name(self, after, end) {
                if self.resolve_local(name).is_none()
                    && matches!(self.resolve_bare(name).0, BareInfo::Missing { .. })
                {
                    return LeafSide::Either;
                }
            }
            return LeafSide::Blind;
        }
        match top_level_colon(bytes, s, end) {
            Some(colon) => {
                // A conditional's mid-leaf `?` ahead of the colon makes
                // the leaf an expression; the plain colon is an entry.
                if top_level_question(bytes, s, colon).is_some() {
                    LeafSide::Set
                } else {
                    LeafSide::Map
                }
            }
            None => LeafSide::Set,
        }
    }

    /// The literal's leaf elements (byte spans): top-level comma segments
    /// with `for`/`if` wrappers unwrapped to their bodies.
    fn literal_leaves(&self, left: Token, right: Token) -> Vec<(usize, usize)> {
        let bytes = self.source.as_bytes();
        let start = self.byte_of(left.range.end());
        let end = self.byte_of(right.range.offset);
        let mut leaves = Vec::new();
        let mut work: Vec<(usize, usize)> = literal_comma_segments(bytes, start, end);
        while let Some((s, e)) = work.pop() {
            // Trivia is not an element (upstream census walks the AST's
            // element list): a comment-only segment — a line/doc/block
            // comment trailing the last element ahead of the `}` — yields
            // no leaf, and a comment-led segment starts at its first
            // significant byte (#2458).
            let s = trim_trivia(bytes, s, e);
            if s >= e {
                continue;
            }
            // `for (...) body`: the body follows the clause's balanced `)`.
            if bytes[s..].starts_with(b"for") {
                let after_for = trim_ws(bytes, s + 3, e);
                if after_for < e && bytes[after_for] == b'(' {
                    let close = balanced_paren(bytes, after_for, e);
                    if let Some(close) = close {
                        work.push((close + 1, e));
                        continue;
                    }
                }
            }
            // `if (c) then [else else-part]`: both branches unwrap.
            if bytes[s..].starts_with(b"if") {
                let after_if = trim_ws(bytes, s + 2, e);
                if after_if < e && bytes[after_if] == b'(' {
                    let close = balanced_paren(bytes, after_if, e);
                    if let Some(close) = close {
                        let else_at = find_word(bytes, close + 1, e, b"else");
                        match else_at {
                            Some(at) => {
                                work.push((close + 1, at));
                                work.push((at + 4, e));
                            }
                            None => work.push((close + 1, e)),
                        }
                        continue;
                    }
                }
            }
            leaves.push((s, e));
        }
        leaves
    }

    /// The last typed row inside `[start, end)` (UTF-16) — the spread
    /// leaf's expression-row selection.
    fn row_in_span(&self, start: u32, end: u32) -> Option<(Ty, SourceRange)> {
        let row = self.rows.iter().rev().find(|row| {
            row.ty != Ty::UNKNOWN && row.range.offset >= start && row.range.end() <= end
        })?;
        Some((row.ty, row.range))
    }

    /// The leaf's completion row: the LAST row inside the span, whatever
    /// its type — the value expression's own node row. A nested
    /// collection's opaque completion row outlives its inner typed rows,
    /// so the completion pick answers nothing exactly where the oracle
    /// checks the nested literal's own type and the row cannot spell it
    /// (the under-report posture); a nested TYPED completion row reads as
    /// the value's static type.
    fn leaf_completion_row(&self, start: u32, end: u32) -> Option<(Ty, SourceRange)> {
        let row = self.rows.iter().rev().find(|row| {
            row.range.offset >= start
                && row.range.end() <= end
                && !matches!(row.shape, RowShape::TypeUse(..))
                && !self.interpolation_child_rows.contains(&row.range.offset)
        })?;
        Some((row.ty, row.range))
    }

    /// The context side the literal's direct position donates (`Some(true)`
    /// = map, `Some(false)` = set): the declaration group's armed
    /// annotation at the direct `=` position, or the host's declared
    /// return at the direct `=>`-body / `return` position — the two
    /// channels the name-level model carries
    /// (`typed_literal_resolver.dart` `_fromContextType`).
    pub(crate) fn literal_context_side(&self, left: Token) -> Option<bool> {
        let Ty::Named { name, .. } = self.literal_context_ty(left)? else {
            return None;
        };
        if name == self.collection_heads[2] {
            return Some(true);
        }
        if name == self.collection_heads[0]
            || name == self.collection_heads[1]
            || name == self.collection_heads[3]
        {
            return Some(false);
        }
        None
    }

    /// The written 2-argument `Map<K, V>` context at the literal's
    /// direct position, split into its type arguments — the entry face's
    /// expected key/value pair.
    pub(crate) fn map_entry_context(&self, left: Token) -> Option<(Ty, Ty)> {
        let Ty::Named { name, args, .. } = self.literal_context_ty(left)? else {
            return None;
        };
        if name != self.collection_heads[2] {
            return None;
        }
        let args = self.boundary.args.borrow().get(args).to_vec();
        match args.as_slice() {
            [k, v] => Some((*k, *v)),
            _ => None,
        }
    }

    fn literal_context_ty(&self, left: Token) -> Option<Ty> {
        let bytes = self.source.as_bytes();
        let cursor = preceding_position(bytes, self.byte_of(left.range.offset))?;
        match bytes[cursor - 1] {
            // The declaration's own initializer: the group is open and no
            // nested executable sits between (`init_frame_depth` latched
            // the initializer world's frame count — a closure body's
            // `=` inside is its own business).
            b'=' if !compound_assignment(bytes, cursor) => {
                if self.in_initializer && self.frames.len() == self.init_frame_depth {
                    self.pending_decl_ty
                } else {
                    None
                }
            }
            // The arrow-body expression.
            b'>' if cursor >= 2 && bytes[cursor - 2] == b'=' => self.host_return_ty(),
            // The bare `return e;` expression.
            b'n' if cursor >= 6
                && bytes[cursor - 6..cursor] == *b"return"
                && (cursor == 6 || !ident_byte(bytes[cursor - 7])) =>
            {
                self.host_return_ty()
            }
            _ => None,
        }
    }

    /// The innermost executable's declared return — a closure frame's
    /// uninferred return never delegates outward (`check_return`'s
    /// `firstDepleted` posture).
    fn host_return_ty(&self) -> Option<Ty> {
        let frame = self
            .frames
            .iter()
            .rev()
            .find(|f| matches!(f.kind, FrameKind::Executable { .. }))?;
        let FrameKind::Executable { ret, .. } = &frame.kind else {
            return None;
        };
        ret.as_ref().map(|r| r.ty)
    }

    /// The generator yield face
    /// (`yield_statement_resolver.dart` `_checkForYieldOfInvalidType`):
    /// `yield e` asks e's type of the declared sequence's element type;
    /// `yield* e` asks the implied `Iterable<E>`/`Stream<E>` of the full
    /// declared return. A declared return outside the body's own sequence
    /// family answers nothing (upstream `asInstanceOf` miss), and the
    /// closure generator's uninferred return under-reports.
    pub(crate) fn check_yield(&mut self, begin: Token, star: Option<Token>, end: Token) {
        let Some(frame) = self
            .frames
            .iter()
            .rev()
            .find(|f| matches!(f.kind, FrameKind::Executable { .. }))
        else {
            return;
        };
        let FrameKind::Executable {
            ret,
            is_async,
            generator,
            ..
        } = &frame.kind
        else {
            return;
        };
        if !*generator {
            return;
        }
        let Some(ret) = ret else { return };
        let is_sync = !*is_async;
        let Some((expr_ty, range)) =
            self.single_value_row_between(begin.range.end(), end.range.offset)
        else {
            return;
        };
        let iterable = self.collection_heads[3];
        let stream = self.intern("Stream");
        let seq_head = if is_sync { iterable } else { stream };
        if star.is_some() {
            // The yield-each ask is the RAW expression type against the
            // declared return (`_checkForYieldOfInvalidType`'s isYieldEach
            // face: `impliedReturnType = expressionType` — no sequence
            // wrap). A subtype head carrying its own fixed instantiation
            // (`XPathSequence extends Iterable<XPathItem>`) answers through
            // its clause, while a wrap would ask the element-vs-element
            // covariance question the oracle never asks.
            if self.assignable(expr_ty, ret.ty, false) == Some(false) {
                // Staged, not emitted: the oracle's verdict reads the
                // source's flow-aware static type
                // (`yield_statement_resolver.dart` `expression.typeOrThrow`
                // — probe2484: a null-guard-promoted `yield* src` stays
                // silent), so the promoted read re-decides at the teardown
                // join with the promoted type as the implied return.
                self.stage(StagedKind::YieldEach, range, expr_ty, ret.ty, false);
            }
            return;
        }
        let Ty::Named { name, args, .. } = ret.ty else {
            return;
        };
        if name != seq_head {
            return;
        }
        let Some(&e) = self.boundary.args.borrow().get(args).first() else {
            return;
        };
        if self.assignable(expr_ty, e, false) == Some(false) {
            // Staged, not emitted: the yield expression's promoted read
            // re-decides at the teardown join — a generator body's top
            // level is no demotion point, so the `??=`/guard promotion
            // stands for the verdict (the same face `check_return` stages;
            // the official row reads the flow-aware static type).
            self.stage(StagedKind::Yield, range, expr_ty, e, false);
        }
    }

    /// The statement window's single value row — the exact-one-row posture
    /// [`Engine::single_row_since`] shares, bounded by token spans instead
    /// of a mark.
    fn single_value_row_between(&self, after: u32, until: u32) -> Option<(Ty, SourceRange)> {
        let rows: Vec<&Row> = self
            .rows
            .iter()
            .filter(|row| row.range.offset >= after && row.range.end() <= until)
            .filter(|row| !matches!(row.shape, RowShape::TypeUse(..)))
            .filter(|row| !self.interpolation_child_rows.contains(&row.range.offset))
            .collect();
        match rows.as_slice() {
            [row] => Some((row.ty, row.range)),
            _ => None,
        }
    }
}

/// A leaf element's must-be side for the ambiguity decision.
#[derive(Clone, Copy, PartialEq, Eq)]
enum LeafSide {
    Map,
    Set,
    /// No side forced, but the leaf is positively undecided in the
    /// oracle too (a `dynamic`/non-collection spread row, an
    /// unresolvable operand's error-dynamic): `_either` evidence.
    Either,
    /// The leaf's type is invisible to the row model (no row at the
    /// operand): its hidden real side may already have decided the
    /// literal, so it forces nothing and vetoes the `_either` tail.
    Blind,
}
