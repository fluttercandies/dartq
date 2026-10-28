use dartq_semantic::model::{ArgListId, Ty};

use super::{Check, NodeKind, RuleSpec, query_site, same_file, slice, static_type_faces, switch_scrutinee_read};
use crate::rules::utf16_token_span;

/// The pattern-face problem text (`unrelatedTypeEqualityChecksInPattern`,
/// `diagnostic.g.dart:3727-3734`): the operand against the matched
/// value.
const PATTERN_PROBLEM: &str = "The type of the operand ('{0}') isn't a subtype or a supertype of the value being matched ('{1}').";

/// Official: `pkg/linter/lib/src/rules/unrelated_type_equality_checks.dart`
/// — an `==`/`!=` comparison (a bool-typed one, :47-49) whose operand
/// types are unrelated reports at the operator, with `(right type,
/// left type)` (:62-66; the pattern face :69-83 needs the matched
/// value type — unported).
///
/// The operand types: scalar literals (`int`, `double`, `String`,
/// `bool`), `this` (the enclosing class's name), a bare name whose
/// same-file written type (parameter, local, field, top-level) carries
/// a name — and the bridge static type when the written face has no
/// answer: a bare read row (an import-bound top-level) or an
/// invocation's callee return type (`f() == true`). The comparability
/// relation (`typesAreUnrelated`): same name related; `Object`/
/// `dynamic` relate to everything; the `num` family relates
/// `int`/`double`; the four scalar faces are final (nothing else
/// relates to them); any other pair with a same-file class-like name
/// stays silent — the supertype chain, or a cross-file link behind
/// it, can relate the names in ways this face cannot see. The bool
/// gate (:47-49) reads the left operand's resolved `==` override: a
/// non-`bool` declared return silences (a bare-name operand only; an
/// unresolvable or undecided override keeps the bool assumption).
/// `null` operands skip (:53-55). The pattern face (:69-83,
/// `visitRelationalPattern`): a relational pattern whose operand type
/// and matched value type are unrelated reports at the pattern — the
/// matched value type rides [`Analysis::matched_value_type`] keyed at
/// the switch scrutinee read's own span (an if-case carries no switch
/// ancestor and stays silent). Message: `diagnostic.g.dart`
/// `unrelatedTypeEqualityChecksInExpression`.
pub(crate) static SPEC: RuleSpec = crate::spec!(
    "unrelated_type_equality_checks",
    &[NodeKind::BinaryExpression, NodeKind::RelationalPattern],
    check,
);

fn check(c: &mut Check<'_>) {
    if c.kind(c.node()) == NodeKind::RelationalPattern {
        pattern_face(c);
        return;
    }
    let node = c.node();
    let Some(op) = c.payload(node).first().copied() else {
        return;
    };
    if !matches!(c.lexeme(op), "==" | "!=") {
        return;
    }
    // The operand runs of the flattened container (:52-59).
    let Some((left, right)) = slice::binary_operands(c) else {
        return;
    };
    // The official bool gate (:47-49): the comparison's static type is
    // `bool` unless the left operand's class overrides `==` with a
    // non-`bool` return.
    if equality_override_silences(c, &left) {
        return;
    }
    let left_type = operand_type(c, &left).map(str::to_owned).or_else(|| {
        c.semantics()
            .and_then(|analysis| operand_static_name(c, analysis, &left))
    });
    let right_type = operand_type(c, &right).map(str::to_owned).or_else(|| {
        c.semantics()
            .and_then(|analysis| operand_static_name(c, analysis, &right))
    });
    let (Some(left_type), Some(right_type)) = (left_type, right_type) else {
        return;
    };
    if !unrelated(c, &left_type, &right_type) {
        return; // `_comparable` on the ported face
    }
    let range = utf16_token_span(c, op);
    c.report(range, &[right_type.as_str(), left_type.as_str()]);
}

/// The relational-pattern face (:69-83): the operand's static type
/// against the matched value type, reported at the pattern
/// (`reportAtNode`). The operand rows precede the childless pattern
/// row (the operator token first, then the operand parsing — the
/// pattern row opens at the operand's first token and seals past it,
/// so the report runs from the operator through the row end).
fn pattern_face(c: &mut Check<'_>) {
    let node = c.node();
    let Some(op) = c.payload(node).first().copied() else {
        return;
    };
    if !matches!(c.lexeme(op), "==" | "!=") {
        return;
    }
    let Some(parent) = c.ancestor(1) else {
        return;
    };
    let kids = c.collect_direct(parent);
    let Some(pos) = kids.iter().position(|&kid| kid == node) else {
        return;
    };
    // The operand's type: a scalar literal or a bare read row — the
    // binary face's bridge shapes. A `null` operand or an undecidable
    // run stays silent.
    let run: Vec<u32> = kids[..pos].to_vec();
    let Some(operand_ty) = operand_static_ty(c, &run) else {
        return;
    };
    // The matched value type, keyed at the switch scrutinee read's own
    // span; a non-lone-identifier scrutinee carries no row.
    let Some(analysis) = c.semantics() else {
        return;
    };
    let Some(scrutinee) = switch_scrutinee_read(c) else {
        return;
    };
    let Some(matched_ty) = analysis.matched_value_type(query_site(c, scrutinee)) else {
        return;
    };
    let (Some(operand), Some(matched)) = (
        ty_display(analysis, operand_ty),
        ty_display(analysis, matched_ty),
    ) else {
        return;
    };
    if !unrelated(c, &operand, &matched) {
        return; // `_comparable(valueType, operandType)` (:82)
    }
    let range = utf16_token_span(c, op).start..c.utf16_span(node).end;
    c.report_texts(
        range,
        PATTERN_PROBLEM,
        "Try changing one or both of the operands.",
        &[operand.as_str(), matched.as_str()],
    );
}

/// The operand run's static type: a scalar literal, or the bridge read
/// face over a lone identifier (the `operandType` shapes this face's
/// display can render exactly).
fn operand_static_ty(c: &Check<'_>, run: &[u32]) -> Option<Ty> {
    let named = |name: &'static str| -> Option<Ty> {
        let analysis = c.semantics()?;
        Some(Ty::Named {
            name: analysis.strings().intern(name),
            nullable: false,
            args: ArgListId::EMPTY,
        })
    };
    let root = static_type_faces::run_root(c, run)?;
    match c.kind(root) {
        NodeKind::LiteralInt => named("int"),
        NodeKind::LiteralDouble => named("double"),
        NodeKind::StringLiteral => named("String"),
        NodeKind::LiteralBool => named("bool"),
        _ => {
            let identifier_run = slice::access_run(c, run);
            let [only] = identifier_run.as_slice() else {
                return None;
            };
            if c.kind(*only) != NodeKind::Identifier {
                return None;
            }
            c.semantics()?.type_of(query_site(c, *only))
        }
    }
}

/// The exact display of a `Ty`: the resolved name with a `?` tail; a
/// type with explicit arguments renders only when the list is empty —
/// the display argument must be exact.
fn ty_display(analysis: &dartq_semantic::Analysis<'_>, ty: Ty) -> Option<String> {
    let Ty::Named {
        name,
        nullable,
        args,
    } = ty
    else {
        return None;
    };
    if args != ArgListId::EMPTY {
        return None;
    }
    let mut display = analysis.strings().resolve(name).to_owned();
    if nullable {
        display.push('?');
    }
    Some(display)
}

/// Whether the left operand's resolved `==` override declares a
/// non-`bool` return (the official gate's silhouette face): a
/// bare-name operand only — the override lookup needs the receiver's
/// own read row.
fn equality_override_silences(c: &Check<'_>, left: &[u32]) -> bool {
    let Some(analysis) = c.semantics() else {
        return false;
    };
    let only = match slice::access_run(c, left).as_slice() {
        [only] if c.kind(*only) == NodeKind::Identifier => *only,
        _ => return false,
    };
    let site = query_site(c, only);
    if !matches!(analysis.type_of(site), Some(Ty::Named { .. })) {
        return false;
    }
    match analysis.member_of(site, analysis.strings().intern("==")) {
        Some(binding) => !matches!(
            binding.entry.return_ty,
            Ty::Named {
                name,
                nullable: false,
                ..
            } if analysis.strings().resolve(name) == "bool"
        ),
        None => false,
    }
}

/// The operand's static type name over the bridge when the written
/// face has no answer: a bare-name read row (`x == true` over an
/// import-bound top-level) or an invocation's callee return type
/// (`f() == true`). The display name only (an argument-carrying type
/// under-reports — the display argument must be exact).
fn operand_static_name(
    c: &Check<'_>,
    analysis: &dartq_semantic::Analysis<'_>,
    rows: &[u32],
) -> Option<String> {
    let run = slice::access_run(c, rows);
    let ty = match run.as_slice() {
        [only] if c.kind(*only) == NodeKind::Identifier => {
            analysis.type_of(query_site(c, *only))?
        }
        [callee, arguments]
            if c.kind(*callee) == NodeKind::Identifier
                && c.kind(*arguments) == NodeKind::Arguments =>
        {
            analysis.callee_type(query_site(c, *callee))?
        }
        _ => return None,
    };
    let Ty::Named {
        name,
        nullable,
        args,
    } = ty
    else {
        return None;
    };
    if args != ArgListId::EMPTY {
        return None;
    }
    let mut display = analysis.strings().resolve(name).to_owned();
    if nullable {
        display.push('?');
    }
    Some(display)
}

/// The operand's type face: a scalar literal, `this` (the enclosing
/// class-like name), or a bare name's written type (the same-file
/// binding slice). `null` answers `None` — the official skips null
/// operands outright (:53-55).
fn operand_type<'a>(c: &'a Check<'_>, rows: &[u32]) -> Option<&'a str> {
    // The bare-name read tiles in the four-piece send-group form —
    // strip the wrapper pieces before the single-row match.
    let run = slice::access_run(c, rows);
    let [root] = run.as_slice() else {
        return None; // a multi-row operand is a chain — no face
    };
    match c.kind(*root) {
        NodeKind::LiteralInt => Some("int"),
        NodeKind::LiteralDouble => Some("double"),
        NodeKind::StringLiteral => Some("String"),
        NodeKind::LiteralBool => Some("bool"),
        NodeKind::ThisExpression => {
            let class_row = c.path.iter().rev().find(|&&a| {
                matches!(
                    c.kind(a),
                    NodeKind::ClassDeclaration
                        | NodeKind::MixinDeclaration
                        | NodeKind::EnumDeclaration
                        | NodeKind::ExtensionDeclaration
                )
            })?;
            let name = c
                .collect_direct(*class_row)
                .into_iter()
                .find(|&kid| {
                    c.kind(kid) == NodeKind::Identifier
                        && c.payload(kid).first()
                            == Some(&(dartq_parser::IdentifierContext::ClassOrMixinOrExtensionDeclaration as u32))
                })?;
            Some(c.lexeme(c.head_token(name)))
        }
        NodeKind::Identifier => {
            let name = c.lexeme(c.head_token(*root));
            crate::rules::written_type_face(c, c.node(), name)?.name
        }
        // every other kind has no scalar face
        _ => None,
    }
}

/// The comparability gate over the two face names (`typesAreRelated`):
/// same name related; `Object`/`dynamic` relate to everything; the
/// `num` family relates `int`/`double`; the four scalar faces are
/// final (nothing else relates to them); any other pair with a
/// same-file class-like name stays silent — the same-file supertype
/// chain, or a cross-file link behind it, can relate the names in
/// ways the syntax face cannot see.
fn unrelated(c: &Check<'_>, left: &str, right: &str) -> bool {
    if left == right {
        return false;
    }
    if matches!(left, "Object" | "dynamic") || matches!(right, "Object" | "dynamic") {
        return false;
    }
    let num_related = |a: &str, b: &str| {
        (a == "num" && matches!(b, "int" | "double"))
            || (b == "num" && matches!(a, "int" | "double"))
    };
    if num_related(left, right) {
        return false;
    }
    if matches!(left, "int" | "double" | "String" | "bool")
        || matches!(right, "int" | "double" | "String" | "bool")
    {
        // A final scalar relates to nothing but its own name and
        // `num` — the operand faces are provably unrelated.
        return true;
    }
    let left_same = crate::rules::same_file_declaration(c, left).is_some();
    let right_same = crate::rules::same_file_declaration(c, right).is_some();
    if left_same && right_same {
        // The written supertype closures decide (the official walk): a
        // shared link relates the pair, complete closures without one
        // are unrelated, an incomplete closure could relate it behind
        // the unindexed link.
        let (Some(left_chain), Some(right_chain)) = (
            same_file::supertype_closure(c, left),
            same_file::supertype_closure(c, right),
        ) else {
            return false;
        };
        let related = left_chain.iter().any(|name| name == right)
            || right_chain.iter().any(|name| name == left)
            || left_chain.iter().any(|name| right_chain.contains(name));
        return !related;
    }
    if left_same || right_same {
        return false;
    }
    true
}
