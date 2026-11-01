//! The #3343 pins — the three const-eval residual faces #3307 routed here:
//! for-element collection literals, const dot shorthands, and the
//! wildcard-field initializer walk.
//!
//! Every pin below is an `#[ignore]`d ORACLE anchor: the row set the pinned
//! 3.13.7 `dart analyze` emits for the corpus shape, at the corpus span
//! (`expected.tsv` lines 6505-6508/906-911/1429 are these rows' machine
//! form). The faces are red by MACHINERY GAP, not by anchor arithmetic
//! inside `const_eval.rs` — each one's minimal fix lives in a file owned by
//! another lane, so the anchor lands against these pins:
//!
//! - For-element (9 corpus cases pair both rows): the oracle reports
//!   `const_eval_for_element` at the whole `for` element
//!   (evaluation.dart `_buildListConstant`: `case ForElement() =>
//!   InvalidConstant.forEntity(entity: element, locatableDiagnostic:
//!   diag.constEvalForElement)`) plus the site default
//!   `const_initialized_with_non_constant_value` at the enclosing literal
//!   (constant_verifier.dart `visitVariableDeclaration` → `_reportError`,
//!   at the entity the literal's evaluation failed on). dartq has neither
//!   row: the parser emits no for-element span event into the const
//!   capture (`begin_ctl_element` carries no span), `SemanticCode` does
//!   not register `const_eval_for_element`, and the literal's failed value
//!   push (`push_literal_value`) hands the ELEMENT's span, not the
//!   literal's, to `push_non_constant`. Routing: codes.rs (register the
//!   code), diagnose/listener.rs (a span-bearing ctl-element event),
//!   const_eval.rs (latch + fold) and const_eval_faces.rs (failed-literal
//!   re-anchor at the literal extent).
//!
//! - Dot shorthand (7 corpus cases): `.new()` / `.name` under a const
//!   declaration resolves against the CONTEXT type upstream
//!   (evaluation.dart `visitDotShorthandConstructorInvocation2` /
//!   `visitDotShorthandPropertyAccess`); with no context type the
//!   evaluation fails at the shorthand and the site default fires there.
//!   dartq's capture folds the selector to a bare name and the verdict
//!   under-reports by design (the valid-context shorthands outnumber the
//!   missing-context ones — pinned green in `const_eval_faces.rs`
//!   `dot_shorthands_stay_silent`), because deciding WHICH context failed
//!   needs context-type resolution this pass does not model. The
//!   resolution-tier rows (`dot_shorthand_undefined_member`,
//!   `dot_shorthand_missing_context`) are unregistered in codes.rs.
//!   Routing: the resolution face for the context-type answer, codes.rs
//!   for the two resolution codes, const_eval_faces.rs' shorthand gates
//!   for the ciwncv half.
//!
//! - Wildcard field initializer: `const A(this._): y = _` — the bare `_`
//!   in the initializer list resolves to the instance FIELD, not the
//!   wildcard formal (wildcard formals are non-binding; upstream
//!   `_Collector._identifier` classifies the field read not-potentially-
//!   constant, and `_reportNotPotentialConstants` emits `invalid_constant`
//!   at it beside `implicit_this_reference_in_initializer`). dartq's
//!   creation-time walk (`check_ctor_initializers`) binds the formal `_`
//!   as an opaque CtorBinding, which shadows the field and silences both
//!   rows. Routing: const_eval_faces.rs (`ctor_init_formals` /
//!   `CtorScope::ident` — a wildcard formal must not bind when the
//!   wildcards feature is enabled, `Engine::is_wildcard`).
//!
//! The loop-variable annotation of the first corpus shape drops (`var`
//! for `int`) and the wildcard shape's field type is a fixture-local
//! class: core-library names are outside this harness, and the pinned
//! faces are annotation-blind. Anchors locate themselves with
//! `source.find` — no hand-counted offsets. Registered through
//! `diagnose/tests.rs`'s `#[path]` block; the module stays inside the
//! `diagnose` subtree so the parent rig resolves.

use super::*;

/// `(code, offset, written text at the row)` — the span-exact projection:
/// code + offset + text pin a row's code, anchor, and length together.
fn oracle_rows<'a>(report: &FileReport, source: &'a str) -> Vec<(&'static str, u32, &'a str)> {
    let mut rows: Vec<(&'static str, u32, &'a str)> = report
        .diagnostics
        .iter()
        .map(|d| {
            (
                d.code.name,
                d.range.offset,
                &source[d.range.offset as usize..d.range.end() as usize],
            )
        })
        .collect();
    rows.sort();
    rows
}

/// Corpus shape `ConstEvalForElementTest/test_listLiteral#0` (upstream Dart
/// SDK source, pkg/analyzer/test/src/diagnostics/const_eval_for_element_test.dart):
/// a C-style `for` element in an implicitly const list reports at the whole
/// for element and the site default at the literal.
#[test]
#[ignore = "for-element face: needs the ctl-element span event + const_eval_for_element registration + literal-extent failed-value re-anchor"]
fn for_element_reports_at_the_element_and_the_literal() {
    let source = "const x = [for (var i = 0; i < 3; i++) i];\n";
    let for_element = source.find("for (").expect("fixture has the for element") as u32;
    let literal = source.find('[').expect("fixture has the list") as u32;
    let rep = report(source);
    assert_eq!(
        oracle_rows(&rep, source),
        vec![
            ("const_eval_for_element", for_element, "for (var i = 0; i < 3; i++) i"),
            (
                "const_initialized_with_non_constant_value",
                literal,
                "[for (var i = 0; i < 3; i++) i]",
            ),
        ],
        "{:#?}",
        oracle_rows(&rep, source)
    );
}

/// Corpus shape `ConstEvalForElementTest/test_mapLiteral_forIn_nested#0`: a
/// for element wrapped in an if element reports at the for element ALONE
/// (the if wrapper contributes nothing), site default at the literal.
#[test]
#[ignore = "for-element face: same machinery gap as the flat shape, plus the ctl-nesting anchor"]
fn nested_for_element_reports_at_the_for_element_alone() {
    let source = "const x = {if (true) for (final i in const []) i: null};\n";
    let for_element = source.find("for (").expect("fixture has the for element") as u32;
    let literal = source.find('{').expect("fixture has the map") as u32;
    let rep = report(source);
    assert_eq!(
        oracle_rows(&rep, source),
        vec![
            (
                "const_eval_for_element",
                for_element,
                "for (final i in const []) i: null",
            ),
            (
                "const_initialized_with_non_constant_value",
                literal,
                "{if (true) for (final i in const []) i: null}",
            ),
        ],
        "{:#?}",
        oracle_rows(&rep, source)
    );
}

/// Corpus shape `ConstantVisitorTest/test_dotShorthand_missingContext_invocation#0`:
/// `.new()` with no context type is a known-invalid evaluation at the
/// shorthand (site default at `.new()`), beside the resolution face at the
/// bare selector.
#[test]
#[ignore = "dot-shorthand face: the ciwncv half needs the context-type answer, the undefined-member row needs codes.rs registration"]
fn shorthand_new_without_context_reports_at_the_shorthand() {
    let source = "const a = .new();\n";
    let shorthand = source.find(".new()").expect("fixture has the shorthand") as u32;
    let rep = report(source);
    assert_eq!(
        oracle_rows(&rep, source),
        vec![
            ("dot_shorthand_undefined_member", shorthand + 1, "new"),
            (
                "const_initialized_with_non_constant_value",
                shorthand,
                ".new()",
            ),
        ],
        "{:#?}",
        oracle_rows(&rep, source)
    );
}

/// Corpus shape `ConstantVisitorTest/test_dotShorthand_missingContext_propertyAccess#0`:
/// the property spelling reports the missing-context face at the whole
/// shorthand and the site default on the same span.
#[test]
#[ignore = "dot-shorthand face: same context-type gap; the missing-context row needs codes.rs registration"]
fn shorthand_property_without_context_reports_at_the_shorthand() {
    let source = "const a = .id;\n";
    let shorthand = source.find(".id").expect("fixture has the shorthand") as u32;
    let rep = report(source);
    assert_eq!(
        oracle_rows(&rep, source),
        vec![
            ("dot_shorthand_missing_context", shorthand, ".id"),
            ("const_initialized_with_non_constant_value", shorthand, ".id"),
        ],
        "{:#?}",
        oracle_rows(&rep, source)
    );
}

/// Corpus shape `ConstantVisitorTest/test_dotShorthand_equalEqual_constructor_lhsShorthand#0`:
/// the shorthand LHS of `==` fails first (left-to-right evaluation), so the
/// site default anchors at the shorthand, not the binary.
#[test]
#[ignore = "dot-shorthand face: same context-type gap on the binary's LHS arm"]
fn shorthand_lhs_of_equal_equal_reports_at_the_shorthand() {
    let source = "class A {\n  const A();\n}\n\nconst v = .new() == A();\n";
    let shorthand = source.find(".new()").expect("fixture has the shorthand") as u32;
    let rep = report(source);
    assert_eq!(
        oracle_rows(&rep, source),
        vec![
            ("dot_shorthand_undefined_member", shorthand + 1, "new"),
            (
                "const_initialized_with_non_constant_value",
                shorthand,
                ".new()",
            ),
        ],
        "{:#?}",
        oracle_rows(&rep, source)
    );
}

/// Corpus shape `ConstantVisitorTest/test_dotShorthand_equalEqual_method_error#0`:
/// the NON-const `A()` creation on the LHS fails first — both rows anchor
/// at `A()` (the LHS failure passes through the binary untouched), and the
/// shorthand RHS never reports.
#[test]
#[ignore = "dot-shorthand face: the creation-valid verdict must grade the initializer verdict at the creation's own span"]
fn non_const_creation_lhs_reports_at_the_creation() {
    let source = "class A {\n  static A method() => A();\n}\n\nconst v = A() == .method();\n";
    let creation = source.find("A() ==").expect("fixture has the LHS creation") as u32;
    let rep = report(source);
    assert_eq!(
        oracle_rows(&rep, source),
        vec![
            ("const_with_non_const", creation, "A()"),
            ("const_initialized_with_non_constant_value", creation, "A()"),
        ],
        "{:#?}",
        oracle_rows(&rep, source)
    );
}

/// Corpus shape `InstanceCreationEvaluatorTest/test_wildcard_regularInitializer_initializerList#0`
/// (upstream Dart SDK source, pkg/analyzer/test/src/dart/constant/
/// evaluation_test.dart): the initializer `y = _` reads the instance FIELD
/// `_` (the wildcard formal is non-binding), which is not potentially
/// constant — `invalid_constant` at the field read, beside the
/// implicit-this face on the same span.
#[test]
#[ignore = "wildcard face: the initializer walk must not bind a wildcard formal over the same-named instance field"]
fn wildcard_field_read_in_initializer_list_reports_at_the_read() {
    let source = "class T {}\nclass A {\n  final T _;\n  final T y;\n  const A(this._): y = _;\n}\n";
    // The read sits at the fixture's LAST `_` (the initializer RHS), never
    // at the formal's `_` earlier on the same line.
    let read = source.rfind('_').expect("fixture carries the RHS _") as u32;
    let rep = report(source);
    assert_eq!(
        oracle_rows(&rep, source),
        vec![
            ("implicit_this_reference_in_initializer", read, "_"),
            ("invalid_constant", read, "_"),
        ],
        "{:#?}",
        oracle_rows(&rep, source)
    );
}
