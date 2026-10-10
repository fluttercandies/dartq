//! The recording half of the engine: [`Flow`] implements
//! [`ParserListener`], buffers the flow-relevant events of each executable
//! (method, top-level function, constructor, factory, local function,
//! closure), and runs the replay pipeline when the executable ends.

use dartq_parser::{
    DeclarationKind, ExperimentalFlag, FormalParameterKind, IdentifierContext, MemberKind, Message,
    ParserListener,
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
    /// The most recent label-reference identifier (a break/continue
    /// target's name — the break/continue events themselves carry only
    /// `has_target`).
    pending_label_ref: Option<u32>,
    /// Recording buffer of the outermost active executable, if any.
    buffer: Option<Vec<Ev>>,
    /// Executable nesting depth inside the active recording.
    exec_depth: usize,
    facts: Vec<FlowFact>,
}

impl<'t> Flow<'t> {
    pub fn new(tokens: &'t TokenTable, source: &'t str) -> Self {
        Self {
            tokens,
            source,
            names: FxHashMap::default(),
            pending_label_ref: None,
            buffer: None,
            exec_depth: 0,
            facts: Vec::new(),
        }
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
            let id = self.names.len() as u32;
            self.names.insert(lexeme.into(), id);
            return id;
        }
        // Parser-inserted synthetic tokens key on the token handle.
        let key: Box<str> = format!("#{}", token.id).into();
        if let Some(&id) = self.names.get(&key) {
            return id;
        }
        let id = self.names.len() as u32;
        self.names.insert(key, id);
        id
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
            FlowFact::DeadCode { span } | FlowFact::UnassignedRead { span, .. } => {
                span.range.offset
            }
        });
        facts
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
    /// `SwitchExprBlockEnd`/`ListLitEnd`).
    fn exec_end(&mut self, is_expr: bool) {
        self.exec_depth = self.exec_depth.saturating_sub(1);
        if self.exec_depth > 0 {
            self.record(Ev::ExecEnd { is_expr });
            return;
        }
        let Some(events) = self.buffer.take() else {
            return;
        };
        let ops = NameTypeOps;
        let mut interp = Interp::new(&ops);
        interp.run(events);
        self.facts.append(&mut interp.facts);
    }
}

impl<'t> ParserListener for Flow<'t> {
    // —— executable framing ————————————————————————————————————————————

    fn begin_top_level_method(
        &mut self,
        _last_consumed: Token,
        _augment: Option<Token>,
        _external: Option<Token>,
    ) {
        self.exec_begin(false);
    }

    fn end_top_level_method(&mut self, _begin: Token, _get_or_set: Option<Token>, _end: Token) {
        self.exec_end(false);
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
        _end: Token,
    ) {
        self.exec_end(false);
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
        _end: Token,
    ) {
        self.exec_end(false);
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

    fn end_factory(&mut self, _kind: DeclarationKind, _begin: Token, _factory: Token, _end: Token) {
        self.exec_end(false);
    }

    fn begin_local_function_declaration(&mut self, _token: Token) {
        self.exec_begin(true);
    }

    fn end_local_function_declaration(&mut self, _end: Token) {
        self.exec_end(false);
    }

    fn begin_function_expression(&mut self, _token: Token) {
        self.exec_begin(true);
    }

    fn end_function_expression(&mut self, _begin: Token, _end: Token) {
        self.exec_end(true);
    }

    fn begin_named_function_expression(&mut self, _token: Token) {
        self.exec_begin(true);
    }

    fn end_named_function_expression(&mut self, _end: Token) {
        self.exec_end(true);
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

    fn end_variables_declaration(&mut self, _count: usize, _end: Option<Token>) {
        self.record(Ev::VarDeclEnd);
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
        _semicolon: Token,
    ) {
        self.record(Ev::PatternVarDeclStmt { span: keyword });
    }

    // —— types ——————————————————————————————————————————————————————————

    fn handle_type(&mut self, name: Token, question: Option<Token>) {
        let id = self.name(name);
        let base = match self
            .tokens
            .get(id as usize)
            .map(|_| self.tokens.lexeme(self.source, id as usize))
        {
            Some("dynamic") => crate::ty::Base::Dynamic,
            Some("void") => crate::ty::Base::Void,
            Some("Never") => crate::ty::Base::Never,
            _ => crate::ty::Base::Name(id),
        };
        self.record(Ev::Type {
            base,
            nullable: question.is_some(),
        });
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

    fn handle_expression_statement(&mut self, begin: Token, _end: Token) {
        self.record(Ev::ExprStmt { span: begin });
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

    fn end_if_statement(&mut self, _token: Token, else_kw: Option<Token>, _end: Token) {
        self.record(Ev::IfEnd {
            has_else: else_kw.is_some(),
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

    fn end_while_statement(&mut self, _token: Token, _end: Token) {
        self.record(Ev::WhileEnd);
    }

    fn begin_do_while_statement(&mut self, token: Token) {
        self.record(Ev::DoBegin { span: token });
    }

    fn begin_do_while_statement_body(&mut self, _token: Token) {
        self.record(Ev::DoBodyBegin);
    }

    fn end_do_while_statement_body(&mut self, _end: Token) {
        self.record(Ev::DoBodyEnd);
    }

    fn end_do_while_statement(&mut self, _token: Token, _while_kw: Token, _end: Token) {
        self.record(Ev::DoEnd);
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

    fn end_for_statement(&mut self, _end: Token) {
        self.record(Ev::ForEnd);
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

    fn end_for_in(&mut self, _end: Token) {
        self.record(Ev::ForInEnd);
    }

    fn end_return_statement(&mut self, has_expression: bool, begin: Token, _end: Token) {
        self.record(Ev::ReturnBegin { span: begin });
        self.record(Ev::ReturnEnd { has_expression });
    }

    fn end_yield_statement(&mut self, begin: Token, _star: Option<Token>, _end: Token) {
        self.record(Ev::YieldBegin { span: begin });
        self.record(Ev::YieldEnd);
    }

    fn begin_await_expression(&mut self, _token: Token) {
        self.record(Ev::AwaitBegin);
    }

    fn end_await_expression(&mut self, _begin: Token, _end: Token) {
        self.record(Ev::AwaitEnd);
    }

    fn handle_break_statement(&mut self, has_target: bool, break_kw: Token, _end: Token) {
        let label = self.take_label(has_target);
        self.record(Ev::Break {
            label,
            span: break_kw,
        });
    }

    fn handle_continue_statement(&mut self, has_target: bool, continue_kw: Token, _end: Token) {
        let label = self.take_label(has_target);
        self.record(Ev::Continue {
            label,
            span: continue_kw,
        });
    }

    fn handle_label(&mut self, _colon: Token) {
        // Label declarations are tracked through the label name event.
    }

    fn begin_labeled_statement(&mut self, token: Token, _label_count: usize) {
        self.record(Ev::LabeledBegin { span: token });
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
        self.record(Ev::FinallyBlock);
    }

    fn end_try_statement(
        &mut self,
        _count: usize,
        _begin: Token,
        finally_kw: Option<Token>,
        _end: Token,
    ) {
        self.record(Ev::TryEnd {
            has_finally: finally_kw.is_some(),
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

    fn begin_switch_case(&mut self, label_count: usize, expression_count: usize, _token: Token) {
        self.record(Ev::CaseBegin {
            label_count,
            expression_count,
        });
    }

    fn begin_case_expression(&mut self, _case_kw: Token) {
        self.record(Ev::CaseExprBegin);
    }

    fn handle_switch_case_no_when_clause(&mut self, _token: Token) {
        self.record(Ev::CaseNoWhen);
    }

    fn end_case_expression(&mut self, _case_kw: Token, when: Option<Token>, _colon: Token) {
        self.record(Ev::CaseExprEnd {
            has_guard: when.is_some(),
        });
    }

    fn end_switch_case(
        &mut self,
        _label_count: usize,
        _expression_count: usize,
        _default_kw: Option<Token>,
        _colon_after_default: Option<Token>,
        statement_count: usize,
        _begin: Token,
        _end: Token,
    ) {
        self.record(Ev::CaseEnd { statement_count });
    }

    fn end_switch_statement(&mut self, _token: Token, _end: Token) {
        self.record(Ev::SwitchEnd);
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
        });
    }

    fn end_binary_expression(&mut self, operator: Token, _end: Token) {
        self.record(Ev::BinaryEnd {
            op: self.bin_op(operator),
        });
    }

    fn begin_is_operator_type(&mut self, _operator: Token) {
        self.record(Ev::IsBegin);
    }

    fn end_is_operator_type(&mut self, _operator: Token) {
        self.record(Ev::IsEnd);
    }

    fn handle_is_operator(&mut self, _is_operator: Token, not: Option<Token>) {
        self.record(Ev::IsOperator {
            is_not: not.is_some(),
        });
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

    fn handle_conditional_expression_colon(&mut self) {
        self.record(Ev::CondColon);
    }

    fn end_conditional_expression(&mut self, _question: Token, _colon: Token, _end: Token) {
        self.record(Ev::CondEnd);
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
