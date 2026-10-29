//! The #2459 pins: the cascade-resolution leakage family A3 of the
//! analyzer-corpus census — 27 wrong rows across drift/html/dio, three
//! shapes sharing one owning area.
//!
//! - (1) a cascade section's RHS member chain resolved against the latched
//!   CASCADE TARGET instead of its own receiver: the merge-time cascade
//!   latch (S4-B) substitutes the target verbatim for every member merge
//!   whose popped row is a leftover at section level, so
//!   `..sourceSpan = token.span` reported `undefined_getter 'span'` on the
//!   target's type (the html `treebuilder.dart` fourteen rows). Pass 2 now
//!   detects the substitution (the recorded receiver equals the newest
//!   section read's receiver exactly) and re-derives the receiver from the
//!   flow read type at the recorded receiver span.
//! - (2) an index-section cascade binds the `[]` fold to the previous
//!   section's RHS leftover row (`JSObject()..['k'] = s.toJS` folded the
//!   next bracket onto the `toJS` getter row), so the extension getter
//!   behind the leftover reported a phantom miss (the drift
//!   `wasm_setup/protocol.dart` eleven rows). The misfolded row now
//!   answers the target's own `[]`/`[]=` question at the bracket and then
//!   resolves its pre-fold read face.
//! - (3) the same misfold over an index-assign section reported
//!   `operator '[]=' isn't defined for the type 'Object'` — the leftover
//!   `error: Object` parameter's type, never the cascade target (the dio
//!   `compute_io.dart` row). The target question resolves through the
//!   section reads or the section-1 target fold, so a genuinely missing
//!   operator reports per section against the TARGET's type.
//!
//! The rigs declare their own classes for the primitive positions (the
//! bare one-library rig carries no dart:core, where every annotation
//! spells `undefined_class` noise over the pin's face); the js_interop
//! `toJS` getters ride plain declared extensions per the census's own
//! modeling.
//!
//! Oracle provenance: /tmp/probe2459/rig — `p1`/`p2`/`p3`/`p5` (oracle
//! silent on the RHS chain, the extension getter inside the index cascade,
//! the `List` index-assign cascade, and the selector-section index
//! regressions), `p7` (oracle reports `undefined_operator '[]='` on the
//! target's type at BOTH brackets of `x..[0] = s..[1] = 2` — never the
//! leftover's type), `p8`/`p9`/`p10` (creation and null-aware targets, the
//! `!`-chain RHS).

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

/// The one-library rig: `source` is the sole analyzed file of
/// `package:t`, no imports.
fn unit_report(source: &str) -> FileReport {
    let strings = StringTable::new();
    let mut files = FileTable::new();
    let file = files.intern(Path::new("main.dart"));
    let set = FeatureSet::for_language_version(LanguageVersion::new(3, 15));
    let features = ParserFeatures::from_feature_set(set);
    let scan_cfg = ScanConfig {
        enable_triple_shift: true,
        enable_augmentations: true,
    };
    let scanned = dartq_scanner::scan(source, scan_cfg, file);
    assert!(
        scanned.diagnostics.is_empty(),
        "scanner diagnostics: {source:?}"
    );
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

fn code_names(report: &FileReport) -> Vec<&'static str> {
    report
        .diagnostics
        .iter()
        .map(|d| d.code.name)
        .collect::<Vec<_>>()
}

/// (1) the html `treebuilder.dart` shape: the cascade RHS's `token.data` /
/// `token.span` resolve through the token parameter — no phantom member
/// row names the cascade target (`Doc`), whose own members lack both
/// selectors (probed p1: oracle silent).
#[test]
fn cascade_rhs_member_chain_resolves_through_own_receiver() {
    let report = unit_report(
        "\
class Span {}

class Token {
  Span data = Span();
  Span span = Span();
}

class Doc {
  Span? attributes;
  Span? sourceSpan;
}

void insertDoctype(Token token) {
  final doctype = Doc()
    ..attributes = token.data
    ..sourceSpan = token.span;
  insertDoctype(doctype); // local used
}
",
    );
    assert!(
        report.diagnostics.is_empty(),
        "{:?}",
        report
            .diagnostics
            .iter()
            .map(|d| (&d.code.name, &d.message))
            .collect::<Vec<_>>()
    );
}

/// (1) the same face through a `!` chain: the RHS receiver's own static
/// type owns the verdict (probed p10: oracle silent).
#[test]
fn cascade_rhs_bang_chain_resolves_through_own_receiver() {
    let report = unit_report(
        "\
class Span {}

class Token {
  Span span = Span();
}

class Doc {
  Span? sourceSpan;
}

Token? token;

void f() {
  final d = Doc()..sourceSpan = token!.span;
  d;
}
",
    );
    assert!(
        report.diagnostics.is_empty(),
        "{:?}",
        report
            .diagnostics
            .iter()
            .map(|d| (&d.code.name, &d.message))
            .collect::<Vec<_>>()
    );
}

/// (1) the correction reports the honest miss when only the RECEIVER lacks
/// the member: `token.missing` answers on `Token` even though the cascade
/// target declares the name (the oracle's `undefined_getter` on the
/// receiver type — the under-report direction of the same leak).
#[test]
fn cascade_rhs_miss_names_the_receiver_type() {
    let report = unit_report(
        "\
class Span {}

class Token {
  Span span = Span();
}

class Doc {
  Span? missing;
}

void f(Token token) {
  Doc()..missing = token.missing;
}
",
    );
    assert_eq!(code_names(&report), ["undefined_getter"]);
    assert!(
        report.diagnostics.iter().any(|d| d
            .message
            .contains("'missing' isn't defined for the type 'Token'")),
        "{:?}",
        report
            .diagnostics
            .iter()
            .map(|d| &d.message)
            .collect::<Vec<_>>()
    );
}

/// (2) the drift `wasm_setup/protocol.dart` shape: the extension getter on
/// the RHS resolves inside the index-section cascade — the second
/// section's fold lands on the `toJS` getter row and must not turn it
/// into a phantom `undefined_getter` on the receiver's type (probed p2:
/// oracle silent, with the js_interop getters modeled as plain declared
/// extensions on the receiver classes).
#[test]
fn extension_getter_in_index_cascade_resolves() {
    let report = unit_report(
        "\
class Obj {
  void operator []=(k, v) {}
}

class Str {}

extension StrToJs on Str {
  Str get toJS => this;
}

class Flag {}

extension FlagToJs on Flag {
  Flag get toJS => this;
}

void f(Str s, Flag b) {
  final o = Obj()
    ..['k'] = s.toJS
    ..['b'] = b.toJS;
  o;
}
",
    );
    assert!(
        report.diagnostics.is_empty(),
        "{:?}",
        report
            .diagnostics
            .iter()
            .map(|d| (&d.code.name, &d.message))
            .collect::<Vec<_>>()
    );
}

/// (2)+(1) composed: the latched getter (`..p = original.toJS` would
/// answer `toJS` on the cascade target) is ALSO the row the next
/// `..[` section folds onto — the receiver re-derivation and the misfold
/// demotion must compose into one silent resolution (oracle silent).
#[test]
fn latched_getter_folded_by_next_section_stays_silent() {
    let report = unit_report(
        "\
class T {}

extension OnT on T {
  T get toJS => this;
}

class N {}

class C {
  N p = N();
  void operator []=(i, v) {}
}

void f(T original) {
  C()
    ..p = original.toJS
    ..[0] = 1;
}
",
    );
    assert!(
        report.diagnostics.is_empty(),
        "{:?}",
        report
            .diagnostics
            .iter()
            .map(|d| (&d.code.name, &d.message))
            .collect::<Vec<_>>()
    );
}

/// (3) the dio `compute_io.dart` shape: the second index-assign section
/// folds onto the `error` leftover row — the leftover keeps its plain
/// read face and the `L` target owns the `[]=` question, so no
/// `operator '[]=' isn't defined for the type 'Arg'` row (probed p3's
/// class: oracle silent when the target defines the operator).
#[test]
fn index_assign_leftover_row_stays_silent() {
    let report = unit_report(
        "\
class L {
  void operator []=(i, v) {}
}

class Arg {}

void f(Arg error, Arg stack) {
  final l = L()
    ..[0] = error
    ..[1] = stack;
  l;
}
",
    );
    assert!(
        report.diagnostics.is_empty(),
        "{:?}",
        report
            .diagnostics
            .iter()
            .map(|d| (&d.code.name, &d.message))
            .collect::<Vec<_>>()
    );
}

/// (3) the creation-target form of the same shape (the corpus's
/// `List<dynamic>.filled(3, null)` with the creation modeled by a named
/// constructor): the creation leaves no readable row for the first
/// section's fold and the leftover-folded second stays silent (oracle
/// silent — the target defines `[]=`).
#[test]
fn creation_target_index_assign_cascade_stays_silent() {
    let report = unit_report(
        "\
class L {
  L.filled(i, v);
  void operator []=(i, v) {}
}

class Arg {}

void f(Arg error, Arg stack) {
  final l = L.filled(3, null)
    ..[0] = error
    ..[1] = stack;
  l;
}
",
    );
    assert!(
        report.diagnostics.is_empty(),
        "{:?}",
        report
            .diagnostics
            .iter()
            .map(|d| (&d.code.name, &d.message))
            .collect::<Vec<_>>()
    );
}

/// (3) the oracle's own report arm: a target without `[]=` reports
/// `undefined_operator '[]='` at EVERY section's bracket against the
/// TARGET's type — never the leftover's (`Str2`) — probed p7:
/// `x..[0] = s..[1] = 2` yields exactly two rows, both on the target
/// class.
#[test]
fn missing_index_operator_reports_per_section_on_target() {
    let report = unit_report(
        "\
class Num {}

class Str2 {}

void f() {
  Num x = Num();
  Str2 s = Str2();
  x..[0] = s..[1] = 2;
}
",
    );
    assert_eq!(
        code_names(&report),
        ["undefined_operator", "undefined_operator"]
    );
    for diagnostic in &report.diagnostics {
        assert!(
            diagnostic
                .message
                .contains("'[]=' isn't defined for the type 'Num'"),
            "{:?}",
            report
                .diagnostics
                .iter()
                .map(|d| &d.message)
                .collect::<Vec<_>>()
        );
    }
}

/// Regression guard for the misfold probe: a selector section's own index
/// (`..b[0]`) and its write form (`..b[0] = 1`) keep their legitimate
/// folds — every bracket resolves on the section's member type (probed
/// p5: oracle silent; the plain-read form `..b[0]` resolves `[]` on the
/// section's type).
#[test]
fn selector_section_index_faces_stay_silent() {
    let report = unit_report(
        "\
class Box {
  operator [](i) => this;
  void operator []=(i, v) {}
}

class A {
  Box b = Box();
}

void f(A a) {
  a..b[0];
  a..b[0] = 1;
}
",
    );
    assert!(
        report.diagnostics.is_empty(),
        "{:?}",
        report
            .diagnostics
            .iter()
            .map(|d| (&d.code.name, &d.message))
            .collect::<Vec<_>>()
    );
}

/// The S4-B latch keeps its intended face: the section selectors after an
/// RHS leftover still resolve against the cascade target (`..maxRedirects`
/// answers on `C`, not on the `version` RHS's type) — the correction must
/// never touch a selector row (oracle silent).
#[test]
fn section_selector_keeps_latched_target() {
    let report = unit_report(
        "\
class Str {}

class C {
  Str version = Str();
  N maxRedirects = N();
}

class N {}

void f(Str version) {
  C()..version = version..maxRedirects = 42;
}
",
    );
    assert!(
        report.diagnostics.is_empty(),
        "{:?}",
        report
            .diagnostics
            .iter()
            .map(|d| (&d.code.name, &d.message))
            .collect::<Vec<_>>()
    );
}
