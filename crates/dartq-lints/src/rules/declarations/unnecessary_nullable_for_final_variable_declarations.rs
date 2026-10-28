use super::{Check, NodeKind, RuleSpec};
use crate::rules::expressions::{query_site, static_type_faces as faces};
use crate::rules::first_child;
use dartq_parser::MemberKind;
use dartq_semantic::model::Ty;

/// Official: `pkg/linter/lib/src/rules/unnecessary_nullable_for_final_variable_declarations.dart:35-117`
/// — a `final`/`const` declaration with a nullable written type whose
/// initializer's static type is provably non-nullable reports at the
/// variable name (`isNullable(declared) && isNonNullable(initializer)`,
/// :106-107).
///
/// dartq faces: the initializer static type answers over the semantic
/// bridge — a bare name's flow read (`type_of`), an invocation's callee
/// return type (`callee_type`), a single-name member access
/// (`member_type`), and an instance creation's container
/// (`creation_type`) — and reports only on a provably non-nullable
/// answer; `null`, unknown and unresolved initializers stay silent
/// (the conservative direction). Literals keep the syntactic face (the
/// bridge's read rows ride identifier reads only). Binary and
/// conditional initializers, and multi-hop chains, carry no bridge face
/// and under-report (the `no_duplicate_case_values` constant-evaluation
/// precedent). Fields keep the official private-or-static gate
/// (:83-88). Message: `diagnostic.g.dart`
/// `unnecessaryNullableForFinalVariableDeclarations`.
pub(crate) static SPEC: RuleSpec = crate::spec!(
    "unnecessary_nullable_for_final_variable_declarations",
    &[NodeKind::VariablesDeclaration],
    check,
);

fn check(c: &mut Check<'_>) {
    let node = c.node();
    // `isFinal || isConst` (:97): the storage keyword — production rows
    // anchor a local declaration at the name, a field at the keyword;
    // `variables_declaration_keyword` recovers `final`/`const` across
    // both (`late final` resolves to `final`).
    let Some(keyword) = crate::rules::variables_declaration_keyword(c, node) else {
        return;
    };
    if !matches!(c.lexeme(keyword), "final" | "const") {
        return;
    }
    // `declaredElement.type is DynamicType` bails (:101-103) — the
    // written type answers from three shapes: a `Type`-kind child
    // (hand-built tables), the token run after the storage keyword
    // ([`local_written_type`] — production rows anchored at the name),
    // and the keyword-anchored field tiling (the type runs from the row
    // head, after `late`/`final`/`const`, up to the first variable's
    // name). The written `?` is the span's tail token (#174).
    let kids = c.collect_direct(node);
    let first_var_token = kids
        .iter()
        .copied()
        .find(|&kid| c.kind(kid) == NodeKind::InitializedIdentifier)
        .map(|kid| c.head_token(kid));
    let written_type = match kids
        .iter()
        .copied()
        .find(|&kid| c.kind(kid) == NodeKind::Type)
    {
        Some(type_node) => {
            let range = c.unit.ast.token_range(type_node);
            Some((range.start, range.end))
        }
        None => super::local_written_type(c, node).or_else(|| {
            let name = first_var_token?;
            let mut start = c.head_token(node);
            for _ in 0..2 {
                match c.lexeme(start) {
                    "late" | "var" | "final" | "const" => start += 1,
                    _ => break,
                }
            }
            (start < name).then_some((start, name))
        }),
    };
    let nullable_written = match written_type {
        Some((start, end)) => start < end && c.lexeme(end - 1) == "?",
        None => false,
    };
    if !nullable_written {
        return;
    }
    // Fields: private or static only (:83-88).
    let is_field = c
        .ancestor(1)
        .is_some_and(|parent| c.kind(parent) == NodeKind::Fields);
    if is_field {
        let parent = c.ancestor(1).expect("checked above");
        let is_static = c.payload(parent).first() == Some(&(MemberKind::StaticField as u32));
        let is_private = c
            .collect_direct(node)
            .iter()
            .any(|&var| c.lexeme(c.head_token(var)).starts_with('_'));
        if !is_static && !is_private {
            return;
        }
    }
    for var in c.collect_direct(node) {
        if c.kind(var) != NodeKind::InitializedIdentifier {
            continue;
        }
        // `variable.initializer?.staticType` (:98) — an uninitialized
        // variable bails per-variable. The initializer's piece run is
        // the VariableInitializer's direct-child tiling; the literal
        // face keeps its head-row answer (a prefixed literal tiles its
        // operator leaf after the head literal).
        let Some(init) = c
            .collect_direct(var)
            .into_iter()
            .find(|&kid| c.kind(kid) == NodeKind::VariableInitializer)
        else {
            continue;
        };
        let head = first_child(c, init);
        let run = c.collect_direct(init);
        if !initializer_non_nullable(c, head, &run) {
            continue;
        }
        // `rule.reportAtToken(variable.name)` (:110).
        let Some(name_token) = crate::rules::initialized_identifier_name(c, var) else {
            continue;
        };
        let Some(start) = c.utf16_of_token(name_token) else {
            continue;
        };
        c.report(start..start, &[]);
    }
}

/// `initializerType != null && isNonNullable(initializerType)`
/// (:98-107): the initializer's static type reports only on a provably
/// non-nullable answer. The literal face answers syntactically on the
/// head row; every other shape answers over the semantic bridge —
/// `null`, unknown and unresolved faces answer `false`, the
/// under-report direction.
fn initializer_non_nullable(c: &Check<'_>, head: Option<u32>, run: &[u32]) -> bool {
    if head.is_some_and(|head| is_non_null_literal(c.kind(head))) {
        return true;
    }
    let Some(analysis) = c.semantics() else {
        return false;
    };
    let Some(root) = faces::run_root(c, run) else {
        return false;
    };
    if c.kind(root) == NodeKind::LiteralNull {
        return false;
    }
    let ty = match c.kind(root) {
        // A bare name answers at its flow read row.
        NodeKind::Identifier => analysis.type_of(query_site(c, root)),
        // An instance creation answers through the container
        // resolution (the constructed type is non-nullable by shape).
        NodeKind::ImplicitCreation | NodeKind::NewExpression => {
            super::constructed_type(c, analysis, root)
        }
        // A trailing `DotAccess` marker roots a bare member access
        // (`c.v`) — the shared `expr_type` face reads its selector off
        // the marker's span tail, but the production marker's window
        // swallows the `;`, so the name answers off the payload's dot
        // token instead (`payload[0] + 1` is the member name). The
        // receiver run ends at the dot — the selector's own tear-off
        // group tiles before the marker and must not join it.
        NodeKind::DotAccess => (|| {
            let dot = c.payload(root).first().copied()?;
            let selector = dot + 1;
            let receiver_run: Vec<u32> = run[..run.len() - 1]
                .iter()
                .copied()
                .filter(|&piece| c.unit.ast.token_range(piece).end <= dot)
                .collect();
            let receiver = faces::single_receiver(c, &receiver_run, selector)?;
            let name = analysis.strings().intern(c.lexeme(selector));
            analysis.member_type(query_site(c, receiver), name)
        })(),
        // The remaining bridge faces: a bare send (the callee return
        // type) and a single-name member access — chains and unmodeled
        // shapes answer `None`.
        _ => faces::expr_type(c, analysis, run).or_else(|| {
            // A bare `D()` send is an implicit class creation — the
            // callee face answers `None` on a type target (no
            // `type_uses` row on an expression-context selector); the
            // binding resolves the class name.
            if c.kind(root) != NodeKind::Send {
                return None;
            }
            let selector = run
                .iter()
                .copied()
                .find(|&piece| c.kind(piece) == NodeKind::Identifier)?;
            analysis
                .binding_of(query_site(c, selector))
                .filter(|binding| {
                    binding.kind == dartq_semantic::BindingKind::Decl
                        && crate::rules::is_upper_camel_case(
                            analysis.strings().resolve(binding.target_name),
                        )
                })
                .map(|binding| Ty::named(binding.target_name))
        }),
    };
    ty.is_some_and(Ty::is_strictly_non_nullable)
}

/// The non-null literal face: any literal form but `null`.
fn is_non_null_literal(kind: NodeKind) -> bool {
    matches!(
        kind,
        NodeKind::LiteralInt
            | NodeKind::LiteralDouble
            | NodeKind::LiteralBool
            | NodeKind::StringLiteral
            | NodeKind::ListLiteral
            | NodeKind::SetOrMapLiteral
            | NodeKind::RecordLiteral
            | NodeKind::ConstExpression
            | NodeKind::ConstLiteral
    )
}
