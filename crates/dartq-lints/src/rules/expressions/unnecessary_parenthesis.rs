use super::{Check, NodeKind, PayloadField, RuleSpec};
use crate::rules::slice;

/// The kinds whose rows are completion wrappers or delimiters rather
/// than semantic parents — excluded from the covering-container read.
fn is_wrapper_kind(kind: NodeKind) -> bool {
    matches!(
        kind,
        NodeKind::Send
            | NodeKind::NoArguments
            | NodeKind::Arguments
            | NodeKind::NamedArgument
            | NodeKind::NoTypeArguments
            | NodeKind::TypeArguments
    )
}

/// The innermost enclosing operator container of the paren: the
/// smallest sibling row in the owner window whose span covers the
/// paren's own (operator containers complete after their operands, so
/// an adopted-begin container trails the paren row; a prefix container
/// anchored at its operator may precede it). This is the upstream
/// `parent` expression for the operator-family faces.
fn container_parent(c: &Check<'_>, node: u32, owner_kids: &[u32], pos: usize) -> Option<NodeKind> {
    let node_span = c.utf16_span(node);
    let mut best: Option<(u32, NodeKind)> = None;
    for (i, &sib) in owner_kids.iter().enumerate() {
        if i == pos || is_wrapper_kind(c.kind(sib)) || c.kind(sib) == c.kind(node) {
            continue;
        }
        let span = c.utf16_span(sib);
        if span.start <= node_span.start
            && span.end >= node_span.end
            && span != node_span
            && best.is_none_or(|(size, _)| span.end - span.start < size)
        {
            best = Some((span.end - span.start, c.kind(sib)));
        }
    }
    best.map(|(_, kind)| kind)
}

/// Whether the paren's direct children carry a row of `kind` — the
/// inner expression's shape read (the inner tiles as one or more
/// sibling rows; the semantic inner kind can sit behind the head row,
/// so the whole direct-child band answers, never a deeper descendant).
fn inner_has_kind(c: &Check<'_>, node: u32, kind: NodeKind) -> bool {
    c.collect_direct(node)
        .into_iter()
        .any(|kid| c.kind(kid) == kind)
}

/// The operator lexeme of the paren's inner binary expression (`(a ==
/// b)`'s `==`), when the inner carries a [`NodeKind::BinaryExpression`]
/// row with an `Operator` payload.
fn binary_child_operator<'a>(c: &'a Check<'_>, node: u32) -> Option<&'a str> {
    c.collect_direct(node).into_iter().find_map(|kid| {
        if c.kind(kid) != NodeKind::BinaryExpression {
            return None;
        }
        c.field(kid, PayloadField::Operator).map(|op| c.lexeme(op))
    })
}

/// Whether the paren's subtree carries a function expression — the
/// upstream `containsFunctionExpression` deep read (a lambda anywhere
/// inside, not only at the head).
fn contains_function_expr(c: &Check<'_>, node: u32) -> bool {
    let window = c.children(node);
    (window.start..window.end).any(|row| {
        matches!(
            c.kind(row),
            NodeKind::FunctionExpression | NodeKind::NamedFunctionExpression
        )
    })
}

/// Whether the paren's token range carries a cascade link (`..`/`?..`)
/// — the `(a?..b = true)` shape whose inner cascade piece does not tile
/// as a `Cascade` row under every owner.
fn has_cascade_token(c: &Check<'_>, node: u32) -> bool {
    c.unit
        .ast
        .token_range(node)
        .any(|token| matches!(c.lexeme(token), ".." | "?.."))
}

/// Whether a `...`/`...?` spread token directly precedes the paren's
/// `(` — the spread position's token-face spelling when the childless
/// `Spread` piece does not tile as the preceding sibling.
fn spread_token_before(c: &Check<'_>, node: u32) -> bool {
    let Some(lparen) = c.unit.ast.token_range(node).next() else {
        return false;
    };
    lparen > 0 && matches!(c.lexeme(lparen - 1), "..." | "...?")
}

/// Whether the paren sits directly inside a `${...}` interpolation —
/// the upstream `InterpolationExpression` parent face (the `${` opener
/// directly before `(`).
fn interpolation_token_before(c: &Check<'_>, node: u32) -> bool {
    let Some(lparen) = c.unit.ast.token_range(node).next() else {
        return false;
    };
    lparen > 0 && c.lexeme(lparen - 1) == "${"
}

/// Whether a `!` postfix directly follows the paren's `)` — the
/// `(a?[0])!` null-check face. The row's token range may spill past the
/// closer (the delta-tile anchor convention), so the pair reads off the
/// range walk rather than the range tail.
fn postfix_bang_after(c: &Check<'_>, node: u32) -> bool {
    let mut previous_close = false;
    for token in c.unit.ast.token_range(node) {
        let lexeme = c.lexeme(token);
        if lexeme == "!" && previous_close {
            return true;
        }
        previous_close = lexeme == ")";
    }
    false
}

/// Official: `upstream Dart SDK source, pkg/linter/lib/src/rules/unnecessary_parenthesis.dart`
/// — a parenthesized expression reports when removing the parentheses
/// keeps the meaning (:39-232, the decision order preserved below). The
/// dartq faces over the production tiling (the paren row is a container:
/// its children tile the inner expression; the enclosing owner slice
/// carries the access-chain and statement pieces):
///
/// - bail faces ported exactly: constant-pattern parent (:41), spread
///   position (the childless `Spread` piece precedes the paren, :45),
///   nested parens (:67), condition segments — the upstream
///   if/while/do/switch direct-parent faces (:68-73) all land as the
///   [`NodeKind::ParenthesizedCondition`] parent here — a conditional
///   inner (:73), cascade positions (inner or ancestor, :104-107), a
///   bare constructor-field-initializer position with a function
///   expression inside (:119-131, the `=>` token face), the
///   `==`/`!=`-under-assignment family (:133-139), a statement-initial
///   switch expression (:146-149), the whitespace-readability family
///   (:151-166 with the exemption set), the operator parents (:169-176),
///   directly-invoked parens (the `FunctionExpressionInvocation` face
///   :178-180), the prefix/postfix access targets (:181-192), and the
///   statement-initial set-or-map literal (:195-197).
/// - report faces ported: the single-positional-argument parent (:69),
///   the one-token inner (:113-121 over a 3-token paren), and the
///   null-aware chain inner (the `containsNullAwareInvocationInChain`
///   extension, extensions.dart:211-224, over a chain-head piece run) —
///   each with the upstream parent-name exemptions
///   (`hashCode`/`runtimeType` accessors, `noSuchMethod`/`toString`
///   invocations, a `!` postfix, :75-101).
///
/// The record-typed parameter exemptions (:75-86
/// `correspondingParameter.type is RecordType`) read the same-file
/// written type of the corresponding formal: the callee resolves as a
/// bare name to exactly one same-file top-level function whose formal
/// at the argument's position (or named label) carries a written
/// [`NodeKind::RecordType`]. A `TypeLiteral` inner (:105-106) is the
/// `name<...>` shape whose name binds to no same-file value (a
/// same-file value spells a generic tear-off, a `FunctionReference`
/// the official keeps reporting); the extension-nullable bail
/// (:117-124, :132-139) judges the accessor per RESOLVED member over
/// the receiver's written type
/// ([`super::same_file::nullable_extension_member`]): the applicable
/// same-file declarers split by `on`-type nullability (a non-nullable
/// `on` type is always the more specific application and reports), a
/// resolved interface member — same-file chain or the semantic
/// member walk — reports, and an accessor no same-file face declares
/// keeps the parens (it may resolve through an import, where the
/// upstream resolver stays silent over a nullable on type).
///
/// Documented gaps: a member or prefixed callee (`obj.g((3))`,
/// `p.G((3))`) stays unresolved and reports; a generic tear-off of a
/// value imported from another file bails as a type literal (the
/// name carries no same-file value binding); an accessor carried by
/// an imported non-nullable extension, or genuinely undefined, keeps
/// the parens (the import-through keep — the under-report family);
/// extension applicability reads at written-name level, so a
/// subclass receiver misses a superclass-on-type extension. The
/// null-aware-index-in-then-arm bail (:144-157) reads off the tiling:
/// the paren as a conditional piece with both arms populated is the
/// then expression, and a set-or-map key position answers on the
/// `{`/`,` token before the `(` (the entry value pieces trail the
/// `:`). The invoked constructor-tear-off inner tiles flat (a send
/// group, no `ConstructorReference` row), so the :207-216 block also
/// answers through the token face ([`constructor_reference_inner`]).
/// Message: `diagnostic.g.dart` `unnecessaryParenthesis`.
pub(crate) static SPEC: RuleSpec = crate::spec!(
    "unnecessary_parenthesis",
    &[NodeKind::ParenthesizedExpression],
    check,
);

fn check(c: &mut Check<'_>) {
    let node = c.node();
    let Some(parent) = c.ancestor(1) else {
        return;
    };
    let parent_kind = c.kind(parent);
    // `case const (a + b):` (:41).
    if parent_kind == NodeKind::ConstantPattern {
        return;
    }
    let owner_kids = c.collect_direct(parent);
    let Some(pos) = owner_kids.iter().position(|&kid| kid == node) else {
        return;
    };
    // `[...(p as List)]` (:45) — the childless spread piece precedes,
    // spelled either by its row kind or the `...` token before `(`.
    if pos > 0 && c.kind(owner_kids[pos - 1]) == NodeKind::Spread {
        return;
    }
    if spread_token_before(c, node) {
        return;
    }
    // `(int,) r = (3);` (:47-53) — the same-file written record type of
    // the variable keeps the parentheses.
    if matches!(
        parent_kind,
        NodeKind::VariableInitializer | NodeKind::InitializedIdentifier
    ) && variable_declares_record_type(c)
        && !inner_has_kind(c, node, NodeKind::RecordLiteral)
    {
        return;
    }
    // `g((3)); => g((int,) i) { }` / `g(i: (3)); =>
    // g({required (int,) i}) { }` (:75-86) — the same-file written
    // record type of the corresponding parameter keeps the parentheses.
    if parent_kind == NodeKind::Arguments
        && !inner_has_kind(c, node, NodeKind::RecordLiteral)
        && argument_parameter_declares_record_type(c, node, parent)
    {
        return;
    }
    let inner = c.collect_direct(node);
    let Some(&inner_first) = inner.first() else {
        return;
    };
    // Always-report set (:67-73, :93): nested parens, string
    // interpolation, single positional argument, and the condition
    // segments (the upstream if/while/do/switch direct parents).
    if parent_kind == NodeKind::ParenthesizedExpression
        || parent_kind == NodeKind::ParenthesizedCondition
        || (parent_kind == NodeKind::Arguments && slice::arguments(c, &owner_kids).len() == 1)
        || interpolation_token_before(c, node)
    {
        report(c, node);
        return;
    }
    // `(foo ? bar : baz)` (:73).
    if inner_has_kind(c, node, NodeKind::Conditional) {
        return;
    }
    // `(List<int>).toString()` (:105-106): the inner is a type literal —
    // `name<...>` whose name binds to no same-file value.
    if type_literal_inner(c, node) {
        return;
    }
    // The one-token / null-aware-chain report block (:78-102), with the
    // parent-name exemptions.
    let one_token = c.unit.ast.token_range(node).len() == 3
        && matches!(
            c.kind(inner_first),
            NodeKind::Identifier
                | NodeKind::LiteralBool
                | NodeKind::LiteralDouble
                | NodeKind::LiteralInt
                | NodeKind::LiteralNull
                | NodeKind::StringLiteral
        );
    // A cascade anywhere inside means the inner is a cascade section —
    // the upstream chain walk answers `false` there (the cascade bail
    // below owns it), so `?..` and the `?.` links inside one never open
    // the null-aware block.
    let null_aware_chain = has_null_aware_token(c, node) && !has_cascade_token(c, node);
    if one_token || null_aware_chain {
        // `(String).hashCode` / `(String).runtimeType` (:82-88).
        if let Some(name) = access_selector_name(c, &owner_kids, pos) {
            if matches!(name, "hashCode" | "runtimeType") {
                return;
            }
            // `(String).noSuchMethod()` / `(String).toString()` (:77-81).
            if matches!(name, "noSuchMethod" | "toString")
                && owner_kids[pos + 2..]
                    .iter()
                    .any(|&piece| c.kind(piece) == NodeKind::Arguments)
            {
                return;
            }
            // The extension-nullable bail (:117-124, :132-139):
            // parentheses are required to stop null-aware shorting
            // when the accessor resolves to an extension member whose
            // `on` type is nullable — judged per resolved member over
            // the receiver's written type ([`super::same_file::
            // nullable_extension_member`]); an accessor no same-file
            // face declares may resolve through imports, and the same
            // keep covers it.
            if super::same_file::nullable_extension_member(c, node, name) {
                return;
            }
        }
        // `(p)!` / `(a?[0])!` / `(a?.sign)!` (:144-147): the `!` postfix
        // parent keeps the parentheses — over a null-aware chain they
        // stop the shorting, over a one-token inner they are the
        // upstream unconditional bail.
        if postfix_bang_after(c, node) {
            return;
        }
        // The null-aware-index inner keeps the parens in every oracle
        // corpus shape (`a ? (b?[c]) : d` and `{(a?[b]): c}` stop a
        // nested conditional `?`, `(a?[0])!` and `(a?[0]).x` stop the
        // null-aware shorting — upstream :82-157); the paren's own tiling
        // carries the `?[` pair, so the bail is parent-shape-free.
        if has_null_aware_index_inner(c, node) {
            return;
        }
        report(c, node);
        return;
    }
    // `(C.new)` / `(List<int>.filled)` (:210-216) — the
    // ConstructorReference block reports unless the invocation carries
    // explicit type arguments (the upstream `parent is
    // FunctionExpressionInvocation && parent.typeArguments != null`
    // exemption; the FEI keep below covers that shape). The flat
    // send-group tiling of the invoked tear-off answers through the
    // token face ([`constructor_reference_inner`]).
    if inner_has_kind(c, node, NodeKind::ConstructorReference)
        || constructor_reference_inner(c, node)
    {
        if !owner_kids
            .get(pos + 1)
            .is_some_and(|&piece| c.kind(piece) == NodeKind::TypeArguments)
        {
            report(c, node);
            return;
        }
    }
    // `a..b = (c..d)` (:218-222): any cascade position keeps the parens
    // (a cascade inner row, a `..`/`?..` link token, or a cascade
    // ancestor).
    if inner_has_kind(c, node, NodeKind::Cascade)
        || has_cascade_token(c, node)
        || c.path.iter().rev().any(|&a| c.kind(a) == NodeKind::Cascade)
    {
        return;
    }
    // The constructor-field-initializer guard (:226-241): a bare
    // initializer position over an inner function expression keeps the
    // parens (a bare `=>` would read as the body delimiter).
    if bare_in_initializer(c) && contains_function_expr(c, node) {
        return;
    }
    // `foo = (a == b)` / `return (count != 0)` (:243-253).
    if binary_child_operator(c, node).is_some_and(|op| matches!(op, "==" | "!="))
        && (matches!(
            parent_kind,
            NodeKind::Assignment
                | NodeKind::InitializedIdentifier
                | NodeKind::VariableInitializer
                | NodeKind::ReturnStatement
                | NodeKind::YieldStatement
        ) || c
            .path
            .iter()
            .rev()
            .any(|&a| c.kind(a) == NodeKind::Initializer))
    {
        return;
    }
    // `switch` at the beginning of a statement (:255-257).
    let statement_owned = owner_kids[pos + 1..].iter().any(|&sib| {
        c.kind(sib) == NodeKind::ExpressionStatement
            && c.utf16_span(sib).start <= c.utf16_span(node).start
            && c.utf16_span(sib).end >= c.utf16_span(node).end
    });
    if statement_owned && inner_has_kind(c, node, NodeKind::SwitchExpression) {
        return;
    }
    // The whitespace-readability family (:259-272): keep the parens
    // unless an exemption parent answers. The unary-operator operand
    // face: the production tiling seals the prefix/postfix operator
    // childless beside the paren, so the tiled parent kind reads the
    // enclosing statement/initializer — the upstream direct parent is
    // the `PrefixExpression`/`PostfixExpression`, which is never in the
    // exemption set, and the keep stands.
    if whitespace_inner(c, node) {
        let exempt = !operand_of_unary(c, node, &owner_kids, pos)
            && matches!(
                parent_kind,
                NodeKind::Assignment
                    | NodeKind::RecordLiteral
                    | NodeKind::ReturnStatement
                    | NodeKind::YieldStatement
                    | NodeKind::InitializedIdentifier
                    | NodeKind::VariableInitializer
                    | NodeKind::Arguments
                    | NodeKind::NamedArgument
            )
            || c.path
                .iter()
                .rev()
                .any(|&a| c.kind(a) == NodeKind::Initializer)
            || has_arrow_body(c, parent);
        if !exempt {
            return;
        }
    }
    // The operator parents (:274-281): binary, conditional, as, is.
    if matches!(
        container_parent(c, node, &owner_kids, pos),
        Some(
            NodeKind::BinaryExpression
                | NodeKind::Conditional
                | NodeKind::AsExpression
                | NodeKind::IsExpression
        )
    ) {
        return;
    }
    // `(p++).toString()` / `(++p).toString()` (:266-269).
    if access_selector_name(c, &owner_kids, pos).is_some()
        && (inner_has_kind(c, node, NodeKind::UnaryPostfix)
            || inner_has_kind(c, node, NodeKind::UnaryPrefix))
    {
        return;
    }
    // Directly-invoked parens (the FunctionExpressionInvocation face,
    // :277-279; the prefixed-identifier corner under-reports).
    if owner_kids.get(pos + 1).is_some_and(|&piece| {
        matches!(
            c.kind(piece),
            NodeKind::Arguments
                | NodeKind::NoArguments
                | NodeKind::TypeArguments
                | NodeKind::NoTypeArguments
        )
    }) {
        return;
    }
    // `({1, 2, 3}).forEach(print);` (:293-303): the parens cannot be
    // removed because then the curly brackets are not interpreted as a
    // set-or-map literal.
    if statement_owned && inner_has_kind(c, node, NodeKind::SetOrMapLiteral) {
        return;
    }
    report(c, node);
}

fn report(c: &mut Check<'_>, node: u32) {
    let range = c.utf16_span(node);
    c.report(range, &[]);
}

/// Whether the enclosing variable declaration carries a written record
/// type: the nearest preceding [`NodeKind::RecordType`] sibling of the
/// declaration row in the owner slice.
fn variable_declares_record_type(c: &Check<'_>) -> bool {
    let Some(&vars) = c
        .path
        .iter()
        .rev()
        .find(|&&a| c.kind(a) == NodeKind::VariablesDeclaration)
    else {
        return false;
    };
    let Some(owner) = c.unit.ast.parent(vars) else {
        return false;
    };
    let kids = c.collect_direct(owner);
    let Some(pos) = kids.iter().position(|&kid| kid == vars) else {
        return false;
    };
    kids[..pos]
        .iter()
        .rev()
        .any(|&kid| c.kind(kid) == NodeKind::RecordType)
}

/// Whether the parenthesized argument sits at a formal whose same-file
/// written type is a record type — the lexical stand-in for the
/// official's `correspondingParameter.type is RecordType` (:75-86).
/// The callee resolves as a bare name to exactly one same-file
/// top-level function; a receiver run before the callee head (member
/// and prefixed faces) or a computed callee leaves the correspondence
/// unresolved and the caller keeps its report posture.
fn argument_parameter_declares_record_type(c: &Check<'_>, paren: u32, args: u32) -> bool {
    // The send's owner slice: the argument list at `pos`, the bare
    // callee head before the type-argument wrappers.
    let Some(owner) = c.unit.ast.parent(args) else {
        return false;
    };
    let kids = c.collect_direct(owner);
    let Some(pos) = kids.iter().position(|&kid| kid == args) else {
        return false;
    };
    let Some(mut head) = pos.checked_sub(1) else {
        return false;
    };
    loop {
        match c.kind(kids[head]) {
            NodeKind::TypeArguments | NodeKind::NoTypeArguments => {
                let Some(prev) = head.checked_sub(1) else {
                    return false;
                };
                head = prev;
            }
            NodeKind::Identifier => break,
            _ => return false,
        }
    }
    // The bare-callee face only: pieces of the statement's own run
    // before the head are a receiver (`obj.g(...)`).
    if !slice::statement_run(c, &kids[..head]).is_empty() {
        return false;
    }
    let name = c.lexeme(c.head_token(kids[head]));
    let Some(function) = same_file_top_level_function(c, name) else {
        return false;
    };
    let rows = slice::parameter_assignments::parameter_rows(c, function);
    // The argument's own group over the argument list's children: a
    // named group matches its formal by label, a positional group by
    // its index over the positional groups.
    let arg_kids = c.collect_direct(args);
    let Some(arg_pos) = arg_kids.iter().position(|&kid| kid == paren) else {
        return false;
    };
    let groups = slice::arguments(c, &arg_kids);
    let Some(group) = groups.iter().find(|range| range.contains(&arg_pos)) else {
        return false;
    };
    let named = arg_kids[group.start..group.end]
        .iter()
        .any(|&piece| c.kind(piece) == NodeKind::NamedArgument);
    if named {
        let Some(name_pos) = slice::named_arg_name(c, &arg_kids, group.end - 1) else {
            return false;
        };
        let label = c.lexeme(c.head_token(arg_kids[name_pos]));
        rows.iter().any(|&(row, _)| {
            slice::parameter_assignments::parameter_name(c, row) == Some(label)
                && written_record_type(c, row)
        })
    } else {
        let index = groups
            .iter()
            .take_while(|range| *range != group)
            .filter(|range| {
                !arg_kids[range.start..range.end]
                    .iter()
                    .any(|&piece| c.kind(piece) == NodeKind::NamedArgument)
            })
            .count();
        rows.get(index)
            .is_some_and(|&(row, _)| written_record_type(c, row))
    }
}

/// The one same-file top-level function carrying `name`, `None` when
/// the name is declared zero or several times at the top level (the
/// library-unique spelling is what makes the correspondence exact).
fn same_file_top_level_function(c: &Check<'_>, name: &str) -> Option<u32> {
    let root = c.path.first().copied()?;
    let ctx = dartq_parser::IdentifierContext::TopLevelFunctionDeclaration as u32;
    let mut functions = Vec::new();
    for top in c.collect_direct(root) {
        for inner in c.collect_direct(top) {
            if c.kind(inner) == NodeKind::TopLevelMethod
                && c.collect_direct(inner).into_iter().any(|kid| {
                    c.kind(kid) == NodeKind::Identifier
                        && c.field(kid, PayloadField::Context) == Some(ctx)
                        && c.lexeme(c.head_token(kid)) == name
                })
            {
                functions.push(inner);
            }
        }
    }
    (functions.len() == 1).then(|| functions.remove(0))
}

/// Whether the formal's written type is a record type — the direct type
/// child (the parameter tiling keeps the annotation inside the row) or,
/// the variable-face tiling, a RecordType owner sibling immediately
/// before the row.
fn written_record_type(c: &Check<'_>, row: u32) -> bool {
    if c.collect_direct(row)
        .into_iter()
        .any(|kid| c.kind(kid) == NodeKind::RecordType)
    {
        return true;
    }
    let Some(parent) = c.unit.ast.parent(row) else {
        return false;
    };
    let kids = c.collect_direct(parent);
    let Some(pos) = kids.iter().position(|&kid| kid == row) else {
        return false;
    };
    pos > 0 && c.kind(kids[pos - 1]) == NodeKind::RecordType
}

/// Whether the paren's inner is a type literal — `(name<...>)`: an
/// identifier directly after `(`, a `<` right after it, and a matching
/// `>` closing before `)`. The name must bind to no same-file value
/// ([`super::same_file::declares_value`]): a same-file value spells a
/// generic tear-off (`(fn<int>)`), which the official keeps reporting
/// (a `FunctionReference`, not a `TypeLiteral`, :105-106).
fn type_literal_inner(c: &Check<'_>, node: u32) -> bool {
    let extent = c.unit.ast.token_range(node);
    if extent.len() < 5 {
        return false; // `( name < > )` minimum
    }
    let lparen = extent.start;
    if c.lexeme(lparen) != "(" {
        return false;
    }
    let name = lparen + 1;
    if c.lexeme(name + 1) != "<" {
        return false;
    }
    let rparen = extent.end - 1;
    if c.lexeme(rparen) != ")" || c.lexeme(rparen - 1) != ">" {
        return false;
    }
    !super::same_file::declares_value(c, node, c.lexeme(name))
}

/// Whether the paren's token range carries a null-aware CHAIN link
/// (`?.`, `?[` — the PropertyAccess/MethodInvocation/Index targets of
/// the upstream `containsNullAwareInvocationInChain` walk,
/// extensions.dart:130-143). `?..` is deliberately excluded: it opens
/// a cascade section, which the walk answers `false` for and the
/// cascade bail owns. The index link spells as two tokens (`?` `[`) —
/// the scanner emits no `?[` punctuation.
fn has_null_aware_token(c: &Check<'_>, node: u32) -> bool {
    let mut previous = "";
    c.unit.ast.token_range(node).any(|token| {
        let lexeme = c.lexeme(token);
        let hit = lexeme == "?." || (lexeme == "[" && previous == "?");
        previous = lexeme;
        hit
    })
}

/// Whether the paren's inner is a null-aware index — the `?` immediately
/// followed by `[` inside the token range (the scanner's two-token
/// spelling of `?[`).
fn has_null_aware_index_inner(c: &Check<'_>, node: u32) -> bool {
    let mut previous = "";
    c.unit.ast.token_range(node).any(|token| {
        let lexeme = c.lexeme(token);
        let hit = lexeme == "[" && previous == "?";
        previous = lexeme;
        hit
    })
}

/// Whether the paren is the operand of a prefix/postfix operator: the
/// childless operator row hugs the paren's span (a [`NodeKind::
/// UnaryPrefix`] ending at the paren's start, a [`NodeKind::UnaryPostfix`]
/// / [`NodeKind::NonNullAssert`] starting at its end). The row's position
/// in the owner slice is completion order — the operator seals last —
/// so the read is a span-adjacency scan, not a sibling offset.
fn operand_of_unary(c: &Check<'_>, node: u32, owner_kids: &[u32], pos: usize) -> bool {
    let span = c.utf16_span(node);
    owner_kids.iter().any(|&row| match c.kind(row) {
        NodeKind::UnaryPrefix => c.utf16_span(row).end == span.start,
        NodeKind::UnaryPostfix | NodeKind::NonNullAssert => c.utf16_span(row).start == span.end,
        _ => false,
    })
}

/// Whether the paren's inner is a constructor tear-off by token face —
/// `(C.new)` (the `.new` spelling) or `(name<...>.name)` (the
/// instantiated spelling): an identifier, optionally angle-nested type
/// arguments, a `.`, and a member name before `)`. The head name must
/// bind to no same-file value ([`super::same_file::declares_value`]) —
/// a same-file value spells a property access, which the official
/// keeps reading as a `PrefixedIdentifier` (the FEI keep's documented
/// corner).
fn constructor_reference_inner(c: &Check<'_>, node: u32) -> bool {
    let extent = c.unit.ast.token_range(node);
    if extent.len() < 5 {
        return false; // `( name . name )` minimum
    }
    let lparen = extent.start;
    if c.lexeme(lparen) != "(" {
        return false;
    }
    let name = lparen + 1;
    let rparen = extent.end - 1;
    if c.lexeme(rparen) != ")" {
        return false;
    }
    let member = rparen - 1;
    if c.lexeme(member - 1) != "." {
        return false;
    }
    if extent.len() == 5 {
        return c.lexeme(name + 1) == "." && c.lexeme(name + 2) == "new";
    }
    // `(name<...>.name)`: angle-nested type arguments between the head
    // identifier and the `.` (`>>`/`<<` step the depth by two).
    if c.lexeme(name + 1) != "<" {
        return false;
    }
    let mut depth: isize = 0;
    let mut i = name + 1;
    while i < member - 1 {
        match c.lexeme(i) {
            "<" => depth += 1,
            "<<" => depth += 2,
            ">" => depth -= 1,
            ">>" => depth -= 2,
            _ => {}
        }
        if depth <= 0 {
            break;
        }
        i += 1;
    }
    i + 1 == member - 1 && !super::same_file::declares_value(c, node, c.lexeme(name))
}

/// The name of the selector that follows the paren in the owner slice,
/// when a covering access marker confirms the paren is the target.
fn access_selector_name<'a>(c: &'a Check<'_>, owner_kids: &[u32], pos: usize) -> Option<&'a str> {
    let selector = *owner_kids.get(pos + 1)?;
    if c.kind(selector) != NodeKind::Identifier {
        return None;
    }
    owner_kids[pos + 2..]
        .iter()
        .copied()
        .find(|&piece| {
            c.kind(piece) == NodeKind::DotAccess
                && c.utf16_span(piece).start <= c.utf16_span(selector).start
                && c.utf16_span(piece).end >= c.utf16_span(selector).end
        })
        .map(|_| c.lexeme(c.head_token(selector)))
}

/// Whether the paren sits bare inside a constructor field initializer:
/// the [`NodeKind::Initializer`] ancestor is reached before any body or
/// argument boundary (the `isBareInConstructorFieldInitializer` walk,
/// :136-149).
fn bare_in_initializer(c: &Check<'_>) -> bool {
    for &ancestor in c.path.iter().rev().skip(1) {
        match c.kind(ancestor) {
            NodeKind::Initializer => return true,
            NodeKind::Block
            | NodeKind::BlockBody
            | NodeKind::ExpressionBody
            | NodeKind::Arguments
            | NodeKind::FormalParameters
            | NodeKind::Method
            | NodeKind::TopLevelMethod
            | NodeKind::Constructor
            | NodeKind::Factory
            | NodeKind::FunctionExpression
            | NodeKind::LocalFunctionDeclaration
            | NodeKind::NamedFunctionExpression => return false,
            _ => {}
        }
    }
    false
}

/// The `directlyContainsWhitespace` faces (:321-343): the container
/// inners — `as`, assignment, await, binary, `is`, switch, function
/// expression, `new`-creation — plus the `const`-literal shape and the
/// access-chain recursion (`-(new List(3)).length`-family): a
/// direct-child read over the paren's inner band answers the
/// container shapes; a `const` head word or an explicit `new`/`const`
/// keyword token inside covers the literal/creation arms.
fn whitespace_inner(c: &Check<'_>, node: u32) -> bool {
    let inner = c.collect_direct(node);
    for kid in &inner {
        match c.kind(*kid) {
            NodeKind::AsExpression
            | NodeKind::Assignment
            | NodeKind::AwaitExpression
            | NodeKind::BinaryExpression
            | NodeKind::FunctionExpression
            | NodeKind::NamedFunctionExpression
            | NodeKind::IsExpression
            | NodeKind::SwitchExpression
            | NodeKind::NewExpression => return true,
            _ => {
                if crate::rules::const_keyword(c, *kid).is_some() {
                    return true;
                }
            }
        }
    }
    // The const-literal head (the upstream `TypedLiteral.constKeyword`
    // / `InstanceCreationExpression.keyword` arms): the production
    // bridge seals the `const`-headed literal as a childless
    // [`NodeKind::ConstLiteral`] row. The head child only — a `const`
    // literal deeper in the inner (an argument) is not the chain
    // target, which the upstream target-chain recursion also answers
    // `false` for.
    if let Some(head) = inner
        .iter()
        .copied()
        .min_by_key(|&kid| c.utf16_span(kid).start)
    {
        if c.kind(head) == NodeKind::ConstLiteral {
            return true;
        }
    }
    // The `new`-keyword creation arm (`(new Foo())` tiles the creation
    // through a generic head, not a `NewExpression` row).
    c.unit
        .ast
        .token_range(node)
        .any(|token| c.lexeme(token) == "new")
}

/// Whether the declaration row carries an arrow body (the childless
/// `ExpressionBody` handle leaves the row as the expression's owner).
fn has_arrow_body(c: &Check<'_>, declaration: u32) -> bool {
    c.collect_direct(declaration)
        .into_iter()
        .any(|kid| c.kind(kid) == NodeKind::ExpressionBody)
}

#[cfg(test)]
mod p3769_pins {
    use crate::testsupport::{Fixture, one_rule, tok, utf16_at};

    /// `test_binaryExpressionInside_prefixExpression#0` —
    /// `var x = ~(1 | 2);` — the paren is the prefix `~`'s operand and
    /// the inner binary carries whitespace, so the official keeps the
    /// parens (the upstream parent is the `PrefixExpression`, never in
    /// the whitespace-exemption set). The production tiling seals the
    /// operator childless beside the paren under the initializer, and
    /// the operand-of-unary face reads the same keep.
    #[test]
    fn binary_inside_prefix_keeps() {
        let f = Fixture::parse("var x = ~(1 | 2);\n");
        assert!(
            f.run(&one_rule("unnecessary_parenthesis")).is_empty(),
            "{:?}",
            f.run(&one_rule("unnecessary_parenthesis"))
        );
    }

    /// `test_startsWithConstInside_prefixExpression#0` /
    /// `test_startsWithConstInside_targetOfMethodCall_prefixExpression#0`
    /// — the `const`-headed literal head makes the inner carry
    /// whitespace (`TypedLiteral.constKeyword`), so both `!(const
    /// [7]…)` shapes keep the parens.
    #[test]
    fn const_headed_inner_keeps() {
        let f = Fixture::parse("void f() {\n  !(const [7].contains(42));\n}\n");
        assert!(f.run(&one_rule("unnecessary_parenthesis")).is_empty());

        let f = Fixture::parse("void f() {\n  !(const [7]).contains(42);\n}\n");
        assert!(f.run(&one_rule("unnecessary_parenthesis")).is_empty());
    }

    /// `test_nullAwareIndexExpressionInside_conditionExpression#0` and
    /// `test_nullAwareIndexExpressionInside_mapLiteralKey#0` — the
    /// `?[` inner keeps the parens in the conditional-then and entry-key
    /// positions (:144-157, the second `?` would open a nested
    /// conditional).
    #[test]
    fn null_aware_index_in_then_and_key_keeps() {
        let f = Fixture::parse("void f(List<int>? a, bool b) {\n  var x = b ? (a?[7]) : 7;\n}\n");
        assert!(f.run(&one_rule("unnecessary_parenthesis")).is_empty());

        let f = Fixture::parse("void f(List<int>? a) {\n  var x = {(a?[7]): 7};\n}\n");
        assert!(f.run(&one_rule("unnecessary_parenthesis")).is_empty());
    }

    /// `test_nullAwareIndexExpressionInside_postfixExpression#0` —
    /// `(a?[0])!` — the `!` postfix parent keeps the parens (they stop
    /// the null-aware shorting). `test_nullAwareIndexExpressionInside_
    /// propertyAccess#0` — `(a?[0]).hashCode` — the `hashCode` accessor
    /// keeps them (:82-88).
    #[test]
    fn null_aware_index_under_bang_and_hash_code_keeps() {
        let f = Fixture::parse("void f(List<int?>? a) {\n  (a?[0])!;\n}\n");
        assert!(f.run(&one_rule("unnecessary_parenthesis")).is_empty());

        let f = Fixture::parse("void f(List<int>? a) {\n  (a?[0]).hashCode;\n}\n");
        assert!(f.run(&one_rule("unnecessary_parenthesis")).is_empty());
    }

    /// `test_constructorTearoffInside#0` — `INFO|…|4:3|7` — `(C.new)`
    /// reports (the flat send-group tiling answers through the token
    /// face); `test_constructorTearoffInstantiatedInside#0` —
    /// `INFO|…|3:3|18` — `(List<int>.filled)` reports. The
    /// explicit-type-arguments invocation keeps instead
    /// (`test_constructorTearoffInside_instantiatedThenCalled#0`).
    #[test]
    fn constructor_tearoff_inner_reports() {
        let f = Fixture::parse("class C {}\nvoid f() {\n  (C.new)();\n}\n");
        let lints = f.run(&one_rule("unnecessary_parenthesis"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        let lparen = tok(&f.source, &f.tokens, "(", 1);
        assert_eq!(
            lints[0].range,
            utf16_at(&f, lparen)..utf16_at(&f, lparen) + 7
        );

        let f = Fixture::parse("void f() {\n  (List<int>.filled)(3, 0);\n}\n");
        let lints = f.run(&one_rule("unnecessary_parenthesis"));
        assert_eq!(lints.len(), 1, "{lints:?}");
        let lparen = tok(&f.source, &f.tokens, "(", 1);
        assert_eq!(
            lints[0].range,
            utf16_at(&f, lparen)..utf16_at(&f, lparen) + 18
        );

        let f = Fixture::parse("void f() {\n  (List.filled)<int>(3, 0);\n}\n");
        assert!(f.run(&one_rule("unnecessary_parenthesis")).is_empty());
    }
}
