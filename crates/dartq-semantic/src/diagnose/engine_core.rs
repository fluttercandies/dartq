use super::*;

impl<'a> Engine<'a> {
    // —— plumbing ————————————————————————————————————————————————————

    #[allow(clippy::too_many_arguments)] // the analysis context, one field each
    #[expect(clippy::too_many_lines)] // long diagnostic-table dispatch
    pub(crate) fn new(
        graph: &'a LibraryGraph,
        namespaces: &'a Namespaces,
        index: &'a ElementIndex,
        strings: &'a StringTable,
        source: &'a str,
        tokens: &'a TokenTable,
        file: FileId,
        lib: LibraryId,
        primary_constructors_enabled: bool,
        variance_enabled: bool,
        field_promotion_enabled: bool,
        patterns_enabled: bool,
        strict: StrictModes,
        class_modifiers: &'a LibraryClassModifiers,
        ops: Option<&'a crate::resolved_ops::ResolvedTypeOps<'a>>,
    ) -> Self {
        let guard = UndefinedGuard::build(graph, lib);
        let identical_id = strings.intern("identical");
        let unit_args = index.get(file).map(FileElements::type_args);
        let boundary = Boundary::new(index, file, unit_args);
        Self {
            graph,
            namespaces,
            index,
            strings,
            boundary,
            source,
            tokens,
            wide_chars: std::cell::OnceCell::new(),
            prefix_names: std::cell::OnceCell::new(),
            prefix_head_memo: std::cell::RefCell::new(FxHashMap::default()),
            numeric_shadow_memo: std::array::from_fn(|_| std::cell::Cell::new(None)),
            file,
            lib,
            guard,
            report: FileReport::default(),
            rows: Vec::new(),
            refs: Vec::new(),
            type_uses: Vec::new(),
            frames: Vec::new(),
            dup_scopes: Vec::new(),
            dup_for_parts: FxHashMap::default(),
            untyped_literals: FxHashMap::default(),
            collection_heads: [
                strings.intern("List"),
                strings.intern("Set"),
                strings.intern("Map"),
                strings.intern("Iterable"),
            ],
            type_params: Vec::new(),
            pending_type_params: Vec::new(),
            pending_rows_mark: 0,
            deferred_miss_rows: Vec::new(),
            bounds: Bounds::new(),
            pending_bounded_checks: Vec::new(),
            raw_type_sites: Vec::new(),
            staged_checks: Vec::new(),
            cast_nullable: Vec::new(),
            body_completions: Vec::new(),
            armed_ty: None,
            armed_ty_unresolved: false,
            armed_ty_span: None,
            pending_decl_ty: None,
            pending_decl_ty_unresolved: false,
            ctor_init_writes: FxHashSet::default(),
            open_ctor_site: None,
            ctor_formals_init: FxHashSet::default(),
            ctor_super_init: FxHashSet::default(),
            ctor_redirects: FxHashSet::default(),
            ctor_head_anchors: FxHashMap::default(),
            factory_head: None,
            factory_head_anchors: FxHashMap::default(),
            unnamed_factory_heads: Vec::new(),
            factory_redirect_targets: FxHashSet::default(),
            group_heads: Vec::new(),
            pending_type_head: None,
            deferred_args: Vec::new(),
            arg_levels: Vec::new(),
            in_enum_element: None,
            enum_const_call: None,
            body_mark: None,
            init_mark: None,
            return_mark: None,
            switch: SwitchTracker::new(),
            pending_binary_operand: None,
            const_fold: ConstTracker::new(),
            null_cmp_groups: Vec::new(),
            in_cascade: false,
            invokes: Vec::new(),
            pending_send: None,
            last_send_callee: None,
            recovered_send: None,
            is_type_mark: None,
            as_type_mark: None,
            pending_mixin_uses: Vec::new(),
            record_marks: Vec::new(),
            record_literal_names: Vec::new(),
            record_ty_stack: Vec::new(),
            type_prefix: None,
            pending_prefixed: None,
            non_type_pos: 0,
            is_type_pos: 0,
            ctor_pos: 0,
            implicit_creation_depth: 0,
            creation_ref: None,
            ctor_prefix_heads: Vec::new(),
            redirect_target_depth: 0,
            factory_redirect_target: None,
            factory_ret_open: false,
            ctor_new_head_kills: FxHashSet::default(),
            pending_unnamed_enum_consts: Vec::new(),
            enum_decl_site: None,
            enum_unnamed_ctor_uses: Vec::new(),
            literal_ctor_arms: Vec::new(),
            literal_ctor_stash: Vec::new(),
            default_value_depth: 0,
            formal_written: None,
            formal_written_saved: None,
            formal_list_depth: 0,
            pending_super_formals: Vec::new(),
            super_formal_positional_index: 0,
            primary_ctor_site: None,
            fn_typed_formal_open: false,
            fn_formal_ret_pending: Vec::new(),
            last_fn_formal_ret: None,
            formal_colon: None,
            formal_has_default: false,
            open_exec_augment: false,
            pending_default_checks: Vec::new(),
            type_arg_depth: 0,
            type_arg_depth_saved: Vec::new(),
            type_arg_miss_rows: Vec::new(),
            non_type_anchors: Vec::new(),
            creation_miss_rows: Vec::new(),
            clause_miss: Vec::new(),
            header_implements_seen: false,
            conditional_uri_rows: FxHashSet::default(),
            conditional_uri_window: false,
            extension_clause_phase: ExtensionClausePhase::Off,
            extension_clause_miss: Vec::new(),
            pending_init_variables: Vec::new(),
            group_late: false,
            group_final: false,
            group_const: false,
            exec_bounds: FxHashMap::default(),
            method_exec_bounds: FxHashMap::default(),
            pending_exec_is_method: false,
            fn_var_sigs: FxHashMap::default(),
            fn_formal_sigs: FxHashMap::default(),
            arg_sigs: FxHashMap::default(),
            closure_caps: Vec::new(),
            capture_formal_required: false,
            fn_formal_named_section: false,
            fn_type_capture: Vec::new(),
            fn_type_stash: None,
            call_type_args: Vec::new(),
            call_args_capture: None,
            pending_send_ty: None,
            pending_exec_name: None,
            in_const_ctor: false,
            members: MemberFace::new(strings),
            external_type_params: EXTERNAL_CORE_TYPE_PARAMS.map(|(n, k)| (strings.intern(n), k)),
            initializer_depth: 0,
            init_rows: FxHashMap::default(),
            field_init_this_rows: FxHashSet::default(),
            pattern_shadows: Vec::new(),
            if_case_scopes: Vec::new(),
            pending_init_super: Vec::new(),
            init_item_head: None,
            redirect_edges: Vec::new(),
            annotation_at: None,
            const_capture: ConstCapture::new(identical_id),
            const_scopes: vec![FxHashMap::default()],
            const_group: false,
            field_group_open: false,
            field_group_depth: 0,
            init_frame_depth: 0,
            in_initializer: false,
            const_group_names: Vec::new(),
            const_group_cursor: 0,
            nonconst_top_level: FxHashSet::default(),
            const_field_values: FxHashMap::default(),
            pending_const_non_const: Vec::new(),
            open_const_creations: Vec::new(),
            pending_bare_creation: None,
            pending_const_creations: Vec::new(),
            identical_id,
            pending_init_names: Vec::new(),
            arg_gates: Vec::new(),
            var_infer_pending: None,
            init_declarators: Vec::new(),
            staged_write_tys: FxHashMap::default(),
            next_exec_serial: 0,
            enum_element_open: false,
            const_string_begins: Vec::new(),
            ctor_init_latch: None,
            ctor_init_pending: Vec::new(),
            ctor_inits: FxHashMap::default(),
            pending_matched: Vec::new(),
            pending_switch_coverings: Vec::new(),
            deferred_var_rebinds: Vec::new(),
            deferred_invocation_faces: Vec::new(),
            rebound_locals: FxHashMap::default(),
            clause_anchor: None,
            primary_constructors_enabled,
            variance_enabled,
            patterns_enabled,
            field_promotion_enabled,
            var_tv_window: Vec::new(),
            var_name_stack: Vec::new(),
            completed_group: None,
            var_header: None,
            var_containers: Vec::new(),
            owner_libs: RefCell::new(FxHashMap::default()),
            member_faces: FaceMemo::default(),
            reported_extension_types: FxHashSet::default(),
            reported_face_checks: FxHashSet::default(),
            reported_modifier_checks: FxHashSet::default(),
            reported_sealed_checks: FxHashSet::default(),
            deferred_modifier: None,
            locals: Vec::new(),
            frame_local_marks: Vec::new(),
            local_hits: FxHashMap::default(),
            pattern_group_open: None,
            pattern_group_seq: 0,
            pattern_bind_vars: Vec::new(),
            pattern_bind_records: 0,
            pattern_bind_dirty: false,
            last_pattern_variable: None,
            pending_pattern_getters: Vec::new(),
            open_exec_site: None,
            decl_sites: Vec::new(),
            pending_annotations: Vec::new(),
            ur_pending: Vec::new(),
            ur_used: FxHashSet::default(),
            ur_spans: FxHashMap::default(),
            ur_creations: Vec::new(),
            in_for_parts: Vec::new(),
            ur_sites: FxHashMap::default(),
            ur_arg_base: Vec::new(),
            cascade_sections: FxHashSet::default(),
            null_aware_members: FxHashSet::default(),
            cascade_null_aware_op: None,
            used_names: FxHashSet::default(),
            import_used: FxHashSet::default(),
            extension_member_used: FxHashSet::default(),
            accessible_extensions: None,
            read_names: FxHashSet::default(),
            member_read_owners: FxHashMap::default(),
            primary_fields: Vec::new(),
            annotation_uses: FxHashSet::default(),
            named_given: FxHashSet::default(),
            positional_given: FxHashMap::default(),
            site_param_uses: FxHashMap::default(),
            imports: Vec::new(),
            in_show: false,
            in_import: false,
            in_export: false,
            exports: Vec::new(),
            label_scopes: Vec::new(),
            catch_on_type: None,
            catch_on_miss_rows: Vec::new(),
            catch_on_nullable: None,
            catch_clause_start: None,
            catch_params: 0,
            try_clauses: Vec::new(),
            if_null_groups: Vec::new(),
            eq_spans: Vec::new(),
            condition_staged: Vec::new(),
            bool_decided: Vec::new(),
            open_return_span: None,
            cond_mark: None,
            for_in_marks: Vec::new(),
            for_in_loop_var: None,
            await_operand_extents: Vec::new(),
            cond_case: false,
            if_element_cond: 0,
            record_type_depth: 0,
            dead_null_chains: Vec::new(),
            null_assert_staged: Vec::new(),
            null_aware_staged: Vec::new(),
            promoted_recv_reads: Vec::new(),
            prop_promotion_rows: PropPromotionStaging::default(),
            member_reads: Vec::new(),
            member_recv_tys: FxHashMap::default(),
            call_ends: Vec::new(),
            non_promotions: Vec::new(),
            pending_non_promotion: Vec::new(),
            string_row_marks: Vec::new(),
            interpolation_child_rows: FxHashSet::default(),
            read_types: Vec::new(),
            query_type_rows: Vec::new(),
            unnamed_extension_faces_opened: 0,
            initializer_locals: FxHashSet::default(),
            const_kw_spans: Vec::new(),
            const_creation_pending: None,
            local_fn_wildcard: Vec::new(),
            site_params: FxHashMap::default(),
            self_scope_rows: FxHashSet::default(),
            field_read_rows: FxHashMap::default(),
            statement_discarded_rows: FxHashSet::default(),
            ctor_pending: None,
            open_toplevel_site: None,
            open_member_setter: false,
            open_member_static: false,
            pending_label: None,
            last_block_end: None,
            pending_local_fn: None,
            local_fn_stack: Vec::new(),
            strict,
            class_modifiers,
            ops,
            tv_window: None,
            pending_tv: None,
            container_unbounded: FxHashMap::default(),
        }
    }

    /// The library whose scope resolves `name` as written in `owner_file`
    /// (#491 S2a): hop 2+ of a super-face climb resolves through the
    /// declaring container's library, not the diagnosing one. Memoized per
    /// `(file, name)` probe — the BFS over the graph edges repeats only for
    /// distinct probes.
    pub(crate) fn owner_library_of(&self, owner_file: FileId, name: StringId) -> Option<LibraryId> {
        if owner_file == self.file {
            return Some(self.lib);
        }
        if let Some(&cached) = self.owner_libs.borrow().get(&(owner_file, name)) {
            return cached;
        }
        let found = owner_library(self.graph, self.lib, owner_file, name);
        self.owner_libs
            .borrow_mut()
            .insert((owner_file, name), found);
        found
    }

    pub(crate) fn intern(&self, name: &str) -> StringId {
        self.strings.intern(name)
    }

    /// Token text, `None` for parser-inserted synthetics (not in the table).
    pub(crate) fn text_of(&self, token: Token) -> Option<&'a str> {
        self.tokens
            .get(token.id as usize)
            .map(|t| &self.source[t.byte_range()])
    }

    /// The whitespace-trimmed span between two offsets (the switch member
    /// body rows' shape: the official row trims to the member's statement
    /// extent, `dead_code_verifier.dart`'s `flowEnd` member trim). Interior
    /// comments ride along — a documented divergence.
    #[expect(clippy::cast_possible_truncation)] // span arithmetic narrows under the scanner u32 offset cap
    pub(crate) fn trimmed_span(&self, start: u32, end_excl: u32) -> Option<SourceRange> {
        let start = start as usize;
        let end = (end_excl as usize).min(self.source.len());
        if start >= end {
            return None;
        }
        let text = &self.source[start..end];
        let lead = text.len() - text.trim_start().len();
        let trimmed = text.trim_end();
        if trimmed.is_empty() {
            return None;
        }
        // `trimmed` keeps the leading run `offset` already skipped, so the
        // length drops it too — the span is both-ends trimmed.
        Some(SourceRange {
            offset: (start + lead) as u32,
            length: (trimmed.len() - lead) as u32,
        })
    }

    pub(crate) fn name_of(&self, token: Token) -> Option<StringId> {
        self.text_of(token).map(|text| self.strings.intern(text))
    }

    /// The parser-inserted synthetic identifier's name: its token id sits
    /// outside the token table (`token_stream_rewriter
    /// .insertSyntheticIdentifier`'s empty lexeme), so the recovery face
    /// resolves under the empty-string name (`check_enum_constructor_selector`'s
    /// synthetic-selector shape).
    pub(crate) fn synthetic_name(&self, token: Token) -> Option<StringId> {
        self.tokens
            .get(token.id as usize)
            .is_none()
            .then(|| self.intern(""))
    }

    /// The nameless recovery face: a declaration whose name the parser
    /// never produced (`ElementImpl.displayName`'s `name ?? '<unnamed>'`).
    pub(crate) fn is_nameless(&self, name: StringId) -> bool {
        self.strings.resolve(name).is_empty()
    }

    /// The bare `new(` head's site: its name slot is the `(` token, and it
    /// declares the unnamed constructor.
    pub(crate) fn is_bare_new_head(&self, name: StringId) -> bool {
        self.strings.resolve(name) == "("
    }

    /// The nameless declaration's anchor: the enclosing container's
    /// declared-name range, else the file start.
    pub(crate) fn nameless_site_range(&self) -> SourceRange {
        self.container_context()
            .and_then(|(name, _)| {
                self.decl_sites
                    .iter()
                    .rev()
                    .find(|site| matches!(site.kind, DeclKind::Container) && site.name == name)
                    .map(|site| site.range)
            })
            .unwrap_or(SourceRange {
                offset: 0,
                length: 0,
            })
    }

    pub(crate) fn emit(&mut self, code: SemanticCode, range: SourceRange, a0: &str, a1: &str) {
        self.report.diagnostics.push(Diagnostic::new(
            code.severity(),
            code.code_info(),
            code.message(a0, a1),
            self.file,
            range,
            None,
        ));
    }

    /// The correction-carrying emit: the correction renders from the table
    /// template with the row's name (the FAM13 context faces and the catch
    /// `on`-type face — `error_converter.dart`'s correction attachments).
    pub(crate) fn emit_corrected(&mut self, code: SemanticCode, range: SourceRange, a0: &str) {
        let correction = code.correction_message(a0);
        self.report.diagnostics.push(Diagnostic::new(
            code.severity(),
            code.code_info(),
            code.message(a0, ""),
            self.file,
            range,
            correction,
        ));
    }

    /// The three-placeholder emit (`message3`).
    pub(crate) fn emit3(
        &mut self,
        code: SemanticCode,
        range: SourceRange,
        a0: &str,
        a1: &str,
        a2: &str,
    ) {
        self.report.diagnostics.push(Diagnostic::new(
            code.severity(),
            code.code_info(),
            code.message3(a0, a1, a2),
            self.file,
            range,
            None,
        ));
    }

    /// The context-carrying emit: the row renders from the table template
    /// and attaches the oracle's `contextMessages` rows (the
    /// `DiagnosticMessageImpl` face the resolver families build at the
    /// report site — `not_a_type`'s declaration anchor).
    pub(crate) fn emit_with_context(
        &mut self,
        code: SemanticCode,
        range: SourceRange,
        a0: &str,
        a1: &str,
        context: Vec<ContextMessage>,
    ) {
        self.report.diagnostics.push(
            Diagnostic::new(
                code.severity(),
                code.code_info(),
                code.message(a0, a1),
                self.file,
                range,
                None,
            )
            .with_context(context),
        );
    }

    /// The three-placeholder context-carrying emit (the boundedness face's
    /// inverted-type row).
    pub(crate) fn emit3_with_context(
        &mut self,
        code: SemanticCode,
        range: SourceRange,
        a0: &str,
        a1: &str,
        a2: &str,
        context: Vec<ContextMessage>,
    ) {
        self.report.diagnostics.push(
            Diagnostic::new(
                code.severity(),
                code.code_info(),
                code.message3(a0, a1, a2),
                self.file,
                range,
                None,
            )
            .with_context(context),
        );
    }

    /// The correction-carrying emit with a caller-filled message (the
    /// argument-count faces split across the message arities — the singular
    /// texts are constant, the plural ones parameterized).
    pub(crate) fn emit_message_with_correction(
        &mut self,
        code: SemanticCode,
        range: SourceRange,
        message: String,
    ) {
        let correction = code.correction_message("");
        self.report.diagnostics.push(Diagnostic::new(
            code.severity(),
            code.code_info(),
            message,
            self.file,
            range,
            correction,
        ));
    }

    /// Stages a negative assignability verdict (#477 S1): flow's promotion
    /// facts are not answerable mid-parse (the recorder replays an
    /// executable only at its end), so the emission defers to the teardown,
    /// where a promoted-read fact on the expression can still clean the row
    /// ([`Engine::filter_staged`]).
    pub(crate) fn stage(
        &mut self,
        kind: StagedKind,
        range: SourceRange,
        from: Ty,
        to: Ty,
        literal: bool,
    ) {
        self.staged_checks.push(StagedCheck {
            kind,
            range,
            from,
            to,
            literal,
        });
    }

    /// Re-decides the staged verdicts against the promotion staging and
    /// emits the survivors (#477 S1). A promoted-read fact on the staged
    /// expression's exact span substitutes its type — the official read's
    /// static type IS the promoted one
    /// (`property_element_resolver.dart:2158-2166`) — and the family's
    /// matrix runs again; the emission is byte-identical to the parse-time
    /// check it defers, with the (possibly promoted) source type displayed.
    /// A promoted-property fact (#1409) substitutes the non-null face of
    /// the parse-time type when the member resolves to a promotable field
    /// ([`Engine::promotable_field_substitution`]). The survivors return
    /// offset-sorted for the report tail's source-ordered merge.
    pub(crate) fn filter_staged(
        &mut self,
        promotions: &PromotionStaging,
        conditions: &[(SourceRange, Ty)],
        props: &PropPromotionStaging,
    ) -> Vec<Diagnostic> {
        let mut survivors: Vec<Diagnostic> = Vec::new();
        for check in std::mem::take(&mut self.staged_checks) {
            // The nullable-condition face re-decides first: the flow's
            // condition-type row (the operand read's flow-aware static
            // type — the promoted type when a promotion stands) replaces
            // the parse-time type; no flow row means the operand was not
            // a promotable reference and the resolved static type decides.
            //
            // Join key contract (#1405, extended by #1409): flow's
            // condition rows exist only for promotable variable reads
            // (`Interp::condition_operand` gates on the read's reference —
            // calls, `is`-tests, and assignments carry no flow-visible
            // type), anchored at the read's span. Property operands have
            // their own face: a promotable-shaped property read stages
            // [`FlowFact::PromotedPropertyRead`] at the member token —
            // the exact offset a staged check on the member read carries
            // (the merged member row's own span) — and
            // [`Engine::promotable_field_substitution`] decides the
            // field's promotability before the non-null face substitutes.
            // Neither row means the parse-time verdict is final by design,
            // which matches upstream wherever the property is not
            // promotable (field promotion needs a private, final field).
            if matches!(check.kind, StagedKind::NullableCondition) {
                // The opaque flow face's rescue (the same face the staged
                // kinds' general arm takes): a flow row spelling `Unknown`
                // carried only the nullability decision — the lattice
                // cannot spell the type (a function-typed formal's send,
                // #2144) — so the parse-time type stands. Deciding
                // `non_bool_condition` on the opaque marker would drop a
                // known non-`bool` candidate the parse-time face answers.
                // The member-read gate (#2344): a staged condition anchored
                // at a member token is a `receiver.member` read, and the
                // local-variable tables must not answer it. Upstream reads
                // the condition's resolved static type — the member access
                // carries its own accessor element, so a same-named local
                // never feeds its promotion into the member's condition face
                // (upstream Dart SDK source,
                // pkg/analyzer/lib/src/error/bool_expression_verifier.dart:44-73).
                // The flow layer's by-name resolution can stage a shadow
                // local's row at the selector span, so a hit in the
                // offset-keyed `member_reads` index (the same index
                // [`Engine::member_read_resolution`] binary-searches) skips
                // the local condition/promotion lookups and decides on the
                // property face alone.
                let member_read = self
                    .member_reads
                    .binary_search_by(|row| row.offset.cmp(&check.range.offset))
                    .is_ok();
                let from = if member_read {
                    self.promotable_field_substitution(props, check.range, check.from)
                        .filter(|ty| *ty != Ty::Unknown)
                        .unwrap_or(check.from)
                } else {
                    conditions
                        .iter()
                        .rev()
                        .find(|(range, _)| range.offset == check.range.offset)
                        .map(|(_, ty)| *ty)
                        .or_else(|| promotions.promoted(check.range))
                        .or_else(|| {
                            self.promotable_field_substitution(props, check.range, check.from)
                        })
                        .filter(|ty| *ty != Ty::Unknown)
                        .unwrap_or(check.from)
                };
                if self.nullable_bool_ty(from) {
                    let code = SemanticCode::UncheckedUseOfNullableValue;
                    // The row anchors at the condition expression: a
                    // property operand reports over the whole
                    // property-access span — the receiver token through the
                    // member's end (`.at(condition)` — capture-proven: all
                    // condition-face rows in the why-not-promoted corpus
                    // anchor at the property-access start). A bare operand's
                    // row already is its own span. The why-not-promoted
                    // context hangs on the read itself (#1877).
                    let range = self
                        .property_access_span(check.range)
                        .unwrap_or(check.range);
                    survivors.push(
                        Diagnostic::new(
                            code.severity(),
                            code.code_info(),
                            code.message("", ""),
                            self.file,
                            range,
                            code.correction_message(""),
                        )
                        .with_context(self.non_promotion_context_at(check.range.offset)),
                    );
                } else if check.literal && self.known_non_bool(from) {
                    // The staged non-`bool` candidate (a promotable read's
                    // parse-time type): the flow's re-decided type still
                    // names a non-`bool` — `non_bool_condition` reports at
                    // the operand's own span.
                    survivors.push(Diagnostic::new(
                        SemanticCode::NonBoolCondition.severity(),
                        SemanticCode::NonBoolCondition.code_info(),
                        SemanticCode::NonBoolCondition.message("", ""),
                        self.file,
                        check.range,
                        None,
                    ));
                }
                continue;
            }
            if let StagedKind::YieldEach { seq_head } = check.kind {
                // The yield-each verdict reads the source's flow-aware
                // static type (upstream Dart SDK source,
                // pkg/analyzer/lib/src/dart/resolver/
                // yield_statement_resolver.dart `_checkForYieldOfInvalidType`
                // — `expression.typeOrThrow` after the shared analyzer's
                // yield-star rewrite): a null-guard promotion on the read
                // substitutes the promoted type itself and the verdict
                // re-decides, while the no-fact face re-decides on the
                // parse-time wrap (byte-identical to the parse-time emit —
                // a promotion can only clean the verdict, never create
                // it, so the arm only stages where the parse-time face
                // answered negative).
                let implied = match promotions.promoted(check.range) {
                    Some(Ty::Unknown) => Self::non_null_form(check.from),
                    Some(promoted) => promoted,
                    None => self
                        .boundary
                        .args
                        .borrow_mut()
                        .named(seq_head, false, &[check.from]),
                };
                if self.assignable(implied, check.to, check.literal) == Some(false) {
                    let (a0, a1) = (self.ty_display(implied), self.ty_display(check.to));
                    survivors.push(Diagnostic::new(
                        SemanticCode::YieldEachOfInvalidType.severity(),
                        SemanticCode::YieldEachOfInvalidType.code_info(),
                        SemanticCode::YieldEachOfInvalidType.message(&a0, &a1),
                        self.file,
                        check.range,
                        None,
                    ));
                }
                continue;
            }
            let staged_promotion = promotions.promoted(check.range);
            let from = match staged_promotion {
                // The opaque-head promotion (#2174 — a signature-less
                // function-typed declared type, or an unknown-written
                // slot): the flow lattice carried only the nullability
                // decision, so the re-decision substitutes the parse-time
                // declared type with its `?` stripped — the real promoted
                // type that decision produces.
                Some(Ty::Unknown) => Self::non_null_form(check.from),
                staged => staged
                    .or_else(|| self.promotable_field_substitution(props, check.range, check.from))
                    .unwrap_or(check.from),
            };
            if matches!(check.kind, StagedKind::NeverReceiver) {
                // One row at the receiver span per `Never` receiver
                // (`receiverOfTypeNever`). The join above replaced the type
                // when the flow layer staged a promoted read; #477's drop
                // rule keeps promoted-to-`Never` facts out today, so the
                // unpromoted declared type decides (documented
                // under-report).
                if self.is_never_ty(from) {
                    let code = SemanticCode::ReceiverOfTypeNever;
                    survivors.push(Diagnostic::new(
                        code.severity(),
                        code.code_info(),
                        code.message("", ""),
                        self.file,
                        check.range,
                        code.correction_message(""),
                    ));
                }
                continue;
            }
            let verdict = match check.kind {
                StagedKind::Return { ctor, is_async, .. } => {
                    // The declared return's verdict reads the alias's RHS
                    // face (`typedef WhenComplete = void Function(...)?` —
                    // `return null` is legal because the alias spells a
                    // nullable function type); the diagnostic's display
                    // still writes the spelled name (`check.to`).
                    let verdict = self.return_verdict(
                        from,
                        self.fold_alias_ty(check.to),
                        check.literal,
                        is_async,
                    );
                    if verdict.is_none() && ctor {
                        self.declared_subtype_negative(&from, &check.to)
                    } else {
                        verdict
                    }
                }
                // An argument row staged at parse time already carries its
                // verdict (the operator folds decide on the name-level
                // lattice, the invocation walk on the assignability
                // matrix); staging exists only so a promotion can clean it.
                // No promotion stood at the operand: the parse-time
                // rejection stands — re-deciding with the matrix alone
                // would drop every name-level-only rejection (`String`
                // into an in-file `M` formal answers `None` there). The one
                // face the standing verdict may not cover is the broken
                // projection: a source head spelling an import prefix is
                // not a type, so no verdict over it is the oracle's
                // ([`Self::prefix_head_ty`]).
                StagedKind::Argument if from == check.from => {
                    (!self.prefix_head_ty(from)).then_some(false)
                }
                _ => self.assignable(from, check.to, check.literal),
            };
            if verdict != Some(false) {
                // Cleaned by the promotion, or the promoted type left the
                // question undecidable — suppress (under-report).
                continue;
            }
            let (code, a2) = match check.kind {
                // Routed by the early blocks above; the arms only keep the
                // match total.
                StagedKind::NeverReceiver | StagedKind::NullableCondition => continue,
                StagedKind::YieldEach { .. } => continue,
                StagedKind::Assignment => (SemanticCode::InvalidAssignment, None),
                StagedKind::Argument => (SemanticCode::ArgumentTypeNotAssignable, None),
                StagedKind::Yield => (SemanticCode::YieldOfInvalidType, None),
                StagedKind::Return { host, name, .. } => {
                    (SemanticCode::ReturnOfInvalidType(host), Some(name))
                }
            };
            // The oracle anchors the assignability face at the whole
            // right-hand expression — a call operand reports over the
            // invocation span (`f()`'s callee through `)`, or `c.m()`'s
            // receiver through `)`), a `receiver.member` operand over the
            // property-access span (receiver start through the member's
            // end, `.at(expression)`), not the name alone. A bare operand's
            // row already is its own span.
            let range = match check.kind {
                StagedKind::Assignment | StagedKind::Argument => self
                    .invocation_span(check.range)
                    .or_else(|| self.property_access_span(check.range))
                    .unwrap_or(check.range),
                _ => check.range,
            };
            let (a0, a1) = (self.ty_display(from), self.ty_display(check.to));
            let message = match a2 {
                Some(name) => {
                    let a2 = self.strings.resolve(name).to_string();
                    code.message3(&a0, &a1, &a2)
                }
                None => code.message3(&a0, &a1, ""),
            };
            // The why-not-promoted context rides the assignability rows the
            // oracle attaches it to — invalid_assignment and
            // argument_type_not_assignable (upstream Dart SDK source,
            // pkg/analyzer/lib/src/dart/resolver/assignment_expression_resolver.dart:793-805,
            // error_detection_helpers.dart:139-143); a `return_of_invalid_type`
            // carries none.
            //
            // The oracle's `convertTypeNames` face (listener.dart:38) is
            // deliberately absent here (#1798 W6, not-wired-by-design): it
            // fires when two typed arguments of one diagnostic share a
            // display name, which for this family requires two distinct
            // same-named elements — reachable only through prefixed
            // imports, whose element identity the written-shape `Ty`
            // intentionally drops — and every row anchors at the resolved
            // element's declaration, which the element model does not carry
            // cross-file. The face lands with the type-engine element
            // provenance, never from display strings alone.
            let context = match check.kind {
                StagedKind::Assignment | StagedKind::Argument => {
                    self.non_promotion_context_at(check.range.offset)
                }
                _ => Vec::new(),
            };
            survivors.push(
                Diagnostic::new(
                    code.severity(),
                    code.code_info(),
                    message,
                    self.file,
                    range,
                    None,
                )
                .with_context(context),
            );
        }
        // Parse order is source order for the staging sites (nested
        // assignments excepted); the sort keeps the merge invariant
        // regardless.
        survivors.sort_by_key(|d| d.range.offset);
        survivors
    }

    /// The four-placeholder emit (`message4`).
    pub(crate) fn emit4(
        &mut self,
        code: SemanticCode,
        range: SourceRange,
        a0: &str,
        a1: &str,
        a2: &str,
        a3: &str,
    ) {
        self.report.diagnostics.push(Diagnostic::new(
            code.severity(),
            code.code_info(),
            code.message4(a0, a1, a2, a3),
            self.file,
            range,
            None,
        ));
    }

    /// The five-placeholder emit (`message5`).
    #[allow(clippy::too_many_arguments)] // the template's five slots, one each
    pub(crate) fn emit5(
        &mut self,
        code: SemanticCode,
        range: SourceRange,
        a0: &str,
        a1: &str,
        a2: &str,
        a3: &str,
        a4: &str,
    ) {
        self.report.diagnostics.push(Diagnostic::new(
            code.severity(),
            code.code_info(),
            code.message5(a0, a1, a2, a3, a4),
            self.file,
            range,
            None,
        ));
    }

    /// The five-placeholder context-carrying emit (the `invalid_override`
    /// family's overridden-member anchor).
    #[allow(clippy::too_many_arguments)] // emit5's slots plus the context rows
    pub(crate) fn emit5_with_context(
        &mut self,
        code: SemanticCode,
        range: SourceRange,
        a0: &str,
        a1: &str,
        a2: &str,
        a3: &str,
        a4: &str,
        context: Vec<ContextMessage>,
    ) {
        self.report.diagnostics.push(
            Diagnostic::new(
                code.severity(),
                code.code_info(),
                code.message5(a0, a1, a2, a3, a4),
                self.file,
                range,
                None,
            )
            .with_context(context),
        );
    }

    /// The class-modifier emit: the resolution layer's frozen registry row
    /// (`ModifierCode::info`) rendered through the shared diagnostic model —
    /// the template-fill shape the resolution family's engine-side emitter
    /// uses (`l1::combinator_diagnostic`). `{0}`/`{1}` fill the outside-
    /// library and subtype-family templates; the declaration-site rows carry
    /// no arguments.
    pub(crate) fn emit_modifier(
        &mut self,
        code: ModifierCode,
        range: SourceRange,
        a0: &str,
        a1: &str,
    ) {
        let info = code.info();
        self.report.diagnostics.push(Diagnostic::new(
            info.dtype.severity(),
            CodeInfo::new(info.name, info.machine, info.dtype, info.has_published),
            info.message.replace("{0}", a0).replace("{1}", a1),
            self.file,
            range,
            info.correction.map(str::to_owned),
        ));
    }

    pub(crate) fn record_miss(
        &mut self,
        name: StringId,
        range: SourceRange,
        qualified: bool,
        kind: NameUse,
    ) {
        self.report.names.push(NameRow {
            name,
            range,
            qualified,
            use_kind: kind,
        });
    }

    // —— scope frames ————————————————————————————————————————————————

    pub(crate) fn push_frame(&mut self, kind: FrameKind) {
        let type_mark = self.type_params.len();
        let row_mark = self.rows.len();
        let pending = std::mem::take(&mut self.pending_type_params);
        // The header's bound rows parse before their variables' frame opens
        // and can name a parameter declared after them (`T extends U, U
        // extends num` — the whole list is in scope in every bound, official
        // element building binds all names before the deferred bound types
        // resolve). A still-missing type use of a now-joined parameter was
        // the parameter all along, not an undefined class — the same
        // rewrite `push_type_parameter` runs for frames already open,
        // bounded to this header's own rows.
        let header_mark = self.pending_rows_mark.min(row_mark);
        for name in &pending {
            for row in &mut self.rows[header_mark..] {
                if row.name == *name
                    && matches!(row.shape, RowShape::TypeUse(TypeVerdict::Missing, false))
                {
                    row.shape = RowShape::Value;
                }
            }
        }
        self.type_params.extend(pending);
        // The executable serial (#2460 A2c): a fresh identity per
        // executable — the `staged_write_tys` inference feed keys on it,
        // so a write staged inside one executable never types another's
        // `var` initializer across a popped frame at the same depth.
        let serial = if matches!(kind, FrameKind::Executable { .. }) {
            self.next_exec_serial += 1;
            self.next_exec_serial
        } else {
            0
        };
        self.frames.push(Frame {
            kind,
            serial,
            locals: FxHashSet::default(),
            var_types: FxHashMap::default(),
            fn_formal_rets: FxHashMap::default(),
            type_mark,
            row_mark,
        });
        self.frame_local_marks.push(self.locals.len());
        self.const_scopes.push(FxHashMap::default());
    }

    pub(crate) fn pop_frame(&mut self) {
        if let Some(frame) = self.frames.pop() {
            self.type_params.truncate(frame.type_mark);
            self.close_frame_locals(frame);
        }
        while self
            .local_fn_stack
            .last()
            .is_some_and(|(depth, _)| *depth >= self.frames.len())
        {
            self.local_fn_stack.pop();
        }
        // Scope 0 is the file scope; it never pops.
        if self.const_scopes.len() > 1 {
            self.const_scopes.pop();
        }
    }

    /// Closes a frame's local declarations: the usage cluster's per-scope
    /// decision point. Every un-referenced declaration routes per its kind
    /// (the official `_visitLocalVariableElement` split,
    /// unused_local_elements_verifier.dart:1259-1280); the emissions stage
    /// for the end-of-pass source-order merge.
    pub(crate) fn close_frame_locals(&mut self, _frame: Frame) {
        // The pop can remove the open window's members: a group never spans
        // frames, so the window dies with its frame.
        self.pattern_group_open = None;
        let mark = self.frame_local_marks.pop().unwrap_or(self.locals.len());
        if mark >= self.locals.len() {
            self.locals.truncate(mark);
            return;
        }
        // Pattern-declaration groups report all-or-nothing: one used member
        // spares the group (official visitPatternVariableDeclaration).
        let mut group_used: FxHashMap<u32, bool> = FxHashMap::default();
        for decl in &self.locals[mark..] {
            if let Some(group) = decl.group {
                *group_used.entry(group).or_default() |= decl.uses > 0;
            }
        }
        for index in mark..self.locals.len() {
            let (name, range, kind, uses, group) = {
                let decl = &self.locals[index];
                (decl.name, decl.range, decl.kind, decl.uses, decl.group)
            };
            // Parameters feed the declaration site's body-reference rule
            // (`unused_element_parameter`); they never emit per se.
            if let LocalKind::Parameter { site: Some(site) } = kind {
                if uses > 0 {
                    self.site_param_uses.entry(site).or_default().insert(name);
                }
                continue;
            }
            if uses > 0 {
                continue;
            }
            // The catch clauses' double-pass parse: the parameter list is
            // walked once for the catch events (the CatchException/CatchStack
            // rows) and once for the formal-parameter events (the
            // Parameter twin). Every body use lands on the twin, so the
            // catch row's verdict reads the twin's count — a used `e` in
            // `catch (e) { print(e); }` spares the clause face.
            if matches!(
                kind,
                LocalKind::CatchException { .. } | LocalKind::CatchStack
            ) && self.locals[mark..].iter().any(|d| {
                d.name == name && matches!(d.kind, LocalKind::Parameter { .. }) && d.uses > 0
            }) {
                continue;
            }
            let spared = match group {
                Some(group) => group_used.get(&group).copied().unwrap_or(false),
                None => false,
            };
            if spared {
                continue;
            }
            let wildcard = self.is_wildcard(name);
            let name_text = self.strings.resolve(name).to_string();
            let (code, name) = match kind {
                LocalKind::Variable | LocalKind::PatternVariable => {
                    // The wildcard-variables feature: an all-underscores
                    // name never reports (unused_local_elements_verifier.dart
                    // :985-994).
                    if wildcard {
                        continue;
                    }
                    (SemanticCode::UnusedLocalVariable, name_text)
                }
                LocalKind::Parameter { .. } => {
                    // Parameters route through `unused_element_parameter` at
                    // the declaration site, never the local-variable row.
                    continue;
                }
                LocalKind::LocalFunction => {
                    if wildcard {
                        continue;
                    }
                    (SemanticCode::UnusedElement, name_text)
                }
                LocalKind::CatchException { auto } => {
                    // A plain `catch (e)` (no `on` type) or a clause that
                    // also declares a stack trace parameter auto-uses its
                    // exception variable (official visitCatchClause,
                    // unused_local_elements_verifier.dart:67-82); the flag
                    // was patched in at `handle_catch_block`.
                    if auto || wildcard {
                        continue;
                    }
                    (SemanticCode::UnusedCatchClause, name_text)
                }
                // The stack trace row carries no wildcard exemption — the
                // official catch-stack branch has none either.
                LocalKind::CatchStack => (SemanticCode::UnusedCatchStack, name_text),
            };
            self.stage_usage(code, range, &name);
        }
        self.locals.truncate(mark);
    }

    /// Stages one usage-cluster diagnostic (merged into the report at the
    /// end of the pass, source order restored there).
    pub(crate) fn stage_usage(&mut self, code: SemanticCode, range: SourceRange, a0: &str) {
        let correction = code.correction_message(a0);
        self.report.usage.push(Diagnostic::new(
            code.severity(),
            code.code_info(),
            code.message(a0, ""),
            self.file,
            range,
            correction,
        ));
    }

    /// Stages one two-argument diagnostic (the disjointness pair's type
    /// displays).
    pub(crate) fn stage_typed(
        &mut self,
        code: SemanticCode,
        range: SourceRange,
        a0: &str,
        a1: &str,
    ) {
        let correction = code.correction_message("");
        self.report.usage.push(Diagnostic::new(
            code.severity(),
            code.code_info(),
            code.message(a0, a1),
            self.file,
            range,
            correction,
        ));
    }

    /// The name-level subtype relation over semantic types
    /// ([`NameWorld::is_subtype_of`], the declared-edge lattice).
    pub(crate) fn name_subtype(&self, sub: Ty, sup: Ty) -> bool {
        self.name_world()
            .is_subtype_of(&NameTy::from_semantic(sub), &NameTy::from_semantic(sup))
    }

    /// The resolved `InvalidType` face at the top level of a semantic
    /// type (upstream Dart SDK source,
    /// pkg/analyzer/lib/src/dart/element/subtype.dart:274-276 — the 0c
    /// rule, an `InvalidType` on either side answers `true`): the
    /// parser's recovered-`Invalid` splice [`Ty::Unknown`], or a `Named`
    /// head the type-position resolution answers as `Missing`/`NonType`
    /// (`UndefinedNav`, a variable or prefix in a type position — the
    /// resolver substitutes `InvalidType` on both). An in-scope type
    /// parameter keeps its own lattice face (`type_resolution` answers
    /// `Type`), a real declaration or alias the same, and a
    /// multiply-defined head is `InvalidType` with no report —
    /// `type_resolution` folds it under `Type` (the silent face), so
    /// the scoped hit re-asks. Heads nested inside type arguments do
    /// not reach this test at the top level; the recursive asks that
    /// surface them (the `FutureOr` legs, the covariant slot walk)
    /// re-enter through [`Self::resolved_subtype`] and read the same
    /// face there.
    pub(crate) fn is_invalid_ty(&self, ty: Ty) -> bool {
        let Ty::Named { name, .. } = ty else {
            return ty == Ty::Unknown;
        };
        // A compiler-known head answers before the scoped walk: the
        // declared-face test (`Object`, the primitives, the indexed
        // containers, [`NameWorld::is_known_head`]) and the in-scope
        // type parameters are real interface types whether or not this
        // unit's graph declares them. `Future`/`FutureOr` are NOT here
        // on purpose — they are `dart:async` faces, not `dart:core`'s,
        // so an unimported spelling is the oracle's `InvalidType`
        // (the type-position miss reports `undefined_class` beside it).
        if self.is_scope_param(name) || self.name_world().is_known_head(name) {
            return false;
        }
        match self.type_resolution(name).0 {
            TypeResolution::Missing | TypeResolution::NonType => true,
            TypeResolution::Type => matches!(
                lookup_scoped(
                    self.graph,
                    self.namespaces,
                    self.index,
                    self.lib,
                    self.file,
                    name,
                ),
                ScopedHit::Ambiguous(_)
            ),
        }
    }

    /// The strict subtype verdict over the resolved operand faces
    /// (`subtype.dart`'s `isSubtypeOf`): [`Engine::slot_subtype`] plus
    /// the three upstream rules the slot cannot ask at `Ty` level —
    /// 0c `InvalidType` (an unresolvable head is a subtype of
    /// everything and vice-versa, [`Self::is_invalid_ty`]), rule 6
    /// Left `FutureOr` (`FutureOr<S> <: T` iff `S <: T` and
    /// `Future<S> <: T`, subtype.dart:372-385) and rule 10 Right
    /// `FutureOr` (`S <: FutureOr<T>` iff `S <: Future<T>` or
    /// `S <: T`, subtype.dart:418-449 — the assignable face's own
    /// `FutureOr` arm answers the looser question, this ask needs the
    /// strict one). A `FutureOr` with no written argument reads
    /// `FutureOr<dynamic>` — the same raw-head face
    /// [`Engine::assignable`]'s Right-`FutureOr` arm applies. `true`
    /// is the proven-constant answer only; every undecidable shape
    /// answers `false` — the under-report direction every consumer of
    /// the fold already takes.
    pub(crate) fn resolved_subtype(&self, sub: Ty, sup: Ty) -> bool {
        if self.is_invalid_ty(sub) || self.is_invalid_ty(sup) {
            return true;
        }
        let future_or_parts = |ty: Ty| -> Option<(Ty, ArgListId)> {
            let Ty::Named {
                name,
                nullable: false,
                args,
            } = ty
            else {
                return None;
            };
            if name != self.intern("FutureOr") {
                return None;
            }
            let value = {
                let pool = self.boundary.pool();
                pool.get(args).first().copied().unwrap_or(Ty::Dynamic)
            };
            Some((value, args))
        };
        if let Some((value, args)) = future_or_parts(sub) {
            let future = Ty::Named {
                name: self.intern("Future"),
                nullable: false,
                args,
            };
            return self.resolved_subtype(value, sup) && self.resolved_subtype(future, sup);
        }
        if let Some((value, args)) = future_or_parts(sup) {
            let future = Ty::Named {
                name: self.intern("Future"),
                nullable: false,
                args,
            };
            return self.resolved_subtype(sub, future) || self.resolved_subtype(sub, value);
        }
        self.slot_subtype(sub, sup) == Some(true)
    }

    /// The name-level `canBeSubtypeOf`
    /// ([`NameWorld::can_be_subtype_of`], the mutual-exclusion companion).
    pub(crate) fn name_can_be_subtype(&self, sub: Ty, sup: Ty) -> bool {
        self.name_world()
            .can_be_subtype_of(&NameTy::from_semantic(sub), &NameTy::from_semantic(sup))
    }

    /// The name-level `_canBeEqual` ([`NameWorld::can_be_equal`]).
    pub(crate) fn name_can_be_equal(&self, constant: Ty, value: Ty) -> bool {
        self.name_world().can_be_equal(
            &NameTy::from_semantic(constant),
            &NameTy::from_semantic(value),
        )
    }

    /// The nearest operand row below the type-use rows the `is`/`as` type
    /// may have pushed (unresolved type operands land as `TypeUse` rows).
    pub(crate) fn operand_row(&self) -> Option<&Row> {
        let mut index = self.rows.len().checked_sub(1)?;
        while matches!(self.rows[index].shape, RowShape::TypeUse(..)) {
            index = index.checked_sub(1)?;
        }
        self.rows.get(index)
    }

    /// The `is`/`is!` operand's resolved static type
    /// (`leftNode.typeOrThrow`, best_practices_verifier.dart:1011): the
    /// nearest operand row's push-time snapshot, or — a bare local's —
    /// the declared/inferred type the frame bound (`var self = this`
    /// reads `self`'s inferred `this` type, not the row's `Unknown` —
    /// mutation plays no part in the verdict). [`Ty::Unknown`] when no
    /// operand row exists or the frame binds nothing — the every-check
    /// suppresses sentinel the callers gate on.
    pub(crate) fn type_check_operand_ty(&self) -> Ty {
        let Some(operand) = self.operand_row() else {
            return Ty::Unknown;
        };
        if operand.ty != Ty::Unknown {
            operand.ty
        } else if let RowShape::Bare(BareInfo::Plain { ty }) = &operand.shape {
            if *ty != Ty::UNKNOWN {
                *ty
            } else {
                self.typed_local(operand.name).unwrap_or(Ty::Unknown)
            }
        } else {
            Ty::Unknown
        }
    }

    /// `unnecessary_cast` on the expression form (`_isUnnecessaryCast`,
    /// best_practices_verifier.dart:1708-1741, mirrored from
    /// `TypeSystem::is_unnecessary_cast` in dartq-types): the operand type
    /// must already equal the cast type structurally, with `dynamic` and
    /// bare `Function` targets excluded (there the explicit cast is the
    /// recommended spelling) and unresolved sides answering no.
    pub(crate) fn check_unnecessary_cast(
        &mut self,
        at: SourceRange,
        operand: Ty,
        cast: Option<Ty>,
    ) {
        let Some(cast) = cast else { return };
        if operand == Ty::Unknown || cast == Ty::Unknown || cast == Ty::Dynamic {
            return;
        }
        if operand != cast {
            return;
        }
        if let Ty::Named { name, .. } = cast {
            // The bare `Function` target keeps its cast (the port's
            // `is_bare_function_id` arm).
            if name == self.intern("Function") {
                return;
            }
        }
        self.stage_usage(SemanticCode::UnnecessaryCast, at, "");
    }

    /// `cast_from_null_always_fails` (`visitAsExpression`,
    /// best_practices_verifier.dart:180-190): an `e as T` whose operand's
    /// static type is exactly `Null` and whose target is non-nullable
    /// always throws. Non-nullability is the syntactic `?` flag at name
    /// level (`isNonNullable` — `dynamic`, `void`, and `Null` targets fail
    /// it and stay silent).
    pub(crate) fn check_cast_from_null(&mut self, at: SourceRange, operand: Ty, cast: Option<Ty>) {
        if operand != Ty::Null {
            return;
        }
        let Some(Ty::Named {
            name,
            nullable: false,
            ..
        }) = cast
        else {
            return;
        };
        // An in-scope type-parameter target rides its bound: `null as T`
        // over an unbounded `T` is a legal null cast (the bound is
        // nullable — the model carries no bound on this face), so the
        // always-throws verdict stays silent (#2172's `return null as T`).
        if self.type_params.contains(&name) || self.pending_type_params.contains(&name) {
            return;
        }
        self.stage_usage(SemanticCode::CastFromNullAlwaysFails, at, "");
    }

    /// `cast_from_nullable_always_fails` (`visitAsExpression`'s
    /// unassigned-read face, type_analyzer.dart:2107-2126): a simple local
    /// read whose declared type is spelled nullable, cast to a non-nullable
    /// named type, reports when the read is definitely unassigned. That
    /// fifth face only the flow layer knows, so the candidate stages keyed
    /// by the read's identifier span and the teardown's `UnassignedRead`
    /// join decides; the remaining four decide here — the operand row is a
    /// bare read (`element is PromotableElementImpl`; the flow side never
    /// surfaces non-locals, so the join keeps non-flow shapes silent), the
    /// declared type carries the written `?` (`isNullable(element.type)` —
    /// the read's parse-time type is the declared one), and the target is
    /// non-nullable named (`isNonNullable(staticType)`).
    pub(crate) fn check_cast_from_nullable(
        &mut self,
        operand: Option<(SourceRange, StringId)>,
        operand_ty: Ty,
        cast: Option<Ty>,
    ) {
        let Some((span, name)) = operand else {
            return;
        };
        let Ty::Named { nullable: true, .. } = operand_ty else {
            return;
        };
        let Some(Ty::Named {
            nullable: false, ..
        }) = cast
        else {
            return;
        };
        match self
            .cast_nullable
            .binary_search_by(|(r, _)| r.offset.cmp(&span.offset))
        {
            // A diverging repeat (two candidates on one span) leaves no
            // single read that stands for it — poisoned, the join misses
            // (the [`PromotionStaging`] rule).
            Ok(index) => {
                self.cast_nullable.remove(index);
            }
            Err(index) => self.cast_nullable.insert(index, (span, name)),
        }
    }

    /// The as-cast flow tail (probed: dart analyze 3.13.1 — `var a = f()
    /// as int;` with a Never-typed `f()` reports `DEAD_CODE` anchored at the
    /// cast type's head token, `Dead code.`: the operand's exit left the
    /// flow unreachable, so the cast type is the first node visited dead,
    /// dead_code_verifier.dart:253-279). Only the initializer face fires
    /// here — an expression-statement wrapper's dead interval is the flow
    /// window's (`ExprStmt`), and a nested executable owns its own. The
    /// row anchors at the type's head token; the oracle's trailing-`;`
    /// trim and interval merge with following statements stay the flow
    /// window's business.
    #[expect(clippy::cast_possible_truncation)] // span arithmetic narrows under the scanner u32 offset cap
    pub(crate) fn check_dead_cast_tail(&mut self, operator: Token, operand: Ty) {
        if !self.is_never_ty(operand)
            || !self.in_initializer
            || self.frames.len() != self.init_frame_depth
        {
            return;
        }
        // The cast type's head follows `as` (`parse_as_operator_rest`
        // opens the type bracket right after the operator).
        if let Some(head) = self.tokens.get(operator.id as usize + 1) {
            let raw = &self.source[head.byte_range()];
            self.stage_usage(
                SemanticCode::DeadCode,
                SourceRange {
                    offset: head.utf16_offset,
                    length: raw.chars().map(char::len_utf16).sum::<usize>() as u32,
                },
                "",
            );
        }
    }

    /// The `is`/`is!` verdict walk (`_checkAllTypeChecks`,
    /// best_practices_verifier.dart:987-1041, mirrored from
    /// `TypeSystem::type_check_verdict`): unresolved sides are informative;
    /// `x is dynamic` is constant-true; `is Null` is the
    /// `type_check_with_null` family, downgraded to the constant row when
    /// the operand is the `null` literal; a provable subtype is
    /// constant-true, `is!` reporting the complement's polarity.
    pub(crate) fn check_type_check(&mut self, at: SourceRange, negated: bool) {
        let Some(checked) = self.armed_ty else { return };
        // The target side folds like the operand side
        // ([`Self::bind_declared_type`]): the verdict reads the resolved
        // interface type — `x is IntAlias` over an `int` operand is
        // constant-true.
        let mut checked = self.fold_alias_ty(checked);
        // A prefixed `is p.T` arms the prefix token's name (the parser's
        // `handle_type` anchors the qualified head at `p`, never `T`):
        // `Named{p}` resolves to the import prefix, not a type. The real
        // checked head is the continuation's own TypeUse row — the last
        // type-use the `is` target pushed. Substituting it keeps the
        // name-level verdict honest (`entity is io.Directory` reads
        // `Directory`, not `io`); a non-prefixed target reads `checked`
        // unchanged.
        if let Ty::Named { name, .. } = checked
            && matches!(self.resolve_bare(name).1, RefTarget::Prefix)
            && let Some(row) = self
                .rows
                .iter()
                .rev()
                .find(|row| matches!(row.shape, RowShape::TypeUse(..)))
        {
            checked = Ty::Named {
                name: row.name,
                nullable: false,
                args: ArgListId::EMPTY,
            };
        }
        // `TypeSystem::type_check_verdict`'s first gate — an
        // `InvalidType` operand is informative before every other arm
        // (`subtype.dart`'s 0c vacuity never reaches the verdict), so
        // `Missing rv is dynamic` reports nothing. The `p.T`-prefixed
        // operand is the same silence (the prefix answers no subtype
        // question — the under-report direction every unresolved side
        // already takes).
        let operand_ty = self.type_check_operand_ty();
        if operand_ty == Ty::Unknown
            || matches!(operand_ty, Ty::Named { name, .. } if matches!(self.resolve_bare(name).1, RefTarget::Prefix))
            || self.is_invalid_ty(operand_ty)
        {
            return;
        }
        let verdict = if checked == Ty::Dynamic {
            Some(true)
        } else if checked == Ty::Unknown || self.is_invalid_ty(checked) {
            None
        } else if checked == Ty::Null {
            // The `is Null` family (`_checkAllTypeChecks`'s `Null` arm)
            // — unless the operand is the `null` literal, where the
            // check is constant instead (the same walk's `null` arm:
            // `Null <: Null` answers `true`).
            let null_literal = self
                .operand_row()
                .is_some_and(|row| row.ty == Ty::Null && matches!(row.shape, RowShape::Value));
            if null_literal {
                // `Null <: T` decides the literal check's polarity.
                Some(self.name_subtype(Ty::Null, checked))
            } else {
                self.stage_type_check_null(at, negated);
                return;
            }
        } else {
            // A bare operand head against an argument-carrying checked
            // type of the same head is informative: the bare side is
            // `C<dynamic>` (upstream `TypeSystem::type_check_verdict` reads
            // the operand's instantiated arguments — `C<dynamic> is
            // C<DateTime>` is a real runtime question), and the wildcard
            // the name-level lattice gives a bare side must not decide the
            // constant-true verdict. The one provable shape stays: every
            // checked argument accepting the `dynamic` row (`dynamic`
            // itself and the bare/nullable `Object` — the only tops
            // `dynamic <: T` answers) keeps the subtype question.
            let object = self.intern("Object");
            if let Ty::Named {
                name: operand_name,
                args: ArgListId::EMPTY,
                ..
            } = operand_ty
                && let Ty::Named {
                    name: checked_name,
                    args: checked_args,
                    ..
                } = checked
                && operand_name == checked_name
                && checked_args != ArgListId::EMPTY
            {
                let accepts_dynamic = self.boundary.args.borrow().get(checked_args).iter().all(
                    |&arg| match arg {
                        Ty::Dynamic => true,
                        Ty::Named {
                            name,
                            args: ArgListId::EMPTY,
                            ..
                        } => name == object,
                        _ => false,
                    },
                );
                if !accepts_dynamic {
                    return;
                }
            }
            // A type-parameter argument position decides through the
            // parameter's bound exactly as the oracle's verdict does
            // (`List<T extends num> is List<num>` is constant-true,
            // `List<T> is List<DateTime?>` is informative — the bare
            // parameter's `T <: S` is provable only through its bound,
            // subtype.dart:306-317): an operand argument spelling a bare
            // type parameter the walk cannot resolve as a container keeps
            // the verdict only when its bound (or a top checked argument)
            // proves the pair. No recoverable bound is the unprovability
            // itself — informative. The bound question asks the
            // declared-edge lattice alone — [`Self::resolved_subtype`]'s
            // slot matrix answers every nullable named target as a right
            // top (the M1 approximation `num <: DateTime? == true`), which
            // is not the strict provability the verdict needs. When every
            // position is such a parameter and every bound proves the
            // pair, the constant-true answer stands directly — the lattice
            // ask below cannot spell the parameter side and would
            // under-report the oracle's row.
            let mut param_pair_true = false;
            let mut non_param_position = false;
            if let Ty::Named {
                name: operand_name,
                args: operand_args,
                ..
            } = operand_ty
                && let Ty::Named {
                    name: checked_name,
                    args: checked_args,
                    ..
                } = checked
                && operand_name == checked_name
                && operand_args != ArgListId::EMPTY
                && operand_args != checked_args
            {
                let operand_args = self.boundary.args.borrow().get(operand_args).to_vec();
                let checked_args = self.boundary.args.borrow().get(checked_args).to_vec();
                if operand_args.len() == checked_args.len() {
                    param_pair_true = true;
                    for (&operand_arg, &checked_arg) in
                        operand_args.iter().zip(checked_args.iter())
                    {
                        let Ty::Named {
                            name: param,
                            args: ArgListId::EMPTY,
                            ..
                        } = operand_arg
                        else {
                            non_param_position = true;
                            continue;
                        };
                        if resolve_container(
                            self.graph,
                            self.namespaces,
                            self.index,
                            self.lib,
                            self.file,
                            param,
                        )
                        .is_some()
                        {
                            non_param_position = true;
                            continue;
                        }
                        // A top checked argument proves every operand side.
                        let top = match checked_arg {
                            Ty::Dynamic => true,
                            Ty::Named {
                                name,
                                args: ArgListId::EMPTY,
                                ..
                            } => name == object,
                            _ => false,
                        };
                        if top {
                            continue;
                        }
                        let bound = match self.param_bound_of(param) {
                            Some(Ty::Dynamic) => return,
                            Some(bound) => bound,
                            None => return,
                        };
                        if !self.name_subtype(bound, checked_arg) {
                            return;
                        }
                    }
                }
            }
            // The wrong-arity checked head (`x is Response<int, int>` over
            // the one-parameter class) reads the oracle's recovered
            // `Response<Invalid>` interface — beside the arity row the
            // type-arguments pass already reports — whose vacuous argument
            // comparison (subtype.dart's 0c arm) answers the same-head
            // question constant-true: the `is`/`is!` polarity rows fire
            // (#2380's recv_arity probes — the oracle reports
            // `unnecessary_type_check` on both polarities).
            let arity_true =
                if let (
                    Ty::Named {
                        name: operand_name, ..
                    },
                    Ty::Named {
                        name: checked_name,
                        args: checked_args,
                        ..
                    },
                ) = (operand_ty, checked)
                    && operand_name == checked_name
                    && checked_args != ArgListId::EMPTY
                    && let Some((container, _)) = resolve_container(
                        self.graph,
                        self.namespaces,
                        self.index,
                        self.lib,
                        self.file,
                        checked_name,
                    )
                {
                    self.boundary.args.borrow().get(checked_args).len()
                        != container.type_params as usize
                } else {
                    false
                };
            if arity_true {
                Some(true)
            } else if param_pair_true && !non_param_position {
                Some(true)
            } else if !self.name_subtype(operand_ty, checked)
                && !self.resolved_subtype(operand_ty, checked)
            {
                // A non-matching pair is informative — the official order
                // has no subtype-negative verdict
                // (`TypeSystem::type_check_verdict`'s doc): only the
                // constant-true answer reports here, with `is!` mapping
                // to the 'false' row below. The name-level lattice
                // answers first; pairs it cannot pose (a `FutureOr`
                // source — `FutureOr<S> <: T` needs rule 6's legs — or an
                // `InvalidType` nested inside the arguments, which only
                // the resolved face reads as 0c-vacuous) re-ask through
                // [`Self::resolved_subtype`], the same strict-subtype
                // question over the operand's full face.
                return;
            } else {
                Some(true)
            }
        };
        let Some(constant_true) = verdict else { return };
        let code = match (constant_true, negated) {
            (true, false) | (false, true) => SemanticCode::UnnecessaryTypeCheckTrue,
            (true, true) | (false, false) => SemanticCode::UnnecessaryTypeCheckFalse,
        };
        self.stage_usage(code, at, "");
    }

    /// The `is Null` family (`_checkAllTypeChecks`'s `Null` arm,
    /// best_practices_verifier.dart:1022-1036): the polarity picks the
    /// row.
    pub(crate) fn stage_type_check_null(&mut self, at: SourceRange, negated: bool) {
        let code = if negated {
            SemanticCode::TypeCheckWithNotNull
        } else {
            SemanticCode::TypeCheckWithNull
        };
        self.stage_usage(code, at, "");
    }

    /// The operand's static type at a postfix junction: the row's
    /// push-time type, or an invoked callee's declared return type (a
    /// completed send's row type is `Unknown` — its static type IS the
    /// callee's return type, the same face [`Self::receiver_static_ty`]
    /// reads at the junctions). A VALUE callee whose declared type is a
    /// callable class answers its `call` method's return instead — the
    /// invocation runs the implicit `call` dispatch (upstream's
    /// `InstanceCall` target resolution, `resolver_visitor.dart`: when the
    /// callee's static type declares a `call` method, the send's static
    /// type is that method's return type).
    pub(crate) fn operand_static_ty(&self, row: &Row) -> Ty {
        if !matches!(row.intent, Intent::Invoke) {
            return row.ty;
        }
        // A send on a formal whose written annotation is a function type
        // (`bool Function(E) test`) reads its signature's return — the
        // row's `ty` is the formal's own declared type (or a fragment of
        // the annotation the armed slot last completed), never the call's
        // result.
        if !row.qualified
            && let Some(sig) = self.fn_var_sigs.get(&row.name)
        {
            return sig.return_ty;
        }
        // The parameterized function-type alias face (`Predicate<T> test`
        // invoked as `test(item)`): the send's static type is the alias
        // RHS's return, the same face `callee_return_ty` answers.
        if !row.qualified
            && let RowShape::Bare(BareInfo::Plain {
                ty: Ty::Named { name, .. },
            }) = &row.shape
            && let Some(sig) = self.alias_fn_sig(*name)
        {
            return sig.return_ty;
        }
        if row.ty != Ty::UNKNOWN {
            // A function-typed callee (or the bare `Function` interface)
            // answers the send face ([`Self::invoked_callee_result`]).
            if let Some(result) = self.invoked_callee_result(row) {
                return result;
            }
            // The row keeps the callee's DECLARED type, never the send's
            // result — a spelled named type that is a type alias's head
            // (`Predicate<T> p` invoked as `p(x)`) answers nothing about
            // the call: the alias head is undecidable when its RHS is
            // unresolvable here (the verdict drops to the declared-return
            // walk).
            if let Ty::Named { name, .. } = row.ty {
                if let Some(decl) = self.top_level_decl(name)
                    && matches!(decl.kind, MicroKind::TypeAlias)
                {
                    return self.callee_return_ty(row);
                }
            }
            return self.callable_call_ret(row, row.ty).unwrap_or(row.ty);
        }
        self.callee_return_ty(row)
    }

    /// The implicit-`call` result of a VALUE send whose captured declared
    /// type is `callee_ty`: a class declaring (or inheriting) an instance
    /// `call` method makes the invocation run that method, so the send's
    /// static type is the method's return type. `None` keeps the captured
    /// declared face — the send is not a callable dispatch.
    ///
    /// The callee must be a VALUE: a creation send (`C()`, `p.C()`,
    /// `C.named()`) also carries the class head as its declared type, but
    /// its result is the created instance. Constructors are not walkable
    /// members (they live in the container's separate constructor table),
    /// so the callee-shape gate below excludes the creation forms outright.
    ///
    /// Generic containers answer `None`: the model cannot name a
    /// container's type parameters here (only the member's own names ride
    /// [`MemberEntry::type_param_names`]), so a return type mentioning the
    /// class's `T` would substitute to nothing — the declared face stays
    /// (under-report, never a wrong head).
    fn callable_call_ret(&self, row: &Row, callee_ty: Ty) -> Option<Ty> {
        let Ty::Named {
            name,
            nullable: false,
            ..
        } = callee_ty
        else {
            return None;
        };
        let value = match &row.shape {
            RowShape::Bare(BareInfo::Plain { .. }) => true,
            RowShape::Member { recv } => match recv {
                Receiver::Typed { type_name, .. }
                | Receiver::This {
                    container: type_name,
                    ..
                }
                | Receiver::Super {
                    container: type_name,
                    ..
                } => matches!(
                    self.walk_members(*type_name, row.name),
                    Walk::Found(entry)
                        if matches!(
                            entry.kind,
                            MemberKind::Field | MemberKind::Getter | MemberKind::Method
                        )
                ),
                _ => false,
            },
            _ => false,
        };
        if !value {
            return None;
        }
        let entry = self.resolved_member(name, self.intern("call"))?;
        if entry.kind != MemberKind::Method || entry.is_static || entry.type_params != 0 {
            return None;
        }
        (self.declared_type_params_of_container(name) == Some(0)).then_some(entry.return_ty)
    }

    /// `TypeSystem::flatten` at name level — see [`flatten_ty`].
    pub(crate) fn flatten(&self, ty: Ty) -> Ty {
        flatten_ty(self.strings, &self.boundary.pool(), ty)
    }

    /// `this`'s static type (the nullability checks and inferred-local
    /// initializers read it): the enclosing container for class-shaped
    /// members, the written `on` type — nullability included — inside a
    /// plain `extension` (`extension E on B?` makes `this : B?`; upstream
    /// `ExtensionImpl.extendedType`). An unresolved `on` clause keeps the
    /// extension's own name — the suppress-everything face below answers
    /// the same way it always has.
    pub(crate) fn this_static_ty(&self, container: StringId, kind: ContainerKind) -> Ty {
        if kind == ContainerKind::Extension {
            let elements = self.index.get(self.file);
            // The named face resolves by name; an unnamed extension's
            // `this` container is the `""` placeholder, so the face is the
            // frame-latched rank instead (the same latch
            // [`Engine::enclosing_extension_on_type`] reads — #2217: a
            // missed lookup answered `Ty::named("")`, a strictly
            // non-nullable type, and `on X?`'s `this` read as non-null).
            let face = elements.and_then(|f| {
                f.extension_faces()
                    .iter()
                    .find(|face| face.name == Some(container))
            });
            let face = face.or_else(|| {
                let innermost = self
                    .frames
                    .iter()
                    .rev()
                    .find(|f| matches!(f.kind, FrameKind::Container { .. }))?;
                let FrameKind::Container {
                    extension_face: Some(rank),
                    ..
                } = innermost.kind
                else {
                    return None;
                };
                elements?
                    .extension_faces()
                    .iter()
                    .filter(|face| face.name.is_none())
                    .nth(rank)
            });
            if let Some(face) = face
                && let Some(on) = face.on_type
            {
                // The interned `on` type carries its argument list
                // (`extension E<K, V> on Map<K, V>` gives `this :
                // Map<K, V>`); a clause that never interned falls back to
                // the bare head.
                if face.on_ty != Ty::Unknown {
                    return face.on_ty;
                }
                return Ty::Named {
                    name: on,
                    nullable: face.on_type_nullable,
                    args: ArgListId::EMPTY,
                };
            }
        }
        Ty::named(container)
    }

    /// A `var`-inferred initializer's resolved expression type
    /// (`end_variable_initializer`'s single-row window): the row's
    /// recorded type when it carries one, else the receiver faces whose
    /// static type lives off the row — `this`'s container, a creation's
    /// result (`C()` → `C`). Anything else stays `Unknown` (the
    /// suppress-everything sentinel — under-report, never over-report).
    /// The flow-aware initializer type for a bare read (#2174's inference
    /// feed): a read of the name whose latest staged plain-`=` write
    /// resolved a type (same executable, earlier offset — the feed keys on
    /// the write's executable serial, #2460 A2c) infers that type — the
    /// oracle's inference reads the promoted type of the initializer
    /// expression, and the declared slot the loop-head join demotes to must
    /// carry it (`var max = end` after
    /// `end = RangeError.checkValidRange(...)` infers `int`, not `int?`).
    /// Any other initializer shape keeps the row's own answer.
    pub(crate) fn staged_read_ty(&self, row: &Row) -> Option<Ty> {
        if !matches!(row.shape, RowShape::Bare(BareInfo::Plain { .. })) {
            return None;
        }
        let &(offset, exec, ty) = self.staged_write_tys.get(&row.name)?;
        (offset < row.range.offset && exec == self.exec_serial()).then_some(ty)
    }

    /// The innermost executable frame's serial — the executable-scoped
    /// identity the inference feed keys on ([`Self::staged_read_ty`]).
    /// Top-level writes and reads share serial 0 (no executable frame).
    pub(crate) fn exec_serial(&self) -> usize {
        self.frames
            .iter()
            .rev()
            .find(|f| matches!(f.kind, FrameKind::Executable { .. }))
            .map(|f| f.serial)
            .unwrap_or(0)
    }

    pub(crate) fn initializer_static_ty(&self, row: &Row) -> Option<Ty> {
        // A completed send's static type is the callee's RESULT, never
        // the callee's declared type the row keeps (`var s = f(x)` infers
        // `f`'s return — [`Self::operand_static_ty`] resolves it through
        // the local-signature, alias, and declared-return faces).
        if matches!(row.intent, Intent::Invoke) {
            let ty = self.operand_static_ty(row);
            if ty != Ty::UNKNOWN {
                // A `?.`-merged invocation's result carries the nullable
                // face when the merge could not join (`var a = c?.m()`
                // keeps `a` nullable) — the same member-offset latch the
                // read arm below re-arms.
                return Some(if self.null_aware_members.contains(&row.range.offset) {
                    Self::nullable_form(ty)
                } else {
                    ty
                });
            }
        } else if row.ty != Ty::UNKNOWN {
            // A `?.`-merged member read keeps the field's written type —
            // the null-aware `?` the merge could not join (`x?.f` reads
            // `f?`) re-arms here from the member's offset latch, so the
            // inferred slot carries the nullable face (`var a = c?.id`
            // keeps `a` nullable).
            let ty = row.ty;
            // An operator-merged row keeps its operator result, and a row
            // inside a statically-`bool` composite keeps `bool` — `var v =
            // x?.m == y` infers `bool`, never `bool?`.
            return Some(
                if self.null_aware_members.contains(&row.range.offset)
                    && !matches!(row.intent, Intent::Operator(..))
                    && !self.bool_decided_covers(row)
                {
                    Self::nullable_form(ty)
                } else {
                    ty
                },
            );
        }
        let ty = match &row.shape {
            RowShape::Receiver(Receiver::This {
                container, kind, ..
            }) => self.this_static_ty(*container, *kind),
            RowShape::Receiver(Receiver::Created {
                name: container, ..
            }) => Ty::named(*container),
            RowShape::Receiver(Receiver::Typed { ty, .. }) => *ty,
            _ => self.operand_static_ty(row),
        };
        if ty != Ty::UNKNOWN {
            return Some(ty);
        }
        // A method tear-off initializer (`var t = C<int>().m`): the member
        // row's `ty` stays `Unknown` (the model does not render method
        // function types on the row), but the tear-off's static type IS
        // the method's signature — the `t<A,B>` instantiation face reads
        // its parameter count (`wrong_number_of_type_arguments_function`,
        // upstream's `InvocationTargetFunctionTypedExpression` shape).
        self.tearoff_ty(row)
    }

    /// The tear-off function type of a member row resolving to a declared
    /// method: `M Function<M>(M)` for `m`'s `M m<M>(M)` — the member's own
    /// type-parameter count, written return type, and formal table
    /// interned into the pool's canonical shape. Getters, fields, broken
    /// chains, and unresolvable receivers suppress (`None`) — the
    /// under-report direction every `Unknown`-typed initializer shares.
    pub(crate) fn tearoff_ty(&self, row: &Row) -> Option<Ty> {
        let RowShape::Member { recv } = &row.shape else {
            return None;
        };
        if !matches!(row.intent, Intent::Read) {
            return None;
        }
        let container = match *recv {
            Receiver::This { container, .. }
            | Receiver::Super { container, .. }
            | Receiver::Typed {
                type_name: container,
                ..
            }
            | Receiver::Container {
                name: container, ..
            }
            | Receiver::Created {
                name: container, ..
            } => container,
            _ => return None,
        };
        let entry = self.resolved_member(container, row.name)?;
        if entry.kind != MemberKind::Method {
            return None;
        }
        let formals = entry.formals.as_deref();
        let positional: Vec<Ty> = formals
            .map(|f| f.positional.iter().map(|p| p.ty).collect())
            .unwrap_or_default();
        let named: Vec<(StringId, Ty)> = formals
            .map(|f| f.named.iter().map(|p| (p.name, p.ty)).collect())
            .unwrap_or_default();
        Some(self.boundary.args.borrow_mut().function(
            entry.type_params,
            entry.return_ty,
            &positional,
            &named,
            false,
        ))
    }

    /// The in-scope type-parameter test: `type_params`/`pending_type_params`
    /// are the de-facto "is this name a parameter" check — the written name
    /// carries no element at this layer (the `Ty` reads `Named{"T"}`).
    pub(crate) fn is_scope_param(&self, name: StringId) -> bool {
        self.type_params.contains(&name) || self.pending_type_params.contains(&name)
    }

    /// The captured bound of the in-scope parameter `name` (`None` = no
    /// bound — the implicit `Object?`): the declaring executable's window
    /// first (`void f<T extends num>(T t)`, the [`Engine::exec_bounds`]
    /// capture), then the enclosing container's capture (`class C<T
    /// extends num>`, the `container_params` face). Both record `(param,
    /// bound)` per owner in declaration order; the innermost owner wins.
    pub(crate) fn param_bound(&self, name: StringId) -> Option<Ty> {
        if let Some(exec) = self.exec_self_name() {
            if let Some(window) = self.exec_bounds.get(&exec)
                && let Some((_, bound)) = window.iter().find(|(param, _)| *param == name)
            {
                return *bound;
            }
            if let Some(window) = self.method_exec_bounds.get(&exec)
                && let Some((_, bound)) = window.iter().find(|(param, _)| *param == name)
            {
                return *bound;
            }
        }
        let container = self.frames.iter().rev().find_map(|f| match &f.kind {
            FrameKind::Container { name, .. } => Some(*name),
            _ => None,
        })?;
        let params = self.bounds.container_params(container)?;
        let tree = params.into_iter().find(|(param, _, _)| *param == name)?.1?;
        Some(self.tree_ty(&tree))
    }

    /// The post-walk counterpart of [`Engine::param_bound`]: the bound of
    /// the parameter `name` when the scope frames are already torn down
    /// (the extension-applicability row resolves a receiver argument
    /// spelled `T` long after `type_params`/`frames` emptied). The bound
    /// survives only in the per-executable captures — a top-level/generic
    /// function's `exec_bounds` window or a method's `method_exec_bounds`
    /// window — so the lookup scans every captured window for one that
    /// declares `name`. Returns `Some(bound)` when a capture declares
    /// `name` (the implicit `Object?` when its slot is empty), `None`
    /// when no window does — i.e. `name` is a concrete head (`String`,
    /// `Color`), never a scope parameter. A class-generic `class C<T>`
    /// receiver stays unmodeled (`container_params` needs the container
    /// name the post-walk caller no longer holds — the under-report
    /// direction).
    pub(crate) fn param_bound_of(&self, name: StringId) -> Option<Ty> {
        let implicit_object = || {
            Ty::Named {
                name: self.intern("Object"),
                nullable: true,
                args: ArgListId::EMPTY,
            }
        };
        for window in self
            .exec_bounds
            .values()
            .chain(self.method_exec_bounds.values())
        {
            if let Some((_, bound)) = window.iter().find(|(param, _)| *param == name) {
                return Some(bound.unwrap_or_else(implicit_object));
            }
        }
        None
    }

    /// Whether `ty` mentions an in-scope parameter anywhere in its shape
    /// (the official's `hasTypeParameterReference` gate over a bound: a
    /// bound that references a parameter — `T extends S` — proves no
    /// disjointness).
    fn ty_mentions_param(&self, ty: Ty) -> bool {
        let Ty::Named { name, args, .. } = ty else {
            return false;
        };
        if self.is_scope_param(name) {
            return true;
        }
        self.boundary
            .args
            .borrow()
            .get(args)
            .iter()
            .any(|&arg| self.ty_mentions_param(arg))
    }

    /// The bound-aware `isStrictlyNonNullable` (type_system.dart:1921-
    /// read through `TypeParameterType`'s bound): an in-scope unsubstituted
    /// parameter's nullability is its written bound's — a nullable bound or
    /// the implicit `Object?` (`T` alone) reads nullable-possible (`?.`
    /// stays legal, `!` stays live), a non-nullable bound keeps the
    /// strictly face (the oracle reports the dead operator over it). A
    /// bound that itself mentions a parameter stays unprovable; every
    /// other face answers [`Ty::is_strictly_non_nullable`] unchanged.
    pub(crate) fn strictly_non_nullable(&self, ty: Ty) -> bool {
        let Ty::Named {
            name,
            nullable: false,
            ..
        } = ty
        else {
            return ty.is_strictly_non_nullable();
        };
        if !self.is_scope_param(name) {
            return ty.is_strictly_non_nullable();
        }
        match self.param_bound(name) {
            None => false,
            Some(bound) => !self.ty_mentions_param(bound) && bound.is_strictly_non_nullable(),
        }
    }

    /// The `e!` pair (`visitNullAssertionExpression`,
    /// best_practices_verifier.dart:795-799, and the
    /// `_checkForUnnecessaryNullAware` gate, `error_verifier.dart`:
    /// 7668-7718): a `Null`-typed operand always throws
    /// (`null_check_always_fails`); a strictly-non-nullable operand makes
    /// the `!` dead (`unnecessary_non_null_assertion`). Exclusive — only
    /// the `Null` class fails the assertion.
    ///
    /// A local's read whose declared type decides neither arm stages for
    /// the teardown re-decision (the flow's promoted type at the read —
    /// a promoted-Never read after an earlier assertion — carries the
    /// verdict). Every decided operand rewrites its row type to the
    /// assertion's own static type (`promoteToNonNull`): the downstream
    /// member merges read the promoted receiver through it.
    pub(crate) fn check_null_assert(&mut self, at: SourceRange) {
        let (ty, read_span, qualified) = match self.rows.last() {
            Some(row) => {
                // The implicit-this getter read's row type is the unknown
                // face — `non_null_form(Unknown)` is a no-op and the
                // downstream member merge then falls back to the declared
                // nullable getter type (`receiver_static_ty`'s
                // `Bare(Plain{ty})` arm), over-reporting
                // `unchecked_use_of_nullable_value` and suppressing the
                // `unnecessary_null_comparison` twin beside it. Resolve the
                // merged equivalent static type first; the assertion's
                // non-null face then rewrites the row over it.
                let ty = self.operand_static_ty(row);
                (
                    if ty == Ty::UNKNOWN {
                        self.receiver_static_ty(row)
                    } else {
                        ty
                    },
                    row.range,
                    row.qualified,
                )
            }
            None => return,
        };
        if ty == Ty::Null {
            self.stage_usage(SemanticCode::NullCheckAlwaysFails, at, "");
        } else if self.strictly_non_nullable(ty) {
            self.stage_usage(SemanticCode::UnnecessaryNonNullAssertion, at, "");
        } else if ty != Ty::Unknown && !qualified && self.local_hits.contains_key(&read_span.offset)
        {
            // The staged re-decision reads the local promotion table at the
            // operand's own span, so it keys the operand's OWN local read —
            // a bare identifier. A receiver-chain row (`dep.dependencies!`)
            // sits on a member selector: the selector's spelling coinciding
            // with a captured local's name is not a reference to that local,
            // and the promotion table can only hold that shadow's type at
            // the selector span (the flow layer's name-keyed continuation
            // read) — staging would fabricate
            // `unnecessary_non_null_assertion` on a nullable member read
            // (riverpod foundation.dart:162, #2338). The member read's own
            // `!` verdict is its declared nullability; the field-promotion
            // re-decision rides the property table elsewhere, never this
            // one.
            self.null_assert_staged.push((read_span, at));
        }
        if let Some(row) = self.rows.last_mut() {
            row.ty = Self::non_null_form(ty);
        }
    }

    /// `_checkForInvariantNullComparison`
    /// (best_practices_verifier.dart:1216-1281, mirrored from
    /// `TypeSystem::is_unnecessary_null_comparison`): one side is the
    /// `null` literal and the other's type is strictly non-nullable —
    /// `==` is constant-false, `!=` constant-true. `super`/`this`
    /// receivers answer never-null without a type (the official's AST
    /// arm); an extension override suppresses, which the name level
    /// cannot see and skips with the rest of the untyped shapes. The
    /// caller pops the comparison's mark (the NaN walk shares it).
    pub(crate) fn null_comparison(&mut self, mark: usize, negated: bool) {
        let Some(right) = self.rows.last() else {
            return;
        };
        let Some(left) = mark.checked_sub(1).and_then(|index| self.rows.get(index)) else {
            return;
        };
        let right_is_null = right.ty == Ty::Null && matches!(right.shape, RowShape::Value);
        let left_is_null = left.ty == Ty::Null && matches!(left.shape, RowShape::Value);
        let (operand, null_row) = if right_is_null {
            (left, right)
        } else if left_is_null {
            (right, left)
        } else {
            return;
        };
        // A `?.`-merged member operand keeps the field's written type —
        // the null-aware `?` the merge could not join re-arms here from
        // the member's offset latch (`x?.f != null` reads `f?`).
        let operand_ty = if self.null_aware_members.contains(&operand.range.offset) {
            Self::nullable_form(operand.ty)
        } else {
            operand.ty
        };
        // A type-parameter read decides through its written bound (the
        // official `isStrictlyNonNullable(TypeParameterType)` reads the
        // bound): a nullable bound (`T extends AstNodeImpl?`) and the
        // implicit `Object?` (`T` alone) suppress, a non-nullable bound
        // (`T extends int`) decides never-null (probed 3.13.1).
        let strictly_non_null = self.strictly_non_nullable(operand_ty);
        let never_null = strictly_non_null
            || match &operand.shape {
                RowShape::Receiver(Receiver::Super { .. }) => true,
                // A `this` receiver answers through its written face: an
                // extension `on T?` keeps `this` nullable, so only a
                // non-nullable on-type (or a class's own `this`) is
                // never-null (lints-new `unnecessary_this_alias.dart`).
                RowShape::Receiver(Receiver::This { container, kind }) => self
                    .this_static_ty(*container, *kind)
                    .is_strictly_non_nullable(),
                _ => false,
            };
        if !never_null {
            return;
        }
        let code = if negated {
            SemanticCode::UnnecessaryNullComparisonTrue
        } else {
            SemanticCode::UnnecessaryNullComparisonFalse
        };
        // The oracle anchors at the operator-through-`null` extent (`==
        // null` / `null ==`), never the compared operand alone.
        let op = self
            .trimmed_span(operand.range.end(), null_row.range.offset)
            .or_else(|| self.trimmed_span(null_row.range.end(), operand.range.offset));
        let range = match op {
            Some(op) => {
                let offset = op.offset.min(null_row.range.offset);
                SourceRange {
                    offset,
                    length: op.end().max(null_row.range.end()) - offset,
                }
            }
            None => operand.range,
        };
        self.stage_usage(code, range, "");
    }

    /// `unnecessary_nan_comparison` (`visitBinaryOperatorInvocation`'s
    /// `checkLeftRight`, best_practices_verifier.dart:1234-1236 with the
    /// `isDoubleNan` extension :2264-2277): an operand that is the
    /// `double.nan` getter read makes the comparison constant — `!=` is
    /// always 'true', `==` always 'false'. The left operand wins when both
    /// are (the official's if/else), and the span runs from the NaN
    /// operand through the operator (`reportStartEnd`). The
    /// constant-pattern face (`visitConstantPattern` :317-324) is the
    /// documented under-report.
    pub(crate) fn check_nan_comparison(
        &mut self,
        mark: usize,
        negated: bool,
        op_range: SourceRange,
    ) {
        let Some(right) = self.rows.last() else {
            return;
        };
        let Some(left) = mark.checked_sub(1).and_then(|index| self.rows.get(index)) else {
            return;
        };
        let left_nan = self.is_double_nan(left);
        let right_nan = self.is_double_nan(right);
        if !left_nan && !right_nan {
            return;
        }
        let code = if negated {
            SemanticCode::UnnecessaryNanComparisonTrue
        } else {
            SemanticCode::UnnecessaryNanComparisonFalse
        };
        let range = if left_nan {
            // `reportStartEnd(diag, left, node.operator)` — the operand's
            // start is the receiver's (`double`, not the `nan` selector the
            // merged row anchors at).
            let start = self
                .property_access_span(left.range)
                .map_or(left.range.offset, |span| span.offset);
            SourceRange {
                offset: start,
                length: op_range.end().saturating_sub(start),
            }
        } else {
            SourceRange {
                offset: op_range.offset,
                length: right.range.end().saturating_sub(op_range.offset),
            }
        };
        self.stage_usage(code, range, "");
    }

    /// The `double.nan` getter read (`isDoubleNan`): a `double.nan` member
    /// row — the class-static read on the `double` container. A shadowing
    /// declaration of `double` resolves through the same face (the
    /// name-level edge; the oracle reads the resolved getter).
    pub(crate) fn is_double_nan(&self, row: &Row) -> bool {
        self.strings.resolve(row.name) == "nan"
            && matches!(
                &row.shape,
                RowShape::Member {
                    recv: Receiver::Container {
                        name,
                        kind: ContainerKind::Class,
                    },
                } if *name == self.members.primitive_ids[1]
            )
    }

    /// An equality operand's static type: the folded creation's container
    /// head (`B() == x` — both operand rows ride the send fold's
    /// `Receiver::Created` shape, the left under the operator mark), or
    /// the row's own static type (the declared read's snapshot, the
    /// invoked callee's declared return type). A bare type-literal
    /// operand (`C == x`) never folds and answers [`Ty::UNKNOWN`] — its
    /// comparison runs through `Object`'s own `==`, silent.
    pub(crate) fn equality_operand_ty(&self, row: &Row) -> Ty {
        if let RowShape::Receiver(Receiver::Created {
            name,
            kind: ContainerKind::Class,
        }) = row.shape
        {
            return Ty::named(name);
        }
        self.receiver_static_ty(row)
    }

    /// The `==`/`!=` invocation's right-operand face (`_resolveEqual`'s
    /// `checkForArgumentTypeNotAssignableForArgument(right,
    /// promoteParameterToNullable: true)`, binary_expression_resolver.dart
    /// :232-236): the left operand's declared `==` formal decides, folded
    /// through a non-parameterized alias first — the verdict reads the
    /// resolved interface type — while the message renders the written
    /// spelling with the official `makeNullable` join (`'IntAlias?'`,
    /// probed 3.13.1). The verdict is the exact-subtype cut
    /// ([`Engine::name_subtype`]): the assignability matrix answers a
    /// nullable target permissively (Right Top) and cannot decide the
    /// cross-head negative. Unresolved operands, a type parameter's head
    /// on either side (its bound is the name level's blind spot), a
    /// missing or non-method `==` face, `void` formals, and the
    /// int-literal → `double` context conversion all suppress (the
    /// under-report posture).
    pub(crate) fn check_equals_argument(&mut self, left: &Row, right: &Row) {
        let Ty::Named { name, .. } = self.equality_operand_ty(left) else {
            return;
        };
        let formals = self.member_formals_full(
            &Receiver::Typed {
                type_name: name,
                ty: Ty::UNKNOWN,
            },
            self.intern("=="),
        );
        let Some(formal) = formals
            .as_deref()
            .and_then(|formals| formals.positional.first())
        else {
            return;
        };
        let right_ty = self.equality_operand_ty(right);
        if matches!(formal.ty, Ty::Unknown | Ty::Void) || matches!(right_ty, Ty::Unknown | Ty::Void)
        {
            return;
        }
        let unbound = |engine: &Self, head| {
            engine.type_params.contains(&head) || engine.pending_type_params.contains(&head)
        };
        if let Ty::Named { name: head, .. } = formal.ty
            && unbound(self, head)
        {
            return;
        }
        if let Ty::Named { name: head, .. } = right_ty
            && unbound(self, head)
        {
            return;
        }
        let verdict_ty = Self::nullable_form(self.fold_alias_ty(formal.ty));
        // Every value inhabits `Object?`: the written-edge walk behind
        // [`Engine::name_subtype`] carries no implicit `extends Object`
        // row (a declaration-less `class C {}` would answer `false`
        // against the top), so the top-type target degenerates first —
        // the same rule [`NameWorld::decide_subtype`] bakes in. The
        // alias-to-`Object` formal lands here through the fold.
        if matches!(
            verdict_ty,
            Ty::Named {
                name: head,
                nullable: true,
                ..
            } if head == self.intern("Object")
        ) {
            return;
        }
        // The int-literal → `double` context conversion
        // (`analyzeIntLiteral`'s `convertToDouble`): the schema-typed
        // literal converts itself where the subtype cut refuses it.
        let literal_converts = Self::primitive_literal(right, &self.members.primitive_ids)
            && matches!(
                (&right_ty, &verdict_ty),
                (Ty::Named { name: from, .. }, Ty::Named { name: to, .. })
                    if *from == self.members.primitive_ids[0]
                        && *to == self.members.primitive_ids[1]
            );
        if !literal_converts && self.name_subtype(right_ty, verdict_ty) {
            return;
        }
        // Stage instead of emitting: the parse-time type is the unpromoted
        // face — a promotion standing at the operand's read still cleans
        // the row at the teardown join (#2161). The verdict rides the
        // `?`-promoted formal (`checkForArgumentTypeNotAssignableForArgument`
        // passes `promoteParameterToNullable: true` for `==`/`!=`,
        // binary_expression_resolver.dart:232-236).
        // The verdict read the folded alias; the row displays the WRITTEN
        // `?`-promoted formal (`'IntAlias?'`, probed 3.13.1). The staged
        // rejection stands unless a promotion re-decides it
        // ([`Engine::filter_staged`]).
        self.stage(
            StagedKind::Argument,
            right.range,
            right_ty,
            Self::nullable_form(formal.ty),
            Self::primitive_literal(right, &self.members.primitive_ids),
        );
    }

    /// The resolved binary-operator signature of `left OP right`: the
    /// `(param type, result type)` pair the oracle's resolved element
    /// declares (`_resolveUserDefinableType`,
    /// binary_expression_resolver.dart:479-515 reads
    /// `element.returnType`). The arm order mirrors the oracle's property
    /// resolution: an exact SDK row (`int.&`, `String.+`) first, then the
    /// user container's declared operator, then the `num`-inheritance
    /// fiction for the undeclared real-core `int`/`double` receivers (the
    /// SDK declarations are external, unreachable by the member walk).
    /// Unresolved receivers, absent operators (`undefined_operator`
    /// reports its own row), `dynamic`, and `Never` answer `None` — the
    /// under-report direction.
    pub(crate) fn binary_operator_sig(&self, op: &str, left: &Row) -> Option<(Ty, Ty)> {
        let left_ty = self.equality_operand_ty(left);
        let Ty::Named { name, .. } = left_ty else {
            return None;
        };
        // `dynamic`/type-parameter heads never reach the member face.
        if left_ty == Ty::Dynamic
            || self.type_params.contains(&name)
            || self.pending_type_params.contains(&name)
        {
            return None;
        }
        let recv_text = self.strings.resolve(name);
        if let Some((_, _, param, ret)) = CORE_BINARY_OPERATORS
            .iter()
            .find(|(recv, o, ..)| *o == op && *recv == recv_text)
        {
            return Some((Ty::named(self.intern(*param)), Ty::named(self.intern(*ret))));
        }
        // The resolved-operator arm: the user operator's declared formals
        // and return type (`Money operator +(Money)`). The oracle resolves
        // the property on the receiver type FIRST
        // (binary_expression_resolver.dart:451-459) — a unit-declared
        // `class int` with its own `operator +` answers its declared
        // signature, never the core fallback below.
        if let Some(entry) = self.resolved_member(name, self.intern(op)) {
            if entry.kind != MemberKind::Method {
                return None;
            }
            let formals = entry.formals.as_deref()?;
            let formal = formals.positional.first()?;
            return Some((formal.ty, entry.return_ty));
        }
        // The core-fiction arm: `int`/`double` inherit `num`'s arithmetic
        // and relational face (the SDK declarations are external,
        // unreachable by the member walk — the `num.+` signature answers
        // `1 + x` on `int` receivers too); `String`/`bool` are not `num`
        // receivers and never take this fallback. A head shadowed by an
        // in-scope container declaration skips the fiction — upstream's
        // property resolution on the shadowed type misses and the node
        // answers invalid, so the fold stays silent.
        if (recv_text == "int" || recv_text == "double")
            && resolve_container(
                self.graph,
                self.namespaces,
                self.index,
                self.lib,
                self.file,
                name,
            )
            .is_none()
        {
            if let Some((_, _, param, ret)) = CORE_BINARY_OPERATORS
                .iter()
                .find(|(recv, o, ..)| *o == op && *recv == "num")
            {
                return Some((Ty::named(self.intern(*param)), Ty::named(self.intern(*ret))));
            }
        }
        None
    }

    /// The `+`/`-`/`*`/`%` numeric refinement
    /// (`_refineNumericInvocationTypeNullSafe`, type_system.dart:
    /// 2059-2121): on a resolved operator whose receiver `T` reads
    /// `num?`-bounded, the node answers `double` when `T <: double?` or
    /// `S <: double?` (S non-Never), `int` when both sides read `int?`
    /// (S non-Never), `num` otherwise. The gate is the receiver's own
    /// bound — a user `extends num` class refines identically — so the
    /// call is unconditional on the resolved signature; non-`num`
    /// receivers and the other operators keep the declared return.
    pub(crate) fn refine_numeric_binary(
        &self,
        op: &str,
        left_ty: Ty,
        right: OperandFace,
        declared: Ty,
    ) -> Ty {
        if !matches!(op, "+" | "-" | "*" | "%") {
            return declared;
        }
        let (int, double, num) = (
            self.members.primitive_ids[0],
            self.members.primitive_ids[1],
            self.members.primitive_ids[2],
        );
        let named_q = |name| Ty::Named {
            name,
            nullable: true,
            args: ArgListId::EMPTY,
        };
        // The shadow guard: a container declaration shadowing a core
        // numeric name owns the operator face when no `num` is declared
        // for it to inherit — the fiction edges ([`CORE_SUPERTYPES`]'s
        // `int <: num`, the `num` signature fallback in
        // [`Self::binary_operator_sig`]) serve the undeclared real-core
        // names only. In the oracle's shadowed world the core `num` face
        // is gone (`class int {}` subtypes nothing numeric), so the node
        // answers the declared return unrefined; a declared
        // `class int extends num` hierarchy keeps the refinement (the
        // declared lattice answers the gate truthfully).
        // The verdict is memoized per head (`numeric_shadow_memo`, one
        // slot per core spelling): arithmetic on core-numeric receivers
        // is the most frequent typed expression shape, the probes are
        // run-stable, and the slot match admits exactly the three heads
        // the guard can ever fire on.
        if let Ty::Named { name, .. } = left_ty {
            let shadow_slot = match self.strings.resolve(name) {
                "int" => Some(0),
                "double" => Some(1),
                "num" => Some(2),
                _ => None,
            };
            if let Some(slot) = shadow_slot {
                let shadowed = match self.numeric_shadow_memo[slot].get() {
                    Some(shadowed) => shadowed,
                    None => {
                        let shadowed = resolve_container(
                            self.graph,
                            self.namespaces,
                            self.index,
                            self.lib,
                            self.file,
                            name,
                        )
                        .is_some()
                            && (name == num
                                || resolve_container(
                                    self.graph,
                                    self.namespaces,
                                    self.index,
                                    self.lib,
                                    self.file,
                                    num,
                                )
                                .is_none());
                        self.numeric_shadow_memo[slot].set(Some(shadowed));
                        shadowed
                    }
                };
                if shadowed {
                    return declared;
                }
            }
        }
        // The gate is the receiver's own bound — a user `extends num` class
        // refines identically — so the call is unconditional on the
        // resolved signature; non-`num` receivers and the other operators
        // keep the declared return.
        if !self.name_subtype(left_ty, named_q(num)) {
            return declared;
        }
        // `_refineNumericInvocationTypeNullSafe`'s branch ladder
        // (type_system.dart:2093-2118). `T <: double?` decides first,
        // whatever `S` is ("this includes S being dynamic or Never").
        if self.name_subtype(left_ty, named_q(double)) {
            return Ty::named(double);
        }
        let right_ty = match right {
            // The oracle's InvalidType operand — an undefined name, a
            // member read on an unresolvable receiver — is a subtype of
            // everything (subtype.dart's `InvalidTypeImpl` arm), so it
            // decides the `double` branch: `int - <invalid>` answers
            // `double` and the downstream `int` sinks report (probed
            // 3.13.1: `g(length: node.end - operator.offset)` with `Token`
            // undefined reports the `double`-against-`int` argument row).
            // An unresolvable written head on a typed operand is the same
            // face (upstream types the undefined annotation InvalidType).
            OperandFace::Invalid => return Ty::named(double),
            // A face the engine cannot type though the oracle can — the
            // node's own type is unknowable here, so every downstream
            // check suppresses (the under-report direction) rather than
            // reading the gap as InvalidType.
            OperandFace::Gap => return Ty::UNKNOWN,
            // `dynamic` and `void` are Left Top (`Object? <: double?` is
            // false, as is `<: int?`): neither branch fires and the node
            // answers `num` (`int + dynamic` is `num`, so an `int` sink
            // still reports — with `num` in the message).
            OperandFace::Typed(Ty::Dynamic | Ty::Void) => return Ty::named(num),
            OperandFace::Typed(ty) => ty,
        };
        let right_invalid = match right_ty {
            Ty::Named { name, .. } => !self.name_world().is_known_head(name),
            _ => false,
        };
        // `Never` stays excluded (the upstream `!s.isBottom` cut) and
        // falls to `num`.
        if right_ty != Ty::Never
            && (right_invalid || self.name_subtype(right_ty, named_q(double)))
        {
            return Ty::named(double);
        }
        if right_ty != Ty::Never
            && self.name_subtype(left_ty, named_q(int))
            && self.name_subtype(right_ty, named_q(int))
        {
            return Ty::named(int);
        }
        Ty::named(num)
    }

    /// The binary expression's fold (`end_binary_expression`'s tail): the
    /// oracle's `BinaryExpression`/`LogicalExpression` node carries one
    /// static type — the resolved operator's return, numeric-refined for
    /// `+ - * %` on `num`-family receivers, `bool` for the `==`/`!=`/
    /// `&&`/`||` composites. The operand rows leave the live stack for
    /// `deferred_miss_rows` (their own resolution — `undefined_operator`,
    /// the `used_names` marks, the operand misses — still runs in pass 2),
    /// and one [`RowShape::Value`] row spanning `left..right` takes their
    /// place so the enclosing single-row checks (`return_of_invalid_type`,
    /// the argument faces, the assignability consumers) read the node's
    /// own type.
    ///
    /// The right operand's `checkForArgumentTypeNotAssignableForArgument`
    /// (binary_expression_resolver.dart:364-367, non-nullable formals —
    /// `promoteParameterToNullable` is the `==` face's own) runs against
    /// the resolved first formal under the same suppress gates the `==`
    /// check applies.
    pub(crate) fn fold_binary(&mut self, op: &str, op_range: SourceRange) {
        let op = self.intern(op);
        // The left operand row carries the operator mark
        // (`begin_binary_expression`'s `mark_operator`); the right operand
        // is the last non-TypeUse row above it. Anything else between
        // them is debris the pass-2 walk still owes a verdict.
        let mut left_index = self
            .rows
            .iter()
            .rposition(|row| matches!(row.intent, Intent::Operator(o, _) if o == op));
        if left_index.is_none() {
            // The statically-`bool` operators carry no mark
            // (`mark_operator` only tags `USER_OPERATORS`) — the left
            // operand is then the last row wholly before the operator: an
            // interpolated-string right operand's inner reads end after
            // the operator and never qualify. Without the fold the right
            // operand stays on top and downstream type reads (`var`/
            // `final` inference, `return`, enclosing conditions) see ITS
            // type instead of `bool` — `flag = s == path` inferred the
            // right operand's type and `if (x == '$a')` fired
            // `non_bool_condition` on the literal row.
            let bool_op = op == self.intern("==")
                || op == self.intern("!=")
                || op == self.intern("&&")
                || op == self.intern("||")
                || op == self.intern("<")
                || op == self.intern(">")
                || op == self.intern("<=")
                || op == self.intern(">=");
            if !bool_op {
                return;
            }
            left_index = self.rows.iter().rposition(|row| {
                !matches!(row.shape, RowShape::TypeUse(..)) && row.range.end() <= op_range.offset
            });
        }
        let Some(left_index) = left_index else {
            return;
        };
        let Some(right_index) = self
            .rows
            .iter()
            .rposition(|row| !matches!(row.shape, RowShape::TypeUse(..)))
        else {
            return;
        };
        if right_index <= left_index {
            return;
        }
        let left = self.rows[left_index].clone();
        let right = self.rows[right_index].clone();
        let left_ty = self.equality_operand_ty(&left);
        let right_ty = self.equality_operand_ty(&right);
        let bool_id = self.members.primitive_ids[3];
        let result = if matches!(op, o if o == self.intern("==") || o == self.intern("!=") || o == self.intern("&&") || o == self.intern("||"))
        {
            // `==`/`!=` and the lazy boolean composites are `bool` by
            // construction (binary_expression_resolver.dart:499-500 and
            // `_resolveLogicalExpression`).
            Ty::named(bool_id)
        } else if op == self.intern("??") {
            // The if-null LUB is outside the name-level model — the
            // under-report direction.
            Ty::UNKNOWN
        } else {
            let op_text = self.strings.resolve(op).to_string();
            match self.binary_operator_sig(&op_text, &left) {
                Some((formal, declared)) => {
                    // The argument face: the right operand against the
                    // resolved formal — same gating as
                    // [`Self::check_equals_argument`], minus the
                    // nullable-formal promotion (the generic call passes
                    // the declared formal unchanged).
                    self.check_binary_argument(right_ty, formal, &right);
                    if left_ty == Ty::Dynamic {
                        // `d + 1` on `dynamic` keeps the dynamic face
                        // (binary_expression_resolver.dart:501-502).
                        Ty::Dynamic
                    } else {
                        // The refinement's own `num?` receiver gate
                        // decides — unconditional here.
                        let face = self.operand_face(&right, right_ty);
                        self.refine_numeric_binary(&op_text, left_ty, face, declared)
                    }
                }
                None => Ty::UNKNOWN,
            }
        };
        let _ = op_range;
        // Fold: the operands defer to pass 2 (their uses, misses, and
        // operator-definedness verdicts ride `deferred_miss_rows`), the
        // node's own row answers the enclosing check. A `mark_operator`
        // on an outer operator re-tags the folded row by intent — the
        // operand face needs `Read` here.
        for row in self.rows.drain(left_index..) {
            self.deferred_miss_rows.push(row);
        }
        self.rows.push(Row {
            name: self.intern(""),
            range: SourceRange {
                offset: left.range.offset,
                length: right.range.end().saturating_sub(left.range.offset),
            },
            qualified: false,
            shape: RowShape::Value,
            intent: Intent::Read,
            write_op: None,
            ty: result,
        });
    }

    /// The right operand's `argument_type_not_assignable` verdict against
    /// the operator's first formal — the generic face of
    /// [`Self::check_equals_argument`]'s
    /// `checkForArgumentTypeNotAssignableForArgument` (the
    /// `promoteParameterToNullable: true` overlay is the `==`/`!=`
    /// spelling's own; the other user operators' calls pass the declared
    /// formal unchanged). The suppress gates mirror it: unknown or `void`
    /// sides, a `dynamic` operand (`isAssignableTo` — the oracle's verdict
    /// is assignability, and `dynamic` fits every target; probed 3.13.1:
    /// `const x = 'a' + b` with `const dynamic b = true` emits only the
    /// const-eval fault), in-scope type-parameter heads, the `Object?`
    /// top-type formal, and the int-literal → `double` conversion all
    /// stay silent.
    pub(crate) fn check_binary_argument(&mut self, right_ty: Ty, formal: Ty, right: &Row) {
        if matches!(formal, Ty::Unknown | Ty::Void)
            || matches!(right_ty, Ty::Unknown | Ty::Void | Ty::Dynamic)
        {
            return;
        }
        let unbound = |engine: &Self, head| {
            engine.type_params.contains(&head) || engine.pending_type_params.contains(&head)
        };
        if let Ty::Named { name: head, .. } = formal
            && unbound(self, head)
        {
            return;
        }
        if let Ty::Named { name: head, .. } = right_ty
            && unbound(self, head)
        {
            return;
        }
        // The `Object?` target degenerates (every value inhabits it —
        // `name_subtype`'s written-edge walk carries no implicit
        // `extends Object` row, so the top-type face answers first).
        if matches!(
            formal,
            Ty::Named {
                name: head,
                nullable: true,
                ..
            } if head == self.intern("Object")
        ) {
            return;
        }
        // `analyzeIntLiteral`'s `convertToDouble`: the schema-typed int
        // literal converts into a `double` formal.
        let literal_converts = Self::primitive_literal(right, &self.members.primitive_ids)
            && matches!(
                (right_ty, formal),
                (Ty::Named { name: from, .. }, Ty::Named { name: to, .. })
                    if from == self.members.primitive_ids[0]
                        && to == self.members.primitive_ids[1]
            );
        if literal_converts || self.name_subtype(right_ty, formal) {
            return;
        }
        // Stage instead of emitting: the parse-time type is the unpromoted
        // face — a `??=`/test promotion standing at the operand's read still
        // cleans the row at the teardown join (#2161 real-corpus:
        // `hangingIndent ??= 0` then `1 - hangingIndent` reported `int?`).
        self.stage(
            StagedKind::Argument,
            right.range,
            right_ty,
            formal,
            Self::primitive_literal(right, &self.members.primitive_ids),
        );
    }

    /// The cast-pattern pair (shared_type_analyzer.dart:145-152 and
    /// `checkPatternNeverMatchesValueType`'s Cast kind,
    /// type_analyzer.dart:723-756): a matched type already below the
    /// target makes the cast dead (`unnecessary_cast_pattern`); a matched
    /// type no value of which can inhabit the target proves the pattern
    /// dead (`pattern_never_matches_value_type`). The two are exclusive
    /// (`canBeSubtypeOf ⊇ isSubtypeOf`). Root patterns only: a nested
    /// pattern's matched type is the enclosing property's column, which
    /// the name level does not descend (the under-report).
    pub(crate) fn check_cast_pattern(&mut self, at: SourceRange, cast: Option<Ty>) {
        let Some(cast) = cast else { return };
        if cast == Ty::Unknown {
            return;
        }
        if !self.switch.case_sink_open || self.switch.pattern_stack.len() != 1 {
            return;
        }
        let Some(ctx) = self.switch.switches.last() else {
            return;
        };
        let matched = ctx.scrutinee;
        if matched == Ty::Unknown {
            return;
        }
        if self.name_subtype(matched, cast) {
            // The official gate is `isSubtypeOf && castType is not
            // InvalidType` — a `dynamic` target reports like any other.
            self.stage_usage(SemanticCode::UnnecessaryCastPattern, at, "");
            return;
        }
        if !self.name_can_be_subtype(matched, cast) {
            let a0 = self.ty_display(matched);
            let a1 = self.ty_display(cast);
            self.stage_typed(SemanticCode::PatternNeverMatchesValueType, at, &a0, &a1);
        }
    }

    /// `cast_from_null_always_fails`'s pattern face (`visitCastPattern`,
    /// best_practices_verifier.dart:227-239): a cast pattern whose matched
    /// value type is `Null` and whose target is non-nullable. The matched
    /// type is the open switch's scrutinee — the proxy
    /// [`Engine::check_cast_pattern`] consumes — under the same root gate.
    pub(crate) fn check_cast_pattern_null(&mut self, at: SourceRange, cast: Option<Ty>) {
        let Some(Ty::Named {
            nullable: false, ..
        }) = cast
        else {
            return;
        };
        if !self.switch.case_sink_open || self.switch.pattern_stack.len() != 1 {
            return;
        }
        let Some(ctx) = self.switch.switches.last() else {
            return;
        };
        if ctx.scrutinee == Ty::Null {
            self.stage_usage(SemanticCode::CastFromNullAlwaysFails, at, "");
        }
    }

    /// `checkPatternNeverMatchesValueType`'s DeclaredVariable/Wildcard/
    /// Object kinds (type_analyzer.dart:723-756): a required type no
    /// matched value can inhabit proves the pattern dead. Root patterns
    /// and refutable contexts only (the official's
    /// `irrefutableContext == null` gate; declaration patterns suppress).
    /// The verdict reads the type folded through a non-parameterized
    /// alias first — the official `requiredType` carries the written
    /// alias into every kind and unwraps it for `canBeSubtypeOf` — while
    /// the message renders the written spelling (`'IntAlias'`, probed
    /// 3.13.1).
    pub(crate) fn check_pattern_never_matches(&mut self, at: SourceRange, required: Ty) {
        if required == Ty::Unknown || required == Ty::Dynamic {
            return;
        }
        if !self.switch.case_sink_open || self.switch.pattern_stack.len() != 1 {
            return;
        }
        let Some(ctx) = self.switch.switches.last() else {
            return;
        };
        let matched = ctx.scrutinee;
        if matched == Ty::Unknown {
            return;
        }
        if !self.name_can_be_subtype(matched, self.fold_alias_ty(required)) {
            let a0 = self.ty_display(matched);
            let a1 = self.ty_display(required);
            self.stage_typed(SemanticCode::PatternNeverMatchesValueType, at, &a0, &a1);
        }
    }

    /// `unnecessary_wildcard_pattern` (`unnecessaryWildcardPattern`,
    /// shared_type_analyzer.dart:250-258, fed by `analyzeWildcardPattern`
    /// type_analyzer.dart:2731-2737): a wildcard pattern that is a direct
    /// operand of a logical-and pattern and always matches. Only the
    /// logical-and kind is wired in the official (`OR` dispatch resets the
    /// kind to null, type_analyzer.dart:1347-1359 vs :1356); a bare `_` is
    /// always matching (no declared type), a typed one when the matched
    /// type already sits below the declaration (`promoteForPattern`'s
    /// unchanged-type face, mirrored with the exact-subtype gate like the
    /// cast-pattern check). Deep propagation through parenthesized and
    /// other kind-resetting wrappers is the documented under-report.
    ///
    /// Both operands of one `&&` report (`type_inference_test.dart`'s
    /// WILDCARD1+WILDCARD2 shape). The operator event follows the left
    /// operand, so a wildcard recorded under a frame without a binary
    /// context arms when the `&&` arrives
    /// ([`Engine::check_pending_wildcards`]); a wildcard whose frame the
    /// `&&` already opened checks here.
    pub(crate) fn check_wildcard_pattern(&mut self, at: SourceRange, declared: Option<Ty>) {
        if !self.switch.case_sink_open {
            return;
        }
        let binary = self.switch.pattern_stack.last().map(|frame| frame.binary);
        match binary {
            Some(Some(false)) => self.wildcard_row(at, declared),
            // A root (or not-yet-binary) frame: the deciding `&&` follows.
            Some(_) => {
                if let Some(frame) = self.switch.pattern_stack.last_mut() {
                    frame.wildcards.push((at, declared));
                }
            }
            None => {}
        }
    }

    /// The `&&` arrived: the top frame's recorded wildcards are its direct
    /// left operands (the right operand opens its own frame after this
    /// event). Each runs the same row the right-operand face runs.
    pub(crate) fn check_pending_wildcards(&mut self) {
        let pending = match self.switch.pattern_stack.last_mut() {
            Some(frame) => std::mem::take(&mut frame.wildcards),
            None => return,
        };
        for (at, declared) in pending {
            self.wildcard_row(at, declared);
        }
    }

    /// One `unnecessary_wildcard_pattern` row: the shared conditions of
    /// both operand faces (the case-header window, the all-binary frames
    /// above the root, the typed wildcard's already-below gate).
    pub(crate) fn wildcard_row(&mut self, at: SourceRange, declared: Option<Ty>) {
        if !self.switch.case_sink_open {
            return;
        }
        // The matched type is the scrutinee only under binary frames (the
        // constant-pattern walk's gate).
        if !self
            .switch
            .pattern_stack
            .iter()
            .skip(1)
            .all(|f| f.binary.is_some())
        {
            return;
        }
        if let Some(declared) = declared {
            if declared == Ty::Unknown {
                return;
            }
            let matched = self
                .switch
                .switches
                .last()
                .map_or(Ty::Unknown, |c| c.scrutinee);
            if matched == Ty::Unknown || !self.name_subtype(matched, declared) {
                return;
            }
        }
        self.stage_usage(SemanticCode::UnnecessaryWildcardPattern, at, "");
    }

    /// The null-check/assert pattern pair
    /// (shared_type_analyzer.dart:129-139): over a non-nullable matched
    /// type both forms are dead. The official's irrefutable-context error
    /// takes precedence there, so only refutable contexts (open switch
    /// cases) report; root patterns only, as in
    /// [`Self::check_pattern_never_matches`].
    pub(crate) fn check_null_pattern(&mut self, at: SourceRange, is_assert: bool) {
        if !self.switch.case_sink_open || self.switch.pattern_stack.len() != 1 {
            return;
        }
        let Some(ctx) = self.switch.switches.last() else {
            return;
        };
        if !self.strictly_non_nullable(ctx.scrutinee) {
            return;
        }
        let code = if is_assert {
            SemanticCode::UnnecessaryNullAssertPattern
        } else {
            SemanticCode::UnnecessaryNullCheckPattern
        };
        self.stage_usage(code, at, "");
    }

    /// The never-equal verdict between a constant's type and a matched
    /// value's type (`TypeSystem::can_be_equal` minus the value domain):
    /// the int-constant-vs-double-value cross answers "possible" (the
    /// official's JavaScript note), otherwise the name-level equality
    /// relation decides. Shared by the parse-time check and the teardown
    /// re-decision (#1351).
    ///
    /// An in-scope unsubstituted parameter stands in its written bound
    /// (upstream Dart SDK source, pkg/analyzer/lib/src/dart/constant/
    /// constant_verifier.dart:577-586, `_canBeEqual`'s TypeParameterType
    /// arm: the bound — made nullable under a `?` suffix — replaces the
    /// parameter face when it references no parameter itself): the
    /// disjointness proof reads the bound (`case 1` never equals a `T
    /// extends Color` value, `case Color.red` can). An unbounded or
    /// parameter-mentioning bound, and a parameter-spelled constant face
    /// (the official's InterfaceType gate never descends for one), answer
    /// "possible" — disjointness unprovable. A parameter bound to a
    /// concrete type before the comparison (the solved-call substitution)
    /// no longer spells a parameter here and decides ordinarily.
    ///
    /// A matched head no declared face resolves answers "possible" too:
    /// an out-of-scope parameter spelling (an unsolved generic call's
    /// residual `T`, its bound living at the declaring executable, not
    /// this scope) and an unresolved type name are the official's
    /// boundless-TypeParameterType/InvalidType value faces, which fall to
    /// the unsupported `return true` (upstream Dart SDK source,
    /// pkg/analyzer/lib/src/dart/constant/constant_verifier.dart:577-596).
    /// `is_known_head` is the same declared-face test the mutual-exclusion
    /// relation's opaque-right arm reads.
    pub(crate) fn constant_never_matches(&self, constant: Ty, matched: Ty) -> bool {
        let matched = match &matched {
            Ty::Named { name, nullable, .. } if self.is_scope_param(*name) => {
                match self.param_bound(*name) {
                    Some(bound) if !self.ty_mentions_param(bound) => {
                        if *nullable {
                            Self::nullable_form(bound)
                        } else {
                            bound
                        }
                    }
                    _ => return false,
                }
            }
            _ => matched,
        };
        if let Ty::Named { name, .. } = &matched
            && !self.name_world().is_known_head(*name)
        {
            return false;
        }
        if let Ty::Named {
            name,
            nullable: false,
            ..
        } = constant
            && self.is_scope_param(name)
        {
            return false;
        }
        if let (Ty::Named { name: c, .. }, Ty::Named { name: v, .. }) = (constant, matched) {
            let (int, double) = (self.members.primitive_ids[0], self.members.primitive_ids[1]);
            if (c == int && v == double) || (c == double && v == int) {
                return false;
            }
        }
        !self.name_can_be_equal(constant, matched)
    }

    /// `visitConstantPattern`'s gate (constant_verifier.dart:140-165,
    /// mirrored from `TypeSystem::can_be_equal`): the
    /// `hasPrimitiveEquality` constants — int/double/String/bool/Null and
    /// enum tails, exactly the arming sites — report when the matched
    /// value's type can never equal the constant's type. The
    /// int-constant-vs-double-value cross (the official's JavaScript
    /// note, a name-level question the declaration-opaque face cannot
    /// answer) answers "possible" before the relation.
    ///
    /// The visitor descends into logical-or/and operands with the same
    /// matched type, so constant leaves under binary-operand frames check
    /// like root leaves (`case true || null:`); field sub-pattern frames
    /// suppress — their matched type is the property column, which the
    /// name level does not descend into (the under-report). A passing
    /// candidate defers to the teardown join when a clause anchor stands
    /// (the flow's promoted matched type can narrow the verdict, #1351).
    pub(crate) fn check_constant_pattern(&mut self, constant: Option<Ty>, at: Option<SourceRange>) {
        let (Some(constant), Some(at)) = (constant, at) else {
            return;
        };
        if !self.switch.case_sink_open
            || !self
                .switch
                .pattern_stack
                .iter()
                .skip(1)
                .all(|f| f.binary.is_some())
        {
            return;
        }
        let Some(ctx) = self.switch.switches.last() else {
            return;
        };
        let matched = ctx.scrutinee;
        if matched == Ty::Unknown {
            return;
        }
        if self.constant_never_matches(constant, matched) {
            let a0 = self.ty_display(matched);
            let a1 = self.ty_display(constant);
            self.stage_typed(
                SemanticCode::ConstantPatternNeverMatchesValueType,
                at,
                &a0,
                &a1,
            );
        } else if let Some(clause) = self.clause_anchor {
            // The declared-scrutinee check passed; the flow's promoted
            // matched type re-decides at the teardown join (#1351) — a
            // narrowed matched type can never equal the constant where
            // the declared type could. A matched face still spelling an
            // in-scope parameter skips the push: its bound relation
            // decided here, and after the walk drains the scope stacks
            // the same spelling re-arrives undecidable (a promoted read
            // instead arrives concretely typed and re-decides).
            let param_matched =
                matches!(&matched, Ty::Named { name, .. } if self.is_scope_param(*name));
            if !param_matched {
                self.pending_matched.push(PendingMatched {
                    constant,
                    at,
                    clause,
                });
            }
        }
    }

    /// The `bool?` verdict of a condition operand's flow-aware static
    /// type (`isDartCoreBool` + nullable — the official
    /// `checkForNonBoolExpression` fallback: assignable to `bool` returns
    /// early, a `bool`-typed but nullable static type reports).
    pub(crate) fn nullable_bool_ty(&self, ty: Ty) -> bool {
        match ty {
            Ty::Named {
                name,
                nullable: true,
                ..
            } => name == self.members.primitive_ids[3],
            _ => false,
        }
    }

    /// The condition-operand face (`checkForNonBoolExpression`'s
    /// unchecked arm, bool_expression_verifier.dart:45-90): the operand's
    /// parse-time static type rides into the staging; the teardown join
    /// substitutes the flow's condition-type row at the operand read and
    /// re-decides (the flow-aware static type — a promoted read is
    /// non-nullable `bool` and stays silent).
    pub(crate) fn check_condition_operand(&mut self) {
        // An if-case condition skips the plain checkpoint: the official
        // driver routes the statement through `analyzeIfCaseStatement`,
        // which never reaches `handle_ifStatement_conditionEnd`.
        if self.cond_case {
            self.cond_case = false;
            self.cond_mark = None;
            return;
        }
        let row = self.rows.last().cloned();
        let Some(row) = row else {
            return;
        };
        // The condition's own extent: the rows pushed since the statement
        // keyword (`begin_if_statement`/`begin_while_statement`) are exactly
        // the condition's operand rows — the oracle anchors
        // `non_bool_condition` at the whole condition node
        // (`bool_expression_verifier.dart:47` `.at(expression)`). `None`
        // leaves the anchor at the top row's own span (the sites without a
        // keyword-mark event).
        let mark = self.cond_mark.take();
        self.classify_condition_marked(&row, mark);
    }

    /// One `unchecked_use_of_nullable_value` candidate per operand site:
    /// a `bool?`-typed operand stages (the teardown join re-decides the
    /// staged `from` against the flow's condition-type row at the same
    /// offset); the dedup keeps an enclosing junction's re-read of the
    /// same last row (the `&&` arm classifies both operands, then the
    /// enclosing `if` re-reads the right one) from doubling.
    pub(crate) fn classify_condition(&mut self, row: &Row) {
        self.classify_condition_marked(row, None);
    }

    /// The condition operand's two faces in one decision: the nullable-bool
    /// staging above, and `non_bool_condition` (`checkForNonBoolExpression`,
    /// bool_expression_verifier.dart:36-60) — a known non-`bool` static
    /// type reports at the condition's extent, unless a statically-`bool`
    /// composite (`==`, `&&`, `is`, `as`, a negation operand) already
    /// decided the span, or the site is a negation operand (the dedicated
    /// `non_bool_negation_expression` face replaces the condition face
    /// there, and an invalid `this`/`super` receiver's
    /// `InvalidType` also suppresses it upstream).
    pub(crate) fn classify_condition_marked(&mut self, row: &Row, mark: Option<usize>) {
        if self.condition_staged.contains(&row.range.offset) {
            return;
        }
        let ty = self.operand_static_ty(row);
        if self.nullable_bool_ty(ty) {
            self.stage_condition_offset(row.range.offset);
            self.staged_checks.push(StagedCheck {
                kind: StagedKind::NullableCondition,
                range: row.range,
                from: ty,
                to: Ty::UNKNOWN,
                literal: false,
            });
            return;
        }
        if !self.bool_decided_covers(row) && self.known_non_bool(ty) {
            // The constant-context gate (`error_verifier.dart`'s
            // `_isInConstantContext` arms): a constant expression's faults
            // belong to the const evaluator — the expression faces stay
            // silent there.
            let anchor = match mark {
                Some(mark) => {
                    let first = self.rows.get(mark).map(|first| first.range.offset);
                    first
                        .filter(|first| *first <= row.range.offset)
                        .map(|first| SourceRange {
                            offset: first,
                            length: row.range.end() - first,
                        })
                }
                None => None,
            }
            .unwrap_or(row.range);
            // A promotable read's parse-time non-`bool` type can still be
            // promoted to `bool` by teardown (`dartValue is bool &&
            // dartValue` — the promoted operand is the oracle's own
            // silent face): stage for the flow join instead of emitting
            // eagerly. The `literal` flag marks the non-`bool` candidate
            // inside the nullable-condition kind (the staged kinds cannot
            // grow here).
            if self.local_hits.contains_key(&row.range.offset) {
                self.staged_checks.push(StagedCheck {
                    kind: StagedKind::NullableCondition,
                    range: row.range,
                    from: ty,
                    to: Ty::UNKNOWN,
                    literal: true,
                });
            } else if !self.const_capture.open() {
                self.emit(SemanticCode::NonBoolCondition, anchor, "", "");
            }
            self.stage_condition_offset(row.range.offset);
        }
    }

    /// The `!` operand's classification: the nullable-bool staging runs,
    /// the condition face does not (the negation face owns the report).
    pub(crate) fn classify_negation_operand(&mut self, row: &Row) {
        let ty = self.operand_static_ty(row);
        if !self.condition_staged.contains(&row.range.offset) && self.nullable_bool_ty(ty) {
            self.stage_condition_offset(row.range.offset);
            self.staged_checks.push(StagedCheck {
                kind: StagedKind::NullableCondition,
                range: row.range,
                from: ty,
                to: Ty::UNKNOWN,
                literal: false,
            });
        }
    }

    /// The bounded staging-only variant: the last value row wholly inside
    /// the span stages for the nullable-condition teardown, and the
    /// condition face never fires (the recovery-borrowed
    /// expression/initializer assert windows own no oracle condition
    /// node).
    pub(crate) fn stage_condition_operand_within(&mut self, start: u32, end: u32) {
        let row = self
            .rows
            .iter()
            .rev()
            .find(|row| row.range.offset >= start && row.range.end() <= end);
        let Some(row) = row else {
            return;
        };
        let row = row.clone();
        self.classify_negation_operand(&row);
    }

    pub(crate) fn stage_condition_offset(&mut self, offset: u32) {
        self.condition_staged.push(offset);
        let excess = self.condition_staged.len().saturating_sub(32);
        if excess > 0 {
            self.condition_staged.drain(..excess + 28);
        }
    }

    /// The known non-`bool` verdict of a condition operand's static type:
    /// every named non-`bool` type, `null`, and function types report;
    /// `dynamic`, `Never`, and unknown types are assignable-or-undecidable
    /// and stay silent. A spelled type alias folds to its RHS face first —
    /// an alias head whose RHS this layer cannot resolve (parameterized
    /// or cross-file) is undecidable, not non-`bool`. `void` stays silent
    /// too — the oracle answers `use_of_void_result` there instead
    /// (`bool_expression_verifier.dart:81` early-returns).
    pub(crate) fn known_non_bool(&self, ty: Ty) -> bool {
        match ty {
            Ty::Named { .. } => match self.fold_alias_ty(ty) {
                Ty::Named { name, .. } => {
                    // An out-of-scope head is a foreign projection this unit
                    // cannot resolve (an unexported function-type alias rides
                    // inference without an import — drift's `Predicate<T>`):
                    // class-vs-alias is undecidable at the name level while
                    // the oracle resolves it, so the condition faces keep
                    // silent. The dart:core primitives have no declaring
                    // stub, yet their non-`bool` verdicts are known.
                    if self.top_level_decl(name).is_none()
                        && !self.members.primitive_ids.contains(&name)
                    {
                        return false;
                    }
                    name != self.members.primitive_ids[3]
                        && !matches!(
                            self.top_level_decl(name).map(|decl| decl.kind),
                            Some(MicroKind::TypeAlias)
                        )
                }
                Ty::Function { .. } => true,
                _ => false,
            },
            Ty::Function { .. } | Ty::Null => true,
            _ => false,
        }
    }

    /// Latches `(start, end)` as a statically-`bool` extent. Same small
    /// ring as [`Self::eq_spans`]; stale entries are exact-span matched.
    pub(crate) fn latch_bool_span(&mut self, start: u32, end: u32) {
        if end < start {
            return;
        }
        self.bool_decided.push(SourceRange {
            offset: start,
            length: end - start,
        });
        let excess = self.bool_decided.len().saturating_sub(32);
        if excess > 0 {
            self.bool_decided.drain(..excess + 28);
        }
    }

    /// Whether the row's span sits wholly inside a statically-`bool`
    /// composite's extent.
    pub(crate) fn bool_decided_covers(&self, row: &Row) -> bool {
        self.bool_decided
            .iter()
            .any(|span| span.offset <= row.range.offset && row.range.end() <= span.end())
    }

    /// The condition operand completed before `bound`: the last value row
    /// wholly inside the span (the C-`for` condition sits between the
    /// separators, the assert condition between the paren and the
    /// message's comma — the parser delivers no condition-end face for
    /// either).
    pub(crate) fn check_condition_operand_within(&mut self, start: u32, end: u32) {
        let row = self
            .rows
            .iter()
            .rev()
            .find(|row| row.range.offset >= start && row.range.end() <= end);
        let Some(row) = row else {
            return;
        };
        let row = row.clone();
        self.classify_condition(&row);
    }

    /// The whitespace-only test between two offsets (the `&&`/`||` left
    /// -operand adjacency: `rows[-2]` is this operator's left operand only
    /// when nothing but whitespace separates its end from the operator —
    /// a compound left operand (`(a ?? b) && c`) leaves its last value row
    /// (`b`) short of the operator).
    pub(crate) fn ws_between(&self, start: u32, end: u32) -> bool {
        if start > end {
            return false;
        }
        let (start, end) = (self.byte_of(start), self.byte_of(end));
        if end > self.source.len() {
            return false;
        }
        self.source[start..end]
            .bytes()
            .all(|b| b.is_ascii_whitespace())
    }

    /// Sparse wide-char knots of [`Self::wide_chars`]: after each non-ASCII
    /// char, `(utf16 offset past it, cumulative byte - utf16 delta)`. Empty
    /// for an ASCII source.
    pub(crate) fn wide_knots(&self) -> &[(u32, u32)] {
        self.wide_chars.get_or_init(|| {
            if self.source.is_ascii() {
                return Vec::new();
            }
            let mut knots = Vec::new();
            let (mut utf16, mut byte) = (0u32, 0u32);
            for ch in self.source.chars() {
                utf16 += ch.len_utf16() as u32;
                byte += ch.len_utf8() as u32;
                if !ch.is_ascii() {
                    knots.push((utf16, byte - utf16));
                }
            }
            knots
        })
    }

    /// Byte offset of a UTF-16 offset (row ranges are UTF-16; `source`
    /// slices by byte). Identity on ASCII sources.
    pub(crate) fn byte_of(&self, utf16: u32) -> usize {
        let knots = self.wide_knots();
        let at = knots.partition_point(|&(end, _)| end <= utf16);
        let delta = if at == 0 { 0 } else { knots[at - 1].1 };
        (utf16 + delta) as usize
    }

    /// UTF-16 offset of a char-boundary byte offset (inverse of
    /// [`Self::byte_of`]).
    pub(crate) fn utf16_of(&self, byte: usize) -> u32 {
        let byte = byte as u32;
        let knots = self.wide_knots();
        let at = knots.partition_point(|&(end, delta)| end + delta <= byte);
        let delta = if at == 0 { 0 } else { knots[at - 1].1 };
        byte - delta
    }

    /// The source byte at a UTF-16 offset.
    pub(crate) fn byte_at(&self, utf16: u32) -> Option<u8> {
        self.source.as_bytes().get(self.byte_of(utf16)).copied()
    }

    /// The bare-identifier spelling at `range`, when the bytes before it
    /// don't make it a selector (`.`/`?.`/`..` left-guards) and the range
    /// itself is one plain Dart identifier (ASCII letter or `_` head,
    /// identifier characters after — the scanner's ASCII-only identifier
    /// contract). The unnamed-creation face's direct-transport head: a
    /// creation whose callee row resolved outside the container shape
    /// still names its class here.
    pub(crate) fn bare_creation_head_name(&self, range: SourceRange) -> Option<StringId> {
        let end = range.offset.checked_add(range.length)?;
        let (start, end) = (self.byte_of(range.offset), self.byte_of(end));
        if start > end || end > self.source.len() {
            return None;
        }
        if start > 0 {
            let bytes = self.source.as_bytes();
            if bytes[start - 1] == b'.' {
                return None;
            }
        }
        let text = &self.source[start..end];
        let mut chars = text.bytes();
        let head = chars.next()?;
        if !(head.is_ascii_alphabetic() || head == b'_') {
            return None;
        }
        if !chars.all(|b| b.is_ascii_alphanumeric() || b == b'_') {
            return None;
        }
        Some(self.intern(text))
    }

    /// The bare identifier immediately before an argument group's open
    /// paren (the invocation open's fallback head): strict adjacency, the
    /// selector left-guard and identifier shape come from
    /// [`Self::bare_creation_head_name`].
    pub(crate) fn bare_creation_head_before(&self, open: u32) -> Option<(StringId, SourceRange)> {
        let open_utf16 = open;
        let open = self.byte_of(open);
        if open == 0 || open > self.source.len() {
            return None;
        }
        let bytes = self.source.as_bytes();
        let mut start = open;
        while start > 0 && (bytes[start - 1].is_ascii_alphanumeric() || bytes[start - 1] == b'_') {
            start -= 1;
        }
        if start == open {
            return None;
        }
        // The head is ASCII, so its UTF-16 length is its byte length.
        let range = SourceRange {
            offset: self.utf16_of(start),
            length: open_utf16 - self.utf16_of(start),
        };
        let name = self.bare_creation_head_name(range)?;
        Some((name, range))
    }

    /// The unnamed-creation face over the open event's source-spelled head:
    /// the row transport can drop the callee row outright (the engine's
    /// creation posture for factory-only targets), so the close gate re-asks
    /// the same `constructor_invocation_resolver` question from the
    /// spelling — an in-scope class with no unnamed face reports.
    pub(crate) fn report_bare_creation_miss(&mut self) {
        let Some((name, range)) = self.pending_bare_creation.take() else {
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
        .is_some_and(|(decl, _)| !decl.has_unnamed_constructor)
        {
            let name_text = self.strings.resolve(name).to_string();
            self.emit(
                SemanticCode::NewWithUndefinedConstructorDefault,
                range,
                &name_text,
                "",
            );
        }
    }

    /// The trivia-only test between two offsets — whitespace plus `//…` and
    /// `/*…*/` comments. Comments are trivia upstream (the parser attaches
    /// them to the following token, `parser_impl.dart`'s comment skip): a
    /// callee abuts its argument group across them (`f /*c*/ (x)` still
    /// checks the argument), so the callee link must see through comments
    /// where [`Self::ws_between`] would break it.
    pub(crate) fn trivia_between(&self, start: u32, end: u32) -> bool {
        if start > end {
            return false;
        }
        let (start, end) = (self.byte_of(start), self.byte_of(end));
        if end > self.source.len() {
            return false;
        }
        let bytes = self.source.as_bytes();
        let mut at = start;
        let limit = end;
        while at < limit {
            let b = bytes[at];
            if b.is_ascii_whitespace() {
                at += 1;
                continue;
            }
            // `//` runs to the line end; `/*…*/` to the closer (a missing
            // closer — a recovery source — runs to `limit` and still counts
            // as trivia for the adjacency question).
            if b == b'/' && at + 1 < limit {
                match bytes[at + 1] {
                    b'/' => {
                        while at < limit && bytes[at] != b'\n' {
                            at += 1;
                        }
                    }
                    b'*' => {
                        at += 2;
                        let mut closed = false;
                        while at + 1 < limit {
                            if bytes[at] == b'*' && bytes[at + 1] == b'/' {
                                at += 2;
                                closed = true;
                                break;
                            }
                            at += 1;
                        }
                        if !closed {
                            at = limit;
                        }
                    }
                    _ => return false,
                }
                continue;
            }
            return false;
        }
        true
    }

    /// The first non-whitespace offset at or after `start` (the anchor of
    /// an operator's operand node: the oracle anchors the unary faces at
    /// the operand node, whose written start follows the operator's own
    /// whitespace).
    pub(crate) fn skip_whitespace(&self, start: u32) -> u32 {
        let bytes = self.source.as_bytes();
        let mut at = self.byte_of(start);
        while at < bytes.len() && bytes[at].is_ascii_whitespace() {
            at += 1;
        }
        // Whitespace is ASCII: the skipped run has equal UTF-16 width.
        start + (at - self.byte_of(start)) as u32
    }

    /// The bare lookup spelling of a site name (setters live under `name=`
    /// in the sites; the use index carries plain names).
    pub(crate) fn bare_name(&self, spelling: StringId) -> StringId {
        let text = self.strings.resolve(spelling);
        match text.strip_suffix('=') {
            Some(bare) => self.intern(bare),
            None => spelling,
        }
    }

    /// The `_`-private test (library privacy, the usage cluster's gate).
    pub(crate) fn is_private_name(&self, name: StringId) -> bool {
        self.strings.resolve(name).starts_with('_')
    }

    /// The `strict-raw-types` verdict for one group-less type-annotation
    /// head (`_checkForRawTypeName`, `type_arguments_verifier.dart:303-336`):
    /// the oracle reports when the resolved type is generic with no
    /// explicit-or-inferred type arguments and at least one argument
    /// instantiates to `dynamic` — an unbounded type parameter
    /// (`_isMissingTypeArguments` :711-735). The M1-decidable set is this
    /// file's containers with a captured unbounded parameter: cross-file
    /// containers carry no bound facts and bounded-only ones fill no
    /// `dynamic` — both suppress (under-report, never a false positive).
    /// The oracle's noisy-parent list is honored where M1 can see it: `is` /
    /// `as` type operands sit inside [`Engine::non_type_pos`], and pattern
    /// type positions under-report with the whole pattern face.
    pub(crate) fn check_raw_type(&mut self, name: StringId, range: SourceRange) {
        if !self.strict.raw_types
            || self.non_type_pos > 0
            || self.switch.pattern_depth > 0
            // An in-scope type parameter shadows any container of the same
            // name; a parameter is never a raw generic use (a header's
            // parked parameters included — its bound parses inside the
            // header).
            || self.type_params.contains(&name)
            || self.pending_type_params.contains(&name)
        {
            return;
        }
        let Some(&unbounded) = self.container_unbounded.get(&name) else {
            return;
        };
        if unbounded == 0 {
            return;
        }
        // The row defers: the oracle's `{0}` display is the
        // instantiated-to-bounds form (`Box<dynamic>`), which the whole-file
        // bounds snapshot renders at the finalize pass.
        self.raw_type_sites.push((name, range));
    }
}

/// `TypeSystem::flatten` (type_system.dart:425-471) at name level — the
/// await expression's static type is `flatten(operand)`
/// (`analyzeAwaitExpression`, type_analyzer.dart:542-548).
///
/// Mirrored decision order:
/// * `S?` strips the suffix, flattens, and re-applies it (:431-435) —
///   `Future<int>?` awaits to `int?`;
/// * a `Future<S>`/`FutureOr<S>` head unwraps one level to `S`
///   (:456-465) — the oracle returns `S` directly, never re-flattens it,
///   so `Future<Future<int>>` awaits to `Future<int>`;
/// * a bare `Future`/`FutureOr` spells `Future<dynamic>`/
///   `FutureOr<dynamic>` — the implicit argument awaits to `dynamic`;
/// * the type-parameter arms (:437-454) have no reach here: a parameter's
///   bound never interned into this `Ty` (name only), so the
///   promoted-bound face falls back to identity;
/// * every other type is its own flatten (:469-470) — `await` on a
///   non-Future operand is legal and yields the operand's type.
///
/// The subtype-driven `asInstanceOf` face (a user class `implements
/// Future<int>` awaiting to `int`) stays an under-report: the name level
/// carries no instantiated supertype walk.
pub(crate) fn flatten_ty(strings: &StringTable, args: &crate::model::ArgPool, ty: Ty) -> Ty {
    let Ty::Named {
        name,
        nullable,
        args: list,
    } = ty
    else {
        return ty;
    };
    if nullable {
        return match flatten_ty(
            strings,
            args,
            Ty::Named {
                name,
                nullable: false,
                args: list,
            },
        ) {
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
            // `Null`, `dynamic`, `Never`, `void`, `Unknown`: the suffix is
            // identity on these faces upstream.
            flat => flat,
        };
    }
    if name != strings.intern("Future") && name != strings.intern("FutureOr") {
        return ty;
    }
    if list == ArgListId::EMPTY {
        return Ty::Dynamic;
    }
    let flat = args.get(list);
    if flat.len() != 1 {
        return ty;
    }
    flat[0]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collector::collect;
    use crate::model::ArgPool;
    use dartq_language::{FeatureSet, LanguageVersion};
    use dartq_resolution::LibraryGraphBuilder;
    use dartq_scanner::ScanConfig;
    use dartq_source::FileTable;
    use std::path::Path;

    /// One pool + table pair the `flatten_ty` pins intern through.
    fn rig() -> (StringTable, ArgPool) {
        (StringTable::new(), ArgPool::default())
    }

    /// `flatten(Future<S>)` is `S` — one level, the oracle's direct
    /// `return S` (type_system.dart:461-465).
    #[test]
    fn flatten_unwraps_future_one_level() {
        let (strings, mut args) = rig();
        let int = Ty::named(strings.intern("int"));
        let inner = args.named(strings.intern("Future"), false, &[int]);
        let nested = args.named(strings.intern("Future"), false, &[inner]);
        assert_eq!(flatten_ty(&strings, &args, nested), inner);
        assert_eq!(flatten_ty(&strings, &args, inner), int);
    }

    /// `flatten(FutureOr<S>)` is `S` — the same head arm
    /// (`_futureTypeOfBounded`'s non-nullable `FutureOr` case,
    /// type_system.dart:1835).
    #[test]
    fn flatten_unwraps_future_or() {
        let (strings, mut args) = rig();
        let list = args.named(
            strings.intern("List"),
            false,
            &[Ty::named(strings.intern("int"))],
        );
        let fut_or = args.named(strings.intern("FutureOr"), false, &[list]);
        assert_eq!(flatten_ty(&strings, &args, fut_or), list);
    }

    /// `flatten(S?)` is `flatten(S)?` — the suffix strips, the recursion
    /// runs, the suffix re-applies (type_system.dart:430-435).
    #[test]
    fn flatten_preserves_nullable_suffix() {
        let (strings, mut args) = rig();
        let int = Ty::named(strings.intern("int"));
        let fut = args.named(strings.intern("Future"), true, &[int]);
        let want = Ty::Named {
            name: strings.intern("int"),
            nullable: true,
            args: ArgListId::EMPTY,
        };
        assert_eq!(flatten_ty(&strings, &args, fut), want);
        // A nullable non-Future face stays itself (`int?` → `int?`).
        assert_eq!(flatten_ty(&strings, &args, want), want);
    }

    /// A bare `Future`/`FutureOr` spells `Future<dynamic>`/
    /// `FutureOr<dynamic>` — the implicit argument awaits to `dynamic`.
    #[test]
    fn flatten_bare_future_is_dynamic() {
        let (strings, args) = rig();
        for head in ["Future", "FutureOr"] {
            let bare = Ty::named(strings.intern(head));
            assert_eq!(flatten_ty(&strings, &args, bare), Ty::Dynamic, "{head}");
        }
    }

    /// Every other type is its own flatten (type_system.dart:469-470):
    /// non-Future heads, `dynamic`, `Null`, `Unknown` pass through.
    #[test]
    fn flatten_non_future_is_identity() {
        let (strings, mut args) = rig();
        for ty in [
            Ty::named(strings.intern("int")),
            Ty::named(strings.intern("DB")),
            Ty::Dynamic,
            Ty::Null,
            Ty::Unknown,
            args.named(strings.intern("List"), false, &[Ty::Dynamic]),
        ] {
            assert_eq!(flatten_ty(&strings, &args, ty), ty, "{ty:?}");
        }
    }

    /// Diagnoses `source` as the sole file of a library with no imports —
    /// the same single-file rig `diagnose/tests.rs` runs (the await-fold
    /// pins need declared `Future`/`FutureOr` containers for the member
    /// walk's entity face).
    fn report(source: &str) -> FileReport {
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

    /// The corpus shape (evidence-2186 §A fixture 1): `var`-inferred local
    /// bound from `await f()` — the member accesses resolve on the
    /// flattened `DB`, not `Future<DB>`.
    #[test]
    fn await_inferred_local_resolves_flattened_members() {
        let report = report(
            "class Future<T> {}\n\
             class DB { void run() {} }\n\
             Future<DB> f() => throw 0;\n\
             void g() async {\n\
             \x20 final db = await f();\n\
             \x20 db.run();\n\
             }",
        );
        assert_eq!(
            code_names(&report),
            Vec::<&'static str>::new(),
            "{:?}",
            code_names(&report)
        );
    }

    /// `await` on a `Future<DB>?` operand flattens to `DB?` — the
    /// null-asserted member read resolves on the `DB` face.
    #[test]
    fn await_nullable_future_preserves_suffix() {
        let report = report(
            "class Future<T> {}\n\
             class DB { void run() {} }\n\
             Future<DB>? h() => throw 0;\n\
             void g() async {\n\
             \x20 final n = await h();\n\
             \x20 n!.run();\n\
             }",
        );
        assert_eq!(
            code_names(&report),
            Vec::<&'static str>::new(),
            "{:?}",
            code_names(&report)
        );
    }

    /// `await` on `FutureOr<DB>` flattens to `DB` — the member resolves
    /// (evidence-2186 §A fixture 3's `FutureOr` arm).
    #[test]
    fn await_future_or_operand_flattens() {
        let report = report(
            "class FutureOr<T> {}\n\
             class DB { void run() {} }\n\
             FutureOr<DB> h() => h();\n\
             void g() async {\n\
             \x20 final n = await h();\n\
             \x20 n.run();\n\
             }",
        );
        assert_eq!(
            code_names(&report),
            Vec::<&'static str>::new(),
            "{:?}",
            code_names(&report)
        );
    }

    /// True-positive pin (evidence-2186 §A fixture 4's under-report bias):
    /// no `await` — a `Future`-typed receiver's member genuinely misses
    /// and the report keeps firing. The flatten is scoped to the await
    /// close; a plain `Future` binding is untouched.
    #[test]
    fn unawaited_future_receiver_still_reports() {
        let report = report(
            "class Future<T> {}\n\
             class DB {}\n\
             void g(Future<DB> x) {\n\
             \x20 x.isEven;\n\
             }",
        );
        assert_eq!(
            code_names(&report),
            ["undefined_getter"],
            "{:?}",
            code_names(&report)
        );
        assert_eq!(
            report.diagnostics[0].message,
            "The getter 'isEven' isn't defined for the type 'Future'."
        );
    }

    /// A nullable-function-type alias expands carrying its RHS's trailing
    /// `?` (`typedef W = void Function()?`): `return null` answers
    /// assignable, no `return_of_invalid_type`. The direct-written form
    /// (`void Function()? g()` → `return null`) already stayed silent —
    /// the alias path expands through the captured RHS.
    #[test]
    fn nullable_function_alias_rhs_expands_nullable() {
        let report = report(
            "typedef W = void Function()?;\n\
             W create() {\n\
             \x20 return null;\n\
             }\n\
             void Function()? direct() {\n\
             \x20 return null;\n\
             }",
        );
        assert_eq!(
            code_names(&report),
            Vec::<&'static str>::new(),
            "{:?}",
            code_names(&report)
        );
    }

    /// A type-parameter operand's `!= null` never decides never-null (the
    /// bound's nullability is unmodeled at this layer; a nullable bound
    /// `T extends B?` answers false officially): no
    /// `unnecessary_null_comparison`.
    #[test]
    fn type_parameter_operand_suppresses_null_comparison() {
        let report = report(
            "class B {}\n\
             class A {\n\
             \x20 B? last;\n\
             \x20 T f<T extends B?>(T t) {\n\
             \x20\x20 if (t != null) {\n\
             \x20\x20\x20 last = t;\n\
             \x20\x20 }\n\
             \x20\x20 return t;\n\
             \x20 }\n\
             }",
        );
        assert_eq!(
            code_names(&report),
            Vec::<&'static str>::new(),
            "{:?}",
            code_names(&report)
        );
    }

    /// A `var`-inferred local bound from a typedef-typed formal's send
    /// reads the signature's return — the alias's `C?` face — never the
    /// formal's own declared type: `s != null` stays silent.
    #[test]
    fn typedef_call_inferred_local_keeps_nullable() {
        let report = report(
            "class C {}\n\
             typedef P = C? Function();\n\
             void g(P f) {\n\
             \x20 var s = f();\n\
             \x20 if (s != null) {}\n\
             }",
        );
        assert_eq!(
            code_names(&report),
            Vec::<&'static str>::new(),
            "{:?}",
            code_names(&report)
        );
    }

    /// A send on a function-typed formal with a nullable return carries
    /// the signature's `C?` face to the `var`-inferred local — `n != null`
    /// stays silent. Twin of the `typedef P` pin above; the bare
    /// `Function` (dart:core) spelling is unreachable in this no-core
    /// rig, so the inline `C? Function()` shape stands in for the
    /// "signature's own return type wins" face.
    #[test]
    fn function_param_send_keeps_nullable_return() {
        let report = report(
            "class C {}\n\
             void g(C? Function() find) {\n\
             \x20 var n = find();\n\
             \x20 if (n != null) {}\n\
             }",
        );
        assert_eq!(
            code_names(&report),
            Vec::<&'static str>::new(),
            "{:?}",
            code_names(&report)
        );
    }

    /// A `?.`-merged member read carries the null-aware `?` the merge
    /// could not join — `var a = c?.id` keeps `a` nullable even over a
    /// non-nullable field, so `a != null` stays silent.
    #[test]
    fn null_aware_member_read_stays_nullable() {
        let report = report(
            "class C { D id = D(); }\n\
             class D {}\n\
             void g(C? c) {\n\
             \x20 var a = c?.id;\n\
             \x20 if (a != null) {}\n\
             }",
        );
        assert_eq!(
            code_names(&report),
            Vec::<&'static str>::new(),
            "{:?}",
            code_names(&report)
        );
    }

    /// An interpolated-string left operand still folds `==` to `bool` —
    /// the `USER_OPERATORS` gap never marked it, so the verdict used to
    /// read the last interpolation child row instead of folding: the
    /// condition stays silent.
    #[test]
    fn interp_left_operand_comparison_folds_bool() {
        let report = report(
            "class C {}\n\
             void g(C x, C y) {\n\
             \x20 if ('a${x}b' == y) {}\n\
             }",
        );
        assert_eq!(
            code_names(&report),
            Vec::<&'static str>::new(),
            "{:?}",
            code_names(&report)
        );
    }

    /// `e is Sub` over the operand's own subtype family is a narrowing
    /// test, not a constant — no `unnecessary_type_check`.
    #[test]
    fn sibling_type_check_stays_silent() {
        let report = report(
            "class FSE {}\n\
             class Dir implements FSE {}\n\
             void g(FSE e) {\n\
             \x20 if (e is Dir) {}\n\
             }",
        );
        assert_eq!(
            code_names(&report),
            Vec::<&'static str>::new(),
            "{:?}",
            code_names(&report)
        );
    }

    /// A send on an implicit-`this` field whose declared type declares
    /// `call` runs the implicit dispatch: the inferred local binds the
    /// `call` method's return (`Out`), not the field's class, so the
    /// receiver's own member resolves. Upstream's `InstanceCall` target
    /// resolution (`resolver_visitor.dart`) types the send by `call`.
    #[test]
    fn callable_this_field_send_takes_call_return() {
        let report = report(
            "class Ctx {}\n\
             class Out { void run() {} }\n\
             class Expr { Out call(Ctx c) => throw 0; }\n\
             class Visitor {\n\
             \x20 Visitor(this.expr);\n\
             \x20 final Expr expr;\n\
             \x20 void visit(Ctx ctx) {\n\
             \x20\x20 final out = expr(ctx);\n\
             \x20\x20 out.run();\n\
             \x20 }\n\
             }",
        );
        assert_eq!(
            code_names(&report),
            Vec::<&'static str>::new(),
            "{:?}",
            code_names(&report)
        );
    }

    /// The same dispatch through an explicit receiver (`box.expr(ctx)`):
    /// the merged member row carries the field's declared `Expr` face and
    /// the send answers `call`'s return.
    #[test]
    fn callable_member_send_takes_call_return() {
        let report = report(
            "class Ctx {}\n\
             class Out { void run() {} }\n\
             class Expr { Out call(Ctx c) => throw 0; }\n\
             class Box { Box(this.expr); final Expr expr; }\n\
             class Visitor {\n\
             \x20 void visit(Ctx ctx, Box box) {\n\
             \x20\x20 final out = box.expr(ctx);\n\
             \x20\x20 out.run();\n\
             \x20 }\n\
             }",
        );
        assert_eq!(
            code_names(&report),
            Vec::<&'static str>::new(),
            "{:?}",
            code_names(&report)
        );
    }

    /// The value's declared type is the interface declaring `call` (the
    /// concrete implementation lives on the value's own class): the send
    /// answers the interface member's return — the interface closure is
    /// part of the member walk, and its `call` is not the concrete
    /// override's untyped identity.
    #[test]
    fn callable_interface_typed_send_takes_call_return() {
        let report = report(
            "class Ctx {}\n\
             class Out { void run() {} }\n\
             abstract class Callable { Out call(Ctx c); }\n\
             class Expr implements Callable {\n\
             \x20 @override Out call(Ctx c) => throw 0;\n\
             }\n\
             class Visitor {\n\
             \x20 Visitor(this.fn);\n\
             \x20 final Callable fn;\n\
             \x20 void visit(Ctx ctx) {\n\
             \x20\x20 final out = fn(ctx);\n\
             \x20\x20 out.run();\n\
             \x20 }\n\
             }",
        );
        assert_eq!(
            code_names(&report),
            Vec::<&'static str>::new(),
            "{:?}",
            code_names(&report)
        );
    }

    /// `call` inherited from a superclass of the value's static type: the
    /// member walk's superclass closure answers the send, so the inherited
    /// return types the local.
    #[test]
    fn callable_inherited_send_takes_call_return() {
        let report = report(
            "class Ctx {}\n\
             class Out { void run() {} }\n\
             class Base { Out call(Ctx c) => throw 0; }\n\
             class Expr extends Base {}\n\
             class Visitor {\n\
             \x20 Visitor(this.expr);\n\
             \x20 final Expr expr;\n\
             \x20 void visit(Ctx ctx) {\n\
             \x20\x20 final out = expr(ctx);\n\
             \x20\x20 out.run();\n\
             \x20 }\n\
             }",
        );
        assert_eq!(
            code_names(&report),
            Vec::<&'static str>::new(),
            "{:?}",
            code_names(&report)
        );
    }

    /// A non-async body under a non-nullable `FutureOr<void>` return type
    /// stays silent (the union's void base admits a normal completion),
    /// while the `FutureOr<K>` sibling is potentially non-nullable and
    /// reports the ERROR twin. The `FutureOr`/`K` spellings are locally
    /// declared — the no-core rig reaches dart:core through declarations
    /// only, and the engine's FutureOr identity keys on the name.
    #[test]
    fn future_or_void_body_stays_silent() {
        let report = report(
            "class FutureOr<T> {}\n\
             class K {}\n\
             class C {\n\
             \x20 FutureOr<void> write(K k) {\n\
             \x20\x20 k = K();\n\
             \x20 }\n\
             \x20 FutureOr<K> compute(K k) {\n\
             \x20\x20 k = K();\n\
             \x20 }\n\
             }",
        );
        assert_eq!(
            code_names(&report),
            ["body_might_complete_normally"],
            "{:?}",
            code_names(&report)
        );
        assert_eq!(report.diagnostics[0].range.offset > 0, true);
    }
}
