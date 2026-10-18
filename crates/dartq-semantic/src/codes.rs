//! The M1 semantic diagnostic code table: the frozen M1 gate families the
//! semantic layer owns (`tests/harness/m1_codes.txt`), with tier and verbatim
//! oracle message per code.
//!
//! One variant per implemented frozen row. Every row was transcribed from
//! the analyzer-8.4.1 registry and re-verified verbatim against the pinned
//! live registry (upstream Dart SDK source,
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
//!   oracle): the trigger lives in the parser there, and dartq's parser
//!   now mirrors it at the setter name — the registry row survives for the
//!   persisted-model round-trip rendering, and the collector-side capture
//!   entry is retired (a log row would double the parse row in the merged
//!   report);
//! * `invalid_null_aware_operator` is the frozen set's only `STATIC_WARNING`
//!   (`diagnostic.g.dart`).
//!
//! The second batch's use-site split for the type-argument family follows
//! the pinned oracle registry (upstream Dart SDK source,
//! `pkg/analyzer/lib/src/diagnostic/diagnostic.g.dart`): type annotations
//! and instance-creation
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
//! function *type* — rendered from the captured member signature (the
//! diagnose layer's `function_type_display` face; the M1
//! [`crate::model::Ty`] cannot render it). The row is implemented with a
//! live emit site on the property-access tear-off shapes, pinned verbatim
//! against this template in the `diagnose` tests; the bare
//! function-reference site (`g<int>;` in statement position) stays dormant —
//! suppressed, under-report — pending the parser's statement-position
//! fallback (#351). The older 8.4.1 registry split carried a dedicated
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
//! under-reports, never false-positives; see the [`mod@crate::diagnose`] module
//! docs): `undefined_referenced_parameter` (`@UseResult(unless:)`-specific
//! check, `annotation_verifier.dart:519-541`) and `undefined_hidden_name` /
//! `undefined_shown_name` (`STATIC_WARNING` tier; the parser consumes
//! directives diet-style and resolution's [`dartq_resolution::Combinators`] exposes no clause
//! enumeration — both arrive with the directive milestone). The
//! constructor-selector rows (`undefined_constructor_in_initializer` /
//! `undefined_enum_constructor`) joined in #350 with the constructor-name
//! capture; the unnamed-selector message variants stay deferred
//! (under-report). `wrong_number_of_type_arguments_function` carries the
//! row and #350 added its emit site (the function-type display renders from
//! the captured member signature). Every other frozen gate
//! row is carried: the `wrong_number_of_type_arguments*` family (including
//! `_element`, with emit sites in [`mod@crate::diagnose`]) and
//! `wrong_number_of_parameters_for_operator` are implemented above, and
//! `uri_does_not_exist` lives in the M0 official registry
//! (`dartq-diagnostics`), emitted by the resolution layer.

use dartq_diagnostics::{CodeInfo, DiagnosticType, Severity, message_template};

/// One implemented frozen semantic code: its official descriptor and the
/// verbatim oracle message, parameterized at the emit site.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SemanticCode {
    /// `The name '{0}' is already defined.` — the scope is per-file (the
    /// capture's duplicates log): a name duplicated across two files of one
    /// library (a part-file collision) is not detected yet. The per-library
    /// fold is a documented milestone deferral — an under-report, never a
    /// false positive.
    DuplicateDefinition,
    /// `Undefined name '{0}' used as an annotation.` — the emit site is
    /// [`mod@crate::diagnose`]'s annotation check (a fully missing
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
    /// `The await expression can only be used in an async function.` — an
    /// `await e` inside a sync (non-generator, non-async) body
    /// (`awaitInWrongContext`, diagnostic.g.dart:1475-1484; the fasta
    /// `AwaitInSync` recoverable error converted at
    /// error_converter.dart — the event arrives as
    /// `end_invalid_await_expression`).
    AwaitInWrongContext,
    /// `The method '{0}' isn't defined for the type '{1}'.`
    UndefinedMethod,
    /// `The named parameter '{0}' isn't defined.`
    UndefinedNamedParameter,
    /// `The operator '{0}' isn't defined for the type '{1}'.`
    UndefinedOperator,
    /// `The name '{0}' is being referenced through the prefix '{1}', but it
    /// isn't defined in any of the libraries imported using that prefix.`
    UndefinedPrefixedName,
    /// `The prefix '{0}' can't be used here because it's shadowed by a
    /// local declaration.` — a prefixed type whose head names a local
    /// scope element (`prefixShadowedByLocalDeclaration`, the
    /// importPrefix gate's fall-through,
    /// named_type_resolver.dart:117/:188).
    PrefixShadowedByLocalDeclaration,
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
    /// itself, rendered from the captured member signature. The emit site
    /// fires on the property-access tear-off shapes (pinned in the
    /// `diagnose` tests); the bare function-reference shape stays dormant
    /// pending #351.
    WrongNumberOfTypeArgumentsFunction,
    /// `The {0} '{1}' is declared with {2} type parameters, but {3}
    /// type arguments are given.` (analyzerCode
    /// `wrong_number_of_type_arguments_element`; `{0}` is the executable's
    /// oracle display kind).
    WrongNumberOfTypeArgumentsElement(ExecKind),
    /// `'{0}' doesn't conform to the bound '{2}' of the type parameter
    /// '{1}'.` — an explicit type argument violating its parameter's
    /// substituted bound (`_checkForTypeArgumentNotMatchingBounds`,
    /// `type_arguments_verifier.dart:342-540`; the template and argument
    /// order are `typeArgumentNotMatchingBounds`, diagnostic.g.dart
    /// :16320-16330 + the `_withArguments` face :22369-22381). `{0}` is the
    /// offending argument's display, `{1}` the parameter name, `{2}` the
    /// substituted bound — under allowed super-boundedness the still-failing
    /// *inverted* arguments report (`well_bounded.dart:62-129` through the
    /// #601 `is_well_bounded` face).
    TypeArgumentNotMatchingBounds,
    /// `The result of instantiating the enum to bounds is not
    /// well-bounded.` — the enum's implicit `values` field type's single
    /// argument (the enum instantiated to bounds) is not well-bounded
    /// (`_checkForEnumInstantiatedToBoundsIsNotWellBounded`,
    /// error_verifier.dart:5060-5076; the template is
    /// `enumInstantiatedToBoundsIsNotWellBounded`, diagnostic.g.dart
    /// :4910-4919).
    EnumInstantiatedToBoundsIsNotWellBounded,
    /// `The enum must have at least one enum constant.` — a non-augmentation
    /// enum declaration whose element list is empty
    /// (`enumWithoutConstants`, diagnostic.g.dart:4966-4974; the emit site
    /// is `error_verifier.dart`'s enum-declaration check, which reports at
    /// `node.namePart.typeName` when `!isAugmentation && constants.isEmpty`).
    EnumWithoutConstants,
    /// `A value of type '{0}' can't be assigned to a variable of type '{1}'.`
    InvalidAssignment,
    /// `The argument type '{0}' can't be assigned to the parameter type
    /// '{1}'. {2}` — `{2}` is the record-shape appendix, empty at M1
    /// (records are M2), rendered as the template does.
    ArgumentTypeNotAssignable,
    /// `The type of the argument to the extension override '{0}' isn't
    /// assignable to the extended type '{1}'.` — the explicit extension
    /// override's receiver check (`extension_override_argument_not_assignable`,
    /// diagnostic.g.dart:5758-5768; emitted at the whole receiver expression,
    /// `extension_member_resolver.dart:290-313`).
    ExtensionOverrideArgumentNotAssignable,
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
    /// `The constructor being called isn't a const constructor.` — a
    /// `const`-prefixed creation whose resolved constructor declares no
    /// `const` (`error_verifier.dart` `_checkForConstWithNonConst`).
    ConstWithNonConst,
    /// `The values in a const list literal must be constants.`
    NonConstantListElement,
    /// `The values in a const set literal must be constants.`
    NonConstantSetElement,
    /// `The elements in a const map literal must be constant.`
    NonConstantMapElement,
    /// `The fields in a const record literal must be constants.`
    NonConstantRecordField,
    /// `The expression of a constant pattern must be a valid constant.` —
    /// a `ConstantPattern` node's expression that fails const-evaluation
    /// (`constantPatternWithNonConstantExpression`, diagnostic.g.dart
    /// :2528-2538; `visitConstantPattern`'s `_evaluateAndReportError`
    /// default, constant_verifier.dart:128-142).
    ConstantPatternWithNonConstantExpression,
    /// `Case expressions must be constant.` — a legacy (`< 3.0`, patterns
    /// disabled) switch-statement `case e:` whose expression fails
    /// const-evaluation (`nonConstantCaseExpression`, diagnostic.g.dart
    /// :12650-12658; `CaseExpression`'s constant requirement,
    /// constant_verifier.dart's switch-case visit).
    NonConstantCaseExpression,
    /// `Key expressions in map patterns must be constants.` — a
    /// `MapPatternEntry`'s key expression that fails const-evaluation
    /// (`nonConstantMapPatternKey`, diagnostic.g.dart:12776-12783;
    /// `visitMapPattern`'s per-entry `_evaluateAndReportError`,
    /// constant_verifier.dart:342-358).
    NonConstantMapPatternKey,
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
    /// `A nullable expression can't be used as a condition.` — a condition
    /// operand whose flow-aware static type is `bool?` (the official
    /// `checkForNonBoolExpression` fallback, upstream Dart SDK source,
    /// `pkg/analyzer/lib/src/error/bool_expression_verifier.dart:45-90`,
    /// fired per condition operand at every `&&`/`||` node and at the
    /// if/if-element/while/do-while/C-`for`/assert/conditional condition
    /// heads).
    UncheckedUseOfNullableValue,
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
    // —— #453: the inheritance-override family (all `COMPILE_TIME_ERROR`) ——
    /// `'{1}.{0}' ('{2}') isn't a valid override of '{3}.{0}' ('{4}').` — an
    /// instance method, getter, operator, or field face whose name-level
    /// signature is decuably incompatible with the same-named instance
    /// member of a direct superinterface (`inheritance_override.dart:498-546`
    /// `_checkDeclaredMember`). `{2}`/`{4}` render the name-level signature
    /// we have; the oracle's instantiated function types stay a documented
    /// text divergence.
    InvalidOverride,
    /// `The setter '{1}.{0}' ('{2}') isn't a valid override of '{3}.{0}'
    /// ('{4}').` — the same verdict for setters (`invalidOverrideSetter`,
    /// `diagnostic.g.dart:9616-9631`): one code name, dedicated text.
    InvalidOverrideSetter,
    /// `'{1}.{0}' ('{2}') isn't a valid concrete implementation of '{3}.{0}'
    /// ('{4}').` — the interface loop's concrete-implementation face
    /// (`inheritance_override.dart:412-481`): an inherited concrete member
    /// (own, mixin, or super chain) whose name-level signature is decuably
    /// incompatible with the interface map's most-specific member of the
    /// same name (`diagnostic.g.dart:9055-9084`).
    InvalidImplementationOverride,
    /// `The setter '{1}.{0}' ('{2}') isn't a valid concrete implementation of
    /// '{3}.{0}' ('{4}').` — the setter face of the same verdict; one code
    /// name, dedicated text (`invalidImplementationOverrideSetter`,
    /// `diagnostic.g.dart:9107-9130`).
    InvalidImplementationOverrideSetter,
    // —— #314: the annotation-driven override residue family ———————————————
    /// `This method overrides a method annotated as '@mustCallSuper' in
    /// '{0}', but doesn't invoke the overridden method.` — an instance
    /// member overriding a `@mustCallSuper`-annotated member found on its
    /// mixin/superclass walk without touching `super` in its body
    /// (`must_call_super_verifier.dart:19-194`;
    /// `diagnostic.g.dart:12055-12068`, `STATIC_WARNING`).
    MustCallSuper,
    /// `Missing a required override of '{0}'.` — a concrete class leaves a
    /// `@mustBeOverridden`-annotated super member un-overridden
    /// (`inheritance_override.dart:1050-1168`). One registry code, three
    /// message tiers by list length (the `sharedName` split).
    MissingOverrideOfMustBeOverriddenOne,
    /// `Missing a required override of '{0}' and '{1}'.` — the two-member
    /// tier of [`SemanticCode::MissingOverrideOfMustBeOverriddenOne`].
    MissingOverrideOfMustBeOverriddenTwo,
    /// `Missing a required override of '{0}', '{1}', and {2} more.` — the
    /// three-or-more tier.
    MissingOverrideOfMustBeOverriddenThreePlus,
    /// `The field doesn't override an inherited getter or setter.` — a
    /// `@override`-annotated field whose name level no superinterface
    /// carries (`override_verifier.dart`). One registry code
    /// (`override_on_non_overriding_member`), four message faces.
    OverrideOnNonOverridingField,
    /// `The getter doesn't override an inherited getter.` — the getter face
    /// of [`SemanticCode::OverrideOnNonOverridingField`].
    OverrideOnNonOverridingGetter,
    /// `The method doesn't override an inherited method.` — the method face
    /// (operators included — the oracle types them as methods).
    OverrideOnNonOverridingMethod,
    /// `The setter doesn't override an inherited setter.` — the setter face.
    OverrideOnNonOverridingSetter,
    // —— #449: the flow-analysis family ————————————————————————————————————
    /// `Dead code.` — a statement entered while the current control-flow
    /// path is unreachable (`diagnostic.g.dart:3542-3552`, a
    /// `STATIC_WARNING` at the dead statement's anchor; the flow facts
    /// come from [`dartq_flow`]). The oracle folds each dead region into
    /// one diagnostic spanning to the region's end
    /// (`dead_code_verifier.dart` `_firstDead`) while the fact layer
    /// reports per statement entered — a documented per-statement
    /// divergence.
    DeadCode,
    // —— #476: the extension-application family (all `COMPILE_TIME_ERROR`) ——
    /// `A member named '{0}' is defined in '{1}' and '{2}', and neither is
    /// more specific.` — two applicable extensions both declare the member
    /// and the M1 name level cannot separate them (`_isMoreSpecific`'s
    /// SDK-ness and subtype steps decide nothing for same-`on`-type
    /// candidates, `extension_member_resolver.dart:469-513`).
    AmbiguousExtensionMemberTwo,
    /// `A member named '{0}' is defined in {1}, and none are more specific.`
    /// — the same verdict for three or more candidates; `{1}` is the
    /// comma-and list of `extension 'E'` / `unnamed extension on 'T'`
    /// displays (`extension_member_resolver.dart:119-124`).
    AmbiguousExtensionMemberThreeOrMore,
    // —— #476: the extension-type representation family ————————————————————
    /// `A class member can't have the same name as the enclosing class.` —
    /// the representation field's name equals the extension type's
    /// (`error_verifier.dart:5411-5413`; `SYNTACTIC_ERROR` tier,
    /// `diagnostic.g.dart:10552-10564`).
    MemberWithClassName,
    /// `Extension types can't declare members with the same name as a member
    /// declared by 'Object'.` — the representation field's name is an
    /// `Object` member (`error_verifier.dart:5416-5423`).
    ExtensionTypeDeclaresMemberOfObject,
    /// `Expected a representation type.` — the representation formal
    /// declares no type (`error_verifier.dart:5450-5452`, the
    /// primary-constructors-disabled arm; `SYNTACTIC_ERROR` tier,
    /// `diagnostic.g.dart:5334-5345`).
    ExpectedRepresentationType,
    /// `The extension type representation can't depend on itself.` — the
    /// representation field's declared type is the extension type itself
    /// (`error_verifier.dart:5343-5354`; the M1 name-level approximation of
    /// `hasRepresentationSelfReference` — direct self references only).
    ExtensionTypeRepresentationDependsOnItself,
    /// `'{0}' is not a supertype of '{1}', the representation type.` — an
    /// extension type's `implements` face that the representation type is
    /// not a subtype of (`extensionTypeImplementsNotSupertype`,
    /// diagnostic.g.dart:5931-5939; `_verifyExtensionElementImplements`'s
    /// `isSubtypeOf(declaredRepresentation, type)` gate,
    /// scope_analyzer.dart:1309-1360).
    ExtensionTypeImplementsNotSupertype,
    // —— #498: the abstract-completeness family (all `COMPILE_TIME_ERROR`) ——
    /// `Missing concrete implementation of '{0}'.` — a concrete class or
    /// enum lacks an implementation of one inherited abstract member
    /// (`inheritance_override.dart:954` `_reportInheritedAbstractMembers`).
    /// One registry code, five message tiers by list length (the
    /// `sharedName` split, `messages.yaml`).
    InheritsAbstractMemberOne,
    /// `Missing concrete implementations of '{0}' and '{1}'.` — the
    /// two-member tier of [`SemanticCode::InheritsAbstractMemberOne`].
    InheritsAbstractMemberTwo,
    /// `Missing concrete implementations of '{0}', '{1}', and '{2}'.` — the
    /// three-member tier.
    InheritsAbstractMemberThree,
    /// `Missing concrete implementations of '{0}', '{1}', '{2}', and
    /// '{3}'.` — the four-member tier.
    InheritsAbstractMemberFour,
    /// `Missing concrete implementations of '{0}', '{1}', '{2}', '{3}', and
    /// {4} more.` — the five-or-more tier; `{4}` is the remaining count.
    InheritsAbstractMemberFivePlus,
    // —— #498: the interface-conflict family (all `COMPILE_TIME_ERROR`) ——
    /// `Superinterfaces don't have a valid override for '{0}': {1}.` — the
    /// same-name super-interface candidates share a kind but none validly
    /// overrides the others and no class-local member resolves the name
    /// (`inheritance_manager3.dart` `_combineSignaturesImpl`, reported by
    /// `inheritance_override.dart:186-204`); `{1}` is the
    /// `Class.member (signature)` list.
    InconsistentInheritance,
    /// `'{0}' is inherited as a getter (from '{1}') and also a method (from
    /// '{2}').` — a name's super-interface candidates mix getter and method
    /// kinds, which no override can resolve (`inheritance_manager3.dart`
    /// `_checkForGetterMethodConflict`). Reported even when the class
    /// declares the name (verified against the oracle: the row anchors at
    /// the local declaration there — we keep the class-name anchor, a
    /// documented span divergence).
    InconsistentInheritanceGetterAndMethod,
    // —— #504: the usage cluster (all `STATIC_WARNING`,
    //    `unused_local_elements_verifier.dart`) ————————————————————————————
    /// `The value of the local variable '{0}' isn't used.` — a local
    /// variable (pattern-declared included) no read of which resolves to it
    /// (`_visitLocalVariableElement`, `unused_local_elements_verifier.dart`:
    /// 1259-1280). Writes alone and statement-discarded reads do not count;
    /// `_`-wildcard names are exempt.
    UnusedLocalVariable,
    /// `The declaration '{0}' isn't referenced.` — a private local function,
    /// private top-level declaration, private container, or private class
    /// member with no library-level reference (`_visitLocalFunctionElement`
    /// /`_visitTopLevelFunctionElement`/`_visitClassElement`/
    /// `_visitMethodElement`/`_visitPropertyAccessorElement`/
    /// `_visitConstructorElement`, `unused_local_elements_verifier.dart`).
    UnusedElement,
    /// `The value of the field '{0}' isn't used.` — a private field no
    /// library-level read resolves to (`_visitFieldElement`,
    /// `unused_local_elements_verifier.dart:1240-1247`): writes never
    /// count, reads do.
    UnusedField,
    /// `A value for optional parameter '{0}' isn't ever given.` — an
    /// optional parameter of a non-publicly-accessible executable that no
    /// invocation supplies and no body reference reads
    /// (`visitFormalParameterList` + `_isUsedElement`'s parameter arm,
    /// unused_local_elements_verifier.dart:781-792, 1060-1125).
    UnusedElementParameter,
    // —— #504/#505: the catch-variable rows (same traversal) ————————————————
    /// `The exception variable '{0}' isn't used, so the 'catch' clause can
    /// be removed.` — an `on T catch (e)` exception variable with no stack
    /// trace sibling and no body reference (`unused_local_elements_verifier`.
    /// dart:1262-1268; a plain `catch (e)` auto-uses its variable).
    UnusedCatchClause,
    /// `The stack trace variable '{0}' isn't used and can be removed.` — a
    /// catch clause's stack trace parameter with no body reference
    /// (unused_local_elements_verifier.dart:1269-1273).
    UnusedCatchStack,
    // —— #505: the import/flow cluster (all `STATIC_WARNING`) ———————————————
    /// `Unused import: '{0}'.` — an import with a resolved, existing,
    /// non-`dart:core` target that contributed no used element
    /// (`generateUnusedImportWarnings`, imports_verifier.dart:228-268);
    /// `{0}` is the URI as written.
    UnusedImport,
    /// `The name {0} is shown, but isn't used.` — a `show`-combinator
    /// name the import resolved but never credited, on an import that at
    /// least one name did use (wholly unused imports keep their single
    /// `unused_import` row; `generateUnusedShownNameHints`,
    /// imports_verifier.dart:270-324); `{0}` is the shown name.
    UnusedShownName,
    /// `The value of '{0}' should be used.` — an invocation of a
    /// `@UseResult`/`@useResult` declaration (package:meta) whose value
    /// the enclosing expression discards (`_check` +
    /// `_isUsed`, `use_result_verifier.dart`); `{0}` is the element display
    /// name — the class name for an unnamed constructor, whose row spans
    /// the whole `A()` invocation. The annotation's reason never renders
    /// (the verifier reads constant field `message`; meta's field is
    /// named `reason`), and the `unused_result_with_message` registry
    /// sibling (same printed name, `uniqueName` split) is unreachable
    /// with package:meta — the plain variant only.
    UnusedResult,
    /// `The parameter '{0}' isn't defined by '{1}'.` — a
    /// `@UseResult.unless(parameterDefined: 'p')` annotation on a
    /// function or method whose formal parameters omit `p`
    /// (`_checkUseResult` + `_findUndefinedUseResultParameter`,
    /// annotation_verifier.dart:519-541, 671+); anchored on the written
    /// `parameterDefined` value, `{1}` the declaring callable's simple
    /// name.
    UndefinedReferencedParameter,
    /// `The left operand can't be null, so the right operand is never
    /// executed.` — an `e1 ?? e2` / `e1 ??= e2` whose left side's static
    /// type is strictly non-nullable (`_checkForDeadNullCoalesce`,
    /// error_verifier.dart:4733-4739, 1512, 1526); anchored at the right
    /// operand.
    DeadNullAwareExpression,
    /// `Dead code: Catch clauses after a 'catch (e)' or an 'on Object catch
    /// (e)' are never reached.` — a try's catch clauses following a
    /// catch-all clause (`_CatchClausesVerifier.nextCatchClause`,
    /// dead_code_verifier.dart:571-586); the span covers the first dead
    /// clause through the last one.
    DeadCodeCatchFollowingCatch,
    /// `The label '{0}' isn't used.` — a label no `break`/`continue` in its
    /// label scope targets (`_withLabelTracker`, `dead_code_verifier.dart`:
    /// 163-176).
    UnusedLabel,
    // —— #506: the provably-check cluster (all `STATIC_WARNING`) ——————————
    /// `Unnecessary cast.` — an `e as T` whose operand type already equals
    /// `T` structurally, with neither side `dynamic` nor the target a bare
    /// `Function` (`_isUnnecessaryCast`, `best_practices_verifier.dart`:
    /// 1708-1741).
    UnnecessaryCast,
    /// `The '!' will have no effect because the receiver can't be null.` —
    /// an `e!` whose operand type is strictly non-nullable
    /// (`_checkForUnnecessaryNullAware`, error_verifier.dart:7668-7718).
    UnnecessaryNonNullAssertion,
    /// `Unnecessary type check; the result is always 'true'.` — an `x is T`
    /// whose answer the static types fix at `true` (`_checkAllTypeChecks`,
    /// best_practices_verifier.dart:987-1041); one registry code across the
    /// polarity pair, the tier lives in the message shape.
    UnnecessaryTypeCheckTrue,
    /// `Unnecessary type check; the result is always 'false'.` — the `is!`
    /// polarity of [`SemanticCode::UnnecessaryTypeCheckTrue`]: the check's
    /// complement is the constant.
    UnnecessaryTypeCheckFalse,
    /// `Tests for null should be done with '== null'.` — an `x is Null`
    /// check (`_checkAllTypeChecks`'s `Null` arm,
    /// best_practices_verifier.dart:1022-1036); one registry code across
    /// the pair, unique names `type_check_is_null`/`type_check_is_not_null`.
    TypeCheckWithNull,
    /// `Tests for non-null should be done with '!= null'.` — the `is! Null`
    /// polarity of [`SemanticCode::TypeCheckWithNull`].
    TypeCheckWithNotNull,
    /// `This null-check will always throw an exception because the
    /// expression will always evaluate to 'null'.` — an `e!` whose operand
    /// type is exactly `Null` (`visitNullAssertionExpression`,
    /// best_practices_verifier.dart:795-799).
    NullCheckAlwaysFails,
    /// `The operand can't be 'null', so the condition is always 'false'.` —
    /// an `e == null` whose operand is provably never null
    /// (`_checkForInvariantNullComparison`, `best_practices_verifier.dart`:
    /// 1216-1281); one registry code across the operator pair, unique
    /// names `unnecessary_null_comparison_never_null_false/true`.
    UnnecessaryNullComparisonFalse,
    /// `The operand can't be 'null', so the condition is always 'true'.` —
    /// the `!=` polarity of [`SemanticCode::UnnecessaryNullComparisonFalse`].
    UnnecessaryNullComparisonTrue,
    // —— #507: the pattern/type-space disjointness cluster (all
    //    `STATIC_WARNING`) ————————————————————————————————————————————————
    /// `The matched value type '{0}' can never be equal to this constant of
    /// type '{1}'.` — a constant pattern whose type the matched value's
    /// type provably never equals (`_canBeEqual`, `constant_verifier.dart`:
    /// 561-596, fired from `visitConstantPattern` :140-165).
    ConstantPatternNeverMatchesValueType,
    /// `The matched value type '{0}' can never match the required type
    /// '{1}'.` — a Cast/DeclaredVariable/Object/Record/Wildcard pattern
    /// whose required type no matched value can inhabit
    /// (`checkPatternNeverMatchesValueType`, type_analyzer.dart:723-756).
    PatternNeverMatchesValueType,
    /// `Unnecessary cast pattern.` — a cast pattern whose matched type is
    /// already a subtype of the cast target
    /// (`matchedTypeIsSubtypeOfRequired`, shared_type_analyzer.dart:145-152).
    UnnecessaryCastPattern,
    /// `The null-check pattern will have no effect because the matched type
    /// isn't nullable.` — a `p?` over a non-nullable matched type
    /// (`matchedTypeIsStrictlyNonNullable`, `shared_type_analyzer.dart`:
    /// 134-139).
    UnnecessaryNullCheckPattern,
    /// `The null-assert pattern will have no effect because the matched
    /// type isn't nullable.` — the `p!` variant of
    /// [`SemanticCode::UnnecessaryNullCheckPattern`]
    /// (shared_type_analyzer.dart:129-133).
    UnnecessaryNullAssertPattern,
    // —— #480: the analyzer language-strictness family (all `STATIC_WARNING`) ——
    /// `The generic type '{0}' should have explicit type arguments but
    /// doesn't.` — the `strict-raw-types` mode's raw-type row
    /// (`type_arguments_verifier.dart:303-336` `_checkForRawTypeName`).
    /// `{0}` renders the type with its instantiated-to-bounds arguments
    /// (`Box<dynamic>` — the oracle's resolved-`type` display, the
    /// `instantiateToBounds` face), which the file's bounds capture
    /// (the `bounds` module) supplies at the fold.
    StrictRawType,
    /// `The return type of '{0}' can't be inferred.` — the `strict-inference`
    /// mode's declaration-shape row (`best_practices_verifier.dart:1587-1606`
    /// `_checkStrictInferenceReturnType`), M1-decidable for top-level
    /// functions and getters only: members stay deferred (the
    /// override-exemption face needs the full supertype walk) and typedefs
    /// and local functions follow their oracle exemptions
    /// (the local-function visit is an upstream TODO there).
    InferenceFailureOnFunctionReturnType,
    // —— #612: the variance family (the `variance` experiment only; all
    //    `COMPILE_TIME_ERROR`) ————————————————————————————————————————————
    /// `The '{0}' type parameter '{1}' can't be used in an '{2}' position.`
    /// — a member face of a class/mixin/enum whose explicitly-variated type
    /// parameter occurs in a position the keyword forbids: a field's
    /// declared type and its non-final implicit setter
    /// (`_checkForWrongTypeParameterVarianceInField`,
    /// error_verifier.dart:7910-7933), a method's formals and return type
    /// (`_checkForWrongTypeParameterVarianceInMethod`, :7936-7996), through
    /// `_checkForWrongVariancePosition` (:8074-8088). Only parameters with
    /// an explicit `in`/`out`/`inout` keyword are checked
    /// (`!typeParameter.isLegacyCovariant`), so the code is unreachable
    /// without the `variance` experiment. `{0}` is the parameter's own
    /// keyword, `{2}` the position's computed variance keyword. The
    /// method-formal and return-type faces anchor at the member name (the
    /// oracle anchors at the formal/return-type node — a span divergence,
    /// metric-neutral on the (code, file) gate); the formal `covariant`
    /// exemption and a method type-parameter-bound face are unmodeled
    /// (documented edges of this family).
    WrongTypeParameterVariancePosition,
    /// `'{0}' is an '{1}' type parameter and can't be used in an '{2}'
    /// position in '{3}'.` — the explicit-keyword face of the
    /// superinterface check
    /// (`wrongExplicitTypeParameterVarianceInSuperinterface`,
    /// error_verifier.dart:8017-8032): a superinterface of a class, mixin,
    /// enum, or extension type uses an explicitly-variated type parameter
    /// in a position its keyword forbids. `{1}` is the parameter's keyword,
    /// `{2}` the computed position keyword, `{3}` the superinterface as
    /// written; the anchor is the type parameter's name.
    WrongExplicitTypeParameterVarianceInSuperinterface,
    /// `'{0}' can't be used contravariantly or invariantly in '{1}'.` — the
    /// legacy-covariant face of the same superinterface check
    /// (`error_verifier.dart:8033-8048`), gated here to the `variance`
    /// experiment by adjudication (#612: zero new emissions on default
    /// programs — the oracle does fire it without the experiment, the
    /// under-report is the recorded gate posture).
    WrongTypeParameterVarianceInSuperinterface,
    // —— W6/#702: the package:meta `@sealed` supertype contract (all
    //    `STATIC_WARNING`) ——————————————————————————————————————————————————
    /// `The class '{0}' shouldn't be used as a mixin constraint because it
    /// is sealed, and any class mixing in this mixin must have '{0}' as a
    /// superclass.` — a mixin whose direct `on` constraint is a cross-
    /// package `@sealed` class (`_checkForInvalidSealedSuperclass`,
    /// best_practices_verifier.dart:1172-1213; the constraint arm,
    /// :1197-1204). One report per sealed supertype, at the declaring node.
    MixinOnSealedClass,
    /// `The class '{0}' shouldn't be extended, mixed in, or implemented
    /// because it's sealed.` — a class/mixin/mixin-application whose
    /// supertype closure contains a cross-package `@sealed` class through
    /// any face other than a direct mixin `on` constraint (the regular arm,
    /// best_practices_verifier.dart:1205-1212). Once per sealed supertype.
    SubtypeOfSealedClass,
    // —— #743: the W1-micro single-condition cluster (all `STATIC_WARNING`) ——
    /// `Dead code: This on-catch block won't be executed because '{0}' is a
    /// subtype of '{1}' and hence will have been caught already.` — a try's
    /// on-clause type that is a subtype of an earlier on-clause type in the
    /// same try (`_CatchClausesVerifier.nextCatchClause`,
    /// dead_code_verifier.dart:589-604); the span covers the dead clause
    /// through the try's last clause.
    DeadCodeOnCatchSubtype,
    /// `'{0}' shouldn't be called with a 'null' argument for the non-nullable
    /// type argument '{1}'.` — `Completer<T>.complete(null)` with a
    /// non-nullable `T` (`NullSafeApiVerifier._checkTypes`,
    /// null_safe_api_verifier.dart:87-107); the registry's other face,
    /// `Future<T>.value`, needs the creation-row machinery (the
    /// documented under-report).
    NullArgumentToNonNullType,
    /// `A potentially nullable type can't be used in an 'on' clause because
    /// it isn't valid to throw a nullable expression.` — an `on T?` (or
    /// `on dynamic`) catch clause (`_checkForNullableTypeInCatchClause`,
    /// `best_practices_verifier.dart`:1377-1391; `isPotentiallyNullable` =
    /// `!isNonNullable`, `type_system.dart`:1260).
    NullableTypeInCatchClause,
    /// `A double can't equal 'double.nan', so the condition is always
    /// 'false'.` — an `==` comparison against `double.nan`
    /// (`isDoubleNan`, best_practices_verifier.dart:2264-2277; the
    /// constant-pattern face is the documented under-report); one registry
    /// code across the operator pair, unique names
    /// `unnecessary_nan_comparison_false/true`.
    UnnecessaryNanComparisonFalse,
    /// `A double can't equal 'double.nan', so the condition is always
    /// 'true'.` — the `!=` polarity of
    /// [`SemanticCode::UnnecessaryNanComparisonFalse`].
    UnnecessaryNanComparisonTrue,
    /// `The '?' is unnecessary because '{0}' is nullable without it.` — a
    /// `Null?`/`dynamic?` type annotation (`visitNamedType`,
    /// best_practices_verifier.dart:762-779), anchored at the `?` token.
    UnnecessaryQuestionMark,
    /// `Unnecessary wildcard pattern.` — a wildcard pattern that is a direct
    /// logical-and operand (`unnecessaryWildcardPattern`,
    /// shared_type_analyzer.dart:250-258; the only wired
    /// `UnnecessaryWildcardKind`), reported when the wildcard always matches
    /// — unconditionally for a bare `_`, by the matched-type-subtype gate
    /// for a typed one (type_analyzer.dart:2731-2737).
    UnnecessaryWildcardPattern,
    // —— #745: the W2a warning pair (all `STATIC_WARNING`) ——————————————————
    /// `This cast always throws an exception because the expression always
    /// evaluates to 'null'.` — an `e as T` whose operand's static type is
    /// exactly `Null` and whose target is non-nullable
    /// (`visitAsExpression`, best_practices_verifier.dart:180-190), or a
    /// cast pattern over a `Null` matched value with a non-nullable target
    /// (`visitCastPattern`, :227-239). `messages.yaml` :25674
    /// (`castFromNullAlwaysFails` — no correctionMessage).
    CastFromNullAlwaysFails,
    /// `The receiver is of type 'Never', and will never complete with a
    /// value.` — a member access, invocation, or operator whose receiver
    /// (or function-position expression) has the static type `Never`
    /// (`receiverOfTypeNever`, messages.yaml :29566). The official
    /// thirteen resolver faces (`binary_expression_resolver.dart:442`,
    /// `unary_operator_invocation_resolver.dart:49`,
    /// `function_expression_invocation_resolver.dart:131`,
    /// `method_invocation_resolver.dart:662`,
    /// `increment_or_decrement_resolver.dart:273`, and the
    /// `property_element_resolver.dart` read/write faces) collapse into
    /// one receiver-row predicate at the diagnoser's junctions.
    ReceiverOfTypeNever,
    // —— #746: the W2b flow-fact consumers (all `STATIC_WARNING`) ————————————
    /// `This cast will always throw an exception because the nullable local
    /// variable '{0}' is not assigned.` — a simple local read whose declared
    /// type is spelled nullable, cast (`as`) to a non-nullable named type,
    /// where the read is definitely unassigned (`visitAsExpression`,
    /// type_analyzer.dart:2107-2126). The definite-unassignment face only
    /// the flow layer knows, so the candidate stages at the cast junction
    /// and the teardown's `UnassignedRead` join decides. `messages.yaml`
    /// :25705.
    CastFromNullableAlwaysFails,
    /// `This function has a nullable return type of '{0}', but ends without
    /// returning a value.` — a block body that can complete normally under
    /// a nullable return type whose `FutureOr` base is none of `dynamic`,
    /// invalid, unknown, `void`, or `Null` (`checkForBodyMayCompleteNormally`,
    /// type_analyzer.dart:639-704's nullable arm, reported at the block's
    /// `{`). The potentially-non-nullable ERROR twin is the never-return
    /// code's business and is never sent from this consumer.
    /// `messages.yaml` :25633.
    BodyMightCompleteNormallyNullable,
    /// `The body might complete normally, causing 'null' to be returned, but
    /// the return type, '{0}', is a potentially non-nullable type.` — the
    /// COMPILE_TIME_ERROR twin of the same walk (`isPotentiallyNonNullable`
    /// picks it over the nullable warning, type_analyzer.dart:688), reported
    /// at the block's `{`. `diagnostic.g.dart` :1571-1586.
    BodyMightCompleteNormally,
    // —— #747: the W4 const-duplicate pair (all `STATIC_WARNING`) ———————————
    /// `Two elements in a set literal shouldn't be equal.` — a non-const
    /// set literal whose later top-level element constant-evaluates to a
    /// value an earlier element already produced
    /// (`_checkForDuplications`, best_practices_verifier.dart:1077-1109,
    /// reported at the later element). Const literals early-return there —
    /// their duplicate face is the ERROR-tier
    /// `equal_elements_in_const_set`, never sent from this producer.
    /// `messages.yaml` :26904.
    EqualElementsInSet,
    /// `Two keys in a map literal shouldn't be equal.` — the same
    /// predicate over the KEY of each `MapLiteralEntry` of a non-const
    /// map literal (same verifier site, reported at the later key); map
    /// values never participate. `messages.yaml` :26951.
    EqualKeysInMap,
    // —— #748: the W5 member-invocation pair (all `STATIC_WARNING`) —————————
    /// `The parameter type of '==' operators should be non-nullable.` — an
    /// `==` operator whose single parameter's written type is `Object?`
    /// (best_practices_verifier.dart:1345-1374, reported at the `==` name
    /// token). Only `Object` and `dynamic` parameters reach the
    /// nullability question — bare `Object` and `dynamic` stay silent, as
    /// does every other type (the official double-report guard).
    /// `diagnostic.g.dart` :13182-13192.
    NonNullableEqualsParameter,
    /// `Unnecessary 'noSuchMethod' declaration.` — a `noSuchMethod`
    /// declaration whose body is exactly `super.noSuchMethod(<one
    /// argument>)` (an expression body, or a block whose single statement
    /// is that return) whose concrete super-dispatch target is a
    /// `noSuchMethod` declared by a non-`Object` class
    /// (best_practices_verifier.dart:1433-1478, reported at the
    /// declaration's name token). `diagnostic.g.dart` :17515-17524.
    UnnecessaryNoSuchMethod,
    /// `This instance creation must be 'const', because the {0}
    /// constructor is marked as '@literal'.` — a NON-const instance
    /// creation whose target constructor carries the package:meta
    /// `@literal` annotation and whose arguments are all
    /// constant-evaluatable (best_practices_verifier.dart:1276-1290,
    /// `_checkForLiteralConstructorUse`, reported at the invocation node;
    /// `{0}` is the written constructor-reference text). Unresolved
    /// targets, already-`const` invocations, non-`@literal` constructors,
    /// and non-const-evaluatable arguments all stay silent. The oracle's
    /// `new`-keyword twin (`..._using_new`) is not in the frozen M2
    /// universe, so `new`-prefixed shapes are dartq-silent by design.
    /// `diagnostic.g.dart` :12911-12922.
    NonConstCallToLiteralConstructor,
    // —— #911: the M3 primary-constructor field face (`STATIC_WARNING`) ——————
    /// `The value of the field '{0}' isn't used.` — a primary-constructor
    /// formal that declares a field (`var`/`final` keyword, no `this`/
    /// `super` selector) of a class or enum whose field no read spares
    /// (`visitPrimaryConstructorDeclaration`,
    /// unused_local_elements_verifier.dart:887-920, reported at the
    /// parameter's name token; extension-type representation formals are
    /// exempt, as is every public name — `_isReadMember`'s public-instance
    /// arm answers used). `diagnostic.g.dart` :18040-18055.
    UnusedFieldFromPrimaryConstructor,
    // —— #1285: the call-site argument-count cluster (all
    //    `COMPILE_TIME_ERROR`) ————————————————————————————————————————————
    /// `Too many positional arguments: {0} expected, but {1} found.` — a
    /// resolved callee applied more positional arguments than it declares
    /// (`{0}` = the positional capacity, required + optional;
    /// `{1}` = the positional arguments given), reported at the first
    /// surplus positional argument (`TypeAnalyzerImpl.
    /// resolveArgumentsToParameters`, upstream Dart SDK source,
    /// `pkg/analyzer/lib/src/dart/resolver/type_analyzer.dart`; registry row
    /// `extra_positional_arguments`, `diagnostic.g.dart` :6295-6310).
    /// Unresolved callees and constructor lists never reach the check.
    ExtraPositionalArguments,
    /// The same verdict when the callee declares named parameters beyond the
    /// distinct named arguments the call already gives (`namedParameterCount
    /// > namedArgumentCount` — the surplus positionals could have been
    /// passed by name). One registry name per verdict text; the
    /// `correctionMessage` splits the pair.
    /// `diagnostic.g.dart` :6312-6327.
    ExtraPositionalArgumentsCouldBeNamed,
    /// `{0} positional arguments expected, but {1} found.` — a resolved
    /// callee received fewer positional arguments than it requires, reported
    /// after the last positional argument (`_reportNotEnoughPositionalArguments`,
    /// same function; the message arity is constant there, the face here).
    /// One registry name across the four faces (the `sharedName` split,
    /// `uniqueName` `..._singular` / `..._plural` / `..._name_singular` /
    /// `..._name_plural`, `diagnostic.g.dart` :13438-13492): the face is the
    /// callee-name availability (unqualified invocations name the callee;
    /// qualified member and prefixed shapes stay nameless) crossed with the
    /// required count's plurality.
    NotEnoughPositionalArguments(ArgCountShape),
    /// `The named parameter '{0}' is required, but there's no corresponding
    /// argument.` — a resolved callee's `required` named formal received no
    /// same-named argument (`RequiredParametersVerifier._check`,
    /// required_parameters_verifier.dart:123-156), reported at the
    /// invocation's callee entity (the constructor reference / invoked
    /// name). `diagnostic.g.dart` :11147-11161.
    MissingRequiredArgument,
    // —— #1314 FAM7: the optional-parameter default-value cluster (all
    //    `COMPILE_TIME_ERROR`) ————————————————————————————————————————————
    /// `The parameter '{0}' can't have a value of 'null' because of its
    /// type, but the implicit default value is 'null'.` — an optional named
    /// parameter without a default whose written type is potentially
    /// non-nullable (`_checkUseOfDefaultValuesInParameters`,
    /// error_verifier.dart:8327-8419, the named face via
    /// `missingDefaultValueForParameter`; reported at the parameter's name
    /// token). The written-type decision mirrors the oracle's
    /// `isPotentiallyNonNullable` over the annotation the M1 model keeps:
    /// a bare written type is potentially non-nullable, `dynamic`/`void`
    /// and `?`-suffixed ones are not, and an in-scope type parameter's head
    /// suppresses (its resolved nullability rests on the bound the
    /// name-level model does not keep).
    MissingDefaultValueForParameter,
    /// The positional-optional face of the same analyzerCode
    /// (`missingDefaultValueForParameterPositional`, `sharedName`
    /// `missingDefaultValueForParameter`, messages.yaml :11434-11440): the
    /// problem text is identical, the correctionMessage drops the
    /// `'required'` arm.
    MissingDefaultValueForParameterPositional,
    /// `Using a colon as the separator before a default value is no longer
    /// supported.` — an optional named parameter's `:`-separated default
    /// value, reported at the colon (`_checkForColonDefaultValue`,
    /// best_practices_verifier.dart:1065-1082, the language-version ≥ 3.0
    /// arm; every formal-parameter shape visits it ungated by the owning
    /// executable).
    ObsoleteColonForDefaultValue,
    // —— #1314 FAM13: the type-position context faces (all
    //    `COMPILE_TIME_ERROR`) ————————————————————————————————————————————
    /// `The name '{0}' isn't a type, so it can't be used as a type
    /// argument.` — an unresolved simple name in a type-argument list
    /// reports this context face instead of `undefined_class` (the
    /// resolver's context chain, named_type_resolver.dart:720-770: catch
    /// clause, `as` operand, `is` operand, then the type-argument list —
    /// the first matching context reports and returns).
    NonTypeAsTypeArgument,
    /// `The name '{0}' isn't defined, so it can't be used in an 'is'
    /// expression.` — an unresolved simple name in an `is` operand
    /// (`type_test_with_undefined_name`, error_verifier.dart
    /// `visitIsExpression`; the metadata-path twin of the FAM13 context
    /// faces — the operand's type row reports this instead of staying
    /// silent).
    TypeTestWithUndefinedName,
    /// `The name '{0}' isn't a type, so it can't be used in an 'as'
    /// expression.` — an unresolved simple name in an `as` operand
    /// (`cast_to_non_type`, error_verifier.dart `visitAsExpression`).
    CastToNonType,
    /// `The class '{0}' can't be used as a mixin because it's neither a
    /// mixin class nor a mixin.` — a `with` entry resolving to a plain
    /// class (`class_used_as_mixin`, messages.yaml :2737-2741; the
    /// resolved-shape twin of the FAM14 `mixin_of_non_class` clause face).
    ClassUsedAsMixin,
    /// `The name '{0}' isn't a class.` — an unresolved simple name heading
    /// an instance creation reports this context face instead of
    /// `undefined_class` (`newWithNonType`/`constWithNonType`, the
    /// `creation_with_non_type` sharedName, messages.yaml :4587-4599).
    CreationWithNonType,
    /// `Couldn't infer type parameter '{0}'.{1}` — generic inference left a
    /// type parameter unsolved or solved it to a type violating the
    /// invocation's context (`couldNotInfer`, diagnostic.g.dart :3478-3485;
    /// the generic inferrer's failure report, generic_inferrer.dart). `{1}`
    /// carries the structured reason text.
    CouldNotInfer,
    /// `The name '{0}' isn't a type and can't be used in an on-catch
    /// clause.` — an unresolved catch `on`-clause exception type reports
    /// this face instead of `undefined_class` (`nonTypeInCatchClause`,
    /// messages.yaml :13748-13753; the F-P recovered-clause rows route
    /// here per the differential).
    NonTypeInCatchClause,
    // —— #1314 FAM14: the class-clause context faces (all
    //    `COMPILE_TIME_ERROR`, diagnostic.g.dart :7504/:11541/:11705/:11730)
    //    ————————————————————————————————————————————————————————————————————
    /// `Classes can only mix in mixins and classes.` — an unresolved or
    /// non-class name in a `with` clause reports this context face
    /// (scope_analyzer.dart `_resolveType`, the `WithClause` arm).
    MixinOfNonClass,
    /// `Classes and mixins can only implement other classes and mixins.`
    /// — the `implements` clause counterpart (the `ImplementsClause` arm).
    ImplementsNonClass,
    /// `Only classes and mixins can be used as superclass constraints.`
    /// — the mixin `on` clause counterpart (the `MixinOnClause` arm); the
    /// oracle keeps the resolver's `undefined_class` alongside it for an
    /// unresolved constraint name.
    MixinSuperClassConstraintNonInterface,
    /// `Mixin can only be applied to class.` — a named mixin application's
    /// superclass face (the `ClassTypeAlias` and extends-with-`with` arms).
    MixinWithNonClassSuperclass,
    /// `Classes can only extend other classes.` — the `extends` clause
    /// counterpart (the `ExtendsClause` arm of the same clause switch); the
    /// recovered extends-after-implements tail reports its unresolvable
    /// heads here.
    ExtendsNonClass,
    // —— #1314 FAM18: the uninitialized-non-nullable faces (both
    //    `COMPILE_TIME_ERROR`, diagnostic.g.dart :13503/:13540) ——————————
    /// `The non-nullable variable '{0}' must be initialized.` — a
    /// non-nullable typed, non-final, non-late, non-const variable without
    /// an initializer (top-level variables, static fields).
    NotInitializedNonNullableVariable,
    /// `Non-nullable instance field '{0}' must be initialized.` — the
    /// instance-field counterpart; the declaring container has no
    /// generative constructor (one would own the
    /// `ConstructorFieldsVerifier` face).
    NotInitializedNonNullableInstanceField,
    /// `The implicitly invoked unnamed constructor from '{0}' has required
    /// parameters.` — a generative constructor with no explicit
    /// super initializer whose superclass's unnamed constructor declares
    /// required parameters (`implicit_super_initializer_missing_arguments`,
    /// diagnostic.g.dart:9734).
    ImplicitSuperInitializerMissingArguments,
    /// `Enums can only declare final fields.` — a non-final instance field
    /// in an enum body (`non_final_field_in_enum`,
    /// diagnostic.g.dart:12821).
    NonFinalFieldInEnum,
    // —— the constructor-initializer faces (all `COMPILE_TIME_ERROR`) ————
    /// `The initializer type '{0}' can't be assigned to the field type
    /// '{1}'.` — a constructor-initializer list owns its assignability
    /// question (`field_initializer_not_assignable`,
    /// diagnostic.g.dart:6615; the const-constructor variant shares the
    /// code name with the `... in a const constructor.` message suffix and
    /// its own uniqueName — the emit site picks the constness).
    FieldInitializerNotAssignable,
    /// `'{0}' isn't a field in the enclosing class.` — a
    /// `ConstructorFieldInitializer` whose name misses the field set, or
    /// resolves to a getter/setter
    /// (`initializer_for_non_existent_field`, diagnostic.g.dart:8208;
    /// error_verifier.dart `_checkForInvalidField`).
    InitializerForNonExistentField,
    /// `'{0}' isn't a field in the enclosing class.` — an initializing
    /// formal (`this.name`) whose name is not a field of the immediately
    /// enclosing class — an inherited field does not count, a getter/
    /// setter-only name counts as missing
    /// (`initializing_formal_for_non_existent_field`,
    /// diagnostic.g.dart:8259; error_verifier.dart `_checkForValidField`).
    InitializingFormalForNonExistentField,
    /// `No associated named super constructor parameter.` — a `super.name`
    /// formal in the named section whose superclass's unnamed constructor
    /// declares no matching named parameter
    /// (`super_formal_parameter_without_associated_named`,
    /// diagnostic.g.dart:15971; error_verifier.dart
    /// `_checkForValidSuperFormalParameter`).
    SuperFormalParameterWithoutAssociatedNamed,
    /// `No associated positional super constructor parameter.` — a
    /// `super.name` formal whose superclass's unnamed constructor has no
    /// positional parameter at this formal's own positional index
    /// (`super_formal_parameter_without_associated_positional`,
    /// diagnostic.g.dart:15985; error_verifier.dart
    /// `_checkForValidSuperFormalParameter`).
    SuperFormalParameterWithoutAssociatedPositional,
    /// `Constructors can't redirect to themselves either directly or
    /// indirectly.` — a generative constructor's
    /// `RedirectingConstructorInvocation` participates in a redirect cycle
    /// (`recursive_constructor_redirect`, diagnostic.g.dart:14638;
    /// error_verifier.dart `_checkForRecursiveConstructorRedirect`).
    RecursiveConstructorRedirect,
    /// `The constant constructor depends on itself.` — a const constructor
    /// whose own formal default values const-instantiate the same
    /// constructor (`recursive_constant_constructor`, the default-value
    /// arm of the constant-dependency cycle).
    RecursiveConstantConstructor,
    // —— the final-field faces (both `COMPILE_TIME_ERROR`) ———————————————
    /// `The final variable '{0}' must be initialized.` — a non-late `final`
    /// declaration without an initializer in the declaration walk
    /// (`final_not_initialized`, diagnostic.g.dart:6856; the instance-field
    /// no-constructor arm and the static/top-level arm — the top-level
    /// spelling stays parser-owned, `final_not_initialized`'s shared CFE
    /// face).
    FinalNotInitialized,
    /// `All final variables must be initialized, but '{0}' isn't.` — the
    /// `ConstructorFieldsVerifier` face, one row per generative constructor
    /// over the sorted uninitialized-final name list
    /// (`final_not_initialized_constructor`, diagnostic.g.dart:6872; the
    /// two- and three-plus-name variants share the code name with their own
    /// message texts, picked by the name count).
    FinalNotInitializedConstructor,
    // —— the ST-sweep wave faces (all `COMPILE_TIME_ERROR`) ——————————————————
    /// `Class '{0}' can't define static member '{1}' and have instance
    /// member '{0}.{1}' with the same name.` — one row per static member
    /// whose base name collides with an instance member of the same
    /// container (`conflicting_static_and_instance`,
    /// member_duplicate_definition_verifier.dart:195-230/:430-470; emitted
    /// at the static member's name, `values` exempt).
    ConflictingStaticAndInstance,
    /// `Only static fields can be declared as const.` — an instance field
    /// group carrying `const` (`const_instance_field`,
    /// error_verifier.dart:1260-1268, one row per group at the `const`
    /// keyword).
    ConstInstanceField,
    /// `The return type of the setter must be 'void' or absent.` — a
    /// written setter return-type annotation that is not `void`
    /// (`non_void_return_for_setter`, error_verifier.dart:7015-7025, at the
    /// written annotation).
    NonVoidReturnForSetter,
    /// `'{0}' isn't a type.` — a name in type position resolving to a
    /// non-type element (`not_a_type`, named_type_resolver.dart:199/:596,
    /// the plain resolver path; local variables and local functions route
    /// to `referenced_before_declaration` instead and stay out of scope
    /// here).
    NotAType,
    /// `Conditions must have a static type of 'bool'.` — an `if`, `while`,
    /// or `for` condition whose static type is known and not `bool`
    /// (`non_bool_condition`, error_verifier.dart:5655/:5687/:5733 via
    /// `checkCollection`/`_checkForUseOfVoidResult` neighbors).
    NonBoolCondition,
    /// `A negation operand must have a static type of 'bool'.` — the `!`
    /// prefix operator's operand type is known and not `bool`
    /// (`non_bool_negation_expression`, error_verifier.dart:6374-6386,
    /// anchored from the operator's end to the operand's end).
    NonBoolNegationExpression,
    /// `An expression whose value is always 'null' can't be dereferenced.`
    /// — an invocation whose callee expression statically evaluates to
    /// `null` (`invalid_use_of_null_value`,
    /// function_expression_invocation_resolver.dart:142-144, the null
    /// receiver face of the needs-getter error).
    InvalidUseOfNullValue,
    /// `The expression doesn't evaluate to a function, so it can't be
    /// invoked.` — an invocation whose callee expression has a known
    /// non-function, non-callable static type
    /// (`invocation_of_non_function_expression`,
    /// function_expression_invocation_resolver.dart:137-140, at the callee
    /// expression).
    InvocationOfNonFunctionExpression,
    /// `Invalid reference to 'this' expression.` — `this` outside an
    /// instance member of a class-family container (`invalid_reference_to_this`,
    /// type_analyzer.dart:4608 `isThisAccessible` via
    /// element_resolver.dart:357-374).
    InvalidReferenceToThis,
    /// `Invalid context for 'super' invocation.` — `super` outside an
    /// instance context of a class-family container
    /// (`super_in_invalid_context`, element_resolver.dart:365 with the
    /// `SuperContext` classification, super_context.dart).
    SuperInInvalidContext,
    /// `The return type '{0}' of the redirected constructor isn't a subtype of
    /// '{1}'.` — a redirecting factory's target constructor whose return type
    /// is not a subtype of the factory's declared return type
    /// (`redirect_to_invalid_return_type`, diagnostic.g.dart:14931;
    /// error_verifier.dart:2986-2999 `_checkForRedirectToInvalidReturnType`).
    RedirectToInvalidReturnType,
    /// `Generative constructors can't redirect to a factory constructor.` —
    /// a generative constructor's `this(...)` initializer resolves to a
    /// factory constructor
    /// (`redirect_generative_to_non_generative_constructor`,
    /// diagnostic.g.dart:14840; error_verifier.dart:4300-4306).
    RedirectGenerativeToNonGenerativeConstructor,
    /// `The name '{0}' isn't a type and can't be used in a redirected
    /// constructor.` — a redirecting factory's `= X` target where `X` is
    /// not a class element
    /// (`redirect_to_non_class`, diagnostic.g.dart:14971;
    /// named_type_resolver.dart:587-594 `_reportRedirectToNonClass`).
    RedirectToNonClass,
    /// `A constant redirecting constructor can't redirect to a non-constant
    /// constructor.` — a `const factory` redirecting to a non-`const` target
    /// (`redirect_to_non_const_constructor`, diagnostic.g.dart:14985;
    /// error_verifier.dart:7335-7347 `_checkForRedirectToNonConstConstructor`).
    RedirectToNonConstConstructor,
    /// `The class '{0}' doesn't have an unnamed constructor.` — a `new C()`
    /// or implicit `C()` creation where `C` declares no unnamed constructor
    /// (`new_with_undefined_constructor_default`, diagnostic.g.dart; the
    /// `undefined_constructor` family folded into the unnamed-selector
    /// face).
    NewWithUndefinedConstructorDefault,
    /// `The unnamed constructor is already defined.` — two unnamed
    /// constructors in a single container
    /// (`duplicate_constructor_default`, diagnostic.g.dart:4463; the named
    /// variant `duplicate_constructor_name` shares the code with its own
    /// `The constructor with name '{0}' is already defined.` message —
    /// `message` picks by the `a0` emptiness convention).
    DuplicateConstructor,
    /// `The instance member '{0}' can't be accessed in an initializer.` —
    /// a constructor initializer referencing an instance member of the
    /// enclosing class
    /// (`implicit_this_reference_in_initializer`, diagnostic.g.dart;
    /// error_verifier.dart `_checkForImplicitThisReferenceInInitializer`).
    ImplicitThisReferenceInInitializer,
    /// `The element type '{0}' can't be assigned to the list type '{1}'.`
    /// — a list literal element whose type is not assignable to the list's
    /// element type (`list_element_type_not_assignable`,
    /// diagnostic.g.dart; error_verifier.dart list-element walk).
    ListElementTypeNotAssignable,
    /// `The late local variable '{0}' is definitely unassigned at this
    /// point.` — a `late` local variable read while flow analysis proves
    /// it definitely unassigned
    /// (`definitely_unassigned_late_local_variable`, diagnostic.g.dart;
    /// type_analyzer.dart `_checkReadOfNotAssignedLocalVariable`).
    DefinitelyUnassignedLateLocalVariable,
    /// `A value of type '{0}' can't be assigned to a parameter of type
    /// '{1}' in a const constructor.` — a constant constructor
    /// invocation's argument whose runtime value does not match the
    /// parameter (or the initializing field's) type
    /// (`const_constructor_param_type_mismatch`, diagnostic.g.dart:2619;
    /// `evaluation.dart` `_checkArgumentType` / the field-formal arm).
    ConstConstructorParamTypeMismatch,
}

/// The message face of [`SemanticCode::NotEnoughPositionalArguments`]: the
/// oracle picks the singular/plural text by the required count and the
/// name/no-name text by the invocation node's shape.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ArgCountShape {
    /// `1 positional argument expected, but 0 found.` — one required
    /// positional, nameless invocation shape.
    Singular,
    /// `{0} positional arguments expected, but {1} found.` — two or more
    /// required positionals, nameless invocation shape.
    Plural,
    /// `1 positional argument expected by '{0}', but 0 found.` — one
    /// required positional, unqualified invocation (`{0}` = the callee).
    NameSingular,
    /// `{0} positional arguments expected by '{2}', but {1} found.` — two or
    /// more required positionals, unqualified invocation.
    NamePlural,
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
    /// `constructor` — the factory/constructor return-check host
    /// (`returnOfInvalidTypeFromConstructor`, upstream Dart SDK source,
    /// pkg/analyzer/messages.yaml:16809-16819). The element face of
    /// [`SemanticCode::WrongNumberOfTypeArgumentsElement`] has no
    /// constructor row and never receives it.
    Constructor,
}

impl ExecKind {
    /// The verbatim oracle word.
    #[must_use]
    pub const fn word(self) -> &'static str {
        match self {
            Self::Function => "function",
            Self::Method => "method",
            Self::Constructor => "constructor",
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

/// Renders a registry-covered row's message from the dartq-diagnostics table
/// (the generated `message_template` match — the emission-path single
/// source). A name absent there is a static-table drift: the exhaustive
/// `registry_backed_rows_stay_in_sync_with_the_table` test walks every
/// [`SemanticCode`] variant against the registry and fails the suite loudly,
/// so the runtime path never sees a drifted name. Invariant: the caller's
/// name is registry-covered; on drift this degrades to the raw official
/// name — an under-reported message text, never a panic on the emission
/// path and never an empty or default text.
fn registry_message(name: &str) -> String {
    match message_template(name) {
        Some(template) => template.to_owned(),
        // Drift degrade: the loud failure lives in the sync test, not here.
        None => name.to_owned(),
    }
}

impl SemanticCode {
    /// The stable official descriptor: name, machine form, family type, and
    /// the published-docs flag.
    #[must_use]
    #[expect(clippy::too_many_lines)] // long diagnostic-table dispatch
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
            Self::AwaitInWrongContext => {
                ("await_in_wrong_context", "AWAIT_IN_WRONG_CONTEXT")
            }
            Self::UndefinedMethod => ("undefined_method", "UNDEFINED_METHOD"),
            Self::UndefinedNamedParameter => {
                ("undefined_named_parameter", "UNDEFINED_NAMED_PARAMETER")
            }
            Self::UndefinedOperator => ("undefined_operator", "UNDEFINED_OPERATOR"),
            Self::UndefinedPrefixedName => ("undefined_prefixed_name", "UNDEFINED_PREFIXED_NAME"),
            Self::PrefixShadowedByLocalDeclaration => (
                "prefix_shadowed_by_local_declaration",
                "PREFIX_SHADOWED_BY_LOCAL_DECLARATION",
            ),
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
            Self::TypeArgumentNotMatchingBounds => (
                "type_argument_not_matching_bounds",
                "TYPE_ARGUMENT_NOT_MATCHING_BOUNDS",
            ),
            Self::EnumInstantiatedToBoundsIsNotWellBounded => (
                "enum_instantiated_to_bounds_is_not_well_bounded",
                "ENUM_INSTANTIATED_TO_BOUNDS_IS_NOT_WELL_BOUNDED",
            ),
            Self::EnumWithoutConstants => ("enum_without_constants", "ENUM_WITHOUT_CONSTANTS"),
            Self::InvalidAssignment => ("invalid_assignment", "INVALID_ASSIGNMENT"),
            Self::ArgumentTypeNotAssignable => (
                "argument_type_not_assignable",
                "ARGUMENT_TYPE_NOT_ASSIGNABLE",
            ),
            Self::ExtensionOverrideArgumentNotAssignable => (
                "extension_override_argument_not_assignable",
                "EXTENSION_OVERRIDE_ARGUMENT_NOT_ASSIGNABLE",
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
            Self::ConstWithNonConst => ("const_with_non_const", "CONST_WITH_NON_CONST"),
            Self::NonConstantListElement => {
                ("non_constant_list_element", "NON_CONSTANT_LIST_ELEMENT")
            }
            Self::NonConstantSetElement => ("non_constant_set_element", "NON_CONSTANT_SET_ELEMENT"),
            Self::NonConstantMapElement => ("non_constant_map_element", "NON_CONSTANT_MAP_ELEMENT"),
            Self::NonConstantRecordField => {
                ("non_constant_record_field", "NON_CONSTANT_RECORD_FIELD")
            }
            Self::NonConstantCaseExpression => (
                "non_constant_case_expression",
                "NON_CONSTANT_CASE_EXPRESSION",
            ),
            Self::ConstantPatternWithNonConstantExpression => (
                "constant_pattern_with_non_constant_expression",
                "CONSTANT_PATTERN_WITH_NON_CONSTANT_EXPRESSION",
            ),
            Self::NonConstantMapPatternKey => (
                "non_constant_map_pattern_key",
                "NON_CONSTANT_MAP_PATTERN_KEY",
            ),
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
            Self::UncheckedUseOfNullableValue => (
                "unchecked_use_of_nullable_value",
                "UNCHECKED_USE_OF_NULLABLE_VALUE",
            ),
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
            // The override family shares one code name; the setter text is a
            // message-shape distinction (`invalidOverrideSetter` keeps the
            // `invalid_override` name in the registry).
            Self::InvalidOverride | Self::InvalidOverrideSetter => {
                ("invalid_override", "INVALID_OVERRIDE")
            }
            // The concrete-implementation face: one code name, two message
            // texts (the `uniqueName` split,
            // `invalid_implementation_override_setter`).
            Self::InvalidImplementationOverride | Self::InvalidImplementationOverrideSetter => (
                "invalid_implementation_override",
                "INVALID_IMPLEMENTATION_OVERRIDE",
            ),
            Self::MustCallSuper => ("must_call_super", "MUST_CALL_SUPER"),
            // One registry code across the three tiers (the `sharedName`
            // split); the tier lives in the message shape.
            Self::MissingOverrideOfMustBeOverriddenOne
            | Self::MissingOverrideOfMustBeOverriddenTwo
            | Self::MissingOverrideOfMustBeOverriddenThreePlus => (
                "missing_override_of_must_be_overridden",
                "MISSING_OVERRIDE_OF_MUST_BE_OVERRIDDEN",
            ),
            // One registry code across the four member faces (the
            // `sharedName` split); the face lives in the message shape.
            Self::OverrideOnNonOverridingField
            | Self::OverrideOnNonOverridingGetter
            | Self::OverrideOnNonOverridingMethod
            | Self::OverrideOnNonOverridingSetter => (
                "override_on_non_overriding_member",
                "OVERRIDE_ON_NON_OVERRIDING_MEMBER",
            ),
            Self::DeadCode => ("dead_code", "DEAD_CODE"),
            // One code name with two message texts (the `uniqueName` split,
            // `ambiguous_extension_member_access_two` / `_three_or_more`).
            Self::AmbiguousExtensionMemberTwo | Self::AmbiguousExtensionMemberThreeOrMore => (
                "ambiguous_extension_member_access",
                "AMBIGUOUS_EXTENSION_MEMBER_ACCESS",
            ),
            Self::MemberWithClassName => ("member_with_class_name", "MEMBER_WITH_CLASS_NAME"),
            Self::ExtensionTypeDeclaresMemberOfObject => (
                "extension_type_declares_member_of_object",
                "EXTENSION_TYPE_DECLARES_MEMBER_OF_OBJECT",
            ),
            Self::ExpectedRepresentationType => (
                "expected_representation_type",
                "EXPECTED_REPRESENTATION_TYPE",
            ),
            Self::ExtensionTypeRepresentationDependsOnItself => (
                "extension_type_representation_depends_on_itself",
                "EXTENSION_TYPE_REPRESENTATION_DEPENDS_ON_ITSELF",
            ),
            Self::ExtensionTypeImplementsNotSupertype => (
                "extension_type_implements_not_supertype",
                "EXTENSION_TYPE_IMPLEMENTS_NOT_SUPERTYPE",
            ),
            // One registry code across the five tiers (the `sharedName`
            // split); the tier lives in the message shape.
            Self::InheritsAbstractMemberOne
            | Self::InheritsAbstractMemberTwo
            | Self::InheritsAbstractMemberThree
            | Self::InheritsAbstractMemberFour
            | Self::InheritsAbstractMemberFivePlus => (
                "non_abstract_class_inherits_abstract_member",
                "NON_ABSTRACT_CLASS_INHERITS_ABSTRACT_MEMBER",
            ),
            Self::InconsistentInheritance => {
                ("inconsistent_inheritance", "INCONSISTENT_INHERITANCE")
            }
            Self::InconsistentInheritanceGetterAndMethod => (
                "inconsistent_inheritance_getter_and_method",
                "INCONSISTENT_INHERITANCE_GETTER_AND_METHOD",
            ),
            Self::UnusedLocalVariable => ("unused_local_variable", "UNUSED_LOCAL_VARIABLE"),
            Self::UnusedElement => ("unused_element", "UNUSED_ELEMENT"),
            Self::UnusedField => ("unused_field", "UNUSED_FIELD"),
            Self::UnusedElementParameter => {
                ("unused_element_parameter", "UNUSED_ELEMENT_PARAMETER")
            }
            Self::UnusedCatchClause => ("unused_catch_clause", "UNUSED_CATCH_CLAUSE"),
            Self::UnusedCatchStack => ("unused_catch_stack", "UNUSED_CATCH_STACK"),
            Self::UnusedImport => ("unused_import", "UNUSED_IMPORT"),
            Self::UnusedShownName => ("unused_shown_name", "UNUSED_SHOWN_NAME"),
            Self::UnusedResult => ("unused_result", "UNUSED_RESULT"),
            Self::UndefinedReferencedParameter => (
                "undefined_referenced_parameter",
                "UNDEFINED_REFERENCED_PARAMETER",
            ),
            Self::DeadNullAwareExpression => {
                ("dead_null_aware_expression", "DEAD_NULL_AWARE_EXPRESSION")
            }
            Self::DeadCodeCatchFollowingCatch => (
                "dead_code_catch_following_catch",
                "DEAD_CODE_CATCH_FOLLOWING_CATCH",
            ),
            Self::UnusedLabel => ("unused_label", "UNUSED_LABEL"),
            // The #506 provably-check cluster: the polarity pairs keep one
            // registry name each (the tier lives in the message shape).
            Self::UnnecessaryCast => ("unnecessary_cast", "UNNECESSARY_CAST"),
            Self::UnnecessaryNonNullAssertion => (
                "unnecessary_non_null_assertion",
                "UNNECESSARY_NON_NULL_ASSERTION",
            ),
            Self::UnnecessaryTypeCheckTrue | Self::UnnecessaryTypeCheckFalse => {
                ("unnecessary_type_check", "UNNECESSARY_TYPE_CHECK")
            }
            Self::TypeCheckWithNull | Self::TypeCheckWithNotNull => {
                ("type_check_with_null", "TYPE_CHECK_WITH_NULL")
            }
            Self::NullCheckAlwaysFails => ("null_check_always_fails", "NULL_CHECK_ALWAYS_FAILS"),
            Self::UnnecessaryNullComparisonFalse | Self::UnnecessaryNullComparisonTrue => {
                ("unnecessary_null_comparison", "UNNECESSARY_NULL_COMPARISON")
            }
            // The #507 pattern/type-space disjointness cluster.
            Self::ConstantPatternNeverMatchesValueType => (
                "constant_pattern_never_matches_value_type",
                "CONSTANT_PATTERN_NEVER_MATCHES_VALUE_TYPE",
            ),
            Self::PatternNeverMatchesValueType => (
                "pattern_never_matches_value_type",
                "PATTERN_NEVER_MATCHES_VALUE_TYPE",
            ),
            Self::UnnecessaryCastPattern => {
                ("unnecessary_cast_pattern", "UNNECESSARY_CAST_PATTERN")
            }
            Self::UnnecessaryNullCheckPattern => (
                "unnecessary_null_check_pattern",
                "UNNECESSARY_NULL_CHECK_PATTERN",
            ),
            Self::UnnecessaryNullAssertPattern => (
                "unnecessary_null_assert_pattern",
                "UNNECESSARY_NULL_ASSERT_PATTERN",
            ),
            // The #480 language-strictness rows: the `strict-raw-types` and
            // `strict-inference` modes' diagnostics, verbatim from the
            // registry (`strictRawType` :15793-15806,
            // `inferenceFailureOnFunctionReturnType` :8097-8115).
            Self::StrictRawType => ("strict_raw_type", "STRICT_RAW_TYPE"),
            Self::InferenceFailureOnFunctionReturnType => (
                "inference_failure_on_function_return_type",
                "INFERENCE_FAILURE_ON_FUNCTION_RETURN_TYPE",
            ),
            // The #612 variance family (diagnostic.g.dart:18498-18519,
            // :18771-18783, :18809-18825).
            Self::WrongTypeParameterVariancePosition => (
                "wrong_type_parameter_variance_position",
                "WRONG_TYPE_PARAMETER_VARIANCE_POSITION",
            ),
            Self::WrongExplicitTypeParameterVarianceInSuperinterface => (
                "wrong_explicit_type_parameter_variance_in_superinterface",
                "WRONG_EXPLICIT_TYPE_PARAMETER_VARIANCE_IN_SUPERINTERFACE",
            ),
            Self::WrongTypeParameterVarianceInSuperinterface => (
                "wrong_type_parameter_variance_in_superinterface",
                "WRONG_TYPE_PARAMETER_VARIANCE_IN_SUPERINTERFACE",
            ),
            // The W6 @sealed contract rows (diagnostic.g.dart:11571-11586
            // and :15857-15870).
            Self::MixinOnSealedClass => ("mixin_on_sealed_class", "MIXIN_ON_SEALED_CLASS"),
            Self::SubtypeOfSealedClass => ("subtype_of_sealed_class", "SUBTYPE_OF_SEALED_CLASS"),
            // The #743 W1-micro cluster. The NaN pair keeps one registry
            // name (sharedName `unnecessaryNanComparison`; the polarity
            // lives in the message shape), like the null-comparison pair.
            Self::DeadCodeOnCatchSubtype => {
                ("dead_code_on_catch_subtype", "DEAD_CODE_ON_CATCH_SUBTYPE")
            }
            Self::NullArgumentToNonNullType => (
                "null_argument_to_non_null_type",
                "NULL_ARGUMENT_TO_NON_NULL_TYPE",
            ),
            Self::NullableTypeInCatchClause => (
                "nullable_type_in_catch_clause",
                "NULLABLE_TYPE_IN_CATCH_CLAUSE",
            ),
            Self::UnnecessaryNanComparisonFalse | Self::UnnecessaryNanComparisonTrue => {
                ("unnecessary_nan_comparison", "UNNECESSARY_NAN_COMPARISON")
            }
            Self::UnnecessaryQuestionMark => {
                ("unnecessary_question_mark", "UNNECESSARY_QUESTION_MARK")
            }
            Self::UnnecessaryWildcardPattern => (
                "unnecessary_wildcard_pattern",
                "UNNECESSARY_WILDCARD_PATTERN",
            ),
            // The #745 W2a pair (diagnostic.g.dart:1843-1853 and
            // :14548-14559).
            Self::CastFromNullAlwaysFails => {
                ("cast_from_null_always_fails", "CAST_FROM_NULL_ALWAYS_FAILS")
            }
            Self::ReceiverOfTypeNever => ("receiver_of_type_never", "RECEIVER_OF_TYPE_NEVER"),
            // The #746 W2b pair (diagnostic.g.dart:1827-1841 and :1608-1619).
            Self::CastFromNullableAlwaysFails => (
                "cast_from_nullable_always_fails",
                "CAST_FROM_NULLABLE_ALWAYS_FAILS",
            ),
            Self::BodyMightCompleteNormallyNullable => (
                "body_might_complete_normally_nullable",
                "BODY_MIGHT_COMPLETE_NORMALLY_NULLABLE",
            ),
            Self::BodyMightCompleteNormally => (
                "body_might_complete_normally",
                "BODY_MIGHT_COMPLETE_NORMALLY",
            ),
            // The #747 W4 pair (messages.yaml :26904 and :26951 — both
            // `hasPublishedDocs: true`).
            Self::EqualElementsInSet => ("equal_elements_in_set", "EQUAL_ELEMENTS_IN_SET"),
            Self::EqualKeysInMap => ("equal_keys_in_map", "EQUAL_KEYS_IN_MAP"),
            // The #748 W5 pair (diagnostic.g.dart :13184-13191 and
            // :17517-17523 — both `hasPublishedDocs: true`).
            Self::NonNullableEqualsParameter => (
                "non_nullable_equals_parameter",
                "NON_NULLABLE_EQUALS_PARAMETER",
            ),
            Self::UnnecessaryNoSuchMethod => {
                ("unnecessary_no_such_method", "UNNECESSARY_NO_SUCH_METHOD")
            }
            // The #749 W7 face (diagnostic.g.dart:12911-12922 —
            // `hasPublishedDocs: true`).
            Self::NonConstCallToLiteralConstructor => (
                "non_const_call_to_literal_constructor",
                "NON_CONST_CALL_TO_LITERAL_CONSTRUCTOR",
            ),
            // The #911 M3 face (diagnostic.g.dart:18040-18055 —
            // `hasPublishedDocs: true`).
            Self::UnusedFieldFromPrimaryConstructor => (
                "unused_field_from_primary_constructor",
                "UNUSED_FIELD_FROM_PRIMARY_CONSTRUCTOR",
            ),
            // The #1285 argument-count cluster: the extra pair keeps one name
            // per verdict; the not-enough row is one registry name across the
            // four faces (the `sharedName` split — the face lives in the
            // message shape).
            Self::ExtraPositionalArguments => {
                ("extra_positional_arguments", "EXTRA_POSITIONAL_ARGUMENTS")
            }
            Self::ExtraPositionalArgumentsCouldBeNamed => (
                "extra_positional_arguments_could_be_named",
                "EXTRA_POSITIONAL_ARGUMENTS_COULD_BE_NAMED",
            ),
            Self::NotEnoughPositionalArguments(_) => (
                "not_enough_positional_arguments",
                "NOT_ENOUGH_POSITIONAL_ARGUMENTS",
            ),
            Self::MissingRequiredArgument => (
                "missing_required_argument",
                "MISSING_REQUIRED_ARGUMENT",
            ),
            // The #1314 FAM7 cluster: the positional face shares the named
            // one's registry name (the `sharedName` split, messages.yaml
            // :11379/:11434).
            Self::MissingDefaultValueForParameter
            | Self::MissingDefaultValueForParameterPositional => (
                "missing_default_value_for_parameter",
                "MISSING_DEFAULT_VALUE_FOR_PARAMETER",
            ),
            Self::ObsoleteColonForDefaultValue => (
                "obsolete_colon_for_default_value",
                "OBSOLETE_COLON_FOR_DEFAULT_VALUE",
            ),
            // The #1314 FAM13 type-position context faces.
            Self::NonTypeAsTypeArgument => {
                ("non_type_as_type_argument", "NON_TYPE_AS_TYPE_ARGUMENT")
            }
            Self::TypeTestWithUndefinedName => (
                "type_test_with_undefined_name",
                "TYPE_TEST_WITH_UNDEFINED_NAME",
            ),
            Self::CastToNonType => ("cast_to_non_type", "CAST_TO_NON_TYPE"),
            Self::ClassUsedAsMixin => ("class_used_as_mixin", "CLASS_USED_AS_MIXIN"),
            Self::CreationWithNonType => ("creation_with_non_type", "CREATION_WITH_NON_TYPE"),
            Self::CouldNotInfer => ("could_not_infer", "COULD_NOT_INFER"),
            Self::NonTypeInCatchClause => ("non_type_in_catch_clause", "NON_TYPE_IN_CATCH_CLAUSE"),
            // The #1314 FAM14 class-clause context faces.
            Self::MixinOfNonClass => ("mixin_of_non_class", "MIXIN_OF_NON_CLASS"),
            Self::ImplementsNonClass => ("implements_non_class", "IMPLEMENTS_NON_CLASS"),
            Self::MixinSuperClassConstraintNonInterface => (
                "mixin_super_class_constraint_non_interface",
                "MIXIN_SUPER_CLASS_CONSTRAINT_NON_INTERFACE",
            ),
            Self::MixinWithNonClassSuperclass => (
                "mixin_with_non_class_superclass",
                "MIXIN_WITH_NON_CLASS_SUPERCLASS",
            ),
            Self::ExtendsNonClass => ("extends_non_class", "EXTENDS_NON_CLASS"),
            // The #1314 FAM18 uninitialized-non-nullable faces.
            Self::NotInitializedNonNullableVariable => (
                "not_initialized_non_nullable_variable",
                "NOT_INITIALIZED_NON_NULLABLE_VARIABLE",
            ),
            Self::NotInitializedNonNullableInstanceField => (
                "not_initialized_non_nullable_instance_field",
                "NOT_INITIALIZED_NON_NULLABLE_INSTANCE_FIELD",
            ),
            Self::ImplicitSuperInitializerMissingArguments => (
                "implicit_super_initializer_missing_arguments",
                "IMPLICIT_SUPER_INITIALIZER_MISSING_ARGUMENTS",
            ),
            Self::NonFinalFieldInEnum => ("non_final_field_in_enum", "NON_FINAL_FIELD_IN_ENUM"),
            // The constructor-initializer and final-field faces.
            Self::FieldInitializerNotAssignable => (
                "field_initializer_not_assignable",
                "FIELD_INITIALIZER_NOT_ASSIGNABLE",
            ),
            Self::InitializerForNonExistentField => (
                "initializer_for_non_existent_field",
                "INITIALIZER_FOR_NON_EXISTENT_FIELD",
            ),
            Self::InitializingFormalForNonExistentField => (
                "initializing_formal_for_non_existent_field",
                "INITIALIZING_FORMAL_FOR_NON_EXISTENT_FIELD",
            ),
            Self::SuperFormalParameterWithoutAssociatedNamed => (
                "super_formal_parameter_without_associated_named",
                "SUPER_FORMAL_PARAMETER_WITHOUT_ASSOCIATED_NAMED",
            ),
            Self::SuperFormalParameterWithoutAssociatedPositional => (
                "super_formal_parameter_without_associated_positional",
                "SUPER_FORMAL_PARAMETER_WITHOUT_ASSOCIATED_POSITIONAL",
            ),
            Self::RecursiveConstructorRedirect => (
                "recursive_constructor_redirect",
                "RECURSIVE_CONSTRUCTOR_REDIRECT",
            ),
            Self::RecursiveConstantConstructor => (
                "recursive_constant_constructor",
                "RECURSIVE_CONSTANT_CONSTRUCTOR",
            ),
            Self::FinalNotInitialized => ("final_not_initialized", "FINAL_NOT_INITIALIZED"),
            Self::ConflictingStaticAndInstance => (
                "conflicting_static_and_instance",
                "CONFLICTING_STATIC_AND_INSTANCE",
            ),
            Self::ConstInstanceField => ("const_instance_field", "CONST_INSTANCE_FIELD"),
            Self::NonVoidReturnForSetter => {
                ("non_void_return_for_setter", "NON_VOID_RETURN_FOR_SETTER")
            }
            Self::NotAType => ("not_a_type", "NOT_A_TYPE"),
            Self::NonBoolCondition => ("non_bool_condition", "NON_BOOL_CONDITION"),
            Self::NonBoolNegationExpression => (
                "non_bool_negation_expression",
                "NON_BOOL_NEGATION_EXPRESSION",
            ),
            Self::InvalidUseOfNullValue => {
                ("invalid_use_of_null_value", "INVALID_USE_OF_NULL_VALUE")
            }
            Self::InvocationOfNonFunctionExpression => (
                "invocation_of_non_function_expression",
                "INVOCATION_OF_NON_FUNCTION_EXPRESSION",
            ),
            Self::InvalidReferenceToThis => {
                ("invalid_reference_to_this", "INVALID_REFERENCE_TO_THIS")
            }
            Self::SuperInInvalidContext => ("super_in_invalid_context", "SUPER_IN_INVALID_CONTEXT"),
            Self::FinalNotInitializedConstructor => (
                "final_not_initialized_constructor",
                "FINAL_NOT_INITIALIZED_CONSTRUCTOR",
            ),
            Self::RedirectToInvalidReturnType => (
                "redirect_to_invalid_return_type",
                "REDIRECT_TO_INVALID_RETURN_TYPE",
            ),
            Self::RedirectGenerativeToNonGenerativeConstructor => (
                "redirect_generative_to_non_generative_constructor",
                "REDIRECT_GENERATIVE_TO_NON_GENERATIVE_CONSTRUCTOR",
            ),
            Self::RedirectToNonClass => ("redirect_to_non_class", "REDIRECT_TO_NON_CLASS"),
            Self::RedirectToNonConstConstructor => (
                "redirect_to_non_const_constructor",
                "REDIRECT_TO_NON_CONST_CONSTRUCTOR",
            ),
            Self::NewWithUndefinedConstructorDefault => (
                "new_with_undefined_constructor_default",
                "NEW_WITH_UNDEFINED_CONSTRUCTOR_DEFAULT",
            ),
            Self::DuplicateConstructor => ("duplicate_constructor", "DUPLICATE_CONSTRUCTOR"),
            Self::ImplicitThisReferenceInInitializer => (
                "implicit_this_reference_in_initializer",
                "IMPLICIT_THIS_REFERENCE_IN_INITIALIZER",
            ),
            Self::ListElementTypeNotAssignable => (
                "list_element_type_not_assignable",
                "LIST_ELEMENT_TYPE_NOT_ASSIGNABLE",
            ),
            Self::DefinitelyUnassignedLateLocalVariable => (
                "definitely_unassigned_late_local_variable",
                "DEFINITELY_UNASSIGNED_LATE_LOCAL_VARIABLE",
            ),
            Self::ConstConstructorParamTypeMismatch => (
                "const_constructor_param_type_mismatch",
                "CONST_CONSTRUCTOR_PARAM_TYPE_MISMATCH",
            ),
        };
        let dtype = self.dtype();
        // Unpublished rows: the function variant is the only wrong-number
        // template without a `hasPublishedDocs` field
        // (diagnostic.g.dart:18725-18744); `memberWithClassName`
        // (:10552-10564) and `expectedRepresentationType` (:5334-5345) carry
        // none either.
        let published = !matches!(
            self,
            Self::WrongNumberOfTypeArgumentsFunction
                | Self::MemberWithClassName
                | Self::ExpectedRepresentationType
                // `inconsistentInheritanceGetterAndMethod` carries no
                // published docs (`messages.yaml` hasPublishedDocs: false).
                | Self::InconsistentInheritanceGetterAndMethod
                // `strictRawType` carries no `hasPublishedDocs` either
                // (diagnostic.g.dart:15793-15806 — a doc comment, not the
                // flag).
                | Self::StrictRawType
                // The #612 variance rows carry none either
                // (diagnostic.g.dart:18498-18519/:18771-18783/:18809-18825).
                | Self::WrongTypeParameterVariancePosition
                | Self::WrongExplicitTypeParameterVarianceInSuperinterface
                | Self::WrongTypeParameterVarianceInSuperinterface
                // `receiverOfTypeNever` carries none either
                // (diagnostic.g.dart:14548-14559; the W2a cast row does —
                // :1849).
                | Self::ReceiverOfTypeNever
                // `enumWithoutConstants` carries no published docs
                // (diagnostic.g.dart:4966-4974 — no `hasPublishedDocs` flag).
                | Self::EnumWithoutConstants
        );
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
            Self::WrongNumberOfParametersForSetter
            | Self::MemberWithClassName
            | Self::ExpectedRepresentationType => DiagnosticType::SyntacticError,
            Self::InvalidNullAwareOperator
            | Self::UnreachableSwitchCase
            | Self::UnreachableSwitchDefault
            | Self::DeadCode
            // The dead-coalesce row (`type: DiagnosticType.STATIC_WARNING`,
            // diagnostic.g.dart:3619-3628).
            | Self::DeadNullAwareExpression
            | Self::UnusedLocalVariable
            | Self::UnusedElement
            | Self::UnusedField
            | Self::UnusedElementParameter
            | Self::UnusedCatchClause
            | Self::UnusedCatchStack
            | Self::UnusedImport
            | Self::UnusedShownName
            | Self::UnusedResult
            | Self::UndefinedReferencedParameter
            | Self::DeadCodeCatchFollowingCatch
            | Self::UnusedLabel
            | Self::UnnecessaryCast
            | Self::UnnecessaryNonNullAssertion
            | Self::UnnecessaryTypeCheckTrue
            | Self::UnnecessaryTypeCheckFalse
            | Self::TypeCheckWithNull
            | Self::TypeCheckWithNotNull
            | Self::NullCheckAlwaysFails
            | Self::UnnecessaryNullComparisonFalse
            | Self::UnnecessaryNullComparisonTrue
            | Self::ConstantPatternNeverMatchesValueType
            | Self::PatternNeverMatchesValueType
            | Self::UnnecessaryCastPattern
            | Self::UnnecessaryNullCheckPattern
            | Self::UnnecessaryNullAssertPattern
            | Self::MustCallSuper
            | Self::MissingOverrideOfMustBeOverriddenOne
            | Self::MissingOverrideOfMustBeOverriddenTwo
            | Self::MissingOverrideOfMustBeOverriddenThreePlus
            | Self::OverrideOnNonOverridingField
            | Self::OverrideOnNonOverridingGetter
            | Self::OverrideOnNonOverridingMethod
            | Self::OverrideOnNonOverridingSetter
            // The #480 language-strictness rows are `STATIC_WARNING` tier
            // (`DiagnosticType.STATIC_WARNING`, diagnostic.g.dart:15804 and
            // :8110).
            | Self::StrictRawType
            | Self::InferenceFailureOnFunctionReturnType
            // The W6 @sealed contract rows are `STATIC_WARNING`
            // (diagnostic.g.dart:11584 and :15866).
            | Self::MixinOnSealedClass
            | Self::SubtypeOfSealedClass
            // The #743 W1-micro cluster (`messages.yaml`, the
            // StaticWarningCode block: deadCodeOnCatchSubtype :25927,
            // nullArgumentToNonNullType :29376,
            // nullableTypeInCatchClause :29334,
            // unnecessaryNanComparisonTrue/False :30910/:30872,
            // unnecessaryQuestionMark :31052, unnecessaryWildcardPattern
            // :31168 — all `type: staticWarning`).
            | Self::DeadCodeOnCatchSubtype
            | Self::NullArgumentToNonNullType
            | Self::NullableTypeInCatchClause
            | Self::UnnecessaryNanComparisonFalse
            | Self::UnnecessaryNanComparisonTrue
            | Self::UnnecessaryQuestionMark
            | Self::UnnecessaryWildcardPattern
            // The #745 W2a pair (`messages.yaml`, the StaticWarningCode
            // block: castFromNullAlwaysFails :25674, receiverOfTypeNever
            // :29566 — both `type: staticWarning`).
            | Self::CastFromNullAlwaysFails
            | Self::ReceiverOfTypeNever
            // The #746 W2b pair (`messages.yaml`, the StaticWarningCode
            // block: castFromNullableAlwaysFails :25705,
            // bodyMightCompleteNormallyNullable :25633 — both `type:
            // staticWarning`).
            | Self::CastFromNullableAlwaysFails
            | Self::BodyMightCompleteNormallyNullable
            // The #747 W4 pair (`messages.yaml`, the StaticWarningCode
            // block: equalElementsInSet :26904, equalKeysInMap :26951 —
            // both `type: staticWarning`).
            | Self::EqualElementsInSet
            | Self::EqualKeysInMap
            // The #911 M3 face (`type: DiagnosticType.STATIC_WARNING`,
            // diagnostic.g.dart:18051).
            // The #748 W5 pair (`type: DiagnosticType.STATIC_WARNING`,
            // diagnostic.g.dart :13189 and :17521).
            | Self::NonNullableEqualsParameter
            | Self::UnnecessaryNoSuchMethod
            // The #749 W7 face (`type: DiagnosticType.STATIC_WARNING`,
            // diagnostic.g.dart:12918).
            | Self::NonConstCallToLiteralConstructor
            | Self::UnusedFieldFromPrimaryConstructor => DiagnosticType::StaticWarning,
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
    #[expect(clippy::match_same_arms)] // rows follow the official code listing; bodies coincide by arity
    #[expect(clippy::too_many_lines)] // long diagnostic-table dispatch
    pub fn message(self, a0: &str, a1: &str) -> String {
        match self {
            Self::DuplicateDefinition => format!("The name '{a0}' is already defined."),
            // The three-placeholder bounds row routes through `message3`
            // (real emissions go through `emit3`).
            Self::TypeArgumentNotMatchingBounds => self.message3(a0, a1, ""),
            // The #504 usage cluster (unused_local_elements_verifier.dart)
            // and the #505 import/flow rows: one-argument messages over the
            // declaration's name; the flow rows carry none.
            Self::UnusedLocalVariable => {
                format!("The value of the local variable '{a0}' isn't used.")
            }
            Self::UnusedElement => format!("The declaration '{a0}' isn't referenced."),
            Self::UnusedField => format!("The value of the field '{a0}' isn't used."),
            Self::UnusedElementParameter => {
                format!("A value for optional parameter '{a0}' isn't ever given.")
            }
            Self::UnusedCatchClause => {
                format!(
                    "The exception variable '{a0}' isn't used, so the 'catch' clause can be removed."
                )
            }
            Self::UnusedCatchStack => {
                format!("The stack trace variable '{a0}' isn't used and can be removed.")
            }
            Self::UnusedLabel => format!("The label '{a0}' isn't used."),
            Self::UnusedFieldFromPrimaryConstructor => {
                format!("The value of the field '{a0}' isn't used.")
            }
            Self::UnusedImport => format!("Unused import: '{a0}'."),
            Self::UnusedShownName => format!("The name {a0} is shown, but isn't used."),
            Self::UnusedResult => format!("The value of '{a0}' should be used."),
            Self::UndefinedReferencedParameter => {
                format!("The parameter '{a0}' isn't defined by '{a1}'.")
            }
            Self::DeadNullAwareExpression => {
                "The left operand can't be null, so the right operand is never executed."
                    .to_string()
            }
            Self::DeadCodeCatchFollowingCatch => {
                "Dead code: Catch clauses after a 'catch (e)' or an 'on Object catch (e)' are never reached."
                    .to_string()
            }
            // The #506 provably-check cluster and the #507 disjointness
            // cluster: verbatim oracle problemMessages
            // (diagnostic.g.dart, the families cited in the variant docs).
            Self::UnnecessaryCast => "Unnecessary cast.".to_string(),
            Self::UnnecessaryNonNullAssertion => {
                "The '!' will have no effect because the receiver can't be null.".to_string()
            }
            Self::UnnecessaryTypeCheckTrue => {
                "Unnecessary type check; the result is always 'true'.".to_string()
            }
            Self::UnnecessaryTypeCheckFalse => {
                "Unnecessary type check; the result is always 'false'.".to_string()
            }
            Self::TypeCheckWithNull => {
                "Tests for null should be done with '== null'.".to_string()
            }
            Self::TypeCheckWithNotNull => {
                "Tests for non-null should be done with '!= null'.".to_string()
            }
            Self::NullCheckAlwaysFails => {
                "This null-check will always throw an exception because the expression will always evaluate to 'null'."
                    .to_string()
            }
            Self::UnnecessaryNullComparisonFalse => {
                "The operand can't be 'null', so the condition is always 'false'.".to_string()
            }
            Self::UnnecessaryNullComparisonTrue => {
                "The operand can't be 'null', so the condition is always 'true'.".to_string()
            }
            Self::ConstantPatternNeverMatchesValueType => {
                format!(
                    "The matched value type '{a0}' can never be equal to this constant of type '{a1}'."
                )
            }
            Self::PatternNeverMatchesValueType => {
                format!("The matched value type '{a0}' can never match the required type '{a1}'.")
            }
            Self::UnnecessaryCastPattern => "Unnecessary cast pattern.".to_string(),
            Self::UnnecessaryNullCheckPattern => {
                "The null-check pattern will have no effect because the matched type isn't nullable."
                    .to_string()
            }
            Self::UnnecessaryNullAssertPattern => {
                "The null-assert pattern will have no effect because the matched type isn't nullable."
                    .to_string()
            }
            // The #480 language-strictness rows, verbatim
            // (diagnostic.g.dart:15800-15801 and :8104).
            Self::StrictRawType => format!(
                "The generic type '{a0}' should have explicit type arguments but doesn't."
            ),
            Self::InferenceFailureOnFunctionReturnType => {
                format!("The return type of '{a0}' can't be inferred.")
            }
            // The #610 bounds rows: the enum face carries no arguments
            // (diagnostic.g.dart:4913-4914).
            Self::EnumInstantiatedToBoundsIsNotWellBounded => {
                "The result of instantiating the enum to bounds is not well-bounded.".to_string()
            }
            // The #314 residue family: the one-argument rows and the four
            // no-argument member faces, verbatim
            // (diagnostic.g.dart:12061/:13806-13855).
            Self::MustCallSuper => format!(
                "This method overrides a method annotated as '@mustCallSuper' in '{a0}', but doesn't invoke the overridden method."
            ),
            Self::MissingOverrideOfMustBeOverriddenOne => {
                format!("Missing a required override of '{a0}'.")
            }
            Self::OverrideOnNonOverridingField => {
                "The field doesn't override an inherited getter or setter.".to_string()
            }
            Self::OverrideOnNonOverridingGetter => {
                "The getter doesn't override an inherited getter.".to_string()
            }
            Self::OverrideOnNonOverridingMethod => {
                "The method doesn't override an inherited method.".to_string()
            }
            Self::OverrideOnNonOverridingSetter => {
                "The setter doesn't override an inherited setter.".to_string()
            }
            Self::UndefinedAnnotation => {
                format!("Undefined name '{a0}' used as an annotation.")
            }
            Self::UndefinedClass => format!("Undefined class '{a0}'."),
            Self::PrefixShadowedByLocalDeclaration => {
                format!("The prefix '{a0}' can't be used here because it's shadowed by a local declaration.")
            }
            Self::UndefinedConstructorInInitializer => {
                format!("The class '{a0}' doesn't have a constructor named '{a1}'.")
            }
            // One code name, two oracle faces (sharedName
            // `undefinedEnumConstructor`, upstream Dart SDK source,
            // pkg/analyzer/messages.yaml:18602-18698): the named row and the
            // parameter-less unnamed row. The unnamed-constructor verdict
            // The two upstream uniqueNames share this code: the constant's
            // close flush (no selector written — `a1` marks it) is the
            // unnamed face; a selector the constant wrote, synthetic or
            // resolved, is the named face (`name` holds the written
            // spelling, `''` for a parser-inserted selector).
            Self::UndefinedEnumConstructor => match a1 {
                "unnamed" => "The enum doesn't have an unnamed constructor.".to_string(),
                _ => format!("The enum doesn't have a constructor named '{a0}'."),
            },
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
            Self::AwaitInWrongContext => {
                "The await expression can only be used in an async function.".to_string()
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
            // The registry-backed row: rendered from the dartq-diagnostics
            // table (the emission-path single source); the previous local
            // literal is pinned in both crates' tests.
            Self::WrongNumberOfParametersForSetter => {
                registry_message("wrong_number_of_parameters_for_setter")
            }
            // The #279 constant-evaluation family: no-argument rows (the
            // pinned registry `diagnostic.g.dart` texts).
            Self::ConstInitializedWithNonConstantValue => {
                "Const variables must be initialized with a constant value.".to_string()
            }
            Self::ConstWithNonConstantArgument => {
                "Arguments of a constant creation must be constant expressions.".to_string()
            }
            Self::ConstWithNonConst => {
                "The constructor being called isn't a const constructor.".to_string()
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
            Self::ConstantPatternWithNonConstantExpression => {
                "The expression of a constant pattern must be a valid constant.".to_string()
            }
            Self::NonConstantCaseExpression => {
                "Case expressions must be constant.".to_string()
            }
            Self::NonConstantMapPatternKey => {
                "Key expressions in map patterns must be constants.".to_string()
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
            // The shared code carries per-site problem messages
            // (messages.yaml, the uncheckedUseOfNullableValue family): the
            // property-access face takes the member's name
            // (uncheckedUseOfNullableValueInPropertyAccess), the condition
            // face renders bare.
            Self::UncheckedUseOfNullableValue if !a0.is_empty() => {
                format!("The property '{a0}' can't be unconditionally accessed because the receiver can be 'null'.")
            }
            Self::UncheckedUseOfNullableValue => {
                "A nullable expression can't be used as a condition.".to_string()
            }
            // The no-argument enum-shape row (diagnostic.g.dart:4969).
            Self::EnumWithoutConstants => {
                "The enum must have at least one enum constant.".to_string()
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
            // The ambiguity pair (a0 = member name; a1 = one display for the
            // two-variant, the joined list for the three-or-more one).
            Self::AmbiguousExtensionMemberTwo | Self::AmbiguousExtensionMemberThreeOrMore => {
                self.message3(a0, a1, "")
            }
            // The no-argument representation rows route through `message3`.
            Self::ExtensionTypeDeclaresMemberOfObject
            | Self::ExtensionTypeRepresentationDependsOnItself => self.message3("", "", ""),
            Self::ExtensionTypeImplementsNotSupertype => {
                format!("'{a0}' is not a supertype of '{a1}', the representation type.")
            }
            // The #498 abstract-completeness tiers route to their slot
            // counts; the two- and three-member tiers and the getter/method
            // row fill their slots through `message3`.
            Self::InheritsAbstractMemberOne => {
                format!("Missing concrete implementation of '{a0}'.")
            }
            Self::InconsistentInheritance => {
                format!("Superinterfaces don't have a valid override for '{a0}': {a1}.")
            }
            Self::InheritsAbstractMemberFour => self.message4(a0, a1, "", ""),
            Self::InheritsAbstractMemberFivePlus => self.message5(a0, a1, "", "", ""),
            // Registry-backed rows, same face as the setter row above.
            Self::MemberWithClassName => registry_message("member_with_class_name"),
            Self::ExpectedRepresentationType => registry_message("expected_representation_type"),
            // No-argument flow row (diagnostic.g.dart:3546).
            Self::DeadCode => "Dead code.".to_string(),
            Self::WrongNumberOfTypeArgumentsConstructor
            | Self::WrongNumberOfTypeArgumentsEnum
            | Self::InvalidAssignment
            | Self::ArgumentTypeNotAssignable
            | Self::ExtensionOverrideArgumentNotAssignable
            | Self::ReturnOfInvalidType(_)
            | Self::InvalidNullAwareOperator => self.message3(a0, a1, ""),
            // Three-argument rows route through `message3` — including the
            // #498 two/three-tier missing-implementation shapes and the
            // getter/method conflict row.
            Self::WrongNumberOfParametersForOperator
            | Self::WrongNumberOfTypeArguments
            | Self::WrongNumberOfTypeArgumentsExtension
            | Self::WrongNumberOfTypeArgumentsFunction
            | Self::NonExhaustiveSwitchStatement
            | Self::NonExhaustiveSwitchExpression
            | Self::InheritsAbstractMemberTwo
            | Self::InheritsAbstractMemberThree
            | Self::InconsistentInheritanceGetterAndMethod => self.message3(a0, a1, ""),
            // The four-placeholder `_element` row routes through `message4`
            // (the kind word leads); real emissions go through `emit4`.
            Self::WrongNumberOfTypeArgumentsElement(kind) => self.message4(kind.word(), a0, a1, ""),
            // The five-placeholder override rows route through `message5`.
            Self::InvalidOverride
            | Self::InvalidOverrideSetter
            | Self::InvalidImplementationOverride
            | Self::InvalidImplementationOverrideSetter => {
                self.message5(a0, a1, "", "", "")
            }
            // The two/three-tier `mustBeOverridden` rows route through
            // `message3`; real emissions go through `emit3`.
            Self::MissingOverrideOfMustBeOverriddenTwo
            | Self::MissingOverrideOfMustBeOverriddenThreePlus => self.message3(a0, a1, ""),
            // The #612 legacy superinterface face: the type parameter name
            // leads, the superinterface display fills the second slot
            // (diagnostic.g.dart:18773-18774). The other two variance rows
            // route through `message3`/`message4`.
            Self::WrongTypeParameterVarianceInSuperinterface => {
                format!("'{a0}' can't be used contravariantly or invariantly in '{a1}'.")
            }
            Self::WrongTypeParameterVariancePosition => self.message3(a0, a1, ""),
            Self::WrongExplicitTypeParameterVarianceInSuperinterface => {
                self.message4(a0, a1, "", "")
            }
            // The W6 `@sealed` contract rows: the sealed class's name fills
            // `{0}` in both templates (diagnostic.g.dart:11573-11576 and
            // :15859-15861).
            Self::MixinOnSealedClass => format!(
                "The class '{a0}' shouldn't be used as a mixin constraint because it is sealed, and any class mixing in this mixin must have '{a0}' as a superclass."
            ),
            Self::SubtypeOfSealedClass => format!(
                "The class '{a0}' shouldn't be extended, mixed in, or implemented because it's sealed."
            ),
            // The #743 W1-micro cluster: verbatim oracle problemMessages
            // (messages.yaml, the registry lines cited in the variant docs).
            Self::DeadCodeOnCatchSubtype => {
                format!(
                    "Dead code: This on-catch block won't be executed because '{a0}' is a subtype of '{a1}' and hence will have been caught already."
                )
            }
            Self::NullArgumentToNonNullType => {
                format!(
                    "'{a0}' shouldn't be called with a 'null' argument for the non-nullable type argument '{a1}'."
                )
            }
            Self::NullableTypeInCatchClause => {
                "A potentially nullable type can't be used in an 'on' clause because it isn't valid to throw a nullable expression."
                    .to_string()
            }
            Self::UnnecessaryNanComparisonFalse => {
                "A double can't equal 'double.nan', so the condition is always 'false'."
                    .to_string()
            }
            Self::UnnecessaryNanComparisonTrue => {
                "A double can't equal 'double.nan', so the condition is always 'true'."
                    .to_string()
            }
            Self::UnnecessaryQuestionMark => {
                format!("The '?' is unnecessary because '{a0}' is nullable without it.")
            }
            Self::UnnecessaryWildcardPattern => "Unnecessary wildcard pattern.".to_string(),
            // The #745 W2a pair: no-argument rows, verbatim
            // (messages.yaml :25676 and :29569).
            Self::CastFromNullAlwaysFails => {
                "This cast always throws an exception because the expression always evaluates to 'null'."
                    .to_string()
            }
            Self::ReceiverOfTypeNever => {
                "The receiver is of type 'Never', and will never complete with a value."
                    .to_string()
            }
            // The #746 W2b pair: one-argument rows, verbatim
            // (messages.yaml :25707 and :25638 — `{0}` is the variable name
            // / the declared return type's display text).
            Self::CastFromNullableAlwaysFails => {
                format!("This cast will always throw an exception because the nullable local variable '{a0}' is not assigned.")
            }
            Self::BodyMightCompleteNormallyNullable => {
                format!("This function has a nullable return type of '{a0}', but ends without returning a value.")
            }
            // The ERROR twin's verbatim template (diagnostic.g.dart
            // :1576-1578 — `{0}` is the declared return type's display
            // text).
            Self::BodyMightCompleteNormally => {
                format!("The body might complete normally, causing 'null' to be returned, but the return type, '{a0}', is a potentially non-nullable type.")
            }
            // The #747 W4 pair: no-argument rows, verbatim
            // (messages.yaml :26907 and :26954).
            Self::EqualElementsInSet => {
                "Two elements in a set literal shouldn't be equal.".to_string()
            }
            Self::EqualKeysInMap => {
                "Two keys in a map literal shouldn't be equal.".to_string()
            }
            // The #748 W5 pair: no-argument rows, verbatim
            // (diagnostic.g.dart :13186 and :17518).
            Self::NonNullableEqualsParameter => {
                "The parameter type of '==' operators should be non-nullable."
                    .to_string()
            }
            Self::UnnecessaryNoSuchMethod => {
                "Unnecessary 'noSuchMethod' declaration.".to_string()
            }
            // The #749 W7 face: the 3.13.1 oracle template, probe-verified
            // (diagnostic.g.dart:12913-12915 carries the 3.15 text with the
            // constructor display name quoted — the shipped 3.13.1 wording
            // wraps the written reference in "the … constructor" instead).
            Self::NonConstCallToLiteralConstructor => format!(
                "This instance creation must be 'const', because the {a0} constructor is marked as '@literal'."
            ),
            // The #1285 argument-count rows: the extra pair shares one
            // problemMessage (the `correctionMessage` splits the verdict);
            // the not-enough faces pick their text by shape. The name-plural
            // face routes through `message3` (real emissions go through
            // `emit3`).
            Self::ExtraPositionalArguments | Self::ExtraPositionalArgumentsCouldBeNamed => {
                format!("Too many positional arguments: {a0} expected, but {a1} found.")
            }
            Self::NotEnoughPositionalArguments(ArgCountShape::Singular) => {
                "1 positional argument expected, but 0 found.".to_string()
            }
            Self::NotEnoughPositionalArguments(ArgCountShape::Plural) => {
                format!("{a0} positional arguments expected, but {a1} found.")
            }
            Self::NotEnoughPositionalArguments(ArgCountShape::NameSingular) => {
                format!("1 positional argument expected by '{a0}', but 0 found.")
            }
            Self::NotEnoughPositionalArguments(ArgCountShape::NamePlural) => {
                self.message3(a0, a1, "")
            }
            // The required-named coverage's verbatim template
            // (diagnostic.g.dart :11154-11156 — `{0}` is the parameter
            // name).
            Self::MissingRequiredArgument => {
                format!("The named parameter '{a0}' is required, but there's no corresponding argument.")
            }
            // The #1314 FAM7 rows: the default-value pair shares one problem
            // template (messages.yaml :11383/:11439 — the face lives in the
            // correctionMessage); the colon row carries no arguments.
            Self::MissingDefaultValueForParameter
            | Self::MissingDefaultValueForParameterPositional => {
                format!(
                    "The parameter '{a0}' can't have a value of 'null' because of its type, but the implicit default value is 'null'."
                )
            }
            Self::ObsoleteColonForDefaultValue => {
                "Using a colon as the separator before a default value is no longer supported."
                    .to_string()
            }
            // The #1314 FAM13 type-position context faces (messages.yaml
            // :11455-11458 region and :4592).
            Self::NonTypeAsTypeArgument => {
                format!(
                    "The name '{a0}' isn't a type, so it can't be used as a type argument."
                )
            }
            Self::TypeTestWithUndefinedName => format!(
                "The name '{a0}' isn't defined, so it can't be used in an 'is' expression."
            ),
            Self::CastToNonType => {
                format!("The name '{a0}' isn't a type, so it can't be used in an 'as' expression.")
            }
            Self::ClassUsedAsMixin => format!(
                "The class '{a0}' can't be used as a mixin because it's neither a mixin class nor a mixin."
            ),
            Self::CreationWithNonType => format!("The name '{a0}' isn't a class."),
            Self::CouldNotInfer => format!("Couldn't infer type parameter '{a0}'.{a1}"),
            Self::NonTypeInCatchClause => format!(
                "The name '{a0}' isn't a type and can't be used in an on-catch clause."
            ),
            // The #1314 FAM14 class-clause context faces (fixed text,
            // diagnostic.g.dart :7506/:11544/:11707/:11731).
            Self::MixinOfNonClass => "Classes can only mix in mixins and classes.".to_string(),
            Self::ImplementsNonClass => {
                "Classes and mixins can only implement other classes and mixins.".to_string()
            }
            Self::MixinSuperClassConstraintNonInterface => {
                "Only classes and mixins can be used as superclass constraints.".to_string()
            }
            Self::MixinWithNonClassSuperclass => "Mixin can only be applied to class.".to_string(),
            Self::ExtendsNonClass => "Classes can only extend other classes.".to_string(),
            // The #1314 FAM18 uninitialized-non-nullable faces
            // (diagnostic.g.dart :13505/:13542).
            Self::NotInitializedNonNullableVariable => {
                format!("The non-nullable variable '{a0}' must be initialized.")
            }
            Self::NotInitializedNonNullableInstanceField => {
                format!("Non-nullable instance field '{a0}' must be initialized.")
            }
            Self::ImplicitSuperInitializerMissingArguments => format!(
                "The implicitly invoked unnamed constructor from '{a0}' has required parameters."
            ),
            Self::NonFinalFieldInEnum => "Enums can only declare final fields.".to_string(),
            // The constructor-initializer and final-field faces
            // (diagnostic.g.dart :6622/:8211/:14644/:6858/:6874). The
            // final-constructor row picks its text by the name count: one
            // name (`a1` empty), two (`a1` filled), three-plus (through
            // `message3`, the third slot the remaining count).
            Self::FieldInitializerNotAssignable => {
                format!("The initializer type '{a0}' can't be assigned to the field type '{a1}'.")
            }
            Self::InitializerForNonExistentField => {
                format!("'{a0}' isn't a field in the enclosing class.")
            }
            Self::InitializingFormalForNonExistentField => {
                format!("'{a0}' isn't a field in the enclosing class.")
            }
            Self::SuperFormalParameterWithoutAssociatedNamed => {
                "No associated named super constructor parameter.".to_string()
            }
            Self::SuperFormalParameterWithoutAssociatedPositional => {
                "No associated positional super constructor parameter.".to_string()
            }
            Self::RecursiveConstructorRedirect => {
                "Constructors can't redirect to themselves either directly or indirectly."
                    .to_string()
            }
            Self::RecursiveConstantConstructor => {
                "The constant constructor depends on itself.".to_string()
            }
            Self::FinalNotInitialized => {
                format!("The final variable '{a0}' must be initialized.")
            }
            Self::FinalNotInitializedConstructor if a1.is_empty() => {
                format!("All final variables must be initialized, but '{a0}' isn't.")
            }
            Self::FinalNotInitializedConstructor => {
                format!("All final variables must be initialized, but '{a0}' and '{a1}' aren't.")
            }
            // `{0}` (the container name) appears twice in the template; the
            // two-argument face renders it directly.
            Self::ConflictingStaticAndInstance => format!(
                "Class '{a0}' can't define static member '{a1}' and have instance member \
                 '{a0}.{a1}' with the same name."
            ),
            Self::ConstInstanceField => "Only static fields can be declared as const.".to_string(),
            Self::NonVoidReturnForSetter => {
                "The return type of the setter must be 'void' or absent.".to_string()
            }
            Self::NotAType => format!("'{a0}' isn't a type."),
            Self::NonBoolCondition => "Conditions must have a static type of 'bool'.".to_string(),
            Self::NonBoolNegationExpression => {
                "A negation operand must have a static type of 'bool'.".to_string()
            }
            Self::InvalidUseOfNullValue => {
                "An expression whose value is always 'null' can't be dereferenced.".to_string()
            }
            Self::InvocationOfNonFunctionExpression => {
                "The expression doesn't evaluate to a function, so it can't be invoked."
                    .to_string()
            }
            Self::InvalidReferenceToThis => "Invalid reference to 'this' expression.".to_string(),
            Self::SuperInInvalidContext => "Invalid context for 'super' invocation.".to_string(),
            Self::RedirectToInvalidReturnType => format!(
                "The return type '{a0}' of the redirected constructor isn't a subtype of '{a1}'."
            ),
            Self::RedirectGenerativeToNonGenerativeConstructor => {
                "Generative constructors can't redirect to a factory constructor.".to_string()
            }
            Self::RedirectToNonClass => format!(
                "The name '{a0}' isn't a type and can't be used in a redirected constructor."
            ),
            Self::RedirectToNonConstConstructor => {
                "A constant redirecting constructor can't redirect to a non-constant constructor."
                    .to_string()
            }
            Self::NewWithUndefinedConstructorDefault => {
                format!("The class '{a0}' doesn't have an unnamed constructor.")
            }
            Self::DuplicateConstructor if a0.is_empty() => {
                "The unnamed constructor is already defined.".to_string()
            }
            Self::DuplicateConstructor => {
                format!("The constructor with name '{a0}' is already defined.")
            }
            Self::ImplicitThisReferenceInInitializer => {
                format!("The instance member '{a0}' can't be accessed in an initializer.")
            }
            Self::ListElementTypeNotAssignable => {
                format!("The element type '{a0}' can't be assigned to the list type '{a1}'.")
            }
            Self::DefinitelyUnassignedLateLocalVariable => format!(
                "The late local variable '{a0}' is definitely unassigned at this point."
            ),
            Self::ConstConstructorParamTypeMismatch => format!(
                "A value of type '{a0}' can't be assigned to a parameter of type '{a1}' in a const constructor."
            ),
        }
    }

    /// The verbatim oracle `problemMessage` for the three-placeholder
    /// templates (`{0}`, `{1}`, `{2}`); unused placeholders pass `""`.
    #[must_use]
    #[expect(clippy::too_many_lines)] // long diagnostic-table dispatch
    pub fn message3(self, a0: &str, a1: &str, a2: &str) -> String {
        match self {
            // The three-plus-name final-constructor variant
            // (diagnostic.g.dart:6917 — the third slot the remaining count).
            Self::FinalNotInitializedConstructor => {
                format!(
                    "All final variables must be initialized, but '{a0}', '{a1}', and {a2} others aren't."
                )
            }
            // The #612 member-face row: the parameter's own keyword leads,
            // then its name, then the position's keyword
            // (diagnostic.g.dart:18811-18812).
            Self::WrongTypeParameterVariancePosition => {
                format!("The '{a0}' type parameter '{a1}' can't be used in an '{a2}' position.")
            }
            // The #1285 name-plural face (registry template
            // `notEnoughPositionalArgumentsNamePlural`, diagnostic.g.dart
            // :13438-13444): the required count, the found count, then the
            // callee name.
            Self::NotEnoughPositionalArguments(ArgCountShape::NamePlural) => {
                format!("{a0} positional arguments expected by '{a2}', but {a1} found.")
            }
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
            // The argument leads ({0}), then the parameter ({1}) and the
            // substituted bound ({2}) — the `_withArguments` face's order
            // (diagnostic.g.dart:22369-22381).
            Self::TypeArgumentNotMatchingBounds => {
                format!("'{a0}' doesn't conform to the bound '{a2}' of the type parameter '{a1}'.")
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
            // `{0}` is the receiver type, `{1}` the extended type
            // (diagnostic.g.dart:5760-5762).
            Self::ExtensionOverrideArgumentNotAssignable => {
                format!(
                    "The type of the argument to the extension override '{a0}' isn't assignable to the extended type '{a1}'."
                )
            }
            // One template shape under sharedName `returnOfInvalidType`; the
            // host word picks the oracle face — `function`/`method`/
            // `constructor` (`returnOfInvalidTypeFromFunction`/`FromMethod`/
            // `FromConstructor`, upstream Dart SDK source,
            // pkg/analyzer/messages.yaml:16809-16867).
            Self::ReturnOfInvalidType(host) => {
                let host = host.word();
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
            // The extension-application ambiguity pair — one code name, two
            // problem texts. The two-variant's `{1}`/`{2}` are *element*
            // displays (`extension E1 on C`, the ExtensionElement toString,
            // `ambiguous_extension_member_access_test.dart:54`); the
            // three-or-more one takes the resolver's inline rendering
            // (`extension 'E1'` / `unnamed extension on 'C'`,
            // `extension_member_resolver.dart:119-124`) joined per
            // `commaSeparatedWithAnd`.
            Self::AmbiguousExtensionMemberThreeOrMore => format!(
                "A member named '{a0}' is defined in {a1}, and none are more specific."
            ),
            Self::AmbiguousExtensionMemberTwo => format!(
                "A member named '{a0}' is defined in '{a1}' and '{a2}', and neither is more specific."
            ),
            // The extension-type representation rows (the rep field's name
            // and declared type are the decidable facts).
            Self::ExtensionTypeDeclaresMemberOfObject => {
                "Extension types can't declare members with the same name as a member declared by 'Object'."
                    .to_string()
            }
            Self::ExtensionTypeRepresentationDependsOnItself => {
                "The extension type representation can't depend on itself.".to_string()
            }
            // The #498 list tiers (messages.yaml, the sharedName family):
            // comma-and lists, the five-or-more one trailing the remaining
            // count.
            Self::InheritsAbstractMemberTwo => {
                format!("Missing concrete implementations of '{a0}' and '{a1}'.")
            }
            Self::InheritsAbstractMemberThree => {
                format!("Missing concrete implementations of '{a0}', '{a1}', and '{a2}'.")
            }
            // The #314 mustBeOverridden tiers (diagnostic.g.dart:11045-11092):
            // the third slot carries the remaining-member count.
            Self::MissingOverrideOfMustBeOverriddenTwo => {
                format!("Missing a required override of '{a0}' and '{a1}'.")
            }
            Self::MissingOverrideOfMustBeOverriddenThreePlus => {
                format!("Missing a required override of '{a0}', '{a1}', and {a2} more.")
            }
            Self::InconsistentInheritanceGetterAndMethod => format!(
                "'{a0}' is inherited as a getter (from '{a1}') and also a method (from '{a2}')."
            ),
            // The #504 usage cluster (unused_local_elements_verifier.dart):
            // one-argument rows over the declaration's name.
            Self::UnusedLocalVariable => {
                format!("The value of the local variable '{a0}' isn't used.")
            }
            Self::UnusedElement => format!("The declaration '{a0}' isn't referenced."),
            Self::UnusedField => format!("The value of the field '{a0}' isn't used."),
            Self::UnusedFieldFromPrimaryConstructor => {
                format!("The value of the field '{a0}' isn't used.")
            }
            Self::UnusedElementParameter => {
                format!("A value for optional parameter '{a0}' isn't ever given.")
            }
            Self::UnusedCatchClause => {
                format!(
                    "The exception variable '{a0}' isn't used, so the 'catch' clause can be removed."
                )
            }
            Self::UnusedCatchStack => {
                format!("The stack trace variable '{a0}' isn't used and can be removed.")
            }
            Self::UnusedLabel => format!("The label '{a0}' isn't used."),
            // The #505 import/flow rows: the URI names the import; the two
            // flow rows carry no arguments.
            Self::UnusedImport => format!("Unused import: '{a0}'."),
            Self::UnusedShownName => format!("The name {a0} is shown, but isn't used."),
            Self::UnusedResult => format!("The value of '{a0}' should be used."),
            Self::UndefinedReferencedParameter => {
                format!("The parameter '{a0}' isn't defined by '{a1}'.")
            }
            Self::DeadNullAwareExpression => {
                "The left operand can't be null, so the right operand is never executed."
                    .to_string()
            }
            Self::DeadCodeCatchFollowingCatch => {
                "Dead code: Catch clauses after a 'catch (e)' or an 'on Object catch (e)' are never reached."
                    .to_string()
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
    #[expect(clippy::too_many_lines)] // long diagnostic-table dispatch
    pub fn correction_message(self, a2: &str) -> Option<String> {
        match self {
            // The W6 @sealed contract rows' verbatim oracle correctionMessages
            // (diagnostic.g.dart:11577-11579 and :15862-15864; the subtype
            // row's `{0}` is the sealed class name).
            Self::MixinOnSealedClass => Some(
                "Try composing with this class, or refer to its documentation for more \
                 information."
                    .to_string(),
            ),
            Self::SubtypeOfSealedClass => Some(format!(
                "Try composing instead of inheriting, or refer to the documentation of \
                 '{a2}' for more information."
            )),
            // The #612 variance rows' verbatim oracle correctionMessages
            // (diagnostic.g.dart:18503-18505, :18775-18778, :18813-18816).
            Self::WrongTypeParameterVariancePosition => Some(
                "Try removing the type parameter or change the explicit variance modifier \
                 declaration for the type parameter to another one of 'in', 'out', or 'inout'."
                    .to_string(),
            ),
            Self::WrongExplicitTypeParameterVarianceInSuperinterface => Some(
                "Try using 'in' type parameters in 'in' positions and 'out' type parameters \
                 in 'out' positions in the superinterface."
                    .to_string(),
            ),
            Self::WrongTypeParameterVarianceInSuperinterface => Some(
                "Try not using class type parameters in types of formal parameters of \
                 function types, nor in explicitly contravariant or invariant superinterfaces."
                    .to_string(),
            ),
            // The #610 bounds rows' verbatim oracle correctionMessages
            // (diagnostic.g.dart:16324 and :4915); the ITB face's `{2}` is
            // the substituted bound.
            Self::TypeArgumentNotMatchingBounds => {
                Some(format!("Try using a type that is or is a subclass of '{a2}'."))
            }
            Self::EnumInstantiatedToBoundsIsNotWellBounded => {
                Some("Try using different bounds for type parameters.".to_string())
            }
            Self::NonExhaustiveSwitchStatement => Some(format!(
                "Try adding a default case or cases that match '{a2}'."
            )),
            Self::NonExhaustiveSwitchExpression => Some(format!(
                "Try adding a wildcard pattern or cases that match '{a2}'."
            )),
            // The dead-code row's verbatim oracle correction
            // (diagnostic.g.dart:3547-3549).
            Self::DeadCode => Some(
                "Try removing the code, or fixing the code before it so that it can be reached."
                    .to_string(),
            ),
            // The #504/#505 rows' verbatim oracle corrections
            // (diagnostic.g.dart, the usage and import/flow families).
            Self::UnusedLocalVariable => Some("Try removing the variable or using it.".to_string()),
            Self::UnusedElement => Some(format!("Try removing the declaration of '{a2}'.")),
            Self::UnusedField => Some("Try removing the field, or using it.".to_string()),
            // The primary-constructor twin's keyword-specific text
            // (diagnostic.g.dart:18047-18049); `{1}` carries the declaring
            // keyword (`var`/`final`), passed as `a2`.
            Self::UnusedFieldFromPrimaryConstructor => Some(format!(
                "Try removing the '{a2}' keyword to avoid declaring a field, or try using the field, or removing it."
            )),
            Self::UnusedElementParameter => {
                Some("Try removing the unused parameter.".to_string())
            }
            Self::UnusedCatchClause => Some("Try removing the catch clause.".to_string()),
            Self::UnusedCatchStack => {
                Some("Try removing the stack trace variable, or using it.".to_string())
            }
            Self::UnusedImport => Some("Try removing the import directive.".to_string()),
            Self::UnusedShownName => {
                Some("Try removing the name from the list of shown members.".to_string())
            }
            // The plain-variant registry correctionMessage
            // (diagnostic.g.dart:18115-18117); the with-message sibling's
            // text is identical and unreachable with package:meta.
            Self::UnusedResult => Some(
                "Try using the result by invoking a member, passing it to a function, \
                 or returning it from this function."
                    .to_string(),
            ),
            Self::DeadNullAwareExpression => {
                Some("Try removing the operator and the right operand.".to_string())
            }
            Self::DeadCodeCatchFollowingCatch => Some(
                "Try reordering the catch clauses so that they can be reached, or removing the unreachable catch clauses."
                    .to_string(),
            ),
            Self::UnusedLabel => Some(
                "Try removing the label, or using it in either a 'break' or 'continue' statement."
                    .to_string(),
            ),
            // The #506/#507 clusters' verbatim oracle correctionMessages
            // (diagnostic.g.dart, the families cited in the variant docs);
            // `null_check_always_fails` carries none.
            Self::UnnecessaryCast => Some("Try removing the cast.".to_string()),
            Self::UnnecessaryNonNullAssertion => {
                Some("Try removing the '!' operator.".to_string())
            }
            Self::UnnecessaryTypeCheckTrue | Self::UnnecessaryTypeCheckFalse => Some(
                "Try correcting the type check, or removing the type check.".to_string(),
            ),
            Self::TypeCheckWithNull => {
                Some("Try replacing the 'is Null' check with '== null'.".to_string())
            }
            Self::TypeCheckWithNotNull => {
                Some("Try replacing the 'is! Null' check with '!= null'.".to_string())
            }
            Self::UnnecessaryNullComparisonFalse => Some(
                "Try removing the condition, an enclosing condition, or the whole conditional statement."
                    .to_string(),
            ),
            Self::UnnecessaryNullComparisonTrue => Some("Remove the condition.".to_string()),
            Self::ConstantPatternNeverMatchesValueType => Some(
                "Try a constant of the same type as the matched value type.".to_string(),
            ),
            Self::PatternNeverMatchesValueType => {
                Some("Try using a different pattern.".to_string())
            }
            Self::UnnecessaryCastPattern => Some("Try removing the cast pattern.".to_string()),
            Self::UnnecessaryNullCheckPattern => Some(
                "Try replacing the null-check pattern with its nested pattern.".to_string(),
            ),
            Self::UnnecessaryNullAssertPattern => Some(
                "Try replacing the null-assert pattern with its nested pattern.".to_string(),
            ),
            // The #480 language-strictness rows' corrections, verbatim
            // (diagnostic.g.dart:15802 and :8105); `{a2}` carries the same
            // name the problem message renders.
            Self::StrictRawType => Some(format!("Use explicit type arguments for '{a2}'.")),
            Self::InferenceFailureOnFunctionReturnType => {
                Some(format!("Declare the return type of '{a2}'."))
            }
            // The #314 residue family's corrections (diagnostic.g.dart
            // :11045-11092/:13806-13855); `mustCallSuper` and the
            // concrete-implementation pair carry none.
            Self::MissingOverrideOfMustBeOverriddenOne => {
                Some("Try overriding the missing member.".to_string())
            }
            Self::MissingOverrideOfMustBeOverriddenTwo
            | Self::MissingOverrideOfMustBeOverriddenThreePlus => {
                Some("Try overriding the missing members.".to_string())
            }
            Self::OverrideOnNonOverridingField
            | Self::OverrideOnNonOverridingGetter
            | Self::OverrideOnNonOverridingMethod
            | Self::OverrideOnNonOverridingSetter => Some(
                "Try updating this class to match the superclass, or removing the override annotation."
                    .to_string(),
            ),
            // The #743 W1-micro cluster's verbatim oracle correctionMessages;
            // `unnecessary_question_mark` carries none (messages.yaml).
            Self::DeadCodeOnCatchSubtype => Some(
                "Try reordering the catch clauses so that this block can be reached, or removing the unreachable catch clause."
                    .to_string(),
            ),
            Self::NullArgumentToNonNullType => {
                Some("Try adding a non-null argument.".to_string())
            }
            Self::NullableTypeInCatchClause => Some("Try using a non-nullable type.".to_string()),
            Self::UnnecessaryNanComparisonFalse | Self::UnnecessaryNanComparisonTrue => Some(
                "Try using 'double.isNan', or removing the condition.".to_string(),
            ),
            Self::UnnecessaryWildcardPattern => {
                Some("Try removing the wildcard pattern.".to_string())
            }
            // The #745 W2a pair: the receiver row carries the registry's
            // verbatim correctionMessage (messages.yaml :29570 — no trailing
            // period in the template); the cast row declares none
            // (:25674-25679).
            Self::ReceiverOfTypeNever => Some(
                "Try checking for throw expressions or type errors in the receiver".to_string(),
            ),
            // The #746 W2b pair's verbatim oracle correctionMessages
            // (messages.yaml :25710 and :25640).
            Self::CastFromNullableAlwaysFails => Some(
                "Try giving it an initializer expression, or ensure that it's assigned on every execution path."
                    .to_string(),
            ),
            Self::BodyMightCompleteNormallyNullable => Some(
                "Try adding a return statement, or if no value is ever returned, try changing the return type to 'void'."
                    .to_string(),
            ),
            // The ERROR twin's verbatim correctionMessage
            // (diagnostic.g.dart :1579).
            Self::BodyMightCompleteNormally => {
                Some("Try adding either a return or a throw statement at the end.".to_string())
            }
            // The #747 W4 pair's verbatim oracle correctionMessages
            // (messages.yaml :26908 and :26955).
            Self::EqualElementsInSet => {
                Some("Change or remove the duplicate element.".to_string())
            }
            Self::EqualKeysInMap => Some("Change or remove the duplicate key.".to_string()),
            // The #748 W5 pair's verbatim oracle correctionMessages
            // (diagnostic.g.dart :13187 and :17519).
            Self::NonNullableEqualsParameter => {
                Some("Try using a non-nullable type.".to_string())
            }
            Self::UnnecessaryNoSuchMethod => {
                Some("Try removing the declaration of 'noSuchMethod'.".to_string())
            }
            // The #749 W7 face (diagnostic.g.dart:12916).
            Self::NonConstCallToLiteralConstructor => {
                Some("Try adding a 'const' keyword.".to_string())
            }
            // The #1285 argument-count rows' verbatim oracle
            // correctionMessages (diagnostic.g.dart :6298 and :6315-6318;
            // the not-enough plurality split :13440/:13456 and
            // :13476/:13488).
            Self::ExtraPositionalArguments => {
                Some("Try removing the extra arguments.".to_string())
            }
            Self::ExtraPositionalArgumentsCouldBeNamed => Some(
                "Try removing the extra positional arguments, or specifying the name \
                 for named arguments."
                    .to_string(),
            ),
            Self::NotEnoughPositionalArguments(ArgCountShape::Singular)
            | Self::NotEnoughPositionalArguments(ArgCountShape::NameSingular) => {
                Some("Try adding the missing argument.".to_string())
            }
            Self::NotEnoughPositionalArguments(ArgCountShape::Plural)
            | Self::NotEnoughPositionalArguments(ArgCountShape::NamePlural) => {
                Some("Try adding the missing arguments.".to_string())
            }
            // `missingRequiredArgument`'s verbatim correctionMessage
            // (diagnostic.g.dart :11157).
            Self::MissingRequiredArgument => {
                Some("Try adding the required argument.".to_string())
            }
            // The #1314 FAM7 rows' verbatim oracle correctionMessages
            // (messages.yaml :11384/:11440/:14667 — the positional face
            // drops the `'required'` arm).
            Self::MissingDefaultValueForParameter => Some(
                "Try adding either an explicit non-'null' default value or the 'required' modifier."
                    .to_string(),
            ),
            Self::MissingDefaultValueForParameterPositional => {
                Some("Try adding an explicit non-'null' default value.".to_string())
            }
            Self::ObsoleteColonForDefaultValue => {
                Some("Try replacing the colon with an equal sign.".to_string())
            }
            // The #1314 FAM13 type-position context faces' verbatim oracle
            // correctionMessages (messages.yaml `nonTypeAsTypeArgument` and
            // `newWithNonType`).
            Self::NonTypeAsTypeArgument => Some(format!(
                "Try correcting the name to an existing type, or defining a type named '{a2}'."
            )),
            Self::TypeTestWithUndefinedName | Self::CastToNonType => Some(format!(
                "Try changing the name to the name of an existing type, or creating a type with the name '{a2}'."
            )),
            Self::CreationWithNonType | Self::NonTypeInCatchClause => {
                Some("Try correcting the name to match an existing class.".to_string())
            }
            // The oracle's `undefinedClass` correctionMessage (messages.yaml
            // :18433; the `{0}` is the written name — the emit helpers pass
            // it in the a2 slot).
            Self::UndefinedClass => Some(format!(
                "Try changing the name to the name of an existing class, or creating a class with the name '{a2}'."
            )),
            // The #1314 FAM14 clause faces' verbatim oracle corrections
            // (diagnostic.g.dart :7507-7508 for the implements face,
            // messages.yaml `extendsNonClass` for the extends face).
            Self::ImplementsNonClass => Some(
                "Try specifying a class or mixin, or remove the name from the list."
                    .to_string(),
            ),
            // The `extends_non_class` correction (messages.yaml
            // `extendsNonClass.correctionMessage`); the remaining FAM14
            // clause faces carry none.
            Self::ExtendsNonClass => Some(
                "Try specifying a different superclass, or removing the extends clause."
                    .to_string(),
            ),
            // The #1314 FAM18 uninitialized-non-nullable corrections
            // (diagnostic.g.dart :13506-13507/:13542).
            Self::NotInitializedNonNullableVariable => {
                Some("Try adding an initializer expression.".to_string())
            }
            Self::NotInitializedNonNullableInstanceField => Some(
                "Try adding an initializer expression, or a generative constructor \
                 that initializes it, or mark it 'late'."
                    .to_string(),
            ),
            Self::ImplicitSuperInitializerMissingArguments => Some(
                "Try adding an explicit super parameter with the required arguments."
                    .to_string(),
            ),
            Self::NonFinalFieldInEnum => Some("Try making the field final.".to_string()),
            // The constructor-initializer and final-field corrections
            // (diagnostic.g.dart :6617(no)/:8212/:14647/:6859/:6875).
            Self::InitializerForNonExistentField => Some(
                "Try correcting the name to match an existing field, or defining a \
                 field named '{a2}'."
                    .to_string(),
            ),
            Self::InitializingFormalForNonExistentField => Some(
                "Try correcting the name to match an existing field, or defining a \
                 field named '{a2}'."
                    .to_string(),
            ),
            Self::SuperFormalParameterWithoutAssociatedNamed => Some(
                "Try changing the name to the name of an existing named super \
                 constructor parameter, or creating such named parameter."
                    .to_string(),
            ),
            Self::SuperFormalParameterWithoutAssociatedPositional => Some(
                "Try using a normal parameter, or adding more positional parameters \
                 to the super constructor."
                    .to_string(),
            ),
            Self::RecursiveConstructorRedirect => Some(
                "Try changing one of the constructors in the loop to not redirect."
                    .to_string(),
            ),
            Self::RecursiveConstantConstructor => Some(
                "Try removing the constant dependency on the constructor itself."
                    .to_string(),
            ),
            Self::FinalNotInitialized => Some("Try initializing the variable.".to_string()),
            Self::FinalNotInitializedConstructor => {
                Some("Try adding an initializer for the field.".to_string())
            }
            // The FAM15 row's verbatim oracle correction (diagnostic.g.dart,
            // the uncheckedUseOfNullableValue family) — the property-access
            // face carries its own text.
            Self::UncheckedUseOfNullableValue if !a2.is_empty() => Some(
                "Try making the access conditional (using '?.') or adding a null check to the target ('!')."
                    .to_string(),
            ),
            Self::UncheckedUseOfNullableValue => Some(
                "Try checking that the value isn't 'null' before using it as a condition."
                    .to_string(),
            ),
            // The `enum_without_constants` correction (diagnostic.g.dart
            // :4970).
            Self::EnumWithoutConstants => {
                Some("Try declaring an enum constant.".to_string())
            }
            // The `non_constant_map_pattern_key` correction
            // (diagnostic.g.dart / messages.yaml `nonConstantMapPatternKey`).
            Self::NonConstantMapPatternKey => {
                Some("Try using constants instead.".to_string())
            }
            // The `await_in_wrong_context` correction (diagnostic.g.dart
            // :1478-1480).
            Self::AwaitInWrongContext => Some(
                "Try marking the function body with either 'async' or 'async*'.".to_string(),
            ),
            // The `constant_pattern_with_non_constant_expression`
            // correction (diagnostic.g.dart :2533).
            Self::ConstantPatternWithNonConstantExpression => {
                Some("Try making the expression a valid constant.".to_string())
            }
            // The redirect family's shared correctionMessage
            // (diagnostic.g.dart :14936/:14976/:14992/:14846 — every face
            // carries the identical `Try redirecting to a different
            // constructor.` text).
            Self::RedirectToInvalidReturnType
            | Self::RedirectToNonClass
            | Self::RedirectToNonConstConstructor
            | Self::RedirectGenerativeToNonGenerativeConstructor => {
                Some("Try redirecting to a different constructor.".to_string())
            }
            // `new_with_undefined_constructor_default`'s `{0}`-placeholder
            // correction (diagnostic.g.dart :12357-12360 — the class name
            // fills both slots).
            Self::NewWithUndefinedConstructorDefault => Some(format!(
                "Try using one of the named constructors defined in '{a2}'."
            )),
            // `duplicate_constructor`'s per-shape correctionMessages
            // (diagnostic.g.dart :4467/:4482).
            Self::DuplicateConstructor if a2.is_empty() => {
                Some("Try giving one of the constructors a name.".to_string())
            }
            Self::DuplicateConstructor => {
                Some("Try renaming one of the constructors.".to_string())
            }
            // `implicit_this_reference_in_initializer`'s verbatim text
            // (diagnostic.g.dart :7614-7617).
            Self::ImplicitThisReferenceInInitializer => Some(
                "Try replacing the reference to the instance member with a different expression"
                    .to_string(),
            ),
            // `definitely_unassigned_late_local_variable`'s correction
            // (diagnostic.g.dart :3734).
            Self::DefinitelyUnassignedLateLocalVariable => Some(
                "Ensure that it is assigned on necessary execution paths.".to_string(),
            ),
            // `const_constructor_param_type_mismatch`'s verbatim
            // correctionMessage (diagnostic.g.dart :2625).
            Self::ConstConstructorParamTypeMismatch => Some(
                "Try using a subtype, or removing the keyword 'const'.".to_string(),
            ),
            _ => None,
        }
    }

    /// The verbatim oracle `problemMessage` for the four-placeholder
    /// `_element` template: the kind word, the executable name, and the two
    /// counts.
    #[must_use]
    pub fn message4(self, a0: &str, a1: &str, a2: &str, a3: &str) -> String {
        match self {
            // The #612 explicit superinterface face: the parameter name, its
            // keyword, the position's keyword, the superinterface display
            // (diagnostic.g.dart:18500-18502).
            Self::WrongExplicitTypeParameterVarianceInSuperinterface => {
                format!(
                    "'{a0}' is an '{a1}' type parameter and can't be used in an '{a2}' position in '{a3}'."
                )
            }
            Self::WrongNumberOfTypeArgumentsElement(_) => {
                format!(
                    "The {a0} '{a1}' is declared with {a2} type parameters, but {a3} type arguments are given."
                )
            }
            Self::InheritsAbstractMemberFour => {
                format!("Missing concrete implementations of '{a0}', '{a1}', '{a2}', and '{a3}'.")
            }
            // Other rows keep their own (shorter) templates.
            _ => self.message(a0, a1),
        }
    }

    /// The verbatim oracle `problemMessage` for the five-placeholder
    /// `invalid_override` templates (`diagnostic.g.dart:9556-9571` and the
    /// setter text at :9616-9631): the member and its container name the
    /// declared face, the two signature texts fill the type slots.
    #[must_use]
    pub fn message5(self, a0: &str, a1: &str, a2: &str, a3: &str, a4: &str) -> String {
        match self {
            Self::InvalidOverride => {
                format!("'{a1}.{a0}' ('{a2}') isn't a valid override of '{a3}.{a0}' ('{a4}').")
            }
            Self::InvalidOverrideSetter => format!(
                "The setter '{a1}.{a0}' ('{a2}') isn't a valid override of '{a3}.{a0}' ('{a4}')."
            ),
            // The concrete-implementation face (diagnostic.g.dart:9067/:9107):
            // the same slots as invalid_override, the "concrete
            // implementation" verdict text.
            Self::InvalidImplementationOverride => {
                format!(
                    "'{a1}.{a0}' ('{a2}') isn't a valid concrete implementation of '{a3}.{a0}' ('{a4}')."
                )
            }
            Self::InvalidImplementationOverrideSetter => format!(
                "The setter '{a1}.{a0}' ('{a2}') isn't a valid concrete implementation of '{a3}.{a0}' ('{a4}')."
            ),
            Self::InheritsAbstractMemberFivePlus => {
                format!(
                    "Missing concrete implementations of '{a0}', '{a1}', '{a2}', '{a3}', and {a4} more."
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
    #[expect(clippy::too_many_lines)] // long diagnostic-table dispatch
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
            SemanticCode::AwaitInWrongContext,
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
            SemanticCode::TypeArgumentNotMatchingBounds,
            SemanticCode::EnumInstantiatedToBoundsIsNotWellBounded,
            SemanticCode::EnumWithoutConstants,
            SemanticCode::InvalidAssignment,
            SemanticCode::ArgumentTypeNotAssignable,
            SemanticCode::ExtensionOverrideArgumentNotAssignable,
            SemanticCode::ReturnOfInvalidType(ExecKind::Function),
            SemanticCode::ReturnOfInvalidType(ExecKind::Method),
            SemanticCode::ReturnOfInvalidType(ExecKind::Constructor),
            SemanticCode::InvalidNullAwareOperator,
            // The switch-exhaustiveness cluster.
            SemanticCode::NonExhaustiveSwitchStatement,
            SemanticCode::NonExhaustiveSwitchExpression,
            SemanticCode::UnreachableSwitchCase,
            SemanticCode::UnreachableSwitchDefault,
            SemanticCode::ConstInitializedWithNonConstantValue,
            SemanticCode::ConstWithNonConstantArgument,
            SemanticCode::ConstWithNonConst,
            SemanticCode::NonConstantListElement,
            SemanticCode::NonConstantSetElement,
            SemanticCode::NonConstantMapElement,
            SemanticCode::NonConstantRecordField,
            SemanticCode::ConstantPatternWithNonConstantExpression,
            SemanticCode::NonConstantCaseExpression,
            SemanticCode::NonConstantMapPatternKey,
            SemanticCode::ConstEvalTypeBool,
            SemanticCode::ConstEvalTypeBoolInt,
            SemanticCode::ConstEvalTypeBoolNumString,
            SemanticCode::ConstEvalTypeInt,
            SemanticCode::ConstEvalTypeNum,
            SemanticCode::ConstEvalTypeNumString,
            SemanticCode::ConstEvalTypeString,
            SemanticCode::UncheckedUseOfNullableValue,
            SemanticCode::ConstEvalThrowsException,
            SemanticCode::ConstEvalThrowsIdbze,
            SemanticCode::ConstEvalPrimitiveEquality,
            SemanticCode::ConstEvalMethodInvocation,
            SemanticCode::RecursiveCompileTimeConstant,
            SemanticCode::InvalidOverride,
            SemanticCode::InvalidOverrideSetter,
            SemanticCode::InvalidImplementationOverride,
            SemanticCode::InvalidImplementationOverrideSetter,
            SemanticCode::MustCallSuper,
            SemanticCode::MissingOverrideOfMustBeOverriddenOne,
            SemanticCode::MissingOverrideOfMustBeOverriddenTwo,
            SemanticCode::MissingOverrideOfMustBeOverriddenThreePlus,
            SemanticCode::OverrideOnNonOverridingField,
            SemanticCode::OverrideOnNonOverridingGetter,
            SemanticCode::OverrideOnNonOverridingMethod,
            SemanticCode::OverrideOnNonOverridingSetter,
            SemanticCode::DeadCode,
            SemanticCode::AmbiguousExtensionMemberTwo,
            SemanticCode::AmbiguousExtensionMemberThreeOrMore,
            SemanticCode::MemberWithClassName,
            SemanticCode::ExtensionTypeDeclaresMemberOfObject,
            SemanticCode::ExpectedRepresentationType,
            SemanticCode::ExtensionTypeRepresentationDependsOnItself,
            SemanticCode::ExtensionTypeImplementsNotSupertype,
            SemanticCode::InheritsAbstractMemberOne,
            SemanticCode::InheritsAbstractMemberTwo,
            SemanticCode::InheritsAbstractMemberThree,
            SemanticCode::InheritsAbstractMemberFour,
            SemanticCode::InheritsAbstractMemberFivePlus,
            SemanticCode::InconsistentInheritance,
            SemanticCode::InconsistentInheritanceGetterAndMethod,
            // The #504 usage cluster and the #505 import/flow rows.
            SemanticCode::UnusedLocalVariable,
            SemanticCode::UnusedElement,
            SemanticCode::UnusedField,
            SemanticCode::UnusedElementParameter,
            SemanticCode::UnusedCatchClause,
            SemanticCode::UnusedCatchStack,
            SemanticCode::UnusedLabel,
            SemanticCode::UnusedImport,
            SemanticCode::UnusedShownName,
            SemanticCode::UnusedResult,
            SemanticCode::UndefinedReferencedParameter,
            SemanticCode::DeadNullAwareExpression,
            SemanticCode::DeadCodeCatchFollowingCatch,
            // The #506 provably-check cluster and the #507 disjointness
            // cluster.
            SemanticCode::UnnecessaryCast,
            SemanticCode::UnnecessaryNonNullAssertion,
            SemanticCode::UnnecessaryTypeCheckTrue,
            SemanticCode::UnnecessaryTypeCheckFalse,
            SemanticCode::TypeCheckWithNull,
            SemanticCode::TypeCheckWithNotNull,
            SemanticCode::NullCheckAlwaysFails,
            SemanticCode::UnnecessaryNullComparisonFalse,
            SemanticCode::UnnecessaryNullComparisonTrue,
            SemanticCode::ConstantPatternNeverMatchesValueType,
            SemanticCode::PatternNeverMatchesValueType,
            SemanticCode::UnnecessaryCastPattern,
            SemanticCode::UnnecessaryNullCheckPattern,
            SemanticCode::UnnecessaryNullAssertPattern,
            // The #480 strict-mode rows, the #612 variance family, and the
            // #702 @sealed contract.
            SemanticCode::StrictRawType,
            SemanticCode::InferenceFailureOnFunctionReturnType,
            SemanticCode::WrongTypeParameterVariancePosition,
            SemanticCode::WrongExplicitTypeParameterVarianceInSuperinterface,
            SemanticCode::WrongTypeParameterVarianceInSuperinterface,
            SemanticCode::MixinOnSealedClass,
            SemanticCode::SubtypeOfSealedClass,
            // The #743 W1-micro cluster.
            SemanticCode::DeadCodeOnCatchSubtype,
            SemanticCode::NullArgumentToNonNullType,
            SemanticCode::NullableTypeInCatchClause,
            SemanticCode::UnnecessaryNanComparisonFalse,
            SemanticCode::UnnecessaryNanComparisonTrue,
            SemanticCode::UnnecessaryQuestionMark,
            SemanticCode::UnnecessaryWildcardPattern,
            // The #745 W2a pair.
            SemanticCode::CastFromNullAlwaysFails,
            SemanticCode::ReceiverOfTypeNever,
            // The #746 W2b pair.
            SemanticCode::CastFromNullableAlwaysFails,
            SemanticCode::BodyMightCompleteNormallyNullable,
            SemanticCode::BodyMightCompleteNormally,
            // The #747 W4 pair.
            SemanticCode::EqualElementsInSet,
            SemanticCode::EqualKeysInMap,
            // The #748 W5 pair.
            SemanticCode::NonNullableEqualsParameter,
            SemanticCode::UnnecessaryNoSuchMethod,
            // The #749 W7 face.
            SemanticCode::NonConstCallToLiteralConstructor,
            // The #911 primary-constructor face.
            SemanticCode::UnusedFieldFromPrimaryConstructor,
            // The #1285 argument-count cluster.
            SemanticCode::ExtraPositionalArguments,
            SemanticCode::ExtraPositionalArgumentsCouldBeNamed,
            SemanticCode::NotEnoughPositionalArguments(ArgCountShape::Singular),
            SemanticCode::NotEnoughPositionalArguments(ArgCountShape::Plural),
            SemanticCode::NotEnoughPositionalArguments(ArgCountShape::NameSingular),
            SemanticCode::NotEnoughPositionalArguments(ArgCountShape::NamePlural),
            SemanticCode::MissingRequiredArgument,
            // The #1314 default-value, type-position, class-clause, and
            // uninitialized-non-nullable clusters.
            SemanticCode::MissingDefaultValueForParameter,
            SemanticCode::MissingDefaultValueForParameterPositional,
            SemanticCode::ObsoleteColonForDefaultValue,
            SemanticCode::NonTypeAsTypeArgument,
            SemanticCode::CreationWithNonType,
            SemanticCode::TypeTestWithUndefinedName,
            SemanticCode::CastToNonType,
            SemanticCode::ClassUsedAsMixin,
            SemanticCode::NonTypeInCatchClause,
            SemanticCode::MixinOfNonClass,
            SemanticCode::ImplementsNonClass,
            SemanticCode::MixinSuperClassConstraintNonInterface,
            SemanticCode::MixinWithNonClassSuperclass,
            SemanticCode::ExtendsNonClass,
            SemanticCode::NotInitializedNonNullableVariable,
            SemanticCode::NotInitializedNonNullableInstanceField,
            SemanticCode::ImplicitSuperInitializerMissingArguments,
            SemanticCode::NonFinalFieldInEnum,
            // The N7 field-/super-formal faces.
            SemanticCode::InitializingFormalForNonExistentField,
            SemanticCode::SuperFormalParameterWithoutAssociatedNamed,
            SemanticCode::SuperFormalParameterWithoutAssociatedPositional,
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
            assert!(
                !code.message5("x", "y", "z", "w", "v").is_empty(),
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

    /// The severity pin's coverage list — grows with each implemented batch
    /// (a data table, so it lives outside the test body).
    const SEVERITY_CODES: &[SemanticCode] = &[
        // The warning-tier rows the usage/import clusters add.
        SemanticCode::UnusedLocalVariable,
        SemanticCode::UnusedElement,
        SemanticCode::UnusedField,
        SemanticCode::UnusedElementParameter,
        SemanticCode::UnusedCatchClause,
        SemanticCode::UnusedCatchStack,
        SemanticCode::UnusedLabel,
        SemanticCode::UnusedImport,
        SemanticCode::UnusedShownName,
        SemanticCode::DeadNullAwareExpression,
        SemanticCode::DeadCodeCatchFollowingCatch,
        // The #743 W1-micro cluster.
        SemanticCode::DeadCodeOnCatchSubtype,
        SemanticCode::NullArgumentToNonNullType,
        SemanticCode::NullableTypeInCatchClause,
        SemanticCode::UnnecessaryNanComparisonFalse,
        SemanticCode::UnnecessaryNanComparisonTrue,
        SemanticCode::UnnecessaryQuestionMark,
        SemanticCode::UnnecessaryWildcardPattern,
        // The #745 W2a pair.
        SemanticCode::CastFromNullAlwaysFails,
        SemanticCode::ReceiverOfTypeNever,
        // The #746 W2b pair.
        SemanticCode::CastFromNullableAlwaysFails,
        SemanticCode::BodyMightCompleteNormallyNullable,
        // The #747 W4 pair.
        SemanticCode::EqualElementsInSet,
        SemanticCode::EqualKeysInMap,
        // The #748 W5 pair.
        SemanticCode::NonNullableEqualsParameter,
        SemanticCode::UnnecessaryNoSuchMethod,
        // The #749 W7 face.
        SemanticCode::NonConstCallToLiteralConstructor,
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
        SemanticCode::ReturnOfInvalidType(ExecKind::Constructor),
        SemanticCode::InvalidNullAwareOperator,
        SemanticCode::NonExhaustiveSwitchStatement,
        SemanticCode::NonExhaustiveSwitchExpression,
        SemanticCode::UnreachableSwitchCase,
        SemanticCode::UnreachableSwitchDefault,
        SemanticCode::ConstInitializedWithNonConstantValue,
        SemanticCode::ConstWithNonConstantArgument,
        SemanticCode::ConstWithNonConst,
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
        SemanticCode::InvalidOverride,
        SemanticCode::InvalidOverrideSetter,
        SemanticCode::InvalidImplementationOverride,
        SemanticCode::InvalidImplementationOverrideSetter,
        SemanticCode::MustCallSuper,
        SemanticCode::MissingOverrideOfMustBeOverriddenOne,
        SemanticCode::MissingOverrideOfMustBeOverriddenTwo,
        SemanticCode::MissingOverrideOfMustBeOverriddenThreePlus,
        SemanticCode::OverrideOnNonOverridingField,
        SemanticCode::OverrideOnNonOverridingGetter,
        SemanticCode::OverrideOnNonOverridingMethod,
        SemanticCode::OverrideOnNonOverridingSetter,
        SemanticCode::DeadCode,
        SemanticCode::AmbiguousExtensionMemberTwo,
        SemanticCode::AmbiguousExtensionMemberThreeOrMore,
        SemanticCode::MemberWithClassName,
        SemanticCode::ExtensionTypeDeclaresMemberOfObject,
        SemanticCode::ExpectedRepresentationType,
        SemanticCode::ExtensionTypeRepresentationDependsOnItself,
        SemanticCode::InheritsAbstractMemberOne,
        SemanticCode::InheritsAbstractMemberTwo,
        SemanticCode::InheritsAbstractMemberThree,
        SemanticCode::InheritsAbstractMemberFour,
        SemanticCode::InheritsAbstractMemberFivePlus,
        SemanticCode::InconsistentInheritance,
        SemanticCode::InconsistentInheritanceGetterAndMethod,
    ];

    /// The severity derivation is the diagnostic layer's: every row's
    /// severity equals its tier's ([`DiagnosticType::severity`], the
    /// `dtype_matches_oracle_severity` invariant) — the pin against a
    /// reintroduced hand-rolled mapping.
    #[test]
    fn severity_folds_onto_the_tier_derivation() {
        for code in SEVERITY_CODES {
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
        assert_eq!(SemanticCode::DeadCode.severity(), Severity::Warning);
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
    #[expect(clippy::too_many_lines)] // long diagnostic-table dispatch
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
        // The unnamed face (the constant's close flush marks `a1` —
        // `unnamed` — to pick the parameter-less oracle row, verbatim).
        assert_eq!(
            SemanticCode::UndefinedEnumConstructor.message("", "unnamed"),
            "The enum doesn't have an unnamed constructor."
        );
        // A selector the constant wrote stays the named face, even when the
        // parser inserted it (the `''` recovery spelling).
        assert_eq!(
            SemanticCode::UndefinedEnumConstructor.message("", ""),
            "The enum doesn't have a constructor named ''."
        );
        assert_eq!(
            SemanticCode::WrongNumberOfParametersForOperator.message3("+", "1", "2"),
            "Operator '+' should declare exactly 1 parameters, but 2 found."
        );
        assert_eq!(
            SemanticCode::WrongNumberOfParametersForOperatorMinus.message3("3", "", ""),
            "Operator '-' should declare 0 or 1 parameter, but 3 found."
        );
        // The registry-backed rows: rendered from the dartq-diagnostics
        // table; these literals are the migration's byte-identity pins (the
        // registry-side pins live in dartq-diagnostics'
        // `message_template_covers_the_table_and_pins_emit_rows`).
        assert_eq!(
            SemanticCode::WrongNumberOfParametersForSetter.message("", ""),
            "Setters must declare exactly one required positional parameter."
        );
        assert_eq!(
            SemanticCode::MemberWithClassName.message("", ""),
            "A class member can't have the same name as the enclosing class."
        );
        assert_eq!(
            SemanticCode::ExpectedRepresentationType.message("", ""),
            "Expected a representation type."
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
        // The constructor face (the factory return check): the host word is
        // "constructor" (returnOfInvalidTypeFromConstructor).
        assert_eq!(
            SemanticCode::ReturnOfInvalidType(ExecKind::Constructor).message3("int", "E", "E"),
            "A value of type 'int' can't be returned from the constructor 'E' because it has a return type of 'E'."
        );
        assert_eq!(
            SemanticCode::InvalidNullAwareOperator.message("?.", "."),
            "The receiver can't be null, so the null-aware operator '?.' is unnecessary."
        );
        assert_eq!(
            SemanticCode::UnreachableSwitchDefault.message("", ""),
            "This default clause is covered by the previous cases."
        );
        // The #449 flow row: no-argument problem message, verbatim
        // correction.
        assert_eq!(SemanticCode::DeadCode.message("", ""), "Dead code.");
        assert_eq!(
            SemanticCode::DeadCode.correction_message(""),
            Some(
                "Try removing the code, or fixing the code before it so that it can be reached."
                    .to_string()
            )
        );
        // The #453 override templates: one code name, two message shapes.
        assert_eq!(
            SemanticCode::InvalidOverride.message5(
                "foo",
                "B",
                "void Function(int)",
                "A",
                "void Function()"
            ),
            "'B.foo' ('void Function(int)') isn't a valid override of 'A.foo' ('void Function()')."
        );
        assert_eq!(
            SemanticCode::InvalidOverrideSetter.message5(
                "foo",
                "B",
                "void Function(int?)",
                "A",
                "void Function(int)"
            ),
            "The setter 'B.foo' ('void Function(int?)') isn't a valid override of 'A.foo' ('void Function(int)')."
        );
        // The #314 residue templates: the concrete-implementation pair, the
        // must-call-super row, the mustBeOverridden tiers, and the four
        // non-overriding member faces (diagnostic.g.dart:9067/:9107/:12061/
        // :11045-11092/:13806-13855).
        assert_eq!(
            SemanticCode::InvalidImplementationOverride.message5(
                "foo",
                "B",
                "void Function(int)",
                "A",
                "void Function(num)"
            ),
            "'B.foo' ('void Function(int)') isn't a valid concrete implementation of 'A.foo' ('void Function(num)')."
        );
        assert_eq!(
            SemanticCode::InvalidImplementationOverrideSetter.message5(
                "foo",
                "B",
                "void Function(int?)",
                "A",
                "void Function(num)"
            ),
            "The setter 'B.foo' ('void Function(int?)') isn't a valid concrete implementation of 'A.foo' ('void Function(num)')."
        );
        assert_eq!(
            SemanticCode::MustCallSuper.message("A", ""),
            "This method overrides a method annotated as '@mustCallSuper' in 'A', but doesn't invoke the overridden method."
        );
        assert_eq!(
            SemanticCode::MissingOverrideOfMustBeOverriddenOne.message("m", ""),
            "Missing a required override of 'm'."
        );
        assert_eq!(
            SemanticCode::MissingOverrideOfMustBeOverriddenTwo.message3("m", "n", ""),
            "Missing a required override of 'm' and 'n'."
        );
        assert_eq!(
            SemanticCode::MissingOverrideOfMustBeOverriddenThreePlus.message3("m", "n", "2"),
            "Missing a required override of 'm', 'n', and 2 more."
        );
        assert_eq!(
            SemanticCode::OverrideOnNonOverridingField.message("", ""),
            "The field doesn't override an inherited getter or setter."
        );
        assert_eq!(
            SemanticCode::OverrideOnNonOverridingGetter.message("", ""),
            "The getter doesn't override an inherited getter."
        );
        assert_eq!(
            SemanticCode::OverrideOnNonOverridingMethod.message("", ""),
            "The method doesn't override an inherited method."
        );
        assert_eq!(
            SemanticCode::OverrideOnNonOverridingSetter.message("", ""),
            "The setter doesn't override an inherited setter."
        );
        assert_eq!(
            SemanticCode::MixinOnSealedClass.message("Guard", ""),
            "The class 'Guard' shouldn't be used as a mixin constraint because it is sealed, and any class mixing in this mixin must have 'Guard' as a superclass."
        );
        assert_eq!(
            SemanticCode::SubtypeOfSealedClass.message("Guard", ""),
            "The class 'Guard' shouldn't be extended, mixed in, or implemented because it's sealed."
        );
        assert_eq!(
            SemanticCode::DeadCodeOnCatchSubtype.message("Error", "Exception"),
            "Dead code: This on-catch block won't be executed because 'Error' is a subtype of 'Exception' and hence will have been caught already."
        );
        assert_eq!(
            SemanticCode::NullArgumentToNonNullType.message("Completer.complete", "int"),
            "'Completer.complete' shouldn't be called with a 'null' argument for the non-nullable type argument 'int'."
        );
        assert_eq!(
            SemanticCode::NullableTypeInCatchClause.message("", ""),
            "A potentially nullable type can't be used in an 'on' clause because it isn't valid to throw a nullable expression."
        );
        assert_eq!(
            SemanticCode::UnnecessaryNanComparisonTrue.message("", ""),
            "A double can't equal 'double.nan', so the condition is always 'true'."
        );
        assert_eq!(
            SemanticCode::UnnecessaryNanComparisonFalse.message("", ""),
            "A double can't equal 'double.nan', so the condition is always 'false'."
        );
        assert_eq!(
            SemanticCode::UnnecessaryQuestionMark.message("dynamic", ""),
            "The '?' is unnecessary because 'dynamic' is nullable without it."
        );
        assert_eq!(
            SemanticCode::UnnecessaryWildcardPattern.message("", ""),
            "Unnecessary wildcard pattern."
        );
        assert_eq!(
            SemanticCode::CastFromNullAlwaysFails.message("", ""),
            "This cast always throws an exception because the expression always evaluates to 'null'."
        );
        assert_eq!(
            SemanticCode::ReceiverOfTypeNever.message("", ""),
            "The receiver is of type 'Never', and will never complete with a value."
        );
        assert_eq!(
            SemanticCode::CastFromNullableAlwaysFails.message("x", ""),
            "This cast will always throw an exception because the nullable local variable 'x' is not assigned."
        );
        assert_eq!(
            SemanticCode::BodyMightCompleteNormallyNullable.message("int?", ""),
            "This function has a nullable return type of 'int?', but ends without returning a value."
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
        assert_eq!(
            SemanticCode::MixinOnSealedClass.correction_message(""),
            Some(
                "Try composing with this class, or refer to its documentation for more information."
                    .to_string()
            )
        );
        assert_eq!(
            SemanticCode::SubtypeOfSealedClass.correction_message("Guard"),
            Some(
                "Try composing instead of inheriting, or refer to the documentation of 'Guard' for more information."
                    .to_string()
            )
        );
        assert_eq!(
            SemanticCode::DeadCodeOnCatchSubtype.correction_message(""),
            Some(
                "Try reordering the catch clauses so that this block can be reached, or removing the unreachable catch clause."
                    .to_string()
            )
        );
        assert_eq!(
            SemanticCode::NullArgumentToNonNullType.correction_message(""),
            Some("Try adding a non-null argument.".to_string())
        );
        assert_eq!(
            SemanticCode::NullableTypeInCatchClause.correction_message(""),
            Some("Try using a non-nullable type.".to_string())
        );
        assert_eq!(
            SemanticCode::UnnecessaryNanComparisonTrue.correction_message(""),
            Some("Try using 'double.isNan', or removing the condition.".to_string())
        );
        assert_eq!(
            SemanticCode::UnnecessaryQuestionMark.correction_message(""),
            None,
            "the registry row declares no correctionMessage"
        );
        assert_eq!(
            SemanticCode::UnnecessaryWildcardPattern.correction_message(""),
            Some("Try removing the wildcard pattern.".to_string())
        );
        // The #745 W2a pair: the receiver row's verbatim correction (no
        // trailing period in the registry template); the cast row declares
        // none.
        assert_eq!(
            SemanticCode::ReceiverOfTypeNever.correction_message(""),
            Some("Try checking for throw expressions or type errors in the receiver".to_string())
        );
        assert_eq!(
            SemanticCode::CastFromNullAlwaysFails.correction_message(""),
            None,
            "the registry row declares no correctionMessage"
        );
        // The #746 W2b pair: both rows carry the registry's verbatim
        // correctionMessage (messages.yaml :25710 and :25640).
        assert_eq!(
            SemanticCode::CastFromNullableAlwaysFails.correction_message(""),
            Some(
                "Try giving it an initializer expression, or ensure that it's assigned on every execution path."
                    .to_string()
            )
        );
        assert_eq!(
            SemanticCode::BodyMightCompleteNormallyNullable.correction_message(""),
            Some(
                "Try adding a return statement, or if no value is ever returned, try changing the return type to 'void'."
                    .to_string()
            )
        );
    }

    /// The rows whose message text renders from the dartq-diagnostics table
    /// (the `registry_message` call sites — the emission-path single
    /// source). The sync test below keeps this mirror exactly equal to the
    /// registry's coverage of `SemanticCode` names.
    const REGISTRY_BACKED: &[&str] = &[
        "wrong_number_of_parameters_for_setter",
        "member_with_class_name",
        "expected_representation_type",
    ];

    /// The exhaustive enum walk: one arm per `SemanticCode` variant, no
    /// wildcard — a new variant fails this match's compilation (E0004) until
    /// it joins the walk. The runtime no longer panics on codes↔registry
    /// drift (`registry_message` degrades to the raw name), so this
    /// compile-time gate is what forces every new row's registry
    /// relationship to be declared here, loudly, at test time.
    #[expect(clippy::too_many_lines)] // the exhaustive per-variant walk
    fn registry_walk(code: &SemanticCode) -> CodeInfo {
        match code {
            SemanticCode::AmbiguousExtensionMemberTwo
            | SemanticCode::AmbiguousExtensionMemberThreeOrMore
            | SemanticCode::ArgumentTypeNotAssignable
            | SemanticCode::BodyMightCompleteNormally
            | SemanticCode::BodyMightCompleteNormallyNullable
            | SemanticCode::CastFromNullAlwaysFails
            | SemanticCode::CastFromNullableAlwaysFails
            | SemanticCode::ConstEvalMethodInvocation
            | SemanticCode::ConstEvalPrimitiveEquality
            | SemanticCode::ConstEvalThrowsException
            | SemanticCode::ConstEvalThrowsIdbze
            | SemanticCode::ConstEvalTypeBool
            | SemanticCode::ConstEvalTypeBoolInt
            | SemanticCode::ConstEvalTypeBoolNumString
            | SemanticCode::ConstEvalTypeInt
            | SemanticCode::ConstEvalTypeNum
            | SemanticCode::ConstEvalTypeNumString
            | SemanticCode::ConstEvalTypeString
            | SemanticCode::ConstInitializedWithNonConstantValue
            | SemanticCode::ConstWithNonConst
            | SemanticCode::ConstWithNonConstantArgument
            | SemanticCode::ConstantPatternNeverMatchesValueType
            | SemanticCode::ConstantPatternWithNonConstantExpression
            | SemanticCode::NonConstantCaseExpression
            | SemanticCode::NonConstantMapPatternKey
            | SemanticCode::CouldNotInfer
            | SemanticCode::CreationWithNonType
            | SemanticCode::DeadCode
            | SemanticCode::DeadCodeCatchFollowingCatch
            | SemanticCode::DeadCodeOnCatchSubtype
            | SemanticCode::DeadNullAwareExpression
            | SemanticCode::DuplicateDefinition
            | SemanticCode::EnumInstantiatedToBoundsIsNotWellBounded
            | SemanticCode::EnumWithoutConstants
            | SemanticCode::EqualElementsInSet
            | SemanticCode::EqualKeysInMap
            | SemanticCode::ExpectedRepresentationType
            | SemanticCode::ExtensionOverrideArgumentNotAssignable
            | SemanticCode::ExtensionTypeDeclaresMemberOfObject
            | SemanticCode::ExtensionTypeRepresentationDependsOnItself
            | SemanticCode::ExtensionTypeImplementsNotSupertype
            | SemanticCode::ExtraPositionalArguments
            | SemanticCode::ExtraPositionalArgumentsCouldBeNamed
            | SemanticCode::ExtendsNonClass
            | SemanticCode::ImplementsNonClass
            | SemanticCode::InconsistentInheritance
            | SemanticCode::InconsistentInheritanceGetterAndMethod
            | SemanticCode::InferenceFailureOnFunctionReturnType
            | SemanticCode::InheritsAbstractMemberFivePlus
            | SemanticCode::InheritsAbstractMemberFour
            | SemanticCode::InheritsAbstractMemberOne
            | SemanticCode::InheritsAbstractMemberThree
            | SemanticCode::InheritsAbstractMemberTwo
            | SemanticCode::InvalidAssignment
            | SemanticCode::InvalidImplementationOverride
            | SemanticCode::InvalidImplementationOverrideSetter
            | SemanticCode::InvalidNullAwareOperator
            | SemanticCode::InvalidOverride
            | SemanticCode::InvalidOverrideSetter
            | SemanticCode::MemberWithClassName
            | SemanticCode::MissingDefaultValueForParameter
            | SemanticCode::MissingDefaultValueForParameterPositional
            | SemanticCode::MissingRequiredArgument
            | SemanticCode::MissingOverrideOfMustBeOverriddenOne
            | SemanticCode::MissingOverrideOfMustBeOverriddenThreePlus
            | SemanticCode::MissingOverrideOfMustBeOverriddenTwo
            | SemanticCode::MixinOfNonClass
            | SemanticCode::MixinOnSealedClass
            | SemanticCode::MixinSuperClassConstraintNonInterface
            | SemanticCode::MixinWithNonClassSuperclass
            | SemanticCode::MustCallSuper
            | SemanticCode::NonConstCallToLiteralConstructor
            | SemanticCode::NonConstantListElement
            | SemanticCode::NonConstantMapElement
            | SemanticCode::NonConstantRecordField
            | SemanticCode::NonConstantSetElement
            | SemanticCode::NonExhaustiveSwitchExpression
            | SemanticCode::NonExhaustiveSwitchStatement
            | SemanticCode::NonNullableEqualsParameter
            | SemanticCode::NonTypeAsTypeArgument
            | SemanticCode::NonTypeInCatchClause
            | SemanticCode::FieldInitializerNotAssignable
            | SemanticCode::FinalNotInitialized
            | SemanticCode::FinalNotInitializedConstructor
            | SemanticCode::InitializerForNonExistentField
            | SemanticCode::InitializingFormalForNonExistentField
            | SemanticCode::SuperFormalParameterWithoutAssociatedNamed
            | SemanticCode::SuperFormalParameterWithoutAssociatedPositional
            | SemanticCode::RecursiveConstructorRedirect
            | SemanticCode::RecursiveConstantConstructor
            | SemanticCode::ConstConstructorParamTypeMismatch
            | SemanticCode::NotInitializedNonNullableInstanceField
            | SemanticCode::ImplicitSuperInitializerMissingArguments
            | SemanticCode::NonFinalFieldInEnum
            | SemanticCode::NotInitializedNonNullableVariable
            | SemanticCode::NullArgumentToNonNullType
            | SemanticCode::NullCheckAlwaysFails
            | SemanticCode::NullableTypeInCatchClause
            | SemanticCode::ObsoleteColonForDefaultValue
            | SemanticCode::OverrideOnNonOverridingField
            | SemanticCode::OverrideOnNonOverridingGetter
            | SemanticCode::OverrideOnNonOverridingMethod
            | SemanticCode::OverrideOnNonOverridingSetter
            | SemanticCode::PatternNeverMatchesValueType
            | SemanticCode::ReceiverOfTypeNever
            | SemanticCode::RecursiveCompileTimeConstant
            | SemanticCode::StrictRawType
            | SemanticCode::SubtypeOfSealedClass
            | SemanticCode::TypeArgumentNotMatchingBounds
            | SemanticCode::TypeCheckWithNotNull
            | SemanticCode::TypeCheckWithNull
            | SemanticCode::UncheckedUseOfNullableValue
            | SemanticCode::UndefinedAnnotation
            | SemanticCode::UndefinedClass
            | SemanticCode::UndefinedConstructorInInitializer
            | SemanticCode::UndefinedEnumConstant
            | SemanticCode::UndefinedEnumConstructor
            | SemanticCode::UndefinedExtensionGetter
            | SemanticCode::UndefinedExtensionMethod
            | SemanticCode::UndefinedExtensionOperator
            | SemanticCode::UndefinedExtensionSetter
            | SemanticCode::UndefinedFunction
            | SemanticCode::UndefinedGetter
            | SemanticCode::UndefinedIdentifier
            | SemanticCode::UndefinedIdentifierAwait
            | SemanticCode::AwaitInWrongContext
            | SemanticCode::UndefinedMethod
            | SemanticCode::UndefinedNamedParameter
            | SemanticCode::UndefinedOperator
            | SemanticCode::UndefinedPrefixedName
            | SemanticCode::UndefinedReferencedParameter
            | SemanticCode::UndefinedSetter
            | SemanticCode::PrefixShadowedByLocalDeclaration
            | SemanticCode::UnnecessaryCast
            | SemanticCode::UnnecessaryCastPattern
            | SemanticCode::UnnecessaryNanComparisonFalse
            | SemanticCode::UnnecessaryNanComparisonTrue
            | SemanticCode::UnnecessaryNoSuchMethod
            | SemanticCode::UnnecessaryNonNullAssertion
            | SemanticCode::UnnecessaryNullAssertPattern
            | SemanticCode::UnnecessaryNullCheckPattern
            | SemanticCode::UnnecessaryNullComparisonFalse
            | SemanticCode::UnnecessaryNullComparisonTrue
            | SemanticCode::UnnecessaryQuestionMark
            | SemanticCode::UnnecessaryTypeCheckFalse
            | SemanticCode::UnnecessaryTypeCheckTrue
            | SemanticCode::UnnecessaryWildcardPattern
            | SemanticCode::UnreachableSwitchCase
            | SemanticCode::UnreachableSwitchDefault
            | SemanticCode::UnusedCatchClause
            | SemanticCode::UnusedCatchStack
            | SemanticCode::UnusedElement
            | SemanticCode::UnusedElementParameter
            | SemanticCode::UnusedField
            | SemanticCode::UnusedFieldFromPrimaryConstructor
            | SemanticCode::UnusedImport
            | SemanticCode::UnusedLabel
            | SemanticCode::UnusedLocalVariable
            | SemanticCode::UnusedResult
            | SemanticCode::UnusedShownName
            | SemanticCode::WrongExplicitTypeParameterVarianceInSuperinterface
            | SemanticCode::WrongNumberOfParametersForOperator
            | SemanticCode::WrongNumberOfParametersForOperatorMinus
            | SemanticCode::WrongNumberOfParametersForSetter
            | SemanticCode::WrongNumberOfTypeArguments
            | SemanticCode::WrongNumberOfTypeArgumentsConstructor
            | SemanticCode::WrongNumberOfTypeArgumentsEnum
            | SemanticCode::WrongNumberOfTypeArgumentsExtension
            | SemanticCode::WrongNumberOfTypeArgumentsFunction
            | SemanticCode::WrongTypeParameterVarianceInSuperinterface
            | SemanticCode::WrongTypeParameterVariancePosition
            | SemanticCode::UndefinedSuperMember(_)
            | SemanticCode::WrongNumberOfTypeArgumentsElement(_)
            | SemanticCode::ReturnOfInvalidType(_)
            | SemanticCode::NotEnoughPositionalArguments(_)
            | SemanticCode::ConflictingStaticAndInstance
            | SemanticCode::ConstInstanceField
            | SemanticCode::NonVoidReturnForSetter
            | SemanticCode::NotAType
            | SemanticCode::NonBoolCondition
            | SemanticCode::NonBoolNegationExpression
            | SemanticCode::InvalidUseOfNullValue
            | SemanticCode::InvocationOfNonFunctionExpression
            | SemanticCode::InvalidReferenceToThis
            | SemanticCode::SuperInInvalidContext
            | SemanticCode::RedirectToInvalidReturnType
            | SemanticCode::RedirectGenerativeToNonGenerativeConstructor
            | SemanticCode::RedirectToNonClass
            | SemanticCode::RedirectToNonConstConstructor
            | SemanticCode::NewWithUndefinedConstructorDefault
            | SemanticCode::DuplicateConstructor
            | SemanticCode::ImplicitThisReferenceInInitializer
            | SemanticCode::ListElementTypeNotAssignable
            | SemanticCode::DefinitelyUnassignedLateLocalVariable
            | SemanticCode::TypeTestWithUndefinedName
            | SemanticCode::CastToNonType
            | SemanticCode::ClassUsedAsMixin => code.code_info(),
        }
    }

    /// Every `SemanticCode` variant, payload shapes materialized — the sync
    /// walk below asserts the registry relationship per rendered name.
    /// Compiler-enforced completeness rides on [`registry_walk`]: a new
    /// variant cannot compile until it joins that match, and this list
    /// materializes its shapes for the runtime assertions.
    #[expect(clippy::too_many_lines)] // the full materialized code table
    fn all_codes() -> Vec<SemanticCode> {
        vec![
            SemanticCode::AmbiguousExtensionMemberTwo,
            SemanticCode::AmbiguousExtensionMemberThreeOrMore,
            SemanticCode::ArgumentTypeNotAssignable,
            SemanticCode::BodyMightCompleteNormally,
            SemanticCode::BodyMightCompleteNormallyNullable,
            SemanticCode::CastFromNullAlwaysFails,
            SemanticCode::CastFromNullableAlwaysFails,
            SemanticCode::ConstEvalMethodInvocation,
            SemanticCode::ConstEvalPrimitiveEquality,
            SemanticCode::ConstEvalThrowsException,
            SemanticCode::ConstEvalThrowsIdbze,
            SemanticCode::ConstEvalTypeBool,
            SemanticCode::ConstEvalTypeBoolInt,
            SemanticCode::ConstEvalTypeBoolNumString,
            SemanticCode::ConstEvalTypeInt,
            SemanticCode::ConstEvalTypeNum,
            SemanticCode::ConstEvalTypeNumString,
            SemanticCode::ConstEvalTypeString,
            SemanticCode::ConstInitializedWithNonConstantValue,
            SemanticCode::ConstWithNonConst,
            SemanticCode::ConstWithNonConstantArgument,
            SemanticCode::ConstantPatternNeverMatchesValueType,
            SemanticCode::ConstantPatternWithNonConstantExpression,
            SemanticCode::NonConstantCaseExpression,
            SemanticCode::NonConstantMapPatternKey,
            SemanticCode::CreationWithNonType,
            SemanticCode::DeadCode,
            SemanticCode::DeadCodeCatchFollowingCatch,
            SemanticCode::DeadCodeOnCatchSubtype,
            SemanticCode::DeadNullAwareExpression,
            SemanticCode::DuplicateDefinition,
            SemanticCode::EnumInstantiatedToBoundsIsNotWellBounded,
            SemanticCode::EnumWithoutConstants,
            SemanticCode::EqualElementsInSet,
            SemanticCode::EqualKeysInMap,
            SemanticCode::ExpectedRepresentationType,
            SemanticCode::ExtensionOverrideArgumentNotAssignable,
            SemanticCode::ExtensionTypeDeclaresMemberOfObject,
            SemanticCode::ExtensionTypeRepresentationDependsOnItself,
            SemanticCode::ExtensionTypeImplementsNotSupertype,
            SemanticCode::ExtraPositionalArguments,
            SemanticCode::ExtraPositionalArgumentsCouldBeNamed,
            SemanticCode::ExtendsNonClass,
            SemanticCode::ImplementsNonClass,
            SemanticCode::InconsistentInheritance,
            SemanticCode::InconsistentInheritanceGetterAndMethod,
            SemanticCode::InferenceFailureOnFunctionReturnType,
            SemanticCode::InheritsAbstractMemberFivePlus,
            SemanticCode::InheritsAbstractMemberFour,
            SemanticCode::InheritsAbstractMemberOne,
            SemanticCode::InheritsAbstractMemberThree,
            SemanticCode::InheritsAbstractMemberTwo,
            SemanticCode::InvalidAssignment,
            SemanticCode::InvalidImplementationOverride,
            SemanticCode::InvalidImplementationOverrideSetter,
            SemanticCode::InvalidNullAwareOperator,
            SemanticCode::InvalidOverride,
            SemanticCode::InvalidOverrideSetter,
            SemanticCode::MemberWithClassName,
            SemanticCode::MissingDefaultValueForParameter,
            SemanticCode::MissingDefaultValueForParameterPositional,
            SemanticCode::MissingRequiredArgument,
            SemanticCode::MissingOverrideOfMustBeOverriddenOne,
            SemanticCode::MissingOverrideOfMustBeOverriddenThreePlus,
            SemanticCode::MissingOverrideOfMustBeOverriddenTwo,
            SemanticCode::MixinOfNonClass,
            SemanticCode::MixinOnSealedClass,
            SemanticCode::MixinSuperClassConstraintNonInterface,
            SemanticCode::MixinWithNonClassSuperclass,
            SemanticCode::MustCallSuper,
            SemanticCode::NonConstCallToLiteralConstructor,
            SemanticCode::NonConstantListElement,
            SemanticCode::NonConstantMapElement,
            SemanticCode::NonConstantRecordField,
            SemanticCode::NonConstantSetElement,
            SemanticCode::NonExhaustiveSwitchExpression,
            SemanticCode::NonExhaustiveSwitchStatement,
            SemanticCode::NonNullableEqualsParameter,
            SemanticCode::NonTypeAsTypeArgument,
            SemanticCode::TypeTestWithUndefinedName,
            SemanticCode::CastToNonType,
            SemanticCode::ClassUsedAsMixin,
            SemanticCode::NotEnoughPositionalArguments(ArgCountShape::Singular),
            SemanticCode::NotEnoughPositionalArguments(ArgCountShape::Plural),
            SemanticCode::NotEnoughPositionalArguments(ArgCountShape::NameSingular),
            SemanticCode::NotEnoughPositionalArguments(ArgCountShape::NamePlural),
            SemanticCode::NotInitializedNonNullableInstanceField,
            SemanticCode::ImplicitSuperInitializerMissingArguments,
            SemanticCode::NonFinalFieldInEnum,
            SemanticCode::NotInitializedNonNullableVariable,
            SemanticCode::NullArgumentToNonNullType,
            SemanticCode::NullCheckAlwaysFails,
            SemanticCode::NullableTypeInCatchClause,
            SemanticCode::ObsoleteColonForDefaultValue,
            SemanticCode::OverrideOnNonOverridingField,
            SemanticCode::OverrideOnNonOverridingGetter,
            SemanticCode::OverrideOnNonOverridingMethod,
            SemanticCode::OverrideOnNonOverridingSetter,
            SemanticCode::PatternNeverMatchesValueType,
            SemanticCode::ReceiverOfTypeNever,
            SemanticCode::RecursiveCompileTimeConstant,
            SemanticCode::ReturnOfInvalidType(ExecKind::Function),
            SemanticCode::ReturnOfInvalidType(ExecKind::Method),
            SemanticCode::ReturnOfInvalidType(ExecKind::Constructor),
            SemanticCode::StrictRawType,
            SemanticCode::SubtypeOfSealedClass,
            SemanticCode::TypeArgumentNotMatchingBounds,
            SemanticCode::TypeCheckWithNotNull,
            SemanticCode::TypeCheckWithNull,
            SemanticCode::UncheckedUseOfNullableValue,
            SemanticCode::UndefinedAnnotation,
            SemanticCode::UndefinedClass,
            SemanticCode::UndefinedConstructorInInitializer,
            SemanticCode::UndefinedEnumConstant,
            SemanticCode::UndefinedEnumConstructor,
            SemanticCode::UndefinedExtensionGetter,
            SemanticCode::UndefinedExtensionMethod,
            SemanticCode::UndefinedExtensionOperator,
            SemanticCode::UndefinedExtensionSetter,
            SemanticCode::UndefinedFunction,
            SemanticCode::UndefinedGetter,
            SemanticCode::UndefinedIdentifier,
            SemanticCode::UndefinedIdentifierAwait,
            SemanticCode::AwaitInWrongContext,
            SemanticCode::UndefinedMethod,
            SemanticCode::UndefinedNamedParameter,
            SemanticCode::UndefinedOperator,
            SemanticCode::UndefinedPrefixedName,
            SemanticCode::UndefinedReferencedParameter,
            SemanticCode::UndefinedSetter,
            SemanticCode::UndefinedSuperMember(SuperShape::Getter),
            SemanticCode::UndefinedSuperMember(SuperShape::Method),
            SemanticCode::UndefinedSuperMember(SuperShape::Operator),
            SemanticCode::UndefinedSuperMember(SuperShape::Setter),
            SemanticCode::UnnecessaryCast,
            SemanticCode::UnnecessaryCastPattern,
            SemanticCode::UnnecessaryNanComparisonFalse,
            SemanticCode::UnnecessaryNanComparisonTrue,
            SemanticCode::UnnecessaryNoSuchMethod,
            SemanticCode::UnnecessaryNonNullAssertion,
            SemanticCode::UnnecessaryNullAssertPattern,
            SemanticCode::UnnecessaryNullCheckPattern,
            SemanticCode::UnnecessaryNullComparisonFalse,
            SemanticCode::UnnecessaryNullComparisonTrue,
            SemanticCode::UnnecessaryQuestionMark,
            SemanticCode::UnnecessaryTypeCheckFalse,
            SemanticCode::UnnecessaryTypeCheckTrue,
            SemanticCode::UnnecessaryWildcardPattern,
            SemanticCode::UnreachableSwitchCase,
            SemanticCode::UnreachableSwitchDefault,
            SemanticCode::UnusedCatchClause,
            SemanticCode::UnusedCatchStack,
            SemanticCode::UnusedElement,
            SemanticCode::UnusedElementParameter,
            SemanticCode::UnusedField,
            SemanticCode::UnusedFieldFromPrimaryConstructor,
            SemanticCode::UnusedImport,
            SemanticCode::UnusedLabel,
            SemanticCode::UnusedLocalVariable,
            SemanticCode::UnusedResult,
            SemanticCode::UnusedShownName,
            SemanticCode::WrongExplicitTypeParameterVarianceInSuperinterface,
            SemanticCode::WrongNumberOfParametersForOperator,
            SemanticCode::WrongNumberOfParametersForOperatorMinus,
            SemanticCode::WrongNumberOfParametersForSetter,
            SemanticCode::WrongNumberOfTypeArguments,
            SemanticCode::WrongNumberOfTypeArgumentsConstructor,
            SemanticCode::WrongNumberOfTypeArgumentsElement(ExecKind::Function),
            SemanticCode::WrongNumberOfTypeArgumentsElement(ExecKind::Method),
            SemanticCode::WrongNumberOfTypeArgumentsEnum,
            SemanticCode::WrongNumberOfTypeArgumentsExtension,
            SemanticCode::WrongNumberOfTypeArgumentsFunction,
            SemanticCode::WrongTypeParameterVarianceInSuperinterface,
            SemanticCode::WrongTypeParameterVariancePosition,
        ]
    }

    /// The batch-G codes↔registry sync gate: the runtime `registry_message`
    /// path degrades to the raw name on table drift, so this test is the
    /// loud failure. The exhaustive walk asserts, for every variant's
    /// official name, that registry coverage is exactly the
    /// [`REGISTRY_BACKED`] mirror — a renamed, dropped, or newly
    /// registry-backed row fails here instead of panicking at emission.
    #[test]
    fn registry_backed_rows_stay_in_sync_with_the_table() {
        for code in all_codes() {
            let info = registry_walk(&code);
            assert_eq!(
                message_template(info.name).is_some(),
                REGISTRY_BACKED.contains(&info.name),
                "{}: registry coverage drifted from the registry-backed mirror",
                info.name
            );
        }
        for name in REGISTRY_BACKED {
            assert!(
                message_template(name).is_some(),
                "{name}: registry-backed row absent from the dartq-diagnostics table"
            );
        }
        // The runtime degrade posture: an absent name renders the raw name
        // (an under-reported text), never a panic on the emission path.
        assert_eq!(
            registry_message("no_such_official_code"),
            "no_such_official_code"
        );
    }
}
