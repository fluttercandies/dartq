use super::{Check, NodeKind, PayloadField, RuleSpec};
use crate::rules::{first_child, slice};

/// The source-head row of the paren's inner expression (the outermost
/// container row when the inner is one — its subtree starts at the
/// window start).
fn inner_head(c: &Check<'_>, node: u32) -> Option<NodeKind> {
    first_child(c, node).map(|head| c.kind(head))
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
/// null-aware-index-in-then-arm bail (:144-157) is not read.
/// Message: `diagnostic.g.dart` `unnecessaryParenthesis`.
pub(crate) static SPEC: RuleSpec = crate::spec!(
    "unnecessary_parenthesis",
    &[NodeKind::ParenthesizedExpression],
    check,
);

#[expect(
    clippy::too_many_lines,
    reason = "one flat exemption ladder per parent shape; splitting it                   would scatter the official line citations"
)]
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
    // `[...(p as List)]` (:45) — the childless spread piece precedes.
    if pos > 0 && c.kind(owner_kids[pos - 1]) == NodeKind::Spread {
        return;
    }
    // `(int,) r = (3);` (:47-53) — the same-file written record type of
    // the variable keeps the parentheses.
    if matches!(
        parent_kind,
        NodeKind::VariableInitializer | NodeKind::InitializedIdentifier
    ) && variable_declares_record_type(c)
        && inner_head(c, node) != Some(NodeKind::RecordLiteral)
    {
        return;
    }
    // `g((3)); => g((int,) i) { }` / `g(i: (3)); =>
    // g({required (int,) i}) { }` (:75-86) — the same-file written
    // record type of the corresponding parameter keeps the parentheses.
    if parent_kind == NodeKind::Arguments
        && inner_head(c, node) != Some(NodeKind::RecordLiteral)
        && argument_parameter_declares_record_type(c, node, parent)
    {
        return;
    }
    let inner = c.collect_direct(node);
    let Some(&inner_first) = inner.first() else {
        return;
    };
    // Always-report set (:67-73): nested parens, single positional
    // argument, and the condition segments (the upstream
    // if/while/do/switch direct parents).
    if parent_kind == NodeKind::ParenthesizedExpression
        || parent_kind == NodeKind::ParenthesizedCondition
        || (parent_kind == NodeKind::Arguments && slice::arguments(c, &owner_kids).len() == 1)
    {
        report(c, node);
        return;
    }
    // `(foo ? bar : baz)` (:73).
    if inner_head(c, node) == Some(NodeKind::Conditional) {
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
    let null_aware_chain = chain_head(c, inner_first) && has_null_aware_token(c, node);
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
        // `(p)!` (:93-95) is inert: the `UnaryPostfix` row carries no
        // payload words, so an operator-word bail could never match and
        // this shape always fell through to the report (the pinned
        // behavior).
        report(c, node);
        return;
    }
    // `(Foo.new)` (:103-108) — the invocation-with-type-arguments
    // exemption is the documented corner.
    if inner_head(c, node) == Some(NodeKind::ConstructorReference) {
        report(c, node);
        return;
    }
    // `a..b = (c..d)` (:104-107): any cascade position keeps the parens.
    if c.kind(inner_first) == NodeKind::Cascade
        || c.path.iter().rev().any(|&a| c.kind(a) == NodeKind::Cascade)
    {
        return;
    }
    // The constructor-field-initializer `=>` guard (:119-131).
    if bare_in_initializer(c) && has_token_lexeme(c, node, "=>") {
        return;
    }
    // `foo = (a == b)` / `return (count != 0)` (:133-139).
    if inner_head(c, node) == Some(NodeKind::BinaryExpression) {
        let binary = first_child(c, node).expect("the head kind matched above");
        let op = c
            .field(binary, PayloadField::Operator)
            .map(|op| c.lexeme(op))
            .unwrap_or_default();
        if matches!(op, "==" | "!=")
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
    }
    // A statement-initial switch expression (:146-149).
    let statement_owned = owner_kids[pos + 1..].iter().any(|&sib| {
        c.kind(sib) == NodeKind::ExpressionStatement
            && c.utf16_span(sib).start <= c.utf16_span(node).start
            && c.utf16_span(sib).end >= c.utf16_span(node).end
    });
    if statement_owned && inner_head(c, node) == Some(NodeKind::SwitchExpression) {
        return;
    }
    // The whitespace-readability family (:151-166): keep the parens
    // unless an exemption parent answers.
    if whitespace_inner(c, first_child(c, node)) {
        let exempt = matches!(
            parent_kind,
            NodeKind::Assignment
                | NodeKind::RecordLiteral
                | NodeKind::ReturnStatement
                | NodeKind::YieldStatement
                | NodeKind::InitializedIdentifier
                | NodeKind::VariableInitializer
        ) || c
            .path
            .iter()
            .rev()
            .any(|&a| c.kind(a) == NodeKind::Initializer)
            || parent_kind == NodeKind::Arguments
            || has_arrow_body(c, parent);
        if !exempt {
            return;
        }
    }
    // The operator parents (:169-176).
    if matches!(
        parent_kind,
        NodeKind::BinaryExpression
            | NodeKind::Conditional
            | NodeKind::AsExpression
            | NodeKind::IsExpression
    ) {
        return;
    }
    // `(p++).toString()` / `(++p).toString()` (:188-192).
    if access_selector_name(c, &owner_kids, pos).is_some()
        && matches!(
            c.kind(inner_first),
            NodeKind::UnaryPostfix | NodeKind::UnaryPrefix
        )
    {
        return;
    }
    // Directly-invoked parens (the FunctionExpressionInvocation face,
    // :178-180; the prefixed-identifier corner under-reports).
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
    // `({1, 2, 3}).forEach(print);` (:195-197).
    if statement_owned && inner_head(c, node) == Some(NodeKind::SetOrMapLiteral) {
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

/// Whether `inner_first` can head an access chain (the
/// `containsNullAwareInvocationInChain` entry faces: a chain walks
/// PropertyAccess/MethodInvocation/Index targets only — a container
/// inner never qualifies, extensions.dart:211-224).
fn chain_head(c: &Check<'_>, inner_first: u32) -> bool {
    matches!(
        c.kind(inner_first),
        NodeKind::Identifier | NodeKind::ThisExpression | NodeKind::SuperExpression
    )
}

/// Whether the paren's token range carries a null-aware link token
/// (`?.`, `?..`, `?[` — the chain faces of the upstream walk).
fn has_null_aware_token(c: &Check<'_>, node: u32) -> bool {
    c.unit
        .ast
        .token_range(node)
        .any(|token| matches!(c.lexeme(token), "?." | "?.." | "?["))
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

/// Whether the node's token range carries `lexeme`.
fn has_token_lexeme(c: &Check<'_>, node: u32, lexeme: &str) -> bool {
    c.unit
        .ast
        .token_range(node)
        .any(|token| c.lexeme(token) == lexeme)
}

/// The `directlyContainsWhitespace` faces (:216-233): the container
/// inners plus the `new`-keyword and `const`-literal shapes.
fn whitespace_inner(c: &Check<'_>, head: Option<u32>) -> bool {
    let Some(head) = head else {
        return false;
    };
    match c.kind(head) {
        NodeKind::AsExpression
        | NodeKind::Assignment
        | NodeKind::AwaitExpression
        | NodeKind::BinaryExpression
        | NodeKind::IsExpression
        | NodeKind::SwitchExpression
        | NodeKind::NewExpression => true,
        _ => crate::rules::const_keyword(c, head).is_some(),
    }
}

/// Whether the declaration row carries an arrow body (the childless
/// `ExpressionBody` handle leaves the row as the expression's owner).
fn has_arrow_body(c: &Check<'_>, declaration: u32) -> bool {
    c.collect_direct(declaration)
        .into_iter()
        .any(|kid| c.kind(kid) == NodeKind::ExpressionBody)
}
