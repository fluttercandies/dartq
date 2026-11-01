use super::constructor_name_part;
use super::{
    AUGMENT_MEMBER_BIT, Check, NodeKind, PayloadField, RuleSpec, formal_parameter_rows,
    member_declaration_name, same_class_field_type,
};
use crate::rules::{
    MemberQuery, NONE_TOKEN, body_members, get_set_token, member_matches, member_name,
    same_file_ancestors,
};
use dartq_ast::mod_flags;

/// Official (pinned 3.13.7):
/// `pkg/linter/lib/src/rules/avoid_positional_boolean_parameters.dart`
/// — the FIRST positional parameter whose type is `bool` reports at the
/// parameter (:46-50, `firstWhereOrNull(_isBoolean)` +
/// `reportAtNode(parameterToLint)`). Public declarations only
/// (:51/:60/:80/:91); setters skip (a getter's null parameter list is
/// inert, :75-88); operators skip; a fixed signature (an augmentation or
/// an override, :79 + `_isOverridingMember` :85-97 + the extension
/// :99-108) skips. A local function rides the pinned
/// `visitFunctionDeclaration` too (the analyzer AST nests the
/// `FunctionDeclaration` inside its statement node), and
/// `visitGenericFunctionType` (:71-73) has NO fixed-signature walk (a
/// `Function(bool)` always checks). The augmentation skip keeps the
/// ORIGINAL declaration of an augmented member lintable — an augmenting
/// part's `augment A(bool b)` stays silent while the plain `A(bool b)`
/// it augments reports. Message: `diagnostic.g.dart`
/// `avoidPositionalBooleanParameters`.
///
/// Approximations: the resolved parameter type folds to the written
/// type spelling — exactly `bool` (an import-prefixed or aliased
/// spelling under-reports); a `this.f` field formal reads the
/// same-class field's written type. The override face folds to the
/// same-file any-member ancestor scan — `implements` interfaces join the
/// chain, the `getInheritedMember` cut (cross-file overrides
/// under-report); augmentations read the member `ModSet` augment bit.
pub(crate) static SPEC: RuleSpec = crate::spec!(
    "avoid_positional_boolean_parameters",
    &[
        NodeKind::Constructor,
        NodeKind::TopLevelMethod,
        NodeKind::Method,
        NodeKind::LocalFunctionDeclaration,
        NodeKind::PrimaryConstructor,
        NodeKind::FunctionType,
    ],
    check,
);

fn check(c: &mut Check<'_>) {
    let node = c.node();
    match c.kind(node) {
        NodeKind::PrimaryConstructor => {
            if mod_flags(c.unit.ast, node) & AUGMENT_MEMBER_BIT != 0 {
                return; // (:90)
            }
            if primary_is_private(c, node) {
                return; // (:93)
            }
            check_params(c, node, class_like_row(c));
        }
        NodeKind::FunctionType => {
            // The pinned `visitGenericFunctionType` (:71-73) checks
            // unconditionally — no fixed-signature walk upstream.
            check_params(c, node, None);
        }
        NodeKind::Constructor
        | NodeKind::TopLevelMethod
        | NodeKind::Method
        | NodeKind::LocalFunctionDeclaration => {
            if mod_flags(c.unit.ast, node) & AUGMENT_MEMBER_BIT != 0 {
                return; // (:48/:59/:81)
            }
            if c.kind(node) == NodeKind::Method {
                if get_set_token(c, node).is_some() {
                    return; // getters/setters (:76-77)
                }
                // Operators skip (:78) — the `OperatorName` tiles as the
                // method row's LEFT SIBLING (bridge `handleOperatorName`
                // precedes `beginMethod`), never as a child.
                if let Some(parent) = c.unit.ast.parent(node) {
                    let siblings = c.collect_direct(parent);
                    if let Some(pos) = siblings.iter().position(|&kid| kid == node) {
                        if siblings[..pos]
                            .iter()
                            .rev()
                            .any(|&kid| c.kind(kid) == NodeKind::OperatorName)
                        {
                            return;
                        }
                    }
                }
                if fixed_signature(c, node) {
                    return; // (:79)
                }
            }
            if is_private(c, node) {
                return; // (:51/:62/:82)
            }
            check_params(c, node, class_like_row(c));
        }
        _ => {}
    }
}

/// `checkParams` (:50-54): the FIRST positional `bool` parameter reports
/// — `firstWhereOrNull`, one report per declaration.
fn check_params(c: &mut Check<'_>, owner: u32, class_row: Option<u32>) {
    for param in formal_parameter_rows(c, owner) {
        if positional(c, param) && bool_typed(c, param, class_row) {
            c.report(param_span(c, param), &[]);
            return;
        }
    }
}

/// The parameter's extent as the official node spans it: through the
/// default value's last token. The row seals `end_incl` on
/// `InitializerEnd`, which the parser sets to the peek AFTER the default
/// expression (exclusive bound), so a defaulted parameter over-covers by
/// the trailing `,`/`]`/`)` token — trim back to `InitializerEnd - 1`.
fn param_span(c: &Check<'_>, param: u32) -> core::ops::Range<u32> {
    let full = c.utf16_span(param);
    match c
        .field(param, PayloadField::InitializerEnd)
        .filter(|&end| end != NONE_TOKEN)
        .and_then(|end| c.utf16_end_of_token(end - 1))
    {
        Some(end) if end > full.start => full.start..end,
        _ => full,
    }
}

/// `p.isPositional` — the required-positional and optional-positional
/// parameter kinds (the row's `FormalParameterKind` word, bridge
/// `end_formal_parameter`).
fn positional(c: &Check<'_>, param: u32) -> bool {
    matches!(
        c.field(param, PayloadField::FormalParameterKind),
        Some(kind)
            if kind == dartq_parser::FormalParameterKind::RequiredPositional as u32
                || kind == dartq_parser::FormalParameterKind::OptionalPositional as u32
    )
}

/// The `bool` face of the parameter's type: the written type slice
/// answers; a `this.f` field formal reads the same-class field's
/// written type (the element's declared-type face).
fn bool_typed(c: &Check<'_>, param: u32, class_row: Option<u32>) -> bool {
    let written = c.collect_direct(param).into_iter().find(|&kid| {
        matches!(
            c.kind(kid),
            NodeKind::Type | NodeKind::FunctionType | NodeKind::RecordType
        )
    });
    if let Some(t) = written {
        return &c.unit.source[c.byte_span(t)] == "bool";
    }
    let receiver = c.field(param, PayloadField::Receiver).unwrap_or(NONE_TOKEN);
    if receiver == NONE_TOKEN || c.lexeme(receiver) != "this" {
        return false;
    }
    let Some(name) = c.field(param, PayloadField::Name) else {
        return false;
    };
    if name == NONE_TOKEN {
        return false;
    }
    let Some(class_row) = class_row else {
        return false;
    };
    same_class_field_type(c, class_row, c.lexeme(name))
        .is_some_and(|t| &c.unit.source[c.byte_span(t)] == "bool")
}

/// The written-name privacy face (:51/:62/:82): a constructor's name
/// part, else the declaration name, starting with `_`. A local
/// function's name nests under `FunctionName` — read through
/// [`member_declaration_name`].
fn is_private(c: &Check<'_>, owner: u32) -> bool {
    let name = match c.kind(owner) {
        NodeKind::Constructor => constructor_name_part(c, owner),
        _ => member_declaration_name(c, owner),
    };
    name.is_some_and(|name| c.lexeme(c.head_token(name)).starts_with('_'))
}

/// The named-section privacy face (:93): a
/// `PrimaryConstructorDeclaration`-context name starting with `_`.
fn primary_is_private(c: &Check<'_>, primary: u32) -> bool {
    let ctx = dartq_parser::IdentifierContext::PrimaryConstructorDeclaration as u32;
    c.collect_direct(primary).into_iter().any(|kid| {
        c.kind(kid) == NodeKind::Identifier
            && c.field(kid, PayloadField::Context) == Some(ctx)
            && c.lexeme(c.head_token(kid)).starts_with('_')
    })
}

/// `hasFixedSignature` (:129-138): an augmentation, or a method a
/// same-file supertype declares under the ANY-member cut
/// (`getInheritedMember` — `implements` interfaces join the extends/with
/// chain, `extensions.dart` :574-597; cross-file supertypes
/// under-report).
fn fixed_signature(c: &Check<'_>, method: u32) -> bool {
    if mod_flags(c.unit.ast, method) & AUGMENT_MEMBER_BIT != 0 {
        return true;
    }
    let Some(name) = member_name(c, method) else {
        return false;
    };
    let name = c.lexeme(c.head_token(name));
    let Some(class_row) = class_like_row(c) else {
        return false;
    };
    same_file_ancestors(c, class_row, false)
        .iter()
        .any(|&ancestor| {
            body_members(c, ancestor)
                .iter()
                .any(|&member| !member_matches(c, member, MemberQuery::Method, name).is_empty())
        })
}

/// The owning class-like row (the field-formal reads need it).
fn class_like_row(c: &Check<'_>) -> Option<u32> {
    c.path[..c.path.len() - 1].iter().rev().copied().find(|&a| {
        matches!(
            c.kind(a),
            NodeKind::ClassDeclaration | NodeKind::EnumDeclaration | NodeKind::MixinDeclaration
        )
    })
}

/// The #3308 oracle-corpus pins: every single-file upstream case (the
/// exact materialized conformance sources) with the oracle's own-row
/// spans; the three cross-file augmentation faces are residuals per the
/// test's doc.
#[cfg(test)]
mod tests {
    use crate::testsupport::{Fixture, one_rule};

    /// The whole upstream oracle corpus — `AvoidPositionalBooleanParametersTest`
    /// (29 cases, `pkg/linter/test/rules/avoid_positional_boolean_parameters_test.dart`)
    /// plus the two `messages.yaml` doc-bad and two doc-good cases: 33 total,
    /// `conformance/status.tsv` `lint avoid_positional_boolean_parameters 33 17 16`.
    /// Each entry is the exact materialized case source and the oracle's
    /// `(line, column, length)` for every rule row; only the rule's own rows
    /// are pinned (co-reported `unused_element` faces are other diagnostics).
    ///
    /// 30 single-file cases pinned (13 oracle rows), every mismatch collected before the single panic.
    /// Cross-file residuals not representable in a single-source fixture (oracle rows on `lib/a.dart`): test_augmentationConstructor#0, test_augmentationFunction#0, test_augmentationMethod#0.
    #[test]
    fn avoid_positional_boolean_parameters_oracle_corpus_pins() {
        // (case, source, oracle rows as (line, column, length)).
        let cases: &[(&str, &str, &[(u32, u32, u32)])] = &[
            (
                "test_anonymousFunction#0",
                "\nvoid f(List<bool> list) {\n  list.where((bool e) => e);\n}",
                &[],
            ),
            (
                "test_constructorPrivate_positionalOptional#0",
                "\nclass C {\n  // ignore: unused_element_parameter\n  C._named([bool p = false]);\n}",
                &[],
            ),
            (
                "test_constructor_fieldFormalParameter_named#0",
                "\nclass C {\n  bool p;\n  C.named({this.p = false});\n}",
                &[],
            ),
            (
                "test_constructor_fieldFormalParameter_positional#0",
                "\nclass C {\n  bool p;\n  C.named(this.p);\n}",
                &[(4, 11, 6)],
            ),
            (
                "test_constructor_named_withDefault#0",
                "\nclass C {\n  C.named({bool p = false}) {\n  }\n}",
                &[],
            ),
            (
                "test_constructor_positional#0",
                "\nclass C {\n  C.named(bool a) {\n  }\n}",
                &[(3, 11, 6)],
            ),
            (
                "test_constructor_primary_declaringParameter#0",
                "\nclass C(final bool p);",
                &[(2, 9, 12)],
            ),
            (
                "test_extensionMethod#0",
                "\nextension Ext on int {\n  void f([bool p = false]) {}\n}",
                &[(3, 11, 14)],
            ),
            (
                "test_extensionMethod_unnamed#0",
                "\nextension on int {\n  // ignore: unused_element, unused_element_parameter\n  void f([bool p = false]) {}\n}",
                &[(4, 11, 14)],
            ),
            (
                "test_instanceMethod_named#0",
                "\nclass C {\n  void m({bool p = true}) {}\n}",
                &[],
            ),
            (
                "test_instanceMethod_overrideExtends_positionalOptional#0",
                "\nclass C {\n  void m([bool p = false]) {}\n}\nclass D extends C {\n  @override\n  void m([bool p = false]) {}\n}",
                &[(3, 11, 14)],
            ),
            (
                "test_instanceMethod_overrideImplements_positionalOptional#0",
                "\nclass C {\n  void m([bool p = false]) {}\n}\nabstract class D implements C {\n  @override\n  void m([bool p = false]) {}\n}",
                &[(3, 11, 14)],
            ),
            (
                "test_instanceMethod_positional#0",
                "\nclass A {\n  void m(bool p) {}\n}",
                &[(3, 10, 6)],
            ),
            (
                "test_instanceMethod_positionalOptional#0",
                "\nclass C {\n  void m([bool p = false]) {}\n}",
                &[(3, 11, 14)],
            ),
            (
                "test_instanceSetter#0",
                "\nclass C {\n  set m(bool p) {}\n}",
                &[],
            ),
            (
                "test_operator_indexAssignment#0",
                "\nclass C {\n  void operator []=(int index, bool value) {}\n}",
                &[],
            ),
            (
                "test_operator_minus#0",
                "\nclass C {\n  void operator -(bool value) {}\n}",
                &[],
            ),
            (
                "test_operator_plus#0",
                "\nclass C {\n  void operator +(bool value) {\n  }\n}",
                &[],
            ),
            (
                "test_staticMethod_namedWithDefault#0",
                "\nclass B {\n  static void m({bool p = false}) {}\n}",
                &[],
            ),
            (
                "test_staticMethod_positional#0",
                "\nclass B {\n  static void m(bool p) {}\n}",
                &[(3, 17, 6)],
            ),
            (
                "test_topLevelPrivate#0",
                "\n// ignore: unused_element\nvoid _f(bool p) {}",
                &[],
            ),
            (
                "test_topLevel_namedParameter#0",
                "\nvoid f({bool p = false}) {}",
                &[],
            ),
            (
                "test_topLevel_namedParameter_defaultValue#0",
                "\nvoid f({bool p = false}) {}",
                &[],
            ),
            (
                "test_topLevel_positionalParameter#0",
                "\nvoid f(bool p) {}",
                &[(2, 8, 6)],
            ),
            (
                "test_typedef_named#0",
                "\ntypedef T = Function({bool p});",
                &[],
            ),
            (
                "test_typedef_positional#0",
                "\ntypedef T = Function(bool p);",
                &[(2, 22, 6)],
            ),
            (
                "doc_bad_0",
                "void f(bool b) {}",
                &[(1, 8, 6)],
            ),
            (
                "doc_bad_1",
                "Task(true);\nTask(false);\nListBox(false, true, true);\nButton(false);",
                &[],
            ),
            (
                "doc_good_0",
                "void f({bool b = false}) {}",
                &[],
            ),
            (
                "doc_good_1",
                "Task.oneShot();\nTask.repeating();\nListBox(scroll: true, showScrollbars: true);\nButton(ButtonState.enabled);",
                &[],
            ),
        ];
        // The UTF-16 offset of a 1-based `(line, column)` in an ASCII source.
        fn offset(source: &str, line: u32, column: u32) -> u32 {
            let line_start: usize = source
                .split_inclusive('\n')
                .take(line as usize - 1)
                .map(str::len)
                .sum();
            (line_start + column as usize - 1) as u32
        }
        let mut mismatches = Vec::new();
        for (case, source, oracle) in cases {
            let f = Fixture::parse(source);
            let mut lints = f.run(&one_rule("avoid_positional_boolean_parameters"));
            lints.sort_by_key(|lint| lint.range.start);
            let got: Vec<(u32, u32)> = lints
                .iter()
                .map(|lint| (lint.range.start, lint.range.end))
                .collect();
            let mut want: Vec<(u32, u32)> = oracle
                .iter()
                .map(|&(line, column, length)| {
                    let start = offset(source, line, column);
                    (start, start + length)
                })
                .collect();
            want.sort_unstable();
            if got != want {
                mismatches.push(format!("{case}: want {want:?} got {got:?}"));
            }
        }
        assert!(mismatches.is_empty(), "{}", mismatches.join("\n"));
    }}
