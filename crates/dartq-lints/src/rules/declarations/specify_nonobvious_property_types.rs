use super::{Check, NodeKind, PayloadField, RuleSpec, fields_written_type, obvious_spelling};
use crate::rules::{
    MemberQuery, member_matches, same_file_ancestors, same_file_declaration, supertypes,
};

/// Official: `pkg/linter/lib/src/rules/specify_nonobvious_property_types.dart:47-120`
/// — a top-level or field variable list (:43-44) with no written type
/// (or exactly `Null`, :63-65) where some variable needs a type
/// reports at the list for a single variable (:112-113) or at each
/// needing variable (:115-116). An instance variable whose name a
/// super-interface accessor answers skips (:71-100 — the
/// override-inference face).
///
/// Approximation (documented): the override-inference face reads the
/// same-file ancestor chain — when any written supertype name leaves
/// the file the whole instance face under-reports (the resolved
/// `allSupertypes` walk is unavailable); static and top-level
/// variables carry no such face. A bare-identifier initializer is never
/// obvious (the rule-local `initializer_obvious` — upstream
/// `obvious_types.dart`:166-179 admits only locals and formals, which
/// cannot occur at these sites). Message: `diagnostic.g.dart`
/// `specifyNonobviousPropertyTypes`.
pub(crate) static SPEC: RuleSpec = crate::spec!(
    "specify_nonobvious_property_types",
    &[NodeKind::VariablesDeclaration],
    check,
);

fn check(c: &mut Check<'_>) {
    let node = c.node();
    let Some(parent) = c.ancestor(1) else {
        return;
    };
    if !matches!(c.kind(parent), NodeKind::TopLevelFields | NodeKind::Fields) {
        return;
    }
    // `staticType != null && !staticType.isDartCoreNull` (:63-65).
    if fields_written_type(c, node).is_some_and(|(start, _)| c.lexeme(start) != "Null") {
        return;
    }
    // The instance face: `!node.isStatic` (:52).
    let instance = c.kind(parent) == NodeKind::Fields
        && c.field(parent, PayloadField::MemberKind)
            == Some(dartq_parser::MemberKind::NonStaticField as u32);
    let container = c.path.iter().rev().find(|&&row| {
        matches!(
            c.kind(row),
            NodeKind::ClassDeclaration
                | NodeKind::MixinDeclaration
                | NodeKind::EnumDeclaration
                | NodeKind::ExtensionDeclaration
                | NodeKind::ExtensionTypeDeclaration
        )
    });
    // (:75-99) — the walk needs every supertype resolvable; a
    // cross-file link under-reports the whole instance face.
    let resolvable = container.is_none_or(|&row| {
        supertypes(c, row)
            .iter()
            .all(|(name, _)| same_file_declaration(c, name).is_some())
    });
    let mut needs = Vec::new();
    let mut declarators = 0;
    for var in c.collect_direct(node) {
        if c.kind(var) != NodeKind::InitializedIdentifier {
            continue;
        }
        declarators += 1;
        if instance {
            let overridden = container
                .zip(crate::rules::initialized_identifier_name(c, var))
                .is_some_and(|(&row, token)| ancestor_accessor(c, row, c.lexeme(token)));
            if overridden || !resolvable {
                continue;
            }
        }
        let initializer = c
            .collect_direct(var)
            .into_iter()
            .find(|&kid| c.kind(kid) == NodeKind::VariableInitializer);
        // `A()..x..x..x` — the `Cascade` row opens at the `..` operator
        // (bridge `begin_cascade`), so its target tiles as the sibling row
        // just before it and its own children are only the trailing
        // sections. The upstream face reads the cascade's target
        // (`obvious_types.dart`:199-200), not the section rows.
        let obvious = initializer.is_some_and(|init| {
            let kids = c.collect_direct(init);
            let value = match kids.iter().position(|&kid| c.kind(kid) == NodeKind::Cascade) {
                Some(0) | None => kids.last().copied(),
                Some(index) => Some(kids[index - 1]),
            };
            value.is_some_and(|value| initializer_obvious(c, value))
        });
        if !obvious {
            needs.push(var);
        }
    }
    if needs.is_empty() {
        return;
    }
    if declarators == 1 {
        // `reportAtNode(node)` (:122) — the list node: the keyword head
        // through the last initializer token, the trailing `;` owned by
        // the field/declaration wrapper excluded.
        if let Some(range) = super::variables_list_range(c, node) {
            c.report(range, &[]);
        }
    } else {
        for var in needs {
            c.report(c.utf16_span(var), &[]);
        }
    }
}

/// The rule-local obviousness face over the shared spelling helper: the
/// upstream `SimpleIdentifier` arm (`obvious_types.dart`:166-179) makes a
/// bare identifier obvious only when it names a local variable or formal
/// parameter — and neither binder exists at a top-level or field
/// declaration site, so a bare identifier is never obvious here (the
/// shared helper, built for the local-variable family, answers obvious
/// for every bare identifier). The unwrap faces follow the shared
/// helper: `ParenthesizedExpression` (:218-220; a parenthesized cascade
/// keeps its target as the row before the `Cascade`, so the first child
/// is the target). The `Cascade` arm is the guard for the anchor-miss
/// tiling (a `Cascade` with no target sibling): its children are only
/// the trailing sections, which are never obvious — the conservative
/// lint.
fn initializer_obvious(c: &Check<'_>, value: u32) -> bool {
    match c.kind(value) {
        NodeKind::Identifier => false,
        NodeKind::ParenthesizedExpression | NodeKind::Cascade => c
            .collect_direct(value)
            .into_iter()
            .next()
            .is_some_and(|inner| initializer_obvious(c, inner)),
        _ => obvious_spelling(c, value).is_some(),
    }
}

/// Whether a same-file ancestor declares a getter, setter or field of
/// `name` (the `getGetter`/`getSetter` face — a field carries both).
fn ancestor_accessor(c: &Check<'_>, container: u32, name: &str) -> bool {
    same_file_ancestors(c, container, false).iter().any(|&row| {
        super::container_members(c, row)
            .1
            .into_iter()
            .any(|member| match c.kind(member) {
                NodeKind::Fields => super::fields_declare(c, member, name),
                NodeKind::Method => {
                    !member_matches(c, member, MemberQuery::Getter, name).is_empty()
                        || !member_matches(c, member, MemberQuery::Setter, name).is_empty()
                }
                _ => false,
            })
    })
}

#[cfg(test)]
mod tests {
    use crate::testsupport::{Fixture, one_rule};

    /// The whole upstream corpus (`SpecifyNonObviousPropertyTypesTest`, 70
    /// cases, plus the two `messages.yaml` doc examples) pinned against the
    /// oracle rows of `conformance/expected.tsv`: each entry is the exact
    /// case source (the materialized file carries a leading blank line, so
    /// the upstream `3:3` rows land as written) and the oracle's
    /// `(line, column, length)` for every `specify_nonobvious_property_types`
    /// row. Every mismatch is collected before the single panic so one run
    /// shows the whole deviation set.
    #[test]
    fn specify_nonobvious_property_types_oracle_corpus_pins() {
        // (case, source, oracle rows as (line, column, length)).
        let cases: &[(&str, &str, &[(u32, u32, u32)])] = &[
    ("test_as_dynamic_instance", "\nclass A {\n  var d = 1 as dynamic;\n}\n", &[]),
    ("test_as_dynamic_static", "\nclass A {\n  static var d = 1 as dynamic;\n}\n", &[]),
    ("test_as_dynamic_topLevel", "\nvar d = 1 as dynamic;\n", &[]),
    ("test_as_instance", "\nclass A {\n  var d = 1 as num;\n}\n", &[]),
    ("test_as_static", "\nclass A {\n  static var d = 1 as num;\n}\n", &[]),
    ("test_as_topLevel", "\nvar d = 1 as num;\n", &[]),
    ("test_cascade_instance", "\nclass A {\n  var c = C()..x..x..x;\n}\n\nclass C {\n  final int x = 0;\n}\n", &[]),
    ("test_cascade_static", "\nclass A {\n  static var c = C()..x..x..x;\n}\n\nclass C {\n  final int x = 0;\n}\n", &[]),
    ("test_cascade_topLevel", "\nvar a = A()..x..x..x;\n\nclass A {\n  final int x = 0;\n}\n", &[]),
    ("test_genericInvocation_paramIsType_instance",
     "\nclass A {\n  final h = bar('');\n}\n\nT bar<T>(T d) => d;\n",
     &[(3, 3, 17)]),
    ("test_genericInvocation_paramIsType_ok_instance",
     "\nclass A {\n  String h = bar('');\n}\n\nT bar<T>(T d) => d;\n",
     &[]),
    ("test_genericInvocation_paramIsType_ok_static",
     "\nclass A {\n  static String h = bar('');\n}\n\nT bar<T>(T d) => d;\n",
     &[]),
    ("test_genericInvocation_paramIsType_ok_topLevel", "\nString h = bar('');\nT bar<T>(T d) => d;\n", &[]),
    ("test_genericInvocation_paramIsType_static",
     "\nclass A {\n  static final h = bar('');\n}\n\nT bar<T>(T d) => d;\n",
     &[(3, 10, 17)]),
    ("test_genericInvocation_paramIsType_topLevel", "\nfinal h = bar('');\n\nT bar<T>(T d) => d;\n", &[(2, 1, 17)]),
    ("test_genericInvocation_typeNeededForInference_instance",
     "\nclass A {\n  static var h = bar('');\n}\n\nT bar<T>(dynamic d) => d;\n",
     &[(3, 10, 15)]),
    ("test_genericInvocation_typeNeededForInference_ok_instance",
     "\nclass A {\n  String h = bar('');\n}\n\nT bar<T>(dynamic d) => d;\n",
     &[]),
    ("test_genericInvocation_typeNeededForInference_ok_static",
     "\nclass A {\n  static String h = bar('');\n}\n\nT bar<T>(dynamic d) => d;\n",
     &[]),
    ("test_genericInvocation_typeNeededForInference_ok_topLevel",
     "\nString h = bar('');\nT bar<T>(dynamic d) => d;\n",
     &[]),
    ("test_genericInvocation_typeNeededForInference_static",
     "\nclass A {\n  static var h = bar('');\n}\n\nT bar<T>(dynamic d) => d;\n",
     &[(3, 10, 15)]),
    ("test_genericInvocation_typeNeededForInference_topLevel",
     "\nvar h = bar('');\nT bar<T>(dynamic d) => d;\n",
     &[(2, 1, 15)]),
    ("test_genericInvocation_typeParamProvided_instance",
     "\nclass A {\n  var h = bar<String>('');\n}\n\nT bar<T>(dynamic d) => d;\n",
     &[(3, 3, 23)]),
    ("test_genericInvocation_typeParamProvided_ok_instance",
     "\nclass A {\n  String h = bar<String>('');\n}\n\nT bar<T>(dynamic d) => d;\n",
     &[]),
    ("test_genericInvocation_typeParamProvided_ok_static",
     "\nclass A {\n  static String h = bar<String>('');\n}\n\nT bar<T>(dynamic d) => d;\n",
     &[]),
    ("test_genericInvocation_typeParamProvided_ok_topLevel",
     "\nString h = bar<String>('');\nT bar<T>(dynamic d) => d;\n",
     &[]),
    ("test_genericInvocation_typeParamProvided_static",
     "\nclass A {\n  static var h = bar<String>('');\n}\n\nT bar<T>(dynamic d) => d;\n",
     &[(3, 10, 23)]),
    ("test_genericInvocation_typeParamProvided_topLevel",
     "\nvar h = bar<String>('');\nT bar<T>(dynamic d) => d;\n",
     &[(2, 1, 23)]),
    ("test_instanceCreation_generic_instance", "\nclass A {\n  var c = C(1);\n}\n\nclass C<X> {\n  C(X x);\n}\n", &[(3, 3, 12)]),
    ("test_instanceCreation_generic_ok1_instance", "\nclass A {\n  C<int> c = C<int>();\n}\n\nclass C<X> {}\n", &[]),
    ("test_instanceCreation_generic_ok1_static",
     "\nclass A {\n  static C<int> c = C<int>();\n}\n\nclass C<X> {}\n",
     &[]),
    ("test_instanceCreation_generic_ok1_topLevel", "\nA<int> a = A<int>();\n\nclass A<X> {}\n", &[]),
    ("test_instanceCreation_generic_ok2_instance", "\nclass A {\n  C<num> a = C<int>();\n}\n\nclass C<X> {}\n", &[]),
    ("test_instanceCreation_generic_ok2_static",
     "\nclass A {\n  static C<num> a = C<int>();\n}\n\nclass C<X> {}\n",
     &[]),
    ("test_instanceCreation_generic_ok2_topLevel", "\nA<num> a = A<int>();\n\nclass A<X> {}\n", &[]),
    ("test_instanceCreation_generic_static",
     "\nclass A {\n  static var c = C(1);\n}\n\nclass C<X> {\n  C(X x);\n}\n",
     &[(3, 10, 12)]),
    ("test_instanceCreation_generic_topLevel", "\nvar a = A(1);\n\nclass A<X> {\n  A(X x);\n}\n", &[(2, 1, 12)]),
    ("test_instanceCreation_nonGeneric_instance", "\nclass A {\n  C c = C();\n}\n\nclass C {}\n", &[]),
    ("test_instanceCreation_nonGeneric_static", "\nclass A {\n  static C c = C();\n}\n\nclass C {}\n", &[]),
    ("test_instanceCreation_nonGeneric_topLevel", "\nA a = A();\n\nclass A {}\n", &[]),
    ("test_literal_bool_instance", "\nclass A {\n  static bool b = true;\n}\n", &[]),
    ("test_literal_bool_static", "\nclass A {\n  static bool b = true;\n}\n", &[]),
    ("test_literal_bool_topLevel", "\nbool b = true;\n", &[]),
    ("test_literal_doubleTypedInt_instance", "\nclass A {\n  double d = 1;\n}\n", &[]),
    ("test_literal_doubleTypedInt_static", "\nclass A {\n  static double d = 1;\n}\n", &[]),
    ("test_literal_doubleTypedInt_topLevel", "\ndouble d = 1;\n", &[]),
    ("test_literal_double_instance", "\nclass A {\n  double d = 1.5;\n}\n", &[]),
    ("test_literal_double_static", "\nclass A {\n  static double d = 1.5;\n}\n", &[]),
    ("test_literal_double_topLevel", "\ndouble d = 1.5;\n", &[]),
    ("test_literal_int_instance", "\nclass A {\n  int i = 1;\n}\n", &[]),
    ("test_literal_int_static", "\nclass A {\n  static int i = 1;\n}\n", &[]),
    ("test_literal_int_topLevel", "\nint i = 1;\n", &[]),
    ("test_literal_null_instance", "\nclass A {\n  Null nil = null;\n}\n", &[]),
    ("test_literal_null_static", "\nclass A {\n  static Null nil = null;\n}\n", &[]),
    ("test_literal_null_topLevel", "\nNull nil = null;\n", &[]),
    ("test_literal_string_instance", "\nclass A {\n  String s = \"A string\";\n}\n", &[]),
    ("test_literal_string_static", "\nclass A {\n  static String s = \"A string\";\n}\n", &[]),
    ("test_literal_string_topLevel", "\nString s = \"A string\";\n", &[]),
    ("test_literal_symbol_instance", "\nclass A {\n  Symbol s = #print;\n}\n", &[]),
    ("test_literal_symbol_static", "\nclass A {\n  static Symbol s = #print;\n}\n", &[]),
    ("test_literal_symbol_topLevel", "\nSymbol s = #print;\n", &[]),
    ("test_multiple_instance", "\nclass A {\n  var a = 'a' + 'a', b = 'b' * 2;\n}\n", &[(3, 7, 13), (3, 22, 11)]),
    ("test_multiple_ok_instance", "\nclass A {\n  String a = 'a' + 'a', b = 'b'.toString();\n}\n", &[]),
    ("test_multiple_ok_static", "\nclass A {\n  static String a = 'a' + 'a', b = 'b'.toString();\n}\n", &[]),
    ("test_multiple_ok_topLevel", "\nString a = 'a' + 'a', b = 'b'.toString();\n", &[]),
    ("test_multiple_static", "\nclass A {\n  static var a = 'a' + 'a', b = 'b' * 2;\n}\n", &[(3, 14, 13), (3, 29, 11)]),
    ("test_multiple_topLevel", "\nvar a = 'a' + 'a', b = 'b' * 2;\n", &[(2, 5, 13), (2, 20, 11)]),
    ("test_override_getter_initialized",
     "\nclass B implements A {\n  var x = 'Hello, ' + 'world!';\n}\n\nabstract class A {\n  String get x;\n}\n",
     &[]),
    ("test_override_getter_uninitialized",
     "\nclass B implements A {\n  var x;\n  B(this.x);\n}\n\nabstract class A {\n  int get x;\n}\n",
     &[]),
    ("test_override_setter_initialized",
     "\nclass B implements A {\n  var x = 'Hello, ' + 'world!';\n}\n\nabstract class A {\n  set x (String _);\n}\n",
     &[]),
    ("test_override_setter_uninitialized",
     "\nclass B implements A {\n  var x;\n  B(this.x);\n}\n\nabstract class A {\n  set x(int _);\n}\n",
     &[]),
    ("doc_bad_0",
     "final myTopLevelVariable =\n    genericFunctionWrittenByOtherFolks(with, args);\n\nclass A {\n  static var myStaticVariable =\n      myTopLevelVariable.update('foo', null);\n}\n",
     &[(1, 1, 77), (5, 10, 67)]),
    ("doc_good_0",
     "final Map<String, Widget?> myTopLevelVariable =\n    genericFunctionWrittenByOtherFolks(with, args);\n\nclass A {\n  static Map<String, Widget?> myStaticVariable =\n      myTopLevelVariable.update('foo', null);\n}\n",
     &[]),
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
            let mut lints = f.run(&one_rule("specify_nonobvious_property_types"));
            lints.sort_by_key(|lint| lint.range.start);
            let got: Vec<(u32, u32)> = lints
                .iter()
                .map(|lint| (lint.range.start, lint.range.end))
                .collect();
            let want: Vec<(u32, u32)> = oracle
                .iter()
                .map(|&(line, column, length)| {
                    let start = offset(source, line, column);
                    (start, start + length)
                })
                .collect();
            if got != want {
                mismatches.push(format!("{case}: want {want:?} got {got:?}"));
            }
        }
        assert!(mismatches.is_empty(), "{}", mismatches.join("\n"));
    }
}
