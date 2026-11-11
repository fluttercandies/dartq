//! #4009 — the nested prefixed type argument (`core.int` inside
//! `core.List<core.int>`) must not clobber the enclosing prefixed head's
//! qualified-name latch.
//!
//! Upstream never compares prefix spellings: `NamedTypeResolver.resolve`
//! resolves `p.C` through the prefix element's scope to the same element a
//! bare `C` names (upstream Dart SDK source,
//! pkg/analyzer/lib/src/dart/resolver/named_type_resolver.dart:78, the
//! prefix branch :85-114), a typedef's aliased type is the already-resolved
//! RHS (pkg/analyzer/lib/src/summary2/types_builder.dart:320-334), and
//! `_checkAllTypeChecks` asks `isSubtypeOf` over two resolved types
//! (pkg/analyzer/lib/src/error/best_practices_verifier.dart:877/:924). The
//! collector interns the written dot-joined head instead, latched at the
//! prefix token and consumed by the completing `handle_type` — which the
//! parser fires at the PREFIX token AFTER the head's own `<...>` group. A
//! single latch slot let the argument's own prefix overwrite the enclosing
//! entry, so `core.List<core.int>` declarations interned the bare `core`
//! sentinel (args lost — a dropped two-root tree) and a function-typed
//! annotation carrying one poisoned to `Unknown`. The census mass is the
//! function_type family's prefixed FORMAL leaf (`typedef F6<T> = int
//! Function([core.List<core.int> x]) Function();`).
//!
//! Oracle provenance: `dart analyze --format=machine` (3.13.7 SDK) on
//! standalone fixtures these sources mirror byte-for-byte below their
//! import header; every `UNNECESSARY_TYPE_CHECK` row reads `Unnecessary type
//! check; the result is always 'true'.` over the whole `is` expression.

use crate::diagnose::FileReport;
use crate::diagnose::test_rig::{CORE_PRELUDE, report_ops_core_prefixed, rows};

const UTC: &str = "unnecessary_type_check";
const ALWAYS_TRUE: &str = "Unnecessary type check; the result is always 'true'.";

/// The corpus's import header: `dart:core` imported plainly AND under the
/// `core` prefix — `core.List` and `List` name ONE declaration.
const HEADER: &str = "import 'dart:core';\nimport 'dart:core' as core;\n";

/// `HEADER + body` and its report under the corpus's `dart:core`-as-`core`
/// posture ([`report_ops_core_prefixed`]).
fn core_alias_report(body: &str) -> (String, FileReport) {
    let source = format!("{HEADER}{body}");
    let report = report_ops_core_prefixed(&source);
    (source, report)
}

/// The `code` rows as `(offset, length)`, in report order.
fn spans(report: &FileReport, code: &str) -> Vec<(u32, u32)> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == code)
        .map(|d| (d.range.offset, d.range.length))
        .collect()
}

/// Exactly one `code` row over `needle`'s first occurrence (the oracle's
/// `(code, offset, length)` contract) carrying `message`.
#[track_caller]
fn assert_row(source: &str, report: &FileReport, code: &str, needle: &str, message: &str) {
    let offset = u32::try_from(source.find(needle).expect("needle in fixture")).unwrap();
    let length = u32::try_from(needle.len()).unwrap();
    let hits: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == code && d.range.offset == offset && d.range.length == length)
        .collect();
    assert_eq!(
        hits.len(),
        1,
        "expected one {code} row over {needle:?} ({offset}+{length}): {:#?}",
        rows(report)
    );
    assert_eq!(hits[0].message, message, "{:#?}", rows(report));
}

/// P1 (canary) — the prefixed RETURN position already emitted: the
/// return region's two-root shape carried its own repair.
/// Oracle: `UNNECESSARY_TYPE_CHECK|fix-a1.dart|5|21|13`.
#[test]
fn prefixed_return_leaf_alias_reports() {
    let (source, report) = core_alias_report(
        "typedef F2<T> = core.List<core.int> Function(int y, {List<Function> x});\n\
         core.List<core.int> f2(int y, {List<Function>? x}) => throw 'x';\n\
         void main() { print(f2 is F2<int>); }\n",
    );
    assert_row(&source, &report, UTC, "f2 is F2<int>", ALWAYS_TRUE);
    assert_eq!(spans(&report, UTC).len(), 1, "{:#?}", rows(&report));
}

/// P2 — the prefixed FORMAL leaf on both the alias and the operand: the
/// operand's formal interned the bare `core` sentinel and the alias's
/// function level poisoned. Oracle: `UNNECESSARY_TYPE_CHECK|fix-a2.dart|5|21|13`.
#[test]
fn prefixed_formal_leaf_alias_and_operand_report() {
    let (source, report) = core_alias_report(
        "typedef F1<T> = List<Function> Function(core.List<core.int> x);\n\
         List<Function> f1(core.List<core.int> x) => throw 'x';\n\
         void main() { print(f1 is F1<int>); }\n",
    );
    assert_row(&source, &report, UTC, "f1 is F1<int>", ALWAYS_TRUE);
    assert_eq!(spans(&report, UTC).len(), 1, "{:#?}", rows(&report));
}

/// P3 (canary) — the nested function-type return carrying prefixed leaves
/// in both its return and its named formal.
/// Oracle: `UNNECESSARY_TYPE_CHECK|fix-a3.dart|5|21|15`.
#[test]
fn prefixed_nested_function_return_alias_reports() {
    let (source, report) = core_alias_report(
        "typedef F13<T> = core.List<core.int> Function({core.List<core.int> x}) Function();\n\
         core.List<core.int> Function({core.List<core.int> x}) f13() => throw 'x';\n\
         void main() { print(f13 is F13<int>); }\n",
    );
    assert_row(&source, &report, UTC, "f13 is F13<int>", ALWAYS_TRUE);
    assert_eq!(spans(&report, UTC).len(), 1, "{:#?}", rows(&report));
}

/// P4 — a Named-RHS alias and a local operand both spelled
/// `core.List<core.int>`. Oracle: `UNNECESSARY_TYPE_CHECK|fix-a4.dart|6|9|6`.
#[test]
fn prefixed_named_rhs_alias_against_prefixed_local_reports() {
    let (source, report) = core_alias_report(
        "typedef A = core.List<core.int>;\n\
         void main() {\n  core.List<core.int> x = [];\n  print(x is A);\n}\n",
    );
    assert_row(&source, &report, UTC, "x is A", ALWAYS_TRUE);
    assert_eq!(spans(&report, UTC).len(), 1, "{:#?}", rows(&report));
}

/// P5 — the same alias against an UNPREFIXED operand isolates the alias's
/// own `var_ty` (the bare-sentinel intern).
/// Oracle: `UNNECESSARY_TYPE_CHECK|a4a.dart|6|9|6`.
#[test]
fn prefixed_named_rhs_alias_against_plain_local_reports() {
    let (source, report) = core_alias_report(
        "typedef A = core.List<core.int>;\n\
         void main() {\n  List<int> x = [];\n  print(x is A);\n}\n",
    );
    assert_row(&source, &report, UTC, "x is A", ALWAYS_TRUE);
    assert_eq!(spans(&report, UTC).len(), 1, "{:#?}", rows(&report));
}

/// P6 — the alias's prefixed FORMAL leaf against a clean operand
/// (`Iterable<int>` accepts every `List<int>` — contravariant formal).
/// Oracle: `UNNECESSARY_TYPE_CHECK|disc-3.dart|5|21|13`.
#[test]
fn prefixed_formal_leaf_alias_against_plain_operand_reports() {
    let (source, report) = core_alias_report(
        "typedef F1<T> = List<Function> Function(core.List<core.int> x);\n\
         List<Function> f1(Iterable<int> x) => throw 'x';\n\
         void main() { print(f1 is F1<int>); }\n",
    );
    assert_row(&source, &report, UTC, "f1 is F1<int>", ALWAYS_TRUE);
    assert_eq!(spans(&report, UTC).len(), 1, "{:#?}", rows(&report));
}

/// P7 — the census's function_type family itself (function_type0's
/// F6/F23): a prefixed formal leaf inside the RETURNED function type, the
/// top-level and the class-generic operands. Oracle: `…|11|11|11`,
/// `…|12|11|13`, `…|16|9|13`, `…|17|9|15`.
#[test]
fn function_type_family_prefixed_inner_formal_reports() {
    let (source, report) = core_alias_report(
        "typedef F6<T> = int Function([core.List<core.int> x]) Function();\n\
         typedef F23<T> = void Function<A>(core.List<core.int> x) Function();\n\
         int Function([core.List<core.int> x]) f6() => throw 'uncalled';\n\
         void Function<A>(core.List<core.int> x) f23() => throw 'uncalled';\n\
         class U<T> {\n\
         \x20 int Function([core.List<core.int> x]) m6() => throw 'uncalled';\n\
         \x20 void Function<A>(core.List<core.int> x) m23() => throw 'uncalled';\n\
         \x20 void t() {\n\
         \x20   print(m6 is F6<T>);\n\
         \x20   print(m23 is F23<T>);\n\
         \x20 }\n\
         }\n\
         void main() {\n\
         \x20 print(f6 is F6<int>);\n\
         \x20 print(f23 is F23<int>);\n\
         }\n",
    );
    for needle in [
        "m6 is F6<T>",
        "m23 is F23<T>",
        "f6 is F6<int>",
        "f23 is F23<int>",
    ] {
        assert_row(&source, &report, UTC, needle, ALWAYS_TRUE);
    }
    assert_eq!(spans(&report, UTC).len(), 4, "{:#?}", rows(&report));
}

/// N1 — a genuinely different signature stays silent (`int Function(int)`
/// is no `core.List<core.int> Function(int, {List<Function> x})`).
/// Oracle: no row.
#[test]
fn prefixed_alias_against_unrelated_signature_stays_silent() {
    let (_, report) = core_alias_report(
        "typedef F2<T> = core.List<core.int> Function(int y, {List<Function> x});\n\
         int f3(int y) => throw 'x';\n\
         void main() { print(f3 is F2<int>); }\n",
    );
    assert!(spans(&report, UTC).is_empty(), "{:#?}", rows(&report));
}

/// N2 — an unknown prefixed leaf stays unresolved: the fix must never let
/// `core.NoSuch` resolve silently. Oracle: `UNDEFINED_CLASS|neg-2.dart|3|16|11`
/// `Undefined class 'NoSuch'.`
#[test]
fn unknown_prefixed_leaf_keeps_undefined_class() {
    let (source, report) = core_alias_report(UNKNOWN_LEAF_ALIAS);
    assert_row(
        &source,
        &report,
        "undefined_class",
        "core.NoSuch",
        "Undefined class 'NoSuch'.",
    );
}

/// The N2 fixture: a group-less prefixed leaf naming nothing.
const UNKNOWN_LEAF_ALIAS: &str = "typedef U<T> = core.NoSuch Function();\n\
     void f() {}\n\
     void main() { print(f is U<int>); }\n";

/// N2's second oracle row: the `InvalidType` alias return accepts every
/// operand return, so `f is U<int>` is constant-true
/// (`UNNECESSARY_TYPE_CHECK|neg-2.dart|5|21|11`). Not a latch face — the
/// unprefixed twin (`typedef U<T> = NoSuch Function();`) misses the same
/// row: the is-verdict declines an alias whose function return is an
/// unresolved head instead of reading it as `InvalidType`.
#[test]
#[ignore = "routed: the is-verdict's InvalidType alias-return face (engine_core.rs), not the #4009 latch"]
fn unknown_prefixed_leaf_alias_return_is_constant_true() {
    let (source, report) = core_alias_report(UNKNOWN_LEAF_ALIAS);
    assert_row(&source, &report, UTC, "f is U<int>", ALWAYS_TRUE);
}

/// N3 (canary) — the prefixed local against the plain written target.
/// Oracle: `UNNECESSARY_TYPE_CHECK|a4b.dart|5|9|14`.
#[test]
fn prefixed_local_against_plain_target_reports() {
    let (source, report) = core_alias_report(
        "void main() {\n  core.List<core.int> x = [];\n  print(x is List<int>);\n}\n",
    );
    assert_row(&source, &report, UTC, "x is List<int>", ALWAYS_TRUE);
    assert_eq!(spans(&report, UTC).len(), 1, "{:#?}", rows(&report));
}

/// The sibling prefixed arguments (`core.Map<core.String, core.int>`) each
/// latch and complete their own entry: the outer head still joins, and the
/// alias folds to the operand's declared face.
/// Oracle: `UNNECESSARY_TYPE_CHECK|…|6|9|6`.
#[test]
fn sibling_prefixed_arguments_keep_the_outer_head() {
    let (source, report) = core_alias_report(
        "typedef M = core.Map<core.String, core.int>;\n\
         void main() {\n  Map<String, int> x = {};\n  print(x is M);\n}\n",
    );
    assert_row(&source, &report, UTC, "x is M", ALWAYS_TRUE);
    assert_eq!(spans(&report, UTC).len(), 1, "{:#?}", rows(&report));
}

/// The #3837 F15 family under the corpus wiring: the alias parameter sits
/// in the inner return while the inner formal spells the prefixed leaf.
/// Oracle (one line, `…|9|25|15` / `…|9|42|19` / `…|9|63|16` / `…|9|81|13`):
/// `f15 is F15<int>`, the widened `F15<dynamic>`, the `is!` twin (the
/// 'false' template), and `m15 is F15<T>` report; the three unprovable
/// pairs stay silent.
#[test]
fn nested_alias_family_decides_each_pair() {
    let (source, report) = core_alias_report(
        "typedef F15<T> = List<T> Function(int, [core.List<core.int> x]) Function();\n\
         List<int> Function(int, [core.List<core.int> x]) f15() => throw 0;\n\
         class U0<T> {\n\
         \x20 List<T> Function(int, [core.List<core.int> x]) m15() => throw 0;\n\
         \x20 List<bool> test() => [f15 is F15<int>, f15 is F15<dynamic>, f15 is! F15<int>, \
         m15 is F15<T>, f15 is F15<bool>, m15 is F15<int>, m15 is F15<bool>];\n\
         }\n",
    );
    for needle in ["f15 is F15<int>", "f15 is F15<dynamic>", "m15 is F15<T>"] {
        assert_row(&source, &report, UTC, needle, ALWAYS_TRUE);
    }
    assert_row(
        &source,
        &report,
        UTC,
        "f15 is! F15<int>",
        "Unnecessary type check; the result is always 'false'.",
    );
    assert_eq!(spans(&report, UTC).len(), 4, "{:#?}", rows(&report));
}

// —— the generative constructor's return faces (folded from #4120) ———————
//
// `verifyReturnStatement` (upstream Dart SDK source,
// pkg/analyzer/lib/src/error/return_type_verifier.dart:57-63): a generative
// constructor's `return e;` reports `return_in_generative_constructor` at
// the expression and skips the matrix; a bare `return;` is silent. The
// `=> e` body has no such arm in `verifyExpressionFunctionBody` (:45-52) —
// the expression still asks the matrix against `thisType` — while the
// error verifier reports the whole body
// (`_checkForReturnInGenerativeConstructor`,
// pkg/analyzer/lib/src/generated/error_verifier.dart:6999-7013).

const RIGC: &str = "return_in_generative_constructor";
const RIGC_MESSAGE: &str = "Constructors can't return values.";
const ROIT: &str = "return_of_invalid_type";

/// The rig fixture's full source (the prelude rides ahead of it) and its
/// report.
fn rig_report(fixture: &str) -> (String, FileReport) {
    (
        format!("{CORE_PRELUDE}{fixture}"),
        crate::diagnose::test_rig::report_ops(fixture),
    )
}

/// The `(offset, length)` of `inner` inside `outer`'s first occurrence.
fn span_in(source: &str, outer: &str, inner: &str) -> (u32, u32) {
    let at = source.find(outer).expect("outer needle in fixture")
        + outer.find(inner).expect("inner needle in outer");
    (
        u32::try_from(at).unwrap(),
        u32::try_from(inner.len()).unwrap(),
    )
}

/// Every return-family row as `(code, offset, length, message)`.
fn return_rows(report: &FileReport) -> Vec<(&'static str, u32, u32, String)> {
    report
        .diagnostics
        .iter()
        .filter(|d| {
            d.code.name == RIGC || d.code.name == ROIT || d.code.name == "return_without_value"
        })
        .map(|d| {
            (
                d.code.name,
                d.range.offset,
                d.range.length,
                d.message.to_string(),
            )
        })
        .collect()
}

/// `C() { return 3; }`. Oracle: `RETURN_IN_GENERATIVE_CONSTRUCTOR|…|1|24|1`
/// alone — never the matrix's `return_of_invalid_type`.
#[test]
fn generative_block_return_reports_at_the_expression() {
    let (source, report) = rig_report("class C { C() { return 3; } }\n");
    let (offset, length) = span_in(&source, "return 3;", "3");
    assert_eq!(
        return_rows(&report),
        [(RIGC, offset, length, RIGC_MESSAGE.to_string())],
        "{:#?}",
        rows(&report)
    );
}

/// `C() { return; }` — the arm fires only on a present expression; the
/// bare-return gate never runs. Oracle: no row.
#[test]
fn generative_bare_return_stays_silent() {
    let (_, report) = rig_report("class C { C() { return; } }\n");
    assert!(return_rows(&report).is_empty(), "{:#?}", rows(&report));
}

/// `C() => 3;` — both faces: the body row over `=> 3;` and the matrix's
/// row at the expression. Oracle: `RETURN_IN_GENERATIVE_CONSTRUCTOR|…|1|15|5`
/// and `RETURN_OF_INVALID_TYPE|…|1|18|1`.
#[test]
fn generative_arrow_body_reports_both_faces() {
    let (source, report) = rig_report("class C { C() => 3; }\n");
    let (body, body_len) = span_in(&source, "C() => 3;", "=> 3;");
    let (expr, expr_len) = span_in(&source, "C() => 3;", "3");
    assert_eq!(
        return_rows(&report),
        [
            (RIGC, body, body_len, RIGC_MESSAGE.to_string()),
            (
                ROIT,
                expr,
                expr_len,
                "A value of type 'int' can't be returned from the constructor 'C' because it \
                 has a return type of 'C'."
                    .to_string()
            ),
        ],
        "{:#?}",
        rows(&report)
    );
}

/// The named arrow constructor keeps its qualified display (`C.named`)
/// beside the body row. Oracle: `RETURN_IN_GENERATIVE_CONSTRUCTOR|…|1|21|5`
/// and `RETURN_OF_INVALID_TYPE|…|1|24|1`.
#[test]
fn generative_named_arrow_body_keeps_the_qualified_display() {
    let (source, report) = rig_report("class C { C.named() => 3; }\n");
    let (body, body_len) = span_in(&source, "C.named() => 3;", "=> 3;");
    let (expr, expr_len) = span_in(&source, "C.named() => 3;", "3");
    assert_eq!(
        return_rows(&report),
        [
            (RIGC, body, body_len, RIGC_MESSAGE.to_string()),
            (
                ROIT,
                expr,
                expr_len,
                "A value of type 'int' can't be returned from the constructor 'C.named' \
                 because it has a return type of 'C'."
                    .to_string()
            ),
        ],
        "{:#?}",
        rows(&report)
    );
}

/// A factory is no generative constructor: its `return e;` keeps the
/// matrix (`factory C() { return 3; }` — the oracle's
/// `return_of_invalid_type` against the container face).
#[test]
fn factory_block_return_keeps_the_matrix() {
    let (_, report) = rig_report("class C { factory C() { return 3; } }\n");
    let codes: Vec<_> = return_rows(&report).into_iter().map(|row| row.0).collect();
    assert_eq!(codes, [ROIT], "{:#?}", rows(&report));
}

// —— The context-free arrow literal's own type ———————————————————————————
//
// `var g = () => 0` declares `g` with the initializer's static type, `int
// Function()` — the literal's written-or-`dynamic` formals and its `=>`
// body's type (`FunctionExpressionImpl.staticType` under no imposed
// context). Without the bind the local stayed `Unknown`, so every verdict
// on it suppressed.

/// Oracle 3.13.7 on the same body: `RETURN_OF_INVALID_TYPE_FROM_CLOSURE` at
/// `k = () => 1`'s `1` (`k` is `String Function()`), `UNNECESSARY_TYPE_CHECK`
/// over `g is int Function()` and `h is int Function(int)`; the unrelated
/// `m is String Function()` stays silent.
#[test]
fn context_free_arrow_literal_types_its_var_local() {
    let (source, report) = rig_report(
        "void f() {\n\
         \x20 var g = () => 0;\n\
         \x20 g is int Function();\n\
         \x20 var h = (int x) => x;\n\
         \x20 h is int Function(int);\n\
         \x20 var k = () => 'a';\n\
         \x20 k = () => 1;\n\
         \x20 k();\n\
         \x20 g() + 1;\n\
         \x20 var m = () => 0;\n\
         \x20 m is String Function();\n\
         }\n",
    );
    let mut codes: Vec<_> = report.diagnostics.iter().map(|d| d.code.name).collect();
    codes.sort_unstable();
    assert_eq!(
        codes,
        ["return_of_invalid_type_from_closure", UTC, UTC],
        "{:#?}",
        rows(&report)
    );
    assert_row(&source, &report, UTC, "g is int Function()", ALWAYS_TRUE);
    assert_row(&source, &report, UTC, "h is int Function(int)", ALWAYS_TRUE);
    let (offset, length) = span_in(&source, "k = () => 1;", "1");
    assert_eq!(
        spans(&report, "return_of_invalid_type_from_closure"),
        [(offset, length)],
        "{:#?}",
        rows(&report)
    );
}
