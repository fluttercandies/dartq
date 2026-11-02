//! The #2879 pins: the anonymous-methods experiment's receiver faces over
//! the collector's captured invocations — `anonymous_method_wrong_parameter_list`
//! (the written list must be exactly one required positional parameter,
//! anchored at the parens) and `anonymous_method_wrong_parameter_type` (the
//! receiver's static type must accept the written parameter type, anchored
//! at the annotation). Decision order and anchors follow the pinned 3.13.7
//! oracle (`ast_builder.dart:918-929` shape check — the list drops on
//! failure, so the type row never co-fires — then
//! `resolver.dart:1977-1982,2063-2079`). Corpus rows: the two `error_test`s
//! under `features/anonymous-methods/.../{expression,block}/`; the
//! super-error files carry only unspecified analyzer rows (not this face)
//! and stay silent here.

use super::test_rig::CORE_PRELUDE;
use super::{FileReport, StrictModes, fold_unit_with_ops, merge_source_ordered};
use crate::library::ElementIndex;
use crate::model::MicroKind;
use dartq_language::{Feature, FeatureSet, LanguageVersion};
use dartq_parser::ParserFeatures;
use dartq_resolution::ClassModifiers;
use dartq_resolution::{
    Combinators, ImportEdge, LibraryClassModifiers, LibraryGraphBuilder, Namespaces,
};
use dartq_scanner::ScanConfig;
use dartq_source::{FileTable, StringTable};
use std::path::Path;

/// Diagnoses `source` with the anonymous-methods experiment opted in (the
/// feature has no `enabledIn` — never shipped — so the fixed-version rig
/// pins it explicitly, exactly like the corpus options' `enable-experiment`
/// list). The prelude rides ahead; offsets in the report are absolute over
/// the prelude-joined fixture.
fn report(source: &str) -> FileReport {
    let full = format!("{CORE_PRELUDE}{source}");
    let strings = StringTable::new();
    let mut files = FileTable::new();
    let file = files.intern(Path::new("main.dart"));
    let scanned = dartq_scanner::scan(
        &full,
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
    let set = FeatureSet::for_language_version(LanguageVersion::new(3, 15))
        .with_experiment(Feature::AnonymousMethods);
    let features = ParserFeatures::from_feature_set(set);
    let elements = crate::collector::collect(&full, &scanned.tokens, features, &strings);
    let mut builder = LibraryGraphBuilder::new(&strings);
    let lib = builder.intern("package:t/main.dart");
    let core_lib = builder.intern("dart:core");
    let core_file = files.intern(Path::new("<dart:core>"));
    for name in [
        "Object", "Never", "Null", "Function", "Future", "FutureOr", "Record", "Enum", "Type",
        "Symbol",
    ] {
        builder.declare(core_lib, strings.intern(name), core_file);
    }
    builder.import(
        lib,
        ImportEdge {
            target: Some(core_lib),
            prefix: None,
            deferred: false,
            non_library: false,
            combinators: Combinators::unfiltered(),
        },
    );
    let mut index = ElementIndex::default();
    index.insert(file, elements.clone());
    builder.unit(lib, file);
    for (name, decl) in elements.declarations() {
        if decl.kind == MicroKind::TypeParam {
            continue;
        }
        if matches!(
            decl.kind,
            MicroKind::Class | MicroKind::Enum | MicroKind::Mixin
        ) {
            builder.declare_class(lib, name, file, ClassModifiers::default());
        } else {
            builder.declare(lib, name, file);
        }
    }
    let graph = builder.finish();
    let namespaces = Namespaces::build(&graph);
    let fold = fold_unit_with_ops(
        &full,
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
        set.is_enabled(Feature::InferenceUpdate2),
        None,
    );
    let mut report = fold.report;
    report.diagnostics = merge_source_ordered(fold.parse_diagnostics, report.diagnostics);
    report
}

/// The fixture's absolute offset of `needle`, searched in the SOURCE
/// region only (past the prelude — short needles like `int` otherwise hit
/// the prelude's own declarations).
fn at(full: &str, needle: &str) -> u32 {
    CORE_PRELUDE.len() as u32 + full[CORE_PRELUDE.len()..].find(needle).expect(needle) as u32
}

/// The source text at an emitted `(offset, length)` — the failure face of
/// an offset pin (the prelude included in `full`, offsets absolute).
fn anchored(full: &str, offset: u32, length: u32) -> &str {
    let start = offset as usize;
    &full[start..start + length as usize]
}

/// The rows of one code name, as `(offset, length, message)`.
fn rows_of<'r>(report: &'r FileReport, name: &str) -> Vec<(u32, u32, &'r str)> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == name)
        .map(|d| (d.range.offset, d.range.length, d.message.as_str()))
        .collect()
}

const WRONG_LIST_MESSAGE: &str = "An anonymous method with a parameter list must have exactly one required, positional parameter.";
const WRONG_TYPE_MESSAGE: &str = "The receiver type 'String' must be assignable to the formal parameter type 'int' in an anonymous method.";

/// The expression corpus face (`error_test`, arrow bodies): five shape rows
/// anchored at the full parameter list, then the type row anchored at the
/// written `int`.
#[test]
fn expression_error_test_rows() {
    let body = r#"
    1.() => 1;
    1.(a, b) => 1;
    1.([a]) => 1;
    1.({a}) => 1;
    1.({required a}) => 1;
    "".(int i) => 1;
"#;
    let source = format!("void main() {{\n{body}}}\n");
    let full = format!("{CORE_PRELUDE}{source}");
    let report = report(&source);
    let list = rows_of(&report, "anonymous_method_wrong_parameter_list");
    let expected: Vec<(u32, u32)> = [
        "() => 1;",
        "(a, b) => 1;",
        "([a]) => 1;",
        "({a}) => 1;",
        "({required a}) => 1;",
    ]
    .iter()
    .map(|shape| {
        let list = shape.split(" =>").next().expect("list");
        // The bare `()` list collides with `main()`'s own parens; anchor
        // the receiver-prefixed form and skip the receiver.
        let needle = format!("1.{list}");
        (at(&full, &needle) + 2, list.len() as u32)
    })
    .collect();
    assert_eq!(
        list,
        expected
            .iter()
            .map(|&(offset, length)| (offset, length, WRONG_LIST_MESSAGE))
            .collect::<Vec<_>>(),
        "{:#?}",
        list.iter()
            .map(|&(o, l, _)| (o, l, anchored(&full, o, l)))
            .collect::<Vec<_>>()
    );
    let int = "int";
    assert_eq!(
        rows_of(&report, "anonymous_method_wrong_parameter_type"),
        vec![(at(&full, int), int.len() as u32, WRONG_TYPE_MESSAGE)],
        "{:#?}",
        report
            .diagnostics
            .iter()
            .map(|d| format!(
                "{} @{} {:?}",
                d.code.name,
                d.range.offset,
                anchored(&full, d.range.offset, d.range.length)
            ))
            .collect::<Vec<_>>()
    );
}

/// The block corpus face (`error_test`, block bodies): the same six rows at
/// the same anchors with `{};` bodies.
#[test]
fn block_error_test_rows() {
    let body = r#"
    1.() {};
    1.(a, b) {};
    1.([a]) {};
    1.({a}) {};
    1.({required a}) {};
    "".(int i) {};
"#;
    let source = format!("void main() {{\n{body}}}\n");
    let full = format!("{CORE_PRELUDE}{source}");
    let report = report(&source);
    let list = rows_of(&report, "anonymous_method_wrong_parameter_list");
    let expected: Vec<(u32, u32)> = [
        "() {};",
        "(a, b) {};",
        "([a]) {};",
        "({a}) {};",
        "({required a}) {};",
    ]
    .iter()
    .map(|shape| {
        let list = shape.split(" {}").next().expect("list");
        // Same receiver-prefix anchoring as the arrow face: bare `()`
        // collides with `main()`.
        let needle = format!("1.{list}");
        (at(&full, &needle) + 2, list.len() as u32)
    })
    .collect();
    assert_eq!(
        list,
        expected
            .iter()
            .map(|&(offset, length)| (offset, length, WRONG_LIST_MESSAGE))
            .collect::<Vec<_>>(),
        "{:#?}",
        list.iter()
            .map(|&(o, l, _)| (o, l, anchored(&full, o, l)))
            .collect::<Vec<_>>()
    );
    let int = "int";
    assert_eq!(
        rows_of(&report, "anonymous_method_wrong_parameter_type"),
        vec![(at(&full, int), int.len() as u32, WRONG_TYPE_MESSAGE)],
        "{:#?}",
        report
            .diagnostics
            .iter()
            .map(|d| format!(
                "{} @{} {:?}",
                d.code.name,
                d.range.offset,
                anchored(&full, d.range.offset, d.range.length)
            ))
            .collect::<Vec<_>>()
    );
}

/// The shape rows survive inside a class-method body (the depth-1 formal
/// path the production mux fan-out feeds): the member formals closed
/// first-wins, so the anonymous list never attaches to the enclosing
/// method's capture.
#[test]
fn shape_rows_inside_a_method_body() {
    let source = "class C {\n  void m() {\n    1.() {}\n    1.(a, b) {}\n  }\n}\n";
    let full = format!("{CORE_PRELUDE}{source}");
    let report = report(source);
    let list = rows_of(&report, "anonymous_method_wrong_parameter_list");
    let expected: Vec<(u32, u32)> = ["() {}", "(a, b) {}"]
        .iter()
        .map(|shape| {
            let list = shape.split(" {}").next().expect("list");
            let needle = format!("1.{list}");
            (at(&full, &needle) + 2, list.len() as u32)
        })
        .collect();
    assert_eq!(
        list,
        expected
            .iter()
            .map(|&(offset, length)| (offset, length, WRONG_LIST_MESSAGE))
            .collect::<Vec<_>>(),
        "{:#?}",
        list
    );
}

/// The type row's null-aware face: the receiver promotes to non-null
/// before the assignability question (`promoteToNonNull`), and a
/// non-nullable String still misses `int`.
#[test]
fn null_aware_receiver_promotes_then_reports() {
    let source = "void main() {\n  \"\"?.(int i) {}\n}\n";
    let full = format!("{CORE_PRELUDE}{source}");
    let report = report(source);
    let int = "int";
    assert_eq!(
        rows_of(&report, "anonymous_method_wrong_parameter_type"),
        vec![(at(&full, int), int.len() as u32, WRONG_TYPE_MESSAGE)],
        "{:#?}",
        report
            .diagnostics
            .iter()
            .map(|d| format!("{} @{}", d.code.name, d.range.offset))
            .collect::<Vec<_>>()
    );
}

/// An identifier receiver types through the shared receiver walk: a
/// `String` local's read face answers, and the same mismatch reports.
#[test]
fn identifier_receiver_types_through_the_read_face() {
    let source = "void main() {\n  String s = \"\";\n  s.(int i) {}\n}\n";
    let full = format!("{CORE_PRELUDE}{source}");
    let report = report(source);
    let int = "int";
    assert_eq!(
        rows_of(&report, "anonymous_method_wrong_parameter_type"),
        vec![(at(&full, int), int.len() as u32, WRONG_TYPE_MESSAGE)],
        "{:#?}",
        report
            .diagnostics
            .iter()
            .map(|d| format!("{} @{}", d.code.name, d.range.offset))
            .collect::<Vec<_>>()
    );
}

/// The silence faces: an implicit list carries no constraint, an
/// unannotated formal takes the receiver's type, a matching annotation is
/// clean, a shape-passing dynamic parameter accepts everything, and the
/// super-error corpus's `this.`-receiver members (implicit lists) report
/// nothing on this face.
#[test]
fn clean_shapes_stay_silent() {
    let plain = report(
        "void main() {\n  1.{}\n  \"\".(i) {}\n  \"\".(String s) {}\n  \"\".(dynamic d) {}\n}\n",
    );
    assert_eq!(
        rows_of(&plain, "anonymous_method_wrong_parameter_list"),
        Vec::new(),
        "{:#?}",
        plain.diagnostics
    );
    assert_eq!(
        rows_of(&plain, "anonymous_method_wrong_parameter_type"),
        Vec::new(),
        "{:#?}",
        plain.diagnostics
    );
    let super_faces = report(
        "class B {\n  int i = 0;\n  void m() {}\n}\nclass D extends B {\n  void t() {\n    this.=> super.m();\n    this.{ super.i = 0; }\n  }\n}\n",
    );
    assert_eq!(
        rows_of(&super_faces, "anonymous_method_wrong_parameter_list"),
        Vec::new(),
        "{:#?}",
        super_faces.diagnostics
    );
    assert_eq!(
        rows_of(&super_faces, "anonymous_method_wrong_parameter_type"),
        Vec::new(),
        "{:#?}",
        super_faces.diagnostics
    );
}

/// An int receiver against a `String` parameter names both sides from the
/// primitive lattice (the message's `{0}`/`{1}` slot order).
#[test]
fn int_receiver_against_string_parameter_reports_both_sides() {
    let source = "void main() {\n  1.(String s) {}\n}\n";
    let full = format!("{CORE_PRELUDE}{source}");
    let report = report(source);
    let string = "String";
    assert_eq!(
        rows_of(&report, "anonymous_method_wrong_parameter_type"),
        vec![(
            at(&full, string),
            string.len() as u32,
            "The receiver type 'int' must be assignable to the formal parameter type 'String' in an anonymous method."
        )],
        "{:#?}",
        report.diagnostics
    );
}
