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
//!   at the entity the literal's evaluation failed on). dartq now reports
//!   both: the listener latches the `for` element's head (`begin_for_control_flow`
//!   → `begin_ctl_element`) and extent (`end_for_control_flow` /
//!   `end_for_in_control_flow` → `close_ctl_element`), and the owning
//!   literal's close reports `const_eval_for_element` at the element while
//!   pushing the site default at the whole literal (never the element's own
//!   body expression).
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
//!
//! Status: the three dot-shorthand pins, the non-const-creation pin and the
//! two for-element pins are green (the shorthand face lives in
//! `const_eval_faces.rs`'s `report_root_shorthand`, the creation rows in the
//! deferred creation tail pass, the for-element spans in `listener.rs` +
//! `const_eval.rs`'s ctl-element latch); the wildcard pin stays `#[ignore]`d
//! and routed to the wildcard-formal face.
//! Expected row vectors are written in [`oracle_rows`]' canonical order
//! (code name, then offset): the helper sorts, so a semantic ordering
//! ("primary row first") silently fails the equality.

use super::test_rig::{CORE_PRELUDE, report_ops_bare};
use super::*;

/// `(code, offset, written text at the row)` — the span-exact projection:
/// code + offset + text pin a row's code, anchor, and length together.
/// The vector is sorted by `(code, offset, text)`, so expected rows are
/// listed in code-name order.
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
///
/// Runs under the core-preloaded rig ([`report_ops_bare`] over
/// [`CORE_PRELUDE`]): the update `i++` resolves `int.operator +`, which the
/// bare `report` rig cannot (it declares no core members) — there the loop
/// variable's `++` would add a spurious `undefined_operator` row the oracle
/// never reports. The corpus's `int` annotation drops to `var` (an
/// annotation would draw `undefined_class int`); the face under pin is
/// annotation-blind.
#[test]
fn for_element_reports_at_the_element_and_the_literal() {
    let fixture = "const x = [for (var i = 0; i < 3; i++) i];\n";
    let source = format!("{CORE_PRELUDE}{fixture}");
    // `[for` (not `[`) — the prelude declares `operator []`.
    let literal = source.find("[for").expect("fixture has the list") as u32;
    let for_element = source.find("for (").expect("fixture has the for element") as u32;
    let rep = report_ops_bare(&source);
    assert_eq!(
        oracle_rows(&rep, &source),
        vec![
            (
                "const_eval_for_element",
                for_element,
                "for (var i = 0; i < 3; i++) i"
            ),
            (
                "const_initialized_with_non_constant_value",
                literal,
                "[for (var i = 0; i < 3; i++) i]",
            ),
        ],
        "{:#?}",
        oracle_rows(&rep, &source)
    );
}

/// Corpus shape `ConstEvalForElementTest/test_mapLiteral_forIn_nested#0`: a
/// for element wrapped in an if element reports at the for element ALONE
/// (the if wrapper contributes nothing), site default at the literal.
#[test]
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
fn shorthand_new_without_context_reports_at_the_shorthand() {
    let source = "const a = .new();\n";
    let shorthand = source.find(".new()").expect("fixture has the shorthand") as u32;
    let rep = report(source);
    assert_eq!(
        oracle_rows(&rep, source),
        vec![
            (
                "const_initialized_with_non_constant_value",
                shorthand,
                ".new()",
            ),
            ("dot_shorthand_undefined_member", shorthand + 1, "new"),
        ],
        "{:#?}",
        oracle_rows(&rep, source)
    );
}

/// Corpus shape `ConstantVisitorTest/test_dotShorthand_missingContext_propertyAccess#0`:
/// the property spelling reports the missing-context face at the whole
/// shorthand and the site default on the same span.
#[test]
fn shorthand_property_without_context_reports_at_the_shorthand() {
    let source = "const a = .id;\n";
    let shorthand = source.find(".id").expect("fixture has the shorthand") as u32;
    let rep = report(source);
    assert_eq!(
        oracle_rows(&rep, source),
        vec![
            (
                "const_initialized_with_non_constant_value",
                shorthand,
                ".id"
            ),
            ("dot_shorthand_missing_context", shorthand, ".id"),
        ],
        "{:#?}",
        oracle_rows(&rep, source)
    );
}

/// Corpus shape `ConstantVisitorTest/test_dotShorthand_equalEqual_constructor_lhsShorthand#0`:
/// the shorthand LHS of `==` fails first (left-to-right evaluation), so the
/// site default anchors at the shorthand, not the binary.
#[test]
fn shorthand_lhs_of_equal_equal_reports_at_the_shorthand() {
    let source = "class A {\n  const A();\n}\n\nconst v = .new() == A();\n";
    let shorthand = source.find(".new()").expect("fixture has the shorthand") as u32;
    let rep = report(source);
    assert_eq!(
        oracle_rows(&rep, source),
        vec![
            (
                "const_initialized_with_non_constant_value",
                shorthand,
                ".new()",
            ),
            ("dot_shorthand_undefined_member", shorthand + 1, "new"),
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
fn non_const_creation_lhs_reports_at_the_creation() {
    let source = "class A {\n  static A method() => A();\n}\n\nconst v = A() == .method();\n";
    let creation = source.find("A() ==").expect("fixture has the LHS creation") as u32;
    let rep = report(source);
    assert_eq!(
        oracle_rows(&rep, source),
        vec![
            ("const_initialized_with_non_constant_value", creation, "A()"),
            ("const_with_non_const", creation, "A()"),
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
// routed: needs listener.rs span event (residual) — no declaration-time
// scan of a field initializer's right-hand side exists (the const capture
// stashes the tree for the creation-time walk only), so the listener must
// run `collect_not_potentially_constant` over the initializer RHS at the
// list's close, with `ctor_init_formals` not binding a wildcard formal.
#[ignore = "wildcard face: the initializer walk must not bind a wildcard formal over the same-named instance field"]
fn wildcard_field_read_in_initializer_list_reports_at_the_read() {
    let source =
        "class T {}\nclass A {\n  final T _;\n  final T y;\n  const A(this._): y = _;\n}\n";
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
