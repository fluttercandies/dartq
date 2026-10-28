use crate::rules::last_child;

use super::same_file::{BindingScope, binding};
use super::{PayloadField, Check, NodeKind, RuleSpec};

/// Official: `pkg/linter/lib/src/rules/avoid_double_and_int_checks.dart:28-59`
/// — an else-if chain whose branches test the same simple identifier —
/// `x is double` then `x is int` — with the identifier a formal
/// parameter or local variable reports at the else condition. The
/// written types must resolve to `double` / `int` (:43-46
/// `ifCondition.type.type`); both operands must be
/// `SimpleIdentifier`s with equal names (:41-42), and only the *if*
/// operand's element kind is checked (:47-49).
///
/// dartq faces: the written type resolves through the written import
/// surface — the plain spelling, a prefixed spelling whose prefix
/// imports `dart:core` with combinators admitting the name, and a
/// same-file typedef alias chain ending on the name; a parameterized
/// spelling never resolves (`double`/`int` are not generic). The
/// binding face is the shadow-correct same-file walk ([`binding`]), so
/// a same-file-typed local/parameter answers and a catch/for-in/pattern
/// bound one under-reports (the crate's documented posture).
pub(crate) static SPEC: RuleSpec = crate::spec!(
    "avoid_double_and_int_checks",
    &[NodeKind::IfStatement],
    check,
);

fn check(c: &mut Check<'_>) {
    let node = c.node();
    // `elseStatement is IfStatement` (:35-36).
    let Some(else_if) = last_child(c, node) else {
        return;
    };
    if c.kind(else_if) != NodeKind::IfStatement {
        return;
    }
    let Some((if_operand, _if_is)) = condition_is_check(c, node, "double") else {
        return;
    };
    let Some((else_operand, else_check)) = condition_is_check(c, else_if, "int") else {
        return;
    };
    // Equal simple names (:41-42); the if operand must bind to a local
    // or parameter (:47-49).
    if c.lexeme(c.head_token(if_operand)) != c.lexeme(c.head_token(else_operand)) {
        return;
    }
    let name = c.lexeme(c.head_token(if_operand));
    if !matches!(
        binding(c, if_operand, name),
        Some((BindingScope::Local | BindingScope::Parameter, _))
    ) {
        return;
    }
    // `reportAtNode(elseCondition)` (:50).
    let range = c.utf16_span(else_check);
    c.report(range, &[]);
}

/// The `wanted`-typed `is` check of the condition: the condition's
/// `IsExpression` child with a `Type` child resolving to `wanted`,
/// paired with its operand row. The flat tiling parents operand rows to
/// the statement owner (probe), so the operand is the window's
/// `Identifier` row spanning from just past the condition's open paren
/// to the `is` keyword — the official `SimpleIdentifier` face (a dotted
/// operand leaves its head row short of the keyword).
fn condition_is_check(c: &Check<'_>, if_stmt: u32, wanted: &str) -> Option<(u32, u32)> {
    let condition = c
        .collect_direct(if_stmt)
        .into_iter()
        .find(|&kid| c.kind(kid) == NodeKind::ParenthesizedCondition)?;
    let is_row = c
        .collect_direct(condition)
        .into_iter()
        .find(|&kid| c.kind(kid) == NodeKind::IsExpression)?;
    let type_row = c
        .collect_direct(is_row)
        .into_iter()
        .find(|&kid| c.kind(kid) == NodeKind::Type)?;
    if !written_type_resolves(c, type_row, wanted) {
        return None;
    }
    let type_head = c.head_token(type_row);
    let condition_start = c.unit.ast.token_range(condition).start;
    let is_kw = type_head.checked_sub(1)?;
    let operand = c.children(if_stmt).find(|&row| {
        c.kind(row) == NodeKind::Identifier
            && c.unit.ast.token_range(row)
                == core::ops::Range {
                    start: condition_start + 1,
                    end: is_kw,
                }
    })?;
    Some((operand, is_row))
}

/// Whether the written `is`-type row resolves to `wanted` (the official
/// `type.type == typeProvider.doubleType/intType`, :45-46): the plain
/// spelling, a prefixed spelling whose prefix imports `dart:core` and
/// whose combinators admit the name, or a same-file typedef alias
/// chain ending on the name. A parameterized spelling never resolves.
fn written_type_resolves(c: &Check<'_>, type_row: u32, wanted: &str) -> bool {
    let tokens = c.unit.ast.token_range(type_row);
    if tokens.len() == 1 {
        // A lone head spelling is the plain face only when no dot
        // precedes it (a prefixed annotation whose row covers the tail
        // must not read as the core name).
        if c.unit
            .ast
            .token_range(type_row)
            .start
            .checked_sub(1)
            .is_some_and(|prev| c.lexeme(prev) == ".")
        {
            return false;
        }
        let head = c.lexeme(tokens.start);
        return head == wanted || typedef_resolves(c, head, wanted, 0);
    }
    if tokens.len() == 3 && c.lexeme(tokens.start + 1) == "." {
        let (prefix, name) = (c.lexeme(tokens.start), c.lexeme(tokens.start + 2));
        return name == wanted && prefix_admits(c, prefix, wanted);
    }
    false
}

/// The same-file typedef alias chain (`typedef D = double;`): each hop
/// reads the aliased type's head name off the `Typedef` row (the
/// payload word 1 is the `=` token); bounded depth guards cycles.
fn typedef_resolves(c: &Check<'_>, name: &str, wanted: &str, depth: usize) -> bool {
    if depth > 8 {
        return false;
    }
    let Some(alias) = same_file_typedef_alias(c, name) else {
        return false;
    };
    alias == wanted || typedef_resolves(c, alias, wanted, depth + 1)
}

/// The aliased type's head name of the same-file `typedef name = …`.
fn same_file_typedef_alias<'a>(c: &'a Check<'_>, name: &str) -> Option<&'a str> {
    let root = c.path.first().copied()?;
    for top in c.collect_direct(root) {
        for inner in c.collect_direct(top) {
            if c.kind(inner) != NodeKind::Typedef {
                continue;
            }
            let Some(name_row) = c
                .collect_direct(inner)
                .into_iter()
                .find(|&kid| c.kind(kid) == NodeKind::Identifier)
            else {
                continue;
            };
            if c.lexeme(c.head_token(name_row)) != name {
                continue;
            }
            let eq = c
                .field(inner, PayloadField::Equals)
                .unwrap_or(crate::rules::NONE_TOKEN);
            if eq == crate::rules::NONE_TOKEN {
                continue;
            }
            let type_row = c
                .collect_direct(inner)
                .into_iter()
                .find(|&kid| c.kind(kid) == NodeKind::Type && c.head_token(kid) > eq)?;
            return Some(c.lexeme(c.head_token(type_row)));
        }
    }
    None
}

/// Whether the import prefixed `prefix` imports `dart:core` with
/// combinators admitting `name`.
fn prefix_admits(c: &Check<'_>, prefix: &str, name: &str) -> bool {
    let Some(root) = c.path.first().copied() else {
        return false;
    };
    for top in c.collect_direct(root) {
        for inner in c.collect_direct(top) {
            if c.kind(inner) != NodeKind::Import {
                continue;
            }
            // The `AsKeyword` word carries the prefix.
            let prefix_token = c
                .field(inner, PayloadField::AsKeyword)
                .unwrap_or(crate::rules::NONE_TOKEN);
            if prefix_token == crate::rules::NONE_TOKEN || c.lexeme(prefix_token) != prefix {
                continue;
            }
            let uri = c
                .collect_direct(inner)
                .into_iter()
                .find(|&kid| c.kind(kid) == NodeKind::StringLiteral);
            let Some(uri) = uri else {
                continue;
            };
            let span = c.byte_span(uri);
            let text =
                c.unit.source[span.start..span.end].trim_matches(|q| q == '\'' || q == '"');
            if text != "dart:core" {
                continue;
            }
            // The combinator face: an authoritative `show` must list
            // the name; a `hide` of the name excludes it.
            let mut shown_any = false;
            let mut shown = false;
            let mut hidden = false;
            for kid in c.collect_direct(inner) {
                match c.kind(kid) {
                    NodeKind::Show => {
                        shown_any = true;
                        shown = shown
                            || c.collect_direct(kid).into_iter().any(|entry| {
                                c.kind(entry) == NodeKind::Identifier
                                    && c.lexeme(c.head_token(entry)) == name
                            });
                    }
                    NodeKind::Hide => {
                        hidden = hidden
                            || c.collect_direct(kid).into_iter().any(|entry| {
                                c.kind(entry) == NodeKind::Identifier
                                    && c.lexeme(c.head_token(entry)) == name
                            });
                    }
                    _ => {}
                }
            }
            return (shown || !shown_any) && !hidden;
        }
    }
    false
}
