use super::{Check, Group, NodeKind, RuleSpec, member_declaration_name, operator_name};

/// Official: `pkg/linter/lib/src/rules/hash_and_equals.dart:45-80` — a
/// class defining exactly one of `==` / `hashCode` reports at the
/// defined member's name. The member scan runs over the
/// `DeclarationBody` children: `==` is a `Method` whose `OperatorName`
/// payload token lexemes to `==`, `hashCode` a `Method` name or a field
/// variable of that name. The element re-checks
/// (`node.hasMethod('==')` etc.) collapse into the same scan — the blind
/// spot is members the syntax shape misclassifies (none today). Message:
/// `diagnostic.g.dart` `hashAndEquals`.
pub(crate) static SPEC: RuleSpec = RuleSpec {
    name: "hash_and_equals",
    group: Group::Core,
    problem: "Missing a corresponding override of '{0}'.",
    correction: "Try overriding '{0}' or removing '{1}'.",
    kinds: &[NodeKind::ClassDeclaration],
    check,
};

fn check(c: &mut Check<'_>) {
    let node = c.node();
    let Some(body) = c
        .direct_children(node)
        .iter()
        .copied()
        .find(|&kid| c.kind(kid) == NodeKind::DeclarationBody)
    else {
        return;
    };
    let mut eq = None;
    let mut hash = None;
    // Collect first: the members scan re-enters `direct_children` for
    // each member, and the scratch RefCell forbids a live outer guard.
    let members: Vec<u32> = c.direct_children(body).iter().copied().collect();
    for member in members {
        if is_equals(c, member) {
            eq = Some(member);
        } else if is_hash_code(c, member) {
            hash = Some(member);
        }
    }
    let (eq, hash) = match (eq, hash) {
        (None, None) => return,
        pair => pair,
    };
    if eq.is_none() {
        let hash = hash.expect("one side is defined past the pair match");
        if let Some(name) = member_or_field_name(c, hash) {
            report_at(c, name, &["==", "hashCode"]);
        }
    }
    if hash.is_none() {
        let eq = eq.expect("one side is defined past the pair match");
        if let Some(name) = member_name_of(c, eq) {
            report_at(c, name, &["hashCode", "=="]);
        }
    }
}

/// `isEquals` (`ast.dart`): a `Method` whose operator name lexemes `==`.
/// The operator name rides the member's preceding sibling on production
/// parses and a direct child on hand-built tables
/// ([`operator_name`]).
fn is_equals(c: &Check<'_>, member: u32) -> bool {
    c.kind(member) == NodeKind::Method
        && operator_name(c, member)
            .is_some_and(|name| c.payload_word(name, 0).is_none_or(|t| c.lexeme(t) == "=="))
}

/// `isHashCode` (`ast.dart`): a `Method` named `hashCode` or a `Fields`
/// declaring a `hashCode` variable.
fn is_hash_code(c: &Check<'_>, member: u32) -> bool {
    match c.kind(member) {
        NodeKind::Method => member_name_of(c, member).is_some_and(|name| {
            c.kind(name) == NodeKind::Identifier && c.lexeme(c.head_token(name)) == "hashCode"
        }),
        NodeKind::Fields => c.children(member).any(|descendant| {
            c.kind(descendant) == NodeKind::InitializedIdentifier
                && c.lexeme(c.head_token(descendant)) == "hashCode"
        }),
        _ => false,
    }
}

/// The report position: a `Method` at its name (an operator through its
/// `OperatorName` row, a named member through its name row), a `Fields`
/// at its `hashCode` variable (the official `getFieldName` fallback).
fn member_or_field_name(c: &Check<'_>, member: u32) -> Option<u32> {
    match c.kind(member) {
        NodeKind::Method => member_name_of(c, member),
        NodeKind::Fields => c.children(member).find(|&descendant| {
            c.kind(descendant) == NodeKind::InitializedIdentifier
                && c.lexeme(c.head_token(descendant)) == "hashCode"
        }),
        _ => None,
    }
}

/// The report position token: an `OperatorName` names through its
/// payload token (the `==` itself — `head_token` would hit the
/// `operator` keyword), an `Identifier`/variable through its span head.
fn report_at(c: &mut Check<'_>, name: u32, args: &[&str]) {
    let token = if c.kind(name) == NodeKind::OperatorName {
        let Some(token) = c.payload_word(name, 0) else {
            return;
        };
        token
    } else {
        c.head_token(name)
    };
    let Some(start) = c.utf16_of_token(token) else {
        return;
    };
    c.report(start..start, args);
}

/// The name child of a `Method`: an operator names through its
/// `OperatorName` row ([`operator_name`]), everything else through the
/// context-stamped declaration name ([`member_declaration_name`]).
fn member_name_of(c: &Check<'_>, member: u32) -> Option<u32> {
    let name = operator_name(c, member);
    name.or_else(|| member_declaration_name(c, member))
}
