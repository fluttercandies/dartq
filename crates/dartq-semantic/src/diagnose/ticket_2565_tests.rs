//! The #2565 pins: the promotion-latched initializer's re-bind
//! propagating down a `var` chain, the re-bind map's scope-safe keys,
//! and the implicitly typed closure formal's context re-bind.
//!
//! The parse-time bind of `var v = promoted.member` rides the UNPROMOTED
//! declared receiver (pass 1 cannot see the flow promotion), so the
//! deferred re-bind (#2539) recomputes the accessor over the promoted
//! face in pass 2. Three gaps the census's analyzer element-model family
//! exposed: the re-derivation seams never consulted the re-bound face
//! (a chain through the local re-typed every link on the stale base
//! declaration), the re-bind map keyed the scoped locals-stack index —
//! which aliases across scopes once the frames close, so one method's
//! re-bind clobbered another's — and an implicitly typed closure formal
//! (`rebound.items.each((e) => …)`) froze the pass-1 schema entry over
//! the unre-derived receiver face.
//!
//! The rig declares no imports, so every pin spells custom types only
//! (`ticket_2564_tests` carries the same rule).

use super::*;
use crate::collector::collect;
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_resolution::LibraryGraphBuilder;
use dartq_scanner::ScanConfig;
use dartq_source::FileTable;
use std::path::Path;

/// Diagnoses `source` as the sole file of a library with no imports —
/// the single-file rig (`ticket_2564_tests`' shape).
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

/// The element-model shape the pins spell: a base fragment whose
/// `element` getter answers the base element, an impl pair overriding it
/// covariantly, and an impl-only member the base face cannot resolve.
const ELEMENTS: &str = "\
abstract class El {
  String get elOnly;
}

abstract class ElImpl implements El {
  String get elOnly;
  String get implOnly;
}

abstract class Frag {
  El get element;
}

abstract class FragImpl implements Frag {
  ElImpl get element;
}
";

/// `var v = promoted.element` re-binds to the covariant override's
/// return: `v.implOnly` resolves on `ElImpl` and the oracle stays silent
/// (the pre-fix chain froze the base getter's `El` and reported).
#[test]
fn rebound_chain_local_reads_the_override_member() {
    let source = format!(
        "{ELEMENTS}\
void check(Frag f) {{
  if (f is! FragImpl) {{
    return;
  }}
  var v = f.element;
  v.implOnly;
}}
"
    );
    let result = report(&source);
    assert_eq!(
        count_code(&result, "undefined_getter"),
        0,
        "{:?}",
        messages(&result)
    );
}

/// An undefined getter on the re-bound local names the override's
/// return in the message — the receiver face the verdict walked.
#[test]
fn rebound_chain_miss_reports_the_override_face() {
    let source = format!(
        "{ELEMENTS}\
void check(Frag f) {{
  if (f is! FragImpl) {{
    return;
  }}
  var v = f.element;
  v.zz;
}}
"
    );
    let result = report(&source);
    assert_eq!(count_code(&result, "undefined_getter"), 1);
    let miss = messages(&result)
        .into_iter()
        .find(|m| m.starts_with("undefined_getter"))
        .expect("the undefined_getter row");
    assert!(miss.contains("'ElImpl'"), "{miss}");
}

/// The chain link itself re-derives over the re-bound face: a member
/// read off `v.element2`-style nesting keeps the impl answers. Spelled
/// here as the closure-formal shape — the callback's implicitly typed
/// formal takes the re-derived receiver's instantiation (`Box<ElImpl>`
/// types the formal `ElImpl`), so the impl-only member resolves and the
/// oracle stays silent. The pre-fix model froze the base schema entry
/// (`Box<El>`'s `El`) and reported.
const BOXES: &str = "\
abstract class El {
  String get elOnly;
}

abstract class ElImpl implements El {
  String get elOnly;
  String get implOnly;
}

abstract class Box<T> {
  T each(T Function(T) cb);
}

abstract class BaseHolder {
  Box<El> get items;
}

abstract class ImplHolder implements BaseHolder {
  Box<ElImpl> get items;
}
";

#[test]
fn closure_formal_types_from_the_rebound_link() {
    let source = format!(
        "{BOXES}\
void check(BaseHolder h) {{
  if (h is! ImplHolder) {{
    return;
  }}
  h.items.each((e) => e.implOnly);
}}
"
    );
    let result = report(&source);
    assert_eq!(
        count_code(&result, "undefined_getter"),
        0,
        "{:?}",
        messages(&result)
    );
}

/// The re-bind map keys the declaring local's name-token offset: the
/// scoped locals-stack index aliases across scopes once the frames
/// close, and a later method's same-index re-bind must not clobber an
/// earlier method's (the pre-fix flat key retyped the earlier method's
/// read on the later's face and reported its impl-only member).
const SCOPES: &str = "\
abstract class P1 {
  String get p1Only;
}

abstract class P1I implements P1 {
  String get p1Only;
  String get p1iOnly;
}

abstract class R1 {
  String get r1Only;
}

abstract class R1I implements R1 {
  String get r1Only;
  String get r1iOnly;
}

abstract class X {
  P1 get p;
  R1 get r;
}

abstract class XI implements X {
  P1I get p;
  R1I get r;
}
";

#[test]
fn rebind_keys_survive_cross_scope_index_reuse() {
    let source = format!(
        "{SCOPES}\
void first(X x) {{
  if (x is! XI) {{
    return;
  }}
  var a = x.p;
  a.p1iOnly;
}}

void second(X x) {{
  if (x is! XI) {{
    return;
  }}
  var b = x.r;
  b.r1iOnly;
}}
"
    );
    let result = report(&source);
    assert_eq!(
        count_code(&result, "undefined_getter"),
        0,
        "{:?}",
        messages(&result)
    );
}
