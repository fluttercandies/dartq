use dartq_ast::NodeKind;

use dartq_language::{FeatureSet, LanguageVersion};
use dartq_parser::ParserFeatures;
use dartq_resolution::{
    Combinators, ImportEdge, LibraryClassModifiers, LibraryGraph, LibraryGraphBuilder,
    Namespaces,
};
use dartq_semantic::{ElementIndex, MicroKind, StrictModes, analyze_unit, collect};
use dartq_source::{FileTable, StringTable};

use crate::engine::{Lint, narrow_u32};
use crate::testsupport::{Fixture, NONE, identifier, one_rule, tok, unit_of, utf16_at};

/// Runs `rule` over `source` with the semantic face mounted: the
/// source folded as the sole analyzed file of `package:t` — the
/// driver's per-file build shape (the #1049 face construction).
/// `dependency` analyzes a second `kit.dart` and imports it into the
/// file's library: `Some((text, None))` plain, `Some((text,
/// Some(prefix)))` deferred-prefixed, `None` no import edge. The
/// context tables leak (`'static`); the tests are one-shot processes.
fn run_semantic(
    source: &str,
    rule: &str,
    dependency: Option<(&str, Option<&str>)>,
) -> Vec<Lint> {
    let fixture = Fixture::parse(source);
    let strings: &'static StringTable = Box::leak(Box::new(StringTable::new()));
    let mut files = FileTable::new();
    let file = files.intern(std::path::Path::new("lint_semantic.dart"));
    let features = ParserFeatures::from_feature_set(FeatureSet::for_language_version(
        LanguageVersion::new(3, 15),
    ));
    let scanned = dartq_scanner::scan(source, crate::testsupport::CONFIG, file);
    assert!(
        scanned.diagnostics.is_empty(),
        "scanner diagnostics: {source:?}"
    );
    let elements = collect(source, &scanned.tokens, features, strings);
    let mut builder = LibraryGraphBuilder::new(strings);
    let lib = builder.intern("package:t/lint_semantic.dart");
    for (name, declaration) in elements.declarations() {
        if declaration.kind == MicroKind::TypeParam {
            continue;
        }
        builder.declare(lib, name, file);
    }
    let mut index = ElementIndex::default();
    index.insert(file, elements);
    if let Some((dependency, prefix)) = dependency {
        let dep_file = files.intern(std::path::Path::new("kit.dart"));
        let scanned_dep = dartq_scanner::scan(dependency, crate::testsupport::CONFIG, dep_file);
        assert!(
            scanned_dep.diagnostics.is_empty(),
            "scanner diagnostics: {dependency:?}"
        );
        let dep_elements = collect(dependency, &scanned_dep.tokens, features, strings);
        let dep_lib = builder.intern("package:t/kit.dart");
        for (name, declaration) in dep_elements.declarations() {
            if declaration.kind == MicroKind::TypeParam {
                continue;
            }
            builder.declare(dep_lib, name, dep_file);
        }
        index.insert(dep_file, dep_elements);
        let (prefix, deferred) = match prefix {
            Some(prefix) => (Some(strings.intern(prefix)), true),
            None => (None, false),
        };
        builder.import(
            lib,
            ImportEdge {
                target: Some(dep_lib),
                prefix,
                deferred,
                combinators: Combinators::unfiltered(),
            },
        );
    }
    let graph: &'static LibraryGraph = Box::leak(Box::new(builder.finish()));
    let namespaces: &'static Namespaces = Box::leak(Box::new(Namespaces::build(graph)));
    let index: &'static ElementIndex = Box::leak(Box::new(index));
    let analysis = analyze_unit(
        source,
        &scanned.tokens,
        features,
        strings,
        file,
        lib,
        graph,
        namespaces,
        index,
        StrictModes::default(),
        &LibraryClassModifiers::default(),
    );
    let unit = crate::engine::Unit {
        source: &fixture.source,
        tokens: &fixture.tokens,
        ast: &fixture.ast,
        lines: &fixture.lines,
        path: Some(&fixture.path),
        features: fixture.features,
        version: fixture.version,
        workspace: None,
        reachability: None,
        async_context_uses: None,
        library_type_uses: None,
    };
    unit.run_with_semantics(
        &one_rule(rule),
        &rustc_hash::FxHashSet::default(),
        Some(&analysis),
    )
}

/// `test/rules/unnecessary_new_test.dart` `test_new` — the keyword
/// reports, zero-width at its start.
#[test]
fn unnecessary_new_reports_the_keyword() {
    let source = "new B();";
    // new0 B1 (2 )3 ;4
    let f = Fixture::build(
        source,
        unit_of(|b, _t| {
            b.begin(NodeKind::ExpressionStatement, 0);
            b.begin(NodeKind::NewExpression, 0);
            identifier(b, 1, 2);
            b.begin(NodeKind::Arguments, 2);

            b.end(NodeKind::Arguments, 4);
            b.end(NodeKind::NewExpression, 4);
            b.end(NodeKind::ExpressionStatement, 5);
        }),
    );
    let lints = f.run(&one_rule("unnecessary_new"));
    assert_eq!(lints.len(), 1);
    assert_eq!(lints[0].message, "Unnecessary 'new' keyword.");
    assert_eq!(lints[0].range.start, utf16_at(&f, 0));
}

/// Implicit creation (`test_implicit`) never dispatches.
#[test]
fn unnecessary_new_implicit_creation_passes() {
    let source = "B();";
    let f = Fixture::build(
        source,
        unit_of(|b, _t| {
            b.begin(NodeKind::ExpressionStatement, 0);
            b.leaf_span(NodeKind::Send, 0, 3);
            b.end(NodeKind::ExpressionStatement, 4);
        }),
    );
    assert!(f.run(&one_rule("unnecessary_new")).is_empty());
}

/// `test/rules/unnecessary_null_in_if_null_operators_test.dart`
/// `test_rhsNull` — the right `null` reports first.
#[test]
fn if_null_right_null_reports() {
    let source = "var a = b ?? null;";
    // var0 a1 =2 b3 ??4 null5 ;6
    let f = Fixture::build(
        source,
        unit_of(|b, t| {
            b.begin(NodeKind::VariablesDeclaration, 0);
            b.patch(0, tok(source, t, "var", 0));
            b.begin(NodeKind::InitializedIdentifier, 1);

            b.begin(NodeKind::VariableInitializer, 2);
            b.begin(NodeKind::BinaryExpression, 3);
            b.patch(0, tok(source, t, "??", 0));
            identifier(b, 3, 4);
            b.leaf(NodeKind::LiteralNull, 5);
            b.end(NodeKind::BinaryExpression, 6);
            b.end(NodeKind::VariableInitializer, 6);
            b.end(NodeKind::InitializedIdentifier, 6);
            b.end(NodeKind::VariablesDeclaration, 7);
        }),
    );
    let lints = f.run(&one_rule("unnecessary_null_in_if_null_operators"));
    assert_eq!(lints.len(), 1);
    assert_eq!(lints[0].message, "Unnecessary use of '??' with 'null'.");
    assert_eq!(lints[0].range.start, utf16_at(&f, 5));
}

/// The left `null` reports when the right operand is a value
/// (`test_lhsNull`).
#[test]
fn if_null_left_null_reports() {
    let source = "null ?? b;";
    // null0 ??1 b2 ;3
    let f = Fixture::build(
        source,
        unit_of(|b, t| {
            b.begin(NodeKind::ExpressionStatement, 0);
            b.begin(NodeKind::BinaryExpression, 0);
            b.patch(0, tok(source, t, "??", 0));
            b.leaf(NodeKind::LiteralNull, 0);
            identifier(b, 2, 3);
            b.end(NodeKind::BinaryExpression, 3);
            b.end(NodeKind::ExpressionStatement, 4);
        }),
    );
    let lints = f.run(&one_rule("unnecessary_null_in_if_null_operators"));
    assert_eq!(lints.len(), 1);
    assert_eq!(lints[0].range.start, utf16_at(&f, 0));
}

#[test]
fn if_null_without_null_passes() {
    let source = "b ?? c;";
    let f = Fixture::build(
        source,
        unit_of(|b, t| {
            b.begin(NodeKind::ExpressionStatement, 0);
            b.begin(NodeKind::BinaryExpression, 0);
            b.patch(0, tok(source, t, "??", 0));
            identifier(b, 0, 1);
            identifier(b, 2, 3);
            b.end(NodeKind::BinaryExpression, 3);
            b.end(NodeKind::ExpressionStatement, 4);
        }),
    );
    assert!(
        f.run(&one_rule("unnecessary_null_in_if_null_operators"))
            .is_empty()
    );
}

/// `test/rules/prefer_is_not_operator_test.dart` `test_notIs` — the
/// whole prefix expression reports, in the statement context (the
/// production flat tiling: the `!` leaf follows the paren row).
#[test]
fn prefer_is_not_reports_the_prefix() {
    let f = Fixture::parse("void f(Object a) {\n  !(a is int);\n}\n");
    let lints = f.run(&one_rule("prefer_is_not_operator"));
    assert_eq!(lints.len(), 1);
    assert_eq!(
        lints[0].message,
        "Use the 'is!' operator rather than negating the value of the 'is' operator."
    );
    let bang = tok(&f.source, &f.tokens, "!", 0);
    assert_eq!(lints[0].range.start, utf16_at(&f, bang));
    assert_eq!(lints[0].range.end - lints[0].range.start, 11); // `!(a is int)`
}

/// The corpus face: a parenthesized `is` under `!` inside an
/// `if` condition — the parenthesized condition adopts the `!` leaf
/// alone, read as its head child beside the paren row.
#[test]
fn prefer_is_not_reports_in_an_if_condition() {
    let f = Fixture::parse("void f(Object a) {\n  if (!(a is int)) {\n    print(0);\n  }\n}\n");
    let lints = f.run(&one_rule("prefer_is_not_operator"));
    assert_eq!(lints.len(), 1);
    let bang = tok(&f.source, &f.tokens, "!", 0);
    assert_eq!(lints[0].range.start, utf16_at(&f, bang));
    assert_eq!(lints[0].range.end - lints[0].range.start, 11);
}

/// The adopted-begin binary lays the `!` leaf back as its first child
/// when the paren operand finalizes lazily (`!(a is int) && b`).
#[test]
fn prefer_is_not_reports_as_a_binary_left_operand() {
    let f = Fixture::parse(
        "void f(Object a, bool b) {\n  var r = !(a is int) && b;\n  print(r);\n}\n",
    );
    let lints = f.run(&one_rule("prefer_is_not_operator"));
    assert_eq!(lints.len(), 1);
    let bang = tok(&f.source, &f.tokens, "!", 0);
    assert_eq!(lints[0].range.start, utf16_at(&f, bang));
}

/// `is!` under a `!` already uses the operator (`test_isNot`).
#[test]
fn prefer_is_not_bare_is_bang_passes() {
    let f =
        Fixture::parse("void f(Object a) {\n  if (!(a is! int)) {\n    print(0);\n  }\n}\n");
    assert!(f.run(&one_rule("prefer_is_not_operator")).is_empty());
}

/// The parenthesized-`!` chain is required (`prefer_is_not_operator.dart`:
/// 38-54) — a bare `is` never dispatches, a double paren breaks the
/// direct chain, and a non-bang prefix stays silent.
#[test]
fn prefer_is_not_unwrapped_shapes_pass() {
    assert!(
        Fixture::parse("void f(Object a) {\n  if (a is int) {\n    print(0);\n  }\n}\n")
            .run(&one_rule("prefer_is_not_operator"))
            .is_empty()
    );
    assert!(
        Fixture::parse("void f(Object a) {\n  !((a is int));\n}\n")
            .run(&one_rule("prefer_is_not_operator"))
            .is_empty()
    );
    assert!(
        Fixture::parse(
            "void f(Object a) {\n  if (-(a is int).hashCode) {\n    print(0);\n  }\n}\n"
        )
        .run(&one_rule("prefer_is_not_operator"))
        .is_empty()
    );
}

/// `test/rules/prefer_adjacent_string_concatenation_test.dart`
/// `test_adjacentStrings`.
#[test]
fn adjacent_strings_reports_the_plus() {
    let source = "'a' + 'b';";
    // 'a'0 +1 'b'2 ;3
    let f = Fixture::build(
        source,
        unit_of(|b, t| {
            b.begin(NodeKind::ExpressionStatement, 0);
            b.begin(NodeKind::BinaryExpression, 0);
            b.patch(0, tok(source, t, "+", 0));
            b.leaf_span(NodeKind::StringLiteral, 0, 1);
            b.leaf_span(NodeKind::StringLiteral, 2, 3);
            b.end(NodeKind::BinaryExpression, 3);
            b.end(NodeKind::ExpressionStatement, 4);
        }),
    );
    let lints = f.run(&one_rule("prefer_adjacent_string_concatenation"));
    assert_eq!(lints.len(), 1);
    assert_eq!(
        lints[0].message,
        "String literals shouldn't be concatenated by the '+' operator."
    );
    assert_eq!(lints[0].range.start, utf16_at(&f, 1));
}

#[test]
fn adjacent_strings_mixed_operands_passes() {
    let source = "'a' + b;";
    let f = Fixture::build(
        source,
        unit_of(|b, t| {
            b.begin(NodeKind::ExpressionStatement, 0);
            b.begin(NodeKind::BinaryExpression, 0);
            b.patch(0, tok(source, t, "+", 0));
            b.leaf_span(NodeKind::StringLiteral, 0, 1);
            identifier(b, 2, 3);
            b.end(NodeKind::BinaryExpression, 3);
            b.end(NodeKind::ExpressionStatement, 4);
        }),
    );
    assert!(
        f.run(&one_rule("prefer_adjacent_string_concatenation"))
            .is_empty()
    );
}

/// `test/rules/avoid_single_cascade_in_expression_statements_test.dart`
/// `test_oneCascade` — message spells the `.` operator; the report
/// covers the target through the section (the production row opens at
/// the `..`).
#[test]
fn single_cascade_reports_with_dot() {
    let f = Fixture::parse(
        "void f() {\n  var buffer = StringBuffer();\n  buffer..write('x');\n  print(buffer);\n}\n",
    );
    let lints = f.run(&one_rule("avoid_single_cascade_in_expression_statements"));
    assert_eq!(lints.len(), 1);
    assert_eq!(lints[0].message, "Unnecessary cascade expression.");
    assert_eq!(
        lints[0].correction,
        Some("Try using the operator '.'.".to_owned())
    );
    let buffer = tok(&f.source, &f.tokens, "buffer", 1);
    assert_eq!(lints[0].range.start, utf16_at(&f, buffer));
    assert_eq!(lints[0].range.end - lints[0].range.start, 18);
}

/// A `?..` section prints `?.` (avoid_single_cascade...dart:40-51).
#[test]
fn single_null_aware_cascade_suggests_qmark_dot() {
    let f = Fixture::parse("void f() {\n  var a = A();\n  a?..b();\n}\n");
    let lints = f.run(&one_rule("avoid_single_cascade_in_expression_statements"));
    assert_eq!(lints.len(), 1);
    assert_eq!(
        lints[0].correction,
        Some("Try using the operator '?.'.".to_owned())
    );
}

#[test]
fn multi_section_cascade_passes() {
    let f = Fixture::parse("void f() {\n  var a = A();\n  a..b()..c();\n}\n");
    assert!(
        f.run(&one_rule("avoid_single_cascade_in_expression_statements"))
            .is_empty()
    );
}

/// A cascade outside an expression statement keeps its meaning — an
/// initializer (or any non-statement owner) stays silent.
#[test]
fn cascade_initializer_passes() {
    let f = Fixture::parse("void f() {\n  var a = A()..b();\n  print(a);\n}\n");
    assert!(
        f.run(&one_rule("avoid_single_cascade_in_expression_statements"))
            .is_empty()
    );
}

// —— prefer_conditional_assignment ———————————————————————————————————

/// `test/rules/prefer_conditional_assignment_test.dart` `test_ifNullAssign`.
#[test]
fn conditional_assignment_reports_the_if() {
    let source = "var x;\nif (x == null) x = 1;";
    // var0 x1 ;2 if3 (4 x5 ==6 null7 )8 x9 =10 111 ;12
    let f = Fixture::build(
        source,
        unit_of(|b, t| {
            b.begin(NodeKind::VariablesDeclaration, 0);
            b.patch(0, 0);
            b.begin(NodeKind::InitializedIdentifier, 1);
            b.end(NodeKind::InitializedIdentifier, 2);
            b.end(NodeKind::VariablesDeclaration, 3);
            b.begin(NodeKind::IfStatement, 3);
            b.patch(0, NONE); // no else
            b.begin(NodeKind::ParenthesizedCondition, 4);
            b.patch(0, NONE);
            b.patch(1, NONE);
            b.begin(NodeKind::BinaryExpression, 5);
            b.patch(0, tok(source, t, "==", 0));
            identifier(b, 5, 6);
            b.leaf(NodeKind::LiteralNull, 7);
            b.end(NodeKind::BinaryExpression, 8);
            b.end(NodeKind::ParenthesizedCondition, 9);
            b.begin(NodeKind::ExpressionStatement, 9);
            b.begin(NodeKind::Assignment, 9);
            b.patch(0, tok(source, t, "=", 0));
            identifier(b, 9, 10);
            b.leaf(NodeKind::LiteralInt, 11);
            b.end(NodeKind::Assignment, 12);
            b.end(NodeKind::ExpressionStatement, 13);
            b.end(NodeKind::IfStatement, 13);
        }),
    );
    let lints = f.run(&one_rule("prefer_conditional_assignment"));
    assert_eq!(lints.len(), 1);
    assert_eq!(
        lints[0].message,
        "The 'if' statement could be replaced by a null-aware assignment."
    );
    assert_eq!(lints[0].range.start, utf16_at(&f, 3));
}

/// An else arm disqualifies (:70-72).
#[test]
fn conditional_assignment_with_else_passes() {
    let source = "if (x == null) x = 1; else ;";
    let f = Fixture::build(
        source,
        unit_of(|b, t| {
            b.begin(NodeKind::IfStatement, 0);
            b.patch(0, tok(source, t, "else", 0));
            b.begin(NodeKind::ParenthesizedCondition, 1);
            b.patch(0, NONE);
            b.patch(1, NONE);
            b.begin(NodeKind::BinaryExpression, 2);
            b.patch(0, tok(source, t, "==", 0));
            identifier(b, 2, 3);
            b.leaf(NodeKind::LiteralNull, 4);
            b.end(NodeKind::BinaryExpression, 5);
            b.end(NodeKind::ParenthesizedCondition, 6);
            b.begin(NodeKind::ExpressionStatement, 6);
            b.begin(NodeKind::Assignment, 6);
            b.patch(0, tok(source, t, "=", 0));
            identifier(b, 6, 7);
            b.leaf(NodeKind::LiteralInt, 8);
            b.end(NodeKind::Assignment, 9);
            b.end(NodeKind::ExpressionStatement, 10);
            b.leaf(NodeKind::EmptyStatement, 11);
            b.end(NodeKind::IfStatement, 12);
        }),
    );
    assert!(f.run(&one_rule("prefer_conditional_assignment")).is_empty());
}

/// A non-assignment then arm passes.
#[test]
fn conditional_assignment_other_then_passes() {
    let source = "if (x == null) y = 1;";
    let f = Fixture::build(
        source,
        unit_of(|b, t| {
            b.begin(NodeKind::IfStatement, 0);
            b.patch(0, NONE);
            b.begin(NodeKind::ParenthesizedCondition, 1);
            b.patch(0, NONE);
            b.patch(1, NONE);
            b.begin(NodeKind::BinaryExpression, 2);
            b.patch(0, tok(source, t, "==", 0));
            identifier(b, 2, 3);
            b.leaf(NodeKind::LiteralNull, 4);
            b.end(NodeKind::BinaryExpression, 5);
            b.end(NodeKind::ParenthesizedCondition, 6);
            b.begin(NodeKind::ExpressionStatement, 6);
            b.begin(NodeKind::Assignment, 6);
            b.patch(0, tok(source, t, "=", 0));
            identifier(b, 6, 7);
            b.leaf(NodeKind::LiteralInt, 8);
            b.end(NodeKind::Assignment, 9);
            b.end(NodeKind::ExpressionStatement, 10);
            b.end(NodeKind::IfStatement, 10);
        }),
    );
    assert!(f.run(&one_rule("prefer_conditional_assignment")).is_empty());
}

// —— prefer_if_null_operators ————————————————————————————————————————

/// `test/rules/prefer_if_null_operators_test.dart` `test_usedElse`.
#[test]
fn if_null_reports_the_else_mirror() {
    let source = "a == null ? 1 : a;";
    // a0 ==1 null2 ?3 14 :5 a6 ;7
    let f = Fixture::build(
        source,
        unit_of(|b, t| {
            b.begin(NodeKind::ExpressionStatement, 0);
            b.begin(NodeKind::Conditional, 3);
            b.patch(0, tok(source, t, "?", 0));
            b.patch(1, tok(source, t, ":", 0));
            b.begin(NodeKind::BinaryExpression, 0);
            b.patch(0, tok(source, t, "==", 0));
            identifier(b, 0, 1);
            b.leaf(NodeKind::LiteralNull, 2);
            b.end(NodeKind::BinaryExpression, 3);
            b.leaf(NodeKind::LiteralInt, 4);
            identifier(b, 6, 7);
            b.end(NodeKind::Conditional, 7);
            b.end(NodeKind::ExpressionStatement, 8);
        }),
    );
    let lints = f.run(&one_rule("prefer_if_null_operators"));
    assert_eq!(lints.len(), 1);
    assert_eq!(
        lints[0].message,
        "Use the '??' operator rather than '?:' when testing for 'null'."
    );
}

/// The `!=` form tests the then arm (`test_usedThen`).
#[test]
fn if_null_reports_the_then_mirror() {
    let source = "a != null ? a : 1;";
    let f = Fixture::build(
        source,
        unit_of(|b, t| {
            b.begin(NodeKind::ExpressionStatement, 0);
            b.begin(NodeKind::Conditional, 3);
            b.patch(0, tok(source, t, "?", 0));
            b.patch(1, tok(source, t, ":", 0));
            b.begin(NodeKind::BinaryExpression, 0);
            b.patch(0, tok(source, t, "!=", 0));
            identifier(b, 0, 1);
            b.leaf(NodeKind::LiteralNull, 2);
            b.end(NodeKind::BinaryExpression, 3);
            identifier(b, 4, 5);
            b.leaf(NodeKind::LiteralInt, 6);
            b.end(NodeKind::Conditional, 7);
            b.end(NodeKind::ExpressionStatement, 8);
        }),
    );
    let lints = f.run(&one_rule("prefer_if_null_operators"));
    assert_eq!(lints.len(), 1);
}

/// Divergent arms pass (`test_different`).
#[test]
fn if_null_divergent_arms_pass() {
    let source = "a == null ? 1 : b;";
    let f = Fixture::build(
        source,
        unit_of(|b, t| {
            b.begin(NodeKind::ExpressionStatement, 0);
            b.begin(NodeKind::Conditional, 3);
            b.patch(0, tok(source, t, "?", 0));
            b.patch(1, tok(source, t, ":", 0));
            b.begin(NodeKind::BinaryExpression, 0);
            b.patch(0, tok(source, t, "==", 0));
            identifier(b, 0, 1);
            b.leaf(NodeKind::LiteralNull, 2);
            b.end(NodeKind::BinaryExpression, 3);
            b.leaf(NodeKind::LiteralInt, 4);
            identifier(b, 6, 7);
            b.end(NodeKind::Conditional, 7);
            b.end(NodeKind::ExpressionStatement, 8);
        }),
    );
    assert!(f.run(&one_rule("prefer_if_null_operators")).is_empty());
}

/// Production pins: the bare report shapes through the real bridge —
/// `a == null ? 1 : a` reports at the whole conditional.
#[test]
fn if_null_production_else_form_reports() {
    let f = Fixture::parse("void f() {\n  a == null ? 1 : a;\n}\n");
    let lints = f.run(&one_rule("prefer_if_null_operators"));
    assert_eq!(lints.len(), 1);
    assert_eq!(
        lints[0].message,
        "Use the '??' operator rather than '?:' when testing for 'null'."
    );
}

/// The `!=` production mirror tests the then arm.
#[test]
fn if_null_production_then_form_reports() {
    let f = Fixture::parse("void f() {\n  a != null ? a : 1;\n}\n");
    assert_eq!(f.run(&one_rule("prefer_if_null_operators")).len(), 1);
}

/// The corpus face: a null-check conditional as a local initializer
/// with string arms reports at the whole conditional expression —
/// `label == null ? 'none' : label` (30 chars) from the condition
/// head, not the `?` the production row opens at.
#[test]
fn if_null_production_initializer_form_reports() {
    let f = Fixture::parse(
        "void f(String? label) {\n  var merged = label == null ? 'none' : label;\n  print(merged);\n}\n",
    );
    let lints = f.run(&one_rule("prefer_if_null_operators"));
    assert_eq!(lints.len(), 1);
    let label = tok(&f.source, &f.tokens, "label", 1);
    assert_eq!(lints[0].range.start, utf16_at(&f, label));
    assert_eq!(lints[0].range.end - lints[0].range.start, 30);
}

/// The expression-sibling bound: a null-check conditional nested as a
/// later argument/element keeps its left operand to its own pieces —
/// the leading sibling rows end at the `,`/`(` separators and never
/// join the operand run.
#[test]
fn if_null_production_expression_sibling_forms() {
    for source in [
        "void f(int? x, int a) {\n  f(0, x == null ? a : x);\n}\n",
        "void f(int? x, int a) {\n  var v = [0, x == null ? a : x];\n}\n",
        "int g({int? w, int? y}) => 0;\nvoid f(int? x, int a) {\n  g(w: 0, y: x == null ? a : x);\n}\n",
    ] {
        let lints = Fixture::parse(source).run(&one_rule("prefer_if_null_operators"));
        assert_eq!(lints.len(), 1, "{source}");
        assert_eq!(
            lints[0].message,
            "Use the '??' operator rather than '?:' when testing for 'null'."
        );
    }
}

/// The receiver chain of a member null check tiles below the binary's
/// adopted window (the covered span opens at the selector) and must
/// join the operand run — `a.b == null ? c : a.b` reports, in a
/// statement and in an argument position.
#[test]
fn if_null_production_receiver_chain_forms() {
    for source in [
        "class C { int? b; }\nvoid f(C a, int c) {\n  var v = a.b == null ? c : a.b;\n}\n",
        "class C { int? b; }\nint g(int i) => i;\nvoid f(C a, int c) {\n  g(a.b == null ? c : a.b);\n}\n",
    ] {
        assert_eq!(
            Fixture::parse(source)
                .run(&one_rule("prefer_if_null_operators"))
                .len(),
            1,
            "{source}"
        );
    }
}

/// Earlier statements' pieces still drop at the statement run — the
/// adjacency bound never re-admits a completed statement.
#[test]
fn if_null_production_statement_boundary_holds() {
    let f = Fixture::parse("void f(int? x, int a) {\n  foo();\n  x == null ? a : x;\n}\n");
    assert_eq!(f.run(&one_rule("prefer_if_null_operators")).len(), 1);
}

/// Range-end riders: the conditional's covered end can swallow the
/// enclosing terminator or step over an arm's own closer — the report
/// must end at the last arm's real token.
#[test]
fn if_null_production_range_end_riders() {
    // `f()`'s own `)` is the arm's closer, not a swallowed follower —
    // stepping back over it would crop the range into `f(`.
    let f =
        Fixture::parse("int g() => 0;\nvoid f(int? x) {\n  var v = x == null ? g() : x;\n}\n");
    let lints = f.run(&one_rule("prefer_if_null_operators"));
    assert_eq!(lints.len(), 1);
    let x = tok(&f.source, &f.tokens, "x", 2);
    assert_eq!(lints[0].range.end, utf16_at(&f, x) + 1, "end at else `x`");

    // A nested conditional's outer `:` rides the inner row's covered
    // span — the end must step back over it to the inner else `x`.
    let f = Fixture::parse(
        "void f(bool b, int? x, int a, int c) {\n  var v = b ? x == null ? a : x : c;\n}\n",
    );
    let lints = f.run(&one_rule("prefer_if_null_operators"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    let x = tok(&f.source, &f.tokens, "x", 2);
    assert_eq!(
        lints[0].range.end,
        utf16_at(&f, x) + 1,
        "end at inner else `x`"
    );

    // The else's own closer rides inside the covered span too —
    // `h(g() == null ? a : g())` ends `))` and the step-back must stop
    // at the arm's own `)`, not crop into `g(`.
    let f = Fixture::parse(
        "int g() => 0;\nint h(int i) => i;\nvoid f(int a) {\n  h(g() == null ? a : g());\n}\n",
    );
    let lints = f.run(&one_rule("prefer_if_null_operators"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    let close = tok(&f.source, &f.tokens, ")", 4);
    assert_eq!(
        lints[0].range.end,
        utf16_at(&f, close) + 1,
        "end at the arm's own `)`"
    );
}

/// Report-start pins: the condition binary's covered span opens at its
/// delta-tile anchor (a member operand's selector, a postfix `!`'s own
/// token — never the operand head), so the report opens at the
/// operand run's earliest head. `a.b == null ? c : a.b` opens at `a`,
/// not at `b`; `x! == null ? a : x` opens at `x`, not at `!`.
#[test]
fn if_null_production_report_start_riders() {
    let f = Fixture::parse(
        "class C { int? b; }\nvoid f(C a, int c) {\n  var v = a.b == null ? c : a.b;\n}\n",
    );
    let lints = f.run(&one_rule("prefer_if_null_operators"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    let a = tok(&f.source, &f.tokens, "a", 1);
    assert_eq!(lints[0].range.start, utf16_at(&f, a), "start at `a`");

    let f = Fixture::parse("void f(int? x, int a) {\n  var v = x! == null ? a : x!;\n}\n");
    let lints = f.run(&one_rule("prefer_if_null_operators"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    let x = tok(&f.source, &f.tokens, "x", 1);
    assert_eq!(lints[0].range.start, utf16_at(&f, x), "start at `x`");

    // The `null`-first mirror opens at `null`, not at the operand.
    let f = Fixture::parse("void f(int? x, int a) {\n  var v = null == x ? a : x;\n}\n");
    let lints = f.run(&one_rule("prefer_if_null_operators"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    let null_tok = tok(&f.source, &f.tokens, "null", 0);
    assert_eq!(
        lints[0].range.start,
        utf16_at(&f, null_tok),
        "start at `null`"
    );
}

/// prefer_null_aware_operators shares the same report-start anchor: the
/// conditional reports from the condition operand's head, not from the
/// binary's covered start.
#[test]
fn null_aware_production_report_start_rider() {
    let f = Fixture::parse("void f() {\n  a.b == null ? null : a.b.c;\n}\n");
    let lints = f.run(&one_rule("prefer_null_aware_operators"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    let a = tok(&f.source, &f.tokens, "a", 0);
    assert_eq!(lints[0].range.start, utf16_at(&f, a), "start at `a`");
}

/// The whole-container faces report from the left operand's first
/// token: `x ??= null` opens at `x` (unnecessary_null_aware_assignments)
/// and `a.b + dyn` opens at `a` — never at the operator token the
/// container row anchors on.
#[test]
fn container_rows_report_from_the_operand_head() {
    let f = Fixture::parse("void f(int? x) {\n  x ??= null;\n}\n");
    let lints = f.run(&one_rule("unnecessary_null_aware_assignments"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    let x = tok(&f.source, &f.tokens, "x", 1);
    assert_eq!(lints[0].range.start, utf16_at(&f, x), "start at `x`");
    let nul = tok(&f.source, &f.tokens, "null", 0);
    assert_eq!(
        lints[0].range.end,
        utf16_at(&f, nul) + 4,
        "end at `null`, not the swallowed `;`"
    );
}

// —— prefer_null_aware_operators —————————————————————————————————————

/// `x == null ? null : x.b` reports (`test_prefixedIdentifier` family):
/// one peel of the result run reaches the condition operand.
#[test]
fn null_aware_reports_the_access_form() {
    let f = Fixture::parse("void f() {\n  a.b == null ? null : a.b.c;\n}\n");
    let lints = f.run(&one_rule("prefer_null_aware_operators"));
    assert_eq!(lints.len(), 1);
    assert_eq!(
        lints[0].message,
        "Use the null-aware operator '?.' rather than an explicit 'null' comparison."
    );
}

/// A result with no peelable access passes (`a == null ? null : a` —
/// the comparison runs only after a peel).
#[test]
fn null_aware_bare_identifier_passes() {
    let f = Fixture::parse("void f() {\n  a == null ? null : a;\n}\n");
    assert!(f.run(&one_rule("prefer_null_aware_operators")).is_empty());
}

/// The `!=` mirror reports on the then arm (`test_notEqual`). The
/// official walk peels the result before the comparison, so the
/// matching shape is `a != null ? a.b : null` — `a.b != null ? a.b :
/// null` peels `a.b` to `a` and never matches.
#[test]
fn null_aware_not_equal_reports() {
    let f = Fixture::parse("void f() {\n  a != null ? a.b : null;\n}\n");
    assert_eq!(f.run(&one_rule("prefer_null_aware_operators")).len(), 1);
    let f = Fixture::parse("void f() {\n  a.b != null ? a.b : null;\n}\n");
    assert!(f.run(&one_rule("prefer_null_aware_operators")).is_empty());
}

/// The corpus face: `user == null ? null : user.handle` as a local
/// initializer — one peel reaches the operand; the report covers the
/// whole conditional (33 chars) from the condition head.
#[test]
fn null_aware_production_member_result_reports() {
    let f = Fixture::parse(
        "class P {\n  String handle = '';\n}\nvoid f(P? user) {\n  var picked = user == null ? null : user.handle;\n  print(picked);\n}\n",
    );
    let lints = f.run(&one_rule("prefer_null_aware_operators"));
    assert_eq!(lints.len(), 1);
    let user = tok(&f.source, &f.tokens, "user", 1);
    assert_eq!(lints[0].range.start, utf16_at(&f, user));
    assert_eq!(lints[0].range.end - lints[0].range.start, 33);
}

// —— unnecessary_null_aware_assignments ———————————————————————————————

/// `test/rules/unnecessary_null_aware_assignments_test.dart` — `x ??=
/// null` reports at the assignment.
#[test]
fn null_aware_assignment_reports() {
    let source = "x ??= null;";
    // x0 ??=1 null2 ;3
    let f = Fixture::build(
        source,
        unit_of(|b, _t| {
            b.begin(NodeKind::ExpressionStatement, 0);
            b.begin(NodeKind::Assignment, 0);
            b.patch(0, 1);
            identifier(b, 0, 1);
            b.leaf(NodeKind::LiteralNull, 2);
            b.end(NodeKind::Assignment, 4);
            b.end(NodeKind::ExpressionStatement, 4);
        }),
    );
    let lints = f.run(&one_rule("unnecessary_null_aware_assignments"));
    assert_eq!(lints.len(), 1);
    assert_eq!(lints[0].message, "Unnecessary assignment of 'null'.");
    assert_eq!(
        lints[0].correction,
        Some("Try removing the assignment.".to_owned())
    );
    assert_eq!(lints[0].range.start, utf16_at(&f, 0));
}

/// A property target (`a.b ??= null`) and a non-null RHS pass.
#[test]
fn null_aware_property_and_non_null_pass() {
    let property = "a.b ??= null;";
    // a0 .1 b2 ??=3 null4 ;5
    let f = Fixture::build(
        property,
        unit_of(|b, _t| {
            b.begin(NodeKind::ExpressionStatement, 0);
            b.begin(NodeKind::Assignment, 0);
            b.patch(0, 3);
            b.begin(NodeKind::DotAccess, 0);
            b.patch(0, 1);
            b.patch(1, 0);
            identifier(b, 0, 1);
            identifier(b, 2, 3);
            b.end(NodeKind::DotAccess, 4);
            b.leaf(NodeKind::LiteralNull, 4);
            b.end(NodeKind::Assignment, 6);
            b.end(NodeKind::ExpressionStatement, 6);
        }),
    );
    assert!(
        f.run(&one_rule("unnecessary_null_aware_assignments"))
            .is_empty()
    );

    let int_rhs = "x ??= 1;";
    // x0 ??=1 12 ;3
    let f = Fixture::build(
        int_rhs,
        unit_of(|b, _t| {
            b.begin(NodeKind::ExpressionStatement, 0);
            b.begin(NodeKind::Assignment, 0);
            b.patch(0, 1);
            identifier(b, 0, 1);
            b.leaf(NodeKind::LiteralInt, 2);
            b.end(NodeKind::Assignment, 4);
            b.end(NodeKind::ExpressionStatement, 4);
        }),
    );
    assert!(
        f.run(&one_rule("unnecessary_null_aware_assignments"))
            .is_empty()
    );
}

// —— prefer_contains ——————————————————————————————————————————————————

/// `test/rules/prefer_contains_test.dart` `test_contains` — `indexOf ==
/// -1` reports with the rewrite template. Production-bridge fixture.
#[test]
fn contains_reports_index_of_equals_minus_one() {
    let f = Fixture::parse("void f(List<int> xs) { xs.indexOf(1) == -1; }");
    let lints = f.run(&one_rule("prefer_contains"));
    assert_eq!(lints.len(), 1);
    assert_eq!(
        lints[0].message,
        "Unnecessary use of 'indexOf' to test for containment."
    );
    assert_eq!(
        lints[0].correction,
        Some("Try using 'contains'.".to_owned())
    );
    // The operand occurrence: "xs"#0 is the formal parameter's token.
    assert_eq!(
        lints[0].range.start,
        utf16_at(&f, tok(&f.source, &f.tokens, "xs", 1))
    );
}

/// The always-false/always-true judgments carry their own texts and no
/// correction; `>= 0` rewrites.
#[test]
fn contains_message_matrix() {
    let always_false = Fixture::parse("void f(List<int> xs) { xs.indexOf(1) < -1; }");
    let lints = always_false.run(&one_rule("prefer_contains"));
    assert_eq!(lints.len(), 1);
    assert_eq!(
        lints[0].message,
        "Always 'false' because 'indexOf' is always greater than or equal to -1."
    );
    assert_eq!(lints[0].correction, None);

    let always_true = Fixture::parse("void f(List<int> xs) { xs.indexOf(1) >= -1; }");
    let lints = always_true.run(&one_rule("prefer_contains"));
    assert_eq!(lints.len(), 1);
    assert_eq!(
        lints[0].message,
        "Always 'true' because 'indexOf' is always greater than or equal to -1."
    );

    let rewrite = Fixture::parse("void f(List<int> xs) { xs.indexOf(1) >= 0; }");
    let lints = rewrite.run(&one_rule("prefer_contains"));
    assert_eq!(lints.len(), 1);
    assert_eq!(
        lints[0].message,
        "Unnecessary use of 'indexOf' to test for containment."
    );
}

/// A left-hand literal compares with the inverted operator.
#[test]
fn contains_left_literal_inverts_the_operator() {
    let f = Fixture::parse("void f(List<int> xs) { -1 != xs.indexOf(1); }");
    let lints = f.run(&one_rule("prefer_contains"));
    assert_eq!(lints.len(), 1);
    assert_eq!(
        lints[0].message,
        "Unnecessary use of 'indexOf' to test for containment."
    );
}

/// An assigned `indexOf`, a bare untargeted invocation, and a non-zero
/// start index pass (`_isUnassignedIndexOf`, :144-171).
#[test]
fn contains_assigned_and_bare_invocations_pass() {
    let f = Fixture::parse("void f(List<int> xs) { (next = xs.indexOf('{')) != -1; }");
    assert!(f.run(&one_rule("prefer_contains")).is_empty());

    let f = Fixture::parse("void f() { var e = 1; indexOf(e) == -1; }");
    assert!(f.run(&one_rule("prefer_contains")).is_empty());

    let f = Fixture::parse("void f(List<int> xs) { xs.indexOf(1, 1) == -1; }");
    assert!(f.run(&one_rule("prefer_contains")).is_empty());
}

/// The restored receiver face (:145-149): a string literal receiver
/// reports (the `String` half), a map literal receiver stays silent
/// (`Map` implements neither `Iterable` nor `String` — the recon's
/// receiver-check divergence, closed).
#[test]
fn contains_receiver_face_decides_literal_receivers() {
    let f = Fixture::parse("void f() { var x = 'abc'.indexOf('a') != -1; }");
    assert_eq!(f.run(&one_rule("prefer_contains")).len(), 1, "String face");

    let f = Fixture::parse("void f() { var x = ({'k': 1}).indexOf('a') == -1; }");
    assert!(
        f.run(&one_rule("prefer_contains")).is_empty(),
        "a map literal is outside [Iterable, String]"
    );

    // A typed-parameter receiver resolves into the family.
    let f = Fixture::parse("void f(List<int> xs) { xs.indexOf(1) == -1; }");
    assert_eq!(f.run(&one_rule("prefer_contains")).len(), 1);
}

/// The typed-receiver gate (:143-149): a receiver without a same-file
/// verdict — an untyped parameter, a user-class parameter, a same-file
/// class owning the written `List` name — stays silent like the
/// official `parentType == null`/interface-miss faces (the converted
/// dartq-only over-report; the oracle probe confirms `g(x)`/`h(User)`
/// stay silent and `f(List<int>)` reports).
#[test]
fn contains_unresolved_receivers_stay_silent() {
    let f = Fixture::parse("void g(x) { if (x.indexOf(1) == -1) { print(0); } }");
    assert!(f.run(&one_rule("prefer_contains")).is_empty());

    let f = Fixture::parse(
        r"
class User {
  int indexOf(Object e, [int s = 0]) => 0;
}

void h(User u) {
  if (u.indexOf(1) == -1) {
    print(0);
  }
}
",
    );
    assert!(f.run(&one_rule("prefer_contains")).is_empty());

    let f = Fixture::parse(
        r"
class List {}

void k(List<int> xs) {
  if (xs.indexOf(1) == -1) {
    print(0);
  }
}
",
    );
    assert!(f.run(&one_rule("prefer_contains")).is_empty());
}

// —— unnecessary_const ————————————————————————————————————————————————

/// The official `constantContext` face: a const-marked literal under a
/// const variable declaration reports (`test_constConstructor` family —
/// `const c = [!const!] C()`). The declaration sits at the top level, so
/// the parser tiles the `VariablesDeclaration` under `TopLevelFields`
/// (the member face — the declaration row heads at the `const` keyword
/// itself, `variables_decl_is_const`'s member arm).
#[test]
fn unnecessary_const_reports_inside_const_declaration() {
    let source = "const x = [const A()];";
    // const0 x1 =2 [3 const4 A5 (6 )7 ]8 ;9
    let f = Fixture::build(
        source,
        unit_of(|b, _t| {
            b.begin(NodeKind::TopLevelFields, 0);
            b.begin(NodeKind::VariablesDeclaration, 0);
            b.patch(0, NONE);
            b.begin(NodeKind::InitializedIdentifier, 1);
            identifier(b, 1, 2);
            b.begin(NodeKind::VariableInitializer, 2);
            b.begin(NodeKind::ListLiteral, 3);
            b.patch(0, NONE);
            b.begin(NodeKind::ConstExpression, 4);
            identifier(b, 5, 6);
            b.begin(NodeKind::Arguments, 6);
            b.end(NodeKind::Arguments, 8);
            b.end(NodeKind::ConstExpression, 8);
            b.end(NodeKind::ListLiteral, 9);
            b.end(NodeKind::VariableInitializer, 9);
            b.end(NodeKind::InitializedIdentifier, 9);
            b.end(NodeKind::VariablesDeclaration, 10);
            b.end(NodeKind::TopLevelFields, 10);
        }),
    );
    let lints = f.run(&one_rule("unnecessary_const"));
    assert_eq!(lints.len(), 1);
    assert_eq!(lints[0].message, "Unnecessary 'const' keyword.");
    assert_eq!(lints[0].range.start, utf16_at(&f, 4));
    // The production bridge agrees: the same source parsed for real
    // reports identically (the fixture mirrors `endTopLevelFields`'s
    // tiling — `TopLevelFields` wrapping the member declaration).
    let lints = Fixture::parse(source).run(&one_rule("unnecessary_const"));
    assert_eq!(lints.len(), 1);
    assert_eq!(lints[0].range.start, utf16_at(&f, 4));
}

/// A const expression under a non-const declaration stays
/// (`final x = const [1]` — the official `VariableDeclarationList` branch
/// stops the walk without the keyword).
#[test]
fn unnecessary_const_final_declaration_passes() {
    let source = "final x = const [1];";
    // final0 x1 =2 const3 [4 15 ]6 ;7
    let f = Fixture::build(
        source,
        unit_of(|b, _t| {
            b.begin(NodeKind::VariablesDeclaration, 0);
            b.patch(0, NONE);
            b.begin(NodeKind::InitializedIdentifier, 1);
            identifier(b, 1, 2);
            b.begin(NodeKind::VariableInitializer, 2);
            b.begin(NodeKind::ConstExpression, 3);
            b.begin(NodeKind::ListLiteral, 4);
            b.patch(0, NONE);
            b.leaf(NodeKind::LiteralInt, 5);
            b.end(NodeKind::ListLiteral, 7);
            b.end(NodeKind::ConstExpression, 7);
            b.end(NodeKind::VariableInitializer, 7);
            b.end(NodeKind::InitializedIdentifier, 7);
            b.end(NodeKind::VariablesDeclaration, 8);
        }),
    );
    assert!(f.run(&one_rule("unnecessary_const")).is_empty());
}

/// A const-marked literal with no const ancestor stays
/// (`final x = const [1]` again exercises the list-literal dispatch
/// through the payload keyword).
#[test]
fn unnecessary_const_list_literal_keyword_face() {
    let source = "final x = const [1];";
    let f = Fixture::build(
        source,
        unit_of(|b, t| {
            b.begin(NodeKind::VariablesDeclaration, 0);
            b.patch(0, NONE);
            b.begin(NodeKind::InitializedIdentifier, 1);
            identifier(b, 1, 2);
            b.begin(NodeKind::VariableInitializer, 2);
            b.begin(NodeKind::ListLiteral, 3);
            b.patch(0, tok(source, t, "const", 0));
            b.leaf(NodeKind::LiteralInt, 5);
            b.end(NodeKind::ListLiteral, 7);
            b.end(NodeKind::VariableInitializer, 7);
            b.end(NodeKind::InitializedIdentifier, 7);
            b.end(NodeKind::VariablesDeclaration, 8);
        }),
    );
    assert!(f.run(&one_rule("unnecessary_const")).is_empty());
}

// —— prefer_is_empty ———————————————————————————————————————————————————

/// `test/rules/prefer_is_empty_test.dart` `test_prefersIsEmptyOverLength0`
/// — `x.length == 0` reports with the isEmpty rewrite. Production-bridge
/// fixture.
#[test]
fn prefer_is_empty_reports_length_eq_zero() {
    let f = Fixture::parse("void f(List<int> xs) { xs.length == 0; }");
    let lints = f.run(&one_rule("prefer_is_empty"));
    assert_eq!(lints.len(), 1);
    assert_eq!(
        lints[0].message,
        "Use 'isEmpty' instead of 'length' to test whether the collection is empty."
    );
    assert_eq!(
        lints[0].correction,
        Some("Try rewriting the expression to use 'isEmpty'.".to_owned())
    );
    assert_eq!(
        lints[0].range.start,
        // The operand occurrence: "xs"#0 is the formal parameter's token.
        utf16_at(&f, tok(&f.source, &f.tokens, "xs", 1))
    );
}

/// A value outside the matrix passes (`== 2`); the always-true face
/// carries its own text and no correction (`x.length >= -1`).
#[test]
fn prefer_is_empty_matrix_faces() {
    let f = Fixture::parse("void f(List<int> xs) { xs.length == 2; }");
    assert!(f.run(&one_rule("prefer_is_empty")).is_empty());

    let f = Fixture::parse("void f(List<int> xs) { xs.length >= -1; }");
    let lints = f.run(&one_rule("prefer_is_empty"));
    assert_eq!(lints.len(), 1);
    assert_eq!(
        lints[0].message,
        "The comparison is always 'true' because the length is always greater than or equal to 0."
    );
    assert_eq!(lints[0].correction, None);
}

/// The restored receiver face (:229-234): a record literal is provably
/// outside `Iterable`/`Map`/`String` and passes (the recon's
/// receiver-check divergence, closed), while a map literal reports like
/// the official `Map` face and an opaque receiver keeps the presumption.
#[test]
fn prefer_is_empty_receiver_face_decides_literal_receivers() {
    let f = Fixture::parse("void f() { if ((1,).length >= 1) {} }");
    assert!(
        f.run(&one_rule("prefer_is_empty")).is_empty(),
        "a record is not Iterable/Map/String"
    );

    let f = Fixture::parse("void f() { if (({'k': 1}).length >= 1) {} }");
    assert_eq!(f.run(&one_rule("prefer_is_empty")).len(), 1, "Map face");

    let f = Fixture::parse("void f(List<int> xs) { if (xs.length >= 1) {} }");
    assert_eq!(f.run(&one_rule("prefer_is_empty")).len(), 1);
}

// —— prefer_is_not_empty ———————————————————————————————————————————————

/// `test/rules/prefer_is_not_empty_test.dart` `test_notIsEmpty` — the
/// negated `isEmpty` reports at the prefix. Production-bridge fixture.
#[test]
fn prefer_is_not_empty_reports_bang_is_empty() {
    let f = Fixture::parse("void f(List<int> xs) { if (!xs.isEmpty) {} }");
    let lints = f.run(&one_rule("prefer_is_not_empty"));
    assert_eq!(lints.len(), 1);
    assert_eq!(
        lints[0].message,
        "Use 'isNotEmpty' rather than negating the result of 'isEmpty'."
    );
    let bang = tok(&f.source, &f.tokens, "!", 0);
    assert_eq!(lints[0].range.start, utf16_at(&f, bang));
    let is_empty = tok(&f.source, &f.tokens, "isEmpty", 0);
    assert_eq!(lints[0].range.end, utf16_at(&f, is_empty) + 7);
}

/// A negated `isNotEmpty` (or any other property) passes.
#[test]
fn prefer_is_not_empty_other_property_passes() {
    let f = Fixture::parse("void f(List<int> xs) { if (!xs.isNotEmpty) {} }");
    assert!(f.run(&one_rule("prefer_is_not_empty")).is_empty());

    let f = Fixture::parse("void f(bool a, bool b) { if (!(a || b)) {} }");
    assert!(f.run(&one_rule("prefer_is_not_empty")).is_empty());
}

/// A string-literal receiver carries `isNotEmpty` on `String` and
/// reports through the face query (:57-62).
#[test]
fn prefer_is_not_empty_literal_receiver_reports() {
    let f = Fixture::parse("void f() { if (!'abc'.isEmpty) {} }");
    assert_eq!(f.run(&one_rule("prefer_is_not_empty")).len(), 1);
}

// —— avoid_function_literals_in_foreach_calls ——————————————————————————

/// `test/rules/avoid_function_literals_in_foreach_calls_test.dart`
/// `test forEach with function literal` — the method name reports.
/// Production-bridge fixture.
#[test]
fn foreach_literal_reports_the_method_name() {
    let f = Fixture::parse("void f(List<int> xs) { xs.forEach((e) {}); }");
    let lints = f.run(&one_rule("avoid_function_literals_in_foreach_calls"));
    assert_eq!(lints.len(), 1);
    assert_eq!(
        lints[0].message,
        "Function literals shouldn't be passed to 'forEach'."
    );
    assert_eq!(
        lints[0].correction,
        Some("Try using a 'for' loop.".to_owned())
    );
    let name = tok(&f.source, &f.tokens, "forEach", 0);
    assert_eq!(lints[0].range.start, utf16_at(&f, name));
    assert_eq!(lints[0].range.end, utf16_at(&f, name) + 7);
}

/// The `Map.forEach` face — a two-parameter closure — passes through the
/// arity gate, a cascade section passes through the cascade guard, and a
/// null-aware chain passes through the marker word (:87).
#[test]
fn foreach_two_params_cascade_and_null_aware_pass() {
    let f = Fixture::parse("void f(Map<String, int> map) { map.forEach((k, v) {}); }");
    assert!(
        f.run(&one_rule("avoid_function_literals_in_foreach_calls"))
            .is_empty()
    );

    let f = Fixture::parse("void f(List<int> xs) { xs..forEach((e) {}); }");
    assert!(
        f.run(&one_rule("avoid_function_literals_in_foreach_calls"))
            .is_empty()
    );

    let f = Fixture::parse("void f(List<int> xs) { xs?.forEach((e) {}); }");
    assert!(
        f.run(&one_rule("avoid_function_literals_in_foreach_calls"))
            .is_empty()
    );
}

/// A chained target passes (`_hasMethodChaining`, :31-43), and the
/// restored receiver face keeps a map literal silent like the official
/// `Iterable`-only gate (:40-45).
#[test]
fn foreach_chaining_and_receiver_face_pass() {
    let f = Fixture::parse("void f(List<int> xs, int g(int e)) { xs.map(g).forEach((e) {}); }");
    assert!(
        f.run(&one_rule("avoid_function_literals_in_foreach_calls"))
            .is_empty()
    );

    let f = Fixture::parse("void f() { ({'k': 1}).forEach((e) {}); }");
    assert!(
        f.run(&one_rule("avoid_function_literals_in_foreach_calls"))
            .is_empty(),
        "a map literal is outside Iterable"
    );

    // An opaque receiver keeps the documented presumption.
    let f = Fixture::parse("void f(List<int> xs) { xs.forEach((e) { print(e); }); }");
    assert_eq!(
        f.run(&one_rule("avoid_function_literals_in_foreach_calls"))
            .len(),
        1
    );
}

/// The async-arrow face: the oracle-of-record (3.13.1) reports
/// `[].forEach((e) async => …)` at the method name — the `async` modifier
/// is parser-diet (an `ExpressionBody` payload word, no AST node), so the
/// shape dispatches identically. The corpus face pairs it with earlier
/// statements in the same block: their rows must not leak into the
/// receiver run (an earlier `x.f()` is not method chaining on this
/// invocation, an earlier `?.` is not its null-aware chain).
#[test]
fn foreach_async_arrow_reports_after_earlier_statements() {
    let f =
        Fixture::parse("void f() { [].where((e) => true); [].forEach((e) async => print(e)); }");
    assert_eq!(
        f.run(&one_rule("avoid_function_literals_in_foreach_calls"))
            .len(),
        1
    );

    let f = Fixture::parse(
        "void f() { [].forEach((e) => print(e)); [].forEach((e) async => print(e)); }",
    );
    assert_eq!(
        f.run(&one_rule("avoid_function_literals_in_foreach_calls"))
            .len(),
        2
    );

    let f = Fixture::parse(
        "void f() { final h = (e) => print(e); h(1); [].forEach((e) async => print(e)); }",
    );
    assert_eq!(
        f.run(&one_rule("avoid_function_literals_in_foreach_calls"))
            .len(),
        1
    );

    // An earlier statement's null-aware access does not suppress the
    // invocation's own face.
    let f = Fixture::parse("void f(List<int>? xs) { xs?.length; [].forEach((e) => print(e)); }");
    assert_eq!(
        f.run(&one_rule("avoid_function_literals_in_foreach_calls"))
            .len(),
        1
    );
}

// —— prefer_interpolation_to_compose_strings ———————————————————————————

/// `name + ' (id)'` reports over the pair span. Production-bridge
/// fixture: the bare-name operand tiles as the four-piece tear-off
/// group, so the report span starts at `name`.
#[test]
fn interpolation_reports_literal_plus_value() {
    let f = Fixture::parse("void f(String name) { var s = name + ' (id)'; }");
    let lints = f.run(&one_rule("prefer_interpolation_to_compose_strings"));
    assert_eq!(lints.len(), 1);
    assert_eq!(
        lints[0].message,
        "Use interpolation to compose strings and values."
    );
    // The operand occurrence: "name"#0 is the formal parameter's token.
    let name = tok(&f.source, &f.tokens, "name", 1);
    let raw = tok(&f.source, &f.tokens, "' (id)'", 0);
    assert_eq!(lints[0].range.start, utf16_at(&f, name));
    assert_eq!(lints[0].range.end, utf16_at(&f, raw) + 7);
}

/// Both literals and a raw string pass (:70-75).
#[test]
fn interpolation_both_literals_and_raw_pass() {
    let f = Fixture::parse("void f() { var s = 'a' + 'b'; }");
    assert!(
        f.run(&one_rule("prefer_interpolation_to_compose_strings"))
            .is_empty()
    );

    let f = Fixture::parse("void f() { var s = 'a' + r'b'; }");
    assert!(
        f.run(&one_rule("prefer_interpolation_to_compose_strings"))
            .is_empty()
    );
}

/// The untyped local's literal initializer answers `String` before
/// the `operator +` presumption (the #1135 declaration-side gate):
/// the oracle row at `use_string_buffers.dart` — `var buffer = ''`
/// under a same-file user `+` — reports. A user-typed initializer
/// keeps the presumption silent.
#[test]
fn interpolation_inferred_string_left_operand_reports() {
    let source = "class B {\n  B operator +(B other) => this;\n}\nvoid f() {\n  var buffer = '';\n  buffer = buffer + 'a';\n}\n";
    let f = Fixture::parse(source);
    let lints = f.run(&one_rule("prefer_interpolation_to_compose_strings"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "Use interpolation to compose strings and values."
    );

    let user_typed = "class B {\n  B operator +(B other) => this;\n}\nvoid f() {\n  var b = B();\n  b = b + B();\n}\n";
    let f = Fixture::parse(user_typed);
    assert!(
        f.run(&one_rule("prefer_interpolation_to_compose_strings"))
            .is_empty(),
        "{user_typed}"
    );
}

/// A `toString` invocation with arguments passes (sdk#52610), the chain
/// reports once at the root, and the left-operand String face (:85)
/// decides literal receivers — a string literal reports.
#[test]
fn interpolation_to_string_gate_and_chain_root() {
    let f = Fixture::parse("void f(A a, int x) { var s = a.toString(x) + 'foo'; }");
    assert!(
        f.run(&one_rule("prefer_interpolation_to_compose_strings"))
            .is_empty()
    );

    let f = Fixture::parse("void f(String a, String b) { var s = a + b + 'c'; }");
    assert_eq!(
        f.run(&one_rule("prefer_interpolation_to_compose_strings"))
            .len(),
        1,
        "an opaque chain reports once, at the literal pair"
    );

    let f = Fixture::parse("void f() { var s = 'a' + f(); }");
    assert_eq!(
        f.run(&one_rule("prefer_interpolation_to_compose_strings"))
            .len(),
        1,
        "the left literal is dart:core String"
    );
}

/// #792 (`test_variableNotString`): a same-file user type declaring
/// `operator +` makes the opaque left operand's static type that user
/// type — the written-type face answers for a typed binding, the
/// untyped face presumes the user type, and a written `String`
/// binding still reports.
#[test]
fn interpolation_user_operator_plus_passes() {
    let f = Fixture::parse(
        "class A {\n  A operator +(String other) => this;\n}\n\nvoid f(A a) {\n  a + ' ';\n}\n",
    );
    assert!(
        f.run(&one_rule("prefer_interpolation_to_compose_strings"))
            .is_empty()
    );

    let f = Fixture::parse(
        "class A {\n  A operator +(String other) => this;\n}\n\nvoid f() {\n  var a = A();\n  a + ' ';\n}\n",
    );
    assert!(
        f.run(&one_rule("prefer_interpolation_to_compose_strings"))
            .is_empty()
    );

    let f = Fixture::parse(
        "class A {\n  A operator +(String other) => this;\n}\n\nvoid f(String a) {\n  a + ' ';\n}\n",
    );
    assert_eq!(
        f.run(&one_rule("prefer_interpolation_to_compose_strings"))
            .len(),
        1
    );
}

// —— null_closures ——————————————————————————————————————————————————————

/// `Future(null)` — a `null` in the unnamed-constructor closure slot
/// reports at the null (`test/rules/null_closures_test.dart`
/// `test_Future`).
#[test]
fn null_closures_reports_a_null_future_callback() {
    let f = Fixture::parse("void f() { Future(null); }");
    let lints = f.run(&one_rule("null_closures"));
    assert_eq!(lints.len(), 1);
    assert_eq!(
        lints[0].message,
        "Closure can't be 'null' because it might be invoked."
    );
    assert_eq!(
        lints[0].range.start,
        utf16_at(&f, tok(&f.source, &f.tokens, "null", 0))
    );
}

/// `new Future(null)` reports the same way through the explicit-new
/// shape.
#[test]
fn null_closures_reports_an_explicit_new_creation() {
    let f = Fixture::parse("void f() { new Future(null); }");
    let lints = f.run(&one_rule("null_closures"));
    assert_eq!(lints.len(), 1);
    assert_eq!(
        lints[0].range.start,
        utf16_at(&f, tok(&f.source, &f.tokens, "null", 0))
    );
}

/// `Future.wait(fs, cleanUp: null)` — the named slot reports over the
/// whole named argument (`test_nullFromFuture_wait`).
#[test]
fn null_closures_reports_a_named_null_over_the_argument() {
    let f = Fixture::parse("void f() { Future.wait(fs, cleanUp: null); }");
    let lints = f.run(&one_rule("null_closures"));
    assert_eq!(lints.len(), 1);
    assert_eq!(
        lints[0].range.start,
        utf16_at(&f, tok(&f.source, &f.tokens, "cleanUp", 0))
    );
    let null_start = utf16_at(&f, tok(&f.source, &f.tokens, "null", 0));
    assert_eq!(lints[0].range.end, null_start + 4);
}

/// `Timer.periodic(d, null)` — the named-creation face, positional
/// slot 1 (real bridge shape: the `Qualified` callee).
#[test]
fn null_closures_reports_a_periodic_timer_callback() {
    let f = Fixture::parse("void f() { Timer.periodic(d, null); }");
    let lints = f.run(&one_rule("null_closures"));
    assert_eq!(lints.len(), 1);
    assert_eq!(
        lints[0].range.start,
        utf16_at(&f, tok(&f.source, &f.tokens, "null", 0))
    );
}

/// A non-null closure passes, and a bare `map(null)` never matches —
/// the official bails with a null target (`null_closures.dart:222`).
#[test]
fn null_closures_non_null_and_bare_calls_pass() {
    let f = Fixture::parse("void f(List<int> list, void g(int e)) { list.map(g); }");
    assert!(f.run(&one_rule("null_closures")).is_empty());

    // A bare unknown callee is no creation (`map(null)`).
    let f = Fixture::parse("void f() { map(null); }");
    assert!(f.run(&one_rule("null_closures")).is_empty());
}

/// `Future.doWhile(null)` — the static face, positional slot 0 (the
/// official `Future.doWhile` entry, `null_closures.dart:174`).
#[test]
fn null_closures_reports_a_static_do_while_callback() {
    let f = Fixture::parse("void f() { Future.doWhile(null); }");
    let lints = f.run(&one_rule("null_closures"));
    assert_eq!(lints.len(), 1);
    assert_eq!(
        lints[0].range.start,
        utf16_at(&f, tok(&f.source, &f.tokens, "null", 0))
    );
}

/// A same-file subclass of a core creation type reports through the
/// `extendsClass` chain (extensions.dart:305-353), and a same-file
/// shadow of the core name passes (the creation resolves to the user
/// class, whose element is not the library type).
#[test]
fn null_closures_user_subclass_creation_reports_and_shadow_passes() {
    let f = Fixture::parse("class MyFuture extends Future {}\nvoid f() { MyFuture(null); }");
    assert_eq!(
        f.run(&one_rule("null_closures")).len(),
        1,
        "MyFuture extends Future: the closure slot reports"
    );

    let f = Fixture::parse("class Future {}\nvoid f() { Future(null); }");
    assert!(
        f.run(&one_rule("null_closures")).is_empty(),
        "a same-file `Future` is not the dart:async element"
    );
}

/// The instance face resolves the receiver's written type through the
/// same-file chain: an `Iterable` receiver reports, a `String` receiver
/// stays silent (the official `_getInstanceMethod` finds no `map` on
/// `String`), a user class with its own `map` stays silent, and a user
/// subclass of `List` reports (the `allSupertypes` walk reaches
/// `Iterable.map`).
#[test]
fn null_closures_instance_receiver_type_pinning() {
    let reports = [
        (
            "void f(Iterable<int> xs) { xs.map(null); }",
            "Iterable receiver",
        ),
        (
            "void f(List<int> l) { l.map(null); }",
            "List receiver (supertype walk)",
        ),
        (
            "class MyList extends List {}\nvoid f(MyList l) { l.map(null); }",
            "user subclass of List",
        ),
    ];
    for (source, why) in reports {
        let f = Fixture::parse(source);
        assert_eq!(f.run(&one_rule("null_closures")).len(), 1, "{why}");
    }

    let silent = [
        ("void f(String s) { s.map(null); }", "String has no map"),
        (
            "class Foo { void map(void cb(int e)) {} }\nvoid f(Foo foo) { foo.map(null); }",
            "user class with its own map",
        ),
    ];
    for (source, why) in silent {
        let f = Fixture::parse(source);
        assert!(f.run(&one_rule("null_closures")).is_empty(), "{why}");
    }
}

// —— unnecessary_this ————————————————————————————————————————————————————

/// `this.x` in an instance method over a same-class field reports at
/// the `this` token (`test/rules/unnecessary_this_test.dart`
/// `test_UnnecessaryThis`); the inherited-member face reports the same
/// way (`isNone` — no scope hit). Production-bridge fixtures: the field
/// and method names ride the collapse-map contexts.
#[test]
fn unnecessary_this_reports_a_field_access() {
    let f = Fixture::parse("class C { int x = 0; int f() { return this.x; } }");
    let lints = f.run(&one_rule("unnecessary_this"));
    assert_eq!(lints.len(), 1, "same-class field: the `this.` reports");
    assert_eq!(lints[0].message, "Unnecessary 'this.' qualifier.");
    let this_kw = tok(&f.source, &f.tokens, "this", 0);
    assert_eq!(lints[0].range.start, utf16_at(&f, this_kw));
    assert_eq!(lints[0].range.end - lints[0].range.start, 4);

    // Inherited member — no declaration anywhere in the file.
    let f =
        Fixture::parse("class A { void f() {} }\nclass B extends A { void g() { this.f(); } }");
    assert_eq!(
        f.run(&one_rule("unnecessary_this")).len(),
        1,
        "inherited member: no scope hit, the `this.` reports"
    );
}

/// The corpus face: a getter expression body's `this.<field>` — the
/// access pieces tile flat in the member window beside the
/// childless `ExpressionBody` handle, and the no-import file keeps
/// the scope gates open.
#[test]
fn unnecessary_this_reports_in_a_getter_expression_body() {
    let f = Fixture::parse(
        "class W {\n  int width = 0;\n  int get doubled => this.width * 2;\n}\n",
    );
    let lints = f.run(&one_rule("unnecessary_this"));
    assert_eq!(lints.len(), 1);
    let this_kw = tok(&f.source, &f.tokens, "this", 0);
    assert_eq!(lints[0].range.start, utf16_at(&f, this_kw));
    assert_eq!(lints[0].range.end - lints[0].range.start, 4);
}

/// The import face rides [`Analysis::import_shadows`] (the official
/// `resolveNameInScope` library-scope half, :87-95): an unprefixed
/// import whose target exports the member name makes the `this.`
/// necessary — no report; a prefixed import's target names stay
/// outside the unqualified scope. A fixture mount carries no import
/// semantics, so the face cannot contradict and the unqualified form
/// keeps reporting (the None-degradation path); the combinator
/// filtering lives in the graph face, not the rule.
#[test]
fn unnecessary_this_import_bails() {
    let kit = "void f() {}\nclass Other {}\n";
    let unprefixed = "import 'kit.dart';\nclass A { void f() {} }\nclass B extends A { void g() { this.f(); } }\n";
    assert!(
        run_semantic(unprefixed, "unnecessary_this", Some((kit, None))).is_empty(),
        "the import carries `f` — the qualified form is necessary"
    );

    let prefixed = "import 'kit.dart' as other;\nclass A { void f() {} }\nclass B extends A { void g() { this.f(); } }\n";
    assert_eq!(
        run_semantic(prefixed, "unnecessary_this", Some((kit, Some("other")))).len(),
        1,
        "a prefixed import never enters the unqualified scope"
    );

    let f = Fixture::parse(unprefixed);
    assert_eq!(
        f.run(&one_rule("unnecessary_this")).len(),
        1,
        "no semantic mount: the import face cannot contradict"
    );
}

/// A same-name unit declaration shadows the inherited form (:87-95 —
/// the resolution finds a different element): no report.
#[test]
fn unnecessary_this_top_level_name_bails() {
    let f = Fixture::parse(
        "void f() {}\nclass A { void f() {} }\nclass B extends A { void g() { this.f(); } }",
    );
    assert!(f.run(&one_rule("unnecessary_this")).is_empty());
}

/// A parameter of the enclosing member shadows — the `this.` stays
/// (unnecessary_this.dart:87-95, `isRequestedName` branch).
#[test]
fn unnecessary_this_parameter_shadow_passes() {
    let f = Fixture::parse("class C { int x = 0; int f(int x) { return this.x; } }");
    assert!(
        f.run(&one_rule("unnecessary_this")).is_empty(),
        "the parameter shadows the field — the qualified form is required"
    );
}

/// A local declared before the use in the same block shadows.
#[test]
fn unnecessary_this_local_shadow_passes() {
    let f = Fixture::parse("class C { int x = 0; int f() { int x = 1; return this.x; } }");
    assert!(
        f.run(&one_rule("unnecessary_this")).is_empty(),
        "the local shadows the field"
    );
}

/// The written `this` on a constructor field initializer always reports
/// (unnecessary_this.dart:31-35); the bare `x = 1;` form passes.
#[test]
fn unnecessary_this_reports_an_initializer_this_and_bare_passes() {
    let f = Fixture::parse("class C { int x = 0; C() : this.x = 1; }");
    let lints = f.run(&one_rule("unnecessary_this"));
    assert_eq!(lints.len(), 1);
    assert_eq!(
        lints[0].range.start,
        utf16_at(&f, tok(&f.source, &f.tokens, "this", 0))
    );

    let f = Fixture::parse("class C { int x = 0; C() : x = 1; }");
    assert!(f.run(&one_rule("unnecessary_this")).is_empty());
}

// —— prefer_final_fields ——————————————————————————————————————————————
// Fixtures over the bridge's `Fields` contracts (`Fields` >
// `VariablesDeclaration` > `InitializedIdentifier` > `VariableInitializer`,
// `Constructor` > `FormalParameters`/`Initializers`).

/// `class C { int _x = 0; }` — the candidate fixture; `extra` appends
/// unit-level siblings (mutation statements, more constructors).
fn field_fixture(
    extra: impl FnOnce(&mut dartq_ast::AstBuilder, &dartq_scanner::TokenTable),
) -> Fixture {
    Fixture::build("class C {\n  int _x = 0;\n}\n", |b, t| {
        b.begin(NodeKind::CompilationUnit, 0);
        b.begin(NodeKind::TopLevelDeclaration, 0);
        identifier(b, 1, 2);
        b.begin(NodeKind::ClassDeclaration, 0);
        b.patch(0, 0);
        b.patch(1, NONE);
        b.patch(2, NONE);
        b.patch(3, NONE);
        b.begin(NodeKind::DeclarationBody, 2);
        b.begin(NodeKind::Fields, 2);
        b.patch(0, dartq_parser::MemberKind::NonStaticField as u32);
        b.begin(NodeKind::VariablesDeclaration, 3);
        b.patch(0, NONE);
        b.leaf_span(NodeKind::Type, 3, 4);
        b.begin(NodeKind::InitializedIdentifier, 4);
        identifier(b, 4, 5);
        b.begin(NodeKind::VariableInitializer, 5);
        b.leaf(NodeKind::LiteralInt, 6);
        b.end(NodeKind::VariableInitializer, 7);
        b.end(NodeKind::InitializedIdentifier, 7);
        b.end(NodeKind::VariablesDeclaration, 8);
        b.end(NodeKind::Fields, 8);
        b.end(NodeKind::DeclarationBody, 8);
        b.end(NodeKind::ClassDeclaration, 8);
        b.end(NodeKind::TopLevelDeclaration, 9);
        extra(b, t);
        b.end(NodeKind::CompilationUnit, narrow_u32(t.len()));
    })
}

fn field_hits(f: &Fixture) -> Vec<crate::engine::Lint> {
    f.run(&one_rule("prefer_final_fields"))
}

/// Through the production bridge: a private initialized field never
/// mutated reports at the variable declaration (the candidate walk
/// reads the real `Fields` tiling).
#[test]
fn final_fields_reports_on_real_parse() {
    let f = Fixture::parse("class C {\n  int _x = 0;\n}\n");
    let lints = field_hits(&f);
    assert_eq!(lints.len(), 1);
    assert_eq!(lints[0].message, "The private field _x could be 'final'.");
}

/// An assignment statement over the field name, as a unit-level sibling.
fn assignment_statement(
    b: &mut dartq_ast::AstBuilder,
    name_at: u32,
    eq_at: u32,
    value_at: u32,
) {
    b.begin(NodeKind::ExpressionStatement, name_at);
    b.begin(NodeKind::Assignment, name_at);
    b.patch(0, eq_at);
    identifier(b, name_at, name_at + 1);
    b.leaf(NodeKind::LiteralInt, value_at);
    b.end(NodeKind::Assignment, value_at + 1);
    b.end(NodeKind::ExpressionStatement, value_at + 2);
}

/// An initialized private field reports on its declaration
/// (`prefer_final_fields_test.dart` `initialized`).
#[test]
fn initialized_private_field_reports() {
    let f = field_fixture(|_b, _t| {});
    let lints = field_hits(&f);
    assert_eq!(lints.len(), 1);
    assert_eq!(lints[0].message, "The private field _x could be 'final'.");
    assert_eq!(
        lints[0].correction,
        Some("Try making the field 'final'.".to_owned())
    );
    // The report covers the declaration `_x = 0` (bytes 16..22).
    assert_eq!(lints[0].range.start, 16);
    assert_eq!(lints[0].range.end, 22);
}

/// An assignment anywhere marks the field mutated (`mutated`).
#[test]
fn assignment_blocks() {
    // The statement sits past the class: `_x14 =15 116 ;17`.
    let source = "class C {\n  int _x = 0;\n}\n_x = 1;";
    let f = Fixture::build(source, |b, t| {
        b.begin(NodeKind::CompilationUnit, 0);
        b.begin(NodeKind::TopLevelDeclaration, 0);
        identifier(b, 1, 2);
        b.begin(NodeKind::ClassDeclaration, 0);
        b.patch(0, 0);
        b.patch(1, NONE);
        b.patch(2, NONE);
        b.patch(3, NONE);
        b.begin(NodeKind::DeclarationBody, 2);
        b.begin(NodeKind::Fields, 2);
        b.patch(0, dartq_parser::MemberKind::NonStaticField as u32);
        b.begin(NodeKind::VariablesDeclaration, 3);
        b.patch(0, NONE);
        b.leaf_span(NodeKind::Type, 3, 4);
        b.begin(NodeKind::InitializedIdentifier, 4);
        identifier(b, 4, 5);
        b.begin(NodeKind::VariableInitializer, 5);
        b.leaf(NodeKind::LiteralInt, 6);
        b.end(NodeKind::VariableInitializer, 7);
        b.end(NodeKind::InitializedIdentifier, 7);
        b.end(NodeKind::VariablesDeclaration, 8);
        b.end(NodeKind::Fields, 8);
        b.end(NodeKind::DeclarationBody, 8);
        b.end(NodeKind::ClassDeclaration, 8);
        b.end(NodeKind::TopLevelDeclaration, 9);
        assignment_statement(b, 9, 10, 11);
        b.end(NodeKind::CompilationUnit, narrow_u32(t.len()));
    });
    assert!(field_hits(&f).is_empty());
}

/// `++` marks the field mutated too (`incrementAndDecrement`) — the
/// prefix operator parses before its operand.
#[test]
fn increment_blocks() {
    // c0..{8 | ++9 _x10 ;11
    let f = Fixture::build("class C {\n  int _x = 0;\n}\n++_x;", |b, t| {
        b.begin(NodeKind::CompilationUnit, 0);
        b.begin(NodeKind::TopLevelDeclaration, 0);
        identifier(b, 1, 2);
        b.begin(NodeKind::ClassDeclaration, 0);
        b.patch(0, 0);
        b.patch(1, NONE);
        b.patch(2, NONE);
        b.patch(3, NONE);
        b.begin(NodeKind::DeclarationBody, 2);
        b.begin(NodeKind::Fields, 2);
        b.patch(0, dartq_parser::MemberKind::NonStaticField as u32);
        b.begin(NodeKind::VariablesDeclaration, 3);
        b.patch(0, NONE);
        b.leaf_span(NodeKind::Type, 3, 4);
        b.begin(NodeKind::InitializedIdentifier, 4);
        identifier(b, 4, 5);
        b.begin(NodeKind::VariableInitializer, 5);
        b.leaf(NodeKind::LiteralInt, 6);
        b.end(NodeKind::VariableInitializer, 7);
        b.end(NodeKind::InitializedIdentifier, 7);
        b.end(NodeKind::VariablesDeclaration, 8);
        b.end(NodeKind::Fields, 8);
        b.end(NodeKind::DeclarationBody, 8);
        b.end(NodeKind::ClassDeclaration, 8);
        b.end(NodeKind::TopLevelDeclaration, 9);
        b.begin(NodeKind::ExpressionStatement, 9);
        b.leaf(NodeKind::UnaryPrefix, 9);
        identifier(b, 10, 11);
        b.end(NodeKind::ExpressionStatement, 12);
        b.end(NodeKind::CompilationUnit, narrow_u32(t.len()));
    });
    assert!(field_hits(&f).is_empty());
}

/// A field formal in every constructor counts as setting
/// (`fieldFormalInParameter`).
#[test]
fn field_formal_set_reports() {
    // c0 l1 a2 s3 s4 ␣5 C6 {7 int8 _x9 ;10 C11 (12 this13 .14 _x15 )16 ;17 }18
    let source = "class C {\n  int _x;\n  C(this._x);\n}\n";
    let f = Fixture::build(source, |b, t| {
        b.begin(NodeKind::CompilationUnit, 0);
        b.begin(NodeKind::TopLevelDeclaration, 0);
        identifier(b, 1, 2);
        b.begin(NodeKind::ClassDeclaration, 0);
        b.patch(0, 0);
        b.patch(1, NONE);
        b.patch(2, NONE);
        b.patch(3, NONE);
        b.begin(NodeKind::DeclarationBody, 2);
        b.begin(NodeKind::Fields, 2);
        b.patch(0, dartq_parser::MemberKind::NonStaticField as u32);
        b.begin(NodeKind::VariablesDeclaration, 3);
        b.patch(0, NONE);
        b.leaf_span(NodeKind::Type, 3, 4);
        b.begin(NodeKind::InitializedIdentifier, 4);
        identifier(b, 4, 5);
        b.end(NodeKind::InitializedIdentifier, 5);
        b.end(NodeKind::VariablesDeclaration, 6);
        b.end(NodeKind::Fields, 6);
        b.begin(NodeKind::Constructor, 6);
        b.patch(0, dartq_parser::MemberKind::NonStaticMethod as u32);
        b.patch(1, NONE);
        b.patch(2, 7);
        b.patch(3, NONE);
        identifier(b, 6, 7);
        b.begin(NodeKind::FormalParameters, 7);
        b.begin(NodeKind::FormalParameter, 8);
        b.patch(0, 8);
        b.patch(1, 10);
        b.patch(2, NONE);
        b.patch(3, NONE);
        b.patch(
            4,
            dartq_parser::FormalParameterKind::RequiredPositional as u32,
        );
        b.leaf(NodeKind::NoType, 10);
        b.end(NodeKind::FormalParameter, 11);
        b.end(NodeKind::FormalParameters, 11);
        b.leaf(NodeKind::EmptyBody, 12);
        b.end(NodeKind::Constructor, 13);
        b.end(NodeKind::DeclarationBody, 13);
        b.end(NodeKind::ClassDeclaration, 14);
        b.end(NodeKind::TopLevelDeclaration, 14);
        b.end(NodeKind::CompilationUnit, narrow_u32(t.len()));
    });
    assert_eq!(field_hits(&f).len(), 1);
}

/// A field initializer in every constructor counts as setting
/// (`fieldInitializer`) — the bare and `this.`-prefixed entry forms.
#[test]
fn field_initializer_set_reports() {
    // Bare form: `C() : _x = 0;` — c0..C6 (7 )8 :9 _x10 =11 012 ;13 }14
    let bare = Fixture::build("class C {\n  int _x;\n  C() : _x = 0;\n}\n", |b, t| {
        b.begin(NodeKind::CompilationUnit, 0);
        b.begin(NodeKind::TopLevelDeclaration, 0);
        identifier(b, 1, 2);
        b.begin(NodeKind::ClassDeclaration, 0);
        b.patch(0, 0);
        b.patch(1, NONE);
        b.patch(2, NONE);
        b.patch(3, NONE);
        b.begin(NodeKind::DeclarationBody, 2);
        b.begin(NodeKind::Fields, 2);
        b.patch(0, dartq_parser::MemberKind::NonStaticField as u32);
        b.begin(NodeKind::VariablesDeclaration, 3);
        b.patch(0, NONE);
        b.leaf_span(NodeKind::Type, 3, 4);
        b.begin(NodeKind::InitializedIdentifier, 4);
        identifier(b, 4, 5);
        b.end(NodeKind::InitializedIdentifier, 5);
        b.end(NodeKind::VariablesDeclaration, 6);
        b.end(NodeKind::Fields, 6);
        b.begin(NodeKind::Constructor, 6);
        b.patch(0, dartq_parser::MemberKind::NonStaticMethod as u32);
        b.patch(1, NONE);
        b.patch(2, 7);
        b.patch(3, 9);
        identifier(b, 6, 7);
        b.begin(NodeKind::FormalParameters, 7);
        b.end(NodeKind::FormalParameters, 8);
        b.begin(NodeKind::Initializers, 9);
        b.begin(NodeKind::Initializer, 10);
        b.begin(NodeKind::FieldInitializer, 11);
        b.patch(0, 11);
        b.leaf(NodeKind::LiteralInt, 12);
        b.end(NodeKind::FieldInitializer, 13);
        b.end(NodeKind::Initializer, 13);
        b.end(NodeKind::Initializers, 13);
        b.leaf(NodeKind::EmptyBody, 13);
        b.end(NodeKind::Constructor, 14);
        b.end(NodeKind::DeclarationBody, 14);
        b.end(NodeKind::ClassDeclaration, 15);
        b.end(NodeKind::TopLevelDeclaration, 15);
        b.end(NodeKind::CompilationUnit, narrow_u32(t.len()));
    });
    assert_eq!(field_hits(&bare).len(), 1);
    // `this.` form: the entry head is the `this` keyword; the name sits
    // two tokens later.
    let this_form =
        Fixture::build("class C {\n  int _x;\n  C() : this._x = 0;\n}\n", |b, t| {
            b.begin(NodeKind::CompilationUnit, 0);
            b.begin(NodeKind::TopLevelDeclaration, 0);
            identifier(b, 1, 2);
            b.begin(NodeKind::ClassDeclaration, 0);
            b.patch(0, 0);
            b.patch(1, NONE);
            b.patch(2, NONE);
            b.patch(3, NONE);
            b.begin(NodeKind::DeclarationBody, 2);
            b.begin(NodeKind::Fields, 2);
            b.patch(0, dartq_parser::MemberKind::NonStaticField as u32);
            b.begin(NodeKind::VariablesDeclaration, 3);
            b.patch(0, NONE);
            b.leaf_span(NodeKind::Type, 3, 4);
            b.begin(NodeKind::InitializedIdentifier, 4);
            identifier(b, 4, 5);
            b.end(NodeKind::InitializedIdentifier, 5);
            b.end(NodeKind::VariablesDeclaration, 6);
            b.end(NodeKind::Fields, 6);
            b.begin(NodeKind::Constructor, 6);
            b.patch(0, dartq_parser::MemberKind::NonStaticMethod as u32);
            b.patch(1, NONE);
            b.patch(2, 7);
            b.patch(3, 9);
            identifier(b, 6, 7);
            b.begin(NodeKind::FormalParameters, 7);
            b.end(NodeKind::FormalParameters, 8);
            b.begin(NodeKind::Initializers, 9);
            b.begin(NodeKind::Initializer, 10);
            b.begin(NodeKind::FieldInitializer, 13);
            b.patch(0, 13);
            b.leaf(NodeKind::LiteralInt, 15);
            b.end(NodeKind::FieldInitializer, 16);
            b.end(NodeKind::Initializer, 16);
            b.end(NodeKind::Initializers, 16);
            b.leaf(NodeKind::EmptyBody, 16);
            b.end(NodeKind::Constructor, 17);
            b.end(NodeKind::DeclarationBody, 17);
            b.end(NodeKind::ClassDeclaration, 18);
            b.end(NodeKind::TopLevelDeclaration, 18);
            b.end(NodeKind::CompilationUnit, narrow_u32(t.len()));
        });
    assert_eq!(field_hits(&this_form).len(), 1);
}

/// Set in some constructors only — never reports (`notPassedInAll`).
#[test]
fn set_in_some_constructors_only_passes() {
    // c0..{7 int8 _x9 ;10 C11 (12 this13 .14 _x15 )16 ;17 C18 .19 n20
    // (21 )22 ;23 }24
    let f = Fixture::build(
        "class C {\n  int _x;\n  C(this._x);\n  C.n();\n}\n",
        |b, t| {
            b.begin(NodeKind::CompilationUnit, 0);
            b.begin(NodeKind::TopLevelDeclaration, 0);
            identifier(b, 1, 2);
            b.begin(NodeKind::ClassDeclaration, 0);
            b.patch(0, 0);
            b.patch(1, NONE);
            b.patch(2, NONE);
            b.patch(3, NONE);
            b.begin(NodeKind::DeclarationBody, 2);
            b.begin(NodeKind::Fields, 2);
            b.patch(0, dartq_parser::MemberKind::NonStaticField as u32);
            b.begin(NodeKind::VariablesDeclaration, 3);
            b.patch(0, NONE);
            b.leaf_span(NodeKind::Type, 3, 4);
            b.begin(NodeKind::InitializedIdentifier, 4);
            identifier(b, 4, 5);
            b.end(NodeKind::InitializedIdentifier, 5);
            b.end(NodeKind::VariablesDeclaration, 6);
            b.end(NodeKind::Fields, 6);
            // The setting constructor: `C(this._x);`.
            b.begin(NodeKind::Constructor, 6);
            b.patch(0, dartq_parser::MemberKind::NonStaticMethod as u32);
            b.patch(1, NONE);
            b.patch(2, 7);
            b.patch(3, NONE);
            identifier(b, 6, 7);
            b.begin(NodeKind::FormalParameters, 7);
            b.begin(NodeKind::FormalParameter, 8);
            b.patch(0, 8);
            b.patch(1, 10);
            b.patch(2, NONE);
            b.patch(3, NONE);
            b.patch(
                4,
                dartq_parser::FormalParameterKind::RequiredPositional as u32,
            );
            b.leaf(NodeKind::NoType, 10);
            b.end(NodeKind::FormalParameter, 11);
            b.end(NodeKind::FormalParameters, 11);
            b.leaf(NodeKind::EmptyBody, 12);
            b.end(NodeKind::Constructor, 13);
            // The unset named constructor: `C.n();`.
            b.begin(NodeKind::Constructor, 13);
            b.patch(0, dartq_parser::MemberKind::NonStaticMethod as u32);
            b.patch(1, 15);
            b.patch(2, 16);
            b.patch(3, NONE);
            identifier(b, 13, 14);
            b.begin(NodeKind::FormalParameters, 16);
            b.end(NodeKind::FormalParameters, 18);
            b.leaf(NodeKind::EmptyBody, 19);
            b.end(NodeKind::Constructor, 20);
            b.end(NodeKind::DeclarationBody, 20);
            b.end(NodeKind::ClassDeclaration, 21);
            b.end(NodeKind::TopLevelDeclaration, 21);
            b.end(NodeKind::CompilationUnit, narrow_u32(t.len()));
        },
    );
    assert!(field_hits(&f).is_empty());
}

/// Public fields, `final`/`const` fields, unset unassigned fields, and
/// top-level variables never report.
#[test]
fn non_candidates_pass() {
    // Public name.
    let f = Fixture::build("class C {\n  int x = 0;\n}\n", |b, t| {
        b.begin(NodeKind::CompilationUnit, 0);
        b.begin(NodeKind::TopLevelDeclaration, 0);
        identifier(b, 1, 2);
        b.begin(NodeKind::ClassDeclaration, 0);
        b.patch(0, 0);
        b.patch(1, NONE);
        b.patch(2, NONE);
        b.patch(3, NONE);
        b.begin(NodeKind::DeclarationBody, 2);
        b.begin(NodeKind::Fields, 2);
        b.patch(0, dartq_parser::MemberKind::NonStaticField as u32);
        b.begin(NodeKind::VariablesDeclaration, 3);
        b.patch(0, NONE);
        b.leaf_span(NodeKind::Type, 3, 4);
        b.begin(NodeKind::InitializedIdentifier, 4);
        identifier(b, 4, 5);
        b.begin(NodeKind::VariableInitializer, 5);
        b.leaf(NodeKind::LiteralInt, 6);
        b.end(NodeKind::VariableInitializer, 7);
        b.end(NodeKind::InitializedIdentifier, 7);
        b.end(NodeKind::VariablesDeclaration, 8);
        b.end(NodeKind::Fields, 8);
        b.end(NodeKind::DeclarationBody, 8);
        b.end(NodeKind::ClassDeclaration, 8);
        b.end(NodeKind::TopLevelDeclaration, 9);
        b.end(NodeKind::CompilationUnit, narrow_u32(t.len()));
    });
    assert!(field_hits(&f).is_empty());
    // `final` keyword on the declaration.
    let f = Fixture::build("class C {\n  final int _x = 0;\n}\n", |b, t| {
        b.begin(NodeKind::CompilationUnit, 0);
        b.begin(NodeKind::TopLevelDeclaration, 0);
        identifier(b, 1, 2);
        b.begin(NodeKind::ClassDeclaration, 0);
        b.patch(0, 0);
        b.patch(1, NONE);
        b.patch(2, NONE);
        b.patch(3, NONE);
        b.begin(NodeKind::DeclarationBody, 2);
        b.begin(NodeKind::Fields, 2);
        b.patch(0, dartq_parser::MemberKind::NonStaticField as u32);
        b.begin(NodeKind::VariablesDeclaration, 3);
        b.patch(0, 3);
        b.leaf_span(NodeKind::Type, 4, 5);
        b.begin(NodeKind::InitializedIdentifier, 5);
        identifier(b, 5, 6);
        b.begin(NodeKind::VariableInitializer, 6);
        b.leaf(NodeKind::LiteralInt, 7);
        b.end(NodeKind::VariableInitializer, 8);
        b.end(NodeKind::InitializedIdentifier, 8);
        b.end(NodeKind::VariablesDeclaration, 9);
        b.end(NodeKind::Fields, 9);
        b.end(NodeKind::DeclarationBody, 9);
        b.end(NodeKind::ClassDeclaration, 9);
        b.end(NodeKind::TopLevelDeclaration, 10);
        b.end(NodeKind::CompilationUnit, narrow_u32(t.len()));
    });
    assert!(field_hits(&f).is_empty());
    // Unset, never assigned.
    let f = Fixture::build("class C {\n  int _x;\n}\n", |b, t| {
        b.begin(NodeKind::CompilationUnit, 0);
        b.begin(NodeKind::TopLevelDeclaration, 0);
        identifier(b, 1, 2);
        b.begin(NodeKind::ClassDeclaration, 0);
        b.patch(0, 0);
        b.patch(1, NONE);
        b.patch(2, NONE);
        b.patch(3, NONE);
        b.begin(NodeKind::DeclarationBody, 2);
        b.begin(NodeKind::Fields, 2);
        b.patch(0, dartq_parser::MemberKind::NonStaticField as u32);
        b.begin(NodeKind::VariablesDeclaration, 3);
        b.patch(0, NONE);
        b.leaf_span(NodeKind::Type, 3, 4);
        b.begin(NodeKind::InitializedIdentifier, 4);
        identifier(b, 4, 5);
        b.end(NodeKind::InitializedIdentifier, 5);
        b.end(NodeKind::VariablesDeclaration, 6);
        b.end(NodeKind::Fields, 6);
        b.end(NodeKind::DeclarationBody, 6);
        b.end(NodeKind::ClassDeclaration, 6);
        b.end(NodeKind::TopLevelDeclaration, 7);
        b.end(NodeKind::CompilationUnit, narrow_u32(t.len()));
    });
    assert!(field_hits(&f).is_empty());
    // Top-level declarations are not fields.
    let f = Fixture::build("var _x = 0;\n", |b, t| {
        b.begin(NodeKind::CompilationUnit, 0);
        b.begin(NodeKind::TopLevelFields, 0);
        b.begin(NodeKind::VariablesDeclaration, 0);
        b.patch(0, NONE);
        b.begin(NodeKind::InitializedIdentifier, 0);
        identifier(b, 0, 1);
        b.begin(NodeKind::VariableInitializer, 2);
        b.leaf(NodeKind::LiteralInt, 3);
        b.end(NodeKind::VariableInitializer, 4);
        b.end(NodeKind::InitializedIdentifier, 4);
        b.end(NodeKind::VariablesDeclaration, 5);
        b.end(NodeKind::TopLevelFields, 5);
        b.end(NodeKind::CompilationUnit, narrow_u32(t.len()));
    });
    assert!(field_hits(&f).is_empty());
}

/// An assignment inside a constructor body is still a mutation.
#[test]
fn constructor_body_assignment_blocks() {
    // c0..{7 int8 _x9 ;10 C11 (12 )13 {14 _x15 =16 017 ;18 }19 }20
    let f = Fixture::build(
        "class C {\n  int _x = 0;\n  C() { _x = 0; }\n}\n",
        |b, t| {
            b.begin(NodeKind::CompilationUnit, 0);
            b.begin(NodeKind::TopLevelDeclaration, 0);
            identifier(b, 1, 2);
            b.begin(NodeKind::ClassDeclaration, 0);
            b.patch(0, 0);
            b.patch(1, NONE);
            b.patch(2, NONE);
            b.patch(3, NONE);
            b.begin(NodeKind::DeclarationBody, 2);
            b.begin(NodeKind::Fields, 2);
            b.patch(0, dartq_parser::MemberKind::NonStaticField as u32);
            b.begin(NodeKind::VariablesDeclaration, 3);
            b.patch(0, NONE);
            b.leaf_span(NodeKind::Type, 3, 4);
            b.begin(NodeKind::InitializedIdentifier, 4);
            identifier(b, 4, 5);
            b.begin(NodeKind::VariableInitializer, 5);
            b.leaf(NodeKind::LiteralInt, 6);
            b.end(NodeKind::VariableInitializer, 7);
            b.end(NodeKind::InitializedIdentifier, 7);
            b.end(NodeKind::VariablesDeclaration, 8);
            b.end(NodeKind::Fields, 8);
            b.begin(NodeKind::Constructor, 8);
            b.patch(0, dartq_parser::MemberKind::NonStaticMethod as u32);
            b.patch(1, NONE);
            b.patch(2, 9);
            b.patch(3, 11);
            identifier(b, 8, 9);
            b.begin(NodeKind::FormalParameters, 9);
            b.end(NodeKind::FormalParameters, 11);
            b.begin(NodeKind::BlockBody, 11);
            b.patch(0, NONE);
            b.patch(1, NONE);
            b.begin(NodeKind::Block, 11);
            b.begin(NodeKind::ExpressionStatement, 12);
            b.begin(NodeKind::Assignment, 12);
            b.patch(0, 13);
            identifier(b, 12, 13);
            b.leaf(NodeKind::LiteralInt, 14);
            b.end(NodeKind::Assignment, 15);
            b.end(NodeKind::ExpressionStatement, 16);
            b.end(NodeKind::Block, 16);
            b.end(NodeKind::BlockBody, 17);
            b.end(NodeKind::Constructor, 18);
            b.end(NodeKind::DeclarationBody, 18);
            b.end(NodeKind::ClassDeclaration, 19);
            b.end(NodeKind::TopLevelDeclaration, 19);
            b.end(NodeKind::CompilationUnit, narrow_u32(t.len()));
        },
    );
    assert!(field_hits(&f).is_empty());
}

/// Enum fields stay out (`PreferFinalFieldsEnum`) — the walk skips the
/// enum declaration whole.
#[test]
fn enum_fields_pass() {
    let f = Fixture::build("enum E { a }\n", |b, t| {
        b.begin(NodeKind::CompilationUnit, 0);
        b.begin(NodeKind::EnumDeclaration, 0);
        b.patch(0, NONE);
        b.patch(1, NONE);
        b.patch(2, NONE);
        identifier(b, 1, 2);
        b.begin(NodeKind::EnumBody, 3);
        b.begin(NodeKind::Fields, 4);
        b.patch(0, dartq_parser::MemberKind::NonStaticField as u32);
        b.begin(NodeKind::VariablesDeclaration, 4);
        b.patch(0, NONE);
        b.begin(NodeKind::InitializedIdentifier, 4);
        identifier(b, 4, 5);
        b.end(NodeKind::InitializedIdentifier, 5);
        b.end(NodeKind::VariablesDeclaration, 6);
        b.end(NodeKind::Fields, 6);
        b.end(NodeKind::EnumBody, 7);
        b.end(NodeKind::EnumDeclaration, 8);
        b.end(NodeKind::CompilationUnit, narrow_u32(t.len()));
    });
    assert!(field_hits(&f).is_empty());
}

// —— write-scan faces (production-bridge fixtures) ————————————————

/// A cascade-section write marks the field mutated (`.._x = 1` — the
/// section assignment is an `AssignmentExpression` upstream and its
/// `writeElement` is the field).
#[test]
fn cascade_section_write_blocks() {
    let f =
        Fixture::parse("class C {\n  int _x = 0;\n  void f(C o) {\n    o.._x = 1;\n  }\n}\n");
    assert!(field_hits(&f).is_empty(), "{:?}", field_hits(&f));
}

/// A write in a later cascade section counts too; the section head is
/// found under the second cascade's window.
#[test]
fn cascade_later_section_write_blocks() {
    let f = Fixture::parse(
        "class C {\n  int _x = 0;\n  void f(C o) {\n    o..toString().._x = 1;\n  }\n}\n",
    );
    assert!(field_hits(&f).is_empty(), "{:?}", field_hits(&f));
}

/// A local namesake does not capture the section head: it selects the
/// cascade target's member, not a scope-resolved name, so the write
/// counts and the field stays silent (the official `writeElement` is
/// the field under any local namesake).
#[test]
fn cascade_section_write_ignores_local_namesake() {
    let f = Fixture::parse(
        "class C {\n  int _x = 0;\n  void f(C o) {\n    var _x = 0;\n    o.._x = 1;\n  }\n}\n",
    );
    assert!(field_hits(&f).is_empty(), "{:?}", field_hits(&f));
}

/// A bare-name write in a method body still blocks the report.
#[test]
fn method_body_write_blocks() {
    let f = Fixture::parse("class C {\n  int _x = 0;\n  void f() {\n    _x = 2;\n  }\n}\n");
    assert!(field_hits(&f).is_empty(), "{:?}", field_hits(&f));
}

/// A block local declared before the write captures the bare name —
/// the field stays unmutated and reports (the official scan is
/// element-based: the write's `writeElement` is the local).
#[test]
fn local_namesake_write_passes() {
    let f = Fixture::parse(
        "class C {\n  int _x = 0;\n  void f() {\n    var _x = 0;\n    _x = 2;\n  }\n}\n",
    );
    assert_eq!(field_hits(&f).len(), 1);
}

/// A parameter namesake captures the same way.
#[test]
fn parameter_namesake_write_passes() {
    let f =
        Fixture::parse("class C {\n  int _x = 0;\n  void f(int _x) {\n    _x = 2;\n  }\n}\n");
    assert_eq!(field_hits(&f).len(), 1);
}

/// A field mutated only through a pattern assignment reports — the
/// official registers no pattern-assignment visitor
/// (`prefer_final_fields.dart` :41-48), so the oracle over-reports
/// here and the bare-name posture keeps parity.
#[test]
fn pattern_assignment_only_mutation_reports() {
    let f =
        Fixture::parse("class C {\n  int _x = 0;\n  void f() {\n    (_x,) = (1,);\n  }\n}\n");
    assert_eq!(field_hits(&f).len(), 1);
}

/// `var` destructuring declares locals and never writes the field —
/// the field reports as upstream (no pattern visitor covers
/// `PatternVariableDeclaration` either).
#[test]
fn var_destructuring_namesake_reports() {
    let f = Fixture::parse(
        "class C {\n  int _x = 0;\n  void f() {\n    var (_x, _y) = (1, 2);\n  }\n}\n",
    );
    assert_eq!(field_hits(&f).len(), 1);
}

/// The setter-override exemption (`prefer_final_fields_test.dart`
/// `test_overrideSetter_extends`): the written supertype chain
/// carries a setter of the same name, so the field never candidates.
#[test]
fn inherited_setter_exempts() {
    let f = Fixture::parse(
        "class A {\n  set _a(bool a) {}\n}\n\nclass B extends A {\n  bool _a = false;\n}\n",
    );
    assert!(field_hits(&f).is_empty(), "{:?}", field_hits(&f));
}

/// The implements clause carries the exemption too
/// (`test_overrideSetter_implements`).
#[test]
fn interface_setter_exempts() {
    let f = Fixture::parse(
        "class A {\n  set _a(bool a) {}\n}\n\nclass B implements A {\n  bool _a = false;\n}\n",
    );
    assert!(field_hits(&f).is_empty(), "{:?}", field_hits(&f));
}

/// Without a supertype setter the field reports (the exemption's
/// negative pin).
#[test]
fn field_without_setter_reports() {
    let f = Fixture::parse("class B {\n  bool _a = false;\n}\n");
    assert_eq!(field_hits(&f).len(), 1);
}

/// The cross-file half of the exemption: the setter sits in the
/// imported library, so the semantic bridge's inheritance arm answers
/// under the `name=` setter spelling.
#[test]
fn cross_file_setter_exempts() {
    let kit = "class A {\n  set _a(bool a) {}\n}\n";
    let source = "import 'kit.dart';\n\nclass B extends A {\n  bool _a = false;\n}\n";
    assert!(run_semantic(source, "prefer_final_fields", Some((kit, None))).is_empty());
}

/// A cross-file supertype without the setter keeps reporting (the
/// bridge arm's negative pin).
#[test]
fn cross_file_field_without_setter_reports() {
    let kit = "class A {}\n";
    let source = "import 'kit.dart';\n\nclass B extends A {\n  bool _a = false;\n}\n";
    let lints = run_semantic(source, "prefer_final_fields", Some((kit, None)));
    assert_eq!(lints.len(), 1, "{lints:?}");
}

// ---- #586 standard-set completions (production-bridge fixtures) ----

/// `await` on scalar literals reports at the keyword with the exact
/// type; `null` (:39) and inference-bound expressions stay silent.
#[test]
fn await_only_futures_scalar_literals_report() {
    let f = Fixture::parse(
        "void f() async {\n  await 1;\n  await 'x';\n  await (2.5);\n  await null;\n  await g();\n}\nvoid g() {}",
    );
    let lints = f.run(&one_rule("await_only_futures"));
    assert_eq!(lints.len(), 3, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "Uses 'await' on an instance of 'int', which is not a subtype of 'Future'."
    );
    assert_eq!(
        lints[1].message,
        "Uses 'await' on an instance of 'String', which is not a subtype of 'Future'."
    );
    assert_eq!(
        lints[2].message,
        "Uses 'await' on an instance of 'double', which is not a subtype of 'Future'."
    );
}

/// `==`/`!=` on unrelated scalar literal faces reports at the
/// operator with `(right, left)`; null operands (:53-55) and equal
/// faces stay silent.
#[test]
fn unrelated_type_equality_reports_mismatched_scalars() {
    let f = Fixture::parse(
        "void f() {\n  if (1 == 'a') {}\n  if (1 != true) {}\n  if (1 == 2) {}\n  if (null == 1) {}\n}",
    );
    let lints = f.run(&one_rule("unrelated_type_equality_checks"));
    assert_eq!(lints.len(), 2, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "The type of the right operand ('String') isn't a subtype or a supertype of the left operand ('int')."
    );
    assert_eq!(
        lints[1].message,
        "The type of the right operand ('bool') isn't a subtype or a supertype of the left operand ('int')."
    );
}

/// The corpus face: an `int` parameter compared against a bool
/// literal reports at the operator — the written-name operand face.
#[test]
fn unrelated_type_equality_reports_written_name_operand() {
    let f =
        Fixture::parse("void f(int someInt) {\n  var g4 = someInt == true;\n  print(g4);\n}\n");
    let lints = f.run(&one_rule("unrelated_type_equality_checks"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "The type of the right operand ('bool') isn't a subtype or a supertype of the left operand ('int')."
    );
    let eq = tok(&f.source, &f.tokens, "==", 0);
    assert_eq!(lints[0].range.start, utf16_at(&f, eq));
    assert_eq!(lints[0].range.end - lints[0].range.start, 2);
}

/// Two same-file class-typed operands report: the supertype walk
/// (`interfaceTypesAreUnrelated` shares only `Object`) finds no link
/// between them — the oracle reports (`typesAreUnrelated` bails only
/// on dynamic/bottom/related chains).
#[test]
fn unrelated_type_equality_same_file_names_report() {
    let f = Fixture::parse(
        "class A {}\nclass B {}\nvoid f(A a, B b) {\n  var r = a == b;\n  print(r);\n}\n",
    );
    let lints = f.run(&one_rule("unrelated_type_equality_checks"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    let eq = tok(&f.source, &f.tokens, "==", 0);
    assert_eq!(lints[0].range.start, utf16_at(&f, eq));
}

/// The switch-member body faces: the `case` head row (the pattern and the
/// guard seal inside it, before the member opens) tiles beside the body
/// rows in the member window, so the operand run must cut past it — a body
/// equality reads its left operand whole under guard-less and guarded
/// heads alike. The guard's own equality keys at the when clause and the
/// default member carries no head row; both already read clean and stay
/// unchanged.
#[test]
fn unrelated_type_equality_reports_in_case_bodies() {
    let f = Fixture::parse(
        "void f(int x, int a, String s, bool b) {\n  switch (x) {\n    case 1:\n      a == s;\n      break;\n    case 2 when b:\n      a == s;\n      break;\n    case 3 when a == s:\n      break;\n    default:\n      a == s;\n      break;\n  }\n}\n",
    );
    let lints = f.run(&one_rule("unrelated_type_equality_checks"));
    assert_eq!(lints.len(), 4, "{lints:?}");
    // Guard-less body, guarded body, guard, default — in member order.
    for (nth, lint) in lints.iter().enumerate() {
        let eq = tok(&f.source, &f.tokens, "==", nth);
        assert_eq!(lint.range.start, utf16_at(&f, eq), "== {nth}");
        assert_eq!(
            lint.message,
            "The type of the right operand ('String') isn't a subtype or a supertype of the left operand ('int')."
        );
    }
}

/// Assigning a non-null scalar to a `void` (or `FutureOr<void>`)
/// field reports; `null` is acceptable when expecting void (:57).
#[test]
fn void_checks_reports_void_field_assignment() {
    let f = Fixture::parse(
        "import 'dart:async';\nclass C {\n  void v;\n  FutureOr<void> fv;\n  int n;\n  void set_it() {\n    this.v = 1;\n    this.v = null;\n    this.fv = 'x';\n    this.fv = null;\n    this.n = 1;\n  }\n}",
    );
    let lints = f.run(&one_rule("void_checks"));
    assert_eq!(lints.len(), 2, "{lints:?}");
    assert_eq!(lints[0].message, "Assignment to a variable of type 'void'.");
    assert_eq!(lints[1].message, "Assignment to a variable of type 'void'.");
}

/// A `==` comparison is not an assignment face — no `void_checks`
/// report (the coordinator boundary case).
#[test]
fn void_checks_equality_operand_stays_silent() {
    let f = Fixture::parse("class C {\n  void v;\n  bool eq(int x) => x == 1;\n}");
    assert!(f.run(&one_rule("void_checks")).is_empty());
}

/// The corpus face: an `int` parameter written into a written
/// `FutureOr<void>` field reports at the assignment
/// (`drained = piece`, 15 chars).
#[test]
fn void_checks_reports_written_scalar_right_side() {
    let f = Fixture::parse(
        "import 'dart:async';\nclass VoidSink {\n  FutureOr<void> drained;\n  VoidSink(this.drained);\n  void pour(int piece) {\n    drained = piece;\n  }\n}\n",
    );
    let lints = f.run(&one_rule("void_checks"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(lints[0].message, "Assignment to a variable of type 'void'.");
    let drained = tok(&f.source, &f.tokens, "drained", 2);
    assert_eq!(lints[0].range.start, utf16_at(&f, drained));
    assert_eq!(lints[0].range.end - lints[0].range.start, 15);
}

/// A written `dynamic` right side is acceptable when expecting void
/// (:55-65) — stays silent.
#[test]
fn void_checks_written_dynamic_right_side_passes() {
    let f = Fixture::parse(
        "import 'dart:async';\nclass C {\n  FutureOr<void> drained;\n  void pour(dynamic piece) {\n    drained = piece;\n  }\n}\n",
    );
    assert!(f.run(&one_rule("void_checks")).is_empty());
}

/// `!` on a `T?` field reports at the operator in the written-`T?`
/// variable-initializer context; a method return carries no expected
/// type (the official lookup requires a `FunctionExpression`-shaped
/// ancestor) and a different context type stays silent.
#[test]
fn null_check_on_nullable_type_parameter_reports_in_matching_context() {
    let f = Fixture::parse(
        "class C<T> {\n  T? f;\n  T? m() { return this.f!; }\n  void init() { T? t = this.f!; print(t); }\n  T? ok() { return this.f; }\n  int? other() { return this.f!; }\n}",
    );
    let lints = f.run(&one_rule("null_check_on_nullable_type_parameter"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "The null check operator shouldn't be used on a variable whose type is a potentially nullable type parameter."
    );
    let bang = tok(&f.source, &f.tokens, "!", 1);
    assert_eq!(lints[0].range.start, utf16_at(&f, bang));
    assert_eq!(lints[0].range.end - lints[0].range.start, 1);
}

/// A `FunctionType` target assigned a `call`-bearing object reports
/// the implicit tear-off; explicit `.call` and plain values stay
/// silent.
#[test]
fn implicit_call_tearoffs_reports_function_typed_target() {
    let f = Fixture::parse(
        "class Callable {\n  void call() {}\n}\nclass Plain {\n  void hello() {}\n}\nclass C {\n  Callable obj = Callable();\n  Plain p = Plain();\n  void Function() f;\n  void setup() {\n    this.f = this.obj;\n    this.f = this.p;\n    this.f = this.obj.call;\n    this.f = 1;\n  }\n}",
    );
    let lints = f.run(&one_rule("implicit_call_tearoffs"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(lints[0].message, "Implicit tear-off of the 'call' method.");
}

/// The corpus face: a local written `void Function()` target bound to
/// a bare or `this.` field whose type names a same-file `call` class
/// — the production type row tiles as the declaration's left sibling.
#[test]
fn implicit_call_tearoffs_reports_local_declaration_target() {
    let f = Fixture::parse(
        "class Callable {\n  void call() {}\n}\nclass TearoffSites {\n  Callable c = Callable();\n  void run() {\n    void Function() p = c;\n    void Function() q = this.c;\n    print(p);\n    print(q);\n  }\n}\n",
    );
    let lints = f.run(&one_rule("implicit_call_tearoffs"));
    assert_eq!(lints.len(), 2, "{lints:?}");
    assert_eq!(lints[0].message, "Implicit tear-off of the 'call' method.");
    let bare = tok(&f.source, &f.tokens, "c", 1);
    assert_eq!(lints[0].range.start, utf16_at(&f, bare));
    assert_eq!(lints[0].range.end - lints[0].range.start, 1);
    let this_kw = tok(&f.source, &f.tokens, "this", 0);
    assert_eq!(lints[1].range.start, utf16_at(&f, this_kw));
    assert_eq!(lints[1].range.end - lints[1].range.start, 6);
}

/// `this is JSClass` / `this as JSClass` with a same-file `@JS`
/// class under a `dart:js_interop` import reports; non-interop
/// types, other receivers and files without the import stay silent.
#[test]
fn invalid_runtime_check_reports_dart_vs_js() {
    let f = Fixture::parse(
        "import 'dart:js_interop';\n\n@JS\nclass JS {\n  external JS();\n}\n\nclass Dart {\n  bool check() => this is JS;\n  Object cast() => this as JS;\n  bool plain(Object o) => o is JS;\n  bool self() => this is Dart;\n}",
    );
    let lints = f.run(&one_rule("invalid_runtime_check_with_js_interop_types"));
    assert_eq!(lints.len(), 2, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "Runtime check between 'Dart' and 'JS' checks whether a Dart value is a JS interop type, which might not be platform-consistent."
    );
    assert_eq!(
        lints[1].message,
        "Cast from 'Dart' to 'JS' casts a Dart value to a JS interop type, which might not be platform-consistent."
    );
}

/// Without `dart:js_interop` the same syntax is not interop-classified.
#[test]
fn invalid_runtime_check_requires_js_interop_import() {
    let f =
        Fixture::parse("@JS\nclass JS {}\n\nclass Dart {\n  bool check() => this is JS;\n}");
    assert!(
        f.run(&one_rule("invalid_runtime_check_with_js_interop_types"))
            .is_empty()
    );
}

/// The cross-file right face ([`Analysis::container_has_annotation`]):
/// a bare operand tested against an imported `@JS` container reports;
/// a resolvable unannotated container (`Some(false)`) and an
/// unresolvable type (`None`) stay silent. The cross-file verdict
/// needs the semantic mount — under a fixture mount the arm degrades
/// to silence (the same-file faces above stay the fixture-pinned
/// path).
#[test]
fn invalid_runtime_check_cross_file_right_face() {
    let kit = "@JS()\nclass I {}\nclass Plain {}\n";
    let source = "import 'dart:js_interop';\nimport 'kit.dart';\nclass Dart {}\nvoid f(Dart d) {\n  if (d is I) {}\n  if (d is Plain) {}\n  if (d is Missing) {}\n}\n";
    let lints = run_semantic(
        source,
        "invalid_runtime_check_with_js_interop_types",
        Some((kit, None)),
    );
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "Runtime check between 'Dart' and 'I' checks whether a Dart value is a JS interop type, which might not be platform-consistent."
    );

    let f = Fixture::parse(
        "import 'dart:js_interop';\nclass Base {\n  bool check() => this is I;\n}\n",
    );
    assert!(
        f.run(&one_rule("invalid_runtime_check_with_js_interop_types"))
            .is_empty(),
        "no semantic mount: the cross-file container face cannot answer"
    );
}

// —— unnecessary_type_check (`_checkAllTypeChecks`,
// best_practices_verifier.dart:987-1041) ————————————————

/// The mirror posture: the emission is the semantic tier's always-on
/// fold (the oracle's `BestPracticesVerifier` pass), so the lint run
/// stays silent even with the name explicitly enabled — the spec's
/// AST arm is structurally off (`kinds: &[]`); a run row would double
/// the semantic tier's and carry the wrong lint tier.
#[test]
fn unnecessary_type_check_is_a_semantic_mirror_not_a_lint_arm() {
    for source in ["void f() { 1 is int; }", "void f() { b is dynamic; }"] {
        let f = Fixture::parse(source);
        assert!(
            f.run(&one_rule("unnecessary_type_check")).is_empty(),
            "{source}"
        );
    }
}

// —— avoid_double_and_int_checks ————————————————————————————————————

/// `test/rules/avoid_double_and_int_checks_test.dart` — `x is double`
/// with an `else if (x is int)` on the same operand reports at the
/// second `is` keyword.
#[test]
fn double_int_checks_reports_the_else_if_int() {
    let source = "void f(num p) { if (p is double) {} else if (p is int) {} }";
    let f = Fixture::parse(source);
    let lints = f.run(&one_rule("avoid_double_and_int_checks"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(lints[0].message, "Explicit check for double or int.");
    assert_eq!(
        lints[0].correction.as_deref(),
        Some("Try removing the check.")
    );
    let is = tok(&f.source, &f.tokens, "is", 1);
    assert_eq!(lints[0].range.start, utf16_at(&f, is));
}

/// A local variable operand reports the same way.
#[test]
fn double_int_checks_local_operand_reports() {
    let f = Fixture::parse("void k() { num p = 1; if (p is double) {} else if (p is int) {} }");
    assert_eq!(f.run(&one_rule("avoid_double_and_int_checks")).len(), 1);
}

/// Different operands, a lone check, two separate ifs, and the
/// reversed order are all exempt.
#[test]
fn double_int_checks_valid_faces_pass() {
    let f = Fixture::parse(
        "void g(num p, num q) { if (p is double) {} else if (q is int) {} }\n\
             void h(num p) { if (p is double) {} }\n\
             void i(num p) { if (p is double) {} if (p is int) {} }\n\
             void j(num p) { if (p is int) {} else if (p is double) {} }",
    );
    assert!(f.run(&one_rule("avoid_double_and_int_checks")).is_empty());
}

// —— avoid_js_rounded_ints ——————————————————————————————————————————

/// `test/rules/avoid_js_rounded_ints_test.dart` — a literal beyond
/// 2^53 exact reports at the literal.
#[test]
fn js_rounded_ints_reports_an_imprecise_literal() {
    let source = "var a = 9007199254740993;";
    let f = Fixture::parse(source);
    let lints = f.run(&one_rule("avoid_js_rounded_ints"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "Integer literal can't be represented exactly when compiled to JavaScript."
    );
    assert_eq!(
        lints[0].correction.as_deref(),
        Some("Try using a 'BigInt' to represent the value.")
    );
    let literal = tok(&f.source, &f.tokens, "9007199254740993", 0);
    assert_eq!(lints[0].range.start, utf16_at(&f, literal));
}

/// The hexadecimal spelling of an imprecise value reports; a negated
/// imprecise literal reports at the literal itself.
#[test]
fn js_rounded_ints_hex_and_negative_faces() {
    let f = Fixture::parse("var b = 0x20000000000001;\nvar e = -9007199254740993;");
    assert_eq!(f.run(&one_rule("avoid_js_rounded_ints")).len(), 2);
}

/// 2^53 itself and small literals are exactly representable.
#[test]
fn js_rounded_ints_exact_literals_pass() {
    let f = Fixture::parse("var c = 9007199254740992;\nvar d = 1;");
    assert!(f.run(&one_rule("avoid_js_rounded_ints")).is_empty());
}

// —— the lints semantic bridge, consumption wave 1 ————————————

/// The prefixed-receiver face: a `prefix.g(e)` callee composes no
/// member face (the prefix carries no receiver type — the deferred
/// import's `mightBeDeferred` bail, :264, and the non-deferred
/// prefix's documented `None` silence alike), and the plain run stays
/// silent (the wave posture). The remaining `unnecessary_lambdas`
/// pins live in the semantic `b4_expr_pins` module.
/// The oracle-of-record's initializer face (probed 2026-10-18 on
/// 3.13.1): a block-body lambda in variable-initializer position
/// stays silent — only arrow bodies report there — while argument
/// positions report for both body forms (`locals_family.dart`'s
/// rc2 divergence, the `handler` initializer). The arrow-initializer
/// row fires through the `parametersMatch` fallback arm: `print`
/// resolves to no model signature (the unindexed dart:core name), so
/// the no-signature face reports — verified end to end against the
/// 3.13.1 SDK (#2095).
#[test]
fn unnecessary_lambdas_initializer_faces() {
    let initializer = "void f() {\n  final handler = (int input) {\n    print(input);\n  };\n  handler(1);\n}\n";
    assert!(
        Fixture::parse(initializer)
            .run(&one_rule("unnecessary_lambdas"))
            .is_empty(),
        "block body in initializer position stays silent"
    );
    // The arrow initializer reports in the semantic run; the plain
    // run stays silent (the wave posture). Both positions — the local
    // declaration and the top-level member — read the same face.
    let arrow_initializer = "void f() {\n  final h2 = (e) => print(e);\n  h2(1);\n}\n";
    assert!(
        Fixture::parse(arrow_initializer)
            .run(&one_rule("unnecessary_lambdas"))
            .is_empty(),
        "the plain run stays silent (the wave posture)"
    );
    let lints = run_semantic(arrow_initializer, "unnecessary_lambdas", None);
    assert_eq!(lints.len(), 1, "{lints:?}");
    let top_level = "final h2 = (e) => print(e);\nvoid f() {\n  h2(1);\n}\n";
    let lints = run_semantic(top_level, "unnecessary_lambdas", None);
    assert_eq!(lints.len(), 1, "{lints:?}");
    // The resolved-callee channel (#2109): a same-file `print` composes
    // `void Function(Object?)`, the untyped formal's inferred channel
    // leaves the verdict `None`, and the `parametersMatch` FunctionType
    // path carries the face — the oracle fires the same way (the #2095
    // lane's live probe on 3.13.1).
    let resolved =
        "void print(Object? e) {}\nvoid f() {\n  final h3 = (e) => print(e);\n  h3(1);\n}\n";
    let lints = run_semantic(resolved, "unnecessary_lambdas", None);
    assert_eq!(lints.len(), 1, "{lints:?}");
    let argument = "void f() {\n  [1].forEach((e) {\n    print(e);\n  });\n}\n";
    let lints = run_semantic(argument, "unnecessary_lambdas", None);
    assert_eq!(lints.len(), 1, "{lints:?}");
}

#[test]
fn unnecessary_lambdas_prefixed_receiver_stays_silent() {
    let source = "import 'kit.dart' deferred as io;\nvoid f(List<int> xs) {\n  xs.forEach((int e) => io.g(e));\n}\n";
    assert!(
        Fixture::parse(source)
            .run(&one_rule("unnecessary_lambdas"))
            .is_empty(),
        "the plain run stays silent"
    );
    assert!(
        run_semantic(
            source,
            "unnecessary_lambdas",
            Some(("void g(int e) {}\n", Some("io")))
        )
        .is_empty(),
        "the prefixed callee composes no face"
    );
}

/// The wave posture: the semantic rule is silent under the plain run
/// (`Check::semantics()` is `None` — the engine contract's
/// driver-configuration shape) and reports with the face mounted.
#[test]
fn prefer_null_aware_method_calls_requires_the_face() {
    let source = "Function()? func;\nvoid f() {\n  func != null ? func!() : null;\n}\n";
    let f = Fixture::parse(source);
    assert!(
        f.run(&one_rule("prefer_null_aware_method_calls"))
            .is_empty()
    );
    // Message: upstream Dart SDK source,
    // pkg/linter/lib/src/rules/prefer_null_aware_method_calls.dart.
    let lints = run_semantic(source, "prefer_null_aware_method_calls", None);
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "Use a null-aware invocation of the 'call' method rather than explicitly testing for 'null'."
    );
}

/// The branch faces: the conditional and `if` forms report at the
/// invocation; a non-`null` else arm, an `else` clause, a
/// two-statement block, a different target, and a bang before a
/// selector stay silent.
#[test]
fn prefer_null_aware_method_calls_branch_faces() {
    let source = "Function()? func;\nvoid f() {\n  if (func != null) func!();\n}\n";
    assert_eq!(
        run_semantic(source, "prefer_null_aware_method_calls", None).len(),
        1
    );

    // The single-statement block unwrap.
    let source = "Function()? func;\nvoid f() {\n  if (func != null) {\n    func!();\n  }\n}\n";
    assert_eq!(
        run_semantic(source, "prefer_null_aware_method_calls", None).len(),
        1
    );

    for source in [
        // A non-`null` else arm is not the shape.
        "Function()? func;\nvoid f() {\n  func != null ? func!() : func;\n}\n",
        // An `else` clause disqualifies the `if` form.
        "Function()? func;\nvoid f() {\n  if (func != null) func!(); else {}\n}\n",
        // A two-statement block is not one expression statement.
        "Function()? func;\nvoid f() {\n  if (func != null) {\n    func!();\n    func!();\n  }\n}\n",
        // A different target never matches (the token-lexeme windows).
        "Function()? f1, f2;\nvoid f() {\n  f1 != null ? f2!() : null;\n}\n",
        // A bang before a selector is a method invocation, not the
        // bare postfix call tile.
        "Function()? func;\nvoid f() {\n  func != null ? func!.m() : null;\n}\n",
        // A parenthesized operand carries its parens in the source
        // comparison.
        "Function()? func;\nvoid f() {\n  func != null ? (func)!() : null;\n}\n",
    ] {
        assert!(
            run_semantic(source, "prefer_null_aware_method_calls", None).is_empty(),
            "{source}"
        );
    }
}

/// Production pin: the `if` face recovers the condition binary from the
/// `ParenthesizedCondition` window's tail and reports exactly once, at
/// the invocation's whole-extent `Send` leaf.
#[test]
fn prefer_null_aware_method_calls_if_form_production_pin() {
    let source = "Function()? v;\nvoid f() {\n  if (v != null) v!();\n}\n";
    let lints = run_semantic(source, "prefer_null_aware_method_calls", None);
    assert_eq!(lints.len(), 1);
    let f = Fixture::parse(source);
    let v = tok(&f.source, &f.tokens, "v", 2);
    assert_eq!(lints[0].range.start, utf16_at(&f, v));
}

/// Tiling pin (audit checkpoint): in the production `x!()` then arm the
/// trailing invocation `Send` covers only `! ()` — the operand head the
/// token-equality window opens at is the bare-name group's `Identifier`
/// (`x`), tiled under the callee sequence before the `NonNullAssert`
/// leaf. The report must span `x!()` whole — the send's own range head
/// is `!`, a delta-tile anchor, never the operand head.
#[test]
fn prefer_null_aware_method_calls_operand_head_is_the_identifier() {
    let source = "Function()? v;\nvoid f() {\n  v != null ? v!() : null;\n}\n";
    let lints = run_semantic(source, "prefer_null_aware_method_calls", None);
    assert_eq!(lints.len(), 1);
    let f = Fixture::parse(source);
    let v = tok(&f.source, &f.tokens, "v", 2);
    assert_eq!(lints[0].range.start, utf16_at(&f, v));
    let bang = tok(&f.source, &f.tokens, "!", 0);
    assert_eq!(lints[0].range.end, utf16_at(&f, bang) + 3);
}

/// The semantic faces decide: the member face resolves a typed
/// receiver's member and the binding face resolves a bare variable;
/// a `dynamic` receiver, an unresolved bare name, and a
/// definitely-non-null dead-check read stay silent (the documented
/// under-report register).
#[test]
fn prefer_null_aware_method_calls_resolution_faces() {
    // The member face: `p.m` resolves on the receiver's container.
    let source = "class C {\n  int Function()? m;\n}\nvoid f(C p) {\n  p.m != null ? p.m!() : null;\n}\n";
    assert_eq!(
        run_semantic(source, "prefer_null_aware_method_calls", None).len(),
        1
    );

    // The dead-check face: a definitely-non-null bare read makes the
    // test provably false — the suggestion is vacuous (under-report).
    let source = "void f() {\n  Function func = () {};\n  func != null ? func!() : null;\n}\n";
    assert!(run_semantic(source, "prefer_null_aware_method_calls", None).is_empty());

    // The dynamic receiver: no member face resolves (under-report).
    let source = "void f(dynamic p) {\n  p.m != null ? p.m!() : null;\n}\n";
    assert!(run_semantic(source, "prefer_null_aware_method_calls", None).is_empty());

    // The unresolved bare name: no binding face resolves (the
    // null-element arm keeps silent under the resolution posture).
    let source = "void f() {\n  gone != null ? gone!() : null;\n}\n";
    assert!(run_semantic(source, "prefer_null_aware_method_calls", None).is_empty());
}

/// The unnecessary_parenthesis imported-extension face: the accessor
/// resolving through an import to a nullable-on-type extension keeps
/// the parens (the upstream resolver sees imported libraries,
/// unnecessary_parenthesis.dart:120-124); the same shape resolving to
/// an imported class member reports (the resolved element is not an
/// extension application).
#[test]
fn paren_imported_extension_member_faces() {
    let kit = "extension E on String? {\n  bool get g => false;\n}\n";
    let source = "import 'kit.dart';\n\nvoid f(String? s) {\n  (s).g;\n}\n";
    assert!(run_semantic(source, "unnecessary_parenthesis", Some((kit, None))).is_empty());

    let kit = "class S {\n  bool get g => false;\n}\n";
    let source = "import 'kit.dart';\n\nvoid f(S s) {\n  (s).g;\n}\n";
    assert_eq!(
        run_semantic(source, "unnecessary_parenthesis", Some((kit, None))).len(),
        1
    );
}

/// The semantic query coordinate pin: `query_site` feeds the
/// analysis queries in UTF-16 offsets (the fold rows' convention —
/// the bridge keys every row at its UTF-16 start). A non-BMP glyph
/// ahead of the queried reads (U+1F600: four UTF-8 bytes, two UTF-16
/// units) skews every later byte offset by +2, so byte-span queries
/// overshoot the read rows and miss — the `await` type face stays
/// silent under the old coordinates and reports 1 under UTF-16.
#[test]
fn semantic_query_coordinates_are_utf16() {
    let source = "// \u{1F600} four-byte, two-unit glyph\nvoid f() async {\n  int x = 1;\n  await x;\n}\n";
    assert_eq!(run_semantic(source, "await_only_futures", None).len(), 1);
}

/// The prefer_contains constant-identifier bound: `kLen` reads its
/// const initializer (the upstream `getIntValue` identifier face,
/// prefer_contains.dart over ast.dart `_getIntValue`) and the bound
/// works like the literal.
#[test]
fn prefer_contains_const_identifier_bound() {
    // The bound rides an `indexOf` comparison inside the official
    // -1/0 matrix (prefer_contains.dart :73-124) — a bound off the
    // matrix or a non-indexOf operand stays silent upstream. `-1`
    // also covers the prefixed-literal face of the const read.
    let source = "const kMinusOne = -1;\nvoid f(List<int> l) {\n  if (l.indexOf(1) <= kMinusOne) {}\n}\n";
    assert_eq!(run_semantic(source, "prefer_contains", None).len(), 1);
}

/// The prefer_contains receiver gate: a receiver resolvable as a
/// same-file non-iterable class stays silent (the official
/// `parentType` check).
#[test]
fn prefer_contains_non_iterable_receiver_silent() {
    let source =
        "class X { int get length => 0; }\nvoid f(X x) {\n  if (x.length <= 3) {}\n}\n";
    assert!(run_semantic(source, "prefer_contains", None).is_empty());
}

/// The oracle-probed face (`dart analyze` on
/// `typedef IntAlias = int; void f(IntAlias x) { if (x is int) {} }`:
/// exactly one `unnecessary_type_check`, never
/// `avoid_double_and_int_checks` on a single is-check): the alias-typed
/// formal folds through the semantic tier's `unnecessary_type_check`
/// mirror (pinned by the semantic suite's
/// `unnecessary_type_check_folds_a_typed_formal_alias`) and this rule
/// stays silent — the else-if chain gate keeps a lone check exempt,
/// alias or not; the mirror's lint arm stays structurally off. A
/// `dynamic` operand never reports either.
#[test]
fn avoid_double_and_int_checks_typedef_alias() {
    let source = "typedef IntAlias = int;\nvoid f(IntAlias x) {\n  if (x is int) {}\n}\n";
    assert!(run_semantic(source, "avoid_double_and_int_checks", None).is_empty());
    assert!(run_semantic(source, "unnecessary_type_check", None).is_empty());
    let source = "void g(dynamic x) {\n  if (x is int) {}\n}\n";
    assert!(run_semantic(source, "avoid_double_and_int_checks", None).is_empty());
}

/// The avoid_function_literals_in_foreach_calls receiver gate: an
/// `Iterable` receiver reports, a same-file non-iterable receiver
/// stays silent (the official `implementsIterable` bail).
#[test]
fn foreach_calls_iterable_gate() {
    let source = "void f(List<int> l) {\n  l.forEach((e) {});\n}\n";
    assert_eq!(
        run_semantic(source, "avoid_function_literals_in_foreach_calls", None).len(),
        1
    );
    let source = "class X {}\nvoid f(X x) {\n  x.forEach((e) {});\n}\n";
    assert!(run_semantic(source, "avoid_function_literals_in_foreach_calls", None).is_empty());
}

/// The await_only_futures bridge type face: awaiting a non-`Future`
/// value reports; awaiting a `Future` stays silent.
#[test]
fn await_only_futures_static_type_gate() {
    let source = "void f() async {\n  int x = 1;\n  await x;\n}\n";
    assert_eq!(run_semantic(source, "await_only_futures", None).len(), 1);
    let source = "Future<int> g() async => 1;\nvoid f() async {\n  await g();\n}\n";
    assert!(run_semantic(source, "await_only_futures", None).is_empty());
}

/// The implicit_call_tearoffs faces: a written function-type target
/// and an operand whose type names a same-file class with an instance
/// `call` method reports; an operand without `call` stays silent.
#[test]
fn implicit_call_tearoffs_call_member_gate() {
    let source = "class C {\n  void call() {}\n}\nvoid f() {\n  C c = C();\n  void Function() g = c;\n}\n";
    assert_eq!(
        run_semantic(source, "implicit_call_tearoffs", None).len(),
        1
    );
    let source = "class D {}\nvoid f(D d) {\n  void Function() g = d;\n}\n";
    assert!(run_semantic(source, "implicit_call_tearoffs", None).is_empty());
}

/// The no_literal_bool_comparisons member faces: `this.flag == true`
/// over a same-class `bool` member reports; a non-literal operand
/// stays silent.
#[test]
fn no_literal_bool_comparisons_this_member_face() {
    let source = "class C {\n  bool flag = true;\n  bool other = false;\n  void m() {\n    if (this.flag == true) {}\n    if (this.flag == other) {}\n  }\n}\n";
    assert_eq!(
        run_semantic(source, "no_literal_bool_comparisons", None).len(),
        1
    );
}

/// The null_check_on_nullable_type_parameter context faces: a method
/// return carries no expected type (the official
/// `thisOrAncestorOfType<FunctionExpression>` miss — the declaration
/// wraps its function expression only for functions), while an
/// assignment target and an argument formal written `T?` report.
#[test]
fn null_check_type_parameter_contexts() {
    let source = "class A {\n  T? m<T>(T? t) {\n    return t!;\n  }\n}\n";
    assert!(run_semantic(source, "null_check_on_nullable_type_parameter", None).is_empty());
    let source = "void g<T>(T? t) {\n  T? x;\n  x = t!;\n}\nvoid h<T>(T? p) {}\nvoid k<T>(T? t) {\n  h(t!);\n}\n";
    assert_eq!(
        run_semantic(source, "null_check_on_nullable_type_parameter", None).len(),
        2
    );
    // The official assignment guard: `x = x!` promotes — the lookup
    // returns no expected type (:64-68), silent.
    let source = "void g<T>() {\n  T? x;\n  x = x!;\n}\n";
    assert!(run_semantic(source, "null_check_on_nullable_type_parameter", None).is_empty());
    // The w3-measured over-report face (members_family.dart:46): a
    // class-method return carries no expected type upstream (the
    // `thisOrAncestorOfType<FunctionExpression>` miss) — silent.
    let source = "class Holder<T> {\n  T? held;\n  T? take() {\n    return held!;\n  }\n}\n";
    assert!(run_semantic(source, "null_check_on_nullable_type_parameter", None).is_empty());
}

/// The pattern face (`visitNullAssertPattern`): `case var a!:` over a
/// `T?` scrutinee reports at the `!` — the matched value type is the
/// nullable type parameter, keyed at the scrutinee read's own span. A
/// non-type-parameter matched value and a non-lone-identifier
/// scrutinee stay silent (the face's read-row shape under-reports).
#[test]
fn null_check_pattern_matched_type_face() {
    let source = "class C<T> {\n  void m(T? t) {\n    switch (t) {\n      case var a!:\n        print(a);\n        break;\n    }\n  }\n}\n";
    let lints = run_semantic(source, "null_check_on_nullable_type_parameter", None);
    assert_eq!(lints.len(), 1, "{lints:?}");

    let source = "class C<T> {\n  void m(String? s) {\n    switch (s) {\n      case var a!:\n        print(a);\n        break;\n    }\n  }\n}\n";
    assert!(
        run_semantic(source, "null_check_on_nullable_type_parameter", None).is_empty(),
        "a class-typed matched value is not a type parameter"
    );

    let source = "class C<T> {\n  void m(T? t) {\n    switch (t.length) {\n      case var a!:\n        print(a);\n        break;\n    }\n  }\n}\n";
    assert!(
        run_semantic(source, "null_check_on_nullable_type_parameter", None).is_empty(),
        "a member-access scrutinee is not a bare variable read"
    );
}

/// The null_closures faces: a `this.`-field receiver reads the
/// field's written type (`List` puts `map`'s slot 0 in the table);
/// a cross-file user class named like a core type resolves through
/// the bridge and the written-name fallback stays silent.
#[test]
fn null_closures_this_receiver_and_user_container() {
    let source =
        "class C {\n  List<int> list = [];\n  void m() {\n    this.list.map(null);\n  }\n}\n";
    assert_eq!(run_semantic(source, "null_closures", None).len(), 1);
    let kit = "class Timer {\n  Timer(void Function() f);\n}\n";
    let source = "import 'kit.dart';\n\nvoid g() {\n  Timer(null);\n}\n";
    assert!(run_semantic(source, "null_closures", Some((kit, None))).is_empty());
}

/// The prefer_is_empty receiver gate: an `Iterable`/`Map`/`String`
/// receiver reports; a same-file class with a plain `length` getter
/// stays silent (the official `type == null` bail closes the opaque
/// presumption for a resolvable non-core receiver).
#[test]
fn prefer_is_empty_receiver_gate() {
    let source = "void f(List<int> l) {\n  if (l.length == 0) {}\n}\n";
    assert_eq!(run_semantic(source, "prefer_is_empty", None).len(), 1);
    let source =
        "class X { int get length => 0; }\nvoid f(X x) {\n  if (x.length == 0) {}\n}\n";
    assert!(run_semantic(source, "prefer_is_empty", None).is_empty());
}

/// The prefer_is_not_empty member gate: a receiver whose type
/// provides `isEmpty` but no `isNotEmpty` stays silent (the official
/// `isNotEmpty`-provision check). The upstream registers a prefix
/// visitor only (`addPrefixExpression`, :31-56) — `x.isEmpty ==
/// false` is not a face of this rule (the boolean-literal comparison
/// belongs to `no_literal_bool_comparisons`), so both shapes stay
/// silent exactly like the oracle.
#[test]
fn prefer_is_not_empty_member_gate() {
    let source = "void f(List<int> l) {\n  if (!l.isEmpty) {}\n}\n";
    assert_eq!(
        run_semantic(source, "prefer_is_not_empty", None).len(),
        1,
        "the prefix face over a core List reports"
    );
    let source = "void f(List<int> l) {\n  if (l.isEmpty == false) {}\n}\n";
    assert!(
        run_semantic(source, "prefer_is_not_empty", None).is_empty(),
        "no binary `== false` arm upstream"
    );
    let source =
        "class X { bool get isEmpty => true; }\nvoid f(X x) {\n  if (!x.isEmpty) {}\n}\n";
    assert!(
        run_semantic(source, "prefer_is_not_empty", None).is_empty(),
        "a user class without `isNotEmpty` stays silent"
    );
}

/// The unrelated_type_equality_checks faces: two unrelated same-file
/// classes report; sibling classes sharing a non-`Object` supertype
/// are comparable (the official supertype walk) and stay silent.
#[test]
fn unrelated_type_equality_checks_supertype_face() {
    let source = "class X {}\nclass Y {}\nvoid f(X x, Y y) {\n  if (x == y) {}\n}\n";
    assert_eq!(
        run_semantic(source, "unrelated_type_equality_checks", None).len(),
        1
    );
    let source = "class Base {}\nclass D1 extends Base {}\nclass D2 extends Base {}\nvoid f(D1 a, D2 b) {\n  if (a == b) {}\n}\n";
    assert!(run_semantic(source, "unrelated_type_equality_checks", None).is_empty());
}

/// The void_checks assignment faces: a bare target resolves
/// shadow-correct (the local `int v` shadows the `void` field — the
/// official `writeType` face), a `this.`-target reports over the
/// field's written type.
#[test]
fn void_checks_assignment_target_resolution() {
    let source =
        "class C {\n  void v = null;\n  void m() {\n    int v = 1;\n    v = 2;\n  }\n}\n";
    assert!(run_semantic(source, "void_checks", None).is_empty());
    let source = "class C {\n  void v = null;\n  void m() {\n    this.v = 1;\n  }\n}\n";
    assert_eq!(run_semantic(source, "void_checks", None).len(), 1);
}

/// The void_checks return face: only a `FutureOr<void>` written
/// return type ever reports (:142-146 silents every plain-`void`
/// return — `dynamic` is void-acceptable and everything else
/// early-silents). `null` is acceptable, the scalars are not.
#[test]
fn void_checks_future_or_void_return() {
    let source = "FutureOr<void> f() {\n  return 1;\n}\n";
    assert_eq!(run_semantic(source, "void_checks", None).len(), 1);
    let source = "FutureOr<void> f() {\n  return null;\n}\n";
    assert!(run_semantic(source, "void_checks", None).is_empty());
    let source = "void f() {\n  return 1;\n}\n";
    assert!(run_semantic(source, "void_checks", None).is_empty());
}

/// The void_checks argument faces: a `void` formal through the
/// bridge signature and through the resolved constructor formals
/// reports a decidable non-acceptable value at the argument;
/// `null` and a non-`void` formal stay silent.
#[test]
fn void_checks_void_formal_arguments() {
    let source = "void h(void x) {}\nvoid g() {\n  h(1);\n}\n";
    assert_eq!(run_semantic(source, "void_checks", None).len(), 1);
    let source = "void h(void x) {}\nvoid h2(int x) {}\nvoid g() {\n  h(null);\n  h2(1);\n}\n";
    assert!(run_semantic(source, "void_checks", None).is_empty());
    // The implicit creation resolves through the semantic member
    // chain (the `this`-face landing: an implicit-`this` bare member
    // read answers like the explicit form, so `resolve_constructor`
    // answers here) — the face reports, matching upstream's
    // post-resolution `InstanceCreationExpression` answer.
    let source = "class K {\n  K(void x);\n}\nvoid m() {\n  K(1);\n}\n";
    assert_eq!(run_semantic(source, "void_checks", None).len(), 1);
}

/// The void_checks assignment value face over the bridge: a callee
/// return type decides where the scalar faces stay silent, and the
/// acceptability cut splits on the target — `FutureOr` is not
/// void-acceptable upstream (`isTypeAcceptableWhenExpectingVoid`
/// recurses only through a `Future` head), `dynamic` passes both
/// cuts.
#[test]
fn void_checks_assignment_bridge_value_face() {
    let source = "class C {\n  FutureOr<void> drained;\n  int piece() => 1;\n  void pour() {\n    drained = piece();\n  }\n}\n";
    assert_eq!(run_semantic(source, "void_checks", None).len(), 1);
    let source = "class C {\n  void v = null;\n  FutureOr<int> h() => 1;\n  void m() {\n    this.v = h();\n  }\n}\n";
    assert_eq!(run_semantic(source, "void_checks", None).len(), 1);
    let source = "class C {\n  void v = null;\n  dynamic d() => null;\n  void m() {\n    this.v = d();\n  }\n}\n";
    assert!(run_semantic(source, "void_checks", None).is_empty());
}

/// The null_check_on_nullable_type_parameter operand face through the
/// same-file binding walk: a local written `T?` is a nullable type
/// parameter operand (the field-face-only read under-reported it); a
/// class-typed local stays silent.
#[test]
fn null_check_binding_operand_face() {
    let source = "void g<T>() {\n  T? x;\n  T? y;\n  y = x!;\n}\n";
    assert_eq!(
        run_semantic(source, "null_check_on_nullable_type_parameter", None).len(),
        1
    );
    let source = "void g() {\n  String? s;\n  String? t;\n  t = s!;\n}\n";
    assert!(run_semantic(source, "null_check_on_nullable_type_parameter", None).is_empty());
}

/// The unrelated_type_equality_checks pattern face
/// (`visitRelationalPattern`): a relational pattern whose operand
/// type and matched value type are unrelated reports at the pattern;
/// a related operand stays silent.
///
/// The pattern face rides [`Analysis::matched_value_type`] like the
/// null-assert pattern face (pinned green over
/// `null_check_pattern_matched_type_face`); the operator lands in the
/// pattern row's payload word 0 (#1723).
#[test]
fn unrelated_type_equality_pattern_face() {
    let source = "void f(int n) {\n  switch (n) {\n    case == 'x':\n      print(0);\n      break;\n  }\n}\n";
    assert_eq!(
        run_semantic(source, "unrelated_type_equality_checks", None).len(),
        1
    );
    let source = "void f(int n) {\n  switch (n) {\n    case == 3:\n      print(0);\n      break;\n  }\n}\n";
    assert!(run_semantic(source, "unrelated_type_equality_checks", None).is_empty());
}

/// The await_only_futures collection-literal faces: a literal
/// collection is never a `Future` (`await []`, a typed list, a map)
/// — the official reports the inferred type.
#[test]
fn await_only_futures_collection_literals() {
    let source = "void f() async {\n  await [];\n}\n";
    assert_eq!(run_semantic(source, "await_only_futures", None).len(), 1);
    let source = "void f() async {\n  await <int>[1];\n}\n";
    assert_eq!(run_semantic(source, "await_only_futures", None).len(), 1);
    let source = "void f() async {\n  await {1: 2};\n}\n";
    assert_eq!(run_semantic(source, "await_only_futures", None).len(), 1);
}

/// The collection-literal message face pins the inferred display:
/// `await {1: 2}` spells `Map<int, int>` — the entry sides come from
/// the sibling run (the `MapEntry` marker is childless on the
/// production tiling), not from `collect_direct` (upstream
/// `pkg/linter/lib/src/rules/await_only_futures.dart:55` reports the
/// resolved static type).
#[test]
fn await_only_futures_map_entry_display() {
    let lints = run_semantic(
        "void f() async {\n  await {1: 2};\n}\n",
        "await_only_futures",
        None,
    );
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "Uses 'await' on an instance of 'Map<int, int>', which is not a subtype of 'Future'."
    );
}

/// Mixed entry faces: identifier sides read their static type, `!`
/// unwraps to the non-nullable side display, and a nested literal
/// spells its own collection face.
#[test]
fn await_only_futures_map_entry_nested_sides() {
    let lints = run_semantic(
        "void f() async {\n  bool b = true;\n  int? x = 0;\n  await {'k': b, 1: x!, [2]: {'n': 0.5}};\n}\n",
        "await_only_futures",
        None,
    );
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "Uses 'await' on an instance of 'Map<dynamic, dynamic>', which is not a subtype of 'Future'."
    );
}

/// A bare-name value side unwraps its send group to the identifier's
/// type; a bare empty `{}` settles as `Map<dynamic, dynamic>` upstream.
#[test]
fn await_only_futures_map_identifier_and_empty() {
    let lints = run_semantic(
        "void f() async {\n  bool b = true;\n  await {1: b};\n  await {};\n}\n",
        "await_only_futures",
        None,
    );
    assert_eq!(lints.len(), 2, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "Uses 'await' on an instance of 'Map<int, bool>', which is not a subtype of 'Future'."
    );
    assert_eq!(
        lints[1].message,
        "Uses 'await' on an instance of 'Map<dynamic, dynamic>', which is not a subtype of 'Future'."
    );
}

/// Explicit type arguments win over the element faces: `<String, int>`
/// decides the map display and `<int>` the list's, even with no
/// elements to read.
#[test]
fn await_only_futures_typed_collection_display() {
    let lints = run_semantic(
        "void f() async {\n  await <String, int>{'a': 1};\n  await <int>[];\n}\n",
        "await_only_futures",
        None,
    );
    assert_eq!(lints.len(), 2, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "Uses 'await' on an instance of 'Map<String, int>', which is not a subtype of 'Future'."
    );
    assert_eq!(
        lints[1].message,
        "Uses 'await' on an instance of 'List<int>', which is not a subtype of 'Future'."
    );
}

/// A nested literal as a map key keeps the outer entry's own sides —
/// the inner `MapEntry` must not bleed into the outer display.
#[test]
fn await_only_futures_map_nested_literal_key() {
    let lints = run_semantic(
        "void f() async {\n  await {{1: 2}: 'a'};\n}\n",
        "await_only_futures",
        None,
    );
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "Uses 'await' on an instance of 'Map<Map<int, int>, String>', which is not a subtype of 'Future'."
    );
}

/// The null_closures computed-receiver face: a bare send's callee
/// return type resolves the receiver's core class (`g().map(null)`
/// reaches the `Iterable.map` entry through the send face).
#[test]
fn null_closures_computed_receiver() {
    let source = "List<int> g() => [];\nvoid f() {\n  g().map(null);\n}\n";
    assert_eq!(run_semantic(source, "null_closures", None).len(), 1);
    let source = "List<int> g() => [];\nvoid f() {\n  g().add(null);\n}\n";
    assert!(run_semantic(source, "null_closures", None).is_empty());
}

/// The implicit_call_tearoffs return and argument faces: a written
/// function-type return with a `call`-carrying operand reports at the
/// returned expression, a same-file top-level callee's function-typed
/// formal reports at the argument value, and an operand without
/// `call` stays silent.
#[test]
fn implicit_call_tearoffs_return_and_argument_faces() {
    let source = "class C {\n  void call() {}\n}\nvoid Function() f(C c) {\n  return c;\n}\n";
    assert_eq!(
        run_semantic(source, "implicit_call_tearoffs", None).len(),
        1
    );
    let source = "class C {\n  void call() {}\n}\nvoid g(void Function() f) {}\nvoid h(C c) {\n  g(c);\n}\n";
    assert_eq!(
        run_semantic(source, "implicit_call_tearoffs", None).len(),
        1
    );
    let source = "class D {}\nvoid g(void Function() f) {}\nvoid h(D d) {\n  g(d);\n}\n";
    assert!(run_semantic(source, "implicit_call_tearoffs", None).is_empty());
}

/// The implicit_call_tearoffs bare-`Function` context and creation
/// operand: `Function p = C()` reports when `Function` resolves to
/// the core type and the creation's container carries `call`.
#[test]
fn implicit_call_tearoffs_function_target_and_creation() {
    let source = "class C {\n  void call() {}\n}\nvoid f() {\n  Function p = C();\n}\n";
    assert_eq!(
        run_semantic(source, "implicit_call_tearoffs", None).len(),
        1
    );
    let source = "class C {\n  void call() {}\n}\nvoid g(void Function() f) {}\nvoid h() {\n  g(C());\n}\n";
    assert_eq!(
        run_semantic(source, "implicit_call_tearoffs", None).len(),
        1
    );
}
