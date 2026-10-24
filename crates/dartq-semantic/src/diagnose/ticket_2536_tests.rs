//! The prefixed-type completion family (#2536): the official
//! `PrefixedType.parseType` protocol hands the semantic layer the PREFIX
//! token in `handleType` and the continuation leaf separately, so the
//! armed head must carry the dot-joined spelling (`io.File`) — the intern
//! table's one-head form — while every name-keyed consumer (the flow
//! registry's alias keys, the container index, the enum coverage space)
//! keeps the dot-free declaration name. The pins hold the two invariants
//! the family's faces depend on: the joined alias must never strand a
//! declaration's supertype edges (the registry inversion keys on the
//! declaration name), and the prefix-reached enum constant must cover in
//! the switch space.
//!
//! Oracle provenance (`dart analyze` 3.13.1): a bare-and-prefixed double
//! import of one library keeps `is`-promotions live (`state is
//! LibraryImportWithFile && !state.importedFile.exists` reports nothing
//! — the promoted face reads the subclass member), a prefixed negated
//! `is!` with early return promotes the trailing reads exactly like the
//! bare form, a prefixed-constant enum switch covers (`case p.Level.a`)
//! when every constant appears, and the sealed is-chain's else-branches
//! stay reachable (no `dead_code`).

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

/// The double-import rig: `package:t/main.dart` imports `kit.dart` BOTH
/// bare and prefixed (the analyzer-corpus shape — `file_state.dart`
/// twice), and returns the main file's report. `prefix` also allows a
/// single prefixed edge (`None` = one bare edge) for the single-face
/// controls.
fn double_import_report(main_source: &str, dependency: &str, prefix: Option<&str>) -> FileReport {
    let strings = StringTable::new();
    let mut files = FileTable::new();
    let file = files.intern(Path::new("main.dart"));
    let dep_file = files.intern(Path::new("kit.dart"));
    let set = FeatureSet::for_language_version(LanguageVersion::new(3, 15));
    let features = ParserFeatures::from_feature_set(set);
    let scanned = dartq_scanner::scan(
        main_source,
        ScanConfig {
            enable_triple_shift: true,
            enable_augmentations: true,
        },
        file,
    );
    assert!(
        scanned.diagnostics.is_empty(),
        "scanner diagnostics: {main_source:?}"
    );
    let scanned_dep = dartq_scanner::scan(
        dependency,
        ScanConfig {
            enable_triple_shift: true,
            enable_augmentations: true,
        },
        dep_file,
    );
    assert!(
        scanned_dep.diagnostics.is_empty(),
        "scanner diagnostics: {dependency:?}"
    );
    let elements = collect(main_source, &scanned.tokens, features, &strings);
    let dep_elements = collect(dependency, &scanned_dep.tokens, features, &strings);
    let mut builder = LibraryGraphBuilder::new(&strings);
    let lib = builder.intern("package:t/main.dart");
    let dep_lib = builder.intern("package:t/kit.dart");
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
    builder.import(
        lib,
        ImportEdge {
            target: Some(dep_lib),
            prefix: None,
            deferred: false,
            combinators: Combinators::unfiltered(),
        },
    );
    if let Some(p) = prefix {
        builder.import(
            lib,
            ImportEdge {
                target: Some(dep_lib),
                prefix: Some(strings.intern(p)),
                deferred: false,
                combinators: Combinators::unfiltered(),
            },
        );
    }
    let graph = builder.finish();
    let namespaces = Namespaces::build(&graph);
    let mut index = ElementIndex::default();
    index.insert(file, elements);
    index.insert(dep_file, dep_elements);
    diagnose_with(
        main_source,
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

/// The single-file no-import rig (the `operand_face_tests` shape): the
/// wave's function-typed-callee and covering faces live in one unit.
fn unit_report(source: &str) -> FileReport {
    let strings = StringTable::new();
    let mut files = FileTable::new();
    let file = files.intern(Path::new("main.dart"));
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
    assert!(
        scanned.diagnostics.is_empty(),
        "scanner diagnostics: {source:?}"
    );
    let elements = collect(source, &scanned.tokens, features, &strings);
    let mut builder = LibraryGraphBuilder::new(&strings);
    let lib = builder.intern("package:t/main.dart");
    for (name, decl) in elements.declarations() {
        if decl.kind == MicroKind::TypeParam {
            continue;
        }
        builder.declare(lib, name, file);
    }
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

fn codes(report: &FileReport) -> Vec<&'static str> {
    report.diagnostics.iter().map(|d| d.code.name).collect()
}

/// The analyzer-corpus shape (`library_analyzer.dart:750-758`): the
/// double import registers every container once, the joined alias keys
/// (`p.LibraryImportWithFile`) must not strand the declaration's
/// supertype edges — the else-if promotion keeps reading the subclass
/// member and the then-branch keeps the base member, no
/// `undefined_getter`, no `dead_code`.
#[test]
fn double_import_promotion_keeps_supertype_edges() {
    let report = double_import_report(
        r#"
import 'package:t/kit.dart';
import 'package:t/kit.dart' as p;

void h(LibraryImportState state) {
  if (state is LibraryImportWithUri) {
    state.importedSource;
  } else if (state is LibraryImportWithFile && state.importedFile.exists) {
    state.importedFile;
  }
}
"#,
        r#"
sealed class LibraryImportState {}
sealed class LibraryImportWithUri extends LibraryImportState {
  FileState get importedSource => FileState();
}
final class LibraryImportWithFile extends LibraryImportWithUri {
  FileState get importedFile => FileState();
}
final class FileState {
  bool exists = true;
}
"#,
        Some("p"),
    );
    let cs = codes(&report);
    assert!(
        !cs.contains(&"undefined_getter"),
        "undefined_getter leaked: {cs:?}"
    );
    assert!(!cs.contains(&"dead_code"), "dead_code leaked: {cs:?}");
}

/// The prefixed negated test (`is! p.X`) with early return promotes the
/// trailing reads exactly like the bare form: the joined armed head
/// resolves through the alias face and the receiver reads the subclass
/// member.
#[test]
fn prefixed_negated_is_promotes_after_early_return() {
    let report = double_import_report(
        r#"
import 'package:t/kit.dart' as p;

void g(p.LibraryImportState state) {
  if (state is! p.LibraryImportWithFile) {
    return;
  }
  state.importedFile;
}
"#,
        r#"
sealed class LibraryImportState {}
final class LibraryImportWithFile extends LibraryImportState {
  FileState get importedFile => FileState();
}
final class FileState {}
"#,
        Some("p"),
    );
    assert!(
        !codes(&report).contains(&"undefined_getter"),
        "codes: {:?}",
        codes(&report)
    );
}

/// The prefix-reached enum constant covers in the switch space
/// (`android_webview.ConsoleMessageLevel.debug`): a fully covered
/// prefixed-constant switch reports nothing, and the one-constant
/// control keeps reporting — the pin proves the coverage, not a
/// suppressed check.
#[test]
fn prefixed_enum_constant_cases_cover() {
    let full = double_import_report(
        r#"
import 'package:t/kit.dart';
import 'package:t/kit.dart' as p;

void f(Msg m) {
  switch (m.level) {
    case p.Level.a:
      m.level;
    case p.Level.b:
      m.level;
  }
}
"#,
        "enum Level { a, b }\n\nclass Msg { final Level level; Msg(this.level); }",
        Some("p"),
    );
    assert!(
        !codes(&full).contains(&"non_exhaustive_switch_statement"),
        "full cover reported: {:?}",
        codes(&full)
    );
    let partial = double_import_report(
        r#"
import 'package:t/kit.dart';
import 'package:t/kit.dart' as p;

void f(Msg m) {
  switch (m.level) {
    case p.Level.a:
      m.level;
  }
}
"#,
        "enum Level { a, b }\n\nclass Msg { final Level level; Msg(this.level); }",
        Some("p"),
    );
    assert!(
        codes(&partial).contains(&"non_exhaustive_switch_statement"),
        "partial control stayed silent: {:?}",
        codes(&partial)
    );
}

/// The sealed is-chain over prefixed spellings (`fs.dart`'s DirectiveUri
/// hierarchy): every else-if branch stays reachable — the factored
/// remainder of one test never swallows the next test's arm, no
/// `dead_code` on the branches.
#[test]
fn prefixed_sealed_is_chain_stays_reachable() {
    let report = double_import_report(
        r#"
import 'package:t/kit.dart' as p;

class Impl extends p.DirectiveUri {}

Impl f(p.DirectiveUri self) {
  if (self is p.WithSource) {
    return Impl();
  } else if (self is p.WithUri) {
    return Impl();
  } else if (self is p.WithString) {
    return Impl();
  }
  return Impl();
}
"#,
        r#"
sealed class DirectiveUri {}
final class WithSource extends DirectiveUri {}
final class WithString extends DirectiveUri {}
final class WithUri extends WithString {}
"#,
        Some("p"),
    );
    assert!(
        !codes(&report).contains(&"dead_code"),
        "dead_code leaked: {:?}",
        codes(&report)
    );
}

/// The function-typed callee's own shape (#2536): the invocation yields
/// the shape's return type — a typedef-typed field, a typedef-typed
/// parameter, and a raw inline function-typed parameter all type the
/// call's result as the return face, never the whole function type.
#[test]
fn function_typed_callee_returns_the_shapes_return() {
    let report = unit_report(
        r#"
typedef Handler = String Function(int x);

class Widget {
  final Handler handler;
  const Widget(this.handler);

  String invoke(int x) {
    return handler(x);
  }
}

num useMapper(num Function(String s) m, String s) {
  return m(s);
}
"#,
    );
    let cs = codes(&report);
    assert!(
        !cs.contains(&"return_of_invalid_type"),
        "return face leaked: {cs:?}"
    );
}

/// The null-checked scrutinee's covering verdict (#2538): the
/// `if (n == null) return;` promotion strips the null witness, so both
/// the statement switch and the `return switch` expression stay
/// exhaustive — no `non_exhaustive` row; the per-case reachability face
/// (`Num()` after `IntNum()`) keeps its row.
#[test]
fn null_promoted_scrutinee_decides_the_covering_gap() {
    let report = unit_report(
        r#"
sealed class Num {}
final class IntNum extends Num {}

int g(Num? n) {
  if (n == null) return 0;
  int r = 0;
  switch (n) {
    case IntNum():
      r = 1;
    case Num():
      r = 2;
  }
  return r;
}

int h(Num? n) {
  if (n == null) return 0;
  return switch (n) {
    IntNum() => 1,
    Num() => 2,
  };
}
"#,
    );
    let cs = codes(&report);
    assert!(
        !cs.contains(&"non_exhaustive_switch_statement")
            && !cs.contains(&"non_exhaustive_switch_expression"),
        "covering leaked: {cs:?}"
    );
    assert!(
        cs.contains(&"unreachable_switch_case"),
        "the per-case face keeps its row: {cs:?}"
    );
}

/// The promotion-latched initializer's re-bind (#2539): `final v = n.value`
/// inside an `is` promotion binds the accessor's return over the PROMOTED
/// receiver — the subclass override's `double`, never the base interface's
/// `Object` — so the downstream member read resolves on the re-bound face.
/// The widening controls stay pinned: a `!`-asserted bind keeps its
/// non-null face (the oracle's assert decided nullness) and an `as`-cast
/// bind keeps the written cast face.
#[test]
fn promotion_latched_initializer_rebinds_the_accessor_face() {
    let report = unit_report(
        r#"
sealed class Num {
  Object get value;
}

final class DNum extends Num {
  @override
  DNum get value => this;
  DNum get onlyHere => this;
  Object get marker => this;
}

void onlyIs(Num n) {
  if (n is DNum) {
    final v = n.value;
    final m = v.onlyHere;
    final a = n.onlyHere;
    final b = a.marker;
    final c = b.value;
  }
}

class Sdk {
  Sdk? get next => null;
  Sdk get languageVersion => this;
}

void keepsAssertFace(Sdk? sourceFactory) {
  var sdk = sourceFactory!.next!;
  final v = sdk.languageVersion;
  final w = v.next;
}

"#,
    );
    let cs = codes(&report);
    let family = cs
        .iter()
        .filter(|c| {
            **c == "undefined_getter" || **c == "unchecked_use_of_nullable_value" || **c == "dead_code"
        })
        .collect::<Vec<_>>();
    assert!(family.is_empty(), "faces leaked: {cs:?}");
}

/// The condition-variable face (language#1274): a bool local bound from a
/// null comparison replays its split when read as a condition operand —
/// `hasFlags && flags.contains(...)` and `!hasFlags || flags.contains(...)`
/// see `flags` promoted in the taken arm (oracle-pinned silent).
#[test]
fn condition_variable_replays_the_null_comparison_split() {
    let report = unit_report(
        r#"
class Flags {
  bool contains(String s) => false;
}

void sink(bool b) {}

void f(Flags? flags) {
  final hasFlags = flags != null;
  sink(hasFlags && flags.contains('q'));
  sink(!hasFlags || flags.contains('x'));
}
"#,
    );
    let cs = codes(&report);
    assert!(
        !cs.contains(&"unchecked_use_of_nullable_value"),
        "unchecked leaked: {cs:?}"
    );
}

/// The switch-case scrutinee promotion in the case body: `spec(context)`
/// calls the narrowed `call` class — the parse-time invocation face
/// defers and re-decides on the flow read type (#2547).
#[test]
fn switch_case_promotes_the_scrutinee_for_the_call() {
    let report = unit_report(
        r#"
class Expr {
  Object call(Object c) => c;
}

void f(Object spec) {
  switch (spec) {
    case Expr():
      final seq = spec(1);
      final other = spec(2);
  }
}
"#,
    );
    let cs = codes(&report);
    assert!(
        !cs.contains(&"invocation_of_non_function_expression")
            && !cs.contains(&"invocation_of_non_function"),
        "invocation face leaked: {cs:?}"
    );
}

/// The collect-rollback binding-journal invariant (Wave K): a collect
/// walk's declarations must not survive into the real walk's scope. The
/// leak this pins against (`driver.dart:2132-2242`, the analyzer corpus):
/// the collect walk's later same-named binding stayed in the pre-existing
/// scope after the length-only truncation, the if-case scrutinee read
/// then resolved the collect-only variable, and the pattern's non-null
/// narrowing landed on the wrong slot — the member read under it reported
/// `unchecked_use_of_nullable_value`. The undo journal (scope depth plus
/// generation) restores the pre-collect binding; the corpus differential
/// is the leak's regression net (the full-method topology that trips the
/// pre-journal engine spans more than this unit rig exercises), and this
/// pin holds the same-named pattern sites' clean baseline: no unchecked,
/// no undefined, no dead rows.
#[test]
fn collect_rollback_keeps_pattern_scrutinee_identity() {
    let report = unit_report(
        r#"
class W {
  W? next() => null;
}
class Box {
  W? take() => null;
}
void drive(void Function() g) {}
void f(Box box) {
  var ws = box.take();
  ws?.next();
  if (ws case var ws?) {
    ws.next();
  }
  drive(() {
    var inner = box.take();
    inner?.next();
  });
  if (ws case var ws?) {
    ws.next();
  }
}
"#,
    );
    let cs = codes(&report);
    assert!(
        !cs.contains(&"unchecked_use_of_nullable_value")
            && !cs.contains(&"undefined_getter")
            && !cs.contains(&"undefined_method")
            && !cs.contains(&"dead_code"),
        "collect rollback leaked a binding into the real walk: {cs:?}"
    );
}

/// The override-precision deferred re-bind (Wave L, #2553): a `var` local
/// initialized from a member read on an `is`-promoted receiver re-binds to
/// the subclass's own covariant member type in pass 2 — `Impl.element`'s
/// field `Sub2`, never the base getter's `Sub?`. Two seams held the fix:
/// the narrowing gate's cross-head arm must ask subtype-ness (the supertype
/// closure), not a member lookup — no member is SPELLED `Sub` — and the
/// consumer's local identity must key on the receiver read's own span (the
/// non-moving roster), because the member row's fold-chased `local_hits`
/// entry can name the outer declarator (`var n = c.m().k()`). The rig is
/// the analyzer corpus's `driver.dart`/`element.dart` shape at unit scale:
/// no unchecked on the narrowed read, nothing else either.
#[test]
fn promoted_receiver_member_rebinds_to_override_face() {
    let report = unit_report(
        r#"
class Sub {
  List<int> typeParameters = [];
}
class Sub2 extends Sub {
  List<int> typeParameters = [];
}
class Base {
  Sub? get element => null;
}
class Impl extends Base {
  final Sub2 element = Sub2();
}
void f(Base b) {
  var t = b;
  if (t is! Impl) return;
  var c = t.element;
  var n = c.typeParameters.length;
}
"#,
    );
    let cs = codes(&report);
    assert!(
        !cs.contains(&"unchecked_use_of_nullable_value"),
        "override-precision re-bind did not land: {cs:?}"
    );
}

/// The operator operand's re-bound face (xml's number.dart rig): a `final`
/// local initialized from a getter read on an `is`-promoted receiver keeps
/// the parse-time bind (the base getter's `Object`) in the local table, so
/// a later operator application on the local reported `undefined_operator`
/// where the oracle resolves the override's `BigInt`. The pass-2 verdict
/// reads the same re-bound static type the receiver faces read
/// (`rebound_read_face` outranks the parse bind).
#[test]
fn rebound_local_feeds_operator_operand() {
    let report = unit_report(
        r#"
sealed class N {
  Object get value => this;
}
final class I extends N {
  BigInt get value => BigInt.zero;
}
void f(N arg) {
  if (arg is I) {
    final v = arg.value;
    print(v ~/ BigInt.two);
  }
}
"#,
    );
    let cs = codes(&report);
    assert!(
        !cs.contains(&"undefined_operator"),
        "operator operand missed the re-bound face: {cs:?}"
    );
}
