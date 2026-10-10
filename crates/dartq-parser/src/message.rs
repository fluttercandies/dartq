//! Diagnostic messages handed to the listener.
//!
//! The parser never collects diagnostics; it reports a [`Message`] (official
//! `Message` analog: code + formatted problem text) through
//! [`ParserListener::handle_recoverable_error`], and the listener owns the
//! translation into a `dartq_diagnostics::Diagnostic` (file attribution is
//! a listener concern).
//!
//! Codes come from the `dartq-diagnostics` registry (single source of
//! truth; every code used here is a registry constant). Message texts follow
//! the official templates (`_fe_analyzer_shared/messages.yaml`), matching the
//! (code, file) oracle differential of PRD §19.

use dartq_diagnostics::CodeInfo;

/// One recoverable-error report: registry code plus problem text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    /// Registry code (analyzer `analyzerCode` name; see `CodeInfo`).
    pub code: CodeInfo,
    /// Formatted `problemMessage` text.
    pub text: String,
}

impl Message {
    /// Builds a message from a code and pre-formatted text.
    pub(crate) fn new(code: CodeInfo, text: impl Into<String>) -> Self {
        Self {
            code,
            text: text.into(),
        }
    }
}

/// Official message templates (messages.yaml `problemMessage` texts) for the
/// codes this milestone emits.
pub(crate) mod msg {
    use super::{BlockKind, Message};
    use dartq_diagnostics::{
        ABSTRACT_CLASS_MEMBER, ABSTRACT_EXTERNAL_FIELD, BASE_ENUM, CLASS_IN_CLASS,
        CONFLICTING_MODIFIERS, CONST_AND_FINAL, CONST_METHOD, CONSTRUCTOR_WITH_RETURN_TYPE,
        COVARIANT_AND_STATIC, COVARIANT_MEMBER, DUPLICATED_MODIFIER, ENUM_IN_CLASS,
        EXPECTED_CLASS_MEMBER, EXPECTED_EXECUTABLE, EXPECTED_INSTEAD,
        EXTENSION_DECLARES_CONSTRUCTOR, EXTENSION_TYPE_EXTENDS, EXTENSION_TYPE_WITH,
        EXTERNAL_CONSTRUCTOR_WITH_INITIALIZER, EXTERNAL_FACTORY_REDIRECTION,
        EXTERNAL_FACTORY_WITH_BODY, EXTERNAL_METHOD_WITH_BODY, FACTORY_TOP_LEVEL_DECLARATION,
        FIELD_INITIALIZED_OUTSIDE_DECLARING_CLASS, FINAL_AND_COVARIANT,
        FINAL_AND_COVARIANT_LATE_WITH_INITIALIZER, FINAL_AND_VAR, FINAL_ENUM, FINAL_MIXIN,
        FINAL_MIXIN_CLASS, GETTER_CONSTRUCTOR, GETTER_WITH_PARAMETERS, IMPLEMENTS_BEFORE_EXTENDS,
        IMPLEMENTS_BEFORE_ON, IMPLEMENTS_BEFORE_WITH, INTERFACE_ENUM, INTERFACE_MIXIN,
        INTERFACE_MIXIN_CLASS, INVALID_CONSTRUCTOR_NAME, MEMBER_WITH_CLASS_NAME,
        MISSING_CONST_FINAL_VAR_OR_TYPE, MISSING_FUNCTION_BODY, MISSING_FUNCTION_PARAMETERS,
        MISSING_METHOD_PARAMETERS, MISSING_PRIMARY_CONSTRUCTOR,
        MISSING_PRIMARY_CONSTRUCTOR_PARAMETERS, MISSING_STAR_AFTER_SYNC,
        MISSING_TYPEDEF_PARAMETERS, MIXIN_DECLARES_CONSTRUCTOR, MODIFIER_OUT_OF_ORDER,
        MULTIPLE_CLAUSES, MULTIPLE_EXTENDS_CLAUSES, NATIVE_CLAUSE_SHOULD_BE_ANNOTATION,
        OUT_OF_ORDER_CLAUSES, REDIRECTING_CONSTRUCTOR_WITH_BODY,
        REDIRECTION_IN_NON_FACTORY_CONSTRUCTOR, SEALED_ENUM, SEALED_MIXIN, SEALED_MIXIN_CLASS,
        SETTER_CONSTRUCTOR, STATIC_CONSTRUCTOR, STATIC_OPERATOR, TOP_LEVEL_OPERATOR,
        TYPE_BEFORE_FACTORY, TYPEDEF_IN_CLASS, UNEXPECTED_TOKEN, UNEXPECTED_TOKENS,
        VAR_RETURN_TYPE, WITH_BEFORE_EXTENDS,
    };
    use dartq_diagnostics::{
        ABSTRACT_FINAL_BASE_CLASS, ABSTRACT_FINAL_INTERFACE_CLASS, ABSTRACT_SEALED_CLASS,
        EXTENSION_AUGMENTATION_HAS_ON_CLAUSE, EXTRANEOUS_MODIFIER, MISSING_KEYWORD_OPERATOR,
        MIXIN_WITH_CLAUSE, MULTIPLE_IMPLEMENTS_CLAUSES, MULTIPLE_ON_CLAUSES, MULTIPLE_WITH_CLAUSES,
        PATTERN_VARIABLE_DECLARATION_OUTSIDE_FUNCTION_OR_METHOD,
    };
    use dartq_diagnostics::{
        ABSTRACT_LATE_FIELD, ABSTRACT_STATIC_FIELD, ANNOTATION_SPACE_BEFORE_PARENTHESIS,
        ASYNC_KEYWORD_USED_AS_IDENTIFIER, CONST_CLASS, CONST_CONSTRUCTOR_WITH_BODY, CONST_FACTORY,
        DEFAULT_VALUE_IN_FUNCTION_TYPE, DUPLICATE_LABEL_IN_SWITCH_STATEMENT,
        EMPTY_RECORD_LITERAL_WITH_COMMA, EXPECTED_BODY, EXPECTED_REPRESENTATION_FIELD,
        EXTENSION_DECLARES_ABSTRACT_MEMBER, EXTERNAL_CLASS,
        EXTERNAL_CONSTRUCTOR_WITH_FIELD_INITIALIZERS, EXTERNAL_ENUM, EXTERNAL_LATE_FIELD,
        EXTERNAL_TYPEDEF, FIELD_INITIALIZER_OUTSIDE_CONSTRUCTOR, FUNCTION_TYPED_PARAMETER_VAR,
        ILLEGAL_ASSIGNMENT_TO_NON_ASSIGNABLE, INVALID_CONSTANT_PATTERN_EMPTY_RECORD_LITERAL,
        INVALID_INITIALIZER, INVALID_OPERATOR, INVALID_OPERATOR_QUESTIONMARK_PERIOD_FOR_SUPER,
        INVALID_SUPER_IN_INITIALIZER, INVALID_THIS_IN_INITIALIZER,
        INVALID_USE_OF_COVARIANT_IN_EXTENSION, MISSING_ENUM_BODY, MISSING_STATEMENT,
        MULTIPLE_REPRESENTATION_FIELDS, REPRESENTATION_FIELD_MODIFIER,
        TYPE_PARAMETER_ON_CONSTRUCTOR, TYPE_PARAMETER_ON_OPERATOR, VAR_AS_TYPE_NAME,
        WRONG_NUMBER_OF_PARAMETERS_FOR_SETTER,
    };
    use dartq_diagnostics::{
        ANNOTATION_ON_TYPE_ARGUMENT, ANNOTATION_WITH_TYPE_ARGUMENTS_UNINSTANTIATED,
        BINARY_OPERATOR_WRITTEN_OUT, BREAK_OUTSIDE_OF_LOOP, CATCH_SYNTAX,
        CATCH_SYNTAX_EXTRA_PARAMETERS, COLON_IN_PLACE_OF_IN, CONTINUE_OUTSIDE_OF_LOOP,
        CONTINUE_WITHOUT_LABEL_IN_CASE, DEFAULT_IN_SWITCH_EXPRESSION,
        EMPTY_RECORD_TYPE_NAMED_FIELDS_LIST, EMPTY_RECORD_TYPE_WITH_COMMA,
        EQUALITY_CANNOT_BE_EQUALITY_OPERAND, EXPECTED_ELSE_OR_COMMA,
        EXPECTED_IDENTIFIER_BUT_GOT_KEYWORD, EXPECTED_TOKEN, EXPECTED_TYPE_NAME,
        ILLEGAL_PATTERN_ASSIGNMENT_VARIABLE_NAME, ILLEGAL_PATTERN_IDENTIFIER_NAME,
        ILLEGAL_PATTERN_VARIABLE_NAME, INITIALIZED_VARIABLE_IN_FOR_EACH, INVALID_AWAIT_IN_FOR,
        INVALID_CONSTANT_CONST_PREFIX, INVALID_CONSTANT_PATTERN_BINARY,
        INVALID_INSIDE_UNARY_PATTERN, LATE_PATTERN_VARIABLE_DECLARATION, LITERAL_WITH_CLASS,
        LITERAL_WITH_CLASS_AND_NEW, LITERAL_WITH_NEW, MISSING_ASSIGNABLE_SELECTOR,
        MISSING_ASSIGNMENT_IN_INITIALIZER, MISSING_CATCH_OR_FINALLY, MISSING_EXPRESSION_IN_THROW,
        MISSING_IDENTIFIER, MISSING_INITIALIZER, MULTIPLE_VARIANCE_MODIFIERS,
        NAMED_FUNCTION_EXPRESSION, NULL_AWARE_CASCADE_OUT_OF_ORDER,
        PATTERN_ASSIGNMENT_DECLARES_VARIABLE, RECORD_LITERAL_ONE_POSITIONAL_NO_TRAILING_COMMA,
        RECORD_TYPE_ONE_POSITIONAL_NO_TRAILING_COMMA, STACK_OVERFLOW,
        SWITCH_HAS_CASE_AFTER_DEFAULT_CASE, SWITCH_HAS_MULTIPLE_DEFAULT_CASES, VAR_AND_TYPE,
        VARIABLE_PATTERN_KEYWORD_IN_DECLARATION_CONTEXT, VOID_WITH_TYPE_ARGUMENTS,
    };
    use dartq_diagnostics::{CodeInfo, DARTQ_INTERNAL_ERROR, DiagnosticType};
    use dartq_diagnostics::{
        DEFERRED_AFTER_PREFIX, DIRECTIVE_AFTER_DECLARATION, DUPLICATE_DEFERRED, DUPLICATE_PREFIX,
        EXPECTED_STRING_LITERAL, EXPORT_DIRECTIVE_AFTER_PART_DIRECTIVE,
        IMPORT_DIRECTIVE_AFTER_PART_DIRECTIVE, LIBRARY_DIRECTIVE_NOT_FIRST,
        MISSING_PREFIX_IN_DEFERRED_IMPORT, MULTIPLE_LIBRARY_DIRECTIVES,
        MULTIPLE_PART_OF_DIRECTIVES, NON_PART_OF_DIRECTIVE_IN_PART, PREFIX_AFTER_COMBINATOR,
    };

    /// `expectedToken` / `expectedButGot` / `expectedAfterButGot` all surface
    /// as the analyzer's `EXPECTED_TOKEN` (pseudoSharedCode in messages.yaml).
    pub(crate) fn expected_token(lexeme: &str) -> Message {
        Message::new(EXPECTED_TOKEN, format!("Expected to find '{lexeme}'."))
    }

    /// `expectedButGot` text; still reports the analyzer `expected_token` code.
    pub(crate) fn expected_but_got(expected: &str) -> Message {
        Message::new(
            EXPECTED_TOKEN,
            format!("Expected '{expected}' before this."),
        )
    }

    /// `expectedAfterButGot` text; analyzer code `expected_token`.
    pub(crate) fn expected_after_but_got(expected: &str) -> Message {
        Message::new(EXPECTED_TOKEN, format!("Expected '{expected}' after this."))
    }

    /// `invalidVoid` (analyzer code `expected_type_name`, per pseudoSharedCode).
    pub(crate) fn invalid_void() -> Message {
        Message::new(EXPECTED_TYPE_NAME, "Type 'void' can't be used here.")
    }

    /// `expectedType` (`NoType.ensureTypeNotVoid`, analyzer code
    /// `expected_type_name`).
    pub(crate) fn expected_type_name() -> Message {
        Message::new(EXPECTED_TYPE_NAME, "Expected a type name.")
    }

    pub(crate) fn missing_identifier() -> Message {
        Message::new(MISSING_IDENTIFIER, "Expected an identifier.")
    }

    /// `expectedIdentifierButGotKeyword` (identifier_context_impl.dart
    /// recovery templates): a reserved word used as a name — the keyword is
    /// reported and then consumed as the identifier.
    pub(crate) fn expected_identifier_but_got_keyword(lexeme: &str) -> Message {
        Message::new(
            EXPECTED_IDENTIFIER_BUT_GOT_KEYWORD,
            format!("'{lexeme}' can't be used as an identifier because it's a keyword."),
        )
    }

    /// `expectedIdentifier` (pseudo-shared `missingIdentifier`; the template
    /// text carries the offending lexeme).
    pub(crate) fn expected_identifier(lexeme: &str) -> Message {
        Message::new(
            MISSING_IDENTIFIER,
            format!("Expected an identifier, but got '{lexeme}'."),
        )
    }

    pub(crate) fn invalid_inside_unary_pattern() -> Message {
        Message::new(
            INVALID_INSIDE_UNARY_PATTERN,
            "This pattern cannot appear inside a unary pattern (cast pattern, null check pattern, or null assert pattern) without parentheses.",
        )
    }

    pub(crate) fn illegal_pattern_identifier_name(name: &str) -> Message {
        Message::new(
            ILLEGAL_PATTERN_IDENTIFIER_NAME,
            format!("A pattern can't refer to an identifier named '{name}'."),
        )
    }

    pub(crate) fn illegal_pattern_variable_name(name: &str) -> Message {
        Message::new(
            ILLEGAL_PATTERN_VARIABLE_NAME,
            format!("The variable declared by a variable pattern can't be named '{name}'."),
        )
    }

    pub(crate) fn illegal_pattern_assignment_variable_name(name: &str) -> Message {
        Message::new(
            ILLEGAL_PATTERN_ASSIGNMENT_VARIABLE_NAME,
            format!("A variable assigned by a pattern assignment can't be named '{name}'."),
        )
    }

    pub(crate) fn variable_pattern_keyword_in_declaration_context() -> Message {
        Message::new(
            VARIABLE_PATTERN_KEYWORD_IN_DECLARATION_CONTEXT,
            "Variable patterns in declaration context can't specify 'var' or 'final' keyword.",
        )
    }

    pub(crate) fn pattern_assignment_declares_variable(name: &str) -> Message {
        Message::new(
            PATTERN_ASSIGNMENT_DECLARES_VARIABLE,
            format!("Variable '{name}' can't be declared in a pattern assignment."),
        )
    }

    pub(crate) fn var_and_type() -> Message {
        Message::new(
            VAR_AND_TYPE,
            "Variables can't be declared using both 'var' and a type name.",
        )
    }

    pub(crate) fn invalid_constant_const_prefix() -> Message {
        Message::new(
            INVALID_CONSTANT_CONST_PREFIX,
            "The expression can't be prefixed by 'const' to form a constant pattern.",
        )
    }

    pub(crate) fn invalid_constant_pattern_binary(operator: &str) -> Message {
        Message::new(
            INVALID_CONSTANT_PATTERN_BINARY,
            format!("The binary operator {operator} is not supported as a constant pattern."),
        )
    }

    pub(crate) fn record_literal_one_positional_no_trailing_comma() -> Message {
        Message::new(
            RECORD_LITERAL_ONE_POSITIONAL_NO_TRAILING_COMMA,
            "A record literal with exactly one positional field requires a trailing comma.",
        )
    }

    pub(crate) fn record_type_one_positional_no_trailing_comma() -> Message {
        Message::new(
            RECORD_TYPE_ONE_POSITIONAL_NO_TRAILING_COMMA,
            "A record type with exactly one positional field requires a trailing comma.",
        )
    }

    pub(crate) fn empty_record_type_named_fields_list() -> Message {
        Message::new(
            EMPTY_RECORD_TYPE_NAMED_FIELDS_LIST,
            "The list of named fields in a record type can't be empty.",
        )
    }

    pub(crate) fn empty_record_type_with_comma() -> Message {
        Message::new(
            EMPTY_RECORD_TYPE_WITH_COMMA,
            "A record type without fields can't have a trailing comma.",
        )
    }

    pub(crate) fn annotation_on_type_argument() -> Message {
        Message::new(
            ANNOTATION_ON_TYPE_ARGUMENT,
            "Type arguments can't have annotations because they aren't declarations.",
        )
    }

    pub(crate) fn annotation_with_type_arguments_uninstantiated() -> Message {
        Message::new(
            ANNOTATION_WITH_TYPE_ARGUMENTS_UNINSTANTIATED,
            "An annotation with type arguments must be followed by an argument list.",
        )
    }

    /// Terminal bailout (rust-analyzer parser-bailout practice): the
    /// per-file diagnostics cap latched, so parsing stops with this one
    /// bounded diagnostic instead of streaming unbounded errors. dartq-own
    /// code: no official analyzer message covers a parser bailout.
    pub(crate) fn too_many_errors(cap: usize) -> Message {
        Message::new(
            DARTQ_INTERNAL_ERROR,
            format!("Too many syntax errors ({cap}); the rest of the file was not analyzed."),
        )
    }

    /// Terminal bailout for structural nesting past the parser's
    /// stack-overflow guard (official `recoverFromStackOverflow`,
    /// `parser_impl.dart:11386`, `diag.stackOverflow`).
    pub(crate) fn stack_overflow(cap: u32) -> Message {
        Message::new(
            STACK_OVERFLOW,
            format!("The file has too many nested expressions or statements (limit {cap})."),
        )
    }

    pub(crate) fn multiple_variance_modifiers() -> Message {
        Message::new(
            MULTIPLE_VARIANCE_MODIFIERS,
            "Each type parameter can have at most one variance modifier.",
        )
    }

    pub(crate) fn void_with_type_arguments() -> Message {
        Message::new(
            VOID_WITH_TYPE_ARGUMENTS,
            "Type 'void' can't have type arguments.",
        )
    }

    pub(crate) fn equality_cannot_be_equality_operand() -> Message {
        Message::new(
            EQUALITY_CANNOT_BE_EQUALITY_OPERAND,
            "A comparison expression can't be an operand of another comparison expression.",
        )
    }

    // —— declarations (#18) ————————————————————————————————————————————
    // Texts follow the shared-parser templates (`diagnostic.g.dart`,
    // `messages.yaml`); `{0}`-style substitutions are formatted here.

    pub(crate) fn expected_declaration(lexeme: &str) -> Message {
        Message::new(
            EXPECTED_EXECUTABLE,
            format!("Expected a declaration, but got '{lexeme}'."),
        )
    }

    pub(crate) fn expected_class_member(lexeme: &str) -> Message {
        Message::new(
            EXPECTED_CLASS_MEMBER,
            format!("Expected a class member, but got '{lexeme}'."),
        )
    }

    pub(crate) fn unexpected_token(lexeme: &str) -> Message {
        Message::new(UNEXPECTED_TOKEN, format!("Unexpected token '{lexeme}'."))
    }

    pub(crate) fn unexpected_tokens() -> Message {
        Message::new(UNEXPECTED_TOKENS, "Unexpected tokens.".to_string())
    }

    pub(crate) fn expected_executable() -> Message {
        Message::new(
            EXPECTED_EXECUTABLE,
            "Expected a method, getter, setter or operator declaration.".to_string(),
        )
    }

    // —— directive family (#300) —————————————————————————————————————————

    /// `expectedString` (`ensureLiteralString`, `parser_impl.dart:5075`;
    /// pseudoSharedCode `expectedStringLiteral`, withArguments text).
    pub(crate) fn expected_string(lexeme: &str) -> Message {
        Message::new(
            EXPECTED_STRING_LITERAL,
            format!("Expected a String, but got '{lexeme}'."),
        )
    }

    pub(crate) fn directive_after_declaration() -> Message {
        Message::new(
            DIRECTIVE_AFTER_DECLARATION,
            "Directives must appear before any declarations.",
        )
    }

    pub(crate) fn import_after_part() -> Message {
        Message::new(
            IMPORT_DIRECTIVE_AFTER_PART_DIRECTIVE,
            "Import directives must precede part directives.",
        )
    }

    pub(crate) fn export_after_part() -> Message {
        Message::new(
            EXPORT_DIRECTIVE_AFTER_PART_DIRECTIVE,
            "Export directives must precede part directives.",
        )
    }

    pub(crate) fn non_part_of_directive_in_part() -> Message {
        Message::new(
            NON_PART_OF_DIRECTIVE_IN_PART,
            "The part-of directive must be the only directive in a part.",
        )
    }

    pub(crate) fn multiple_library_directives() -> Message {
        Message::new(
            MULTIPLE_LIBRARY_DIRECTIVES,
            "Only one library directive may be declared in a file.",
        )
    }

    pub(crate) fn library_directive_not_first() -> Message {
        Message::new(
            LIBRARY_DIRECTIVE_NOT_FIRST,
            "The library directive must appear before all other directives.",
        )
    }

    pub(crate) fn part_of_twice() -> Message {
        Message::new(
            MULTIPLE_PART_OF_DIRECTIVES,
            "Only one part-of directive may be declared in a file.",
        )
    }

    pub(crate) fn duplicate_deferred() -> Message {
        Message::new(
            DUPLICATE_DEFERRED,
            "An import directive can only have one 'deferred' keyword.",
        )
    }

    pub(crate) fn deferred_after_prefix() -> Message {
        Message::new(
            DEFERRED_AFTER_PREFIX,
            "The deferred keyword should come immediately before the prefix ('as' clause).",
        )
    }

    pub(crate) fn duplicate_prefix() -> Message {
        Message::new(
            DUPLICATE_PREFIX,
            "An import directive can only have one prefix ('as' clause).",
        )
    }

    pub(crate) fn prefix_after_combinator() -> Message {
        Message::new(
            PREFIX_AFTER_COMBINATOR,
            "The prefix ('as' clause) should come before any show/hide combinators.",
        )
    }

    pub(crate) fn missing_prefix_in_deferred_import() -> Message {
        Message::new(
            MISSING_PREFIX_IN_DEFERRED_IMPORT,
            "Deferred imports should have a prefix.",
        )
    }

    pub(crate) fn expected_instead(expected: &str) -> Message {
        Message::new(
            EXPECTED_INSTEAD,
            format!("Expected '{expected}' instead of this."),
        )
    }

    pub(crate) fn extraneous_modifier(lexeme: &str) -> Message {
        Message::new(
            EXTRANEOUS_MODIFIER,
            format!("Can't have modifier '{lexeme}' here."),
        )
    }

    pub(crate) fn duplicated_modifier(lexeme: &str) -> Message {
        Message::new(
            DUPLICATED_MODIFIER,
            format!("The modifier '{lexeme}' was already specified."),
        )
    }

    pub(crate) fn modifier_out_of_order(modifier: &str, expected_later: &str) -> Message {
        Message::new(
            MODIFIER_OUT_OF_ORDER,
            format!("The modifier '{modifier}' should be before the modifier '{expected_later}'."),
        )
    }

    pub(crate) fn conflicting_modifiers(modifier: &str, earlier: &str) -> Message {
        Message::new(
            CONFLICTING_MODIFIERS,
            format!("Members can't be declared to be both '{modifier}' and '{earlier}'."),
        )
    }

    pub(crate) fn const_and_final() -> Message {
        Message::new(
            CONST_AND_FINAL,
            "Members can't be declared to be both 'const' and 'final'.",
        )
    }

    pub(crate) fn final_and_var() -> Message {
        Message::new(
            FINAL_AND_VAR,
            "Members can't be declared to be both 'final' and 'var'.",
        )
    }

    pub(crate) fn covariant_and_static() -> Message {
        Message::new(
            COVARIANT_AND_STATIC,
            "Members can't be declared to be both 'covariant' and 'static'.",
        )
    }

    pub(crate) fn missing_const_final_var_or_type() -> Message {
        Message::new(
            MISSING_CONST_FINAL_VAR_OR_TYPE,
            "Variables must be declared using the keywords 'const', 'final', 'var' or a type name.",
        )
    }

    pub(crate) fn var_return_type() -> Message {
        Message::new(VAR_RETURN_TYPE, "The return type can't be 'var'.")
    }

    pub(crate) fn type_after_var() -> Message {
        Message::new(
            VAR_AND_TYPE,
            "Variables can't be declared using both 'var' and a type name.",
        )
    }

    pub(crate) fn abstract_external_field() -> Message {
        Message::new(
            ABSTRACT_EXTERNAL_FIELD,
            "Fields can't be declared both 'abstract' and 'external'.",
        )
    }

    pub(crate) fn final_and_covariant() -> Message {
        Message::new(
            FINAL_AND_COVARIANT,
            "Members can't be declared to be both 'final' and 'covariant'.",
        )
    }

    pub(crate) fn final_and_covariant_late_with_initializer() -> Message {
        Message::new(
            FINAL_AND_COVARIANT_LATE_WITH_INITIALIZER,
            "Members marked 'late' with an initializer can't be declared to be both 'final' and 'covariant'.",
        )
    }

    pub(crate) fn abstract_class_member() -> Message {
        Message::new(
            ABSTRACT_CLASS_MEMBER,
            "Members of classes can't be declared to be 'abstract'.",
        )
    }

    pub(crate) fn type_before_factory() -> Message {
        Message::new(
            TYPE_BEFORE_FACTORY,
            "Factory constructors cannot have a return type.",
        )
    }

    pub(crate) fn factory_top_level_declaration() -> Message {
        Message::new(
            FACTORY_TOP_LEVEL_DECLARATION,
            "Top-level declarations can't be declared to be 'factory'.",
        )
    }

    pub(crate) fn top_level_operator() -> Message {
        Message::new(
            TOP_LEVEL_OPERATOR,
            "Operators must be declared within a class.",
        )
    }

    pub(crate) fn static_operator() -> Message {
        Message::new(STATIC_OPERATOR, "Operators can't be static.")
    }

    pub(crate) fn covariant_member() -> Message {
        Message::new(
            COVARIANT_MEMBER,
            "Getters, setters and methods can't be declared to be 'covariant'.",
        )
    }

    pub(crate) fn const_method() -> Message {
        Message::new(
            CONST_METHOD,
            "Getters, setters and methods can't be declared to be 'const'.",
        )
    }

    pub(crate) fn getter_with_formals() -> Message {
        Message::new(
            GETTER_WITH_PARAMETERS,
            "Getters must be declared without a parameter list.",
        )
    }

    pub(crate) fn missing_parameters(kind: crate::listener::MemberKind) -> Message {
        use crate::listener::MemberKind;
        let (code, text) = match kind {
            MemberKind::FunctionTypeAlias => (
                MISSING_TYPEDEF_PARAMETERS,
                "Typedefs must have an explicit list of parameters.",
            ),
            MemberKind::StaticMethod | MemberKind::NonStaticMethod => (
                MISSING_METHOD_PARAMETERS,
                "Methods must have an explicit list of parameters.",
            ),
            _ => (
                MISSING_FUNCTION_PARAMETERS,
                "Functions must have an explicit list of parameters.",
            ),
        };
        Message::new(code, text)
    }

    /// `expectedBody` (analyzer registry code `missing_function_body`).
    pub(crate) fn expected_body() -> Message {
        Message::new(MISSING_FUNCTION_BODY, "A function body must be provided.")
    }

    /// `expected{Class,Mixin,Enum,Extension,ExtensionType,Try,Catch,Finally,
    /// SwitchStatement,SwitchExpression}Body`: the shared parser's per-kind
    /// block-body codes all surface under the single analyzer registry name
    /// `expected_body` (`diagnostic.g.dart` names every one `expected_body`),
    /// each keeping its official fixed text. The template flavors
    /// (`expectedEnumBody`, `expectedFunctionBody`) interpolate the offending
    /// lexeme like the official templates.
    pub(crate) fn expected_block_body(kind: BlockKind, _lexeme: &str) -> Message {
        if kind == BlockKind::Body {
            return expected_but_got("{");
        }
        let text = match kind {
            BlockKind::Class => "A class declaration must have a body, even if it is empty.",
            BlockKind::Mixin => "A mixin declaration must have a body, even if it is empty.",
            // `expectedEnumBody` is a pseudoSharedCode mapping to the
            // analyzer's `missing_enum_body`, not `expected_body`.
            BlockKind::Enum => {
                return Message::new(
                    MISSING_ENUM_BODY,
                    "An enum definition must have a body with at least one constant name.",
                );
            }
            BlockKind::Extension => {
                "An extension declaration must have a body, even if it is empty."
            }
            BlockKind::ExtensionType => {
                "An extension type declaration must have a body, even if it is empty."
            }
            BlockKind::TryStatement => "A try statement must have a body, even if it is empty.",
            BlockKind::CatchClause => "A catch clause must have a body, even if it is empty.",
            BlockKind::FinallyClause => "A finally clause must have a body, even if it is empty.",
            BlockKind::SwitchStatement => {
                "A switch statement must have a body, even if it is empty."
            }
            BlockKind::SwitchExpression => {
                "A switch expression must have a body, even if it is empty."
            }
            // Never reported: statement blocks are only entered with `{`
            // present.
            BlockKind::Statement => return expected_but_got("{"),
            BlockKind::Body => unreachable!("handled above"),
        };
        Message::new(EXPECTED_BODY, text)
    }

    pub(crate) fn member_with_same_name_as_class() -> Message {
        Message::new(
            MEMBER_WITH_CLASS_NAME,
            "A class member can't have the same name as the enclosing class.",
        )
    }

    pub(crate) fn invalid_constructor_name() -> Message {
        Message::new(
            INVALID_CONSTRUCTOR_NAME,
            "The name of a constructor must match the name of the enclosing class.",
        )
    }

    pub(crate) fn static_constructor() -> Message {
        Message::new(STATIC_CONSTRUCTOR, "Constructors can't be static.")
    }

    pub(crate) fn getter_constructor() -> Message {
        Message::new(GETTER_CONSTRUCTOR, "Constructors can't be a getter.")
    }

    pub(crate) fn setter_constructor() -> Message {
        Message::new(SETTER_CONSTRUCTOR, "Constructors can't be a setter.")
    }

    pub(crate) fn constructor_with_return_type() -> Message {
        Message::new(
            CONSTRUCTOR_WITH_RETURN_TYPE,
            "Constructors can't have a return type.",
        )
    }

    pub(crate) fn external_constructor_with_initializer() -> Message {
        Message::new(
            EXTERNAL_CONSTRUCTOR_WITH_INITIALIZER,
            "An external constructor can't have any initializers.",
        )
    }

    /// `fieldInitializedOutsideDeclaringClass` (`C() : super.x = 42;`).
    pub(crate) fn field_initialized_outside_declaring_class() -> Message {
        Message::new(
            FIELD_INITIALIZED_OUTSIDE_DECLARING_CLASS,
            "A field can only be initialized in its declaring class",
        )
    }

    /// `expectedAnInitializer` (analyzer code `missingInitializer`).
    pub(crate) fn expected_an_initializer() -> Message {
        Message::new(MISSING_INITIALIZER, "Expected an initializer.")
    }

    /// `missingAssignmentInInitializer` (`C() : x(3) {}`).
    pub(crate) fn missing_assignment_in_initializer() -> Message {
        Message::new(
            MISSING_ASSIGNMENT_IN_INITIALIZER,
            "Expected an assignment after the field name.",
        )
    }

    /// `handleConstFactory` (analyzer registry code `const_factory`): a
    /// `const factory` whose body is neither a redirection nor `external …;`.
    pub(crate) fn const_factory() -> Message {
        Message::new(
            CONST_FACTORY,
            "Only redirecting factory constructors can be declared to be 'const'.",
        )
    }

    /// `constConstructorWithBody`: a `const` constructor with a `{}` body.
    pub(crate) fn const_constructor_with_body() -> Message {
        Message::new(
            CONST_CONSTRUCTOR_WITH_BODY,
            "Const constructors can't have a body.",
        )
    }

    /// `reportTopLevelModifierError` for `const` before a declaration
    /// keyword (only `class` gets a dedicated code; everything else is
    /// `extraneous_modifier`).
    pub(crate) fn const_class() -> Message {
        Message::new(CONST_CLASS, "Classes can't be declared to be 'const'.")
    }

    pub(crate) fn external_class() -> Message {
        Message::new(
            EXTERNAL_CLASS,
            "Classes can't be declared to be 'external'.",
        )
    }

    pub(crate) fn external_enum() -> Message {
        Message::new(EXTERNAL_ENUM, "Enums can't be declared to be 'external'.")
    }

    pub(crate) fn external_typedef() -> Message {
        Message::new(
            EXTERNAL_TYPEDEF,
            "Typedefs can't be declared to be 'external'.",
        )
    }

    /// `_endClassFields`: `external` + `late` on one field.
    pub(crate) fn external_late_field() -> Message {
        Message::new(EXTERNAL_LATE_FIELD, "External fields cannot be late.")
    }

    /// `_endClassFields`: `abstract` + `late` on one field.
    pub(crate) fn abstract_late_field() -> Message {
        Message::new(ABSTRACT_LATE_FIELD, "Abstract fields cannot be late.")
    }

    /// `_endClassFields`: `abstract` + `static` on one field.
    pub(crate) fn abstract_static_field() -> Message {
        Message::new(
            ABSTRACT_STATIC_FIELD,
            "Static fields can't be declared 'abstract'.",
        )
    }

    /// `external` constructor with `: field` initializers (analyzer
    /// `external_constructor_with_field_initializers`).
    pub(crate) fn external_constructor_with_field_initializers() -> Message {
        Message::new(
            EXTERNAL_CONSTRUCTOR_WITH_FIELD_INITIALIZERS,
            "An external constructor can't initialize fields.",
        )
    }

    /// `invalidUseOfCovariantInExtension` (`covariant` members in an
    /// `extension` body).
    pub(crate) fn invalid_use_of_covariant_in_extension(modifier: &str) -> Message {
        Message::new(
            INVALID_USE_OF_COVARIANT_IN_EXTENSION,
            format!("Can't have modifier '{modifier}' in an extension."),
        )
    }

    /// `extensionDeclaresAbstractMember`: an abstract member in an extension.
    pub(crate) fn extension_declares_abstract_member() -> Message {
        Message::new(
            EXTENSION_DECLARES_ABSTRACT_MEMBER,
            "Extensions can't declare abstract members.",
        )
    }

    /// `AstBuilder` `_endClassMethod`: a setter's parameter-count validation.
    pub(crate) fn wrong_number_of_parameters_for_setter() -> Message {
        Message::new(
            WRONG_NUMBER_OF_PARAMETERS_FOR_SETTER,
            "Setters must declare exactly one required positional parameter.",
        )
    }

    /// Primary-constructor / extension-type representation-field recovery.
    pub(crate) fn expected_representation_field() -> Message {
        Message::new(
            EXPECTED_REPRESENTATION_FIELD,
            "Expected a representation field.",
        )
    }

    /// A modifier (`final`/`var`/`late`/…) on a representation field.
    pub(crate) fn representation_field_modifier(modifier: &str) -> Message {
        Message::new(
            REPRESENTATION_FIELD_MODIFIER,
            format!("Representation fields can't have the modifier '{modifier}'."),
        )
    }

    /// A second representation field in one extension type.
    pub(crate) fn multiple_representation_fields() -> Message {
        Message::new(
            MULTIPLE_REPRESENTATION_FIELDS,
            "Each extension type should have exactly one representation field.",
        )
    }

    /// `this.x` field formal outside a constructor.
    pub(crate) fn field_initializer_outside_constructor() -> Message {
        Message::new(
            FIELD_INITIALIZER_OUTSIDE_CONSTRUCTOR,
            "Field formal parameters can only be used in a constructor.",
        )
    }

    /// Statement-position junk the official `parseStatement` cannot shape.
    pub(crate) fn missing_statement() -> Message {
        Message::new(MISSING_STATEMENT, "Expected a statement.")
    }

    /// `parseTypedef` without a parameter list (issue 26073 recovery).
    pub(crate) fn missing_typedef_parameters() -> Message {
        Message::new(
            MISSING_TYPEDEF_PARAMETERS,
            "Typedefs must have an explicit list of parameters.",
        )
    }

    /// `parseOperatorName` invalid operator name recovery.
    pub(crate) fn invalid_operator(lexeme: &str) -> Message {
        Message::new(
            INVALID_OPERATOR,
            format!("The string '{lexeme}' isn't a user-definable operator."),
        )
    }

    /// `AstBuilder` `handleInvalidInitializer` (`C() : 3;`).
    pub(crate) fn invalid_initializer() -> Message {
        Message::new(INVALID_INITIALIZER, "Not a valid initializer.")
    }

    /// `@Annotation ()` — whitespace between the annotation name and `()`.
    pub(crate) fn annotation_space_before_parenthesis() -> Message {
        Message::new(
            ANNOTATION_SPACE_BEFORE_PARENTHESIS,
            "Annotations can't have spaces or comments before the parenthesis.",
        )
    }

    /// `()` record literal with a trailing comma.
    pub(crate) fn empty_record_literal_with_comma() -> Message {
        Message::new(
            EMPTY_RECORD_LITERAL_WITH_COMMA,
            "A record literal without fields can't have a trailing comma.",
        )
    }

    /// `case ()` — the empty record literal is not a constant pattern.
    pub(crate) fn invalid_constant_pattern_empty_record_literal() -> Message {
        Message::new(
            INVALID_CONSTANT_PATTERN_EMPTY_RECORD_LITERAL,
            "The empty record literal is not supported as a constant pattern.",
        )
    }

    /// `AstBuilder` assignment-LHS validation (`(a) = 3` non-assignable).
    pub(crate) fn illegal_assignment_to_non_assignable() -> Message {
        Message::new(
            ILLEGAL_ASSIGNMENT_TO_NON_ASSIGNABLE,
            "Illegal assignment to non-assignable expression.",
        )
    }

    /// `handleAssignmentExpression`'s non-assignable LHS report (analyzer
    /// `ast_builder.dart:3825-3841`): "Missing selector such as
    /// '.identifier' or '[0]'."
    pub(crate) fn missing_assignable_selector() -> Message {
        Message::new(
            MISSING_ASSIGNABLE_SELECTOR,
            "Missing selector such as '.identifier' or '[0]'.",
        )
    }

    /// `await`/`yield` consumed as an identifier inside an async function.
    pub(crate) fn async_keyword_used_as_identifier() -> Message {
        Message::new(
            ASYNC_KEYWORD_USED_AS_IDENTIFIER,
            "The keywords 'await' and 'yield' can't be used as identifiers in an asynchronous or generator function.",
        )
    }

    /// A duplicated label on one switch member (`AstBuilder`).
    pub(crate) fn duplicate_label_in_switch_statement(label: &str) -> Message {
        Message::new(
            DUPLICATE_LABEL_IN_SWITCH_STATEMENT,
            format!("The label '{label}' was already used in this switch statement."),
        )
    }

    /// `super` in an initializer but not a constructor call (`AstBuilder`).
    pub(crate) fn invalid_super_in_initializer() -> Message {
        Message::new(
            INVALID_SUPER_IN_INITIALIZER,
            "Can only use 'super' in an initializer for calling the superclass constructor (e.g. 'super()' or 'super.namedConstructor()')",
        )
    }

    /// `this` in an initializer but not field init/redirection (`AstBuilder`).
    pub(crate) fn invalid_this_in_initializer() -> Message {
        Message::new(
            INVALID_THIS_IN_INITIALIZER,
            "Can only use 'this' in an initializer for field initialization (e.g. 'this.x = something') and constructor redirection (e.g. 'this()' or 'this.namedConstructor())",
        )
    }

    /// `super?.foo` — null-aware access on `super` (parser recovery).
    pub(crate) fn invalid_operator_questionmark_period_for_super() -> Message {
        Message::new(
            INVALID_OPERATOR_QUESTIONMARK_PERIOD_FOR_SUPER,
            "The operator '?.' cannot be used with 'super' because 'super' cannot be null.",
        )
    }

    /// `var` in a type position (`varAsTypeName`, identifier context).
    pub(crate) fn var_as_type_name() -> Message {
        Message::new(
            VAR_AS_TYPE_NAME,
            "The keyword 'var' can't be used as a type name.",
        )
    }

    /// `<T>` on a constructor declaration.
    pub(crate) fn type_parameter_on_constructor() -> Message {
        Message::new(
            TYPE_PARAMETER_ON_CONSTRUCTOR,
            "Constructors can't have type parameters.",
        )
    }

    /// `<T>` on an operator declaration.
    pub(crate) fn type_parameter_on_operator() -> Message {
        Message::new(
            TYPE_PARAMETER_ON_OPERATOR,
            "Types parameters aren't allowed when defining an operator.",
        )
    }

    /// `var` where a function-typed parameter's return type sits (official
    /// `parseFormalParameterModifiers` `GeneralizedFunctionType` arm).
    pub(crate) fn function_typed_parameter_var() -> Message {
        Message::new(
            FUNCTION_TYPED_PARAMETER_VAR,
            "Function-typed parameters can't specify 'const', 'final' or 'var' in place of a return type.",
        )
    }

    /// `void f([int x = 1])` inside a function *type*.
    pub(crate) fn default_value_in_function_type() -> Message {
        Message::new(
            DEFAULT_VALUE_IN_FUNCTION_TYPE,
            "Parameters in a function type can't have default values.",
        )
    }

    /// `redirectingConstructorWithBody` (`C() : this.x() {}`).
    pub(crate) fn redirecting_constructor_with_body() -> Message {
        Message::new(
            REDIRECTING_CONSTRUCTOR_WITH_BODY,
            "Redirecting constructors can't have a body.",
        )
    }

    pub(crate) fn external_method_with_body() -> Message {
        Message::new(
            EXTERNAL_METHOD_WITH_BODY,
            "An external or native method can't have a body.",
        )
    }

    pub(crate) fn external_factory_redirection() -> Message {
        Message::new(
            EXTERNAL_FACTORY_REDIRECTION,
            "A redirecting factory can't be external.",
        )
    }

    pub(crate) fn external_factory_with_body() -> Message {
        Message::new(
            EXTERNAL_FACTORY_WITH_BODY,
            "External factories can't have a body.",
        )
    }

    pub(crate) fn factory_not_sync() -> Message {
        Message::new(
            local_code("non_sync_factory", "NON_SYNC_FACTORY"),
            "Factory bodies can't use 'async', 'async*', or 'sync*'.",
        )
    }

    pub(crate) fn setter_not_sync() -> Message {
        Message::new(
            local_code("invalid_modifier_on_setter", "INVALID_MODIFIER_ON_SETTER"),
            "Setters can't use 'async', 'async*', or 'sync*'.",
        )
    }

    pub(crate) fn abstract_not_sync() -> Message {
        Message::new(
            local_code("non_sync_abstract_method", "NON_SYNC_ABSTRACT_METHOD"),
            "Abstract methods can't use 'async', 'async*', or 'sync*'.",
        )
    }

    pub(crate) fn invalid_sync_modifier() -> Message {
        Message::new(
            MISSING_STAR_AFTER_SYNC,
            "The modifier 'sync' must be followed by a star ('*').",
        )
    }

    pub(crate) fn redirection_in_non_factory() -> Message {
        Message::new(
            REDIRECTION_IN_NON_FACTORY_CONSTRUCTOR,
            "Only factory constructor can specify '=' redirection.",
        )
    }

    pub(crate) fn mixin_declares_constructor() -> Message {
        Message::new(
            MIXIN_DECLARES_CONSTRUCTOR,
            "Mixins can't declare constructors.",
        )
    }

    pub(crate) fn extension_declares_constructor() -> Message {
        Message::new(
            EXTENSION_DECLARES_CONSTRUCTOR,
            "Extensions can't declare constructors.",
        )
    }

    pub(crate) fn class_in_class() -> Message {
        Message::new(
            CLASS_IN_CLASS,
            "Classes can't be declared inside other classes.",
        )
    }

    pub(crate) fn enum_in_class() -> Message {
        Message::new(ENUM_IN_CLASS, "Enums can't be declared inside classes.")
    }

    pub(crate) fn typedef_in_class() -> Message {
        Message::new(
            TYPEDEF_IN_CLASS,
            "Typedefs can't be declared inside classes.",
        )
    }

    pub(crate) fn base_enum() -> Message {
        Message::new(BASE_ENUM, "Enums can't be declared to be 'base'.")
    }

    pub(crate) fn final_enum() -> Message {
        Message::new(FINAL_ENUM, "Enums can't be declared to be 'final'.")
    }

    pub(crate) fn interface_enum() -> Message {
        Message::new(INTERFACE_ENUM, "Enums can't be declared to be 'interface'.")
    }

    pub(crate) fn sealed_enum() -> Message {
        Message::new(SEALED_ENUM, "Enums can't be declared to be 'sealed'.")
    }

    pub(crate) fn final_mixin() -> Message {
        Message::new(FINAL_MIXIN, "A mixin can't be declared 'final'.")
    }

    pub(crate) fn interface_mixin() -> Message {
        Message::new(INTERFACE_MIXIN, "A mixin can't be declared 'interface'.")
    }

    pub(crate) fn sealed_mixin() -> Message {
        Message::new(SEALED_MIXIN, "A mixin can't be declared 'sealed'.")
    }

    pub(crate) fn final_mixin_class() -> Message {
        Message::new(
            FINAL_MIXIN_CLASS,
            "A mixin class can't be declared 'final'.",
        )
    }

    pub(crate) fn interface_mixin_class() -> Message {
        Message::new(
            INTERFACE_MIXIN_CLASS,
            "A mixin class can't be declared 'interface'.",
        )
    }

    pub(crate) fn sealed_mixin_class() -> Message {
        Message::new(
            SEALED_MIXIN_CLASS,
            "A mixin class can't be declared 'sealed'.",
        )
    }

    pub(crate) fn multiple_clauses(definition: &str, clause: &str) -> Message {
        Message::new(
            MULTIPLE_CLAUSES,
            format!("Each '{definition}' definition can have at most one '{clause}' clause."),
        )
    }

    pub(crate) fn out_of_order_clauses(expected_earlier: &str, expected_later: &str) -> Message {
        Message::new(
            OUT_OF_ORDER_CLAUSES,
            format!(
                "The '{expected_earlier}' clause must come before the '{expected_later}' clause."
            ),
        )
    }

    pub(crate) fn multiple_on_clauses() -> Message {
        Message::new(
            MULTIPLE_ON_CLAUSES,
            "Each mixin definition can have at most one on clause.",
        )
    }

    pub(crate) fn multiple_extends() -> Message {
        Message::new(
            MULTIPLE_EXTENDS_CLAUSES,
            "Each class definition can have at most one extends clause.",
        )
    }

    pub(crate) fn with_before_extends() -> Message {
        Message::new(
            WITH_BEFORE_EXTENDS,
            "The extends clause must be before the with clause.",
        )
    }

    pub(crate) fn implements_before_extends() -> Message {
        Message::new(
            IMPLEMENTS_BEFORE_EXTENDS,
            "The extends clause must be before the implements clause.",
        )
    }

    pub(crate) fn implements_before_with() -> Message {
        Message::new(
            IMPLEMENTS_BEFORE_WITH,
            "The with clause must be before the implements clause.",
        )
    }

    pub(crate) fn implements_before_on() -> Message {
        Message::new(
            IMPLEMENTS_BEFORE_ON,
            "The on clause must be before the implements clause.",
        )
    }

    pub(crate) fn extension_type_extends() -> Message {
        Message::new(
            EXTENSION_TYPE_EXTENDS,
            "An extension type declaration can't have an 'extends' clause.",
        )
    }

    pub(crate) fn extension_type_with() -> Message {
        Message::new(
            EXTENSION_TYPE_WITH,
            "An extension type declaration can't have a 'with' clause.",
        )
    }

    pub(crate) fn mixin_with_clause() -> Message {
        Message::new(MIXIN_WITH_CLAUSE, "A mixin can't have a with clause.")
    }

    pub(crate) fn missing_primary_constructor() -> Message {
        Message::new(
            MISSING_PRIMARY_CONSTRUCTOR,
            "An extension type declaration must have a primary constructor declaration.",
        )
    }

    pub(crate) fn missing_primary_constructor_parameters() -> Message {
        Message::new(
            MISSING_PRIMARY_CONSTRUCTOR_PARAMETERS,
            "A primary constructor declaration must have formal parameters.",
        )
    }

    pub(crate) fn const_without_primary_constructor() -> Message {
        Message::new(
            local_code(
                "const_without_primary_constructor",
                "CONST_WITHOUT_PRIMARY_CONSTRUCTOR",
            ),
            "'const' can only be used together with a primary constructor declaration.",
        )
    }

    pub(crate) fn mixin_primary_constructor() -> Message {
        Message::new(
            local_code("mixin_primary_constructor", "MIXIN_PRIMARY_CONSTRUCTOR"),
            "Mixins can't have primary constructors.",
        )
    }

    pub(crate) fn extension_primary_constructor() -> Message {
        Message::new(
            local_code(
                "extension_primary_constructor",
                "EXTENSION_PRIMARY_CONSTRUCTOR",
            ),
            "Extensions can't have primary constructors.",
        )
    }

    pub(crate) fn native_clause_should_be_annotation() -> Message {
        Message::new(
            NATIVE_CLAUSE_SHOULD_BE_ANNOTATION,
            "Native clause in this form is deprecated.",
        )
    }

    // —— registry-gap codes —————————————————————————————————————————————
    // CFE parser diagnostics whose analyzer registry entries are absent from
    // the `syntax-codes.tsv` capture (the analyzer surfaces them through its
    // own resolver-side codes or not at all). Declared here with the shared
    // parser's code names so parser behavior stays complete; the
    // differential (code, file) gate treats them as parser-only until the
    // registry promotes them.

    fn local_code(name: &'static str, machine: &'static str) -> CodeInfo {
        // Registry-gap codes are absent from the official table, so they have
        // no published docs page (`has_published_docs: false`).
        CodeInfo::new(name, machine, DiagnosticType::CompileTimeError, false)
    }

    pub(crate) fn built_in_identifier_in_declaration(lexeme: &str) -> Message {
        Message::new(
            local_code(
                "built_in_identifier_in_declaration",
                "BUILT_IN_IDENTIFIER_IN_DECLARATION",
            ),
            format!("Can't use '{lexeme}' as a name here."),
        )
    }

    pub(crate) fn extension_declares_instance_field() -> Message {
        Message::new(
            local_code(
                "extension_declares_instance_field",
                "EXTENSION_DECLARES_INSTANCE_FIELD",
            ),
            "Extensions can't declare instance fields",
        )
    }

    pub(crate) fn extension_type_declares_instance_field() -> Message {
        Message::new(
            local_code(
                "extension_type_declares_instance_field",
                "EXTENSION_TYPE_DECLARES_INSTANCE_FIELD",
            ),
            "Extension types can't declare instance fields",
        )
    }

    pub(crate) fn abstract_extension_field() -> Message {
        Message::new(
            local_code("abstract_extension_field", "ABSTRACT_EXTENSION_FIELD"),
            "Extension fields can't be declared 'abstract'.",
        )
    }

    pub(crate) fn typedef_augmentation() -> Message {
        Message::new(
            local_code("typedef_augmentation", "TYPEDEF_AUGMENTATION"),
            "Type aliases can't be augmented.",
        )
    }

    pub(crate) fn abstract_sealed_class() -> Message {
        Message::new(
            ABSTRACT_SEALED_CLASS,
            "Classes can't be declared both 'abstract' and 'sealed'.",
        )
    }

    pub(crate) fn abstract_final_base_class() -> Message {
        Message::new(
            ABSTRACT_FINAL_BASE_CLASS,
            "Classes can't be declared both 'abstract' and 'final' and 'base'.",
        )
    }

    pub(crate) fn abstract_final_interface_class() -> Message {
        Message::new(
            ABSTRACT_FINAL_INTERFACE_CLASS,
            "Classes can't be declared both 'abstract' and 'final' and 'interface'.",
        )
    }

    pub(crate) fn mixin_application_class_augmentation() -> Message {
        Message::new(
            local_code(
                "mixin_application_class_augmentation",
                "MIXIN_APPLICATION_CLASS_AUGMENTATION",
            ),
            "A mixin application class can't be augmented.",
        )
    }

    pub(crate) fn extension_type_augmentation_specifies_representation_field() -> Message {
        Message::new(
            local_code(
                "extension_type_augmentation_specifies_representation_field",
                "EXTENSION_TYPE_AUGMENTATION_SPECIFIES_REPRESENTATION_FIELD",
            ),
            "An extension type augmentation can't specify a representation field.",
        )
    }

    pub(crate) fn mixin_augmentation_has_on_clause() -> Message {
        Message::new(
            local_code(
                "mixin_augmentation_has_on_clause",
                "MIXIN_AUGMENTATION_HAS_ON_CLAUSE",
            ),
            "Mixin augmentations can't have 'on' clauses.",
        )
    }

    pub(crate) fn extension_augmentation_has_on_clause() -> Message {
        Message::new(
            EXTENSION_AUGMENTATION_HAS_ON_CLAUSE,
            "Extension augmentations can't have 'on' clauses.",
        )
    }

    pub(crate) fn extension_augmentation_without_name() -> Message {
        Message::new(
            local_code(
                "extension_augmentation_without_name",
                "EXTENSION_AUGMENTATION_WITHOUT_NAME",
            ),
            "An extension augmentation must have a name.",
        )
    }

    pub(crate) fn multiple_with_clauses() -> Message {
        Message::new(
            MULTIPLE_WITH_CLAUSES,
            "Each class definition can have at most one with clause.",
        )
    }

    pub(crate) fn missing_operator_keyword() -> Message {
        Message::new(
            MISSING_KEYWORD_OPERATOR,
            "Operator declarations must be preceded by the keyword 'operator'.",
        )
    }

    pub(crate) fn new_constructor_qualified_name() -> Message {
        Message::new(
            local_code(
                "new_constructor_qualified_name",
                "NEW_CONSTRUCTOR_QUALIFIED_NAME",
            ),
            "Constructors declared with the 'new' keyword can't have qualified names.",
        )
    }

    pub(crate) fn new_constructor_new_name() -> Message {
        Message::new(
            local_code("new_constructor_new_name", "NEW_CONSTRUCTOR_NEW_NAME"),
            "Constructors declared with the 'new' keyword can't be named 'new'.",
        )
    }

    pub(crate) fn new_constructor_dot_name() -> Message {
        Message::new(
            local_code("new_constructor_dot_name", "NEW_CONSTRUCTOR_DOT_NAME"),
            "Constructors declared with the 'new' keyword can't use '.' before the constructor name.",
        )
    }

    pub(crate) fn factory_constructor_new_name() -> Message {
        Message::new(
            local_code(
                "factory_constructor_new_name",
                "FACTORY_CONSTRUCTOR_NEW_NAME",
            ),
            "Factory constructors can't be named 'new'.",
        )
    }

    pub(crate) fn pattern_variable_declaration_outside_function_or_method() -> Message {
        Message::new(
            PATTERN_VARIABLE_DECLARATION_OUTSIDE_FUNCTION_OR_METHOD,
            "Pattern variable declarations can't be used outside of a function or method.",
        )
    }

    pub(crate) fn const_field_without_initializer(name: &str) -> Message {
        Message::new(
            local_code(
                "const_field_without_initializer",
                "CONST_FIELD_WITHOUT_INITIALIZER",
            ),
            format!("The const variable '{name}' must be initialized."),
        )
    }

    pub(crate) fn final_field_without_initializer(name: &str) -> Message {
        Message::new(
            local_code(
                "final_field_without_initializer",
                "FINAL_FIELD_WITHOUT_INITIALIZER",
            ),
            format!("The final variable '{name}' must be initialized."),
        )
    }

    pub(crate) fn required_parameter_with_default() -> Message {
        Message::new(
            local_code(
                "required_parameter_with_default",
                "REQUIRED_PARAMETER_WITH_DEFAULT",
            ),
            "Non-optional parameters can't have a default value.",
        )
    }

    pub(crate) fn positional_parameter_with_equals() -> Message {
        Message::new(
            local_code(
                "positional_parameter_with_equals",
                "POSITIONAL_PARAMETER_WITH_EQUALS",
            ),
            "Positional optional parameters can't use ':' or '=' to specify a default value.",
        )
    }

    pub(crate) fn function_type_default_value() -> Message {
        Message::new(
            local_code("function_type_default_value", "FUNCTION_TYPE_DEFAULT_VALUE"),
            "Can't have a default value in a function type.",
        )
    }

    pub(crate) fn empty_optional_parameter_list() -> Message {
        Message::new(
            local_code(
                "empty_optional_parameter_list",
                "EMPTY_OPTIONAL_PARAMETER_LIST",
            ),
            "Optional parameter lists cannot be empty.",
        )
    }

    pub(crate) fn empty_named_parameter_list() -> Message {
        Message::new(
            local_code("empty_named_parameter_list", "EMPTY_NAMED_PARAMETER_LIST"),
            "Named parameter lists cannot be empty.",
        )
    }

    pub(crate) fn invalid_inline_function_type() -> Message {
        Message::new(
            local_code(
                "invalid_inline_function_type",
                "INVALID_INLINE_FUNCTION_TYPE",
            ),
            "Function-typed parameters aren't allowed.",
        )
    }

    pub(crate) fn multiple_implements() -> Message {
        Message::new(
            MULTIPLE_IMPLEMENTS_CLAUSES,
            "Each class or mixin definition can have at most one implements clause.",
        )
    }

    // —— statements (#19) ————————————————————————————————————————————————

    /// `ExpectedFunctionBody` template (pseudo-shared `missingFunctionBody`;
    /// the analyzer surfaces the registry `MISSING_FUNCTION_BODY` code).
    pub(crate) fn expected_function_body(lexeme: &str) -> Message {
        Message::new(
            MISSING_FUNCTION_BODY,
            format!("Expected a function body, but got '{lexeme}'."),
        )
    }

    pub(crate) fn only_try() -> Message {
        Message::new(
            MISSING_CATCH_OR_FINALLY,
            "A try block must be followed by an 'on', 'catch', or 'finally' clause.",
        )
    }

    pub(crate) fn catch_syntax() -> Message {
        Message::new(
            CATCH_SYNTAX,
            "'catch' must be followed by '(identifier)' or '(identifier, identifier)'.",
        )
    }

    pub(crate) fn catch_syntax_extra_parameters() -> Message {
        Message::new(
            CATCH_SYNTAX_EXTRA_PARAMETERS,
            "'catch' must be followed by '(identifier)' or '(identifier, identifier)'.",
        )
    }

    pub(crate) fn switch_has_multiple_defaults() -> Message {
        Message::new(
            SWITCH_HAS_MULTIPLE_DEFAULT_CASES,
            "The 'default' case can only be declared once.",
        )
    }

    pub(crate) fn switch_has_case_after_default() -> Message {
        Message::new(
            SWITCH_HAS_CASE_AFTER_DEFAULT_CASE,
            "The default case should be the last case in a switch statement.",
        )
    }

    pub(crate) fn colon_in_place_of_in() -> Message {
        Message::new(
            COLON_IN_PLACE_OF_IN,
            "For-in loops use 'in' rather than a colon.",
        )
    }

    pub(crate) fn invalid_await_for() -> Message {
        Message::new(
            INVALID_AWAIT_IN_FOR,
            "The keyword 'await' isn't allowed for a normal 'for' statement.",
        )
    }

    pub(crate) fn initialized_variable_in_for_each() -> Message {
        Message::new(
            INITIALIZED_VARIABLE_IN_FOR_EACH,
            "The loop variable in a for-each loop can't be initialized.",
        )
    }

    pub(crate) fn late_pattern_variable_declaration() -> Message {
        Message::new(
            LATE_PATTERN_VARIABLE_DECLARATION,
            "A pattern variable declaration may not use the `late` keyword.",
        )
    }

    pub(crate) fn break_outside_of_loop() -> Message {
        Message::new(
            BREAK_OUTSIDE_OF_LOOP,
            "A break statement can't be used outside of a loop or switch statement.",
        )
    }

    pub(crate) fn continue_outside_of_loop() -> Message {
        Message::new(
            CONTINUE_OUTSIDE_OF_LOOP,
            "A continue statement can't be used outside of a loop or switch statement.",
        )
    }

    pub(crate) fn continue_without_label_in_case() -> Message {
        Message::new(
            CONTINUE_WITHOUT_LABEL_IN_CASE,
            "A continue statement in a switch statement must have a label as a target.",
        )
    }

    // —— expression engine (#20) ——————————————————————————————————————————
    // Registry-backed templates against the same 3.13.1 oracle lock (texts
    // from the pinned `messages.yaml`).

    pub(crate) fn missing_expression_in_throw() -> Message {
        Message::new(
            MISSING_EXPRESSION_IN_THROW,
            "Missing expression after 'throw'.",
        )
    }

    pub(crate) fn expected_else_or_comma() -> Message {
        Message::new(EXPECTED_ELSE_OR_COMMA, "Expected 'else' or comma.")
    }

    pub(crate) fn null_aware_cascade_out_of_order() -> Message {
        Message::new(
            NULL_AWARE_CASCADE_OUT_OF_ORDER,
            "The '?..' cascade operator must be first in the cascade sequence.",
        )
    }

    pub(crate) fn literal_with_class(class: &str, prefix: &str) -> Message {
        Message::new(
            LITERAL_WITH_CLASS,
            format!("A {class} literal can't be prefixed by '{prefix}'."),
        )
    }

    pub(crate) fn literal_with_class_and_new(class: &str, lexeme: &str) -> Message {
        Message::new(
            LITERAL_WITH_CLASS_AND_NEW,
            format!("A {class} literal can't be prefixed by 'new {lexeme}'."),
        )
    }

    pub(crate) fn literal_with_new() -> Message {
        Message::new(LITERAL_WITH_NEW, "A literal can't be prefixed by 'new'.")
    }

    pub(crate) fn default_in_switch_expression() -> Message {
        Message::new(
            DEFAULT_IN_SWITCH_EXPRESSION,
            "A switch expression may not use the `default` keyword.",
        )
    }

    pub(crate) fn named_function_expression() -> Message {
        Message::new(
            NAMED_FUNCTION_EXPRESSION,
            "A function expression can't have a name.",
        )
    }

    pub(crate) fn binary_operator_written_out(written: &str, operator: &str) -> Message {
        Message::new(
            BINARY_OPERATOR_WRITTEN_OUT,
            format!(
                "Binary operator '{written}' is written as '{operator}' instead of the written out word."
            ),
        )
    }
}

/// Block-shape expectations for `ensureBlock` recovery (official
/// `BlockKind`, restricted to the declaration families this milestone
/// parses).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BlockKind {
    /// Function/statement body (`skipBlock` with no declaration kind;
    /// reports `Expected '{' before this.`).
    Body,
    /// A plain statement block (`parseBlock`, `BlockKind.statement`; never
    /// reports — only entered with `{` present).
    Statement,
    /// The block after `try`.
    TryStatement,
    /// The block after `on`/`catch`.
    CatchClause,
    /// The block after `finally`.
    FinallyClause,
    /// A switch statement body.
    SwitchStatement,
    /// A switch expression body (`BlockKind.switchExpression`; official
    /// `expectedSwitchExpressionBody` — a shared-parser own code, absent
    /// from the syntax-codes capture, so `local_code`). Emit-vs-cull
    /// decision (pinned oracle, block_kind.dart:58 + parser_impl.dart
    /// :12433): EMIT — `ensureBlock` reports it live on `switch (n);`
    /// (script in messages.yaml), with the report anchored on the token
    /// before the missing `{`.
    SwitchExpression,
    Class,
    Mixin,
    Enum,
    Extension,
    ExtensionType,
}
