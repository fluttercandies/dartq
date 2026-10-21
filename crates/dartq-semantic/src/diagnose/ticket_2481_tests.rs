//! The #2481 pins: the `var`-inference window's extension-face receiver
//! typing (the resolve-side gap behind the UNUSED_ELEMENT census's Shape C
//! residual — the analyzer's ffi_verifier cluster).
//!
//! A `var x = <member read>;` declarator binds the initializer expression's
//! static type, and every extension use reached through `x` resolves only
//! when that type stands. The merged member row of a GETTER read keeps
//! `Unknown` (the merge-time declared-type face answers fields only), so
//! the inference window owes a follow-up answer: the instance accessor face
//! for a getter on a walkable container, and — the shape the census rows
//! die on — the applied extension face's declared return when the
//! initializer's accessor itself resolves through an extension
//! (`var enclosingElement = element.enclosingElement2;` then
//! `enclosingElement.isStruct`). The application runs the resolve-side
//! pipeline, so a genuinely unused member still reports, an ambiguous
//! two-face application credits nothing, and a `?.`-merged read keeps the
//! nullable face.
//!
//! Oracle provenance: probes under /tmp/probe2481 (dart analyze over the
//! minimal rigs) — probe c (`elementAnnotation?.element` into `var` spares
//! `ffiClass`), probe h (an extension getter into `var` spares the
//! follow-on member), probe g (the unused getter beside a typed var still
//! reports), probe i (the `?.`-merged nullable face spares through `?.`),
//! probe j (the ambiguous application leaves the member unused), probe g2
//! (the `this.element` chain keeps the oracle's verdict), and probe d (the
//! promoted `e.supertype` chain spares).

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

/// The instance-getter chain into a `var` (probe c): the initializer's
/// accessor resolves on the walked container, the local carries the
/// getter's declared return, and the extension use through the local
/// spares the member — the oracle reports nothing.
#[test]
fn var_of_instance_getter_chain_spares_extension_member() {
    let source = r#"
class Element {}

class ElementAnnotation {
  Element? get element => null;
}

class Annotation {
  ElementAnnotation? get elementAnnotation => null;
}

class ConstructorElement extends Element {}

extension on Element? {
  ClassElement? get ffiClass => null;
}

void use(Annotation a) {
  var e = a.elementAnnotation?.element;
  e.ffiClass;
}
"#;
    assert!(
        unused_element_rows(source).is_empty(),
        "expected no unused rows: {:?}",
        unused_element_rows(source)
    );
}

/// The extension-getter initializer into a `var` (probe h): the
/// initializer's accessor itself resolves through the applied extension
/// face, the local carries that getter's declared return, and the
/// follow-on extension use through the local spares its member — the
/// oracle reports nothing.
#[test]
fn var_of_extension_getter_spares_extension_member() {
    let source = r#"
class Element {}

class InterfaceElement extends Element {}

extension on InterfaceElement {
  Element? get enclosingElement2 => null;
}

extension on Element? {
  bool get isStruct => false;
}

bool usesGetter(InterfaceElement element) {
  var enclosingElement = element.enclosingElement2;
  return enclosingElement.isStruct;
}
"#;
    assert!(
        unused_element_rows(source).is_empty(),
        "expected no unused rows: {:?}",
        unused_element_rows(source)
    );
}

/// A genuinely unused member still reports beside the typed-var face
/// (probe g): the inference window's extension answer only spares members
/// an access actually resolves through.
#[test]
fn unused_extension_member_still_reports_beside_typed_var() {
    let source = r#"
class Element {}

class ConstructorElement extends Element {}

class ElementAnnotation extends Element {
  Element? get element => null;
}

class Annotation {
  ElementAnnotation? get elementAnnotation => null;
}

extension on Element? {
  bool get ffiClass => false;
}

extension on Annotation {
  bool get isMapping {
    var element = elementAnnotation?.element;
    return element is ConstructorElement && element.ffiClass != null;
  }
}
"#;
    assert_eq!(
        unused_element_rows(source),
        vec!["The declaration 'isMapping' isn't referenced.".to_string()],
    );
}

/// The `?.`-merged initializer keeps the nullable face (probe i): `var v =
/// c.maybe` carries `C2?`, and the null-aware extension use through `v`
/// resolves on the nullable `on` face — the oracle reports nothing.
#[test]
fn null_aware_extension_getter_var_stays_nullable_and_spares() {
    let source = r#"
class C2 {
  C2? get maybe => null;
}

extension on C2? {
  void _ext() {}
}

void f(C2 c) {
  var v = c.maybe;
  v?._ext();
}
"#;
    assert!(
        unused_element_rows(source).is_empty(),
        "expected no unused rows: {:?}",
        unused_element_rows(source)
    );
}

/// The ambiguous two-face application credits nothing (probe j): two
/// accessible faces declare the initializer's getter, no unique resolution
/// stands, and the follow-on member stays unused — the oracle's
/// `ambiguous_extension_member_access` posture.
#[test]
fn ambiguous_extension_getter_keeps_member_uncredited() {
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
    let rows = unused_element_rows(source);
    assert!(
        rows.iter().any(|row| row.contains("'_gone'")),
        "expected the ambiguous posture to keep '_gone' unused: {rows:?}"
    );
}

/// The `this.element` chain into a `var` keeps the oracle's verdict
/// (probe g2): the extension body's own getter use credits `ffiClass`
/// while the never-referenced `isNativeLeaf` still reports.
#[test]
fn this_getter_chain_var_keeps_oracle_verdict() {
    let source = r#"
class Element {}

class ConstructorElement extends Element {}

class ElementAnnotation extends Element {
  Element? get element => null;
}

extension on Element? {
  bool get ffiClass => false;
}

extension on ElementAnnotation {
  bool get isNativeLeaf {
    var element = this.element;
    return element is ConstructorElement && element.ffiClass;
  }
}
"#;
    assert_eq!(
        unused_element_rows(source),
        vec!["The declaration 'isNativeLeaf' isn't referenced.".to_string()],
    );
}

/// The promoted chain keeps sparing (probe d): `e.supertype` resolves on
/// the promoted `ClassElement` receiver and the extension use on its
/// result resolves — the oracle reports nothing.
#[test]
fn promoted_chain_keeps_sparing() {
    let source = r#"
class Element {}

class ClassElement extends Element {
  Element? get supertype => null;
}

extension on Element? {
  bool get isAbiSpecificInteger => false;
}

bool f(Element? e) {
  if (e is ClassElement) {
    return e.supertype.isAbiSpecificInteger;
  }
  return false;
}
"#;
    assert!(
        unused_element_rows(source).is_empty(),
        "expected no unused rows: {:?}",
        unused_element_rows(source)
    );
}
