//! The #2472 pins: instance-creation typing from the constructor's return
//! (census families G4+G9, xml 7 + webview 1 rows).
//!
//! G4: an explicit-keyword creation with written type arguments
//! (`const Equality<Apple>()`) folds to a receiver typed by the
//! constructor's return instantiated with the written arguments (upstream
//! Dart SDK source, pkg/analyzer/lib/src/generated/resolver.dart,
//! visitInstanceCreationExpression) — its members resolve there, never on
//! the enclosing context's face (the sibling argument's type, the `&&`
//! composite's face). The oracle is silent on every census site (probed
//! 3.13.1 over the xml corpus, /tmp/probe2466 rig x1_eqbool), and a
//! genuine miss on the created type still reports, naming the
//! constructor's type.
//!
//! G9: an initializing formal's captured type is the field's declared
//! type (`FieldFormalParameterElement.type`), never the default value
//! expression's own type arguments — `this.headers = const <K, V>{}`
//! answers the field's generic type at the named-argument slot, and the
//! neighboring plain slot keeps enforcing its own formal's type (oracle
//! silence probed over the webview corpus shape, /tmp/probe2466 rig
//! x6_singles).
//!
//! The rigs declare no imports, so every pin spells custom types only
//! (`ticket_2461_tests` carries the same rule — the harness's bare world
//! seeds no core spellings, keywords excepted), and wrong-typed arguments
//! use named custom types — a numeric literal against a custom generic is
//! the documented undecided face, not a decisive mismatch.

use super::*;
use crate::collector::collect;
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_resolution::LibraryGraphBuilder;
use dartq_scanner::ScanConfig;
use dartq_source::FileTable;
use std::path::Path;

/// Diagnoses `source` as the sole file of a library with no imports —
/// the single-file rig (`ticket_2461_tests` carries the same shape).
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

const EQUALITY: &str = "\
class Equality<T> {
  const Equality();
  Flag same(T? a, T? b) => const Flag();
  Num code(T? a) => const Num();
}

class Flag {
  const Flag();
}

class Num {
  const Num();
}

enum NodeType { one }

class Apple {
  const Apple();
}
";

/// The census's declaration.dart hashCode shape: the creation's members
/// resolve on `Equality<Apple>`, so `code` exists and the oracle is
/// silent. The pre-fix model folded no creation row (the head is separated
/// from the paren by the type-argument group), and the member merge popped
/// the sibling argument's row — the census reported `hash` against the
/// sibling argument's type.
#[test]
fn const_creation_hash_in_hashcode_resolves_on_ctor_type() {
    let source = format!(
        "{EQUALITY}\
void mix(NodeType nodeType, Num code) {{}}

class Ev {{
  final Apple attributes;
  Ev(this.attributes);
  NodeType get nodeType => NodeType.one;

  @override
  Num get hashCode {{
    mix(nodeType, const Equality<Apple>().code(attributes));
    return const Num();
  }}
}}
"
    );
    let report = report(&source);
    assert!(report.diagnostics.is_empty(), "{:?}", messages(&report));
}

/// The census's declaration.dart operator== shape: the creation sits in
/// the `&&` composite's bool-latched context, and `same` must resolve on
/// `Equality<Apple>` — the composite's face must not lend itself to the
/// creation's receiver (the oracle is silent; the census row named `bool`).
#[test]
fn const_creation_same_in_operator_resolves_on_ctor_type() {
    let source = format!(
        "{EQUALITY}\
class Ev {{
  final Apple attributes;
  Ev(this.attributes);

  void eq(other) {{
    other is Ev &&
        const Equality<Apple>().same(other.attributes, attributes);
  }}
}}
"
    );
    let report = report(&source);
    assert!(report.diagnostics.is_empty(), "{:?}", messages(&report));
}

/// The start_element.dart/binary.dart shapes: the same creation spellings
/// through a `new` keyword, a non-generic const head, and a top-level
/// initializer — every arm of the creation fold keeps its silence.
#[test]
fn const_creation_controls_stay_silent() {
    let source = format!(
        "{EQUALITY}\
class Bin {{
  final Apple value;
  Bin(this.value);

  void eq(other) {{
    if (other is Bin) {{
      const Equality<Apple>().same(value, other.value);
    }}
  }}

  Num get hashCode => new Equality<Apple>().code(value);
}}

class Plain {{
  const Plain();
  Flag ok() => const Flag();
}}

Flag plainMember() => const Plain().ok();
final topLevel = const Equality<Apple>().same(const Apple(), const Apple());

// The non-const implicit creation with written type arguments rides the
// folded-container send, not this fold — the control keeps it green.
Flag nonConst(Apple a, Apple b) => Equality<Apple>().same(a, b);
"
    );
    let report = report(&source);
    assert!(report.diagnostics.is_empty(), "{:?}", messages(&report));
}

/// The keep-honest face: a genuinely absent member on the created type
/// reports `undefined_method` naming the constructor's type — the oracle's
/// row for `const Equality<Apple>().nope()` reads `... for the type
/// 'Equality'` (probed 3.13.1: the raw display convention matches), so the
/// miss must fire with that type text and never the enclosing context's
/// face.
#[test]
fn genuine_miss_on_const_creation_names_ctor_type() {
    let source = format!(
        "{EQUALITY}\
void probe() {{
  const Equality<Apple>().nope();
}}
"
    );
    let report = report(&source);
    assert_eq!(
        count_code(&report, "undefined_method"),
        1,
        "{:?}",
        messages(&report)
    );
    assert!(
        report
            .diagnostics
            .iter()
            .any(|d| d.message.contains("'Equality'")),
        "the miss names the constructor's type: {:?}",
        messages(&report)
    );
}

const PARAMS: &str = "\
class Key {}
class Pair<K, V> {}
class Apple {
  const Apple();
}
class Pear {}

class LoadParams {
  final Key absoluteFilePath;
  LoadParams({required this.absoluteFilePath});
}

class ALP extends LoadParams {
  final Pair<Apple, Pear> headers;
  ALP({required Key absoluteFilePath, this.headers = const <Apple, Pear>{}})
    : super(absoluteFilePath: absoluteFilePath);

  factory ALP.fromParams(
    LoadParams params, {
    Pair<Apple, Pear> headers = const <Apple, Pear>{},
  }) {
    return ALP(absoluteFilePath: params.absoluteFilePath, headers: headers);
  }
}
";

/// The census's android_webview_controller.dart shape: the factory forwards
/// `headers:` — a generic-typed factory formal — to the creation's
/// initializing formal whose default spells typed arguments. The slot's
/// type is the field's declared `Pair<Apple, Pear>`, so the oracle is
/// silent. The pre-fix capture took the default's last type argument
/// (`Pear`) as the formal's type and reported ATNA against it.
#[test]
fn initializing_formal_named_slot_takes_field_type() {
    let report = report(PARAMS);
    assert!(report.diagnostics.is_empty(), "{:?}", messages(&report));
}

/// The slot's field type enforces for real: a wrong-typed `headers:`
/// argument reports ATNA naming the field's declared type — never the
/// default's own type argument. Silence alone cannot distinguish the
/// patched slot from an unchecked one.
#[test]
fn initializing_formal_slot_enforces_field_type() {
    let source = format!(
        "{PARAMS}\
void probe(Key path) {{
  ALP(absoluteFilePath: path, headers: const Apple());
}}
"
    );
    let report = report(&source);
    assert_eq!(
        count_code(&report, "argument_type_not_assignable"),
        1,
        "{:?}",
        messages(&report)
    );
    assert!(
        report
            .diagnostics
            .iter()
            .any(|d| d.message.contains("'Pair<Apple, Pear>'")),
        "the slot reads the field's declared type: {:?}",
        messages(&report)
    );
}

/// The plain slot keeps its own face: a wrong-typed `absoluteFilePath:`
/// argument still reports against the declaring constructor's own formal
/// type — the named-parameter list keys each slot to its own declaring
/// constructor's formal, and the fix must not loosen the neighbors.
#[test]
fn plain_named_slot_keeps_enforcing_its_type() {
    let source = format!(
        "{PARAMS}\
void probe(LoadParams params) {{
  ALP(absoluteFilePath: params, headers: const <Apple, Pear>{{}});
}}
"
    );
    let report = report(&source);
    assert_eq!(
        count_code(&report, "argument_type_not_assignable"),
        1,
        "{:?}",
        messages(&report)
    );
    assert!(
        report
            .diagnostics
            .iter()
            .any(|d| d.message.contains("'Key'")),
        "the plain slot keeps its face: {:?}",
        messages(&report)
    );
}

/// An annotated formal's declared type survives its default window: the
/// default's typed collection leaves nothing in the armed slot, so the
/// written annotation answers the argument check — a wrong-typed `slot:`
/// argument reports against the annotation, not the default's type
/// argument.
#[test]
fn annotated_formal_with_typed_default_keeps_annotation() {
    let source = "\
class Pair<K, V> {}
class Apple {
  const Apple();
}
class Pear {}

class Box {
  Pair<Apple, Pear> slot;
  Box({this.slot = const <Apple, Pear>{}});
}

void probe() {
  Box(slot: const Apple());
}
";
    let report = report(source);
    assert_eq!(
        count_code(&report, "argument_type_not_assignable"),
        1,
        "{:?}",
        messages(&report)
    );
    assert!(
        report
            .diagnostics
            .iter()
            .any(|d| d.message.contains("'Pair<Apple, Pear>'")),
        "the annotation answers, not the default's arguments: {:?}",
        messages(&report)
    );
}
