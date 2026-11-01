use super::*;

/// The statement-form operand face a teardown candidate reports through
/// (#2814): the message pair is verbatim from messages.yaml's
/// `uncheckedUseOfNullableValue` shared-name family (the code is one; the
/// problem/correction texts differ per report site). Named apart from
/// [`super::operand_face::OperandFace`] — the resolved-static-type face
/// the operator rows classify.
#[derive(Clone, Copy)]
enum UncheckedOperandFace {
    /// `uncheckedUseOfNullableValueAsIterator` — the for-in iterable.
    ForInIterator,
    /// `uncheckedUseOfNullableValueInSpread` — the spread operand.
    Spread,
    /// `uncheckedUseOfNullableValueInYieldEach` — the `yield*` operand.
    YieldEach,
}

impl UncheckedOperandFace {
    /// The (problem, correction) texts of the face.
    fn texts(self) -> (&'static str, &'static str) {
        match self {
            UncheckedOperandFace::ForInIterator => (
                "A nullable expression can't be used as an iterator in a for-in loop.",
                "Try checking that the value isn't 'null' before using it as an iterator.",
            ),
            UncheckedOperandFace::Spread => (
                "A nullable expression can't be used in a spread.",
                "Try checking that the value isn't 'null' before using it in a spread, or use a null-aware spread.",
            ),
            UncheckedOperandFace::YieldEach => (
                "A nullable expression can't be used in a yield-each statement.",
                "Try checking that the value isn't 'null' before using it in a yield-each statement.",
            ),
        }
    }
}

/// The arm-scope sentinel floor for [`LocalDecl::group`]: a group id at or
/// above this value is not a pattern-declaration window id but a
/// case-header binder's declaring switch depth (`u32::MAX - depth`), the
/// encoding [`Engine::add_tracked_local`] stamps so
/// [`Engine::resolve_local`] can retire the binding with its switch. The
/// verdict walk reads the same floor to give those binders their own
/// reporting rule. Kept in lockstep with `PATTERN_ARM_SCOPE_BASE`.
const ARM_SCOPE_SENTINEL_BASE: u32 = u32::MAX - u16::MAX as u32;

/// The non-`bool` face a condition check reports through — the
/// `locatableDiagnostic` parameter of upstream `checkForNonBoolExpression`
/// (bool_expression_verifier.dart:51-77): the statement/element
/// `non_bool_condition`, the assert face's `non_bool_expression`.
/// `&&`/`||` operands override either to `non_bool_operand` at classify
/// time — the face picks the code, the shared verdict picks whether it
/// fires.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum CondFace {
    Condition,
    Expression,
}

impl CondFace {
    fn code(self) -> SemanticCode {
        match self {
            CondFace::Condition => SemanticCode::NonBoolCondition,
            CondFace::Expression => SemanticCode::NonBoolExpression,
        }
    }
}

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
        augmentations_enabled: bool,
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
        let boundary = Boundary::new(index, file, lib, unit_args, graph, namespaces, strings);
        Self {
            graph,
            namespaces,
            index,
            strings,
            boundary,
            source,
            tokens,
            ctor_augment_pending: false,
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
            closure_literal_starts: FxHashSet::default(),
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
            alias_fold_memo: std::cell::RefCell::new(FxHashMap::default()),
            cast_nullable: Vec::new(),
            body_completions: Vec::new(),
            armed_ty: None,
            armed_ty_unresolved: false,
            armed_ty_span: None,
            type_args_group_end: None,
            written_ty_span: None,
            literal_row_open: None,
            fn_formal_closer_end: None,
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
            pending_pattern_type_args: None,
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
            ctor_pos_levels: Vec::new(),
            invalid_type_names: InvalidTypeNames::default(),
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
            pairing: PairingLedger::default(),
            default_value_depth: 0,
            formal_written: None,
            formal_written_saved: None,
            formal_list_depth: 0,
            fn_type_formal_lists: 0,
            this_formal_names: Vec::new(),
            switch_expr_marks: Vec::new(),
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
            group_augment: false,
            exec_bounds: FxHashMap::default(),
            method_exec_bounds: FxHashMap::default(),
            pending_exec_is_method: false,
            fn_var_sigs: FxHashMap::default(),
            fn_param_sigs: FxHashMap::default(),
            fn_formal_sigs: FxHashMap::default(),
            arg_sigs: FxHashMap::default(),
            closure_caps: Vec::new(),
            capture_formal_required: false,
            fn_formal_named_section: false,
            fn_type_capture: Vec::new(),
            fn_nested_unclaimed: false,
            fn_type_stash: None,
            fn_shape_required: FxHashMap::default(),
            last_fn_seal: None,
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
            ext_nullable_member_rows: FxHashSet::default(),
            ext_write_target_rows: FxHashSet::default(),
            pattern_shadows: Vec::new(),
            if_case_scopes: Vec::new(),
            pattern_shadow_binders: Vec::new(),
            if_case_binders: Vec::new(),
            switch_arm_scopes: Vec::new(),
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
            top_var_inferred_tys: FxHashMap::default(),
            field_inferred_tys: FxHashMap::default(),
            pending_model_declarators: Vec::new(),
            staged_write_tys: FxHashMap::default(),
            stmt_block_depth: 0,
            next_exec_serial: 0,
            enum_element_open: false,
            const_string_begins: Vec::new(),
            ctor_init_latch: None,
            ctor_init_pending: Vec::new(),
            ctor_inits: FxHashMap::default(),
            pending_matched: Vec::new(),
            pending_switch_coverings: Vec::new(),
            deferred_var_rebinds: Vec::new(),
            deferred_call_rebinds: Vec::new(),
            deferred_bare_rebinds: Vec::new(),
            deferred_invocation_faces: Vec::new(),
            promotion_function_reads: FxHashSet::default(),
            rebound_locals: FxHashMap::default(),
            clause_anchor: None,
            primary_constructors_enabled,
            augmentations_enabled,
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
            ffi_clause_spans: FxHashMap::default(),
            ffi_invocations: Vec::new(),
            ffi_address_reads: Vec::new(),
            ffi_pending_annos: Vec::new(),
            ffi_decl_annos: FxHashMap::default(),
            locals: Vec::new(),
            frame_local_marks: Vec::new(),
            local_hits: FxHashMap::default(),
            local_decl_at_read: FxHashMap::default(),
            shadowed_bare_rows: FxHashSet::default(),
            local_init_rows: FxHashMap::default(),
            init_derived_types: std::cell::RefCell::default(),
            invalid_init_locals: std::cell::RefCell::default(),
            local_decl_at_member: FxHashMap::default(),
            deferred_closure_formals: Vec::new(),
            pattern_group_open: None,
            pattern_group_seq: 0,
            pattern_bind_vars: Vec::new(),
            pattern_bind_records: 0,
            pattern_bind_dirty: false,
            last_pattern_variable: None,
            pending_pattern_null_check: None,
            last_case_object_head: None,
            null_check_binder_candidates: Vec::new(),
            pending_pattern_getters: Vec::new(),
            primary_formals: FxHashSet::default(),
            primary_formals_muted: true,
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
            record_short_fields: FxHashMap::default(),
            cascade_target_spans: Vec::new(),
            cascade_last: None,
            cascade_open: Vec::new(),
            cascade_target_tys: Vec::new(),
            cascade_value_rows: Vec::new(),
            cascade_last_ty: None,
            cascade_null_aware_op: None,
            used_names: FxHashSet::default(),
            import_used: FxHashSet::default(),
            extension_member_used: FxHashSet::default(),
            closure_return_counted: FxHashSet::default(),
            closure_return_valid: FxHashSet::default(),
            closure_invalid_returns: FxHashSet::default(),
            invalid_closure_spans: Vec::new(),
            invalid_typed_locals: FxHashSet::default(),
            invalid_typed_member_rows: FxHashSet::default(),
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
            catch_on_record: false,
            catch_on_miss_rows: Vec::new(),
            catch_on_nullable: None,
            catch_clause_start: None,
            catch_head_end: None,
            catch_params: 0,
            catch_trace_slot: false,
            catch_formal_group: None,
            catch_ord_exc: None,
            catch_ord_trace: None,
            try_clauses: Vec::new(),
            if_null_groups: Vec::new(),
            if_null_rhs_end: None,
            if_null_results: FxHashMap::default(),
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
            coalesce_staged: Vec::new(),
            dead_windows: Vec::new(),
            promoted_recv_reads: Vec::new(),
            inferred_formal_containers: FxHashSet::default(),
            inferred_recv_used_members: FxHashSet::default(),
            closure_formal_names: FxHashSet::default(),
            unresolved_member_rows: Vec::new(),
            extension_nullable_resolved: FxHashSet::default(),
            prop_promotion_rows: PropPromotionStaging::default(),
            member_reads: Vec::new(),
            member_read_ends: FxHashMap::default(),
            recv_memo: RefCell::new(FxHashMap::default()),
            recv_memo_active: false,
            cascade_first_spans: FxHashMap::default(),
            recv_walk_depth: Cell::new(0),
            member_recv_tys: FxHashMap::default(),
            index_result_recvs: FxHashMap::default(),
            index_arg_faces: FxHashMap::default(),
            call_ends: Vec::new(),
            solved_call_params: FxHashMap::default(),
            send_return_staging: None,
            send_invalid_staging: None,
            member_spellings: std::cell::OnceCell::new(),
            refined_call_returns: FxHashMap::default(),
            non_promotions: Vec::new(),
            pending_non_promotion: Vec::new(),
            string_row_marks: Vec::new(),
            interpolation_child_rows: FxHashSet::default(),
            read_types: Vec::new(),
            cast_rewrites: Vec::new(),
            record_recv_tys: Vec::new(),
            query_type_rows: Vec::new(),
            unnamed_extension_faces_opened: 0,
            initializer_locals: FxHashSet::default(),
            const_kw_spans: Vec::new(),
            const_creation_pending: None,
            local_fn_wildcard: Vec::new(),
            wildcard_dups_emitted: 0,
            site_params: FxHashMap::default(),
            self_scope_rows: FxHashSet::default(),
            field_read_rows: FxHashMap::default(),
            statement_discarded_rows: FxHashSet::default(),
            ctor_pending: None,
            ctor_body_open: false,
            ctor_frame_len: 0,
            open_toplevel_site: None,
            open_member_setter: false,
            open_member_accessor: false,
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
    /// extent, `dead_code_verifier.dart`'s `flowEnd` member trim). The
    /// leading edge snaps to the first token in the span — comments are
    /// trivia outside the token table (the parser attaches them to the
    /// following token), so a comment-led case body anchors at its first
    /// statement, never the `// …` after the head's `:` (#2861). Interior
    /// comments ride along — a documented divergence.
    #[expect(clippy::cast_possible_truncation)] // span arithmetic narrows under the scanner u32 offset cap
    pub(crate) fn trimmed_span(&self, start: u32, end_excl: u32) -> Option<SourceRange> {
        // The first token at/after `start` opens the span when it lands
        // inside it; a token-free span keeps the byte walk below (and dies
        // on all-trivia). In code the first non-whitespace byte before the
        // next token is that token, so comment-free spans are unchanged —
        // only a leading comment (invisible to the token table) gets
        // skipped.
        let toks = self.tokens.as_slice();
        let head = match toks.get(toks.partition_point(|t| t.utf16_offset < start)) {
            Some(t) if t.utf16_offset < end_excl => t.utf16_offset,
            _ => start,
        };
        // Both bounds arrive as UTF-16 offsets (row/token spans) while the
        // trim walk reads source bytes: convert at the boundary — a wide
        // char anywhere before the span would otherwise desync the slice (or
        // cut mid-character, #2594) — and convert the trimmed bounds back so
        // the returned span keeps the external UTF-16 convention.
        let start = self.byte_of(head);
        let end = self.byte_of(end_excl).min(self.source.len());
        if start >= end {
            return None;
        }
        let text = &self.source[start..end];
        let trimmed = text.trim_end();
        if trimmed.is_empty() {
            return None;
        }
        // The span opens at the snapped token head — every leading
        // whitespace and comment byte is behind it — and trims only the
        // trailing run: both-ends trimmed.
        let end16 = self.utf16_of(start + trimmed.len());
        Some(SourceRange {
            offset: head,
            length: end16.saturating_sub(head),
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

    /// Whether `range`'s head sits inside one of the flow layer's dead
    /// intervals (`FlowFact::DeadCode` ranges re-homed into
    /// [`Engine::dead_windows`]). The oracle's `ErrorVerifier` visits a
    /// dead node but the flow marks it dead and the non-`dead_code`
    /// verification never fires — the diagnostic faces that consult this
    /// read the oracle's dead-node short-circuit. Containment keys on the
    /// row's own head: a dead window covers the nodes inside it, and a
    /// row whose head lands before the window's own anchor is the
    /// enclosing live statement, never the dead content.
    pub(crate) fn in_dead_window(&self, range: SourceRange) -> bool {
        self.dead_windows
            .iter()
            .any(|w| range.offset >= w.offset && range.offset < w.end())
    }

    /// Whether `ty` spells the `void` type. The name-level model carries
    /// two faces for it: a written annotation folds onto [`Ty::Void`],
    /// while a LITERAL's written type argument keeps the `void` name
    /// (`literal_arg_leaf` special-cases only `dynamic`/`Null`, so
    /// `<void>[]` types as `List` over a `Named{void}` element).
    pub(crate) fn is_void_spelling(&self, ty: Ty) -> bool {
        match ty {
            Ty::Void => true,
            Ty::Named {
                name,
                nullable: false,
                ..
            } => self.strings.resolve(name) == "void",
            _ => false,
        }
    }

    /// `use_of_void_result` on one operand row (upstream
    /// `checkForUseOfVoidResult`, error_detection_helpers.dart:245-258):
    /// the operand's `staticType` is `VoidTypeImpl` and its value is
    /// consumed by the enclosing construct. The anchor is the operand's
    /// use-site span ([`Self::use_site_range`]), never always the row's
    /// own token — `at(methodName)` keeps a declared-method send's name,
    /// while every other shape reports the operand's whole expression.
    /// A non-`void` operand is a no-op.
    pub(crate) fn emit_use_of_void_result(&mut self, ty: Ty, operand: &Row) {
        if ty != Ty::Void {
            return;
        }
        // One row per consumed node. The same operand is folded from more
        // than one seam — the `&&`/`||` condition faces beside
        // `fold_binary`'s own operand check, and `??`'s `end_if_null`
        // beside that same fold (scopes_bodies.rs) — and the oracle answers
        // ONE `use_of_void_result` per operand: the resolver's
        // `_checkNonBoolOperand` (binary_expression_resolver.dart:294-295 →
        // bool_expression_verifier.dart:57) and the verifier's
        // `visitBinaryExpression` (error_verifier.dart:470/:482) both land
        // on the operand span without doubling the row. Identity is the
        // span: the row the consuming seam reads IS the operand node.
        let range = self.use_site_range(operand);
        if self.report.diagnostics.iter().any(|d| {
            (d.range == range || d.range == operand.range)
                && d.code.name == "use_of_void_result"
        }) {
            return;
        }
        self.emit(SemanticCode::UseOfVoidResult, range, "", "");
    }

    /// The operand's use-site span for `use_of_void_result` (upstream
    /// `checkForUseOfVoidResult`'s anchor split,
    /// error_detection_helpers.dart:245-258): a `MethodInvocation` operand
    /// anchors `methodName` — a declared-method send's row already spans
    /// exactly that name token — while every other operand shape anchors
    /// `.at(expression)`. Two widenings fall out of the expression arm:
    ///
    /// - a send through a function-typed VALUE (`FunctionExpressionInvocation`
    ///   — a `void Function()` formal or local, or a function-typed member,
    ///   never a declared method head) spans receiver-start through the
    ///   closing `)` (`invocation_span`);
    /// - a member-shaped operand (`x.f`, `C.m`, `ET1("42").id`) spans the
    ///   receiver's start through the selector's end
    ///   (`property_access_span`).
    ///
    /// Declared-method sends, bare reads, and every other operand keep the
    /// row's own span. A declared-method send must NOT take the
    /// property-access widening: `x.m()` anchors `m`, never `x.m`.
    pub(crate) fn use_site_range(&self, operand: &Row) -> SourceRange {
        if matches!(operand.intent, Intent::Invoke) {
            self.value_send_span(operand).unwrap_or(operand.range)
        } else {
            self.property_access_span(operand.range)
                .unwrap_or(operand.range)
        }
    }

    /// The `name: operand` span when the two tokens directly before
    /// `range`'s head are a `:` and a label identifier — the
    /// `NamedExpression`/`RecordLiteralNamedField` wrapper the oracle
    /// anchors (`at(expression)` on the entry, never the value alone).
    /// `None` without the label. Only callers that KNOW the operand sits
    /// in a named-entry position (a named argument's `name`, a record
    /// literal's field label) may apply it — a bare token probe would
    /// misread a ternary's `:` or a `case` colon as a label.
    pub(crate) fn label_span(&self, range: SourceRange) -> Option<SourceRange> {
        let tokens = self.tokens.as_slice();
        let colon = tokens
            .partition_point(|t| t.utf16_offset < range.offset)
            .checked_sub(1)?;
        if !matches!(tokens.get(colon)?.kind, dartq_scanner::TokenKind::Colon) {
            return None;
        }
        let name = tokens.get(colon.checked_sub(1)?)?;
        Some(SourceRange {
            offset: name.utf16_offset,
            length: range.end() - name.utf16_offset,
        })
    }

    /// The whole-send span of a `FunctionExpressionInvocation` operand —
    /// `None` for a `MethodInvocation` operand (a send resolving to a
    /// declared method or function head, which anchors the callee's name
    /// token instead) and for non-send operands. The value-callee shapes:
    /// a bare send on a `void Function()`-typed formal or local, a local
    /// without a declared-function capture (`fn_var_sigs` misses plain
    /// `void`-typed variables — their send is a function-expression
    /// invocation all the same), or a member send whose resolved member is
    /// a field or getter (a function-typed property — `Method` members
    /// keep the `methodName` anchor).
    fn value_send_span(&self, operand: &Row) -> Option<SourceRange> {
        if !matches!(operand.intent, Intent::Invoke) {
            return None;
        }
        let value_callee = match &operand.shape {
            RowShape::Member { recv } => {
                let type_name = match recv {
                    Receiver::Typed { type_name, .. }
                    | Receiver::This {
                        container: type_name,
                        ..
                    }
                    | Receiver::Super {
                        container: type_name,
                        ..
                    } => *type_name,
                    _ => return None,
                };
                matches!(
                    self.walk_members(type_name, operand.name),
                    Walk::Found(entry)
                        if matches!(entry.kind, MemberKind::Field | MemberKind::Getter)
                )
            }
            _ => {
                self.fn_var_sigs.contains_key(&operand.name)
                    || (self
                        .frames
                        .iter()
                        .any(|f| f.locals.contains(&operand.name))
                        && self.local_fn_sig(operand.name).is_none())
            }
        };
        if !value_callee {
            return None;
        }
        let range = self.invocation_span(operand.range).unwrap_or(operand.range);
        // A chained send (`f()()` — the outer `()` invokes the inner
        // send's result) keeps extending through each directly-adjacent
        // argument list: `call_ends` records only the first send's close
        // for a shared callee offset, so the tail groups ride the same
        // paren walk `row_ifnull_operator_at` runs (all UTF-16 offsets).
        let mut end = range.end();
        while self.byte_at(end) == Some(b'(') {
            let Some(close) = self.matching_paren(end) else {
                break;
            };
            end = close + 1;
        }
        Some(SourceRange {
            offset: range.offset,
            length: end - range.offset,
        })
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

    /// The unchecked-nullable row's per-site rendering, dispatched by the
    /// access shape the caller resolves (`UncheckedNullableKind`): the
    /// function text for a bare nullable function expression invoked, the
    /// method text for a member invocation (the index and compound/unary
    /// operator shapes included, the oracle's `[]`/`unary-` spellings), the
    /// operator text for a binary-operator application, the property text
    /// for every other access — with the matching correction
    /// (type_property_resolver.dart:128-148, messages.yaml's four
    /// shared-name variants). A range inside the flow's dead region
    /// suppresses: the oracle's `ErrorVerifier` visits the dead node but
    /// the non-`dead_code` verification never fires there — only the
    /// `dead_code` row stands (the upstream dead-node short-circuit).
    pub(crate) fn emit_unchecked_nullable(
        &mut self,
        range: SourceRange,
        name: &str,
        kind: UncheckedNullableKind,
    ) {
        if self.in_dead_window(range) {
            return;
        }
        let code = SemanticCode::UncheckedUseOfNullableValue;
        self.report.diagnostics.push(Diagnostic::new(
            code.severity(),
            code.code_info(),
            SemanticCode::unchecked_nullable_message(kind, name),
            self.file,
            range,
            Some(SemanticCode::unchecked_nullable_correction(kind).to_string()),
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
        // The statement-form operand faces (#2814) join the same
        // offset-sorted survivor tail.
        self.stage_operand_faces(&mut survivors);
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
                // A member read that resolved through a nullable-applicable
                // extension face (`extension on DartType?`'s getter over an
                // `InterfaceType?` receiver — ffi_verifier.dart:2644's
                // `element.supertype.isAbiSpecificInteger`) carries the
                // extension's own non-nullable result: the staged
                // null-shorting face stands down, exactly like the walk's
                // safe-`Single` verdict already did for the member row.
                if member_read
                    && self
                        .extension_nullable_resolved
                        .contains(&check.range.offset)
                {
                    continue;
                }
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
                    // names a non-`bool` — the face's own code reports at
                    // the operand's own span (`non_bool_operand` for a
                    // `&&`/`||` operand, `non_bool_expression` inside an
                    /// `assert`, `non_bool_condition` elsewhere).
                    let (code, arg) = self.condition_face_at(check.range);
                    survivors.push(Diagnostic::new(
                        code.severity(),
                        code.code_info(),
                        code.message(arg, ""),
                        self.file,
                        check.range,
                        None,
                    ));
                }
                continue;
            }
            if let StagedKind::YieldEach = check.kind {
                // The yield-each verdict reads the source's flow-aware
                // static type (upstream Dart SDK source,
                // pkg/analyzer/lib/src/dart/resolver/
                // yield_statement_resolver.dart `_checkForYieldOfInvalidType`
                // — `expression.typeOrThrow`, the RAW expression type
                // against the declared return, never a sequence wrap): a
                // null-guard promotion on the read substitutes the promoted
                // type itself and the verdict re-decides, while the no-fact
                // face re-decides on the parse-time raw type
                // (byte-identical to the parse-time emit — a promotion can
                // only clean the verdict, never create it, so the arm only
                // stages where the parse-time face answered negative).
                let implied = match promotions.promoted(check.range) {
                    Some(Ty::Unknown) => Self::non_null_form(check.from),
                    Some(promoted) => promoted,
                    None => check.from,
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
            if let StagedKind::ForIn = check.kind {
                // The parse-time rejection stands unless a promotion on
                // the iterable read re-decides it: a promoted type the
                // sequence accepts (or one the matrix cannot decide)
                // cleans the row (the under-report direction).
                let from = match promotions.promoted(check.range) {
                    None => check.from,
                    Some(Ty::Unknown) => continue,
                    Some(promoted) => {
                        if self.assignable(promoted, check.to, check.literal) != Some(false) {
                            continue;
                        }
                        promoted
                    }
                };
                let sequence = match check.to {
                    Ty::Named { name, .. } => self.strings.resolve(name).to_string(),
                    _ => continue,
                };
                let a0 = self.ty_display(from);
                let code = SemanticCode::ForInOfInvalidType;
                survivors.push(Diagnostic::new(
                    code.severity(),
                    code.code_info(),
                    code.message(&a0, &sequence),
                    self.file,
                    check.range,
                    None,
                ));
                continue;
            }
            let staged_promotion = promotions.promoted(check.range);
            // The invalid-type vacuous arm (subtype.dart's 0c case): either
            // side's head resolving nowhere makes every assignability ask
            // vacuous — the flow's unresolved-tested is-test promotes the
            // operand TO the tested spelling (`uriSource is
            // InSummarySource` over the failed package:front_end import
            // promotes to `InSummarySource`, and the oracle's argument row
            // stays silent), and a check against an unresolved formal head
            // is the same non-question.
            let invalid_face = matches!(staged_promotion, Some(ty) if self.is_invalid_ty(ty));
            if invalid_face {
                continue;
            }
            // An `as` expression's static type is its resolved target — a
            // staged check anchored inside a cast's whole span reads the
            // rewritten face outright (`g(v?.element as El)` offers `El`,
            // never the operand's nullable parse-time capture); the cast
            // outranks every promotion the operand might carry.
            let cast_ty = self
                .cast_rewrites
                .iter()
                .find(|(span, _)| {
                    span.offset <= check.range.offset && check.range.end() <= span.end()
                })
                .map(|(_, ty)| *ty);
            let mut from = if let Some(ty) = cast_ty {
                ty
            } else {
                match staged_promotion {
                    // The opaque-head promotion (#2174 — a signature-less
                    // function-typed declared type, or an unknown-written
                    // slot): the flow lattice carried only the nullability
                    // decision, so the re-decision substitutes the parse-time
                    // declared type with its `?` stripped — the real promoted
                    // type that decision produces.
                    Some(Ty::Unknown) => Self::non_null_form(check.from),
                    staged => staged
                        .or_else(|| {
                            self.promotable_field_substitution(props, check.range, check.from)
                        })
                        .unwrap_or(check.from),
                }
            };
            // A staged invocation operand reads the call's RESULT: a callee
            // whose function face became non-null only through the
            // substitution above (`if (_create != null) return _create();`
            // over a private final `T Function()?` field) still spells the
            // function type here, so the send's value is the shape's return.
            if self.invocation_span(check.range).is_some()
                && let Ty::Function {
                    shape,
                    nullable: false,
                } = self.fold_alias_ty(from)
            {
                from = self.boundary.pool().function_shape(shape).return_ty;
            }
            // The promotion-latched initializer's re-bind (#2539's argument
            // face): an argument read whose local's initializer member row
            // re-derived under a promoted receiver (`var e = t.element` past
            // `t is Sub` — the base getter's `Element?` at parse time, the
            // promoted container's own declaration after the pass-2
            // re-decision) carries the corrected face only in the re-bind
            // map, so the parse-time capture spelled the base face and the
            // standing rejection is a false positive. The flush reads the
            // same corrected static type the operator verdict
            // (row_resolution.rs's `Intent::Operator` arm) and every
            // receiver face read — `rebound_read_face` outranks the parse
            // bind; a standing promotion, a promotable-field substitution,
            // or a cast at the read outranks the re-bind (decided above).
            // The re-bind is narrowing-gated at insert, so this can only
            // clean a staged rejection, never create one.
            if matches!(check.kind, StagedKind::Argument | StagedKind::Assignment)
                && cast_ty.is_none()
                && staged_promotion.is_none()
                && let Some(face) = self
                    .rebound_read_face(check.range)
                    .filter(|ty| *ty != Ty::Unknown)
            {
                from = face;
            }
            // A record-literal operand keeps its per-ELEMENT promotions: the
            // whole-record span stages no promotion of its own (the record is
            // not a variable read), but each element's read inside the span
            // does (`featureSet != null && languageVersion != null` then
            // `return (featureSet, languageVersion)` — file_state.dart's two
            // return rows). Rebuild the shape by substituting, in source
            // order, each element whose read inside the range carries a
            // promotion that is exactly the element's non-null form — the
            // null-guard/bang face, never a shape change.
            if cast_ty.is_none()
                && staged_promotion.is_none()
                && let Ty::Record { shape, nullable } = from
            {
                let (positional, named) = {
                    let pool = self.boundary.pool();
                    let data = pool.record_shape(shape);
                    (
                        data.positional.to_vec(),
                        data.named.iter().map(|&(n, t)| (n, t)).collect::<Vec<_>>(),
                    )
                };
                let element_count = positional.len() + named.len();
                let mut inside: Vec<Ty> = Vec::with_capacity(element_count);
                // The element window. The staged span's own -64 pre-window
                // answers the short literal (`return (a, b);`'s elements),
                // but a record-return check anchors at a degenerate tail
                // byte PAST the closing `)` (the merged row's 1-byte
                // range), so a long literal's first elements sit far
                // before it. Recover the record's own extent — walk back
                // over the statement close and paren-match the `)` — and
                // take the wider of the two bounds; the exact-form and
                // subtype gates below still refuse anything the window
                // lets in that no element owns.
                let window_start = check.range.offset.saturating_sub(64);
                let elem_start = {
                    let mut at = check.range.offset;
                    let floor = check.range.offset.saturating_sub(2_048);
                    let ws = |b: u8| matches!(b, b' ' | b'\t' | b'\n' | b'\r');
                    // The record's closer: the anchor byte itself (the
                    // merged row's 1-byte range sits ON the `)`), or the
                    // `)` behind it past the statement's `;`.
                    let close = if self.byte_at(at) == Some(b')') {
                        Some(at)
                    } else {
                        while at > floor && self.byte_at(at - 1).is_some_and(ws) {
                            at -= 1;
                        }
                        if at > floor && self.byte_at(at - 1) == Some(b';') {
                            at -= 1;
                            while at > floor && self.byte_at(at - 1).is_some_and(ws) {
                                at -= 1;
                            }
                        }
                        (at > floor && self.byte_at(at - 1) == Some(b')')).then_some(at - 1)
                    };
                    match close {
                        Some(close) => {
                            let mut depth = 0u32;
                            let mut p = close + 1;
                            while p > floor {
                                match self.byte_at(p - 1) {
                                    Some(b')') => depth += 1,
                                    Some(b'(') => {
                                        depth = depth.saturating_sub(1);
                                        if depth == 0 {
                                            p -= 1;
                                            break;
                                        }
                                    }
                                    _ => {}
                                }
                                p -= 1;
                            }
                            if depth == 0 { p } else { window_start }
                        }
                        None => window_start,
                    }
                };
                let elem_start = elem_start.min(window_start);
                for (range, ty) in &promotions.rows {
                    if range.offset >= elem_start
                        && range.end() <= check.range.end()
                        && *ty != Ty::Unknown
                    {
                        inside.push(*ty);
                    }
                }
                // The flow read types (#1056's rows) in the window: an
                // element's bare read carries its flow-aware static face —
                // a write promotion the `promotions` staging cannot spell
                // (`x ??= SubImpl` over `x: T?` types later reads non-null
                // through the write, never a `PromotedRead` fact when the
                // flow's declared face already matched).
                let scan_start = self
                    .read_types
                    .partition_point(|(row, _)| row.offset < elem_start);
                for (range, ty) in &self.read_types[scan_start..] {
                    if range.offset > check.range.end() {
                        break;
                    }
                    if range.end() <= check.range.end() && *ty != Ty::Unknown {
                        inside.push(*ty);
                    }
                }
                // Pair by exact non-null form, not by position: a literal
                // element (`(cachedStmt, true)`'s `true`) emits no
                // promotion row, so a strict count match would refuse the
                // rebuild; each nullable element whose non-null form the
                // window saw takes it (distinct-element records — the
                // oracle's own per-element reads). A row may instead carry
                // a strict SUBTYPE head of the element (`x ??= SubImpl`
                // over `x: T?` promotes to `SubImpl`): `LUB(elem,
                // written)` is exactly the element's non-null form
                // whenever the written type is assignable to it, so the
                // substitution holds on that proof alone.
                if !inside.is_empty() {
                    let mut positional = positional;
                    let mut named = named;
                    let mut changed = false;
                    let covers = |seen: &[Ty], promoted: Ty| {
                        seen.contains(&promoted)
                            || seen.iter().any(|ty| {
                                *ty != promoted
                                    && self.assignable(*ty, promoted, false) == Some(true)
                            })
                    };
                    for element in positional.iter_mut() {
                        if let Ty::Named { nullable: true, .. } = element {
                            let promoted = Self::non_null_form(*element);
                            if &promoted != element && covers(&inside, promoted) {
                                *element = promoted;
                                changed = true;
                            }
                        }
                    }
                    for (_, element) in named.iter_mut() {
                        if let Ty::Named { nullable: true, .. } = element {
                            let promoted = Self::non_null_form(*element);
                            if &promoted != element && covers(&inside, promoted) {
                                *element = promoted;
                                changed = true;
                            }
                        }
                    }
                    if changed {
                        from =
                            self.boundary
                                .args
                                .borrow_mut()
                                .record(&positional, &named, nullable);
                    }
                }
            }
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
            // The invalid-poison face (A6): a staged argument whose
            // operand — or formal — carries a written head this scope
            // resolves to no type is the oracle's `InvalidType` operand
            // (`Source` under the unresolvable `_fe_analyzer_shared`
            // export chain): the argument check is vacuous and stays
            // silent, whatever the name-level pair spells. The compiler-
            // known core heads are exempt — they are never invalid in a
            // mounted run, and a synthetic unit test (no SDK) still
            // decides real pairs over them.
            let invalid_poison = |engine: &Self, ty: Ty| match ty {
                Ty::Named { name, .. } => !engine.core_head(name) && engine.written_ty_invalid(ty),
                _ => false,
            };
            if matches!(check.kind, StagedKind::Argument)
                && (invalid_poison(self, from) || invalid_poison(self, check.to))
            {
                continue;
            }
            let verdict = match check.kind {
                StagedKind::Return { ctor, is_async, .. } => {
                    // The expression body's `void` exemption (upstream Dart
                    // SDK source,
                    // pkg/analyzer/lib/src/error/return_type_verifier.dart
                    // `verifyExpressionFunctionBody`:57-64): an
                    // expression-bodied executable checks NOTHING when the
                    // FLATTENED declared return type is `void` — `void f()
                    // => e;` and `Future<void> f() async => e;` (likewise
                    // `FutureOr<void>`, `Future<void>?`) all run the body
                    // expression for its value, never its type, so the
                    // block-body matrix (`T_v` void accepts only
                    // `void`/`dynamic`/`Null`) never runs. The sync arm is
                    // already skipped where the arrow body closes
                    // (`handle_expression_function_body` reads the raw
                    // return type); the async arm needs the flatten, which
                    // only the teardown can ask — `is_async` rides the
                    // staged row and `check.to` is the declared face.
                    let target = self.fold_alias_ty(check.to);
                    let flattened = if is_async {
                        self.future_value_type(target)
                    } else {
                        target
                    };
                    if flattened == Ty::Void && self.arrow_body_expression(check.range) {
                        continue;
                    }
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
                // The assignment face closes the nominal blind spot (the
                // declared-subtype negative); the yield face keeps the
                // matrix posture.
                StagedKind::Assignment => self.assignment_verdict(from, check.to, check.literal),
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
                StagedKind::YieldEach { .. } | StagedKind::ForIn => continue,
                StagedKind::Assignment => (SemanticCode::InvalidAssignment, None),
                StagedKind::Argument => (SemanticCode::ArgumentTypeNotAssignable, None),
                StagedKind::Yield => (SemanticCode::YieldOfInvalidType, None),
                StagedKind::Return {
                    host,
                    name,
                    display,
                    ..
                } => (
                    SemanticCode::ReturnOfInvalidType(host),
                    Some(display.unwrap_or(name)),
                ),
            };
            // The oracle anchors the assignability face at the whole
            // right-hand expression — a call operand reports over the
            // invocation span (`f()`'s callee through `)`, or `c.m()`'s
            // receiver through `)`), a `receiver.member` operand over the
            // property-access span (receiver start through the member's
            // end), and every prefix/suffix wrapper (`await`, `new`,
            // `const`, unary heads, parens, `is`/`as`, postfix `!`, literal
            // brackets) rides the same `.at(expression)` extent. A bare
            // operand's row already is its own span.
            //
            // The return face asks the identical anchor (upstream Dart SDK
            // source, return_type_verifier.dart `reportTypeError`'s
            // `.at(expression)`), and its row is just as often a head token
            // — a callee name (`return f();` staged on `f`), an untyped
            // collection literal's close bracket (`return [1, 2];` staged on
            // `]`), a conditional's `?` operand. Leaving it unwidened
            // reported the row one token wide against the oracle's whole
            // expression, so `Return` rides the same recovery. The closure
            // literal stays out of it: a returned function literal is its
            // own expression, and its braces are the row's own span.
            let range = match check.kind {
                StagedKind::Assignment | StagedKind::Argument | StagedKind::Return { .. }
                    if !self.closure_literal_starts.contains(&check.range.offset) =>
                {
                    self.staged_expression_span(check.range)
                }
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

    /// Whether a staged return expression is an EXPRESSION body — `f() =>
    /// e` / `f() async => e` — rather than a `return e;` statement. The two
    /// forms are the same staged kind (both call
    /// [`Engine::check_return`]) but the oracle's `void` exemption hangs on
    /// the distinction (upstream Dart SDK source,
    /// pkg/analyzer/lib/src/error/return_type_verifier.dart
    /// `verifyExpressionFunctionBody`:57-64 — only the expression body
    /// early-returns on a `void` flattened return; `verifyReturnStatement`
    /// keeps checking).
    ///
    /// The byte probe reads the arrow: the expression-body listener hands
    /// the body expression's own span, which always sits immediately after
    /// `=>` (an `await`-widened span keeps the arrow ahead of the keyword),
    /// while a return statement's span follows the `return` keyword, so no
    /// staged face lands on both. An `await`-rewritten range still starts
    /// at a token that ends in `=>`.
    fn arrow_body_expression(&self, range: SourceRange) -> bool {
        let bytes = self.source.as_bytes();
        let mut cursor = self.byte_of(range.offset);
        while cursor > 0 && bytes[cursor - 1].is_ascii_whitespace() {
            cursor -= 1;
        }
        cursor >= 2 && &bytes[cursor - 2..cursor] == b"=>"
    }

    /// The statement-form operand faces of `unchecked_use_of_nullable_value`
    /// (#2814): the for-in iterable (`uncheckedUseOfNullableValueAsIterator`
    /// — `for_resolver.dart:124/:208`, the verifier ask `.at(iterable)`
    /// after the iterable expression resolves) and the spread operand
    /// (`uncheckedUseOfNullableValueInSpread` — `type_analyzer.dart`
    /// `visitSpreadElement:4479-4484`, gated on `!node.isNullAware`).
    ///
    /// The parse-time pass has no window for these faces: the for-in
    /// iterable window closes inside `check_for_in_iterable`, whose
    /// nullable arm defers to "the nullable-dereference face" without
    /// staging one, and a spread's operand never arms a window at all. So
    /// the candidates re-derive here at the teardown join: a token-table
    /// walk recovers the windows the parser saw, and the operand's flow
    /// type comes from the `FlowFact::ReadType` rows the #2798 lane
    /// pinned ([`Engine::read_types`] — the flow-aware static type at
    /// every local/parameter read, promoted or declared; the pass-2
    /// pre-pass has emptied the parse-time row stack by this point). A
    /// window holding exactly one read whose span reaches the window's
    /// last token decides — a compound iterable's window (an if-null
    /// pair, a member chain's receiver read) attributes to no single
    /// read and stays under-reported, the same discipline the parse-time
    /// staging holds. A promotion on the read
    /// (`if (l != null) for (var e in l)`) flows through the same row's
    /// non-nullable type and stays silent.
    fn stage_operand_faces(&self, out: &mut Vec<Diagnostic>) {
        // Only files carrying a potentially-nullable operand pay the token
        // walk (a nullable operand is a necessary condition of every
        // candidate — the flow's read rows are the post-teardown operand
        // face: a `void`/`Null` operand drops at the `ReadType` staging
        // boundary and its pass-1 row is already consumed, so the
        // declared-type fallback lives in the assignability pass that owns
        // `use_of_void_result`, outside this teardown join).
        if !self
            .read_types
            .iter()
            .any(|(_, ty)| self.operand_face_ty_nullable(*ty))
        {
            return;
        }
        let tokens = self.tokens.as_slice();
        for i in 0..tokens.len() {
            match tokens[i].kind {
                dartq_scanner::TokenKind::Keyword(dartq_scanner::Keyword::For) => {
                    let Some((start, end, close_idx, spread_inside)) =
                        self.for_in_iterable_window(i)
                    else {
                        continue;
                    };
                    // A spread operator inside the iterable window means
                    // the iterable is (or embeds) a collection literal —
                    // its own type, not the element read's, is the
                    // iterable's static type; the spread face owns the
                    // element read's row.
                    if spread_inside {
                        continue;
                    }
                    let Some((ty, range)) =
                        self.single_reachable_read_between(start, end, close_idx)
                    else {
                        continue;
                    };
                    self.emit_operand_face_row(UncheckedOperandFace::ForInIterator, ty, range, out);
                }
                // The null-aware spread (`...?`) is `!node.isNullAware`'s
                // true arm — the oracle stays silent there.
                dartq_scanner::TokenKind::PeriodPeriodPeriod => {
                    let Some((start, end, boundary_idx)) = self.spread_operand_window(i) else {
                        continue;
                    };
                    let Some((ty, range)) =
                        self.single_reachable_read_between(start, end, boundary_idx)
                    else {
                        continue;
                    };
                    self.emit_operand_face_row(UncheckedOperandFace::Spread, ty, range, out);
                }
                dartq_scanner::TokenKind::Keyword(dartq_scanner::Keyword::Yield) => {
                    // `yield* expression` — the verifier's
                    // `uncheckedUseOfNullableValueInYieldEach` arm
                    // (yield_statement_resolver.dart:163-168,
                    // `nullableDereferenceVerifier.expression` gated on
                    // `node.star != null`, `.at(expression)`); a bare
                    // `yield` asks no operand face. The pseudo-keyword's
                    // identifier reading (`var z = yield * 3` — a bare
                    // multiplication statement) produces no operand read in
                    // its own window, so it under-reports to nothing either
                    // way.
                    let Some((start, end, boundary_idx)) = self.yield_each_window(i) else {
                        continue;
                    };
                    let Some((ty, range)) =
                        self.single_reachable_read_between(start, end, boundary_idx)
                    else {
                        continue;
                    };
                    self.emit_operand_face_row(UncheckedOperandFace::YieldEach, ty, range, out);
                }
                _ => {}
            }
        }
    }

    /// One candidate's verdict: the operand's flow-aware static type (the
    /// read row's own payload) decides. `Ty::Unknown` is the opaque-flow
    /// marker (the lattice could not spell the type) — the nullable
    /// posture of the row still decides, matching the parse-time arms'
    /// treatment of the marker. A `Null`-typed operand reports
    /// `invalid_use_of_null_value` instead — the verifier's `report()`
    /// swaps the code when `receiverType == typeProvider.nullType`
    /// (`nullable_dereference_verifier.dart:47-49`, reached through
    /// `for_resolver.dart:88`/`resolver.dart:4189`).
    fn emit_operand_face_row(
        &self,
        face: UncheckedOperandFace,
        ty: Ty,
        range: SourceRange,
        out: &mut Vec<Diagnostic>,
    ) {
        if !self.operand_face_ty_nullable(ty) {
            return;
        }
        if ty == Ty::Null {
            let code = SemanticCode::InvalidUseOfNullValue;
            out.push(
                Diagnostic::new(
                    code.severity(),
                    code.code_info(),
                    code.message("", ""),
                    self.file,
                    range,
                    None,
                )
                .with_context(self.non_promotion_context_at(range.offset)),
            );
            return;
        }
        let (message, correction) = face.texts();
        out.push(
            Diagnostic::new(
                SemanticCode::UncheckedUseOfNullableValue.severity(),
                SemanticCode::UncheckedUseOfNullableValue.code_info(),
                message.to_string(),
                self.file,
                range,
                Some(correction.to_string()),
            )
            .with_context(self.non_promotion_context_at(range.offset)),
        );
    }

    /// The `isPotentiallyNullable` posture of an operand's static type:
    /// nullable named/record/function shapes, `void`, and `Null` report
    /// (`isPotentiallyNullable`'s `type is VoidType` arm — the
    /// `forIn_iterable_typeVoid` oracle rows carry this code — and the
    /// `NullType` arm, whose row the verifier's `report()` swaps to
    /// `invalid_use_of_null_value`); `dynamic` is excluded upstream
    /// (`NullableDereferenceVerifier._check`'s `DynamicType` arm), and
    /// `Unknown`/`Never` are not potentially nullable.
    fn operand_face_ty_nullable(&self, ty: Ty) -> bool {
        match ty {
            Ty::Void | Ty::Null => true,
            Ty::Named { nullable, .. }
            | Ty::Record { nullable, .. }
            | Ty::Function { nullable, .. } => nullable,
            _ => false,
        }
    }

    /// The for-in iterable's window: the tokens strictly between the
    /// header's `in` keyword and the header's `)` (the paren pairing is
    /// the table's precomputed `end_of`; the closer's index rides along
    /// for the reach gate), plus whether a spread operator stands in that
    /// window — an iterable that is (or embeds) a collection literal has
    /// the literal's own type, so the element read's row answers through
    /// the spread face instead. A C-style `for` carries no `in` and
    /// yields no window; the first `in` after the `(` is always the
    /// header's (the reserved keyword has no identifier reading, so
    /// neither the loop variable nor its pattern can carry one).
    fn for_in_iterable_window(&self, for_idx: usize) -> Option<(u32, u32, usize, bool)> {
        let tokens = self.tokens.as_slice();
        let open = tokens.get(for_idx + 1)?;
        if open.is_synthetic() || open.kind != dartq_scanner::TokenKind::OpenParen {
            return None;
        }
        let close_idx = self.tokens.matching_end(for_idx + 1)?;
        let in_idx = tokens[for_idx + 2..close_idx].iter().position(|t| {
            t.kind == dartq_scanner::TokenKind::Keyword(dartq_scanner::Keyword::In)
        })? + for_idx
            + 2;
        let in_tok = &tokens[in_idx];
        let spread_inside = tokens[in_idx + 1..close_idx]
            .iter()
            .any(|t| t.kind == dartq_scanner::TokenKind::PeriodPeriodPeriod);
        // `in` is ASCII: its UTF-16 end is its byte length past the start.
        Some((
            in_tok.utf16_offset + in_tok.byte_len,
            tokens[close_idx].utf16_offset,
            close_idx,
            spread_inside,
        ))
    }

    /// The utf16 offsets of bare for-in loop targets: `for (i in …)`'s `i`,
    /// where the head carries no `var`/`final`/`const`/`late` declaration
    /// keyword. Upstream the loop variable is the `ForEachPartsWithIdentifier`'s
    /// assignment target — a WRITE per iteration, never a read — so the
    /// official verifier's `_isReadIdentifier` (`inGetterContext` fails)
    /// refuses the used-element credit
    /// (unused_local_elements_verifier.dart:429-453): `int i; for (i in xs)`
    /// still reports `unused_local_variable` on `i`. The row path credits a
    /// bare `i` `resolve_local` use it never converts (no `=`/compound arm
    /// reaches it), so the close retracts the write-position hits here.
    ///
    /// Returns the identifier token's own utf16 offset per bare-target head —
    /// the `local_hits` key the row's read credit parked at. A `var`/typed/
    /// pattern head declares its own loop variable (its `LocalVariableDeclaration`
    /// tracking reports separately) and yields no write offset; a head whose
    /// `(`..`in` window carries anything but a single identifier (a pattern
    /// destructure, a metadata annotation, recovery debris) is left alone —
    /// under-reporting the write rather than mis-crediting a read.
    fn for_in_write_offsets(&self) -> FxHashSet<u32> {
        let mut writes = FxHashSet::default();
        let tokens = self.tokens.as_slice();
        for i in 0..tokens.len() {
            if tokens[i].kind
                != dartq_scanner::TokenKind::Keyword(dartq_scanner::Keyword::For)
            {
                continue;
            }
            // The `(` opens immediately after `for` (`await for` parks the
            // `await` before the `for`, not between `for` and `(`). A
            // non-paren token here is a `for`-element keyword or recovery —
            // never a loop-variable head.
            let open_idx = i + 1;
            if tokens.get(open_idx).is_none_or(|t| {
                t.is_synthetic() || t.kind != dartq_scanner::TokenKind::OpenParen
            }) {
                continue;
            }
            // The head's `in` keyword before the paren's close — the loop
            // variable region is `( .. in`. `await` before `(` is fine.
            let Some(close_idx) = self.tokens.matching_end(open_idx) else {
                continue;
            };
            let Some(in_rel) = tokens[open_idx + 1..close_idx]
                .iter()
                .position(|t| {
                    t.kind
                        == dartq_scanner::TokenKind::Keyword(dartq_scanner::Keyword::In)
                })
            else {
                continue;
            };
            let in_idx = open_idx + 1 + in_rel;
            let head = &tokens[open_idx + 1..in_idx];
            // A bare existing-variable target: exactly one identifier token,
            // no declaration keyword (`var`/`final`/`const`/`late`), no
            // pattern or type structure. Any other shape declares or
            // destructures and reports through its own tracking.
            let mut target: Option<u32> = None;
            let mut bare = true;
            for tok in head {
                match tok.kind {
                    dartq_scanner::TokenKind::Identifier => {
                        if target.is_some() {
                            // A second identifier (a written type name
                            // before the variable, or a pattern head) — not
                            // the bare-target shape.
                            bare = false;
                            break;
                        }
                        target = Some(tok.utf16_offset);
                    }
                    dartq_scanner::TokenKind::Keyword(dartq_scanner::Keyword::Var)
                    | dartq_scanner::TokenKind::Keyword(dartq_scanner::Keyword::Final)
                    | dartq_scanner::TokenKind::Keyword(dartq_scanner::Keyword::Const)
                    | dartq_scanner::TokenKind::Keyword(dartq_scanner::Keyword::Late) => {
                        bare = false;
                        break;
                    }
                    _ => {
                        bare = false;
                        break;
                    }
                }
            }
            if bare && let Some(offset) = target {
                writes.insert(offset);
            }
        }
        writes
    }

    /// The `(start, end)` utf16 windows of `when` guard clauses: a read whose
    /// offset lands inside one resolved the case head's OWN pattern variable
    /// (the official `_resolveGuardedPattern` resolves the guarded pattern's
    /// individual `BindPatternVariableElement`, never the shared-case-scope
    /// joined element — resolution_visitor.dart:926-929). The window ends at
    /// the head's `:`/`=>` terminator (or the `;`/`{`/`}`/`case`/`default` a
    /// recovered head runs into): nested parens/brackets/braces jump whole so
    /// an argument list or collection literal inside the guard can't forge
    /// the boundary.
    ///
    /// Reads OUTSIDE every window against an arm-scoped binder are BODY
    /// reads — the joined element's own scope — and those are the only reads
    /// the all-cases join spares through `arm_name_used`
    /// (`UsedLocalElements.addElement` expanding a
    /// `JoinPatternVariableElementImpl`'s transitiveVariables,
    /// unused_local_elements_verifier.dart:1237-1241).
    fn guard_spans(&self) -> Vec<(u32, u32)> {
        let mut spans = Vec::new();
        let tokens = self.tokens.as_slice();
        for i in 0..tokens.len() {
            if tokens[i].kind
                != dartq_scanner::TokenKind::Keyword(dartq_scanner::Keyword::When)
            {
                continue;
            }
            let mut end = tokens[i].utf16_offset;
            let mut j = i + 1;
            while j < tokens.len() {
                match tokens[j].kind {
                    dartq_scanner::TokenKind::Colon
                    | dartq_scanner::TokenKind::Function
                    | dartq_scanner::TokenKind::Semicolon
                    | dartq_scanner::TokenKind::CloseCurly
                    | dartq_scanner::TokenKind::Keyword(dartq_scanner::Keyword::Case)
                    | dartq_scanner::TokenKind::Keyword(dartq_scanner::Keyword::Default) => {
                        end = tokens[j].utf16_offset;
                        break;
                    }
                    // A group opener's whole extent jumps past its close —
                    // `:`/`=>` inside `f(…)`, `[…]`, `{…}` is the group's own.
                    dartq_scanner::TokenKind::OpenParen
                    | dartq_scanner::TokenKind::OpenSquare
                    | dartq_scanner::TokenKind::OpenCurly => {
                        j = self.tokens.matching_end(j).unwrap_or(j) + 1;
                        continue;
                    }
                    _ => {}
                }
                j += 1;
            }
            spans.push((tokens[i].utf16_offset, end));
        }
        spans
    }

    /// A spread operand's window: the tokens between the `...` operator
    /// and the element boundary at the operator's own nesting depth (the
    /// next `,` or closing bracket; the boundary's index rides along for
    /// the reach gate). Begin-groups jump whole via the table's
    /// precomputed pairing, so nested collections, type arguments, and
    /// interpolations never forge a boundary.
    fn spread_operand_window(&self, dots_idx: usize) -> Option<(u32, u32, usize)> {
        let tokens = self.tokens.as_slice();
        let dots = &tokens[dots_idx];
        let mut j = dots_idx + 1;
        while j < tokens.len() {
            if let Some(close) = self.tokens.matching_end(j) {
                j = close + 1;
                continue;
            }
            if matches!(
                tokens[j].kind,
                dartq_scanner::TokenKind::Comma
                    | dartq_scanner::TokenKind::CloseParen
                    | dartq_scanner::TokenKind::CloseSquare
                    | dartq_scanner::TokenKind::CloseCurly
                    | dartq_scanner::TokenKind::Eof
            ) {
                let start = dots.utf16_offset + dots.byte_len;
                let end = tokens[j].utf16_offset;
                return (start < end).then_some((start, end, j));
            }
            j += 1;
        }
        None
    }

    /// A `yield*` operand's window: the tokens between the `*` marker and
    /// the statement's `;` terminator at the marker's own nesting depth
    /// (begin-groups jump whole, so a parenthesized or collection operand
    /// never forges the boundary). A `yield` without the star (or a `yield`
    /// used as an identifier — the pseudo-keyword's other reading) yields
    /// no window. Returns `(operand start, operand end, boundary index)`.
    fn yield_each_window(&self, yield_idx: usize) -> Option<(u32, u32, usize)> {
        let tokens = self.tokens.as_slice();
        let star = tokens.get(yield_idx + 1)?;
        if star.is_synthetic() || star.kind != dartq_scanner::TokenKind::Star {
            return None;
        }
        let mut j = yield_idx + 2;
        while j < tokens.len() {
            if let Some(close) = self.tokens.matching_end(j) {
                j = close + 1;
                continue;
            }
            if matches!(
                tokens[j].kind,
                dartq_scanner::TokenKind::Semicolon
                    | dartq_scanner::TokenKind::CloseCurly
                    | dartq_scanner::TokenKind::Eof
            ) {
                let start = star.utf16_offset + star.byte_len;
                let end = tokens[j].utf16_offset;
                return (start < end).then_some((start, end, j));
            }
            j += 1;
        }
        None
    }

    /// The single read that IS the window's operand: exactly one
    /// [`Engine::read_types`] row wholly inside `[start, end)`, and that
    /// read's span reaches the window's last token (the operand's own
    /// extent). A compound iterable's window — an if-null pair (two
    /// reads), a member chain (its receiver read ends mid-window, never
    /// standing for the chain's type) — attributes to nothing, the
    /// under-report direction the parse-time staging holds.
    fn single_reachable_read_between(
        &self,
        start: u32,
        end: u32,
        boundary_idx: usize,
    ) -> Option<(Ty, SourceRange)> {
        let tokens = self.tokens.as_slice();
        // The window's last content token: the operand's span must reach
        // it (`for (var x in l.length)`'s `l` read ends mid-window — the
        // iterable's type is the chain's, not the receiver's).
        let last_tok_offset = tokens[boundary_idx.checked_sub(1)?].utf16_offset;
        let mut found: Option<(Ty, SourceRange)> = None;
        for (range, ty) in &self.read_types {
            if range.offset >= end {
                break;
            }
            if range.offset >= start {
                if found.is_some() {
                    return None;
                }
                if range.end() < last_tok_offset {
                    return None;
                }
                found = Some((*ty, *range));
            }
        }
        found
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
        // A fresh container frame owns a fresh primary-constructor formal
        // set: the previous class body's formals never answer inside this
        // one (the set is cleared again at the pop, so a top-level `late`
        // group after the class cannot read the stale names either).
        if matches!(&kind, FrameKind::Container { .. }) {
            self.primary_formals.clear();
            self.primary_formals_muted = true;
        }
        self.frames.push(Frame {
            kind,
            serial,
            locals: FxHashSet::default(),
            var_types: FxHashMap::default(),
            fn_formal_rets: FxHashMap::default(),
            local_fn_sigs: FxHashMap::default(),
            formal_optional_from: None,
            formal_named: false,
            type_mark,
            row_mark,
        });
        self.frame_local_marks.push(self.locals.len());
        self.const_scopes.push(FxHashMap::default());
    }

    pub(crate) fn pop_frame(&mut self) {
        if let Some(frame) = self.frames.pop() {
            // The container's primary-constructor formals die with the
            // frame: a top-level `late` group after the class must not read
            // them through the stale set.
            if matches!(&frame.kind, FrameKind::Container { .. }) {
                self.primary_formals.clear();
                self.primary_formals_muted = true;
            }
            // A block-bodied executable leaves `body_mark` latched (the
            // arrow-body arm `take`s it at close; the block body's own end
            // never reads it) — the stale `Some` would read as "a body
            // opened" through the next declaration's head region
            // (`fn_capture_begin`'s head window). The latch dies with the
            // frame that owns it; `close_frame_locals` consumes `frame`,
            // so the kind reads first.
            let was_executable = matches!(frame.kind, FrameKind::Executable { .. });
            // A local function's executable frame releases its
            // body-completion row at this pop (upstream runs
            // `checkForBodyMayCompleteNormally` on the `FunctionDeclaration`
            // walk — resolver.dart:3109 — and a local declaration's
            // `end_local_function_declaration` carries no dedicated staging
            // call). The popping frame is the local function's exactly when
            // `local_fn_stack`'s top names its index (`note_local_function`
            // records `frames.len() - 1` at the name event — the popped
            // index after the pop above). Named executables stage from
            // their own end events, so this release can only ever arm the
            // declaration flavor; an unstaged non-local frame (the setter's
            // early return, a recovery path) simply finds no stack entry.
            if self
                .local_fn_stack
                .last()
                .is_some_and(|(depth, _)| *depth == self.frames.len())
                && let FrameKind::Executable {
                    ret: Some(exec),
                    body_brace,
                    is_async,
                    generator,
                    ..
                } = &frame.kind
            {
                self.stage_local_fn_completion(
                    exec.ty,
                    exec.name_span,
                    *body_brace,
                    *is_async,
                    *generator,
                );
            }
            // The tear-off capture reads this frame's own type-parameter
            // window and tracked-locals window — both still untruncated
            // here (the truncate and the locals close run after).
            self.capture_local_fn_tearoff(&frame);
            self.type_params.truncate(frame.type_mark);
            self.close_frame_locals(frame);
            if was_executable {
                self.body_mark = None;
            }
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

    /// The local-function tear-off capture (#2842): a declared local
    /// function's own value IS the tear-off (`plus is IntFun` over a local
    /// `int plus(int a, [int b = 1])` — the name is a frame local, so the
    /// top-level and member tear-off faces refuse it), and its signature
    /// is only whole at the function's own frame close: the return
    /// annotation latched in the frame's executable kind, the formals the
    /// tracked declaration order typed by the frame's `var_types`. The
    /// capture joins the ENCLOSING frame — the name's own scope, the same
    /// lifetime [`Engine::note_local_function`] gives the `locals` entry.
    /// Everything the declared-face model cannot spell captures nothing
    /// (the under-report direction): a generic function (its tear-off
    /// instantiates to bounds outside the model, the #2835 discipline), a
    /// named-formal section (the closure capture's poison precedent), a
    /// formal list no `[`/`{` event ever answered. Unwritten columns ride
    /// the executable-capture convention — `Ty::Unknown` leaves the
    /// verdict's [`Self::official_fn_face`] normalizes to `dynamic`, the
    /// analyzer element's own read.
    fn capture_local_fn_tearoff(&mut self, frame: &Frame) {
        let FrameKind::Executable { ret: Some(ret), .. } = &frame.kind else {
            return;
        };
        // The host word plus the enclosing frame's name entry pick the
        // local-function frames out: top-level functions and accessors
        // share the `Function` host word but join no frame's `locals`.
        if ret.host != ExecKind::Function
            || !self
                .frames
                .last()
                .is_some_and(|enclosing| enclosing.locals.contains(&ret.name))
        {
            return;
        }
        // A generic local function's tear-off instantiates to bounds this
        // snapshot cannot spell — it captures nothing. The frame's own
        // header window is exactly the type parameters pushed above its
        // mark (nested frames truncated their own on their pops). An
        // async/generator body rewrites the element's return (`int f()
        // async` reads `Future<int> Function()`), which the written
        // annotation cannot stand for — the closure capture's poison
        // precedent.
        if self.type_params.len() > frame.type_mark
            || frame.formal_named
            || matches!(
                frame.kind,
                FrameKind::Executable { is_async: true, .. }
                    | FrameKind::Executable {
                        generator: true,
                        ..
                    }
            )
        {
            return;
        }
        let mark = *self.frame_local_marks.last().unwrap_or(&0);
        let positional: Vec<Ty> = self.locals[mark..]
            .iter()
            .filter(|decl| matches!(decl.kind, LocalKind::Parameter { .. }))
            .map(|decl| {
                frame
                    .var_types
                    .get(&decl.name)
                    .copied()
                    .unwrap_or(Ty::UNKNOWN)
            })
            .collect();
        let required_positional = frame
            .formal_optional_from
            .unwrap_or(u16::try_from(positional.len()).unwrap_or(u16::MAX));
        let sig = Box::new(FnVarSig {
            params: Box::default(),
            required_positional,
            positional: positional.into_boxed_slice(),
            named: Box::default(),
            return_ty: ret.ty,
        });
        if let Some(enclosing) = self.frames.last_mut() {
            enclosing.local_fn_sigs.insert(ret.name, sig);
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
        // A destructuring `PatternVariableDeclaration` registers each binder
        // into the enclosing statement-list duplicate scope twice: once when
        // `add_tracked_local` claims the name (the declaration's own
        // `checkStatements` arm) and again when the statement boundary's
        // `handle_pattern_variable_declaration_statement` re-adds every group
        // member. The second insert finds the FIRST registration's identical
        // span, so `stage_local_duplicate` reports the token against itself
        // (range == context first range). Upstream inserts each declared
        // element once, so a self-coincident row is always the double-add —
        // a genuine redeclaration carries a distinct second span. Retract it.
        // This is the destructuring `var (a, b) = …` face; plain `var x;` and
        // `note_local`/`note_local_function` each register once and never
        // produce a self-coincident row.
        self.report.usage.retain(|d| {
            !(d.code.name == "duplicate_definition"
                && d.context.as_ref().map_or(false, |ctx| {
                    ctx.first().map_or(false, |c| c.range == d.range)
                }))
        });
        let mark = self.frame_local_marks.pop().unwrap_or(self.locals.len());
        if mark >= self.locals.len() {
            self.drop_stale_local_hits(mark);
            self.locals.truncate(mark);
            return;
        }
        // Pattern-declaration groups report all-or-nothing: one used member
        // spares the group (official visitPatternVariableDeclaration). A
        // case-header binder never joins that window: the official verifier
        // visits each case pattern's variable as its own local
        // (`visitDeclaredVariablePattern` outside any
        // `PatternVariableDeclaration`, unused_local_elements_verifier.dart
        // :565-579), so only a same-name sibling keeps the exemption — the
        // logical-or / shared-case join is one element upstream and a use of
        // any of its spellings marks every transitive variable used
        // (`UsedLocalElements.addElement`). The sentinel group value (the
        // declaring switch's depth, not a window id) keys that pair, so a
        // used binder in one arm never spares a different spelling in
        // another.
        let mut group_used: FxHashMap<u32, bool> = FxHashMap::default();
        let mut arm_name_used: FxHashSet<(u32, StringId)> = FxHashSet::default();
        // The write-position hits that still ride `local_hits`: a bare
        // for-in loop target (`for (i in xs)`'s `i`) resolves a read credit
        // the official `_isReadIdentifier` refuses — the loop assigns `i`,
        // it never reads it. The hit's offset is the write position; the
        // count of a declaration's surviving read credits that sit on a
        // write offset is subtracted before the `uses > 0` gate below.
        let for_in_writes = if self.local_hits.is_empty() {
            FxHashSet::default()
        } else {
            self.for_in_write_offsets()
        };
        // The arm-scope split the official shared-case-scope join draws:
        // a `when` guard's read resolves the case head's OWN binder (the
        // individual `BindPatternVariableElement`), while a body-statement
        // read resolves the joined element — and only the join's
        // transitiveVariables expansion marks every same-named sibling used.
        // `guard_spans` gives the `when` extents; a `(group, name)` spelling
        // joins (spares all) iff a same-named binder took a BODY read — a
        // crediting hit outside every guard window. A binder's own `uses`
        // still spares itself below; this set only models the cross-arm join.
        let guard_spans = self.guard_spans();
        // Precompute the per-declaration effective read counts (the closures
        // would hold an immutable borrow across the mutable `stage_usage`
        // below): `effective[i]` = `uses` minus the for-in write-position
        // hits, `body_read[i]` = whether a surviving credit sits outside a
        // `when` guard.
        let mut effective_uses = Vec::with_capacity(self.locals.len() - mark);
        for (offset, decl) in self.locals[mark..].iter().enumerate() {
            let index = mark + offset;
            let write_hits = if for_in_writes.is_empty() {
                0
            } else {
                self.local_hits
                    .iter()
                    .filter(|(o, hit)| **hit as usize == index && for_in_writes.contains(*o))
                    .count() as u32
            };
            let body_read = self.local_hits.iter().any(|(o, hit)| {
                *hit as usize == index
                    && !guard_spans
                        .iter()
                        .any(|(s, e)| *o >= *s && *o < *e)
            });
            effective_uses.push((decl.uses.saturating_sub(write_hits), body_read));
        }
        for (offset, decl) in self.locals[mark..].iter().enumerate() {
            let (effective, body_read) = effective_uses[offset];
            match decl.group {
                Some(group) if group >= ARM_SCOPE_SENTINEL_BASE => {
                    if effective > 0 && body_read {
                        arm_name_used.insert((group, decl.name));
                    }
                }
                Some(group) => {
                    *group_used.entry(group).or_default() |= effective > 0;
                }
                None => {}
            }
        }
        for index in mark..self.locals.len() {
            let (name, range, kind, group) = {
                let decl = &self.locals[index];
                (decl.name, decl.range, decl.kind, decl.group)
            };
            let uses = effective_uses[index - mark].0;
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
                Some(group) if group >= ARM_SCOPE_SENTINEL_BASE => {
                    arm_name_used.contains(&(group, name))
                }
                Some(group) => group_used.get(&group).copied().unwrap_or(false),
                None => false,
            };
            if spared {
                continue;
            }
            let wildcard = self.is_wildcard(name);
            // The nameless recovery face renders `<unnamed>` (`ElementImpl
            // .displayName`'s `name ?? '<unnamed>'`). Every nameless
            // declaration kind carries an empty spelling upstream — the
            // catch rows (the parser's synthetic-filled trace/exception
            // slots, `C::CatchParameter`'s `synthetic_name` intern), a
            // nameless local variable or local function (the parser's
            // synthetic-filled declaration-name slot), and a nameless
            // parameter (the same synthetic-filled formal slot; it keeps
            // the substitution for completeness even though its arm
            // `continue`s below before emitting).
            let raw_text = self.strings.resolve(name);
            let name_text = if raw_text.is_empty()
                && matches!(
                    kind,
                    LocalKind::CatchException { .. }
                        | LocalKind::CatchStack
                        | LocalKind::Variable
                        | LocalKind::LocalFunction
                        | LocalKind::Parameter { .. }
                ) {
                "<unnamed>".to_string()
            } else {
                raw_text.to_string()
            };
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
        self.drop_stale_local_hits(mark);
        self.locals.truncate(mark);
    }

    /// The `local_hits` pairing invariant: every entry's index must point
    /// into the live `locals` range. A read credited inside a frame can
    /// outlive it — parser recovery re-walks tokens after the frame's body
    /// closed (the `for`-body double walk), so a write-intent conversion
    /// may consume the entry after `locals` truncated. Dropping the
    /// truncated range's entries keeps the read's `uses` credit (the
    /// declaration is gone — no verdict reads it) and turns the
    /// conversion's `remove` into a miss instead of an out-of-bounds
    /// index (`parser_recovery_04`'s `x.(…)` face).
    fn drop_stale_local_hits(&mut self, mark: usize) {
        if self.local_hits.is_empty() {
            return;
        }
        self.local_hits.retain(|_, hit| (*hit as usize) < mark);
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

    /// The strict subtype verdict over the resolved operand faces — the
    /// official `TypeSystem.isSubtypeOf` walk (`_isSubtypeOf`, upstream
    /// Dart SDK source, pkg/analyzer/lib/src/dart/element/subtype.dart
    /// 64-524) mirrored at the [`Ty`] shapes the resolved face spells:
    ///
    /// - 0c `InvalidType` either side answers `true`
    ///   ([`Self::is_invalid_ty`]; subtype.dart 72-74);
    /// - Left Bottom `Never` (163-165);
    /// - Left/Right `FutureOr` flattening (371-449: `FutureOr<S> <: T`
    ///   iff `S <: T` and `Future<S> <: T`; `S <: FutureOr<T>` iff
    ///   `S <: Future<T>` or `S <: T`, the bound leg included);
    /// - Right Top (216-224: `dynamic`, `void`, `Object?`), Left Top
    ///   (226-231: `dynamic`/`void` fit only the tops), Right `Object`
    ///   (233-317: `Null`, the tops, and every `S?` fail; a bare
    ///   in-scope type parameter answers through its bound);
    /// - Left Null (319-344: `Null` fits only the tops, every nullable
    ///   spelling, and `FutureOr<S>` iff `Null <: S`);
    /// - Left Nullable (346-351: `S0? <: T1` iff `S0 <: T1` and
    ///   `Null <: T1`) and Right Nullable (452-478: `S0 <: S1?` iff
    ///   `S0 <: S1`, the `Null` leg kept);
    /// - function/record faces: a function (record) type subtypes the
    ///   bare `Function` (`Record`) head (505-520), function- and
    ///   record-shape pairs keep [`Engine::slot_subtype`] (its function
    ///   rule is real subtyping — contravariant formals, covariant
    ///   return — and its record rule the shape identity), and every
    ///   other pairing is false (interface types never subtype
    ///   `Function`/record shapes, subtype.dart 196-200);
    /// - same-head interface pairs decide through the type-argument
    ///   list ([`Self::same_head_args`], the `_interfaceArguments`
    ///   variance walk over the element model's covariant default) and
    ///   distinct heads keep the declared-edge lattice
    ///   ([`Self::name_subtype`]).
    ///
    /// `true` is the proven-constant answer only; every undecidable
    /// shape answers `false` — the under-report direction every
    /// consumer of the fold already takes. The name-level fallback is
    /// deliberately NOT [`Engine::slot_subtype`]: its same-head matrix
    /// runs the assignable face, whose legacy `Null` and top leniencies
    /// answer `Null → C` beyond the strict relation — as a subtype
    /// answer that over-fires the constant-true verdict
    /// (`List<Null> is List<Object>`: the covariant argument leg asks
    /// `Null <: Object` and Left Null refuses — the oracle is silent).
    pub(crate) fn resolved_subtype(&self, sub: Ty, sup: Ty) -> bool {
        self.strict_subtype(sub, sup, 0)
    }

    /// `ty` with the `?` flag cleared — the Left/Right Nullable strips
    /// (`T0.withNullability(none)` / `T1.withNullability(none)`,
    /// subtype.dart 347-348 / 453-454). `Ty` is an enum, so the fields
    /// copy explicitly per variant; the non-nullable faces pass through.
    pub(crate) fn non_nullable_ty(ty: Ty) -> Ty {
        match ty {
            Ty::Named { name, args, .. } => Ty::Named {
                name,
                nullable: false,
                args,
            },
            Ty::Record { shape, .. } => Ty::Record {
                shape,
                nullable: false,
            },
            Ty::Function { shape, .. } => Ty::Function {
                shape,
                nullable: false,
            },
            other => other,
        }
    }

    /// The canonical `Null` face: a bare `Named{Null}` spelling reads as
    /// [`Ty::Null`] (every spelling of `Null` resolves to the one
    /// `NullType` — `isDartCoreNull`'s identity, the type-position
    /// capture's identifier row and the inference face's unit meeting
    /// at one variant before any structural ask).

    /// The canonical `Null` face: a bare `Named{Null}` spelling reads as
    /// [`Ty::Null`] (every spelling of `Null` resolves to the one
    /// `NullType` — `isDartCoreNull`'s identity, the type-position
    /// capture's identifier row and the inference face's unit meeting
    /// at one variant before any structural ask).
    fn normalized_null(&self, ty: Ty) -> Ty {
        if let Ty::Named {
            name,
            nullable: false,
            args: ArgListId::EMPTY,
        } = ty
            && name == self.intern("Null")
        {
            return Ty::Null;
        }
        ty
    }

    /// The canonical `Never` face, whole-face form of
    /// [`Self::normalized_null`]: every bare `Named{Never}` spelling —
    /// head, type argument, or a function shape's formal/return leaf —
    /// reads as the one `NeverType` (`isDartCoreNever`'s identity, the
    /// same unit meeting). The written-leaf captures (`typedef
    /// Fun(Never a, Never b)`'s synthesized signature) carry the
    /// identifier spelling; without the fold the deep slot asks read
    /// `Named{Never}` against a concrete head as two unrelated names and
    /// answer undecidable where the official's canonical bottom decides.
    /// Nullable spellings (`Never?`) are no bottom face and keep their
    /// written shape.
    fn normalized_never(&self, ty: Ty) -> Ty {
        self.normalize_never_at(ty, 0)
    }

    fn normalize_never_at(&self, ty: Ty, depth: u32) -> Ty {
        if depth > 16 {
            return ty;
        }
        let next = depth + 1;
        match ty {
            Ty::Named {
                name,
                nullable: false,
                args: ArgListId::EMPTY,
            } if name == self.intern("Never") => Ty::Never,
            Ty::Named {
                name,
                nullable,
                args,
            } => {
                if args == ArgListId::EMPTY {
                    ty
                } else {
                    let args: Vec<Ty> = {
                        let pool = self.boundary.pool();
                        pool.get(args).to_vec()
                    };
                    let args: Vec<Ty> = args
                        .into_iter()
                        .map(|arg| self.normalize_never_at(arg, next))
                        .collect();
                    let args = self.boundary.intern_list(&args);
                    Ty::Named {
                        name,
                        nullable,
                        args,
                    }
                }
            }
            Ty::Function { shape, nullable } => {
                let (type_params, names, return_ty, positional, named) = {
                    let pool = self.boundary.pool();
                    let shape = pool.function_shape(shape);
                    (
                        shape.type_params,
                        shape.type_params_named.to_vec(),
                        shape.return_ty,
                        shape.positional.to_vec(),
                        shape.named.to_vec(),
                    )
                };
                let return_ty = self.normalize_never_at(return_ty, next);
                let positional: Vec<Ty> = positional
                    .into_iter()
                    .map(|formal| self.normalize_never_at(formal, next))
                    .collect();
                let named: Vec<(StringId, Ty)> = named
                    .into_iter()
                    .map(|(name, formal)| (name, self.normalize_never_at(formal, next)))
                    .collect();
                // The generic-parameter names ride the rebuild (the
                // generic-signature asks correspond on them): a count-only
                // producer would strip the very names the faces above
                // preserved.
                if names.is_empty() {
                    self.boundary.intern_function_ty(
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
            Ty::Record { shape, nullable } => {
                let (fields, named) = {
                    let pool = self.boundary.pool();
                    let shape = pool.record_shape(shape);
                    (shape.positional.to_vec(), shape.named.to_vec())
                };
                let fields: Vec<Ty> = fields
                    .into_iter()
                    .map(|field| self.normalize_never_at(field, next))
                    .collect();
                let named: Vec<(StringId, Ty)> = named
                    .into_iter()
                    .map(|(name, field)| (name, self.normalize_never_at(field, next)))
                    .collect();
                self.boundary
                    .args
                    .borrow_mut()
                    .record(&fields, &named, nullable)
            }
            _ => ty,
        }
    }

    /// The recursion cap of [`Self::strict_subtype`]: bounded depth in
    /// place of the official `TypeOperationDepthLimitExceeded` circuit
    /// (subtype.dart 18-25 — the over-limit ask answers `false`), the
    /// engine's termination invariant for the mutual-recursion the
    /// argument walk and the bound legs drive.
    const SUBTYPE_DEPTH: u32 = 10;

    fn strict_subtype(&self, sub: Ty, sup: Ty, depth: u32) -> bool {
        if depth >= Self::SUBTYPE_DEPTH {
            return false;
        }
        // The `Null` head normalizes to the canonical unit face: a
        // type-position capture spells `Named{Null}` (the identifier's
        // own row) while inference-side faces carry `Ty::Null`, and the
        // official resolves every spelling to the one `NullType`
        // (`isDartCoreNull`, the NullTypeImpl identity).
        let sub = self.normalized_null(sub);
        let sup = self.normalized_null(sup);
        let next = depth + 1;
        // Reflexivity (subtype.dart 66-68) — the argument pool's
        // canonical ids make the structural ask an `Eq` ask.
        if sub == sup {
            return true;
        }
        // 0c: an `InvalidType` on either side answers `true`
        // (subtype.dart 72-74).
        if self.is_invalid_ty(sub) || self.is_invalid_ty(sup) {
            return true;
        }
        // Left Bottom: `Never <: T1` (subtype.dart 163-165).
        if sub == Ty::Never {
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
        // Left FutureOr (subtype.dart 371-385): `FutureOr<S0> <: T1` iff
        // `S0 <: T1` and `Future<S0> <: T1`.
        if let Some((value, args)) = future_or_parts(sub) {
            let future = Ty::Named {
                name: self.intern("Future"),
                nullable: false,
                args,
            };
            return self.strict_subtype(value, sup, next) && self.strict_subtype(future, sup, next);
        }
        // Right FutureOr (subtype.dart 418-449): `S0 <: FutureOr<S1>`
        // iff `S0 <: Future<S1>` or `S0 <: S1` — the type-parameter
        // bound leg included (`X0` with bound `B0`: `B0 <: FutureOr<S1>`).
        if let Some((value, args)) = future_or_parts(sup) {
            let future = Ty::Named {
                name: self.intern("Future"),
                nullable: false,
                args,
            };
            if self.strict_subtype(sub, future, next) || self.strict_subtype(sub, value, next) {
                return true;
            }
            if let Ty::Named {
                name: param,
                args: ArgListId::EMPTY,
                nullable: false,
            } = sub
                && self.is_scope_param(param)
                && let Some(bound) = self.param_bound(param)
                && bound != Ty::Dynamic
            {
                return self.strict_subtype(bound, sup, next);
            }
            return false;
        }
        // Right Top (subtype.dart 216-224): `dynamic`, `void`, `Object?`.
        if sup == Ty::Dynamic || sup == Ty::Void {
            return true;
        }
        if let Ty::Named {
            name,
            nullable: true,
            args: ArgListId::EMPTY,
        } = sup
            && name == self.intern("Object")
        {
            return true;
        }
        // Right Null: nothing but `Null`/`Never` (both caught above)
        // subtypes `Null` (subtype.dart 342-344's tail).
        if sup == Ty::Null {
            return false;
        }
        // Left Null (subtype.dart 319-344): the tops and every nullable
        // spelling fit; `FutureOr<S>` rode the Right leg above
        // (`Null <: S`); a non-nullable concrete face never does.
        if sub == Ty::Null {
            return matches!(
                sup,
                Ty::Named { nullable: true, .. }
                    | Ty::Record { nullable: true, .. }
                    | Ty::Function { nullable: true, .. }
            );
        }
        // Left Nullable (subtype.dart 346-351): `S0? <: T1` iff `S0 <: T1`
        // and `Null <: T1`.
        if matches!(
            sub,
            Ty::Named { nullable: true, .. }
                | Ty::Record { nullable: true, .. }
                | Ty::Function { nullable: true, .. }
        ) {
            return self.strict_subtype(Self::non_nullable_ty(sub), sup, next)
                && self.strict_subtype(Ty::Null, sup, next);
        }
        // Left Top (subtype.dart 226-231): `dynamic`/`void` fit only the
        // tops — every fit the Right Top arm above answers, so a
        // remaining target is no fit.
        if sub == Ty::Dynamic || sub == Ty::Void {
            return false;
        }
        // Right Object's tail (subtype.dart 307-317): after the `Null`,
        // top, and `S?` rejections above, every source fits `Object` —
        // except a bare in-scope type parameter, which answers through
        // its bound (`B0 <: Object`; an unbounded parameter is bounded
        // by `dynamic`, no fit).
        if let Ty::Named {
            name,
            nullable: false,
            args: ArgListId::EMPTY,
        } = sup
            && name == self.intern("Object")
        {
            if let Ty::Named {
                name: param,
                args: ArgListId::EMPTY,
                ..
            } = sub
                && self.is_scope_param(param)
            {
                return match self.param_bound(param) {
                    Some(bound) if bound != Ty::Dynamic => self.strict_subtype(bound, sup, next),
                    _ => false,
                };
            }
            return true;
        }
        // Function/record faces (subtype.dart 505-520): the bare
        // `Function`/`Record` heads fit; shape pairs keep the slot walk;
        // every other target is false — interface types never subtype
        // `Function` or a record shape (subtype.dart 196-200).
        match sub {
            Ty::Function { .. } => match sup {
                Ty::Named {
                    name,
                    nullable: false,
                    args: ArgListId::EMPTY,
                } if name == self.intern("Function") => true,
                // A generic signature pair decides through the positional
                // unification first (`relateTypeParameters` — the interned
                // shapes carry `type_params_named`, which the pooled
                // `FnVarSig` rebuild in `function_shape_sig` cannot spell:
                // count-only entries never decide). Non-generic pairs and
                // every undecidable generic face keep the slot walk.
                Ty::Function { .. } => match self.generic_signature_subtype(sub, sup) {
                    Some(answer) => answer,
                    // The required-positional map decides the arity gate the
                    // pooled rebuild fabricates (`required = positional.len`):
                    // rebuild both signatures with the written counts and run
                    // `fn_sig_subtype`'s honest legs before the slot walk —
                    // `x is List<int> Function([int])` over a
                    // `List<int> Function(int)` operand answers `false`
                    // (the source's required 1 exceeds the target's 0), not
                    // the canonical-shape reflexivity the interned ids give.
                    None => match (self.fn_face_sig(sub, None), self.fn_face_sig(sup, None)) {
                        (Some(from), Some(to)) => match self.fn_sig_subtype(&from, &to) {
                            Some(answer) => answer,
                            None => self.slot_subtype(sub, sup) == Some(true),
                        },
                        _ => self.slot_subtype(sub, sup) == Some(true),
                    },
                },
                _ => false,
            },
            Ty::Record { .. } => match sup {
                Ty::Named {
                    name,
                    nullable: false,
                    args: ArgListId::EMPTY,
                } if name == self.intern("Record") => true,
                Ty::Record { .. } => self.slot_subtype(sub, sup) == Some(true),
                _ => false,
            },
            // The interface faces. The Right Nullable strip precedes the
            // argument walk (`S <: C<A>?` iff `S <: C<A>`); the bare
            // in-scope type parameter answers through its bound (Left
            // Type Variable Bound, subtype.dart 484-493).
            Ty::Named {
                name,
                args: sub_args_id,
                ..
            } => {
                let Ty::Named {
                    name: sup_name,
                    nullable: sup_nullable,
                    args: sup_args_id,
                } = sup
                else {
                    return false;
                };
                if sup_nullable {
                    return self.strict_subtype(sub, Self::non_nullable_ty(sup), next);
                }
                if sub_args_id == ArgListId::EMPTY
                    && self.is_scope_param(name)
                    && let Some(bound) = self.param_bound(name)
                {
                    // An unbounded parameter is bounded by `dynamic` —
                    // `dynamic <: C` is no fit (Left Top above).
                    return bound != Ty::Dynamic && self.strict_subtype(bound, sup, next);
                }
                if sup_name == name {
                    return self.same_head_args(name, sub_args_id, sup_args_id, sub, sup, next);
                }
                // The interface-cycle collapse: upstream resets a cycle
                // participant's whole face (`interface_cycles.dart:64-82` —
                // `supertype` becomes `objectType`, `mixins`/`interfaces`
                // cleared), so `allSupertypes` of a cyclic head is the bare
                // `[Object]` and any distinct-head target it once declared
                // is unprovable. The snapshot keeps the surviving edges
                // (`class C2 = S with C2` holds `C2 → S` while the `C2` back
                // edge drops), so a cyclic `sub` would prove `C2 <: S` where
                // the oracle reads only `Object` — the over-proved extra.
                // `sup == Object` already answered above (Right Object's
                // tail); any other distinct head suppresses.
                if self.decl_in_interface_cycle(name) {
                    return false;
                }
                // Distinct heads: the substituted snapshot answers first —
                // the official verdict reads `allSupertypes` with
                // substitution (subtype.dart's interface case), a face the
                // name-level edge walk cannot spell (its hit fires on the
                // spelling alone: `B<num, String> <: Mixin1<String>` through
                // the mixin-application alias `class MyTypedef<K, V> = A
                // with Mixin1<K>, Mixin2<V>` proves where the oracle
                // refuses — the alias's real instantiation is
                // `Mixin1<num>`). A pair the snapshot cannot map, or one
                // carrying a type parameter whose leaf the M1 parameter
                // arms would answer past its bound, keeps the name-level
                // verdict — the under-report direction.
                if let Some(ops) = self.ops
                    && let (Some(sub_id), Some(sup_id)) = {
                        let pool = self.boundary.args.borrow();
                        (ops.map_ty_strict(sub, &pool), ops.map_ty_strict(sup, &pool))
                    }
                    && !ops.mentions_param(sub_id)
                    && !ops.mentions_param(sup_id)
                {
                    return ops.subtype_ids(sub_id, sup_id);
                }
                self.name_subtype(sub, sup)
            }
            _ => false,
        }
    }

    /// Whether `name`'s declared supertype graph participates in an
    /// interface cycle — the element the official `breakInterfaceCycles`
    /// clears (upstream Dart SDK source,
    /// pkg/analyzer/lib/src/summary2/interface_cycles.dart:29-83): a
    /// declaration whose superclass/`with`/`implements`/`on` edges reach
    /// itself transitively. The collapse it mirrors is upstream's
    /// `_markCircular` — `supertype = objectType`, `mixins`/`interfaces`
    /// emptied — so a cyclic head's `allSupertypes` is `[Object]` alone,
    /// and the subtype verdict cannot prove it against any declared
    /// distinct head it once named.
    ///
    /// The walk is the declared-edge graph the container model records
    /// (superclass + with + implements + on, the same set
    /// [`NameWorld::direct_edges`] traverses), each hop re-resolved through
    /// [`resolve_container`] so a cross-file edge descends in its own
    /// declaring file. `name` itself seeds the visited set: a direct
    /// self-edge (`class C2 = S with C2`'s `with C2`, `class C4 = S with M
    /// implements C4`'s `implements C4`) and a transitive return (`A
    /// extends B`, `B extends A`) both close the cycle. A bounded frontier
    /// keeps the malformed deep-hierarchy case terminating.
    fn decl_in_interface_cycle(&self, name: StringId) -> bool {
        // The frontier carries `(head, declaring file, declaring library)`:
        // an edge resolves in the file and library that wrote it, so a
        // foreign clause name descends in its own declaring scope — the
        // oracle's whole-library `_ImplementsWalker` visits the element,
        // not the spelling.
        let mut frontier: Vec<(StringId, FileId, LibraryId)> = Vec::new();
        let mut seen: rustc_hash::FxHashSet<StringId> = rustc_hash::FxHashSet::default();
        seen.insert(name);
        frontier.push((name, self.file, self.lib));
        let mut steps = 0u32;
        while let Some((head, head_file, head_lib)) = frontier.pop() {
            if steps >= 256 {
                return false;
            }
            steps += 1;
            let Some((container, owner)) = resolve_container(
                self.graph,
                self.namespaces,
                self.index,
                head_lib,
                head_file,
                head,
            ) else {
                continue;
            };
            let owner_lib = self.graph.library_of_file(owner).unwrap_or(head_lib);
            let edges = container
                .superclass
                .iter()
                .chain(container.with_clause.iter())
                .chain(container.implements_clause.iter())
                .chain(container.on_clause.iter());
            for &edge in edges {
                if edge == name {
                    return true;
                }
                if seen.insert(edge) {
                    frontier.push((edge, owner, owner_lib));
                }
            }
        }
        false
    }

    /// The same-head argument walk (`_interfaceArguments`,
    /// subtype.dart 44-82): per position the declared variance decides,
    /// and the element model's default for an unmarked parameter is
    /// COVARIANT (`TypeParameterElementImpl.variance` = `_variance ??
    /// Variance.covariant`, element.dart 11559-11561 — the summary
    /// builder sets a variance only for a written keyword,
    /// variance_builder.dart 244-256), so every position the model
    /// cannot spell a marker for walks the covariant leg
    /// `strict(subArgument, superArgument)`. The leg is the strict
    /// subtype ask — never the assignable slot, whose legacy `Null` and
    /// top leniencies over-fire (`List<Null> is List<Object>`: `Null
    /// <: Object` is false, Left Null — the oracle is silent). The
    /// top-type relief rides the recursion (`int <: List<dynamic>`
    /// through Right Top). A bare side materializes the all-dynamic
    /// raw face (`C` reads `C<dynamic>` — the oracle's raw
    /// instantiation), and an arity the container cannot answer
    /// under-reports through the head walk. Explicit `inout` markers
    /// (the variance experiment's invariant positions) stay unmodeled —
    /// the covariant default over-proves those rare rows.
    fn same_head_args(
        &self,
        head: StringId,
        sub_args: ArgListId,
        sup_args: ArgListId,
        sub: Ty,
        sup: Ty,
        depth: u32,
    ) -> bool {
        if depth >= Self::SUBTYPE_DEPTH {
            return false;
        }
        let arity = match resolve_container(
            self.graph,
            self.namespaces,
            self.index,
            self.lib,
            self.file,
            head,
        ) {
            Some((container, _)) => container.type_params as usize,
            None => return self.name_subtype(sub, sup),
        };
        let args_of = |args: ArgListId| -> Option<Vec<Ty>> {
            if args == ArgListId::EMPTY {
                return Some(vec![Ty::Dynamic; arity]);
            }
            Some(self.boundary.args.borrow().get(args).to_vec())
        };
        let (Some(sub_list), Some(sup_list)) = (args_of(sub_args), args_of(sup_args)) else {
            return self.name_subtype(sub, sup);
        };
        // A written argument count the container cannot take reads as the
        // recovered `C<Invalid>` interface (the wrong-number-of-type-
        // arguments face, #2380's recv_arity probes): every argument
        // comparison is 0c-vacuous (subtype.dart 72-74 — an `InvalidType`
        // on either side answers `true`), so the same-head question is
        // constant-true on BOTH polarity directions — a bare side is the
        // raw all-dynamic face, not an invalid one, and stays out.
        if (sub_args != ArgListId::EMPTY && sub_list.len() != arity)
            || (sup_args != ArgListId::EMPTY && sup_list.len() != arity)
        {
            return true;
        }
        if sub_list.len() != sup_list.len() {
            return false;
        }
        sub_list
            .iter()
            .zip(sup_list.iter())
            .all(|(&sub_arg, &sup_arg)| self.strict_subtype(sub_arg, sup_arg, depth + 1))
    }

    /// The source span of the token group `open` opened — `open` through its
    /// matching closer, resolved through the scanner's group table
    /// (`TokenTable::matching_end`) rather than through an enclosing frame's
    /// end token, which also covers a trailing `||`/`&&` operand or postfix
    /// (`case (a, b) || (c, d):`). `None` for an unclosed group (recovery) or
    /// a parser-inserted opener; the caller keeps its own fallback span.
    pub(crate) fn group_span(&self, open: Token) -> Option<SourceRange> {
        let close = self.tokens.get(self.tokens.matching_end(open.id as usize)?)?;
        // The closer is one UTF-16 unit (`)`/`]`/`}`).
        Some(SourceRange {
            offset: open.range.offset,
            length: (close.utf16_offset + 1).saturating_sub(open.range.offset),
        })
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

    /// The bare name's local-function tear-off signature (#2842),
    /// innermost frame first ([`Self::capture_local_fn_tearoff`]'s
    /// capture). A plain local of the same spelling — a variable, a
    /// parameter, anything in `locals` without a capture — shadows the
    /// outer spelling and answers `None` (the read is that local's own
    /// value, `typed_local`'s face), the resolution precedence
    /// `resolve_bare` applies.
    pub(crate) fn local_fn_sig(&self, name: StringId) -> Option<&FnVarSig> {
        for frame in self.frames.iter().rev() {
            if let Some(sig) = frame.local_fn_sigs.get(&name) {
                return Some(sig);
            }
            if frame.locals.contains(&name) {
                return None;
            }
        }
        None
    }

    /// The `is`/`is!` operand's resolved static type
    /// (`leftNode.typeOrThrow`, best_practices_verifier.dart:1011): the
    /// nearest operand row's push-time snapshot, a bare local's
    /// declared/inferred type the frame bound (`var self = this` reads
    /// `self`'s inferred `this` type, not the row's `Unknown` —
    /// mutation plays no part in the verdict), or a bare read's
    /// tear-off face ([`Self::operand_tear_off_ty`] — a function,
    /// method, getter, or field spelled as its own value carries its
    /// declared face as the static type). [`Ty::Unknown`] when no
    /// operand row exists or nothing resolves — the every-check
    /// suppresses sentinel the callers gate on.
    pub(crate) fn type_check_operand_ty(&self) -> Ty {
        let Some(operand) = self.operand_row() else {
            return Ty::Unknown;
        };
        // A completed UNKNOWN-send's static type is its result face —
        // bare, member, or value shape alike (`MethodInvocationImpl.
        // staticType` — `(Class).toString() is String`, `doNotReturn()
        // is int`): the invoked callee's declared return, the same face
        // the postfix junctions read ([`Self::operand_static_ty`]). The
        // dispatch rides above the bare-read faces: a send is never its
        // callee's tear-off, and a bare send row (`Bare(Plain)`) would
        // otherwise die in the declared-local faces unknown. A typed row
        // keeps its own push-time answer first — the exact behavior every
        // settled face pinned. A `?.` short re-arms the nullability the
        // merge could not join — the member offset latch
        // [`Self::null_comparison`] reads.
        if operand.ty != Ty::Unknown {
            operand.ty
        } else if operand.intent == Intent::Invoke {
            return self.value_operand_ty(operand).unwrap_or(Ty::Unknown);
        } else if let RowShape::Bare(BareInfo::Plain { ty }) = &operand.shape {
            if *ty != Ty::UNKNOWN {
                *ty
            } else if let Some(ty) = self.typed_local(operand.name) {
                ty
            } else {
                // The parenthesized FunctionReference (`((C.new)<int>)`) is
                // a `Bare(Plain)` row at the `)` (end_parenthesized_
                // expression's push) with an opaque type — the constructor
                // tear-off face answers it (the type-argument group's
                // entry keys at this row).
                self.operand_tear_off_ty(operand)
                    .or_else(|| self.ctor_tear_off_ty(operand))
                    .unwrap_or(Ty::Unknown)
            }
        } else {
            // The constructor-selector face precedes the member walk: a
            // ctor selector (`C.new`, `C.constr`) also sits in the member
            // table, whose signature face lacks the class return head and
            // the receiver instantiation — the declared-constructor table
            // owns the verdict, and every non-ctor selector keeps the
            // member walk (the ctor gates reject it first).
            self.ctor_tear_off_ty(operand)
                .or_else(|| self.member_tear_off_ty(operand))
                .or_else(|| self.value_operand_ty(operand))
                // The untyped row's own static type (the `C is T` TypeLiteral
                // face, the prefixed `p.C` read, the `this`/`super` keyword
                // receiver): the row shape alone answers it — a `C()` send
                // never reaches here (the Invoke arm owns it), and a `Value`
                // row's literal faces already ran inside `value_operand_ty`.
                .or_else(|| {
                    let ty = self.untyped_operand_ty(operand);
                    (ty != Ty::UNKNOWN).then_some(ty)
                })
                .unwrap_or(Ty::Unknown)
        }
    }

    /// The value-operand faces at an `is` check (#2854) — the operand
    /// rows that are no declaration read: a completed send's static
    /// type, an untyped collection literal's inferred element type, and
    /// a closure literal's written-signature function type. `None` keeps
    /// the every-check suppress ([`Ty::Unknown`]).
    fn value_operand_ty(&self, operand: &Row) -> Option<Ty> {
        // The completed send's result face: the invoked callee's declared
        // return ([`Self::operand_static_ty`]), filtered to a decidable
        // answer.
        if operand.intent == Intent::Invoke {
            let ty = self.operand_static_ty(operand);
            if ty == Ty::UNKNOWN || self.is_invalid_ty(ty) {
                return None;
            }
            return Some(if self.null_aware_members.contains(&operand.range.offset) {
                Self::nullable_form(ty)
            } else {
                ty
            });
        }
        if !matches!(operand.shape, RowShape::Value) {
            return None;
        }
        self.collection_operand_ty(operand)
            .or_else(|| self.closure_operand_ty(operand))
    }

    /// The untyped collection literal's inferred static type at an `is`
    /// operand (`[] is List`, `[1, 2, 3] is List` — `ListLiteralImpl.
    /// staticType` / `SetOrMapLiteralImpl.staticType`): the no-context
    /// resolution [`Self::literal_return_ty`] documents, now with the
    /// element leaves' row types as the arguments — `[1, 2, 3]` reads
    /// `List<int>` (each primitive leaf pushes its own typed row), a
    /// leaf without an exact row type stays `dynamic` (the under-report
    /// direction: `dynamic` decides every provable ask and refuses every
    /// unprovable one, so `[1, 'a']` against `List<Object>` fires and
    /// against `List<int>` stays informative exactly like the oracle's
    /// `List<Object>` inference). The literal's row sits at its close
    /// bracket, latched by the listener's `untyped_literals` key — a
    /// miss is no literal at all.
    fn collection_operand_ty(&self, operand: &Row) -> Option<Ty> {
        let (kind, open, close) = self.literal_window(operand)?;
        let bytes = self.source.as_bytes();
        let (list, set, map) = (
            self.collection_heads[0],
            self.collection_heads[1],
            self.collection_heads[2],
        );
        let dynamic = Ty::Dynamic;
        let (head, args) = match kind {
            // The empty `{}` literal's default (`literal_return_ty`'s
            // map arm).
            UntypedLiteral::EmptyBraces => (map, self.boundary.intern_list(&[dynamic, dynamic])),
            UntypedLiteral::List => (
                list,
                self.boundary
                    .intern_list(&[self.least_leaf_ty(bytes, open + 1, close)]),
            ),
            UntypedLiteral::SetElements => (
                set,
                self.boundary
                    .intern_list(&[self.least_leaf_ty(bytes, open + 1, close)]),
            ),
            UntypedLiteral::MapEntries => {
                let (key, value) = self.entry_leaf_tys(bytes, open + 1, close);
                (map, self.boundary.intern_list(&[key, value]))
            }
        };
        Some(Ty::Named {
            name: head,
            nullable: false,
            args,
        })
    }

    /// The untyped literal's bracket window `(kind, open, close)` — the
    /// byte offsets of its own brackets. The row/key rides the close
    /// bracket, except the empty square literal's, which sits at the `[`
    /// itself (the parser hands the empty list's right as its own `[`).
    /// A key miss is no literal at all; a byte that is neither bracket,
    /// or a walk-back landing on a foreign opener (a desynced depth — a
    /// bracket inside a leaf string), is no window either face reads.
    fn literal_window(&self, operand: &Row) -> Option<(UntypedLiteral, usize, usize)> {
        let kind = *self.untyped_literals.get(&operand.range.offset)?;
        let (open_b, close_b) = match kind {
            UntypedLiteral::List => (b'[', b']'),
            UntypedLiteral::EmptyBraces
            | UntypedLiteral::SetElements
            | UntypedLiteral::MapEntries => (b'{', b'}'),
        };
        let bytes = self.source.as_bytes();
        let mut close = self.byte_of(operand.range.offset);
        match bytes.get(close) {
            Some(&b) if b == close_b => {}
            Some(&b) if b == open_b => close = matching_bracket_fwd(bytes, close)?,
            _ => return None,
        }
        let open = matching_bracket_back(bytes, close)?;
        (bytes.get(open) == Some(&open_b)).then_some((kind, open, close))
    }

    /// The untyped literal's own bracket span (UTF-16, the open bracket
    /// through the close bracket) — the anchor a wrapper-free literal
    /// reports at, `const` aside.
    pub(crate) fn untyped_literal_span(&self, operand: &Row) -> Option<SourceRange> {
        let (_, open, close) = self.literal_window(operand)?;
        let (start, end) = (self.utf16_of(open), self.utf16_of(close + 1));
        Some(SourceRange {
            offset: start,
            length: end - start,
        })
    }

    /// The untyped collection literal's exact upward-inferred static type
    /// under a context its own head cannot match (#3424 — upstream Dart
    /// SDK source, pkg/analyzer/lib/src/dart/resolver/
    /// typed_literal_resolver.dart `_inferListTypeUpwards` / `_toSetType`
    /// / `_toMapType`: the failed downward constraint leaves the leaves'
    /// static types the only bounds on the type arguments). `family` is
    /// the head the literal resolves to — `List`, `Set`, or `Map`; an
    /// empty `{}` takes its side from the context, which only the caller
    /// reads — and every argument is the least upper bound of its leaves,
    /// `dynamic` when there are none (`{}` reads `Set<dynamic>` or
    /// `Map<dynamic, dynamic>`). Unlike [`Self::collection_operand_ty`]
    /// (the `is` face, whose doubt folds to `dynamic` — the under-report
    /// direction there), the assignment face prints this type in its
    /// message, so doubt refuses: a leaf no single row spans, an unknown
    /// leaf type, or a join [`Self::exact_leaf_lub`] cannot spell answers
    /// `None`.
    pub(crate) fn collection_literal_ty(&self, operand: &Row, family: StringId) -> Option<Ty> {
        let (_, open, close) = self.literal_window(operand)?;
        let bytes = self.source.as_bytes();
        let mut segments = value_comma_segments(bytes, open + 1, close);
        // A trailing comma closes the last element: its empty tail (the
        // whole window of an empty literal) is no leaf.
        if segments
            .last()
            .is_some_and(|&(start, end)| trim_value_ws(bytes, start, end) >= end)
        {
            segments.pop();
        }
        let args = if family == self.collection_heads[2] {
            let mut keys = Vec::with_capacity(segments.len());
            let mut values = Vec::with_capacity(segments.len());
            for (start, end) in segments {
                let colon = value_top_level_colon(bytes, start, end)?;
                keys.push(self.exact_leaf_ty(bytes, start, colon)?);
                values.push(self.exact_leaf_ty(bytes, colon + 1, end)?);
            }
            vec![self.exact_leaf_lub(&keys)?, self.exact_leaf_lub(&values)?]
        } else {
            let leaves = segments
                .into_iter()
                .map(|(start, end)| self.exact_leaf_ty(bytes, start, end))
                .collect::<Option<Vec<_>>>()?;
            vec![self.exact_leaf_lub(&leaves)?]
        };
        Some(Ty::Named {
            name: family,
            nullable: false,
            args: self.boundary.intern_list(&args),
        })
    }

    /// One leaf's exact static type: the row spanning exactly the trimmed
    /// segment — the leaf expression's own node row (the last one pushed
    /// for that span is the outermost) — read through
    /// [`Self::operand_static_ty`]. A set or map literal's element rows
    /// stay live; a list literal drains its own into the deferred pass-2
    /// queue at its close, so both stacks are scanned newest-first, each
    /// stopping below the leaf's start. A segment no single row spans (a
    /// spread, a control-flow or null-aware element, an operator
    /// operand), a nested untyped literal (its type is this face's own
    /// question, never the `is` face's `dynamic`-folded answer), and an
    /// unknown type answer `None`.
    fn exact_leaf_ty(&self, bytes: &[u8], start: usize, end: usize) -> Option<Ty> {
        let end = end.min(bytes.len());
        let start = trim_value_ws(bytes, start, end);
        let end = start + bytes[start..end].trim_ascii_end().len();
        if start >= end {
            return None;
        }
        // A spread element's leaf is its operand — the `...`/`...?` prefix
        // is not part of the operand's own row span, so the exact match
        // below starts after it. The element face the leaf contributes is
        // the operand's ELEMENT type (`_computeElementType`'s iterable
        // arm), which [`Self::spread_element_ty`] reads.
        let spread_prefix = bytes[start..end].starts_with(b"...");
        let operand_start = if spread_prefix {
            let skip = 3 + usize::from(bytes.get(start + 3) == Some(&b'?'));
            trim_value_ws(bytes, start + skip, end)
        } else {
            start
        };
        let (s16, e16) = (self.utf16_of(operand_start), self.utf16_of(end));
        let exact = |row: &Row| {
            row.range.offset == s16
                && row.range.end() == e16
                && !matches!(row.shape, RowShape::TypeUse(..))
        };
        let row = self
            .rows
            .iter()
            .rev()
            .take_while(|row| row.range.offset >= s16)
            .find(|row| exact(row))
            .or_else(|| {
                self.deferred_miss_rows
                    .iter()
                    .rev()
                    .take_while(|row| row.range.offset >= s16)
                    .find(|row| exact(row))
            })?;
        if self.untyped_literals.contains_key(&row.range.offset) {
            return None;
        }
        if spread_prefix {
            return self.spread_element_ty(row);
        }
        let ty = self.operand_static_ty(row);
        (ty != Ty::UNKNOWN).then_some(ty)
    }

    /// The leaves' least upper bound where this layer spells it exactly
    /// (upstream `TypeSystem.leastUpperBound`, the join the upward
    /// inference folds its lower bounds to): no leaf leaves the argument
    /// unconstrained — `dynamic`; a `dynamic` leaf absorbs the join; the
    /// `Null` legs re-arm `?` on the join of the non-null forms. One form
    /// keeps itself, an `Object` form absorbs every other, `int` /
    /// `double` / `num` mixes join at `num`, and distinct `dart:core`
    /// finals (`int`, `double`, `num`, `bool`, `String`, `Type`) share no
    /// superinterface below `Object`. Every other mix — user classes whose
    /// shared supertype needs the depth-ranked walk, distinct generic
    /// instantiations, function and record shapes — and a `void`,
    /// `Never`, or unknown leaf answer `None`.
    fn exact_leaf_lub(&self, leaves: &[Ty]) -> Option<Ty> {
        if leaves.is_empty() || leaves.contains(&Ty::Dynamic) {
            return Some(Ty::Dynamic);
        }
        let mut nullable = false;
        let mut forms: Vec<Ty> = Vec::new();
        for &leaf in leaves {
            match leaf {
                Ty::Null => nullable = true,
                Ty::Named {
                    nullable: leaf_nullable,
                    ..
                }
                | Ty::Record {
                    nullable: leaf_nullable,
                    ..
                }
                | Ty::Function {
                    nullable: leaf_nullable,
                    ..
                } => {
                    nullable |= leaf_nullable;
                    let form = Self::non_null_form(leaf);
                    if !forms.contains(&form) {
                        forms.push(form);
                    }
                }
                _ => return None,
            }
        }
        let object = Ty::named(self.intern("Object"));
        let join = match forms.as_slice() {
            [] => return Some(Ty::Null),
            [one] => *one,
            _ if forms.contains(&object) => object,
            _ => {
                let heads = forms
                    .iter()
                    .map(|form| match *form {
                        Ty::Named {
                            name,
                            args: ArgListId::EMPTY,
                            ..
                        } => Some(name),
                        _ => None,
                    })
                    .collect::<Option<Vec<_>>>()?;
                let ids = &self.members.primitive_ids;
                let type_head = self.intern("Type");
                if heads.iter().all(|head| ids[..3].contains(head)) {
                    Ty::named(ids[2])
                } else if heads
                    .iter()
                    .all(|head| ids.contains(head) || *head == type_head)
                {
                    object
                } else {
                    return None;
                }
            }
        };
        Some(if nullable {
            Self::nullable_form(join)
        } else {
            join
        })
    }

    /// The element leaves' least row type: each top-level comma
    /// segment's own row (the push-ordered row the leaf completed with —
    /// a primitive literal's declared primitive, a bare local's declared
    /// type, a typed nested literal's face), folded to the least type
    /// the name-level lattice can stand ([`Self::least_leaf_ty_from`]).
    /// A segment without an exact leaf row forces `dynamic`.
    fn least_leaf_ty(&self, bytes: &[u8], start: usize, end: usize) -> Ty {
        let leaves: Vec<Option<Ty>> = value_comma_segments(bytes, start, end)
            .into_iter()
            .map(|(s, e)| self.leaf_row_ty(bytes, s, e))
            .collect();
        if leaves.iter().any(|leaf| leaf.is_none()) {
            return Ty::Dynamic;
        }
        self.least_leaf_ty_from(leaves.into_iter().flatten().collect())
    }

    /// The map entry leaves' key/value least types: each segment's
    /// top-level colon splits the sides, a colonless leaf (a spread, a
    /// control-flow element) keeps both sides `dynamic`.
    fn entry_leaf_tys(&self, bytes: &[u8], start: usize, end: usize) -> (Ty, Ty) {
        let mut keys = Vec::new();
        let mut values = Vec::new();
        for (s, e) in value_comma_segments(bytes, start, end) {
            match value_top_level_colon(bytes, s, e) {
                Some(colon) => {
                    keys.push(self.leaf_row_ty(bytes, s, colon));
                    values.push(self.leaf_row_ty(bytes, colon + 1, e));
                }
                None => {
                    keys.push(None);
                    values.push(None);
                }
            }
        }
        let fold = |tys: Vec<Option<Ty>>| -> Ty {
            if tys.iter().any(|ty| ty.is_none()) {
                return Ty::Dynamic;
            }
            self.least_leaf_ty_from(tys.into_iter().flatten().collect())
        };
        (fold(keys), fold(values))
    }

    /// The leaf's own row type: the push-ordered row the segment
    /// completed with, its push-time type only (a [`Ty::Unknown`] row —
    /// a send, an untyped read — is no exact face and answers `None`).
    /// Type-use rows are annotation positions, never leaves. The
    /// listener's `leaf_completion_row` scan at name level. A segment
    /// no row covers answers its own literal spelling
    /// ([`Self::literal_leaf_ty`]) — the primitive literals push no
    /// row.
    fn leaf_row_ty(&self, bytes: &[u8], start: usize, end: usize) -> Option<Ty> {
        let start = trim_value_ws(bytes, start, end.min(bytes.len()));
        let end = end.min(bytes.len());
        if start >= end {
            return None;
        }
        let (s16, e16) = (self.utf16_of(start), self.utf16_of(end));
        // A leaf's own row may live on either stack: the enclosing list
        // literal drains its element rows into the deferred pass-2 queue
        // at close (`handle_literal_list`'s drain), so both stacks are
        // scanned newest-first, each stopping below the leaf's start. A
        // spread element (`...expr`) contributes the spread operand's
        // ELEMENT type (upstream `typed_literal_resolver.dart`
        // `_computeElementType`'s iterable arm) — the operand's own row
        // reads the iterable whole, never the element. The operand's row
        // sits inside the leaf span (the `...` prefix is not part of it),
        // so the span scan finds it.
        let spread = bytes[start..end].starts_with(b"...");
        let in_span = |row: &Row| {
            row.range.offset >= s16
                && row.range.end() <= e16
                && !matches!(row.shape, RowShape::TypeUse(..))
        };
        let row = self
            .rows
            .iter()
            .rev()
            .find(|row| in_span(row))
            .or_else(|| self.deferred_miss_rows.iter().rev().find(|row| in_span(row)))?;
        if spread {
            // The spread's element face: `Iterable`'s first argument where
            // the operand's head reaches it, `dynamic` where it does not
            // (a `Map`, a non-collection, an unspellable shape all read
            // `dynamic` upstream — `_computeElementType`'s else arm).
            return self.spread_element_ty(row);
        }
        let ty = row.ty;
        (ty != Ty::UNKNOWN && ty != Ty::Dynamic)
            .then_some(ty)
            .or_else(|| self.literal_leaf_ty(bytes, start, end))
    }

    /// The spread element's inferred type (`typed_literal_resolver.dart`
    /// `_computeElementType`'s spread arm): `Iterable`'s first argument
    /// where the operand's head reaches `Iterable`/`List`/`Set` through
    /// its declared superchain, `dynamic` where it does not. A `Map`
    /// operand, a non-collection operand, and an unspellable head all read
    /// `dynamic` — the oracle's `else` arm, never a more specific type.
    /// The operand's own row is the spread's value row (the `...` prefix
    /// is not part of it); a `Named` head with a resolved `Iterable` first
    /// argument answers that argument.
    fn spread_element_ty(&self, row: &Row) -> Option<Ty> {
        let Ty::Named { name, args, .. } = self.operand_static_ty(row) else {
            return None;
        };
        let iterable_heads = [
            self.collection_heads[3],
            self.collection_heads[0],
            self.collection_heads[1],
        ];
        let first = || {
            self.boundary
                .pool()
                .get(args)
                .first()
                .copied()
                .filter(|ty| *ty != Ty::UNKNOWN)
        };
        if iterable_heads.contains(&name) {
            return first().or(Some(Ty::Dynamic));
        }
        // A user head reaching `Iterable` through `extends`/`with`/
        // `implements` answers the same first argument — the clause's
        // written arguments live only in source, so the first-position
        // substitution is exact for every single-parameter wrapper chain.
        let mut queue: Vec<StringId> = vec![name];
        let mut seen: FxHashSet<StringId> = FxHashSet::from_iter([name]);
        while let Some(current) = queue.pop() {
            let Some((decl, _)) = crate::library::resolve_container(
                self.graph,
                self.namespaces,
                self.index,
                self.lib,
                self.file,
                current,
            ) else {
                continue;
            };
            let supers = decl
                .superclass
                .into_iter()
                .chain(decl.with_clause.iter().copied())
                .chain(decl.implements_clause.iter().copied());
            for next in supers {
                if iterable_heads.contains(&next) {
                    return first().or(Some(Ty::Dynamic));
                }
                if seen.insert(next) {
                    queue.push(next);
                }
            }
        }
        Some(Ty::Dynamic)
    }

    /// The leaf segment's own literal spelling, byte-read (`1` → `int`,
    /// `1.5` / `1e3` → `double`, `true` / `false` → `bool`, a quoted
    /// literal → `String`): the element faces the oracle infers from the
    /// leaf's own node, which pushes no row here. Anything else — an
    /// identifier, a send, a spread — answers `None` (the `dynamic`
    /// under-report).
    fn literal_leaf_ty(&self, bytes: &[u8], start: usize, end: usize) -> Option<Ty> {
        let b = &bytes[start..end.min(bytes.len())];
        let ids = &self.members.primitive_ids;
        match b {
            [b'\'', .., b'\''] | [b'"', .., b'"'] => return Some(Ty::named(ids[4])),
            [b't', b'r', b'u', b'e'] | [b'f', b'a', b'l', b's', b'e'] => {
                return Some(Ty::named(ids[3]));
            }
            _ => {}
        }
        let (digits, hex) = match b.strip_prefix(b"0x").or_else(|| b.strip_prefix(b"0X")) {
            Some(rest) => (rest, true),
            None => (b, false),
        };
        let first = *digits.first()?;
        if !first.is_ascii_digit() {
            return None;
        }
        let mut is_double = false;
        let mut prev = first;
        for &c in digits {
            let ok = match c {
                b'0'..=b'9' | b'_' => true,
                b'a'..=b'f' | b'A'..=b'F' if hex => true,
                b'.' if !hex => {
                    is_double = true;
                    true
                }
                b'e' | b'E' if !hex => {
                    is_double = true;
                    true
                }
                b'+' | b'-' => matches!(prev, b'e' | b'E'),
                _ => false,
            };
            if !ok {
                return None;
            }
            prev = c;
        }
        Some(Ty::named(if is_double { ids[1] } else { ids[0] }))
    }

    /// The collected leaf types' least face: one exact type keeps
    /// itself, `int` mixed with `double` lifts to `num`, unrelated
    /// non-nullable interface faces lift to bare `Object` (the official
    /// LUB of the oracle's list inference — every target the lifted list
    /// folds, this folds), everything else — a nullable leaf (whose lift
    /// rides the nullability union this face does not recompute),
    /// `Null` leaves, function/record faces — keeps `dynamic` (the
    /// under-report).
    fn least_leaf_ty_from(&self, tys: Vec<Ty>) -> Ty {
        let int = Ty::named(self.members.primitive_ids[0]);
        let double = Ty::named(self.members.primitive_ids[1]);
        let mut first: Option<Ty> = None;
        let mut diverse = false;
        let mut other_face = false;
        for ty in tys.iter().copied() {
            if ty == Ty::Dynamic {
                return Ty::Dynamic;
            }
            if ty != int && ty != double {
                other_face = true;
            }
            match first {
                None => first = Some(ty),
                Some(prev) if prev != ty => diverse = true,
                Some(_) => {}
            }
        }
        // The unrelated-face lift: bare `Object` folds exactly where the
        // oracle's inferred `List<Object>` folds (every non-nullable
        // interface face subtypes `Object`, so the argument legs agree in
        // both directions); a leaf outside that shape answers no
        // recomputable LUB here.
        let object_lift = tys.iter().all(|ty| {
            matches!(
                ty,
                Ty::Named {
                    nullable: false,
                    ..
                }
            )
        });
        match first {
            None => Ty::Dynamic,
            Some(ty) if !diverse => ty,
            // The int/double pair is the one diverse fold the lattice
            // names exactly (`num`).
            Some(_) if !other_face => Ty::named(self.members.primitive_ids[2]),
            Some(_) if object_lift => Ty::named(self.intern("Object")),
            Some(_) => Ty::Dynamic,
        }
    }

    /// The closure literal's function type at an `is` operand
    /// (`(List x) {} is Function`, `((int x) {}) is void Function(int)`
    /// — `FunctionExpressionImpl.staticType`): the written formal
    /// annotations as the parameter types (an unannotated formal keeps
    /// the official element's `dynamic` column, the #2813 convention
    /// [`Self::official_fn_face`] normalizes), the return `dynamic` —
    /// the block body's inferred return is a flow face the row model
    /// does not carry, and `dynamic` decides every return-top ask and
    /// refuses every mismatching one, so the pair provability answers
    /// exactly where the oracle's inferred return would. The plain
    /// positional shapes only: a named section, a generic literal
    /// (whose own parameters the declared-face model does not carry),
    /// an arrow body (its row closes at the body expression, not `}`),
    /// or an annotation richer than a head token refuses the whole face
    /// — the under-report posture every operand face shares.
    fn closure_operand_ty(&self, operand: &Row) -> Option<Ty> {
        if operand.intent != Intent::Read {
            return None;
        }
        let bytes = self.source.as_bytes();
        let close = self.byte_of(operand.range.offset);
        if bytes.get(close) != Some(&b'}') {
            return None;
        }
        // The body's `{`, then the formal group's `)` — only `async` /
        // `sync*` modifiers and whitespace may sit between — then the
        // group's `(`. A `<`-group head before the `(` is a generic
        // literal, refused.
        let body_open = matching_bracket_back(bytes, close)?;
        if bytes.get(body_open) != Some(&b'{') {
            return None;
        }
        let mut paren_close = body_open;
        while paren_close > 0 {
            let b = bytes[paren_close - 1];
            if b.is_ascii_whitespace() || b == b'*' || b.is_ascii_alphanumeric() || b == b'_' {
                paren_close -= 1;
            } else {
                break;
            }
        }
        if paren_close == 0 || bytes[paren_close - 1] != b')' {
            return None;
        }
        let paren_close = paren_close - 1;
        let paren_open = matching_bracket_back(bytes, paren_close)?;
        if bytes.get(paren_open) != Some(&b'(') {
            return None;
        }
        let mut head = paren_open;
        while head > 0 && bytes[head - 1].is_ascii_whitespace() {
            head -= 1;
        }
        // A `<...>` group head is a generic literal: its own parameters
        // ride no declared face here, refused.
        if head > 0 && bytes[head - 1] == b'>' {
            return None;
        }
        if (paren_open + 1..paren_close).any(|i| bytes[i] == b'{') {
            return None;
        }
        // The group's written formals: each top-level comma segment's
        // trailing identifier is the formal's name, the head before it
        // the written annotation. An annotation is a single head token
        // (its type-use row starts the segment and only `?`/whitespace
        // separates it from the name) — anything richer refuses. A
        // trailing comma's empty tail segment is legal and skipped.
        let mut segments = value_comma_segments(bytes, paren_open + 1, paren_close);
        let tail_empty = match segments.last() {
            Some((s, e)) => trim_value_ws(bytes, *s, *e) >= *e,
            None => false,
        };
        if tail_empty {
            segments.pop();
        }
        let mut positional = Vec::new();
        for (s, e) in segments {
            let seg_end = value_default_cut(bytes, s, e);
            let seg_start = trim_value_ws(bytes, s, seg_end);
            if seg_start >= seg_end {
                return None;
            }
            let mut name_end = seg_end;
            while name_end > seg_start {
                let b = bytes[name_end - 1];
                if b.is_ascii_alphanumeric() || b == b'_' || b == b'$' {
                    name_end -= 1;
                } else {
                    break;
                }
            }
            if name_end == seg_end {
                // No identifier tail (`covariant`-only residue, a
                // malformed group): refuse.
                return None;
            }
            let head_start = trim_value_ws(bytes, seg_start, name_end);
            if head_start >= name_end {
                // No written annotation: the official element's
                // `dynamic` column.
                positional.push(Ty::Dynamic);
                continue;
            }
            let (s16, e16) = (self.utf16_of(head_start), self.utf16_of(name_end));
            let Some(&(range, name)) = self
                .type_uses
                .iter()
                .find(|(r, _)| r.offset == s16 && r.end() <= e16)
            else {
                return None;
            };
            // Only `?` and whitespace may ride between the head token
            // and the formal's name.
            let mut cursor = self.byte_of(range.end());
            let mut nullable = false;
            while cursor < name_end {
                match bytes[cursor] {
                    b if b.is_ascii_whitespace() => cursor += 1,
                    b'?' if !nullable => {
                        nullable = true;
                        cursor += 1;
                    }
                    _ => return None,
                }
            }
            // An unresolved annotation head is the oracle's InvalidType
            // formal — the closure's own static type goes invalid with
            // it and the verdict stays informative (`named_type_is_
            // type`'s `_resolveToElement` gate).
            if !self.named_type_is_type(name) {
                return None;
            }
            positional.push(Ty::Named {
                name,
                nullable,
                args: crate::model::ArgListId::EMPTY,
            });
        }
        let positional: Vec<Ty> = positional
            .into_iter()
            .map(|ty| self.fold_alias_ty(ty))
            .collect();
        Some(
            self.boundary
                .intern_function_ty(0, Ty::Dynamic, &positional, &[], false),
        )
    }

    /// The tear-off's static type: the signature as the interned
    /// non-nullable function type the value read carries (the top-level
    /// and local-function faces share the conversion). Each signature
    /// piece folds through a type alias first — the tear-off's static
    /// type is the resolved face (`C foo()`'s tear-off reads `D
    /// Function()` under `typedef D C();`, the alias-unfolding
    /// `subtype.dart` runs before any structural ask), the same
    /// convention `bind_declared_type` applies to the frame's declared
    /// types.
    fn tear_off_fn_ty(&self, sig: &FnVarSig, type_params: &[(StringId, Option<Ty>)]) -> Ty {
        let named: Vec<(StringId, Ty)> = sig
            .named
            .iter()
            .map(|(name, ty, _)| (*name, self.fold_alias_ty(*ty)))
            .collect();
        let positional: Vec<Ty> = sig
            .positional
            .iter()
            .map(|ty| self.fold_alias_ty(*ty))
            .collect();
        // The declaration's own parameter group rides the shape (#2878):
        // the generic tear-off's display spells it (`void foo<T>(T a)`
        // reads `void Function<T>(T)`). The names travel beside the
        // signature — `FnVarSig::params` feeds the subtype and inference
        // verdicts, which keep their count-only posture; this face is
        // display-only. A `Dynamic` bound is the unbounded face.
        //
        // The return fold runs BEFORE the pool borrow: `fold_alias_ty`
        // descends into `instantiated_head_ty`, which interns through the
        // same `args` RefCell — holding the borrow across the call
        // re-enters `borrow_mut` and panics (#2992).
        let return_ty = self.fold_alias_ty(sig.return_ty);
        let mut args = self.boundary.args.borrow_mut();
        let ty = args.function_generic(type_params, return_ty, &positional, &named, false);
        drop(args);
        ty
    }

    /// The bare-read tear-off face at an `is` operand (`f0 is F0<int>`
    /// over a top-level `int f0(...)`, `m0 is F0<T>` over an instance
    /// `int m0(...)` — the operand's static type is the callable's
    /// declared signature as a function type, the official
    /// `FunctionType` a tear-off expression carries): a top-level
    /// function answers through [`Engine::tear_off_sig`], an
    /// enclosing-container member through the member walk — a method
    /// interns its signature (generic members stay untyped: the
    /// instantiate-to-bounds freshening is outside the declared-face
    /// model), a getter its return type, a field its declared type.
    /// Value reads only: a completed send's row is the call's operand,
    /// never a tear-off. The under-report direction keeps every
    /// unresolvable spelling at `None`.
    fn operand_tear_off_ty(&self, operand: &Row) -> Option<Ty> {
        if operand.qualified || operand.intent != Intent::Read {
            return None;
        }
        // A frame local shadows any member/top-level spelling (the
        // precedence `resolve_bare` applies): the read is the local's own
        // value, and `typed_local` already answered the plain variables.
        // The one frame-local face that IS a tear-off: a declared local
        // function, whose own value is the function (#2842) — the read's
        // static type is the declaration's own signature as a function
        // type ([`Self::capture_local_fn_tearoff`]'s capture).
        if self.frames.iter().any(|f| f.locals.contains(&operand.name)) {
            // The local capture records its own parameter group — the
            // pooled face carries the names and written bounds the
            // signature unification reads (`lf is Fn` over a local generic
            // fn); a capture with no group keeps the empty face.
            return self
                .local_fn_sig(operand.name)
                .map(|sig| self.tear_off_fn_ty(sig, &sig.params[..]));
        }
        if !matches!(
            operand.shape,
            RowShape::Bare(BareInfo::Plain { .. } | BareInfo::Missing { .. })
        ) {
            return None;
        }
        // The top-level function face (`tear_off_sig`'s value-read
        // gate): the signature interns whole, the declaration's own
        // parameter spellings beside it (#2878).
        if let Some(sig) = self.tear_off_sig(operand) {
            // The declaration's own parameter group rides the shape — names
            // AND written bounds (`fn is Fn` over `fn<T extends num>`
            // unifies the real bound against the typedef's — the
            // `relateTypeParameters` equality leg reads it). The bounds
            // live in the per-executable captures (`TopDecl` carries no
            // bound column); a miss keeps the unbounded (`None`) face.
            let type_params: Vec<(StringId, Option<Ty>)> = self
                .top_level_decl(operand.name)
                .map(|decl| {
                    let bounds = self
                        .exec_bounds
                        .get(&operand.name)
                        .or_else(|| self.method_exec_bounds.get(&operand.name));
                    decl.type_param_names
                        .iter()
                        .copied()
                        .map(|name| {
                            let bound = bounds.and_then(|window| {
                                window
                                    .iter()
                                    .find(|(param, _)| *param == name)
                                    .map(|(_, bound)| *bound)
                            });
                            (name, bound.flatten())
                        })
                        .collect()
                })
                .unwrap_or_default();
            let face = self.tear_off_fn_ty(&sig, &type_params);
            // The same broken-capture gate as the member face: a
            // prefix-sentinel leaf or an `Unknown` return in the captured
            // declaration re-reads the written signature from the
            // source, the captured counts gating the candidate. A
            // type-less formal's `Unknown` is not a trigger.
            if self.needs_recovery(face)
                && let Some(rebuilt) = self.source_top_level_signature(
                    operand.name,
                    sig.positional.len(),
                    sig.named.len(),
                    type_params.len(),
                )
            {
                return Some(rebuilt);
            }
            return Some(face);
        }
        // The enclosing-container member face: the unqualified read of a
        // method/getter/field of `this`'s container (`m0` inside the
        // class). The conversion is the qualified face's ([`Self::
        // member_entry_tear_off_ty`]); a generic method's freshening stays
        // out (the documented under-report posture).
        let (container, _) = self.container_context()?;
        match self.walk_members(container, operand.name) {
            Walk::Found(entry) => self.member_entry_tear_off_ty(&entry),
            _ => None,
        }
    }

    /// The member entry's tear-off face as the interned non-nullable type:
    /// a method's signature as a function type (the formals ride their
    /// declared leaves, the container's own parameters included — the raw
    /// face a `this` read carries), a getter its return type, a field its
    /// declared type. Each piece folds through a type alias first (the
    /// resolved-face convention [`Self::tear_off_fn_ty`] documents). A
    /// generic method carries its own `<T extends B>` group as the
    /// signature's `type_params_named` — upstream's tear-off `staticType`
    /// is the generic `FunctionType` (the `is` verdict's
    /// `relateTypeParameters` then unifies it against a generic alias or
    /// written signature by position); the unbounded face spells
    /// [`Ty::Dynamic`]. The shared conversion of the bare
    /// ([`Self::operand_tear_off_ty`]) and qualified
    /// ([`Self::member_tear_off_ty`]) faces; `None` for every other kind
    /// and for an untyped accessor.
    fn member_entry_tear_off_ty(&self, entry: &MemberEntry) -> Option<Ty> {
        match entry.kind {
            MemberKind::Method => {
                let formals = entry.formals.as_deref()?;
                // A broken annotation capture (a prefix-sentinel leaf
                // anywhere, or an `Unknown` RETURN — the armed walk's
                // `p.T` degradation; a type-less formal's `Unknown` is an
                // honest capture, not a trigger) re-reads the member's
                // own written signature from the source: the return
                // annotation before the name, the formals group after
                // it, both with the honest required/optional/named
                // split. The captured formal COUNTS gate the rebuild —
                // a mis-split parse declines back to the captured face.
                let broken = entry.return_ty == Ty::Unknown
                    || self.needs_recovery(entry.return_ty)
                    || formals
                        .positional
                        .iter()
                        .any(|formal| self.needs_recovery(formal.ty))
                    || formals
                        .named
                        .iter()
                        .any(|formal| self.needs_recovery(formal.ty))
                    || entry
                        .type_param_bounds
                        .iter()
                        .any(|bound| self.needs_recovery(*bound));
                if broken
                    && entry.name_range != MemberEntry::MISSING_NAME_RANGE
                    && let Some((type_params, return_ty, positional, required, named)) =
                        self.source_member_signature(entry)
                    && positional.len() == formals.positional.len()
                    && named.len() == formals.named.len()
                    && type_params.len() == usize::from(entry.type_params)
                {
                    let ty = if type_params.is_empty() {
                        self.boundary.args.borrow_mut().function_required(
                            0,
                            required,
                            return_ty,
                            &positional,
                            &named,
                            false,
                        )
                    } else {
                        self.boundary.args.borrow_mut().function_generic_required(
                            &type_params,
                            required,
                            return_ty,
                            &positional,
                            &named,
                            false,
                        )
                    };
                    let ty = self.boundary.canonical_dotted_ty(ty);
                    return Some(ty);
                }
                let named: Vec<(StringId, Ty)> = formals
                    .named
                    .iter()
                    .map(|formal| (formal.name, self.fold_alias_ty(formal.ty)))
                    .collect();
                let positional: Vec<Ty> = formals
                    .positional
                    .iter()
                    .map(|formal| self.fold_alias_ty(formal.ty))
                    .collect();
                // The armed head a `p.T` return spelling keeps on the
                // member table is the bare prefix (`core.List<core.int>
                // m2` captures `Named{core}` — the same armed face
                // `end_typedef`'s RHS head keeps); the declared face the
                // `is`-verdict's signature legs compare lives only in the
                // written annotation. `name_range` anchors the member's
                // own name token and the backward scan recovers the
                // spelling verbatim (`core.List<core.int>`), which
                // `canonical_dotted_ty` then projects to the same
                // element face the checked side spells.
                let return_ty = match entry.return_ty {
                    Ty::Named {
                        name,
                        args: ArgListId::EMPTY,
                        ..
                    } if !self.strings.resolve(name).contains('.')
                        && matches!(self.resolve_bare(name).1, RefTarget::Prefix)
                        && entry.name_range != MemberEntry::MISSING_NAME_RANGE =>
                    {
                        self.backward_ty_text(entry.name_range.offset)
                            .and_then(|text| self.parse_spelled_ty(&text))
                            .map(|ty| self.fold_alias_ty(self.boundary.canonical_dotted_ty(ty)))
                            .unwrap_or_else(|| self.fold_alias_ty(entry.return_ty))
                    }
                    _ => self.fold_alias_ty(entry.return_ty),
                };
                if entry.type_params == 0 {
                    return Some(self.boundary.intern_function_ty(
                        0,
                        return_ty,
                        &positional,
                        &named,
                        false,
                    ));
                }
                // The generic face: names and declared bounds ride the
                // signature so `generic_signature_subtype` can unify the
                // pair by position (`m5<B extends core.int>` against
                // `F5<int>`'s `B extends core.int`). A nameless or
                // boundless slot keeps the unbounded face.
                let type_params: Vec<(StringId, Option<Ty>)> = entry
                    .type_param_names
                    .iter()
                    .enumerate()
                    .map(|(i, name)| {
                        let bound = entry
                            .type_param_bounds
                            .get(i)
                            .copied()
                            .filter(|b| *b != Ty::Dynamic && *b != Ty::UNKNOWN)
                            .map(|b| self.fold_alias_ty(b));
                        (*name, bound)
                    })
                    .collect();
                if type_params.iter().any(|(name, _)| *name == StringId::NONE) {
                    return None;
                }
                Some(
                    self.boundary
                        .args
                        .borrow_mut()
                        .function_generic(&type_params, return_ty, &positional, &named, false),
                )
            }
            MemberKind::Getter if entry.return_ty != Ty::Unknown => {
                Some(self.fold_alias_ty(entry.return_ty))
            }
            MemberKind::Field if entry.var_ty != Ty::Unknown => {
                Some(self.fold_alias_ty(entry.var_ty))
            }
            _ => None,
        }
    }

    /// The qualified member tear-off face at an `is` operand (`b.foo is
    /// F<bool>` over `var b = new A<bool>()`, `super.remove is
    /// RemoveFunctionType<String, String>`): a value read of a
    /// method/getter/field through a typed, `this`, or `super` receiver
    /// carries the member's declared face as its static type (upstream
    /// `PropertyAccessImpl.staticType` — a method read resolves the
    /// `MethodElement`, whose `type` is the function type). A `Typed`
    /// receiver instantiates the anchor container's own-member face with
    /// the written arguments ([`Engine::instantiate_accessor_static`]'s
    /// read face); `this` and `super` keep the walked face — the member
    /// climb already instantiated the ancestor hops (the application-site
    /// substitution), and a `this` container's own parameters ride in
    /// scope. A generic member stays untyped (the freshening posture
    /// [`Self::operand_tear_off_ty`] documents); a completed send's row
    /// (the `Invoke` flip) is a call, never a tear-off. Every
    /// unresolvable spelling answers `None` (under-report).
    fn member_tear_off_ty(&self, row: &Row) -> Option<Ty> {
        if row.intent != Intent::Read {
            return None;
        }
        let RowShape::Member { recv } = &row.shape else {
            return None;
        };
        let (container, super_walk) = match recv {
            Receiver::Typed { type_name, .. } => (*type_name, false),
            Receiver::This { container, .. } => (*container, false),
            Receiver::Super { container, .. } => (*container, true),
            // A creation result's member read (`C().m`): the walk answers
            // the raw face — the created class's own parameters ride in
            // the table walk, and the folded instance type's substitution
            // stays out (the raw face keeps the nullability the null-aware
            // junctions need; the documented generic under-report).
            Receiver::Created { name, .. } => (*name, false),
            _ => return None,
        };
        let found = if super_walk {
            self.walk_from(container, row.name, true)
        } else {
            self.walk_members(container, row.name)
        };
        let Walk::Found(entry) = found else {
            return None;
        };
        // A generic member keeps its own `<T>` group on the signature face
        // (`member_entry_tear_off_ty` carries it as `type_params_named`);
        // the receiver instantiation's `subst_params` re-interns a
        // function face through the count-only `intern_function_ty`, which
        // would strip those very names the generic-signature unification
        // reads. A generic member therefore keeps its raw face — a formal
        // leaf spelling the member's own `<A>` is a signature variable,
        // never the receiver's `T`, so skipping the container substitution
        // loses only the rare member that mentions both.
        let raw = self.member_entry_tear_off_ty(&entry)?;
        if entry.type_params != 0 {
            return Some(raw);
        }
        match recv {
            Receiver::Typed { .. } => {
                let out = self.instantiate_accessor_static(container, recv, raw);
                (out != Ty::UNKNOWN).then_some(out)
            }
            _ => Some(raw),
        }
    }

    /// The constructor tear-off face at an `is` operand (`C.constr is
    /// C Function()`, `C<int>.new is C<int> Function()`, `io.C.constr`,
    /// `CAlias<int>.id` through the grounded alias's expansion head —
    /// `ConstructorReference.staticType`, the same
    /// `name_tear_off_type` the generic-instantiation resolver assigns
    /// the selector). The merged member row's receiver is a container
    /// face (`Receiver::Container`/`Receiver::PrefixedContainer`), and
    /// the selector resolves through the declared-constructor table —
    /// never the member walk (`ctor_arity_target`'s same pair).
    /// The returned function type's leaves ride the captured formals
    /// (`ctor_formals`), an initializing formal's untyped leaf falling
    /// back to its field's declared type (the same face the creation
    /// arity checks run); the result head instantiates with the
    /// receiver's own written type arguments when a `<...>` group closed
    /// on the head (`C<int>.constr` returns `C<int>`, never `C`).
    /// A selector that names no declared constructor, and every other
    /// receiver face, answers `None` (the documented under-report).
    fn ctor_tear_off_ty(&self, row: &Row) -> Option<Ty> {
        if row.intent != Intent::Read {
            return None;
        }
        // The instantiated parenthesized tear-off (`((C.new)<int>)` — the
        // official FunctionReference over a ParenthesizedExpression whose
        // function is a ConstructorReference, upstream Dart SDK source,
        // pkg/analyzer/lib/src/dart/resolver/function_reference_resolver
        // .dart): the operand row is the paren push (`Bare(Plain)`, opaque
        // — end_parenthesized_expression), the type-argument group's entry
        // keys at this row (`call_args_end`'s head-row offset), and the
        // inner tear-off's Member row sits directly below. No group (the
        // bare `(C.new)`) delegates to the member face unchanged.
        if let RowShape::Bare(BareInfo::Plain { ty }) = &row.shape {
            if *ty != Ty::UNKNOWN {
                return None;
            }
            let member = self.rows.iter().rev().find(|inner| {
                inner.range.end() <= row.range.offset
                    && matches!(
                        &inner.shape,
                        RowShape::Member {
                            recv: Receiver::Container { .. } | Receiver::PrefixedContainer { .. },
                        }
                    )
            })?;
            // The member must sit inside THIS paren group: the bytes from
            // its selector to the `)` the row anchors carry only group
            // closers (`((C.new)<int>)`'s inner `)`) and balanced `<...>`
            // groups — any other byte is a different expression.
            let bytes = self.source.as_bytes();
            let close = self.byte_of(row.range.offset);
            if bytes.get(close) != Some(&b')') {
                return None;
            }
            let mut q = self.byte_of(member.range.end());
            // The `)` anchors the paren rows the selector folded through —
            // the type-argument group's entry keys at the NEAREST one
            // (`call_args_end`'s head-row offset).
            let mut group_at = None;
            while q < close {
                let b = bytes[q];
                if b.is_ascii_whitespace() {
                    q += 1;
                    continue;
                }
                if b == b')' {
                    // A closer the selector chain folded through: its
                    // offset is a folded paren row's anchor.
                    if group_at.is_none() {
                        group_at = Some(self.utf16_of(q));
                    }
                    q += 1;
                    continue;
                }
                if b == b'<' {
                    let mut depth = 0usize;
                    loop {
                        match bytes.get(q) {
                            Some(b'<') => depth += 1,
                            Some(b'>') => {
                                depth -= 1;
                                if depth == 0 {
                                    break;
                                }
                            }
                            Some(_) => {}
                            None => return None,
                        }
                        q += 1;
                        if q > close {
                            return None;
                        }
                    }
                    q += 1;
                    continue;
                }
                return None;
            }
            // The written group's key candidates: the byte walk's recorded
            // closer, the operand row's own anchor, and `call_args_end`'s
            // head-row offset — the inner paren row itself (`rows[base-1]`),
            // which anchors at its open, not at the closer the walk recorded.
            let inner_paren = self.rows.iter().rev().find(|inner| {
                inner.range.end() <= row.range.offset
                    && matches!(
                        &inner.shape,
                        RowShape::Bare(BareInfo::Plain { ty: Ty::UNKNOWN })
                    )
            });
            let written = [group_at, Some(row.range.offset)]
                .into_iter()
                .flatten()
                .chain(inner_paren.map(|inner| inner.range.offset))
                .find_map(|anchor| {
                    self.call_type_args
                        .iter()
                        .rposition(|(at, _)| *at == anchor)
                        .map(|pos| self.call_type_args[pos].1.tys.to_vec())
                });
            return self.ctor_selector_ty(member, written);
        }
        let RowShape::Member { .. } = &row.shape else {
            return None;
        };
        self.ctor_selector_ty(row, None)
    }

    /// The constructor-selector tear-off's static type: the declared
    /// signature as a function type with the class's return head. The
    /// written type arguments (`C<int>.new`, the `((C.new)<int>)` group)
    /// instantiate the head AND the class's own parameters in the formals
    /// (`ConstructorElement.type` substitutes the receiver's
    /// instantiation); a bare generic-class receiver instantiates nothing
    /// — the class's own type parameters become the tear-off's own group
    /// (`C.new` on `class C<T>` is `C Function<T>(int, T)`, the official
    /// generic tear-off form).
    fn ctor_selector_ty(&self, row: &Row, written_args: Option<Vec<Ty>>) -> Option<Ty> {
        let RowShape::Member { recv } = &row.shape else {
            return None;
        };
        let (resolved, class) = match *recv {
            Receiver::Container { name, .. } => (
                resolve_container(
                    self.graph,
                    self.namespaces,
                    self.index,
                    self.lib,
                    self.file,
                    name,
                )?,
                name,
            ),
            Receiver::PrefixedContainer { prefix, name, .. } => (
                prefixed_container(
                    self.graph,
                    self.namespaces,
                    self.index,
                    self.lib,
                    prefix,
                    name,
                )?,
                name,
            ),
            _ => return None,
        };
        let (container, owner) = resolved;
        let declares = container.constructors.contains(&row.name)
            || (row.name == self.intern("new") && container.has_unnamed_constructor);
        if !declares {
            return None;
        }
        // The unnamed face's formals key under the CONTAINER's own name
        // (`C()` → `C` — the creation-head spelling), never the `new`
        // selector the tear-off row carries.
        let lookup = if row.name == self.intern("new") {
            class
        } else {
            row.name
        };
        let mut formals = (**container.ctor_formals.get(&lookup)?).clone();
        self.project_formals(owner, &mut formals);
        self.patch_field_formals(class, &mut formals);
        // `C<int>` on the receiver's own head — the group's entry keys
        // on the head row's offset, which survives the member merge as
        // the row's `range.offset` (the head sits before `.selector` in
        // the source bytes; find it by walking the bytes back over the
        // selector and the balanced `<...>` group).
        let written = written_args.or_else(|| self.receiver_group_args(row));
        // A receiver that SPELLS a type-argument group (`C<int>.new`) whose
        // written arguments the walk could not recover must not fall back to
        // the bare receiver's form. The two spell DIFFERENT types — the bare
        // `C.new` on `class C<T>` is the official generic tear-off
        // `C Function<T>()`, while `C<int>.new` is the non-generic
        // `C<int> Function()` — so the fallback would hand the verdict a
        // branch whose return head and own `<T>` group are both wrong. The
        // group is unrecoverable here (its entry never keys `call_type_args`,
        // and the #610 latch is a single slot the statement's later heads
        // overwrite), so the honest answer is the undecidable one: no type,
        // the under-report direction every unresolved operand face takes.
        if written.is_none() && self.receiver_spells_group(row) {
            return None;
        }
        let subst: FxHashMap<StringId, Ty> = written
            .as_deref()
            .into_iter()
            .flat_map(|args| {
                container
                    .type_param_names
                    .iter()
                    .copied()
                    .zip(args.iter().copied())
            })
            .collect();
        let ret_args = written
            .as_ref()
            .map(|args| self.boundary.args.borrow_mut().intern(args))
            .unwrap_or(ArgListId::EMPTY);
        let return_ty = Ty::Named {
            name: class,
            nullable: false,
            args: ret_args,
        };
        // The class parameters' written substitution in the formals rides
        // the same map (a bare `Named` head equal to a parameter reads its
        // positional argument; an empty map passes every leaf through).
        let positional: Vec<Ty> = formals
            .positional
            .iter()
            .map(|f| {
                if subst.is_empty() {
                    f.ty
                } else {
                    self.subst_named_leaves(f.ty, &subst)
                }
            })
            .collect();
        let named: Vec<(StringId, Ty)> = formals
            .named
            .iter()
            .map(|f| {
                let ty = if subst.is_empty() {
                    f.ty
                } else {
                    self.subst_named_leaves(f.ty, &subst)
                };
                (f.name, ty)
            })
            .collect();
        // A bare generic-class receiver instantiates nothing: the class's
        // own type parameters become the tear-off's own group (the
        // official generic tear-off's `FunctionType` type parameters).
        // The declared bounds ride in the parallel array when captured
        // (a pre-field payload decodes it empty — the unbounded posture).
        let sig_params: Vec<(StringId, Option<Ty>)> = if ret_args == ArgListId::EMPTY {
            container
                .type_param_names
                .iter()
                .enumerate()
                .map(|(index, name)| {
                    (
                        *name,
                        container
                            .type_param_bounds
                            .get(index)
                            .copied()
                            .filter(|bound| *bound != Ty::Dynamic),
                    )
                })
                .collect()
        } else {
            Vec::new()
        };
        let ty = if sig_params.is_empty() {
            self.boundary
                .args
                .borrow_mut()
                .function(0, return_ty, &positional, &named, false)
        } else {
            self.boundary.args.borrow_mut().function_generic(
                &sig_params,
                return_ty,
                &positional,
                &named,
                false,
            )
        };
        Some(ty)
    }

    /// The named-leaf substitution over a captured type: a bare `Named`
    /// head the map holds reads its entry; an argument list re-interns
    /// bottom-up; a function shape re-interns its parts with its own
    /// parameters shadowing the map (a nested signature's parameters are
    /// fresh). The shared walk of the tear-off instantiation and the
    /// generic-signature unification; canonical interning keeps an
    /// untouched leaf byte-identical.
    fn subst_named_leaves(&self, ty: Ty, map: &FxHashMap<StringId, Ty>) -> Ty {
        match ty {
            Ty::Named {
                name,
                nullable,
                args: ArgListId::EMPTY,
            } => map.get(&name).copied().unwrap_or(ty),
            Ty::Named {
                name,
                nullable,
                args,
            } => {
                let list: Vec<Ty> = {
                    let pool = self.boundary.pool();
                    pool.get(args).to_vec()
                };
                let list: Vec<Ty> = list
                    .into_iter()
                    .map(|arg| self.subst_named_leaves(arg, map))
                    .collect();
                Ty::Named {
                    name,
                    nullable,
                    args: self.boundary.intern_list(&list),
                }
            }
            Ty::Function { shape, nullable } => {
                let (count, names, ret, pos, named) = {
                    let pool = self.boundary.pool();
                    let shape = pool.function_shape(shape);
                    (
                        shape.type_params,
                        shape.type_params_named.to_vec(),
                        shape.return_ty,
                        shape.positional.to_vec(),
                        shape.named.to_vec(),
                    )
                };
                // The shape's own parameters shadow the map.
                let inner: FxHashMap<StringId, Ty> = map
                    .iter()
                    .filter(|(key, _)| !names.iter().any(|(param, _)| param == *key))
                    .map(|(key, value)| (*key, *value))
                    .collect();
                let ret = self.subst_named_leaves(ret, &inner);
                let pos: Vec<Ty> = pos
                    .into_iter()
                    .map(|formal| self.subst_named_leaves(formal, &inner))
                    .collect();
                let named: Vec<(StringId, Ty)> = named
                    .into_iter()
                    .map(|(name, formal)| (name, self.subst_named_leaves(formal, &inner)))
                    .collect();
                // Both producers wrap the rebuilt shape with the carried
                // nullability — the interned face is the substitution.
                if names.is_empty() {
                    self.boundary
                        .args
                        .borrow_mut()
                        .function(count, ret, &pos, &named, nullable)
                } else {
                    self.boundary
                        .args
                        .borrow_mut()
                        .function_generic(&names, ret, &pos, &named, nullable)
                }
            }
            other => other,
        }
    }

    /// The alias fold's function-RHS arm for a `Named` the `type_params ==
    /// 0` gate refused (`fold_alias_ty`'s `alias_decl_rhs`): a typedef
    /// spelling a function right-hand side reads that resolved shape
    /// whole — the same stand-for-the-whole rule the fold's `Ty::Function`
    /// arm applies. Two capture faces answer it, richest first: the
    /// `index.aliases` row (`AliasRhs::Fn`) carries the RHS signature's OWN
    /// parameter bounds (`Function<T extends num>`); the declaration's
    /// `var_ty` keeps the pooled shape where the alias row dropped out.
    /// Only a function-RHS alias answers — a `Named`/`Record` RHS stays
    /// unfolded, the under-report direction.
    fn alias_function_ty(&mut self, ty: Ty) -> Option<Ty> {
        let Ty::Named {
            name,
            nullable,
            args: ArgListId::EMPTY,
        } = ty
        else {
            return None;
        };
        // The `index.aliases` face carries the RHS signature's own
        // parameter bounds (`AliasRhs::Fn.type_params`); populated on the
        // real pipeline it answers the bounded-typedef pair (`xn is Xn`).
        // A rig-side alias whose capture dropped (the nested
        // function-RHS's stray-root arm) falls to `var_ty` below.
        if let Some((_, type_params, ret, positional, named, required_positional)) =
            self.bounds.alias_fn_shape(name)
        {
            let required = required_positional;
            let ret = ret.map_or(Ty::Dynamic, |tree| self.tree_ty(tree));
            let positional: Vec<Ty> =
                positional.iter().map(|tree| self.tree_ty(tree)).collect();
            let named: Vec<(StringId, Ty)> = named
                .iter()
                .map(|(name, tree, _)| (*name, self.tree_ty(tree)))
                .collect();
            let type_params: Vec<(StringId, Option<Ty>)> = type_params
                .iter()
                .map(|(name, bound, _)| {
                    (*name, bound.as_ref().map(|b| self.tree_ty(b)))
                })
                .collect();
            let ty = self.boundary.args.borrow_mut().function_generic(
                &type_params,
                ret,
                &positional,
                &named,
                nullable,
            );
            // The capture's honest required-positional count rides the
            // shape: `typedef int f<T>([int? a])` spells all-optional
            // formals, which the pooled `Function(...)` id merges with the
            // all-required `Function(int?, ...)`. Without this record the
            // arity gate reads `required = positional.len` and answers the
            // wrong side of `_isFunctionSubtypeOf`'s rule.
            if let (Some(required), Ty::Function { shape, .. }) = (required, ty)
                && shape != crate::model::FnShapeId::EMPTY
            {
                self.fn_shape_required
                    .entry(shape)
                    .or_insert(u16::try_from(required).unwrap_or(u16::MAX));
            }
            return Some(ty);
        }
        // The armed-annotation face: `var_ty` already interned the resolved
        // `Ty::Function` at capture (a nested function-RHS typedef drops
        // out of `index.aliases` on the stray-root guard but keeps this
        // shape); its `type_params_named` entries carry the parameter
        // names, their written bounds where the pooled shape recorded them.
        let decl = self.top_level_decl(name)?;
        if decl.kind != MicroKind::TypeAlias {
            return None;
        }
        match decl.var_ty {
            Ty::Function {
                shape,
                nullable: rhs_nullable,
            } => Some(Ty::Function {
                shape,
                nullable: nullable || rhs_nullable,
            }),
            _ => None,
        }
    }

    /// One subtype leg inside a signature comparison: a nested generic
    /// function pair (`Function<S>(S)` against `Function<S>(T)` inside a
    /// typedef's return) decides through [`Self::generic_signature_subtype`]
    /// — the interned leaves carry `type_params_named`, which the pooled
    /// `FnVarSig` rebuild behind [`Self::slot_subtype`] cannot spell; every
    /// other face keeps the slot ask. `outer` carries the enclosing
    /// unification's `to`-parameter bounds: upstream's shared fresh set
    /// spans every nesting level, so a `to`-param leaf spelled inside a
    /// nested signature is the SAME fresh variable its outer position
    /// introduced.
    fn signature_leg_subtype(
        &self,
        from: Ty,
        to: Ty,
        outer: &FxHashMap<StringId, Option<Ty>>,
    ) -> Option<bool> {
        if let Some(answer) = self.generic_signature_subtype_at(from, to, outer) {
            return Some(answer);
        }
        self.slot_subtype(from, to)
    }

    /// Rebuilds a non-generic `Ty::Function`'s [`FnVarSig`] with the honest
    /// required-positional count the written signature carried — the
    /// `fn_shape_required` side table the listener seals at each
    /// `end_function_type`. The canonical `FnShapeId` merges required and
    /// optional positionals (`Function(int)` and `Function([int])` are one
    /// id), so the arity gate [`Self::fn_sig_subtype`] runs upstream
    /// (`_isFunctionSubtypeOf`'s `required` rule, subtype.dart:107-113)
    /// cannot read the split off the pool. A shape with no recorded
    /// spelling — a declaration signature or an alias RHS folded through
    /// the pool — keeps the all-required face the count-only rebuild
    /// [`crate::diagnose::types`] `canonical_dotted_ty` plus the
    /// required-count propagation the re-intern loses: the rewrite builds
    /// a new `FnShapeId` when a `core.`/`p.` leaf changed, and that id
    /// answers `positional.len()` (the all-required face) in
    /// [`Self::fn_face_sig`] — a `core.List<int> Function(int, [int])`
    /// face would read required 2 where the written signature said 1. The
    /// written count is a positional-LAYOUT fact the leaf rewrite
    /// preserves, so it copies across onto the new shape — the
    /// `last_fn_seal` pair first (the written site's own answer), then the
    /// map's record for the old id.
    fn canonical_function_ty(&mut self, ty: Ty) -> Ty {
        let before = match ty {
            Ty::Function { shape, .. } if shape != crate::model::FnShapeId::EMPTY => {
                Some(shape)
            }
            _ => None,
        };
        let rewritten = self.boundary.canonical_dotted_ty(ty);
        let (
            Some(before),
            Ty::Function {
                shape: after, ..
            },
        ) = (before, rewritten)
        else {
            return rewritten;
        };
        if after != before
            && let Some(required) = self
                .last_fn_seal
                .and_then(|(sealed, count)| (sealed == before).then_some(count))
                .or_else(|| self.fn_shape_required.get(&before).copied())
        {
            self.fn_shape_required.entry(after).or_insert(required);
        }
        rewritten
    }

    /// The `is`-verdict's parameterized function-RHS typedef fold: a named
    /// head whose `index.aliases` capture is a signature (`typedef F2<T> =
    /// core.List<core.int> Function(int y, {List<Function> x})`) answers
    /// the use-site-instantiated `Ty::Function` the verdict's function rule
    /// compares — the same face `assignable`'s `function_shape_sig` reads
    /// for the declared-type asks, applied to the written `is` target and
    /// operand. The `var_ty` fold cannot carry this pair: `end_typedef`'s
    /// `take_armed` snapshots the armed head — the anonymous signature's
    /// return head, never the `Ty::Function` — so `fold_parameterized_alias`
    /// lands on `Named{core.List}` and the strict ask answers false. A
    /// BARE parameterized use (`x is F2` — `instantiateTypeFormalsToBounds`)
    /// reads the bounds-substituted defaults through `itb_default_args`
    /// before the same instantiation; a no-capture head keeps `None`.
    /// `?` at the use site joins the folded face's nullability; the honest
    /// required-positional count rides `fn_shape_required` exactly like
    /// `alias_function_ty`'s own record.
    fn parameterized_alias_fn_ty(&mut self, ty: Ty) -> Option<Ty> {
        let Ty::Named {
            name,
            nullable,
            args,
        } = ty
        else {
            return None;
        };
        // The capture gate mirrors `function_shape_sig`'s: only a
        // function-RHS alias's own file-side capture can answer — a named
        // RHS (`AliasRhs::Named`) or a head with no capture keeps the
        // caller's ordinary path. A SAME-FILE typedef whose capture
        // dropped out (a `p.T`-leafed formal poisons the tree) re-reads
        // the written RHS from the source instead.
        let Some((params, ..)) = self.bounds.alias_fn_shape(name) else {
            return self.source_alias_fn_ty(name, args, nullable);
        };
        let param_count = params.len();
        let args = if args == ArgListId::EMPTY {
            if param_count == 0 {
                // A non-parameterized function-RHS alias (`typedef F = int
                // Function()`) spells bare with no defaults to
                // instantiate — the raw signature face answers it.
                return self
                    .alias_fn_sig(name)
                    .map(|sig| self.fold_sig_ty(*sig, nullable));
            }
            // A parameterized head's bare use instantiates to bounds
            // (`x is F2` — `instantiateTypeFormalsToBounds`).
            let defaults = self.bounds.itb_default_args(self.strings, name)?;
            let tys: Vec<Ty> = defaults.iter().map(|tree| self.tree_ty(tree)).collect();
            self.boundary.intern_list(&tys)
        } else {
            args
        };
        let sig = self.instantiated_alias_sig(name, args)?;
        Some(self.fold_sig_ty(*sig, nullable))
    }

    /// The sig-to-`Ty` tail of [`Self::parameterized_alias_fn_ty`]: interns
    /// the signature through `fn_sig_ty`, records the honest
    /// required-positional count on the fresh `FnShapeId`, and joins the
    /// use-site `?` onto the face's own nullability.
    fn fold_sig_ty(&mut self, sig: FnVarSig, nullable: bool) -> Ty {
        let required = sig.required_positional;
        let ty = self.fn_sig_ty(&sig);
        match ty {
            Ty::Function {
                shape,
                nullable: rhs_nullable,
            } => {
                if shape != crate::model::FnShapeId::EMPTY {
                    self.fn_shape_required.entry(shape).or_insert(required);
                }
                Ty::Function {
                    shape,
                    nullable: nullable || rhs_nullable,
                }
            }
            _ => ty,
        }
    }

    /// The type annotation ending immediately before a declaration's name
    /// token (`core.List<core.int> m2`, `async.A<int> x`): the armed head
    /// a `p.T` spelling keeps on the declaration tables is the bare prefix
    /// (`Named{core}` — `end_typedef`'s `take_armed` reads the same face for
    /// the alias RHS's return head), so the declared face the verdict's
    /// subtype ask needs lives only in the source. `name_offset` is the
    /// name token's UTF-16 offset; the walk backs over balanced `<...>`
    /// groups and the qualified head's `.` continuations and returns the
    /// written slice verbatim (`Some` only when at least one name
    /// character closed the scan — a keyword-only or empty prefix answers
    /// `None`).
    fn backward_ty_text(&self, name_offset: u32) -> Option<String> {
        let bytes = self.source.as_bytes();
        let mut end = self.byte_of(name_offset);
        while end > 0 && bytes[end - 1].is_ascii_whitespace() {
            end -= 1;
        }
        if end == 0 {
            return None;
        }
        let mut depth: u32 = 0;
        let mut start = end;
        while start > 0 {
            let ch = bytes[start - 1];
            match ch {
                b'>' => {
                    depth += 1;
                    start -= 1;
                }
                b'<' => {
                    if depth == 0 {
                        break;
                    }
                    depth -= 1;
                    start -= 1;
                }
                b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'_' | b'$' | b'.' | b'?' | b','
                    if depth > 0 || !matches!(ch, b',') =>
                {
                    start -= 1;
                }
                _ => break,
            }
        }
        if start == end {
            return None;
        }
        Some(self.source[start..end].to_string())
    }

    /// Parses the recovered written slice [`Self::backward_ty_text`]
    /// returns into a [`Ty`]: a `p.T`-joined head (the interned dotted
    /// spelling the armed face and `split_prefixed_spelling` spell alike),
    /// an optional `<...>` argument group split at depth-0 commas, a
    /// trailing `?`. Function/record spellings (`Function(...)` shapes)
    /// decline — the declared heads this recovery feeds are named
    /// interface faces (`p.T`, `C<args>`).
    fn parse_spelled_ty(&self, text: &str) -> Option<Ty> {
        let text = text.trim();
        let (text, nullable) = text.strip_suffix('?').map_or((text, false), |t| (t, true));
        let (head_text, args) = match text.find('<') {
            Some(open) => {
                if !text.ends_with('>') {
                    return None;
                }
                let inner = &text[open + 1..text.len() - 1];
                let mut depth: u32 = 0;
                let mut parts: Vec<Ty> = Vec::new();
                let mut piece_start = 0;
                for (index, ch) in inner.char_indices() {
                    match ch {
                        '<' => depth += 1,
                        '>' => depth = depth.saturating_sub(1),
                        ',' if depth == 0 => {
                            parts.push(self.parse_spelled_ty(&inner[piece_start..index])?);
                            piece_start = index + 1;
                        }
                        _ => {}
                    }
                }
                parts.push(self.parse_spelled_ty(&inner[piece_start..])?);
                let args = self.boundary.intern_list(&parts);
                (&text[..open], args)
            }
            None => (text, ArgListId::EMPTY),
        };
        let head_text = head_text.trim();
        if head_text.is_empty()
            || !head_text
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '$' | '.'))
        {
            return None;
        }
        Some(Ty::Named {
            name: self.intern(head_text),
            nullable,
            args,
        })
    }

    /// Whether a pooled face still carries a prefix-sentinel leaf — the
    /// bare `Named{p}` a `p.T<p.U>` spelling degrades to inside a
    /// declaration-tree capture when the collector's armed walk poisons
    /// the annotation (the same face the `Named` arms above recover from
    /// the written spelling). Walks returns, formals, type-parameter
    /// bounds, and argument lists at every depth. An `Unknown` is never a
    /// sentinel here: a type-less formal (`topy([y,])` — implicit
    /// `dynamic`) and an unannotated return are honest captures, and a
    /// sig carrying them must keep its honest required-positional count.
    fn has_prefix_leaf(&self, ty: Ty) -> bool {
        self.sentinel_walk(ty, false)
    }

    /// Whether a face is worth re-reading from its written spelling: a
    /// prefix-sentinel leaf anywhere ([`Self::has_prefix_leaf`]), or an
    /// `Unknown` in RETURN position at any depth — the collector's armed
    /// walk refuses the whole annotation tree of a prefixed generic
    /// return (`core.List<core.int> f(...)`) and leaves `Unknown` there.
    /// A type-less formal's `Unknown` does not qualify. The re-read of an
    /// honestly unannotated return declines at the parse (no annotation
    /// text) and the caller keeps the captured face, so the trigger errs
    /// wide while the DISCARD in `operand_fn_sig` stays strict.
    fn needs_recovery(&self, ty: Ty) -> bool {
        self.sentinel_walk(ty, true)
    }

    fn sentinel_walk(&self, ty: Ty, unknown_return_counts: bool) -> bool {
        match ty {
            Ty::Named { name, args, .. } => {
                if args == ArgListId::EMPTY {
                    !self.strings.resolve(name).contains('.')
                        && matches!(self.resolve_bare(name).1, RefTarget::Prefix)
                } else {
                    let list = self.boundary.args.borrow().get(args).to_vec();
                    list.iter()
                        .any(|&arg| self.sentinel_walk(arg, unknown_return_counts))
                }
            }
            Ty::Function { shape, .. } => {
                if shape == crate::model::FnShapeId::EMPTY {
                    return false;
                }
                let (names, ret, pos, named) = {
                    let pool = self.boundary.pool();
                    let shape = pool.function_shape(shape);
                    (
                        shape.type_params_named.to_vec(),
                        shape.return_ty,
                        shape.positional.to_vec(),
                        shape.named.to_vec(),
                    )
                };
                (unknown_return_counts && ret == Ty::Unknown)
                    || self.sentinel_walk(ret, unknown_return_counts)
                    || pos
                        .iter()
                        .any(|&formal| self.sentinel_walk(formal, unknown_return_counts))
                    || named
                        .iter()
                        .any(|(_, formal)| self.sentinel_walk(*formal, unknown_return_counts))
                    || names.iter().any(|(_, bound)| {
                        bound.is_some_and(|b| self.sentinel_walk(b, unknown_return_counts))
                    })
            }
            _ => false,
        }
    }

    /// Splits `text` at depth-0 `sep` characters, quote- and
    /// escape-aware — the parameter/type-parameter list splitter for the
    /// written-signature recoveries (a default expression may carry
    /// commas, strings, and nested groups).
    fn split_top_commas(text: &str, sep: char) -> Vec<&str> {
        let mut parts = Vec::new();
        let mut depth: u32 = 0;
        let mut quote: Option<char> = None;
        let mut start = 0;
        for (index, ch) in text.char_indices() {
            if let Some(q) = quote {
                if ch == '\\' {
                    // The escape and its target ride inside the piece.
                } else if ch == q {
                    quote = None;
                }
                continue;
            }
            match ch {
                '\'' | '"' => quote = Some(ch),
                '(' | '[' | '{' | '<' => depth += 1,
                ')' | ']' | '}' | '>' => depth = depth.saturating_sub(1),
                c if c == sep && depth == 0 => {
                    parts.push(&text[start..index]);
                    start = index + ch.len_utf8();
                }
                _ => {}
            }
        }
        parts.push(&text[start..]);
        parts
    }

    /// The `(inner, after_close)` slice of the balanced group opening at
    /// `open` — quote- and escape-aware, nesting every bracket kind. The
    /// written-signature recoveries read formal and type-parameter groups
    /// straight out of the source with it; `None` when the group never
    /// closes (a truncated source cannot produce, but the walk stays
    /// honest anyway).
    fn balanced_group(text: &str, open: usize) -> Option<(&str, usize)> {
        let bytes = text.as_bytes();
        let opener = *bytes.get(open)?;
        let closer = match opener {
            b'(' => b')',
            b'[' => b']',
            b'{' => b'}',
            b'<' => b'>',
            _ => return None,
        };
        let mut depth: u32 = 0;
        let mut quote: Option<u8> = None;
        let mut index = open;
        while index < bytes.len() {
            let byte = bytes[index];
            if let Some(q) = quote {
                if byte == b'\\' {
                    index += 2;
                    continue;
                }
                if byte == q {
                    quote = None;
                }
                index += 1;
                continue;
            }
            match byte {
                b'\'' | b'"' => quote = Some(byte),
                b'(' | b'[' | b'{' | b'<' => depth += 1,
                b')' | b']' | b'}' | b'>' => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 && byte == closer {
                        return Some((&text[open + 1..index], index + 1));
                    }
                }
                _ => {}
            }
            index += 1;
        }
        None
    }

    /// [`Self::parse_spelled_ty`] with an alias-parameter substitution at
    /// the leaves: a bare identifier spelling a substituted parameter
    /// reads the use-site argument's face (`typedef F2<T> = List<T>
    /// Function(int)` under `x is F2<bool>` — the RHS's `T` IS `bool`),
    /// at any argument depth. Everything else matches the unprefixed
    /// parser verbatim.
    fn parse_spelled_leaf(
        &self,
        text: &str,
        subst: &FxHashMap<StringId, Ty>,
    ) -> Option<Ty> {
        let text = text.trim();
        let (text, nullable) = text.strip_suffix('?').map_or((text, false), |t| (t, true));
        // A whole-leaf substitution must be the bare parameter spelling.
        if !text.is_empty()
            && !nullable
            && text
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '$'))
            && let Some(ty) = subst.get(&self.intern(text))
        {
            return Some(*ty);
        }
        let Some(open) = text.find('<') else {
            if text.is_empty()
                || !text
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '$' | '.'))
            {
                return None;
            }
            // The keyword types carry dedicated variants — a written
            // annotation's `void`/`dynamic`/`Never`/`Null` return or
            // formal reads the same face the captured columns keep.
            return Some(match text {
                "void" if !nullable => Ty::Void,
                "dynamic" if !nullable => Ty::Dynamic,
                "Never" if !nullable => Ty::Never,
                "Null" if !nullable => Ty::Null,
                _ => Ty::Named {
                    name: self.intern(text),
                    nullable,
                    args: ArgListId::EMPTY,
                },
            });
        };
        if !text.ends_with('>') {
            return None;
        }
        let inner = &text[open + 1..text.len() - 1];
        let mut parts: Vec<Ty> = Vec::new();
        for piece in Self::split_top_commas(inner, ',') {
            parts.push(self.parse_spelled_leaf(piece, subst)?);
        }
        let head_text = text[..open].trim();
        if head_text.is_empty()
            || !head_text
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '$' | '.'))
        {
            return None;
        }
        Some(Ty::Named {
            name: self.intern(head_text),
            nullable,
            args: self.boundary.intern_list(&parts),
        })
    }

    /// Parses a written function-type literal (`[ret] Function<tp>?(...)
    /// [Function<tp>?(...)]*`, the grammar's `functionTypeTail` chain —
    /// upstream Dart SDK source, pkg/_fe_analyzer_shared/... grammar)
    /// from the head of `text`, returning the face and the consumed
    /// length: the recoveries feed it source slices whose tail (the `;`
    /// of a typedef RHS, the `)` of a verdict span) is not part of the
    /// type, so the parse must bound itself. Reading left to right, the
    /// FIRST tail's formals are the INNERMOST return's and each further
    /// tail wraps (`int Function() Function()` is a function returning
    /// `int Function()`, the last tail outermost). Every shape interns
    /// through the `_required` producers — the honest
    /// required-positional/optional split is type identity
    /// (`FnShape::required_positional`), which `fn_face_sig` reads back
    /// from the shape. `None` declines the whole recovery (under-report).
    fn parse_fn_literal_at(
        &self,
        text: &str,
        subst: &FxHashMap<StringId, Ty>,
    ) -> Option<(Ty, usize)> {
        // The first depth-0 `Function` followed by `<`/`(` bounds the
        // return part; a `Function` deeper inside a group or argument is
        // a leaf's own spelling, not the boundary.
        let bytes = text.as_bytes();
        let mut depth: u32 = 0;
        let mut quote: Option<u8> = None;
        let mut tail_at: Option<usize> = None;
        let mut index = 0;
        while index < bytes.len() {
            let byte = bytes[index];
            if let Some(q) = quote {
                if byte == b'\\' {
                    index += 2;
                    continue;
                }
                if byte == q {
                    quote = None;
                }
                index += 1;
                continue;
            }
            match byte {
                b'\'' | b'"' => quote = Some(byte),
                b'(' | b'[' | b'{' | b'<' => depth += 1,
                b')' | b']' | b'}' | b'>' => depth = depth.saturating_sub(1),
                _ => {}
            }
            if depth == 0
                && byte == b'F'
                && text[index..].starts_with("Function")
                && !matches!(bytes.get(index.wrapping_sub(1)), Some(b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'_' | b'$'))
            {
                let mut after = index + "Function".len();
                while after < bytes.len() && bytes[after].is_ascii_whitespace() {
                    after += 1;
                }
                if matches!(bytes.get(after), Some(b'<' | b'(')) {
                    tail_at = Some(index);
                    break;
                }
            }
            index += 1;
        }
        let tail_at = tail_at?;
        let head = text[..tail_at].trim();
        let mut ty = if head.is_empty() {
            // A bare tail chain reads the `dynamic` return.
            Ty::Dynamic
        } else {
            // The return head is either a named leaf or a nested chain
            // (`int Function() Function() Function()`).
            match self.parse_fn_literal_at(head, subst) {
                Some((inner, _)) => inner,
                None => self.parse_spelled_leaf(head, subst)?,
            }
        };
        let mut index = tail_at;
        let mut nullable = false;
        loop {
            // Each tail: `Function`, optional `<...>` group, `(...)`
            // group, optional trailing `?` on the outermost level.
            index += "Function".len();
            while index < bytes.len() && bytes[index].is_ascii_whitespace() {
                index += 1;
            }
            let mut type_params: Vec<(StringId, Option<Ty>)> = Vec::new();
            if bytes.get(index) == Some(&b'<')
                && let Some((inner, after)) = Self::balanced_group(text, index)
            {
                type_params = self.parse_tp_group(inner, subst)?;
                index = after;
                while index < bytes.len() && bytes[index].is_ascii_whitespace() {
                    index += 1;
                }
            }
            let open = index;
            if bytes.get(index) != Some(&b'(') {
                return None;
            }
            let (inner, after) = Self::balanced_group(text, open)?;
            let (positional, required, named) = self.parse_fn_params(inner, subst, false)?;
            ty = if type_params.is_empty() {
                self.boundary.args.borrow_mut().function_required(
                    0,
                    required,
                    ty,
                    &positional,
                    &named,
                    false,
                )
            } else {
                self.boundary.args.borrow_mut().function_generic_required(
                    &type_params,
                    required,
                    ty,
                    &positional,
                    &named,
                    false,
                )
            };
            index = after;
            while index < bytes.len() && bytes[index].is_ascii_whitespace() {
                index += 1;
            }
            if bytes.get(index) == Some(&b'?') {
                nullable = true;
                index += 1;
                while index < bytes.len() && bytes[index].is_ascii_whitespace() {
                    index += 1;
                }
            }
            if text[index..].starts_with("Function") {
                let mut after = index + "Function".len();
                while after < bytes.len() && bytes[after].is_ascii_whitespace() {
                    after += 1;
                }
                if !matches!(bytes.get(after), Some(b'<' | b'(')) {
                    break;
                }
            } else {
                break;
            }
        }
        if let Ty::Function { shape, .. } = ty
            && nullable
        {
            ty = Ty::Function { shape, nullable };
        }
        Some((ty, index))
    }

    /// Parses a parameter group's inner text into the formal columns:
    /// leading required positionals, then ONE trailing `[...]`
    /// (optional positional) or `{...}` (named) section. A formal is
    /// `Type [name] [= default]` (named: `[required] Type name [=
    /// default]`); the name strips from the trailing identifier, the
    /// default from the first depth-0 `=`, and the remaining head parses
    /// as a leaf or a nested function literal (`void Function() f`).
    /// `decl` marks a DECLARATION's formal list (a method or top-level
    /// function's own group): there a lone identifier is the formal's
    /// NAME with the implicit `dynamic` type (`topy([y,])`), where a
    /// function-type literal's `Function(int)` spells a type-only formal.
    fn parse_fn_params(
        &self,
        inner: &str,
        subst: &FxHashMap<StringId, Ty>,
        decl: bool,
    ) -> Option<(Vec<Ty>, u16, Vec<(StringId, Ty)>)> {
        let inner = inner.trim();
        if inner.is_empty() {
            return Some((Vec::new(), 0, Vec::new()));
        }
        // The optional/named section is ONE trailing bracket group — the
        // first depth-0 `[`/`{` (a default expression's own brackets and
        // strings sit deeper). Everything before it is the required
        // positional run.
        let bytes = inner.as_bytes();
        let mut depth: u32 = 0;
        let mut quote: Option<u8> = None;
        let mut section_open: Option<(usize, bool)> = None;
        let mut index = 0;
        while index < bytes.len() {
            let byte = bytes[index];
            if let Some(q) = quote {
                if byte == b'\\' {
                    index += 2;
                    continue;
                }
                if byte == q {
                    quote = None;
                }
                index += 1;
                continue;
            }
            match byte {
                b'\'' | b'"' => quote = Some(byte),
                b'[' if depth == 0 => {
                    section_open = Some((index, false));
                    depth += 1;
                }
                b'{' if depth == 0 => {
                    section_open = Some((index, true));
                    depth += 1;
                }
                b'(' | b'[' | b'{' | b'<' => depth += 1,
                b')' | b']' | b'}' | b'>' => depth = depth.saturating_sub(1),
                _ => {}
            }
            index += 1;
        }
        let (positional_text, section) = match section_open {
            Some((open, named)) => {
                let head = inner[..open].trim();
                let group = Self::balanced_group(inner, open)?;
                if !inner[group.1..].trim().is_empty() {
                    return None;
                }
                (head, Some((group.0, named)))
            }
            None => (inner, None),
        };
        let mut positional: Vec<Ty> = Vec::new();
        for piece in Self::split_top_commas(positional_text, ',') {
            if piece.trim().is_empty() {
                continue;
            }
            positional.push(self.parse_formal_ty(piece, subst, false, decl)?.1);
        }
        let required = u16::try_from(positional.len()).ok()?;
        let named: Vec<(StringId, Ty)> = match section {
            None => Vec::new(),
            Some((group, false)) => {
                if !group.trim().is_empty() {
                    positional.extend(
                        Self::split_top_commas(group, ',')
                            .into_iter()
                            .filter(|piece| !piece.trim().is_empty())
                            .map(|piece| {
                                self.parse_formal_ty(piece, subst, false, decl).map(|(_, ty)| ty)
                            })
                            .collect::<Option<Vec<_>>>()?,
                    );
                }
                Vec::new()
            }
            Some((group, true)) => Self::split_top_commas(group, ',')
                .into_iter()
                .filter(|piece| !piece.trim().is_empty())
                .map(|piece| {
                    let (name, ty) = self.parse_formal_ty(piece, subst, true, decl)?;
                    Some((name?, ty))
                })
                .collect::<Option<Vec<_>>>()?,
        };
        Some((positional, required, named))
    }

    /// One formal's type and stripped name: strips a leading `required `
    /// (named formals), a trailing `= default` (the first depth-0 `=`),
    /// and the trailing name identifier — what remains is the type head
    /// (`int x` → `int`, `List<int> x` → `List<int>`, bare `int` stays
    /// whole). A formal that is itself a function literal (`void
    /// Function() f`) recurses into [`Self::parse_fn_literal_at`]. The
    /// name returns beside the type — the named column keys by it. In a
    /// declaration's group (`decl`) a lone identifier is the type-less
    /// formal's name, its type the implicit `dynamic` (upstream's
    /// unannotated-formal reading); a literal's lone identifier is a type.
    fn parse_formal_ty(
        &self,
        piece: &str,
        subst: &FxHashMap<StringId, Ty>,
        named: bool,
        decl: bool,
    ) -> Option<(Option<StringId>, Ty)> {
        let mut head = piece.trim();
        if named && let Some(rest) = head.strip_prefix("required ") {
            head = rest.trim();
        }
        // The default tail splits at the FIRST depth-0 `=` — a default
        // expression's own `=`s, commas, and strings sit inside the
        // groups the depth walk carries.
        if let Some(eq) = Self::depth0_eq(head) && !head[..eq].trim_end().is_empty() {
            head = head[..eq].trim();
        }
        let bytes = head.as_bytes();
        let mut end = bytes.len();
        while end > 0 && bytes[end - 1].is_ascii_whitespace() {
            end -= 1;
        }
        let mut name = None;
        if end > 0
            && matches!(bytes[end - 1], b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'_' | b'$')
        {
            let mut start = end - 1;
            while start > 0
                && matches!(bytes[start - 1], b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'_' | b'$')
            {
                start -= 1;
            }
            if start > 0 {
                name = Some(self.intern(&head[start..end]));
                head = head[..start].trim_end();
            } else if decl {
                // The whole piece is one identifier: a declaration's
                // type-less formal, named and implicitly `dynamic`.
                return Some((Some(self.intern(&head[..end])), Ty::Dynamic));
            }
        }
        if head.is_empty() {
            return None;
        }
        let ty = if let Some((ty, consumed)) = self.parse_fn_literal_at(head, subst)
            && consumed == head.len()
        {
            ty
        } else {
            self.parse_spelled_leaf(head, subst)?
        };
        Some((name, ty))
    }

    /// The byte index of the first depth-0, quote-aware `=` in `text`
    /// — the formal-default separator. `None` when every `=` hides
    /// inside a group, string, or argument list.
    fn depth0_eq(text: &str) -> Option<usize> {
        let bytes = text.as_bytes();
        let mut depth: u32 = 0;
        let mut quote: Option<u8> = None;
        let mut index = 0;
        while index < bytes.len() {
            let byte = bytes[index];
            if let Some(q) = quote {
                if byte == b'\\' {
                    index += 2;
                    continue;
                }
                if byte == q {
                    quote = None;
                }
                index += 1;
                continue;
            }
            match byte {
                b'\'' | b'"' => quote = Some(byte),
                b'(' | b'[' | b'{' | b'<' => depth += 1,
                b')' | b']' | b'}' | b'>' => depth = depth.saturating_sub(1),
                b'=' if depth == 0 => return Some(index),
                _ => {}
            }
            index += 1;
        }
        None
    }

    /// A type-parameter group's inner text (`T`, `T extends num`): the
    /// name with its written bound — the bound the
    /// `relateTypeParameters` equality leg of the generic verdict asks.
    fn parse_tp_group(
        &self,
        inner: &str,
        subst: &FxHashMap<StringId, Ty>,
    ) -> Option<Vec<(StringId, Option<Ty>)>> {
        let mut params: Vec<(StringId, Option<Ty>)> = Vec::new();
        for piece in Self::split_top_commas(inner, ',') {
            let piece = piece.trim();
            if piece.is_empty() {
                continue;
            }
            let (name_text, bound) = match piece.find("extends") {
                Some(at) if at > 0 && piece.as_bytes()[at - 1].is_ascii_whitespace() => {
                    let name = piece[..at].trim();
                    let bound = self.parse_spelled_leaf(&piece[at + "extends".len()..], subst);
                    if bound.is_none() {
                        return None;
                    }
                    (name, bound)
                }
                _ => (piece, None),
            };
            if name_text.is_empty()
                || !name_text
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '$'))
            {
                return None;
            }
            params.push((self.intern(name_text), bound));
        }
        Some(params)
    }

    /// The written type annotation ending immediately before
    /// `name_offset`, function-literal aware — [`Self::backward_ty_text`]
    /// for signatures whose return annotation is itself a function type
    /// (`int Function([core.List<core.int> x]) mm`): the walk carries
    /// balanced `(...)`/`[...]`/`{...}` groups on a stack (a `(...)` may
    /// carry commas and defaults inside), tracks the generic `<...>`
    /// depth only outside those groups, and stops at any other character
    /// — a `;`, `{`, `=`, or `(` of the enclosing declaration. Leading
    /// modifier words trim; a remaining head that starts with a statement
    /// keyword vetoes (the `return f2(` a call site would otherwise
    /// misread as `return`-the-type).
    fn backward_type_text(&self, name_offset: u32) -> Option<String> {
        self.backward_type_text_at_byte(self.byte_of(name_offset))
    }

    /// [`Self::backward_type_text`] anchored at a byte index (the
    /// top-level occurrence scan works in bytes).
    fn backward_type_text_at_byte(&self, name_end: usize) -> Option<String> {
        const MODIFIERS: &[&str] = &[
            "static", "final", "const", "late", "var", "external", "covariant", "factory",
            "abstract",
        ];
        const KEYWORDS: &[&str] = &[
            "return", "throw", "new", "await", "yield", "assert", "else", "case", "in", "is",
            "as", "do", "on", "when", "get", "set", "operator", "extension", "typedef", "class",
            "enum", "mixin", "switch", "default", "while", "for", "if", "try", "catch",
        ];
        let bytes = self.source.as_bytes();
        let mut end = name_end;
        while end > 0 && bytes[end - 1].is_ascii_whitespace() {
            end -= 1;
        }
        if end == 0 {
            return None;
        }
        let mut stack: Vec<u8> = Vec::new();
        let mut angle: u32 = 0;
        let mut start = end;
        while start > 0 {
            let ch = bytes[start - 1];
            match ch {
                b')' | b']' | b'}' => {
                    stack.push(ch);
                    start -= 1;
                }
                b'(' | b'[' | b'{' => {
                    if !matches!(
                        (stack.last(), ch),
                        (Some(b')'), b'(') | (Some(b']'), b'[') | (Some(b'}'), b'{')
                    ) {
                        break;
                    }
                    stack.pop();
                    start -= 1;
                }
                b'>' => {
                    angle += 1;
                    start -= 1;
                }
                b'<' => {
                    if angle == 0 {
                        break;
                    }
                    angle -= 1;
                    start -= 1;
                }
                b',' if !stack.is_empty() || angle > 0 => start -= 1,
                b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'_' | b'$' | b'.' | b'?' => {
                    start -= 1;
                }
                // Intra-annotation whitespace (`[core.List<core.int> x]`,
                // `int Function`): the walk skips it and keeps the
                // token-left scan — any statement-level character still
                // stops the walk, and a cross-declaration walk's garbage
                // declines at parse.
                ch if ch.is_ascii_whitespace() => start -= 1,
                _ => break,
            }
        }
        if start == end {
            return None;
        }
        let mut text = self.source[start..end].trim().to_string();
        loop {
            let first = text.split_whitespace().next().unwrap_or_default();
            if MODIFIERS.contains(&first) {
                text = text[first.len()..].trim_start().to_string();
            } else {
                break;
            }
        }
        let first = text.split_whitespace().next().unwrap_or_default();
        if first.is_empty() || KEYWORDS.contains(&first) {
            return None;
        }
        Some(text)
    }

    /// Parses a recovered annotation slice as one type: a function
    /// literal consumed whole, else a named leaf (`T`, `p.T<args>`, `T?`).
    fn parse_annotation_ty(&self, text: &str) -> Option<Ty> {
        if let Some((ty, consumed)) = self.parse_fn_literal_at(text, &FxHashMap::default())
            && consumed == text.trim().len()
        {
            return Some(ty);
        }
        self.parse_spelled_leaf(text, &FxHashMap::default())
    }

    /// The member's declared signature re-read from the written source
    /// when the captured face carries a broken annotation (a
    /// prefix-sentinel or `Unknown` return — the collector's armed walk
    /// degrades `p.T` leaves inside declaration trees): `name_range`
    /// anchors the member's own name token, [`Self::backward_type_text`]
    /// recovers the return annotation verbatim, and the group after the
    /// (optional `<...>` and) name re-reads the formals with their
    /// honest required/optional/named split. The upstream verdict reads
    /// the declaration's resolved `FunctionType` — this is that face,
    /// re-derived where the capture lost it.
    fn source_member_signature(
        &self,
        entry: &MemberEntry,
    ) -> Option<(Vec<(StringId, Option<Ty>)>, Ty, Vec<Ty>, u16, Vec<(StringId, Ty)>)> {
        let name_end = self.byte_of(entry.name_range.end());
        let bytes = self.source.as_bytes();
        let mut index = name_end;
        while index < bytes.len() && bytes[index].is_ascii_whitespace() {
            index += 1;
        }
        let mut type_params: Vec<(StringId, Option<Ty>)> = Vec::new();
        if bytes.get(index) == Some(&b'<')
            && let Some((inner, after)) = Self::balanced_group(self.source, index)
        {
            type_params = self.parse_tp_group(inner, &FxHashMap::default())?;
            index = after;
            while index < bytes.len() && bytes[index].is_ascii_whitespace() {
                index += 1;
            }
        }
        if bytes.get(index) != Some(&b'(') {
            return None;
        }
        let (inner, _) = Self::balanced_group(self.source, index)?;
        let (positional, required, named) =
            self.parse_fn_params(inner, &FxHashMap::default(), true)?;
        let return_text = self.backward_type_text(entry.name_range.offset)?;
        let return_ty = self.parse_annotation_ty(&return_text)?;
        Some((type_params, return_ty, positional, required, named))
    }

    /// The top-level function's declared signature re-read from source
    /// under the same broken-capture gate (`TopDecl` carries no position,
    /// so the declaration is the word-boundary occurrence whose backward
    /// scan lands a real annotation and whose formals match the captured
    /// signature's counts — a call site answers no backward type or a
    /// keyword, and a mismatched arity declines). The first passing
    /// occurrence wins; every miss keeps the captured face
    /// (under-report).
    fn source_top_level_signature(
        &self,
        name: StringId,
        positional_count: usize,
        named_count: usize,
        tp_count: usize,
    ) -> Option<Ty> {
        let name_text = self.strings.resolve(name);
        let bytes = self.source.as_bytes();
        let mut search = 0;
        while let Some(found) = self.source[search..].find(name_text) {
            let at = search + found;
            search = at + name_text.len();
            let before_ok = at == 0
                || !matches!(bytes[at - 1], b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'_' | b'$');
            let after_name = at + name_text.len();
            let after_ok = after_name >= bytes.len()
                || !matches!(bytes[after_name], b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'_' | b'$');
            if !before_ok || !after_ok {
                continue;
            }
            let mut index = after_name;
            while index < bytes.len() && bytes[index].is_ascii_whitespace() {
                index += 1;
            }
            let mut type_params: Vec<(StringId, Option<Ty>)> = Vec::new();
            if bytes.get(index) == Some(&b'<')
                && let Some((inner, after)) = Self::balanced_group(self.source, index)
            {
                let Some(params) = self.parse_tp_group(inner, &FxHashMap::default()) else {
                    continue;
                };
                type_params = params;
                index = after;
                while index < bytes.len() && bytes[index].is_ascii_whitespace() {
                    index += 1;
                }
            }
            if bytes.get(index) != Some(&b'(') {
                continue;
            }
            let Some((inner, after_group)) = Self::balanced_group(self.source, index) else {
                continue;
            };
            let Some((positional, required, named)) =
                self.parse_fn_params(inner, &FxHashMap::default(), true)
            else {
                continue;
            };
            if positional.len() != positional_count
                || named.len() != named_count
                || type_params.len() != tp_count
            {
                continue;
            }
            // The declaration's body opener separates it from calls: a
            // call site with a backward-readable context still ends its
            // group with `;` or an operator, never a body.
            let mut after = after_group;
            while after < bytes.len() && bytes[after].is_ascii_whitespace() {
                after += 1;
            }
            if self.source[after..].starts_with("async") {
                after += "async".len();
                while after < bytes.len() && bytes[after].is_ascii_whitespace() {
                    after += 1;
                }
            }
            if !matches!(bytes.get(after), Some(b'=' | b'{' | b';')) {
                continue;
            }
            let Some(return_text) = self.backward_type_text_at_byte(at) else {
                continue;
            };
            let Some(return_ty) = self.parse_annotation_ty(&return_text) else {
                continue;
            };
            let ty = if type_params.is_empty() {
                self.boundary.args.borrow_mut().function_required(
                    0,
                    required,
                    return_ty,
                    &positional,
                    &named,
                    false,
                )
            } else {
                self.boundary.args.borrow_mut().function_generic_required(
                    &type_params,
                    required,
                    return_ty,
                    &positional,
                    &named,
                    false,
                )
            };
            return Some(self.boundary.canonical_dotted_ty(ty));
        }
        None
    }

    /// The typedef's function RHS re-read from the written source when
    /// the file-side `index.aliases` capture dropped out (a `p.T`-leafed
    /// formal in the RHS poisons the capture — `typedef F2b<T> = List<T>
    /// Function(int, [core.List<core.int> x])`): the `<...>` group after
    /// the alias name reads its own parameters, the RHS after `=` parses
    /// as a function literal with the alias parameters substituted by
    /// the use-site arguments (`F2b<T>` under `m2 is F2b<T>` — the
    /// RHS's `T` IS the class parameter), mirroring the oracle's
    /// alias-instantiation the verdict then asks. A bare use reads each
    /// parameter's written bound (the
    /// `instantiateTypeFormalsToBounds` face); a boundless bare
    /// parameter or a non-function RHS declines (under-report).
    fn source_alias_fn_ty(
        &self,
        name: StringId,
        args: ArgListId,
        nullable: bool,
    ) -> Option<Ty> {
        let name_text = self.strings.resolve(name);
        let bytes = self.source.as_bytes();
        let mut search = 0;
        while let Some(found) = self.source[search..].find("typedef") {
            let at = search + found;
            search = at + "typedef".len();
            let before_ok = at == 0
                || !matches!(bytes[at - 1], b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'_' | b'$');
            let mut index = at + "typedef".len();
            while index < bytes.len() && bytes[index].is_ascii_whitespace() {
                index += 1;
            }
            if !before_ok || !self.source[index..].starts_with(name_text) {
                continue;
            }
            let after_name = index + name_text.len();
            let after_ok = after_name >= bytes.len()
                || !matches!(bytes[after_name], b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'_' | b'$');
            if !after_ok {
                continue;
            }
            index = after_name;
            while index < bytes.len() && bytes[index].is_ascii_whitespace() {
                index += 1;
            }
            let mut params: Vec<(StringId, Option<Ty>)> = Vec::new();
            if bytes.get(index) == Some(&b'<')
                && let Some((inner, after)) = Self::balanced_group(self.source, index)
            {
                params = self.parse_tp_group(inner, &FxHashMap::default())?;
                index = after;
                while index < bytes.len() && bytes[index].is_ascii_whitespace() {
                    index += 1;
                }
            }
            if bytes.get(index) != Some(&b'=') {
                continue;
            }
            index += 1;
            while index < bytes.len() && bytes[index].is_ascii_whitespace() {
                index += 1;
            }
            let subst = self.alias_subst(&params, args)?;
            let (ty, _) = self.parse_fn_literal_at(&self.source[index..], &subst)?;
            let ty = match ty {
                Ty::Function {
                    shape,
                    nullable: parsed,
                } => Ty::Function {
                    shape,
                    nullable: nullable || parsed,
                },
                other => other,
            };
            return Some(self.boundary.canonical_dotted_ty(ty));
        }
        None
    }

    /// The alias-parameter substitution a use site spells: the written
    /// arguments zipped onto the parameters (counts must agree), or —
    /// for a bare use — each parameter's own written bound
    /// (`instantiateTypeFormalsToBounds`); a boundless bare parameter
    /// declines.
    fn alias_subst(
        &self,
        params: &[(StringId, Option<Ty>)],
        args: ArgListId,
    ) -> Option<FxHashMap<StringId, Ty>> {
        if args == ArgListId::EMPTY {
            return params
                .iter()
                .map(|&(name, bound)| bound.map(|bound| (name, bound)))
                .collect();
        }
        let list = self.boundary.args.borrow().get(args).to_vec();
        if list.len() != params.len() {
            return None;
        }
        Some(params.iter().map(|(name, _)| *name).zip(list).collect())
    }

    /// already produced. `None` for a non-function or nullable face.
    /// `required` overrides the map answer for a caller that knows the
    /// face's own written site (`check_type_check`'s `last_fn_seal` match)
    /// — the per-site count the shared `FnShapeId` cannot carry.
    fn fn_face_sig(&self, ty: Ty, required: Option<u16>) -> Option<Box<FnVarSig>> {
        let Ty::Function {
            shape,
            nullable: false,
        } = ty
        else {
            return None;
        };
        let (type_params, type_params_named, return_ty, positional, named) = {
            let pool = self.boundary.pool();
            let shape = pool.function_shape(shape);
            (
                shape.type_params,
                shape.type_params_named.to_vec(),
                shape.return_ty,
                shape.positional.to_vec(),
                shape.named.to_vec(),
            )
        };
        // The shape's own `required_positional` is the last resort: it
        // equals the positional count for every split-blind producer
        // (`ArgPool::function`'s all-required reading) and carries the
        // honest written count for the `_required` producers (the source
        // recoveries below), which build outside an `&mut self` that
        // could record the engine map.
        let shape_required = {
            let pool = self.boundary.pool();
            pool.function_shape(shape).required_positional
        };
        let required_positional = required
            .or_else(|| self.fn_shape_required.get(&shape).copied())
            .unwrap_or(shape_required);
        // A named shape carries its parameter spellings (and bounds)
        // through `type_params_named`; a count-only face keeps the
        // anonymous entries `function_shape_sig` produces.
        let params: Box<[(StringId, Option<Ty>)]> = if type_params_named.is_empty() {
            (0..usize::from(type_params))
                .map(|_| (StringId::NONE, None))
                .collect()
        } else {
            type_params_named.into_boxed_slice()
        };
        Some(Box::new(FnVarSig {
            params,
            required_positional,
            positional: positional.into(),
            named: named.iter().map(|(name, ty)| (*name, *ty, false)).collect(),
            return_ty,
        }))
    }

    /// The `is` operand's honest signature at a `Ty::Function` verdict
    /// pair: the row's own declaration answers the required-positional
    /// count the pooled shape cannot (the canonical `FnShapeId` merges the
    /// split — `x is List<int> Function([int])` over a `Function(int)`
    /// operand is one id, and the map records only the last-spelled count).
    /// A function-typed local reads `fn_var_sigs`, a local function its
    /// frame capture, a callable tear-off its declared formals; anything
    /// else rebuilds the shape with the recorded count.
    fn operand_fn_sig(&self, operand: &Row, operand_ty: Ty) -> Option<Box<FnVarSig>> {
        if let Some(sig) = self.fn_var_sigs.get(&operand.name) {
            return Some(Box::new(sig.as_ref().clone()));
        }
        if let Some(sig) = self.local_fn_sig(operand.name) {
            return Some(Box::new(sig.clone()));
        }
        // A captured declaration sig carrying a broken-annotation leaf
        // (the `p.T` truncation) declines in favor of the operand FACE's
        // own sig — but only when that face IS the source-recovered one
        // (a clean `Ty::Function`): the captured sig alone carries the
        // declaration's honest required-positional count, and a face the
        // recovery declined would hand the ask the pooled all-required
        // reading instead (`topy([y,]) is! fy` — the trailing-comma
        // family's optional formals). An `Unknown` leaf is never a reason
        // to discard: a type-less formal is an honest capture.
        if let Some(sig) = self.tear_off_sig(operand) {
            let captured_broken = self.has_prefix_leaf(sig.return_ty)
                || sig.positional.iter().any(|formal| self.has_prefix_leaf(*formal))
                || sig.named.iter().any(|(_, formal, _)| self.has_prefix_leaf(*formal));
            let face_recovered =
                matches!(operand_ty, Ty::Function { .. }) && !self.needs_recovery(operand_ty);
            if !captured_broken || !face_recovered {
                return Some(sig);
            }
        }
        self.fn_face_sig(operand_ty, None)
    }

    /// The generic-signature unification at the type-check verdict
    /// (`relateTypeParameters` — upstream Dart SDK source,
    /// pkg/analyzer/lib/src/dart/element/type_system.dart:1581-1635, which
    /// `_isFunctionSubtypeOf` runs ahead of its return/formal legs): both
    /// sides carry their own named type parameters with EQUAL nonzero
    /// counts, and the sides' parameters unify BY POSITION before the
    /// return (covariant) and formal (contravariant) legs ask — the
    /// source's `T` in a formal compares against the target's parameter
    /// of the same index, whatever its spelling (`C.new` —
    /// `C Function<T>(int, T)` — is `C Function<X>(int, X)`). A written
    /// bound on either side asks the mutual-subtype equality over the
    /// substituted pair; unwritten sides coincide (the implicit `dynamic`
    /// bound). `None` keeps the ordinary ask (count mismatches, count-only
    /// entries, non-function pairs, nullable shapes — the strict ask
    /// strips those itself).
    fn generic_signature_subtype(&self, from: Ty, to: Ty) -> Option<bool> {
        self.generic_signature_subtype_at(from, to, &FxHashMap::default())
    }

    /// [`Self::generic_signature_subtype`] with an inherited parameter
    /// context: `outer` is the enclosing level's `to`-parameter bound map
    /// (upstream's shared fresh set crosses nesting levels — a nested
    /// signature's `to` leaf that spells an outer parameter name IS the
    /// outer variable). The inner `to` parameters shadow the outer map on
    /// a same-spelling collision (the inner level's own variable wins).
    fn generic_signature_subtype_at(
        &self,
        from: Ty,
        to: Ty,
        outer: &FxHashMap<StringId, Option<Ty>>,
    ) -> Option<bool> {
        let (
            Ty::Function {
                shape: from_shape,
                nullable: false,
            },
            Ty::Function {
                shape: to_shape,
                nullable: false,
            },
        ) = (from, to)
        else {
            return None;
        };
        let (from_params, from_ret, from_pos, from_named) = {
            let pool = self.boundary.pool();
            let shape = pool.function_shape(from_shape);
            (
                shape.type_params_named.to_vec(),
                shape.return_ty,
                shape.positional.to_vec(),
                shape.named.to_vec(),
            )
        };
        let (to_params, to_ret, to_pos, to_named) = {
            let pool = self.boundary.pool();
            let shape = pool.function_shape(to_shape);
            (
                shape.type_params_named.to_vec(),
                shape.return_ty,
                shape.positional.to_vec(),
                shape.named.to_vec(),
            )
        };
        if from_params.is_empty()
            || from_params.len() != to_params.len()
            || from_params.iter().any(|(name, _)| *name == StringId::NONE)
            || to_params.iter().any(|(name, _)| *name == StringId::NONE)
        {
            return None;
        }
        let unify: FxHashMap<StringId, Ty> = from_params
            .iter()
            .zip(to_params.iter())
            .map(|((from_name, _), (to_name, _))| (*from_name, Ty::named(*to_name)))
            .collect();
        // Upstream instantiates both signatures with one shared fresh set —
        // the `to` parameters ARE those fresh variables here: a `to`-param
        // leaf on a leg's LEFT answers through its bound (Left Type
        // Variable Bound, subtype.dart:484-493 — `zn is Yn`'s inner `T`
        // satisfies `num` through its own `extends num`), on the RIGHT it
        // is a fresh variable no concrete face subtypes (`Some(false)` —
        // the unproved ask the verdict reports silent). The map merges the
        // inherited `outer` bindings (the shared set's outer variables)
        // with this level's own parameters — the inner spelling shadows a
        // same-named outer one (the inner variable is a different fresh
        // entry upstream).
        let to_bounds: FxHashMap<StringId, Option<Ty>> = outer
            .iter()
            .map(|(name, bound)| (*name, *bound))
            .chain(to_params.iter().map(|(name, bound)| (*name, *bound)))
            .collect();
        let ask = |this: &Self, a: Ty, b: Ty| -> Option<bool> {
            if a == b {
                return Some(true);
            }
            if let Ty::Named {
                name,
                args: ArgListId::EMPTY,
                nullable: false,
            } = a
                && let Some(&bound) = to_bounds.get(&name)
            {
                return this.signature_leg_subtype(bound.unwrap_or(Ty::Dynamic), b, &to_bounds);
            }
            if let Ty::Named {
                name,
                args: ArgListId::EMPTY,
                nullable: false,
            } = b
                && to_bounds.contains_key(&name)
            {
                return Some(false);
            }
            this.signature_leg_subtype(a, b, &to_bounds)
        };
        for ((_, from_bound), (_, to_bound)) in from_params.iter().zip(to_params.iter()) {
            if from_bound.is_none() && to_bound.is_none() {
                // Both unwritten: the implicit bounds coincide.
                continue;
            }
            let from_bound = self.subst_named_leaves(from_bound.unwrap_or(Ty::Dynamic), &unify);
            let to_bound = to_bound.unwrap_or(Ty::Dynamic);
            match (
                ask(self, from_bound, to_bound),
                ask(self, to_bound, from_bound),
            ) {
                (Some(true), Some(true)) => {}
                (Some(false), _) | (_, Some(false)) => return Some(false),
                _ => return None,
            }
        }
        // Return covariance over the substituted source face.
        match ask(self, self.subst_named_leaves(from_ret, &unify), to_ret) {
            Some(false) => return Some(false),
            Some(true) => {}
            None => return None,
        }
        if from_pos.len() < to_pos.len() {
            return Some(false);
        }
        // The required-positional half of `_isFunctionSubtypeOf`'s arity
        // gate (subtype.dart:107-113): the source must accept AT LEAST the
        // target's required count (`from.required <= to.required`). The
        // interned shape carries no required column — `Function(int)` and
        // `Function([int])` are one `FnShapeId` — so the written count
        // rides `fn_shape_required`, keyed at each signature's own close;
        // a shape with no recorded spelling reads its full length (the
        // all-required face).
        let from_required = self
            .fn_shape_required
            .get(&from_shape)
            .map(|&r| r as usize)
            .unwrap_or(from_pos.len());
        let to_required = self
            .fn_shape_required
            .get(&to_shape)
            .map(|&r| r as usize)
            .unwrap_or(to_pos.len());
        if from_required > to_required {
            return Some(false);
        }
        for (index, to_formal) in to_pos.iter().enumerate() {
            match ask(self, *to_formal, self.subst_named_leaves(from_pos[index], &unify)) {
                Some(false) => return Some(false),
                Some(true) => {}
                None => return None,
            }
        }
        // The named-formal set (FnShape carries no required column — the
        // tear-off face's arity posture): every named formal the target
        // accepts must exist on the source, contravariantly typed; an
        // extra source-side named spelling leaves the required-ness
        // undecidable and falls to the ordinary ask.
        for (name, to_formal) in &to_named {
            let Some(from_formal) = from_named
                .iter()
                .find(|(n, _)| n == name)
                .map(|(_, formal)| *formal)
            else {
                return Some(false);
            };
            match ask(self, *to_formal, self.subst_named_leaves(from_formal, &unify)) {
                Some(false) => return Some(false),
                Some(true) => {}
                None => return None,
            }
        }
        if from_named
            .iter()
            .any(|(name, _)| !to_named.iter().any(|(n, _)| n == name))
        {
            return None;
        }
        Some(true)
    }

    /// Whether the receiver head before a constructor selector's `.` closes
    /// a written type-argument group (`C<int>` of `C<int>.constr`) — the
    /// presence half of [`Self::receiver_group_args`]'s byte walk, without
    /// the recovery. A `true` with no recovered arguments is the
    /// undecidable shape (see [`Self::ctor_selector_ty`]): the receiver
    /// spells an instantiation, so the bare-receiver form is a different
    /// type and must not stand in for it.
    fn receiver_spells_group(&self, row: &Row) -> bool {
        let bytes = self.source.as_bytes();
        let mut p = self.byte_of(row.range.offset);
        while p > 0 && bytes[p - 1].is_ascii_whitespace() {
            p -= 1;
        }
        if p == 0 || bytes[p - 1] != b'.' {
            return false;
        }
        p -= 1;
        while p > 0 && bytes[p - 1].is_ascii_whitespace() {
            p -= 1;
        }
        p > 0 && bytes[p - 1] == b'>'
    }

    /// The receiver head's written type-argument list at a constructor
    /// tear-off operand (`C<int>` of `C<int>.constr`): the merged row
    /// anchors the member token, so the receiver's head byte sits
    /// before the `.` — walk the bytes back over the selector and the
    /// balanced `<...>` group, then read the `call_type_args` entry
    /// keyed at the head's offset. `None` when no group was written
    /// (the bare `C` face, whose all-dynamic raw face a `C<int>`
    /// checked spelling cannot spell).
    fn receiver_group_args(&self, row: &Row) -> Option<Vec<Ty>> {
        // The selector's token start minus one: the `.` sits there
        // (possibly through whitespace). The head's last byte is the
        // `>` of the closed group for `C<int>` — or the head ident's
        // end for the bare `C` face.
        let mut p = self.byte_of(row.range.offset);
        let bytes = self.source.as_bytes();
        while p > 0 && bytes[p - 1].is_ascii_whitespace() {
            p -= 1;
        }
        if p == 0 || bytes[p - 1] != b'.' {
            return None;
        }
        p -= 1;
        while p > 0 && bytes[p - 1].is_ascii_whitespace() {
            p -= 1;
        }
        if p == 0 {
            return None;
        }
        // `C<int>` ends in `>`: walk back to its matching `<`, then to
        // the head ident's start — the `call_type_args` key.
        if bytes[p - 1] != b'>' {
            return None;
        }
        let mut depth = 0usize;
        let mut scan = p - 1;
        let head_open = loop {
            if scan == 0 {
                return None;
            }
            scan -= 1;
            match bytes[scan] {
                b'>' => depth += 1,
                b'<' => {
                    if depth == 0 {
                        break scan;
                    }
                    depth -= 1;
                }
                _ => {}
            }
        };
        let mut ident = head_open;
        while ident > 0
            && matches!(bytes[ident - 1], b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'_' | b'$')
        {
            ident -= 1;
        }
        if ident == head_open {
            return None;
        }
        let head_offset = self.utf16_of(ident);
        if let Some((_, entry)) = self
            .call_type_args
            .iter()
            .find(|(offset, _)| *offset == head_offset)
        {
            return Some(entry.tys.to_vec());
        }
        // A type-head group (`C<int>.new`) never keys the call-args table:
        // the group closes with a TYPE head (end_type_arguments' head arm
        // skips `call_args_end`), so its written arguments ride the #610
        // completed-group latch until a declaration consumes it — and a
        // constructor-reference selector is an expression head, no
        // declaration ever fires. The latch keys at the same head offset
        // the byte walk recovered; an unrelated later head replaces it
        // (the latch's own reset rule), so a stale entry cannot answer.
        if let Some((_, group_offset, args)) = &self.completed_group
            && *group_offset == head_offset
        {
            return Some(args.clone());
        }
        None
    }

    /// A function-typed operand/checked spelling rebuilt with the
    /// official element's leaves for the type-check verdict (#2813): the
    /// capture parks `Ty::Unknown` for every unwritten column (the
    /// executable-capture convention — a bare formal or return keeps the
    /// question open for the arity and compare faces), while the
    /// analyzer's element reads `dynamic` for an omitted annotation
    /// (`typedef fx(x, )` and `void topx(x,)` both spell
    /// `… Function(dynamic)`). The strict ask
    /// ([`Self::resolved_subtype`]'s Function rule — return covariance,
    /// formals contravariance) is undecided over the `Unknown` leaves,
    /// so the verdict normalizes both sides through this face before it
    /// asks; fully written signatures and non-function shapes pass
    /// through unchanged, and the interned pool keeps the rebuild
    /// canonical (same-shape re-interns share one slot).
    fn official_fn_face(&self, ty: Ty) -> Ty {
        let Ty::Function { shape, nullable } = ty else {
            return ty;
        };
        if shape == crate::model::FnShapeId::EMPTY {
            return ty;
        }
        let (type_params, names, return_ty, positional, named) = {
            let pool = self.boundary.args.borrow();
            let shape = pool.function_shape(shape);
            (
                shape.type_params,
                shape.type_params_named.to_vec(),
                shape.return_ty,
                shape.positional.to_vec(),
                shape.named.to_vec(),
            )
        };
        let leaf = |ty: Ty| if ty == Ty::Unknown { Ty::Dynamic } else { ty };
        let positional: Vec<Ty> = positional.into_iter().map(leaf).collect();
        let named: Vec<(StringId, Ty)> = named
            .into_iter()
            .map(|(name, ty)| (name, leaf(ty)))
            .collect();
        // The written-parameter group rides the rebuild (#2878): a
        // count-only producer would strip the names the generic-signature
        // asks correspond on.
        if names.is_empty() {
            self.boundary.intern_function_ty(
                type_params,
                leaf(return_ty),
                &positional,
                &named,
                nullable,
            )
        } else {
            self.boundary.args.borrow_mut().function_generic(
                &names,
                leaf(return_ty),
                &positional,
                &named,
                nullable,
            )
        }
    }

    /// The `this`/`super` keyword operand at an is-check the keyword
    /// heads: the keyword pushes no row ([`Self::push_keyword_receiver`]
    /// stages the flow side channel only), so the nearest row below
    /// belongs to an older expression and must not stand in as the
    /// operand — the keyword itself is the operand, typed by the
    /// enclosing container ([`Self::this_static_ty`], an extension's
    /// written `on` type included; `super`'s static type is the same
    /// `thisType` face, upstream `ThisExpressionImpl.staticType`).
    /// Answers the keyword's own span beside the type when the token
    /// directly before `at` (the caller passes the `is` operator's
    /// offset) is the keyword and a container face exists; `None`
    /// otherwise, the ordinary operand-row read stands.
    pub(crate) fn keyword_operand_ty(&self, at: u32) -> Option<(Ty, SourceRange)> {
        let tokens = self.tokens.as_slice();
        let index = tokens.partition_point(|t| t.utf16_offset < at);
        let keyword = index.checked_sub(1).and_then(|i| tokens.get(i))?;
        if !matches!(
            keyword.kind,
            dartq_scanner::TokenKind::Keyword(dartq_scanner::Keyword::This)
                | dartq_scanner::TokenKind::Keyword(dartq_scanner::Keyword::Super)
        ) {
            return None;
        }
        let (container, kind) = self.container_context()?;
        let ty = self.this_static_ty(container, kind);
        if ty == Ty::Unknown {
            return None;
        }
        // `this`/`super` are pure ASCII: the UTF-16 length is the byte
        // length.
        Some((
            ty,
            SourceRange {
                offset: keyword.utf16_offset,
                length: keyword.byte_len,
            },
        ))
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
    pub(crate) fn check_type_check(
        &mut self,
        at: SourceRange,
        keyword_operand: Option<(Ty, SourceRange)>,
        negated: bool,
    ) {
        let Some(checked) = self.armed_ty else { return };
        // The written target's own shape before the alias/canonical
        // folds: `last_fn_seal` matches it to recover the written
        // signature's required count — the per-site answer the shared
        // `FnShapeId` map cannot give (`fn_face_sig` at the verdict).
        let checked_shape0 = match checked {
            Ty::Function { shape, .. } if shape != crate::model::FnShapeId::EMPTY => {
                Some(shape)
            }
            _ => None,
        };
        // The `this`/`super` keyword operand (`this is C` in `C`), resolved
        // by the listener at the operator's own left — the keyword pushes
        // no row, so the nearest row below belongs to an older expression
        // and must not decide the verdict. The listener's verdict span
        // already anchors at the keyword and extends through the written
        // type; `at` needs no re-anchor.
        // The target side folds like the operand side
        // ([`Self::bind_declared_type`]): the verdict reads the resolved
        // interface type — `x is IntAlias` over an `int` operand is
        // constant-true. A function-typed spelling (an old-style
        // `typedef fx(x, );` alias folds to its synthesized signature —
        // #2813's collector capture) reads through the official function
        // face, the unwritten columns' `dynamic` leaves; the `Never`
        // unit fold canonicalizes its written formal leaves (#2842).
        let mut checked = self.normalized_never(self.official_fn_face(self.fold_alias_ty(checked)));
        // A function-RHS typedef whose own `<T>` group went missing
        // (`typedef Fn = void Function<S>(S) Function<T>(T)` — the RHS's
        // first signature group sealed `pending.type_params`, so the decl
        // reads generic and the `type_params == 0` alias-fold gate refuses)
        // still folds here: `var_ty` already interned the resolved
        // `Ty::Function` at capture, the same face `fold_alias_ty` would
        // answer had the gate admitted it. A genuinely parameterized alias
        // keeps `type_params > 0` the honest way — but its `var_ty` is a
        // `Ty::Named` RHS, never a `Ty::Function`, so the function check
        // below refuses it and the fold only fires on the captured
        // function shape.
        if let Some(folded) = self.alias_function_ty(checked) {
            checked = self.normalized_never(self.official_fn_face(folded));
        }
        // A qualified spelling (`core.List`, `p.T`) rides the captured RHS
        // and the written target as the dot-joined head; the resolved
        // element the operand's declared face carries spells the same
        // declaration bare. The same-element rewrite `boundary.ty` runs on
        // the foreign projection is needed here too — without it a
        // `core.`-spelled alias RHS (`typedef F2<T> = core.List<core.int>
        // Function(int, [List<T> x])`) reads a different head from the
        // operand's `List` declaration face and the always-true verdict
        // stays silent (the `f2 is F2<int>` family).
        checked = self.canonical_function_ty(checked);
        // A parameterized function-RHS typedef whose written arguments
        // survived the folds (`m2 is F2<T>`) folds here through the
        // file-side `index.aliases` capture: `var_ty` snapshots the armed
        // return head (`core.List`), never the anonymous signature, so
        // `fold_parameterized_alias` lands the pair as (Function, Named)
        // and the strict ask stays silent where the oracle's
        // `instantiateTypeFormalsToBounds` reads the instantiated
        // signature (`isSubtypeOf` over `sig<T→arg>`, subtype.dart). The
        // fold records the written `?` on the use and the honest
        // required-positional count on the fresh shape.
        if let Some(folded) = self.parameterized_alias_fn_ty(checked) {
            let folded = self.canonical_function_ty(folded);
            checked = self.normalized_never(self.official_fn_face(folded));
        }
        // A bare generic alias (`x is F0` over `typedef F0<T> = int
        // Function(int, [Function x])`) survives the first fold as the
        // instantiate-to-bounds head `F0<dynamic>` — the bare loop's
        // `type_params == 0` gate refuses a parameterized RHS. A second
        // pass reads that arg-carrying head through
        // `fold_parameterized_alias`, substituting the bounds into the
        // captured signature the verdict's function rule answers (the
        // oracle's `instantiateTypeFormalsToBounds` face).
        checked = self.normalized_never(self.official_fn_face(self.fold_alias_ty(checked)));
        // A written signature's `p.T` RETURN leaf truncates to the bare
        // prefix inside the pooled shape (`is p.C<p.I> Function(...)`
        // interns the `Function`'s `return_ty` as `Named{p}`): the same
        // armed-prefix face the `Named` arm below recovers for the
        // whole-type spelling, one level down. `armed_ty_span` rides the
        // LAST type head inside the target (a formal's `C`, not the
        // return's `p`), so the `p` anchor is re-derived from the verdict
        // span: the first `{p}.` after the `is` keyword IS the signature's
        // return head. The `.ident<args>` continuation spells the leaf,
        // the pooled shape re-interns with the joined face — every OTHER
        // leaf (`p.C` formals) truncates identically on the operand's
        // declared face and self-equalizes in the slot walk.
        if let Ty::Function { shape, nullable } = checked {
            let (type_params, type_params_named, return_ty, positional, named) = {
                let pool = self.boundary.pool();
                let shape = pool.function_shape(shape);
                (
                    shape.type_params,
                    shape.type_params_named.to_vec(),
                    shape.return_ty,
                    shape.positional.to_vec(),
                    shape.named.to_vec(),
                )
            };
            if let Ty::Named {
                name,
                args: ArgListId::EMPTY,
                ..
            } = return_ty
                && !self.strings.resolve(name).contains('.')
                && matches!(self.resolve_bare(name).1, RefTarget::Prefix)
            {
                let at_text = &self.source[self.byte_of(at.offset)..self.byte_of(at.end())];
                let head_text = format!("{}.", self.strings.resolve(name));
                if let Some(is_at) = at_text.rfind(" is ").or_else(|| at_text.rfind(" is! "))
                    && let Some(rel) = at_text[is_at..].find(&head_text)
                {
                    let end = self.byte_of(at.offset) + is_at + rel;
                    let tail = &self.source[end..];
                    let continuation_len = {
                        let mut len = head_text.len();
                        let bytes = tail.as_bytes();
                        if tail.starts_with(&head_text) {
                            while len < bytes.len()
                                && matches!(bytes[len], b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'_' | b'$')
                            {
                                len += 1;
                            }
                            if bytes[len..].starts_with(b"<") {
                                let mut depth = 0u32;
                                for (index, &byte) in bytes[len..].iter().enumerate() {
                                    match byte {
                                        b'<' => depth += 1,
                                        b'>' => {
                                            depth = depth.saturating_sub(1);
                                            if depth == 0 {
                                                len += index + 1;
                                                break;
                                            }
                                        }
                                        _ => {}
                                    }
                                }
                            }
                        }
                        len
                    };
                    if continuation_len > head_text.len() {
                        let spelled = tail[..continuation_len].to_string();
                        if let Some(recovered) = self.parse_spelled_ty(&spelled) {
                            let recovered = self.boundary.canonical_dotted_ty(recovered);
                            // A count-only `<...>` group keeps anonymous
                            // entries — the same face `fn_face_sig`'s rebuild
                            // carries for unwritten parameter spellings.
                            let params: Vec<(StringId, Option<Ty>)> = if type_params_named.is_empty()
                            {
                                (0..usize::from(type_params))
                                    .map(|_| (StringId::NONE, None))
                                    .collect()
                            } else {
                                type_params_named
                            };
                            let rebuilt = self.boundary.args.borrow_mut().function_generic(
                                &params,
                                recovered,
                                &positional,
                                &named,
                                nullable,
                            );
                            if let Ty::Function {
                                shape: before,
                                nullable: _,
                            } = checked
                                && let Ty::Function {
                                    shape: after,
                                    nullable,
                                } = rebuilt
                            {
                                if let Some(required) = self
                                    .fn_shape_required
                                    .get(&before)
                                    .copied()
                                    .or_else(|| {
                                        self.last_fn_seal.and_then(|(sealed, count)| {
                                            (sealed == before).then_some(count)
                                        })
                                    })
                                {
                                    self.fn_shape_required.entry(after).or_insert(required);
                                }
                                checked = Ty::Function {
                                    shape: after,
                                    nullable,
                                };
                            }
                        }
                    }
                }
            }
        }
        // The listener's leaf-substitute keeps only the continuation of a
        // dot-joined checked head (`is p.T` arms `Named{T}` — the leaf
        // resolves the element but drops the import prefix the subtype
        // ask needs). `armed_ty_span` still anchors the prefix token:
        // `p`'s written range, resolving to `RefTarget::Prefix`, rebuilds
        // the joined head `p.T` — the same element `lookup_prefixed`
        // answers, and the spelling the operand's own prefixed declared
        // face spells. A bare `is T` (`armed_ty_span` over `T`, not a
        // prefix) and an unresolvable prefix keep `checked` unchanged.
        if let Ty::Named {
            name,
            nullable,
            args,
        } = checked
            && let Some(span) = self.armed_ty_span
        {
            let start = self.byte_of(span.offset);
            let end = self.byte_of(span.end());
            // The span may trail the written `?` (`is p.T?` arms `p`..`?`)
            // — the prefix token's own spelling is the head candidate.
            let span_text = self.source[start..end].trim_end_matches('?');
            if self.strings.resolve(name) != span_text
                && matches!(self.resolve_bare(self.intern(span_text)).1, RefTarget::Prefix)
            {
                let joined = self.intern(&format!("{}.{}", span_text, self.strings.resolve(name)));
                checked = Ty::Named {
                    name: joined,
                    nullable,
                    args,
                };
            } else if self.strings.resolve(name) == span_text
                && matches!(self.resolve_bare(name).1, RefTarget::Prefix)
            {
                // The armed head IS the bare prefix (`is p.T` armed
                // `Named{p}` — the prefix-token face the declaration
                // tables keep): the leaf and any argument group never
                // interned, so the source continuation after the span
                // spells them. `.ident` reads the qualified head; a
                // trailing `<...>` group/`?` rides the recovered slice
                // through `parse_spelled_ty`, the same face the operand's
                // declared `p.T` keeps.
                let tail = &self.source[end..];
                let continuation_len = {
                    let mut len = 0;
                    let bytes = tail.as_bytes();
                    if bytes.first() == Some(&b'.') {
                        len = 1;
                        while len < bytes.len()
                            && matches!(bytes[len], b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'_' | b'$')
                        {
                            len += 1;
                        }
                        if bytes[len..].starts_with(b"<") {
                            let mut depth = 0u32;
                            for (index, &byte) in bytes[len..].iter().enumerate() {
                                match byte {
                                    b'<' => depth += 1,
                                    b'>' => {
                                        depth = depth.saturating_sub(1);
                                        if depth == 0 {
                                            len += index + 1;
                                            break;
                                        }
                                    }
                                    _ => {}
                                }
                            }
                        }
                        if bytes[len..].starts_with(b"?") {
                            len += 1;
                        }
                    }
                    len
                };
                if continuation_len > 1
                    && let Some(recovered) = self.parse_spelled_ty(&format!(
                        "{}{}",
                        span_text,
                        &self.source[end..end + continuation_len]
                    ))
                {
                    // The joined `p.T` collapses through the same-element
                    // face the operand's declared `p.T` keeps
                    // (`canonical_dotted_ty` — `async.A` and `A` name one
                    // element when the prefix's library also resolves
                    // bare); the written `?` at the use site joins.
                    checked = match self.boundary.canonical_dotted_ty(recovered) {
                        Ty::Named {
                            name,
                            nullable: recovered_nullable,
                            args,
                        } => Ty::Named {
                            name,
                            nullable: recovered_nullable || nullable,
                            args,
                        },
                        other => other,
                    };
                }
            }
        }
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
        // A written function-literal target whose pooled face still
        // carries a prefix-sentinel leaf at ANY depth (the return-level
        // arm above recovers the head alone; the formals' `p.T` leaves
        // truncate the same way one level down) re-reads the whole
        // literal from the verdict span — the written spelling after the
        // `is`/`is!` keyword parses with every leaf intact (`int
        // Function([core.List<core.int> x]) Function()`), the same face
        // the operand's own source recovery spells. The parse bounds
        // itself structurally, so the span's trailing `)` never joins.
        if let Ty::Function { .. } = checked
            && self.needs_recovery(checked)
        {
            let start = self.byte_of(at.offset);
            let end = self.byte_of(at.end());
            let span_text = &self.source[start..end];
            let is_at = span_text
                .rfind(" is! ")
                .or_else(|| span_text.rfind(" is "))
                .map(|pos| pos + if span_text[pos..].starts_with(" is! ") { 5 } else { 4 });
            if let Some(keyword_end) = is_at {
                let tail = &span_text[keyword_end..];
                let trimmed = tail.trim_start();
                let skip = tail.len() - trimmed.len();
                if let Some((recovered, _)) =
                    self.parse_fn_literal_at(&span_text[keyword_end + skip..], &FxHashMap::default())
                {
                    checked = self.canonical_function_ty(recovered);
                }
            }
        }
        // `TypeSystem::type_check_verdict`'s first gate — an
        // `InvalidType` operand is informative before every other arm
        // (`subtype.dart`'s 0c vacuity never reaches the verdict), so
        // `Missing rv is dynamic` reports nothing. A bare prefix token as
        // operand (`e is p` nonsense) is the same silence — the prefix
        // answers no subtype question — while a dot-joined `p.T` head IS
        // a resolvable element the subtype walk reads through
        // `split_prefixed_spelling`; only the dot-free name vetoes here.
        // The operand's resolved static type (`leftNode.typeOrThrow`,
        // best_practices_verifier.dart:1011) — the alias fold mirrors the
        // target side's: `f0 is F0<int>` over a tear-off typed
        // `F0<int>` reads both sides' aliased function shapes, the pair
        // the strict face's function rule answers (an unfolded operand
        // would strand the pair as (Named, Function) — informative,
        // the under-report the oracle's flag answers true).
        let operand_ty = match keyword_operand {
            Some((ty, _)) => ty,
            None => self.type_check_operand_ty(),
        };
        // The declared `p.T` face surfaces as the bare prefix
        // (`Named{async}` for `async.A<int> x` — the armed head's
        // prefix-token face): the prefix veto below must not read it as
        // the `e is p` nonsense, whose operand is a bare prefix READ.
        // The operand row's own local declaration anchors the name token
        // — the backward scan recovers the written `p.T<args>` spelling
        // and rejoins the declared face the strict ask needs.
        let operand_decl_range = if let Ty::Named {
            name,
            args: ArgListId::EMPTY,
            ..
        } = operand_ty
            && !self.strings.resolve(name).contains('.')
            && matches!(self.resolve_bare(name).1, RefTarget::Prefix)
        {
            self.operand_row()
                .and_then(|row| {
                    self.locals
                        .iter()
                        .rev()
                        .find(|decl| decl.name == row.name && !decl.retired)
                })
                .map(|decl| decl.range.offset)
        } else {
            None
        };
        let operand_ty = operand_decl_range
            .and_then(|offset| self.backward_ty_text(offset))
            .and_then(|text| self.parse_spelled_ty(&text))
            .unwrap_or(operand_ty);
        // The tear-off face over an unwritten-signature function (`void
        // topx(x,)` — a bare-read is-check's container-less operand,
        // #2813) reads the same official leaves as the checked side. The
        // `Never` unit fold canonicalizes the written-leaf captures
        // (#2842 — `typedef Fun(Never a, Never b)`'s synthesized formals)
        // before any structural leg asks.
        let operand_ty =
            self.normalized_never(self.official_fn_face(self.fold_alias_ty(operand_ty)));
        // The function-RHS typedef arm above fires on the operand face too:
        // `Alias f; f is F` reads `f`'s declared `Alias` head, whose
        // captured `Ty::Function` the verdict's function rule answers.
        let operand_ty = self
            .alias_function_ty(operand_ty)
            .map(|ty| self.normalized_never(self.official_fn_face(ty)))
            .unwrap_or(operand_ty);
        // The operand's `p.T`/`core.` leaves canonicalize the same way —
        // the declared face spells `core.List` where the checked side's
        // written or folded head reads `List`. The same raw-generic-alias
        // second fold then reads an `x: F0` declared face's bounds into the
        // signature.
        let operand_ty = self.canonical_function_ty(operand_ty);
        // The same parameterized function-RHS fold the checked side ran:
        // an operand whose declared face spells `F2<T>` (`x15 is F15<bool>`
        // — the tear-off var's alias-written declared type) unfolds to the
        // instantiated signature so the pair asks the strict function rule
        // instead of stranding as (Named, Function).
        let operand_ty = self
            .parameterized_alias_fn_ty(operand_ty)
            .map(|ty| {
                let ty = self.canonical_function_ty(ty);
                self.normalized_never(self.official_fn_face(ty))
            })
            .unwrap_or(operand_ty);
        let operand_ty =
            self.normalized_never(self.official_fn_face(self.fold_alias_ty(operand_ty)));
        if operand_ty == Ty::Unknown
            || matches!(operand_ty, Ty::Named { name, .. } if !self.strings.resolve(name).contains('.') && matches!(self.resolve_bare(name).1, RefTarget::Prefix))
            || self.is_invalid_ty(operand_ty)
        {
            return;
        }
        let verdict =
            if checked == Ty::Dynamic {
                Some(true)
            } else if checked == Ty::Unknown || self.is_invalid_ty(checked) {
                None
            } else if checked == Ty::Null {
                // The `is Null` family (`_checkAllTypeChecks`'s `Null` arm)
                // — unless the operand is the `null` literal, where the
                // check is constant instead (the same walk's `null` arm:
                // `Null <: Null` answers `true`). The literal's row is the
                // operand's own — a keyword operand (`this is Null`) has no
                // row and the stale one below must not stand in.
                let null_literal = keyword_operand.is_none()
                    && self.operand_row().is_some_and(|row| {
                        row.ty == Ty::Null && matches!(row.shape, RowShape::Value)
                    });
                if null_literal {
                    // `Null <: T` decides the literal check's polarity — the
                    // strict face (the official walk is
                    // `isSubtypeOf(Null, rightType)`, the same relation).
                    Some(self.resolved_subtype(Ty::Null, checked))
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
                            // The gate owns TYPE-PARAMETER positions only: a
                            // bare spelling that names no in-scope parameter
                            // (`Null`'s identifier row, an unresolvable head)
                            // is not a parameter with a bound to prove — it
                            // defers to the strict ask below, which reads the
                            // `Null` identity and the 0c faces itself.
                            if !self.is_scope_param(param) {
                                non_param_position = true;
                                continue;
                            }
                            let bound = match self.param_bound(param) {
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
                let arity_true = if let (
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
                    ) {
                    self.boundary.args.borrow().get(checked_args).len()
                        != container.type_params as usize
                } else {
                    false
                };
                if arity_true {
                    Some(true)
                } else if param_pair_true && !non_param_position {
                    Some(true)
                } else if let Some(unified) = self.generic_signature_subtype(operand_ty, checked) {
                    // The both-generic signature pair answers through the
                    // positional unification (`relateTypeParameters`'s
                    // shared fresh set — the strict ask's bounds leg alone
                    // cannot spell the formal correspondence); a decided
                    // miss is informative exactly like the ask below.
                    if !unified {
                        return;
                    }
                    Some(true)
                } else if let (Ty::Function { .. }, Ty::Function { .. }) = (operand_ty, checked)
                    && let Some(sig_verdict) = {
                        // The non-generic (or unification-undecidable)
                        // signature pair answers through the honest
                        // required-positional counts: the operand's own
                        // declaration sig where the row resolves one, the
                        // checked side's recorded written count. The pooled
                        // `sub == sup` reflexivity reads `Function(int)` and
                        // `Function([int])` as one shape and would report
                        // true here; `fn_sig_subtype`'s arity gate
                        // (`_isFunctionSubtypeOf`, subtype.dart:107-113)
                        // answers `false` on the real split.
                        let operand_sig = match keyword_operand {
                            Some(_) => self.fn_face_sig(operand_ty, None),
                            None => self
                                .operand_row()
                                .and_then(|row| self.operand_fn_sig(row, operand_ty))
                                .or_else(|| self.fn_face_sig(operand_ty, None)),
                        };
                        // The written target is the signature the walk
                        // sealed last — its own required count rides
                        // `last_fn_seal` while the shared `FnShapeId` map
                        // may answer a same-shape spelling's (the operand
                        // declaration's `Function(int)` and the target's
                        // `Function([int])` are one id). The pre-fold
                        // `checked_shape0` match keeps the read on the
                        // literal target alone: a folded alias's shape is
                        // not the seal's.
                        let checked_required = checked_shape0
                            .and_then(|shape0| {
                                self.last_fn_seal.and_then(|(sealed, count)| {
                                    (sealed == shape0).then_some(count)
                                })
                            });
                        let checked_sig = self.fn_face_sig(checked, checked_required);
                        operand_sig
                            .zip(checked_sig)
                            .and_then(|(from, to)| self.fn_sig_subtype(&from, &to))
                    }
                {
                    if !sig_verdict {
                        return;
                    }
                    Some(true)
                } else if !self.resolved_subtype(operand_ty, checked) {
                    // A non-matching pair is informative — the official order
                    // has no subtype-negative verdict
                    // (`TypeSystem::type_check_verdict`'s doc): only the
                    // constant-true answer reports here, with `is!` mapping
                    // to the 'false' row below. The ask is
                    // [`Self::resolved_subtype`], the strict mirror of
                    // `TypeSystem.isSubtypeOf` — the name-level lattice
                    // alone cannot stand here: its same-head argument walk
                    // delegates to the assignability slot, whose legacy
                    // `Null` and top leniencies answer `Null → C` beyond
                    // the strict relation (`List<Null> is List<Object>` —
                    // the covariant argument leg asks `Null <: Object`,
                    // Left Null refuses, the oracle is silent).
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
            if row.ty != Ty::UNKNOWN {
                return row.ty;
            }
            return self.untyped_operand_ty(row);
        }
        // A send on a formal whose written annotation is a function type
        // (`bool Function(E) test`) reads its signature's return — the
        // row's `ty` is the formal's own declared type (or a fragment of
        // the annotation the armed slot last completed), never the call's
        // result.
        if !row.qualified
            && let Some(sig) = self.fn_var_sigs.get(&row.name)
            // A formal-claimed signature binds only while that formal is in
            // scope: outside its executable the spelling names something
            // else (a sibling `open()` method beside another method's
            // `Conn Function() open` formal).
            && (!self.fn_param_sigs.contains_key(&row.name)
                || self.frames.iter().any(|f| f.locals.contains(&row.name)))
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

    /// The untyped row's static type the oracle still answers
    /// (`expression.typeOrThrow`): `this`/`super` keyword receivers read the
    /// enclosing container's `thisType`; a bare container name or a
    /// prefixed container name in value position is a `TypeLiteral` (static
    /// type `Type`, upstream `TypeLiteralImpl.staticType`); a member or
    /// bare read resolves its declared accessor type
    /// ([`Self::receiver_static_ty`]), a tear-off spelling falling through
    /// to the function faces (a ctor selector `C.new` via
    /// [`Self::ctor_tear_off_ty`], a member read via
    /// [`Self::member_tear_off_ty`], a bare function read via
    /// [`Self::operand_tear_off_ty`] — the same dispatch
    /// [`Self::type_check_operand_ty`] runs); a collection or closure
    /// literal row resolves through the literal faces. Every other shape
    /// keeps the suppress-everything sentinel.
    fn untyped_operand_ty(&self, row: &Row) -> Ty {
        match &row.shape {
            RowShape::Receiver(
                Receiver::This { container, kind } | Receiver::Super { container, kind },
            ) => self.this_static_ty(*container, *kind),
            RowShape::Bare(BareInfo::Container { .. }) => Ty::named(self.intern("Type")),
            RowShape::Member {
                recv: Receiver::Prefix { name: prefix },
            } => {
                let ty = self.receiver_static_ty(row);
                if ty != Ty::UNKNOWN {
                    return ty;
                }
                // A prefixed container name in value position (`p.C`) is a
                // TypeLiteral — the scoped declaration's kind decides.
                if self.prefixed_decl_is_container(*prefix, row.name) {
                    Ty::named(self.intern("Type"))
                } else {
                    // A three-segment prefixed tear-off (`p.C.new`) falls
                    // through to the function faces (the ctor table owns
                    // the `PrefixedContainer` receiver).
                    self.ctor_tear_off_ty(row).unwrap_or(Ty::UNKNOWN)
                }
            }
            RowShape::Member { .. } | RowShape::Bare(BareInfo::Plain { .. }) => {
                let ty = self.receiver_static_ty(row);
                if ty != Ty::UNKNOWN {
                    return ty;
                }
                // A READ spelled as a callable's name is a tear-off whose
                // static type is its function face (`ConstructorReference`
                // / `PropertyAccess` / `Identifier` `staticType`): the
                // accessor face above answered getters/fields, so what
                // remains is the ctor selector, the member walk, and the
                // bare function signature.
                self.ctor_tear_off_ty(row)
                    .or_else(|| self.member_tear_off_ty(row))
                    .or_else(|| self.operand_tear_off_ty(row))
                    .unwrap_or(Ty::UNKNOWN)
            }
            RowShape::Value => self.value_operand_ty(row).unwrap_or(Ty::UNKNOWN),
            _ => Ty::UNKNOWN,
        }
    }

    /// Whether the prefix-qualified name `prefix.name` resolves to a
    /// container or alias declaration — the TypeLiteral face of a `p.C`
    /// read (value positions of variables and functions keep their
    /// declared type instead).
    fn prefixed_decl_is_container(&self, prefix: StringId, name: StringId) -> bool {
        let dartq_resolution::Lookup::Found(NamespaceEntry::Decl(file)) = self
            .namespaces
            .lookup_prefixed(self.graph, self.lib, prefix, name)
        else {
            return false;
        };
        let Some(decl) = self.index.get(file).and_then(|f| f.decl(name)) else {
            return false;
        };
        !matches!(
            decl.kind,
            MicroKind::TopLevelVar
                | MicroKind::TopLevelFn
                | MicroKind::Prefix
                | MicroKind::TypeParam
        )
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
            // A record (or function) `on` type has NO honest name — the
            // interned `on_ty` is the only spelling `this` owns, and the
            // extension body's record accessors (`that.$1`) resolve through
            // it exactly like the named face above.
            if let Some(face) = face
                && face.on_ty != Ty::Unknown
            {
                return face.on_ty;
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
        // A bare callable read initializer (`var v = fi`, `var t = m`
        // inside the container): the `is`-operand face
        // [`Self::operand_tear_off_ty`] already spells — the top-level,
        // local-function, and enclosing-member signatures as the function
        // type the read's static type IS (the oracle's
        // `PropertyAccess`/`Identifier.staticType` resolves the callable
        // element, whose `type` is the function type). Without it a
        // `var`-inferred local bound `Unknown`, and every `v is T` verdict
        // under-reported where the oracle's function-typed operand
        // decides (`v is Function` is constant-true).
        if let Some(ty) = self.operand_tear_off_ty(row) {
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

    /// The `??` initializer pair's inferred type (`resolveIfNull`,
    /// upstream Dart SDK source,
    /// pkg/analyzer/lib/src/dart/resolver/binary_expression_resolver.dart:
    /// 69-109): the declaration window holds exactly the two operand rows
    /// — the ifnull leaves no result row ([`Self::end_if_null`] only
    /// stages the dead-coalesce verdict) — with the operator in the gap
    /// between them. The local's type is `LUB(NonNull(lhs), rhs)`:
    /// nullable exactly when the right side is (`int? ?? int?` stays
    /// `int?`), the left's non-null form when the right's face is unknown
    /// (the official folds an invalid operand away,
    /// `LUB(NonNull(T), Invalid) = NonNull(T)`), and the right's face when
    /// the left's is (the same fold, mirrored). Only same-head,
    /// same-argument, NON-NULL faces bind: a cross-head or widened least
    /// upper bound stays unbound — the under-report posture every untyped
    /// initializer keeps (binding a nullable inference fires the
    /// return/unchecked verdicts before the write-side re-decides run, the
    /// `[]`-fold arm's rule below the declaration bind).
    /// The byte position of the `??` operator that directly follows a
    /// row's effective expression end — `None` when no `??` does. An
    /// invoked callee's expression extends through its argument list's
    /// closing `)` (string and character literals inside the scan are
    /// skipped whole so a quoted paren cannot break the balance); a
    /// generic callee's `<...>` group rides the same extension when a
    /// `(` follows its matching `>`. The operand ends at the closer;
    /// whitespace then `??` decides.
    fn row_ifnull_operator_at(&self, row: &Row) -> Option<u32> {
        let mut at = row.range.end();
        if matches!(row.intent, Intent::Invoke) {
            if self.byte_at(at) == Some(b'<')
                && let Some(close) = self.matching_angle(at)
                && self.byte_at(close + 1) == Some(b'(')
            {
                at = close + 1;
            }
            if self.byte_at(at) == Some(b'(') {
                at = self.matching_paren(at)?;
                at += 1;
            }
        }
        while matches!(self.byte_at(at), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            at += 1;
        }
        (self.byte_at(at) == Some(b'?') && self.byte_at(at + 1) == Some(b'?')).then_some(at)
    }

    /// The byte offset of the `)` matching the `(` at `at` — literals
    /// skipped whole, escapes honored; `None` when unbalanced.
    fn matching_paren(&self, at: u32) -> Option<u32> {
        let mut depth = 0u32;
        let mut i = at;
        while let Some(b) = self.byte_at(i) {
            match b {
                b'(' => depth += 1,
                b')' => {
                    depth = depth.checked_sub(1)?;
                    if depth == 0 {
                        return Some(i);
                    }
                }
                b'\'' | b'"' => {
                    let quote = b;
                    i += 1;
                    while let Some(c) = self.byte_at(i) {
                        if c == b'\\' {
                            i += 2;
                            continue;
                        }
                        if c == quote {
                            break;
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

    /// The byte offset of the `>` matching the `<` at `at` (no literal
    /// skipping — a type-argument group holds no strings).
    fn matching_angle(&self, at: u32) -> Option<u32> {
        let mut depth = 0u32;
        let mut i = at;
        while let Some(b) = self.byte_at(i) {
            match b {
                b'<' => depth += 1,
                b'>' => {
                    depth = depth.checked_sub(1)?;
                    if depth == 0 {
                        return Some(i);
                    }
                }
                _ => {}
            }
            i += 1;
        }
        None
    }

    /// Whether the bytes in `[start, end)` spell nothing but a receiver
    /// chain — identifier characters, `.`, `?.`, whitespace. A cascade
    /// (`)..m = v..n = `), a parenthesized operand (`(a ?? `), or any
    /// other operator between the window's first row and the left
    /// operand marks a NESTED `??`, never the declaration's own.
    fn is_plain_receiver_prefix(&self, start: u32, end: u32) -> bool {
        let mut at = start;
        while at < end {
            match self.byte_at(at) {
                Some(b) if b.is_ascii_alphanumeric() || b == b'_' || b == b'$' => at += 1,
                Some(b'.') => at += 1,
                Some(b'?') if self.byte_at(at + 1) == Some(b'.') => at += 2,
                Some(b' ' | b'\t' | b'\n' | b'\r') => at += 1,
                _ => return false,
            }
        }
        true
    }

    /// Whether the bytes in `start..end` spell exactly one plain `=`
    /// (whitespace around it) — the gap between a write target and its
    /// right side in a simple assignment expression.
    pub(crate) fn plain_assign_between(&self, start: u32, end: u32) -> bool {
        let mut seen = false;
        for at in start..end {
            match self.byte_at(at) {
                Some(b' ' | b'\t' | b'\n' | b'\r') => {}
                Some(b'=') if !seen => seen = true,
                _ => return false,
            }
        }
        seen
    }

    pub(crate) fn ifnull_initializer_ty(&mut self, mark: usize) -> Option<Ty> {
        // The right operand is the window's LAST row; the left operand is
        // the last row whose end the `??` operator directly follows — a
        // call left side leaves its argument rows on the stack (the exact
        // `mark + 2` gate rejected those), and the operator's own bytes
        // identify the boundary whatever the row count.
        let rhs = self.rows.last().cloned()?;
        let upper = self.rows.len().saturating_sub(1);
        if mark >= upper {
            return None;
        }
        let lhs = {
            // The `??`'s left operand: the row whose effective end the
            // operator directly follows. An invoked callee row's
            // expression extends through its argument list's closing
            // paren — no result row exists for a send, the callee row IS
            // the operand (`int.tryParse(match.group(2) ?? '0') ?? 0`
            // pairs the OUTER `??` with the tryParse callee). Candidates
            // compare by the `??`'s own byte position, not row order: a
            // `??` nested inside the argument list follows an EARLIER
            // byte, so the maximum picks the outermost operator.
            let mut best: Option<(u32, usize)> = None;
            for (index, row) in self.rows[mark..upper].iter().enumerate() {
                let Some(question_at) = self.row_ifnull_operator_at(row) else {
                    continue;
                };
                if best.is_none_or(|(at, _)| question_at > at) {
                    best = Some((question_at, index));
                }
            }
            let (question_at, lhs_index) = best?;
            let lhs = self.rows[mark + lhs_index].clone();
            // The located `??` must be the window's TOP-LEVEL operator. A
            // `??` nested inside a larger initializer (`(g?.x.length ?? 0)
            // > 1 || (s?.x.length ?? 0) > 1`) pairs its left `int` with
            // the `||` result row — the same-head arms decline on the head
            // mismatch, but the different-head nullability arm below
            // would bind the operand's head onto the whole initializer's
            // local (a `bool`-valued `||` chain typed `int`). The test is
            // the operator's bracket depth from the window start: scan
            // forward from the `??` tracking `()`/`[]`/`{}` — the right
            // operand's own bytes stay at or above that depth; a closer
            // dropping BELOW it ends the enclosing construct, and real
            // content after that (a `>`, `||`, any operator) proves the
            // `??` was nested, while `;`/`,` (or only closers up to one)
            // prove the initializer ended with this operand.
            let mut depth = 0i32;
            let mut at = self.rows[mark].range.offset;
            while at < question_at {
                match self.byte_at(at) {
                    Some(b'(' | b'[' | b'{') => depth += 1,
                    Some(b')' | b']' | b'}') => depth -= 1,
                    _ => {}
                }
                at += 1;
            }
            // `depth` is the operator's own bracket depth from the
            // window start; the right operand lives AT that depth. A
            // closer dropping below it ends the enclosing construct (an
            // argument list, a parenthesized operand) — only `;`/`,` or
            // further closers may follow before real content does.
            let base = depth;
            let mut at = question_at + 2;
            let mut below = false;
            loop {
                match self.byte_at(at) {
                    None => break,
                    Some(b' ' | b'\t' | b'\n' | b'\r') => {}
                    Some(b';' | b',') if depth <= base || below => break,
                    Some(b')' | b']' | b'}') => {
                        depth -= 1;
                        if depth < base {
                            below = true;
                        }
                    }
                    Some(b'(' | b'[' | b'{') if !below => depth += 1,
                    Some(_) if below => return None,
                    Some(_) => {}
                }
                at += 1;
                if at > question_at + 4_096 {
                    return None;
                }
            }
            lhs
        };
        // The fold answers only a TOP-LEVEL `??`: the bytes from the
        // window's first row to the left operand may spell nothing but
        // the operand's own receiver chain (identifiers, `.`, `?.`). A
        // `??` nested inside the window — a cascade write
        // (`C.c(this)..a = x..b = y ?? false`) parks its operand rows
        // beside the receiver's, and a member suffix
        // (`(a ?? b ?? c).offset`) parks the chain's — pairing those
        // with `rows.last()` types the declaration from a nested operand
        // pair (`context` becomes `bool`, `enterOffset` becomes the
        // never-null left head).
        if !self.is_plain_receiver_prefix(self.rows[mark].range.offset, lhs.range.offset) {
            return None;
        }
        let lhs_face = self
            .initializer_static_ty(&lhs)
            .filter(|ty| *ty != Ty::UNKNOWN)
            .or_else(|| {
                let derived = self.initializer_read_ty(&lhs);
                (derived != Ty::UNKNOWN).then_some(derived)
            })
            .unwrap_or(Ty::UNKNOWN);
        let rhs_face = self
            .initializer_static_ty(&rhs)
            .filter(|ty| *ty != Ty::UNKNOWN)
            .or_else(|| {
                let derived = self.initializer_read_ty(&rhs);
                (derived != Ty::UNKNOWN).then_some(derived)
            })
            .unwrap_or(Ty::UNKNOWN);
        let face = match (lhs_face, rhs_face) {
            (a, b) if a == b => a,
            (Ty::UNKNOWN, b) => b,
            // An unresolved right operand poisons the join rather than
            // non-nullifying the left: upstream's `resolveIfNull` computes
            // `LUB(NonNull(T1), T2)` against the right operand's own type,
            // and an unresolved `T2` is `InvalidType` — the variable
            // inherits the invalid face and every downstream check stays
            // silent (probed 3.47.1: `_dartToDrift[Dart] ??
            // values.whereType<X>().singleOrNull` keeps the `== null`
            // comparison alive). Non-nullifying here wrote the left head
            // into the declaration (`type == null` over-reported
            // `unnecessary_null_comparison`; `offset:` arguments reported
            // `argument_type_not_assignable`).
            (_, Ty::UNKNOWN) => return None,
            (
                Ty::Named { name: l, .. },
                Ty::Named {
                    name: r,
                    args: ra,
                    nullable: rn,
                },
            ) if l == r => {
                // Upstream folds every same-head pair: `resolveIfNull`
                // computes `LUB(NonNull(T1), T2)`, and two instantiations
                // of the same generic head LUB to that head instantiated
                // (`List<TableColumn> ?? List<Column>.toList()` keeps
                // `List`-of-the-join, never `Object`). The name-level
                // model carries the right side's argument tree — the
                // materialized side deciding nullability — as the join's
                // approximation; an arg-wise LUB beyond it stays in the
                // under-report posture.
                Ty::Named {
                    name: r,
                    nullable: rn,
                    args: ra,
                }
            }
            // Different heads: the join's HEAD needs the hierarchy (a
            // supertype walk this name-level fold does not own), but its
            // NULLABILITY is operand-decided — `T1 ?? T2` is nullable only
            // when both sides are (`resolveIfNull` joins `NonNull(T1)`).
            // The flow layer already widened the local's write to the left
            // head (nullable), so re-binding the left head with the joined
            // nullability changes nothing about member resolution and only
            // corrects nullness: a non-null right operand makes every
            // downstream dereference safe, and the oracle's LUB stays
            // silent there (`constructorName ?? beginToken` reads `.end`
            // clean). Both-nullable pairs keep the widened face via the
            // final nullable gate's None.
            (
                Ty::Named {
                    name: l,
                    nullable: ln,
                    args: la,
                },
                Ty::Named { nullable: rn, .. },
            ) if !ln || !rn => Ty::Named {
                name: l,
                nullable: ln && rn,
                args: la,
            },
            _ => return None,
        };
        (!matches!(face, Ty::Named { nullable: true, .. })).then_some(face)
    }

    /// Whether a named head is a compiler-known core container the
    /// invalid-poison face never applies to (a mounted run resolves them
    /// always; a synthetic unit test without the SDK decides real
    /// argument pairs over them regardless).
    fn core_head(&self, name: StringId) -> bool {
        matches!(
            self.strings.resolve(name),
            "List" | "Map" | "Set" | "Iterable" | "Object" | "Future" | "Stream" | "Function"
        )
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
        let Some(entry) = self.resolved_member(container, row.name) else {
            // The receiver is a container and the selector names no
            // member — the constructor-selector face (a `Receiver::
            // Container`/`PrefixedContainer` recv resolved through
            // `ctor_arity_target`'s declared-constructor table) is the
            // tear-off's own, not a member. `ctor_tear_off_ty` covers
            // it; every other recv shape stays `None`.
            return self.ctor_tear_off_ty(row);
        };
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
        // The declaration's own parameter group rides the shape (#2878):
        // the tear-off's display spells it (`C m<T>(T)` reads
        // `T Function<T>(T)`); a `Dynamic` bound is the unbounded face
        // (never spelled). A pre-field payload row carries the count
        // without names — the count-only form keeps its exact identity.
        let type_params: Vec<(StringId, Option<Ty>)> = entry
            .type_param_names
            .iter()
            .copied()
            .zip(entry.type_param_bounds.iter().copied())
            .map(|(name, bound)| (name, (bound != Ty::Dynamic).then_some(bound)))
            .collect();
        let mut args = self.boundary.args.borrow_mut();
        let fn_ty = if type_params.is_empty() && entry.type_params > 0 {
            args.function(
                entry.type_params,
                entry.return_ty,
                &positional,
                &named,
                false,
            )
        } else {
            args.function_generic(&type_params, entry.return_ty, &positional, &named, false)
        };
        drop(args);
        Some(fn_ty)
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

    /// A type-parameter head replaced by its captured bound (#2349): the
    /// oracle types an operand by the parameter itself, whose interface IS
    /// the bound's — `void f<T extends num>(T t)` reads `t + 1` as `num`,
    /// `S extends int` as `int`, `U extends double` as `double` (probed
    /// 3.13.1), and the operator lookup runs on that same interface. Every
    /// other type passes through untouched. A parameter with NO captured
    /// bound keeps its written spelling: the implicit `Object?` bound is no
    /// `num` face, and those shapes stay on the under-report side (their
    /// own row family is separate). The operand's written nullability rides
    /// onto the bound (`T?` over `T extends num` reads `num?`).
    pub(crate) fn bound_face(&self, ty: Ty) -> Ty {
        let Ty::Named { name, nullable, .. } = ty else {
            return ty;
        };
        if !self.is_scope_param(name) {
            return ty;
        }
        let Some(bound) = self.param_bound(name) else {
            return ty;
        };
        if bound == Ty::UNKNOWN || bound == Ty::Dynamic {
            return ty;
        }
        if nullable {
            Self::nullable_form(bound)
        } else {
            bound
        }
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
        let implicit_object = || Ty::Named {
            name: self.intern("Object"),
            nullable: true,
            args: ArgListId::EMPTY,
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
            return self.declared_strict_non_nullable(ty);
        };
        if !self.is_scope_param(name) {
            return self.declared_strict_non_nullable(ty);
        }
        match self.param_bound(name) {
            None => false,
            Some(bound) => {
                !self.ty_mentions_param(bound) && self.declared_strict_non_nullable(bound)
            }
        }
    }

    /// The model's argument-aware `isStrictlyNonNullable`
    /// (`FileElements::is_strictly_non_nullable_in`): a non-nullable
    /// `FutureOr<T>` is strictly non-nullable exactly when `T` is, and an
    /// extension type exactly when it declares interfaces. Every face the
    /// bound-aware delegate above does not decide itself routes here, so
    /// the union and extension-type arms live in one place (the model). A
    /// file the index never recorded keeps the plain shape face.
    ///
    /// `ty` arrives minted in the engine's own [`ArgPool`]
    /// (`boundary.pool()` — every row/`operand_static_ty`/`param_bound`
    /// face lives there), never the unit model's `type_args`: the helper
    /// must resolve the `FutureOr` argument list through the pool that
    /// minted the ids, so the diagnosis pool crosses the call boundary as
    /// the explicit `args` parameter (#3531 — indexing the unit pool with
    /// an engine slot read another universe and panicked).
    fn declared_strict_non_nullable(&self, ty: Ty) -> bool {
        match self.index.get(self.file) {
            Some(elements) => {
                elements.is_strictly_non_nullable_in(self.strings, ty, &self.boundary.pool())
            }
            None => ty.is_strictly_non_nullable(),
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
        if ty == Ty::Void {
            // The `!`-postfix operand's own face (upstream
            // `visitPostfixExpression`'s `checkForUseOfVoidResult(node)` on
            // the whole `x!`, error_verifier.dart:1692 →
            // error_detection_helpers.dart:245-258): a `void`-typed
            // operand's value is consumed by the assertion — the oracle
            // reports `use_of_void_result` at the operand-plus-`!` span
            // before the null-check faces ever ask their question. The
            // span opens at the operand EXPRESSION's head — a member
            // operand's `x.m()!` spans from `x`, not the `m` selector.
            let head = self.rows.last().and_then(|row| {
                self.property_access_span(row.range)
                    .map(|span| span.offset)
            });
            let range = SourceRange {
                offset: head.unwrap_or(read_span.offset),
                length: at.end() - head.unwrap_or(read_span.offset),
            };
            self.emit(SemanticCode::UseOfVoidResult, range, "", "");
        } else if ty == Ty::Null {
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
            let nullable_params: Box<[StringId]> = self
                .type_params
                .iter()
                .chain(&self.pending_type_params)
                .copied()
                .filter(|&name| {
                    !self.strictly_non_nullable(Ty::Named {
                        name,
                        nullable: false,
                        args: ArgListId::EMPTY,
                    })
                })
                .collect();
            self.null_assert_staged
                .push((read_span, at, nullable_params));
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
        // An untyped operand row still owns a static type the oracle answers
        // (`leftNode.typeOrThrow`): the bare container literal (`C == null`)
        // types `Type`, the `this`/`super` receivers their container face,
        // the prefixed `p.C` TypeLiteral `Type` — the same row-shape read
        // [`Self::type_check_operand_ty`] takes for the `is` twin.
        let operand_ty = if self.null_aware_members.contains(&operand.range.offset) {
            Self::nullable_form(operand.ty)
        } else if operand.ty != Ty::UNKNOWN {
            operand.ty
        } else {
            self.untyped_operand_ty(operand)
        };
        // An invalid-typed operand suspends the nullability question: the
        // official's classification over `InvalidType` answers nothing —
        // no `unnecessary_null_comparison`, no constant verdict, no
        // promotion (oracle-probed: `Und x` and `Und? x` both silent on
        // `x == null` / `x != null`).
        if self.written_ty_invalid(operand_ty) {
            return;
        }
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
        // null` / `null ==`), never the compared operand alone. The
        // operator token itself is read from the table: an operand row
        // ends at its own node's head face, so a completed send's row
        // ends at the callee identifier and the gap over its argument
        // list (`intFunction() == null`) must not widen the anchor.
        let op = self
            .comparison_operator_span(operand.range.end(), null_row.range.offset)
            .or_else(|| self.comparison_operator_span(null_row.range.end(), operand.range.offset));
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

    /// The `==`/`!=` token inside the gap `[from, to)` the two operand
    /// rows leave: the operator is the anchor the invariant-null-
    /// comparison row opens at (`reportStartEnd`, the operator through the
    /// `null` operand). Reading the token beats trimming the whole gap —
    /// a send operand's row ends at the callee identifier, so the gap also
    /// holds the invocation's argument list (`intFunction() == null`), and
    /// a comment between the leaves is invisible to the token table either
    /// way. `None` when the interval holds no equality token (a nested
    /// expression the rows do not cover — the caller keeps its fallback).
    fn comparison_operator_span(&self, from: u32, to: u32) -> Option<SourceRange> {
        let tokens = self.tokens.as_slice();
        let head = tokens.partition_point(|token| token.utf16_offset < from);
        tokens[head..]
            .iter()
            .take_while(|token| token.utf16_offset < to)
            .find(|token| {
                matches!(
                    token.kind,
                    dartq_scanner::TokenKind::EqEq | dartq_scanner::TokenKind::BangEq
                )
            })
            .map(|token| SourceRange {
                offset: token.utf16_offset,
                // `==`/`!=` are ASCII: the byte length is the UTF-16 length.
                length: token.byte_len,
            })
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
            ..
        }) = row.shape
        {
            // The creation's INSTANCE type, never the bare head (#3063):
            // the created row's own solved face (`pending_send_ty`) or the
            // written `<…>` group keyed on the row's offset
            // ([`Self::creation_row_ty`]) instantiates the container's
            // parameters, so `Box<int>(1) + make()` resolves the operator's
            // `Box<int>` formals and return instead of leaving `T` free.
            // An unwritten group (`C()`) keeps the bare head — the
            // to-bounds residual is the display face's, not the operand's.
            if let Ty::Named { args, .. } = row.ty
                && args != ArgListId::EMPTY
            {
                return row.ty;
            }
            return self
                .creation_row_ty(name, false, row.range.offset)
                .unwrap_or_else(|| Ty::named(name));
        }
        let ty = self.receiver_static_ty(row);
        // A literal operand's row carries no type at push
        // ([`Self::push_value_row`] types only `null`): the token's own
        // bytes name the primitive — `5 - offset`'s left operand answers
        // `int`, the fold resolves the operator signature, and the
        // assignment staging reads the binary's result type (the `??=`
        // promotion family: `length ??= 5 - offset` left the target
        // unpromoted because the fold's result stayed unknown and the
        // write fell to the invalid-RHS no-promotion face).
        if ty == Ty::UNKNOWN
            && matches!(row.shape, RowShape::Value)
            && row.range.length > 0
            && let Some(lit) = self.numeric_literal_face(row.range)
        {
            return lit;
        }
        ty
    }

    /// `ty` when the engine typed it, else the numeric literal face of the
    /// row it came from (#2349): a literal's row carries no type at push,
    /// so every numeric face that reads a value row would otherwise see a
    /// gap — `3.clamp(0, i)` keeps the declared `num` instead of the
    /// oracle's `int`, and the `int` sink reports a `num` it should not.
    /// Anything that is not a numeric literal keeps `Ty::UNKNOWN`.
    fn numeric_literal_or(&self, ty: Ty, range: SourceRange) -> Ty {
        if ty != Ty::UNKNOWN {
            return ty;
        }
        self.numeric_literal_face(range).unwrap_or(Ty::UNKNOWN)
    }

    /// The primitive face of a token whose bytes spell a numeric literal:
    /// `int` for a hex or plain-digit run, `double` once a `.`, `e`, or
    /// `E` marker appears outside the hex prefix. The digit-first gate
    /// keeps every word-shaped token (`true`, `null`, identifiers) out;
    /// anything else answers `None` (the unknown face stands).
    fn numeric_literal_face(&self, range: SourceRange) -> Option<Ty> {
        let mut bytes = Vec::with_capacity(range.length.min(64) as usize);
        for i in range.offset..range.end() {
            bytes.push(self.byte_at(i)?);
        }
        if !bytes.first().is_some_and(|b| b.is_ascii_digit())
            || !bytes
                .iter()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_'))
        {
            return None;
        }
        let is_hex = bytes.len() > 2 && bytes[0] == b'0' && (bytes[1] | 0x20) == b'x';
        let has_double_marker = !is_hex && bytes.iter().any(|b| matches!(b, b'.' | b'e' | b'E'));
        Some(Ty::named(if has_double_marker {
            self.members.primitive_ids[1]
        } else {
            self.members.primitive_ids[0]
        }))
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

    /// The operator signature an applicable extension declares for a
    /// potentially nullable receiver: `int? x; x + null` over `extension
    /// on int? { int operator +(Null o) => 0; }`, where the extension owns
    /// the operator and the oracle reports nothing.
    ///
    /// The nullable branch of the property consult runs the `Object` face,
    /// then the applicable-extension face, and only then reads the
    /// nullability verdict (upstream Dart SDK source,
    /// pkg/analyzer/lib/src/dart/resolver/type_property_resolver.dart:
    /// 98-110 — `_lookupExtension(receiverType)` precedes the
    /// `uncheckedOperatorInvocationOfNullableValue` report), so a face
    /// whose `on` type covers the nullable spelling resolves before any
    /// dereference is read. Only the covering verdict counts:
    /// [`ExtensionApp::UnsafeSingle`] is the non-null projection alone,
    /// which `applicableTo`'s subtype ask rejects for a nullable receiver
    /// (`int? <: int` fails) — `extension on int` over `int? x; x + null`
    /// keeps the dereference report and the recovery `num` formal.
    ///
    /// `None` = no covering face (the interface face and the nullability
    /// verdict decide). `Some((formal, return))` = the extension owns the
    /// operator; either half is [`Ty::UNKNOWN`] when the name level cannot
    /// read the declaration — a generic face spells its own parameters
    /// (`extension E<T> on T?`), and a spelling several declarations in the
    /// winning file claim answers no single signature. The verdict stays
    /// authoritative in that case, so callers report nothing: the operator
    /// RESOLVED, and the member faces below would invent
    /// `undefined_operator`/`argument_type_not_assignable` rows against a
    /// face the oracle never consults.
    pub(crate) fn nullable_extension_operator_sig(&mut self, recv: Ty, op: StringId) -> Option<(Ty, Ty)> {
        let Ty::Named {
            name,
            nullable: true,
            ..
        } = recv
        else {
            return None;
        };
        // An unbound type-parameter head resolves nothing at name level
        // (its bound already reached the caller through `bound_face`).
        if self.type_params.contains(&name) || self.pending_type_params.contains(&name) {
            return None;
        }
        if !matches!(
            self.extension_application(name, recv, op, false, true),
            ExtensionApp::Single
        ) {
            return None;
        }
        let unknown = (Ty::UNKNOWN, Ty::UNKNOWN);
        // The application latched its winning face's declaring file, its
        // spellings and name (the single-slot `ext_ref`, written only at a
        // single-face conclusion). One face per declaration means the latch
        // plus the spelled member identifies it.
        let Some((face_file, spelling, face_name, kind)) = self.members.ext_ref.get() else {
            return Some(unknown);
        };
        // A field's read face declares no operator invocation.
        if kind != MemberKind::Method {
            return Some(unknown);
        }
        let Some(elements) = self.index.get(face_file) else {
            return Some(unknown);
        };
        let mut winners = elements.extension_faces().iter().filter(|face| {
            face.members.contains_key(&spelling)
                && if face_name == StringId::NONE {
                    face.name.is_none()
                } else {
                    face.name == Some(face_name)
                }
        });
        let (Some(face), None) = (winners.next(), winners.next()) else {
            return Some(unknown);
        };
        // A generic face's declared spelling mentions its own parameters,
        // which the name level cannot instantiate against the receiver.
        if !face.type_param_names.is_empty() {
            return Some(unknown);
        }
        let Some(entry) = face.members.get(&spelling) else {
            return Some(unknown);
        };
        let Some(formal) = entry
            .formals
            .as_deref()
            .and_then(|formals| formals.positional.first())
        else {
            return Some(unknown);
        };
        // A face declared in another file interns its types in that file's
        // pool: project both halves into the diagnosing pool.
        let boundary = &self.boundary;
        Some((
            boundary.ty(face_file, formal.ty),
            boundary.ty(face_file, entry.return_ty),
        ))
    }

    /// The resolved binary-operator signature of `left OP right`: the
    /// `(param type, result type)` pair the oracle's resolved element
    /// declares (`_resolveUserDefinableType`,
    /// binary_expression_resolver.dart:479-515 reads
    /// `element.returnType`). The arm order mirrors the oracle's property
    /// resolution: an applicable extension over a nullable receiver first
    /// (the nullable branch consults it before reading the dereference,
    /// [`Self::nullable_extension_operator_sig`]), then an exact SDK row
    /// (`int.&`, `String.+`), then the user container's declared operator,
    /// then the `num`-inheritance fiction for the undeclared real-core
    /// `int`/`double` receivers (the SDK declarations are external,
    /// unreachable by the member walk). Unresolved receivers, absent
    /// operators (`undefined_operator` reports its own row), `dynamic`, and
    /// `Never` answer `None` — the under-report direction.
    pub(crate) fn binary_operator_sig(&mut self, op: &str, left: &Row) -> Option<(Ty, Ty)> {
        // A type-parameter receiver resolves the operator on its BOUND's
        // interface (#2349: `S extends int` answers `int.+`, `T extends
        // num` answers `num.+`); an unbound parameter keeps the written
        // head and falls to the `None` below.
        let left_ty = self.bound_face(self.equality_operand_ty(left));
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
        // The nullable receiver's ordering: the extension face resolves the
        // operator before the head's interface face stands in for it.
        let spelling = self.intern(op);
        if let Some(signature) = self.nullable_extension_operator_sig(left_ty, spelling) {
            return Some(signature);
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
            // A method entry with no positional formal cannot serve the
            // BINARY invocation — `int` declares only the unary
            // `int operator -()` (the binary `-` is inherited from `num`),
            // so a formal-less hit is a unary-face miss for this lookup:
            // the `int`/`double` receivers fall through to the `num`
            // fiction below instead of the whole signature answering
            // `None` (`output.length - startDataLength` folded unknown).
            if let Some(formals) = entry.formals.as_deref()
                && let Some(formal) = formals.positional.first()
            {
                // The declared signature spells the DECLARING container's
                // parameters (`Box<T> operator +(Box<T>)`); the receiver's
                // written arguments bind them (#3063) — `Box<int>` answers
                // `(Box<int>) -> Box<int>`, so the fold's node type and the
                // operand check both read the instantiated face instead of
                // the raw `Box<T>` (which matched nothing downstream and
                // leaked the parameter spelling into every sink). The
                // environment is the one a member invocation derives
                // ([`Self::member_recv_env`]: an own member pairs the
                // container's parameters with the receiver's arguments, an
                // inherited one composes the clause hops); a receiver with
                // no environment (raw spelling, unresolvable container)
                // keeps the declared face.
                let declaring = self.member_faces.last_origin().unwrap_or(name);
                let keys: FxHashMap<StringId, Ty> = self
                    .member_recv_env(left_ty, declaring, &entry)
                    .map(|env| env.into_iter().collect())
                    .unwrap_or_default();
                return Some((
                    self.substitute_ty(formal.ty, &keys),
                    self.substitute_ty(entry.return_ty, &keys),
                ));
            }
        }
        // The core-fiction arm: `int`/`double` inherit `num`'s arithmetic
        // and relational face (the SDK declarations are external, the
        // member walk stops at the receiver's own most-derived hit —
        // dart:core `int` declares only the unary `int operator -()`, so
        // the binary `-` never reaches `num`'s declaration through the
        // walk; the `num.-` signature answers `x - y` on `int` receivers
        // here); `String`/`bool` are not `num` receivers and never take
        // this fallback. The gate is the precise shadow test
        // ([`Self::refine_numeric_binary`]'s memo): the fiction serves the
        // head unless a USER container shadows the core name while `num`
        // itself is absent (`class int {}` subtypes nothing numeric) — the
        // real dart:core pair resolves both names, and the earlier
        // any-resolution gate would skip the fiction in every real package
        // (dart:core auto-import resolves `int`), folding every `int`
        // subtraction unknown.
        if recv_text == "int" || recv_text == "double" {
            let num_id = self.members.primitive_ids[2];
            let container = |head| {
                resolve_container(
                    self.graph,
                    self.namespaces,
                    self.index,
                    self.lib,
                    self.file,
                    head,
                )
            };
            let shadowed =
                container(name).is_some() && (name == num_id || container(num_id).is_none());
            if !shadowed {
                if let Some((_, _, param, ret)) = CORE_BINARY_OPERATORS
                    .iter()
                    .find(|(recv, o, ..)| *o == op && *recv == "num")
                {
                    return Some((Ty::named(self.intern(*param)), Ty::named(self.intern(*ret))));
                }
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
    /// The numeric CALL refinement (`_refineNumericInvocationTypeNullSafe`
    /// and its `clamp` sibling, type_system.dart:2058-2165): a member
    /// invoke of `remainder` follows the binary ladder over its one
    /// argument, and `clamp(e2, e3)` answers `int` when the receiver and
    /// both arguments all read `int?` (the `?`-checked error-recovery
    /// equivalence), `double` when all read `double?`, the declared
    /// `num` otherwise — the receiver must be `num?`-family for any of
    /// it. The typed arguments are live exactly at the argument group's
    /// close, so the refined return stashes there (keyed on the callee
    /// row's range offset) and [`Self::callee_return_ty`] adopts it.
    pub(crate) fn refine_numeric_call_return(
        &mut self,
        callee: usize,
        row: &Row,
        args: &[(Option<StringId>, Ty, bool, SourceRange)],
    ) {
        let name = self.strings.resolve(row.name);
        if !matches!(name, "clamp" | "remainder") {
            return;
        }
        // The receiver's static type: the pre-merge callee's receiver row
        // sits directly below (`_position.clamp(...)`), typed by its
        // declared face.
        let Some(below) = callee
            .checked_sub(1)
            .and_then(|index| self.rows.get(index))
            .cloned()
        else {
            return;
        };
        let recv = match &below.shape {
            RowShape::Bare(info) => Engine::receiver_of_bare(*info),
            RowShape::Member { recv } => *recv,
            // A literal receiver (`3.clamp(0, i)`, `2.5.remainder(x)`)
            // rides a Value row — the literal face below answers it.
            RowShape::Value => Receiver::Unknown,
            _ => return,
        };
        // The receiver's static type (#2349): the declared face, a numeric
        // literal's own primitive — a literal row carries no type at push,
        // so `3.clamp(0, i)` would otherwise read the receiver as a gap and
        // keep the declared `num` where the oracle answers `int` — or a
        // type parameter's BOUND.
        let recv_ty = match recv {
            Receiver::Typed { ty, .. } => self.numeric_literal_or(ty, below.range),
            _ => match self.numeric_literal_face(below.range) {
                Some(lit) => lit,
                None => return,
            },
        };
        let recv_ty = self.bound_face(recv_ty);
        let positional: Vec<Ty> = args
            .iter()
            .filter(|(label, ..)| label.is_none())
            .map(|(_, ty, _, range)| self.numeric_literal_or(*ty, *range))
            .collect();
        let (int, double, num) = (
            self.members.primitive_ids[0],
            self.members.primitive_ids[1],
            self.members.primitive_ids[2],
        );
        let named_q = |head| Ty::Named {
            name: head,
            nullable: true,
            args: ArgListId::EMPTY,
        };
        // The ladder owns `num`-family receivers only — a `BigInt
        // .remainder` or a user class's own `remainder` keeps the member
        // walk's declared return (`_refineNumericInvocationTypeNullSafe`'s
        // `T <: num?` gate; the `num`-typed `remainder` of a non-`num`
        // receiver is never this face's answer).
        if !self.name_subtype(recv_ty, named_q(num)) {
            return;
        }
        let refined = match name {
            "remainder" if positional.len() == 1 => {
                let declared = Ty::named(num);
                self.refine_numeric_binary(
                    "remainder",
                    recv_ty,
                    OperandFace::Typed(positional[0]),
                    declared,
                )
            }
            "clamp" if positional.len() == 2 => {
                let all = |tys: &[Ty], head| {
                    tys.iter()
                        .all(|t| *t != Ty::Never && self.name_subtype(*t, named_q(head)))
                };
                let operands = [recv_ty, positional[0], positional[1]];
                if all(&operands, int) {
                    Ty::named(int)
                } else if all(&operands, double) {
                    Ty::named(double)
                } else {
                    Ty::named(num)
                }
            }
            _ => return,
        };
        if refined != Ty::UNKNOWN {
            self.refined_call_returns.insert(row.range.offset, refined);
        }
    }

    pub(crate) fn refine_numeric_binary(
        &self,
        op: &str,
        left_ty: Ty,
        right: OperandFace,
        declared: Ty,
    ) -> Ty {
        if !matches!(op, "+" | "-" | "*" | "%" | "remainder") {
            return declared;
        }
        // A type-parameter receiver refines through its BOUND (#2349): the
        // oracle's gates (`T <: num?` / `T <: double?` / `T <: int?`) run
        // on the parameter's interface, so `T extends num` over an `int`
        // answers `num`, `S extends int` over an `int` answers `int`, and
        // `U extends double` answers `double` (probed 3.13.1). The right
        // operand canonicalizes the same way — the compound-assignment
        // caller hands the written head through unreduced.
        let left_ty = self.bound_face(left_ty);
        let right = match right {
            OperandFace::Typed(ty) => OperandFace::Typed(self.bound_face(ty)),
            other => other,
        };
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
        // The shadow holds on `num`'s ABSENCE alone. With the receiver
        // spelled `num` the receiver probe and the `num` probe are the
        // same lookup, so a shadow can never hold there — a library that
        // resolves `num` has the numeric face, and `num + double` must
        // refine to `double` as `int + double` does (#3063: a spurious
        // `name == num` disjunct short-circuited every `num` receiver
        // into the unrefined arm, folding `num + double` to `num` and
        // `num + int` to `num` by accident rather than by the int arm).
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
                            && resolve_container(
                                self.graph,
                                self.namespaces,
                                self.index,
                                self.lib,
                                self.file,
                                num,
                            )
                            .is_none();
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
        if right_ty != Ty::Never && (right_invalid || self.name_subtype(right_ty, named_q(double)))
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
            // No mark. Two shapes own one: the statically-`bool`
            // operators (`mark_operator` only tags `USER_OPERATORS`), and
            // an INVOKED left operand (`v() + 1`, `a.b() * 2`, `a[0] + 1`)
            // whose row keeps `Intent::Invoke`/`Intent::Operator` — the
            // mark's `Read` gate skips it, yet the oracle's operand check
            // carries no operator whitelist at all (error_verifier.dart
            // `visitBinaryOperatorInvocation` runs
            // `checkForUseOfVoidResult(node.leftOperand)` for every
            // binary). The left operand is then the last row wholly before
            // the operator: an interpolated-string right operand's inner
            // reads end after the operator and never qualify. Without the
            // fold the right operand stays on top and downstream type
            // reads (`var`/`final` inference, `return`, enclosing
            // conditions) see ITS type instead of the node's —
            // `flag = s == path` inferred the right operand's type,
            // `if (x == '$a')` fired `non_bool_condition` on the literal
            // row, and `v() + 1` never reached the void-operand verdict.
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
        // The void-operand verdict runs first (upstream
        // `visitBinaryExpression`, error_verifier.dart:467-482 →
        // error_detection_helpers.dart:245-258): the LEFT operand is
        // checked unconditionally; the RIGHT operand is checked only for
        // `&&`/`||` (every other binary's right operand is the resolver's
        // own assignability/operator question, never a void use). A `void`
        // operand's value is consumed by the operator, so the row reports
        // at the operand's own span — ahead of every member/argument face
        // the binary's own resolution asks, and before `bound_face`.
        self.emit_use_of_void_result(left_ty, &left);
        if op == self.intern("&&") || op == self.intern("||") {
            self.emit_use_of_void_result(right_ty, &right);
        }
        // Both operands read through their BOUND when they are type
        // parameters (#2349): the oracle's ladder and the argument check
        // run on the parameter's interface (`T extends num` is `num`), and
        // the argument row's message spells the bound the same way.
        let left_ty = self.bound_face(left_ty);
        let right_ty = self.bound_face(right_ty);
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
            // A potentially nullable receiver resolves through an applicable
            // extension face BEFORE the nullability verdict (the ordering
            // [`Self::nullable_extension_operator_sig`] documents), and an
            // extension-declared operator is never numerically refined:
            // `_refineNumericInvocationTypeNullSafe` answers the declared
            // type unchanged for an `ExtensionElement` encloser
            // (`extension on int? { int operator +(Null) }` over `int?`/`Null`
            // stays `int`, not the ladder's `double`; upstream Dart SDK
            // source, pkg/analyzer/lib/src/dart/element/type_system.dart:
            // 2266-2277).
            if let Some((formal, declared)) = self.nullable_extension_operator_sig(left_ty, op) {
                self.check_binary_argument(right_ty, formal, &right);
                declared
            } else {
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
            }
        };
        let _ = op_range;
        // The marked left row anchors at its own member token (`node
        // .selector.end`'s `.end` row; the receiver rows a merge absorbs
        // never reach the fold), while the expression's true head sits in
        // the source bytes — a dotted chain of identifiers joined by `.`
        // or `?.`. Walk it back so the folded row spans the whole binary
        // expression: the oracle's argument/assignment rows anchor there
        // (`node.selector.end - …` at the expression head, never at a
        // mid-chain member token).
        let is_ident_byte = |b: u8| b.is_ascii_alphanumeric() || b == b'_' || b == b'$';
        let mut expr_start = left.range.offset;
        loop {
            let Some(mut gap) = expr_start.checked_sub(1) else {
                break;
            };
            match self.byte_at(gap) {
                Some(b'.') => gap -= 1,
                Some(b'?') if gap >= 2 && self.byte_at(gap - 1) == Some(b'.') => gap -= 2,
                _ => break,
            }
            let mut head = gap;
            while head > 0 && self.byte_at(head - 1).is_some_and(is_ident_byte) {
                head -= 1;
            }
            if head == gap {
                break;
            }
            expr_start = head;
        }
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
                offset: expr_start,
                length: right.range.end().saturating_sub(expr_start),
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
        // A type-parameter operand checks as its BOUND (#2349): the
        // oracle's argument row spells the parameter but decides on its
        // interface — `U extends String` reports `String` against `num`,
        // `T extends num` reports nothing. A parameter with no captured
        // bound keeps the suppression (the implicit `Object?` accepts no
        // numeric formal, and the unbound family under-reports elsewhere).
        let right_ty = if let Ty::Named { name: head, .. } = right_ty
            && unbound(self, head)
        {
            match self.param_bound(head) {
                Some(bound) if !matches!(bound, Ty::Unknown | Ty::Dynamic) => bound,
                _ => return,
            }
        } else {
            right_ty
        };
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

    /// The matched value type of the pattern node whose check is running
    /// (`DartPatternImpl.resolvePattern`'s `context.matchedValueType`,
    /// ast.dart:4138/8564/24753/27989/35197 and
    /// list_pattern_resolver.dart:45): the open case/if-case scrutinee while
    /// every frame above the root is TRANSPARENT for the matched type — a
    /// logical operand (`analyzeLogicalOrPattern`/`analyzeLogicalAndPattern`
    /// dispatch both operands with the enclosing matched type) keeps it.
    ///
    /// `None` = the node's matched type is not reachable at parse time: a
    /// container field/element sub-pattern reads its property column or
    /// slot type, which the name level does not descend into. Callers stay
    /// silent there — the pre-#3235 posture for every nested node, now
    /// limited to exactly those nodes instead of all of them.
    pub(crate) fn pattern_matched_ty(&self) -> Option<Ty> {
        if !self.switch.case_sink_open || self.switch.pattern_stack.is_empty() {
            return None;
        }
        if !self.switch.pattern_stack.iter().skip(1).all(|f| f.binary.is_some()) {
            return None;
        }
        self.switch.switches.last().map(|ctx| ctx.scrutinee)
    }

    /// `checkPatternNeverMatchesValueType`'s List kind (upstream Dart SDK
    /// source, pkg/analyzer/lib/src/dart/resolver/list_pattern_resolver.dart:
    /// 45, fed by `analyzeListPattern`, pkg/_fe_analyzer_shared/lib/src/
    /// type_inference/type_analyzer.dart:979-1009): the required type is
    /// `List<elementType>` — the written element type, else the matched
    /// list's element type, else `Object?` — so a matched face that cannot be
    /// a `List` at all proves the pattern dead.
    ///
    /// Only the element-type-free spelling runs here: a written `<T>` is not
    /// carried into a resolved pattern type at parse time (the
    /// `end_type_arguments` latch is the object pattern's arity face, not a
    /// type), so the row's required-type rendering could not say `List<T>` —
    /// the under-report direction.
    ///
    /// `Some((at, required))` is the deferred candidate (the node is a
    /// container field's sub-pattern, so its matched type is that column):
    /// the required face rides along — it is written-type-free and needs no
    /// frame.
    pub(crate) fn check_list_pattern(&mut self, at: SourceRange) -> Option<(SourceRange, Ty)> {
        let Some(matched) = self.pattern_matched_ty() else {
            if !self.pattern_reads_container_column() {
                return None;
            }
            return Some((at, self.list_pattern_required_ty()));
        };
        if matched == Ty::Unknown || self.pending_pattern_type_args.is_some() {
            return None;
        }
        if self.name_can_be_subtype(matched, Ty::named(self.intern("List"))) {
            return None;
        }
        let required = self.list_pattern_required_ty();
        let a0 = self.ty_display(matched);
        let a1 = self.ty_display(required);
        self.stage_typed(SemanticCode::PatternNeverMatchesValueType, at, &a0, &a1);
        None
    }

    /// The List kind's required type: `List<Object?>` (`elementType` absent —
    /// see [`Self::check_list_pattern`]).
    fn list_pattern_required_ty(&mut self) -> Ty {
        let element = Self::nullable_form(Ty::named(self.intern("Object")));
        let head = self.intern("List");
        self.boundary.args.borrow_mut().named(head, false, &[element])
    }

    /// `checkPatternNeverMatchesValueType`'s Record kind: the required type
    /// is the record over the written fields with every slot `Object?`
    /// (upstream `analyzeRecordPattern`'s `operations.recordType(
    /// positional: List.filled(count, objectQuestionType), named: ...)`,
    /// pkg/_fe_analyzer_shared/lib/src/type_inference/type_analyzer.dart:
    /// 1841-1855), and the verdict runs at the record node itself with the
    /// node's own matched type (`resolvePattern`'s
    /// `checkPatternNeverMatchesValueType`, pkg/analyzer/lib/src/generated/
    /// resolver.dart:1632-1637, `canBeSubtypeOf`'s record arms at
    /// pkg/analyzer/lib/src/dart/element/type_system.dart:269-330).
    ///
    /// `positional` is the written positional field count, `named` the
    /// written named set (duplicates excluded, the official's
    /// `_reportDuplicateRecordPatternFields` gate).
    pub(crate) fn check_record_pattern(
        &mut self,
        at: SourceRange,
        positional: usize,
        named: &[StringId],
    ) {
        let Some(matched) = self.pattern_matched_ty() else {
            return;
        };
        if matched == Ty::Unknown || !self.record_pattern_disjoint(matched, positional, named) {
            return;
        }
        let object = Self::nullable_form(Ty::named(self.intern("Object")));
        let slots = vec![object; positional];
        let names: Vec<(StringId, Ty)> = named.iter().map(|name| (*name, object)).collect();
        let required = self.boundary.args.borrow_mut().record(&slots, &names, false);
        let a0 = self.ty_display(matched);
        let a1 = self.ty_display(required);
        self.stage_typed(SemanticCode::PatternNeverMatchesValueType, at, &a0, &a1);
    }

    /// Whether a record-shaped required type provably admits no value of
    /// `matched` (`canBeSubtypeOf`'s record arms, type_system.dart:269-330):
    /// against a record right an interface left answers `true` only for
    /// `Object`/`Record`, a function left never does, and two record sides
    /// compare their field shapes. An opaque or undecidable left keeps the
    /// official's conservative `true` (a type parameter, a non-interface
    /// shape).
    fn record_pattern_disjoint(
        &self,
        matched: Ty,
        positional: usize,
        named: &[StringId],
    ) -> bool {
        match matched {
            // The `Null` arm decides before the record arms
            // (type_system.dart:150-154): the required record type is never
            // nullable, so `null` can inhabit no value of it.
            Ty::Null => true,
            Ty::Function { .. } => true,
            Ty::Named { name, .. } => match self.strings.resolve(name) {
                "Object" | "Record" => false,
                // `Function`'s interface face: `canBeSubtypeOf`'s
                // interface-vs-record arm admits only `Object`/`Record`.
                "Function" => true,
                // A declared interface head is disjoint from every record;
                // an opaque one (a scope parameter, an unresolvable name) is
                // the official's non-interface fallthrough.
                _ => self.name_world().is_known_head(name),
            },
            Ty::Record { shape, .. } => {
                let pool = self.boundary.pool();
                let shape = pool.record_shape(shape);
                if shape.positional.len() != positional || shape.named.len() != named.len() {
                    return true;
                }
                let mut matched_names: Vec<StringId> =
                    shape.named.iter().map(|(name, _)| *name).collect();
                matched_names.sort_unstable();
                let mut required: Vec<StringId> = named.to_vec();
                required.sort_unstable();
                matched_names != required
            }
            Ty::Unknown | Ty::Dynamic | Ty::Never | Ty::Void => false,
        }
    }

    /// The cast-pattern triple (shared_type_analyzer.dart:145-152 and
    /// `checkPatternNeverMatchesValueType`'s Cast kind,
    /// type_analyzer.dart:723-756, plus `visitCastPattern`'s null face,
    /// best_practices_verifier.dart:227-239): a matched `Null` against a
    /// non-nullable target always fails (`cast_from_null_always_fails`), a
    /// matched type already below the target makes the cast dead
    /// (`unnecessary_cast_pattern`), and a matched type no value of which
    /// can inhabit the target proves the pattern dead
    /// (`pattern_never_matches_value_type`) — the last two exclusive
    /// (`canBeSubtypeOf ⊇ isSubtypeOf`).
    ///
    /// The matched type is the node's own
    /// ([`Self::pattern_matched_ty`]): a root or logical-operand node reads
    /// the open scrutinee. A field sub-pattern's matched type is its
    /// property column, unreadable at parse time, so the call stays silent
    /// there (the under-report).
    pub(crate) fn check_cast_pattern(&mut self, at: SourceRange, cast: Option<Ty>) {
        let Some(cast) = cast else { return };
        let Some(matched) = self.pattern_matched_ty() else {
            return;
        };
        if cast == Ty::Unknown || matched == Ty::Unknown {
            return;
        }
        // Every `Null` spelling resolves to the one `NullType` upstream
        // (`isDartCoreNull`'s identity), so the verdict reads canonical
        // faces on both sides — a written `Null` capture must not masquerade
        // as a non-nullable interface head.
        let cast = self.normalized_null(cast);
        let matched = self.normalized_null(matched);
        let null_face = matched == Ty::Null && matches!(cast, Ty::Named { nullable: false, .. });
        if self.name_subtype(matched, cast) {
            // The official gate is `isSubtypeOf && castType is not
            // InvalidType` — a `dynamic` target reports like any other.
            self.stage_usage(SemanticCode::UnnecessaryCastPattern, at, "");
        } else if self.pattern_never_matches(matched, cast) {
            let a0 = self.ty_display(matched);
            let a1 = self.ty_display(cast);
            self.stage_typed(SemanticCode::PatternNeverMatchesValueType, at, &a0, &a1);
        }
        // `visitCastPattern`'s face (best_practices_verifier.dart:227-239),
        // staged after the pair as before: a matched `Null` can inhabit no
        // non-nullable target, so the row rides beside the never-match one.
        if null_face {
            self.stage_usage(SemanticCode::CastFromNullAlwaysFails, at, "");
        }
    }

    /// `checkPatternNeverMatchesValueType`'s DeclaredVariable/Wildcard/
    /// Object kinds (type_analyzer.dart:723-756): a required type no
    /// matched value can inhabit proves the pattern dead. Refutable
    /// contexts only (the official's `irrefutableContext == null` gate;
    /// declaration patterns suppress).
    /// The verdict reads the type folded through a non-parameterized
    /// alias first — the official `requiredType` carries the written
    /// alias into every kind and unwraps it for `canBeSubtypeOf` — while
    /// the message renders the written spelling (`'IntAlias'`, probed
    /// 3.13.1).
    ///
    /// `Some((at, required))` is the deferred candidate: the node's matched
    /// type is its enclosing container's column
    /// ([`Self::pattern_reads_container_column`]), which no parse-time face
    /// carries — the caller parks it on the container frame and re-judges it
    /// once that container closes ([`Self::judge_pattern_never_matches`]).
    pub(crate) fn check_pattern_never_matches(
        &mut self,
        at: SourceRange,
        required: Ty,
    ) -> Option<(SourceRange, Ty)> {
        if required == Ty::Unknown || required == Ty::Dynamic {
            return None;
        }
        let Some(matched) = self.pattern_matched_ty() else {
            return self.pattern_reads_container_column().then_some((at, required));
        };
        self.judge_pattern_never_matches(at, required, matched);
        None
    }

    /// The `TypeSystem.isPotentiallyNullable` face the `canBeSubtypeOf`
    /// Null arms read (upstream Dart SDK source,
    /// pkg/analyzer/lib/src/dart/element/type_system.dart:147-159 over
    /// :1161-1179): the top faces and `Null` itself, every `?`-suffixed
    /// spelling, a `FutureOr` whose element is, and a `?`-less type
    /// parameter whose bound is. [`Self::potentially_nullable`] is the
    /// receiver gate's posture — it answers `dynamic`/`void`/`Null` false
    /// because its consumers swap those rows — so the plain upstream
    /// predicate recomposes over it and the `FutureOr` element walk.
    fn potentially_nullable_face(&self, ty: Ty) -> bool {
        match ty {
            Ty::Null | Ty::Dynamic | Ty::Void => true,
            Ty::Unknown | Ty::Never => false,
            _ => self.potentially_nullable(ty) || self.future_or_potentially_nullable(ty),
        }
    }

    /// `checkPatternNeverMatchesValueType`'s verdict — `!
    /// typeSystem.canBeSubtypeOf(matchedValueType, requiredType)` (upstream
    /// Dart SDK source, pkg/analyzer/lib/src/generated/resolver.dart:
    /// 624-653): the Null arms first (type_system.dart:147-159 — a `Null`
    /// face fits exactly the potentially-nullable opposite face and is
    /// disjoint from every non-nullable one), then the name-level
    /// mutual-exclusion companion. The arms decide over the [`Ty`] shape
    /// because the name-level world cannot: the function/record sentinels,
    /// the `?`-less parameter bound, and the `FutureOr` element are
    /// [`Self::potentially_nullable_face`] questions.
    fn pattern_never_matches(&self, matched: Ty, required: Ty) -> bool {
        if matched == Ty::Null {
            return !self.potentially_nullable_face(required);
        }
        if required == Ty::Null {
            return !self.potentially_nullable_face(matched);
        }
        !self.name_can_be_subtype(matched, required)
    }

    /// The [`Self::check_pattern_never_matches`] verdict over an EXPLICIT
    /// matched face: the open scrutinee for a root/logical-operand node, the
    /// property column for a deferred object-pattern field
    /// (`analyzeObjectPattern` pushes each field's property type as that
    /// sub-pattern's `matchedValueType`, upstream Dart SDK source,
    /// pkg/_fe_analyzer_shared/lib/src/type_inference/type_analyzer.dart:
    /// 1674-1697).
    pub(crate) fn judge_pattern_never_matches(
        &mut self,
        at: SourceRange,
        required: Ty,
        matched: Ty,
    ) {
        if required == Ty::Unknown || required == Ty::Dynamic || matched == Ty::Unknown {
            return;
        }
        // Every `Null` spelling resolves to the one `NullType` upstream
        // (`isDartCoreNull`'s identity): the written annotation capture
        // spells `Named{Null}` where the inference face carries
        // [`Ty::Null`], and the verdict must read canonical faces — the
        // message keeps the written spellings.
        let matched = self.normalized_null(matched);
        let folded = self.normalized_null(self.fold_alias_ty(required));
        if self.pattern_never_matches(matched, folded) {
            let a0 = self.ty_display(matched);
            let a1 = self.ty_display(required);
            self.stage_typed(SemanticCode::PatternNeverMatchesValueType, at, &a0, &a1);
        }
    }

    /// Whether the running pattern node's matched type is its enclosing
    /// container's column — a property type for an object-pattern field, a
    /// slot/column for a record, list, or map field (the official pushes
    /// each field's own face as the sub-pattern's `matchedValueType`;
    /// [`Self::pattern_matched_ty`] answers `None` for exactly these nodes).
    /// Only the object kind can turn the column back into a type
    /// ([`Self::judge_pattern_never_matches`] over
    /// [`Self::object_pattern_binder_ty`]); the other kinds drop their
    /// deferrals, the established under-report.
    ///
    /// A node outside a refutable case window owes nothing (the official's
    /// `irrefutableContext == null` gate), so it is never a deferral.
    pub(crate) fn pattern_reads_container_column(&self) -> bool {
        self.switch.case_sink_open
            && self.switch.pattern_stack.len() >= 2
            && !self
                .switch
                .pattern_stack
                .iter()
                .skip(1)
                .all(|frame| frame.binary.is_some())
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
    /// cases) report, and only nodes whose matched type is reachable at
    /// parse time ([`Self::pattern_matched_ty`]) — the root and the logical
    /// operands, as in [`Self::check_pattern_never_matches`].
    pub(crate) fn check_null_pattern(&mut self, at: SourceRange, is_assert: bool) {
        let Some(matched) = self.pattern_matched_ty() else {
            return;
        };
        if !self.strictly_non_nullable(matched) {
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
    /// stay silent — their matched type is the property column, which the
    /// name level does not descend into (the under-report, exactly the
    /// nodes [`Self::pattern_matched_ty`] answers `None` for). A passing
    /// candidate defers to the teardown join when a clause anchor stands
    /// (the flow's promoted matched type can narrow the verdict, #1351).
    pub(crate) fn check_constant_pattern(&mut self, constant: Option<Ty>, at: Option<SourceRange>) {
        let (Some(constant), Some(at)) = (constant, at) else {
            return;
        };
        let Some(matched) = self.pattern_matched_ty() else {
            return;
        };
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
        self.classify_condition_marked(&row, mark, CondFace::Condition);
    }

    /// One `unchecked_use_of_nullable_value` candidate per operand site:
    /// a `bool?`-typed operand stages (the teardown join re-decides the
    /// staged `from` against the flow's condition-type row at the same
    /// offset); the dedup keeps an enclosing junction's re-read of the
    /// same last row (the `&&` arm classifies both operands, then the
    /// enclosing `if` re-reads the right one) from doubling.
    pub(crate) fn classify_condition(&mut self, row: &Row) {
        self.classify_condition_marked(row, None, CondFace::Condition);
    }

    /// The condition operand's two faces in one decision: the nullable-bool
    /// staging above, and the non-`bool` face `checkForNonBoolExpression`
    /// reports through its `locatableDiagnostic` parameter
    /// (bool_expression_verifier.dart:51-77) — `non_bool_condition` for a
    /// statement/element condition, `non_bool_operand` for a `&&`/`||`
    /// operand (binary_expression_resolver.dart:81-92, :294-295, :340-341),
    /// `non_bool_expression` for an `assert` condition (resolver.dart
    /// :2136/:2163). A known non-`bool` static type reports at the
    /// condition's extent, unless a statically-`bool` composite (`==`,
    /// `&&`, `is`, `as`, a negation operand) already decided the span, or
    /// the site is a negation operand (the dedicated
    /// `non_bool_negation_expression` face replaces the condition face
    /// there, and an invalid `this`/`super` receiver's
    /// `InvalidType` also suppresses it upstream). The `CondFace`
    /// distinguishes the report, never the verdict — one decision point,
    /// three codes.
    pub(crate) fn classify_condition_marked(
        &mut self,
        row: &Row,
        mark: Option<usize>,
        face: CondFace,
    ) {
        if self.condition_staged.contains(&row.range.offset) {
            return;
        }
        // A `&&`/`||` operand carries its own face wherever the caller's
        // window claimed the plain condition face (upstream checks every
        // operand in every context — a junction inside a statement
        // condition or an `assert` reports `non_bool_operand` at the
        // operand, not the enclosing face).
        let (code, arg, operand_at) = match self.junction_operand(row.range) {
            Some((op, at)) => (SemanticCode::NonBoolOperand, op, Some(at)),
            None => (face.code(), "", None),
        };
        let ty = self.condition_operand_ty(row);
        // The condition operand's void face precedes every bool verdict
        // (upstream `bool_expression_verifier.dart:57`'s
        // `_checkForUseOfVoidResult(expression)` early-return —
        // `x ? a : b`, `if (x)`, `while (x)`, `assert(x)`, `x &&`/`x ||`
        // consume a `void` operand's value, so the oracle reports
        // `use_of_void_result` at the operand and `known_non_bool`'s
        // `void` arm stays silent below, error_detection_helpers.dart:
        // 245-258).
        self.emit_use_of_void_result(ty, row);
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
            .unwrap_or(operand_at.unwrap_or(row.range));
            // A promotable read's parse-time non-`bool` type can still be
            // promoted to `bool` by teardown (`dartValue is bool &&
            // dartValue` — the promoted operand is the oracle's own
            // silent face): stage for the flow join instead of emitting
            // eagerly. The `literal` flag marks the non-`bool` candidate
            // inside the nullable-condition kind (the staged kinds cannot
            // grow here).
            //
            // The resolver's non-`bool` faces fire inside constant contexts
            // too — the oracle's `checkForNonBool*` callers never gate on
            // `_isInConstantContext` (a `?:`/if-element/`&&`/`||`/`assert`
            // inside a const expression reports `non_bool_*` from
            // evaluation.dart:1520/1640/1762's `toBoolValue` misses AND from
            // the resolver's own static-type checks) — the const evaluator's
            // `const_eval_type_bool` family is a separate row, not a
            // replacement.
            if self.local_hits.contains_key(&row.range.offset) {
                self.staged_checks.push(StagedCheck {
                    kind: StagedKind::NullableCondition,
                    range: row.range,
                    from: ty,
                    to: Ty::UNKNOWN,
                    literal: true,
                });
            } else {
                self.emit(code, anchor, arg, "");
            }
            self.stage_condition_offset(row.range.offset);
        }
    }

    /// The `&&`/`||` lexeme directly adjoining an operand span (upstream
    /// `_checkNonBoolOperand`, binary_expression_resolver.dart:81-92): the
    /// operator is the token immediately before the right operand or
    /// immediately after the left operand, each fully-parenthesized
    /// operand's own `(`/`)` pair included in the anchor (the oracle
    /// reports `.at(operand)` — the ParenthesizedExpression, parens
    /// included). Answers the lexeme and the operand's expanded span.
    fn junction_operand(&self, range: SourceRange) -> Option<(&'static str, SourceRange)> {
        let tokens = self.tokens.as_slice();
        let op_of = |token: &dartq_scanner::Token| match token.kind {
            dartq_scanner::TokenKind::AmpersandAmpersand => Some("&&"),
            dartq_scanner::TokenKind::BarBar => Some("||"),
            _ => None,
        };
        let mut at = range;
        loop {
            // The fold anchors a parenthesized operand at its closing `)`.
            // The oracle's `.at(operand)` is the ParenthesizedExpression —
            // grow the span to its matching `(` (and repeat, so `((x))`
            // covers the outermost pair).
            while let Some(index) = tokens
                .iter()
                .position(|t| t.utf16_offset == at.offset && t.byte_len == at.length)
                .filter(|index| {
                    matches!(
                        tokens[*index].kind,
                        dartq_scanner::TokenKind::CloseParen
                    )
                })
            {
                let mut depth = 1u32;
                let mut open = None;
                for back in (0..index).rev() {
                    match tokens[back].kind {
                        dartq_scanner::TokenKind::CloseParen => depth += 1,
                        dartq_scanner::TokenKind::OpenParen => {
                            depth -= 1;
                            if depth == 0 {
                                open = Some(back);
                                break;
                            }
                        }
                        _ => {}
                    }
                }
                match open {
                    Some(open) => {
                        at = SourceRange {
                            offset: tokens[open].utf16_offset,
                            length: at.end() - tokens[open].utf16_offset,
                        };
                    }
                    None => break,
                }
            }
            let next = tokens.get(tokens.partition_point(|t| t.utf16_offset < at.end()));
            if let Some(op) = next.and_then(op_of) {
                return Some((op, at));
            }
            let prev = tokens
                .partition_point(|t| t.utf16_offset < at.offset)
                .checked_sub(1)
                .and_then(|index| tokens.get(index));
            if let Some(op) = prev.and_then(&op_of) {
                return Some((op, at));
            }
            // A directly-wrapping `(`/`)` pair belongs to the operand — the
            // anchor grows to it and the adjoined-token check retries.
            match (prev, next) {
                (Some(p), Some(n))
                    if matches!(p.kind, dartq_scanner::TokenKind::OpenParen)
                        && matches!(n.kind, dartq_scanner::TokenKind::CloseParen) =>
                {
                    at = SourceRange {
                        offset: p.utf16_offset,
                        length: n.utf16_offset + n.byte_len - p.utf16_offset,
                    };
                }
                _ => return None,
            }
        }
    }

    /// The for-in element's `void` use (upstream `_checkForEachParts`'s
    /// `sequenceElementType is VoidType && variableType is! VoidType` arm —
    /// error_verifier.dart:4478-4484): the iterated value is implicitly
    /// assigned to the loop variable, so a `void` element's value can only
    /// be discarded into a `void` loop variable. The report anchors at the
    /// ITERABLE expression (`.at(node.iterable)`), the same face the
    /// iterable-type check uses.
    ///
    /// `loop_var_ty` is the loop variable's DECLARED face: a `var`/`final`
    /// head answers `Unknown` here because its type IS the element (a
    /// `void` head — upstream's `variableType is VoidType`, silent), and a
    /// typed head must spell a known non-`void` type. The element itself is
    /// read off the iterable's written instantiation: the name-level model
    /// carries no substitution through a user subtype, so a `void` element
    /// is only ever spelled by a core sequence head, and any unspellable
    /// shape keeps the under-report direction.
    pub(crate) fn for_in_void_element(
        &self,
        mark: usize,
        await_: bool,
        loop_var_ty: Option<Ty>,
    ) -> Option<SourceRange> {
        match loop_var_ty {
            Some(ty) if ty != Ty::UNKNOWN && ty != Ty::Void => {}
            _ => return None,
        }
        let (ty, _, range) = self.single_row_since(mark)?;
        let Ty::Named {
            name,
            nullable: false,
            args,
        } = ty
        else {
            return None;
        };
        if args == ArgListId::EMPTY {
            return None;
        }
        // The sequence heads the element staging accepts, restricted to the
        // spellings that can carry a written `void` argument
        // (`List`/`Set`/`Iterable`, or `Stream` under `await`).
        let sequence = if await_ {
            name == self.intern("Stream")
        } else {
            name == self.collection_heads[0]
                || name == self.collection_heads[1]
                || name == self.collection_heads[3]
        };
        if !sequence {
            return None;
        }
        let element = self.boundary.args.borrow().get(args).first().copied();
        (element.is_some_and(|element| self.is_void_spelling(element))).then_some(range)
    }

    /// The non-`bool` face the staged non-`bool` re-decision emits: a
    /// `&&`/`||` operand reports `non_bool_operand` with its lexeme, an
    /// `assert` operand reports `non_bool_expression`, everything else the
    /// plain `non_bool_condition` — the same span-keyed derivation
    /// [`Self::junction_operand`]/[`Self::assert_condition_window`] runs at
    /// stage time (the staged check stores no face of its own; `to` and
    /// `literal` are already spoken for).
    fn condition_face_at(&self, range: SourceRange) -> (SemanticCode, &'static str) {
        if let Some((op, _)) = self.junction_operand(range) {
            return (SemanticCode::NonBoolOperand, op);
        }
        let tokens = self.tokens.as_slice();
        let at = tokens.partition_point(|t| t.utf16_offset < range.offset);
        if matches!(
            (tokens.get(at.wrapping_sub(2)), tokens.get(at.wrapping_sub(1))),
            (Some(head), Some(open))
                if matches!(open.kind, dartq_scanner::TokenKind::OpenParen)
                    && head.kind
                        == dartq_scanner::TokenKind::Keyword(dartq_scanner::Keyword::Assert)
        ) {
            return (SemanticCode::NonBoolExpression, "");
        }
        (SemanticCode::NonBoolCondition, "")
    }

    /// Whether the condition window opening at `start` is an `assert`'s
    /// condition — the token directly before `start` is the `(` of
    /// `assert (…` (the bounded windows open at the paren's end: the
    /// assert statement's `assert(condition)`, the constructor
    /// initializer's `: assert(condition)`). Statement/initializer asserts
    /// share the `non_bool_expression` face; every other bounded window
    /// (the for-segment, the do-while tail) keeps `non_bool_condition`.
    fn assert_condition_window(&self, start: u32) -> bool {
        let tokens = self.tokens.as_slice();
        let at = tokens.partition_point(|t| t.utf16_offset < start);
        matches!(
            (tokens.get(at.wrapping_sub(2)), tokens.get(at.wrapping_sub(1))),
            (Some(head), Some(open))
                if matches!(open.kind, dartq_scanner::TokenKind::OpenParen)
                    && head.kind
                        == dartq_scanner::TokenKind::Keyword(dartq_scanner::Keyword::Assert)
        )
    }

    /// The non-`bool` face a bounded condition window reports:
    /// `non_bool_expression` for an `assert` condition
    /// (resolver.dart:2129-2178, `locatableDiagnostic: diag.nonBoolExpression`),
    /// `non_bool_condition` elsewhere.
    fn window_condition_face(&self, start: u32) -> CondFace {
        if self.assert_condition_window(start) {
            CondFace::Expression
        } else {
            CondFace::Condition
        }
    }

    /// The condition/negation verdict's operand type: the operand's
    /// static type as the official `checkForNonBool*` callers read it —
    /// `expression.staticType`, which carries the `?.` short-circuit's
    /// nullable join (`a?.foo` over `a: A?`, `foo: bool` reads `bool?`,
    /// and `a?.foo.bar` reads `bar?` — the whole shorted chain's own
    /// type). `null_aware_members` keys exactly that face (the `?.`-merged
    /// selector plus every continuation selector of the shorted chain,
    /// invocation.rs:1433-1464's `receiver_offset` join). This is the
    /// `widen_null_aware` reading [`Self::coalesce_operand_never_null`]
    /// gives the `??` operand — the static-type face, never the target's
    /// read-resolution face (`??=`'s `a?.m ??= 1` reads `m` unwrapped).
    fn condition_operand_ty(&self, row: &Row) -> Ty {
        let ty = self.operand_static_ty(row);
        if ty != Ty::UNKNOWN && self.null_aware_members.contains(&row.range.offset) {
            Self::nullable_form(ty)
        } else {
            ty
        }
    }

    /// The `!` operand's classification: the nullable-bool staging runs,
    /// the condition face does not (the negation face owns the report).
    pub(crate) fn classify_negation_operand(&mut self, row: &Row) {
        let ty = self.condition_operand_ty(row);
        // The `!`-prefix operand's void face runs ahead of the bool
        // verdict (upstream `visitPrefixExpression`'s
        // `checkForUseOfVoidResult(operand)`, error_verifier.dart:1727 →
        // error_detection_helpers.dart:245-258): a `void`-typed operand's
        // value is consumed by the negation — `!x` on `void x` reports
        // `use_of_void_result`, never `non_bool_*`.
        self.emit_use_of_void_result(ty, row);
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
    /// condition face never fires — except a constructor initializer's
    /// `assert`, which is a real `AssertInitializer` upstream
    /// (`non_bool_expression`, resolver.dart:2129-2151); only the
    /// expression-position recovery shape keeps the staging-only posture
    /// (a bare `assert(e)` inside an expression owns no oracle condition
    /// node).
    pub(crate) fn stage_condition_operand_within(&mut self, start: u32, end: u32) {
        if self.initializer_depth > 0 && self.assert_condition_window(start) {
            self.check_condition_operand_within(start, end);
            return;
        }
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
    /// `strict-casts` rejects `dynamic` at the same verdict — upstream
    /// `isAssignableTo(dynamic, bool, strictCasts: true)` answers false, so
    /// `checkForNonBoolExpression` reports (`NonBoolExpressionWithStrictCasts
    /// Test/test_assert`); the default posture keeps `dynamic` assignable.
    /// A `Type`-headed face (a `TypeLiteral`'s or a `Type`-declared read's
    /// static type) is always known non-`bool` — the names-only dart:core
    /// stub may not declare the class, and no alias folds it.
    pub(crate) fn known_non_bool(&self, ty: Ty) -> bool {
        match ty {
            Ty::Dynamic => self.strict.casts,
            Ty::Named { .. } => match self.fold_alias_ty(ty) {
                Ty::Named { name, .. } => {
                    if name == self.intern("Type") {
                        return true;
                    }
                    // An out-of-scope head is a foreign projection this unit
                    // cannot resolve (an unexported function-type alias rides
                    // inference without an import — drift's `Predicate<T>`):
                    // class-vs-alias is undecidable at the name level while
                    // the oracle resolves it, so the condition faces keep
                    // silent. In-scope-ness is the NAMESPACE question
                    // (`lookup_scoped`), never `top_level_decl` — a name
                    // declared through the graph without an `ElementIndex`
                    // stub (the unindexed `dart:core` face posture,
                    // `List`/`Map`/`Set`) is in scope, not a foreign
                    // projection. The dart:core primitives have no declaring
                    // stub, yet their non-`bool` verdicts are known.
                    let in_scope = !matches!(
                        lookup_scoped(
                            self.graph,
                            self.namespaces,
                            self.index,
                            self.lib,
                            self.file,
                            name,
                        ),
                        ScopedHit::NotFound
                    );
                    if !in_scope && !self.members.primitive_ids.contains(&name) {
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
    /// either). The window's own face comes from
    /// [`Self::window_condition_face`]: an `assert` head reports
    /// `non_bool_expression`, the for/do-while statement windows report
    /// `non_bool_condition` — a `&&`/`||` operand inside either overrides
    /// both to `non_bool_operand`.
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
        let face = self.window_condition_face(start);
        self.classify_condition_marked(&row, None, face);
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
    /// Whether `row` is an unfolded index's KEY (`f()[k]`, `m[k]?[n]` —
    /// a receiver that keeps its operand rows): the next source byte
    /// after the row closes the brackets, so the enclosing operand is
    /// the `[]` read, never the key's own face.
    /// Whether an index fold's receiver (`node.ps` in `node.ps[i]`) may
    /// read a flow-promoted face the pass-1 index result cannot see: its
    /// root identifier is type-tested (`node is FI` / `node as FI`)
    /// somewhere in the unit. The pass-1 result reads the unpromoted
    /// container's `[]` (`List<P>` where the promoted `FI.ps` answers
    /// `List<PI>`), so binding it as an initializer's inferred type would
    /// type later reads on the wrong face; the gate keeps the unbound
    /// (under-report) posture for exactly those receivers.
    pub(crate) fn index_receiver_latched(&self, row: &Row) -> bool {
        let bytes = self.source.as_bytes();
        let chain_start = self
            .property_access_span(row.range)
            .map_or(row.range.offset, |span| span.offset);
        let start = self.byte_of(chain_start).min(bytes.len());
        let len = bytes[start..]
            .iter()
            .take_while(|b| b.is_ascii_alphanumeric() || **b == b'_' || **b == b'$')
            .count();
        let root = &self.source[start..start + len];
        let latched = !root.is_empty()
            && root != "this"
            && [" is ", " as "].iter().any(|op| {
                let needle = format!("{root}{op}");
                self.source.match_indices(&needle).any(|(at, _)| {
                    at == 0
                        || !bytes[at - 1].is_ascii_alphanumeric()
                            && bytes[at - 1] != b'_'
                            && bytes[at - 1] != b'$'
                            && bytes[at - 1] != b'.'
                })
            });

        latched
    }

    pub(crate) fn is_index_key_row(&self, row: &Row) -> bool {
        let end = self.byte_of(row.range.end()).min(self.source.len());
        self.source.as_bytes()[end..]
            .iter()
            .find(|b| !b.is_ascii_whitespace())
            == Some(&b']')
    }

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

    /// The `ConstructorFieldInitializer` node span whose written field name
    /// (or bare `this` head) is `name_range`: the name through the assigned
    /// expression's last token. The upstream report anchors `.at(initializer)`
    /// — the whole node (error_verifier.dart:5572-5586), never the bare
    /// selector. The walk rides the token table: the `=` after the name, then
    /// the value runs to the first depth-0 `,` / `;` / list-closing `)` /
    /// body-opening `{` (comments and whitespace are outside the token table;
    /// nested brackets stay balanced), and a `this.` head widens the start to
    /// the keyword (the node's own offset). A shape without the `=` degrades
    /// to the name's own range.
    pub(crate) fn initializer_node_span(&self, name_range: SourceRange) -> SourceRange {
        let toks = self.tokens.as_slice();
        let mut at = toks.partition_point(|t| t.utf16_offset < name_range.end());
        if !toks
            .get(at)
            .is_some_and(|t| t.kind == dartq_scanner::TokenKind::Eq)
        {
            return name_range;
        }
        // `this.x = v` node offset: the keyword two tokens before the name
        // (`this` `.` `x`); a bare-name initializer keeps the name's offset.
        let mut offset = name_range.offset;
        if at >= 2
            && toks[at - 1].kind == dartq_scanner::TokenKind::Period
            && toks[at - 2].kind == dartq_scanner::TokenKind::Keyword(dartq_scanner::Keyword::This)
            && toks[at - 2].utf16_offset < offset
        {
            offset = toks[at - 2].utf16_offset;
        }
        let mut depth: u32 = 0;
        let mut end = name_range.end();
        at += 1;
        while let Some(t) = toks.get(at) {
            match t.kind {
                dartq_scanner::TokenKind::OpenParen
                | dartq_scanner::TokenKind::OpenSquare
                | dartq_scanner::TokenKind::OpenCurly => depth += 1,
                dartq_scanner::TokenKind::CloseParen
                | dartq_scanner::TokenKind::CloseSquare
                | dartq_scanner::TokenKind::CloseCurly
                    if depth > 0 =>
                {
                    depth -= 1;
                }
                dartq_scanner::TokenKind::CloseParen | dartq_scanner::TokenKind::CloseCurly
                    if depth == 0 =>
                {
                    break;
                }
                dartq_scanner::TokenKind::Comma | dartq_scanner::TokenKind::Semicolon
                    if depth == 0 =>
                {
                    break;
                }
                _ => {}
            }
            end = self.utf16_of((t.byte_offset + t.byte_len) as usize);
            at += 1;
        }
        SourceRange {
            offset,
            length: end.saturating_sub(offset),
        }
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
        if !(head.is_ascii_alphabetic() || head == b'_' || head == b'$') {
            return None;
        }
        if !chars.all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'$') {
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
        while start > 0
            && (bytes[start - 1].is_ascii_alphanumeric()
                || bytes[start - 1] == b'_'
                || bytes[start - 1] == b'$')
        {
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
    /// The target span a cascade opening at `operator` re-reads: the
    /// previous cascade's target when this one continues it (the target
    /// row sits after that cascade's operator, and the source between the
    /// two operators stays inside one expression — no `;`, no block brace,
    /// no bracket closing past its own opener), else the popped `target`.
    pub(crate) fn cascade_continuation_target(
        &self,
        operator: u32,
        target: SourceRange,
    ) -> SourceRange {
        let Some((prev_op, prev_target)) = self.cascade_last else {
            return target;
        };
        if !(prev_op < target.offset && target.offset < operator) {
            return target;
        }
        let bytes = self.source.as_bytes();
        let (start, end) = (self.byte_of(prev_op), self.byte_of(operator));
        let mut depth = 0i32;
        for &b in &bytes[start..end] {
            match b {
                b';' | b'{' | b'}' => return target,
                b'(' | b'[' => depth += 1,
                b')' | b']' => {
                    depth -= 1;
                    if depth < 0 {
                        return target;
                    }
                }
                _ => {}
            }
        }
        if depth != 0 {
            return target;
        }

        prev_target
    }

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

/// The opener byte of the bracket group that closes at `cursor`: a
/// depth walk backward over `()[]{}` pairs. The plain walk carries no
/// string/comment awareness (a bracket spelled inside a leaf string or
/// comment desyncs the depth) — every consumer's corruption direction
/// is the under-report one (a wrong opener's byte mismatches the leaf
/// rows and the scan answers `dynamic`), never a more specific type.
fn matching_bracket_back(bytes: &[u8], cursor: usize) -> Option<usize> {
    let mut depth = 0usize;
    let mut i = cursor;
    loop {
        // Bounded by the file extent: past the first byte there is no
        // opener, and an unmatched walk is no window (`None`).
        i = i.checked_sub(1)?;
        match bytes[i] {
            b')' | b']' | b'}' => depth += 1,
            b'(' | b'[' | b'{' => {
                // The first opener at depth 0 is the match — an empty
                // body (`[]`, `{}`, `() {}`) walks straight into it.
                if depth == 0 {
                    return Some(i);
                }
                depth -= 1;
            }
            _ => {}
        }
    }
}

/// The closer byte of the bracket group that opens at `cursor` — the
/// empty square literal's window, whose row/key rides the `[` itself
/// (the parser hands the empty list's right as its own `[`). Bounded by
/// the file extent: an unmatched walk is no window (`None`).
fn matching_bracket_fwd(bytes: &[u8], cursor: usize) -> Option<usize> {
    let (open_b, close_b) = match bytes.get(cursor)? {
        b'[' => (b'[', b']'),
        b'{' => (b'{', b'}'),
        b'(' => (b'(', b')'),
        _ => return None,
    };
    let mut depth = 1usize;
    for (i, &b) in bytes[cursor + 1..].iter().enumerate() {
        if b == open_b {
            depth += 1;
        } else if b == close_b {
            depth -= 1;
            if depth == 0 {
                return Some(cursor + 1 + i);
            }
        }
    }
    None
}

/// Top-level comma segments of `[start, end)`: a depth walk over
/// `()[]{}` with string contents skipped (the same shape the literal
/// element census walks). A corrupted split (a comma spelled inside an
/// unbalanced string) desyncs segments into rowless leaves — the
/// `dynamic` under-report direction [`Engine::least_leaf_ty`] takes.
fn value_comma_segments(bytes: &[u8], start: usize, end: usize) -> Vec<(usize, usize)> {
    let end = end.min(bytes.len());
    let mut segments = Vec::new();
    let mut seg_start = start;
    let mut i = start;
    let mut depth = 0usize;
    while i < end {
        match bytes[i] {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => depth = depth.saturating_sub(1),
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

/// The first top-level `:` in `[start, end)`, string contents skipped —
/// the map-entry leaf's key/value split.
fn value_top_level_colon(bytes: &[u8], start: usize, end: usize) -> Option<usize> {
    let end = end.min(bytes.len());
    let mut i = start;
    let mut depth = 0usize;
    while i < end {
        match bytes[i] {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => depth = depth.saturating_sub(1),
            b':' if depth == 0 => return Some(i),
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

/// The end of a formal segment's signature part: the first top-level `=`
/// (the default value's head), or `end`.
fn value_default_cut(bytes: &[u8], start: usize, end: usize) -> usize {
    let end = end.min(bytes.len());
    let mut i = start;
    let mut depth = 0usize;
    while i < end {
        match bytes[i] {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => depth = depth.saturating_sub(1),
            b'=' if depth == 0 => return i,
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
    end
}

/// The first significant byte at or after `start`.
fn trim_value_ws(bytes: &[u8], start: usize, end: usize) -> usize {
    let end = end.min(bytes.len());
    let mut start = start.min(end);
    while start < end && bytes[start].is_ascii_whitespace() {
        start += 1;
    }
    start
}

/// The restricted supertype spellings, name+library keyed
/// (`TypeProviderImpl.isNonSubtypableClass`, upstream Dart SDK source,
/// pkg/analyzer/lib/src/dart/element/type_provider.dart:13-76 and :547-555):
/// every name lives only in its `dart:core`/`dart:async`/`dart:typed_data`
/// library, so an entry the analysis set resolves to its own declaration is
/// a user element and never restricted — the spelling is the SDK face
/// exactly when nothing indexed claims it. #2972. The modifier-use gate
/// (`overrides.rs`'s `non_subtypable_clause_head`) reads the same table —
/// one spelling list serves both the disallowed-subtype rows and the
/// clause-error drive gate.
pub(crate) const NON_SUBTYPABLE_SDK_NAMES: &[&str] = &[
    // dart:core (type_provider.dart:27-36).
    "bool",
    "double",
    "Enum",
    "int",
    "Null",
    "num",
    "Record",
    "String",
    // dart:async (type_provider.dart:25).
    "FutureOr",
    // dart:typed_data (type_provider.dart:38-76).
    "ByteBuffer",
    "ByteData",
    "Endian",
    "Float32List",
    "Float32x4",
    "Float32x4List",
    "Float64List",
    "Float64x2",
    "Float64x2List",
    "Int16List",
    "Int32List",
    "Int32x4",
    "Int32x4List",
    "Int64List",
    "Int8List",
    "TypedData",
    "Uint16List",
    "Uint32List",
    "Uint64List",
    "Uint8ClampedList",
    "Uint8List",
    "UnmodifiableByteBufferView",
    "UnmodifiableByteDataView",
    "UnmodifiableFloat32ListView",
    "UnmodifiableFloat32x4ListView",
    "UnmodifiableFloat64ListView",
    "UnmodifiableFloat64x2ListView",
    "UnmodifiableInt16ListView",
    "UnmodifiableInt32ListView",
    "UnmodifiableInt32x4ListView",
    "UnmodifiableInt64ListView",
    "UnmodifiableInt8ListView",
    "UnmodifiableUint16ListView",
    "UnmodifiableUint32ListView",
    "UnmodifiableUint64ListView",
    "UnmodifiableUint8ClampedListView",
    "UnmodifiableUint8ListView",
];

impl<'a> Engine<'a> {
    /// The `<…>` group of the nearest enclosing generalized function type
    /// whose formal list covers `offset` — the raw byte window of the
    /// declared parameter names. `None` when no `Function<…>(…)` encloses
    /// the offset inside its parameter list. The names inside bind
    /// throughout the signature (a formal's type `T` and a nested
    /// `void Function(T)` both reference the declaration), so the type-use
    /// miss path checks membership before reporting.
    pub(crate) fn gft_param_group_covering(&self, offset: u32) -> Option<(u32, u32)> {
        let fn_word_before = |from: u32| -> bool {
            if from < 8 {
                return false;
            }
            let start = from - 8;
            (0..8).all(|i| self.byte_at(start + i as u32) == Some(b"Function"[i]))
        };
        // Walk back over closers: each `>` is a candidate param-group
        // closer (its opener found by balanced scan), each `)` skips a
        // whole formals list. The FIRST `Function<…>(…)` whose formals
        // span covers the offset is the enclosing declaration.
        let mut back = offset;
        while back > 0 {
            back -= 1;
            match self.byte_at(back) {
                // `=>`/`>=`-style `>` is no group closer.
                Some(b'>') if self.byte_at(back.wrapping_sub(1)) != Some(b'=') => {
                    // The matching `<`: scanning back from just left of
                    // this `>`, `<` opens (+1) and `>` closes (−1); the
                    // opener is where the running sum first reaches +1.
                    let mut sum = 0i32;
                    let mut probe = back;
                    let mut lt = None;
                    while probe > 0 {
                        probe -= 1;
                        match self.byte_at(probe) {
                            Some(b'<') => {
                                sum += 1;
                                if sum == 1 {
                                    lt = Some(probe);
                                    break;
                                }
                            }
                            Some(b'>') => sum -= 1,
                            _ => {}
                        }
                    }
                    let Some(lt) = lt else {
                        continue;
                    };
                    if fn_word_before(lt) {
                        let mut after = back + 1;
                        if self.byte_at(after) == Some(b'?') {
                            after += 1;
                        }
                        if self.byte_at(after) == Some(b'(') {
                            // The formal list's closing `)` (depth-aware
                            // over nested lists): the offset sits inside
                            // the signature when it lands within.
                            let mut depth = 0i32;
                            let mut close = after;
                            loop {
                                let Some(b) = self.byte_at(close) else {
                                    break;
                                };
                                match b {
                                    b'(' => depth += 1,
                                    b')' => {
                                        depth -= 1;
                                        if depth == 0 {
                                            break;
                                        }
                                    }
                                    _ => {}
                                }
                                close += 1;
                            }
                            if back < offset && offset < close {
                                return Some((lt + 1, back));
                            }
                        }
                    }
                    // Not the enclosing GFT: jump over the whole `<…>`
                    // group and keep walking.
                    back = lt;
                }
                Some(b'<') => {
                    // An opener to the LEFT: the offset can only be inside
                    // its span when the balanced closer sits past it.
                    // Forward-match the `>`; a GFT-shaped group containing
                    // the offset answers, anything else just continues.
                    let mut sum = 0i32;
                    let mut probe = back;
                    let mut gt = None;
                    loop {
                        let Some(b) = self.byte_at(probe) else {
                            break;
                        };
                        match b {
                            b'<' => sum += 1,
                            b'>' => {
                                sum -= 1;
                                if sum == 0 {
                                    gt = Some(probe);
                                    break;
                                }
                            }
                            _ => {}
                        }
                        probe += 1;
                    }
                    if let (Some(gt), true) = (gt, fn_word_before(back)) {
                        let mut after = gt + 1;
                        if self.byte_at(after) == Some(b'?') {
                            after += 1;
                        }
                        if self.byte_at(after) == Some(b'(') && back < offset && offset < gt {
                            return Some((back + 1, gt));
                        }
                    }
                }
                Some(b')') => {
                    // A closer of an inner formals list: skip its whole
                    // span so nothing inside it reads as enclosing.
                    let mut inner = 0i32;
                    let mut probe = back;
                    loop {
                        match self.byte_at(probe) {
                            Some(b')') => inner += 1,
                            Some(b'(') => {
                                inner -= 1;
                                if inner == 0 {
                                    break;
                                }
                            }
                            None => break,
                            _ => {}
                        }
                        if probe == 0 {
                            break;
                        }
                        probe -= 1;
                    }
                    back = probe;
                }
                _ => {}
            }
        }
        None
    }

    /// The clause entry's SDK-face head when the entry names a restricted
    /// supertype (#2972, #3154): `None` when the entry resolves to a USER
    /// element (a same-named declaration shadows the SDK face — upstream
    /// keys the restriction on the RESOLVED element's library,
    /// `isNonSubtypableClass`, type_provider.dart:547-555), `Some` when it
    /// resolves to a `dart:*` library element (the restricted SDK face) or
    /// stays unresolved (the closed-spelling face). A one-level chase through
    /// a non-generic bare alias (`typedef A = int`, the written-shape face
    /// the element model keeps in `var_ty`) peels the alias head before the
    /// same decision. `Enum` never reaches `isNonSubtypableClass` upstream —
    /// the `isDartCoreEnum` arm in `_checkDirectSuperType`
    /// (inheritance_override.dart:610-620) intercepts it for every clause —
    /// so the head is filtered before the table lookup. Returns the head
    /// spelling (the chased one for an alias); the REPORT's `{0}` stays the
    /// entry's written name (`Classes can't mixin 'T'.`, messages.yaml
    /// :17358).
    fn disallowed_subtype_head(&self, entry: StringId) -> Option<StringId> {
        let mut head = entry;
        // An unresolved alias-shape entry keeps the written head; a resolved
        // one chases the aliased face. `resolve_container` runs on the
        // written entry first: a user typedef named `int` shadows the SDK
        // `int` before its `var_ty` is ever read.
        let resolved_owner = resolve_container(
            self.graph,
            self.namespaces,
            self.index,
            self.lib,
            self.file,
            entry,
        )
        .map(|(_, owner_file)| owner_file);
        match resolved_owner {
            Some(owner_file) => {
                // The resolved element's library decides the restriction:
                // `dart:*` is the SDK face, anything else is a user shadow
                // (type_provider.dart:550-553).
                let is_sdk = self
                    .graph
                    .library_of_file(owner_file)
                    .is_some_and(|lib| self.graph.uri(lib).starts_with("dart:"));
                if !is_sdk {
                    return None;
                }
            }
            None => {
                // Unresolved at the entry: the only chase the model keeps is
                // the bare non-generic alias arm.
                if let Some(decl) = self.index.get(self.file).and_then(|f| f.decl(entry)) {
                    if decl.kind != MicroKind::TypeAlias || decl.type_params != 0 {
                        return None;
                    }
                    // The head is the resolved type's element name — a
                    // spelled `FutureOr<int>` still lands on `FutureOr` the
                    // way `F`'s interface-type expansion hands upstream its
                    // `dart:async` element (a spelled argument never changes
                    // the restricted face `isNonSubtypableClass` reads).
                    match decl.var_ty {
                        Ty::Named { name, .. } => head = name,
                        // The collector folds the unshadowed `Null` spelling
                        // to the dedicated variant (its annotation fold) —
                        // that is dart:core's restricted `Null` at this
                        // model's grade.
                        Ty::Null => head = self.intern("Null"),
                        _ => return None,
                    }
                    // The chased head still goes through the same
                    // resolution decision: a user element under the chased
                    // name stays silent, an SDK element keeps the face.
                    if let Some((_, owner_file)) = resolve_container(
                        self.graph,
                        self.namespaces,
                        self.index,
                        self.lib,
                        self.file,
                        head,
                    ) {
                        let is_sdk = self
                            .graph
                            .library_of_file(owner_file)
                            .is_some_and(|lib| self.graph.uri(lib).starts_with("dart:"));
                        if !is_sdk {
                            return None;
                        }
                    }
                }
            }
        }
        // `Enum`'s every clause face routes through the dedicated
        // `isDartCoreEnum` arm upstream — `isNonSubtypableClass` never sees
        // it (inheritance_override.dart:610-620); the `with` conjunct at
        // error_verifier.dart:2461-2463 carries the same carve-out on the
        // older path.
        if self.strings.resolve(head) == "Enum" {
            return None;
        }
        NON_SUBTYPABLE_SDK_NAMES
            .contains(&self.strings.resolve(head))
            .then_some(head)
    }

    /// The `subtype_of_disallowed_type` teardown (#2972): every clause face
    /// of every container in this unit whose target is a restricted SDK
    /// class reports once, mirroring the upstream per-clause walks —
    /// `_checkForExtendsDisallowedClass` (error_verifier.dart:4692-4697),
    /// the `with` face inside `_checkForAllMixinErrorCodes` (:2446-2463,
    /// the `!mixinType.isDartCoreEnum` conjunct suppressing the core
    /// `Enum`), `_checkForImplementsClauseErrorCodes` (:5365-5381), and
    /// `_checkForOnClauseErrorCodes` (:6724-6745). The clause split per
    /// kind follows the visitors: class and named mixin application carry
    /// extends+with+implements (`visitClassDeclaration` and
    /// `visitClassTypeAlias`), enums with+implements (:855-864, a null
    /// superclass), mixins on+implements (`_checkMixinInheritance`,
    /// :7808-7817). Runs after the parse like the other finalizers — the
    /// whole-unit container set is what the shadow test resolves against.
    /// The dart: current-library exemption is `isScheme('dart')`
    /// (:4734-4736); rows anchor at the declaring name, the subtype
    /// family's convention (`HeaderClauseSpans`'s doc).
    pub(crate) fn finalize_disallowed_subtypes(&mut self) {
        if self.graph.uri(self.lib).starts_with("dart:") {
            return;
        }
        let Some(elements) = self.index.get(self.file) else {
            return;
        };
        // The face list collects first — the emit loop needs `&mut self`
        // while the element index and the classifier read shared state.
        let mut faces: Vec<(SemanticCode, SourceRange)> = Vec::new();
        for (decl_name, container) in elements.containers() {
            let Some(site) = self
                .decl_sites
                .iter()
                .rev()
                .find(|site| matches!(site.kind, DeclKind::Container) && site.name == decl_name)
            else {
                continue;
            };
            let mut entries: Vec<(SemanticCode, StringId)> = Vec::new();
            match container.kind {
                ContainerKind::Class => {
                    if let Some(superclass) = container.superclass {
                        entries.push((SemanticCode::SubtypeOfDisallowedTypeInExtends, superclass));
                    }
                    entries.extend(
                        container
                            .with_clause
                            .iter()
                            .map(|mixin| (SemanticCode::SubtypeOfDisallowedTypeInWith, *mixin)),
                    );
                    entries.extend(container.implements_clause.iter().map(|interface| {
                        (SemanticCode::SubtypeOfDisallowedTypeInImplements, *interface)
                    }));
                }
                ContainerKind::Enum => {
                    entries.extend(
                        container
                            .with_clause
                            .iter()
                            .map(|mixin| (SemanticCode::SubtypeOfDisallowedTypeInWith, *mixin)),
                    );
                    entries.extend(container.implements_clause.iter().map(|interface| {
                        (SemanticCode::SubtypeOfDisallowedTypeInImplements, *interface)
                    }));
                }
                ContainerKind::Mixin => {
                    entries.extend(
                        container
                            .on_clause
                            .iter()
                            .map(|constraint| (SemanticCode::SubtypeOfDisallowedTypeInOn, *constraint)),
                    );
                    entries.extend(container.implements_clause.iter().map(|interface| {
                        (SemanticCode::SubtypeOfDisallowedTypeInImplements, *interface)
                    }));
                }
                // Extension types carry their own code
                // (`extension_type_implements_disallowed_type`,
                // messages.yaml:6752-6757); extensions declare no
                // supertype faces.
                _ => continue,
            }
            // Per-clause-entry ordinals keyed on (code, entry): the n-th
            // pushed row for `entry` under `code` anchors the n-th written
            // `entry` under that clause keyword in source (the model's
            // clause lists are in written order).
            let mut ordinals: FxHashMap<(SemanticCode, StringId), usize> =
                FxHashMap::default();
            for (code, entry) in entries {
                // `Enum` never reaches this table (filtered inside
                // `disallowed_subtype_head` — upstream's `isDartCoreEnum`
                // arm, inheritance_override.dart:610-620).
                if self.disallowed_subtype_head(entry).is_some() {
                    let ordinal = ordinals.entry((code, entry)).or_insert(0);
                    let range = self
                        .clause_entry_range(site.range, code, entry, *ordinal)
                        .unwrap_or(site.range);
                    *ordinal += 1;
                    faces.push((code, range));
                }
            }
        }
        for (code, range) in faces {
            let text = self.disallowed_subtype_message(range);
            self.emit_corrected(code, range, &text);
        }
    }

    /// The clause-entry span for `entry` under the `code`'s own clause in
    /// the header opened at `site`'s declaring name (#3154): walks source
    /// forward from the name's end, skips the container's own
    /// type-parameter group, then tokenizes the header — only entries
    /// under `code`'s own clause keyword are candidates, so `class A
    /// extends Null implements Null` anchors each row on its own arm
    /// (`SubtypeOfDisallowedTypeInExtends` reads the `extends` list only,
    /// `…InImplements` reads `implements`, `…InWith` reads `with`,
    /// `…InOn` reads `on`). `None` for a header whose written shape no
    /// longer carries the entry; the caller falls back to the declaring
    /// name's span then.
    fn clause_entry_range(
        &self,
        site: SourceRange,
        code: SemanticCode,
        entry: StringId,
        ordinal: usize,
    ) -> Option<SourceRange> {
        let entry_text = self.strings.resolve(entry);
        let bytes = self.source.as_bytes();
        // The clause keyword this code's entries live under, plus whether
        // a bare `=` head counts (`extends` reads the named mixin
        // application's `class A = S` head as its own superclass face).
        let (clause_kw, alias_head): (&[u8], bool) = match code {
            SemanticCode::SubtypeOfDisallowedTypeInExtends => (b"extends", true),
            SemanticCode::SubtypeOfDisallowedTypeInWith => (b"with", false),
            SemanticCode::SubtypeOfDisallowedTypeInImplements => (b"implements", false),
            SemanticCode::SubtypeOfDisallowedTypeInOn => (b"on", false),
            _ => return None,
        };
        let mut at = self.byte_of(site.end());
        at = self.skip_angle_group(at);
        // `matched` counts written entries under `clause_kw` whose head
        // spells `entry_text`: the n-th pushed row anchors the n-th match
        // in source (the element model's clause lists are in written
        // order, so entry `i` pairs with written entry `i`).
        let mut matched = 0usize;
        loop {
            let probe = self.skip_ws_bytes(at);
            match bytes.get(probe) {
                Some(b'{') | Some(b';') | None => return None,
                Some(b'(') | Some(b'[') => return None,
                Some(b'=') => {
                    // A named mixin application's `=` head: the `extends`
                    // code's own superclass face (`class A = S with M`'s
                    // `S`); other codes skip it and keep walking toward
                    // their own clause keyword.
                    if alias_head {
                        let head_at = self.skip_ws_bytes(probe + 1);
                        let (span, next) =
                            self.scan_clause_type_at(head_at, entry_text);
                        if let Some(span) = span {
                            if matched == ordinal {
                                return Some(span);
                            }
                            matched += 1;
                        }
                        at = next;
                    } else {
                        at = probe + 1;
                    }
                    continue;
                }
                _ => {}
            }
            let Some((_, kw_end, kw)) = self.scan_ident_at(probe) else {
                return None;
            };
            if kw == clause_kw {
                let mut cur = kw_end;
                loop {
                    let head_at = self.skip_ws_bytes(cur);
                    let (span, next) = self.scan_clause_type_at(head_at, entry_text);
                    if let Some(span) = span {
                        if matched == ordinal {
                            return Some(span);
                        }
                        matched += 1;
                    }
                    cur = next;
                    let comma = self.skip_ws_bytes(cur);
                    if bytes.get(comma) == Some(&b',') {
                        cur = comma + 1;
                        continue;
                    }
                    break;
                }
                at = cur;
            } else {
                at = kw_end;
            }
        }
    }

    /// The byte offset past one balanced `<…>` group when `at` opens one;
    /// `at` unchanged otherwise. The container's own type-parameter group
    /// is the only arm this scanner needs (`class C<T extends Bound>`'s
    /// `Bound` is a bound, never a clause entry).
    fn skip_angle_group(&self, at: usize) -> usize {
        let bytes = self.source.as_bytes();
        if bytes.get(at) != Some(&b'<') {
            return at;
        }
        let mut depth = 0i32;
        let mut probe = at;
        while let Some(&b) = bytes.get(probe) {
            match b {
                b'<' => depth += 1,
                b'>' => {
                    depth -= 1;
                    if depth == 0 {
                        return probe + 1;
                    }
                }
                _ => {}
            }
            probe += 1;
        }
        at
    }

    /// The identifier bytes at `at` (skipping leading whitespace and
    /// comments): `(start, end, spelling)`. `None` past the unit's end.
    fn scan_ident_at(&self, at: usize) -> Option<(usize, usize, &[u8])> {
        let bytes = self.source.as_bytes();
        let mut probe = self.skip_ws_and_comments(at);
        let start = probe;
        while let Some(&b) = bytes.get(probe) {
            if !(b.is_ascii_alphanumeric() || b == b'_' || b == b'$') {
                break;
            }
            probe += 1;
        }
        if probe == start {
            return None;
        }
        Some((start, probe, &bytes[start..probe]))
    }

    /// Whitespace and `//`/`/*…*/` runs skipped (the header's own tokens).
    fn skip_ws_and_comments(&self, at: usize) -> usize {
        let bytes = self.source.as_bytes();
        let mut at = at;
        loop {
            while let Some(&b) = bytes.get(at) {
                if b.is_ascii_whitespace() {
                    at += 1;
                } else {
                    break;
                }
            }
            if bytes.get(at) == Some(&b'/') && bytes.get(at + 1) == Some(&b'/') {
                while let Some(&b) = bytes.get(at) {
                    if b == b'\n' {
                        break;
                    }
                    at += 1;
                }
                continue;
            }
            if bytes.get(at) == Some(&b'/') && bytes.get(at + 1) == Some(&b'*') {
                let mut probe = at + 2;
                while probe + 1 < bytes.len() {
                    if bytes[probe] == b'*' && bytes[probe + 1] == b'/' {
                        break;
                    }
                    probe += 1;
                }
                at = (probe + 2).min(bytes.len());
                continue;
            }
            return at;
        }
    }

    /// Whitespace skipped, returning the byte offset of the next
    /// non-whitespace byte (the entry scan works in bytes).
    fn skip_ws_bytes(&self, at: usize) -> usize {
        self.skip_ws_and_comments(at)
    }

    /// One clause-entry's span at byte offset `at`, returned paired with
    /// the next byte to scan: `(Some(range), next)` when the entry parses
    /// AND its head spelling matches `entry_text`, `(None, next)` when it
    /// parses but the head is a different name. The span covers the head
    /// (with any `p.` prefix) through the `<…>` group's closer or a
    /// trailing `?`, mirroring upstream's whole-NamedType anchor
    /// (`note_header_type_span`).
    fn scan_clause_type_at(&self, at: usize, entry_text: &str) -> (Option<SourceRange>, usize) {
        let bytes = self.source.as_bytes();
        let head_start = at;
        let Some((_, first_end, _)) = self.scan_ident_at(at) else {
            return (None, at);
        };
        let mut probe = first_end;
        // A `p.S` continuation extends the head's written span through the
        // leaf name.
        if bytes.get(probe) == Some(&b'.') {
            let after_dot = self.skip_ws_bytes(probe + 1);
            if let Some((_, leaf_end, _)) = self.scan_ident_at(after_dot) {
                probe = leaf_end;
            }
        }
        // Optional `<…>` group and trailing `?` extend the entry's span.
        let mut end = probe;
        if bytes.get(end) == Some(&b'<') {
            end = self.skip_angle_group(end);
        }
        if bytes.get(end) == Some(&b'?') {
            end += 1;
        }
        let head_bytes = &bytes[head_start..probe];
        // The leaf spelling decides the match: the element model keys the
        // clause lists on the dot-joined spelling (`p.S`), while a bare
        // name is its own leaf.
        let leaf = head_bytes
            .rsplit(|&b| b == b'.')
            .next()
            .unwrap_or(head_bytes);
        let matches = head_bytes == entry_text.as_bytes() || leaf == entry_text.as_bytes();
        let range = matches.then(|| SourceRange {
            offset: self.utf16_of(head_start),
            length: self.utf16_of(end) - self.utf16_of(head_start),
        });
        (range, end)
    }

    /// The `{0}` argument of a `subtype_of_disallowed_type` row (#3154):
    /// the written clause entry's type display — `int`, `Null`, `FutureOr`
    /// — with `FutureOr`'s bare head widening to `FutureOr<dynamic>`
    /// (upstream's `namedType.type` display: the bare `FutureOr` resolves
    /// to `FutureOr<dynamic>`, `DartType.getDisplayString`,
    /// inheritance_override.dart:648-649).
    fn disallowed_subtype_message(&self, range: SourceRange) -> String {
        let start = self.byte_of(range.offset);
        let end = self.byte_of(range.end()).min(self.source.len());
        let text = &self.source[start..end];
        if text == "FutureOr" {
            "FutureOr<dynamic>".to_string()
        } else {
            text.to_string()
        }
    }
}

/// The #2935 condition-verdict pins (`non_bool_*` faces on untyped
/// shapes, `&&`/`||`/`assert` face selection, const contexts). `#[path]`
/// resolves against this file's directory — `diagnose/`.
#[cfg(test)]
#[path = "ticket_2935_tests.rs"]
mod ticket_2935_tests;

/// The #3164 `use_of_void_result` pins (the operand-consumption faces the
/// engine decides at its own operand-type seams — binary operands,
/// `!`-postfix, `!`-prefix, and every condition operand).
#[cfg(test)]
#[path = "ticket_3164_tests.rs"]
mod ticket_3164_tests;

/// The #3470 `use_of_void_result` USE-SITE span pins: the report anchors
/// the operand's whole expression for every non-`MethodInvocation` shape
/// (`FunctionExpressionInvocation` sends through `void Function()`
/// formals/locals and function-typed members, `PropertyAccess` chains,
/// `NamedExpression`/`RecordLiteralNamedField` entries), and keeps the
/// `methodName` token only for declared-method sends.
#[cfg(test)]
#[path = "ticket_3470_tests.rs"]
mod ticket_3470_tests;

/// The #3104 dead-region suppression pins: the flow layer's `DeadCode`
/// intervals re-home into [`Engine::dead_windows`] before `resolve_rows`,
/// and `emit_unchecked_nullable` re-decides each candidate against them —
/// a row anchored inside a dead region reports only `dead_code` upstream.
/// `#[path]` resolves against this file's directory — `diagnose/`.
#[cfg(test)]
#[path = "ticket_3104_tests.rs"]
mod ticket_3104_tests;

/// The #2999 `unused_local_variable` write-vs-read credit pins: the
/// statement-position discard law (`_isReadIdentifier`,
/// unused_local_elements_verifier.dart:429-453) — `++`/`--`/non-`??=`
/// compound/plain assignments never spare the declaration, `??=` and
/// every nested-position read do.
#[cfg(test)]
#[path = "ticket_2999_tests.rs"]
mod ticket_2999_tests;

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
