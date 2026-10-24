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

/// The inference-site guard is NARROW: only a `FutureOr<R>` return over
/// a name the diagnosing file cannot spell (the callee's own inference
/// parameter, `then<R>`'s callback formal — the corpus `xz_decoder.dart`
/// `onDone` row) skips the return-covariance check. A callback whose
/// mismatched return is fully decidable in scope still reports — the
/// guard must never silence a checkable disagreement.
#[test]
fn callback_return_mismatch_still_reports() {
    let src = [
        "class A {}",
        "class B {}",
        "class C {",
        "  void m(A Function(A) g) {}",
        "}",
        "void f(C c, B Function(B) h) {",
        "  c.m(h);",
        "}",
    ].join("\n");
    let r = report(&src);
    let args: Vec<&str> = r
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "argument_type_not_assignable")
        .map(|d| d.message.as_str())
        .collect();
    assert_eq!(
        args,
        ["The argument type 'B Function(B)' can't be assigned to the parameter type 'A Function(A)'. "],
        "{:?}",
        messages(&r)
    );
}

/// A foreign container's member send still instantiates with the
/// receiver's written arguments: the cross-file `Box<T>`'s parameters
/// answer through its declaring library's unit (the receiver-seed face
/// [`Engine::foreign_container_params`] feeds when this-file bounds hold
/// no capture — the SDK seam the corpus rides), and the member's return
/// face carries them (`b.pick()` is `Box<A>`, not the raw `Box<T>` — the
/// corpus `selectable.dart:176` `watch` result row: `Stream<T>.transform`'s
/// return must carry the receiver's element type).
#[test]
fn foreign_container_return_instantiated_from_receiver() {
    let r = super::test_rig::report_ops_imports(
        "void f(Box<A> b) {\n  b.take(b.pick());\n}\n",
        &[super::test_rig::RigImport {
            uri: "package:t/other.dart",
            prefix: None,
            source: "class A {}\nclass B {}\nclass Box<T> { Box<T> pick() => throw 0; void take(Box<B> x) {} }\n",
        }],
    );
    let args: Vec<&str> = r
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "argument_type_not_assignable")
        .map(|d| d.message.as_str())
        .collect();
    assert_eq!(
        args,
        ["The argument type 'Box<A>' can't be assigned to the parameter type 'Box<B>'. "],
        "{:?}",
        messages(&r)
    );
}

/// A class clause reaching an unmodeled `dart:collection`/`dart:convert`
/// base unfolds through the SDK alias face (`IterableBase` IS `Iterable`,
/// `SetMixin` IS `SetBase implements Set`, `SplayTreeSet` applies
/// `SetMixin` — upstream sdk/lib/collection + convert typedefs): the
/// creation returned as the core interface stays silent (the corpus rows
/// xml ancestors.dart:8, petitparser matches.dart:27, collection
/// priority_queue.dart:314), while a base the SDK never declares (`Nope`)
/// stays the dead InvalidType edge that reports.
#[test]
fn sdk_collection_alias_face_reaches_core_interface() {
    let r = super::test_rig::report_ops(concat!(
        "class Warn {}\n",
        "class Anc extends IterableBase<Warn> {}\n",
        "Iterable<Warn> ancestors() => Anc();\n",
        "class Mine extends SetMixin<Warn> {}\n",
        "Set<Warn> make() => Mine();\n",
        "Set<Warn> splay() { var s = SplayTreeSet<Warn>(); return s; }\n",
        "class Dead extends Nope {}\n",
        "Iterable<Warn> dead() => Dead();\n",
    ));
    let returns: Vec<&str> = r
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "return_of_invalid_type")
        .map(|d| d.message.as_str())
        .collect();
    assert_eq!(
        returns.len(),
        1,
        "only the dead-edge return reports: {:?}",
        messages(&r)
    );
}

/// The `yield*` implied type is the RAW expression type
/// (`impliedReturnType = expressionType` for the isYieldEach ask,
/// yield_statement_resolver.dart — the live oracle prints 'String', never
/// a 'Stream<String>' wrap). A sequence subtype carrying its own fixed
/// instantiation (`Seq extends Iterable<Item>`) answers through its
/// clause and stays silent (the corpus rows xml xpath sequence.dart);
/// a raw non-sequence value reports against the declared head.
#[test]
fn yield_star_implied_type_is_raw_expression_type() {
    let r = super::test_rig::report_ops(concat!(
        "class Item {}\n",
        "class Seq extends Iterable<Item> {}\n",
        "Iterable<Item> ok() sync* { Seq s = Seq(); yield* s; }\n",
        "Iterable<Item> bad() sync* { yield* 'x'; }\n",
    ));
    let yields: Vec<&str> = r
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "yield_of_invalid_type")
        .map(|d| d.message.as_str())
        .collect();
    assert_eq!(
        yields,
        ["The type 'String' implied by the 'yield*' expression must be assignable to 'Iterable<Item>'."],
        "{:?}",
        messages(&r)
    );
}

/// A switch-expression scrutinee read through a receiver whose Named
/// type arrived from CROSS-FILE inference (the callee's own library
/// spells the return type; the consuming file never imports the name)
/// is not the InvalidType face: the oracle's element model carries the
/// resolved element (`var result = typePropertyResolver.resolve()`
/// types the scrutinee `ResolutionResult` and keeps every arm live —
/// the corpus property_element_resolver.dart:1615 rows). Only a head
/// the whole graph fails to declare is the invalid face.
#[test]
fn cross_file_inferred_scrutinee_keeps_arms_live() {
    let other = [
        "class Ext {}",
        "class Res {",
        "  final Ext? getter2;",
        "  const Res(this.getter2);",
        "}",
        "Res resolve() => const Res(null);",
    ].join("\n");
    let main = [
        "class MethodEl {}",
        "Object f() {",
        "  var result = p.resolve();",
        "  return switch (result.getter2) {",
        "    MethodEl(:var type) => 0,",
        "    p.Ext(:var returnType) => 1,",
        "    _ => 2,",
        "  };",
        "}",
    ].join("\n");
    let r = super::test_rig::report_ops_imports(&main, &[super::test_rig::RigImport {
        uri: "package:t/other.dart",
        prefix: Some("p"),
        source: &other,
    }]);
    let dead: Vec<&str> = r
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "dead_code")
        .map(|d| d.message.as_str())
        .collect();
    assert!(dead.is_empty(), "arms died: {:?} ALL: {:?}", dead, messages(&r));
}

/// A callback formal over a receiver whose element type is a FOREIGN
/// but real class (spelled by another graph unit, never imported into
/// the diagnosing file — `List<Reference>.map((e) => e.uriString)`'s
/// `_fe_analyzer_shared` `Reference`) is that class, not the `Object?`
/// invalid recovery: the member read on the formal stays silent (the
/// corpus driver.dart:2868 row — the unchecked verdict the recovery
/// produced).
#[test]
fn foreign_element_type_callback_formal_is_not_nullable() {
    let other = ["class Ref {}"].join("\n");
    let main = [
        "void f(List<p.Ref> refs) {",
        "  refs.map((e) => e.uriString);",
        "}",
    ].join("\n");
    let r = super::test_rig::report_ops_imports(&main, &[super::test_rig::RigImport {
        uri: "package:t/other.dart",
        prefix: Some("p"),
        source: &other,
    }]);
    let unchecked: Vec<&str> = r
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "unchecked_use_of_nullable_value")
        .map(|d| d.message.as_str())
        .collect();
    assert!(unchecked.is_empty(), "unchecked rows: {:?}", unchecked);
}

/// A cascade section's receiver reads the target's flow-PROMOTED face:
/// `if (v != null) v..a()..b()` resolves every section against
/// `cascade.target2`'s promoted type (type_analyzer.dart's
/// `cascadeReceiver`), so neither section reports
/// `unchecked_use_of_nullable_value` (the corpus rows riverpod
/// element.dart:126/127, analyzer ast_builder.dart:1349/1350, archive
/// zip_decoder.dart:72/73). Only the pure null-stripping promotion
/// substitutes — a `?..` section keeps its nullable latched face.
#[test]
fn cascade_section_receiver_reads_promoted_target() {
    let r = super::test_rig::report_ops(concat!(
        "class Fut { void ignore() {} }\n",
        "class C { final Fut future; C(this.future); void completeError(Object e) {} }\n",
        "class Holder { C? c;\n",
        "  void m() {\n",
        "    final v = c;\n",
        "    if (v != null) {\n",
        "      v\n",
        "        ..future.ignore()\n",
        "        ..completeError(0);\n",
        "    }\n",
        "  }\n",
        "}\n",
    ));
    let unchecked: Vec<&str> = r
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "unchecked_use_of_nullable_value")
        .map(|d| d.message.as_str())
        .collect();
    assert_eq!(unchecked, Vec::<&str>::new(), "{:?}", messages(&r));
}

/// A write through an index-bang or getter-bang RHS types the assigned
/// local: `command = commands[name]!` and `argResults = argResults
/// .command!` both promote the nullable local for later reads (the args
/// command_runner.dart:188-196 idiom — the oracle's write promotion over
/// a null-asserted RHS).
#[test]
fn bang_rhs_write_promotes_the_local() {
    let r = super::test_rig::report_ops(concat!(
        "class Cmd { final sub = <String, Cmd>{}; final name = 'x'; Cmd? get command => null; }\n",
        "void f(Map<String, Cmd> commands, Cmd argResults) {\n",
        "  Cmd? command;\n",
        "  command = commands[argResults.name]!;\n",
        "  command.name;\n",
        "  argResults = argResults.command!;\n",
        "  argResults.name;\n",
        "}\n",
    ));
    let unchecked: Vec<&str> = r
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "unchecked_use_of_nullable_value")
        .map(|d| d.message.as_str())
        .collect();
    assert_eq!(unchecked, Vec::<&str>::new(), "{:?}", messages(&r));
}

/// A prefixed import never shadows the bare namespace (#2567, the
/// blaze_watcher shape): the unit imports `other.dart` bare — its own
/// `File` carrying the members — beside a PREFIXED import of `sdk.dart`
/// exporting a different, member-less `File`. The flat registry answers
/// the bare spelling with the bare import's declaration, `is File`
/// promotes over `Resource`, and the promoted reads
/// (`modificationStamp`, `lengthSync`) resolve. The import sweep's old
/// first-writer-wins walk registered the prefixed chain's `File`
/// instead, refused the promotion, and reported both reads as
/// `undefined_getter`.
#[test]
fn prefixed_import_does_not_shadow_bare_registration() {
    let r = super::test_rig::report_ops_imports(
        concat!(
            "class W {\n",
            "  final Provider _provider;\n",
            "  W(this._provider);\n",
            "  int? poll(String path) {\n",
            "    try {\n",
            "      var pathResource = _provider.getResource(path);\n",
            "      var symlinkTarget = pathResource.resolveSymbolicLinksSync().path;\n",
            "      var resolvedResource = _provider.getResource(symlinkTarget);\n",
            "      if (resolvedResource is File) {\n",
            "        var timestamp = resolvedResource.modificationStamp;\n",
            "        var length = resolvedResource.lengthSync;\n",
            "        return timestamp + length;\n",
            "      } else if (resolvedResource is Folder) {\n",
            "        return 0;\n",
            "      } else {\n",
            "        return null;\n",
            "      }\n",
            "    } on FileSystemException catch (_) {\n",
            "      return null;\n",
            "    }\n",
            "  }\n",
            "}\n",
        ),
        &[
            super::test_rig::RigImport {
                uri: "package:t/other.dart",
                prefix: None,
                source: concat!(
                    "abstract class Resource { Resource resolveSymbolicLinksSync(); String get path; }\n",
                    "abstract class File implements Resource { final int modificationStamp = 0; final int lengthSync = 0; }\n",
                    "abstract class Folder implements Resource { final String path = ''; }\n",
                    "class FileSystemException implements Exception {}\n",
                    "abstract class Provider { Resource getResource(String path); }\n",
                ),
            },
            super::test_rig::RigImport {
                uri: "package:t/sdk.dart",
                prefix: Some("io"),
                source: "class File { int get unrelated => 0; }\n",
            },
        ],
    );
    let miss: Vec<&str> = r
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "undefined_getter")
        .map(|d| d.message.as_str())
        .collect();
    assert_eq!(miss, Vec::<&str>::new(), "{:?}", messages(&r));
}

/// A core-member call right side stages the write's type (#2567, the
/// xml qname shape): `String.substring` has no element model in the
/// graph (the SDK libraries), so the send's return used to fold the
/// unknown face — `final String? prefix; prefix = s.substring(..);`
/// never promoted, and the later read reported
/// `argument_type_not_assignable` where the oracle resolves through
/// dart:core. The builtin core member face (`callee_return_ty_for_write`)
/// answers the assignment-RHS staging alone: `prefix` promotes to
/// `String` and the argument check passes.
#[test]
fn core_member_rhs_stages_the_write_promotion() {
    let r = super::test_rig::report_ops(concat!(
        "void g(String s) {}\n",
        "class Q {\n",
        "  void f(String paramQName, int colonIndex) {\n",
        "    final String? prefix;\n",
        "    prefix = paramQName.substring(0, colonIndex);\n",
        "    if (!g2(prefix)) { throw \'\'; }\n",
        "  }\n",
        "  bool g2(String s) => true;\n",
        "}\n",
    ));
    let miss: Vec<&str> = r
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "argument_type_not_assignable")
        .map(|d| d.message.as_str())
        .collect();
    assert_eq!(miss, Vec::<&str>::new(), "{:?}", messages(&r));
}

/// A bare-container element constructor on a write's right side
/// instantiates its parameter from the assignment's context (#2567, the
/// intl locale shape): `List.unmodifiable` spells no receiver arguments,
/// so the core member face alone staged an argless `List` whose
/// `dynamic` element the promotion gate refused against the declared
/// `Iterable<String>` — the oracle's downward inference writes
/// `List<String>` (the context's element), which promotes and the later
/// argument read passes.
#[test]
fn element_constructor_rhs_instantiates_from_the_write_context() {
    let r = super::test_rig::report_ops(concat!(
        "class L {\n",
        "  final Iterable<String>? variants;\n",
        "  L(this.variants);\n",
        "}\n",
        "void g(Iterable<String> xs) {}\n",
        "class W {\n",
        "  void f(Iterable<String>? variants) {\n",
        "    variants = (variants != null && variants.isNotEmpty)\n",
        "        ? List.unmodifiable(variants.toList()..sort())\n",
        "        : const [];\n",
        "    g(variants);\n",
        "  }\n",
        "}\n",
    ));
    let miss: Vec<&str> = r
        .diagnostics
        .iter()
        .filter(|d| d.code.name == "argument_type_not_assignable")
        .map(|d| d.message.as_str())
        .collect();
    assert_eq!(miss, Vec::<&str>::new(), "{:?}", messages(&r));
}

/// A nullable-`on` extension member wins over the class's own instance
/// member on a nullable receiver (#2567, the manifest_item shape): the
/// oracle's applicable-extension consult runs against the nullable
/// receiver type, and `extension E on E?` resolves the access without a
/// dereference — `x.match(...)` stays silent even though the class also
/// declares an instance `match` (the unsafe instance read never runs).


/// Explicit type arguments at a call to a generic top-level function
/// declared in ANOTHER file fix its inference parameters outright — the
/// upward face (here: `List<Object?>` arguments, which alone would solve
/// `T1/T2 := Object?`, a nullable face) neither re-solves nor poisons the
/// written instantiation. The oracle runs no inference over an explicit
/// argument list (instance_member_inferrer.dart:511-521).

/// A field whose initializer is a typed collection literal carries the
/// literal's written arguments as its declared face (`final _commands =
/// <String, Command<T>>{}` declares `Map<String, Command<T>>`), so the
/// `var`-inferred local over it, its index read, and the trailing-`!`
/// write all promote — the member accesses on the written local stay
/// clean (args command_runner.dart:73/156/189-196).
/// A closure formal's schema entry spelled through an import prefix
/// (`List<idl.IndexSyntheticElementKind>`'s element head interns
/// dot-joined) stays decidable: the closure-formal solver resolves the
/// prefixed head through the import prefix instead of collapsing it to
/// `Object?`, so the formal's member reads stay clean under a negated
/// disjunction guard (`!(eks == null || eks.isEmpty)` — the guard's
/// non-null promotion plus the solved formal keep `b.index` and the
/// guarded read clean; format.dart:1479/1560/1585).
#[test]
fn demorgan_guard_promotes_prefixed_element_closure_formal() {
    let idl = concat!(
        "class K {\n",
        "  final int index = 0;\n",
        "}\n",
    );
    let main = concat!(
        "import 'idl.dart' as idl;\n",
        "class W {\n",
        "  List<idl.K>? _eks;\n",
        "  int f() {\n",
        "    var eks = _eks;\n",
        "    if (!(eks == null || eks.isEmpty)) {\n",
        "      return eks.map((b) => b.index).length;\n",
        "    }\n",
        "    return 0;\n",
        "  }\n",
        "}\n",
    );
    let r = super::test_rig::report_ops_imports(main, &[super::test_rig::RigImport {
        uri: "idl.dart",
        prefix: Some("idl"),
        source: idl,
    }]);
    // The rig's core stubs carry no `List.map`/`List.isEmpty` element
    // model, so those two rows report as unresolved getters — the pinned
    // faces are the guard's promotion (no unchecked nullable use) and
    // the solved closure formal (`b.index` resolves on `idl.K`, and no
    // argument mismatch rides the guarded reads).
    for d in &r.diagnostics {
        assert_ne!(
            d.code.name, "unchecked_use_of_nullable_value",
            "demorgan guard must promote the guarded local: {}",
            d.message
        );
        assert_ne!(
            d.code.name, "argument_type_not_assignable",
            "solved formal must carry the prefixed element face: {}",
            d.message
        );
        assert!(
            !d.message.contains("getter 'index'"),
            "closure formal must resolve through the prefixed element head: {}",
            d.message
        );
    }
}

#[test]
fn typed_literal_field_face_carries_through_index_bang_write() {
    let source = concat!(
        "class Cmd<T> { int x = 0; }\n",
        "class R<T> {\n",
        "  final _cmds = <String, Cmd<T>>{};\n",
        "  void f() {\n",
        "    var m = _cmds;\n",
        "    Cmd? command;\n",
        "    command = m['k']!;\n",
        "    command.x = 1;\n",
        "  }\n",
        "}\n",
    );
    let r = super::test_rig::report_ops(source);
    let unchecked: Vec<&str> = r
        .diagnostics
        .iter()
        .filter(|d| d.code.name.contains("unchecked"))
        .map(|d| d.code.name)
        .collect();
    assert_eq!(unchecked, Vec::<&str>::new(), "{:?}", messages(&r));
}

#[test]
fn explicit_type_arguments_fix_foreign_callee_closure_formals() {
    let main = concat!(
        "import 'helper.dart';\n",
        "class P {\n",
        "  int type = 0;\n",
        "}\n",
        "void f(List<Object?> xs) {\n",
        "  forCorrespondingPairs<P, P>(\n",
        "    xs,\n",
        "    xs,\n",
        "    (parameter, baseParameter) {\n",
        "      parameter.type = baseParameter.type;\n",
        "    },\n",
        "  );\n",
        "}\n",
    );
    let helper = concat!(
        "void forCorrespondingPairs<T1, T2>(\n",
        "  Iterable<T1> list1,\n",
        "  Iterable<T2> list2,\n",
        "  void Function(T1, T2) f,\n",
        ") {\n",
        "  var i1 = list1.iterator;\n",
        "  var i2 = list2.iterator;\n",
        "  while (i1.moveNext() && i2.moveNext()) {\n",
        "    f(i1.current, i2.current);\n",
        "  }\n",
        "}\n",
    );
    let r = super::test_rig::report_ops_imports(main, &[super::test_rig::RigImport {
        uri: "helper.dart",
        prefix: None,
        source: helper,
    }]);
    let unchecked: Vec<&str> = r
        .diagnostics
        .iter()
        .filter(|d| d.code.name.contains("unchecked"))
        .map(|d| d.code.name)
        .collect();
    assert_eq!(unchecked, Vec::<&str>::new(), "{:?}", messages(&r));
}

#[test]
fn nullable_on_extension_wins_over_instance_member() {
    let r = super::test_rig::report_ops(concat!(
        "class Z {\n",
        "  bool match(Object other) => false;\n",
        "}\n",
        "extension EE on Z? {\n",
        "  bool match(Object? other) => true;\n",
        "}\n",
        "class W {\n",
        "  final Z? x;\n",
        "  W(this.x);\n",
        "  bool f() => x.match(null);\n",
        "}\n",
    ));
    let miss: Vec<&str> = r
        .diagnostics
        .iter()
        .map(|d| d.code.name)
        .collect();
    assert_eq!(miss, Vec::<&str>::new(), "{:?}", messages(&r));
}

/// A write whose right-hand side is a promoted property read keeps the
/// promotion: `file = _defaultFile` under `_defaultFile != null` writes
/// the promoted face, so the target local promotes exactly like the
/// variable-meta arm's `b = a` — the flow fold of a property read is the
/// non-null promotion marker beside the semantic layer's staged nullable
/// declared head, and the staged head minus its nullability is the
/// write's type (context_locator.dart:215-216's `file.parent` reads stay
/// clean).
#[test]
fn promoted_field_read_rhs_promotes_the_write_target() {
    let src = concat!(
        "class F {\n",
        "  F get parent => this;\n",
        "}\n",
        "class W {\n",
        "  final F? _defaultFile;\n",
        "  W(this._defaultFile);\n",
        "  F? _find(F f) => f;\n",
        "  void f() {\n",
        "    F? file;\n",
        "    if (_defaultFile != null) {\n",
        "      file = _defaultFile;\n",
        "      var x = _find(file.parent) ?? file.parent;\n",
        "      g(x);\n",
        "    }\n",
        "  }\n",
        "  void g(F x) {}\n",
        "}\n",
    );
    let r = super::test_rig::report_ops(src);
    // The pinned faces are the write's promotion: both `file.parent`
    // reads stay clean (no unchecked nullable use) and the guarded reads
    // carry no argument mismatch.
    for d in &r.diagnostics {
        assert_ne!(
            d.code.name, "unchecked_use_of_nullable_value",
            "the promoted field read's write must promote the target: {}",
            d.message
        );
        assert_ne!(
            d.code.name, "argument_type_not_assignable",
            "the promoted read keeps its face through the write: {}",
            d.message
        );
    }
}

/// A null-aware RECORD-field read's chain continuation stays clean:
/// `result?.read.icc` on `({M read, …})?` latches the continuation the
/// way the class-member design does — the record arm bakes the short's
/// `?` into the stored face, so the field's own face rides the
/// shorted-field map and the continuation never dereferences the short's
/// null (assignment_expression_resolver.dart:686/693/694).
#[test]
fn record_short_field_chain_continuation_stays_clean() {
    let src = concat!(
        "class M {\n",
        "  final int icc = 0;\n",
        "  final int eor = 1;\n",
        "}\n",
        "({M read, M write})? resolve() => null;\n",
        "void f() {\n",
        "  var result = resolve();\n",
        "  var x = result?.read.icc;\n",
        "  var y = result?.read.eor;\n",
        "  var z = result?.write.eor;\n",
        "  g(x ?? 0); g(y ?? 0); g(z ?? 0);\n",
        "}\n",
        "void g(int v) {}\n",
    );
    let r = super::test_rig::report_ops(src);
    for d in &r.diagnostics {
        assert_ne!(
            d.code.name, "unchecked_use_of_nullable_value",
            "the shorted field's chain continuation never dereferences the short: {}",
            d.message
        );
        assert_ne!(
            d.code.name, "argument_type_not_assignable",
            "the chain node keeps the short's nullability for the enclosing read: {}",
            d.message
        );
    }
}


/// A cascade section's write no longer retracts the cascade target's own
/// read facts: the first section's selector latches on the target's read
/// span, so the flow's promotion of the target read answers the receiver
/// verdict (`var builder = _b; if (builder != null) builder..a = x..b = y`
/// stays clean through every section — the official keeps the target read
/// promoted across section writes, ast_builder.dart:1349-1350).
#[test]
fn cascade_sections_on_promoted_local_stay_clean() {
    let src = concat!(
        "class B {\n",
        "  int? leftBracket;\n",
        "  int? rightBracket;\n",
        "}\n",
        "class W {\n",
        "  B? _b;\n",
        "  void f(int lb, int rb) {\n",
        "    var builder = _b;\n",
        "    if (builder != null) {\n",
        "      builder\n",
        "        ..leftBracket = lb\n",
        "        ..rightBracket = rb;\n",
        "    }\n",
        "  }\n",
        "}\n",
    );
    let r = super::test_rig::report_ops(src);
    for d in &r.diagnostics {
        assert_ne!(
            d.code.name, "unchecked_use_of_nullable_value",
            "the promoted target read answers every section's receiver: {}",
            d.message
        );
        assert_ne!(
            d.code.name, "undefined_setter",
            "the sections resolve on the cascade target's own class: {}",
            d.message
        );
    }
}
