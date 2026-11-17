//! #4011 pins: the type-free pattern binder's static type. A written-type-
//! less `var v`/`final v` binder never bound — the frame's `var_types`
//! slot stayed empty, so every later read (`v is int`) resolved the
//! `Unknown` face and the `unnecessary_type_check` family under-reported
//! the whole cluster C mass (the co19 `type_inference_A24..A30`,
//! `switch_expression_A03` rows). The binder now takes the oracle's
//! `staticType = variableTypeFromInitializerType(matchedValueType)`
//! (upstream Dart SDK source,
//! pkg/_fe_analyzer_shared/lib/src/type_inference/
//! type_analyzer.dart:588-590): the matched value type is the scrutinee
//! at a case root (behind only transparent brackets — logical-or/and
//! operands, parenthesized wrappers), the container lattice's element /
//! value / slot type inside a root container bracket (the written type
//! argument first, else the scrutinee's own container instantiation, else
//! `dynamic` for a dynamic matched value, else `Object?` — the co19
//! `type_inference_A28/A29` spec comments quote the lattice verbatim),
//! and the initializer demote (`Null` widens to `dynamic`,
//! `variableTypeFromInitializerType`, resolver.dart:1862-1869).
//!
//! Oracle provenance: every expectation below was probed against the
//! pinned oracle (`~/.dartq-oracle/3.13.7` `dart analyze --format=machine`
//! over the same standalone sources) during #4011; the pins freeze those
//! verdicts at the `(code, presence)` face.

#![allow(clippy::semicolon_if_nothing_returned)]
use crate::collector::collect;
use crate::diagnose::{FileReport, StrictModes, diagnose_with};
use crate::library::ElementIndex;
use crate::model::MicroKind;
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_parser::ParserFeatures;
use dartq_resolution::{LibraryClassModifiers, LibraryGraphBuilder, Namespaces};
use dartq_scanner::ScanConfig;
use dartq_source::{FileTable, StringTable};
use std::path::Path;

fn unit_report(source: &str) -> FileReport {
    let strings = StringTable::new();
    let mut files = FileTable::new();
    let file = files.intern(Path::new("main.dart"));
    let set = FeatureSet::for_language_version(LanguageVersion::new(3, 15));
    let features = ParserFeatures::from_feature_set(set);
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
    let elements = collect(source, &scanned.tokens, features, &strings);
    let mut builder = LibraryGraphBuilder::new(&strings);
    let lib = builder.intern("package:t/main.dart");
    for (name, decl) in elements.declarations() {
        if decl.kind == MicroKind::TypeParam {
            continue;
        }
        builder.declare(lib, name, file);
    }
    let graph = builder.finish();
    let namespaces = Namespaces::build(&graph);
    let mut index = ElementIndex::default();
    index.insert(file, elements);
    diagnose_with(
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
    )
}

fn has(report: &FileReport, name: &str) -> bool {
    report.diagnostics.iter().any(|d| d.code.name == name)
}

fn code_names(report: &FileReport) -> Vec<&str> {
    report.diagnostics.iter().map(|d| d.code.name).collect()
}

/// ① The bare case-header binder over an `int` scrutinee (the co19
/// `type_inference_A24/A25` shape): `v is int` is constant-true.
#[test]
fn bare_case_var_binds_scrutinee_static_type() {
    let report = unit_report(
        "class int {}\nvoid f(int x) {\n  switch (x) {\n    case var v:\n      if (v is int) {}\n      break;\n  }\n}\n",
    );
    assert!(
        has(&report, "unnecessary_type_check"),
        "{:?}",
        code_names(&report)
    );
}

/// ② The if-case twin of ① (`type_inference_A24_t03`).
#[test]
fn bare_if_case_var_binds_scrutinee_static_type() {
    let report = unit_report(
        "class int {}\nvoid f(int x) {\n  if (x case var v) {\n    if (v is int) {}\n  }\n}\n",
    );
    assert!(
        has(&report, "unnecessary_type_check"),
        "{:?}",
        code_names(&report)
    );
}

/// ③ The same binder over an unrelated check stays informative — the
/// bind must not over-fire (`v is String` over `int`; the oracle probe
/// `p3b` reports no row on this shape).
#[test]
fn bare_case_var_unrelated_check_stays_informative() {
    let report = unit_report(
        "class int {}\nclass String {}\nvoid f(int x) {\n  if (x case var v) {\n    if (v is String) {}\n  }\n}\n",
    );
    assert!(
        !has(&report, "unnecessary_type_check"),
        "{:?}",
        code_names(&report)
    );
}

/// ④ A parenthesized root wrapper is transparent (`type_inference_A27`):
/// `case (var v)` binds the scrutinee, not a record slot.
#[test]
fn paren_wrapped_case_var_binds_scrutinee_static_type() {
    let report = unit_report(
        "class int {}\nvoid f(int x) {\n  switch (x) {\n    case (var v):\n      if (v is int) {}\n      break;\n  }\n}\n",
    );
    assert!(
        has(&report, "unnecessary_type_check"),
        "{:?}",
        code_names(&report)
    );
}

/// ⑤ A logical-or operand keeps the scrutinee as its matched type
/// (`case 3 || var v` — the constant left arm never promotes).
#[test]
fn logical_or_operand_var_binds_scrutinee_static_type() {
    let report = unit_report(
        "class int {}\nvoid f(int x) {\n  switch (x) {\n    case 3 || var v:\n      if (v is int) {}\n      break;\n  }\n}\n",
    );
    assert!(
        has(&report, "unnecessary_type_check"),
        "{:?}",
        code_names(&report)
    );
}

/// ⑥ The typed list element (`type_inference_A28_t01`: `<num>[var a]`):
/// the written type argument wins — `a is num` is constant-true.
#[test]
fn typed_list_element_binder_binds_written_arg() {
    let report = unit_report(
        "class num {}\nclass List<T> {}\nvoid f(List<int> x) {\n  if (x case <num>[var a]) {\n    if (a is num) {}\n  }\n}\n",
    );
    assert!(
        has(&report, "unnecessary_type_check"),
        "{:?}",
        code_names(&report)
    );
}

/// ⑦ The unwritten list element (`type_inference_A28_t04`'s lattice leg
/// b): the scrutinee's own `List` instantiation answers — `List<int>`
/// gives `int`.
#[test]
fn list_element_binder_binds_scrutinee_element() {
    let report = unit_report(
        "class int {}\nclass List<T> {}\nvoid f(List<int> x) {\n  switch (x) {\n    case [var a]:\n      if (a is int) {}\n      break;\n  }\n}\n",
    );
    assert!(
        has(&report, "unnecessary_type_check"),
        "{:?}",
        code_names(&report)
    );
}

/// ⑧ The lattice's `Object?` tail over a non-container, non-dynamic
/// scrutinee (`type_inference_A28_t04`): `E` is `Object?`, so `a is int`
/// stays informative — the false-positive guard. The tail's positive leg
/// (`a is Object?` constant-true) is oracle-proven on the real-`Object`
/// shape (`switch ([] as Object) { case [var a]: … a is Object? }` —
/// exactly one `unnecessary_type_check` row, the #4011 probe `p8b`);
/// the harness's local-`Object` spelling would compare OUR interned
/// `Object?` against the local class — a shape the oracle answers
/// differently — so the positive rides the conformance corpus instead.
#[test]
fn list_element_lattice_object_q_tail_stays_informative() {
    let report = unit_report(
        "class int {}\nvoid f(int x) {\n  switch (x) {\n    case [var a]:\n      if (a is int) {}\n      break;\n  }\n}\n",
    );
    assert!(
        !has(&report, "unnecessary_type_check"),
        "{:?}",
        code_names(&report)
    );
}

/// ⑨ The nested-container guard: a binder inside an INNER list reads the
/// outer container's column (`num`), which only the outer close could
/// name — the oracle binds `Object?` there, so `a is num` stays
/// informative and the binder must not bind.
#[test]
fn nested_list_binder_stays_unbound() {
    let report = unit_report(
        "class num {}\nclass List<T> {}\nvoid f(List<num> x) {\n  if (x case <num>[[var a]]) {\n    if (a is num) {}\n  }\n}\n",
    );
    assert!(
        !has(&report, "unnecessary_type_check"),
        "{:?}",
        code_names(&report)
    );
}

/// ⑩ The list rest's own binder (`type_inference_A28_t01/t02':
/// `...final c` binds `List<num>`, never `num`).
#[test]
fn list_rest_binder_binds_list_of_element() {
    let report = unit_report(
        "class num {}\nclass List<T> {}\nvoid f(List<num> x) {\n  if (x case <num>[var a, ...var r]) {\n    if (r is List<num>) {}\n  }\n}\n",
    );
    assert!(
        has(&report, "unnecessary_type_check"),
        "{:?}",
        code_names(&report)
    );
}

/// ⑪ The map value binder (`type_inference_A29_t03`: `<String, int>{'k':
/// var a}` — the SECOND written argument is the value type).
#[test]
fn typed_map_value_binder_binds_written_value_arg() {
    let report = unit_report(
        "class int {}\nclass String {}\nclass Map<K, V> {}\nvoid f(Map<String, int> x) {\n  if (x case <String, int>{'k': var a}) {\n    if (a is int) {}\n  }\n}\n",
    );
    assert!(
        has(&report, "unnecessary_type_check"),
        "{:?}",
        code_names(&report)
    );
}

/// ⑫ The unwritten map value (`type_inference_A29_t07`'s lattice leg: the
/// scrutinee `Map<K, V>` instantiation answers `V`).
#[test]
fn map_value_binder_binds_scrutinee_value() {
    let report = unit_report(
        "class int {}\nclass String {}\nclass Map<K, V> {}\nvoid f(Map<String, int> x) {\n  switch (x) {\n    case {'k': var a}:\n      if (a is int) {}\n      break;\n  }\n}\n",
    );
    assert!(
        has(&report, "unnecessary_type_check"),
        "{:?}",
        code_names(&report)
    );
}

/// ⑬ The record slots (`type_inference_A30_t01`: `(var v1, name: var n1)`
/// over `(double, {String name})` — the positional ordinal's slot and the
/// named field's own slot).
#[test]
fn record_slots_bind_from_scrutinee_shape() {
    let report = unit_report(
        "class double {}\nclass String {}\nvoid f((double, {String name}) r) {\n  if (r case (var v1, name: var n1)) {\n    if (v1 is double) {}\n    if (n1 is String) {}\n  }\n}\n",
    );
    assert!(
        report
            .diagnostics
            .iter()
            .filter(|d| d.code.name == "unnecessary_type_check")
            .count()
            == 2,
        "{:?}",
        code_names(&report)
    );
}

/// ⑭ A `dynamic` scrutinee binds `dynamic` (the lattice's dynamic leg):
/// `v is int` stays informative.
#[test]
fn dynamic_scrutinee_binder_binds_dynamic() {
    let report = unit_report(
        "class int {}\nvoid f(dynamic x) {\n  if (x case var v) {\n    if (v is int) {}\n  }\n}\n",
    );
    assert!(
        !has(&report, "unnecessary_type_check"),
        "{:?}",
        code_names(&report)
    );
}

/// ⑮ A `Null` scrutinee demotes to `dynamic`
/// (`variableTypeFromInitializerType`: `Null` widens, never stays): `v is
/// int` stays informative.
#[test]
fn null_scrutinee_binder_demotes_to_dynamic() {
    let report = unit_report(
        "class int {}\nvoid f() {\n  switch (null) {\n    case var v:\n      if (v is int) {}\n      break;\n  }\n}\n",
    );
    assert!(
        !has(&report, "unnecessary_type_check"),
        "{:?}",
        code_names(&report)
    );
}

/// ⑯ The statement declaration pattern's initializer face: `var [a, b] =
/// <int>[1, 2]` types `a` off the written argument on the statement's own
/// path (pre-#4011 behavior, unchanged by the case-window bind); the
/// oracle agrees (`a is int` constant-true, the #4011 probe `p16`). This
/// pins that the case-window gate does NOT capture statement patterns —
/// the row here is the initializer path's, not the binder bind's.
#[test]
fn statement_list_destructure_stays_on_initializer_path() {
    let report = unit_report(
        "class int {}\nclass List<T> {}\nvoid f() {\n  var [a, b] = <int>[1, 2];\n  if (a is int) {}\n}\n",
    );
    assert!(
        has(&report, "unnecessary_type_check"),
        "{:?}",
        code_names(&report)
    );
}
