use crate::testsupport::{Fixture, one_rule, tok, utf16_at};

/// `test_nullableParameter_neNull` and the `??` face report at the
/// comparison; the non-`?` parameter (the analyzer's
/// `nonNullableEqualsParameter` face) and a non-`==` operator stay
/// silent.
#[test]
fn avoid_null_checks_in_equality_operator_faces() {
    let f = Fixture::parse(
        "class C {\n  operator ==(Object? other) => other != null && other is C;\n}\n",
    );
    let lints = f.run(&one_rule("avoid_null_checks_in_equality_operators"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "Unnecessary null comparison in implementation of '=='."
    );
    let start = tok(&f.source, &f.tokens, "other", 1);
    assert_eq!(lints[0].range.start, utf16_at(&f, start));

    // The parenthesized operand keeps the binary's canonical element.
    let f = Fixture::parse("class C {\n  operator ==(Object? other) => (other) ?? null;\n}\n");
    assert_eq!(
        f.run(&one_rule("avoid_null_checks_in_equality_operators"))
            .len(),
        1
    );

    // A `?.` access on the parameter reports at the access.
    let f = Fixture::parse(
        "class C {\n  operator ==(Object? other) => other is C && other?.hashCode == 1;\n}\n",
    );
    let lints = f.run(&one_rule("avoid_null_checks_in_equality_operators"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    let start = tok(&f.source, &f.tokens, "other", 2);
    assert_eq!(lints[0].range.start, utf16_at(&f, start));

    // Non-`?` and `dynamic` parameters are the analyzer's face, not
    // this lint's.
    for source in [
        "class C {\n  operator ==(Object other) => other != null;\n}\n",
        "class C {\n  operator ==(dynamic other) => other != null;\n}\n",
    ] {
        let f = Fixture::parse(source);
        assert!(
            f.run(&one_rule("avoid_null_checks_in_equality_operators"))
                .is_empty()
        );
    }

    // The `null == other` mirror and the `?.` invocation face report
    // once, at the same anchor the official reports (`reportAtNode` /
    // the target head); a block body walks like the arrow body.
    let f = Fixture::parse(
        "class C {\n  bool operator ==(Object? other) {\n    return null == other;\n  }\n}\n",
    );
    let lints = f.run(&one_rule("avoid_null_checks_in_equality_operators"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    let start = tok(&f.source, &f.tokens, "null", 0);
    assert_eq!(lints[0].range.start, utf16_at(&f, start));

    let f = Fixture::parse("class C {\n  bool operator ==(Object? other) => other?.foo();\n}\n");
    let lints = f.run(&one_rule("avoid_null_checks_in_equality_operators"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    let start = tok(&f.source, &f.tokens, "other", 1);
    assert_eq!(lints[0].range.start, utf16_at(&f, start));

    // A chain receiver is not the parameter (the canonicalElement
    // identity face).
    let f = Fixture::parse("class C {\n  int m(C x, Object? other) => x.other?.foo();\n}\n");
    assert!(
        f.run(&one_rule("avoid_null_checks_in_equality_operators"))
            .is_empty()
    );

    // A non-`==` operator and a two-parameter `==` stay silent.
    for source in [
        "class C {\n  operator <(Object? other) => other != null;\n}\n",
        "class C {\n  operator ==(Object? a, Object? b) => a != null;\n}\n",
    ] {
        let f = Fixture::parse(source);
        assert!(
            f.run(&one_rule("avoid_null_checks_in_equality_operators"))
                .is_empty()
        );
    }
}

/// The single-return block body reports at the body; a bare `return;`,
/// several statements, and a non-return body stay silent.
/// (`unnecessary_lambdas` moved to the semantic `b4_expr_pins` module
/// with its signature-gate faces.)
#[test]
fn prefer_expression_function_bodies_faces() {
    let f = Fixture::parse("int f() {\n  return 1;\n}\n");
    let lints = f.run(&one_rule("prefer_expression_function_bodies"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "Unnecessary use of a block function body."
    );
    // The body `{` is the fixture's only brace.
    let start = tok(&f.source, &f.tokens, "{", 0);
    assert_eq!(lints[0].range.start, utf16_at(&f, start));

    let f = Fixture::parse("class A {\n  get g {\n    return 7;\n  }\n}\n");
    assert_eq!(
        f.run(&one_rule("prefer_expression_function_bodies")).len(),
        1
    );

    for source in [
        "void f() {\n  return;\n}\n",
        "int f() {\n  var a = 1;\n  return a;\n}\n",
        "set x(int v) {\n  y = v;\n}\nvar y = 0;\n",
        "int f() {\n  if (1 == 2) {\n    return 1;\n  }\n  return 0;\n}\n",
    ] {
        let f = Fixture::parse(source);
        assert!(
            f.run(&one_rule("prefer_expression_function_bodies"))
                .is_empty(),
            "{source}"
        );
    }
}

/// The removable parens report. (`avoid_bool_literals_in_conditional_expressions`,
/// `prefer_int_literals`, `literal_only_boolean_expressions`, and
/// `unnecessary_lambdas` moved to the semantic `b4_expr_pins` module
/// with their wave faces.)
#[test]
fn inherited_wave_faces() {
    // The single positional argument and the condition paren report.
    let f = Fixture::parse("void g(int a) {}\nvoid f(int a) {\n  g((a));\n}\n");
    let lints = f.run(&one_rule("unnecessary_parenthesis"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(lints[0].message, "Unnecessary use of parentheses.");
    let f = Fixture::parse("void f(bool a) {\n  if ((a)) {}\n}\n");
    assert_eq!(f.run(&one_rule("unnecessary_parenthesis")).len(), 1);
    // A meaningful operator operand stays silent.
    let f = Fixture::parse("void f(int a, int b) {\n  var x = (a + b) * 2;\n}\n");
    assert!(f.run(&one_rule("unnecessary_parenthesis")).is_empty());
}

/// The record-parameter faces (`test_recordInside_param` /
/// `test_recordInside_namedParam`): the same-file written record type
/// of the corresponding formal keeps the parentheses (:75-86); the
/// typed-out parameter keeps the report. The variable face
/// (`(int,) r = (3);`, :47-53) stays exempt either way.
#[test]
fn record_parameter_faces() {
    let f = Fixture::parse("void g((int,) i) {}\nvoid f() {\n  g((3));\n}\n");
    assert!(f.run(&one_rule("unnecessary_parenthesis")).is_empty());

    let f = Fixture::parse("void g({required (int,) i}) {}\nvoid f() {\n  g(i: (3));\n}\n");
    assert!(f.run(&one_rule("unnecessary_parenthesis")).is_empty());

    let f = Fixture::parse("void g((int,) i) {}\nvoid f() {\n  g(((3,)));\n}\n");
    assert_eq!(f.run(&one_rule("unnecessary_parenthesis")).len(), 1);

    let f = Fixture::parse("void g(int i) {}\nvoid f() {\n  g((3));\n}\n");
    assert_eq!(f.run(&one_rule("unnecessary_parenthesis")).len(), 1);

    let f = Fixture::parse("void g(int i) {}\nvoid f() {\n  g(i: (3));\n}\n");
    assert_eq!(f.run(&one_rule("unnecessary_parenthesis")).len(), 1);

    // The exempt variable face for contrast (:47-53).
    let f = Fixture::parse("void f() {\n  (int,) r = (3);\n}\n");
    assert!(f.run(&one_rule("unnecessary_parenthesis")).is_empty());
}

/// The type-literal inner face (`test_typeLiteralInside`):
/// `(List<int>).toString()` keeps the parentheses (:105-106), while a
/// same-file generic tear-off is a `FunctionReference` and reports.
#[test]
fn type_literal_inner_face() {
    let f = Fixture::parse("void f() {\n  (List<int>).toString();\n}\n");
    assert!(f.run(&one_rule("unnecessary_parenthesis")).is_empty());

    let f = Fixture::parse("int g<T>(T x) => 0;\nvoid f() {\n  (g<int>).foo;\n}\n");
    assert_eq!(f.run(&one_rule("unnecessary_parenthesis")).len(), 1);
}

/// The extension-nullable bail (`test_targetOfGetterInNullableExtension`
/// / `test_targetOfMethodInNullableExtension`): parentheses are
/// required when the accessor resolves to a same-file extension member
/// whose `on` type is written nullable (:117-124, :132-139); a
/// non-nullable `on` type keeps the report.
#[test]
fn nullable_extension_faces() {
    let f = Fixture::parse(
        "class C {\n  String? get s => 'yay';\n}\nextension on String? {\n  bool get g => false;\n}\nvoid f(C? c) {\n  (c?.s).g;\n}\n",
    );
    assert!(f.run(&one_rule("unnecessary_parenthesis")).is_empty());

    let f = Fixture::parse(
        "class C {\n  String? get s => 'yay';\n}\nextension on String? {\n  bool g() => false;\n}\nvoid f(C? c) {\n  (c?.s).g();\n}\n",
    );
    assert!(f.run(&one_rule("unnecessary_parenthesis")).is_empty());

    let f = Fixture::parse(
        "extension on String {\n  bool get g => false;\n}\nvoid f(String s) {\n  (s).g;\n}\n",
    );
    assert_eq!(f.run(&one_rule("unnecessary_parenthesis")).len(), 1);
}

/// The dual-nullability face: two same-named members judge
/// independently through the resolved receiver — the `String?`-on-type
/// member applies alone to a nullable receiver and keeps the parens
/// (:117-124, :132-139); both apply to a non-nullable receiver, the
/// non-nullable `on` type is the more specific resolution, and the
/// one-token report stands (:108-161).
#[test]
fn dual_nullability_same_name_faces() {
    let f = Fixture::parse(
        "extension A on String? {\n  bool get g => false;\n\n  bool m() => false;\n}\nextension B on String {\n  bool get g => false;\n\n  bool m() => false;\n}\nvoid f(String? s, String t) {\n  (s).g;\n  (s).m();\n  (t).g;\n  (t).m();\n}\n",
    );
    let lints = f.run(&one_rule("unnecessary_parenthesis"));
    assert_eq!(lints.len(), 2, "{lints:?}");
    // Both reports land on the non-nullable receiver's parens (after
    // the two `m()` declarations and the `(s)` faces).
    let third_paren = tok(&f.source, &f.tokens, "(", 6);
    let fourth_paren = tok(&f.source, &f.tokens, "(", 7);
    assert_eq!(lints[0].range.start, utf16_at(&f, third_paren));
    assert_eq!(lints[1].range.start, utf16_at(&f, fourth_paren));
}

/// The interface-member precedence: an inherited same-file class
/// member resolves before any extension application (the oracle's
/// resolve-then-judge order) and reports even when a nullable
/// extension declares the same name over the subclass.
#[test]
fn class_member_precedes_extension() {
    let f = Fixture::parse(
        "class A {\n  bool get g => false;\n}\nclass B extends A {}\nextension on B? {\n  bool get g => false;\n}\nvoid f(B b) {\n  (b).g;\n}\n",
    );
    assert_eq!(f.run(&one_rule("unnecessary_parenthesis")).len(), 1);
}

/// Extensions never apply to `dynamic` (the oracle's accessor element
/// is null): the one-token face reports even when a same-file
/// extension declares the name over a nullable on type.
#[test]
fn dynamic_receiver_reports() {
    let f = Fixture::parse(
        "extension on String? {\n  bool get g => false;\n}\nvoid f(dynamic d) {\n  (d).g;\n}\n",
    );
    assert_eq!(f.run(&one_rule("unnecessary_parenthesis")).len(), 1);
}
