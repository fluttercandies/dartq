//! The #2465 pins: the unused-element usage scan's extension-member and
//! part-file shapes (the census ⑨ false positives plus the analyzer-source
//! 55-row table's over-report shapes).
//!
//! Two derivation arms feed the element-attributed application credit the
//! consume arms check: a bare or explicit-`this` reference inside an
//! extension body resolves to the extension's own member element (the
//! resolved row names the extension as its owner), and a member access
//! whose receiver the name level cannot walk an `extension<T> on T`-style
//! face's type-parameter `on` type for still resolved through that face in
//! the oracle when the face is the spelling's unique declarer. The
//! part-file direction pins the semantic products the library-wide close
//! consumes: a use in the part lands in the part's use face and spares the
//! host-declared element against the merged set.
//!
//! Oracle provenance: probes under /tmp/probe2465 (dart analyze over the
//! minimal rigs) — the implicit-`this` use spares its member and the
//! never-referenced sibling reports; the external typed access spares the
//! unnamed extension's getter; the universal-on-type face's used member
//! spares while its unused member reports; a private named extension with
//! used members is fully silent (the extension face itself is never
//! visited); and a host-declared private used only in the part stays
//! silent while a genuinely unused one reports.

use super::*;
use crate::collector::collect;
use crate::model::FileElements;
use crate::use_face::{LibraryUsages, LibraryUseSet};
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_resolution::{LibraryClassModifiers, LibraryGraphBuilder};
use dartq_scanner::ScanConfig;
use dartq_source::FileTable;
use std::path::Path;

/// Scans `source` and collects its element model (the rigs' shared
/// preamble), returning the pieces the per-file diagnosis needs.
fn scanned_and_collected(
    source: &str,
    file: FileId,
    strings: &StringTable,
) -> (dartq_scanner::ScanResult, FileElements) {
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
    let elements = collect(source, &scanned.tokens, features, strings);
    (scanned, elements)
}

/// Diagnoses `source` as the sole file of a library with no imports — the
/// single-file rig.
fn report(source: &str) -> FileReport {
    let strings = StringTable::new();
    let mut files = FileTable::new();
    let file = files.intern(Path::new("main.dart"));
    let (scanned, elements) = scanned_and_collected(source, file, &strings);
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
    let set = FeatureSet::for_language_version(LanguageVersion::new(3, 15));
    let (report, _) = diagnose_unit(
        source,
        &scanned.tokens,
        ParserFeatures::from_feature_set(set),
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
    report
}

/// The `unused_element` rows of `report`, as their message texts in source
/// order — the family's lens (the message carries the declared name).
fn unused_element_rows(source: &str) -> Vec<String> {
    report(source)
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "unused_element")
        .map(|d| d.message.to_string())
        .collect()
}

/// The two-unit part rig: `host` and `part` are the two files of ONE
/// library (both files' declarations land in the shared namespace), each
/// diagnosed separately — the shape the engine's library-wide close folds
/// (`LibraryUsages::close_library_usage`). Returns the host report, the
/// part's used-name snapshot (taken before the face joins the fold), the
/// library's merged consumed set, and the rig's string table (the ids
/// resolve against it).
fn part_library(
    host: &str,
    part: &str,
) -> (
    FileReport,
    Vec<StringId>,
    LibraryUseSet,
    dartq_source::StringTable,
) {
    let strings = StringTable::new();
    let mut files = FileTable::new();
    let host_file = files.intern(Path::new("main.dart"));
    let part_file = files.intern(Path::new("p.dart"));
    let (scanned_host, host_elements) = scanned_and_collected(host, host_file, &strings);
    let (scanned_part, part_elements) = scanned_and_collected(part, part_file, &strings);
    let mut builder = LibraryGraphBuilder::new(&strings);
    let lib = builder.intern("package:t/main.dart");
    for (name, decl) in host_elements.declarations() {
        if decl.kind == MicroKind::TypeParam {
            continue;
        }
        builder.declare(lib, name, host_file);
    }
    for (name, decl) in part_elements.declarations() {
        if decl.kind == MicroKind::TypeParam {
            continue;
        }
        builder.declare(lib, name, part_file);
    }
    let graph = builder.finish();
    let namespaces = Namespaces::build(&graph);
    let mut index = ElementIndex::default();
    index.insert(host_file, host_elements);
    index.insert(part_file, part_elements);
    let set = FeatureSet::for_language_version(LanguageVersion::new(3, 15));
    let features = ParserFeatures::from_feature_set(set);
    let promotion = set.is_enabled(dartq_language::Feature::InferenceUpdate2);
    let (host_report, host_face) = diagnose_unit(
        host,
        &scanned_host.tokens,
        features,
        &strings,
        host_file,
        lib,
        &graph,
        &namespaces,
        &index,
        StrictModes::default(),
        &LibraryClassModifiers::default(),
        promotion,
    );
    let (_, part_face) = diagnose_unit(
        part,
        &scanned_part.tokens,
        features,
        &strings,
        part_file,
        lib,
        &graph,
        &namespaces,
        &index,
        StrictModes::default(),
        &LibraryClassModifiers::default(),
        promotion,
    );
    let part_used: Vec<StringId> = part_face.used.iter().copied().collect();
    let mut usages = LibraryUsages::default();
    usages.add_unit(lib, host_face);
    usages.add_unit(lib, part_face);
    let merged = usages.close_library_usage(lib);
    (host_report, part_used, merged, strings)
}

/// Acceptance (1), the part-file direction: a use in the part spares the
/// host-declared private element through the library-wide close. The host
/// pass stages the row against its own unit's uses (the engine close then
/// re-decides it — the dartq-engine pin carries that side), so the
/// semantic contract is exact: the staged rows exist, the part's use face
/// carries the used name, and the merged consumed set spares `_Host` while
/// keeping the genuinely unused `_Orphan` (`probe2465/e`: the oracle keeps
/// the part-used host private silent and reports the orphan).
#[test]
fn part_file_use_spares_the_host_element() {
    let (host_report, part_used, merged, strings) = part_library(
        "part 'p.dart';\n\nclass _Host {\n  const _Host();\n}\n\nclass _Orphan {}\n",
        "part of 'main.dart';\n\nvoid use() {\n  _Host();\n}\n",
    );
    let rows = |report: &FileReport| {
        report
            .diagnostics
            .iter()
            .filter(|d| d.code.name == "unused_element")
            .map(|d| d.message.to_string())
            .collect::<Vec<_>>()
    };
    let host_rows = rows(&host_report);
    assert!(
        host_rows.iter().any(|m| m.contains("_Host")),
        "the host pass must stage _Host against its own unit: {host_rows:?}"
    );
    assert!(
        host_rows.iter().any(|m| m.contains("_Orphan")),
        "the orphan must stage: {host_rows:?}"
    );
    let host_id = strings.intern("_Host");
    let orphan_id = strings.intern("_Orphan");
    assert!(
        part_used.contains(&host_id),
        "the part's use face must carry the host element: {part_used:?}"
    );
    assert!(
        merged.used(host_id),
        "the library close must spare the part-used host element"
    );
    assert!(
        !merged.used(orphan_id),
        "the genuinely unused orphan must stay unspared"
    );
}

/// Acceptance (2), the implicit-`this` shape (`probe2465/a`): a private
/// extension method referenced bare inside the extension's own body is
/// used — the resolution names the extension as the member's owner —
/// while the never-referenced sibling still reports. The external access
/// `c.call()` spares `call` through the resolved application credit.
#[test]
fn extension_member_implicit_this_use_spares() {
    let source = "\
class C {}

extension Ext on C {
  void _usedBare() {}
  void _unused() {}

  void call() {
    _usedBare();
  }
}

void f(C c) {
  c.call();
}
";
    let rows = unused_element_rows(source);
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert!(rows[0].contains("_unused"), "{rows:?}");
}

/// Acceptance (3), the external-access shape (`probe2465/b`): an unnamed
/// extension's getter read through a typed receiver is spared by the
/// resolved implicit application — the element-attributed credit the
/// consume arms require.
#[test]
fn extension_member_external_access_spares() {
    let source = "\
class Annotation {}

extension on Annotation {
  bool get isArray => true;
}

void f(Annotation a) {
  print(a.isArray);
}
";
    assert!(unused_element_rows(source).is_empty(), "{source:?}");
}

/// Acceptance (4), the false-negative guard (`probe2465/c`): a genuinely
/// unreferenced extension member still reports, and an unreferenced static
/// extension field reports `unused_field` — the derivations spare only
/// what an actual use resolved.
#[test]
fn genuinely_unused_extension_members_still_report() {
    let source = "\
class C {}

extension Ext on C {
  void _gone() {}
  static int _goneStatic = 0;
}
";
    let report = report(source);
    let element: Vec<String> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "unused_element")
        .map(|d| d.message.to_string())
        .collect();
    assert_eq!(element.len(), 1, "{element:?}");
    assert!(element[0].contains("'_gone'"), "{element:?}");
    let field: Vec<String> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "unused_field")
        .map(|d| d.message.to_string())
        .collect();
    assert_eq!(field.len(), 1, "{field:?}");
    assert!(field[0].contains("_goneStatic"), "{field:?}");
}

/// The universal-`on`-type shape (`probe2465/d`, the census's
/// `extension<T> on T` rows — `visitWithOverride`, `mapOrNull`): the used
/// member of a face whose `on` type's head is the face's own type
/// parameter is spared (the oracle instantiates the parameter to the
/// receiver), and the face's unreferenced member still reports. The
/// receiver is a user-declared type — the validity gate reads a resolvable
/// static type, exactly the census receivers'.
#[test]
fn universal_on_type_param_used_member_spares() {
    let source = "\
class N {}

extension<T> on T {
  R? mapOrNull<R>(R Function(T) mapper) => mapper(this);
  void visit() {}
}

void f(N x) {
  x.mapOrNull((v) => v);
}
";
    let rows = unused_element_rows(source);
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert!(rows[0].contains("visit"), "{rows:?}");
}

/// The named-private extension shape (`probe2465/f`): members used — one
/// through implicit `this`, one through an external access — keep the
/// whole face silent; the oracle never visits the extension element
/// itself and spares every used member.
#[test]
fn named_private_extension_implicit_this_use_spares() {
    let source = "\
class C {}

extension _E on C {
  int get _count => 0;
  int get doubled => _count * 2;
}

void f(C c) {
  print(c.doubled);
}
";
    assert!(unused_element_rows(source).is_empty(), "{source:?}");
}

/// The unnamed-extension self-use shape (the ffi_verifier.dart census
/// cluster): a getter referenced bare inside a sibling getter of the same
/// unnamed extension is used — the nameless container's face key carries
/// the credit the consume loop checks.
#[test]
fn unnamed_extension_implicit_this_use_spares() {
    let source = "\
class Annotation {}

extension on Annotation {
  bool get _inner => true;
  bool get outer => _inner;
}

void f(Annotation a) {
  print(a.outer);
}
";
    assert!(unused_element_rows(source).is_empty(), "{source:?}");
}

/// The class-member self-use guard (the bundle_writer.dart census row's
/// shape): a private method referenced only through a bare in-class call
/// stays spared through the name-level element pool — the derivation must
/// not disturb the non-extension path.
#[test]
fn class_member_bare_self_use_still_spares() {
    let source = "\
class K {
  void _handler() {}

  void run() {
    _handler();
  }
}
";
    assert!(unused_element_rows(source).is_empty(), "{source:?}");
}

/// The element-identity guard: a same-spelled member on the `on` type's
/// class never spares a sibling extension's member — inside `extension B
/// on Y`, the bare `_m` resolves to `Y._m`, so `A._m` keeps its report
/// (the oracle's `usedElements.members` joins on the element, never the
/// spelling).
#[test]
fn sibling_container_same_spelling_still_reports() {
    let source = "\
class X {}

class Y {
  void _m() {}
}

extension A on X {
  void _m() {}
}

extension B on Y {
  void f() {
    _m();
  }
}
";
    let rows = unused_element_rows(source);
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert!(rows[0].contains("'_m'"), "{rows:?}");
}
