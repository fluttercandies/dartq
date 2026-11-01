use super::{Check, NodeKind, PayloadField, RuleSpec};
use crate::rules::{NONE_TOKEN, first_child};

/// Official: `upstream Dart SDK source, pkg/linter/lib/src/rules/avoid_init_to_null.dart:59-88`
/// — a variable whose initializer is the `null` literal reports at the
/// declaration; const/final declarations are exempt (`!node.isConst &&
/// !node.isFinal`, :62-63), the declared type must be nullable
/// (`typeSystem.isNullable`, :41/:65 — `int x = null;` is a compile
/// error upstream), and the formal-parameter default-value faces
/// (:74-87) carry the same nullable gate.
///
/// The nullable gate reads the declaration's tokens
/// ([`super::declaration_written_type`] + the tail-`?`/`dynamic`
/// spell): a written non-nullable type bails, no written type infers
/// `dynamic` (nullable). A type-alias spelling of a nullable type
/// (`MyNull?` aside) under-reports. The super-formal sub-check
/// (`superConstructorParameter.defaultValueCode`, :78-84) reads the
/// same-file super constructor's parameter default — `?? 'null'` — and
/// stays silent when the supertype leaves the file (the official's
/// `is! FormalParameterElement` bail); a field formal without a written
/// type takes the declaring field's type (`this.x` element type,
/// :65/:87). The keyword gate reads the declaration's
/// `varFinalOrConst` token through
/// [`crate::rules::variables_declaration_keyword`] (`final`/`const`
/// exempt), and the parameter report spans the whole formal
/// (`reportAtNode(parameter)` — written type or `var` keyword
/// included). Message: `diagnostic.g.dart` `avoidInitToNull`.
pub(crate) static SPEC: RuleSpec = crate::spec!(
    "avoid_init_to_null",
    &[NodeKind::InitializedIdentifier, NodeKind::FormalParameter],
    check,
);

fn check(c: &mut Check<'_>) {
    match c.kind(c.node()) {
        NodeKind::InitializedIdentifier => check_variable(c),
        NodeKind::FormalParameter => check_parameter(c),
        _ => {}
    }
}

fn check_variable(c: &mut Check<'_>) {
    // The initializer is the `VariableInitializer` child; its operand is
    // the null literal.
    let initializer = c
        .collect_direct(c.node())
        .into_iter()
        .find(|&kid| c.kind(kid) == NodeKind::VariableInitializer);
    let Some(initializer) = initializer else {
        return;
    };
    let Some(value) = first_child(c, initializer) else {
        return;
    };
    if c.kind(value) != NodeKind::LiteralNull {
        return;
    }
    let Some(parent) = c.ancestor(1) else {
        return;
    };
    // `!node.isConst && !node.isFinal` (:62-63): the declaration
    // keyword (`final`/`const` exempt; `var` and late-`var` do not) —
    // the shared modifier read (payload word, head token, else the
    // bounded backward walk from the name).
    if c.kind(parent) == NodeKind::VariablesDeclaration
        && matches!(
            crate::rules::variables_declaration_keyword(c, parent).map(|keyword| c.lexeme(keyword)),
            Some("final" | "const")
        )
    {
        return;
    }
    // `isNullable(declaredElement.type)` (:41/:65): a written
    // non-nullable type is a compile error upstream — `int x = null;`
    // stays silent; no written type infers `dynamic` (nullable).
    if c.kind(parent) == NodeKind::VariablesDeclaration
        && let Some(run) = super::declaration_written_type(c, parent)
        && !super::written_run_nullable(c, run)
    {
        return;
    }
    let range = c.utf16_span(c.node());
    c.report(range, &[]);
}

fn check_parameter(c: &mut Check<'_>) {
    let node = c.node();
    // `{int x = null}` — the default value is delimited by the
    // `InitializerStart`/`InitializerEnd` words;
    // exactly the `null` token qualifies under either end convention.
    let Some(start) = c.field(node, PayloadField::InitializerStart) else {
        return;
    };
    if start == NONE_TOKEN {
        return;
    }
    let end = c.field(node, PayloadField::InitializerEnd).unwrap_or(start);
    if c.lexeme(start) != "null" || end.saturating_sub(start) > 1 {
        return;
    }
    let receiver = c
        .field(node, PayloadField::Receiver)
        .filter(|&token| token != NONE_TOKEN);
    match receiver.map(|token| c.lexeme(token)) {
        // `super.a = null` (:78-84): the *super constructor parameter's*
        // default code gates the report (`superConstructorParameter
        // .defaultValueCode ?? 'null'`) — the AST default written here is
        // irrelevant. A supertype outside this file is unknown, and the
        // official `is! FormalParameterElement` bail keeps it silent.
        Some("super") => {
            if super_formal_default_is_null(c, node) != Some(true) {
                return;
            }
        }
        receiver => {
            // `isNullable(declaredElement.type)` (:87): the parameter's
            // written type decides; a field formal (`this.a`) without one
            // takes the field's declared type; anything else infers
            // `dynamic` (nullable).
            let written = written_parameter_type(c, node).or_else(|| {
                (receiver == Some("this"))
                    .then(|| field_written_type(c, node))
                    .flatten()
            });
            if let Some(run) = written
                && !super::written_run_nullable(c, run)
            {
                return;
            }
        }
    }
    // `reportAtNode(parameter)` spans the whole formal — the written type
    // (or `var` keyword) and the `this`/`super` receiver included, ending
    // at the default clause's last token.
    let start = c
        .utf16_of_token(c.unit.ast.token_range(node).start)
        .unwrap_or_default();
    let end = c
        .utf16_end_of_token(end.saturating_sub(1))
        .unwrap_or(start);
    c.report(start..end, &[]);
}

/// The `(start, end)` token run of the formal's written type (`None` = no
/// written type; `{x = null}` infers `dynamic`).
fn written_parameter_type(c: &Check<'_>, node: u32) -> Option<(u32, u32)> {
    c.collect_direct(node)
        .into_iter()
        .find(|&kid| {
            matches!(
                c.kind(kid),
                NodeKind::Type | NodeKind::FunctionType | NodeKind::RecordType
            )
        })
        .map(|written| {
            let range = c.unit.ast.token_range(written);
            (range.start, range.end)
        })
}

/// The declared type run of the enclosing class's field named like the
/// field formal (`this.a = null` — the element's type is the field's).
fn field_written_type(c: &Check<'_>, param: u32) -> Option<(u32, u32)> {
    let name = c
        .field(param, PayloadField::Name)
        .filter(|&token| token != NONE_TOKEN)?;
    let name = c.lexeme(name);
    let class_row = *c.path.iter().rev().find(|&&row| {
        matches!(
            c.kind(row),
            NodeKind::ClassDeclaration
                | NodeKind::MixinDeclaration
                | NodeKind::EnumDeclaration
                | NodeKind::ExtensionTypeDeclaration
        )
    })?;
    for member in crate::rules::body_members(c, class_row) {
        if c.kind(member) != NodeKind::Fields {
            continue;
        }
        for decl in c.collect_direct(member) {
            if c.kind(decl) != NodeKind::VariablesDeclaration {
                continue;
            }
            let declares_name = c
                .collect_direct(decl)
                .into_iter()
                .filter(|&kid| c.kind(kid) == NodeKind::InitializedIdentifier)
                .any(|var| {
                    crate::rules::initialized_identifier_name(c, var)
                        .is_some_and(|row| c.lexeme(row) == name)
                });
            if declares_name {
                return super::declaration_written_type(c, decl);
            }
        }
    }
    None
}

/// Whether the same-file super constructor's parameter of the same name
/// defaults to the `null` literal (:78-84: `defaultValueCode ?? 'null'`).
/// `None` = the supertype (or that parameter) is not in this file.
fn super_formal_default_is_null(c: &Check<'_>, param: u32) -> Option<bool> {
    let name = c
        .field(param, PayloadField::Name)
        .filter(|&token| token != NONE_TOKEN)?;
    let name = c.lexeme(name);
    let class_row = *c.path.iter().rev().find(|&&row| {
        matches!(
            c.kind(row),
            NodeKind::ClassDeclaration
                | NodeKind::MixinDeclaration
                | NodeKind::EnumDeclaration
                | NodeKind::ExtensionTypeDeclaration
        )
    })?;
    let supertype = *crate::rules::same_file_ancestors(c, class_row, true).first()?;
    let mut found = None;
    for member in crate::rules::body_members(c, supertype) {
        if !matches!(
            c.kind(member),
            NodeKind::Constructor | NodeKind::PrimaryConstructor
        ) {
            continue;
        }
        for formal in crate::rules::types::leak_formal_rows(c, member) {
            let same_name = c
                .field(formal, PayloadField::Name)
                .filter(|&token| token != NONE_TOKEN)
                .is_some_and(|token| c.lexeme(token) == name);
            if !same_name {
                continue;
            }
            let clause = c
                .field(formal, PayloadField::InitializerStart)
                .filter(|&token| token != NONE_TOKEN);
            found = Some(match clause {
                None => true,
                Some(start) => {
                    let end = c.field(formal, PayloadField::InitializerEnd).unwrap_or(start);
                    c.lexeme(start) == "null" && end.saturating_sub(start) <= 1
                }
            });
        }
    }
    found
}

#[cfg(test)]
mod oracle_corpus_pins {
    use crate::testsupport::{Fixture, one_rule};

    /// The whole upstream oracle corpus for `avoid_init_to_null`: `AvoidInitToNullTest` + `AvoidInitToNullSuperFormalsTest` (28 cases) plus four `messages.yaml` doc entries,
    /// `conformance/status.tsv` `lint avoid_init_to_null 32 24 8`. Each entry is the exact
    /// materialized case source and the oracle's `(line, column, length)` for
    /// every `avoid_init_to_null` row; only the rule's own rows are pinned.
    ///
    /// 32 single-file cases pinned (15 oracle rows).
    #[test]
    fn avoid_init_to_null_oracle_corpus_pins() {
        // (case, source, oracle rows as (line, column, length)).
        let cases: &[(&str, &str, &[(u32, u32, u32)])] = &[
            (
                r#"lints/avoid_init_to_null/AvoidInitToNullSuperFormalsTest/test_forLoop#0"#,
                r#"
void f() {
  for (var i = null; i != null; i++) {
  }
}
"#,
                &[(3, 12, 8)],
            ),
            (
                r#"lints/avoid_init_to_null/AvoidInitToNullSuperFormalsTest/test_nullableStringInit#0"#,
                r#"
class A {
  String? a;
  A({this.a = null});
}
"#,
                &[(4, 6, 13)],
            ),
            (
                r#"lints/avoid_init_to_null/AvoidInitToNullSuperFormalsTest/test_superInit_2#0"#,
                r#"
class A {
  String? a;
  A({this.a = null});
}
class B extends A {
  B({super.a = null});
}
"#,
                &[(4, 6, 13), (7, 6, 14)],
            ),
            (
                r#"lints/avoid_init_to_null/AvoidInitToNullSuperFormalsTest/test_superInit_nolint#0"#,
                r#"
class A {
  String? a;
  A({this.a = ''});
}

class B extends A {
  B({super.a = null});
}
"#,
                &[],
            ),
            (
                r#"lints/avoid_init_to_null/AvoidInitToNullTest/test_fieldFormalParameter_inferredType#0"#,
                r#"
class C {
  int? i;
  C({this.i = null});
}
"#,
                &[(4, 6, 13)],
            ),
            (
                r#"lints/avoid_init_to_null/AvoidInitToNullTest/test_instanceField_inferredType_final#0"#,
                r#"
class C {
  final i = null;
}
"#,
                &[],
            ),
            (
                r#"lints/avoid_init_to_null/AvoidInitToNullTest/test_instanceField_intType_noInitializer#0"#,
                r#"
class C {
  int i;
  C(): i = 1;
}
"#,
                &[],
            ),
            (
                r#"lints/avoid_init_to_null/AvoidInitToNullTest/test_instanceField_nullableIntType#0"#,
                r#"
class C {
  int? i = null;
  C(): i = 1;
}
"#,
                &[(3, 8, 8)],
            ),
            (
                r#"lints/avoid_init_to_null/AvoidInitToNullTest/test_invalidAssignment_field#0"#,
                r#"
class X {
  int x = null;
}
"#,
                &[],
            ),
            (
                r#"lints/avoid_init_to_null/AvoidInitToNullTest/test_invalidAssignment_namedParameter#0"#,
                r#"
class X {
  X({int a = null});
}
"#,
                &[],
            ),
            (
                r#"lints/avoid_init_to_null/AvoidInitToNullTest/test_invalidAssignment_namedParameter_fieldFormal#0"#,
                r#"
class X {
  int x;
  X({this.x = null});
}
"#,
                &[],
            ),
            (
                r#"lints/avoid_init_to_null/AvoidInitToNullTest/test_invalidAssignment_topLevelVariable#0"#,
                r#"
int i = null;
"#,
                &[],
            ),
            (
                r#"lints/avoid_init_to_null/AvoidInitToNullTest/test_namedParameter_inferredType#0"#,
                r#"
foo({p = null}) {}
"#,
                &[(2, 6, 8)],
            ),
            (
                r#"lints/avoid_init_to_null/AvoidInitToNullTest/test_namedParameter_inferredType_defaultValueIsInt#0"#,
                r#"
foo({p = 1}) {}
"#,
                &[],
            ),
            (
                r#"lints/avoid_init_to_null/AvoidInitToNullTest/test_namedParameter_inferredType_noDefaultValue#0"#,
                r#"
foo({p}) {}
"#,
                &[],
            ),
            (
                r#"lints/avoid_init_to_null/AvoidInitToNullTest/test_namedParameter_inferredType_var#0"#,
                r#"
// @dart = 3.10
foo({var p = null}) {}
"#,
                &[(3, 6, 12)],
            ),
            (
                r#"lints/avoid_init_to_null/AvoidInitToNullTest/test_optionalParameter_inferredType#0"#,
                r#"
foo([p = null]) {}
"#,
                &[(2, 6, 8)],
            ),
            (
                r#"lints/avoid_init_to_null/AvoidInitToNullTest/test_optionalParameter_inferredType_defaultValueIsInt#0"#,
                r#"
foo([p = 1]) {}
"#,
                &[],
            ),
            (
                r#"lints/avoid_init_to_null/AvoidInitToNullTest/test_optionalParameter_inferredType_noDefaultValue#0"#,
                r#"
foo([p]) {}
"#,
                &[],
            ),
            (
                r#"lints/avoid_init_to_null/AvoidInitToNullTest/test_optionalParameter_inferredType_var#0"#,
                r#"
// @dart = 3.10
foo([var p = null]) {}
"#,
                &[(3, 6, 12)],
            ),
            (
                r#"lints/avoid_init_to_null/AvoidInitToNullTest/test_staticConstField_inferredType_final#0"#,
                r#"
class C {
  static const i = null;
}
"#,
                &[],
            ),
            (
                r#"lints/avoid_init_to_null/AvoidInitToNullTest/test_topLevelVariable_dynamic#0"#,
                r#"
dynamic i = null;
"#,
                &[(2, 9, 8)],
            ),
            (
                r#"lints/avoid_init_to_null/AvoidInitToNullTest/test_topLevelVariable_inferredType#0"#,
                r#"
var i = null;
"#,
                &[(2, 5, 8)],
            ),
            (
                r#"lints/avoid_init_to_null/AvoidInitToNullTest/test_topLevelVariable_inferredType_const#0"#,
                r#"
const i = null;
"#,
                &[],
            ),
            (
                r#"lints/avoid_init_to_null/AvoidInitToNullTest/test_topLevelVariable_inferredType_final#0"#,
                r#"
final i = null;
"#,
                &[],
            ),
            (
                r#"lints/avoid_init_to_null/AvoidInitToNullTest/test_topLevelVariable_inferredType_initializeToInt#0"#,
                r#"
var i = 1;
"#,
                &[],
            ),
            (
                r#"lints/avoid_init_to_null/AvoidInitToNullTest/test_topLevelVariable_inferredType_noInitialization#0"#,
                r#"
var i;
"#,
                &[],
            ),
            (
                r#"lints/avoid_init_to_null/AvoidInitToNullTest/test_topLevelVariable_nullableType#0"#,
                r#"
int? ii = null;
"#,
                &[(2, 6, 9)],
            ),
            (
                r#"lints/avoid_init_to_null/doc_bad_0"#,
                r#"class C {
  int? f = null;

  void m() {
    if (f != null) {
      print(f);
    }
  }
}
"#,
                &[(2, 8, 8)],
            ),
            (
                r#"lints/avoid_init_to_null/doc_bad_1"#,
                r#"Item? bestDeal(List<Item> cart) {
  Item? bestItem = null;

  for (final item in cart) {
    if (bestItem == null || item.price < bestItem.price) {
      bestItem = item;
    }
  }

  return bestItem;
}
"#,
                &[(2, 9, 15)],
            ),
            (
                r#"lints/avoid_init_to_null/doc_good_0"#,
                r#"class C {
  int? f;

  void m() {
    if (f != null) {
      print(f);
    }
  }
}
"#,
                &[],
            ),
            (
                r#"lints/avoid_init_to_null/doc_good_1"#,
                r#"Item? bestDeal(List<Item> cart) {
  Item? bestItem;

  for (final item in cart) {
    if (bestItem == null || item.price < bestItem.price) {
      bestItem = item;
    }
  }

  return bestItem;
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
            let mut lints = f.run(&one_rule("avoid_init_to_null"));
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
