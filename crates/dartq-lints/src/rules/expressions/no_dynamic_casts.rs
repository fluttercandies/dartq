use dartq_ast::{NodeKind, PayloadField};
use dartq_semantic::model::Ty;

use super::{
    Check, RuleSpec, query_site, same_file, slice, static_type_faces as faces, value_run_end,
};
use crate::rules::{TypeFace, byte_span_of_token, type_before, type_face};

/// Official: `upstream Dart SDK source,
/// pkg/linter/lib/src/rules/no_dynamic_casts.dart` — a `dynamic`-typed
/// expression flowing into a typed context reports at the expression
/// (`_check` :251-264): the target type must decide and be neither
/// `dynamic` nor `Object?`, and an explicit `as` cast on the expression
/// bails. The registered contexts (:39-60) are assignments
/// (:77-79), arguments (:67-74), returns and arrow bodies (:100-104,
/// :189-195), and variable declarations (:216-223), plus the boolean
/// faces (conditions, guards, logical operands, `!` :39-60,
/// :111-121), the collection-element faces (list/set/map elements and
/// spreads :127-186), the for-each faces (iterable and loop-variable
/// casts :198-235), and the yield faces (:237-249).
///
/// dartq faces over the production tiling: the decidable context slice
/// ports — variable declarations (the written type row), assignments
/// (the LHS's innermost same-file written type, the `same_file`
/// channel), returns and arrow bodies (the nearest enclosing
/// declaration's written return type, async-flattened through the first
/// type argument), arguments (the member, creation, or bare-call formal
/// table), the boolean contexts (the parenthesized condition container,
/// the for-parts condition segment, the if-element condition group, a
/// conditional's condition, `&&`/`||` operands, and the `!` operand —
/// target `bool`), collection literals (the written type arguments read
/// off the literal's `TypeArguments` rows; a map entry's key and value
/// split at the entry's `:`), for-each (the iterable face against the
/// implicit `Iterable`/`Stream` target, the loop-variable face against
/// the iterable's written element type), and yield (the element type for
/// `yield`, the whole written return type for `yield*`). The source type
/// is the piece-run static-type face; the explicit-cast bail reads.
/// Approximations (documented, under-report): bare-call and
/// local-callable arguments answer only the bridge signature face, the
/// loop-variable element type reads the iterable's same-file written
/// type argument (`List<dynamic>`), a `when` guard stays silent (no
/// row kind), and member-write assignment targets stay silent (no
/// same-file written type). Message: `diagnostic.g.dart`
/// `noDynamicCasts`.
pub(crate) static SPEC: RuleSpec = crate::spec!(
    "no_dynamic_casts",
    &[
        NodeKind::VariablesDeclaration,
        NodeKind::Assignment,
        NodeKind::ReturnStatement,
        NodeKind::Arguments,
        NodeKind::ParenthesizedCondition,
        NodeKind::ForParts,
        NodeKind::IfControlFlow,
        NodeKind::Conditional,
        NodeKind::BinaryExpression,
        NodeKind::UnaryPrefix,
        NodeKind::ListLiteral,
        NodeKind::SetOrMapLiteral,
        NodeKind::ForIn,
        NodeKind::YieldStatement,
        NodeKind::ExpressionBody,
    ],
    check,
);

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
        NodeKind::ParenthesizedCondition => condition_face(c, analysis),
        NodeKind::ForParts => for_parts_face(c, analysis),
        NodeKind::IfControlFlow => if_element_face(c, analysis),
        NodeKind::Conditional => conditional_face(c, analysis),
        NodeKind::BinaryExpression => logical_binary_face(c, analysis),
        NodeKind::UnaryPrefix => negation_face(c, analysis),
        NodeKind::ListLiteral | NodeKind::SetOrMapLiteral => collection_face(c, analysis),
        NodeKind::ForIn => for_each_face(c, analysis),
        NodeKind::YieldStatement => yield_face(c, analysis),
        NodeKind::ExpressionBody => body_face(c, analysis),
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
fn source_is_dynamic(c: &Check<'_>, analysis: &dartq_semantic::Analysis<'_>, run: &[u32]) -> bool {
    let ty = faces::expr_type(c, analysis, run);
    !faces::is_explicit_cast(c, run) && ty == Some(Ty::Dynamic)
}

/// The report span of a piece run: its first token through the last
/// value row (the statement-level completion wrappers — the send group's
/// `Send` and its argument markers — finalize at the statement's `;`,
/// which the container row swallows; the expression's own rows bound the
/// span, exactly the upstream `reportAtNode(expression)` extent).
/// Whether the row carries the expression's own tokens — the send group's
/// completion wrappers (`Send`, the argument markers, the `.` access row)
/// anchor at their delta-tile operator or swallow the statement's `;`,
/// never at the operand head.
fn is_completion_row(c: &Check<'_>, row: u32) -> bool {
    matches!(
        c.kind(row),
        NodeKind::Send
            | NodeKind::NoArguments
            | NodeKind::NoTypeArguments
            | NodeKind::TypeArguments
            | NodeKind::DotAccess
    )
}

fn report_run(c: &mut Check<'_>, run: &[u32]) {
    let Some(&last) = run.last() else {
        return;
    };
    // The container's own covered span anchors at its delta-tile operator,
    // never the operand head (the `a && b` right run's `Send` covers `&& b`)
    // — the report opens at the first value row's head token, exactly the
    // upstream `reportAtNode(expression)` extent.
    let start = run
        .iter()
        .find(|&&row| !is_completion_row(c, row))
        .map(|&row| c.utf16_span(row).start)
        .or_else(|| slice::run_utf16_start(c, run))
        .unwrap_or_else(|| c.utf16_span(run[0]).start);
    let end = value_run_end(c, run, last);
    c.report(start..end, &[]);
}

/// The variable-declaration face (:216-223): a written type that
/// admits the report, over a `dynamic` initializer, reports at the
/// initializer (an untyped `var` binding carries no target — silent,
/// exactly like the official `type?` read).
fn declaration_face(c: &mut Check<'_>, analysis: &dartq_semantic::Analysis<'_>) {
    let node = c.node();
    let kids = c.collect_direct(node);
    // The written type tiles as the declaration's left sibling (the
    // production parser opens the declaration at the name); an untyped
    // `var` binding has none and stays silent.
    let Some(type_row) =
        super::written_type_sibling(c, node, &[NodeKind::Type, NodeKind::FunctionType])
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
    let Some((_, Some(type_row))) = same_file::binding(c, node, c.lexeme(c.head_token(lhs))) else {
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

/// The nearest enclosing declaration's written return type, as a full
/// [`TypeFace`] — the `_getEnclosingReturnType` walk (:349-380): a
/// function literal's or constructor's return carries no written face
/// and stays silent.
fn enclosing_return_face<'a>(c: &'a Check<'a>) -> Option<(u32, TypeFace<'a>)> {
    for &row in c.path.iter().rev().skip(1) {
        match c.kind(row) {
            NodeKind::Method | NodeKind::TopLevelMethod | NodeKind::LocalFunctionDeclaration => {
                // A local function's name nests under `FunctionName` —
                // `member_declaration_name` covers all three kinds.
                let written = crate::rules::declarations::member_declaration_name(c, row)
                    .and_then(|name| type_before(c, row, name))
                    .filter(|(_, kind)| matches!(kind, NodeKind::Type))
                    .map(|(type_row, _)| type_face(c, type_row))?;
                return Some((row, written));
            }
            // An inferred return type carries no face; a constructor's
            // return type is the enclosing class (never written here).
            NodeKind::FunctionExpression
            | NodeKind::NamedFunctionExpression
            | NodeKind::Constructor
            | NodeKind::Factory
            | NodeKind::CompilationUnit => return None,
            _ => {}
        }
    }
    None
}

/// The return-type target for a value returned or yielded: the written
/// face async-flattened when the declaration is `async` (`flatten()` —
/// a `Future`/`FutureOr` wrapper decides through its first type
/// argument :349-380).
fn flattened_target<'a>(
    c: &Check<'_>,
    declaration: u32,
    written: &TypeFace<'a>,
) -> Option<TypeFace<'a>> {
    let async_marked = c
        .unit
        .ast
        .token_range(declaration)
        .any(|token| c.lexeme(token) == "async");
    let (name, nullable) = if async_marked && matches!(written.name, Some("Future" | "FutureOr")) {
        // flatten(): the wrapper's first type argument decides.
        *written.type_arguments.first()?
    } else {
        (written.name?, written.nullable)
    };
    Some(TypeFace {
        name: Some(name),
        nullable,
        is_void: false,
        is_future_or_void: false,
        is_function_type: false,
        type_arguments: Vec::new(),
    })
}

/// The shared return-or-body verdict: the run judges against the
/// enclosing declaration's written return type (:100-104, :189-195).
fn return_or_body_face(c: &mut Check<'_>, analysis: &dartq_semantic::Analysis<'_>, run: &[u32]) {
    if run.is_empty() || !source_is_dynamic(c, analysis, run) {
        return;
    }
    if let Some((declaration, written)) = enclosing_return_face(c)
        && let Some(target) = flattened_target(c, declaration, &written)
        && target_admits(target.name.unwrap_or("dynamic"), target.nullable)
    {
        report_run(c, run);
    }
}

/// The return face (:100-104).
fn return_face(c: &mut Check<'_>, analysis: &dartq_semantic::Analysis<'_>) {
    let run = c.collect_direct(c.node());
    return_or_body_face(c, analysis, &run);
}

/// The arrow-body face (:189-195): the `ExpressionBody` row is a
/// childless marker sealed after its expression completed, so the body's
/// rows tile as the marker's preceding siblings under the enclosing
/// declaration (the read `avoid_returning_null_for_void` and
/// `async_return_with_no_await` share). The run is the sibling tail
/// opening past the marker's head token — the arrow; the declaration's
/// header rows (return type, name, formals) all open before it.
fn body_face(c: &mut Check<'_>, analysis: &dartq_semantic::Analysis<'_>) {
    let node = c.node();
    let Some(parent) = c.unit.ast.parent(node) else {
        return;
    };
    let kids = c.collect_direct(parent);
    let Some(pos) = kids.iter().position(|&kid| kid == node) else {
        return;
    };
    let arrow = c.head_token(node);
    let start = kids[..pos]
        .iter()
        .rposition(|&kid| c.head_token(kid) <= arrow)
        .map_or(0, |i| i + 1);
    return_or_body_face(c, analysis, &kids[start..pos]);
}

/// The boolean-condition faces (the `boolType` targets of :39-60): the
/// parenthesized condition container owns the condition rows of
/// if/while/do/switch statements and clauses; the run judges against
/// `bool`.
fn condition_face(c: &mut Check<'_>, analysis: &dartq_semantic::Analysis<'_>) {
    let run = condition_run(c, c.node());
    check_bool_target(c, analysis, &run);
}

/// The condition segment's full piece run: `ParenthesizedCondition`
/// adopts only the expression's completion row (`begin_adopting(…, 1)`),
/// so the direct children are the lone `Send`/`BinaryExpression` wrapper
/// while its send-group pieces (`Identifier`, `NoTypeArguments`,
/// `NoArguments`) tile as the node's left siblings under the statement.
/// The run is the contiguous tail of left-sibling send-group pieces plus
/// the node's own children — the tail walk stops at the `(` boundary so
/// an earlier statement's pieces stay out.
fn condition_run(c: &Check<'_>, node: u32) -> Vec<u32> {
    let mut run = c.collect_direct(node);
    let Some(parent) = c.unit.ast.parent(node) else {
        return run;
    };
    let kids = c.collect_direct(parent);
    let Some(pos) = kids.iter().position(|&kid| kid == node) else {
        return run;
    };
    // The send-group pieces tile immediately before the node inside the
    // condition's `(...)` — walk left while each sibling's covered range
    // sits past the `(` (the node's own covered span opens at `(`),
    // keeping the operand `Identifier` and the send-group markers.
    let open = c.byte_span(node).start;
    let mut prefix: Vec<u32> = kids[..pos]
        .iter()
        .rev()
        .copied()
        .take_while(|&kid| {
            c.byte_span(kid).start >= open
                && (slice::is_send_group_piece(c, kid)
                    || c.kind(kid) == NodeKind::Identifier)
        })
        .collect();
    prefix.reverse();
    prefix.extend(run.drain(..));
    prefix
}

/// The for-statement condition face (:104-109): the condition segment
/// sits between the for-parts' two `;` separators.
fn for_parts_face(c: &mut Check<'_>, analysis: &dartq_semantic::Analysis<'_>) {
    let node = c.node();
    let Some(left) = c.field(node, PayloadField::LeftSeparator) else {
        return;
    };
    let Some(right) = c.field(node, PayloadField::RightSeparator) else {
        return;
    };
    let left_end = byte_span_of_token(c, left).end;
    let right_start = byte_span_of_token(c, right).start;
    let run = window_pieces(c, node, left_end, right_start);
    check_bool_target(c, analysis, &run);
}

/// The expression pieces tiling inside `[lo, hi)`: the send-group pieces
/// of a `for`-parts condition (`for (; a;)`) or a for-in iterable can tile
/// as the container's siblings under the enclosing statement while only the
/// completion wrapper is the direct child (the same adoption tiling
/// [`condition_run`] reads), and the condition's statement-level wrapper
/// (`ExpressionStatement` in `for (; a;)`) owns the send group instead of
/// the condition row itself. The run is every direct row of the node plus
/// the parent's rows whose covered range sits inside the window, sorted by
/// byte offset — a statement wrapper inside the window unwraps to its own
/// children (the wrapper itself drops out, it is the header's statement
/// marker not a value row).
fn window_pieces(c: &Check<'_>, node: u32, lo: usize, hi: usize) -> Vec<u32> {
    let inside = |row: u32| {
        let span = c.byte_span(row);
        span.start >= lo && span.end <= hi
    };
    let mut candidates: Vec<u32> = c.collect_direct(node).into_iter().collect();
    if let Some(parent) = c.unit.ast.parent(node) {
        for kid in c.collect_direct(parent) {
            if kid != node && !candidates.contains(&kid) {
                candidates.push(kid);
            }
        }
    }
    // A statement-level wrapper in the candidates (`for (; a;)`'s
    // `ExpressionStatement`) is the header's marker — its children are
    // the real expression pieces inside the window; the wrapper itself
    // drops out. Pieces still outside the window (a marker's own send
    // group that straddles the `;`) stay out.
    let mut run: Vec<u32> = Vec::new();
    for row in candidates {
        if c.kind(row) == NodeKind::ExpressionStatement {
            for kid in c.collect_direct(row) {
                if inside(kid) && !run.contains(&kid) {
                    run.push(kid);
                }
            }
        } else if inside(row) && !run.contains(&row) {
            run.push(row);
        }
    }
    run.sort_by_key(|&row| c.byte_span(row).start);
    run
}

/// The if-element condition face (:110-113): the condition rows sit
/// inside the element's `(...)` group — the rows whose byte range ends
/// before the group's close paren.
fn if_element_face(c: &mut Check<'_>, analysis: &dartq_semantic::Analysis<'_>) {
    let node = c.node();
    let mut depth = 0i32;
    let mut group_end = None;
    for token in c.unit.ast.token_range(node) {
        match c.lexeme(token) {
            "(" | "[" | "{" => depth += 1,
            ")" => {
                depth -= 1;
                if depth == 0 {
                    group_end = Some(byte_span_of_token(c, token).end);
                    break;
                }
            }
            "]" | "}" => {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            _ => {}
        }
    }
    let Some(group_end) = group_end else {
        return;
    };
    let run = window_pieces(c, node, c.byte_span(node).start, group_end);
    // The production tiling keeps a `ParenthesizedCondition` row for the
    // element's `(...)` group — its own dispatch (`condition_face`)
    // reports the same condition once; this face only fills the shape
    // where the group carries no dedicated row.
    if run
        .iter()
        .any(|&row| c.kind(row) == NodeKind::ParenthesizedCondition)
    {
        return;
    }
    check_bool_target(c, analysis, &run);
}

/// The conditional-expression condition face (:115-117): the rows before
/// the conditional's `?` operator judge against `bool`. The container's
/// own covered span anchors at the `?` (the delta-tile operator), so the
/// condition pieces tile as the node's left siblings under the parent —
/// the window is the token-adjacent tail ending at the operator.
fn conditional_face(c: &mut Check<'_>, analysis: &dartq_semantic::Analysis<'_>) {
    let node = c.node();
    let Some(op) = c.field(node, PayloadField::Question) else {
        return;
    };
    let op_start = byte_span_of_token(c, op).start;
    let run = window_pieces(c, node, 0, op_start);
    // The window opens at the source start, so a preceding statement's
    // rows could join — bound at the token-adjacent tail (the operand's
    // pieces chain back to the `?`; an earlier statement's do not).
    let run = slice::contiguous_tail(c, &run, op).to_vec();
    check_bool_target(c, analysis, &run);
}

/// The logical-binary face (the `boolType` operands of :39-60): both
/// operands of `&&`/`||` judge against `bool`.
fn logical_binary_face(c: &mut Check<'_>, analysis: &dartq_semantic::Analysis<'_>) {
    let Some(op) = c.field(c.node(), PayloadField::Operator) else {
        return;
    };
    if !matches!(c.lexeme(op), "&&" | "||") {
        return;
    }
    let Some((left, right)) = slice::binary_operands(c) else {
        return;
    };
    check_bool_target(c, analysis, &left);
    check_bool_target(c, analysis, &right);
}

/// The negation face (:120-121): the `!` operand judges against `bool`.
fn negation_face(c: &mut Check<'_>, analysis: &dartq_semantic::Analysis<'_>) {
    if c.lexeme(c.head_token(c.node())) != "!" {
        return;
    }
    let Some((kids, pos)) = slice::owner_slice(c) else {
        return;
    };
    let operand = slice::statement_run(c, &kids[..pos]);
    check_bool_target(c, analysis, &operand);
}

/// The shared boolean verdict: a non-empty run typed `dynamic` under a
/// `bool` target reports at the run.
fn check_bool_target(c: &mut Check<'_>, analysis: &dartq_semantic::Analysis<'_>, run: &[u32]) {
    if run.is_empty() || !source_is_dynamic(c, analysis, run) {
        return;
    }
    report_run(c, run);
}

/// The written type arguments of a collection literal: the `Type` rows
/// of the literal's own `TypeArguments` child (an untyped literal
/// carries none and the whole face stays silent — the official's
/// `typeArguments` read). `(name, nullable)` per argument, owned so the
/// faces read them across the `&mut` report boundary.
fn literal_type_arguments(c: &Check<'_>, literal: u32) -> Vec<(String, bool)> {
    // The `<T>` prefix anchors as a sibling sealing at the literal's `[`
    // (the literal row covers `[…]` only), so the row comes off the
    // parent's child window when it is not a direct child.
    let span = c.byte_span(literal);
    c.collect_direct(literal)
        .into_iter()
        .find(|&kid| c.kind(kid) == NodeKind::TypeArguments)
        .or_else(|| {
            c.unit.ast.parent(literal).and_then(|parent| {
                c.collect_direct(parent).into_iter().find(|&kid| {
                    c.kind(kid) == NodeKind::TypeArguments && c.byte_span(kid).end == span.start
                })
            })
        })
        .map(|args| {
            c.collect_direct(args)
                .into_iter()
                .filter(|&arg| c.kind(arg) == NodeKind::Type)
                .map(|arg| {
                    let face = type_face(c, arg);
                    (face.name.unwrap_or("").to_owned(), face.nullable)
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The collection-element faces (:127-186): a list/set element judges
/// against the written element type, a map entry's key and value
/// against the written key/value types, and a spread expression against
/// the whole `Iterable`/`Map` face (which admits unless `dynamic`).
fn collection_face(c: &mut Check<'_>, analysis: &dartq_semantic::Analysis<'_>) {
    let node = c.node();
    let args = literal_type_arguments(c, node);
    let is_map = c
        .unit
        .ast
        .token_range(node)
        .any(|token| c.lexeme(token) == ":");
    // The element pieces can tile as the literal's siblings under the
    // enclosing statement (the same send-group adoption `condition_run`
    // reads) — collect every row inside the literal's span.
    let span = c.byte_span(node);
    let raw = window_pieces(c, node, span.start, span.end);
    let segments = slice::comma_segments(c, node, &raw);
    if is_map && args.len() >= 2 {
        let (key, value) = (args[0].clone(), args[1].clone());
        for segment in &segments {
            // A spread judges against the whole map face; an entry
            // splits at its `:` — the `MapEntry` marker trails the
            // segment and carries the colon token (`PayloadField::Colon`,
            // the same boundary `declarations::entry_sides` reads).
            if segment
                .first()
                .is_some_and(|&row| c.kind(row) == NodeKind::Spread)
            {
                spread_face(c, analysis, segment, &key);
                continue;
            }
            let Some(marker) = segment
                .iter()
                .copied()
                .find(|&row| c.kind(row) == NodeKind::MapEntry)
            else {
                continue;
            };
            let Some(colon) = c.field(marker, PayloadField::Colon) else {
                continue;
            };
            let sides: Vec<u32> = segment
                .iter()
                .copied()
                .filter(|&row| row != marker)
                .collect();
            let split = sides
                .iter()
                .position(|&row| c.unit.ast.token_range(row).end > colon + 1)
                .unwrap_or(sides.len());
            check_typed_target(c, analysis, &sides[..split], &key);
            check_typed_target(c, analysis, &sides[split..], &value);
        }
    } else if !is_map && !args.is_empty() {
        let element = args[0].clone();
        for segment in &segments {
            if segment
                .first()
                .is_some_and(|&row| c.kind(row) == NodeKind::Spread)
            {
                spread_face(c, analysis, segment, &element);
                continue;
            }
            check_typed_target(c, analysis, segment, &element);
        }
    }
}

/// The spread arm: the expression judges against the whole collection
/// face — `Iterable<T>` / `Map<K, V>` (:151-155, :175-179) — which
/// admits unless `dynamic`/`void`/`Object?`.
fn spread_face(
    c: &mut Check<'_>,
    analysis: &dartq_semantic::Analysis<'_>,
    segment: &[u32],
    whole: &(String, bool),
) {
    let run = &segment[1..];
    if run.is_empty() || !source_is_dynamic(c, analysis, run) || !target_admits(&whole.0, whole.1) {
        return;
    }
    report_run(c, run);
}

/// The shared typed-target verdict for one element run.
fn check_typed_target(
    c: &mut Check<'_>,
    analysis: &dartq_semantic::Analysis<'_>,
    run: &[u32],
    target: &(String, bool),
) {
    if run.is_empty() {
        return;
    }
    if !target_admits(&target.0, target.1) || !source_is_dynamic(c, analysis, run) {
        return;
    }
    report_run(c, run);
}

/// The for-each faces (:198-235): the iterable judges against the
/// implicit `Iterable`/`Stream` target (`await for` streams), and a
/// typed loop variable judges when the iterable's written element type
/// is `dynamic` (the `List<dynamic>` face — the official reads the
/// iterable's static type argument).
fn for_each_face(c: &mut Check<'_>, analysis: &dartq_semantic::Analysis<'_>) {
    let node = c.node();
    let Some(in_token) = c
        .unit
        .ast
        .token_range(node)
        .find(|&token| c.lexeme(token) == "in")
    else {
        return;
    };
    let in_span = byte_span_of_token(c, in_token);
    let node_span = c.byte_span(node);
    // The loop-variable and iterable pieces can tile as the `ForIn`
    // container's siblings under the enclosing statement (the send-group
    // adoption [`window_pieces`] reads).
    let kids = window_pieces(c, node, node_span.start, node_span.end);
    let loop_var_rows: Vec<u32> = kids
        .iter()
        .copied()
        .filter(|&kid| c.byte_span(kid).end <= in_span.start)
        .collect();
    let iterable_rows: Vec<u32> = kids
        .iter()
        .copied()
        .filter(|&kid| c.byte_span(kid).start >= in_span.end)
        .collect();
    if iterable_rows.is_empty() {
        return;
    }
    // The iterable face: the target is `Iterable<dynamic>` (`Stream`
    // under `await for`) — it admits, so a `dynamic` iterable reports.
    if source_is_dynamic(c, analysis, &iterable_rows) {
        report_run(c, &iterable_rows);
        return;
    }
    // The loop-variable face: a written non-dynamic loop variable over
    // an iterable whose written element type is `dynamic` reports at
    // the iterable (the loop variable's type tiles as its left sibling,
    // the same declaration tiling).
    let Some(loop_name_row) = loop_var_rows
        .iter()
        .copied()
        .filter(|&row| c.kind(row) == NodeKind::Identifier)
        .next_back()
    else {
        return;
    };
    let Some(loop_type_row) =
        super::written_type_sibling(c, loop_name_row, &[NodeKind::Type, NodeKind::FunctionType])
    else {
        return;
    };
    let loop_face = type_face(c, loop_type_row);
    let Some(loop_type) = loop_face.name else {
        return;
    };
    if !target_admits(loop_type, loop_face.nullable) {
        return;
    }
    // The iterable's written element type (`List<dynamic>` — parameter,
    // local, field, or top-level, the same-file binding channel). A
    // non-bare iterable (`g()`/`a.b`) carries no same-file written face
    // and stays silent.
    let Some(iter_name_row) = iterable_rows
        .iter()
        .copied()
        .filter(|&row| c.kind(row) == NodeKind::Identifier)
        .next()
    else {
        return;
    };
    let iter_name = c.lexeme(c.head_token(iter_name_row));
    let Some((_, Some(iter_type_row))) = same_file::binding(c, node, iter_name) else {
        return;
    };
    let iter_face = type_face(c, iter_type_row);
    let Some(&(element, _element_nullable)) = iter_face.type_arguments.first() else {
        return;
    };
    if element == "dynamic" {
        report_run(c, &iterable_rows);
    }
}

/// The yield faces (:237-249): `yield` judges against the enclosing
/// written return type's first type argument, `yield*` against the
/// whole written return type.
fn yield_face(c: &mut Check<'_>, analysis: &dartq_semantic::Analysis<'_>) {
    let node = c.node();
    let run = c.collect_direct(node);
    if run.is_empty() || !source_is_dynamic(c, analysis, &run) {
        return;
    }
    let is_star = c.field(node, PayloadField::Star).is_some();
    if let Some((_, written)) = enclosing_return_face(c) {
        let admitted = if is_star {
            written
                .name
                .is_some_and(|name| target_admits(name, written.nullable))
        } else {
            written
                .type_arguments
                .first()
                .is_some_and(|&(name, nullable)| target_admits(name, nullable))
        };
        if admitted {
            report_run(c, &run);
        }
    }
}

/// The argument face (:67-74): the resolved callee's formal type
/// decides — a member invocation's member table, an implicit creation's
/// constructor table, or a bare top-level/implicit-this call's bridge
/// signature. An unresolvable callee stays silent.
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
        analysis
            .member_of(query_site(c, receiver), name)
            .and_then(|member| member.entry.formals.as_deref())
    } else {
        match analysis.resolve_constructor(query_site(c, selector_row), None) {
            Some(ctor) if ctor.exists => ctor.formals,
            _ => {
                // A bare top-level or implicit-this call: the bridge
                // signature face (a local callable's formals are not
                // modeled and stay silent).
                analysis
                    .function_signature_of(query_site(c, selector_row))
                    .map(|sig| sig.formals)
            }
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
