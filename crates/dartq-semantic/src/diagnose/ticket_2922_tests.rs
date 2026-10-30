//! The #2922 `argument_type_not_assignable` pins: the invocation/bounds
//! faces the argument walk decides.
//!
//! 1. The superinterface downward solve (`asInstanceOf`'s hop
//!    substitution): `B<S, T> extends A<T, S>` under an `A<int, String>`
//!    context solves `B<String, int>`, so the creation's formals check
//!    `String`/`int` — not the raw `S`/`T` spellings an unsolved slot
//!    leaves (StrongModeLocalInferenceTest `test_instanceCreation#0`
//!    rows 42:35/42:44/54:35/54:38, oracle Dart 3.13).
//! 2. The folded-creation receiver's generic-method inference
//!    (`new C().m(42)` — `Receiver::Created` is the instance face):
//!    `m<T>(T x)` binds `T := int` upward, the send stays silent
//!    (InferredTypeTest `test_inferGenericMethodType_required#0`,
//!    `test_inferGenericMethodType_named#0` — oracle silent).
//! 3. The closure-literal argument pairing: a `() => 0` argument's
//!    captured signature pairs through `level.closures` against the
//!    callee's written function-typed formal — `a.n(() => 0)` over
//!    `n(void f(int i))` reports the arity face
//!    (ArgumentTypeNotAssignableTest `test_functionType#0` family —
//!    #2921's `probe_function_type_arity` rides along).
//! 4. The extension-type primary-constructor instantiation: `E(a)` over
//!    `extension type E(int i)` checks `a` against `int`
//!    (ArgumentTypeNotAssignableWithStrictCastsTest
//!    `test_extensionTypePrimaryConstructor#0`).
//!
//! Oracle provenance: the #2922 census rows — all citations
//! `conformance/expected.tsv` (Dart 3.13).

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
    report_modes(source, StrictModes::default())
}

/// `report` under explicit strict modes — the `strict-casts` face needs
/// the cast posture flag the single-file harness otherwise leaves off.
fn report_modes(source: &str, modes: StrictModes) -> FileReport {
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
        modes,
        &LibraryClassModifiers::default(),
        set.is_enabled(dartq_language::Feature::InferenceUpdate2),
    );
    let mut report = fold.report;
    report.diagnostics = merge_source_ordered(fold.parse_diagnostics, report.diagnostics);
    report
}

fn atna_rows(report: &FileReport) -> Vec<(u32, u32, String)> {
    let mut rows: Vec<(u32, u32, String)> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "argument_type_not_assignable")
        .map(|d| (d.range.offset, d.range.length, d.message.to_string()))
        .collect();
    rows.sort();
    rows
}

// —— face 1: superinterface downward solve ——

/// `new B(3, "hello")` under `A<int, String>`: `B<S, T> extends A<T, S>`
/// inverts to `B<String, int>` — `int` vs `y: String` and `String` vs
/// `x: int` both report (oracle's rows at 54:35/54:38 in the census case).
#[test]
fn superinterface_downward_solve_reports_substituted_formals() {
    let report = report(
        "class A<S, T> {\n\
         \x20 S x;\n\
         \x20 T y;\n\
         \x20 A(this.x, this.y);\n\
         }\n\
         class B<S, T> extends A<T, S> {\n\
         \x20 B(S y, T x) : super(x, y);\n\
         }\n\
         main() {\n\
         \x20 A<int, String> a0 = new B(3, \"hello\");\n\
         }\n",
    );
    let rows = atna_rows(&report);
    assert_eq!(rows.len(), 2, "both substituted formals reject: {rows:?}");
    let texts: Vec<&str> = rows.iter().map(|(_, _, m)| m.as_str()).collect();
    assert!(
        texts
            .iter()
            .any(|m| *m
                == "The argument type 'int' can't be assigned to the parameter type 'String'."),
        "int into String formal: {rows:?}"
    );
    assert!(
        texts
            .iter()
            .any(|m| *m
                == "The argument type 'String' can't be assigned to the parameter type 'int'."),
        "String into int formal: {rows:?}"
    );
}

/// `new B("hello", 3)` under the same context solves `B<String, int>` —
/// `hello` vs `String` and `3` vs `int` stay silent (the oracle's
/// matching-shape arm).
#[test]
fn superinterface_downward_solve_matching_args_stay_silent() {
    let report = report(
        "class A<S, T> {\n\
         \x20 S x;\n\
         \x20 T y;\n\
         \x20 A(this.x, this.y);\n\
         }\n\
         class B<S, T> extends A<T, S> {\n\
         \x20 B(S y, T x) : super(x, y);\n\
         }\n\
         main() {\n\
         \x20 A<int, String> a0 = new B(\"hello\", 3);\n\
         }\n",
    );
    let rows = atna_rows(&report);
    assert!(rows.is_empty(), "matching args stay silent: {rows:?}");
}

/// `new D(3)` under `A<int, String>` climbs two hops:
/// `D<S, T> extends B<T, int>`, `B<S, T> extends A<T, S>` — the clause
/// chain substitutes to `A<int, T>`, so `T := String` and `3` reports
/// `int` into `String` (the census's test7 rows at 78:35/79:41).
#[test]
fn superinterface_downward_solve_climbs_two_hops() {
    let report = report(
        "class A<S, T> {\n\
         \x20 S x;\n\
         \x20 T y;\n\
         \x20 A(this.x, this.y);\n\
         }\n\
         class B<S, T> extends A<T, S> {\n\
         \x20 B(S y, T x) : super(x, y);\n\
         }\n\
         class D<S, T> extends B<T, int> {\n\
         \x20 D(T a) : super(a, 3);\n\
         }\n\
         main() {\n\
         \x20 A<int, String> a0 = new D(3);\n\
         }\n",
    );
    let rows = atna_rows(&report);
    assert!(
        rows.iter().any(|(_, _, m)| m
            == "The argument type 'int' can't be assigned to the parameter type 'String'."),
        "the two-hop solve reports int into String: {rows:?}"
    );
}

// —— face 2: folded-creation generic-method inference ——

/// `new C().m(42)` — `m<T>(T x)` binds `T := int` upward; the oracle is
/// silent (test_inferGenericMethodType_required#0's only row is the
/// unused-local warning).
#[test]
fn created_receiver_generic_method_solves_upward() {
    let report = report(
        "class C {\n\
         \x20 T m<T>(T x) => x;\n\
         }\n\
         main() {\n\
         \x20 var y = new C().m(42);\n\
         }\n",
    );
    let rows = atna_rows(&report);
    assert!(
        rows.is_empty(),
        "T := int solves the send silently: {rows:?}"
    );
}

/// `new C().m(1, c: 2.0)` — the named `c: T` binds `T := double` upward;
/// oracle silent (test_inferGenericMethodType_named#0).
#[test]
fn created_receiver_generic_method_named_arg_solves_upward() {
    let report = report(
        "class C {\n\
         \x20 T m<T>(int a, {String? b, T? c}) => b == null ? a as T : c;\n\
         }\n\
         main() {\n\
         \x20 var y = new C().m(1, b: 'bbb', c: 2.0);\n\
         }\n",
    );
    let rows = atna_rows(&report);
    assert!(
        rows.is_empty(),
        "T := double solves the named arg silently: {rows:?}"
    );
}

// —— face 3: closure-literal argument pairing ——

/// `a.n(() => 0)` over `n(void f(int i))` — the closure's own
/// `void Function()` shape pairs against the written formal and reports
/// the arity face (the #2921 rider probe's exact message).
#[test]
fn closure_argument_pairs_against_written_formal() {
    let report = report(
        "m() {\n\
         \x20 var a = new A();\n\
         \x20 a.n(() => 0);\n\
         }\n\
         class A {\n\
         \x20 n(void f(int i)) {}\n\
         }\n",
    );
    let rows = atna_rows(&report);
    assert_eq!(rows.len(), 1, "the arity face reports: {rows:?}");
    assert_eq!(
        rows[0].2,
        "The argument type 'void Function()' can't be assigned to the \
         parameter type 'void Function(int)'.",
        "the arity message: {rows:?}"
    );
}

// —— face 4: extension-type primary-ctor instantiation ——

/// `E(a)` over `extension type E(int i)` with `a: dynamic` — under
/// `strict-casts` the dynamic source reports (the
/// `test_extensionTypePrimaryConstructor#0` oracle row).
#[test]
fn extension_type_primary_ctor_checks_dynamic_arg() {
    let report = report_modes(
        "extension type E(int i) {}\n\
         dynamic a = 0;\n\
         var e = E(a);\n",
        StrictModes {
            casts: true,
            ..StrictModes::default()
        },
    );
    let rows = atna_rows(&report);
    assert!(
        rows.iter().any(|(_, _, m)| m
            == "The argument type 'dynamic' can't be assigned to the parameter type 'int'."),
        "dynamic into int under strict-casts: {rows:?}"
    );
}
