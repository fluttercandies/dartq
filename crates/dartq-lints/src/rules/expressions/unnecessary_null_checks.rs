use super::{Check, NodeKind, PayloadField, RuleSpec, query_site, slice, written_type_sibling};
use crate::rules::{TypeFace, type_face, utf16_token_span};
use dartq_semantic::model::Ty;

/// Official: `upstream Dart SDK source,
/// pkg/linter/lib/src/rules/unnecessary_null_checks.dart` — a `!`
/// whose context expects a nullable type reports at the operator
/// (:209-216: the expected type decides — removing the check cannot
/// change the program when the context accepts `null`). The expected
/// type faces (:26-161) cover returns, arrow bodies, variable
/// declarations, assignments, operator right sides, collection
/// members, yield values, and arguments.
///
/// dartq faces over the production tiling: the decidable context slice
/// ports — the variable/field/top-level-declaration initializer (the
/// written type row tiles as the declaration's left sibling,
/// [`written_type_sibling`], or among the declaration row's own
/// children), the return statement, the arrow body and the await
/// wrapper (:34-37), each through the nearest enclosing declaration's
/// written return type with the async flatten (:50-53 — only a
/// `Future`/`FutureOr` wrapper decides, through its first type
/// argument; a same-file typedef alias resolves first), the yield
/// statement (:59-72 — `Iterable`/`Stream` through the first type
/// argument), collection members (:107-129 — the written return's
/// iterable/map head; a map entry reads its side's argument, the
/// for/if element nesting is transparent at this read), and arguments
/// (:131-159 — the callee's bridge signature or constructor formal
/// table; the official `Future.value`/`Completer.complete` exceptions
/// fall out at the resolution boundary — SDK callees resolve to no
/// indexed signature). The enclosing-declaration lookup keeps the
/// official `thisOrAncestorOfType<FunctionExpression>` shape (:41):
/// class methods, constructors and factories never satisfy it, so
/// their return and arrow faces stay silent exactly like the oracle.
/// The pattern arm (:183-205) belongs to the pattern family and stays
/// unported; assignments and operator right sides under-report (the
/// promotable-LHS guard and the operator-member formal face make them
/// undecidable at M1). Message: `diagnostic.g.dart`
/// `unnecessaryNullChecks`.
pub(crate) static SPEC: RuleSpec =
    crate::spec!("unnecessary_null_checks", &[NodeKind::NonNullAssert], check,);

fn check(c: &mut Check<'_>) {
    // The wave posture: the rule reads the semantic face; face absent
    // is the plain-run posture and stays silent (the engine contract).
    if c.semantics().is_none() {
        return;
    }
    let node = c.node();
    // `getExpectedType`'s realNode (:30-37): the `!`'s nearest
    // ancestor whose own parent is not a transparent `(...)` or
    // `await` wrapper. An `await` on the way marks the async flatten
    // (:46-53 — `withAwait` reads the awaited type, not the Future).
    let mut real = node;
    let mut awaited = false;
    while let Some(parent) = c.unit.ast.parent(real)
        && matches!(
            c.kind(parent),
            NodeKind::ParenthesizedExpression | NodeKind::AwaitExpression
        )
    {
        awaited |= c.kind(parent) == NodeKind::AwaitExpression;
        real = parent;
    }
    let Some(mut context) = c.unit.ast.parent(real) else {
        return;
    };
    // The map entry's element nesting is transparent (:129-137): the
    // for/if element wrappers climb to their `SetOrMapLiteral`.
    while matches!(
        c.kind(context),
        NodeKind::ForControlFlow
            | NodeKind::IfControlFlow
            | NodeKind::ThenControlFlow
            | NodeKind::ElseControlFlow
    ) {
        let Some(outer) = c.unit.ast.parent(context) else {
            return;
        };
        context = outer;
    }
    let admits = match c.kind(context) {
        // `int? y = x!;` — the direct-initializer face (:95-98): the
        // written type row decides. The production tiling parks the
        // expression pieces flat in the initializer window, so the
        // `!` is the whole initializer exactly when it is the
        // window's tail row (`foo(x!)`'s pieces fill the argument
        // container and trail the `!`; a nested `x!.foo()` completes
        // later pieces — both carry no expected type, :132-159).
        NodeKind::VariableInitializer => {
            c.children(context).end - 1 == real
                && c.path
                    .iter()
                    .rev()
                    .copied()
                    .find(|&row| c.kind(row) == NodeKind::VariablesDeclaration)
                    .is_some_and(|decl| written_nullable(c, declaration_written_type(c, decl)))
        }
        // `int? f() { return x!; }` — the return's own expression
        // window roots the `!` and the enclosing declaration's
        // written return type decides (:40-57).
        NodeKind::ReturnStatement => {
            c.collect_direct(context).last() == Some(&real) && enclosing_return_admits(c, awaited)
        }
        // `Iterable<int?> f() sync* { yield x!; }` — the generator's
        // written return decides through its first type argument
        // (:59-72).
        NodeKind::YieldStatement => {
            c.collect_direct(context).last() == Some(&real)
                && yield_type(c).is_some_and(nullable_face)
        }
        // `List<int?> f() => [x!];` / `Set<int?> f() => {x!};` — the
        // literal's context-inferred element type (:107-114), read as
        // the written return's iterable head through its first type
        // argument.
        NodeKind::ListLiteral => member_admits(c, 0, awaited),
        // `Map<K, V> f() => {…: x!};` — the entry side's argument
        // (:115-129); the colon splits key from value.
        NodeKind::SetOrMapLiteral => member_admits(c, map_index(c, context, node), awaited),
        NodeKind::MapEntry => member_admits(c, entry_side(c, context, node), awaited),
        // `f(x!);` / `A(x!);` — the callee's formal at the `!`'s own
        // slot (:131-159).
        NodeKind::Arguments => argument_admits(c, context, real),
        // `int? f() => x!;` — the childless `ExpressionBody` handle
        // leaves the arrow expression as the declaration's own tail
        // rows, so the arrow posture reads at the declaration row
        // (:40-57's ExpressionFunctionBody arm). The tail row decides
        // the sub-face: the whole-body expression reads the return
        // face, a collection literal row reads its member slot (the
        // literal row completes after its element rows in the flat
        // window).
        NodeKind::TopLevelMethod
        | NodeKind::LocalFunctionDeclaration
        | NodeKind::FunctionExpression
        | NodeKind::NamedFunctionExpression => arrow_admits(c, context, real, node, awaited),
        _ => false,
    };
    if admits {
        let range = utf16_token_span(c, c.head_token(node));
        c.report(range, &[]);
    }
}

/// The arrow posture (`ExpressionFunctionBody`, :40-57): the flat window
/// parks the body rows before the childless `ExpressionBody` handle. The
/// literal row completes AFTER its element rows, so the collection faces
/// key on span containment; otherwise only a body that IS the `!` (modulo
/// the transparent wrappers) reads the return face.
fn arrow_admits(c: &Check<'_>, declaration: u32, real: u32, node: u32, awaited: bool) -> bool {
    let kids = c.collect_direct(declaration);
    if kids.len() < 2 || c.kind(kids[kids.len() - 1]) != NodeKind::ExpressionBody {
        return false;
    }
    let body = &kids[..kids.len() - 1];
    if let Some(literal) = body
        .iter()
        .rev()
        .copied()
        .find(|&row| {
            matches!(
                c.kind(row),
                NodeKind::ListLiteral | NodeKind::SetOrMapLiteral
            ) && belongs_to(c, row, node)
        })
    {
        let index = if c.kind(literal) == NodeKind::SetOrMapLiteral {
            map_index(c, literal, node)
        } else {
            0
        };
        return member_admits(c, index, awaited);
    }
    if let Some(entry) = body
        .iter()
        .rev()
        .copied()
        .find(|&row| c.kind(row) == NodeKind::MapEntry && belongs_to(c, row, node))
    {
        return member_admits(c, entry_side(c, entry, node), awaited);
    }
    body.last() == Some(&real) && return_admits_at(c, declaration, awaited)
}

/// The map-entry slot a `!` inside `literal` belongs to: the entry whose
/// span contains `node` (the for/if element nesting is transparent,
/// :129-137), read through its colon; a literal with no entry ancestor
/// reads the set face (slot 0).
pub(super) fn map_index(c: &Check<'_>, literal: u32, node: u32) -> usize {
    entry_containing(c, literal, node).map_or(0, |entry| entry_side(c, entry, node))
}

/// The `MapLiteralEntry` an `inner` marker belongs to: the entry that
/// started last at or before the marker, else the one that starts just
/// after it (a key-side marker precedes its own entry's value row).
///
/// The literal is childless in the production tiling — its entry rows leak
/// as siblings in the enclosing window, and a for/if element wraps them
/// one level down — so the scan covers the literal's own window plus the
/// sibling run inside its token extent.
pub(super) fn entry_containing(c: &Check<'_>, literal: u32, inner: u32) -> Option<u32> {
    let marker = c.byte_span(inner).start;
    let extent = c.byte_span(literal);
    let mut pending: Vec<u32> = Vec::new();
    pending.extend(c.children(literal).start..c.children(literal).end);
    if let Some(owner) = c.unit.ast.parent(literal) {
        pending.extend((c.children(owner).start..c.children(owner).end).filter(|&row| {
            row != literal && {
                let span = c.byte_span(row);
                span.start >= extent.start && span.start <= extent.end
            }
        }));
    }
    let mut before: Option<(usize, u32)> = None;
    let mut after: Option<(usize, u32)> = None;
    while let Some(row) = pending.pop() {
        match c.kind(row) {
            NodeKind::MapEntry => {
                let start = c.byte_span(row).start;
                if start <= marker {
                    if before.is_none_or(|(seen, _)| start >= seen) {
                        before = Some((start, row));
                    }
                } else if after.is_none_or(|(seen, _)| start <= seen) {
                    after = Some((start, row));
                }
            }
            NodeKind::IfControlFlow
            | NodeKind::ThenControlFlow
            | NodeKind::ElseControlFlow
            | NodeKind::ForControlFlow
            | NodeKind::SetOrMapLiteral => pending.extend(c.children(row).start..c.children(row).end),
            _ => {}
        }
    }
    before.or(after).map(|(_, row)| row)
}

/// Whether `inner` belongs to the row `outer` — the trailing-marker
/// containment: a `!` that seals a literal/entry seals the row's span at
/// its own offset (`{'': p!}` places the `!` exactly at the entry's end),
/// so the end is inclusive. Only `inner`'s START is compared: the marker
/// itself may extend past the row it terminates.
pub(super) fn belongs_to(c: &Check<'_>, outer: u32, inner: u32) -> bool {
    let outer_span = c.byte_span(outer);
    let inner_span = c.byte_span(inner);
    outer_span.start <= inner_span.start && inner_span.start <= outer_span.end
}

/// A nullable-or-`dynamic` expected-type face (the `isNullable` read).
fn nullable_face((name, nullable): (&str, bool)) -> bool {
    nullable || name == "dynamic"
}

/// The expected-type slot of a map entry member: the colon splits the
/// key (0) from the value (1) — the value's first token follows the
/// colon.
pub(super) fn entry_side(c: &Check<'_>, entry: u32, real: u32) -> usize {
    let key_side = c
        .field(entry, PayloadField::Colon)
        .is_some_and(|colon| c.unit.ast.token_range(real).end <= colon + 1);
    if key_side {
        0
    } else {
        1
    }
}

/// The written type row of the declaration an initializer belongs to:
/// the local shape tiles the type as the declaration's left sibling
/// ([`written_type_sibling`]); the top-level shape parks it among the
/// declaration row's own children (`TopLevelFields`' flat window); the
/// field shape nests the declaration in a `Fields` row whose own type
/// child heads the member group.
fn declaration_written_type(c: &Check<'_>, decl: u32) -> Option<u32> {
    written_type_sibling(c, decl, &[NodeKind::Type])
        .or_else(|| {
            c.collect_direct(decl)
                .into_iter()
                .find(|&kid| matches!(c.kind(kid), NodeKind::Type | NodeKind::FunctionType))
        })
        // The top-level shape tiles the window flat — the written type
        // sits in the `TopLevelFields` row with the variable's own name
        // row between it and the declaration (`[Identifier, Type,
        // Identifier, VariablesDeclaration]`), so the nearest
        // preceding type row in the window is the written type.
        .or_else(|| {
            let members = c.collect_direct(c.unit.ast.parent(decl)?);
            let index = members.iter().position(|&m| m == decl)?;
            members[..index]
                .iter()
                .rev()
                .copied()
                .find(|&m| matches!(c.kind(m), NodeKind::Type | NodeKind::FunctionType))
        })
        .or_else(|| {
            let fields = c.unit.ast.parent(decl)?;
            let type_row = c
                .collect_direct(fields)
                .into_iter()
                .find(|&kid| matches!(c.kind(kid), NodeKind::Type | NodeKind::FunctionType));
            (c.kind(fields) == NodeKind::Fields).then_some(type_row)?
        })
}

/// The written type row's nullable verdict (the declaration-context
/// face — an `async` local variable is not a thing).
fn written_nullable(c: &Check<'_>, type_row: Option<u32>) -> bool {
    let Some(type_row) = type_row else {
        return false;
    };
    let face = type_face(c, type_row);
    face.nullable || face.name == Some("dynamic")
}

/// The nearest enclosing function-like declaration's written return
/// type with the async flatten (:40-57). The official lookup is
/// `thisOrAncestorOfType<FunctionExpression>` (:41): class methods,
/// constructors and factories never satisfy it, so their return faces
/// stay silent exactly like the oracle; inferred lambdas carry no
/// written face.
fn enclosing_return_admits(c: &Check<'_>, awaited: bool) -> bool {
    enclosing_function_row(c).is_some_and(|row| return_admits_at(c, row, awaited))
}

/// The enclosing function-shaped declaration row, when the nearest
/// function-like ancestor is a function shape (the official
/// `thisOrAncestorOfType<FunctionExpression>` cut, :41).
fn enclosing_function_row(c: &Check<'_>) -> Option<u32> {
    for &row in c.path.iter().rev().skip(1) {
        match c.kind(row) {
            NodeKind::TopLevelMethod
            | NodeKind::LocalFunctionDeclaration
            | NodeKind::FunctionExpression
            | NodeKind::NamedFunctionExpression => return Some(row),
            NodeKind::Method
            | NodeKind::Constructor
            | NodeKind::Factory
            | NodeKind::CompilationUnit => return None,
            _ => {}
        }
    }
    None
}

/// The enclosing function's written return face, when the nearest
/// function-like ancestor is a function shape (the same lookup cut as
/// [`enclosing_return_admits`]).
fn enclosing_return_face<'a>(c: &'a Check<'_>) -> Option<TypeFace<'a>> {
    super::void_checks::written_return_face(c, enclosing_function_row(c)?)
}

/// The `isNullable` read on an argument position of a collection-shape
/// return face (`:69-71`, `:108-129`): the head names the shape, the
/// indexed type argument's own face decides.
fn shape_argument<'a>(
    face: &TypeFace<'a>,
    heads: &[&str],
    index: usize,
) -> Option<(&'a str, bool)> {
    let name = face.name?;
    if !heads.contains(&name) {
        return None;
    }
    face.type_arguments.get(index).copied()
}

/// The yield face (:59-72): `Iterable`/`Stream` decide through the
/// first type argument — the shared expected-type face the
/// `null_check_on_nullable_type_parameter` operand read consumes too.
pub(super) fn yield_type<'a>(c: &'a Check<'_>) -> Option<(&'a str, bool)> {
    let face = enclosing_return_face(c)?;
    shape_argument(&face, &["Iterable", "Stream"], 0)
}

/// The collection-member face (:107-129): the written return's head —
/// `List`/`Set`/`Iterable` at the element index, `Map` at the entry
/// side's index — decides through the indexed type argument. The
/// `await`/`async` flatten (:46-53) unwraps one `Future`/`FutureOr`
/// first: under an `async` return the literal's static type is the
/// context-inferred (unwrapped) container.
pub(super) fn member_type<'a>(
    c: &'a Check<'_>,
    index: usize,
    awaited: bool,
) -> Option<(&'a str, bool)> {
    let declaration = enclosing_function_row(c)?;
    let face = enclosing_return_face(c)?;
    if !awaited && !is_async(c, declaration) {
        return shape_argument(&face, &["List", "Set", "Iterable", "Map"], index);
    }
    let unwrapped = flattened_argument(c, declaration, &face)?;
    shape_argument(&unwrapped, &["List", "Set", "Iterable", "Map"], index)
}

/// The single argument face of the `Future`/`FutureOr` wrapper the
/// declaration returns (`Future<List<T>>` → `List<T>`).
///
/// The argument list tiles flat: its window also carries the nested
/// rows (`Future<List<T>>` completes `<T>`, `List`, then the outer list),
/// so the argument is the `Type` row that starts FIRST — the nested ones
/// always start later. Reading the args row's `type_arguments` in
/// completion order would answer the nested `T` instead.
fn flattened_argument<'a>(
    c: &'a Check<'_>,
    declaration: u32,
    face: &TypeFace<'a>,
) -> Option<TypeFace<'a>> {
    if !matches!(face.name, Some("Future" | "FutureOr")) {
        return None;
    }
    let row = written_return_row(c, declaration)?;
    let after = c.byte_span(row).start;
    let owner = c.unit.ast.parent(row)?;
    let outer = (c.children(owner).start..c.children(owner).end)
        .filter(|&kid| c.kind(kid) == NodeKind::Type && c.byte_span(kid).start > after)
        .min_by_key(|&kid| c.byte_span(kid).start)?;
    Some(type_face(c, outer))
}

/// The written return type ROW of a method-like declaration — the row
/// [`super::void_checks::written_return_face`] reads the face from.
fn written_return_row(c: &Check<'_>, function: u32) -> Option<u32> {
    let kids = c.collect_direct(function);
    let is_declaration_name = |kid: u32| {
        c.kind(kid) == NodeKind::Identifier
            && matches!(
                c.field(kid, PayloadField::Context),
                Some(ctx)
                    if ctx == dartq_parser::IdentifierContext::MethodDeclaration as u32
                        || ctx
                            == dartq_parser::IdentifierContext::TopLevelFunctionDeclaration
                                as u32
            )
    };
    let name = kids
        .iter()
        .copied()
        .find(|&kid| is_declaration_name(kid))
        .or_else(|| {
            kids.iter()
                .copied()
                .find(|&kid| c.kind(kid) == NodeKind::Identifier)
        })?;
    let (row, kind) = crate::rules::type_before(c, function, name)?;
    matches!(
        kind,
        NodeKind::Type | NodeKind::FunctionType | NodeKind::VoidType
    )
    .then_some(row)
}

/// Whether the declaration carries the `async`/`async*` body keyword.
fn is_async(c: &Check<'_>, declaration: u32) -> bool {
    c.unit
        .ast
        .token_range(declaration)
        .any(|token| c.lexeme(token) == "async")
}

/// The collection-member nullable verdict (:107-129).
fn member_admits(c: &Check<'_>, index: usize, awaited: bool) -> bool {
    member_type(c, index, awaited).is_some_and(nullable_face)
}

/// The argument face (:131-159): the callee's formal type at the
/// dispatch node's own argument slot — a positional slot by index, a
/// named slot by label — through the bridge signature (a bare callee)
/// or the constructor formal table (a creation head). The official
/// `Future.value`/`Completer.complete` exceptions (:138-157) need no
/// port of their own: both are SDK callees, and the resolution faces
/// answer `None` for every unindexed target.
fn argument_admits(c: &Check<'_>, arguments: u32, real: u32) -> bool {
    argument_formal_ty(c, arguments, real).is_some_and(|ty| {
        matches!(ty, Ty::Dynamic | Ty::Named { nullable: true, .. })
    })
}

/// The callee's formal type at the dispatch node's own argument slot
/// (`None` on every unresolvable or out-of-slot shape); `real` is the
/// `!`'s value node with its transparent wrappers collapsed.
fn argument_formal_ty(c: &Check<'_>, arguments: u32, real: u32) -> Option<Ty> {
    let analysis = c.semantics()?;
    let parent = c.unit.ast.parent(arguments)?;
    let kids = c.collect_direct(parent);
    let pos = kids.iter().position(|&kid| kid == arguments)?;
    // The invocation send is the sibling right after the argument list
    // (the production tiling).
    if c.kind(*kids.get(pos.checked_add(1)?)?) != NodeKind::Send {
        return None;
    }
    let (head, _) = slice::send_parts(c, &kids, pos + 1)?;
    // The dispatch node's argument slot: the operand rows end at the
    // piece before the leaf (a bare-read operand tiles as a send group
    // and strands the `!` leaf outside its argument range, so the slot
    // range is the one covering `node_pos - 1`). A named argument
    // pairs `name … NamedArgument` immediately before the value range —
    // the formal reads by label there; otherwise positionally by the
    // count of preceding positional ranges.
    // The slot's range: the one holding the piece before the `!`'s own
    // node — a bare-read operand tiles as a send group and the trailing
    // `!` seals its own range, so the value range is the one before it.
    // A transparent `(...)` wrapper is the value itself, so the `!`'s
    // own range is the fallback.
    let raw = c.collect_direct(arguments);
    let ranges = slice::arguments(c, &raw);
    let anchor = raw
        .iter()
        .position(|&kid| kid == real)
        .unwrap_or(0)
        .saturating_sub(1);
    let bang = raw.iter().position(|&kid| kid == c.node());
    let index = ranges
        .iter()
        .position(|range| range.contains(&anchor))
        .or_else(|| {
            bang.and_then(|position| ranges.iter().position(|range| range.contains(&position)))
        })?;
    // A named argument's childless marker tiles inside its own segment
    // (the merged shape: name, marker, value rows in one range) or as
    // the previous segment's tail (the pair shape) — the label reads
    // off the marker either way (`slice::named_arg_name`).
    let is_named = |range: &std::ops::Range<usize>| {
        range
            .clone()
            .any(|piece| c.kind(raw[piece]) == NodeKind::NamedArgument)
    };
    let label = {
        let segment = if is_named(&ranges[index]) {
            Some(index)
        } else {
            index.checked_sub(1).filter(|&prev| is_named(&ranges[prev]))
        };
        segment.and_then(|s| {
            let marker = ranges[s]
                .clone()
                .find(|&piece| c.kind(raw[piece]) == NodeKind::NamedArgument)?;
            let name_idx = slice::named_arg_name(c, &raw, marker)?;
            Some(c.lexeme(c.head_token(raw[name_idx])))
        })
    };
    let site = query_site(c, kids[head]);
    let signature = analysis.function_signature_of(site);
    // A creation head (`A(x!)`): the constructor formal table at
    // the unnamed selector when no plain signature answers.
    let binding = if signature.is_none() {
        analysis.resolve_constructor(site, None)
    } else {
        None
    };
    let formals = match (&signature, &binding) {
        (Some(sig), _) => Some(sig.formals),
        (None, Some(binding)) if binding.exists => binding.formals,
        _ => None,
    };
    let Some(formals) = formals else {
        return None;
    };
    if let Some(label) = label {
        return Some(formals.named_formal(analysis.strings().intern(label))?.ty);
    }
    let slot = ranges[..index].iter().filter(|r| !is_named(r)).count();
    Some(formals.positional.get(slot)?.ty)
}

/// A same-file typedef's aliased type face (`typedef F = Future<int?>;`
/// — the `=` body's `Type` row), one level: the written return's head
/// resolves through it before the async read (:45-56).
fn alias_face<'a>(c: &'a Check<'_>, name: &str) -> Option<TypeFace<'a>> {
    let root = c.path.first().copied()?;
    for top in c.collect_direct(root) {
        for inner in c.collect_direct(top) {
            if c.kind(inner) != NodeKind::Typedef {
                continue;
            }
            let is_named = c.collect_direct(inner).into_iter().any(|kid| {
                c.kind(kid) == NodeKind::Identifier && c.lexeme(c.head_token(kid)) == name
            });
            if !is_named {
                continue;
            }
            let body = c
                .collect_direct(inner)
                .into_iter()
                .find(|&kid| c.kind(kid) == NodeKind::Type)?;
            return Some(type_face(c, body));
        }
    }
    None
}

/// The declaration's own written return type, read at the row.
fn return_admits_at(c: &Check<'_>, declaration: u32, awaited: bool) -> bool {
    let written = super::void_checks::written_return_face(c, declaration);
    admits_return(c, declaration, written, awaited)
}

/// The `isNullable` read on the flattened expected type (:50-57).
fn admits_return(
    c: &Check<'_>,
    declaration: u32,
    written: Option<TypeFace<'_>>,
    awaited: bool,
) -> bool {
    let Some(mut face) = written else {
        return false;
    };
    // A same-file typedef alias resolves first (one level).
    if let Some(alias) = face.name.and_then(|name| alias_face(c, name)) {
        face = alias;
    }
    let Some(name) = face.name else {
        return false;
    };
    if awaited || is_async(c, declaration) {
        // Only the `Future`/`FutureOr` wrapper decides (:50-53) — a
        // bare `dynamic` async return carries no expected type.
        return shape_argument(&face, &["Future", "FutureOr"], 0).is_some_and(nullable_face);
    }
    face.nullable || name == "dynamic"
}
