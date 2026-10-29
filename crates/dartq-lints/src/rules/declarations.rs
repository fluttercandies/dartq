//! The declaration-shape family: `avoid_return_types_on_setters`,
//! `unnecessary_late`, `use_function_type_syntax_for_parameters`,
//! `prefer_typing_uninitialized_variables`,
//! `prefer_generic_function_type_aliases`, `unnecessary_constructor_name`,
//! `hash_and_equals`, `avoid_init_to_null`,
//! `prefer_function_declarations_over_variables`, `type_init_formals`,
//! `avoid_equals_and_hash_code_on_mutable_classes`,
//! `avoid_setters_without_getters`, `deprecated_consistency`,
//! `omit_local_variable_types`, `unnecessary_async`,
//! `use_setters_to_change_properties`, `use_to_and_as_if_applicable`,
//! `annotate_redeclares`, `avoid_implementing_value_types`,
//! `omit_obvious_local_variable_types`, `omit_obvious_property_types`,
//! `prefer_const_constructors_in_immutables`, `prefer_final_in_for_each`,
//! `prefer_final_locals`, `specify_nonobvious_local_variable_types`,
//! `specify_nonobvious_property_types`,
//! `use_late_for_private_fields_and_variables`,
//! `prefer_const_declarations`, `prefer_mixin`, `use_enums`,
//! `deprecated_member_use_from_same_package`,
//! `remove_deprecations_in_breaking_versions`.

use dartq_ast::{NodeKind, PayloadField};
use rustc_hash::FxHashMap;

use crate::engine::Check;
use crate::engine::narrow_u32;
use crate::registry::RuleSpec;
use crate::rules::{NONE_TOKEN, SupertypeClause, body_members, member_name, slice, type_before};

/// The domain's incoming registrations: a new standalone rule appends its
/// `SPEC` reference here (name order) and joins the registry through the
/// aggregated tail — adding a rule touches only this file. Batch close
/// promotes the block into the registry's frozen skeleton together with
/// the pin-test wave list (`tests/registry_pin.rs`). The slice is empty
/// since the W6 closeout (#1213): the #1157 wave's L1 internal block and
/// the L2 `implicit_reopen` face — and the closeout's own
/// `unreachable_from_main` — were promoted into the frozen skeleton in
/// that one edit.
pub(crate) static INCOMING_RULES: &[&RuleSpec] = &[];

// —— shared helpers ———————————————————————————————————————————————————————

/// The declaration-name row of a member: the direct `Identifier` child
/// carrying an official name context (`MethodDeclaration` /
/// `TopLevelFunctionDeclaration` / `LocalFunctionDeclaration` — bridge
/// `handle_identifier` stamps it), falling back to the `Identifier` inside
/// a `FunctionName` sub-row, then to the first name-shaped child.
/// Production parses tile the written return type's `Identifier` row into
/// the member window before the name (`handle_type` emits the type leaf
/// after the identifier row), so the first-identifier read alone answers
/// the type on typed members; the context disambiguates. A
/// local/named function nests its stamped name under the `FunctionName`
/// child, which the second arm reaches. Hand-built tables stamp the name
/// as the first name-shaped child with no name context — the last
/// fallback keeps them answered.
pub(crate) fn member_declaration_name(c: &Check<'_>, member: u32) -> Option<u32> {
    let kids = c.collect_direct(member);
    kids.iter()
        .copied()
        .find(|&kid| {
            c.kind(kid) == NodeKind::Identifier
                && matches!(
                    c.field(kid, PayloadField::Context),
                    Some(ctx)
                        if ctx == dartq_parser::IdentifierContext::MethodDeclaration as u32
                            || ctx
                                == dartq_parser::IdentifierContext::TopLevelFunctionDeclaration
                                    as u32
                            || ctx
                                == dartq_parser::IdentifierContext::LocalFunctionDeclaration
                                    as u32
                )
        })
        .or_else(|| {
            kids.iter()
                .copied()
                .find(|&kid| c.kind(kid) == NodeKind::FunctionName)
                .and_then(|wrapper| {
                    c.collect_direct(wrapper)
                        .into_iter()
                        .find(|&id| c.kind(id) == NodeKind::Identifier)
                })
        })
        .or_else(|| {
            kids.into_iter()
                .find(|&kid| matches!(c.kind(kid), NodeKind::Identifier | NodeKind::OperatorName))
        })
}

/// The constructed type of an instance-creation root: the
/// `ConstructorReference` container's bare head identifier — the span the
/// fold's type-use row rides (`C()`, `new C.named()`, `C<int>()`). A
/// prefixed head publishes no type-use row and stays silent.
pub(crate) fn constructed_type(
    c: &Check<'_>,
    analysis: &dartq_semantic::Analysis<'_>,
    creation: u32,
) -> Option<dartq_semantic::model::Ty> {
    let ctor_ref = c
        .collect_direct(creation)
        .into_iter()
        .find(|&kid| c.kind(kid) == NodeKind::ConstructorReference)?;
    let head = crate::rules::first_child(c, ctor_ref)?;
    if c.kind(head) != NodeKind::Identifier {
        return None;
    }
    analysis.creation_type(crate::rules::expressions::query_site(c, head))
}

/// The operator name of a `Method`: a direct `OperatorName` child
/// (hand-built tables) or the row immediately preceding the member among
/// its siblings ([`adjacent_operator_name`] — the production wrapper).
pub(crate) fn operator_name(c: &Check<'_>, member: u32) -> Option<u32> {
    if let Some(child) = c
        .collect_direct(member)
        .into_iter()
        .find(|&kid| c.kind(kid) == NodeKind::OperatorName)
    {
        return Some(child);
    }
    adjacent_operator_name(c, member)
}

/// The `OperatorName` row immediately preceding `member` among its
/// siblings (the production wrapper — the name completes right before the
/// member row opens). Immediate adjacency only: a named member following
/// an operator must not adopt the neighbor's operator name.
fn adjacent_operator_name(c: &Check<'_>, member: u32) -> Option<u32> {
    let parent = c.unit.ast.parent(member)?;
    let siblings = c.collect_direct(parent);
    let pos = siblings.iter().position(|&kid| kid == member)?;
    let prev = siblings.get(pos.checked_sub(1)?)?;
    (c.kind(*prev) == NodeKind::OperatorName).then_some(*prev)
}

// —— shadow-check face ——————————————————————————————————————————————————
//
// Shared by `recursive_getters` (read-vs-getter identity) and
// `unnecessary_getters_setters` (assignment-target-vs-field identity): the
// official checks are element identities, so a same-lexeme read or target
// captured by a nearer binding is a different element and must not report.
// The binders the walk honors: local variables, local/named functions (name
// and parameters), catch parameters, for-loop variables, switch-case
// pattern variables, and the enclosing member's own parameters.

/// Whether `stmt` declares a binding of `name` at its own level (a block
/// sibling, or the initializer of a `for` parts list).
fn statement_binds(c: &Check<'_>, stmt: u32, name: &str) -> bool {
    match c.kind(stmt) {
        NodeKind::VariablesDeclaration => c.collect_direct(stmt).into_iter().any(|decl| {
            c.kind(decl) == NodeKind::InitializedIdentifier
                && crate::rules::initialized_identifier_name(c, decl)
                    // Hand-built trees anchor the row at the `var` keyword
                    // (the head answers `var`, not the name) and carry the
                    // name as an `Identifier` child — read it there.
                    .or_else(|| {
                        c.collect_direct(decl)
                            .into_iter()
                            .find(|&kid| c.kind(kid) == NodeKind::Identifier)
                            .map(|kid| c.head_token(kid))
                    })
                    .is_some_and(|n| c.lexeme(n) == name)
        }),
        NodeKind::LocalFunctionDeclaration | NodeKind::NamedFunctionExpression => {
            function_name_binds(c, stmt, name)
        }
        NodeKind::PatternVarDeclStatement => pattern_binds(c, stmt, name),
        NodeKind::LabeledStatement => c
            .collect_direct(stmt)
            .into_iter()
            .any(|kid| c.kind(kid) != NodeKind::Label && statement_binds(c, kid, name)),
        _ => false,
    }
}

/// The name a local/named function binds (the `LocalFunctionDeclaration`-
/// stamped `Identifier`, reached through the `FunctionName` wrapper by
/// [`member_declaration_name`]).
fn function_name_binds(c: &Check<'_>, node: u32, name: &str) -> bool {
    member_declaration_name(c, node).is_some_and(|n| c.lexeme(c.head_token(n)) == name)
}

/// Whether a pattern subtree declares `name` (`DeclaredVariablePattern`
/// payload word 0 carries the variable token).
fn pattern_binds(c: &Check<'_>, pattern: u32, name: &str) -> bool {
    c.children(pattern).any(|descendant| {
        c.kind(descendant) == NodeKind::DeclaredVariablePattern
            && c
                .field(descendant, PayloadField::Variable)
                .is_some_and(|v| v != NONE_TOKEN && c.lexeme(v) == name)
    })
}

/// Whether a formal-parameter list declares `name` (plain and
/// function-typed formals, through the shared [`parameter_name`]).
fn parameters_bind(c: &Check<'_>, params: u32, name: &str) -> bool {
    c.collect_direct(params)
        .into_iter()
        .any(|param| parameter_name(c, param) == Some(name))
}

/// The for-in loop variable binds inside the loop: the bare form is an
/// `Identifier` child completing before the parts' `in` token, the `var x`
/// form a `DeclaredVariablePattern` child (the parts node is a childless
/// `ForInLoopPartsHandle`).
fn for_in_binds(c: &Check<'_>, scope: u32, name: &str) -> bool {
    let kids = c.collect_direct(scope);
    let Some(in_token) = kids
        .iter()
        .copied()
        .find(|&kid| c.kind(kid) == NodeKind::ForInParts)
        .and_then(|parts| c.field(parts, PayloadField::InKeyword))
        .filter(|&token| token != NONE_TOKEN)
    else {
        return false;
    };
    kids.into_iter().any(|kid| match c.kind(kid) {
        NodeKind::DeclaredVariablePattern => c
            .field(kid, PayloadField::Variable)
            .is_some_and(|v| v < in_token && c.lexeme(v) == name),
        NodeKind::Identifier => {
            c.unit.ast.token_range(kid).end <= in_token && c.lexeme(c.head_token(kid)) == name
        }
        _ => false,
    })
}

/// The read-to-member shadow walk over the ancestor path: at every scope
/// level between `read_index` (exclusive) and the read, a same-lexeme
/// binder on the path captures it. Block siblings bind from their
/// declaration; parameters, catch parameters, for-loop variables, and
/// switch-case pattern variables bind through their scope node; a nested
/// function's own name binds inside its body.
fn path_shadowed(c: &Check<'_>, top_index: usize, name: &str) -> bool {
    for (offset, &scope) in c.path[top_index + 1..c.path.len() - 1].iter().enumerate() {
        // The path node immediately inside `scope` — the subtree holding
        // the read (its own declarations never shadow it). The chain is
        // root-first, so that node is the next slice element, or the read
        // itself at the tail.
        let child = c
            .path
            .get(top_index + 1 + offset + 1)
            .copied()
            .unwrap_or_else(|| *c.path.last().expect("read node"));
        let binds = match c.kind(scope) {
            NodeKind::Block => c
                .collect_direct(scope)
                .into_iter()
                .any(|stmt| stmt != child && statement_binds(c, stmt, name)),
            NodeKind::CatchClause | NodeKind::FunctionExpression => c
                .collect_direct(scope)
                .into_iter()
                .find(|&kid| c.kind(kid) == NodeKind::FormalParameters)
                .is_some_and(|params| parameters_bind(c, params, name)),
            NodeKind::LocalFunctionDeclaration | NodeKind::NamedFunctionExpression => {
                function_name_binds(c, scope, name)
                    || c.collect_direct(scope)
                        .into_iter()
                        .find(|&kid| c.kind(kid) == NodeKind::FormalParameters)
                        .is_some_and(|params| parameters_bind(c, params, name))
            }
            NodeKind::ForIn | NodeKind::ForControlFlow => for_in_binds(c, scope, name),
            NodeKind::ForStatement => c
                .collect_direct(scope)
                .into_iter()
                .find(|&kid| c.kind(kid) == NodeKind::ForParts)
                .is_some_and(|parts| {
                    c.collect_direct(parts)
                        .into_iter()
                        .any(|stmt| statement_binds(c, stmt, name))
                }),
            NodeKind::SwitchCase | NodeKind::SwitchExpressionCase => {
                c.collect_direct(scope).into_iter().any(|kid| {
                    // Case patterns complete as flat direct children,
                    // so the variable is the child's own `Variable` word.
                    c.kind(kid) == NodeKind::DeclaredVariablePattern
                        && c
                            .field(kid, PayloadField::Variable)
                            .is_some_and(|v| v != NONE_TOKEN && c.lexeme(v) == name)
                })
            }
            NodeKind::IfStatement => {
                // An if-case's pattern variables bind the then branch and
                // the `when` guard only — the official scoping visits the
                // else branch outside the pattern scope, and the condition
                // itself precedes the binding.
                if !if_case_binds(c, scope, name) {
                    return false;
                }
                let branch_kids = c.collect_direct(scope);
                let Some(cond_pos) = branch_kids
                    .iter()
                    .position(|&kid| c.kind(kid) == NodeKind::ParenthesizedCondition)
                else {
                    return false;
                };
                let has_guard = c
                    .field(branch_kids[cond_pos], PayloadField::WhenKeyword)
                    .is_some_and(|when| when != NONE_TOKEN);
                branch_kids.get(cond_pos + 1) == Some(&child)
                    || (has_guard && branch_kids.get(cond_pos) == Some(&child))
            }
            _ => false,
        };
        if binds {
            return true;
        }
    }
    false
}

/// Whether `name` is an import prefix of the unit — the written import
/// surface (the prefix is the import row's direct `Identifier` child —
/// `as name;`; the directive rows sit one container level under the unit
/// root).
fn written_import_prefix(c: &Check<'_>, name: &str) -> bool {
    let Some(root) = c.path.first().copied() else {
        return false;
    };
    c.collect_direct(root).into_iter().any(|top| {
        c.collect_direct(top).into_iter().any(|inner| {
            c.kind(inner) == NodeKind::Import
                && c.collect_direct(inner).into_iter().any(|kid| {
                    c.kind(kid) == NodeKind::Identifier && c.lexeme(c.head_token(kid)) == name
                })
        })
    })
}

/// Whether `node` is one of the scope binders that capture `name` for its
/// whole span (the shadow walk's binder family: block locals, catch
/// parameters, nested functions, loop variables, case pattern variables,
/// and an if-case's pattern variables). The element-identity stand-in for
/// the write scans and reference counts that share the walk's semantics.
fn scope_binds(c: &Check<'_>, node: u32, name: &str) -> bool {
    match c.kind(node) {
        NodeKind::Block => c
            .collect_direct(node)
            .into_iter()
            .any(|stmt| statement_binds(c, stmt, name)),
        NodeKind::CatchClause | NodeKind::FunctionExpression => c
            .collect_direct(node)
            .into_iter()
            .find(|&kid| c.kind(kid) == NodeKind::FormalParameters)
            .is_some_and(|params| parameters_bind(c, params, name)),
        NodeKind::LocalFunctionDeclaration | NodeKind::NamedFunctionExpression => {
            function_name_binds(c, node, name)
                || c.collect_direct(node)
                    .into_iter()
                    .find(|&kid| c.kind(kid) == NodeKind::FormalParameters)
                    .is_some_and(|params| parameters_bind(c, params, name))
        }
        NodeKind::ForIn | NodeKind::ForControlFlow => for_in_binds(c, node, name),
        NodeKind::ForStatement => c
            .collect_direct(node)
            .into_iter()
            .find(|&kid| c.kind(kid) == NodeKind::ForParts)
            .is_some_and(|parts| {
                c.collect_direct(parts)
                    .into_iter()
                    .any(|stmt| statement_binds(c, stmt, name))
            }),
        NodeKind::SwitchCase | NodeKind::SwitchExpressionCase => {
            c.collect_direct(node).into_iter().any(|kid| {
                c.kind(kid) == NodeKind::DeclaredVariablePattern
                    && c
                        .field(kid, PayloadField::Variable)
                        .is_some_and(|v| v != NONE_TOKEN && c.lexeme(v) == name)
            })
        }
        NodeKind::IfStatement => if_case_binds(c, node, name),
        _ => false,
    }
}

/// Whether an `IfStatement`'s case clause binds `name`: the `case` keyword
/// rides the `ParenthesizedCondition` payload word 0 and the pattern rows
/// sit before it (the condition adopts the last-completed row — the
/// pattern root, or the `when` guard when one is written).
fn if_case_binds(c: &Check<'_>, stmt: u32, name: &str) -> bool {
    let kids = c.collect_direct(stmt);
    let Some(cond_pos) = kids
        .iter()
        .position(|&kid| c.kind(kid) == NodeKind::ParenthesizedCondition)
    else {
        return false;
    };
    let has_case = c
        .field(kids[cond_pos], PayloadField::CaseKeyword)
        .is_some_and(|case| case != NONE_TOKEN);
    has_case
        && kids[..=cond_pos].iter().any(|&kid| {
            // A `DeclaredVariablePattern` completes as a flat direct child
            // of the if statement (sibling of the condition row), so the
            // binder check reads the row's own `Variable` word, not only
            // its descendants.
            (c.kind(kid) == NodeKind::DeclaredVariablePattern
                && c
                    .field(kid, PayloadField::Variable)
                    .is_some_and(|v| v != NONE_TOKEN && c.lexeme(v) == name))
                || pattern_binds(c, kid, name)
        })
}

/// Whether the position `at` sits in `stmt`'s then branch — the only
/// branch an if-case pattern binding reaches (`scope_analyzer.dart`
/// `visitIfStatement`: the else branch is visited outside the pattern
/// scope).
fn in_then_branch(c: &Check<'_>, stmt: u32, at: usize) -> bool {
    let kids = c.collect_direct(stmt);
    let Some(cond_pos) = kids
        .iter()
        .position(|&kid| c.kind(kid) == NodeKind::ParenthesizedCondition)
    else {
        return false;
    };
    kids.get(cond_pos + 1)
        .is_some_and(|&then| c.byte_span(then).contains(&at))
}

/// [`scope_binds`] with the if-case precision: the binder family binds the
/// whole span, an if-case only its then branch.
fn scope_binds_at(c: &Check<'_>, node: u32, name: &str, at: usize) -> bool {
    match c.kind(node) {
        NodeKind::IfStatement => if_case_binds(c, node, name) && in_then_branch(c, node, at),
        // A nested function's parameter scope covers its whole body — the
        // `=>`/block rows complete as the function's *children* on the
        // production tiling and can sit past the row's own token span (the
        // `ExpressionBody` arrow value seals after the row), so the bound
        // runs from the head through the last subtree row's byte end.
        NodeKind::FunctionExpression
        | NodeKind::NamedFunctionExpression
        | NodeKind::LocalFunctionDeclaration => {
            scope_binds(c, node, name) && {
                let span = c.byte_span(node);
                let head = span.start;
                // The subtree rows tile contiguously; the deepest row's
                // byte end reaches the `=>` body when that value seals past
                // the row's own token span.
                let end = (c.children(node).start..node)
                    .map(|tail| c.byte_span(tail).end)
                    .max()
                    .unwrap_or(span.end)
                    .max(span.end);
                at >= head && at < end
            }
        }
        _ => scope_binds(c, node, name) && c.byte_span(node).contains(&at),
    }
}

/// A formal parameter's name (`FormalParameter` word 1, or the
/// function-typed form's name: the production `FunctionTypedFormalParameter`
/// anchors at its name token but may still tile the written return type's
/// `Identifier` (a `TypeReference` leaf) as a child — so the read skips the
/// type leaf and otherwise answers the head token; a hand-built table
/// carries a name `Identifier` child).
fn parameter_name<'a>(c: &'a Check<'_>, parameter: u32) -> Option<&'a str> {
    match c.kind(parameter) {
        NodeKind::FormalParameter => {
            let name = c.field(parameter, PayloadField::Name)?;
            (name != NONE_TOKEN).then(|| c.lexeme(name))
        }
        NodeKind::FunctionTypedFormalParameter => {
            let name_row = c.collect_direct(parameter).into_iter().find(|&kid| {
                c.kind(kid) == NodeKind::Identifier
                    && c.field(kid, PayloadField::Context)
                        != Some(dartq_parser::IdentifierContext::TypeReference as u32)
            });
            Some(
                c.lexeme(name_row.map_or_else(|| c.head_token(parameter), |kid| c.head_token(kid))),
            )
        }
        _ => None,
    }
}

/// The semantic bridge's inheritance-chain arm shared by the
/// override-family rules: each WRITTEN supertype of `class_row` anchors
/// its own cross-file walk ([`dartq_semantic::Analysis::inherited_member`],
/// transitively over the extends/with edges) — the checked member is
/// self-declared in `class_row`, so an anchor at `class_row` itself would
/// hit its own row and shadow the whole chain, while a supertype anchor's
/// own declarations are genuinely inherited. `concrete` selects the
/// official concrete-member cut (`getInheritedConcreteMember` —
/// extends/with/on anchors, abstract entries skipped, `implements` heads
/// excluded); otherwise every clause anchors (`getInheritedMember`).
/// The first anchor whose chain answers yields the declaring container's
/// name with its entry; `None` under an unmounted bridge, a class without
/// written supertypes, and unresolved chains (the under-report posture).
pub(crate) fn bridge_inherited_member<'a>(
    c: &'a Check<'_>,
    class_row: u32,
    concrete: bool,
    name: &str,
) -> Option<(String, &'a dartq_semantic::model::MemberEntry)> {
    let analysis = c.semantics()?;
    let member = analysis.strings().intern(name);
    for (head, _) in supertype_anchors(c, class_row, concrete) {
        // A prefixed `p.S` head queries the continuation — the model's
        // name-level key (the prefix itself carries no key).
        let spelled = if c.unit.tokens.get(head as usize + 1).is_some()
            && c.lexeme(head + 1) == "."
            && c.unit.tokens.get(head as usize + 2).is_some()
        {
            c.lexeme(head + 2)
        } else {
            c.lexeme(head)
        };
        let anchor = analysis.strings().intern(spelled);
        if let Some((defining, entry)) = analysis.inherited_member(anchor, member, concrete) {
            return Some((analysis.strings().resolve(defining).to_owned(), entry));
        }
    }
    None
}

/// The written supertype heads of `class_row`, clause-filtered (the
/// concrete cut drops `implements`), in written order — the
/// [`crate::rules::supertypes`] walk kept token-bearing (the bridge needs
/// the prefixed-head continuation, which the lexeme view cannot reach).
fn supertype_anchors(c: &Check<'_>, class_row: u32, concrete: bool) -> Vec<(u32, SupertypeClause)> {
    c.collect_direct(class_row)
        .into_iter()
        .take_while(|&kid| !matches!(c.kind(kid), NodeKind::DeclarationBody | NodeKind::EnumBody))
        .filter(|&kid| c.kind(kid) == NodeKind::Type)
        .map(|kid| {
            let head = c.head_token(kid);
            (head, supertype_clause_of(c, class_row, head))
        })
        .filter(|&(_, clause)| !concrete || !matches!(clause, SupertypeClause::Implements))
        .collect()
}

/// The clause keyword governing the supertype named at `head`: the nearest
/// preceding `extends`/`with`/`on`/`implements` lexeme back to the
/// declaration's span head (the header is a handful of tokens).
fn supertype_clause_of(c: &Check<'_>, class_row: u32, head: u32) -> SupertypeClause {
    let start = c.head_token(class_row);
    let mut token = head;
    while token > start {
        token -= 1;
        match c.lexeme(token) {
            "extends" => return SupertypeClause::Extends,
            "with" => return SupertypeClause::With,
            "on" => return SupertypeClause::On,
            "implements" => return SupertypeClause::Implements,
            _ => {}
        }
    }
    SupertypeClause::Implements
}

/// The extends-clause supertype's name token: the header's first `Type` row
/// when an `extends` keyword governs it (the clause keywords ride the header
/// as plain tokens — the bounded backward walk reads the nearest one), `None`
/// when the class names no extends supertype (the implicit `Object`
/// superclass has no parameters to forward to).
fn extends_head_token(c: &Check<'_>, class_row: u32) -> Option<u32> {
    let head = c
        .collect_direct(class_row)
        .into_iter()
        .take_while(|&kid| !matches!(c.kind(kid), NodeKind::DeclarationBody | NodeKind::EnumBody))
        .find(|&kid| c.kind(kid) == NodeKind::Type)
        .map(|row| c.head_token(row))?;
    let mut token = head;
    for _ in 0..32 {
        token = token.checked_sub(1)?;
        match c.lexeme(token) {
            "extends" => return Some(head),
            "with" | "on" | "implements" | "{" | ";" => return None,
            _ => {}
        }
    }
    None
}

/// The selector of the constructor's `super.named(...)` initializer, if any
/// (`Initializer` rows anchor at their head token — the unnamed form has no
/// selector face).
fn super_initializer_selector(c: &Check<'_>, ctor: u32) -> Option<String> {
    c.collect_direct(ctor)
        .into_iter()
        .find(|&kid| c.kind(kid) == NodeKind::Initializers)
        .and_then(|initializers| {
            c.collect_direct(initializers).into_iter().find(|&row| {
                c.kind(row) == NodeKind::Initializer && c.lexeme(c.head_token(row)) == "super"
            })
        })
        .and_then(|row| {
            let head = c.head_token(row);
            (c.lexeme(head + 1) == ".").then(|| c.lexeme(head + 2).to_string())
        })
}

/// Whether the super formal `param` written with `written` provably has no
/// corresponding equal-typed super-constructor parameter — the
/// `type_init_formals` element check (:52-64,
/// `SuperFormalParameterElement.superConstructorParameter`): positional
/// formals map by index, named formals by name. Only decidable faces answer:
/// a parameter absent from the table, or a no-argument named model type
/// against a one-token (plus `?`) written run. Parameterized, prefixed and
/// unknown model types are undecidable (`false`) — the lexical report
/// posture, so valid-code parity never regresses.
fn super_formal_provably_unmatched(c: &Check<'_>, param: u32, written: u32) -> bool {
    let Some(analysis) = c.semantics() else {
        return false;
    };
    let Some(&ctor) = c
        .path
        .iter()
        .rev()
        .find(|&&row| c.kind(row) == NodeKind::Constructor)
    else {
        return false; // no owning constructor visible — undecidable
    };
    let Some(&class_row) = c
        .path
        .iter()
        .rev()
        .find(|&&row| c.kind(row) == NodeKind::ClassDeclaration)
    else {
        return false; // no class header visible — undecidable
    };
    // No extends supertype: the implicit `Object` superclass declares no
    // constructor parameters — the formal never corresponds.
    let Some(head) = extends_head_token(c, class_row) else {
        return true;
    };
    // The written `super.named(...)` initializer selects the constructor the
    // formals correspond to, else the unnamed face. The extends head's
    // type-reference position publishes the resolution row — the continuation
    // token for a prefixed `p.S` head (the prefix publishes no type-use row).
    let selector = super_initializer_selector(c, ctor);
    let query_token = if c.lexeme(head + 1) == "." {
        head + 2
    } else {
        head
    };
    let Some(token) = c.unit.tokens.get(query_token as usize) else {
        return false;
    };
    let site = dartq_source::SourceRange {
        offset: token.byte_offset,
        length: token.byte_len,
    };
    let selector_id = selector.map(|name| analysis.strings().intern(&name));
    let Some(binding) = analysis.resolve_constructor(site, selector_id) else {
        // The supertype is unindexed (an SDK type, an unanalyzed dependency)
        // — undecidable.
        return false;
    };
    if !binding.exists {
        // The selector names no constructor of the supertype (or the
        // superclass has no unnamed face) — the implicit forwarding is a
        // compile error upstream and the formal never corresponds.
        return true;
    }
    let Some(table) = binding.formals else {
        return false; // formals uncaptured — undecidable
    };
    let strings = analysis.strings();
    let kind = c.field(param, PayloadField::FormalParameterKind);
    let named = matches!(
        kind,
        Some(k)
            if k == dartq_parser::FormalParameterKind::OptionalNamed as u32
                || k == dartq_parser::FormalParameterKind::RequiredNamed as u32
    );
    let target = if named {
        let Some(name_token) = c.field(param, PayloadField::Name) else {
            return false; // a nameless named-formal face — undecidable
        };
        if name_token == crate::rules::NONE_TOKEN {
            return false;
        }
        let name = c.lexeme(name_token);
        table
            .named
            .iter()
            .find(|formal| strings.resolve(formal.name) == name)
    } else {
        let Some(index) = super_formal_positional_index(c, param) else {
            return false; // the formal sits outside the positional walk — undecidable
        };
        table.positional.get(index)
    };
    let Some(formal) = target else {
        // No corresponding parameter — a compile error upstream, silent.
        return true;
    };
    written_run_differs(c, written, formal.ty, strings)
}

/// The positional index of `param` in its list's positional group (required
/// group, then the `[]` group — the named `{}` group is out), the
/// correspondence order of positional super formals.
fn super_formal_positional_index(c: &Check<'_>, param: u32) -> Option<usize> {
    let list = c
        .path
        .iter()
        .rev()
        .copied()
        .find(|&row| c.kind(row) == NodeKind::FormalParameters)?;
    let mut index = 0;
    for section in c.collect_direct(list) {
        match c.kind(section) {
            NodeKind::FormalParameter => {
                if section == param {
                    return Some(index);
                }
                index += 1;
            }
            NodeKind::OptionalFormalParameters if c.lexeme(c.head_token(section)) == "[" => {
                for inner in c.collect_direct(section) {
                    if c.kind(inner) != NodeKind::FormalParameter {
                        continue;
                    }
                    if inner == param {
                        return Some(index);
                    }
                    index += 1;
                }
            }
            _ => {}
        }
    }
    None
}

/// Whether the written type run of `written` provably differs from the
/// model's declared `ty`: a no-argument named model type answers a one-token
/// (plus `?`) run by name and nullability, `dynamic` answers its own
/// spelling; every other model face is undecidable (`false`).
fn written_run_differs(
    c: &Check<'_>,
    written: u32,
    ty: dartq_semantic::model::Ty,
    strings: &dartq_source::StringTable,
) -> bool {
    let run = c.unit.ast.token_range(written);
    let model: Option<(&str, bool)> = match ty {
        dartq_semantic::model::Ty::Named {
            name,
            nullable,
            args,
        } if args == dartq_semantic::model::ArgListId::EMPTY => {
            Some((strings.resolve(name), nullable))
        }
        dartq_semantic::model::Ty::Dynamic => Some(("dynamic", false)),
        _ => None,
    };
    let Some((name, nullable)) = model else {
        return false;
    };
    match run.end - run.start {
        // `int` written vs a `T`/`T?` model — differs on a name or
        // nullability mismatch.
        1 => nullable || c.lexeme(run.start) != name,
        // `int?` written vs a `T?` model — differs on either piece.
        2 => !nullable || c.lexeme(run.start) != name || c.lexeme(run.start + 1) != "?",
        _ => false,
    }
}

pub(crate) use crate::rules::get_set_token;

// —— primary-constructors family faces ———————————————————————————————————
//
// Shared token faces of the `Feature.primary_constructors`-gated rules:
// each consumes the engine bit in-rule (the registration bail is
// behaviorally identical at check time), mirroring the upstream
// `isFeatureEnabled` posture in `registerNodeProcessors`.

/// The `external` (member family) `ModSet` bit, bridge `mod_bits`.
const EXTERNAL_MEMBER_BIT: u16 = 1 << 2;
/// The `var`/`final`/`const` (member family) `ModSet` bit, bridge `mod_bits`.
const VAR_FINAL_OR_CONST_MEMBER_BIT: u16 = 1 << 6;

/// The type-name echo of a constructor row: the member-name context
/// identifier child (the parser emits the class-name head there —
/// members.rs `ensure_identifier(IdentifierContext::MethodDeclaration)`).
fn type_name_echo(c: &Check<'_>, ctor: u32) -> Option<u32> {
    let ctx = dartq_parser::IdentifierContext::MethodDeclaration as u32;
    c.collect_direct(ctor)
        .into_iter()
        .find(|&kid| c.kind(kid) == NodeKind::Identifier && c.field(kid, PayloadField::Context) == Some(ctx))
}

/// The written constructor name part: the method-continuation context
/// identifier child (the `.name` after the class-name echo).
fn constructor_name_part(c: &Check<'_>, ctor: u32) -> Option<u32> {
    let ctx = dartq_parser::IdentifierContext::MethodDeclarationContinuation as u32;
    c.collect_direct(ctor)
        .into_iter()
        .find(|&kid| c.kind(kid) == NodeKind::Identifier && c.field(kid, PayloadField::Context) == Some(ctx))
}

/// A modifier keyword immediately before a constructor row's span head:
/// the row anchors at the declared name, so the head modifiers (`augment`,
/// `external`, `const`, `factory` — four max) sit in the tokens before it.
fn modifier_keyword(c: &Check<'_>, ctor: u32, needle: &str) -> Option<u32> {
    let mut token = c.head_token(ctor);
    for _ in 0..4 {
        token = token.checked_sub(1)?;
        match c.lexeme(token) {
            lexeme if lexeme == needle => return Some(token),
            "augment" | "external" | "const" | "factory" => {}
            _ => return None,
        }
    }
    None
}

/// The name identifier row of a class-like declaration: the production
/// wrapper keeps it as the row's left sibling, hand-built trees nest it
/// (`class_declaration_name` returning the node instead of the lexeme).
fn container_name_node(c: &Check<'_>, row: u32) -> Option<u32> {
    let is_name = |kid: u32| {
        c.kind(kid) == NodeKind::Identifier
            && matches!(
                c.field(kid, PayloadField::Context),
                Some(ctx)
                    if ctx == dartq_parser::IdentifierContext::ClassOrMixinOrExtensionDeclaration
                        as u32
                        || ctx == dartq_parser::IdentifierContext::EnumDeclaration as u32
            )
    };
    if let Some(name) = c.collect_direct(row).into_iter().find(|&kid| is_name(kid)) {
        return Some(name);
    }
    let parent = c.unit.ast.parent(row)?;
    let siblings = c.collect_direct(parent);
    let pos = siblings.iter().position(|&kid| kid == row)?;
    siblings[..pos]
        .iter()
        .rev()
        .copied()
        .find(|&kid| is_name(kid))
}

/// The name token of a class-like declaration — the
/// `unreachable_from_main` face anchor. Class/mixin/enum names resolve
/// through [`container_name_node`]'s `Identifier` row (the token is the
/// node's span head). The two extension shapes never mint a name node:
/// the parser reads the name straight off the cursor
/// (`parse_extension_declaration` / `parse_extension_type_declaration`)
/// and the bridge stores it as a payload word
/// (`begin_extension_declaration` word 1,
/// `begin_extension_type_declaration` word 2), `NONE_TOKEN` when absent
/// (an unnamed extension never reports through this face).
fn container_name_token(c: &Check<'_>, row: u32) -> Option<u32> {
    match c.kind(row) {
        NodeKind::ExtensionDeclaration => {
            let token = c.field(row, PayloadField::Name)?;
            (token != NONE_TOKEN).then_some(token)
        }
        NodeKind::ExtensionTypeDeclaration => {
            let token = c.field(row, PayloadField::Name)?;
            (token != NONE_TOKEN).then_some(token)
        }
        _ => container_name_node(c, row).map(|node| c.head_token(node)),
    }
}

// —— W1 declaration rules (the #1097 wave) ———————————————————————————————
//
// Shared faces of the incoming wave: the written formal-parameter rows,
// the body async words, and the same-class field read the const-class and
// declaring-parameter rules fold their element checks into.

/// The `augment` (member family) `ModSet` bit, bridge `mod_bits`.
const AUGMENT_MEMBER_BIT: u16 = 1 << 1;

/// The plain formal rows of a parameters pair: the owner's direct plain
/// formals plus the optional group's rows (the analyzer's
/// `parameters.parameters` list includes both nests; primary constructors
/// and generic function types carry the same shape).
fn formal_parameter_rows(c: &Check<'_>, owner: u32) -> Vec<u32> {
    let mut rows = Vec::new();
    // The parameter rows under `owner`, descending the optional group:
    // `{p}`/`[p]` sections tile as an `OptionalFormalParameters` row
    // inside `FormalParameters`, one level deeper than requireds.
    fn into_group(c: &Check<'_>, group: u32, rows: &mut Vec<u32>) {
        for kid in c.collect_direct(group) {
            match c.kind(kid) {
                NodeKind::FormalParameter => rows.push(kid),
                NodeKind::OptionalFormalParameters => into_group(c, kid, rows),
                _ => {}
            }
        }
    }
    for kid in c.collect_direct(owner) {
        match c.kind(kid) {
            NodeKind::FormalParameter => rows.push(kid),
            NodeKind::FormalParameters | NodeKind::OptionalFormalParameters => {
                into_group(c, kid, &mut rows);
            }
            _ => {}
        }
    }
    rows
}

/// The (`async` keyword, `*`) token pair of a function body:
/// `BlockBody` words 0-1, `ExpressionBody` words 2-3; bodies that
/// carry no modifier words answer the absent sentinel.
fn async_words(c: &Check<'_>, body: u32) -> (u32, u32) {
    match c.kind(body) {
        NodeKind::BlockBody => (
            c.field(body, PayloadField::AsyncKeyword).unwrap_or(NONE_TOKEN),
            c.field(body, PayloadField::Star).unwrap_or(NONE_TOKEN),
        ),
        NodeKind::ExpressionBody => (
            c.field(body, PayloadField::AsyncKeyword).unwrap_or(NONE_TOKEN),
            c.field(body, PayloadField::Star).unwrap_or(NONE_TOKEN),
        ),
        _ => (NONE_TOKEN, NONE_TOKEN),
    }
}

/// The written type row of the same-class field `name`, `None` when the
/// name is not a same-file field or carries no written type (the field
/// read the const-class and declaring-parameter rules fold their element
/// type checks into).
fn same_class_field_type(c: &Check<'_>, class_row: u32, name: &str) -> Option<u32> {
    for member in body_members(c, class_row) {
        if c.kind(member) != NodeKind::Fields {
            continue;
        }
        for decl in c.collect_direct(member) {
            if c.kind(decl) != NodeKind::VariablesDeclaration {
                continue;
            }
            for var in c.collect_direct(decl) {
                if c.kind(var) == NodeKind::InitializedIdentifier
                    && crate::rules::initialized_identifier_name(c, var)
                        .is_some_and(|tok| c.lexeme(tok) == name)
                {
                    // The `Type` row tiles as the `Fields` child before
                    // the variable's name row, and the name row precedes
                    // `decl` — step back twice through `member`'s window.
                    return type_before(c, member, decl)
                        .and_then(|(name_row, _)| type_before(c, member, name_row))
                        .map(|(t, _)| t)
                        .filter(|&t| c.kind(t) != NodeKind::NoType);
                }
            }
        }
    }
    None
}

/// The report token of a name row: an `OperatorName` names through its
/// payload token (the operator itself — `head_token` would hit the
/// `operator` keyword), an `Identifier` through its span head.
fn name_row_token(c: &Check<'_>, name: u32) -> u32 {
    if c.kind(name) == NodeKind::OperatorName {
        c.field(name, PayloadField::Name)
            .unwrap_or_else(|| c.head_token(name))
    } else {
        c.head_token(name)
    }
}

/// Whether a `return` sits lexically inside a try statement's body block
/// (the official `isWithinTryBlock` fold): the walk stops at the function
/// body block (outside), the try body (inside — the official registration
/// visitor defers those returns to the analyzer's try-block face), and
/// keeps climbing through catch/finally containers (the official walk
/// answers false for both).
fn within_try_block(c: &Check<'_>, node: u32) -> bool {
    let mut current = node;
    while let Some(parent) = c.unit.ast.parent(current) {
        if c.kind(current) == NodeKind::Block {
            match c.kind(parent) {
                NodeKind::BlockBody => return false,
                NodeKind::TryStatement
                    // Only the try body counts — the first `Block` child;
                    // the finally block trails the catch clauses.
                    if c.collect_direct(parent).first() == Some(&current) => {
                        return true;
                    }
                _ => {}
            }
        }
        current = parent;
    }
    false
}

// —— shared faces of the annotation / member-scan / trivial-body rules ——

/// Whether a token is a plain identifier.
fn is_identifier_token(c: &Check<'_>, token: u32) -> bool {
    c.unit
        .tokens
        .get(token as usize)
        .is_some_and(|t| matches!(t.kind, dartq_scanner::TokenKind::Identifier))
}

/// The `Metadata` rows riding a top-level declaration: the star is the
/// wrapper's left sibling (the metadata parses before the wrapper opens —
/// `parse_top_level_declaration` runs `parse_metadata_star` first).
pub(crate) fn top_level_annotations(c: &Check<'_>, row: u32) -> Vec<u32> {
    let Some(wrapper) = c.unit.ast.parent(row) else {
        return Vec::new();
    };
    if c.kind(wrapper) != NodeKind::TopLevelDeclaration {
        return Vec::new();
    }
    let Some(root) = c.unit.ast.parent(wrapper) else {
        return Vec::new();
    };
    let siblings = c.collect_direct(root);
    let Some(pos) = siblings.iter().position(|&s| s == wrapper) else {
        return Vec::new();
    };
    let Some(&star) = pos.checked_sub(1).and_then(|i| siblings.get(i)) else {
        return Vec::new();
    };
    if c.kind(star) != NodeKind::MetadataStar {
        return Vec::new();
    }
    c.collect_direct(star)
        .into_iter()
        .filter(|&meta| c.kind(meta) == NodeKind::Metadata)
        .collect()
}

/// Whether the annotation carries a name identifier of `name` (a prefix
/// chain's trailing segment answers through the same scan).
pub(crate) fn annotation_names(c: &Check<'_>, meta: u32, name: &str) -> bool {
    c.collect_direct(meta)
        .into_iter()
        .any(|piece| c.kind(piece) == NodeKind::Identifier && c.lexeme(c.head_token(piece)) == name)
}

/// Whether the annotation resolves to the deprecation faces (`deprecated`
/// the constant or `Deprecated` the class — the name-identifier lexeme).
fn is_deprecated_metadata(c: &Check<'_>, meta: u32) -> bool {
    c.collect_direct(meta).into_iter().any(|piece| {
        c.kind(piece) == NodeKind::Identifier
            && matches!(c.lexeme(c.head_token(piece)), "deprecated" | "Deprecated")
    })
}

/// The member rows of a class-like container: the body wrapper's children
/// (`DeclarationBody` for classes and extension types, `EnumBody` for
/// enums — [`crate::rules::body_members`] folds only the former), else the
/// row's own direct children (hand-built tables). Returns the wrapper (or
/// the row itself) — the metadata-star sibling lives beside it.
fn container_members(c: &Check<'_>, row: u32) -> (u32, Vec<u32>) {
    let kids = c.collect_direct(row);
    match kids
        .iter()
        .copied()
        .find(|&kid| matches!(c.kind(kid), NodeKind::DeclarationBody | NodeKind::EnumBody))
    {
        Some(body) => (body, crate::rules::rows_with_parent(c, body)),
        None => (row, kids),
    }
}

/// Whether a `Fields` row declares a variable of `name` (any storage —
/// the field's implicit getter carries the readable face).
fn fields_declare(c: &Check<'_>, fields: u32, name: &str) -> bool {
    c.collect_direct(fields)
        .into_iter()
        .filter(|&decl| c.kind(decl) == NodeKind::VariablesDeclaration)
        .flat_map(|decl| c.collect_direct(decl))
        .any(|var| {
            c.kind(var) == NodeKind::InitializedIdentifier
                && crate::rules::initialized_identifier_name(c, var)
                    .is_some_and(|token| c.lexeme(token) == name)
        })
}

/// Whether the member carries `@override`: its star is the left sibling in
/// the body container it hangs from.
fn override_annotation(c: &Check<'_>, member: u32) -> bool {
    c.unit
        .ast
        .parent(member)
        .is_some_and(|body| crate::rules::has_override_annotation(c, body, member))
}

/// The formal parameter rows of a group: both kinds, descending into the
/// optional group.
fn parameter_rows(c: &Check<'_>, group: u32) -> Vec<u32> {
    let mut rows = Vec::new();
    for kid in c.collect_direct(group) {
        match c.kind(kid) {
            NodeKind::FormalParameter | NodeKind::FunctionTypedFormalParameter => rows.push(kid),
            NodeKind::OptionalFormalParameters => {
                rows.extend(c.collect_direct(kid).into_iter().filter(|&row| {
                    matches!(
                        c.kind(row),
                        NodeKind::FormalParameter | NodeKind::FunctionTypedFormalParameter
                    )
                }));
            }
            _ => {}
        }
    }
    rows
}

/// The token run of a member's single-statement body: an `=>` body's
/// interior (the arrow and `;` ride the childless body row, `;` included
/// when the member flavor sealed one), or a block body's interior when
/// exactly one statement row hangs from it (the statement's tokens fill
/// the braces).
fn sole_statement_tokens(c: &Check<'_>, method: u32) -> Option<core::ops::Range<u32>> {
    let body = crate::rules::last_child(c, method)?;
    match c.kind(body) {
        NodeKind::ExpressionBody => {
            let arrow = c.field(body, PayloadField::Arrow)?;
            let semi = c
                .field(body, PayloadField::Semicolon)
                .unwrap_or(crate::rules::NONE_TOKEN);
            (semi != crate::rules::NONE_TOKEN).then_some(arrow + 1..semi)
        }
        NodeKind::BlockBody => {
            let statements = c
                .collect_direct(body)
                .into_iter()
                .filter(|&row| crate::rules::is_statement_row(c, row))
                .count();
            let range = c.unit.ast.token_range(body);
            (statements == 1 && range.len() >= 2)
                .then(|| range.start + 1..range.end - 1)
                .filter(|run| !run.is_empty())
        }
        _ => None,
    }
}

/// The matching `>` of the `<` at `open`, within `end` (lexeme counting;
/// shift tokens close their due depth, strings and closures cannot appear
/// in a type-argument token face).
fn balanced_angle(c: &Check<'_>, open: u32, end: u32) -> Option<u32> {
    let mut depth = 0u32;
    let mut token = open;
    while token < end {
        match c.lexeme(token) {
            "<" => depth += 1,
            ">" => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Some(token);
                }
            }
            ">>" => {
                depth = depth.saturating_sub(2);
                if depth == 0 {
                    return Some(token);
                }
            }
            _ => {}
        }
        token += 1;
    }
    None
}

/// The written type token run of a local declaration row `(start,
/// end_exclusive)`: the gap between the recovered storage keyword and the
/// row, else the type-ish run before the row (the row anchors at the first
/// declarator name; braces, brackets and parens bound the walk, so record
/// and function-type spellings under-report). `None` when no type is
/// written (`var x`, `final x`).
fn local_written_type(c: &Check<'_>, row: u32) -> Option<(u32, u32)> {
    let head = c.head_token(row);
    if let Some(keyword) = crate::rules::variables_declaration_keyword(c, row) {
        (head > keyword + 1).then_some((keyword + 1, head))
    } else {
        let mut start = head;
        while let Some(prev) = start.checked_sub(1) {
            // `,` is in the set: a multi-argument generic written run
            // (`Map<int, int>`) must read whole; the row anchors at the
            // first declarator name, so a `,` before it can only sit inside
            // the type's own argument list.
            if matches!(c.lexeme(prev), "." | "?" | "<" | ">" | ">>" | ">>>" | ",")
                || is_identifier_token(c, prev)
            {
                start = prev;
            } else {
                break;
            }
        }
        (start < head).then_some((start, head))
    }
}

/// The written type token run `(start, end_exclusive)` of a
/// `VariablesDeclaration`, shared by the null-initializer and nullable-
/// final gates: a `Type`-kind child (hand-built tables), the production
/// row anchored at the name ([`local_written_type`]), or the
/// keyword-anchored field run after `late`/`var`/`final`/`const` up to the
/// first variable's name.
fn declaration_written_type(c: &Check<'_>, decl: u32) -> Option<(u32, u32)> {
    let kids = c.collect_direct(decl);
    if let Some(type_node) = kids
        .iter()
        .copied()
        .find(|&kid| c.kind(kid) == NodeKind::Type)
    {
        let range = c.unit.ast.token_range(type_node);
        return Some((range.start, range.end));
    }
    local_written_type(c, decl).or_else(|| {
        let name = kids
            .iter()
            .copied()
            .find(|&kid| c.kind(kid) == NodeKind::InitializedIdentifier)
            .map(|kid| c.head_token(kid))?;
        let mut start = c.head_token(decl);
        for _ in 0..2 {
            match c.lexeme(start) {
                "late" | "var" | "final" | "const" => start += 1,
                _ => break,
            }
        }
        (start < name).then_some((start, name))
    })
}

/// The nullable written-type read over a token run: the tail `?` spell, or
/// the bare `dynamic` type — the `typeSystem.isNullable` faces decidable
/// from the declaration's tokens.
fn written_run_nullable(c: &Check<'_>, run: (u32, u32)) -> bool {
    let (start, end) = run;
    start < end && (c.lexeme(end - 1) == "?" || (end - start == 1 && c.lexeme(start) == "dynamic"))
}

/// The written type row of the non-static instance field `name` declared in
/// the enclosing class or enum body — the initializing-formal element
/// check's same-file face. The written type rides the `Fields` row as a
/// production sibling before the declaration (hand-built tables nest it
/// under the declaration). `None` when the container or the field is
/// absent, or the field is unwritten.
fn instance_field_written_type(c: &Check<'_>, name: &str) -> Option<u32> {
    let container = c.path.iter().rev().copied().find(|&row| {
        matches!(
            c.kind(row),
            NodeKind::ClassDeclaration | NodeKind::EnumDeclaration
        )
    })?;
    for member in c.children(container).start..=container {
        if c.kind(member) != NodeKind::Fields
            || c.field(member, PayloadField::MemberKind)
                != Some(dartq_parser::MemberKind::NonStaticField as u32)
        {
            continue;
        }
        let group_type = c
            .collect_direct(member)
            .into_iter()
            .find(|&kid| c.kind(kid) == NodeKind::Type);
        for decl in c.collect_direct(member) {
            if c.kind(decl) != NodeKind::VariablesDeclaration {
                continue;
            }
            let written = group_type.or_else(|| {
                c.collect_direct(decl)
                    .into_iter()
                    .find(|&kid| c.kind(kid) == NodeKind::Type)
            });
            for var in c.collect_direct(decl) {
                if c.kind(var) != NodeKind::InitializedIdentifier {
                    continue;
                }
                if crate::rules::initialized_identifier_name(c, var)
                    .is_some_and(|token| c.lexeme(token) == name)
                {
                    return written;
                }
            }
        }
    }
    None
}

// —— shared faces of the obvious-type / finality / lateability rules ——

/// The written type token run of a field or top-level
/// [`NodeKind::VariablesDeclaration`] row `(start, end_exclusive)`: the row
/// anchors at the keyword or type start (members.rs `decl_head` — `late`,
/// else `var`/`final`/`const`, else the type start), so the run is the
/// keyword-skipped head up to the first declarator's name. `None` when no
/// type is written (`var _x`, `late _x`).
fn fields_written_type(c: &Check<'_>, row: u32) -> Option<(u32, u32)> {
    let mut start = c.head_token(row);
    while matches!(
        c.lexeme(start),
        "late" | "var" | "final" | "const" | "static"
    ) {
        start += 1;
    }
    let name = c
        .collect_direct(row)
        .into_iter()
        .find(|&decl| c.kind(decl) == NodeKind::InitializedIdentifier)
        .and_then(|decl| crate::rules::initialized_identifier_name(c, decl))?;
    (start < name).then_some((start, name))
}

/// The lexeme run of a token range with `>`-family closers split into
/// single `>` pieces — the token table lexes `>>`/`>>>` as one token while
/// derived spellings are built from `>` pieces, so every
/// written-vs-derived compare works on the split form.
fn expanded_lexemes<'a>(c: &'a Check<'a>, range: core::ops::Range<u32>) -> Vec<&'a str> {
    let mut lexemes = Vec::with_capacity(range.len());
    for token in range {
        match c.lexeme(token) {
            ">>" => lexemes.extend([">", ">"]),
            ">>>" => lexemes.extend([">", ">", ">"]),
            lexeme => lexemes.push(lexeme),
        }
    }
    lexemes
}

/// The argument lexemes inside the `<…>` opened at `open` (the match
/// bounded by `end`): a `>>`/`>>>` closer's spare `>` pieces belong to the
/// inner arguments — `<List<int>>`'s `>>` closes the inner `<` first.
fn angle_arguments<'a>(c: &'a Check<'a>, open: u32, end: u32) -> Option<Vec<&'a str>> {
    let close = balanced_angle(c, open, end)?;
    let mut args = expanded_lexemes(c, open + 1..close);
    args.extend(core::iter::repeat_n(">", c.lexeme(close).len() - 1));
    Some(args)
}

/// Whether the written type run's lexemes equal the derived spelling.
fn spelling_matches(c: &Check<'_>, written: (u32, u32), spelling: &[&str]) -> bool {
    expanded_lexemes(c, written.0..written.1) == spelling
}

/// The lexically-derived type spelling of an obvious initializer
/// (`upstream Dart SDK source, pkg/linter/lib/src/util/obvious_types.dart`,
/// `hasObviousType`, over the token faces): `Some` spells the static type
/// the initializer provably carries — empty for the obvious faces whose
/// spelling the token table cannot name (a bare identifier carries its
/// declaration's type, `this`, `throw`). `None` when the initializer is
/// not obvious. The faces the element model answers but the token table
/// cannot (`hashCode`/`toString` accesses, spreads, null-aware and
/// control-flow collection elements) answer `None` — the documented
/// under-report of the obvious-type family.
fn obvious_spelling<'a>(c: &'a Check<'a>, value: u32) -> Option<Vec<&'a str>> {
    match c.kind(value) {
        NodeKind::LiteralInt => Some(vec!["int"]),
        NodeKind::LiteralDouble => Some(vec!["double"]),
        NodeKind::LiteralBool | NodeKind::IsExpression => Some(vec!["bool"]),
        NodeKind::StringLiteral => Some(vec!["String"]),
        NodeKind::LiteralNull => Some(vec!["Null"]),
        NodeKind::SymbolLiteral => Some(vec!["Symbol"]),
        // `-1` / `-1.0` — the MINUS-only prefix face (`!true` is not
        // obvious upstream: the PrefixExpression case is MINUS-only).
        NodeKind::UnaryPrefix => {
            let operand = crate::rules::last_child(c, value)?;
            match c.lexeme(c.head_token(value)) {
                "-" => match c.kind(operand) {
                    NodeKind::LiteralInt => Some(vec!["int"]),
                    NodeKind::LiteralDouble => Some(vec!["double"]),
                    _ => None,
                },
                _ => None,
            }
        }
        // `1 as num` — obvious; the spelling is the as-target.
        NodeKind::AsExpression => {
            let target = c
                .collect_direct(value)
                .into_iter()
                .rfind(|&kid| c.kind(kid) == NodeKind::Type)?;
            Some(expanded_lexemes(c, c.unit.ast.token_range(target)))
        }
        // `A()..x..x..x` — the target's face.
        NodeKind::Cascade => obvious_spelling(c, c.collect_direct(value).into_iter().next()?),
        NodeKind::ParenthesizedExpression => {
            obvious_spelling(c, c.collect_direct(value).into_iter().next()?)
        }
        // Both branches obvious with one spelling (`obvious_types.dart`
        // :203-209; the `else` face is ignored for the candidate type).
        NodeKind::Conditional => {
            let branches: Vec<u32> = c.collect_direct(value);
            let then = obvious_spelling(c, *branches.get(branches.len().checked_sub(2)?)?)?;
            let otherwise = obvious_spelling(c, *branches.last()?)?;
            (then == otherwise).then_some(then)
        }
        // A positional record of obvious fields (`(1, 'a')` is
        // `(int, String)`); a named field under-reports.
        NodeKind::RecordLiteral => {
            let mut spelling = vec!["("];
            for (index, field) in c.collect_direct(value).into_iter().enumerate() {
                if c.kind(field) == NodeKind::NamedArgument {
                    return None;
                }
                if index > 0 {
                    spelling.push(",");
                }
                spelling.extend(obvious_spelling(c, field)?);
            }
            spelling.push(")");
            Some(spelling)
        }
        // A bare identifier carries its declaration's type — obvious with
        // a spelling the token table cannot name (the promotion-equality
        // face of `SimpleIdentifier`).
        NodeKind::ThisExpression | NodeKind::ThrowExpression | NodeKind::Identifier => {
            Some(Vec::new())
        }
        // `a == b` / `a != b` are `bool` (`obvious_types.dart` :234-238);
        // every other operator face under-reports.
        NodeKind::BinaryExpression => {
            matches!(c.lexeme(c.head_token(value)), "==" | "!=").then(|| vec!["bool"])
        }

        NodeKind::ListLiteral | NodeKind::SetOrMapLiteral => collection_spelling(c, value),
        NodeKind::Send
        | NodeKind::NewExpression
        | NodeKind::ImplicitCreation
        | NodeKind::ConstExpression
        | NodeKind::ConstLiteral => creation_spelling(c, c.unit.ast.token_range(value)),
        _ => None,
    }
}

/// The collection-literal face: the spelling is the kind around the
/// explicit type arguments (`<int>[…]`, `<K, V>{…}`), else the elements'
/// common derived spelling (`['a', 'b']` is `List<String>`). A bare empty
/// literal and any non-obvious or mixed element answer `None` — the
/// context-inferred faces under-report.
fn collection_spelling<'a>(c: &'a Check<'a>, literal: u32) -> Option<Vec<&'a str>> {
    let elements = collection_elements(c, literal);
    let is_map = elements.iter().any(|segment| {
        segment
            .last()
            .is_some_and(|&root| c.kind(root) == NodeKind::MapEntry)
    });
    let kind: &str = match c.kind(literal) {
        NodeKind::ListLiteral => "List",
        // `node.isMap`: an own `MapEntry` element makes the literal a map
        // (a nested entry is buried inside its own literal's segment, and
        // an if element carrying one contributes only a null triple — the
        // under-report corner, `obvious_types.dart`'s `keyType`/`valueType`
        // default arms).
        NodeKind::SetOrMapLiteral => {
            if is_map {
                "Map"
            } else {
                "Set"
            }
        }
        _ => return None,
    };
    let tokens = c.unit.ast.token_range(literal);
    // `self.typeArguments != null` is trivially obvious upstream
    // (`obvious_types.dart` :120-123) — heterogeneous elements included.
    if let Some(open) = type_arguments_open(c, literal) {
        let mut spelling = vec![kind, "<"];
        spelling.extend(angle_arguments(c, open, tokens.end)?);
        spelling.push(">");
        return Some(spelling);
    }
    if elements.is_empty() {
        return None;
    }
    // The element fold mirrors the official `TypedLiteral` arm
    // (`obvious_types.dart` :130-154): every element must be obvious, then
    // its `(elementType, keyType, valueType)` triple joins the running
    // candidates through `_sameOrNull` (:14-15) — a `null` leg is
    // compatible with everything, so spread/if/null-aware elements
    // participate with `None` legs rather than bailing; `for` elements are
    // never obvious (:56-57). All three candidates null at the end means
    // `return false` (:152-153) — the literal is not obvious.
    let mut element: Option<Vec<&str>> = None;
    let mut key: Option<Vec<&str>> = None;
    let mut value: Option<Vec<&str>> = None;
    for segment in &elements {
        let (e, k, v) = element_types(c, segment)?;
        for (candidate, contribution) in [(&mut element, e), (&mut key, k), (&mut value, v)] {
            match (candidate, contribution) {
                (Some(previous), Some(next)) if *previous != next => return None,
                (slot @ None, next) => *slot = next,
                _ => {}
            }
        }
    }
    let mut spelling = vec![kind, "<"];
    if is_map {
        // A map whose fold also gathered an element type is not obvious
        // upstream (`obvious_types.dart` :144-146 takes the iterable check
        // — `elementTypeOfIterable` of a `Map` is `null`, which no
        // contributed element type equals).
        if element.is_some() {
            return None;
        }
        spelling.extend(key?);
        spelling.push(",");
        spelling.extend(value?);
    } else {
        spelling.extend(element?);
    }
    spelling.push(">");
    Some(spelling)
}

/// The literal's explicit type-arguments `<` token, if written. Hand-built
/// tables anchor the literal's span at `<` or carry a `TypeArguments`
/// child; the production tiling leaves the `TypeArguments` row an
/// owner-sibling — the literal itself starts at `[`/`{`, and a `>>`
/// closer's split `>` tokens park at the token-table tail, so the sibling
/// match reads byte spans: the `<` run right before the literal head
/// contains the head's predecessor token (the same span-containment walk
/// [`collection_elements`] does).
fn type_arguments_open(c: &Check<'_>, literal: u32) -> Option<u32> {
    let tokens = c.unit.ast.token_range(literal);
    if c.lexeme(tokens.start) == "<" {
        return Some(tokens.start);
    }
    if let Some(args) = c
        .collect_direct(literal)
        .into_iter()
        .find(|&kid| c.kind(kid) == NodeKind::TypeArguments)
    {
        return Some(c.unit.ast.token_range(args).start);
    }
    let parent = c.unit.ast.parent(literal)?;
    let prev = crate::rules::byte_span_of_token(c, tokens.start.checked_sub(1)?);
    (c.children(parent).start..c.children(parent).end)
        .filter(|&row| c.kind(row) == NodeKind::TypeArguments)
        .find(|&row| {
            let span = c.byte_span(row);
            span.start <= prev.start && prev.end <= span.end
        })
        .map(|row| c.unit.ast.token_range(row).start)
}

/// An element segment's `(element, key, value)` spelling triple —
/// `None` legs are the `null` types of the upstream fold
/// (`obvious_types.dart` :63-81, :18-40): only plain expressions carry an
/// element type, only `MapEntry` markers carry key/value. `None` for the
/// whole triple means the element itself is not obvious — the literal
/// bails (a `for` element, a spread of a non-obvious operand, an if
/// element with a non-obvious branch).
fn element_types<'a>(
    c: &'a Check<'a>,
    segment: &[u32],
) -> Option<(
    Option<Vec<&'a str>>,
    Option<Vec<&'a str>>,
    Option<Vec<&'a str>>,
)> {
    let &root = segment.last()?;
    match c.kind(root) {
        // `ForElement.hasObviousType` is `false` upstream (:56-57).
        NodeKind::ForControlFlow => None,
        // `SpreadElement`: the operand must be obvious (:52-53); the
        // contribution is `elementTypeOfIterable` — the operand spelling's
        // argument run when it spells `List`/`Set`, `null` otherwise (a
        // `Map` spread, an identifier whose type the token table cannot
        // name).
        NodeKind::Spread => {
            let spelling = expression_spelling(c, &segment[..segment.len() - 1])?;
            Some((iterable_argument(&spelling), None, None))
        }
        // `NullAwareElement` contributes `null` (:33-36) gated on the
        // value's obviousness (:54-55).
        NodeKind::NullAwareElement => {
            expression_spelling(c, &segment[..segment.len() - 1])?;
            Some((None, None, None))
        }
        // `IfElement`: then and else must be obvious (:49-51); the
        // contribution is the then element's own `elementType` (:26-28) —
        // `keyType`/`valueType` stay `null` even when the then side is an
        // entry (:73-81 default arms).
        NodeKind::IfControlFlow => {
            let then_pos = segment
                .iter()
                .position(|&row| c.kind(row) == NodeKind::ThenControlFlow)?;
            let else_pos = segment
                .iter()
                .position(|&row| c.kind(row) == NodeKind::ElseControlFlow);
            let then_end = else_pos.unwrap_or(segment.len() - 1);
            if let Some(else_pos) = else_pos {
                element_types(c, &segment[else_pos + 1..segment.len() - 1])?;
            }
            let (element, _, _) = element_types(c, &segment[then_pos + 1..then_end])?;
            Some((element, None, None))
        }
        NodeKind::MapEntry => {
            let (key, value) = entry_sides(c, segment)?;
            let (&marker, sides) = segment.split_last()?;
            let (key_spelling, value_spelling) = if sides.is_empty() {
                // The hand-built tiling: the side roots are the marker's
                // adopted children, not piece rows.
                (
                    expression_spelling(c, core::slice::from_ref(&key))?,
                    expression_spelling(c, core::slice::from_ref(&value))?,
                )
            } else {
                // The colon (the marker's `Colon` word) splits the side
                // piece runs — `entry_sides`' own boundary.
                let colon = c.field(marker, PayloadField::Colon)?;
                let split =
                    sides.partition_point(|&row| c.unit.ast.token_range(row).end <= colon + 1);
                (
                    expression_spelling(c, &sides[..split])?,
                    expression_spelling(c, &sides[split..])?,
                )
            };
            Some((None, Some(key_spelling), Some(value_spelling)))
        }
        _ => Some((Some(expression_spelling(c, segment)?), None, None)),
    }
}

/// The obvious-type spelling of an expression piece run (the root is its
/// last row): `obvious_spelling`, with the bare-name send face unwrapped
/// to its identifier — the production tiling wraps a bare `x` in
/// `(Identifier, NoTypeArguments, NoArguments, Send)`, which the
/// `SimpleIdentifier` arm upstream reads straight (`obvious_types.dart`
/// :169-178).
fn expression_spelling<'a>(c: &'a Check<'a>, pieces: &[u32]) -> Option<Vec<&'a str>> {
    let &root = pieces.last()?;
    if c.kind(root) == NodeKind::Send
        && pieces.len() >= 4
        && crate::rules::slice::is_tear_off_group(c, pieces, pieces.len() - 4)
    {
        return Some(Vec::new());
    }
    obvious_spelling(c, root)
}

/// The element-type leg of a spread operand's spelling: `List<T>`/`Set<T>`
/// spell `T`'s run; every other spelling (`Map<K, V>`, the unnameable
/// identifier face, bare atoms) reads as `elementTypeOfIterable`'s `null`.
fn iterable_argument<'a>(spelling: &[&'a str]) -> Option<Vec<&'a str>> {
    if spelling.len() >= 4
        && matches!(spelling[0], "List" | "Set")
        && spelling[1] == "<"
        && spelling[spelling.len() - 1] == ">"
    {
        Some(spelling[2..spelling.len() - 1].to_vec())
    } else {
        None
    }
}

/// The literal's element piece runs, in element order. Hand-built trees
/// nest the elements as direct children (one row each); the production
/// tiling leaves the childless `ListLiteral`/`SetOrMapLiteral`/`MapEntry`
/// rows flat — every piece tiles inside the literal's own completion
/// window or the owner's, so the element rows are exactly the rows whose
/// span sits inside the literal's (the literal itself excepted). A comma
/// token no piece row covers separates two elements (an interior comma —
/// a nested literal's or a call's — sits inside its root's span); each
/// comma-separated segment holds one element's piece rows in completion
/// order, so its last row is the element root — a `MapEntry` marker
/// trails its key/value pieces (see [`entry_sides`]).
fn collection_elements(c: &Check<'_>, literal: u32) -> Vec<Vec<u32>> {
    let direct = c.collect_direct(literal);
    if !direct.is_empty() {
        return direct.into_iter().map(|row| vec![row]).collect();
    }
    let span = c.byte_span(literal);
    let mut rows: Vec<u32> = (c.children(literal).start..c.children(literal).end).collect();
    if let Some(parent) = c.unit.ast.parent(literal) {
        rows.extend(c.children(parent).start..c.children(parent).end);
    }
    rows.retain(|&row| {
        row != literal && {
            let piece = c.byte_span(row);
            piece.start >= span.start && piece.end <= span.end
        }
    });
    rows.sort_unstable();
    rows.dedup();
    if rows.is_empty() {
        return Vec::new();
    }
    // The separator commas: a `,` token inside the literal's run covered
    // by no piece row.
    let tokens = c.unit.ast.token_range(literal);
    let mut separators = Vec::new();
    for token in tokens.start..tokens.end {
        if c.lexeme(token) != "," {
            continue;
        }
        let comma = crate::rules::byte_span_of_token(c, token);
        let covered = rows.iter().any(|&row| {
            let piece = c.byte_span(row);
            piece.start <= comma.start && comma.end <= piece.end
        });
        if !covered {
            separators.push(comma.start);
        }
    }
    // Group: a separator between the running segment end and the next
    // row's start splits the run (row ids ascend per element — a piece
    // contained in an earlier row's span never trips the boundary).
    let mut elements = Vec::new();
    let mut current: Vec<u32> = Vec::new();
    let mut end = 0usize;
    for row in rows {
        let piece = c.byte_span(row);
        if !current.is_empty()
            && separators
                .iter()
                .any(|&sep| end <= sep && sep < piece.start)
        {
            elements.push(core::mem::take(&mut current));
            end = 0;
        }
        end = end.max(piece.end);
        current.push(row);
    }
    elements.push(current);
    elements
}

/// A map-entry segment's `(key, value)` root rows. `segment` ends at the
/// `MapEntry` marker; the hand-built tiling keeps `[key, value]` as its
/// direct children, the production tiling tiles them as the preceding
/// piece rows split at the colon (payload word 0 — the same boundary
/// `collections.rs`'s `then_reports` reads: the key side's rows end at or
/// before the token after the colon, every value row ends past it; the
/// side's root is its last row).
fn entry_sides(c: &Check<'_>, segment: &[u32]) -> Option<(u32, u32)> {
    let (&marker, sides) = segment.split_last()?;
    if c.kind(marker) != NodeKind::MapEntry {
        return None;
    }
    let direct = c.collect_direct(marker);
    if let [key, value] = direct.as_slice() {
        return Some((*key, *value));
    }
    let colon = c.field(marker, PayloadField::Colon)?;
    let split = sides.partition_point(|&row| c.unit.ast.token_range(row).end <= colon + 1);
    Some((*sides[..split].last()?, *sides[split..].last()?))
}

/// The creation face (`Foo()`, `new Foo.named<int>()`, `const Foo()`): the
/// created type is the head with its written type arguments — the named
/// suffix is irrelevant. The argument group must be exactly `()` (the
/// landed `omit_local_variable_types` divider: an argument-bearing call
/// shape — `bar('x')` — is lexically a method call and under-reports). A
/// head naming a same-file generic class without written arguments is the
/// raw-generic face (the type needs inference — not obvious).
fn creation_spelling<'a>(c: &'a Check<'a>, tokens: core::ops::Range<u32>) -> Option<Vec<&'a str>> {
    let mut cursor = tokens.start;
    if matches!(c.lexeme(cursor), "new" | "const") {
        cursor += 1;
    }
    if !is_identifier_token(c, cursor) {
        return None;
    }
    let head = cursor;
    cursor += 1;
    while c.lexeme(cursor) == "." {
        if !is_identifier_token(c, cursor + 1) {
            return None;
        }
        cursor += 2;
    }
    let mut arguments = None;
    if c.lexeme(cursor) == "<" {
        let close = balanced_angle(c, cursor, tokens.end)?;
        arguments = Some(cursor..close + 1);
        cursor = close + 1;
    }
    if tokens.end != cursor + 2 || c.lexeme(cursor) != "(" || c.lexeme(cursor + 1) != ")" {
        return None;
    }
    let mut spelling = vec![c.lexeme(head)];
    if let Some(arguments) = arguments {
        // The closer may be one `>>`/`>>` token — the derived spelling
        // keeps the `>`-split shape the written compare works on.
        spelling.extend(expanded_lexemes(c, arguments));
    } else {
        let generic = crate::rules::same_file_declaration(c, c.lexeme(head)).is_some_and(|row| {
            c.kind(row) == NodeKind::ClassDeclaration
                && c.collect_direct(row)
                    .into_iter()
                    .any(|kid| c.kind(kid) == NodeKind::TypeVariables)
        });
        if generic {
            return None;
        }
    }
    Some(spelling)
}

/// The compound-assignment operator lexemes (`name_written_in_window`'s
/// write faces beyond plain `=` and `++`/`--`).
const ASSIGNMENT_OPERATORS: [&str; 13] = [
    "+=", "-=", "*=", "/=", "%=", "~/=", "<<=", ">>=", ">>>=", "&=", "|=", "^=", "??=",
];

/// Whether `name` is written anywhere in the token window — assigned
/// (`x = …`), compound-assigned or incremented/decremented: the lexical
/// face of `isPotentiallyMutatedInScope`. Occurrences preceded by `.` are
/// property targets, not the variable.
fn name_written_in_window(c: &Check<'_>, window: core::ops::Range<u32>, name: &str) -> bool {
    for token in window.clone() {
        if !is_identifier_token(c, token) || c.lexeme(token) != name {
            continue;
        }
        if token > 0 && c.lexeme(token - 1) == "." {
            continue;
        }
        if token + 1 < window.end {
            let next = c.lexeme(token + 1);
            if next == "=" || next == "++" || next == "--" || ASSIGNMENT_OPERATORS.contains(&next) {
                return true;
            }
        }
        if token > window.start && matches!(c.lexeme(token - 1), "++" | "--") {
            return true;
        }
    }
    false
}

/// The token ranges of the nested function subtrees inside `owner`'s
/// completion window (closures, named and local functions — the rows whose
/// bodies may capture).
fn closure_windows(c: &Check<'_>, owner: u32) -> Vec<core::ops::Range<u32>> {
    (c.children(owner).start..=owner)
        .filter(|&row| {
            matches!(
                c.kind(row),
                NodeKind::FunctionExpression
                    | NodeKind::NamedFunctionExpression
                    | NodeKind::LocalFunctionDeclaration
            )
        })
        .map(|row| c.unit.ast.token_range(row))
        .collect()
}

/// Whether `name` occurs inside any of the closure windows — the
/// capture-face bail of `prefer_final_in_for_each`: a reference inside a
/// nested function is judged unreadable, so any capture skips the report.
fn name_captured(c: &Check<'_>, closures: &[core::ops::Range<u32>], name: &str) -> bool {
    closures.iter().any(|window| {
        window
            .clone()
            .any(|token| is_identifier_token(c, token) && c.lexeme(token) == name)
    })
}

/// The UTF-16 span of a token range (the written-type report spans).
/// `None` on the shared token-helper miss — the caller's report drops.
pub(crate) fn utf16_token_run_span(
    c: &Check<'_>,
    range: core::ops::Range<u32>,
) -> Option<core::ops::Range<u32>> {
    let start = c.utf16_of_token(range.start)?;
    let end = c.utf16_end_of_token(range.end - 1)?;
    Some(start..end)
}

/// The iterable expression row of a for-each declaration: the sibling
/// before the parts child of the owning [`NodeKind::ForIn`].
fn for_each_iterable(c: &Check<'_>, node: u32) -> Option<u32> {
    let parent = c.unit.ast.parent(node)?;
    let kids = c.collect_direct(parent);
    let position = kids
        .iter()
        .position(|&kid| c.kind(kid) == NodeKind::ForInParts)?;
    kids.get(position.checked_sub(1)?).copied()
}

/// Whether every declarator of the row carries an initializer that is
/// obvious and spells exactly the written type (the
/// `_visitVariableDeclarationList` loop of the 2024 omit rules).
fn every_initializer_obvious(c: &Check<'_>, row: u32, written: (u32, u32)) -> bool {
    let mut declarators = 0;
    for var in c.collect_direct(row) {
        if c.kind(var) != NodeKind::InitializedIdentifier {
            continue;
        }
        declarators += 1;
        let initializer = c
            .collect_direct(var)
            .into_iter()
            .find(|&kid| c.kind(kid) == NodeKind::VariableInitializer);
        let Some(value) = initializer.and_then(|init| crate::rules::last_child(c, init)) else {
            return false;
        };
        match obvious_spelling(c, value) {
            Some(spelling) if !spelling.is_empty() && spelling_matches(c, written, &spelling) => {}
            _ => return false,
        }
    }
    declarators > 0
}

/// Whether any token in the range writes `const` (the constructor /
/// declaration-header const faces).
fn range_has_const(c: &Check<'_>, range: core::ops::Range<u32>) -> bool {
    range.clone().any(|token| c.lexeme(token) == "const")
}

/// Whether the constructor writes `const` before its name — upstream
/// `element.isConst` reads `constKeyword != null`. The Constructor row
/// anchors at the declared name (`begin_constructor` passes `name`), so a
/// `const`/`external` modifier lands *before* the span head; the scan
/// walks back from the head over the modifier spellings rather than
/// scanning forward into the row.
fn ctor_header_has_const(c: &Check<'_>, ctor: u32) -> bool {
    let mut cursor = c.head_token(ctor);
    while cursor > 0 {
        match c.lexeme(cursor - 1) {
            "const" => return true,
            "external" | "static" | "covariant" | "augment" | "factory" => {
                cursor -= 1;
            }
            _ => return false,
        }
    }
    false
}

// The mounting seam: the pubspec pass at the mounting face wires the two
// pure faces (the crate-root re-export alongside the #1153 promotion, the
// `secure_pubspec_urls` shape) — the per-unit walk lives in the engine's
// `run_breaking_version_annotations`.

/// The verdict row of a variable's initializer expression (the
/// `use_enums` candidate read): the send group's `Send` row when one
/// tiles in the `VariableInitializer` — a member read trails its access
/// marker *after* the send, so the last row is not always the verdict
/// row — else the last row (literals, `const` wrappers and structured
/// creations seal last).
fn initializer_expression(c: &Check<'_>, var: u32) -> Option<u32> {
    let init = c
        .collect_direct(var)
        .into_iter()
        .find(|&kid| c.kind(kid) == NodeKind::VariableInitializer)?;
    let kids = c.collect_direct(init);
    // The expression's outer tail: the last creation-shaped row of the
    // flat send-chain (a `DotAccess` marker rides after its send, so the
    // invoke send is the chain's last `Send`, not the row list's last).
    kids.iter()
        .rfind(|&&kid| {
            matches!(
                c.kind(kid),
                NodeKind::Send
                    | NodeKind::NewExpression
                    | NodeKind::ImplicitCreation
                    | NodeKind::ConstExpression
            )
        })
        .or_else(|| kids.last())
        .copied()
}

// —— the #1157 wave L1 face: the nine `RuleState.internal()` SDK-dogfooding
// rules of the #1157 wave. Upstream registers them
// unconditionally and enablement is name-only (registry.dart:117-137), so
// naming one in `analysis_options` silently enables it — registering the
// names here is what honors that face. They carry `kinds: &[]` and no
// check: every firing path's antecedent is an SDK-internal artifact (the
// SDK's own annotations or the `package:analyzer/*` URI gate), so the rule
// cannot fire outside SDK sources. All nine join `STRUCTURALLY_SILENT`
// (`tests/registry_pin.rs`) with `lint_divergences.tsv` `excluded` rows.

/// The same-file class-hierarchy face of [`implicit_reopen`], built once per
/// run on first query and cached in the run's
/// [`crate::engine::RunCaches::class_hierarchy`]: the extends-able class
/// rows by name plus each row's effective class-family flags — the written
/// `ModSet` bits, plus the sealed induction for `sealed` rows.
pub(crate) struct ClassHierarchy {
    /// Class-like row by name for the kinds an `extends` clause can name
    /// (`ClassDeclaration` and `NamedMixinApplication` — a class type alias
    /// is a `ClassElement` upstream); the first declaration wins per name.
    classes: FxHashMap<String, u32>,
    /// `ClassDeclaration`/`NamedMixinApplication` row -> effective
    /// class-family flags (written + the sealed induction).
    flags: FxHashMap<u32, u16>,
}

impl ClassHierarchy {
    /// The row the extends-able same-file declaration `name` resolves to.
    fn row(&self, name: &str) -> Option<u32> {
        self.classes.get(name).copied()
    }

    /// The effective class-family flags of a walked row.
    fn flags_of(&self, row: u32) -> Option<u16> {
        self.flags.get(&row).copied()
    }
}

/// The unit's [`ClassHierarchy`] queries: the face builds once per run on
/// first query (one pass over the top-level wrappers in declaration order,
/// inducing the sealed modifiers as it walks) and the reads copy single
/// values out — the borrow never crosses a statement, so no guard can
/// outlive the call. The order matters and is official behavior:
/// `_setInducedModifier` runs when the class is first visited (its own
/// declaration position, or the supertype recursion of an earlier
/// subclass), reading the supertypes' flags as they stand — a sealed chain
/// is induced in declaration order, so a supertype declared after its
/// subclass reads uninduced flags (`instance_member_inferrer.dart:337-350`:
/// induction precedes the supertype recursion, and the recursion visits
/// nothing that a later index would visit differently).
pub(crate) fn hierarchy_row(c: &Check<'_>, name: &str) -> Option<u32> {
    ensure_class_hierarchy(c);
    c.caches
        .class_hierarchy
        .borrow()
        .as_ref()
        .and_then(|face| face.row(name))
}

pub(crate) fn hierarchy_flags(c: &Check<'_>, row: u32) -> Option<u16> {
    c.caches
        .class_hierarchy
        .borrow()
        .as_ref()
        .and_then(|face| face.flags_of(row))
}

fn ensure_class_hierarchy(c: &Check<'_>) {
    if c.caches.class_hierarchy.borrow().is_some() {
        return;
    }
    let face = build_class_hierarchy(c);
    *c.caches.class_hierarchy.borrow_mut() = Some(face);
}

fn build_class_hierarchy(c: &Check<'_>) -> ClassHierarchy {
    let mut face = ClassHierarchy {
        classes: FxHashMap::default(),
        flags: FxHashMap::default(),
    };
    let Some(root) = c.unit.ast.root() else {
        return face;
    };
    for top in c.collect_direct(root) {
        for inner in c.collect_direct(top) {
            match c.kind(inner) {
                NodeKind::ClassDeclaration | NodeKind::NamedMixinApplication => {
                    if let Some(name) = container_name_node(c, inner) {
                        face.classes
                            .entry(c.lexeme(c.head_token(name)).to_owned())
                            .or_insert(inner);
                    }
                    let written = dartq_ast::mod_flags(c.unit.ast, inner);
                    let induced = if written & class_bits::SEALED != 0 {
                        sealed_induction(c, inner, &face)
                    } else {
                        0
                    };
                    face.flags.insert(inner, written | induced);
                }
                _ => {}
            }
        }
    }
    face
}

/// The effective flags a supertype-position name carries: a walked class
/// row answers with its effective flags (an earlier declaration is already
/// induced); a mixin or enum answers with its written flags (induction only
/// touches classes, `_setInducedModifier`'s `ClassElementImpl` gate); a
/// name that leaves the file answers `None` — the official reads the
/// resolved element there, so the same-file cut is the documented
/// under-approximation.
fn declared_class_flags(c: &Check<'_>, face: &ClassHierarchy, name: &str) -> Option<u16> {
    if let Some(row) = face.row(name) {
        return face.flags_of(row);
    }
    crate::rules::same_file_declaration(c, name).map(|row| dartq_ast::mod_flags(c.unit.ast, row))
}

/// `_setInducedModifier` (`instance_member_inferrer.dart:643-722`): the
/// modifier a `sealed` declaration carries by constraint of its own
/// supertypes, reading the headers' flags as they stand at the walk
/// position. Each step mirrors the official branch order.
fn sealed_induction(c: &Check<'_>, row: u32, face: &ClassHierarchy) -> u16 {
    let (supertype, mixins, interfaces) = header_supertype_flags(c, row, face);
    let any =
        |flags: &[Option<u16>], bit: u16| flags.iter().any(|f| f.is_some_and(|f| f & bit != 0));
    // A direct superclass which is 'final' — or a 'final' mixin — induces
    // 'final' (:650-660).
    if any(&mixins, class_bits::FINAL) {
        return class_bits::FINAL;
    }
    if supertype.is_some_and(|f| f & class_bits::FINAL != 0) {
        return class_bits::FINAL;
    }
    if let Some(f) = supertype {
        // A 'base' superclass: 'final' when a mixin is 'interface', else
        // the constraint flows as 'base' (:662-673).
        if f & class_bits::BASE != 0 {
            return if any(&mixins, class_bits::INTERFACE) {
                class_bits::FINAL
            } else {
                class_bits::BASE
            };
        }
        // An 'interface' superclass: 'final' when a superinterface is
        // 'base', else 'interface' (:674-693).
        if f & class_bits::INTERFACE != 0 {
            return if any(&interfaces, class_bits::BASE) || any(&mixins, class_bits::BASE) {
                class_bits::FINAL
            } else {
                class_bits::INTERFACE
            };
        }
    }
    // A 'base' or 'final' superinterface induces 'base' (:695-704).
    if any(&interfaces, class_bits::BASE | class_bits::FINAL)
        || any(&mixins, class_bits::BASE | class_bits::FINAL)
    {
        return class_bits::BASE;
    }
    // An 'interface' mixin: 'final' when an interface is 'base', else
    // 'interface' (:706-719).
    if any(&mixins, class_bits::INTERFACE) {
        return if any(&interfaces, class_bits::BASE) {
            class_bits::FINAL
        } else {
            class_bits::INTERFACE
        };
    }
    0
}

/// The effective flags of a `sealed` row's own header positions: the direct
/// superclass (the `extends` name, or the mixin application's first type),
/// the `with` mixins, and the `implements` tail — each resolved through
/// [`declared_class_flags`].
fn header_supertype_flags(
    c: &Check<'_>,
    row: u32,
    face: &ClassHierarchy,
) -> (Option<u16>, Vec<Option<u16>>, Vec<Option<u16>>) {
    let supertypes = crate::rules::supertypes(c, row);
    let mut supertype = None;
    let mut mixins = Vec::new();
    let mut interfaces = Vec::new();
    for (index, (name, clause)) in supertypes.iter().enumerate() {
        // A mixin application's superclass is its first type row — the
        // clause walk finds no `extends` keyword before it (collapse map:
        // the `=` form has none).
        let is_super = if c.kind(row) == NodeKind::NamedMixinApplication {
            index == 0
        } else {
            *clause == SupertypeClause::Extends
        };
        let flags = declared_class_flags(c, face, name);
        if is_super {
            supertype = flags;
        } else if *clause == SupertypeClause::With {
            mixins.push(flags);
        } else {
            interfaces.push(flags);
        }
    }
    (supertype, mixins, interfaces)
}

// Per-rule modules (mechanical split 2026-10-19; registry paths unchanged).
pub(crate) mod avoid_return_types_on_setters;
pub(crate) mod unnecessary_late;
pub(crate) mod use_function_type_syntax_for_parameters;
pub(crate) mod prefer_typing_uninitialized_variables;
pub(crate) mod prefer_generic_function_type_aliases;
pub(crate) mod unnecessary_constructor_name;
pub(crate) mod hash_and_equals;
pub(crate) mod avoid_init_to_null;
pub(crate) mod prefer_function_declarations_over_variables;
pub(crate) mod type_init_formals;
pub(crate) mod recursive_getters;
pub(crate) mod unnecessary_getters_setters;
pub(crate) mod unnecessary_nullable_for_final_variable_declarations;
pub(crate) mod prefer_initializing_formals;
pub(crate) mod annotate_overrides;
pub(crate) mod avoid_renaming_method_parameters;
pub(crate) mod overridden_fields;
pub(crate) mod unnecessary_overrides;
pub(crate) mod always_declare_return_types;
pub(crate) mod always_put_required_named_parameters_first;
pub(crate) mod avoid_final_parameters;
pub(crate) mod avoid_classes_with_only_static_members;
pub(crate) mod avoid_multiple_declarations_per_line;
pub(crate) mod avoid_private_typedef_functions;
pub(crate) mod avoid_returning_this;
pub(crate) mod avoid_unused_constructor_parameters;
pub(crate) mod avoid_types_on_closure_parameters;
pub(crate) mod initialize_in_field_declaration;
pub(crate) mod empty_container_bodies;
pub(crate) mod unnecessary_const_in_enum_constructor;
pub(crate) mod unnecessary_type_name_in_constructor;
pub(crate) mod use_primary_constructors;
pub(crate) mod var_with_no_type_annotation;
pub(crate) mod prefer_final_parameters;
pub(crate) mod avoid_void_async;
pub(crate) mod async_return_with_no_await;
pub(crate) mod unnecessary_primary_constructor_body;
pub(crate) mod prefer_asserts_in_initializer_lists;
pub(crate) mod test_types_in_equals;
pub(crate) mod avoid_positional_boolean_parameters;
pub(crate) mod avoid_field_initializers_in_const_classes;
pub(crate) mod use_declaring_parameters;
pub(crate) mod avoid_equals_and_hash_code_on_mutable_classes;
pub(crate) mod avoid_setters_without_getters;
pub(crate) mod use_setters_to_change_properties;
pub(crate) mod use_to_and_as_if_applicable;
pub(crate) mod deprecated_consistency;
pub(crate) mod unnecessary_async;
pub(crate) mod omit_local_variable_types;
pub(crate) mod annotate_redeclares;
pub(crate) mod avoid_implementing_value_types;
pub(crate) mod omit_obvious_local_variable_types;
pub(crate) mod omit_obvious_property_types;
pub(crate) mod prefer_const_constructors_in_immutables;
pub(crate) mod prefer_final_in_for_each;
pub(crate) mod prefer_final_locals;
pub(crate) mod specify_nonobvious_local_variable_types;
pub(crate) mod specify_nonobvious_property_types;
pub(crate) mod use_late_for_private_fields_and_variables;
pub(crate) mod deprecated_member_use_from_same_package;
pub(crate) mod remove_deprecations_in_breaking_versions;
pub(crate) mod prefer_const_declarations;
pub(crate) mod prefer_mixin;
pub(crate) mod use_enums;
pub(crate) mod analyzer_element_model_tracking_bad;
pub(crate) mod analyzer_element_model_tracking_more_than_one;
pub(crate) mod analyzer_element_model_tracking_zero;
pub(crate) mod analyzer_public_api_bad_part_directive;
pub(crate) mod analyzer_public_api_bad_type;
pub(crate) mod analyzer_public_api_experimental_inconsistency;
pub(crate) mod analyzer_public_api_exports_non_public_name;
pub(crate) mod analyzer_public_api_impl_in_public_api;
pub(crate) mod analyzer_to_be_deprecated_use;
/// The class-family `ModSet` bits (bridge `mod_bits`) the
/// rule reads. They live in `NodeHeader.flags` bits 1..=7; the member family
/// shares positions on disjoint kinds.
mod class_bits;
pub(crate) mod implicit_reopen;
#[cfg(test)]
mod implicit_reopen_tests;
#[cfg(test)]
mod tests;
/// The W5 (#1124) approximation pins over real parser trees.
#[cfg(test)]
mod w5_approx_pins;
pub(crate) mod unreachable_from_main;
#[cfg(test)]
mod unreachable_from_main_tests;
