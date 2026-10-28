use super::{Check, NodeKind, RuleSpec, same_file};
use crate::rules::{NONE_TOKEN, utf16_token_span};

/// Official: `pkg/linter/lib/src/rules/unnecessary_this.dart` — a `this.`
/// qualifier reports at the `this` keyword unless shadowing makes it
/// necessary. Two faces (:37-45): a constructor field initializer's
/// `this` reports unconditionally when written (:31-35 — `x = e;` is
/// the bare form), and a `this` in a property access / method invocation
/// reports when the unqualified name would resolve to the same member
/// (:47-64).
///
/// Decision order (the official `resolveNameInScope` + element identity,
/// :66-99): locals, parameters, and inner declarations shadow (the
/// `this.` stays — a scope hit that is not the member makes the
/// qualified form necessary, :87-95); the name scope then covers the
/// unit's own declarations AND unprefixed imports, so a same-name hit
/// there bails; only with no scope hit (`isNone` — the member is
/// inherited, or declared on the class itself and reachable through the
/// instance interface) does the `this.` report.
///
/// The unit's own declarations bail through [`same_file::declares_name`];
/// the import layer rides the semantic bridge's import face
/// ([`Analysis::import_shadows`]): a prefixed import contributes its
/// prefix spelling alone, an unprefixed import contributes its target's
/// combinator-filtered export set, and an out-of-graph target keeps the
/// conservative `true` — the `this.` silences, never reporting over a
/// possible shadow. A mount without semantics degrades to the no-bail
/// face (the lexical import guess is gone). Message:
/// `diagnostic.g.dart` `unnecessaryThis`.
pub(crate) static SPEC: RuleSpec = crate::spec!(
    "unnecessary_this",
    &[NodeKind::ThisExpression, NodeKind::Initializer],
    check,
);

fn check(c: &mut Check<'_>) {
    match c.kind(c.node()) {
        NodeKind::Initializer => check_field_initializer(c),
        NodeKind::ThisExpression => check_this(c),
        _ => {}
    }
}

/// `visitConstructorFieldInitializer` (:31-35): the written `this` on a
/// field initializer always reports — the bare `x = e;` form exists.
/// The `Initializer` node owns the `this` prefix (its span head; the
/// `FieldInitializer` child begins at the `=`). The official reports
/// at the `this` token (`reportAtToken(thisKeyword)`).
fn check_field_initializer(c: &mut Check<'_>) {
    if c.lexeme(c.head_token(c.node())) != "this" {
        return;
    }
    let range = utf16_token_span(c, c.head_token(c.node()));
    c.report(range, &[]);
}

/// `visitThisExpression` (:37-64): `this.name` accesses only. The
/// production shape (bridge): a `this` owns no children — the access
/// pieces tile flat into the enclosing owner as `this`, the selector,
/// the tear-off wrappers, and the trailing `DotAccess` marker (same
/// span, dot token + null-aware word).
fn check_this(c: &mut Check<'_>) {
    let node = c.node();
    let Some(parent) = c.ancestor(1) else {
        return;
    };
    if c.kind(parent) == NodeKind::Initializer {
        // A constructor field initializer owns its report (:31-35).
        return;
    }
    let kids = c.collect_direct(parent);
    let Some(pos) = kids.iter().position(|&kid| kid == node) else {
        return;
    };
    let Some(&selector) = kids.get(pos + 1) else {
        return;
    };
    if c.kind(selector) != NodeKind::Identifier {
        return;
    }
    // The `DotAccess` marker rides the expression piece run — a flat
    // sibling for `this.width;`, or inside an adopted container's
    // window for `this.width * 2` (the binary owns the marker). Walk
    // the run's rows and each container's window for the marker whose
    // span covers the selector.
    let selector_span = c.utf16_span(selector);
    let mut stack: Vec<u32> = kids[pos + 2..].to_vec();
    let access = loop {
        let Some(row) = stack.pop() else {
            return;
        };
        if c.kind(row) == NodeKind::DotAccess {
            let span = c.utf16_span(row);
            if span.start <= selector_span.start && span.end >= selector_span.end {
                break row;
            }
        }
        stack.extend(c.collect_direct(row));
    };
    // `!parent.isNullAware` (:49/:53) — the payload word is the
    // `is_null_aware` bool of the `DotAccess` payload.
    if c.payload(access).get(1).copied().unwrap_or(0) != 0 {
        return;
    }
    let name_lexeme = c.lexeme(c.head_token(selector));
    if member_shadows(c, node, name_lexeme) {
        return;
    }
    // The library-scope half of the name scope (:66-83): the import
    // face — an import bringing `name` into the unit scope makes the
    // `this.` necessary. Out-of-graph targets answer the conservative
    // `true`; a mount without semantics carries no import face and
    // keeps reporting.
    if let Some(analysis) = c.semantics()
        && analysis.import_shadows(analysis.strings().intern(name_lexeme))
    {
        return;
    }
    if same_file::declares_name(c, name_lexeme) {
        return;
    }
    // No scope hit (`isNone`): the unqualified form resolves through
    // the instance interface — the enclosing class declares the member,
    // or it is inherited (:87-95 element identity). The official
    // reports at the `this` token (`reportAtToken(node.thisKeyword)`).
    let range = utf16_token_span(c, c.head_token(node));
    c.report(range, &[]);
}

/// Whether any declaration visible between the use and the enclosing
/// member binds `name`. The walk stops at the member, after
/// collecting its own parameters.
fn member_shadows(c: &Check<'_>, use_node: u32, name: &str) -> bool {
    let use_start = c.byte_span(use_node).start;
    for &ancestor in c.path.iter().rev().skip(1) {
        match c.kind(ancestor) {
            // The member boundary: its parameters shadow, then stop.
            NodeKind::Method
            | NodeKind::Constructor
            | NodeKind::Factory
            | NodeKind::TopLevelMethod
            | NodeKind::PrimaryConstructorBody => {
                if collect_member_parameters(c, ancestor).contains(&name) {
                    return true;
                }
                return false;
            }
            // The production tiling keeps a block's statements flat in
            // the owner slice — locals precede the use as pieces.
            NodeKind::Block | NodeKind::BlockBody => {
                if block_declares_before(c, ancestor, use_start, name) {
                    return true;
                }
            }
            NodeKind::CatchClause | NodeKind::ForIn => {
                if scope_declares(c, ancestor, name) {
                    return true;
                }
            }
            NodeKind::FunctionExpression | NodeKind::NamedFunctionExpression => {
                // Parameters scope the whole body regardless of position.
                if collect_member_parameters(c, ancestor).contains(&name) {
                    return true;
                }
            }
            NodeKind::LocalFunctionDeclaration => {
                // The function name binds from its declaration on; the
                // parameters scope the whole body. The name nests under
                // `FunctionName` — `member_declaration_name` reaches it.
                let names_decl =
                    crate::rules::declarations::member_declaration_name(c, ancestor)
                        .is_some_and(|decl| {
                            c.lexeme(c.head_token(decl)) == name
                                && c.byte_span(decl).start < use_start
                        });
                if names_decl || collect_member_parameters(c, ancestor).contains(&name) {
                    return true;
                }
            }
            _ => {}
        }
    }
    false
}

/// Local declarations of a block that precede the use. The production
/// shape: the name identifier carries the local-variable context word
/// inside its `VariablesDeclaration` slice; pattern bindings keep their
/// statement node.
fn block_declares_before(c: &Check<'_>, block: u32, use_start: usize, name: &str) -> bool {
    let ctx = dartq_parser::IdentifierContext::LocalVariableDeclaration as u32;
    for decl in c.collect_direct(block) {
        match c.kind(decl) {
            NodeKind::VariablesDeclaration => {
                if c.collect_direct(decl).into_iter().any(|kid| {
                    c.kind(kid) == NodeKind::Identifier
                        && c.payload(kid).first() == Some(&ctx)
                        && c.lexeme(c.head_token(kid)) == name
                        && c.byte_span(kid).start < use_start
                }) {
                    return true;
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

/// Any name binding directly inside a catch clause / for-in header.
fn scope_declares(c: &Check<'_>, scope: u32, name: &str) -> bool {
    let window = c.children(scope);
    for id in window.start..window.end {
        if c.kind(id) == NodeKind::Identifier && c.lexeme(c.head_token(id)) == name {
            return true;
        }
    }
    false
}

/// The parameter names of a member or closure (the `FormalParameters`
/// child's `FormalParameter` name words).
fn collect_member_parameters<'a>(c: &'a Check<'a>, member: u32) -> Vec<&'a str> {
    let Some(params) = c
        .collect_direct(member)
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
            (name != NONE_TOKEN).then(|| c.lexeme(name))
        })
        .collect()
}
