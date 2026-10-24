//! The #2567 pins: assignability matrix sweep — fill in as roots close. The rig
//! declares no imports, so every pin spells custom types only.

use super::*;
use crate::collector::collect;
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_resolution::LibraryGraphBuilder;
use dartq_scanner::ScanConfig;
use dartq_source::FileTable;
use std::path::Path;

/// Diagnoses `source` as the sole file of a library with no imports —
/// the single-file rig (`ticket_2564_tests`' shape).
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


/// The two-file rig: `main_src` imports `other_src` under the prefix
/// `p` — the prefixed-callee seam (`p.f(x)` resolving through the
/// import table into the other unit's element model).
fn report_prefixed(other_src: &str, main_src: &str) -> FileReport {
    let strings = StringTable::new();
    let mut files = FileTable::new();
    let file = files.intern(Path::new("main.dart"));
    let other_file = files.intern(Path::new("other.dart"));
    let scan = |src: &str| {
        let scanned = dartq_scanner::scan(
            src,
            ScanConfig {
                enable_triple_shift: true,
                enable_augmentations: true,
            },
            file,
        );
        assert!(scanned.diagnostics.is_empty(), "scanner diagnostics: {src:?}");
        scanned
    };
    let set = FeatureSet::for_language_version(LanguageVersion::new(3, 15));
    let features = ParserFeatures::from_feature_set(set);
    let scanned = scan(main_src);
    let other_scanned = scan(other_src);
    let elements = collect(main_src, &scanned.tokens, features, &strings);
    let other_elements = collect(other_src, &other_scanned.tokens, features, &strings);
    let mut builder = LibraryGraphBuilder::new(&strings);
    let lib = builder.intern("package:t/main.dart");
    let other_lib = builder.intern("package:t/other.dart");
    let mut index = ElementIndex::default();
    index.insert(file, elements.clone());
    index.insert(other_file, other_elements.clone());
    for (name, decl) in elements.declarations() {
        if decl.kind == MicroKind::TypeParam {
            continue;
        }
        builder.declare(lib, name, file);
    }
    for (name, decl) in other_elements.declarations() {
        if decl.kind == MicroKind::TypeParam {
            continue;
        }
        builder.declare(other_lib, name, other_file);
    }
    builder.import(
        lib,
        dartq_resolution::library_graph::ImportEdge {
            target: Some(other_lib),
            prefix: Some(strings.intern("p")),
            deferred: false,
            combinators: dartq_resolution::library_graph::Combinators::unfiltered(),
        },
    );
    let graph = builder.finish();
    let namespaces = Namespaces::build(&graph);
    let fold = fold_unit(
        main_src,
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

/// The corpus `??=` shape (`filename ??= path.basename(file.path)` —
/// archive's zip encoder): a prefixed callee's concrete return types
/// the assignment compatible — no invalid-assignment face, and the
/// prefix itself never leaks as the RHS type face.
#[test]
fn prefixed_callee_return_feeds_the_assign_rebind() {
    let report = report_prefixed(
        "class Payload { }\nPayload pick(Payload n) => n;\n",
        "import 'other.dart' as p;\n\
         class Holder {\n\
           p.Payload? field;\n\
           void f(p.Payload n) {\n\
             field ??= p.pick(n);\n\
             field.missing;\n\
           }\n\
         }\n",
    );
    assert!(
        report.diagnostics.is_empty(),
        "prefixed-callee ??= mis-reports: {:?}",
        messages(&report)
    );
}

/// A generic prefixed callee's free inference parameter never escapes
/// as the caller-scope type: `field ??= p.pick(a, b)` answers the
/// unknown face (the under-report direction until call-site inference
/// solves it), so no downstream sink reports against the raw `T`
/// spelling (the analyzer `math.min` family).
#[test]
fn prefixed_generic_callee_frees_to_unknown() {
    let report = report_prefixed(
        "class Payload { }\nT pick<T extends num>(T a, T b) => a;\n",
        "import 'other.dart' as p;\n\
         class Holder {\n\
           p.Payload? field;\n\
           void f(p.Payload a, p.Payload b) {\n\
             field ??= p.pick(a, b);\n\
             field.missing;\n\
           }\n\
         }\n",
    );
    assert!(
        !messages(&report).iter().any(|m| m.contains("'T'")),
        "raw schema spelling visible: {:?}",
        messages(&report)
    );
}

/// A foreign generic callee's parameter spelling is recovered even when
/// an unrelated declaration in THIS file declares a type parameter of
/// the same name: `pick`'s `T` is not `acc<T>`'s `T` — the recovery
/// proves leaves from the call site's scope (the frame latch), never the
/// file's global declaration table, so the free parameter still
/// collapses to the unknown face (analyzer's `single_file_byte_store`
/// `min` family: the file's own `withExclusiveAccess<T>` methods
/// emptied the recovery and the raw `T` leaked into every sink).
#[test]
fn foreign_param_recovery_ignores_same_file_type_param_decls() {
    let report = report_prefixed(
        "class Payload { }\nT pick<T extends num>(T a, T b) => a;\n",
        "import 'other.dart' as p;\n\
         class Unrelated {\n\
           T acc<T>(T Function() f) => f();\n\
         }\n\
         class Pool {\n\
           void read() {\n\
             var n = p.pick(1, 2);\n\
             int o = 3 + n;\n\
             print(o);\n\
           }\n\
         }\n",
    );
    assert!(
        !messages(&report).iter().any(|m| m.contains("'T'")),
        "raw schema spelling visible: {:?}",
        messages(&report)
    );
}

/// A `??=` whose right side is a numeric binary (`length ??= 5 -
/// offset`) promotes the target: the fold's left operand is a literal
/// row typed from its own token bytes, the binary's result reaches the
/// write staging, and the join writes the non-null face (the corpus
/// `min((end - first) * pageSize, ...)` / `length ??= 5 - offset`
/// family). The rig seeds the implicit dart:core names (the bare rig
/// would report `int` itself undefined).
#[test]
fn if_null_rhs_binary_promotes_target() {
    let report = report(
        "class Sink { void take(int x) {} }\n\
         void f(int? offset, int? length) {\n\
           offset ??= 0;\n\
           length ??= 5 - offset;\n\
           Sink().take(length);\n\
           Sink().take(offset + length);\n\
         }\n",
    );
    let report = FileReport {
        diagnostics: report
            .diagnostics
            .into_iter()
            .filter(|d| d.code.name != "undefined_class")
            .collect(),
        ..report
    };
    assert!(
        report.diagnostics.is_empty(),
        "??= binary-RHS promotion missing: {:?}",
        messages(&report)
    );
}

/// A `??=` whose right side is a generic call the site solves
/// (`x ??= pick(a, b)` over `T pick<T>(T a, T b)`) promotes the target:
/// the send-close upward inference binds `T := W` from the typed
/// arguments, the instantiated return reaches the write staging, and
/// the join writes the non-null face (the corpus `math.max(0, n - 1)`
/// family — same-file spelling here; the prefixed face rides the same
/// solver through the import table). `W y = x` observably needs the
/// promotion — an unpromoted `W?` writes `invalid_assignment`.
#[test]
fn if_null_rhs_generic_call_promotes_target() {
    let report = report(
        "class W {}\n\
         T pick<T>(T a, T b) => a;\n\
         W sink(W v) => v;\n\
         void f(W? x, W a, W b) {\n\
           x ??= pick(a, b);\n\
           W y = x;\n\
           sink(y);\n\
         }\n",
    );
    assert!(
        report.diagnostics.is_empty(),
        "??= generic-call-RHS promotion missing: {:?}",
        messages(&report)
    );
}

/// A `??=` whose right side reads an unannotated numeric `static const`
/// (`bufferSize ??= OutputFileStream.kDefaultBufferSize`) promotes the
/// target: the collector's declaration-site inference types the const's
/// member table entry from its arithmetic-literal initializer, the read
/// stages `int`, and the join writes the non-null face (the corpus
/// extract_archive_to_disk.dart 136 family).
#[test]
fn if_null_rhs_numeric_const_promotes_target() {
    let report = report(
        "class C {\n\
           static const kDefault = 1024 * 1024;\n\
         }\n\
         void take(int v) {}\n\
         void f(int? x) {\n\
           x ??= C.kDefault;\n\
           take(x);\n\
         }\n",
    );
    let report = FileReport {
        diagnostics: report
            .diagnostics
            .into_iter()
            .filter(|d| d.code.name != "undefined_class")
            .collect(),
        ..report
    };
    assert!(
        report.diagnostics.is_empty(),
        "??= numeric-const-RHS promotion missing: {:?}",
        messages(&report)
    );
}

/// `e1.clamp(e2, e3)` on an `int` receiver with `int` arguments answers
/// `int` — the numeric-call refinement
/// (`_refineNumericInvocationTypeNullSafe`'s `clamp` sibling,
/// type_system.dart:2122-2160) — so the write to the `int` field checks
/// clean (the corpus input_memory_stream.dart 92/99 family), while a
/// non-`num` receiver's own `remainder` keeps its declared return (the
/// BigInt regression guard).
#[test]
fn clamp_refines_to_int_on_int_operands() {
    let report = report(
        "class C {\n\
           int _position = 0;\n\
           int _length = 10;\n\
           void rewind(int length) {\n\
             _position -= length;\n\
             _position = _position.clamp(0, _length);\n\
           }\n\
         }\n",
    );
    let report = FileReport {
        diagnostics: report
            .diagnostics
            .into_iter()
            .filter(|d| d.code.name != "undefined_class")
            .collect(),
        ..report
    };
    assert!(
        report.diagnostics.is_empty(),
        "clamp numeric-call refinement missing: {:?}",
        messages(&report)
    );
}

/// A supertype clause's `Never` argument carries into the edge —
/// `class Failure extends Result<Never>` keeps its supertype, so
/// `limiter is Failure` promotes and `return limiter` satisfies
/// `Result<List<int>>` (Never is bottom; the unregistered-name miss
/// dropped the whole edge and both the promotion and the return went
/// red — the greedy.dart 94/97 family, oracle-silent).
#[test]
fn never_clause_argument_keeps_the_supertype_edge() {
    let report = super::test_rig::report_ops(
        "class Parser<R> { Result<R> parseOn(Object c) => throw 0; }\n\
         class Result<R> {}\n\
         class Failure extends Result<Never> {}\n\
         class Picky extends Parser<int> {\n\
           Result<List<int>> parseOn(Object c) {\n\
             Parser<void> limit = throw 0;\n\
             final limiter = limit.parseOn(c);\n\
             if (limiter is Failure) {\n\
               return limiter;\n\
             }\n\
             throw 0;\n\
           }\n\
         }\n",
    );
    assert!(
        count_code(&report, "return_of_invalid_type") == 0,
        "Never clause edge dropped: {:?}",
        messages(&report)
    );
}

/// A raw object pattern's downward inference walks the superclass CHAIN,
/// not the direct edges: `final class AD<T> extends AR<T>` under
/// `sealed class AR<T> extends AV<T>` narrows an `AV<T>` scrutinee to
/// `AD<T>` in the case body (official `downwardInferObjectPattern
/// RequiredType`'s step-by-step instantiation). The direct-only scan
/// refused every grandchild and the case body's argument kept the
/// declared `AV<T>` (riverpod async_value.dart map/when family — 6
/// oracle-silent rows).
#[test]
fn raw_pattern_infers_through_intermediate_sealed_layer() {
    let report = super::test_rig::report_ops(
        "sealed class AsyncValue<T> {}\n\
         sealed class AsyncResult<T> extends AsyncValue<T> {}\n\
         final class AsyncData<T> extends AsyncResult<T> {}\n\
         final class AsyncError<T> extends AsyncResult<T> {}\n\
         final class AsyncLoading<T> extends AsyncValue<T> {}\n\
         String pick<T>(\n\
           AsyncValue<T> v, {\n\
           required String Function(AsyncData<T> data) onData,\n\
           required String Function(AsyncError<T> error) onError,\n\
           required String Function(AsyncLoading<T> loading) onLoading,\n\
         }) {\n\
           switch (v) {\n\
             case AsyncData():\n\
               return onData(v);\n\
             case AsyncError():\n\
               return onError(v);\n\
             case AsyncLoading():\n\
               return onLoading(v);\n\
           }\n\
         }\n",
    );
    assert!(
        count_code(&report, "argument_type_not_assignable") == 0,
        "grandchild pattern inference dropped: {:?}",
        messages(&report)
    );
}

/// A record- or function-typed switch scrutinee suppresses the
/// exhaustiveness check whole — the name-level world decomposes no shape
/// columns, so the matched value joins as a shape-keyed unknown sentinel
/// the official treats as matched-by-nothing (shared.dart:310-320); the
/// binding-pattern row set can neither prove nor refute. Without the
/// suppression every such switch reported a spurious covering gap
/// (drift's `switch (where) { null => .., final where => .. }` over a
/// nullable function-typed field).
#[test]
fn function_typed_scrutinee_suppresses_exhaustiveness() {
    let report = super::test_rig::report_ops(
        "class W {}\n\
         class C {\n\
           W? Function(int, int)? where;\n\
           Object? f() {\n\
             return switch (where) {\n\
               null => null,\n\
               final w => w(1, 2),\n\
             };\n\
           }\n\
         }\n",
    );
    assert!(
        count_code(&report, "non_exhaustive_switch_expression") == 0,
        "function scrutinee reported: {:?}",
        messages(&report)
    );
}

/// A Never-returning invocation does not anchor its own dead-code row:
/// the member-access node poll runs post-order (after the invocation's
/// own `Ev::Exit`), and the official's pre-order `checkUnreachableNode`
/// saw the access live — the statement's own death stays silent (the
/// next node anchors; a call ending its block reports nothing). Without
/// the `exit_at` boundary the poll self-anchored at the call's closing
/// `);` (the throwWithStackTrace/Isolate.exit family — 10 oracle-silent
/// rows across drift/dio/http/riverpod).
#[test]
fn never_call_does_not_anchor_the_dead_row_on_itself() {
    let report = super::test_rig::report_ops_bare(
        "class E { static Never exit() => throw StateError('x'); }\n\
         void g() {\n\
           E.exit();\n\
         }\n\
         ",
    );
    assert!(
        count_code(&report, "dead_code") == 0,
        "a Never member call ending its block reports nothing: {:?}",
        messages(&report)
    );
}

/// A `super.member` accessor read types against the SUPER declaration,
/// never the enclosing override — the override's own covariant face made
/// a legal `super.member!` read unnecessary (the element.dart
/// `LibraryElementImpl get library => super.library!;` family).
#[test]
fn super_accessor_read_types_against_the_super_declaration() {
    let report = super::test_rig::report_ops_bare(
        "class Child {}\n\
         class Base {\n\
           Child? get child => null;\n\
         }\n\
         class Sub extends Base {\n\
           @override\n\
           Child get child => super.child!;\n\
         }\n",
    );
    assert!(
        count_code(&report, "unnecessary_non_null_assertion") == 0,
        "super accessor read typed on the override: {:?}",
        messages(&report)
    );
}

/// SDK stub member faces answer through the frozen rows: dart:isolate's
/// names-only `ReceivePort` (an empty member map, an unindexed
/// `implements Stream` clause) resolves `sendPort` on its own row and
/// `first` through the SDK super-face alias — and an unknown member still
/// reports (the frozen consult's Known/Absent split). The pre-fix walk
/// answered a clean `Missing` on the resolved stub and reported every
/// legal member (the file_byte_store/drift `response.sendPort` /
/// The SDK-stub member seam (dart:isolate's names-only `ReceivePort`):
/// the stub unit registers as its `dart:` node's unit with a bare
/// declaration and no element model — production's face-file shape
/// (`<dart:isolate>`, `pipeline`'s names-only SDK arm) — so a member
/// access walks to `Missing` and the frozen rows decide. The stub's own
/// face (`sendPort`) and its `implements Stream` super-face (`first`,
/// the `SDK_SUPER_FACES` alias) answer defined; a name no face carries
/// keeps the miss's own row with the stub's spelling in the message.
/// The package-side twin (a names-only NON-dart: unit) stays the
/// under-report consult — pinned by `unindexed_interface_receiver_
/// reports_absent_members` (the dep-closure shape) — and the unit-less
/// local twin (the bare rig's own files) never consults — pinned by
/// the #2497 `same_head_parameter_bound_decides` local `Iterable`.
#[test]
fn sdk_stub_member_face_answers_through_frozen_rows() {
    let source = "import 'stub';\n\
                  void f(ReceivePort p) {\n\
                    p.sendPort.send(1);\n\
                    p.first;\n\
                    p.nope;\n\
                  }\n";
    let strings = StringTable::new();
    let mut files = FileTable::new();
    let file = files.intern(Path::new("main.dart"));
    let stub_file = files.intern(Path::new("<dart:isolate>"));
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
    let stub_source = "abstract class ReceivePort {}\n";
    let scanned_stub = dartq_scanner::scan(
        stub_source,
        ScanConfig {
            enable_triple_shift: true,
            enable_augmentations: true,
        },
        stub_file,
    );
    let set = FeatureSet::for_language_version(LanguageVersion::new(3, 15));
    let features = ParserFeatures::from_feature_set(set);
    let elements = collect(source, &scanned.tokens, features, &strings);
    // Production's stub units carry an empty element model beside the
    // bare declaration: resolve_container succeeds, the member walk
    // misses cleanly, and the frozen consult owns the verdict.
    let stub_elements = collect(stub_source, &scanned_stub.tokens, features, &strings);
    let mut builder = LibraryGraphBuilder::new(&strings);
    let lib = builder.intern("package:t/main.dart");
    let stub_lib = builder.intern("dart:isolate");
    for (name, decl) in elements.declarations() {
        if decl.kind == MicroKind::TypeParam {
            continue;
        }
        builder.declare(lib, name, file);
    }
    // The names-only stub: a unit of its node, `ReceivePort` declared on
    // it, no index entry — resolve_container finds the spelling but no
    // member model.
    builder.unit(stub_lib, stub_file);
    builder.declare(stub_lib, strings.intern("ReceivePort"), stub_file);
    builder.import(
        lib,
        ImportEdge {
            target: Some(stub_lib),
            prefix: None,
            deferred: false,
            combinators: dartq_resolution::Combinators::unfiltered(),
        },
    );
    let graph = builder.finish();
    let namespaces = Namespaces::build(&graph);
    let mut index = ElementIndex::default();
    index.insert(file, elements);
    index.insert(stub_file, stub_elements);
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
    let getters: Vec<&str> = report
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "undefined_getter")
        .map(|d| d.message.as_str())
        .collect();
    assert_eq!(
        getters,
        ["The getter 'nope' isn't defined for the type 'ReceivePort'."],
        "{:?}",
        messages(&report)
    );
}

/// A `case T x:` binder shadows only inside its own member: the
/// scrutinee spelling reads the OUTER declaration again in later
/// members — the arm's binder must not retype it (the
/// `bundle_writer.dart` `case SubstitutedElementImpl element:` family,
/// where later members' `localId`/`reference` reads resolved on the
/// binder's type and false-reported). The switch-expression arm twin
/// scopes the same way.
#[test]
fn case_binder_scopes_to_its_member() {
    let stmt = report(
        "class Element {} class Sub extends Element {}\n\
         class Pre extends Element { int get localId => 1; }\n\
         void f(Element element) {\n\
         \x20 switch (element) {\n\
         \x20   case Sub element:\n\
         \x20     break;\n\
         \x20   case Pre():\n\
         \x20     var x = element.localId;\n\
         \x20     break;\n\
         \x20   default:\n\
         \x20     break;\n\
         \x20 }\n\
         \x20 var after = element;\n\
         }",
    );
    assert_eq!(count_code(&stmt, "undefined_getter"), 0, "{:?}", messages(&stmt));
    let expr = report(
        "class Element {} class Sub extends Element {}\n\
         class Pre extends Element { int get localId => 1; }\n\
         int f(Element element) => switch (element) {\n\
         \x20 Sub element => 0,\n\
         \x20 Pre() => element.localId,\n\
         \x20 _ => 0,\n\
         };",
    );
    assert_eq!(count_code(&expr, "undefined_getter"), 0, "{:?}", messages(&expr));
}

/// `var x = p;` where `p`'s flow state carries a promotion infers the
/// promoted type (official inference reads the initializer's static
/// type): the parse-time bind saw the declared face, and the pass-2
/// read type re-binds it under the narrowing gate — the
/// `evaluation.dart` `var declaration = constant;` family, where the
/// `constantInitializer2` read false-reported on the declared type.
#[test]
fn bare_initializer_infers_the_promoted_type() {
    let source = [
        "class Element {} class VarImpl extends Element { int get init2 => 1; }",
        "void f(Element constant) {",
        "  if (constant is VarImpl) {",
        "    var declaration = constant;",
        "    var x = declaration.init2;",
        "  }",
        "}",
    ]
    .join("\n");
    let r = report(&source);
    assert_eq!(count_code(&r, "undefined_getter"), 0, "{:?}", messages(&r));
}

#[test]
fn tmp_cast_assign() {
    let plain = [
        "class Element {}",
        "class Internal extends Element { String get sig => ''; bool get st => true; }",
        "void f(Element x) {",
        "  x = x as Internal;",
        "  var a = x.sig;",
        "  var b = x.st;",
        "}",
    ].join("\n");
    let r = report(&plain);
    assert_eq!(count_code(&r, "undefined_getter"), 0, "{:?}", messages(&r));
    let in_if_case = [
        "class Element {}",
        "class Top extends Element {}",
        "class Meth extends Element {}",
        "class Internal extends Element { String get sig => ''; bool get st => true; }",
        "void f(Element x) {",
        "  if (x case Top() || Meth()) {",
        "    x = x as Internal;",
        "    var a = x.sig;",
        "    var b = x.st;",
        "  }",
        "}",
    ].join("\n");
    let r2 = report(&in_if_case);
    assert_eq!(count_code(&r2, "undefined_getter"), 0, "{:?}", messages(&r2));
}

/// A top-level logical-or alternation joins its branches' promotions
/// (official `analyzeLogicalOrPattern`): two branches promoting the
/// scrutinee to distinct faces are a union the type model cannot spell
/// — no promotion stands, and a member read names the declared type;
/// identical branch promotions keep the shared face. The `ffi_verifier`
/// family (`if (x case TopLevelFunctionElement() || MethodElement())`)
/// over-promoted to the last operand and silenced member reads the
/// oracle reports.
#[test]
fn or_alternation_joins_branch_promotions() {
    // Both branches declare `sig` — any single-branch promotion would
    // resolve it silently; only the restore to the declared face reports.
    let differing = [
        "class Element {}",
        "class Top extends Element { String get sig => ''; }",
        "class Meth extends Element { String get sig => ''; }",
        "void f(Element x) {",
        "  if (x case Top() || Meth()) {",
        "    var a = x.sig;",
        "  }",
        "}",
    ].join("\n");
    let r = report(&differing);
    assert_eq!(
        count_code(&r, "undefined_getter"),
        1,
        "differing OR promotions must not promote: {:?}",
        messages(&r)
    );
    let shared = [
        "class Element {}",
        "class Top extends Element { String get sig => ''; }",
        "void f(Element x) {",
        "  if (x case Top() || Top()) {",
        "    var a = x.sig;",
        "  }",
        "}",
    ].join("\n");
    let r2 = report(&shared);
    assert_eq!(
        count_code(&r2, "undefined_getter"),
        0,
        "identical OR promotions keep the promotion: {:?}",
        messages(&r2)
    );
}

/// An object-pattern shorthand binder takes the property's declared
/// return (`Head(:var x)` binds the getter's face, official
/// `resolveObjectPatternPropertyGet`): a member the face lacks reports
/// NAMING the face — the untyped binder suppressed (the unknown receiver
/// never reports) — and the chained property read resolves through it,
/// so the `utilities.dart` `GetterInvocationResolution(:var element)`
/// family's downstream guard promotions decide on the real face.
#[test]
fn object_pattern_binder_takes_property_type() {
    let src = [
        "class Node { Node2 get resolution => throw 0; }",
        "class Node2 { VarEl get element => throw 0; }",
        "class VarEl { bool get isConst => false; }",
        "void f(Node node) {",
        "  if (node.resolution case Node2(:var element)) {",
        "    var q = element.nope;",
        "    var variable = element.isConst;",
        "  }",
        "}",
    ].join("\n");
    let r = report(&src);
    let getters: Vec<&str> = r
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "undefined_getter")
        .map(|d| d.message.as_str())
        .collect();
    assert_eq!(
        getters,
        ["The getter 'nope' isn't defined for the type 'VarEl'."],
        "{:?}",
        messages(&r)
    );
}
