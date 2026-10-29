//! The undefined-name/resolution family pins: the corpus differential
//! between dartq and `dart analyze` (the `(severity, code, file,
//! line:col)` Jaccard gate) burned down through three root patterns.
//! Each fixture was probed against oracle `dart analyze` (fvm default,
//! Dart 3.13-era) with a minimal source.
//!
//! - Constructor-reference prefix heads (`fb.RuleState.stable()`): the
//!   oracle's `resolveConstructorTypeReference` importPrefix arm resolves
//!   the CLASS segment through the prefix scope — a type hit is silent;
//!   a miss reports `creation_with_non_type` at the class token
//!   (upstream Dart SDK source, pkg/analyzer/lib/src/dart/resolver/
//!   named_type_resolver.dart:134-332). The prefix itself is a resolved
//!   element and never reports `undefined_identifier`.
//! - Record `on` types (`extension E on (int, int)`): the record's `$N`
//!   positional and named getters belong to `this`'s member scope, so a
//!   bare `$1` inside the body resolves; an out-of-range `$9` still
//!   reports `undefined_identifier`.
//! - Extension invoke misses name the `on` type's `this`, not the
//!   extension: `on C` → `undefined_method` for 'C', `on (int, int)` →
//!   'UndefinedMethod' for '<unknown>', `on Missing` → nothing (the
//!   `this` is the oracle's `InvalidType`; the `undefined_class` on the
//!   header is the only report).

use super::*;
use crate::collector::collect;
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_resolution::{Combinators, LibraryGraphBuilder};
use dartq_scanner::ScanConfig;
use dartq_source::FileTable;
use std::path::Path;

/// Diagnoses `source` as the sole file of a library with no imports —
/// the `tests.rs` rig mirrored (the sibling rig stays off-limits while
/// its lane is in flight).
fn report(source: &str) -> FileReport {
    let strings = StringTable::new();
    let mut files = FileTable::new();
    let file = files.intern(Path::new("main.dart"));
    let (scanned, features, set) = scan(source, file);
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

/// Diagnoses `source_m` importing `source_a` under `prefix` (the
/// two-file rig the constructor-prefix faces need: `fb.RuleState.x`
/// resolves `RuleState` through `fb`'s export scope).
fn report_prefixed(source_m: &str, source_a: &str, prefix: &str) -> FileReport {
    let strings = StringTable::new();
    let mut files = FileTable::new();
    let file_a = files.intern(Path::new("a.dart"));
    let file_m = files.intern(Path::new("main.dart"));
    let (scanned_a, features, _) = scan(source_a, file_a);
    let (scanned_m, _, set) = scan(source_m, file_m);
    let elements_a = collect(source_a, &scanned_a.tokens, features, &strings);
    let elements_m = collect(source_m, &scanned_m.tokens, features, &strings);
    let mut builder = LibraryGraphBuilder::new(&strings);
    let lib_a = builder.intern("package:t/a.dart");
    let lib_m = builder.intern("package:t/main.dart");
    let mut index = ElementIndex::default();
    index.insert(file_a, elements_a.clone());
    index.insert(file_m, elements_m.clone());
    for (name, _) in elements_a.declarations() {
        builder.declare(lib_a, name, file_a);
    }
    for (name, decl) in elements_m.declarations() {
        if decl.kind == MicroKind::TypeParam {
            continue;
        }
        builder.declare(lib_m, name, file_m);
    }
    builder.import(
        lib_m,
        ImportEdge {
            target: Some(lib_a),
            prefix: Some(strings.intern(prefix)),
            deferred: false,
            combinators: Combinators::unfiltered(),
        },
    );
    let graph = builder.finish();
    let namespaces = Namespaces::build(&graph);
    let fold = fold_unit(
        source_m,
        &scanned_m.tokens,
        features,
        &strings,
        file_m,
        lib_m,
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

fn scan(source: &str, file: FileId) -> (dartq_scanner::ScanResult, ParserFeatures, FeatureSet) {
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
    (scanned, features, set)
}

fn code_names(report: &FileReport) -> Vec<&'static str> {
    report
        .diagnostics
        .iter()
        .map(|d| d.code.name)
        .collect::<Vec<_>>()
}

/// `(code, offset)` rows, for span assertions (`node.name` tokens).
fn rows(report: &FileReport) -> Vec<(&'static str, u32)> {
    report
        .diagnostics
        .iter()
        .map(|d| (d.code.name, d.range.offset))
        .collect()
}

// —— P0: constructor-reference heads resolving to an import prefix ————

/// `const fb.RuleState.stable()` with `RuleState` exported under `fb`:
/// the oracle resolves the class segment through the prefix scope and
/// stays silent — no `undefined_identifier` on the prefix, no
/// `creation_with_non_type` (named_type_resolver.dart:140-147).
#[test]
fn ctor_prefix_class_hit_is_silent() {
    let report = report_prefixed(
        "import 'a.dart' as fb;\nvoid f() { var y = const fb.RuleState.stable(); }",
        "class RuleState { const RuleState.stable(); }",
        "fb",
    );
    let names = code_names(&report);
    for code in [
        "undefined_identifier",
        "undefined_class",
        "creation_with_non_type",
    ] {
        assert!(!names.contains(&code), "{code} -> {names:?}");
    }
}

/// The class-segment miss reports `creation_with_non_type` AT the
/// class-name token — the oracle's `diagnostic.at(node.name)` tail —
/// never the prefix span.
#[test]
fn ctor_prefix_class_miss_reports_at_class_name() {
    let source_m = "import 'a.dart' as fb;\nvoid f() { var y = const fb.Gone.stable(); }";
    let report = report_prefixed(source_m, "class RuleState {}", "fb");
    let spans = rows(&report);
    let gone = source_m.find("Gone").unwrap() as u32;
    assert!(
        spans.contains(&("creation_with_non_type", gone)),
        "{spans:?}"
    );
    for (code, _) in &spans {
        assert!(
            !["undefined_identifier", "undefined_class"].contains(code),
            "{spans:?}"
        );
    }
}

/// Same face without `const` — `isConst` false upstream still reports
/// `creation_with_non_type` (its `new_with_non_type` unique name) at
/// the class token.
#[test]
fn ctor_prefix_class_miss_without_const() {
    let source_m = "import 'a.dart' as fb;\nvoid f() { var y = new fb.Gone(); }";
    let report = report_prefixed(source_m, "class RuleState {}", "fb");
    let names = code_names(&report);
    assert!(names.contains(&"creation_with_non_type"), "{names:?}");
    assert!(!names.contains(&"undefined_identifier"), "{names:?}");
}

/// A prefix spelled bare in creation position (`new fb()`) is not a
/// class: `creation_with_non_type` at the head token.
#[test]
fn bare_prefix_in_creation_reports_non_type() {
    let source_m = "import 'a.dart' as fb;\nvoid f() { var y = new fb(); }";
    let report = report_prefixed(source_m, "class RuleState {}", "fb");
    let names = code_names(&report);
    assert!(names.contains(&"creation_with_non_type"), "{names:?}");
    assert!(!names.contains(&"undefined_identifier"), "{names:?}");
}

/// A two-segment prefix reference (`new fb.RuleState()`) resolves the
/// class through the prefix scope — silent on hit.
#[test]
fn ctor_prefix_two_segment_hit_is_silent() {
    let report = report_prefixed(
        "import 'a.dart' as fb;\nvoid f() { var y = new fb.RuleState(); }",
        "class RuleState { RuleState(); }",
        "fb",
    );
    let names = code_names(&report);
    for code in [
        "undefined_identifier",
        "undefined_class",
        "creation_with_non_type",
    ] {
        assert!(!names.contains(&code), "{code} -> {names:?}");
    }
}

// —— P1: record `on` types expose their field getters ————————————————

/// Inside `extension E on (C, D)` the bare positional getters
/// `$1`/`$2` resolve through `this`'s record face; the out-of-range
/// `$9` still reports `undefined_identifier` (oracle probe: `dart
/// analyze` reports the miss only). Field types are declared classes —
/// the rig carries no dart:core, so `int`/`String` would read as
/// `undefined_class` noise here.
#[test]
fn extension_record_on_resolves_field_getters() {
    let source = "class C {} class D {}\n\
                      extension E on (C, D) {\n\
                        void m() { var a = $1; var b = $2; var c = $9; }\n\
                      }";
    let names = code_names(&report(source));
    assert_eq!(
        names
            .iter()
            .filter(|c| **c == "undefined_identifier")
            .count(),
        1,
        "{names:?}"
    );
    for code in ["undefined_class", "undefined_method", "undefined_function"] {
        assert!(!names.contains(&code), "{code} -> {names:?}");
    }
}

/// Named record fields resolve the same way; an absent name misses.
#[test]
fn extension_record_on_resolves_named_fields() {
    let source = "class C {} class D {}\n\
                      extension E on (C, {D title}) {\n\
                        void m() { var a = title; var b = absent; }\n\
                      }";
    let names = code_names(&report(source));
    assert_eq!(
        names
            .iter()
            .filter(|c| **c == "undefined_identifier")
            .count(),
        1,
        "{names:?}"
    );
    for code in ["undefined_class", "undefined_method", "undefined_function"] {
        assert!(!names.contains(&code), "{code} -> {names:?}");
    }
}

// —— P2: extension invoke misses name the `on` type's `this` ——————————

/// `extension E on C` + a bare invoke miss → `undefined_method` for
/// 'C' (the extended type, not the extension name) — oracle probe.
#[test]
fn extension_on_resolved_names_on_type() {
    let source = "class C {}\n\
                      extension E on C {\n\
                        void m() { missingCall(); }\n\
                      }";
    let report = report(source);
    let names = code_names(&report);
    assert_eq!(
        names.iter().filter(|c| **c == "undefined_method").count(),
        1,
        "{names:?}"
    );
    let diag = report
        .diagnostics
        .iter()
        .find(|d| d.code.name == "undefined_method")
        .unwrap();
    assert!(diag.message.contains("'C'"), "{diag:?}");
    assert!(!names.contains(&"undefined_function"), "{names:?}");
}

/// An unnamed extension names its `on` type identically.
#[test]
fn unnamed_extension_on_resolved_names_on_type() {
    let source = "class C {}\n\
                      extension on C {\n\
                        void m() { missingCall(); }\n\
                      }";
    let names = code_names(&report(source));
    assert_eq!(
        names.iter().filter(|c| **c == "undefined_method").count(),
        1,
        "{names:?}"
    );
    assert!(!names.contains(&"undefined_function"), "{names:?}");
}

/// `extension F on Missing` — the `this` is the oracle's
/// `InvalidType`; the header reports `undefined_class` and the body's
/// invoke miss reports NOTHING (no `undefined_function`, no
/// `undefined_method`).
#[test]
fn extension_on_missing_is_silent() {
    let source = "extension F on Missing {\n\
                        void m() { anotherMiss(); }\n\
                      }";
    let names = code_names(&report(source));
    assert_eq!(names, ["undefined_class"], "{names:?}");
}

/// `extension on (C, D)` — a record `this` has no element name; the
/// oracle displays '<unknown>' as the host type. Field types are
/// declared classes (the rig carries no dart:core).
#[test]
fn extension_on_record_names_unknown() {
    let source = "class C {} class D {}\n\
                      extension G on (C, D) {\n\
                        void m() { thirdMiss(); }\n\
                      }";
    let report = report(source);
    let names = code_names(&report);
    assert_eq!(
        names.iter().filter(|c| **c == "undefined_method").count(),
        1,
        "{names:?}"
    );
    let diag = report
        .diagnostics
        .iter()
        .find(|d| d.code.name == "undefined_method")
        .unwrap();
    assert!(diag.message.contains("<unknown>"), "{diag:?}");
}

/// `on void Function()`: the oracle displays 'Function' — the
/// function-type `this` (oracle probe: `c()` reports `undefined_method`
/// "isn't defined for the type 'Function'"). The interned function
/// `on_ty` reaching `this_static_ty`'s fallback made the miss report
/// exactly like the named face (the pre-fallback posture was a silent
/// under-report).
#[test]
fn extension_on_function_type_is_silent() {
    let source = "extension G on void Function() {\n\
                        void m() { c(); }\n\
                      }";
    let report = report(source);
    let names = code_names(&report);
    assert!(!names.contains(&"undefined_function"), "{names:?}");
    assert_eq!(
        names.iter().filter(|c| **c == "undefined_method").count(),
        1,
        "{names:?}"
    );
    let diag = report
        .diagnostics
        .iter()
        .find(|d| d.code.name == "undefined_method")
        .unwrap();
    assert!(diag.message.contains("Function"), "{diag:?}");
}

/// A mixin `on` miss keeps the MIXIN name — `this` inside a mixin is
/// the mixin, not a constraint (oracle probe: `mixinMiss2` reports
/// 'N'). Guards the P2 change against regressing the mixin face.
#[test]
fn mixin_on_miss_names_the_mixin() {
    let source = "mixin N on Gone {\n\
                        void m() { mixinMiss2(); }\n\
                      }";
    let names = code_names(&report(source));
    assert!(names.contains(&"undefined_method"), "{names:?}");
}

// —— #2249: the extension-application producer gates —————————————————

/// An `InvalidType` receiver never applies an extension: `Missing x`
/// resolves no type element, so `x.used` cannot credit `_E.used` on
/// `Missing` — the member stays `unused_element` beside the receiver's
/// own miss rows (the oracle's `applicableTo` returns no
/// `ExtensionResolution` for an `InvalidType`).
#[test]
fn extension_invalid_receiver_credits_nothing() {
    let source = "extension _E on Missing { int get used => 0; }\n\
                      void f(Missing x) { x.used; }\n";
    let names = code_names(&report(source));
    assert!(names.contains(&"unused_element"), "{names:?}");
}

/// The resolved-`on` twin of the InvalidType gate: a resolved receiver
/// head whose TYPE ARGUMENT does not resolve (`List<Missing>`) is
/// still an `InvalidType`, so `x.used` reports the receiver's own miss
/// rows and never reaches the extension's member face — the `on` list
/// resolves (`List<C>`), but the access keeps its `undefined_class` +
/// `non_type_as_type_argument` pair with no `undefined_getter` beside
/// it.
#[test]
fn extension_invalid_receiver_argument_credits_nothing() {
    let source = "class List<T> {} class C {}\n\
                      extension _E on List<C> { int get used => 0; }\n\
                      void f(List<Missing> x) { x.used; }\n";
    let names = code_names(&report(source));
    assert!(names.contains(&"undefined_class"), "{names:?}");
    assert!(names.contains(&"non_type_as_type_argument"), "{names:?}");
    assert!(!names.contains(&"undefined_method"), "{names:?}");
    assert!(!names.contains(&"undefined_getter"), "{names:?}");
}

/// An unsafe match (nullable receiver vs a non-nullable `on`) reports
/// `unchecked_use_of_nullable_value` at the selector AND leaves the
/// member uncredited — the oracle never enters the extension element
/// into `usedElements`, so `unused_element` still fires on `used`.
#[test]
fn extension_unsafe_match_still_reports_unchecked_and_unused() {
    let source = "class L {}\n\
                      extension _E on L { int get used => 0; }\n\
                      void f(L? l) { l.used; }\n";
    let names = code_names(&report(source));
    assert!(
        names.contains(&"unchecked_use_of_nullable_value"),
        "{names:?}"
    );
    assert!(names.contains(&"unused_element"), "{names:?}");
    assert!(!names.contains(&"undefined_getter"), "{names:?}");
}

/// The safe nullable match credits normally — `on L?` applies to `L?`
/// and the member stays used.
#[test]
fn extension_nullable_on_credits_member() {
    let source = "class L {}\n\
                      extension _E on L? { int get used => 0; }\n\
                      void f(L? l) { l.used; }\n";
    let names = code_names(&report(source));
    assert!(!names.contains(&"unused_element"), "{names:?}");
    assert!(
        !names.contains(&"unchecked_use_of_nullable_value"),
        "{names:?}"
    );
}

// —— #2260: receiver-model faces — named-selector creations ————————

/// A named-selector implicit creation folds to its class: `final c =
/// Ctr.broadcast(sync: true)` types `c` as `Ctr` (the oracle's
/// InstanceCreationExpression static type), so `c.ping()` resolves —
/// the constructor-reference posture pushes no callee row (the
/// resolving head is a type use, the selector none) and the argument
/// rows used to stand as the initializer's window, binding the LAST
/// ARGUMENT's type (`bool` — probed against the oracle, bloc bloc.dart
/// 353/363 `controller.close()` for `bool`).
#[test]
fn named_selector_creation_types_the_var() {
    let source = "class Ctr {\n  Ctr.broadcast({bool? sync});\n  void ping() {}\n}\n\
                      void f() {\n  final c = Ctr.broadcast(sync: true);\n  print(c.ping());\n}\n\
                      void print(Object x) {}\n";
    let names = code_names(&report(source));
    assert!(!names.contains(&"undefined_method"), "{names:?}");
    assert!(!names.contains(&"undefined_getter"), "{names:?}");
    assert!(!names.contains(&"undefined_setter"), "{names:?}");
}

/// The typed head (`Ctr<int>.broadcast(...)`) binds the same way — the
/// type-argument group sits between the class and the selector.
#[test]
fn typed_named_selector_creation_types_the_var() {
    let source = "class Ctr<T> {\n  Ctr.broadcast({bool? sync});\n  void ping() {}\n}\n\
                      void f() {\n  final c = Ctr<int>.broadcast(sync: true);\n  print(c.ping());\n}\n\
                      void print(Object x) {}\n";
    let names = code_names(&report(source));
    assert!(!names.contains(&"undefined_method"), "{names:?}");
}

/// The named-selector creation's declarator feed (the oracle's
/// `int bad = Ctr.broadcast(...)` answers `invalid_assignment` at the
/// creation — the pre-fix model read the last argument's `bool`). The
/// declarator-assignment face's consumption of the folded creation row
/// is a recorded #2260 residual (the face needs the SDK-spelled `int`,
/// unavailable in this rig world) — this pin holds the fold's
/// wrong-code contract: the creation neither mis-types the declarator
/// (no `bool`-carried face) nor reports a member miss.
#[test]
fn named_selector_creation_feeds_assignment_check() {
    let source = "class Ctr {\n  Ctr.broadcast({bool? sync});\n}\n\
                      void f() {\n  var bad = Ctr.broadcast(sync: true);\n  print(bad);\n}\n\
                      void print(Object x) {}\n";
    let names = code_names(&report(source));
    assert!(!names.contains(&"undefined_method"), "{names:?}");
    assert!(!names.contains(&"invalid_assignment"), "{names:?}");
}

/// A cascade section over the folded creation resolves on the class
/// face (`Ctr.broadcast(..)..ping()` — the section's receiver is the
/// created value).
#[test]
fn named_selector_creation_cascade_resolves() {
    let source = "class Ctr {\n  Ctr.broadcast({bool? sync});\n  void ping() {}\n}\n\
                      void f() {\n  Ctr.broadcast(sync: true)..ping();\n}\n";
    let names = code_names(&report(source));
    assert!(!names.contains(&"undefined_method"), "{names:?}");
}

/// An unresolved head keeps its own posture: the miss row reports and
/// no creation fold fires (the oracle's error-recovery creation type is
/// invalid — member reads stay silent beside the undefined name).
#[test]
fn undefined_named_selector_head_stays_miss_only() {
    let source = "void f() {\n  final c = Missing.named(3);\n  print(c.ping());\n}\n\
                      void print(Object x) {}\n";
    let report = report(source);
    let names = code_names(&report);
    assert!(names.contains(&"undefined_identifier"), "{names:?}");
    assert!(!names.contains(&"undefined_method"), "{names:?}");
    assert!(!names.contains(&"undefined_getter"), "{names:?}");
}

/// A non-type head (a local variable) is a member send, not a creation:
/// the fold must not fire (`n.named(3)` reports the invocation miss
/// against the local's type, exactly the oracle's face).
#[test]
fn local_head_named_send_stays_member_miss() {
    let source = "void f() {\n  var n = 1;\n  final d = n.named(3);\n  print(d);\n}\n";
    let report = report(source);
    let spans = rows(&report);
    let named = source.find("named").unwrap() as u32;
    assert!(spans.contains(&("undefined_method", named)), "{spans:?}");
}

// —— #2260: the keyword-spelled prefix head (dio dio_mixin.dart:419) —

/// Value-position built-in reads never report an
/// `undefined_identifier`: `T != dynamic` resolves both sides (the
/// oracle resolves the TypeLiteral/`dynamic` top type — dio
/// dio_mixin.dart:419/644). The value-position row rides the keyword
/// gate; the same skip guards the recovery-routed PrefixHead face
/// whose `UndefinedIdentifier` emit lacked it. The SDK-spelled type
/// positions in this rig world report `undefined_class` beside the
/// gate's scope (the rig has no dart:core faces) — the CLI/oracle
/// pair on the corpus source is 1:1 clean.
#[test]
fn type_param_vs_dynamic_comparison_is_clean() {
    let source = "class Dio {\n  void fetch<T>(Object o) async {\n    \
                      if (T != dynamic) {\n      print(1);\n    }\n  }\n}\n\
                      void print(Object x) {}\n";
    let names = code_names(&report(source));
    assert!(!names.contains(&"undefined_identifier"), "{names:?}");
}

// —— #2275: the extension-receiver InvalidType gate ————————————————

/// A callee's declared return carrying its OWN type parameters — the
/// `A`/`B` inside `seq2`'s `Parser<(A, B)>` — is not an `InvalidType`
/// receiver: those spellings are bound at the callee's declaration and
/// instantiate at the send, so the extension's `on` face applies.
/// Before the fix the name-level read treated them as unresolvable
/// heads and refused the application — `undefined_method` on `map2`
/// (petitparser `map2`/`map3`/… over `Parser<(T, …)>`).
#[test]
fn extension_applies_through_callee_type_param_record() {
    let source = "class Parser<R> {}\n\
                      extension Ext<T1, T2> on Parser<(T1, T2)> {\n  \
                        Parser<R> map2<R>(R Function(T1, T2) f) => Parser<R>();\n}\n\
                      Parser<(A, B)> seq2<A, B>(Parser<A> a, Parser<B> b) => Parser<(A, B)>();\n\
                      Parser<String> letter() => Parser<String>();\n\
                      Parser<String> build() {\n  \
                        return seq2(letter(), letter()).map2((a, b) => a + b);\n}\n";
    let names = code_names(&report(source));
    assert!(!names.contains(&"undefined_method"), "{names:?}");
    assert!(!names.contains(&"undefined_getter"), "{names:?}");
}

/// The resolved head still gates: `Missing x` is the oracle's
/// `InvalidType` receiver — the extension's member stays uncredited
/// and the access keeps its own miss rows (the head-level
/// `type_resolution` verdict is unchanged).
#[test]
fn extension_invalid_head_still_suppresses() {
    let source = "extension _E on Missing { int get used => 0; }\n\
                      void f(Missing x) { x.used; }\n";
    let names = code_names(&report(source));
    assert!(names.contains(&"undefined_class"), "{names:?}");
    assert!(!names.contains(&"undefined_method"), "{names:?}");
    assert!(!names.contains(&"undefined_getter"), "{names:?}");
}

/// Diagnoses `source_m` importing `source_a` unprefixed: `a.dart`'s
/// public face joins `main.dart`'s unit scope (the plain-import twin
/// of [`report_prefixed`]).
fn report_imported(source_m: &str, source_a: &str) -> FileReport {
    let strings = StringTable::new();
    let mut files = FileTable::new();
    let file_a = files.intern(Path::new("a.dart"));
    let file_m = files.intern(Path::new("main.dart"));
    let (scanned_a, features, _) = scan(source_a, file_a);
    let (scanned_m, _, set) = scan(source_m, file_m);
    let elements_a = collect(source_a, &scanned_a.tokens, features, &strings);
    let elements_m = collect(source_m, &scanned_m.tokens, features, &strings);
    let mut builder = LibraryGraphBuilder::new(&strings);
    let lib_a = builder.intern("package:t/a.dart");
    let lib_m = builder.intern("package:t/main.dart");
    let mut index = ElementIndex::default();
    index.insert(file_a, elements_a.clone());
    index.insert(file_m, elements_m.clone());
    for (name, _) in elements_a.declarations() {
        builder.declare(lib_a, name, file_a);
    }
    for (name, decl) in elements_m.declarations() {
        if decl.kind == MicroKind::TypeParam {
            continue;
        }
        builder.declare(lib_m, name, file_m);
    }
    builder.import(
        lib_m,
        ImportEdge {
            target: Some(lib_a),
            prefix: None,
            deferred: false,
            combinators: Combinators::unfiltered(),
        },
    );
    let graph = builder.finish();
    let namespaces = Namespaces::build(&graph);
    let fold = fold_unit(
        source_m,
        &scanned_m.tokens,
        features,
        &strings,
        file_m,
        lib_m,
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

// —— #2284: the implicit-`this` extension application on a bare miss ——

/// `variable(other)` inside `class Expression` and inside
/// `extension ComparableExpr on Expression<DT>` resolves through the
/// imported `WithTypes on Expression<T>` face — the oracle's
/// `findExtension` applies the extension's member to `this` after the
/// member face and the scope both miss (drift's
/// `WithTypes<T> on Expression<T>` in `helpers.dart`). Before the fix
/// the bare call reported `undefined_method` naming `Expression`.
#[test]
fn bare_call_applies_imported_extension_member() {
    let report = report_imported(
        "import 'a.dart';\n\
             class Expression<D extends Object> {\n  \
               const Expression();\n  \
               Expression<bool> equals(D compare) => isE(variable(compare));\n  \
               Expression<bool> isE(Expression<D> other) => E<bool>();\n}\n\
             class E<D extends Object> extends Expression<D> {\n  \
               const E();\n}\n\
             extension ComparableExpr<DT extends Comparable<dynamic>>\n    \
                 on Expression<DT> {\n  \
               Expression<bool> bigger(DT other) => isE(variable(other));\n}\n",
        "import 'main.dart';\n\
             extension WithTypes<T extends Object> on Expression<T> {\n  \
               Variable<T> variable(T? value) => Variable<T>();\n}\n\
             class Variable<T> {}\n",
    );
    let names = code_names(&report);
    for code in [
        "undefined_method",
        "undefined_getter",
        "undefined_identifier",
    ] {
        assert!(!names.contains(&code), "{code} -> {names:?}");
    }
}

/// The same face inside a `mixin`'s `on` constraint — `this` carries
/// the constraint's member face (riverpod's
/// `$AsyncClassModifier on AnyNotifier` calling the imported
/// `ClassBaseX.requireElement`).
#[test]
fn bare_call_applies_extension_through_mixin_constraint() {
    let report = report_imported(
        "import 'a.dart';\n\
             class AnyNotifier<S, V> {}\n\
             mixin M<V> on AnyNotifier<V, V> {\n  \
               V get future => requireElement().state;\n}\n",
        "import 'main.dart';\n\
             extension X<S, V> on AnyNotifier<S, V> {\n  \
               AnyNotifier<S, V> requireElement() => this;\n  \
               V get state => throw '';\n}\n",
    );
    let names = code_names(&report);
    for code in [
        "undefined_method",
        "undefined_getter",
        "undefined_identifier",
    ] {
        assert!(!names.contains(&code), "{code} -> {names:?}");
    }
}

/// The miss still reports when no in-scope face carries the member:
/// the bare `absent()` inside the same class body keeps the oracle's
/// `undefined_method` for 'Expression'.
#[test]
fn bare_call_miss_still_reports_when_no_extension_carries() {
    let report = report_imported(
        "import 'a.dart';\n\
             class Expression<D extends Object> {\n  \
               Expression<bool> f(D compare) => absent(compare);\n}\n",
        "extension WithTypes<T extends Object> on Expression<T> {\n  \
               void variable(T? value) {}\n}\n",
    );
    let names = code_names(&report);
    assert!(names.contains(&"undefined_method"), "{names:?}");
}

// —— #2342: the broken-chain bare face + the dead-prefix creation head ——

/// Diagnoses `source_m` whose supertype hop resolves through a
/// DECLARED but unindexed library (`Walk::ChainBroken` — the hop has a
/// namespace entry while its file carries no model; the two-file rig
/// skips the second file's index insertion, which is exactly the
/// synthetic-SDK-face posture `report_with_core_faces` spells).
fn report_with_unindexed_super(source_m: &str, source_a: &str) -> FileReport {
    let strings = StringTable::new();
    let mut files = FileTable::new();
    let file_a = files.intern(Path::new("a.dart"));
    let file_m = files.intern(Path::new("main.dart"));
    let (scanned_a, features, _) = scan(source_a, file_a);
    let (scanned_m, _, set) = scan(source_m, file_m);
    let elements_a = collect(source_a, &scanned_a.tokens, features, &strings);
    let elements_m = collect(source_m, &scanned_m.tokens, features, &strings);
    let mut builder = LibraryGraphBuilder::new(&strings);
    let lib_a = builder.intern("package:t/a.dart");
    let lib_m = builder.intern("package:t/main.dart");
    let mut index = ElementIndex::default();
    index.insert(file_m, elements_m.clone());
    for (name, _) in elements_a.declarations() {
        builder.declare(lib_a, name, file_a);
    }
    for (name, decl) in elements_m.declarations() {
        if decl.kind == MicroKind::TypeParam {
            continue;
        }
        builder.declare(lib_m, name, file_m);
    }
    builder.import(
        lib_m,
        ImportEdge {
            target: Some(lib_a),
            prefix: None,
            deferred: false,
            combinators: Combinators::unfiltered(),
        },
    );
    let graph = builder.finish();
    let namespaces = Namespaces::build(&graph);
    let fold = fold_unit(
        source_m,
        &scanned_m.tokens,
        features,
        &strings,
        file_m,
        lib_m,
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

/// A bare miss inside a container whose supertype hop is
/// resolved-but-unindexed reports the member face: the oracle's
/// interface ends at the resolved part of the chain and still answers
/// the miss (probed: `class C extends Missing` + bare `zorch()` →
/// `undefined_method` for 'C'). The broken chain used to claim the
/// member face and silence the bare row (the star corpus's o-only
/// `undefined_method` family).
#[test]
fn broken_chain_bare_miss_reports_the_member_face() {
    let source_m = "class C implements Foot { void f() { zorch(); } }";
    let report = report_with_unindexed_super(source_m, "class Foot {}");
    let spans = rows(&report);
    let zorch = source_m.find("zorch").unwrap() as u32;
    assert_eq!(spans, vec![("undefined_method", zorch)], "{spans:?}");
}

/// The mirror half: a bare creation send of a scoped class inside the
/// same container resolves through the scope — the send is the class's
/// creation, never an implicit-this member (the drift corpus's
/// bare-creation `undefined_method` family, the retriage posture:
/// a `ChainBroken` member question falls through to the scope).
#[test]
fn broken_chain_bare_creation_resolves_the_scoped_class() {
    let source_m = "class Thing {}\n\
                        class C implements Foot { void f() { Thing(); } }";
    let report = report_with_unindexed_super(source_m, "class Foot {}");
    let names = code_names(&report);
    assert!(names.is_empty(), "{names:?}");
}

/// Negative pin: the member-SELECT face keeps its cascade suppression
/// over the same broken chain — `this.zorch()` stays silent (the row
/// face's frozen verdict owns that question; the bare-face fall-through
/// must not leak into it).
#[test]
fn broken_chain_member_select_keeps_the_cascade_silence() {
    let source_m = "class C implements Foot { void f() { this.zorch(); } }";
    let report = report_with_unindexed_super(source_m, "class Foot {}");
    let names = code_names(&report);
    assert!(names.is_empty(), "{names:?}");
}

/// `const fb.C<int>()` with `fb` in no scope: the head is the
/// NamedType's import prefix and its element is null — the oracle
/// reports `undefined_identifier` over the whole `fb.C` span and the
/// creation/type-argument faces never fire (probe: 1:17 length 4; the
/// head used to read `creation_with_non_type` at the prefix token).
#[test]
fn dead_prefix_const_creation_reports_identifier_face() {
    let source = "class int {} void f() { const fb.C<int>(); }";
    let report = report(source);
    let spans = rows(&report);
    let fb = source.find("fb").unwrap() as u32;
    assert_eq!(spans, vec![("undefined_identifier", fb)], "{spans:?}");
    let diagnostic = &report.diagnostics[0];
    assert_eq!(diagnostic.range.length, 4, "{:?}", diagnostic.range);
    assert_eq!(diagnostic.message, "Undefined name 'fb'.");
}

/// The keyword-less shape `fb.C()` reports the same face at the prefix
/// token alone (probe: 1:11 length 2 — the invocation parse resolves
/// the target identifier first); the miss never reads
/// `undefined_class`.
#[test]
fn dead_prefix_implicit_creation_reports_identifier_at_prefix() {
    let source = "void f() { fb.C(); }";
    let report = report(source);
    let spans = rows(&report);
    let fb = source.find("fb").unwrap() as u32;
    assert_eq!(spans, vec![("undefined_identifier", fb)], "{spans:?}");
    assert_eq!(report.diagnostics[0].range.length, 2);
}

/// A dead prefix's continuation counts against nothing: the typedef's
/// own-name homonym must not own the group (the analyzer corpus's
/// `typedef GeneratedTypeConstraint = shared.GeneratedTypeConstraint<…>`
/// counted its argument against the typedef being declared —
/// `wrong_number_of_type_arguments` beside the prefix's
/// `undefined_identifier`; the oracle counts against nothing).
#[test]
fn dead_prefix_annotation_group_counts_against_nothing() {
    let source = "class int {} typedef Ours = theirs.Ours<int>;";
    let report = report(source);
    let spans = rows(&report);
    let theirs = source.find("theirs").unwrap() as u32;
    assert_eq!(spans, vec![("undefined_identifier", theirs)], "{spans:?}");
}

// —— #2351: the Map ctor-arity family (creation selector faces) ——————————
//
// Every fixture below mirrors a face probed against oracle `dart analyze`
// (fvm default) this round; the code/message pair is the oracle's
// verbatim. The rig carries no dart:core, so the fixtures declare the
// container locally (`class Map<K, V>`) — the arms account against the
// resolved container's declared count either way (the corpus run answers
// `Map` through the graph, the rig through the declaration).
//
// The class-segment faces account against the CLASS's declared count
// (`_buildTypeArguments` takes the count from the type element's type
// parameters) and report the base code; the selector-segment faces of an
// explicit `new`/const creation report `_constructor` for ANY written
// count — the constructor's own name never declares type parameters, and
// the oracle keeps that verdict even when the count equals the class's
// arity (`new Map.fromIterables<int, int>` reports).

/// The analyzer-corpus twin (upstream Dart SDK source,
/// pkg/analyzer/lib/src/dart/element/type_algebra.dart:217 —
/// `Map<TypeParameterElement, DartType>.fromIterables(parameters,
/// types)`): the class-segment group accounts against the class's
/// declared count — `Map` declares 2, the group writes 2 → silent. The
/// selector name never enters the count question.
#[test]
fn map_class_segment_matching_arity_is_silent() {
    let report = report(
        "class Map<K, V> { Map.fromIterables(a, b); }\
         class A {} class B {} void f() { Map<A, B>.fromIterables([], []); }",
    );
    assert!(report.diagnostics.is_empty(), "{:?}", code_names(&report));
}

/// The class-segment mismatch keeps the base code and the class message —
/// `Map` written with 3 arguments reports `wrong_number_of_type_arguments`
/// ("The type 'Map' is declared with 2 …"), never the constructor variant
/// (the NamedType resolution face owns the group).
#[test]
fn map_class_segment_mismatch_reports_base_code() {
    let report = report(
        "class Map<K, V> { Map.fromIterables(a, b); }\
         class A {} class B {} class C {} void f() { Map<A, B, C>.fromIterables([], []); }",
    );
    assert_eq!(
        code_names(&report),
        ["wrong_number_of_type_arguments"],
        "{:?}",
        code_names(&report)
    );
    assert_eq!(
        report.diagnostics[0].message,
        "The type 'Map' is declared with 2 type parameters, but 3 type arguments were given."
    );
}

/// The explicit-creation selector face reports `_constructor` for ANY
/// written count — including a count EQUAL to the class's arity
/// (`new Map.fromIterables<int, int>` reports; oracle-probed). This is
/// the arm the class-arity gate must NOT reach: the selector segment of
/// an explicit creation never class-accounts.
#[test]
fn new_selector_group_reports_constructor_code_at_matching_count() {
    let report = report(
        "class Map<K, V> { Map.fromIterables(a, b); }\
         class A {} class B {} void f() { new Map.fromIterables<A, B>([], []); }",
    );
    assert_eq!(
        code_names(&report),
        ["wrong_number_of_type_arguments_constructor"],
        "{:?}",
        code_names(&report)
    );
    assert_eq!(
        report.diagnostics[0].message,
        "The constructor 'Map.fromIterables' doesn't have type parameters."
    );
}

/// The explicit-creation class-segment faces mirror the implicit ones:
/// `new Map<A, B>.fromIterables` is silent at the class's arity, the
/// 3-argument spelling reports the base code with the class message.
#[test]
fn new_class_segment_faces_match_class_arity() {
    let matched = report(
        "class Map<K, V> { Map.fromIterables(a, b); }\
         class A {} class B {} void f() { new Map<A, B>.fromIterables([], []); }",
    );
    assert!(matched.diagnostics.is_empty(), "{:?}", code_names(&matched));
    let mismatched = report(
        "class Map<K, V> { Map.fromIterables(a, b); }\
         class A {} class B {} class C {} void f() { new Map<A, B, C>.fromIterables([], []); }",
    );
    assert_eq!(
        code_names(&mismatched),
        ["wrong_number_of_type_arguments"],
        "{:?}",
        code_names(&mismatched)
    );
    assert_eq!(
        mismatched.diagnostics[0].message,
        "The type 'Map' is declared with 2 type parameters, but 3 type arguments were given."
    );
}

/// The unnamed creation (`new Map<A, B, C>()` — no selector segment)
/// keeps the class face: the base code through `check_annotation_arity`
/// (the `creation_ref` `None` arm), the pre-existing posture.
#[test]
fn unnamed_creation_mismatch_keeps_base_code() {
    let report = report(
        "class Map<K, V> {} class A {} class B {} class C {} void f() { new Map<A, B, C>(); }",
    );
    assert_eq!(
        code_names(&report),
        ["wrong_number_of_type_arguments"],
        "{:?}",
        code_names(&report)
    );
    assert_eq!(
        report.diagnostics[0].message,
        "The type 'Map' is declared with 2 type parameters, but 3 type arguments were given."
    );
}
