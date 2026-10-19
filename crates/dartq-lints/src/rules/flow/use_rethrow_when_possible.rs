use super::{Check, Group, NodeKind, RuleSpec};
use crate::rules::{
    byte_span_of_token, initialized_identifier_name, tail_token, utf16_token_span,
};

/// Official: `pkg/linter/lib/src/rules/use_rethrow_when_possible.dart:36-46`
/// — a `throw` of exactly the caught exception parameter, as a statement,
/// reports at the throw. The parameter identity
/// (`element == exceptionParameter`, :39-44) resolves lexically: the
/// thrown identifier must name the nearest enclosing catch's parameter
/// and not be shadowed between the throw and that catch clause — a
/// same-lexeme local variable, pattern variable, loop-variable
/// declaration, or closure parameter inside the catch body binds the
/// read instead. The catch clause is the walk's boundary: its parameter
/// is the candidate itself, never a shadow. The remaining lexical face
/// (documented): a sibling local-function declaration sharing the name
/// shadows in the official scope but is not walked here. Message:
/// `diagnostic.g.dart` `useRethrowWhenPossible`.
pub(crate) static SPEC: RuleSpec = RuleSpec {
    name: "use_rethrow_when_possible",
    group: Group::Recommended,
    problem: "Use 'rethrow' to rethrow a caught exception.",
    correction: "Try replacing the 'throw' with a 'rethrow'.",
    kinds: &[NodeKind::ThrowExpression],
    check,
};

fn check(c: &mut Check<'_>) {
    // `node.parent is! ExpressionStatement` bails (:37). The production
    // tiling seals the statement as a childless handle right after the
    // throw (the operand rows leak beside it), so the face reads the
    // owner run's next row when the parent itself is not the statement.
    let stmt = match c.ancestor(1) {
        Some(s) if c.kind(s) == NodeKind::ExpressionStatement => s,
        Some(parent) => {
            let kids = c.collect_direct(parent);
            match kids
                .iter()
                .position(|&kid| kid == c.node())
                .and_then(|pos| kids.get(pos + 1).copied())
            {
                Some(next) if c.kind(next) == NodeKind::ExpressionStatement => next,
                _ => return,
            }
        }
        None => return,
    };
    // `node.thisOrAncestorOfType<CatchClause>()` (:41).
    let Some(catch_clause) = c
        .path
        .iter()
        .rev()
        .copied()
        .find(|&n| c.kind(n) == NodeKind::CatchClause)
    else {
        return;
    };
    // The exception parameter is the catch's first formal (a stack-trace
    // formal follows it; an `on`-only catch has none and never fires —
    // `exceptionParameter` null, :42-43). Production parses wrap the
    // formals in a `FormalParameters` container (the bridge opens it at
    // the `(`); hand-built trees may nest the parameter directly.
    let formals = c.collect_direct(catch_clause);
    let exception = formals
        .iter()
        .copied()
        .find(|&kid| c.kind(kid) == NodeKind::FormalParameters)
        .map(|list| c.collect_direct(list))
        .unwrap_or(formals)
        .into_iter()
        .find(|&kid| c.kind(kid) == NodeKind::FormalParameter);
    let Some(exception) = exception else {
        return;
    };
    // The parameter name is payload word 1 (the
    // `FormalParameterEnd` layout).
    let Some(&name) = c.payload(exception).get(1) else {
        return;
    };
    // The thrown expression is exactly the parameter name (upstream
    // `node.expression.canonicalElement == exceptionParameter` — a
    // SimpleIdentifier resolving to the parameter): the statement's
    // token run between the throw keyword and the trailing `;` answers.
    // Production parses leave the throw a childless handle whose
    // operand rows precede it as siblings, so exactly one
    // identifier-shaped token in the run is the simple-identifier face
    // (reserved words cannot name parameters; built-in keywords can).
    let throw_tok = c.head_token(c.node());
    let operand_tok = throw_tok + 1;
    if tail_token(c, stmt) != operand_tok + 1 {
        return;
    }
    let Some(token) = c.unit.tokens.get(operand_tok as usize) else {
        return;
    };
    let thrown = match token.kind {
        dartq_scanner::TokenKind::Identifier => c.lexeme(operand_tok),
        dartq_scanner::TokenKind::Keyword(kw)
            if kw.style() == dartq_scanner::KeywordStyle::BuiltIn =>
        {
            c.lexeme(operand_tok)
        }
        _ => return,
    };
    if thrown == c.lexeme(name) && !shadowed_between(c, operand_tok, catch_clause, thrown) {
        // The official report anchors at the throw expression node —
        // `throw` through the operand (`reportAtNode`, :45).
        let range = utf16_token_span(c, throw_tok).start..utf16_token_span(c, operand_tok).end;
        c.report(range, &[]);
    }
}

/// Whether any declaration visible between the thrown operand and the
/// catch clause binds `name` — the lexical face of the official element
/// identity: the innermost binding of a read inside the catch body
/// wins, so a same-name declaration below the catch parameter makes the
/// thrown identifier a different element.
fn shadowed_between(c: &Check<'_>, use_tok: u32, catch_clause: u32, name: &str) -> bool {
    let use_start = byte_span_of_token(c, use_tok).start;
    for &ancestor in c.path.iter().rev().skip(1) {
        if ancestor == catch_clause {
            return false;
        }
        match c.kind(ancestor) {
            NodeKind::Block => {
                if block_declares_before(c, ancestor, use_start, name) {
                    return true;
                }
            }
            NodeKind::ForIn => {
                if scope_declares(c, ancestor, name) {
                    return true;
                }
            }
            NodeKind::ForParts => {
                // `for (var e = …; …)` declares in the loop parts,
                // outside the body block.
                if for_parts_declares(c, ancestor, name) {
                    return true;
                }
            }
            NodeKind::FunctionExpression | NodeKind::NamedFunctionExpression => {
                // Parameters scope the whole body regardless of position.
                if parameter_names(c, ancestor).contains(&name) {
                    return true;
                }
            }
            NodeKind::LocalFunctionDeclaration => {
                // The function name binds from its declaration on — it
                // nests under the `FunctionName` child on the production
                // tiling — and the parameters scope the whole body.
                let names_decl =
                    crate::rules::declarations::member_declaration_name(c, ancestor)
                        .is_some_and(|decl| {
                            c.lexeme(c.head_token(decl)) == name
                                && c.byte_span(decl).start < use_start
                        });
                if names_decl || parameter_names(c, ancestor).contains(&name) {
                    return true;
                }
            }
            _ => {}
        }
    }
    false
}

/// Local declarations of a block that precede the use: variables
/// (`VariablesDeclaration`) and pattern bindings. The variable name
/// rides its [`NodeKind::InitializedIdentifier`] row's span head (the
/// production row is width-0 at the name — the Identifier row the
/// parser emits precedes the wrapper, never inside it).
fn block_declares_before(c: &Check<'_>, block: u32, use_start: usize, name: &str) -> bool {
    for decl in c.collect_direct(block) {
        match c.kind(decl) {
            NodeKind::VariablesDeclaration => {
                for var in c.collect_direct(decl) {
                    if c.kind(var) != NodeKind::InitializedIdentifier {
                        continue;
                    }
                    if let Some(var_name) = initialized_identifier_name(c, var)
                        && c.lexeme(var_name) == name
                        && byte_span_of_token(c, var_name).start < use_start
                    {
                        return true;
                    }
                }
            }
            NodeKind::PatternVarDeclStatement => {
                let window = c.children(decl);
                for id in window.start..window.end {
                    if c.kind(id) == NodeKind::Identifier
                        && c.lexeme(c.head_token(id)) == name
                        && c.byte_span(id).start < use_start
                    {
                        return true;
                    }
                }
            }
            _ => {}
        }
    }
    false
}

/// Any name binding directly inside a for-in header.
fn scope_declares(c: &Check<'_>, scope: u32, name: &str) -> bool {
    let window = c.children(scope);
    for id in window.start..window.end {
        if c.kind(id) == NodeKind::Identifier && c.lexeme(c.head_token(id)) == name {
            return true;
        }
    }
    false
}

/// The loop-variable declarations of a classic for header (the name
/// read mirrors [`block_declares_before`]'s span-head face).
fn for_parts_declares(c: &Check<'_>, parts: u32, name: &str) -> bool {
    c.collect_direct(parts).into_iter().any(|decl| {
        c.kind(decl) == NodeKind::VariablesDeclaration
            && c.collect_direct(decl).into_iter().any(|var| {
                c.kind(var) == NodeKind::InitializedIdentifier
                    && initialized_identifier_name(c, var)
                        .is_some_and(|var_name| c.lexeme(var_name) == name)
            })
    })
}

/// The parameter names of a closure or local function (the
/// `FormalParameters` child's `FormalParameter` name words).
fn parameter_names<'a>(c: &'a Check<'a>, function: u32) -> Vec<&'a str> {
    let Some(params) = c
        .collect_direct(function)
        .into_iter()
        .find(|&kid| c.kind(kid) == NodeKind::FormalParameters)
    else {
        return Vec::new();
    };
    c.collect_direct(params)
        .into_iter()
        .filter(|&param| c.kind(param) == NodeKind::FormalParameter)
        .filter_map(|param| {
            let name = c.payload(param).get(1).copied()?;
            (name != super::NONE_TOKEN).then(|| c.lexeme(name))
        })
        .collect()
}
