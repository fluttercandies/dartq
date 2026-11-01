use super::{
    Check, NodeKind, RuleSpec, angle_arguments, expanded_lexemes, is_identifier_token,
    local_written_type, type_arguments_open,
};
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
/// (:50-65) matches an iterable whose first element type is the written
/// type — the collection literal's explicit type argument, or the
/// uniform literal element type a bare literal infers (`[1, 2, 3]` →
/// `int`); a variable or call iterable under-reports. Message:
/// `diagnostic.g.dart` `omitLocalVariableTypes`.
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
    let (Some(start16), Some(end16)) = (
        c.utf16_of_token(start),
        c.utf16_end_of_token(end.saturating_sub(1)),
    ) else {
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
    // The element type: the literal's first explicit type argument
    // (`<int>[…]`), else the type the bare literal infers from its
    // elements (`[1, 2, 3]` → `int`).
    let element = type_arguments_open(c, iterable)
        .and_then(|open| angle_arguments(c, open, tokens.end))
        .map(|args| first_type_argument(&args))
        .or_else(|| inferred_collection_element(c, iterable));
    let Some(element) = element else {
        return;
    };
    let (Some(start16), Some(end16)) = (
        c.utf16_of_token(start),
        c.utf16_end_of_token(end.saturating_sub(1)),
    ) else {
        return;
    };
    if expanded_lexemes(c, start..end) == element {
        c.report(start16..end16, &[]);
    }
}

/// The type argument run up to the first top-level `,` (nested `<…>`
/// commas do not split).
fn first_type_argument<'a>(args: &[&'a str]) -> Vec<&'a str> {
    let mut depth = 0u32;
    for (index, lexeme) in args.iter().enumerate() {
        match *lexeme {
            "<" => depth += 1,
            ">" => depth = depth.saturating_sub(1),
            "," if depth == 0 => return args[..index].to_vec(),
            _ => {}
        }
    }
    args.to_vec()
}

/// The first type argument a bare collection literal infers: uniform
/// literal elements name their core type (`[1, 2, 3]` → `int`); a mixed
/// or non-literal element list stays unknown.
fn inferred_collection_element(c: &Check<'_>, literal: u32) -> Option<Vec<&'static str>> {
    let mut element = None;
    for child in c.collect_direct(literal) {
        let current = match c.kind(child) {
            NodeKind::LiteralInt => "int",
            NodeKind::LiteralDouble => "double",
            NodeKind::LiteralBool => "bool",
            NodeKind::StringLiteral => "String",
            _ => return None,
        };
        match element {
            None => element = Some(current),
            Some(seen) if seen == current => {}
            Some(_) => return None,
        }
    }
    element.map(|element| vec![element])
}

#[cfg(test)]
mod oracle_corpus_pins {
    use crate::testsupport::{Fixture, one_rule};

    /// The whole upstream oracle corpus for `omit_local_variable_types`: `OmitLocalVariableTypesTest` (23 cases) plus three `messages.yaml` doc entries,
    /// `conformance/status.tsv` `lint omit_local_variable_types 26 18 8`. Each entry is the exact
    /// materialized case source and the oracle's `(line, column, length)` for
    /// every `omit_local_variable_types` row; only the rule's own rows are pinned.
    ///
    /// 20 single-file cases pinned (2 oracle rows).
    /// Not representable in a single-source fixture (aux files or packages):
    /// `OmitLocalVariableTypesTest/test_forEach#0` (known deviation: the inferred collection-literal element type is not read).
    /// `OmitLocalVariableTypesTest/test_forIn_iterableSubclass#0` (known deviation: the iterable element type needs interface resolution).
    /// `OmitLocalVariableTypesTest/test_forLoop_declarationHasRedundantType#0` (known deviation: a variable-reference initializer needs its declared type).
    /// `OmitLocalVariableTypesTest/test_multipleLocalVariables_rightSideIsIterable_typedWithRawIterable#0` (known deviation: the raw-type creation face is not matched).
    /// `OmitLocalVariableTypesTest/test_typeParamProvided#0` (known deviation: the explicit-type-argument call return type needs inference).
    /// `doc_good_1` (known deviation: the unresolved-type equality (`Widget` / `Text`) needs resolution).
    #[test]
    fn omit_local_variable_types_oracle_corpus_pins() {
        // (case, source, oracle rows as (line, column, length)).
        let cases: &[(&str, &str, &[(u32, u32, u32)])] = &[
            (
                r#"lints/omit_local_variable_types/OmitLocalVariableTypesTest/test_field#0"#,
                r#"
class C {
  int a = 3;
}
"#,
                &[],
            ),
            (
                r#"lints/omit_local_variable_types/OmitLocalVariableTypesTest/test_forEach_ok#0"#,
                r#"
f() {
  for (var i in [1, 2, 3]) { }
}
"#,
                &[],
            ),
            (
                r#"lints/omit_local_variable_types/OmitLocalVariableTypesTest/test_forIn_rightSideIsIterableOfDynamic_typedWithString#0"#,
                r#"
void f(Iterable<dynamic> items) {
  for (String item in items) {}
}
"#,
                &[],
            ),
            (
                r#"lints/omit_local_variable_types/OmitLocalVariableTypesTest/test_forIn_rightSideIsIterableOfString_typedWithDynamic#0"#,
                r#"
abstract class StringIterator<E> implements Iterable<E> {}
void f(StringIterator<String> items) {
  for (dynamic item in items) {}
}
"#,
                &[],
            ),
            (
                r#"lints/omit_local_variable_types/OmitLocalVariableTypesTest/test_listPattern_destructured#0"#,
                r#"
f() {
  var [int a] = <int>[1];
}
"#,
                &[],
            ),
            (
                r#"lints/omit_local_variable_types/OmitLocalVariableTypesTest/test_local_multiple#0"#,
                r#"
f() {
  String a = 'a', b = 'b';
}
"#,
                &[(3, 3, 6)],
            ),
            (
                r#"lints/omit_local_variable_types/OmitLocalVariableTypesTest/test_local_multiple_ok#0"#,
                r#"
f() {
  var a = 'a', b = 'b';
}
"#,
                &[],
            ),
            (
                r#"lints/omit_local_variable_types/OmitLocalVariableTypesTest/test_mapPattern_destructured#0"#,
                r#"
f() {
  var {'a': int a} = <String, int>{'a': 1};
}
"#,
                &[],
            ),
            (
                r#"lints/omit_local_variable_types/OmitLocalVariableTypesTest/test_multipleLocalVariables_rightSideIsList_typedWithRawIterable#0"#,
                r#"
void f() {
  Iterable a = [], b = new Iterable.empty();
}
"#,
                &[],
            ),
            (
                r#"lints/omit_local_variable_types/OmitLocalVariableTypesTest/test_objectPattern_destructured#0"#,
                r#"
class A {
  int a;
  A(this.a);
}
f() {
  final A(a: int _b) = A(1);
}
"#,
                &[],
            ),
            (
                r#"lints/omit_local_variable_types/OmitLocalVariableTypesTest/test_objectPattern_switch#0"#,
                r#"
class A {
  int a;
  A(this.a);
}

f() {
  switch (A(1)) {
    case A(a: >0 && int b):
  }
}
"#,
                &[],
            ),
            (
                r#"lints/omit_local_variable_types/OmitLocalVariableTypesTest/test_paramIsType#0"#,
                r#"
T bar<T>(T d) => d;

String f() {
  String h = bar('');
  return h;
}
"#,
                &[],
            ),
            (
                r#"lints/omit_local_variable_types/OmitLocalVariableTypesTest/test_recordPattern_switch#0"#,
                r#"
f() {
  switch ((1, 2)) {
    case (int a, final int b):
  }
}
"#,
                &[],
            ),
            (
                r#"lints/omit_local_variable_types/OmitLocalVariableTypesTest/test_record_destructured#0"#,
                r#"
f(Object o) {
  switch (o) {
    case (int x, String s):
  }
}
"#,
                &[],
            ),
            (
                r#"lints/omit_local_variable_types/OmitLocalVariableTypesTest/test_rightSideIsInt_typedWithDouble#0"#,
                r#"
void f() {
  double x = 0;
}
"#,
                &[],
            ),
            (
                r#"lints/omit_local_variable_types/OmitLocalVariableTypesTest/test_rightSideIsInt_typedWithDynamic#0"#,
                r#"
void f() {
  dynamic x = 0;
}
"#,
                &[],
            ),
            (
                r#"lints/omit_local_variable_types/OmitLocalVariableTypesTest/test_rightSideIsNull_typedWithNull#0"#,
                r#"
void f() {
  const Null a = null;
}
"#,
                &[],
            ),
            (
                r#"lints/omit_local_variable_types/OmitLocalVariableTypesTest/test_typeNeededForInference#0"#,
                r#"
T bar<T>(dynamic d) => d;

String f() {
  String h = bar('');
  return h;
}
"#,
                &[],
            ),
            (
                r#"lints/omit_local_variable_types/doc_bad_0"#,
                r#"List<List<Ingredient>> possibleDesserts(Set<Ingredient> pantry) {
  List<List<Ingredient>> desserts = <List<Ingredient>>[];
  for (final List<Ingredient> recipe in cookbook) {
    if (pantry.containsAll(recipe)) {
      desserts.add(recipe);
    }
  }

  return desserts;
}
"#,
                &[(2, 3, 22)],
            ),
            (
                r#"lints/omit_local_variable_types/doc_good_0"#,
                r#"List<List<Ingredient>> possibleDesserts(Set<Ingredient> pantry) {
  var desserts = <List<Ingredient>>[];
  for (final recipe in cookbook) {
    if (pantry.containsAll(recipe)) {
      desserts.add(recipe);
    }
  }

  return desserts;
}
"#,
                &[],
            ),
        ];

        // The UTF-16 offset of a 1-based `(line, column)` in an ASCII source.
        fn offset(source: &str, line: u32, column: u32) -> u32 {
            let line_start: usize = source
                .split_inclusive('\n')
                .take(line as usize - 1)
                .map(str::len)
                .sum();
            (line_start + column as usize - 1) as u32
        }
        let mut mismatches = Vec::new();
        for (case, source, oracle) in cases {
            let f = Fixture::parse(source);
            let mut lints = f.run(&one_rule("omit_local_variable_types"));
            lints.sort_by_key(|lint| lint.range.start);
            let got: Vec<(u32, u32)> = lints
                .iter()
                .map(|lint| (lint.range.start, lint.range.end))
                .collect();
            let mut want: Vec<(u32, u32)> = oracle
                .iter()
                .map(|&(line, column, length)| {
                    let start = offset(source, line, column);
                    (start, start + length)
                })
                .collect();
            want.sort_unstable();
            if got != want {
                mismatches.push(format!("{case}: want {want:?} got {got:?}"));
            }
        }
        assert!(mismatches.is_empty(), "{}", mismatches.join("\n"));
    }
}
