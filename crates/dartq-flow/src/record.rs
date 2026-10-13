//! The recording half of the engine: [`Flow`] implements
//! [`ParserListener`], buffers the flow-relevant events of each executable
//! (method, top-level function, constructor, factory, local function,
//! closure), and runs the replay pipeline when the executable ends.

use dartq_parser::{
    AssertKind, BlockKind, DeclarationKind, ExperimentalFlag, FormalParameterKind,
    IdentifierContext, MemberKind, Message, ParserListener,
};
// The listener trait spells its token spans `Token` (the parser's own
// alias of `TokenSpan`).
use dartq_parser::TokenSpan as Token;
use dartq_parser::TokenSpan;
use dartq_scanner::TokenTable;

use rustc_hash::FxHashMap;

use crate::ev::{BinOp, Ev};
use crate::facts::FlowFact;
use crate::interp::Interp;
use crate::ty::NameTypeOps;

/// The flow-analysis engine over one compilation unit. Construct one per
/// parse; [`Flow::take_facts`] collects every fact after the parse.
///
/// `'t` borrows the token table and source so operator tokens classify and
/// identifier names intern at record time (parser-inserted synthetic
/// tokens classify conservatively and get unique synthetic names).
pub struct Flow<'t> {
    tokens: &'t TokenTable,
    source: &'t str,
    /// Lexeme to interned name id (dense `u32` for the interpreter).
    names: FxHashMap<Box<str>, u32>,
    /// Interned name ids to their lexemes — the mirror of `names` in id
    /// order (dense, one push per intern), so a later stage can recover
    /// `Base::Name(id)` spellings through [`Flow::name_lexeme`].
    names_by_id: Vec<Box<str>>,
    /// Open type-argument groups (nested annotations stack). Each group
    /// collects the argument rows recorded inside it; the parser fires
    /// `begin_type_arguments` at `<` and `end_type_arguments` immediately
    /// before the head's own row (`type_info_impl.dart`), so the group's
    /// entries are complete when it closes.
    type_arg_groups: Vec<Vec<crate::ty::ArgTy>>,
    /// Interned written-argument lists; index 0 is the reserved
    /// argument-less list ([`crate::ty::EMPTY_ARGS`]). Deduplicated
    /// structurally, so equal spellings share one entry and the promotion
    /// gate's id comparison is tree equality (#611 step 4).
    pub(crate) type_args: Vec<Vec<crate::ty::ArgTy>>,
    /// The interned argument list of the type row that fires next:
    /// `end_type_arguments` sets it, `handle_type` consumes it,
    /// `handle_no_type_arguments` clears it — all three fire immediately
    /// before the row (official `parseArguments`,
    /// `type_info_impl.dart:1158/1191`).
    pending_type_args: u32,
    /// The most recent label-reference identifier (a break/continue
    /// target's name, consumed by the next break/continue event).
    pending_label_ref: Option<u32>,
    /// Label-declaration names since the last consuming event (a labeled
    /// statement begin, or a switch case begin — case labels are counted
    /// on `CaseBegin` and never attach to a labeled statement).
    pending_labels: Vec<u32>,
    /// The FIRST label-declaration token since the last consuming event.
    /// `beginLabeledStatement` carries the statement past the labels
    /// (official fasta shape), but the official dead row anchors at the
    /// labeled statement's `firstToken` — the first label
    /// (`dead_code_verifier.dart:253`) — so the begin latches this.
    pending_label_token: Option<Token>,
    /// Recording buffer of the outermost active executable, if any.
    buffer: Option<Vec<Ev>>,
    /// Open `Ev::Block` event indices, for end-of-block span widening.
    block_events: Vec<usize>,
    /// The do-while body's last token: distinguishes the block-body
    /// `while`-part start (the closing brace) from the keyword start.
    do_body_end: Option<Token>,
    /// The open do-while body is a block (its `{` opened the body).
    do_body_is_block: bool,
    /// Executable nesting depth inside the active recording.
    exec_depth: usize,
    /// The then-arm boundary latched by
    /// [`Flow::end_conditional_expression_then_arm`], consumed by the
    /// argless colon call that follows it.
    cond_then: Option<(TokenSpan, u32)>,
    facts: Vec<FlowFact>,
}

impl<'t> Flow<'t> {
    #[must_use]
    pub fn new(tokens: &'t TokenTable, source: &'t str) -> Self {
        Self {
            tokens,
            source,
            names: FxHashMap::default(),
            names_by_id: Vec::new(),
            type_arg_groups: Vec::new(),
            type_args: vec![Vec::new()],
            pending_type_args: crate::ty::EMPTY_ARGS,
            pending_label_ref: None,
            pending_labels: Vec::new(),
            pending_label_token: None,
            buffer: None,
            block_events: Vec::new(),
            do_body_end: None,
            do_body_is_block: false,
            exec_depth: 0,
            cond_then: None,
            facts: Vec::new(),
        }
    }

    /// The span of the token at table `index` (the recorder's token-table
    /// navigation for arm boundaries the listener surface carries only by
    /// adjacency).
    #[expect(
        clippy::cast_possible_truncation,
        reason = "SourceRange is u32 by the scanner's own offset/length \
                  convention; source files fit that width"
    )]
    fn span_at(&self, index: usize) -> Option<TokenSpan> {
        self.tokens.get(index).map(|token| {
            let range = token.byte_range();
            TokenSpan {
                id: index as u32,
                range: dartq_source::SourceRange {
                    offset: range.start as u32,
                    length: (range.end - range.start) as u32,
                },
            }
        })
    }

    /// Interns a written-argument list structurally: equal trees share one
    /// arena entry, so the interpreter's id equality is tree equality.
    /// Lists are tiny (the unit's distinct generic spellings) and
    /// argument-less types never reach here, so the linear scan is bounded
    /// by unit size, not corpus size.
    fn intern_args(&mut self, entries: Vec<crate::ty::ArgTy>) -> u32 {
        if entries.is_empty() {
            return crate::ty::EMPTY_ARGS;
        }
        if let Some(index) = self
            .type_args
            .iter()
            .position(|existing| *existing == entries)
        {
            #[expect(
                clippy::cast_possible_truncation,
                reason = "arena index; a unit with 2^32 distinct generic \
                          spellings cannot parse"
            )]
            return index as u32;
        }
        #[expect(
            clippy::cast_possible_truncation,
            reason = "arena index; a unit with 2^32 distinct generic \
                      spellings cannot parse"
        )]
        let id = self.type_args.len() as u32;
        self.type_args.push(entries);
        id
    }

    /// Interns a name token's lexeme to a dense id. Synthetic tokens
    /// (parser-inserted) get a unique id keyed on the token handle.
    fn name(&mut self, token: TokenSpan) -> u32 {
        let index = token.id as usize;
        // The lookup borrows the lexeme: identifiers are the hottest flow
        // event and a repeat occurrence (the common case) must not
        // allocate; the owning key is built only on first sight.
        if self.tokens.get(index).is_some() {
            let lexeme = self.tokens.lexeme(self.source, index);
            if let Some(&id) = self.names.get(lexeme) {
                return id;
            }
            let id = u32::try_from(self.names.len()).expect("interned name count fits u32");
            let key: Box<str> = lexeme.into();
            self.names_by_id.push(key.clone());
            self.names.insert(key, id);
            return id;
        }
        // Parser-inserted synthetic tokens key on the token handle.
        let key: Box<str> = format!("#{}", token.id).into();
        if let Some(&id) = self.names.get(&key) {
            return id;
        }
        let id = u32::try_from(self.names.len()).expect("interned name count fits u32");
        self.names_by_id.push(key.clone());
        self.names.insert(key, id);
        id
    }

    /// The lexeme of an interned name id — the recovery face turning
    /// `Base::Name(id)` (and event name ids) back into their source
    /// spelling. Ids are dense by construction; synthetic tokens read as
    /// their `#<handle>` placeholder.
    #[must_use]
    pub fn name_lexeme(&self, id: u32) -> &str {
        let index = id as usize;
        &self.names_by_id[index]
    }

    /// Takes the pending label reference if one is set (break/continue
    /// with a target).
    fn take_label(&mut self, has_target: bool) -> Option<u32> {
        if has_target {
            self.pending_label_ref.take()
        } else {
            None
        }
    }

    /// Every fact of the unit, ordered by source offset.
    pub fn take_facts(&mut self) -> Vec<FlowFact> {
        let mut facts = std::mem::take(&mut self.facts);
        facts.sort_by_key(|fact| match fact {
            FlowFact::DeadCode { span }
            | FlowFact::UnassignedRead { span, .. }
            | FlowFact::PromotedRead { span, .. }
            | FlowFact::BodyMayCompleteNormally { span } => span.range.offset,
        });
        facts
    }

    /// Records the `handleExit` contract: a subexpression's static type is
    /// Never, so the flow state goes unreachable from the expression's end
    /// (official `FlowAnalysis.handleExit`, flow_analysis.dart:919-925).
    /// Fired by the engine once it resolves a subexpression's type to
    /// Never; the parser listener alone cannot know it.
    pub fn handle_exit(&mut self, end: TokenSpan) {
        self.record(Ev::Exit { end });
    }

    fn record(&mut self, ev: Ev) {
        if let Some(buffer) = self.buffer.as_mut() {
            buffer.push(ev);
        }
    }

    fn bin_op(&self, operator: Token) -> BinOp {
        match self.kind(operator) {
            Some(dartq_scanner::TokenKind::AmpersandAmpersand) => BinOp::And,
            Some(dartq_scanner::TokenKind::BarBar) => BinOp::Or,
            Some(dartq_scanner::TokenKind::QuestionQuestion) => BinOp::IfNull,
            Some(dartq_scanner::TokenKind::EqEq) => BinOp::Eq,
            Some(dartq_scanner::TokenKind::BangEq) => BinOp::NotEq,
            _ => BinOp::Other,
        }
    }

    /// `true` for `=`, `false` for every compound assignment operator
    /// (`+=`, `??=`, …). Synthetic operators read as compound.
    fn is_plain_assign(&self, operator: Token) -> bool {
        matches!(self.kind(operator), Some(dartq_scanner::TokenKind::Eq))
    }

    fn kind(&self, token: TokenSpan) -> Option<dartq_scanner::TokenKind> {
        let index = usize::try_from(token.id).ok()?;
        self.tokens.get(index).map(|token| token.kind)
    }

    /// Executable begin: starts a recording or nests inside the active one.
    fn exec_begin(&mut self, is_closure: bool) {
        match self.buffer.as_mut() {
            None => {
                self.buffer = Some(Vec::new());
                self.exec_depth = 1;
            }
            Some(buffer) => {
                buffer.push(Ev::ExecBegin { is_closure });
                self.exec_depth += 1;
            }
        }
    }

    /// Executable end: closes the outermost recording and runs the
    /// pipeline over it. `is_expr` marks function-expression ends, which
    /// net one value for the consumer (crate contract, see
    /// `SwitchExprBlockEnd`/`ListLitEnd`); `end` is the executable's end
    /// token — recorded on the nested `ExecEnd` event, and the
    /// completion fact's anchor for the outermost executable (which
    /// records no framing events of its own).
    fn exec_end(&mut self, is_expr: bool, end: Token) {
        self.exec_depth = self.exec_depth.saturating_sub(1);
        if self.exec_depth > 0 {
            self.record(Ev::ExecEnd { is_expr, end });
            return;
        }
        let Some(events) = self.buffer.take() else {
            return;
        };
        let ops = NameTypeOps;
        let mut interp = Interp::new(&ops, &self.type_args);
        interp.run(&events);
        // The outermost body's fall-through completion: after `run` the
        // interpreter's current state IS the body's exit state (official
        // `checkForBodyMayCompleteNormally`'s reachability gate,
        // type_analyzer.dart:647-652; nested executables emit at their
        // own `ExecEnd` inside the replay).
        if interp.reachable() {
            interp.emit(FlowFact::BodyMayCompleteNormally { span: end });
        }
        self.facts.append(&mut interp.facts);
    }
}

impl ParserListener for Flow<'_> {
    // —— executable framing ————————————————————————————————————————————

    fn begin_top_level_method(
        &mut self,
        _last_consumed: Token,
        _augment: Option<Token>,
        _external: Option<Token>,
    ) {
        self.exec_begin(false);
    }

    fn end_top_level_method(&mut self, _begin: Token, _get_or_set: Option<Token>, end: Token) {
        self.exec_end(false, end);
    }

    fn begin_method(
        &mut self,
        _kind: DeclarationKind,
        _augment_kw: Option<Token>,
        _external_kw: Option<Token>,
        _static_kw: Option<Token>,
        _covariant_kw: Option<Token>,
        _var_final_or_const: Option<Token>,
        _get_or_set: Option<Token>,
        _name: Token,
        _enclosing: Option<&str>,
    ) {
        self.exec_begin(false);
    }

    fn end_method(
        &mut self,
        _kind: DeclarationKind,
        _get_or_set: Option<Token>,
        _begin: Token,
        _begin_param: Token,
        _begin_initializers: Option<Token>,
        end: Token,
    ) {
        self.exec_end(false, end);
    }

    fn begin_constructor(
        &mut self,
        _kind: DeclarationKind,
        _augment_kw: Option<Token>,
        _external_kw: Option<Token>,
        _static_kw: Option<Token>,
        _covariant_kw: Option<Token>,
        _var_final_or_const: Option<Token>,
        _get_or_set: Option<Token>,
        _new_kw: Option<Token>,
        _name: Token,
        _enclosing: Option<&str>,
    ) {
        self.exec_begin(false);
    }

    fn end_constructor(
        &mut self,
        _kind: DeclarationKind,
        _begin: Token,
        _new_kw: Option<Token>,
        _begin_param: Token,
        _begin_initializers: Option<Token>,
        end: Token,
    ) {
        self.exec_end(false, end);
    }

    fn begin_factory(
        &mut self,
        _kind: DeclarationKind,
        _last_consumed: Token,
        _augment_kw: Option<Token>,
        _external_kw: Option<Token>,
        _const_keyword: Option<Token>,
    ) {
        self.exec_begin(false);
    }

    fn end_factory(&mut self, _kind: DeclarationKind, _begin: Token, _factory: Token, end: Token) {
        self.exec_end(false, end);
    }

    fn begin_local_function_declaration(&mut self, _token: Token) {
        self.exec_begin(true);
    }

    fn end_local_function_declaration(&mut self, end: Token) {
        self.exec_end(false, end);
        self.record(Ev::StmtEnd {
            end: end.range.end(),
        });
    }

    fn begin_function_expression(&mut self, _token: Token) {
        self.exec_begin(true);
    }

    fn end_function_expression(&mut self, _begin: Token, end: Token) {
        self.exec_end(true, end);
    }

    fn begin_named_function_expression(&mut self, _token: Token) {
        self.exec_begin(true);
    }

    fn end_named_function_expression(&mut self, end: Token) {
        self.exec_end(true, end);
    }

    // —— bodies ————————————————————————————————————————————————————————

    fn begin_block_function_body(&mut self, _token: Token) {
        self.record(Ev::BlockBodyBegin);
    }

    fn end_block_function_body(&mut self, _count: usize, _begin: Token, _end: Token) {
        self.record(Ev::BlockBodyEnd);
    }

    fn handle_expression_function_body(&mut self, _arrow: Token, _end: Option<Token>) {
        self.record(Ev::ExprBody);
    }

    fn handle_empty_function_body(&mut self, _semicolon: Token) {
        self.record(Ev::EmptyBody);
    }

    fn handle_async_modifier(&mut self, async_kw: Option<Token>, star: Option<Token>) {
        self.record(Ev::AsyncModifier {
            is_async: async_kw.is_some(),
            is_generator: star.is_some(),
        });
    }

    // —— blocks and asserts ——————————————————————————————————————————————

    fn begin_block(&mut self, token: Token, kind: BlockKind) {
        // The finally marker rides the finally block's BEGIN (official
        // `tryFinallyStatement_finallyBegin`): the finally context must be
        // open before the body's events replay. The parser's own
        // `handle_finally_block` fires only after the block has parsed.
        if matches!(kind, BlockKind::FinallyClause) {
            self.record(Ev::FinallyBlock);
        }
        // Narrow to the recorded subset: only plain statement blocks carry
        // the dead-code check; catch blocks still scope their locals; try,
        // finally, and switch blocks (and function bodies, which frame
        // through the dedicated body events) push scope only.
        let kind = match kind {
            BlockKind::Statement => crate::ev::BlockKind::Statement,
            BlockKind::FunctionBody => crate::ev::BlockKind::FunctionBody,
            BlockKind::CatchClause => crate::ev::BlockKind::CatchClause,
            BlockKind::TryStatement | BlockKind::FinallyClause | BlockKind::SwitchStatement => {
                crate::ev::BlockKind::Other
            }
        };
        self.record(Ev::Block { kind, span: token });
        // The dead-code row for a dead block spans the full `{ ... }`
        // extent (official dead_code reports the whole statement); the
        // closing brace only arrives at `endBlock`, so remember the
        // event index and widen it there.
        if let Some(buffer) = &self.buffer {
            self.block_events.push(buffer.len() - 1);
        }
    }

    fn end_block(&mut self, _count: usize, begin: Token, end: Token, _kind: BlockKind) {
        if let (Some(buffer), Some(index)) = (&mut self.buffer, self.block_events.pop())
            && let Some(Ev::Block { span, .. }) = buffer.get_mut(index)
        {
            span.range = dartq_source::SourceRange {
                offset: begin.range.offset,
                length: end.range.offset + end.range.length - begin.range.offset,
            };
        }
        // Inside a do-while block body this is the body's own close (the
        // last-wins order makes inner blocks yield to it).
        if self.do_body_is_block {
            self.do_body_end = Some(end);
        }
        self.record(Ev::EndBlock {
            end: end.range.end(),
        });
    }

    fn begin_assert(&mut self, token: Token, _kind: AssertKind) {
        // The kind carries no flow weight: every assertion's condition is
        // evaluated and its exit joins the pre-assert state, whatever the
        // parse position.
        self.record(Ev::AssertBegin { span: token });
    }

    fn end_assert(
        &mut self,
        _begin: Token,
        kind: AssertKind,
        _left_paren: Token,
        _comma: Option<Token>,
        end: Token,
    ) {
        self.record(Ev::AssertEnd);
        // The hook's `end` is the closing `)` (official `endAssert`
        // delivers the endGroup paren, parser_impl.dart:11137); the
        // statement's `;` rides no hook, so the completion event reads it
        // from the token table when the statement is well-formed (the
        // official dead-assert row spans through the `;` —
        // dead_code_test.dart `test_assert`, 14 carets over
        // `assert (true);`).
        let stmt_end = if matches!(kind, AssertKind::Statement)
            && let Some(next) = self.tokens.get(end.id as usize + 1)
            && next.kind == dartq_scanner::TokenKind::Semicolon
        {
            next.utf16_offset + 1
        } else {
            end.range.end()
        };
        self.record(Ev::StmtEnd { end: stmt_end });
    }

    // —— formal parameters ——————————————————————————————————————————————

    fn end_formal_parameter(
        &mut self,
        _var_or_final: Option<Token>,
        this_keyword: Option<Token>,
        _super_keyword: Option<Token>,
        _period: Option<Token>,
        name: Token,
        _initializer_start: Option<Token>,
        _initializer_end: Option<Token>,
        _kind: FormalParameterKind,
        _member_kind: MemberKind,
    ) {
        let name = self.name(name);
        self.record(Ev::Formal {
            name,
            this_or_super: this_keyword.is_some(),
        });
    }

    fn end_function_typed_formal_parameter(&mut self, token: Token, _question: Option<Token>) {
        // The official event's token is the parameter name position.
        let name = self.name(token);
        self.record(Ev::Formal {
            name,
            this_or_super: false,
        });
    }

    // —— local declarations —————————————————————————————————————————————

    fn begin_variables_declaration(
        &mut self,
        token: Token,
        late_kw: Option<Token>,
        var_final_or_const: Option<Token>,
    ) {
        let is_final = var_final_or_const.is_some_and(|token| {
            matches!(
                self.kind(token),
                Some(dartq_scanner::TokenKind::Keyword(
                    dartq_scanner::Keyword::Final | dartq_scanner::Keyword::Const
                ))
            )
        });
        self.record(Ev::VarDeclBegin {
            late: late_kw.is_some(),
            is_final,
            span: token,
        });
    }

    fn end_variables_declaration(&mut self, _count: usize, end: Option<Token>) {
        self.record(Ev::VarDeclEnd);
        if let Some(end) = end {
            self.record(Ev::StmtEnd {
                end: end.range.end(),
            });
        }
    }

    fn begin_initialized_identifier(&mut self, token: Token) {
        let name = self.name(token);
        self.record(Ev::InitializedIdent { name });
    }

    fn end_initialized_identifier(&mut self, _name: Token) {
        // The declare/initialize point is `InitializedIdent`; the paired
        // end event carries no payload the replay needs.
    }

    fn handle_no_variable_initializer(&mut self, _token: Token) {
        self.record(Ev::NoInitializer);
    }

    fn begin_variable_initializer(&mut self, _token: Token) {
        self.record(Ev::InitializerBegin);
    }

    fn end_variable_initializer(&mut self, _token: Token) {
        self.record(Ev::InitializerEnd);
    }

    fn handle_pattern_variable_declaration_statement(
        &mut self,
        keyword: Token,
        _equals: Token,
        semicolon: Token,
    ) {
        self.record(Ev::PatternVarDeclStmt { span: keyword });
        self.record(Ev::StmtEnd {
            end: semicolon.range.end(),
        });
    }

    // —— types ——————————————————————————————————————————————————————————

    fn handle_type(&mut self, name: Token, question: Option<Token>) {
        let id = self.name(name);
        // Classify on the type-name TOKEN's own lexeme: `name.id` is the
        // token index while `id` is the interned name id — unrelated
        // namespaces. Indexing the table by the interned id classified a
        // special base only when the ids collided (every fixture starting
        // `void f(` made `void` collide by accident).
        let base = match self
            .tokens
            .get(name.id as usize)
            .map(|_| self.tokens.lexeme(self.source, name.id as usize))
        {
            Some("dynamic") => crate::ty::Base::Dynamic,
            Some("void") => crate::ty::Base::Void,
            Some("Never") => crate::ty::Base::Never,
            _ => crate::ty::Base::Name(id),
        };
        // Consumed per row, so the pending list can never leak across
        // types; an argument row nested in an open group joins it.
        let args = std::mem::take(&mut self.pending_type_args);
        let nullable = question.is_some();
        if let Some(group) = self.type_arg_groups.last_mut() {
            group.push(crate::ty::ArgTy {
                base,
                nullable,
                args,
            });
        }
        self.record(Ev::Type { base, nullable, args });
    }

    fn begin_type_arguments(&mut self, _token: Token) {
        self.type_arg_groups.push(Vec::new());
    }

    fn handle_no_type_arguments(&mut self, _token: Token) {
        self.pending_type_args = crate::ty::EMPTY_ARGS;
    }

    fn end_type_arguments(&mut self, _count: usize, _begin: Token, _end: Token) {
        // The group is complete (every argument row fired before the
        // close, nested groups folded bottom-up); the head row follows.
        let entries = self.type_arg_groups.pop().unwrap_or_default();
        self.pending_type_args = self.intern_args(entries);
    }

    fn handle_no_type(&mut self, _last_consumed: Token) {
        self.record(Ev::NoType);
    }

    fn end_function_type(&mut self, _function: Token, _question: Option<Token>) {
        self.record(Ev::FunctionType);
    }

    fn handle_void_keyword(&mut self, _token: Token) {
        self.record(Ev::VoidType);
    }

    // —— statements ——————————————————————————————————————————————————————

    fn handle_expression_statement(&mut self, begin: Token, end: Token) {
        self.record(Ev::ExprStmt { span: begin });
        self.record(Ev::StmtEnd {
            end: end.range.end(),
        });
    }

    fn begin_if_statement(&mut self, token: Token) {
        self.record(Ev::IfBegin { span: token });
    }

    fn begin_then_statement(&mut self, _token: Token) {
        self.record(Ev::ThenBegin);
    }

    fn begin_else_statement(&mut self, _token: Token) {
        self.record(Ev::ElseBegin);
    }

    fn end_if_statement(&mut self, _token: Token, else_kw: Option<Token>, end: Token) {
        self.record(Ev::IfEnd {
            has_else: else_kw.is_some(),
        });
        self.record(Ev::StmtEnd {
            end: end.range.end(),
        });
    }

    fn begin_while_statement(&mut self, token: Token) {
        self.record(Ev::WhileBegin { span: token });
    }

    fn begin_while_statement_body(&mut self, _token: Token) {
        self.record(Ev::WhileBodyBegin);
    }

    fn end_while_statement_body(&mut self, _end: Token) {
        self.record(Ev::WhileBodyEnd);
    }

    fn end_while_statement(&mut self, _token: Token, end: Token) {
        self.record(Ev::WhileEnd);
        self.record(Ev::StmtEnd {
            end: end.range.end(),
        });
    }

    fn begin_do_while_statement(&mut self, token: Token) {
        self.record(Ev::DoBegin { span: token });
    }

    fn begin_do_while_statement_body(&mut self, token: Token) {
        self.record(Ev::DoBodyBegin);
        // The official `while` part starts at a block body's closing brace
        // (dead_code_verifier.dart:292-308); a non-block body starts at
        // the `while` keyword itself. The hook's own token is the body's
        // first token — a `{` means the body is the block whose `}` (the
        // last `end_block` inside the body) carries the part's start.
        self.do_body_is_block = self.kind(token) == Some(dartq_scanner::TokenKind::OpenCurly);
        self.do_body_end = None;
    }

    fn end_do_while_statement_body(&mut self, _end: Token) {
        // The hook's token is the one past the body (`while`), so the
        // brace comes from `end_block` above.
        self.record(Ev::DoBodyEnd);
    }

    fn end_do_while_statement(&mut self, _token: Token, while_kw: Token, end: Token) {
        let while_start = match self.do_body_end.take() {
            Some(body_end) if self.kind(body_end) == Some(dartq_scanner::TokenKind::CloseCurly) => {
                body_end.range.offset
            }
            _ => while_kw.range.offset,
        };
        self.record(Ev::DoEnd {
            while_range: dartq_source::SourceRange {
                offset: while_start,
                length: end.range.end() - while_start,
            },
        });
        self.record(Ev::StmtEnd {
            end: end.range.end(),
        });
    }

    fn begin_for_statement(&mut self, token: Token) {
        self.record(Ev::ForBegin { span: token });
    }

    fn handle_for_loop_parts(
        &mut self,
        _for_kw: Token,
        _left_paren: Token,
        left_separator: Token,
        right_separator: Token,
        _expression_count: usize,
    ) {
        self.record(Ev::ForParts {
            left_sep: left_separator,
            right_sep: right_separator,
        });
    }

    fn begin_for_statement_body(&mut self, _token: Token) {
        self.record(Ev::ForBodyBegin);
    }

    fn end_for_statement_body(&mut self, _end: Token) {
        self.record(Ev::ForBodyEnd);
    }

    fn end_for_statement(&mut self, end: Token) {
        self.record(Ev::ForEnd);
        self.record(Ev::StmtEnd {
            end: end.range.end(),
        });
    }

    fn handle_for_in_loop_parts(
        &mut self,
        _await_kw: Option<Token>,
        _for_kw: Token,
        _left_paren: Token,
        _pattern_keyword: Option<Token>,
        _in_kw: Token,
    ) {
        self.record(Ev::ForInParts);
    }

    fn begin_for_in_expression(&mut self, _token: Token) {
        self.record(Ev::ForInExprBegin);
    }

    fn end_for_in_expression(&mut self, _end: Token) {
        self.record(Ev::ForInExprEnd);
    }

    fn begin_for_in_body(&mut self, _token: Token) {
        self.record(Ev::ForInBodyBegin);
    }

    fn end_for_in_body(&mut self, _end: Token) {
        self.record(Ev::ForInBodyEnd);
    }

    fn end_for_in(&mut self, end: Token) {
        self.record(Ev::ForInEnd);
        self.record(Ev::StmtEnd {
            end: end.range.end(),
        });
    }

    fn end_return_statement(&mut self, has_expression: bool, begin: Token, end: Token) {
        self.record(Ev::ReturnBegin { span: begin });
        self.record(Ev::ReturnEnd { has_expression });
        self.record(Ev::StmtEnd {
            end: end.range.end(),
        });
    }

    fn end_yield_statement(&mut self, begin: Token, _star: Option<Token>, end: Token) {
        self.record(Ev::YieldBegin { span: begin });
        self.record(Ev::YieldEnd);
        self.record(Ev::StmtEnd {
            end: end.range.end(),
        });
    }

    fn begin_await_expression(&mut self, _token: Token) {
        self.record(Ev::AwaitBegin);
    }

    fn end_await_expression(&mut self, _begin: Token, _end: Token) {
        self.record(Ev::AwaitEnd);
    }

    fn handle_break_statement(&mut self, has_target: bool, break_kw: Token, end: Token) {
        let label = self.take_label(has_target);
        self.record(Ev::Break {
            label,
            span: break_kw,
        });
        self.record(Ev::StmtEnd {
            end: end.range.end(),
        });
    }

    fn handle_continue_statement(&mut self, has_target: bool, continue_kw: Token, end: Token) {
        let label = self.take_label(has_target);
        self.record(Ev::Continue {
            label,
            span: continue_kw,
        });
        self.record(Ev::StmtEnd {
            end: end.range.end(),
        });
    }

    fn handle_label(&mut self, _colon: Token) {
        // The label name arrives through `handle_identifier`
        // (`LabelDeclaration`); the colon carries no id.
    }

    fn begin_labeled_statement(&mut self, token: Token, _label_count: usize) {
        // The first label rides `LabeledBegin`, the rest follow as
        // `LabelDecl` — one frame tracks every label of the statement, so
        // `a: b: s` resolves a jump to either name (official
        // `getLabelTarget` scans every label of the labeled statement).
        let labels = std::mem::take(&mut self.pending_labels);
        let first = labels.first().copied();
        // The event token is the statement PAST the labels (official fasta
        // shape); the dead row anchors at the labeled statement's own
        // `firstToken` — the first label (dead_code_verifier.dart:253) —
        // so the latched label token wins when one was seen. No latch
        // (recovered label without a name) falls back to the event token.
        let anchor = self.pending_label_token.take().unwrap_or(token);
        self.record(Ev::LabeledBegin {
            label: first,
            span: anchor,
        });
        for id in labels.into_iter().skip(1) {
            self.record(Ev::LabelDecl { id });
        }
    }

    fn end_labeled_statement(&mut self, _label_count: usize) {
        self.record(Ev::LabeledEnd);
    }

    fn begin_try_statement(&mut self, token: Token) {
        self.record(Ev::TryBegin { span: token });
    }

    fn handle_catch_block(
        &mut self,
        _on_kw: Option<Token>,
        _catch_kw: Option<Token>,
        _comma: Option<Token>,
    ) {
        self.record(Ev::CatchClauseEnd);
    }

    fn begin_catch_clause(&mut self, _token: Token) {
        self.record(Ev::CatchClauseBegin);
    }

    fn end_catch_clause(&mut self, _token: Token) {
        // The clause HEADER closes here; the clause BODY ends at
        // `handle_catch_block`, which records the state join.
    }

    fn handle_finally_block(&mut self, _finally_kw: Token) {
        // Already recorded at the finally block's begin (official
        // `tryFinallyStatement_finallyBegin` position — before the body).
    }

    fn end_try_statement(
        &mut self,
        _count: usize,
        _begin: Token,
        finally_kw: Option<Token>,
        end: Token,
    ) {
        self.record(Ev::TryEnd {
            has_finally: finally_kw.is_some(),
        });
        self.record(Ev::StmtEnd {
            end: end.range.end(),
        });
    }

    fn begin_switch_statement(&mut self, token: Token) {
        self.record(Ev::SwitchBegin { span: token });
    }

    fn begin_switch_block(&mut self, _token: Token) {
        self.record(Ev::SwitchBlockBegin);
    }

    fn end_switch_block(&mut self, _count: usize, _begin: Token, _end: Token) {
        self.record(Ev::SwitchBlockEnd);
    }

    fn begin_switch_case(&mut self, label_count: usize, expression_count: usize, token: Token) {
        // Case labels are counted on `CaseBegin`; drop their name
        // captures so they can never attach to a later labeled statement.
        self.pending_labels.clear();
        self.pending_label_token = None;
        self.record(Ev::CaseBegin {
            label_count,
            expression_count,
            // A default-headed group's member begin token is the `default`
            // keyword (or the first label of a labeled default) — the
            // head-time anchor face.
            kw: (expression_count == 0).then_some(token),
        });
    }

    fn begin_case_expression(&mut self, case_kw: Token) {
        self.record(Ev::CaseExprBegin { span: case_kw });
    }

    fn handle_switch_case_no_when_clause(&mut self, _token: Token) {
        self.record(Ev::CaseNoWhen);
    }

    fn end_case_expression(&mut self, case_kw: Token, when: Option<Token>, _colon: Token) {
        self.record(Ev::CaseExprEnd {
            has_guard: when.is_some(),
            kw: case_kw,
        });
    }

    fn end_switch_case(
        &mut self,
        _label_count: usize,
        _expression_count: usize,
        default_kw: Option<Token>,
        _colon_after_default: Option<Token>,
        statement_count: usize,
        _begin: Token,
        _end: Token,
    ) {
        self.record(Ev::CaseEnd {
            statement_count,
            default_kw,
        });
    }

    fn end_switch_statement(&mut self, _token: Token, end: Token) {
        self.record(Ev::SwitchEnd);
        self.record(Ev::StmtEnd {
            end: end.range.end(),
        });
    }

    fn begin_switch_expression(&mut self, token: Token) {
        self.record(Ev::SwitchExprBegin { span: token });
    }

    fn begin_switch_expression_block(&mut self, _token: Token) {
        self.record(Ev::SwitchExprBlockBegin);
    }

    fn end_switch_expression_block(&mut self, _count: usize, _begin: Token, _end: Token) {
        self.record(Ev::SwitchExprBlockEnd);
    }

    fn begin_switch_expression_case(&mut self) {
        self.record(Ev::SwitchExprCaseBegin);
    }

    fn handle_switch_expression_case_pattern(&mut self, _token: Token) {
        self.record(Ev::SwitchExprCasePattern);
    }

    fn end_switch_expression_case(
        &mut self,
        _begin: Token,
        when: Option<Token>,
        _arrow: Token,
        _end: Token,
    ) {
        self.record(Ev::SwitchExprCaseEnd {
            has_guard: when.is_some(),
        });
    }

    // —— expressions —————————————————————————————————————————————————————

    fn handle_identifier(&mut self, token: Token, context: IdentifierContext) {
        match context {
            IdentifierContext::Expression
            | IdentifierContext::ExpressionContinuation
            | IdentifierContext::NamedArgumentReference
            | IdentifierContext::CatchParameter
            | IdentifierContext::LocalVariableDeclaration => {
                let name = self.name(token);
                self.record(Ev::Ident {
                    name,
                    token,
                    context,
                });
            }
            IdentifierContext::LabelReference => {
                // A break/continue target name (or case label): held for
                // the next break/continue event, which carries only
                // `has_target`.
                self.pending_label_ref = Some(self.name(token));
            }
            IdentifierContext::LabelDeclaration => {
                // A label name (`label:` on a statement or a switch case),
                // consumed by the enclosing construct's begin event.
                let id = self.name(token);
                self.pending_labels.push(id);
                if self.pending_label_token.is_none() {
                    self.pending_label_token = Some(token);
                }
            }
            _ => {}
        }
    }

    fn handle_literal_bool(&mut self, token: Token) {
        let value = self.kind(token)
            == Some(dartq_scanner::TokenKind::Keyword(
                dartq_scanner::Keyword::True,
            ));
        self.record(Ev::Bool { value });
    }

    fn handle_literal_null(&mut self, _token: Token) {
        self.record(Ev::Null);
    }

    fn handle_literal_int(&mut self, token: Token) {
        self.record(Ev::OtherExpr { span: token });
    }

    fn handle_literal_int_with_separators(&mut self, token: Token) {
        self.record(Ev::OtherExpr { span: token });
    }

    fn handle_literal_double(&mut self, token: Token) {
        self.record(Ev::OtherExpr { span: token });
    }

    fn handle_literal_double_with_separators(&mut self, token: Token) {
        self.record(Ev::OtherExpr { span: token });
    }

    fn end_literal_string(&mut self, interpolation_count: usize, _end: Token) {
        self.record(Ev::EndLiteralString {
            interpolation_count,
        });
    }

    fn begin_literal_symbol(&mut self, _token: Token) {
        // The symbol body contributes no expression events.
    }

    fn handle_literal_list(
        &mut self,
        count: usize,
        _left: Token,
        _const_keyword: Option<Token>,
        _right: Token,
    ) {
        self.record(Ev::ListLitEnd { count });
    }

    fn handle_literal_set_or_map(
        &mut self,
        count: usize,
        _left: Token,
        _const_keyword: Option<Token>,
        _right: Token,
        _has_set_entry: bool,
    ) {
        self.record(Ev::ListLitEnd { count });
    }

    fn handle_literal_map_entry(
        &mut self,
        _colon: Token,
        _end: Token,
        _null_aware_key: Option<Token>,
        _null_aware_value: Option<Token>,
    ) {
        self.record(Ev::MapEntryEnd);
    }

    fn handle_this_expression(&mut self, token: Token, _context: IdentifierContext) {
        self.record(Ev::OtherExpr { span: token });
    }

    fn handle_super_expression(&mut self, token: Token, _context: IdentifierContext) {
        self.record(Ev::OtherExpr { span: token });
    }

    fn handle_unary_prefix_expression(&mut self, operator: Token) {
        self.record(Ev::UnaryPrefix {
            is_not: self.kind(operator) == Some(dartq_scanner::TokenKind::Bang),
        });
    }

    fn handle_non_null_assert_expression(&mut self, _bang: Token) {
        self.record(Ev::NonNullAssert);
    }

    fn begin_binary_expression(&mut self, operator: Token) {
        self.record(Ev::BinaryBegin {
            op: self.bin_op(operator),
            span: operator,
        });
    }

    fn end_binary_expression(&mut self, operator: Token, end: Token) {
        self.record(Ev::BinaryEnd {
            op: self.bin_op(operator),
            end: end.range.end(),
        });
    }

    fn begin_is_operator_type(&mut self, _operator: Token) {
        self.record(Ev::IsBegin);
    }

    // The polarity only arrives with the follow-up `handle_is_operator`
    // call (official `parseIsOperatorRest` fires `endIsOperatorType`
    // before `handleIsOperator`), so the flow's completion pair is
    // emitted there: the interp applies the branch at `IsEnd` from the
    // `(is_not, tested)` state `IsOperator` establishes.
    fn end_is_operator_type(&mut self, _operator: Token) {}

    fn handle_is_operator(&mut self, _is_operator: Token, not: Option<Token>) {
        self.record(Ev::IsOperator {
            is_not: not.is_some(),
        });
        self.record(Ev::IsEnd);
    }

    fn begin_as_operator_type(&mut self, _operator: Token) {
        self.record(Ev::AsBegin);
    }

    fn end_as_operator_type(&mut self, _operator: Token) {
        self.record(Ev::AsEnd);
    }

    fn handle_as_operator(&mut self, _operator: Token) {
        self.record(Ev::AsOperator);
    }

    fn handle_assignment_expression(&mut self, operator: Token, _end: Token) {
        self.record(Ev::Assign {
            plain: self.is_plain_assign(operator),
        });
    }

    fn handle_unary_postfix_assignment_expression(&mut self, operator: Token) {
        // The operator's end is the updater expression's end — the anchor
        // the dead-updater row needs (header parts carry no statement
        // wrapper, dead_code_verifier.dart:361-365's `updaters.endToken`).
        self.record(Ev::UnaryPostfixAssign {
            end: operator.range.end(),
        });
    }

    fn handle_for_initializer_local_variable_declaration(&mut self, _end: Token, _for_in: bool) {
        // The init/cond index boundary: the condition's expression events
        // precede their `ExprStmt` wrapper (the wrapper fires at the
        // statement's end), so a literal condition's value would fall into
        // the init region of the offset-based header partition.
        self.record(Ev::ForInit);
    }

    fn handle_for_initializer_expression_statement(&mut self, _end: Token, _for_in: bool) {
        self.record(Ev::ForInit);
    }

    fn handle_for_initializer_empty_statement(&mut self, _at: Token) {
        self.record(Ev::ForInit);
    }

    fn handle_pattern_assignment(&mut self, _equals: Token) {
        self.record(Ev::PatternAssign);
    }

    fn handle_for_initializer_pattern_variable_assignment(
        &mut self,
        _pattern_keyword: Token,
        _equals: Token,
    ) {
        self.record(Ev::PatternAssign);
    }

    fn begin_conditional_expression(&mut self, _question: Token) {
        self.record(Ev::CondBegin);
    }

    fn end_conditional_expression_then_arm(
        &mut self,
        _question: Token,
        then_begin: Token,
        _colon: Token,
        then_end: Token,
    ) {
        // Latched for the argless colon call that follows immediately.
        self.cond_then = Some((then_begin, then_end.range.end()));
    }

    fn handle_conditional_expression_colon(&mut self) {
        let (then_begin, then_end) = self.cond_then.take().unwrap_or((
            TokenSpan {
                id: 0,
                range: dartq_source::SourceRange {
                    offset: 0,
                    length: 0,
                },
            },
            0,
        ));
        self.record(Ev::CondColon {
            then_begin,
            then_end,
        });
    }

    fn end_conditional_expression(&mut self, _question: Token, colon: Token, end: Token) {
        // The else arm starts at the token after the colon (official
        // `flowEnd(elseExpression)` rows span the arm's own range).
        let else_begin = self
            .tokens
            .next(colon.id as usize)
            .and_then(|index| self.span_at(index))
            .unwrap_or(colon);
        self.record(Ev::CondEnd {
            else_begin,
            end: end.range.end(),
        });
    }

    fn handle_dot_access(&mut self, _token: Token, _end: Token, is_null_aware: bool) {
        self.record(Ev::DotAccess {
            null_aware: is_null_aware,
        });
    }

    fn handle_indexed_expression(&mut self, question: Option<Token>, _open: Token, _close: Token) {
        self.record(Ev::IndexAccess {
            null_aware: question.is_some(),
        });
    }

    fn handle_send(&mut self, _begin: Token, _end: Token) {
        self.record(Ev::Send);
    }

    fn handle_send_without_arguments(&mut self, _begin: Token, _end: Token, _next: Token) {
        self.record(Ev::SendNoArgs);
    }

    fn begin_arguments(&mut self, _token: Token) {
        self.record(Ev::ArgumentsBegin);
    }

    fn end_arguments(&mut self, count: usize, _begin: Token, _end: Token) {
        self.record(Ev::ArgumentsEnd { count });
    }

    fn handle_named_argument(&mut self, _colon: Token) {
        // The named-argument marker rides the identifier event.
    }

    fn handle_throw_expression(&mut self, _throw: Token, _end: Token) {
        self.record(Ev::Throw);
    }

    fn begin_parenthesized_expression_or_record_literal(&mut self, _token: Token) {
        self.record(Ev::ParenBegin);
    }

    fn end_parenthesized_expression(&mut self, _token: Token) {
        self.record(Ev::ParenEnd);
    }

    fn end_record_literal(&mut self, _token: Token, count: usize, _const_keyword: Option<Token>) {
        self.record(Ev::RecordLitEnd { count });
    }

    fn begin_cascade(&mut self, _operator: Token) {
        self.record(Ev::CascadeBegin);
    }

    fn handle_cascade_access(&mut self, _operator: Token, _end: Token, is_null_aware: bool) {
        self.record(Ev::CascadeAccess {
            null_aware: is_null_aware,
        });
    }

    fn end_cascade(&mut self) {
        self.record(Ev::CascadeEnd);
    }

    fn handle_spread_expression(&mut self, _operator: Token) {
        self.record(Ev::Spread);
    }

    fn handle_null_aware_element(&mut self, _token: Token) {
        self.record(Ev::NullAwareElement);
    }

    fn begin_if_control_flow(&mut self, _token: Token) {
        self.record(Ev::IfControlFlowBegin);
    }

    fn handle_then_control_flow(&mut self, _token: Token) {
        self.record(Ev::ThenControlFlow);
    }

    fn handle_else_control_flow(&mut self, _token: Token) {
        self.record(Ev::ElseControlFlow);
    }

    fn end_if_control_flow(&mut self, _token: Token) {
        self.record(Ev::EndIfControlFlow { has_else: false });
    }

    fn end_if_else_control_flow(&mut self, _token: Token) {
        self.record(Ev::EndIfControlFlow { has_else: true });
    }

    fn begin_for_control_flow(&mut self, _await_kw: Option<Token>, _for_kw: Token) {
        self.record(Ev::ForControlFlowBegin);
    }

    fn end_for_control_flow(&mut self, _token: Token) {
        self.record(Ev::ForControlFlowEnd { for_in: false });
    }

    fn end_for_in_control_flow(&mut self, _token: Token) {
        self.record(Ev::ForControlFlowEnd { for_in: true });
    }

    // —— patterns ————————————————————————————————————————————————————————

    fn begin_pattern(&mut self, _token: Token) {
        self.record(Ev::PatternBegin);
    }

    fn end_pattern(&mut self, _token: Token) {
        self.record(Ev::PatternEnd);
    }

    fn begin_constant_pattern(&mut self, _const_keyword: Option<Token>) {
        self.record(Ev::ConstantPatternBegin);
    }

    fn end_constant_pattern(&mut self, _const_keyword: Option<Token>) {
        self.record(Ev::ConstantPatternEnd);
    }

    fn handle_declared_variable_pattern(
        &mut self,
        keyword: Option<Token>,
        variable: Token,
        _in_assignment_pattern: bool,
    ) {
        let name = self.name(variable);
        self.record(Ev::DeclaredVarPattern {
            name,
            is_final: keyword.is_some(),
        });
    }

    fn handle_assigned_variable_pattern(&mut self, variable: Token) {
        let name = self.name(variable);
        self.record(Ev::AssignedVarPattern { name });
    }

    fn handle_wildcard_pattern(&mut self, _keyword: Option<Token>, wildcard: Token) {
        self.record(Ev::Wildcard { token: wildcard });
    }

    fn begin_pattern_guard(&mut self, _when: Token) {
        self.record(Ev::GuardBegin);
    }

    fn end_pattern_guard(&mut self, _token: Token) {
        self.record(Ev::GuardEnd);
    }

    fn begin_switch_case_when_clause(&mut self, _token: Token) {
        // A switch-statement case guard. The guard's value is mandatory
        // flow input: official `switch_endAlternative` receives it as
        // `guardInfo` (flow_analysis.dart:1725-1740).
        self.record(Ev::GuardBegin);
    }

    fn end_switch_case_when_clause(&mut self, _token: Token) {
        self.record(Ev::GuardEnd);
    }

    // —— deliberately unrecorded faces ———————————————————————————————————

    fn handle_recoverable_error(&mut self, _message: Message, _start: Token, _end: Token) {}
    fn handle_experiment_not_enabled(
        &mut self,
        _flag: ExperimentalFlag,
        _begin: Token,
        _end: Token,
    ) {
    }
    fn end_invalid_await_expression(&mut self, _begin: Token, _end: Token) {}
    fn end_invalid_yield_statement(&mut self, _begin: Token, _star: Option<Token>, _end: Token) {}
    fn handle_invalid_function_body(&mut self, _token: Token) {}
    fn handle_invalid_statement(&mut self, _begin: Token) {}
    fn begin_rethrow_statement(&mut self, _token: Token) {}
    fn end_rethrow_statement(&mut self, _rethrow_kw: Token, _end: Token) {
        self.record(Ev::Rethrow);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ev::Ev;
    use dartq_parser::{IdentifierContext, ParserListener};
    use dartq_scanner::ScanConfig;

    /// Label ids ride the recorded events: the first label on
    /// `LabeledBegin`, additional labels on `LabelDecl`, and the
    /// break/continue target on `Break`/`Continue` — interned by lexeme,
    /// so the same name at different tokens resolves to the same id.
    #[test]
    fn label_ids_are_recorded_and_stable() {
        let source = "void f() { a: b: break a; a: break a; }";
        let result = dartq_scanner::scan(
            source,
            ScanConfig {
                enable_triple_shift: true,
                enable_augmentations: true,
            },
            dartq_source::FileTable::new().intern(std::path::Path::new("test.dart")),
        );
        assert!(result.diagnostics.is_empty());
        let tokens: &'static TokenTable = Box::leak(Box::new(result.tokens));
        // The Nth token with the given lexeme (labels recur in the source).
        let id_of = |lexeme: &str, occurrence: usize| -> u32 {
            let mut seen = 0;
            for index in 0.. {
                match tokens.get(index) {
                    Some(_) if tokens.lexeme(source, index) == lexeme => {
                        if seen == occurrence {
                            return u32::try_from(index).expect("token index fits u32");
                        }
                        seen += 1;
                    }
                    Some(_) => {}
                    None => panic!("token {lexeme}#{occurrence} not found"),
                }
            }
            unreachable!()
        };
        let sp = |id: u32| TokenSpan {
            id,
            range: dartq_source::SourceRange {
                offset: id * 2,
                length: 1,
            },
        };
        let (a1, a2, b) = (id_of("a", 0), id_of("a", 1), id_of("b", 0));

        let mut flow = Flow::new(tokens, source);
        flow.begin_top_level_method(sp(0), None, None);
        // `a: b: break a;`
        flow.handle_identifier(sp(a1), IdentifierContext::LabelDeclaration);
        flow.handle_identifier(sp(b), IdentifierContext::LabelDeclaration);
        flow.begin_labeled_statement(sp(a1), 2);
        flow.handle_identifier(sp(a1), IdentifierContext::LabelReference);
        flow.handle_break_statement(true, sp(a1), sp(a1));
        flow.end_labeled_statement(2);
        // `a: break a;` — a different `a` token, same lexeme.
        flow.handle_identifier(sp(a2), IdentifierContext::LabelDeclaration);
        flow.begin_labeled_statement(sp(a2), 1);
        flow.handle_identifier(sp(a2), IdentifierContext::LabelReference);
        flow.handle_break_statement(true, sp(a2), sp(a2));
        flow.end_labeled_statement(1);

        let events = flow.buffer.as_ref().unwrap();
        // 5 framed events + two breaks, each of which also records its
        // statement-completion `StmtEnd`.
        assert_eq!(events.len(), 9);
        let Ev::LabeledBegin { label: first_a, .. } = events[0] else {
            panic!("expected LabeledBegin, got {:?}", events[0]);
        };
        let Ev::LabelDecl { id: b_id } = events[1] else {
            panic!("expected LabelDecl, got {:?}", events[1]);
        };
        assert_ne!(Some(b_id), first_a, "distinct labels intern distinctly");
        let Ev::Break { label, .. } = events[2] else {
            panic!("expected Break, got {:?}", events[2]);
        };
        assert_eq!(label, first_a, "the break target carries its label id");
        let Ev::LabeledBegin {
            label: second_a, ..
        } = events[5]
        else {
            panic!("expected LabeledBegin, got {:?}", events[5]);
        };
        assert_eq!(
            second_a, first_a,
            "the same lexeme interns to the same id across tokens"
        );
    }

    /// Type-base classification reads the type-name TOKEN's lexeme: the
    /// old code indexed the token table by the interned name id, so
    /// `dynamic`, `Never`, and every `void` past the file's head
    /// classified as a plain named type (`void` only worked because every
    /// fixture began `void f(`, colliding id 0 with token 0).
    #[test]
    fn type_bases_classify_on_the_type_token_not_the_interned_id() {
        let source = "int x; void f(dynamic d) { if (d is Never) {} }";
        let result = dartq_scanner::scan(
            source,
            ScanConfig {
                enable_triple_shift: true,
                enable_augmentations: true,
            },
            dartq_source::FileTable::new().intern(std::path::Path::new("test.dart")),
        );
        assert!(result.diagnostics.is_empty());
        let tokens: &'static TokenTable = Box::leak(Box::new(result.tokens));
        // The Nth token with the given lexeme.
        let index_of = |lexeme: &str| -> u32 {
            let index = (0..)
                .map_while(|i| tokens.get(i).map(|_| i))
                .find(|&i| tokens.lexeme(source, i) == lexeme)
                .unwrap_or_else(|| panic!("token {lexeme} not found"));
            u32::try_from(index).expect("token index fits u32")
        };
        let sp = |id: u32| TokenSpan {
            id,
            range: dartq_source::SourceRange {
                offset: id * 2,
                length: 1,
            },
        };

        let mut flow = Flow::new(tokens, source);
        flow.begin_top_level_method(sp(0), None, None);
        // Interned in this order: int=0, void=1, dynamic=2, Never=3 — the
        // old code read tokens[0..=3] ("int", "x", ";", "void") instead of
        // each type name's own token.
        flow.handle_type(sp(index_of("int")), None);
        flow.handle_type(sp(index_of("void")), None);
        flow.handle_type(sp(index_of("dynamic")), None);
        flow.handle_type(sp(index_of("Never")), None);

        let events = flow.buffer.as_ref().unwrap();
        let bases: Vec<crate::ty::Base> = events
            .iter()
            .filter_map(|ev| match ev {
                Ev::Type { base, .. } => Some(*base),
                _ => None,
            })
            .collect();
        assert!(matches!(bases.first(), Some(crate::ty::Base::Name(0))));
        assert_eq!(bases.get(1), Some(&crate::ty::Base::Void));
        assert_eq!(bases.get(2), Some(&crate::ty::Base::Dynamic));
        assert_eq!(bases.get(3), Some(&crate::ty::Base::Never));
    }
}
