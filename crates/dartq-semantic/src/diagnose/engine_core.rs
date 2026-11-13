use super::*;

/// The if-null composite's S-branch decision — see
/// [`Engine::if_null_context_face`].
enum IfNullContext {
    /// The join face `UP(NonNull(T1), T2)` stands (no schema, `T <: S`, or
    /// the branch closed against `S`).
    Join(Ty),
    /// The context schema stamped the node (`S`).
    Schema(Ty),
    /// Every decidable input admits the branch; only the target's promoted
    /// read is outstanding (the executable's replay owns it).
    Pending,
}

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
    /// The void operand of a prefix operator: the member spelling
    /// (`unary-`, `~`) resolves as a METHOD invocation
    /// (`uncheckedMethodInvocationOfNullableValue` —
    /// `type_property_resolver.dart`'s member-call arm; the oracle's
    /// `The method 'unary-' …`/`The method '~' …` rows).
    Operator(&'static str),
}

impl UncheckedOperandFace {
    /// The (problem, correction) texts of the face.
    fn texts(self) -> (String, String) {
        match self {
            UncheckedOperandFace::ForInIterator => (
                "A nullable expression can't be used as an iterator in a for-in loop."
                    .to_string(),
                "Try checking that the value isn't 'null' before using it as an iterator."
                    .to_string(),
            ),
            UncheckedOperandFace::Spread => (
                "A nullable expression can't be used in a spread.".to_string(),
                "Try checking that the value isn't 'null' before using it in a spread, or use a null-aware spread."
                    .to_string(),
            ),
            UncheckedOperandFace::YieldEach => (
                "A nullable expression can't be used in a yield-each statement.".to_string(),
                "Try checking that the value isn't 'null' before using it in a yield-each statement."
                    .to_string(),
            ),
            UncheckedOperandFace::Operator(name) => (
                SemanticCode::unchecked_nullable_message(UncheckedNullableKind::Method, name),
                SemanticCode::unchecked_nullable_correction(UncheckedNullableKind::Method)
                    .to_string(),
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

// The pending composite-fold records (#3980): a binary or conditional
// fold whose operand/arm `read_type_at` consult starved because the
// flow's read-type table only fills after `parse_unit` returns. The fold
// records the operand rows and fold-time types it needs; `filter_staged`
// drains the table once `read_types` is populated and recomputes the
// promoted result, feeding the staged-check `from` ladder the corrected
// composite type. Thread-local rather than an engine field so the fold
// state stays untouched — a sequential fold drains it before the next
// unit parses (the `DEFERRED_DEFAULTS` precedent).
thread_local! {
    static PENDING_COMPOSITE_FOLDS: std::cell::RefCell<Vec<PendingCompositeFold>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

/// One recorded composite fold awaiting its promoted-type recompute.
/// `file` tags the record so a stale row from a previous unit on the same
/// rayon thread never answers a consult for a different file.
enum PendingCompositeFold {
    /// `fold_binary`'s record: the operator, the operand rows it cloned,
    /// and the fold-time result type (the `UNKNOWN`/`bool`/declared face
    /// the starvation produced). `node_range` is the folded `Value` row's
    /// span — the join key the staged-check `from` ladder consults.
    Binary {
        file: FileId,
        node_range: SourceRange,
        op: StringId,
        left: Row,
        right: Row,
        left_ty: Ty,
        right_ty: Ty,
        /// The scope-parameter spellings live at fold time
        /// (`type_params` + `pending_type_params`): the teardown liveness
        /// sets are already truncated, so the recompute reads param
        /// spellings — and their bound faces — from this snapshot.
        params: Box<[StringId]>,
        result: Ty,
    },
    /// `fold_conditional`'s record: the arm rows and the fold-time result
    /// type. `context` captures the `return_expression_context` answer the
    /// fold computed (the frame-dependent face is dead at teardown).
    Conditional {
        file: FileId,
        node_range: SourceRange,
        then_row: Row,
        else_row: Row,
        then_ty: Ty,
        else_ty: Ty,
        context: Option<Ty>,
        result: Ty,
    },
}

/// The cap on pending records per unit: a pathological file with thousands
/// of composite folds must not grow the thread-local unboundedly. At the
/// cap the record silently drops — the pre-fix starvation posture stands
/// (the under-report direction, never a panic).
const PENDING_COMPOSITE_FOLD_CAP: usize = 512;

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
        triple_shift_enabled: bool,
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
            literal_extents: Vec::new(),
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
            exec_windows: Vec::new(),
            infer_poisoned_params: FxHashSet::default(),
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
            cond_expr_depth: 0,
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
            deferred_composite_rebinds: Vec::new(),
            composite_rewrites: Vec::new(),
            deferred_invocation_faces: Vec::new(),
            promotion_function_reads: FxHashSet::default(),
            fn_is_promotions: Vec::new(),
            rebound_locals: FxHashMap::default(),
            clause_anchor: None,
            primary_constructors_enabled,
            augmentations_enabled,
            variance_enabled,
            patterns_enabled,
            triple_shift_enabled,
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
            closure_lit_rows: FxHashMap::default(),
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
            open_member_face: None,
            member_face_spans: Vec::new(),
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
            null_aware_member_ops: FxHashMap::default(),
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
            if_null_s_pending: FxHashSet::default(),
            deferred_if_null_bounds: Vec::new(),
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
            void_read_rows: Vec::new(),
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
    ///
    /// The gate reads the void SPELLING through the alias fold, not the
    /// dedicated variant alone: a typedef-folded `void` (`typedef V =
    /// void; V x = …; x == null`) reaches the operand faces as the alias
    /// head `Named{V}` (or its RHS `Named{void}`) while upstream's
    /// `staticType` answers `VoidTypeImpl` for the grounded use — the
    /// dedicated-variant check would under-report both faces.
    pub(crate) fn emit_use_of_void_result(&mut self, ty: Ty, operand: &Row) {
        let raw = ty;
        let ty = self.fold_alias_ty(ty);
        if !self.is_void_spelling(ty) {
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
            (d.range == range || d.range == operand.range) && d.code.name == "use_of_void_result"
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
    ///
    /// One last widening precedes every shape's answer: an operand row
    /// anchored at a closing `)` is the fold's face of a
    /// ParenthesizedExpression (the listener pushes the `)` token as the
    /// group's row). The oracle's `.at(expression)` covers the parens —
    /// `(x)` reports `(x)`, `((x))` reports `((x))` — so the resolved span
    /// grows back to each matching `(` ([`Self::junction_operand`]'s
    /// parenthesized-operand walk, minus its operator probe).
    pub(crate) fn use_site_range(&self, operand: &Row) -> SourceRange {
        let range = if matches!(operand.intent, Intent::Invoke) {
            self.value_send_span(operand).unwrap_or(operand.range)
        } else {
            self.property_access_span(operand.range)
                .unwrap_or(operand.range)
        };
        let mut range = self.enclosing_paren_span(range);
        // The caller may hand in a leaf row nested inside a
        // ParenthesizedExpression's `)`-anchored group row — a `?:` arm or
        // a bare read the seam picked instead of the group's own face
        // (`(true ? x : x) is T` reads the arm `x` where the oracle's
        // operand is the whole group). While the resolved span's tail sits
        // exactly where a `)`-anchored GROUP row begins, the operand's
        // expression face is that group: expand through its matching `(`
        // and repeat (`((x))` walks out one pair at a time). A call's
        // closing `)` carries no group row — only `end_parenthesized
        // _expression` pushes one — so an argument leaf like the `x` of
        // `f(x)` never expands.
        loop {
            let has_group_row = self.rows.iter().any(|row| {
                row.range.offset == range.end()
                    && row.range.length == 1
                    && self.tokens.as_slice().iter().any(|t| {
                        t.utf16_offset == row.range.offset
                            && matches!(t.kind, dartq_scanner::TokenKind::CloseParen)
                    })
            });
            if !has_group_row {
                return range;
            }
            range = self.enclosing_paren_span(SourceRange {
                offset: range.end(),
                length: 1,
            });
            if range.length == 1 {
                // The `)` matched nothing (unbalanced recovery shape) —
                // the expanded span still sits on the same `)`, so any
                // further pass can only repeat it. Keep the leaf's span.
                return range;
            }
            // `enclosing_paren_span` grew the span to the `(`..`)` pair;
            // a still-deeper group may follow (`((x))`'s outer `)` row).
            if self
                .tokens
                .as_slice()
                .iter()
                .find(|t| t.utf16_offset == range.end() && t.byte_len == 1)
                .is_none()
            {
                return range;
            }
        }
    }

    /// The span's enclosing-paren expansion for a `)`-anchored row: while
    /// the resolved span sits exactly on a `)` token, grow it to cover the
    /// matching `(` (and repeat — `((x))` walks out one pair at a time).
    /// Any other anchor returns unchanged.
    fn enclosing_paren_span(&self, mut range: SourceRange) -> SourceRange {
        let tokens = self.tokens.as_slice();
        while let Some(index) = tokens
            .iter()
            .position(|t| t.utf16_offset == range.offset && t.byte_len == range.length)
            .filter(|index| matches!(tokens[*index].kind, dartq_scanner::TokenKind::CloseParen))
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
            let Some(open) = open else { break };
            range = SourceRange {
                offset: tokens[open].utf16_offset,
                length: range.end() - tokens[open].utf16_offset,
            };
        }
        range
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
        if !self.is_value_callee(operand) {
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

    /// Whether an `Invoke` row's callee is a VALUE — the
    /// `FunctionExpressionInvocation` shape upstream's resolver splits
    /// from a `MethodInvocation`: a member send whose resolved member is a
    /// field or getter (a function-typed property — a `Method` member's
    /// send is a declared-method invocation whose static type is the
    /// method's own return, never a `.call` dispatch), or a bare send on a
    /// `void Function()`-typed formal/local or a local with no
    /// declared-function capture. `false` for every non-send row and every
    /// declared-callee send — the predicate [`Self::value_send_span`]'s
    /// span arm, [`Self::callable_call_ret`]'s implicit-`call` arm, and
    /// [`Self::operand_static_ty`]'s misfired-`call`-ret salvage share.
    fn is_value_callee(&self, row: &Row) -> bool {
        match &row.shape {
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
                    _ => return false,
                };
                matches!(
                    self.walk_members(type_name, row.name),
                    Walk::Found(entry)
                        if matches!(entry.kind, MemberKind::Field | MemberKind::Getter)
                )
            }
            _ => {
                self.fn_var_sigs.contains_key(&row.name)
                    || (self.frames.iter().any(|f| {
                        f.locals.contains(&row.name) || f.fn_formal_rets.contains_key(&row.name)
                    }) && self.local_fn_sig(row.name).is_none())
            }
        }
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

    /// Drains [`PENDING_COMPOSITE_FOLDS`] and recomputes each recorded
    /// composite fold's result type against the now-populated
    /// `read_types` table (#3980). Returns the `(node_range → ty)` rewrite
    /// entries whose recomputed type differs from the fold-time answer,
    /// offset-sorted for the staged-check `from` ladder's binary search.
    /// Records tagged for a different file drop silently — the thread-local
    /// is per-rayon-thread and a unit that errored before `filter_staged`
    /// could leave stragglers the next unit must not answer for.
    pub(crate) fn drain_pending_composite_folds(&mut self) -> Vec<(SourceRange, Ty)> {
        let records =
            PENDING_COMPOSITE_FOLDS.with(|queue| std::mem::take(&mut *queue.borrow_mut()));
        // The rewrite map builds in fold order: an outer composite's
        // operand is the inner composite's `Value` row, pushed AFTER the
        // inner's own record, so the inner's recompute already sits in
        // `rewrites` when the outer consults it — the order-independent
        // forward pass the records' fold-order guarantees.
        let mut rewrites: Vec<(SourceRange, Ty)> = Vec::new();
        for record in records {
            let (node_range, new_ty, old_ty) = match record {
                PendingCompositeFold::Binary {
                    file,
                    node_range,
                    op,
                    left,
                    right,
                    left_ty,
                    right_ty,
                    params,
                    result,
                } => {
                    if file != self.file {
                        continue;
                    }
                    let new_ty = self.recompute_binary_result(
                        op,
                        &left,
                        &right,
                        left_ty,
                        right_ty,
                        &params,
                        &rewrites,
                    );
                    (node_range, new_ty, result)
                }
                PendingCompositeFold::Conditional {
                    file,
                    node_range,
                    then_row,
                    else_row,
                    then_ty,
                    else_ty,
                    context,
                    result,
                } => {
                    if file != self.file {
                        continue;
                    }
                    let new_ty = self.recompute_conditional_result(
                        &then_row, &else_row, then_ty, else_ty, context, &rewrites,
                    );
                    (node_range, new_ty, result)
                }
            };
            if new_ty != old_ty {
                rewrites.push((node_range, new_ty));
            }
        }
        rewrites.sort_by_key(|(range, _)| range.offset);
        rewrites
    }

    /// Whether `range` names a recorded composite fold's `Value` row —
    /// the `PendingCompositeFold` join key the inferred-local re-bind
    /// (#4265) stages against. `end_variable_initializer` runs during
    /// `parse_unit`, before `read_types` fills, so a `?:`/`??`/binary
    /// initializer's arm promotions starve the fold-time LUB; the
    /// record's existence is the signal the pass-2 re-bind consults.
    pub(crate) fn is_pending_composite(&self, range: SourceRange) -> bool {
        PENDING_COMPOSITE_FOLDS.with(|queue| {
            queue.borrow().iter().any(|record| match record {
                PendingCompositeFold::Binary {
                    file, node_range, ..
                }
                | PendingCompositeFold::Conditional {
                    file, node_range, ..
                } => *file == self.file && *node_range == range,
            })
        })
    }

    /// One composite operand/arm's teardown type: the inner composite's
    /// recomputed face when the operand IS a recorded node (its `Value`
    /// row's range is the inner `node_range`, paren-transparent), else the
    /// flow read-type fact at the operand's own span, else the fold-time
    /// fallback. `rewrites` holds only the records already recomputed in
    /// this forward pass — an operand nested deeper than the current record
    /// is by construction earlier in fold order and already present.
    fn composite_operand_ty(&self, row: &Row, fallback: Ty, rewrites: &[(SourceRange, Ty)]) -> Ty {
        let key = self.strip_outer_parens(row.range);
        rewrites
            .iter()
            .find(|(range, _)| *range == key)
            .map(|(_, ty)| *ty)
            .or_else(|| {
                self.read_type_at(row.range)
                    .filter(|promoted| *promoted != Ty::Unknown)
            })
            .unwrap_or(fallback)
    }

    /// [`PendingCompositeFold::Binary`]'s teardown recompute: re-asks the
    /// operand type consults now that `read_types` is populated and the
    /// inner composites' rewrites stand, then re-runs the pure result-ty
    /// derivation — the operator signature resolution and the numeric
    /// refinement — skipping the side-effecting `check_binary_argument`
    /// (its staged `Argument` row already exists from the fold and the
    /// `from` ladder cleans it independently). The fold-time
    /// `left_ty`/`right_ty` are the post-`bound_face` fallbacks: a
    /// standing promotion substitutes the concrete promoted face. An
    /// unpromoted scope-parameter read answers its raw parameter spelling
    /// (`T`, not the bound) — the teardown liveness sets the bound walk
    /// reads are already truncated, so the read answer is held against the
    /// fold-time `params` snapshot: a bare snapshot-parameter spelling
    /// never beats the fold-time bound face (`T extends num` reads `num`).
    fn recompute_binary_result(
        &mut self,
        op: StringId,
        left: &Row,
        right: &Row,
        left_ty_fallback: Ty,
        right_ty_fallback: Ty,
        params: &[StringId],
        rewrites: &[(SourceRange, Ty)],
    ) -> Ty {
        let bound_pref = |ty: Ty, fallback: Ty| match ty {
            Ty::Named { name, args, .. } if args == ArgListId::EMPTY && params.contains(&name) => {
                fallback
            }
            ty => ty,
        };
        let left_ty = bound_pref(
            self.composite_operand_ty(left, left_ty_fallback, rewrites),
            left_ty_fallback,
        );
        let right_ty = bound_pref(
            self.composite_operand_ty(right, right_ty_fallback, rewrites),
            right_ty_fallback,
        );
        let op_text = self.strings.resolve(op).to_string();
        // Mirror `fold_binary`'s result arms minus the
        // `check_binary_argument` side effect — the nullable-extension
        // face first, then the resolved signature + numeric refinement.
        if let Some((_formal, declared)) = self.nullable_extension_operator_sig(left_ty, op) {
            declared
        } else {
            match self.binary_operator_sig_typed(&op_text, left_ty) {
                Some((_formal, declared)) => {
                    if left_ty == Ty::Dynamic {
                        Ty::Dynamic
                    } else {
                        let face = self.operand_face(right, right_ty);
                        self.refine_numeric_binary(&op_text, left_ty, face, declared)
                    }
                }
                None => Ty::UNKNOWN,
            }
        }
    }

    /// [`PendingCompositeFold::Conditional`]'s teardown recompute: re-asks
    /// the arm type consults, re-joins through `composite_lub`, then
    /// re-applies the captured return-context face —
    /// `return_expression_context` reads `self.frames` (dead at teardown),
    /// so the fold stored its `Option<Ty>` answer and the `assignable`
    /// ladder below mirrors `conditional_return_context_ty`'s
    /// gate-for-gate.
    fn recompute_conditional_result(
        &mut self,
        then_row: &Row,
        else_row: &Row,
        then_ty_fallback: Ty,
        else_ty_fallback: Ty,
        context: Option<Ty>,
        rewrites: &[(SourceRange, Ty)],
    ) -> Ty {
        let then_ty = self.composite_operand_ty(then_row, then_ty_fallback, rewrites);
        let else_ty = self.composite_operand_ty(else_row, else_ty_fallback, rewrites);
        let mut ty = if then_ty == Ty::UNKNOWN || else_ty == Ty::UNKNOWN {
            Ty::UNKNOWN
        } else {
            self.composite_lub(then_ty, else_ty, false)
        };
        if ty != Ty::UNKNOWN
            && let Some(context) = context
            && self.assignable(ty, context, false) != Some(true)
            && self.assignable(then_ty, context, false) == Some(true)
            && self.assignable(else_ty, context, false) == Some(true)
        {
            ty = context;
        }
        ty
    }

    /// Strips balanced outer paren pairs from `range` — `(n + 1)` unwraps
    /// to `n + 1`, `((n + 1))` to `n + 1` — so a parenthesized composite's
    /// `Value` row keys the same `node_range` the fold recorded. Whitespace
    /// just inside each paren pair trims with it. The walk stops at the
    /// first byte that is not a wrapping paren; a `(` whose match closes
    /// before the range's end does not wrap the whole span and ends the
    /// strip. Byte access rides [`Self::byte_at`] (UTF-16 → byte), and the
    /// paren bytes never occur inside a multi-byte UTF-8 sequence, so the
    /// scan is exact on any source.
    pub(crate) fn strip_outer_parens(&self, mut range: SourceRange) -> SourceRange {
        loop {
            let start = range.offset;
            let end = range.end();
            if end <= start.saturating_add(1)
                || self.byte_at(start) != Some(b'(')
                || self.byte_at(end.saturating_sub(1)) != Some(b')')
            {
                return range;
            }
            // The `(` at `start` must match the `)` at `end - 1`: depth
            // returns to zero only at the final byte.
            let mut depth = 0u32;
            let mut pos = start;
            let mut wraps = false;
            while pos < end {
                match self.byte_at(pos) {
                    Some(b'(') => depth += 1,
                    Some(b')') => {
                        depth = depth.saturating_sub(1);
                        if depth == 0 {
                            wraps = pos == end.saturating_sub(1);
                            break;
                        }
                    }
                    _ => {}
                }
                pos += 1;
            }
            if !wraps {
                return range;
            }
            range = SourceRange {
                offset: start.saturating_add(1),
                length: end.saturating_sub(start.saturating_add(2)),
            };
            while range.length > 0
                && self
                    .byte_at(range.offset)
                    .is_some_and(|b| b.is_ascii_whitespace())
            {
                range.offset = range.offset.saturating_add(1);
                range.length = range.length.saturating_sub(1);
            }
            while range.length > 0
                && self
                    .byte_at(range.end().saturating_sub(1))
                    .is_some_and(|b| b.is_ascii_whitespace())
            {
                range.length = range.length.saturating_sub(1);
            }
            if range.length == 0 {
                return range;
            }
        }
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
        // The composite-fold recompute (#3980): drained once `read_types`
        // filled (before `resolve_rows`, so the inferred-local re-bind
        // #4265 reads the same answer); the staged-check `from` ladder
        // consumes the stored rewrites here.
        let composite_rewrites = std::mem::take(&mut self.composite_rewrites);
        // The statement-form operand faces (#2814) join the same
        // offset-sorted survivor tail — promotion-aware: an opaque
        // nullability-only promotion on the operand read silences the
        // face the declared type would report.
        self.stage_operand_faces(promotions, &mut survivors);
        self.stage_nullable_operator_faces(&mut survivors);
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
                    // The junction operand's staged extent (`a && !x`) can
                    // cover the read the flow's promotion fact anchors at
                    // (`promoted_within`, #4265): the exact-span asks run
                    // first, the containment rescue only on their miss.
                    conditions
                        .iter()
                        .rev()
                        .find(|(range, _)| range.offset == check.range.offset)
                        .map(|(_, ty)| *ty)
                        .or_else(|| promotions.promoted(check.range))
                        .or_else(|| promotions.promoted_within(check.range))
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
            // The composite-fold rewrite (#3980): a staged check anchored
            // on a binary/conditional node's span reads the recomputed
            // promoted result — the fold-time `read_type_at` consult
            // starved, so `check.from` carries the unpromoted face. The key
            // strips outer parens first: a parenthesized `(n + 1)` stages
            // wider than the `n + 1` node range but is the same value, while
            // a call wrapper `foo(n + 1)` strips nothing and correctly
            // misses (its type is the call's, never the argument's).
            // `composite_rewrites` is offset-sorted; the file tag was
            // applied at drain time. Skipped entirely when the unit recorded
            // no rewrites — the common case pays one emptiness check.
            let composite_ty = if composite_rewrites.is_empty() {
                None
            } else {
                let key = self.strip_outer_parens(check.range);
                composite_rewrites
                    .iter()
                    .find(|(range, _)| *range == key)
                    .map(|(_, ty)| *ty)
            };
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
                        .or_else(|| composite_ty)
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
            // The composite-fold rewrite (#3980) also owns the literal flag:
            // a staged check anchored on a recorded binary/conditional node
            // is that composite expression, never a primitive literal — yet
            // `check.literal` was captured from the fold row's `num`/`int`
            // face (`primitive_literal` keys on the row's declared type, not
            // the node kind), so a promoted `int` result would wrongly take
            // the int→double literal conversion and silence a real
            // `invalid_assignment`. `composite_ty.is_some()` is exactly "the
            // staged expression is a fold node", so it clears the flag.
            let literal = check.literal && composite_ty.is_none();
            let probe_kind = match check.kind {
                StagedKind::Assignment => "assign",
                StagedKind::Return { .. } => "return",
                StagedKind::Argument => "arg",
                _ => "",
            };
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
                        self.future_value_type(target, false)
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
                    let verdict =
                        self.return_verdict(from, self.fold_alias_ty(check.to), literal, is_async);
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
                StagedKind::Assignment => self.assignment_verdict(from, check.to, literal),
                _ => self.assignable(from, check.to, literal),
            };
            if !probe_kind.is_empty() {
            }
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
    /// (#2814, attribution re-derived in #3632): the for-in iterable
    /// (`uncheckedUseOfNullableValueAsIterator` — `for_resolver.dart`
    /// :71-74/:178-180, the verifier ask `.at(iterable)` fired for the
    /// plain, the bare-target, AND the pattern-header foreach alike), the
    /// spread operand (`uncheckedUseOfNullableValueInSpread` —
    /// resolver.dart:4188-4193 `.at(node.expression)`, gated on
    /// `!node.isNullAware`), the `yield*` operand
    /// (`uncheckedUseOfNullableValueInYieldEach` —
    /// yield_statement_resolver.dart:163-168, gated on `node.star !=
    /// null`), and the void-operand negation
    /// (`uncheckedMethodInvocationOfNullableValue` with the `unary-`/`~`
    /// member spelling — prefix_expression_resolver.dart:152-227 routes
    /// every user-definable operator through the type property resolver's
    /// nullable arm, which resolves the operand's receiver to the
    /// `unary-`/`~` METHOD and reports the method-invocation face at the
    /// operator token — the oracle's `The method 'unary-' …` rows).
    ///
    /// The parse-time pass has no window for these faces: the for-in
    /// iterable window closes inside `check_for_in_iterable`, whose
    /// nullable arm defers to "the nullable-dereference face" without
    /// staging one, and a spread's operand never arms a window at all. So
    /// the candidates re-derive here at the teardown join: a token-table
    /// walk recovers the windows the parser saw, and the operand's static
    /// type grounds the way upstream's `expression.typeOrThrow` does —
    /// the OPERAND EXPRESSION's own type, never a single-read attribution
    /// (the #2814 single-read discipline under-reported every compound
    /// iterable: a member chain's window held only its receiver's row, a
    /// pattern header's a whole head, a `void` iterable none at all). The
    /// grounding ladder (`operand_expression_ty`) asks the flow's
    /// `ReadType` row at the operand's last token first (the promoted
    /// read, the bare local — a promotion rides the row's non-nullable
    /// type and stays silent), then the pass-2 receiver-expression walk
    /// (`recv_expression_ty` — chains, call results through the
    /// completed-send swap, parenthesized groups), then the token before
    /// a window-closing `)` (the callee/member whose invocation answer
    /// IS the send's result). A `void` read grounds through the
    /// [`Engine::void_read_rows`] side table the shared join drops.
    /// `Ty::Unknown` grounds to nothing — the under-report posture the
    /// parse-time staging holds. `promotions` is the staged promoted-read
    /// table the verdict gate reads (a promotion the `ReadType` join
    /// cannot spell still proves the read non-null).
    fn stage_operand_faces(&self, promotions: &PromotionStaging, out: &mut Vec<Diagnostic>) {
        // The walk runs unconditionally: a candidate's operand type comes
        // from the grounding ladder (`operand_expression_ty`), whose
        // `recv_expression_ty` leg re-derives member chains, call results,
        // and groups — none of which appear in `read_types`/`void_read_rows`.
        // A file's read rows can hold only non-nullable bare reads while a
        // `for (var x in c.bad)` iterable still reports `List?` — the early
        // exit a row-table scan would give misses that face. The token scan
        // itself is cheap; the ladder only pays at a `for`/`...`/`yield`/
        // `-`/`~` hit.
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
                    let Some((ty, range)) = self.operand_expression_ty(start, end, close_idx)
                    else {
                        continue;
                    };
                    self.emit_operand_face_row(
                        UncheckedOperandFace::ForInIterator,
                        ty,
                        range,
                        promotions,
                        out,
                    );
                }
                // The null-aware spread (`...?`) is `!node.isNullAware`'s
                // true arm — the oracle stays silent there.
                dartq_scanner::TokenKind::PeriodPeriodPeriod => {
                    let Some((start, end, boundary_idx)) = self.spread_operand_window(i) else {
                        continue;
                    };
                    let Some((ty, range)) = self.operand_expression_ty(start, end, boundary_idx)
                    else {
                        continue;
                    };
                    self.emit_operand_face_row(
                        UncheckedOperandFace::Spread,
                        ty,
                        range,
                        promotions,
                        out,
                    );
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
                    let Some((ty, range)) = self.operand_expression_ty(start, end, boundary_idx)
                    else {
                        continue;
                    };
                    self.emit_operand_face_row(
                        UncheckedOperandFace::YieldEach,
                        ty,
                        range,
                        promotions,
                        out,
                    );
                }
                // The void operand of a prefix `-`/`~` (`-x;` over a `void`
                // parameter — `void_type_usage_test`'s `param_negation`
                // shape): the value-consuming unary operators resolve as
                // member invocations (`unary-`/`~`), and a `void` receiver
                // is `isPotentiallyNullable` — the method-invocation face
                // reports at the operator token. Grounded on `Ty::Void`
                // only: a nullable-TYPED operator operand carries a
                // resolved interface face the operator lanes already
                // verdict, and re-deciding it here would double the row.
                dartq_scanner::TokenKind::Minus | dartq_scanner::TokenKind::Tilde => {
                    let Some((ty, at)) = self.unary_void_operand_ty(i) else {
                        continue;
                    };
                    self.emit_operand_face_row(
                        UncheckedOperandFace::Operator(
                            if tokens[i].kind == dartq_scanner::TokenKind::Minus {
                                "unary-"
                            } else {
                                "~"
                            },
                        ),
                        ty,
                        at,
                        promotions,
                        out,
                    );
                }
                _ => {}
            }
        }
    }

    /// The operand expression's static type and the anchor the oracle
    /// reports at (`start .. end` — the window IS the expression, the
    /// `.at(expression)` span). The grounding ladder: the flow read row
    /// at the operand's last content token (a bare read's own span, a
    /// promoted property read's member row), then the receiver-expression
    /// walk at the same span (the member chain's record, the
    /// parenthesized group's close-paren row), then — for a window
    /// closing in a call's `)` — the callee/member token before the
    /// matching `(` (the completed-send swap answers the call's result).
    /// `None` grounds to nothing.
    fn operand_expression_ty(
        &self,
        start: u32,
        _end: u32,
        boundary_idx: usize,
    ) -> Option<(Ty, SourceRange)> {
        let tokens = self.tokens.as_slice();
        // The operand's last content token: recovery synthetics never
        // stand for the expression's own tail.
        let mut last = boundary_idx.checked_sub(1)?;
        while last > 0 && tokens[last].is_synthetic() && tokens[last].utf16_offset >= start {
            last -= 1;
        }
        if tokens[last].utf16_offset < start {
            return None;
        }
        // The operand's first content token: the `.at(expression)` anchor
        // trims the keyword-to-operand whitespace (`yield* s` reports `s`,
        // never ` s`), and the end trims the trailing gap to `last`'s own
        // end — the window's boundary token (`)`, `;`, `,`) is the
        // delimiter, never the expression's extent.
        let mut first = tokens
            .iter()
            .position(|t| t.utf16_offset >= start)
            .unwrap_or(last);
        while first < last && tokens[first].is_synthetic() {
            first += 1;
        }
        let last_span = self.token_utf16_span(last)?;
        let anchor = SourceRange {
            offset: tokens[first].utf16_offset,
            length: last_span.end().saturating_sub(tokens[first].utf16_offset),
        };
        // A ParenthesizedExpression operand types as its INNER expression
        // (`ParenthesizedExpression.staticType` is the child's), while the
        // oracle's `.at(expression)` anchor still covers the parens
        // (`for (var v in (x))` reports `(x)`). The grounding legs below
        // read the operand's LAST token, which for a group is the closing
        // `)` — a token no read row keys — so the group unwraps onto its
        // own inner extent here and repeats (`((x))` walks out one pair at
        // a time). The anchor stays the outer group.
        let mut grounded_last = last;
        loop {
            if tokens[first].kind != dartq_scanner::TokenKind::OpenParen
                || self.tokens.matching_end(first) != Some(grounded_last)
            {
                break;
            }
            let mut inner_first = first + 1;
            while inner_first < grounded_last && tokens[inner_first].is_synthetic() {
                inner_first += 1;
            }
            if inner_first >= grounded_last {
                break;
            }
            let mut inner_last = grounded_last - 1;
            while inner_last > inner_first && tokens[inner_last].is_synthetic() {
                inner_last -= 1;
            }
            first = inner_first;
            grounded_last = inner_last;
        }
        let last = grounded_last;
        let last_span = self.token_utf16_span(last)?;
        // 1. The flow row at the operand's last token — the bare read
        //    (`for (var x in l)`), the promoted property read's member
        //    row; a `void` read joins through the side table the shared
        //    join drops.
        let ty = self
            .read_type_at(last_span)
            .or_else(|| self.void_read_ty_at(last_span));
        if let Some(ty) = ty.filter(|ty| *ty != Ty::Unknown) {
            return Some((ty, anchor));
        }
        // 2. The receiver-expression walk at the last token: the member
        //    chain's own record (`c.bad` keys at the member token's end),
        //    the parenthesized group's close-paren row, the compound
        //    `??`/`?:` arms' re-derivation.
        if let Some(ty) = self
            .recv_expression_ty(last_span)
            .filter(|ty| *ty != Ty::Unknown)
        {
            return Some((ty, anchor));
        }
        // 3. A window closing in `)` that neither leg answered is a call:
        //    the token before the matching `(` — the callee read of `f()`
        //    (the completed-send swap answers the call's result) or the
        //    member of `a.b()` (the chain record's answer IS the send's
        //    result). A cascade section inside the window (`xs..m()`) is
        //    NOT a plain send — the cascade's static type is the TARGET's
        //    type (cascade_expression_resolver.dart), never the section's
        //    result, so the callee answer would spell `m`'s return over a
        //    `List<int>` iterable and report a face the oracle keeps
        //    silent. The under-report posture stands there.
        if tokens[last].kind == dartq_scanner::TokenKind::CloseParen {
            // A cascade marker at the operand's own depth (nested `..`
            // inside a group is the group's own — `f(a..b)` stays a send).
            let mut head = tokens
                .iter()
                .position(|t| t.utf16_offset >= start)
                .unwrap_or(last);
            let mut cascade = false;
            while head < last {
                if let Some(close) = self.tokens.matching_end(head) {
                    head = close + 1;
                    continue;
                }
                if matches!(
                    tokens[head].kind,
                    dartq_scanner::TokenKind::PeriodPeriod
                        | dartq_scanner::TokenKind::QuestionPeriodPeriod
                ) {
                    cascade = true;
                    break;
                }
                head += 1;
            }
            if cascade {
                return None;
            }
            let mut depth = 0usize;
            let mut at = last;
            while at > 0 {
                at -= 1;
                match tokens[at].kind {
                    dartq_scanner::TokenKind::CloseParen
                    | dartq_scanner::TokenKind::CloseSquare
                    | dartq_scanner::TokenKind::CloseCurly => depth += 1,
                    dartq_scanner::TokenKind::OpenParen
                    | dartq_scanner::TokenKind::OpenSquare
                    | dartq_scanner::TokenKind::OpenCurly => {
                        if depth == 0 {
                            break;
                        }
                        depth -= 1;
                    }
                    _ => {}
                }
            }
            if depth == 0
                && tokens[at].kind == dartq_scanner::TokenKind::OpenParen
                && tokens[at].utf16_offset >= start
            {
                // The token before the `(`, skipping recovery synthetics.
                let mut callee = at;
                while callee > 0 {
                    callee -= 1;
                    if !tokens[callee].is_synthetic() && tokens[callee].utf16_offset >= start {
                        break;
                    }
                }
                if tokens[callee].utf16_offset >= start
                    && let Some(ty) = self
                        .recv_expression_ty(self.token_utf16_span(callee)?)
                        .filter(|ty| *ty != Ty::Unknown)
                {
                    return Some((ty, anchor));
                }
            }
        }
        None
    }

    /// The prefix `-`/`~` operand's `void` grounding (`unary_void`): the
    /// operand starts at the token after the operator, and only the shapes
    /// whose whole extent the ask covers ground — a bare read (`-x`),
    /// a parenthesized group (`-(x)`), a call whose close the operand ends
    /// (`-f()`). A member/index/increment continuation after the grounding
    /// token (`-a.b`, `-x[0]`, `-x++`) names a composite the single-token
    /// ask cannot spell — the under-report posture stands.
    fn unary_void_operand_ty(&self, op_idx: usize) -> Option<(Ty, SourceRange)> {
        let tokens = self.tokens.as_slice();
        // Prefix position: the token before the operator does not end an
        // operand (a `y - x`'s minus reads binary). The lexer's own
        // classification the increment faces hold (`a -b` after an
        // operand token is binary) — the operand-ending spellings only.
        let prior_text = if op_idx == 0 {
            None
        } else {
            Some(&self.source[tokens[op_idx - 1].byte_range()])
        };
        let unary = prior_text.is_none_or(|text| {
            matches!(
                text,
                "(" | "["
                    | "{"
                    | ","
                    | "="
                    | "=="
                    | "!="
                    | "=>"
                    | "return"
                    | "case"
                    | ";"
                    | ":"
                    | "?"
                    | "&&"
                    | "||"
                    | "??"
                    | "??="
            )
        });
        if !unary {
            return None;
        }
        let mut head = op_idx + 1;
        while head < tokens.len() && tokens[head].is_synthetic() {
            head += 1;
        }
        let head_tok = tokens.get(head)?;
        let op_span = self.token_utf16_span(op_idx)?;
        // A parenthesized head: the group face answers at the matching
        // close's span — the group's static type is its inner expression's
        // own (`ParenthesizedExpression.staticType`), so the inner extent
        // grounds it. A `void` read inside (`-(x)`) never reaches the
        // re-derived face: `void` rows live in the `void_read_rows` side
        // table the shared join drops, and `recv_expression_ty`'s value
        // legs do not consult it — the inner read's own void face must be
        // asked directly (`void_read_ty_at`), with a completed send inside
        // (`-(f())`) grounding through its result row.
        if head_tok.kind == dartq_scanner::TokenKind::OpenParen {
            let close = self.tokens.matching_end(head)?;
            // The inner expression's head token (recovery synthetics never
            // stand for the expression's own head).
            let mut inner = head + 1;
            while inner < close && tokens[inner].is_synthetic() {
                inner += 1;
            }
            if inner >= close {
                return None;
            }
            let inner_span = self.token_utf16_span(inner)?;
            let ty = self
                .recv_expression_ty(self.token_utf16_span(close)?)
                .filter(|ty| *ty == Ty::Void)
                .or_else(|| {
                    self.void_read_ty_at(inner_span)
                        .filter(|ty| *ty == Ty::Void)
                });
            return ty.map(|ty| (ty, op_span));
        }
        // A callee head (`-f()`): the call's RESULT grounds — the send row
        // keyed at the callee token carries the invoked signature's return
        // (`operand_static_ty` answers `Void` for a `void f()` formal's
        // send just as for a `void f()` declaration's; reading the callee's
        // own face spells `void Function()` and grounds nothing). The
        // operand must end at the call's own close (a `.`/`(`/`[`/`!`
        // continuation names a composite).
        let next = head + 1;
        if tokens.get(next).map(|t| t.kind) == Some(dartq_scanner::TokenKind::OpenParen)
            && let Some(close) = self.tokens.matching_end(next)
        {
            let cont = tokens.get(close + 1).map(|t| t.kind);
            if !matches!(
                cont,
                Some(
                    dartq_scanner::TokenKind::Period
                        | dartq_scanner::TokenKind::QuestionPeriod
                        | dartq_scanner::TokenKind::Bang
                        | dartq_scanner::TokenKind::OpenParen
                        | dartq_scanner::TokenKind::OpenSquare
                )
            ) {
                let head_span = self.token_utf16_span(head)?;
                // The completed send's RESULT face: a callable-typed callee
                // contributes its signature's return, never its own type
                // (`f()` on a `void f()` formal consumes `void`, upstream
                // types the `FunctionExpressionInvocation` by
                // `staticType.returnType`). The send rows are drained by
                // teardown, so the grounding re-derives the result the same
                // places `operand_static_ty` does: the value legs' face
                // through the completed-send swap (declared heads), then —
                // for a callable-typed formal/local whose face spells
                // `Unknown` — the armed signature's return.
                let ty = self
                    .recv_expression_ty(head_span)
                    .filter(|ty| *ty == Ty::Void)
                    .or_else(|| {
                        // The completed-send gate (`call_ends` keys the
                        // callee offset): the token is an invoked callee,
                        // so a callable-typed spelling contributes its
                        // signature's return — `operand_static_ty`'s
                        // `fn_var_sigs` arm answers `Void` for `void f()`
                        // invoked `f()`. The frames are drained by
                        // teardown, so the in-scope gate `operand_static_ty`
                        // applies mid-analysis cannot re-decide here; the
                        // `call_ends` key is the structural record that the
                        // callee stood and completed a send at this span.
                        let text = &self.source[tokens[head].byte_range()];
                        let name = self.intern(text);
                        let invoked = self
                            .call_ends
                            .binary_search_by_key(&head_span.offset, |(offset, _)| *offset)
                            .is_ok();
                        if !invoked {
                            return None;
                        }
                        self.fn_var_sigs
                            .get(&name)
                            .map(|sig| sig.return_ty)
                            .filter(|ty| *ty == Ty::Void)
                    });
                if let Some(ty) = ty {
                    return Some((ty, op_span));
                }
            }
            return None;
        }
        // A bare read head: the flow row (or the void side table) at the
        // head token, when the operand ends there (no continuation). The
        // resolved-send lane already owns this shape — `-x`'s operand row
        // stages the `unary-` verdict on `x`'s own offset
        // (row_resolution.dart's adjacent-operator arm re-emits it through
        // [`Engine::emit_unchecked_nullable`] at teardown) — and the
        // teardown walk must not double the row. The shapes the send never
        // resolves (the `-(x)` group, the `-f()` send-close) carry no
        // staged check on their head and still ground here.
        if !matches!(
            tokens.get(next).map(|t| t.kind),
            Some(
                dartq_scanner::TokenKind::Period
                    | dartq_scanner::TokenKind::QuestionPeriod
                    | dartq_scanner::TokenKind::Bang
                    | dartq_scanner::TokenKind::OpenParen
                    | dartq_scanner::TokenKind::OpenSquare
                    | dartq_scanner::TokenKind::MinusMinus
                    | dartq_scanner::TokenKind::PlusPlus
            )
        ) {
            let head_span = self.token_utf16_span(head)?;
            if self
                .staged_checks
                .iter()
                .any(|check| check.range.offset == head_span.offset)
            {
                return None;
            }
            let ty = self
                .read_type_at(head_span)
                .or_else(|| self.void_read_ty_at(head_span))
                .filter(|ty| *ty == Ty::Void);
            return ty.map(|ty| (ty, op_span));
        }
        None
    }

    /// The `void`-read side table's row at one span (the shared join
    /// drops `void` rows — the operand faces ask here).
    fn void_read_ty_at(&self, span: SourceRange) -> Option<Ty> {
        self.void_read_rows
            .iter()
            .any(|(row, _)| *row == span)
            .then_some(Ty::Void)
    }

    /// A token's UTF-16 span (the engine's external position convention).
    fn token_utf16_span(&self, idx: usize) -> Option<SourceRange> {
        let token = self.tokens.get(idx)?;
        let end = self.utf16_of(token.byte_offset as usize + token.byte_len as usize);
        Some(SourceRange {
            offset: token.utf16_offset,
            length: end.saturating_sub(token.utf16_offset),
        })
    }

    /// One candidate's verdict: the operand's flow-aware static type (the
    /// read row's own payload) decides. `Ty::Unknown` is the opaque-flow
    /// marker (the lattice could not spell the type) — the nullable
    /// posture of the row still decides, matching the parse-time arms'
    /// treatment of the marker. A `Null`-typed operand reports
    /// `invalid_use_of_null_value` instead — the verifier's `report()`
    /// swaps the code when `receiverType == typeProvider.nullType`
    /// (`nullable_dereference_verifier.dart:47-49`, reached through
    /// `for_resolver.dart:88`/`resolver.dart:4189`). `promotions` is the
    /// staged promoted-read table: a bare operand read whose promotion
    /// the `ReadType` join could not spell (the signature-less function
    /// head's `Ty::Unknown` marker — `staged_promotion_typed`) still
    /// proves non-null, and the face must not fire on the declared type's
    /// `?` the recv-walk grounding re-derives.
    fn emit_operand_face_row(
        &self,
        face: UncheckedOperandFace,
        ty: Ty,
        range: SourceRange,
        promotions: &PromotionStaging,
        out: &mut Vec<Diagnostic>,
    ) {
        // A dead window's operand never fires the verifier's ask — the
        // `ErrorVerifier` walks the dead node but the non-`dead_code`
        // checks suppress inside it (the same short-circuit
        // [`Engine::emit_unchecked_nullable`] holds). The receiver-
        // expression walk re-decides dead sends, so the gate belongs here
        // at emit, not at grounding.
        if self.in_dead_window(range) {
            return;
        }
        // The opaque-promotion gate: a nullability-only promotion stages
        // `Ty::Unknown` (the lattice spells no type for the promoted
        // face), and the oracle's `expression.typeOrThrow` there is the
        // non-null face — `_check`'s `InvalidType` arm silences the row
        // either way. Only a bare read keys a promotion row (a compound
        // operand's `.at(expression)` span matches no staged offset), and
        // a spelled promotion already grounded the same non-null type
        // through `read_type_at` — the marker is the one answer the
        // ladder cannot carry.
        if promotions.promoted(range) == Some(Ty::UNKNOWN) {
            return;
        }
        if !self.operand_face_ty_nullable(ty) {
            return;
        }
        // No resolved-lane dedup here — and none was possible: this
        // teardown walk runs inside `filter_staged`, after `resolve_rows`
        // `mem::take`s `self.report` (diagnose.rs's `let mut report`),
        // so a `report.diagnostics` probe would always read an empty
        // list. The overlap that probe was meant to suppress is already
        // covered upstream of this emit: the only shape the resolved
        // lane reports at the `-`/`~` token that `unary_void_operand_ty`
        // could also spell — the bare `-x`/`~x` read, whose
        // row_resolution adjacent arm emits the `unary-`/`~` method face —
        // is bailed by that walk's own `staged_checks` probe on the
        // operand's offset (the parse-time `NeverReceiver` staging sits
        // there for every prefix operand). The group (`-(x)`) and callee
        // (`-f()`) groundings never collide: the resolved lane's
        // `Void`/`nullable` adjacent verdict fires on the operand row's
        // own type, which those token-level shapes cannot re-derive — a
        // declared-function callee keeps its `void Function()` face, and
        // the group close carries no `Void` read.
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
                message,
                self.file,
                range,
                Some(correction),
            )
            .with_context(self.non_promotion_context_at(range.offset)),
        );
    }

    /// The `isPotentiallyNullable` posture of an operand's static type
    /// (`isPotentiallyNullable = !isNonNullable`, upstream Dart SDK source,
    /// pkg/analyzer/lib/src/dart/element/type_system.dart:1161-1184):
    /// nullable named/record/function shapes, `void`, and `Null` report
    /// (`isPotentiallyNullable`'s `type is VoidType` arm — the
    /// `forIn_iterable_typeVoid` oracle rows carry this code — and the
    /// `NullType` arm, whose row the verifier's `report()` swaps to
    /// `invalid_use_of_null_value`); `dynamic` is excluded upstream
    /// (`NullableDereferenceVerifier._check`'s `DynamicType` arm), and
    /// `Unknown`/`Never` are not potentially nullable. A `?`-less spelling
    /// still answers potentially nullable through `isNonNullable`'s
    /// recursions: a type parameter's bound (`T extends List?`, the
    /// implicit `Object?` on an unbounded `T` — [`Engine::potentially_nullable`]
    /// walks the declared-bound table), a `FutureOr` of a nullable
    /// argument, and an extension type declaring no interfaces
    /// (`isNonNullable` answers `type.interfaces.isNotEmpty`).
    fn operand_face_ty_nullable(&self, ty: Ty) -> bool {
        match ty {
            Ty::Void | Ty::Null => true,
            Ty::Named {
                name,
                nullable: false,
                ..
            } => {
                self.potentially_nullable(ty)
                    || self.future_or_potentially_nullable(ty)
                    || self.extension_type_operand_nullable(name)
            }
            _ => self.potentially_nullable(ty),
        }
    }

    /// The extension-type arm of `isNonNullable` for a `?`-less named
    /// operand (`type.element is ExtensionTypeElement` →
    /// `type.interfaces.isNotEmpty`, type_system.dart:1176-1178): an
    /// extension type whose header declares no `implements` clause is
    /// potentially nullable — its representation's nullability does not
    /// cover the spelling — while one claiming an interface is strictly
    /// non-nullable. A type-parameter head never reaches here (its bound
    /// decides in [`Engine::potentially_nullable`], upstream's
    /// `TypeParameterType` arm) — the captured-bound check keeps a
    /// parameter spelling that happens to share a container's name on the
    /// bound's verdict, and a non-extension-type or unresolved head
    /// answers false (the plain interface face).
    fn extension_type_operand_nullable(&self, name: StringId) -> bool {
        if self.param_bound_of(name).is_some() {
            return false;
        }
        resolve_container(
            self.graph,
            self.namespaces,
            self.index,
            self.lib,
            self.file,
            name,
        )
        .is_some_and(|(decl, _)| {
            decl.kind == ContainerKind::ExtensionType && decl.implements_clause.is_empty()
        })
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
            if tokens[i].kind != dartq_scanner::TokenKind::Keyword(dartq_scanner::Keyword::For) {
                continue;
            }
            // The `(` opens immediately after `for` (`await for` parks the
            // `await` before the `for`, not between `for` and `(`). A
            // non-paren token here is a `for`-element keyword or recovery —
            // never a loop-variable head.
            let open_idx = i + 1;
            if tokens
                .get(open_idx)
                .is_none_or(|t| t.is_synthetic() || t.kind != dartq_scanner::TokenKind::OpenParen)
            {
                continue;
            }
            // The head's `in` keyword before the paren's close — the loop
            // variable region is `( .. in`. `await` before `(` is fine.
            let Some(close_idx) = self.tokens.matching_end(open_idx) else {
                continue;
            };
            let Some(in_rel) = tokens[open_idx + 1..close_idx].iter().position(|t| {
                t.kind == dartq_scanner::TokenKind::Keyword(dartq_scanner::Keyword::In)
            }) else {
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
            if tokens[i].kind != dartq_scanner::TokenKind::Keyword(dartq_scanner::Keyword::When) {
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

    // —— record field names (`invalid_field_name`) ————————————————————

    /// The `Object`-member collision set upstream forbids for record field
    /// names (`isForbiddenNameForRecordField`, upstream Dart SDK source
    /// pkg/analyzer/lib/src/dart/resolver/record_literal_resolver.dart
    /// :208-212 — the records feature specification's member-name clause).
    pub(crate) fn is_forbidden_record_field_name(name: &str) -> bool {
        matches!(
            name,
            "hashCode" | "runtimeType" | "noSuchMethod" | "toString"
        )
    }

    /// Upstream `RecordTypeExtension.positionalFieldIndex`
    /// (pkg/analyzer/lib/src/dart/element/extensions.dart:210-220): a `$`
    /// followed by `[1-9]\d*` parses to the zero-based positional index
    /// (`$1` → 0); anything else — including `$0` and leading zeros like
    /// `$01` — is not a positional name. `u32` overflow returns `None`,
    /// the same `int.tryParse` bail the oracle takes.
    pub(crate) fn positional_field_index(name: &str) -> Option<u32> {
        let digits = name.strip_prefix('$')?;
        if digits.is_empty()
            || digits.starts_with('0')
            || !digits.bytes().all(|b| b.is_ascii_digit())
        {
            return None;
        }
        digits.parse::<u32>().ok().map(|pos| pos - 1)
    }

    /// One record field-name check (`invalid_field_name`): the shared
    /// decision order of upstream `reportInvalidFieldNames` /
    /// `_reportInvalidFieldNames` — `_`-private first, then the `$n`
    /// positional-collision arm, then the `Object`-member arm. Emits at the
    /// name token's own span, `&mut self` so the call sites stay walk-time.
    ///
    /// `positional_index` is the field's own index among the record's
    /// positional fields (record type annotations only — the oracle's
    /// `positionalFields.indexOf(field)`); `None` for a named-region field
    /// and for every record-literal field, where the upstream check is
    /// position-blind. `positional_count` is the record's positional field
    /// count. `wildcard_positional` exempts exactly the record-type
    /// positional `_` (upstream `isPositionalWildCard` — a record literal's
    /// named `_` never exempts).
    pub(crate) fn check_record_field_name(
        &mut self,
        name: &str,
        range: SourceRange,
        positional_index: Option<u32>,
        positional_count: u32,
        wildcard_positional: bool,
    ) {
        if name.starts_with('_') {
            // Positional record-type fields named `_` are legal under the
            // wildcard-variables feature; every other `_` field reports.
            if !wildcard_positional {
                self.emit(SemanticCode::InvalidFieldNamePrivate, range, "", "");
            }
            return;
        }
        if let Some(index) = Self::positional_field_index(name) {
            // The type-annotation arm also exempts the positional field's
            // own `$self` name (`positionalFields.indexOf(field) != index`
            // — `(int $1)`'s first field keeps `$1`; a literal's named
            // `$1` has no self exemption — `positional_index` is `None`).
            if index < positional_count && positional_index != Some(index) {
                self.emit(SemanticCode::InvalidFieldNamePositional, range, "", "");
            }
            return;
        }
        if Self::is_forbidden_record_field_name(name) {
            self.emit(SemanticCode::InvalidFieldNameFromObject, range, "", "");
        }
    }

    /// `end_record_type`'s field-name walk
    /// (`reportInvalidFieldNames`): fires at `var_end_record_type`, while
    /// the closing [`RecordTyLevel`] still heads
    /// [`Engine::record_ty_stack`] — its `names` pairs one written label
    /// (or `None`) with every field the level folded. The listener event
    /// carries the record's `(` but hands it only to the flow layer, so
    /// the walk re-derives the token window: the record's `(` is the
    /// innermost still-open paren at the last field type's completion
    /// span (`armed_ty_span`), verified by splitting the candidate window
    /// on its field commas and matching `level.fields.len()` entries —
    /// then each `Some` name's entry ends on its name token.
    pub(crate) fn check_record_type_field_names(&mut self) {
        use dartq_scanner::TokenKind;
        let Some(level) = self.record_ty_stack.last() else {
            return;
        };
        if level.fields.is_empty() || !level.names.iter().any(Option::is_some) {
            return;
        }
        let named_start = level.fields.len().saturating_sub(level.named_count);
        let positional_count = named_start as u32;
        let names: Vec<Option<StringId>> = level.names.clone();
        let Some(pos) = self.armed_ty_span.map(|span| span.end()) else {
            return;
        };
        let toks = self.tokens.as_slice();
        // The closing record's `(`: walk back from the inside position and
        // take the first `(` whose window carries exactly `fields.len()`
        // entries (a stale span inside an already-closed nested record
        // misses the count and the walk steps out to the real one).
        let mut at = toks.partition_point(|t| t.utf16_offset < pos);
        let mut depth = 0u32;
        let mut open = None;
        while at > 0 {
            at -= 1;
            match toks[at].kind {
                k if k.matching_opens().first().is_some() => depth += 1,
                k if k.is_begin_group() => {
                    if depth == 0 {
                        if k == TokenKind::OpenParen
                            && self.record_type_entries(at).is_some_and(|entries| {
                                entries.len() == level.fields.len()
                                    && Self::entries_match_names(toks, &entries, &names, self)
                            })
                        {
                            open = Some(at);
                            break;
                        }
                    } else {
                        depth -= 1;
                    }
                }
                _ => {}
            }
        }
        let Some(open) = open else { return };
        let Some(entries) = self.record_type_entries(open) else {
            return;
        };
        for (index, entry) in entries.iter().enumerate() {
            let Some(name_id) = names.get(index).copied().flatten() else {
                continue;
            };
            let name = self.strings.resolve(name_id).to_string();
            let Some(&last) = entry.last() else { continue };
            let tok = toks[last];
            let range = SourceRange {
                offset: tok.utf16_offset,
                length: self
                    .utf16_of((tok.byte_offset + tok.byte_len) as usize)
                    .saturating_sub(tok.utf16_offset),
            };
            // The oracle's positional-`_` wildcard exemption fires only on
            // a positional field (`(int _)`), never a `{…}`-region one.
            let positional_index = (index < named_start).then_some(index as u32);
            self.check_record_field_name(
                &name,
                range,
                positional_index,
                positional_count,
                positional_index.is_some() && self.is_wildcard(name_id),
            );
        }
    }

    /// The record type window's field entries: `open` indexes the `(` —
    /// returns each field's token indices (empty entries ride as empty
    /// vecs, the `(int, ,)` recovery's hole). Top-level commas split the
    /// positional region; the `{…}` named region's own commas split its
    /// entries. Nested groups are skipped whole through `matching_end`.
    fn record_type_entries(&self, open: usize) -> Option<Vec<Vec<usize>>> {
        use dartq_scanner::TokenKind;
        let toks = self.tokens.as_slice();
        let close = self.tokens.matching_end(open)?;
        let mut entries: Vec<Vec<usize>> = Vec::new();
        let mut entry: Vec<usize> = Vec::new();
        let mut i = open + 1;
        let mut in_named = false;
        while i < close {
            match toks[i].kind {
                TokenKind::Comma => {
                    entries.push(std::mem::take(&mut entry));
                }
                TokenKind::OpenCurly if !in_named => {
                    in_named = true;
                }
                k if k.is_begin_group() => {
                    i = self.tokens.matching_end(i)?;
                    entry.push(i);
                }
                TokenKind::CloseCurly if in_named => {
                    in_named = false;
                }
                _ => entry.push(i),
            }
            i += 1;
        }
        entries.push(entry);
        // A trailing comma leaves an empty tail entry — not a field.
        if entries.last().is_some_and(Vec::is_empty) {
            entries.pop();
        }
        Some(entries)
    }

    /// The `(` candidate's names check: every `Some` name must end its
    /// entry on an identifier spelling it — the guard that separates the
    /// record's own `(` from a same-arity enclosing group
    /// (`entries_match_names` is the second verification leg of
    /// [`Self::check_record_type_field_names`]'s backward walk).
    fn entries_match_names(
        toks: &[dartq_scanner::Token],
        entries: &[Vec<usize>],
        names: &[Option<StringId>],
        engine: &Self,
    ) -> bool {
        entries
            .iter()
            .zip(names.iter())
            .all(|(entry, name)| match (entry.last(), name) {
                (Some(&last), Some(id)) => {
                    &engine.source[toks[last].byte_range()] == engine.strings.resolve(*id)
                }
                (_, None) => true,
                (None, Some(_)) => false,
            })
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
        // The nameless recovery face's anchor (the official
        // `_reportDiagnosticForElement`, unused_local_elements_verifier.dart
        // :1204-1211): a nameless element reports at
        // `fragment.nameOffset ?? enclosingFragment?.nameOffset ?? 0` with
        // `length = fragment.name?.length ?? 0`. The fragment itself is
        // nameless (`nameOffset == null`), so the anchor is the ENCLOSING
        // executable fragment's `nameOffset`, and the length is 0. The
        // enclosing fragment is the innermost Executable frame — the popped
        // `_frame` when it is one (a function body's locals sit on its own
        // executable frame, `begin_block_function_body` opens no block), else
        // the nearest enclosing Executable past the transparent Block frames
        // (blocks are not fragments upstream). A nameless executable (an
        // anonymous closure, `FunctionExpressionFragmentImpl.nameOffset ==
        // null`) or one whose return never latched contributes no
        // `nameOffset` — the `?? 0` arm anchors it at the file start; the
        // walk stops there rather than climbing to an outer named frame.
        let nameless_anchor = std::iter::once(&_frame)
            .chain(self.frames.iter().rev())
            .find_map(|frame| match &frame.kind {
                FrameKind::Executable { ret, .. } => Some(match ret {
                    Some(ret) if !self.is_nameless(ret.name) => SourceRange {
                        offset: ret.name_span.offset,
                        length: 0,
                    },
                    _ => SourceRange {
                        offset: 0,
                        length: 0,
                    },
                }),
                _ => None,
            })
            .unwrap_or(SourceRange {
                offset: 0,
                length: 0,
            });
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
                *hit as usize == index && !guard_spans.iter().any(|(s, e)| *o >= *s && *o < *e)
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
            let nameless = raw_text.is_empty();
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
            // The nameless face anchors at the enclosing executable's
            // declared name (length 0), not at the recovered declaration's
            // own token range — the official `_reportDiagnosticForElement`'s
            // `enclosingFragment.nameOffset` arm. A named declaration keeps
            // its own range.
            let anchor = if nameless { nameless_anchor } else { range };
            self.stage_usage(code, anchor, &name);
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
        // A dot-joined `p.T` head names an element through an import
        // prefix — the official resolver's `PrefixedType` arm, which the
        // bare-scope walk below cannot see (the leaf is not in this
        // library's own scope). The prefix table answers instead: a
        // spelling that reaches a container is a real interface type, not
        // the `InvalidType` an unresolved head is.
        if self.strings.resolve(name).contains('.')
            && resolve_joined_container(
                self.graph,
                self.namespaces,
                self.index,
                self.lib,
                self.strings,
                name,
            )
            .is_some()
        {
            return false;
        }
        // A dot-joined `p.T` head names an element through an import
        // prefix — the official resolver's `PrefixedType` arm, which the
        // bare-scope walk below cannot see (the leaf is not in this
        // library's own scope). The prefix table answers instead: a
        // spelling that reaches a container is a real interface type, not
        // the `InvalidType` an unresolved head is.
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
                let (type_params, required, names, return_ty, positional, named) = {
                    let pool = self.boundary.pool();
                    let shape = pool.function_shape(shape);
                    (
                        shape.type_params,
                        shape.required_positional,
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
                // preserved. The shape's own `required_positional` split
                // rides too — the rebuild is a leaf substitution, never a
                // shape re-decision (the all-required producers would
                // re-intern a different identity).
                if names.is_empty() {
                    self.boundary.intern_function_ty_required(
                        type_params,
                        required,
                        return_ty,
                        &positional,
                        &named,
                        nullable,
                    )
                } else {
                    self.boundary.args.borrow_mut().function_generic_required(
                        &names,
                        required,
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
                // Two spellings of one leaf that resolve to different
                // declarations (`async.B` vs `await.B` — two prefixes
                // importing two `B`s) are distinct elements: the name-level
                // lattice below keys a dotted head by its leaf and would
                // read the pair as one head, proving what the oracle's
                // element-identity walk refuses. One declaration reached
                // through both spellings (`async.A` vs a bare `A` imported
                // from the same library) falls through unchanged.
                let (sub_text, sup_text) =
                    (self.strings.resolve(name), self.strings.resolve(sup_name));
                if (sub_text.contains('.') || sup_text.contains('.'))
                    && sub_text.rsplit('.').next() == sup_text.rsplit('.').next()
                {
                    let decl_file = |head: StringId| {
                        if self.strings.resolve(head).contains('.') {
                            resolve_joined_container(
                                self.graph,
                                self.namespaces,
                                self.index,
                                self.lib,
                                self.strings,
                                head,
                            )
                        } else {
                            resolve_container(
                                self.graph,
                                self.namespaces,
                                self.index,
                                self.lib,
                                self.file,
                                head,
                            )
                        }
                        .map(|(_, file)| file)
                    };
                    if let (Some(sub_file), Some(sup_file)) = (decl_file(name), decl_file(sup_name))
                        && sub_file != sup_file
                    {
                        return false;
                    }
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
            None => {
                // The head carries no element model in this unit, but a
                // resolved-but-unmodeled spelling still has a real face:
                // the ops adapter's opaque declaration with the written
                // arguments riding the Interface (#2288) — distinct
                // instantiations of one opaque head read "unrelated", the
                // covariant decision the name-level lattice cannot spell
                // (its unresolved-head optimism would fold every such
                // test to constant-true and dead a live else). A pair the
                // opaque map declines (a genuinely undefined head or
                // argument) keeps the name-level verdict below — the
                // oracle's InvalidType-argument vacuous arm (#3443).
                if let Some(ops) = self.ops
                    && let Some(answer) = {
                        let pool = self.boundary.args.borrow();
                        ops.try_subtype_with_opaque(sub, sup, &pool)
                    }
                {
                    return answer;
                }
                return self.name_subtype(sub, sup);
            }
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
        // Each argument answers against its ALIASED face: upstream
        // `isSubtypeOf` never sees a `TypeAliasElement` — a written
        // `C<void___int>` vs `C<void_>` asks `void Function([int]) <:
        // void Function()`, invisible under the typedef leaf. The fold
        // instantiates the alias to its declaration's argument frame.
        sub_list
            .iter()
            .zip(sup_list.iter())
            .all(|(&sub_arg, &sup_arg)| {
                self.strict_subtype(
                    self.fold_alias_ty(sub_arg),
                    self.fold_alias_ty(sup_arg),
                    depth + 1,
                )
            })
    }

    /// The source span of the token group `open` opened — `open` through its
    /// matching closer, resolved through the scanner's group table
    /// (`TokenTable::matching_end`) rather than through an enclosing frame's
    /// end token, which also covers a trailing `||`/`&&` operand or postfix
    /// (`case (a, b) || (c, d):`). `None` for an unclosed group (recovery) or
    /// a parser-inserted opener; the caller keeps its own fallback span.
    pub(crate) fn group_span(&self, open: Token) -> Option<SourceRange> {
        let close = self
            .tokens
            .get(self.tokens.matching_end(open.id as usize)?)?;
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
    pub(crate) fn collection_operand_ty(&self, operand: &Row) -> Option<Ty> {
        let (kind, open, close) = self.literal_window(operand)?;
        let bytes = self.source.as_bytes();
        let (list, set, map) = (
            self.collection_heads[0],
            self.collection_heads[1],
            self.collection_heads[2],
        );
        let dynamic = Ty::Dynamic;
        let (head, args) = match kind {
            // The empty `{}` literal's map default (`literal_return_ty`'s
            // map arm) — and the spread-only map-decided literal
            // (`{...m}`, the same marker): its spreads donate `Map<K, V>`
            // arguments to the per-slot folds, `dynamic` where none is
            // spellable. The same conventions the initializer face's
            // `collection_init_ty` runs for the slot.
            UntypedLiteral::EmptyBraces => {
                let blank = trim_value_ws(bytes, open + 1, close) >= close;
                if blank {
                    (map, self.boundary.intern_list(&[dynamic, dynamic]))
                } else {
                    let (key, value) = self.entry_leaf_tys(bytes, open + 1, close);
                    (map, self.boundary.intern_list(&[key, value]))
                }
            }
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
    /// superinterface below `Object`. Every other mix takes the shared
    /// superinterface walk [`Self::composite_lub`] runs (a user class pair
    /// joins at `Pattern`, distinct generic instantiations join pairwise);
    /// a function/record shape the walk cannot spell and a `void`,
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
                    // Unrelated heads join at their nearest shared
                    // superinterface — upstream `TypeSystem.leastUpperBound`'s
                    // depth-ranked walk (`String`/`RegExp` → `Pattern`,
                    // `least_upper_bound.dart`), which
                    // [`Self::composite_lub`] already implements for the
                    // composite folds. A pair whose only common supertype
                    // is `Object` joins there, exactly as the oracle's UP
                    // answers. A shape the walk cannot spell answers
                    // `UNKNOWN`, and the leaf face keeps its under-report.
                    let mut acc = forms[0];
                    for &next in &forms[1..] {
                        acc = self.composite_lub(acc, next, false);
                        if acc == Ty::UNKNOWN {
                            return None;
                        }
                    }
                    acc
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
    /// type, a typed nested literal's face), folded through the exact
    /// join ([`Self::exact_leaf_lub`]) — the same lattice the upward
    /// inference runs, so the direct operand answers the `Object` /
    /// `num` / nullable joins the oracle infers instead of a name-level
    /// under-report. A segment without an exact leaf row forces
    /// `dynamic`; a join the lattice cannot spell answers `dynamic` too
    /// (the is-face under-report direction).
    fn least_leaf_ty(&self, bytes: &[u8], start: usize, end: usize) -> Ty {
        let leaves: Vec<Option<Ty>> = value_comma_segments(bytes, start, end)
            .into_iter()
            .map(|(s, e)| self.leaf_row_ty(bytes, s, e))
            .collect();
        if leaves.iter().any(|leaf| leaf.is_none()) {
            return Ty::Dynamic;
        }
        self.exact_leaf_lub(&leaves.into_iter().flatten().collect::<Vec<_>>())
            .unwrap_or(Ty::Dynamic)
    }

    /// The map entry leaves' key/value least types: each segment's
    /// top-level colon splits the sides, a colonless map spread donates
    /// its `Map<K, V>` arguments to the per-slot folds
    /// ([`Self::spread_map_entry_tys`]), and any other colonless leaf
    /// (a control-flow element, a non-map spread) keeps both sides
    /// `dynamic`. The fold rides [`Self::exact_leaf_lub`], `dynamic`
    /// where a slot lost every leaf or the join is unspellable.
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
                    let s = trim_value_ws(bytes, s, e);
                    if bytes.get(s..).is_some_and(|rest| rest.starts_with(b"...")) {
                        match self.spread_map_entry_tys(bytes, s, e) {
                            Some((k, v)) => {
                                keys.push(Some(k));
                                values.push(Some(v));
                            }
                            None => {
                                keys.push(None);
                                values.push(None);
                            }
                        }
                    } else {
                        keys.push(None);
                        values.push(None);
                    }
                }
            }
        }
        let fold = |tys: Vec<Option<Ty>>| -> Ty {
            if tys.iter().any(|ty| ty.is_none()) {
                return Ty::Dynamic;
            }
            self.exact_leaf_lub(&tys.into_iter().flatten().collect::<Vec<_>>())
                .unwrap_or(Ty::Dynamic)
        };
        (fold(keys), fold(values))
    }

    /// The map spread's per-slot donation (`{...m}` inside a map-decided
    /// literal — `_computeElementType`'s map arm): the operand's
    /// `Map<K, V>` arguments are the key/value leaves it contributes,
    /// an operand whose head is an in-scope type parameter reads the
    /// parameter's declared bound first ([`Self::bound_face`] —
    /// `T extends Map<num, String>` donating `(num, String)`), and every
    /// other operand shape answers `None` (the map default's `dynamic`
    /// slot, never a guessed element). `start` is the trimmed segment
    /// start, `...`/`...?` prefix included.
    fn spread_map_entry_tys(&self, bytes: &[u8], start: usize, end: usize) -> Option<(Ty, Ty)> {
        let skip = 3 + usize::from(bytes.get(start + 3) == Some(&b'?'));
        let operand_start = trim_value_ws(bytes, start + skip, end);
        let ty = self.bound_face(self.leaf_row_ty(bytes, operand_start, end)?);
        let Ty::Named { name, args, .. } = ty else {
            return None;
        };
        if name != self.collection_heads[2] {
            return None;
        }
        let pool = self.boundary.pool();
        let slots = pool.get(args);
        let key = slots.first().copied().filter(|ty| *ty != Ty::UNKNOWN)?;
        let value = slots.get(1).copied().filter(|ty| *ty != Ty::UNKNOWN)?;
        Some((key, value))
    }

    /// The leaf's own row type: the push-ordered row the segment
    /// completed with (a [`Ty::Unknown`] or `dynamic` answer — an
    /// untyped read, an unresolvable send — is no exact face and answers
    /// `None`). Type-use rows are annotation positions, never leaves. The
    /// listener's `leaf_completion_row` scan at name level. A segment
    /// no row covers answers its own literal spelling
    /// ([`Self::literal_leaf_ty`]) — the primitive literals push no
    /// row.
    ///
    /// A control-flow `if` element joins its BRANCH leaves before any
    /// row scan (`typed_literal_resolver.dart` `_computeElementType`'s
    /// IfElementImpl arm — `leastUpperBound(thenType, elseType)`, a
    /// missing `else` keeping the then leaf): the keyword's word
    /// boundary and the parenthesized condition detect the element, the
    /// first top-level depth-aware `else` splits it, and each branch
    /// walks this same read (a nested `if` joins its own, a nested
    /// spread contributes its element). The branches join through
    /// [`Self::exact_leaf_lub`] — the same lattice the element fold
    /// runs — and a branch without an exact leaf answers no exact face
    /// (the oracle's join reads `dynamic` there).
    fn leaf_row_ty(&self, bytes: &[u8], start: usize, end: usize) -> Option<Ty> {
        let start = trim_value_ws(bytes, start, end.min(bytes.len()));
        let end = end.min(bytes.len());
        if start >= end {
            return None;
        }
        // `if` is a reserved word: the prefix is the element, never an
        // identifier's head, and the parenthesized condition follows.
        let after_if = trim_value_ws(bytes, start + 2, end);
        if bytes[start..end].starts_with(b"if")
            && !bytes.get(start + 2).is_some_and(|b| ident_byte(*b))
            && bytes.get(after_if) == Some(&b'(')
            && let Some(close) = matching_bracket_fwd(bytes, after_if)
        {
            let then_start = trim_value_ws(bytes, close + 1, end);
            return match value_top_level_else(bytes, close + 1, end) {
                Some(else_at) => {
                    let else_start = trim_value_ws(bytes, else_at + 4, end);
                    match (
                        self.leaf_row_ty(bytes, then_start, else_at),
                        self.leaf_row_ty(bytes, else_start, end),
                    ) {
                        (Some(then_ty), Some(else_ty)) => Some(
                            self.exact_leaf_lub(&[then_ty, else_ty])
                                .unwrap_or(Ty::Dynamic),
                        ),
                        // A branch without an exact leaf (a `dynamic` read)
                        // answers no exact face — the oracle's join reads
                        // `dynamic` there, the same face a `None` leaf
                        // forces through the literal fold.
                        _ => None,
                    }
                }
                None => self.leaf_row_ty(bytes, then_start, end),
            };
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
            .or_else(|| {
                self.deferred_miss_rows
                    .iter()
                    .rev()
                    .find(|row| in_span(row))
            })?;
        if spread {
            // The spread's element face: `Iterable`'s first argument where
            // the operand's head reaches it, `dynamic` where it does not
            // (a `Map`, a non-collection, an unspellable shape all read
            // `dynamic` upstream — `_computeElementType`'s else arm).
            return self.spread_element_ty(row);
        }
        // The leaf's face reads through the row's re-derived static type
        // ([`Self::operand_static_ty`] — a creation leaf's `Unknown` push
        // answers its created type, `new RegExp('.')` is `RegExp`, and a
        // send answers the callee's declared return). An `Unknown` or
        // `dynamic` answer stays no exact face (the under-report
        // direction) and falls to the literal spelling.
        let ty = self.operand_static_ty(row);
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
        let ty = args.function_generic_required(
            type_params,
            sig.required_positional,
            return_ty,
            &positional,
            &named,
            false,
        );
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
                        .own_exec_window(operand.name, false)
                        .or_else(|| self.own_exec_window(operand.name, true));
                    decl.type_param_names
                        .iter()
                        .copied()
                        .map(|name| {
                            let bound = bounds.and_then(|window| {
                                window
                                    .params
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
                // The declared required/optional split joins the face's
                // identity (`FnShape::required_positional`): interning the
                // all-required reading strands `m is Function(int, [..])`
                // against a source signature whose optional group the
                // arity gate reads as required — a decided miss where the
                // oracle's tear-off `staticType` proves the check.
                let required = formals
                    .positional
                    .iter()
                    .filter(|formal| formal.required)
                    .count() as u16;
                if entry.type_params == 0 {
                    return Some(self.boundary.intern_function_ty_required(
                        0,
                        required,
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
                Some(self.boundary.args.borrow_mut().function_generic_required(
                    &type_params,
                    required,
                    return_ty,
                    &positional,
                    &named,
                    false,
                ))
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
            // A member read spelled through the container NAME (`A.sm`) —
            // the static-member face: the read's static type is the
            // member's own value face (`Identifier.staticType` over a
            // static method is its tear-off signature — `A.foo!` reports
            // `unnecessary_non_null_assertion` over `void Function()`,
            // UnnecessaryNonNullAssertionTest.test_nonNull_method). The
            // constructor selectors (`C.new`) answered earlier through
            // [`Self::ctor_tear_off_ty`]; the raw face stands (a bare
            // container name carries no written instantiation).
            Receiver::Container { name, .. } => (*name, false),
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
        // The container-name arm is the STATIC face: an instance member
        // reached through `A.m` is the invalid-access shape whose typing
        // the error's own lane owns — only a static member's value face
        // answers here.
        if matches!(recv, Receiver::Container { .. }) && !entry.is_static {
            return None;
        }
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
                match resolve_container(
                    self.graph,
                    self.namespaces,
                    self.index,
                    self.lib,
                    self.file,
                    name,
                ) {
                    Some(hit) => hit,
                    // An unindexed SDK container (`List.filled`'s receiver)
                    // or an ungrounded alias head: the core-constructor
                    // table owns that face.
                    None => return self.core_ctor_face(row),
                },
                name,
            ),
            Receiver::PrefixedContainer { prefix, name, .. } => (
                match prefixed_container(
                    self.graph,
                    self.namespaces,
                    self.index,
                    self.lib,
                    prefix,
                    name,
                ) {
                    Some(hit) => hit,
                    None => return self.core_ctor_face(row),
                },
                name,
            ),
            // The grounding postures an unindexed receiver head leaves:
            // the listener grounds an alias whose right-hand head resolves
            // to no indexed container (`typedef IntList = List<int>`'s
            // receiver) to `Typed` carrying the right-hand type as
            // written (a generic alias keeps its unsubstituted `T`), and
            // a bare SDK head with a written group (`List<int>.filled`)
            // stays `Unknown` — no container ever arms for either, so
            // the source bytes' spelling (the fold's own start) are the
            // only face left: the core-constructor table's.
            Receiver::Typed { .. } | Receiver::Unknown => return self.core_ctor_face(row),
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
        // The declared required/optional split joins the face's identity
        // (`FnShape::required_positional`) — `C.new is C Function([int])`
        // over `C([int])` reads the arity gate's honest count.
        let required = formals
            .positional
            .iter()
            .filter(|formal| formal.required)
            .count() as u16;
        let ty = if sig_params.is_empty() {
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
                &sig_params,
                required,
                return_ty,
                &positional,
                &named,
                false,
            )
        };
        Some(ty)
    }

    /// The receiver head's written spelling at a constructor tear-off
    /// operand, with its written type-argument group when one closed on
    /// the head (`IntList` / `MyList` / `List` before the `.`). The
    /// merged row anchors the selector, so the head sits before the `.`:
    /// the walk recovers the SPELLING (the alias the source wrote — the
    /// row's own receiver may be a `Typed` grounding or `Unknown`, and a
    /// grounded expansion head drops its right-hand-side arguments)
    /// beside the group's arguments — a `call_type_args` entry or the
    /// #610 `completed_group` latch keyed at the head's offset first,
    /// the written bytes between the balanced brackets otherwise
    /// ([`Self::written_group_tys`] — an expression head's group keys
    /// neither latch, the #2917 hole). [`Self::receiver_group_args`]
    /// delegates here for the group answer alone.
    fn receiver_head_face(&self, row: &Row) -> Option<(StringId, Option<Vec<Ty>>)> {
        let is_ident = |b: u8| matches!(b, b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'_' | b'$');
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
        if bytes[p - 1] != b'>' {
            // The bare head: an identifier run ends at `p`.
            let mut ident = p;
            while ident > 0 && is_ident(bytes[ident - 1]) {
                ident -= 1;
            }
            if ident == p {
                return None;
            }
            let spelling = std::str::from_utf8(&bytes[ident..p]).ok()?;
            return Some((self.intern(spelling), None));
        }
        // `Head<...>` ends in `>`: walk back to the matching `<`, then to
        // the head ident's start — the group latches' key.
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
        while ident > 0 && is_ident(bytes[ident - 1]) {
            ident -= 1;
        }
        if ident == head_open {
            return None;
        }
        let head_offset = self.utf16_of(ident);
        let spelling = std::str::from_utf8(&bytes[ident..head_open]).ok()?;
        let head = self.intern(spelling);
        let group = if let Some((_, entry)) = self
            .call_type_args
            .iter()
            .find(|(offset, _)| *offset == head_offset)
        {
            Some(entry.tys.to_vec())
        } else if let Some((_, group_offset, args)) = &self.completed_group
            && *group_offset == head_offset
        {
            Some(args.clone())
        } else {
            // Neither latch holds an expression head's group (the #2917
            // hole: `call_type_args` keys only headless groups,
            // `completed_group` only declaration heads) — but the group
            // is written in the bytes the walk already bounds.
            self.written_group_tys(head_open, p - 1)
        };
        Some((head, group))
    }

    /// The written type-argument spellings between a balanced `<` (at
    /// `open`) and its `>` (at `close`): simple named forms alone — an
    /// identifier head, an optional nested group, an optional trailing
    /// `?` (`int`, `String?`, `List<int>`, `dynamic`). Anything else
    /// (function syntax, `void`, a dotted prefix, `dynamic?`) keeps the
    /// under-report: the family's groups are plain instantiations, and
    /// an out-of-claim spelling is the honest decline.
    fn written_group_tys(&self, open: usize, close: usize) -> Option<Vec<Ty>> {
        let is_ident = |b: u8| matches!(b, b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'_' | b'$');
        let bytes = self.source.as_bytes();
        let mut cursor = open + 1;
        let mut args = Vec::new();
        loop {
            while cursor < close && bytes[cursor].is_ascii_whitespace() {
                cursor += 1;
            }
            let start = cursor;
            while cursor < close && is_ident(bytes[cursor]) {
                cursor += 1;
            }
            if cursor == start {
                return None;
            }
            let head = std::str::from_utf8(&bytes[start..cursor]).ok()?;
            let mut nested: Vec<Ty> = Vec::new();
            if cursor < close && bytes[cursor] == b'<' {
                let mut depth = 0usize;
                let mut scan = cursor;
                while scan < close {
                    match bytes[scan] {
                        b'<' => depth += 1,
                        b'>' => {
                            depth -= 1;
                            if depth == 0 {
                                break;
                            }
                        }
                        _ => {}
                    }
                    scan += 1;
                }
                if scan >= close {
                    return None;
                }
                nested = self.written_group_tys(cursor, scan)?;
                cursor = scan + 1;
            }
            let nullable = cursor < close && bytes[cursor] == b'?';
            if nullable {
                cursor += 1;
            }
            if head == "dynamic" {
                if !nested.is_empty() || nullable {
                    return None;
                }
                args.push(Ty::Dynamic);
            } else {
                let arg_list = self.boundary.args.borrow_mut().intern(&nested);
                args.push(Ty::Named {
                    name: self.intern(head),
                    nullable,
                    args: arg_list,
                });
            }
            while cursor < close && bytes[cursor].is_ascii_whitespace() {
                cursor += 1;
            }
            if cursor == close {
                return Some(args);
            }
            if bytes[cursor] != b',' {
                return None;
            }
            cursor += 1;
        }
    }

    /// The core-constructor tear-off face — the SDK containers' factory
    /// selectors ([`crate::member_face::CORE_CTOR_FORMALS`], the
    /// reference SDK's own signature table) whose element index cannot
    /// answer: the SDK units are unindexed, so `resolve_container`
    /// chain-breaks at the receiver and the declared-constructor table
    /// never arms — every `List.filled` tear-off through a typedef
    /// (`typedef IntList = List<int>`'s `IntList.filled`, the
    /// co19 `tearing_off_from_typedef` family) stayed the under-report
    /// `None` before this face.
    ///
    /// The receiver's head comes from the source bytes
    /// ([`Self::receiver_head_face`]) — the row's receiver name may be a
    /// grounded alias head whose right-hand-side arguments the grounding
    /// dropped, and those arguments instantiate the tear-off
    /// (`IntList`'s `List<int>` reads `E` as `int`):
    ///
    /// * a written `<...>` group instantiates directly
    ///   (`List<int>.filled`) or substitutes a GENERIC alias's own
    ///   parameters (`typedef MyList<T> = List<T>` written
    ///   `MyList<int>.filled` — the right-hand `T` reads the written
    ///   `int`);
    /// * a bare NON-generic alias reads its right-hand side's arguments
    ///   (`IntList.filled` — `[int]`); a spelled group on a non-generic
    ///   alias is the wrong-type-argument-count face's own report, so
    ///   the fold declines it;
    /// * a bare SDK head with no alias (`List.filled`) is the generic
    ///   tear-off form — outside this face's claim, the under-report.
    ///
    /// A foreign alias (declared in another library) and a chain past
    /// the loop bound keep the same under-report. The tear-off's return
    /// is the container head instantiated with the receiver's arguments
    /// — the same face [`Self::ctor_selector_ty`] builds for an indexed
    /// container.
    fn core_ctor_face(&self, row: &Row) -> Option<Ty> {
        use crate::member_face::{CoreCtorArg, core_ctor_formals};
        let (mut head, written) = self.receiver_head_face(row)?;
        let mut elem: Option<Vec<Ty>> = written.clone();
        for _ in 0..4 {
            if let Some((positional, named)) =
                core_ctor_formals(self.strings.resolve(head), self.strings.resolve(row.name))
            {
                let elem = elem?;
                let ret_args = self.boundary.args.borrow_mut().intern(&elem);
                let return_ty = Ty::Named {
                    name: head,
                    nullable: false,
                    args: ret_args,
                };
                let positional: Vec<Ty> = positional
                    .iter()
                    .map(|arg| self.core_ctor_arg_ty(arg, &elem))
                    .collect();
                let named: Vec<(StringId, Ty)> = named
                    .iter()
                    .map(|(name, arg)| (self.intern(name), self.core_ctor_arg_ty(arg, &elem)))
                    .collect();
                // Every table row's positionals are required — no
                // optional-positional factory formals in the covered set.
                let required = positional.len() as u16;
                let ty = self.boundary.args.borrow_mut().function_required(
                    0,
                    required,
                    return_ty,
                    &positional,
                    &named,
                    false,
                );
                return Some(ty);
            }
            // The alias fold: this hop's head names a type alias whose
            // named right-hand side re-heads the walk (chained aliases
            // fold up to the bound).
            let decl = self.top_level_decl(head)?;
            if decl.kind != MicroKind::TypeAlias || decl.var_ty == Ty::UNKNOWN {
                return None;
            }
            let Ty::Named {
                name: next,
                nullable: false,
                args,
            } = decl.var_ty
            else {
                return None;
            };
            if decl.type_params == 0 {
                // A spelled group on a non-generic alias is the count
                // face's report, never an instantiation.
                if written.is_some() {
                    return None;
                }
                elem = Some({
                    let pool = self.boundary.pool();
                    pool.get(args).to_vec()
                });
            } else {
                // The alias's own parameters substitute by the written
                // group before the right-hand arguments read.
                let written = elem.as_ref()?;
                let subst: FxHashMap<StringId, Ty> = decl
                    .type_param_names
                    .iter()
                    .zip(written.iter().copied())
                    .map(|(name, ty)| (*name, ty))
                    .collect();
                let rhs = {
                    let pool = self.boundary.pool();
                    pool.get(args).to_vec()
                };
                elem = Some(
                    rhs.into_iter()
                        .map(|ty| self.subst_named_leaves(ty, &subst))
                        .collect(),
                );
            }
            head = next;
        }
        None
    }

    /// One [`crate::member_face::CoreCtorArg`] resolved against the
    /// receiver's element arguments.
    fn core_ctor_arg_ty(&self, arg: &crate::member_face::CoreCtorArg, elem: &[Ty]) -> Ty {
        use crate::member_face::CoreCtorArg;
        match arg {
            CoreCtorArg::Fixed(spelling) => self.core_spelling_ty(spelling),
            CoreCtorArg::Param(index) => elem
                .get(usize::from(*index))
                .copied()
                .unwrap_or(Ty::Dynamic),
            CoreCtorArg::FixedGeneric(head, spellings) => {
                let args: Vec<Ty> = spellings
                    .iter()
                    .map(|spelling| self.core_spelling_ty(spelling))
                    .collect();
                let args = self.boundary.args.borrow_mut().intern(&args);
                Ty::Named {
                    name: self.intern(head),
                    nullable: false,
                    args,
                }
            }
            CoreCtorArg::Params(head, count) => {
                let args: Vec<Ty> = (0..*count)
                    .map(|index| elem.get(usize::from(index)).copied().unwrap_or(Ty::Dynamic))
                    .collect();
                let args = self.boundary.args.borrow_mut().intern(&args);
                Ty::Named {
                    name: self.intern(head),
                    nullable: false,
                    args,
                }
            }
        }
    }

    /// One fixed spelling of a core-constructor formal: `dynamic`, or a
    /// named head with an optional `?` nullability suffix (`Object?`).
    fn core_spelling_ty(&self, spelling: &str) -> Ty {
        if spelling == "dynamic" {
            return Ty::Dynamic;
        }
        if let Some(head) = spelling.strip_suffix('?') {
            return Ty::Named {
                name: self.intern(head),
                nullable: true,
                args: ArgListId::EMPTY,
            };
        }
        Ty::Named {
            name: self.intern(spelling),
            nullable: false,
            args: ArgListId::EMPTY,
        }
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
                let (count, required, names, ret, pos, named) = {
                    let pool = self.boundary.pool();
                    let shape = pool.function_shape(shape);
                    (
                        shape.type_params,
                        shape.required_positional,
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
                // nullability — the interned face is the substitution; the
                // `required_positional` split is the shape's own.
                if names.is_empty() {
                    self.boundary
                        .args
                        .borrow_mut()
                        .function_required(count, required, ret, &pos, &named, nullable)
                } else {
                    self.boundary
                        .args
                        .borrow_mut()
                        .function_generic_required(&names, required, ret, &pos, &named, nullable)
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
            let ret = ret.map_or(Ty::Dynamic, |tree| self.tree_ty(tree));
            let positional: Vec<Ty> = positional.iter().map(|tree| self.tree_ty(tree)).collect();
            let named: Vec<(StringId, Ty)> = named
                .iter()
                .map(|(name, tree, _)| (*name, self.tree_ty(tree)))
                .collect();
            let type_params: Vec<(StringId, Option<Ty>)> = type_params
                .iter()
                .map(|(name, bound, _)| (*name, bound.as_ref().map(|b| self.tree_ty(b))))
                .collect();
            // The capture's honest required-positional count rides the
            // shape itself: `typedef TOpt = void Function([int])` spells
            // an optional positional formal, and a split-blind intern
            // would merge that face with the all-required
            // `Function(int)` id — `fn_face_sig`'s fallback then reads
            // `positional.len()` off the merged shape and the arity gate
            // (`_isFunctionSubtypeOf`'s required rule,
            // subtype.dart:363-369) answers the wrong side, proving
            // `req is TOpt` constant-true where the oracle stays silent.
            // The count also lands in `fn_shape_required` keyed by the
            // honest id: a `canonical_dotted_ty` leaf rewrite that
            // re-interns the face copies it across onto the new shape.
            // A capture without the count (a persisted pre-split
            // payload) keeps the all-required reading.
            let required = required_positional
                .map(|n| u16::try_from(n).unwrap_or(u16::MAX))
                .unwrap_or_else(|| u16::try_from(positional.len()).unwrap_or(u16::MAX));
            let ty = self.boundary.args.borrow_mut().function_generic_required(
                &type_params,
                required,
                ret,
                &positional,
                &named,
                nullable,
            );
            if let Ty::Function { shape, .. } = ty
                && shape != crate::model::FnShapeId::EMPTY
            {
                self.fn_shape_required.entry(shape).or_insert(required);
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
    /// `end_function_type`. The required/optional split is part of the
    /// shape identity ([`FnShape::required_positional`]), but a face
    /// re-interned by the count-blind leaf-rewrite path lands on the
    /// all-required id — [`Self::fn_face_sig`]'s fallback then reads that
    /// shape's `required_positional`, which answers `positional.len()` —
    /// a `core.List<int> Function(int, [int])` face would read required 2
    /// where the written signature said 1. The written count is a
    /// positional-LAYOUT fact the leaf rewrite preserves, so it copies
    /// across onto the new shape — the `last_fn_seal` pair first (the
    /// written site's own answer), then the map's record for the old id.
    fn canonical_function_ty(&mut self, ty: Ty) -> Ty {
        let before = match ty {
            Ty::Function { shape, .. } if shape != crate::model::FnShapeId::EMPTY => Some(shape),
            _ => None,
        };
        let rewritten = self.boundary.canonical_dotted_ty(ty);
        let (Some(before), Ty::Function { shape: after, .. }) = (before, rewritten) else {
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

    /// The operand's declared type as WRITTEN when the declaration spells
    /// it with an import prefix; the captured face otherwise.
    ///
    /// The official verdict reads `operand.staticType`, which spells the
    /// resolved element whatever prefix the annotation used. This engine's
    /// capture drops that prefix on two of the three written shapes — a
    /// group-less `p.T` annotation arms nothing the variable can read, and
    /// a parameterised one arms the leaf (`Named{T}` with the written
    /// arguments) — so the spelled-head comparison against the checked
    /// side's joined `p.T` reads two different types and the row stays
    /// silent. The annotation itself survives only in the source: re-read
    /// it backwards from the declaration's own name token and keep the
    /// recovered face when it is a `p.T` spelling whose prefix resolves.
    /// Every other declaration (a bare head, an inferred `var`, a dead
    /// prefix) keeps its captured face, so the recovery can only restore
    /// a prefix the capture dropped, never invent one.
    fn prefixed_operand_ty(&self, operand_ty: Ty) -> Ty {
        // A face that already spells a joined head kept its prefix (a
        // promoted read, an augmented capture): nothing was dropped, and
        // the declaration's own spelling must not downgrade it.
        if let Ty::Named { name, .. } = operand_ty
            && self.strings.resolve(name).contains('.')
        {
            return operand_ty;
        }
        let Some(offset) = self
            .operand_row()
            .and_then(|row| self.operand_decl_offset(row))
        else {
            return operand_ty;
        };
        let Some(recovered) = self
            .backward_ty_text(offset)
            .and_then(|text| self.parse_spelled_ty(&text))
        else {
            return operand_ty;
        };
        let Ty::Named { name, .. } = recovered else {
            return operand_ty;
        };
        let spelled = self.strings.resolve(name);
        let Some((prefix, _)) = spelled.split_once('.') else {
            return operand_ty;
        };
        if !matches!(self.resolve_bare(self.intern(prefix)).1, RefTarget::Prefix) {
            return operand_ty;
        }
        // The joined `p.T` collapses onto the bare element where this
        // file also reaches it (`canonical_head`'s same-element guard) —
        // the face the captured operand already spells in that
        // configuration.
        self.boundary.canonical_dotted_ty(recovered)
    }

    /// The name-token offset of the declaration the operand reads: the
    /// innermost live local or formal of the same spelling, the top-level
    /// variable's own declaration site, or the field of the receiver's
    /// container. Only declarations of THIS file qualify — the backward
    /// scan reads `self.source`, so a foreign file's offsets mean
    /// nothing here.
    fn operand_decl_offset(&self, row: &Row) -> Option<u32> {
        if let Some(decl) = self
            .locals
            .iter()
            .rev()
            .find(|decl| decl.name == row.name && !decl.retired)
        {
            return Some(decl.range.offset);
        }
        if let Some(site) = self
            .decl_sites
            .iter()
            .find(|site| site.name == row.name && matches!(site.kind, DeclKind::TopLevelVar))
        {
            return Some(site.range.offset);
        }
        let RowShape::Member { recv } = &row.shape else {
            return None;
        };
        let (container, owner) = match recv {
            Receiver::Container { name, .. } => resolve_container(
                self.graph,
                self.namespaces,
                self.index,
                self.lib,
                self.file,
                *name,
            )?,
            Receiver::Typed { type_name, .. } => resolve_container(
                self.graph,
                self.namespaces,
                self.index,
                self.lib,
                self.file,
                *type_name,
            )?,
            Receiver::PrefixedContainer { prefix, name, .. } => prefixed_container(
                self.graph,
                self.namespaces,
                self.index,
                self.lib,
                *prefix,
                *name,
            )?,
            _ => return None,
        };
        (owner == self.file)
            .then(|| container.members.get(&row.name))
            .flatten()
            .map(|entry| entry.name_range.offset)
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

    /// Whether `shape` is the parameterized function-RHS alias's armed-head
    /// stub: the anonymous signature's return head alone (`Unknown
    /// Function()` — no formals, no own type parameters). [`Self::alias_
    /// function_ty`]'s `var_ty` face and the `take_armed` capture both land
    /// this face when the typedef's RHS is a nested function type, and it
    /// carries no signature the `is` verdict could compare against.
    fn is_armed_head_stub(&self, shape: crate::model::FnShapeId) -> bool {
        if shape == crate::model::FnShapeId::EMPTY {
            return false;
        }
        let pool = self.boundary.pool();
        let shape = pool.function_shape(shape);
        shape.type_params == 0
            && shape.type_params_named.is_empty()
            && shape.positional.is_empty()
            && shape.named.is_empty()
            && shape.return_ty == Ty::Unknown
    }

    /// The bare alias fold's split repair: [`Self::fold_alias_ty`] answers
    /// a function-RHS alias with the RHS shape interned at capture, whose
    /// producer is split-blind — `typedef TOpt = void Function([int])`
    /// interns as the all-required `void Function(int)` id and shares it
    /// with every all-required spelling, so `req is TOpt` proves
    /// constant-true by pooled reflexivity where the oracle stays silent
    /// (`_isFunctionSubtypeOf`'s required rule, subtype.dart:363-369).
    /// The capture's honest required-positional count re-interns the face
    /// onto its own id (the #3511 identity contract: the two spellings are
    /// distinct types); a capture without the count, a non-alias target,
    /// or an already-honest face keeps the fold as written.
    fn repair_alias_fold_split(&mut self, armed: Ty, folded: Ty) -> Ty {
        let Ty::Named {
            name,
            args: ArgListId::EMPTY,
            ..
        } = armed
        else {
            return folded;
        };
        let Ty::Function { shape, nullable } = folded else {
            return folded;
        };
        if shape == crate::model::FnShapeId::EMPTY {
            return folded;
        }
        let Some((.., required_positional)) = self.bounds.alias_fn_shape(name) else {
            return folded;
        };
        let Some(required) = required_positional.and_then(|n| u16::try_from(n).ok()) else {
            return folded;
        };
        let (type_params_named, type_params, return_ty, positional, named, split_blind) = {
            let pool = self.boundary.pool();
            let s = pool.function_shape(shape);
            (
                s.type_params_named.to_vec(),
                s.type_params,
                s.return_ty,
                s.positional.to_vec(),
                s.named.to_vec(),
                s.required_positional != required,
            )
        };
        if !split_blind {
            return folded;
        }
        // A count-only shape keeps anonymous entries — the same rebuild
        // `fn_face_sig` answers for unwritten parameter spellings.
        let params: Vec<(StringId, Option<Ty>)> = if type_params_named.is_empty() {
            (0..usize::from(type_params))
                .map(|_| (StringId::NONE, None))
                .collect()
        } else {
            type_params_named
        };
        let repaired = self.boundary.args.borrow_mut().function_generic_required(
            &params,
            required,
            return_ty,
            &positional,
            &named,
            nullable,
        );
        if let Ty::Function { shape: honest, .. } = repaired
            && honest != crate::model::FnShapeId::EMPTY
        {
            // The count rides the map for the leaf-rewrite copies
            // (`canonical_function_ty` re-interns the face and reads it
            // back here).
            self.fn_shape_required.entry(honest).or_insert(required);
        }
        repaired
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
    fn parse_spelled_leaf(&self, text: &str, subst: &FxHashMap<StringId, Ty>) -> Option<Ty> {
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
                && !matches!(
                    bytes.get(index.wrapping_sub(1)),
                    Some(b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'_' | b'$')
                )
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
                                self.parse_formal_ty(piece, subst, false, decl)
                                    .map(|(_, ty)| ty)
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
        if let Some(eq) = Self::depth0_eq(head)
            && !head[..eq].trim_end().is_empty()
        {
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
    pub(crate) fn backward_type_text(&self, name_offset: u32) -> Option<String> {
        self.backward_type_text_at_byte(self.byte_of(name_offset))
    }

    /// [`Self::backward_type_text`] anchored at a byte index (the
    /// top-level occurrence scan works in bytes).
    fn backward_type_text_at_byte(&self, name_end: usize) -> Option<String> {
        const MODIFIERS: &[&str] = &[
            "static",
            "final",
            "const",
            "late",
            "var",
            "external",
            "covariant",
            "factory",
            "abstract",
        ];
        const KEYWORDS: &[&str] = &[
            "return",
            "throw",
            "new",
            "await",
            "yield",
            "assert",
            "else",
            "case",
            "in",
            "is",
            "as",
            "do",
            "on",
            "when",
            "get",
            "set",
            "operator",
            "extension",
            "typedef",
            "class",
            "enum",
            "mixin",
            "switch",
            "default",
            "while",
            "for",
            "if",
            "try",
            "catch",
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
    /// literal consumed whole, a record shape, else a named leaf (`T`,
    /// `p.T<args>`, `T?`).
    pub(crate) fn parse_annotation_ty(&self, text: &str) -> Option<Ty> {
        if let Some((ty, consumed)) = self.parse_fn_literal_at(text, &FxHashMap::default())
            && consumed == text.trim().len()
        {
            return Some(ty);
        }
        if let Some(ty) = self.parse_record_annotation(text) {
            return Some(ty);
        }
        self.parse_spelled_leaf(text, &FxHashMap::default())
    }

    /// Parses a written record-type annotation (`(T1, T2, {T3 n3})?`): the
    /// `(`-led leaf the named/function legs decline. Positional pieces are
    /// bare types, the optional `{...}` section holds `Type name` pairs;
    /// every field parses through [`Self::parse_annotation_ty`], so
    /// nested records, function literals, and generic leaves all fold.
    /// A malformed shape (an empty middle hole, an unparsable field)
    /// declines the whole recovery (under-report).
    pub(crate) fn parse_record_annotation(&self, text: &str) -> Option<Ty> {
        let text = text.trim();
        let (text, nullable) = text
            .strip_suffix('?')
            .map_or((text, false), |t| (t, true));
        let inner = text.strip_prefix('(')?.strip_suffix(')')?;
        let mut positional: Vec<Ty> = Vec::new();
        let mut named: Vec<(StringId, Ty)> = Vec::new();
        let pieces = Self::split_top_commas(inner, ',');
        for (index, piece) in pieces.iter().enumerate() {
            let piece = piece.trim();
            if piece.is_empty() {
                // Only the trailing comma of a singleton record may be
                // empty (`(String,)`); a hole elsewhere is malformed.
                if index + 1 == pieces.len() && !positional.is_empty() {
                    break;
                }
                return None;
            }
            if let Some(fields) = piece.strip_prefix('{').and_then(|p| p.strip_suffix('}')) {
                for field in Self::split_top_commas(fields, ',') {
                    let field = field.trim();
                    let (ty_text, name_text) = field.rsplit_once(' ')?;
                    let ty = self.parse_annotation_ty(ty_text.trim())?;
                    if !name_text
                        .trim()
                        .chars()
                        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '$'))
                    {
                        return None;
                    }
                    named.push((self.intern(name_text.trim()), ty));
                }
            } else {
                positional.push(self.parse_annotation_ty(piece)?);
            }
        }
        Some(
            self.boundary
                .args
                .borrow_mut()
                .record(&positional, &named, nullable),
        )
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
    ) -> Option<(
        Vec<(StringId, Option<Ty>)>,
        Ty,
        Vec<Ty>,
        u16,
        Vec<(StringId, Ty)>,
    )> {
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
    fn source_alias_fn_ty(&self, name: StringId, args: ArgListId, nullable: bool) -> Option<Ty> {
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

    /// The pooled shape's face as an [`FnVarSig`]: the required-positional
    /// count reads, in order, the caller's `required` override (the face's
    /// own written site — `check_type_check`'s `last_fn_seal` match), the
    /// `fn_shape_required` map (a spelling a count-blind re-intern lost),
    /// and finally the shape's own `required_positional` field (the honest
    /// written count for the split-aware producers, the all-required
    /// reading for the rest). `None` for a non-function or nullable face.
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
                || sig
                    .positional
                    .iter()
                    .any(|formal| self.has_prefix_leaf(*formal))
                || sig
                    .named
                    .iter()
                    .any(|(_, formal, _)| self.has_prefix_leaf(*formal));
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
            || from_params.iter().any(|(name, _)| *name == StringId::NONE)
            || to_params.iter().any(|(name, _)| *name == StringId::NONE)
        {
            return None;
        }
        // `relateTypeParameters` refuses a parameter-count mismatch
        // (upstream type_system.dart:1585-1587) — a generic source never
        // subtypes a non-generic target (`m15 is F15<T>` over a tear-off
        // `List<T> Function(int, [List<int>]) Function()` answers false:
        // the `is` expression's operand stays un-instantiated, so the
        // arity ask short-circuits before the return/positional legs).
        if from_params.len() != to_params.len() {
            return Some(false);
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
        // written count rides `fn_shape_required` for spellings interned
        // through the all-required producers (whose `FnShapeId` merges the
        // split — `Function(int)` and `Function([int])` share one slot);
        // a shape interned with its split baked in
        // (`function_required`/`function_generic_required`, the tear-off
        // faces) reads its own `required_positional` column, and only a
        // shape carrying neither record reads its full length.
        let from_required = self
            .fn_shape_required
            .get(&from_shape)
            .map(|&r| r as usize)
            .unwrap_or_else(|| {
                self.boundary
                    .pool()
                    .function_shape(from_shape)
                    .required_positional as usize
            });
        let to_required = self
            .fn_shape_required
            .get(&to_shape)
            .map(|&r| r as usize)
            .unwrap_or_else(|| {
                self.boundary
                    .pool()
                    .function_shape(to_shape)
                    .required_positional as usize
            });
        if from_required > to_required {
            return Some(false);
        }
        for (index, to_formal) in to_pos.iter().enumerate() {
            match ask(
                self,
                *to_formal,
                self.subst_named_leaves(from_pos[index], &unify),
            ) {
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
            match ask(
                self,
                *to_formal,
                self.subst_named_leaves(from_formal, &unify),
            ) {
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
    /// tear-off operand (`C<int>` of `C<int>.constr`): the shared head
    /// walk ([`Self::receiver_head_face`]) with its group answer alone.
    /// `None` when no group was written (the bare `C` face, whose
    /// all-dynamic raw face a `C<int>` checked spelling cannot spell).
    fn receiver_group_args(&self, row: &Row) -> Option<Vec<Ty>> {
        self.receiver_head_face(row).and_then(|(_, group)| group)
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
        let (type_params, required, names, return_ty, positional, named) = {
            let pool = self.boundary.args.borrow();
            let shape = pool.function_shape(shape);
            (
                shape.type_params,
                shape.required_positional,
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
        // asks correspond on. The shape's own `required_positional` split
        // rides too — the leaf substitution is not a shape re-decision,
        // and the all-required producers would re-intern a different
        // identity (`Function(int, [..])` collapsing into
        // `Function(int, ..)`).
        if names.is_empty() {
            self.boundary.intern_function_ty_required(
                type_params,
                required,
                leaf(return_ty),
                &positional,
                &named,
                nullable,
            )
        } else {
            self.boundary.args.borrow_mut().function_generic_required(
                &names,
                required,
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
            // A cast inside a `?:` arm reports through the flow layer's
            // `cast_tail_row` instead (the official `flowEnd(arm)` covers
            // it): the arm row runs `[type.offset, arm.end)` — this face
            // would duplicate the head.
            || self.cond_expr_depth > 0
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
        let Some(armed) = self.armed_ty else { return };
        // The written target's own shape before the alias/canonical
        // folds: `last_fn_seal` matches it to recover the written
        // signature's required count — the per-site answer the shared
        // `FnShapeId` map cannot give (`fn_face_sig` at the verdict).
        let checked_shape0 = match armed {
            Ty::Function { shape, .. } if shape != crate::model::FnShapeId::EMPTY => Some(shape),
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
        // A parameterized function-RHS alias use (`f15 is F15<bool>`) folds
        // from its ARMED HEAD first: `end_typedef`'s `take_armed` snapshots
        // the anonymous signature's RETURN head alone, so `fold_alias_ty`
        // substitutes the written arguments into an `Unknown Function()`
        // stub carrying no formals. `official_fn_face` then reads that
        // `Unknown` return as the `dynamic` top — the reserved empty shape
        // (`dynamic Function()`) — and every alias-aware fold below is
        // stranded, because they all key on the `Named` face. The pair's
        // verdict then asks a `dynamic Function()` target, which every
        // operand satisfies, and the check reports constant-true: the
        // whole `language/function_type` family (`f15 is F15<bool>` over a
        // `F15<int>` operand, `m13 is F13<int>` over a `F13<T>` one). The
        // written `Named` face is the last point where
        // [`Self::parameterized_alias_fn_ty`] can re-read the alias's own
        // RHS and land the instantiated signature, so the stub hands over.
        let mut checked = {
            let folded = self.repair_alias_fold_split(armed, self.fold_alias_ty(armed));
            match folded {
                Ty::Function { shape, .. } if self.is_armed_head_stub(shape) => {
                    self.parameterized_alias_fn_ty(armed).unwrap_or(folded)
                }
                _ => folded,
            }
        };
        checked = self.normalized_never(self.official_fn_face(checked));
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
                            let params: Vec<(StringId, Option<Ty>)> =
                                if type_params_named.is_empty() {
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
                                if let Some(required) =
                                    self.fn_shape_required.get(&before).copied().or_else(|| {
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
                && matches!(
                    self.resolve_bare(self.intern(span_text)).1,
                    RefTarget::Prefix
                )
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
                .map(|pos| {
                    pos + if span_text[pos..].starts_with(" is! ") {
                        5
                    } else {
                        4
                    }
                });
            if let Some(keyword_end) = is_at {
                let tail = &span_text[keyword_end..];
                let trimmed = tail.trim_start();
                let skip = tail.len() - trimmed.len();
                if let Some((recovered, _)) = self
                    .parse_fn_literal_at(&span_text[keyword_end + skip..], &FxHashMap::default())
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
            None => self.prefixed_operand_ty(self.type_check_operand_ty()),
        };
        // The declared `p.T` face surfaces as the bare prefix
        // (`Named{async}` for `async.A<int> x` — the armed head's
        // prefix-token face): the prefix veto below must not read it as
        // the `e is p` nonsense, whose operand is a bare prefix READ.
        // `prefixed_operand_ty` above restores the other two written
        // shapes the #3611 `known_prefix` family carries — a group-less
        // `p.T` annotation arms nothing the variable can read and a
        // parameterised one arms the leaf (`Named{T}` with the written
        // arguments) — from the declaration's own written spelling, which
        // is where the dropped prefix survives.
        // The tear-off face over an unwritten-signature function (`void
        // topx(x,)` — a bare-read is-check's container-less operand,
        // #2813) reads the same official leaves as the checked side. The
        // `Never` unit fold canonicalizes the written-leaf captures
        // (#2842 — `typedef Fun(Never a, Never b)`'s synthesized formals)
        // before any structural leg asks. The bare alias fold repairs its
        // split-blind capture intern the same way the target side does:
        // an operand declared `TOpt opt` carries the alias's honest
        // required-positional count onto its own shape id.
        let operand_ty = {
            let folded = self.repair_alias_fold_split(operand_ty, self.fold_alias_ty(operand_ty));
            self.normalized_never(self.official_fn_face(folded))
        };
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
        // #3593's signature twin: the resolved checked face of an
        // un-negated `is` test over a local's read joins the engine's
        // promotion-twin table — the flow promotion over the same test
        // spells only the signature-less `Base::Function`, so the pass-2
        // if-null re-decide reads the resolved face here.
        if keyword_operand.is_none()
            && matches!(checked, Ty::Function { shape, .. } if shape != crate::model::FnShapeId::EMPTY)
            && let Some((name, offset)) = self.operand_row().and_then(|row| match row.shape {
                RowShape::Bare(BareInfo::Plain { .. }) => Some((row.name, row.range.offset)),
                _ => None,
            })
        {
            self.record_fn_is_promotion(offset, name, checked, negated);
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
                        let checked_required = checked_shape0.and_then(|shape0| {
                            self.last_fn_seal
                                .and_then(|(sealed, count)| (sealed == shape0).then_some(count))
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
        // A `this`/`super` keyword invoke is the implicit-`call` dispatch
        // on the enclosing instance (`this(42)` over `class C { String
        // call(int) ... }`): the invoked expression is the keyword whose
        // face is the container's `thisType`, and the send's static type
        // is its `call` member's declared return — the same rewrite every
        // callable receiver runs. No `call` member keeps the under-report
        // (the arm only ever answers a resolved member).
        if let RowShape::Receiver(
            Receiver::This { container, kind } | Receiver::Super { container, kind },
        ) = &row.shape
        {
            let this_ty = self.this_static_ty(*container, *kind);
            if let Some((_, call_ret, _)) = self.implicit_call_face(this_ty) {
                return call_ret;
            }
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
            // A stored `void` on a DECLARED-callee send can be a misfired
            // implicit-`call` retyping: the send-close fold rewrites the
            // row to the callee type's `call` return whenever
            // `implicit_call_face(row.ty)` answers, and a declared
            // method's or function's return type that happens to name a
            // callable class (`C m()` on `class C { void call() {} }`,
            // invoked `d.m()`/`D.m()`) folds the row to `void` where the
            // send's static type is `C`. Upstream only a value callee
            // runs `.call` — when the callee is a declared head and its
            // declared return is not itself `void`, the declared answer
            // is the send's static type and the stored `void` is wrong.
            // A genuine `void` return keeps `void` (the recompute answers
            // `Void` identically); a value callee's `void` is its real
            // `.call`-dispatch face and never reaches this arm.
            if row.ty == Ty::Void
                && !self.is_value_callee(row)
                && let declared @ (Ty::Void | Ty::Named { .. } | Ty::Function { .. } | Ty::Dynamic) =
                    self.callee_return_ty(row)
                && declared != Ty::Void
            {
                return declared;
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
            // A BARE callee's function-typed `ty` is the send's
            // already-flattened RESULT, never the callee's own face: the
            // send close folds a non-null function-typed bare callee onto
            // its return before any verdict reads the row, and the send
            // result lands on the row the same way — so `int Function(int)
            // top()` invoked `top()` carries `int Function(int)` here, and
            // unwrapping it again ([`Self::invoked_callee_result`]) would
            // answer `int` where the oracle answers the function type
            // (`method_invocation_resolver.dart` types the node by the
            // invoked signature's return — one unwrap, never two). A
            // NULLABLE function type survives that fold only as the
            // callee's own standing face (`int Function(int)? f` invoked
            // `f()` — the fold refuses nullable faces so the
            // unchecked-use verdict owns the value), so it keeps the
            // unwrap arms below — except when no local binds the name: a
            // top-level send's nullable result (`int Function(int)?
            // top()`) is as flattened as its non-null sibling. Member and
            // prefixed rows are exempt wholesale: their faces arm at the
            // dot merge, which the deferred pass can run AFTER the send
            // close, so a function-typed member `ty` may still be the
            // callee's own face owed exactly one unwrap
            // (`comparison(a, b)` on `int Function(E, E) comparison`).
            if !row.qualified && self.bare_invoke_ty_is_result(row) {
                return row.ty;
            }
            // The bare `Function` interface spelling whose declaration
            // names a result keeps that result: `Function top()` reads
            // `Function`, never the interface's `dynamic` dispatch face —
            // the callee spelling (`Function f` invoked `f()`, whose
            // declaration names no return) is the dispatch:
            // `InvocationInferrer.computeInvokeReturnType`'s else arm
            // (invocation_inferrer.dart:727-733) types every invoke whose
            // callee face is not a function signature `dynamic`, so a
            // `Function`-typed local/param's send is `dynamic` — strictly
            // non-nullable's question never fires on it (`f2(42)!` stays
            // silent, co19 `null_check_operator_A02_t01`'s rows) and the
            // result assigns everywhere the oracle's `dynamic` does.
            if !row.qualified
                && let Ty::Named { name, .. } = row.ty
                && self.strings.resolve(name) == "Function"
            {
                let declared = self.callee_return_ty(row);
                if declared != Ty::UNKNOWN {
                    return declared;
                }
                return Ty::Dynamic;
            }
            // A function-typed callee (or the bare `Function` interface)
            // answers the send face ([`Self::invoked_callee_result`]).
            if let Some(result) = self.invoked_callee_result(row) {
                return result;
            }
            return self.callable_call_ret(row, row.ty).unwrap_or(row.ty);
        }
        self.callee_return_ty(row)
    }

    /// Whether a bare Invoke row's function-typed `ty` is the send's
    /// result rather than the callee's own standing face
    /// ([`Self::operand_static_ty`]'s gate): a non-null function type
    /// always is (the send-close fold consumed every non-null bare face
    /// ahead of any read); a nullable one is unless a local of that name
    /// still binds the face in scope.
    fn bare_invoke_ty_is_result(&self, row: &Row) -> bool {
        match row.ty {
            Ty::Function {
                nullable: false, ..
            } => true,
            Ty::Function { nullable: true, .. } => {
                !self.frames.iter().any(|f| f.locals.contains(&row.name))
            }
            _ => false,
        }
    }

    /// The untyped row's static type the oracle still answers
    /// (`expression.typeOrThrow`): `this`/`super` keyword receivers read the
    /// enclosing container's `thisType`; a folded creation (`C()`,
    /// `C<int>()`, `ET(1)`) reads the created instance type — the written
    /// type arguments or solved instantiation through
    /// [`Self::creation_row_ty`], the bare head's to-bounds face otherwise
    /// (the same send-result face [`Self::single_row_since`]'s
    /// `Receiver::Created` arm answers); a bare container name or a
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
            // A creation fold's own row keeps no `ty` while the generic
            // head's inference stays open, but the `InstanceCreationExpression`
            // types as its container regardless — `C()` as a `==` left
            // operand arms the shorthand's `C` context, and `ET(1)` an
            // extension type's. The `MicroKind::Extension` tag conflates a
            // plain `extension`'s `E(1)` — upstream an invalid call whose
            // operand types `InvalidType` — so the resolved kind decides,
            // the same discrimination `handle_is_operator`'s created-type
            // arm runs. Enum and mixin `E(`/`M(` spellings are likewise no
            // creation upstream and keep the suppress.
            RowShape::Receiver(Receiver::Created { name, kind, .. })
                if *kind == ContainerKind::Class
                    || (*kind == ContainerKind::Extension
                        && self.container_kind_of(*name) == Some(ContainerKind::ExtensionType)) =>
            {
                self.creation_row_ty(*name, false, row.range.offset)
                    .filter(|ty| *ty != Ty::UNKNOWN)
                    .unwrap_or_else(|| self.instantiated_head_ty(Ty::named(*name)))
            }
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
    ///
    /// The callee must additionally be a VALUE — the same predicate
    /// [`Self::value_send_span`] runs: a member send's resolved member is
    /// a field or getter (a `Method` member's send IS its declared return,
    /// never a `.call` dispatch — `localFunction()` on `void Function()
    /// localFunction` is the field face; `instanceMethod()` on a declared
    /// method is the method's own return), and a bare send is a
    /// function-typed formal/local or a local with no declared-function
    /// capture. A declared-function send (`topLevelFunction()`,
    /// `D.staticMethod()`) carries the function's return type on the row —
    /// that face never names a callable container, but a spelled return
    /// type that happens to be a callable class name must not re-dispatch:
    /// `C topLevel()` invoked `topLevel()` returns `C`, not `C.call`'s
    /// return.
    fn callable_call_ret(&self, row: &Row, callee_ty: Ty) -> Option<Ty> {
        let Ty::Named {
            name,
            nullable: false,
            ..
        } = callee_ty
        else {
            return None;
        };
        if !self.is_value_callee(row) {
            return None;
        }
        let entry = self.resolved_member(name, self.intern("call"))?;
        if entry.kind != MemberKind::Method || entry.is_static || entry.type_params != 0 {
            return None;
        }
        (self.declared_type_params_of_container(name) == Some(0)).then_some(entry.return_ty)
    }

    /// `TypeSystem::flatten` — see [`flatten_ty`] for the head/nullability
    /// arms. This method adds the two arms the name-level helper cannot
    /// reach:
    ///
    /// * the type-parameter arm (upstream Dart SDK source,
    ///   `pkg/analyzer/lib/src/dart/element/type_system.dart`:423-435): an
    ///   in-scope parameter `X` flattens through its captured bound — `X
    ///   extends Future<A>` awaits to `A`, `X extends FutureOr<A>` to `A`,
    ///   `X extends A` stays `X`, a chained parameter (`X extends Y`,
    ///   `Y extends Future<A>`) recurses through `Y`. The bound itself is
    ///   fed back through this method so a `Future<A>?` bound awaits to
    ///   `A?` and a bare `Future` bound awaits to `dynamic`. Unbounded or
    ///   non-Future bounds leave the parameter spelled as written
    ///   (identity), mirroring the oracle's `otherwise flatten(X)` return.
    /// * the derived-future arm — upstream's `futureType(T)` walks
    ///   `T.asInstanceOf(futureElement)` (:464-478), so a user class,
    ///   mixin, enum, or extension type that `implements Future<E>` awaits
    ///   to `E` ([`Self::future_supertype_arg`]).
    ///
    /// The walk is bounded by the parameter chain: a bound that names
    /// the parameter itself (`X extends X` — rejected upstream) or loops
    /// back would re-enter; the `depth` guard caps the recursion at the
    /// in-scope parameter count, beyond which the parameter stays its
    /// own flatten (under-report, never a hang).
    pub(crate) fn flatten(&self, ty: Ty) -> Ty {
        // The chain cap doubles as the post-walk budget: in-scope the
        // parameter count bounds the chain exactly, but the replays run
        // with `type_params` emptied — a fixed small bound still covers
        // every real chain (`X extends Y extends Z extends Future<A>`)
        // while guaranteeing termination on cyclic spellings.
        self.flatten_param(ty, 8)
    }

    fn flatten_param(&self, ty: Ty, depth: usize) -> Ty {
        let Ty::Named {
            name,
            nullable,
            args: list,
        } = ty
        else {
            return ty;
        };
        // The `S?` arm mirrors `flatten_ty`'s own split — strip, flatten,
        // re-apply — but the recursion must run through this method so a
        // `X?` parameter keeps reading its bound's future type.
        if nullable {
            let flat = self.flatten_param(
                Ty::Named {
                    name,
                    nullable: false,
                    args: list,
                },
                depth,
            );
            return match flat {
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
                flat => flat,
            };
        }
        // The parameter arm (type_system.dart:437-454 +
        // `_futureTypeOfBounded`'s TypeParameterType recursion,
        // :1856-1872): `X`'s future type is its bound's, chained through
        // bound parameters (`X extends Y`, `Y extends Future<A>` → `A`).
        // Only a bound that actually flattens to a future type rewrites
        // `X`; every other bound leaves `X` spelled (the oracle's
        // `otherwise flatten(X)` return — awaiting a non-future parameter
        // yields the parameter itself, never the bound).
        // The bound lookup rides the in-scope window when the scope is
        // live (`param_bound` — authoritative), and the post-walk captures
        // when the rows re-derive the operand's face after teardown
        // (`param_bound_of` — the await-extent replays in row_resolution /
        // invocation run with `type_params` already emptied; a name that
        // no capture declares answers `None`, the concrete-head case).
        if list == ArgListId::EMPTY && depth > 0 {
            let bound = if self.is_scope_param(name) {
                self.param_bound(name)
            } else {
                self.param_bound_of(name)
            };
            if let Some(bound) = bound
                && bound != ty
                && bound != Ty::UNKNOWN
            {
                let flat_bound = self.flatten_param(bound, depth - 1);
                if flat_bound != bound {
                    return flat_bound;
                }
                return ty;
            }
        }
        let flat = flatten_ty(self.strings, &self.boundary.pool(), ty);
        if flat != ty {
            return flat;
        }
        // The derived-future arm (`TypeSystem.futureType`'s
        // `T.asInstanceOf(futureElement)` walk, type_system.dart:464-478):
        // a written head that is neither `Future` nor `FutureOr` — the
        // name-level helper already answered both — but maps into
        // `Future<E>` through its superinterface chain awaits to `E`.
        // `flatten(SubFuture)` is `int`, never `flatten(int)` again: the
        // official flatten takes the future type's argument verbatim
        // (`flatten(Future<Future<int>>)` is `Future<int>`, :443-455), so
        // the argument is NOT fed back through this method.
        if depth > 0
            && let Some(arg) = self.future_supertype_arg(name, list)
        {
            return arg;
        }
        ty
    }

    /// `T`'s `Future<E>` argument through the superinterface chain —
    /// `TypeSystem.futureType`'s first arm (upstream Dart SDK source,
    /// `pkg/analyzer/lib/src/dart/element/type_system.dart`:464-472:
    /// "T implements S, and there is a U such that S is Future<U>"), read
    /// off the strict snapshot's instantiated supertype view
    /// (`ResolvedTypeOps::instance_env`). Only a class, mixin, enum, or
    /// extension type `implements Future<E>` head answers — a type
    /// parameter's bound walk is [`Engine::flatten_param`]'s own arm, and
    /// the `Future`/`FutureOr` heads were already peeled by
    /// [`flatten_ty`]. `None` when the snapshot cannot answer (an
    /// unregistered head, an unspellable argument shape): the keep
    /// direction.
    fn future_supertype_arg(&self, name: StringId, args: ArgListId) -> Option<Ty> {
        if name == self.intern("Future") || name == self.intern("FutureOr") {
            return None;
        }
        let ops = self.ops?;
        let env = ops.instance_env(
            Ty::Named {
                name,
                nullable: false,
                args,
            },
            self.intern("Future"),
            &self.boundary.args,
        )?;
        env.first().map(|(_, ty)| *ty)
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
    pub(crate) fn core_head(&self, name: StringId) -> bool {
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
        // The declared required/optional split joins the face's identity
        // (`FnShape::required_positional`), like
        // [`Self::member_entry_tear_off_ty`].
        let required = formals
            .map(|f| f.positional.iter().filter(|p| p.required).count() as u16)
            .unwrap_or(0);
        let mut args = self.boundary.args.borrow_mut();
        let fn_ty = if type_params.is_empty() && entry.type_params > 0 {
            args.function_required(
                entry.type_params,
                required,
                entry.return_ty,
                &positional,
                &named,
                false,
            )
        } else {
            args.function_generic_required(
                &type_params,
                required,
                entry.return_ty,
                &positional,
                &named,
                false,
            )
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
    /// first (`void f<T extends num>(T t)`, the [`Engine::exec_windows`]
    /// capture), then the enclosing container's capture (`class C<T
    /// extends num>`, the `container_params` face). Both record `(param,
    /// bound)` per owner in declaration order; the innermost owner wins.
    pub(crate) fn param_bound(&self, name: StringId) -> Option<Ty> {
        if let Some(exec) = self.exec_self_name() {
            if let Some(window) = self.own_exec_window(exec, false)
                && let Some((_, bound)) = window.params.iter().find(|(param, _)| *param == name)
            {
                return *bound;
            }
            if let Some(window) = self.own_exec_window(exec, true)
                && let Some((_, bound)) = window.params.iter().find(|(param, _)| *param == name)
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

    /// The LAST captured window declaring `owner` on the requested side —
    /// the exact-owner lookup (`keep-last`, matching the former map
    /// inserts), a deterministic scan over declaration order.
    pub(crate) fn own_exec_window(&self, owner: StringId, is_method: bool) -> Option<&ExecWindow> {
        self.exec_windows
            .iter()
            .rev()
            .find(|window| window.owner == owner && window.is_method == is_method)
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
    /// function's window or a method's window — so the lookup scans every
    /// captured window for one that declares `name`. Returns `Some(bound)`
    /// when a capture declares `name` (the implicit `Object?` when its slot
    /// is empty), `None` when no window does — i.e. `name` is a concrete
    /// head (`String`, `Color`), never a scope parameter. A class-generic
    /// `class C<T>` receiver stays unmodeled (`container_params` needs the
    /// container name the post-walk caller no longer holds — the
    /// under-report direction).
    ///
    /// The scan iterates [`Engine::exec_windows`] in DECLARATION order —
    /// the one ordering a name-only lookup may read (#4202: hash order is
    /// per-run unstable under parallel interning). `at` — the reference's
    /// own offset where the caller knows it — narrows the pick to the
    /// nearest declaration at or before the reference, so each reference
    /// reads its own enclosing scope; without it the first declaration
    /// wins.
    pub(crate) fn param_bound_of(&self, name: StringId) -> Option<Ty> {
        self.param_bound_of_at(name, None)
    }

    /// [`Self::param_bound_of`] with the reference offset: the nearest
    /// declaring window at or before `at` answers; the first declaration
    /// otherwise.
    pub(crate) fn param_bound_of_at(&self, name: StringId, at: Option<u32>) -> Option<Ty> {
        let implicit_object = || Ty::Named {
            name: self.intern("Object"),
            nullable: true,
            args: ArgListId::EMPTY,
        };
        let mut first = None;
        let mut nearest = None;
        for window in &self.exec_windows {
            let Some((_, bound)) = window.params.iter().find(|(param, _)| *param == name) else {
                continue;
            };
            let bound = bound.unwrap_or_else(implicit_object);
            first.get_or_insert(bound);
            if at.is_some_and(|at| window.at < at) {
                nearest = Some(bound);
            }
        }
        nearest.or(first)
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
        let (ty, read_span, qualified, super_operand) = match self.rows.last() {
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
                    // A `super` operand never reports the null-check pair:
                    // `_checkForUnnecessaryNullAware` returns before the
                    // strictly-non-null question for a `SuperExpression`
                    // target (error_verifier.dart:7299-7302), and
                    // `nullCheckAlwaysFails` needs a `Null`-typed operand
                    // `super` never carries — so `super!` and `super()!`
                    // stage nothing (the oracle's silence on both shapes,
                    // co19 `null_check_operator_A04_t01`'s rows).
                    matches!(row.shape, RowShape::Receiver(Receiver::Super { .. })),
                )
            }
            None => return,
        };
        if self.is_void_spelling(self.fold_alias_ty(ty)) {
            // The `!`-postfix operand's own face (upstream
            // `visitPostfixExpression`'s `checkForUseOfVoidResult(node)` on
            // the whole `x!`, error_verifier.dart:1692 →
            // error_detection_helpers.dart:245-258): a `void`-typed
            // operand's value is consumed by the assertion — the oracle
            // reports `use_of_void_result` at the operand-plus-`!` span
            // before the null-check faces ever ask their question. The
            // span opens at the operand EXPRESSION's head — a member
            // operand's `x.m()!` spans from `x`, not the `m` selector.
            let head = self
                .rows
                .last()
                .and_then(|row| self.property_access_span(row.range).map(|span| span.offset));
            let range = SourceRange {
                offset: head.unwrap_or(read_span.offset),
                length: at.end() - head.unwrap_or(read_span.offset),
            };
            self.emit(SemanticCode::UseOfVoidResult, range, "", "");
        } else if ty == Ty::Null {
            // The whole postfix node anchors the always-fails row
            // (`nullCheckAlwaysFails.at(node)` over the `e!` expression,
            // best_practices_verifier.dart:752-754): the span opens at the
            // operand EXPRESSION's head — a member operand's `x.m()!`
            // spans from `x` — and closes at the `!`, exactly the extent
            // the `use_of_void_result` arm above computes (`null!` is
            // five columns, never the operator's one).
            let head = self
                .rows
                .last()
                .and_then(|row| self.property_access_span(row.range).map(|span| span.offset));
            let offset = head.unwrap_or(read_span.offset);
            let range = SourceRange {
                offset,
                length: at.end() - offset,
            };
            self.stage_usage(SemanticCode::NullCheckAlwaysFails, range, "");
        } else if super_operand {
            // `_resolveNullCheck`'s SuperExpression arm (upstream Dart SDK
            // source, `pkg/analyzer/lib/src/resolver/
            // postfix_expression_resolver.dart:196-205`): a bare `super!`
            // has no selector to assert on, so the RESOLVER reports the
            // parser-tier selector row over the whole `super!` node —
            // `missingAssignableSelector.at(node)` — and returns before
            // any promotion. The engine's `super` bail (the
            // `_checkForUnnecessaryNullAware` skip above) covers the
            // verifier pair; this arm carries the resolver row the oracle
            // answers for the same fixture (`super!?[0]` → one
            // MISSING_ASSIGNABLE_SELECTOR at `super!`, nothing else —
            // probed 3.13.7, ticket #3802).
            let head = self
                .rows
                .last()
                .and_then(|row| self.property_access_span(row.range).map(|span| span.offset));
            let offset = head.unwrap_or(read_span.offset);
            let range = SourceRange {
                offset,
                length: at.end() - offset,
            };
            let code = SemanticCode::MissingAssignableSelector;
            self.report.diagnostics.push(Diagnostic::new(
                code.severity(),
                code.code_info(),
                code.message("", ""),
                self.file,
                range,
                Some("Try adding a selector.".to_string()),
            ));
        } else if !super_operand && self.strictly_non_nullable(ty) {
            self.stage_usage(SemanticCode::UnnecessaryNonNullAssertion, at, "");
        } else if !qualified && self.local_hits.contains_key(&read_span.offset) {
            // A bare local read stages whatever its walk-time type — the
            // unknown faces (an implicitly typed formal, an unannotated
            // `var`) promote through `is` exactly like `dynamic` upstream
            // (`_checkForUnnecessaryNullAware` asks only
            // `isStrictlyNonNullable`, error_verifier.dart:7352), so the
            // teardown re-decision, not the declared face, carries their
            // verdict (#3954). A never-promoted read's staged entry simply
            // answers `None` at the join — the suppression stays in the
            // promotion table, never in this gate.
            //
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
            // `promoteToNonNull(Null)` is `Never` (the assertion's only
            // success path throws): the postfix node's static type over a
            // `Null` operand is bottom, so `f(null!)` offers `Never` —
            // assignable everywhere — and the argument face stays silent
            // exactly as the oracle's `null!` verdicts do.
            row.ty = if super_operand {
                // The `_resolveNullCheck` arm types BOTH the operand and
                // the whole `super!` node `dynamic` and returns before the
                // promotion — the selector face below (`super!?[0]`) reads
                // a `dynamic` receiver, so every null-aware question on it
                // stays silent (probed 3.13.7: no
                // `invalid_null_aware_operator` beside the selector row).
                Ty::Dynamic
            } else if ty == Ty::Null {
                Ty::Never
            } else {
                Self::non_null_form(ty)
            };
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
            // The resolved face decides: an alias use's `staticType` IS
            // the aliased type (`typedef R1 = (int, String)?;` reads a
            // nullable record, never the strictly-non-null raw head), the
            // same fold every typed face applies.
            self.fold_alias_ty(operand.ty)
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
        // The `MicroKind::Extension` tag conflates a plain `extension`'s
        // `E(1)` — upstream an invalid call whose operand types
        // `InvalidType` — with an extension-type creation, so the resolved
        // kind decides (the same discrimination [`Self::untyped_operand_ty`]
        // runs). Enum and mixin `E(`/`M(` spellings are no creation
        // upstream and keep the suppress.
        if let RowShape::Receiver(Receiver::Created { name, kind, .. }) = row.shape
            && (kind == ContainerKind::Class
                || (kind == ContainerKind::Extension
                    && self.container_kind_of(name) == Some(ContainerKind::ExtensionType)))
        {
            // The creation's INSTANCE type, never the bare head (#3063):
            // the created row's own solved face (`pending_send_ty`) or the
            // written `<…>` group keyed on the row's offset
            // ([`Self::creation_row_ty`]) instantiates the container's
            // parameters, so `Box<int>(1) + make()` resolves the operator's
            // `Box<int>` formals and return instead of leaving `T` free.
            // An unwritten group (`C()`, `ET(1)`) keeps the bare head — the
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
        let right_ty = self.equality_operand_ty(right);
        let left_ty = self.equality_operand_ty(left);
        // The right operand's void face precedes every formal lookup —
        // upstream `checkForArgumentTypeNotAssignable`'s `checkForUseOf
        // VoidResult` short-circuit (error_detection_helpers.dart:49-50)
        // reports `use_of_void_result` whenever a `void` actual lands in a
        // non-`void` formal. The `==`/`!=` invocation's default formal is
        // `Object` (`Object.==(Object)` — the receiver's own `==`
        // redeclaration aside), so a `void` right operand fires on every
        // resolved `==` face and on every face the name-level walk cannot
        // see — the under-report posture only carves out the declared-`void`
        // formal (`bool operator ==(void o)`).
        // The spelled-`void` check reads through the alias fold: a
        // typedef-`void` actual (`typedef V = void; V x`) carries the
        // alias head upstream folds to `VoidTypeImpl` — the dedicated
        // variant alone would under-report it ([`Self::emit_use_of_void
        // _result`]'s own gate folds the same way).
        if self.is_void_spelling(self.fold_alias_ty(right_ty)) {
            let void_formal = match left_ty {
                Ty::Named { name, .. } => self
                    .member_formals_full(
                        &Receiver::Typed {
                            type_name: name,
                            ty: left_ty,
                        },
                        self.intern("=="),
                    )
                    .and_then(|formals| formals.positional.first().map(|f| f.ty))
                    .is_some_and(|formal| self.is_void_spelling(self.fold_alias_ty(formal))),
                _ => false,
            };
            if !void_formal {
                self.emit_use_of_void_result(right_ty, right);
            }
            return;
        }
        let left_ty @ Ty::Named { name, .. } = left_ty else {
            return;
        };
        // A type-parameter receiver resolves `==` through its bound
        // (upstream resolves the member on the receiver's `leftType`, and a
        // `TypeParameterType`'s member interface IS its bound's —
        // binary_expression_resolver.dart:147-151's `findMember` over the
        // bound's interface, the same face `subclass_of` runs): `X extends
        // Covar1`'s `x == e` asks `Covar1.==`'s `covariant Covar1` formal,
        // `?`-promoted for the verdict exactly like every receiver face. No
        // recoverable bound keeps the under-report (an unbounded parameter's
        // effective bound is `Object?`, whose formal the `Object?` arm below
        // silences anyway).
        let (recv_name, recv_ty) = if self.declared_head_is_type_param(name) {
            match self.param_bound(name) {
                Some(
                    bound @ Ty::Named {
                        name: bound_name, ..
                    },
                ) => (bound_name, bound),
                _ => return,
            }
        } else {
            (name, left_ty)
        };
        // The operand's INSTANTIATED type rides the receiver: `C<int>(1)`'s
        // `C<int>` binds the `T` the container's `==` formal spells, the same
        // substitution `member_recv_env` composes for every typed receiver
        // (upstream resolves `==` on the receiver's `leftType` —
        // binary_expression_resolver.dart:451-459 — and the instantiated
        // element's formal renders `C<int>`). A bare head (`C()`, `x`)
        // carries no arguments, `member_recv_env` answers `None`, and the
        // declared table stands — the pre-#3719 face.
        let formals = self.member_formals_full(
            &Receiver::Typed {
                type_name: recv_name,
                ty: recv_ty,
            },
            self.intern("=="),
        );
        let Some(formal) = formals
            .as_deref()
            .and_then(|formals| formals.positional.first())
        else {
            return;
        };
        if matches!(formal.ty, Ty::Unknown | Ty::Void) {
            return;
        }
        if right_ty == Ty::Unknown {
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
    pub(crate) fn nullable_extension_operator_sig(
        &mut self,
        recv: Ty,
        op: StringId,
    ) -> Option<(Ty, Ty)> {
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
        self.binary_operator_sig_typed(op, left_ty)
    }

    /// [`Self::binary_operator_sig`]'s signature resolution over an
    /// already-resolved receiver type — the teardown recompute (#3980)
    /// hands the promoted operand face directly, skipping the
    /// frame-dependent `bound_face`/`equality_operand_ty` derivation the
    /// fold-time row consult runs (both read `self.frames`, dead after
    /// pass 2).
    fn binary_operator_sig_typed(&mut self, op: &str, left_ty: Ty) -> Option<(Ty, Ty)> {
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
                        .all(|t| !self.is_never_ty(*t) && self.name_subtype(*t, named_q(head)))
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
        // `Never` stays excluded (the upstream `!s.isBottom` cut,
        // type_system.dart:2029/:2036 — the written `Never` spelling
        // included, which the name lattice folds to the bottom atom and
        // would grant against `double?`) and falls to `num`.
        if !self.is_never_ty(right_ty)
            && (right_invalid || self.name_subtype(right_ty, named_q(double)))
        {
            return Ty::named(double);
        }
        if !self.is_never_ty(right_ty)
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
        // The operands read their flow-PROMOTED static types (upstream
        // types each operand through the branch's flow facts):
        // `if (n is int) (n + i)` resolves on the promoted `int`, not the
        // declared `num` — the promotion latch answers the read at the
        // operand's own span; an operand without a fact keeps the
        // declared face.
        let promoted_operand_ty = |row: &Row, ty: Ty| -> Ty {
            self.read_type_at(row.range)
                .filter(|promoted| *promoted != Ty::Unknown)
                .unwrap_or(ty)
        };
        let left_ty = promoted_operand_ty(&left, self.equality_operand_ty(&left));
        let right_ty = promoted_operand_ty(&right, self.equality_operand_ty(&right));
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
            // The if-null join's `T2'` callable coercion lives in
            // `rhs_row_ty`'s deferred-operand recovery (assignability.rs
            // `join_operand_ty`'s `coerce_if_null_callable` arm — private
            // to that module). Keeping `UNKNOWN` here routes the `??`
            // composite's reads through that coerced join instead of a
            // local one that would miss `Function? ?? callable` →
            // `Function`.
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
        let node_range = SourceRange {
            offset: expr_start,
            length: right.range.end().saturating_sub(expr_start),
        };
        // Record the fold for the teardown recompute (#3980): the
        // `read_type_at` consults above starved because `read_types` fills
        // only after `parse_unit` returns. Skip only the promotion-invariant
        // arms (`==`/`!=`/`&&`/`||`/`??` — their result is `bool`/`UNKNOWN`
        // regardless of operand promotion). Every other operator records:
        // an operand that is itself a composite node (a `Value` row, never
        // a bare read) can still shift when the inner fold's recompute
        // lands, so the operand-shape gate would drop the outer record the
        // order-independent forward pass needs.
        if !matches!(
            op,
            o if o == self.intern("==")
                || o == self.intern("!=")
                || o == self.intern("&&")
                || o == self.intern("||")
                || o == self.intern("??")
        ) {
            PENDING_COMPOSITE_FOLDS.with(|queue| {
                let mut queue = queue.borrow_mut();
                if queue.len() < PENDING_COMPOSITE_FOLD_CAP {
                    let mut params: Vec<StringId> =
                        self.type_params.iter().copied().collect();
                    params.extend(self.pending_type_params.iter().copied());
                    queue.push(PendingCompositeFold::Binary {
                        file: self.file,
                        node_range,
                        op,
                        left: left.clone(),
                        right: right.clone(),
                        left_ty,
                        right_ty,
                        params: params.into_boxed_slice(),
                        result,
                    });
                }
            });
        }
        self.rows.push(Row {
            name: self.intern(""),
            range: node_range,
            qualified: false,
            shape: RowShape::Value,
            intent: Intent::Read,
            write_op: None,
            ty: result,
        });
    }

    // —— #3681: composite (`?:`/`??`) result fold ————————————————————————
    // Upstream records a static type on the whole conditional/if-null
    // node (`static_type_analyzer.dart`'s `visitConditionalExpression` /
    // `resolveIfNull`: `T = LUB(T1, T2)` / `LUB(NonNull(T1), T2')`, then
    // the greatest-closure-of-K merge). The row model folded no result,
    // so the parenthesized group's `inner_ty`, a cascade's receiver, and
    // the argument faces read the right ARM's own type (`d` typed
    // `double` where the node is `num`), and the `expectStaticType`
    // receiver asked `Exactly<double>` of `Exactly<num>` — the census
    // shape-1.4a rows.

    /// The `?:` node's own static type, folded at
    /// `end_conditional_expression`. The stack at that event is
    /// `[.., condition.., then, else]` — the else arm is the last row and
    /// the then arm ends where the `:` operator starts — so the same
    /// positional pairing [`Self::conditional_arms`] uses for `??`
    /// applies. Arms drain into `deferred_miss_rows` (their own verdicts
    /// still run in pass 2) and one `RowShape::Value` row spanning the
    /// whole conditional carries the joined type — the same contract
    /// [`Self::fold_binary`] keeps for the operator composites, so the
    /// parenthesized group's `operand_expr_head` gate reaches the
    /// condition's head and reads the node's type.
    ///
    /// `question`/`end` anchor the folded row's extent (the listener
    /// passes the `?` and the else arm's end token).
    pub(crate) fn fold_conditional(&mut self, question: SourceRange, end: SourceRange) {
        let Some((then_index, else_index)) = self.conditional_arms() else {
            return;
        };
        // The arms read their flow-PROMOTED static types (upstream
        // flow-analyzes each branch under the test's facts — `c == null ?
        // 0 : c` promotes the else arm's `c` to its non-null form): the
        // promotion latch answers the read at the arm's own span; an arm
        // without a fact keeps the row's own face.
        let arm_ty = |row: &Row| -> Ty {
            self.read_type_at(row.range)
                .filter(|promoted| *promoted != Ty::Unknown)
                .unwrap_or_else(|| self.operand_static_ty(row))
        };
        let then_ty = arm_ty(&self.rows[then_index]);
        let else_ty = arm_ty(&self.rows[else_index]);
        let mut ty = if then_ty == Ty::UNKNOWN || else_ty == Ty::UNKNOWN {
            Ty::UNKNOWN
        } else {
            self.composite_lub(then_ty, else_ty, false)
        };
        // The node's head is the condition's operand row — the nearest
        // live non-`TypeUse` row ending at or before the `?` (a composite
        // condition folds to its own `Value` row already).
        let head = self.rows[..then_index]
            .iter()
            .rev()
            .find(|row| {
                !matches!(row.shape, RowShape::TypeUse(..)) && row.range.end() <= question.offset
            })
            .map(|row| row.range.offset)
            .unwrap_or(question.offset);
        // The arm rows STAY live: their own verdicts (the dot-shorthand
        // context sweep, dead-arm rows, `use_of_void_result`) re-read them
        // from the live stack, and a shorthand arm's context-resolution
        // runs `live` only while the row sits there. The `Value` row on
        // top is the node's face — every upward read
        // (`operand_static_ty`, the parenthesized group's `inner_ty`,
        // `rhs_row_ty`, the cascade receiver) reads `rows.last()`.
        //
        // `end` is the token the listener hands in — the else arm's own
        // consumed tail for a send or a postfix/type-test expression
        // (`)`, `++`, `!`, a type tail), but the send-leaf followER for a
        // bare read or member read whose tiling consumed no delimiter
        // (#585): the unconsumed `;`/`,`/`)`/`]`/`}`/`=`/`:` that begins
        // the surrounding construct. A follower always sits at or past
        // the else arm's use-site end and is always one of those
        // delimiter kinds — a consumed tail token can never be one — so
        // kind decides before the position check even runs.
        let else_row = &self.rows[else_index];
        let arm_site_end = if matches!(else_row.intent, Intent::Invoke) {
            self.value_send_span(else_row)
        } else {
            self.property_access_span(else_row.range)
        }
        .map_or(else_row.range.end(), |span| span.end())
        .max(else_row.range.end());
        let is_follower = end.offset >= arm_site_end
            && self
                .tokens
                .as_slice()
                .iter()
                .find(|t| t.utf16_offset == end.offset && t.byte_len == end.length)
                .is_some_and(|t| {
                    matches!(
                        t.kind,
                        dartq_scanner::TokenKind::Semicolon
                            | dartq_scanner::TokenKind::Comma
                            | dartq_scanner::TokenKind::CloseParen
                            | dartq_scanner::TokenKind::CloseSquare
                            | dartq_scanner::TokenKind::CloseCurly
                            | dartq_scanner::TokenKind::Eq
                            | dartq_scanner::TokenKind::Colon
                    )
                });
        let tail = if is_follower { arm_site_end } else { end.end() };
        // The `inference_update_3` context arm (upstream Dart SDK source,
        // `pkg/analyzer/lib/src/generated/static_type_analyzer.dart`:74-112):
        // a conditional in direct return position is typed under the
        // return expression's context — when the plain LUB falls outside
        // it but BOTH arms fit, the context type itself is the node's
        // static type (`x ? 42 : Future.value(42)` under an async
        // `Future<int>` return types `FutureOr<int>`, never `Object`).
        if ty != Ty::UNKNOWN {
            ty = self.conditional_return_context_ty(ty, then_ty, else_ty, head, tail);
        }
        let node_range = SourceRange {
            offset: head,
            length: tail.saturating_sub(head),
        };
        // Record the fold for the teardown recompute (#3980): the arm
        // `read_type_at` consults starved during pass 1. The context face
        // (`return_expression_context`) reads `self.frames`, which is dead
        // at teardown — capture its answer now. `conditional_return_context_ty`
        // already ran `assignable` on the fold-time arms; the recompute
        // re-asks it with the promoted arm types and the same context. Every
        // conditional records: an arm that is itself a composite node can
        // shift when the inner fold's recompute lands, so an arm-shape gate
        // would drop the record the order-independent forward pass needs.
        let context = if ty != Ty::UNKNOWN {
            self.return_expression_context(head, tail)
        } else {
            None
        };
        PENDING_COMPOSITE_FOLDS.with(|queue| {
            let mut queue = queue.borrow_mut();
            if queue.len() < PENDING_COMPOSITE_FOLD_CAP {
                queue.push(PendingCompositeFold::Conditional {
                    file: self.file,
                    node_range,
                    then_row: self.rows[then_index].clone(),
                    else_row: self.rows[else_index].clone(),
                    then_ty,
                    else_ty,
                    context,
                    result: ty,
                });
            }
        });
        self.rows.push(Row {
            name: self.intern(""),
            range: node_range,
            qualified: false,
            shape: RowShape::Value,
            intent: Intent::Read,
            write_op: None,
            ty,
        });
    }

    /// [`Self::fold_conditional`]'s `inference_update_3` arm: upstream
    /// `visitConditionalExpression` types the node `T = UP(T1, T2)` unless
    /// a context `K` stands (`T <: S` keeps `T`; `T1 <: S` and `T2 <: S`
    /// with `S` the greatest closure of `K` take `S`; anything else keeps
    /// `T` — upstream Dart SDK source,
    /// `pkg/analyzer/lib/src/generated/static_type_analyzer.dart`:94-112).
    /// The greatest closure of a closed written schema is the schema
    /// itself, so the arm reduces to the subtype questions below. Only a
    /// DIRECT return expression carries a context here (`var y = cond`
    /// binds the plain `T`, c6-shaped); an undecidable ask keeps `T` (the
    /// under-report direction).
    fn conditional_return_context_ty(
        &self,
        lub: Ty,
        then_ty: Ty,
        else_ty: Ty,
        head: u32,
        tail: u32,
    ) -> Ty {
        let Some(context) = self.return_expression_context(head, tail) else {
            return lub;
        };
        if self.assignable(lub, context, false) == Some(true) {
            return lub;
        }
        if self.assignable(then_ty, context, false) == Some(true)
            && self.assignable(else_ty, context, false) == Some(true)
        {
            return context;
        }
        lub
    }

    /// The context type a return expression gives its expression: the
    /// innermost executable frame's latched declared return, `Future<T>`
    /// widened to `FutureOr<T>` under `async` (upstream's return-expression
    /// context, `body_inference_context.dart` — `Future<int> f() async`
    /// types `return e` under `FutureOr<int>`). `None` unless the
    /// conditional spanning `head..tail` IS the whole returned expression:
    /// the tokens before `head`, open parens skipped, spell `return` or
    /// the `=>` arrow, and the matching close parens after `tail` end the
    /// statement — `return (cond) + 1;` fails the forward leg and keeps
    /// the plain LUB, exactly as the oracle's nested-context inference
    /// does not reach the conditional through `+`'s own context.
    fn return_expression_context(&self, head: u32, tail: u32) -> Option<Ty> {
        let tokens = self.tokens.as_slice();
        let mut open = tokens.partition_point(|t| t.utf16_offset < head);
        let mut depth = 0usize;
        while open > 0 && tokens[open - 1].kind == dartq_scanner::TokenKind::OpenParen {
            open -= 1;
            depth += 1;
        }
        // An `=>` arrow only counts for a frame that owns the return — a
        // function literal's arrow body meets the innermost frame's
        // unlatched (context-inferred) return below and answers `None`.
        match tokens.get(open.checked_sub(1)?)?.kind {
            dartq_scanner::TokenKind::Keyword(dartq_scanner::Keyword::Return) => {}
            dartq_scanner::TokenKind::Function => {}
            _ => return None,
        }
        let mut close = tokens.partition_point(|t| t.utf16_offset < tail);
        while depth > 0 {
            // A non-paren token before the parens close: the conditional
            // is a proper sub-expression (`+ 1`, an argument, an index)
            // and owns no return context.
            if tokens.get(close)?.kind != dartq_scanner::TokenKind::CloseParen {
                return None;
            }
            depth -= 1;
            close += 1;
        }
        if !matches!(
            tokens.get(close)?.kind,
            dartq_scanner::TokenKind::Semicolon | dartq_scanner::TokenKind::Comma
        ) {
            return None;
        }
        // The INNERMOST executable owns the return (the same frame
        // `check_return` walks): a function literal's frame carries no
        // latched declared return, and the conditional in its body owns
        // that literal's context — never the enclosing method's declared
        // face — so the walk stops there.
        let frame = self
            .frames
            .iter()
            .rev()
            .find(|f| matches!(f.kind, FrameKind::Executable { .. }))?;
        let FrameKind::Executable {
            ret: Some(ret),
            is_async,
            ..
        } = &frame.kind
        else {
            return None;
        };
        let (ret, is_async) = (ret.ty, *is_async);
        // The async widening runs only over a genuine `Future`/`FutureOr`
        // declared head (`futureValueType`'s own gate); every other
        // declared shape keeps itself as the context — the oracle's
        // non-future async returns ask their value face directly.
        let future_head = matches!(
            ret,
            Ty::Named { name, .. }
                if (name == self.intern("Future") || name == self.intern("FutureOr"))
                    && self.named_type_is_type(name)
        );
        if !is_async || !future_head {
            return Some(ret);
        }
        // The unwrap runs BEFORE the pool borrow: a method receiver is
        // evaluated before its arguments, so `future_value_type` inside
        // the re-intern's argument list read the pool (`pool()`'s shared
        // borrow) against the live `RefMut` and panicked — the same
        // discipline `tear_off_fn_ty` keeps.
        let value = self.future_value_type(ret, false);
        Some(
            self.boundary
                .args
                .borrow_mut()
                .named(self.intern("FutureOr"), false, &[value]),
        )
    }

    /// The `??=` node's own static type, staged at
    /// `handle_assignment_expression` for the parenthesized close:
    /// `LUB(NonNull(T1), T2)` (upstream `_resolveIfNullAssignment`'s
    /// `recordStaticType`). The target is the closest non-`Value` row
    /// ending at the operator; the RHS is the last non-`TypeUse` row.
    /// The join rides the same `if_null_results` latch
    /// `stage_if_null_result` uses for a parenthesized `??` — keyed on
    /// the RHS row's offset — so `(x ??= y)`'s `inner_ty` reads the
    /// node's type, never the RHS's own. The rows stay untouched: an
    /// assignment's read/write bookkeeping keys on the target being the
    /// last real row, which a folded result row would disturb.
    pub(crate) fn fold_if_null_assignment(&mut self, op_range: SourceRange, end: SourceRange) {
        self.fold_if_null_assignment_with(op_range, end, None);
    }

    /// [`Self::fold_if_null_assignment`] with the target read's FLOW type
    /// when the caller has one (#3822). Upstream's `T1` is "the static
    /// type `e1` would have as an expression with context type schema
    /// `_`" — the PROMOTED read type, not the declared one: under
    /// `if (local is Iterable<int>?)` the read is `Iterable<int>?`, so
    /// `UP(NonNull(T1), T2')` is `Iterable<num>`, never `Object`. The
    /// promotion facts only materialize after pass 1
    /// ([`crate::diagnose::FileReport`]'s read-type table), so the walk
    /// passes them in when its flow state can answer; `None` keeps the
    /// declared-face answer (the under-report direction the join had
    /// before the promotion plumbing).
    pub(crate) fn fold_if_null_assignment_with(
        &mut self,
        op_range: SourceRange,
        _end: SourceRange,
        promoted_target: Option<Ty>,
    ) {
        let rhs_index = match self.rows.iter().rposition(|row| {
            !matches!(row.shape, RowShape::TypeUse(..)) && row.range.offset >= op_range.end()
        }) {
            Some(index) => index,
            None => return,
        };
        // The target: the nearest row wholly before `??=`. A `[]` fold's
        // `Value` stand-in IS the index target (its offset rides
        // `index_result_recvs`); a member/bare/keyword receiver names the
        // target itself.
        let Some(target_index) = self.rows[..rhs_index].iter().rposition(|row| {
            !matches!(row.shape, RowShape::TypeUse(..)) && row.range.end() <= op_range.offset
        }) else {
            return;
        };
        let target_ty = promoted_target
            .or_else(|| {
                // The flow-published read of the target (#3822's T1 — the
                // PROMOTED read type, not the declared one): the identifier
                // bridge (`drain_published_read_types`) lands earlier
                // executables' facts before the operator event, so a
                // target promoted under an earlier executable's `is` answers
                // its promoted face here. A promotion from THIS executable's
                // own branch is still pending (the #3914 ordering — the
                // replay runs at its end) and the S-branch tri-state below
                // carries that band.
                let range = self.rows[target_index].range;
                self.read_type_at(range).filter(|ty| *ty != Ty::UNKNOWN)
            })
            .unwrap_or_else(|| self.operand_static_ty(&self.rows[target_index]));
        let rhs_ty = self.operand_static_ty(&self.rows[rhs_index]);
        if target_ty == Ty::UNKNOWN || rhs_ty == Ty::UNKNOWN {
            return;
        }
        let lhs_offset = self.rows[target_index].range.offset;
        // A `?.`-merged target null-shorts the whole `??=` —
        // `Extension(s)?.p ??= v` reads `Null` whenever `s` is null, so the
        // node's type keeps the nullable form (`resolveIfNull`'s
        // `nullAware` join), never the bare non-null `UP` face. The
        // `?.`-merged selector lives in `null_aware_members`; a plain
        // `local ??= v` carries no `?.` and keeps the `UP` face (`Function`,
        // never `Function?`).
        //
        // A STATIC qualifier target never short-circuits: `D?.staticE`
        // reports `invalid_null_aware_operator` and resolves as the plain
        // `.` access, so `F? f = (D?.staticE ??= e)` types the bare `E`
        // (probed against the 3.13.7 oracle, whose row is
        // `A value of type 'E' can't be assigned to a variable of type
        // 'F?'.`). Upstream decides this by the receiver's syntax, not its
        // nullability (`assignment_expression_resolver.dart`'s
        // `analyzePropertyTargetReceiver` returns early for
        // `StaticQualifierImpl`), and the same exclusion already gates the
        // `=` twin at `listener.rs`'s `handle_assignment_expression`.
        // The gate is NOT "any receiver the `?.` warns about": an extension
        // override over a non-null receiver warns the same way yet still
        // short-circuits (`EX(s)?.p ??= e` types `E?`, same probe), so only
        // the `Container`/`PrefixedContainer` pair — the merge's own static
        // classification, the one that emits the invalid-operator
        // warning — is excluded.
        let static_receiver = matches!(
            &self.rows[target_index].shape,
            RowShape::Member {
                recv: Receiver::Container { .. } | Receiver::PrefixedContainer { .. }
            }
        );
        let shorted = !static_receiver && self.null_aware_members.contains(&lhs_offset);
        let rhs_offset = self.rows[rhs_index].range.offset;
        let ty = self.composite_lub(target_ty, rhs_ty, true);
        let ty = if shorted {
            Self::nullable_form(ty)
        } else {
            match self.if_null_context_face(op_range, ty, target_ty, rhs_ty) {
                IfNullContext::Join(t) => t,
                IfNullContext::Schema(s) => s,
                // The S-branch is promotion-pending (`NonNull(T1) <: S`
                // undecidable until the executable's replay publishes the
                // target's promoted read — the #3914 ordering): the
                // parse-time join stands for mid-walk consumers, and the
                // extension-bounds seam abstains over the composite
                // (#3820's band).
                IfNullContext::Pending => {
                    self.if_null_s_pending.insert(rhs_offset);
                    ty
                }
            }
        };
        self.if_null_results.insert(rhs_offset, (lhs_offset, ty));
    }

    /// [`Self::check_type_check`]'s signature-twin record (#3593): the
    /// checked face of an un-negated `is` test over a local read joins
    /// `fn_is_promotions`, appended in source order and gated by the
    /// same step decision the flow promotion chain runs — a later test
    /// whose face is not a strict-subtype promotion from the name's
    /// standing face (unrelated signatures; the chain keeps the earlier
    /// head) leaves the standing entry, and a negated test's factored
    /// remainder is undecidable at the seam and retires the name's
    /// entries.
    fn record_fn_is_promotion(&mut self, offset: u32, name: StringId, face: Ty, negated: bool) {
        if negated {
            self.fn_is_promotions
                .retain(|&(_, entry_name, _)| entry_name != name);
            return;
        }
        if let Some(&(_, _, standing)) = self
            .fn_is_promotions
            .iter()
            .rev()
            .find(|&&(_, entry_name, _)| entry_name == name)
        {
            if standing == face {
                return;
            }
            let non_null = Self::non_null_form;
            let steps = self.resolved_subtype(non_null(face), non_null(standing))
                && !self.resolved_subtype(non_null(standing), non_null(face));
            if !steps {
                return;
            }
        }
        self.fn_is_promotions.push((offset, name, face));
    }

    /// Pass-2 re-decide of the if-null composite join over a
    /// flow-confirmed function promotion (#3593): the parse-time join
    /// read the target's DECLARED face — the flow replay publishes the
    /// promoted read only at its executable's end, after the operator
    /// event — so `local ??= callable` under `local is C1<int>
    /// Function()?` joined the bare `Function` interface. The target
    /// read's `promotion_function_reads` entry is the promotion's
    /// proof; the signature twin ([`Self::fn_is_promotions`], the
    /// latest test on the operand's name before the target read) names
    /// the face. The re-join runs the parse-time `composite_lub` with
    /// the twin's face and the RHS row's own operand face (the
    /// callable-object coercion included), keeping the parse-time
    /// nullability posture (a `?.`-shorted target stays nullable).
    /// Every other entry — no twin, no flow proof, or a join that did
    /// not degrade to the bare interface — keeps its parse-time face.
    pub(crate) fn redecide_promoted_if_null_faces(&mut self) {
        if self.fn_is_promotions.is_empty() || self.if_null_results.is_empty() {
            return;
        }
        let function = self.intern("Function");
        let entries: Vec<(u32, (u32, Ty))> = self.if_null_results.drain().collect();
        for (rhs_offset, (lhs_offset, parse_ty)) in entries {
            let joined = (|| {
                if !self.promotion_function_reads.contains(&lhs_offset) {
                    return None;
                }
                if !matches!(self.fold_alias_ty(parse_ty), Ty::Named { name, .. } if name == function)
                {
                    return None;
                }
                let target = match self.rows.iter().find(|row| row.range.offset == lhs_offset) {
                    Some(row) => row,
                    None => {
                        return None;
                    }
                };
                let face = match self
                    .fn_is_promotions
                    .iter()
                    .rev()
                    .find(|&&(at, entry_name, _)| entry_name == target.name && at < lhs_offset)
                {
                    Some((_, _, face)) => *face,
                    None => {
                        return None;
                    }
                };
                let rhs = match self.rows.iter().find(|row| row.range.offset == rhs_offset) {
                    Some(row) => row,
                    None => {
                        return None;
                    }
                };
                let rhs_ty = self.operand_static_ty(rhs);
                if rhs_ty == Ty::UNKNOWN {
                    return None;
                }
                let joined = self.composite_lub(face, rhs_ty, true);
                let nullable = matches!(
                    parse_ty,
                    Ty::Named { nullable: true, .. }
                        | Ty::Function { nullable: true, .. }
                        | Ty::Record { nullable: true, .. }
                );
                Some(if nullable {
                    Self::nullable_form(joined)
                } else {
                    joined
                })
            })()
            .unwrap_or(parse_ty);
            if joined != parse_ty {
                self.rewrite_if_null_consumers(rhs_offset, parse_ty, joined);
            }
            self.if_null_results.insert(rhs_offset, (lhs_offset, joined));
        }
    }

    /// Rewrite the parse-time consumers of a re-decided if-null join
    /// (#3593): the composite's face froze at parse into three row shapes
    /// — the parenthesized group row (`end_parenthesized_expression`'s
    /// `inner_ty`), the receivers that captured it at the merge
    /// (`merge_member`'s push-time classification; `rederived_receiver`
    /// never re-decides cascade sections, so the stale face stands unless
    /// rewritten here), and the cascade's own value row (`end_cascade`
    /// re-pushes the target face — upstream types the whole cascade as
    /// its target's static type). The scan is bounded to the composite's
    /// own statement — every cascade section, type argument, and argument
    /// row of its consumers lives before the next `;` (or `}`/EOF for
    /// the terminator-less tail) — and a row counts as a consumer only
    /// when its frozen face equals the parse join byte for byte, so an
    /// unrelated row sharing the spelling keeps its own face.
    fn rewrite_if_null_consumers(&mut self, rhs_offset: u32, parse_ty: Ty, joined: Ty) {
        let mut limit = u32::MAX;
        for token in &self.tokens.as_slice()[self.token_at_offset(rhs_offset)..] {
            if matches!(
                token.kind,
                dartq_scanner::TokenKind::Semicolon | dartq_scanner::TokenKind::CloseCurly
            ) {
                limit = token.utf16_offset;
                break;
            }
        }
        for row in self.rows.iter_mut() {
            if row.range.offset <= rhs_offset || row.range.offset >= limit {
                continue;
            }
            let rewritten = match &mut row.shape {
                RowShape::Bare(BareInfo::Plain { ty }) if *ty == parse_ty => {
                    *ty = joined;
                    row.ty = joined;
                    true
                }
                RowShape::Member {
                    recv: Receiver::Typed { ty, .. },
                }
                | RowShape::Receiver(Receiver::Typed { ty, .. })
                    if *ty == parse_ty =>
                {
                    *ty = joined;
                    true
                }
                RowShape::Value if row.ty == parse_ty => {
                    row.ty = joined;
                    true
                }
                _ => false,
            };
        }
    }

    /// The if-null composite's context S-branch outcome (#3820, upstream
    /// `_computeIfNullType` under `inference_update_3`,
    /// assignment_expression_resolver.dart:869-906 — the same branch
    /// `resolveIfNull` runs for the plain `??`,
    /// binary_expression_resolver.dart:112-127): with the composite's
    /// context schema `K` and `S = greatestClosure(K)`, the node's type is
    /// `T` when `T <: S`, else `S` when `NonNull(T1) <: S` and `T2 <: S`,
    /// else `T`. The schema rides the argument position's formal — a
    /// composite in an argument of `contextB1<T>(B1<T> x)` closes its
    /// schema to `B1<Object?>`; under `if (local1 is C1<int>?)` the
    /// promoted join `UP(C1<int>, C2<double>) = A` (`<:`-fails `S`) stamps
    /// `S`, so `(local1 ??= c2Double)` types `B1<Object?>` — the face the
    /// corpus's `..expectStaticType<Exactly<B1<Object?>>>()` bound checks
    /// against.
    ///
    /// The one input the parse walk cannot see is the target's PROMOTED
    /// read (the flow replay publishes it only at the executable's end —
    /// the #3914 ordering), so the second condition is tri-state:
    /// [`IfNullContext::Pending`] when every decidable input already
    /// admits the branch and only `NonNull(T1) <: S` waits on a promotion
    /// (`T <!: S`, `T2 <: S`, the unpromoted `NonNull(T1) <!: S`). A miss
    /// on any gate (no argument position, no top-level callee, a shadowed
    /// or member-owned spelling) keeps the join face.
    fn if_null_context_face(
        &self,
        op_range: SourceRange,
        up: Ty,
        target_ty: Ty,
        rhs_ty: Ty,
    ) -> IfNullContext {
        let Some(s) = self.argument_context_schema(op_range.offset) else {
            return IfNullContext::Join(up);
        };
        // `T <: S` keeps the join face.
        if self.assignable(up, s, false) == Some(true) {
            return IfNullContext::Join(up);
        }
        // `T2 <: S` is promotion-independent — its failure closes the
        // branch for good (the join face stands).
        if self.assignable(rhs_ty, s, false) != Some(true) {
            return IfNullContext::Join(up);
        }
        // `NonNull(T1) <: S` decides between the context face and the
        // pending band.
        if self.assignable(Self::non_null_form(target_ty), s, false) == Some(true) {
            IfNullContext::Schema(s)
        } else {
            IfNullContext::Pending
        }
    }

    /// The innermost open argument's formal schema — the context type the
    /// oracle hands `analyzeExpression` for the argument's expression (a
    /// bare top-level callee's formal, the callee's own type parameters
    /// still free), closed through [`Self::schema_greatest_closure`].
    fn argument_context_schema(&self, operator_offset: u32) -> Option<Ty> {
        let level = self.arg_levels.last()?;
        let callee = level.callee_row?;
        let row = self.rows.get(callee)?;
        if row.qualified || row.range.end() > operator_offset {
            return None;
        }
        let RowShape::Bare(BareInfo::Plain { .. }) = &row.shape else {
            return None;
        };
        let name = row.name;
        // The callee-ownership gates `check_arguments`'s bare arm runs: a
        // shadowing local invokes dynamically, and an implicit-this member
        // owns the spelling ahead of the namespace — neither may lend its
        // formals as the schema.
        if self.frames.iter().any(|f| f.locals.contains(&name)) {
            return None;
        }
        if let Some((container, kind)) = self.container_context()
            && matches!(self.bare_member_walk(container, kind, name), Walk::Found(_))
        {
            return None;
        }
        let decl = self.top_level_decl(name)?;
        let params: &[StringId] = &decl.type_param_names;
        let formal = if let Some(label) = level.pending_name {
            decl.formals.as_deref()?.named_formal(label)?
        } else {
            let slot = level.args.iter().filter(|(n, ..)| n.is_none()).count();
            decl.formals.as_deref()?.positional.get(slot)?
        };
        Some(self.schema_greatest_closure(formal.ty, params, true))
    }

    /// `greatestClosure` over the schema's free type parameters
    /// (type_system.dart:592-620 over least_greatest_closure.dart:12-105,
    /// the Ty-world twin of bounds.rs's snapshot walker): every parameter
    /// leaf eliminates to the top (`Object?`) in covariant positions and
    /// the bottom (`Never`, a nullable leaf to `Null`) in contravariant
    /// ones. Interface arguments are covariant; a parameter-free schema
    /// closes to itself. Function and record schemas that mention a
    /// parameter keep their written shape — the under-report direction
    /// (the subtype asks over a free leaf stay undecidable).
    fn schema_greatest_closure(&self, ty: Ty, params: &[StringId], covariant: bool) -> Ty {
        match ty {
            Ty::Named {
                name,
                nullable,
                args,
            } if args == ArgListId::EMPTY && params.contains(&name) => {
                if covariant {
                    Ty::Named {
                        name: self.intern("Object"),
                        nullable: true,
                        args: ArgListId::EMPTY,
                    }
                } else if nullable {
                    Ty::Null
                } else {
                    Ty::Never
                }
            }
            Ty::Named {
                name,
                nullable,
                args,
            } => {
                let list = self.boundary.args.borrow().get(args).to_vec();
                let closed: Vec<Ty> = list
                    .iter()
                    .map(|&arg| self.schema_greatest_closure(arg, params, covariant))
                    .collect();
                if closed == list {
                    return ty;
                }
                self.boundary
                    .args
                    .borrow_mut()
                    .named(name, nullable, &closed)
            }
            _ => ty,
        }
    }

    /// The `(then, else)` row pair of the innermost open `?:`: the else
    /// arm is the last non-`TypeUse` row, the then arm is the nearest
    /// non-`TypeUse` row ENDING where the `:` operator begins. A `?`
    /// before the then arm's start distinguishes the conditional's `?`
    /// from a map literal's or named argument's `:` (mirroring
    /// `binary_join_operands`' guard in assignability.rs — kept local
    /// because that module's pairing is private).
    fn conditional_arms(&self) -> Option<(usize, usize)> {
        let else_index = self
            .rows
            .iter()
            .rposition(|row| !matches!(row.shape, RowShape::TypeUse(..)))?;
        let else_row = &self.rows[else_index];
        let toks = self.tokens.as_slice();
        // The `:` is the token immediately before the else arm (only
        // trivia separates them).
        let hi = toks.partition_point(|t| t.utf16_offset < else_row.range.offset);
        let colon = toks[..hi]
            .iter()
            .rev()
            .find(|t| t.kind == dartq_scanner::TokenKind::Colon)?;
        let then_index = self.rows[..else_index].iter().rposition(|row| {
            !matches!(row.shape, RowShape::TypeUse(..))
                && row.range.end() <= colon.utf16_offset
                && !self.interpolation_child_rows.contains(&row.range.offset)
        })?;
        let then_row = &self.rows[then_index];
        // The matching `?` sits before the then arm (a named-argument or
        // map-literal `:` pairs no `?` between the prior row's end and
        // the then arm's start). Scan the token window that precedes the
        // then row back to the previous row's end.
        let before = self.rows[..then_index]
            .iter()
            .rev()
            .find(|row| {
                !matches!(row.shape, RowShape::TypeUse(..))
                    && !self.interpolation_child_rows.contains(&row.range.offset)
            })
            .map_or(0, |row| row.range.end());
        let lo = toks.partition_point(|t| t.utf16_offset < before);
        toks[lo..]
            .iter()
            .take_while(|t| t.utf16_offset < then_row.range.offset)
            .any(|t| t.kind == dartq_scanner::TokenKind::Question)
            .then_some((then_index, else_index))
    }

    /// The least upper bound two composite operand types join to —
    /// upstream `TypeSystem.leastUpperBound` (`least_upper_bound.dart`
    /// 391-672), the same rule order assignability.rs's `join_operand_ty`
    /// runs for the assignment face. `non_null_left` applies the
    /// `??`/`??=` `NonNull(T1)` rewrite (`resolveIfNull` /
    /// `_resolveIfNullAssignment`): a `Null` left becomes `Never`, every
    /// other left loses its `?`.
    fn composite_lub(&self, left: Ty, right: Ty, non_null_left: bool) -> Ty {
        if left == right {
            // `UP(T, T) = T` precedes every rule — including the non-null
            // rewrite (`int? ?? int?` joins `int?`, not `int`).
            return left;
        }
        let left = if non_null_left {
            if left == Ty::Null {
                Ty::Never
            } else {
                Self::non_null_form(left)
            }
        } else {
            left
        };
        match (left, right) {
            // `LUB(InvalidType, X) = X` and the BOTTOM rule.
            (Ty::Unknown, other) | (other, Ty::Unknown) => return other,
            (Ty::Never, other) | (other, Ty::Never) => return other,
            // TOP: `dynamic`/`void` absorb.
            (top @ (Ty::Dynamic | Ty::Void), _) => return top,
            (_, Ty::Dynamic | Ty::Void) => return right,
            _ => {}
        }
        // NULL: `UP(Null, T)` is `T`/`T?`.
        if let (Ty::Null, other) | (other, Ty::Null) = (left, right) {
            return Self::nullable_form(other);
        }
        // The if-null callable coercion (`resolveIfNull`'s `T2'`): the
        // `??=`/`??` value operand's static type already carries the `.call`
        // tear-off rewrite (`analyzeExpression`'s popRewrite applies the
        // callable-interface coercion), so a `call`-declaring class right
        // operand facing a function-shaped left joins as its `call`
        // signature — `local ??= callableClass` against a `Function?` local
        // is `UP(Function, int Function())` = `Function`, never `Object`.
        // The `??` arm [`Self::join_operand_ty`] runs the same rewrite.
        let (left, right) = if non_null_left {
            self.coerce_if_null_callable(left, right)
        } else {
            (left, right)
        };
        // Nullability merge: `UP(T1?, T2) = UP(T1, T2)?`.
        if Self::nullable_lub_leg(left) || Self::nullable_lub_leg(right) {
            return Self::nullable_form(
                self.composite_lub_stripped(Self::non_null_form(left), Self::non_null_form(right)),
            );
        }
        self.composite_lub_stripped(left, right)
    }

    /// Whether the type carries the trailing `?` (the nullability-merge
    /// gate; `Null` never reaches it — NULL ran first). Named apart from
    /// assignability.rs's private `potentially_nullable_ty`, the same
    /// predicate that module keeps for its own join.
    fn nullable_lub_leg(ty: Ty) -> bool {
        matches!(
            ty,
            Ty::Named { nullable: true, .. }
                | Ty::Function { nullable: true, .. }
                | Ty::Record { nullable: true, .. }
        )
    }

    /// [`Self::composite_lub`]'s stripped core: both operands non-null,
    /// no `Null`/top/bottom legs left.
    fn composite_lub_stripped(&self, left: Ty, right: Ty) -> Ty {
        if left == right {
            return left;
        }
        // A bare `Function` head is the supertype every function type
        // shares — the pair collapses onto it.
        let bare_function = |ty: Ty| matches!(ty, Ty::Named { name, nullable: false, args } if name == self.intern("Function") && args == ArgListId::EMPTY);
        if bare_function(left) || bare_function(right) {
            if matches!(left, Ty::Function { .. }) || matches!(right, Ty::Function { .. }) {
                return if bare_function(left) { left } else { right };
            }
        }
        match (left, right) {
            (
                Ty::Named {
                    name: l, args: la, ..
                },
                Ty::Named {
                    name: r, args: ra, ..
                },
            ) => {
                let world = self.name_world();
                if l == r {
                    // Same head over covariant arguments: the pairwise
                    // LUB (`Iterable<int>`/`Iterable<double>` →
                    // `Iterable<num>`); a bare or arity-mismatched pair
                    // keeps the written right (under-report).
                    let (left_args, right_args) = {
                        let pool = self.boundary.pool();
                        (pool.get(la).to_vec(), pool.get(ra).to_vec())
                    };
                    if la != ra && left_args.len() == right_args.len() && !left_args.is_empty() {
                        let joined: Vec<Ty> = left_args
                            .iter()
                            .zip(right_args.iter())
                            .map(|(&a, &b)| self.composite_lub(a, b, false))
                            .collect();
                        return self.boundary.args.borrow_mut().named(l, false, &joined);
                    }
                    return Ty::Named {
                        name: l,
                        nullable: false,
                        args: if la == ra { la } else { ra },
                    };
                }
                if world.named_sub(r, l) && (ra == ArgListId::EMPTY || ra == la) {
                    return Ty::Named {
                        name: l,
                        nullable: false,
                        args: la,
                    };
                }
                if world.named_sub(l, r) && (la == ArgListId::EMPTY || la == ra) {
                    return Ty::Named {
                        name: r,
                        nullable: false,
                        args: ra,
                    };
                }
                self.composite_common_supertype(left, right)
            }
            (Ty::Record { .. }, Ty::Record { .. }) => right,
            (Ty::Record { .. }, _) | (_, Ty::Record { .. }) => Ty::named(self.intern("Object")),
            // The function-type LUB (upstream
            // `LeastUpperBoundHelper.getLeastUpperBoundOfFunctionTypes`,
            // least_upper_bound.dart:663-779): a compatible pair joins
            // elementwise — the RETURN types lift (`C1<int> Function()` /
            // `C2<int> Function()` → `A Function()`) and each paired formal
            // takes the greatest lower bound. The forms need not be
            // identical: surplus optional positionals and unmatched named
            // formals drop out of the joined signature, exactly as the
            // official walk does. Only a missing REQUIRED formal (or an
            // undecidable formal type) falls back to the bare `Function`
            // head — the same under-report direction the arm answered
            // before the lift, never a fabricated rejection.
            (Ty::Function { shape: l, .. }, Ty::Function { shape: r, .. }) => self
                .function_type_lub(l, r)
                .unwrap_or_else(|| Ty::named(self.intern("Function"))),
            (Ty::Function { .. }, _) | (_, Ty::Function { .. }) => Ty::named(self.intern("Object")),
            _ => right,
        }
    }

    /// [`Self::composite_lub_stripped`]'s function-type join, mirroring
    /// upstream `_functionType` (least_upper_bound.dart:663-779) gate for
    /// gate:
    ///
    /// - the type-parameter COUNTS must agree, else the bare `Function`
    ///   head (upstream :667-671);
    /// - the required-positional counts must agree: upstream walks both
    ///   lists until the kinds diverge, then rejects any *leftover*
    ///   required formal (:757-769), and required positionals form a
    ///   prefix — so a count difference always leaves a required formal
    ///   behind (:695-704's break);
    /// - the positional lists need NOT be the same length: the walk pairs
    ///   `min(len)` entries and the surplus *optional* ones are simply
    ///   dropped from the result (:757-769 skips non-required leftovers);
    /// - the named sets need NOT match: upstream matches by name and skips
    ///   an unmatched *optional* named formal, bailing only when the
    ///   unmatched one is required (:715-753). The shape carries no
    ///   per-named requiredness, so an unmatched name is dropped here —
    ///   the under-report direction, never a fabricated rejection.
    ///
    /// The return slot takes the LUB (:771) and each paired formal the
    /// GLB (:700/:710/:729). `None` — the bare-`Function` fallback —
    /// whenever a gate or a formal cannot decide.
    fn function_type_lub(
        &self,
        left: crate::model::FnShapeId,
        right: crate::model::FnShapeId,
    ) -> Option<Ty> {
        let (type_params, required, ret, positional, named) = {
            let pool = self.boundary.args.borrow();
            let a = pool.function_shape(left);
            let b = pool.function_shape(right);
            // Upstream `relateTypeParameters` (least_upper_bound.dart:675)
            // must also relate the BOUNDS and instantiate both sides with
            // the shared fresh set; that substitution is not modelled here,
            // so a differing count still bails as it did before.
            if a.type_params != b.type_params {
                return None;
            }
            // Required positionals are a prefix on both sides; a count
            // difference always strands a required formal on the longer
            // side, which upstream rejects (:757-769).
            if a.required_positional != b.required_positional {
                return None;
            }
            let paired = a.positional.len().min(b.positional.len());
            if paired < a.required_positional as usize {
                return None;
            }
            (
                a.type_params,
                a.required_positional,
                (a.return_ty, b.return_ty),
                (
                    a.positional[..paired].to_vec(),
                    b.positional[..paired].to_vec(),
                ),
                (a.named.to_vec(), b.named.to_vec()),
            )
        };
        let return_ty = self.composite_lub(ret.0, ret.1, false);
        let mut joined = Vec::with_capacity(positional.0.len());
        for (x, y) in positional.0.iter().zip(positional.1.iter()) {
            joined.push(self.composite_glb(*x, *y)?);
        }
        let mut joined_named = Vec::with_capacity(named.0.len());
        for (name, x) in named.0.iter() {
            let Some((_, y)) = named.1.iter().find(|(other, _)| other == name) else {
                // An unmatched named formal is dropped (upstream skips the
                // optional ones); a required one would bail upstream, which
                // the shape cannot spell.
                continue;
            };
            joined_named.push((*name, self.composite_glb(*x, *y)?));
        }
        Some(self.boundary.args.borrow_mut().function_required(
            type_params,
            required,
            return_ty,
            &joined,
            &joined_named,
            false,
        ))
    }

    /// The two-type greatest lower bound the function-formal join needs
    /// (upstream `getGreatestLowerBound`): equality, the `Never` bottom,
    /// the top faces, and the subtype pair decide; everything else answers
    /// `None` so the caller keeps the bare-`Function` fallback instead of
    /// inventing a formal type.
    fn composite_glb(&self, left: Ty, right: Ty) -> Option<Ty> {
        if left == right {
            return Some(left);
        }
        if left == Ty::Never || right == Ty::Never {
            return Some(Ty::Never);
        }
        if matches!(left, Ty::Dynamic) {
            return Some(right);
        }
        if matches!(right, Ty::Dynamic) {
            return Some(left);
        }
        if self.name_subtype(left, right) {
            return Some(left);
        }
        if self.name_subtype(right, left) {
            return Some(right);
        }
        None
    }

    /// The cross-head common-supertype walk (the official interface LUB's
    /// instantiated-closure intersection): candidates both operands reach
    /// under the SAME instantiation (`instance_env` equality — `C1<int>`
    /// and `C2<double>` both reach `B1`, but as `B1<int>`/`B1<double>`,
    /// so `B1` is out and the walk answers `A`), then the max-unique-depth
    /// descent. Empty/undecidable answers `Object` — the oracle's terminal
    /// upper bound.
    fn composite_common_supertype(&self, left: Ty, right: Ty) -> Ty {
        let Ty::Named { name: l, .. } = left else {
            return right;
        };
        let Ty::Named { name: r, .. } = right else {
            return right;
        };
        let world = self.name_world();
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
        let mut candidates: Vec<(StringId, Vec<Ty>)> = Vec::new();
        for head in heads {
            if !world.named_sub(l, head) || !world.named_sub(r, head) {
                continue;
            }
            if let Some(args) = self.composite_env_args(left, right, head) {
                candidates.push((head, args));
            }
        }
        if candidates.is_empty() {
            return Ty::named(self.intern("Object"));
        }
        let mut level = candidates;
        loop {
            let dominated: Vec<usize> = (0..level.len())
                .filter(|&i| {
                    (0..level.len()).any(|j| {
                        j != i
                            && world.named_sub(level[j].0, level[i].0)
                            && !world.named_sub(level[i].0, level[j].0)
                    })
                })
                .collect();
            let top: Vec<(StringId, Vec<Ty>)> = level
                .iter()
                .enumerate()
                .filter(|(i, _)| !dominated.contains(i))
                .map(|(_, c)| c.clone())
                .collect();
            if top.len() == 1 {
                let (head, args) = top[0].clone();
                return self.boundary.args.borrow_mut().named(head, false, &args);
            }
            if top.len() == level.len() {
                return Ty::named(self.intern("Object"));
            }
            // Drop the tied top level; the descent continues among the
            // candidates it dominated (least_upper_bound.dart's
            // max-unique-depth loop — `C implements I1, I2; D implements
            // I1, I2` over a shared `J` answers `J`, the next level
            // down). Keeping the tied top here would strand the walk on
            // mutually incomparable faces and fall to `Object`.
            level = level
                .into_iter()
                .enumerate()
                .filter(|(i, _)| dominated.contains(i))
                .map(|(_, candidate)| candidate)
                .collect();
        }
    }

    /// Both operands' `asInstanceOf(head)` instantiation args, equal —
    /// the shared-instantiation certificate
    /// [`Self::composite_common_supertype`] requires. `None` when the
    /// snapshot cannot answer either side (under-report: the candidate
    /// drops).
    fn composite_env_args(&self, left: Ty, right: Ty, head: StringId) -> Option<Vec<Ty>> {
        if let Some(ops) = self.ops {
            let env_left = ops.instance_env(left, head, &self.boundary.args)?;
            let env_right = ops.instance_env(right, head, &self.boundary.args)?;
            return (env_left == env_right)
                .then(|| env_left.into_iter().map(|(_, ty)| ty).collect());
        }
        let (container, _owner) = resolve_container(
            self.graph,
            self.namespaces,
            self.index,
            self.lib,
            self.file,
            head,
        )?;
        container.type_param_names.is_empty().then(Vec::new)
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
        if matches!(formal, Ty::Unknown | Ty::Void) {
            return;
        }
        // The void-operand face rides `checkForArgumentTypeNotAssignable`'s
        // `checkForUseOfVoidResult` short-circuit (upstream
        // `error_detection_helpers.dart:49-50`): `3 + v()` reports
        // `use_of_void_result` at the operand and answers no
        // `argument_type_not_assignable`. The `dynamic` operand still
        // suppresses — `isAssignableTo(dynamic, X)` holds upstream, and
        // the const-eval verdict owns the row. The spelled check folds the
        // typedef-`void` faces the same way [`Self::emit_use_of_void
        // _result`] does.
        if self.is_void_spelling(self.fold_alias_ty(right_ty)) {
            self.emit_use_of_void_result(right_ty, right);
            return;
        }
        if matches!(right_ty, Ty::Unknown | Ty::Dynamic) {
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
        if !self
            .switch
            .pattern_stack
            .iter()
            .skip(1)
            .all(|f| f.binary.is_some())
        {
            return None;
        }
        self.switch.switches.last().map(|ctx| ctx.scrutinee)
    }

    /// [`Self::pattern_matched_ty`] narrowed by the space the PREVIOUS
    /// cases consumed — the face `checkPatternNeverMatchesValueType`'s
    /// kinds ask (`case Null _` after `case null:` over `int?` reports
    /// "matched value type 'int'", resolver.dart:624-653 over the per-case
    /// matched type): a nullable named scrutinee whose earlier unguarded
    /// cases all covered exactly the null space reads its non-null
    /// projection at this case. Wider or guarded coverage keeps the
    /// declared face (the under-report posture for a space the name level
    /// cannot subtract). The CONSTANT kind stays on the declared face —
    /// its verifier never runs on a clause already covered (#3520's
    /// dead-clause law), so the narrowing must not reach it.
    fn pattern_matched_ty_narrowed(&self) -> Option<Ty> {
        let ctx = self.switch.switches.last()?;
        let scrutinee = self.pattern_matched_ty()?;
        let null_covered = !ctx.cases.is_empty()
            && ctx
                .cases
                .iter()
                .all(|case| !case.guarded && case.space == Restriction::Null);
        Some(match scrutinee {
            Ty::Named {
                name,
                nullable: true,
                args,
            } if null_covered => Ty::Named {
                name,
                nullable: false,
                args,
            },
            scrutinee => scrutinee,
        })
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
        self.boundary
            .args
            .borrow_mut()
            .named(head, false, &[element])
    }

    /// `checkPatternNeverMatchesValueType`'s Map kind (upstream Dart SDK
    /// source, `resolveMapPattern` at pkg/analyzer/lib/src/generated/
    /// resolver.dart:1599-1640, fed by `analyzeMapPattern`,
    /// pkg/_fe_analyzer_shared/lib/src/type_inference/type_analyzer.dart:
    /// 1257-1295): the required type is `Map<keyType, valueType>` — the
    /// matched map's own type arguments when `matchMapType` answers (a
    /// matched `Map` can never be disjoint from its own shape, so the row
    /// only fires against a non-`Map` matched face), `Map<dynamic,
    /// dynamic>`/`Map<InvalidType, InvalidType>` on the dynamic/invalid
    /// faces (both `canBeSubtypeOf`-satisfiable anyway), and
    /// `Map<Object?, Object?>` on every other face — the only required
    /// shape the verdict can ever render.
    ///
    /// Only the type-argument-free spelling runs here: a written `<K, V>`
    /// is not carried into a resolved pattern type at parse time, so the
    /// row's required-type rendering could not say `Map<K, V>` — the
    /// under-report direction, same posture as the List kind.
    ///
    /// `Some((at, required))` is the deferred candidate (the node is a
    /// container field's sub-pattern, so its matched type is that column).
    pub(crate) fn check_map_pattern(&mut self, at: SourceRange) -> Option<(SourceRange, Ty)> {
        let required = self.map_pattern_required_ty();
        let Some(matched) = self.pattern_matched_ty() else {
            if !self.pattern_reads_container_column() {
                return None;
            }
            return Some((at, required));
        };
        if matched == Ty::Unknown || self.pending_pattern_type_args.is_some() {
            return None;
        }
        self.judge_pattern_never_matches(at, required, matched);
        None
    }

    /// The Map kind's required type: `Map<Object?, Object?>` (see
    /// [`Self::check_map_pattern`]).
    fn map_pattern_required_ty(&mut self) -> Ty {
        let object = Self::nullable_form(Ty::named(self.intern("Object")));
        let head = self.intern("Map");
        self.boundary
            .args
            .borrow_mut()
            .named(head, false, &[object, object])
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
        let required = self
            .boundary
            .args
            .borrow_mut()
            .record(&slots, &names, false);
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
    fn record_pattern_disjoint(&self, matched: Ty, positional: usize, named: &[StringId]) -> bool {
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
        let null_face = matched == Ty::Null
            && matches!(
                cast,
                Ty::Named {
                    nullable: false,
                    ..
                }
            );
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
        let Some(matched) = self.pattern_matched_ty_narrowed() else {
            return self
                .pattern_reads_container_column()
                .then_some((at, required));
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
        // The inferred-const candidate (#3632): an operand whose
        // parse-time type never resolved (`const c1 = cond ? true : null;
        // … const c4 = c1 ? 1 : 2;` — the const local's initializer-
        // derived `bool?` is the flow's answer, a face the parse-time
        // static type does not spell). A bare local read stages for the
        // teardown join: the flow's condition-type row at the read
        // decides, and no row re-decides on the opaque unknown — the
        // silent posture every unresolvable candidate holds.
        if ty == Ty::UNKNOWN
            && !self.bool_decided_covers(row)
            && self.local_hits.contains_key(&row.range.offset)
        {
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

    /// The full extent of the operand that FOLLOWS the operator ending at
    /// `op_end` — from the first token past the operator to the end of
    /// the balanced expression it opens. A member-invocation operand
    /// (`e.same(x)`, `const Equality<Apple>().same(x)`) leaves its
    /// SELECTOR row on top of the row stack (the send stages no composite
    /// row of its own), a span that neither abuts the operator nor covers
    /// the argument group — the operand's own extent is only readable
    /// from the tokens: bracket groups are consumed whole
    /// (`tokens.matching_end`), and a depth-0 terminator (`;` `,` `&&`
    /// `||` `??` `?` `:` `=>` or an enclosing closer) ends the walk.
    /// [`Self::junction_operand`] then finds the operator adjoining the
    /// extent's start, so the operand reports `non_bool_operand` anchored
    /// at its whole span — the oracle's `.at(operand)`
    /// (binary_expression_resolver.dart:81-92, probed dart analyze
    /// 3.13.7: `is && const Equality<Apple>().same(…)` anchors the 58-col
    /// send, not the 4-col selector).
    pub(crate) fn operand_extent_after(&self, op_end: u32, fallback: SourceRange) -> SourceRange {
        use dartq_scanner::TokenKind as TK;
        let tokens = self.tokens.as_slice();
        let mut idx = tokens.partition_point(|t| t.utf16_offset < op_end);
        let Some(first) = tokens.get(idx) else {
            return fallback;
        };
        let start = first.utf16_offset;
        let mut end = self.utf16_of(first.byte_offset as usize + first.byte_len as usize);
        while let Some(token) = tokens.get(idx) {
            match token.kind {
                TK::OpenParen | TK::OpenSquare | TK::OpenCurly => {
                    match self.tokens.matching_end(idx) {
                        Some(close) => {
                            let closer = tokens[close];
                            end = self.utf16_of(closer.byte_offset as usize + closer.byte_len as usize);
                            idx = close + 1;
                            continue;
                        }
                        None => return fallback,
                    }
                }
                // An enclosing group's closer or a same-level separator
                // ends the operand (`x && y)` / `x && y, z` / the next
                // junction `x && y || z`).
                TK::CloseParen
                | TK::CloseSquare
                | TK::CloseCurly
                | TK::Semicolon
                | TK::Comma
                | TK::AmpersandAmpersand
                | TK::BarBar
                | TK::QuestionQuestion
                | TK::Question
                | TK::Colon
                | TK::Function => break,
                _ => {}
            }
            end = self.utf16_of(token.byte_offset as usize + token.byte_len as usize);
            idx += 1;
        }
        SourceRange {
            offset: start,
            length: end - start,
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
                .filter(|index| matches!(tokens[*index].kind, dartq_scanner::TokenKind::CloseParen))
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
            Some(ty) if ty != Ty::UNKNOWN && !self.is_void_spelling(self.fold_alias_ty(ty)) => {}
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
        (element.is_some_and(|element| self.is_void_spelling(self.fold_alias_ty(element))))
            .then_some(range)
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
/// Future<int>` awaiting to `int`) lives one level up: the name level
/// carries no instantiated supertype walk, so [`Engine::flatten`] adds it
/// beside the type-parameter arm. This helper answers only what a
/// written head decides.
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

/// The first top-level `else` keyword in `[start, end)`: a depth walk
/// over `()[]{}` with string contents skipped, word-bounded both sides —
/// the if-element leaf's then/else split. A nested `if`'s own `else`
/// (an inner control-flow element inside a branch) lives inside the
/// branch's own brackets or belongs to the nearest if, so the FIRST
/// top-level hit is the split the outer element owns.
fn value_top_level_else(bytes: &[u8], start: usize, end: usize) -> Option<usize> {
    let end = end.min(bytes.len());
    let mut i = start;
    let mut depth = 0usize;
    while i < end {
        match bytes[i] {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => depth = depth.saturating_sub(1),
            b'e' if depth == 0
                && bytes[i..].starts_with(b"else")
                && (i == start || !ident_byte(bytes[i - 1]))
                && bytes.get(i + 4).is_none_or(|b| !ident_byte(*b)) =>
            {
                return Some(i);
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

/// An identifier byte (Dart identifiers are `[a-zA-Z0-9_$]` — the
/// official scanner spells them ASCII-only): the keyword scans' word
/// boundary.
fn ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'$'
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
    pub(crate) fn disallowed_subtype_head(&self, entry: StringId) -> Option<StringId> {
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
            let Some(site) =
                self.decl_sites.iter().rev().find(|site| {
                    matches!(site.kind, DeclKind::Container) && site.name == decl_name
                })
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
                        (
                            SemanticCode::SubtypeOfDisallowedTypeInImplements,
                            *interface,
                        )
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
                        (
                            SemanticCode::SubtypeOfDisallowedTypeInImplements,
                            *interface,
                        )
                    }));
                }
                ContainerKind::Mixin => {
                    entries.extend(container.on_clause.iter().map(|constraint| {
                        (SemanticCode::SubtypeOfDisallowedTypeInOn, *constraint)
                    }));
                    entries.extend(container.implements_clause.iter().map(|interface| {
                        (
                            SemanticCode::SubtypeOfDisallowedTypeInImplements,
                            *interface,
                        )
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
            let mut ordinals: FxHashMap<(SemanticCode, StringId), usize> = FxHashMap::default();
            for (code, entry) in entries {
                // `Enum` stays blanket-silent in this table on every
                // clause face: the oracle's `Enum` row is the separate
                // `concrete_class_has_enum_superinterface` code (probed
                // 3.13.7: `class A implements Enum {}` reports
                // CONCRETE_CLASS_HAS_ENUM_SUPERINTERFACE over the written
                // name, while mixins, abstract classes, and enums stay
                // silent) — a code this engine does not spell, so
                // `disallowed_subtype_head` filters the spelling.
                let restricted = self.disallowed_subtype_head(entry).is_some();
                if restricted {
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
                        let (span, next) = self.scan_clause_type_at(head_at, entry_text);
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

/// The #3821 `unnecessary_type_check` precision pins: the oracle's single
/// report gate (`_checkAllTypeChecks`, best_practices_verifier.dart:877-930
/// — report `true`/`false` only through `TypeSystem.isSubtypeOf`) and the
/// shapes the gate must stay silent on. The `#[ignore]`d cases are the
/// measured over-reports whose root cause is type-argument substitution,
/// not the gate itself — see the module docs.
#[cfg(test)]
#[path = "ticket_3821_tests.rs"]
mod ticket_3821_tests;

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
    /// close; a plain `Future` binding is untouched. The message spells
    /// the receiver's instantiated static type: oracle 3.13.7 reports
    /// `The getter 'isEven' isn't defined for the type 'Future<DB>'.`
    /// for this fixture (the pin previously expected the bare `'Future'`
    /// — a spelling fitted to the un-instantiated face).
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
            "The getter 'isEven' isn't defined for the type 'Future<DB>'."
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
