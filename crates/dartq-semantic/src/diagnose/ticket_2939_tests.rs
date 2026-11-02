//! The #2939 pins: the #2931 residual pack — the positional arm of
//! `element_usage_detector.dart`'s `_visitParametersAndArguments`
//! (:405-412, upstream Dart SDK source, pkg/analyzer/lib/src/error/
//! element_usage_detector.dart) and the constructor `displayName` join
//! (element.dart:838-847 — `Class.selector` for a named constructor, the
//! class's own spelling for the `new`/unnamed face).
//!
//! The positional arm drains the still-open argument level at each `name:`
//! label: upstream pairs `parameters[positionalIndex++]` per unlabeled
//! argument and anchors `checkUsage` on the argument expression, so a
//! deprecated positional formal reports at the argument's own span. A
//! label-free call (`f(0)`) has no drain point inside this lane's scope —
//! the arglist-close seam beside `solve_call_params`
//! (invocation.rs `close_arguments`) sits in a sibling lane's file and is
//! the deferred one-line wiring; the pins here cover every callee shape
//! the label-triggered drain reaches.
//!
//! Behavioral oracle: dart 3.13.7 `deprecated_member_use_test.dart`
//! parameter probes (`_visitParametersAndArguments`'s positional loop).

use super::*;
use crate::library::ElementIndex;
use crate::model::MicroKind;
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_parser::ParserFeatures;
use dartq_resolution::{Combinators, ImportEdge, LibraryGraphBuilder, Namespaces};
use dartq_source::{FileTable, StringTable};
use std::path::Path;

fn code_rows<'a>(report: &'a FileReport, name: &str) -> Vec<&'a Diagnostic> {
    report
        .diagnostics
        .iter()
        .filter(|row| row.code.name == name)
        .collect()
}

/// The same cross-package two-library rig `ticket_2924_tests` mounts: the
/// main unit imports the dependency plain so the declaring package's star
/// survives the `isInSamePackage` gate.
fn member_use_report(main: &str, dependency: &str, dependency_uri: &str) -> FileReport {
    let strings = StringTable::new();
    let mut files = FileTable::new();
    let file = files.intern(Path::new("main.dart"));
    let dep_file = files.intern(Path::new("kit.dart"));
    let set = FeatureSet::for_language_version(LanguageVersion::new(3, 15));
    let features = ParserFeatures::from_feature_set(set);
    let scanned = scan_fixture(main, file);
    let scanned_dep = scan_fixture(dependency, dep_file);
    let elements = crate::collector::collect(main, &scanned.tokens, features, &strings);
    let dep_elements =
        crate::collector::collect(dependency, &scanned_dep.tokens, features, &strings);
    let mut builder = LibraryGraphBuilder::new(&strings);
    let lib = builder.intern("package:t/main.dart");
    let dep_lib = builder.intern(dependency_uri);
    let meta = builder.intern("package:meta/meta.dart");
    builder.library_name(meta, "meta");
    for (name, decl) in elements.declarations() {
        if decl.kind == MicroKind::TypeParam {
            continue;
        }
        builder.declare(lib, name, file);
    }
    for (name, decl) in dep_elements.declarations() {
        if decl.kind == MicroKind::TypeParam {
            continue;
        }
        builder.declare(dep_lib, name, dep_file);
    }
    builder.unit(lib, file);
    builder.unit(dep_lib, dep_file);
    builder.import(
        lib,
        ImportEdge {
            target: Some(dep_lib),
            prefix: None,
            deferred: false,
            non_library: false,
            combinators: Combinators::unfiltered(),
        },
    );
    builder.import(
        lib,
        ImportEdge {
            target: Some(meta),
            prefix: None,
            deferred: false,
            non_library: false,
            combinators: Combinators::unfiltered(),
        },
    );
    builder.import(
        dep_lib,
        ImportEdge {
            target: Some(meta),
            prefix: None,
            deferred: false,
            non_library: false,
            combinators: Combinators::unfiltered(),
        },
    );
    let graph = builder.finish();
    let namespaces = Namespaces::build(&graph);
    let mut index = ElementIndex::default();
    index.insert(file, elements);
    index.insert(dep_file, dep_elements);
    fold_unit_with_ops(
        main,
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
        false,
        None,
    )
    .report
}

fn scan_fixture(source: &str, file: dartq_source::FileId) -> dartq_scanner::ScanResult {
    let scanned = dartq_scanner::scan(
        source,
        dartq_scanner::ScanConfig {
            enable_triple_shift: true,
            enable_augmentations: true,
        },
        file,
    );
    assert!(
        scanned.diagnostics.is_empty(),
        "scanner diagnostics: {source:?}"
    );
    scanned
}

// —— deprecated_member_use: the positional-parameter face ————————————————

/// An unlabeled argument bound to a deprecated positional formal reports
/// at the argument's own expression — upstream `checkUsage(parameter,
/// argument)` (the `parameters[positionalIndex++]` pairing, :405-412). The
/// drain runs at the first `name:` label while the group's level is open.
#[test]
fn dmu_deprecated_positional_parameter_top_level() {
    let main = "import 'package:s/kit.dart'; void g() { f(0, p: 1); }";
    let report = member_use_report(
        main,
        "void f(@deprecated int a, {int? p}) {}",
        "package:s/kit.dart",
    );
    let rows = code_rows(&report, "deprecated_member_use");
    assert_eq!(rows.len(), 1, "{report:?}");
    assert_eq!(rows[0].message, "'a' is deprecated and shouldn't be used.");
    // The argument-expression anchor, not the callee head.
    let arg_offset = main.find("0, p").expect("argument offset") as u32;
    assert_eq!(rows[0].range.offset, arg_offset, "{report:?}");
}

/// The member-callee form: the declaring owner resolves off the member
/// walk's `found_origin`, the argument pairs against the method's
/// positional table.
#[test]
fn dmu_deprecated_positional_parameter_member() {
    let report = member_use_report(
        "import 'package:s/kit.dart'; void g(A a) { a.m(0, p: 1); }",
        "class A { void m(@deprecated int a, {int? p}) {} }",
        "package:s/kit.dart",
    );
    let rows = code_rows(&report, "deprecated_member_use");
    assert_eq!(rows.len(), 1, "{report:?}");
    assert_eq!(rows[0].message, "'a' is deprecated and shouldn't be used.");
}

/// A named constructor's positional formal joins under the `Ctor.selector`
/// owner spelling — `ctor_formals`'s own lookup key.
#[test]
fn dmu_deprecated_positional_parameter_named_ctor() {
    let report = member_use_report(
        "import 'package:s/kit.dart'; void g() { A.named(0, p: 1); }",
        "class A { A.named(@deprecated int a, {int? p}); }",
        "package:s/kit.dart",
    );
    let rows = code_rows(&report, "deprecated_member_use");
    assert_eq!(rows.len(), 1, "{report:?}");
    assert_eq!(rows[0].message, "'a' is deprecated and shouldn't be used.");
}

/// The unnamed constructor's positional formal joins under the class's
/// own spelling — `ctor_annotations[A][A]`.
#[test]
fn dmu_deprecated_positional_parameter_unnamed_ctor() {
    let report = member_use_report(
        "import 'package:s/kit.dart'; void g() { A(0, p: 1); }",
        "class A { A(@deprecated int a, {int? p}); }",
        "package:s/kit.dart",
    );
    let rows = code_rows(&report, "deprecated_member_use");
    assert_eq!(rows.len(), 1, "{report:?}");
    assert_eq!(rows[0].message, "'a' is deprecated and shouldn't be used.");
}

/// An explicit super-constructor invocation's positional argument pairs
/// against the resolved superclass constructor's table — the same
/// `init_super_callee` resolution the named arm runs, the positional index
/// in place of the label join.
#[test]
fn dmu_deprecated_positional_parameter_super_invocation() {
    let report = member_use_report(
        "import 'package:s/kit.dart'; class C extends B { C() : super(0, p: 1); }",
        "class B { B(@deprecated int a, {int? p}); }",
        "package:s/kit.dart",
    );
    let rows = code_rows(&report, "deprecated_member_use");
    assert_eq!(rows.len(), 1, "{report:?}");
    assert_eq!(rows[0].message, "'a' is deprecated and shouldn't be used.");
}

/// A positional argument bound to an untagged formal stays silent (the
/// tag-absent direction of the same consult).
#[test]
fn dmu_plain_positional_parameter_is_silent() {
    let report = member_use_report(
        "import 'package:s/kit.dart'; void g() { f(0, p: 1); }",
        "void f(int a, {int? p}) {}",
        "package:s/kit.dart",
    );
    assert!(
        code_rows(&report, "deprecated_member_use").is_empty(),
        "{report:?}"
    );
}

/// The label arm is unchanged: a `name:` argument bound to a deprecated
/// named formal still reports at the label — the #2924 regression face.
#[test]
fn dmu_deprecated_named_parameter_regression() {
    let report = member_use_report(
        "import 'package:s/kit.dart'; void g() { f(p: 1); }",
        "void f({@deprecated int? p}) {}",
        "package:s/kit.dart",
    );
    let rows = code_rows(&report, "deprecated_member_use");
    assert_eq!(rows.len(), 1, "{report:?}");
    assert_eq!(rows[0].message, "'p' is deprecated and shouldn't be used.");
}

/// The mixed call reports each tagged parameter once — a positional and a
/// labeled argument to two different tagged formals emit two rows, one
/// per (argument, tag).
#[test]
fn dmu_mixed_positional_and_named_parameters() {
    let report = member_use_report(
        "import 'package:s/kit.dart'; void g() { f(0, p: 1); }",
        "void f(@deprecated int a, {@deprecated int? p}) {}",
        "package:s/kit.dart",
    );
    let rows = code_rows(&report, "deprecated_member_use");
    assert_eq!(rows.len(), 2, "{report:?}");
}

// —— deprecated_member_use: the constructor displayName join —————————————

/// A deprecated named constructor reports under its `displayName` —
/// `ConstructorElement.displayName`'s `$className.$name` composition
/// (element.dart:838-847), never the bare selector the `member_refs` edge
/// keys the lookup under.
#[test]
fn dmu_named_constructor_display_joins_class() {
    let report = member_use_report(
        "import 'package:s/kit.dart'; final x = A.named();",
        "class A { @deprecated A.named(); }",
        "package:s/kit.dart",
    );
    let rows = code_rows(&report, "deprecated_member_use");
    assert_eq!(rows.len(), 1, "{report:?}");
    assert_eq!(
        rows[0].message,
        "'A.named' is deprecated and shouldn't be used."
    );
}

/// The unnamed face's `displayName` is the class's own spelling — `A()`'s
/// lookup key normalizes to the container, and the rendered form follows
/// it (upstream's unnamed ConstructorElement has no selector segment).
#[test]
fn dmu_unnamed_constructor_display_is_class() {
    let report = member_use_report(
        "import 'package:s/kit.dart'; final x = A();",
        "class A { @deprecated A(); }",
        "package:s/kit.dart",
    );
    let rows = code_rows(&report, "deprecated_member_use");
    assert_eq!(rows.len(), 1, "{report:?}");
    assert_eq!(rows[0].message, "'A' is deprecated and shouldn't be used.");
}

/// The written `A.new()` selector normalizes the same way — `new` is the
/// unnamed face's alias upstream (`constructorName`'s name resolves to the
/// unnamed element), so the report renders the class alone.
#[test]
fn dmu_new_constructor_display_is_class() {
    let report = member_use_report(
        "import 'package:s/kit.dart'; final x = A.new();",
        "class A { @deprecated A(); }",
        "package:s/kit.dart",
    );
    let rows = code_rows(&report, "deprecated_member_use");
    assert_eq!(rows.len(), 1, "{report:?}");
    assert_eq!(rows[0].message, "'A' is deprecated and shouldn't be used.");
}

/// A deprecated method's display keeps the bare selector — the join is
/// the constructor arm's alone (a `MethodElement`'s `displayName` is the
/// member name).
#[test]
fn dmu_method_display_stays_bare() {
    let report = member_use_report(
        "import 'package:s/kit.dart'; void g(A a) { a.m(); }",
        "class A { @deprecated void m() {} }",
        "package:s/kit.dart",
    );
    let rows = code_rows(&report, "deprecated_member_use");
    assert_eq!(rows.len(), 1, "{report:?}");
    assert_eq!(rows[0].message, "'m' is deprecated and shouldn't be used.");
}

/// An unnamed extension's member carries `StringId::NONE` as the edge's
/// `target_container` (`note_extension_ref`'s `face.name.unwrap_or(NONE)`
/// — the nameless face has no container spelling). The member arm must
/// match the nameless face on the sentinel instead of falling through to
/// the constructor arm's `resolve(target_container)`, which panics on the
/// non-interned sentinel. An unnamed extension is library-local (it
/// exports under no name), so the only reachable shape is the same-file
/// use — the `isInSamePackage` gate then keeps the report silent, and the
/// pin locks the no-panic traversal.
#[test]
fn dmu_unnamed_extension_member_use() {
    let report = member_use_report(
        "class A {} extension on A { @deprecated int get foo => 1; }\nvoid g(A a) { a.foo; }",
        "void f() {}",
        "package:s/kit.dart",
    );
    let rows = code_rows(&report, "deprecated_member_use");
    assert!(rows.is_empty(), "{report:?}");
}
