//! The #2499 pins: the unnamed extension face's members report exactly
//! once. An unnamed extension's members DO carry declaration sites (the
//! container frame interns the empty spelling), so the `DeclKind::Member`
//! arm and the extension face loop both walked the same member and staged
//! the row twice — the `DeclKind::Member` arm now defers every unnamed-
//! face verdict (instance and static alike) to the face loop, whose gate
//! keys the `extension_member_used` credit pool on the member's own face
//! index.
//!
//! Oracle provenance: each expectation was probed with `dart analyze`
//! (Dart 3.13.1 via fvm 3.47.1) over the /tmp/probe2499 fixtures during
//! this ticket — n1 (the unused unnamed member: the oracle reports once),
//! n2 (the named-private `_E` face reports once), n3 (`v?._gone()` through
//! a nullable parameter receiver spares the member), st1/st2/st5 (the
//! static member, its sibling self-use, and the static field report once),
//! and j (the ambiguous `shared` getter leaves `_gone` reporting once —
//! the #2481 pin's presence-based posture stands beside this count pin).

use super::*;
use crate::collector::collect;
use crate::model::FileElements;
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

/// The `unused_element`/`unused_field` rows of `report`, as their message
/// texts in source order — the family's lens (the count is the pin: the
/// duplicate emission staged the same message twice).
fn unused_rows(source: &str) -> Vec<String> {
    report(source)
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "unused_element" || d.code.name == "unused_field")
        .map(|d| d.message.to_string())
        .collect()
}

/// The unused unnamed-extension member reports exactly once (the #2499
/// duplicate: the site arm and the face loop both staged the row).
#[test]
fn unnamed_extension_member_reports_once() {
    let source = r#"
class C3 {}

extension on C3 {
  void _gone() {}
}
"#;
    assert_eq!(
        unused_rows(source),
        vec!["The declaration '_gone' isn't referenced.".to_string()],
    );
}

/// A use through a nullable parameter receiver spares the member — the
/// credit decides, and the deferral must not drop the face's spare.
#[test]
fn unnamed_extension_member_param_receiver_spares() {
    let source = r#"
class C3 {}

extension on C3 {
  void _gone() {}
}

void f(C3? v) {
  v?._gone();
}
"#;
    assert!(unused_rows(source).is_empty(), "{:?}", unused_rows(source));
}

/// The ambiguous var-receiver shape (probe j) reports the member exactly
/// once — the #2481 ambiguous posture stands, the duplicate does not.
#[test]
fn ambiguous_var_receiver_reports_member_once() {
    let source = r#"
class C3 {}

extension A1 on C3 {
  C3? get shared => null;
}

extension A2 on C3 {
  int get shared => 0;
}

extension on C3 {
  void _gone() {}
}

void f(C3 c) {
  var v = c.shared;
  v?._gone();
}
"#;
    assert_eq!(
        unused_rows(source),
        vec!["The declaration '_gone' isn't referenced.".to_string()],
    );
}

/// The named-private face keeps its single emission (the site arm owns its
/// verdicts; only the unnamed faces defer).
#[test]
fn named_private_extension_member_reports_once() {
    let source = r#"
class C3 {}

extension _E on C3 {
  void _gone() {}
}
"#;
    assert_eq!(
        unused_rows(source),
        vec!["The declaration '_gone' isn't referenced.".to_string()],
    );
}

/// The unnamed face's static member reports exactly once — the deferral
/// covers statics (the loop's static spelling arm reads the same sets).
#[test]
fn unnamed_extension_static_member_reports_once() {
    let source = r#"
class C3 {}

extension on C3 {
  static void _sgone() {}
}
"#;
    assert_eq!(
        unused_rows(source),
        vec!["The declaration '_sgone' isn't referenced.".to_string()],
    );
}

/// A sibling static's self-use spares its target while the unused sibling
/// itself reports once (the self-use credit lands in the same pool the
/// face loop reads).
#[test]
fn unnamed_extension_static_self_use_spares_target() {
    let source = r#"
class C3 {}

extension on C3 {
  static void _sgone() {}

  static void _user() {
    _sgone();
  }
}
"#;
    assert_eq!(
        unused_rows(source),
        vec!["The declaration '_user' isn't referenced.".to_string()],
    );
}

/// The unnamed face's static field reports `unused_field` exactly once.
#[test]
fn unnamed_extension_static_field_reports_once() {
    let source = r#"
class C3 {}

extension on C3 {
  static int _sf = 0;
}
"#;
    assert_eq!(
        unused_rows(source),
        vec!["The value of the field '_sf' isn't used.".to_string()],
    );
}
