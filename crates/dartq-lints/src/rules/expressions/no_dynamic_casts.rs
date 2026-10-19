use dartq_semantic::model::Ty;

use super::{
    Check, Group, NodeKind, RuleSpec, query_site, same_file, slice, static_type_faces as faces,
};
use crate::rules::{TypeFace, type_before, type_face};

/// Official: `upstream Dart SDK source,
/// pkg/linter/lib/src/rules/no_dynamic_casts.dart` — a `dynamic`-typed
/// expression flowing into a typed context reports at the expression
/// (`_check` :251-264): the target type must decide and be neither
/// `dynamic` nor `Object?`, and an explicit `as` cast on the expression
/// bails. The registered contexts (:39-60) are assignments
/// (:77-79), arguments (:67-74), returns and arrow bodies (:100-104,
/// :189-195), and variable declarations (:216-223), plus the boolean
/// faces (conditions, guards, logical operands, `!`).
///
/// dartq faces over the production tiling: the decidable context slice
/// ports — variable declarations (the written type row), assignments
/// (the LHS's innermost same-file written type, the `same_file`
/// channel), returns (the nearest enclosing declaration's written
/// return type, async-flattened through the first type argument), and
/// arguments (the member or creation formal table). The source type is
/// the piece-run static-type face; the explicit-cast bail reads.
/// Approximations (documented, under-report): bare-call and
/// local-callable arguments (no formal-table face at M1), the boolean
/// contexts, collection elements, for-each iterables, and yield
/// targets stay silent; member-write assignment targets stay silent
/// (no same-file written type). Message: `diagnostic.g.dart`
/// `noDynamicCasts`.
pub(crate) static SPEC: RuleSpec = RuleSpec {
    name: "no_dynamic_casts",
    group: Group::Standalone,
    problem: "Implicit cast from 'dynamic'.",
    correction: "Try adding an explicit cast or changing the target type.",
    kinds: &[
        NodeKind::VariablesDeclaration,
        NodeKind::Assignment,
        NodeKind::ReturnStatement,
        NodeKind::Arguments,
    ],
    check,
};

fn check(c: &mut Check<'_>) {
    // The wave posture: the rule reads the semantic face; face absent
    // is the plain-run posture and stays silent (the engine contract).
    let Some(analysis) = c.semantics() else {
        return;
    };
    match c.kind(c.node()) {
        NodeKind::VariablesDeclaration => declaration_face(c, analysis),
        NodeKind::Assignment => assignment_face(c, analysis),
        NodeKind::ReturnStatement => return_face(c, analysis),
        NodeKind::Arguments => arguments_face(c, analysis),
        _ => {}
    }
}

/// Whether a written target type admits the report: it must decide and
/// be neither `dynamic`/`void` nor `Object?` (:257-258).
fn target_admits(name: &str, nullable: bool) -> bool {
    name != "dynamic" && name != "void" && !(name == "Object" && nullable)
}

/// The dynamic-source verdict for a piece run: the static type is
/// `dynamic` and the expression is not an explicit cast (:255-262).
fn source_is_dynamic(
    c: &Check<'_>,
    analysis: &dartq_semantic::Analysis<'_>,
    run: &[u32],
) -> bool {
    !faces::is_explicit_cast(c, run) && faces::expr_type(c, analysis, run) == Some(Ty::Dynamic)
}

/// The report span of a piece run: its first token through its last.
fn report_run(c: &mut Check<'_>, run: &[u32]) {
    let Some((first, last)) = run.first().zip(run.last()) else {
        return;
    };
    let range = c.utf16_span(*first).start..c.utf16_span(*last).end;
    c.report(range, &[]);
}

/// The variable-declaration face (:216-223): a written type that
/// admits the report, over a `dynamic` initializer, reports at the
/// initializer (an untyped `var` binding carries no target — silent,
/// exactly like the official `type?` read).
fn declaration_face(c: &mut Check<'_>, analysis: &dartq_semantic::Analysis<'_>) {
    let node = c.node();
    let kids = c.collect_direct(node);
    // A typed declaration's written type tiles beside the declaration
    // row under the statement wrapper (the header-sibling pattern); an
    // untyped `var` binding has none and stays silent.
    let Some(type_row) = c.unit.ast.parent(node).and_then(|top| {
        c.collect_direct(top)
            .into_iter()
            .find(|&kid| c.kind(kid) == NodeKind::Type)
    }) else {
        return;
    };
    let face = type_face(c, type_row);
    let Some(name) = face.name else {
        return;
    };
    if !target_admits(name, face.nullable) {
        return;
    }
    for var in kids
        .iter()
        .copied()
        .filter(|&kid| c.kind(kid) == NodeKind::InitializedIdentifier)
        .collect::<Vec<u32>>()
    {
        // The initializer pieces tile inside the `VariableInitializer`
        // container (the `= d` row wraps its operand group) — the same
        // direct-child tiling every initializer reader reads.
        let Some(init) = c
            .collect_direct(var)
            .into_iter()
            .find(|&kid| c.kind(kid) == NodeKind::VariableInitializer)
        else {
            continue;
        };
        let run = c.collect_direct(init);
        if run.is_empty() {
            continue;
        }
        if source_is_dynamic(c, analysis, &run) {
            report_run(c, &run);
        }
    }
}

/// The assignment face (:77-79): the target's innermost same-file
/// written type decides (the official `writeType` — a member-write
/// target has no same-file face and stays silent).
fn assignment_face(c: &mut Check<'_>, analysis: &dartq_semantic::Analysis<'_>) {
    let node = c.node();
    let Some((left, right)) = slice::binary_operands(c) else {
        return;
    };
    let run = slice::access_run(c, &left);
    let [lhs] = run[..] else {
        return;
    };
    if c.kind(lhs) != NodeKind::Identifier {
        return;
    }
    let Some((_, Some(type_row))) = same_file::binding(c, node, c.lexeme(c.head_token(lhs)))
    else {
        return;
    };
    let face = type_face(c, type_row);
    let Some(name) = face.name else {
        return;
    };
    if !target_admits(name, face.nullable) {
        return;
    }
    if source_is_dynamic(c, analysis, &right) {
        report_run(c, &right);
    }
}

/// The return face (:100-104, :189-195): the nearest enclosing
/// declaration's written return type decides, async-flattened
/// (:349-380 — a `Future`/`FutureOr` wrapper reports through its first
/// type argument, any other written type through [`target_admits`]).
fn return_face(c: &mut Check<'_>, analysis: &dartq_semantic::Analysis<'_>) {
    let node = c.node();
    let run = c.collect_direct(node);
    if run.is_empty() || !source_is_dynamic(c, analysis, &run) {
        return;
    }
    for &row in c.path.iter().rev().skip(1) {
        match c.kind(row) {
            NodeKind::Method
            | NodeKind::TopLevelMethod
            | NodeKind::LocalFunctionDeclaration => {
                // A local function's name nests under `FunctionName` —
                // `member_declaration_name` covers all three kinds.
                let written = crate::rules::declarations::member_declaration_name(c, row)
                    .and_then(|name| type_before(c, row, name))
                    .map(|(type_row, _)| type_face(c, type_row));
                if admits_return(c, row, written) {
                    report_run(c, &run);
                }
                return;
            }
            // An inferred return type carries no face; a constructor's
            // return type is the enclosing class (never nullable).
            NodeKind::FunctionExpression
            | NodeKind::NamedFunctionExpression
            | NodeKind::Constructor
            | NodeKind::Factory
            | NodeKind::CompilationUnit => return,
            _ => {}
        }
    }
}

/// Whether the written return type admits the report (`_check`
/// :251-264): the target must decide and be neither `dynamic`/`void`
/// nor `Object?` — nullability never exempts (`int?` admits like
/// `int`).
fn admits_return(c: &Check<'_>, declaration: u32, written: Option<TypeFace<'_>>) -> bool {
    let Some(face) = written else {
        return false;
    };
    let Some(name) = face.name else {
        return false;
    };
    let async_marked = c
        .unit
        .ast
        .token_range(declaration)
        .any(|token| c.lexeme(token) == "async");
    if async_marked && matches!(name, "Future" | "FutureOr") {
        // flatten(): the wrapper's first type argument decides.
        return match face.type_arguments.first() {
            Some((arg, arg_nullable)) => target_admits(arg, *arg_nullable),
            None => false,
        };
    }
    target_admits(name, face.nullable)
}

/// The argument face (:67-74): the resolved callee's formal type
/// decides — a member invocation's member table or an implicit
/// creation's constructor table. A bare top-level call carries no
/// formal face at M1 and stays silent.
fn arguments_face(c: &mut Check<'_>, analysis: &dartq_semantic::Analysis<'_>) {
    let Some((kids, pos)) = slice::owner_slice(c) else {
        return;
    };
    let Some(&send) = kids.get(pos + 1) else {
        return;
    };
    if c.kind(send) != NodeKind::Send {
        return;
    }
    let Some((head, _)) = slice::send_parts(c, &kids, pos + 1) else {
        return;
    };
    let selector_row = kids[head];
    let formals = if slice::access_marker(c, &kids, pos + 1, NodeKind::DotAccess).is_some() {
        let selector = c.head_token(selector_row);
        let Some(receiver) = faces::single_receiver(c, &kids[..head], selector) else {
            return;
        };
        let name = analysis.strings().intern(c.lexeme(selector));
        match analysis.member_of(query_site(c, receiver), name) {
            Some(member) => member.entry.formals.as_deref(),
            None => return,
        }
    } else {
        match analysis.resolve_constructor(query_site(c, selector_row), None) {
            Some(ctor) if ctor.exists => ctor.formals,
            _ => return,
        }
    };
    let Some(formals) = formals else {
        return;
    };
    let raw = c.collect_direct(c.node());
    let ranges = slice::arguments(c, &raw);
    let mut positional = 0usize;
    for range in ranges {
        let named = c.kind(raw[range.end - 1]) == NodeKind::NamedArgument;
        let (value, ty) = if named {
            let arg_name = c.lexeme(c.head_token(raw[range.start]));
            (
                &raw[range.start + 1..range.end - 1],
                formals
                    .named
                    .iter()
                    .find(|formal| analysis.strings().resolve(formal.name) == arg_name)
                    .map(|formal| formal.ty),
            )
        } else {
            let ty = formals.positional.get(positional).map(|formal| formal.ty);
            positional += 1;
            (&raw[range.clone()], ty)
        };
        let Some(ty) = ty else { continue };
        if !faces::typed_non_dynamic_target(analysis, ty) {
            continue;
        }
        if source_is_dynamic(c, analysis, value) {
            report_run(c, value);
        }
    }
}
