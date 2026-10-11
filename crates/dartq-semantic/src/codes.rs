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
    /// `The name '{0}' is already defined.` — the scope is per-file (the
    /// capture's duplicates log): a name duplicated across two files of one
    /// library (a part-file collision) is not detected yet. The per-library
    /// fold is a documented milestone deferral — an under-report, never a
    /// false positive.
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
    /// (`_visitLocalVariableElement`, unused_local_elements_verifier.dart:
    /// 1259-1280). Writes alone and statement-discarded reads do not count;
    /// `_`-wildcard names are exempt.
    UnusedLocalVariable,
    /// `The declaration '{0}' isn't referenced.` — a private local function,
    /// private top-level declaration, private container, or private class
    /// member with no library-level reference (`_visitLocalFunctionElement`
    /// /`_visitTopLevelFunctionElement`/`_visitClassElement`/
    /// `_visitMethodElement`/`_visitPropertyAccessorElement`/
    /// `_visitConstructorElement`, unused_local_elements_verifier.dart).
    UnusedElement,
    /// `The value of the field '{0}' isn't used.` — a private field no
    /// library-level read resolves to (`_visitFieldElement`, unused_local_
    /// elements_verifier.dart:1240-1247): writes never count, reads do.
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
    /// trace sibling and no body reference (unused_local_elements_verifier.
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
    /// label scope targets (`_withLabelTracker`, dead_code_verifier.dart:
    /// 163-176).
    UnusedLabel,
    // —— #506: the provably-check cluster (all `STATIC_WARNING`) ——————————
    /// `Unnecessary cast.` — an `e as T` whose operand type already equals
    /// `T` structurally, with neither side `dynamic` nor the target a bare
    /// `Function` (`_isUnnecessaryCast`, best_practices_verifier.dart:
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
    /// (`_checkForInvariantNullComparison`, best_practices_verifier.dart:
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
    /// type provably never equals (`_canBeEqual`, constant_verifier.dart:
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
    /// (`matchedTypeIsStrictlyNonNullable`, shared_type_analyzer.dart:
    /// 134-139).
    UnnecessaryNullCheckPattern,
    /// `The null-assert pattern will have no effect because the matched
    /// type isn't nullable.` — the `p!` variant of
    /// [`SemanticCode::UnnecessaryNullCheckPattern`]
    /// (shared_type_analyzer.dart:129-133).
    UnnecessaryNullAssertPattern,
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
            | Self::UnusedLocalVariable
            | Self::UnusedElement
            | Self::UnusedField
            | Self::UnusedElementParameter
            | Self::UnusedCatchClause
            | Self::UnusedCatchStack
            | Self::UnusedImport
            | Self::DeadNullAwareExpression
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
            | Self::OverrideOnNonOverridingSetter => DiagnosticType::StaticWarning,
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
            Self::UnusedImport => format!("Unused import: '{a0}'."),
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
            // The ambiguity pair (a0 = member name; a1 = one display for the
            // two-variant, the joined list for the three-or-more one).
            Self::AmbiguousExtensionMemberTwo | Self::AmbiguousExtensionMemberThreeOrMore => {
                self.message3(a0, a1, "")
            }
            // The no-argument representation rows route through `message3`.
            Self::ExtensionTypeDeclaresMemberOfObject
            | Self::ExtensionTypeRepresentationDependsOnItself => self.message3("", "", ""),
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
            Self::MemberWithClassName => {
                "A class member can't have the same name as the enclosing class.".to_string()
            }
            Self::ExpectedRepresentationType => "Expected a representation type.".to_string(),
            // No-argument flow row (diagnostic.g.dart:3546).
            Self::DeadCode => "Dead code.".to_string(),
            Self::WrongNumberOfTypeArgumentsConstructor
            | Self::WrongNumberOfTypeArgumentsEnum
            | Self::InvalidAssignment
            | Self::ArgumentTypeNotAssignable
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
    pub fn correction_message(self, a2: &str) -> Option<String> {
        match self {
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
            Self::UnusedElementParameter => {
                Some("Try removing the unused parameter.".to_string())
            }
            Self::UnusedCatchClause => Some("Try removing the catch clause.".to_string()),
            Self::UnusedCatchStack => {
                Some("Try removing the stack trace variable, or using it.".to_string())
            }
            Self::UnusedImport => Some("Try removing the import directive.".to_string()),
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
            // The #504 usage cluster and the #505 import/flow rows.
            SemanticCode::UnusedLocalVariable,
            SemanticCode::UnusedElement,
            SemanticCode::UnusedField,
            SemanticCode::UnusedElementParameter,
            SemanticCode::UnusedCatchClause,
            SemanticCode::UnusedCatchStack,
            SemanticCode::UnusedLabel,
            SemanticCode::UnusedImport,
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
        SemanticCode::DeadNullAwareExpression,
        SemanticCode::DeadCodeCatchFollowingCatch,
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
