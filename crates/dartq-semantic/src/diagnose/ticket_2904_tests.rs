//! The #2904 `invalid_override` pins: the combined-signature faces the
//! override verifier gained.
//!
//! 1. Instance-member inference (`instance_member_inferrer.dart`): an
//!    implicit type slot on a declared field/method infers from the
//!    combined superinterface signature — `var foo;` over incompatible
//!    getters compares as `dynamic Function()`, over one getter as that
//!    getter's type, and a method's implicit slots fill from the
//!    combined signature.
//! 2. The per-face merged witness (`inheritance_manager3.dart`
//!    `_getValidOverrides`/`_topMerge`): a name carried by several
//!    super-interface faces resolves to the most-specific candidate (the
//!    `num` setter over the `int` one), never the first-declared face;
//!    an incomparable candidate set conflicts and reports nothing.
//! 3. The inherited field's induced setter: `A { num f }` answers `f=`
//!    exactly like a written setter, so a narrowing redeclaration
//!    reports the setter face.
//! 4. The frozen top faces: an `Object`-less class, an enum, and an
//!    `on`-less / `on Enum` mixin compare against `Object`'s and
//!    `Enum`'s frozen signatures (`noSuchMethod`, `hashCode`, `index`).
//!
//! Oracle provenance: the #2904 census rows —
//! `instance_member_inference_class` `_multiple_*_incompatible`,
//! `override/inheritance_field_test_33/42`, `InvalidOverrideTest`
//! `test_method_parameter_requiredAndPositional`, `mixin_legacy`
//! `invalid_override_in_mixin_test`, and co19 `Enhanced-Enum`
//! `implementing_enum_A04_t21` (`dart analyze`, Dart 3.13).

use crate::collector::collect;
use crate::diagnose::{FileReport, StrictModes, fold_unit, merge_source_ordered};
use crate::library::ElementIndex;
use crate::model::MicroKind;
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_parser::ParserFeatures;
use dartq_resolution::{LibraryClassModifiers, LibraryGraphBuilder, Namespaces};
use dartq_scanner::ScanConfig;
use dartq_source::{FileTable, StringTable};
use std::path::Path;

/// Diagnoses `source` as the sole file of a library with no imports —
/// the single-file rig, local to this module.
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

fn invalid_override_rows(report: &FileReport) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "invalid_override")
        .map(|d| d.message.to_string())
        .collect()
}

// —— shape 1: instance-member inference over merged super faces ——

/// `var foo;` over two incompatible field getters: the inferred field type
/// is `dynamic` (the combined getter signature conflicts), so BOTH faces
/// break with the `dynamic Function()` declared text.
#[test]
fn inferred_field_over_incompatible_getters_reports_both() {
    let report = report(
        "class A {\n\
         \x20 int foo = 0;\n\
         }\n\
         class B {\n\
         \x20 String foo = '';\n\
         }\n\
         class C implements A, B {\n\
         \x20 var foo;\n\
         }\n",
    );
    let mut rows = invalid_override_rows(&report);
    rows.sort();
    assert_eq!(
        rows,
        vec![
            "'C.foo' ('dynamic Function()') isn't a valid override of \
             'A.foo' ('int Function()')."
                .to_string(),
            "'C.foo' ('dynamic Function()') isn't a valid override of \
             'B.foo' ('String Function()')."
                .to_string(),
        ],
        "both faces break on the inferred dynamic: {rows:?}"
    );
}

/// A getter over an incompatible setter pair: the setter side conflicts to
/// `dynamic`, the field type is `dynamic` (getter `int` != setter
/// `dynamic`), so only the getter face breaks — the `dynamic` setter face
/// accepts both written setters.
#[test]
fn inferred_field_over_incompatible_setters_reports_one() {
    let report = report(
        "class A {\n\
         \x20 int get foo => 0;\n\
         }\n\
         class B {\n\
         \x20 set foo(String _) {}\n\
         }\n\
         class C {\n\
         \x20 set foo(int _) {}\n\
         }\n\
         class X implements A, B, C {\n\
         \x20 var foo;\n\
         }\n",
    );
    let rows = invalid_override_rows(&report);
    assert_eq!(
        rows,
        vec![
            "'X.foo' ('dynamic Function()') isn't a valid override of \
             'A.foo' ('int Function()')."
                .to_string(),
        ],
        "only the getter face breaks: {rows:?}"
    );
}

/// One compatible getter: the field infers the getter's own type and the
/// override is valid — the inference never downgrades a resolvable merge.
#[test]
fn inferred_field_over_single_getter_stays_silent() {
    let report = report(
        "class A {\n\
         \x20 int get foo => 0;\n\
         }\n\
         class C implements A {\n\
         \x20 var foo;\n\
         }\n",
    );
    let rows = invalid_override_rows(&report);
    assert!(
        rows.is_empty(),
        "the inferred int field overrides cleanly: {rows:?}"
    );
}

/// A method's implicit slots fill from the combined signature: the return
/// reads `void` and the parameter reads `int`, so the newly-required
/// positional breaks exactly like a written `void foo(int a)`.
#[test]
fn inferred_method_parameter_breaks_on_added_requiredness() {
    let report = report(
        "class B {\n\
         \x20 void foo([int a]) {}\n\
         }\n\
         class C extends B {\n\
         \x20 foo(int a) {}\n\
         }\n",
    );
    let rows = invalid_override_rows(&report);
    assert_eq!(
        rows,
        vec![
            "'C.foo' ('void Function(int)') isn't a valid override of \
             'B.foo' ('void Function([int])')."
                .to_string(),
        ],
        "the inferred signature keeps the written requiredness: {rows:?}"
    );
}

// —— shape 2: the per-face merged witness ——

/// `implements I, J` with `I: int setter`, `J: num setter` and a `double`
/// redeclaration: the most-specific inherited signature is J's (`num`
/// setter is a subtype of the `int` one — the parameter arm), so the one
/// diagnostic anchors at `J`, never at the first-declared `I`.
#[test]
fn merged_setter_witness_anchors_at_the_most_specific_face() {
    let report = report(
        "abstract class I {\n\
         \x20 set setter13(int _);\n\
         }\n\
         abstract class J {\n\
         \x20 set setter13(num _);\n\
         }\n\
         abstract class Class implements I, J {}\n\
         class SubClass extends Class {\n\
         \x20 set setter13(double _) {}\n\
         }\n",
    );
    let rows = invalid_override_rows(&report);
    assert_eq!(
        rows,
        vec![
            "The setter 'SubClass.setter13' ('void Function(double)') isn't \
             a valid override of 'J.setter13' ('void Function(num)')."
                .to_string(),
        ],
        "the num setter subsumes the int one: {rows:?}"
    );
}

/// The declared-first flip of the same shape (`I: num`, `J: int`): the
/// witness IS the first face — one diagnostic, anchored at `I`.
#[test]
fn merged_setter_witness_keeps_the_first_declared_witness() {
    let report = report(
        "abstract class I {\n\
         \x20 set setter13(num _);\n\
         }\n\
         abstract class J {\n\
         \x20 set setter13(int _);\n\
         }\n\
         abstract class Class implements I, J {}\n\
         class SubClass extends Class {\n\
         \x20 set setter13(double _) {}\n\
         }\n",
    );
    let rows = invalid_override_rows(&report);
    assert_eq!(
        rows,
        vec![
            "The setter 'SubClass.setter13' ('void Function(double)') isn't \
             a valid override of 'I.setter13' ('void Function(num)')."
                .to_string(),
        ],
        "the first-declared num setter is the witness: {rows:?}"
    );
}

// —— shape 3: the inherited field's induced setter ——

/// A narrowing field redeclaration over a superclass field: the inherited
/// field answers the setter face through its induced setter (`num`
/// parameter), so the setter comparison breaks while the covariant getter
/// face stays valid — one diagnostic, the setter variant.
#[test]
fn inherited_field_induced_setter_reports_narrowing() {
    let report = report(
        "class A {\n\
         \x20 num field2 = 0;\n\
         }\n\
         class Sub extends A {\n\
         \x20 int field2 = 0;\n\
         }\n",
    );
    let rows = invalid_override_rows(&report);
    assert_eq!(
        rows,
        vec![
            "The setter 'Sub.field2' ('void Function(int)') isn't a valid \
             override of 'A.field2' ('void Function(num)')."
                .to_string(),
        ],
        "the induced setter face carries the comparison: {rows:?}"
    );
}

// —— shape 4: the frozen top faces ——

/// An unannotated `noSuchMethod()` with no supertype written: the implicit
/// `Object` face's frozen signature is `dynamic Function(Invocation)`, so
/// the zero-parameter override breaks on arity with the inferred `dynamic`
/// return in the declared slot.
#[test]
fn frozen_object_no_such_method_breaks_on_arity() {
    let report = report("class A {\n\x20 noSuchMethod() {}\n}\n");
    let rows = invalid_override_rows(&report);
    assert_eq!(
        rows,
        vec![
            "'A.noSuchMethod' ('dynamic Function()') isn't a valid override \
             of 'Object.noSuchMethod' ('dynamic Function(Invocation)')."
                .to_string(),
        ],
        "the frozen Object signature owns the comparison: {rows:?}"
    );
}

/// A conforming `hashCode` redeclaration stays silent — the frozen face
/// compares, the types agree.
#[test]
fn frozen_object_hash_code_valid_stays_silent() {
    let report = report("class C {\n\x20 int get hashCode => 7;\n}\n");
    let rows = invalid_override_rows(&report);
    assert!(
        rows.is_empty(),
        "the int hashCode overrides Object's cleanly: {rows:?}"
    );
}

/// An `on Enum` mixin's `final String hashCode`: the `Enum` constraint's
/// frozen face falls through to `Object`'s (`Enum` redeclares only
/// `index`), so the String field's getter face breaks against the frozen
/// `int Function()`.
#[test]
fn frozen_enum_face_falls_through_to_object() {
    let report = report(
        "mixin M2 on Enum {\n\
         \x20 final String hashCode = '';\n\
         }\n",
    );
    let rows = invalid_override_rows(&report);
    assert_eq!(
        rows,
        vec![
            "'M2.hashCode' ('String Function()') isn't a valid override of \
             'Object.hashCode' ('int Function()')."
                .to_string(),
        ],
        "the on-Enum constraint reaches Object's frozen member: {rows:?}"
    );
}

/// An enum redeclaring `index` with an unfit type: the enum's implicit
/// `Enum` supertype carries the frozen `int Function()` getter, so the
/// String redeclaration breaks with the witness named `Enum.index`.
#[test]
fn frozen_enum_index_breaks_on_type() {
    let report = report("enum E {\n\x20 a;\n\x20 String get index => 'x';\n}\n");
    let rows = invalid_override_rows(&report);
    assert_eq!(
        rows,
        vec![
            "'E.index' ('String Function()') isn't a valid override of \
             'Enum.index' ('int Function()')."
                .to_string(),
        ],
        "the enum's implicit Enum face owns index: {rows:?}"
    );
}

/// An `on`-less mixin constrains `Object`: a written-`String` getter
/// breaks against the frozen `int` hashCode exactly like the oracle's
/// `M.hashCode` rows.
#[test]
fn on_less_mixin_constrains_object() {
    let report = report(
        "mixin M {\n\
         \x20 String get hashCode => 'x';\n\
         }\n",
    );
    let rows = invalid_override_rows(&report);
    assert_eq!(
        rows,
        vec![
            "'M.hashCode' ('String Function()') isn't a valid override of \
             'Object.hashCode' ('int Function()')."
                .to_string(),
        ],
        "the implicit Object constraint joins the faces: {rows:?}"
    );
}
