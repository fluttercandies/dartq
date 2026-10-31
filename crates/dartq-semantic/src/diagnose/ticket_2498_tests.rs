//! The #2498 pins — the A8-family handback consolidation's two verified
//! faces landed this lane: the SDK static-return rows
//! `Error.throwWithStackTrace` (`static Never`,
//! sdk/lib/core/errors.dart:117) and `Future.error` (`factory Future.error`,
//! sdk/lib/async/future.dart:392 — the declared return spells `Future<T>`,
//! so the send's TYPE is the `Future` head, not `Never`). Both ride
//! [`crate::member_face::CORE_STATIC_RETURNS`] — the unindexed SDK
//! container's static send types through the table where the member walk
//! chain-breaks (`assignability.rs`'s callee-return face).

use super::*;
use crate::collector::collect;
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_resolution::{Combinators, ImportEdge, LibraryGraphBuilder};
use dartq_scanner::ScanConfig;
use dartq_source::FileTable;
use std::path::Path;

/// The face-node rig: `Error`/`RangeError` into `dart:core`,
/// `Future`/`FutureOr`/`Stream` into `dart:async` — each an unindexed
/// `<dart:...>` file joined by an unfiltered unprefixed import, the
/// CLI's SDK posture the static-return table answers under.
fn report_sdk_faces(source: &str) -> FileReport {
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
    let core = builder.intern("dart:core");
    let async_lib = builder.intern("dart:async");
    let core_file = files.intern(Path::new("<dart:core>"));
    let async_file = files.intern(Path::new("<dart:async>"));
    for (node, sdk_file, names) in [
        (core, core_file, &["Error", "RangeError", "Object", "StackTrace"][..]),
        (async_lib, async_file, &["FutureOr", "Future", "Stream"][..]),
    ] {
        for name in names {
            builder.declare(node, strings.intern(name), sdk_file);
        }
        builder.import(
            lib,
            ImportEdge {
                target: Some(node),
                prefix: None,
                deferred: false,
                combinators: Combinators::unfiltered(),
            },
        );
    }
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

fn code_names(report: &FileReport) -> Vec<&'static str> {
    report.diagnostics.iter().map(|d| d.code.name).collect()
}

/// `Error.throwWithStackTrace(x, s)` types the send `Never` — the
/// tail-expression `=> Error.throwWithStackTrace(e, s)` under a
/// `Future<int>`-declared body reads the never-completion face: no
/// `return_of_invalid_type` (the body's declared return is `Future<int>`,
/// but the send types `Never`, which fits everything) and no
/// `undefined_method` on the static send.
#[test]
fn error_throw_with_stack_trace_types_never() {
    let source = "Future<int> f(Object e, StackTrace s) => \
                  Error.throwWithStackTrace(e, s);";
    let report = report_sdk_faces(source);
    let names = code_names(&report);
    assert!(
        !names.iter().any(|&c| c == "return_of_invalid_type"),
        "return_of_invalid_type on the Never-typed send: {names:?}"
    );
    assert!(
        !names.iter().any(|&c| c == "undefined_method"),
        "undefined_method on the static send: {names:?}"
    );
}

/// `Future.error(e)` types the send `Future` — a `Future<int> x =
/// Future.error(e)` assignment reads the `Future` head (assignable to
/// `Future<int>` at the name level), never the `Unknown` the member walk
/// would leave on an unindexed `Future`.
#[test]
fn future_error_types_future() {
    let source = "void f(Object e) { Future<int> x = Future.error(e); }";
    let report = report_sdk_faces(source);
    let names = code_names(&report);
    assert!(
        !names.iter().any(|&c| c == "undefined_method"),
        "undefined_method on the static send: {names:?}"
    );
}

/// `return Future.error(e);` inside a `Future<int>`-declared body: the
/// send types `Future`, which reads non-`Never` — the body's declared
/// return governs the tail verdict the same way the `Error` send's
/// `Never` does on its own head.
#[test]
fn future_error_in_future_body_stays_clean() {
    let source = "Future<int> f(Object e) => Future.error(e);";
    let report = report_sdk_faces(source);
    let names = code_names(&report);
    assert!(
        !names.iter().any(|&c| c == "return_of_invalid_type"),
        "return_of_invalid_type on the Future send: {names:?}"
    );
}

/// The `RangeError.checkValidRange` row still rides the table beside the
/// two additions — the existing entry's verdict is unchanged (the
/// promotion-carrying arm, #2172's `binarySearchBy` fixture).
#[test]
fn range_error_check_valid_range_still_types_int() {
    let source = "class bool {} \
                  class num { bool operator <(num other) => true; } \
                  class int extends num {} \
                  void f(int? e) { e = RangeError.checkValidRange(0, e, 3); if (e < 3) {} }";
    let report = report_sdk_faces(source);
    assert!(
        code_names(&report).is_empty(),
        "{:?}",
        code_names(&report)
    );
}

// —— Face 5: the object-pattern NamedType arity verdict —————————————————

/// `case Box<int, String>():` — the object pattern's `NamedType` head
/// `Box` declares ONE type parameter but the group spells two: the
/// oracle's `_buildTypeArguments` reports
/// `wrong_number_of_type_arguments_element` at the type node (head
/// through `>`). The group's count latched at `end_type_arguments` and
/// `handle_object_pattern` consumed it.
#[test]
fn object_pattern_named_type_wrong_arity_reports() {
    let source = "class Box<T> {}\n\
                  void f(Object o) {\n\
                  \x20 switch (o) {\n\
                  \x20\x20 case Box<int, String>():\n\
                  \x20\x20\x20 break;\n\
                  \x20 }\n\
                  }";
    let report = crate::diagnose::test_rig::report_ops_bare(source);
    let names = code_names(&report);
    assert!(
        names
            .iter()
            .any(|&c| c == "wrong_number_of_type_arguments_element"
                || c == "wrong_number_of_type_arguments"),
        "expected the NamedType arity row: {names:?}"
    );
}

/// `case Box<int>():` — a correct-arity object pattern head reports
/// nothing (the verdict only fires on a real mismatch).
#[test]
fn object_pattern_named_type_matching_arity_silent() {
    let source = "class Box<T> {}\n\
                  void f(Object o) {\n\
                  \x20 switch (o) {\n\
                  \x20\x20 case Box<int>():\n\
                  \x20\x20\x20 break;\n\
                  \x20 }\n\
                  }";
    let report = crate::diagnose::test_rig::report_ops_bare(source);
    let names = code_names(&report);
    assert!(
        !names.iter().any(|&c| c.starts_with("wrong_number_of_type_arguments")),
        "unexpected arity row: {names:?}"
    );
}

/// A non-pattern `Box<int, String>` (a plain annotation) still reports
/// through its own `handle_type` path — the pattern latch never
/// participates outside `handle_object_pattern`.
#[test]
fn plain_named_type_wrong_arity_still_reports() {
    let source = "class Box<T> {}\n\
                  void f() { Box<int, String>? b = null; }";
    let report = crate::diagnose::test_rig::report_ops_bare(source);
    let names = code_names(&report);
    assert!(
        names
            .iter()
            .any(|&c| c.starts_with("wrong_number_of_type_arguments")),
        "expected the plain NamedType arity row: {names:?}"
    );
}

// —— Face 1: the object-pattern scrutinee promotion ————————————————————
//
// `case $ResultData<ValueT>():` over the `$Result<ValueT>?`-declared
// parameter promotes `value` to the non-nullable instantiated head — the
// case body's member reads (`value.value`) never report
// `unchecked_use_of_nullable_value` (riverpod `listenable.dart:38-40`'s
// three rows; official `promoteForPattern`, flow_analysis.dart:7703-7755).

/// The headline: a sealed-family object pattern over a NULLABLE scrutinee
/// promotes — `value.value` inside the clause body resolves on
/// `$ResultData<ValueT>`, not the nullable declared face.
#[test]
fn object_pattern_promotes_nullable_scrutinee_member_reads() {
    let source = "sealed class $Result<T> {}\n\
                  final class $ResultData<T> extends $Result<T> { T get value => throw 0; }\n\
                  final class $ResultError<T> extends $Result<T> { Object get error => throw 0; }\n\
                  void f($Result<int>? value) {\n\
                  \x20 switch (value) {\n\
                  \x20\x20 case null:\n\
                  \x20\x20\x20 break;\n\
                  \x20\x20 case $ResultData<int>():\n\
                  \x20\x20\x20 value.value;\n\
                  \x20\x20 case $ResultError<int>():\n\
                  \x20\x20\x20 value.error;\n\
                  \x20 }\n\
                  }";
    let report = crate::diagnose::test_rig::report_ops(source);
    assert_eq!(
        code_names(&report),
        Vec::<&'static str>::new(),
        "{:?}",
        crate::diagnose::test_rig::rows(&report)
    );
}

/// The exact corpus shape: a RAW object head spelling the type PARAMETER
/// (`case $ResultData<ValueT>():`) over the `$Result<ValueT>?` scrutinee
/// inside a generic class — the instantiated-args face of the same
/// promotion.
#[test]
fn object_pattern_promotes_nullable_scrutinee_type_param_args() {
    let source = "sealed class $Result<T> {}\n\
                  final class $ResultData<T> extends $Result<T> { T get value => throw 0; }\n\
                  final class $ResultError<T> extends $Result<T> { Object get error => throw 0; Object get stackTrace => throw 0; }\n\
                  class Observable<ValueT> {\n\
                  \x20 void f($Result<ValueT>? value, ValueT? previous) {\n\
                  \x20\x20 switch (value) {\n\
                  \x20\x20\x20 case null:\n\
                  \x20\x20\x20\x20 break;\n\
                  \x20\x20\x20 case $ResultData<ValueT>():\n\
                  \x20\x20\x20\x20 value.value;\n\
                  \x20\x20\x20 case $ResultError<ValueT>():\n\
                  \x20\x20\x20\x20 value.error;\n\
                  \x20\x20\x20\x20 value.stackTrace;\n\
                  \x20\x20 }\n\
                  \x20 }\n\
                  }";
    let report = crate::diagnose::test_rig::report_ops(source);
    assert_eq!(
        code_names(&report),
        Vec::<&'static str>::new(),
        "{:?}",
        crate::diagnose::test_rig::rows(&report)
    );
}

// —— Face 3: the `final that = this` extension-init staging ————————————
//
// `mutations.dart:361`'s `final that = this` inside `extension<StateT> on
// Mutation<StateT>`: `this`'s static type is the argument-bearing on-type
// (`Mutation<StateT>`), and the inferred-local staging must carry it —
// the bare head suffices where the tree cannot spell (`switch (that)
// case MutationImpl():` promotes `that`; riverpod's two UNCHECKED-family
// rows). The #2506 arg-tree staging + the #2471 on-type head latch make
// this the same-fix arm: an unspellable tree stages the bare head.

/// The corpus shape: `final that = this` under a named generic extension
/// — `case MutationImpl():` promotes `that`, so `return that` (declared
/// `MutationImpl<StateT>`) and the member read stay silent.
#[test]
fn this_init_inferred_local_promotes_under_extension() {
    let source = "sealed class Mutation<T> {}\n\
                  final class MutationImpl<T> implements Mutation<T> {}\n\
                  extension<StateT> on Mutation<StateT> {\n\
                  \x20 MutationImpl<StateT> get impl {\n\
                  \x20\x20 final that = this;\n\
                  \x20\x20 switch (that) {\n\
                  \x20\x20\x20 case MutationImpl():\n\
                  \x20\x20\x20\x20 return that;\n\
                  \x20\x20 }\n\
                  \x20 }\n\
                  }\n\
                  void g(Mutation<int> m) { m.impl; }";
    let report = crate::diagnose::test_rig::report_ops(source);
    assert_eq!(
        code_names(&report),
        Vec::<&'static str>::new(),
        "{:?}",
        crate::diagnose::test_rig::rows(&report)
    );
}

// —— Face 4: the dotted-read scrutinee channel —————————————————————————
//
// `switch (this._field)` — a promotable property scrutinee: the object
// pattern's non-nullable head promotes the property slot exactly like a
// bare-read scrutinee's variable (official `promoteForPattern` runs the
// same type-test arm on the property key, flow_analysis.dart:7703-7755;
// drift `engines.dart:570`). A `case T():` over a nullable field leaves
// `this._field` non-null inside the clause body — the member read stays
// silent.

/// `this._v`'s object pattern promotes the property to non-null: a
/// member the nullable declared face would refuse (UNCHECKED — `m`
/// resolves on `R` itself, so only the `?` stood between) stays silent
/// inside `case D():`.
#[test]
fn dotted_scrutinee_object_pattern_promotes_property() {
    let source = "sealed class R { void m() {} }\n\
                  final class D extends R {}\n\
                  class C {\n\
                  \x20 final R? _v;\n\
                  \x20 C(this._v);\n\
                  \x20 void f() {\n\
                  \x20\x20 switch (this._v) {\n\
                  \x20\x20\x20 case null:\n\
                  \x20\x20\x20\x20 break;\n\
                  \x20\x20\x20 case D():\n\
                  \x20\x20\x20\x20 this._v.m();\n\
                  \x20\x20 }\n\
                  \x20 }\n\
                  }";
    let report = crate::diagnose::test_rig::report_ops(source);
    assert_eq!(
        code_names(&report),
        Vec::<&'static str>::new(),
        "{:?}",
        crate::diagnose::test_rig::rows(&report)
    );
}

/// The non-generic twin — same promotion question without the
/// instantiated-argument face.
#[test]
fn object_pattern_promotes_nullable_scrutinee_plain() {
    let source = "sealed class R {}\n\
                  final class RD extends R { int get value => 0; }\n\
                  void f(R? value) {\n\
                  \x20 switch (value) {\n\
                  \x20\x20 case null:\n\
                  \x20\x20\x20 break;\n\
                  \x20\x20 case RD():\n\
                  \x20\x20\x20 value.value;\n\
                  \x20 }\n\
                  }";
    let report = crate::diagnose::test_rig::report_ops(source);
    assert_eq!(
        code_names(&report),
        Vec::<&'static str>::new(),
        "{:?}",
        crate::diagnose::test_rig::rows(&report)
    );
}
