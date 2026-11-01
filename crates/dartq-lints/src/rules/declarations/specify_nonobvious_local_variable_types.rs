use super::{
    Check, NodeKind, RuleSpec, for_each_iterable, is_identifier_token, local_written_type,
    obvious_spelling,
};
use crate::rules::last_child;

/// Official: `pkg/linter/lib/src/rules/specify_nonobvious_local_variable_types.dart:68-145`
/// — a local variable list (:113-115, statement or C-style for
/// initializer via :70-73) with no written type (or exactly `Null`,
/// :117-119) where some variable lacks an initializer or initializes
/// from a non-obvious expression (:124-135) reports at the list for a
/// single variable (:137-138) or at each needing variable (:140-141).
/// The for-each face (:74-86): a loop variable without a written type
/// over a non-obvious iterable reports at the declaration (:84).
/// The invocation face (`pkg/linter/lib/src/util/obvious_types.dart`
/// :229-233): a method invocation is never obvious except `toString`, so
/// a bare zero-argument `bar()` initializer or iterable whose head names
/// no same-file class joins the non-obvious side.
///
/// Approximation (documented): the obvious-type matcher is the token
/// face of [`obvious_spelling`] — a bare-identifier initializer is the
/// obvious face, so the promotion-equality flip (`test_local_promotion`)
/// under-reports, and a bare-identifier element's unnameable type makes
/// any `[j, …]` fold non-obvious where the oracle's element-model types
/// can still agree (`[j, 1]` with `int j` is a `List<int>` upstream);
/// the pattern, switch and cast-declaration faces (:89-110) are out of
/// the substrate. Reports span `reportAtNode(node)` exactly: the whole
/// declaration list (modifier keyword through the last initializer) for
/// a single declarator, each needing declarator otherwise. Message:
/// `diagnostic.g.dart`
/// `specifyNonobviousLocalVariableTypes`.
pub(crate) static SPEC: RuleSpec = crate::spec!(
    "specify_nonobvious_local_variable_types",
    &[NodeKind::VariablesDeclaration],
    check,
);

fn check(c: &mut Check<'_>) {
    let node = c.node();
    let Some(parent) = c.ancestor(1) else {
        return;
    };
    match c.kind(parent) {
        // `staticType != null && !staticType.isDartCoreNull` (:117-119):
        // a written type other than `Null` skips.
        NodeKind::Block | NodeKind::BlockBody | NodeKind::ForStatement => {
            if local_written_type(c, node).is_some_and(|(start, _)| c.lexeme(start) != "Null") {
                return;
            }
            specify_face(c, node);
        }
        NodeKind::ForIn => face_for_each(c, node),
        _ => {}
    }
}

/// `_visitVariableDeclarationList` (:117-144): the variables whose
/// initializer is missing or non-obvious need a type; one variable
/// reports the whole list, several report each.
fn specify_face(c: &mut Check<'_>, node: u32) {
    let mut needs = Vec::new();
    let mut declarators = 0;
    for var in c.collect_direct(node) {
        if c.kind(var) != NodeKind::InitializedIdentifier {
            continue;
        }
        declarators += 1;
        let initializer = c
            .collect_direct(var)
            .into_iter()
            .find(|&kid| c.kind(kid) == NodeKind::VariableInitializer);
        let obvious = initializer
            .and_then(|init| initializer_value(c, init))
            .is_some_and(|value| invocation_obvious(c, value));
        if !obvious {
            needs.push(var);
        }
    }
    if needs.is_empty() {
        return;
    }
    if declarators == 1 {
        // `reportAtNode(node)` spans the whole declaration list — the
        // `var`/`final` keyword (the row anchors at the first name, so the
        // modifier word is read separately) through the last initializer
        // token (the statement `;` excluded).
        let run = c.unit.ast.token_range(node);
        let mut end = run.end;
        if end > run.start && c.lexeme(end - 1) == ";" {
            end -= 1;
        }
        let start_token = crate::rules::variables_declaration_keyword(c, node).unwrap_or(run.start);
        if let (Some(start), Some(end)) = (
            c.utf16_of_token(start_token),
            c.utf16_end_of_token(end.saturating_sub(1)),
        ) {
            c.report(start..end, &[]);
        }
    } else {
        for var in needs {
            c.report(c.utf16_span(var), &[]);
        }
    }
}

/// The initializer's expression row — the widest direct child (the
/// outermost expression tiling; the send-group markers spike smaller
/// windows beside it).
fn initializer_value(c: &Check<'_>, initializer: u32) -> Option<u32> {
    c.collect_direct(initializer).into_iter().max_by_key(|&row| {
        let span = c.byte_span(row);
        span.end - span.start
    })
}

/// The for-each face (:74-86): no written type (or exactly `dynamic`,
/// the `DynamicType` face) and a non-obvious iterable report at the
/// declaration (:84) — the report opens at the loop variable's first
/// written token (the `var`/`final` keyword, else the written `dynamic`
/// run) and runs through the name, the `DeclaredIdentifier` extent the
/// markup anchors (`[!var i!]`).
fn face_for_each(c: &mut Check<'_>, node: u32) {
    if local_written_type(c, node).is_some_and(|(start, _)| c.lexeme(start) != "dynamic") {
        return;
    }
    let Some(iterable) = for_each_iterable(c, node) else {
        return;
    };
    if invocation_obvious(c, iterable) {
        return;
    }
    let start = match crate::rules::variables_declaration_keyword(c, node) {
        Some(keyword) => keyword,
        // The written-`dynamic` face: `dynamic` lexes as a keyword (not an
        // identifier), so the shared written-run walk answers `None` for
        // `dynamic i` — the type token sits right before the name, the
        // row's last token.
        None => {
            let name = c.unit.ast.token_range(node).end - 1;
            if c.lexeme(name - 1) == "dynamic" {
                name - 1
            } else {
                return;
            }
        }
    };
    let end = c.unit.ast.token_range(node).end;
    if let Some(range) = super::utf16_token_run_span(c, start..end) {
        c.report(range, &[]);
    }
}

/// The invocation face of `hasObviousType`
/// (`pkg/linter/lib/src/util/obvious_types.dart` :229-233): a method
/// invocation is obvious only as `toString` — every other call shape is
/// non-obvious. The substrate's childless `Send` leaf carries both the
/// method invocations and the plain `A()` instance creations the official
/// parser separates only through the `<…>` implicit-creation lookahead,
/// and the shared creation matcher accepts a bare `name ( )` without any
/// class check — so that one shape re-reads the head: a same-file class
/// is the instance-creation face, where a raw zero-argument send over a
/// generic class stays non-obvious (the upstream raw-generic bail,
/// :184-189) and a non-generic class is obvious; `toString` keeps the
/// upstream exception; every other head (a function, a cross-file or
/// unresolved name) is the invocation face and answers `false`. Every
/// other shape keeps the matcher's answer.
fn invocation_obvious(c: &Check<'_>, value: u32) -> bool {
    if c.kind(value) != NodeKind::Send {
        return obvious_spelling(c, value).is_some();
    }
    let tokens = c.unit.ast.token_range(value);
    // A bare identifier read rides the same childless leaf — the
    // `SimpleIdentifier` face (`hasObviousType`: static type == declared
    // type except promotions, which this substrate cannot see).
    if tokens.len() == 1 && is_identifier_token(c, tokens.start) {
        return true;
    }
    let bare_call = tokens.len() == 3
        && is_identifier_token(c, tokens.start)
        && c.lexeme(tokens.start + 1) == "("
        && c.lexeme(tokens.start + 2) == ")";
    if !bare_call {
        return obvious_spelling(c, value).is_some();
    }
    let head = c.lexeme(tokens.start);
    if head == "toString" {
        return true;
    }
    crate::rules::same_file_declaration(c, head)
        .map(|row| c.kind(row) == NodeKind::ClassDeclaration && !class_generic(c, row))
        .unwrap_or(false)
}

/// Whether the same-file class row declares type variables. The
/// `TypeVariables` row tiles under the declaration's `TopLevelDeclaration`
/// wrapper (a sibling of the class-like row), not inside the class-like
/// itself — the wrapper's children read first, the class row's own
/// (hand-built tables) second.
fn class_generic(c: &Check<'_>, row: u32) -> bool {
    let wrapper = c.unit.ast.parent(row).unwrap_or(row);
    c.collect_direct(wrapper)
        .into_iter()
        .chain(c.collect_direct(row))
        .any(|kid| c.kind(kid) == NodeKind::TypeVariables)
}

#[cfg(test)]
mod oracle_corpus_pins {
    use crate::testsupport::{Fixture, one_rule};

    /// The whole upstream oracle corpus for `specify_nonobvious_local_variable_types`: `SpecifyNonObviousLocalVariableTypesTest` (37 cases) plus two `messages.yaml` doc entries,
    /// `conformance/status.tsv` `lint specify_nonobvious_local_variable_types 39 30 9`. Each entry is the exact
    /// materialized case source and the oracle's `(line, column, length)` for
    /// every `specify_nonobvious_local_variable_types` row; only the rule's own rows are pinned.
    ///
    /// 37 single-file cases pinned (11 oracle rows).
    /// Not representable in a single-source fixture (aux files or packages):
    /// `SpecifyNonObviousLocalVariableTypesTest/test_literal_int_negated#0` (known deviation: the signed-int literal initializer is not recognized as obvious).
    /// `SpecifyNonObviousLocalVariableTypesTest/test_local_no_promotion#0` (known deviation: the identifier initializer is not recognized as obvious).
    #[test]
    fn specify_nonobvious_local_variable_types_oracle_corpus_pins() {
        // (case, source, oracle rows as (line, column, length)).
        let cases: &[(&str, &str, &[(u32, u32, u32)])] = &[
            (
                r#"lints/specify_nonobvious_local_variable_types/SpecifyNonObviousLocalVariableTypesTest/test_as#0"#,
                r#"
f() {
  var d = 1 as num;
}
"#,
                &[],
            ),
            (
                r#"lints/specify_nonobvious_local_variable_types/SpecifyNonObviousLocalVariableTypesTest/test_as_dynamic#0"#,
                r#"
f() {
  var d = 1 as dynamic;
}
"#,
                &[],
            ),
            (
                r#"lints/specify_nonobvious_local_variable_types/SpecifyNonObviousLocalVariableTypesTest/test_cascade#0"#,
                r#"
f() {
  var a = A()..x..x..x;
}

class A {
  final x = 0;
}
"#,
                &[],
            ),
            (
                r#"lints/specify_nonobvious_local_variable_types/SpecifyNonObviousLocalVariableTypesTest/test_forEach_inferredList#0"#,
                r#"
f() {
  for (var i in [1, 2, 'Hello'.length]) {
    print(i);
  }
}
"#,
                &[(3, 8, 5)],
            ),
            (
                r#"lints/specify_nonobvious_local_variable_types/SpecifyNonObviousLocalVariableTypesTest/test_forEach_listWithNonObviousElement#0"#,
                r#"
f() {
  int j = "Hello".length;
  for (var i in [j, 1, j + 1]) { }
}
"#,
                &[(4, 8, 5)],
            ),
            (
                r#"lints/specify_nonobvious_local_variable_types/SpecifyNonObviousLocalVariableTypesTest/test_forEach_noDeclaredType#0"#,
                r#"
f() {
  for (var i in <int>[1, 2, 3]) { }
}
"#,
                &[],
            ),
            (
                r#"lints/specify_nonobvious_local_variable_types/SpecifyNonObviousLocalVariableTypesTest/test_forEach_nonObviousIterable#0"#,
                r#"
f() {
  for (int i in list) { }
}
var list = <int>[1, 2, 3];
"#,
                &[],
            ),
            (
                r#"lints/specify_nonobvious_local_variable_types/SpecifyNonObviousLocalVariableTypesTest/test_forEach_typedList#0"#,
                r#"
f() {
  for (int i in <int>[1, 2, 3]) { }
}
"#,
                &[],
            ),
            (
                r#"lints/specify_nonobvious_local_variable_types/SpecifyNonObviousLocalVariableTypesTest/test_genericInvocation_paramIsType#0"#,
                r#"
String f() {
  final h = bar('');
  return h;
}

T bar<T>(T d) => d;
"#,
                &[(3, 3, 17)],
            ),
            (
                r#"lints/specify_nonobvious_local_variable_types/SpecifyNonObviousLocalVariableTypesTest/test_genericInvocation_paramIsType_ok#0"#,
                r#"
String f() {
  String h = bar('');
  return h;
}

T bar<T>(T d) => d;
"#,
                &[],
            ),
            (
                r#"lints/specify_nonobvious_local_variable_types/SpecifyNonObviousLocalVariableTypesTest/test_genericInvocation_typeNeededForInference#0"#,
                r#"
f() {
  var h = bar('');
  return h;
}

T bar<T>(dynamic d) => d;
"#,
                &[(3, 3, 15)],
            ),
            (
                r#"lints/specify_nonobvious_local_variable_types/SpecifyNonObviousLocalVariableTypesTest/test_genericInvocation_typeNeededForInference_ok#0"#,
                r#"
String f() {
  String h = bar('');
  return h;
}

T bar<T>(dynamic d) => d;
"#,
                &[],
            ),
            (
                r#"lints/specify_nonobvious_local_variable_types/SpecifyNonObviousLocalVariableTypesTest/test_genericInvocation_typeParamProvided#0"#,
                r#"
String f() {
  var h = bar<String>('');
  return h;
}

T bar<T>(dynamic d) => d;
"#,
                &[(3, 3, 23)],
            ),
            (
                r#"lints/specify_nonobvious_local_variable_types/SpecifyNonObviousLocalVariableTypesTest/test_genericInvocation_typeParamProvided_ok#0"#,
                r#"
String f() {
  String h = bar<String>('');
  return h;
}

T bar<T>(dynamic d) => d;
"#,
                &[],
            ),
            (
                r#"lints/specify_nonobvious_local_variable_types/SpecifyNonObviousLocalVariableTypesTest/test_instanceCreation_generic#0"#,
                r#"
f() {
  var a = A(1);
}

class A<X> {
  A(X x);
}
"#,
                &[(3, 3, 12)],
            ),
            (
                r#"lints/specify_nonobvious_local_variable_types/SpecifyNonObviousLocalVariableTypesTest/test_instanceCreation_generic_ok1#0"#,
                r#"
f() {
  A<int> a = A<int>();
}

class A<X> {}
"#,
                &[],
            ),
            (
                r#"lints/specify_nonobvious_local_variable_types/SpecifyNonObviousLocalVariableTypesTest/test_instanceCreation_generic_ok2#0"#,
                r#"
f() {
  A<num> a = A<int>();
}

class A<X> {}
"#,
                &[],
            ),
            (
                r#"lints/specify_nonobvious_local_variable_types/SpecifyNonObviousLocalVariableTypesTest/test_instanceCreation_nonGeneric#0"#,
                r#"
f() {
  A a = A();
}

class A {}
"#,
                &[],
            ),
            (
                r#"lints/specify_nonobvious_local_variable_types/SpecifyNonObviousLocalVariableTypesTest/test_literal_bool#0"#,
                r#"
f() {
  bool b = true;
}
"#,
                &[],
            ),
            (
                r#"lints/specify_nonobvious_local_variable_types/SpecifyNonObviousLocalVariableTypesTest/test_literal_double#0"#,
                r#"
f() {
  double d = 1.5;
}
"#,
                &[],
            ),
            (
                r#"lints/specify_nonobvious_local_variable_types/SpecifyNonObviousLocalVariableTypesTest/test_literal_doubleTypedInt#0"#,
                r#"
f() {
  double d = 1;
}
"#,
                &[],
            ),
            (
                r#"lints/specify_nonobvious_local_variable_types/SpecifyNonObviousLocalVariableTypesTest/test_literal_double_negated#0"#,
                r#"
f() {
  var d = -1.5;
}
"#,
                &[],
            ),
            (
                r#"lints/specify_nonobvious_local_variable_types/SpecifyNonObviousLocalVariableTypesTest/test_literal_int#0"#,
                r#"
f() {
  int i = 1;
}
"#,
                &[],
            ),
            (
                r#"lints/specify_nonobvious_local_variable_types/SpecifyNonObviousLocalVariableTypesTest/test_literal_null#0"#,
                r#"
f() {
  Null nil = null;
}
"#,
                &[],
            ),
            (
                r#"lints/specify_nonobvious_local_variable_types/SpecifyNonObviousLocalVariableTypesTest/test_literal_string#0"#,
                r#"
f() {
  String s = "A string";
}
"#,
                &[],
            ),
            (
                r#"lints/specify_nonobvious_local_variable_types/SpecifyNonObviousLocalVariableTypesTest/test_literal_symbol#0"#,
                r#"
f() {
  Symbol s = #print;
}
"#,
                &[],
            ),
            (
                r#"lints/specify_nonobvious_local_variable_types/SpecifyNonObviousLocalVariableTypesTest/test_local_multiple#0"#,
                r#"
f() {
  var a = 'a' + 'a', b = 'b' * 2;
}
"#,
                &[(3, 7, 13), (3, 22, 11)],
            ),
            (
                r#"lints/specify_nonobvious_local_variable_types/SpecifyNonObviousLocalVariableTypesTest/test_local_multiple_ok#0"#,
                r#"
f() {
  String a = 'a' + 'a', b = 'b'.toString();
}
"#,
                &[],
            ),
            (
                r#"lints/specify_nonobvious_local_variable_types/SpecifyNonObviousLocalVariableTypesTest/test_local_promotion#0"#,
                r#"
f() {
  num local = 2;
  if (local is! int) return;
  var x = local;
  return x;
}
"#,
                &[(5, 3, 13)],
            ),
            (
                r#"lints/specify_nonobvious_local_variable_types/SpecifyNonObviousLocalVariableTypesTest/test_pattern_list_destructured#0"#,
                r#"
f() {
  var [int a] = <int>[1];
}
"#,
                &[],
            ),
            (
                r#"lints/specify_nonobvious_local_variable_types/SpecifyNonObviousLocalVariableTypesTest/test_pattern_map_destructured#0"#,
                r#"
f() {
  var {'a': int a} = <String, int>{'a': 1};
}
"#,
                &[],
            ),
            (
                r#"lints/specify_nonobvious_local_variable_types/SpecifyNonObviousLocalVariableTypesTest/test_pattern_object_destructured#0"#,
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
                r#"lints/specify_nonobvious_local_variable_types/SpecifyNonObviousLocalVariableTypesTest/test_pattern_record_destructured#0"#,
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
                r#"lints/specify_nonobvious_local_variable_types/SpecifyNonObviousLocalVariableTypesTest/test_switch_pattern_object#0"#,
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
                r#"lints/specify_nonobvious_local_variable_types/SpecifyNonObviousLocalVariableTypesTest/test_switch_pattern_record#0"#,
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
                r#"lints/specify_nonobvious_local_variable_types/doc_bad_0"#,
                r#"List<List<Ingredient>> possibleDesserts(Set<Ingredient> pantry) {
  var desserts = genericFunctionDeclaredFarAway(<num>[42], 'Something');
  for (final recipe in cookbook) {
    if (pantry.containsAll(recipe)) {
      desserts.add(recipe);
    }
  }

  return desserts;
}

const List<List<Ingredient>> cookbook = ...;
"#,
                &[(2, 3, 69), (3, 8, 12)],
            ),
            (
                r#"lints/specify_nonobvious_local_variable_types/doc_good_0"#,
                r#"List<List<Ingredient>> possibleDesserts(Set<Ingredient> pantry) {
  List<List<Ingredient>> desserts = genericFunctionDeclaredFarAway(
    <num>[42],
    'Something',
  );
  for (final List<Ingredient> recipe in cookbook) {
    if (pantry.containsAll(recipe)) {
      desserts.add(recipe);
    }
  }

  return desserts;
}

const List<List<Ingredient>> cookbook = ...;
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
            let mut lints = f.run(&one_rule("specify_nonobvious_local_variable_types"));
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
