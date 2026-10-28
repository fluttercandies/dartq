use super::{Check, NodeKind, RuleSpec, query_site};
use crate::rules::{class_declaration_name, initialized_identifier_name, utf16_token_span};

/// Official: `upstream Dart SDK source,
/// pkg/linter/lib/src/rules/unnecessary_this_alias.dart` — a local
/// variable initialized to `this` that (1) carries no `late`, (2) has
/// the variable's type equal to the `this` type, (3) is never
/// potentially mutated in the enclosing function body, and (4) has at
/// least one promotion usage in that body (`_ReferenceVisitor`
/// `_isPromotionUsage`) reports at the variable's name
/// (`reportAtToken(node.name)`). The usage faces: `is`/`as` operands,
/// `==`/`!=` against `null`, `!`, the null-aware member/index/cascade
/// receivers, `?..` cascade targets, `if (self case ...)`,
/// `switch (self)` statements and expressions — each read through
/// `_skipParenthesesUp`. Registered only with `this_promotion`
/// enabled; the 3.13 test stays silent.
///
/// dartq faces over the production tiling: the dispatch row is the
/// initialized identifier. The late gate reads the declaration
/// keyword; the type-equality gate compares a written type's head
/// name with the enclosing class-like container's name (a written
/// nullable never equals the non-null `this`; extensions read the
/// this-nullable `on` type through inference alone, so any written
/// type skips — documented under-report); the mutation gate is the
/// semantic `is_potentially_mutated_in_scope` over the declared name.
/// The usage scan walks the function body's subtree: candidates are
/// same-name identifiers bound to a local, paren-climbed up, and read
/// per parent face — `IsExpression`/`AsExpression`, the `==`/`!=`
/// null binary, `SwitchStatement`/`SwitchExpression` conditions, the
/// `if`-case face (`ParenthesizedCondition` parent plus a
/// `CaseExpression` child on the statement), a null-aware `?..`
/// cascade target — and per run adjacency — the null-aware `?.` dot
/// marker, the null-aware `?[` index, the `!` postfix, and the
/// `?..` cascade head. Shadowing rebinding is excluded: an innermost
/// declaring `Block` or function container between the use and the
/// declaration that does not bind this row skips the candidate.
/// Approximations (documented, under-report): the
/// `AnonymousMethodInvocation` face (the anonymous-methods
/// experiment) is not tiled into the scan, and `catch (self)`
/// parameters do not rebinder-exclude. Message: `diagnostic.g.dart`
/// `unnecessaryThisAlias`.
pub(crate) static SPEC: RuleSpec = crate::spec!(
    "unnecessary_this_alias",
    &[NodeKind::InitializedIdentifier],
    check,
);

#[expect(
    clippy::too_many_lines,
    reason = "one flat face ladder over the alias write's contexts;                   splitting it would scatter the official line citations"
)]
fn check(c: &mut Check<'_>) {
    let Some(analysis) = c.semantics() else {
        // The wave posture: no binding or mutation faces.
        return;
    };
    // The experimental-feature posture folds into the effective
    // version (the 3.13 test stays silent).
    if let Some(version) = c.unit.version
        && (version.major, version.minor) < (3, 14)
    {
        return;
    }
    let var = c.node();
    // `initializer?.unParenthesized is ThisExpression` — the
    // parenthesized spelling rides the wrapper row.
    let mut initializer = c
        .collect_direct(var)
        .into_iter()
        .find(|&kid| c.kind(kid) == NodeKind::VariableInitializer);
    let Some(first) = initializer
        .take()
        .and_then(|wrapper| c.collect_direct(wrapper).into_iter().next_back())
    else {
        return;
    };
    let mut expression = first;
    while c.kind(expression) == NodeKind::ParenthesizedExpression {
        let Some(inner) = c.collect_direct(expression).into_iter().next_back() else {
            return;
        };
        expression = inner;
    }
    if c.kind(expression) != NodeKind::ThisExpression {
        return;
    }
    let Some(name_row) = initialized_identifier_name(c, var) else {
        return;
    };
    // `parentList.isLate` — the declaration keyword reads `late`.
    let Some(decl_row) = c.unit.ast.parent(var) else {
        return;
    };
    // `late`: real parses store the token in payload word 0 (the
    // keyword reader answers var/final/const only); the name-anchored
    // local face recovers it as the token before the answered keyword.
    let stored = c
        .payload(decl_row)
        .first()
        .copied()
        .unwrap_or(crate::rules::NONE_TOKEN);
    let declares_late = (stored != crate::rules::NONE_TOKEN && c.lexeme(stored) == "late")
        || crate::rules::variables_declaration_keyword(c, decl_row).is_some_and(|keyword| {
            keyword
                .checked_sub(1)
                .is_some_and(|prev| c.lexeme(prev) == "late")
        });
    if c.kind(decl_row) == NodeKind::VariablesDeclaration && declares_late {
        return;
    }
    // `variableType != thisType` — a written type must spell the
    // enclosing class-like container's non-nullable name (`var`
    // infers the `this` type and passes).
    let Some(container) = c.path.iter().rev().copied().find(|&row| {
        matches!(
            c.kind(row),
            NodeKind::ClassDeclaration
                | NodeKind::MixinDeclaration
                | NodeKind::ExtensionDeclaration
                | NodeKind::ExtensionTypeDeclaration
                | NodeKind::EnumDeclaration
        )
    }) else {
        return;
    };
    // The written type: the binding face at the alias declaration —
    // the production tiling leaks the written `Type` row out of the
    // declaration's own window, so the `type_before` sibling read
    // cannot see it. `None` is the `var` shape — the inferred type is
    // the `this` type and the gate passes.
    // The use anchor is the initializer row: the alias's own
    // declaration anchors at the same token as the alias (`var`), and
    // the binding walk's use-position filter would exclude it.
    if let Some(face) = crate::rules::written_type_face(c, expression, c.lexeme(name_row)) {
        let matches = match c.kind(container) {
            // The `this` type of an extension is its (possibly
            // nullable) `on` type — inference-only here.
            NodeKind::ExtensionDeclaration => false,
            _ => face.name == class_declaration_name(c, container) && !face.nullable,
        };
        if !matches {
            return;
        }
    }
    // `functionBody` — the body row owned by the nearest function-like
    // container.
    let Some(body) = c
        .path
        .iter()
        .rev()
        .copied()
        .find(|&row| {
            matches!(
                c.unit.ast.parent(row).map(|parent| c.kind(parent)),
                Some(
                    NodeKind::Method
                        | NodeKind::Constructor
                        | NodeKind::Factory
                        | NodeKind::TopLevelMethod
                        | NodeKind::LocalFunctionDeclaration
                        | NodeKind::NamedFunctionExpression
                        | NodeKind::FunctionExpression
                )
            )
        })
        .filter(|&row| {
            matches!(
                c.kind(row),
                NodeKind::Block | NodeKind::BlockBody | NodeKind::ExpressionBody
            )
        })
    else {
        return;
    };
    // `isPotentiallyMutatedInScope(element)`.
    if analysis.is_potentially_mutated_in_scope(query_site(c, name_row)) {
        return;
    }
    let name = c.lexeme(name_row).to_owned();
    let decl_range = query_site(c, name_row);
    for row in c.children(body) {
        if c.kind(row) != NodeKind::Identifier
            || c.lexeme(c.head_token(row)) != name
            || row == name_row
        {
            continue;
        }
        // The same-element face: a local binding, unshadowed between
        // the use and this declaration.
        if !matches!(
            analysis.binding_of(query_site(c, row)).map(|b| b.kind),
            Some(dartq_semantic::BindingKind::Local)
        ) || shadowed(c, row, &name, decl_range)
        {
            continue;
        }
        if is_promotion_usage(c, row) {
            c.report(utf16_token_span(c, c.head_token(name_row)), &[]);
            return;
        }
    }
}

/// `_skipParenthesesUp`: the identifier's top row past the
/// parenthesized-expression wrappers.
fn paren_top(c: &Check<'_>, row: u32) -> u32 {
    let mut top = row;
    while let Some(parent) = c.unit.ast.parent(top)
        && c.kind(parent) == NodeKind::ParenthesizedExpression
    {
        top = parent;
    }
    top
}

/// Whether a local `name` rebinding sits between the use and the
/// declaration: the innermost scope on the path that binds the name —
/// a `Block`'s locals or a nested function container's parameters —
/// decides (it must be this declaration).
fn shadowed(
    c: &Check<'_>,
    use_row: u32,
    name: &str,
    decl_range: dartq_source::SourceRange,
) -> bool {
    let mut current = use_row;
    while let Some(parent) = c.unit.ast.parent(current) {
        match c.kind(parent) {
            NodeKind::Block => {
                if let Some(bound) = block_binds(c, parent, name) {
                    return query_site(c, bound) != decl_range;
                }
            }
            NodeKind::FunctionExpression | NodeKind::LocalFunctionDeclaration
                if declares_parameter(c, parent, name) =>
            {
                return true;
            }
            _ => {}
        }
        current = parent;
    }
    false
}

/// The `InitializedIdentifier` row of the block's direct local named
/// `name`, if any.
fn block_binds(c: &Check<'_>, block: u32, name: &str) -> Option<u32> {
    c.collect_direct(block)
        .into_iter()
        .filter(|&kid| c.kind(kid) == NodeKind::VariablesDeclaration)
        .flat_map(|decl| c.collect_direct(decl))
        .find(|&var| {
            c.kind(var) == NodeKind::InitializedIdentifier
                && initialized_identifier_name(c, var)
                    .is_some_and(|bound| c.lexeme(c.head_token(bound)) == name)
        })
        .and_then(|var| initialized_identifier_name(c, var))
}

/// Whether the function-like container binds a parameter `name`.
fn declares_parameter(c: &Check<'_>, owner: u32, name: &str) -> bool {
    c.collect_direct(owner).into_iter().any(|kid| {
        c.kind(kid) == NodeKind::FormalParameters
            && c.collect_direct(kid).into_iter().any(|param| {
                matches!(
                    c.kind(param),
                    NodeKind::FormalParameter | NodeKind::FunctionTypedFormalParameter
                ) && c.payload(param).get(1).copied().is_some_and(|name_token| {
                    name_token != crate::rules::NONE_TOKEN && c.lexeme(name_token) == name
                })
            })
    })
}

/// `_isPromotionUsage`: the parent faces over the paren-climbed row,
/// then the run-adjacency faces at the raw row.
fn is_promotion_usage(c: &Check<'_>, row: u32) -> bool {
    let top = paren_top(c, row);
    let parent = c.unit.ast.parent(top);
    if let Some(parent) = parent {
        match c.kind(parent) {
            NodeKind::IsExpression | NodeKind::AsExpression => return true,
            NodeKind::BinaryExpression => {
                return binary_null_test(c, parent, top);
            }
            NodeKind::Cascade => {
                // The cascade target: the row anchors at the `?..`/`..`
                // operator itself.
                return c.lexeme(c.head_token(parent)) == "?..";
            }
            NodeKind::ParenthesizedCondition => {
                if let Some(grandparent) = c.unit.ast.parent(parent) {
                    match c.kind(grandparent) {
                        // `switch (self)`.
                        NodeKind::SwitchStatement | NodeKind::SwitchExpression => {
                            return true;
                        }
                        // `if (self case ...)` — the case clause gate.
                        NodeKind::IfStatement => {
                            return c
                                .collect_direct(grandparent)
                                .iter()
                                .any(|&kid| c.kind(kid) == NodeKind::CaseExpression);
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    // The run-adjacency faces: childless containers leak the send
    // group's pieces into the enclosing run, so the pieces around the
    // (paren-climbed) receiver decide. The adopted operator container
    // (`self is C` / `self as C` / `self == null`) tiles beside the
    // receiver's run — over the operator and the right operand
    // (completion order seals it after its operands) — leaking past
    // the send-group markers, one container deep through the
    // parenthesized condition, and inside the paren-climbed top
    // itself (`(self as C)`). The first container whose token window
    // overlaps the receiver binds it; a later statement's rows never
    // overlap.
    if let Some(parent) = c.unit.ast.parent(top) {
        let siblings = c.collect_direct(parent);
        if let Some(position) = siblings.iter().position(|&kid| kid == top) {
            let operand = c.unit.ast.token_range(row);
            let judge = |op: u32| -> Option<bool> {
                let window = c.unit.ast.token_range(op);
                // One-token slack: the `?.` marker's row anchors past
                // the receiver's tail (the question mark rides the
                // payload, the row covers `.m()`).
                if !(window.start <= operand.end + 1 && operand.start <= window.end) {
                    return None;
                }
                match c.kind(op) {
                    NodeKind::IsExpression | NodeKind::AsExpression => Some(true),
                    NodeKind::BinaryExpression => Some(binary_null_test(c, op, row)),
                    // `self!` — the production bridge tiles the `!`
                    // postfix as its own kind.
                    NodeKind::NonNullAssert | NodeKind::UnaryPostfix => Some(true),
                    // `self?[0]` — the payload carries the `?`.
                    NodeKind::Indexed => Some(
                        c.payload(op)
                            .first()
                            .copied()
                            .is_some_and(|question| question != crate::rules::NONE_TOKEN),
                    ),
                    // `self?.m()` / `self?.x` — the marker's null-aware bit.
                    NodeKind::DotAccess => Some(c.payload(op).get(1).copied() == Some(1)),
                    _ => None,
                }
            };
            // `if (self case C _)` — the usage row is the if-case
            // scrutinee: hand-built trees tile a CaseExpression row;
            // the production tiling leaks the pattern rows instead, so
            // the `case` token directly after the receiver decides.
            let next_token = ((operand.end as usize) < c.unit.tokens.as_slice().len())
                .then(|| c.unit.tokens.lexeme(c.unit.source, operand.end as usize));
            if siblings
                .iter()
                .any(|&sib| c.kind(sib) == NodeKind::CaseExpression)
                || next_token == Some("case")
            {
                return true;
            }
            for &sib in &siblings[position + 1..] {
                if let Some(verdict) = judge(sib) {
                    return verdict;
                }
                // `if (self is C)` — the operator lives inside the
                // condition container the run leaks into.
                if c.kind(sib) == NodeKind::ParenthesizedCondition {
                    for kid in c.collect_direct(sib) {
                        if let Some(verdict) = judge(kid) {
                            return verdict;
                        }
                    }
                }
            }
            // `(self as C)` — the paren-climbed top wraps the usage;
            // the operator container trails inside its own window.
            if c.kind(top) == NodeKind::ParenthesizedExpression {
                for kid in c.collect_direct(top) {
                    if let Some(verdict) = judge(kid) {
                        return verdict;
                    }
                }
            }
        }
    }
    false
}

/// `self == null` / `null != self` / `(self) == null`: the operator is
/// `==`/`!=` and the other operand (unparenthesized) is `null`.
fn binary_null_test(c: &Check<'_>, binary: u32, operand: u32) -> bool {
    if !matches!(
        c.lexeme(c.payload(binary).first().copied().unwrap_or(0)),
        "==" | "!="
    ) {
        return false;
    }
    c.collect_direct(binary).into_iter().any(|kid| {
        kid != operand
            && match c.kind(kid) {
                NodeKind::LiteralNull => true,
                NodeKind::ParenthesizedExpression => c
                    .collect_direct(kid)
                    .into_iter()
                    .any(|inner| c.kind(inner) == NodeKind::LiteralNull),
                _ => false,
            }
    })
}
