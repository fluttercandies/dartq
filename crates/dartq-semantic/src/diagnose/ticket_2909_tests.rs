//! #2909 pins: the `undefined_operator` faces over typed/record/function
//! operands and the prefixed-type error-range spans.
//!
//! - The shared operator face (`check_operator_expr`) answers record and
//!   function operands with the full type render (`(int,) + 0`, a
//!   function-typed relational `f < g`), the scope-parameter bound lattice
//!   (`T extends num` resolves `+` through `num`), and the surviving typed
//!   receiver rows (the symbol literal's static face).
//! - The compound/prefix/postfix consult reports the undefined base
//!   operator at its own token (`x++` on an operator-less class names
//!   `+`, unary minus names `unary-`), an extension-declared operator
//!   resolving the read silent.
//! - `_getErrorRange`'s default span (named_type_resolver.dart:731-747):
//!   a prefixed type's `undefined_class` reports over the whole written
//!   `p.Name`, not the bare name; and `dynamic` resolves through an
//!   import prefix whose library re-exports dart:core's face.

use crate::collector::collect;
use crate::diagnose::{FileReport, StrictModes, diagnose_with};
use crate::library::ElementIndex;
use crate::model::MicroKind;
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_parser::ParserFeatures;
use dartq_resolution::{
    CombinatorClause, Combinators, ExportEdge, ImportEdge, LibraryClassModifiers,
    LibraryGraphBuilder, Namespaces,
};
use dartq_scanner::ScanConfig;
use dartq_source::{FileTable, StringTable};
use std::path::Path;

/// Operator rows as `(code, offset, length, message)` in emission order.
fn operator_rows(report: &FileReport) -> Vec<(&'static str, u32, u32, String)> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "undefined_operator")
        .map(|d| {
            (
                d.code.name,
                d.range.offset,
                d.range.length,
                d.message.as_str().to_owned(),
            )
        })
        .collect()
}

fn class_rows(report: &FileReport) -> Vec<(u32, u32, String)> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "undefined_class")
        .map(|d| {
            (
                d.range.offset,
                d.range.length,
                d.message.as_str().to_owned(),
            )
        })
        .collect()
}

/// The single-file rig (the tests.rs `report` shape): no SDK, core stand-ins
/// declared in the file itself (#537).
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

/// The two-library rig: `main.dart` imports `a.dart` as `p`; `a.dart` is a
/// graph node whose directives the rig mirrors (the export edge carries the
/// written combinators), and the `dart:core` node carries its face names.
fn report_with_prefix(source: &str, core_names: &[&str], show_dynamic_export: bool) -> FileReport {
    let strings = StringTable::new();
    let mut files = FileTable::new();
    let file = files.intern(Path::new("main.dart"));
    let _a_file = files.intern(Path::new("a.dart"));
    let core_file = files.intern(Path::new("<dart:core>"));
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
    let a_lib = builder.intern("package:t/a.dart");
    let core = builder.intern("dart:core");
    for name in core_names {
        builder.declare(core, strings.intern(name), core_file);
    }
    if show_dynamic_export {
        let show = Combinators::new(
            [CombinatorClause::Show(Box::from([
                strings.intern("dynamic")
            ]))],
            &strings,
        );
        builder.export(
            a_lib,
            ExportEdge {
                target: Some(core),
                combinators: show,
            },
        );
    }
    builder.import(
        lib,
        ImportEdge {
            target: Some(a_lib),
            prefix: Some(strings.intern("p")),
            deferred: false,
            non_library: false,
            combinators: Combinators::unfiltered(),
        },
    );
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

/// A record-typed operand declares no user operator: the miss reports with
/// the record's official display (`(String,)` — the corpus's
/// `test_expression_recordType_noOperator` shape over `int`).
#[test]
fn record_operand_reports_full_render() {
    let source = "void f((int,) a) { a + 0; }\n";
    let r = report(source);
    let rows = operator_rows(&r);
    let at = source.find('+').expect("anchor") as u32;
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0].1, at);
    assert_eq!(
        rows[0].3,
        "The operator '+' isn't defined for the type '(int,)'."
    );
}

/// A function-typed operand reports the relational miss with the
/// function's official display — the Constructor-tear-offs `a < b`
/// family (oracle: `The operator '<' isn't defined for the type
/// 'String Function<T1, T2>(int)'.`).
#[test]
fn function_operand_reports_full_render() {
    let source = "void f(int Function() x, int Function() y) { if (x < y) {} }\n";
    let r = report(source);
    let rows = operator_rows(&r);
    let at = source.find('<').expect("anchor") as u32;
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0].1, at);
    assert_eq!(
        rows[0].3,
        "The operator '<' isn't defined for the type 'int Function()'."
    );
}

/// A generic function tear-off's bare read carries its full signature —
/// the corpus's exact ambiguity-disambiguation message
/// (`ambiguities_A12_t02`: `String Function<T1, T2>(int)`).
#[test]
fn generic_tearoff_operand_reports_signature_display() {
    let source = "String a<T1, T2>(int x) => \"\";\nvoid f() { if (a < 1) {} }\n";
    let r = report(source);
    let rows = operator_rows(&r);
    let at = source.find('<').expect("anchor") as u32;
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0].1, at);
    assert_eq!(
        rows[0].3,
        "The operator '<' isn't defined for the type 'String Function<T1, T2>(int)'."
    );
}

/// A type-parameter receiver answers on its bound (the upstream
/// `resolveToBound` consult): `T extends num` keeps `t + 1` and `t++`
/// silent when `num` declares `+`; an unbounded `T` reports.
#[test]
fn bound_carries_operator_face() {
    let source = "\
class num {
  num operator +(num other) => this;
  num operator -() => this;
}
void f<T extends num>(T t) {
  t + 1;
  t++;
}
void g<T>(T t) {
  t + 1;
}
";
    let r = report(source);
    let rows = operator_rows(&r);
    let plus_at = source.rfind("+ 1").expect("anchor") as u32;
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0].1, plus_at);
    assert_eq!(
        rows[0].3,
        "The operator '+' isn't defined for the type 'T'."
    );
}

/// The index pair on an operator-less class (`a[0]++`): both the read and
/// the write spelling report at the bracket pair — the corpus's
/// `test_indexBoth_undefined` shape.
#[test]
fn index_pair_reports_both_spellings() {
    // The compound write rides the assignment upgrade's `OpWrite::Compound`
    // mark (the postfix `a[0]++` marks nothing yet — that consult lives in
    // the listener's postfix path).
    let source = "class A {}\nvoid f(A a) { a[0] += 1; }\n";
    let r = report(source);
    let rows = operator_rows(&r);
    let bracket = source.find('[').expect("anchor") as u32;
    assert_eq!(rows.len(), 2, "{rows:?}");
    assert_eq!((rows[0].1, rows[0].2), (bracket, 3), "{rows:?}");
    assert_eq!(
        rows[0].3,
        "The operator '[]' isn't defined for the type 'A'."
    );
    assert_eq!((rows[1].1, rows[1].2), (bracket, 3), "{rows:?}");
    assert_eq!(
        rows[1].3,
        "The operator '[]=' isn't defined for the type 'A'."
    );
}

/// `~` on a String operand: String declares no `~` — the prefix unary
/// reports at its own token with the receiver's display.
#[test]
fn tilde_on_string_reports() {
    let source = "class String {}\nvoid f(String s) { ~s; }\n";
    let r = report(source);
    let rows = operator_rows(&r);
    let at = source.find('~').expect("anchor") as u32;
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0].1, at);
    assert_eq!(
        rows[0].3,
        "The operator '~' isn't defined for the type 'String'."
    );
}

/// The compound/prefix/postfix consult: `x++`/`x--`/`x += 1` on an
/// operator-less class report the BASE operator at the compound token;
/// unary minus reports its `unary-` spelling.
#[test]
fn compound_and_inc_dec_report_base_operator() {
    let source = "class A {}\nvoid f(A x) { x++; x--; x += 1; -x; }\n";
    let r = report(source);
    let rows = operator_rows(&r);
    let pp = source.find("++").expect("anchor") as u32;
    let mm = source.find("--").expect("anchor") as u32;
    let pe = source.find("+=").expect("anchor") as u32;
    let neg = source.rfind('-').expect("anchor") as u32;
    assert_eq!(
        rows,
        vec![
            (
                "undefined_operator",
                pp,
                2,
                "The operator '+' isn't defined for the type 'A'.".to_owned()
            ),
            (
                "undefined_operator",
                mm,
                2,
                "The operator '-' isn't defined for the type 'A'.".to_owned()
            ),
            (
                "undefined_operator",
                pe,
                2,
                "The operator '+' isn't defined for the type 'A'.".to_owned()
            ),
            (
                "undefined_operator",
                neg,
                1,
                "The operator 'unary-' isn't defined for the type 'A'.".to_owned()
            ),
        ],
        "{rows:?}"
    );
}

/// An extension-declared operator resolves the compound read silent (the
/// upstream property consult answers through the extension).
#[test]
fn extension_declared_operator_keeps_compound_silent() {
    let source = "\
class A {}
extension E on A {
  A operator +(int other) => this;
}
void f(A x) { x++; x += 1; }
";
    let r = report(source);
    assert_eq!(operator_rows(&r), vec![], "{:?}", r.diagnostics);
}

/// A surviving typed receiver row (the symbol literal's static face)
/// answers through the shared operator face: `#void[0]` reports the index
/// miss on `Symbol`.
#[test]
fn typed_receiver_row_answers_operator_face() {
    let source = "void f() { #void[0]; }\n";
    let r = report(source);
    let rows = operator_rows(&r);
    let bracket = source.find('[').expect("anchor") as u32;
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!((rows[0].1, rows[0].2), (bracket, 3), "{rows:?}");
    assert_eq!(
        rows[0].3,
        "The operator '[]' isn't defined for the type 'Symbol'."
    );
}

/// `_getErrorRange`'s default: a prefixed type miss reports over the whole
/// written `p.Name` — the corpus's `math.Unresolved` 4:8|15 shape.
#[test]
fn prefixed_type_miss_spans_whole_name() {
    let source = "import 'a.dart' as p;\n\nvoid f(p.Unresolved<int> x) {}\n";
    let r = report_with_prefix(source, &[], false);
    let rows = class_rows(&r);
    let prefix = source.find("p.Unresolved").expect("anchor") as u32;
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(
        (rows[0].0, rows[0].1),
        (prefix, "p.Unresolved".len() as u32),
        "{rows:?}"
    );
    assert_eq!(rows[0].2, "Undefined class 'Unresolved'.");
}

/// `dynamic` resolves through a prefix whose library re-exports dart:core's
/// face (`export 'dart:core' show dynamic;` — the
/// `test_elements_export_dartCoreDynamic` shape stays silent).
#[test]
fn prefixed_dynamic_resolves_through_reexport() {
    let source = "import 'a.dart' as p;\n\np.dynamic f() => 1 as p.dynamic;\n";
    let r = report_with_prefix(source, &["dynamic"], true);
    assert_eq!(class_rows(&r), vec![], "{:?}", r.diagnostics);
}

/// The direct face: `import 'dart:core' as core;` + `core.dynamic` resolves
/// (the face carries `dynamic`; bare `dynamic` never needed the import).
#[test]
fn core_prefixed_dynamic_resolves_directly() {
    let source = "import 'dart:core' as core;\n\ncore.dynamic f() => 1 as core.dynamic;\n";
    let r = report_with_prefix(source, &["dynamic"], false);
    assert_eq!(class_rows(&r), vec![], "{:?}", r.diagnostics);
}
