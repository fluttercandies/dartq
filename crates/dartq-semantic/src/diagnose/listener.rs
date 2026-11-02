use super::*;

/// The deferred `pattern_never_matches_value_type` candidates of one open
/// pattern frame (#3242). A container field's sub-pattern is judged against
/// that field's own column, and a field's column is a property of the
/// container's head — which arrives only at the container's close
/// (`handle_object_pattern`), long after the sub-pattern's check ran. The
/// candidates therefore park here, keyed to the frame whose field they sit
/// in, and the object close re-judges them per field
/// ([`Engine::judge_pattern_never_matches`] over the resolved column).
///
/// The stack runs in lockstep with `SwitchTracker::pattern_stack`: one
/// entry pushed by `begin_pattern`, popped by `end_pattern`. A container
/// kind that cannot turn a column back into a type (record, list, map)
/// simply drops its buckets when the frame pops.
pub(crate) struct PatternNodeState {
    /// The record pattern's `(` token, when this frame IS a record pattern's
    /// own bracket. The record row spans the whole `(...)` NODE (the official
    /// `.at(pattern)`), whose closer is resolved through the scanner's group
    /// table ([`Engine::group_span`]) — the frame's own `end_pattern` token
    /// runs past the close whenever the pattern carries a `||`/`&&` operand
    /// or a postfix.
    pub(crate) pending_record_open: Option<Token>,
    /// Candidates recorded since the last field boundary of this frame —
    /// the field sub-pattern currently parsing.
    pub(crate) pending: Vec<(SourceRange, Ty)>,
    /// One bucket per `handle_pattern_field` boundary, index-aligned with the
    /// frame's drained `ObjectField`s (both append once per boundary).
    pub(crate) fields: Vec<Vec<(SourceRange, Ty)>>,
    /// A map pattern's evaluated entry keys, in source order: the captured
    /// key expression plus the key's own span. `visitMapPattern` keeps the
    /// same list in its `uniqueKeys` table and reports
    /// `equal_keys_in_map_pattern` at the LATER key whose value equals an
    /// earlier one (`constant_verifier.dart:313-355`), so the frame
    /// accumulates per entry and the map's close drains.
    pub(crate) map_keys: Vec<(crate::const_eval::Node, SourceRange)>,
}

impl PatternNodeState {
    fn new() -> Self {
        Self {
            pending_record_open: None,
            pending: Vec::new(),
            fields: Vec::new(),
            map_keys: Vec::new(),
        }
    }
}

/// The potentially-constant scan face of a const constructor's field
/// initializer (`_reportNotPotentialConstants` over `x = e` initializer
/// entries, #3400): the const constructor's formal parameters bound opaque
/// (potentially constant), with two verdicts the shared engine face does
/// not carry —
///
/// * a bare instance-member spelling (`f = g` over instance `g`): the name
///   resolves through implicit `this`, which is never a constant — the
///   oracle's `_Collector` reports `invalid_constant` at the read (and the
///   implicit-`this` row reports its own
///   `implicit_this_reference_in_initializer`). A static member keeps the
///   declarator-keyed verdict (`f = s` over `static const s` stays
///   silent);
/// * a wildcard formal (`_`): wildcards declare no name the initializer
///   can read, so `ctor_init_formals` drops them before the env is built —
///   `assert(_ > 0)` classifies `_` unresolved and reports
///   `invalid_constant` beside the row's `undefined_identifier`.
///
/// Every other leaf defers to the engine's own classification, so the
/// scan stays the single gate the shared scan sites run.
struct InitRhsLookup<'a, 'e> {
    engine: &'e Engine<'a>,
    env: &'e FxHashMap<StringId, CtorBinding>,
}

impl ConstLookup for InitRhsLookup<'_, '_> {
    fn ident(&self, name: StringId) -> IdentVerdict {
        if let Some(binding) = self.env.get(&name) {
            return match binding {
                CtorBinding::Value(v) => IdentVerdict::Value(v.clone()),
                CtorBinding::Opaque => IdentVerdict::Constant,
                CtorBinding::Invalid => IdentVerdict::NotConstant,
            };
        }
        if let (BareInfo::Plain { .. }, RefTarget::Member { container }) =
            self.engine.resolve_bare(name)
            && let Walk::Found(entry) = self.engine.walk_members(container, name)
        {
            return if entry.is_static {
                self.engine.ident(name)
            } else {
                IdentVerdict::NotConstant
            };
        }
        self.engine.ident(name)
    }

    fn member(&self, recv: &ConstantValue, name: StringId) -> Verdict {
        self.engine.member(recv, name)
    }

    fn prefixed(&self, prefix: StringId, name: StringId) -> Option<IdentVerdict> {
        self.engine.prefixed(prefix, name)
    }

    fn is_identical(&self, name: StringId) -> bool {
        self.engine.is_identical(name)
    }

    fn created_state(&self, container: StringId) -> crate::const_eval::CreatedState {
        self.engine.created_state(container)
    }

    fn shorthand_context(&self, span: SourceRange) -> bool {
        self.engine.shorthand_context(span)
    }
}

/// A record pattern's required-type shape: the positional field count and the
/// named field set (`analyzeRecordPattern`'s `recordType(positional:
/// List.filled(positionalCount, objectQuestionType), named: ...)`, pkg/
/// _fe_analyzer_shared/lib/src/type_inference/type_analyzer.dart:1841-1855).
/// A duplicate name keeps its first occurrence only — the official excludes
/// the later slots (`_reportDuplicateRecordPatternFields`).
fn record_field_shape(fields: &[ObjectField]) -> (usize, Vec<StringId>) {
    let mut positional = 0usize;
    let mut named: Vec<StringId> = Vec::new();
    for field in fields {
        match field.name {
            None => positional += 1,
            Some(name) => {
                if !named.contains(&name) {
                    named.push(name);
                }
            }
        }
    }
    (positional, named)
}

/// The frame-local function tear-off capture a bare read resolves to
/// (#2852), innermost frame first: a frame whose `local_fn_sigs` holds the
/// name answers the #2842 capture (joined at the declaring frame's close),
/// and a plain local of the same spelling shadows every outer one — the
/// read is that local's own value, the flow's own scope resolution answers
/// it, never a staged row. The listener-side mirror of the capture walk:
/// the frame stack is listener-visible, so the resolution needs no
/// engine-side seam.
fn local_fn_tear_off_sig(engine: &Engine<'_>, name: StringId) -> Option<super::types::FnVarSig> {
    for frame in engine.frames.iter().rev() {
        if let Some(sig) = frame.local_fn_sigs.get(&name) {
            return Some((**sig).clone());
        }
        if frame.locals.contains(&name) {
            return None;
        }
    }
    None
}

/// The relational pattern's nullable-matched verdict (#2843). The matched
/// type is the open switch's scrutinee exactly when every frame above the
/// root is a logical-or/and operand (`PatternFrame::binary` — the operands
/// keep the scrutinee as their matched type); record, list, map, and
/// object field frames carry property columns the name level never
/// descends into, and a pattern under one stays silent (the under-report
/// direction). `==`/`!=` never report: both spell `operator ==`, which
/// every type resolves on `Object` (type_property_resolver.dart's
/// Object-lookup arm). An applicable extension operator resolves the read
/// silent (the `operator >`-on-`int?` pin); an `on int` face under a
/// nullable receiver is the oracle's not-applicable arm for the pattern
/// face (`int?` is not a subtype of `int`) — the read stays unchecked, the
/// `UnsafeSingle` verdict. The row anchors at the operator token, its
/// length the lexeme's own width.
impl DiagnoseListener<'_> {

    /// The WRITTEN extent of the map-pattern key that ends at `colon`: the
    /// first token after the entry's opening `{`/`,` through the token
    /// before the `:`. The oracle anchors both the key rows
    /// (`non_constant_map_pattern_key`'s `_evaluateAndReportError` entity
    /// and `equal_keys_in_map_pattern`'s `.at(duplicateEntry.key)`) at the
    /// KEY NODE, whose span is the expression's own — a literal, a
    /// creation, a record, a member access. The capture root cannot supply
    /// it: a record literal's value node anchors at its closer, and the
    /// recovery shapes carry no span at all.
    fn map_pattern_key_span(&self, colon: Token) -> SourceRange {
        let toks = self.engine.tokens.as_slice();
        let end = self.engine.token_at_offset(colon.range.offset);
        let mut start = end;
        let mut depth = 0i32;
        while start > 0 {
            let kind = toks[start - 1].kind;
            match kind {
                dartq_scanner::TokenKind::CloseParen
                | dartq_scanner::TokenKind::CloseSquare
                | dartq_scanner::TokenKind::CloseCurly => depth += 1,
                dartq_scanner::TokenKind::OpenParen
                | dartq_scanner::TokenKind::OpenSquare
                | dartq_scanner::TokenKind::OpenCurly => {
                    if depth > 0 {
                        depth -= 1;
                    } else if kind == dartq_scanner::TokenKind::OpenCurly {
                        // The entry's own brace: the key starts after it.
                        break;
                    }
                }
                dartq_scanner::TokenKind::Comma | dartq_scanner::TokenKind::Colon
                    if depth == 0 =>
                {
                    break;
                }
                _ => {}
            }
            start -= 1;
        }
        let (Some(first), Some(last)) = (toks.get(start), end.checked_sub(1).and_then(|i| toks.get(i)))
        else {
            return colon.range;
        };
        if start >= end {
            return colon.range;
        }
        SourceRange {
            offset: first.utf16_offset,
            length: (last.utf16_offset + last.byte_len as u32).saturating_sub(first.utf16_offset),
        }
    }
    /// Parks a never-match candidate whose matched type is the enclosing
    /// container's column (the `Some` face of
    /// [`Engine::check_pattern_never_matches`]) on the CONTAINER frame: the
    /// node being checked is that frame's field sub-pattern, so the
    /// container is the second frame from the top (`pattern_rows` runs in
    /// lockstep with the pattern stack).
    fn defer_pattern_row(rows: &mut [PatternNodeState], row: Option<(SourceRange, Ty)>) {
        let Some((at, required)) = row else {
            return;
        };
        if let Some(state) = rows.len().checked_sub(2).and_then(|i| rows.get_mut(i)) {
            state.pending.push((at, required));
        }
    }

    /// The record pattern's close: `handle_record_pattern` latched the `(`,
    /// and the row spans the whole `(...)` node — the official
    /// `.at(pattern)` on the `RecordPattern` node
    /// (`checkPatternNeverMatchesValueType` falls through to the pattern
    /// itself, pkg/analyzer/lib/src/generated/resolver.dart:642). The closer
    /// comes from the scanner's group table rather than the frame's end
    /// token, which also covers a `||`/`&&` operand or a postfix.
    fn close_record_pattern(&mut self) {
        let Some(open) = self
            .pattern_rows
            .last_mut()
            .and_then(|state| state.pending_record_open.take())
        else {
            return;
        };
        let Some(frame) = self.engine.switch.pattern_stack.last() else {
            return;
        };
        let (positional, named) = record_field_shape(&frame.fields);
        // An unclosed group (recovery) keeps the `(` itself: the row anchors
        // at the opener instead of the recovered node.
        let at = self.engine.group_span(open).unwrap_or(open.range);
        self.engine.check_record_pattern(at, positional, &named);
    }

    /// The per-field column face of a closing object pattern (#3242): each
    /// field's deferred candidates are judged against the property column the
    /// field reads ([`Engine::object_pattern_binder_ty`] — the official's
    /// `resolveObjectPatternPropertyGet` receiver face, type_analyzer.dart:
    /// 1674-1697). A field whose getter stays unresolvable, and a positional
    /// field with no sub-pattern variable, keep the under-report.
    fn judge_object_field_rows(&mut self, head: Option<StringId>, fields: &[ObjectField]) {
        let Some(head) = head else {
            return;
        };
        let buckets = match self.pattern_rows.last_mut() {
            Some(state) => std::mem::take(&mut state.fields),
            None => return,
        };
        for (field, rows) in fields.iter().zip(buckets) {
            if rows.is_empty() {
                continue;
            }
            let Some((getter, _)) = field.getter else {
                continue;
            };
            let Some(column) = self.engine.object_pattern_binder_ty(head, getter) else {
                continue;
            };
            for (at, required) in rows {
                self.engine
                    .judge_pattern_never_matches(at, required, column);
            }
        }
    }

    fn check_relational_pattern_nullable(&mut self, operator: Token) {
        let engine = &mut self.engine;
        if !engine
            .switch
            .pattern_stack
            .iter()
            .skip(1)
            .all(|frame| frame.binary.is_some())
        {
            return;
        }
        let scrutinee = engine
            .switch
            .switches
            .last()
            .map_or(Ty::Unknown, |ctx| ctx.scrutinee);
        if !engine.potentially_nullable(scrutinee) {
            return;
        }
        let Some(op_text) = engine.text_of(operator) else {
            return;
        };
        if op_text == "==" || op_text == "!=" {
            return;
        }
        // The extension walk runs on the alias-folded face: `typedef A =
        // int?` reaches `int`'s extensions through `int`, never through
        // the alias spelling. A record/function/unknown face keeps the
        // under-report posture.
        let face = engine.fold_alias_ty(scrutinee);
        let Ty::Named { name, .. } = face else {
            return;
        };
        let op_id = engine.intern(op_text);
        if !matches!(
            engine.extension_application(name, face, op_id, false, true),
            ExtensionApp::None | ExtensionApp::UnsafeSingle,
        ) {
            return;
        }
        // The operator-face rendering (messages.yaml
        // `uncheckedOperatorInvocationOfNullableValue`, the shared
        // `unchecked_use_of_nullable_value` name): the exact oracle text,
        // built at the site — the shared helper renders the
        // property/method variants only.
        let code = SemanticCode::UncheckedUseOfNullableValue;
        engine.report.diagnostics.push(Diagnostic::new(
            code.severity(),
            code.code_info(),
            format!(
                "The operator '{op_text}' can't be unconditionally invoked \
                 because the receiver can be 'null'."
            ),
            engine.file,
            operator.range,
            Some("Try adding a null check to the target ('!').".to_string()),
        ));
    }

    /// The relational pattern's operand consult (#2917): the matched type
    /// and the operator lexeme under the shared window — the open
    /// switch's scrutinee exactly when every frame above the root is a
    /// logical-or/and operand (the same posture the nullable twin
    /// documents; a pattern under a record/list/map/object field frame
    /// carries a property column the name level never descends into), and
    /// `==`/`!=` excluded (both spell `operator ==`, which every type
    /// resolves on `Object`).
    fn relational_pattern_operand(&self, operator: Token) -> Option<(Ty, String)> {
        let engine = &self.engine;
        let all_logical = engine
            .switch
            .pattern_stack
            .iter()
            .skip(1)
            .all(|frame| frame.binary.is_some());
        let scrutinee = engine
            .switch
            .switches
            .last()
            .map_or(Ty::Unknown, |ctx| ctx.scrutinee);
        // The rest-element's sub-pattern (`case [...< 0]`) is the one
        // non-logical nest whose matched type is still computable: it is
        // the list the rest collects into — `List<E>` of the scrutinee's
        // element type — never a property column (upstream
        // `RestPatternElement`'s sub-pattern matched type,
        // type_analyzer.dart's list-pattern element walk). The `...`
        // marker is the token directly before the operator in source —
        // inside a pattern it only ever spells a rest element, so a
        // `...` immediately ahead of the relational (modulo trivia) names
        // this shape exactly and admits it through the logical-only gate.
        let scrutinee = if all_logical {
            scrutinee
        } else {
            let bytes = engine.source.as_bytes();
            let mut cursor = operator.range.offset as usize;
            while cursor > 0 && bytes[cursor - 1].is_ascii_whitespace() {
                cursor -= 1;
            }
            if cursor < 3 || &bytes[cursor - 3..cursor] != b"..." {
                return None;
            }
            // The rest's own list: `List<E>` where `E` is the scrutinee's
            // element argument (an `Iterable`-face's written first arg), or
            // `List<dynamic>` when the scrutinee hands no decidable element
            // (a `dynamic`/unknown matched value — the oracle's
            // `List<dynamic>` row on `void f(x)`).
            let element = match engine.fold_alias_ty(scrutinee) {
                Ty::Named { args, .. } if args != crate::model::ArgListId::EMPTY => {
                    engine.boundary.pool().get(args).first().copied().unwrap_or(Ty::Dynamic)
                }
                _ => Ty::Dynamic,
            };
            let list = engine.intern("List");
            engine.boundary.args.borrow_mut().named(list, false, &[element])
        };
        let op_text = engine.text_of(operator)?.to_owned();
        if op_text == "==" || op_text == "!=" {
            return None;
        }
        Some((scrutinee, op_text))
    }

    /// The relational pattern's operator-definedness face (`case > 0` over
    /// a class without `>`): the matched type answers the operator's
    /// definedness, a miss reports `undefined_operator` at the pattern's
    /// operator token (upstream Dart SDK source,
    /// pkg/analyzer/lib/src/generated/resolver.dart:1696-1731 —
    /// `resolveRelationalPatternOperator` feeds `typePropertyResolver
    /// .resolve` the matched type with `propertyErrorEntity: node.operator`,
    /// and `needsGetterError` reports `undefinedOperator` there).
    /// dynamic/void/unknown faces dispatch dynamically — silent. An
    /// applicable extension operator resolves the consult silent, and a
    /// scope-parameter matched value answers through its written bound
    /// (the `resolveToBound` consult — the same composition the shared
    /// operator-definedness face runs for the expression twins,
    /// #2909's `adjacent_operator_defined`).
    fn check_relational_pattern_operator(&mut self, operator: Token) {
        let Some((scrutinee, op_text)) = self.relational_pattern_operand(operator) else {
            return;
        };
        let engine = &mut self.engine;
        // The extension walk runs on the alias-folded face (the nullable
        // twin's posture): `typedef A = C` reaches `C`'s extensions through
        // `C`, never through the alias spelling.
        let face = engine.fold_alias_ty(scrutinee);
        let probe = engine.intern(&op_text);
        let defined = match &face {
            Ty::Named { name, .. } => {
                matches!(engine.walk_members(*name, probe), Walk::Found(_))
                    || Self::relational_bound_defines(engine, *name, probe)
                    || matches!(
                        engine.extension_application(*name, face, probe, false, false),
                        ExtensionApp::Single | ExtensionApp::UnsafeSingle
                    )
            }
            // A record or function face declares no relational operator —
            // the same verdict the applied-operator face reports with the
            // full render.
            Ty::Record { .. } | Ty::Function { .. } => false,
            _ => true,
        };
        if defined {
            return;
        }
        let type_text = match &face {
            Ty::Record { .. } | Ty::Function { .. } => {
                face.render(&engine.strings, &engine.boundary.pool())
            }
            Ty::Named { name, .. } => engine.strings.resolve(*name).to_owned(),
            _ => return,
        };
        engine.emit(
            SemanticCode::UndefinedOperator,
            operator.range,
            &op_text,
            &type_text,
        );
    }

    /// The scope-parameter bound's operator face (the upstream
    /// `resolveToBound` consult — binary_expression_resolver.dart:461): a
    /// parameter-typed matched value resolves the pattern's operator
    /// through its written bound (`T extends num` keeps `case > 0`
    /// silent). The captured windows survive the pass-2 frame teardown
    /// ([`Engine::param_bound_of`]); a class-generic parameter stays
    /// unmodeled (`None` — the under-report direction).
    fn relational_bound_defines(engine: &Engine<'_>, name: StringId, probe: StringId) -> bool {
        let Some(bound) = engine
            .param_bound(name)
            .or_else(|| engine.param_bound_of(name))
        else {
            return false;
        };
        let Ty::Named { name, .. } = engine.fold_alias_ty(bound) else {
            return false;
        };
        matches!(engine.walk_members(name, probe), Walk::Found(_))
    }

    /// The increment/decrement operand's index-write marking (`a[0]++`):
    /// the operand resolves for write with read (upstream Dart SDK source,
    /// pkg/analyzer/lib/src/dart/resolver/postfix_expression_resolver.dart
    /// :35-37 `resolveForWrite(hasRead: true)` and the prefix twin at
    /// prefix_expression_resolver.dart:50), so the index access needs BOTH
    /// spellings — the write state the assignment junction's compound face
    /// sets on the `[]` operator row (`mark_assignment`'s index arm).
    /// Without it the `[]`/`[]=` both-missing pair stays half-silent
    /// (`operator_spellings` answers the read spelling only). The
    /// operand's folded stand-in row (the `]` token) sits directly above
    /// the `[]` operator row; any other operand shape carries no index
    /// write (an unfolded bracket — a group receiver — materialized no
    /// operator row, and a nested `a[0][1]++` writes through a result the
    /// dynamic suppression covers).
    fn mark_index_compound_write(&mut self) {
        let engine = &mut self.engine;
        let index_op = engine.intern("[]");
        let Some(mut top) = engine.rows.len().checked_sub(1) else {
            return;
        };
        while matches!(engine.rows[top].shape, RowShape::TypeUse(..)) {
            top = match top.checked_sub(1) {
                Some(next) => next,
                None => return,
            };
        }
        if !matches!(engine.rows[top].shape, RowShape::Value) {
            return;
        }
        let Some(below) = top.checked_sub(1) else {
            return;
        };
        let stand_in = engine.rows[top].range;
        let (hit, bracket_offset) = match &engine.rows[below].intent {
            Intent::Operator(op, bracket) => (
                *op == index_op
                    && bracket.offset <= stand_in.offset
                    && stand_in.end() <= bracket.end(),
                bracket.offset,
            ),
            _ => (false, 0),
        };
        if hit {
            engine.rows[below].write_op = Some(OpWrite::Compound);
            // The compound index's argument check (`++a[i]`/`a[i]++`): the
            // `[]`/`[]=` element pair both apply (`hasRead: true`) — the
            // staged operand decides at the bracket (upstream Dart SDK
            // source, `checkIndexExpressionIndex`,
            // error_detection_helpers.dart:260-289).
            engine.check_index_argument(bracket_offset, Some(OpWrite::Compound));
        }
    }
}

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
        // The container's own static-vs-instance collisions report at the
        // enum's body close, while the frame is still on the stack — the
        // enum arm of the same `check_conflicting_static_and_instance`
        // `end_class_or_mixin_or_extension_body` runs for classes/mixins/
        // extensions. Upstream `_checkUnitStatic` dispatches
        // `_checkEnumStatic` for an `EnumDeclarationImpl`, driven from
        // `visitEnumDeclaration` at body close (upstream Dart SDK source,
        // pkg/analyzer/lib/src/error/member_duplicate_definition_verifier.dart:565).
        engine.check_conflicting_static_and_instance();
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
        // The extension-type's own static-vs-instance collisions report at
        // its body close, while the frame is still on the stack — upstream
        // `_checkUnitStatic` routes an `ExtensionTypeDeclarationImpl`
        // through `_checkClassStatic` (the class member face), driven from
        // `visitExtensionTypeDeclaration` at body close (upstream Dart SDK
        // source,
        // pkg/analyzer/lib/src/error/member_duplicate_definition_verifier.dart:569-571).
        engine.check_conflicting_static_and_instance();
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
        self.flow()
            .begin_class_or_mixin_or_extension_body(kind, token);
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
        // The function form is a first-class capture fact: its columns
        // open their own level in every open window (the shared sink
        // folds them into a function tree), so a written
        // `NativeFunction<Handle Function()>` argument survives to its
        // consumers. The poison belt stays reserved for the shapes the
        // model cannot represent (records, headless groups).
        self.engine.bounds.begin_function();
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
        // A record that OPENS the clause's `on` head (`on (A, B)` — the
        // head is a paren token with no name reference, so no
        // `TypeReference` ever claims it) is the catch-all face: upstream's
        // resolved `exceptionType` is never `isDartCoreObject` here, so the
        // clause must keep `on_type: None` while its field types' references
        // go about their normal rows. The guard reads the head window: the
        // clause is open, no parameter was consumed, and no name head has
        // claimed the clause yet. A record type inside the clause BODY
        // (`on String { var (A, B) x = …; }`) misses the third leg, and a
        // body record after an already-record head misses the latch itself.
        if self.engine.catch_clause_start.is_some()
            && self.engine.catch_params == 0
            && self.engine.catch_on_type.is_none()
            && !self.engine.catch_on_record
        {
            self.engine.catch_on_record = true;
        }
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
        // The written record type's extent: `(` through the matching `)`
        // (the parser guarantees the closer, real or recovery-synthetic),
        // extended by the trailing `?` when it belongs to the type — the
        // same bracket end the is-check verdict's span takes (`(1, 2) is
        // (int, int)` ends at the type's `)`).
        let closer_end = question.map_or_else(
            || {
                engine
                    .tokens
                    .matching_end(left_paren.id as usize)
                    .and_then(|close| engine.tokens.get(close))
                    .map_or(left_paren.range.end(), |t| {
                        engine.utf16_of((t.byte_offset + t.byte_len) as usize)
                    })
            },
            |q| q.range.end(),
        );
        engine.written_ty_span = Some(SourceRange {
            offset: left_paren.range.offset,
            length: closer_end - left_paren.range.offset,
        });
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
        // A list under a captured function level is the shape's own formal
        // region (the return/formal split latches here; the sink guards
        // itself to function levels).
        self.engine.bounds.function_formals_begin();
        // The #1351 capture: the open typedef RHS's formals region opens —
        // the trees since the `Function` head become the return type (the
        // head event precedes the return parse).
        if kind == dartq_parser::MemberKind::GeneralizedFunctionType {
            self.engine.bounds.alias_formals_region_open();
            self.engine.fn_type_formal_lists += 1;
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
        // The catch group's re-parse opens its ordinal window: the list's
        // own first/second real formal names are the official AST's
        // exception/trace faces (`handleCatchBlock`'s
        // `catchParameters[0]/[1].name`); nested function-typed formals run
        // at a deeper list and never count.
        if kind == dartq_parser::MemberKind::Catch {
            engine.catch_formal_group = Some(engine.formal_list_depth);
            engine.catch_ord_exc = None;
            engine.catch_ord_trace = None;
        }
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
        end: Token,
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
        if kind == dartq_parser::MemberKind::GeneralizedFunctionType {
            engine.fn_type_formal_lists = engine.fn_type_formal_lists.saturating_sub(1);
            // The written function type's formals closed — the closer is
            // the type's own end when no `?` follows (`end_function_type`
            // carries the keyword and the `?`, never this paren). The
            // event always fires for the same production immediately
            // before `end_function_type`, inner lists before outer, so
            // the latch pairs one-to-one.
            engine.fn_formal_closer_end = Some(end.range.end());
        }
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
        // The catch group's re-parse closed: the second real formal name
        // takes the stack-trace face when the fast pass's own trace slot
        // stayed junk-filled, and the typed-catch recovery (`catch (int
        // e)`) hands the face to the scan's second name (see
        // [`Engine::reconcile_catch_slots`]).
        if kind == dartq_parser::MemberKind::Catch {
            let open = engine.catch_formal_group.take().is_some();
            let exc = engine.catch_ord_exc.take();
            let trace = engine.catch_ord_trace.take();
            if open {
                engine.reconcile_catch_slots(exc, trace);
            }
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
        // The default value's own assignability face (#3251): upstream
        // `_checkForDefaultValueAssignableAtType` (error_verifier.dart —
        // `checkForAssignableExpressionAtType` over the formal's declared
        // type with the assignment reporter, so a `void` value
        // short-circuits to `use_of_void_result` and every other mismatch
        // answers `invalid_assignment`). The formal's written type rides
        // `formal_written` from its annotation bind; an untyped formal
        // infers `dynamic` and stays silent.
        if let Some(to) = self.engine.formal_written {
            self.engine.check_default_value_assignable(to);
        }
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
        // #2937 (3c): the old-style formal's own signature capture — the
        // nested list `(int p)` of `b(int p)` is the formal's parameter
        // shape. The modern `Function(...)` face stashes a `FnVarSig` at
        // `fn_capture_end`; the old-style spelling feeds the same per-formal
        // machinery through a dedicated capture pushed here and sealed at
        // `end_function_typed_formal_parameter` (the element model's formal
        // keeps no callable record, so `fn_param_sigs` is the callee's only
        // formals — upstream's `FunctionType` on the parameter element).
        engine.fn_type_capture.push(FnTypeCapture::default());
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
        self.engine.this_formal_names.clear();
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
        // The accessor face (`get`/`set` head): the return-check host word
        // reads it (upstream `PropertyAccessorElement` answers `isFunction`,
        // error_verifier.dart:164-166).
        engine.open_member_accessor = get_or_set.is_some();
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
            // The punctuation operator head (`>`, `==`, `[]=`) is the
            // declaration's name token — no `MethodDeclaration` identifier
            // event follows it, so the return face latches HERE (an
            // operator is a `MethodElement` upstream, host word `method`:
            // `int operator >(C o) => "s";` reports `... from the method
            // '>'`). The return annotation parses AFTER this event, so the
            // armed slot still holds an earlier construct's type — the
            // container header's `on`/`extends` clause (`extension E on C
            // { void operator +(int i) {} }` armed `C`) — and must not
            // latch: the face starts `unknown`, the written annotation
            // backfills it as it completes ([`Engine::arm_return_span`]),
            // and an untyped operator keeps the `dynamic` face. The
            // `C::OperatorName` arm stays as the identifier-shaped operator
            // twin (`ret.is_none()` keeps the latch single-fire).
            engine.armed_ty = None;
            engine.latch_return_type(ExecKind::Method, spelling, name_range);
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
        // The augment bit rides onto the constructor's site at its head
        // (the `duplicate_constructor` pass skips augment sites).
        engine.ctor_augment_pending = augment_kw.is_some();
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
        // The ctor's own frame depth for the `this`-legality latch: the
        // body brace is noted against the ctor frame only while no closure
        // frame sits above it (`ctor_frame_len` equality), so a braceless
        // closure inside the body cannot fake the initializer window shut
        // and a braced closure inside an initializer cannot open it.
        engine.ctor_frame_len = engine.frames.len();
        engine.ctor_body_open = false;
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
        engine.ctor_body_open = false;
        engine.ctor_frame_len = 0;
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
        // The augment bit rides onto the factory's site at its head — an
        // `augment factory` merges into the selector's fragment chain
        // exactly like an `augment` constructor (the
        // `duplicate_constructor` pass skips augment sites), so it marks
        // the same pending latch `begin_constructor` sets.
        engine.ctor_augment_pending = augment_kw.is_some();
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
                .get(engine.byte_of(head.offset)..engine.byte_of(head.end()))
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
        // The completion staging reads the factory's frame (the container
        // latched as the return at the name event) before it pops — the
        // official walk checks factory constructors only
        // (`resolver.dart`'s `factoryKeyword != null` gate; a generative
        // constructor's body never stages).
        engine.stage_body_completion(None, end);
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
        // declaration, used or not — but only with the wildcard-variables
        // feature enabled for the library (DeadCodeVerifier's
        // visitFunctionDeclaration, dead_code_verifier.dart:72-82: the
        // `_wildCardVariablesEnabled` gate over `library.featureSet`; a
        // `// @dart = 3.4` file treats `_` as an ordinary name and stays
        // silent — the preWildcards census rows).
        if let Some((start, wildcard)) = engine.local_fn_wildcard.pop()
            && wildcard
            && engine.class_modifiers.wildcards_enabled(engine.lib)
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
        self.flow()
            .end_function_name(begin, end, is_function_expression);
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
        // The literal's context type `K` resolves while its marks still
        // hold — the row count the literal opened at is the mark every
        // channel (initializer / assignment / argument / return) keys on.
        let (ctx, ctx_from_arg) = engine.closure_context_ty(token.range.offset);
        engine.closure_capture_begin(token.range.offset, ctx, ctx_from_arg);
        // The completion arm rides the same context: the context
        // function's return arms the literal's frame (the official
        // `imposedType` — the completion warning checks a closure under a
        // typed context at its body brace).
        engine.arm_closure_completion(ctx);
    }

    fn end_function_expression(&mut self, begin: Token, end: Token) {
        self.flow().end_function_expression(begin, end);

        let engine = &mut self.engine;
        let literal_ty = engine.closure_capture_end(end.range.end());
        engine.const_capture.end_opaque_group(end.range);
        // The completion staging reads the literal's frame (the context
        // return armed at the begin, the body brace, the modifiers)
        // before it pops.
        engine.stage_body_completion(None, end);
        // #1314 FAM7: recovery paths without a body event drop the rows.
        engine.drop_default_checks();
        // The literal folds to one expression node upstream: its own
        // rows — every body read, every inner send — belong to the node,
        // not the stack. Drain them into the deferred pass-2 queue (the
        // list-literal and send folds' precedent: the checks they owed
        // already ran against their rows, and the deferred queue still
        // credits their reads and resolves their own misses), so the
        // enclosing single-row faces — `init_mark`'s `rows.len() ==
        // mark + 1` window, `check_assignment`'s single-RHS scan — see
        // the literal's own row alone.
        let mark = engine
            .frames
            .last()
            .map(|frame| frame.row_mark)
            .unwrap_or_else(|| engine.rows.len());
        // The literal's own node end: `end` bounds the head (the `=>`
        // arrow for an expression body), so the body's last completed row
        // supplies the real close — the oracle's FunctionExpression span
        // covers the body (`(x) => 3` ends at `3`, not `>`).
        let interior_end = engine
            .rows
            .iter()
            .skip(mark.min(engine.rows.len()))
            .map(|row| row.range.end())
            .max();
        for row in engine.rows.drain(mark.min(engine.rows.len())..) {
            engine.deferred_miss_rows.push(row);
        }
        engine.pop_frame();
        match literal_ty {
            // A matched context supplied the literal's final type: the row
            // spans the literal (the oracle's function-expression anchor)
            // and carries the inferred `Ty::Function` — the decl
            // initializer, assignment, and argument faces read it like any
            // typed expression.
            Some((offset, ty)) => {
                let lit_end = interior_end.unwrap_or(0).max(end.range.end());
                let range = SourceRange {
                    offset,
                    length: lit_end.saturating_sub(offset),
                };
                engine.closure_literal_starts.insert(offset);
                engine.push_typed_value_row(range, ty);
            }
            None => engine.push_value_row(end),
        }
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
        // The named literal sits in the same expression slots as the
        // anonymous form — the context resolves through the same channels
        // and arms the completion face (the official checks a named
        // function expression as a closure too: `isFunctionDeclaration`
        // is a FunctionDeclaration parent, never a name).
        let (ctx, _) = engine.closure_context_ty(token.range.offset);
        engine.arm_closure_completion(ctx);
    }

    fn end_named_function_expression(&mut self, end: Token) {
        self.flow().end_named_function_expression(end);

        let engine = &mut self.engine;
        // The completion staging reads the literal's frame before it pops
        // (the context return armed at the begin, the body brace).
        engine.stage_body_completion(None, end);
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
        engine.catch_on_record = false;
        engine.catch_on_nullable = None;
        engine.catch_head_end = None;
        engine.catch_params = 0;
        engine.catch_trace_slot = false;
        // The clause fact's end is ITS body block's end: a recovered clause
        // with no block (`on X;` — `expected_body`) must not inherit the end
        // of whatever block closed before it.
        engine.last_block_end = None;
        engine.push_block_frame();
    }

    fn end_catch_clause(&mut self, token: Token) {
        self.flow().end_catch_clause(token);

        // The frame pop moved to `handle_catch_block` (the clause's body
        // block parses between the two events). The peek token opens the
        // body, so its offset is the head's exclusive end — the fact's end
        // only when no body block materialized (`on X;`).
        self.engine.catch_head_end = Some(token.range.offset);
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

        // The pattern-head form's binders (#2858): `for (var (a, b) in xs)`
        // parses its head as a bare pattern — no `PatternVariableDeclaration`
        // wrapper — so the official verifier visits each binder individually
        // (unused_local_elements_verifier.dart:565-579), never the
        // all-or-nothing window the registration opened. `pattern_keyword`
        // is the only event naming the head a pattern one (it arrives after
        // the pattern: the `in` disambiguates the head), and the head's rows
        // are the only tracked rows above the loop frame's mark — a closure
        // in the iterable truncated its own rows at its frame pop. The
        // window dies with the head: a body declaration opens its own.
        if pattern_keyword.is_some() {
            let engine = &mut self.engine;
            if let Some(&lo) = engine.frame_local_marks.last() {
                for index in lo..engine.locals.len() {
                    engine.convert_window_binder_to_site(index);
                }
            }
            engine.pattern_group_open = None;
        }

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
        } else if let (Some(&mark), Some((_, name_tok))) =
            (self.engine.for_in_marks.last(), loop_var.as_ref())
            && let Some((row_ty, _, row_range)) = self.engine.single_row_since(mark)
            // The single-row discipline the parse-time staging holds: a
            // multi-row iterable window (a map-index expression's rows)
            // attributes to no one row, so it stays under-reported.
            && row_ty == Ty::UNKNOWN
        {
            // The flow-blind fallback: the iterable's face resolves only
            // in pass 2 (a member-send return the flow lattice cannot
            // spell), so the loop variable's ELEMENT derivation defers to
            // the `resolve_rows` pre-pass over the initializer row.
            self.engine
                .local_init_rows
                .insert(name_tok.offset, (row_range.offset, true));
        }

        // The irrefutable walk over a pattern head (#3640): the head's
        // declared binders went through the pattern-variable path (never
        // `for_in_loop_var`), so the element-type derivation asks
        // `for_in_element_staging` directly — the same `Iterable` face the
        // identifier-head staging reads. `for_in_marks` still holds the
        // iterable's row mark (the pop below).
        if let Some(kw) = pattern_keyword
            && let Some(&mark) = self.engine.for_in_marks.last()
            && let Some((sem_ty, _)) =
                self.engine.for_in_element_staging(mark, await_kw.is_some())
        {
            self.engine.irrefutable_walk_for_in_head(kw, in_kw, sem_ty);
        }

        // The element-value `void` face (`_checkForEachParts`'s
        // `sequenceElementType is VoidType && variableType is! VoidType`
        // arm, error_verifier.dart:4478-4484): a `void` element's value is
        // consumed by the implicit assignment to the loop variable, which
        // can only discard it into a `void` head — the oracle reports at
        // the ITERABLE expression. The loop variable's declared face gates
        // it (a `var`/`final` head infers the element itself, so a `void`
        // element lands in a `void` head and stays silent upstream).
        let void_element = match (self.engine.for_in_marks.last(), loop_var.as_ref()) {
            (Some(&mark), Some((name, _))) => self.engine.for_in_void_element(
                mark,
                await_kw.is_some(),
                self.engine.local_declared_ty(*name),
            ),
            _ => None,
        };
        if let Some(range) = void_element {
            self.engine.emit(SemanticCode::UseOfVoidResult, range, "", "");
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
        // A for-element's loop-variable group opens inside the enclosing
        // declaration's initializer; park the outer group's declared type
        // so its close hands it back (the outer initializer check needs its
        // own target — `List<int> v = <num>[for (int i = 0; …) i]`).
        self.decl_group_stack
            .push((engine.pending_decl_ty, engine.pending_decl_ty_unresolved));
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
        // Hand the enclosing group's declared type back when this one was
        // nested inside its initializer (a for-element's loop variable);
        // the outermost group closes to the idle `None` pair.
        let (outer_ty, outer_unresolved) = self
            .decl_group_stack
            .pop()
            .unwrap_or((None, false));
        engine.pending_decl_ty = outer_ty;
        engine.pending_decl_ty_unresolved = outer_unresolved;
        engine.armed_ty = None;
        engine.armed_ty_unresolved = false;
        // The group closed: a declarator no initializer claimed (a `var a;`
        // ahead of an initialized sibling, or a recovery shape) belongs to
        // no window and must not pair with the NEXT group's initializers.
        // The sweep stops at this group's own end, so an initializer-nested
        // group (`[for (var i = 0; ...) ...]`) never drops the enclosing
        // group's later declarators — those names sit past its `;`.
        engine.var_infer_pending = None;
        let group_end = end.map_or(u32::MAX, |token| token.range.offset);
        engine
            .pending_model_declarators
            .retain(|(_, range, _)| range.end() > group_end);
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
        // A collection literal's `for` element opens its loop variable's
        // initializer inside the enclosing declaration's initializer: park
        // the outer window so the nested close can hand it back (the outer
        // declaration's own bind and initializer check read it).
        self.init_windows.push((
            engine.init_mark,
            engine.in_initializer,
            engine.init_frame_depth,
        ));
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
        // initializer cannot re-latch it. A field/top-level variable group
        // folds its names ahead of the group head, so its declarators wait
        // in a queue and pair by position instead.
        let declarator = engine
            .var_infer_pending
            .take()
            .map(|(name, range)| (name, range, VarInferSlot::Local))
            .or_else(|| engine.take_model_declarator(token.range.offset));
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
            // The declarator's range ahead of the let-chain: the flow-blind
            // fallback below reads it, and a let-chain's own bindings never
            // reach an else arm.
            let init_declarator_range = engine.init_declarators.last().copied().flatten();
            let popped = engine.init_declarators.pop().flatten();
            if let Some((name, decl_range, infer_slot)) = popped
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
                            // index result's type exactly like a plain read. The
                            // bind carries that row's static type at WHATEVER
                            // nullability it holds (#3085): a `Map<K, V>` read is
                            // `V?`, and the oracle's inferred local takes that face
                            // — the SDK's own `var reference =
                            // _referenceByIndex[index]` (reference.dart:680, the
                            // switch's arm writes promoting it back to `Reference`)
                            // and `var argumentValue =
                            // _argumentValueMap[baseParameter]` (evaluation.dart
                            // :3335, the `:3341` null guard promoting it) are both
                            // written on it. Binding the nullable face is what makes
                            // those promotions decidable: the bind's staged declared
                            // type (keyed on the declarator's name token, below)
                            // reaches the flow layer, so `!= null` / `case null` and
                            // every arm write re-decide on the real face instead of
                            // the `Unknown` slot — and where no promotion stands, the
                            // unchecked/undefined verdicts fire on the `V?` receiver
                            // exactly like the oracle's. The one shape still held
                            // back is the type-tested receiver
                            // ([`Engine::index_receiver_latched`]): its pass-1 index
                            // result reads the unpromoted container, so the
                            // write-side staging — not this bind — owns it.
                            count
                                if count == mark + 2
                                    && matches!(
                                        engine.rows[mark].intent,
                                        Intent::Operator(op, _) if op == engine.intern("[]")
                                    )
                                    && matches!(engine.rows[mark + 1].shape, RowShape::Value)
                                    && engine.rows[mark + 1].ty != Ty::UNKNOWN
                                    && (engine.byte_at(engine.rows[mark + 1].range.end())
                                        == Some(b'!')
                                        || ((Engine::is_nullable_ty(engine.rows[mark + 1].ty)
                                            || engine
                                                .strictly_non_nullable(engine.rows[mark + 1].ty))
                                            && !engine
                                                .index_receiver_latched(&engine.rows[mark]))) =>
                            {
                                Some((mark + 1, None))
                            }
                            // A typed literal over an unresolvable element head
                            // (`var r = <Und>[]`) leaves the head's verdict row
                            // ahead of the literal's value row: the literal is
                            // the expression (`List<InvalidType>` — a typed row
                            // only when the literal face typed it).
                            count
                                if count == mark + 2
                                    && matches!(engine.rows[mark].shape, RowShape::TypeUse(..))
                                    && matches!(engine.rows[mark + 1].shape, RowShape::Value)
                                    && engine.rows[mark + 1].ty != Ty::UNKNOWN
                                    && engine.rows[mark + 1].range.offset
                                        <= engine.rows[mark].range.offset
                                    && engine.rows[mark].range.end()
                                        <= engine.rows[mark + 1].range.end() =>
                            {
                                Some((mark + 1, None))
                            }
                            // A plain-`=` assignment initializer (`var w = _w =
                            // Impl()`): the window holds the write target and the
                            // right side, and the assignment expression's static
                            // type is the right side's (assignment_expression_
                            // resolver.dart `_resolveTypes`: `=` writes
                            // `rightType`), so the right row is the expression.
                            count
                                if count == mark + 2
                                    && engine.rows[mark].intent == Intent::Write
                                    && engine.rows[mark].write_op.is_none()
                                    && engine.plain_assign_between(
                                        engine.rows[mark].range.end(),
                                        engine.rows[mark + 1].range.offset,
                                    ) =>
                            {
                                Some((mark + 1, None))
                            }
                            // The expression-plus-type-arguments window
                            // (`var c = C<int>`, `var t = C<int>.new`, `var
                            // f = id<int>`): the head's own row at `mark` is
                            // the initializer's expression — the `<...>`
                            // group's `TypeUse` rows above it are the written
                            // type-argument list, never a second expression —
                            // and its row-shape alone answers the static
                            // type (a `C<int>` TypeLiteral reads `Type`, a
                            // `p.C<T>` prefixed literal the same, a bare
                            // `id<int>` the instantiated tear-off signature
                            // the tear-off faces resolve). A `Member`/`Value`
                            // tail or any non-TypeUse above `mark` leaves the
                            // window unpaired exactly like before.
                            count
                                if count > mark + 1
                                    && engine.rows[mark + 1..]
                                        .iter()
                                        .all(|row| matches!(row.shape, RowShape::TypeUse(..))) =>
                            {
                                Some((mark, None))
                            }
                            _ => None,
                        }
                    }
                }
                && let Some(ty) = ifnull_ty.or_else(|| {
                    engine
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
                        })
                })
            {
                // The await operand's extent re-applies the flatten here
                // (#2506): the close-time rewrite reads an `Unknown` row
                // (member rows resolve in the later pass), so the resolved
                // `Future<T>`-shaped type flattens at the bind — the local
                // holds `T`, nullability preserved.
                let ty = if engine.await_operand_extents.iter().any(|&(start, end)| {
                    engine.rows[expr].range.offset >= start && engine.rows[expr].range.end() <= end
                }) {
                    engine.flatten(ty)
                } else {
                    ty
                };
                // An explicit-argument creation's written arguments ride
                // the head (`creation_args.rs`).
                let ty = engine.with_explicit_creation_args(expr, ty);
                engine.bind_inferred_ty(name, ty, infer_slot);
                // A promotion-latched initializer defers its re-bind
                // (#2539): the member read's accessor return rides the
                // UNPROMOTED declared receiver at this bind, and the
                // promotion re-decision runs only in pass 2. The entry
                // keys the declaring local's NAME-TOKEN offset — the
                // scoped locals-stack index aliases across scopes once
                // the frames close, and the re-bind map must stay
                // per-declaration. A cast-typed initializer (`var t =
                // p.first as T`) owes none: the cast decided the type, and
                // an unresolvable target would let the accessor's face
                // replace it.
                if infer_slot == VarInferSlot::Local
                    && let Some(expr_row) = engine.rows.get(expr)
                    && !engine.cast_rewrites.iter().any(|(span, _)| {
                        span.offset <= expr_row.range.offset && span.end() >= expr_row.range.end()
                    })
                    && engine
                        .promoted_recv_reads
                        .binary_search_by(|(member, _)| member.cmp(&expr_row.range.offset))
                        .is_ok()
                    && let Some(decl) = engine.locals.last().filter(|decl| decl.name == name)
                {
                    engine.deferred_var_rebinds.push((
                        decl.range.offset,
                        expr_row.range.offset,
                        ty,
                    ));
                }
                // The bare-identifier twin (#2567): `var x = p;` defers the
                // same re-bind — the initializer read's flow type (a
                // promotion standing at the read) re-binds the inferred
                // face in pass 2. A plain bare read only; every other
                // shape's face decided above.
                if infer_slot == VarInferSlot::Local
                    && let Some(expr_row) = engine.rows.get(expr)
                    && matches!(expr_row.shape, RowShape::Bare(BareInfo::Plain { .. }))
                    && let Some(decl) = engine.locals.last().filter(|decl| decl.name == name)
                {
                    engine
                        .deferred_bare_rebinds
                        .push((decl.range.offset, expr_row.range, ty));
                }
                // The implicit-call twin (`var f = local(args)` over a
                // promotable bare callee): the send's result is the
                // promoted callee's `call` member return — a pass-2 face
                // (the callee's flow read re-decides there), so the rebind
                // defers like the bare read's.
                if infer_slot == VarInferSlot::Local
                    && let Some(expr_row) = engine.rows.get(expr)
                    && expr_row.intent == Intent::Invoke
                    && matches!(expr_row.shape, RowShape::Bare(_))
                    && let Some(decl) = engine.locals.last().filter(|decl| decl.name == name)
                {
                    engine
                        .deferred_call_rebinds
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
                // under-report stands). A model-level declaration (a
                // top-level variable or a field) owns no frame local to
                // stage against: its inferred face rides the model overlay
                // ([`Engine::bind_inferred_ty`]) instead.
                if infer_slot == VarInferSlot::Local
                    && let Ty::Named {
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
                    let tree =
                        staged_arg_tree(&engine.boundary.args.borrow(), &engine.strings, args)
                            .unwrap_or_default();
                    staged = Some((decl_range.offset, lexeme, nullable, tree));
                }
            } else if let Some(mark) = engine.init_mark
                && engine.rows.len() == mark + 1
                && engine.rows[mark].ty == Ty::UNKNOWN
                && let Some((_, decl_range, VarInferSlot::Local)) = init_declarator_range
            {
                // The flow-blind declarator fallback (the plain
                // `var x = <member read>`): the initializer row's face
                // resolves only in pass 2 (the accessor return the flow
                // lattice cannot spell), so the local's type derivation
                // defers to the `resolve_rows` pre-pass — the same single-
                // row discipline the bind window holds.
                let row_offset = engine.rows[mark].range.offset;
                engine
                    .local_init_rows
                    .insert(decl_range.offset, (row_offset, false));
                // The invalid face is decidable now (the receiver chain's
                // written heads are staged), and the later binary folds
                // that read the local run at parse time.
                let invalid = matches!(engine.rows[mark].shape, RowShape::Member { .. })
                    && engine
                        .member_reads
                        .iter()
                        .find(|read| read.offset == row_offset)
                        .is_some_and(|read| engine.member_read_invalid(read));

                if invalid {
                    engine
                        .invalid_init_locals
                        .borrow_mut()
                        .insert(decl_range.offset);
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
        // Hand the enclosing initializer's window back when this one was a
        // nested window (a for-element's loop variable): the outer
        // declaration's bind and check close against its own mark, not the
        // nested one's (already cleared by `check_decl_initializer`).
        if let Some((mark, in_initializer, frame_depth)) = self.init_windows.pop() {
            let engine = &mut self.engine;
            engine.init_mark = mark;
            engine.in_initializer = in_initializer;
            engine.init_frame_depth = frame_depth;
        }
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
        // A literal's own `<T…>` variable counts toward the context-match
        // gate (a foreign variable's close — a nested annotation's —
        // never reaches here with `list_depth` still 0 while frames
        // match, so the guard inside keys on the literal's own frame).
        engine.closure_type_variable();
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
                    // #3419 — a bare read inside an implicit-formals
                    // anonymous-method body that the scope resolved onto
                    // the enclosing container's member map is an implicit
                    // `this.` on the RECEIVER, not the container's member:
                    // the row rewrites to a pass-2 miss and the
                    // enclosing-`this` staging below stands down (the
                    // receiver's own face is staged at pass 2).
                    let anon_rewritten = engine.rewrite_anon_bare_row(name, token.range);
                    // The implicit-this member read's declared face (the
                    // #2292 `this`-stage sibling): a bare identifier the
                    // scope cannot answer may resolve to an instance
                    // field/getter of the enclosing container — stage the
                    // member's declared type keyed on the read's offset so
                    // the flow's unknown-read arm answers on the real
                    // element (a switch scrutinee over the bare read then
                    // closes its exhaustiveness verdict). A same-named
                    // local never reaches the staged row: the flow's own
                    // scope resolution answers first. Only the spellable
                    // named shapes ride (the same gate the `this` stage
                    // applies); anything else keeps the unknown face.
                    if let Some((container, kind)) = engine.container_context()
                        && !anon_rewritten
                    {
                        let mut face =
                            engine.accessor_face_for(&Receiver::This { container, kind }, name);
                        // A member the enclosing class declares itself reads
                        // through the identity substitution (`this` is
                        // `C<T1..Tn>` inside `C`): a field typed by one of
                        // the class's own parameters (`DB? _database`) keeps
                        // its declared spelling, which the accessor face's
                        // receiver-argument instantiation cannot supply. The
                        // flow layer resolves the parameter head through its
                        // own bound table. The probe leaves the memo's
                        // last-found origin as it was: later member-edge
                        // readers key on it.
                        if face == Ty::Unknown
                            && matches!(kind, ContainerKind::Class | ContainerKind::Mixin)
                        {
                            let origin = engine.member_faces.found_origin.get();
                            if let Walk::Found(entry) = engine.walk_members(container, name)
                                && engine.member_faces.last_origin() == Some(container)
                            {
                                face = match entry.kind {
                                    MemberKind::Field => entry.var_ty,
                                    MemberKind::Getter => entry.return_ty,
                                    _ => Ty::Unknown,
                                };
                            }
                            engine.member_faces.found_origin.set(origin);
                        }
                        if let Ty::Named {
                            args: ArgListId::EMPTY,
                            ..
                        } = face
                        {
                            if let Ty::Named {
                                name: ty_name,
                                nullable,
                                ..
                            } = engine.fold_alias_ty(face)
                            {
                                let lexeme = engine.strings.resolve(ty_name).to_string();
                                let base = self.flow().intern_name(&lexeme);
                                self.flow().stage_this_member_type(
                                    token.range.offset,
                                    FlowTy {
                                        base: FlowBase::Name(base),
                                        nullable,
                                    },
                                );
                            }
                        }
                    } else if let Some(sig) = local_fn_tear_off_sig(engine, name) {
                        // The container-less bare read's LOCAL tear-off face
                        // (the #2852 sibling): a frame-local function the
                        // read resolves to stages its OWN spelling at the
                        // read's offset — the same channel the top-level arm
                        // below rides, with the shape registered fold-time
                        // beside the adapter (`map_name`'s final-miss arm
                        // resolves it; the element model carries no local
                        // declarations, so the snapshot's build-time walk
                        // cannot see one). The capture is the #2842
                        // product: a read before the declaration's frame
                        // close or a refused capture (generic, named
                        // formals, async/generator) finds nothing and keeps
                        // the unknown face, and a plain local variable's
                        // read answers through the flow's own scope first
                        // (it never consults the staged row — the walk
                        // above refuses it too). Registration gates
                        // staging: a signature whose columns map nowhere
                        // stays unstaged, since an unregistered staged
                        // name would read `Invalid`'s vacuous arm at the
                        // fold — a false dead branch.
                        let pool = engine.boundary.args.borrow();
                        let staged = engine.ops.is_some_and(|ops| {
                            ops.register_local_fn_shape(
                                name,
                                sig.return_ty,
                                &sig.positional,
                                &sig.named,
                                sig.required_positional,
                                &pool,
                            )
                        });
                        drop(pool);
                        if staged {
                            let lexeme = engine.strings.resolve(name).to_string();
                            let base = self.flow().intern_name(&lexeme);
                            self.flow().stage_this_member_type(
                                token.range.offset,
                                FlowTy {
                                    base: FlowBase::Name(base),
                                    nullable: false,
                                },
                            );
                        }
                    } else if let Some(decl) = engine.top_level_decl(name)
                        && decl.kind == MicroKind::TopLevelFn
                        && decl.formals.is_some()
                        && decl.type_params == 0
                    {
                        // The container-less bare read's tear-off face
                        // (the #2835 sibling): a top-level function the
                        // scope resolves stages its OWN spelling at the
                        // read's offset — the resolved snapshot's
                        // `fn_shapes` is what the spelling folds through
                        // (`map_name` answers the declared signature), so
                        // the operand rides the flow's existing is-test
                        // consumer honestly (a matching alias pair folds
                        // constant, a non-matching one stays live). The
                        // gate admits only the tear-off shape the
                        // name-level lattice cannot spell: a local or a
                        // top-level variable's read answers through the
                        // flow's own resolution first (it never consults
                        // the staged row), and a getter's read is its
                        // return type, never a tear-off. A generic
                        // function's tear-off instantiates to bounds the
                        // snapshot does not register, so it keeps the
                        // unknown face.
                        let lexeme = engine.strings.resolve(name).to_string();
                        let base = self.flow().intern_name(&lexeme);
                        self.flow().stage_this_member_type(
                            token.range.offset,
                            FlowTy {
                                base: FlowBase::Name(base),
                                nullable: false,
                            },
                        );
                    }
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
                } else if let Some(name) = engine.synthetic_name(token) {
                    // The nameless recovery face (`name ?? '<unnamed>'`):
                    // upstream registers a real `LocalVariableElement` with
                    // a null name, so the tracked local joins under the
                    // empty name — the unused-local arm renders it
                    // `<unnamed>` exactly like the oracle's nameless row.
                    engine.note_local(name, token.range);
                    engine.const_decl_name(name, false);
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
                        engine.fn_param_sigs.insert(name, sig.clone());
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
                    } else {
                        // The alias-instantiated claim (#2849): a written
                        // parameterized function-RHS alias annotation
                        // (`F<int>`) binds the same stash entry its
                        // instantiated function type would — the element
                        // model's resolved annotation type, bounds
                        // substituted (upstream Dart SDK source,
                        // pkg/analyzer/lib/src/dart/resolver/
                        // named_type_resolver.dart `_instantiateElement`).
                        engine.stash_alias_instantiated_formal(name);
                    }
                    let type_level =
                        engine.fn_type_formal_lists > 0 || engine.formal_list_depth > 1;

                    if type_level {
                        // The inner formal's written type is consumed but
                        // binds nothing in the enclosing body's scope.
                        engine.armed_ty = None;
                    } else {
                        engine.note_parameter(name, token.range);
                        engine.bind_parameter_type(name);
                        // The catch group's ordinal: the list's own first
                        // and second real formal names are the official
                        // AST's exception/trace faces (nested lists run at
                        // a deeper depth and never count).
                        if engine.catch_formal_group == Some(engine.formal_list_depth) {
                            let row = engine.locals.len().checked_sub(1);
                            if engine.catch_ord_exc.is_none() {
                                engine.catch_ord_exc = row;
                            } else if engine.catch_ord_trace.is_none() {
                                engine.catch_ord_trace = row;
                            }
                        }
                    }
                }
            }
            C::CatchParameter => {
                match name {
                    Some(name) => {
                        engine.note_catch_parameter(name, token.range);
                        engine.bind_parameter_type(name);
                    }
                    None => {
                        // The nameless recovery face registers a real
                        // element on either slot: upstream reports a
                        // null-name catch element (`name == null`), never a
                        // dropped one — the `<unnamed>` exception row of
                        // `catch (var)` and the `<unnamed>` stack-trace row
                        // of a junk-filled second slot both surface. The
                        // slot decides the kind (`note_catch_parameter`
                        // reads the parser's comma fact): a nameless
                        // exception row still spares the auto-used plain
                        // face through `finish_catch_clause`, matching the
                        // oracle's silence there.
                        if let Some(name) = engine.synthetic_name(token) {
                            engine.note_catch_parameter(name, token.range);
                        }
                    }
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
                    // #3118: the header's declaring name — its
                    // type-variable window pairs at the window's close
                    // exactly like a top-level function's (the
                    // `exec_bounds` route, never `method_exec_bounds`: a
                    // local function owns no member send, so the
                    // member-inference guard the `method_` table exists
                    // for has nothing to race). The nameless arm stays
                    // unlatched — a synthetic `""` key would collide with
                    // the generic function-literal send's
                    // `bounds.pending_window` fallback
                    // (`row.name == intern("")`), and the top-level
                    // nameless arm keeps the same closed-window posture.
                    engine.pending_exec_name = Some((name, token.range.offset));
                    engine.pending_exec_is_method = false;
                    // The written return is the call's result face
                    // (`List<Und> ranges(f) => …; ranges(x).any((r) => …)`):
                    // it joins the enclosing frame's local-callee return
                    // table [`Engine::callee_return_ty`] reads. An absent
                    // or unresolved annotation stays out (`dynamic`), and a
                    // generic local function (`T f<T>()`) needs the call's
                    // instantiation the table cannot carry.
                    if engine.byte_at(token.range.end()) != Some(b'<')
                        && let Some(ret) =
                            engine.frames.last().and_then(|frame| match &frame.kind {
                                FrameKind::Executable { ret: Some(ret), .. } => Some(ret.ty),
                                _ => None,
                            })
                        && ret != Ty::UNKNOWN
                        && let Some(enclosing) = engine.frames.len().checked_sub(2)
                    {
                        engine.frames[enclosing].fn_formal_rets.insert(name, ret);
                    }
                } else if let Some(name) = engine.synthetic_name(token) {
                    // The nameless recovery face (`name ?? '<unnamed>'`):
                    // upstream registers a real `LocalFunctionElement` with
                    // a null name — the tracked entry joins under the empty
                    // name at the same boundary a named head owns (the
                    // stash drop and the `unknown` return latch mirror the
                    // oracle's dynamic-return face for a local function
                    // written without one), and the frame's locals set
                    // keeps a resolution entry the body's recovery reads
                    // can hit.
                    engine.note_local_function(name, token.range);
                    engine.fn_stash_drop();
                    engine.latch_return_type(ExecKind::Function, name, token.range);
                }
            }
            C::LabelDeclaration => {
                if let Some(name) = name {
                    // The row's anchor is the `Label` node — the written name
                    // plus its colon (upstream `dead_code_verifier.dart`'s
                    // `diag.unusedLabel…at(label)`, the label identifier's
                    // `Label` wrapper). The parser fires at the colon, so the
                    // `:` immediately follows the name token.
                    let mut range = token.range;
                    if engine.byte_at(range.end()) == Some(b':') {
                        range.length += 1;
                    }
                    engine.pending_label = Some((name, range));
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
                        // The written head spelling rides along: a named
                        // factory's display name is the qualified `C.named`
                        // (`ConstructorElement.displayName`).
                        let head = name.unwrap_or(container);
                        engine.latch_factory_return(container, head, token.range);
                    }
                }
                if let Some(name) = name {
                    // A method declaration's name is a boundary: a
                    // function-typed RETURN annotation's stash is not a
                    // variable's.
                    engine.fn_stash_drop();
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
                        // A generative constructor's return check is the
                        // constructor face: `ConstructorElement.returnType`
                        // is the enclosing class's `thisType`
                        // (element.dart:1004) and the message host is
                        // `constructor` (rtv.dart's
                        // `EnclosingExecutableContext.isConstructor`), not
                        // the armed `Method` latch — a generative head
                        // parses no return annotation, so the armed slot is
                        // empty anyway. `=> e` bodies report
                        // `return_of_invalid_type` against `C` (`C() => 3`
                        // reports `int` can't be returned from the
                        // constructor 'C'`).
                        if let Some((container, _)) = engine.ctor_pending {
                            engine.latch_factory_return(container, name, token.range);
                        }
                    } else {
                        // The accessor face (`get`/`set`) is a
                        // `PropertyAccessorElement` upstream — its
                        // return-check host word is `function`, never
                        // `method`
                        // (`EnclosingExecutableContext.isFunction`,
                        // error_verifier.dart:164-166; the corpus face:
                        // `I3 get a => null;` reports `... from the
                        // function 'a'`).
                        let host = if engine.open_member_accessor {
                            ExecKind::Function
                        } else {
                            ExecKind::Method
                        };
                        engine.latch_return_type(host, name, token.range);
                    }
                    // #2430: the method header's declaring name joins the
                    // FAM10 pairing — its type-variable window lands in
                    // `method_exec_bounds` at the close, the bound-aware
                    // nullability/never-match gates' read of the
                    // enclosing method's own parameter bounds.
                    engine.pending_exec_name = Some((name, token.range.offset));
                    engine.pending_exec_is_method = true;
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
                } else if let Some(name) = engine.synthetic_name(token) {
                    // The nameless recovery face (`name ?? '<unnamed>'`):
                    // upstream registers a real `MethodElement` with a null
                    // name (element.dart:2179's `isPrivate` maps it private,
                    // so the unused-element gate owns it), never a dropped
                    // member. A nameless head is never a constructor head —
                    // the provisional site's range is a real head token —
                    // so the member twin registers unconditionally under
                    // the empty name at the anchor chain.
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
            C::OperatorName => {
                if let Some(name) = name {
                    // The operator name is a declaration-name boundary like
                    // every executable's (a function-typed return
                    // annotation's stash must not cross it), and the
                    // operator's own `ret` latches the armed return type —
                    // an operator is a `MethodElement` upstream, so its
                    // host word is `method` (`int operator >(C o) => "s";`
                    // reports `... from the method '>'`).
                    engine.fn_stash_drop();
                    engine.latch_return_type(ExecKind::Method, name, token.range);
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
                    // The completion row's anchor is the compound name
                    // (`ConstructorDeclaration.errorRange`: the type name
                    // through the selector's end — `A.named() {}` reports
                    // at `A.named`). The head event latched the bare
                    // container name; the selector's end widens it here —
                    // for every constructor frame (`ret.ctor`), factory
                    // heads read their `factory_head` anchor and
                    // generative heads keep their own latched offset.
                    if let Some(frame) = engine.frames.last_mut()
                        && let FrameKind::Executable { ret: Some(ret), .. } = &mut frame.kind
                        && ret.ctor
                    {
                        let head_offset = engine
                            .factory_head
                            .map_or(ret.name_span.offset, |(head, _)| head.offset);
                        ret.name_span = SourceRange {
                            offset: head_offset,
                            length: token.range.end().saturating_sub(head_offset),
                        };
                        // The constructor's message display joins the
                        // selector (`C.named`,
                        // `ConstructorElement.displayName`): the head
                        // event latched the bare container; the selector
                        // arrives here. A `new` selector keeps the bare
                        // face (`displayName`'s `name == 'new'` arm
                        // answers the container).
                        if engine.strings.resolve(name) != "new" {
                            ret.display = Some(engine.strings.intern(&format!(
                                "{}.{}",
                                engine.strings.resolve(container),
                                engine.strings.resolve(name)
                            )));
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
                    // A group with NO written annotation infers every
                    // declarator's type from its initializer (#2513): the
                    // model rows (`TopDecl::var_ty` / `MemberEntry::var_ty`)
                    // carry `Unknown` there — the collector runs before any
                    // inference — so the close binds the inferred face into
                    // the overlay the read surfaces answer through. An
                    // `augment` group declares no new element and owns no
                    // row to overlay, and a function-typed annotation is
                    // the written-signature stash's face
                    // ([`Engine::fn_var_sigs`]), never an inference input.
                    if engine.pending_decl_ty.is_none()
                        && engine.armed_ty.is_none()
                        && engine.fn_type_stash.is_none()
                        && !engine.group_augment
                    {
                        let slot = engine.container_context().map_or(
                            VarInferSlot::TopLevel,
                            |(container, _)| VarInferSlot::Field(container),
                        );
                        engine
                            .pending_model_declarators
                            .push((name, token.range, slot));
                    }
                    let final_gate = engine.group_final && !engine.group_late;
                    let non_nullable_gate = !engine.group_const
                        && !engine.group_final
                        && !engine.group_late
                        && !ty_unresolved
                        && engine.written_potentially_non_nullable(ty);
                    // An `augment` group owns the same initializer obligation
                    // as any declaration: upstream walks every declaration
                    // node, the augmenting one included
                    // (`_checkForNotInitializedFieldDeclaration`,
                    // `visitTopLevelVariableDeclaration` — `class C { final
                    // int x = 0; } augment class C { augment final int x; }`
                    // reports at the augment's `x`). Its exemptions are the
                    // node's OWN: an `abstract`/`external` head (the flushes'
                    // member-model exemption reads the introductory entry,
                    // never this fragment's keywords — `augment abstract
                    // final String? foo;` stays silent), and an extension
                    // type's instance augment — the representation field's
                    // fragment, which the representation constructor
                    // initializes (upstream `hasGenerativeConstructor`).
                    let augment_exempt = engine.group_augment
                        && (group_head_exempts_init(engine, token.range.offset)
                            || (!engine.open_member_static
                                && matches!(
                                    engine.container_context(),
                                    Some((_, ContainerKind::ExtensionType))
                                )));
                    if (final_gate || non_nullable_gate) && !augment_exempt {
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
                } else if let Some(name) = engine.synthetic_name(token) {
                    // The nameless recovery face (`name ?? '<unnamed>'`):
                    // upstream registers a real `FieldElement` /
                    // `TopLevelVariableElement` with a null name — a real
                    // element, not a dropped declaration. Inside a
                    // container the member site joins under the empty name
                    // (element.dart:2179's `isPrivate` maps a null name
                    // private, so the unused-field gate owns it); at top
                    // level the same face keeps the `<unnamed>` anchor.
                    if context == C::FieldDeclaration {
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
                        let anchor = engine.nameless_site_range();
                        engine.note_decl_site(name, anchor, DeclKind::TopLevelVar);
                    }
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
                let joined = format!("{}.{}", text, engine.strings.resolve(leaf_id));
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
        let mut span = match question {
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
        let mut re_fired_group = false;
        match engine.completed_group.take() {
            Some((group_head, group_offset, args))
                if group_head == leaf_id && group_offset == name.range.offset =>
            {
                re_fired_group = true;
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
        // The written-argument extent (`List<int>`, not the bare head): a
        // re-fired parameterized head's own group closed immediately
        // before this completion, so `type_args_group_end` is its `>` and
        // the annotation's span grows to it — the call-args capture's
        // per-argument anchor (`_typeArgumentErrorNode` returns the whole
        // written TypeAnnotation). A trailing `?` already spans past the
        // group and keeps its longer face.
        if re_fired_group
            && let Some(end) = engine.type_args_group_end
            && end > span.offset + span.length
        {
            span = SourceRange {
                offset: span.offset,
                length: end - span.offset,
            };
        }
        if let Some((head_name, head_range)) = head {
            engine.check_raw_type(head_name, head_range);
            // #3296 — the same group-less head answers its boundedness
            // verdict: a head with no `<...>` group opens no #610 site, so
            // `check_named`'s instantiate-to-bounds arm never ran on it
            // (`void f(G g)` over a bare bounded alias reports upstream).
            // Two contexts own their heads elsewhere: the declaration
            // windows (container clauses, bound annotations — `leaf` above
            // routed into them) and an open `<...>` group, whose members
            // ride the enclosing site's tree.
            if engine.group_heads.is_empty()
                && !engine.bounds.in_type_variables()
                && engine.var_header.is_none()
            {
                engine.queue_bare_head_site(head_name, span);
            }
        }
        // The written-signature and call-argument captures take the final
        // completion (the re-fired parameterized head carries its written
        // arguments on the armed type by now).
        if let Some(ty) = engine.armed_ty {
            let leaf = leaf_id;
            engine.fn_capture_type(ty, leaf, name.range.offset);
            engine.call_args_feed(ty, leaf, name.range.offset, span);
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
        engine.call_args_feed(Ty::Void, void_id, token.range.offset, token.range);
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
        engine.call_args_feed(Ty::Void, void_id, token.range.offset, token.range);
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
                .get(engine.byte_of(candidate.span.end())..)
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
            // The const constructor's `_reportNotPotentialConstants` gate
            // (upstream Dart SDK source, pkg/analyzer/lib/src/dart/
            // constant/constant_verifier.dart —
            // `_validateConstructorInitializers` scans each `x = e` field
            // initializer under `node.constKeyword != null`): every leaf
            // must be *potentially* constant — a const-ctor formal bound
            // opaque passes, a non-const or implicit-`this` member read
            // collects `invalid_constant` at its own span. Wildcard
            // formals bind no readable name, so they are filtered out —
            // `assert(_ > 0)` leaves `_` unresolved. The formals resolve
            // off the latch key directly: `initializer_depth` is already
            // closed above, so `ctor_init_formals` no longer sees the
            // constructor.
            let formals_key = if engine.is_bare_new_head(key.1) {
                key.0
            } else {
                key.1
            };
            let formals: Vec<StringId> = engine
                .index
                .get(engine.file)
                .and_then(|f| f.container(key.0))
                .and_then(|c| c.ctor_formals.get(&formals_key))
                .map(|formals| {
                    formals
                        .positional
                        .iter()
                        .chain(formals.named.iter())
                        .map(|formal| formal.name)
                        .collect()
                })
                .unwrap_or_default();
            let mut spans = Vec::new();
            {
                let mut env: FxHashMap<StringId, CtorBinding> = FxHashMap::default();
                for name in formals {
                    if !engine.is_wildcard(name) {
                        env.insert(name, CtorBinding::Opaque);
                    }
                }
                let scope = InitRhsLookup {
                    engine,
                    env: &env,
                };
                for node in &engine.ctor_init_pending {
                    collect_not_potentially_constant(node, &scope, &mut spans);
                }
            }
            for span in spans {
                // A leaf another face already reported at the same span
                // (`(a: g)`'s record-element verdict under
                // `check_const_elements`, or the super-arglist scan on a
                // shared offset) fires once upstream — the oracle's
                // `_reportNotPotentialConstants` is a single pass, so a
                // same-code same-span hit is the duplicate, not a second
                // row.
                if engine.report.diagnostics.iter().any(|d| {
                    d.code.name == "invalid_constant" && d.range == span
                }) {
                    continue;
                }
                engine.emit(SemanticCode::InvalidConstant, span, "", "");
            }
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
        // The `this : ...` body is one of the two scopes the primary
        // constructor's formals live in (upstream `PrimaryParameterScope`,
        // pkg/analyzer/lib/src/dart/resolver/scope_context.dart:406-430);
        // every other member plane keeps them muted
        // ([`Engine::primary_formals_muted`]).
        self.engine.primary_formals_muted = false;
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
        self.engine.primary_formals_muted = true;
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
        engine.note_ffi_annotation(begin, end);
        // The annotation-node span exists only at the metadata end:
        // upstream reports `undefined_annotation` `.at(node)` on the whole
        // Annotation (`annotation_resolver.dart:245`), `@` through the
        // arguments' closing paren when present (manual face
        // annotation_space_before_parenthesis/1: 1:1|16 over
        // `@metadata (x, y)`).
        let node = SourceRange {
            offset: begin.range.offset,
            length: end.range.end() - begin.range.offset,
        };
        engine.emit_undefined_annotation(node);
        engine.const_capture.close_annotation();
        engine.annotation_at = None;
    }

    // —— receivers and selectors ——————————————————————————————————————

    fn handle_this_expression(&mut self, token: Token, context: IdentifierContext) {
        // #3419 — `this` inside an implicit-formals anonymous-method body
        // (`x.{ ... }` / `x.=> e`) IS the receiver: `_thisType` wraps the
        // body and the `ThisContext.instanceMemberBody` guard makes the
        // keyword legal in every enclosing context (resolver.dart
        // :2031-2046, error_verifier.dart:400-406 — a static, top-level or
        // initializer context still binds the receiver). The keyword
        // receiver push is bypassed whole — no enclosing-container row, no
        // `invalid_reference_to_this` — and the receiver row classifies
        // typed only on the promotion-free literal faces (a declared face
        // may be stale against a flow promotion, and a member row frozen
        // with a stale `Typed` receiver never re-decides in pass 2); the
        // pass-2 walk re-derives every other face from the read-type row
        // `stage_anon_method_bodies` joins at the keyword's span.
        if let Some(entry) = self
            .engine
            .anon_method_body_at(token.range.offset)
            .filter(|entry| entry.parens.is_none())
        {
            let this_ty = self.engine.anon_this_ty(entry);
            let literal_ty = self.engine.anon_receiver_literal_ty(entry);
            // The flow side's staged channel (#2292's sibling): the same
            // offset-keyed adoption `Ev::This` reads.
            if let Some(Ty::Named {
                name,
                nullable,
                args: ArgListId::EMPTY,
            }) = this_ty
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
            self.flow().handle_this_expression(token, context);
            let engine = &mut self.engine;
            // The query face's `this` publication (#1869-2): the reference
            // row binds the keyword; the receiver's face answers `type_of`.
            engine.refs.push(UnitRef {
                range: token.range,
                name: engine.intern("this"),
                target: RefTarget::This,
            });
            if let Some(ty) = this_ty {
                engine.query_type_rows.push((token.range, ty));
            }
            let recv = match literal_ty {
                Some(ty @ Ty::Named { name, .. }) => Receiver::Typed { type_name: name, ty },
                _ => Receiver::Unknown,
            };
            engine.push_row(Row {
                name: engine.intern(""),
                range: token.range,
                qualified: false,
                shape: RowShape::Receiver(recv),
                intent: Intent::Read,
                write_op: None,
                ty: Ty::UNKNOWN,
            });
            engine.const_capture.this_or_super(token.range);
            return;
        }

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
        // The dead-chain row's extent is the ACCESS node's own end
        // (`_verifyUnassignedSimpleIdentifier`'s `node.end -
        // operator.offset`, dead_code_verifier.dart:578-590): a folded
        // send ends at its `)`, a plain member at its identifier — the
        // handler's `end` param is the send-tiling follower, one token
        // past both.
        let chain_end = engine
            .last_send_callee
            .as_ref()
            .map(|(_, send_end)| *send_end)
            .unwrap_or_else(|| {
                engine
                    .tokens
                    .get(token.id as usize + 1)
                    .map(|member| {
                        let raw = &engine.source[member.byte_range()];
                        member.utf16_offset + raw.chars().map(char::len_utf16).sum::<usize>() as u32
                    })
                    .unwrap_or_else(|| end.range.end())
            });
        // A named selector is Identifier- or keyword-lexeme shaped; the
        // recovery folds (`?.{`) produce a literal token there instead.
        let member_ident = engine
            .tokens
            .get(token.id as usize + 1)
            .is_some_and(|member| {
                matches!(
                    member.kind,
                    dartq_scanner::TokenKind::Identifier
                        | dartq_scanner::TokenKind::Keyword(_)
                        | dartq_scanner::TokenKind::OpenSquare
                )
            });
        // The `void`-receiver face (#3249): resolved before the merge folds
        // the receiver row away — `v().m`, `x?.m` and `x.m` over a `void`
        // local all consume the receiver's value.
        if let Some(pos) = engine.access_receiver_index(token.range.offset)
            && let Some(selector) = engine.access_selector_span(token)
        {
            engine.emit_void_member_receiver(pos, selector);
        }
        engine.merge_member(is_null_aware, token.range, chain_end, member_ident);
        // The explicit-receiver member read's declared face stages for the
        // flow's member-access fold (the implicit-this stage's sibling):
        // the name-level fold cannot walk the receiver's members, so a
        // switch over `d.t` would miss the always-exhaustive verdict on
        // the accessor's real type. Only the spellable named shapes ride.
        let staged_member = (!is_null_aware)
            .then(|| engine.rows.last())
            .flatten()
            .filter(|row| matches!(row.shape, RowShape::Member { .. }))
            .and_then(|row| {
                let offset = row.range.offset;
                let read = engine
                    .member_reads
                    .binary_search_by(|read| read.offset.cmp(&offset))
                    .ok()
                    .map(|index| &engine.member_reads[index])?;
                // The accessor walk answers class members; a record field
                // getter (`pair.$1`) has no member entry — the merge's own
                // field read typed the row.
                let face = match engine.accessor_face_for(&read.recv, read.name) {
                    Ty::Unknown => row.ty,
                    face => face,
                };
                match engine.fold_alias_ty(face) {
                    Ty::Named {
                        name,
                        nullable,
                        args: ArgListId::EMPTY,
                    } => Some((offset, engine.strings.resolve(name).to_string(), nullable)),
                    _ => None,
                }
            });
        engine.const_capture.dot_access(is_null_aware, end.range);
        // The member send's bottom face latches here, not at the send
        // event that preceded this one (`never_send.rs`): the merged row
        // is the first shape the callee's declared return reads through.
        if !is_null_aware && engine.merged_send_returns_never() {
            self.pending_exit = Some(end);
        }
        if let Some((offset, lexeme, nullable)) = staged_member {
            let base = self.flow().intern_name(&lexeme);
            self.flow().stage_member_type(
                offset,
                FlowTy {
                    base: FlowBase::Name(base),
                    nullable,
                },
            );
        }
    }

    fn handle_cascade_access(&mut self, operator: Token, end: Token, is_null_aware: bool) {
        self.flow()
            .handle_cascade_access(operator, end, is_null_aware);

        let engine = &mut self.engine;
        // The first section carried the `?..` verdict (`is_null_aware`) —
        // the pending op's stand-in duty ends here.
        engine.cascade_null_aware_op = None;
        // The dead-chain extent mirrors `handle_dot_access`'s: the access
        // node's own end, never the send-tiling follower.
        let chain_end = engine
            .last_send_callee
            .as_ref()
            .map(|(_, send_end)| *send_end)
            .unwrap_or_else(|| {
                engine
                    .tokens
                    .get(operator.id as usize + 1)
                    .map(|member| {
                        let raw = &engine.source[member.byte_range()];
                        member.utf16_offset + raw.chars().map(char::len_utf16).sum::<usize>() as u32
                    })
                    .unwrap_or_else(|| end.range.end())
            });
        let member_ident = engine
            .tokens
            .get(operator.id as usize + 1)
            .is_some_and(|member| {
                matches!(
                    member.kind,
                    dartq_scanner::TokenKind::Identifier
                        | dartq_scanner::TokenKind::Keyword(_)
                        | dartq_scanner::TokenKind::OpenSquare
                )
            });
        // The `void`-receiver face (#3249): a cascade section's target is
        // consumed exactly like a plain member access's receiver
        // (`a..m()` reports at the target, `a..m` at the member). Resolved
        // before the merge folds the target row away.
        if let Some(pos) = engine.access_receiver_index(operator.range.offset)
            && let Some(selector) = engine.access_selector_span(operator)
        {
            engine.emit_void_member_receiver(pos, selector);
        }
        engine.merge_member(is_null_aware, operator.range, chain_end, member_ident);
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
        // The resolved send return stages into the flow recorder (#2567
        // R18): the name-level fold reads the callee as an unknown face,
        // so a switch whose scrutinee is the call misses the
        // always-exhaustive verdict without the real return. Only the
        // spellable named shapes ride (the same tree posture the
        // assignment-RHS staging applies); anything else keeps the
        // unknown-result under-report.
        let staged_call = self
            .engine
            .send_return_staging
            .take()
            .and_then(|(offset, ty)| {
                // The unit-kind returns (`Null` of `Null Function() nf`,
                // `dynamic`/`void`/`Never` of the same signature) spell no
                // written name but the flow lattice carries them: the
                // callable-typed callee's head reads `Base::Function`,
                // whose nullability the name-level fold cannot answer —
                // the staged row is the invocation's only static-type
                // source, so the `null == nf()` equality and the
                // `nf() is int` check fold on the real return.
                // `FlowBase::Null` (never the `Name("Null")` spelling) is
                // the only face both type-ops backends read identically:
                // `NameTypeOps::classify` answers `NullOrEquivalent` on
                // the identity, while the same `Name` walks as an opaque
                // class and folds `null == nf()` backwards. The
                // spellable-only contract keeps `Ty::Function`,
                // `Ty::Record` and the free-parameter `Unknown` out —
                // under-report stands ahead of a fabricated face.
                let (base, nullable, tree) = match ty {
                    Ty::Named {
                        name,
                        nullable,
                        args,
                    } => {
                        let tree = staged_arg_tree(
                            &self.engine.boundary.args.borrow(),
                            &self.engine.strings,
                            args,
                        )?;
                        let lexeme = self.engine.strings.resolve(name).to_string();
                        (Some(lexeme), nullable, tree)
                    }
                    Ty::Null => (None, false, Vec::new()),
                    Ty::Dynamic => (None, true, Vec::new()),
                    Ty::Void => (None, false, Vec::new()),
                    Ty::Never => (None, false, Vec::new()),
                    Ty::Function { .. } | Ty::Record { .. } | Ty::Unknown => return None,
                };
                Some((offset, base, nullable, tree, ty))
            });
        if let Some((offset, lexeme, nullable, tree, ty)) = staged_call {
            let base = match lexeme {
                Some(lexeme) => FlowBase::Name(self.flow().intern_name(&lexeme)),
                None => match ty {
                    Ty::Null => FlowBase::Null,
                    Ty::Dynamic => FlowBase::Dynamic,
                    Ty::Void => FlowBase::Void,
                    Ty::Never => FlowBase::Never,
                    _ => FlowBase::Unknown,
                },
            };
            self.flow().stage_call_result(offset, FlowTy { base, nullable }, tree);
        } else if let Some((offset, name)) = self.engine.send_invalid_staging.take() {
            // The genuinely-unresolved head's Invalid face (#2860): the
            // setter certified the call head resolves nowhere (the send's
            // return read Unknown — the Named arm above could not have
            // staged), so the oracle's static type for the invocation is
            // `InvalidType`. The flow lattice spells it as the head's own
            // interned name — the same unresolved-spelling stand-in the
            // flow's bare-head face uses (a nowhere-resolving name maps to
            // the invalid face, every relation over it vacuous) — so the
            // existing consumer arms (the vacuous cover over typed
            // patterns, the exhaustiveness suppression) read the Invalid
            // scrutinee exactly like the oracle's chain.
            let lexeme = self.engine.strings.resolve(name).to_string();
            let base = self.flow().intern_name(&lexeme);
            self.flow().stage_call_result(
                offset,
                FlowTy {
                    base: FlowBase::Name(base),
                    nullable: false,
                },
                Vec::new(),
            );
        }
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
        // The compound operator's `void` read face (#3249): upstream
        // `_resolveOperator` reports `use_of_void_result` at the operator
        // when a read-modify-write target's READ type is `void` — `x.g += 1`
        // over a `void` getter (upstream Dart SDK source, pkg/analyzer/lib/
        // src/dart/resolver/assignment_expression_resolver.dart's
        // `operatorType != EQ` gate; the plain `x.g = 1` stays silent). The
        // target is the closest non-value row before the operator, the same
        // row the assignability faces read.
        let compound_void = {
            let engine = &mut self.engine;
            let limit = operator.range.offset;
            match engine.text_of(operator) {
                Some(text) if text != "=" => engine
                    .rows
                    .iter()
                    .rposition(|row| {
                        row.range.end() <= limit && !matches!(row.shape, RowShape::Value)
                    })
                    .is_some_and(|target| {
                        engine.receiver_static_ty(&engine.rows[target]) == Ty::Void
                    }),
                _ => false,
            }
        };
        if compound_void {
            let range = operator.range;
            if !self.engine.void_result_reported_at(range) {
                self.engine
                    .emit(SemanticCode::UseOfVoidResult, range, "", "");
            }
        }

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
            // A switch-expression RHS: the parser fires this event while
            // the RHS is still parsing, so the row heuristic below reads a
            // PARTIAL arm's face (a non-null call, say) — the official
            // static type is the arms' nullability-joined LUB, exactly the
            // face the flow layer's own value carries. Stand down on the
            // source spelling (`switch` right after the operator).
            let after_op = engine.skip_whitespace(operator.range.end());
            let is_switch_rhs = engine.byte_at(after_op) == Some(b's')
                && engine.byte_at(after_op + 1) == Some(b'w')
                && engine.byte_at(after_op + 2) == Some('i' as u8)
                && engine.byte_at(after_op + 3) == Some(b't')
                && engine.byte_at(after_op + 4) == Some(b'c')
                && engine.byte_at(after_op + 5) == Some(b'h')
                && engine.byte_at(after_op + 6).is_none_or(|b| {
                    b == b' ' || b == b'(' || b == b'\n' || b == b'\t' || b == b'\r'
                });
            if is_switch_rhs {
                return None;
            }
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
                .filter(|(_, row)| !matches!(row.shape, RowShape::Bare(_) | RowShape::Value))
                // The result row is the OUTERMOST expression — the row
                // with the largest END, not the last pushed: an
                // argument-bearing send's callee row spans through the
                // closing paren while an argument's own member row
                // (`recoveryElement: a ?? r.m2 ?? b` inside the parens)
                // trails it in push order with an earlier end. The
                // largest end picks the send's own face; a bare chain's
                // last member is both the largest end and the last push.
                .max_by_key(|(index, row)| (row.range.end(), *index))
                .or_else(|| {
                    engine.rows[target + 1..]
                        .iter()
                        .enumerate()
                        .filter(|(_, row)| row.range.offset > limit)
                        .last()
                })
                .map(|(index, _)| target + 1 + index)?;
            // A chained assignment's right side (`m = f.x = v`): the row
            // the search found is the INNER write's target member row
            // (`f.x`, its intent upgraded by `mark_assignment`) — its
            // declared READ type is not the assignment expression's value.
            // The value is the inner RHS — the region's topmost row
            // (`v`), which the staging computes from instead.
            let rhs_index = if engine.rows[rhs_index].intent == Intent::Write
                && let Some((index, _)) = engine.rows[target + 1..]
                    .iter()
                    .enumerate()
                    .filter(|(_, row)| row.range.offset > limit)
                    .last()
            {
                target + 1 + index
            } else {
                rhs_index
            };
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
                _ if rhs_row.intent == Intent::Invoke => {
                    // The write's context — the target's declared face —
                    // rides beside the row: a bare element constructor
                    // (`List.of(xs)` on `Iterable<String>? x`) instantiates
                    // its parameter from the assignment's downward
                    // inference, which the target's element args carry.
                    let context = engine.receiver_static_ty(&engine.rows[target]);
                    engine.callee_return_ty_for_write(&rhs_row, context)
                }
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
                                let rhs_offset = engine.rows.get(rhs_index).map(|r| r.range.offset);
                                let explicit = engine
                                    .call_type_args
                                    .iter()
                                    .find(|(row, _)| Some(*row) == rhs_offset)
                                    .map(|(_, entry)| entry.tys.clone());
                                match explicit {
                                    Some(tys) => {
                                        engine.boundary.args.borrow_mut().named(*name, false, &tys)
                                    }
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
            // `x = await e` writes the flattened operand type: the send
            // face answers `Future<T>`, the assignment's value is `T`.
            let from_ty = if from_ty != Ty::UNKNOWN
                && engine.await_operand_extents.iter().any(|&(start, end)| {
                    rhs_row.range.offset >= start
                        && rhs_row.range.end() <= end
                        && start > engine.rows[target].range.end()
                }) {
                engine.flatten(from_ty)
            } else {
                from_ty
            };
            // `x ??= e` writes the if-null assignment's OWN static type —
            // `LUB(NonNull(T1), T2)` (assignment_expression_resolver.dart
            // `_resolveTypes`, the spec's if-null rule), never the bare RHS
            // face the plain `=` carries. A `null` right side joins
            // `NonNull(T1)` into the declared `T1?` form (the RHS context
            // rule: the write face is the declared type), and a nullable
            // right side keeps its `?` on the joined head — so the flow
            // write face and every later read (`f(x)`'s argument ask
            // included) see the same static type the oracle's
            // `recordStaticType` records (#3061). The join mirrors
            // `ifnull_initializer_ty`'s name-level shape: the same head
            // keeps the right side's instantiation, different heads keep
            // the left head with the right side's nullability.
            let from_ty = if text == "??=" {
                let base = Engine::non_null_form(engine.receiver_static_ty(&engine.rows[target]));
                match (base, from_ty) {
                    (_, Ty::Null) => Engine::nullable_form(base),
                    (
                        Ty::Named { name: l, .. },
                        Ty::Named {
                            name: r,
                            nullable: rn,
                            args: ra,
                        },
                    ) if l == r => Ty::Named {
                        name: r,
                        nullable: rn,
                        args: ra,
                    },
                    (Ty::Named { name: l, args: la, .. }, Ty::Named { nullable: rn, .. }) => {
                        Ty::Named {
                            name: l,
                            nullable: rn,
                            args: la,
                        }
                    }
                    _ => from_ty,
                }
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
                        engine.staged_write_tys.remove(&engine.rows[target].name);
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
        // The index-write's argument check (#2937): `mark_assignment` just
        // landed the write form on the `[]` operator row (`=` → `Assign`,
        // `+=` → `Compound`), so the staged operand decides against the
        // `[]=` first formal (a plain `=`) or the `[]`/`[]=` pair (a
        // compound). Scan for the operator row the mark touched — a `[]`
        // carrying a staged `index_arg_faces` entry is the one.
        {
            let index_op = engine.intern("[]");
            let marked = engine.rows.iter().find_map(|row| {
                match (&row.intent, row.write_op) {
                    (Intent::Operator(op, bracket), write_op @ Some(_))
                        if *op == index_op
                            && engine.index_arg_faces.contains_key(&bracket.offset) =>
                    {
                        Some((bracket.offset, write_op))
                    }
                    _ => None,
                }
            });
            if let Some((bracket_offset, write_op)) = marked {
                engine.check_index_argument(bracket_offset, write_op);
            }
        }
        engine.check_assignment(operator);
        engine.cancel_dead_chains_on_write(operator);
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
            // The right operand's node range's start: the first token after
            // the operator (whitespace and comments ride no token).
            let rhs_offset = engine
                .tokens
                .get(operator.id as usize + 1)
                .map(|token| token.utf16_offset)
                .unwrap_or_else(|| operator.range.end());
            engine.begin_if_null(operator.range, rhs_offset);
        }
        if text == "==" || text == "!=" {
            // The comparison walk parks the left operand's row index (the
            // `_checkForInvariantNullComparison` mark).
            let mark = engine.rows.len();
            engine.null_cmp_groups.push((mark, operator.range));
        }
        if USER_OPERATORS.contains(&text) {
            // `>>>` / `>>>=` re-glue under `no-triple-shift` (`@dart=2.12`):
            // the scanner splits `>>>` into `>>`+`>` and `>>>=` into
            // `>>=`+`>`, and the parser does NOT re-glue (upstream Dart SDK
            // source, pkg/front_end `handleExperimentNotEnabled`). The
            // oracle's `undefined_operator '>>>'` anchors at the `>>`/`>>=`
            // position len 0 — the synthetic re-glued token's range. The
            // `>`'s own `mark_operator` is a no-op on an already-marked
            // row (the `_ => {}` arm keeps the `>>`/`>>=` consult), so the
            // re-glue rewrites the operand's intent to the oracle's
            // spelling. Byte-adjacent only: a spaced `>> >` is two real
            // operators.
            let reglue = if text == ">"
                && let Some(prev) = (operator.id as usize)
                    .checked_sub(1)
                    .and_then(|i| engine.tokens.get(i))
                && matches!(
                    prev.kind,
                    dartq_scanner::TokenKind::GtGt | dartq_scanner::TokenKind::GtGtEq
                )
                && prev.byte_offset + prev.byte_len
                    == engine.byte_of(operator.range.offset) as u32
            {
                Some(prev.utf16_offset)
            } else {
                None
            };
            if let Some(offset) = reglue {
                let op = engine.intern(">>>");
                let range = SourceRange { offset, length: 0 };
                // The operand is the last non-TypeUse row — the `>>`
                // left operand (`Intent::Operator(">>")` from the `>>`'s
                // own mark) or the `>>=`'s compound face (`ReadWrite`
                // from `mark_assignment`). Rewrite whichever intent the
                // re-glued `>>>` supersedes.
                if let Some(row) = engine
                    .rows
                    .iter_mut()
                    .rev()
                    .find(|row| !matches!(row.shape, RowShape::TypeUse(..)))
                {
                    row.intent = Intent::Operator(op, range);
                }
            } else {
                let op = engine.intern(text);
                engine.mark_operator(op, operator.range);
            }
        }
    }

    fn handle_binary_rhs_end(&mut self, operator: Token, rhs_last: Token) {
        self.flow().handle_binary_rhs_end(operator, rhs_last);
        // The `??` right operand's own node end: the dead-null-aware row's
        // extent latch (consumed by `end_if_null`).
        if self.engine.text_of(operator) == Some("??") {
            self.engine.if_null_rhs_end = Some(rhs_last.range.end());
        }
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
                    // #3240: the fold below reports BOTH operands of the
                    // lazy boolean operators (`fold_binary` runs
                    // `checkForUseOfVoidResult` on the pair), so a `void`
                    // right operand must not be classified here as well —
                    // `classify_condition` emits the same
                    // `use_of_void_result` a second time while the oracle
                    // answers one row for `flag && v()`. The operand's
                    // other faces never fire on a `void` row
                    // (`known_non_bool(void)` is false and no nullable
                    // `void` spelling exists), so the fold's single report
                    // is the whole decision.
                    if engine.operand_static_ty(&right) != Ty::Void {
                        engine.classify_condition(&right);
                    }
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
            // The void-operand use (upstream `visitPrefixExpression` —
            // `operatorType != BANG` → `checkForUseOfVoidResult(operand)` at
            // error_verifier.dart:1727): `-v()`/`~v()`/`++v()`/`--v()`
            // consume the void operand's value and report at its span. The
            // operand's static type resolves the invoke's `void` return —
            // `receiver_static_ty` answers the operator method's receiver,
            // not the consumed value. The anchor is the operand's
            // use-site span: `-f()` on a `void Function()` formal reports
            // the whole `f()`, a declared-method send its name token.
            if op != UnOp::Not && engine.operand_static_ty(&row) == Ty::Void {
                let at = engine.use_site_range(&row);
                engine.emit(SemanticCode::UseOfVoidResult, at, "", "");
            }
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
                // An `await` operand's close-time rewrite read an `Unknown`
                // row (member rows resolve in the later pass), so the
                // declared return standing here is the RAW face
                // (`FutureOr<bool>`) — the extent re-applies the flatten
                // exactly like the var-inferred bind's deferred face
                // (#2506): `!await f()` negates the flattened `bool`.
                let ty = if ty != Ty::UNKNOWN
                    && engine
                        .await_operand_extents
                        .iter()
                        .any(|&(start, end)| row.range.offset >= start && row.range.end() <= end)
                {
                    engine.flatten(ty)
                } else {
                    ty
                };
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
        self.flow().handle_unary_prefix_assignment_expression(operator);

        let engine = &mut self.engine;
        // The prefix `++`/`--` junction (`increment_or_decrement_resolver.
        // dart:273`, the prefix face): the operand's row is complete — the
        // parser fires this event after the operand parses.
        if let Some(row) = engine.operand_row().cloned() {
            let at = row.range;
            let receiver_ty = engine.receiver_static_ty(&row);
            engine.stage_receiver_of_type_never(at, receiver_ty);
            // The void-operand use (upstream `visitPrefixExpression` —
            // `operatorType != BANG` → `checkForUseOfVoidResult(operand)`):
            // `++v()`/`--v()` consume the void operand's value. The anchor
            // is the operand's use-site span (a `void Function()` callee's
            // send reports the whole `v()`).
            if engine.operand_static_ty(&row) == Ty::Void {
                let at = engine.use_site_range(&row);
                engine.emit(SemanticCode::UseOfVoidResult, at, "", "");
            }
        }
        // The index operand's compound write (`++a[0]` — the same
        // `resolveForWrite(hasRead: true)` consult the postfix face runs):
        // both `[]` spellings check.
        self.mark_index_compound_write();
        let _ = operator;
    }

    fn handle_unary_postfix_assignment_expression(&mut self, operator: Token) {
        self.flow().handle_unary_postfix_assignment_expression(operator);

        let engine = &mut self.engine;
        engine.const_capture.unary_postfix(operator.range);
        // The postfix `++`/`--` junction (`increment_or_decrement_resolver.
        // dart:273`, the postfix face): the operand's row precedes the
        // operator token.
        if let Some(row) = engine.operand_row().cloned() {
            let at = row.range;
            let receiver_ty = engine.receiver_static_ty(&row);
            engine.stage_receiver_of_type_never(at, receiver_ty);
            // The void-operand use (upstream `visitPostfixExpression` —
            // `operatorType != BANG` → `checkForUseOfVoidResult(operand)`):
            // `v()++`/`v()--` consume the void operand's value. The anchor
            // is the operand's use-site span.
            if engine.operand_static_ty(&row) == Ty::Void {
                let at = engine.use_site_range(&row);
                engine.emit(SemanticCode::UseOfVoidResult, at, "", "");
            }
        }
        // The index operand's compound write (`a[0]++` — upstream Dart SDK
        // source, postfix_expression_resolver.dart:35-37
        // `resolveForWrite(hasRead: true)`): the `[]` operator row carries
        // the compound write state the `+=` junction's marking already
        // sets, so the `[]`/`[]=` pair reports.
        self.mark_index_compound_write();
    }

    fn begin_conditional_expression(&mut self, question: Token) {
        self.flow().begin_conditional_expression(question);

        let engine = &mut self.engine;
        engine.nsm_poison();
        // The conditional's condition completed at the `?` — its operand's
        // flow-aware static type is `bool?` — the unchecked-use face
        // (#1351). The enclosing statement's condition window
        // (`cond_mark`/`cond_case`) belongs to that statement: a ternary
        // nested in its condition (`if (f(c ? a : b) case var r?)`) must
        // neither consume the window nor take its anchor, or the if-case
        // routing in `begin_pattern` never arms and the scrutinee is
        // checked as a plain condition.
        let outer_mark = engine.cond_mark.take();
        let outer_case = std::mem::take(&mut engine.cond_case);
        engine.check_condition_operand();
        engine.cond_mark = outer_mark;
        engine.cond_case = outer_case;
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
        // The verdict row anchors the whole `is` expression — the oracle's
        // `_checkAllTypeChecks` reports `.at(node)` (`IsExpressionImpl`):
        // the operand's start through the written type's end, never the
        // bare operand (`null is! Null` spans all 13 columns). The type's
        // extent rides the armed slot (head + `?`) extended by this
        // target's closed type-arguments group (`List<int>` ends at `>` —
        // `handle_type` carries the head token only).
        //
        // The `this`/`super` keyword pushes no row, so the keyword face
        // resolves at the operator's own left — a stale row below must
        // not stand in as the operand, and the span anchors at the
        // keyword.
        let keyword_operand = engine.keyword_operand_ty(is_operator.range.offset);
        // The literal faces push the operand's row at the closing bracket;
        // the oracle's `.at(node)` anchors at the node offset — the
        // opening bracket. Consumed here: any face's write survives only
        // until the first verdict reads it, and a row at another offset
        // (a member access over the group, a bare read) never matches.
        let literal_open = engine.literal_row_open.take();
        let type_end = |mut end: u32| -> u32 {
            if let Some(span) = engine.armed_ty_span
                && span.offset >= is_operator.range.end()
            {
                end = end.max(span.end());
            }
            if let Some(args_end) = engine.type_args_group_end
                && args_end > end
                && args_end > is_operator.range.end()
            {
                end = args_end;
            }
            // The written bracket type's own end — a function type's
            // trailing formal list (`void Function(int, [List<T>])
            // Function(int x)` ends at the outer formals' `)`, the armed
            // slot stops at the last formal's head token) or a record
            // type's closer (`(int, int)`): the TypeAnnotation's endToken,
            // the same extent the oracle's `.at(node)` span takes. The
            // enclosing call's paren never enters — the slot ends at the
            // type production's own closer.
            if let Some(span) = engine.written_ty_span
                && span.offset >= is_operator.range.end()
                && span.end() > end
            {
                end = span.end();
            }
            // The prefixed target's continuation (`is lib.T1`): `handle_type`
            // anchors the qualified head at the prefix token (the parser's
            // `PrefixedType.parseType` fires `handleType` there, never at the
            // continuation), so the armed slot ends at `lib` while the
            // oracle's TypeAnnotation spans through `T1`. The continuation
            // sits in the source bytes directly after the head — one
            // `.ident` step (a `<` group rides `type_args_group_end`; a
            // trailing `?` stays out, the parser never arms the qualified
            // question and a conditional `?` after the target is not the
            // type's).
            if let Some(span) = engine.armed_ty_span
                && span.offset >= is_operator.range.end()
            {
                let mut p = span.end();
                while matches!(engine.byte_at(p), Some(b' ' | b'\t' | b'\r' | b'\n')) {
                    p += 1;
                }
                if engine.byte_at(p) == Some(b'.') {
                    p += 1;
                    while matches!(engine.byte_at(p), Some(b' ' | b'\t' | b'\r' | b'\n')) {
                        p += 1;
                    }
                    let ident = p;
                    while engine
                        .byte_at(p)
                        .is_some_and(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'$')
                    {
                        p += 1;
                    }
                    if p > ident {
                        end = end.max(p);
                    }
                }
            }
            end
        };
        let verdict_at = if let Some((_, range)) = keyword_operand {
            type_end(range.end())
                .checked_sub(range.offset)
                .map(|length| SourceRange {
                    offset: range.offset,
                    length,
                })
                .unwrap_or(range)
        } else {
            engine.operand_row().map_or(is_operator.range, |row| {
                let start = literal_open
                    .filter(|&(close, _)| close == row.range.offset)
                    .map_or_else(
                        || operand_expr_head(engine, row.range.offset),
                        |(_, open)| open,
                    );
                let end = type_end(row.range.end());
                SourceRange {
                    offset: start,
                    length: end - start,
                }
            })
        };
        // The prefixed checked head (`is p.T`): `handle_type` arms the
        // dot-joined spelling (`Named{"p.T"}` — the intern table's
        // one-head convention for prefixed annotations), which the
        // verdict's `InvalidType` gate cannot scope — a joined name is
        // no bare name, so `is p.T` would answer `None` where the
        // oracle's resolved NamedType (`TypeAnnotation.type` = `T`)
        // decides. The continuation segment carries the resolved local
        // head: substitute it for the verdict alone — the armed slot
        // keeps the joined spelling every later face (declaration
        // binds, span latches) reads.
        let checked_saved = engine.armed_ty;
        if let Some(Ty::Named {
            name,
            nullable,
            args,
        }) = checked_saved
            && let Some((_, local)) = engine.strings.resolve(name).split_once('.')
            && !local.is_empty()
        {
            engine.armed_ty = Some(Ty::Named {
                name: engine.intern(local),
                nullable,
                args,
            });
        }
        // The operand faces `type_check_operand_ty`'s walk cannot reach —
        // its `callee_return_ty` answers callables (the declaration
        // table's `return_ty`) and its untyped arms receiver shapes, never
        // a bare container head. Upstream's `leftNode.typeOrThrow` reads
        // the operand node's own `staticType`: a bare container name in
        // value position is a `TypeLiteral` (`C is T` — `TypeLiteralImpl.
        // staticType` = `Type`, and `p.C` the same through the prefixed
        // lookup), while an unnamed-constructor send (`D()`, `D(1)`,
        // `p.C()`) types the created instance (`InstanceCreationExpression.
        // staticType` — the bare `D` callee resolves the default
        // constructor, a `C.new`/`D.named` selector already resolves
        // through `Receiver::Created`'s own face). The verdict reads
        // `operand.ty` first, so re-typing the row here reaches it without
        // touching the engine's operand walk; a genuinely-undecidable
        // operand (an unresolvable head, a generic ctor's raw name) keeps
        // the unknown suppress — the under-report direction.
        if keyword_operand.is_none()
            && let Some(index) = engine
                .rows
                .iter()
                .rposition(|row| !matches!(row.shape, RowShape::TypeUse(..)))
            && engine.rows[index].ty == Ty::UNKNOWN
        {
            // The `Member{Prefix}` prefixed-container test (`prefixed_
            // decl_is_container`'s contract): a scoped declaration that is
            // no variable, function, prefix, or type parameter is a
            // container spelling — `p.C` reads `Type`, `p.C(...)` the
            // created instance; a prefixed call (`p.f()`) is neither and
            // keeps `callee_return_ty`'s declared-return answer.
            let prefixed_container = |prefix: StringId, name: StringId| -> bool {
                let Lookup::Found(NamespaceEntry::Decl(file)) =
                    engine.namespaces.lookup_prefixed(engine.graph, engine.lib, prefix, name)
                else {
                    return false;
                };
                engine.index.get(file).and_then(|f| f.decl(name)).is_some_and(
                    |decl| {
                        !matches!(
                            decl.kind,
                            MicroKind::TopLevelVar
                                | MicroKind::TopLevelFn
                                | MicroKind::Prefix
                                | MicroKind::TypeParam
                        )
                    },
                )
            };
            let row = &engine.rows[index];
            // `MicroKind::Extension` collapses `extension E` and
            // `extension type Ext` into one row kind (`Bare(Container.
            // Extension)`); only the extension-type spelling is a creation
            // — a plain `extension`'s `E(1)` is an invalid call upstream
            // whose operand types `InvalidType` (the oracle stays silent
            // on `Ex(1) is Ex`), so it keeps the unknown suppress. The
            // read face (`C is Type`) is a `TypeLiteral` either way.
            let retyped = match &row.shape {
                RowShape::Bare(BareInfo::Container { name, kind }) => {
                    if row.intent == Intent::Invoke {
                        if *kind != ContainerKind::Extension
                            || engine.container_kind_of(*name)
                                == Some(ContainerKind::ExtensionType)
                        {
                            Some(Ty::named(row.name))
                        } else {
                            None
                        }
                    } else {
                        Some(Ty::named(engine.intern("Type")))
                    }
                }
                RowShape::Member {
                    recv: Receiver::Prefix { name: prefix },
                } if prefixed_container(*prefix, row.name) => {
                    if row.intent == Intent::Invoke {
                        Some(Ty::named(row.name))
                    } else {
                        Some(Ty::named(engine.intern("Type")))
                    }
                }
                // The unnamed-creation fold's own row (`D()`, `p.C()`,
                // `Ext(1)`) — its `ty` stays open while the send-close
                // inference solves the written arguments, but the
                // `InstanceCreationExpression` reads `D`/`Ext` regardless
                // (`end_parenthesized_expression`'s same arm): the
                // container head is the created type. The `Extension` kind
                // carries both `extension E` and `extension type Ext`
                // (`MicroKind::Extension` collapses them); only the
                // extension-type spelling is a creation, so the same
                // `container_kind_of` discrimination the bare arm reads
                // decides it — a plain `extension`'s `(` keeps the unknown
                // suppress (`Ex(1) is Ex` stays silent upstream).
                RowShape::Receiver(Receiver::Created { name, kind, .. }) => {
                    if *kind != ContainerKind::Extension
                        || engine.container_kind_of(*name)
                            == Some(ContainerKind::ExtensionType)
                    {
                        Some(Ty::named(*name))
                    } else {
                        None
                    }
                }
                _ => None,
            };
            if let Some(ty) = retyped {
                engine.rows[index].ty = ty;
            }
        }
        engine.check_type_check(verdict_at, keyword_operand, not.is_some());
        engine.armed_ty = checked_saved;
        // An `is` test is const-transparent (upstream `visitIsExpression`,
        // evaluation.dart:1106 — no arm of its own): the operand's own
        // verdict is the result, so the operand node stays as the
        // expression's root — `o is int` reports at `o` (the oracle's
        // deepest-non-constant anchor), `1 is int` stays silent. No leaf
        // is pushed: a shadowing node would discard the operand's
        // non-constant face.
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
            // A `?.`-shorted chain (`a?.b as S`) reads nullable: the
            // whole null-shorting node's static type is the selector's
            // type made nullable, never the bare member face.
            let operand = engine.operand_row().map_or(Ty::Unknown, |row| {
                let shorted = engine.null_aware_members.contains(&row.range.offset);

                if shorted && row.ty != Ty::UNKNOWN {
                    Engine::nullable_form(row.ty)
                } else {
                    row.ty
                }
            });
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
            // The cast's whole span pairs with its resolved target — the
            // staged teardown's rewrite face (a check anchored inside the
            // cast reads the target, the expression's static type).
            if let Some(ty) = cast_ty.filter(|ty| *ty != Ty::UNKNOWN) {
                engine.cast_rewrites.push((cast_span, ty));
            }
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
        // An `as` cast is const-transparent like `is` (`visitAsExpression`
        // carries no arm of its own): the operand's verdict is the result,
        // so the operand node stays as the expression's root — `o as int`
        // reports at `o`, `1 as int` stays silent. No leaf is pushed: an
        // opaque shadow would discard the operand's non-constant face.
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
        // The void-operand use (upstream `visitIsExpression` →
        // `checkForUseOfVoidResult(node.expression)`): `x is T` on a
        // `void`-typed `x` reports at the operand's use-site span — the
        // operand is the row just below the type-argument window
        // (`is_type_mark` latches `rows.len()` after it parses).
        if let Some(mark) = engine.is_type_mark
            && let Some(operand) = mark
                .checked_sub(1)
                .and_then(|index| engine.rows.get(index))
                .cloned()
            && engine.operand_static_ty(&operand) == Ty::Void
        {
            let range = engine.use_site_range(&operand);
            engine.emit(SemanticCode::UseOfVoidResult, range, "", "");
        }
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
        // A redirecting `this(...)`/`this.named(...)` list is the same
        // site (`RedirectingConstructorInvocation`, same verifier arm) —
        // never a method invocation for the const evaluator. The redirect
        // is a whole initializer, so its list is the only open level: a
        // nested creation (`this._(const V())`) inherits the selector row
        // as its head and must keep the generic close.
        let init_super = self.engine.initializer_depth > 0
            && (self.engine.init_super_callee().is_some()
                || matches!(&self.engine.arg_levels[..], [level]
                    if self.engine.init_invocation_level(level)));
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
                // bound as opaque names (the [`InitRhsLookup`] overlay) — a
                // const-ctor formal passed through `: super(f)` /
                // `: this.other(f)` stays silent, a genuinely non-constant
                // argument still collects.
                let mut spans = Vec::new();
                if self.engine.in_const_ctor {
                    let formals = self.engine.ctor_init_formals();
                    // Wildcard formals declare no readable name — they drop
                    // out of the env so `super(_)`/`assert(_ > 0)` leaves
                    // `_` unresolved (the oracle reports `invalid_constant`
                    // beside the row's `undefined_identifier`, #3400).
                    let mut env: FxHashMap<StringId, CtorBinding> = FxHashMap::default();
                    for name in formals {
                        if !self.engine.is_wildcard(name) {
                            env.insert(name, CtorBinding::Opaque);
                        }
                    }
                    // [`InitRhsLookup`] also carries the implicit-`this`
                    // member rule — `super(g)` over instance `g` collects
                    // the same `invalid_constant` the field-initializer
                    // scan reports.
                    let scope = InitRhsLookup {
                        engine: &self.engine,
                        env: &env,
                    };
                    for arg in &closed.args {
                        collect_not_potentially_constant(arg, &scope, &mut spans);
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
                    CallClass::ValueCall => {
                        // The resolver rewrote this call to a
                        // `FunctionExpressionInvocation` (see
                        // [`CallClass::ValueCall`]): `ConstantVisitor` has no
                        // visitor for it, so `visitNode`'s `genericError`
                        // returns `invalidConstant` — NOT on `_reportError`'s
                        // allow list, so the enclosing site's default code
                        // reports at the invocation. `const_eval_method_invocation`
                        // never fires for this shape.
                        engine.const_capture.push_non_constant(closed.span);
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
                                        ..
                                    })
                                    | RowShape::Bare(BareInfo::Container {
                                        name,
                                        kind: ContainerKind::Class,
                                    }) => Some(*name),
                                    // An unindexed SDK type head (`Stream`
                                    // re-exported through `dart:core`)
                                    // resolves as a plain type-valued decl;
                                    // the frozen [`CORE_CONST_CTOR`] filter
                                    // below decides its selector.
                                    RowShape::Bare(BareInfo::Plain { .. })
                                        if engine.type_resolution(receiver.name).0
                                            == TypeResolution::Type =>
                                    {
                                        Some(receiver.name)
                                    }
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
        // The annotation's argument-assignability face (#2937): the same
        // pending-metadata condition as the arity registration above — a
        // metadata creation's invoke slot carries no callee link, so the
        // generic `close_arguments` walk never pairs the arguments against
        // the annotated constructor's formals. Resolve the constructor
        // here and run the argument walk (upstream Dart SDK source,
        // resolver.dart:1887 — `visitAnnotation`'s
        // `checkForArgumentTypesNotAssignableInList`).
        if let Some(level) = engine.arg_levels.last()
            && engine.invokes.last().and_then(|slot| *slot).is_none()
            && !level.enum_const
            && let Some(annotation) = engine.pending_annotations.last().map(|a| a.name)
        {
            let args = level.args.clone();
            engine.check_annotation_arguments(annotation, &args);
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
        // #3419 — a return directly inside an anonymous-method block body
        // answers to the body's own inferred return type upstream, never
        // the enclosing executable: both the enclosing check and the
        // enclosing closure-return attribution stand down.
        if engine.anon_body_owns_return(begin.range.offset) {
            return;
        }
        if !has_expression {
            // Bare `return;` is a `void` expression in the oracle's matrix.
            engine.check_return(Ty::Void, end.range, false);
            return;
        }
        if let Some(mark) = mark
            && let Some((ty, literal, range)) = engine.single_row_since(mark)
        {
            engine.check_return(ty, range, literal);
            engine.note_closure_return(mark);
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
        // skip lives only on this event. The innermost executable's own
        // return decides: a literal (`ret: None`) asks its matched context
        // (`(x) => 3` inside `void f` is no void body — the context's
        // return `R` is the `=>` body's `T`, and `T_v`'s `void`-ness is
        // what the official early return reads); a context-free literal
        // answers `dynamic`, which the gate treats as non-void — the same
        // posture the declared path carries.
        let void_return = engine
            .frames
            .iter()
            .rev()
            .find_map(|f| match &f.kind {
                FrameKind::Executable { ret: Some(ret), .. } => Some(ret.ty == Ty::Void),
                FrameKind::Executable { ret: None, .. } => {
                    let ctx_ret = engine.closure_context_return();
                    Some(ctx_ret == Some(Ty::Void))
                }
                _ => None,
            })
            .unwrap_or(false);
        if !void_return && let Some((ty, literal, range)) = engine.single_row_since(mark) {
            engine.check_return(ty, range, literal);
            engine.note_closure_return(mark);
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
        // A directly-following `..` reopens the cascade (one begin/end per
        // section): the previous close's value stand-in must not become
        // the reopened cascade's target — pop it, so the span latch below
        // and every section merge read the real target row exactly as a
        // fresh cascade would. The reopened cascade's stack top is the
        // PREVIOUS SECTION'S RHS leftover, never the target: its captured
        // type comes from the last close's stash (the type twin of
        // `cascade_last`), not the leftover row.
        let mut reopened = false;
        if engine.rows.last().is_some_and(|row| {
            matches!(row.shape, RowShape::Value)
                && engine.cascade_value_rows.last() == Some(&row.range.offset)
        }) {
            engine.rows.pop();
            engine.cascade_value_rows.pop();
            reopened = true;
        }
        let target_ty = if reopened {
            engine.cascade_last_ty.unwrap_or(Ty::UNKNOWN)
        } else {
            engine.rows.last().map_or(Ty::UNKNOWN, |row| {
                // An unnamed creation's target row (`B()..m`) carries its
                // face in the `Created` shape — the row's own ty stays
                // unset — so the container's instance type answers before
                // the static fallback.
                if let RowShape::Receiver(Receiver::Created { name, .. }) = &row.shape {
                    return Ty::named(*name);
                }
                let ty = Engine::receiver_row_ty(row);
                if ty == Ty::UNKNOWN {
                    engine.receiver_static_ty(row)
                } else {
                    ty
                }
            })
        };
        engine.cascade_target_tys.push((target_ty, operator.range));
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
        // The cascade target's own read row: every section selector of
        // this cascade re-reads it, and the promoted-receiver latch keys
        // the sections on this span (the flow's promotion of the target
        // read answers each section's receiver verdict).
        if let Some(target) = engine.rows.last() {
            let span = engine.cascade_continuation_target(operator.range.offset, target.range);
            engine.cascade_target_spans.push(span);
            engine.cascade_open.push((operator.range.offset, span));
        }
    }

    fn end_cascade(&mut self) {
        self.flow().end_cascade();

        let engine = &mut self.engine;
        engine.cascade_target_spans.pop();
        engine.cascade_last = engine.cascade_open.pop();
        // The cascade expression's value is its TARGET's static type
        // (`CascadeExpression`'s `staticType`), never the last section's
        // result: the value row pushed here stands for the whole
        // expression, so the continuation (`(b..f).m`'s `.m`) resolves on
        // the target's face. The section rows stay untouched — their own
        // faces (member misses, latch settlements) report through the
        // pass-2 scans.
        if let Some((target_ty, op_range)) = engine.cascade_target_tys.pop() {
            engine.cascade_last_ty = (target_ty != Ty::UNKNOWN).then_some(target_ty);
            if target_ty != Ty::UNKNOWN {
                engine.push_row(Row {
                    name: engine.intern(""),
                    range: op_range,
                    qualified: false,
                    shape: RowShape::Value,
                    intent: Intent::Read,
                    write_op: None,
                    ty: target_ty,
                });
                engine.cascade_value_rows.push(op_range.offset);
            }
        }
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
        // `e1[e2]` is never a constant expression — upstream's constant
        // verifier flags the whole IndexExpression (`const x = a[0]`
        // reports at `a[0]`, receiver start through `]`), so the capture
        // pushes a non-constant leaf over the whole access rather than
        // deferring through an opaque one.
        let index_span = engine
            .rows
            .iter()
            .rposition(|row| row.range.end() <= open.range.offset)
            .map_or(bracket, |pos| SourceRange {
                offset: engine.rows[pos].range.offset,
                length: close.range.end() - engine.rows[pos].range.offset,
            });
        engine.const_capture.push_non_constant(index_span);
        // The receiver: the closest row wholly before the `[`.
        let Some(pos) = engine
            .rows
            .iter()
            .rposition(|row| row.range.end() <= open.range.offset)
        else {
            return;
        };
        // The `void`-receiver face (#3249): an index access consumes the
        // receiver's value, and the oracle anchors the report on the
        // bracket pair (`_reportUnresolvedIndex`, property_element_resolver
        // .dart) for the read and the write form alike. Any other receiver
        // type is a no-op.
        {
            let row = &engine.rows[pos];
            let ty = Engine::receiver_row_ty(row);
            let ty = if ty == Ty::UNKNOWN {
                engine.receiver_static_ty(row)
            } else {
                ty
            };
            if ty == Ty::Void && !engine.void_result_reported_at(bracket) {
                engine.emit(SemanticCode::UseOfVoidResult, bracket, "", "");
            }
        }
        // `?..[i]`'s index section carries no `?` of its own — the `?`
        // rode the `?..` operator, so the pending cascade-op range stands
        // in as the question token's span (consumed: only the FIRST
        // section is null-aware).
        let question_range = match question {
            Some(q) => Some(q.range),
            None => engine.cascade_null_aware_op.take(),
        };
        // The index-continuation climb
        // (`_verifyUnassignedSimpleIdentifier`'s
        // `while (parent is IndexExpression)`, dead_code_verifier.dart
        // :492-497): an index access whose operator sits inside-or-at a
        // latched chain's end is the outer access the dead span climbs to
        // — `l?[0][0]` reports `?.[0][0]` whole, `l?[0]?[1]` keeps one row
        // at the inner `?[` (the second `?` lands exactly at the inner
        // extent's end). The climb is kind-agnostic for the plain faces (a
        // `?.`/`?[` root's climb covers every access parent); a cascade
        // root grows too — `a?..[0][1]` keeps `first` an IndexExpression
        // whose `period` is the `?..`, so the bare-body gate holds
        // (`verifyCascadeExpression`'s index arm). The bracket's own close
        // is the grown end; the operator offset is the containment key
        // (`?` if present, `[` otherwise).
        let index_op_offset = question_range.map_or(open.range.offset, |q| q.offset);
        if let Some(index) = engine
            .dead_null_chains
            .iter()
            .position(|(op, end, ..)| *op <= index_op_offset && index_op_offset <= *end)
        {
            engine.dead_null_chains[index].1 =
                engine.dead_null_chains[index].1.max(close.range.end());
        }
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
            // The `?[`/`?..` operator is the anchor, not the receiver row.
            // The index arm anchors `operator.offset` through the bracket's
            // own end (`_checkForUnnecessaryNullAware`'s `atOffset(operator
            // .offset, operator.next!.end - operator.offset)` — the `?[`'s
            // `?` token's `next` is the `[`, so the range covers both),
            // while the `?..[` cascade's `?` stands in for the whole `?..`
            // (its span already runs `?` through `..`).
            let op_range = if question.is_some() {
                SourceRange {
                    offset: question_range.offset,
                    length: open.range.end() - question_range.offset,
                }
            } else {
                question_range
            };
            let recv = engine.rows[pos].range;
            // The parenthesized receiver
            // (`_verifyUnassignedSimpleIdentifier`'s
            // `target.unParenthesized`, dead_code_verifier.dart:486-490):
            // `(l)?[0]` folds `l` into a group row — the `local_hits` entry
            // stays at `l`'s own offset inside the group's span, so the
            // bare-receiver lookup misses. Re-spell the receiver onto the
            // inner local read: the staged null-aware face and the dead
            // chain both key on the inner span, never the parens.
            let recv = if !engine.local_hits.contains_key(&recv.offset)
                && !engine.rows[pos].qualified
                && matches!(engine.rows[pos].shape, RowShape::Bare(BareInfo::Plain { .. }))
                && engine.strings.resolve(engine.rows[pos].name).is_empty()
            {
                engine
                    .local_hits
                    .iter()
                    .find(|&(off, _)| recv.offset < *off && *off < recv.end())
                    .and_then(|(&inner_offset, _)| {
                        engine
                            .tokens
                            .as_slice()
                            .get(engine.tokens.as_slice().partition_point(|t| {
                                t.utf16_offset < inner_offset
                            }))
                            .filter(|t| t.utf16_offset == inner_offset)
                            .map(|token| {
                                let raw = &engine.source[token.byte_range()];
                                SourceRange {
                                    offset: inner_offset,
                                    length: raw
                                        .chars()
                                        .map(char::len_utf16)
                                        .sum::<usize>()
                                        as u32,
                                }
                            })
                    })
                    .unwrap_or(recv)
            } else {
                recv
            };
            let kind = if question.is_some() {
                NullAwareKind::Index
            } else {
                NullAwareKind::Cascade
            };
            // The bound-aware strictly face: an in-scope parameter
            // receiver decides through its written bound (`T extends C?`
            // keeps `?[` legal, `T extends C` reports).
            if engine.strictly_non_nullable(ty) {
                // The short-circuit variant — an earlier `?.`/`?[`/`?..` in
                // the same chain already shorted this receiver, so the
                // operator reports `after_short_circuit` and carries the
                // causing operator as a context row (`x?[0]?[1]`'s second
                // `?[`, `s?.a?[0]`'s `?[`). `indexExpression` is one of the
                // `canParticipateInShortCircuiting` kinds
                // (error_verifier.dart :7679-7740).
                let prior = engine
                    .dead_null_chains
                    .iter()
                    .find(|(op, end, ..)| *op <= op_range.offset && op_range.offset <= *end)
                    .map(|(op, _, _, k, _)| (*op, *k));
                let (op, replacement) = kind.lexemes();
                if let Some((prior_offset, prior_kind)) = prior {
                    let (prior_lexeme, prior_len) = match prior_kind {
                        crate::diagnose::types::DeadChainKind::Member
                        | crate::diagnose::types::DeadChainKind::Index => ("?.", 2u32),
                        crate::diagnose::types::DeadChainKind::CascadeMember
                        | crate::diagnose::types::DeadChainKind::CascadeIndex => ("?..", 3u32),
                    };
                    let code = SemanticCode::InvalidNullAwareOperator;
                    engine.report.diagnostics.push(
                        Diagnostic::new(
                            code.severity(),
                            code.code_info(),
                            format!(
                                "The receiver can't be 'null' because of short-circuiting, so the null-aware operator '{op}' can't be used."
                            ),
                            engine.file,
                            op_range,
                            Some(format!(
                                "Try replacing the operator '{op}' with '{replacement}'."
                            )),
                        )
                        .with_context(vec![ContextMessage {
                            message: SharedStr::from(format!(
                                "The operator '{prior_lexeme}' is causing the short circuiting."
                            )),
                            file: engine.file,
                            range: SourceRange {
                                offset: prior_offset,
                                length: prior_len,
                            },
                        }]),
                    );
                } else {
                    engine.emit3(
                        SemanticCode::InvalidNullAwareOperator,
                        op_range,
                        op,
                        replacement,
                        "",
                    );
                }
            } else if engine.local_hits.contains_key(&recv.offset) {
                engine.null_aware_staged.push((recv, op_range, kind));
                // The dead-chain receiver nullability reads the written
                // `?` face AND the `FutureOr` argument's own nullability
                // (`FutureOr<int?>` is potentially nullable upstream —
                // `?[i]` reads through it — while `FutureOr<int>` stays
                // non-nullable; type_system.dart's `FutureOr` arm).
                if Engine::is_nullable_ty(ty) || engine.future_or_potentially_nullable(ty) {
                    // The `c1?[0]` dead-chain candidate: a null-aware index
                    // over a local's nullable read, resolved at teardown
                    // (`_verifyUnassignedVariable`'s index face). The root
                    // kind fixes the verifier face: a plain `?[`/`?.[` answers
                    // `verifyReceiverIndexExpression`'s climb, a `?..[`
                    // cascade section answers `verifyCascadeExpression`'s
                    // bare-body gate.
                    let chain_kind = if question.is_some() {
                        crate::diagnose::types::DeadChainKind::Index
                    } else {
                        crate::diagnose::types::DeadChainKind::CascadeIndex
                    };
                    engine.dead_null_chains.push((
                        question_range.offset,
                        close.range.end(),
                        recv,
                        chain_kind,
                        false,
                    ));
                }
            }
        }
        // Fold (`a[i]`, `c.j[i] = v`): the index arguments leave the stack —
        // the receiver row carries the `[]` access with the bracket span, and
        // a value row stands for the result so compound-expression detection
        // (argument types, initializer checks) still sees one opaque row. An
        // INVOKED receiver row (`getFragments(p)[i]` — a call result indexed
        // directly) folds the same way: without the fold, the operand row
        // (the literal index, an `int`) stays topmost and the next member
        // merge would classify IT as the receiver — the chained send then
        // reports against `int` instead of the element type. A receiver
        // whose own resolution left no name row (groups) stays unfolded —
        // its operator checks suppress anyway.
        if matches!(
            engine.rows[pos].shape,
            RowShape::Bare(_) | RowShape::Member { .. } | RowShape::Receiver(_)
        ) && matches!(engine.rows[pos].intent, Intent::Read | Intent::Invoke)
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

            // The index-argument check staging (#2937): the operand rows
            // fold away below, so the index argument's static type and span
            // — and the receiver's named head — ride `index_arg_faces`
            // keyed by the bracket's open `[`. The read/write `[]`/`[]=`
            // element pair resolves at emission time once the write form
            // finalizes (a `=`/`+=`/`++` mark sets the operator row's
            // `write_op` after this fold); a pure `a[i]` drains as the
            // read-only face at the unit's end (upstream Dart SDK source,
            // `checkIndexExpressionIndex`,
            // error_detection_helpers.dart:260-289).
            {
                // The index argument is the outermost operand row — the
                // last stacked expression inside the bracket (a member
                // merge, a send fold, or a bare/literal operand all leave
                // exactly one row covering the argument's span).
                let arg = engine
                    .rows
                    .get(pos + 1..)
                    .and_then(|rows| {
                        rows.iter()
                            .filter(|row| {
                                open.range.offset <= row.range.offset
                                    && row.range.end() <= close.range.end()
                            })
                            .max_by_key(|row| (row.range.end(), row.range.offset))
                    });
                if let Some(arg) = arg {
                    let index_ty = engine.expression_row_ty(arg);
                    let literal = Engine::primitive_literal(arg, &engine.members.primitive_ids);
                    let recv = match engine.fold_alias_ty(receiver_ty) {
                        Ty::Named { name, .. } => Some(name),
                        _ => None,
                    };
                    engine.index_arg_faces.insert(
                        open.range.offset,
                        IndexArgFace {
                            index_ty,
                            index_range: arg.range,
                            literal,
                            recv,
                        },
                    );
                }
            }

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
            // A `?.`-shorted receiver (`a?.l[i]`) shorts the index result
            // too: the whole chain node reads nullable, and the chain
            // continuation (`a?.l[i].t`) joins through the value row.
            let shorted = !engine.in_cascade
                && engine
                    .null_aware_members
                    .contains(&engine.rows[pos].range.offset);

            if shorted {
                engine.null_aware_members.insert(close.range.offset);
            }
            let indexed = engine.rows[pos].range;
            engine
                .index_result_recvs
                .insert(close.range.offset, (indexed, question.is_some()));
            engine.push_value_row(close);
            if result_ty != Ty::UNKNOWN
                && let Some(result) = engine.rows.last_mut()
            {
                result.ty = result_ty;
            }
        }
    }

    /// The unit's end (`endCompilationUnit`): every index expression's write
    /// form is final — a `[]` operator row an assignment/compound mark never
    /// touched is a pure read. The staged `index_arg_faces` entries that
    /// survive (no `=`/`+=`/`++` consumed them) check against the `[]`
    /// element's first formal only (`checkIndexExpressionIndex`'s read-arm,
    /// resolver.dart:3380 — `writeElement: null`).
    fn end_compilation_unit(&mut self, count: usize, token: Token) {
        let staged: Vec<u32> = self.engine.index_arg_faces.keys().copied().collect();
        for bracket in staged {
            self.engine.check_index_argument(bracket, None);
        }
        let _ = (count, token);
    }

    // —— patterns & switch exhaustiveness (M2 face) ——————————————————————

    fn begin_pattern(&mut self, token: Token) {
        self.flow().begin_pattern(token);

        let engine = &mut self.engine;
        engine.switch.pattern_depth += 1;
        // A root pattern starts a fresh flat-destructure window: a case
        // or if-case pattern never reaches `destructure_record`, so its
        // marks (an object pattern's dirty bit, its binders) must not
        // leak into the next declaration's bind.
        if engine.switch.pattern_depth == 1 {
            engine.pattern_bind_vars.clear();
            engine.pattern_bind_records = 0;
            engine.pattern_bind_dirty = false;
            engine.switch.root_begin = token.range.offset;
        }
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
                // An unfolded index (`m[k]?[n]`, a value-row receiver
                // keeps its operand rows) leaves the KEY as the top row:
                // the matched value is the `[]` read, never the key.
                let scrutinee_row = engine
                    .rows
                    .last()
                    .filter(|row| !engine.is_index_key_row(row));
                let scrutinee = scrutinee_row.map_or(Ty::Unknown, |row| {
                    // A null-shorted chain (`a?.b case …`) types the whole
                    // node nullable (the official `ShortCircuiting`
                    // result): the member row keeps the field's own face.
                    if engine.null_aware_members.contains(&row.range.offset) {
                        Engine::nullable_form(row.ty)
                    } else {
                        row.ty
                    }
                });
                let member_scrutinee = scrutinee_row.and_then(|row| {
                    matches!(row.shape, RowShape::Member { .. }).then_some(row.range)
                });

                engine
                    .switch
                    .open_if_case(scrutinee, token.range, member_scrutinee);
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
        self.pattern_rows.push(PatternNodeState::new());
    }

    fn end_pattern(&mut self, token: Token) {
        self.flow().end_pattern(token);

        // The record close (#3242) runs while its frame is still open: the
        // never-match verdict reads the frame's matched type off the pattern
        // stack, and `handle_record_pattern` latched the `(` this row spans
        // from.
        self.close_record_pattern();
        let engine = &mut self.engine;
        engine.switch.pattern_depth = engine.switch.pattern_depth.saturating_sub(1);
        let popped = engine.switch.pattern_stack.pop();
        if popped.is_some() {
            // Lockstep with the pattern stack: the frame's deferred-row state
            // dies with the frame.
            self.pattern_rows.pop();
        }
        let Some(frame) = popped else {
            return;
        };
        if engine.switch.pattern_stack.is_empty() {
            // The variable latch dies with its root pattern: only a field
            // boundary inside the SAME pattern reads it as a getter, and a
            // survivor would mis-name a later field's getter (upstream reads
            // nothing but the field's own sub-pattern,
            // type_analyzer.dart:1674-1676).
            engine.last_pattern_variable = None;
        }
        let merged = SwitchTracker::collapse(&engine.name_world(), frame.leaves, frame.or_ops);
        // The whole-pattern null-check binder stagings (bound inside the
        // engine borrow below, staged into the flow's inferred-local seam
        // after it ends).
        let mut staged_null_check: Vec<(String, SourceRange)> = Vec::new();
        if engine.switch.pattern_stack.is_empty() {
            // The whole-pattern null-check binders take their face here:
            // the matched type's non-null projection — the open switch
            // context's scrutinee face, read before an if-case window
            // closes with it (`if (expr?.f case final x?)` binds `x` to
            // the chain's non-null element face).
            // The candidates are ROOT null-checks only (the queue's own
            // depth gate) — record/object sub-pattern binders never queue,
            // their faces belong to the destructure/field paths.
            let candidates = std::mem::take(&mut engine.null_check_binder_candidates);
            let mut scrutinee = engine
                .switch
                .switches
                .last()
                .map_or(Ty::Unknown, |c| c.scrutinee);
            // A MEMBER scrutinee's chain (`transformer?.value case final
            // t?`) carries its face in the accessor walk, not the merged
            // row's own ty (the merge's declared-type write answers fields
            // only): re-derive this gate's face from the member read
            // recorded at exactly the chain head's span, the `?.` short's
            // nullability joined. ONLY the member-row shape re-derives —
            // a bare-variable scrutinee's nearest-earlier member read
            // would be a stale, unrelated face. The raw latch stays the
            // answer for every other scrutinee consumer (the null-check
            // pattern's own verdicts among them: widening them
            // over-fires on degraded receivers the oracle types nullable).
            if !matches!(scrutinee, Ty::Named { .. })
                && let Some(span) = engine
                    .switch
                    .switches
                    .last()
                    .and_then(|c| c.if_case_member_scrutinee)
                && let Some(read) = engine
                    .member_reads
                    .binary_search_by(|read| read.offset.cmp(&span.offset))
                    .ok()
                    .map(|index| &engine.member_reads[index])
            {
                let face = engine.accessor_face_for(&read.recv, read.name);
                if matches!(face, Ty::Named { .. }) {
                    scrutinee = if engine.null_aware_members.contains(&span.offset) {
                        Engine::nullable_form(face)
                    } else {
                        face
                    };
                }
            }
            if let (false, mut scrutinee) = (candidates.is_empty(), scrutinee)
                && let Ty::Named { nullable, .. } = &mut scrutinee
            {
                *nullable = false;
                if let Ty::Named { name, .. } = scrutinee {
                    let lexeme = engine.strings.resolve(name).to_string();
                    for (name, span) in candidates {
                        engine.bind_var_type(name, scrutinee);
                        staged_null_check.push((lexeme.clone(), span));
                    }
                }
            }
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
        for (lexeme, span) in staged_null_check {
            let base = self.flow().intern_name(&lexeme);
            self.flow().stage_inferred_local(
                span.offset,
                FlowTy {
                    base: FlowBase::Name(base),
                    nullable: false,
                },
                Vec::new(),
            );
        }
    }

    fn begin_binary_pattern(&mut self, operator: Token) {
        self.flow().begin_binary_pattern(operator);
        let engine = &mut self.engine;
        // The left operand is complete: a variable it latched is no
        // logical-and/or field's getter name — that pattern shape never
        // unwraps to a variable pattern (ast.dart:57337 covers the variable
        // and the transparent wrappers only), and the operator fires before
        // the right operand, so clearing here keeps the right operand's own
        // field boundary from reading the left's name.
        engine.last_pattern_variable = None;
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

    fn end_binary_pattern(&mut self, operator: Token, end: Token) {
        self.flow().end_binary_pattern(operator, end);
    }

    fn handle_cast_pattern(&mut self, operator: Token) {
        let engine = &mut self.engine;
        // The still-armed type is the cast target: the triple at
        // shared_type_analyzer.dart:129-152 (the Cast kind of
        // `checkPatternNeverMatchesValueType`), plus `visitCastPattern`'s
        // matched-`Null` face (best_practices_verifier.dart:227-239).
        let cast_ty = engine.armed_ty;
        engine.check_cast_pattern(operator.range, cast_ty);
        // The coverage half (`createCastSpace`, shared.dart:704-780): the
        // cast does NOT cover its own type — the sub-pattern's space (already
        // in the open frame) is what the case covers, widened to the whole
        // matched type when the cast type fits inside it and joined with
        // `null` when the cast type is non-nullable.
        if let Some(cast_ty) = cast_ty {
            engine.finish_cast_pattern(cast_ty);
        }
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
        self.engine.pending_pattern_null_check =
            self.engine.last_pattern_variable.map(|(name, _)| name);
        // A type-free binder the null-check wrapped is a WHOLE-pattern
        // binder candidate (`case final transformer?` over a member-chain
        // scrutinee): its face is the matched type's non-null projection,
        // applied at the root pattern's close — the object-pattern
        // shorthand path (`:final x?`) claims its binder at the field
        // boundary first and the claim retracts the candidate. Only a
        // ROOT null-check queues: a null-check nested inside a record or
        // object sub-pattern (`case (final x?, _)`'s slot, `Foo(:final
        // x?)`'s field) fires at pattern_depth > 1 and its binder's face
        // belongs to the destructure/field path, never the scrutinee's.
        if self.engine.switch.pattern_depth == 1
            && let Some(pair) = self.engine.last_pattern_variable
        {
            self.engine.null_check_binder_candidates.push(pair);
        }
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
        // The nullable-matched gate (#2843): `case > 0` / `case < y` over a
        // nullable matched value reports the shared unchecked code — the
        // relational arm of the official member resolution
        // (resolver.dart `resolveRelationalPatternOperator` feeds
        // `typePropertyResolver.resolve` with the matched type as the
        // receiver, `propertyErrorEntity: node.operator`).
        self.check_relational_pattern_nullable(operator);
        // The operator-definedness consult (#2917): the same official
        // consult's `needsGetterError` arm — a matched class without the
        // spelled operator reports `undefined_operator` at the operator
        // token.
        self.check_relational_pattern_operator(operator);
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
        // The List kind of `checkPatternNeverMatchesValueType`
        // (list_pattern_resolver.dart:45): the pattern's required type is a
        // `List`, so a matched value that cannot be one proves it dead. The
        // row spans the whole `[...]` node (`.at(pattern)`).
        let at = SourceRange {
            offset: left.range.offset,
            length: right.range.end() - left.range.offset,
        };
        Self::defer_pattern_row(&mut self.pattern_rows, self.engine.check_list_pattern(at));
        // A variable INSIDE a list pattern is no positional object field's
        // getter name (only the transparent wrappers unwrap,
        // ast.dart:6640/:39354/:39708/:40734): the latch dies with the
        // bracket.
        self.engine.last_pattern_variable = None;
        // The list face arrives through keysOfInterest/additional properties
        // — not modeled at name level (documented under-report).
        self.engine.pattern_leaf(Restriction::Unknown);
        self.engine.pattern_bind_dirty = true;
    }

    fn handle_map_pattern(&mut self, count: usize, left: Token, right: Token) {
        self.flow().handle_map_pattern(count, left, right);
        // `visitMapPattern`'s duplicate-key walk (constant_verifier.dart
        // :325-355) drains here: every entry key that folded to a value was
        // collected in source order, and a key equal to an EARLIER one
        // reports `equal_keys_in_map_pattern` at the later key, with the
        // first key as the "first key with this value" context message.
        // Equality is the oracle's `uniqueKeys` comparator: `identical` for
        // keys without primitive equality (records of non-primitive-equality
        // fields, const instances with a user `==`), `==` for the
        // primitive-equality states (`int`/`double`/`String`/`bool`/`null`/
        // `Type`/`Symbol`, and records whose fields all have it).
        let keys = self
            .pattern_rows
            .last_mut()
            .map_or_else(Vec::new, |state| std::mem::take(&mut state.map_keys));
        let mut first_of: Vec<(crate::const_eval::ConstantValue, SourceRange)> = Vec::new();
        let mut duplicates: Vec<(SourceRange, SourceRange)> = Vec::new();
        for (node, at) in &keys {
            let crate::const_eval::Verdict::Value(value) = eval(node, &self.engine) else {
                continue;
            };
            // A value this subset only knows as an opaque instance carries
            // no decidable identity (the under-report direction): two
            // distinct const creations fold to the same sentinel, and the
            // oracle's `identical` over them is unknowable here.
            if matches!(value, crate::const_eval::ConstantValue::Instance(_)) {
                continue;
            }
            let existing = first_of.iter().find(|(prior, _)| {
                crate::const_eval::map_pattern_keys_equal(prior, &value, &|v| {
                    self.engine.legacy_primitive_equality(v)
                })
            });
            match existing {
                Some((_, prior_span)) => duplicates.push((*at, *prior_span)),
                None => first_of.push((value, *at)),
            }
        }
        for (at, prior) in duplicates {
            self.engine.emit_with_context(
                SemanticCode::EqualKeysInMapPattern,
                at,
                "",
                "",
                vec![ContextMessage {
                    message: SharedStr::from("The first key with this value."),
                    file: self.engine.file,
                    range: prior,
                }],
            );
        }
        // Same latch death as the list arm: a map's entry values never name
        // an enclosing positional field's getter.
        self.engine.last_pattern_variable = None;
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
        //
        // A key that DOES fold to a value also joins the frame's key list:
        // `visitMapPattern`'s `uniqueKeys` table collects every
        // `DartObjectImpl` key and the map's close reports the later entry
        // of an equal pair (`constant_verifier.dart:325-355`).
        if let Some(node) = self.engine.const_capture.close_map_pattern_key() {
            // A key that reaches its value through a deferred import's
            // prefix fails const-eval before value production
            // (`evaluation.dart`'s `_getDeferredLibraryError`, whose
            // `SwitchPatternCase` arm answers `patternConstantFromDeferred
            // Library` at the referenced name): the deferred row preempts
            // the site default, and the key joins no duplicate table.
            if let Some(span) = self.engine.pattern_deferred_span(&node) {
                self.engine.emit(
                    SemanticCode::PatternConstantFromDeferredLibrary,
                    span,
                    "",
                    "",
                );
                return;
            }
            let value = match eval(&node, &self.engine) {
                Verdict::Value(value) => Some(value),
                Verdict::Constant => None,
                Verdict::NotConstant(span)
                | Verdict::Unresolved(span)
                | Verdict::Fault(_, span) => {
                    let span = if span.length > 0 { span } else { colon.range };
                    self.engine
                        .emit_corrected(SemanticCode::NonConstantMapPatternKey, span, "");
                    None
                }
            };
            // The key expression's WRITTEN span: the capture root's own
            // span is unreliable for the literal shapes (a record literal's
            // value node anchors at its closer), so the key extent is read
            // from the token stream — the token after the entry's opening
            // `{`/`,` through the token before the `:`.
            if let Some(value) = value {
                let at = self.map_pattern_key_span(colon);
                if let Some(state) = self.pattern_rows.last_mut() {
                    state.map_keys.push((node, at));
                }
            }
        }
    }

    fn handle_record_pattern(&mut self, left_paren: Token, count: usize) {
        // The record shape test can fail — the flow recorder joins the
        // failure arm into the unmatched chain so the statement after an
        // if-case stays live.
        self.flow().handle_record_pattern(left_paren, count);
        // The `(` latches: the record kind's never-match row spans the whole
        // `(...)` node, whose closer the scanner's group table answers
        // (#3242).
        if let Some(state) = self.pattern_rows.last_mut() {
            state.pending_record_open = Some(left_paren);
        }
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
        let mut leaf = engine
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
        // The deferred-library preemption (`evaluation.dart`'s
        // `_getDeferredLibraryError`, the `SwitchPatternCase` arm): a
        // constant expression reaching its value through a `deferred`
        // import's prefix fails const-eval BEFORE value production, so the
        // row preempts every verdict the subset folds (value, opaque,
        // generic non-constant, and a same-node fault alike) and anchors at
        // the referenced name. The oracle's early-return also skips the
        // constant-pattern value bookkeeping, so nothing else fires here.
        if let Some(node) = &capture
            && let Some(span) = engine.pattern_deferred_span(node)
        {
            engine.emit(
                SemanticCode::PatternConstantFromDeferredLibrary,
                span,
                "",
                "",
            );
            engine.pattern_leaf(leaf);
            return;
        }
        // `visitConstantPattern`'s `_evaluateAndReportError`: the
        // expression's own verdict — a fault overrides the site default,
        // a generic miss reports `constant_pattern_with_non_constant_expression`
        // at the failing node's span. `Value`/`Constant` (and a `null` root
        // — a window that never opened, e.g. a recovery path) stay silent;
        // the oracle's `InvalidType` early-return maps to the same silence.
        if let Some(node) = capture {
            let verdict = eval(&node, engine);
            // The coverage leaf folds a bare `const` variable to its
            // value's static type — `const c = true` is `BoolValue(true)`
            // (upstream `PatternConverter._convertConstantValue`'s
            // `getBoolValueStaticType`/`getEnumElementStaticType` arms,
            // analyzer exhaustiveness.dart:744-788), which
            // `classify_constant_tail`'s member-row shape never reaches.
            if leaf == Restriction::Unknown
                && let Verdict::Value(value) = &verdict
            {
                leaf = match value {
                    ConstantValue::Bool(value) => Restriction::BoolValue(*value),
                    ConstantValue::Enum { container, name } => {
                        Restriction::EnumValue(*container, *name)
                    }
                    _ => leaf,
                };
            }
            match verdict {
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
        // The object pattern's NamedType arity face
        // (`_buildTypeArguments`, named_type_resolver.dart — `.at(node)`):
        // `case Box<int, String>():`'s head group closed before the head
        // row pushed, so `end_type_arguments` latched the group's count
        // for this close. The `<`'s offset abuts the head's own token end
        // across trivia — consume it into the container-arity verdict
        // (declared arity vs written count over the type node's
        // head-through-`>` span). The container-only count keeps the
        // resolve side-effect-free: `declared_type_params`'s `resolve_bare`
        // arm binds the head as a resolution site (an extra resolution the
        // oracle's pattern walk never runs — it would feed downstream
        // verdicts the `as ValueT` cast reads differently).
        if let Some(head) = head
            && let Some((count, group_open, group_end)) =
                engine.pending_pattern_type_args.take()
            && engine.trivia_between(first_identifier.range.end(), group_open)
            && let Some(declared) = engine.declared_type_params_of_container(head)
            && declared != count
        {
            let span = SourceRange {
                offset: first_identifier.range.offset,
                length: group_end - first_identifier.range.offset,
            };
            let name_text = engine.strings.resolve(head).to_string();
            engine.emit3(
                SemanticCode::WrongNumberOfTypeArguments,
                span,
                &name_text,
                &declared.to_string(),
                &count.to_string(),
            );
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
        // required type. A nested object head (`Box(value: Some(...))`) is a
        // field sub-pattern — its matched type is the field's column, so the
        // candidate defers to the enclosing object's close (#3242).
        if let Some(head) = head {
            let required = Ty::named(head);
            Self::defer_pattern_row(
                &mut self.pattern_rows,
                engine.check_pattern_never_matches(first_identifier.range, required),
            );
        }
        // A case/if-case window's object head latches for the logical-and
        // binder face (`AsyncData() && final transformer` — the binder
        // takes the head's type, the matched-type binding for the
        // and-pattern's right operand).
        if head.is_some() && (engine.switch.case_sink_open || !engine.switch.switches.is_empty()) {
            engine.last_case_object_head = head;
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
                {
                    // The field path owns this binder: any whole-pattern
                    // null-check candidate the `?` latched for it retracts
                    // (the property face binds, never the scrutinee).
                    engine
                        .null_check_binder_candidates
                        .retain(|(name, _)| *name != binder);
                    if let Some(mut ty) = engine.object_pattern_binder_ty(head, getter) {
                        if field.binder_non_null
                            && let Ty::Named { nullable, .. } = &mut ty
                        {
                            *nullable = false;
                        }
                        engine.bind_var_type(binder, ty.clone());
                        staged_binders.push((ty, binder_span));
                    } else {
                        // The shorthand's variable is a LOCAL even when its
                        // getter face stays unresolvable (parametric return,
                        // unindexed head): the name shadows any implicit-this
                        // getter for the rest of the body — the R53
                        // `$ResultError(:final stackTrace)` face — while the
                        // unknown slot keeps member reads on the binder
                        // under-reported.
                        engine.bind_var_type(binder, Ty::UNKNOWN);
                    }
                }
            }
        }
        engine.close_object_fields(head, &fields);
        // The nullable-matched head's field reads (#2843): the object
        // pattern resolves each getter on the head's OWN required type
        // (type_analyzer.dart `analyzeObjectPattern` →
        // `resolveObjectPatternPropertyGet`, `receiverType: requiredType`)
        // — a typedef-aliased or bound-nullable head (`typedef A = int?`,
        // `T extends int?`) reads every field over a potentially-nullable
        // receiver and reports the shared unchecked code at the pattern's
        // type name (`propertyErrorEntity: objectPattern.type`, the span
        // `p.Type` covers when the head is prefixed). The nullable arm's
        // silence set: `Object` members and `==` resolve, an applicable
        // extension face resolves (`on int?` twins win the nullable
        // bucket) — a found getter never pairs with `undefined_getter`
        // (the arm's `_reportedGetterError`), and the head's own members
        // never answer a nullable receiver, so the member walk stays out.
        // An `on int` face under the nullable receiver is the oracle's
        // not-applicable arm (`int?` is not a subtype of `int`) — the
        // `UnsafeSingle` verdict reports.
        if let Some(head) = head {
            let face = engine.fold_alias_ty(Ty::named(head));
            if engine.potentially_nullable(face)
                && let Ty::Named {
                    name: face_name, ..
                } = face
            {
                let eq = engine.intern("==");
                let ty_end = second_identifier
                    .map_or(first_identifier.range.end(), |token| token.range.end());
                let ty_span = SourceRange {
                    offset: first_identifier.range.offset,
                    length: ty_end - first_identifier.range.offset,
                };
                for field in &fields {
                    let Some((getter, _)) = field.getter else {
                        continue;
                    };
                    if getter == eq || engine.members.object_members.contains(&getter) {
                        continue;
                    }
                    if !matches!(
                        engine.extension_application(face_name, face, getter, false, true),
                        ExtensionApp::None | ExtensionApp::UnsafeSingle,
                    ) {
                        continue;
                    }
                    let name_text = engine.strings.resolve(getter).to_string();
                    engine.emit_unchecked_nullable(
                        ty_span,
                        &name_text,
                        crate::codes::UncheckedNullableKind::Property,
                    );
                }
            }
        }
        // An object pattern's fields are member reads, not record slots.
        engine.pattern_bind_dirty = true;
        // The per-field column face (#3242): the head and the drained field
        // list are both in hand here, so every field's deferred never-match
        // candidates are judged against the property column it reads.
        self.judge_object_field_rows(head, &fields);
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
        // latch is consumed per field so a field never reads a stale one.
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
            // A POSITIONAL field (`Circle(var c)`) reads the getter its own
            // sub-pattern's variable names: upstream falls back to
            // `field.pattern.variablePattern?.name` when the field writes no
            // name and reports `undefined_getter` at that token (upstream
            // Dart SDK source, pkg/analyzer/lib/src/dart/resolver/
            // type_analyzer.dart:1674-1679 — the lookup then runs over the
            // head's required type, `:1681-1697`). Only the transparent
            // wrappers resolve through to a variable pattern — cast, null
            // assert, null check, parentheses (pkg/analyzer/lib/src/dart/ast/
            // ast.dart:6640, :39354, :39708, :40734); a list/map/record/
            // constant/logical-and sub-pattern answers none and reads no
            // getter (the `dynamic` fall-back, type_analyzer.dart:1677-1679)
            // — those shapes clear the latch before this boundary fires. The
            // variable itself is the sub-pattern's own binder
            // (`handle_declared_variable_pattern`), never the field's
            // shorthand binder, so `binder` stays `None`.
            (None, _) => latched,
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
                // A positional field keeps no member name: its column is
                // order-keyed, not name-keyed, so only the getter it reads
                // is recorded (`self.name` stays `None`).
                name: if colon.is_some() {
                    getter.map(|(name, _)| name).or(name)
                } else {
                    None
                },
                leaves,
                getter,
                binder,
                binder_non_null: {
                    engine.pending_pattern_null_check.take();
                    binder_non_null
                },
            });
        }
        // The boundary closes the field's sub-pattern, so the never-match
        // candidates it raised are exactly this field's (#3242): they drain
        // into the bucket index-aligned with the `ObjectField` just pushed,
        // and the object close judges each against its column.
        if let Some(state) = self.pattern_rows.last_mut() {
            let rows = std::mem::take(&mut state.pending);
            state.fields.push(rows);
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
        //
        // An assignment-pattern binder (`(a, b) = …`) declares nothing
        // (#2858): every variable pattern there refers to an existing
        // local, the official verifier creates no element for it, and the
        // write never counts as a use (the verifier's write arm skips
        // `LocalVariableElement`) — so the usage cluster registers no row:
        // reads after the assignment credit the original declarations, the
        // write spares nothing. The shadow record still fires (the typed
        // binder's declared-type write reverts at the case-member close),
        // and the type/exhaustiveness faces below are binder-shape faces,
        // not declaration faces.
        if let Some(name) = engine.name_of(variable) {
            engine.last_pattern_variable = Some((name, variable.range));
            engine.note_pattern_shadow(name);
            if !in_assignment_pattern {
                // A binder in an if-case condition that shadows an outer
                // tracked local retires with its case scope: a read after the
                // statement counts for the outer declaration
                // (`T? e; if (x case C(:var e)) {...} print(e);`).
                let shadows_local = engine.cond_mark.is_some()
                    && engine.cond_case
                    && engine.resolve_local(name).is_some();
                engine.add_tracked_local(name, variable.range, LocalKind::PatternVariable);
                if shadows_local {
                    let index = engine.locals.len() - 1;
                    engine.note_pattern_binder(index, variable.range.offset);
                }
            }
            // The DeclaredVariable kind of `checkPatternNeverMatchesValueType`
            // (type_analyzer.dart:723-756) reads the still-armed declared
            // type before the binding consumes it, anchored at
            // `pattern.type` — the written type's span (probed 3.13.1:
            // `IntAlias` at 6:14, not the bound variable).
            let required = engine.armed_ty;
            if let Some(required) = required {
                let at = engine.armed_ty_span.unwrap_or(variable.range);
                Self::defer_pattern_row(
                    &mut self.pattern_rows,
                    engine.check_pattern_never_matches(at, required),
                );
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
                    args,
                }) => {
                    engine.bind_var_type(
                        name,
                        Ty::Named {
                            name: ty_name,
                            nullable,
                            args: ArgListId::EMPTY,
                        },
                    );
                    // The space leaf keeps the WRITTEN arguments: two
                    // same-head typed-variable cases (`final Map<A, B> m`
                    // then `final Map<Object, Object?> m`) differ at the
                    // argument list, and neither covers the other.
                    engine.pattern_leaf(if args != ArgListId::EMPTY {
                        Restriction::NamedArgs(ty_name, nullable, args)
                    } else {
                        Restriction::Named(ty_name, nullable)
                    });
                }
                ty => {
                    // The logical-and binder's staging pair (bound inside
                    // the engine borrow below, staged into the flow's
                    // inferred-local seam after it ends).
                    let mut staged_and: Option<(StringId, SourceRange)> = None;
                    if let Some(ty) = ty {
                        engine.bind_var_type(name, ty);
                    } else if !in_assignment_pattern {
                        // A type-free binder on the RIGHT operand of a
                        // logical-and inside a case window (`AsyncData() &&
                        // final transformer`): the matched type is the left
                        // object pattern's head — the oracle binds the
                        // variable to it (the and-pattern's variable keeps
                        // the narrowed face for the arm's member reads).
                        let and_operand = engine
                            .switch
                            .pattern_stack
                            .last()
                            .is_some_and(|frame| frame.binary == Some(false));
                        if and_operand
                            && let Some(head) = engine.last_case_object_head.take()
                            && engine
                                .local_declared_ty(name)
                                .map_or(true, |slot| slot == Ty::UNKNOWN)
                        {
                            engine.bind_var_type(name, Ty::named(head));
                            staged_and = Some((head, variable.range));
                        }
                        // A bare pattern variable (`(a, b)`'s `a`): its type
                        // is the scrutinee record's slot — recorded for the
                        // declaration-statement bind ([`Self::
                        // destructure_record`]). Assignment patterns rebind
                        // existing locals and never enter the list.
                        engine.pattern_bind_vars.push((name, variable.range));
                    }
                    // A type-free variable covers its context: no leaf.
                    if let Some((head, span)) = staged_and {
                        let lexeme = self.engine.strings.resolve(head).to_string();
                        let base = self.flow().intern_name(&lexeme);
                        self.flow().stage_inferred_local(
                            span.offset,
                            FlowTy {
                                base: FlowBase::Name(base),
                                nullable: false,
                            },
                            Vec::new(),
                        );
                    }
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
            Self::defer_pattern_row(
                &mut self.pattern_rows,
                engine.check_pattern_never_matches(at, required),
            );
        }
        let declared = engine.armed_ty;
        engine.armed_ty_span = None;
        engine.check_wildcard_pattern(wildcard.range, declared);
        let Some(Ty::Named {
            name,
            nullable,
            args,
        }) = engine.armed_ty.take()
        else {
            return;
        };
        engine.pattern_leaf(if args != ArgListId::EMPTY {
            Restriction::NamedArgs(name, nullable, args)
        } else {
            Restriction::Named(name, nullable)
        });
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

    fn begin_switch_case_when_clause(&mut self, token: Token) {
        // The guard is flow input: its false arm joins the case's
        // unmatched chain (official `switch_endAlternative`'s `guardInfo`).
        self.flow().begin_switch_case_when_clause(token);
    }

    /// The case header's pattern window opens before its pattern parses;
    /// the root it produces is captured at [`Self::end_case_expression`].
    /// A legacy (`< 3.0`, patterns disabled) toolchain parses the head as a
    /// plain expression instead — its const requirement rides the constant
    /// window, evaluated at the same close.
    fn begin_case_expression(&mut self, case_kw: Token) {
        self.flow().begin_case_expression(case_kw);

        let engine = &mut self.engine;
        engine.switch.case_sink_open = true;
        // The clause anchor latches at the HEAD's opening, before its
        // pattern parses: a constant-pattern candidate inside the head
        // defers to the teardown join keyed on this anchor (#1351), and
        // `begin_switch_case` fires only at the case body — after every
        // head in the group has already been checked (the parser's
        // `parse_statements_in_switch_case` runs past the head loop).
        // The token is the same `case` keyword the flow's `MatchedType`
        // fact anchors at (`end_case_expression`'s `case_kw`). It lands
        // BEFORE `switch_member_head_begin` — the scope's `head_offset`
        // records this head's own keyword, not the previous clause's.
        engine.clause_anchor = Some(case_kw.range);
        engine.switch_member_head_begin();
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
        // The clause's pattern window closed: its object head and any
        // unapplied null-check candidates never cross into the next clause.
        engine.last_case_object_head = None;
        engine.null_check_binder_candidates.clear();
        // The legacy head's const verdict, folded for the pre-patterns
        // verifier (`SwitchCtx::legacy_cases`): `Value` keeps its constant,
        // `Constant` stays opaque, every other verdict already reported at
        // its own span.
        let mut legacy: Option<ConstantValue> = None;
        let mut deferred: Option<SourceRange> = None;
        if engine.const_capture.top_is_constant_pattern()
            && let Some(node) = engine.const_capture.close_constant_pattern()
        {
            let head_span = node.span;
            // A constant through a deferred import's prefix fails
            // const-eval upstream (`evaluation.dart`'s
            // `_getDeferredLibraryError` answers `InvalidConstant` at the
            // prefix — before value production, so the deferred row
            // preempts every verdict the subset folds: value, opaque,
            // generic non-constant, and a same-head fault alike) and
            // anchors at the prefix token.
            deferred = engine.legacy_deferred_span(&node);
            if let Some(prefix_span) = deferred {
                engine.emit(
                    SemanticCode::NonConstantCaseExpressionFromDeferredLibrary,
                    prefix_span,
                    "",
                    "",
                );
            } else {
                match eval(&node, engine) {
                    Verdict::Value(value) => legacy = Some(value),
                    Verdict::Constant => {}
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
            // A legacy-verifier entry exists per HEAD, verdict or not: the
            // opaque `Constant` carries `None`, the reported-non-constant
            // verdicts carry the same `None` upstream's
            // `expressionValue is! DartObjectImpl` skip implies.
            if !engine.patterns_enabled
                && let Some(ctx) = engine.switch.switches.last_mut()
            {
                ctx.legacy_cases.push(crate::switch_tracker::LegacyCase {
                    span: head_span,
                    value: legacy.take(),
                    deferred,
                });
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
        engine.switch_expr_marks.push(engine.rows.len());
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
        // A selector after the closed expression (`switch (x) {…}[i]`,
        // `.m`) reads the arm fold's static type, which this pass does not
        // compute: the window above the scrutinee folds to one opaque
        // value row so the selector's receiver stays unresolved (never an
        // arm's own row, e.g. a `throw`'s `AssertionError` operand).
        let engine = &mut self.engine;
        let mark = engine.switch_expr_marks.pop();

        if count > 0
            && let Some(mark) = mark
            && {
                // `end` is the closing `}` (or, in some recovery paths, the
                // token after it): the selector is the first non-space byte
                // past the brace.
                let mut at = if engine.byte_at(end.range.offset) == Some(b'}') {
                    end.range.end()
                } else {
                    end.range.offset
                };
                while matches!(engine.byte_at(at), Some(b' ' | b'\t' | b'\n' | b'\r')) {
                    at += 1;
                }
                matches!(engine.byte_at(at), Some(b'[' | b'.' | b'?'))
            }
        {
            let scrutinee_end = mark + 1;

            if engine.rows.len() > scrutinee_end {
                for row in engine.rows.drain(mark..) {
                    engine.deferred_miss_rows.push(row);
                }
                engine.push_row(Row {
                    name: engine.intern(""),
                    range: SourceRange {
                        offset: begin.range.offset,
                        length: end.range.end().saturating_sub(begin.range.offset),
                    },
                    qualified: false,
                    shape: RowShape::Value,
                    intent: Intent::Read,
                    write_op: None,
                    ty: Ty::UNKNOWN,
                });
            }
        }
    }

    // —— if-case ————————————————————————————————————————————————————————

    fn handle_parenthesized_condition(
        &mut self,
        begin: Token,
        case_kw: Option<Token>,
        when: Option<Token>,
    ) {
        // The flow's if-case marker: a non-reference scrutinee (a top-level
        // or implicit-this field read) leaves no variable scrutinee for the
        // flow's `ThenBegin` fallback adoption, so only this marker routes
        // the pattern context to the branch.
        self.flow()
            .handle_parenthesized_condition(begin, case_kw, when);
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
        // feed the argument check's grounding. The capture latches this
        // group's own depth — the increment below — so a member's nested
        // annotation never joins the member list.
        engine.call_args_begin(head.is_some(), engine.type_arg_depth + 1);
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
            } else if head.is_none() {
                // #3296 — the implicit creation's own group (`A<String>()`)
                // arrives headless (the parser reports the callee as a
                // constructor reference, never as a type head), so the
                // oracle's `checkNamedType` face never ran on it. The callee
                // head recovered from the reference re-opens the site.
                engine.implicit_creation_head(token.range.offset).or(head)
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
            // The is-check verdict's whole-expression span extends the
            // armed type to this group's `>` (`x is List<int>`) — the
            // last closed group at the verdict site, the is-target's own.
            engine.type_args_group_end = Some(end.range.end());
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
        if engine.const_capture.top_is_constant_pattern()
            && (engine.ur_arg_base.is_empty() || engine.switch.case_sink_open)
        {
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
        // The `ur_arg_base` gate rejects literals that are plain ARGUMENTS
        // (a constant-pattern window never opens for those) — but a
        // switch-expression case head parsing inside an open argument list
        // (`enclosingPrefix: switch (name) { var n? => .., null => .. }`)
        // is a genuine pattern head: the open case-header sink proves it
        // (library_analyzer.dart:937's `null` arm, whose Null space the
        // plain gate starved to Unknown and false-positived the switch
        // non-exhaustive).
        if engine.const_capture.top_is_constant_pattern()
            && (engine.ur_arg_base.is_empty() || engine.switch.case_sink_open)
        {
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
            let children = &engine.rows[mark..engine.rows.len() - 1];
            // The void-operand use (upstream `visitInterpolationExpression`
            // → `checkForUseOfVoidResult(node.expression)`): `"$x"`/`"${e}"`
            // consumes the element's value — every `void`-typed operand row
            // reports at its use-site span.
            let void_rows: Vec<SourceRange> = children
                .iter()
                .filter(|row| engine.operand_static_ty(row) == Ty::Void)
                .map(|row| engine.use_site_range(row))
                .collect();
            let offsets: Vec<u32> = children.iter().map(|row| row.range.offset).collect();
            for range in void_rows {
                engine.emit(SemanticCode::UseOfVoidResult, range, "", "");
            }
            engine.interpolation_child_rows.extend(offsets);
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
            // A `for` element already folded into the parts as an
            // `Expr::Faulted` leaf (the literal close staged it) — the
            // element check emits `const_eval_for_element` at its span and
            // the literal's own verdict carries the fault, so no separate
            // face runs here. Drain the bookkeeping the close left behind.
            let _ = engine.const_capture.take_pending_for_elements();
            // Implicit const literals in a const context check per element
            // with the element code too (the oracle evaluates the literal
            // at the enclosing site).
            let failed =
                engine.check_const_elements(SemanticCode::NonConstantListElement, &elements);
            // The const spread-shape face: a `...expr` operand that
            // evaluates to a non-list/set value reports
            // `const_spread_expected_list_or_set`.
            engine.check_const_spread_shapes(left, right, &elements, false);
            let payload = engine
                .literal_payload(&elements)
                .map(|values| ConstantValue::List(values.into()));
            engine.push_literal_value(
                failed,
                right.range,
                SourceRange {
                    offset: left.range.offset,
                    length: right.range.end() - left.range.offset,
                },
                crate::const_eval::InstanceKind::List,
                payload,
            );
        }
        // The void-operand use (upstream `literal_element_verifier.dart`
        // :95 — `checkElement` → `checkForUseOfVoidResult(element)`): a
        // `void`-typed element expression reports at its own span, but only
        // when the literal's element type expects a non-`void` value
        // (`elementType is! VoidType`). Runs on the live element rows
        // before the fold drains them below.
        let typed = engine.typed_list_literal_ty(left);
        let element = typed.and_then(|ty| match ty {
            Ty::Named { args, .. } => engine.boundary.args.borrow().get(args).first().copied(),
            _ => None,
        });
        engine.check_collection_void_elements(left, right, element, None, None);
        engine.check_list_literal_elements(left, right);
        // The spread-element verifier (`_verifySpreadForListOrSet`): each
        // `...`-led leaf's operand answers the iterable-compatibility and
        // element-assignability asks against the list's declared or
        // inferred element type.
        engine.check_spread_elements(left, right, SpreadLiteralKind::List);
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
        if typed.is_some() {
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
                typed
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
        // The row sits at the `]` — the is-check verdict anchors the
        // literal operand at the literal's own `[` (the ListLiteral's
        // node offset). A typed literal's row already spans the whole
        // literal and took the early return above.
        engine.literal_row_open = Some((right.range.offset, left.range.offset));
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
        // The literal's resolved kind (`_inferSetOrMapLiteralType`):
        // a `Map` context dominates, then the leaves' can-be/must-be
        // fold, then a set context's iterable fallback. `None` is the
        // ambiguous literal — upstream records `dynamic` and runs NO
        // element verifier (`visitSetOrMapLiteral`'s `isMap`/`isSet`
        // dispatch), so the spread/expression/const-shape faces all
        // abstain rather than fire on the parser's map default.
        let resolved_kind = match engine.typed_set_or_map_literal_ty(left) {
            // A written type group resolves by its own arity: `<E>{...}`
            // is a set, `<K, V>{...}` a map, any other arity the oracle's
            // nonsense-group recovery (it records no usable type — the
            // leaves still decide below).
            Some((_, Ty::Named { args, .. })) => {
                match engine.boundary.args.borrow().get(args).len() {
                    1 => Some(SpreadLiteralKind::Set),
                    2 => Some(SpreadLiteralKind::Map),
                    _ => None,
                }
            }
            _ if count == 0 => Some(match engine.literal_context_side(left) {
                Some(false) => SpreadLiteralKind::Set,
                _ => SpreadLiteralKind::Map,
            }),
            _ => match engine.literal_context_side(left) {
                // A `Map` context dominates the leaves outright.
                Some(true) => Some(SpreadLiteralKind::Map),
                // Ambiguous leaves defer to a `Set` context's iterable
                // fallback; with no context at all the literal stays
                // unresolved.
                Some(false) => Some(
                    engine
                        .leaves_resolve_kind(left, right)
                        .unwrap_or(SpreadLiteralKind::Set),
                ),
                None => engine.leaves_resolve_kind(left, right),
            },
        };
        let resolved_is_map = matches!(resolved_kind, Some(SpreadLiteralKind::Map));
        if let Some((shape, has_ctl)) = engine.const_capture.close_non_const_dup(
            count,
            left.range,
            has_set_entry,
            resolved_is_map,
        ) {
            // The `_checkForDuplications` face (WARNING tier).
            engine.check_equal_duplicates(shape, has_ctl);
        } else if let Some(shape) = engine.const_capture.literal_set_or_map(
            count,
            left.range,
            has_set_entry,
            resolved_is_map,
        ) {
            // A `for` element already folded into the parts as an
            // `Expr::Faulted` leaf (the literal close staged it) — the
            // element check emits `const_eval_for_element` at its span and
            // the literal's own verdict carries the fault, so no separate
            // face runs here. Drain the bookkeeping the close left behind.
            let _ = engine.const_capture.take_pending_for_elements();
            match shape {
                SetOrMap::Map(parts) => {
                        // The oracle names the failing side
                        // (`_validateMapLiteralEntry`): `non_constant_map_key` /
                        // `non_constant_map_value` at each entry's key and
                        // value. The ordered parts interleave `Expr::Entry`
                        // pseudo-operands with control elements — an entry
                        // checks each side's face, a control element checks
                        // the map-element code (`expression_in_map` for a
                        // bare leaf, `const_eval_for_element` for a `for`,
                        // the `if`/spread verifier arms).
                        let mut failed = None;
                        for part in &parts {
                            match &part.expr {
                                crate::const_eval::Expr::Entry(key, value) => {
                                    for (code, side) in [
                                        (SemanticCode::NonConstantMapKey, key.as_ref()),
                                        (SemanticCode::NonConstantMapValue, value.as_ref()),
                                    ] {
                                        if let Some(at) = engine.check_const_elements(
                                            code,
                                            std::slice::from_ref(side),
                                        ) {
                                            failed.get_or_insert(at);
                                        }
                                    }
                                }
                                _ => {
                                    if let Some(at) = engine.check_const_elements(
                                        SemanticCode::NonConstantMapElement,
                                        std::slice::from_ref(part),
                                    ) {
                                        failed.get_or_insert(at);
                                    }
                                }
                            }
                        }
                        // The const spread-shape face: a `...expr`
                        // operand that evaluates to a non-map value
                        // reports `const_spread_expected_map`. The literal's
                        // RESOLVED kind decides the expected shape — the
                        // parser's `hasSetEntry` flag missed spread-decided
                        // literals (`const {...list}` is a set upstream),
                        // and an ambiguous literal's verifier never runs.
                        if let Some(kind) = resolved_kind {
                            engine.check_const_spread_shapes(
                                left,
                                right,
                                &parts,
                                kind == SpreadLiteralKind::Map,
                            );
                        }
                        let payload = engine.map_payload(&parts);
                        engine.push_literal_value(
                            failed,
                            right.range,
                            SourceRange {
                                offset: left.range.offset,
                                length: right.range.end() - left.range.offset,
                            },
                            InstanceKind::Map,
                            payload,
                        );
                    }
                    SetOrMap::Set(elements) => {
                        // The literal's own resolution decides the element
                        // code: a `:`-led entry anywhere in the leaves
                        // makes the literal a map upstream, so a
                        // non-Entry element (a bare expression, a spread)
                        // reports `non_constant_map_element` instead of
                        // `non_constant_set_element` — the parser's
                        // `has_set_entry` only reflects the first simple
                        // entry's colon, not the resolved kind.
                        let has_entry = {
                            let bytes = engine.source.as_bytes();
                            let start = engine.byte_of(left.range.end());
                            let end = engine.byte_of(right.range.offset);
                            literal_comma_segments(bytes, start, end)
                                .into_iter()
                                .any(|(s, e)| {
                                    let s = trim_trivia(bytes, s, e);
                                    s < e && top_level_colon(bytes, s, e).is_some()
                                })
                        };
                        let element_code = if has_entry {
                            SemanticCode::NonConstantMapElement
                        } else {
                            SemanticCode::NonConstantSetElement
                        };
                        let failed =
                            engine.check_const_elements(element_code, &elements);
                        // The const spread-shape face asks the literal's
                        // RESOLVED kind — a `:`-led leaf makes it a map,
                        // a spread-decided literal reads its operand's
                        // side (`const {...list}` is a set upstream), and
                        // an ambiguous literal's verifier never runs.
                        if let Some(kind) = resolved_kind {
                            engine.check_const_spread_shapes(
                                left,
                                right,
                                &elements,
                                kind == SpreadLiteralKind::Map,
                            );
                        }
                        let payload = engine
                            .literal_payload(&elements)
                            .map(|values| ConstantValue::Set(values.into()));
                        engine.push_literal_value(
                            failed,
                            right.range,
                            SourceRange {
                                offset: left.range.offset,
                                length: right.range.end() - left.range.offset,
                            },
                            InstanceKind::Set,
                            payload,
                        );
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
        // The void-operand use over the literal's elements and entry
        // key/value pairs (upstream `literal_element_verifier.dart` :95/
        // :151/:166 — `checkElement`/`_verifyMapLiteralEntry*` →
        // `checkForUseOfVoidResult`, each gated on the enclosing element /
        // `mapKeyType` / `mapValueType` not being `void`): before the typed
        // arm below takes its early return, so `check_map_literal_entries`
        // stays assignability-only. The written group supplies the gate
        // (an untyped literal's inferred element face is not modeled here,
        // so its leaves stay silent).
        let nested = engine.group_has_nested_args(left);
        let helper_typed = engine.typed_set_or_map_literal_ty(left);
        let (mut element, mut key, mut value) = (None, None, None);
        if !nested
            && let Some((_, Ty::Named { args, .. })) = helper_typed.as_ref()
        {
            let args = engine.boundary.args.borrow().get(*args).to_vec();
            match args.as_slice() {
                [only] => element = Some(*only),
                [k, v] => {
                    key = Some(*k);
                    value = Some(*v);
                }
                _ => {}
            }
        }
        engine.check_collection_void_elements(left, right, element, key, value);
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
                    engine.literal_row_open = Some((right.range.offset, left.range.offset));
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
            // The spread-element verifier (`_verifySpreadForListOrSet` /
            // `_verifySpreadForMap`) and the `expression_in_map` face run
            // on the live element rows before the drain below.
            match args.len() {
                1 => engine.check_spread_elements(left, right, SpreadLiteralKind::Set),
                2 => {
                    engine.check_spread_elements(left, right, SpreadLiteralKind::Map);
                    engine.check_expression_in_map(left, right);
                }
                _ => {}
            }
            // The literal folds to one expression node upstream: its
            // element rows belong to the node, not the stack — drain them
            // into the deferred pass-2 queue exactly like the list
            // literal's close does, so the enclosing single-row faces
            // (`x = <int>{1}`'s `invalid_assignment`, `init_mark`'s
            // `rows.len() == mark + 1` window) see the literal's own row
            // alone. Element checks above already read their rows; the
            // drained rows still owe their own reports through pass 2.
            let literal_end = right.range.end();
            let split = engine
                .rows
                .iter()
                .position(|row| {
                    row.range.offset > left.range.offset && row.range.end() <= literal_end
                })
                .unwrap_or(engine.rows.len());
            for row in engine.rows.drain(split..) {
                engine.deferred_miss_rows.push(row);
            }
            engine.push_typed_value_row(range, ty);
            return;
        }
        // The literal's row-model side for the downstream typing faces:
        // `resolved_kind` (computed above the const block) is the
        // oracle's decision — the empty form's `EmptyBraces` marker, a
        // decided side's own kind, or NOTHING for the ambiguous literal
        // (upstream records `dynamic`; an `untyped_literals` entry would
        // let the assignability faces spell a `Map` the oracle never
        // typed).
        match resolved_kind {
            Some(SpreadLiteralKind::Set) => {
                engine
                    .untyped_literals
                    .insert(right.range.offset, UntypedLiteral::SetElements);
            }
            Some(SpreadLiteralKind::Map) if count == 0 => {
                engine
                    .untyped_literals
                    .insert(right.range.offset, UntypedLiteral::EmptyBraces);
            }
            // An ambiguous literal resolves `dynamic` upstream, and the
            // `None` answer suppresses its element VERIFIERS below — but
            // the typing faces downstream still read the literal's
            // `MapEntries` marker (the promotion/assignability asks the
            // legacy shape answered before the resolved-kind verdict
            // existed; dropping the entry starved `??=` promotion and
            // return-context reads — ticket_3534 regression #1).
            _ => {
                engine
                    .untyped_literals
                    .insert(right.range.offset, UntypedLiteral::MapEntries);
            }
        }
        // The untyped literal's map-context entry face: a written
        // `Map<K, V>` in the direct initializer / arrow-body / `return`
        // position donates its type arguments to the entry checks
        // (`typed_literal_resolver.dart` `_fromContextType` →
        // literal_element_verifier.dart).
        if let Some((k, v)) = engine.map_entry_context(left) {
            engine.check_map_literal_entries(left, right, k, v);
        }
        // The spread-element verifier and the `expression_in_map` face run
        // on the resolved kind (upstream `literal_element_verifier.dart`
        // `_verifyElement`'s dispatch). An ambiguous literal resolves to
        // `dynamic` upstream — `visitSetOrMapLiteral`'s `isMap`/`isSet`
        // dispatch runs NEITHER verifier, so both faces abstain here.
        match resolved_kind {
            Some(SpreadLiteralKind::Set) => {
                engine.check_spread_elements(left, right, SpreadLiteralKind::Set);
            }
            Some(SpreadLiteralKind::Map) => {
                engine.check_spread_elements(left, right, SpreadLiteralKind::Map);
                engine.check_expression_in_map(left, right);
            }
            _ => {}
        }
        engine.push_value_row(right);
        // The row sits at the `}` — the is-check verdict anchors the
        // literal operand at the literal's own `{` (the node offset). A
        // typed literal's row already spans the whole literal and took
        // the early return above.
        engine.literal_row_open = Some((right.range.offset, left.range.offset));
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

        // The `{?k: v}`/`{k: ?v}` null-aware entry faces (upstream
        // `visitMapLiteralEntry` :1714-1729 → `_checkForUnnecessaryNullAware`
        // with `kind: mapEntryKey`/`mapEntryValue`): a strictly non-nullable
        // key/value operand makes the `?` dead — the report anchors the `?`
        // token ("The map entry key/value can't be null, so the null-aware
        // operator '?' is unnecessary."). The operand row is the closest
        // row wholly before its `?` marker.
        // The operand row is the expression immediately AFTER the `?`
        // marker (upstream `node.key2`/`node.value2`): for the value face
        // it is the last row pushed (`{k: ?v}`'s `v` completes right before
        // the entry event); for the key face it is the row ending before
        // the `:` separator (`{?k: v}`'s `k` sits left of it while `v` is
        // already stacked).
        let engine = &mut self.engine;
        for (question, side, before) in [
            (nk, "key", Some(colon.range.offset)),
            (nv, "value", None),
        ]
        .into_iter()
        .filter_map(|(q, side, b)| q.map(|q| (q, side, b)))
        {
            let pos = match before {
                Some(bound) => engine
                    .rows
                    .iter()
                    .rposition(|row| row.range.end() <= bound),
                None => engine.rows.len().checked_sub(1),
            };
            let Some(pos) = pos else {
                continue;
            };
            // `node.key.staticType` / `node.value.staticType` — the
            // operand's composite type (see `handle_spread_expression`'s
            // note), never the row's push-time payload.
            let ty = engine.operand_static_ty(&engine.rows[pos]);
            if engine.strictly_non_nullable(ty) {
                // The `_mapEntryKey`/`_mapEntryValue` uniqueNames' verbatim
                // pair (diagnostic.g.dart invalidNullAwareMapEntryKey/Value
                // :9418-9445): the side-named message and the bare-remove
                // correction, never the `{0}`/`{1}` replace template.
                let code = SemanticCode::InvalidNullAwareOperator;
                engine.report.diagnostics.push(Diagnostic::new(
                    code.severity(),
                    code.code_info(),
                    format!(
                        "The map entry {side} can't be null, so the null-aware operator '?' is unnecessary."
                    ),
                    engine.file,
                    question.range,
                    Some("Try removing the operator '?'.".to_string()),
                ));
            }
        }
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
        let open_offset = engine
            .record_marks
            .last()
            .map_or(u32::MAX, |(_, open)| open.range.offset);
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
            // A parenthesized top-level `??` (`(a ?? b).m`) types as the
            // if-null join `end_if_null` staged for its right operand row —
            // the inner rows are the operands, never the expression.
            Some(row)
                if let Some(&(lhs_offset, ty)) = engine.if_null_results.get(&row.range.offset)
                    && lhs_offset >= open_offset =>
            {
                ty
            }
            // The group's static type is the inner expression's own —
            // `ParenthesizedExpressionImpl.staticType` forwards
            // `expression.staticType` — so the operand-static face the
            // is-check's `leftNode.typeOrThrow` reads answers here too: a
            // `TypeLiteral` (`(C)`, `(p.C)`) types `Type`, a completed send
            // `(f())` its declared return, a `this`/`super` group the
            // container's `thisType`, a member chain the accessor face.
            // Without it every `(expr).member` merge saw an Unknown
            // receiver — `(C).toString() is String`,
            // `(C()).toString() is String` and their nested forms never
            // reached `check_type_check` (the operand row's verdict
            // suppresses on Unknown).
            //
            // The widening is gated on the last row BEING the group's whole
            // inner expression: its operand head (`operand_expr_head`'s
            // backward walk — bracket closers jump to their openers, a
            // `new`/`const` keyword and a receiver head join) reaches the
            // first significant byte after the `(`. A composite's tail row
            // (the `C` of `(a + C)` — a binary pushes no result row) heads
            // at its own operand, answers only that operand's face, and the
            // opaque face stands.
            Some(row)
                if open_offset != u32::MAX && {
                    let bytes = engine.source.as_bytes();
                    let from = engine.byte_of(open_offset + 1).min(bytes.len());
                    let head_byte = trim_ws(bytes, from, bytes.len());
                    head_byte < bytes.len()
                        && operand_expr_head(engine, row.range.offset)
                            == engine.utf16_of(head_byte)
                } =>
            {
                let ty = engine.operand_static_ty(row);
                if ty != Ty::UNKNOWN {
                    ty
                } else {
                    // An unsolved creation's row keeps no `ty` (the
                    // generic head's inference stays open), but the
                    // parenthesized `InstanceCreationExpression` still
                    // types as its container — `(C())` reads `C`
                    // regardless of the argument slots.
                    match &row.shape {
                        RowShape::Receiver(Receiver::Created { name, kind, .. })
                            if *kind != ContainerKind::Extension =>
                        {
                            Ty::named(*name)
                        }
                        _ => Ty::UNKNOWN,
                    }
                }
            }
            // The tail row of a composite (or any inner face the widened
            // arm does not claim): the push-time `ty` stands — the
            // pre-widening behavior.
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
        // The row sits at the `)` — the is-check verdict anchors the
        // parenthesized operand at its `(` (the oracle's `.at(node)`,
        // the ParenthesizedExpression's own offset). A recovery shape
        // with no open bracket keeps the opaque face.
        if open_offset != u32::MAX {
            engine.literal_row_open = Some((token.range.offset, open_offset));
        }
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
        // Peeked before `record_field_tys` pops the window: the literal's
        // own `(`, the anchor the is-check verdict reads when this row
        // becomes an operand (the oracle's `.at(node)` sits at the
        // RecordLiteral's offset, not its `)`).
        let open_offset = engine
            .record_marks
            .last()
            .map_or(u32::MAX, |(_, open)| open.range.offset);
        if let Some(fields) = engine.const_capture.record_literal(count, token.range) {
            let failed = engine.check_const_elements(SemanticCode::NonConstantRecordField, &fields);
            let payload = engine.record_payload(&fields, &names);
            engine.push_literal_value(
                failed,
                token.range,
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
                // The field's `void` face (#3249): a record literal's field
                // value is consumed by the literal, so the oracle reports
                // `use_of_void_result` at each `void` field (upstream Dart
                // SDK source, pkg/analyzer/lib/src/dart/resolver/
                // record_literal_resolver.dart:159 `.at(field)`). The field
                // window drains below, so the reports happen while its rows
                // still stand — one per `void` field, in source order. The
                // anchor is the FIELD NODE's extent, never the value row's
                // own token: a declared-method send's row spells only the
                // callee name (`v` of `v()`), so the value widens through
                // its completed send / member chain first. A NAMED field is
                // a `RecordLiteralNamedField` (`name: value` — the trailing
                // `named.len()` field rows, positional fields precede
                // them), so its extent re-opens at the label.
                let void_fields = positional.iter().filter(|ty| **ty == Ty::Void).count()
                    + named.iter().filter(|entry| entry.1 == Ty::Void).count();
                if void_fields > 0 {
                    let mut anchors: Vec<SourceRange> = Vec::new();
                    {
                        let field_rows: Vec<&Row> = engine.rows[mark..]
                            .iter()
                            .filter(|row| !matches!(row.shape, RowShape::TypeUse(..)))
                            .collect();
                        let first_named = field_rows.len().saturating_sub(named.len());
                        for (index, row) in field_rows.iter().enumerate() {
                            if anchors.len() == void_fields {
                                break;
                            }
                            if engine.operand_static_ty(row) != Ty::Void {
                                continue;
                            }
                            let value = engine
                                .invocation_span(row.range)
                                .or_else(|| engine.property_access_span(row.range))
                                .unwrap_or(row.range);
                            anchors.push(if index >= first_named {
                                engine.named_field_span(value).unwrap_or(value)
                            } else {
                                value
                            });
                        }
                    }
                    for anchor in anchors {
                        if !engine.void_result_reported_at(anchor) {
                            engine.emit(SemanticCode::UseOfVoidResult, anchor, "", "");
                        }
                    }
                }
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
        if open_offset != u32::MAX {
            engine.literal_row_open = Some((token.range.offset, open_offset));
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
        // An `augment` field group declares no new field: it owns no model
        // row for the inferred-type overlay, while its initializer
        // obligation stays its own — the declarator gate exempts only the
        // node's own `abstract`/`external` head and an extension type's
        // representation augment.
        engine.group_augment = _augment_kw.is_some();
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
        // A static or `late` group's initializers sit OUTSIDE the primary
        // constructor's `ConstructorInitializerScope`
        // (pkg/analyzer/lib/src/dart/resolver/scope_context.dart:432-461):
        // the formal names stay out of scope for the whole group, and the
        // close below restores them (the constructor body follows the
        // class's field groups).
        engine.primary_formals_muted = static_kw.is_some() || late_kw.is_some();
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
        engine.group_augment = false;
        engine.primary_formals_muted = true;
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
        engine.group_augment = false;
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
        // An `await` is never a constant expression (upstream's constant
        // verifier flags the whole AwaitExpression — `const x = await f()`
        // reports at `await f()`, the `await` keyword through the operand's
        // end): close the group with the whole-expression span so the
        // non-constant leaf it pushes anchors where the oracle anchors.
        engine.const_capture.end_opaque_group(SourceRange {
            offset: begin.range.offset,
            length: end.range.end().saturating_sub(begin.range.offset),
        });
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
        let operand = engine
            .rows
            .last()
            .filter(|row| row.range.offset >= begin.range.end())
            .cloned()
            .map(|row| (engine.operand_static_ty(&row), row));
        // The void-operand use (upstream `visitAwaitExpression` →
        // `checkForUseOfVoidResult(node.expression)`): `await x` on a
        // `void`-typed `x` reports at the operand's use-site span.
        if let Some((Ty::Void, row)) = &operand {
            let range = engine.use_site_range(row);
            engine.emit(SemanticCode::UseOfVoidResult, range, "", "");
        }
        let flat_ty = operand.map(|(ty, _)| engine.flatten(ty));
        if let (Some(row), Some(ty)) = (engine.rows.last_mut(), flat_ty) {
            row.ty = ty;
        }
        // A bottom-typed await (`await Future<Never>.error(..)`) is the
        // expression-level `handleExit` (type_analyzer.dart:815-819): the
        // flow state dies at the await's end, latched like a Never send.
        let bottom = flat_ty.is_some_and(|ty| ty == Ty::Never || engine.is_never_ty(ty));
        // The extent outlives the rewrite: a member operand's row resolves
        // only in the later pass (its close-time `ty` is still `Unknown`),
        // so the `var`-inferred bind re-applies the flatten over the
        // resolved type (#2506 await face — `final d = await t.r;` binds
        // `R`, not `Future<R?>`).
        engine
            .await_operand_extents
            .push((begin.range.end(), end.range.offset));
        if bottom {
            self.pending_exit = Some(end);
        }
    }

    fn handle_throw_expression(&mut self, throw: Token, end: Token) {
        self.flow().handle_throw_expression(throw, end);

        // `ThrowExpression` is in the official used set (the event fires
        // after the operand parsed).
        let engine = &mut self.engine;
        engine.nsm_poison();
        engine.ur_seal_used();
        // The oracle anchors the throw's `InvalidConstant` at the whole
        // `throw expr` node, not the keyword — the leaf spans the `throw`
        // keyword through the operand's end.
        engine.const_capture.throw_expression(SourceRange {
            offset: throw.range.offset,
            length: end.range.end() - throw.range.offset,
        });
        // The throw's static type is `Never` (the resolver records
        // `NeverTypeImpl`, `resolver_visitor.dart` `visitThrowExpression`):
        // the operand's row is the stack top and carries the throw's value
        // downstream, so the assignment/argument checks read the bottom type
        // and never report its operand's own type (`b = throw 'x'` assigns
        // into `bool` — `subtype.dart` Right Bottom). The void-operand use
        // fires first (upstream `visitThrowExpression` →
        // `checkForUseOfVoidResult(node.expression)`): `throw x` on a
        // `void`-typed `x` reports at the operand before the row's type is
        // rewritten to `Never`.
        let void_operand = engine
            .rows
            .last()
            .filter(|row| engine.operand_static_ty(row) == Ty::Void)
            .cloned()
            .map(|row| engine.use_site_range(&row));
        if let Some(range) = void_operand {
            engine.emit(SemanticCode::UseOfVoidResult, range, "", "");
        }
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
        // The constructor body leaves the initializing-formal scope: a
        // `this.x` name reads the field from here on.
        let this_formals = std::mem::take(&mut self.engine.this_formal_names);
        if !this_formals.is_empty()
            && let Some(frame) = self.engine.frames.last_mut()
            && matches!(frame.kind, FrameKind::Executable { .. })
        {
            for (name, serial) in this_formals {
                // Only the constructor that declared the formal: a
                // body-less constructor's leftovers never touch the next
                // executable's own parameters.
                if serial != frame.serial {
                    continue;
                }

                frame.locals.remove(&name);
                frame.var_types.remove(&name);
            }
        }
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

    /// The collection-element `for` head: opens the loop frame, the
    /// condition window and the const capture's `for`-element latch (a
    /// constant collection rejects a `for` element, so the element's extent
    /// is reported when the owning literal closes).
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
        // The element head (the `await`/`for` keyword): a constant collection
        // rejects a `for` element outright, and this head is what the
        // element's whole extent anchors at when the literal closes.
        engine.const_capture.begin_ctl_element(
            crate::const_eval::CtlElementKind::For,
            await_kw.unwrap_or(for_kw).range,
        );
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
        self.engine.const_capture.begin_ctl_element(
            crate::const_eval::CtlElementKind::If,
            token.range,
        );
        // The collection form's condition arms the same window the
        // statement's does when it carries a case clause (the element face
        // of `begin_pattern`'s gate). The parser pairs this event with
        // `handle_then_control_flow` unconditionally, so the depth always
        // tears down at the condition's own end.
        self.engine.if_element_cond += 1;
        // The element's case scope opens the same way the statement's does
        // (`visitIfElement` runs the same `_resolveGuardedPattern` arm as
        // `visitIfStatement` — resolution_visitor.dart:569-583): only
        // patterns inside this condition belong to the element's case
        // scope, so a stale shadow list from an enclosing arm or sibling
        // clears here.
        self.engine.if_condition_begin();
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
        let end = element_end_before(&self.engine, token);
        let engine = &mut self.engine;
        engine.const_capture.close_ctl_element(
            crate::const_eval::CtlElementKind::For,
            end,
            false,
            false,
        );
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
        let end = element_end_before(&self.engine, token);
        let engine = &mut self.engine;
        engine.const_capture.close_ctl_element(
            crate::const_eval::CtlElementKind::For,
            end,
            false,
            false,
        );
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
        // An unnamed GFT formal's pending type claims at its own close (a
        // named formal's identifier event already claimed it — no-op).
        self.engine.fn_capture_formal_unnamed();
        // A captured function level's formal: the name pairs with the type
        // node that just landed (named formals trail the positional ones —
        // the sink guards itself to function levels).
        {
            let named = matches!(
                kind,
                FormalParameterKind::RequiredNamed | FormalParameterKind::OptionalNamed
            );
            let formal_name = named.then(|| self.engine.name_of(name)).flatten();
            self.engine.bounds.function_formal_end(named, formal_name);
        }
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
        if self.engine.formal_written.is_none() && this_keyword.is_none() && super_keyword.is_none()
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
            } else if !self.engine.formal_has_default
                && member_kind == dartq_parser::MemberKind::PrimaryConstructor
                && matches!(
                    self.engine.container_context(),
                    Some((_, ContainerKind::ExtensionType))
                )
            {
                // The #2843 extension-type face: an untyped representation
                // formal reads `Object?` — the implicit final representation
                // field's declared type (the same face the collector's member
                // entry carries) — so the primary constructor body's reads of
                // the formal run the nullable walk. The body sits after the
                // formal's close, so the rebind here precedes every read; a
                // default value keeps the dynamic posture, mirroring the
                // collector's face.
                if let Some(name_id) = self.engine.name_of(name) {
                    let object_ty = Ty::Named {
                        name: self.engine.intern("Object"),
                        nullable: true,
                        args: ArgListId::EMPTY,
                    };
                    self.engine.bind_var_type(name_id, object_ty);
                }
                let base = self.flow().intern_name("Object");
                self.flow().stage_inferred_local(
                    name.range.offset,
                    FlowTy {
                        base: FlowBase::Name(base),
                        nullable: true,
                    },
                    Vec::new(),
                );
            }
        }
        // The primary constructor's formals answer only from its own two
        // upstream scopes (`ConstructorInitializerScope` over the non-late
        // INSTANCE field initializers and `PrimaryParameterScope` over the
        // `this : ...` body —
        // pkg/analyzer/lib/src/dart/resolver/scope_context.dart:406-461);
        // the member scope never sees them. The container frame still
        // carries them as locals (the body needs them), so the names are
        // marked here for [`Engine::resolve_local`] to refuse in the
        // illegal planes (static and `late` field initializers, member
        // bodies) where the oracle answers `undefined_identifier`.
        if member_kind == dartq_parser::MemberKind::PrimaryConstructor
            && self.engine.formal_list_depth == 1
            && let Some(primary) = self.engine.name_of(name)
        {
            self.engine.primary_formals.insert(primary);
        }
        // #3419 — the explicit-formal receiver injection: the anonymous
        // method's first plain required positional formal (no written
        // annotation, no function-typed suffix — the collector's
        // `first_param` latch) carries the RECEIVER's static type
        // upstream (the element type is set before the body resolves,
        // promoted to non-null under `?.`/`?..`). The engine-side bind
        // rides the enclosing frame (the body opens none of its own);
        // the flow side stages the same face on the formal's own token
        // so the body's promotions see it — only the argument-less
        // named shape stages (the flow lattice's spellability gate, the
        // #2843 pattern).
        if member_kind == dartq_parser::MemberKind::AnonymousMethod
            && let Some(this_ty) = self
                .engine
                .anon_method_body_at(name.range.offset)
                .filter(|entry| entry.first_param == Some(name.range))
                .and_then(|entry| self.engine.anon_this_ty(entry))
            && let Some(name_id) = self.engine.name_of(name)
        {
            self.engine.bind_var_type(name_id, this_ty);
            if let Ty::Named {
                name: ty_name,
                nullable,
                args: ArgListId::EMPTY,
            } = this_ty
            {
                let lexeme = self.engine.strings.resolve(ty_name).to_string();
                let base = self.flow().intern_name(&lexeme);
                self.flow().stage_inferred_local(
                    name.range.offset,
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
        // container's flush consults. A `this`/`super` formal never joins
        // the explicit-super set: upstream's implicit-super face
        // early-returns on a `SuperConstructorInvocation` /
        // `RedirectingConstructorInvocation` initializer ENTRY alone
        // (error_verifier.dart:7182-7189) — a formal is not one — and the
        // formals' supplies ride the granular super-formal counts the flush
        // reads off the captured formal table instead
        // (`verifySuperFormalParameters`, error_verifier.dart:7244-7258).
        if let Some(site) = self.engine.open_ctor_site
            && let Some(name_id) = self.engine.name_of(name)
            && this_keyword.is_some()
        {
            self.engine.ctor_formals_init.insert((site, name_id));
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
        // A `this.`/`super.` formal's name event arrives under the
        // `FieldInitializer` context — `formal_written` never latches for
        // it, while the armed annotation still stands until the
        // `bind_parameter_type` below consumes it. The default-value check
        // reads the same written face (`{int super.a}` gates `int`
        // non-nullable, error_verifier.dart's `parameterElement.type`).
        let written = written.or_else(|| {
            (this_keyword.is_some() || super_keyword.is_some())
                .then(|| self.engine.armed_ty)
                .flatten()
        });
        // The unannotated receiver formal's merged type is the bound
        // field's own (`ET.c({this.id})` reads the representation `id`'s
        // `int` — the upstream initializing formal's element type), which
        // the `in_ctor` bind below resolves through `member_declared_ty`.
        // A `super.` formal reads the super-constructor's same-named
        // parameter type — whose `this.` shape the superclass's field face
        // approximates (`S({this.s2})` types `s2` at the field's `int`).
        let written = written.or_else(|| {
            if this_keyword.is_none() && super_keyword.is_none() {
                return None;
            }
            let name_id = self.engine.name_of(name)?;
            let (container, _) = self.engine.container_context()?;
            let owner = if this_keyword.is_some() {
                container
            } else {
                resolve_container(
                    self.engine.graph,
                    self.engine.namespaces,
                    self.engine.index,
                    self.engine.lib,
                    self.engine.file,
                    container,
                )
                .and_then(|(decl, _)| decl.superclass)?
            };
            let ty = self.engine.member_declared_ty(owner, name_id);
            (ty != Ty::UNKNOWN).then_some(ty)
        });
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
                    // #2937 (3c): the old-style formal's callable signature —
                    // `end_function_typed_formal_parameter` sealed the nested
                    // list's `FnVarSig` into `fn_type_stash`; the formal
                    // claims it for the invocation face's formals exactly
                    // like the modern `Function(...)` spelling's
                    // `FormalParameterDeclaration` claim (upstream's
                    // `FunctionType` on the parameter element; `b('0')` over
                    // `b(int p)` counts against `int`).
                    if let Some(sig) = self.engine.fn_type_stash.take() {
                        self.engine.fn_var_sigs.insert(name_id, sig.clone());
                        self.engine.fn_param_sigs.insert(name_id, sig.clone());
                        if let Some(exec) =
                            self.engine.frames.last().and_then(|f| match &f.kind {
                                FrameKind::Executable { ret: Some(ret), .. } => {
                                    Some(ret.name)
                                }
                                _ => None,
                            })
                        {
                            let table = self.engine.fn_formal_sigs.entry(exec).or_default();
                            if self.engine.fn_formal_named_section {
                                table.named.insert(name_id, sig);
                            } else {
                                table.positional.push(sig);
                            }
                        }
                    }
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
            if this_keyword.is_some()
                && self.engine.formal_list_depth == 1
                && let Some(serial) = self.engine.frames.last().map(|frame| frame.serial)
            {
                self.engine.this_formal_names.push((name_id, serial));
            }
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
                // The flow layer's formal slot takes the same field face:
                // an untyped `this.p` declares raw-unknown there, so the
                // initializer list's `p == null || p.isNotEmpty` had no
                // declared type to promote from.
                if let Ty::Named {
                    name: ty_name,
                    nullable,
                    args,
                } = ty
                {
                    let lexeme = self.engine.strings.resolve(ty_name).to_string();
                    let tree = staged_arg_tree(
                        &self.engine.boundary.args.borrow(),
                        &self.engine.strings,
                        args,
                    )
                    .unwrap_or_default();
                    let base = self.flow().intern_name(&lexeme);
                    self.flow().stage_inferred_local(
                        name.range.offset,
                        FlowTy {
                            base: FlowBase::Name(base),
                            nullable,
                        },
                        tree,
                    );
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
        // The window level seals into its function node (the mirror of
        // `begin_function_type`'s routing; the range spans the keyword
        // through the written `?`, the same extent the collector arms).
        let range = SourceRange {
            offset: function.range.offset,
            length: question.map_or(function.range, |q| q.range).end() - function.range.offset,
        };
        self.engine.bounds.end_function(question.is_some(), range);
        self.flow().end_function_type(function, question);
        self.engine.var_end_function_type(question.is_some());
        // The written signature's required-positional count: the capture
        // the close pops still carries it here, and the interned shape
        // `fn_capture_end` arms merges required and optional positionals
        // into one column. Record the split into `fn_shape_required` keyed
        // by the resulting `FnShapeId` — the subtype function rule's
        // `required` gate (`_isFunctionSubtypeOf`) cannot read it off the
        // canonical shape (where `Function(int)` and `Function([int])`
        // collapse to one id). The FIRST spelling's count wins the map
        // slot: declarations seal before the `is` targets that reuse the
        // shape, so the shared id keeps the declared face's count while
        // `last_fn_seal` answers each written site's own.
        let required = self
            .engine
            .fn_type_capture
            .last()
            .map(|capture| capture.required_positional.min(u16::MAX as usize) as u16);
        self.engine.fn_capture_end(question.is_some());
        if let (Some(required), Some(Ty::Function { shape, .. })) =
            (required, self.engine.armed_ty)
            && shape != crate::model::FnShapeId::EMPTY
        {
            self.engine
                .fn_shape_required
                .entry(shape)
                .or_insert(required);
            self.engine.last_fn_seal = Some((shape, required));
        }
        // The #1351 capture: a typedef RHS function type's own close
        // latches its nullability suffix (`typedef W = void Function()?;`
        // expands nullable); a nested function-typed formal's close is
        // not the RHS's own and leaves the latch alone.
        self.engine.bounds.alias_rhs_question(question.is_some());
        // The written extent's true end: the trailing `?` when it belongs
        // to this starter, else the formals' closing paren the formal
        // latch carries. The pass-2 walk fires innermost-first, so the
        // outer type's close lands last and the slot holds the full
        // written shape (`void Function(int, [List<T>]) Function(int x)`
        // ends at the outer formals' `)`).
        let engine = &mut self.engine;
        let written_end = question.map_or_else(
            || {
                engine
                    .fn_formal_closer_end
                    .take()
                    .unwrap_or(function.range.end())
            },
            |q| q.range.end(),
        );
        engine.written_ty_span = Some(SourceRange {
            offset: function.range.offset,
            length: written_end - function.range.offset,
        });
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
        // #2937 (3c): seal the old-style formal's signature capture — the
        // dedicated frame `begin_function_typed_formal_parameter` pushed.
        // The nested list's formals are its positional/named columns and
        // the return annotation is its `ret`; `b('0')` then counts against
        // this shape through `fn_param_sigs` exactly like the modern
        // `Function(...)` spelling (the desugared `FunctionType` on the
        // parameter element). A poisoned capture (a stray/unnamed nested
        // formal) stays the under-report posture.
        if let Some(capture) = self.engine.fn_type_capture.pop() {
            if !capture.poisoned {
                let return_ty = capture.ret.map_or(Ty::Dynamic, |(ty, ..)| ty);
                self.engine.fn_type_stash = Some(Box::new(FnVarSig {
                    params: capture.params.into_boxed_slice(),
                    required_positional: capture
                        .required_positional
                        .min(u16::MAX as usize) as u16,
                    positional: capture.positional.into_boxed_slice(),
                    named: capture.named.into_boxed_slice(),
                    return_ty,
                }));
            }
        }
    }

    fn end_if_control_flow(&mut self, token: Token) {
        self.flow().end_if_control_flow(token);
        // The element completes with no else (the parser fires this only
        // when `else` is absent — literal_entry_info_impl.dart
        // `IfComplete.parse`): a still-open case scope closes, the
        // `IfElement` arm of `end_if_statement`.
        let end = element_end_before(&self.engine, token);
        self.engine.const_capture.close_ctl_element(
            crate::const_eval::CtlElementKind::If,
            end,
            false,
            false,
        );
        self.engine.if_statement_end();
    }

    fn end_if_else_control_flow(&mut self, token: Token) {
        self.flow().end_if_else_control_flow(token);
        // The else element completes: the case scope already emptied at
        // `handle_else_control_flow`; this pops the cleared entry — the
        // `IfElement` arm of `end_if_statement`.
        let end = element_end_before(&self.engine, token);
        self.engine.const_capture.close_ctl_element(
            crate::const_eval::CtlElementKind::If,
            end,
            true,
            false,
        );
        self.engine.if_statement_end();
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
        // (`[a, var d] = y;` reports `a` — the verdict emits here instead
        // of a deferred row). The write never counts as a use (#2858): the
        // official verifier never visits an assigned-variable pattern
        // (`AssignedVariablePatternImpl`'s `visitChildren2` is empty, no
        // verifier override) and the write path skips
        // `LocalVariableElement` — a pattern-assignment write spares
        // nothing, so the hit only proves the name defined.
        let engine = &mut self.engine;
        let Some(name) = engine.name_of(variable) else {
            return;
        };
        engine.last_pattern_variable = Some((name, variable.range));
        if engine.resolve_local(name).is_some() {
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

    fn handle_catch_trace_parameter(&mut self) {
        // The written comma: the clause's following catch-parameter events
        // fill the stack-trace slot (the engine's classification rides the
        // parser's comma fact, never the event arrival order).
        self.engine.catch_trace_slot = true;
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
        // The else entry opens: the then entry's case scope closes — the
        // `IfElement` arm of the statement's `begin_else_statement` wiring
        // (the else resolves a shadowed spelling on the OUTER declaration).
        self.engine.if_then_end();
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
                Some((target_decl, target_file)) => {
                    // The faces key on a resolved CONSTRUCTOR element
                    // upstream (`redirectedElement == null` early-returns
                    // into the missing-constructor face — a class with no
                    // matching constructor never reports here): the
                    // selector joins through the container's selector set,
                    // the unnamed spellings (the head itself, `new`)
                    // through the implicit/explicit unnamed face.
                    if engine.ctor_face_exists(target_decl, target_container, target) {
                        // The message spells the instantiated faces
                        // (#2878; the oracle's `_checkForRedirectToInvalidReturnType`
                        // renders the resolved types): the target
                        // instantiated to its own parameter bounds (the
                        // bare `= C.create` reading reads `C<dynamic>`
                        // under an unbounded `C<T>` — instantiate-to-
                        // bounds; the written arguments of the target
                        // reference are not modeled here), the enclosing
                        // class with its own parameters (`Link<T>` — the
                        // factory's declared return face). The bounds ride
                        // only for a LOCAL target — a foreign container's
                        // bound slots are its own pool's, never rendered
                        // through this one — and the subtype verdict reads
                        // the heads only, so the enriched faces decide
                        // exactly as the bare names did.
                        let from_ty = if target_file == engine.file {
                            let mut args = engine.boundary.args.borrow_mut();
                            args.named(target_container, false, &target_decl.type_param_bounds)
                        } else {
                            Ty::named(target_container)
                        };
                        let to_ty = resolve_container(
                            engine.graph,
                            engine.namespaces,
                            engine.index,
                            engine.lib,
                            engine.file,
                            container,
                        )
                        .map_or_else(
                            || Ty::named(container),
                            |(decl, _)| {
                                let type_args: Vec<Ty> = decl
                                    .type_param_names
                                    .iter()
                                    .copied()
                                    .map(Ty::named)
                                    .collect();
                                let mut args = engine.boundary.args.borrow_mut();
                                args.named(container, false, &type_args)
                            },
                        );
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

    fn handle_for_updaters_end(&mut self, updaters_last: Token) {
        // Record `Ev::ForParts.updaters_end`: the flow side trims a
        // header-anchored dead row to the updaters' own last token
        // (upstream Dart SDK source, dead_code_verifier.dart:309-310), and
        // dead updaters report at their full extent
        // (dead_code_verifier.dart:357-378 `for_updaterBegin`).
        self.flow().handle_for_updaters_end(updaters_last);
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
        // The `?e` collection element (upstream `visitNullAwareElement`
        // :1912-1918 → `_checkForUnnecessaryNullAware(_, node.question,
        // kind: element)`): when the element VALUE's static type is
        // strictly non-nullable the `?` is dead — oracle reports at the `?`
        // token alone ("The element can't be null, so the null-aware
        // operator '?' is unnecessary."). The operand (`node.value2`) is the
        // expression AFTER the `?` — the last row pushed, which completes
        // right before the marker event fires.
        let engine = &mut self.engine;
        if let Some(pos) = engine.rows.len().checked_sub(1) {
            // `node.value.staticType` — the composite type (see
            // `handle_spread_expression`'s note): an untyped collection
            // literal (`?[1, 2]`) answers its inferred `List`, a send its
            // result rather than the callee's own type. A still-unfolded
            // compound element (`?x += 1` leaves `[x, 1]` on the stack —
            // the assignment event fires after this one) reads the
            // AssignmentExpression's resolved-operator result
            // (`compound_element_ty`), not the RHS literal's face the tail
            // row carries.
            let ty = engine
                .compound_element_ty()
                .unwrap_or_else(|| engine.operand_static_ty(&engine.rows[pos]));
            if engine.strictly_non_nullable(ty) {
                // The `_element` uniqueName's verbatim pair
                // (diagnostic.g.dart invalidNullAwareElement :9404-9415):
                // the shared-name row's own message and the bare-remove
                // correction, never the `{0}`/`{1}` replace template.
                let code = SemanticCode::InvalidNullAwareOperator;
                engine.report.diagnostics.push(Diagnostic::new(
                    code.severity(),
                    code.code_info(),
                    "The element can't be null, so the null-aware operator '?' is unnecessary."
                        .to_string(),
                    engine.file,
                    token.range,
                    Some("Try removing the operator '?'.".to_string()),
                ));
            }
        }
    }

    fn handle_pattern_assignment(&mut self, equals: Token) {
        self.flow().handle_pattern_assignment(equals);
        // `PatternAssignment` is in the official used set (the event fires
        // after the RHS parsed).
        let engine = &mut self.engine;
        engine.nsm_poison();
        engine.ur_seal_used();
        // The irrefutable walk over the assignment's left side (#3640).
        engine.irrefutable_walk_pattern_assignment(equals);
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
        // The flow-side staging rides the same bind (the destructured
        // local's VarMeta declared raw-unknown; the element face reaches
        // it through `inferred_locals`, the #2567 seam).
        for (name, span, ty) in self
            .engine
            .destructure_record(equals.range.end(), semicolon.range.offset)
        {
            if let Ty::Named {
                name: head,
                nullable,
                ..
            } = ty
            {
                let lexeme = self.engine.strings.resolve(head).to_string();
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

    fn handle_recoverable_error(&mut self, p0: Message, p1: Token, p2: Token) {
        // The row joins the fold's parse face (a SYNTACTIC_ERROR, always
        // ERROR-severity) beside the flow forward below. The span follows
        // the official `handleRecoverableError` formula (analyzer
        // `fasta/ast_builder.dart`: offset = start token's offset, length =
        // end token's end − offset): single-token reports pass one token
        // twice and keep the token span, while with-end reports
        // (`reportRecoverableErrorWithEnd`) span begin..end — the native
        // clause row covers `native "x"` (manual face
        // native_clause_should_be_annotation/1: 1:9|10) and the
        // literal-with-`new` row covers `new List` (1:20|8).
        let range = SourceRange {
            offset: p1.range.offset,
            length: p2.range.end() - p1.range.offset,
        };
        self.parse_diagnostics.push(Diagnostic::new(
            Severity::Error,
            p0.code.clone(),
            p0.text.clone(),
            self.file,
            range,
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
        self.engine.const_capture.begin_ctl_element(
            crate::const_eval::CtlElementKind::Spread,
            operator.range,
        );

        // The `...?e` null-aware spread (upstream `visitSpreadElement`
        // :2176-2184 → `_checkForUnnecessaryNullAware(_, spreadOperator,
        // kind: spread)`): a strictly non-nullable spread operand makes the
        // `...?` dead — oracle reports at the whole operator token ("The
        // receiver can't be null, so the null-aware operator '?...' is
        // unnecessary.", len 4 covering `...?`). The operand
        // (`node.expression2`) is the expression AFTER the `...?` token —
        // the last row pushed, which completes right before the event.
        // The `...?` lexeme is the only 4-column spread operator; `...`
        // (len 3) never checks.
        if operator.range.length == 4 {
            let engine = &mut self.engine;
            if let Some(pos) = engine.rows.len().checked_sub(1) {
                // The gate reads `node.expression.staticType` — the
                // operand's COMPOSITE type, never the last row's push-time
                // payload. [`Engine::operand_static_ty`] is that face: a
                // completed send answers the callee's declared return (or
                // the implicit `call` method's return for a callable-value
                // callee), while the row's payload is the CALLEE's own
                // declared type (`...?f()` over `int? f()` read
                // `int? Function()`, strictly non-nullable, and reported a
                // live operator); an untyped collection literal answers its
                // inferred `List`/`Set`/`Map`, while its row is an opaque
                // `Value` (`...?[1, 2]` never reported); a `!`-asserted
                // read answers the assertion's rewritten non-null row type
                // (`...?e!`); a getter/field read its declared accessor
                // type. The `?e` element and the `{?k: v}`/`{k: ?v}`
                // map-entry sites read the same face.
                //
                // Neither flow promotion nor a trailing `??` composite is
                // reachable here: the first needs the teardown join
                // (`Engine::read_types` fills only after pass 1), the
                // second stages its join only when the right operand leaves
                // a row of its own (a folded literal leaves none).
                let ty = engine.operand_static_ty(&engine.rows[pos]);
                if engine.strictly_non_nullable(ty) {
                    engine.emit3(
                        SemanticCode::InvalidNullAwareOperator,
                        operator.range,
                        "?...",
                        "...",
                        "",
                    );
                }
            }
        }
        // The element's operands are staged — fold them into one
        // `Expr::Spread` node (`...`/`...?` differ only by the marker).
        self.engine.const_capture.close_ctl_element(
            crate::const_eval::CtlElementKind::Spread,
            0,
            false,
            operator.range.length == 4,
        );
    }

    fn handle_switch_case_no_when_clause(&mut self, token: Token) {
        self.flow().handle_switch_case_no_when_clause(token);
        // `SwitchStatement` is in the official used set (the case head's
        // constants); the body statements answer through their own events.
        self.engine.ur_seal_used();
    }

    fn end_switch_case_when_clause(&mut self, token: Token) {
        self.flow().end_switch_case_when_clause(token);
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
        // The then-element opens: the condition's pattern shadows become
        // the element's case scope — the `IfElement` arm of the statement's
        // `begin_then_statement` wiring (a case clause scopes its variables
        // to the guard and the then entry only).
        self.engine.if_then_begin();
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

    /// The rest element's close (`...`/`...p` inside a list or map
    /// pattern): marks the innermost open frame so
    /// [`crate::switch_tracker::SwitchTracker::collapse`] can rebuild the
    /// head/rest/tail split the official `createListSpace` produces
    /// (shared.dart:878-905). `has_sub_pattern` distinguishes `...p` (the
    /// leaf before the mark is the rest's own sub-pattern) from a bare
    /// `...` (the mark simply bounds head/tail). A rest mark inside a MAP
    /// pattern's frame collapses to `Unknown` the same way — the map face
    /// is unmodeled, so the mark only records the split, never claims
    /// coverage the world cannot see.
    fn handle_rest_pattern(&mut self, dots: Token, has_sub_pattern: bool) {
        // The structural rest faces (#3629): the enclosing bracket decides
        // — every rest inside a map pattern reports
        // `rest_element_in_map_pattern`, a second rest in one list pattern
        // reports `duplicate_rest_element_in_pattern` (the innermost
        // unclosed group is the suffix's own `[`/`{`).
        super::pattern_faces::rest_pattern_faces(self, dots, has_sub_pattern);
        self.engine.pattern_leaf(Restriction::RestMark {
            had_sub: has_sub_pattern,
        });
    }

    /// The object pattern's field-group close (`parseObjectPatternRest`'s
    /// trailing event, fired before [`Self::handle_object_pattern`]): the
    /// #3629 field faces — `missing_named_pattern_field_name` (a nameless
    /// `: pattern` field whose sub-pattern binds no variable),
    /// `positional_field_in_object_pattern` and `duplicate_pattern_field`.
    /// The fields are already drained into this pattern's own bracket
    /// frame; the per-field spans re-derive from the token table
    /// (resolver.dart:494-508, shared_type_analyzer.dart:78-91).
    fn handle_object_pattern_fields(&mut self, count: usize, begin: Token, end: Token) {
        super::pattern_faces::object_pattern_field_faces(self, count, begin, end);
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
    b.is_ascii_alphanumeric() || b == b'_' || b == b'$'
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

/// The UTF-16 exclusive end of the source just before `token` — the
/// `]`/`,`/`}` that follows a collection element's body. Trailing trivia is
/// walked back, so a `for` element's extent stops at its body (the oracle
/// anchors `const_eval_for_element` at the whole element).
fn element_end_before(engine: &Engine<'_>, token: Token) -> u32 {
    let Some(scanned) = engine.tokens.get(token.id as usize) else {
        return token.range.offset;
    };
    preceding_position(engine.source.as_bytes(), scanned.byte_range().start)
        .map_or(token.range.offset, |byte| engine.utf16_of(byte))
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

/// The entry colon of the collection leaf `[start, end)` (`start` past
/// trivia) — `Some` exactly when the leaf is a map entry (upstream
/// `MapLiteralEntry`, the null-aware `?k: v` and `k: ?v` forms included).
/// A leading `?` is the null-aware marker of `?e` / `?k: v`, never a
/// conditional's; a conditional `?` ahead of the first top-level colon
/// makes the leaf an expression (`c ? a : b`), while the `?` of `??`,
/// `?.`, and `?..` is an operator of its own (`a ?? b: v`, `a?.b: v`).
fn entry_colon(bytes: &[u8], start: usize, end: usize) -> Option<usize> {
    let start = start + usize::from(bytes.get(start) == Some(&b'?'));
    let colon = top_level_colon(bytes, start, end)?;
    let mut from = start;
    while let Some(at) = top_level_question(bytes, from, colon) {
        if !matches!(bytes.get(at + 1), Some(b'?' | b'.')) {
            return None;
        }
        from = at + 2;
    }
    Some(colon)
}

/// The `}` closing the brace group opening at `open` (nested `{`/`}` and
/// quoted strings skipped — an interpolation's braces pair inside the
/// string skip).
fn balanced_brace(bytes: &[u8], open: usize, end: usize) -> Option<usize> {
    let mut depth = 0usize;
    let mut i = open;
    while i < end {
        match bytes[i] {
            b'{' => depth += 1,
            b'}' => {
                depth = depth.checked_sub(1)?;
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

/// Whether the variable group head declaring the name at `name_offset`
/// spells its own `abstract` or `external` keyword — the declaration node's
/// exemption from both uninitialized faces (the `abstractKeyword` /
/// `externalKeyword` gate of `_checkForNotInitializedFieldDeclaration` and
/// `visitTopLevelVariableDeclaration`). The head runs from the previous
/// member boundary to the group's first `=`: a later declarator's
/// initializer can hold a bare `external` identifier, a head cannot.
fn group_head_exempts_init(engine: &Engine<'_>, name_offset: u32) -> bool {
    use dartq_scanner::{Keyword, TokenKind};
    let tokens = engine.tokens.as_slice();
    let at = tokens.partition_point(|token| token.utf16_offset < name_offset);
    let start = tokens[..at]
        .iter()
        .rposition(|token| {
            matches!(
                token.kind,
                TokenKind::Semicolon | TokenKind::OpenCurly | TokenKind::CloseCurly
            )
        })
        .map_or(0, |boundary| boundary + 1);
    tokens[start..at]
        .iter()
        .take_while(|token| token.kind != TokenKind::Eq)
        .any(|token| {
            matches!(
                token.kind,
                TokenKind::Keyword(Keyword::Abstract | Keyword::External)
            )
        })
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
                // A `void` key/value already reported through
                // [`Self::check_collection_void_elements`] — it short-
                // circuits the assignability ask here (upstream
                // `_verifyMapLiteralEntry*` gates on `entry.key|value
                // .typeOrThrow is VoidType` before the use-of-void
                // early-return); an unspellable completion row suppresses.
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

    /// The void-operand use over a list/set/map literal's value elements
    /// (upstream `literal_element_verifier.dart` :95 — `checkElement` →
    /// `checkForUseOfVoidResult(element)`, and the entry arms :151/:156/
    /// :166/:171 — `_verifyMapLiteralEntryKey`/`_Value` →
    /// `checkForUseOfVoidResult(entry.key|entry.value)`): each `void`-typed
    /// leaf expression reports at its own span. A `:`-led map entry splits
    /// at its top-level colon and checks the key and value independently;
    /// a `...`/`...?`-led spread operand answers its own face
    /// (`_verifySpreadForListOrSet` runs no void check) and is left to it.
    ///
    /// The enclosing element / `mapKeyType` / `mapValueType` supplies the
    /// gate: the oracle only consumes the value into a non-`void` slot
    /// (`elementType is! VoidType`, :95; `mapKeyType`/`mapValueType`, :127/
    /// :152/:166/:171), so `<void>[v()]` and an untyped `[v()]` — whose
    /// inferred element is the `void` leaf itself — stay silent while
    /// `<int>[v()]` reports. An unspellable gate (`None`/`Unknown`) keeps
    /// the under-report direction. Runs for every literal, so the entry
    /// arms in [`Self::check_map_literal_entries`] stay assignability-only.
    pub(crate) fn check_collection_void_elements(
        &mut self,
        left: Token,
        right: Token,
        element: Option<Ty>,
        key: Option<Ty>,
        value: Option<Ty>,
    ) {
        let bytes = self.source.as_bytes();
        for (s, e) in self.literal_leaves(left, right) {
            let s = trim_ws(bytes, s, e);
            // A spread operand (`...x`/`...?x`) is not a value element —
            // its iterable-compatibility face owns the operand.
            if bytes[s..].starts_with(b"...") {
                continue;
            }
            // A map entry checks its key and value independently (the
            // `?`-key/value promote non-null upstream, which a `void` leaf
            // never reaches — the void use fires on the operand first).
            let mut sides: Vec<(usize, usize, Option<Ty>)> = Vec::new();
            match top_level_colon(bytes, s, e) {
                // A conditional element's `? :` splits inside the
                // condition — the whole leaf stays one expression; only a
                // `?`-free entry is a key/value pair.
                Some(colon) if top_level_question(bytes, s, colon).is_none() => {
                    sides.push((s, colon, key));
                    sides.push((colon + 1, e, value));
                }
                _ => sides.push((s, e, element)),
            }
            for (ls, le, slot) in sides {
                // The slot's value is consumed only by a non-`void`
                // element type; a `void` slot (or none the name layer can
                // spell) discards it.
                if !matches!(slot, Some(ty) if ty != Ty::UNKNOWN && !self.is_void_spelling(ty)) {
                    continue;
                }
                let (ls, le) = (self.utf16_of(ls), self.utf16_of(le));
                let Some((ty, range)) = self.leaf_completion_row(ls, le) else {
                    continue;
                };
                // The leaf's `v()` invoke spells `Unknown` until
                // `operand_static_ty` resolves the callee's `void` return.
                let operand_row = self
                    .rows
                    .iter()
                    .rev()
                    .find(|row| row.range == range)
                    .cloned();
                let operand_ty = operand_row
                    .as_ref()
                    .map(|row| self.operand_static_ty(row))
                    .unwrap_or(ty);
                if operand_ty == Ty::Void {
                    let range = operand_row
                        .as_ref()
                        .map(|row| self.use_site_range(row))
                        .unwrap_or(range);
                    self.emit(SemanticCode::UseOfVoidResult, range, "", "");
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
                // `Either` forces nothing itself; `Neither` positively
                // kills both can-be bits — both leave the literal
                // unresolvable, so both are `_either` evidence.
                LeafSide::Either | LeafSide::Neither => either_evidence = true,
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

    /// The brace literal's resolved kind from its leaves
    /// (`typed_literal_resolver.dart` `_inferSetOrMapLiteralType`'s
    /// can-be/must-be combination): each leaf fills the can-be/must-be
    /// slots through [`Self::leaf_side`], the walk folds them
    /// (`canBeASet = canBeASet && element.canBeSet`,
    /// `mustBeASet = mustBeASet || element.mustBeSet`), and the tail
    /// decides — `canBeASet && mustBeASet` resolves a set, `canBeAMap &&
    /// mustBeAMap` a map, every other shape is the ambiguous literal the
    /// oracle leaves UNRESOLVED (its `typeOrThrow` is `dynamic`, so
    /// `visitSetOrMapLiteral` runs neither element verifier — the
    /// `None` answer suppresses the spread/expression faces instead of
    /// fabricating the parser's map default).
    fn leaves_resolve_kind(&self, left: Token, right: Token) -> Option<SpreadLiteralKind> {
        let mut can_set = true;
        let mut must_set = false;
        let mut can_map = true;
        let mut must_map = false;
        for (start, end) in self.literal_leaves(left, right) {
            match self.leaf_side(start, end) {
                // `elementType` fills the element slot alone: a decided
                // expression/set-side leaf never contributes the key/value
                // slots, so it poisons `canBeMap` too (`{8, ...aMap}` is
                // the ambiguous `_both`, not a map).
                LeafSide::Set => {
                    can_map = false;
                    must_set = true;
                }
                LeafSide::Map => {
                    can_set = false;
                    must_map = true;
                }
                LeafSide::Neither => {
                    can_set = false;
                    can_map = false;
                }
                // `Either` fills every can-be slot without forcing a
                // must-be bit; `Blind` may hide a side the oracle already
                // decided — both leave the fold untouched (the abstain
                // posture).
                LeafSide::Either | LeafSide::Blind => {}
            }
        }
        if can_set && must_set {
            Some(SpreadLiteralKind::Set)
        } else if can_map && must_map {
            Some(SpreadLiteralKind::Map)
        } else {
            None
        }
    }

    /// One leaf's can-be/must-be side (`_inferCollectionElementType`'s
    /// per-element type-information slots): an expression is a set side,
    /// an entry a map side; a spread's operand type decides — an
    /// `Iterable` operand fills the element slot (set side), a `Map`
    /// operand the key/value slots (map side), `dynamic`/`Never`/
    /// null-aware-`Null` fills all three (neither must-be), a decided
    /// non-collection fills none (poisons both can-be bits), and an
    /// operand the row model cannot see stays undecided in the abstain
    /// posture.
    fn leaf_side(&self, start: usize, end: usize) -> LeafSide {
        let bytes = self.source.as_bytes();
        let s = trim_ws(bytes, start, end);
        if bytes[s..].starts_with(b"...") {
            let null_aware = bytes.get(s + 3) == Some(&b'?');
            let s16 = self.utf16_of(s);
            let e16 = self.utf16_of(end);
            let after = trim_ws(bytes, s + 3 + usize::from(null_aware), end);
            // A collection-literal operand: the rows inside it are its
            // elements, never the operand — `row_in_span` would read an
            // inner element's type as the spread's. A list literal is an
            // iterable (set side); a brace literal holding an entry is a
            // map (the oracle infers it before the outer census); any
            // other brace literal's own side is hidden.
            match bytes.get(after) {
                Some(b'[') => return LeafSide::Set,
                Some(b'{') => {
                    let close = balanced_brace(bytes, after, end);
                    let has_entry = close.is_some_and(|close| {
                        literal_comma_segments(bytes, after + 1, close)
                            .into_iter()
                            .any(|(s, e)| {
                                let s = trim_trivia(bytes, s, e);
                                s < e
                                    && !bytes[s..].starts_with(b"...")
                                    && entry_colon(bytes, s, e).is_some()
                            })
                    });
                    return if has_entry {
                        LeafSide::Map
                    } else {
                        LeafSide::Blind
                    };
                }
                _ => {}
            }
            match self.spread_operand_ty(s16, e16) {
                // No row at all: the operand's type is invisible. An
                // unresolvable operand is the oracle's error-dynamic
                // shape (positive `_either` evidence); a resolved one
                // hides a real type whose side may already have decided
                // the literal — blind.
                None => {
                    if let Some(name) = leading_ident_name(self, after, end) {
                        if self.resolve_local(name).is_none()
                            && matches!(self.resolve_bare(name).0, BareInfo::Missing { .. })
                        {
                            return LeafSide::Either;
                        }
                    }
                    return LeafSide::Blind;
                }
                // `dynamic`/undecidable/`Never` fill all three slots —
                // can-be either side, must-be neither.
                Some(ty)
                    if ty == Ty::UNKNOWN || ty == Ty::Dynamic || self.is_never_ty(ty) =>
                {
                    // An untyped CONSTANT read still carries its evaluated
                    // value, and a constant's value kind is its static
                    // type's side: `const m = {1: 2}` (an untyped literal
                    // initializer the inferred-variable overlay never binds)
                    // spread into `const Set<int> y = {...m}` is a
                    // must-be-map leaf, so the literal resolves a MAP
                    // upstream before the `Set` context's fallback.
                    if ty == Ty::UNKNOWN
                        && let Some(name) = leading_ident_name(self, after, end)
                        && trim_ws(bytes, after + self.strings.resolve(name).len(), end) == end
                        && let IdentVerdict::Value(value) = self.ident(name)
                    {
                        match value {
                            ConstantValue::Map(_) => return LeafSide::Map,
                            ConstantValue::List(_) | ConstantValue::Set(_) => {
                                return LeafSide::Set;
                            }
                            _ => {}
                        }
                    }
                    return LeafSide::Either;
                }
                Some(ty) => {
                    // Subtype-of-`Null`: `...?` admits it (all three
                    // slots fill with `Never`); `...` leaves the
                    // element's info EMPTY — it poisons both can-be
                    // bits.
                    if self.assignable(ty, Ty::Null, false) == Some(true) {
                        return if null_aware {
                            LeafSide::Either
                        } else {
                            LeafSide::Neither
                        };
                    }
                    // A type-parameter head reads its bound, matching
                    // `asInstanceOf`'s bound resolution.
                    let ty = self.bound_face(ty);
                    let Ty::Named { name: head, .. } = ty else {
                        // A function/record/void operand fills no slot.
                        return LeafSide::Neither;
                    };
                    // A head the model cannot resolve (a callee-side
                    // type parameter, an unmodeled spelling) is
                    // undecidable — upstream's `typeOrThrow` reads it as
                    // `dynamic`, which fills every can-be slot.
                    if crate::library::resolve_container(
                        self.graph,
                        self.namespaces,
                        self.index,
                        self.lib,
                        self.file,
                        head,
                    )
                    .is_none()
                        && !self.collection_heads.contains(&head)
                    {
                        return LeafSide::Either;
                    }
                    // `asInstanceOf(Iterable)` first, `asInstanceOf(Map)`
                    // second — upstream's order.
                    if self.head_reaches_iterable(head) {
                        return LeafSide::Set;
                    }
                    if self.head_reaches_map(head) {
                        return LeafSide::Map;
                    }
                    return LeafSide::Neither;
                }
            }
        }
        // `_fromElement`: a map entry (its null-aware `?k: v` / `k: ?v`
        // forms included) is a map side; an expression — the null-aware
        // `?e` element and a conditional included — a set side.
        if entry_colon(bytes, s, end).is_some() {
            LeafSide::Map
        } else {
            LeafSide::Set
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

    /// The spread operand's static type over `[start, end)` (UTF-16): the
    /// leaf's completion row when it spells a type, else the last row in
    /// the span through `operand_static_ty` (an untyped bare read resolves
    /// its declared accessor type — `...list1` reads `List`). `None` when
    /// no row lives inside the operand span at all — the operand is
    /// invisible to the row model and every ladder that asks its type must
    /// abstain rather than fabricate. `Some(Ty::UNKNOWN)` when a row
    /// exists but resolves to nothing — the oracle's error-dynamic shape.
    fn spread_operand_ty(&self, start: u32, end: u32) -> Option<Ty> {
        let (ty, _) = self.leaf_completion_row(start, end)?;
        if ty != Ty::UNKNOWN {
            return Some(ty);
        }
        Some(
            self.rows
                .iter()
                .rev()
                .find(|row| row.range.offset >= start && row.range.end() <= end)
                .map(|row| self.operand_static_ty(row))
                .unwrap_or(Ty::UNKNOWN),
        )
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

    /// The spread-element verifier's ladder over a literal's `...`-led
    /// leaf segments (upstream `literal_element_verifier.dart`
    /// `_verifySpreadForListOrSet` :238-335 and `_verifySpreadForMap`
    /// :337-391). `kind` decides which ladder runs and which expected
    /// types the element-assignability asks read: `List`/`Set` for
    /// `asInstanceOf(Iterable)` → `not_iterable_spread` and
    /// `set_element_type_not_assignable` (the list twin is already covered
    /// by `check_list_literal_elements`'s own `...` arm), `Map` for
    /// `asInstanceOf(Map)` → `not_map_spread` and
    /// `map_key_type_not_assignable`/`map_value_type_not_assignable`.
    ///
    /// The operand's static type answers the ladder's asks: `Dynamic`
    /// folds silently (the strict-casts tier is not modeled here),
    /// `Never` is silent, `Null` fires `not_null_aware_null_spread` unless
    /// the `...?` null-aware form wrote it, and a non-`Iterable`/`Map`
    /// operand fires the iterability row. An `Iterable`/`Map` operand
    /// then asks each element side's assignability against the literal's
    /// declared or context-inferred type arguments.
    fn check_spread_elements(&mut self, left: Token, right: Token, kind: SpreadLiteralKind) {
        let bytes = self.source.as_bytes();
        for (s, e) in self.literal_leaves(left, right) {
            let s = trim_ws(bytes, s, e);
            if !bytes[s..].starts_with(b"...") {
                continue;
            }
            let null_aware = bytes.get(s + 3) == Some(&b'?');
            let operand_start = trim_ws(
                bytes,
                s + 3 + usize::from(null_aware),
                e,
            );
            if operand_start >= e {
                continue;
            }
            let (s16, e16) = (self.utf16_of(operand_start), self.utf16_of(e));
            let operand_range = SourceRange {
                offset: s16,
                length: e16 - s16,
            };
            let Some(operand_ty) = self.spread_operand_ty(s16, e16) else {
                continue;
            };
            // Dynamic folds silently (the strict-casts tier is not
            // modeled); Never is silent (upstream `isSubtypeOf(exprType,
            // Never)` → return). An undecidable operand (the oracle's
            // error-dynamic shape: an unresolved name, a receiver the row
            // model cannot spell) is silent in the same posture — upstream
            // reads `typeOrThrow` and gets `dynamic`, which folds the whole
            // ladder; firing on `UNKNOWN` fabricates rows the oracle never
            // emits.
            if operand_ty == Ty::UNKNOWN
                || operand_ty == Ty::Dynamic
                || self.is_never_ty(operand_ty)
            {
                continue;
            }
            // Subtype-of-`Null` (upstream `isSubtypeOf(expressionType,
            // nullNone)`): `Null` itself and a parameter bound to `Null`
            // (`X extends Null` — `param_bound` answers `Null`, whose own
            // subtype ask closes true). `...?` admits the null side
            // silently; `...` reports `not_null_aware_null_spread`.
            if self.assignable(operand_ty, Ty::Null, false) == Some(true) {
                if !null_aware {
                    self.emit(
                        SemanticCode::NotNullAwareNullSpread,
                        operand_range,
                        "",
                        "",
                    );
                }
                continue;
            }
            // A type-parameter head reads its captured bound (upstream's
            // `asInstanceOf` resolves through the bound's interface); the
            // nullability flag rides along. `asInstanceOf` itself ignores
            // the receiver's `?` — a `List<int>?` operand still answers
            // `Iterable<int>`, so nullability never decides this ladder.
            let operand_ty = self.bound_face(operand_ty);
            let Ty::Named {
                name: head, args, ..
            } = operand_ty
            else {
                // A function/record/void operand is never an Iterable or
                // Map — the iterability row fires.
                let code = match kind {
                    SpreadLiteralKind::Map => SemanticCode::NotMapSpread,
                    _ => SemanticCode::NotIterableSpread,
                };
                self.emit(code, operand_range, "", "");
                continue;
            };
            // `asInstanceOf` delegates through `FutureOr`'s union legs:
            // `FutureOr<List>` reaches `Iterable`/`Map` iff the argument's
            // head does. Unwrap the single argument and re-ask on its
            // head.
            let operand_heads = {
                let future_or = self.intern("FutureOr");
                if head == future_or
                    && let Some(&inner) = self.boundary.args.borrow().get(args).first()
                    && let Ty::Named {
                        name: inner_head, ..
                    } = inner
                {
                    vec![head, inner_head]
                } else {
                    vec![head]
                }
            };
            // A head that resolves to NO declaration — a callee-side type
            // parameter (`...a()` of `T a<T>()`, whose instantiation the
            // context decides upstream), an unmodeled spelling — is the
            // oracle's undecidable shape: the literal's `typeOrThrow` side
            // never fabricates a verdict, so the iterability asks abstain.
            let head_decidable = operand_heads.iter().any(|h| {
                self.collection_heads.contains(h)
                    || crate::library::resolve_container(
                        self.graph,
                        self.namespaces,
                        self.index,
                        self.lib,
                        self.file,
                        *h,
                    )
                    .is_some()
            });
            if !head_decidable {
                continue;
            }
            match kind {
                SpreadLiteralKind::List | SpreadLiteralKind::Set => {
                    // `asInstanceOf(Iterable)` asks whether the operand's
                    // head reaches `Iterable`/`List`/`Set` through its
                    // declared superchain.
                    if !operand_heads.iter().any(|h| self.head_reaches_iterable(*h)) {
                        self.emit(
                            SemanticCode::NotIterableSpread,
                            operand_range,
                            "",
                            "",
                        );
                        continue;
                    }
                    // The element type asks assignability: the iterable's
                    // first argument against the literal's declared or
                    // inferred element type. The list-side check already
                    // runs in `check_list_literal_elements` (its own
                    // `...` arm), so only the set side's code fires here.
                    if kind == SpreadLiteralKind::Set {
                        let element_ty = {
                            let pool = self.boundary.args.borrow();
                            pool.get(args).first().copied().unwrap_or(Ty::UNKNOWN)
                        };
                        if element_ty != Ty::UNKNOWN
                            && let Some(expected) =
                                self.literal_set_or_map_element(left, right)
                            && self.assignable(element_ty, expected, false) == Some(false)
                        {
                            let (a0, a1) =
                                (self.ty_display(element_ty), self.ty_display(expected));
                            self.emit(
                                SemanticCode::SetElementTypeNotAssignable,
                                operand_range,
                                &a0,
                                &a1,
                            );
                        }
                    }
                }
                SpreadLiteralKind::Map => {
                    // `asInstanceOf(Map)` asks whether the operand's head
                    // reaches `Map`.
                    if !operand_heads.iter().any(|h| self.head_reaches_map(*h)) {
                        self.emit(SemanticCode::NotMapSpread, operand_range, "", "");
                        continue;
                    }
                    // The key/value types ask assignability against the
                    // literal's declared or context-inferred type
                    // arguments (`_verifySpreadForMap`'s
                    // `mapKeyType`/`mapValueType` gates).
                    let map_args = self.boundary.pool().get(args).to_vec();
                    if let [k, v] = map_args.as_slice() {
                        let (expected_key, expected_value) =
                            self.literal_map_key_value(left, right);
                        if let Some(expected) = expected_key
                            && self.assignable(*k, expected, false) == Some(false)
                        {
                            let (a0, a1) = (self.ty_display(*k), self.ty_display(expected));
                            self.emit(
                                SemanticCode::MapKeyTypeNotAssignable,
                                operand_range,
                                &a0,
                                &a1,
                            );
                        }
                        if let Some(expected) = expected_value
                            && self.assignable(*v, expected, false) == Some(false)
                        {
                            let (a0, a1) = (self.ty_display(*v), self.ty_display(expected));
                            self.emit(
                                SemanticCode::MapValueTypeNotAssignable,
                                operand_range,
                                &a0,
                                &a1,
                            );
                        }
                    }
                }
            }
        }
    }

    /// Whether the operand's declared head reaches `Iterable`/`List`/`Set`
    /// through its superchain (`_verifySpreadForListOrSet`'s
    /// `asInstanceOf(Iterable)` ask).
    fn head_reaches_iterable(&self, head: StringId) -> bool {
        let iterable_heads = [
            self.collection_heads[3],
            self.collection_heads[0],
            self.collection_heads[1],
        ];
        if iterable_heads.contains(&head) {
            return true;
        }
        self.head_reaches_family(head, &iterable_heads)
    }

    /// Whether the operand's declared head reaches `Map` through its
    /// superchain (`_verifySpreadForMap`'s `asInstanceOf(Map)` ask).
    fn head_reaches_map(&self, head: StringId) -> bool {
        let map_head = self.collection_heads[2];
        if head == map_head {
            return true;
        }
        self.head_reaches_family(head, &[map_head])
    }

    /// The declared-superchain walk: `extends`/`with`/`implements` edges
    /// from `resolve_container`, BFS to any target head.
    fn head_reaches_family(&self, head: StringId, targets: &[StringId]) -> bool {
        let mut queue: Vec<StringId> = vec![head];
        let mut seen: FxHashSet<StringId> = FxHashSet::from_iter([head]);
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
                if targets.contains(&next) {
                    return true;
                }
                if seen.insert(next) {
                    queue.push(next);
                }
            }
        }
        false
    }

    /// The literal's inferred or declared element type for the
    /// set-element assignability ask: a typed `<E>{...}` reads `E` from
    /// its own written group; an untyped `{...}` in a `Set<E>` context
    /// reads `E` from `literal_context_ty`.
    fn literal_set_or_map_element(&mut self, left: Token, right: Token) -> Option<Ty> {
        let _ = right;
        if let Some((_, Ty::Named { args, .. })) = self.typed_set_or_map_literal_ty(left)
            && let Some(elem) = self.boundary.pool().get(args).first().copied()
        {
            return Some(elem);
        }
        if let Ty::Named { name, args, .. } = self.literal_context_ty(left)?
            && name == self.collection_heads[1]
            && let Some(elem) = self.boundary.pool().get(args).first().copied()
        {
            return Some(elem);
        }
        None
    }

    /// The literal's declared or context-inferred map key/value pair for
    /// the spread map-side assignability asks: a typed `<K, V>{...}` reads
    /// its own written arguments; an untyped `{...}` in a `Map<K, V>`
    /// context reads the context's arguments.
    fn literal_map_key_value(&mut self, left: Token, right: Token) -> (Option<Ty>, Option<Ty>) {
        let _ = right;
        if let Some((_, Ty::Named { args, .. })) = self.typed_set_or_map_literal_ty(left) {
            let args = self.boundary.pool().get(args).to_vec();
            if let [k, v] = args.as_slice() {
                return (Some(*k), Some(*v));
            }
        }
        if let Some(Ty::Named { name, args, .. }) = self.literal_context_ty(left)
            && name == self.collection_heads[2]
        {
            let args = self.boundary.pool().get(args).to_vec();
            if let [k, v] = args.as_slice() {
                return (Some(*k), Some(*v));
            }
        }
        (None, None)
    }

    /// The const spread-shape verifier (upstream `constant_verifier.dart`
    /// `_validateListOrSetSpread` :1401-1417 and `_validateMapSpread`
    /// :1561-1589): a `const` list/set literal's `...expr` operand must
    /// evaluate to a constant list or set, a `const` map literal's to a
    /// constant map. Elements whose evaluation fails already reported
    /// through the site-default element code (`non_constant_*_element`);
    /// a `Verdict::Value` of the wrong shape reports the const-spread
    /// code at the operand's span. `Constant`/`Unresolved` verdicts are
    /// silent (the oracle's unknown-value posture).
    ///
    /// When the staged `Node` list does not carry the operand (a
    /// map-resolved `SetOrMap` drops non-Entry elements), the operand's
    /// row-level static type answers the same shape ask: a `List`/`Set`/
    /// `Iterable` head or a provably non-`Map` head reports, a `Map` or
    /// unspellable head stays silent.
    fn check_const_spread_shapes(
        &mut self,
        left: Token,
        right: Token,
        elements: &[crate::const_eval::Node],
        map: bool,
    ) {
        let bytes = self.source.as_bytes();
        for (s, e) in self.literal_leaves(left, right) {
            let s = trim_ws(bytes, s, e);
            if !bytes[s..].starts_with(b"...") {
                continue;
            }
            let operand_start = trim_ws(
                bytes,
                s + 3 + usize::from(bytes.get(s + 3) == Some(&b'?')),
                e,
            );
            if operand_start >= e {
                continue;
            }
            let s16 = self.utf16_of(operand_start);
            // The `...` operand inside a staged `Expr::Spread` (or a bare
            // staged leaf for the pre-wrap shapes) — the probe keys on the
            // operand's own offset.
            let node = elements.iter().find_map(|n| match &n.expr {
                crate::const_eval::Expr::Spread { operand, .. }
                    if operand.span.offset == s16 =>
                {
                    Some(operand.as_ref())
                }
                _ if n.span.offset == s16 => Some(n),
                _ => None,
            });
            match node {
                Some(node) => {
                    let verdict = eval(node, self);
                    // `...?` admits a `null` constant silently (upstream
                    // `spread.isNull && element.isNullAware` → `continue`).
                    let null_aware = bytes.get(s + 3) == Some(&b'?');
                    let code = match (&verdict, map) {
                        (
                            Verdict::Value(
                                ConstantValue::List(_) | ConstantValue::Set(_),
                            ),
                            false,
                        ) => continue,
                        (Verdict::Value(ConstantValue::Map(_)), true) => continue,
                        (Verdict::Value(ConstantValue::Null), _) if null_aware => {
                            continue;
                        }
                        (Verdict::Value(_), false) => {
                            SemanticCode::ConstSpreadExpectedListOrSet
                        }
                        (Verdict::Value(_), true) => SemanticCode::ConstSpreadExpectedMap,
                        _ => continue,
                    };
                    self.emit(code, node.span, "", "");
                }
                None => {
                    // The staged nodes do not carry this operand (a
                    // map-resolved `SetOrMap` drops non-Entry elements):
                    // the row's static type answers the shape ask. An
                    // undecidable operand (the oracle's error-dynamic
                    // shape) abstains rather than fabricate a shape
                    // failure — upstream reads `typeOrThrow`'s `dynamic`
                    // and stays silent.
                    let e16 = self.utf16_of(e);
                    let Some((ty, range)) = self
                        .rows
                        .iter()
                        .rev()
                        .find(|row| {
                            row.range.offset >= s16
                                && row.range.end() <= e16
                                && !matches!(row.shape, RowShape::TypeUse(..))
                        })
                        .map(|row| (self.bound_face(self.operand_static_ty(row)), row.range))
                    else {
                        continue;
                    };
                    if matches!(ty, Ty::UNKNOWN | Ty::Dynamic | Ty::Never)
                        || self.is_never_ty(ty)
                    {
                        continue;
                    }
                    // A `Null`-typed operand: `...?` admits `null` silently
                    // (upstream `spread.isNull && element.isNullAware` →
                    // `continue`); `...` evaluates to no list/set/map —
                    // the const shape code still fires.
                    if self.assignable(ty, Ty::Null, false) == Some(true)
                        && bytes.get(s + 3) == Some(&b'?')
                    {
                        continue;
                    }
                    let code = if map {
                        // A map literal's spread operand must be a Map.
                        if matches!(ty, Ty::Named { name, .. } if name == self.collection_heads[2])
                        {
                            continue;
                        }
                        SemanticCode::ConstSpreadExpectedMap
                    } else {
                        // A list/set literal's spread operand must be a
                        // List/Set/Iterable.
                        let iterable_heads = [
                            self.collection_heads[3],
                            self.collection_heads[0],
                            self.collection_heads[1],
                        ];
                        if matches!(ty, Ty::Named { name, .. } if iterable_heads.contains(&name)) {
                            continue;
                        }
                        SemanticCode::ConstSpreadExpectedListOrSet
                    };
                    self.emit(code, range, "", "");
                }
            }
        }
    }

    /// The `expression_in_map` face (upstream `literal_element_verifier.dart`
    /// `_verifyElement`'s map arm :100/:135 — `ExpressionImpl` and
    /// `NullAwareElement` in a map literal): a non-entry, non-spread,
    /// non-control-flow leaf in a map-resolved literal reports. A
    /// `key: value` entry, a `...`/`...?` spread, an `if`/`for` control
    /// element, and a `?`-led null-aware element are all silent (their
    /// own faces own them). The anchor is the leaf's own span.
    fn check_expression_in_map(&mut self, left: Token, right: Token) {
        let bytes = self.source.as_bytes();
        for (s, e) in self.literal_leaves(left, right) {
            let s = trim_ws(bytes, s, e);
            if s >= e {
                continue;
            }
            // A `...`/`...?` spread, an `if`/`for` control element, and a
            // `?`-led null-aware element are not expressions — their own
            // faces own them.
            if bytes[s..].starts_with(b"...")
                || bytes[s] == b'?'
                || bytes[s..].starts_with(b"if")
                || bytes[s..].starts_with(b"for")
            {
                continue;
            }
            // A map entry (`key: value`) is not an expression.
            if top_level_colon(bytes, s, e).is_some() {
                continue;
            }
            let (s16, e16) = (self.utf16_of(s), self.utf16_of(e));
            self.emit_message_with_correction(
                SemanticCode::ExpressionInMap,
                SourceRange {
                    offset: s16,
                    length: e16 - s16,
                },
                SemanticCode::ExpressionInMap.message("", ""),
            );
        }
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
        // Pull the owned return type out of the frame borrow — `emit` needs
        // `&mut self`, so the `&frame`/`&ret` borrow cannot outlive this.
        let ret_ty = ret.ty;
        let Some((expr_ty, range)) =
            self.single_value_row_between(begin.range.end(), end.range.offset)
        else {
            return;
        };
        // The void-operand use (upstream `yield_statement_resolver.dart`
        // `_checkForUseOfVoidResult(node.expression)` at :181/:198 — both
        // the `yield` and `yield*` arms): `yield x` on a `void`-typed `x`
        // reports at the yielded expression. The row's raw `ty` spells an
        // invoke's return `Unknown` — `operand_static_ty` resolves the
        // callee's `void` face.
        let operand_row = self
            .rows
            .iter()
            .rev()
            .find(|row| row.range == range)
            .cloned();
        let operand_ty = operand_row
            .as_ref()
            .map(|row| self.operand_static_ty(row))
            .unwrap_or(expr_ty);
        if operand_ty == Ty::Void {
            let range = operand_row
                .as_ref()
                .map(|row| self.use_site_range(row))
                .unwrap_or(range);
            self.emit(SemanticCode::UseOfVoidResult, range, "", "");
        }
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
            if self.assignable(expr_ty, ret_ty, false) == Some(false) {
                // Staged, not emitted: the oracle's verdict reads the
                // source's flow-aware static type
                // (`yield_statement_resolver.dart` `expression.typeOrThrow`
                // — probe2484: a null-guard-promoted `yield* src` stays
                // silent), so the promoted read re-decides at the teardown
                // join with the promoted type as the implied return.
                self.stage(StagedKind::YieldEach, range, expr_ty, ret_ty, false);
            }
            return;
        }
        let Ty::Named { name, args, .. } = ret_ty else {
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

    /// The selector span of a member access: the token right after the
    /// access operator (`x.m` → `m`, `x?.m` → `m`, the recovered `x.{` →
    /// `{`). The oracle anchors member reports on it (`.at(propertyName)`).
    fn access_selector_span(&self, operator: Token) -> Option<SourceRange> {
        let member = self.tokens.get(operator.id as usize + 1)?;
        let raw = &self.source[member.byte_range()];
        let length: u32 = raw.chars().map(char::len_utf16).sum::<usize>() as u32;
        Some(SourceRange {
            offset: member.utf16_offset,
            length,
        })
    }

    /// Whether a call's `(` opens at `end` (whitespace skipped): the access
    /// is a member send rather than a read/write.
    fn call_opens_at(&self, end: u32) -> bool {
        self.byte_at(self.skip_whitespace(end)) == Some(b'(')
    }

    /// The receiver row of an access whose operator sits at `at` — the
    /// closest row wholly before it (the row `merge_member` is about to
    /// fold). `None` for a recovery shape with no operand row.
    fn access_receiver_index(&self, at: u32) -> Option<usize> {
        self.rows
            .iter()
            .rposition(|row| row.range.end() <= at)
    }

    /// Whether a `use_of_void_result` row already sits at `range`. The
    /// operand-consumption faces fold the same node from more than one seam
    /// (a compound operator's read target, a member send's target and its
    /// receiver), and the oracle answers one row per consumed value.
    fn void_result_reported_at(&self, range: SourceRange) -> bool {
        self.report
            .diagnostics
            .iter()
            .any(|d| d.range == range && d.code.name == "use_of_void_result")
    }

    /// The `name: value` span of a record literal's named field — the
    /// `RecordLiteralNamedField` node the oracle anchors
    /// `use_of_void_result` on (upstream Dart SDK source, record_literal_
    /// resolver.dart:159 `.at(field)`), never the value alone. `None` for a
    /// positional field (no label adjoins its value).
    fn named_field_span(&self, value: SourceRange) -> Option<SourceRange> {
        let tokens = self.tokens.as_slice();
        let colon = tokens
            .partition_point(|t| t.utf16_offset < value.offset)
            .checked_sub(1)?;
        if !matches!(tokens.get(colon)?.kind, dartq_scanner::TokenKind::Colon) {
            return None;
        }
        let name = tokens.get(colon.checked_sub(1)?)?;
        Some(SourceRange {
            offset: name.utf16_offset,
            length: value.end() - name.utf16_offset,
        })
    }

    /// The `void`-receiver face of a member access (#3249): a `void`
    /// receiver's value is consumed by the access, so the oracle reports
    /// `use_of_void_result` there — at the member's own name for a read or
    /// a write (upstream Dart SDK source, pkg/analyzer/lib/src/dart/
    /// resolver/property_element_resolver.dart `_resolve`'s
    /// `targetType is VoidType` arm) and at the receiver node for a member
    /// *send* (error_verifier.dart `visitAnonymousMethodInvocation`'s
    /// `checkForUseOfVoidResult(node.target2)`). Any other receiver type is
    /// a no-op.
    fn emit_void_member_receiver(&mut self, receiver: usize, selector: SourceRange) {
        let row = &self.rows[receiver];
        let ty = Engine::receiver_row_ty(row);
        let ty = if ty == Ty::UNKNOWN {
            self.receiver_static_ty(row)
        } else {
            ty
        };
        if ty != Ty::Void {
            return;
        }
        let anchor = if self.call_opens_at(selector.end()) {
            // The send's receiver is a whole expression node (upstream
            // `method_invocation_resolver.dart` `_reportUseOfVoidType` —
            // `useOfVoidResult.at(receiver)`): an invoked receiver spans
            // the completed send through its `)` (`v()` of `v().m()`, `c.m()`
            // of `c.m().n()` — the callee row itself keeps its name-only
            // range), a member chain widens to the whole access (`b.g` of
            // `b.g.m()`), and a bare read keeps its span (`a` of `a.m()`).
            let range = self.rows[receiver].range;
            self.invocation_span(range)
                .or_else(|| self.property_access_span(range))
                .unwrap_or(range)
        } else {
            selector
        };
        if !self.void_result_reported_at(anchor) {
            self.emit(SemanticCode::UseOfVoidResult, anchor, "", "");
        }
    }
}

/// The literal kind a spread-element verification runs against.
#[derive(Clone, Copy, PartialEq, Eq)]
enum SpreadLiteralKind {
    List,
    Set,
    Map,
}

/// A leaf element's must-be side for the ambiguity decision.
#[derive(Clone, Copy, PartialEq, Eq)]
enum LeafSide {
    Map,
    Set,
    /// No side forced, but the leaf is positively undecided in the
    /// oracle too (a `dynamic`/`Never` spread row, a null-aware `Null`
    /// operand, an unresolvable operand's error-dynamic): fills every
    /// can-be slot, forces neither must-be — `_either` evidence.
    Either,
    /// The leaf's type is invisible to the row model (no row at the
    /// operand): its hidden real side may already have decided the
    /// literal, so it forces nothing and vetoes the `_either` tail.
    Blind,
    /// A decided leaf that fills NO slot (a resolved non-collection
    /// spread operand — `int`, a function type, `void`, a non-aware
    /// `Null`): poisons both can-be bits, so the literal cannot resolve
    /// — `_either` evidence through the ambiguous tail.
    Neither,
}

/// The operand expression's head offset for the is-verdict span. The merged
/// operand row anchors at its own member/index token (a `receiver.member`
/// merge keeps the member name's offset, an index merge the `]`), while the
/// oracle's `.at(node)` anchors at the operand expression's head
/// (`IsExpressionImpl.offset`: `c1.a`, `super.remove`, `list[0]`,
/// `(-0.0).ceil()`, `new Foo()`). The head sits in the source bytes — the
/// same walk the binary-expression fold takes (dotted chains), extended
/// with the bracket closer's opener (plus the receiver identifier before
/// it) and the whole-word `new`/`const` instance-creation keyword.
/// Unrecognized bytes stop the walk; a bare head returns `from` unchanged.
fn operand_expr_head(engine: &Engine<'_>, from: u32) -> u32 {
    let is_ident_byte = |b: u8| b.is_ascii_alphanumeric() || b == b'_' || b == b'$';
    let is_ws = |b: u8| matches!(b, b' ' | b'\t' | b'\r' | b'\n');
    let mut head = from;
    loop {
        // The instance-creation keyword: a whole-word `new`/`const`
        // separated from the current head by whitespace (`new C()`,
        // `new C.named()`, `const C()` — a tear-off's `.new` included: the
        // dot arm below takes the receiver next).
        let mut gap = head;
        while gap > 0 && engine.byte_at(gap - 1).is_some_and(is_ws) {
            gap -= 1;
        }
        let keywords: [&[u8]; 2] = [b"new", b"const"];
        let keyword = keywords.into_iter().find(|kw| {
            gap >= kw.len() as u32
                && (gap == kw.len() as u32
                    || !engine
                        .byte_at(gap - kw.len() as u32 - 1)
                        .is_some_and(is_ident_byte))
                && kw
                    .iter()
                    .enumerate()
                    .all(|(i, b)| engine.byte_at(gap - kw.len() as u32 + i as u32) == Some(*b))
        });
        if let Some(kw) = keyword {
            head = gap - kw.len() as u32;
            continue;
        }
        // The bracketed group: a `)`/`]` at (or, after whitespace, before)
        // the head — an index merge's own anchor, or a call/paren receiver's
        // closer the dot walk stopped at — jumps to its opener, and the
        // receiver identifier before the opener joins (`list[0]`,
        // `(-0.5).ceil()`, `(C<int>).toString()`).
        let closer = if matches!(engine.byte_at(head), Some(b')' | b']')) {
            Some(head)
        } else {
            let mut gap = head;
            while gap > 0 && engine.byte_at(gap - 1).is_some_and(is_ws) {
                gap -= 1;
            }
            (gap > 0 && matches!(engine.byte_at(gap - 1), Some(b')' | b']'))).then_some(gap - 1)
        };
        if let Some(closer) = closer {
            let (open, close) = match engine.byte_at(closer) {
                Some(b']') => (b'[', b']'),
                _ => (b'(', b')'),
            };
            let mut depth = 0u32;
            let mut scan = closer;
            let opener = loop {
                if scan == 0 {
                    break None;
                }
                scan -= 1;
                match engine.byte_at(scan) {
                    Some(b) if b == close => depth += 1,
                    Some(b) if b == open => {
                        if depth == 0 {
                            break Some(scan);
                        }
                        depth -= 1;
                    }
                    _ => {}
                }
            };
            if let Some(opener) = opener {
                head = opener;
                while head > 0 && engine.byte_at(head - 1).is_some_and(is_ident_byte) {
                    head -= 1;
                }
                continue;
            }
            break;
        }
        // The dotted chain: `.member` / `?.member` — the identifier before
        // the dot is the next head (`c1.a`, `super.remove`, `d.caa_.b_`); a
        // non-identifier before the dot (a group's `)`) defers to the
        // bracket arm on the next lap.
        if head == 0 || engine.byte_at(head - 1) != Some(b'.') {
            break;
        }
        head = if head >= 2 && engine.byte_at(head - 2) == Some(b'?') {
            head - 2
        } else {
            head - 1
        };
        while head > 0 && engine.byte_at(head - 1).is_some_and(is_ident_byte) {
            head -= 1;
        }
        // The dotted chain's receiver may carry a type-argument group
        // (`C<int>.constr`, `prefix.C<int>.new`): the ident walk stopped
        // at the group's `>`. Walk the balanced `<>` back to its `<`,
        // then over the head ident — the same walk the `)`/`]` bracket
        // arm applies to call/index receivers.
        if head > 0 && engine.byte_at(head - 1) == Some(b'>') {
            let mut depth = 0u32;
            let mut scan = head - 1;
            let opener = loop {
                if scan == 0 {
                    break None;
                }
                scan -= 1;
                match engine.byte_at(scan) {
                    Some(b'>') => depth += 1,
                    Some(b'<') => {
                        if depth == 0 {
                            break Some(scan);
                        }
                        depth -= 1;
                    }
                    _ => {}
                }
            };
            if let Some(opener) = opener {
                head = opener;
                while head > 0 && engine.byte_at(head - 1).is_some_and(is_ident_byte) {
                    head -= 1;
                }
            }
        }
    }
    head
}

/// The `unnecessary_type_check` verdict's span pins: the oracle's
/// `_checkAllTypeChecks` reports `.at(node)` on the whole `IsExpression`
/// (best_practices_verifier.dart:886-890) — the operand expression's head
/// through the written type's end — so every operand shape anchors at its
/// own head, not at the merged row's member/index token.
#[cfg(test)]
mod type_check_span_pins {
    use crate::diagnose::test_rig::{
        CORE_PRELUDE, RigImport, report_ops, report_ops_imports, rows,
    };

    /// One `unnecessary_type_check` spanning exactly `text` inside
    /// `source` — the (offset, length) pair pinned by the text itself:
    /// the row's offset..end must slice out `text` verbatim and carry
    /// the pinned `length` (the oracle's whole-IsExpression span, per
    /// operand shape). The report's ranges are absolute into the mounted
    /// unit (`CORE_PRELUDE + source`), so the slice runs over the same
    /// buffer the engine saw.
    fn pin(source: &str, text: &str, length: u32) {
        pin_report(
            report_ops(source),
            &format!("{CORE_PRELUDE}{source}"),
            text,
            length,
        );
    }

    /// [`pin`] with sibling libraries mounted — the fixture names the
    /// dep's declarations through `prefix` without an in-source
    /// `import` directive (a directive after `CORE_PRELUDE`'s
    /// declarations would mis-parse before the pin's face runs).
    fn pin_imports(source: &str, imports: &[RigImport<'_>], text: &str, length: u32) {
        pin_report(
            report_ops_imports(source, imports),
            &format!("{CORE_PRELUDE}{source}"),
            text,
            length,
        );
    }

    fn pin_report(report: crate::diagnose::FileReport, full: &str, text: &str, length: u32) {
        let utc: Vec<_> = report
            .diagnostics
            .iter()
            .filter(|d| d.code.name == "unnecessary_type_check")
            .collect();
        assert_eq!(
            utc.len(),
            1,
            "exactly one row expected: {:#?}",
            rows(&report)
        );
        let d = utc[0];
        assert_eq!(
            full.get(d.range.offset as usize..d.range.end() as usize),
            Some(text),
            "row at ({}, {}) spans the wrong text: {:#?}",
            d.range.offset,
            d.range.length,
            rows(&report)
        );
        assert_eq!(
            d.range.length,
            length,
            "pinned length: {:#?}",
            rows(&report)
        );
    }

    /// Instance-creation operand (`new Foo() is Foo` — cycle_test_none):
    /// the span starts at `new`, not at the constructor name row.
    #[test]
    fn instance_creation_operand_spans_the_keyword() {
        pin(
            "class C {}\nvoid f() {\n  if (new C() is C) {}\n}\n",
            "new C() is C",
            12,
        );
    }

    /// `const` twin of the keyword face.
    #[test]
    fn const_instance_creation_operand_spans_the_keyword() {
        pin(
            "class C {}\nvoid f() {\n  if (const C() is C) {}\n}\n",
            "const C() is C",
            14,
        );
    }

    /// Member-chain operand (`c1.a is A` — CastOperator_A01_t01,
    /// `d.caa_.b_ is B<AA>` — generic_test): the span starts at the
    /// receiver, not at the merged member-name row.
    #[test]
    fn member_chain_operand_spans_the_receiver() {
        pin(
            "class A { int i = 0; }\nvoid f(A a) {\n  if (a.i is int) {}\n}\n",
            "a.i is int",
            10,
        );
    }

    /// Index operand (`list[0] is S` — closure_test): the index merge
    /// anchors at the `]`; the span starts at the receiver identifier.
    #[test]
    fn index_operand_spans_the_receiver() {
        pin(
            "void f(List<int> l) {\n  if (l[0] is int) {}\n}\n",
            "l[0] is int",
            11,
        );
    }

    /// Parenthesized receiver over a call (`(-0.0).ceil() is int` —
    /// arithmetic_test): the dot walk stops at the group's `)`; the span
    /// starts at the opener. The member call names a fixture-declared
    /// element — the prelude's names-only `double` stub carries no
    /// `ceil`, so a core stub member would pin `undefined_method`, not
    /// the span.
    #[test]
    fn parenthesized_call_operand_spans_the_group() {
        pin(
            "class D { int ceil() => 0; }\nvoid f(D d) {\n  if ((d).ceil() is int) {}\n}\n",
            "(d).ceil() is int",
            17,
        );
    }

    /// Prefixed target (`r1 is lib.T1` — Records/static_semantics_A02):
    /// `handle_type` anchors the qualified head at the prefix token, but
    /// the TypeAnnotation spans through the continuation — the verdict
    /// ends at `T1`, not at `lib`. The prefix binds through the imports
    /// rig — an in-source `import` after `CORE_PRELUDE`'s declarations
    /// would only pin a `directive_after_declaration`, never the span.
    /// The `is!` polarity keeps the operand unprefixed (the same type
    /// reaches it through the plain edge): a `p.T`-typed operand arms
    /// the prefix head, which the verdict's Prefix gate silences before
    /// the span question is ever asked.
    #[test]
    fn prefixed_target_ends_at_the_continuation() {
        pin_imports(
            "void f(T l) {\n  if (l is! p.T) {}\n}\n",
            &[
                RigImport {
                    uri: "package:t/p.dart",
                    prefix: Some("p"),
                    source: "class T {}\n",
                },
                RigImport {
                    uri: "package:t/p.dart",
                    prefix: None,
                    source: "class T {}\n",
                },
            ],
            "l is! p.T",
            9,
        );
    }

    /// The bare head control: the walk must leave a plain identifier's
    /// span untouched (the landed family's anchor).
    #[test]
    fn bare_operand_head_is_unchanged() {
        pin("void f(int x) {\n  if (x is int) {}\n}\n", "x is int", 8);
    }
}

/// The #2937 pins (the index-expression argument check, the annotation
/// argument-assignability face, and the old-style function-typed
/// formal's callee signature). Kept as a standalone `#[path]` submodule
/// so the in-file test modules stay untouched by the registration.
#[cfg(test)]
#[path = "ticket_2937_tests.rs"]
mod ticket_2937_tests;

/// The #2933 pins (the nameless-declaration registration: local variable,
/// field, method, local function, and catch parameter join as real
/// elements under the empty name — `name ?? '<unnamed>'`,
/// element.dart:2093 — and the unused-family gates map the null name
/// private, element.dart:2179).
#[cfg(test)]
#[path = "ticket_2933_tests.rs"]
mod ticket_2933_tests;

/// The #3089 pins (a positional object-pattern field reads the getter its
/// sub-pattern's variable names — `resolveObjectPatternPropertyGet`'s
/// `field.pattern.variablePattern?.name` fallback, type_analyzer.dart:1675-1676
/// — so `Circle(var c as num)` reports `undefined_getter` at `c`).
#[cfg(test)]
#[path = "ticket_3089_tests.rs"]
mod ticket_3089_tests;

/// The #3085 pins (the var-window's index-result arm binds the read's own
/// static type at any nullability — a `Map<K, V>` read binds `V?` and the
/// downstream write/guard promotions decide on the real face,
/// reference.dart:680 / evaluation.dart:3335).
#[cfg(test)]
#[path = "ticket_3085_tests.rs"]
mod ticket_3085_tests;

/// The #3087 pins (a null-aware access over a receiver whose bound spells
/// an invalid type classifies null-or-equivalent — the `?.` right side and
/// a `!= null` then-block are dead, subtype.dart:274 / classifyType).
#[cfg(test)]
#[path = "ticket_3087_tests.rs"]
mod ticket_3087_tests;

/// The #3172 Lane D pins (the null-aware dead chain over a definitely
/// unassigned local — `_verifyUnassignedSimpleIdentifier`,
/// dead_code_verifier.dart:467-505: the `?.`/`?[`/`?..` operator anchors,
/// the parent climb's outer-access end, and the `unParenthesized`
/// receiver's inner-local gate).
#[cfg(test)]
#[path = "ticket_3172_tests.rs"]
mod ticket_3172_tests;

/// The #3165 pins (the rest-element relational sub-pattern's `List<E>`
/// matched type, the `>>>`/`>>>=` re-glue under `no-triple-shift`, and the
/// object-pattern shorthand binder's receiver-instantiated getter return).
#[cfg(test)]
#[path = "ticket_3165_tests.rs"]
mod ticket_3165_tests;

/// The #3179 pins (the `on`-head shapes that resolve to a null/record
/// `exceptionType` upstream — a record type, a named-record recovery, and
/// the bare `on (…)` parse — joining the try's clause list as the catch-all
/// face, plus the `on Object?` subtype arm;
/// dead_code_verifier.dart:552-608).
#[cfg(test)]
#[path = "ticket_3179_tests.rs"]
mod ticket_3179_tests;

/// The #3176 `use_of_void_result` pins — the assignability-driven operand
/// seams folded onto the #3164 set (assignment right operand, declaration
/// initializer, generic argument, for-in iterable, switch scrutinee, `??`
/// left, `is` operand, interpolation, `await`, `throw`, `yield`, non-`!`
/// unary, collection/map-entry elements, anonymous-invocation callee;
/// error_detection_helpers.dart:245-258).
#[cfg(test)]
#[path = "ticket_3176_tests.rs"]
mod ticket_3176_tests;
