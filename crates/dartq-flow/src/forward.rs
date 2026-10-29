//! Generated forwarding impl: every parser listener event reaches the
//! [`Flow`] engine through the shared handle. Regenerate from
//! `dartq-parser/src/listener.rs` when the trait grows. The block/assert
//! events forward their parser `BlockKind`/`AssertKind` (re-exported since
//! the parser's re-export list gained them); `record.rs` narrows the block
//! kind to the recorded subset.

use std::cell::RefCell;
use std::rc::Rc;

use dartq_parser::TokenSpan as Token;
use dartq_parser::{
    AssertKind, BlockKind, ConstructorReferenceContext, DeclarationHeaderKind, DeclarationKind,
    ExperimentalFlag, FormalParameterKind, IdentifierContext, MemberKind, Message, ParserListener,
};

use crate::record::Flow;

/// Shared handle to a [`Flow`] engine (orphan-rule-friendly newtype).
pub struct SharedFlow<'t>(pub Rc<RefCell<Flow<'t>>>);

impl ParserListener for SharedFlow<'_> {
    fn begin_compilation_unit(&mut self, token: Token) {
        self.0.borrow_mut().begin_compilation_unit(token);
    }

    fn end_compilation_unit(&mut self, count: usize, token: Token) {
        self.0.borrow_mut().end_compilation_unit(count, token);
    }

    fn begin_library_name(&mut self, token: Token) {
        self.0.borrow_mut().begin_library_name(token);
    }

    fn end_library_name(&mut self, library_keyword: Token, semicolon: Token, has_name: bool) {
        self.0
            .borrow_mut()
            .end_library_name(library_keyword, semicolon, has_name);
    }

    fn begin_library_augmentation(&mut self, augment_kw: Token, library_keyword: Token) {
        self.0
            .borrow_mut()
            .begin_library_augmentation(augment_kw, library_keyword);
    }

    fn end_library_augmentation(
        &mut self,
        augment_kw: Token,
        library_keyword: Token,
        semicolon: Token,
    ) {
        self.0
            .borrow_mut()
            .end_library_augmentation(augment_kw, library_keyword, semicolon);
    }

    fn begin_import(&mut self, import_keyword: Token) {
        self.0.borrow_mut().begin_import(import_keyword);
    }

    fn end_import(&mut self, import_keyword: Token, semicolon: Option<Token>) {
        self.0.borrow_mut().end_import(import_keyword, semicolon);
    }

    fn handle_import_prefix(&mut self, deferred_kw: Option<Token>, as_kw: Option<Token>) {
        self.0.borrow_mut().handle_import_prefix(deferred_kw, as_kw);
    }

    fn handle_recover_import(&mut self, semicolon: Option<Token>) {
        self.0.borrow_mut().handle_recover_import(semicolon);
    }

    fn begin_conditional_uris(&mut self, token: Token) {
        self.0.borrow_mut().begin_conditional_uris(token);
    }

    fn end_conditional_uris(&mut self, count: usize) {
        self.0.borrow_mut().end_conditional_uris(count);
    }

    fn begin_conditional_uri(&mut self, if_kw: Token) {
        self.0.borrow_mut().begin_conditional_uri(if_kw);
    }

    fn end_conditional_uri(&mut self, if_kw: Token, left_paren: Token, equal_sign: Option<Token>) {
        self.0
            .borrow_mut()
            .end_conditional_uri(if_kw, left_paren, equal_sign);
    }

    fn handle_dotted_name(&mut self, count: usize, first_identifier: Token) {
        self.0
            .borrow_mut()
            .handle_dotted_name(count, first_identifier);
    }

    fn begin_export(&mut self, token: Token) {
        self.0.borrow_mut().begin_export(token);
    }

    fn end_export(&mut self, export_keyword: Token, semicolon: Token) {
        self.0.borrow_mut().end_export(export_keyword, semicolon);
    }

    fn begin_combinators(&mut self, token: Token) {
        self.0.borrow_mut().begin_combinators(token);
    }

    fn end_combinators(&mut self, count: usize) {
        self.0.borrow_mut().end_combinators(count);
    }

    fn begin_show(&mut self, show_keyword: Token) {
        self.0.borrow_mut().begin_show(show_keyword);
    }

    fn end_show(&mut self, show_keyword: Token) {
        self.0.borrow_mut().end_show(show_keyword);
    }

    fn begin_hide(&mut self, hide_keyword: Token) {
        self.0.borrow_mut().begin_hide(hide_keyword);
    }

    fn end_hide(&mut self, hide_keyword: Token) {
        self.0.borrow_mut().end_hide(hide_keyword);
    }

    fn handle_identifier_list(&mut self, count: usize) {
        self.0.borrow_mut().handle_identifier_list(count);
    }

    fn begin_part(&mut self, token: Token) {
        self.0.borrow_mut().begin_part(token);
    }

    fn end_part(&mut self, part_keyword: Token, semicolon: Token) {
        self.0.borrow_mut().end_part(part_keyword, semicolon);
    }

    fn begin_part_of(&mut self, token: Token) {
        self.0.borrow_mut().begin_part_of(token);
    }

    fn end_part_of(&mut self, part_keyword: Token, of_kw: Token, semicolon: Token, has_name: bool) {
        self.0
            .borrow_mut()
            .end_part_of(part_keyword, of_kw, semicolon, has_name);
    }

    fn begin_metadata_star(&mut self, token: Token) {
        self.0.borrow_mut().begin_metadata_star(token);
    }

    fn end_metadata_star(&mut self, count: usize) {
        self.0.borrow_mut().end_metadata_star(count);
    }

    fn begin_metadata(&mut self, token: Token) {
        self.0.borrow_mut().begin_metadata(token);
    }

    fn end_metadata(&mut self, begin: Token, period_before_name: Option<Token>, end: Token) {
        self.0
            .borrow_mut()
            .end_metadata(begin, period_before_name, end);
    }

    fn handle_type(&mut self, name: Token, question: Option<Token>) {
        self.0.borrow_mut().handle_type(name, question);
    }

    fn handle_no_type(&mut self, last_consumed: Token) {
        self.0.borrow_mut().handle_no_type(last_consumed);
    }

    fn handle_qualified(&mut self, period: Token) {
        self.0.borrow_mut().handle_qualified(period);
    }

    fn handle_identifier(&mut self, token: Token, context: IdentifierContext) {
        self.0.borrow_mut().handle_identifier(token, context);
    }

    fn begin_type_arguments(&mut self, token: Token) {
        self.0.borrow_mut().begin_type_arguments(token);
    }

    fn end_type_arguments(&mut self, count: usize, begin: Token, end: Token) {
        self.0.borrow_mut().end_type_arguments(count, begin, end);
    }

    fn handle_no_type_arguments(&mut self, token: Token) {
        self.0.borrow_mut().handle_no_type_arguments(token);
    }

    fn begin_type_variables(&mut self, token: Token) {
        self.0.borrow_mut().begin_type_variables(token);
    }

    fn end_type_variables(&mut self, begin: Token, end: Token) {
        self.0.borrow_mut().end_type_variables(begin, end);
    }

    fn handle_no_type_variables(&mut self, token: Token) {
        self.0.borrow_mut().handle_no_type_variables(token);
    }

    fn handle_type_variables_defined(&mut self, token: Token, count: usize) {
        self.0
            .borrow_mut()
            .handle_type_variables_defined(token, count);
    }

    fn begin_type_variable(&mut self, token: Token) {
        self.0.borrow_mut().begin_type_variable(token);
    }

    fn end_type_variable(
        &mut self,
        token: Token,
        index: usize,
        extends_or_super: Option<Token>,
        variance: Option<Token>,
    ) {
        self.0
            .borrow_mut()
            .end_type_variable(token, index, extends_or_super, variance);
    }

    fn begin_record_type(&mut self, left_paren: Token) {
        self.0.borrow_mut().begin_record_type(left_paren);
    }

    fn end_record_type(
        &mut self,
        left_paren: Token,
        question: Option<Token>,
        count: usize,
        has_named_fields: bool,
    ) {
        self.0
            .borrow_mut()
            .end_record_type(left_paren, question, count, has_named_fields);
    }

    fn begin_record_type_entry(&mut self, first: Token) {
        self.0.borrow_mut().begin_record_type_entry(first);
    }

    fn end_record_type_entry(&mut self, last: Token) {
        self.0.borrow_mut().end_record_type_entry(last);
    }

    fn handle_no_name(&mut self, token: Token) {
        self.0.borrow_mut().handle_no_name(token);
    }

    fn begin_record_type_named_fields(&mut self, left_brace: Token) {
        self.0
            .borrow_mut()
            .begin_record_type_named_fields(left_brace);
    }

    fn end_record_type_named_fields(&mut self, count: usize, left_brace: Token) {
        self.0
            .borrow_mut()
            .end_record_type_named_fields(count, left_brace);
    }

    fn begin_function_type(&mut self, begin: Token) {
        self.0.borrow_mut().begin_function_type(begin);
    }

    fn end_function_type(&mut self, function: Token, question: Option<Token>) {
        self.0.borrow_mut().end_function_type(function, question);
    }

    fn handle_void_keyword(&mut self, token: Token) {
        self.0.borrow_mut().handle_void_keyword(token);
    }

    fn handle_void_keyword_with_type_arguments(&mut self, token: Token) {
        self.0
            .borrow_mut()
            .handle_void_keyword_with_type_arguments(token);
    }

    fn handle_literal_int(&mut self, token: Token) {
        self.0.borrow_mut().handle_literal_int(token);
    }

    fn handle_literal_int_with_separators(&mut self, token: Token) {
        self.0
            .borrow_mut()
            .handle_literal_int_with_separators(token);
    }

    fn handle_literal_double(&mut self, token: Token) {
        self.0.borrow_mut().handle_literal_double(token);
    }

    fn handle_literal_double_with_separators(&mut self, token: Token) {
        self.0
            .borrow_mut()
            .handle_literal_double_with_separators(token);
    }

    fn handle_literal_bool(&mut self, token: Token) {
        self.0.borrow_mut().handle_literal_bool(token);
    }

    fn handle_literal_null(&mut self, token: Token) {
        self.0.borrow_mut().handle_literal_null(token);
    }

    fn begin_literal_string(&mut self, token: Token) {
        self.0.borrow_mut().begin_literal_string(token);
    }

    fn handle_interpolation_expression(&mut self, left: Token, right: Option<Token>) {
        self.0
            .borrow_mut()
            .handle_interpolation_expression(left, right);
    }

    fn end_literal_string(&mut self, interpolation_count: usize, end: Token) {
        self.0
            .borrow_mut()
            .end_literal_string(interpolation_count, end);
    }

    fn handle_adjacent_string_literals(&mut self, start: Token, literal_count: usize) {
        self.0
            .borrow_mut()
            .handle_adjacent_string_literals(start, literal_count);
    }

    fn begin_literal_symbol(&mut self, token: Token) {
        self.0.borrow_mut().begin_literal_symbol(token);
    }

    fn handle_symbol_void(&mut self, token: Token) {
        self.0.borrow_mut().handle_symbol_void(token);
    }

    fn end_literal_symbol(&mut self, hash: Token, identifier_count: usize) {
        self.0
            .borrow_mut()
            .end_literal_symbol(hash, identifier_count);
    }

    fn handle_operator(&mut self, token: Token) {
        self.0.borrow_mut().handle_operator(token);
    }

    fn handle_this_expression(&mut self, token: Token, context: IdentifierContext) {
        self.0.borrow_mut().handle_this_expression(token, context);
    }

    fn handle_super_expression(&mut self, token: Token, context: IdentifierContext) {
        self.0.borrow_mut().handle_super_expression(token, context);
    }

    fn handle_unary_prefix_expression(&mut self, operator: Token) {
        self.0.borrow_mut().handle_unary_prefix_expression(operator);
    }

    fn handle_non_null_assert_expression(&mut self, bang: Token) {
        self.0.borrow_mut().handle_non_null_assert_expression(bang);
    }

    fn begin_binary_expression(&mut self, operator: Token) {
        self.0.borrow_mut().begin_binary_expression(operator);
    }

    fn end_binary_expression(&mut self, operator: Token, end: Token) {
        self.0.borrow_mut().end_binary_expression(operator, end);
    }

    fn handle_binary_rhs_end(&mut self, operator: Token, rhs_last: Token) {
        self.0.borrow_mut().handle_binary_rhs_end(operator, rhs_last);
    }

    fn begin_const_expression(&mut self, const_keyword: Token) {
        self.0.borrow_mut().begin_const_expression(const_keyword);
    }

    fn end_const_expression(&mut self, token: Token) {
        self.0.borrow_mut().end_const_expression(token);
    }

    fn begin_const_literal(&mut self, token: Token) {
        self.0.borrow_mut().begin_const_literal(token);
    }

    fn end_const_literal(&mut self, end: Token) {
        self.0.borrow_mut().end_const_literal(end);
    }

    fn handle_literal_list(
        &mut self,
        count: usize,
        left: Token,
        const_keyword: Option<Token>,
        right: Token,
    ) {
        self.0
            .borrow_mut()
            .handle_literal_list(count, left, const_keyword, right);
    }

    fn handle_literal_set_or_map(
        &mut self,
        count: usize,
        left: Token,
        const_keyword: Option<Token>,
        right: Token,
        has_set_entry: bool,
    ) {
        self.0.borrow_mut().handle_literal_set_or_map(
            count,
            left,
            const_keyword,
            right,
            has_set_entry,
        );
    }

    fn handle_literal_map_entry(
        &mut self,
        colon: Token,
        end: Token,
        null_aware_key: Option<Token>,
        null_aware_value: Option<Token>,
    ) {
        self.0
            .borrow_mut()
            .handle_literal_map_entry(colon, end, null_aware_key, null_aware_value);
    }

    fn begin_new_expression(&mut self, token: Token) {
        self.0.borrow_mut().begin_new_expression(token);
    }

    fn end_new_expression(&mut self, token: Token) {
        self.0.borrow_mut().end_new_expression(token);
    }

    fn begin_parenthesized_expression_or_record_literal(&mut self, token: Token) {
        self.0
            .borrow_mut()
            .begin_parenthesized_expression_or_record_literal(token);
    }

    fn end_parenthesized_expression(&mut self, token: Token) {
        self.0.borrow_mut().end_parenthesized_expression(token);
    }

    fn end_record_literal(&mut self, token: Token, count: usize, const_keyword: Option<Token>) {
        self.0
            .borrow_mut()
            .end_record_literal(token, count, const_keyword);
    }

    fn handle_named_record_field(&mut self, colon: Token) {
        self.0.borrow_mut().handle_named_record_field(colon);
    }

    fn handle_positional_record_field(&mut self, token: Token) {
        self.0.borrow_mut().handle_positional_record_field(token);
    }

    fn begin_arguments(&mut self, token: Token) {
        self.0.borrow_mut().begin_arguments(token);
    }

    fn end_arguments(&mut self, count: usize, begin: Token, end: Token) {
        self.0.borrow_mut().end_arguments(count, begin, end);
    }

    fn handle_no_arguments(&mut self, token: Token) {
        self.0.borrow_mut().handle_no_arguments(token);
    }

    fn handle_named_argument(&mut self, colon: Token) {
        self.0.borrow_mut().handle_named_argument(colon);
    }

    fn handle_positional_argument(&mut self, token: Token) {
        self.0.borrow_mut().handle_positional_argument(token);
    }

    fn handle_send(&mut self, begin: Token, end: Token) {
        self.0.borrow_mut().handle_send(begin, end);
    }

    fn handle_send_without_arguments(&mut self, begin: Token, end: Token, next: Token) {
        self.0
            .borrow_mut()
            .handle_send_without_arguments(begin, end, next);
    }

    fn handle_dot_access(&mut self, token: Token, end: Token, is_null_aware: bool) {
        self.0
            .borrow_mut()
            .handle_dot_access(token, end, is_null_aware);
    }

    fn handle_null_aware_index_operand_begin(&mut self, question: Token) {
        self.0
            .borrow_mut()
            .handle_null_aware_index_operand_begin(question);
    }

    fn handle_null_aware_member_begin(&mut self, question: Token) {
        self.0.borrow_mut().handle_null_aware_member_begin(question);
    }

    fn handle_indexed_expression(&mut self, question: Option<Token>, open: Token, close: Token) {
        self.0
            .borrow_mut()
            .handle_indexed_expression(question, open, close);
    }

    fn begin_constructor_reference(&mut self, start: Token) {
        self.0.borrow_mut().begin_constructor_reference(start);
    }

    fn handle_no_constructor_reference_continuation_after_type_arguments(&mut self, token: Token) {
        self.0
            .borrow_mut()
            .handle_no_constructor_reference_continuation_after_type_arguments(token);
    }

    fn end_constructor_reference(
        &mut self,
        start: Token,
        period_before_name: Option<Token>,
        end: Token,
        context: ConstructorReferenceContext,
    ) {
        self.0
            .borrow_mut()
            .end_constructor_reference(start, period_before_name, end, context);
    }

    fn begin_await_expression(&mut self, token: Token) {
        self.0.borrow_mut().begin_await_expression(token);
    }

    fn end_await_expression(&mut self, begin: Token, end: Token) {
        self.0.borrow_mut().end_await_expression(begin, end);
    }

    fn end_invalid_await_expression(&mut self, begin: Token, end: Token) {
        self.0.borrow_mut().end_invalid_await_expression(begin, end);
    }

    fn handle_throw_expression(&mut self, throw: Token, end: Token) {
        self.0.borrow_mut().handle_throw_expression(throw, end);
    }

    fn handle_assignment_expression(&mut self, operator: Token, end: Token) {
        self.0
            .borrow_mut()
            .handle_assignment_expression(operator, end);
    }

    fn begin_cascade(&mut self, operator: Token) {
        self.0.borrow_mut().begin_cascade(operator);
    }

    fn handle_cascade_access(&mut self, operator: Token, end: Token, is_null_aware: bool) {
        self.0
            .borrow_mut()
            .handle_cascade_access(operator, end, is_null_aware);
    }

    fn end_cascade(&mut self) {
        self.0.borrow_mut().end_cascade();
    }

    fn begin_conditional_expression(&mut self, question: Token) {
        self.0.borrow_mut().begin_conditional_expression(question);
    }

    fn end_conditional_expression_then_arm(
        &mut self,
        question: Token,
        then_begin: Token,
        colon: Token,
        then_end: Token,
    ) {
        self.0
            .borrow_mut()
            .end_conditional_expression_then_arm(question, then_begin, colon, then_end);
    }

    fn handle_conditional_expression_colon(&mut self) {
        self.0.borrow_mut().handle_conditional_expression_colon();
    }

    fn end_conditional_expression_else_arm(&mut self, else_last: Token) {
        self.0
            .borrow_mut()
            .end_conditional_expression_else_arm(else_last);
    }

    fn end_conditional_expression(&mut self, question: Token, colon: Token, end: Token) {
        self.0
            .borrow_mut()
            .end_conditional_expression(question, colon, end);
    }

    fn handle_unary_prefix_assignment_expression(&mut self, operator: Token) {
        self.0
            .borrow_mut()
            .handle_unary_prefix_assignment_expression(operator);
    }

    fn handle_unary_postfix_assignment_expression(&mut self, operator: Token) {
        self.0
            .borrow_mut()
            .handle_unary_postfix_assignment_expression(operator);
    }

    fn begin_function_expression(&mut self, token: Token) {
        self.0.borrow_mut().begin_function_expression(token);
    }

    fn end_function_expression(&mut self, begin: Token, end: Token) {
        self.0.borrow_mut().end_function_expression(begin, end);
    }

    fn begin_named_function_expression(&mut self, token: Token) {
        self.0.borrow_mut().begin_named_function_expression(token);
    }

    fn end_named_function_expression(&mut self, end: Token) {
        self.0.borrow_mut().end_named_function_expression(end);
    }

    fn begin_implicit_creation_expression(&mut self, token: Token) {
        self.0
            .borrow_mut()
            .begin_implicit_creation_expression(token);
    }

    fn end_implicit_creation_expression(&mut self, begin: Token, open_angle: Token) {
        self.0
            .borrow_mut()
            .end_implicit_creation_expression(begin, open_angle);
    }

    fn handle_augment_super_expression(
        &mut self,
        augment: Token,
        super_kw: Token,
        context: IdentifierContext,
    ) {
        self.0
            .borrow_mut()
            .handle_augment_super_expression(augment, super_kw, context);
    }

    fn handle_dot_shorthand_head(&mut self, dot: Token) {
        self.0.borrow_mut().handle_dot_shorthand_head(dot);
    }

    fn handle_dot_shorthand_context(&mut self, dot: Token) {
        self.0.borrow_mut().handle_dot_shorthand_context(dot);
    }

    fn begin_const_dot_shorthand(&mut self, const_keyword: Token) {
        self.0.borrow_mut().begin_const_dot_shorthand(const_keyword);
    }

    fn end_const_dot_shorthand(&mut self, const_keyword: Token) {
        self.0.borrow_mut().end_const_dot_shorthand(const_keyword);
    }

    fn handle_invalid_type_arguments(&mut self, token: Token) {
        self.0.borrow_mut().handle_invalid_type_arguments(token);
    }

    fn handle_pattern_assignment(&mut self, equals: Token) {
        self.0.borrow_mut().handle_pattern_assignment(equals);
    }

    fn begin_is_operator_type(&mut self, operator: Token) {
        self.0.borrow_mut().begin_is_operator_type(operator);
    }

    fn end_is_operator_type(&mut self, operator: Token) {
        self.0.borrow_mut().end_is_operator_type(operator);
    }

    fn handle_is_operator(&mut self, is_operator: Token, not: Option<Token>) {
        self.0.borrow_mut().handle_is_operator(is_operator, not);
    }

    fn begin_as_operator_type(&mut self, operator: Token) {
        self.0.borrow_mut().begin_as_operator_type(operator);
    }

    fn end_as_operator_type(&mut self, operator: Token) {
        self.0.borrow_mut().end_as_operator_type(operator);
    }

    fn handle_as_operator(&mut self, operator: Token) {
        self.0.borrow_mut().handle_as_operator(operator);
    }

    fn handle_type_argument_application(&mut self, open_angle: Token) {
        self.0
            .borrow_mut()
            .handle_type_argument_application(open_angle);
    }

    fn begin_pattern(&mut self, token: Token) {
        self.0.borrow_mut().begin_pattern(token);
    }

    fn end_pattern(&mut self, token: Token) {
        self.0.borrow_mut().end_pattern(token);
    }

    fn begin_constant_pattern(&mut self, const_keyword: Option<Token>) {
        self.0.borrow_mut().begin_constant_pattern(const_keyword);
    }

    fn end_constant_pattern(&mut self, const_keyword: Option<Token>) {
        self.0.borrow_mut().end_constant_pattern(const_keyword);
    }

    fn handle_relational_pattern(&mut self, operator: Token) {
        self.0.borrow_mut().handle_relational_pattern(operator);
    }

    fn handle_cast_pattern(&mut self, operator: Token) {
        self.0.borrow_mut().handle_cast_pattern(operator);
    }

    fn handle_null_assert_pattern(&mut self, bang: Token) {
        self.0.borrow_mut().handle_null_assert_pattern(bang);
    }

    fn handle_null_check_pattern(&mut self, question: Token) {
        self.0.borrow_mut().handle_null_check_pattern(question);
    }

    fn begin_binary_pattern(&mut self, operator: Token) {
        self.0.borrow_mut().begin_binary_pattern(operator);
    }

    fn end_binary_pattern(&mut self, operator: Token) {
        self.0.borrow_mut().end_binary_pattern(operator);
    }

    fn handle_list_pattern(&mut self, count: usize, left: Token, right: Token) {
        self.0.borrow_mut().handle_list_pattern(count, left, right);
    }

    fn handle_map_pattern(&mut self, count: usize, left: Token, right: Token) {
        self.0.borrow_mut().handle_map_pattern(count, left, right);
    }

    fn begin_map_pattern_key(&mut self) {
        self.0.borrow_mut().begin_map_pattern_key();
    }

    fn handle_map_pattern_entry(&mut self, colon: Token, end: Token) {
        self.0.borrow_mut().handle_map_pattern_entry(colon, end);
    }

    fn handle_rest_pattern(&mut self, dots: Token, has_sub_pattern: bool) {
        self.0
            .borrow_mut()
            .handle_rest_pattern(dots, has_sub_pattern);
    }

    fn handle_record_pattern(&mut self, left_paren: Token, count: usize) {
        self.0.borrow_mut().handle_record_pattern(left_paren, count);
    }

    fn handle_parenthesized_pattern(&mut self, left_paren: Token) {
        self.0.borrow_mut().handle_parenthesized_pattern(left_paren);
    }

    fn handle_pattern_field(&mut self, colon: Option<Token>) {
        self.0.borrow_mut().handle_pattern_field(colon);
    }

    fn handle_object_pattern(
        &mut self,
        first_identifier: Token,
        dot: Option<Token>,
        second_identifier: Option<Token>,
    ) {
        self.0
            .borrow_mut()
            .handle_object_pattern(first_identifier, dot, second_identifier);
    }

    fn handle_object_pattern_fields(&mut self, count: usize, begin: Token, end: Token) {
        self.0
            .borrow_mut()
            .handle_object_pattern_fields(count, begin, end);
    }

    fn handle_wildcard_pattern(&mut self, keyword: Option<Token>, wildcard: Token) {
        self.0
            .borrow_mut()
            .handle_wildcard_pattern(keyword, wildcard);
    }

    fn handle_assigned_variable_pattern(&mut self, variable: Token) {
        self.0
            .borrow_mut()
            .handle_assigned_variable_pattern(variable);
    }

    fn handle_declared_variable_pattern(
        &mut self,
        keyword: Option<Token>,
        variable: Token,
        in_assignment_pattern: bool,
    ) {
        self.0.borrow_mut().handle_declared_variable_pattern(
            keyword,
            variable,
            in_assignment_pattern,
        );
    }

    fn begin_formal_parameters(&mut self, token: Token, kind: MemberKind) {
        self.0.borrow_mut().begin_formal_parameters(token, kind);
    }

    fn end_formal_parameters(&mut self, count: usize, begin: Token, end: Token, kind: MemberKind) {
        self.0
            .borrow_mut()
            .end_formal_parameters(count, begin, end, kind);
    }

    fn begin_optional_formal_parameters(&mut self, token: Token) {
        self.0.borrow_mut().begin_optional_formal_parameters(token);
    }

    fn end_optional_formal_parameters(
        &mut self,
        count: usize,
        begin: Token,
        end: Token,
        kind: MemberKind,
    ) {
        self.0
            .borrow_mut()
            .end_optional_formal_parameters(count, begin, end, kind);
    }

    fn begin_formal_parameter(
        &mut self,
        token: Token,
        kind: MemberKind,
        required: Option<Token>,
        covariant_kw: Option<Token>,
        var_final_or_const: Option<Token>,
    ) {
        self.0.borrow_mut().begin_formal_parameter(
            token,
            kind,
            required,
            covariant_kw,
            var_final_or_const,
        );
    }

    fn begin_function_typed_formal_parameter(&mut self, token: Token) {
        self.0
            .borrow_mut()
            .begin_function_typed_formal_parameter(token);
    }

    fn end_function_typed_formal_parameter(&mut self, token: Token, question: Option<Token>) {
        self.0
            .borrow_mut()
            .end_function_typed_formal_parameter(token, question);
    }

    fn begin_formal_parameter_default_value_expression(&mut self, token: Token) {
        self.0
            .borrow_mut()
            .begin_formal_parameter_default_value_expression(token);
    }

    fn end_formal_parameter_default_value_expression(&mut self, token: Token) {
        self.0
            .borrow_mut()
            .end_formal_parameter_default_value_expression(token);
    }

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
        member_kind: MemberKind,
    ) {
        self.0.borrow_mut().end_formal_parameter(
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
    }

    fn handle_valued_formal_parameter(
        &mut self,
        equals: Token,
        end: Token,
        kind: FormalParameterKind,
    ) {
        self.0
            .borrow_mut()
            .handle_valued_formal_parameter(equals, end, kind);
    }

    fn handle_formal_parameter_without_value(&mut self, token: Token) {
        self.0
            .borrow_mut()
            .handle_formal_parameter_without_value(token);
    }

    fn handle_no_formal_parameters(&mut self, token: Token, kind: MemberKind) {
        self.0.borrow_mut().handle_no_formal_parameters(token, kind);
    }

    fn end_top_level_declaration(&mut self, end: Token) {
        self.0.borrow_mut().end_top_level_declaration(end);
    }

    fn handle_invalid_top_level_declaration(&mut self, end: Token) {
        self.0
            .borrow_mut()
            .handle_invalid_top_level_declaration(end);
    }

    fn begin_uncategorized_top_level_declaration(&mut self, token: Token) {
        self.0
            .borrow_mut()
            .begin_uncategorized_top_level_declaration(token);
    }

    fn begin_top_level_member(&mut self, token: Token) {
        self.0.borrow_mut().begin_top_level_member(token);
    }

    fn begin_top_level_method(
        &mut self,
        last_consumed: Token,
        augment: Option<Token>,
        external: Option<Token>,
    ) {
        self.0
            .borrow_mut()
            .begin_top_level_method(last_consumed, augment, external);
    }

    fn end_top_level_method(&mut self, begin: Token, get_or_set: Option<Token>, end: Token) {
        self.0
            .borrow_mut()
            .end_top_level_method(begin, get_or_set, end);
    }

    fn end_top_level_fields(
        &mut self,
        augment_kw: Option<Token>,
        abstract_kw: Option<Token>,
        external_kw: Option<Token>,
        static_kw: Option<Token>,
        covariant_kw: Option<Token>,
        late_kw: Option<Token>,
        var_final_or_const: Option<Token>,
        count: usize,
        begin: Token,
        end: Token,
    ) {
        self.0.borrow_mut().end_top_level_fields(
            augment_kw,
            abstract_kw,
            external_kw,
            static_kw,
            covariant_kw,
            late_kw,
            var_final_or_const,
            count,
            begin,
            end,
        );
    }

    fn begin_typedef(&mut self, token: Token) {
        self.0.borrow_mut().begin_typedef(token);
    }

    fn end_typedef(
        &mut self,
        augment: Option<Token>,
        typedef: Token,
        equals: Option<Token>,
        end: Token,
    ) {
        self.0
            .borrow_mut()
            .end_typedef(augment, typedef, equals, end);
    }

    fn begin_member(&mut self) {
        self.0.borrow_mut().begin_member();
    }

    fn end_member(&mut self) {
        self.0.borrow_mut().end_member();
    }

    fn handle_invalid_member(&mut self, end: Token) {
        self.0.borrow_mut().handle_invalid_member(end);
    }

    fn begin_fields(
        &mut self,
        kind: DeclarationKind,
        augment_kw: Option<Token>,
        abstract_kw: Option<Token>,
        external_kw: Option<Token>,
        static_kw: Option<Token>,
        covariant_kw: Option<Token>,
        late_kw: Option<Token>,
        var_final_or_const: Option<Token>,
        last_consumed: Token,
    ) {
        self.0.borrow_mut().begin_fields(
            kind,
            augment_kw,
            abstract_kw,
            external_kw,
            static_kw,
            covariant_kw,
            late_kw,
            var_final_or_const,
            last_consumed,
        );
    }

    fn end_fields(
        &mut self,
        kind: DeclarationKind,
        abstract_kw: Option<Token>,
        augment_kw: Option<Token>,
        external_kw: Option<Token>,
        static_kw: Option<Token>,
        covariant_kw: Option<Token>,
        late_kw: Option<Token>,
        var_final_or_const: Option<Token>,
        count: usize,
        begin: Token,
        end: Token,
    ) {
        self.0.borrow_mut().end_fields(
            kind,
            abstract_kw,
            augment_kw,
            external_kw,
            static_kw,
            covariant_kw,
            late_kw,
            var_final_or_const,
            count,
            begin,
            end,
        );
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
        self.0.borrow_mut().begin_constructor(
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
        self.0.borrow_mut().end_constructor(
            kind,
            begin,
            new_kw,
            begin_param,
            begin_initializers,
            end,
        );
    }

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
        self.0.borrow_mut().begin_method(
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
        self.0.borrow_mut().end_method(
            kind,
            get_or_set,
            begin,
            begin_param,
            begin_initializers,
            end,
        );
    }

    fn begin_factory(
        &mut self,
        kind: DeclarationKind,
        last_consumed: Token,
        augment_kw: Option<Token>,
        external_kw: Option<Token>,
        const_keyword: Option<Token>,
    ) {
        self.0.borrow_mut().begin_factory(
            kind,
            last_consumed,
            augment_kw,
            external_kw,
            const_keyword,
        );
    }

    fn end_factory(&mut self, kind: DeclarationKind, begin: Token, factory: Token, end: Token) {
        self.0.borrow_mut().end_factory(kind, begin, factory, end);
    }

    fn handle_operator_name(&mut self, operator: Token, name: Token) {
        self.0.borrow_mut().handle_operator_name(operator, name);
    }

    fn handle_invalid_operator_name(&mut self, operator: Token, name: Token) {
        self.0
            .borrow_mut()
            .handle_invalid_operator_name(operator, name);
    }

    fn handle_no_identifier(&mut self, token: Token, context: IdentifierContext) {
        self.0.borrow_mut().handle_no_identifier(token, context);
    }

    fn begin_field_initializer(&mut self, token: Token) {
        self.0.borrow_mut().begin_field_initializer(token);
    }

    fn end_field_initializer(&mut self, assignment: Token, end: Token) {
        self.0.borrow_mut().end_field_initializer(assignment, end);
    }

    fn handle_no_field_initializer(&mut self, token: Token) {
        self.0.borrow_mut().handle_no_field_initializer(token);
    }

    fn begin_initializers(&mut self, token: Token) {
        self.0.borrow_mut().begin_initializers(token);
    }

    fn end_initializers(&mut self, count: usize, begin: Token, end: Token) {
        self.0.borrow_mut().end_initializers(count, begin, end);
    }

    fn handle_no_initializers(&mut self) {
        self.0.borrow_mut().handle_no_initializers();
    }

    fn handle_async_modifier(&mut self, async_kw: Option<Token>, star: Option<Token>) {
        self.0.borrow_mut().handle_async_modifier(async_kw, star);
    }

    fn handle_no_function_body(&mut self, token: Token) {
        self.0.borrow_mut().handle_no_function_body(token);
    }

    fn handle_function_body_skipped(&mut self, begin: Token, end: Token, is_expression_body: bool) {
        self.0
            .borrow_mut()
            .handle_function_body_skipped(begin, end, is_expression_body);
    }

    fn handle_empty_function_body(&mut self, semicolon: Token) {
        self.0.borrow_mut().handle_empty_function_body(semicolon);
    }

    fn handle_native_clause(&mut self, native: Token, has_name: bool) {
        self.0.borrow_mut().handle_native_clause(native, has_name);
    }

    fn handle_native_function_body_skipped(&mut self, native: Token, semicolon: Token) {
        self.0
            .borrow_mut()
            .handle_native_function_body_skipped(native, semicolon);
    }

    fn handle_native_function_body_ignored(&mut self, native: Token, next: Token) {
        self.0
            .borrow_mut()
            .handle_native_function_body_ignored(native, next);
    }

    fn begin_type_list(&mut self, token: Token) {
        self.0.borrow_mut().begin_type_list(token);
    }

    fn end_type_list(&mut self, count: usize) {
        self.0.borrow_mut().end_type_list(count);
    }

    fn handle_named_mixin_application_with_clause(&mut self, with_keyword: Token) {
        self.0
            .borrow_mut()
            .handle_named_mixin_application_with_clause(with_keyword);
    }

    fn handle_class_with_clause(&mut self, with_keyword: Token) {
        self.0.borrow_mut().handle_class_with_clause(with_keyword);
    }

    fn handle_class_no_with_clause(&mut self) {
        self.0.borrow_mut().handle_class_no_with_clause();
    }

    fn handle_enum_with_clause(&mut self, with_keyword: Token) {
        self.0.borrow_mut().handle_enum_with_clause(with_keyword);
    }

    fn handle_enum_no_with_clause(&mut self) {
        self.0.borrow_mut().handle_enum_no_with_clause();
    }

    fn handle_implements(&mut self, implements: Option<Token>, count: usize) {
        self.0.borrow_mut().handle_implements(implements, count);
    }

    fn handle_class_extends(&mut self, extends: Option<Token>, count: usize) {
        self.0.borrow_mut().handle_class_extends(extends, count);
    }

    fn handle_class_header(&mut self, begin: Token, class: Token, native: Option<Token>) {
        self.0
            .borrow_mut()
            .handle_class_header(begin, class, native);
    }

    fn handle_mixin_header(&mut self, mixin: Token) {
        self.0.borrow_mut().handle_mixin_header(mixin);
    }

    fn handle_mixin_on(&mut self, on: Option<Token>, count: usize) {
        self.0.borrow_mut().handle_mixin_on(on, count);
    }

    fn handle_recover_declaration_header(&mut self, kind: DeclarationHeaderKind) {
        self.0.borrow_mut().handle_recover_declaration_header(kind);
    }

    fn handle_recover_mixin_header(&mut self) {
        self.0.borrow_mut().handle_recover_mixin_header();
    }

    fn begin_class_or_mixin_or_named_mixin_application_prelude(&mut self, token: Token) {
        self.0
            .borrow_mut()
            .begin_class_or_mixin_or_named_mixin_application_prelude(token);
    }

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
        self.0.borrow_mut().begin_class_declaration(
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
    }

    fn end_class_declaration(&mut self, begin: Token, end: Token) {
        self.0.borrow_mut().end_class_declaration(begin, end);
    }

    fn begin_named_mixin_application(
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
        self.0.borrow_mut().begin_named_mixin_application(
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
    }

    fn end_named_mixin_application(
        &mut self,
        begin: Token,
        class: Token,
        equals: Token,
        implements: Option<Token>,
        end: Token,
    ) {
        self.0
            .borrow_mut()
            .end_named_mixin_application(begin, class, equals, implements, end);
    }

    fn begin_class_or_mixin_or_extension_body(&mut self, kind: DeclarationKind, token: Token) {
        self.0
            .borrow_mut()
            .begin_class_or_mixin_or_extension_body(kind, token);
    }

    fn end_class_or_mixin_or_extension_body(
        &mut self,
        kind: DeclarationKind,
        count: usize,
        begin: Token,
        end: Token,
    ) {
        self.0
            .borrow_mut()
            .end_class_or_mixin_or_extension_body(kind, count, begin, end);
    }

    fn handle_no_class_body(&mut self, semicolon: Token) {
        self.0.borrow_mut().handle_no_class_body(semicolon);
    }

    fn begin_mixin_declaration(
        &mut self,
        begin: Token,
        augment_kw: Option<Token>,
        base_kw: Option<Token>,
        mixin: Token,
        name: Token,
    ) {
        self.0
            .borrow_mut()
            .begin_mixin_declaration(begin, augment_kw, base_kw, mixin, name);
    }

    fn end_mixin_declaration(&mut self, begin: Token, end: Token) {
        self.0.borrow_mut().end_mixin_declaration(begin, end);
    }

    fn handle_no_mixin_body(&mut self, semicolon: Token) {
        self.0.borrow_mut().handle_no_mixin_body(semicolon);
    }

    fn begin_enum_declaration_prelude(&mut self, enum_keyword: Token) {
        self.0
            .borrow_mut()
            .begin_enum_declaration_prelude(enum_keyword);
    }

    fn begin_enum_declaration(
        &mut self,
        begin: Token,
        augment_kw: Option<Token>,
        enum_keyword: Token,
        name: Token,
    ) {
        self.0
            .borrow_mut()
            .begin_enum_declaration(begin, augment_kw, enum_keyword, name);
    }

    fn end_enum_declaration(
        &mut self,
        begin: Token,
        enum_keyword: Token,
        left_brace: Token,
        member_count: usize,
        end: Token,
    ) {
        self.0.borrow_mut().end_enum_declaration(
            begin,
            enum_keyword,
            left_brace,
            member_count,
            end,
        );
    }

    fn handle_enum_header(
        &mut self,
        augment: Option<Token>,
        enum_keyword: Token,
        left_brace: Token,
    ) {
        self.0
            .borrow_mut()
            .handle_enum_header(augment, enum_keyword, left_brace);
    }

    fn begin_enum_body(&mut self, token: Token) {
        self.0.borrow_mut().begin_enum_body(token);
    }

    fn end_enum_body(&mut self, begin: Token, end: Token) {
        self.0.borrow_mut().end_enum_body(begin, end);
    }

    fn handle_enum_elements(&mut self, elements_end: Token, count: usize) {
        self.0
            .borrow_mut()
            .handle_enum_elements(elements_end, count);
    }

    fn handle_enum_element(&mut self, begin: Token, augment: Option<Token>) {
        self.0.borrow_mut().handle_enum_element(begin, augment);
    }

    fn handle_no_enum_body(&mut self, semicolon: Token) {
        self.0.borrow_mut().handle_no_enum_body(semicolon);
    }

    fn begin_extension_declaration_prelude(&mut self, extension_keyword: Token) {
        self.0
            .borrow_mut()
            .begin_extension_declaration_prelude(extension_keyword);
    }

    fn begin_extension_declaration(
        &mut self,
        augment: Option<Token>,
        extension_keyword: Token,
        name: Option<Token>,
    ) {
        self.0
            .borrow_mut()
            .begin_extension_declaration(augment, extension_keyword, name);
    }

    fn end_extension_declaration(
        &mut self,
        begin: Token,
        extension_keyword: Token,
        on: Option<Token>,
        end: Token,
    ) {
        self.0
            .borrow_mut()
            .end_extension_declaration(begin, extension_keyword, on, end);
    }

    fn handle_no_extension_body(&mut self, semicolon: Token) {
        self.0.borrow_mut().handle_no_extension_body(semicolon);
    }

    fn begin_extension_type_declaration(
        &mut self,
        augment_kw: Option<Token>,
        extension_keyword: Token,
        name: Token,
    ) {
        self.0
            .borrow_mut()
            .begin_extension_type_declaration(augment_kw, extension_keyword, name);
    }

    fn end_extension_type_declaration(
        &mut self,
        begin: Token,
        augment_kw: Option<Token>,
        extension_keyword: Token,
        type_keyword: Token,
        end: Token,
    ) {
        self.0.borrow_mut().end_extension_type_declaration(
            begin,
            augment_kw,
            extension_keyword,
            type_keyword,
            end,
        );
    }

    fn handle_no_extension_type_body(&mut self, semicolon: Token) {
        self.0.borrow_mut().handle_no_extension_type_body(semicolon);
    }

    fn begin_redirecting_factory_body(&mut self, equals: Token) {
        self.0.borrow_mut().begin_redirecting_factory_body(equals);
    }

    fn end_redirecting_factory_body(&mut self, equals: Token, end: Token) {
        self.0
            .borrow_mut()
            .end_redirecting_factory_body(equals, end);
    }

    fn handle_no_type_name_in_constructor_reference(&mut self, token: Token) {
        self.0
            .borrow_mut()
            .handle_no_type_name_in_constructor_reference(token);
    }

    fn handle_mixin_with_clause(&mut self, with_keyword: Token) {
        self.0.borrow_mut().handle_mixin_with_clause(with_keyword);
    }

    fn begin_primary_constructor_body(&mut self, token: Token, augment: Option<Token>) {
        self.0
            .borrow_mut()
            .begin_primary_constructor_body(token, augment);
    }

    fn end_primary_constructor_body(
        &mut self,
        begin: Token,
        this: Token,
        begin_initializers: Option<Token>,
        end: Token,
    ) {
        self.0
            .borrow_mut()
            .end_primary_constructor_body(begin, this, begin_initializers, end);
    }

    fn begin_primary_constructor(&mut self, begin: Token) {
        self.0.borrow_mut().begin_primary_constructor(begin);
    }

    fn end_primary_constructor(
        &mut self,
        kind: DeclarationKind,
        begin: Token,
        end: Token,
        const_keyword: Option<Token>,
        has_constructor_name: bool,
        name: Option<Token>,
    ) {
        self.0.borrow_mut().end_primary_constructor(
            kind,
            begin,
            end,
            const_keyword,
            has_constructor_name,
            name,
        );
    }

    fn handle_no_primary_constructor(
        &mut self,
        kind: DeclarationKind,
        token: Token,
        const_keyword: Option<Token>,
    ) {
        self.0
            .borrow_mut()
            .handle_no_primary_constructor(kind, token, const_keyword);
    }

    fn begin_block(&mut self, token: Token, kind: BlockKind) {
        self.0.borrow_mut().begin_block(token, kind);
    }

    fn end_block(&mut self, count: usize, begin: Token, end: Token, kind: BlockKind) {
        self.0.borrow_mut().end_block(count, begin, end, kind);
    }

    fn begin_block_function_body(&mut self, token: Token) {
        self.0.borrow_mut().begin_block_function_body(token);
    }

    fn end_block_function_body(&mut self, count: usize, begin: Token, end: Token) {
        self.0
            .borrow_mut()
            .end_block_function_body(count, begin, end);
    }

    fn handle_invalid_function_body(&mut self, token: Token) {
        self.0.borrow_mut().handle_invalid_function_body(token);
    }

    fn handle_expression_function_body(&mut self, arrow: Token, end: Option<Token>) {
        self.0
            .borrow_mut()
            .handle_expression_function_body(arrow, end);
    }

    fn handle_expression_statement(&mut self, begin: Token, end: Token) {
        self.0.borrow_mut().handle_expression_statement(begin, end);
    }

    fn handle_empty_statement(&mut self, token: Token) {
        self.0.borrow_mut().handle_empty_statement(token);
    }

    fn handle_invalid_statement(&mut self, begin: Token) {
        self.0.borrow_mut().handle_invalid_statement(begin);
    }

    fn begin_if_statement(&mut self, token: Token) {
        self.0.borrow_mut().begin_if_statement(token);
    }

    fn begin_then_statement(&mut self, token: Token) {
        self.0.borrow_mut().begin_then_statement(token);
    }

    fn end_then_statement(&mut self, begin: Token, end: Token) {
        self.0.borrow_mut().end_then_statement(begin, end);
    }

    fn begin_else_statement(&mut self, token: Token) {
        self.0.borrow_mut().begin_else_statement(token);
    }

    fn end_else_statement(&mut self, begin: Token, end: Token) {
        self.0.borrow_mut().end_else_statement(begin, end);
    }

    fn end_if_statement(&mut self, token: Token, else_kw: Option<Token>, end: Token) {
        self.0.borrow_mut().end_if_statement(token, else_kw, end);
    }

    fn begin_for_statement(&mut self, token: Token) {
        self.0.borrow_mut().begin_for_statement(token);
    }

    fn handle_for_initializer_empty_statement(&mut self, token: Token) {
        self.0
            .borrow_mut()
            .handle_for_initializer_empty_statement(token);
    }

    fn handle_for_initializer_expression_statement(&mut self, token: Token, for_in: bool) {
        self.0
            .borrow_mut()
            .handle_for_initializer_expression_statement(token, for_in);
    }

    fn handle_for_initializer_local_variable_declaration(&mut self, token: Token, for_in: bool) {
        self.0
            .borrow_mut()
            .handle_for_initializer_local_variable_declaration(token, for_in);
    }

    fn handle_for_initializer_pattern_variable_assignment(
        &mut self,
        pattern_keyword: Token,
        equals: Token,
    ) {
        self.0
            .borrow_mut()
            .handle_for_initializer_pattern_variable_assignment(pattern_keyword, equals);
    }

    fn begin_for_statement_body(&mut self, token: Token) {
        self.0.borrow_mut().begin_for_statement_body(token);
    }

    fn end_for_statement_body(&mut self, end: Token) {
        self.0.borrow_mut().end_for_statement_body(end);
    }

    fn end_for_statement(&mut self, end: Token) {
        self.0.borrow_mut().end_for_statement(end);
    }

    fn handle_for_loop_parts(
        &mut self,
        for_kw: Token,
        left_paren: Token,
        left_separator: Token,
        right_separator: Token,
        expression_count: usize,
    ) {
        self.0.borrow_mut().handle_for_loop_parts(
            for_kw,
            left_paren,
            left_separator,
            right_separator,
            expression_count,
        );
    }

    fn handle_for_updaters_end(&mut self, updaters_last: Token) {
        self.0
            .borrow_mut()
            .handle_for_updaters_end(updaters_last);
    }

    fn begin_for_in_body(&mut self, token: Token) {
        self.0.borrow_mut().begin_for_in_body(token);
    }

    fn end_for_in_body(&mut self, end: Token) {
        self.0.borrow_mut().end_for_in_body(end);
    }

    fn end_for_in(&mut self, end: Token) {
        self.0.borrow_mut().end_for_in(end);
    }

    fn handle_for_in_loop_parts(
        &mut self,
        await_kw: Option<Token>,
        for_kw: Token,
        left_paren: Token,
        pattern_keyword: Option<Token>,
        in_kw: Token,
    ) {
        self.0.borrow_mut().handle_for_in_loop_parts(
            await_kw,
            for_kw,
            left_paren,
            pattern_keyword,
            in_kw,
        );
    }

    fn begin_for_in_expression(&mut self, token: Token) {
        self.0.borrow_mut().begin_for_in_expression(token);
    }

    fn end_for_in_expression(&mut self, end: Token) {
        self.0.borrow_mut().end_for_in_expression(end);
    }

    fn begin_while_statement(&mut self, token: Token) {
        self.0.borrow_mut().begin_while_statement(token);
    }

    fn begin_while_statement_body(&mut self, token: Token) {
        self.0.borrow_mut().begin_while_statement_body(token);
    }

    fn end_while_statement_body(&mut self, end: Token) {
        self.0.borrow_mut().end_while_statement_body(end);
    }

    fn end_while_statement(&mut self, token: Token, end: Token) {
        self.0.borrow_mut().end_while_statement(token, end);
    }

    fn begin_do_while_statement(&mut self, token: Token) {
        self.0.borrow_mut().begin_do_while_statement(token);
    }

    fn begin_do_while_statement_body(&mut self, token: Token) {
        self.0.borrow_mut().begin_do_while_statement_body(token);
    }

    fn end_do_while_statement_body(&mut self, end: Token) {
        self.0.borrow_mut().end_do_while_statement_body(end);
    }

    fn end_do_while_statement(&mut self, token: Token, while_kw: Token, end: Token) {
        self.0
            .borrow_mut()
            .end_do_while_statement(token, while_kw, end);
    }

    fn begin_return_statement(&mut self, token: Token) {
        self.0.borrow_mut().begin_return_statement(token);
    }

    fn end_return_statement(&mut self, has_expression: bool, begin: Token, end: Token) {
        self.0
            .borrow_mut()
            .end_return_statement(has_expression, begin, end);
    }

    fn begin_yield_statement(&mut self, token: Token) {
        self.0.borrow_mut().begin_yield_statement(token);
    }

    fn end_yield_statement(&mut self, begin: Token, star: Option<Token>, end: Token) {
        self.0.borrow_mut().end_yield_statement(begin, star, end);
    }

    fn end_invalid_yield_statement(&mut self, begin: Token, star: Option<Token>, end: Token) {
        self.0
            .borrow_mut()
            .end_invalid_yield_statement(begin, star, end);
    }

    fn begin_labeled_statement(&mut self, token: Token, label_count: usize) {
        self.0
            .borrow_mut()
            .begin_labeled_statement(token, label_count);
    }

    fn end_labeled_statement(&mut self, label_count: usize) {
        self.0.borrow_mut().end_labeled_statement(label_count);
    }

    fn handle_label(&mut self, token: Token) {
        self.0.borrow_mut().handle_label(token);
    }

    fn begin_local_function_declaration(&mut self, token: Token) {
        self.0.borrow_mut().begin_local_function_declaration(token);
    }

    fn end_local_function_declaration(&mut self, end: Token) {
        self.0.borrow_mut().end_local_function_declaration(end);
    }

    fn begin_function_name(&mut self, token: Token) {
        self.0.borrow_mut().begin_function_name(token);
    }

    fn end_function_name(&mut self, begin: Token, end: Token, is_function_expression: bool) {
        self.0
            .borrow_mut()
            .end_function_name(begin, end, is_function_expression);
    }

    fn begin_variables_declaration(
        &mut self,
        token: Token,
        late_kw: Option<Token>,
        var_final_or_const: Option<Token>,
    ) {
        self.0
            .borrow_mut()
            .begin_variables_declaration(token, late_kw, var_final_or_const);
    }

    fn end_variables_declaration(&mut self, count: usize, end: Option<Token>) {
        self.0.borrow_mut().end_variables_declaration(count, end);
    }

    fn begin_initialized_identifier(&mut self, token: Token) {
        self.0.borrow_mut().begin_initialized_identifier(token);
    }

    fn end_initialized_identifier(&mut self, name: Token) {
        self.0.borrow_mut().end_initialized_identifier(name);
    }

    fn handle_no_variable_initializer(&mut self, token: Token) {
        self.0.borrow_mut().handle_no_variable_initializer(token);
    }

    fn begin_variable_initializer(&mut self, token: Token) {
        self.0.borrow_mut().begin_variable_initializer(token);
    }

    fn end_variable_initializer(&mut self, token: Token) {
        self.0.borrow_mut().end_variable_initializer(token);
    }

    fn begin_try_statement(&mut self, token: Token) {
        self.0.borrow_mut().begin_try_statement(token);
    }

    fn end_try_statement(
        &mut self,
        count: usize,
        begin: Token,
        finally_kw: Option<Token>,
        end: Token,
    ) {
        self.0
            .borrow_mut()
            .end_try_statement(count, begin, finally_kw, end);
    }

    fn begin_catch_clause(&mut self, token: Token) {
        self.0.borrow_mut().begin_catch_clause(token);
    }

    fn end_catch_clause(&mut self, token: Token) {
        self.0.borrow_mut().end_catch_clause(token);
    }

    fn handle_catch_block(
        &mut self,
        on_kw: Option<Token>,
        catch_kw: Option<Token>,
        comma: Option<Token>,
    ) {
        self.0
            .borrow_mut()
            .handle_catch_block(on_kw, catch_kw, comma);
    }

    fn handle_finally_block(&mut self, finally_kw: Token) {
        self.0.borrow_mut().handle_finally_block(finally_kw);
    }

    fn begin_switch_statement(&mut self, token: Token) {
        self.0.borrow_mut().begin_switch_statement(token);
    }

    fn end_switch_statement(&mut self, token: Token, end: Token) {
        self.0.borrow_mut().end_switch_statement(token, end);
    }

    fn begin_assert(&mut self, token: Token, kind: AssertKind) {
        self.0.borrow_mut().begin_assert(token, kind);
    }

    fn assert_after_condition(&mut self) {
        self.0.borrow_mut().assert_after_condition();
    }

    fn end_assert(
        &mut self,
        begin: Token,
        kind: AssertKind,
        left_paren: Token,
        comma: Option<Token>,
        end: Token,
    ) {
        self.0
            .borrow_mut()
            .end_assert(begin, kind, left_paren, comma, end);
    }

    fn begin_switch_block(&mut self, token: Token) {
        self.0.borrow_mut().begin_switch_block(token);
    }

    fn end_switch_block(&mut self, count: usize, begin: Token, end: Token) {
        self.0.borrow_mut().end_switch_block(count, begin, end);
    }

    fn begin_case_expression(&mut self, case_kw: Token) {
        self.0.borrow_mut().begin_case_expression(case_kw);
    }

    fn end_case_expression(&mut self, case_kw: Token, when: Option<Token>, colon: Token) {
        self.0
            .borrow_mut()
            .end_case_expression(case_kw, when, colon);
    }

    fn begin_switch_case(&mut self, label_count: usize, expression_count: usize, token: Token) {
        self.0
            .borrow_mut()
            .begin_switch_case(label_count, expression_count, token);
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
        self.0.borrow_mut().end_switch_case(
            label_count,
            expression_count,
            default_kw,
            colon_after_default,
            statement_count,
            begin,
            end,
        );
    }

    fn begin_switch_case_when_clause(&mut self, token: Token) {
        self.0.borrow_mut().begin_switch_case_when_clause(token);
    }

    fn end_switch_case_when_clause(&mut self, token: Token) {
        self.0.borrow_mut().end_switch_case_when_clause(token);
    }

    fn handle_switch_case_no_when_clause(&mut self, token: Token) {
        self.0.borrow_mut().handle_switch_case_no_when_clause(token);
    }

    fn handle_break_statement(&mut self, has_target: bool, break_kw: Token, end: Token) {
        self.0
            .borrow_mut()
            .handle_break_statement(has_target, break_kw, end);
    }

    fn handle_continue_statement(&mut self, has_target: bool, continue_kw: Token, end: Token) {
        self.0
            .borrow_mut()
            .handle_continue_statement(has_target, continue_kw, end);
    }

    fn begin_rethrow_statement(&mut self, token: Token) {
        self.0.borrow_mut().begin_rethrow_statement(token);
    }

    fn end_rethrow_statement(&mut self, rethrow_kw: Token, end: Token) {
        self.0.borrow_mut().end_rethrow_statement(rethrow_kw, end);
    }

    fn begin_initializer(&mut self, token: Token) {
        self.0.borrow_mut().begin_initializer(token);
    }

    fn end_initializer(&mut self, end: Token) {
        self.0.borrow_mut().end_initializer(end);
    }

    fn handle_parenthesized_condition(
        &mut self,
        begin: Token,
        case_kw: Option<Token>,
        when: Option<Token>,
    ) {
        self.0
            .borrow_mut()
            .handle_parenthesized_condition(begin, case_kw, when);
    }

    fn begin_pattern_guard(&mut self, when: Token) {
        self.0.borrow_mut().begin_pattern_guard(when);
    }

    fn end_pattern_guard(&mut self, token: Token) {
        self.0.borrow_mut().end_pattern_guard(token);
    }

    fn handle_pattern_variable_declaration_statement(
        &mut self,
        keyword: Token,
        equals: Token,
        semicolon: Token,
    ) {
        self.0
            .borrow_mut()
            .handle_pattern_variable_declaration_statement(keyword, equals, semicolon);
    }

    fn begin_if_control_flow(&mut self, token: Token) {
        self.0.borrow_mut().begin_if_control_flow(token);
    }

    fn handle_then_control_flow(&mut self, token: Token) {
        self.0.borrow_mut().handle_then_control_flow(token);
    }

    fn handle_else_control_flow(&mut self, else_kw: Token, else_start: Token) {
        self.0.borrow_mut().handle_else_control_flow(else_kw, else_start);
    }

    fn end_if_control_flow(&mut self, token: Token) {
        self.0.borrow_mut().end_if_control_flow(token);
    }

    fn end_if_else_control_flow(&mut self, token: Token) {
        self.0.borrow_mut().end_if_else_control_flow(token);
    }

    fn begin_for_control_flow(&mut self, await_kw: Option<Token>, for_kw: Token) {
        self.0.borrow_mut().begin_for_control_flow(await_kw, for_kw);
    }

    fn end_for_control_flow(&mut self, token: Token) {
        self.0.borrow_mut().end_for_control_flow(token);
    }

    fn end_for_in_control_flow(&mut self, token: Token) {
        self.0.borrow_mut().end_for_in_control_flow(token);
    }

    fn handle_spread_expression(&mut self, operator: Token) {
        self.0.borrow_mut().handle_spread_expression(operator);
    }

    fn handle_null_aware_element(&mut self, token: Token) {
        self.0.borrow_mut().handle_null_aware_element(token);
    }

    fn begin_switch_expression(&mut self, token: Token) {
        self.0.borrow_mut().begin_switch_expression(token);
    }

    fn end_switch_expression(&mut self, token: Token, end: Token) {
        self.0.borrow_mut().end_switch_expression(token, end);
    }

    fn begin_switch_expression_block(&mut self, token: Token) {
        self.0.borrow_mut().begin_switch_expression_block(token);
    }

    fn end_switch_expression_block(&mut self, count: usize, begin: Token, end: Token) {
        self.0
            .borrow_mut()
            .end_switch_expression_block(count, begin, end);
    }

    fn begin_switch_expression_case(&mut self) {
        self.0.borrow_mut().begin_switch_expression_case();
    }

    fn handle_switch_expression_case_pattern(&mut self, token: Token) {
        self.0
            .borrow_mut()
            .handle_switch_expression_case_pattern(token);
    }

    fn end_switch_expression_case(
        &mut self,
        begin: Token,
        when: Option<Token>,
        arrow: Token,
        end: Token,
    ) {
        self.0
            .borrow_mut()
            .end_switch_expression_case(begin, when, arrow, end);
    }

    fn handle_recoverable_error(&mut self, message: Message, start: Token, end: Token) {
        self.0
            .borrow_mut()
            .handle_recoverable_error(message, start, end);
    }

    fn handle_experiment_not_enabled(&mut self, flag: ExperimentalFlag, begin: Token, end: Token) {
        self.0
            .borrow_mut()
            .handle_experiment_not_enabled(flag, begin, end);
    }
}
