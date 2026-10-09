//! Diagnostic messages handed to the listener.
//!
//! The parser never collects diagnostics; it reports a [`Message`] (official
//! `Message` analog: code + formatted problem text) through
//! [`ParserListener::handle_recoverable_error`], and the listener owns the
//! translation into a `dartiq_diagnostics::Diagnostic` (file attribution is
//! a listener concern).
//!
//! Codes come from the `dartiq-diagnostics` registry (single source of
//! truth; every code used here is a registry constant). Message texts follow
//! the official templates (`_fe_analyzer_shared/messages.yaml`), matching the
//! (code, file) oracle differential of PRD §19.

use dartiq_diagnostics::CodeInfo;

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
    use dartiq_diagnostics::{
        ABSTRACT_CLASS_MEMBER, ABSTRACT_EXTERNAL_FIELD, BASE_ENUM, CLASS_IN_CLASS,
        CONFLICTING_MODIFIERS, CONST_AND_FINAL, CONST_METHOD, CONSTRUCTOR_WITH_RETURN_TYPE,
        COVARIANT_AND_STATIC, COVARIANT_MEMBER, DUPLICATED_MODIFIER, ENUM_IN_CLASS,
        EXPECTED_CLASS_MEMBER, EXPECTED_EXECUTABLE, EXPECTED_INSTEAD,
        EXTENSION_DECLARES_CONSTRUCTOR, EXTENSION_TYPE_EXTENDS, EXTENSION_TYPE_WITH,
        EXTERNAL_CONSTRUCTOR_WITH_INITIALIZER, EXTERNAL_FACTORY_REDIRECTION,
        EXTERNAL_FACTORY_WITH_BODY, EXTERNAL_METHOD_WITH_BODY, FACTORY_TOP_LEVEL_DECLARATION,
        FINAL_AND_COVARIANT, FINAL_AND_COVARIANT_LATE_WITH_INITIALIZER, FINAL_AND_VAR, FINAL_ENUM,
        FINAL_MIXIN, FINAL_MIXIN_CLASS, GETTER_CONSTRUCTOR, GETTER_WITH_PARAMETERS,
        IMPLEMENTS_BEFORE_EXTENDS, IMPLEMENTS_BEFORE_ON, IMPLEMENTS_BEFORE_WITH, INTERFACE_ENUM,
        INTERFACE_MIXIN, INTERFACE_MIXIN_CLASS, INVALID_CONSTRUCTOR_NAME, MEMBER_WITH_CLASS_NAME,
        MISSING_CONST_FINAL_VAR_OR_TYPE, MISSING_FUNCTION_BODY, MISSING_FUNCTION_PARAMETERS,
        MISSING_METHOD_PARAMETERS, MISSING_PRIMARY_CONSTRUCTOR,
        MISSING_PRIMARY_CONSTRUCTOR_PARAMETERS, MISSING_STAR_AFTER_SYNC,
        MISSING_TYPEDEF_PARAMETERS, MIXIN_DECLARES_CONSTRUCTOR, MODIFIER_OUT_OF_ORDER,
        MULTIPLE_CLAUSES, MULTIPLE_EXTENDS_CLAUSES, NATIVE_CLAUSE_SHOULD_BE_ANNOTATION,
        OUT_OF_ORDER_CLAUSES, REDIRECTION_IN_NON_FACTORY_CONSTRUCTOR, SEALED_ENUM, SEALED_MIXIN,
        SEALED_MIXIN_CLASS, SETTER_CONSTRUCTOR, STATIC_CONSTRUCTOR, STATIC_OPERATOR,
        TOP_LEVEL_OPERATOR, TYPE_BEFORE_FACTORY, TYPEDEF_IN_CLASS, UNEXPECTED_TOKEN,
        VAR_RETURN_TYPE, WITH_BEFORE_EXTENDS,
    };
    use dartiq_diagnostics::{
        ABSTRACT_FINAL_BASE_CLASS, ABSTRACT_FINAL_INTERFACE_CLASS, ABSTRACT_SEALED_CLASS,
        EXTENSION_AUGMENTATION_HAS_ON_CLAUSE, EXTRANEOUS_MODIFIER, MISSING_KEYWORD_OPERATOR,
        MIXIN_WITH_CLAUSE, MULTIPLE_IMPLEMENTS_CLAUSES, MULTIPLE_ON_CLAUSES, MULTIPLE_WITH_CLAUSES,
        PATTERN_VARIABLE_DECLARATION_OUTSIDE_FUNCTION_OR_METHOD,
    };
    use dartiq_diagnostics::{
        ANNOTATION_ON_TYPE_ARGUMENT, ANNOTATION_WITH_TYPE_ARGUMENTS_UNINSTANTIATED,
        EMPTY_RECORD_TYPE_NAMED_FIELDS_LIST, EMPTY_RECORD_TYPE_WITH_COMMA,
        EQUALITY_CANNOT_BE_EQUALITY_OPERAND, EXPECTED_TOKEN, EXPECTED_TYPE_NAME,
        ILLEGAL_PATTERN_ASSIGNMENT_VARIABLE_NAME, ILLEGAL_PATTERN_IDENTIFIER_NAME,
        ILLEGAL_PATTERN_VARIABLE_NAME, INVALID_CONSTANT_CONST_PREFIX,
        INVALID_CONSTANT_PATTERN_BINARY, INVALID_INSIDE_UNARY_PATTERN, MISSING_IDENTIFIER,
        MULTIPLE_VARIANCE_MODIFIERS, PATTERN_ASSIGNMENT_DECLARES_VARIABLE,
        RECORD_LITERAL_ONE_POSITIONAL_NO_TRAILING_COMMA,
        RECORD_TYPE_ONE_POSITIONAL_NO_TRAILING_COMMA, VAR_AND_TYPE,
        VARIABLE_PATTERN_KEYWORD_IN_DECLARATION_CONTEXT, VOID_WITH_TYPE_ARGUMENTS,
    };
    use dartiq_diagnostics::{CodeInfo, DiagnosticType};

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

    /// `expected{Class,Mixin,Enum,Extension,ExtensionType}Body` (shared-parser
    /// own codes; not in the analyzer registry capture).
    pub(crate) fn expected_block_body(kind: BlockKind, lexeme: &str) -> Message {
        if kind == BlockKind::Body {
            return expected_but_got("{");
        }
        let (name, machine, text) = match kind {
            BlockKind::Class => (
                "expected_class_body",
                "EXPECTED_CLASS_BODY",
                format!("Expected a class body, but got '{lexeme}'."),
            ),
            BlockKind::Mixin => (
                "expected_mixin_body",
                "EXPECTED_MIXIN_BODY",
                format!("Expected a mixin body, but got '{lexeme}'."),
            ),
            BlockKind::Enum => (
                "expected_enum_body",
                "EXPECTED_ENUM_BODY",
                format!("Expected a enum body, but got '{lexeme}'."),
            ),
            BlockKind::Extension => (
                "expected_extension_body",
                "EXPECTED_EXTENSION_BODY",
                format!("Expected a extension body, but got '{lexeme}'."),
            ),
            BlockKind::ExtensionType => (
                "expected_extension_type_body",
                "EXPECTED_EXTENSION_TYPE_BODY",
                format!("Expected a extension type body, but got '{lexeme}'."),
            ),
            BlockKind::Body => unreachable!("handled above"),
        };
        Message::new(local_code(name, machine), text)
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
}

/// Block-shape expectations for `ensureBlock` recovery (official
/// `BlockKind`, restricted to the declaration families this milestone
/// parses).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BlockKind {
    /// Function/statement body (`skipBlock` with no declaration kind;
    /// reports `Expected '{' before this.`).
    Body,
    Class,
    Mixin,
    Enum,
    Extension,
    ExtensionType,
}
