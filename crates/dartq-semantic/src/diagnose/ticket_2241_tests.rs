//! #2241 pins: the unchecked-use receiver verdict against the flow's
//! variable promotions.
//!
//! A `var`-inferred local whose initializer is nullable (`var files =
//! this._files;`) promoted by a null check reads non-null at its member
//! accesses — the oracle is silent. The member walk's receiver verdict
//! re-decides against the flow's staged promotion ([`PromotionStaging`]):
//! the opaque head (`Ty::Unknown`, the lattice's only spelling for the
//! promoted `var`-inferred local) substitutes the parse-time type's
//! non-null form, exactly like `filter_staged`'s general arm. The
//! `== null` branch (promoted to `Null`), the else branch, and the
//! unguarded read keep the report.

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

fn code_names(report: &FileReport) -> Vec<String> {
    report
        .diagnostics
        .iter()
        .map(|d| format!("{}@{}", d.code.name, d.range.offset))
        .collect()
}

fn unchecked(report: &FileReport) -> Vec<String> {
    code_names(report)
        .into_iter()
        .filter(|name| name.starts_with("unchecked_use_of_nullable_value"))
        .collect()
}

fn with_prefix(report: &FileReport, prefix: &str) -> Vec<String> {
    code_names(report)
        .into_iter()
        .filter(|name| name.starts_with(prefix))
        .collect()
}

/// Cell a: `var`-inferred from an implicit-this field read, `!= null`
/// then-branch. The oracle is silent.
#[test]
fn pin_var_inferred_field_init_then_silent() {
    let source = "\
class int {}
class Box {
  int length = 0;
}
class C {
  Box? _files;
  void a() {
    var files = _files;
    if (files != null) {
      sink(files.length);
    }
  }
}
void sink(int x) {}
";
    let report = unit_report(source);
    assert!(
        unchecked(&report).is_empty(),
        "cell a: {:#?}",
        code_names(&report)
    );
}

/// Cell b: `var`-inferred from a nullable-returning invocation,
/// `!= null` then-branch. The oracle is silent.
#[test]
fn pin_var_inferred_invoke_init_then_silent() {
    let source = "\
class int {}
class Box {
  int length = 0;
}
class C {
  Box? getter() => null;
  void b() {
    var files = getter();
    if (files != null) sink(files.length);
  }
}
void sink(int x) {}
";
    let report = unit_report(source);
    assert!(
        unchecked(&report).is_empty(),
        "cell b: {:#?}",
        code_names(&report)
    );
}

/// Cell c (control): the explicit `Box?` declaration with an assignment
/// initializer behaved before the fix.
#[test]
fn pin_explicit_declared_then_silent() {
    let source = "\
class int {}
class Box {
  int length = 0;
}
class C {
  Box? _files;
  void c() {
    Box? files;
    files = _files;
    if (files != null) sink(files.length);
  }
}
void sink(int x) {}
";
    let report = unit_report(source);
    assert!(
        unchecked(&report).is_empty(),
        "cell c: {:#?}",
        code_names(&report)
    );
}

/// Cell d: `var`-inferred from an explicit `this.` field read,
/// `!= null` then-branch. The oracle is silent.
#[test]
fn pin_var_inferred_this_init_then_silent() {
    let source = "\
class int {}
class Box {
  int length = 0;
}
class C {
  Box? _files;
  void d() {
    var files = this._files;
    if (files != null) sink(files.length);
  }
}
void sink(int x) {}
";
    let report = unit_report(source);
    assert!(
        unchecked(&report).is_empty(),
        "cell d: {:#?}",
        code_names(&report)
    );
}

/// Cell i (control): the parameter form behaved before the fix.
#[test]
fn pin_parameter_then_silent() {
    let source = "\
class int {}
class Box {
  int length = 0;
}
void i(Box? p) {
  if (p != null) sink(p.length);
}
void sink(int x) {}
";
    let report = unit_report(source);
    assert!(
        unchecked(&report).is_empty(),
        "cell i: {:#?}",
        code_names(&report)
    );
}

/// Cell e: the `== null` then-branch promotes to `Null` — the oracle
/// reports, and the staged `Null` face must keep the verdict.
#[test]
fn pin_null_branch_keeps_report() {
    let source = "\
class int {}
class Box {
  int length = 0;
}
class C {
  Box? _files;
  void e() {
    var files = this._files;
    if (files == null) {
      sink(files.length);
    }
  }
}
void sink(int x) {}
";
    let report = unit_report(source);
    assert!(
        !unchecked(&report).is_empty(),
        "cell e lost the report: {:#?}",
        code_names(&report)
    );
}

/// Cell g: the else branch of `!= null` — the oracle reports.
#[test]
fn pin_else_branch_keeps_report() {
    let source = "\
class int {}
class Box {
  int length = 0;
}
class C {
  Box? _files;
  void g() {
    var files = this._files;
    if (files != null) {
    } else {
      sink(files.length);
    }
  }
}
void sink(int x) {}
";
    let report = unit_report(source);
    assert!(
        !unchecked(&report).is_empty(),
        "cell g lost the report: {:#?}",
        code_names(&report)
    );
}

/// Cell h: the unguarded read — no promotion stands, the oracle reports.
#[test]
fn pin_unguarded_keeps_report() {
    let source = "\
class int {}
class Box {
  int length = 0;
}
class C {
  Box? _files;
  void h() {
    var files = this._files;
    sink(files.length);
  }
}
void sink(int x) {}
";
    let report = unit_report(source);
    assert!(
        !unchecked(&report).is_empty(),
        "cell h lost the report: {:#?}",
        code_names(&report)
    );
}

// —— initializer-closure cells (the #2241 rider): a closure nested in a
// —— top-level variable initializer analyzes its own locals like any
// —— function body — the flow world's framing must survive the closure
// —— body's nested local-initializer brackets. The oracle is silent on
// —— every guarded read below (probed: `dart analyze` 3.13.1).

/// Guarded member read inside the initializer closure: the oracle is
/// silent. Before the flow fix the closure's promotion never formed and
/// the read false-reported.
#[test]
fn pin_initializer_closure_guarded_read_silent() {
    let source = "\
class int {}
class Box {
  int length = 0;
}
Box? mk() => null;
final f = () {
  final files = mk();
  if (files == null) return 0;
  return files.length;
};
";
    let report = unit_report(source);
    assert!(
        unchecked(&report).is_empty(),
        "initializer closure: {:#?}",
        code_names(&report)
    );
}

/// Guarded argument read inside the initializer closure: the staged
/// assignability face sees the promotion — the oracle reports no
/// argument-type error (`take(files)` passes a promoted-non-null value).
#[test]
fn pin_initializer_closure_guarded_argument_silent() {
    let source = "\
class int {}
class Box {
  int length = 0;
}
Box? mk() => null;
void take(Box b) {}
final f = () {
  final files = mk();
  if (files == null) return 0;
  take(files);
  return 1;
};
";
    let report = unit_report(source);
    let assignability: Vec<String> = code_names(&report)
        .into_iter()
        .filter(|name| name.starts_with("argument_type_not_assignable"))
        .collect();
    assert!(
        assignability.is_empty(),
        "initializer closure argument: {:#?}",
        code_names(&report)
    );
}

/// The promotion also kills the branch inside the initializer closure:
/// after the early-return guard the second `== null` test is known-false,
/// so its then-branch is dead — the oracle reports `dead_code` there,
/// same as in a plain function body.
#[test]
fn pin_initializer_closure_promotion_deads_branch() {
    let source = "\
class int {}
class Box {
  int length = 0;
}
Box? mk() => null;
final f = () {
  final files = mk();
  if (files == null) return 0;
  if (files == null) {
    mk();
  }
  return 1;
};
";
    let report = unit_report(source);
    let dead: Vec<String> = code_names(&report)
        .into_iter()
        .filter(|name| name.starts_with("dead_code"))
        .collect();
    assert!(
        !dead.is_empty(),
        "initializer closure lost the dead row: {:#?}",
        code_names(&report)
    );
}

// —— #2285: the promotion plumbing reaches the bare-row and extension
// —— resolution faces. Each pin keeps a control within the same source
// —— where the face can fire, so the assertion is not vacuous.

/// #2285 item 1: a bare row's index operator resolves against the flow's
/// promoted type. `o` is declared `O`; the `is!` guard promotes it to
/// `A`, whose `[]` applies. The unguarded control keeps the
/// `undefined_operator` miss (the operator face itself fires in this
/// harness).
#[test]
fn pin_bare_operator_reads_the_flow_promotion() {
    let source = "\
class int {}
class O {}
class A extends O {
  int operator [](int index) => 0;
}
void guarded(O o) {
  if (o is! A) return;
  o[0];
}
void unguarded(O p) {
  p[0];
}
";
    let report = unit_report(source);
    let misses = with_prefix(&report, "undefined_operator");
    assert_eq!(
        misses.len(),
        1,
        "guarded index read lost the promotion: {:#?}",
        code_names(&report)
    );
}

/// #2285 item 3: the operator-merged member row over an `Object` face
/// member. `hashCode` is defined for every class, but the walk cannot
/// reach the unindexed root — the plain member arm exempts the
/// `Object` members, and the operator arm owes the same exemption
/// (`inner.hashCode * 5` is a getter read plus an operator check on its
/// result).
#[test]
fn pin_operator_merged_object_member_reads_clean() {
    let source = "\
class int {}
class A {}
class C {
  A inner = A();
  int f() => inner.hashCode * 5;
}
";
    let report = unit_report(source);
    assert!(
        with_prefix(&report, "undefined_").is_empty(),
        "operator-merged Object member reported a miss: {:#?}",
        code_names(&report)
    );
}

/// #2285 item 4: the `dart:collection` alias entries reach their
/// declared interfaces — `HashMap <: Map` through the alias table (the
/// stub declarations the declared-lattice walk cannot see). The
/// extension on `Map<K, V>` applies to a `HashMap` receiver.
#[test]
fn pin_collection_alias_reaches_its_interface() {
    let source = "\
class int {}
class Map<K, V> {}
class HashMap<K, V> {}
extension MapSize<K, V> on Map<K, V> {
  int get size => 0;
}
void f(HashMap<String, int> m) {
  m.size;
}
";
    let report = unit_report(source);
    assert!(
        with_prefix(&report, "undefined_").is_empty(),
        "HashMap did not reach Map: {:#?}",
        code_names(&report)
    );
}

/// #2285 item 5: the assignment window's receiver read (`outer =
/// outer.julianday`) resolves on the promoted face, so the extension on
/// `Expression<DateTime>` applies. The raw declared `Expression`
/// receiver would pad `dynamic` into the `on` argument and fail the
/// gate, reporting the getter as undefined.
#[test]
fn pin_assignment_window_receiver_reads_promoted_face() {
    let source = "\
class bool {}
class double {}
class DateTime {}
class Expression<T> {}
extension DateTimeExpressions on Expression<DateTime> {
  Expression<double> get julianday => Expression<double>();
}
class C {
  final bool cond = true;
  final Expression target = Expression();
  void f() {
    var outer = this.target;
    if (cond) {
      if (outer is Expression<DateTime>) outer = outer.julianday;
    }
  }
}
void main() {}
";
    let report = unit_report(source);
    assert!(
        with_prefix(&report, "undefined_").is_empty(),
        "assignment-window receiver lost the promotion: {:#?}",
        code_names(&report)
    );
}

/// #2285 item 6: a `dynamic` receiver argument subtypes a top-shaped
/// `on` argument (`dynamic <: Object?`), so the extension on
/// `List<Object?>` applies to `List<dynamic>`.
#[test]
fn pin_dynamic_type_argument_reaches_top_on_argument() {
    let source = "\
class int {}
class Object {}
class List<E> {}
extension ListExt on List<Object?> {
  int get ext => 0;
}
void f(List<dynamic> values) {
  values.ext;
}
";
    let report = unit_report(source);
    assert!(
        with_prefix(&report, "undefined_").is_empty(),
        "dynamic argument did not reach Object?: {:#?}",
        code_names(&report)
    );
}

/// #2285 item 7: a condition over a callable whose alias head is out of
/// scope (`Predicate<T>` declared in another file, never imported) is
/// undecidable at the name level — the condition faces stay silent. The
/// condition operand's type keeps the unresolved alias head (the send's
/// result is beyond the name-level model there), and an undecidable
/// head must not decide a non-`bool` verdict.
#[test]
fn pin_unresolved_alias_condition_stays_silent() {
    let source = "\
class T {}
Predicate<T> createMatcher() => (T value) => true;
void f(T value) {
  final tester = createMatcher();
  if (tester(value)) {}
}
";
    let report = unit_report(source);
    assert!(
        with_prefix(&report, "non_bool_condition").is_empty(),
        "unresolved alias head fired a condition verdict: {:#?}",
        code_names(&report)
    );
}
