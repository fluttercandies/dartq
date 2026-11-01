use super::{Check, NodeKind, PayloadField, RuleSpec};
use crate::rules::{MemberQuery, class_declaration_name, member_matches, same_file_ancestors};

/// Official: `upstream Dart SDK source, pkg/linter/lib/src/rules/overridden_fields.dart` — an
/// instance field whose inherited concrete member is a variable-backed
/// getter reports at the field name, with the defining interface's name
/// as the argument (:66-81).
///
/// Approximations (documented): `getInheritedConcreteMember` (:69-71)
/// is the same-file extends/with/on chain, concrete members only, plus
/// the semantic bridge's inheritance-chain arm for the cross-file
/// residue ([`dartq_semantic::Analysis::inherited_member`] with
/// `concrete_only`); the variable-backed getter test
/// (`isOriginVariable`, :72-73) is the ancestor's concrete `Fields` rows
/// exclusively in the same-file arm (a declared getter never matches, and
/// an `abstract` field declares no concrete getter, so it never matches
/// either) and the entry's `MemberKind::Field` in the bridge arm (a
/// declared getter captures as `Getter`). The defining interface (:74-78) is
/// the nearest ancestor class row declaring the field in the same-file
/// arm — the most-specific concrete declaration, as the official
/// lookup resolves — and the walk's defining-container name in the
/// bridge arm. Static fields skip (:40) and augmentations are
/// scanner-gated off. The primary-constructor parameter face (:51-64)
/// rides the same chain: every `var`/`final` formal of a primary
/// constructor declares a field (the `element is
/// FieldFormalParameterElement` read over the written keyword; a
/// `this.x` receiver formal is the AST `FieldFormalParameter` shape
/// and skips, and extension-type representation formals take no
/// keyword) and checks like a field, reporting at its name token.
/// Message: `diagnostic.g.dart` `overriddenFields`.
pub(crate) static SPEC: RuleSpec = crate::spec!(
    "overridden_fields",
    &[NodeKind::Fields, NodeKind::PrimaryConstructor],
    check,
);

fn check(c: &mut Check<'_>) {
    let node = c.node();
    if c.kind(node) == NodeKind::PrimaryConstructor {
        check_primary(c);
        return;
    }
    // `node.isStatic` (:40).
    if c.field(node, PayloadField::MemberKind)
        != Some(dartq_parser::MemberKind::NonStaticField as u32)
    {
        return;
    }
    let Some(&class_row) = c.path.iter().rev().find(|&&a| {
        matches!(
            c.kind(a),
            NodeKind::ClassDeclaration
                | NodeKind::MixinDeclaration
                | NodeKind::EnumDeclaration
                | NodeKind::ExtensionDeclaration
        )
    }) else {
        return;
    };
    let chain = same_file_ancestors(c, class_row, true);
    let fields: Vec<(&str, u32)> = field_names(c, node);
    // The nearest concrete variable-backed declaration in the chain
    // supplies the defining interface (:69-78); names collect owned —
    // the lookup borrows `c` immutably across the report.
    let mut hits: Vec<(u32, String)> = Vec::new();
    for (name, name_row) in fields {
        if let Some(defining) = inherited_variable_holder(c, &chain, name)
            .map(str::to_owned)
            .or_else(|| bridge_variable_holder(c, class_row, name))
        {
            hits.push((name_row, defining));
        }
    }
    for (name_row, defining) in hits {
        // The name row is the name token (`initialized_identifier_name`)
        // — the official `reportAtToken(nameToken)` anchor (:79).
        let range = crate::rules::utf16_token_span(c, name_row);
        c.report(range, &[defining.as_str()]);
    }
}

/// The primary-constructor parameter face (:51-64): each `var`/`final`
/// formal of a primary constructor declares a field and checks like
/// one — the inherited getter namespace, reporting at the name token.
/// A `this.x` receiver formal is the AST field-formal shape and skips
/// (:53); a nameless formal has no element upstream and never checks.
fn check_primary(c: &mut Check<'_>) {
    let node = c.node();
    let Some(owner) = c.ancestor(1) else {
        return;
    };
    let chain = same_file_ancestors(c, owner, true);
    let mut hits: Vec<(u32, String)> = Vec::new();
    for param in crate::rules::types::leak_formal_rows(c, node) {
        // `parameter is! FieldFormalParameter` (:53) — the `this.x`
        // receiver formal.
        let receiver = c.field(param, PayloadField::Receiver);
        if receiver.is_some_and(|token| token != crate::rules::NONE_TOKEN) {
            continue;
        }
        let Some(name_token) = c.field(param, PayloadField::Name) else {
            continue;
        };
        if name_token == crate::rules::NONE_TOKEN {
            continue;
        }
        // The declaring-field face: the `var`/`final` keyword (the
        // `element is FieldFormalParameterElement` read; `const`
        // declares no field).
        let Some(keyword) = primary_field_keyword(c, param, name_token) else {
            continue;
        };
        if !matches!(c.lexeme(keyword), "final" | "var") {
            continue;
        }
        if let Some(defining) = inherited_variable_holder(c, &chain, c.lexeme(name_token))
            .map(str::to_owned)
            .or_else(|| bridge_variable_holder(c, owner, c.lexeme(name_token)))
        {
            hits.push((name_token, defining));
        }
    }
    for (name_token, defining) in hits {
        let range = crate::rules::utf16_token_span(c, name_token);
        c.report(range, &[defining.as_str()]);
    }
}

/// The last `const`/`final`/`var` keyword written before the formal's
/// name (the primary formal's storage keyword — `None` for a
/// keywordless formal).
fn primary_field_keyword(c: &Check<'_>, param: u32, name_token: u32) -> Option<u32> {
    let mut keyword = None;
    for token in c.unit.ast.token_range(param) {
        if token >= name_token {
            break;
        }
        if matches!(c.lexeme(token), "const" | "final" | "var") {
            keyword = Some(token);
        }
    }
    keyword
}

/// The nearest ancestor in `chain` declaring a *concrete*, variable-backed
/// field of `name`, as its class-like name (the defining interface, :74-78).
/// An `abstract` field declares no concrete getter, so
/// `getInheritedConcreteMember` (:69-71) skips it.
fn inherited_variable_holder<'a>(c: &'a Check<'_>, chain: &[u32], name: &str) -> Option<&'a str> {
    chain.iter().find_map(|&ancestor| {
        crate::rules::body_members(c, ancestor)
            .into_iter()
            .find(|&member| {
                c.kind(member) == NodeKind::Fields
                    && !is_abstract_field(c, member)
                    && !member_matches(c, member, MemberQuery::Variable, name).is_empty()
            })
            .and_then(|_| class_declaration_name(c, ancestor))
    })
}

/// Whether the `Fields` group is an `abstract` declaration (`abstract int
/// x;`) — the `abstract` modifier word rides the member's wrapper row, so
/// both token runs are read.
fn is_abstract_field(c: &Check<'_>, member: u32) -> bool {
    let contains = |row: u32| {
        c.unit
            .ast
            .token_range(row)
            .any(|token| c.lexeme(token) == "abstract")
    };
    contains(member)
        || c.unit
            .ast
            .parent(member)
            .is_some_and(|wrapper| wrapper != member && contains(wrapper))
}

/// The cross-file arm of the inherited concrete member's lookup
/// (`getInheritedConcreteMember` — each written extends/with/on
/// supertype anchors the bridge's concrete walk), gated to the
/// variable-backed face (`isOriginVariable`, :72-73 — a captured
/// `MemberKind::Field`; a declared getter captures as `Getter` and
/// never reports). The defining interface is the walk's
/// defining-container name; unresolved chains stay silent.
fn bridge_variable_holder(c: &Check<'_>, class_row: u32, name: &str) -> Option<String> {
    let (defining, entry) = super::bridge_inherited_member(c, class_row, true, name)?;
    (entry.kind == dartq_semantic::model::MemberKind::Field).then_some(defining)
}

/// The `(name lexeme, name row)` pairs of one `Fields` group.
fn field_names<'a>(c: &Check<'a>, node: u32) -> Vec<(&'a str, u32)> {
    let mut out = Vec::new();
    for decl in c.collect_direct(node) {
        if c.kind(decl) != NodeKind::VariablesDeclaration {
            continue;
        }
        for var in c.collect_direct(decl) {
            if c.kind(var) != NodeKind::InitializedIdentifier {
                continue;
            }
            if let Some(row) = crate::rules::initialized_identifier_name(c, var) {
                out.push((c.lexeme(row), row));
            }
        }
    }
    out
}

#[cfg(test)]
mod oracle_corpus_pins {
    use crate::testsupport::{Fixture, one_rule};

    /// The whole upstream oracle corpus for `overridden_fields`: `OverriddenFieldsTest` (21 single-file cases) plus six `messages.yaml` doc entries,
    /// `conformance/status.tsv` `lint overridden_fields 27 19 8`. Each entry is the exact
    /// materialized case source and the oracle's `(line, column, length)` for
    /// every `overridden_fields` row; only the rule's own rows are pinned.
    ///
    /// 25 single-file cases pinned (23 oracle rows).
    /// Not representable in a single-source fixture (aux files or packages):
    /// `OverriddenFieldsTest/test_augmentationClass#0` (aux part file a.dart holds the overridden field's ancestor).
    /// `OverriddenFieldsTest/test_augmentationClass#1` (multi_file).
    /// `OverriddenFieldsTest/test_augmentedField#1` (multi_file).
    /// `OverriddenFieldsTest/test_externalLibrary#0` (imported a.dart declares the ancestor field).
    #[test]
    fn overridden_fields_oracle_corpus_pins() {
        // (case, source, oracle rows as (line, column, length)).
        let cases: &[(&str, &str, &[(u32, u32, u32)])] = &[
            (
                r#"lints/overridden_fields/OverriddenFieldsTest/test_augmentedField#0"#,
                r#"
part 'b.dart';

class O {
  final a = '';
}

class A extends O {
  @override
  final a = '';
}
"#,
                &[(10, 9, 1)],
            ),
            (
                r#"lints/overridden_fields/OverriddenFieldsTest/test_conflictingFieldAndMethod#0"#,
                r#"
class A {
  int x() => 0;
}

class B extends A {
  int x = 9;
}
"#,
                &[],
            ),
            (
                r#"lints/overridden_fields/OverriddenFieldsTest/test_conflictingStaticAndInstance#0"#,
                r#"
class A {
  static final String field = 'value';
}

class B extends A {
  String field = 'otherValue';
}
"#,
                &[],
            ),
            (
                r#"lints/overridden_fields/OverriddenFieldsTest/test_extendingClass_multipleDeclarations#0"#,
                r#"
class A {
  int y = 1;
}
class B extends A {
  final x = 1, y = 2;
}
"#,
                &[(6, 16, 1)],
            ),
            (
                r#"lints/overridden_fields/OverriddenFieldsTest/test_extendingClass_overridingAbstract#0"#,
                r#"
abstract class A {
  abstract int x;
}
class B extends A {
  @override
  int x = 1;
}
"#,
                &[],
            ),
            (
                r#"lints/overridden_fields/OverriddenFieldsTest/test_extendingClass_staticField#0"#,
                r#"
class A {
  static int x = 1;
}
class B extends A {
  static int x = 2;
}
"#,
                &[],
            ),
            (
                r#"lints/overridden_fields/OverriddenFieldsTest/test_extendsClass_indirectly#0"#,
                r#"
class A {
  int x = 0;
}
class B extends A {}
class C extends B {
  @override
  int x = 1;
}
"#,
                &[(8, 7, 1)],
            ),
            (
                r#"lints/overridden_fields/OverriddenFieldsTest/test_externalLibraryWithPrivateField#0"#,
                r#"
import 'a.dart';
class B extends A {
  int? _private;
}
"#,
                &[],
            ),
            (
                r#"lints/overridden_fields/OverriddenFieldsTest/test_fieldOverridesGetter#0"#,
                r#"
class A {
  int get a => 0;
}
class B extends A {
  @override
  int a = 1;
}
"#,
                &[],
            ),
            (
                r#"lints/overridden_fields/OverriddenFieldsTest/test_implementingClass#0"#,
                r#"
class A {
  int x = 1;
}
class B implements A {
  @override
  int x = 2;
}
"#,
                &[],
            ),
            (
                r#"lints/overridden_fields/OverriddenFieldsTest/test_mixinInheritsFromNotObject#0"#,
                r#"
class Base {
  Object field = 'lorem';

  Object something = 'change';
}

class Bad1 extends Base {
  @override
  final x = 1, field = 'ipsum';
}

class GC11 extends Bad1 {
  @override
  Object something = 'done';

  Object gc33 = 'gc33';
}

class GC13 extends Object with Bad1 {
  @override
  Object something = 'done';

  @override
  Object field = 'lint';
}

abstract class GC21 extends GC11 {
  @override
  Object something = 'done';
}

class GC23 extends Object with GC13 {
  @override
  Object something = 'done';

  @override
  Object field = 'lint';
}

abstract class GC31 extends GC13 {
  @override
  Object something = 'done';
}

abstract class GC32 implements GC13 {
  @override
  Object something = 'done';
}

class GC33 extends GC21 with GC13 {
  @override
  Object something = 'done';

  @override
  Object gc33 = 'yada';
}

class GC34 extends GC33 {
  @override
  var x = 3;

  @override
  Object gc33 = 'yada';
}
"#,
                &[(10, 16, 5), (15, 10, 9), (22, 10, 9), (25, 10, 5), (30, 10, 9), (35, 10, 9), (38, 10, 5), (43, 10, 9), (53, 10, 9), (56, 10, 4), (61, 7, 1), (64, 10, 4)],
            ),
            (
                r#"lints/overridden_fields/OverriddenFieldsTest/test_mixinSuperclassConstraint#0"#,
                r#"
class A {
  int x = 1;
}
mixin M on A {
  @override
  final x = 1;
}
"#,
                &[(7, 9, 1)],
            ),
            (
                r#"lints/overridden_fields/OverriddenFieldsTest/test_mixingInMixin#0"#,
                r#"
mixin M {
  int x = 1;
}
class A with M {
  @override
  int x = 2;
}
"#,
                &[(7, 7, 1)],
            ),
            (
                r#"lints/overridden_fields/OverriddenFieldsTest/test_mixingInMixin_overridingAbstract#0"#,
                r#"
mixin M {
  abstract int x;
}
class A with M {
  @override
  int x = 2;
}
"#,
                &[],
            ),
            (
                r#"lints/overridden_fields/OverriddenFieldsTest/test_overridingAbstractField#0"#,
                r#"
abstract class A {
  abstract int x;
}

class B extends A {
  @override
  int x = 1;
}
"#,
                &[],
            ),
            (
                r#"lints/overridden_fields/OverriddenFieldsTest/test_privateFieldInSameLibrary#0"#,
                r#"
class A {
  int _x = 0;
}

class B extends A {
  int _x = 9;
}
"#,
                &[(7, 7, 2)],
            ),
            (
                r#"lints/overridden_fields/OverriddenFieldsTest/test_publicFieldFromDeclaringParameter#0"#,
                r#"
class A {
  int x = 0;
}

class B(var int x) extends A {}
"#,
                &[(6, 17, 1)],
            ),
            (
                r#"lints/overridden_fields/OverriddenFieldsTest/test_publicFieldInSameLibrary#0"#,
                r#"
class A {
  int x = 0;
}

class B extends A {
  int x = 9;
}
"#,
                &[(7, 7, 1)],
            ),
            (
                r#"lints/overridden_fields/OverriddenFieldsTest/test_recursiveInterfaceInheritance#0"#,
                r#"
class A extends B {}
class B extends A {
  int field = 0;
}
"#,
                &[],
            ),
            (
                r#"lints/overridden_fields/doc_bad_0"#,
                r#"class A {
  int f = 1;
}

class B extends A {
  @override
  int f = 2;
}
"#,
                &[(7, 7, 1)],
            ),
            (
                r#"lints/overridden_fields/doc_bad_1"#,
                r#"class Base {
  Object field = 'lorem';

  Object something = 'change';
}

class Bad1 extends Base {
  @override
  final field = 'ipsum'; // LINT
}

class Bad2 extends Base {
  @override
  Object something = 'done'; // LINT
}
"#,
                &[(9, 9, 5), (14, 10, 9)],
            ),
            (
                r#"lints/overridden_fields/doc_good_0"#,
                r#"class A {
  int f = 1;
}

class B extends A {}
"#,
                &[],
            ),
            (
                r#"lints/overridden_fields/doc_good_1"#,
                r#"class A {
  int f = 1;
}

class B extends A {
  int g = 2;
}
"#,
                &[],
            ),
            (
                r#"lints/overridden_fields/doc_good_2"#,
                r#"class Base {
  Object field = 'lorem';

  Object something = 'change';
}

class Ok extends Base {
  Object newField; // OK

  final Object newFinal = 'ignore'; // OK
}
"#,
                &[],
            ),
            (
                r#"lints/overridden_fields/doc_good_3"#,
                r#"abstract class BaseLoggingHandler {
  Base transformer;
}

class LogPrintHandler implements BaseLoggingHandler {
  @override
  Derived transformer; // OK
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
            let mut lints = f.run(&one_rule("overridden_fields"));
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
