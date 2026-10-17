//! The listener multiplexer: one parse pass feeding several listeners.
//!
//! Changed-file analysis needs the same unit parse observed by several
//! consumers at once — parse-recovery diagnostics, element collection, and
//! index capture all read the same token table. Parsing dominates the
//! per-file cost, so a second full pass over the same tokens is pure waste;
//! [`MultiplexListener`] fans one parse's event stream out to every
//! consumer instead (rust-analyzer's one-parse-per-file shape: the green
//! tree is read by every downstream analysis, never re-parsed).
//!
//! Pairs nest, so any arity composes:
//! `MultiplexListener::new(&mut a, MultiplexListener::new(&mut b, &mut c))`.
//! Every forwarded event reaches `a` before `b`, in call order, so each
//! receiver observes exactly the subsequence it would observe in a solo
//! parse — the fan-out is order-deterministic and equivalent to a
//! sequential replay.
//!
//! # Forwarding coverage invariant
//!
//! The real in-tree mux consumers are the multi-consumer fan-outs only:
//! dartq-engine's parse-once face (`l1.rs`: recovery diagnostics + element
//! collection + index capture over one parse). A single-consumer site must
//! pass its listener to [`crate::Parser::new`] directly — wrapping it in a
//! mux forwards only the event set below (declaration/method/type surface,
//! formal parameters, recovery and experiment reports), so every override
//! outside that set silently starves. A trait event with no forward falls
//! through to the default no-op and never reaches the receivers, so when a
//! listener gains an override outside that set, the matching forward must
//! be added here — the fan-out equality tests pin the corpus paths.

use crate::listener::{
    ConstructorReferenceContext, DeclarationKind, ExperimentalFlag, FormalParameterKind,
    IdentifierContext, MemberKind, ParserListener, Token,
};
use crate::message::Message;

/// Forwards every parse event to two borrowed receivers, `a` then `b`.
/// Nest pairs for wider fan-outs; a [`crate::NullListener`] slot is the
/// empty fan-out.
pub struct MultiplexListener<'a, A, B> {
    a: &'a mut A,
    b: &'a mut B,
}

impl<'a, A, B> MultiplexListener<'a, A, B> {
    /// Bundles two borrowed receivers behind one listener.
    pub fn new(a: &'a mut A, b: &'a mut B) -> Self {
        Self { a, b }
    }
}

impl<A: ParserListener, B: ParserListener> ParserListener for MultiplexListener<'_, A, B> {
    fn begin_library_name(&mut self, token: Token) {
        self.a.begin_library_name(token);
        self.b.begin_library_name(token);
    }

    fn begin_library_augmentation(&mut self, augment_kw: Token, library_keyword: Token) {
        self.a
            .begin_library_augmentation(augment_kw, library_keyword);
        self.b
            .begin_library_augmentation(augment_kw, library_keyword);
    }

    fn begin_import(&mut self, import_keyword: Token) {
        self.a.begin_import(import_keyword);
        self.b.begin_import(import_keyword);
    }

    fn begin_export(&mut self, token: Token) {
        self.a.begin_export(token);
        self.b.begin_export(token);
    }

    fn begin_part(&mut self, token: Token) {
        self.a.begin_part(token);
        self.b.begin_part(token);
    }

    fn end_metadata_star(&mut self, count: usize) {
        self.a.end_metadata_star(count);
        self.b.end_metadata_star(count);
    }

    fn end_metadata(&mut self, begin: Token, period_before_name: Option<Token>, end: Token) {
        self.a.end_metadata(begin, period_before_name, end);
        self.b.end_metadata(begin, period_before_name, end);
    }

    fn handle_type(&mut self, name: Token, question: Option<Token>) {
        self.a.handle_type(name, question);
        self.b.handle_type(name, question);
    }

    fn handle_no_type(&mut self, last_consumed: Token) {
        self.a.handle_no_type(last_consumed);
        self.b.handle_no_type(last_consumed);
    }

    fn handle_identifier(&mut self, token: Token, context: IdentifierContext) {
        self.a.handle_identifier(token, context);
        self.b.handle_identifier(token, context);
    }

    fn begin_type_arguments(&mut self, token: Token) {
        self.a.begin_type_arguments(token);
        self.b.begin_type_arguments(token);
    }

    fn end_type_arguments(&mut self, count: usize, begin: Token, end: Token) {
        self.a.end_type_arguments(count, begin, end);
        self.b.end_type_arguments(count, begin, end);
    }

    fn begin_type_variables(&mut self, token: Token) {
        self.a.begin_type_variables(token);
        self.b.begin_type_variables(token);
    }

    fn end_type_variables(&mut self, begin: Token, end: Token) {
        self.a.end_type_variables(begin, end);
        self.b.end_type_variables(begin, end);
    }

    fn begin_type_variable(&mut self, token: Token) {
        self.a.begin_type_variable(token);
        self.b.begin_type_variable(token);
    }

    fn begin_record_type(&mut self, left_paren: Token) {
        self.a.begin_record_type(left_paren);
        self.b.begin_record_type(left_paren);
    }

    fn begin_function_type(&mut self, begin: Token) {
        self.a.begin_function_type(begin);
        self.b.begin_function_type(begin);
    }

    fn end_function_type(&mut self, function: Token, question: Option<Token>) {
        self.a.end_function_type(function, question);
        self.b.end_function_type(function, question);
    }

    fn handle_void_keyword(&mut self, token: Token) {
        self.a.handle_void_keyword(token);
        self.b.handle_void_keyword(token);
    }

    fn handle_void_keyword_with_type_arguments(&mut self, token: Token) {
        self.a.handle_void_keyword_with_type_arguments(token);
        self.b.handle_void_keyword_with_type_arguments(token);
    }

    fn handle_no_arguments(&mut self, token: Token) {
        self.a.handle_no_arguments(token);
        self.b.handle_no_arguments(token);
    }

    fn handle_send(&mut self, begin: Token, end: Token) {
        self.a.handle_send(begin, end);
        self.b.handle_send(begin, end);
    }

    fn handle_send_without_arguments(&mut self, begin: Token, end: Token, next: Token) {
        self.a.handle_send_without_arguments(begin, end, next);
        self.b.handle_send_without_arguments(begin, end, next);
    }

    fn handle_null_aware_index_operand_begin(&mut self, question: Token) {
        self.a.handle_null_aware_index_operand_begin(question);
        self.b.handle_null_aware_index_operand_begin(question);
    }

    fn handle_null_aware_member_begin(&mut self, question: Token) {
        self.a.handle_null_aware_member_begin(question);
        self.b.handle_null_aware_member_begin(question);
    }

    fn handle_indexed_expression(&mut self, question: Option<Token>, open: Token, close: Token) {
        self.a.handle_indexed_expression(question, open, close);
        self.b.handle_indexed_expression(question, open, close);
    }

    fn end_constructor_reference(
        &mut self,
        start: Token,
        period_before_name: Option<Token>,
        end: Token,
        context: ConstructorReferenceContext,
    ) {
        self.a
            .end_constructor_reference(start, period_before_name, end, context);
        self.b
            .end_constructor_reference(start, period_before_name, end, context);
    }

    fn handle_assignment_expression(&mut self, operator: Token, end: Token) {
        self.a.handle_assignment_expression(operator, end);
        self.b.handle_assignment_expression(operator, end);
    }

    fn handle_cascade_access(&mut self, operator: Token, end: Token, is_null_aware: bool) {
        self.a.handle_cascade_access(operator, end, is_null_aware);
        self.b.handle_cascade_access(operator, end, is_null_aware);
    }

    fn begin_formal_parameters(&mut self, token: Token, kind: MemberKind) {
        self.a.begin_formal_parameters(token, kind);
        self.b.begin_formal_parameters(token, kind);
    }

    fn end_formal_parameters(&mut self, count: usize, begin: Token, end: Token, kind: MemberKind) {
        self.a.end_formal_parameters(count, begin, end, kind);
        self.b.end_formal_parameters(count, begin, end, kind);
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
        self.a.end_formal_parameter(
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
        self.b.end_formal_parameter(
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

    fn begin_function_typed_formal_parameter(&mut self, token: Token) {
        self.a.begin_function_typed_formal_parameter(token);
        self.b.begin_function_typed_formal_parameter(token);
    }

    fn end_function_typed_formal_parameter(&mut self, token: Token, question: Option<Token>) {
        self.a.end_function_typed_formal_parameter(token, question);
        self.b.end_function_typed_formal_parameter(token, question);
    }

    fn handle_no_formal_parameters(&mut self, token: Token, kind: MemberKind) {
        self.a.handle_no_formal_parameters(token, kind);
        self.b.handle_no_formal_parameters(token, kind);
    }

    fn begin_top_level_method(
        &mut self,
        last_consumed: Token,
        augment: Option<Token>,
        external: Option<Token>,
    ) {
        self.a
            .begin_top_level_method(last_consumed, augment, external);
        self.b
            .begin_top_level_method(last_consumed, augment, external);
    }

    fn end_top_level_method(&mut self, begin: Token, get_or_set: Option<Token>, end: Token) {
        self.a.end_top_level_method(begin, get_or_set, end);
        self.b.end_top_level_method(begin, get_or_set, end);
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
        self.a.end_top_level_fields(
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
        self.b.end_top_level_fields(
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
        self.a.begin_typedef(token);
        self.b.begin_typedef(token);
    }

    fn end_typedef(
        &mut self,
        augment: Option<Token>,
        typedef: Token,
        equals: Option<Token>,
        end: Token,
    ) {
        self.a.end_typedef(augment, typedef, equals, end);
        self.b.end_typedef(augment, typedef, equals, end);
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
        self.a.begin_fields(
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
        self.b.begin_fields(
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
        self.a.end_fields(
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
        self.b.end_fields(
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
        self.a.begin_constructor(
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
        self.b.begin_constructor(
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
        self.a
            .end_constructor(kind, begin, new_kw, begin_param, begin_initializers, end);
        self.b
            .end_constructor(kind, begin, new_kw, begin_param, begin_initializers, end);
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
        self.a.begin_method(
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
        self.b.begin_method(
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
        self.a.end_method(
            kind,
            get_or_set,
            begin,
            begin_param,
            begin_initializers,
            end,
        );
        self.b.end_method(
            kind,
            get_or_set,
            begin,
            begin_param,
            begin_initializers,
            end,
        );
    }

    fn handle_async_modifier(&mut self, async_kw: Option<Token>, star: Option<Token>) {
        self.a.handle_async_modifier(async_kw, star);
        self.b.handle_async_modifier(async_kw, star);
    }

    fn begin_type_list(&mut self, token: Token) {
        self.a.begin_type_list(token);
        self.b.begin_type_list(token);
    }

    fn handle_named_mixin_application_with_clause(&mut self, with_keyword: Token) {
        self.a
            .handle_named_mixin_application_with_clause(with_keyword);
        self.b
            .handle_named_mixin_application_with_clause(with_keyword);
    }

    fn handle_class_with_clause(&mut self, with_keyword: Token) {
        self.a.handle_class_with_clause(with_keyword);
        self.b.handle_class_with_clause(with_keyword);
    }

    fn handle_enum_with_clause(&mut self, with_keyword: Token) {
        self.a.handle_enum_with_clause(with_keyword);
        self.b.handle_enum_with_clause(with_keyword);
    }

    fn handle_implements(&mut self, implements: Option<Token>, count: usize) {
        self.a.handle_implements(implements, count);
        self.b.handle_implements(implements, count);
    }

    fn handle_class_extends(&mut self, extends: Option<Token>, count: usize) {
        self.a.handle_class_extends(extends, count);
        self.b.handle_class_extends(extends, count);
    }

    fn handle_class_header(&mut self, begin: Token, class: Token, native: Option<Token>) {
        self.a.handle_class_header(begin, class, native);
        self.b.handle_class_header(begin, class, native);
    }

    fn handle_mixin_header(&mut self, mixin: Token) {
        self.a.handle_mixin_header(mixin);
        self.b.handle_mixin_header(mixin);
    }

    fn handle_mixin_on(&mut self, on: Option<Token>, count: usize) {
        self.a.handle_mixin_on(on, count);
        self.b.handle_mixin_on(on, count);
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
        self.a.begin_class_declaration(
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
        self.b.begin_class_declaration(
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
        self.a.end_class_declaration(begin, end);
        self.b.end_class_declaration(begin, end);
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
        self.a.begin_named_mixin_application(
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
        self.b.begin_named_mixin_application(
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
        self.a
            .end_named_mixin_application(begin, class, equals, implements, end);
        self.b
            .end_named_mixin_application(begin, class, equals, implements, end);
    }

    fn begin_class_or_mixin_or_extension_body(&mut self, kind: DeclarationKind, token: Token) {
        self.a.begin_class_or_mixin_or_extension_body(kind, token);
        self.b.begin_class_or_mixin_or_extension_body(kind, token);
    }

    fn end_class_or_mixin_or_extension_body(
        &mut self,
        kind: DeclarationKind,
        count: usize,
        begin: Token,
        end: Token,
    ) {
        self.a
            .end_class_or_mixin_or_extension_body(kind, count, begin, end);
        self.b
            .end_class_or_mixin_or_extension_body(kind, count, begin, end);
    }

    fn handle_no_class_body(&mut self, semicolon: Token) {
        self.a.handle_no_class_body(semicolon);
        self.b.handle_no_class_body(semicolon);
    }

    fn begin_mixin_declaration(
        &mut self,
        begin: Token,
        augment_kw: Option<Token>,
        base_kw: Option<Token>,
        mixin: Token,
        name: Token,
    ) {
        self.a
            .begin_mixin_declaration(begin, augment_kw, base_kw, mixin, name);
        self.b
            .begin_mixin_declaration(begin, augment_kw, base_kw, mixin, name);
    }

    fn end_mixin_declaration(&mut self, begin: Token, end: Token) {
        self.a.end_mixin_declaration(begin, end);
        self.b.end_mixin_declaration(begin, end);
    }

    fn handle_no_mixin_body(&mut self, semicolon: Token) {
        self.a.handle_no_mixin_body(semicolon);
        self.b.handle_no_mixin_body(semicolon);
    }

    fn begin_enum_declaration(
        &mut self,
        begin: Token,
        augment_kw: Option<Token>,
        enum_keyword: Token,
        name: Token,
    ) {
        self.a
            .begin_enum_declaration(begin, augment_kw, enum_keyword, name);
        self.b
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
        self.a
            .end_enum_declaration(begin, enum_keyword, left_brace, member_count, end);
        self.b
            .end_enum_declaration(begin, enum_keyword, left_brace, member_count, end);
    }

    fn handle_enum_header(
        &mut self,
        augment: Option<Token>,
        enum_keyword: Token,
        left_brace: Token,
    ) {
        self.a.handle_enum_header(augment, enum_keyword, left_brace);
        self.b.handle_enum_header(augment, enum_keyword, left_brace);
    }

    fn handle_no_enum_body(&mut self, semicolon: Token) {
        self.a.handle_no_enum_body(semicolon);
        self.b.handle_no_enum_body(semicolon);
    }

    fn begin_enum_body(&mut self, token: Token) {
        self.a.begin_enum_body(token);
        self.b.begin_enum_body(token);
    }

    fn end_enum_body(&mut self, begin: Token, end: Token) {
        self.a.end_enum_body(begin, end);
        self.b.end_enum_body(begin, end);
    }

    fn begin_extension_declaration(
        &mut self,
        augment: Option<Token>,
        extension_keyword: Token,
        name: Option<Token>,
    ) {
        self.a
            .begin_extension_declaration(augment, extension_keyword, name);
        self.b
            .begin_extension_declaration(augment, extension_keyword, name);
    }

    fn end_extension_declaration(
        &mut self,
        begin: Token,
        extension_keyword: Token,
        on: Option<Token>,
        end: Token,
    ) {
        self.a
            .end_extension_declaration(begin, extension_keyword, on, end);
        self.b
            .end_extension_declaration(begin, extension_keyword, on, end);
    }

    fn handle_no_extension_body(&mut self, semicolon: Token) {
        self.a.handle_no_extension_body(semicolon);
        self.b.handle_no_extension_body(semicolon);
    }

    fn begin_extension_type_declaration(
        &mut self,
        augment_kw: Option<Token>,
        extension_keyword: Token,
        name: Token,
    ) {
        self.a
            .begin_extension_type_declaration(augment_kw, extension_keyword, name);
        self.b
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
        self.a.end_extension_type_declaration(
            begin,
            augment_kw,
            extension_keyword,
            type_keyword,
            end,
        );
        self.b.end_extension_type_declaration(
            begin,
            augment_kw,
            extension_keyword,
            type_keyword,
            end,
        );
    }

    fn handle_no_extension_type_body(&mut self, semicolon: Token) {
        self.a.handle_no_extension_type_body(semicolon);
        self.b.handle_no_extension_type_body(semicolon);
    }

    fn begin_block_function_body(&mut self, token: Token) {
        self.a.begin_block_function_body(token);
        self.b.begin_block_function_body(token);
    }

    fn handle_expression_function_body(&mut self, arrow: Token, end: Option<Token>) {
        self.a.handle_expression_function_body(arrow, end);
        self.b.handle_expression_function_body(arrow, end);
    }

    fn handle_recoverable_error(&mut self, message: Message, start: Token, end: Token) {
        // The trait passes `Message` by value, so every receiver past the
        // first clones it. Recovery reports are the slow path already
        // (formatted text, bounded by the parser's diagnostic cap), and
        // healthy files produce none.
        self.a.handle_recoverable_error(message.clone(), start, end);
        self.b.handle_recoverable_error(message, start, end);
    }

    fn handle_experiment_not_enabled(&mut self, flag: ExperimentalFlag, begin: Token, end: Token) {
        self.a.handle_experiment_not_enabled(flag, begin, end);
        self.b.handle_experiment_not_enabled(flag, begin, end);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::NullListener;
    use crate::expr_const::test_support::{parser, parser_ref};

    /// Records the forwarded event names in arrival order.
    #[derive(Default)]
    struct Recorder {
        events: Vec<&'static str>,
    }

    impl ParserListener for Recorder {
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
            self.events.push("begin_class_declaration");
        }
        fn end_class_declaration(&mut self, _: Token, _: Token) {
            self.events.push("end_class_declaration");
        }
        fn begin_method(
            &mut self,
            _: DeclarationKind,
            _: Option<Token>,
            _: Option<Token>,
            _: Option<Token>,
            _: Option<Token>,
            _: Option<Token>,
            _: Option<Token>,
            _: Token,
            _: Option<&str>,
        ) {
            self.events.push("begin_method");
        }
        fn end_method(
            &mut self,
            _: DeclarationKind,
            _: Option<Token>,
            _: Token,
            _: Token,
            _: Option<Token>,
            _: Token,
        ) {
            self.events.push("end_method");
        }
        fn begin_formal_parameters(&mut self, _: Token, _: MemberKind) {
            self.events.push("begin_formal_parameters");
        }
        fn end_formal_parameters(&mut self, _: usize, _: Token, _: Token, _: MemberKind) {
            self.events.push("end_formal_parameters");
        }
        fn begin_function_typed_formal_parameter(&mut self, _: Token) {
            self.events.push("begin_function_typed_formal_parameter");
        }
        fn end_function_typed_formal_parameter(&mut self, _: Token, _: Option<Token>) {
            self.events.push("end_function_typed_formal_parameter");
        }
        fn handle_identifier(&mut self, _: Token, _: IdentifierContext) {
            self.events.push("handle_identifier");
        }
        fn handle_implements(&mut self, _: Option<Token>, _: usize) {
            self.events.push("handle_implements");
        }
        fn end_function_type(&mut self, _: Token, _: Option<Token>) {
            self.events.push("end_function_type");
        }
        fn begin_typedef(&mut self, _: Token) {
            self.events.push("begin_typedef");
        }
        fn end_typedef(&mut self, _: Option<Token>, _: Token, _: Option<Token>, _: Token) {
            self.events.push("end_typedef");
        }
        fn handle_assignment_expression(&mut self, _: Token, _: Token) {
            self.events.push("handle_assignment_expression");
        }
        fn handle_recoverable_error(&mut self, _: Message, _: Token, _: Token) {
            self.events.push("handle_recoverable_error");
        }
    }

    /// A corpus firing every event the recorder overrides.
    const CORPUS: &str = "typedef IntList = List<int>;\n\nclass Point extends Base implements Draw {\n  int x = 0;\n  void move(int dx) {\n    x = x + dx;\n  }\n}\n\nenum Color { red, green }\n\nmixin Walker on Base {\n  void walk() {}\n}\n\ntypedef Handler = int Function(String name);\n\ntypedef Pred = bool Function()?;\n\nvoid top(int callback(String value)) {}\n\nvoid legacy(int hook()?) {}\n";

    /// A recovery-error corpus: the class header never closes.
    const BROKEN: &str = "class {\n";

    /// The recorder's event sequence when it observes the unit alone.
    fn solo(source: &'static str) -> Vec<&'static str> {
        let mut p = parser(source, Recorder::default());
        p.parse_unit();
        std::mem::take(&mut p.listener.events)
    }

    #[test]
    fn fan_out_equals_sequential_replay() {
        for source in [CORPUS, BROKEN] {
            let mut a = Recorder::default();
            let mut b = Recorder::default();
            let mut c = Recorder::default();
            let mut inner = MultiplexListener::new(&mut b, &mut c);
            let mut mux = MultiplexListener::new(&mut a, &mut inner);
            let mut p = parser_ref(source, &mut mux);
            p.parse_unit();
            let expected = solo(source);
            assert!(!expected.is_empty(), "corpus fired no events: {source:?}");
            assert_eq!(a.events, expected, "receiver a diverged");
            assert_eq!(b.events, expected, "receiver b diverged");
            assert_eq!(c.events, expected, "receiver c diverged");
        }
    }

    #[test]
    fn null_listener_slot_is_the_empty_fan_out() {
        let mut r = Recorder::default();
        let mut empty = NullListener;
        let mut mux = MultiplexListener::new(&mut r, &mut empty);
        let mut p = parser_ref(CORPUS, &mut mux);
        p.parse_unit();
        assert_eq!(r.events, solo(CORPUS));
    }

    #[test]
    fn receiver_order_is_deterministic() {
        // The interleaving pin: a's log is a prefix-disjoint copy of the
        // same stream b sees — equal to each other and to the solo run.
        let mut a = Recorder::default();
        let mut b = Recorder::default();
        let mut mux = MultiplexListener::new(&mut a, &mut b);
        let mut p = parser_ref(CORPUS, &mut mux);
        p.parse_unit();
        assert_eq!(a.events, b.events);
        assert_eq!(a.events, solo(CORPUS));
    }

    #[test]
    fn function_type_events_fan_out_to_both_sides() {
        // The forwarding-coverage pin: these three events have in-tree
        // consumers overriding them, so a missing forward falls through to
        // the default no-op and silently starves every receiver.
        let mut a = Recorder::default();
        let mut b = Recorder::default();
        let mut mux = MultiplexListener::new(&mut a, &mut b);
        let mut p = parser_ref(CORPUS, &mut mux);
        p.parse_unit();
        for event in [
            "begin_function_typed_formal_parameter",
            "end_function_typed_formal_parameter",
            "end_function_type",
        ] {
            assert!(a.events.contains(&event), "receiver a missed {event}");
            assert!(b.events.contains(&event), "receiver b missed {event}");
        }
    }

    /// Records the container-body events with their UTF-16 token offsets.
    /// The engine's index capture latches `decl_end`/`body_open` from these
    /// spans, so a starved receiver manifests downstream as the
    /// `DeclRow::UNKNOWN` sentinel.
    #[derive(Default)]
    struct BodySpy {
        events: Vec<String>,
    }

    impl ParserListener for BodySpy {
        fn begin_class_or_mixin_or_extension_body(&mut self, _: DeclarationKind, token: Token) {
            self.events
                .push(format!("begin_class_body@{}", token.range.offset));
        }
        fn end_class_or_mixin_or_extension_body(
            &mut self,
            _: DeclarationKind,
            count: usize,
            begin: Token,
            end: Token,
        ) {
            self.events.push(format!(
                "end_class_body@{}..{} x{}",
                begin.range.offset, end.range.offset, count
            ));
        }
        fn begin_enum_body(&mut self, token: Token) {
            self.events
                .push(format!("begin_enum_body@{}", token.range.offset));
        }
        fn end_enum_body(&mut self, begin: Token, end: Token) {
            let (begin, end) = (begin.range.offset, end.range.offset);
            self.events.push(format!("end_enum_body@{begin}..{end}"));
        }
    }

    #[test]
    fn container_body_events_fan_out_with_correct_spans() {
        // Offsets: `class C {` opens at 8 and closes at 24 (one member);
        // `enum E {` opens at 34 and closes at 41.
        const BODY_CORPUS: &str = "class C {\n  void m() {}\n}\n\nenum E { a, b }";
        let expected = vec![
            "begin_class_body@8".to_string(),
            "end_class_body@8..24 x1".to_string(),
            "begin_enum_body@34".to_string(),
            "end_enum_body@34..41".to_string(),
        ];
        let mut a = BodySpy::default();
        let mut b = BodySpy::default();
        let mut mux = MultiplexListener::new(&mut a, &mut b);
        let mut p = parser_ref(BODY_CORPUS, &mut mux);
        p.parse_unit();
        assert_eq!(a.events, expected, "receiver a");
        assert_eq!(b.events, expected, "receiver b");
    }
}
