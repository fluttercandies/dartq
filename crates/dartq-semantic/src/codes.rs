//! The M1 semantic diagnostic code table: the frozen M1 gate families the
//! semantic layer owns (`tests/harness/m1_codes.txt`), with tier and verbatim
//! oracle message per code.
//!
//! One variant per implemented frozen row. Every row was transcribed from
//! the analyzer-8.4.1 registry and re-verified verbatim against the pinned
//! live registry (`.third/dart-sdk`
//! `pkg/analyzer/lib/src/diagnostic/diagnostic.g.dart`, dart 3.13.1) — that
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
//! docs): `undefined_constructor_in_initializer` and
//! `undefined_enum_constructor` (constructors are outside the M1 model),
//! `undefined_referenced_parameter` (`@UseResult(unless:)`-specific check,
//! `annotation_verifier.dart:519-541`), `undefined_hidden_name` /
//! `undefined_shown_name` (`STATIC_WARNING` tier; the parser consumes
//! directives diet-style and resolution's [`Combinators`] exposes no clause
//! enumeration — both arrive with the directive milestone), and
//! `wrong_number_of_type_arguments_function` (the row exists above but has
//! no emit site: the M1 [`crate::model::Ty`] cannot render the function-type
//! display the template leads with — under-report). Every other frozen gate
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
    /// `Undefined name '{0}' used as an annotation.`
    UndefinedAnnotation,
    /// `Undefined class '{0}'.`
    UndefinedClass,
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
    /// unnecessary.` (`STATIC_WARNING` — the only non-error row here)
    InvalidNullAwareOperator,
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
        };
        // Tier: the setter row is a parser error in the oracle; the
        // null-aware row is the frozen set's only static warning.
        let dtype = match self {
            Self::WrongNumberOfParametersForSetter => DiagnosticType::SyntacticError,
            Self::InvalidNullAwareOperator => DiagnosticType::StaticWarning,
            _ => DiagnosticType::CompileTimeError,
        };
        // The function variant is the one wrong-number template without a
        // `hasPublishedDocs` field (diagnostic.g.dart:18725-18744).
        let published = !matches!(self, Self::WrongNumberOfTypeArgumentsFunction);
        CodeInfo::new(name, machine, dtype, published)
    }

    /// Oracle severity: every row is an error except the null-aware
    /// static warning.
    #[must_use]
    pub const fn severity(self) -> Severity {
        match self {
            Self::InvalidNullAwareOperator => Severity::Warning,
            _ => Severity::Error,
        }
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
            // One- and two-argument rows reused through `message3`.
            Self::WrongNumberOfParametersForOperatorMinus => self.message3(a0, "", ""),
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
            | Self::WrongNumberOfTypeArgumentsFunction => self.message3(a0, a1, ""),
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
            _ => unreachable!("two-placeholder rows route through `message`"),
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
            _ => unreachable!("four-placeholder rows are the `_element` family"),
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
    }
}
