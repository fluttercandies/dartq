//! The #2471 pins: the switch return-exhaustiveness face that
//! `body_might_complete_normally` reads — a function whose body is a
//! `switch` over an exhaustively-handled sealed scrutinee with every case
//! returning cannot complete normally, so no row reports
//! (`checkForBodyMayCompleteNormally`,
//! upstream Dart SDK source, pkg/analyzer/lib/src/dart/resolver/
//! type_analyzer.dart:639-704; the flow verdict is
//! `TypeSystem.isAlwaysExhaustive(scrutineeType)` —
//! type_system.dart:846-888's sealed arm, an ELEMENT question that reads
//! the scrutinee's declaration, not its case heads or type arguments).
//!
//! The #2471 gap this file pins: `this` inside a generic `extension`
//! (`extension Sub<S> on A<S>`) carries the written `on` type, whose
//! argument-bearing spelling the flow layer's staging seam could not
//! spell — the scrutinee stayed unknown and the verdict under-reported
//! `false`, over-firing the row. The recorder now captures the extension
//! header's `on`-type head (argument-bearing spellings drop to the bare
//! head) and stages `this` with it, and a `final that = this;` initializer
//! adopts that face, so the sealed-scrutinee switch closes. Probes:
//! `/tmp/probe2471/f1..f10` — oracle `dart analyze` 3.13.1 silent on every
//! closing fixture, reporting on the open-class guard (f5, `f`'s name at
//! 4:3). The census faces this closes: riverpod `async_value.dart:148`,
//! `provider_subscription.dart:50`. The census faces it does NOT (their
//! root causes live in the switch pattern-space and Never-send faces —
//! handed back): `mutations.dart:360`, `provider_subscription.dart:58`,
//! drift `engines.dart:570`, args `command_runner.dart:129`,
//! http `browser_client.dart:162`, riverpod `async_value.dart:524`,
//! dio `compute_io.dart:37` (intl's row closed earlier through the
//! implicit-call return typing).
//!
//! The harness is the #2450 one: the sole file of a library folded with
//! the resolved `TypeOps` mounted, diagnostics read off the joined
//! report.

use super::*;
use crate::collector::collect;
use crate::resolved_ops::{LibraryTypes, ResolvedTypeOps};
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_resolution::{LibraryClassModifiers, LibraryGraphBuilder};
use dartq_scanner::ScanConfig;
use dartq_source::FileTable;
use std::path::Path;

fn scan(source: &str, file: FileId) -> (dartq_scanner::ScanResult, ParserFeatures, FeatureSet) {
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
    (scanned, features, set)
}

/// Diagnoses `source` as the sole file of a library with the resolved
/// `TypeOps` face mounted — the engine path the switch exhaustiveness
/// verdict decides through.
fn report_ops(source: &str) -> FileReport {
    let strings = StringTable::new();
    let mut files = FileTable::new();
    let file = files.intern(Path::new("main.dart"));
    let (scanned, features, set) = scan(source, file);
    let elements = collect(source, &scanned.tokens, features, &strings);
    let mut builder = LibraryGraphBuilder::new(&strings);
    let lib = builder.intern("package:t/main.dart");
    builder.unit(lib, file);
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
    let types = LibraryTypes::build(&graph, &namespaces, &index, &strings, lib);
    let ops = ResolvedTypeOps::new(&types, &strings, file);
    let fold = fold_unit_with_ops(
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
        Some(&ops),
    );
    let mut report = fold.report;
    report.diagnostics = merge_source_ordered(fold.parse_diagnostics, report.diagnostics);
    report
}

fn rows(report: &FileReport, code: &str) -> Vec<u32> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == code)
        .map(|d| d.range.offset)
        .collect()
}

// —— the generic-extension `this` scrutinee closes ——————————————————————

/// The census face (riverpod `provider_subscription.dart:50`'s twin,
/// probe f8): a getter in a generic extension reads `final that = this;`
/// and switches over it with one object-pattern case that returns. The
/// scrutinee is the written `on` type — sealed — so the switch is
/// exhaustive and the body's end is unreachable: no
/// `body_might_complete_normally` (oracle probe f8 silent).
#[test]
fn extension_this_local_scrutinee_switch_closes() {
    let source = concat!(
        "sealed class ProviderSubscription<OutT> {}\n",
        "sealed class ProviderSubscriptionImpl<OutT> extends ProviderSubscription<OutT> {\n",
        "  ProviderSubscriptionImpl();\n",
        "}\n",
        "final class ProviderProviderSubscription<StateT>\n",
        "    extends ProviderSubscriptionImpl<StateT> {}\n",
        "extension ProviderSubImpl<StateT> on ProviderSubscription<StateT> {\n",
        "  ProviderSubscriptionImpl<StateT> get impl {\n",
        "    final that = this;\n",
        "    switch (that) {\n",
        "      case ProviderSubscriptionImpl<StateT>():\n",
        "        return that;\n",
        "    }\n",
        "  }\n",
        "}\n",
    );
    let report = report_ops(source);
    assert!(
        rows(&report, "body_might_complete_normally").is_empty(),
        "the sealed on-type closes the switch: {:?}",
        report.diagnostics
    );
}

/// The direct-scrutinee twin (probe f2): `switch (this)` inside a generic
/// extension closes the same way — the staged on-type head feeds the
/// verdict without an intermediate local (oracle probe f2 silent).
#[test]
fn extension_this_scrutinee_switch_closes() {
    let source = concat!(
        "sealed class A<T> {}\n",
        "class B<T> extends A<T> {}\n",
        "extension Sub<S> on A<S> {\n",
        "  A<S> get impl {\n",
        "    switch (this) {\n",
        "      case B<S>():\n",
        "        return this;\n",
        "    }\n",
        "  }\n",
        "}\n",
    );
    let report = report_ops(source);
    assert!(
        rows(&report, "body_might_complete_normally").is_empty(),
        "the direct this scrutinee closes: {:?}",
        report.diagnostics
    );
}

/// The sealed case-head sibling (probe f10): the same generic-extension
/// shape over a sealed one-subtype hierarchy closes (oracle probe f10
/// silent).
#[test]
fn sealed_head_extension_switch_closes() {
    let source = concat!(
        "sealed class A<T> {}\n",
        "sealed class B<T> extends A<T> {}\n",
        "extension Sub<S> on A<S> {\n",
        "  B<S> get impl {\n",
        "    final that = this;\n",
        "    switch (that) {\n",
        "      case B<S>():\n",
        "        return that;\n",
        "    }\n",
        "  }\n",
        "}\n",
    );
    let report = report_ops(source);
    assert!(
        rows(&report, "body_might_complete_normally").is_empty(),
        "the sealed head shape closes: {:?}",
        report.diagnostics
    );
}

// —— keep-greens: the faces the capture must not disturb ————————————————

/// A plain method's `final that = this;` keeps closing through the
/// consumer's own bare-container staging (probe f3; oracle silent).
#[test]
fn plain_method_this_scrutinee_switch_stays_silent() {
    let source = concat!(
        "sealed class A<T> {\n",
        "  B<T> map() {\n",
        "    final that = this;\n",
        "    switch (that) {\n",
        "      case B<T>():\n",
        "        return that;\n",
        "    }\n",
        "  }\n",
        "}\n",
        "class B<T> extends A<T> {}\n",
    );
    let report = report_ops(source);
    assert!(
        rows(&report, "body_might_complete_normally").is_empty(),
        "the plain-method face stays closed: {:?}",
        report.diagnostics
    );
}

/// A non-generic extension's `this` keeps closing through the written
/// `on` type the consumer stages whole (probe f4; oracle silent).
#[test]
fn nongeneric_extension_this_scrutinee_stays_silent() {
    let source = concat!(
        "sealed class A {}\n",
        "class B extends A {}\n",
        "extension Sub on A {\n",
        "  B get impl {\n",
        "    final that = this;\n",
        "    switch (that) {\n",
        "      case B():\n",
        "        return that;\n",
        "    }\n",
        "  }\n",
        "}\n",
    );
    let report = report_ops(source);
    assert!(
        rows(&report, "body_might_complete_normally").is_empty(),
        "the non-generic extension face stays closed: {:?}",
        report.diagnostics
    );
}

// —— the reporting direction guards ——————————————————————————————————————

/// An open (non-sealed) scrutinee keeps reporting: a single returning case
/// leaves the unmatched chain live and `body_might_complete_normally`
/// anchors at the declaring function's name (oracle probe f5 reports at
/// `f`, 4:3).
#[test]
fn open_class_single_case_switch_reports_body_might() {
    let source = concat!(
        "class R {}\n",
        "class A {}\n",
        "class B extends A {}\n",
        "R f(A x) {\n",
        "  switch (x) {\n",
        "    case B():\n",
        "      return R();\n",
        "  }\n",
        "}\n",
    );
    let report = report_ops(source);
    let might = rows(&report, "body_might_complete_normally");
    let name_offset = source.find("R f(").map(|found| found + "R ".len());
    assert!(
        !might.is_empty(),
        "the open scrutinee keeps the completion live: {:?}",
        report.diagnostics
    );
    if let Some(expected) = name_offset {
        assert!(
            might.contains(&(expected as u32)),
            "body_might anchors at the declaring name at {expected}: {might:?}"
        );
    }
}

/// The census face itself (riverpod `async_value.dart:148`'s twin,
/// probe f11): a `map`-style method in a generic extension with three
/// returning object-pattern cases over `final that = this;`, the
/// subtypes declared after the method — the body cannot complete
/// normally, no row (oracle probe f11 silent).
#[test]
fn async_value_map_shape_switch_closes() {
    let source = concat!(
        "sealed class AsyncValue<ValueT> {\n",
        "  NewT map<NewT>(\n",
        "    NewT Function() data,\n",
        "    NewT Function() error,\n",
        "    NewT Function() loading,\n",
        "  ) {\n",
        "    final that = this;\n",
        "    switch (that) {\n",
        "      case AsyncLoading<ValueT>():\n",
        "        return loading();\n",
        "      case AsyncData<ValueT>():\n",
        "        return data();\n",
        "      case AsyncError<ValueT>():\n",
        "        return error();\n",
        "    }\n",
        "  }\n",
        "}\n",
        "final class AsyncData<ValueT> extends AsyncValue<ValueT> {\n",
        "  AsyncData();\n",
        "}\n",
        "final class AsyncLoading<ValueT> extends AsyncValue<ValueT> {\n",
        "  AsyncLoading();\n",
        "}\n",
        "final class AsyncError<ValueT> extends AsyncValue<ValueT> {\n",
        "  AsyncError();\n",
        "}\n",
    );
    let report = report_ops(source);
    assert!(
        rows(&report, "body_might_complete_normally").is_empty(),
        "the census map shape closes: {:?}",
        report.diagnostics
    );
}
