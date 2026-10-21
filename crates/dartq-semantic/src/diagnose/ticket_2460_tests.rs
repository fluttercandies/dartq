//! The #2460 pins: the flow-promotion batch A2 (census ⑨ decomposition,
//! `non-analyzer-clusters` §A2a–A2d) — the promotion mechanisms whose
//! wrong rows the oracle never emits.
//!
//! Oracle provenance: `dart analyze` silence on every shape below
//! (probe2443 corpus rounds), anchored against upstream Dart SDK source,
//! pkg/_fe_analyzer_shared/lib/src/flow_analysis/flow_analysis.dart:
//! a generator body's `yield` demotes only inside a local function
//! (`suspension`, :7909-7950 — `_enclosingFunctionExpressionInfoStack`
//! must be non-empty), a plain assignment writes the assignment
//! expression's own static type (`assignment_expression_resolver.dart`
//! `flow.write` — non-null for `e!` right sides), a `var`-inferred local
//! binds the initializer expression's static type (`_inferLocalVariable`
//! — the field's declared nullable face, never an earlier write's RHS),
//! and `assert_afterCondition` (:6478-6485) runs the message under the
//! negated-condition state.
//!
//! Lane split: the flow interpreter's generator yield, non-null-assert,
//! and write edges are faithful to the official decision order (the
//! flow-crate `ticket_2460_tests` pins prove the promotion facts), so
//! each remaining divergence's fix lands at its semantic check site —
//! `check_yield`, the assignment-RHS stager, the `staged_write_tys`
//! inference feed, and the `assert_after_condition` forward, all in
//! `diagnose/listener.rs`/`engine_core.rs` outside this ticket's file
//! grant. Those pins ride `#[ignore]`d with the fix site named, the
//! house pattern for pending oracle-truth rows; they flip green with the
//! fix and assert the full oracle-exact set.

use crate::collector::collect;
use crate::diagnose::{FileReport, StrictModes, diagnose_with};
use crate::library::ElementIndex;
use crate::model::MicroKind;
use dartq_language::{FeatureSet, LanguageVersion};
use dartq_parser::ParserFeatures;
use dartq_resolution::{LibraryClassModifiers, LibraryGraphBuilder, Namespaces};
use dartq_scanner::ScanConfig;
use dartq_source::{FileTable, StringTable};
use std::path::Path;

/// The one-library rig: `source` is the sole analyzed file of
/// `package:t`, no imports (the #2453 rig — core types are declared
/// locally so their nullability faces resolve).
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

fn codes(report: &FileReport) -> Vec<&str> {
    report
        .diagnostics
        .iter()
        .map(|d| d.code.name)
        .collect()
}

// —— A2a: generator-body promotion loss ——————————————————————————————————————

/// The corpus shape (collection `iterable_extensions.dart:531`,
/// `chunk ??= []` then `yield chunk` in a `sync*` walker): the `??=`
/// write promotes `chunk` to the non-null element face and the yield is
/// clean — the oracle's `suspension` demotes nothing at a generator
/// body's own top level (flow_analysis.dart:7909-7950: only a local
/// function's suspension point walks its read set).
///
/// `#[ignore]`d pending the semantic fix: `check_yield`
/// (`diagnose/listener.rs`) emits `yield_of_invalid_type` from the
/// parse-time row (the declared `List<int>?`) instead of staging the
/// verdict for the teardown re-decision the way `check_return` does
/// against the promoted-read facts — the flow layer's promoted read
/// type is correct (`crates/dartq-flow` `ticket_2460_tests`).
#[test]
#[ignore = "fix site: check_yield (diagnose/listener.rs) must stage for the teardown promotion re-decision"]
fn a2a_generator_ifnull_write_then_yield_stays_promoted() {
    let source = "\
class int {}
class List<T> {}
class Iterable<T> {}

Iterable<List<int>> walk(List<List<int>>? chunk) sync* {
  chunk ??= <List<int>>[];
  yield chunk;
}
";
    let report = unit_report(source);
    assert_eq!(
        codes(&report),
        Vec::<&str>::new(),
        "A2a ??= then yield: {:#?}",
        codes(&report)
    );
}

/// The same face over an `!= null` guard (xml `xpath` `sync*` walkers):
/// the guard promotes `node` for the whole then-branch, and the yield
/// carries the promoted type — the oracle is silent.
#[test]
#[ignore = "fix site: check_yield (diagnose/listener.rs) must stage for the teardown promotion re-decision"]
fn a2a_generator_null_guard_then_yield_stays_promoted() {
    let source = "\
class Node {}
class Iterable<T> {}

Iterable<Node> walk(Iterable<Node> source, Node? node) sync* {
  if (node != null) {
    yield node;
  }
}
";
    let report = unit_report(source);
    assert_eq!(
        codes(&report),
        Vec::<&str>::new(),
        "A2a guard then yield: {:#?}",
        codes(&report)
    );
}

/// The `async*` twin (probe2443 `g4`): the promotion stands across the
/// await-for head too — the generator body's top level is not a closure,
/// so no suspension demotion applies on the way to the yield.
#[test]
#[ignore = "fix site: check_yield (diagnose/listener.rs) must stage for the teardown promotion re-decision"]
fn a2a_async_generator_write_await_yield_stays_promoted() {
    let source = "\
class int {}
class Stream<T> {}

Stream<int> walk(Stream<int> source, int? chunk) async* {
  chunk ??= 0;
  await for (final v in source) {
    yield chunk;
  }
}
";
    let report = unit_report(source);
    assert_eq!(
        codes(&report),
        Vec::<&str>::new(),
        "A2a async twin: {:#?}",
        codes(&report)
    );
}

// —— A2b: null-assertion edges ———————————————————————————————————————————————

/// The corpus shape (args `command_runner.dart:190-196`,
/// probe2443 `M4`): `command = cmds.at('x')!` writes the assignment
/// expression's own static type — the `!`-stripped non-null face — so
/// `command` reads promoted and the member access is clean.
///
/// `#[ignore]`d pending the semantic fix: the assignment-RHS stager
/// (`handle_assignment_expression`, `diagnose/listener.rs`) recomputes
/// `callee_return_ty` for an invoke row, dropping the non-null rewrite
/// `check_null_assert` left on the row — the staged face stays nullable,
/// the flow write promotes nothing, and the member read reports
/// `unchecked_use_of_nullable_value`.
#[test]
#[ignore = "fix site: assignment-RHS stager (diagnose/listener.rs) must keep the ! rewrite over callee_return_ty"]
fn a2b_assign_bang_result_promotes_target() {
    let source = "\
class String {}
class Cmd {
  void run() {}
}
class Cmds {
  Cmd? at(String name) => null;
}

void execute(Cmds cmds) {
  Cmd? command;
  command = cmds.at('x')!;
  command.run();
}
";
    let report = unit_report(source);
    assert_eq!(
        codes(&report),
        Vec::<&str>::new(),
        "A2b assign bang result: {:#?}",
        codes(&report)
    );
}

/// The path twin (`path_map.dart:33`): inside the closure the `!`
/// re-establishes the non-null receiver for the member access — the
/// assertion's operand read rewrites to the non-null face the merge
/// consumes (`check_null_assert`'s `promoteToNonNull` rewrite, the
/// oracle's `PropertyAccess.staticType`), and the pass-2 flow face for
/// the captured read is exactly the `!`'s own pre-state (never a
/// promotion). This face is green: the pin holds it.
#[test]
fn a2b_bang_narrows_member_receiver_in_closure() {
    let source = "\
class String {}
class bool {}
class Context {
  bool equals(String? a, String? b) => true;
}
class P {
  Context get context => Context();
}

bool run(P p, String? a, String? b) {
  Context? context;
  context ??= p.context;
  var probe = () {
    return context!.equals(a, b);
  };
  return probe();
}
";
    let report = unit_report(source);
    assert_eq!(
        codes(&report),
        Vec::<&str>::new(),
        "A2b bang receiver: {:#?}",
        codes(&report)
    );
}

// —— A2c: field nullability pipeline —————————————————————————————————————————

/// The corpus shape (collection `combined_iterator.dart:22,29`,
/// probe2443 `M2b`): the field's declared nullable face rides the
/// `var`-inferred local — an earlier constructor-initializer write's
/// RHS type must not leak into the inference (`staged_write_tys` is the
/// locals' own flow-aware feed, never a field's declared-type source),
/// so the null check is live and the oracle is silent.
///
/// `#[ignore]`d pending the semantic fix: the feed's insert
/// (`diagnose/listener.rs`) keys by bare name and its read
/// (`staged_read_ty`, `engine_core.rs`) gates on frame depth only — a
/// field write in one executable leaks its RHS type into a later
/// executable's `var` inference at the same depth, and the local binds
/// the non-null face.
#[test]
#[ignore = "fix site: staged_write_tys feed (diagnose/listener.rs insert, engine_core.rs staged_read_ty) must not cross executables for non-locals"]
fn a2c_nullable_field_read_infers_nullable_local() {
    let source = "\
class It<T> {}
class C<T> {
  It<It<T>>? _iterators;

  C(It<It<T>> iterators) : _iterators = iterators;

  void move() {
    var iterators = _iterators;
    if (iterators != null) {
      iterators.hashCode;
    }
  }
}
";
    let report = unit_report(source);
    assert_eq!(
        codes(&report),
        Vec::<&str>::new(),
        "A2c declared-nullable field: {:#?}",
        codes(&report)
    );
}

/// The archive twin (`_PortSink`, `int _staged = 0;`): the written
/// non-nullable field type is what every read answers — `_staged` as an
/// argument reads `int`, never `int?`, so the `setRange` call is clean.
/// This face is green in the plain-class posture; the corpus's remaining
/// rows need the unresolved-`OutputStream` superclass context to
/// reproduce (the census's own open trigger variable).
#[test]
fn a2c_written_non_nullable_field_reads_non_nullable() {
    let source = "\
class Ctx {}
class List<T> {}
class Uint8List {
  void setRange(Ctx start, Ctx end, List<Ctx> bytes) {}
}
class OutputStream {}

class PortSink extends OutputStream {
  final Uint8List _staging = Uint8List();
  Ctx _staged = Ctx();

  void writeBytes(List<Ctx> bytes) {
    _staging.setRange(_staged, _staged, bytes);
  }
}
";
    let report = unit_report(source);
    assert_eq!(
        codes(&report),
        Vec::<&str>::new(),
        "A2c non-nullable field read: {:#?}",
        codes(&report)
    );
}

// —— A2d: assert-message state ———————————————————————————————————————————————

/// The corpus shape (intl `locale_extensions.dart:41`): the message
/// expression runs under the negated condition (`assert_afterCondition`,
/// flow_analysis.dart:6478-6485) — `!(x == null || …)` implies
/// `x != null`, so the message's `x.length` reads the promoted
/// non-null receiver and the oracle is silent.
///
/// `#[ignore]`d pending the semantic fix: `DiagnoseListener` never
/// forwards the parser's `assert_after_condition` event
/// (`diagnose/listener.rs` — the flow recorder's
/// `assert_after_condition`/`Ev::AssertAfterCondition` arm is
/// implemented and faithful), so the message analyzes under the
/// unconditioned entry state.
#[test]
#[ignore = "fix site: forward assert_after_condition to the flow recorder (DiagnoseListener, diagnose/listener.rs)"]
fn a2d_assert_message_reads_negated_condition_state() {
    let source = "\
class int {}
class String {}
class Box {
  int length = 0;
  String toString() => 'box';
}

void f(Box? x) {
  assert(x == null || x.length > 0, 'size: ' + x.length.toString());
}
";
    let report = unit_report(source);
    assert_eq!(
        codes(&report),
        Vec::<&str>::new(),
        "A2d assert message: {:#?}",
        codes(&report)
    );
}

/// The closure-message twin: the message closure's body answers to the
/// same negated-condition state (the closure enters while `current` is
/// the condition-false arm), so the guarded read inside is clean.
#[test]
#[ignore = "fix site: forward assert_after_condition to the flow recorder (DiagnoseListener, diagnose/listener.rs)"]
fn a2d_assert_closure_message_reads_negated_condition_state() {
    let source = "\
class int {}
class String {}
class Box {
  int length = 0;
}

void f(Box? x) {
  assert(x == null || x.length > 0, () {
    return 'size: ' + x.length.toString();
  });
}
";
    let report = unit_report(source);
    assert_eq!(
        codes(&report),
        Vec::<&str>::new(),
        "A2d assert closure message: {:#?}",
        codes(&report)
    );
}
