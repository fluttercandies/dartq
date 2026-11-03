//! The #3837 `unnecessary_type_check` pins — the parameterized
//! function-RHS alias whose signature nests the alias parameter inside a
//! returned function type.
//!
//! Census `unnecessary_type_check` (freshest report: 395 missing / 319
//! extra). 288 of the extras are `language/function_type` rows, and every
//! one of them is this one shape:
//!
//! ```text
//! typedef F15<T> = List<T> Function(int, [List<int> x]) Function();
//! List<int> Function(int, [List<int> x]) f15() => ...;
//! class U0<T> { List<T> Function(int, [List<int> x]) m15() => ...; }
//!
//! f15 is F15<int>      // always true — the oracle reports it
//! f15 is F15<bool>     // not provable — the oracle is silent
//! m15 is F15<int>      // `T` undecided — the oracle is silent
//! m15 is F15<bool>     // `T` undecided — the oracle is silent
//! ```
//!
//! Oracle 3.13.7 (`dart analyze --format=machine`) answers the first row
//! alone. The mechanism was not the subtype walk: `end_typedef`'s
//! `take_armed` snapshots the alias RHS's RETURN head, so the armed
//! `F15<bool>` folded through `fold_alias_ty` into an `Unknown Function()`
//! stub with no formals. `official_fn_face` then read that `Unknown`
//! return as the `dynamic` top — the reserved empty shape
//! (`dynamic Function()`) — and every later alias-aware fold was
//! stranded, because they all key on the `Named` face. The verdict then
//! asked a `dynamic Function()` target, which every operand satisfies:
//! constant-true for the whole family. `check_type_check` now hands the
//! stub over to `parameterized_alias_fn_ty` while the written `Named`
//! face still exists, and the pair asks the instantiated signature.
//!
//! Both directions are pinned: the unprovable pairs stay silent, and every
//! pair the oracle does fix at a constant keeps reporting.

use super::test_rig::{
    CORE_PRELUDE, RigImport, assert_silent, code_names, report_ops, report_ops_imports, rows,
};

/// The declarations every nested fixture shares: the corpus's own shape —
/// the alias parameter sits inside the INNER function's return type
/// (`List<T>`), while the inner formal spells the prefixed leaf the corpus
/// writes (`core.List<core.int>`).
const NESTED_PROLOGUE: &str = r#"
typedef F15<T> = List<T> Function(int, [core.List<core.int> x]) Function();

List<int> Function(int, [core.List<core.int> x]) f15() => throw 0;

class U0<T> {
  List<T> Function(int, [core.List<core.int> x]) m15() => throw 0;
"#;

/// [`NESTED_PROLOGUE`] with the two spellings of one library mounted: the
/// bare one binds the fixture's `List`/`int`, the `core.`-prefixed one
/// mirrors the corpus's `import 'dart:core' as core` — the alias RHS's
/// leaf arrives qualified while the declaration's return arrives bare, the
/// split that strands the armed-head fold.
fn nested_report(body: &str) -> super::FileReport {
    report_ops_imports(
        &format!("{NESTED_PROLOGUE}{body}\n}}\n"),
        &[
            RigImport {
                uri: "package:t/core.dart",
                prefix: None,
                source: CORE_PRELUDE,
            },
            RigImport {
                uri: "package:t/core.dart",
                prefix: Some("core"),
                source: "",
            },
        ],
    )
}

/// The three unprovable pairs: a differing nested argument (`List<int>` vs
/// `List<bool>`) and the undecided `T` of the enclosing class. The oracle
/// is silent on all three; before the fix each reported constant-true.
#[test]
fn undecidable_nested_argument_pairs_stay_silent() {
    let report = nested_report(
        r#"
  List<bool> test() => [f15 is F15<bool>, m15 is F15<int>, m15 is F15<bool>];"#,
    );
    assert_silent(&report);
}

/// The immediately true pair keeps reporting — `f15`'s declared
/// `F15<int>` IS the written `F15<int>`.
#[test]
fn identical_instantiation_keeps_reporting() {
    let report = nested_report(
        r#"
  List<bool> test() => [f15 is F15<int>];"#,
    );
    assert_eq!(code_names(&report), ["unnecessary_type_check"], "{:#?}", rows(&report));
}

/// `m15`'s declared type IS the written `F15<T>` — constant-true through
/// the class's own parameter.
#[test]
fn identical_parameterized_instantiation_keeps_reporting() {
    let report = nested_report(
        r#"
  List<bool> test() => [m15 is F15<T>];"#,
    );
    assert_eq!(code_names(&report), ["unnecessary_type_check"], "{:#?}", rows(&report));
}

/// The `is!` polarity of the constant-true pair is the constant-FALSE row
/// (one registry name, the 'false' template) — the fold fix must not
/// swallow it.
#[test]
fn negated_identical_instantiation_reports_the_false_polarity() {
    let report = nested_report(
        r#"
  List<bool> test() => [f15 is! F15<int>];"#,
    );
    assert_eq!(code_names(&report), ["unnecessary_type_check"], "{:#?}", rows(&report));
    assert!(
        rows(&report)[0].contains("always 'false'"),
        "expected the false polarity, got {:#?}",
        rows(&report)
    );
}

/// A widened nested argument stays constant-true: `List<int>` fits
/// `List<dynamic>` covariantly.
#[test]
fn widened_nested_argument_keeps_reporting() {
    let report = nested_report(
        r#"
  List<bool> test() => [f15 is F15<dynamic>];"#,
    );
    assert_eq!(code_names(&report), ["unnecessary_type_check"], "{:#?}", rows(&report));
}

/// The same family spelled WITHOUT the qualified leaf — the capture is
/// intact there and the stub never forms, so the fix must leave the
/// already-correct answers alone: the silent pair stays silent, the true
/// pair keeps its row.
#[test]
fn bare_spelled_family_keeps_its_answers() {
    let silent = report_ops(
        r#"
typedef F15<T> = List<T> Function(int, [List<int> x]) Function();

List<int> Function(int, [List<int> x]) f15() => throw 0;

class U0<T> {
  List<T> Function(int, [List<int> x]) m15() => throw 0;

  List<bool> test() => [
    f15 is F15<bool>,
    m15 is F15<int>,
    m15 is F15<bool>,
  ];
}
"#,
    );
    assert_silent(&silent);

    let true_pair = report_ops(
        r#"
typedef F15<T> = List<T> Function(int, [List<int> x]) Function();

List<int> Function(int, [List<int> x]) f15() => throw 0;

List<bool> test() => [f15 is F15<int>];
"#,
    );
    assert_eq!(
        code_names(&true_pair),
        ["unnecessary_type_check"],
        "{:#?}",
        rows(&true_pair)
    );
}
