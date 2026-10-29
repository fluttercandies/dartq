//! The #2782 pins: the key-only-diff sweep's span/message conventions,
//! one pin per fixed code asserting the exact oracle row — same
//! `(severity, code, file)` key, the position/length/message text the
//! pinned SDK's emit site produces:
//!
//! - `unused_element`'s `{0}` is the element `displayName`: a setter's
//!   bare spelling (never `_s=`) and a written `.new` head's container
//!   alone (`E`, never `E.new`) — element.dart `displayName`.
//! - `unchecked_use_of_nullable_value` splits per access shape
//!   (messages.yaml `uncheckedMethodInvocationOfNullableValue` /
//!   `uncheckedPropertyAccessOfNullableValue`): an invocation reports the
//!   method text, a read the property text
//!   (`type_property_resolver.dart:128-148`).
//! - `unused_label` anchors at the `Label` node — the name plus its colon
//!   (dead_code_verifier.dart:177-178, `.at(label)`).
//! - `initializer_for_non_existent_field` anchors at the whole
//!   `ConstructorFieldInitializer` node (error_verifier.dart:5572-5586,
//!   `.at(initializer)`), `this`-head forms included.
//! - `return_of_invalid_type`'s host word: a getter/setter is a
//!   `PropertyAccessorElement` — `function`, never `method`
//!   (error_verifier.dart:164-166 `isFunction`); a named factory's `{2}`
//!   is the qualified `C.named` (`ConstructorElement.displayName`).
//! - `undefined_operator`'s `{1}` for a type-literal receiver is the
//!   literal's static type `Type` (the type object's own interface).
//! - `non_exhaustive_switch_expression`'s witness: an instance-shape
//!   witness always carries the constructor-form parentheses
//!   (`_BaseStaticType.witnessToDart`, static_type.dart:214-239).

use super::test_rig::{CORE_PRELUDE, report_ops, report_ops_bare};
use super::*;

/// The fixture source rides behind the rig's core prelude: every offset
/// the rig reports is the prelude's length plus the source offset.
fn at(src: &str, needle: &str) -> u32 {
    CORE_PRELUDE.len() as u32 + src.find(needle).unwrap() as u32
}

fn rows(report: &FileReport, code: &str) -> Vec<(u32, u32, String)> {
    let mut rows: Vec<(u32, u32, String)> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == code)
        .map(|d| (d.range.offset, d.range.length, d.message.to_string()))
        .collect();
    rows.sort_unstable();
    rows
}

/// The setter's `{0}` is the bare spelling — `_s`, never `_s=`.
#[test]
fn unused_element_setter_display_is_the_bare_name() {
    let src = "set _s(int v) {}\n";
    let report = report_ops_bare(src);
    let at = src.find("_s").unwrap() as u32; // the bare rig prepends no prelude
    assert_eq!(
        rows(&report, "unused_element"),
        vec![(at, 2, "The declaration '_s' isn't referenced.".to_string())],
        "{:?}",
        report.diagnostics
    );
}

/// A written `.new` head renders as the container alone — `_C`, never
/// `_C.new` (`displayName`'s `name == 'new'` arm).
#[test]
fn unused_element_unnamed_head_display_is_the_container() {
    let src = "class _C {\n  _C();\n  _C.new();\n}\n";
    let report = report_ops_bare(src);
    let new_at = src.find("new").unwrap() as u32; // bare rig: no prelude
    let class_at = src.find("_C").unwrap() as u32; // bare rig: no prelude
    assert_eq!(
        rows(&report, "unused_element"),
        vec![
            (
                class_at,
                2,
                "The declaration '_C' isn't referenced.".to_string()
            ),
            (
                new_at,
                3,
                "The declaration '_C' isn't referenced.".to_string()
            ),
        ],
        "{:?}",
        report.diagnostics
    );
}

/// A method invocation through a nullable receiver reports the method
/// variant (`The method 'foo' … invoked`), never the property text.
#[test]
fn unchecked_nullable_method_invocation_reports_the_method_text() {
    let src = concat!(
        "class C {\n",
        "  int foo() => 0;\n",
        "}\n",
        "void f(C? c) {\n",
        "  c.foo();\n",
        "}\n",
    );
    let report = report_ops(src);
    let at = at(src, "c.foo") + 2; // the member read, not the declaration head
    assert_eq!(
        rows(&report, "unchecked_use_of_nullable_value"),
        vec![(
            at,
            3,
            "The method 'foo' can't be unconditionally invoked because the receiver can be 'null'."
                .to_string()
        )],
        "{:?}",
        report.diagnostics
    );
}

/// A getter read through a nullable receiver keeps the property variant.
#[test]
fn unchecked_nullable_property_access_reports_the_property_text() {
    let src = concat!(
        "class C {\n",
        "  int get g => 0;\n",
        "}\n",
        "void f(C? c) {\n",
        "  c.g;\n",
        "}\n",
    );
    let report = report_ops(src);
    let at = at(src, "g;");
    assert_eq!(
        rows(&report, "unchecked_use_of_nullable_value"),
        vec![(
            at,
            1,
            "The property 'g' can't be unconditionally accessed because the receiver can be 'null'."
                .to_string()
        )],
        "{:?}",
        report.diagnostics
    );
}

/// The unused label's row spans the `Label` node — the name plus its
/// colon (`loop:`, length 5).
#[test]
fn unused_label_spans_the_label_and_its_colon() {
    let src = concat!(
        "void f() {\n",
        "  loop: while (true) {\n",
        "    break;\n",
        "  }\n",
        "}\n",
    );
    let report = report_ops(src);
    let at = at(src, "loop");
    assert_eq!(
        rows(&report, "unused_label"),
        vec![(at, 5, "The label 'loop' isn't used.".to_string())],
        "{:?}",
        report.diagnostics
    );
}

/// The field-initializer miss anchors at the whole
/// `ConstructorFieldInitializer` node (`x = 'foo'`, length 9).
#[test]
fn initializer_miss_spans_the_whole_initializer_node() {
    let src = concat!("class C {\n", "  C() : x = 'foo';\n", "}\n",);
    let report = report_ops(src);
    let at = at(src, "x = 'foo'");
    assert_eq!(
        rows(&report, "initializer_for_non_existent_field"),
        vec![(
            at,
            9,
            "'x' isn't a field in the enclosing class.".to_string()
        )],
        "{:?}",
        report.diagnostics
    );
}

/// A getter is a `PropertyAccessorElement` — the host word is `function`,
/// never `method` (`EnclosingExecutableContext.isFunction`).
#[test]
fn getter_return_host_word_is_function() {
    let src = concat!("class C {\n", "  String get g => 0;\n", "}\n",);
    let report = report_ops(src);
    let text =
        "A value of type 'int' can't be returned from the function 'g' because it has a return type of 'String'."
            .to_string();
    assert_eq!(
        rows(&report, "return_of_invalid_type"),
        vec![(at(src, "0"), 1, text)],
        "{:?}",
        report.diagnostics
    );
}

/// A named factory's `{2}` is the qualified `C.named`
/// (`ConstructorElement.displayName`), never the bare container.
#[test]
fn named_factory_display_is_qualified() {
    let src = concat!("class C {\n", "  factory C.named() => 7;\n", "}\n",);
    let report = report_ops(src);
    let text =
        "A value of type 'int' can't be returned from the constructor 'C.named' because it has a return type of 'C'."
            .to_string();
    assert_eq!(
        rows(&report, "return_of_invalid_type"),
        vec![(at(src, "7"), 1, text)],
        "{:?}",
        report.diagnostics
    );
}

/// A type-literal receiver's operator miss names the literal's static
/// type `Type`, not the aliased head.
#[test]
fn type_literal_operator_miss_names_the_type_interface() {
    let src = concat!("typedef c = String;\n", "void f() {\n", "  c > 1;\n", "}\n",);
    let report = report_ops(src);
    let at = at(src, ">");
    assert_eq!(
        rows(&report, "undefined_operator"),
        vec![(
            at,
            1,
            "The operator '>' isn't defined for the type 'Type'.".to_string()
        )],
        "{:?}",
        report.diagnostics
    );
}

/// The switch-expression exhaustiveness witness carries the
/// constructor-form parentheses, empty or not — `int()`, never `int`.
#[test]
fn switch_expression_witness_renders_the_empty_parens() {
    let src = concat!("void f(int x) {\n", "  var v = switch (x) {};\n", "}\n",);
    let report = report_ops(src);
    let text = "The type 'int' isn't exhaustively matched by the switch cases since it doesn't match the pattern 'int()'."
        .to_string();
    let got = rows(&report, "non_exhaustive_switch_expression");
    assert_eq!(got.len(), 1, "{got:?}");
    assert_eq!(got[0].2, text, "{got:?}");
    assert_eq!(got[0].0, at(src, "switch (x)"), "{got:?}");
    assert_eq!(got[0].1, 6, "{got:?}");
}
