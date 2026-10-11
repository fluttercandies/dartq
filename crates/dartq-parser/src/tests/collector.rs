//! Shared test scaffolding: scan helper, event collector, parser setup.

use dartq_language::{FeatureSet, LanguageVersion};
use dartq_scanner::{ScanConfig, TokenTable};
use dartq_source::FileTable;

use crate::listener::Token;
use crate::listener::{ExperimentalFlag, ParserListener};
use crate::message::Message;
use crate::parser::Parser;
use crate::token_id::TokenSpan;

/// Scans `source` with the default (latest) configuration and asserts no
/// scanner diagnostics. Returns the leaked table + source so cursors can be
/// `'static` in test helpers.
pub(crate) fn scan_ok(source: &'static str) -> (&'static TokenTable, &'static str) {
    let config = ScanConfig {
        enable_triple_shift: true,
        enable_augmentations: true,
    };
    let mut files = FileTable::new();
    let file = files.intern(std::path::Path::new("test.dart"));
    let result = dartq_scanner::scan(source, config, file);
    assert!(
        result.diagnostics.is_empty(),
        "scanner diagnostics in {source:?}: {:?}",
        result.diagnostics
    );
    let table: &'static TokenTable = Box::leak(Box::new(result.tokens));
    let text: &'static str = Box::leak(source.to_string().into_boxed_str());
    (table, text)
}

/// Latest-version feature set.
pub(crate) fn latest_features() -> crate::ParserFeatures {
    crate::ParserFeatures::from_feature_set(FeatureSet::for_language_version(LanguageVersion::new(
        3, 15,
    )))
}

/// Flat event log entry: `("begin", "pattern")` style.
pub(crate) type Logged = (&'static str, &'static str);

/// Listener that records recoverable errors (code, start, end in UTF-16),
/// their problem texts (parallel to `errors`, for message-template pins),
/// their correction texts (same parallel slot), and a log of the structural
/// events the tests observe.
#[derive(Default)]
pub(crate) struct Collector {
    pub errors: Vec<(String, u32, u32)>,
    pub texts: Vec<String>,
    pub corrections: Vec<Option<String>>,
    pub log: Vec<Logged>,
}

impl Collector {
    pub(crate) fn error_codes(&self) -> Vec<String> {
        self.errors.iter().map(|(c, _, _)| c.clone()).collect()
    }

    /// Subsequence check over the log: every `(phase, name)` appears in
    /// order (gaps allowed).
    pub(crate) fn assert_sequence(&self, expected: &[Logged]) {
        let mut it = self.log.iter();
        for want in expected {
            assert!(
                it.any(|got| got == want),
                "expected {want:?} in order; log so far: {:?} (full: {:?})",
                self.log,
                self.errors
            );
        }
    }
}

impl ParserListener for Collector {
    fn handle_recoverable_error(&mut self, message: Message, start: TokenSpan, end: TokenSpan) {
        self.texts.push(message.text);
        self.corrections.push(message.correction);
        self.errors.push((
            message.code.name.to_owned(),
            start.range.offset,
            end.range.offset + end.range.length,
        ));
    }

    fn handle_experiment_not_enabled(
        &mut self,
        _flag: ExperimentalFlag,
        begin: TokenSpan,
        _end: TokenSpan,
    ) {
        self.errors.push((
            "experiment_not_enabled".into(),
            begin.range.offset,
            begin.range.offset,
        ));
    }

    // —— representative structural events for sequence assertions ————
    fn begin_pattern(&mut self, _: TokenSpan) {
        self.log.push(("begin", "pattern"));
    }
    fn end_pattern(&mut self, _: TokenSpan) {
        self.log.push(("end", "pattern"));
    }
    fn begin_constant_pattern(&mut self, _: Option<TokenSpan>) {
        self.log.push(("begin", "constant_pattern"));
    }
    fn end_constant_pattern(&mut self, _: Option<TokenSpan>) {
        self.log.push(("end", "constant_pattern"));
    }
    fn handle_type(&mut self, _: TokenSpan, _: Option<TokenSpan>) {
        self.log.push(("handle", "type"));
    }
    fn handle_no_type(&mut self, _: TokenSpan) {
        self.log.push(("handle", "no_type"));
    }
    fn begin_type_arguments(&mut self, _: TokenSpan) {
        self.log.push(("begin", "type_arguments"));
    }
    fn end_type_arguments(&mut self, _: usize, _: TokenSpan, _: TokenSpan) {
        self.log.push(("end", "type_arguments"));
    }
    fn handle_send(&mut self, _: TokenSpan, _: TokenSpan) {
        self.log.push(("handle", "send"));
    }
    fn handle_send_without_arguments(&mut self, _: Token, _: Token, _: Token) {
        // Official argument-less send (`handleSendWithoutArguments`,
        // listener.dart:1368): a bare-identifier constant-pattern operand
        // fires this variant, not `handle_send`.
        self.log.push(("handle", "send_without_arguments"));
    }
    fn handle_type_argument_application(&mut self, _: TokenSpan) {
        self.log.push(("handle", "type_argument_application"));
    }
    fn begin_function_typed_formal_parameter(&mut self, _: TokenSpan) {
        self.log.push(("begin", "function_typed_formal"));
    }
    fn end_function_typed_formal_parameter(&mut self, _: TokenSpan, _: Option<TokenSpan>) {
        self.log.push(("end", "function_typed_formal"));
    }
    fn begin_optional_formal_parameters(&mut self, _: TokenSpan) {
        self.log.push(("begin", "optional_formal_parameters"));
    }
    fn end_optional_formal_parameters(
        &mut self,
        _count: usize,
        _: TokenSpan,
        _: TokenSpan,
        _: crate::listener::MemberKind,
    ) {
        self.log.push(("end", "optional_formal_parameters"));
    }
    fn begin_record_type(&mut self, _: TokenSpan) {
        self.log.push(("begin", "record_type"));
    }
    fn end_record_type(&mut self, _: TokenSpan, _: Option<TokenSpan>, _: usize, _: bool) {
        self.log.push(("end", "record_type"));
    }
    fn begin_function_type(&mut self, _: TokenSpan) {
        self.log.push(("begin", "function_type"));
    }
    fn end_function_type(&mut self, _: TokenSpan, _: Option<TokenSpan>) {
        self.log.push(("end", "function_type"));
    }
    fn handle_list_pattern(&mut self, _: usize, _: TokenSpan, _: TokenSpan) {
        self.log.push(("handle", "list_pattern"));
    }
    fn handle_map_pattern(&mut self, _: usize, _: TokenSpan, _: TokenSpan) {
        self.log.push(("handle", "map_pattern"));
    }
    fn handle_record_pattern(&mut self, _: TokenSpan, _: usize) {
        self.log.push(("handle", "record_pattern"));
    }
    fn handle_parenthesized_pattern(&mut self, _: TokenSpan) {
        self.log.push(("handle", "parenthesized_pattern"));
    }
    fn handle_object_pattern(&mut self, _: TokenSpan, _: Option<TokenSpan>, _: Option<TokenSpan>) {
        self.log.push(("handle", "object_pattern"));
    }
    fn handle_wildcard_pattern(&mut self, _: Option<TokenSpan>, _: TokenSpan) {
        self.log.push(("handle", "wildcard_pattern"));
    }
    fn handle_declared_variable_pattern(&mut self, _: Option<TokenSpan>, _: TokenSpan, _: bool) {
        self.log.push(("handle", "declared_variable_pattern"));
    }
    fn handle_assigned_variable_pattern(&mut self, _: TokenSpan) {
        self.log.push(("handle", "assigned_variable_pattern"));
    }
    fn handle_cast_pattern(&mut self, _: TokenSpan) {
        self.log.push(("handle", "cast_pattern"));
    }
    fn handle_null_assert_pattern(&mut self, _: TokenSpan) {
        self.log.push(("handle", "null_assert_pattern"));
    }
    fn handle_null_check_pattern(&mut self, _: TokenSpan) {
        self.log.push(("handle", "null_check_pattern"));
    }
    fn begin_binary_pattern(&mut self, _: TokenSpan) {
        self.log.push(("begin", "binary_pattern"));
    }
    fn end_binary_pattern(&mut self, _: TokenSpan) {
        self.log.push(("end", "binary_pattern"));
    }
    fn handle_relational_pattern(&mut self, _: TokenSpan) {
        self.log.push(("handle", "relational_pattern"));
    }
    fn handle_literal_int(&mut self, _: TokenSpan) {
        self.log.push(("handle", "literal_int"));
    }
    fn handle_literal_bool(&mut self, _: TokenSpan) {
        self.log.push(("handle", "literal_bool"));
    }
    fn handle_literal_set_or_map(
        &mut self,
        _: usize,
        _: TokenSpan,
        _: Option<TokenSpan>,
        _: TokenSpan,
        _: bool,
    ) {
        self.log.push(("handle", "literal_set_or_map"));
    }
    fn begin_new_expression(&mut self, _: TokenSpan) {
        self.log.push(("begin", "new_expression"));
    }
    fn end_new_expression(&mut self, _: TokenSpan) {
        self.log.push(("end", "new_expression"));
    }

    // —— declarations (#18) ————————————————————————————————————————————
    fn begin_class_declaration(
        &mut self,
        _: Token,
        _: Option<Token>,
        _: Option<Token>,
        _: Option<Token>,
        _: Option<Token>,
        _: Option<Token>,
        _: Option<Token>,
        _: Option<Token>,
        _: Token,
    ) {
        self.log.push(("begin", "class_declaration"));
    }
    fn end_class_declaration(&mut self, _: Token, _: Token) {
        self.log.push(("end", "class_declaration"));
    }
    fn begin_named_mixin_application(
        &mut self,
        _: Token,
        _: Option<Token>,
        _: Option<Token>,
        _: Option<Token>,
        _: Option<Token>,
        _: Option<Token>,
        _: Option<Token>,
        _: Option<Token>,
        _: Token,
    ) {
        self.log.push(("begin", "named_mixin_application"));
    }
    fn end_named_mixin_application(
        &mut self,
        _: Token,
        _: Token,
        _: Token,
        _: Option<Token>,
        _: Token,
    ) {
        self.log.push(("end", "named_mixin_application"));
    }
    fn begin_mixin_declaration(
        &mut self,
        _: Token,
        _: Option<Token>,
        _: Option<Token>,
        _: Token,
        _: Token,
    ) {
        self.log.push(("begin", "mixin_declaration"));
    }
    fn end_mixin_declaration(&mut self, _: Token, _: Token) {
        self.log.push(("end", "mixin_declaration"));
    }
    fn begin_enum_declaration(&mut self, _: Token, _: Option<Token>, _: Token, _: Token) {
        self.log.push(("begin", "enum_declaration"));
    }
    fn end_enum_declaration(&mut self, _: Token, _: Token, _: Token, _: usize, _: Token) {
        self.log.push(("end", "enum_declaration"));
    }
    fn begin_extension_declaration(&mut self, _: Option<Token>, _: Token, _: Option<Token>) {
        self.log.push(("begin", "extension_declaration"));
    }
    fn end_extension_declaration(&mut self, _: Token, _: Token, _: Option<Token>, _: Token) {
        self.log.push(("end", "extension_declaration"));
    }
    fn begin_extension_type_declaration(&mut self, _: Option<Token>, _: Token, _: Token) {
        self.log.push(("begin", "extension_type_declaration"));
    }
    fn end_extension_type_declaration(
        &mut self,
        _: Token,
        _: Option<Token>,
        _: Token,
        _: Token,
        _: Token,
    ) {
        self.log.push(("end", "extension_type_declaration"));
    }
    fn begin_typedef(&mut self, _: Token) {
        self.log.push(("begin", "typedef"));
    }
    fn end_typedef(&mut self, _: Option<Token>, _: Token, _: Option<Token>, _: Token) {
        self.log.push(("end", "typedef"));
    }
    fn begin_top_level_member(&mut self, _: Token) {
        self.log.push(("begin", "top_level_member"));
    }
    fn begin_method(
        &mut self,
        _: crate::listener::DeclarationKind,
        _: Option<Token>,
        _: Option<Token>,
        _: Option<Token>,
        _: Option<Token>,
        _: Option<Token>,
        _: Option<Token>,
        _: Token,
        _: Option<&str>,
    ) {
        self.log.push(("begin", "method"));
    }
    fn end_method(
        &mut self,
        _: crate::listener::DeclarationKind,
        _: Option<Token>,
        _: Token,
        _: Token,
        _: Option<Token>,
        _: Token,
    ) {
        self.log.push(("end", "method"));
    }
    fn begin_constructor(
        &mut self,
        _: crate::listener::DeclarationKind,
        _: Option<Token>,
        _: Option<Token>,
        _: Option<Token>,
        _: Option<Token>,
        _: Option<Token>,
        _: Option<Token>,
        _: Option<Token>,
        _: Token,
        _: Option<&str>,
    ) {
        self.log.push(("begin", "constructor"));
    }
    fn end_constructor(
        &mut self,
        _: crate::listener::DeclarationKind,
        _: Token,
        _: Option<Token>,
        _: Token,
        _: Option<Token>,
        _: Token,
    ) {
        self.log.push(("end", "constructor"));
    }
    fn begin_factory(
        &mut self,
        _: crate::listener::DeclarationKind,
        _: Token,
        _: Option<Token>,
        _: Option<Token>,
        _: Option<Token>,
    ) {
        self.log.push(("begin", "factory"));
    }
    fn end_factory(&mut self, _: crate::listener::DeclarationKind, _: Token, _: Token, _: Token) {
        self.log.push(("end", "factory"));
    }
    fn begin_fields(
        &mut self,
        _: crate::listener::DeclarationKind,
        _: Option<Token>,
        _: Option<Token>,
        _: Option<Token>,
        _: Option<Token>,
        _: Option<Token>,
        _: Option<Token>,
        _: Option<Token>,
        _: Token,
    ) {
        self.log.push(("begin", "fields"));
    }
    fn end_fields(
        &mut self,
        _: crate::listener::DeclarationKind,
        _: Option<Token>,
        _: Option<Token>,
        _: Option<Token>,
        _: Option<Token>,
        _: Option<Token>,
        _: Option<Token>,
        _: Option<Token>,
        _: usize,
        _: Token,
        _: Token,
    ) {
        self.log.push(("end", "fields"));
    }
    fn begin_field_initializer(&mut self, _: Token) {
        self.log.push(("begin", "field_initializer"));
    }
    fn end_field_initializer(&mut self, _: Token, _: Token) {
        self.log.push(("end", "field_initializer"));
    }
    fn begin_initializers(&mut self, _: Token) {
        self.log.push(("begin", "initializers"));
    }
    fn end_initializers(&mut self, _: usize, _: Token, _: Token) {
        self.log.push(("end", "initializers"));
    }
    fn begin_initializer(&mut self, _: Token) {
        self.log.push(("begin", "initializer"));
    }
    fn end_initializer(&mut self, _: Token) {
        self.log.push(("end", "initializer"));
    }
    fn begin_binary_expression(&mut self, _: Token) {
        self.log.push(("begin", "binary_expression"));
    }
    fn end_binary_expression(&mut self, _: Token, _: Token) {
        self.log.push(("end", "binary_expression"));
    }
    fn begin_class_or_mixin_or_extension_body(
        &mut self,
        _: crate::listener::DeclarationKind,
        _: Token,
    ) {
        self.log.push(("begin", "body"));
    }
    fn end_class_or_mixin_or_extension_body(
        &mut self,
        _: crate::listener::DeclarationKind,
        _: usize,
        _: Token,
        _: Token,
    ) {
        self.log.push(("end", "body"));
    }
    fn begin_enum_body(&mut self, _: Token) {
        self.log.push(("begin", "body"));
    }
    fn end_enum_body(&mut self, _: Token, _: Token) {
        self.log.push(("end", "body"));
    }
    fn handle_no_class_body(&mut self, _: Token) {
        self.log.push(("handle", "no_class_body"));
    }

    // —— statements (#19) ————————————————————————————————————————————————
    fn begin_block(&mut self, _: Token, _: crate::listener::BlockKind) {
        self.log.push(("begin", "block"));
    }
    fn begin_formal_parameters(&mut self, _: Token, _: crate::listener::MemberKind) {
        self.log.push(("begin", "formal_parameters"));
    }
    fn end_formal_parameters(
        &mut self,
        _: usize,
        _: Token,
        _: Token,
        _: crate::listener::MemberKind,
    ) {
        self.log.push(("end", "formal_parameters"));
    }
    fn begin_function_name(&mut self, _: Token) {
        self.log.push(("begin", "function_name"));
    }
    fn end_function_name(&mut self, _: Token, _: Token, _: bool) {
        self.log.push(("end", "function_name"));
    }
    fn end_block(&mut self, _: usize, _: Token, _: Token, _: crate::listener::BlockKind) {
        self.log.push(("end", "block"));
    }
    fn begin_block_function_body(&mut self, _: Token) {
        self.log.push(("begin", "block_function_body"));
    }
    fn end_block_function_body(&mut self, _: usize, _: Token, _: Token) {
        self.log.push(("end", "block_function_body"));
    }
    fn handle_empty_function_body(&mut self, _: Token) {
        self.log.push(("handle", "empty_function_body"));
    }
    fn handle_expression_function_body(&mut self, _: Token, _: Option<Token>) {
        self.log.push(("handle", "expression_function_body"));
    }
    fn handle_expression_statement(&mut self, _: Token, _: Token) {
        self.log.push(("handle", "expression_statement"));
    }
    fn handle_empty_statement(&mut self, _: Token) {
        self.log.push(("handle", "empty_statement"));
    }
    fn begin_if_statement(&mut self, _: Token) {
        self.log.push(("begin", "if_statement"));
    }
    fn end_if_statement(&mut self, _: Token, _: Option<Token>, _: Token) {
        self.log.push(("end", "if_statement"));
    }
    fn begin_for_statement(&mut self, _: Token) {
        self.log.push(("begin", "for_statement"));
    }
    fn end_for_statement(&mut self, _: Token) {
        self.log.push(("end", "for_statement"));
    }
    fn end_for_in(&mut self, _: Token) {
        self.log.push(("end", "for_in"));
    }
    fn begin_while_statement(&mut self, _: Token) {
        self.log.push(("begin", "while_statement"));
    }
    fn end_while_statement(&mut self, _: Token, _: Token) {
        self.log.push(("end", "while_statement"));
    }
    fn begin_do_while_statement(&mut self, _: Token) {
        self.log.push(("begin", "do_while_statement"));
    }
    fn end_do_while_statement(&mut self, _: Token, _: Token, _: Token) {
        self.log.push(("end", "do_while_statement"));
    }
    fn begin_return_statement(&mut self, _: Token) {
        self.log.push(("begin", "return_statement"));
    }
    fn end_return_statement(&mut self, _: bool, _: Token, _: Token) {
        self.log.push(("end", "return_statement"));
    }
    fn begin_yield_statement(&mut self, _: Token) {
        self.log.push(("begin", "yield_statement"));
    }
    fn end_yield_statement(&mut self, _: Token, _: Option<Token>, _: Token) {
        self.log.push(("end", "yield_statement"));
    }
    /// The invalid yield completes the statement event pair (official
    /// `endInvalidYieldStatement`; the error code is resolution-domain).
    fn end_invalid_yield_statement(&mut self, _: Token, _: Option<Token>, _: Token) {
        self.log.push(("end", "yield_statement"));
    }
    fn begin_labeled_statement(&mut self, _: Token, _: usize) {
        self.log.push(("begin", "labeled_statement"));
    }
    fn end_labeled_statement(&mut self, _: usize) {
        self.log.push(("end", "labeled_statement"));
    }
    fn begin_local_function_declaration(&mut self, _: Token) {
        self.log.push(("begin", "local_function_declaration"));
    }
    fn end_local_function_declaration(&mut self, _: Token) {
        self.log.push(("end", "local_function_declaration"));
    }
    fn begin_variables_declaration(&mut self, _: Token, _: Option<Token>, _: Option<Token>) {
        self.log.push(("begin", "variables_declaration"));
    }
    fn end_variables_declaration(&mut self, _: usize, _: Option<Token>) {
        self.log.push(("end", "variables_declaration"));
    }
    fn begin_initialized_identifier(&mut self, _: Token) {
        self.log.push(("begin", "initialized_identifier"));
    }
    fn end_initialized_identifier(&mut self, _: Token) {
        self.log.push(("end", "initialized_identifier"));
    }
    fn begin_variable_initializer(&mut self, _: Token) {
        self.log.push(("begin", "variable_initializer"));
    }
    fn end_variable_initializer(&mut self, _: Token) {
        self.log.push(("end", "variable_initializer"));
    }
    fn begin_try_statement(&mut self, _: Token) {
        self.log.push(("begin", "try_statement"));
    }
    fn end_try_statement(&mut self, _: usize, _: Token, _: Option<Token>, _: Token) {
        self.log.push(("end", "try_statement"));
    }
    fn begin_catch_clause(&mut self, _: Token) {
        self.log.push(("begin", "catch_clause"));
    }
    fn end_catch_clause(&mut self, _: Token) {
        self.log.push(("end", "catch_clause"));
    }
    fn begin_switch_statement(&mut self, _: Token) {
        self.log.push(("begin", "switch_statement"));
    }
    fn end_switch_statement(&mut self, _: Token, _: Token) {
        self.log.push(("end", "switch_statement"));
    }
    fn begin_switch_block(&mut self, _: Token) {
        self.log.push(("begin", "switch_block"));
    }
    fn end_switch_block(&mut self, _: usize, _: Token, _: Token) {
        self.log.push(("end", "switch_block"));
    }
    fn begin_case_expression(&mut self, _: Token) {
        self.log.push(("begin", "case_expression"));
    }
    fn end_case_expression(&mut self, _: Token, _: Option<Token>, _: Token) {
        self.log.push(("end", "case_expression"));
    }
    fn begin_switch_case(&mut self, _: usize, _: usize, _: Token) {
        self.log.push(("begin", "switch_case"));
    }
    fn end_switch_case(
        &mut self,
        _: usize,
        _: usize,
        _: Option<Token>,
        _: Option<Token>,
        _: usize,
        _: Token,
        _: Token,
    ) {
        self.log.push(("end", "switch_case"));
    }
    fn handle_break_statement(&mut self, _: bool, _: Token, _: Token) {
        self.log.push(("handle", "break_statement"));
    }
    fn handle_continue_statement(&mut self, _: bool, _: Token, _: Token) {
        self.log.push(("handle", "continue_statement"));
    }
    fn begin_rethrow_statement(&mut self, _: Token) {
        self.log.push(("begin", "rethrow_statement"));
    }
    fn end_rethrow_statement(&mut self, _: Token, _: Token) {
        self.log.push(("end", "rethrow_statement"));
    }
    fn begin_assert(&mut self, _: Token, _: crate::listener::AssertKind) {
        self.log.push(("begin", "assert"));
    }
    fn end_assert(
        &mut self,
        _: Token,
        _: crate::listener::AssertKind,
        _: Token,
        _: Option<Token>,
        _: Token,
    ) {
        self.log.push(("end", "assert"));
    }
    fn handle_pattern_variable_declaration_statement(&mut self, _: Token, _: Token, _: Token) {
        self.log
            .push(("handle", "pattern_variable_declaration_statement"));
    }

    // —— directives (#300) ———————————————————————————————————————————————
    fn begin_library_name(&mut self, _: Token) {
        self.log.push(("begin", "library_name"));
    }
    fn end_library_name(&mut self, _: Token, _: Token, _: bool) {
        self.log.push(("end", "library_name"));
    }
    fn begin_library_augmentation(&mut self, _: Token, _: Token) {
        self.log.push(("begin", "library_augmentation"));
    }
    fn end_library_augmentation(&mut self, _: Token, _: Token, _: Token) {
        self.log.push(("end", "library_augmentation"));
    }
    fn begin_import(&mut self, _: Token) {
        self.log.push(("begin", "import"));
    }
    fn end_import(&mut self, _: Token, _: Option<Token>) {
        self.log.push(("end", "import"));
    }
    fn handle_import_prefix(&mut self, _: Option<Token>, _: Option<Token>) {
        self.log.push(("handle", "import_prefix"));
    }
    fn handle_recover_import(&mut self, _: Option<Token>) {
        self.log.push(("handle", "recover_import"));
    }
    fn begin_conditional_uris(&mut self, _: Token) {
        self.log.push(("begin", "conditional_uris"));
    }
    fn end_conditional_uris(&mut self, _: usize) {
        self.log.push(("end", "conditional_uris"));
    }
    fn begin_conditional_uri(&mut self, _: Token) {
        self.log.push(("begin", "conditional_uri"));
    }
    fn end_conditional_uri(&mut self, _: Token, _: Token, _: Option<Token>) {
        self.log.push(("end", "conditional_uri"));
    }
    fn handle_dotted_name(&mut self, _: usize, _: Token) {
        self.log.push(("handle", "dotted_name"));
    }
    fn begin_export(&mut self, _: Token) {
        self.log.push(("begin", "export"));
    }
    fn end_export(&mut self, _: Token, _: Token) {
        self.log.push(("end", "export"));
    }
    fn begin_combinators(&mut self, _: Token) {
        self.log.push(("begin", "combinators"));
    }
    fn end_combinators(&mut self, _: usize) {
        self.log.push(("end", "combinators"));
    }
    fn begin_show(&mut self, _: Token) {
        self.log.push(("begin", "show"));
    }
    fn end_show(&mut self, _: Token) {
        self.log.push(("end", "show"));
    }
    fn begin_hide(&mut self, _: Token) {
        self.log.push(("begin", "hide"));
    }
    fn end_hide(&mut self, _: Token) {
        self.log.push(("end", "hide"));
    }
    fn handle_identifier_list(&mut self, _: usize) {
        self.log.push(("handle", "identifier_list"));
    }
    fn begin_part(&mut self, _: Token) {
        self.log.push(("begin", "part"));
    }
    fn end_part(&mut self, _: Token, _: Token) {
        self.log.push(("end", "part"));
    }
    fn begin_part_of(&mut self, _: Token) {
        self.log.push(("begin", "part_of"));
    }
    fn end_part_of(&mut self, _: Token, _: Token, _: Token, _: bool) {
        self.log.push(("end", "part_of"));
    }

    // —— clause events (#460) ——————————————————————————————————————————————
    fn handle_implements(&mut self, _: Option<Token>, _: usize) {
        self.log.push(("handle", "implements"));
    }
}

/// Builds a parser over `source` with a fresh [`Collector`].
pub(crate) fn setup(source: &'static str) -> Parser<'static, 'static, Collector> {
    let (table, text) = scan_ok(source);
    Parser::new(text, table, latest_features(), Collector::default())
}
