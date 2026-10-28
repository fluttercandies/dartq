use super::{Check, NodeKind, RuleSpec, narrow_u32};

/// Official: `upstream Dart SDK source, pkg/linter/lib/src/rules/always_put_required_named_parameters_first.dart:31-92`
/// — a required named parameter written after a non-required named
/// parameter reports at the parameter's name. Required = the `required`
/// keyword (the `RequiredNamed` kind) or a `@required`
/// annotation (:59 `element.metadata.hasRequired`). The parameter name
/// is the payload word 1 token.
///
/// The super-parameter ordering face (:42, :65-91
/// `_superParametersAreInOrder`) rides the semantic bridge: with two or
/// more named super-formals, each resolves to its target parameter in
/// the super constructor's formal table ([`dartq_semantic::Analysis::resolve_constructor`]
/// over the extends-clause type reference; the written `super.named(...)`
/// initializer selector supplies the constructor) and their positions
/// must not decrease — an out-of-order or unresolvable face arms the
/// flag like any non-required parameter (:55-61), upstream's
/// `superConstructorParameter == null` posture. At most one named
/// super-formal is trivially in order (:71) — the shipped `super.key`
/// widget shape never resolves anything.
pub(crate) static SPEC: RuleSpec = crate::spec!(
    "always_put_required_named_parameters_first",
    &[
        NodeKind::FormalParameters,
        NodeKind::OptionalFormalParameters,
    ],
    check,
);

fn check(c: &mut Check<'_>) {
    let node = c.node();
    // `_superParametersAreInOrder` (:42, :65-91) — a per-list invariant,
    // derived once before the walk.
    let super_in_order = super_parameters_are_in_order(c, node);
    let mut non_required_seen = false;
    for param in c.collect_direct(node) {
        if c.kind(param) != NodeKind::FormalParameter {
            continue;
        }
        let kind = c.payload(param).get(4).copied().unwrap_or(u32::MAX);
        let required_kind = kind == dartq_parser::FormalParameterKind::RequiredNamed as u32;
        let named =
            required_kind || kind == dartq_parser::FormalParameterKind::OptionalNamed as u32;
        if !named {
            continue;
        }
        let required = required_kind || has_required_annotation(c, node, param);
        if required {
            if non_required_seen && let Some(&name) = c.payload(param).get(1) {
                let Some(start) = c.utf16_of_token(name) else {
                    continue;
                };
                let len = narrow_u32(c.lexeme(name).encode_utf16().count());
                c.report(start..start + len, &[]);
            }
        } else if !(super_in_order && is_super_formal(c, param)) {
            // An in-order super-parameter doesn't arm the flag (:55-58).
            non_required_seen = true;
        }
    }
}

/// `_superParametersAreInOrder` (:65-91): with at most one named
/// super-formal, trivially in order (:71); otherwise every named
/// super-formal's target position in the super constructor's formal
/// list must not decrease.
fn super_parameters_are_in_order(c: &Check<'_>, list: u32) -> bool {
    let names: Vec<&str> = c
        .collect_direct(list)
        .into_iter()
        .filter(|&param| {
            c.kind(param) == NodeKind::FormalParameter && is_super_formal(c, param)
        })
        .filter(|&param| {
            matches!(
                c.payload(param).get(4).copied(),
                Some(k)
                    if k == dartq_parser::FormalParameterKind::RequiredNamed as u32
                        || k == dartq_parser::FormalParameterKind::OptionalNamed as u32
            )
        })
        .filter_map(|param| c.payload(param).get(1).copied())
        .filter(|&token| token != crate::rules::NONE_TOKEN)
        .map(|token| c.lexeme(token))
        .collect();
    if names.len() <= 1 {
        return true;
    }
    let Some(positions) = super_formal_positions(c, &names) else {
        return false; // unresolvable target — the null-parameter face
    };
    let mut previous: Option<usize> = None;
    for index in positions {
        let Some(index) = index else {
            // `superConstructorParameter == null` (:78) — never in order.
            return false;
        };
        if previous.is_some_and(|prev| index < prev) {
            return false;
        }
        previous = Some(index);
    }
    true
}

/// Each named super-formal's target position in the super constructor's
/// full formal list (positional group first, then named — the model's
/// declaration order), `None` when the super constructor declares no
/// parameter of that name. The constructor is the written `super(...)`
/// initializer's selector, else the unnamed face; its container is the
/// extends-clause supertype, resolved through the bridge across files.
/// `None` when the bridge is unmounted or any link stays unresolved.
fn super_formal_positions(c: &Check<'_>, names: &[&str]) -> Option<Vec<Option<usize>>> {
    let analysis = c.semantics()?;
    // The owner constructor (a primary constructor carries no
    // initializer list — the unnamed face) and its class-like header.
    let owner = c.ancestor(1)?;
    let class_row = c
        .path
        .iter()
        .rev()
        .find(|&&row| c.kind(row) == NodeKind::ClassDeclaration)
        .copied()?;
    let selector = (c.kind(owner) == NodeKind::Constructor)
        .then(|| super::super_initializer_selector(c, owner))
        .flatten();
    // The extends head's type-reference position: the published row sits
    // at the name token — the continuation token for a prefixed `p.S`
    // head (the prefix itself publishes no type-use row).
    let head = super::extends_head_token(c, class_row)?;
    let query_token = if c.lexeme(head + 1) == "." {
        head + 2
    } else {
        head
    };
    let token = c.unit.tokens.get(query_token as usize)?;
    let site = dartq_source::SourceRange {
        offset: token.byte_offset,
        length: token.byte_len,
    };
    let selector_id = selector.map(|name| analysis.strings().intern(&name));
    let binding = analysis.resolve_constructor(site, selector_id)?;
    let table = binding.formals?;
    Some(
        names
            .iter()
            .map(|name| {
                table
                    .named
                    .iter()
                    .position(|formal| analysis.strings().resolve(formal.name) == *name)
                    .map(|at| table.positional.len() + at)
            })
            .collect(),
    )
}

/// The `super` receiver word (the `FormalParameter` payload's first word).
fn is_super_formal(c: &Check<'_>, param: u32) -> bool {
    let receiver = c
        .payload(param)
        .first()
        .copied()
        .unwrap_or(crate::rules::NONE_TOKEN);
    receiver != crate::rules::NONE_TOKEN && c.lexeme(receiver) == "super"
}

/// The parameter's left sibling among the list's children is a
/// `MetadataStar` naming `required` (the probe shape: parameter
/// annotations ride a preceding sibling row).
fn has_required_annotation(c: &Check<'_>, list: u32, param: u32) -> bool {
    let siblings = c.collect_direct(list);
    let Some(index) = siblings.iter().position(|&kid| kid == param) else {
        return false;
    };
    let Some(&star) = index.checked_sub(1).and_then(|i| siblings.get(i)) else {
        return false;
    };
    if c.kind(star) != NodeKind::MetadataStar {
        return false;
    }
    c.collect_direct(star).into_iter().any(|meta| {
        c.kind(meta) == NodeKind::Metadata
            && c.collect_direct(meta).into_iter().any(|piece| {
                c.kind(piece) == NodeKind::Identifier
                    && c.lexeme(c.head_token(piece)) == "required"
            })
    })
}
