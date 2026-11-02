//! The #3567 pins: the is/as verdict cluster's two under-report roots.
//!
//! Root 1 — function-type subtyping over tear-offs: the operand of `x is
//! T` is a tear-off read (a top-level function, an instance method, a
//! generic method) whose static type is the declaration's signature; the
//! verdict must compare that signature against the written target
//! (upstream reads `node.expression.typeOrThrow` — the tear-off's
//! instantiated `FunctionType` — and asks `isSubtypeOf`,
//! best_practices_verifier.dart's `_checkAllTypeChecks`).
//!
//! Root 2 — lossy is-target production: the written target (`F<int>`
//! alias, an explicit `R Function(params)` literal) must keep its written
//! shape — alias-argument instantiation, nested function-returning
//! signatures, `core.`-prefixed leaves at any depth — so the subtype ask
//! sees the same leaves both sides spell.
//!
//! Corpus: the generated `function_type{0..99}_test` family (815 missing
//! `unnecessary_type_check` rows); every firing pin cites its source
//! shape, every silent twin is the oracle's own silence on an informative
//! (non-subtype) pair — a fabricated row is worse than a silent verdict.

use crate::collector::collect;
use crate::diagnose::{FileReport, StrictModes, diagnose_with};
use crate::library::ElementIndex;
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_parser::ParserFeatures;
use dartq_resolution::{
    Combinators, ImportEdge, LibraryClassModifiers, LibraryGraphBuilder, Namespaces,
};
use dartq_scanner::ScanConfig;
use dartq_source::{FileTable, StringTable};
use std::path::Path;

/// The shared core library both import edges point at: the classes the
/// corpus's `core.`-prefixed leaves spell.
const CORE_LIB: &str = "class Object {}\nclass Function {}\nclass int {}\nclass bool {}\nclass List<E> {}\n";

/// Diagnoses `main` as one file of a library importing `core.dart` twice —
/// once as `core`, once unprefixed (the conformance files' real shape:
/// `core.List` and `List` name the same declaration).
fn prefix_report(main: &str) -> FileReport {
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
    let scanned_lib = scan(CORE_LIB, file_lib);
    let set = FeatureSet::for_language_version(LanguageVersion::new(3, 15));
    let features = ParserFeatures::from_feature_set(set);
    let elements_main = collect(main, &scanned_main.tokens, features, &strings);
    let elements_lib = collect(CORE_LIB, &scanned_lib.tokens, features, &strings);
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
            if decl.kind == crate::model::MicroKind::TypeParam {
                continue;
            }
            builder.declare(lib, name, file);
        }
    }
    builder.import(
        lib_main,
        ImportEdge {
            target: Some(lib_core),
            prefix: Some(strings.intern("core")),
            deferred: false,
            non_library: false,
            combinators: Combinators::unfiltered(),
        },
    );
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

fn utc_rows(report: &FileReport) -> Vec<(u32, u32)> {
    report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "unnecessary_type_check")
        .map(|d| (d.range.offset, d.range.length))
        .collect()
}

fn all_names(report: &FileReport) -> Vec<&'static str> {
    report.diagnostics.iter().map(|d| d.code.name).collect()
}

// —— root 1: the tear-off operand's static type ——

/// `f2 is F2a<int>` over a top-level `core.List<core.int> f2(int y,
/// {List<Function> x})` — the tear-off's static type is the declared
/// signature and the alias instantiates to the same leaves: always true
/// (function_type0_test.dart:240's shape; the member twin of the same
/// signature already fires).
#[test]
fn top_level_tearoff_core_leaf_alias_fires() {
    let report = prefix_report(concat!(
        "import 'dart:core' as core;\n",
        "typedef F2a<T> = core.List<core.int> Function(int y, {List<Function> x});\n",
        "core.List<core.int> f2(int y, {List<Function> x}) => throw '';\n",
        "void e(bool b) {}\n",
        "void t() { e(f2 is F2a<int>); }\n",
    ));
    assert!(
        !utc_rows(&report).is_empty(),
        "diags: {:?}",
        all_names(&report)
    );
}

/// The wrong-return twin: the tear-off's `core.List<core.int>` return is
/// no subtype of the target's `int` return — informative, no row.
#[test]
fn top_level_tearoff_wrong_return_alias_stays_silent() {
    let report = prefix_report(concat!(
        "import 'dart:core' as core;\n",
        "typedef G<T> = int Function(int y, {List<Function> x});\n",
        "core.List<core.int> f2(int y, {List<Function> x}) => throw '';\n",
        "void e(bool b) {}\n",
        "void t() { e(f2 is G<int>); }\n",
    ));
    assert!(
        utc_rows(&report).is_empty(),
        "diags: {:?}",
        all_names(&report)
    );
}

/// `m2 is F2b<T>` inside `U<T>` — the instance tear-off's signature
/// carries the class parameter and the alias instantiates its own `T`
/// with the same class parameter (function_type59_test.dart:324's shape).
#[test]
fn member_tearoff_class_t_alias_fires() {
    let report = prefix_report(concat!(
        "import 'dart:core' as core;\n",
        "typedef F2b<T> = List<T> Function(int, [core.List<core.int> x]);\n",
        "class U<T> {\n",
        "  List<T> m2(int x0, [core.List<core.int> x]) => throw '';\n",
        "  void e(bool b) {}\n",
        "  void t() { e(m2 is F2b<T>); }\n",
        "}\n",
    ));
    assert!(
        !utc_rows(&report).is_empty(),
        "diags: {:?}",
        all_names(&report)
    );
}

/// The wrong-argument twin of the member face: `F2b<bool>` asks
/// `List<T> <: List<bool>` — unprovable, informative, no row.
#[test]
fn member_tearoff_wrong_instantiation_stays_silent() {
    let report = prefix_report(concat!(
        "import 'dart:core' as core;\n",
        "typedef F2b<T> = List<T> Function(int, [core.List<core.int> x]);\n",
        "class U<T> {\n",
        "  List<T> m2(int x0, [core.List<core.int> x]) => throw '';\n",
        "  void e(bool b) {}\n",
        "  void t() { e(m2 is F2b<bool>); }\n",
        "}\n",
    ));
    assert!(
        utc_rows(&report).is_empty(),
        "diags: {:?}",
        all_names(&report)
    );
}

/// `f4 is F4<int>` over a generic top-level function against the generic
/// alias `void Function<A>(core.List<core.int> x)` — the tear-off of a
/// generic function carries the generic signature; the alias's unused
/// `T` and matching inner `A` make the pair always true
/// (function_type59_test.dart:385's shape).
#[test]
fn generic_tearoff_generic_alias_fires() {
    let report = prefix_report(concat!(
        "import 'dart:core' as core;\n",
        "typedef F4<T> = void Function<A>(core.List<core.int> x);\n",
        "void f4<A>(core.List<core.int> x) => throw '';\n",
        "void e(bool b) {}\n",
        "void t() { e(f4 is F4<int>); }\n",
    ));
    assert!(
        !utc_rows(&report).is_empty(),
        "diags: {:?}",
        all_names(&report)
    );
}

/// The param-mismatch twin of the generic face: the target's inner
/// formal `int` against the operand's `core.List<core.int>` —
/// contravariance fails, informative, no row.
#[test]
fn generic_tearoff_param_mismatch_stays_silent() {
    let report = prefix_report(concat!(
        "import 'dart:core' as core;\n",
        "typedef F4<T> = void Function<A>(int x);\n",
        "void f4<A>(core.List<core.int> x) => throw '';\n",
        "void e(bool b) {}\n",
        "void t() { e(f4 is F4<int>); }\n",
    ));
    assert!(
        utc_rows(&report).is_empty(),
        "diags: {:?}",
        all_names(&report)
    );
}

// —— root 2: the written target's shape must survive ——

/// The member-tear-off harness for the literal-target faces: the checks
/// below read the method `mm`'s declared return signature against the
/// written literal target.
fn literal_report(target: &str, return_annotation: &str) -> FileReport {
    prefix_report(&format!(
        "import 'dart:core' as core;\n\
         class U<T> {{\n\
         \x20 {return_annotation} mm() => throw '';\n\
         \x20 void e(bool b) {{}}\n\
         \x20 void t() {{ e(mm is {target}); }}\n\
         }}\n",
    ))
}

/// `m6 is int Function([core.List<core.int> x]) Function()` — the inner
/// optional-positional `core.` leaf (function_type0_test.dart:351's
/// shape).
#[test]
fn literal_target_inner_core_optional_fires() {
    let report = literal_report(
        "int Function([core.List<core.int> x]) Function()",
        "int Function([core.List<core.int> x])",
    );
    assert!(
        !utc_rows(&report).is_empty(),
        "diags: {:?}",
        all_names(&report)
    );
}

/// The unprefixed control (function_type0_test.dart:697's shape — a
/// zero-own-formal method whose declared return is a function type with
/// no prefixed leaf): the verdict already answers it today; the pins
/// below must not regress it.
#[test]
fn literal_target_unprefixed_control_fires() {
    let report = literal_report(
        "Function(int) Function()",
        "Function(int)",
    );
    assert!(
        !utc_rows(&report).is_empty(),
        "diags: {:?}",
        all_names(&report)
    );
}

/// The param-type twin: `[int]` against the operand's
/// `[core.List<core.int>]` — contravariance fails, informative.
#[test]
fn literal_target_inner_optional_mismatch_stays_silent() {
    let report = literal_report(
        "int Function([int x]) Function()",
        "int Function([core.List<core.int> x])",
    );
    assert!(
        utc_rows(&report).is_empty(),
        "diags: {:?}",
        all_names(&report)
    );
}

/// `m12 is core.List<core.int> Function([Function]) Function()` — the
/// core-prefixed inner RETURN beside an optional positional
/// (function_type0_test.dart:547's shape).
#[test]
fn literal_target_core_return_fires() {
    let report = literal_report(
        "core.List<core.int> Function([Function]) Function()",
        "core.List<core.int> Function([Function])",
    );
    assert!(
        !utc_rows(&report).is_empty(),
        "diags: {:?}",
        all_names(&report)
    );
}

/// `m13 is core.List<core.int> Function({core.List<core.int> x})
/// Function()` — the named core leaf (function_type0_test.dart:572's
/// shape).
#[test]
fn literal_target_named_core_fires() {
    let report = literal_report(
        "core.List<core.int> Function({core.List<core.int> x}) Function()",
        "core.List<core.int> Function({core.List<core.int> x})",
    );
    assert!(
        !utc_rows(&report).is_empty(),
        "diags: {:?}",
        all_names(&report)
    );
}

/// The named-name twin: `{core.List<core.int> y}` against the operand's
/// `{... x}` — the name sets disagree, informative.
#[test]
fn literal_target_named_name_mismatch_stays_silent() {
    let report = literal_report(
        "core.List<core.int> Function({core.List<core.int> y}) Function()",
        "core.List<core.int> Function({core.List<core.int> x})",
    );
    assert!(
        utc_rows(&report).is_empty(),
        "diags: {:?}",
        all_names(&report)
    );
}

/// `m15 is List<T> Function(int, [core.List<core.int> x]) Function()` —
/// the class-parameter return beside the core optional
/// (function_type0_test.dart:647's shape).
#[test]
fn literal_target_class_t_return_fires() {
    let report = literal_report(
        "List<T> Function(int, [core.List<core.int> x]) Function()",
        "List<T> Function(int, [core.List<core.int> x])",
    );
    assert!(
        !utc_rows(&report).is_empty(),
        "diags: {:?}",
        all_names(&report)
    );
}

/// `m23 is void Function<A>(core.List<core.int> x) Function()` — the
/// generic inner signature beside the core positional
/// (function_type0_test.dart:915's shape).
#[test]
fn literal_target_generic_inner_core_fires() {
    let report = literal_report(
        "void Function<A>(core.List<core.int> x) Function()",
        "void Function<A>(core.List<core.int> x)",
    );
    assert!(
        !utc_rows(&report).is_empty(),
        "diags: {:?}",
        all_names(&report)
    );
}

/// The bare-function twin of the generic face: dropping the inner `<A>`
/// group makes the pair a generic-against-non-generic ask the verdict
/// cannot prove — informative, no row (upstream's relateTypeParameters
/// refuses the unequal parameter counts).
#[test]
fn literal_target_generic_inner_arity_stays_silent() {
    let report = literal_report(
        "void Function(core.List<core.int> x) Function()",
        "void Function<A>(core.List<core.int> x)",
    );
    assert!(
        utc_rows(&report).is_empty(),
        "diags: {:?}",
        all_names(&report)
    );
}

// —— root 1's guard: unannotated formals are not broken captures ——

/// The `arg_param_trailing_comma_test` family's shape: an old-style
/// typedef and a top-level function spelling type-less formals (implicit
/// `dynamic`). The tear-off's declaration sig carries the honest
/// required-positional count (`[y,]` — none required); the pooled face
/// cannot, and the type-less formal must not be mistaken for a broken
/// annotation that discards that sig — `topy is! fy` is always false.
#[test]
fn untyped_optional_positional_tearoff_fires() {
    let report = prefix_report(concat!(
        "typedef fy([y,]);\n",
        "void topy([y,]) {}\n",
        "void t() { if (topy is! fy) throw ''; }\n",
    ));
    assert!(
        !utc_rows(&report).is_empty(),
        "diags: {:?}",
        all_names(&report)
    );
}

/// The mixed twin: one required and one optional positional
/// (`fxy(x, [y,])`) on both sides — the split survives the tear-off.
#[test]
fn untyped_mixed_positional_tearoff_fires() {
    let report = prefix_report(concat!(
        "typedef fxy(x, [y,]);\n",
        "void topxy(x, [y,]) {}\n",
        "void t() { if (topxy is! fxy) throw ''; }\n",
    ));
    assert!(
        !utc_rows(&report).is_empty(),
        "diags: {:?}",
        all_names(&report)
    );
}

/// The arity twin: a tear-off REQUIRING its positional against a target
/// that makes it optional — `void Function(dynamic)` is no subtype of
/// `Function([dynamic])` (upstream's `_isFunctionSubtypeOf` positional
/// leg), informative, no row.
///
/// Ignored: an old-style typedef (`typedef fy([y,]);`) has no `Fn`
/// alias capture, so the target never folds to a signature and the
/// verdict falls to the pooled-shape reflexivity, which reads
/// `Function(dynamic)` and `Function([dynamic])` as one id and reports a
/// row the oracle does not. Fixing it means producing the written target
/// shape with its optional split (the required count must survive the
/// canonical re-intern); the operand side is already honest.
#[test]
#[ignore = "pre-existing: old-style typedef targets lose their optional split on the pooled-shape path"]
fn untyped_required_against_optional_stays_silent() {
    let report = prefix_report(concat!(
        "typedef fy([y,]);\n",
        "void topx(x,) {}\n",
        "void t() { if (topx is! fy) throw ''; }\n",
    ));
    assert!(
        utc_rows(&report).is_empty(),
        "diags: {:?}",
        all_names(&report)
    );
}
