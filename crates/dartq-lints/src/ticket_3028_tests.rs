use crate::testsupport::{Fixture, one_rule, tok, utf16_at};

/// The #3028 lint residual-wave pins: one green pin per fixed shape,
/// each citing the conformance row it mirrors (the pinned
/// `dart analyze` 3.13.7 oracle output in `conformance/expected.tsv`).

// —— prefer_is_empty ————————————————————————————————————————————————
// Upstream Dart SDK source, pkg/linter .../prefer_is_empty.dart
// (:97-101 `reportAtNode(expression)`; :188-208 `_drillDownTo`).

/// `test_zero_notEq_listLength#0` — `INFO|…|2:9|14` — the whole binary
/// expression, not the `;`-sealed container span.
#[test]
fn pie_zero_not_eq_list_length_span() {
    let f = Fixture::parse("var x = 0 != [].length;\n");
    let lints = f.run_semantic(&one_rule("prefer_is_empty"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "Use 'isNotEmpty' instead of 'length' to test whether the collection is empty."
    );
    let zero = tok(&f.source, &f.tokens, "0", 0);
    assert_eq!(lints[0].range.start, utf16_at(&f, zero));
    assert_eq!(lints[0].range.end - lints[0].range.start, 14);
}

/// `test_listLength_asInt_greaterThan_zero#0` — the `as` cast drills
/// through to the `.length` access (`ignoreAs`).
#[test]
fn pie_as_cast_drills_to_length() {
    let f = Fixture::parse("var x = ([].length as int) > 0;\n");
    let lints = f.run_semantic(&one_rule("prefer_is_empty"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(
        lints[0].message,
        "Use 'isNotEmpty' instead of 'length' to test whether the collection is empty."
    );
}

/// `test_stringLength_eqEq_zero_inConstructorInitializer#0` —
/// `INFO|…|4:21|13` — the initializer target leaks no longer into the
/// report span; the operand-adjacent piece anchors the start.
#[test]
fn pie_constructor_initializer_span_anchors_at_operand() {
    let f = Fixture::parse("class C {\n  final bool x;\n  C(String s) : x = s.length == 0;\n}\n");
    let lints = f.run_semantic(&one_rule("prefer_is_empty"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    let s = tok(&f.source, &f.tokens, "s", 1);
    assert_eq!(lints[0].range.start, utf16_at(&f, s));
    assert_eq!(lints[0].range.end - lints[0].range.start, 13);
}

// —— unnecessary_statements —————————————————————————————————————————
// Upstream Dart SDK source, pkg/linter .../unnecessary_statements.dart
// (:114-116 the `visitNode` report face).

/// `test_listLiteral#0` / `test_inForLoopUpdaters#0` (the `[];`
/// statement and the `[]` for-updater) stay unpinned: the empty-list
/// tiling keeps no `[` token row for the span probes — the shapes are
/// tracked as conformance residuals.

/// `test_instanceField#0` — `INFO|…|3:3|5` — an invocation receiver's
/// same-file class field tear-off (`C().g`).
#[test]
fn us_invocation_receiver_field_reports() {
    let f = Fixture::parse("void f() {\n  C().g;\n}\n\nclass C {\n  int g = 1;\n}\n");
    let lints = f.run_semantic(&one_rule("unnecessary_statements"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(lints[0].range.end - lints[0].range.start, 5);
}

// —— implicit_call_tearoffs ——————————————————————————————————————————
// Upstream Dart SDK source, pkg/linter .../implicit_call_tearoffs.dart
// (:34-42 the resolver-built `ImplicitCallReference` report face).

/// `test_ifNullExpression_functionTypeContext#0` — `INFO|…|3:10|8` —
/// the `??` operands tile beside the container; either operand's
/// class-with-`call` answers and the report covers the whole
/// expression.
#[test]
fn ict_if_null_return_reports_whole_expression() {
    let f = Fixture::parse(
        "void Function() f(C? c1, C c2) {\n  return c1 ?? c2;\n}\n\nclass C {\n  void call() {}\n}\n",
    );
    let lints = f.run_semantic(&one_rule("implicit_call_tearoffs"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    let c1 = tok(&f.source, &f.tokens, "c1", 1);
    assert_eq!(lints[0].range.start, utf16_at(&f, c1));
    assert_eq!(lints[0].range.end - lints[0].range.start, 8);
}

/// `test_simpleIdentifier_functionTypeContext#0` — `INFO|…|2:27|1` —
/// the arrow body's value tiles beside the body row; a parameter of a
/// class with `call` reports.
#[test]
fn ict_arrow_body_parameter_with_call_reports() {
    let f = Fixture::parse("void Function() f(C c) => c;\n\nclass C {\n  void call() {}\n}\n");
    let lints = f.run_semantic(&one_rule("implicit_call_tearoffs"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert_eq!(lints[0].range.end - lints[0].range.start, 1);
}

// —— deprecated_member_use_from_same_package —————————————————————————
// Upstream Dart SDK source, pkg/linter
// .../deprecated_member_use_from_same_package.dart.

/// `test_deprecatedUnnamedConstructor_newSyntax#0` — `INFO|…|7:9|1` —
/// the unnamed constructor keys on the container's type name. The
/// named-constructor selector, extension-type-name, enum-constant and
/// increment-operand shapes stay unpinned: their tilings did not
/// answer in this wave (tracked as conformance residuals).
#[test]
fn dmfs_unnamed_constructor_reports_at_type_name() {
    let f = Fixture::parse("class C {\n  @deprecated\n  new();\n}\nvar x = C();\n");
    let lints = f.run(&one_rule("deprecated_member_use_from_same_package"));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert!(
        lints[0].message.starts_with("'C' is deprecated"),
        "{:?}",
        lints[0].message
    );
}
