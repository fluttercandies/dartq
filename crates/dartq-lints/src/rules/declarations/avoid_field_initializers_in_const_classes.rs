use super::{
    AUGMENT_MEMBER_BIT, Check, NodeKind, PayloadField, RuleSpec, VAR_FINAL_OR_CONST_MEMBER_BIT,
    formal_parameter_rows,
};
use crate::rules::{NONE_TOKEN, body_members, variables_declaration_keyword};
use dartq_ast::mod_flags;

/// Official:
/// `pkg/linter/lib/src/rules/avoid_field_initializers_in_const_classes.dart`
/// — in a const class, a final instance field with an initializer
/// reports at the variable (:87-118), and a const constructor's field
/// initializer reports at the initializer (:60-84 — the "no lint if
/// several constructors" guard :75). Message: `diagnostic.g.dart`
/// `avoidFieldInitializersInConstClasses`.
///
/// Approximations: the constructor-element const/count checks read the
/// written rows (the const `ModSet` bit, the primary's payload const
/// keyword); a parameter reference folds to a token scan over the
/// initializer expression against the constructor's (or the paired
/// primary's) parameter names — a same-named non-parameter shadows
/// toward silence (under-reports).
pub(crate) static SPEC: RuleSpec = crate::spec!(
    "avoid_field_initializers_in_const_classes",
    &[NodeKind::Fields, NodeKind::Initializer],
    check,
);

fn check(c: &mut Check<'_>) {
    match c.kind(c.node()) {
        NodeKind::Fields => check_fields(c),
        // The `ConstructorFieldInitializer` face is the `Initializer`
        // row whose last child is the `= expr` assignment —
        // `super(…)`/`assert(…)`/`this(…)` initializers tile call
        // chains instead.
        NodeKind::Initializer => check_initializer(c),
        _ => {}
    }
}

/// The field-declaration face (:87-118).
fn check_fields(c: &mut Check<'_>) {
    let node = c.node();
    if mod_flags(c.unit.ast, node) & AUGMENT_MEMBER_BIT != 0 {
        return; // `node.isAugmentation` (:88)
    }
    // `node.isStatic` (:89) — the `Fields` row's `MemberKind` word is
    // the discriminant.
    if c.field(node, PayloadField::MemberKind) == Some(dartq_parser::MemberKind::StaticField as u32)
    {
        return;
    }
    // The face requires a class owner (:93).
    let Some(class_row) = class_declaration_row(c) else {
        return;
    };
    // "only const class" (:91-99): a class without any const
    // constructor skips.
    if !class_has_const_constructor(c, class_row) {
        return;
    }
    // Only the parameters of a primary constructor are in scope in a
    // field initializer (:100-115).
    let primary_params: Vec<String> = c
        .collect_direct(class_row)
        .into_iter()
        .find(|&member| c.kind(member) == NodeKind::PrimaryConstructor)
        .map(|primary| {
            formal_parameter_rows(c, primary)
                .into_iter()
                .filter_map(|row| c.field(row, PayloadField::Name))
                .filter(|&token| token != NONE_TOKEN)
                .map(|token| str::to_owned(c.lexeme(token)))
                .collect()
        })
        .unwrap_or_default();
    for decl in c.collect_direct(node) {
        if c.kind(decl) != NodeKind::VariablesDeclaration {
            continue;
        }
        // `node.fields.isFinal` (:90) — per declaration list.
        if variables_declaration_keyword(c, decl).is_none_or(|kw| c.lexeme(kw) != "final") {
            continue;
        }
        for var in c.collect_direct(decl) {
            if c.kind(var) != NodeKind::InitializedIdentifier || !has_initializer(c, var) {
                continue; // `initializer == null` (:108)
            }
            if initializer_references(c, var, &primary_params) {
                continue; // (:110-115)
            }
            c.report(c.utf16_span(var), &[]); // reportAtNode(variable)
        }
    }
}

/// The constructor-field-initializer face (:60-84).
fn check_initializer(c: &mut Check<'_>) {
    let node = c.node();
    // `a = expr` / `this.a = expr` only — a `super(…)`, `assert(…)` or
    // redirecting `this(…)` initializer ends in a call send, not an
    // `Assignment` row.
    if c.collect_direct(node)
        .last()
        .is_none_or(|&last| c.kind(last) != NodeKind::Assignment)
    {
        return;
    }
    // `node.parent` (:61) — the initializer wrapper's owner row: the
    // secondary constructor or the primary constructor body.
    let Some(owner) = c.ancestor(2) else {
        return;
    };
    let Some(class_row) = class_declaration_row(c) else {
        return; // `classElement is! ClassElement` (:72)
    };
    let (is_const, params) = match c.kind(owner) {
        NodeKind::Constructor => (
            mod_flags(c.unit.ast, owner) & VAR_FINAL_OR_CONST_MEMBER_BIT != 0,
            formal_parameter_rows(c, owner),
        ),
        NodeKind::PrimaryConstructorBody => {
            // `declaration.declaredFragment?.element` (:63-65) — the
            // paired primary carries the const keyword (payload word
            // 1) and the parameters.
            let Some(primary) = c
                .collect_direct(class_row)
                .into_iter()
                .find(|&member| c.kind(member) == NodeKind::PrimaryConstructor)
            else {
                return;
            };
            (
                c.field(primary, PayloadField::ConstKeyword)
                    .is_some_and(|token| token != NONE_TOKEN),
                formal_parameter_rows(c, primary),
            )
        }
        _ => return,
    };
    if !is_const {
        return; // (:69)
    }
    // "No lint if several constructors" (:75).
    if constructor_count(c, class_row) > 1 {
        return;
    }
    let names: Vec<&str> = params
        .into_iter()
        .filter_map(|row| c.field(row, PayloadField::Name))
        .filter(|&token| token != NONE_TOKEN)
        .map(|token| c.lexeme(token))
        .collect();
    // A parameter reference in the initializer expression skips
    // (:77-83).
    if initializer_references(c, node, &names) {
        return;
    }
    c.report(c.utf16_span(node), &[]); // reportAtNode(node)
}

/// Whether the class carries at least one const constructor: written
/// constructor rows carry the const `ModSet` bit, the primary
/// constructor its payload const keyword.
fn class_has_const_constructor(c: &Check<'_>, class_row: u32) -> bool {
    for member in body_members(c, class_row) {
        if matches!(c.kind(member), NodeKind::Constructor | NodeKind::Factory)
            && mod_flags(c.unit.ast, member) & VAR_FINAL_OR_CONST_MEMBER_BIT != 0
        {
            return true;
        }
    }
    c.collect_direct(class_row).into_iter().any(|kid| {
        c.kind(kid) == NodeKind::PrimaryConstructor
            && c.field(kid, PayloadField::ConstKeyword)
                .is_some_and(|token| token != NONE_TOKEN)
    })
}

/// The written constructor rows of the class: secondaries, factories,
/// and the primary (the `classElement.constructors` face).
fn constructor_count(c: &Check<'_>, class_row: u32) -> usize {
    let mut count = body_members(c, class_row)
        .into_iter()
        .filter(|member| matches!(c.kind(*member), NodeKind::Constructor | NodeKind::Factory))
        .count();
    if c.collect_direct(class_row)
        .iter()
        .any(|&kid| c.kind(kid) == NodeKind::PrimaryConstructor)
    {
        count += 1;
    }
    count
}

/// Whether the initializer's `= expr` tail references one of the
/// parameter names (the `HasParameterReferenceVisitor`
/// `visitSimpleIdentifier` face over the expression tokens).
fn initializer_references<S: AsRef<str>>(c: &Check<'_>, row: u32, params: &[S]) -> bool {
    if params.is_empty() {
        return false;
    }
    let range = c.unit.ast.token_range(row);
    let Some(eq) = range.clone().find(|&token| c.lexeme(token) == "=") else {
        return false;
    };
    (eq + 1..range.end).any(|token| params.iter().any(|param| param.as_ref() == c.lexeme(token)))
}

/// Whether the variable row carries a `VariableInitializer` child (the
/// diagnostic collapses to plain absence).
fn has_initializer(c: &Check<'_>, var: u32) -> bool {
    c.children(var)
        .any(|descendant| c.kind(descendant) == NodeKind::VariableInitializer)
}

/// The owning `ClassDeclaration` row — the walk's first class-like
/// container must be a class (mixins, enums, and extension types are
/// out of face, :72/:93).
fn class_declaration_row(c: &Check<'_>) -> Option<u32> {
    let row = c.path[..c.path.len() - 1]
        .iter()
        .rev()
        .copied()
        .find(|&a| {
            matches!(
                c.kind(a),
                NodeKind::ClassDeclaration
                    | NodeKind::EnumDeclaration
                    | NodeKind::MixinDeclaration
                    | NodeKind::ExtensionTypeDeclaration
            )
        })?;
    (c.kind(row) == NodeKind::ClassDeclaration).then_some(row)
}
