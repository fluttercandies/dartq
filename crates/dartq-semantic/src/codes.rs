//! The M1 semantic diagnostic code table: the frozen M1 gate families the
//! semantic layer owns (`tests/harness/m1_codes.txt`), with tier and verbatim
//! oracle message per code.
//!
//! One variant per implemented frozen row. Every row was transcribed from
//! the analyzer-8.4.1 registry and re-verified verbatim against the pinned
//! live registry (`.third/dart-sdk`
//! `pkg/analyzer/lib/src/diagnostic/diagnostic.g.dart`, dart 3.15 main,
//! analyzer 14.6.0-dev) — that
//! unified registry is also the line-anchor source for the per-variant doc
//! comments and the tier split below: the 8.4.1 `codes.g.dart` /
//! `syntactic_errors.g.dart` files do not exist in the pinned checkout, and
//! the syntactic rows were absorbed into the unified registry (#121, #222).
//! Tier split:
//!
//! * the `undefined_*` family, `duplicate_definition`, and the
//!   `wrong_number_of_type_arguments*` / `..._parameters_for_operator`
//!   rows are compile-time errors (oracle severity ERROR,
//!   `DiagnosticType.COMPILE_TIME_ERROR`);
//! * `wrong_number_of_parameters_for_setter` is a *syntactic* row
//!   (`diagnostic.g.dart:18561-18571`, `SYNTACTIC_ERROR` at :18568; fired
//!   from `pkg/analyzer/lib/src/fasta/ast_builder.dart:6477,6510` in the
//!   oracle): the trigger lives
//!   in the parser there, and dartq's parser does not emit it yet — the
//!   semantic layer owns the check until the parser lands it (remove this
//!   site then, or the code double-reports);
//! * `invalid_null_aware_operator` is the frozen set's only `STATIC_WARNING`
//!   (`diagnostic.g.dart`).
//!
//! The second batch's use-site split for the type-argument family follows
//! the pinned oracle registry (`.third/dart-sdk` `pkg/analyzer/lib/src/
//! diagnostic/diagnostic.g.dart`): type annotations and instance-creation
//! type references report the base code, named-constructor references
//! report `_constructor`, enum-constant type arguments report `_enum`,
//! explicit extension overrides report `_extension`, and every resolved
//! executable applied or invoked with explicit type arguments reports
//! `_element` — the message names the executable's kind word, `function`
//! or `method` (`type_instantiation_target.dart`,
//! `InvocationTargetExecutableElement.wrongNumberOfTypeArgumentsError`).
//! `_function` fires only when the instantiation operand is a *function-typed
//! expression* (`type_instantiation_target.dart`,
//! `InvocationTargetFunctionTypedExpression`) and its message displays the
//! function *type* — a display the M1 [`crate::model::Ty`] cannot render, so
//! the row is implemented here but has no emit site (suppress,
//! under-report). The older 8.4.1 registry split carried a dedicated
//! `_method` row instead of `_element`; the pinned oracle no longer has it,
//! so a `_method` emission could never match the differential gate.
//!
//! The same family lives next to its emit layer by the repo's per-family
//! table convention (`dartq-resolution/src/codes.rs` is the resolution
//! family's table); the M0-measured `duplicate_definition` /
//! `undefined_identifier` / `argument_type_not_assignable` constants in
//! `dartq-diagnostics/src/code.rs` stay untouched — this table is the
//! semantic emit-side authority and produces the [`CodeInfo`] the
//! diagnostic model consumes.
//!
//! Frozen gate rows this table deliberately does not carry (each
//! under-reports, never false-positives; see the [`crate::diagnose`] module
//! docs): `undefined_referenced_parameter` (`@UseResult(unless:)`-specific
//! check, `annotation_verifier.dart:519-541`) and `undefined_hidden_name` /
//! `undefined_shown_name` (`STATIC_WARNING` tier; the parser consumes
//! directives diet-style and resolution's [`Combinators`] exposes no clause
//! enumeration — both arrive with the directive milestone). The
//! constructor-selector rows (`undefined_constructor_in_initializer` /
//! `undefined_enum_constructor`) joined in #350 with the constructor-name
//! capture; the unnamed-selector message variants stay deferred
//! (under-report). `wrong_number_of_type_arguments_function` carries the
//! row and #350 added its emit site (the function-type display renders from
//! the captured member signature). Every other frozen gate
//! row is carried: the `wrong_number_of_type_arguments*` family (including
//! `_element`, with emit sites in [`crate::diagnose`]) and
//! `wrong_number_of_parameters_for_operator` are implemented above, and
//! `uri_does_not_exist` lives in the M0 official registry
//! (`dartq-diagnostics`), emitted by the resolution layer.

use dartq_diagnostics::{CodeInfo, DiagnosticType, Severity};

/// One implemented frozen semantic code: its official descriptor and the
/// verbatim oracle message, parameterized at the emit site.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SemanticCode {
    /// `The name '{0}' is already defined.`
    DuplicateDefinition,
    /// `Undefined name '{0}' used as an annotation.` — the emit site is
    /// [`crate::diagnose`]'s annotation check (a fully missing
    /// `MetadataReference` name), and upstream removed the
    /// diagnostic in 3.14 (`removedIn: "3.14"` — absent from the pinned
    /// 3.15-main registry, only `messages.yaml` retains the text). The row
    /// stays for the 3.13.1 measurement oracle, which still emits it and
    /// freezes it in `tests/harness/m1_codes.txt` — under-report, never
    /// false-positive.
    UndefinedAnnotation,
    /// `Undefined class '{0}'.`
    UndefinedClass,
    /// `The class '{0}' doesn't have a constructor named '{1}'.` — a
    /// `super.named()` initializer invokes a constructor the superclass does
    /// not declare (`undefined_constructor_in_initializer`; the
    /// unnamed-selector variant `..._default` stays deferred).
    UndefinedConstructorInInitializer,
    /// `The enum doesn't have a constructor named '{0}'.` — an enum
    /// constant's constructor selector names a constructor the enum does
    /// not declare (the unnamed variant `..._unnamed` stays deferred).
    UndefinedEnumConstructor,
    /// `There's no constant named '{0}' in '{1}'.`
    UndefinedEnumConstant,
    /// `The getter '{0}' isn't defined for the extension '{1}'.`
    UndefinedExtensionGetter,
    /// `The method '{0}' isn't defined for the extension '{1}'.`
    UndefinedExtensionMethod,
    /// `The operator '{0}' isn't defined for the extension '{1}'.`
    UndefinedExtensionOperator,
    /// `The setter '{0}' isn't defined for the extension '{1}'.`
    UndefinedExtensionSetter,
    /// `The function '{0}' isn't defined.`
    UndefinedFunction,
    /// `The getter '{0}' isn't defined for the type '{1}'.`
    UndefinedGetter,
    /// `Undefined name '{0}'.`
    UndefinedIdentifier,
    /// `Undefined name 'await' in function body not marked with 'async'.`
    /// (no arguments — `CompileTimeErrorWithoutArguments`)
    UndefinedIdentifierAwait,
    /// `The method '{0}' isn't defined for the type '{1}'.`
    UndefinedMethod,
    /// `The named parameter '{0}' isn't defined.`
    UndefinedNamedParameter,
    /// `The operator '{0}' isn't defined for the type '{1}'.`
    UndefinedOperator,
    /// `The name '{0}' is being referenced through the prefix '{1}', but it
    /// isn't defined in any of the libraries imported using that prefix.`
    UndefinedPrefixedName,
    /// `The setter '{0}' isn't defined for the type '{1}'.`
    UndefinedSetter,
    /// `UNDEFINED_SUPER_MEMBER`: one code name with a per-member-shape message
    /// (`uniqueName` `UNDEFINED_SUPER_GETTER`/`_METHOD`/`_OPERATOR`/`_SETTER`).
    UndefinedSuperMember(SuperShape),
    /// `Operator '{0}' should declare exactly {1} parameters, but {2} found.`
    /// (analyzerCode `WRONG_NUMBER_OF_PARAMETERS_FOR_OPERATOR`)
    WrongNumberOfParametersForOperator,
    /// `Operator '-' should declare 0 or 1 parameter, but {0} found.`
    /// (same analyzerCode, `uniqueName` `..._MINUS`)
    WrongNumberOfParametersForOperatorMinus,
    /// `Setters must declare exactly one required positional parameter.`
    /// (no arguments — `ParserErrorWithoutArguments`)
    WrongNumberOfParametersForSetter,
    /// `The type '{0}' is declared with {1} type parameters, but {2} type
    /// arguments were given.`
    WrongNumberOfTypeArguments,
    /// `The constructor '{0}.{1}' doesn't have type parameters.`
    WrongNumberOfTypeArgumentsConstructor,
    /// `The enum is declared with {0} type parameters, but {1} type arguments
    /// were given.` (counts only — the template carries no name)
    WrongNumberOfTypeArgumentsEnum,
    /// `The extension '{0}' is declared with {1} type parameters, but {2}
    /// type arguments were given.`
    WrongNumberOfTypeArgumentsExtension,
    /// `The type of this function is '{0}', which has {1} type parameters,
    /// but {2} type arguments were given.` — `{0}` is the function type
    /// itself; M1 cannot render function types, so no emit site fires.
    WrongNumberOfTypeArgumentsFunction,
    /// `The {0} '{1}' is declared with {2} type parameters, but {3}
    /// type arguments are given.` (analyzerCode
    /// `wrong_number_of_type_arguments_element`; `{0}` is the executable's
    /// oracle display kind).
    WrongNumberOfTypeArgumentsElement(ExecKind),
    /// `A value of type '{0}' can't be assigned to a variable of type '{1}'.`
    InvalidAssignment,
    /// `The argument type '{0}' can't be assigned to the parameter type
    /// '{1}'. {2}` — `{2}` is the record-shape appendix, empty at M1
    /// (records are M2), rendered as the template does.
    ArgumentTypeNotAssignable,
    /// `A value of type '{0}' can't be returned from the {host} '{2}'
    /// because it has a return type of '{1}'.` — one analyzerCode with a
    /// per-host message (`uniqueName` `..._FROM_FUNCTION`/`_FROM_METHOD`).
    ReturnOfInvalidType(ExecKind),
    /// `The receiver can't be null, so the null-aware operator '{0}' is
    /// unnecessary.` (`STATIC_WARNING`)
    InvalidNullAwareOperator,
    /// `The type '{0}' isn't exhaustively matched by the switch cases since
    /// it doesn't match the pattern '{1}'.` — switch statements
    /// (`COMPILE_TIME_ERROR`, at the `switch` keyword; emitted only when no
    /// `default` is present and the scrutinee's static type is always
    /// exhaustive, `constant_verifier.dart:1017-1140` +
    /// `type_system.dart:846-887`). `{2}` is the suggested correction
    /// pattern (the witness's `asCorrection` text).
    NonExhaustiveSwitchStatement,
    /// The same template for switch expressions, which are always
    /// mustBeExhaustive (`constant_verifier.dart:1136-1140`).
    NonExhaustiveSwitchExpression,
    /// `This case is covered by the previous cases.` (`STATIC_WARNING`, at
    /// the `case` keyword of a statement case / the `=>` of an expression
    /// case, `constant_verifier.dart:1080-1088`).
    UnreachableSwitchCase,
    /// `This default clause is covered by the previous cases.`
    /// (`STATIC_WARNING`, at the `default` keyword: the case spaces
    /// already cover the scrutinee and the switch is mustBeExhaustive,
    /// `constant_verifier.dart:1141-1148`).
    UnreachableSwitchDefault,
    // —— #279: the constant-evaluation family (all `COMPILE_TIME_ERROR`) ——
    /// `Const variables must be initialized with a constant value.` — a
    /// const variable/field initializer that is decidentally not constant
    /// (the `visitVariableDeclaration` site default,
    /// `constant_verifier.dart:550-556`).
    ConstInitializedWithNonConstantValue,
    /// `Arguments of a constant creation must be constant expressions.` —
    /// an argument of a const creation / enum constant (the
    /// `_validateConstantArguments` site default, `constant_verifier.dart:843`).
    ConstWithNonConstantArgument,
    /// `The values in a const list literal must be constants.`
    NonConstantListElement,
    /// `The values in a const set literal must be constants.`
    NonConstantSetElement,
    /// `The elements in a const map literal must be constant.`
    NonConstantMapElement,
    /// `The fields in a const record literal must be constants.`
    NonConstantRecordField,
    /// `In constant expressions, operands of this operator must be of type
    /// 'bool'.`
    ConstEvalTypeBool,
    /// `In constant expressions, operands of this operator must be of type
    /// 'bool' or 'int'.`
    ConstEvalTypeBoolInt,
    /// `In constant expressions, operands of this operator must be of type
    /// 'bool', 'num', 'String' or 'null'.`
    ConstEvalTypeBoolNumString,
    /// `In constant expressions, operands of this operator must be of type
    /// 'int'.`
    ConstEvalTypeInt,
    /// `In constant expressions, operands of this operator must be of type
    /// 'num'.`
    ConstEvalTypeNum,
    /// `In constant expressions, operands of this operator must be of type
    /// 'num' or 'String'.`
    ConstEvalTypeNumString,
    /// `In constant expressions, operands of this operator must be of type
    /// 'String'.`
    ConstEvalTypeString,
    /// `Evaluation of this constant expression throws an exception.`
    ConstEvalThrowsException,
    /// `Evaluation of this constant expression throws an
    /// IntegerDivisionByZeroException.`
    ConstEvalThrowsIdbze,
    /// `In constant expressions, operands of the equality operator must
    /// have primitive equality.`
    ConstEvalPrimitiveEquality,
    /// `Methods can't be invoked in constant expressions.` — an invocation
    /// of a non-constructor callee in a const context (the
    /// `_invalidConstantForMethodInvocation` site,
    /// `evaluation.dart:2197-2212`).
    ConstEvalMethodInvocation,
    /// `The compile-time constant expression depends on itself.`
    RecursiveCompileTimeConstant,
}

/// The executable-kind word the per-host oracle templates pick: the return
/// host of [`SemanticCode::ReturnOfInvalidType`] and the element kind of
/// [`SemanticCode::WrongNumberOfTypeArgumentsElement`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ExecKind {
    /// `function`.
    Function,
    /// `method`.
    Method,
}

impl ExecKind {
    /// The verbatim oracle word.
    #[must_use]
    pub const fn word(self) -> &'static str {
        match self {
            Self::Function => "function",
            Self::Method => "method",
        }
    }
}

/// The member shape of an [`SemanticCode::UndefinedSuperMember`] row: the
/// oracle splits one code name into four message texts by access shape.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SuperShape {
    /// `The getter '{0}' isn't defined in a superclass of '{1}'.`
    Getter,
    /// `The method '{0}' isn't defined in a superclass of '{1}'.`
    Method,
    /// `The operator '{0}' isn't defined in a superclass of '{1}'.`
    Operator,
    /// `The setter '{0}' isn't defined in a superclass of '{1}'.`
    Setter,
}

impl SemanticCode {
    /// The stable official descriptor: name, machine form, family type, and
    /// the published-docs flag.
    #[must_use]
    pub fn code_info(self) -> CodeInfo {
        let (name, machine) = match self {
            Self::DuplicateDefinition => ("duplicate_definition", "DUPLICATE_DEFINITION"),
            Self::UndefinedAnnotation => ("undefined_annotation", "UNDEFINED_ANNOTATION"),
            Self::UndefinedClass => ("undefined_class", "UNDEFINED_CLASS"),
            Self::UndefinedConstructorInInitializer => (
                "undefined_constructor_in_initializer",
                "UNDEFINED_CONSTRUCTOR_IN_INITIALIZER",
            ),
            Self::UndefinedEnumConstructor => {
                ("undefined_enum_constructor", "UNDEFINED_ENUM_CONSTRUCTOR")
            }
            Self::UndefinedEnumConstant => ("undefined_enum_constant", "UNDEFINED_ENUM_CONSTANT"),
            Self::UndefinedExtensionGetter => {
                ("undefined_extension_getter", "UNDEFINED_EXTENSION_GETTER")
            }
            Self::UndefinedExtensionMethod => {
                ("undefined_extension_method", "UNDEFINED_EXTENSION_METHOD")
            }
            Self::UndefinedExtensionOperator => (
                "undefined_extension_operator",
                "UNDEFINED_EXTENSION_OPERATOR",
            ),
            Self::UndefinedExtensionSetter => {
                ("undefined_extension_setter", "UNDEFINED_EXTENSION_SETTER")
            }
            Self::UndefinedFunction => ("undefined_function", "UNDEFINED_FUNCTION"),
            Self::UndefinedGetter => ("undefined_getter", "UNDEFINED_GETTER"),
            Self::UndefinedIdentifier => ("undefined_identifier", "UNDEFINED_IDENTIFIER"),
            Self::UndefinedIdentifierAwait => {
                ("undefined_identifier_await", "UNDEFINED_IDENTIFIER_AWAIT")
            }
            Self::UndefinedMethod => ("undefined_method", "UNDEFINED_METHOD"),
            Self::UndefinedNamedParameter => {
                ("undefined_named_parameter", "UNDEFINED_NAMED_PARAMETER")
            }
            Self::UndefinedOperator => ("undefined_operator", "UNDEFINED_OPERATOR"),
            Self::UndefinedPrefixedName => ("undefined_prefixed_name", "UNDEFINED_PREFIXED_NAME"),
            Self::UndefinedSetter => ("undefined_setter", "UNDEFINED_SETTER"),
            // The analyzer registry keeps one `analyzerCode` for the whole
            // super-member family; the shape lives in the `uniqueName`.
            Self::UndefinedSuperMember(_) => ("undefined_super_member", "UNDEFINED_SUPER_MEMBER"),
            Self::WrongNumberOfParametersForOperator
            | Self::WrongNumberOfParametersForOperatorMinus => (
                "wrong_number_of_parameters_for_operator",
                "WRONG_NUMBER_OF_PARAMETERS_FOR_OPERATOR",
            ),
            Self::WrongNumberOfParametersForSetter => (
                "wrong_number_of_parameters_for_setter",
                "WRONG_NUMBER_OF_PARAMETERS_FOR_SETTER",
            ),
            Self::WrongNumberOfTypeArguments => (
                "wrong_number_of_type_arguments",
                "WRONG_NUMBER_OF_TYPE_ARGUMENTS",
            ),
            Self::WrongNumberOfTypeArgumentsConstructor => (
                "wrong_number_of_type_arguments_constructor",
                "WRONG_NUMBER_OF_TYPE_ARGUMENTS_CONSTRUCTOR",
            ),
            Self::WrongNumberOfTypeArgumentsEnum => (
                "wrong_number_of_type_arguments_enum",
                "WRONG_NUMBER_OF_TYPE_ARGUMENTS_ENUM",
            ),
            Self::WrongNumberOfTypeArgumentsExtension => (
                "wrong_number_of_type_arguments_extension",
                "WRONG_NUMBER_OF_TYPE_ARGUMENTS_EXTENSION",
            ),
            Self::WrongNumberOfTypeArgumentsFunction => (
                "wrong_number_of_type_arguments_function",
                "WRONG_NUMBER_OF_TYPE_ARGUMENTS_FUNCTION",
            ),
            Self::WrongNumberOfTypeArgumentsElement(_) => (
                "wrong_number_of_type_arguments_element",
                "WRONG_NUMBER_OF_TYPE_ARGUMENTS_ELEMENT",
            ),
            Self::InvalidAssignment => ("invalid_assignment", "INVALID_ASSIGNMENT"),
            Self::ArgumentTypeNotAssignable => (
                "argument_type_not_assignable",
                "ARGUMENT_TYPE_NOT_ASSIGNABLE",
            ),
            Self::ReturnOfInvalidType(_) => ("return_of_invalid_type", "RETURN_OF_INVALID_TYPE"),
            Self::InvalidNullAwareOperator => {
                ("invalid_null_aware_operator", "INVALID_NULL_AWARE_OPERATOR")
            }
            Self::NonExhaustiveSwitchStatement => (
                "non_exhaustive_switch_statement",
                "NON_EXHAUSTIVE_SWITCH_STATEMENT",
            ),
            Self::NonExhaustiveSwitchExpression => (
                "non_exhaustive_switch_expression",
                "NON_EXHAUSTIVE_SWITCH_EXPRESSION",
            ),
            Self::UnreachableSwitchCase => ("unreachable_switch_case", "UNREACHABLE_SWITCH_CASE"),
            Self::UnreachableSwitchDefault => {
                ("unreachable_switch_default", "UNREACHABLE_SWITCH_DEFAULT")
            }
            Self::ConstInitializedWithNonConstantValue => (
                "const_initialized_with_non_constant_value",
                "CONST_INITIALIZED_WITH_NON_CONSTANT_VALUE",
            ),
            Self::ConstWithNonConstantArgument => (
                "const_with_non_constant_argument",
                "CONST_WITH_NON_CONSTANT_ARGUMENT",
            ),
            Self::NonConstantListElement => {
                ("non_constant_list_element", "NON_CONSTANT_LIST_ELEMENT")
            }
            Self::NonConstantSetElement => ("non_constant_set_element", "NON_CONSTANT_SET_ELEMENT"),
            Self::NonConstantMapElement => ("non_constant_map_element", "NON_CONSTANT_MAP_ELEMENT"),
            Self::NonConstantRecordField => {
                ("non_constant_record_field", "NON_CONSTANT_RECORD_FIELD")
            }
            Self::ConstEvalTypeBool => ("const_eval_type_bool", "CONST_EVAL_TYPE_BOOL"),
            Self::ConstEvalTypeBoolInt => ("const_eval_type_bool_int", "CONST_EVAL_TYPE_BOOL_INT"),
            Self::ConstEvalTypeBoolNumString => (
                "const_eval_type_bool_num_string",
                "CONST_EVAL_TYPE_BOOL_NUM_STRING",
            ),
            Self::ConstEvalTypeInt => ("const_eval_type_int", "CONST_EVAL_TYPE_INT"),
            Self::ConstEvalTypeNum => ("const_eval_type_num", "CONST_EVAL_TYPE_NUM"),
            Self::ConstEvalTypeNumString => {
                ("const_eval_type_num_string", "CONST_EVAL_TYPE_NUM_STRING")
            }
            Self::ConstEvalTypeString => ("const_eval_type_string", "CONST_EVAL_TYPE_STRING"),
            Self::ConstEvalThrowsException => {
                ("const_eval_throws_exception", "CONST_EVAL_THROWS_EXCEPTION")
            }
            Self::ConstEvalThrowsIdbze => ("const_eval_throws_idbze", "CONST_EVAL_THROWS_IDBZE"),
            Self::ConstEvalPrimitiveEquality => (
                "const_eval_primitive_equality",
                "CONST_EVAL_PRIMITIVE_EQUALITY",
            ),
            Self::ConstEvalMethodInvocation => (
                "const_eval_method_invocation",
                "CONST_EVAL_METHOD_INVOCATION",
            ),
            Self::RecursiveCompileTimeConstant => (
                "recursive_compile_time_constant",
                "RECURSIVE_COMPILE_TIME_CONSTANT",
            ),
        };
        let dtype = self.dtype();
        // The function variant is the one wrong-number template without a
        // `hasPublishedDocs` field (diagnostic.g.dart:18725-18744).
        let published = !matches!(self, Self::WrongNumberOfTypeArgumentsFunction);
        CodeInfo::new(name, machine, dtype, published)
    }

    /// The oracle tier of the row — the single derivation of the family
    /// split: the frozen set is compile-time errors except the setter row
    /// (a *syntactic* error in the oracle, whose trigger lives in its
    /// parser, `ast_builder.dart:6477,6510`) and the two static warnings
    /// (`diagnostic.g.dart:17726`). Both [`SemanticCode::code_info`] and
    /// [`SemanticCode::severity`] fold onto it.
    #[must_use]
    pub const fn dtype(self) -> DiagnosticType {
        match self {
            Self::WrongNumberOfParametersForSetter => DiagnosticType::SyntacticError,
            Self::InvalidNullAwareOperator
            | Self::UnreachableSwitchCase
            | Self::UnreachableSwitchDefault => DiagnosticType::StaticWarning,
            _ => DiagnosticType::CompileTimeError,
        }
    }

    /// Oracle severity: folded onto the tier derivation
    /// ([`DiagnosticType::severity`] — the single severity mapping, the
    /// `dtype_matches_oracle_severity` invariant): every row is an error
    /// except the two static warnings.
    #[must_use]
    pub const fn severity(self) -> Severity {
        self.dtype().severity()
    }

    /// The verbatim oracle `problemMessage` with placeholders filled.
    #[must_use]
    pub fn message(self, a0: &str, a1: &str) -> String {
        match self {
            Self::DuplicateDefinition => format!("The name '{a0}' is already defined."),
            Self::UndefinedAnnotation => {
                format!("Undefined name '{a0}' used as an annotation.")
            }
            Self::UndefinedClass => format!("Undefined class '{a0}'."),
            Self::UndefinedConstructorInInitializer => {
                format!("The class '{a0}' doesn't have a constructor named '{a1}'.")
            }
            Self::UndefinedEnumConstructor => {
                format!("The enum doesn't have a constructor named '{a0}'.")
            }
            Self::UndefinedEnumConstant => {
                format!("There's no constant named '{a0}' in '{a1}'.")
            }
            Self::UndefinedExtensionGetter => {
                format!("The getter '{a0}' isn't defined for the extension '{a1}'.")
            }
            Self::UndefinedExtensionMethod => {
                format!("The method '{a0}' isn't defined for the extension '{a1}'.")
            }
            Self::UndefinedExtensionOperator => {
                format!("The operator '{a0}' isn't defined for the extension '{a1}'.")
            }
            Self::UndefinedExtensionSetter => {
                format!("The setter '{a0}' isn't defined for the extension '{a1}'.")
            }
            Self::UndefinedFunction => format!("The function '{a0}' isn't defined."),
            Self::UndefinedGetter => {
                format!("The getter '{a0}' isn't defined for the type '{a1}'.")
            }
            Self::UndefinedIdentifier => format!("Undefined name '{a0}'."),
            // Verbatim: the message names `await` itself and takes no arguments.
            Self::UndefinedIdentifierAwait => {
                "Undefined name 'await' in function body not marked with 'async'.".to_string()
            }
            Self::UndefinedMethod => {
                format!("The method '{a0}' isn't defined for the type '{a1}'.")
            }
            Self::UndefinedNamedParameter => format!("The named parameter '{a0}' isn't defined."),
            Self::UndefinedOperator => {
                format!("The operator '{a0}' isn't defined for the type '{a1}'.")
            }
            Self::UndefinedPrefixedName => format!(
                "The name '{a0}' is being referenced through the prefix '{a1}', but it isn't \
                 defined in any of the libraries imported using that prefix."
            ),
            Self::UndefinedSetter => {
                format!("The setter '{a0}' isn't defined for the type '{a1}'.")
            }
            Self::UndefinedSuperMember(shape) => match shape {
                SuperShape::Getter => {
                    format!("The getter '{a0}' isn't defined in a superclass of '{a1}'.")
                }
                SuperShape::Method => {
                    format!("The method '{a0}' isn't defined in a superclass of '{a1}'.")
                }
                SuperShape::Operator => {
                    format!("The operator '{a0}' isn't defined in a superclass of '{a1}'.")
                }
                SuperShape::Setter => {
                    format!("The setter '{a0}' isn't defined in a superclass of '{a1}'.")
                }
            },
            // No-argument rows: verbatim texts.
            Self::WrongNumberOfParametersForSetter => {
                "Setters must declare exactly one required positional parameter.".to_string()
            }
            // The #279 constant-evaluation family: no-argument rows (the
            // pinned registry `diagnostic.g.dart` texts).
            Self::ConstInitializedWithNonConstantValue => {
                "Const variables must be initialized with a constant value.".to_string()
            }
            Self::ConstWithNonConstantArgument => {
                "Arguments of a constant creation must be constant expressions.".to_string()
            }
            Self::NonConstantListElement => {
                "The values in a const list literal must be constants.".to_string()
            }
            Self::NonConstantSetElement => {
                "The values in a const set literal must be constants.".to_string()
            }
            Self::NonConstantMapElement => {
                "The elements in a const map literal must be constant.".to_string()
            }
            Self::NonConstantRecordField => {
                "The fields in a const record literal must be constants.".to_string()
            }
            Self::ConstEvalTypeBool => {
                "In constant expressions, operands of this operator must be of type 'bool'."
                    .to_string()
            }
            Self::ConstEvalTypeBoolInt => {
                "In constant expressions, operands of this operator must be of type 'bool' or 'int'."
                    .to_string()
            }
            Self::ConstEvalTypeBoolNumString => {
                "In constant expressions, operands of this operator must be of type 'bool', 'num', 'String' or 'null'."
                    .to_string()
            }
            Self::ConstEvalTypeInt => {
                "In constant expressions, operands of this operator must be of type 'int'."
                    .to_string()
            }
            Self::ConstEvalTypeNum => {
                "In constant expressions, operands of this operator must be of type 'num'."
                    .to_string()
            }
            Self::ConstEvalTypeNumString => {
                "In constant expressions, operands of this operator must be of type 'num' or 'String'."
                    .to_string()
            }
            Self::ConstEvalTypeString => {
                "In constant expressions, operands of this operator must be of type 'String'."
                    .to_string()
            }
            Self::ConstEvalThrowsException => {
                "Evaluation of this constant expression throws an exception.".to_string()
            }
            Self::ConstEvalThrowsIdbze => {
                "Evaluation of this constant expression throws an IntegerDivisionByZeroException."
                    .to_string()
            }
            Self::ConstEvalPrimitiveEquality => {
                "In constant expressions, operands of the equality operator must have primitive equality."
                    .to_string()
            }
            Self::ConstEvalMethodInvocation => {
                "Methods can't be invoked in constant expressions.".to_string()
            }
            Self::RecursiveCompileTimeConstant => {
                "The compile-time constant expression depends on itself.".to_string()
            }
            // One- and two-argument rows reused through `message3`.
            Self::WrongNumberOfParametersForOperatorMinus => self.message3(a0, "", ""),
            Self::UnreachableSwitchCase | Self::UnreachableSwitchDefault => {
                self.message3("", "", "")
            }
            Self::WrongNumberOfTypeArgumentsConstructor
            | Self::WrongNumberOfTypeArgumentsEnum
            | Self::InvalidAssignment
            | Self::ArgumentTypeNotAssignable
            | Self::ReturnOfInvalidType(_)
            | Self::InvalidNullAwareOperator => self.message3(a0, a1, ""),
            // Three-argument rows route through `message3`.
            Self::WrongNumberOfParametersForOperator
            | Self::WrongNumberOfTypeArguments
            | Self::WrongNumberOfTypeArgumentsExtension
            | Self::WrongNumberOfTypeArgumentsFunction
            | Self::NonExhaustiveSwitchStatement
            | Self::NonExhaustiveSwitchExpression => self.message3(a0, a1, ""),
            // The four-placeholder `_element` row routes through `message4`
            // (the kind word leads); real emissions go through `emit4`.
            Self::WrongNumberOfTypeArgumentsElement(kind) => self.message4(kind.word(), a0, a1, ""),
        }
    }

    /// The verbatim oracle `problemMessage` for the three-placeholder
    /// templates (`{0}`, `{1}`, `{2}`); unused placeholders pass `""`.
    #[must_use]
    pub fn message3(self, a0: &str, a1: &str, a2: &str) -> String {
        match self {
            Self::WrongNumberOfParametersForOperator => {
                format!("Operator '{a0}' should declare exactly {a1} parameters, but {a2} found.")
            }
            Self::WrongNumberOfParametersForOperatorMinus => {
                format!("Operator '-' should declare 0 or 1 parameter, but {a0} found.")
            }
            Self::WrongNumberOfTypeArguments => {
                format!(
                    "The type '{a0}' is declared with {a1} type parameters, but {a2} type arguments were given."
                )
            }
            Self::WrongNumberOfTypeArgumentsConstructor => {
                format!("The constructor '{a0}.{a1}' doesn't have type parameters.")
            }
            Self::WrongNumberOfTypeArgumentsEnum => {
                format!(
                    "The enum is declared with {a0} type parameters, but {a1} type arguments were given."
                )
            }
            Self::WrongNumberOfTypeArgumentsExtension => {
                format!(
                    "The extension '{a0}' is declared with {a1} type parameters, but {a2} type arguments were given."
                )
            }
            Self::WrongNumberOfTypeArgumentsFunction => {
                format!(
                    "The type of this function is '{a0}', which has {a1} type parameters, but {a2} type arguments were given."
                )
            }
            Self::InvalidAssignment => {
                format!("A value of type '{a0}' can't be assigned to a variable of type '{a1}'.")
            }
            // The trailing space is the template's: `{2}` is the record
            // appendix, empty at M1 (`error_detection_helpers.dart`).
            Self::ArgumentTypeNotAssignable => {
                format!(
                    "The argument type '{a0}' can't be assigned to the parameter type '{a1}'. {a2}"
                )
            }
            Self::ReturnOfInvalidType(host) => {
                let host = match host {
                    ExecKind::Function => "function",
                    ExecKind::Method => "method",
                };
                format!(
                    "A value of type '{a0}' can't be returned from the {host} '{a2}' because it has a return type of '{a1}'."
                )
            }
            Self::InvalidNullAwareOperator => {
                format!(
                    "The receiver can't be null, so the null-aware operator '{a0}' is unnecessary."
                )
            }
            // The switch exhaustiveness template (diagnostic.g.dart
            // :13016-13018 / :13069-13071); `{2}` carries the correction
            // pattern (the witness `asCorrection` text).
            Self::NonExhaustiveSwitchStatement | Self::NonExhaustiveSwitchExpression => format!(
                "The type '{a0}' isn't exhaustively matched by the switch cases since it doesn't match the pattern '{a1}'."
            ),
            // No-argument row (diagnostic.g.dart:17721).
            Self::UnreachableSwitchCase => {
                "This case is covered by the previous cases.".to_string()
            }
            // No-argument row (the `unreachableSwitchDefault` row,
            // constant_verifier.dart:1141-1148).
            Self::UnreachableSwitchDefault => {
                "This default clause is covered by the previous cases.".to_string()
            }
            // One- and two-placeholder rows: the extra placeholder is
            // ignored (the descriptor stays total over the frozen set).
            _ => self.message(a0, a1),
        }
    }

    /// The verbatim oracle `correctionMessage` when the row declares one;
    /// `a2` is the suggested-pattern placeholder. Carried in the
    /// diagnostic's correction slot (`DiagnosticWithArguments`, the switch
    /// exhaustiveness rows, diagnostic.g.dart:13018/:13079). Rows without a
    /// correction answer `None`.
    #[must_use]
    pub fn correction_message(self, a2: &str) -> Option<String> {
        match self {
            Self::NonExhaustiveSwitchStatement => Some(format!(
                "Try adding a default case or cases that match '{a2}'."
            )),
            Self::NonExhaustiveSwitchExpression => Some(format!(
                "Try adding a wildcard pattern or cases that match '{a2}'."
            )),
            _ => None,
        }
    }

    /// The verbatim oracle `problemMessage` for the four-placeholder
    /// `_element` template: the kind word, the executable name, and the two
    /// counts.
    #[must_use]
    pub fn message4(self, a0: &str, a1: &str, a2: &str, a3: &str) -> String {
        match self {
            Self::WrongNumberOfTypeArgumentsElement(_) => {
                format!(
                    "The {a0} '{a1}' is declared with {a2} type parameters, but {a3} type arguments are given."
                )
            }
            // Other rows keep their own (shorter) templates.
            _ => self.message(a0, a1),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every implemented row is officially named (machine form = uppercase
    /// name), carries its oracle tier, and its descriptor round-trips
    /// through the frozen M1 gate list.
    #[test]
    fn rows_are_official_named_and_tiered() {
        let codes = [
            SemanticCode::DuplicateDefinition,
            SemanticCode::UndefinedAnnotation,
            SemanticCode::UndefinedClass,
            SemanticCode::UndefinedConstructorInInitializer,
            SemanticCode::UndefinedEnumConstructor,
            SemanticCode::UndefinedEnumConstant,
            SemanticCode::UndefinedExtensionGetter,
            SemanticCode::UndefinedExtensionMethod,
            SemanticCode::UndefinedExtensionOperator,
            SemanticCode::UndefinedExtensionSetter,
            SemanticCode::UndefinedFunction,
            SemanticCode::UndefinedGetter,
            SemanticCode::UndefinedIdentifier,
            SemanticCode::UndefinedIdentifierAwait,
            SemanticCode::UndefinedMethod,
            SemanticCode::UndefinedNamedParameter,
            SemanticCode::UndefinedOperator,
            SemanticCode::UndefinedPrefixedName,
            SemanticCode::UndefinedSetter,
            SemanticCode::UndefinedSuperMember(SuperShape::Getter),
            SemanticCode::UndefinedSuperMember(SuperShape::Method),
            SemanticCode::UndefinedSuperMember(SuperShape::Operator),
            SemanticCode::UndefinedSuperMember(SuperShape::Setter),
            SemanticCode::WrongNumberOfParametersForOperator,
            SemanticCode::WrongNumberOfParametersForOperatorMinus,
            SemanticCode::WrongNumberOfParametersForSetter,
            SemanticCode::WrongNumberOfTypeArguments,
            SemanticCode::WrongNumberOfTypeArgumentsConstructor,
            SemanticCode::WrongNumberOfTypeArgumentsEnum,
            SemanticCode::WrongNumberOfTypeArgumentsExtension,
            SemanticCode::WrongNumberOfTypeArgumentsFunction,
            SemanticCode::WrongNumberOfTypeArgumentsElement(ExecKind::Function),
            SemanticCode::WrongNumberOfTypeArgumentsElement(ExecKind::Method),
            SemanticCode::InvalidAssignment,
            SemanticCode::ArgumentTypeNotAssignable,
            SemanticCode::ReturnOfInvalidType(ExecKind::Function),
            SemanticCode::ReturnOfInvalidType(ExecKind::Method),
            SemanticCode::InvalidNullAwareOperator,
            SemanticCode::ConstInitializedWithNonConstantValue,
            SemanticCode::ConstWithNonConstantArgument,
            SemanticCode::NonConstantListElement,
            SemanticCode::NonConstantSetElement,
            SemanticCode::NonConstantMapElement,
            SemanticCode::NonConstantRecordField,
            SemanticCode::ConstEvalTypeBool,
            SemanticCode::ConstEvalTypeBoolInt,
            SemanticCode::ConstEvalTypeBoolNumString,
            SemanticCode::ConstEvalTypeInt,
            SemanticCode::ConstEvalTypeNum,
            SemanticCode::ConstEvalTypeNumString,
            SemanticCode::ConstEvalTypeString,
            SemanticCode::ConstEvalThrowsException,
            SemanticCode::ConstEvalThrowsIdbze,
            SemanticCode::ConstEvalPrimitiveEquality,
            SemanticCode::ConstEvalMethodInvocation,
            SemanticCode::RecursiveCompileTimeConstant,
        ];
        for code in codes {
            let info = code.code_info();
            assert_eq!(info.name.to_uppercase(), info.machine, "{}", info.name);
            assert!(!code.message("x", "y").is_empty(), "{}", info.name);
            assert!(!code.message3("x", "y", "z").is_empty(), "{}", info.name);
            assert!(
                !code.message4("x", "y", "z", "w").is_empty(),
                "{}",
                info.name
            );
        }
        // Tier split: two rows leave the compile-time-error tier.
        assert_eq!(
            SemanticCode::WrongNumberOfParametersForSetter
                .code_info()
                .dtype,
            DiagnosticType::SyntacticError
        );
        assert_eq!(
            SemanticCode::InvalidNullAwareOperator.code_info().dtype,
            DiagnosticType::StaticWarning
        );
        assert_eq!(
            SemanticCode::InvalidNullAwareOperator.severity(),
            Severity::Warning
        );
        assert_eq!(SemanticCode::InvalidAssignment.severity(), Severity::Error);
        // The function variant is the only unpublished wrong-number row;
        // `_element` publishes.
        assert!(
            !SemanticCode::WrongNumberOfTypeArgumentsFunction
                .code_info()
                .has_published_docs
        );
        assert!(
            SemanticCode::WrongNumberOfTypeArguments
                .code_info()
                .has_published_docs
        );
        assert!(
            SemanticCode::WrongNumberOfTypeArgumentsElement(ExecKind::Function)
                .code_info()
                .has_published_docs
        );
    }

    /// The severity derivation is the diagnostic layer's: every row's
    /// severity equals its tier's ([`DiagnosticType::severity`], the
    /// `dtype_matches_oracle_severity` invariant) — the pin against a
    /// reintroduced hand-rolled mapping.
    #[test]
    fn severity_folds_onto_the_tier_derivation() {
        let codes = [
            SemanticCode::DuplicateDefinition,
            SemanticCode::UndefinedAnnotation,
            SemanticCode::UndefinedClass,
            SemanticCode::UndefinedConstructorInInitializer,
            SemanticCode::UndefinedEnumConstructor,
            SemanticCode::UndefinedEnumConstant,
            SemanticCode::UndefinedExtensionGetter,
            SemanticCode::UndefinedExtensionMethod,
            SemanticCode::UndefinedExtensionOperator,
            SemanticCode::UndefinedExtensionSetter,
            SemanticCode::UndefinedFunction,
            SemanticCode::UndefinedGetter,
            SemanticCode::UndefinedIdentifier,
            SemanticCode::UndefinedIdentifierAwait,
            SemanticCode::UndefinedMethod,
            SemanticCode::UndefinedNamedParameter,
            SemanticCode::UndefinedOperator,
            SemanticCode::UndefinedPrefixedName,
            SemanticCode::UndefinedSetter,
            SemanticCode::UndefinedSuperMember(SuperShape::Getter),
            SemanticCode::UndefinedSuperMember(SuperShape::Method),
            SemanticCode::UndefinedSuperMember(SuperShape::Operator),
            SemanticCode::UndefinedSuperMember(SuperShape::Setter),
            SemanticCode::WrongNumberOfParametersForOperator,
            SemanticCode::WrongNumberOfParametersForOperatorMinus,
            SemanticCode::WrongNumberOfParametersForSetter,
            SemanticCode::WrongNumberOfTypeArguments,
            SemanticCode::WrongNumberOfTypeArgumentsConstructor,
            SemanticCode::WrongNumberOfTypeArgumentsEnum,
            SemanticCode::WrongNumberOfTypeArgumentsExtension,
            SemanticCode::WrongNumberOfTypeArgumentsFunction,
            SemanticCode::WrongNumberOfTypeArgumentsElement(ExecKind::Function),
            SemanticCode::WrongNumberOfTypeArgumentsElement(ExecKind::Method),
            SemanticCode::InvalidAssignment,
            SemanticCode::ArgumentTypeNotAssignable,
            SemanticCode::ReturnOfInvalidType(ExecKind::Function),
            SemanticCode::ReturnOfInvalidType(ExecKind::Method),
            SemanticCode::InvalidNullAwareOperator,
            SemanticCode::NonExhaustiveSwitchStatement,
            SemanticCode::NonExhaustiveSwitchExpression,
            SemanticCode::UnreachableSwitchCase,
            SemanticCode::UnreachableSwitchDefault,
            SemanticCode::ConstInitializedWithNonConstantValue,
            SemanticCode::ConstWithNonConstantArgument,
            SemanticCode::NonConstantListElement,
            SemanticCode::NonConstantSetElement,
            SemanticCode::NonConstantMapElement,
            SemanticCode::NonConstantRecordField,
            SemanticCode::ConstEvalTypeBool,
            SemanticCode::ConstEvalTypeBoolInt,
            SemanticCode::ConstEvalTypeBoolNumString,
            SemanticCode::ConstEvalTypeInt,
            SemanticCode::ConstEvalTypeNum,
            SemanticCode::ConstEvalTypeNumString,
            SemanticCode::ConstEvalTypeString,
            SemanticCode::ConstEvalThrowsException,
            SemanticCode::ConstEvalThrowsIdbze,
            SemanticCode::ConstEvalPrimitiveEquality,
            SemanticCode::ConstEvalMethodInvocation,
            SemanticCode::RecursiveCompileTimeConstant,
        ];
        for code in codes {
            assert_eq!(
                code.severity(),
                code.dtype().severity(),
                "{}",
                code.code_info().name
            );
        }
        // The two static warnings are the frozen set's only warnings; the
        // syntactic setter row stays an error.
        assert_eq!(
            SemanticCode::InvalidNullAwareOperator.severity(),
            Severity::Warning
        );
        assert_eq!(
            SemanticCode::UnreachableSwitchCase.severity(),
            Severity::Warning
        );
        assert_eq!(
            SemanticCode::UnreachableSwitchDefault.severity(),
            Severity::Warning
        );
        assert_eq!(
            SemanticCode::WrongNumberOfParametersForSetter.severity(),
            Severity::Error
        );
    }

    /// Verbatim oracle texts (pinned live registry `diagnostic.g.dart`),
    /// including the no-argument await and setter texts, the four
    /// `UNDEFINED_SUPER_MEMBER` shapes, and the batch-2 wrong-number and
    /// nullability templates.
    #[test]
    fn messages_are_verbatim() {
        assert_eq!(
            SemanticCode::DuplicateDefinition.message("C", ""),
            "The name 'C' is already defined."
        );
        assert_eq!(
            SemanticCode::UndefinedIdentifierAwait.message("", ""),
            "Undefined name 'await' in function body not marked with 'async'."
        );
        assert_eq!(
            SemanticCode::UndefinedPrefixedName.message("foo", "p"),
            "The name 'foo' is being referenced through the prefix 'p', but it isn't defined \
             in any of the libraries imported using that prefix."
        );
        assert_eq!(
            SemanticCode::UndefinedSuperMember(SuperShape::Operator).message("+", "C"),
            "The operator '+' isn't defined in a superclass of 'C'."
        );
        assert_eq!(
            SemanticCode::UndefinedEnumConstant.message("x", "E"),
            "There's no constant named 'x' in 'E'."
        );
        assert_eq!(
            SemanticCode::UndefinedConstructorInInitializer.message("A", "named"),
            "The class 'A' doesn't have a constructor named 'named'."
        );
        assert_eq!(
            SemanticCode::UndefinedEnumConstructor.message("other", ""),
            "The enum doesn't have a constructor named 'other'."
        );
        assert_eq!(
            SemanticCode::WrongNumberOfParametersForOperator.message3("+", "1", "2"),
            "Operator '+' should declare exactly 1 parameters, but 2 found."
        );
        assert_eq!(
            SemanticCode::WrongNumberOfParametersForOperatorMinus.message3("3", "", ""),
            "Operator '-' should declare 0 or 1 parameter, but 3 found."
        );
        assert_eq!(
            SemanticCode::WrongNumberOfParametersForSetter.message("", ""),
            "Setters must declare exactly one required positional parameter."
        );
        assert_eq!(
            SemanticCode::WrongNumberOfTypeArguments.message3("C", "2", "1"),
            "The type 'C' is declared with 2 type parameters, but 1 type arguments were given."
        );
        assert_eq!(
            SemanticCode::WrongNumberOfTypeArgumentsConstructor.message3("C", "named", ""),
            "The constructor 'C.named' doesn't have type parameters."
        );
        assert_eq!(
            SemanticCode::WrongNumberOfTypeArgumentsEnum.message3("2", "1", ""),
            "The enum is declared with 2 type parameters, but 1 type arguments were given."
        );
        assert_eq!(
            SemanticCode::WrongNumberOfTypeArgumentsExtension.message3("Ext", "1", "2"),
            "The extension 'Ext' is declared with 1 type parameters, but 2 type arguments were given."
        );
        assert_eq!(
            SemanticCode::WrongNumberOfTypeArgumentsFunction.message3(
                "void Function<T>()",
                "1",
                "2"
            ),
            "The type of this function is 'void Function<T>()', which has 1 type parameters, but 2 type arguments were given."
        );
        assert_eq!(
            SemanticCode::WrongNumberOfTypeArgumentsElement(ExecKind::Method)
                .message4("method", "m", "2", "1"),
            "The method 'm' is declared with 2 type parameters, but 1 type arguments are given."
        );
        assert_eq!(
            SemanticCode::WrongNumberOfTypeArgumentsElement(ExecKind::Function)
                .message4("function", "g", "1", "2"),
            "The function 'g' is declared with 1 type parameters, but 2 type arguments are given."
        );
        assert_eq!(
            SemanticCode::InvalidAssignment.message3("Null", "int", ""),
            "A value of type 'Null' can't be assigned to a variable of type 'int'."
        );
        assert_eq!(
            SemanticCode::ArgumentTypeNotAssignable.message3("int?", "int", ""),
            "The argument type 'int?' can't be assigned to the parameter type 'int'. "
        );
        assert_eq!(
            SemanticCode::ReturnOfInvalidType(ExecKind::Method).message3("Null", "int", "g"),
            "A value of type 'Null' can't be returned from the method 'g' because it has a return type of 'int'."
        );
        assert_eq!(
            SemanticCode::ReturnOfInvalidType(ExecKind::Function).message3("int", "void", "f"),
            "A value of type 'int' can't be returned from the function 'f' because it has a return type of 'void'."
        );
        assert_eq!(
            SemanticCode::InvalidNullAwareOperator.message("?.", "."),
            "The receiver can't be null, so the null-aware operator '?.' is unnecessary."
        );
        assert_eq!(
            SemanticCode::UnreachableSwitchDefault.message("", ""),
            "This default clause is covered by the previous cases."
        );
    }

    /// The correction-carrying rows answer the verbatim oracle
    /// `correctionMessage` templates (diagnostic.g.dart:13018/:13079); the
    /// suggested pattern fills `{2}`. Rows without a correction answer
    /// `None`.
    #[test]
    fn correction_messages_carry_the_suggested_pattern() {
        assert_eq!(
            SemanticCode::NonExhaustiveSwitchStatement.correction_message("Color.blue"),
            Some("Try adding a default case or cases that match 'Color.blue'.".to_string())
        );
        assert_eq!(
            SemanticCode::NonExhaustiveSwitchExpression.correction_message("B"),
            Some("Try adding a wildcard pattern or cases that match 'B'.".to_string())
        );
        assert_eq!(
            SemanticCode::UnreachableSwitchCase.correction_message(""),
            None,
            "the no-argument row declares no correction"
        );
        assert_eq!(
            SemanticCode::UnreachableSwitchDefault.correction_message(""),
            None,
            "the unreachable-default row keeps the case row's slot parity"
        );
    }
}
