//! The [`ParserListener`] event contract.
//!
//! One trait with all-default methods, mirroring the official single
//! `Listener` class (`listener.dart:45`, 379 methods — the core spec chose
//! the same shape for the same reasons: builders span domains, and the
//! official `ForwardingListener`/`NullListener` prove the "one big
//! interface + empty defaults" form). Events are emitted in the official
//! order, so event-sequence goldens map 1:1 onto the oracle.
//!
//! This milestone declares the events its grammar surface produces (types,
//! metadata, patterns, the reduced constant-expression engine, function-type
//! formal parameters, recovery). Later grammar milestones (#18/#19/#20)
//! extend the trait with their events — adding methods with default bodies
//! is a non-breaking extension for existing listeners.
//!
//! Naming: official lowerCamelCase mechanically transliterated to
//! `snake_case` (`beginClassDeclaration` → `begin_class_declaration`); no
//! invented names. Token positions carry [`TokenSpan`].

use crate::message::Message;
use crate::token_id::TokenSpan;

/// Alias so event signatures read like the official `Token` parameters.
pub type Token = TokenSpan;

/// `PatternContext` (`parser_impl.dart:12576`): refutability and binding
/// behavior of the pattern being parsed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PatternContext {
    /// Part of a local variable declaration or for-in parts: bare
    /// identifiers declare fresh variables.
    Declaration,
    /// Part of a guarded pattern (if-case, switch expression, switch
    /// statement): bare identifiers refer to constants.
    Matching,
    /// Part of a pattern assignment: bare identifiers refer to previously
    /// declared variables.
    Assignment,
}

impl PatternContext {
    /// Official `isRefutable`.
    #[must_use]
    pub fn is_refutable(self) -> bool {
        match self {
            PatternContext::Declaration | PatternContext::Assignment => false,
            PatternContext::Matching => true,
        }
    }
}

/// `ConstantPatternContext` (`parser_impl.dart:12599`): how a constant
/// pattern operand is being parsed, driving the deliberate over-collection
/// diagnostics of `_checkForInvalidConstantPatternOperator`
/// (`parser_impl.dart:7507`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConstantPatternContext {
    /// Not parsing a constant pattern.
    None,
    /// Bare constant pattern (implicit `const`).
    Implicit,
    /// `const`-prefixed constant pattern.
    Explicit,
    /// Unary minus over a numeric literal only.
    NumericLiteralOnly,
}

/// `BlockKind` (`block_kind.dart`), statement-scope kinds only — declaration
/// bodies use `DeclarationKind`/member events instead.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockKind {
    /// A plain `{ ... }` statement block.
    Statement,
    /// A function body block.
    FunctionBody,
    /// The block after `try`.
    TryStatement,
    /// The block after `on type` / `catch (...)`.
    CatchClause,
    /// The block after `finally`.
    FinallyClause,
    /// The block after a switch statement's condition.
    SwitchStatement,
}

/// `Assert` (`assert.dart`): the position an assertion is parsed in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AssertKind {
    /// Constructor initializer `assert`.
    Initializer,
    /// An `assert;` statement.
    Statement,
    /// `assert` in expression position.
    Expression,
}

/// `LoopState` (`loop_state.dart`): break/continue admissibility of the
/// enclosing construct.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoopState {
    /// Not inside a loop or switch.
    OutsideLoop,
    /// Inside a loop body.
    InsideLoop,
    /// Inside a switch statement (but not a loop).
    InsideSwitch,
}

/// `ConstructorReferenceContext` (`constructor_reference_context.dart`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConstructorReferenceContext {
    /// Explicit `new` constructor invocation.
    New,
    /// Explicit `const` constructor invocation.
    Const,
    /// Implicit `new`/`const` constructor invocation.
    Implicit,
    /// Redirecting factory body.
    RedirectingFactory,
}

/// `FormalParameterKind` (`formal_parameter_kind.dart`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FormalParameterKind {
    RequiredPositional,
    RequiredNamed,
    OptionalNamed,
    OptionalPositional,
}

/// `MemberKind` (`member_kind.dart`), as far as this milestone's grammar
/// surface reaches; later milestones extend usage, not variants.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MemberKind {
    /// A catch block, not a real member.
    Catch,
    /// A factory.
    Factory,
    /// Old-style typedef.
    FunctionTypeAlias,
    /// Old-style function-typed parameter, not a real member.
    FunctionTypedParameter,
    /// A generalized function type, not a real member.
    GeneralizedFunctionType,
    /// A local function.
    Local,
    /// An anonymous method.
    AnonymousMethod,
    /// A non-static method in a class (including constructors).
    NonStaticMethod,
    /// A static method in a class.
    StaticMethod,
    /// A top-level method.
    TopLevelMethod,
    /// A non-static method in an extension.
    ExtensionNonStaticMethod,
    /// A static method in an extension.
    ExtensionStaticMethod,
    /// A non-static method or constructor in an extension type.
    ExtensionTypeNonStaticMethod,
    /// A static method in an extension type.
    ExtensionTypeStaticMethod,
    /// An instance field in a class.
    NonStaticField,
    /// A static field in a class.
    StaticField,
    /// A top-level field.
    TopLevelField,
    /// Primary constructor.
    PrimaryConstructor,
}

/// `DeclarationKind` (`declaration_kind.dart`): the declaration family a
/// member or header belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeclarationKind {
    /// A top-level declaration.
    TopLevel,
    /// A class declaration.
    Class,
    /// A mixin declaration.
    Mixin,
    /// An enum declaration.
    Enum,
    /// An extension declaration.
    Extension,
    /// An extension type declaration.
    ExtensionType,
}

/// `DeclarationHeaderKind` (`parser_impl.dart`): header shape during
/// out-of-order clause recovery; only class and extension type headers run
/// that recovery today.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeclarationHeaderKind {
    /// A class declaration header.
    Class,
    /// An extension type declaration header.
    ExtensionType,
}

/// `IdentifierContext` (`identifier_context.dart`) subset for this
/// milestone's grammar surface; later milestones extend usage, not variants
/// beyond their needs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IdentifierContext {
    /// `metadataReference`.
    MetadataReference,
    /// `metadataContinuation`.
    MetadataContinuation,
    /// `metadataContinuationAfterTypeArguments`.
    MetadataContinuationAfterTypeArguments,
    /// `typeReference`.
    TypeReference,
    /// `prefixedTypeReference`.
    PrefixedTypeReference,
    /// `typeReferenceContinuation`.
    TypeReferenceContinuation,
    /// `typeVariableDeclaration`.
    TypeVariableDeclaration,
    /// `recordFieldDeclaration`.
    RecordFieldDeclaration,
    /// `formalParameterDeclaration`.
    FormalParameterDeclaration,
    /// `namedRecordFieldReference`.
    NamedRecordFieldReference,
    /// `namedArgumentReference`.
    NamedArgumentReference,
    /// `localVariableDeclaration`.
    LocalVariableDeclaration,
    /// `literalSymbol`: the first identifier of a literal symbol's dotted
    /// chain (`foo` in `#foo.bar`).
    LiteralSymbol,
    /// `literalSymbolContinuation`: an identifier of a literal symbol that
    /// follows a `.` (`bar` in `#foo.bar`).
    LiteralSymbolContinuation,
    /// `expression`.
    Expression,
    /// `expressionContinuation`.
    ExpressionContinuation,
    /// `typedefDeclaration`.
    TypedefDeclaration,
    /// `topLevelVariableDeclaration`.
    TopLevelVariableDeclaration,
    /// `topLevelFunctionDeclaration`.
    TopLevelFunctionDeclaration,
    /// `fieldDeclaration`.
    FieldDeclaration,
    /// `fieldInitializer`.
    FieldInitializer,
    /// `methodDeclaration`.
    MethodDeclaration,
    /// `methodDeclarationContinuation`.
    MethodDeclarationContinuation,
    /// `operatorName`.
    OperatorName,
    /// `classOrMixinOrExtensionDeclaration`.
    ClassOrMixinOrExtensionDeclaration,
    /// `enumDeclaration`.
    EnumDeclaration,
    /// `enumValueDeclaration`.
    EnumValueDeclaration,
    /// `constructorReference`.
    ConstructorReference,
    /// `constructorReferenceContinuation`.
    ConstructorReferenceContinuation,
    /// `constructorReferenceContinuationAfterTypeArguments`.
    ConstructorReferenceContinuationAfterTypeArguments,
    /// `primaryConstructorDeclaration`.
    PrimaryConstructorDeclaration,
    /// `localFunctionDeclaration`.
    LocalFunctionDeclaration,
    /// `labelDeclaration`.
    LabelDeclaration,
    /// `labelReference`.
    LabelReference,
    /// `catchParameter`.
    CatchParameter,
    // —— directive identifier positions (#300) ———————————————————————————
    /// `libraryName` (`identifier_context.dart`): the first name of a
    /// `library` directive's qualified name.
    LibraryName,
    /// `libraryNameContinuation`: names after a `.` in a library name.
    LibraryNameContinuation,
    /// `importPrefixDeclaration`: the prefix name after `as`.
    ImportPrefixDeclaration,
    /// `dottedName`: the first identifier of a conditional-uri dotted name.
    DottedName,
    /// `dottedNameContinuation`: identifiers after a `.` in a dotted name.
    DottedNameContinuation,
    /// `combinator`: an identifier in a `show`/`hide` name list.
    Combinator,
    /// `partName`: the first name of a `part of` directive's qualified name.
    PartName,
    /// `partNameContinuation`: names after a `.` in a part-of name.
    PartNameContinuation,
}

impl IdentifierContext {
    /// Contexts whose official `ensureIdentifier` runs
    /// `checkAsyncAwaitYieldAsIdentifier` (`identifier_context_impl.dart`):
    /// `await`/`yield` in these positions report
    /// `async_keyword_used_as_identifier` inside async functions.
    pub(crate) fn checks_async_keyword(self) -> bool {
        matches!(
            self,
            Self::CatchParameter
                | Self::ConstructorReference
                | Self::ConstructorReferenceContinuationAfterTypeArguments
                | Self::Expression
                | Self::ExpressionContinuation
                | Self::FormalParameterDeclaration
                | Self::RecordFieldDeclaration
                | Self::LocalFunctionDeclaration
                | Self::LabelDeclaration
                | Self::LabelReference
                | Self::LocalVariableDeclaration
                | Self::MetadataReference
                | Self::MetadataContinuation
                | Self::NamedArgumentReference
                | Self::NamedRecordFieldReference
        )
    }
}

/// Experimental feature flag referenced by
/// [`ParserListener::handle_experiment_not_enabled`]; mirrors the flags the
/// parser consults (`ExperimentalFlag` in the SDK experiments registry).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExperimentalFlag {
    TripleShift,
    Patterns,
    Variance,
    Augmentations,
    PrimaryConstructors,
}

/// Parser event sink. Every method has an empty default (official
/// `NullListener` semantics), so implementers override only what they need.
///
/// `begin*`/`end*` pairs bracket a structure; `handle*` events stand alone
/// (official contract, listener.dart:46-57). End events carry `count`s where
/// the official ones do — the AST builder uses them to pop stacks
///.
#[allow(unused_variables)]
pub trait ParserListener {
    // —— compilation unit ————————————————————————————————————————————
    fn begin_compilation_unit(&mut self, token: Token) {}
    fn end_compilation_unit(&mut self, count: usize, token: Token) {}

    // —— directives (#300) ———————————————————————————————————————————————
    /// `beginLibraryName(libraryKeyword)`.
    fn begin_library_name(&mut self, token: Token) {}
    /// `endLibraryName(libraryKeyword, semicolon, hasName)`.
    fn end_library_name(&mut self, library_keyword: Token, semicolon: Token, has_name: bool) {}
    /// `beginLibraryAugmentation(augmentKeyword, libraryKeyword)`.
    fn begin_library_augmentation(&mut self, augment_kw: Token, library_keyword: Token) {}
    /// `endLibraryAugmentation(augmentKeyword, libraryKeyword, semicolon)`.
    fn end_library_augmentation(
        &mut self,
        augment_kw: Token,
        library_keyword: Token,
        semicolon: Token,
    ) {
    }
    /// `beginImport(importKeyword)`.
    fn begin_import(&mut self, import_keyword: Token) {}
    /// `endImport(importKeyword, semicolon)`; `None` when
    /// [`ParserListener::handle_recover_import`] takes over the tail
    /// (official `endImport(importKeyword, /* semicolon = */ null)`).
    fn end_import(&mut self, import_keyword: Token, semicolon: Option<Token>) {}
    /// `handleImportPrefix(deferredKeyword, asKeyword)`: the prefix face of
    /// the current import; either token may be absent.
    fn handle_import_prefix(&mut self, deferred_kw: Option<Token>, as_kw: Option<Token>) {}
    /// `handleRecoverImport(semicolon)`: one out-of-order-clause recovery
    /// pass after `endImport(_, None)` finished (official `listener.dart:997`).
    fn handle_recover_import(&mut self, semicolon: Option<Token>) {}
    /// `beginConditionalUris(token)`.
    fn begin_conditional_uris(&mut self, token: Token) {}
    /// `endConditionalUris(count)`.
    fn end_conditional_uris(&mut self, count: usize) {}
    /// `beginConditionalUri(ifKeyword)`.
    fn begin_conditional_uri(&mut self, if_kw: Token) {}
    /// `endConditionalUri(ifKeyword, leftParen, equalSign)`.
    fn end_conditional_uri(&mut self, if_kw: Token, left_paren: Token, equal_sign: Option<Token>) {}
    /// `handleDottedName(count, firstIdentifier)`.
    fn handle_dotted_name(&mut self, count: usize, first_identifier: Token) {}
    /// `beginExport(exportKeyword)`.
    fn begin_export(&mut self, token: Token) {}
    /// `endExport(exportKeyword, semicolon)`.
    fn end_export(&mut self, export_keyword: Token, semicolon: Token) {}
    /// `beginCombinators(token)`.
    fn begin_combinators(&mut self, token: Token) {}
    /// `endCombinators(count)`.
    fn end_combinators(&mut self, count: usize) {}
    /// `beginShow(showKeyword)`.
    fn begin_show(&mut self, show_keyword: Token) {}
    /// `endShow(showKeyword)`.
    fn end_show(&mut self, show_keyword: Token) {}
    /// `beginHide(hideKeyword)`.
    fn begin_hide(&mut self, hide_keyword: Token) {}
    /// `endHide(hideKeyword)`.
    fn end_hide(&mut self, hide_keyword: Token) {}
    /// `handleIdentifierList(count)`: the name list of one show/hide.
    fn handle_identifier_list(&mut self, count: usize) {}
    /// `beginPart(partKeyword)`.
    fn begin_part(&mut self, token: Token) {}
    /// `endPart(partKeyword, semicolon)`.
    fn end_part(&mut self, part_keyword: Token, semicolon: Token) {}
    /// `beginPartOf(partKeyword)`.
    fn begin_part_of(&mut self, token: Token) {}
    /// `endPartOf(partKeyword, ofKeyword, semicolon, hasName)`: `hasName`
    /// selects the qualified-name form over the URI form.
    fn end_part_of(&mut self, part_keyword: Token, of_kw: Token, semicolon: Token, has_name: bool) {
    }

    // —— metadata (D1/D2) ————————————————————————————————————————————
    fn begin_metadata_star(&mut self, token: Token) {}
    fn end_metadata_star(&mut self, count: usize) {}
    fn begin_metadata(&mut self, token: Token) {}
    fn end_metadata(&mut self, begin: Token, period_before_name: Option<Token>, end: Token) {}

    // —— types (C49-C56) ——————————————————————————————————————————————
    /// `handleType`: a parsed type reference; `question` is the trailing `?`
    /// when it is part of the type.
    fn handle_type(&mut self, name: Token, question: Option<Token>) {}
    /// `handleNoType`: a required-but-absent type.
    fn handle_no_type(&mut self, last_consumed: Token) {}
    /// `handleQualified`: the `.` of an `id.id` reference, after both
    /// identifier events.
    fn handle_qualified(&mut self, period: Token) {}
    fn handle_identifier(&mut self, token: Token, context: IdentifierContext) {}
    fn begin_type_arguments(&mut self, token: Token) {}
    fn end_type_arguments(&mut self, count: usize, begin: Token, end: Token) {}
    fn handle_no_type_arguments(&mut self, token: Token) {}
    fn begin_type_variables(&mut self, token: Token) {}
    fn end_type_variables(&mut self, begin: Token, end: Token) {}
    fn handle_no_type_variables(&mut self, token: Token) {}
    fn handle_type_variables_defined(&mut self, token: Token, count: usize) {}
    fn begin_type_variable(&mut self, token: Token) {}
    fn end_type_variable(
        &mut self,
        token: Token,
        index: usize,
        extends_or_super: Option<Token>,
        variance: Option<Token>,
    ) {
    }
    fn begin_record_type(&mut self, left_paren: Token) {}
    fn end_record_type(
        &mut self,
        left_paren: Token,
        question: Option<Token>,
        count: usize,
        has_named_fields: bool,
    ) {
    }
    /// The official events are tokenless (`listener.dart:1639/:1647`);
    /// the entry's token bounds ride along so span-building listeners need
    /// no private token stack (the official `AstBuilder` keeps one).
    fn begin_record_type_entry(&mut self, first: Token) {}
    fn end_record_type_entry(&mut self, last: Token) {}
    fn handle_no_name(&mut self, token: Token) {}
    fn begin_record_type_named_fields(&mut self, left_brace: Token) {}
    fn end_record_type_named_fields(&mut self, count: usize, left_brace: Token) {}
    fn begin_function_type(&mut self, begin: Token) {}
    fn end_function_type(&mut self, function: Token, question: Option<Token>) {}
    fn handle_void_keyword(&mut self, token: Token) {}
    fn handle_void_keyword_with_type_arguments(&mut self, token: Token) {}

    // —— reduced constant-expression engine (until #20) —————————————————
    fn handle_literal_int(&mut self, token: Token) {}
    fn handle_literal_int_with_separators(&mut self, token: Token) {}
    fn handle_literal_double(&mut self, token: Token) {}
    fn handle_literal_double_with_separators(&mut self, token: Token) {}
    fn handle_literal_bool(&mut self, token: Token) {}
    fn handle_literal_null(&mut self, token: Token) {}
    fn begin_literal_string(&mut self, token: Token) {}
    fn handle_interpolation_expression(&mut self, left: Token, right: Option<Token>) {}
    fn end_literal_string(&mut self, interpolation_count: usize, end: Token) {}
    fn handle_adjacent_string_literals(&mut self, start: Token, literal_count: usize) {}
    fn begin_literal_symbol(&mut self, token: Token) {}
    fn handle_symbol_void(&mut self, token: Token) {}
    fn end_literal_symbol(&mut self, hash: Token, identifier_count: usize) {}
    /// `handleOperator` (listener.dart:2122): a bare operator token. In
    /// `parseLiteralSymbol` this carries the body of an operator symbol
    /// (`~` in `#~`), followed by `end_literal_symbol(hash, 1)`
    /// (parser_impl.dart:9252-9254).
    fn handle_operator(&mut self, token: Token) {}
    fn handle_this_expression(&mut self, token: Token, context: IdentifierContext) {}
    fn handle_super_expression(&mut self, token: Token, context: IdentifierContext) {}
    fn handle_unary_prefix_expression(&mut self, operator: Token) {}
    fn handle_non_null_assert_expression(&mut self, bang: Token) {}
    fn begin_binary_expression(&mut self, operator: Token) {}
    fn end_binary_expression(&mut self, operator: Token, end: Token) {}
    fn begin_const_expression(&mut self, const_keyword: Token) {}
    fn end_const_expression(&mut self, token: Token) {}
    fn begin_const_literal(&mut self, token: Token) {}
    fn end_const_literal(&mut self, end: Token) {}
    fn handle_literal_list(
        &mut self,
        count: usize,
        left: Token,
        const_keyword: Option<Token>,
        right: Token,
    ) {
    }
    /// A set/map literal opened: fires right after the `{` is consumed,
    /// before any element expression event. No official counterpart — the
    /// semantic face opens its non-const duplicate-detection window here
    /// (#747), which needs the literal's const keyword before the elements
    /// fold.
    fn begin_literal_set_or_map(&mut self, left: Token, const_keyword: Option<Token>) {}
    fn handle_literal_set_or_map(
        &mut self,
        count: usize,
        left: Token,
        const_keyword: Option<Token>,
        right: Token,
        has_set_entry: bool,
    ) {
    }
    fn handle_literal_map_entry(
        &mut self,
        colon: Token,
        end: Token,
        null_aware_key: Option<Token>,
        null_aware_value: Option<Token>,
    ) {
    }
    fn begin_new_expression(&mut self, token: Token) {}
    fn end_new_expression(&mut self, token: Token) {}
    fn begin_parenthesized_expression_or_record_literal(&mut self, token: Token) {}
    fn end_parenthesized_expression(&mut self, token: Token) {}
    fn end_record_literal(&mut self, token: Token, count: usize, const_keyword: Option<Token>) {}
    fn handle_named_record_field(&mut self, colon: Token) {}
    fn handle_positional_record_field(&mut self, token: Token) {}
    fn begin_arguments(&mut self, token: Token) {}
    fn end_arguments(&mut self, count: usize, begin: Token, end: Token) {}
    fn handle_no_arguments(&mut self, token: Token) {}
    fn handle_named_argument(&mut self, colon: Token) {}
    fn handle_positional_argument(&mut self, token: Token) {}
    fn handle_send(&mut self, begin: Token, end: Token) {}
    fn handle_send_without_arguments(&mut self, begin: Token, end: Token, next: Token) {}
    fn handle_dot_access(&mut self, token: Token, end: Token, is_null_aware: bool) {}
    /// Flow-face extension fired the moment the `?[` of a null-aware index
    /// selector is consumed — before the index operand parses. The official
    /// analyzer's null-shorting begins at the `?` (`nullAwareAccess_rightBegin`,
    /// flow_analysis.dart:9199): the operand runs on the not-null path, so a
    /// `throw`/Never inside it must not kill the skip path the listener is
    /// asked to capture here. No official counterpart (the official parser
    /// reports the whole index at `handleIndexedExpression`); existing
    /// listeners ignore it.
    fn handle_null_aware_index_operand_begin(&mut self, question: Token) {}
    /// Flow-face extension fired the moment the `?.` of a null-aware member
    /// selector is consumed — before the member name (and any invocation
    /// arguments on it) parses. Same official anchor as the index face
    /// (`nullAwareAccess_rightBegin`, flow_analysis.dart:9199): a
    /// `throw`/Never inside `x?.m(throw _)`'s argument list must not kill
    /// the skip path. No official counterpart; existing listeners ignore it.
    fn handle_null_aware_member_begin(&mut self, question: Token) {}
    fn handle_indexed_expression(&mut self, question: Option<Token>, open: Token, close: Token) {}
    fn begin_constructor_reference(&mut self, start: Token) {}
    fn handle_no_constructor_reference_continuation_after_type_arguments(&mut self, token: Token) {}
    fn end_constructor_reference(
        &mut self,
        start: Token,
        period_before_name: Option<Token>,
        end: Token,
        context: ConstructorReferenceContext,
    ) {
    }
    fn begin_await_expression(&mut self, token: Token) {}
    fn end_await_expression(&mut self, begin: Token, end: Token) {}
    /// `endInvalidAwaitExpression(begin, end, errorCode)`; the error code is
    /// resolution-domain and never a TSV emission (`await` in a sync
    /// function), mirroring `endInvalidYieldStatement`.
    fn end_invalid_await_expression(&mut self, begin: Token, end: Token) {}
    fn handle_throw_expression(&mut self, throw: Token, end: Token) {}
    /// `handleAssignmentExpression(operator, end)` (C7).
    fn handle_assignment_expression(&mut self, operator: Token, end: Token) {}
    /// `beginCascade(operator)` / `handleCascadeAccess(operator, end,
    /// isNullAware)` / `endCascade` (C8).
    fn begin_cascade(&mut self, operator: Token) {}
    fn handle_cascade_access(&mut self, operator: Token, end: Token, is_null_aware: bool) {}
    fn end_cascade(&mut self) {}
    /// `beginConditionalExpression(question)` /
    /// `handleConditionalExpressionColon` /
    /// `endConditionalExpression(question, colon, end)` (C9).
    fn begin_conditional_expression(&mut self, question: Token) {}
    fn handle_conditional_expression_colon(&mut self) {}
    fn end_conditional_expression(&mut self, question: Token, colon: Token, end: Token) {}
    /// Flow-face extension fired immediately before
    /// [`ParserListener::handle_conditional_expression_colon`]: the then
    /// arm's first token and the colon-adjacent end. The official
    /// dead-arm rows span the arm's own range
    /// (`flowEnd(thenExpression)`, `type_analyzer.dart:2588`), which the
    /// argless colon call cannot carry. No official counterpart; existing
    /// listeners ignore it.
    fn end_conditional_expression_then_arm(
        &mut self,
        _question: Token,
        _then_begin: Token,
        _colon: Token,
        _then_end: Token,
    ) {
    }
    /// Prefix `++`/`--` (`handleUnaryPrefixAssignmentExpression`); the plain
    /// unary prefixes `-`/`!`/`~` use [`ParserListener::handle_unary_prefix_expression`].
    fn handle_unary_prefix_assignment_expression(&mut self, operator: Token) {}
    /// Postfix `++`/`--` (`handleUnaryPostfixAssignmentExpression`).
    fn handle_unary_postfix_assignment_expression(&mut self, operator: Token) {}
    /// `beginFunctionExpression(token)` / `endFunctionExpression(begin, end)`:
    /// an anonymous function literal `(...) { ... }` (C22).
    fn begin_function_expression(&mut self, token: Token) {}
    fn end_function_expression(&mut self, begin: Token, end: Token) {}
    /// `beginNamedFunctionExpression(token)` /
    /// `endNamedFunctionExpression(end)`: the named-function-expression
    /// recovery shape `T name(...) { ... }` (C23).
    fn begin_named_function_expression(&mut self, token: Token) {}
    fn end_named_function_expression(&mut self, end: Token) {}
    /// Implicit-creation constructor invocation `C<T>(...)` (C17).
    fn begin_implicit_creation_expression(&mut self, token: Token) {}
    fn end_implicit_creation_expression(&mut self, begin: Token, open_angle: Token) {}
    /// `handleAugmentSuperExpression(augment, super, context)`.
    fn handle_augment_super_expression(
        &mut self,
        augment: Token,
        super_kw: Token,
        context: IdentifierContext,
    ) {
    }
    /// Dot-shorthand events (`.name` static-access shorthands):
    /// `handleDotShorthandHead(dot)` / `handleDotShorthandContext(dot)`;
    /// `const .name` brackets both with `begin/endConstDotShorthand`.
    fn handle_dot_shorthand_head(&mut self, dot: Token) {}
    fn handle_dot_shorthand_context(&mut self, dot: Token) {}
    fn begin_const_dot_shorthand(&mut self, const_keyword: Token) {}
    fn end_const_dot_shorthand(&mut self, const_keyword: Token) {}
    /// `handleInvalidTypeArguments(token)`: type arguments in a constructor
    /// invocation position (event only — the paired diagnostic is culled at
    /// the pinned oracle).
    fn handle_invalid_type_arguments(&mut self, token: Token) {}
    /// `handlePatternAssignment(equals)` (C46): the `=` of an outer-pattern
    /// assignment.
    fn handle_pattern_assignment(&mut self, equals: Token) {}
    /// `is` / `is!` type check (C33): begin/end bracket the type operand.
    fn begin_is_operator_type(&mut self, operator: Token) {}
    fn end_is_operator_type(&mut self, operator: Token) {}
    fn handle_is_operator(&mut self, is_operator: Token, not: Option<Token>) {}
    /// `as` cast (C34).
    fn begin_as_operator_type(&mut self, operator: Token) {}
    fn end_as_operator_type(&mut self, operator: Token) {}
    fn handle_as_operator(&mut self, operator: Token) {}
    fn handle_type_argument_application(&mut self, open_angle: Token) {}

    // —— patterns (C39-C45) ————————————————————————————————————————————
    fn begin_pattern(&mut self, token: Token) {}
    fn end_pattern(&mut self, token: Token) {}
    fn begin_constant_pattern(&mut self, const_keyword: Option<Token>) {}
    fn end_constant_pattern(&mut self, const_keyword: Option<Token>) {}
    fn handle_relational_pattern(&mut self, operator: Token) {}
    fn handle_cast_pattern(&mut self, operator: Token) {}
    fn handle_null_assert_pattern(&mut self, bang: Token) {}
    fn handle_null_check_pattern(&mut self, question: Token) {}
    fn begin_binary_pattern(&mut self, operator: Token) {}
    fn end_binary_pattern(&mut self, operator: Token) {}
    fn handle_list_pattern(&mut self, count: usize, left: Token, right: Token) {}
    fn handle_map_pattern(&mut self, count: usize, left: Token, right: Token) {}
    /// A `key: value` map-pattern entry's key expression is about to be
    /// parsed (`parseMapPatternSuffix`'s entry arm, parser_impl.dart:12087).
    /// The constant-evaluation face brackets the key in a capture window so
    /// a non-constant key reports `non_constant_map_pattern_key`
    /// (constant_verifier.dart:342-358 `_evaluateAndReportError`).
    fn begin_map_pattern_key(&mut self) {}
    fn handle_map_pattern_entry(&mut self, colon: Token, end: Token) {}
    fn handle_rest_pattern(&mut self, dots: Token, has_sub_pattern: bool) {}
    fn handle_record_pattern(&mut self, left_paren: Token, count: usize) {}
    fn handle_parenthesized_pattern(&mut self, left_paren: Token) {}
    fn handle_pattern_field(&mut self, colon: Option<Token>) {}
    fn handle_object_pattern(
        &mut self,
        first_identifier: Token,
        dot: Option<Token>,
        second_identifier: Option<Token>,
    ) {
    }
    fn handle_object_pattern_fields(&mut self, count: usize, begin: Token, end: Token) {}
    fn handle_wildcard_pattern(&mut self, keyword: Option<Token>, wildcard: Token) {}
    fn handle_assigned_variable_pattern(&mut self, variable: Token) {}
    fn handle_declared_variable_pattern(
        &mut self,
        keyword: Option<Token>,
        variable: Token,
        in_assignment_pattern: bool,
    ) {
    }

    // —— formal parameters (D7/D8) ——————————————————————————————————————
    /// Official `beginFormalParameters`.
    fn begin_formal_parameters(&mut self, token: Token, kind: MemberKind) {}
    fn end_formal_parameters(&mut self, count: usize, begin: Token, end: Token, kind: MemberKind) {}
    /// `[`-bracketed optional positional group (official
    /// `beginOptionalFormalParameters`; one event pair for both the `[` and
    /// `{` groups, like the official listener).
    fn begin_optional_formal_parameters(&mut self, token: Token) {}
    fn end_optional_formal_parameters(
        &mut self,
        count: usize,
        begin: Token,
        end: Token,
        kind: MemberKind,
    ) {
    }
    /// Official `beginFormalParameter(start, memberKind, requiredToken,
    /// covariantToken, varFinalOrConst)`.
    fn begin_formal_parameter(
        &mut self,
        token: Token,
        kind: MemberKind,
        required: Option<Token>,
        covariant_kw: Option<Token>,
        var_final_or_const: Option<Token>,
    ) {
    }
    /// Inline function-typed formal parameter (`void f(int g(String x))`,
    /// official `beginFunctionTypedFormalParameter`).
    fn begin_function_typed_formal_parameter(&mut self, token: Token) {}
    fn end_function_typed_formal_parameter(&mut self, token: Token, question: Option<Token>) {}
    fn begin_formal_parameter_default_value_expression(&mut self, token: Token) {}
    fn end_formal_parameter_default_value_expression(&mut self, token: Token) {}
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
        member_kind: MemberKind,
    ) {
    }
    fn handle_valued_formal_parameter(
        &mut self,
        equals: Token,
        end: Token,
        kind: FormalParameterKind,
    ) {
    }
    fn handle_formal_parameter_without_value(&mut self, token: Token) {}
    fn handle_no_formal_parameters(&mut self, token: Token, kind: MemberKind) {}

    // —— declarations (top-level dispatch, #18) ——————————————————————————
    /// `endTopLevelDeclaration`: one top-level declaration has been parsed.
    fn end_top_level_declaration(&mut self, end: Token) {}
    /// `handleInvalidTopLevelDeclaration`: an unparseable top-level token
    /// sequence was consumed by recovery.
    fn handle_invalid_top_level_declaration(&mut self, end: Token) {}
    /// `beginUncategorizedTopLevelDeclaration` (typedef entry event).
    fn begin_uncategorized_top_level_declaration(&mut self, token: Token) {}
    /// `beginTopLevelMember`: a top-level field or function starts.
    fn begin_top_level_member(&mut self, token: Token) {}
    /// `beginTopLevelMethod(lastConsumed, augmentToken, externalToken)`.
    fn begin_top_level_method(
        &mut self,
        last_consumed: Token,
        augment: Option<Token>,
        external: Option<Token>,
    ) {
    }
    /// `endTopLevelMethod(begin, getOrSet, end)`.
    fn end_top_level_method(&mut self, begin: Token, get_or_set: Option<Token>, end: Token) {}
    /// `endTopLevelFields` (top-level variable declarations).
    #[allow(clippy::too_many_arguments)]
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
    }
    /// `beginTypedef`.
    fn begin_typedef(&mut self, token: Token) {}
    /// `endTypedef(augmentToken, typedefKeyword, equals, end)`.
    fn end_typedef(
        &mut self,
        augment: Option<Token>,
        typedef: Token,
        equals: Option<Token>,
        end: Token,
    ) {
    }

    // —— class/mixin/extension/enum members (#18) ————————————————————————
    /// `beginMember`/`endMember`: one class member.
    fn begin_member(&mut self) {}
    fn end_member(&mut self) {}
    /// `handleInvalidMember`: an unparseable member was consumed by recovery.
    fn handle_invalid_member(&mut self, end: Token) {}
    /// `beginFields(declarationKind, ..., lastConsumed)`; the modifier tokens
    /// precede the field group.
    #[allow(clippy::too_many_arguments)]
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
    }
    /// `endFields(kind, ..., count, begin, end)`.
    #[allow(clippy::too_many_arguments)]
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
    }
    /// `beginConstructor(...)`; `name` is the declared name token (possibly
    /// `new`), `enclosing_declaration_name` the lexical class name when the
    /// constructor decision needs it.
    #[allow(clippy::too_many_arguments)]
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
    }
    /// `endConstructor(kind, begin, newToken, beginParam, beginInitializers, end)`.
    #[allow(clippy::too_many_arguments)]
    fn end_constructor(
        &mut self,
        kind: DeclarationKind,
        begin: Token,
        new_kw: Option<Token>,
        begin_param: Token,
        begin_initializers: Option<Token>,
        end: Token,
    ) {
    }
    /// `beginMethod(...)` for a non-constructor member method.
    #[allow(clippy::too_many_arguments)]
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
    }
    /// `endMethod(kind, getOrSet, begin, beginParam, beginInitializers, end)`.
    #[allow(clippy::too_many_arguments)]
    fn end_method(
        &mut self,
        kind: DeclarationKind,
        get_or_set: Option<Token>,
        begin: Token,
        begin_param: Token,
        begin_initializers: Option<Token>,
        end: Token,
    ) {
    }
    /// `beginFactory(kind, lastConsumed, augmentToken, externalToken, constToken)`.
    fn begin_factory(
        &mut self,
        kind: DeclarationKind,
        last_consumed: Token,
        augment_kw: Option<Token>,
        external_kw: Option<Token>,
        const_keyword: Option<Token>,
    ) {
    }
    /// `endFactory(kind, begin, factoryKeyword, end)`.
    fn end_factory(&mut self, kind: DeclarationKind, begin: Token, factory: Token, end: Token) {}
    /// `handleOperatorName(operatorKeyword, token)` for `operator <op>` names.
    fn handle_operator_name(&mut self, operator: Token, name: Token) {}
    /// `handleInvalidOperatorName(operatorKeyword, token)`.
    fn handle_invalid_operator_name(&mut self, operator: Token, name: Token) {}
    /// `handleNoIdentifier(token, context)`: a required identifier is absent.
    fn handle_no_identifier(&mut self, token: Token, context: IdentifierContext) {}

    // —— field initializers and constructor initializers —————————————————
    /// `beginFieldInitializer`/`endFieldInitializer(assignment, end)`.
    fn begin_field_initializer(&mut self, token: Token) {}
    fn end_field_initializer(&mut self, assignment: Token, end: Token) {}
    fn handle_no_field_initializer(&mut self, token: Token) {}
    /// `beginInitializers`/`endInitializers(count, begin, end)` (constructor
    /// `: a = 1, b = 2` lists).
    fn begin_initializers(&mut self, token: Token) {}
    fn end_initializers(&mut self, count: usize, begin: Token, end: Token) {}
    fn handle_no_initializers(&mut self) {}

    // —— async modifiers and function bodies (diet, #18) ——————————————————
    /// `handleAsyncModifier(asyncToken, starToken)`.
    fn handle_async_modifier(&mut self, async_kw: Option<Token>, star: Option<Token>) {}
    /// `handleNoFunctionBody(token)`: an abstract (`;`) body.
    fn handle_no_function_body(&mut self, token: Token) {}
    /// `handleFunctionBodySkipped(begin, end, isExpressionBody)`: the diet
    /// body was consumed without statement-level parsing.
    fn handle_function_body_skipped(&mut self, begin: Token, end: Token, is_expression_body: bool) {
    }
    /// `handleEmptyFunctionBody(semicolon)`: the full-parser `;` body event.
    fn handle_empty_function_body(&mut self, semicolon: Token) {}
    /// `handleNativeClause(nativeToken, hasName)`.
    fn handle_native_clause(&mut self, native: Token, has_name: bool) {}
    /// `handleNativeFunctionBodySkipped(nativeToken, semicolon)`.
    fn handle_native_function_body_skipped(&mut self, native: Token, semicolon: Token) {}
    /// `handleNativeFunctionBodyIgnored(nativeToken, next)`.
    fn handle_native_function_body_ignored(&mut self, native: Token, next: Token) {}

    // —— declaration headers (extends/with/implements/on) ——————————————————
    /// `beginTypeList`/`endTypeList(count)` around a `,`-separated type list.
    fn begin_type_list(&mut self, token: Token) {}
    fn end_type_list(&mut self, count: usize) {}
    /// `handleNamedMixinApplicationWithClause(withKeyword)`.
    fn handle_named_mixin_application_with_clause(&mut self, with_keyword: Token) {}
    /// `handleClassWithClause`/`handleClassNoWithClause`.
    fn handle_class_with_clause(&mut self, with_keyword: Token) {}
    fn handle_class_no_with_clause(&mut self) {}
    /// `handleEnumWithClause`/`handleEnumNoWithClause`.
    fn handle_enum_with_clause(&mut self, with_keyword: Token) {}
    fn handle_enum_no_with_clause(&mut self) {}
    /// `handleImplements(implementsKeyword, interfacesCount)`.
    fn handle_implements(&mut self, implements: Option<Token>, count: usize) {}
    /// `handleClassExtends(extendsKeyword, typeCount)`.
    fn handle_class_extends(&mut self, extends: Option<Token>, count: usize) {}
    /// `handleClassHeader(begin, classKeyword, nativeToken)`.
    fn handle_class_header(&mut self, begin: Token, class: Token, native: Option<Token>) {}
    /// `handleMixinHeader(mixinKeyword)`.
    fn handle_mixin_header(&mut self, mixin: Token) {}
    /// `handleMixinOn(onKeyword, typeCount)`.
    fn handle_mixin_on(&mut self, on: Option<Token>, count: usize) {}
    /// `handleRecoverDeclarationHeader(kind)`: one recovery pass over
    /// out-of-order header clauses finished.
    fn handle_recover_declaration_header(&mut self, kind: DeclarationHeaderKind) {}
    /// `handleRecoverMixinHeader()`.
    fn handle_recover_mixin_header(&mut self) {}

    // —— class declarations ————————————————————————————————————————————
    /// `beginClassOrMixinOrNamedMixinApplicationPrelude(token)`.
    fn begin_class_or_mixin_or_named_mixin_application_prelude(&mut self, token: Token) {}
    /// `beginClassDeclaration(begin, abstract, sealed, base, interface, final,
    /// augment, mixin, name)`.
    #[allow(clippy::too_many_arguments)]
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
    }
    /// `endClassDeclaration(begin, end)`.
    fn end_class_declaration(&mut self, begin: Token, end: Token) {}
    /// `beginNamedMixinApplication(...)`/`endNamedMixinApplication(...)`.
    #[allow(clippy::too_many_arguments)]
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
    }
    fn end_named_mixin_application(
        &mut self,
        begin: Token,
        class: Token,
        equals: Token,
        implements: Option<Token>,
        end: Token,
    ) {
    }
    /// `beginClassOrMixinOrExtensionBody(kind, token)` /
    /// `endClassOrMixinOrExtensionBody(kind, memberCount, begin, end)`.
    fn begin_class_or_mixin_or_extension_body(&mut self, kind: DeclarationKind, token: Token) {}
    fn end_class_or_mixin_or_extension_body(
        &mut self,
        kind: DeclarationKind,
        count: usize,
        begin: Token,
        end: Token,
    ) {
    }
    /// `handleNoClassBody(semicolonToken)`: `class C;` (primary-constructor
    /// shape).
    fn handle_no_class_body(&mut self, semicolon: Token) {}

    // —— mixin declarations —————————————————————————————————————————————
    /// `beginMixinDeclaration(begin, augment, base, mixin, name)`.
    fn begin_mixin_declaration(
        &mut self,
        begin: Token,
        augment_kw: Option<Token>,
        base_kw: Option<Token>,
        mixin: Token,
        name: Token,
    ) {
    }
    /// `endMixinDeclaration(begin, end)`.
    fn end_mixin_declaration(&mut self, begin: Token, end: Token) {}
    /// `handleNoMixinBody(semicolonToken)`.
    fn handle_no_mixin_body(&mut self, semicolon: Token) {}

    // —— enum declarations ——————————————————————————————————————————————
    /// `beginEnumDeclarationPrelude(enumKeyword)`.
    fn begin_enum_declaration_prelude(&mut self, enum_keyword: Token) {}
    /// `beginEnumDeclaration(begin, augment, enumKeyword, name)`.
    fn begin_enum_declaration(
        &mut self,
        begin: Token,
        augment_kw: Option<Token>,
        enum_keyword: Token,
        name: Token,
    ) {
    }
    /// `endEnumDeclaration(begin, enumKeyword, leftBrace, memberCount, end)`.
    fn end_enum_declaration(
        &mut self,
        begin: Token,
        enum_keyword: Token,
        left_brace: Token,
        member_count: usize,
        end: Token,
    ) {
    }
    /// `handleEnumHeader(augmentToken, enumKeyword, leftBrace)`.
    fn handle_enum_header(
        &mut self,
        augment: Option<Token>,
        enum_keyword: Token,
        left_brace: Token,
    ) {
    }
    /// `beginEnumBody(token)`/`endEnumBody(begin, end)`.
    fn begin_enum_body(&mut self, token: Token) {}
    fn end_enum_body(&mut self, begin: Token, end: Token) {}
    /// `handleEnumElements(elementsEndToken, elementsCount)`.
    fn handle_enum_elements(&mut self, elements_end: Token, count: usize) {}
    /// `handleEnumElement(beginToken, augmentToken)`.
    fn handle_enum_element(&mut self, begin: Token, augment: Option<Token>) {}
    /// `handleNoEnumBody(semicolonToken)`: `enum E;` (primary-constructor
    /// shape).
    fn handle_no_enum_body(&mut self, semicolon: Token) {}

    // —— extension and extension type declarations ————————————————————————
    /// `beginExtensionDeclarationPrelude(extensionKeyword)`.
    fn begin_extension_declaration_prelude(&mut self, extension_keyword: Token) {}
    /// `beginExtensionDeclaration(augmentToken, extensionKeyword, name)`.
    fn begin_extension_declaration(
        &mut self,
        augment: Option<Token>,
        extension_keyword: Token,
        name: Option<Token>,
    ) {
    }
    /// `endExtensionDeclaration(begin, extensionKeyword, onKeyword, end)`.
    fn end_extension_declaration(
        &mut self,
        begin: Token,
        extension_keyword: Token,
        on: Option<Token>,
        end: Token,
    ) {
    }
    /// `handleNoExtensionBody(semicolonToken)`.
    fn handle_no_extension_body(&mut self, semicolon: Token) {}
    /// `beginExtensionTypeDeclaration(augmentToken, extensionKeyword, name)`.
    fn begin_extension_type_declaration(
        &mut self,
        augment_kw: Option<Token>,
        extension_keyword: Token,
        name: Token,
    ) {
    }
    /// `endExtensionTypeDeclaration(begin, augment, extensionKeyword,
    /// typeKeyword, end)`.
    fn end_extension_type_declaration(
        &mut self,
        begin: Token,
        augment_kw: Option<Token>,
        extension_keyword: Token,
        type_keyword: Token,
        end: Token,
    ) {
    }
    /// `handleNoExtensionTypeBody(semicolonToken)`.
    fn handle_no_extension_type_body(&mut self, semicolon: Token) {}
    /// `beginRedirectingFactoryBody(equals)` /
    /// `endRedirectingFactoryBody(equals, end)`.
    fn begin_redirecting_factory_body(&mut self, equals: Token) {}
    fn end_redirecting_factory_body(&mut self, equals: Token, end: Token) {}
    /// `handleNoTypeNameInConstructorReference(token)` (enum elements).
    fn handle_no_type_name_in_constructor_reference(&mut self, token: Token) {}
    /// `handleMixinWithClause(withKeyword)` (invalid mixin with-clause
    /// recovery).
    fn handle_mixin_with_clause(&mut self, with_keyword: Token) {}
    /// `beginPrimaryConstructorBody(token, augmentToken)` /
    /// `endPrimaryConstructorBody(begin, this, beginInitializers, end)`.
    fn begin_primary_constructor_body(&mut self, token: Token, augment: Option<Token>) {}
    fn end_primary_constructor_body(
        &mut self,
        begin: Token,
        this: Token,
        begin_initializers: Option<Token>,
        end: Token,
    ) {
    }

    // —— primary constructors ———————————————————————————————————————————
    /// `beginPrimaryConstructor(beginToken)`.
    fn begin_primary_constructor(&mut self, begin: Token) {}
    /// `endPrimaryConstructor(kind, begin, end, constKeyword,
    /// hasConstructorName)`.
    fn end_primary_constructor(
        &mut self,
        kind: DeclarationKind,
        begin: Token,
        end: Token,
        const_keyword: Option<Token>,
        has_constructor_name: bool,
    ) {
    }
    /// `handleNoPrimaryConstructor(kind, token, constKeyword)`.
    fn handle_no_primary_constructor(
        &mut self,
        kind: DeclarationKind,
        token: Token,
        const_keyword: Option<Token>,
    ) {
    }

    // —— statements (#19) ————————————————————————————————————————————————
    /// `beginBlock(token, blockKind)`.
    fn begin_block(&mut self, token: Token, kind: BlockKind) {}
    /// `endBlock(count, begin, end, blockKind)`.
    fn end_block(&mut self, count: usize, begin: Token, end: Token, kind: BlockKind) {}
    /// `beginBlockFunctionBody(token)`.
    fn begin_block_function_body(&mut self, token: Token) {}
    /// `endBlockFunctionBody(count, begin, end)`.
    fn end_block_function_body(&mut self, count: usize, begin: Token, end: Token) {}
    /// `handleInvalidFunctionBody(token)`.
    fn handle_invalid_function_body(&mut self, token: Token) {}
    /// `handleExpressionFunctionBody(arrow, end)`; `end` is `None` for
    /// function-expression bodies.
    fn handle_expression_function_body(&mut self, arrow: Token, end: Option<Token>) {}
    /// `handleExpressionStatement(begin, end)`.
    fn handle_expression_statement(&mut self, begin: Token, end: Token) {}
    /// `handleEmptyStatement(token)`.
    fn handle_empty_statement(&mut self, token: Token) {}
    /// `handleInvalidStatement(begin, message)`; the message is a
    /// pseudo-shared (resolution-domain) code and is not a TSV emission.
    fn handle_invalid_statement(&mut self, begin: Token) {}
    /// `beginIfStatement(token)`.
    fn begin_if_statement(&mut self, token: Token) {}
    fn begin_then_statement(&mut self, token: Token) {}
    fn end_then_statement(&mut self, begin: Token, end: Token) {}
    fn begin_else_statement(&mut self, token: Token) {}
    fn end_else_statement(&mut self, begin: Token, end: Token) {}
    /// `endIfStatement(ifToken, elseToken, end)`. `end` is the
    /// statement's own last consumed token — its `node.end` token, never
    /// the follower (the `parseStatement` result,
    /// `parser_impl.dart:10148`).
    fn end_if_statement(&mut self, token: Token, else_kw: Option<Token>, end: Token) {}
    /// `beginForStatement(token)`.
    fn begin_for_statement(&mut self, token: Token) {}
    fn handle_for_initializer_empty_statement(&mut self, token: Token) {}
    /// `handleForInitializerExpressionStatement(token, forIn)`.
    fn handle_for_initializer_expression_statement(&mut self, token: Token, for_in: bool) {}
    /// `handleForInitializerLocalVariableDeclaration(token, forIn)`.
    fn handle_for_initializer_local_variable_declaration(&mut self, token: Token, for_in: bool) {}
    /// `handleForInitializerPatternVariableAssignment(patternKeyword, equals)`.
    fn handle_for_initializer_pattern_variable_assignment(
        &mut self,
        pattern_keyword: Token,
        equals: Token,
    ) {
    }
    fn begin_for_statement_body(&mut self, token: Token) {}
    fn end_for_statement_body(&mut self, end: Token) {}
    /// `endForStatement(endToken)` (the for-in counterpart is
    /// [`ParserListener::end_for_in`]; exactly one of the two fires).
    /// `end` is the statement's own last consumed token — its `node.end`
    /// token, never the follower (parser_impl.dart:10339-10340).
    fn end_for_statement(&mut self, end: Token) {}
    /// `handleForLoopParts(forToken, leftParen, leftSeparator,
    /// rightSeparator, expressionCount)`.
    #[allow(clippy::too_many_arguments)]
    fn handle_for_loop_parts(
        &mut self,
        for_kw: Token,
        left_paren: Token,
        left_separator: Token,
        right_separator: Token,
        expression_count: usize,
    ) {
    }
    fn begin_for_in_body(&mut self, token: Token) {}
    fn end_for_in_body(&mut self, end: Token) {}
    /// `endForIn(endToken)`. `end` is the statement's own last consumed
    /// token — its `node.end` token, never the follower
    /// (parser_impl.dart:10415-10416).
    fn end_for_in(&mut self, end: Token) {}
    /// `handleForInLoopParts(awaitToken, forToken, leftParen,
    /// patternKeyword, inKeyword)`.
    #[allow(clippy::too_many_arguments)]
    fn handle_for_in_loop_parts(
        &mut self,
        await_kw: Option<Token>,
        for_kw: Token,
        left_paren: Token,
        pattern_keyword: Option<Token>,
        in_kw: Token,
    ) {
    }
    fn begin_for_in_expression(&mut self, token: Token) {}
    fn end_for_in_expression(&mut self, end: Token) {}
    fn begin_while_statement(&mut self, token: Token) {}
    fn begin_while_statement_body(&mut self, token: Token) {}
    fn end_while_statement_body(&mut self, end: Token) {}
    /// `endWhileStatement(whileToken, endToken)`. `end` is the
    /// statement's own last consumed token — its `node.end` token, never
    /// the follower (parser_impl.dart:10485-10486).
    fn end_while_statement(&mut self, token: Token, end: Token) {}
    fn begin_do_while_statement(&mut self, token: Token) {}
    fn begin_do_while_statement_body(&mut self, token: Token) {}
    fn end_do_while_statement_body(&mut self, end: Token) {}
    fn end_do_while_statement(&mut self, token: Token, while_kw: Token, end: Token) {}
    /// `beginReturnStatement(token)`.
    fn begin_return_statement(&mut self, token: Token) {}
    /// `endReturnStatement(hasExpression, begin, end)`.
    fn end_return_statement(&mut self, has_expression: bool, begin: Token, end: Token) {}
    fn begin_yield_statement(&mut self, token: Token) {}
    fn end_yield_statement(&mut self, begin: Token, star: Option<Token>, end: Token) {}
    /// `endInvalidYieldStatement(begin, star, end, errorCode)`; the error
    /// code is resolution-domain and never a TSV emission.
    fn end_invalid_yield_statement(&mut self, begin: Token, star: Option<Token>, end: Token) {}
    fn begin_labeled_statement(&mut self, token: Token, label_count: usize) {}
    fn end_labeled_statement(&mut self, label_count: usize) {}
    /// `handleLabel(colon)`.
    fn handle_label(&mut self, token: Token) {}
    fn begin_local_function_declaration(&mut self, token: Token) {}
    fn end_local_function_declaration(&mut self, end: Token) {}
    fn begin_function_name(&mut self, token: Token) {}
    fn end_function_name(&mut self, begin: Token, end: Token, is_function_expression: bool) {}
    /// `beginVariablesDeclaration(name, lateToken, varFinalOrConst)`.
    fn begin_variables_declaration(
        &mut self,
        token: Token,
        late_kw: Option<Token>,
        var_final_or_const: Option<Token>,
    ) {
    }
    /// `endVariablesDeclaration(count, end)`; `end` is `None` in for-parts
    /// mode.
    fn end_variables_declaration(&mut self, count: usize, end: Option<Token>) {}
    fn begin_initialized_identifier(&mut self, token: Token) {}
    fn end_initialized_identifier(&mut self, name: Token) {}
    fn handle_no_variable_initializer(&mut self, token: Token) {}
    fn begin_variable_initializer(&mut self, token: Token) {}
    fn end_variable_initializer(&mut self, token: Token) {}
    fn begin_try_statement(&mut self, token: Token) {}
    fn end_try_statement(
        &mut self,
        count: usize,
        begin: Token,
        finally_kw: Option<Token>,
        end: Token,
    ) {
    }
    fn begin_catch_clause(&mut self, token: Token) {}
    fn end_catch_clause(&mut self, token: Token) {}
    /// `handleCatchBlock(onKeyword, catchKeyword, comma)`.
    fn handle_catch_block(
        &mut self,
        on_kw: Option<Token>,
        catch_kw: Option<Token>,
        comma: Option<Token>,
    ) {
    }
    fn handle_finally_block(&mut self, finally_kw: Token) {}
    fn begin_switch_statement(&mut self, token: Token) {}
    /// `endSwitchStatement(switchToken, endToken)`. `end` is the
    /// statement's own last consumed token (the block's `}`) — its
    /// `node.end` token, never the follower.
    fn end_switch_statement(&mut self, token: Token, end: Token) {}
    fn begin_switch_block(&mut self, token: Token) {}
    fn end_switch_block(&mut self, count: usize, begin: Token, end: Token) {}
    fn begin_case_expression(&mut self, case_kw: Token) {}
    fn end_case_expression(&mut self, case_kw: Token, when: Option<Token>, colon: Token) {}
    fn begin_switch_case(&mut self, label_count: usize, expression_count: usize, token: Token) {}
    /// `endSwitchCase(labelCount, expressionCount, defaultKeyword,
    /// colonAfterDefault, statementCount, begin, end)`.
    #[allow(clippy::too_many_arguments)]
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
    }
    fn begin_switch_case_when_clause(&mut self, token: Token) {}
    fn end_switch_case_when_clause(&mut self, token: Token) {}
    fn handle_switch_case_no_when_clause(&mut self, token: Token) {}
    fn handle_break_statement(&mut self, has_target: bool, break_kw: Token, end: Token) {}
    fn handle_continue_statement(&mut self, has_target: bool, continue_kw: Token, end: Token) {}
    fn begin_rethrow_statement(&mut self, token: Token) {}
    fn end_rethrow_statement(&mut self, rethrow_kw: Token, end: Token) {}
    fn begin_assert(&mut self, token: Token, kind: AssertKind) {}
    fn end_assert(
        &mut self,
        begin: Token,
        kind: AssertKind,
        left_paren: Token,
        comma: Option<Token>,
        end: Token,
    ) {
    }
    /// `beginInitializer`/`endInitializer(end)` (one `: init` entry; `end`
    /// is the last consumed token of the entry).
    fn begin_initializer(&mut self, token: Token) {}
    fn end_initializer(&mut self, end: Token) {}
    /// `handleParenthesizedCondition(begin, case, when)` (if/while/switch
    /// conditions; `case`/`when` only under the patterns feature).
    fn handle_parenthesized_condition(
        &mut self,
        begin: Token,
        case_kw: Option<Token>,
        when: Option<Token>,
    ) {
    }
    fn begin_pattern_guard(&mut self, when: Token) {}
    fn end_pattern_guard(&mut self, token: Token) {}
    /// `handlePatternVariableDeclarationStatement(keyword, equals, semicolon)`.
    fn handle_pattern_variable_declaration_statement(
        &mut self,
        keyword: Token,
        equals: Token,
        semicolon: Token,
    ) {
    }

    // —— collection control-flow elements (#20) ————————————————————————————
    /// `beginIfControlFlow(ifToken)` / `handleThenControlFlow(token)` /
    /// `handleElseControlFlow(elseToken)` / `endIfControlFlow(token)` /
    /// `endIfElseControlFlow(token)` (C24).
    fn begin_if_control_flow(&mut self, token: Token) {}
    fn handle_then_control_flow(&mut self, token: Token) {}
    fn handle_else_control_flow(&mut self, token: Token) {}
    fn end_if_control_flow(&mut self, token: Token) {}
    fn end_if_else_control_flow(&mut self, token: Token) {}
    /// `beginForControlFlow(awaitToken, forToken)` /
    /// `endForControlFlow(token)` / `endForInControlFlow(token)` (C25).
    fn begin_for_control_flow(&mut self, await_kw: Option<Token>, for_kw: Token) {}
    fn end_for_control_flow(&mut self, token: Token) {}
    fn end_for_in_control_flow(&mut self, token: Token) {}
    /// `handleSpreadExpression(operator)` (`...`/`...?` elements, C26).
    fn handle_spread_expression(&mut self, operator: Token) {}
    /// `handleNullAwareElement(token)` (`?e` elements, C27).
    fn handle_null_aware_element(&mut self, token: Token) {}

    // —— switch expressions (#20) ————————————————————————————————————————
    /// `beginSwitchExpression(switchKeyword)` /
    /// `endSwitchExpression(switchKeyword, end)`.
    fn begin_switch_expression(&mut self, token: Token) {}
    fn end_switch_expression(&mut self, token: Token, end: Token) {}
    /// `beginSwitchExpressionBlock(token)` /
    /// `endSwitchExpressionBlock(caseCount, begin, end)`.
    fn begin_switch_expression_block(&mut self, token: Token) {}
    fn end_switch_expression_block(&mut self, count: usize, begin: Token, end: Token) {}
    /// `beginSwitchExpressionCase()` /
    /// `handleSwitchExpressionCasePattern(patternEnd)` /
    /// `endSwitchExpressionCase(begin, when, arrow, end)`.
    fn begin_switch_expression_case(&mut self) {}
    fn handle_switch_expression_case_pattern(&mut self, token: Token) {}
    fn end_switch_expression_case(
        &mut self,
        begin: Token,
        when: Option<Token>,
        arrow: Token,
        end: Token,
    ) {
    }

    // —— recovery ——————————————————————————————————————————————————————
    /// The only error channel: official `handleRecoverableError`
    /// (listener.dart:2375). `start`/`end` delimit the reported span.
    fn handle_recoverable_error(&mut self, message: Message, start: Token, end: Token) {}
    /// Official `handleExperimentNotEnabled` (listener.dart:2386): a
    /// feature-gated construct appeared while the feature is off.
    fn handle_experiment_not_enabled(&mut self, flag: ExperimentalFlag, begin: Token, end: Token) {}
}

/// The no-op listener (official `NullListener`, `forwarding_listener.dart:2510)`:
/// every event is dropped. Also the right choice for speculative parsing —
/// suppressed events cannot produce phantom output.
#[derive(Debug, Default, Clone, Copy)]
pub struct NullListener;

impl ParserListener for NullListener {}

/// Error-counting listener for speculative parses (the `hasErrors` half of
/// the official `canParseAsConditional`'s swapped-in `NullListener`): all
/// events are dropped, recovery reports are counted.
pub(crate) struct SpeculationListener {
    /// Recovery reports seen so far.
    pub(crate) errors: u32,
}

impl ParserListener for SpeculationListener {
    fn handle_recoverable_error(&mut self, _: Message, _: Token, _: Token) {
        self.errors += 1;
    }
}
