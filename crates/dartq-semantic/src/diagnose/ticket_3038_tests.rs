//! The #3038 probes: `unnecessary_type_check` under-emission — sites the
//! deferred replay currently misses. Sources are distilled from the
//! conformance failures (param_null_to_object_test, expression_A01_t01,
//! named_constructor_A06_t01).

use crate::collector::collect;
use crate::diagnose::{FileReport, StrictModes, diagnose_with};
use crate::library::ElementIndex;
use crate::model::MicroKind;
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_parser::ParserFeatures;
use dartq_resolution::{
    Combinators, ImportEdge, LibraryClassModifiers, LibraryGraphBuilder, Namespaces,
};
use dartq_scanner::ScanConfig;
use dartq_source::{FileTable, StringTable};
use std::path::Path;

fn unit_report(source: &str) -> FileReport {
    let strings = StringTable::new();
    let mut files = FileTable::new();
    let file = files.intern(Path::new("main.dart"));
    let core_file = files.intern(Path::new("<dart:core>"));
    let set = FeatureSet::for_language_version(LanguageVersion::new(3, 15));
    let features = ParserFeatures::from_feature_set(set);
    let scanned = dartq_scanner::scan(
        source,
        ScanConfig {
            enable_triple_shift: true,
            enable_augmentations: true,
        },
        file,
    );
    assert!(scanned.diagnostics.is_empty(), "{source:?}");
    let elements = collect(source, &scanned.tokens, features, &strings);
    let mut builder = LibraryGraphBuilder::new(&strings);
    let lib = builder.intern("package:t/main.dart");
    builder.unit(lib, file);
    for (name, decl) in elements.declarations() {
        if decl.kind == MicroKind::TypeParam {
            continue;
        }
        builder.declare(lib, name, file);
    }
    builder.seed_implicit_core(
        lib,
        core_file,
        &[
            "Object", "String", "int", "bool", "double", "num", "List", "Map",
            "Set", "Type", "Function", "Null", "Iterable",
        ],
    );
    let graph = builder.finish();
    let namespaces = Namespaces::build(&graph);
    let mut index = ElementIndex::default();
    index.insert(file, elements);
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

fn rows(report: &FileReport, code: &str) -> Vec<(u32, u32)> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == code)
        .map(|d| (d.range.offset, d.range.length))
        .collect()
}

fn names(report: &FileReport) -> Vec<&'static str> {
    report.diagnostics.iter().map(|d| d.code.name).collect()
}

/// Local declared `int Function(Null)` is `int Function(Null)` — always true.
#[test]
fn local_fn_null_is_same_signature_fires() {
    let report = unit_report(
        "void t() {\n  int Function(Null) f = (x) => 1;\n  if (f is int Function(Null)) {}\n}\n",
    );
    eprintln!("diags: {:?}", names(&report));
    assert!(
        !rows(&report, "unnecessary_type_check").is_empty(),
        "{:?}",
        report.diagnostics
    );
}

/// `var c = int; c is Type` — the type literal is a Type, always true.
#[test]
fn type_literal_is_type_fires() {
    let report = unit_report("void t() {\n  var c = int;\n  if (c is Type) {}\n}\n");
    eprintln!("diags: {:?}", names(&report));
    assert!(
        !rows(&report, "unnecessary_type_check").is_empty(),
        "{:?}",
        report.diagnostics
    );
}

/// `var c = C; c is C` — `c`'s inferred type is `Type`; `Type` is no
/// subtype of `C`, so the verdict stays silent upstream (the
/// `isSubtypeOf` gate alone decides — no constant-false row exists).
#[test]
fn type_literal_is_class_stays_silent() {
    let report =
        unit_report("class C {}\nvoid t() {\n  var c = C;\n  if (c is C) {}\n}\n");
    eprintln!("diags: {:?}", names(&report));
    assert!(
        rows(&report, "unnecessary_type_check").is_empty(),
        "{:?}",
        report.diagnostics
    );
}

/// `var c = C<int>; c is Type` — generic type literal.
#[test]
fn generic_type_literal_is_type_fires() {
    let report = unit_report(
        "class C<T> {}\nvoid t() {\n  var c = C<int>;\n  if (c is Type) {}\n}\n",
    );
    eprintln!("diags: {:?}", names(&report));
    assert!(
        !rows(&report, "unnecessary_type_check").is_empty(),
        "{:?}",
        report.diagnostics
    );
}

/// Top-level function tear-off vs typedef spelling — the function_type
/// family shape (`f2 is F2<int>`), prefixed generic leaves.
#[test]
fn top_level_fn_prefixed_sig_is_typedef_fires() {
    let report = unit_report(
        "typedef F<T> = List<int> Function(int, [List<int> x]);\n\
         List<int> f(int a, [List<int> x = const []]) => x;\n\
         void e(bool b) {}\n\
         void t() {\n  e(f is F<int>);\n}\n",
    );
    eprintln!("diags: {:?}", names(&report));
    assert!(
        !rows(&report, "unnecessary_type_check").is_empty(),
        "{:?}",
        report.diagnostics
    );
}

/// Local generic method tear-off vs typedef (`m2 is F2<T>`).
#[test]
fn local_generic_fn_is_typedef_fires() {
    let report = unit_report(
        "typedef F<T> = List<int> Function(int, [List<int> x]);\n\
         void e(bool b) {}\n\
         void t() {\n  List<int> m(int a, [List<int> x = const []]) => x;\n  e(m is F<int>);\n}\n",
    );
    eprintln!("diags: {:?}", names(&report));
    assert!(
        !rows(&report, "unnecessary_type_check").is_empty(),
        "{:?}",
        report.diagnostics
    );
}

/// `f is int Function(Null)` inside a static-method call argument —
/// the exact `Expect.isTrue` shape (prefixed member send).
#[test]
fn local_fn_null_is_same_signature_in_static_arg_fires() {
    let report = unit_report(
        "class E {\n  static void isTrue(bool b) {}\n}\nvoid t() {\n  int Function(Null) f = (x) => 1;\n  E.isTrue(f is int Function(Null));\n}\n",
    );
    eprintln!("diags: {:?}", names(&report));
    assert!(
        !rows(&report, "unnecessary_type_check").is_empty(),
        "{:?}",
        report.diagnostics
    );
}

/// `f is int Function(Null)` inside a call argument — the dominant
/// conformance shape (param_null_to_object).
#[test]
fn local_fn_null_is_same_signature_in_call_arg_fires() {
    let report = unit_report(
        "void e(bool b) {}\nvoid t() {\n  int Function(Null) f = (x) => 1;\n  e(f is int Function(Null));\n}\n",
    );
    eprintln!("diags: {:?}", names(&report));
    assert!(
        !rows(&report, "unnecessary_type_check").is_empty(),
        "{:?}",
        report.diagnostics
    );
}

/// Two-file harness: `main.dart` imports `core.dart` under the `core`
/// prefix (the conformance shape `import 'dart:core' as core;`) so a
/// typedef's `core.`-prefixed leaves resolve through the import edge.
fn prefix_report(main: &str, prefix_name: &str, lib_src: &str) -> FileReport {
    let strings = StringTable::new();
    let mut files = FileTable::new();
    let file_main = files.intern(Path::new("main.dart"));
    let file_lib = files.intern(Path::new("core.dart"));
    let scan = |source: &str, file: dartq_source::FileId| {
        let scanned = dartq_scanner::scan(
            source,
            ScanConfig {
                enable_triple_shift: true,
                enable_augmentations: true,
            },
            file,
        );
        assert!(scanned.diagnostics.is_empty(), "{source:?}");
        scanned
    };
    let scanned_main = scan(main, file_main);
    let scanned_lib = scan(lib_src, file_lib);
    let set = FeatureSet::for_language_version(LanguageVersion::new(3, 15));
    let features = ParserFeatures::from_feature_set(set);
    let elements_main = collect(main, &scanned_main.tokens, features, &strings);
    let elements_lib = collect(lib_src, &scanned_lib.tokens, features, &strings);
    let mut builder = LibraryGraphBuilder::new(&strings);
    let lib_main = builder.intern("package:t/main.dart");
    let lib_core = builder.intern("dart:core");
    let mut index = ElementIndex::default();
    index.insert(file_main, elements_main.clone());
    index.insert(file_lib, elements_lib.clone());
    for (lib, file, elements) in [
        (lib_main, file_main, &elements_main),
        (lib_core, file_lib, &elements_lib),
    ] {
        builder.unit(lib, file);
        for (name, decl) in elements.declarations() {
            if decl.kind == MicroKind::TypeParam {
                continue;
            }
            builder.declare(lib, name, file);
        }
    }
    builder.import(
        lib_main,
        ImportEdge {
            target: Some(lib_core),
            prefix: Some(strings.intern(prefix_name)),
            deferred: false,
            non_library: false,
            combinators: Combinators::unfiltered(),
        },
    );
    // dart:core is also auto-imported unprefixed — `core.List` and `List`
    // name the same declaration, the conformance files' real shape
    // (`import 'dart:core' as core` rides on top of the implicit import).
    builder.import(
        lib_main,
        ImportEdge {
            target: Some(lib_core),
            prefix: None,
            deferred: false,
            non_library: false,
            combinators: Combinators::unfiltered(),
        },
    );
    let graph = builder.finish();
    let namespaces = Namespaces::build(&graph);
    diagnose_with(
        main,
        &scanned_main.tokens,
        features,
        &strings,
        file_main,
        lib_main,
        &graph,
        &namespaces,
        &index,
        StrictModes::default(),
        &LibraryClassModifiers::default(),
        set.is_enabled(dartq_language::Feature::InferenceUpdate2),
    )
}

/// `typedef F = core.List<core.int> Function({core.List<core.int> x})`
/// against `f: core.List<core.int> Function({core.List<core.int> x})` —
/// the function_type family's dominant miss (`f2 is F2<int>`): the
/// typedef RHS's `core.`-prefixed leaves must subtype-match the operand's
/// identically spelled signature.
///
/// Blocked on the collector's `pending_qualified` clobber (#3097): a
/// `p.T<p.U>` spelling inside the typedef's Function RHS lets the inner
/// `p` overwrite the outer join state, so `F`'s captured `var_ty` degrades
/// to `Named{"core"}` and `F<int>` cannot fold to the signature. The
/// spelled-site half already emits today (the operand's own `core.List`
/// args match the written `core.List` leaves head-for-head).
#[test]
#[ignore = "blocked on #3097: typedef Function-RHS var_ty degrades to the bare prefix"]
fn prefixed_typedef_fold_is_same_signature_fires() {
    let core_lib = "class List<E> {}\nclass int {}\nclass bool {}\n";
    let report = prefix_report(
        "import 'core.dart' as core;\n\
         typedef F<T> = core.List<core.int> Function({core.List<core.int> x});\n\
         core.List<core.int> f({core.List<core.int> x = const []}) => x;\n\
         void e(bool b) {}\n\
         void t() {\n  e(f is F<int>);\n  e(f is core.List<core.int> Function({core.List<core.int> x}));\n}\n",
        "core",
        core_lib,
    );
    eprintln!("diags: {:?}", names(&report));
    let utc = rows(&report, "unnecessary_type_check");
    assert_eq!(utc.len(), 2, "{:?}", report.diagnostics);
}

/// The LOCAL generic-fn operand shape (`m2 is F2<T>` inside `testF2`) —
/// isolates whether a local function's tear-off read carries a deferred
/// read-type row at all (the deferred channel's entry condition).
///
/// Blocked on #3087 (listener/resolved_ops): a local function's tear-off
/// read stages no `read_types` row, so the deferred channel never reaches
/// the site — the operand's declared signature is invisible post-walk and
/// pass-1's operand face stays silent on the `m is FN<T>` family.
#[test]
#[ignore = "blocked on #3087: local-fn tear-off stages no read_type row"]
fn local_fn_prefixed_typedef_fold_fires() {
    let core_lib = "class List<E> {}\nclass int {}\nclass bool {}\n";
    let report = prefix_report(
        "import 'core.dart' as core;\n\
         typedef F<T> = core.List<core.int> Function({core.List<core.int> x});\n\
         void e(bool b) {}\n\
         void t() {\n  core.List<core.int> m({core.List<core.int> x = const []}) => x;\n  e(m is F<int>);\n  e(m is core.List<core.int> Function({core.List<core.int> x}));\n}\n",
        "core",
        core_lib,
    );
    eprintln!("diags: {:?}", names(&report));
    let utc = rows(&report, "unnecessary_type_check");
    assert_eq!(utc.len(), 2, "{:?}", report.diagnostics);
}

/// Non-function RHS fold probe: `typedef G<T> = core.List<core.int>` against
/// `x: core.List<core.int>` — isolates whether `F<int>` folding itself
/// resolves through a `core.`-prefixed leaf (vs the Function-RHS arm).
///
/// Blocked on the same #3097 clobber: `p.T<p.U>` — here both the typedef
/// RHS AND the parameter annotation `core.List<core.int>` capture as
/// `Named{"core"}` (the inner `p` identifier's `handle_identifier` fires
/// `PrefixedTypeReference` before the outer `handle_type` can join), so
/// neither face survives to the verdict. The bare-`p.T` arm
/// (`prefixed_typedef_bare_rhs_fold_fires`) proves the canonicalized fold
/// itself once the capture is intact.
#[test]
#[ignore = "blocked on #3097: p.T<p.U> captures as the bare prefix name"]
fn prefixed_typedef_named_rhs_fold_fires() {
    let core_lib = "class List<E> {}\nclass int {}\nclass bool {}\n";
    let report = prefix_report(
        "import 'core.dart' as core;\n\
         typedef G<T> = core.List<core.int>;\n\
         void e(bool b) {}\n\
         void t(core.List<core.int> x) {\n  e(x is G<int>);\n  e(x is core.List<core.int>);\n}\n",
        "core",
        core_lib,
    );
    eprintln!("diags: {:?}", names(&report));
    let utc = rows(&report, "unnecessary_type_check");
    assert_eq!(utc.len(), 2, "{:?}", report.diagnostics);
}

/// Function-RHS fold probe minus `core.` leaves: `typedef H<T> = List<int>
/// Function({List<int> x})` in the prefixed harness — same shape as the
/// failing pin, unprefixed leaves.
#[test]
fn unprefixed_typedef_fn_rhs_fold_fires() {
    let core_lib = "class List<E> {}\nclass int {}\nclass bool {}\n";
    let report = prefix_report(
        "import 'core.dart' as core;\n\
         typedef H<T> = List<int> Function({List<int> x});\n\
         List<int> h({List<int> x = const []}) => x;\n\
         void e(bool b) {}\n\
         void t() {\n  e(h is H<int>);\n}\n",
        "core",
        core_lib,
    );
    eprintln!("diags: {:?}", names(&report));
    let utc = rows(&report, "unnecessary_type_check");
    assert_eq!(utc.len(), 1, "{:?}", report.diagnostics);
}

/// Bare `p.T` typedef RHS (no nested prefix inside args): isolates whether
/// the corrupt RHS face is specific to `p.T<p.U>` nestings (the inner
/// prefix's `handle_identifier` clobbering `pending_qualified` before the
/// outer `handle_type` joins).
#[test]
fn prefixed_typedef_bare_rhs_fold_fires() {
    let core_lib = "class List<E> {}\nclass int {}\nclass bool {}\n";
    let report = prefix_report(
        "import 'core.dart' as core;\n\
         typedef J<T> = core.List;\n\
         void e(bool b) {}\n\
         void t(core.List x) {\n  e(x is J<int>);\n  e(x is core.List);\n}\n",
        "core",
        core_lib,
    );
    eprintln!("diags: {:?}", names(&report));
    let utc = rows(&report, "unnecessary_type_check");
    assert_eq!(utc.len(), 2, "{:?}", report.diagnostics);
}

/// `var l = <dynamic>[A()]; var o = l[0]; o is A` — the inheritance_chain
/// extra-emit shape. `o` is `dynamic`; `dynamic <: A` is false upstream, so
/// the verdict must stay silent.
#[test]
fn dynamic_operand_is_class_stays_silent() {
    let report = unit_report(
        "class A {}\nvoid e(bool b) {}\nvoid t() {\n  var l = <dynamic>[A()];\n  var o = l[0];\n  e(o is A);\n}\n",
    );
    eprintln!("diags: {:?}", names(&report));
    assert!(
        rows(&report, "unnecessary_type_check").is_empty(),
        "{:?}",
        report.diagnostics
    );
}

/// The conformance shape exactly: the `is` sits inside a call argument.
#[test]
fn generic_type_literal_is_type_in_call_arg_fires() {
    let report = unit_report(
        "class C<T> {}\nvoid e(bool b) {}\nvoid t() {\n  var c = C<int>;\n  e(c is Type);\n  e(c is C);\n}\n",
    );
    eprintln!("diags: {:?}", names(&report));
    let utc = rows(&report, "unnecessary_type_check");
    assert_eq!(utc.len(), 1, "{:?}", report.diagnostics);
    // `c is C` (Type vs C) stays silent upstream; only `c is Type` fires.
    assert!(
        !report.diagnostics.iter().any(|d| d.message.contains("'C'")),
        "{:?}",
        report.diagnostics
    );
}
