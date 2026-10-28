use super::{Check, NodeKind, RuleSpec, angle_arguments, expanded_lexemes, is_identifier_token, local_written_type, type_arguments_open};
use crate::rules::last_child;

/// Official: `pkg/linter/lib/src/rules/omit_local_variable_types.dart:43-97`
/// — a local variable declaration (statement or C-style for
/// initializer, :38-40/:46-49) with a written type that every
/// initializer's static type equals reports at the type (:95). The
/// exact-type equality (`initializer.staticType != staticType`,
/// :81-84) folds to the literal and creation faces: `int`/`double`/
/// `bool`/`String` literals (the negated `-1`/`-1.0`/`!true` run),
/// collection literals with explicit type arguments equal to the
/// written ones, and constructor references whose head names the
/// written type. `dynamic`/`Null` declared types skip (:75-79), a
/// nullable written type never matches a non-null literal (:81-84),
/// the coerced-int face bails (`double d = 1`, :86-89), and a bare
/// collection literal (context-inferred, :81-84 resolved) and every
/// call/cast face under-report. The for-each loop-variable face
/// (:50-65) matches a collection-literal iterable whose explicit first
/// type argument is the written type; a variable or call iterable
/// under-reports. Message: `diagnostic.g.dart`
/// `omitLocalVariableTypes`.
pub(crate) static SPEC: RuleSpec = crate::spec!(
    "omit_local_variable_types",
    &[NodeKind::VariablesDeclaration],
    check,
);

fn check(c: &mut Check<'_>) {
    let node = c.node();
    let Some(parent) = c.ancestor(1) else {
        return;
    };
    match c.kind(parent) {
        // The statement and the C-style for initializer
        // (`visitVariableDeclarationStatement` +
        // `ForPartsWithDeclarations`); field rows (`Fields` /
        // `TopLevelFields`) are the official's unvisited faces.
        NodeKind::Block | NodeKind::BlockBody | NodeKind::ForStatement => {
            face_declaration(c, node);
        }
        // The for-each loop variable (`ForEachPartsWithDeclaration`).
        NodeKind::ForIn => face_for_each(c, node),
        _ => {}
    }
}

fn face_declaration(c: &mut Check<'_>, node: u32) {
    // `node.type?.type` (:74-79): written, not dynamic, not Null — and
    // the literal/creation faces only match non-nullable declared
    // types (:81-84), so a `?` tail skips.
    let Some((start, end)) = local_written_type(c, node) else {
        return;
    };
    if c.lexeme(end - 1) == "?" {
        return;
    }
    let head = c.lexeme(start);
    if matches!(head, "dynamic" | "Null" | "void") {
        return;
    }
    // Every declarator needs an initializer of the declared type
    // (:80-94).
    for var in c.collect_direct(node) {
        if c.kind(var) != NodeKind::InitializedIdentifier {
            continue;
        }
        let Some(initializer) = c
            .collect_direct(var)
            .into_iter()
            .find(|&kid| c.kind(kid) == NodeKind::VariableInitializer)
        else {
            return;
        };
        let Some(value) = last_child(c, initializer) else {
            return;
        };
        if !initializer_matches(c, start, end, head, value) {
            return;
        }
    }
    let (Some(start16), Some(end16)) = (c.utf16_of_token(start), c.utf16_of_token(end)) else {
        return;
    };
    c.report(start16..end16, &[]);
}

/// `initializer.staticType == staticType` (:81-84) — the literal and
/// creation faces over the value's token run.
fn initializer_matches(c: &Check<'_>, start: u32, end: u32, head: &str, value: u32) -> bool {
    let tokens = c.unit.ast.token_range(value);
    if tokens.start >= tokens.end {
        return false;
    }
    match c.kind(value) {
        NodeKind::LiteralInt => head == "int",
        NodeKind::LiteralDouble => head == "double",
        NodeKind::LiteralBool => head == "bool",
        NodeKind::StringLiteral => head == "String",
        // `-1` / `-1.0` / `!true` — the operand's literal face (the
        // coerced `double d = -1` quirk under-reports).
        NodeKind::UnaryPrefix => {
            let operand = last_child(c, value);
            match c.lexeme(c.head_token(value)) {
                "-" => match operand.map(|row| c.kind(row)) {
                    Some(NodeKind::LiteralInt) => head == "int",
                    Some(NodeKind::LiteralDouble) => head == "double",
                    _ => false,
                },
                "!" => {
                    head == "bool"
                        && operand.is_some_and(|row| c.kind(row) == NodeKind::LiteralBool)
                }
                _ => false,
            }
        }
        NodeKind::ListLiteral | NodeKind::SetOrMapLiteral => {
            collection_matches(c, start, end, head, tokens, value)
        }
        NodeKind::Send
        | NodeKind::NewExpression
        | NodeKind::ImplicitCreation
        | NodeKind::ConstExpression
        | NodeKind::ConstLiteral => creation_matches(c, start, end, head, tokens),
        _ => false,
    }
}

/// `List<int> l = <int>[1]` / `Set<int> s = <int>{}` /
/// `Map<K, V> m = <K, V>{}`: the written head names the collection and
/// the literal's explicit type arguments are the written ones (a bare
/// literal infers from context upstream and under-reports here).
fn collection_matches(
    c: &Check<'_>,
    start: u32,
    end: u32,
    head: &str,
    tokens: core::ops::Range<u32>,
    literal: u32,
) -> bool {
    if !(head == "List" || head == "Set" || head == "Map") {
        return false;
    }
    let Some(open) = type_arguments_open(c, literal) else {
        return false;
    };
    let Some(written) = written_arguments(c, start, end) else {
        return false;
    };
    let Some(arguments) = angle_arguments(c, open, tokens.end) else {
        return false;
    };
    written == arguments
}

/// `Foo f = Foo()` / `Foo f = new Foo()` / `Foo f = Foo.named()` /
/// `Foo<int> f = Foo()`: the creation head names the written type and
/// explicit type arguments are the written ones (an argument-less
/// creation infers the written arguments from context). Any call,
/// cast or tear-off face under-reports.
fn creation_matches(
    c: &Check<'_>,
    start: u32,
    end: u32,
    head: &str,
    tokens: core::ops::Range<u32>,
) -> bool {
    let mut cursor = tokens.start;
    if matches!(c.lexeme(cursor), "new" | "const") {
        cursor += 1;
    }
    if !is_identifier_token(c, cursor) || c.lexeme(cursor) != head {
        return false;
    }
    cursor += 1;
    while c.lexeme(cursor) == "." {
        if !is_identifier_token(c, cursor + 1) {
            return false;
        }
        cursor += 2;
    }
    let mut arguments = None;
    if c.lexeme(cursor) == "<" {
        let Some(args) = angle_arguments(c, cursor, tokens.end) else {
            return false;
        };
        arguments = Some(args);
        // Advance past the `<…>` run: the closer token (`>`, or the
        // last `>` piece of a `>>`/`>>>`) is `args`-deep — walk to the
        // token after the balanced close.
        let mut depth = 0u32;
        while cursor < tokens.end {
            match c.lexeme(cursor) {
                "<" => depth += 1,
                close @ (">" | ">>" | ">>>") => {
                    depth = depth.saturating_sub(close.len() as u32);
                    if depth == 0 {
                        cursor += 1;
                        break;
                    }
                }
                _ => {}
            }
            cursor += 1;
        }
    }
    // The argument group is exactly `()`.
    if tokens.end != cursor + 2 || c.lexeme(cursor) != "(" || c.lexeme(cursor + 1) != ")" {
        return false;
    }
    match arguments {
        Some(arguments) => match written_arguments(c, start, end) {
            Some(written) => written == arguments,
            None => false,
        },
        None => true,
    }
}

/// The written type's `<…>` argument lexemes (`List<int>` of
/// `List<List<int>>` — `>`-family closers split into `>` pieces), or
/// `None` for a raw spelling.
fn written_arguments<'a>(c: &'a Check<'a>, start: u32, end: u32) -> Option<Vec<&'a str>> {
    let lexemes = expanded_lexemes(c, start + 1..end);
    if lexemes.len() < 2 || lexemes.first() != Some(&"<") || lexemes.last() != Some(&">") {
        return None;
    }
    Some(lexemes[1..lexemes.len() - 1].to_vec())
}

/// `ForEachPartsWithDeclaration` (:50-65): the loop variable's written
/// type must equal the iterable's element type — the
/// collection-literal face with explicit type arguments.
fn face_for_each(c: &mut Check<'_>, node: u32) {
    let Some((start, end)) = local_written_type(c, node) else {
        return;
    };
    if c.lexeme(end - 1) == "?" {
        return;
    }
    let head = c.lexeme(start);
    if matches!(head, "dynamic" | "Null" | "void") {
        return;
    }
    // The iterable is the expression row before the parts child.
    let Some(parent) = c.ancestor(1) else {
        return;
    };
    let kids = c.collect_direct(parent);
    let Some(pos) = kids
        .iter()
        .position(|&kid| c.kind(kid) == NodeKind::ForInParts)
    else {
        return;
    };
    let Some(&iterable) = pos.checked_sub(1).and_then(|i| kids.get(i)) else {
        return;
    };
    let tokens = c.unit.ast.token_range(iterable);
    if !matches!(
        c.kind(iterable),
        NodeKind::ListLiteral | NodeKind::SetOrMapLiteral
    ) {
        return;
    }
    let Some(open) = type_arguments_open(c, iterable) else {
        return;
    };
    let Some(args) = angle_arguments(c, open, tokens.end) else {
        return;
    };
    // The element type is the first type argument — the run up to the
    // first top-level `,` (nested `<…>` commas do not split).
    let mut depth = 0u32;
    let mut element_end = args.len();
    for (index, lexeme) in args.iter().enumerate() {
        match *lexeme {
            "<" => depth += 1,
            ">" => depth = depth.saturating_sub(1),
            "," if depth == 0 => {
                element_end = index;
                break;
            }
            _ => {}
        }
    }
    let (Some(start16), Some(end16)) = (c.utf16_of_token(start), c.utf16_of_token(end)) else {
        return;
    };
    if expanded_lexemes(c, start..end) == args[..element_end] {
        c.report(start16..end16, &[]);
    }
}
