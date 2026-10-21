//! The #2491 pins: a switch-case object pattern's named type must never
//! lend its type-argument group to an older expression's row.
//!
//! The pattern's head pushes its row only at `handle_object_pattern` —
//! after its group closed — so the group-close deferral found whatever
//! invocation row the previous case body left open and classified the
//! pattern's argument count against it (`wrong_number_of_type_arguments_
//! element` blaming the previous arm's callee at the pattern's `<`; census
//! rows riverpod `listenable.dart:36:24` / `element.dart:544:48`). The fix
//! anchors the deferral to a trivia-adjacent head row: a row ending before
//! the group's `<` across anything but trivia belongs to an older
//! expression, and an unanchored headless group defers against nothing.
//!
//! The oracle is silent on every pin's pattern spelling (probed 3.13.1
//! over the /tmp/probe2491 rig: the rig's four dartq-only rows — the
//! `_element` row plus three `unchecked_use_of_nullable_value` rows from
//! the pattern-promotion face, a separate lane — and silence on a
//! wrong-arity pattern head's base-code face too, which dartq does not
//! model). Send-position arity keeps its anchors: the adjacent-head
//! controls still report.

use super::*;
use crate::collector::collect;
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_resolution::LibraryGraphBuilder;
use dartq_scanner::ScanConfig;
use dartq_source::FileTable;
use std::path::Path;

/// Diagnoses `source` as the sole file of a library with no imports —
/// the single-file rig (`ticket_2472_tests` carries the same shape).
fn report(source: &str) -> FileReport {
    let strings = StringTable::new();
    let mut files = FileTable::new();
    let file = files.intern(Path::new("main.dart"));
    let scanned = dartq_scanner::scan(
        source,
        ScanConfig {
            enable_triple_shift: true,
            enable_augmentations: true,
        },
        file,
    );
    assert!(
        scanned.diagnostics.is_empty(),
        "scanner diagnostics: {source:?}"
    );
    let set = FeatureSet::for_language_version(LanguageVersion::new(3, 15));
    let features = ParserFeatures::from_feature_set(set);
    let elements = collect(source, &scanned.tokens, features, &strings);
    let mut builder = LibraryGraphBuilder::new(&strings);
    let lib = builder.intern("package:t/main.dart");
    let mut index = ElementIndex::default();
    index.insert(file, elements.clone());
    for (name, decl) in elements.declarations() {
        if decl.kind == MicroKind::TypeParam {
            continue;
        }
        builder.declare(lib, name, file);
    }
    let graph = builder.finish();
    let namespaces = Namespaces::build(&graph);
    let fold = fold_unit(
        source,
        &scanned.tokens,
        features,
        &strings,
        file,
        lib,
        &graph,
        &namespaces,
        &index,
        StrictModes::default(),
        &LibraryClassModifiers::default(),
        set.is_enabled(dartq_language::Feature::InferenceUpdate2),
    );
    let mut report = fold.report;
    report.diagnostics = merge_source_ordered(fold.parse_diagnostics, report.diagnostics);
    report
}

fn count_code(report: &FileReport, name: &str) -> usize {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == name)
        .count()
}

fn messages(report: &FileReport) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .map(|d| format!("{}: {}", d.code.name, d.message))
        .collect()
}

const RESULT: &str = "\
sealed class Result<ValueT> {
  const Result();
}

class ResultData<ValueT> implements Result<ValueT> {
  const ResultData();
  ValueT get value => const ResultData();
}

class ResultError<ValueT> implements Result<ValueT> {
  const ResultError();
  Err get error => const Err();
  Trace get stackTrace => const Trace();
}

class Err {
  const Err();
}

class Trace {
  const Trace();
}

class Listener<ValueT> {
  const Listener();
  void notifyValue(ValueT? previous, ValueT next) {}
  void notifyError(Err error, Trace stack) {}
}
";

/// The census's listenable.dart shape: each case body calls a 0-generic
/// method, and the NEXT case's pattern named type carries a written
/// argument group. The pre-fix engine anchored each pattern group to the
/// previous body's last send row and reported the method's arity at the
/// pattern's `<`; the pattern's group defers against nothing.
#[test]
fn pattern_named_type_group_never_blames_previous_case_body() {
    let source = format!(
        "{RESULT}\
class Watcher<ValueT> extends Listener<ValueT> {{
  Result<ValueT>? _result;

  set result(Result<ValueT>? value) {{
    final previous = _result;
    _result = value;

    switch (value) {{
      case null:
        break;
      case ResultData<ValueT>():
        notifyValue(previous?.value, value.value);
      case ResultError<ValueT>():
        notifyError(value.error, value.stackTrace);
    }}
  }}
}}
"
    );
    let report = report(&source);
    assert_eq!(
        count_code(&report, "wrong_number_of_type_arguments_element"),
        0,
        "{:?}",
        messages(&report)
    );
}

/// The audit's narrowing shape: two calls in one body blame the LAST
/// invocation pre-fix (the row stack's top); an unanchored group blames
/// neither.
#[test]
fn two_calls_before_pattern_case_stay_silent() {
    let source = format!(
        "{RESULT}\
class Watcher<ValueT> extends Listener<ValueT> {{
  set result(Result<ValueT>? value) {{
    switch (value) {{
      case null:
        notifyValue(value as ValueT?, value as ValueT);
        notifyValue(value as ValueT?, value as ValueT);
      case ResultData<ValueT>():
        break;
    }}
  }}
}}
"
    );
    let report = report(&source);
    assert_eq!(
        count_code(&report, "wrong_number_of_type_arguments_element"),
        0,
        "{:?}",
        messages(&report)
    );
}

/// Empty fall-through cases between the body and the firing case keep the
/// stale row reachable pre-fix; the anchor gate reads geometry, not
/// distance.
#[test]
fn empty_cases_between_body_and_pattern_stay_silent() {
    let source = format!(
        "{RESULT}\
class Watcher<ValueT> extends Listener<ValueT> {{
  set result(Result<ValueT>? value) {{
    switch (value) {{
      case null:
        notifyValue(value as ValueT?, value as ValueT);
      case ResultData<ValueT>():
      case ResultError<ValueT>():
        break;
    }}
  }}
}}
"
    );
    let report = report(&source);
    assert_eq!(
        count_code(&report, "wrong_number_of_type_arguments_element"),
        0,
        "{:?}",
        messages(&report)
    );
}

/// An if-case's pattern named type gets the same anchor: the preceding
/// send must not answer the pattern's group.
#[test]
fn if_case_pattern_after_send_stays_silent() {
    let source = format!(
        "{RESULT}\
void probe(Result<ValueT>? value) {{
  notifyValue(value?.error, value as ValueT);
  if (value case ResultData<ValueT>()) {{
    notifyValue(value.value, value.value);
  }}
}}
"
    );
    let report = report(&source);
    assert_eq!(
        count_code(&report, "wrong_number_of_type_arguments_element"),
        0,
        "{:?}",
        messages(&report)
    );
}

/// The positive control: a generic send inside a case body keeps its own
/// group — the head row is trivia-adjacent to the `<`, so the deferral
/// fires against it exactly as before. The pre-fix rig fired the same
/// shape twice (the send's own row AND the following pattern's stale
/// anchor at the NEXT case's `<`); post-fix only the send's row remains.
#[test]
fn case_body_generic_send_still_reports_once() {
    let source = format!(
        "{RESULT}\
class Watcher<ValueT> extends Listener<ValueT> {{
  set result(Result<ValueT>? value) {{
    switch (value) {{
      case null:
        notifyValue<Err>(value as ValueT?, value as ValueT);
      case ResultData<ValueT>():
        break;
    }}
  }}
}}
"
    );
    let report = report(&source);
    assert_eq!(
        count_code(&report, "wrong_number_of_type_arguments_element"),
        1,
        "{:?}",
        messages(&report)
    );
    assert_eq!(
        report.diagnostics[0].message,
        "The method 'notifyValue' is declared with 0 type parameters, but 1 type arguments are given."
    );
}

/// The comment control: a callee separated from its group by comments is
/// still the group's head (`trivia_between` sees through trivia), so the
/// arity check keeps firing across a layout the byte probe would miss.
#[test]
fn commented_head_still_anchors_its_group() {
    let report = report("class D {} void f() {} void g() { f /*c*/ <D>(); }");
    assert_eq!(
        count_code(&report, "wrong_number_of_type_arguments_element"),
        1,
        "{:?}",
        messages(&report)
    );
}
